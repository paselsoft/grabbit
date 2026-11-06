// SPDX-License-Identifier: GPL-3.0-or-later
//! Aggiornamento automatico di Grabbit stesso.
//!
//! Legge l'ultima release del repository indicato in `Cargo.toml`
//! (campo `repository`) e, se la versione è più recente, scarica il file
//! giusto per la piattaforma e sostituisce l'eseguibile in uso.
//!
//! Nomi dei file che devi allegare a ogni release GitHub:
//!   • Windows : grabbit-windows-x86_64.exe
//!   • Linux   : grabbit-linux-x86_64.AppImage   (e -aarch64)
//!   • macOS   : grabbit-macos-aarch64 / grabbit-macos-x86_64  (binario nudo)
use crate::tr;
use crate::{net, tools};
use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: Option<String>,
}

/// Argomento con cui l'app si riavvia dopo un aggiornamento: in quella esecuzione l'aggiornamento
/// automatico è disattivato, così una release sbagliata non può causare un ciclo infinito.
pub const AFTER_UPDATE_ARG: &str = "--after-update";

/// "owner/repo" da un indirizzo GitHub; `None` finché c'è il segnaposto OWNER.
fn repo_from_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let r = url
        .split("github.com/")
        .nth(1)?
        .trim_matches('/')
        .to_string();
    (r.contains('/') && !r.starts_with("OWNER/")).then_some(r)
}

/// Repository GitHub in cui cercare le release. Nelle build di GitHub è quello stesso
/// repository (variabile GRABBIT_REPO impostata dal workflow); altrimenti vale il campo
/// `repository` di Cargo.toml.
pub fn repository() -> Option<String> {
    if let Some(r) = option_env!("GRABBIT_REPO") {
        let r = r.trim();
        if r.contains('/') && !r.starts_with("OWNER/") {
            return Some(r.to_string());
        }
    }
    repo_from_url(env!("CARGO_PKG_REPOSITORY"))
}

/// Indirizzo dell'API di GitHub. Nelle build di sviluppo si può sostituire (GRABBIT_API_BASE)
/// per provare l'aggiornamento con un finto server; nelle release questa possibilità non esiste.
fn api_base() -> String {
    if cfg!(debug_assertions) {
        if let Ok(v) = std::env::var("GRABBIT_API_BASE") {
            return v.trim_end_matches('/').to_string();
        }
    }
    "https://api.github.com".to_string()
}

pub fn asset_name() -> Option<String> {
    let arch = std::env::consts::ARCH; // x86_64 | aarch64
    Some(match std::env::consts::OS {
        "windows" => format!("grabbit-windows-{arch}.exe"),
        "linux" => format!("grabbit-linux-{arch}.AppImage"),
        "macos" => format!("grabbit-macos-{arch}"),
        _ => return None,
    })
}

/// Su Linux l'aggiornamento in-place funziona solo da AppImage; su macOS non funziona
/// quando il sistema avvia l'app da una copia in sola lettura (App Translocation).
pub fn supported() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE").is_some()
    } else {
        !is_translocated()
    }
}

/// macOS «App Translocation»: un'app scaricata da Internet e aperta senza averla spostata
/// con il Finder (ad esempio in Applicazioni) viene avviata da una copia in sola lettura in
/// una cartella nascosta (…/AppTranslocation/…). Da lì non può sostituire sé stessa.
pub fn is_translocated() -> bool {
    cfg!(target_os = "macos")
        && std::env::current_exe()
            .map(|p| path_is_translocated(&p.to_string_lossy()))
            .unwrap_or(false)
}

fn path_is_translocated(path: &str) -> bool {
    path.contains("/AppTranslocation/")
}

/// Perché l'aggiornamento automatico non è possibile su questo sistema (se non lo è).
pub fn unsupported_reason() -> String {
    if cfg!(target_os = "linux") {
        tr!(
            "Per aggiornarsi da sola, l'app va avviata dal file AppImage.",
            "To update itself, the app must be run from the AppImage file."
        )
        .into()
    } else {
        translocation_hint()
    }
}

/// Cosa fare quando macOS avvia Grabbit dalla copia in sola lettura.
pub fn translocation_hint() -> String {
    tr!(
        "macOS sta avviando Grabbit da una copia in sola lettura, quindi non può aggiornarsi da solo. Chiudi Grabbit, spostalo con il Finder nella cartella Applicazioni, nel Terminale scrivi «xattr -dr com.apple.quarantine /Applications/Grabbit.app» e riaprilo da Applicazioni.",
        "macOS is running Grabbit from a read-only copy, so it cannot update itself. Quit Grabbit, move it with the Finder into the Applications folder, run “xattr -dr com.apple.quarantine /Applications/Grabbit.app” in Terminal and open it again from Applications."
    )
    .into()
}

/// Messaggio per un aggiornamento non riuscito, con il rimedio quando è noto.
pub fn describe_apply_error(e: &str) -> String {
    let read_only = e.contains("Read-only file system") || e.contains("os error 30");
    if read_only && (path_is_translocated(e) || cfg!(target_os = "macos")) {
        format!("{e}\n\n{}", translocation_hint())
    } else {
        e.to_string()
    }
}

/// Esito del controllo degli aggiornamenti.
#[derive(Clone, Debug)]
pub enum Check {
    /// repository non impostato: non si sa dove cercare
    NotConfigured,
    /// il repository non ha ancora nessuna release (o è privato)
    NoRelease,
    UpToDate,
    Newer(Release),
}

pub fn check() -> Result<Check> {
    let Some(repo) = repository() else {
        return Ok(Check::NotConfigured);
    };
    let url = format!("{}/repos/{repo}/releases/latest", api_base());
    let v = match net::get_json(&url) {
        Ok(v) => v,
        Err(e) if status_code(&e) == Some(404) => return Ok(Check::NoRelease),
        Err(e) => return Err(e),
    };
    Ok(match parse_release(&v, CURRENT)? {
        Some(r) => Check::Newer(r),
        None => Check::UpToDate,
    })
}

fn status_code(e: &anyhow::Error) -> Option<u16> {
    e.chain()
        .find_map(|c| match c.downcast_ref::<ureq::Error>() {
            Some(ureq::Error::StatusCode(code)) => Some(*code),
            _ => None,
        })
}

/// Messaggio comprensibile per un errore del controllo.
pub fn describe_error(e: &anyhow::Error) -> String {
    match status_code(e) {
        Some(403) | Some(429) => tr!(
            "GitHub ha limitato le richieste, riprova tra qualche minuto",
            "GitHub rate-limited the requests, try again in a few minutes"
        )
        .into(),
        Some(code) => tr!(
            format!("GitHub ha risposto con errore {code}"),
            format!("GitHub replied with error {code}")
        ),
        None => format!("{e:#}"),
    }
}

/// Note da mostrare nell'app: tolgono la riga di credito che sta solo nella pagina GitHub.
fn clean_notes(body: &str) -> String {
    body.lines()
        .filter(|l| {
            !l.contains("è stato scritto con l'aiuto di")
                && !l.contains("was written with the help of")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Confronta la release GitHub con la versione in uso.
fn parse_release(v: &serde_json::Value, current: &str) -> Result<Option<Release>> {
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow!(tr!("release senza tag", "release without a tag")))?;
    let latest = semver::Version::parse(tag.trim_start_matches('v'))?;
    if latest <= semver::Version::parse(current)? {
        return Ok(None);
    }
    let want = asset_name();
    let asset_url = v["assets"]
        .as_array()
        .and_then(|list| list.iter().find(|a| a["name"].as_str() == want.as_deref()))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(String::from);
    Ok(Some(Release {
        version: latest.to_string(),
        notes: clean_notes(v["body"].as_str().unwrap_or("")),
        page_url: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_url,
    }))
}

#[cfg(unix)]
fn make_executable(p: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = fs::metadata(p)?.permissions();
    perm.set_mode(0o755);
    Ok(fs::set_permissions(p, perm)?)
}
#[cfg(not(unix))]
fn make_executable(_: &std::path::Path) -> Result<()> {
    Ok(())
}

pub fn apply(rel: &Release, status: &mut dyn FnMut(String)) -> Result<()> {
    let url = rel.asset_url.as_ref().ok_or_else(|| {
        anyhow!(tr!(
            "la release non contiene il file per questo sistema",
            "the release does not contain the file for this system"
        ))
    })?;
    let name =
        asset_name().ok_or_else(|| anyhow!(tr!("sistema non supportato", "unsupported system")))?;
    let tmp = tools::data_dir().join("update").join(name);
    net::download(url, &tmp, &mut |d, t| {
        let p = t.filter(|t| *t > 0).map(|t| d * 100 / t).unwrap_or(0);
        status(tr!(
            format!("Scarico Grabbit {}… {p}%", rel.version),
            format!("Downloading Grabbit {}… {p}%", rel.version)
        ));
    })?;
    make_executable(&tmp)?;

    #[cfg(target_os = "linux")]
    {
        let appimage = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow!(tr!(
                    "l'aggiornamento automatico richiede la versione AppImage",
                    "automatic updates require the AppImage version"
                ))
            })?;
        // si copia accanto all'AppImage e poi si rinomina (atomico, anche se in uso)
        let staged = appimage.with_extension("new");
        fs::copy(&tmp, &staged)?;
        make_executable(&staged)?;
        fs::rename(&staged, &appimage)?;
        let _ = fs::remove_file(&tmp);
    }
    #[cfg(not(target_os = "linux"))]
    {
        self_replace::self_replace(&tmp)?;
        let _ = fs::remove_file(&tmp);
    }
    Ok(())
}

/// Riavvia l'app (dopo un aggiornamento).
pub fn restart() -> ! {
    let exe: Option<PathBuf> = if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
    } else {
        std::env::current_exe().ok()
    };
    if let Some(e) = exe {
        let _ = std::process::Command::new(e).arg(AFTER_UPDATE_ARG).spawn();
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release_json(tag: &str) -> serde_json::Value {
        json!({
            "tag_name": tag, "body": "novità", "html_url": "https://example.org/r",
            "assets": [
                {"name": "qualcosa-altro", "browser_download_url": "https://example.org/other"},
                {"name": asset_name().unwrap(), "browser_download_url": "https://example.org/asset"}
            ]
        })
    }

    #[test]
    fn newer_release_picks_platform_asset() {
        let r = parse_release(&release_json("v9.9.9"), "0.1.0")
            .unwrap()
            .unwrap();
        assert_eq!(r.version, "9.9.9");
        assert_eq!(r.asset_url.as_deref(), Some("https://example.org/asset"));
        assert_eq!(r.notes, "novità");
    }

    #[test]
    fn credit_line_is_hidden_in_app_notes() {
        let body = "Questo programma è stato scritto con l'aiuto di [Claude](https://claude.ai), l'assistente AI di Anthropic.\n\n## Cosa cambia\n* Tema scuro";
        assert_eq!(clean_notes(body), "## Cosa cambia\n* Tema scuro");
        assert_eq!(
            clean_notes("solo credito: è stato scritto con l'aiuto di X"),
            ""
        );
        assert_eq!(clean_notes("novità"), "novità");
    }

    #[test]
    fn same_or_older_release_is_ignored() {
        assert!(parse_release(&release_json("v0.1.0"), "0.1.0")
            .unwrap()
            .is_none());
        assert!(parse_release(&release_json("v0.0.9"), "0.1.0")
            .unwrap()
            .is_none());
        assert!(parse_release(&release_json("0.1.1"), "0.1.0")
            .unwrap()
            .is_some()); // tag senza "v"
        assert!(parse_release(&release_json("v0.10.0"), "0.9.0")
            .unwrap()
            .is_some()); // non è un confronto di stringhe
    }

    #[test]
    fn release_without_matching_asset() {
        let v = json!({"tag_name": "v2.0.0", "assets": []});
        let r = parse_release(&v, "0.1.0").unwrap().unwrap();
        assert!(r.asset_url.is_none());
    }

    #[test]
    fn recognises_macos_app_translocation() {
        assert!(path_is_translocated(
            "/private/var/folders/rn/x/T/AppTranslocation/1B47BC18/d/Grabbit.app/Contents/MacOS/grabbit"
        ));
        assert!(!path_is_translocated(
            "/Applications/Grabbit.app/Contents/MacOS/grabbit"
        ));
        let msg = describe_apply_error(
            "Read-only file system (os error 30) at path \"/private/var/folders/rn/x/T/AppTranslocation/1B/d/Grabbit.app/Contents/MacOS/.grabbit\"",
        );
        assert!(msg.contains("com.apple.quarantine"), "{msg}");
        assert_eq!(
            describe_apply_error("rete non raggiungibile"),
            "rete non raggiungibile"
        );
    }

    #[test]
    fn repository_from_url() {
        assert_eq!(repo_from_url("https://github.com/OWNER/grabbit"), None); // segnaposto
        assert_eq!(
            repo_from_url("https://github.com/mario/grabbit"),
            Some("mario/grabbit".into())
        );
        assert_eq!(
            repo_from_url("https://github.com/mario/grabbit.git"),
            Some("mario/grabbit".into())
        );
        assert_eq!(
            repo_from_url("https://github.com/mario/grabbit/"),
            Some("mario/grabbit".into())
        );
        assert_eq!(repo_from_url("https://example.org/x"), None);
        assert_eq!(repo_from_url(""), None);
    }

    #[test]
    fn errors_are_described() {
        let e = anyhow::Error::new(ureq::Error::StatusCode(403));
        assert!(describe_error(&e).contains(tr!("limitato", "rate-limited")));
        let e = anyhow::Error::new(ureq::Error::StatusCode(500));
        assert!(describe_error(&e).contains("500"));
        assert_eq!(status_code(&anyhow::anyhow!("altro")), None);
    }

    /// GRABBIT_TEST_ASSET_URL=http://127.0.0.1:8767/grabbit-linux-x86_64.AppImage cargo test apply_ -- --ignored
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn apply_replaces_appimage() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(url) = std::env::var("GRABBIT_TEST_ASSET_URL") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("grabbit-upd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("Grabbit.AppImage");
        fs::write(&fake, b"VECCHIA").unwrap();
        std::env::set_var("APPIMAGE", &fake);
        let rel = Release {
            version: "9.9.9".into(),
            notes: String::new(),
            page_url: String::new(),
            asset_url: Some(url),
        };
        let mut msgs = Vec::new();
        apply(&rel, &mut |s| msgs.push(s)).unwrap();
        assert_eq!(fs::read(&fake).unwrap(), b"NUOVA-VERSIONE");
        assert_eq!(
            fs::metadata(&fake).unwrap().permissions().mode() & 0o111,
            0o111
        );
        assert!(
            !dir.join("Grabbit.new").exists(),
            "file temporaneo lasciato in giro"
        );
        assert!(!msgs.is_empty());
    }
}
