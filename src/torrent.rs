// SPDX-License-Identifier: GPL-3.0-or-later
//! Torrent e link magnet tramite Transmission o qBittorrent già installati sul Mac.
//!
//! Grabbit non contiene un motore torrent (libtorrent in C++ non parte su High Sierra):
//! comanda Transmission con la sua interfaccia remota (Preferenze › Remoto › «Abilita accesso
//! remoto», porta 9091) e qBittorrent con la sua interfaccia web (porta 8080). Se l'app scelta
//! non risponde, il link si apre con l'app predefinita per i torrent.
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

mod qbit;
pub use qbit::{QBittorrent, DEFAULT_PORT as QBIT_PORT};

pub const DEFAULT_PORT: u16 = 9091;

/// L'app torrent che fa il lavoro.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum App {
    #[default]
    Transmission,
    QBittorrent,
}

impl App {
    pub const ALL: [App; 2] = [App::Transmission, App::QBittorrent];

    pub fn label(self) -> &'static str {
        match self {
            App::Transmission => "Transmission",
            App::QBittorrent => "qBittorrent",
        }
    }

    /// Apre l'app (solo Mac).
    pub fn launch(self) {
        if cfg!(target_os = "macos") {
            let _ = std::process::Command::new("open")
                .args(["-a", self.label()])
                .spawn();
        }
    }
}

/// Un magnet, un indirizzo che finisce in .torrent o un file .torrent sul disco.
pub fn is_torrent_source(s: &str) -> bool {
    let t = s.trim();
    if t.len() > 7 && t[..7].eq_ignore_ascii_case("magnet:") {
        return true;
    }
    let path = t.split(['?', '#']).next().unwrap_or("");
    path.to_ascii_lowercase().ends_with(".torrent")
}

/// Stato di un torrent, uguale per le due app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Stopped,
    Checking,
    Queued,
    Downloading,
    Seeding,
}

#[derive(Clone, Debug)]
pub struct Torrent {
    pub app: App,
    /// numero in Transmission, hash in qBittorrent
    pub id: String,
    pub name: String,
    pub status: Status,
    /// 0–1
    pub progress: f32,
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// byte al secondo
    pub down_rate: u64,
    pub up_rate: u64,
    /// secondi, None se sconosciuto
    pub eta: Option<u64>,
    pub peers: u32,
    pub dir: String,
    pub error: Option<String>,
    /// i metadati di un magnet non sono ancora arrivati
    pub fetching_metadata: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTorrent {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: i64,
    #[serde(default)]
    percent_done: f64,
    #[serde(default)]
    size_when_done: u64,
    #[serde(default)]
    left_until_done: u64,
    #[serde(default)]
    rate_download: u64,
    #[serde(default)]
    rate_upload: u64,
    #[serde(default)]
    eta: i64,
    #[serde(default)]
    peers_connected: u32,
    #[serde(default)]
    download_dir: String,
    #[serde(default)]
    error: i64,
    #[serde(default)]
    error_string: String,
    #[serde(default)]
    metadata_percent_complete: Option<f64>,
}

impl From<RawTorrent> for Torrent {
    fn from(r: RawTorrent) -> Self {
        // stati di Transmission: 0 fermo, 1-2 verifica, 3 in coda, 4 scarica, 5-6 condivide
        let status = match r.status {
            1 | 2 => Status::Checking,
            3 => Status::Queued,
            4 => Status::Downloading,
            5 | 6 => Status::Seeding,
            _ => Status::Stopped,
        };
        Torrent {
            app: App::Transmission,
            id: r.id.to_string(),
            name: r.name,
            status,
            progress: r.percent_done.clamp(0.0, 1.0) as f32,
            done_bytes: r.size_when_done.saturating_sub(r.left_until_done),
            total_bytes: r.size_when_done,
            down_rate: r.rate_download,
            up_rate: r.rate_upload,
            eta: u64::try_from(r.eta).ok(),
            peers: r.peers_connected,
            dir: r.download_dir,
            error: (r.error != 0 && !r.error_string.is_empty()).then_some(r.error_string),
            fetching_metadata: r.metadata_percent_complete.is_some_and(|m| m < 1.0),
        }
    }
}

/// Client dell'interfaccia remota di Transmission (JSON su HTTP, con l'intestazione di sessione).
pub struct Transmission {
    url: String,
    agent: ureq::Agent,
    session: Mutex<Option<String>>,
}

const SESSION_HEADER: &str = "X-Transmission-Session-Id";

impl Transmission {
    pub fn new(port: u16) -> Self {
        Self::with_url(format!("http://127.0.0.1:{port}/transmission/rpc"))
    }

    fn with_url(url: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(crate::net::USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            // Transmission è su questo Mac: niente proxy
            .proxy(None)
            .build()
            .into();
        Self {
            url,
            agent,
            session: Mutex::new(None),
        }
    }

    fn call(&self, method: &str, arguments: Value) -> Result<Value> {
        let body = json!({ "method": method, "arguments": arguments });
        // la prima risposta è 409 con l'identificativo di sessione: si ripete con quello
        for _ in 0..2 {
            let mut req = self.agent.post(&self.url);
            if let Some(id) = self
                .session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                req = req.header(SESSION_HEADER, id);
            }
            let mut resp = req.send_json(&body).map_err(|e| anyhow!(not_running(&e)))?;
            let status = resp.status().as_u16();
            if status == 409 {
                let id = resp
                    .headers()
                    .get(SESSION_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                *self.session.lock().unwrap_or_else(|e| e.into_inner()) = id;
                continue;
            }
            if status == 401 {
                bail!(tr!(
                    "Transmission chiede nome utente e password: in Preferenze › Remoto togli «Richiedi autenticazione» e lascia solo 127.0.0.1 negli indirizzi consentiti",
                    "Transmission asks for a username and password: in Preferences › Remote turn off “Require authentication” and allow only 127.0.0.1"
                ));
            }
            if status == 403 {
                bail!(tr!(
                    "Transmission rifiuta il collegamento: in Preferenze › Remoto aggiungi 127.0.0.1 agli indirizzi consentiti",
                    "Transmission refuses the connection: in Preferences › Remote add 127.0.0.1 to the allowed addresses"
                ));
            }
            if status != 200 {
                bail!(tr!(
                    format!("Transmission ha risposto {status}"),
                    format!("Transmission replied {status}")
                ));
            }
            let v: Value = resp
                .body_mut()
                .read_json()
                .context("risposta di Transmission non valida")?;
            let result = v.get("result").and_then(Value::as_str).unwrap_or("");
            if result != "success" {
                bail!("Transmission: {result}");
            }
            return Ok(v.get("arguments").cloned().unwrap_or(Value::Null));
        }
        bail!(tr!(
            "Transmission non accetta la sessione",
            "Transmission does not accept the session"
        ))
    }

    /// Aggiunge un magnet, un indirizzo .torrent o un file .torrent; torna il nome.
    pub fn add(&self, source: &str, dir: &Path) -> Result<String> {
        let source = source.trim();
        let mut args = json!({ "download-dir": dir.to_string_lossy() });
        let local = Path::new(source);
        if !source.contains("://") && !source.starts_with("magnet:") && local.is_file() {
            let data = std::fs::read(local)
                .with_context(|| format!("non riesco a leggere {}", local.display()))?;
            args["metainfo"] = Value::String(base64(&data));
        } else {
            args["filename"] = Value::String(source.to_string());
        }
        let r = self.call("torrent-add", args)?;
        let t = r
            .get("torrent-added")
            .or_else(|| r.get("torrent-duplicate"))
            .ok_or_else(|| anyhow!("Transmission non ha aggiunto il torrent"))?;
        let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
        Ok(if name.is_empty() {
            magnet_name(source).unwrap_or_default()
        } else {
            name.to_string()
        })
    }

    pub fn list(&self) -> Result<Vec<Torrent>> {
        let r = self.call(
            "torrent-get",
            json!({ "fields": [
                "id", "name", "status", "percentDone", "sizeWhenDone", "leftUntilDone",
                "rateDownload", "rateUpload", "eta", "peersConnected", "downloadDir",
                "error", "errorString", "metadataPercentComplete"
            ]}),
        )?;
        let raw: Vec<RawTorrent> =
            serde_json::from_value(r.get("torrents").cloned().unwrap_or(json!([])))?;
        Ok(raw.into_iter().map(Torrent::from).collect())
    }

    pub fn pause(&self, id: i64) -> Result<()> {
        self.call("torrent-stop", json!({ "ids": [id] }))
            .map(|_| ())
    }

    pub fn resume(&self, id: i64) -> Result<()> {
        self.call("torrent-start", json!({ "ids": [id] }))
            .map(|_| ())
    }

    /// Toglie il torrent da Transmission; i file scaricati restano.
    pub fn remove(&self, id: i64) -> Result<()> {
        self.call(
            "torrent-remove",
            json!({ "ids": [id], "delete-local-data": false }),
        )
        .map(|_| ())
    }

    /// Limite di velocità in download in KB/s (0 = togli il limite).
    pub fn set_speed_limit(&self, kbps: u32) -> Result<()> {
        let args = if kbps > 0 {
            json!({ "speed-limit-down": kbps, "speed-limit-down-enabled": true })
        } else {
            json!({ "speed-limit-down-enabled": false })
        };
        self.call("session-set", args).map(|_| ())
    }
}

/// Il nome scritto nel magnet (`dn=`), finché Transmission non ha i dati del torrent.
pub fn magnet_name(source: &str) -> Option<String> {
    let query = source.trim().split_once('?')?.1;
    let dn = query.split('&').find_map(|kv| kv.strip_prefix("dn="))?;
    let bytes = dn.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    let name = String::from_utf8_lossy(&out).trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn not_running(e: &ureq::Error) -> String {
    match e {
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::Timeout(_) => tr!(
            "Transmission non risponde: aprilo e attiva Preferenze › Remoto › «Abilita accesso remoto» (porta 9091)",
            "Transmission does not respond: open it and turn on Preferences › Remote › “Enable remote access” (port 9091)"
        )
        .to_string(),
        e => format!("Transmission: {e}"),
    }
}

/// Apre il link o il file con l'app predefinita per i torrent (quando Transmission non risponde).
pub fn open_with_default_app(source: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(source.trim()).spawn().map(|_| ())
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ───────────────────────────── lavoratore in sottofondo ─────────────────────────────

/// Comandi dall'interfaccia: le chiamate alle app non bloccano mai la finestra.
pub enum Cmd {
    Add(Vec<String>, std::path::PathBuf, App),
    Pause(App, String),
    Resume(App, String),
    Remove(App, String),
    /// vale per tutte e due le app (quella spenta si ignora)
    SpeedLimit(u32),
    Refresh,
}

/// Elenco di ogni app, oppure il motivo per cui non risponde.
pub type AppLists = Vec<(App, Result<Vec<Torrent>, String>)>;

/// Notizie per l'interfaccia.
pub enum Update {
    List(AppLists),
    /// esito dell'aggiunta: (aggiunti all'app, aperti con l'app predefinita, errori)
    Added {
        app: App,
        added: Vec<String>,
        opened: usize,
        errors: Vec<String>,
    },
    Error(String),
}

/// Le due app con la stessa interfaccia.
struct Clients {
    tm: Transmission,
    qb: QBittorrent,
}

impl Clients {
    fn add(&self, app: App, source: &str, dir: &Path) -> Result<String> {
        match app {
            App::Transmission => self.tm.add(source, dir),
            App::QBittorrent => self.qb.add(source, dir),
        }
    }

    fn list(&self, app: App) -> Result<Vec<Torrent>> {
        match app {
            App::Transmission => self.tm.list(),
            App::QBittorrent => self.qb.list(),
        }
    }

    fn act(&self, cmd: &Cmd) -> Result<()> {
        let tm_id = |id: &str| {
            id.parse::<i64>()
                .map_err(|_| anyhow!("id non valido: {id}"))
        };
        match cmd {
            Cmd::Pause(App::Transmission, id) => self.tm.pause(tm_id(id)?),
            Cmd::Resume(App::Transmission, id) => self.tm.resume(tm_id(id)?),
            Cmd::Remove(App::Transmission, id) => self.tm.remove(tm_id(id)?),
            Cmd::Pause(App::QBittorrent, id) => self.qb.pause(id),
            Cmd::Resume(App::QBittorrent, id) => self.qb.resume(id),
            Cmd::Remove(App::QBittorrent, id) => self.qb.remove(id),
            Cmd::SpeedLimit(k) => {
                // un errore vero di una delle due app si mostra; un'app spenta no
                let a = self.tm.set_speed_limit(*k);
                let b = self.qb.set_speed_limit(*k);
                [a, b]
                    .into_iter()
                    .find(|r| r.as_ref().is_err_and(|e| !is_not_running(e)))
                    .unwrap_or(Ok(()))
            }
            Cmd::Add(..) | Cmd::Refresh => Ok(()),
        }
    }
}

/// Avvia il lavoratore. Rilegge gli elenchi ogni 2 s mentre `watching()` è vero (scheda «File»
/// aperta o torrent in corso), altrimenti ogni 30 s.
pub fn spawn(
    tm_port: u16,
    qb_port: u16,
    rx: Receiver<Cmd>,
    watching: impl Fn() -> bool + Send + 'static,
    send: impl Fn(Update) + Send + 'static,
) {
    std::thread::spawn(move || {
        let c = Clients {
            tm: Transmission::new(tm_port),
            qb: QBittorrent::new(qb_port),
        };
        let mut active = false;
        let mut last: Option<std::time::Instant> = None;
        loop {
            let every = if watching() || active {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(30)
            };
            // si rilegge subito all'avvio e appena si apre la scheda «File»
            let due = last.is_none_or(|t| t.elapsed() >= every);
            let cmd = if due {
                None
            } else {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            match cmd {
                Some(Cmd::Add(sources, dir, app)) => {
                    let (mut added, mut opened, mut errors) = (Vec::new(), 0, Vec::new());
                    for s in sources {
                        match c.add(app, &s, &dir) {
                            Ok(name) => added.push(name),
                            // l'app è spenta: ci pensa l'app predefinita per i torrent
                            Err(e) if is_not_running(&e) => match open_with_default_app(&s) {
                                Ok(()) => opened += 1,
                                Err(e2) => errors.push(format!("{e:#} ({e2})")),
                            },
                            Err(e) => errors.push(format!("{e:#}")),
                        }
                    }
                    send(Update::Added {
                        app,
                        added,
                        opened,
                        errors,
                    });
                }
                Some(cmd) => report(&send, c.act(&cmd)),
                None => {}
            }
            last = Some(std::time::Instant::now());
            let lists: Vec<_> = App::ALL
                .into_iter()
                .map(|app| (app, c.list(app).map_err(|e| format!("{e:#}"))))
                .collect();
            active = lists.iter().any(|(_, l)| {
                l.as_ref().is_ok_and(|l| {
                    l.iter()
                        .any(|t| t.status != Status::Stopped && t.progress < 1.0)
                })
            });
            send(Update::List(lists));
        }
    });
}

pub fn is_not_running_text(m: &str) -> bool {
    m.contains("non risponde") || m.contains("does not respond")
}

fn is_not_running(e: &anyhow::Error) -> bool {
    is_not_running_text(&format!("{e:#}"))
}

fn report(send: &impl Fn(Update), r: Result<()>) {
    // Transmission spento: lo dice già il riquadro, niente messaggio in più
    if let Err(e) = r.map_or_else(|e| if is_not_running(&e) { Ok(()) } else { Err(e) }, Ok) {
        send(Update::Error(format!("{e:#}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    #[test]
    fn torrent_sources_are_recognized() {
        assert!(is_torrent_source("magnet:?xt=urn:btih:abc"));
        assert!(is_torrent_source("  MAGNET:?xt=urn:btih:abc "));
        assert!(is_torrent_source("https://x.org/iso/artix.torrent"));
        assert!(is_torrent_source("https://x.org/a.TORRENT?dl=1"));
        assert!(is_torrent_source("/Users/paolo/Downloads/ubuntu.torrent"));
        assert!(!is_torrent_source("https://x.org/artix.iso"));
        assert!(!is_torrent_source("magnet"));
        assert!(!is_torrent_source("https://youtu.be/x"));
    }

    #[test]
    fn magnet_name_is_decoded() {
        assert_eq!(
            magnet_name("magnet:?xt=urn:btih:abc&dn=Artix+XFCE%20%C3%A8.iso&tr=x").as_deref(),
            Some("Artix XFCE è.iso")
        );
        assert_eq!(magnet_name("magnet:?xt=urn:btih:abc"), None);
        assert_eq!(magnet_name("magnet:?dn=50%"), Some("50%".into()));
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    /// Finto Transmission: la prima richiesta senza sessione riceve 409, poi risponde
    /// secondo il metodo; tiene le richieste ricevute.
    fn fake(responses: Vec<(&'static str, Value)>) -> (Transmission, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/transmission/rpc", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let (mut len, mut has_session) = (0usize, false);
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let l = h.to_ascii_lowercase();
                    if let Some(v) = l.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if l.starts_with("x-transmission-session-id: abc") {
                        has_session = true;
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                if !has_session {
                    conn.write_all(
                        b"HTTP/1.1 409 Conflict\r\nX-Transmission-Session-Id: abc\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
                    continue;
                }
                let req: Value = serde_json::from_slice(&body).unwrap();
                let method = req["method"].as_str().unwrap_or("").to_string();
                seen2.lock().unwrap().push(req);
                let args = responses
                    .iter()
                    .find(|(m, _)| *m == method)
                    .map(|(_, a)| a.clone())
                    .unwrap_or(json!({}));
                let out = json!({ "result": "success", "arguments": args }).to_string();
                let _ = conn.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    )
                    .as_bytes(),
                );
            }
        });
        (Transmission::with_url(url), seen)
    }

    #[test]
    fn magnet_is_added_with_session_and_folder() {
        let (tm, seen) = fake(vec![(
            "torrent-add",
            json!({ "torrent-added": { "id": 1, "name": "artix.iso" } }),
        )]);
        let name = tm
            .add(
                "magnet:?xt=urn:btih:abc",
                Path::new("/Users/paolo/Downloads"),
            )
            .unwrap();
        assert_eq!(name, "artix.iso");
        let req = &seen.lock().unwrap()[0];
        assert_eq!(req["method"], "torrent-add");
        assert_eq!(req["arguments"]["filename"], "magnet:?xt=urn:btih:abc");
        assert_eq!(req["arguments"]["download-dir"], "/Users/paolo/Downloads");
    }

    #[test]
    fn local_torrent_file_is_sent_as_metainfo() {
        let (tm, seen) = fake(vec![(
            "torrent-add",
            json!({ "torrent-duplicate": { "id": 2, "name": "già presente" } }),
        )]);
        let f = std::env::temp_dir().join(format!("grabbit-{}.torrent", std::process::id()));
        std::fs::write(&f, b"foobar").unwrap();
        let name = tm.add(&f.to_string_lossy(), Path::new("/tmp")).unwrap();
        let _ = std::fs::remove_file(&f);
        assert_eq!(name, "già presente");
        let req = &seen.lock().unwrap()[0];
        assert_eq!(req["arguments"]["metainfo"], "Zm9vYmFy");
        assert!(req["arguments"].get("filename").is_none());
    }

    #[test]
    fn list_is_parsed() {
        let (tm, _) = fake(vec![(
            "torrent-get",
            json!({ "torrents": [
                { "id": 1, "name": "artix.iso", "status": 4, "percentDone": 0.25,
                  "sizeWhenDone": 2000, "leftUntilDone": 1500, "rateDownload": 100,
                  "rateUpload": 5, "eta": 15, "peersConnected": 7,
                  "downloadDir": "/d", "error": 0, "errorString": "",
                  "metadataPercentComplete": 1.0 },
                { "id": 2, "name": "magnet", "status": 0, "percentDone": 0.0,
                  "eta": -1, "error": 3, "errorString": "tracker",
                  "metadataPercentComplete": 0.0 }
            ]}),
        )]);
        let l = tm.list().unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].status, Status::Downloading);
        assert_eq!(l[0].done_bytes, 500);
        assert_eq!(l[0].eta, Some(15));
        assert_eq!(l[0].peers, 7);
        assert!(l[0].error.is_none() && !l[0].fetching_metadata);
        assert_eq!(l[1].status, Status::Stopped);
        assert_eq!(l[1].eta, None);
        assert_eq!(l[1].error.as_deref(), Some("tracker"));
        assert!(l[1].fetching_metadata);
    }

    #[test]
    fn speed_limit_and_actions_send_the_right_methods() {
        let (tm, seen) = fake(vec![]);
        tm.set_speed_limit(1024).unwrap();
        tm.set_speed_limit(0).unwrap();
        tm.pause(3).unwrap();
        tm.resume(3).unwrap();
        tm.remove(3).unwrap();
        let s = seen.lock().unwrap();
        assert_eq!(s[0]["arguments"]["speed-limit-down"], 1024);
        assert_eq!(s[0]["arguments"]["speed-limit-down-enabled"], true);
        assert_eq!(s[1]["arguments"]["speed-limit-down-enabled"], false);
        let methods: Vec<&str> = s.iter().map(|r| r["method"].as_str().unwrap()).collect();
        assert_eq!(
            methods,
            [
                "session-set",
                "session-set",
                "torrent-stop",
                "torrent-start",
                "torrent-remove"
            ]
        );
        assert_eq!(s[4]["arguments"]["delete-local-data"], false);
    }

    #[test]
    fn closed_port_says_transmission_is_not_running() {
        // una porta appena liberata: nessuno ascolta
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let e = Transmission::new(port).list().unwrap_err();
        assert!(is_not_running(&e), "{e:#}");
    }
}
