//! Updating Portable App Manager itself from a GitHub release.
//!
//! - The repo is public: no token is needed or sent.
//! - Only `https://github.com/...` download links are accepted.
//! - If the release publishes a SHA-256 (GitHub's asset `digest`, or a `<file>.sha256` asset) the download is verified.
//! - A running exe cannot be overwritten on Windows, but it CAN be renamed: the old exe becomes `<exe>.old`,
//!   the new one takes its place, and the app restarts. The `.old` file is removed on the next start.

use crate::util;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

/// Put your own `owner/repo` here to ship it as the default update source (empty = the user sets it in Settings).
pub const DEFAULT_REPO: &str = "";

/// Refuse absurdly large downloads.
const MAX_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    /// tag without the leading "v"
    pub version: String,
    /// release title
    pub name: String,
    /// release description (markdown)
    pub notes: String,
    /// the release page on github.com
    pub page: String,
    /// file to install; empty when the release has no file for this system
    pub asset: String,
    pub url: String,
    /// expected SHA-256 (lowercase hex) when GitHub / the release provides one
    pub sha256: Option<String>,
    /// download link of a `<file>.sha256` asset
    pub sha_url: Option<String>,
}

impl Release {
    pub fn can_install(&self) -> bool {
        !self.url.is_empty()
    }
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent("portable-app-manager-selfupdate")
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// "owner/repo", "github.com/owner/repo", "https://github.com/owner/repo/releases" -> "owner/repo"
pub fn normalize_repo(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    let parts: Vec<&str> = s.trim_end_matches('/').split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    let valid = |p: &str| !p.is_empty() && p.len() <= 100 && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let repo = parts[1].trim_end_matches(".git");
    (valid(parts[0]) && valid(repo)).then(|| format!("{}/{}", parts[0], repo))
}

/// "v1.2.3-rc1" -> [1, 2, 3]  (a pre-release suffix is ignored for the comparison)
fn key(v: &str) -> Vec<u64> {
    util::version_key(v.trim_start_matches(|c| c == 'v' || c == 'V').split('-').next().unwrap_or(""))
}

/// Look for a newer release. `Ok(None)` = you are up to date.
pub fn check(repo: &str, pattern: &str, prerelease: bool) -> Result<Option<Release>, String> {
    let repo = normalize_repo(repo).ok_or("The update source must look like owner/repo")?;
    let url = if prerelease {
        format!("https://api.github.com/repos/{repo}/releases?per_page=1")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    };
    let v: serde_json::Value = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "Update source not found (is the repo public, and does it have a release?)".to_string(),
            ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => "GitHub rate limit reached, try again later".to_string(),
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            other => other.to_string(),
        })?
        .into_json()
        .map_err(|e| e.to_string())?;

    let rel = (if prerelease { v.get(0) } else { Some(&v) }).ok_or("No releases found")?;
    let tag = rel["tag_name"].as_str().ok_or("The release has no tag")?.to_string();
    let version = tag.trim_start_matches(|c| c == 'v' || c == 'V').to_string();
    if key(&version) <= key(current_version()) {
        return Ok(None);
    }

    // (name, download url, digest)
    let assets: Vec<(String, String, Option<String>)> = rel["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((
                        x["name"].as_str()?.to_string(),
                        x["browser_download_url"].as_str()?.to_string(),
                        x["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_ascii_lowercase()),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let best = assets
        .iter()
        .filter(|(n, _, _)| util::name_matches(pattern, n))
        .filter(|(n, _, _)| util::platform_score(n) > -25)
        .max_by_key(|(n, _, _)| (util::platform_score(n), -(n.len() as i32)));

    let (asset, url, sha256, sha_url) = match best {
        Some((n, u, d)) => {
            let want = format!("{n}.sha256").to_lowercase();
            let sha_url = assets.iter().find(|(x, _, _)| x.to_lowercase() == want).map(|(_, u, _)| u.clone());
            (n.clone(), u.clone(), d.clone(), sha_url)
        }
        None => (String::new(), String::new(), None, None),
    };

    Ok(Some(Release {
        tag,
        version,
        name: rel["name"].as_str().unwrap_or("").to_string(),
        notes: rel["body"].as_str().unwrap_or("").chars().take(20_000).collect(),
        page: rel["html_url"].as_str().unwrap_or("").to_string(),
        asset,
        url,
        sha256,
        sha_url,
    }))
}

fn github_only(url: &str) -> Result<(), String> {
    if url.starts_with("https://github.com/") {
        Ok(())
    } else {
        Err("Refusing to download from a non-GitHub address".into())
    }
}

/// First 64-hex-digit word of a `.sha256` file.
fn parse_sha(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_start_matches('*').to_ascii_lowercase())
        .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Path the current exe is renamed to while updating.
fn old_path(cur: &Path) -> PathBuf {
    let mut s = cur.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Remove the leftover `<exe>.old` of a previous update (call on start).
pub fn cleanup() {
    if let Ok(cur) = std::env::current_exe() {
        let _ = fs::remove_file(old_path(&cur));
    }
}

/// Download the release file, verify it, and put it in place of the running exe.
/// Returns the path of the (new) exe to restart. `progress(done, total)` is throttled.
pub fn download_and_install(rel: &Release, tmp: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf, String> {
    github_only(&rel.url)?;
    let lower = rel.asset.to_lowercase();
    if [".zip", ".7z", ".tar.gz", ".tgz"].iter().any(|e| lower.ends_with(e)) {
        return Err("The release file is an archive; the update file must be the program itself (see \"Release file\" in Settings)".into());
    }

    // the checksum comes from GitHub's own digest, else from a <file>.sha256 asset
    let expected = match (&rel.sha256, &rel.sha_url) {
        (Some(h), _) => Some(h.clone()),
        (None, Some(u)) => {
            github_only(u)?;
            let text = agent().get(u).call().map_err(|e| format!("Could not read the checksum file: {e}"))?.into_string().map_err(|e| e.to_string())?;
            Some(parse_sha(&text).ok_or("The checksum file is not valid")?)
        }
        (None, None) => None,
    };

    let resp = agent().get(&rel.url).call().map_err(|e| format!("Download failed: {e}"))?;
    let total = resp.header("content-length").and_then(|s| s.parse::<u64>().ok());
    if total.map(|t| t > MAX_BYTES).unwrap_or(false) {
        return Err("The download is unexpectedly large".into());
    }
    fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let file = tmp.join("pam-selfupdate.bin");
    let result = (|| -> Result<(), String> {
        let mut reader = resp.into_reader();
        let mut out = File::create(&file).map_err(|e| format!("Cannot create temp file: {e}"))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let (mut done, mut last) = (0u64, Instant::now());
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > MAX_BYTES {
                return Err("The download is unexpectedly large".into());
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if last.elapsed() > Duration::from_millis(100) {
                progress(done, total);
                last = Instant::now();
            }
        }
        progress(done, total);
        if let Some(want) = expected {
            let got = format!("{:x}", hasher.finalize());
            if got != want {
                return Err("The downloaded file does not match its SHA-256 checksum; nothing was changed".into());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&file);
        return Err(e);
    }

    let installed = replace_current_exe(&file);
    let _ = fs::remove_file(&file);
    installed
}

fn replace_current_exe(new_file: &Path) -> Result<PathBuf, String> {
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = old_path(&cur);
    let _ = fs::remove_file(&old);
    fs::rename(&cur, &old).map_err(|e| format!("Could not replace the program (is its folder writable?): {e}"))?;
    if let Err(e) = fs::copy(new_file, &cur) {
        let _ = fs::remove_file(&cur);
        let _ = fs::rename(&old, &cur); // put the working version back
        return Err(format!("Could not write the new version: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cur, fs::Permissions::from_mode(0o755));
    }
    Ok(cur)
}

/// Start the (new) exe. Called when the old process is exiting.
pub fn restart(exe: &Path) {
    let mut cmd = Command::new(exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    let _ = cmd.spawn();
}
