// SPDX-License-Identifier: GPL-3.0-or-later
//! Riquadro «Torrent» della scheda «File»: i torrent di Transmission, comandati da Grabbit.
use super::{card, open_folder, progress_row, reveal, App, ACCENT, ERR, OK, WARN};
use crate::dm;
use crate::torrent::{self, Cmd, Status, Torrent, Update};
use crate::tr;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};

/// Ultimo elenco letto da Transmission e ultimo esito da mostrare.
#[derive(Default)]
pub struct TorrentUi {
    pub list: Option<Result<Vec<Torrent>, String>>,
    /// testo ed esito (true = andato bene)
    pub notice: Option<(String, bool)>,
}

fn status_text(t: &Torrent) -> &'static str {
    if t.fetching_metadata && t.status != Status::Stopped {
        return tr!("Cerca i dati del magnet…", "Fetching magnet data…");
    }
    match t.status {
        Status::Stopped if t.progress >= 1.0 => tr!("Completato", "Completed"),
        Status::Stopped => tr!("In pausa", "Paused"),
        Status::Checking => tr!("Verifica", "Checking"),
        Status::Queued => tr!("In coda", "Queued"),
        Status::Downloading => tr!("In corso", "Downloading"),
        Status::Seeding => tr!("Completato, in condivisione", "Completed, seeding"),
    }
}

fn status_color(t: &Torrent, weak: Color32) -> Color32 {
    if t.error.is_some() {
        return ERR;
    }
    match t.status {
        Status::Downloading => ACCENT,
        Status::Seeding => OK,
        Status::Stopped if t.progress >= 1.0 => OK,
        _ => weak,
    }
}

impl App {
    /// Manda magnet, indirizzi .torrent o file .torrent a Transmission (o all'app predefinita).
    pub(super) fn add_torrents(&mut self, sources: Vec<String>) {
        if sources.is_empty() {
            return;
        }
        self.torrents.notice = Some((
            tr!("Invio a Transmission…", "Sending to Transmission…").into(),
            true,
        ));
        let _ = self
            .torrent_tx
            .send(Cmd::Add(sources, self.cfg.dm_dir.clone()));
    }

    pub(super) fn torrent_update(&mut self, u: Update) {
        match u {
            Update::List(l) => self.torrents.list = Some(l),
            Update::Added {
                added,
                opened,
                errors,
            } => {
                let mut parts = Vec::new();
                if !added.is_empty() {
                    let names = added
                        .iter()
                        .map(|n| if n.is_empty() { "magnet" } else { n.as_str() })
                        .collect::<Vec<_>>()
                        .join(", ");
                    parts.push(tr!(
                        format!("Aggiunto a Transmission: {names}"),
                        format!("Added to Transmission: {names}")
                    ));
                }
                if opened > 0 {
                    parts.push(tr!(
                        format!("Transmission non risponde: {opened} aperti con l'app predefinita per i torrent"),
                        format!("Transmission does not respond: {opened} opened with the default torrent app")
                    ));
                }
                let ok = errors.is_empty();
                parts.extend(errors);
                self.torrents.notice = Some((parts.join(" · "), ok));
            }
            Update::Error(e) => self.torrents.notice = Some((e, false)),
        }
    }

    pub(super) fn torrent_card(&mut self, ui: &mut egui::Ui) {
        let weak = ui.visuals().weak_text_color();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr!("Torrent", "Torrents")).strong());
                ui.label(
                    RichText::new(tr!("tramite Transmission", "via Transmission"))
                        .small()
                        .color(weak),
                );
            });
            if let Some((text, ok)) = &self.torrents.notice {
                ui.label(
                    RichText::new(text)
                        .small()
                        .color(if *ok { OK } else { ERR }),
                );
            }
            let mut actions: Vec<Cmd> = Vec::new();
            match &self.torrents.list {
                None => {
                    ui.label(
                        RichText::new(tr!(
                            "Collegamento a Transmission…",
                            "Connecting to Transmission…"
                        ))
                        .color(weak),
                    );
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).small().color(WARN));
                    if cfg!(target_os = "macos")
                        && ui
                            .button(tr!("Apri Transmission", "Open Transmission"))
                            .clicked()
                    {
                        torrent::launch_transmission();
                        actions.push(Cmd::Refresh);
                    }
                    ui.label(
                        RichText::new(tr!(
                            "Senza Transmission i link magnet e i .torrent si aprono con l'app predefinita per i torrent.",
                            "Without Transmission, magnet links and .torrent files open with the default torrent app."
                        ))
                        .small()
                        .color(weak),
                    );
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(
                        RichText::new(tr!(
                            "Nessun torrent in Transmission. Incolla qui sopra un link magnet o un indirizzo .torrent, oppure premi «Scegli file .torrent…».",
                            "No torrents in Transmission. Paste a magnet link or a .torrent address above, or press “Choose .torrent files…”."
                        ))
                        .color(weak),
                    );
                }
                Some(Ok(list)) => {
                    for t in list {
                        ui.add_space(4.0);
                        torrent_row(ui, t, weak, &mut actions);
                    }
                }
            }
            for a in actions {
                let _ = self.torrent_tx.send(a);
            }
        });
    }
}

fn torrent_row(ui: &mut egui::Ui, t: &Torrent, weak: Color32, actions: &mut Vec<Cmd>) {
    egui::Frame::group(ui.style())
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let col = status_color(t, weak);
            let state = status_text(t);
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, col);
                let name = if t.name.is_empty() { "magnet" } else { &t.name };
                let r = ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{name}: {state}"))
                });
                ui.label(RichText::new(state).small().color(col));
            });
            if t.progress < 1.0 && t.total_bytes > 0 {
                progress_row(ui, t.progress);
            }
            let mut parts: Vec<String> = Vec::new();
            if t.total_bytes > 0 {
                parts.push(if t.progress >= 1.0 {
                    dm::fmt_size(t.total_bytes)
                } else {
                    format!(
                        "{} / {}",
                        dm::fmt_size(t.done_bytes),
                        dm::fmt_size(t.total_bytes)
                    )
                });
            }
            if t.status == Status::Downloading {
                if t.down_rate > 0 {
                    parts.push(dm::fmt_speed(t.down_rate as f64));
                }
                if let Some(e) = t.eta {
                    parts.push(tr!(
                        format!("ancora {}", dm::fmt_hms(e)),
                        format!("{} left", dm::fmt_hms(e))
                    ));
                }
            }
            if t.up_rate > 0 && matches!(t.status, Status::Downloading | Status::Seeding) {
                let up = dm::fmt_speed(t.up_rate as f64);
                parts.push(tr!(format!("invia {up}"), format!("sending {up}")));
            }
            if matches!(t.status, Status::Downloading | Status::Seeding) {
                parts.push(tr!(
                    format!("{} peer", t.peers),
                    format!("{} peers", t.peers)
                ));
            }
            if !parts.is_empty() {
                ui.label(RichText::new(parts.join(" · ")).small().color(weak));
            }
            if let Some(e) = &t.error {
                ui.label(RichText::new(e).small().color(ERR));
            }
            ui.horizontal_wrapped(|ui| {
                if t.status == Status::Stopped {
                    if t.progress < 1.0 && ui.small_button(tr!("Riprendi", "Resume")).clicked() {
                        actions.push(Cmd::Resume(t.id));
                    }
                } else if ui.small_button(tr!("Pausa", "Pause")).clicked() {
                    actions.push(Cmd::Pause(t.id));
                }
                if !t.dir.is_empty()
                    && ui
                        .small_button(tr!("Apri cartella", "Open folder"))
                        .clicked()
                {
                    let item = Path::new(&t.dir).join(&t.name);
                    if !t.name.is_empty() && item.exists() {
                        reveal(&item);
                    } else {
                        open_folder(&PathBuf::from(&t.dir));
                    }
                }
                if ui
                    .small_button(tr!("Togli da Transmission", "Remove from Transmission"))
                    .on_hover_text(tr!(
                        "Il torrent esce da Transmission; i file già scaricati restano nella cartella.",
                        "The torrent leaves Transmission; files already downloaded stay in the folder."
                    ))
                    .clicked()
                {
                    actions.push(Cmd::Remove(t.id));
                }
            });
        });
}
