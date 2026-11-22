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
/// Video che il player non legge ma si aprono nel player video del sistema
/// (gli MP4 con un video dentro si riconoscono leggendo il file).
const VIDEO: [&str; 4] = ["mkv", "mov", "m4v", "avi"];
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
    /// file con un video (es. un video musicale MP4): si apre nel player video del sistema
    pub video: bool,
    /// numero del disco (dai tag o dalla cartella «CD2»), per gli album su più dischi
    pub disc: Option<u32>,
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
    /// brani letti davvero dal disco (gli altri vengono dalla cache della scansione)
    pub fresh: usize,
}

/// Cache della scansione, come un backup incrementale: per ogni file i dati già letti, con
/// dimensione e data di modifica. Un file uguale non si riapre; le copertine già rimpicciolite
/// stanno in `covers/` come PNG.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ScanCache {
    /// versione del formato: se cambia, la cache si ignora
    v: u32,
    files: std::collections::HashMap<PathBuf, CachedFile>,
}

const CACHE_VERSION: u32 = 2;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct CachedFile {
    size: u64,
    /// data di modifica in nanosecondi dal 1970
    mtime: u128,
    title: String,
    artist: String,
    number: Option<u32>,
    duration_ms: Option<u64>,
    video: bool,
    tag_artist: Option<String>,
    album_artist: Option<String>,
    year: Option<u32>,
    has_cover: bool,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    disc: Option<u32>,
}

/// Dimensione e data di modifica di un file (None se non si leggono).
fn stamp(path: &Path) -> Option<(u64, u128)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((m.len(), t))
}

impl CachedFile {
    fn new(stamp: (u64, u128), track: &Track, tags: &Tags) -> Self {
        CachedFile {
            size: stamp.0,
            mtime: stamp.1,
            title: track.title.clone(),
            artist: track.artist.clone(),
            number: track.number,
            duration_ms: track.duration.map(|d| d.as_millis() as u64),
            video: track.video,
            tag_artist: tags.artist.clone(),
            album_artist: tags.album_artist.clone(),
            year: tags.year,
            has_cover: tags.cover.is_some(),
            album: tags.album.clone(),
            disc: track.disc,
        }
    }

    fn restore(&self, path: &Path) -> (Track, Tags) {
        let track = Track {
            path: path.to_path_buf(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            number: self.number,
            duration: self.duration_ms.map(Duration::from_millis),
            video: self.video,
            disc: self.disc,
        };
        let tags = Tags {
            artist: self.tag_artist.clone(),
            year: self.year,
            album_artist: self.album_artist.clone(),
            album: self.album.clone(),
            cover: None,
        };
        (track, tags)
    }
}

/// Nome stabile per la copertina in cache (FNV-1a: non cambia tra versioni di Rust).
fn cover_key(path: &Path, stamp: (u64, u128), px: u32) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let text = format!("{}|{}|{}|{px}", path.display(), stamp.0, stamp.1);
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}.png")
}

fn load_cache(dir: &Path) -> ScanCache {
    std::fs::read(dir.join("library.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<ScanCache>(&b).ok())
        .filter(|c| c.v == CACHE_VERSION)
        .unwrap_or_default()
}

fn save_cache(dir: &Path, cache: &ScanCache) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join("library.json.tmp");
    if let Ok(b) = serde_json::to_vec(cache) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("library.json"));
        }
    }
}

/// Copertine in cache non più usate da nessun album: si tolgono.
fn prune_covers(dir: &Path, used: &std::collections::HashSet<String>) {
    let Ok(rd) = std::fs::read_dir(dir.join("covers")) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !used.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Esplora `root`: ogni cartella che contiene brani è un album (i brani direttamente in
/// `root` formano l'album «Brani sciolti»). `cover_px` è il lato massimo delle copertine.
#[cfg(test)]
pub fn scan(root: &Path, cover_px: u32, cancel: &AtomicBool) -> Library {
    scan_cached(root, cover_px, cancel, None, &|_, _| {})
}

/// Come `scan`, ma con la cache in `cache_dir`: si riaprono solo i file nuovi o cambiati.
/// `progress(fatti, totale)` dice quanti brani sono stati letti (al più ogni 0,1%).
pub fn scan_cached(
    root: &Path,
    cover_px: u32,
    cancel: &AtomicBool,
    cache_dir: Option<&Path>,
    progress: &dyn Fn(usize, usize),
) -> Library {
    let old = cache_dir.map(load_cache).unwrap_or_default();
    let mut new = ScanCache {
        v: CACHE_VERSION,
        files: Default::default(),
    };
    let mut used_covers = std::collections::HashSet::new();
    let mut lib = Library::default();
    // 1) si elencano le cartelle e si contano i brani (veloce): serve per la percentuale
    let mut found: Vec<(PathBuf, usize, Vec<PathBuf>)> = Vec::new();
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
            if PLAYABLE.contains(&ext.as_str()) || VIDEO.contains(&ext.as_str()) {
                files.push(p);
            } else if UNSUPPORTED.contains(&ext.as_str()) {
                lib.unsupported += 1;
            }
        }
        if !files.is_empty() {
            found.push((dir, depth, files));
        }
    }
    // 2) si leggono i brani (dalla cache quelli già visti), dicendo a che punto si è
    let total: usize = found.iter().map(|f| f.2.len()).sum();
    let mut done = 0usize;
    let mut last_step = usize::MAX;
    progress(0, total);
    let mut albums: Vec<Album> = Vec::new();
    let mut from_tag: Vec<bool> = Vec::new();
    for (dir, depth, files) in found {
        if cancel.load(Ordering::Relaxed) {
            break;
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
        // il file da cui viene la copertina (il primo che ne ha una), per la cache
        let mut cover_from: Option<(PathBuf, (u64, u128))> = None;
        let mut years = Vec::new();
        let mut titles: Vec<String> = Vec::new();
        for f in files {
            done += 1;
            let step = done * 1000 / total.max(1);
            if step != last_step {
                last_step = step;
                progress(done, total);
            }
            let st = stamp(&f);
            let cached = st.and_then(|st| {
                old.files
                    .get(&f)
                    .filter(|c| c.size == st.0 && c.mtime == st.1)
            });
            let (track, tags, has_cover) = match cached {
                Some(c) => {
                    let (t, g) = c.restore(&f);
                    new.files.insert(f.clone(), c.clone());
                    (t, g, c.has_cover)
                }
                None => {
                    let (t, g) = read_track(&f);
                    lib.fresh += 1;
                    if let Some(st) = st {
                        new.files.insert(f.clone(), CachedFile::new(st, &t, &g));
                    }
                    let has = g.cover.is_some();
                    (t, g, has)
                }
            };
            if album.artist.is_empty() {
                album.artist = tags
                    .album_artist
                    .clone()
                    .or(tags.artist.clone())
                    .unwrap_or_default();
            }
            if let Some(y) = tags.year {
                years.push(y);
            }
            if let Some(t) = tags.album.as_ref().filter(|t| !t.trim().is_empty()) {
                titles.push(t.trim().to_string());
            }
            if cover_from.is_none() && has_cover {
                cover_from = st.map(|st| (f.clone(), st));
                cover_bytes = tags.cover;
            }
            album.tracks.push(track);
        }
        album.year = album_year(&years);
        // senza anno nei tag: quello davanti al nome della cartella («2015 - 25»)
        if album.year.is_none() && !album.loose {
            album.year = folder_year(&album.name);
        }
        // il nome dai tag dell'album («30»), non dalla cartella
        // («Adele.-.30.(RTI).PBTHAL.(2021.-.Pop).[Flac.24-96.LP]-xpost»)
        let tag_name = (!album.loose).then(|| common_title(&titles)).flatten();
        let named_by_tag = tag_name.is_some();
        if let Some(t) = tag_name {
            album.name = t;
        }
        // il disco dalla cartella («CD2») per i brani che non lo dicono nei tag
        let folder_disc = dir
            .file_name()
            .and_then(|n| disc_folder(&n.to_string_lossy()))
            .filter(|_| !album.loose);
        if folder_disc.is_some() {
            for t in album.tracks.iter_mut().filter(|t| t.disc.is_none()) {
                t.disc = folder_disc;
            }
        }
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
        // niente copertina nei brani: un'immagine nella cartella (o in quella dell'album, per
        // «Album/CD1»), come «folder.jpg» o «cover.jpg»
        let image = if cover_from.is_none() && !album.loose {
            folder_image(&dir).or_else(|| folder_disc.and(dir.parent()).and_then(folder_image))
        } else {
            None
        };
        if let Some(img) = &image {
            cover_from = stamp(img).map(|st| (img.clone(), st));
            cover_bytes = None;
        }
        let read_cover = |f: &Path| -> Option<Vec<u8>> {
            if image.as_deref() == Some(f) {
                std::fs::read(f).ok()
            } else {
                read_track(f).1.cover
            }
        };
        album.cover = match (&cover_from, cache_dir) {
            // copertina già rimpicciolita in cache: si carica il PNG
            (Some((f, st)), Some(dir)) => {
                let key = cover_key(f, *st, cover_px);
                let png = dir.join("covers").join(&key);
                used_covers.insert(key);
                let cached = image::open(&png).ok().map(|i| {
                    let i = i.to_rgba8();
                    (i.width(), i.height(), i.into_raw())
                });
                cached.or_else(|| {
                    // non c'è (o il file è cambiato): si legge e si salva per la prossima volta
                    let bytes = cover_bytes.take().or_else(|| read_cover(f))?;
                    let c = crate::search::decode_cover(&bytes, cover_px).ok()?;
                    let _ = std::fs::create_dir_all(dir.join("covers"));
                    if let Some(img) = image::RgbaImage::from_raw(c.0, c.1, c.2.clone()) {
                        let _ = img.save(&png);
                    }
                    Some(c)
                })
            }
            (Some((f, _)), None) => cover_bytes
                .or_else(|| read_cover(f))
                .and_then(|b| crate::search::decode_cover(&b, cover_px).ok()),
            (None, _) => None,
        };
        albums.push(album);
        from_tag.push(named_by_tag);
    }
    // «Album/CD1», «Album/CD2»: un solo album, disco per disco
    lib.albums = merge_discs(albums, from_tag, root);
    sort_albums(&mut lib.albums);
    // una scansione interrotta non deve buttare via la cache dei file non visti
    if let Some(dir) = cache_dir.filter(|_| !cancel.load(Ordering::Relaxed)) {
        save_cache(dir, &new);
        prune_covers(dir, &used_covers);
    }
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
/// Ordine dei brani: disco, numero di traccia, poi nome del file.
fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| {
        let n = |t: &Track| (t.disc.unwrap_or(0), t.number.unwrap_or(u32::MAX));
        n(a).cmp(&n(b)).then_with(|| a.path.cmp(&b.path))
    });
}

/// Una cartella di un disco dentro l'album: «CD1», «CD 2», «Disc 1», «Disco 2» → il numero;
/// «Bonus CD», «Bonus» → 99 (in fondo).
fn disc_folder(name: &str) -> Option<u32> {
    let s: String = name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    for p in ["cd", "disc", "disk", "disco"] {
        if let Some(rest) = s.strip_prefix(p) {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return rest.parse().ok();
            }
        }
    }
    for p in ["bonuscd", "bonusdisc", "bonusdisk", "bonus"] {
        if let Some(rest) = s.strip_prefix(p) {
            if rest.chars().all(|c| c.is_ascii_digit()) {
                return Some(99);
            }
        }
    }
    None
}

/// Immagine della copertina nella cartella dell'album, per gli album senza copertina nei
/// brani: prima «cover», «folder», «front», «album…», poi una qualsiasi; poi le sottocartelle
/// «Artwork», «Scans», «Covers».
fn folder_image(dir: &Path) -> Option<PathBuf> {
    fn images(dir: &Path) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.flatten()
            .map(|e| e.path())
            .filter(|p| {
                let hidden = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
                !hidden && p.is_file() && matches!(ext_of(p).as_str(), "jpg" | "jpeg" | "png")
            })
            .collect()
    }
    fn best(mut v: Vec<PathBuf>) -> Option<PathBuf> {
        let rank = |p: &PathBuf| {
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            ["cover", "folder", "front", "album"]
                .iter()
                .position(|k| stem.starts_with(k))
                .unwrap_or(9)
        };
        v.sort_by_key(|p| (rank(p), p.clone()));
        v.into_iter().next()
    }
    if let Some(p) = best(images(dir)) {
        return Some(p);
    }
    let rd = std::fs::read_dir(dir).ok()?;
    let subs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .is_some_and(|n| {
                        matches!(n.as_str(), "artwork" | "scans" | "covers" | "images")
                    })
        })
        .collect();
    best(subs.iter().flat_map(|d| images(d)).collect())
}

/// L'anno all'inizio del nome di una cartella: «2015 - 25», «2002.Tracce» → 2015, 2002.
fn folder_year(name: &str) -> Option<u32> {
    let t = name.trim();
    let digits: String = t.chars().take(4).collect();
    let next = t.chars().nth(4);
    (digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit()))
        .then_some(())
        .filter(|_| next.is_none_or(|c| !c.is_alphanumeric()))
        .and_then(|_| year_of(&digits))
}

/// Il titolo più frequente tra quelli scritti nei tag dei brani.
fn common_title(titles: &[String]) -> Option<String> {
    let mut best: Option<(&String, usize)> = None;
    for t in titles {
        let n = titles.iter().filter(|x| *x == t).count();
        if best.is_none_or(|(_, m)| n > m) {
            best = Some((t, n));
        }
    }
    best.map(|(t, _)| t.clone())
}

/// Unisce le cartelle dei dischi («Album/CD1», «Album/CD2», «Album/Bonus CD») nel loro album.
/// `from_tag[i]` dice se il nome dell'album `i` viene dai tag (altrimenti è la cartella).
fn merge_discs(albums: Vec<Album>, from_tag: Vec<bool>, root: &Path) -> Vec<Album> {
    let mut out: Vec<Album> = Vec::new();
    let mut named_by_tag: Vec<bool> = Vec::new();
    let mut by_dir: std::collections::HashMap<PathBuf, usize> = Default::default();
    // prima gli album veri, poi i dischi (così un disco trova la cartella madre se ha brani)
    let mut pending = Vec::new();
    for (a, t) in albums.into_iter().zip(from_tag) {
        let disc = (!a.loose)
            .then(|| a.dir.file_name().map(|n| n.to_string_lossy().into_owned()))
            .flatten()
            .and_then(|n| disc_folder(&n));
        let parent = a.dir.parent().map(Path::to_path_buf);
        match (disc, parent) {
            (Some(_), Some(parent)) if parent != root => pending.push((a, t, parent)),
            _ => {
                by_dir.insert(a.dir.clone(), out.len());
                out.push(a);
                named_by_tag.push(t);
            }
        }
    }
    for (disc, tag, parent) in pending {
        let i = match by_dir.get(&parent) {
            Some(&i) => i,
            None => {
                let name = if tag {
                    disc.name.clone()
                } else {
                    parent
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                by_dir.insert(parent.clone(), out.len());
                out.push(Album {
                    name,
                    dir: parent,
                    tracks: Vec::new(),
                    cover: None,
                    ..disc.clone()
                });
                named_by_tag.push(tag);
                out.len() - 1
            }
        };
        let a = &mut out[i];
        if !named_by_tag[i] && tag {
            a.name = disc.name.clone();
            named_by_tag[i] = true;
        }
        if a.artist.is_empty() {
            a.artist = disc.artist.clone();
        }
        if a.group.is_empty() {
            a.group = disc.group.clone();
        }
        a.year = match (a.year, disc.year) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        };
        if a.cover.is_none() {
            a.cover = disc.cover;
        }
        a.tracks.extend(disc.tracks);
        sort_tracks(&mut a.tracks);
    }
    out
}

#[derive(Default)]
struct Tags {
    artist: Option<String>,
    year: Option<u32>,
    album_artist: Option<String>,
    /// titolo dell'album scritto nei tag
    album: Option<String>,
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
    if VIDEO.contains(&ext_of(path).as_str()) {
        track.video = true;
        return (track, tags);
    }
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
    // una traccia senza frequenza di campionamento è un video (H.264 di un MP4)
    track.video = probed
        .format
        .tracks()
        .iter()
        .any(|t| t.codec_params.sample_rate.is_none());
    // la durata dalla traccia audio, che in un video non è la prima
    let audio = probed
        .format
        .tracks()
        .iter()
        .find(|t| t.codec_params.sample_rate.is_some());
    if let Some(t) = audio {
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
                Some(StandardTagKey::Album) => tags.album = Some(text),
                Some(StandardTagKey::DiscNumber) => {
                    if let Some(n) = leading_number(&text) {
                        track.disc = Some(n);
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

// ───────────────────────── volume uguale tra i brani ─────────────────────────

/// Livello a cui si portano tutti i brani (dB di potenza media sui tratti non silenziosi):
/// circa quello di ReplayGain. I master moderni, più forti, vengono abbassati.
const TARGET_DB: f32 = -16.0;
/// Correzione massima: quanto si alza un brano molto piano, quanto si abbassa uno forte.
const MAX_BOOST_DB: f32 = 6.0;
const MAX_CUT_DB: f32 = 15.0;

/// Correzione di volume (moltiplicatore) per portare il brano al livello comune.
/// Decodifica tutto il file: blocchi da 0,4 s, si scartano i silenzi (sotto -70 dB) e i
/// tratti molto più piani della media (10 dB sotto), come in EBU R128 ma senza filtro K.
/// Non alza mai il brano oltre il suo picco (niente distorsione).
pub fn measure_gain(path: &Path) -> Option<f32> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;

    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(&ext_of(path));
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed.format.default_track()?.clone();
    let rate = track.codec_params.sample_rate.unwrap_or(44_100) as usize;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;
    let block = (rate * 2 / 5).max(1);
    let (mut blocks, mut sum, mut frames, mut peak) = (Vec::<f64>::new(), 0f64, 0usize, 0f32);
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match probed.format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(_)) | Err(Error::ResetRequired) => break,
            Err(_) => return None,
        };
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        for frame in b.samples().chunks(channels) {
            let mut ms = 0f64;
            for &x in frame {
                peak = peak.max(x.abs());
                ms += (x as f64) * (x as f64);
            }
            sum += ms / channels as f64;
            frames += 1;
            if frames == block {
                blocks.push(sum / block as f64);
                sum = 0.0;
                frames = 0;
            }
        }
    }
    gain_from_blocks(&blocks, peak)
}

/// Correzione dal livello dei blocchi da 0,4 s (potenza media) e dal picco del brano.
fn gain_from_blocks(blocks: &[f64], peak: f32) -> Option<f32> {
    let db = |ms: f64| 10.0 * ms.max(1e-12).log10();
    let loud: Vec<f64> = blocks.iter().copied().filter(|&m| db(m) > -70.0).collect();
    if loud.is_empty() || peak <= 0.0 {
        return None;
    }
    let mean = loud.iter().sum::<f64>() / loud.len() as f64;
    let gate = db(mean) - 10.0;
    let kept: Vec<f64> = loud.into_iter().filter(|&m| db(m) > gate).collect();
    let level = db(kept.iter().sum::<f64>() / kept.len() as f64) as f32;
    let gain_db = (TARGET_DB - level).clamp(-MAX_CUT_DB, MAX_BOOST_DB);
    let gain = 10f32.powf(gain_db / 20.0);
    // alzare sì, ma non oltre il picco: il brano non deve distorcere
    Some(gain.min(0.99 / peak).max(10f32.powf(-MAX_CUT_DB / 20.0)))
}

/// Correzioni già misurate, salvate nella cartella dei dati: file → (data di modifica,
/// dimensione, correzione). Se il file cambia, si rimisura.
#[derive(Default)]
pub struct GainCache {
    path: Option<PathBuf>,
    map: std::collections::HashMap<String, (u64, u64, f32)>,
}

fn file_stamp(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((mtime, m.len()))
}

impl GainCache {
    pub fn load(path: Option<PathBuf>) -> Self {
        let map = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, map }
    }

    pub fn get(&self, file: &Path) -> Option<f32> {
        let (mtime, len, gain) = *self.map.get(&file.to_string_lossy().to_string())?;
        (file_stamp(file)? == (mtime, len)).then_some(gain)
    }

    pub fn insert(&mut self, file: &Path, gain: f32) {
        if let Some((mtime, len)) = file_stamp(file) {
            self.map
                .insert(file.to_string_lossy().into_owned(), (mtime, len, gain));
        }
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string(&self.map) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

/// Misura i brani in sottofondo, uno alla volta: prima quelli chiesti con `urgent`
/// (il brano in ascolto e il successivo), poi gli altri.
pub struct Analyzer {
    queue: Arc<(
        Mutex<std::collections::VecDeque<PathBuf>>,
        std::sync::Condvar,
    )>,
}

impl Analyzer {
    /// `done(file, correzione)` arriva dal thread di sottofondo.
    pub fn start(done: impl Fn(PathBuf, Option<f32>) + Send + 'static) -> Self {
        let queue: Arc<(
            Mutex<std::collections::VecDeque<PathBuf>>,
            std::sync::Condvar,
        )> = Default::default();
        let q = queue.clone();
        std::thread::spawn(move || loop {
            let next = {
                let (lock, cv) = &*q;
                let Ok(mut g) = lock.lock() else { return };
                while g.is_empty() {
                    g = match cv.wait(g) {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                }
                g.pop_front()
            };
            if let Some(p) = next {
                let gain = measure_gain(&p);
                done(p, gain);
            }
        });
        Self { queue }
    }

    pub fn urgent(&self, file: &Path) {
        self.push(file, true);
    }

    pub fn later(&self, file: &Path) {
        self.push(file, false);
    }

    fn push(&self, file: &Path, front: bool) {
        let (lock, cv) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            if let Some(i) = q.iter().position(|p| p == file) {
                if !front {
                    return;
                }
                q.remove(i);
            }
            if front {
                q.push_front(file.to_path_buf());
            } else {
                q.push_back(file.to_path_buf());
            }
            cv.notify_one();
        }
    }
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
    /// correzione per il volume uguale tra i brani (si moltiplica al volume)
    Gain(f32),
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
            let mut gain = 1.0f32;
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
                                p.set_volume(volume * gain);
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
                            p.set_volume(volume * gain);
                        }
                    }
                    Ok(Cmd::Gain(g)) => {
                        gain = g;
                        if let Some(p) = &player {
                            p.set_volume(volume * gain);
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
    /// Correzione del brano per il volume uguale (1 = nessuna).
    pub fn set_gain(&self, g: f32) {
        let _ = self.tx.send(Cmd::Gain(g.clamp(0.05, 2.0)));
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
    fn loud_and_quiet_tracks_get_opposite_corrections() {
        // brano forte (potenza media -8 dB): va abbassato di 8 dB
        let loud = vec![10f64.powf(-0.8); 100];
        let g = gain_from_blocks(&loud, 0.9).unwrap();
        assert!((20.0 * g.log10() - (-8.0)).abs() < 0.1, "{g}");
        // brano piano (-30 dB): si alzerebbe di 14, ma al massimo di 6 dB
        let quiet = vec![10f64.powf(-3.0); 100];
        let g = gain_from_blocks(&quiet, 0.1).unwrap();
        assert!((20.0 * g.log10() - MAX_BOOST_DB).abs() < 0.1, "{g}");
        // ...e mai oltre il picco: con picco 0,9 non si può alzare di 6 dB
        let g = gain_from_blocks(&quiet, 0.9).unwrap();
        assert!(g * 0.9 <= 1.0, "{g}");
        // i silenzi non contano; tutto silenzio: nessuna misura
        let mut with_silence = loud.clone();
        with_silence.extend(vec![0.0; 300]);
        let g2 = gain_from_blocks(&with_silence, 0.9).unwrap();
        assert!((g2 - gain_from_blocks(&loud, 0.9).unwrap()).abs() < 1e-4);
        assert_eq!(gain_from_blocks(&[0.0; 10], 0.0), None);
    }

    /// Un WAV vero: un'onda a metà volume si misura e si salva nell'archivio.
    #[test]
    fn a_real_file_is_measured_and_cached() {
        let dir = std::env::temp_dir().join(format!("grabbit-gain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("onda.wav");
        let rate = 8000u32;
        let mut data = Vec::new();
        for i in 0..rate * 2 {
            let x = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
            data.extend_from_slice(&((x * i16::MAX as f32) as i16).to_le_bytes());
        }
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
        std::fs::write(&wav, b).unwrap();
        // sinusoide di ampiezza 0,5: potenza media 0,125 = -9 dB → circa -7 dB di correzione
        let g = measure_gain(&wav).expect("misura");
        let db = 20.0 * g.log10();
        assert!((db - (-7.0)).abs() < 0.5, "{db}");
        let cache_file = dir.join("loudness.json");
        let mut cache = GainCache::load(Some(cache_file.clone()));
        cache.insert(&wav, g);
        let again = GainCache::load(Some(cache_file));
        assert_eq!(again.get(&wav), Some(g));
        let _ = std::fs::remove_dir_all(&dir);
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
            fresh: 0,
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
        // un video (qui riconosciuto dall'estensione) si elenca, segnato come video
        std::fs::write(album.join("4 - Il video.mkv"), b"x").unwrap();

        let lib = scan(&root, 64, &AtomicBool::new(false));
        assert_eq!(lib.unsupported, 1);
        assert_eq!(lib.albums.len(), 2);
        let a = lib
            .albums
            .iter()
            .find(|a| a.name == "Il mio album")
            .unwrap();
        let titles: Vec<_> = a.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Primo", "Secondo", "Il video"]);
        assert!(!a.tracks[0].video && a.tracks[2].video);
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

    /// MP3 minimo (qualche fotogramma di silenzio) con una copertina PNG 4×4 rossa nell'ID3.
    fn mp3_with_cover(path: &Path) {
        mp3(path, true, None);
    }

    /// MP3 minimo con, a scelta, la copertina e il titolo dell'album (TALB) nell'ID3.
    fn mp3(path: &Path, cover: bool, album: Option<&str>) {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        // fotogramma APIC (ID3v2.3): codifica, tipo MIME, tipo immagine, descrizione, dati
        let mut apic = vec![0u8];
        apic.extend_from_slice(b"image/png\0");
        apic.push(3);
        apic.push(0);
        apic.extend_from_slice(&png);
        let mut frames = Vec::new();
        let mut frame = |id: &[u8], body: &[u8]| {
            frames.extend_from_slice(id);
            frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(body);
        };
        if cover {
            frame(b"APIC", &apic);
        }
        if let Some(a) = album {
            let mut talb = vec![0u8];
            talb.extend_from_slice(a.as_bytes());
            frame(b"TALB", &talb);
        }
        let n = frames.len() as u32;
        let syncsafe = [
            ((n >> 21) & 0x7f) as u8,
            ((n >> 14) & 0x7f) as u8,
            ((n >> 7) & 0x7f) as u8,
            (n & 0x7f) as u8,
        ];
        let mut b = b"ID3\x03\x00\x00".to_vec();
        b.extend_from_slice(&syncsafe);
        b.extend_from_slice(&frames);
        // MPEG-1 Layer III, 128 kbit/s, 44,1 kHz: 417 byte a fotogramma, tutto silenzio
        for _ in 0..20 {
            b.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            b.extend_from_slice(&[0u8; 413]);
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn disc_folders_are_recognized() {
        assert_eq!(disc_folder("CD1"), Some(1));
        assert_eq!(disc_folder("CD 2"), Some(2));
        assert_eq!(disc_folder("Disc 3"), Some(3));
        assert_eq!(disc_folder("disco_1"), Some(1));
        assert_eq!(disc_folder("Bonus CD"), Some(99));
        assert_eq!(disc_folder("Bonus"), Some(99));
        assert_eq!(disc_folder("CD"), None);
        assert_eq!(disc_folder("21"), None);
        assert_eq!(disc_folder("Cdrom live"), None);
        assert_eq!(folder_year("2015 - 25"), Some(2015));
        assert_eq!(folder_year("2002.Tracce"), Some(2002));
        assert_eq!(folder_year("1999"), Some(1999));
        assert_eq!(folder_year("25"), None);
        assert_eq!(folder_year("2015abc"), None);
        assert_eq!(folder_year("5000 - Futuro"), None);
    }

    /// Nomi dai tag, dischi uniti nel loro album, copertina da «folder.jpg».
    #[test]
    fn albums_are_named_merged_and_get_folder_covers() {
        let root = std::env::temp_dir().join(format!("grabbit-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let artist = root.join("Adele");
        // nome della cartella scomposto, titolo vero nei tag
        let thirty = artist.join("Adele.-.30.(RTI).PBTHAL.(2021.-.Pop).[Flac.24-96.LP]-xpost");
        std::fs::create_dir_all(&thirty).unwrap();
        mp3(&thirty.join("1 - Strangers.mp3"), true, Some("30"));
        // due dischi e un bonus, senza copertina nei brani ma con folder.jpg nell'album
        let nineteen = artist.join("Adele-19-Deluxe.Edition-2CD-FLAC-2008-WRE");
        for (d, n) in [
            ("CD2", "1 - Secondo"),
            ("CD1", "1 - Primo"),
            ("Bonus CD", "1 - Extra"),
        ] {
            std::fs::create_dir_all(nineteen.join(d)).unwrap();
            mp3(
                &nineteen.join(d).join(format!("{n}.mp3")),
                false,
                Some("19"),
            );
        }
        image::RgbImage::from_pixel(8, 8, image::Rgb([20, 200, 40]))
            .save(nineteen.join("folder.jpg"))
            .unwrap();
        // senza titolo nei tag resta il nome della cartella
        let plain = artist.join("2015 - 25");
        std::fs::create_dir_all(&plain).unwrap();
        mp3(&plain.join("1 - Hello.mp3"), false, None);

        let lib = scan(&root, 16, &AtomicBool::new(false));
        let names: Vec<&str> = lib.albums.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(lib.albums.len(), 3, "{names:?}");
        assert!(names.contains(&"30") && names.contains(&"19") && names.contains(&"2015 - 25"));
        let a19 = lib.albums.iter().find(|a| a.name == "19").unwrap();
        let titles: Vec<&str> = a19.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Primo", "Secondo", "Extra"],
            "disco per disco, il bonus in fondo"
        );
        assert_eq!(a19.dir, nineteen);
        assert_eq!(a19.group, "Adele");
        let c = a19.cover.as_ref().expect("copertina da folder.jpg");
        assert!(c.2[1] > 150 && c.2[0] < 100, "verde: {:?}", &c.2[..4]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// La seconda scansione non riapre i file (stesso risultato, copertina dalla cache);
    /// un file cambiato si rilegge, uno cancellato sparisce.
    #[test]
    fn scan_is_incremental() {
        let base = std::env::temp_dir().join(format!("grabbit-inc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, cache) = (base.join("Musica"), base.join("cache"));
        let album = root.join("Adele").join("21");
        std::fs::create_dir_all(&album).unwrap();
        mp3_with_cover(&album.join("1 - Rolling.mp3"));
        mp3_with_cover(&album.join("2 - Rumour.mp3"));
        mp3_with_cover(&root.join("Sciolto.mp3"));
        let no = AtomicBool::new(false);

        let seen = std::cell::RefCell::new(Vec::new());
        let first = scan_cached(&root, 32, &no, Some(&cache), &|d, t| {
            seen.borrow_mut().push((d, t))
        });
        assert_eq!(first.fresh, 3);
        // l'avanzamento parte da 0 e arriva a tutti i brani
        let seen = seen.into_inner();
        assert_eq!(seen.first(), Some(&(0, 3)));
        assert_eq!(seen.last(), Some(&(3, 3)));
        let cover = |l: &Library| {
            l.albums
                .iter()
                .find(|a| a.name == "21")
                .and_then(|a| a.cover.clone())
        };
        let c1 = cover(&first).expect("copertina letta dall'MP3");
        assert_eq!(c1.2[..3], [200, 30, 30]);

        let second = scan_cached(&root, 32, &no, Some(&cache), &|_, _| {});
        assert_eq!(second.fresh, 0, "nessun file riaperto");
        assert_eq!(cover(&second).unwrap(), c1, "copertina uguale, dalla cache");
        let titles = |l: &Library| {
            let mut t: Vec<String> = l
                .albums
                .iter()
                .flat_map(|a| a.tracks.iter().map(|t| t.title.clone()))
                .collect();
            t.sort();
            t
        };
        assert_eq!(titles(&first), titles(&second));
        assert_eq!(second.albums.len(), first.albums.len());

        // un file riscritto si rilegge; uno cancellato sparisce
        std::thread::sleep(Duration::from_millis(20));
        mp3_with_cover(&album.join("2 - Rumour.mp3"));
        std::fs::remove_file(root.join("Sciolto.mp3")).unwrap();
        let third = scan_cached(&root, 32, &no, Some(&cache), &|_, _| {});
        assert!(
            third.fresh <= 1,
            "solo il file cambiato, visti {}",
            third.fresh
        );
        assert_eq!(third.albums.len(), 1);
        assert_eq!(third.albums[0].tracks.len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }
}
