// SPDX-License-Identifier: GPL-3.0-or-later
// Adattato da Dropall 0.4.3 di Marco Giannini (https://github.com/marcogiannini/dropall),
// stessa licenza GPL-3.0-or-later.
//! Prove del motore con un piccolo server HTTP locale (Range, limiti per connessione, ecc.).
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicUsize;

struct Req {
    path: String,
}

struct TestServer {
    base: String,
    log: Arc<Mutex<Vec<Req>>>,
    max_active: Arc<AtomicUsize>,
    bytes_sent: Arc<AtomicU64>,
}

struct Cfg {
    body: Arc<Vec<u8>>,
    ranges: bool,
    /// byte al secondo per ogni connessione (0 = nessun limite)
    per_conn_bps: u64,
    disposition: Option<&'static str>,
}

fn body(len: usize) -> Arc<Vec<u8>> {
    // contenuto riconoscibile: ogni posizione ha un valore diverso
    Arc::new(
        (0..len)
            .map(|i| (i.wrapping_mul(31).wrapping_add(i >> 8)) as u8)
            .collect(),
    )
}

fn serve(cfg: Cfg) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let bytes_sent = Arc::new(AtomicU64::new(0));
    let cfg = Arc::new(cfg);
    let (l2, a2, m2, b2) = (
        log.clone(),
        active.clone(),
        max_active.clone(),
        bytes_sent.clone(),
    );
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let (cfg, log, active, max_active, bytes) =
                (cfg.clone(), l2.clone(), a2.clone(), m2.clone(), b2.clone());
            thread::spawn(move || {
                let _ = handle(conn, &cfg, &log, &active, &max_active, &bytes);
            });
        }
    });
    TestServer {
        base,
        log,
        max_active,
        bytes_sent,
    }
}

fn handle(
    conn: TcpStream,
    cfg: &Cfg,
    log: &Mutex<Vec<Req>>,
    active: &AtomicUsize,
    max_active: &AtomicUsize,
    bytes_sent: &AtomicU64,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut range = None;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        if h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(r) = lower.strip_prefix("range: bytes=") {
            let (a, b) = r.trim().split_once('-').unwrap_or(("0", "0"));
            range = Some((a.parse().unwrap_or(0), b.parse().unwrap_or(0)));
        }
    }
    log.lock().unwrap().push(Req { path: path.clone() });
    let mut out = conn;
    if path.starts_with("/missing") {
        out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
    let total = cfg.body.len() as u64;
    let (status, a, b) = match (cfg.ranges, range) {
        (true, Some((a, b))) => ("206 Partial Content", a, b.min(total - 1)),
        _ => ("200 OK", 0, total - 1),
    };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        b - a + 1
    );
    if cfg.ranges {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    if status.starts_with("206") {
        head.push_str(&format!("Content-Range: bytes {a}-{b}/{total}\r\n"));
    }
    if let Some(d) = cfg.disposition {
        head.push_str(&format!("Content-Disposition: {d}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;

    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
    max_active.fetch_max(now, Ordering::SeqCst);
    let mut res = Ok(());
    // il probe (0-0) non conta come connessione di download
    let mut pos = a as usize;
    let end = b as usize + 1;
    while pos < end {
        let n = (end - pos).min(8192);
        if let Err(e) = out.write_all(&cfg.body[pos..pos + n]) {
            res = Err(e);
            break;
        }
        bytes_sent.fetch_add(n as u64, Ordering::SeqCst);
        pos += n;
        if cfg.per_conn_bps > 0 {
            thread::sleep(Duration::from_secs_f64(n as f64 / cfg.per_conn_bps as f64));
        }
    }
    active.fetch_sub(1, Ordering::SeqCst);
    res
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dropall-dm-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn manager() -> Manager {
    Manager::new(None, || {})
}

fn wait_for(m: &Manager, what: &str, secs: u64, cond: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let t0 = Instant::now();
    loop {
        let s = m.snapshot();
        if cond(&s) {
            return s;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "tempo scaduto in attesa di: {what}; stato: {:?}",
            s.items
                .iter()
                .map(|i| (&i.file_name, i.state, i.downloaded, &i.error))
                .collect::<Vec<_>>()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn all_done(s: &Snapshot) -> bool {
    !s.items.is_empty() && s.items.iter().all(|i| i.state == State::Done)
}

fn queue_id(m: &Manager) -> u64 {
    m.snapshot().queues[0].id
}

// ───────────────────────────── prove ─────────────────────────────

#[test]
fn multi_connection_download_is_correct_and_parallel() {
    let data = body(6 * 1024 * 1024 + 123);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 4 * 1024 * 1024,
        disposition: None,
    });
    let dir = temp_dir("multi");
    let m = manager();
    let q = queue_id(&m);
    assert_eq!(m.add(&format!("{}/big.bin", srv.base), &dir, q, 4), 1);
    m.start_queue(q);
    let s = wait_for(&m, "download completo", 60, all_done);
    let path = s.items[0].final_path.clone().unwrap();
    assert_eq!(path, dir.join("big.bin"));
    assert_eq!(fs::read(&path).unwrap(), *data, "contenuto diverso");
    assert!(
        srv.max_active.load(Ordering::SeqCst) >= 3,
        "attese più connessioni in parallelo, viste {}",
        srv.max_active.load(Ordering::SeqCst)
    );
    // nessun file temporaneo rimasto
    assert!(!fs::read_dir(&dir).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn more_connections_are_faster() {
    // ogni connessione è limitata a 1 MiB/s: con 4 connessioni si va ~4 volte più veloci
    let data = body(2 * 1024 * 1024);
    let time_with = |conns: u8, name: &str| {
        let srv = serve(Cfg {
            body: data.clone(),
            ranges: true,
            per_conn_bps: 1024 * 1024,
            disposition: None,
        });
        let dir = temp_dir(name);
        let m = manager();
        let q = queue_id(&m);
        m.add(&format!("{}/f.bin", srv.base), &dir, q, conns);
        let t0 = Instant::now();
        m.start_queue(q);
        let s = wait_for(&m, "download", 60, all_done);
        let el = t0.elapsed();
        assert_eq!(
            fs::read(s.items[0].final_path.as_ref().unwrap()).unwrap(),
            *data
        );
        let _ = fs::remove_dir_all(&dir);
        el
    };
    let t1 = time_with(1, "speed1");
    let t4 = time_with(4, "speed4");
    eprintln!("1 connessione: {t1:?}, 4 connessioni: {t4:?}");
    assert!(
        t1 > Duration::from_millis(1500),
        "1 connessione troppo veloce: {t1:?}"
    );
    assert!(
        t4.as_secs_f64() < t1.as_secs_f64() * 0.55,
        "4 connessioni non abbastanza più veloci: {t1:?} contro {t4:?}"
    );
}

#[test]
fn server_without_ranges_falls_back_to_single_connection() {
    let data = body(1536 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: false,
        per_conn_bps: 0,
        disposition: None,
    });
    let dir = temp_dir("noranges");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/plain.bin", srv.base), &dir, q, 8);
    m.start_queue(q);
    let s = wait_for(&m, "download", 30, all_done);
    assert!(!s.items[0].resumable);
    assert_eq!(
        fs::read(s.items[0].final_path.as_ref().unwrap()).unwrap(),
        *data
    );
    assert_eq!(srv.max_active.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn speed_limit_is_respected() {
    let data = body(1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 0,
        disposition: None,
    });
    let dir = temp_dir("limit");
    let m = manager();
    m.set_speed_limit_kbps(512); // 512 KiB/s → 1 MiB in ~2 s
    let q = queue_id(&m);
    m.add(&format!("{}/l.bin", srv.base), &dir, q, 4);
    let t0 = Instant::now();
    m.start_queue(q);
    let s = wait_for(&m, "download", 30, all_done);
    let el = t0.elapsed().as_secs_f64();
    eprintln!("1 MiB a 512 KiB/s: {el:.2} s");
    assert!(el > 1.5, "limite non rispettato: {el:.2} s");
    assert!(el < 4.5, "troppo lento: {el:.2} s");
    assert_eq!(
        fs::read(s.items[0].final_path.as_ref().unwrap()).unwrap(),
        *data
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn limit_can_be_changed_while_downloading() {
    let data = body(3 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 0,
        disposition: None,
    });
    let dir = temp_dir("limit2");
    let m = manager();
    m.set_speed_limit_kbps(256);
    let q = queue_id(&m);
    m.add(&format!("{}/l.bin", srv.base), &dir, q, 4);
    let t0 = Instant::now();
    m.start_queue(q);
    thread::sleep(Duration::from_millis(1200));
    m.set_speed_limit_kbps(0); // tolto il limite: finisce subito
    wait_for(&m, "download", 30, all_done);
    let el = t0.elapsed().as_secs_f64();
    eprintln!("3 MiB, limite tolto dopo 1,2 s: {el:.2} s");
    assert!(el < 4.0, "il limite non è stato tolto: {el:.2} s");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pause_and_resume_does_not_restart_from_zero() {
    let data = body(4 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 512 * 1024,
        disposition: None,
    });
    let dir = temp_dir("pause");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/p.bin", srv.base), &dir, q, 2);
    let id = m.snapshot().items[0].id;
    m.start_queue(q);
    wait_for(&m, "avvio", 20, |s| s.items[0].downloaded >= 1024 * 1024);
    m.pause(id);
    let s = wait_for(&m, "pausa", 20, |s| s.items[0].state == State::Paused);
    let at_pause = s.items[0].downloaded;
    assert!(at_pause < 4 * 1024 * 1024, "era già finito");
    assert!(part_path(&s.items[0]).exists(), "file parziale mancante");
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        m.snapshot().items[0].downloaded,
        at_pause,
        "è andato avanti durante la pausa"
    );
    m.resume(id);
    let s = wait_for(&m, "ripresa", 60, all_done);
    assert_eq!(
        fs::read(s.items[0].final_path.as_ref().unwrap()).unwrap(),
        *data
    );
    let sent = srv.bytes_sent.load(Ordering::SeqCst);
    assert!(
        sent < 4 * 1024 * 1024 + 1536 * 1024,
        "ha riscaricato troppo: inviati {sent} byte per un file di 4 MiB"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn state_survives_restart_and_resumes() {
    let data = body(3 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 512 * 1024,
        disposition: None,
    });
    let dir = temp_dir("restart");
    let state = dir.join("downloads.json");
    let m = Manager::new(Some(state.clone()), || {});
    let q = queue_id(&m);
    m.add(&format!("{}/r.bin", srv.base), &dir, q, 2);
    m.start_queue(q);
    wait_for(&m, "avvio", 20, |s| s.items[0].downloaded >= 1024 * 1024);
    m.shutdown(); // come chiudere l'app
    let before = m.snapshot().items[0].clone();
    assert_eq!(before.state, State::Queued);
    assert!(before.downloaded > 0 && before.downloaded < 3 * 1024 * 1024);

    // "riapre" l'app: lo stato viene riletto dal disco
    let m2 = Manager::new(Some(state), || {});
    let s = m2.snapshot();
    assert_eq!(s.items.len(), 1);
    assert_eq!(s.items[0].state, State::Queued);
    assert_eq!(s.items[0].downloaded, before.downloaded);
    m2.start_queue(s.queues[0].id);
    let s = wait_for(&m2, "ripresa dopo il riavvio", 60, all_done);
    assert_eq!(
        fs::read(s.items[0].final_path.as_ref().unwrap()).unwrap(),
        *data
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn queue_downloads_files_one_after_the_other() {
    let data = body(600 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 1024 * 1024,
        disposition: None,
    });
    let dir = temp_dir("seq");
    let m = manager();
    let q = queue_id(&m);
    let urls = ["a", "b", "c"]
        .iter()
        .map(|n| format!("{}/{n}.bin", srv.base))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(m.add(&urls, &dir, q, 4), 3);
    m.start_queue(q);
    wait_for(&m, "tutti i file", 60, all_done);
    // l'ordine delle richieste deve essere raggruppato per file: a a… b b… c c…
    let log = srv.log.lock().unwrap();
    let order: Vec<&str> = log.iter().map(|r| r.path.as_str()).collect();
    let mut firsts: Vec<&str> = Vec::new();
    for p in &order {
        if firsts.last() != Some(p) {
            firsts.push(p);
        }
    }
    assert_eq!(
        firsts,
        vec!["/a.bin", "/b.bin", "/c.bin"],
        "richieste: {order:?}"
    );
    // e la coda, finito il lavoro, si ferma da sola
    drop(log);
    wait_for(&m, "coda ferma", 10, |s| !s.queues[0].running);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stopping_a_queue_puts_the_file_back_in_queue() {
    let data = body(4 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 512 * 1024,
        disposition: None,
    });
    let dir = temp_dir("stopq");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/s.bin", srv.base), &dir, q, 2);
    m.start_queue(q);
    wait_for(&m, "in corso", 20, |s| {
        s.items[0].state == State::Downloading
    });
    m.stop_queue(q);
    let s = wait_for(&m, "fermata", 20, |s| s.items[0].state == State::Queued);
    assert!(!s.queues[0].running);
    assert_ne!(s.items[0].state, State::Done);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn scheduler_starts_and_stops_the_queue() {
    let data = body(4 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 512 * 1024,
        disposition: None,
    });
    let dir = temp_dir("sched");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/t.bin", srv.base), &dir, q, 2);
    m.set_schedule(
        q,
        Schedule {
            enabled: true,
            start_min: 2 * 60,
            stop_enabled: true,
            stop_min: 3 * 60,
            days: [true; 7],
        },
    );
    // fuori orario: non parte
    m.scheduler_tick(1, 60);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(m.snapshot().items[0].state, State::Queued);
    assert!(!m.snapshot().queues[0].running);
    // alle 02:00 parte
    m.scheduler_tick(1, 2 * 60);
    wait_for(&m, "partenza pianificata", 20, |s| {
        s.items[0].state == State::Downloading
    });
    // alle 03:00 si ferma
    m.scheduler_tick(1, 3 * 60);
    let s = wait_for(&m, "stop pianificato", 20, |s| {
        s.items[0].state == State::Queued
    });
    assert!(!s.queues[0].running);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn schedule_windows() {
    let mut s = Schedule {
        enabled: true,
        start_min: 22 * 60,
        stop_enabled: true,
        stop_min: 6 * 60,
        days: [true; 7],
    };
    // intervallo notturno 22:00 → 06:00
    assert!(s.active(0, 23 * 60));
    assert!(s.active(1, 5 * 60 + 59));
    assert!(!s.active(1, 6 * 60));
    assert!(!s.active(1, 12 * 60));
    // il giorno conta quando l'intervallo inizia: solo il lunedì
    s.days = [true, false, false, false, false, false, false];
    assert!(s.active(0, 23 * 60)); // lunedì sera
    assert!(s.active(1, 3 * 60)); // martedì notte, ma iniziato lunedì
    assert!(!s.active(1, 23 * 60)); // martedì sera: non abilitato
    assert!(!s.active(2, 3 * 60)); // mercoledì notte: iniziato martedì
                                   // intervallo diurno
    let d = Schedule {
        enabled: true,
        start_min: 9 * 60,
        stop_enabled: true,
        stop_min: 17 * 60,
        days: [true; 7],
    };
    assert!(d.active(3, 9 * 60));
    assert!(!d.active(3, 17 * 60));
    assert!(!d.active(3, 8 * 60));
    // senza orario di stop vale fino a fine giornata
    let n = Schedule {
        stop_enabled: false,
        ..d.clone()
    };
    assert!(n.active(3, 23 * 60 + 59));
    assert!(!n.active(3, 60));
    // inizio e fine uguali: nessun intervallo
    let e = Schedule {
        stop_min: 9 * 60,
        ..d
    };
    assert!(!e.active(3, 9 * 60));
}

#[test]
fn http_error_marks_the_item_failed_and_queue_continues() {
    let data = body(100 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 0,
        disposition: None,
    });
    let dir = temp_dir("fail");
    let m = manager();
    let q = queue_id(&m);
    m.add(
        &format!("{b}/missing.bin\n{b}/ok.bin", b = srv.base),
        &dir,
        q,
        4,
    );
    m.start_queue(q);
    let s = wait_for(&m, "fine", 20, |s| {
        s.items
            .iter()
            .all(|i| matches!(i.state, State::Done | State::Failed))
    });
    assert_eq!(s.items[0].state, State::Failed);
    assert!(
        s.items[0].error.contains("404"),
        "errore: {}",
        s.items[0].error
    );
    assert_eq!(s.items[1].state, State::Done);
    // "Riprendi" rimette in coda il file fallito
    m.resume(s.items[0].id);
    let s = wait_for(&m, "secondo tentativo", 20, |s| {
        s.items[0].state == State::Failed && !s.queues[0].running
    });
    assert!(s.items[0].error.contains("404"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn file_name_from_content_disposition_and_unique_names() {
    let data = body(10 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 0,
        disposition: Some("attachment; filename=\"Report finale.pdf\""),
    });
    let dir = temp_dir("names");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/get?id=7", srv.base), &dir, q, 4);
    m.add(&format!("{}/get?id=8", srv.base), &dir, q, 4);
    m.start_queue(q);
    let s = wait_for(&m, "download", 20, all_done);
    assert_eq!(
        s.items[0].final_path.as_ref().unwrap(),
        &dir.join("Report finale.pdf")
    );
    assert_eq!(
        s.items[1].final_path.as_ref().unwrap(),
        &dir.join("Report finale (1).pdf")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn removing_an_item_deletes_partial_file() {
    let data = body(4 * 1024 * 1024);
    let srv = serve(Cfg {
        body: data.clone(),
        ranges: true,
        per_conn_bps: 512 * 1024,
        disposition: None,
    });
    let dir = temp_dir("remove");
    let m = manager();
    let q = queue_id(&m);
    m.add(&format!("{}/x.bin", srv.base), &dir, q, 2);
    let id = m.snapshot().items[0].id;
    m.start_queue(q);
    wait_for(&m, "avvio", 20, |s| s.items[0].downloaded > 0);
    m.remove(id, false);
    wait_for(&m, "rimosso", 20, |s| s.items.is_empty());
    thread::sleep(Duration::from_millis(300));
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "file temporaneo rimasto"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn names_are_parsed() {
    assert_eq!(
        name_from_url("https://ex.com/a/b/file%20one.zip?x=1#f"),
        "file one.zip"
    );
    assert_eq!(name_from_url("https://ex.com/"), "download");
    assert_eq!(name_from_url("https://ex.com"), "download");
    assert_eq!(
        name_from_disposition("attachment; filename*=UTF-8''caff%C3%A8.txt").as_deref(),
        Some("caffè.txt")
    );
    assert_eq!(
        name_from_disposition("attachment; filename=plain.bin; size=3").as_deref(),
        Some("plain.bin")
    );
    assert_eq!(name_from_disposition("inline"), None);
    // niente percorsi: i separatori vengono tolti
    assert_eq!(
        name_from_disposition("attachment; filename=\"../../etc/passwd\"").as_deref(),
        Some("passwd")
    );
}

#[test]
fn segments_cover_the_file_exactly() {
    for (total, conns) in [
        (1u64, 8u8),
        (700_000, 4),
        (10 * 1024 * 1024, 8),
        (123_456_789, 16),
    ] {
        let segs = plan_segments(total, conns);
        assert_eq!(segs.first().unwrap().start, 0);
        assert_eq!(segs.last().unwrap().end, total - 1);
        for w in segs.windows(2) {
            assert_eq!(w[0].end + 1, w[1].start, "buco o sovrapposizione");
        }
        assert!(segs.iter().all(|s| s.pos == s.start && s.end >= s.start));
    }
    assert!(plan_segments(0, 4).is_empty());
    // un file da 2 MiB con 4 connessioni si divide in 4 blocchi
    assert_eq!(plan_segments(2 * 1024 * 1024, 4).len(), 4);
}
