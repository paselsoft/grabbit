// SPDX-License-Identifier: GPL-3.0-or-later
//! Ricerca su YouTube tramite yt-dlp (`ytsearchN:`), senza scaricare nulla:
//! restituisce titolo, canale, durata e link dei primi risultati.
use crate::downloader::{terminate, Toolset};
use crate::tools;
use crate::tr;
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Quanti risultati chiedere a YouTube.
pub const RESULTS: usize = 15;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub channel: String,
    /// secondi
    pub duration: Option<f64>,
    pub views: Option<u64>,
    /// link da scaricare (pagina del video)
    pub url: String,
}

/// Una riga di `--print "%(.{…})j"`: un oggetto JSON per risultato.
#[derive(Deserialize)]
struct Raw {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    view_count: Option<u64>,
    url: Option<String>,
    webpage_url: Option<String>,
}

pub fn build_args(
    query: &str,
    count: usize,
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
        "%(.{id,title,channel,uploader,duration,view_count,url,webpage_url})j",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(d) = &tools.deno {
        a.push("--js-runtimes".into());
        a.push(format!("deno:{}", d.to_string_lossy()));
    }
    if !cookies_browser.is_empty() {
        a.push("--cookies-from-browser".into());
        a.push(cookies_browser.into());
    }
    // "--": una ricerca che comincia con "-" non diventa un'opzione di yt-dlp
    a.push("--".into());
    a.push(format!("ytsearch{count}:{}", query.trim()));
    a
}

pub fn parse_line(line: &str) -> Option<SearchResult> {
    let raw: Raw = serde_json::from_str(line.trim()).ok()?;
    let url = raw
        .webpage_url
        .filter(|u| u.starts_with("http"))
        .or_else(|| raw.url.filter(|u| u.starts_with("http")))
        .or_else(|| {
            raw.id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        })?;
    let title = raw
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| url.clone());
    Some(SearchResult {
        title,
        channel: raw.channel.or(raw.uploader).unwrap_or_default(),
        duration: raw.duration.filter(|d| *d > 0.0),
        views: raw.view_count,
        url,
    })
}

/// Esegue la ricerca (bloccante: va chiamata da un thread a parte).
pub fn search(
    query: &str,
    tools: &Toolset,
    cookies_browser: &str,
    cancel: &AtomicBool,
) -> Result<Vec<SearchResult>> {
    let args = build_args(query, RESULTS, tools, cookies_browser);
    let mut child = tools::command(&tools.ytdlp)
        .args(&args)
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
        if started.elapsed() > TIMEOUT {
            terminate(&mut child);
            let _ = child.wait();
            bail!(tr!(
                "la ricerca ha impiegato troppo tempo",
                "the search took too long"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = out_t.join().unwrap_or_default();
    let err = err_t.join().unwrap_or_default();
    let results: Vec<SearchResult> = out.lines().filter_map(parse_line).collect();
    if results.is_empty() {
        if let Some(e) = err
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .map(|l| l.trim_start_matches("ERROR:").trim().to_string())
        {
            bail!(e);
        }
    }
    Ok(results)
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
            deno: Some("/dn/deno".into()),
        }
    }

    #[test]
    fn args_end_with_the_search_after_double_dash() {
        let a = build_args("  -Male Fantasy ", 15, &toolset(), "firefox");
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
        let l = r#"{"id": "xyz", "title": "", "channel": null, "uploader": "Qualcuno", "duration": null, "view_count": null, "url": "xyz", "webpage_url": null}"#;
        let r = parse_line(l).unwrap();
        assert_eq!(r.url, "https://www.youtube.com/watch?v=xyz");
        assert_eq!(r.title, r.url);
        assert_eq!(r.channel, "Qualcuno");
        assert_eq!(r.duration, None);
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
        let mut a = build_args("x", 1, &toolset(), "");
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
