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
    /// Folder path, "/" separated for nesting, e.g. "Games/Emulators". Empty = top level.
    #[serde(default)]
    pub group: String,
    /// true = the user picked their own icon; it is stored as `<data>/icons/<id>.png`
    #[serde(default)]
    pub custom_icon: bool,
}

impl AppEntry {
    /// A program that is only linked (not copied into the data folder).
    pub fn is_link(&self) -> bool {
        matches!(self.source, Source::Manual) && !self.managed
    }
    /// Short badge text: "Link" for linked programs, otherwise the source type.
    pub fn kind_label(&self) -> &'static str {
        if self.is_link() { "Link" } else { self.source.label() }
    }
    pub fn describe(&self) -> String {
        if self.is_link() {
            match &self.exe {
                Some(e) => format!("linked: {e}"),
                None => "linked program (no exe selected)".into(),
            }
        } else {
            self.source.describe()
        }
    }
    pub fn icon_path(&self, paths: &Paths) -> PathBuf {
        paths.icons().join(format!("{}.png", crate::util::slug(&self.id)))
    }
    pub fn dir(&self, paths: &Paths) -> PathBuf {
        // slug() makes sure an id can never point outside the apps folder (no "..", no absolute paths)
        paths.apps().join(crate::util::slug(&self.id))
    }
    pub fn exe_path(&self, paths: &Paths) -> Option<PathBuf> {
        let e = self.exe.as_ref()?;
        let p = Path::new(e);
        Some(if p.is_absolute() { p.to_path_buf() } else { self.dir(paths).join(p) })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum ViewMode {
    #[default]
    List,
    Tiles,
}

fn default_tile() -> f32 {
    128.0
}

fn default_group_tile() -> f32 {
    110.0
}

fn default_per_row() -> u32 {
    3
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub github_token: String,
    #[serde(default = "yes")]
    pub auto_check: bool,
    #[serde(default)]
    pub view: ViewMode,
    /// current tile (icon) size - saved on exit when `remember_window` is on
    #[serde(default = "default_tile")]
    pub tile_size: f32,
    /// size used when "remember" is off, or when you press "use default now"
    #[serde(default = "default_tile")]
    pub default_tile_size: f32,
    /// max icons per row in a group popup window (the window is sized to fit it)
    #[serde(default = "default_per_row", alias = "tile_columns")]
    pub max_per_row: u32,
    /// icon size inside the group popup window (independent of the main window's icon size)
    #[serde(default = "default_group_tile")]
    pub group_tile_size: f32,
    /// save window size + icon size when closing, restore them on start
    #[serde(default = "yes")]
    pub remember_window: bool,
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    #[serde(default)]
    pub window_maximized: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            apps: vec![],
            github_token: String::new(),
            auto_check: true,
            view: ViewMode::List,
            tile_size: default_tile(),
            default_tile_size: default_tile(),
            max_per_row: default_per_row(),
            group_tile_size: default_group_tile(),
            remember_window: true,
            window_size: None,
            window_maximized: false,
        }
    }
}

#[derive(Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// PAM_DATA environment variable = exact data folder (overrides everything).
    pub fn env_override() -> Option<PathBuf> {
        std::env::var_os("PAM_DATA").map(PathBuf::from)
    }

    pub fn exe_dir() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Small pointer file next to the exe that remembers the chosen location.
    pub fn pointer_file() -> PathBuf {
        Self::exe_dir().join("pam-location.txt")
    }

    pub fn read_pointer() -> Option<PathBuf> {
        let s = std::fs::read_to_string(Self::pointer_file()).ok()?;
        let p = PathBuf::from(s.trim());
        (!p.as_os_str().is_empty()).then_some(p)
    }

    /// `Some(folder)` = use that folder as the base, `None` = back to the app folder.
    pub fn write_pointer(base: Option<&Path>) -> Result<(), String> {
        match base {
            Some(b) => std::fs::write(Self::pointer_file(), b.to_string_lossy().as_bytes()).map_err(|e| e.to_string()),
            None => match std::fs::remove_file(Self::pointer_file()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
        }
    }

    /// The data folder that lives inside a chosen base folder.
    pub fn root_for(base: &Path) -> PathBuf {
        base.join("pam-data")
    }

    pub fn at(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Paths { root }
    }

    /// Order: PAM_DATA  >  folder chosen in Settings (pam-location.txt)  >  next to the exe.
    pub fn detect() -> Self {
        if let Some(p) = Self::env_override() {
            return Self::at(p);
        }
        if let Some(base) = Self::read_pointer() {
            let root = Self::root_for(&base);
            if std::fs::create_dir_all(&root).is_ok() {
                return Paths { root };
            }
        }
        Self::at(Self::root_for(&Self::exe_dir()))
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }
    pub fn apps(&self) -> PathBuf {
        self.root.join("apps")
    }
    pub fn icons(&self) -> PathBuf {
        self.root.join("icons")
    }
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Self {
        let mut cfg = match std::fs::read_to_string(paths.config()) {
            Ok(text) => match serde_json::from_str::<Config>(&text) {
                Ok(c) => c,
                Err(_) => {
                    // keep the unreadable file instead of silently overwriting it on the next save
                    let _ = std::fs::write(paths.root.join("config.json.bad"), &text);
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        };
        for a in cfg.apps.iter_mut() {
            a.group = crate::util::norm_group(&a.group); // groups are one level only
        }
        cfg
    }
    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = paths.root.join("config.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, paths.config()).map_err(|e| e.to_string())
    }
}
