// SPDX-License-Identifier: GPL-3.0-or-later
//! Interfaccia grafica (egui).
use crate::config::{Config, Mode};
use crate::downloader::{self, Emit, Event, Toolset};
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
        deno: Option<PathBuf>,
        warnings: Vec<String>,
    },
    ToolsFailed(String),
    UpdateCheck(Result<selfupdate::Check, String>),
    UpdateStatus(String),
    UpdateDone(Result<(), String>),
    Folder(PathBuf),
    Dl(Event),
    /// esito di una ricerca; `seq` scarta le risposte di ricerche ormai superate
    SearchDone {
        seq: u64,
        result: Result<Vec<SearchResult>, String>,
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
}

enum UpdateUi {
    None,
    Available(Release),
    Working(String),
    Ready(String),
    Failed(String),
}

#[derive(Default)]
struct SearchUi {
    query: String,
    /// testo cercato per ultimo (per il messaggio «nessun risultato»)
    last_query: String,
    results: Vec<SearchResult>,
    /// risultati spuntati per scaricarli insieme (stessi indici di `results`)
    selected: Vec<bool>,
    busy: bool,
    error: Option<String>,
    seq: u64,
    cancel: Arc<AtomicBool>,
    /// messaggio dopo «Aggiungi ai link»
    notice: Option<String>,
}

#[derive(Default)]
struct ToolsUi {
    ytdlp: Option<String>,
    ffmpeg: Option<PathBuf>,
    deno: Option<PathBuf>,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let logo = load_logo(&cc.egui_ctx);
        let (tx, rx) = channel();
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
        };
        cc.egui_ctx
            .set_zoom_factor(app.cfg.ui_scale.clamp(0.8, 2.0));
        app.start_setup(&cc.egui_ctx);
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
            let deno = tools::ensure_deno(&mut status)
                .map_err(|e| warnings.push(tr!(format!("Deno non disponibile ({e:#}): alcuni video YouTube potrebbero non funzionare."), format!("Deno is not available ({e:#}): some YouTube videos may not work."))))
                .ok();
            let ffmpeg = tools::ensure_ffmpeg(&mut status)
                .map_err(|e| warnings.push(tr!(format!("ffmpeg non disponibile ({e:#}): serve per la conversione audio e l'unione video/audio. Installalo e riavvia."), format!("ffmpeg is not available ({e:#}): it is needed for audio conversion and merging video/audio. Install it and restart."))))
                .ok();
            send(Msg::ToolsReady {
                ytdlp,
                ffmpeg,
                deno,
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
        self.cancel.store(false, Ordering::Relaxed);

        let toolset = self.toolset();
        let send = self.sender(ctx);
        let emit: Emit = Arc::new(move |e| send(Msg::Dl(e)));
        let (cfg, cancel, base) = (self.cfg.clone(), self.cancel.clone(), self.next_id);
        thread::spawn(move || downloader::run(inputs, cfg, toolset, base, emit, cancel));
    }

    fn toolset(&self) -> Toolset {
        Toolset {
            ytdlp: tools::ytdlp_path(),
            ffmpeg_dir: self.tools.ffmpeg.clone(),
            deno: self.tools.deno.clone(),
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
        let seq = self.search.seq;
        let toolset = self.toolset();
        let cookies = self.cfg.cookies_browser.clone();
        let send = self.sender(ctx);
        thread::spawn(move || {
            let result =
                search::search(&query, &toolset, &cookies, &cancel).map_err(|e| format!("{e:#}"));
            send(Msg::SearchDone { seq, result });
        });
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
                deno,
                warnings,
            } => {
                self.tools.busy = false;
                self.tools.ytdlp = Some(ytdlp);
                self.tools.ffmpeg = ffmpeg;
                self.tools.deno = deno;
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
                    Err(e) => tr!(format!("Controllo aggiornamenti non riuscito: {e}"), format!("Update check failed: {e}")),
                    Ok(Check::NotConfigured) => tr!("Aggiornamenti non attivi: repository GitHub non impostato (campo repository di Cargo.toml)", "Updates are off: GitHub repository not set (repository field in Cargo.toml)").into(),
                    Ok(Check::NoRelease) => {
                        let repo = selfupdate::repository().unwrap_or_default();
                        tr!(
                            format!("Nessuna release trovata su GitHub ({repo}). Se il repository è privato, l'app non può leggerlo."),
                            format!("No release found on GitHub ({repo}). If the repository is private, the app cannot read it.")
                        )
                    }
                    Ok(Check::UpToDate) => tr!(format!("Grabbit {} è aggiornato", selfupdate::CURRENT), format!("Grabbit {} is up to date", selfupdate::CURRENT)),
                    Ok(Check::Newer(rel)) => {
                        let msg = tr!(format!("Disponibile la versione {}", rel.version), format!("Version {} is available", rel.version));
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
            Msg::UpdateDone(Err(e)) => self.update = UpdateUi::Failed(e),
            Msg::Folder(p) => self.cfg.out_dir = p,
            Msg::SearchDone { seq, result } => {
                if seq != self.search.seq {
                    return; // risposta di una ricerca superata o annullata
                }
                self.search.busy = false;
                match result {
                    Ok(r) => {
                        self.search.selected = vec![false; r.len()];
                        self.search.results = r;
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
                    });
                }
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
                Event::JobFinished { id, ok, detail } => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                        j.state = if ok { JobState::Done } else { JobState::Failed };
                        j.pct = if ok { 1.0 } else { j.pct };
                        j.detail = detail;
                    }
                }
                Event::Finished => self.running = false,
            },
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m, ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    self.header(ui);
                    ui.add_space(8.0);
                    self.update_banner(ui, ctx);
                    self.tools_banner(ui, ctx);
                    ui.add_space(4.0);
                    self.search_card(ui, ctx);
                    ui.add_space(8.0);
                    self.link_card(ui);
                    ui.add_space(8.0);
                    self.options_card(ui, ctx);
                    ui.add_space(10.0);
                    self.action_row(ui, ctx);
                    ui.add_space(10.0);
                    self.queue(ui);
                    self.log_view(ui);
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
                        "Video e musica da YouTube e Spotify",
                        "Video and music from YouTube and Spotify"
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
                            RichText::new(tr!(
                                "Per aggiornarsi da sola, l'app va avviata dal file AppImage.",
                                "To update itself, the app must be run from the AppImage file."
                            ))
                            .small()
                            .color(ui.visuals().weak_text_color()),
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
        let mut clear = false;

        card(ui, |ui| {
            let title_id = ui.label(RichText::new(tr!("Cerca", "Search")).strong()).id;
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
                let resp = resp.labelled_by(title_id);
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
            ui.label(
                RichText::new(tr!(
                    format!(
                        "{} risultati per «{}» · «Scarica» li salva {what}, con le opzioni qui sotto.",
                        res.len(),
                        self.search.last_query
                    ),
                    format!(
                        "{} results for “{}” · “Download” saves them {what}, with the options below.",
                        res.len(),
                        self.search.last_query
                    )
                ))
                .small()
                .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .id_salt("search_results")
                .max_height(330.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, r) in self.search.results.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
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
                            let dl = ui
                                .add_enabled(
                                    can_download,
                                    egui::Button::new(tr!("Scarica", "Download")).small(),
                                )
                                .on_hover_text(tr!(
                                    "Scarica subito questo risultato",
                                    "Download this result now"
                                ));
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
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&r.title).strong()).truncate(),
                                )
                                .on_hover_text(&r.title);
                                let mut meta: Vec<String> = Vec::new();
                                if !r.channel.is_empty() {
                                    meta.push(r.channel.clone());
                                }
                                if let Some(d) = r.duration {
                                    meta.push(search::fmt_duration(d));
                                }
                                if let Some(v) = r.views {
                                    meta.push(search::fmt_views(v));
                                }
                                ui.horizontal(|ui| {
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
                                    ui.hyperlink_to(
                                        RichText::new(tr!("apri", "open")).small(),
                                        &r.url,
                                    );
                                });
                            });
                        });
                    }
                });

            let n_sel = self.search.selected.iter().filter(|s| **s).count();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let picked = || {
                    self.search
                        .results
                        .iter()
                        .zip(&self.search.selected)
                        .filter(|(_, s)| **s)
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
        if clear {
            self.cancel_search();
            self.search.results.clear();
            self.search.selected.clear();
            self.search.last_query.clear();
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

    fn action_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let has_input = self.input.lines().any(|l| !l.trim().is_empty());
        let ready = self.tools.ytdlp.is_some() && !self.tools.busy;
        ui.horizontal(|ui| {
            if self.running {
                let btn = egui::Button::new(RichText::new("   Annulla   ").size(18.0))
                    .min_size(egui::vec2(0.0, 38.0));
                if ui.add(btn).clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                ui.spinner();
            } else {
                let enabled = ready && has_input;
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
                // niente add_enabled: egui sfumerebbe ancora il testo; basta non rendere cliccabile il pulsante
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
                if !self.jobs.is_empty() && ui.button(tr!("Pulisci elenco", "Clear list")).clicked()
                {
                    self.jobs.clear();
                }
            }
        });
    }

    fn queue(&self, ui: &mut egui::Ui) {
        if self.jobs.is_empty() {
            return;
        }
        card(ui, |ui| {
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
                if !j.detail.is_empty() {
                    let col = if j.state == JobState::Failed {
                        ERR
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.label(RichText::new(&j.detail).small().color(col));
                }
            }
        });
    }

    fn log_view(&self, ui: &mut egui::Ui) {
        if self.log.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.collapsing(tr!("Registro dettagliato", "Detailed log"), |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for l in &self.log {
                        ui.label(RichText::new(l).monospace().small());
                    }
                });
        });
    }

    fn footer(&self, ui: &mut egui::Ui) {
        ui.separator();
        let yt = self.tools.ytdlp.as_deref().unwrap_or("…");
        let ok = |b: bool| if b { "ok" } else { tr!("manca", "missing") };
        ui.label(
            RichText::new(format!(
                "Grabbit {} · yt-dlp {} · ffmpeg {} · Deno {}",
                selfupdate::CURRENT,
                yt,
                ok(self.tools.ffmpeg.is_some()),
                ok(self.tools.deno.is_some()),
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

// ───────────────────────────── utilità ─────────────────────────────

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
