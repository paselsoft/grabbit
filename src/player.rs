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
    /// artista sotto cui l'album compare in «Ascolta»: la cartella dell'artista
    /// (`Artista/Album`), altrimenti il primo artista dei tag; vuoto per i brani sciolti
    pub group: String,
    /// anno di uscita dai tag (il più vecchio tra i brani)
    pub year: Option<u32>,
    /// brani direttamente nella cartella della musica, fuori dagli album
    pub loose: bool,
    /// cartella dell'album (per i brani sciolti, la cartella della musica)
    pub dir: PathBuf,
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
            loose: dir == root,
            dir: dir.clone(),
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
        let mut years = Vec::new();
        for f in files {
            let (track, tags) = read_track(&f);
            if album.artist.is_empty() {
                album.artist = tags.album_artist.or(tags.artist).unwrap_or_default();
            }
            if let Some(y) = tags.year {
                years.push(y);
            }
            if cover_bytes.is_none() {
                cover_bytes = tags.cover;
            }
            album.tracks.push(track);
        }
        album.year = album_year(&years);
        // in «Artista/Album» (e «Artista/Album/CD1») l'artista è la cartella sotto la radice
        let artist_dir = (depth >= 2)
            .then(|| dir.strip_prefix(root).ok()?.components().next())
            .flatten()
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        if album.artist.is_empty() {
            album.artist = artist_dir.clone().unwrap_or_default();
        }
        if !album.loose {
            album.group = artist_dir.unwrap_or_else(|| primary_artist(&album.artist));
        }
        sort_tracks(&mut album.tracks);
        album.cover = cover_bytes.and_then(|b| crate::search::decode_cover(&b, cover_px).ok());
        lib.albums.push(album);
    }
    sort_albums(&mut lib.albums);
    lib
}

/// Testo per confrontare nomi di album, brani e artisti: minuscolo, solo lettere e cifre,
/// senza un anno davanti («2002.Tracce» → «tracce»).
pub fn match_key(s: &str) -> String {
    let words: Vec<String> = s
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    // «2002.Tracce», «2019 - L'altra metà»: l'anno davanti non fa parte del nome
    let skip = usize::from(
        words.len() > 1
            && words[0].len() == 4
            && words[0].chars().all(|c| c.is_ascii_digit())
            && year_of(&words[0]).is_some(),
    );
    words[skip..].join(" ")
}

impl Library {
    /// L'album della libreria con questo titolo e di questo artista (un risultato della
    /// ricerca): per scrivere «Già nella libreria».
    pub fn find_album(&self, title: &str, artist: &str) -> Option<usize> {
        let t = match_key(title);
        if t.is_empty() {
            return None;
        }
        let a = match_key(artist);
        self.albums
            .iter()
            .position(|al| !al.loose && match_key(&al.name) == t && same_artist(al, None, &a))
    }

    /// Il brano della libreria con questo titolo e di questo artista: (album, brano).
    pub fn find_track(&self, title: &str, artist: &str) -> Option<(usize, usize)> {
        let t = match_key(title);
        if t.is_empty() {
            return None;
        }
        let a = match_key(artist);
        self.albums.iter().enumerate().find_map(|(ai, al)| {
            al.tracks
                .iter()
                .position(|tr| match_key(&tr.title) == t && same_artist(al, Some(tr), &a))
                .map(|ti| (ai, ti))
        })
    }
}

/// L'artista cercato (`a`, già `match_key`) è quello dell'album o del brano? Uno dei due
/// nomi contiene l'altro («francesco renga» e «francesco renga ermal meta»).
fn same_artist(al: &Album, tr: Option<&Track>, a: &str) -> bool {
    if a.is_empty() {
        return true;
    }
    let mut names = vec![match_key(&al.group), match_key(&al.artist)];
    if let Some(t) = tr {
        names.push(match_key(&t.artist));
    }
    names
        .iter()
        .filter(|n| !n.is_empty())
        .any(|n| n.contains(a) || a.contains(n.as_str()))
}

/// Fino a questo numero di brani un'uscita conta come singolo (o EP breve).
pub const SINGLE_MAX_TRACKS: usize = 3;

impl Album {
    pub fn is_single(&self) -> bool {
        self.tracks.len() <= SINGLE_MAX_TRACKS
    }
}

/// Ordine di «Ascolta»: artisti in ordine alfabetico (senza artista e brani sciolti in
/// fondo); per ogni artista prima gli album poi i singoli, dal più recente.
fn sort_albums(albums: &mut [Album]) {
    albums.sort_by_key(|a| {
        (
            a.loose,
            a.group.is_empty(),
            a.group.to_lowercase(),
            a.is_single(),
            std::cmp::Reverse(a.year.unwrap_or(0)),
            a.name.to_lowercase(),
        )
    });
}

/// Anno dell'album: quello della maggior parte dei brani (un singolo uscito prima non
/// sposta tutto l'album); a parità, il più recente.
fn album_year(years: &[u32]) -> Option<u32> {
    years
        .iter()
        .map(|y| (years.iter().filter(|x| *x == y).count(), *y))
        .max()
        .map(|(_, y)| y)
}

/// Primo artista di «Artista A, Artista B», «A & B», «A feat. B».
pub fn primary_artist(s: &str) -> String {
    let mut end = s.len();
    let lower = s.to_lowercase();
    for sep in [
        ", ",
        " & ",
        " feat. ",
        " feat ",
        " ft. ",
        " featuring ",
        " x ",
        "; ",
    ] {
        if let Some(i) = lower.find(sep) {
            end = end.min(i);
        }
    }
    s[..end].trim().to_string()
}

/// Anno da una data dei tag: «2019», «2019-05-03», «20230907».
fn year_of(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take(4).collect();
    digits.parse().ok().filter(|y| (1900..=2100).contains(y))
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
    year: Option<u32>,
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
                Some(
                    StandardTagKey::Date
                    | StandardTagKey::ReleaseDate
                    | StandardTagKey::OriginalDate,
                ) => {
                    if let Some(y) = year_of(&text) {
                        tags.year = Some(tags.year.map_or(y, |t| t.min(y)));
                    }
                }
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
    /// brano, da dove partire, se restare in pausa (ripresa all'avvio)
    Play(PathBuf, Duration, bool),
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
                    Ok(Cmd::Play(path, start, paused)) => {
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
                                // in pausa prima di aggiungere il brano: non si sente nulla
                                if paused {
                                    p.pause();
                                }
                                p.append(dec);
                                if !start.is_zero() {
                                    let _ = p.try_seek(start);
                                }
                                player = Some(p);
                                set(&|s| {
                                    s.loaded = true;
                                    s.paused = paused;
                                    s.pos = start;
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
        let _ = self
            .tx
            .send(Cmd::Play(path.to_path_buf(), Duration::ZERO, false));
    }
    /// Carica un brano in pausa alla posizione indicata (per riprendere da dove eri).
    pub fn load_paused(&self, path: &Path, pos: Duration) {
        let _ = self.tx.send(Cmd::Play(path.to_path_buf(), pos, true));
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

    #[test]
    fn search_results_are_found_in_the_library() {
        assert_eq!(match_key("2002.Tracce"), "tracce");
        assert_eq!(match_key("L'altra metà"), "l altra metà");
        assert_eq!(match_key("21"), "21");
        assert_eq!(match_key("2019 - L'altra metà"), "l altra metà");
        let track = |title: &str| Track {
            title: title.into(),
            ..Track::default()
        };
        let lib = Library {
            albums: vec![
                Album {
                    name: "21".into(),
                    artist: "Adele".into(),
                    group: "Adele".into(),
                    tracks: vec![track("Rolling in the Deep"), track("Someone Like You")],
                    ..Album::default()
                },
                Album {
                    name: "2019.L'altra metà".into(),
                    artist: "Francesco Renga, Ermal Meta".into(),
                    group: "Francesco Renga".into(),
                    tracks: vec![track("L'altra metà")],
                    ..Album::default()
                },
            ],
            unsupported: 0,
        };
        assert_eq!(lib.find_album("21", "Adele"), Some(0));
        assert_eq!(lib.find_album("21", "Altro artista"), None);
        assert_eq!(lib.find_album("25", "Adele"), None);
        assert_eq!(lib.find_album("L'altra metà", "Francesco Renga"), Some(1));
        assert_eq!(lib.find_track("Someone like you", "Adele"), Some((0, 1)));
        assert_eq!(lib.find_track("Hello", "Adele"), None);
    }

    #[test]
    fn library_order_and_artists() {
        assert_eq!(
            primary_artist("Renga Nek, Francesco Renga, Nek"),
            "Renga Nek"
        );
        assert_eq!(primary_artist("Mina & Celentano"), "Mina");
        assert_eq!(primary_artist("Adele"), "Adele");
        assert_eq!(primary_artist("gIANMARIA Feat. Michielin"), "gIANMARIA");
        assert_eq!(year_of("20230907"), Some(2023));
        assert_eq!(year_of("2019-05-03"), Some(2019));
        assert_eq!(year_of("abc"), None);
        assert_eq!(year_of("NA"), None);
        // «19» di Adele: 11 brani del 2008, «Hometown Glory» uscito come singolo nel 2007
        let mut y = vec![2008; 11];
        y.push(2007);
        assert_eq!(album_year(&y), Some(2008));
        assert_eq!(album_year(&[2007, 2008]), Some(2008));
        assert_eq!(album_year(&[]), None);
        let album = |group: &str, name: &str, year: Option<u32>, n: usize| Album {
            name: name.into(),
            group: group.into(),
            year,
            tracks: vec![Track::default(); n],
            ..Album::default()
        };
        let mut v = vec![
            album("Francesco Renga", "Tracce", Some(2002), 11),
            album("", "Senza artista", None, 5),
            album("adele", "21", Some(2011), 11),
            album("Francesco Renga", "Non Dovrei", Some(2024), 1),
            album("Francesco Renga", "Ferro E Cartone", Some(2019), 11),
            Album {
                loose: true,
                ..album("", "Brani sciolti", None, 2)
            },
            album("Adele", "30", Some(2021), 12),
        ];
        sort_albums(&mut v);
        let names: Vec<&str> = v.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "30",
                "21",
                "Ferro E Cartone",
                "Tracce",
                "Non Dovrei",
                "Senza artista",
                "Brani sciolti"
            ]
        );
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
