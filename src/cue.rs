// SPDX-License-Identifier: GPL-3.0-or-later
//! Album «immagine + CUE»: un solo file audio con tutto il disco e un `.cue` con l'elenco delle
//! tracce. «Dividi in brani» lo trasforma in un file per brano, con i tag, senza perdere qualità
//! (le sorgenti senza perdita diventano FLAC, le altre si tagliano senza ricodificare).
use crate::tr;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq)]
pub struct Sheet {
    pub performer: Option<String>,
    pub title: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    /// il file audio a cui si riferisce il foglio (nome come scritto nel .cue)
    pub file: String,
    pub tracks: Vec<CueTrack>,
}

#[derive(Debug, Default, PartialEq)]
pub struct CueTrack {
    pub number: u32,
    pub title: String,
    pub performer: Option<String>,
    /// inizio in secondi (INDEX 01)
    pub start: f64,
}

/// Testo di un .cue: UTF-8 (con o senza BOM) oppure Windows-1252, frequente nei .cue fatti
/// su Windows con le lettere accentate («Verità»).
pub fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            _ => c as char,
        })
        .collect()
}

/// Il valore dopo il comando: tra virgolette se ci sono, altrimenti il resto della riga.
fn value(rest: &str) -> String {
    let r = rest.trim();
    match r.strip_prefix('"') {
        Some(inner) => inner.rsplit_once('"').map_or(inner, |(v, _)| v).to_string(),
        None => r.to_string(),
    }
}

/// «mm:ss:ff» (75 fotogrammi al secondo) in secondi.
fn time(s: &str) -> Option<f64> {
    let mut p = s.trim().split(':').map(|x| x.parse::<f64>().ok());
    let (m, sec, f) = (p.next()??, p.next()??, p.next()??);
    Some(m * 60.0 + sec + f / 75.0)
}

pub fn parse(text: &str) -> Result<Sheet> {
    let mut sheet = Sheet::default();
    let mut files = 0;
    for line in text.lines() {
        let line = line.trim();
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match cmd.to_ascii_uppercase().as_str() {
            "REM" => {
                let (key, v) = rest
                    .trim()
                    .split_once(char::is_whitespace)
                    .unwrap_or((rest, ""));
                match key.to_ascii_uppercase().as_str() {
                    "DATE" => sheet.date = Some(value(v)),
                    "GENRE" => sheet.genre = Some(value(v)),
                    _ => {}
                }
            }
            "FILE" => {
                files += 1;
                // FILE "nome" WAVE: il tipo in fondo non fa parte del nome
                let r = rest.trim();
                sheet.file = if r.starts_with('"') {
                    value(r)
                } else {
                    r.rsplit_once(char::is_whitespace)
                        .map_or(r, |(n, _)| n)
                        .to_string()
                };
            }
            "TRACK" => {
                let n = rest
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(sheet.tracks.len() as u32 + 1);
                sheet.tracks.push(CueTrack {
                    number: n,
                    start: f64::NAN,
                    ..CueTrack::default()
                });
            }
            "TITLE" | "PERFORMER" => {
                let v = value(rest);
                let is_title = cmd.eq_ignore_ascii_case("TITLE");
                match sheet.tracks.last_mut() {
                    Some(t) if is_title => t.title = v,
                    Some(t) => t.performer = Some(v),
                    None if is_title => sheet.title = Some(v),
                    None => sheet.performer = Some(v),
                }
            }
            "INDEX" => {
                let mut it = rest.split_whitespace();
                if it.next() == Some("01") {
                    if let (Some(t), Some(s)) = (sheet.tracks.last_mut(), it.next().and_then(time))
                    {
                        t.start = s;
                    }
                }
            }
            _ => {}
        }
    }
    if files != 1 {
        bail!(tr!(
            "il .cue non si riferisce a un solo file audio",
            "the .cue does not refer to a single audio file"
        ));
    }
    sheet.tracks.retain(|t| t.start.is_finite());
    if sheet.tracks.is_empty() {
        bail!(tr!("il .cue non ha tracce", "the .cue has no tracks"));
    }
    for (i, t) in sheet.tracks.iter_mut().enumerate() {
        if t.title.trim().is_empty() {
            t.title = tr!(format!("Traccia {}", i + 1), format!("Track {}", i + 1));
        }
    }
    Ok(sheet)
}

/// Estensioni dei file audio che un .cue può indicare.
const AUDIO: [&str; 10] = [
    "flac", "wav", "ape", "wv", "aiff", "aif", "tta", "mp3", "m4a", "ogg",
];

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Il .cue di un album «immagine + CUE» nella cartella, con il suo file audio (anche se nel
/// .cue c'è scritto «.wav» e il file vero è «.flac», come capita spesso).
pub fn find_in(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let rd = std::fs::read_dir(dir).ok()?;
    let cues: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| ext_of(p) == "cue" && p.is_file())
        .collect();
    for cue in cues {
        let Ok(sheet) = std::fs::read(&cue).map(|b| decode(&b)).map(|t| parse(&t)) else {
            continue;
        };
        let Ok(sheet) = sheet else { continue };
        if sheet.tracks.len() < 2 {
            continue;
        }
        let named = dir.join(&sheet.file);
        if named.is_file() {
            return Some((cue, named));
        }
        let stem = Path::new(&sheet.file).file_stem()?.to_os_string();
        for stem in [stem, cue.file_stem()?.to_os_string()] {
            for ext in AUDIO {
                let mut name = stem.clone();
                name.push(format!(".{ext}"));
                let p = dir.join(&name);
                if p.is_file() {
                    return Some((cue.clone(), p));
                }
            }
        }
    }
    None
}

/// Senza perdita → FLAC (ricodificato, identico); con perdita → stesso formato, senza ricodificare.
fn output_format(audio: &Path) -> (&'static str, &'static str) {
    match ext_of(audio).as_str() {
        "mp3" => ("mp3", "copy"),
        "m4a" => ("m4a", "copy"),
        "ogg" => ("ogg", "copy"),
        _ => ("flac", "flac"),
    }
}

/// Nome del file di un brano: «01 - Le Tre Verità.flac».
pub fn out_name(n: u32, title: &str, ext: &str) -> String {
    format!("{n:02} - {}.{ext}", crate::downloader::sanitize(title))
}

/// Divide `audio` nei brani descritti da `cue`, nella stessa cartella. Torna i file creati;
/// se qualcosa va storto li toglie e non tocca gli originali.
pub fn split(
    ffmpeg: &Path,
    cue: &Path,
    audio: &Path,
    cover: Option<&Path>,
    progress: &dyn Fn(usize, usize),
) -> Result<Vec<PathBuf>> {
    let sheet = parse(&decode(&std::fs::read(cue)?))?;
    let dir = audio.parent().ok_or_else(|| anyhow!("cartella del file"))?;
    let (ext, codec) = output_format(audio);
    let total = sheet.tracks.len();
    let mut made: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        for (i, t) in sheet.tracks.iter().enumerate() {
            progress(i, total);
            let out = dir.join(out_name(t.number, &t.title, ext));
            if out == audio {
                bail!("il brano avrebbe lo stesso nome del file intero");
            }
            let end = sheet.tracks.get(i + 1).map(|n| n.start);
            let performer = t
                .performer
                .clone()
                .or_else(|| sheet.performer.clone())
                .unwrap_or_default();
            let run = |with_cover: bool| -> Result<()> {
                let mut c = crate::tools::command(ffmpeg);
                c.args(["-nostdin", "-y", "-loglevel", "error"]);
                c.args(["-ss", &format!("{:.3}", t.start)])
                    .arg("-i")
                    .arg(audio);
                if let (true, Some(img)) = (with_cover, cover) {
                    c.arg("-i").arg(img);
                }
                if let Some(e) = end {
                    c.args(["-t", &format!("{:.3}", e - t.start)]);
                }
                c.args(["-map", "0:a", "-c:a", codec, "-map_metadata", "-1"]);
                if with_cover && cover.is_some() {
                    c.args([
                        "-map",
                        "1:v",
                        "-c:v",
                        "copy",
                        "-disposition:v",
                        "attached_pic",
                    ]);
                }
                let mut meta = vec![
                    ("title", t.title.clone()),
                    ("artist", performer.clone()),
                    ("track", format!("{}/{}", t.number, total)),
                ];
                if let Some(a) = &sheet.performer {
                    meta.push(("album_artist", a.clone()));
                }
                if let Some(a) = &sheet.title {
                    meta.push(("album", a.clone()));
                }
                if let Some(d) = &sheet.date {
                    meta.push(("date", d.clone()));
                }
                if let Some(g) = &sheet.genre {
                    meta.push(("genre", g.clone()));
                }
                for (k, v) in meta.iter().filter(|(_, v)| !v.is_empty()) {
                    c.arg("-metadata").arg(format!("{k}={v}"));
                }
                c.arg(&out);
                let o = c.output().context("ffmpeg non si avvia")?;
                if !o.status.success() {
                    bail!("{}", String::from_utf8_lossy(&o.stderr).trim());
                }
                Ok(())
            };
            // la copertina dentro al file; se questa versione di ffmpeg non ci riesce, senza
            if run(cover.is_some()).is_err() {
                let _ = std::fs::remove_file(&out);
                run(false)?;
            }
            // prima di toccare gli originali: il brano deve esserci davvero e non essere vuoto
            if std::fs::metadata(&out).map_or(true, |m| m.len() == 0) {
                bail!(tr!(
                    format!("ffmpeg non ha creato «{}»", out.display()),
                    format!("ffmpeg did not create “{}”", out.display())
                ));
            }
            made.push(out);
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            progress(total, total);
            Ok(made)
        }
        Err(e) => {
            for f in &made {
                let _ = std::fs::remove_file(f);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BATTISTI: &str = r#"REM GENRE Pop
REM DATE 1974
PERFORMER "Lucio Battisti"
TITLE "Pensieri e Parole"
FILE "Lucio Battisti - Pensieri e Parole.wav" WAVE
  TRACK 01 AUDIO
    TITLE "Le Tre Verità"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Dio Mio No"
    PERFORMER "Lucio Battisti"
    INDEX 00 04:48:50
    INDEX 01 04:50:37
  TRACK 03 AUDIO
    INDEX 01 12:15:00
"#;

    #[test]
    fn cue_sheet_is_parsed() {
        let s = parse(BATTISTI).unwrap();
        assert_eq!(s.performer.as_deref(), Some("Lucio Battisti"));
        assert_eq!(s.title.as_deref(), Some("Pensieri e Parole"));
        assert_eq!(s.date.as_deref(), Some("1974"));
        assert_eq!(s.file, "Lucio Battisti - Pensieri e Parole.wav");
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.tracks[0].title, "Le Tre Verità");
        // INDEX 01, non il pregap INDEX 00; 37 fotogrammi = 37/75 s
        assert!((s.tracks[1].start - (4.0 * 60.0 + 50.0 + 37.0 / 75.0)).abs() < 1e-9);
        assert!(
            s.tracks[2].title.ends_with(" 3"),
            "titolo di riserva: {}",
            s.tracks[2].title
        );
        assert_eq!(
            out_name(1, "Le Tre Verità", "flac"),
            "01 - Le Tre Verità.flac"
        );
    }

    #[test]
    fn windows_1252_and_bom_are_decoded() {
        assert_eq!(decode(b"\xEF\xBB\xBFVerit\xC3\xA0"), "Verità");
        assert_eq!(decode(b"Verit\xE0 \x93ok\x94"), "Verità “ok”");
    }

    #[test]
    fn cue_with_many_files_is_refused() {
        let two = "FILE \"a.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"b.flac\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n";
        assert!(parse(two).is_err());
    }

    /// Divisione vera con ffmpeg (se c'è): 3 s di FLAC in 3 brani da 1 s, con titoli e numeri;
    /// il .cue dice «.wav» ma il file è «.flac».
    #[test]
    fn image_is_split_into_tracks() {
        let Some(dir) = crate::tools::find_ffmpeg_dir() else {
            eprintln!("ffmpeg non c'è: prova saltata");
            return;
        };
        let ffmpeg = dir.join(format!("ffmpeg{}", crate::tools::EXE));
        let root = std::env::temp_dir().join(format!("grabbit-cue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let album = root.join("Battisti");
        std::fs::create_dir_all(&album).unwrap();
        let image = album.join("Pensieri e Parole.flac");
        let ok = std::process::Command::new(&ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
            ])
            .arg(&image)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        std::fs::write(
            album.join("Pensieri e Parole.cue"),
            "PERFORMER \"Lucio Battisti\"\nTITLE \"Pensieri e Parole\"\nREM DATE 1974\n\
             FILE \"Pensieri e Parole.wav\" WAVE\n\
             TRACK 01 AUDIO\nTITLE \"Le Tre Verità\"\nINDEX 01 00:00:00\n\
             TRACK 02 AUDIO\nTITLE \"Dio Mio No\"\nINDEX 01 00:01:00\n\
             TRACK 03 AUDIO\nTITLE \"Adesso Sì\"\nINDEX 01 00:02:00\n",
        )
        .unwrap();
        let (cue, audio) = find_in(&album).expect("trovato il .cue e il suo file");
        assert_eq!(audio, image);
        // un ffmpeg che dice «fatto» senza creare i file: errore, originali intatti
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // stesso lucchetto degli altri programmi finti: niente «Text file busy»
            let _lock = crate::downloader::fake_ytdlp_lock();
            let fake = root.join("ffmpeg-finto");
            std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(split(&fake, &cue, &audio, None, &|_, _| {}).is_err());
            assert!(image.is_file() && cue.is_file());
        }
        let seen = std::cell::RefCell::new(Vec::new());
        let made = split(&ffmpeg, &cue, &audio, None, &|d, t| {
            seen.borrow_mut().push((d, t))
        })
        .unwrap();
        assert_eq!(made.len(), 3);
        assert_eq!(seen.borrow().last(), Some(&(3, 3)));
        // gli originali restano: li sposta nel Cestino chi chiama
        assert!(image.is_file());
        // via il file intero, la libreria vede i tre brani con titoli, numeri e durate
        std::fs::remove_file(&image).unwrap();
        let lib = crate::player::scan(&root, 16, &std::sync::atomic::AtomicBool::new(false));
        let a = &lib.albums[0];
        assert_eq!(a.name, "Pensieri e Parole");
        let titles: Vec<&str> = a.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Le Tre Verità", "Dio Mio No", "Adesso Sì"]);
        assert_eq!(a.tracks[1].number, Some(2));
        let d = a.tracks[0].duration.unwrap().as_secs_f32();
        assert!((d - 1.0).abs() < 0.1, "{d}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
