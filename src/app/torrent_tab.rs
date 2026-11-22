// SPDX-License-Identifier: GPL-3.0-or-later
//! Riquadro «Torrent» della scheda «File»: i torrent di Transmission e qBittorrent.
use super::{card, open_folder, progress_row, reveal, App, ACCENT, ERR, OK, WARN};
use crate::dm;
use crate::torrent::{self, Cmd, Status, Torrent, Update};
use crate::tr;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use torrent::App as TApp;

/// Ultimi elenchi letti dalle app e ultimo esito da mostrare.
#[derive(Default)]
pub struct TorrentUi {
    pub lists: Option<torrent::AppLists>,
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
    /// Manda magnet, indirizzi .torrent o file .torrent all'app scelta (o all'app predefinita).
    pub(super) fn add_torrents(&mut self, sources: Vec<String>) {
        if sources.is_empty() {
            return;
        }
        let app = self.cfg.torrent_app;
        self.torrents.notice = Some((
            tr!(
                format!("Invio a {}…", app.label()),
                format!("Sending to {}…", app.label())
            ),
            true,
        ));
        let _ = self
            .torrent_tx
            .send(Cmd::Add(sources, self.cfg.dm_dir.clone(), app));
    }

    pub(super) fn torrent_update(&mut self, u: Update) {
        match u {
            Update::List(l) => self.torrents.lists = Some(l),
            Update::Added {
                app,
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
                        format!("Aggiunto a {}: {names}", app.label()),
                        format!("Added to {}: {names}", app.label())
                    ));
                }
                if opened > 0 {
                    parts.push(tr!(
                        format!(
                            "{} non risponde: {opened} aperti con l'app predefinita per i torrent",
                            app.label()
                        ),
                        format!(
                            "{} does not respond: {opened} opened with the default torrent app",
                            app.label()
                        )
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
            // torrent finiti (in condivisione o fermi al 100%): «Togli i completati»
            let done: Vec<(TApp, String)> = self
                .torrents
                .lists
                .iter()
                .flatten()
                .filter_map(|(_, l)| l.as_ref().ok())
                .flatten()
                .filter(|t| t.progress >= 1.0)
                .map(|t| (t.app, t.id.clone()))
                .collect();
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr!("Torrent", "Torrents")).strong());
                ui.add_space(8.0);
                ui.label(tr!("Aggiungi a", "Add to"));
                for app in TApp::ALL {
                    if ui.radio(self.cfg.torrent_app == app, app.label()).clicked()
                        && self.cfg.torrent_app != app
                    {
                        self.cfg.torrent_app = app;
                        self.cfg.save();
                    }
                }
                if !done.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        clear = ui
                            .small_button(tr!(
                                format!("Togli i completati ({})", done.len()),
                                format!("Remove completed ({})", done.len())
                            ))
                            .on_hover_text(tr!(
                                "I torrent finiti escono dall'app torrent e smettono di essere condivisi; i file scaricati restano nella cartella.",
                                "Finished torrents leave the torrent app and stop seeding; the downloaded files stay in the folder."
                            ))
                            .clicked();
                    });
                }
            });
            if clear {
                let n = done.len();
                for (app, id) in done {
                    let _ = self.torrent_tx.send(Cmd::Remove(app, id));
                }
                let _ = self.torrent_tx.send(Cmd::Refresh);
                self.torrents.notice = Some((
                    tr!(
                        format!("Tolti {n} torrent completati: i file restano nella cartella"),
                        format!("Removed {n} completed torrents: the files stay in the folder")
                    ),
                    true,
                ));
            }
            if let Some((text, ok)) = &self.torrents.notice {
                ui.label(
                    RichText::new(text)
                        .small()
                        .color(if *ok { OK } else { ERR }),
                );
            }
            let chosen = self.cfg.torrent_app;
            let mut actions: Vec<Cmd> = Vec::new();
            let Some(lists) = &self.torrents.lists else {
                ui.label(
                    RichText::new(tr!("Collegamento alle app…", "Connecting to the apps…"))
                        .color(weak),
                );
                return;
            };
            // l'app scelta che non risponde lo dice con il rimedio; l'altra solo se ha un
            // problema diverso dall'essere chiusa (chi non la usa non vede niente)
            for (app, l) in lists {
                let Err(e) = l else { continue };
                if *app != chosen && torrent::is_not_running_text(e) {
                    continue;
                }
                ui.label(RichText::new(e).small().color(WARN));
                if *app == chosen {
                    ui.horizontal(|ui| {
                        if cfg!(target_os = "macos")
                            && ui
                                .button(tr!(
                                    format!("Apri {}", app.label()),
                                    format!("Open {}", app.label())
                                ))
                                .clicked()
                        {
                            app.launch();
                            actions.push(Cmd::Refresh);
                        }
                        ui.label(
                            RichText::new(tr!(
                                "Intanto i link magnet e i .torrent si aprono con l'app predefinita per i torrent.",
                                "Meanwhile magnet links and .torrent files open with the default torrent app."
                            ))
                            .small()
                            .color(weak),
                        );
                    });
                }
            }
            let all: Vec<&Torrent> = lists
                .iter()
                .filter_map(|(_, l)| l.as_ref().ok())
                .flatten()
                .collect();
            let chosen_ok = lists.iter().any(|(a, l)| *a == chosen && l.is_ok());
            if all.is_empty() && chosen_ok {
                ui.label(
                    RichText::new(tr!(
                        "Nessun torrent. Incolla qui sopra un link magnet o un indirizzo .torrent, oppure premi «Scegli file .torrent…».",
                        "No torrents. Paste a magnet link or a .torrent address above, or press “Choose .torrent files…”."
                    ))
                    .color(weak),
                );
            }
            // l'etichetta dell'app serve solo se ne rispondono due
            let both = lists.iter().filter(|(_, l)| l.is_ok()).count() > 1;
            for t in all {
                ui.add_space(4.0);
                torrent_row(ui, t, weak, both, &mut actions);
            }
            for a in actions {
                let _ = self.torrent_tx.send(a);
            }
        });
    }
}

fn torrent_row(
    ui: &mut egui::Ui,
    t: &Torrent,
    weak: Color32,
    show_app: bool,
    actions: &mut Vec<Cmd>,
) {
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
                if show_app {
                    ui.label(RichText::new(t.app.label()).small().color(weak));
                }
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
                // come qBittorrent: collegati, e tra parentesi quanti ce ne sono in tutto
                let count = |n: u32, all: Option<u32>| match all {
                    Some(a) => format!("{n} ({a})"),
                    None => n.to_string(),
                };
                parts.push(format!(
                    "seed {} · peer {}",
                    count(t.seeds, t.swarm_seeds),
                    count(t.peers, t.swarm_peers)
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
                        actions.push(Cmd::Resume(t.app, t.id.clone()));
                    }
                } else if ui.small_button(tr!("Pausa", "Pause")).clicked() {
                    actions.push(Cmd::Pause(t.app, t.id.clone()));
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
                    .small_button(tr!(
                        format!("Togli da {}", t.app.label()),
                        format!("Remove from {}", t.app.label())
                    ))
                    .on_hover_text(tr!(
                        "Il torrent esce dall'app; i file già scaricati restano nella cartella.",
                        "The torrent leaves the app; files already downloaded stay in the folder."
                    ))
                    .clicked()
                {
                    actions.push(Cmd::Remove(t.app, t.id.clone()));
                }
            });
        });
}
