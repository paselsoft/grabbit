// SPDX-License-Identifier: GPL-3.0-or-later
// Su Windows (release) niente finestra console dietro all'interfaccia.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod config;
mod downloader;
mod i18n;
mod net;
mod search;
mod selfupdate;
mod spotify;
mod tools;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn window_icon() -> Option<egui_icon::IconData> {
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    Some(egui_icon::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

mod egui_icon {
    pub use eframe::egui::IconData;
}

fn main() -> eframe::Result {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Grabbit")
        .with_inner_size([780.0, 780.0])
        .with_min_inner_size([640.0, 560.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Grabbit",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
