// SPDX-License-Identifier: GPL-3.0-or-later
//! Piccoli helper HTTP (ureq 3): JSON, testo e download con avanzamento.
use crate::tr;
use anyhow::{Context, Result};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("Grabbit/", env!("CARGO_PKG_VERSION"));

fn agent() -> ureq::Agent {
    // ureq 3 legge da solo le variabili d'ambiente del proxy (HTTPS_PROXY...).
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(15)))
        .build()
        .into()
}

pub fn get_json(url: &str) -> Result<serde_json::Value> {
    let mut resp = agent()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| {
            tr!(
                format!("richiesta a {url} fallita"),
                format!("request to {url} failed")
            )
        })?;
    Ok(resp.body_mut().read_json()?)
}

pub fn get_text(url: &str) -> Result<String> {
    let mut resp = agent()
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.8")
        .call()
        .with_context(|| {
            tr!(
                format!("richiesta a {url} fallita"),
                format!("request to {url} failed")
            )
        })?;
    Ok(resp.body_mut().read_to_string()?)
}

/// POST con corpo JSON e risposta JSON (con intestazioni aggiuntive).
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req.send_json(body).with_context(|| {
        tr!(
            format!("richiesta a {url} fallita"),
            format!("request to {url} failed")
        )
    })?;
    Ok(resp.body_mut().read_json()?)
}

/// Scarica in memoria un file piccolo (al massimo `limit` byte), ad esempio un'immagine.
pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut resp = agent().get(url).call().with_context(|| {
        tr!(
            format!("richiesta a {url} fallita"),
            format!("request to {url} failed")
        )
    })?;
    Ok(resp.body_mut().with_config().limit(limit).read_to_vec()?)
}

/// Scarica `url` in `dest` (passando da un file `.part`), chiamando
/// `progress(scaricati, totali)` mentre procede.
pub fn download(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let resp = agent().get(url).call().with_context(|| {
        tr!(
            format!("download di {url} fallito"),
            format!("download of {url} failed")
        )
    })?;
    let total = resp.body().content_length();
    let mut reader = resp.into_body().into_reader();

    let part = dest.with_extension("part");
    let mut file = File::create(&part)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if done - last_report >= 256 * 1024 {
            last_report = done;
            progress(done, total);
        }
    }
    file.flush()?;
    drop(file);
    progress(done, total);
    fs::rename(&part, dest)?;
    Ok(())
}
