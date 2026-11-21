// SPDX-License-Identifier: GPL-3.0-or-later
// Adattato da Dropall 0.4.3 di Marco Giannini (https://github.com/marcogiannini/dropall),
// stessa licenza GPL-3.0-or-later.
//! Scheda "Download manager": link, code, pianificazione e limite di velocità.
use super::{
    accent_toggle, card, open_folder, progress_row, App, Msg, ACCENT, ERR, OK, ON_ACCENT, WARN,
};
use crate::dm::{self, Item, Queue, Schedule, State};
use crate::tr;
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::thread;

const CONNECTIONS: [u8; 5] = [1, 2, 4, 8, 16];
const SPEED_PRESETS: [u32; 4] = [256, 1024, 5 * 1024, 20 * 1024];

/// Stato della scheda che non va salvato.
pub struct DmUi {
    pub input: String,
    pub queue: u64,
    pub new_queue: String,
    pub rename: Option<(u64, String)>,
    /// ultimo valore del limite impostato (si ricorda quando lo si disattiva)
    pub limit_val: u32,
    pub confirm_delete_queue: bool,
}

impl DmUi {
    pub fn new(limit_kbps: u32) -> Self {
        Self {
            input: String::new(),
            queue: 0,
            new_queue: String::new(),
            rename: None,
            limit_val: if limit_kbps > 0 { limit_kbps } else { 1024 },
            confirm_delete_queue: false,
        }
    }
}

fn state_text(s: State) -> &'static str {
    match s {
        State::Queued => tr!("In coda", "Queued"),
        State::Downloading => tr!("In corso", "Downloading"),
        State::Paused => tr!("In pausa", "Paused"),
        State::Done => tr!("Completato", "Completed"),
        State::Failed => tr!("Non riuscito", "Failed"),
    }
}

fn state_color(s: State, weak: Color32) -> Color32 {
    match s {
        State::Queued => weak,
        State::Downloading => ACCENT,
        State::Paused => WARN,
        State::Done => OK,
        State::Failed => ERR,
    }
}

fn two_digits(v: f64, _: std::ops::RangeInclusive<usize>) -> String {
    format!("{:02}", v as u32)
}

impl App {
    pub(super) fn dm_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let snap = self.dm.snapshot();
        if !snap.queues.iter().any(|q| q.id == self.dm_ui.queue) {
            self.dm_ui.queue = snap.queues.first().map(|q| q.id).unwrap_or(1);
        }
        self.dm_add_card(ui, ctx, &snap.queues);
        ui.add_space(8.0);
        self.dm_speed_card(ui);
        ui.add_space(8.0);
        self.dm_queues(ui, &snap.queues, &snap.items);
        ui.add_space(8.0);
        self.torrent_card(ui);
    }

    fn dm_add_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, queues: &[Queue]) {
        let field_id = ui.scope_id().with("dm_field");
        card(ui, |ui| {
            let mut paste = false;
            let mut label_id = None;
            ui.horizontal(|ui| {
                label_id = Some(
                    ui.label(RichText::new(tr!("Indirizzi dei file", "File addresses")).strong())
                        .id,
                );
                paste = ui.small_button(tr!("Incolla", "Paste")).clicked();
                if ui.small_button(tr!("Svuota", "Clear")).clicked() {
                    self.dm_ui.input.clear();
                }
                if ui
                    .small_button(tr!("Scegli file .torrent…", "Choose .torrent files…"))
                    .clicked()
                {
                    let send = self.sender(ctx);
                    thread::spawn(move || {
                        if let Some(files) = rfd::FileDialog::new()
                            .set_title(tr!("Scegli i file .torrent", "Choose .torrent files"))
                            .add_filter("torrent", &["torrent"])
                            .pick_files()
                        {
                            send(Msg::TorrentFiles(files));
                        }
                    });
                }
            });
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.dm_ui.input)
                    .id(field_id)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text(tr!(
                        "Incolla qui uno o più indirizzi http(s) di file, o link magnet e .torrent, uno per riga",
                        "Paste one or more http(s) file addresses, or magnet and .torrent links, one per line"
                    )),
            );
            super::paste_fallback(ui.ctx(), &resp);
            if let Some(id) = label_id {
                resp.labelled_by(id);
            }
            if paste {
                ui.memory_mut(|m| m.request_focus(field_id));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            ui.add_space(6.0);

            egui::Grid::new("dm_opts")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label(tr!("Salva in", "Save to"));
                    ui.horizontal(|ui| {
                        if ui.button(tr!("Sfoglia…", "Browse…")).clicked() {
                            let send = self.sender(ctx);
                            let start = self.cfg.dm_dir.clone();
                            thread::spawn(move || {
                                if let Some(p) = rfd::FileDialog::new()
                                    .set_title(tr!(
                                        "Scegli la cartella di destinazione",
                                        "Choose the destination folder"
                                    ))
                                    .set_directory(start)
                                    .pick_folder()
                                {
                                    send(Msg::DmFolder(p));
                                }
                            });
                        }
                        ui.add(egui::Label::new(self.cfg.dm_dir.to_string_lossy()).truncate());
                    });
                    ui.end_row();

                    ui.label(tr!("Coda", "Queue"));
                    let current = queues
                        .iter()
                        .find(|q| q.id == self.dm_ui.queue)
                        .map(|q| q.name.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("dm_queue_pick")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for q in queues {
                                ui.selectable_value(&mut self.dm_ui.queue, q.id, &q.name);
                            }
                        });
                    ui.end_row();

                    ui.label(tr!("Connessioni", "Connections"));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("dm_conns")
                            .selected_text(self.cfg.dm_connections.to_string())
                            .show_ui(ui, |ui| {
                                for c in CONNECTIONS {
                                    ui.selectable_value(
                                        &mut self.cfg.dm_connections,
                                        c,
                                        c.to_string(),
                                    );
                                }
                            });
                        ui.label(
                            RichText::new(tr!(
                                "Più connessioni scaricano più in fretta se il server lo permette.",
                                "More connections download faster if the server allows it."
                            ))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                        );
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);

            let has_input = self.dm_ui.input.lines().any(|l| !l.trim().is_empty());
            ui.horizontal(|ui| {
                let (fill, text) = if has_input {
                    (ACCENT, ON_ACCENT)
                } else {
                    (
                        Color32::from_rgb(44, 44, 49),
                        Color32::from_rgb(130, 130, 138),
                    )
                };
                let sense = if has_input {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let start_label = tr!("Aggiungi e scarica", "Add and download");
                let b = egui::Button::new(RichText::new(start_label).strong().color(text))
                    .fill(fill)
                    .sense(sense)
                    .min_size(egui::vec2(0.0, 32.0));
                let r1 = ui.add(b);
                r1.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, has_input, start_label)
                });
                let add_label = tr!("Aggiungi alla coda", "Add to queue");
                let r2 = ui.add(
                    egui::Button::new(add_label)
                        .sense(sense)
                        .min_size(egui::vec2(0.0, 32.0)),
                );
                r2.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, has_input, add_label)
                });
                let go = r1.clicked();
                if go || r2.clicked() {
                    // magnet e .torrent vanno a Transmission
                    let (tor, plain): (Vec<&str>, Vec<&str>) = self
                        .dm_ui
                        .input
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .partition(|l| crate::torrent::is_torrent_source(l));
                    let tor: Vec<String> = tor.into_iter().map(String::from).collect();
                    let plain = plain.join("\n");
                    self.add_torrents(tor);
                    let n = self.dm.add(
                        &plain,
                        &self.cfg.dm_dir,
                        self.dm_ui.queue,
                        self.cfg.dm_connections,
                    );
                    if n > 0 || plain.trim().is_empty() {
                        self.dm_ui.input.clear();
                        if go && n > 0 {
                            self.dm.start_queue(self.dm_ui.queue);
                        }
                    }
                }
            });
            if !self.dm_ui.input.trim().is_empty()
                && !self.dm_ui.input.lines().any(|l| {
                    let t = l.trim();
                    t.starts_with("http://")
                        || t.starts_with("https://")
                        || crate::torrent::is_torrent_source(t)
                })
            {
                ui.label(
                    RichText::new(tr!(
                        "Gli indirizzi devono iniziare con http:// o https://",
                        "Addresses must start with http:// or https://"
                    ))
                    .small()
                    .color(WARN),
                );
            }
        });
    }

    fn dm_speed_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new(tr!("Limite di velocità", "Speed limit")).strong());
            let mut on = self.cfg.speed_limit_kbps > 0;
            let before = self.cfg.speed_limit_kbps;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(&mut on, tr!("Limita la velocità a", "Limit the speed to"))
                    .changed()
                {
                    self.cfg.speed_limit_kbps = if on { self.dm_ui.limit_val.max(1) } else { 0 };
                }
                ui.add_enabled_ui(on, |ui| {
                    let mut v = if on {
                        self.cfg.speed_limit_kbps
                    } else {
                        self.dm_ui.limit_val
                    };
                    let r = ui.add(
                        egui::DragValue::new(&mut v)
                            .range(1..=10_000_000)
                            .speed(16)
                            .suffix(" KB/s"),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::SpinButton,
                            true,
                            tr!(
                                "Limite di velocità in KB al secondo",
                                "Speed limit in KB per second"
                            ),
                        )
                    });
                    if on && v != self.cfg.speed_limit_kbps {
                        self.cfg.speed_limit_kbps = v;
                    }
                    for p in SPEED_PRESETS {
                        let t = if p >= 1024 {
                            format!("{} MB/s", p / 1024)
                        } else {
                            format!("{p} KB/s")
                        };
                        if ui.small_button(t).clicked() {
                            self.cfg.speed_limit_kbps = p;
                        }
                    }
                });
            });
            if self.cfg.speed_limit_kbps > 0 {
                self.dm_ui.limit_val = self.cfg.speed_limit_kbps;
            }
            if self.cfg.speed_limit_kbps != before {
                self.dm.set_speed_limit_kbps(self.cfg.speed_limit_kbps);
                let _ = self
                    .torrent_tx
                    .send(crate::torrent::Cmd::SpeedLimit(self.cfg.speed_limit_kbps));
            }
            ui.label(
                RichText::new(tr!(
                    "Vale per tutti i download di Grabbit, video, musica e torrent di Transmission compresi, così gli altri possono usare la rete. Per i video già avviati vale dal prossimo.",
                    "Applies to all Grabbit downloads, including video, music and Transmission torrents, so others can use the network. For videos already started it applies from the next one."
                ))
                .small()
                .color(ui.visuals().weak_text_color()),
            );
        });
    }

    fn dm_queues(&mut self, ui: &mut egui::Ui, queues: &[Queue], items: &[Item]) {
        // schede delle code
        ui.horizontal_wrapped(|ui| {
            for q in queues {
                let selected = q.id == self.dm_ui.queue;
                let n = items
                    .iter()
                    .filter(|i| i.queue == q.id && i.state != State::Done)
                    .count();
                let mut text = format!("{} ({n})", q.name);
                if q.running {
                    text.push_str(&format!(" · {}", tr!("attiva", "running")));
                }
                let rt = RichText::new(text).size(15.0).color(if selected {
                    ON_ACCENT
                } else {
                    ui.visuals().text_color()
                });
                if accent_toggle(ui, selected, rt).clicked() {
                    self.dm_ui.queue = q.id;
                    self.dm_ui.rename = None;
                    self.dm_ui.confirm_delete_queue = false;
                }
            }
            // nuova coda
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.dm_ui.new_queue)
                    .hint_text(tr!("Nome della nuova coda", "New queue name"))
                    .desired_width(150.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(tr!("Crea coda", "Create queue")).clicked() || enter {
                let name = std::mem::take(&mut self.dm_ui.new_queue);
                self.dm_ui.queue = self.dm.add_queue(&name);
                self.dm_ui.rename = None;
            }
        });
        ui.add_space(4.0);
        let Some(q) = queues.iter().find(|q| q.id == self.dm_ui.queue).cloned() else {
            return;
        };

        card(ui, |ui| {
            // comandi della coda
            ui.horizontal_wrapped(|ui| {
                if q.running {
                    let b = egui::Button::new(
                        RichText::new(tr!("Ferma la coda", "Stop the queue")).strong(),
                    )
                    .min_size(egui::vec2(0.0, 30.0));
                    if ui.add(b).clicked() {
                        self.dm.stop_queue(q.id);
                    }
                    ui.spinner();
                } else {
                    let b = egui::Button::new(
                        RichText::new(tr!("Avvia la coda", "Start the queue"))
                            .strong()
                            .color(ON_ACCENT),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(0.0, 30.0));
                    if ui.add(b).clicked() {
                        self.dm.start_queue(q.id);
                    }
                }
                if ui
                    .button(tr!("Pulisci i completati", "Clear completed"))
                    .clicked()
                {
                    self.dm.clear_finished();
                }
                if ui.button(tr!("Rinomina", "Rename")).clicked() {
                    self.dm_ui.rename = Some((q.id, q.name.clone()));
                }
                if queues.len() > 1 {
                    if self.dm_ui.confirm_delete_queue {
                        if ui
                            .button(
                                RichText::new(tr!(
                                    "Conferma: elimina la coda",
                                    "Confirm: delete the queue"
                                ))
                                .color(ERR),
                            )
                            .clicked()
                        {
                            self.dm.remove_queue(q.id);
                            self.dm_ui.confirm_delete_queue = false;
                        }
                    } else if ui
                        .button(tr!("Elimina la coda", "Delete the queue"))
                        .clicked()
                    {
                        self.dm_ui.confirm_delete_queue = true;
                    }
                }
            });
            if let Some((id, buf)) = &mut self.dm_ui.rename {
                if *id == q.id {
                    let mut done = false;
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(buf).desired_width(220.0));
                        r.request_focus();
                        if ui.button("OK").clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            done = true;
                        }
                    });
                    if done {
                        let name = buf.clone();
                        self.dm.rename_queue(q.id, &name);
                        self.dm_ui.rename = None;
                    }
                }
            }
            if q.running {
                ui.label(
                    RichText::new(tr!(
                        "I file vengono scaricati uno dopo l'altro, nell'ordine dell'elenco.",
                        "Files are downloaded one after the other, in list order."
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            }
            ui.add_space(4.0);
            self.dm_schedule(ui, &q);
        });
        ui.add_space(8.0);

        // elenco dei download della coda
        let mine: Vec<&Item> = items.iter().filter(|i| i.queue == q.id).collect();
        if mine.is_empty() {
            ui.label(
                RichText::new(tr!(
                    "Nessun download in questa coda. Incolla un indirizzo qui sopra e premi «Aggiungi e scarica».",
                    "No downloads in this queue. Paste an address above and press “Add and download”."
                ))
                .color(ui.visuals().weak_text_color()),
            );
            return;
        }
        let last = mine.len() - 1;
        for (n, it) in mine.iter().enumerate() {
            self.dm_row(ui, it, n == 0, n == last);
            ui.add_space(4.0);
        }
    }

    fn dm_schedule(&mut self, ui: &mut egui::Ui, q: &Queue) {
        let mut s: Schedule = q.schedule.clone();
        ui.collapsing(
            tr!("Pianificazione della coda", "Queue scheduler"),
            |ui| {
                ui.checkbox(
                    &mut s.enabled,
                    tr!(
                        "Avvia e ferma questa coda da sola",
                        "Start and stop this queue automatically"
                    ),
                );
                ui.add_enabled_ui(s.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr!("Avvia alle", "Start at"));
                        time_edit(ui, "start", &mut s.start_min);
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.stop_enabled, tr!("Ferma alle", "Stop at"));
                        ui.add_enabled_ui(s.stop_enabled, |ui| {
                            time_edit(ui, "stop", &mut s.stop_min);
                        });
                    });
                    ui.label(tr!("Nei giorni", "On days"));
                    ui.horizontal_wrapped(|ui| {
                        let names = [
                            tr!("Lun", "Mon"),
                            tr!("Mar", "Tue"),
                            tr!("Mer", "Wed"),
                            tr!("Gio", "Thu"),
                            tr!("Ven", "Fri"),
                            tr!("Sab", "Sat"),
                            tr!("Dom", "Sun"),
                        ];
                        for (i, name) in names.iter().enumerate() {
                            let on = s.days[i];
                            let rt = RichText::new(*name).color(if on {
                                ON_ACCENT
                            } else {
                                ui.visuals().text_color()
                            });
                            if accent_toggle(ui, on, rt).clicked() {
                                s.days[i] = !on;
                            }
                        }
                    });
                    if s.enabled && s.stop_enabled && s.stop_min <= s.start_min {
                        let msg = if s.stop_min == s.start_min {
                            tr!(
                                "Inizio e fine coincidono: la coda non partirà.",
                                "Start and stop are the same: the queue will not start."
                            )
                        } else {
                            tr!(
                                "L'orario di fine è prima di quello di inizio: la coda resta attiva oltre la mezzanotte, fino all'ora di fine del giorno dopo.",
                                "The stop time is before the start time: the queue stays active past midnight, until the stop time of the next day."
                            )
                        };
                        ui.label(RichText::new(msg).small().color(WARN));
                    }
                });
                ui.label(
                    RichText::new(tr!(
                        "Grabbit deve essere aperto all'orario previsto. Se all'apertura l'orario è già dentro l'intervallo, la coda parte subito.",
                        "Grabbit must be open at the scheduled time. If the time is already inside the window when the app opens, the queue starts right away."
                    ))
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
            },
        );
        if s != q.schedule {
            self.dm.set_schedule(q.id, s);
        }
    }

    fn dm_row(&mut self, ui: &mut egui::Ui, it: &Item, first: bool, last: bool) {
        let weak = ui.visuals().weak_text_color();
        egui::Frame::group(ui.style())
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let col = state_color(it.state, weak);
                let state = state_text(it.state);
                let r = ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 14.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, col);
                    let r =
                        ui.add(egui::Label::new(RichText::new(&it.file_name).strong()).truncate());
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Label,
                            true,
                            format!("{}: {state}", it.file_name),
                        )
                    });
                    ui.label(RichText::new(state).small().color(col));
                });
                let _ = r;

                // dettagli
                let size = match (it.total, it.state) {
                    (Some(t), State::Done) => dm::fmt_size(t),
                    (Some(t), _) => {
                        format!("{} / {}", dm::fmt_size(it.downloaded), dm::fmt_size(t))
                    }
                    (None, _) if it.downloaded > 0 => dm::fmt_size(it.downloaded),
                    _ => String::new(),
                };
                if matches!(it.state, State::Downloading | State::Paused | State::Queued)
                    && it.total.is_some()
                    && it.downloaded > 0
                {
                    progress_row(ui, it.fraction());
                }
                let mut parts: Vec<String> = Vec::new();
                if !size.is_empty() {
                    parts.push(size);
                }
                if it.state == State::Downloading && it.speed > 0.0 {
                    parts.push(dm::fmt_speed(it.speed));
                    if let Some(e) = it.eta_secs() {
                        parts.push(tr!(
                            format!("ancora {}", dm::fmt_hms(e)),
                            format!("{} left", dm::fmt_hms(e))
                        ));
                    }
                }
                if it.state == State::Downloading && it.total.is_some() && !it.resumable {
                    parts.push(
                        tr!(
                            "una sola connessione (il server non supporta le ripartenze)",
                            "single connection (the server does not support resuming)"
                        )
                        .into(),
                    );
                }
                if !parts.is_empty() {
                    ui.label(RichText::new(parts.join(" · ")).small().color(weak));
                }
                if it.state == State::Failed && !it.error.is_empty() {
                    ui.label(RichText::new(&it.error).small().color(ERR));
                }
                if it.state == State::Done {
                    if let Some(p) = &it.final_path {
                        ui.label(RichText::new(p.to_string_lossy()).small().color(weak));
                    }
                }

                ui.horizontal_wrapped(|ui| {
                    match it.state {
                        State::Downloading | State::Queued => {
                            if ui.small_button(tr!("Pausa", "Pause")).clicked() {
                                self.dm.pause(it.id);
                            }
                        }
                        State::Paused => {
                            if ui.small_button(tr!("Riprendi", "Resume")).clicked() {
                                self.dm.resume(it.id);
                            }
                        }
                        State::Failed => {
                            if ui.small_button(tr!("Riprova", "Retry")).clicked() {
                                self.dm.resume(it.id);
                            }
                        }
                        State::Done => {}
                    }
                    if it.state != State::Done {
                        ui.add_enabled_ui(!first, |ui| {
                            if ui.small_button(tr!("Su", "Up")).clicked() {
                                self.dm.move_item(it.id, true);
                            }
                        });
                        ui.add_enabled_ui(!last, |ui| {
                            if ui.small_button(tr!("Giù", "Down")).clicked() {
                                self.dm.move_item(it.id, false);
                            }
                        });
                    }
                    if ui
                        .small_button(tr!("Apri cartella", "Open folder"))
                        .clicked()
                    {
                        let dir: PathBuf = it
                            .final_path
                            .as_ref()
                            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                            .unwrap_or_else(|| it.dir.clone());
                        open_folder(&dir);
                    }
                    let label = if it.state == State::Done {
                        tr!("Rimuovi dall'elenco", "Remove from list")
                    } else {
                        tr!("Annulla e rimuovi", "Cancel and remove")
                    };
                    if ui.small_button(label).clicked() {
                        self.dm.remove(it.id, false);
                    }
                });
            });
    }
}

fn time_edit(ui: &mut egui::Ui, id: &str, minutes: &mut u16) {
    let mut h = (*minutes / 60) as u32;
    let mut m = (*minutes % 60) as u32;
    ui.push_id(id, |ui| {
        let rh = ui.add(
            egui::DragValue::new(&mut h)
                .range(0..=23)
                .custom_formatter(two_digits),
        );
        rh.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, tr!("ore", "hours"))
        });
        ui.label(":");
        let rm = ui.add(
            egui::DragValue::new(&mut m)
                .range(0..=59)
                .custom_formatter(two_digits),
        );
        rm.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, tr!("minuti", "minutes"))
        });
    });
    *minutes = (h * 60 + m) as u16;
}
