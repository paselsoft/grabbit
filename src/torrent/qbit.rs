// SPDX-License-Identifier: GPL-3.0-or-later
//! Client dell'interfaccia web di qBittorrent (API v2, porta 8080).
//!
//! Grabbit non salva password: in qBittorrent va spuntato «Salta autenticazione per i client in
//! localhost» (Preferenze › Interfaccia web), con 127.0.0.1 come indirizzo.
use super::{magnet_name, App, Status, Torrent};
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8080;

pub struct QBittorrent {
    base: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct RawTorrent {
    hash: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    progress: f64,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    completed: u64,
    #[serde(default)]
    dlspeed: u64,
    #[serde(default)]
    upspeed: u64,
    #[serde(default)]
    eta: i64,
    #[serde(default)]
    num_seeds: u32,
    #[serde(default)]
    num_leechs: u32,
    /// seed e peer noti nello sciame (-1 se non si sa)
    #[serde(default = "minus_one")]
    num_complete: i64,
    #[serde(default = "minus_one")]
    num_incomplete: i64,
    #[serde(default)]
    save_path: String,
}

fn minus_one() -> i64 {
    -1
}

/// qBittorrent usa 8640000 secondi (100 giorni) per «tempo infinito».
const ETA_INFINITE: i64 = 8_640_000;

impl From<RawTorrent> for Torrent {
    fn from(r: RawTorrent) -> Self {
        let s = r.state.as_str();
        let status = match s {
            "downloading" | "forcedDL" | "stalledDL" | "metaDL" | "forcedMetaDL" => {
                Status::Downloading
            }
            "uploading" | "stalledUP" | "forcedUP" => Status::Seeding,
            "queuedDL" | "queuedUP" => Status::Queued,
            "checkingDL" | "checkingUP" | "checkingResumeData" | "allocating" | "moving" => {
                Status::Checking
            }
            // pausedDL/UP (4.x), stoppedDL/UP (5.x), error, missingFiles
            _ => Status::Stopped,
        };
        let error = match s {
            "error" => Some(tr!("errore di qBittorrent", "qBittorrent error").to_string()),
            "missingFiles" => Some(tr!("file mancanti", "missing files").to_string()),
            _ => None,
        };
        Torrent {
            app: App::QBittorrent,
            id: r.hash,
            name: r.name,
            status,
            progress: r.progress.clamp(0.0, 1.0) as f32,
            done_bytes: r.completed.min(r.size),
            total_bytes: r.size,
            down_rate: r.dlspeed,
            up_rate: r.upspeed,
            eta: (0..ETA_INFINITE).contains(&r.eta).then_some(r.eta as u64),
            seeds: r.num_seeds,
            peers: r.num_leechs,
            swarm_seeds: u32::try_from(r.num_complete).ok(),
            swarm_peers: u32::try_from(r.num_incomplete).ok(),
            dir: r.save_path,
            error,
            fetching_metadata: matches!(s, "metaDL" | "forcedMetaDL"),
        }
    }
}

impl QBittorrent {
    pub fn new(port: u16) -> Self {
        Self::with_base(format!("http://127.0.0.1:{port}"))
    }

    pub(super) fn with_base(base: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(crate::net::USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            // qBittorrent è su questo Mac: niente proxy
            .proxy(None)
            .build()
            .into();
        Self { base, agent }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v2/{path}", self.base)
    }

    /// Controlla lo stato della risposta e ne restituisce il testo.
    fn check(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, String)> {
        let mut resp = resp.map_err(|e| anyhow!(not_running(&e)))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        match status {
            200 => Ok((status, text)),
            403 => bail!(tr!(
                "qBittorrent chiede la password: in Preferenze › Interfaccia web spunta «Salta autenticazione per i client in localhost»",
                "qBittorrent asks for a password: in Preferences › Web UI tick “Bypass authentication for clients on localhost”"
            )),
            401 => bail!(tr!(
                "qBittorrent rifiuta il collegamento: in Preferenze › Interfaccia web metti 127.0.0.1 come indirizzo IP",
                "qBittorrent refuses the connection: in Preferences › Web UI set the IP address to 127.0.0.1"
            )),
            s => Ok((s, text)),
        }
    }

    fn get(&self, path: &str) -> Result<(u16, String)> {
        Self::check(self.agent.get(self.url(path)).call())
    }

    fn post(&self, path: &str, form: &[(&str, &str)]) -> Result<(u16, String)> {
        Self::check(
            self.agent
                .post(self.url(path))
                .send_form(form.iter().copied()),
        )
    }

    /// Aggiunge un magnet, un indirizzo .torrent o un file .torrent; torna il nome se si conosce.
    pub fn add(&self, source: &str, dir: &Path) -> Result<String> {
        let source = source.trim();
        let boundary = "----grabbit-7d1f3a9c2b";
        let mut body: Vec<u8> = Vec::new();
        let mut field = |name: &str, filename: Option<&str>, value: &[u8]| {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(f) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/x-bittorrent\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(value);
            body.extend_from_slice(b"\r\n");
        };
        let local = Path::new(source);
        let name;
        if !source.contains("://") && !source.starts_with("magnet:") && local.is_file() {
            let data = std::fs::read(local)
                .with_context(|| format!("non riesco a leggere {}", local.display()))?;
            let file = local
                .file_name()
                .map(|f| f.to_string_lossy().replace('"', ""))
                .unwrap_or_else(|| "file.torrent".into());
            name = file.trim_end_matches(".torrent").to_string();
            field("torrents", Some(&file), &data);
        } else {
            name = magnet_name(source).unwrap_or_default();
            field("urls", None, source.as_bytes());
        }
        field("savepath", None, dir.to_string_lossy().as_bytes());
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let (status, text) = Self::check(
            self.agent
                .post(self.url("torrents/add"))
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .send(&body[..]),
        )?;
        if status != 200 || text.trim() == "Fails." {
            bail!(tr!(
                format!(
                    "qBittorrent non ha aggiunto il torrent ({status} {})",
                    text.trim()
                ),
                format!(
                    "qBittorrent did not add the torrent ({status} {})",
                    text.trim()
                )
            ));
        }
        Ok(name)
    }

    pub fn list(&self) -> Result<Vec<Torrent>> {
        let (status, text) = self.get("torrents/info")?;
        if status != 200 {
            bail!(tr!(
                format!("qBittorrent ha risposto {status}"),
                format!("qBittorrent replied {status}")
            ));
        }
        let raw: Vec<RawTorrent> =
            serde_json::from_str(&text).context("risposta di qBittorrent non valida")?;
        Ok(raw.into_iter().map(Torrent::from).collect())
    }

    /// qBittorrent 5 chiama «stop»/«start» quello che nella 4 era «pause»/«resume».
    fn action(&self, new: &str, old: &str, hash: &str) -> Result<()> {
        let (status, _) = self.post(&format!("torrents/{new}"), &[("hashes", hash)])?;
        let status = if status == 404 {
            self.post(&format!("torrents/{old}"), &[("hashes", hash)])?
                .0
        } else {
            status
        };
        if status != 200 {
            bail!(tr!(
                format!("qBittorrent ha risposto {status}"),
                format!("qBittorrent replied {status}")
            ));
        }
        Ok(())
    }

    pub fn pause(&self, hash: &str) -> Result<()> {
        self.action("stop", "pause", hash)
    }

    pub fn resume(&self, hash: &str) -> Result<()> {
        self.action("start", "resume", hash)
    }

    /// Toglie il torrent da qBittorrent; i file scaricati restano.
    pub fn remove(&self, hash: &str) -> Result<()> {
        self.post(
            "torrents/delete",
            &[("hashes", hash), ("deleteFiles", "false")],
        )
        .map(|_| ())
    }

    /// Limite di velocità in download in KB/s (0 = nessun limite).
    pub fn set_speed_limit(&self, kbps: u32) -> Result<()> {
        let bytes = (kbps as u64 * 1024).to_string();
        self.post("transfer/setDownloadLimit", &[("limit", &bytes)])
            .map(|_| ())
    }
}

fn not_running(e: &ureq::Error) -> String {
    match e {
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::Timeout(_) => tr!(
            "qBittorrent non risponde: aprilo e attiva Preferenze › Interfaccia web (porta 8080)",
            "qBittorrent does not respond: open it and turn on Preferences › Web UI (port 8080)"
        )
        .to_string(),
        e => format!("qBittorrent: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    struct Req {
        path: String,
        body: String,
    }

    /// Finto qBittorrent 4.x: niente «stop»/«start» (404), elenco fisso, tiene le richieste.
    fn fake(status_all: Option<u16>) -> (QBittorrent, Arc<Mutex<Vec<Req>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                seen2.lock().unwrap().push(Req {
                    path: path.clone(),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let (code, text) = match (status_all, path.as_str()) {
                    (Some(c), _) => (c, String::new()),
                    (_, "/api/v2/torrents/stop" | "/api/v2/torrents/start") => (404, String::new()),
                    (_, "/api/v2/torrents/add") => (200, "Ok.".to_string()),
                    (_, "/api/v2/torrents/info") => (
                        200,
                        r#"[{"hash":"abc","name":"artix.iso","state":"stalledDL","progress":0.5,
                            "size":2000,"completed":1000,"dlspeed":0,"upspeed":10,"eta":8640000,
                            "num_seeds":6,"num_leechs":0,"num_complete":7,"num_incomplete":50,
                            "save_path":"/d"},
                           {"hash":"def","name":"","state":"metaDL","progress":0,
                            "eta":-1,"save_path":"/d"},
                           {"hash":"ghi","name":"x","state":"pausedUP","progress":1,
                            "size":5,"completed":5}]"#
                            .to_string(),
                    ),
                    _ => (200, String::new()),
                };
                let _ = conn.write_all(
                    format!(
                        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (QBittorrent::with_base(base), seen)
    }

    #[test]
    fn magnet_is_added_as_multipart_with_folder() {
        let (qb, seen) = fake(None);
        let name = qb
            .add(
                "magnet:?xt=urn:btih:abc&dn=Linux+Mint.iso",
                Path::new("/Users/paolo/Downloads"),
            )
            .unwrap();
        assert_eq!(name, "Linux Mint.iso");
        let s = seen.lock().unwrap();
        assert_eq!(s[0].path, "/api/v2/torrents/add");
        assert!(s[0]
            .body
            .contains("name=\"urls\"\r\n\r\nmagnet:?xt=urn:btih:abc&dn=Linux+Mint.iso\r\n"));
        assert!(s[0]
            .body
            .contains("name=\"savepath\"\r\n\r\n/Users/paolo/Downloads\r\n"));
    }

    #[test]
    fn local_torrent_file_is_uploaded() {
        let (qb, seen) = fake(None);
        let f = std::env::temp_dir().join(format!("grabbit-qb-{}.torrent", std::process::id()));
        std::fs::write(&f, b"d4:infoe").unwrap();
        qb.add(&f.to_string_lossy(), Path::new("/tmp")).unwrap();
        let _ = std::fs::remove_file(&f);
        let body = &seen.lock().unwrap()[0].body;
        assert!(body.contains("name=\"torrents\"; filename=\"grabbit-qb-"));
        assert!(body.contains("d4:infoe"));
        assert!(!body.contains("name=\"urls\""));
    }

    #[test]
    fn list_is_parsed() {
        let (qb, _) = fake(None);
        let l = qb.list().unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].app, App::QBittorrent);
        assert_eq!(l[0].id, "abc");
        assert_eq!(l[0].status, Status::Downloading);
        assert_eq!(l[0].done_bytes, 1000);
        assert_eq!(l[0].eta, None, "8640000 vuol dire infinito");
        // come la colonna di qBittorrent: «Seed 6 (7)», «Peer 0 (50)»
        assert_eq!((l[0].seeds, l[0].peers), (6, 0));
        assert_eq!((l[0].swarm_seeds, l[0].swarm_peers), (Some(7), Some(50)));
        assert_eq!(l[1].swarm_seeds, None, "campo mancante: non si sa");
        assert!(l[1].fetching_metadata);
        assert_eq!(l[2].status, Status::Stopped);
        assert_eq!(l[2].progress, 1.0);
    }

    #[test]
    fn old_versions_get_pause_and_resume() {
        let (qb, seen) = fake(None);
        qb.pause("abc").unwrap();
        qb.resume("abc").unwrap();
        qb.remove("abc").unwrap();
        qb.set_speed_limit(1024).unwrap();
        let s = seen.lock().unwrap();
        let paths: Vec<&str> = s.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/api/v2/torrents/stop",
                "/api/v2/torrents/pause",
                "/api/v2/torrents/start",
                "/api/v2/torrents/resume",
                "/api/v2/torrents/delete",
                "/api/v2/transfer/setDownloadLimit"
            ]
        );
        assert_eq!(s[1].body, "hashes=abc");
        assert_eq!(s[4].body, "hashes=abc&deleteFiles=false");
        assert_eq!(s[5].body, "limit=1048576");
    }

    #[test]
    fn password_request_explains_the_fix() {
        let (qb, _) = fake(Some(403));
        let e = qb.list().unwrap_err();
        assert!(format!("{e:#}").contains("localhost"), "{e:#}");
    }
}
