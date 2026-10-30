//! Resolving "what is the latest file?" for every source type, and downloading it.

use crate::model::Source;
use crate::util;
use percent_encoding::percent_decode_str;
use regex::Regex;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use url::Url;

#[derive(Clone, Debug)]
pub struct Remote {
    /// Version label used for "is there an update?" comparison.
    pub version: String,
    pub url: String,
    pub filename: String,
}

const DEFAULT_LINK_RE: &str =
    r#"(?i)href\s*=\s*["']([^"']+?\.(?:zip|7z|exe|msi|tar\.gz|tgz|appimage)(?:\?[^"']*)?)["']"#;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent("portable-app-manager/0.1")
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .build()
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        other => other.to_string(),
    }
}

pub fn filename_from_url(u: &Url) -> String {
    u.path_segments()
        .and_then(|s| s.last())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
        .unwrap_or_default()
}

pub fn resolve(source: &Source, token: &str) -> Result<Remote, String> {
    match source {
        Source::GitHub { repo, pattern, prerelease } => github(repo, pattern, *prerelease, token),
        Source::Direct { url } => direct(url),
        Source::Site { page_url, link_regex, pattern, version_regex } => {
            site(page_url, link_regex, pattern, version_regex)
        }
        Source::Manual => Err("Manual apps have no update source".into()),
    }
}

fn github(repo: &str, pattern: &str, pre: bool, token: &str) -> Result<Remote, String> {
    let url = if pre {
        format!("https://api.github.com/repos/{repo}/releases?per_page=1")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    };
    let mut req = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28");
    if !token.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", token.trim()));
    }
    let v: serde_json::Value = req
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => {
                "GitHub rate limit hit - add a token in Settings".to_string()
            }
            ureq::Error::Status(404, _) => "Repo or release not found".to_string(),
            o => http_err(o),
        })?
        .into_json()
        .map_err(|e| e.to_string())?;

    let rel = (if pre { v.get(0) } else { Some(&v) }).ok_or("No releases found")?;
    let tag = rel["tag_name"].as_str().unwrap_or("?").to_string();
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();

    let all: Vec<(String, String)> = assets
        .iter()
        .filter_map(|a| {
            Some((
                a["name"].as_str()?.to_string(),
                a["browser_download_url"].as_str()?.to_string(),
            ))
        })
        .collect();

    let best = all
        .iter()
        .filter(|(n, _)| util::name_matches(pattern, n))
        .filter(|(n, _)| util::platform_score(n) > -25)
        .max_by_key(|(n, _)| (util::platform_score(n), -(n.len() as i32)));

    match best {
        Some((name, url)) => Ok(Remote { version: tag, url: url.clone(), filename: name.clone() }),
        None => {
            let names: Vec<_> = all.iter().take(8).map(|(n, _)| n.as_str()).collect();
            Err(format!(
                "No release file matches '{}' in {}. Available: {}",
                pattern,
                tag,
                names.join(", ")
            ))
        }
    }
}

fn direct(url: &str) -> Result<Remote, String> {
    let a = agent();
    let resp = a
        .head(url)
        .call()
        .or_else(|_| a.get(url).call())
        .map_err(http_err)?;

    let h = |n: &str| resp.header(n).map(|s| s.trim().trim_matches('"').to_string());
    let version = if let Some(e) = h("etag") {
        format!("etag:{e}")
    } else if let Some(m) = h("last-modified") {
        m
    } else if let Some(l) = h("content-length") {
        format!("size:{l}")
    } else {
        "?".into()
    };

    let filename = h("content-disposition")
        .and_then(|cd| {
            Regex::new(r#"filename\*?=(?:UTF-8'')?"?([^";]+)"?"#)
                .ok()?
                .captures(&cd)
                .map(|c| percent_decode_str(&c[1]).decode_utf8_lossy().to_string())
        })
        .or_else(|| Url::parse(resp.get_url()).ok().map(|u| filename_from_url(&u)))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "download.bin".into());

    Ok(Remote { version, url: resp.get_url().to_string(), filename })
}

fn site(page: &str, link_re: &str, pattern: &str, version_re: &str) -> Result<Remote, String> {
    let base = Url::parse(page).map_err(|e| format!("Bad page URL: {e}"))?;
    let html = agent()
        .get(page)
        .call()
        .map_err(http_err)?
        .into_string()
        .map_err(|e| e.to_string())?;

    let re = Regex::new(if link_re.trim().is_empty() { DEFAULT_LINK_RE } else { link_re })
        .map_err(|e| format!("Bad link regex: {e}"))?;
    let ver_re = if version_re.trim().is_empty() {
        None
    } else {
        Some(Regex::new(version_re).map_err(|e| format!("Bad version regex: {e}"))?)
    };

    let mut best: Option<(i32, Vec<u64>, Remote)> = None;
    for caps in re.captures_iter(&html) {
        let raw = caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str()).unwrap_or("");
        let href = raw.replace("&amp;", "&");
        let Ok(full) = base.join(&href) else { continue };
        let filename = filename_from_url(&full);
        if filename.is_empty() || !util::name_matches(pattern, &filename) {
            continue;
        }
        let version = ver_re
            .as_ref()
            .and_then(|r| r.captures(&href).or_else(|| r.captures(&filename)))
            .and_then(|c| c.get(1).or_else(|| c.get(0)).map(|m| m.as_str().to_string()))
            .or_else(|| util::find_version(&filename))
            .unwrap_or_else(|| filename.clone());
        let score = util::platform_score(&filename);
        let key = util::version_key(&version);
        let cand = Remote { version, url: full.to_string(), filename };
        let better = match &best {
            None => true,
            Some((s, k, _)) => (score, &key) > (*s, k),
        };
        if better {
            best = Some((score, key, cand));
        }
    }
    best.map(|(_, _, r)| r)
        .ok_or_else(|| "No matching download link found on that page (adjust the link regex / filter)".into())
}

/// Download `url` to `dest`. `progress(done, total)` is throttled.
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    let resp = agent().get(url).call().map_err(http_err)?;
    let total = resp.header("content-length").and_then(|s| s.parse::<u64>().ok());
    let mut reader = resp.into_reader();
    let mut file = File::create(dest).map_err(|e| format!("Cannot create temp file: {e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let (mut done, mut last) = (0u64, Instant::now());
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if last.elapsed() > Duration::from_millis(100) {
            progress(done, total);
            last = Instant::now();
        }
    }
    progress(done, total);
    Ok(())
}
