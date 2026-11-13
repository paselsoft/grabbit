// SPDX-License-Identifier: GPL-3.0-or-later
//! Interfaccia grafica (egui).
use crate::config::{Config, Mode, Repeat};
use crate::downloader::{self, Emit, Event, JsRuntime, Toolset};
use crate::player;
use crate::search::{self, SearchResult};
use crate::selfupdate::{self, Release};
use crate::tools;
use crate::tr;
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// Tema scuro con accento arancione.
const ACCENT: Color32 = Color32::from_rgb(249, 115, 22);
/// Arancione più chiaro: contorni e focus.
const ACCENT_LIGHT: Color32 = Color32::from_rgb(251, 146, 60);
/// Testo sopra uno sfondo arancione (il bianco avrebbe poco contrasto).
const ON_ACCENT: Color32 = Color32::from_rgb(26, 15, 5);
const OK: Color32 = Color32::from_rgb(64, 185, 120);
const ERR: Color32 = Color32::from_rgb(235, 87, 87);
const WARN: Color32 = Color32::from_rgb(240, 205, 70);

const VIDEO_FORMATS: [(&str, &str); 3] = [("mp4", "MP4"), ("mkv", "MKV"), ("webm", "WebM")];
const HEIGHTS: [(u32, &str); 7] = [
    (0, "Migliore disponibile"),
    (2160, "4K (2160p)"),
    (1440, "1440p"),
    (1080, "Full HD (1080p)"),
    (720, "HD (720p)"),
    (480, "480p"),
    (360, "360p"),
];
const AUDIO_FORMATS: [(&str, &str); 6] = [
    ("mp3", "MP3"),
    ("m4a", "M4A (AAC)"),
    ("opus", "Opus"),
    ("vorbis", "OGG Vorbis"),
    ("flac", "FLAC (senza perdita)"),
    ("wav", "WAV (senza perdita)"),
];
const BITRATES: [u32; 7] = [320, 256, 192, 160, 128, 96, 64];
const BROWSERS: [(&str, &str); 7] = [
    ("", "Nessuno"),
    ("chrome", "Chrome"),
    ("firefox", "Firefox"),
    ("edge", "Edge"),
    ("brave", "Brave"),
    ("opera", "Opera"),
    ("safari", "Safari"),
];

enum Msg {
    ToolsStatus(String),
    ToolsReady {
        ytdlp: String,
        ffmpeg: Option<PathBuf>,
        js: Option<JsRuntime>,
        warnings: Vec<String>,
    },
    ToolsFailed(String),
    UpdateCheck(Result<selfupdate::Check, String>),
    UpdateStatus(String),
    UpdateDone(Result<(), String>),
    Folder(PathBuf),
    /// cartella della musica scelta per «Ascolta»
    PlayerDir(PathBuf),
    /// libreria musicale letta (`seq` scarta le letture superate)
    Library {
        seq: u64,
        library: player::Library,
    },
    Dl(Event),
    /// esito di una ricerca; `seq` scarta le risposte di ricerche ormai superate
    SearchDone {
        seq: u64,
        result: Result<search::Outcome, String>,
    },
    /// titolo, artista, brani e copertina di un album arrivato senza (risultato `idx`)
    Details {
        seq: u64,
        idx: usize,
        details: Option<search::Details>,
    },
    /// copertina del risultato `idx` della ricerca `seq`, già rimpicciolita
    Thumb {
        seq: u64,
        idx: usize,
        image: egui::ColorImage,
    },
    /// album spostato nel Cestino (o errore)
    Trashed(Result<PathBuf, String>),
    /// album aggiunto a Musica o iTunes (nome dell'album, o errore)
    MusicAdded(Result<String, String>),
    /// correzione di volume misurata per un brano (`None`: non misurabile)
    Gain {
        path: PathBuf,
        gain: Option<f32>,
    },
    /// brani dell'album `idx` della ricerca `seq`, per sceglierli
    Tracks {
        seq: u64,
        idx: usize,
        tracks: Option<Vec<(usize, String)>>,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum JobState {
    Waiting,
    Running,
    Done,
    Failed,
}

struct JobUi {
    id: usize,
    label: String,
    pct: f32,
    detail: String,
    state: JobState,
    /// come riscaricarlo con «Riprova»
    spec: Option<downloader::Job>,
    /// file salvato, per «Mostra nel Finder»
    file: Option<PathBuf>,
}

enum UpdateUi {
    None,
    Available(Release),
    Working(String),
    Ready(String),
    Failed(String),
}

/// Filtro delle uscite nella discografia di un artista.
#[derive(Default, Clone, Copy, PartialEq)]
enum ReleaseFilter {
    #[default]
    Albums,
    Singles,
    All,
}

impl ReleaseFilter {
    fn shows(self, r: &SearchResult) -> bool {
        match self {
            ReleaseFilter::All => true,
            ReleaseFilter::Albums => is_album(r),
            ReleaseFilter::Singles => !is_album(r),
        }
    }
}

/// Brani di un album nei risultati, da scegliere prima di scaricarlo.
#[derive(Default)]
struct Pick {
    open: bool,
    loading: bool,
    /// posizione nell'album, titolo, spuntato
    tracks: Vec<(usize, String, bool)>,
    error: bool,
}

#[derive(Default)]
struct SearchUi {
    query: String,
    /// testo cercato per ultimo (per il messaggio «nessun risultato»)
    last_query: String,
    last_source: search::Source,
    /// ricerca per artista: il nome dell'artista trovato
    artist: Option<String>,
    results: Vec<SearchResult>,
    /// risultati spuntati per scaricarli insieme (stessi indici di `results`)
    selected: Vec<bool>,
    /// copertine già scaricate (stessi indici di `results`)
    thumbs: Vec<Option<egui::TextureHandle>>,
    busy: bool,
    error: Option<String>,
    seq: u64,
    cancel: Arc<AtomicBool>,
    /// messaggio dopo «Aggiungi ai link»
    notice: Option<String>,
    /// discografia di un artista: quali uscite mostrare (come i filtri di YouTube Music)
    release_filter: ReleaseFilter,
    /// «Scegli i brani…» degli album (indice del risultato)
    picks: std::collections::HashMap<usize, Pick>,
}

#[derive(Default)]
struct ToolsUi {
    ytdlp: Option<String>,
    ffmpeg: Option<PathBuf>,
    js: Option<JsRuntime>,
    status: String,
    error: Option<String>,
    warnings: Vec<String>,
    busy: bool,
}

pub struct App {
    cfg: Config,
    input: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    tools: ToolsUi,
    jobs: Vec<JobUi>,
    next_id: usize,
    running: bool,
    cancel: Arc<AtomicBool>,
    log: Vec<String>,
    update: UpdateUi,
    search: SearchUi,
    /// ultimo esito del controllo aggiornamenti, sempre visibile in fondo alla finestra
    update_status: String,
    update_checking: bool,
    /// false dopo un riavvio per aggiornamento: niente nuovo aggiornamento automatico
    auto_install: bool,
    /// l'aggiornamento in corso è partito da solo (non da un clic): a fine lavoro si riavvia
    update_auto: bool,
    logo: egui::TextureHandle,
    /// quando è stato copiato il registro (per mostrare «Copiato» per qualche secondo)
    log_copied: Option<std::time::Instant>,
    tab: Tab,
    listen: ListenUi,
    engine: player::Engine,
    /// correzioni già misurate per il volume uguale tra i brani
    gains: player::GainCache,
    /// misura i brani in sottofondo
    analyzer: player::Analyzer,
    /// porta in vista l'elenco dei download al prossimo disegno («Vedi elenco», avvio)
    scroll_to_queue: bool,
    /// brani da fermare uno per uno («Annulla» sulla riga)
    stop: downloader::StopSet,
    /// titolo della finestra mostrato per ultimo (con l'avanzamento durante i download)
    title: String,
    /// la finestra aveva il focus al disegno precedente (per accorgersi del ritorno)
    was_focused: bool,
    /// ultimo testo degli appunti già esaminato: lo stesso link non si propone due volte
    clip_seen: String,
    /// link appena copiato, proposto con «Aggiungi»
    clip_offer: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let logo = load_logo(&cc.egui_ctx);
        let (tx, rx) = channel();
        let analyzer = {
            let (tx, ctx) = (tx.clone(), cc.egui_ctx.clone());
            player::Analyzer::start(move |path, gain| {
                let _ = tx.send(Msg::Gain { path, gain });
                ctx.request_repaint();
            })
        };
        let mut app = Self {
            cfg: Config::load(),
            input: String::new(),
            tx,
            rx,
            tools: ToolsUi::default(),
            jobs: Vec::new(),
            next_id: 0,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            log: Vec::new(),
            update: UpdateUi::None,
            search: SearchUi::default(),
            update_status: String::new(),
            update_checking: false,
            auto_install: !std::env::args().any(|a| a == selfupdate::AFTER_UPDATE_ARG),
            update_auto: false,
            logo,
            log_copied: None,
            tab: Tab::default(),
            listen: ListenUi::default(),
            engine: {
                let ctx = cc.egui_ctx.clone();
                player::Engine::start(move || ctx.request_repaint())
            },
            gains: player::GainCache::load(Some(tools::data_dir().join("loudness.json"))),
            analyzer,
            scroll_to_queue: false,
            stop: Default::default(),
            title: String::new(),
            was_focused: false,
            clip_seen: String::new(),
            clip_offer: None,
        };
        cc.egui_ctx
            .set_zoom_factor(app.cfg.ui_scale.clamp(0.8, 2.0));
        app.start_setup(&cc.egui_ctx);
        if app.cfg.last_track.is_some() {
            app.start_scan(&cc.egui_ctx);
        }
        // il controllo degli aggiornamenti parte subito, senza aspettare yt-dlp e ffmpeg
        app.start_update_check(&cc.egui_ctx);
        app
    }

    fn sender(&self, ctx: &egui::Context) -> impl Fn(Msg) + Send + Clone + 'static {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        move |m| {
            let _ = tx.send(m);
            ctx.request_repaint();
        }
    }

    /// Scarica/aggiorna yt-dlp, Deno e ffmpeg.
    fn start_setup(&mut self, ctx: &egui::Context) {
        self.tools.busy = true;
        self.tools.error = None;
        self.tools.status = tr!("Preparazione…", "Preparing…").into();
        let send = self.sender(ctx);
        thread::spawn(move || {
            let mut status = |s: String| send(Msg::ToolsStatus(s));
            let ytdlp = match tools::ensure_ytdlp(&mut status) {
                Ok(v) => v,
                Err(e) => {
                    send(Msg::ToolsFailed(format!("{e:#}")));
                    return;
                }
            };
            let mut warnings = Vec::new();
            if let Err(e) = tools::ensure_python_modules(&mut status) {
                warnings.push(tr!(
                    format!("componenti Python di yt-dlp non installati ({e:#}): le copertine potrebbero non essere incorporate nei file."),
                    format!("yt-dlp's Python components are not installed ({e:#}): cover art may not be embedded in the files.")
                ));
            }
            let js = tools::ensure_js_runtime(&mut status)
                .map_err(|e| warnings.push(tr!(format!("Runtime JavaScript (Deno) non disponibile ({e:#}): alcuni video YouTube potrebbero non funzionare."), format!("JavaScript runtime (Deno) is not available ({e:#}): some YouTube videos may not work."))))
                .ok();
            let ffmpeg = tools::ensure_ffmpeg(&mut status)
                .map_err(|e| warnings.push(tr!(format!("ffmpeg non disponibile ({e:#}): serve per la conversione audio e l'unione video/audio. Installalo e riavvia."), format!("ffmpeg is not available ({e:#}): it is needed for audio conversion and merging video/audio. Install it and restart."))))
                .ok();
            send(Msg::ToolsReady {
                ytdlp,
                ffmpeg,
                js,
                warnings,
            });
        });
    }

    /// Cerca una nuova versione di Grabbit su GitHub (in un thread a parte).
    fn start_update_check(&mut self, ctx: &egui::Context) {
        self.update_checking = true;
        self.update_status = tr!("Controllo aggiornamenti…", "Checking for updates…").into();
        let send = self.sender(ctx);
        thread::spawn(move || {
            let r = selfupdate::check().map_err(|e| selfupdate::describe_error(&e));
            send(Msg::UpdateCheck(r));
        });
    }

    fn start_update(&mut self, ctx: &egui::Context, rel: Release, auto: bool) {
        self.update_auto = auto;
        self.update = UpdateUi::Working(tr!("Avvio aggiornamento…", "Starting update…").into());
        let send = self.sender(ctx);
        thread::spawn(move || {
            let mut st = |s: String| send(Msg::UpdateStatus(s));
            let r = selfupdate::apply(&rel, &mut st).map_err(|e| format!("{e:#}"));
            send(Msg::UpdateDone(r));
        });
    }

    fn start_download(&mut self, ctx: &egui::Context) {
        let inputs: Vec<String> = self
            .input
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        self.start_jobs(ctx, inputs);
    }

    /// Avvia il download dei link indicati, con le opzioni scelte (Video/Audio, formato…).
    fn start_jobs(&mut self, ctx: &egui::Context, inputs: Vec<String>) {
        if inputs.is_empty() || self.running {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.cfg.out_dir) {
            self.log.push(tr!(
                format!("Impossibile creare la cartella: {e}"),
                format!("Could not create the folder: {e}")
            ));
            return;
        }
        self.cfg.save();
        self.jobs.clear();
        self.running = true;
        self.scroll_to_queue = true;
        self.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut s) = self.stop.lock() {
            s.clear();
        }

        let toolset = self.toolset();
        let send = self.sender(ctx);
        let emit: Emit = Arc::new(move |e| send(Msg::Dl(e)));
        let (cfg, cancel, base) = (self.cfg.clone(), self.cancel.clone(), self.next_id);
        let archive = Some(downloader::archive_path());
        let stop = self.stop.clone();
        thread::spawn(move || {
            downloader::run(inputs, cfg, toolset, base, emit, cancel, archive, stop)
        });
    }

    /// Riscarica i brani non riusciti indicati, nelle loro righe dell'elenco.
    fn retry_jobs(&mut self, ctx: &egui::Context, ids: &[usize]) {
        if self.running {
            return;
        }
        let mut specs = Vec::new();
        for j in self.jobs.iter_mut() {
            if ids.contains(&j.id) && j.state == JobState::Failed {
                if let Some(spec) = &j.spec {
                    specs.push(spec.clone());
                    j.state = JobState::Waiting;
                    j.pct = 0.0;
                    j.detail.clear();
                }
            }
        }
        if specs.is_empty() {
            return;
        }
        self.running = true;
        self.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut s) = self.stop.lock() {
            s.clear();
        }
        let toolset = self.toolset();
        let send = self.sender(ctx);
        let emit: Emit = Arc::new(move |e| send(Msg::Dl(e)));
        let (cfg, cancel, stop) = (self.cfg.clone(), self.cancel.clone(), self.stop.clone());
        let archive = Some(downloader::archive_path());
        thread::spawn(move || downloader::retry(specs, cfg, toolset, emit, cancel, archive, stop));
    }

    fn toolset(&self) -> Toolset {
        Toolset {
            ytdlp: tools::ytdlp_path(),
            ffmpeg_dir: self.tools.ffmpeg.clone(),
            js: self.tools.js.clone(),
        }
    }

    /// Cerca su YouTube il testo scritto nella barra di ricerca (in un thread a parte).
    fn start_search(&mut self, ctx: &egui::Context) {
        let query = self.search.query.trim().to_string();
        if query.is_empty() || self.tools.ytdlp.is_none() || self.tools.busy {
            return;
        }
        // un link incollato nella ricerca va dritto nei link da scaricare
        if query.starts_with("http://") || query.starts_with("https://") {
            self.append_links(&[query]);
            self.search.query.clear();
            return;
        }
        // una ricerca nuova annulla quella eventualmente in corso
        self.search.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = cancel.clone();
        self.search.seq += 1;
        self.search.busy = true;
        self.search.error = None;
        self.search.notice = None;
        self.search.last_query = query.clone();
        self.search.artist = None;
        let seq = self.search.seq;
        let toolset = self.toolset();
        let cookies = self.cfg.cookies_browser.clone();
        let source = self.cfg.search_source;
        self.search.last_source = source;
        let send = self.sender(ctx);
        thread::spawn(move || {
            let result = search::search(source, &query, &toolset, &cookies, &cancel)
                .map_err(|e| format!("{e:#}"));
            send(Msg::SearchDone { seq, result });
        });
    }

    /// Completa i risultati in thread a parte, quattro alla volta: per gli album arrivati
    /// senza titolo chiede a yt-dlp i loro dati, poi scarica le copertine.
    /// Una copertina che non arriva lascia semplicemente il riquadro grigio.
    fn start_thumbnails(&self, ctx: &egui::Context, seq: u64, results: &[SearchResult]) {
        // gli album da completare per primi: sono quelli senza titolo
        let mut work: Vec<(usize, bool, String, Option<String>)> = results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.needs_details || r.thumbnail.is_some())
            .map(|(i, r)| (i, r.needs_details, r.url.clone(), r.thumbnail.clone()))
            .collect();
        work.sort_by_key(|w| !w.1);
        let work = Arc::new(std::sync::Mutex::new(work.into_iter()));
        // pixel reali (comprende lo zoom): su uno schermo Retina il riquadro vale il doppio
        let scale = ctx.pixels_per_point().max(1.0);
        let (w, h) = (
            (THUMB_SLOT.x * scale).round() as u32,
            (THUMB_SLOT.y * scale).round() as u32,
        );
        let toolset = self.toolset();
        let cookies = self.cfg.cookies_browser.clone();
        for _ in 0..4 {
            let work = work.clone();
            let cancel = self.search.cancel.clone();
            let send = self.sender(ctx);
            let (toolset, cookies) = (toolset.clone(), cookies.clone());
            thread::spawn(move || loop {
                let next = work.lock().ok().and_then(|mut it| it.next());
                let Some((idx, needs_details, url, mut thumb)) = next else {
                    break;
                };
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if needs_details {
                    let details = search::fetch_details(&url, &toolset, &cookies, &cancel).ok();
                    if thumb.is_none() {
                        thumb = details.as_ref().and_then(|d| d.thumbnail.clone());
                    }
                    send(Msg::Details { seq, idx, details });
                }
                let Some(thumb) = thumb else { continue };
                if let Ok((iw, ih, rgba)) = search::fetch_thumbnail(&thumb, w, h) {
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied([iw as usize, ih as usize], &rgba);
                    send(Msg::Thumb { seq, idx, image });
                }
            });
        }
    }

    fn cancel_search(&mut self) {
        self.search.cancel.store(true, Ordering::Relaxed);
        self.search.seq += 1; // l'eventuale risposta in arrivo verrà ignorata
        self.search.busy = false;
    }

    /// Aggiunge dei link in fondo al campo «Link», senza doppioni.
    fn append_links(&mut self, urls: &[String]) -> usize {
        let mut added = 0;
        for u in urls {
            if self.input.lines().any(|l| l.trim() == u) {
                continue;
            }
            if !self.input.is_empty() && !self.input.ends_with('\n') {
                self.input.push('\n');
            }
            self.input.push_str(u);
            added += 1;
        }
        added
    }

    fn pick_folder(&self, ctx: &egui::Context) {
        let send = self.sender(ctx);
        let start = self.cfg.out_dir.clone();
        thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .set_title(tr!(
                    "Scegli la cartella di download",
                    "Choose the download folder"
                ))
                .set_directory(start)
                .pick_folder()
            {
                send(Msg::Folder(p));
            }
        });
    }

    fn handle(&mut self, msg: Msg, ctx: &egui::Context) {
        match msg {
            Msg::ToolsStatus(s) => self.tools.status = s,
            Msg::ToolsReady {
                ytdlp,
                ffmpeg,
                js,
                warnings,
            } => {
                self.tools.busy = false;
                self.tools.ytdlp = Some(ytdlp);
                self.tools.ffmpeg = ffmpeg;
                self.tools.js = js;
                self.tools.warnings = warnings;
            }
            Msg::ToolsFailed(e) => {
                self.tools.busy = false;
                self.tools.error = Some(e);
            }
            Msg::UpdateCheck(result) => {
                self.update_checking = false;
                use selfupdate::Check;
                self.update_status = match result {
                    Err(e) => tr!(
                        format!("Controllo aggiornamenti non riuscito: {e}"),
                        format!("Update check failed: {e}")
                    ),
                    // build locale senza repository: niente da dire a chi usa l'app
                    Ok(Check::NotConfigured) => String::new(),
                    Ok(Check::NoRelease) => {
                        let repo = selfupdate::repository().unwrap_or_default();
                        tr!(
                            format!("Nessuna release trovata su GitHub ({repo}). Se il repository è privato, l'app non può leggerlo."),
                            format!("No release found on GitHub ({repo}). If the repository is private, the app cannot read it.")
                        )
                    }
                    Ok(Check::UpToDate) => tr!(
                        format!("Grabbit {} è aggiornato", selfupdate::CURRENT),
                        format!("Grabbit {} is up to date", selfupdate::CURRENT)
                    ),
                    Ok(Check::Newer(rel)) => {
                        let msg = tr!(
                            format!("Disponibile la versione {}", rel.version),
                            format!("Version {} is available", rel.version)
                        );
                        if self.cfg.auto_update_app
                            && self.auto_install
                            && selfupdate::supported()
                            && rel.asset_url.is_some()
                            && !matches!(self.update, UpdateUi::Working(_))
                        {
                            self.start_update(ctx, rel, true);
                        } else {
                            self.update = UpdateUi::Available(rel);
                        }
                        msg
                    }
                };
            }
            Msg::UpdateStatus(s) => self.update = UpdateUi::Working(s),
            Msg::UpdateDone(Ok(())) => {
                // aggiornamento partito da solo all'avvio: si riavvia da sé, a meno che
                // nel frattempo non sia iniziato un download
                if self.update_auto && !self.running {
                    self.cfg.save();
                    selfupdate::restart();
                }
                self.update = UpdateUi::Ready(
                    tr!(
                        "Aggiornamento installato. Riavvia per usarlo.",
                        "Update installed. Restart to use it."
                    )
                    .into(),
                )
            }
            Msg::UpdateDone(Err(e)) => {
                self.update = UpdateUi::Failed(selfupdate::describe_apply_error(&e))
            }
            Msg::Folder(p) => self.cfg.out_dir = p,
            Msg::Gain { path, gain } => {
                // non misurabile: nessuna correzione, e non si riprova a ogni ascolto
                let g = gain.unwrap_or(1.0);
                self.gains.insert(&path, g);
                if self.now_track().is_some_and(|t| t.path == path) {
                    self.apply_gain();
                }
            }
            Msg::Tracks { seq, idx, tracks } => {
                if seq == self.search.seq {
                    if let Some(p) = self.search.picks.get_mut(&idx) {
                        p.loading = false;
                        match tracks {
                            Some(t) => {
                                p.tracks =
                                    t.into_iter().map(|(n, title)| (n, title, true)).collect()
                            }
                            None => p.error = true,
                        }
                    }
                }
            }
            Msg::Trashed(Ok(dir)) => {
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.listen.notice = Some((
                    tr!(
                        format!("«{name}» è nel Cestino."),
                        format!("“{name}” is in the Trash.")
                    ),
                    false,
                ));
                self.listen.open = None;
                self.start_scan(ctx);
            }
            Msg::Trashed(Err(e)) => {
                self.listen.notice = Some((
                    tr!(
                        format!("Non sono riuscito a spostarlo nel Cestino: {e}"),
                        format!("Could not move it to the Trash: {e}")
                    ),
                    true,
                ))
            }
            Msg::MusicAdded(Ok(name)) => {
                self.listen.notice = Some((
                    tr!(
                        format!("«{name}» aggiunto a {}.", music_app()),
                        format!("“{name}” added to {}.", music_app())
                    ),
                    false,
                ))
            }
            Msg::MusicAdded(Err(e)) => {
                self.listen.notice = Some((
                    tr!(
                        format!("{} non ha aggiunto l'album: {e}", music_app()),
                        format!("{} did not add the album: {e}", music_app())
                    ),
                    true,
                ))
            }
            Msg::PlayerDir(p) => {
                self.cfg.player_dir = Some(p);
                self.cfg.save();
                self.listen.open = None;
                self.start_scan(ctx);
            }
            Msg::Library { seq, library } => {
                if seq == self.listen.seq {
                    self.library_arrived(ctx, library);
                }
            }
            Msg::Details { seq, idx, details } => {
                if seq == self.search.seq {
                    if let Some(r) = self.search.results.get_mut(idx) {
                        r.needs_details = false;
                        if let Some(d) = details {
                            if let Some(t) = d.title {
                                r.title = t;
                            }
                            if let Some(a) = d.artist {
                                r.channel = a;
                            }
                            r.tracks = d.tracks.or(r.tracks);
                            r.thumbnail = r.thumbnail.take().or(d.thumbnail);
                        }
                    }
                }
            }
            Msg::Thumb { seq, idx, image } => {
                if seq == self.search.seq {
                    if let Some(slot) = self.search.thumbs.get_mut(idx) {
                        *slot = Some(ctx.load_texture(
                            format!("thumb-{seq}-{idx}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
            Msg::SearchDone { seq, result } => {
                if seq != self.search.seq {
                    return; // risposta di una ricerca superata o annullata
                }
                self.search.busy = false;
                match result {
                    Ok(outcome) => {
                        let r = outcome.results;
                        self.search.artist = outcome.artist;
                        self.search.selected = vec![false; r.len()];
                        self.search.thumbs = vec![None; r.len()];
                        self.start_thumbnails(ctx, seq, &r);
                        self.search.results = r;
                        self.search.picks.clear();
                        if (self.listen.library.is_none() || self.listen.stale) && !self.listen.busy
                        {
                            self.start_scan(ctx);
                        }
                    }
                    Err(e) => self.search.error = Some(e),
                }
            }
            Msg::Dl(ev) => match ev {
                Event::Log(l) => {
                    self.log.push(l);
                    if self.log.len() > 2000 {
                        self.log.drain(..500);
                    }
                }
                Event::JobAdded { id, label } => {
                    self.next_id = self.next_id.max(id + 1);
                    self.jobs.push(JobUi {
                        id,
                        label,
                        pct: 0.0,
                        detail: String::new(),
                        state: JobState::Waiting,
                        spec: None,
                        file: None,
                    });
                }
                Event::JobRemoved { id } => self.jobs.retain(|j| j.id != id),
                Event::JobLabel { id, label } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                        j.label = label;
                    }
                }
                Event::JobProgress { id, pct, detail } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                        j.state = JobState::Running;
                        j.pct = pct;
                        j.detail = detail;
                    }
                }
                Event::JobFinished {
                    id,
                    ok,
                    detail,
                    file,
                } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                        j.state = if ok { JobState::Done } else { JobState::Failed };
                        j.pct = if ok { 1.0 } else { j.pct };
                        j.detail = detail;
                        j.file = file;
                    }
                }
                Event::JobSpec { id, job } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                        j.spec = Some(job);
                    }
                }
                Event::Finished => {
                    self.running = false;
                    self.notify_finished(ctx);
                    // nuovi brani nella cartella: «Ascolta» la rilegge alla prossima apertura
                    self.listen.stale = true;
                    // la libreria già letta (o «Già nella libreria» nei risultati) si aggiorna
                    // subito, senza premere «Aggiorna»
                    if (self.listen.library.is_some() || !self.search.results.is_empty())
                        && !self.listen.busy
                    {
                        self.start_scan(ctx);
                    }
                }
            },
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m, ctx);
        }
        self.player_tick(ctx);
        self.update_title(ctx);
        self.check_clipboard(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.player_keys(ctx);
        // il brano in ascolto resta in basso in tutte e due le schede
        self.now_playing_bar(ui);
        // «Scarica» e l'avanzamento restano visibili senza scorrere (sopra il player)
        if self.tab == Tab::Download {
            self.download_bar(ui, ctx);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    self.header(ui);
                    ui.add_space(6.0);
                    self.tab_bar(ui, ctx);
                    ui.add_space(6.0);
                    self.update_banner(ui, ctx);
                    match self.tab {
                        Tab::Download => {
                            self.tools_banner(ui, ctx);
                            ui.add_space(4.0);
                            self.search_card(ui, ctx);
                            ui.add_space(8.0);
                            self.link_card(ui);
                            ui.add_space(8.0);
                            self.options_card(ui, ctx);
                            ui.add_space(10.0);
                            self.queue(ui);
                            self.log_view(ui, ctx);
                        }
                        Tab::Listen => self.listen_view(ui, ctx),
                    }
                    ui.add_space(6.0);
                    self.footer(ui);
                });
        });
    }
}

// ───────────────────────────── sezioni UI ─────────────────────────────

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

impl App {
    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((self.logo.id(), egui::vec2(46.0, 46.0))).alt_text("Grabbit"));
            ui.vertical(|ui| {
                ui.label(RichText::new("Grabbit").size(28.0).strong());
                ui.label(
                    RichText::new(tr!(
                        "Video e musica da YouTube e YouTube Music, anche dai link Spotify",
                        "Video and music from YouTube and YouTube Music, Spotify links too"
                    ))
                    .color(ui.visuals().weak_text_color()),
                );
            });
        });
    }

    fn update_banner(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut start: Option<Release> = None;
        let mut restart = false;
        match &self.update {
            UpdateUi::None => return,
            UpdateUi::Available(rel) => {
                card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(tr!(
                                format!("Nuova versione {} disponibile", rel.version),
                                format!("New version {} available", rel.version)
                            ))
                            .strong()
                            .color(ACCENT),
                        );
                        if selfupdate::supported() && rel.asset_url.is_some() {
                            if ui.button(tr!("Aggiorna ora", "Update now")).clicked() {
                                start = Some(rel.clone());
                            }
                        } else if !rel.page_url.is_empty() {
                            ui.hyperlink_to(
                                tr!("Scarica dal sito", "Download from the website"),
                                &rel.page_url,
                            );
                        }
                    });
                    if rel.asset_url.is_none() {
                        ui.label(
                            RichText::new({
                                let f = selfupdate::asset_name().unwrap_or_default();
                                tr!(
                                    format!("La release non contiene il file «{f}» per questo sistema."),
                                    format!("The release does not contain the file “{f}” for this system.")
                                )
                            })
                            .small()
                            .color(WARN),
                        );
                    } else if !selfupdate::supported() {
                        ui.label(
                            RichText::new(selfupdate::unsupported_reason())
                                .small()
                                .color(WARN),
                        );
                    }
                    let notes = rel.notes.trim();
                    if !notes.is_empty() {
                        ui.collapsing(tr!("Novità", "What's new"), |ui| {
                            ui.label(RichText::new(notes).small());
                        });
                    }
                });
            }
            UpdateUi::Working(s) => {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(s);
                    });
                });
            }
            UpdateUi::Ready(s) => {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(s).color(OK));
                        if ui.button(tr!("Riavvia ora", "Restart now")).clicked() {
                            restart = true;
                        }
                    });
                });
            }
            UpdateUi::Failed(e) => {
                card(ui, |ui| {
                    ui.label(
                        RichText::new(tr!(
                            format!("Aggiornamento non riuscito: {e}"),
                            format!("Update failed: {e}")
                        ))
                        .color(ERR),
                    );
                });
            }
        }
        if let Some(r) = start {
            self.start_update(ctx, r, false);
        }
        if restart {
            selfupdate::restart();
        }
        ui.add_space(6.0);
    }

    fn tools_banner(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.tools.busy {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.tools.status);
                });
            });
            ui.add_space(6.0);
        }
        if let Some(e) = self.tools.error.clone() {
            card(ui, |ui| {
                ui.label(
                    RichText::new(tr!(
                        "Impossibile preparare yt-dlp",
                        "Could not set up yt-dlp"
                    ))
                    .strong()
                    .color(ERR),
                );
                ui.label(e);
                if ui.button(tr!("Riprova", "Retry")).clicked() {
                    self.start_setup(ctx);
                }
            });
            ui.add_space(6.0);
        }
        for w in &self.tools.warnings {
            ui.label(
                RichText::new(tr!(format!("Attenzione: {w}"), format!("Warning: {w}"))).color(WARN),
            );
        }
    }

    /// Barra di ricerca: cerca su YouTube e permette di scaricare i risultati scelti.
    fn search_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let field_id = ui.scope_id().with("search_field");
        let ready = self.tools.ytdlp.is_some() && !self.tools.busy;
        let can_download = ready && !self.running;
        let mut do_search = false;
        let mut do_cancel = false;
        let mut download: Vec<String> = Vec::new();
        let mut add: Vec<String> = Vec::new();
        let mut listen_album: Option<usize> = None;
        let mut toggle_pick: Vec<usize> = Vec::new();
        // risultati che ci sono già nella libreria di «Ascolta» (indice dell'album)
        let owned: Vec<Option<usize>> = match &self.listen.library {
            Some(lib) => self
                .search
                .results
                .iter()
                .map(|r| {
                    if r.collection {
                        lib.find_album(&r.title, &r.channel)
                    } else {
                        lib.find_track(&r.title, &r.channel).map(|(a, _)| a)
                    }
                })
                .collect(),
            None => Vec::new(),
        };
        let mut clear = false;

        card(ui, |ui| {
            let mut title_id = None;
            ui.horizontal(|ui| {
                title_id = Some(ui.label(RichText::new(tr!("Cerca", "Search")).strong()).id);
                ui.label(tr!("su", "on"));
                let before = self.cfg.search_source;
                egui::ComboBox::from_id_salt("search_source")
                    .selected_text(self.cfg.search_source.label())
                    .show_ui(ui, |ui| {
                        for src in search::Source::ALL {
                            ui.selectable_value(&mut self.cfg.search_source, src, src.label());
                        }
                    });
                if self.cfg.search_source != before {
                    self.cfg.save();
                    // cambiando sorgente si ripete subito la ricerca già fatta
                    if !self.search.last_query.is_empty() && ready {
                        self.search.query = self.search.last_query.clone();
                        do_search = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                let btn_w = if self.search.busy { 190.0 } else { 90.0 };
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search.query)
                        .id(field_id)
                        .desired_width((ui.available_width() - btn_w).max(120.0))
                        .hint_text(tr!(
                            "Artista e titolo di un brano, oppure il nome di un video",
                            "Artist and song title, or the name of a video"
                        )),
                );
                let resp = match title_id {
                    Some(id) => resp.labelled_by(id),
                    None => resp,
                };
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    do_search = true;
                }
                let has_query = !self.search.query.trim().is_empty();
                if ui
                    .add_enabled(
                        ready && has_query,
                        egui::Button::new(tr!("Cerca", "Search")),
                    )
                    .clicked()
                {
                    do_search = true;
                }
                if self.search.busy {
                    ui.spinner();
                    if ui.button(tr!("Annulla", "Cancel")).clicked() {
                        do_cancel = true;
                    }
                }
            });
            if !ready {
                ui.label(
                    RichText::new(tr!(
                        "La ricerca sarà disponibile appena yt-dlp è pronto.",
                        "Search will be available as soon as yt-dlp is ready."
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            }
            if self.cfg.search_source != search::Source::YouTube
                && self.cfg.cookies_browser.is_empty()
            {
                ui.label(
                    RichText::new(tr!(
                        "Per usare il tuo account Google, scegli il browser in cui hai fatto l'accesso in «Opzioni avanzate › Usa i cookie del browser».",
                        "To use your Google account, pick the browser you are signed in with in “Advanced options › Use browser cookies”."
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            }
            if let Some(e) = &self.search.error {
                ui.label(
                    RichText::new(tr!(
                        format!("Ricerca non riuscita: {e}"),
                        format!("Search failed: {e}")
                    ))
                    .color(ERR),
                );
            }
            if let Some(n) = &self.search.notice {
                ui.label(RichText::new(n).small().color(OK));
            }

            let res = &self.search.results;
            if res.is_empty() {
                if !self.search.busy
                    && self.search.error.is_none()
                    && !self.search.last_query.is_empty()
                {
                    ui.label(
                        RichText::new(tr!(
                            format!("Nessun risultato per «{}».", self.search.last_query),
                            format!("No results for “{}”.", self.search.last_query)
                        ))
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                return;
            }

            let what = match self.cfg.mode {
                Mode::Video => tr!("come video", "as video"),
                Mode::Audio => tr!("come audio", "as audio"),
            };
            let header = match (&self.search.artist, self.search.last_source) {
                (Some(name), search::Source::MusicArtist) => tr!(
                    format!(
                        "Discografia di {name} · «Scarica» scarica tutto l'album {what}, con le opzioni qui sotto."
                    ),
                    format!(
                        "Discography of {name} · “Download” saves the whole album {what}, with the options below."
                    )
                ),
                _ => {
                    let n = res.len();
                    let (count, count_en) = if n == 1 {
                        ("1 risultato".to_string(), "1 result".to_string())
                    } else {
                        (format!("{n} risultati"), format!("{n} results"))
                    };
                    tr!(
                        format!(
                            "{count} su {} per «{}» · «Scarica» li salva {what}, con le opzioni qui sotto.",
                            self.search.last_source.label(),
                            self.search.last_query
                        ),
                        format!(
                            "{count_en} on {} for “{}” · “Download” saves them {what}, with the options below.",
                            self.search.last_source.label(),
                            self.search.last_query
                        )
                    )
                }
            };
            ui.label(
                RichText::new(header)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            // discografia: filtri come quelli di YouTube Music (Album / Singoli ed EP)
            let artist_mode = self.search.last_source == search::Source::MusicArtist;
            if artist_mode {
                let albums = res.iter().filter(|r| is_album(r)).count();
                let singles = res.len() - albums;
                ui.horizontal_wrapped(|ui| {
                    for (f, label) in [
                        (
                            ReleaseFilter::Albums,
                            tr!(format!("Album ({albums})"), format!("Albums ({albums})")),
                        ),
                        (
                            ReleaseFilter::Singles,
                            tr!(
                                format!("Singoli ed EP ({singles})"),
                                format!("Singles and EPs ({singles})")
                            ),
                        ),
                        (
                            ReleaseFilter::All,
                            tr!(
                                format!("Tutto ({})", res.len()),
                                format!("All ({})", res.len())
                            ),
                        ),
                    ] {
                        let on = self.search.release_filter == f;
                        let text = RichText::new(label).color(if on {
                            ON_ACCENT
                        } else {
                            ui.visuals().text_color()
                        });
                        if ui.selectable_label(on, text).clicked() {
                            self.search.release_filter = f;
                        }
                    }
                });
            }
            let filter = if artist_mode {
                self.search.release_filter
            } else {
                ReleaseFilter::All
            };
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .id_salt("search_results")
                .max_height(330.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let mut shown = 0;
                    for (i, r) in self.search.results.iter().enumerate() {
                        if !filter.shows(r) {
                            continue;
                        }
                        if shown > 0 {
                            ui.separator();
                        }
                        shown += 1;
                        ui.horizontal(|ui| {
                            let sel = &mut self.search.selected[i];
                            let resp = ui.add(egui::Checkbox::without_text(sel));
                            let checked = *sel;
                            resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::CheckBox,
                                    true,
                                    checked,
                                    tr!(
                                        format!("Seleziona {}", r.title),
                                        format!("Select {}", r.title)
                                    ),
                                )
                            });

                            // copertina: un clic sopra spunta (o toglie la spunta a) il risultato
                            let (rect, cover) =
                                ui.allocate_exact_size(THUMB_SLOT, egui::Sense::click());
                            match self.search.thumbs.get(i).and_then(|t| t.as_ref()) {
                                Some(tex) => {
                                    egui::Image::new((tex.id(), fit(tex.size_vec2(), THUMB_SLOT)))
                                        .corner_radius(6)
                                        .paint_at(
                                            ui,
                                            egui::Rect::from_center_size(
                                                rect.center(),
                                                fit(tex.size_vec2(), THUMB_SLOT),
                                            ),
                                        );
                                }
                                None => {
                                    ui.painter().rect_filled(
                                        rect,
                                        6,
                                        Color32::from_rgb(36, 36, 40),
                                    );
                                    // disegnato: i caratteri ▶ e ♫ escono come quadratini
                                    let col = Color32::from_rgb(90, 90, 98);
                                    let c = rect.center();
                                    if r.collection {
                                        ui.painter().circle_filled(c + egui::vec2(-4.0, 6.0), 5.0, col);
                                        ui.painter().line_segment(
                                            [c + egui::vec2(0.5, 6.0), c + egui::vec2(0.5, -10.0)],
                                            egui::Stroke::new(2.0, col),
                                        );
                                    } else {
                                        ui.painter().add(egui::Shape::convex_polygon(
                                            vec![
                                                c + egui::vec2(-6.0, -9.0),
                                                c + egui::vec2(9.0, 0.0),
                                                c + egui::vec2(-6.0, 9.0),
                                            ],
                                            col,
                                            egui::Stroke::NONE,
                                        ));
                                    }
                                }
                            }
                            let cover = cover.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if cover.clicked() {
                                self.search.selected[i] = !checked;
                            }

                            ui.vertical(|ui| {
                                if r.needs_details {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new(tr!(
                                                "Album: carico titolo e copertina…",
                                                "Album: loading title and cover…"
                                            ))
                                            .color(ui.visuals().weak_text_color()),
                                        );
                                    });
                                } else {
                                    ui.add(
                                        egui::Label::new(RichText::new(&r.title).strong())
                                            .truncate(),
                                    )
                                    .on_hover_text(&r.title);
                                }
                                let mut meta: Vec<String> = Vec::new();
                                if r.collection {
                                    meta.push(release_kind(r));
                                }
                                if let Some(y) = &r.year {
                                    meta.push(y.clone());
                                }
                                if !r.channel.is_empty() {
                                    meta.push(r.channel.clone());
                                }
                                if let Some(n) = r.tracks {
                                    meta.push(tr!(format!("{n} brani"), format!("{n} tracks")));
                                }
                                if let Some(d) = r.duration {
                                    meta.push(search::fmt_duration(d));
                                }
                                if let Some(v) = r.views {
                                    meta.push(search::fmt_views(v));
                                }
                                if !meta.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(meta.join(" · "))
                                                .small()
                                                .color(ui.visuals().weak_text_color()),
                                        )
                                        .truncate(),
                                    );
                                }
                                if let Some(album) = owned.get(i).copied().flatten() {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(tr!(
                                                "Già nella libreria",
                                                "Already in the library"
                                            ))
                                            .small()
                                            .color(OK),
                                        );
                                        if ui
                                            .small_button(tr!("Ascolta", "Listen"))
                                            .on_hover_text(tr!(
                                                "Apri nella scheda Ascolta",
                                                "Open in the Listen tab"
                                            ))
                                            .clicked()
                                        {
                                            listen_album = Some(album);
                                        }
                                    });
                                }
                                ui.horizontal(|ui| {
                                    let dl = ui
                                        .add_enabled(
                                            can_download,
                                            egui::Button::new(tr!("Scarica", "Download")).small(),
                                        )
                                        .on_hover_text(if r.collection {
                                            tr!(
                                                "Scarica subito tutto l'album o la playlist, in una cartella con il suo nome",
                                                "Download the whole album or playlist now, in a folder with its name"
                                            )
                                        } else {
                                            tr!("Scarica subito questo risultato", "Download this result now")
                                        });
                                    dl.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            can_download,
                                            tr!(
                                                format!("Scarica {}", r.title),
                                                format!("Download {}", r.title)
                                            ),
                                        )
                                    });
                                    if dl.clicked() {
                                        download.push(r.url.clone());
                                    }
                                    let plus = ui.small_button("+").on_hover_text(tr!(
                                        "Aggiungi ai link da scaricare",
                                        "Add to the links to download"
                                    ));
                                    plus.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            true,
                                            tr!(
                                                format!("Aggiungi ai link: {}", r.title),
                                                format!("Add to links: {}", r.title)
                                            ),
                                        )
                                    });
                                    if plus.clicked() {
                                        add.push(r.url.clone());
                                    }
                                    if r.collection {
                                        let open = self.search.picks.get(&i).is_some_and(|p| p.open);
                                        let label = if open {
                                            tr!("Chiudi i brani", "Hide tracks")
                                        } else {
                                            tr!("Scegli i brani…", "Choose tracks…")
                                        };
                                        if ui
                                            .small_button(label)
                                            .on_hover_text(tr!(
                                                "Scegli quali brani dell'album scaricare",
                                                "Choose which tracks of the album to download"
                                            ))
                                            .clicked()
                                        {
                                            toggle_pick.push(i);
                                        }
                                    }
                                    ui.hyperlink_to(
                                        RichText::new(tr!("apri", "open")).small(),
                                        &r.url,
                                    );
                                });
                                if let Some(pick) = self.search.picks.get_mut(&i).filter(|p| p.open) {
                                    if let Some(sel) = pick_list(ui, pick, can_download) {
                                        download.push(downloader::pick_input(&r.url, &sel));
                                    }
                                }
                            });
                        });
                    }
                });

            // si scaricano solo i risultati spuntati e visibili con il filtro scelto
            let n_sel = self
                .search
                .results
                .iter()
                .zip(&self.search.selected)
                .filter(|(r, s)| **s && filter.shows(r))
                .count();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let picked = || {
                    self.search
                        .results
                        .iter()
                        .zip(&self.search.selected)
                        .filter(|(r, s)| **s && filter.shows(r))
                        .map(|(r, _)| r.url.clone())
                        .collect::<Vec<_>>()
                };
                if ui
                    .add_enabled(
                        can_download && n_sel > 0,
                        egui::Button::new(tr!(
                            format!("Scarica selezionati ({n_sel})"),
                            format!("Download selected ({n_sel})")
                        )),
                    )
                    .clicked()
                {
                    download = picked();
                }
                if ui
                    .add_enabled(
                        n_sel > 0,
                        egui::Button::new(tr!(
                            "Aggiungi selezionati ai link",
                            "Add selected to links"
                        )),
                    )
                    .clicked()
                {
                    add = picked();
                }
                if ui
                    .button(tr!("Pulisci risultati", "Clear results"))
                    .clicked()
                {
                    clear = true;
                }
            });
        });

        if do_cancel {
            self.cancel_search();
        } else if do_search {
            self.start_search(ctx);
        }
        if !add.is_empty() {
            let n = self.append_links(&add);
            self.search.notice = Some(match n {
                0 => tr!("Già presente nei link.", "Already in the links.").into(),
                1 => tr!(
                    "Aggiunto ai link: premi «Scarica» quando vuoi.",
                    "Added to the links: press “Download” when you like."
                )
                .into(),
                n => tr!(
                    format!("{n} link aggiunti: premi «Scarica» quando vuoi."),
                    format!("{n} links added: press “Download” when you like.")
                ),
            });
        }
        if !download.is_empty() {
            self.search.notice = None;
            self.start_jobs(ctx, download);
        }
        for i in toggle_pick {
            self.toggle_pick(ctx, i);
        }
        if let Some(a) = listen_album {
            self.tab = Tab::Listen;
            self.listen.open = Some(a);
            self.listen.filter.clear();
        }
        if clear {
            self.cancel_search();
            self.search.results.clear();
            self.search.selected.clear();
            self.search.thumbs.clear();
            self.search.last_query.clear();
            self.search.artist = None;
            self.search.error = None;
            self.search.notice = None;
        }
    }

    fn link_card(&mut self, ui: &mut egui::Ui) {
        let field_id = ui.scope_id().with("link_field");
        card(ui, |ui| {
            let mut paste = false;
            let mut link_label = None;
            ui.horizontal(|ui| {
                link_label = Some(ui.label(RichText::new("Link").strong()).id);
                paste = ui.small_button(tr!("Incolla", "Paste")).clicked();
                if ui.small_button(tr!("Svuota", "Clear")).clicked() {
                    self.input.clear();
                }
            });
            if let Some(link) = self.clip_offer.clone() {
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr!("Hai copiato un link:", "You copied a link:"))
                            .color(ACCENT_LIGHT),
                    );
                    let w = (ui.available_width() - 200.0).max(60.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_max_width(w);
                            ui.add(egui::Label::new(RichText::new(&link).small()).truncate());
                        },
                    );
                    if ui.small_button(tr!("Aggiungi", "Add")).clicked() {
                        self.append_links(std::slice::from_ref(&link));
                        close = true;
                    }
                    if ui.small_button(tr!("No, grazie", "No, thanks")).clicked() {
                        close = true;
                    }
                });
                if close {
                    self.clip_offer = None;
                }
            }
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.input)
                    .id(field_id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text(tr!(
                        "Incolla qui uno o più link YouTube o Spotify, uno per riga (Ctrl+V)",
                        "Paste one or more YouTube or Spotify links here, one per line (Ctrl+V)"
                    )),
            );
            let resp = match link_label {
                Some(id) => resp.labelled_by(id),
                None => resp,
            };
            resp.context_menu(|ui| {
                if ui.button(tr!("Incolla", "Paste")).clicked() {
                    paste = true;
                    ui.close();
                }
                if ui.button(tr!("Svuota", "Clear")).clicked() {
                    self.input.clear();
                    ui.close();
                }
            });
            if paste {
                // il testo incollato va al campo che ha il focus: glielo ridiamo
                ui.memory_mut(|m| m.request_focus(field_id));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
        });
    }

    fn options_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (Mode::Video, "  Video  "),
                    (Mode::Audio, tr!("  Audio (musica)  ", "  Audio (music)  ")),
                ] {
                    let selected = self.cfg.mode == mode;
                    let text = RichText::new(label).size(16.0).color(if selected {
                        ON_ACCENT
                    } else {
                        ui.visuals().text_color()
                    });
                    if ui.selectable_label(selected, text).clicked() {
                        self.cfg.mode = mode;
                    }
                }
            });
            ui.add_space(6.0);

            egui::Grid::new("opts")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    match self.cfg.mode {
                        Mode::Video => {
                            ui.label(tr!("Formato", "Format"));
                            egui::ComboBox::from_id_salt("vf")
                                .selected_text(label_of(&VIDEO_FORMATS, &self.cfg.video_container))
                                .show_ui(ui, |ui| {
                                    for (v, l) in VIDEO_FORMATS {
                                        ui.selectable_value(
                                            &mut self.cfg.video_container,
                                            v.to_string(),
                                            loc(l),
                                        );
                                    }
                                });
                            ui.end_row();
                            ui.label(tr!("Qualità massima", "Maximum quality"));
                            egui::ComboBox::from_id_salt("vh")
                                .selected_text(
                                    HEIGHTS
                                        .iter()
                                        .find(|h| h.0 == self.cfg.max_height)
                                        .map(|h| loc(h.1))
                                        .unwrap_or(tr!("Migliore disponibile", "Best available")),
                                )
                                .show_ui(ui, |ui| {
                                    for (v, l) in HEIGHTS {
                                        ui.selectable_value(&mut self.cfg.max_height, v, loc(l));
                                    }
                                });
                            ui.end_row();
                        }
                        Mode::Audio => {
                            ui.label(tr!("Formato", "Format"));
                            egui::ComboBox::from_id_salt("af")
                                .selected_text(label_of(&AUDIO_FORMATS, &self.cfg.audio_format))
                                .show_ui(ui, |ui| {
                                    for (v, l) in AUDIO_FORMATS {
                                        ui.selectable_value(
                                            &mut self.cfg.audio_format,
                                            v.to_string(),
                                            loc(l),
                                        );
                                    }
                                });
                            ui.end_row();
                            ui.label("Bitrate");
                            let lossless = matches!(self.cfg.audio_format.as_str(), "flac" | "wav");
                            ui.add_enabled_ui(!lossless, |ui| {
                                egui::ComboBox::from_id_salt("ab")
                                    .selected_text(if lossless {
                                        "—".to_string()
                                    } else {
                                        format!("{} kbps", self.cfg.audio_bitrate)
                                    })
                                    .show_ui(ui, |ui| {
                                        for b in BITRATES {
                                            ui.selectable_value(
                                                &mut self.cfg.audio_bitrate,
                                                b,
                                                format!("{b} kbps"),
                                            );
                                        }
                                    });
                            });
                            ui.end_row();
                        }
                    }
                    ui.label(tr!("Salva in", "Save to"));
                    ui.horizontal(|ui| {
                        if ui.button(tr!("Sfoglia…", "Browse…")).clicked() {
                            self.pick_folder(ctx);
                        }
                        ui.label(self.cfg.out_dir.to_string_lossy());
                    });
                    ui.end_row();
                });

            ui.add_space(4.0);
            ui.checkbox(
                &mut self.cfg.playlist,
                tr!(
                    "Scarica l'intera playlist (se il link è una playlist)",
                    "Download the whole playlist (if the link is a playlist)"
                ),
            );
            ui.checkbox(
                &mut self.cfg.embed_meta,
                tr!(
                    "Incorpora copertina e metadati",
                    "Embed cover art and metadata"
                ),
            );
            if self.cfg.mode == Mode::Video {
                ui.label(
                    RichText::new(tr!(
                        "I link Spotify vengono sempre scaricati come audio.",
                        "Spotify links are always downloaded as audio."
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            }

            ui.collapsing(tr!("Opzioni avanzate", "Advanced options"), |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr!("Usa i cookie del browser", "Use browser cookies"));
                    egui::ComboBox::from_id_salt("ck")
                        .selected_text(label_of(&BROWSERS, &self.cfg.cookies_browser))
                        .show_ui(ui, |ui| {
                            for (v, l) in BROWSERS {
                                ui.selectable_value(&mut self.cfg.cookies_browser, v.to_string(), loc(l));
                            }
                        });
                });
                ui.label(RichText::new(tr!("Utile per video con restrizioni d'età o quando YouTube chiede di dimostrare che non sei un robot.", "Useful for age-restricted videos or when YouTube asks you to prove you are not a robot.")).small().color(ui.visuals().weak_text_color()));
                ui.horizontal(|ui| {
                    ui.label(tr!("Dimensione del testo", "Text size"));
                    let sizes = [
                        (1.0_f32, tr!("Normale", "Normal")),
                        (1.15, tr!("Grande", "Large")),
                        (1.3, tr!("Molto grande", "Very large")),
                        (1.5, tr!("Enorme", "Huge")),
                    ];
                    let current = sizes
                        .iter()
                        .find(|(v, _)| (self.cfg.ui_scale - v).abs() < 0.01)
                        .map(|(_, l)| *l)
                        .unwrap_or(tr!("Personalizzata", "Custom"));
                    egui::ComboBox::from_id_salt("scale")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (v, l) in sizes {
                                if ui.selectable_value(&mut self.cfg.ui_scale, v, l).clicked() {
                                    ctx.set_zoom_factor(v);
                                    self.cfg.save();
                                }
                            }
                        });
                    ui.label(RichText::new(tr!("(oppure Ctrl + e Ctrl −)", "(or Ctrl + and Ctrl −)")).small().color(ui.visuals().weak_text_color()));
                });
                ui.horizontal(|ui| {
                    ui.label(tr!("Download contemporanei", "Simultaneous downloads"));
                    egui::ComboBox::from_id_salt("parallel")
                        .selected_text(self.cfg.parallel.clamp(1, 4).to_string())
                        .show_ui(ui, |ui| {
                            for n in 1..=4u32 {
                                if ui
                                    .selectable_value(&mut self.cfg.parallel, n, n.to_string())
                                    .clicked()
                                {
                                    self.cfg.save();
                                }
                            }
                        });
                    ui.label(
                        RichText::new(tr!(
                            "(brani scaricati insieme: gli album si scaricano prima)",
                            "(tracks downloaded at the same time: albums finish sooner)"
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                });
                ui.checkbox(&mut self.cfg.music_folders, tr!("Musica in cartelle Artista / Album", "Music in Artist / Album folders"))
                    .on_hover_text(tr!(
                        "Solo per l'audio: i brani vanno in Artista/Album/01 - Titolo, come in iTunes e Plex.",
                        "Audio only: tracks go in Artist/Album/01 - Title, as in iTunes and Plex."
                    ));
                ui.checkbox(&mut self.cfg.skip_downloaded, tr!("Salta i brani già scaricati", "Skip tracks already downloaded"))
                    .on_hover_text(tr!(
                        "Un brano già scaricato nello stesso formato non si riscarica, finché il file esiste.",
                        "A track already downloaded in the same format is not downloaded again, as long as the file exists."
                    ));
                ui.checkbox(&mut self.cfg.auto_update_app, tr!("Installa automaticamente gli aggiornamenti di Grabbit", "Install Grabbit updates automatically"));
                ui.horizontal(|ui| {
                    let can_check = !self.update_checking && !matches!(self.update, UpdateUi::Working(_));
                    if ui.add_enabled(can_check, egui::Button::new(tr!("Controlla aggiornamenti di Grabbit", "Check for Grabbit updates"))).clicked() {
                        self.auto_install = true;
                        self.start_update_check(ctx);
                    }
                    if ui.add_enabled(!self.tools.busy && !self.running, egui::Button::new(tr!("Aggiorna yt-dlp ora", "Update yt-dlp now"))).clicked() {
                        self.start_setup(ctx);
                    }
                    if ui.button(tr!("Apri cartella dei download", "Open downloads folder")).clicked() {
                        open_folder(&self.cfg.out_dir);
                    }
                });
            });
        });
    }

    /// Barra fissa in basso nella scheda Scarica: «Scarica» con il numero di link, oppure
    /// «Annulla» con l'avanzamento totale mentre si scarica.
    fn download_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let links = self.input.lines().filter(|l| !l.trim().is_empty()).count();
        let ready = self.tools.ytdlp.is_some() && !self.tools.busy;
        let total = self.jobs.len();
        let done = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Done)
            .count();
        let failed = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Failed)
            .count();
        egui::Panel::bottom("download_bar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 28, 31))
                    .inner_margin(10)
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 52, 57))),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.running {
                        let btn = egui::Button::new(
                            RichText::new(tr!("   Annulla   ", "   Cancel   ")).size(18.0),
                        )
                        .min_size(egui::vec2(0.0, 38.0));
                        let resp = ui.add(btn);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                tr!("Annulla i download", "Cancel the downloads"),
                            )
                        });
                        if resp.clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                        }
                    } else {
                        let enabled = ready && links > 0;
                        // attivo: arancione con testo scuro; disattivato: grigio, ben leggibile
                        let (fill, text_color) = if enabled {
                            (ACCENT, ON_ACCENT)
                        } else {
                            (
                                Color32::from_rgb(44, 44, 49),
                                Color32::from_rgb(130, 130, 138),
                            )
                        };
                        let btn = egui::Button::new(
                            RichText::new(tr!("   Scarica   ", "   Download   "))
                                .size(18.0)
                                .strong()
                                .color(text_color),
                        )
                        .fill(fill)
                        .min_size(egui::vec2(0.0, 38.0));
                        // niente add_enabled: egui sfumerebbe ancora il testo; basta non
                        // rendere cliccabile il pulsante
                        let btn = btn.sense(if enabled {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        });
                        let resp = ui.add(btn);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                enabled,
                                tr!("Scarica", "Download"),
                            )
                        });
                        if resp.clicked() {
                            self.start_download(ctx);
                        }
                    }
                    ui.vertical(|ui| {
                        let weak = ui.visuals().weak_text_color();
                        if total == 0 {
                            let text = match links {
                                0 => tr!(
                                    "Nessun link: cerca o incolla un link",
                                    "No links: search or paste a link"
                                )
                                .to_string(),
                                1 => tr!("1 link pronto", "1 link ready").to_string(),
                                n => tr!(format!("{n} link pronti"), format!("{n} links ready")),
                            };
                            let text = if self.running {
                                tr!("Preparo i download…", "Preparing the downloads…").into()
                            } else {
                                text
                            };
                            ui.add_space(8.0);
                            ui.label(RichText::new(text).color(weak));
                            return;
                        }
                        let running_pct: f32 = self
                            .jobs
                            .iter()
                            .filter(|j| j.state == JobState::Running)
                            .map(|j| j.pct)
                            .sum();
                        let frac = ((done + failed) as f32 + running_pct) / total as f32;
                        let mut summary = tr!(
                            format!("{done} di {total} completati"),
                            format!("{done} of {total} completed")
                        );
                        if failed > 0 {
                            summary.push_str(&tr!(
                                format!(" · {failed} non riusciti"),
                                format!(" · {failed} failed")
                            ));
                        }
                        if self.running {
                            // avanzamento di tutto il download (un album: l'album intero)
                            summary.push_str(&format!(" · {:.0}%", frac.clamp(0.0, 1.0) * 100.0));
                        }
                        let color = if self.running {
                            ui.visuals().text_color()
                        } else if failed > 0 {
                            ERR
                        } else {
                            OK
                        };
                        ui.horizontal(|ui| {
                            if self.running {
                                ui.spinner();
                            }
                            ui.label(RichText::new(summary).color(color));
                        });
                        if self.running {
                            ui.add(
                                egui::ProgressBar::new(frac.clamp(0.0, 1.0))
                                    .desired_height(8.0)
                                    .desired_width((ui.available_width() - 200.0).max(80.0)),
                            );
                        } else if links > 0 {
                            ui.label(
                                RichText::new(match links {
                                    1 => tr!("1 link pronto", "1 link ready").to_string(),
                                    n => {
                                        tr!(format!("{n} link pronti"), format!("{n} links ready"))
                                    }
                                })
                                .small()
                                .color(weak),
                            );
                        }
                    });
                    if total > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !self.running
                                && ui.button(tr!("Pulisci elenco", "Clear list")).clicked()
                            {
                                self.jobs.clear();
                            }
                            let retryable: Vec<usize> = self
                                .jobs
                                .iter()
                                .filter(|j| j.state == JobState::Failed && j.spec.is_some())
                                .map(|j| j.id)
                                .collect();
                            if !self.running && !retryable.is_empty() {
                                let label = tr!(
                                    format!("Riprova i non riusciti ({})", retryable.len()),
                                    format!("Retry the failed ones ({})", retryable.len())
                                );
                                if ui.button(label).clicked() {
                                    self.retry_jobs(ctx, &retryable);
                                }
                            }
                            if ui
                                .button(tr!("Vedi elenco", "Show list"))
                                .on_hover_text(tr!(
                                    "Mostra l'elenco dei download, brano per brano",
                                    "Show the list of downloads, track by track"
                                ))
                                .clicked()
                            {
                                self.scroll_to_queue = true;
                            }
                        });
                    }
                });
            });
    }

    fn queue(&mut self, ui: &mut egui::Ui) {
        if self.jobs.is_empty() {
            return;
        }
        let scroll = std::mem::take(&mut self.scroll_to_queue);
        let mut stop: Vec<usize> = Vec::new();
        let mut retry: Vec<usize> = Vec::new();
        card(ui, |ui| {
            if scroll {
                ui.scroll_to_cursor(Some(egui::Align::TOP));
            }
            ui.label(RichText::new("Download").strong());
            for j in &self.jobs {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let col = match j.state {
                        JobState::Waiting => ui.visuals().weak_text_color(),
                        JobState::Running => ACCENT,
                        JobState::Done => OK,
                        JobState::Failed => ERR,
                    };
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 14.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, col);
                    let state = match j.state {
                        JobState::Waiting => tr!("in attesa", "waiting"),
                        JobState::Running => tr!("in corso", "in progress"),
                        JobState::Done => tr!("completato", "completed"),
                        JobState::Failed => tr!("non riuscito", "failed"),
                    };
                    let resp = ui.add(egui::Label::new(&j.label).truncate());
                    // il colore da solo non basta: lo stato va detto anche a parole
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Label,
                            true,
                            format!("{}: {state}", j.label),
                        )
                    });
                    if j.detail.is_empty() {
                        ui.label(RichText::new(state).small().color(col));
                    }
                });
                if j.state == JobState::Running {
                    ui.add(egui::ProgressBar::new(j.pct).desired_height(8.0));
                }
                // seconda riga: stato a parole e l'azione possibile su quel brano
                ui.horizontal(|ui| {
                    if j.state == JobState::Running {
                        // percentuale del brano davanti a velocità e tempo rimasto
                        let pct = format!("{:.0}%", j.pct * 100.0);
                        let text = if j.detail.is_empty() {
                            pct
                        } else {
                            format!("{pct} · {}", j.detail)
                        };
                        ui.label(
                            RichText::new(text)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    } else if !j.detail.is_empty() {
                        let col = if j.state == JobState::Failed {
                            ERR
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.label(RichText::new(&j.detail).small().color(col));
                    }
                    let busy = matches!(j.state, JobState::Running | JobState::Waiting);
                    if busy && self.running {
                        if ui
                            .small_button(tr!("Annulla", "Cancel"))
                            .on_hover_text(tr!("Ferma solo questo brano", "Stop only this track"))
                            .clicked()
                        {
                            stop.push(j.id);
                        }
                    } else if j.state == JobState::Done {
                        if let Some(f) = &j.file {
                            if ui
                                .small_button(reveal_label())
                                .on_hover_text(f.display().to_string())
                                .clicked()
                            {
                                reveal(f);
                            }
                        }
                    } else if j.state == JobState::Failed
                        && !self.running
                        && j.spec.is_some()
                        && ui
                            .small_button(tr!("Riprova", "Retry"))
                            .on_hover_text(tr!(
                                "Scarica di nuovo solo questo brano",
                                "Download only this track again"
                            ))
                            .clicked()
                    {
                        retry.push(j.id);
                    }
                });
            }
        });
        if !stop.is_empty() {
            if let Ok(mut s) = self.stop.lock() {
                s.extend(stop);
            }
        }
        if !retry.is_empty() {
            let ctx = ui.ctx().clone();
            self.retry_jobs(&ctx, &retry);
        }
    }

    /// A fine download, con la finestra in secondo piano: notifica di sistema e richiamo
    /// dell'attenzione (icona che saltella nel Dock, pulsante che lampeggia nella barra).
    fn notify_finished(&mut self, ctx: &egui::Context) {
        if self.jobs.is_empty() || ctx.input(|i| i.viewport().focused).unwrap_or(true) {
            return;
        }
        let done = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Done)
            .count();
        let failed = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Failed)
            .count();
        let body = if failed == 0 {
            tr!(
                format!("Download finito: {done} scaricati"),
                format!("Downloads finished: {done} downloaded")
            )
        } else {
            tr!(
                format!("Download finito: {done} scaricati, {failed} non riusciti"),
                format!("Downloads finished: {done} downloaded, {failed} failed")
            )
        };
        notify("Grabbit", &body);
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
            egui::UserAttentionType::Informational,
        ));
    }

    /// Titolo della finestra: «Grabbit — 45%» mentre scarica, visibile anche da coperta.
    fn update_title(&mut self, ctx: &egui::Context) {
        let title = if self.running && !self.jobs.is_empty() {
            let total = self.jobs.len() as f32;
            let sum: f32 = self
                .jobs
                .iter()
                .map(|j| match j.state {
                    JobState::Done | JobState::Failed => 1.0,
                    JobState::Running => j.pct,
                    JobState::Waiting => 0.0,
                })
                .sum();
            format!("Grabbit — {:.0}%", (sum / total).clamp(0.0, 1.0) * 100.0)
        } else {
            "Grabbit".to_string()
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Al ritorno sulla finestra: se negli appunti c'è un link nuovo, lo propone.
    fn check_clipboard(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let gained = focused && !self.was_focused;
        self.was_focused = focused;
        if !gained {
            return;
        }
        let Some(text) = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
        else {
            return;
        };
        if text == self.clip_seen {
            return;
        }
        self.clip_seen = text.clone();
        let Some(link) = copied_link(&text) else {
            return;
        };
        if self.input.lines().any(|l| l.trim() == link) {
            return;
        }
        self.clip_offer = Some(link);
        ctx.request_repaint();
    }

    fn log_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.log.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let mut copy = false;
        ui.collapsing(tr!("Registro dettagliato", "Detailed log"), |ui| {
            ui.horizontal(|ui| {
                copy = ui
                    .button(tr!("Copia tutto il registro", "Copy the whole log"))
                    .on_hover_text(tr!(
                        "Copia negli appunti il registro, con le versioni di Grabbit e degli strumenti",
                        "Copy the log to the clipboard, with the versions of Grabbit and its tools"
                    ))
                    .clicked();
                if let Some(t) = self.log_copied {
                    if t.elapsed().as_secs() < 3 {
                        ui.label(
                            RichText::new(tr!("Copiato negli appunti", "Copied to the clipboard"))
                                .small()
                                .color(OK),
                        );
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    } else {
                        self.log_copied = None;
                    }
                }
            });
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &self.log {
                        ui.label(RichText::new(l).monospace().small());
                    }
                });
        });
        if copy {
            ctx.copy_text(self.log_report());
            self.log_copied = Some(std::time::Instant::now());
        }
    }

    /// Il registro intero, preceduto da versioni e sistema: pronto da incollare in una
    /// segnalazione.
    fn log_report(&self) -> String {
        let yes = |b: bool| if b { "ok" } else { tr!("manca", "missing") };
        let mut out = format!(
            "Grabbit {} · {} {} · yt-dlp {} · ffmpeg {} · {} {}\n\n",
            selfupdate::CURRENT,
            std::env::consts::OS,
            std::env::consts::ARCH,
            self.tools.ytdlp.as_deref().unwrap_or("?"),
            yes(self.tools.ffmpeg.is_some()),
            self.tools.js.as_ref().map(|j| j.name()).unwrap_or("Deno"),
            yes(self.tools.js.is_some()),
        );
        for l in &self.log {
            out.push_str(l);
            out.push('\n');
        }
        out
    }

    fn footer(&self, ui: &mut egui::Ui) {
        ui.separator();
        let yt = self.tools.ytdlp.as_deref().unwrap_or("…");
        let ok = |b: bool| if b { "ok" } else { tr!("manca", "missing") };
        ui.label(
            RichText::new(format!(
                "Grabbit {} · yt-dlp {} · ffmpeg {} · {} {}",
                selfupdate::CURRENT,
                yt,
                ok(self.tools.ffmpeg.is_some()),
                self.tools.js.as_ref().map(|j| j.name()).unwrap_or("Deno"),
                ok(self.tools.js.is_some()),
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        if !self.update_status.is_empty() {
            ui.label(
                RichText::new(&self.update_status)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        }
        ui.label(
            RichText::new(tr!("Usalo solo per contenuti che hai il diritto di scaricare, nel rispetto del copyright e dei termini dei servizi.", "Use it only for content you have the right to download, respecting copyright and the services' terms."))
                .small()
                .color(ui.visuals().weak_text_color()),
        );
    }
}

// ───────────────────────────── Ascolta (player) ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Download,
    Listen,
}

#[derive(Default)]
struct ListenUi {
    library: Option<player::Library>,
    busy: bool,
    seq: u64,
    cancel: Arc<AtomicBool>,
    /// la cartella è cambiata dall'ultima lettura (nuovi download): va riletta
    stale: bool,
    covers: Vec<Option<egui::TextureHandle>>,
    /// album aperto (indice in `library.albums`)
    open: Option<usize>,
    /// brano in riproduzione: (album, brano)
    now: Option<(usize, usize)>,
    last_ended: u64,
    /// posizione trascinata sulla barra, finché non si rilascia
    seek_drag: Option<f32>,
    /// testo di «Cerca nella libreria»
    filter: String,
    /// elenco scelto (un album o tutto un artista) nel suo ordine
    base: Vec<PathBuf>,
    /// ordine di ascolto: `base`, oppure mescolato con «Casuale»
    queue: Vec<PathBuf>,
    /// posizione in `queue` del brano in ascolto
    qpos: usize,
    /// l'ultimo brano ascoltato è già stato ripreso (una volta sola, all'avvio)
    restored: bool,
    /// quando è stata salvata l'ultima posizione nel brano
    last_save: Option<std::time::Instant>,
    /// album da confermare prima di spostarlo nel Cestino
    confirm_trash: Option<usize>,
    /// esito dell'ultima azione sulla libreria (Cestino, Musica): testo, è un errore
    notice: Option<(String, bool)>,
}

/// Icone dei pulsanti del player, disegnate (i caratteri ▶ ⏸ non ci sono nei font di serie).
#[derive(Clone, Copy)]
enum Icon {
    Play,
    Pause,
    Prev,
    Next,
}

/// Pulsante con un'icona disegnata; `label` è il nome letto dal lettore di schermo.
fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, big: bool) -> egui::Response {
    let side = if big { 40.0 } else { 32.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let resp = resp.on_hover_text(label);
    let visuals = ui.style().interact(&resp);
    let (fill, fg) = if big {
        (ACCENT, ON_ACCENT)
    } else {
        (visuals.weak_bg_fill, visuals.fg_stroke.color)
    };
    ui.painter()
        .circle(rect.center(), side / 2.0, fill, visuals.bg_stroke);
    let c = rect.center();
    let s = side * 0.22;
    let p = ui.painter();
    let tri = |dir: f32, x: f32| {
        // triangolo che punta a destra (dir = 1) o a sinistra (dir = -1), centrato in x
        vec![
            egui::pos2(x - dir * s * 0.8, c.y - s),
            egui::pos2(x - dir * s * 0.8, c.y + s),
            egui::pos2(x + dir * s * 0.9, c.y),
        ]
    };
    let shape = |pts: Vec<egui::Pos2>| egui::Shape::convex_polygon(pts, fg, egui::Stroke::NONE);
    match icon {
        Icon::Play => {
            p.add(shape(tri(1.0, c.x + s * 0.15)));
        }
        Icon::Pause => {
            for dx in [-s * 0.55, s * 0.55] {
                p.rect_filled(
                    egui::Rect::from_center_size(
                        egui::pos2(c.x + dx, c.y),
                        egui::vec2(s * 0.5, s * 2.0),
                    ),
                    1,
                    fg,
                );
            }
        }
        Icon::Prev | Icon::Next => {
            let dir = if matches!(icon, Icon::Next) {
                1.0
            } else {
                -1.0
            };
            p.add(shape(tri(dir, c.x - dir * s * 0.25)));
            p.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(c.x + dir * s * 0.95, c.y),
                    egui::vec2(s * 0.35, s * 2.0),
                ),
                1,
                fg,
            );
        }
    }
    resp
}

/// Riquadro della copertina (o grigio con una nota disegnata, se manca).
fn cover_box(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, side: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    match tex {
        Some(t) => {
            egui::Image::new((t.id(), egui::vec2(side, side)))
                .corner_radius(6)
                .paint_at(ui, rect);
        }
        None => {
            ui.painter()
                .rect_filled(rect, 6, Color32::from_rgb(36, 36, 40));
            // una croma stilizzata
            let c = rect.center();
            let r = side * 0.12;
            let col = Color32::from_rgb(90, 90, 98);
            ui.painter()
                .circle_filled(egui::pos2(c.x - r * 0.6, c.y + r * 1.4), r, col);
            ui.painter().line_segment(
                [
                    egui::pos2(c.x - r * 0.6 + r * 0.9, c.y + r * 1.4),
                    egui::pos2(c.x - r * 0.6 + r * 0.9, c.y - r * 2.2),
                ],
                egui::Stroke::new((r * 0.35).max(1.5), col),
            );
        }
    }
    resp
}

impl App {
    fn listen_dir(&self) -> PathBuf {
        self.cfg
            .player_dir
            .clone()
            .unwrap_or_else(|| self.cfg.out_dir.clone())
    }

    /// Rilegge la cartella della musica (in un thread a parte).
    fn start_scan(&mut self, ctx: &egui::Context) {
        self.listen.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.listen.cancel = cancel.clone();
        self.listen.seq += 1;
        self.listen.busy = true;
        self.listen.stale = false;
        let seq = self.listen.seq;
        let dir = self.listen_dir();
        let px = (COVER_LIST * ctx.pixels_per_point().max(1.0)).round() as u32 * 2;
        let send = self.sender(ctx);
        thread::spawn(move || {
            let library = player::scan(&dir, px, &cancel);
            send(Msg::Library { seq, library });
        });
    }

    fn pick_player_dir(&self, ctx: &egui::Context) {
        let send = self.sender(ctx);
        let start = self.listen_dir();
        thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .set_title(tr!(
                    "Scegli la cartella della musica",
                    "Choose the music folder"
                ))
                .set_directory(start)
                .pick_folder()
            {
                send(Msg::PlayerDir(p));
            }
        });
    }

    fn library_arrived(&mut self, ctx: &egui::Context, library: player::Library) {
        // il brano in riproduzione può aver cambiato posto: lo si ritrova dal percorso
        let playing = self.now_track().map(|t| t.path.clone());
        self.listen.covers = library
            .albums
            .iter()
            .enumerate()
            .map(|(i, a)| {
                a.cover.as_ref().map(|(w, h, rgba)| {
                    ctx.load_texture(
                        format!("cover-{}-{i}", self.listen.seq),
                        egui::ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], rgba),
                        egui::TextureOptions::LINEAR,
                    )
                })
            })
            .collect();
        self.listen.now =
            playing.and_then(|p| {
                library.albums.iter().enumerate().find_map(|(ai, a)| {
                    a.tracks.iter().position(|t| t.path == p).map(|ti| (ai, ti))
                })
            });
        if self.listen.open.is_some_and(|i| i >= library.albums.len()) {
            self.listen.open = None;
        }
        // i brani non ancora misurati, in sottofondo, uno alla volta
        if self.cfg.normalize {
            for t in library.albums.iter().flat_map(|a| &a.tracks) {
                if self.gains.get(&t.path).is_none() {
                    self.analyzer.later(&t.path);
                }
            }
        }
        self.listen.library = Some(library);
        self.listen.busy = false;
        self.restore_last_track();
    }

    fn now_track(&self) -> Option<&player::Track> {
        let (a, t) = self.listen.now?;
        self.listen.library.as_ref()?.albums.get(a)?.tracks.get(t)
    }

    /// Correzione da usare per un brano (1 se il volume uguale è spento o non ancora misurato).
    fn gain_for(&self, path: &std::path::Path) -> f32 {
        if self.cfg.normalize {
            self.gains.get(path).unwrap_or(1.0)
        } else {
            1.0
        }
    }

    /// Aggiorna la correzione del brano in ascolto (dopo una misura o l'opzione cambiata).
    fn apply_gain(&mut self) {
        if let Some(path) = self.now_track().map(|t| t.path.clone()) {
            self.engine.set_gain(self.gain_for(&path));
        }
    }

    /// (album, brano) del file nella libreria.
    fn locate(&self, path: &std::path::Path) -> Option<(usize, usize)> {
        let lib = self.listen.library.as_ref()?;
        lib.albums.iter().enumerate().find_map(|(ai, a)| {
            a.tracks
                .iter()
                .position(|t| t.path == path)
                .map(|ti| (ai, ti))
        })
    }

    fn play_at(&mut self, album: usize, track: usize) {
        let Some(t) = self
            .listen
            .library
            .as_ref()
            .and_then(|l| l.albums.get(album))
            .and_then(|a| a.tracks.get(track))
        else {
            return;
        };
        let path = t.path.clone();
        self.engine.set_volume(self.cfg.volume);
        self.engine.set_gain(self.gain_for(&path));
        if self.cfg.normalize && self.gains.get(&path).is_none() {
            self.analyzer.urgent(&path);
        }
        self.engine.play(&path);
        self.listen.now = Some((album, track));
        self.listen.seek_drag = None;
        self.cfg.last_track = Some(path);
        self.cfg.last_pos = 0.0;
        self.cfg.save();
        self.listen.last_save = Some(std::time::Instant::now());
    }

    /// Ascolta un elenco di brani (un album, tutto un artista) a partire da `start`.
    fn start_queue(&mut self, list: Vec<PathBuf>, start: usize) {
        if list.is_empty() {
            return;
        }
        let start = start.min(list.len() - 1);
        self.listen.base = list;
        if self.cfg.shuffle {
            // il brano scelto per primo, gli altri mescolati
            let first = self.listen.base[start].clone();
            let mut rest: Vec<PathBuf> = self
                .listen
                .base
                .iter()
                .filter(|p| **p != first)
                .cloned()
                .collect();
            shuffle(&mut rest);
            self.listen.queue = std::iter::once(first).chain(rest).collect();
            self.listen.qpos = 0;
        } else {
            self.listen.queue = self.listen.base.clone();
            self.listen.qpos = start;
        }
        self.play_current();
    }

    fn play_current(&mut self) {
        let Some(path) = self.listen.queue.get(self.listen.qpos).cloned() else {
            return;
        };
        // il brano dopo si misura subito dopo quello in ascolto: sarà pronto quando parte
        if let Some(next) = self.listen.queue.get(self.listen.qpos + 1) {
            if self.cfg.normalize && self.gains.get(next).is_none() {
                self.analyzer.urgent(next);
            }
        }
        match self.locate(&path) {
            Some((a, t)) => self.play_at(a, t),
            // il file non c'è più (cartella riletta): si passa al successivo
            None => {
                self.listen.queue.remove(self.listen.qpos);
                if self.listen.qpos < self.listen.queue.len() {
                    self.play_current();
                }
            }
        }
    }

    /// «Ascolta l'album» o un clic su un brano: l'album, da quel brano.
    fn play_album_from(&mut self, album: usize, track: usize) {
        let list: Vec<PathBuf> = self
            .listen
            .library
            .as_ref()
            .and_then(|l| l.albums.get(album))
            .map(|a| a.tracks.iter().map(|t| t.path.clone()).collect())
            .unwrap_or_default();
        self.start_queue(list, track);
    }

    /// «Ascolta tutto»: le uscite di un artista di fila, nell'ordine dell'elenco.
    fn play_albums(&mut self, albums: &[usize]) {
        let list: Vec<PathBuf> = match &self.listen.library {
            Some(lib) => albums
                .iter()
                .filter_map(|&i| lib.albums.get(i))
                .flat_map(|a| a.tracks.iter().map(|t| t.path.clone()))
                .collect(),
            None => return,
        };
        self.start_queue(list, 0);
    }

    /// Brano successivo. `auto`: il brano è finito da solo (allora vale «Ripeti brano»).
    fn play_next(&mut self, auto: bool) {
        if self.listen.now.is_none() {
            return;
        }
        if auto && self.cfg.repeat == Repeat::One {
            self.play_current();
        } else if self.listen.qpos + 1 < self.listen.queue.len() {
            self.listen.qpos += 1;
            self.play_current();
        } else if self.cfg.repeat == Repeat::All && !self.listen.queue.is_empty() {
            if self.cfg.shuffle {
                shuffle(&mut self.listen.queue);
            }
            self.listen.qpos = 0;
            self.play_current();
        } else {
            // fine dell'elenco
            self.engine.stop();
            self.listen.now = None;
            self.cfg.last_track = None;
            self.cfg.save();
        }
    }

    fn play_prev(&mut self) {
        if self.listen.now.is_none() {
            return;
        }
        // come nei lettori musicali: dopo i primi secondi «indietro» riparte dall'inizio
        if self.engine.status().pos > Duration::from_secs(3) || self.listen.qpos == 0 {
            self.engine.seek(Duration::ZERO);
        } else {
            self.listen.qpos -= 1;
            self.play_current();
        }
    }

    fn toggle_play(&mut self) {
        let st = self.engine.status();
        if st.loaded {
            if st.paused {
                self.engine.resume();
            } else {
                self.engine.pause();
                self.save_position(st.pos);
            }
        } else if self.listen.now.is_some() {
            self.play_current();
        }
    }

    /// «Casuale»: mescola i brani che restano, o rimette l'ordine dell'elenco.
    fn toggle_shuffle(&mut self) {
        self.cfg.shuffle = !self.cfg.shuffle;
        self.cfg.save();
        let Some(current) = self.listen.queue.get(self.listen.qpos).cloned() else {
            return;
        };
        if self.cfg.shuffle {
            let mut rest: Vec<PathBuf> = self
                .listen
                .base
                .iter()
                .filter(|p| **p != current)
                .cloned()
                .collect();
            shuffle(&mut rest);
            self.listen.queue = std::iter::once(current).chain(rest).collect();
            self.listen.qpos = 0;
        } else {
            self.listen.queue = self.listen.base.clone();
            self.listen.qpos = self
                .listen
                .queue
                .iter()
                .position(|p| *p == current)
                .unwrap_or(0);
        }
    }

    /// Ricorda brano e posizione, per riprendere alla riapertura.
    fn save_position(&mut self, pos: Duration) {
        self.listen.last_save = Some(std::time::Instant::now());
        if self.listen.now.is_some() {
            self.cfg.last_pos = pos.as_secs_f32();
            self.cfg.save();
        }
    }

    /// All'avvio, alla prima lettura della libreria: l'ultimo brano ascoltato, in pausa
    /// dove era rimasto, con il suo album come elenco.
    fn restore_last_track(&mut self) {
        if self.listen.restored || self.listen.now.is_some() {
            return;
        }
        self.listen.restored = true;
        let Some(path) = self.cfg.last_track.clone() else {
            return;
        };
        let Some((a, t)) = self.locate(&path) else {
            return;
        };
        let list: Vec<PathBuf> = self.listen.library.as_ref().map_or(Vec::new(), |l| {
            l.albums[a].tracks.iter().map(|t| t.path.clone()).collect()
        });
        self.listen.base = list.clone();
        self.listen.queue = list;
        self.listen.qpos = t;
        self.listen.now = Some((a, t));
        self.engine.set_volume(self.cfg.volume);
        self.engine.set_gain(self.gain_for(&path));
        self.engine
            .load_paused(&path, Duration::from_secs_f32(self.cfg.last_pos.max(0.0)));
    }

    /// Tasti del player, quando non si sta scrivendo: spazio = pausa o riprendi,
    /// frecce sinistra e destra = brano precedente e successivo.
    fn player_keys(&mut self, ctx: &egui::Context) {
        if self.listen.now.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, left, right) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
            )
        });
        if space {
            self.toggle_play();
        }
        if left {
            self.play_prev();
        }
        if right {
            self.play_next(false);
        }
        if space || left || right {
            ctx.request_repaint();
        }
    }

    /// Da chiamare a ogni fotogramma: passa al brano successivo quando uno finisce.
    fn player_tick(&mut self, ctx: &egui::Context) {
        let st = self.engine.status();
        if st.ended != self.listen.last_ended {
            self.listen.last_ended = st.ended;
            if self.listen.now.is_some() {
                self.play_next(true);
            }
        }
        // ogni tanto si salva dove si è arrivati, per riprendere alla riapertura
        if st.loaded && !st.paused {
            let due = self
                .listen
                .last_save
                .is_none_or(|t| t.elapsed() > Duration::from_secs(10));
            if due {
                self.save_position(st.pos);
            }
        }
        // mentre suona (o sta per partire) la barra avanza da sola, senza aspettare il mouse
        if self.listen.now.is_some() && !st.paused {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Download, tr!("  Scarica  ", "  Download  ")),
                (Tab::Listen, tr!("  Ascolta  ", "  Listen  ")),
            ] {
                let on = self.tab == tab;
                let text = RichText::new(label).size(17.0).strong().color(if on {
                    ON_ACCENT
                } else {
                    ui.visuals().text_color()
                });
                if ui.selectable_label(on, text).clicked() && !on {
                    self.tab = tab;
                    if tab == Tab::Listen
                        && (self.listen.library.is_none() || self.listen.stale)
                        && !self.listen.busy
                    {
                        self.start_scan(ctx);
                    }
                }
            }
        });
    }

    fn listen_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut rescan = false;
        let mut play: Option<(usize, usize)> = None;
        let mut play_group: Option<Vec<usize>> = None;
        let mut add_music: Option<PathBuf> = None;
        let mut toggle_norm = false;
        let mut confirm: Option<usize> = None;
        let mut open: Option<Option<usize>> = None;
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(tr!("Musica in", "Music in")).strong());
                if ui.button(tr!("Sfoglia…", "Browse…")).clicked() {
                    self.pick_player_dir(ctx);
                }
                if ui
                    .add_enabled(
                        !self.listen.busy,
                        egui::Button::new(tr!("Aggiorna", "Refresh")),
                    )
                    .on_hover_text(tr!(
                        "Rilegge la cartella (per i brani scaricati nel frattempo)",
                        "Reads the folder again (for tracks downloaded in the meantime)"
                    ))
                    .clicked()
                {
                    rescan = true;
                }
                ui.label(
                    RichText::new(self.listen_dir().to_string_lossy())
                        .color(ui.visuals().weak_text_color()),
                );
            });
            let mut norm = self.cfg.normalize;
            if ui
                .checkbox(&mut norm, tr!("Volume uguale tra i brani", "Same volume for all tracks"))
                .on_hover_text(tr!(
                    "Grabbit misura ogni brano (una volta, in sottofondo) e porta tutti allo stesso volume, senza toccare i file.",
                    "Grabbit measures each track (once, in the background) and brings them all to the same volume, without changing the files."
                ))
                .changed()
            {
                toggle_norm = true;
            }
            if self.listen.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr!("Leggo la cartella…", "Reading the folder…"));
                });
            }
            let Some(lib) = &self.listen.library else {
                return;
            };
            if lib.albums.is_empty() && !self.listen.busy {
                ui.label(tr!(
                    "Nessun brano riproducibile in questa cartella (MP3, M4A, FLAC, WAV, OGG).",
                    "No playable tracks in this folder (MP3, M4A, FLAC, WAV, OGG)."
                ));
            }
            if lib.unsupported > 0 {
                ui.label(
                    RichText::new(tr!(
                        format!("{} file Opus/WebM non si possono ascoltare qui: per la musica scegli M4A o MP3.", lib.unsupported),
                        format!("{} Opus/WebM files cannot be played here: for music choose M4A or MP3.", lib.unsupported)
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            }
            ui.add_space(4.0);
            match self.listen.open.filter(|i| *i < lib.albums.len()) {
                None => {
                    // cerca nella libreria: artista, album o brano
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.listen.filter)
                            .hint_text(tr!(
                                "Cerca nella libreria: artista, album o brano",
                                "Search the library: artist, album or track"
                            ))
                            .desired_width(f32::INFINITY),
                    );
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            tr!("Cerca nella libreria", "Search the library"),
                        )
                    });
                    if let Some((text, err)) = &self.listen.notice {
                        ui.label(
                            RichText::new(text)
                                .small()
                                .color(if *err { ERR } else { OK }),
                        );
                    }
                    ui.add_space(6.0);
                    let needle = self.listen.filter.trim().to_lowercase();
                    let shown: Vec<usize> = (0..lib.albums.len())
                        .filter(|&i| needle.is_empty() || album_matches(&lib.albums[i], &needle))
                        .collect();
                    if shown.is_empty() && !needle.is_empty() {
                        ui.label(
                            RichText::new(tr!(
                                format!(
                                    "Nessun artista, album o brano con «{}».",
                                    self.listen.filter.trim()
                                ),
                                format!(
                                    "No artist, album or track matching “{}”.",
                                    self.listen.filter.trim()
                                )
                            ))
                            .color(ui.visuals().weak_text_color()),
                        );
                    }
                    // gli album sono già in ordine di artista: un gruppo per artista
                    let mut k = 0;
                    while k < shown.len() {
                        let first = &lib.albums[shown[k]];
                        let key = (first.loose, first.group.to_lowercase());
                        let mut end = k;
                        while end < shown.len() {
                            let a = &lib.albums[shown[end]];
                            if (a.loose, a.group.to_lowercase()) != key {
                                break;
                            }
                            end += 1;
                        }
                        let group = &shown[k..end];
                        let title = if first.loose {
                            tr!("Brani sciolti", "Loose tracks").to_string()
                        } else if first.group.is_empty() {
                            tr!("Artista sconosciuto", "Unknown artist").to_string()
                        } else {
                            first.group.clone()
                        };
                        let count = match group.len() {
                            1 => tr!("1 uscita", "1 release").to_string(),
                            n => tr!(format!("{n} uscite"), format!("{n} releases")),
                        };
                        let header = egui::CollapsingHeader::new(
                            RichText::new(format!("{title}  ·  {count}"))
                                .strong()
                                .size(16.0),
                        )
                        .id_salt(("artist", &key.1, key.0))
                        .default_open(true);
                        // mentre si cerca, i gruppi trovati restano aperti
                        let header = if needle.is_empty() {
                            header
                        } else {
                            header.open(Some(true))
                        };
                        header.show(ui, |ui| {
                            let n: usize = group.iter().map(|&i| lib.albums[i].tracks.len()).sum();
                            if group.len() > 1
                                && ui
                                    .small_button(tr!(
                                        format!("Ascolta tutto ({n} brani)"),
                                        format!("Play all ({n} tracks)")
                                    ))
                                    .on_hover_text(tr!(
                                        format!("Ascolta di fila tutte le uscite di {title}"),
                                        format!("Play all of {title}'s releases in a row")
                                    ))
                                    .clicked()
                            {
                                play_group = Some(group.to_vec());
                            }
                            for &i in group {
                                let a = &lib.albums[i];
                                let cover = self.listen.covers.get(i).and_then(|c| c.as_ref());
                                let playing = self.listen.now.is_some_and(|(na, _)| na == i);
                                if album_row(ui, a, i, cover, playing) {
                                    open = Some(Some(i));
                                }
                                ui.add_space(4.0);
                            }
                        });
                        ui.add_space(2.0);
                        k = end;
                    }
                }
                Some(ai) => {
                    // un album: copertina, dati e brani
                    let a = &lib.albums[ai];
                    if ui.button(tr!("Tutti gli album", "All albums")).clicked() {
                        open = Some(None);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        cover_box(
                            ui,
                            self.listen.covers.get(ai).and_then(|c| c.as_ref()),
                            COVER_BIG,
                        );
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&a.name).size(20.0).strong()).wrap(),
                            );
                            if !a.artist.is_empty() {
                                ui.label(&a.artist);
                            }
                            ui.label(
                                RichText::new(tracks_label(a.tracks.len()))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.add_space(4.0);
                            let btn = egui::Button::new(
                                RichText::new(tr!("Ascolta l'album", "Play the album"))
                                    .strong()
                                    .color(ON_ACCENT),
                            )
                            .fill(ACCENT);
                            if ui.add(btn).clicked() {
                                play = Some((ai, 0));
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .small_button(reveal_label())
                                    .on_hover_text(a.dir.display().to_string())
                                    .clicked()
                                {
                                    reveal(&a.dir);
                                }
                                if cfg!(target_os = "macos")
                                    && ui
                                        .small_button(music_app_label())
                                        .on_hover_text(tr!(
                                            "Aggiunge l'album alla libreria dell'app Musica (iTunes sui Mac meno recenti)",
                                            "Adds the album to the Music app library (iTunes on older Macs)"
                                        ))
                                        .clicked()
                                {
                                    add_music = Some(a.dir.clone());
                                }
                                // i brani sciolti stanno nella cartella della musica: mai nel Cestino
                                if !a.loose
                                    && ui
                                        .small_button(tr!("Sposta nel Cestino…", "Move to Trash…"))
                                        .clicked()
                                {
                                    confirm = Some(ai);
                                }
                            });
                        });
                    });
                    if let Some((text, err)) = &self.listen.notice {
                        ui.label(
                            RichText::new(text)
                                .small()
                                .color(if *err { ERR } else { OK }),
                        );
                    }
                    ui.add_space(6.0);
                    for (ti, t) in a.tracks.iter().enumerate() {
                        let current = self.listen.now == Some((ai, ti));
                        let color = if current {
                            ACCENT_LIGHT
                        } else {
                            ui.visuals().text_color()
                        };
                        let row = ui
                            .horizontal(|ui| {
                                ui.add_sized(
                                    [28.0, 20.0],
                                    egui::Label::new(
                                        RichText::new(
                                            t.number.map(|n| n.to_string()).unwrap_or_default(),
                                        )
                                        .color(ui.visuals().weak_text_color()),
                                    ),
                                );
                                let dur = t.duration.map(player::fmt_time).unwrap_or_default();
                                let w = (ui.available_width() - 60.0).max(60.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(w, 20.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width(w);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&t.title).color(color).strong(),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                                ui.label(RichText::new(dur).color(ui.visuals().weak_text_color()));
                            })
                            .response;
                        let resp = ui
                            .interact(
                                row.rect,
                                ui.scope_id().with(("track", ai, ti)),
                                egui::Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                tr!(format!("Ascolta {}", t.title), format!("Play {}", t.title)),
                            )
                        });
                        if resp.hovered() || current {
                            ui.painter().rect_stroke(
                                row.rect.expand(2.0),
                                4,
                                egui::Stroke::new(1.0, if current { ACCENT } else { ACCENT_LIGHT }),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if resp.clicked() {
                            play = Some((ai, ti));
                        }
                    }
                }
            }
        });
        if rescan {
            self.start_scan(ctx);
        }
        if let Some(o) = open {
            self.listen.open = o;
            self.listen.notice = None;
        }
        if let Some((a, t)) = play {
            self.play_album_from(a, t);
            ctx.request_repaint();
        }
        if let Some(albums) = play_group {
            self.play_albums(&albums);
            ctx.request_repaint();
        }
        if let Some(dir) = add_music {
            self.add_to_music(ctx, dir);
        }
        if toggle_norm {
            self.cfg.normalize = !self.cfg.normalize;
            self.cfg.save();
            self.apply_gain();
            // accesa ora: si misurano i brani che mancano
            if self.cfg.normalize {
                if let Some(lib) = &self.listen.library {
                    for t in lib.albums.iter().flat_map(|a| &a.tracks) {
                        if self.gains.get(&t.path).is_none() {
                            self.analyzer.later(&t.path);
                        }
                    }
                }
            }
        }
        if confirm.is_some() {
            self.listen.confirm_trash = confirm;
        }
        self.trash_dialog(ctx);
    }

    /// «Scegli i brani…»: apre o chiude l'elenco; la prima volta lo chiede a yt-dlp.
    fn toggle_pick(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(url) = self.search.results.get(idx).map(|r| r.url.clone()) else {
            return;
        };
        let pick = self.search.picks.entry(idx).or_default();
        pick.open = !pick.open;
        if !pick.open || pick.loading || !pick.tracks.is_empty() {
            return;
        }
        pick.loading = true;
        pick.error = false;
        let (cfg, tools, seq) = (self.cfg.clone(), self.toolset(), self.search.seq);
        let cancel = self.search.cancel.clone();
        let send = self.sender(ctx);
        thread::spawn(move || {
            let tracks = downloader::list_tracks(&url, &cfg, &tools, &cancel).map(|(_, t)| t);
            send(Msg::Tracks { seq, idx, tracks });
        });
    }

    /// Conferma prima di spostare un album nel Cestino (si può ripristinare dal Cestino).
    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(ai) = self.listen.confirm_trash else {
            return;
        };
        let Some((name, dir)) = self
            .listen
            .library
            .as_ref()
            .and_then(|l| l.albums.get(ai))
            .filter(|a| !a.loose)
            .map(|a| (a.name.clone(), a.dir.clone()))
        else {
            self.listen.confirm_trash = None;
            return;
        };
        let mut go = false;
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::unique("trash_album")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(
                RichText::new(tr!(
                    format!("Spostare «{name}» nel Cestino?"),
                    format!("Move “{name}” to the Trash?")
                ))
                .strong(),
            );
            ui.label(
                RichText::new(tr!(
                    "La cartella dell'album va nel Cestino, con tutti i suoi brani. Se cambi idea, la ripristini dal Cestino.",
                    "The album folder goes to the Trash with all its tracks. If you change your mind, restore it from the Trash."
                ))
                .small(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let btn = egui::Button::new(
                    RichText::new(tr!("Sposta nel Cestino", "Move to Trash")).color(Color32::WHITE),
                )
                .fill(ERR);
                if ui.add(btn).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                if ui.button(tr!("Annulla", "Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() || close {
            self.listen.confirm_trash = None;
        }
        if go {
            self.listen.confirm_trash = None;
            // se suona un brano di questo album, si ferma prima di spostarlo
            if self.now_track().is_some_and(|t| t.path.starts_with(&dir)) {
                self.engine.stop();
                self.listen.now = None;
                self.cfg.last_track = None;
                self.cfg.save();
            }
            let send = self.sender(ctx);
            thread::spawn(move || {
                let r = trash::delete(&dir).map(|_| dir).map_err(|e| e.to_string());
                send(Msg::Trashed(r));
            });
        }
    }

    /// «Aggiungi a Musica»: l'app Musica (o iTunes) importa la cartella dell'album.
    fn add_to_music(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.listen.notice = Some((
            tr!("Aggiungo l'album a Musica…", "Adding the album to Music…").into(),
            false,
        ));
        let send = self.sender(ctx);
        thread::spawn(move || {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let script = format!(
                "tell application {} to add (POSIX file {})",
                applescript_quote(music_app()),
                applescript_quote(&dir.to_string_lossy())
            );
            let r = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| {
                    if o.status.success() {
                        Ok(name)
                    } else {
                        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                    }
                });
            send(Msg::MusicAdded(r));
        });
    }

    /// Barra in basso con il brano in ascolto: comandi, posizione e volume.
    fn now_playing_bar(&mut self, ui: &mut egui::Ui) {
        let Some((ai, _)) = self.listen.now else {
            return;
        };
        let Some(track) = self.now_track().cloned() else {
            return;
        };
        let album = self
            .listen
            .library
            .as_ref()
            .and_then(|l| l.albums.get(ai))
            .map(|a| (a.name.clone(), a.artist.clone()))
            .unwrap_or_default();
        let st = self.engine.status();
        let mut action: Option<Icon> = None;
        let mut toggle_shuffle = false;
        let mut cycle_repeat = false;
        egui::Panel::bottom("now_playing")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 28, 31))
                    .inner_margin(10)
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(52, 52, 57))),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    cover_box(
                        ui,
                        self.listen.covers.get(ai).and_then(|c| c.as_ref()),
                        48.0,
                    );
                    for (icon, label, big) in [
                        (Icon::Prev, tr!("Brano precedente", "Previous track"), false),
                        (
                            if st.loaded && !st.paused {
                                Icon::Pause
                            } else {
                                Icon::Play
                            },
                            if st.loaded && !st.paused {
                                tr!("Pausa", "Pause")
                            } else {
                                tr!("Riproduci", "Play")
                            },
                            true,
                        ),
                        (Icon::Next, tr!("Brano successivo", "Next track"), false),
                    ] {
                        if icon_button(ui, icon, label, big).clicked() {
                            action = Some(icon);
                        }
                    }
                    ui.vertical(|ui| {
                        let shuffle = ui
                            .selectable_label(
                                self.cfg.shuffle,
                                on_off_text(tr!("Casuale", "Shuffle"), self.cfg.shuffle, ui),
                            )
                            .on_hover_text(tr!(
                                "Ascolta i brani in ordine casuale",
                                "Play the tracks in random order"
                            ));
                        if shuffle.clicked() {
                            toggle_shuffle = true;
                        }
                        let (text, hover) = match self.cfg.repeat {
                            Repeat::Off => (
                                tr!("Ripeti", "Repeat"),
                                tr!(
                                    "Non ripete: si ferma alla fine",
                                    "No repeat: stops at the end"
                                ),
                            ),
                            Repeat::All => (
                                tr!("Ripeti tutto", "Repeat all"),
                                tr!(
                                    "Ricomincia l'elenco quando finisce",
                                    "Starts the list again at the end"
                                ),
                            ),
                            Repeat::One => (
                                tr!("Ripeti brano", "Repeat track"),
                                tr!("Ripete questo brano", "Repeats this track"),
                            ),
                        };
                        let repeat = ui
                            .selectable_label(
                                self.cfg.repeat != Repeat::Off,
                                on_off_text(text, self.cfg.repeat != Repeat::Off, ui),
                            )
                            .on_hover_text(hover);
                        if repeat.clicked() {
                            cycle_repeat = true;
                        }
                    });
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 150.0).max(80.0));
                        ui.add(egui::Label::new(RichText::new(&track.title).strong()).truncate());
                        let who = if track.artist.is_empty() {
                            album.1.clone()
                        } else {
                            track.artist.clone()
                        };
                        let sub = [who, album.0.clone()]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.add(
                            egui::Label::new(
                                RichText::new(sub)
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .truncate(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 90.0;
                        // percentuale a destra del cursore, larga fissa: «100%» non sposta nulla
                        let pct = format!("{:.0}%", self.cfg.volume.clamp(0.0, 1.0) * 100.0);
                        ui.add_sized(
                            [38.0, 18.0],
                            egui::Label::new(
                                RichText::new(pct)
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            ),
                        );
                        let mut vol = self.cfg.volume;
                        let resp = ui
                            .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                            .on_hover_text(tr!("Volume", "Volume"));
                        if resp.changed() {
                            self.cfg.volume = vol;
                            self.engine.set_volume(vol);
                        }
                        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                            self.cfg.save();
                        }
                        ui.label(RichText::new(tr!("Volume", "Volume")).small());
                    });
                });
                // posizione nel brano
                let dur = track.duration.unwrap_or_default().as_secs_f32();
                ui.horizontal(|ui| {
                    let pos = self
                        .listen
                        .seek_drag
                        .unwrap_or(st.pos.as_secs_f32())
                        .min(dur.max(0.0));
                    ui.label(RichText::new(player::fmt_time(Duration::from_secs_f32(pos))).small());
                    ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(80.0);
                    let mut v = pos;
                    let resp = ui.add_enabled(
                        dur > 0.0,
                        egui::Slider::new(&mut v, 0.0..=dur.max(1.0)).show_value(false),
                    );
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Slider,
                            dur > 0.0,
                            tr!("Posizione nel brano", "Position in the track"),
                        )
                    });
                    if resp.dragged() || resp.changed() {
                        self.listen.seek_drag = Some(v);
                    }
                    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                        self.engine.seek(Duration::from_secs_f32(v));
                        self.listen.seek_drag = None;
                    }
                    ui.label(RichText::new(player::fmt_time(Duration::from_secs_f32(dur))).small());
                });
                if let Some(e) = &st.error {
                    ui.label(RichText::new(e).small().color(ERR));
                }
            });
        if action.is_some() {
            ui.ctx().request_repaint();
        }
        match action {
            Some(Icon::Prev) => self.play_prev(),
            Some(Icon::Next) => self.play_next(false),
            Some(Icon::Play) | Some(Icon::Pause) => self.toggle_play(),
            None => {}
        }
        if toggle_shuffle {
            self.toggle_shuffle();
        }
        if cycle_repeat {
            self.cfg.repeat = match self.cfg.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            };
            self.cfg.save();
        }
    }
}

/// L'album corrisponde alla ricerca (testo già in minuscolo): artista, album o un brano.
fn album_matches(a: &player::Album, needle: &str) -> bool {
    a.group.to_lowercase().contains(needle)
        || a.artist.to_lowercase().contains(needle)
        || a.name.to_lowercase().contains(needle)
        || a.tracks
            .iter()
            .any(|t| t.title.to_lowercase().contains(needle))
}

/// Una riga dell'elenco degli album; `true` se è stata cliccata.
fn album_row(
    ui: &mut egui::Ui,
    a: &player::Album,
    i: usize,
    cover: Option<&egui::TextureHandle>,
    playing: bool,
) -> bool {
    let row = ui
        .horizontal(|ui| {
            cover_box(ui, cover, COVER_LIST);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(&a.name).strong()).truncate());
                let total: Duration = a.tracks.iter().filter_map(|t| t.duration).sum();
                let mut meta = Vec::new();
                if let Some(y) = a.year {
                    meta.push(y.to_string());
                }
                if a.is_single() && !a.loose {
                    meta.push(tr!("Singolo", "Single").to_string());
                }
                // l'artista solo se dice qualcosa in più del gruppo (duetti, ospiti)
                if !a.artist.is_empty() && a.artist != a.group {
                    meta.push(a.artist.clone());
                }
                meta.push(tracks_label(a.tracks.len()));
                if !total.is_zero() {
                    meta.push(player::fmt_time(total));
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(meta.join(" · "))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    )
                    .truncate(),
                );
                if playing {
                    ui.label(
                        RichText::new(tr!("in ascolto", "now playing"))
                            .small()
                            .color(ACCENT),
                    );
                }
            });
        })
        .response;
    let resp = ui
        .interact(
            row.rect,
            ui.scope_id().with(("album", i)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            format!("{} ({})", a.name, a.artist),
        )
    });
    if resp.hovered() {
        ui.painter().rect_stroke(
            row.rect.expand(3.0),
            6,
            egui::Stroke::new(1.0, ACCENT_LIGHT),
            egui::StrokeKind::Outside,
        );
    }
    resp.clicked()
}

/// «1 brano», «12 brani».
fn tracks_label(n: usize) -> String {
    match n {
        1 => tr!("1 brano", "1 track").into(),
        n => tr!(format!("{n} brani"), format!("{n} tracks")),
    }
}

/// Lato delle copertine nell'elenco degli album e nella pagina di un album.
const COVER_LIST: f32 = 56.0;
const COVER_BIG: f32 = 120.0;

// ───────────────────────────── utilità ─────────────────────────────

/// Riquadro della copertina nei risultati della ricerca (16:9, come le miniature YouTube).
const THUMB_SLOT: egui::Vec2 = egui::vec2(112.0, 63.0);

/// È un album (e non un singolo, un EP o una playlist)?
fn is_album(r: &SearchResult) -> bool {
    release_kind(r).eq_ignore_ascii_case("album")
}

/// «Album», «Singolo» o «Playlist» per un risultato che raccoglie più brani.
fn release_kind(r: &SearchResult) -> String {
    // come lo scrive YouTube Music, quando lo dice («Album», «Singolo», «EP»)
    if let Some(k) = &r.kind {
        return k.clone();
    }
    // gli album hanno indirizzi …/browse/MPREb_… (YouTube Music) o playlist OLAK5uy_…
    let album = r.url.contains("/browse/MPREb") || r.url.contains("list=OLAK5uy");
    match r.tracks {
        Some(1..=3) if album => tr!("Singolo", "Single"),
        _ if album => "Album",
        _ => "Playlist",
    }
    .into()
}

/// Dimensione di `size` rimpicciolita per stare dentro `slot` senza deformarsi.
fn fit(size: egui::Vec2, slot: egui::Vec2) -> egui::Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return slot;
    }
    let k = (slot.x / size.x).min(slot.y / size.y);
    size * k
}

fn label_of(list: &[(&str, &'static str)], key: &str) -> &'static str {
    list.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| loc(l))
        .unwrap_or("—")
}

/// Traduce le etichette fisse delle liste (scritte in italiano) nella lingua attiva.
fn loc(label: &'static str) -> &'static str {
    match label {
        "Migliore disponibile" => tr!("Migliore disponibile", "Best available"),
        "FLAC (senza perdita)" => tr!("FLAC (senza perdita)", "FLAC (lossless)"),
        "WAV (senza perdita)" => tr!("WAV (senza perdita)", "WAV (lossless)"),
        "Nessuno" => tr!("Nessuno", "None"),
        other => other,
    }
}

/// Testo del pulsante che mostra un file nella sua cartella.
fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        tr!("Mostra nel Finder", "Show in Finder")
    } else if cfg!(windows) {
        tr!("Mostra in Esplora file", "Show in File Explorer")
    } else {
        tr!("Apri la cartella", "Open the folder")
    }
}

/// Apre la cartella del file con il file selezionato (su Linux solo la cartella).
fn reveal(file: &std::path::Path) {
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(file)
            .spawn();
    } else if cfg!(windows) {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(file);
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    } else if let Some(dir) = file.parent() {
        open_folder(dir);
    }
}

/// L'app della musica del Mac: «Music» da Catalina (10.15), prima «iTunes».
fn music_app() -> &'static str {
    if std::path::Path::new("/System/Applications/Music.app").exists() {
        "Music"
    } else {
        "iTunes"
    }
}

fn music_app_label() -> &'static str {
    if music_app() == "Music" {
        tr!("Aggiungi a Musica", "Add to Music")
    } else {
        tr!("Aggiungi a iTunes", "Add to iTunes")
    }
}

/// Testo tra virgolette per AppleScript.
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Notifica di sistema (macOS e Linux; su Windows basta il pulsante che lampeggia).
fn notify(title: &str, body: &str) {
    if cfg!(target_os = "macos") {
        let script = format!(
            "display notification {} with title {}",
            applescript_quote(body),
            applescript_quote(title)
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn();
    } else if cfg!(target_os = "linux") {
        let _ = std::process::Command::new("notify-send")
            .arg(title)
            .arg(body)
            .spawn();
    }
}

/// Il link negli appunti, se è un solo link YouTube, YouTube Music o Spotify.
fn copied_link(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) || !t.starts_with("http") {
        return None;
    }
    let l = t.to_lowercase();
    let ok =
        l.contains("youtube.com/") || l.contains("youtu.be/") || l.contains("open.spotify.com/");
    ok.then(|| t.to_string())
}

/// Testo di un interruttore della barra del player: scuro sullo sfondo arancione quando è
/// attivo, altrimenti si leggerebbe arancione su arancione.
fn on_off_text(text: &str, on: bool, ui: &egui::Ui) -> RichText {
    RichText::new(text).small().color(if on {
        ON_ACCENT
    } else {
        ui.visuals().text_color()
    })
}

/// Elenco dei brani di un album da spuntare; `Some(posizioni)` quando si preme
/// «Scarica i brani scelti».
fn pick_list(ui: &mut egui::Ui, pick: &mut Pick, can_download: bool) -> Option<Vec<usize>> {
    ui.add_space(2.0);
    if pick.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(tr!(
                    "Leggo i brani dell'album…",
                    "Reading the album's tracks…"
                ))
                .small(),
            );
        });
        return None;
    }
    if pick.error {
        ui.label(
            RichText::new(tr!(
                "Non riesco a leggere i brani: chiudi e riprova.",
                "Could not read the tracks: close and try again."
            ))
            .small()
            .color(ERR),
        );
        return None;
    }
    ui.horizontal(|ui| {
        if ui.small_button(tr!("Tutti", "All")).clicked() {
            pick.tracks.iter_mut().for_each(|t| t.2 = true);
        }
        if ui.small_button(tr!("Nessuno", "None")).clicked() {
            pick.tracks.iter_mut().for_each(|t| t.2 = false);
        }
    });
    for (n, title, on) in pick.tracks.iter_mut() {
        ui.checkbox(on, RichText::new(format!("{n}. {title}")).small());
    }
    let chosen: Vec<usize> = pick.tracks.iter().filter(|t| t.2).map(|t| t.0).collect();
    let label = tr!(
        format!("Scarica i brani scelti ({})", chosen.len()),
        format!("Download the chosen tracks ({})", chosen.len())
    );
    let go = ui
        .add_enabled(can_download && !chosen.is_empty(), egui::Button::new(label))
        .clicked();
    go.then_some(chosen)
}

/// Mescola un elenco (Fisher-Yates), con un generatore semplice seminato dall'orologio:
/// per l'ordine casuale dei brani basta.
fn shuffle<T>(v: &mut [T]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..v.len()).rev() {
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

fn open_folder(p: &std::path::Path) {
    let cmd = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .expect("icona incorporata non valida")
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    ctx.load_texture("logo", color, egui::TextureOptions::LINEAR)
}

fn configure_style(ctx: &egui::Context) {
    // Interfaccia sempre scura, anche se il sistema è in modalità chiara.
    // La barra del titolo, invece, la lascia gestire al sistema: forzarne il
    // tema da qui la rendeva chiara senza pulsanti su Windows al primo avvio
    // e non seguiva il tema di sistema su Linux.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.options_mut(|o| o.sync_window_theme = false);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);

        // corpi del testo più grandi (di serie sono 12.5 / 9 / 12.5)
        use egui::{FontFamily, FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(15.5, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(15.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(14.0, FontFamily::Monospace),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(24.0, FontFamily::Proportional),
        );
        style.spacing.interact_size.y = 28.0; // bersagli più comodi da cliccare

        let v = &mut style.visuals;
        v.dark_mode = true;
        // testo: chiaro (14:1 sullo sfondo), secondario ben leggibile (8:1)
        v.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(226, 226, 232));
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(232, 232, 238));
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, Color32::WHITE);
        v.weak_text_color = Some(Color32::from_rgb(172, 172, 182));
        v.panel_fill = Color32::from_rgb(20, 20, 22);
        v.window_fill = Color32::from_rgb(26, 26, 29);
        v.extreme_bg_color = Color32::from_rgb(12, 12, 14); // sfondo dei campi di testo
        v.faint_bg_color = Color32::from_rgb(28, 28, 31);
        v.override_text_color = None;

        // accento arancione: selezione, barra di avanzamento, link, bordo del campo con focus
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = egui::Stroke::new(1.0, ACCENT_LIGHT);
        v.hyperlink_color = ACCENT_LIGHT;

        v.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 28, 31);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(52, 52, 57));
        v.widgets.inactive.bg_fill = Color32::from_rgb(44, 44, 49);
        // contorno ben visibile su caselle, menu e pulsanti (prima si confondevano con lo sfondo)
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(104, 104, 114));
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 44, 49);
        v.widgets.hovered.bg_fill = Color32::from_rgb(58, 52, 48);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 52, 48);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        v.widgets.active.bg_fill = ACCENT;
        v.widgets.active.weak_bg_fill = ACCENT;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_LIGHT);
        v.widgets.open.bg_fill = Color32::from_rgb(58, 52, 48);
        v.widgets.open.weak_bg_fill = Color32::from_rgb(58, 52, 48);

        let r = egui::CornerRadius::same(8);
        v.widgets.noninteractive.corner_radius = r;
        v.widgets.inactive.corner_radius = r;
        v.widgets.hovered.corner_radius = r;
        v.widgets.active.corner_radius = r;
        v.widgets.open.corner_radius = r;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_links_are_recognised() {
        let yt = "https://music.youtube.com/playlist?list=OLAK5uy_x";
        assert_eq!(copied_link(&format!("  {yt}\n")), Some(yt.to_string()));
        assert!(copied_link("https://youtu.be/abc").is_some());
        assert!(copied_link("https://open.spotify.com/album/1").is_some());
        // testo qualsiasi, altri siti o più link insieme: niente proposta
        assert_eq!(copied_link("ciao a tutti"), None);
        assert_eq!(copied_link("https://example.com/video"), None);
        assert_eq!(copied_link("https://youtu.be/a https://youtu.be/b"), None);
        assert_eq!(copied_link(""), None);
    }

    #[test]
    fn shuffle_keeps_every_track_once() {
        let mut v: Vec<u32> = (0..50).collect();
        shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(v, (0..50).collect::<Vec<_>>(), "50 brani rimasti in ordine");
        let mut one = vec![7];
        shuffle(&mut one);
        assert_eq!(one, [7]);
        shuffle::<u32>(&mut []);
    }

    #[test]
    fn notification_text_is_quoted_for_applescript() {
        assert_eq!(
            applescript_quote("Brano \"live\""),
            "\"Brano \\\"live\\\"\""
        );
        assert_eq!(applescript_quote("a\\b"), "\"a\\\\b\"");
    }
}
