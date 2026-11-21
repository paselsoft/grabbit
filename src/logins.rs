// SPDX-License-Identifier: GPL-3.0-or-later
//! «Accesso ai siti»: i cookie di un solo sito (Facebook, Instagram, RaiPlay), copiati una
//! volta dal browser in un file della cartella dei dati, leggibile solo dall'utente.
//!
//! Così i video che richiedono l'accesso si scaricano senza dare a ogni download tutti i
//! cookie del browser (e senza che il Mac chieda ogni volta l'accesso al Portachiavi), e gli
//! altri siti non li ricevono. Idea presa da ReelDownloader, ma senza mai scrivere i cookie
//! di tutti i siti in una cartella condivisa come /tmp.
use crate::downloader::Toolset;
use crate::{tools, tr};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Un sito con l'accesso: chiave (nome del file), nome, pagina da aprire per esportare i
/// cookie, domini dei suoi cookie.
pub struct Site {
    pub key: &'static str,
    pub name: &'static str,
    pub home: &'static str,
    pub domains: &'static [&'static str],
}

pub const SITES: [Site; 3] = [
    Site {
        key: "facebook",
        name: "Facebook",
        home: "https://www.facebook.com/",
        domains: &["facebook.com", "fb.watch"],
    },
    Site {
        key: "instagram",
        name: "Instagram",
        home: "https://www.instagram.com/",
        domains: &["instagram.com"],
    },
    Site {
        key: "raiplay",
        name: "RaiPlay",
        home: "https://www.raiplay.it/",
        domains: &["raiplay.it", "rai.it", "raiplaysound.it"],
    },
];

/// Cartella dei file dei cookie (dentro la cartella dei dati di Grabbit).
pub fn cookie_dir() -> PathBuf {
    tools::data_dir().join("cookies")
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split("://").nth(1)?;
    let host = rest.split(['/', '?', '#']).next()?.split(':').next()?;
    Some(host.to_lowercase())
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// Il sito con l'accesso a cui appartiene un link.
pub fn site_for(url: &str) -> Option<&'static Site> {
    let host = host_of(url)?;
    SITES
        .iter()
        .find(|s| s.domains.iter().any(|d| domain_matches(&host, d)))
}

/// File dei cookie di un sito in `dir` (esista o no).
pub fn cookie_file(dir: &Path, site: &Site) -> PathBuf {
    dir.join(format!("{}.txt", site.key))
}

/// Il file dei cookie da usare per un link, se l'accesso a quel sito è stato aggiornato.
pub fn cookie_file_for(dir: &Path, url: &str) -> Option<PathBuf> {
    let f = cookie_file(dir, site_for(url)?);
    f.is_file().then_some(f)
}

/// Argomenti di yt-dlp per i cookie di un link: il file del suo sito se c'è, altrimenti i
/// cookie del browser scelto in «Usa i cookie del browser» (se scelto).
pub fn cookie_args(dir: &Path, url: &str, browser: &str) -> Vec<String> {
    match cookie_file_for(dir, url) {
        Some(f) => vec!["--cookies".into(), f.to_string_lossy().into_owned()],
        None if !browser.is_empty() => vec!["--cookies-from-browser".into(), browser.into()],
        None => Vec::new(),
    }
}

/// Da un file di cookie in formato Netscape tiene l'intestazione e i soli cookie dei domini
/// del sito. Le righe «#HttpOnly_.facebook.com…» sono cookie, non commenti.
pub fn filter_cookies(text: &str, domains: &[&str]) -> (String, usize) {
    let mut out = String::new();
    let mut kept = 0;
    for line in text.lines() {
        let cookie = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        let is_comment = line.starts_with('#') && !line.starts_with("#HttpOnly_");
        if line.trim().is_empty() || is_comment {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let domain = cookie
            .split('\t')
            .next()
            .unwrap_or("")
            .trim_start_matches('.')
            .to_lowercase();
        if domains.iter().any(|d| domain_matches(&domain, d)) {
            out.push_str(line);
            out.push('\n');
            kept += 1;
        }
    }
    (out, kept)
}

/// Intestazione di un file di cookie che yt-dlp accetta come valido (anche senza cookie).
const NETSCAPE_HEADER: &str = "# Netscape HTTP Cookie File\n";

/// Scrive un file leggibile solo dall'utente (sui sistemi Unix, permessi 600).
fn write_private(path: &Path, text: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(path);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(text.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// «Aggiorna l'accesso»: yt-dlp legge i cookie dal browser e li salva in un file
/// temporaneo nella cartella dei cookie; si tengono solo quelli del sito e il temporaneo
/// si cancella. Ritorna quanti cookie del sito sono stati salvati.
pub fn refresh(site: &Site, browser: &str, tools: &Toolset, dir: &Path) -> Result<usize> {
    if browser.is_empty() {
        bail!(tr!("scegli prima un browser", "choose a browser first"));
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = dir.join(format!(".{}.tutti.tmp", site.key));
    let _ = std::fs::remove_file(&tmp);
    // il file temporaneo nasce privato e già valido: yt-dlp prima lo legge (un file vuoto
    // lo rifiuta con «does not look like a Netscape format cookies file»), poi lo riscrive
    // con i cookie del browser mantenendo i permessi
    write_private(&tmp, NETSCAPE_HEADER)?;
    let out = crate::tools::command(&tools.ytdlp)
        .args([
            "--cookies-from-browser",
            browser,
            "--cookies",
            &tmp.to_string_lossy(),
            "--skip-download",
            "--no-warnings",
            "--ignore-errors",
            "--",
            site.home,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .context("yt-dlp")?;
    let text = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    let (filtered, kept) = filter_cookies(&text, site.domains);
    if kept == 0 {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .unwrap_or("")
            .trim_start_matches("ERROR:")
            .trim()
            .to_string();
        if !last.is_empty() {
            // yt-dlp si è fermato prima di leggere i cookie: si mostra il suo errore
            bail!(tr!(
                format!("non riesco a leggere i cookie da {browser}: {last}"),
                format!("could not read the cookies from {browser}: {last}")
            ));
        }
        bail!(tr!(
            format!(
                "nessun cookie di {} in {browser}: entra nel sito con quel browser e riprova",
                site.name
            ),
            format!(
                "no {} cookies in {browser}: sign in to the site with that browser and try again",
                site.name
            )
        ));
    }
    write_private(&cookie_file(dir, site), &filtered)?;
    Ok(kept)
}

/// Quando è stato aggiornato l'accesso a un sito (data di modifica del file), se c'è.
pub fn updated(dir: &Path, site: &Site) -> Option<std::time::SystemTime> {
    std::fs::metadata(cookie_file(dir, site))
        .ok()?
        .modified()
        .ok()
}

/// «Togli»: cancella i cookie salvati di un sito.
pub fn forget(dir: &Path, site: &Site) {
    let _ = std::fs::remove_file(cookie_file(dir, site));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_go_to_their_site() {
        assert_eq!(
            site_for("https://www.facebook.com/reel/1").map(|s| s.key),
            Some("facebook")
        );
        assert_eq!(
            site_for("https://fb.watch/x").map(|s| s.key),
            Some("facebook")
        );
        assert_eq!(
            site_for("https://www.raiplay.it/video/x.html").map(|s| s.key),
            Some("raiplay")
        );
        assert_eq!(
            site_for("https://www.raiplaysound.it/audio/x").map(|s| s.key),
            Some("raiplay")
        );
        assert_eq!(
            site_for("https://www.youtube.com/watch?v=x").map(|s| s.key),
            None
        );
        // «notfacebook.com» non è Facebook
        assert_eq!(site_for("https://notfacebook.com/x").map(|s| s.key), None);
    }

    #[test]
    fn only_the_site_cookies_are_kept() {
        let jar = "# Netscape HTTP Cookie File\n\
.facebook.com\tTRUE\t/\tTRUE\t0\tc_user\t1\n\
#HttpOnly_.facebook.com\tTRUE\t/\tTRUE\t0\txs\t2\n\
.google.com\tTRUE\t/\tTRUE\t0\tSID\t3\n\
#HttpOnly_.bank.it\tTRUE\t/\tTRUE\t0\tsess\t4\n";
        let (out, kept) = filter_cookies(jar, &["facebook.com"]);
        assert_eq!(kept, 2);
        assert!(out.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(out.contains("c_user") && out.contains("xs"));
        assert!(!out.contains("SID") && !out.contains("bank.it"), "{out}");
    }

    /// «Aggiorna l'accesso» con un finto yt-dlp che salva i cookie di più siti: restano
    /// solo quelli di Facebook, in un file privato, e il temporaneo sparisce.
    #[cfg(unix)]
    #[test]
    fn refresh_keeps_only_the_site_cookies() {
        let _guard = crate::downloader::fake_ytdlp_lock();
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("grabbit-refresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-yt-dlp");
        // scrive nel file dopo «--cookies» come farebbe yt-dlp
        std::fs::write(
            &fake,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --cookies ]; then f=\"$2\"; fi; shift; done\n\
if [ -f \"$f\" ] && ! head -1 \"$f\" | grep -q 'Netscape HTTP Cookie File'; then echo \"ERROR: '$f' does not look like a Netscape format cookies file\" >&2; exit 1; fi\n\
printf '# Netscape HTTP Cookie File\\n.facebook.com\\tTRUE\\t/\\tTRUE\\t0\\tc_user\\t1\\n.google.com\\tTRUE\\t/\\tTRUE\\t0\\tSID\\t3\\n' > \"$f\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Toolset {
            ytdlp: fake,
            ffmpeg_dir: None,
            js: None,
        };
        let cookies = dir.join("cookies");
        let n = refresh(&SITES[0], "chrome", &tools, &cookies).unwrap();
        assert_eq!(n, 1);
        let saved = std::fs::read_to_string(cookie_file(&cookies, &SITES[0])).unwrap();
        assert!(
            saved.contains("c_user") && !saved.contains("SID"),
            "{saved}"
        );
        let mode = std::fs::metadata(cookie_file(&cookies, &SITES[0]))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let left: Vec<_> = std::fs::read_dir(&cookies)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        // senza browser scelto: errore chiaro
        assert!(refresh(&SITES[0], "", &tools, &cookies).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cookie_arguments_per_site() {
        let dir = std::env::temp_dir().join(format!("grabbit-login-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fb = "https://www.facebook.com/reel/1";
        let yt = "https://www.youtube.com/watch?v=x";
        // senza file: il browser scelto, o niente
        assert_eq!(
            cookie_args(&dir, fb, "chrome"),
            ["--cookies-from-browser", "chrome"]
        );
        assert!(cookie_args(&dir, fb, "").is_empty());
        // con il file di Facebook: solo i link di Facebook lo usano
        write_private(&cookie_file(&dir, &SITES[0]), "# x\n").unwrap();
        let a = cookie_args(&dir, fb, "chrome");
        assert_eq!(a[0], "--cookies");
        assert!(a[1].ends_with("facebook.txt"));
        assert_eq!(
            cookie_args(&dir, yt, "chrome"),
            ["--cookies-from-browser", "chrome"]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cookie_file(&dir, &SITES[0]))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(updated(&dir, &SITES[0]).is_some());
        forget(&dir, &SITES[0]);
        assert!(updated(&dir, &SITES[0]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
