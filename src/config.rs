// SPDX-License-Identifier: GPL-3.0-or-later
//! Impostazioni salvate su disco (JSON nella cartella di configurazione).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    pub out_dir: PathBuf,
    /// "mp4" | "mkv" | "webm"
    pub video_container: String,
    /// 0 = migliore disponibile
    pub max_height: u32,
    /// "mp3" | "m4a" | "opus" | "vorbis" | "flac" | "wav"
    pub audio_format: String,
    /// kbit/s (ignorato per flac/wav)
    pub audio_bitrate: u32,
    pub playlist: bool,
    pub embed_meta: bool,
    /// "" = nessuno, altrimenti "chrome", "firefox", ...
    pub cookies_browser: String,
    pub auto_update_app: bool,
    /// ingrandimento dell'interfaccia (1.0 = normale)
    pub ui_scale: f32,
    /// dove cerca la barra di ricerca (YouTube o una sezione di YouTube Music)
    pub search_source: crate::search::Source,
    /// quanti download insieme (1–4)
    pub parallel: u32,
    /// cartella della musica da ascoltare (None = quella dei download)
    pub player_dir: Option<PathBuf>,
    /// volume del player (0–1)
    pub volume: f32,
    /// non riscaricare i brani che sono già nella cartella (archivio dei download)
    pub skip_downloaded: bool,
}

impl Default for Config {
    fn default() -> Self {
        let out_dir = directories::UserDirs::new()
            .map(|u| {
                u.download_dir()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| u.home_dir().join("Downloads"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            mode: Mode::Video,
            out_dir,
            video_container: "mp4".into(),
            max_height: 1080,
            audio_format: "mp3".into(),
            audio_bitrate: 320,
            playlist: false,
            embed_meta: true,
            cookies_browser: String::new(),
            auto_update_app: true,
            ui_scale: 1.0,
            search_source: Default::default(),
            parallel: 3,
            player_dir: None,
            volume: 0.8,
            skip_downloaded: true,
        }
    }
}

fn path() -> Option<PathBuf> {
    crate::tools::project_dirs().map(|d| d.config_dir().join("config.json"))
}

impl Config {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}
