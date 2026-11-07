// SPDX-License-Identifier: GPL-3.0-or-later
//! Gestione degli strumenti esterni: yt-dlp, Deno (runtime JS richiesto da
//! yt-dlp per YouTube) e ffmpeg. Vengono scaricati nella cartella dati
//! dell'app e tenuti aggiornati automaticamente.
use crate::downloader::JsRuntime;
use crate::net;
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use directories::ProjectDirs;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("org", "Grabbit", "Grabbit")
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("grabbit"))
}

pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

pub fn ytdlp_path() -> PathBuf {
    bin_dir().join(format!("yt-dlp{EXE}"))
}

pub fn deno_path() -> PathBuf {
    bin_dir().join(format!("deno{EXE}"))
}

/// `Command` che su Windows non apre una finestra console.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "macos")]
    c.env("PATH", macos_path());
    c
}

/// Le app aperte dal Finder ricevono un PATH minimo (/usr/bin:/bin:…): senza le cartelle
/// dove si installano Python (python.org) e Homebrew, la versione di yt-dlp per Python
/// (che si avvia con `#!/usr/bin/env python3`) non troverebbe python3.
#[cfg(target_os = "macos")]
fn macos_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        "/usr/local/bin",
        "/opt/homebrew/bin",
        "/Library/Frameworks/Python.framework/Versions/Current/bin",
        "/usr/bin",
        "/bin",
    ] {
        let extra = PathBuf::from(extra);
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(unix)]
fn make_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = fs::metadata(p)?.permissions();
    perm.set_mode(0o755);
    fs::set_permissions(p, perm)?;
    Ok(())
}
#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

fn pct(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("{}%", done * 100 / t),
        _ => format!("{} MB", done / 1_048_576),
    }
}

// ───────────────────────────── yt-dlp ─────────────────────────────

/// Versione di yt-dlp che gira con Python (un unico file che comincia con `#!`):
/// serve sui Mac in cui `yt-dlp_macos` non parte (macOS precedenti a 10.15).
const YTDLP_PYTHON: &str = "yt-dlp";

/// Il yt-dlp installato è la versione per Python? (si riconosce da `#!` all'inizio)
fn installed_is_python_zipapp() -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    File::open(ytdlp_path())
        .and_then(|mut f| f.read_exact(&mut head))
        .map(|_| &head == b"#!")
        .unwrap_or(false)
}

fn ytdlp_asset() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        // una volta passati alla versione per Python, gli aggiornamenti restano su quella
        if installed_is_python_zipapp() {
            YTDLP_PYTHON
        } else {
            "yt-dlp_macos"
        }
    } else if cfg!(target_arch = "aarch64") {
        "yt-dlp_linux_aarch64"
    } else {
        "yt-dlp_linux"
    }
}

pub fn local_ytdlp_version() -> Option<String> {
    let p = ytdlp_path();
    if !p.exists() {
        return None;
    }
    let out = command(&p).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn latest_ytdlp_version() -> Result<String> {
    let v = net::get_json("https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest")?;
    v["tag_name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow!(tr!(
                "risposta GitHub senza tag_name",
                "GitHub reply without tag_name"
            ))
        })
}

fn install_ytdlp(status: &mut dyn FnMut(String)) -> Result<()> {
    install_ytdlp_asset(ytdlp_asset(), status)
}

fn install_ytdlp_asset(asset: &str, status: &mut dyn FnMut(String)) -> Result<()> {
    let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}");
    let dest = ytdlp_path();
    net::download(&url, &dest, &mut |d, t| {
        status(tr!(
            format!("Scarico yt-dlp… {}", pct(d, t)),
            format!("Downloading yt-dlp… {}", pct(d, t))
        ))
    })?;
    make_executable(&dest)?;
    Ok(())
}

/// Scarica yt-dlp se manca, lo aggiorna se esiste una release più recente.
/// Ritorna la versione installata.
pub fn ensure_ytdlp(status: &mut dyn FnMut(String)) -> Result<String> {
    match local_ytdlp_version() {
        None => {
            install_ytdlp(status).context(tr!("installazione di yt-dlp", "installing yt-dlp"))?;
        }
        Some(local) => {
            status(
                tr!(
                    "Controllo aggiornamenti di yt-dlp…",
                    "Checking for yt-dlp updates…"
                )
                .into(),
            );
            // se il controllo fallisce (offline, rate limit) si usa la versione locale
            if let Ok(latest) = latest_ytdlp_version() {
                if latest != local {
                    status(tr!(
                        format!("Aggiorno yt-dlp dalla {local} alla {latest}…"),
                        format!("Updating yt-dlp from {local} to {latest}…")
                    ));
                    install_ytdlp(status)
                        .context(tr!("aggiornamento di yt-dlp", "updating yt-dlp"))?;
                }
            }
        }
    }
    if let Some(v) = local_ytdlp_version() {
        return Ok(v);
    }
    // macOS vecchi (High Sierra, Mojave): yt-dlp_macos richiede macOS 10.15 e non parte.
    // Si passa alla versione per Python, che funziona se è installato Python 3.10 o più recente.
    if cfg!(target_os = "macos") && !installed_is_python_zipapp() {
        status(
            tr!(
                "Questa versione di macOS è troppo vecchia per yt-dlp: provo la versione per Python…",
                "This macOS version is too old for yt-dlp: trying the Python version…"
            )
            .into(),
        );
        install_ytdlp_asset(YTDLP_PYTHON, status)
            .context(tr!("installazione di yt-dlp", "installing yt-dlp"))?;
        if let Some(v) = local_ytdlp_version() {
            return Ok(v);
        }
        bail!(tr!(
            "su questa versione di macOS yt-dlp ha bisogno di Python 3.10 o più recente. Installalo da python.org (Python 3.13, «macOS installer», poi apri «Install Certificates.command» nella cartella Applicazioni › Python 3.13) e premi Riprova.",
            "on this macOS version yt-dlp needs Python 3.10 or newer. Install it from python.org (Python 3.13, “macOS installer”, then open “Install Certificates.command” in Applications › Python 3.13) and press Retry."
        ));
    }
    bail!(tr!(
        "yt-dlp non si avvia dopo l'installazione",
        "yt-dlp does not start after installation"
    ))
}

/// Moduli Python che la versione di yt-dlp per Python non contiene (la versione per Mac
/// sì): (nome del pacchetto, nome per `import`). mutagen serve a incorporare le copertine
/// in M4A, MP3 e FLAC (senza, yt-dlp ripiega su ffprobe, che non c'è, e la copertina
/// resta accanto al brano come file .jpg); requests e brotli rendono le richieste più
/// efficienti.
const PYTHON_MODULES: [(&str, &str); 3] = [
    ("mutagen", "mutagen"),
    ("requests", "requests"),
    ("brotli", "brotli"),
];

/// Se si usa la versione di yt-dlp per Python, installa una volta sola (con pip, nella
/// cartella dell'utente) i moduli che le mancano. Negli altri casi non fa nulla.
/// Restituisce un errore solo se manca mutagen, l'unico indispensabile.
pub fn ensure_python_modules(status: &mut dyn FnMut(String)) -> Result<()> {
    if !installed_is_python_zipapp() {
        return Ok(());
    }
    let has = |module: &str| {
        command("python3")
            .args(["-c", &format!("import {module}")])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    for (package, module) in PYTHON_MODULES {
        if has(module) {
            continue;
        }
        status(tr!(
            format!("Installo il componente Python «{package}» per yt-dlp…"),
            format!("Installing the Python component “{package}” for yt-dlp…")
        ));
        // un pacchetto alla volta: se uno facoltativo non si installa, gli altri sì;
        // --only-binary: mai compilare da sorgente (servirebbero gli strumenti di Xcode)
        let _ = command("python3")
            .args([
                "-m",
                "pip",
                "install",
                "--user",
                "--quiet",
                "--disable-pip-version-check",
                "--only-binary",
                ":all:",
                package,
            ])
            .output();
    }
    if has("mutagen") {
        Ok(())
    } else {
        bail!(tr!(
            "installazione di mutagen non riuscita; prova nel Terminale: python3 -m pip install --user mutagen",
            "mutagen could not be installed; try in Terminal: python3 -m pip install --user mutagen"
        ))
    }
}

// ───────────────────────────── Deno ─────────────────────────────

fn deno_target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    })
}

/// Deno è il runtime JavaScript che yt-dlp usa per risolvere le "sfide" di
/// YouTube. Se manca lo scarichiamo accanto a yt-dlp.
pub fn ensure_deno(status: &mut dyn FnMut(String)) -> Result<PathBuf> {
    let dest = deno_path();
    if dest.exists() {
        // già scaricato: va usato solo se parte davvero. Se non parte (es. macOS troppo
        // vecchio) non lo si riscarica a ogni avvio: si avvisa e basta.
        return check_deno(dest);
    }
    let target = deno_target().ok_or_else(|| {
        anyhow!(tr!(
            "piattaforma non supportata per Deno",
            "platform not supported for Deno"
        ))
    })?;
    let url =
        format!("https://github.com/denoland/deno/releases/latest/download/deno-{target}.zip");
    let tmp = data_dir().join("deno.zip");
    net::download(&url, &tmp, &mut |d, t| {
        status(tr!(
            format!("Scarico il runtime JavaScript (Deno)… {}", pct(d, t)),
            format!("Downloading the JavaScript runtime (Deno)… {}", pct(d, t))
        ))
    })?;
    extract_zip(&tmp, &[&format!("deno{EXE}")], &bin_dir())?;
    let _ = fs::remove_file(&tmp);
    if !dest.exists() {
        bail!(tr!(
            "Deno non trovato nell'archivio",
            "Deno not found in the archive"
        ));
    }
    check_deno(dest)
}

fn check_deno(dest: PathBuf) -> Result<PathBuf> {
    let runs = command(&dest)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if runs {
        Ok(dest)
    } else {
        bail!(tr!(
            "Deno non funziona su questa versione del sistema operativo",
            "Deno does not work on this operating system version"
        ))
    }
}

// ───────────────────────────── QuickJS ─────────────────────────────

/// Runtime JavaScript per yt-dlp: Deno; sui Mac dove Deno non parte (le sue versioni
/// richiedono un macOS recente) si ripiega su QuickJS, più lento ma leggero.
pub fn ensure_js_runtime(status: &mut dyn FnMut(String)) -> Result<JsRuntime> {
    match ensure_deno(status) {
        Ok(p) => Ok(JsRuntime::deno(p)),
        Err(e) if cfg!(target_os = "macos") => ensure_quickjs(status)
            .map(JsRuntime::quickjs)
            .map_err(|q| anyhow!("{e:#}; QuickJS: {q:#}")),
        Err(e) => Err(e),
    }
}

/// QuickJS-ng nella versione universale (Cosmopolitan, «APE»): gira anche su macOS
/// vecchi come High Sierra, dove la versione per Mac non parte. Un file APE però non
/// si avvia direttamente: va passato a `sh`. Per questo yt-dlp riceve un piccolo
/// script (`qjs`) che lo avvia nel modo giusto.
const QUICKJS_URL: &str =
    "https://github.com/quickjs-ng/quickjs/releases/latest/download/qjs-cosmo.com";

const QUICKJS_LAUNCHER: &str = r#"#!/bin/sh
# Avvia QuickJS (versione universale Cosmopolitan) per yt-dlp: creato da Grabbit.
q="$(dirname "$0")/qjs.com"
case "$(head -c 2 "$q")" in
  MZ) exec /bin/sh "$q" "$@" ;;
  *) exec "$q" "$@" ;;
esac
"#;

pub fn ensure_quickjs(status: &mut dyn FnMut(String)) -> Result<PathBuf> {
    let dir = bin_dir();
    let com = dir.join("qjs.com");
    let launcher = dir.join("qjs");
    if !com.exists() {
        net::download(QUICKJS_URL, &com, &mut |d, t| {
            status(tr!(
                format!("Scarico il runtime JavaScript (QuickJS)… {}", pct(d, t)),
                format!(
                    "Downloading the JavaScript runtime (QuickJS)… {}",
                    pct(d, t)
                )
            ))
        })?;
        make_executable(&com)?;
    }
    if fs::read_to_string(&launcher).ok().as_deref() != Some(QUICKJS_LAUNCHER) {
        fs::write(&launcher, QUICKJS_LAUNCHER)?;
    }
    make_executable(&launcher)?;
    let runs = command(&launcher)
        .args(["-e", "0"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if runs {
        Ok(launcher)
    } else {
        bail!(tr!("QuickJS non si avvia", "QuickJS does not start"))
    }
}

// ───────────────────────────── ffmpeg ─────────────────────────────

/// Cartella che contiene ffmpeg: prima la nostra, poi il PATH di sistema.
pub fn find_ffmpeg_dir() -> Option<PathBuf> {
    let ours = bin_dir();
    if ours.join(format!("ffmpeg{EXE}")).is_file() {
        return Some(ours);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // le app grafiche su macOS non ereditano il PATH della shell
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter()
        .find(|d| d.join(format!("ffmpeg{EXE}")).is_file())
}

pub fn ensure_ffmpeg(status: &mut dyn FnMut(String)) -> Result<PathBuf> {
    if let Some(d) = find_ffmpeg_dir() {
        return Ok(d);
    }
    let dest = bin_dir();
    fs::create_dir_all(&dest)?;
    let tmp = data_dir().join("ffmpeg-download");
    let mut progress = |d: u64, t: Option<u64>| {
        status(tr!(
            format!("Scarico ffmpeg… {}", pct(d, t)),
            format!("Downloading ffmpeg… {}", pct(d, t))
        ))
    };

    #[cfg(windows)]
    {
        let url = "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";
        net::download(url, &tmp, &mut progress)?;
        extract_zip(&tmp, &["ffmpeg.exe", "ffprobe.exe"], &dest)?;
    }
    #[cfg(target_os = "macos")]
    {
        // evermeet.cx, la fonte di prima, ora risponde «410 Gone». ffmpeg arriva dal
        // pacchetto imageio-ffmpeg su PyPI: un ffmpeg statico che usa solo librerie di
        // sistema, per Intel da macOS 10.13 (High Sierra) e per Apple Silicon.
        // Su Apple Silicon, se quello nativo non partisse, si prova quello Intel (Rosetta).
        let pypi = net::get_json(PYPI_IMAGEIO_FFMPEG)?;
        let archs: &[&str] = if cfg!(target_arch = "aarch64") {
            &["arm64", "x86_64"]
        } else {
            &["x86_64"]
        };
        let exe = dest.join("ffmpeg");
        let mut ok = false;
        for arch in archs {
            let Some(url) = macos_ffmpeg_wheel_url(&pypi, arch) else {
                continue;
            };
            net::download(&url, &tmp, &mut progress)?;
            extract_wheel_ffmpeg(&tmp, &exe)?;
            if command(&exe)
                .arg("-version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                ok = true;
                break;
            }
            let _ = fs::remove_file(&exe);
        }
        if !ok {
            let _ = fs::remove_file(&tmp);
            bail!(tr!(
                "nessuna versione di ffmpeg scaricata parte su questo Mac",
                "no downloaded version of ffmpeg starts on this Mac"
            ));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let arch = if cfg!(target_arch = "aarch64") {
            "linuxarm64"
        } else {
            "linux64"
        };
        let url = format!(
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-{arch}-gpl.tar.xz"
        );
        net::download(&url, &tmp, &mut progress)?;
        status(tr!("Estraggo ffmpeg…", "Extracting ffmpeg…").into());
        extract_tar_xz(&tmp, &dest)?;
    }
    let _ = fs::remove_file(&tmp);
    find_ffmpeg_dir().ok_or_else(|| {
        anyhow!(tr!(
            "ffmpeg non trovato dopo l'estrazione",
            "ffmpeg not found after extraction"
        ))
    })
}

// ─────────────────────────── estrazione archivi ───────────────────────────

fn extract_zip(zip_path: &Path, wanted: &[&str], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let entry_name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        let base = Path::new(&entry_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if wanted.contains(&base.as_str()) {
            let out_path = dest.join(&base);
            let mut out = File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            make_executable(&out_path)?;
        }
    }
    Ok(())
}

/// Dati del pacchetto imageio-ffmpeg su PyPI (elenco dei file dell'ultima versione).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const PYPI_IMAGEIO_FFMPEG: &str = "https://pypi.org/pypi/imageio-ffmpeg/json";

/// Indirizzo del pacchetto («wheel») di imageio-ffmpeg per macOS e l'architettura data
/// ("x86_64" o "arm64"), preso dal JSON di PyPI.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_ffmpeg_wheel_url(pypi: &serde_json::Value, arch: &str) -> Option<String> {
    pypi.get("urls")?.as_array()?.iter().find_map(|u| {
        let name = u.get("filename")?.as_str()?;
        let url = u.get("url")?.as_str()?;
        (name.ends_with(".whl")
            && name.contains("macosx")
            && name.contains(&format!("_{arch}."))
            && url.starts_with("https://"))
        .then(|| url.to_string())
    })
}

/// Estrae dal pacchetto di imageio-ffmpeg (uno zip) l'eseguibile
/// `imageio_ffmpeg/binaries/ffmpeg-…` e lo salva in `exe`, pronto da eseguire.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn extract_wheel_ffmpeg(wheel: &Path, exe: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(wheel)?)?;
    let name = archive
        .file_names()
        .filter_map(|n| n.ok())
        .find(|n| n.contains("/binaries/ffmpeg-") && !n.ends_with(".md"))
        .map(|n| n.into_owned())
        .ok_or_else(|| {
            anyhow!(tr!(
                "ffmpeg non trovato nel pacchetto",
                "ffmpeg not found in the package"
            ))
        })?;
    let mut entry = archive.by_name(&name)?;
    if let Some(dir) = exe.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = File::create(exe)?;
    std::io::copy(&mut entry, &mut out)?;
    drop(out);
    make_executable(exe)
}

#[cfg(target_os = "linux")]
fn extract_tar_xz(path: &Path, dest: &Path) -> Result<()> {
    let file = std::io::BufReader::new(File::open(path)?);
    let mut archive = tar::Archive::new(xz2::read::XzDecoder::new(file));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let (name, in_bin) = {
            let p = entry.path()?;
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let in_bin = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|n| n == "bin")
                .unwrap_or(false);
            (name, in_bin)
        };
        if in_bin && (name == "ffmpeg" || name == "ffprobe") {
            let out = dest.join(&name);
            entry.unpack(&out)?;
            make_executable(&out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grabbit-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn picks_the_macos_ffmpeg_wheel_for_each_arch() {
        let pypi: serde_json::Value = serde_json::from_str(
            r#"{"urls": [
              {"filename": "imageio_ffmpeg-0.8.1-py3-none-macosx_10_13_x86_64.whl", "url": "https://files.pythonhosted.org/x86.whl"},
              {"filename": "imageio_ffmpeg-0.8.1-py3-none-macosx_12_0_arm64.whl", "url": "https://files.pythonhosted.org/arm.whl"},
              {"filename": "imageio_ffmpeg-0.8.1-py3-none-manylinux_2_28_x86_64.whl", "url": "https://files.pythonhosted.org/linux.whl"},
              {"filename": "imageio_ffmpeg-0.8.1.tar.gz", "url": "https://files.pythonhosted.org/src.tar.gz"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            macos_ffmpeg_wheel_url(&pypi, "x86_64").as_deref(),
            Some("https://files.pythonhosted.org/x86.whl")
        );
        assert_eq!(
            macos_ffmpeg_wheel_url(&pypi, "arm64").as_deref(),
            Some("https://files.pythonhosted.org/arm.whl")
        );
        assert_eq!(
            macos_ffmpeg_wheel_url(&serde_json::json!({}), "x86_64"),
            None
        );
    }

    #[test]
    fn extracts_ffmpeg_from_the_wheel() {
        let dir = tmpdir("wheel");
        let whl = dir.join("ff.whl");
        {
            let mut w = zip::ZipWriter::new(File::create(&whl).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("imageio_ffmpeg/binaries/README.md", opts)
                .unwrap();
            w.write_all(b"leggimi").unwrap();
            w.start_file("imageio_ffmpeg/binaries/ffmpeg-macos-x86_64-v9.0.2", opts)
                .unwrap();
            w.write_all(b"ffmpeg finto").unwrap();
            w.finish().unwrap();
        }
        let exe = dir.join("bin").join("ffmpeg");
        extract_wheel_ffmpeg(&whl, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"ffmpeg finto");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Con il pacchetto vero scaricato da PyPI:
    /// GRABBIT_FFMPEG_WHEEL=/percorso/imageio_ffmpeg-…-macosx_10_13_x86_64.whl cargo test real_wheel -- --ignored
    #[test]
    #[ignore]
    fn real_wheel_contains_a_macho_ffmpeg() {
        let Ok(whl) = std::env::var("GRABBIT_FFMPEG_WHEEL") else {
            return;
        };
        let dir = tmpdir("realwheel");
        let exe = dir.join("ffmpeg");
        extract_wheel_ffmpeg(Path::new(&whl), &exe).unwrap();
        let head = fs::read(&exe).unwrap();
        assert!(head.len() > 10_000_000);
        assert_eq!(&head[..4], &[0xcf, 0xfa, 0xed, 0xfe]); // Mach-O 64 bit
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_wanted_files_from_zip_in_nested_dirs() {
        let dir = tmpdir("zip");
        let zip_path = dir.join("a.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("ffmpeg-master/bin/ffmpeg.exe", opts).unwrap();
            w.write_all(b"FF").unwrap();
            w.start_file("ffmpeg-master/bin/ffprobe.exe", opts).unwrap();
            w.write_all(b"FP").unwrap();
            w.start_file("ffmpeg-master/doc/readme.txt", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let out = dir.join("out");
        extract_zip(&zip_path, &["ffmpeg.exe", "ffprobe.exe"], &out).unwrap();
        assert_eq!(fs::read(out.join("ffmpeg.exe")).unwrap(), b"FF");
        assert_eq!(fs::read(out.join("ffprobe.exe")).unwrap(), b"FP");
        assert!(!out.join("readme.txt").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn extracts_ffmpeg_from_tar_xz() {
        let dir = tmpdir("tarxz");
        let archive = dir.join("a.tar.xz");
        {
            let enc = xz2::write::XzEncoder::new(File::create(&archive).unwrap(), 1);
            let mut tb = tar::Builder::new(enc);
            for (name, data) in [
                ("ff-master/bin/ffmpeg", &b"FF"[..]),
                ("ff-master/bin/ffprobe", b"FP"),
                ("ff-master/LICENSE", b"L"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                tb.append_data(&mut h, name, data).unwrap();
            }
            tb.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        extract_tar_xz(&archive, &out).unwrap();
        assert_eq!(fs::read(out.join("ffmpeg")).unwrap(), b"FF");
        assert!(out.join("ffprobe").exists());
        assert!(!out.join("LICENSE").exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(out.join("ffmpeg"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0o111
        );
    }
}
