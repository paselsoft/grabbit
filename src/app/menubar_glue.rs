// SPDX-License-Identifier: GPL-3.0-or-later
//! Collega la finestra alla barra dei menu del Mac: cosa mostrare e cosa fare per ogni voce.
use super::{notify, open_folder, App, JobState, Tab};
use crate::dm::State;
use crate::menubar::{self, Action, LinkKind, Status};
use crate::torrent::{self, Cmd};
use crate::tr;
use eframe::egui;
use std::time::{Duration, Instant};

/// Stato della barra dei menu tenuto dalla finestra.
#[derive(Default)]
pub struct MenuBarUi {
    pub last: Option<Instant>,
    /// spunta per qualche secondo dopo un giro di download finito bene
    pub done_until: Option<Instant>,
    /// pallino finché non si torna sulla finestra
    pub failed: bool,
    /// file e torrent messi in pausa dal menu, da riprendere con «Riprendi»
    pub paused_files: Vec<u64>,
    pub paused_torrents: Vec<(torrent::App, String)>,
}

fn short(name: &str) -> String {
    const MAX: usize = 40;
    if name.chars().count() <= MAX {
        return name.to_string();
    }
    let head: String = name.chars().take(MAX - 1).collect();
    format!("{head}…")
}

fn pct(f: f32) -> String {
    format!("{:.0}%", f.clamp(0.0, 1.0) * 100.0)
}

impl App {
    /// Da chiamare a ogni giro: aggiorna la barra dei menu (due volte al secondo) e fa le
    /// scelte arrivate dal menu.
    pub(super) fn menubar_tick(&mut self, ctx: &egui::Context) {
        for a in self.menubar.actions() {
            self.menubar_action(a, ctx);
        }
        if self
            .menubar_ui
            .last
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        self.menubar_ui.last = Some(Instant::now());
        let st = self.menubar_status();
        self.menubar.update(&st);
        // mentre si scarica (o la spunta è visibile) la barra si aggiorna anche senza mouse
        if st.active > 0 || self.menubar_ui.done_until.is_some() {
            ctx.request_repaint_after(Duration::from_millis(600));
        }
    }

    fn menubar_status(&mut self) -> Status {
        let mut st = Status {
            limit_kbps: self.cfg.speed_limit_kbps,
            ..Default::default()
        };
        let mut fractions = Vec::new();
        // video e musica
        if self.running {
            for j in self.jobs.iter().filter(|j| j.state == JobState::Running) {
                st.rows.push((short(&j.label), pct(j.pct)));
                fractions.push(j.pct);
            }
        }
        // file della scheda «File»
        let snap = self.dm.snapshot();
        for it in &snap.items {
            match it.state {
                State::Downloading => {
                    st.rows.push((short(&it.file_name), pct(it.fraction())));
                    fractions.push(it.fraction());
                    st.speed += it.speed;
                    st.can_pause = true;
                }
                State::Queued => st.can_pause = true,
                _ => {}
            }
        }
        // torrent delle app
        if let Some(lists) = &self.torrents.lists {
            for t in lists.iter().filter_map(|(_, l)| l.as_ref().ok()).flatten() {
                if t.status == torrent::Status::Downloading && t.progress < 1.0 {
                    st.rows.push((
                        short(&t.name),
                        format!("{} · {}", pct(t.progress), t.app.label()),
                    ));
                    fractions.push(t.progress);
                    st.speed += t.down_rate as f64;
                    st.can_pause = true;
                }
            }
        }
        st.active = fractions.len();
        if !fractions.is_empty() {
            st.fraction = Some(fractions.iter().sum::<f32>() / fractions.len() as f32);
        }
        st.can_resume =
            !self.menubar_ui.paused_files.is_empty() || !self.menubar_ui.paused_torrents.is_empty();
        if self
            .menubar_ui
            .done_until
            .is_some_and(|t| Instant::now() >= t)
        {
            self.menubar_ui.done_until = None;
        }
        st.look_done = self.menubar_ui.done_until.is_some();
        st.look_failed = self.menubar_ui.failed;
        st
    }

    /// Un giro di download con yt-dlp è finito: spunta, oppure pallino se qualcosa non è riuscito.
    pub(super) fn menubar_finished(&mut self) {
        if self.jobs.iter().any(|j| j.state == JobState::Failed) {
            self.menubar_ui.failed = true;
        } else {
            self.menubar_ui.done_until = Some(Instant::now() + Duration::from_secs(5));
        }
        self.menubar_ui.last = None;
    }

    /// Si è tornati sulla finestra: il pallino dell'errore ha fatto il suo lavoro.
    pub(super) fn menubar_seen(&mut self) {
        if self.menubar_ui.failed {
            self.menubar_ui.failed = false;
            self.menubar_ui.last = None;
        }
    }

    fn show_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn menubar_action(&mut self, a: Action, ctx: &egui::Context) {
        self.menubar_ui.last = None;
        match a {
            Action::OpenApp => Self::show_window(ctx),
            Action::OpenFolder => open_folder(&self.save_dir().clone()),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::SpeedLimit(k) => {
                self.cfg.speed_limit_kbps = k;
                if k > 0 {
                    self.dm_ui.limit_val = k;
                }
                self.dm.set_speed_limit_kbps(k);
                let _ = self.torrent_tx.send(Cmd::SpeedLimit(k));
                self.cfg.save();
            }
            Action::PauseAll => {
                for it in self.dm.snapshot().items {
                    if matches!(it.state, State::Downloading | State::Queued) {
                        self.dm.pause(it.id);
                        self.menubar_ui.paused_files.push(it.id);
                    }
                }
                if let Some(lists) = &self.torrents.lists {
                    for t in lists.iter().filter_map(|(_, l)| l.as_ref().ok()).flatten() {
                        if t.status == torrent::Status::Downloading && t.progress < 1.0 {
                            let _ = self.torrent_tx.send(Cmd::Pause(t.app, t.id.clone()));
                            self.menubar_ui.paused_torrents.push((t.app, t.id.clone()));
                        }
                    }
                }
            }
            Action::ResumeAll => {
                for id in std::mem::take(&mut self.menubar_ui.paused_files) {
                    self.dm.resume(id);
                }
                for (app, id) in std::mem::take(&mut self.menubar_ui.paused_torrents) {
                    let _ = self.torrent_tx.send(Cmd::Resume(app, id));
                }
            }
            Action::DownloadClipboard => self.download_clipboard(ctx),
        }
    }

    fn download_clipboard(&mut self, ctx: &egui::Context) {
        let text = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
            .unwrap_or_default();
        let Some((kind, link)) = menubar::link_kind(&text) else {
            notify(
                "Grabbit",
                tr!(
                    "Negli appunti non c'è un link da scaricare",
                    "There is no link to download on the clipboard"
                ),
            );
            return;
        };
        // non riproporlo con «Aggiungi» al ritorno sulla finestra
        self.clip_seen = text;
        match kind {
            LinkKind::Torrent => {
                self.add_torrents(vec![link]);
                let app = self.cfg.torrent_app.label();
                notify(
                    "Grabbit",
                    &tr!(
                        format!("Torrent mandato a {app}"),
                        format!("Torrent sent to {app}")
                    ),
                );
            }
            LinkKind::File => {
                let q = self.dm_ui.queue;
                if self
                    .dm
                    .add(&link, &self.cfg.dm_dir, q, self.cfg.dm_connections)
                    > 0
                {
                    self.dm.start_queue(q);
                    notify(
                        "Grabbit",
                        tr!(
                            "File aggiunto alla scheda «File»",
                            "File added to the Files tab"
                        ),
                    );
                }
            }
            LinkKind::Media if !self.running => {
                self.start_jobs(ctx, vec![link]);
                notify("Grabbit", tr!("Download avviato", "Download started"));
            }
            LinkKind::Media => {
                // c'è già un giro in corso: il link resta pronto nel campo
                self.tab = Tab::Download;
                if !self.input.trim().is_empty() {
                    self.input.push('\n');
                }
                self.input.push_str(&link);
                Self::show_window(ctx);
            }
        }
    }
}
