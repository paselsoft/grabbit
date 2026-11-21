// SPDX-License-Identifier: GPL-3.0-or-later
// Adattato da Dropall 0.4.3 di Marco Giannini (https://github.com/marcogiannini/dropall),
// stessa licenza GPL-3.0-or-later.
//! Download manager: scarica file qualsiasi da un link HTTP/HTTPS.
//!
//! - **Più connessioni**: il file viene diviso in blocchi scaricati in parallelo con
//!   richieste `Range` (quando il server le supporta); chi finisce prima prende il blocco
//!   successivo, così nessuna connessione resta ferma.
//! - **Code**: ogni coda scarica i suoi file uno dopo l'altro; code diverse sono indipendenti.
//! - **Pianificatore**: ogni coda può partire e fermarsi da sola in certi orari e giorni.
//! - **Limite di velocità**: un tetto unico, condiviso da tutte le connessioni.
//! - **Pausa e ripresa**: lo stato (anche i blocchi già scaricati) è salvato su disco.

use crate::tr;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_CONNECTIONS: u8 = 16;
/// Un blocco non è mai più piccolo di così (a meno che il file lo sia).
const MIN_CHUNK: u64 = 512 * 1024;
const MAX_CHUNK: u64 = 64 * 1024 * 1024;
const READ_BUF: usize = 32 * 1024;
const MAX_RETRIES: u32 = 6;

// ───────────────────────────── dati ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Queued,
    Downloading,
    Paused,
    Done,
    Failed,
}

/// Un blocco del file: byte `start..=end`, `pos` è il prossimo byte da scaricare.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Seg {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub url: String,
    pub file_name: String,
    pub dir: PathBuf,
    pub queue: u64,
    pub conns: u8,
    pub state: State,
    pub error: String,
    pub total: Option<u64>,
    /// il server accetta richieste a intervalli (quindi pausa/ripresa e più connessioni)
    pub resumable: bool,
    pub probed: bool,
    pub segs: Vec<Seg>,
    pub downloaded: u64,
    pub final_path: Option<PathBuf>,
    #[serde(skip)]
    pub speed: f64,
}

impl Item {
    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(t) if t > 0 => (self.downloaded as f64 / t as f64).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        }
    }

    pub fn eta_secs(&self) -> Option<u64> {
        let total = self.total?;
        if self.speed < 1.0 || self.state != State::Downloading {
            return None;
        }
        Some((total.saturating_sub(self.downloaded) as f64 / self.speed) as u64)
    }
}

/// Orari in cui una coda parte e si ferma da sola.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    /// minuti dalla mezzanotte
    pub start_min: u16,
    pub stop_enabled: bool,
    pub stop_min: u16,
    /// lunedì … domenica (il giorno è quello in cui l'intervallo inizia)
    pub days: [bool; 7],
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            start_min: 2 * 60,
            stop_enabled: true,
            stop_min: 7 * 60,
            days: [true; 7],
        }
    }
}

impl Schedule {
    /// `weekday`: 0 = lunedì; `minute`: minuti dalla mezzanotte.
    /// Senza orario di stop, "attivo" vale dall'ora di inizio fino a fine giornata.
    pub fn active(&self, weekday: usize, minute: u16) -> bool {
        let d = weekday % 7;
        let (start, stop) = (self.start_min, self.stop_min);
        if !self.stop_enabled {
            return self.days[d] && minute >= start;
        }
        if stop > start {
            self.days[d] && minute >= start && minute < stop
        } else if stop < start {
            // l'intervallo passa la mezzanotte
            (self.days[d] && minute >= start) || (self.days[(d + 6) % 7] && minute < stop)
        } else {
            false
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Queue {
    pub id: u64,
    pub name: String,
    pub schedule: Schedule,
    #[serde(skip)]
    pub running: bool,
    #[serde(skip)]
    sched_prev: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub items: Vec<Item>,
    pub queues: Vec<Queue>,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    next_id: u64,
    queues: Vec<Queue>,
    items: Vec<Item>,
}

// ───────────────────────────── limite di velocità ─────────────────────────────

/// Tetto di velocità condiviso: ogni lettura "prenota" il proprio tempo di trasmissione.
pub struct Limiter {
    bps: AtomicU64,
    next: Mutex<Instant>,
}

impl Limiter {
    pub fn new() -> Self {
        Self {
            bps: AtomicU64::new(0),
            next: Mutex::new(Instant::now()),
        }
    }

    /// 0 = nessun limite
    pub fn set_kbps(&self, kbps: u32) {
        self.bps.store(kbps as u64 * 1024, Ordering::Relaxed);
    }

    pub fn bytes_per_sec(&self) -> u64 {
        self.bps.load(Ordering::Relaxed)
    }

    /// Aspetta il proprio turno per `n` byte; esce subito se `stop()` diventa vero.
    pub fn take(&self, n: u64, stop: &dyn Fn() -> bool) {
        let bps = self.bytes_per_sec();
        if bps == 0 {
            return;
        }
        let start = {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let start = (*next).max(now);
            *next = start + Duration::from_secs_f64(n as f64 / bps as f64);
            start
        };
        loop {
            let now = Instant::now();
            if now >= start || stop() {
                return;
            }
            thread::sleep((start - now).min(Duration::from_millis(100)));
        }
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────── stato condiviso ─────────────────────────────

const STOP_NONE: u8 = 0;
const STOP_PAUSE: u8 = 1;
/// la coda è stata fermata: l'elemento torna "in coda"
const STOP_QUEUE: u8 = 2;
const STOP_REMOVE: u8 = 3;

struct SegRt {
    start: u64,
    end: u64,
    pos: AtomicU64,
}

#[derive(Default)]
struct Run {
    stop: AtomicU8,
    failed: AtomicBool,
    error: Mutex<Option<String>>,
    segs: OnceLock<Vec<SegRt>>,
    next: AtomicUsize,
    stream_pos: AtomicU64,
}

impl Run {
    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed) != STOP_NONE || self.failed.load(Ordering::Relaxed)
    }

    fn fail(&self, msg: String) {
        let mut e = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if e.is_none() {
            *e = Some(msg);
        }
        self.failed.store(true, Ordering::Relaxed);
    }

    fn downloaded(&self) -> u64 {
        match self.segs.get() {
            Some(segs) => segs
                .iter()
                .map(|s| s.pos.load(Ordering::Relaxed).min(s.end + 1) - s.start)
                .sum(),
            None => self.stream_pos.load(Ordering::Relaxed),
        }
    }
}

struct Inner {
    items: Vec<Item>,
    queues: Vec<Queue>,
    next_id: u64,
    runs: HashMap<u64, Arc<Run>>,
    workers: HashSet<u64>,
    dirty: bool,
}

struct Shared {
    st: Mutex<Inner>,
    limiter: Limiter,
    path: Option<PathBuf>,
    repaint: Box<dyn Fn() + Send + Sync>,
    quit: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save_locked(&self, st: &mut Inner) {
        st.dirty = false;
        let Some(path) = &self.path else { return };
        let saved = Saved {
            next_id: st.next_id,
            queues: st.queues.clone(),
            items: st.items.clone(),
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(&saved) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, path);
            }
        }
    }
}

/// Maniglia clonabile verso il motore dei download.
#[derive(Clone)]
pub struct Manager(Arc<Shared>);

impl Manager {
    /// `path`: file JSON dove salvare lo stato (None = niente salvataggio).
    pub fn new(path: Option<PathBuf>, repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let mut saved: Saved = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if saved.queues.is_empty() {
            saved.queues.push(Queue {
                id: 1,
                name: tr!("Principale", "Main").into(),
                schedule: Schedule::default(),
                running: false,
                sched_prev: false,
            });
        }
        let max_id = saved
            .items
            .iter()
            .map(|i| i.id)
            .chain(saved.queues.iter().map(|q| q.id))
            .max()
            .unwrap_or(0);
        for it in &mut saved.items {
            // un'interruzione brusca lascia i file a metà: si riprendono con "Riprendi"
            if it.state == State::Downloading {
                it.state = State::Paused;
            }
            it.speed = 0.0;
        }
        let sh = Arc::new(Shared {
            st: Mutex::new(Inner {
                items: saved.items,
                queues: saved.queues,
                next_id: saved.next_id.max(max_id + 1),
                runs: HashMap::new(),
                workers: HashSet::new(),
                dirty: false,
            }),
            limiter: Limiter::new(),
            path,
            repaint: Box::new(repaint),
            quit: AtomicBool::new(false),
        });
        let m = Manager(sh);
        m.spawn_scheduler();
        m
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.0.lock();
        Snapshot {
            items: st.items.clone(),
            queues: st.queues.clone(),
        }
    }

    /// true se almeno un download è in corso (serve a ridisegnare la finestra)
    pub fn any_active(&self) -> bool {
        self.0
            .lock()
            .items
            .iter()
            .any(|i| i.state == State::Downloading)
    }

    pub fn set_speed_limit_kbps(&self, kbps: u32) {
        self.0.limiter.set_kbps(kbps);
    }

    /// Aggiunge uno o più link (uno per riga). Restituisce quanti ne ha accettati.
    pub fn add(&self, urls: &str, dir: &Path, queue: u64, conns: u8) -> usize {
        let mut added = 0;
        let mut st = self.0.lock();
        let queue = if st.queues.iter().any(|q| q.id == queue) {
            queue
        } else {
            st.queues.first().map(|q| q.id).unwrap_or(1)
        };
        for line in urls.lines() {
            let url = line.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let id = st.next_id;
            st.next_id += 1;
            st.items.push(Item {
                id,
                url: url.to_string(),
                file_name: name_from_url(url),
                dir: dir.to_path_buf(),
                queue,
                conns: conns.clamp(1, MAX_CONNECTIONS),
                state: State::Queued,
                error: String::new(),
                total: None,
                resumable: false,
                probed: false,
                segs: Vec::new(),
                downloaded: 0,
                final_path: None,
                speed: 0.0,
            });
            added += 1;
        }
        if added > 0 {
            self.0.save_locked(&mut st);
        }
        drop(st);
        (self.0.repaint)();
        added
    }

    /// Ferma un download (resta in elenco, riprendibile).
    pub fn pause(&self, id: u64) {
        let mut st = self.0.lock();
        if let Some(run) = st.runs.get(&id) {
            run.stop.store(STOP_PAUSE, Ordering::Relaxed);
        } else if let Some(it) = st.item_mut(id) {
            if it.state == State::Queued {
                it.state = State::Paused;
            }
        }
        self.0.save_locked(&mut st);
        drop(st);
        (self.0.repaint)();
    }

    /// Rimette in coda un download fermo o fallito e avvia la sua coda.
    pub fn resume(&self, id: u64) {
        let mut st = self.0.lock();
        let mut queue = None;
        if let Some(it) = st.item_mut(id) {
            if matches!(it.state, State::Paused | State::Failed) {
                it.state = State::Queued;
                it.error.clear();
                queue = Some(it.queue);
            }
        }
        self.0.save_locked(&mut st);
        drop(st);
        if let Some(q) = queue {
            self.start_queue(q);
        }
        (self.0.repaint)();
    }

    /// Toglie un download dall'elenco; `delete_file` cancella anche il file scaricato.
    pub fn remove(&self, id: u64, delete_file: bool) {
        let mut st = self.0.lock();
        if let Some(run) = st.runs.get(&id) {
            // sta scaricando: il thread del download fa pulizia quando si ferma
            run.stop.store(STOP_REMOVE, Ordering::Relaxed);
        } else if let Some(pos) = st.items.iter().position(|i| i.id == id) {
            let it = st.items.remove(pos);
            let _ = fs::remove_file(part_path(&it));
            if delete_file {
                if let Some(p) = &it.final_path {
                    let _ = fs::remove_file(p);
                }
            }
        }
        self.0.save_locked(&mut st);
        drop(st);
        (self.0.repaint)();
    }

    pub fn clear_finished(&self) {
        let mut st = self.0.lock();
        st.items.retain(|i| i.state != State::Done);
        self.0.save_locked(&mut st);
        drop(st);
        (self.0.repaint)();
    }

    /// Sposta un elemento prima/dopo gli altri della sua coda.
    pub fn move_item(&self, id: u64, up: bool) {
        let mut st = self.0.lock();
        let Some(pos) = st.items.iter().position(|i| i.id == id) else {
            return;
        };
        let queue = st.items[pos].queue;
        let other = if up {
            st.items[..pos].iter().rposition(|i| i.queue == queue)
        } else {
            st.items[pos + 1..]
                .iter()
                .position(|i| i.queue == queue)
                .map(|p| p + pos + 1)
        };
        if let Some(o) = other {
            st.items.swap(pos, o);
            self.0.save_locked(&mut st);
        }
        drop(st);
        (self.0.repaint)();
    }

    pub fn add_queue(&self, name: &str) -> u64 {
        let mut st = self.0.lock();
        let id = st.next_id;
        st.next_id += 1;
        let name = name.trim();
        let name = if name.is_empty() {
            format!("{} {}", tr!("Coda", "Queue"), st.queues.len() + 1)
        } else {
            name.to_string()
        };
        st.queues.push(Queue {
            id,
            name,
            schedule: Schedule::default(),
            running: false,
            sched_prev: false,
        });
        self.0.save_locked(&mut st);
        drop(st);
        (self.0.repaint)();
        id
    }

    pub fn rename_queue(&self, id: u64, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut st = self.0.lock();
        if let Some(q) = st.queues.iter_mut().find(|q| q.id == id) {
            q.name = name.to_string();
        }
        self.0.save_locked(&mut st);
    }

    /// Elimina una coda (non l'ultima): i suoi download passano alla prima coda rimasta.
    pub fn remove_queue(&self, id: u64) {
        self.stop_queue(id);
        let mut st = self.0.lock();
        if st.queues.len() <= 1 {
            return;
        }
        st.queues.retain(|q| q.id != id);
        let first = st.queues[0].id;
        for it in st.items.iter_mut().filter(|i| i.queue == id) {
            it.queue = first;
        }
        self.0.save_locked(&mut st);
        drop(st);
        (self.0.repaint)();
    }

    pub fn set_schedule(&self, id: u64, schedule: Schedule) {
        let mut st = self.0.lock();
        if let Some(q) = st.queues.iter_mut().find(|q| q.id == id) {
            if q.schedule != schedule {
                q.schedule = schedule;
                q.sched_prev = false;
            }
        }
        self.0.save_locked(&mut st);
    }

    /// Avvia la coda: scarica in sequenza i suoi download in attesa.
    pub fn start_queue(&self, id: u64) {
        let mut st = self.0.lock();
        let Some(q) = st.queues.iter_mut().find(|q| q.id == id) else {
            return;
        };
        q.running = true;
        if st.workers.insert(id) {
            let sh = self.0.clone();
            thread::spawn(move || queue_worker(sh, id));
        }
        drop(st);
        (self.0.repaint)();
    }

    /// Ferma la coda; il download in corso torna "in coda" (si riprende dal punto raggiunto).
    pub fn stop_queue(&self, id: u64) {
        let mut st = self.0.lock();
        if let Some(q) = st.queues.iter_mut().find(|q| q.id == id) {
            q.running = false;
        }
        let active: Vec<u64> = st
            .items
            .iter()
            .filter(|i| i.queue == id && i.state == State::Downloading)
            .map(|i| i.id)
            .collect();
        for a in active {
            if let Some(run) = st.runs.get(&a) {
                // non sovrascrive una pausa o una rimozione già richieste
                let _ = run.stop.compare_exchange(
                    STOP_NONE,
                    STOP_QUEUE,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
        drop(st);
        (self.0.repaint)();
    }

    /// Ferma tutto e salva (chiusura dell'app). Aspetta un attimo che i thread si fermino.
    pub fn shutdown(&self) {
        self.0.quit.store(true, Ordering::Relaxed);
        {
            let mut st = self.0.lock();
            for q in st.queues.iter_mut() {
                q.running = false;
            }
            for run in st.runs.values() {
                // tornano "in coda": alla prossima apertura si riprendono dal punto raggiunto
                let _ = run.stop.compare_exchange(
                    STOP_NONE,
                    STOP_QUEUE,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
        for _ in 0..30 {
            if self.0.lock().runs.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let mut st = self.0.lock();
        self.0.save_locked(&mut st);
    }

    // ── pianificatore ──

    fn spawn_scheduler(&self) {
        let m = self.clone();
        thread::spawn(move || {
            while !m.0.quit.load(Ordering::Relaxed) {
                {
                    use chrono::{Datelike, Local, Timelike};
                    let now = Local::now();
                    m.scheduler_tick(
                        now.weekday().num_days_from_monday() as usize,
                        (now.hour() * 60 + now.minute()) as u16,
                    );
                }
                for _ in 0..20 {
                    if m.0.quit.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(500));
                }
            }
        });
    }

    /// Un giro del pianificatore all'istante dato (separato dall'orologio per poterlo provare).
    pub fn scheduler_tick(&self, weekday: usize, minute: u16) {
        let mut to_start = Vec::new();
        let mut to_stop = Vec::new();
        {
            let mut st = self.0.lock();
            for q in st.queues.iter_mut() {
                if !q.schedule.enabled {
                    q.sched_prev = false;
                    continue;
                }
                let want = q.schedule.active(weekday, minute);
                if want && !q.sched_prev {
                    to_start.push(q.id);
                } else if !want && q.sched_prev && q.schedule.stop_enabled {
                    to_stop.push(q.id);
                }
                q.sched_prev = want;
            }
        }
        for id in to_start {
            self.start_queue(id);
        }
        for id in to_stop {
            self.stop_queue(id);
        }
    }
}

impl Inner {
    fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }
}

// ───────────────────────────── esecuzione ─────────────────────────────

fn queue_worker(sh: Arc<Shared>, qid: u64) {
    loop {
        let next = {
            let mut st = sh.lock();
            let running = st.queues.iter().any(|q| q.id == qid && q.running);
            let next = if running && !sh.quit.load(Ordering::Relaxed) {
                st.items
                    .iter()
                    .find(|i| i.queue == qid && i.state == State::Queued)
                    .map(|i| i.id)
            } else {
                None
            };
            if next.is_none() {
                if let Some(q) = st.queues.iter_mut().find(|q| q.id == qid) {
                    q.running = false;
                }
                st.workers.remove(&qid);
                sh.save_locked(&mut st);
            }
            next
        };
        match next {
            Some(id) => run_item(&sh, id),
            None => break,
        }
        (sh.repaint)();
    }
    (sh.repaint)();
}

enum Outcome {
    Finished(PathBuf),
    Stopped,
}

fn run_item(sh: &Arc<Shared>, id: u64) {
    let run = Arc::new(Run::default());
    let snap = {
        let mut st = sh.lock();
        let Some(it) = st.item_mut(id) else { return };
        if it.state != State::Queued {
            return;
        }
        it.state = State::Downloading;
        it.error.clear();
        it.speed = 0.0;
        let snap = it.clone();
        st.runs.insert(id, run.clone());
        snap
    };
    (sh.repaint)();

    let result = download_item(sh, snap, &run);

    let mut st = sh.lock();
    st.runs.remove(&id);
    let reason = run.stop.load(Ordering::Relaxed);
    match result {
        Ok(Outcome::Finished(path)) => {
            if let Some(it) = st.item_mut(id) {
                it.state = State::Done;
                it.speed = 0.0;
                it.downloaded = it.total.unwrap_or(it.downloaded);
                it.segs.clear();
                it.final_path = Some(path);
            }
        }
        Ok(Outcome::Stopped) if reason == STOP_REMOVE => {
            if let Some(pos) = st.items.iter().position(|i| i.id == id) {
                let it = st.items.remove(pos);
                let _ = fs::remove_file(part_path(&it));
            }
        }
        Ok(Outcome::Stopped) => {
            if let Some(it) = st.item_mut(id) {
                it.speed = 0.0;
                it.state = if reason == STOP_QUEUE {
                    State::Queued
                } else {
                    State::Paused
                };
            }
        }
        Err(e) => {
            if let Some(it) = st.item_mut(id) {
                it.speed = 0.0;
                it.state = State::Failed;
                it.error = format!("{e:#}");
            }
        }
    }
    sh.save_locked(&mut st);
}

fn make_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(crate::net::USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(40)))
        .http_status_as_error(false)
        .build()
        .into()
}

struct Probe {
    total: Option<u64>,
    ranges: bool,
    name: Option<String>,
}

fn probe(agent: &ureq::Agent, url: &str) -> Result<Probe> {
    use ureq::ResponseExt;
    let resp = agent.get(url).header("Range", "bytes=0-0").call()?;
    let status = resp.status().as_u16();
    let header = |n: &str| {
        resp.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let mut name = header("content-disposition").and_then(|d| name_from_disposition(&d));
    if name.is_none() {
        let final_uri = resp.get_uri().to_string();
        let n = name_from_url(&final_uri);
        if n != "download" {
            name = Some(n);
        }
    }
    let range_total = header("content-range").and_then(|r| r.rsplit('/').next().map(str::to_owned));
    let range_total = range_total.and_then(|t| t.trim().parse::<u64>().ok());
    match status {
        206 => Ok(Probe {
            total: range_total,
            ranges: range_total.is_some(),
            name,
        }),
        200 => Ok(Probe {
            total: resp.body().content_length(),
            ranges: false,
            name,
        }),
        // file vuoto: il server rifiuta qualsiasi intervallo
        416 => Ok(Probe {
            total: range_total.or(Some(0)),
            ranges: false,
            name,
        }),
        c => bail!(tr!(
            format!("il server ha risposto con errore {c}"),
            format!("the server replied with error {c}")
        )),
    }
}

fn part_path(it: &Item) -> PathBuf {
    it.dir.join(format!("{}.{}.part", it.file_name, it.id))
}

pub fn plan_segments(total: u64, conns: u8) -> Vec<Seg> {
    if total == 0 {
        return Vec::new();
    }
    let conns = conns.clamp(1, MAX_CONNECTIONS) as u64;
    let chunk = (total / (conns * 4)).clamp(MIN_CHUNK, MAX_CHUNK);
    let mut segs = Vec::new();
    let mut start = 0;
    while start < total {
        let end = (start + chunk - 1).min(total - 1);
        // se resta solo un pezzettino, lo unisce all'ultimo blocco
        let end = if total - end - 1 < MIN_CHUNK / 4 {
            total - 1
        } else {
            end
        };
        segs.push(Seg {
            start,
            end,
            pos: start,
        });
        start = end + 1;
    }
    segs
}

fn download_item(sh: &Arc<Shared>, mut item: Item, run: &Arc<Run>) -> Result<Outcome> {
    let agent = make_agent();
    let id = item.id;

    if !item.probed {
        let p = probe(&agent, &item.url)?;
        item.total = p.total;
        item.resumable = p.ranges;
        if let Some(n) = p.name {
            let n = crate::downloader::sanitize(&n);
            if !n.is_empty() {
                item.file_name = n;
            }
        }
        item.probed = true;
        item.segs.clear();
        item.downloaded = 0;
        let mut st = sh.lock();
        if let Some(it) = st.item_mut(id) {
            it.total = item.total;
            it.resumable = item.resumable;
            it.file_name = item.file_name.clone();
            it.probed = true;
            it.segs.clear();
            it.downloaded = 0;
        }
    }
    if run.should_stop() {
        return Ok(Outcome::Stopped);
    }

    fs::create_dir_all(&item.dir)?;
    let part = part_path(&item);
    let ranged = item.resumable && item.total.is_some();

    if ranged {
        let total = item.total.unwrap_or(0);
        if item.segs.is_empty() || !part.exists() {
            item.segs = plan_segments(total, item.conns);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&part)?;
        if file.metadata()?.len() != total {
            file.set_len(total)?;
        }
        let rts: Vec<SegRt> = item
            .segs
            .iter()
            .map(|s| SegRt {
                start: s.start,
                end: s.end,
                pos: AtomicU64::new(s.pos.clamp(s.start, s.end + 1)),
            })
            .collect();
        let _ = run.segs.set(rts);
        let n_workers = (item.conns.clamp(1, MAX_CONNECTIONS) as usize).min(item.segs.len().max(1));
        let active = AtomicUsize::new(n_workers);
        let ctx = Ctx {
            agent: &agent,
            url: &item.url,
            total,
            file: &file,
            run,
            limiter: &sh.limiter,
        };
        thread::scope(|s| {
            for _ in 0..n_workers {
                s.spawn(|| {
                    segment_worker(&ctx);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            monitor(sh, id, run, &active);
        });
        report(sh, id, run, 0.0);
        file.sync_all().ok();
        drop(file);
    } else {
        // senza intervalli: una sola connessione, si riparte da zero a ogni ripresa
        let file = File::create(&part)?;
        let ctx = Ctx {
            agent: &agent,
            url: &item.url,
            total: item.total.unwrap_or(0),
            file: &file,
            run,
            limiter: &sh.limiter,
        };
        let active = AtomicUsize::new(1);
        thread::scope(|s| {
            s.spawn(|| {
                if let Err(e) = stream_worker(&ctx) {
                    run.fail(format!("{e:#}"));
                }
                active.fetch_sub(1, Ordering::SeqCst);
            });
            monitor(sh, id, run, &active);
        });
        report(sh, id, run, 0.0);
        file.sync_all().ok();
    }

    if run.failed.load(Ordering::Relaxed) {
        let msg = run
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| tr!("download non riuscito", "download failed").to_string());
        return Err(anyhow!(msg));
    }
    if run.stop.load(Ordering::Relaxed) != STOP_NONE {
        return Ok(Outcome::Stopped);
    }

    let done = run.downloaded();
    if let Some(total) = item.total {
        if done != total {
            bail!(tr!(
                format!("file incompleto ({done} di {total} byte)"),
                format!("incomplete file ({done} of {total} bytes)")
            ));
        }
    } else {
        let mut st = sh.lock();
        if let Some(it) = st.item_mut(id) {
            it.total = Some(done);
        }
    }
    let dest = unique_path(&item.dir, &item.file_name);
    fs::rename(&part, &dest)?;
    Ok(Outcome::Finished(dest))
}

/// Aggiorna l'avanzamento dell'elemento (ogni 250 ms) finché i lavoratori non finiscono.
fn monitor(sh: &Arc<Shared>, id: u64, run: &Arc<Run>, active: &AtomicUsize) {
    let mut last_t = Instant::now();
    let mut last_bytes = run.downloaded();
    let mut speed = 0.0_f64;
    let mut last_save = Instant::now();
    while active.load(Ordering::SeqCst) > 0 {
        thread::sleep(Duration::from_millis(250));
        let now = Instant::now();
        let bytes = run.downloaded();
        let dt = now.duration_since(last_t).as_secs_f64();
        if dt >= 0.9 {
            let inst = bytes.saturating_sub(last_bytes) as f64 / dt;
            speed = if speed == 0.0 {
                inst
            } else {
                0.6 * speed + 0.4 * inst
            };
            last_t = now;
            last_bytes = bytes;
        }
        report(sh, id, run, speed);
        if last_save.elapsed() > Duration::from_secs(5) {
            last_save = Instant::now();
            let mut st = sh.lock();
            sh.save_locked(&mut st);
        }
    }
}

fn report(sh: &Arc<Shared>, id: u64, run: &Run, speed: f64) {
    let downloaded = run.downloaded();
    {
        let mut st = sh.lock();
        if let Some(it) = st.item_mut(id) {
            it.downloaded = downloaded;
            it.speed = speed;
            if let Some(rts) = run.segs.get() {
                it.segs = rts
                    .iter()
                    .map(|r| Seg {
                        start: r.start,
                        end: r.end,
                        pos: r.pos.load(Ordering::Relaxed),
                    })
                    .collect();
            }
        }
    }
    (sh.repaint)();
}

struct Ctx<'a> {
    agent: &'a ureq::Agent,
    url: &'a str,
    total: u64,
    file: &'a File,
    run: &'a Run,
    limiter: &'a Limiter,
}

enum FetchErr {
    Fatal(anyhow::Error),
    Retry(anyhow::Error),
}

fn segment_worker(ctx: &Ctx) {
    let Some(segs) = ctx.run.segs.get() else {
        return;
    };
    loop {
        if ctx.run.should_stop() {
            return;
        }
        let i = ctx.run.next.fetch_add(1, Ordering::SeqCst);
        let Some(seg) = segs.get(i) else { return };
        if let Err(e) = fetch_segment(ctx, seg) {
            ctx.run.fail(format!("{e:#}"));
            return;
        }
    }
}

fn fetch_segment(ctx: &Ctx, seg: &SegRt) -> Result<()> {
    let mut attempts = 0u32;
    loop {
        if ctx.run.should_stop() {
            return Ok(());
        }
        let before = seg.pos.load(Ordering::Relaxed);
        if before > seg.end {
            return Ok(());
        }
        let err = match fetch_range(ctx, seg) {
            Ok(()) if seg.pos.load(Ordering::Relaxed) > seg.end || ctx.run.should_stop() => {
                return Ok(())
            }
            Ok(()) => anyhow!(tr!(
                "il server ha chiuso la connessione prima della fine",
                "the server closed the connection early"
            )),
            Err(FetchErr::Fatal(e)) => return Err(e),
            Err(FetchErr::Retry(e)) => e,
        };
        if seg.pos.load(Ordering::Relaxed) > before {
            attempts = 0; // ha comunque fatto progressi
        }
        attempts += 1;
        if attempts > MAX_RETRIES {
            return Err(err);
        }
        // pausa crescente, interrompibile
        let wait = Duration::from_millis(400 * attempts as u64);
        let t0 = Instant::now();
        while t0.elapsed() < wait && !ctx.run.should_stop() {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn fetch_range(ctx: &Ctx, seg: &SegRt) -> std::result::Result<(), FetchErr> {
    let pos = seg.pos.load(Ordering::Relaxed);
    // richieste a finestre di al massimo 16 MiB: un blocco bloccato non resta appeso per sempre
    let window_end = seg.end.min(pos + 16 * 1024 * 1024 - 1);
    let wanted = window_end - pos + 1;
    let bps = ctx.limiter.bytes_per_sec().max(1);
    let secs = (wanted / bps * 2).max(300);
    let resp = ctx
        .agent
        .get(ctx.url)
        .config()
        .timeout_recv_body(Some(Duration::from_secs(secs)))
        .build()
        .header("Range", format!("bytes={pos}-{window_end}"))
        .call()
        .map_err(|e| FetchErr::Retry(e.into()))?;
    let status = resp.status().as_u16();
    if status != 206 {
        let e = anyhow!(tr!(
            format!("il server ha risposto {status} a una richiesta a intervalli"),
            format!("the server replied {status} to a range request")
        ));
        return Err(if status >= 500 || status == 408 || status == 429 {
            FetchErr::Retry(e)
        } else {
            FetchErr::Fatal(e)
        });
    }
    // controlla che il file sia quello atteso
    let cr = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (range, total) = cr
        .trim_start_matches("bytes ")
        .split_once('/')
        .unwrap_or(("", ""));
    let start_ok = range
        .split('-')
        .next()
        .and_then(|s| s.trim().parse::<u64>().ok())
        == Some(pos);
    let total_ok = total.trim().parse::<u64>().ok() == Some(ctx.total);
    if !start_ok || !total_ok {
        return Err(FetchErr::Fatal(anyhow!(tr!(
            "il file sul server è cambiato durante il download",
            "the file on the server changed during the download"
        ))));
    }

    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; READ_BUF];
    let mut pos = pos;
    while pos <= window_end {
        if ctx.run.should_stop() {
            return Ok(());
        }
        let want = ((window_end - pos + 1) as usize).min(buf.len());
        let n = reader
            .read(&mut buf[..want])
            .map_err(|e| FetchErr::Retry(e.into()))?;
        if n == 0 {
            break;
        }
        ctx.limiter.take(n as u64, &|| ctx.run.should_stop());
        write_all_at(ctx.file, &buf[..n], pos).map_err(|e| FetchErr::Fatal(e.into()))?;
        pos += n as u64;
        seg.pos.store(pos, Ordering::Relaxed);
    }
    Ok(())
}

/// Download su una sola connessione (server senza intervalli o lunghezza ignota).
fn stream_worker(ctx: &Ctx) -> Result<()> {
    let resp = ctx.agent.get(ctx.url).call()?;
    let status = resp.status().as_u16();
    if status != 200 {
        bail!(tr!(
            format!("il server ha risposto con errore {status}"),
            format!("the server replied with error {status}")
        ));
    }
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; READ_BUF];
    let mut pos = 0u64;
    loop {
        if ctx.run.should_stop() {
            return Ok(());
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        ctx.limiter.take(n as u64, &|| ctx.run.should_stop());
        write_all_at(ctx.file, &buf[..n], pos)?;
        pos += n as u64;
        ctx.run.stream_pos.store(pos, Ordering::Relaxed);
    }
}

#[cfg(unix)]
fn write_all_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, off)
}

#[cfg(windows)]
fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, off)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

// ───────────────────────────── nomi dei file ─────────────────────────────

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Nome del file ricavato dall'indirizzo ("download" se non c'è).
pub fn name_from_url(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    let no_query = no_frag.split('?').next().unwrap_or(no_frag);
    let after_scheme = no_query.split("://").nth(1).unwrap_or(no_query);
    let path = after_scheme.split_once('/').map(|x| x.1).unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    let name = crate::downloader::sanitize(&percent_decode(last));
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
}

/// Nome dall'intestazione Content-Disposition (`filename*=UTF-8''…` o `filename="…"`).
pub fn name_from_disposition(h: &str) -> Option<String> {
    let lower = h.to_ascii_lowercase();
    let value_after = |key: &str| -> Option<String> {
        let i = lower.find(key)? + key.len();
        let rest = h[i..].trim_start();
        let v = if let Some(r) = rest.strip_prefix('"') {
            r.split('"').next()?.to_string()
        } else {
            rest.split(';').next()?.trim().to_string()
        };
        Some(v)
    };
    let name = if let Some(v) = value_after("filename*=") {
        // charset'lingua'valore
        let enc = v.splitn(3, '\'').nth(2).unwrap_or(&v).to_string();
        percent_decode(&enc)
    } else {
        value_after("filename=")?
    };
    // solo il nome, mai un percorso
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = crate::downloader::sanitize(name);
    (!name.is_empty()).then_some(name)
}

/// Se `name` esiste già in `dir` aggiunge " (1)", " (2)"… prima dell'estensione.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.into());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or(first)
}

// ───────────────────────────── formattazione per l'interfaccia ─────────────────────────────

pub fn fmt_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn fmt_speed(bps: f64) -> String {
    format!("{}/s", fmt_size(bps.max(0.0) as u64))
}

pub fn fmt_hms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests;
