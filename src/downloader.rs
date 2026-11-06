// SPDX-License-Identifier: GPL-3.0-or-later
//! Esecuzione dei download: costruisce la riga di comando di yt-dlp, legge
//! l'output in tempo reale e lo traduce in eventi per l'interfaccia.
use crate::config::{Config, Mode};
use crate::tr;
use crate::{spotify, tools};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Event {
    Log(String),
    JobAdded { id: usize, label: String },
    JobLabel { id: usize, label: String },
    JobProgress { id: usize, pct: f32, detail: String },
    JobFinished { id: usize, ok: bool, detail: String },
    Finished,
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Runtime JavaScript che yt-dlp usa per risolvere le «sfide» di YouTube:
/// Deno di solito, QuickJS sui Mac dove Deno non parte (macOS troppo vecchi).
#[derive(Clone, Debug, PartialEq)]
pub struct JsRuntime {
    /// nome per l'opzione `--js-runtimes` di yt-dlp: "deno" o "quickjs"
    pub kind: &'static str,
    pub path: PathBuf,
}

impl JsRuntime {
    pub fn deno(path: PathBuf) -> Self {
        Self { kind: "deno", path }
    }

    pub fn quickjs(path: PathBuf) -> Self {
        Self {
            kind: "quickjs",
            path,
        }
    }

    /// Valore per `--js-runtimes`, es. `deno:/percorso/deno`.
    pub fn arg(&self) -> String {
        format!("{}:{}", self.kind, self.path.to_string_lossy())
    }

    /// Nome da mostrare nell'interfaccia.
    pub fn name(&self) -> &'static str {
        if self.kind == "quickjs" {
            "QuickJS"
        } else {
            "Deno"
        }
    }
}

#[derive(Clone)]
pub struct Toolset {
    pub ytdlp: PathBuf,
    pub ffmpeg_dir: Option<PathBuf>,
    pub js: Option<JsRuntime>,
}

struct Job {
    id: usize,
    target: String,
    label: String,
    /// nome file senza estensione (brani Spotify)
    out_name: Option<String>,
    subdir: Option<String>,
    audio_only: bool,
}

/// Toglie i caratteri non ammessi nei nomi file.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|%\n\r\t".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned.trim_matches('.').chars().take(150).collect()
}

pub fn run(
    inputs: Vec<String>,
    cfg: Config,
    tools: Toolset,
    id_base: usize,
    emit: Emit,
    cancel: Arc<AtomicBool>,
) {
    // 1) trasforma gli input in lavori (i link Spotify diventano ricerche YouTube)
    let mut jobs: Vec<Job> = Vec::new();
    let mut next = id_base;
    for line in inputs {
        if spotify::is_spotify(&line) {
            emit(Event::Log(tr!(
                format!("Leggo il link Spotify: {line}"),
                format!("Reading Spotify link: {line}")
            )));
            match spotify::fetch(&line) {
                Ok(col) => {
                    emit(Event::Log(tr!(
                        format!(
                            "«{}»: {} brani trovati. L'audio viene cercato su YouTube.",
                            col.name,
                            col.tracks.len()
                        ),
                        format!(
                            "“{}”: {} tracks found. The audio is searched on YouTube.",
                            col.name,
                            col.tracks.len()
                        )
                    )));
                    let multi = col.tracks.len() > 1;
                    for t in col.tracks {
                        let label = if t.artist.is_empty() {
                            t.title.clone()
                        } else {
                            format!("{} – {}", t.artist, t.title)
                        };
                        let query = format!("{} {} audio", t.artist, t.title);
                        let name = sanitize(&label.replace(" – ", " - "));
                        jobs.push(Job {
                            id: next,
                            target: format!("ytsearch1:{}", query.trim()),
                            label,
                            out_name: Some(name),
                            subdir: multi.then(|| sanitize(&col.name)),
                            audio_only: true,
                        });
                        next += 1;
                    }
                }
                Err(e) => {
                    emit(Event::JobAdded {
                        id: next,
                        label: line.clone(),
                    });
                    emit(Event::JobFinished {
                        id: next,
                        ok: false,
                        detail: format!("Spotify: {e:#}"),
                    });
                    next += 1;
                }
            }
        } else {
            jobs.push(Job {
                id: next,
                target: line.clone(),
                label: line,
                out_name: None,
                subdir: None,
                audio_only: false,
            });
            next += 1;
        }
    }
    for j in &jobs {
        emit(Event::JobAdded {
            id: j.id,
            label: j.label.clone(),
        });
    }

    // 2) esegue i lavori uno alla volta
    for mut job in jobs {
        if cancel.load(Ordering::Relaxed) {
            emit(Event::JobFinished {
                id: job.id,
                ok: false,
                detail: tr!("Annullato", "Cancelled").into(),
            });
            continue;
        }
        // link che può essere una playlist: cartella con il nome della playlist
        if job.out_name.is_none() && job.subdir.is_none() && !looks_like_single_video(&job.target) {
            emit(Event::Log(
                tr!(
                    "Controllo se il link è una playlist…",
                    "Checking whether the link is a playlist…"
                )
                .into(),
            ));
            if let Some(title) = probe_playlist_title(&job, &cfg, &tools, &cancel) {
                let folder = sanitize(&title);
                if !folder.is_empty() {
                    emit(Event::Log(tr!(
                        format!("Playlist «{title}»: salvo i file nella cartella «{folder}»"),
                        format!("Playlist “{title}”: saving the files in the folder “{folder}”")
                    )));
                    job.subdir = Some(folder);
                }
            }
        }
        let (ok, detail) = run_job(&job, &cfg, &tools, &emit, &cancel);
        emit(Event::JobFinished {
            id: job.id,
            ok,
            detail,
        });
    }
    emit(Event::Finished);
}

/// Link a un singolo video di YouTube, senza playlist: inutile cercare il titolo di una playlist.
fn looks_like_single_video(url: &str) -> bool {
    let u = url.trim().to_lowercase();
    if u.contains("list=") {
        return false;
    }
    u.contains("youtube.com/watch?")
        || u.contains("youtu.be/")
        || u.contains("youtube.com/shorts/")
        || u.contains("youtube.com/live/")
}

/// Chiede a yt-dlp il titolo della playlist a cui appartiene il link.
/// `None` se il link non è una playlist, o se la richiesta non riesce o va in timeout:
/// in quel caso i file vanno semplicemente nella cartella scelta.
fn probe_playlist_title(
    job: &Job,
    cfg: &Config,
    tools: &Toolset,
    cancel: &AtomicBool,
) -> Option<String> {
    let mut a: Vec<String> = Vec::new();
    add(
        &mut a,
        &[
            "--flat-playlist",
            "--playlist-items",
            "1",
            "--skip-download",
            "--no-warnings",
            "--ignore-errors",
            "--print",
            "%(playlist_title|)s",
        ],
    );
    // stessa scelta del download vero: con "--no-playlist" un link video+lista è un solo video
    a.push(
        if cfg.playlist {
            "--yes-playlist"
        } else {
            "--no-playlist"
        }
        .into(),
    );
    if let Some(js) = &tools.js {
        add(&mut a, &["--js-runtimes", &js.arg()]);
    }
    if !cfg.cookies_browser.is_empty() {
        add(&mut a, &["--cookies-from-browser", &cfg.cookies_browser]);
    }
    a.push("--".into());
    a.push(job.target.clone());

    let mut child = tools::command(&tools.ytdlp)
        .args(&a)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => return None,
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(60) {
            terminate(&mut child);
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(String::from)
}

fn add(a: &mut Vec<String>, items: &[&str]) {
    a.extend(items.iter().map(|s| s.to_string()));
}

fn build_args(job: &Job, cfg: &Config, tools: &Toolset) -> Vec<String> {
    let audio = job.audio_only || cfg.mode == Mode::Audio;
    let mut a: Vec<String> = Vec::new();

    add(
        &mut a,
        &[
            "--newline",
            "--color",
            "never",
            "--ignore-errors",
            "--no-mtime",
            "-N",
            "4",
        ],
    );
    add(&mut a, &[
        "--progress",
        "--progress-template",
        "download:GRB|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(info.playlist_index)s|%(info.n_entries)s|%(info.title)s",
        "--print",
        "after_move:GRBFILE|%(filepath)s",
        "--no-quiet",
    ]);

    // cartella e nome file
    let mut dir = cfg.out_dir.clone();
    if let Some(sub) = &job.subdir {
        dir = dir.join(sub);
    }
    a.push("-P".into());
    a.push(dir.to_string_lossy().into_owned());
    a.push("-o".into());
    a.push(match &job.out_name {
        Some(n) => format!("{n}.%(ext)s"),
        // Solo numero d'ordine (se è una playlist) e titolo. La cartella della playlist NON
        // si fa con il modello: yt-dlp trasforma ogni "/" dentro un valore in un carattere
        // innocuo, quindi finirebbe tutto nel nome del file. La sceglie Grabbit (vedi
        // `probe_playlist_title`) e la passa a -P.
        None => "%(playlist_index&{} - |)s%(title).150B.%(ext)s".into(),
    });

    if cfg.playlist && job.out_name.is_none() {
        add(&mut a, &["--yes-playlist"]);
    } else {
        add(&mut a, &["--no-playlist"]);
    }

    if audio {
        add(
            &mut a,
            &[
                "-f",
                "bestaudio/best",
                "-x",
                "--audio-format",
                &cfg.audio_format,
            ],
        );
        if !matches!(cfg.audio_format.as_str(), "flac" | "wav") {
            add(
                &mut a,
                &["--audio-quality", &format!("{}K", cfg.audio_bitrate)],
            );
        }
        if cfg.embed_meta {
            add(&mut a, &["--embed-metadata"]);
            if matches!(cfg.audio_format.as_str(), "mp3" | "m4a" | "flac") {
                add(
                    &mut a,
                    &["--embed-thumbnail", "--convert-thumbnails", "jpg"],
                );
            }
        }
    } else {
        let res = if cfg.max_height > 0 {
            format!("res:{}", cfg.max_height)
        } else {
            "res".to_string()
        };
        let codecs = match cfg.video_container.as_str() {
            "mp4" => ",vcodec:h264,acodec:aac",
            "webm" => ",vcodec:vp9,acodec:opus",
            _ => "",
        };
        add(&mut a, &["-f", "bv*+ba/b", "-S", &format!("{res}{codecs}")]);
        add(&mut a, &["--merge-output-format", &cfg.video_container]);
        if cfg.embed_meta {
            add(&mut a, &["--embed-metadata"]);
            if matches!(cfg.video_container.as_str(), "mp4" | "mkv") {
                add(
                    &mut a,
                    &["--embed-thumbnail", "--convert-thumbnails", "jpg"],
                );
            }
        }
    }

    if let Some(d) = &tools.ffmpeg_dir {
        add(&mut a, &["--ffmpeg-location", &d.to_string_lossy()]);
    }
    if let Some(js) = &tools.js {
        add(&mut a, &["--js-runtimes", &js.arg()]);
    }
    if !cfg.cookies_browser.is_empty() {
        add(&mut a, &["--cookies-from-browser", &cfg.cookies_browser]);
    }
    a.push("--".into());
    a.push(job.target.clone());
    a
}

fn pump(r: impl Read + Send + 'static, tx: Sender<(bool, String)>, is_err: bool) {
    std::thread::spawn(move || {
        let mut br = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf).trim_end().to_string();
                    if tx.send((is_err, s)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// yt-dlp (PyInstaller) lancia un processo figlio: va terminato l'intero albero.
pub(crate) fn terminate(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = tools::command("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .output();
    }
    let _ = child.kill();
}

fn fmt_speed(bps: f64) -> String {
    if bps >= 1_048_576.0 {
        format!("{:.1} MB/s", bps / 1_048_576.0)
    } else {
        format!("{:.0} KB/s", bps / 1024.0)
    }
}

fn fmt_eta(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn num(s: Option<&str>) -> Option<f64> {
    s.and_then(|x| x.trim().parse::<f64>().ok())
}

fn run_job(
    job: &Job,
    cfg: &Config,
    tools: &Toolset,
    emit: &Emit,
    cancel: &Arc<AtomicBool>,
) -> (bool, String) {
    let args = build_args(job, cfg, tools);
    emit(Event::Log(format!("> {}", job.label)));

    let mut cmd = tools::command(&tools.ytdlp);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                tr!(
                    format!("impossibile avviare yt-dlp: {e}"),
                    format!("could not start yt-dlp: {e}")
                ),
            )
        }
    };

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    if let Some(o) = child.stdout.take() {
        pump(o, tx.clone(), false);
    }
    if let Some(e) = child.stderr.take() {
        pump(e, tx.clone(), true);
    }
    drop(tx);

    let mut last_pct = 0.0f32;
    let mut last_error: Option<String> = None;
    let mut files = 0usize;
    let mut last_file = String::new();
    let mut title_seen = false;
    let mut cancelled = false;

    loop {
        if cancel.load(Ordering::Relaxed) {
            terminate(&mut child);
            cancelled = true;
            break;
        }
        let (is_err, line) = match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(x) => x,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        if let Some(rest) = line.strip_prefix("GRBFILE|") {
            files += 1;
            last_file = rest.to_string();
            emit(Event::Log(tr!(
                format!("Salvato: {rest}"),
                format!("Saved: {rest}")
            )));
        } else if line.starts_with("GRB|") {
            let f: Vec<&str> = line.splitn(9, '|').collect();
            let get = |i: usize| f.get(i).copied().filter(|s| *s != "NA" && *s != "None");
            let done = num(get(1));
            let total = num(get(2)).or_else(|| num(get(3)));
            if let (Some(d), Some(t)) = (done, total) {
                if t > 0.0 {
                    last_pct = (d / t).clamp(0.0, 1.0) as f32;
                }
            }
            let mut parts: Vec<String> = Vec::new();
            if let (Some(i), Some(n)) = (get(6), get(7)) {
                parts.push(format!("{i}/{n}"));
            }
            if let Some(s) = num(get(4)) {
                parts.push(fmt_speed(s));
            }
            if let Some(e) = num(get(5)) {
                parts.push(tr!(
                    format!("ancora {}", fmt_eta(e)),
                    format!("{} left", fmt_eta(e))
                ));
            }
            if let Some(title) = get(8) {
                if job.out_name.is_none() && (!title_seen || get(6).is_some()) {
                    title_seen = true;
                    emit(Event::JobLabel {
                        id: job.id,
                        label: title.to_string(),
                    });
                }
            }
            emit(Event::JobProgress {
                id: job.id,
                pct: last_pct,
                detail: parts.join(" · "),
            });
        } else {
            if is_err && line.starts_with("ERROR") {
                last_error = Some(line.trim_start_matches("ERROR:").trim().to_string());
            }
            if line.starts_with("[Merger]")
                || line.starts_with("[ExtractAudio]")
                || line.starts_with("[VideoConvertor]")
                || line.starts_with("[EmbedThumbnail]")
                || line.starts_with("[Metadata]")
            {
                emit(Event::JobProgress {
                    id: job.id,
                    pct: last_pct.max(0.99),
                    detail: "elaborazione…".into(),
                });
            }
            emit(Event::Log(line));
        }
    }

    let status = child.wait();
    if cancelled {
        return (false, tr!("Annullato", "Cancelled").into());
    }
    let success = status.map(|s| s.success()).unwrap_or(false);
    match (success, files) {
        (true, _) | (false, 1..) => {
            let name = std::path::Path::new(&last_file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let note = if success {
                tr!("Completato", "Completed")
            } else {
                tr!("Completato in parte", "Partially completed")
            };
            (
                true,
                if name.is_empty() {
                    note.into()
                } else {
                    format!("{note} · {name}")
                },
            )
        }
        _ => (
            false,
            last_error.unwrap_or_else(|| {
                tr!(
                    "download non riuscito (vedi il registro)",
                    "download failed (see the log)"
                )
                .into()
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(target: &str, out_name: Option<&str>, audio_only: bool) -> Job {
        Job {
            id: 0,
            target: target.into(),
            label: target.into(),
            out_name: out_name.map(String::from),
            subdir: None,
            audio_only,
        }
    }

    fn toolset() -> Toolset {
        Toolset {
            ytdlp: "yt-dlp".into(),
            ffmpeg_dir: Some("/ff".into()),
            js: Some(JsRuntime::deno("/dn/deno".into())),
        }
    }

    fn has_pair(args: &[String], k: &str, v: &str) -> bool {
        args.windows(2).any(|w| w[0] == k && w[1] == v)
    }

    #[test]
    fn sanitize_removes_forbidden_chars() {
        assert_eq!(sanitize("AC/DC: Back in Black?"), "AC DC Back in Black");
        assert_eq!(sanitize("100% <live>"), "100 live");
    }

    #[test]
    fn video_mp4_1080() {
        let cfg = Config {
            mode: Mode::Video,
            max_height: 1080,
            video_container: "mp4".into(),
            ..Config::default()
        };
        let a = build_args(&job("https://youtu.be/x", None, false), &cfg, &toolset());
        assert!(has_pair(&a, "-S", "res:1080,vcodec:h264,acodec:aac"));
        assert!(has_pair(&a, "--merge-output-format", "mp4"));
        assert!(has_pair(&a, "--js-runtimes", "deno:/dn/deno"));
        assert!(a.contains(&"--no-playlist".to_string()));
        assert_eq!(a.last().unwrap(), "https://youtu.be/x");
    }

    #[test]
    fn audio_mp3_320() {
        let cfg = Config {
            mode: Mode::Audio,
            audio_format: "mp3".into(),
            audio_bitrate: 320,
            ..Config::default()
        };
        let a = build_args(&job("https://youtu.be/x", None, false), &cfg, &toolset());
        assert!(has_pair(&a, "--audio-format", "mp3"));
        assert!(has_pair(&a, "--audio-quality", "320K"));
        assert!(a.contains(&"--embed-thumbnail".to_string()));
    }

    #[test]
    fn flac_has_no_bitrate_and_spotify_forces_audio() {
        let cfg = Config {
            mode: Mode::Video,
            audio_format: "flac".into(),
            ..Config::default()
        };
        let a = build_args(
            &job("ytsearch1:x", Some("Artist - Song"), true),
            &cfg,
            &toolset(),
        );
        assert!(has_pair(&a, "--audio-format", "flac"));
        assert!(!a.contains(&"--audio-quality".to_string()));
        assert!(has_pair(&a, "-o", "Artist - Song.%(ext)s"));
    }

    #[test]
    fn playlist_flag() {
        let cfg = Config {
            playlist: true,
            ..Config::default()
        };
        let a = build_args(
            &job("https://youtube.com/playlist?list=x", None, false),
            &cfg,
            &toolset(),
        );
        assert!(a.contains(&"--yes-playlist".to_string()));
    }

    #[test]
    fn playlist_template_has_no_playlist_name_and_folder_goes_in_path() {
        let cfg = Config {
            out_dir: "/D".into(),
            playlist: true,
            ..Config::default()
        };
        let mut j = job("https://www.youtube.com/playlist?list=PL1", None, false);
        let a = build_args(&j, &cfg, &toolset());
        let o = a.iter().position(|x| x == "-o").unwrap();
        assert_eq!(a[o + 1], "%(playlist_index&{} - |)s%(title).150B.%(ext)s");
        assert!(!a[o + 1].contains("playlist_title") && !a[o + 1].contains('/'));
        let p = a.iter().position(|x| x == "-P").unwrap();
        assert_eq!(a[p + 1], "/D");
        j.subdir = Some("La mia playlist".into());
        let a = build_args(&j, &cfg, &toolset());
        let p = a.iter().position(|x| x == "-P").unwrap();
        assert_eq!(
            PathBuf::from(&a[p + 1]),
            PathBuf::from("/D").join("La mia playlist")
        );
    }

    #[test]
    fn single_video_links_skip_the_playlist_probe() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=10",
            "https://www.youtube.com/shorts/abc123",
        ] {
            assert!(looks_like_single_video(u), "{u}");
        }
        for u in [
            "https://www.youtube.com/playlist?list=PL123",
            "https://www.youtube.com/watch?v=abc&list=PL123",
            "https://www.youtube.com/@canale/videos",
            "https://soundcloud.com/utente/sets/album",
        ] {
            assert!(!looks_like_single_video(u), "{u}");
        }
    }

    /// Playlist vera (feed RSS locale) con yt-dlp reale: cartella con il nome della playlist,
    /// file senza il nome della playlist.
    /// GRABBIT_YTDLP=... GRABBIT_PLAYLIST_URL=http://127.0.0.1:8768/feed.rss GRABBIT_PLAYLIST_TITLE="Mix: Best/Of 2026" \
    ///   cargo test e2e_playlist -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_playlist_folder() {
        let (Ok(bin), Ok(url), Ok(title)) = (
            std::env::var("GRABBIT_YTDLP"),
            std::env::var("GRABBIT_PLAYLIST_URL"),
            std::env::var("GRABBIT_PLAYLIST_TITLE"),
        ) else {
            return;
        };
        let out = std::env::temp_dir().join("grabbit-e2e-playlist");
        let _ = std::fs::remove_dir_all(&out);
        let cfg = Config {
            mode: Mode::Video,
            out_dir: out.clone(),
            playlist: true,
            embed_meta: false,
            ..Config::default()
        };
        let tools = Toolset {
            ytdlp: bin.into(),
            ffmpeg_dir: None,
            js: None,
        };
        let emit: Emit = Arc::new(|e| {
            if let Event::Log(l) = e {
                println!("log: {l}");
            }
        });
        run(
            vec![url],
            cfg,
            tools,
            0,
            emit,
            Arc::new(AtomicBool::new(false)),
        );
        let folder = out.join(sanitize(&title));
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap_or_else(|e| panic!("cartella {folder:?} non creata: {e}"))
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        println!("{names:?}");
        assert_eq!(names.len(), 2);
        for n in &names {
            assert!(
                !n.contains("Mix") && !n.contains("Best"),
                "il nome della playlist è nel file: {n}"
            );
        }
        assert!(
            names[0].starts_with("1 - ") && names[1].starts_with("2 - "),
            "{names:?}"
        );
        // niente file fuori dalla cartella
        assert_eq!(out.read_dir().unwrap().count(), 1);
    }

    /// Verifica che un vero yt-dlp accetti tutte le combinazioni di opzioni.
    /// Uso: GRABBIT_YTDLP=/percorso/yt-dlp cargo test -- --ignored
    #[test]
    #[ignore]
    fn real_ytdlp_accepts_all_args() {
        let Ok(bin) = std::env::var("GRABBIT_YTDLP") else {
            return;
        };
        let mut configs = Vec::new();
        for c in ["mp4", "mkv", "webm"] {
            for h in [0, 720, 2160] {
                configs.push(Config {
                    mode: Mode::Video,
                    video_container: c.into(),
                    max_height: h,
                    playlist: h == 720,
                    ..Config::default()
                });
            }
        }
        for f in ["mp3", "m4a", "opus", "vorbis", "flac", "wav"] {
            configs.push(Config {
                mode: Mode::Audio,
                audio_format: f.into(),
                cookies_browser: String::new(),
                ..Config::default()
            });
        }
        for cfg in configs {
            let a = build_args(
                &job("http://127.0.0.1:9/none", None, false),
                &cfg,
                &toolset(),
            );
            let out = std::process::Command::new(&bin).args(&a).output().unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                !err.contains("Usage:") && !err.contains("yt-dlp: error:"),
                "argomenti rifiutati per {:?}:\n{}\n{err}",
                cfg.mode,
                a.join(" ")
            );
        }
    }

    /// Prova completa su un link diretto (senza YouTube):
    /// GRABBIT_YTDLP=... GRABBIT_TEST_URL=http://127.0.0.1:8765/clip.mp4 cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn e2e_direct_link() {
        use std::sync::Mutex;
        let (Ok(bin), Ok(url)) = (
            std::env::var("GRABBIT_YTDLP"),
            std::env::var("GRABBIT_TEST_URL"),
        ) else {
            return;
        };
        let out = std::env::temp_dir().join("grabbit-e2e");
        let _ = std::fs::remove_dir_all(&out);
        let cfg = Config {
            mode: Mode::Video,
            out_dir: out.clone(),
            embed_meta: false,
            ..Config::default()
        };
        let tools = Toolset {
            ytdlp: bin.into(),
            ffmpeg_dir: None,
            js: None,
        };
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let ev = events.clone();
        let emit: Emit = Arc::new(move |e| {
            ev.lock().unwrap().push(match e {
                Event::Log(l) => format!("log: {l}"),
                Event::JobAdded { label, .. } => format!("added: {label}"),
                Event::JobLabel { label, .. } => format!("label: {label}"),
                Event::JobProgress { pct, detail, .. } => format!("progress: {pct:.2} {detail}"),
                Event::JobFinished { ok, detail, .. } => format!("finished: {ok} {detail}"),
                Event::Finished => "done".into(),
            })
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_after = std::env::var("GRABBIT_CANCEL_AFTER_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(ms) = cancel_after {
            let c = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                c.store(true, Ordering::Relaxed);
            });
        }
        run(vec![url], cfg, tools, 0, emit, cancel);
        let ev = events.lock().unwrap();
        println!("{}", ev.join("\n"));
        if cancel_after.is_some() {
            assert!(
                ev.iter()
                    .any(|e| *e == format!("finished: false {}", tr!("Annullato", "Cancelled"))),
                "annullamento non riconosciuto"
            );
            return;
        }
        assert!(
            ev.iter().any(|e| e.starts_with("finished: true")),
            "download non riuscito"
        );
        assert!(
            ev.iter().any(|e| e.starts_with("progress:")),
            "nessun evento di avanzamento"
        );
        assert_eq!(out.read_dir().unwrap().count(), 1);
    }
}
