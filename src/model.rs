//! Persistent data model + on-disk layout (everything lives next to the exe => portable).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum Source {
    /// GitHub release. `pattern` = package name WITHOUT version (supports `*`).
    GitHub {
        repo: String,
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        prerelease: bool,
    },
    /// A plain download link.
    Direct { url: String },
    /// A custom download page: we scrape links from the HTML.
    Site {
        page_url: String,
        #[serde(default)]
        link_regex: String,
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        version_regex: String,
    },
    /// You download / pick it yourself.
    Manual,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::GitHub { .. } => "GitHub",
            Source::Direct { .. } => "Direct link",
            Source::Site { .. } => "Download page",
            Source::Manual => "Manual",
        }
    }
    pub fn can_check(&self) -> bool {
        !matches!(self, Source::Manual)
    }
    pub fn describe(&self) -> String {
        match self {
            Source::GitHub { repo, pattern, .. } if pattern.is_empty() => repo.clone(),
            Source::GitHub { repo, pattern, .. } => format!("{repo}  ·  {pattern}"),
            Source::Direct { url } => url.clone(),
            Source::Site { page_url, .. } => page_url.clone(),
            Source::Manual => "added manually".into(),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub source: Source,
    #[serde(default)]
    pub version: Option<String>,
    /// Relative to the app folder, or absolute (linked external app).
    #[serde(default)]
    pub exe: Option<String>,
    #[serde(default)]
    pub args: String,
    /// false = a linked external program: we never delete its files.
    #[serde(default = "yes")]
    pub managed: bool,
}

impl AppEntry {
    pub fn dir(&self, paths: &Paths) -> PathBuf {
        paths.apps().join(&self.id)
    }
    pub fn exe_path(&self, paths: &Paths) -> Option<PathBuf> {
        let e = self.exe.as_ref()?;
        let p = Path::new(e);
        Some(if p.is_absolute() { p.to_path_buf() } else { self.dir(paths).join(p) })
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub github_token: String,
    #[serde(default = "yes")]
    pub auto_check: bool,
}

#[derive(Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn detect() -> Self {
        let root = if let Some(p) = std::env::var_os("PAM_DATA") {
            PathBuf::from(p)
        } else {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|p| p.join("pam-data")))
                .unwrap_or_else(|| PathBuf::from("pam-data"))
        };
        let _ = std::fs::create_dir_all(&root);
        Paths { root }
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }
    pub fn apps(&self) -> PathBuf {
        self.root.join("apps")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.config())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = paths.root.join("config.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, paths.config()).map_err(|e| e.to_string())
    }
}
