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
    JobAdded {
        id: usize,
        label: String,
    },
    JobLabel {
        id: usize,
        label: String,
    },
    /// lavoro sostituito da altri (un album diventa i suoi brani)
    JobRemoved {
        id: usize,
    },
    JobProgress {
        id: usize,
        pct: f32,
        detail: String,
    },
    JobFinished {
        id: usize,
        ok: bool,
        detail: String,
        /// file salvato (o già presente), per «Mostra nel Finder»
        file: Option<PathBuf>,
    },
    /// come scaricare di nuovo il lavoro con «Riprova» (stessa cartella e stesso nome)
    JobSpec {
        id: usize,
        job: Job,
    },
    Finished,
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Lavori da fermare uno per uno (l'«Annulla» di una riga): i loro id.
pub type StopSet = Arc<std::sync::Mutex<std::collections::HashSet<usize>>>;

fn is_stopped(stop: &StopSet, id: usize) -> bool {
    stop.lock().map(|s| s.contains(&id)).unwrap_or(false)
}

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

/// Un download: un link (o una ricerca), con cartella e nome del file già decisi.
#[derive(Clone, Debug)]
pub struct Job {
    id: usize,
    target: String,
    label: String,
    /// nome file senza estensione (brani Spotify)
    out_name: Option<String>,
    subdir: Option<String>,
    audio_only: bool,
    /// numero del brano nell'album: va anche nei metadati del file
    track: Option<usize>,
    /// cartella dell'artista dai metadati del brano (yt-dlp): `subdir` è allora l'album
    /// dentro quella cartella
    artist_from_tags: bool,
    /// album di cui scaricare solo alcuni brani (posizioni nella playlist, da 1)
    pick: Option<Vec<usize>>,
}

/// Separa il link di un album dai brani scelti in una riga di `run` (vedi `pick_input`).
const PICK_SEP: char = '\u{1f}';

/// Riga da passare a `run` per scaricare solo alcuni brani di un album: il link, un
/// separatore invisibile e le posizioni dei brani («1,3,5»).
pub fn pick_input(url: &str, tracks: &[usize]) -> String {
    let list: Vec<String> = tracks.iter().map(|n| n.to_string()).collect();
    format!("{url}{PICK_SEP}{}", list.join(","))
}

/// Link e brani scelti di una riga di `run`.
fn split_pick(line: &str) -> (String, Option<Vec<usize>>) {
    match line.split_once(PICK_SEP) {
        Some((url, list)) => (
            url.to_string(),
            Some(
                list.split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect(),
            ),
        ),
        None => (line.to_string(), None),
    }
}

/// Elenco dei brani di un album o di una playlist, per sceglierli prima di scaricare:
/// (titolo, [(posizione, titolo del brano)]).
pub fn list_tracks(
    url: &str,
    cfg: &Config,
    tools: &Toolset,
    cancel: &AtomicBool,
) -> Option<(String, Vec<(usize, String)>)> {
    let job = Job {
        id: 0,
        target: url.to_string(),
        label: url.to_string(),
        out_name: None,
        subdir: None,
        audio_only: false,
        track: None,
        artist_from_tags: false,
        pick: None,
    };
    let cfg = Config {
        playlist: true,
        ..cfg.clone()
    };
    let l = list_playlist(&job, &cfg, tools, cancel)?;
    let title = crate::search::clean_name(&l.title);
    Some((
        title,
        l.entries.into_iter().map(|e| (e.index, e.title)).collect(),
    ))
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

/// File dell'archivio dei brani già scaricati.
pub fn archive_path() -> PathBuf {
    tools::data_dir().join("downloaded.json")
}

/// Archivio dei brani già scaricati: per ogni brano (e formato) il file salvato.
/// Diversamente da `--download-archive` di yt-dlp, un brano conta come già scaricato
/// solo se il file c'è ancora: se lo cancelli, si riscarica.
struct Archive {
    path: Option<PathBuf>,
    map: std::collections::BTreeMap<String, String>,
}

impl Archive {
    fn load(path: Option<PathBuf>) -> Self {
        let map = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, map }
    }

    /// Il file già scaricato per questa chiave, se esiste ancora.
    fn existing(&self, key: &str) -> Option<PathBuf> {
        let p = PathBuf::from(self.map.get(key)?);
        p.is_file().then_some(p)
    }

    fn record(&mut self, key: String, file: String) {
        self.map.insert(key, file);
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(&self.map) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

/// Id di un video di YouTube o YouTube Music nel link.
fn youtube_id(url: &str) -> Option<String> {
    let u = url.trim();
    let id = if let Some(i) = u.find("v=").filter(|_| u.contains("youtube.com/watch")) {
        &u[i + 2..]
    } else if let Some(i) = u.find("youtu.be/") {
        &u[i + 9..]
    } else {
        let i = u.find("youtube.com/shorts/")?;
        &u[i + 19..]
    };
    let id: String = id
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!id.is_empty()).then_some(id)
}

/// Chiave dell'archivio: formato scelto + brano. `None` per i link che possono contenere
/// più brani (playlist, canali), che non si possono saltare in blocco.
fn archive_key(job: &Job, cfg: &Config) -> Option<String> {
    let format = if job.audio_only || cfg.mode == Mode::Audio {
        format!("audio:{}", cfg.audio_format)
    } else {
        format!("video:{}:{}", cfg.video_container, cfg.max_height)
    };
    let t = job.target.trim();
    let item = if let Some(q) = t.strip_prefix("ytsearch1:") {
        // brano Spotify: la ricerca su YouTube identifica il brano
        format!("search:{}", q.to_lowercase())
    } else {
        let id = youtube_id(t)?;
        if cfg.playlist && t.contains("list=") && job.track.is_none() {
            return None;
        }
        format!("youtube:{id}")
    };
    Some(format!("{format}|{item}"))
}

// i parametri sono tutti indipendenti: raggrupparli non renderebbe più chiaro nulla
#[allow(clippy::too_many_arguments)]
pub fn run(
    inputs: Vec<String>,
    cfg: Config,
    tools: Toolset,
    id_base: usize,
    emit: Emit,
    cancel: Arc<AtomicBool>,
    archive: Option<PathBuf>,
    stop: StopSet,
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
                            track: None,
                            artist_from_tags: false,
                            pick: None,
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
                        file: None,
                    });
                    next += 1;
                }
            }
        } else {
            let (target, pick) = split_pick(&line);
            jobs.push(Job {
                id: next,
                label: target.clone(),
                target,
                pick,
                out_name: None,
                subdir: None,
                audio_only: false,
                track: None,
                artist_from_tags: false,
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

    // 2) un link che è una playlist (un album) diventa un lavoro per ogni brano, nella
    //    cartella con il nome della playlist: così i brani si scaricano in parallelo
    let mut queue: Vec<Job> = Vec::new();
    for job in jobs {
        if cancel.load(Ordering::Relaxed)
            || job.out_name.is_some()
            || job.subdir.is_some()
            || (job.pick.is_none() && looks_like_single_video(&job.target))
        {
            queue.push(job);
            continue;
        }
        emit(Event::Log(
            tr!(
                "Controllo se il link è una playlist e leggo l'elenco dei brani…",
                "Checking whether the link is a playlist and reading its tracks…"
            )
            .into(),
        ));
        let Some(Listing {
            title,
            entries,
            info,
        }) = list_playlist(&job, &cfg, &tools, &cancel)
        else {
            if job.pick.is_some() {
                // senza l'elenco non si sa quali brani scaricare: meglio non scaricare tutto
                emit(Event::JobFinished {
                    id: job.id,
                    ok: false,
                    detail: tr!(
                        "Non riesco a leggere i brani dell'album: riprova",
                        "Could not read the album's tracks: try again"
                    )
                    .into(),
                    file: None,
                });
            } else {
                queue.push(job);
            }
            continue;
        };
        // solo i brani scelti, con il loro numero nell'album
        let entries: Vec<Entry> = match &job.pick {
            Some(p) => entries
                .into_iter()
                .filter(|e| p.contains(&e.index))
                .collect(),
            None => entries,
        };
        // «Album - Titolo» (così lo chiama YouTube) → «Titolo»
        let album = sanitize(&crate::search::clean_name(&title));
        let audio = job.audio_only || cfg.mode == Mode::Audio;
        // musica: un album va in «Artista/Album», con i brani numerati a due cifre
        let music_album = audio && cfg.music_folders && is_music_album(&job.target, &info);
        let artist = if music_album {
            album_artist(&entries, &info).map(|a| sanitize(&a))
        } else {
            None
        };
        let artist = artist.filter(|a| !a.is_empty() && !album.is_empty());
        let folder = match &artist {
            Some(a) => format!("{a}/{album}"),
            None => album.clone(),
        };
        let shown = if music_album && artist.is_none() {
            tr!(format!("(artista)/{folder}"), format!("(artist)/{folder}"))
        } else {
            folder.clone()
        };
        emit(Event::Log(tr!(
            format!(
                "Playlist «{title}»: {} brani, salvati nella cartella «{shown}»",
                entries.len()
            ),
            format!(
                "Playlist “{title}”: {} tracks, saved in the folder “{shown}”",
                entries.len()
            )
        )));
        emit(Event::JobRemoved { id: job.id });
        for e in entries {
            let num = if music_album {
                format!("{:02}", e.index)
            } else {
                e.index.to_string()
            };
            let j = Job {
                id: next,
                out_name: Some(sanitize(&format!("{num} - {}", e.title))),
                label: e.title,
                target: e.url,
                subdir: (!folder.is_empty()).then(|| folder.clone()),
                audio_only: job.audio_only,
                track: Some(e.index),
                artist_from_tags: music_album && artist.is_none(),
                pick: None,
            };
            next += 1;
            emit(Event::JobAdded {
                id: j.id,
                label: j.label.clone(),
            });
            queue.push(j);
        }
    }

    for j in &queue {
        emit(Event::JobSpec {
            id: j.id,
            job: j.clone(),
        });
    }
    execute(queue, &cfg, &tools, &emit, &cancel, archive, &stop);
    emit(Event::Finished);
}

/// Scarica di nuovo alcuni lavori già preparati («Riprova»), con le opzioni attuali.
pub fn retry(
    jobs: Vec<Job>,
    cfg: Config,
    tools: Toolset,
    emit: Emit,
    cancel: Arc<AtomicBool>,
    archive: Option<PathBuf>,
    stop: StopSet,
) {
    execute(jobs, &cfg, &tools, &emit, &cancel, archive, &stop);
    emit(Event::Finished);
}

/// Esegue i lavori, alcuni alla volta: YouTube limita la velocità di ogni singolo
/// download, non quella della connessione, quindi in parallelo si fa prima.
fn execute(
    queue: Vec<Job>,
    cfg: &Config,
    tools: &Toolset,
    emit: &Emit,
    cancel: &Arc<AtomicBool>,
    archive: Option<PathBuf>,
    stop: &StopSet,
) {
    let archive = std::sync::Mutex::new(Archive::load(archive));
    let workers = (cfg.parallel.clamp(1, 4) as usize).min(queue.len().max(1));
    let tagged = workers > 1;
    let queue = std::sync::Mutex::new(
        queue
            .into_iter()
            .enumerate()
            .collect::<std::collections::VecDeque<_>>(),
    );
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                // il lucchetto della coda si rilascia subito, prima del download
                let next = queue.lock().ok().and_then(|mut q| q.pop_front());
                let Some((n, job)) = next else {
                    break;
                };
                if cancel.load(Ordering::Relaxed) || is_stopped(stop, job.id) {
                    emit(Event::JobFinished {
                        id: job.id,
                        ok: false,
                        detail: tr!("Annullato", "Cancelled").into(),
                        file: None,
                    });
                    continue;
                }
                // con più download insieme le righe del registro si mescolano:
                // ognuna porta il numero del suo download
                let job_emit: Emit = if tagged {
                    let emit = emit.clone();
                    let tag = format!("#{} ", n + 1);
                    Arc::new(move |e| match e {
                        Event::Log(l) => emit(Event::Log(format!("{tag}{l}"))),
                        other => emit(other),
                    })
                } else {
                    emit.clone()
                };
                let key = archive_key(&job, cfg);
                if cfg.skip_downloaded {
                    let found = key.as_ref().and_then(|k| archive.lock().ok()?.existing(k));
                    if let Some(file) = found {
                        let name = file
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        job_emit(Event::Log(tr!(
                            format!("Già scaricato, salto: {}", file.display()),
                            format!("Already downloaded, skipping: {}", file.display())
                        )));
                        emit(Event::JobFinished {
                            id: job.id,
                            ok: true,
                            detail: tr!(
                                format!("Già scaricato · {name}"),
                                format!("Already downloaded · {name}")
                            ),
                            file: Some(file),
                        });
                        continue;
                    }
                }
                let (ok, detail, file, saved) = run_job(&job, cfg, tools, &job_emit, cancel, stop);
                if let (Some(k), Some(f)) = (key, file) {
                    if let Ok(mut a) = archive.lock() {
                        a.record(k, f);
                    }
                }
                emit(Event::JobFinished {
                    id: job.id,
                    ok,
                    detail,
                    file: saved.map(PathBuf::from),
                });
            });
        }
    });
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

/// Un brano di una playlist.
#[derive(Debug, PartialEq)]
struct Entry {
    url: String,
    title: String,
    /// posizione nella playlist (da 1)
    index: usize,
    /// canale del brano (per gli album di YouTube Music: «Artista - Topic»)
    channel: Option<String>,
}

/// Dati della playlist che yt-dlp ripete in ogni riga dell'elenco.
#[derive(Debug, Default, PartialEq)]
struct PlaylistInfo {
    title: Option<String>,
    id: Option<String>,
    uploader: Option<String>,
}

/// Elenco dei brani di una playlist.
struct Listing {
    title: String,
    entries: Vec<Entry>,
    info: PlaylistInfo,
}

/// Legge una riga di `--print "%(.{…})j"` dell'elenco di una playlist:
/// (dati della playlist, brano).
fn parse_entry(line: &str, fallback_index: usize) -> Option<(PlaylistInfo, Entry)> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let text = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "NA")
            .map(String::from)
    };
    let url = text("url")
        .filter(|u| u.starts_with("http"))
        .or_else(|| text("id").map(|id| format!("https://www.youtube.com/watch?v={id}")))?;
    let index = v
        .get("playlist_index")
        .and_then(|x| x.as_u64())
        .map(|n| n as usize)
        .unwrap_or(fallback_index);
    let title = text("title").unwrap_or_else(|| url.clone());
    let info = PlaylistInfo {
        title: text("playlist_title"),
        id: text("playlist_id"),
        uploader: text("playlist_uploader").or_else(|| text("playlist_channel")),
    };
    let channel = text("channel").or_else(|| text("uploader"));
    Some((
        info,
        Entry {
            url,
            title,
            index,
            channel,
        },
    ))
}

/// Un album di YouTube Music (playlist «OLAK5uy_…» o pagina «browse/MPREb…»), non una
/// playlist qualsiasi.
fn is_music_album(target: &str, info: &PlaylistInfo) -> bool {
    info.id
        .as_deref()
        .is_some_and(|id| id.starts_with("OLAK5uy_"))
        || target.contains("list=OLAK5uy_")
        || target.contains("/browse/MPREb")
}

/// Canale generato da YouTube per un artista («Artista - Topic»): il suo nome è l'artista.
/// Gli altri canali sono di chi ha caricato i video («Rengaofficial»), non l'artista.
fn topic_artist(c: &str) -> Option<String> {
    let a = c.trim().strip_suffix(" - Topic")?.trim();
    (!a.is_empty()).then(|| a.to_string())
}

/// L'artista dell'album dall'elenco dei brani: il canale «Artista - Topic» di almeno metà
/// dei brani, altrimenti quello della playlist; brani di artisti «Topic» diversi: «Artisti
/// vari». `None` se l'elenco non lo dice: lo prende yt-dlp dai metadati di ogni brano.
fn album_artist(entries: &[Entry], info: &PlaylistInfo) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for a in entries
        .iter()
        .filter_map(|e| e.channel.as_deref())
        .filter_map(topic_artist)
    {
        match counts.iter_mut().find(|(x, _)| *x == a) {
            Some((_, n)) => *n += 1,
            None => counts.push((a, 1)),
        }
    }
    if let Some((a, n)) = counts.iter().max_by_key(|(_, n)| *n) {
        if n * 2 >= entries.len() {
            return Some(a.clone());
        }
    }
    match info.uploader.as_deref().and_then(topic_artist) {
        Some(u) => Some(u),
        None if counts.len() > 1 => Some(tr!("Artisti vari", "Various Artists").into()),
        None => None,
    }
}

/// Chiede a yt-dlp l'elenco dei brani della playlist del link (senza scaricarli).
/// `None` se il link non è una playlist, o se la richiesta non riesce o va in timeout:
/// in quel caso il link si scarica così com'è, nella cartella scelta.
fn list_playlist(job: &Job, cfg: &Config, tools: &Toolset, cancel: &AtomicBool) -> Option<Listing> {
    let mut a: Vec<String> = Vec::new();
    add(
        &mut a,
        &[
            "--flat-playlist",
            "--skip-download",
            "--no-warnings",
            "--ignore-errors",
            "--print",
            "%(.{id,url,title,channel,uploader,playlist_title,playlist_index,playlist_id,playlist_uploader,playlist_channel})j",
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
    // letto in un thread a parte: con playlist lunghe l'uscita riempirebbe il canale
    // e yt-dlp resterebbe bloccato
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => return None,
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(120) {
            terminate(&mut child);
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = reader.join().ok()?;
    let mut info = PlaylistInfo::default();
    let mut entries = Vec::new();
    for (i, line) in out.lines().enumerate() {
        if let Some((p, e)) = parse_entry(line, i + 1) {
            info.title = info.title.or(p.title);
            info.id = info.id.or(p.id);
            info.uploader = info.uploader.or(p.uploader);
            entries.push(e);
        }
    }
    // senza titolo di playlist è un video singolo (es. un link video+lista con «--no-playlist»)
    let title = info.title.clone()?;
    (!entries.is_empty()).then_some(Listing {
        title,
        entries,
        info,
    })
}

/// Copertina quadrata nei file audio: la miniatura di YouTube è 16:9 con la copertina al
/// centro e bande ai lati; ffmpeg tiene il quadrato centrale mentre la converte in JPEG.
const SQUARE_COVER: &str = "ThumbnailsConvertor+FFmpeg_o:-c:v mjpeg -qmin 1 -qscale:v 1 -vf crop=\"'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'\"";

/// Anno di uscita nei tag dell'audio (vedi `build_args`).
const RELEASE_YEAR: &str = "%(release_date>%Y,release_year|)s:%(meta_date)s";

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
    if let Some(sub) = job.subdir.as_ref().filter(|_| !job.artist_from_tags) {
        // «Artista/Album»: una cartella dentro l'altra (sanitize toglie le «/» dai nomi)
        for part in sub.split('/').filter(|p| !p.is_empty()) {
            dir = dir.join(part);
        }
    }
    a.push("-P".into());
    a.push(dir.to_string_lossy().into_owned());
    a.push("-o".into());
    let artist_tmpl = format!(
        "%(album_artists.0,artists.0,artist,creator,uploader|{})s",
        tr!("Artista sconosciuto", "Unknown Artist")
    );
    a.push(match &job.out_name {
        // brano di un album con l'artista dai metadati: «Artista/Album/01 - Titolo»
        Some(n) if job.artist_from_tags => match &job.subdir {
            Some(album) => format!("{artist_tmpl}/{album}/{n}.%(ext)s"),
            None => format!("{artist_tmpl}/{n}.%(ext)s"),
        },
        Some(n) => format!("{n}.%(ext)s"),
        // musica senza album noto: cartelle dai metadati di yt-dlp (YouTube Music li ha
        // per ogni brano). Qui le «/» del modello creano le cartelle.
        None if audio && cfg.music_folders && !(cfg.playlist && job.target.contains("list=")) => {
            format!(
                "{artist_tmpl}/%(album|{})s/%(track,title).150B.%(ext)s",
                tr!("Singoli", "Singles"),
            )
        }
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
    // brano di un album scaricato da solo: il numero del brano va nei metadati
    if let Some(n) = job.track {
        add(
            &mut a,
            &["--parse-metadata", &format!("{n}:%(track_number)s")],
        );
    }

    if audio {
        // Se YouTube ha già l'audio nel formato scelto (M4A = AAC, Opus/Vorbis = WebM),
        // si prende quello: ffmpeg lo copia senza ricodificarlo (più veloce, qualità originale).
        let format = match cfg.audio_format.as_str() {
            "m4a" => "bestaudio[ext=m4a]/bestaudio/best",
            "opus" => "bestaudio[acodec=opus]/bestaudio/best",
            _ => "bestaudio/best",
        };
        add(
            &mut a,
            &["-f", format, "-x", "--audio-format", &cfg.audio_format],
        );
        if !matches!(cfg.audio_format.as_str(), "flac" | "wav") {
            add(
                &mut a,
                &["--audio-quality", &format!("{}K", cfg.audio_bitrate)],
            );
        }
        if cfg.embed_meta {
            add(&mut a, &["--embed-metadata"]);
            // nei tag la data è quella del caricamento su YouTube: per la musica serve l'anno
            // di uscita. Prima «Released on: 2008-01-28», poi «℗ 2007» (anno di registrazione)
            // dalla descrizione dei brani di YouTube Music. Se mancano entrambi il testo è vuoto:
            // yt-dlp avvisa soltanto e resta la data di caricamento (mai «NA»).
            add(&mut a, &["--parse-metadata", RELEASE_YEAR]);
            if matches!(cfg.audio_format.as_str(), "mp3" | "m4a" | "flac") {
                add(
                    &mut a,
                    &[
                        "--embed-thumbnail",
                        "--convert-thumbnails",
                        "jpg",
                        "--ppa",
                        SQUARE_COVER,
                    ],
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
    stop: &StopSet,
) -> (bool, String, Option<String>, Option<String>) {
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
                None,
                None,
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
        if cancel.load(Ordering::Relaxed) || is_stopped(stop, job.id) {
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
        return (false, tr!("Annullato", "Cancelled").into(), None, None);
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
            // un solo file salvato e nessun errore: va nell'archivio dei già scaricati
            let file = (success && files == 1).then(|| last_file.clone());
            let saved = (!last_file.is_empty()).then(|| last_file.clone());
            (
                true,
                if name.is_empty() {
                    note.into()
                } else {
                    format!("{note} · {name}")
                },
                file,
                saved,
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
            None,
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I test che scrivono e avviano un finto yt-dlp girano uno alla volta: se un test
    /// avvia un processo mentre un altro sta ancora scrivendo il suo script, il processo
    /// figlio eredita per un istante il file aperto e Linux rifiuta di eseguirlo
    /// («Text file busy»): l'avvio fallisce a caso.
    #[cfg(unix)]
    static FAKE_YTDLP: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn fake_ytdlp_lock() -> std::sync::MutexGuard<'static, ()> {
        FAKE_YTDLP.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn job(target: &str, out_name: Option<&str>, audio_only: bool) -> Job {
        Job {
            id: 0,
            target: target.into(),
            label: target.into(),
            out_name: out_name.map(String::from),
            subdir: None,
            audio_only,
            track: None,
            artist_from_tags: false,
            pick: None,
        }
    }

    /// Con un finto yt-dlp (uno script che impiega 1 s per «download»): un album diventa i
    /// suoi brani numerati nella cartella dell'album, e 3 download vanno in parallelo.
    #[cfg(unix)]
    #[test]
    fn albums_are_split_into_tracks_and_downloaded_in_parallel() {
        let _guard = fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        let dir = std::env::temp_dir().join(format!("grabbit-par-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let fake = dir.join("fake-yt-dlp");
        std::fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
last=""; for a in "$@"; do last="$a"; done
echo "$*" >> "{log}"
case " $* " in *" --flat-playlist "*)
  case "$last" in *album*)
    for i in 1 2 3 4; do echo "{{\"id\":\"id$i\",\"url\":\"https://x.test/watch?v=$i\",\"title\":\"Brano $i\",\"playlist_title\":\"Album - Disco\",\"playlist_index\":$i}}"; done;;
  esac
  exit 0;;
esac
sleep 1
echo "GRB|1|1|1|1|0|NA|NA|t"
echo "GRBFILE|/x/$last"
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let ev = events.clone();
        let emit: Emit = Arc::new(move |e| {
            let s = match e {
                Event::JobAdded { label, .. } => format!("added {label}"),
                Event::JobRemoved { .. } => "removed".into(),
                Event::JobFinished { ok, .. } => format!("finished {ok}"),
                _ => return,
            };
            ev.lock().unwrap().push(s);
        });
        let cfg = Config {
            mode: Mode::Audio,
            out_dir: dir.clone(),
            parallel: 3,
            ..Config::default()
        };
        let tools = Toolset {
            ytdlp: fake,
            ffmpeg_dir: None,
            js: None,
        };
        let started = Instant::now();
        run(
            vec!["https://x.test/album".into(), "https://x.test/other".into()],
            cfg,
            tools,
            0,
            emit,
            Arc::new(AtomicBool::new(false)),
            None,
            StopSet::default(),
        );
        let secs = started.elapsed().as_secs_f32();
        let ev = events.lock().unwrap();
        // l'album è sostituito dai suoi 4 brani; l'altro link resta com'è
        assert_eq!(ev.iter().filter(|e| *e == "removed").count(), 1, "{ev:?}");
        for i in 1..=4 {
            assert!(ev.contains(&format!("added Brano {i}")), "{ev:?}");
        }
        assert_eq!(
            ev.iter().filter(|e| *e == "finished true").count(),
            5,
            "{ev:?}"
        );
        // 5 download da 1 s con 3 alla volta: 2 giri, non 5
        assert!(
            secs < 3.5,
            "troppo lento per essere in parallelo: {secs:.1} s"
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        let downloads: Vec<&str> = calls
            .lines()
            .filter(|l| !l.contains("--flat-playlist"))
            .collect();
        assert_eq!(downloads.len(), 5);
        let track3 = downloads
            .iter()
            .find(|l| l.ends_with("https://x.test/watch?v=3"))
            .expect("brano 3 scaricato dal suo link");
        assert!(track3.contains("3:%(track_number)s"), "{track3}");
        assert!(track3.contains("3 - Brano 3.%(ext)s"), "{track3}");
        assert!(
            track3.contains(&dir.join("Disco").display().to_string()),
            "{track3}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un brano già scaricato non si riscarica, a meno che il file sia sparito, il formato
    /// sia diverso o l'opzione sia spenta.
    #[cfg(unix)]
    #[test]
    fn already_downloaded_tracks_are_skipped() {
        let _guard = fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        let dir = std::env::temp_dir().join(format!("grabbit-arc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let calls = dir.join("calls.log");
        let file = dir.join("Brano.m4a");
        let fake = dir.join("fake-yt-dlp");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho x >> \"{calls}\"\ntouch \"{file}\"\necho \"GRBFILE|{file}\"\n",
                calls = calls.display(),
                file = file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let archive = dir.join("downloaded.json");

        let go = |cfg: Config| -> String {
            let details = Arc::new(Mutex::new(Vec::<String>::new()));
            let d = details.clone();
            let emit: Emit = Arc::new(move |e| {
                if let Event::JobFinished { ok, detail, .. } = e {
                    assert!(ok, "{detail}");
                    d.lock().unwrap().push(detail);
                }
            });
            let tools = Toolset {
                ytdlp: fake.clone(),
                ffmpeg_dir: None,
                js: None,
            };
            run(
                vec!["https://music.youtube.com/watch?v=abc123&si=x".into()],
                cfg,
                tools,
                0,
                emit,
                Arc::new(AtomicBool::new(false)),
                Some(archive.clone()),
                StopSet::default(),
            );
            let d = details.lock().unwrap();
            assert_eq!(d.len(), 1);
            d[0].clone()
        };
        let n_calls = || {
            std::fs::read_to_string(&calls)
                .unwrap_or_default()
                .lines()
                .count()
        };
        let audio = |fmt: &str| Config {
            mode: Mode::Audio,
            audio_format: fmt.into(),
            out_dir: dir.clone(),
            ..Config::default()
        };

        go(audio("m4a"));
        assert_eq!(n_calls(), 1);
        // stesso brano, stesso formato: saltato
        let d = go(audio("m4a"));
        assert!(d.contains("Brano.m4a"), "{d}");
        assert_eq!(n_calls(), 1, "non doveva riscaricare");
        // un altro link allo stesso video (youtu.be) è lo stesso brano
        assert_eq!(
            archive_key(&job("https://youtu.be/abc123", None, false), &audio("m4a")),
            archive_key(
                &job("https://www.youtube.com/watch?v=abc123", None, false),
                &audio("m4a")
            )
        );
        // formato diverso: si scarica
        go(audio("mp3"));
        assert_eq!(n_calls(), 2);
        // opzione spenta: si scarica
        go(Config {
            skip_downloaded: false,
            ..audio("m4a")
        });
        assert_eq!(n_calls(), 3);
        // file cancellato: si riscarica
        std::fs::remove_file(&file).unwrap();
        go(audio("m4a"));
        assert_eq!(n_calls(), 4);
        // le playlist intere non hanno chiave
        let cfg = Config {
            playlist: true,
            ..audio("m4a")
        };
        assert_eq!(
            archive_key(
                &job("https://www.youtube.com/watch?v=a&list=PL1", None, false),
                &cfg
            ),
            None
        );
        assert_eq!(
            archive_key(
                &job("https://www.youtube.com/playlist?list=PL1", None, false),
                &cfg
            ),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un album di YouTube Music scaricato come audio va in «Artista/Album/01 - Titolo»;
    /// una playlist qualsiasi resta nella cartella con il suo nome.
    #[cfg(unix)]
    #[test]
    fn music_albums_go_in_artist_album_folders() {
        let _guard = fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("grabbit-mf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let fake = dir.join("fake-yt-dlp");
        std::fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
last=""; for a in "$@"; do last="$a"; done
echo "$*" >> "{log}"
case " $* " in *" --flat-playlist "*)
  case "$last" in
  *OLAK5uy_r*)
    for i in 1 2; do echo "{{\"id\":\"r$i\",\"url\":\"https://x.test/watch?v=r$i\",\"title\":\"Pezzo $i\",\"channel\":\"Rengaofficial\",\"playlist_title\":\"Album - Ferro E Cartone\",\"playlist_id\":\"OLAK5uy_r\",\"playlist_index\":$i}}"; done;;
  *OLAK5uy_*)
    for i in 1 2 3; do echo "{{\"id\":\"v$i\",\"url\":\"https://x.test/watch?v=a$i\",\"title\":\"Brano $i\",\"channel\":\"Mina - Topic\",\"playlist_title\":\"Album - Studio Uno\",\"playlist_id\":\"OLAK5uy_x\",\"playlist_index\":$i}}"; done;;
  *PLmix*)
    for i in 1 2; do echo "{{\"id\":\"m$i\",\"url\":\"https://x.test/watch?v=m$i\",\"title\":\"Pezzo $i\",\"channel\":\"Canale $i\",\"playlist_title\":\"Estate\",\"playlist_id\":\"PLmix\",\"playlist_index\":$i}}"; done;;
  esac
  exit 0;;
esac
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = Config {
            mode: Mode::Audio,
            out_dir: dir.clone(),
            ..Config::default()
        };
        let tools = Toolset {
            ytdlp: fake,
            ffmpeg_dir: None,
            js: None,
        };
        run(
            vec![
                "https://music.youtube.com/playlist?list=OLAK5uy_x".into(),
                "https://www.youtube.com/playlist?list=PLmix".into(),
                "https://music.youtube.com/playlist?list=OLAK5uy_r".into(),
            ],
            cfg,
            tools,
            0,
            Arc::new(|_| {}),
            Arc::new(AtomicBool::new(false)),
            None,
            StopSet::default(),
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        let dl = |url: &str| -> String {
            calls
                .lines()
                .find(|l| !l.contains("--flat-playlist") && l.ends_with(url))
                .unwrap_or_else(|| panic!("{url} non scaricato:\n{calls}"))
                .to_string()
        };
        let t2 = dl("https://x.test/watch?v=a2");
        let album_dir = dir.join("Mina").join("Studio Uno");
        assert!(t2.contains(&album_dir.display().to_string()), "{t2}");
        assert!(t2.contains("02 - Brano 2.%(ext)s"), "{t2}");
        // canale non «Topic»: l'artista viene dai metadati di yt-dlp, l'album resta quello
        let r1 = dl("https://x.test/watch?v=r1");
        assert!(r1.contains(&format!("-P {} ", dir.display())), "{r1}");
        assert!(
            r1.contains("%(album_artists.0,artists.0,artist,creator,uploader|"),
            "{r1}"
        );
        assert!(
            r1.contains(")s/Ferro E Cartone/01 - Pezzo 1.%(ext)s"),
            "{r1}"
        );
        let m1 = dl("https://x.test/watch?v=m1");
        assert!(
            m1.contains(&dir.join("Estate").display().to_string()),
            "{m1}"
        );
        assert!(m1.contains("1 - Pezzo 1.%(ext)s"), "{m1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Solo i brani scelti di un album, con il loro numero nell'album.
    #[cfg(unix)]
    #[test]
    fn only_the_chosen_tracks_are_downloaded() {
        let _guard = fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("grabbit-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let fake = dir.join("fake-yt-dlp");
        std::fs::write(
            &fake,
            format!(
                r#"#!/bin/sh
echo "$*" >> "{log}"
case " $* " in *" --flat-playlist "*)
  for i in 1 2 3 4; do echo "{{\"id\":\"v$i\",\"url\":\"https://x.test/watch?v=p$i\",\"title\":\"Brano $i\",\"channel\":\"Mina - Topic\",\"playlist_title\":\"Album - Studio Uno\",\"playlist_id\":\"OLAK5uy_x\",\"playlist_index\":$i}}"; done
  exit 0;;
esac
"#,
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Toolset {
            ytdlp: fake,
            ffmpeg_dir: None,
            js: None,
        };
        let cfg = Config {
            mode: Mode::Audio,
            out_dir: dir.clone(),
            ..Config::default()
        };
        // l'elenco per scegliere
        let (title, tracks) = list_tracks(
            "https://music.youtube.com/playlist?list=OLAK5uy_x",
            &cfg,
            &tools,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(title, "Studio Uno");
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[2], (3, "Brano 3".to_string()));
        // scaricati solo l'1 e il 3
        std::fs::remove_file(&log).unwrap();
        run(
            vec![pick_input(
                "https://music.youtube.com/playlist?list=OLAK5uy_x",
                &[1, 3],
            )],
            cfg,
            tools,
            0,
            Arc::new(|_| {}),
            Arc::new(AtomicBool::new(false)),
            None,
            StopSet::default(),
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        let downloads: Vec<&str> = calls
            .lines()
            .filter(|l| !l.contains("--flat-playlist"))
            .collect();
        assert_eq!(downloads.len(), 2, "{calls}");
        assert!(downloads
            .iter()
            .any(|l| l.ends_with("watch?v=p1") && l.contains("01 - Brano 1.%(ext)s")));
        assert!(downloads
            .iter()
            .any(|l| l.ends_with("watch?v=p3") && l.contains("03 - Brano 3.%(ext)s")));
        assert_eq!(split_pick("https://a"), ("https://a".to_string(), None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn album_artist_from_the_track_channels() {
        let e = |c: Option<&str>| Entry {
            url: String::new(),
            title: String::new(),
            index: 1,
            channel: c.map(String::from),
        };
        let none = PlaylistInfo::default();
        let up = PlaylistInfo {
            uploader: Some("Francesco Renga - Topic".into()),
            ..PlaylistInfo::default()
        };
        // la maggioranza dei brani, senza « - Topic»
        assert_eq!(
            album_artist(
                &[
                    e(Some("Mina - Topic")),
                    e(Some("Mina - Topic")),
                    e(Some("Celentano"))
                ],
                &none
            ),
            Some("Mina".into())
        );
        // nessun canale: chi ha caricato la playlist
        assert_eq!(
            album_artist(&[e(None), e(None)], &up),
            Some("Francesco Renga".into())
        );
        // tutti diversi e nessun autore: artisti vari
        let various = album_artist(
            &[
                e(Some("A - Topic")),
                e(Some("B - Topic")),
                e(Some("C - Topic")),
            ],
            &none,
        )
        .unwrap();
        assert!(various == "Artisti vari" || various == "Various Artists");
        // nessuna informazione: decide yt-dlp
        assert_eq!(album_artist(&[e(None)], &none), None);
        // canali di chi ha caricato i video, non dell'artista: decide yt-dlp
        assert_eq!(
            album_artist(&[e(Some("Rengaofficial")), e(Some("Rengaofficial"))], &none),
            None
        );
        assert!(is_music_album(
            "https://music.youtube.com/browse/MPREb_x",
            &none
        ));
        assert!(!is_music_album(
            "https://www.youtube.com/playlist?list=PL1",
            &none
        ));
    }

    #[test]
    fn single_songs_use_the_metadata_folders() {
        let cfg = Config {
            mode: Mode::Audio,
            ..Config::default()
        };
        let a = build_args(
            &job("https://music.youtube.com/watch?v=x", None, false),
            &cfg,
            &toolset(),
        );
        let o = a.iter().position(|x| x == "-o").unwrap();
        assert!(
            a[o + 1].starts_with("%(album_artists.0,artists.0,artist,"),
            "{}",
            a[o + 1]
        );
        assert!(a[o + 1].contains("/%(album|"), "{}", a[o + 1]);
        assert!(
            a[o + 1].ends_with("/%(track,title).150B.%(ext)s"),
            "{}",
            a[o + 1]
        );
        // opzione spenta o video: come prima
        for cfg in [
            Config {
                music_folders: false,
                ..cfg.clone()
            },
            Config {
                mode: Mode::Video,
                ..cfg.clone()
            },
        ] {
            let a = build_args(
                &job("https://music.youtube.com/watch?v=x", None, false),
                &cfg,
                &toolset(),
            );
            assert!(has_pair(
                &a,
                "-o",
                "%(playlist_index&{} - |)s%(title).150B.%(ext)s"
            ));
        }
    }

    /// «Annulla» su una riga ferma solo quel brano; «Riprova» riscarica il lavoro con la
    /// stessa cartella e lo stesso nome del file.
    #[cfg(unix)]
    #[test]
    fn single_tracks_can_be_stopped_and_retried() {
        let _guard = fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        let dir = std::env::temp_dir().join(format!("grabbit-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let fake = dir.join("fake-yt-dlp");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nlast=\"\"; for a in \"$@\"; do last=\"$a\"; done\necho \"$*\" >> \"{}\"\necho \"GRBFILE|/x/$last\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Toolset {
            ytdlp: fake,
            ffmpeg_dir: None,
            js: None,
        };
        let cfg = Config {
            mode: Mode::Audio,
            out_dir: dir.clone(),
            parallel: 1,
            ..Config::default()
        };
        let finished = Arc::new(Mutex::new(Vec::<(usize, bool, String)>::new()));
        let specs = Arc::new(Mutex::new(Vec::<Job>::new()));
        let (f, sp) = (finished.clone(), specs.clone());
        let emit: Emit = Arc::new(move |e| match e {
            Event::JobFinished { id, ok, detail, .. } => f.lock().unwrap().push((id, ok, detail)),
            Event::JobSpec { job, .. } => sp.lock().unwrap().push(job),
            _ => {}
        });
        // il secondo link (id 1) è fermato prima di partire
        let stop: StopSet = Default::default();
        stop.lock().unwrap().insert(1);
        run(
            vec!["https://youtu.be/aaa".into(), "https://youtu.be/bbb".into()],
            cfg.clone(),
            tools.clone(),
            0,
            emit.clone(),
            Arc::new(AtomicBool::new(false)),
            None,
            stop,
        );
        let calls = || std::fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(calls().lines().count(), 1, "{}", calls());
        assert!(calls().contains("youtu.be/aaa"));
        {
            let f = finished.lock().unwrap();
            assert!(f.iter().any(|x| x.0 == 0 && x.1), "{f:?}");
            assert!(f.iter().any(|x| x.0 == 1 && !x.1), "{f:?}");
        }
        // «Riprova» sul brano fermato: lo stesso lavoro, con lo stesso id
        let spec = specs
            .lock()
            .unwrap()
            .iter()
            .find(|j| j.id == 1)
            .cloned()
            .unwrap();
        retry(
            vec![spec],
            cfg,
            tools,
            emit,
            Arc::new(AtomicBool::new(false)),
            None,
            Default::default(),
        );
        assert_eq!(calls().lines().count(), 2);
        assert!(calls().lines().nth(1).unwrap().ends_with("youtu.be/bbb"));
        assert!(finished
            .lock()
            .unwrap()
            .iter()
            .filter(|x| x.0 == 1)
            .any(|x| x.1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_entries_become_numbered_tracks() {
        let l = r#"{"id": "gVYHLmeHmCc", "url": "https://music.youtube.com/watch?v=gVYHLmeHmCc", "title": "Angelo (VENTI Edition)", "playlist_title": "Album - Scriverò il tuo nome - Live", "playlist_index": 1}"#;
        let (t, e) = parse_entry(l, 9).unwrap();
        assert_eq!(
            t.title.as_deref(),
            Some("Album - Scriverò il tuo nome - Live")
        );
        assert_eq!(
            e,
            Entry {
                url: "https://music.youtube.com/watch?v=gVYHLmeHmCc".into(),
                title: "Angelo (VENTI Edition)".into(),
                index: 1,
                channel: None,
            }
        );
        // senza url e senza indice: link dall'id, posizione nell'elenco
        let (t, e) = parse_entry(r#"{"id": "abcdefghijk", "title": "X"}"#, 3).unwrap();
        assert_eq!(t, PlaylistInfo::default());
        assert_eq!(e.url, "https://www.youtube.com/watch?v=abcdefghijk");
        assert_eq!(e.index, 3);
        assert!(parse_entry("[youtube:tab] testo", 1).is_none());
    }

    #[test]
    fn album_tracks_carry_their_track_number() {
        let cfg = Config {
            mode: Mode::Audio,
            ..Config::default()
        };
        let mut j = job(
            "https://music.youtube.com/watch?v=x",
            Some("7 - Titolo"),
            false,
        );
        j.track = Some(7);
        let a = build_args(&j, &cfg, &toolset());
        assert!(has_pair(&a, "--parse-metadata", "7:%(track_number)s"));
        assert!(has_pair(&a, "-o", "7 - Titolo.%(ext)s"));
        assert!(a.contains(&"--no-playlist".to_string()));
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
        // copertina quadrata, senza le bande della miniatura 16:9
        assert!(has_pair(&a, "--ppa", SQUARE_COVER));
        assert!(SQUARE_COVER.contains("crop=\"'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'\""));
        // anno di uscita, non data di caricamento
        assert!(has_pair(&a, "--parse-metadata", RELEASE_YEAR));
    }

    #[test]
    fn m4a_and_opus_prefer_the_original_youtube_audio() {
        for (fmt, sel) in [
            ("m4a", "bestaudio[ext=m4a]/bestaudio/best"),
            ("opus", "bestaudio[acodec=opus]/bestaudio/best"),
            ("mp3", "bestaudio/best"),
        ] {
            let cfg = Config {
                mode: Mode::Audio,
                audio_format: fmt.into(),
                ..Config::default()
            };
            let a = build_args(&job("https://youtu.be/x", None, false), &cfg, &toolset());
            assert!(has_pair(&a, "-f", sel), "{fmt}");
        }
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
            None,
            StopSet::default(),
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
                Event::JobRemoved { .. } => "removed".into(),
                Event::JobSpec { .. } => "spec".into(),
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
        run(
            vec![url],
            cfg,
            tools,
            0,
            emit,
            cancel,
            None,
            StopSet::default(),
        );
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
