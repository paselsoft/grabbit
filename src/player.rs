// SPDX-License-Identifier: GPL-3.0-or-later
//! Player audio («Ascolta»): legge una cartella di musica (album = sottocartelle) con titoli,
//! artisti, numeri di traccia e copertine, e riproduce i brani.
//!
//! La riproduzione gira in un thread a parte che possiede l'uscita audio (su macOS lo stream
//! di CoreAudio non può cambiare thread): l'interfaccia gli manda comandi e ne legge lo stato.
//! Formati: MP3, M4A/AAC, FLAC, WAV, OGG Vorbis (decodificati da symphonia). Opus no.
use crate::tr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey, Value};
use symphonia::core::probe::Hint;

/// Estensioni che il player sa riprodurre.
const PLAYABLE: [&str; 8] = ["mp3", "m4a", "mp4", "aac", "flac", "wav", "ogg", "oga"];
/// Estensioni audio che il player riconosce ma non sa riprodurre (si contano soltanto).
const UNSUPPORTED: [&str; 3] = ["opus", "webm", "weba"];
/// Profondità massima delle sottocartelle esplorate.
const MAX_DEPTH: usize = 3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub number: Option<u32>,
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug, Default)]
pub struct Album {
    pub name: String,
    pub artist: String,
    pub tracks: Vec<Track>,
    /// copertina già rimpicciolita: larghezza, altezza, pixel RGBA
    pub cover: Option<(u32, u32, Vec<u8>)>,
}

#[derive(Debug, Default)]
pub struct Library {
    pub albums: Vec<Album>,
    /// file audio trovati che il player non sa riprodurre (Opus, WebM)
    pub unsupported: usize,
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Esplora `root`: ogni cartella che contiene brani è un album (i brani direttamente in
/// `root` formano l'album «Brani sciolti»). `cover_px` è il lato massimo delle copertine.
pub fn scan(root: &Path, cover_px: u32, cancel: &AtomicBool) -> Library {
    let mut lib = Library::default();
    let mut dirs = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = dirs.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                if depth < MAX_DEPTH {
                    dirs.push((p, depth + 1));
                }
                continue;
            }
            let ext = ext_of(&p);
            if PLAYABLE.contains(&ext.as_str()) {
                files.push(p);
            } else if UNSUPPORTED.contains(&ext.as_str()) {
                lib.unsupported += 1;
            }
        }
        if files.is_empty() {
            continue;
        }
        let mut album = Album {
            name: if dir == root {
                tr!("Brani sciolti", "Loose tracks").into()
            } else {
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            ..Album::default()
        };
        let mut cover_bytes: Option<Vec<u8>> = None;
        for f in files {
            let (track, tags) = read_track(&f);
            if album.artist.is_empty() {
                album.artist = tags.album_artist.or(tags.artist).unwrap_or_default();
            }
            if cover_bytes.is_none() {
                cover_bytes = tags.cover;
            }
            album.tracks.push(track);
        }
        // senza tag: in «Artista/Album» l'artista è il nome della cartella sopra l'album
        if album.artist.is_empty() && depth == 2 {
            if let Some(parent) = dir.parent().and_then(|p| p.file_name()) {
                album.artist = parent.to_string_lossy().into_owned();
            }
        }
        sort_tracks(&mut album.tracks);
        album.cover = cover_bytes.and_then(|b| crate::search::decode_cover(&b, cover_px).ok());
        lib.albums.push(album);
    }
    // album in ordine alfabetico, i brani sciolti in fondo
    let loose = tr!("Brani sciolti", "Loose tracks");
    lib.albums.sort_by_key(|a| {
        (
            a.name == loose,
            a.name.to_lowercase(),
            a.artist.to_lowercase(),
        )
    });
    lib
}

/// Ordine dei brani: numero di traccia, poi nome del file.
fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| {
        let n = |t: &Track| t.number.unwrap_or(u32::MAX);
        n(a).cmp(&n(b)).then_with(|| a.path.cmp(&b.path))
    });
}

#[derive(Default)]
struct Tags {
    artist: Option<String>,
    album_artist: Option<String>,
    cover: Option<Vec<u8>>,
}

/// Numero all'inizio di un testo: «7», «07 - Titolo», «3/12».
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok().filter(|n| *n > 0)
}

/// Titolo e numero dal nome del file: «7 - Titolo.m4a» → (Some(7), «Titolo»).
fn from_file_name(p: &Path) -> (Option<u32>, String) {
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some((num, rest)) = stem.split_once(" - ") {
        if let Some(n) =
            leading_number(num).filter(|_| num.trim().chars().all(|c| c.is_ascii_digit()))
        {
            return (Some(n), rest.trim().to_string());
        }
    }
    (None, stem)
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string().trim().to_string(),
    }
}

/// Legge titolo, artista, numero di traccia, durata e copertina di un file.
fn read_track(path: &Path) -> (Track, Tags) {
    let (file_num, file_title) = from_file_name(path);
    let mut track = Track {
        path: path.to_path_buf(),
        title: file_title,
        number: file_num,
        ..Track::default()
    };
    let mut tags = Tags::default();
    let Ok(file) = File::open(path) else {
        return (track, tags);
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(&ext_of(path));
    let Ok(mut probed) = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) else {
        return (track, tags);
    };
    if let Some(t) = probed.format.default_track() {
        let p = &t.codec_params;
        if let (Some(n), Some(rate)) = (p.n_frames, p.sample_rate) {
            if rate > 0 {
                track.duration = Some(Duration::from_secs_f64(n as f64 / rate as f64));
            }
        }
    }
    let mut apply = |rev: &MetadataRevision| {
        for tag in rev.tags() {
            let text = value_text(&tag.value);
            if text.is_empty() {
                continue;
            }
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) => track.title = text,
                Some(StandardTagKey::Artist) => {
                    track.artist = text.clone();
                    tags.artist = Some(text);
                }
                Some(StandardTagKey::AlbumArtist) => tags.album_artist = Some(text),
                Some(StandardTagKey::TrackNumber) => {
                    if let Some(n) = leading_number(&text) {
                        track.number = Some(n);
                    }
                }
                _ => {}
            }
        }
        if tags.cover.is_none() {
            tags.cover = rev.visuals().first().map(|v| v.data.to_vec());
        }
    };
    // i metadati possono stare prima del contenitore (ID3 degli MP3) o dentro (M4A, FLAC)
    if let Some(rev) = probed.metadata.get().as_ref().and_then(|m| m.current()) {
        apply(rev);
    }
    if let Some(rev) = probed.format.metadata().current() {
        apply(rev);
    }
    (track, tags)
}

pub fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ───────────────────────────── riproduzione ─────────────────────────────

enum Cmd {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
    Seek(Duration),
    Volume(f32),
}

/// Stato del player, letto dall'interfaccia.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// c'è un brano caricato (in riproduzione o in pausa)
    pub loaded: bool,
    pub paused: bool,
    pub pos: Duration,
    /// aumenta ogni volta che un brano finisce da solo: l'interfaccia passa al successivo
    pub ended: u64,
    pub error: Option<String>,
}

pub struct Engine {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
}

impl Engine {
    /// Avvia il thread di riproduzione. `wake` chiede all'interfaccia di ridisegnarsi
    /// (quando un brano finisce o c'è un errore).
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel::<Cmd>();
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        std::thread::spawn(move || {
            // l'uscita audio si apre al primo brano, non all'avvio di Grabbit
            let mut sink: Option<rodio::MixerDeviceSink> = None;
            let mut player: Option<rodio::Player> = None;
            let mut volume = 1.0f32;
            let set = |f: &dyn Fn(&mut Status)| {
                if let Ok(mut s) = st.lock() {
                    f(&mut s);
                }
            };
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Cmd::Play(path)) => {
                        if let Some(p) = player.take() {
                            p.stop();
                        }
                        if sink.is_none() {
                            match rodio::DeviceSinkBuilder::open_default_sink() {
                                Ok(mut s) => {
                                    s.log_on_drop(false);
                                    sink = Some(s);
                                }
                                Err(e) => {
                                    let msg = tr!(
                                        format!("uscita audio non disponibile: {e}"),
                                        format!("audio output not available: {e}")
                                    );
                                    set(&|s| {
                                        s.loaded = false;
                                        s.error = Some(msg.clone());
                                    });
                                    wake();
                                    continue;
                                }
                            }
                        }
                        let decoded = File::open(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|f| rodio::Decoder::try_from(f).map_err(|e| e.to_string()));
                        match (decoded, sink.as_ref()) {
                            (Ok(dec), Some(s)) => {
                                let p = rodio::Player::connect_new(s.mixer());
                                p.set_volume(volume);
                                p.append(dec);
                                player = Some(p);
                                set(&|s| {
                                    s.loaded = true;
                                    s.paused = false;
                                    s.pos = Duration::ZERO;
                                    s.error = None;
                                });
                            }
                            (Err(e), _) => {
                                let msg = tr!(
                                    format!("impossibile riprodurre il file: {e}"),
                                    format!("could not play the file: {e}")
                                );
                                set(&|s| {
                                    s.loaded = false;
                                    s.error = Some(msg.clone());
                                });
                                wake();
                            }
                            (Ok(_), None) => {}
                        }
                    }
                    Ok(Cmd::Pause) => {
                        if let Some(p) = &player {
                            p.pause();
                            set(&|s| s.paused = true);
                        }
                    }
                    Ok(Cmd::Resume) => {
                        if let Some(p) = &player {
                            p.play();
                            set(&|s| s.paused = false);
                        }
                    }
                    Ok(Cmd::Stop) => {
                        if let Some(p) = player.take() {
                            p.stop();
                        }
                        set(&|s| {
                            s.loaded = false;
                            s.paused = false;
                            s.pos = Duration::ZERO;
                        });
                    }
                    Ok(Cmd::Seek(pos)) => {
                        if let Some(p) = &player {
                            let _ = p.try_seek(pos);
                        }
                    }
                    Ok(Cmd::Volume(v)) => {
                        volume = v;
                        if let Some(p) = &player {
                            p.set_volume(v);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // posizione, e fine naturale del brano
                if let Some(p) = &player {
                    if p.empty() {
                        player = None;
                        set(&|s| {
                            s.loaded = false;
                            s.paused = false;
                            s.ended += 1;
                        });
                        wake();
                    } else {
                        let pos = p.get_pos();
                        set(&|s| s.pos = pos);
                    }
                }
            }
        });
        Self { tx, status }
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn play(&self, path: &Path) {
        let _ = self.tx.send(Cmd::Play(path.to_path_buf()));
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    pub fn seek(&self, pos: Duration) {
        let _ = self.tx.send(Cmd::Seek(pos));
    }
    pub fn set_volume(&self, v: f32) {
        let _ = self.tx.send(Cmd::Volume(v.clamp(0.0, 1.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_titles_from_file_names() {
        assert_eq!(
            from_file_name(Path::new("/m/Album/7 - Anche Per Te.m4a")),
            (Some(7), "Anche Per Te".into())
        );
        assert_eq!(
            from_file_name(Path::new("/m/12 - Via!.mp3")),
            (Some(12), "Via!".into())
        );
        // un trattino nel titolo non è un numero
        assert_eq!(
            from_file_name(Path::new("/m/Max Nek Renga - Il disco.mp3")),
            (None, "Max Nek Renga - Il disco".into())
        );
        assert_eq!(leading_number("3/12"), Some(3));
        assert_eq!(leading_number("0"), None);
        assert_eq!(fmt_time(Duration::from_secs(225)), "3:45");
    }

    /// Legge una cartella vera e stampa cosa trova:
    /// GRABBIT_MUSIC=/percorso cargo test scan_real_folder -- --ignored --nocapture
    #[test]
    #[ignore]
    fn scan_real_folder() {
        let Ok(dir) = std::env::var("GRABBIT_MUSIC") else {
            return;
        };
        let lib = scan(Path::new(&dir), 128, &AtomicBool::new(false));
        println!("non riproducibili: {}", lib.unsupported);
        for a in &lib.albums {
            println!(
                "ALBUM «{}» di «{}», copertina: {:?}",
                a.name,
                a.artist,
                a.cover.as_ref().map(|(w, h, _)| (*w, *h))
            );
            for t in &a.tracks {
                println!(
                    "  {:?} «{}» «{}» {:?}",
                    t.number, t.title, t.artist, t.duration
                );
            }
        }
    }

    #[test]
    fn tracks_sort_by_number_then_name() {
        let t = |p: &str, n: Option<u32>| Track {
            path: p.into(),
            number: n,
            ..Track::default()
        };
        let mut v = vec![
            t("b", None),
            t("x", Some(10)),
            t("y", Some(2)),
            t("a", None),
        ];
        sort_tracks(&mut v);
        let order: Vec<_> = v
            .iter()
            .map(|t| t.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(order, ["y", "x", "a", "b"]);
    }

    /// Una cartella con un album (sottocartella) e un brano sciolto: WAV generati qui,
    /// letti davvero con symphonia (durata), più un file Opus da contare come non riproducibile.
    #[test]
    fn scans_a_music_folder() {
        let root = std::env::temp_dir().join(format!("grabbit-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let album = root.join("Il mio album");
        std::fs::create_dir_all(&album).unwrap();
        // 1 s di silenzio, 8 kHz mono 16 bit
        let wav = |path: &Path| {
            let rate = 8000u32;
            let data = vec![0u8; (rate * 2) as usize];
            let mut b = Vec::new();
            b.extend_from_slice(b"RIFF");
            b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
            b.extend_from_slice(b"WAVEfmt ");
            b.extend_from_slice(&16u32.to_le_bytes());
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&1u16.to_le_bytes());
            b.extend_from_slice(&rate.to_le_bytes());
            b.extend_from_slice(&(rate * 2).to_le_bytes());
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(&16u16.to_le_bytes());
            b.extend_from_slice(b"data");
            b.extend_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend_from_slice(&data);
            std::fs::write(path, b).unwrap();
        };
        wav(&album.join("2 - Secondo.wav"));
        wav(&album.join("1 - Primo.wav"));
        wav(&root.join("Sciolto.wav"));
        std::fs::write(album.join("3 - Terzo.opus"), b"x").unwrap();
        std::fs::write(album.join("copertina.jpg"), b"x").unwrap();

        let lib = scan(&root, 64, &AtomicBool::new(false));
        assert_eq!(lib.unsupported, 1);
        assert_eq!(lib.albums.len(), 2);
        let a = lib
            .albums
            .iter()
            .find(|a| a.name == "Il mio album")
            .unwrap();
        let titles: Vec<_> = a.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Primo", "Secondo"]);
        assert_eq!(a.tracks[0].number, Some(1));
        let d = a.tracks[0].duration.expect("durata letta dal file");
        assert!((d.as_secs_f32() - 1.0).abs() < 0.05, "{d:?}");
        let loose = lib
            .albums
            .iter()
            .find(|a| a.name != "Il mio album")
            .unwrap();
        assert_eq!(loose.tracks[0].title, "Sciolto");
        let _ = std::fs::remove_dir_all(&root);
    }
}
