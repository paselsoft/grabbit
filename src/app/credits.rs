// SPDX-License-Identifier: GPL-3.0-or-later
//! Finestra «Licenze e crediti»: la licenza di Grabbit, da dove viene il codice adattato, gli
//! strumenti che Grabbit scarica e le licenze di tutte le librerie incluse.
use super::{App, ACCENT};
use crate::tr;
use eframe::egui::{self, RichText};

/// Licenze delle librerie Rust, generate da `scripts/licenses.py` (licenza TAB librerie).
const LICENSES: &str = include_str!("../../assets/licenses.txt");

/// (licenza, librerie) dalle righe di `assets/licenses.txt`.
pub fn license_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    LICENSES
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.split_once('\t'))
        .map(|(lic, crates)| (lic, crates.split(", ").collect()))
        .collect()
}

fn link(ui: &mut egui::Ui, text: &str, url: &str) {
    ui.hyperlink_to(RichText::new(text).color(ACCENT), url);
}

impl App {
    pub(super) fn credits_window(&mut self, ctx: &egui::Context) {
        if !self.show_credits {
            return;
        }
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::unique("credits")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(
                RichText::new(tr!("Licenze e crediti", "Licenses and credits"))
                    .strong()
                    .size(20.0),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(ctx.content_rect().height() * 0.7)
                .show(ui, |ui| {
                    ui.label(RichText::new("Grabbit").strong());
                    ui.label(tr!(
                        format!("Versione {}. Software libero con licenza GPL-3.0-or-later: puoi usarlo, studiarlo, condividerlo e modificarlo; le versioni modificate vanno distribuite con la stessa licenza e con i sorgenti.", env!("CARGO_PKG_VERSION")),
                        format!("Version {}. Free software under the GPL-3.0-or-later license: you may use, study, share and modify it; modified versions must be distributed under the same license with their source code.", env!("CARGO_PKG_VERSION"))
                    ));
                    ui.horizontal_wrapped(|ui| {
                        link(ui, tr!("Testo della licenza", "License text"), "https://www.gnu.org/licenses/gpl-3.0.html");
                        ui.label("·");
                        link(ui, tr!("Sorgenti", "Source code"), "https://github.com/paselsoft/grabbit");
                    });
                    ui.label(
                        RichText::new(tr!(
                            "Scritto con l'aiuto di Claude, l'assistente di Anthropic.",
                            "Written with the help of Claude, Anthropic's assistant."
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(10.0);

                    ui.label(RichText::new(tr!("Crediti", "Credits")).strong());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr!(
                            "Il download manager della scheda «File» è adattato da",
                            "The download manager in the Files tab is adapted from"
                        ));
                        link(ui, "Dropall", "https://github.com/marcogiannini/dropall");
                        ui.label(tr!(
                            "di Marco Giannini (GPL-3.0-or-later).",
                            "by Marco Giannini (GPL-3.0-or-later)."
                        ));
                    });
                    ui.add_space(10.0);

                    ui.label(RichText::new(tr!("Programmi che Grabbit scarica e usa", "Programs Grabbit downloads and uses")).strong());
                    ui.label(
                        RichText::new(tr!(
                            "Non sono dentro Grabbit: li scarica al primo avvio dai siti ufficiali e li usa come programmi separati.",
                            "They are not inside Grabbit: it downloads them on first launch from their official sites and runs them as separate programs."
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                    for (name, lic, url) in [
                        ("yt-dlp", "Unlicense", "https://github.com/yt-dlp/yt-dlp"),
                        ("FFmpeg", tr!("LGPL-2.1 o GPL, secondo la build", "LGPL-2.1 or GPL, depending on the build"), "https://ffmpeg.org/legal.html"),
                        ("Deno", "MIT", "https://deno.com"),
                        ("QuickJS", "MIT", "https://bellard.org/quickjs/"),
                    ] {
                        ui.horizontal_wrapped(|ui| {
                            link(ui, name, url);
                            ui.label(RichText::new(lic).color(ui.visuals().weak_text_color()));
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr!("Torrent tramite", "Torrents via"));
                        link(ui, "Transmission", "https://transmissionbt.com");
                        ui.label(tr!("e", "and"));
                        link(ui, "qBittorrent", "https://www.qbittorrent.org");
                        ui.label(tr!(
                            "già installati sul computer (GPL).",
                            "already installed on the computer (GPL)."
                        ));
                    });
                    ui.add_space(10.0);

                    let groups = license_groups();
                    let total: usize = groups.iter().map(|g| g.1.len()).sum();
                    ui.label(
                        RichText::new(tr!(
                            format!("Librerie incluse ({total})"),
                            format!("Included libraries ({total})")
                        ))
                        .strong(),
                    );
                    ui.label(
                        RichText::new(tr!(
                            "Le librerie Rust compilate dentro Grabbit, raggruppate per licenza. I caratteri dell'interfaccia (Ubuntu, Noto, Hack) vengono da egui con le loro licenze (Ubuntu Font Licence, OFL).",
                            "The Rust libraries built into Grabbit, grouped by license. The interface fonts (Ubuntu, Noto, Hack) come from egui with their licenses (Ubuntu Font Licence, OFL)."
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                    for (lic, crates) in groups {
                        egui::CollapsingHeader::new(format!("{lic}  ·  {}", crates.len()))
                            .id_salt(("lic", lic))
                            .show(ui, |ui| {
                                ui.label(RichText::new(crates.join(", ")).small());
                            });
                    }
                });
            ui.add_space(8.0);
            if ui.button(tr!("Chiudi", "Close")).clicked() {
                close = true;
            }
        });
        if close || resp.should_close() {
            self.show_credits = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ogni libreria di Cargo.lock è nell'elenco delle licenze: se fallisce, lancia
    /// `python3 scripts/licenses.py` dopo aver cambiato le dipendenze.
    #[test]
    fn licenses_cover_every_crate() {
        let listed: std::collections::HashSet<String> = license_groups()
            .into_iter()
            .flat_map(|(_, c)| c)
            .map(String::from)
            .collect();
        let lock = include_str!("../../Cargo.lock");
        let mut missing = Vec::new();
        for pkg in lock.split("[[package]]").skip(1) {
            let field = |k: &str| {
                pkg.lines()
                    .find_map(|l| l.strip_prefix(&format!("{k} = \"")))
                    .map(|v| v.trim_end_matches('"').to_string())
                    .unwrap_or_default()
            };
            let (name, version) = (field("name"), field("version"));
            if name == "grabbit" {
                continue;
            }
            if !listed.contains(&format!("{name} {version}")) {
                missing.push(format!("{name} {version}"));
            }
        }
        assert!(
            missing.is_empty(),
            "librerie senza licenza nell'elenco (lancia scripts/licenses.py): {missing:?}"
        );
    }
}
