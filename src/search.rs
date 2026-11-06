// SPDX-License-Identifier: GPL-3.0-or-later
//! Ricerca su YouTube e YouTube Music tramite yt-dlp, senza scaricare nulla:
//! restituisce titolo, canale/artista, durata e link dei primi risultati.
//!
//! - YouTube: `ytsearchN:testo`
//! - YouTube Music: `https://music.youtube.com/search?q=testo#songs` (o `#albums`,
//!   `#videos`): la sezione dopo `#` è quella che yt-dlp legge dalla pagina dei risultati.
use crate::downloader::{terminate, Toolset};
use crate::tools;
use crate::tr;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Quanti risultati chiedere a YouTube.
pub const RESULTS: usize = 15;
const TIMEOUT: Duration = Duration::from_secs(60);

/// Dove cercare.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    #[default]
    YouTube,
    /// discografia di un artista: tutti i suoi album e singoli
    MusicArtist,
    MusicSongs,
    MusicAlbums,
    MusicVideos,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::YouTube,
        Source::MusicArtist,
        Source::MusicSongs,
        Source::MusicAlbums,
        Source::MusicVideos,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Source::YouTube => "YouTube",
            Source::MusicArtist => tr!(
                "YouTube Music · artista (discografia)",
                "YouTube Music · artist (discography)"
            ),
            Source::MusicSongs => tr!("YouTube Music · brani", "YouTube Music · songs"),
            Source::MusicAlbums => tr!("YouTube Music · album", "YouTube Music · albums"),
            Source::MusicVideos => tr!("YouTube Music · video", "YouTube Music · videos"),
        }
    }

    /// Cosa passare a yt-dlp per cercare `query`.
    pub fn target(self, query: &str, count: usize) -> String {
        let q = query.trim();
        let section = match self {
            Source::YouTube => return format!("ytsearch{count}:{q}"),
            Source::MusicArtist => "artists",
            Source::MusicSongs => "songs",
            Source::MusicAlbums => "albums",
            Source::MusicVideos => "videos",
        };
        format!(
            "https://music.youtube.com/search?q={}#{section}",
            encode_query(q)
        )
    }
}

/// Codifica il testo per l'indirizzo di ricerca (spazi come `+`, il resto in %XX).
fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub channel: String,
    /// secondi
    pub duration: Option<f64>,
    pub views: Option<u64>,
    /// link da scaricare (pagina del video, oppure dell'album/playlist)
    pub url: String,
    /// album o playlist: si scarica tutto, in una cartella con il suo nome
    pub collection: bool,
    /// indirizzo della copertina (miniatura del video o copertina dell'album)
    pub thumbnail: Option<String>,
    /// numero di brani (album e playlist, quando lo si conosce)
    pub tracks: Option<u64>,
    /// album di YouTube Music arrivato senza titolo o copertina: i dati vanno chiesti
    /// a parte con `fetch_details`
    pub needs_details: bool,
    /// tipo di uscita come lo scrive YouTube Music («Album», «Singolo», «EP»)
    pub kind: Option<String>,
    /// anno di uscita
    pub year: Option<String>,
}

pub fn build_args(
    source: Source,
    query: &str,
    count: usize,
    tools: &Toolset,
    cookies_browser: &str,
) -> Vec<String> {
    // la pagina dei risultati di YouTube Music non ha un limite: lo mette yt-dlp
    let limit = (source != Source::YouTube).then_some(count);
    list_args(&source.target(query, count), limit, tools, cookies_browser)
}

/// Opzioni per far elencare a yt-dlp (senza scaricare) i primi `limit` elementi di
/// `target`: una ricerca o una pagina che contiene un elenco.
fn list_args(
    target: &str,
    limit: Option<usize>,
    tools: &Toolset,
    cookies_browser: &str,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "--flat-playlist",
        "--skip-download",
        "--no-warnings",
        "--ignore-errors",
        "--color",
        "never",
        "--print",
        "%(.{id,title,channel,uploader,artist,artists,creator,duration,view_count,url,webpage_url,ie_key,_type,thumbnail,thumbnails,playlist_title,playlist_uploader,playlist_channel})j",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(js) = &tools.js {
        a.push("--js-runtimes".into());
        a.push(js.arg());
    }
    if !cookies_browser.is_empty() {
        a.push("--cookies-from-browser".into());
        a.push(cookies_browser.into());
    }
    if let Some(n) = limit {
        a.push("--playlist-items".into());
        a.push(format!("1:{n}"));
    }
    // "--": una ricerca che comincia con "-" non diventa un'opzione di yt-dlp
    a.push("--".into());
    a.push(target.into());
    a
}

/// Legge una riga di `--print "%(.{…})j"`: un oggetto JSON per risultato.
/// Si usa `Value` e non una struttura rigida: i campi cambiano tra YouTube e YouTube Music
/// (e tra versioni di yt-dlp), e un campo di tipo inatteso non deve far sparire il risultato.
pub fn parse_line(line: &str) -> Option<SearchResult> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "NA")
            .map(String::from)
    };
    let http = |k: &str| text(k).filter(|u| u.starts_with("http"));
    let url = http("webpage_url").or_else(|| http("url")).or_else(|| {
        text("id")
            .filter(|id| id.len() == 11) // id di un video YouTube
            .map(|id| format!("https://www.youtube.com/watch?v={id}"))
    })?;
    let has_title = text("title").is_some();
    let title = text("title").unwrap_or_else(|| url.clone());
    let artists = v
        .get("artists")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty());
    let channel = artists
        .or_else(|| text("artist"))
        .or_else(|| text("creator"))
        .or_else(|| text("channel"))
        .or_else(|| text("uploader"))
        .unwrap_or_default();
    let collection = text("_type").is_some_and(|t| t == "playlist")
        || text("ie_key").is_some_and(|k| k.contains("Tab") || k.contains("Playlist"))
        || is_collection_url(&url);
    let thumbnail = pick_thumbnail(&v, &url);
    Some(SearchResult {
        title,
        channel,
        duration: v
            .get("duration")
            .and_then(Value::as_f64)
            .filter(|d| *d > 0.0),
        views: v
            .get("view_count")
            .and_then(|n| n.as_u64().or_else(|| n.as_f64().map(|f| f as u64))),
        url,
        collection,
        needs_details: collection && (!has_title || thumbnail.is_none()),
        thumbnail,
        tracks: None,
        kind: None,
        year: None,
    })
}

/// Larghezza ideale della copertina da scaricare (in pixel): basta per la lista dei
/// risultati anche su schermi Retina, senza scaricare immagini enormi.
const THUMB_WANTED: i64 = 240;

/// Sceglie la copertina più adatta: la più piccola larga almeno `THUMB_WANTED`, altrimenti la
/// più grande disponibile; per i video YouTube senza elenco usa la miniatura standard.
fn pick_thumbnail(v: &Value, url: &str) -> Option<String> {
    let ok = |u: &str| u.starts_with("https://") || u.starts_with("http://");
    if let Some(list) = v.get("thumbnails").and_then(Value::as_array) {
        let mut sized: Vec<(i64, String)> = list
            .iter()
            .filter_map(|t| {
                let u = t.get("url")?.as_str()?.to_string();
                ok(&u).then(|| (t.get("width").and_then(Value::as_i64).unwrap_or(0), u))
            })
            .collect();
        sized.sort_by_key(|(w, _)| *w);
        if let Some((_, u)) = sized
            .iter()
            .find(|(w, _)| *w >= THUMB_WANTED)
            .or(sized.last())
        {
            return Some(u.clone());
        }
    }
    if let Some(u) = v.get("thumbnail").and_then(Value::as_str).filter(|u| ok(u)) {
        return Some(u.to_string());
    }
    video_id(url).map(|id| format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"))
}

/// Id del video in un link `…watch?v=ID…` (YouTube o YouTube Music).
fn video_id(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("v=")?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (id.len() == 11).then_some(id)
}

/// Scarica e rimpicciolisce una copertina, pronta per diventare una texture.
/// Restituisce larghezza, altezza e pixel RGBA.
pub fn fetch_thumbnail(url: &str, max_w: u32, max_h: u32) -> Result<(u32, u32, Vec<u8>)> {
    let bytes = crate::net::get_bytes(url, 4 * 1024 * 1024)?;
    decode_thumbnail(&bytes, max_w, max_h)
}

/// Copertina incorporata in un brano (JPEG o PNG), rimpicciolita a `px` di lato al massimo.
pub fn decode_cover(bytes: &[u8], px: u32) -> Result<(u32, u32, Vec<u8>)> {
    decode_thumbnail(bytes, px, px)
}

fn decode_thumbnail(bytes: &[u8], max_w: u32, max_h: u32) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes)?;
    let img = img.thumbnail(max_w, max_h).to_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

/// Album, playlist o pagina di un canale (non un singolo video).
fn is_collection_url(url: &str) -> bool {
    let u = url.to_lowercase();
    (u.contains("list=") && !u.contains("watch?"))
        || u.contains("/browse/")
        || u.contains("/playlist")
}

/// Esegue la ricerca (bloccante: va chiamata da un thread a parte).
/// Esito di una ricerca.
#[derive(Debug, Default)]
pub struct Outcome {
    pub results: Vec<SearchResult>,
    /// ricerca per artista: il nome dell'artista trovato
    pub artist: Option<String>,
}

pub fn search(
    source: Source,
    query: &str,
    tools: &Toolset,
    cookies_browser: &str,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    if source == Source::MusicArtist {
        return search_artist(query, tools, cookies_browser, cancel);
    }
    let args = build_args(source, query, RESULTS, tools, cookies_browser);
    let (out, err) = run_ytdlp(tools, &args, cancel, TIMEOUT)?;
    let results: Vec<SearchResult> = out.lines().filter_map(parse_line).collect();
    if results.is_empty() {
        if let Some(e) = last_error(&err) {
            bail!(e);
        }
    }
    Ok(Outcome {
        results,
        artist: None,
    })
}

/// Quante uscite (album e singoli) di un artista elencare al massimo.
const RELEASES: usize = 100;

/// Discografia di un artista, come la pagina «Album» dell'artista su YouTube Music:
/// 1) si cerca l'artista su YouTube Music (con yt-dlp) e si prende il primo, il più
///    pertinente: il suo id di canale (UC…);
/// 2) si chiede a YouTube Music la pagina dei suoi album (`browse/MPADUC…`, la pagina
///    «Album → Altro» del sito); se non c'è (artisti con pochi album) la pagina
///    dell'artista, dove album e singoli sono in vista; in ultimo la scheda «Uscite»
///    del canale YouTube.
fn search_artist(
    query: &str,
    tools: &Toolset,
    cookies_browser: &str,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let args = list_args(
        &Source::MusicArtist.target(query, 5),
        Some(5),
        tools,
        cookies_browser,
    );
    let (out, err) = run_ytdlp(tools, &args, cancel, TIMEOUT)?;
    let Some(channel) = out.lines().find_map(channel_id_in) else {
        if let Some(e) = last_error(&err) {
            bail!(e);
        }
        return Ok(Outcome::default());
    };
    let mut errors = Vec::new();
    for browse_id in [format!("MPAD{channel}"), channel.clone()] {
        if cancel.load(Ordering::Relaxed) {
            bail!(tr!("Ricerca annullata", "Search cancelled"));
        }
        match ytmusic_browse(&browse_id) {
            Ok(page) => {
                let results = ytmusic_releases(&page);
                if !results.is_empty() {
                    let artist = page_title(&page).or_else(|| Some(query.trim().to_string()));
                    return Ok(Outcome { results, artist });
                }
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    match releases_tab(&channel, tools, cookies_browser, cancel) {
        Ok(o) if !o.results.is_empty() => Ok(o),
        Ok(o) if errors.is_empty() => Ok(o),
        Ok(_) => bail!(errors.join("; ")),
        Err(e) => {
            errors.push(format!("{e:#}"));
            bail!(errors.join("; "))
        }
    }
}

/// Chiede a YouTube Music una pagina (`browse`) come fa il suo sito, senza account.
fn ytmusic_browse(browse_id: &str) -> Result<Value> {
    let hl = if crate::i18n::is_italian() {
        "it"
    } else {
        "en"
    };
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": YTMUSIC_CLIENT_VERSION,
                "hl": hl,
            }
        },
        "browseId": browse_id,
    });
    crate::net::post_json(
        "https://music.youtube.com/youtubei/v1/browse?prettyPrint=false",
        &[
            ("Origin", "https://music.youtube.com"),
            ("Referer", "https://music.youtube.com/"),
            ("X-YouTube-Client-Name", "67"),
            ("X-YouTube-Client-Version", YTMUSIC_CLIENT_VERSION),
        ],
        &body,
    )
}

const YTMUSIC_CLIENT_VERSION: &str = "1.20251027.01.00";

/// Tutti gli elementi di tipo `key` (a qualsiasi profondità) di una risposta di YouTube.
fn collect<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, c) in m {
                if k == key {
                    out.push(c);
                } else {
                    collect(c, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|c| collect(c, key, out)),
        _ => {}
    }
}

fn runs_text(v: Option<&Value>) -> Vec<String> {
    v.and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|runs| {
            runs.iter()
                .filter_map(|r| r.get("text").and_then(Value::as_str))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "•")
                .collect()
        })
        .unwrap_or_default()
}

/// Album e singoli (riquadri con un id «MPREb_…») di una pagina di YouTube Music.
fn ytmusic_releases(page: &Value) -> Vec<SearchResult> {
    let mut items = Vec::new();
    collect(page, "musicTwoRowItemRenderer", &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(|it| {
            let mut ids = Vec::new();
            collect(it, "browseId", &mut ids);
            let id = ids
                .into_iter()
                .filter_map(Value::as_str)
                .find(|s| s.starts_with("MPREb"))?
                .to_string();
            if !seen.insert(id.clone()) {
                return None; // lo stesso album in due sezioni della pagina
            }
            let title = runs_text(it.get("title")).join(" ");
            let subtitle = runs_text(it.get("subtitle"));
            let year = subtitle
                .iter()
                .find(|s| s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()))
                .cloned();
            let kind = subtitle
                .first()
                .filter(|s| Some(*s) != year.as_ref())
                .cloned();
            let mut thumbs = Vec::new();
            collect(it, "thumbnails", &mut thumbs);
            let url = format!("https://music.youtube.com/browse/{id}");
            let thumbnail = thumbs
                .first()
                .and_then(|t| pick_thumbnail(&serde_json::json!({ "thumbnails": t }), &url));
            Some(SearchResult {
                needs_details: title.is_empty() || thumbnail.is_none(),
                title: if title.is_empty() { url.clone() } else { title },
                channel: String::new(),
                duration: None,
                views: None,
                url,
                collection: true,
                thumbnail,
                tracks: None,
                kind,
                year,
            })
        })
        .collect()
}

/// Titolo della pagina (il nome dell'artista) dall'intestazione.
fn page_title(page: &Value) -> Option<String> {
    let header = page.get("header")?;
    let mut titles = Vec::new();
    collect(header, "title", &mut titles);
    titles
        .into_iter()
        .map(|t| runs_text(Some(t)).join(" "))
        .find(|s| !s.is_empty())
}

/// Ultima possibilità: la scheda «Uscite» del canale YouTube (non tutti i canali la hanno).
fn releases_tab(
    channel: &str,
    tools: &Toolset,
    cookies_browser: &str,
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let args = list_args(
        &format!("https://www.youtube.com/channel/{channel}/releases"),
        Some(RELEASES),
        tools,
        cookies_browser,
    );
    let (out, err) = run_ytdlp(tools, &args, cancel, Duration::from_secs(90))?;
    let mut artist = None;
    let mut results = Vec::new();
    for line in out.lines() {
        if artist.is_none() {
            artist = artist_name(line);
        }
        if let Some(mut r) = parse_line(line) {
            // ogni uscita è un album o un singolo: titolo, numero di brani e copertina
            // si completano a parte, come per gli album di YouTube Music
            r.collection = true;
            r.needs_details = true;
            results.push(r);
        }
    }
    if results.is_empty() {
        if let Some(e) = last_error(&err) {
            bail!(e);
        }
    }
    Ok(Outcome { results, artist })
}

/// Id del canale (UC + 22 caratteri) contenuto nell'indirizzo o nell'id di una riga di yt-dlp.
fn channel_id_in(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    ["url", "webpage_url", "id", "channel_id"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .find_map(find_channel_id)
}

fn find_channel_id(s: &str) -> Option<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    s.match_indices("UC").find_map(|(i, _)| {
        if s[..i].chars().next_back().is_some_and(ok) {
            return None; // «UC» in mezzo a un'altra parola
        }
        let id: String = s[i..].chars().take_while(|c| ok(*c)).collect();
        (id.len() == 24).then_some(id)
    })
}

/// Nome dell'artista dalle informazioni della pagina che contiene l'elenco.
fn artist_name(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    [
        "playlist_uploader",
        "playlist_channel",
        "channel",
        "uploader",
    ]
    .iter()
    .filter_map(|k| v.get(*k).and_then(Value::as_str))
    .map(clean_name)
    .find(|s| !s.is_empty() && s != "NA")
}

fn last_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .find(|l| l.starts_with("ERROR"))
        .map(|l| l.trim_start_matches("ERROR:").trim().to_string())
}

/// Esegue yt-dlp e ne restituisce stdout e stderr, con annullamento e tempo massimo.
fn run_ytdlp(
    tools: &Toolset,
    args: &[String],
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(String, String)> {
    let mut child = tools::command(&tools.ytdlp)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            anyhow!(tr!(
                format!("impossibile avviare yt-dlp: {e}"),
                format!("could not start yt-dlp: {e}")
            ))
        })?;
    // stdout e stderr letti in thread a parte: se si riempiono, yt-dlp si blocca
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            terminate(&mut child);
            let _ = child.wait();
            bail!(tr!("Ricerca annullata", "Search cancelled"));
        }
        if started.elapsed() > timeout {
            terminate(&mut child);
            let _ = child.wait();
            bail!(tr!(
                "la ricerca ha impiegato troppo tempo",
                "the search took too long"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok((
        out_t.join().unwrap_or_default(),
        err_t.join().unwrap_or_default(),
    ))
}

/// Dati di un album (o playlist) letti dalla sua pagina.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub tracks: Option<u64>,
    pub thumbnail: Option<String>,
}

pub fn details_args(url: &str, tools: &Toolset, cookies_browser: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "--flat-playlist",
        "--skip-download",
        "--no-warnings",
        "--ignore-errors",
        "--color",
        "never",
        // basta il primo brano: i dati dell'album arrivano con lui (campi playlist_*)
        "--playlist-items",
        "1",
        "--print",
        "%(.{playlist_title,playlist_uploader,playlist_channel,playlist_count,n_entries,id,url,thumbnails,artists,artist,channel,uploader})j",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(js) = &tools.js {
        a.push("--js-runtimes".into());
        a.push(js.arg());
    }
    if !cookies_browser.is_empty() {
        a.push("--cookies-from-browser".into());
        a.push(cookies_browser.into());
    }
    a.push("--".into());
    a.push(url.into());
    a
}

/// Toglie le aggiunte di YouTube ai nomi: «Album - X» → «X», «Artista - Topic» → «Artista».
pub(crate) fn clean_name(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("Album - ")
        .or_else(|| s.strip_prefix("Single - "))
        .or_else(|| s.strip_prefix("EP - "))
        .unwrap_or(s);
    s.strip_suffix(" - Topic").unwrap_or(s).trim().to_string()
}

pub fn parse_details(line: &str) -> Option<Details> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(clean_name)
            .filter(|s| !s.is_empty() && s != "NA")
    };
    let artists = v
        .get("artists")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(clean_name);
    let count = |k: &str| v.get(k).and_then(Value::as_u64).filter(|n| *n > 0);
    // copertina: quella del primo brano (per un album è la copertina dell'album)
    let first_url = text("url").unwrap_or_default();
    let thumbnail = pick_thumbnail(&v, &first_url).or_else(|| {
        text("id")
            .filter(|id| id.len() == 11)
            .map(|id| format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"))
    });
    Some(Details {
        title: text("playlist_title"),
        artist: text("playlist_uploader")
            .or_else(|| text("playlist_channel"))
            .or(artists)
            .or_else(|| text("artist"))
            .or_else(|| text("channel"))
            .or_else(|| text("uploader")),
        tracks: count("playlist_count").or_else(|| count("n_entries")),
        thumbnail,
    })
}

/// Chiede a yt-dlp titolo, artista, numero di brani e copertina di un album.
pub fn fetch_details(
    url: &str,
    tools: &Toolset,
    cookies_browser: &str,
    cancel: &AtomicBool,
) -> Result<Details> {
    let args = details_args(url, tools, cookies_browser);
    let (out, err) = run_ytdlp(tools, &args, cancel, Duration::from_secs(45))?;
    match out.lines().find_map(parse_details) {
        Some(d) => Ok(d),
        None => bail!(last_error(&err).unwrap_or_else(|| "nessun dato".into())),
    }
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn fmt_views(v: u64) -> String {
    let n = if v >= 1_000_000_000 {
        format!("{:.1} Mrd", v as f64 / 1e9)
    } else if v >= 1_000_000 {
        format!("{:.1} Mln", v as f64 / 1e6)
    } else if v >= 1_000 {
        format!("{:.0} mila", v as f64 / 1e3)
    } else {
        v.to_string()
    };
    if crate::i18n::is_italian() {
        format!("{n} visualizzazioni")
    } else {
        let n = n
            .replace(" Mrd", "B")
            .replace(" Mln", "M")
            .replace(" mila", "K");
        format!("{n} views")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolset() -> Toolset {
        Toolset {
            ytdlp: "yt-dlp".into(),
            ffmpeg_dir: None,
            js: Some(crate::downloader::JsRuntime::deno("/dn/deno".into())),
        }
    }

    #[test]
    fn args_end_with_the_search_after_double_dash() {
        let a = build_args(
            Source::YouTube,
            "  -Male Fantasy ",
            15,
            &toolset(),
            "firefox",
        );
        let n = a.len();
        assert_eq!(a[n - 2], "--");
        assert_eq!(a[n - 1], "ytsearch15:-Male Fantasy");
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--cookies-from-browser" && w[1] == "firefox"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--js-runtimes" && w[1] == "deno:/dn/deno"));
        assert!(a.contains(&"--flat-playlist".to_string()));
    }

    #[test]
    fn parses_flat_youtube_entry() {
        let l = r#"{"id": "abc123", "title": "Male Fantasy", "channel": "Billie Eilish", "uploader": null, "duration": 194.0, "view_count": 1234567, "url": "https://www.youtube.com/watch?v=abc123", "webpage_url": null}"#;
        let r = parse_line(l).unwrap();
        assert_eq!(r.title, "Male Fantasy");
        assert_eq!(r.channel, "Billie Eilish");
        assert_eq!(r.url, "https://www.youtube.com/watch?v=abc123");
        assert_eq!(fmt_duration(r.duration.unwrap()), "3:14");
        assert_eq!(r.views, Some(1234567));
    }

    #[test]
    fn falls_back_to_id_and_uploader() {
        let l = r#"{"id": "dQw4w9WgXcQ", "title": "", "channel": null, "uploader": "Qualcuno", "duration": null, "view_count": null, "url": "dQw4w9WgXcQ", "webpage_url": null}"#;
        let r = parse_line(l).unwrap();
        assert_eq!(r.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(r.title, r.url);
        assert_eq!(r.channel, "Qualcuno");
        assert_eq!(r.duration, None);
    }

    #[test]
    fn music_targets() {
        assert_eq!(
            Source::MusicSongs.target(" Billie Eilish & Male Fantasy? ", 15),
            "https://music.youtube.com/search?q=Billie+Eilish+%26+Male+Fantasy%3F#songs"
        );
        assert_eq!(
            Source::MusicAlbums.target("perché", 15),
            "https://music.youtube.com/search?q=perch%C3%A9#albums"
        );
        let a = build_args(Source::MusicSongs, "x", 15, &toolset(), "");
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--playlist-items" && w[1] == "1:15"));
        assert_eq!(
            a.last().unwrap(),
            "https://music.youtube.com/search?q=x#songs"
        );
        let a = build_args(Source::YouTube, "x", 15, &toolset(), "");
        assert!(!a.contains(&"--playlist-items".to_string()));
    }

    #[test]
    fn parses_music_song_and_album() {
        // brano: artisti come elenco, durata intera
        let l = r#"{"id": "abcdefghijk", "title": "Male Fantasy", "artists": ["Billie Eilish"], "duration": 194, "url": "https://music.youtube.com/watch?v=abcdefghijk", "ie_key": "Youtube", "_type": "url"}"#;
        let r = parse_line(l).unwrap();
        assert_eq!(r.channel, "Billie Eilish");
        assert_eq!(r.duration, Some(194.0));
        assert!(!r.collection);
        assert_eq!(r.url, "https://music.youtube.com/watch?v=abcdefghijk");
        // album: si scarica tutto
        let l = r#"{"id": "MPREb_xyz", "title": "Hit Me Hard and Soft", "url": "https://music.youtube.com/browse/MPREb_xyz", "ie_key": "YoutubeTab", "_type": "url"}"#;
        let r = parse_line(l).unwrap();
        assert!(r.collection);
        assert_eq!(r.channel, "");
        // un campo di tipo inatteso non fa perdere il risultato
        let l = r#"{"title": "X", "artists": "non è un elenco", "duration": "?", "url": "https://music.youtube.com/watch?v=abcdefghijk"}"#;
        assert!(parse_line(l).is_some());
    }

    #[test]
    fn picks_a_reasonable_thumbnail() {
        let l = r#"{"id": "abcdefghijk", "title": "X", "url": "https://www.youtube.com/watch?v=abcdefghijk", "thumbnails": [
            {"url": "https://i.ytimg.com/vi/abcdefghijk/hq720.jpg", "width": 720, "height": 404},
            {"url": "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg", "width": 360, "height": 202},
            {"url": "https://i.ytimg.com/vi/abcdefghijk/default.jpg", "width": 120, "height": 90}]}"#;
        assert_eq!(
            parse_line(l).unwrap().thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg")
        );
        // tutte piccole: la più grande
        let l = r#"{"title": "Album", "url": "https://music.youtube.com/browse/MPREb_x", "thumbnails": [
            {"url": "https://lh3.googleusercontent.com/a=w60-h60", "width": 60},
            {"url": "https://lh3.googleusercontent.com/a=w120-h120", "width": 120}]}"#;
        assert_eq!(
            parse_line(l).unwrap().thumbnail.as_deref(),
            Some("https://lh3.googleusercontent.com/a=w120-h120")
        );
        // nessuna copertina nei dati: miniatura standard ricavata dall'id del video
        let l = r#"{"title": "Brano", "url": "https://music.youtube.com/watch?v=abcdefghijk&list=RDAMVM"}"#;
        assert_eq!(
            parse_line(l).unwrap().thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/abcdefghijk/mqdefault.jpg")
        );
        // album senza copertina: niente
        let l = r#"{"title": "Album", "url": "https://music.youtube.com/browse/MPREb_x"}"#;
        assert_eq!(parse_line(l).unwrap().thumbnail, None);
    }

    #[test]
    fn decodes_and_shrinks_jpeg_covers() {
        let mut jpg = Vec::new();
        image::RgbImage::from_pixel(640, 360, image::Rgb([200, 80, 20]))
            .write_to(
                &mut std::io::Cursor::new(&mut jpg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        // 16:9 nel riquadro 16:9 (Retina: 224x126)
        let (w, h, px) = decode_thumbnail(&jpg, 224, 126).unwrap();
        assert_eq!((w, h), (224, 126));
        assert_eq!(px.len(), (w * h * 4) as usize);
        // copertina quadrata: resta quadrata
        let mut sq = Vec::new();
        image::RgbImage::from_pixel(544, 544, image::Rgb([1, 2, 3]))
            .write_to(&mut std::io::Cursor::new(&mut sq), image::ImageFormat::Jpeg)
            .unwrap();
        let (w, h, _) = decode_thumbnail(&sq, 224, 126).unwrap();
        assert_eq!((w, h), (126, 126));
        assert!(decode_thumbnail(b"non sono un'immagine", 10, 10).is_err());
    }

    #[test]
    fn album_without_title_needs_details() {
        let l = r#"{"id": "MPREb_JpXW8CzJwCB", "url": "https://music.youtube.com/browse/MPREb_JpXW8CzJwCB", "ie_key": "YoutubeTab", "_type": "url"}"#;
        let r = parse_line(l).unwrap();
        assert!(r.collection && r.needs_details);
        // un brano non ne ha bisogno
        let l =
            r#"{"title": "Male Fantasy", "url": "https://music.youtube.com/watch?v=abcdefghijk"}"#;
        assert!(!parse_line(l).unwrap().needs_details);
    }

    #[test]
    fn parses_album_details() {
        let l = r#"{"playlist_title": "Album - Vivere a colori", "playlist_uploader": "Alessandra Amoroso", "playlist_count": 12, "id": "abcdefghijk", "url": "https://music.youtube.com/watch?v=abcdefghijk", "thumbnails": [{"url": "https://lh3.googleusercontent.com/x=w544-h544", "width": 544}], "channel": "Alessandra Amoroso - Topic"}"#;
        let d = parse_details(l).unwrap();
        assert_eq!(d.title.as_deref(), Some("Vivere a colori"));
        assert_eq!(d.artist.as_deref(), Some("Alessandra Amoroso"));
        assert_eq!(d.tracks, Some(12));
        assert_eq!(
            d.thumbnail.as_deref(),
            Some("https://lh3.googleusercontent.com/x=w544-h544")
        );
        // dati minimi: artista dal canale «- Topic», copertina dall'id del primo brano
        let l = r#"{"playlist_title": "Senza Nuvole", "id": "abcdefghijk", "channel": "Alessandra Amoroso - Topic", "n_entries": 10}"#;
        let d = parse_details(l).unwrap();
        assert_eq!(d.artist.as_deref(), Some("Alessandra Amoroso"));
        assert_eq!(d.tracks, Some(10));
        assert_eq!(
            d.thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/abcdefghijk/mqdefault.jpg")
        );
        let a = details_args("https://music.youtube.com/browse/MPREb_x", &toolset(), "");
        assert_eq!(
            a.last().unwrap(),
            "https://music.youtube.com/browse/MPREb_x"
        );
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--playlist-items" && w[1] == "1"));
    }

    #[test]
    fn artist_search_finds_the_channel_and_its_releases() {
        assert_eq!(
            Source::MusicArtist.target("Francesco Renga", 5),
            "https://music.youtube.com/search?q=Francesco+Renga#artists"
        );
        // l'artista trovato su YouTube Music: …/browse/UC…
        let l = r#"{"id": "UCk3xZ7ODRSczVq3hsQXlGkg", "url": "https://music.youtube.com/browse/UCk3xZ7ODRSczVq3hsQXlGkg", "ie_key": "YoutubeTab"}"#;
        assert_eq!(
            channel_id_in(l).as_deref(),
            Some("UCk3xZ7ODRSczVq3hsQXlGkg")
        );
        assert_eq!(
            channel_id_in(r#"{"url": "https://music.youtube.com/browse/MPREb_x"}"#),
            None
        );
        assert_eq!(find_channel_id("xUCk3xZ7ODRSczVq3hsQXlGkg"), None);
        // un'uscita della scheda «Uscite» del canale
        let l = r#"{"id": "OLAK5uy_abc", "title": "Tracce", "url": "https://www.youtube.com/playlist?list=OLAK5uy_abc", "_type": "url", "playlist_uploader": "Francesco Renga - Topic", "playlist_title": "Francesco Renga - Releases"}"#;
        assert_eq!(artist_name(l).as_deref(), Some("Francesco Renga"));
        let r = parse_line(l).unwrap();
        assert!(r.collection);
        assert_eq!(r.title, "Tracce");
        let a = list_args(
            "https://www.youtube.com/channel/UCk3xZ7ODRSczVq3hsQXlGkg/releases",
            Some(100),
            &toolset(),
            "",
        );
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--playlist-items" && w[1] == "1:100"));
        assert_eq!(
            a.last().unwrap(),
            "https://www.youtube.com/channel/UCk3xZ7ODRSczVq3hsQXlGkg/releases"
        );
    }

    #[test]
    fn reads_the_albums_page_of_youtube_music() {
        // forma della risposta di music.youtube.com/youtubei/v1/browse per «MPADUC…»
        let page: Value = serde_json::from_str(
            r#"{
            "header": {"musicHeaderRenderer": {"title": {"runs": [{"text": "Francesco Renga"}]}}},
            "contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {
              "sectionListRenderer": {"contents": [{"gridRenderer": {"items": [
                {"musicTwoRowItemRenderer": {
                  "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                    {"url": "https://lh3.googleusercontent.com/a=w226-h226", "width": 226, "height": 226},
                    {"url": "https://lh3.googleusercontent.com/a=w544-h544", "width": 544, "height": 544}]}}},
                  "title": {"runs": [{"text": "L'altra metà", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_altrameta"}}}]},
                  "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "2019"}]},
                  "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_altrameta"}}}},
                {"musicTwoRowItemRenderer": {
                  "title": {"runs": [{"text": "Il meglio di me"}]},
                  "subtitle": {"runs": [{"text": "Singolo"}, {"text": " • "}, {"text": "2026"}]},
                  "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_meglio"}}}},
                {"musicTwoRowItemRenderer": {
                  "title": {"runs": [{"text": "Un video"}]},
                  "navigationEndpoint": {"watchEndpoint": {"videoId": "abcdefghijk"}}}}
              ]}}]}}}}]}}
            }"#,
        )
        .unwrap();
        assert_eq!(page_title(&page).as_deref(), Some("Francesco Renga"));
        let r = ytmusic_releases(&page);
        assert_eq!(r.len(), 2, "il video non è un'uscita");
        assert_eq!(r[0].title, "L'altra metà");
        assert_eq!(r[0].url, "https://music.youtube.com/browse/MPREb_altrameta");
        assert_eq!(r[0].kind.as_deref(), Some("Album"));
        assert_eq!(r[0].year.as_deref(), Some("2019"));
        assert_eq!(
            r[0].thumbnail.as_deref(),
            Some("https://lh3.googleusercontent.com/a=w544-h544")
        );
        assert!(r[0].collection && !r[0].needs_details);
        // senza copertina nei dati: va completato a parte
        assert_eq!(r[1].kind.as_deref(), Some("Singolo"));
        assert!(r[1].needs_details);
    }

    #[test]
    fn ignores_garbage_lines() {
        assert!(parse_line("[youtube:search] query").is_none());
        assert!(parse_line(r#"{"id": null, "url": null}"#).is_none());
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(59.4), "0:59");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
    }

    /// Con un vero yt-dlp, verifica che accetti le opzioni e che il formato di stampa
    /// produca righe leggibili (su un file locale, senza YouTube):
    /// GRABBIT_YTDLP=... GRABBIT_TEST_URL=http://127.0.0.1:8765/clip.mp4 cargo test search_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn search_real_ytdlp_print_format() {
        let (Ok(bin), Ok(url)) = (
            std::env::var("GRABBIT_YTDLP"),
            std::env::var("GRABBIT_TEST_URL"),
        ) else {
            return;
        };
        let mut a = build_args(Source::YouTube, "x", 1, &toolset(), "");
        a.retain(|x| !x.starts_with("--js-runtimes") && !x.starts_with("deno:"));
        let n = a.len();
        a[n - 1] = url;
        let out = std::process::Command::new(&bin).args(&a).output().unwrap();
        let so = String::from_utf8_lossy(&out.stdout);
        println!("{so}\n{}", String::from_utf8_lossy(&out.stderr));
        let r: Vec<_> = so.lines().filter_map(parse_line).collect();
        assert_eq!(r.len(), 1, "riga non riconosciuta: {so}");
    }
}
