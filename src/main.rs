#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod icons;
mod install;
mod model;
mod net;
mod util;
mod version;

use eframe::egui::{self, Color32, RichText};
use model::{AppEntry, Config, Paths, Source, ViewMode};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

// ------------------------------------------------------------------ messages

struct Installed {
    version: String,
    exe: Option<String>,
}

enum Msg {
    Progress { id: String, text: String, frac: Option<f32> },
    Checked { id: String, res: Result<net::Remote, String> },
    /// version read from the exe of a manual / linked app (`None` = could not be read)
    LocalVersion { id: String, version: Option<String> },
    Installed { id: String, res: Result<Installed, String> },
    Icon { id: String, key: String, img: Option<egui::ColorImage> },
    /// finished copying the data folder to a new location (`None` = the app folder)
    DataMoved { base: Option<PathBuf>, res: Result<(), String> },
}

struct DataMove {
    /// chosen base folder; `None` = back to the folder next to the exe
    base: Option<PathBuf>,
    root: PathBuf,
    can_copy: bool,
    copy: bool,
}

struct IconSlot {
    key: String,
    tex: Option<egui::TextureHandle>,
}

enum InstallSrc {
    /// Download from the source (resolve first if `None`).
    Remote(Option<net::Remote>),
    /// A file the user downloaded / picked themselves.
    File(PathBuf),
}

fn run_install(
    paths: &Paths,
    entry: &AppEntry,
    token: &str,
    src: InstallSrc,
    progress: &dyn Fn(String, Option<f32>),
) -> Result<Installed, String> {
    let dir = entry.dir(paths);
    let tmp = paths.tmp();
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    let (version, filename, file, is_temp) = match src {
        InstallSrc::Remote(r) => {
            let r = match r {
                Some(r) => r,
                None => {
                    progress("Resolving latest version…".into(), None);
                    net::resolve(&entry.source, token)?
                }
            };
            let dest = tmp.join(format!("{}-{}", util::slug(&entry.id), util::sanitize(&r.filename)));
            let label = format!("Downloading {}", r.filename);
            progress(label.clone(), Some(0.0));
            net::download(&r.url, &dest, |done, total| {
                progress(label.clone(), total.filter(|t| *t > 0).map(|t| done as f32 / t as f32));
            })?;
            (r.version, r.filename, dest, true)
        }
        InstallSrc::File(p) => {
            let fname = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (util::find_version(&fname).unwrap_or_else(|| "manual".into()), fname, p, false)
        }
    };

    progress("Installing…".into(), None);
    let res = install::install_file(&file, &filename, &dir);
    if is_temp {
        let _ = fs::remove_file(&file);
    }
    res?;

    let exe_ok = entry.exe.as_ref().map(|e| dir.join(e).exists()).unwrap_or(false);
    let exe = if exe_ok {
        None
    } else {
        install::pick_default(&entry.name, &install::find_executables(&dir))
    };
    Ok(Installed { version, exe })
}

// ------------------------------------------------------------------ add / edit form

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    GitHub,
    Direct,
    Site,
    Manual,
}

#[derive(PartialEq, Clone, Copy)]
enum ManualMode {
    Import,
    Link,
    /// just create an empty app folder; the user installs / copies the app into it
    Folder,
}

struct Detected {
    kind: Kind,
    pattern: Option<String>,
    name: String,
}

/// "owner/repo", "github.com/owner/repo", ".../releases/download/tag/file.zip" -> (repo, file)
fn parse_github(s: &str) -> Option<(String, Option<String>)> {
    let s = s.trim();
    let (rest, prefixed) = if let Some(r) = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")) {
        let r = r.strip_prefix("www.").unwrap_or(r);
        (r.strip_prefix("github.com/")?, true)
    } else if let Some(r) = s.strip_prefix("github.com/") {
        (r, true)
    } else {
        (s, false)
    };
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let valid = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !prefixed && (parts.len() != 2 || parts[0].contains('.') || !valid(parts[0]) || !valid(parts[1])) {
        return None;
    }
    // validate in BOTH cases (the token is sent with these requests): no "?", "#", "%" ... in owner / repo
    let name = parts[1].split(|c: char| c == '?' || c == '#').next().unwrap_or("").trim_end_matches(".git");
    if !valid(parts[0]) || !valid(name) {
        return None;
    }
    let repo = format!("{}/{}", parts[0], name);
    let file = (parts.len() >= 5 && parts[2] == "releases" && parts[3] == "download")
        .then(|| percent_encoding::percent_decode_str(parts[parts.len() - 1]).decode_utf8_lossy().to_string());
    Some((repo, file))
}

fn detect(input: &str) -> Option<Detected> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((repo, file)) = parse_github(s) {
        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        return Some(Detected { kind: Kind::GitHub, pattern: file.map(|f| util::strip_version(&f)), name });
    }
    let u = Url::parse(s).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
    let file = net::filename_from_url(&u);
    let l = file.to_lowercase();
    let is_file = [".zip", ".exe", ".7z", ".tar.gz", ".tgz", ".tar.xz", ".appimage", ".msi"].iter().any(|e| l.ends_with(e));
    if is_file {
        let name = util::strip_version(&util::strip_ext(&file)).trim_matches(|c: char| "-_. ".contains(c)).to_string();
        Some(Detected { kind: Kind::Direct, pattern: None, name })
    } else {
        Some(Detected { kind: Kind::Site, pattern: None, name: u.host_str().unwrap_or("").trim_start_matches("www.").to_string() })
    }
}

struct Form {
    editing: Option<String>,
    name: String,
    auto_name: String,
    kind: Kind,
    locked: bool,
    input: String,
    last_input: String,
    pattern: String,
    prerelease: bool,
    link_regex: String,
    version_regex: String,
    manual_mode: ManualMode,
    manual_file: Option<PathBuf>,
    exe: String,
    exes: Vec<String>,
    args: String,
    group: String,
    /// newly picked icon image (applied on save)
    icon_src: Option<PathBuf>,
    /// go back to the automatic icon on save
    icon_clear: bool,
    had_icon: bool,
    error: String,
}

impl Form {
    fn new() -> Self {
        Form {
            editing: None,
            name: String::new(),
            auto_name: String::new(),
            kind: Kind::GitHub,
            locked: false,
            input: String::new(),
            last_input: String::new(),
            pattern: String::new(),
            prerelease: false,
            link_regex: String::new(),
            version_regex: String::new(),
            manual_mode: ManualMode::Import,
            manual_file: None,
            exe: String::new(),
            exes: vec![],
            args: String::new(),
            group: String::new(),
            icon_src: None,
            icon_clear: false,
            had_icon: false,
            error: String::new(),
        }
    }

    fn from_entry(e: &AppEntry, paths: &Paths) -> Self {
        let mut f = Form::new();
        f.editing = Some(e.id.clone());
        f.name = e.name.clone();
        f.locked = true;
        f.args = e.args.clone();
        f.group = e.group.clone();
        f.had_icon = e.custom_icon;
        if e.is_link() {
            f.manual_mode = ManualMode::Link;
        }
        f.exe = e.exe.clone().unwrap_or_default();
        f.exes = install::find_executables(&e.dir(paths));
        match &e.source {
            Source::GitHub { repo, pattern, prerelease } => {
                f.kind = Kind::GitHub;
                f.input = repo.clone();
                f.pattern = pattern.clone();
                f.prerelease = *prerelease;
            }
            Source::Direct { url } => {
                f.kind = Kind::Direct;
                f.input = url.clone();
            }
            Source::Site { page_url, link_regex, pattern, version_regex } => {
                f.kind = Kind::Site;
                f.input = page_url.clone();
                f.link_regex = link_regex.clone();
                f.pattern = pattern.clone();
                f.version_regex = version_regex.clone();
            }
            Source::Manual => f.kind = Kind::Manual,
        }
        f.last_input = f.input.clone();
        f
    }

    fn on_input_changed(&mut self) {
        if self.input == self.last_input {
            return;
        }
        self.last_input = self.input.clone();
        if self.editing.is_some() {
            return;
        }
        if let Some(d) = detect(&self.input) {
            if !self.locked {
                self.kind = d.kind;
            }
            if let Some(p) = d.pattern {
                self.pattern = p;
            }
            if self.name.is_empty() || self.name == self.auto_name {
                self.name = d.name.clone();
            }
            self.auto_name = d.name;
        }
    }

    fn to_source(&self) -> Result<Source, String> {
        let input = self.input.trim();
        match self.kind {
            Kind::GitHub => {
                let (repo, _) = parse_github(input).ok_or("Enter owner/repo or a github.com URL")?;
                Ok(Source::GitHub { repo, pattern: self.pattern.trim().into(), prerelease: self.prerelease })
            }
            Kind::Direct => {
                Url::parse(input).map_err(|_| "Enter a valid http(s) download link")?;
                Ok(Source::Direct { url: input.into() })
            }
            Kind::Site => {
                Url::parse(input).map_err(|_| "Enter a valid http(s) page URL")?;
                for (label, r) in [("link", &self.link_regex), ("version", &self.version_regex)] {
                    if !r.trim().is_empty() {
                        regex::Regex::new(r).map_err(|e| format!("Bad {label} regex: {e}"))?;
                    }
                }
                Ok(Source::Site {
                    page_url: input.into(),
                    link_regex: self.link_regex.trim().into(),
                    pattern: self.pattern.trim().into(),
                    version_regex: self.version_regex.trim().into(),
                })
            }
            Kind::Manual => Ok(Source::Manual),
        }
    }
}

// ------------------------------------------------------------------ app

enum Act {
    Launch,
    Update,
    Check,
    Reinstall,
    Import,
    OpenDir,
    Edit,
    /// look for an executable inside the app folder and select it
    Detect,
    /// pick a custom icon image for the app
    SetIcon,
    /// go back to the automatic icon
    ResetIcon,
    /// dissolve a group (the id slot carries the group path)
    Ungroup,
    /// rename a group (the id slot carries the group name)
    RenameGroup,
    Remove,
    /// open a folder in tile view (the id slot carries the group path)
    Group,
}

struct Manager {
    paths: Paths,
    cfg: Config,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    busy: HashMap<String, (String, Option<f32>)>,
    latest: HashMap<String, net::Remote>,
    errors: HashMap<String, String>,
    form: Option<Form>,
    filter: String,
    remove: Option<(String, bool)>,
    show_settings: bool,
    toast: Option<(String, Instant)>,
    /// the group whose popup window is open (tile view)
    open_group: Option<String>,
    /// bumped every time a group popup is opened, so it starts at a fresh, fitted size
    group_gen: u32,
    /// group rename dialog: (old name, text being edited)
    rename: Option<(String, String)>,
    import_confirm: bool,
    data_move: Option<DataMove>,
    moving: bool,
    icons: HashMap<String, IconSlot>,
    icon_pending: HashSet<String>,
    /// bumped whenever a custom icon is set / reset, so cached icons are reloaded
    icon_rev: u32,
    last_win: Option<[f32; 2]>,
    win_maxi: bool,
}

fn truncate(s: &str, max: usize) -> String {
    let max = max.max(4);
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

enum TileItem<'a> {
    /// a group (folder) tile, by group name
    Folder(String),
    App(&'a AppEntry),
}

impl Manager {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let paths = Paths::detect();
        let cfg = Config::load(&paths);
        let (tx, rx) = channel();
        let mut m = Manager {
            paths,
            cfg,
            tx,
            rx,
            busy: HashMap::new(),
            latest: HashMap::new(),
            errors: HashMap::new(),
            form: None,
            filter: String::new(),
            remove: None,
            show_settings: false,
            toast: None,
            open_group: None,
            group_gen: 0,
            rename: None,
            import_confirm: false,
            data_move: None,
            moving: false,
            icons: HashMap::new(),
            icon_pending: HashSet::new(),
            icon_rev: 0,
            last_win: None,
            win_maxi: false,
        };
        if !m.cfg.remember_window {
            m.cfg.tile_size = m.cfg.default_tile_size;
        }
        if m.cfg.auto_check {
            m.check_all(&cc.egui_ctx);
        }
        m
    }

    fn save(&mut self) {
        if let Err(e) = self.cfg.save(&self.paths) {
            self.say(format!("Could not save config: {e}"));
        }
    }

    fn say(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn entry(&self, id: &str) -> Option<&AppEntry> {
        self.cfg.apps.iter().find(|a| a.id == id)
    }

    fn update_available(&self, e: &AppEntry) -> bool {
        match (self.latest.get(&e.id), &e.version) {
            (Some(r), Some(v)) => r.version != "?" && &r.version != v,
            (Some(_), None) => true,
            _ => false,
        }
    }

    // ---- background work

    fn check_all(&mut self, ctx: &egui::Context) {
        // manual / linked apps have no update source: their version is read from the exe instead
        let ids: Vec<String> = self.cfg.apps.iter().filter(|a| a.source.can_check() || a.exe.is_some()).map(|a| a.id.clone()).collect();
        for id in ids {
            self.start_check(ctx, &id);
        }
    }

    fn start_check(&mut self, ctx: &egui::Context, id: &str) {
        let Some(e) = self.entry(id).cloned() else { return };
        if self.busy.contains_key(id) {
            return;
        }
        if !e.source.can_check() {
            // manual app: read the version from the executable (cheap, local)
            let Some(exe) = e.exe_path(&self.paths) else { return };
            let (tx, ctx, id) = (self.tx.clone(), ctx.clone(), id.to_string());
            thread::spawn(move || {
                let version = version::exe_version(&exe);
                let _ = tx.send(Msg::LocalVersion { id, version });
                ctx.request_repaint();
            });
            return;
        }
        self.busy.insert(id.into(), ("Checking for updates…".into(), None));
        let (tx, ctx, token, id) = (self.tx.clone(), ctx.clone(), self.cfg.github_token.clone(), id.to_string());
        thread::spawn(move || {
            let res = net::resolve(&e.source, &token);
            let _ = tx.send(Msg::Checked { id, res });
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context, id: &str, src: InstallSrc) {
        let Some(e) = self.entry(id).cloned() else { return };
        if self.busy.contains_key(id) {
            return;
        }
        self.busy.insert(id.into(), ("Starting…".into(), None));
        let (tx, ctx, paths, token, id) =
            (self.tx.clone(), ctx.clone(), self.paths.clone(), self.cfg.github_token.clone(), id.to_string());
        thread::spawn(move || {
            let progress = |text: String, frac: Option<f32>| {
                let _ = tx.send(Msg::Progress { id: id.clone(), text, frac });
                ctx.request_repaint();
            };
            let res = run_install(&paths, &e, &token, src, &progress);
            let _ = tx.send(Msg::Installed { id: id.clone(), res });
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress { id, text, frac } => {
                    if let Some(b) = self.busy.get_mut(&id) {
                        *b = (text, frac);
                    }
                }
                Msg::LocalVersion { id, version } => {
                    if let Some(v) = version {
                        if !self.busy.contains_key(&id) {
                            if let Some(e) = self.cfg.apps.iter_mut().find(|a| a.id == id) {
                                if e.version.as_deref() != Some(v.as_str()) {
                                    e.version = Some(v);
                                    self.save();
                                }
                            }
                        }
                    }
                }
                Msg::Checked { id, res } => {
                    self.busy.remove(&id);
                    match res {
                        Ok(r) => {
                            self.errors.remove(&id);
                            self.latest.insert(id, r);
                        }
                        Err(e) => {
                            self.errors.insert(id, e);
                        }
                    }
                }
                Msg::DataMoved { base, res } => {
                    self.moving = false;
                    match res {
                        Ok(()) => self.finish_switch(base),
                        Err(e) => self.say(format!("Copying the data folder failed: {e}")),
                    }
                }
                Msg::Icon { id, key, img } => {
                    self.icon_pending.remove(&id);
                    let tex = img.map(|i| ctx.load_texture(format!("icon-{id}"), i, egui::TextureOptions::LINEAR));
                    self.icons.insert(id, IconSlot { key, tex });
                }
                Msg::Installed { id, res } => {
                    self.busy.remove(&id);
                    match res {
                        Ok(ins) => {
                            let mut name = String::new();
                            if let Some(e) = self.cfg.apps.iter_mut().find(|a| a.id == id) {
                                e.version = Some(ins.version.clone());
                                if let Some(x) = ins.exe {
                                    e.exe = Some(x);
                                }
                                name = e.name.clone();
                            }
                            self.errors.remove(&id);
                            self.save();
                            self.say(format!("{name} installed ({})", ins.version));
                        }
                        Err(e) => {
                            self.errors.insert(id, e);
                        }
                    }
                }
            }
        }
    }

    // ---- actions

    fn act(&mut self, ctx: &egui::Context, id: String, act: Act) {
        match act {
            Act::Launch => {
                if let Some(e) = self.entry(&id) {
                    let r = match e.exe_path(&self.paths) {
                        Some(p) => install::launch(&p, &e.args),
                        None => Err("No executable selected - use Edit".into()),
                    };
                    if let Err(err) = r {
                        self.errors.insert(id, err);
                    } else {
                        self.errors.remove(&id);
                        self.open_group = None; // like the Start menu: launching closes the folder
                    }
                }
            }
            Act::Update => {
                let src = InstallSrc::Remote(self.latest.get(&id).cloned());
                self.start_install(ctx, &id, src);
            }
            Act::Reinstall => self.start_install(ctx, &id, InstallSrc::Remote(None)),
            Act::Check => self.start_check(ctx, &id),
            Act::Import => {
                if let Some(f) = rfd::FileDialog::new().set_title("Pick the downloaded file (zip / 7z / tar.gz / exe / AppImage)").pick_file() {
                    self.start_install(ctx, &id, InstallSrc::File(f));
                }
            }
            Act::OpenDir => {
                if let Some(e) = self.entry(&id) {
                    let d = e.dir(&self.paths);
                    let _ = fs::create_dir_all(&d);
                    let _ = open::that(d);
                }
            }
            Act::Edit => {
                if let Some(e) = self.entry(&id) {
                    self.form = Some(Form::from_entry(e, &self.paths));
                }
            }
            Act::Detect => {
                let Some(e) = self.entry(&id).cloned() else { return };
                let dir = e.dir(&self.paths);
                let list = install::find_executables(&dir);
                match install::pick_default(&e.name, &list) {
                    Some(x) => {
                        if let Some(m) = self.cfg.apps.iter_mut().find(|a| a.id == id) {
                            m.exe = Some(x.clone());
                        }
                        self.errors.remove(&id);
                        self.save();
                        self.say(format!("Executable set to {x}"));
                    }
                    None => {
                        self.errors.insert(id, format!("No executable found in {} - copy the app there first", dir.display()));
                    }
                }
            }
            Act::SetIcon => {
                if let Some(p) = rfd::FileDialog::new()
                    .set_title("Pick an icon (png / jpg / bmp / ico, or an exe / dll to take its icon)")
                    .add_filter("Images and programs", &["png", "jpg", "jpeg", "bmp", "ico", "exe", "dll"])
                    .pick_file()
                {
                    match self.apply_icon(&id, &p) {
                        Ok(()) => self.say("Icon changed"),
                        Err(e) => self.say(format!("Could not use that icon: {e}")),
                    }
                }
            }
            Act::ResetIcon => {
                self.clear_icon(&id);
                self.say("Icon reset to automatic");
            }
            Act::Ungroup => self.ungroup(&id),
            Act::RenameGroup => self.rename = Some((id.clone(), id)),
            Act::Group => {
                // click a folder tile: open its popup (click again to close)
                self.open_group = if id.is_empty() || self.open_group.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    self.group_gen = self.group_gen.wrapping_add(1);
                    Some(id)
                };
            }
            Act::Remove => {
                // file deletion is opt-in every time: the box starts unchecked
                self.remove = Some((id, false));
            }
        }
    }

    /// Save `src` as the app's own icon (converted to a small PNG inside the data folder).
    fn apply_icon(&mut self, id: &str, src: &std::path::Path) -> Result<(), String> {
        let Some(e) = self.entry(id) else { return Err("App not found".into()) };
        let dest = e.icon_path(&self.paths);
        icons::import_custom(src, &dest)?;
        if let Some(e) = self.cfg.apps.iter_mut().find(|a| a.id == id) {
            e.custom_icon = true;
        }
        self.icon_rev = self.icon_rev.wrapping_add(1);
        self.save();
        Ok(())
    }

    fn clear_icon(&mut self, id: &str) {
        if let Some(e) = self.entry(id) {
            let _ = fs::remove_file(e.icon_path(&self.paths));
        }
        if let Some(e) = self.cfg.apps.iter_mut().find(|a| a.id == id) {
            e.custom_icon = false;
        }
        self.icon_rev = self.icon_rev.wrapping_add(1);
        self.save();
    }

    fn submit_form(&mut self, ctx: &egui::Context, f: &Form) -> Result<(), String> {
        let name = f.name.trim();
        if name.is_empty() {
            return Err("Give the app a name".into());
        }
        let source = f.to_source()?;

        if let Some(id) = &f.editing {
            if let Some(e) = self.cfg.apps.iter_mut().find(|a| &a.id == id) {
                e.name = name.into();
                e.source = source;
                e.args = f.args.trim().into();
                e.group = util::norm_group(&f.group);
                e.exe = (!f.exe.trim().is_empty()).then(|| f.exe.trim().to_string());
            }
            self.latest.remove(id);
            self.save();
            if f.icon_clear {
                self.clear_icon(id);
            }
            if let Some(p) = &f.icon_src {
                if let Err(e) = self.apply_icon(id, p) {
                    self.say(format!("Saved, but the icon could not be used: {e}"));
                }
            }
            return Ok(());
        }

        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let id = format!("{}-{:x}", util::slug(name), nanos % 0xFFFFFF);
        let mut entry = AppEntry {
            id: id.clone(),
            name: name.into(),
            source: source.clone(),
            version: None,
            exe: None,
            args: f.args.trim().into(),
            managed: true,
            group: util::norm_group(&f.group),
            custom_icon: false,
        };

        let mut to_install: Option<InstallSrc> = None;
        let mut new_folder: Option<PathBuf> = None;
        match (&source, f.manual_mode) {
            (Source::Manual, ManualMode::Link) => {
                let p = f.manual_file.as_ref().ok_or("Pick the executable to link")?;
                entry.managed = false;
                entry.exe = Some(p.to_string_lossy().to_string());
                entry.version = Some("external".into());
            }
            (Source::Manual, ManualMode::Folder) => {
                let d = entry.dir(&self.paths);
                fs::create_dir_all(&d).map_err(|e| format!("Cannot create folder: {e}"))?;
                entry.version = Some("manual".into());
                new_folder = Some(d);
            }
            (Source::Manual, ManualMode::Import) => {
                let p = f.manual_file.clone().ok_or("Pick the file you downloaded")?;
                to_install = Some(InstallSrc::File(p));
            }
            _ => to_install = Some(InstallSrc::Remote(None)),
        }
        self.cfg.apps.push(entry);
        self.save();
        if let Some(p) = &f.icon_src {
            if let Err(e) = self.apply_icon(&id, p) {
                self.say(format!("App added, but the icon could not be used: {e}"));
            }
        }
        if let Some(src) = to_install {
            self.start_install(ctx, &id, src);
        }
        if let Some(d) = new_folder {
            let _ = open::that(&d);
            self.say(format!(
                "Folder created: {} - put the app in it, then use ⋮ → Detect executable (or Edit) to pick the exe",
                d.display()
            ));
        }
        Ok(())
    }

    // ---- export / import (app list only, no downloaded files, no token)

    fn export_apps(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export app list")
            .set_file_name("portable-apps.json")
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        let apps: Vec<AppEntry> = self
            .cfg
            .apps
            .iter()
            .cloned()
            .map(|mut a| {
                if a.managed {
                    a.version = None; // will be (re)installed on the other PC
                }
                a.custom_icon = false; // icon files are not part of the export
                a
            })
            .collect();
        let n = apps.len();
        let doc = serde_json::json!({ "format": "portable-app-manager-export", "version": 1, "apps": apps });
        let res = serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())
            .and_then(|s| fs::write(&path, s).map_err(|e| e.to_string()));
        match res {
            Ok(()) => self.say(format!("Exported {n} apps to {}", path.display())),
            Err(e) => self.say(format!("Export failed: {e}")),
        }
    }

    /// Called after the user confirmed the trust warning.
    fn import_apps(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import app list")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let res: Result<(usize, usize, usize), String> = (|| {
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("Not valid JSON: {e}"))?;
            let list = v.get("apps").cloned().ok_or("This file has no \"apps\" list")?;
            let apps: Vec<AppEntry> = serde_json::from_value(list).map_err(|e| format!("Unreadable app list: {e}"))?;
            let (mut added, mut skipped, mut relink) = (0, 0, 0);
            for (n, mut a) in apps.into_iter().enumerate() {
                a.name = a.name.trim().to_string();
                if a.name.is_empty() {
                    a.name = "app".into();
                }
                let dup = self
                    .cfg
                    .apps
                    .iter()
                    .any(|x| x.id == a.id || (x.name.eq_ignore_ascii_case(&a.name) && x.source == a.source));
                if dup {
                    skipped += 1;
                    continue;
                }
                // ids become folder names: never trust them
                if !util::valid_id(&a.id) {
                    a.id = format!("{}-{:x}", util::slug(&a.name), (nanos + n as u128) % 0xFFFFFF);
                }
                if a.managed {
                    a.version = None;
                    // the exe must be a plain relative path inside the app folder
                    if let Some(e) = &a.exe {
                        let p = std::path::Path::new(e);
                        if p.is_absolute() || !p.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
                            a.exe = None;
                        }
                    }
                } else {
                    // linked programs point at a path on the OLD pc: never import it, re-link on this PC
                    a.exe = None;
                    relink += 1;
                }
                a.group = util::norm_group(&a.group);
                a.custom_icon = false; // never trust an icon flag from a file
                self.cfg.apps.push(a);
                added += 1;
            }
            Ok((added, skipped, relink))
        })();
        match res {
            Ok((added, skipped, relink)) => {
                self.save();
                let mut msg = format!("Imported {added} app(s), skipped {skipped} already present. Use “Install / update all” to download them.");
                if relink > 0 {
                    msg.push_str(&format!(" {relink} linked program(s) need their exe re-selected (Edit)."));
                }
                self.say(msg);
            }
            Err(e) => self.say(format!("Import failed: {e}")),
        }
    }

    /// Ask before switching the data folder; nothing changes until the user confirms.
    fn begin_move(&mut self, base: Option<PathBuf>) {
        let root = Paths::root_for(&base.clone().unwrap_or_else(Paths::exe_dir));
        if root == self.paths.root {
            self.say("That is already the data folder");
            return;
        }
        if root.starts_with(&self.paths.root) {
            self.say("Choose a folder outside the current data folder");
            return;
        }
        if !self.busy.is_empty() {
            self.say("Wait until the running downloads have finished");
            return;
        }
        let can_copy = !root.join("config.json").exists();
        self.data_move = Some(DataMove { base, root, can_copy, copy: can_copy });
    }

    /// Point the app at the new data folder (the old one is left untouched).
    fn finish_switch(&mut self, base: Option<PathBuf>) {
        let root = Paths::root_for(&base.clone().unwrap_or_else(Paths::exe_dir));
        if let Err(e) = Paths::write_pointer(base.as_deref()) {
            self.say(format!("Could not save the new location (is the app folder writable?): {e}"));
            return;
        }
        self.paths = Paths::at(root);
        self.cfg = Config::load(&self.paths);
        self.latest.clear();
        self.errors.clear();
        self.icons.clear();
        self.icon_pending.clear();
        self.open_group = None;
        self.say(format!("Data folder is now {} (the old folder was left in place)", self.paths.root.display()));
    }

    // ---- UI pieces

    // ---- groups

    /// All group names (groups are one level only), sorted.
    fn all_groups(&self) -> Vec<String> {
        let set: BTreeSet<String> = self.cfg.apps.iter().filter(|a| !a.group.is_empty()).map(|a| a.group.clone()).collect();
        set.into_iter().collect()
    }

    fn in_group<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a AppEntry> + 'a {
        self.cfg.apps.iter().filter(move |a| a.group == name)
    }

    // ---- icons

    fn ensure_icons(&mut self, ctx: &egui::Context) {
        for a in &self.cfg.apps {
            let exe = a.exe_path(&self.paths);
            let key = format!(
                "{}|{}|{}|{}",
                exe.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                a.version.clone().unwrap_or_default(),
                a.custom_icon,
                self.icon_rev
            );
            let fresh = self.icons.get(&a.id).map(|s| s.key == key).unwrap_or(false);
            if fresh || self.icon_pending.contains(&a.id) {
                continue;
            }
            let custom = a.custom_icon.then(|| a.icon_path(&self.paths));
            if exe.is_none() && custom.is_none() {
                self.icons.insert(a.id.clone(), IconSlot { key, tex: None });
                continue;
            }
            self.icon_pending.insert(a.id.clone());
            let (tx, ctx, id, dir) = (self.tx.clone(), ctx.clone(), a.id.clone(), a.dir(&self.paths));
            thread::spawn(move || {
                // the user's own icon wins; if it can't be read, fall back to the automatic one
                let img = custom
                    .and_then(|p| icons::load_custom(&p))
                    .or_else(|| exe.and_then(|x| icons::load(&x, &dir)));
                let _ = tx.send(Msg::Icon { id, key, img });
                ctx.request_repaint();
            });
        }
    }

    /// Real icon if we have one, otherwise a coloured tile with the first letter.
    fn paint_icon(&self, p: &egui::Painter, rect: egui::Rect, e: &AppEntry) {
        if let Some(t) = self.icons.get(&e.id).and_then(|s| s.tex.as_ref()) {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            p.image(t.id(), rect, uv, Color32::WHITE);
        } else {
            let h = e.name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            let c = Color32::from_rgb(
                50 + (h & 0x5f) as u8,
                50 + ((h >> 8) & 0x5f) as u8,
                70 + ((h >> 16) & 0x5f) as u8,
            );
            p.rect_filled(rect, rect.width() * 0.2, c);
            let letter = e.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
            p.text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(rect.height() * 0.5), Color32::WHITE);
        }
    }

    fn icon(&self, ui: &mut egui::Ui, e: &AppEntry, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        self.paint_icon(ui.painter(), rect, e);
    }

    // ---- shared per-app menu (⋮ in list view, right-click in tile view)

    fn menu_items(&self, ui: &mut egui::Ui, e: &AppEntry, acts: &mut Vec<(String, Act)>, extra: bool) {
        let id = e.id.clone();
        let busy = self.busy.contains_key(&id);
        let manual = matches!(e.source, Source::Manual);
        let mut push = |ui: &mut egui::Ui, a: Act| {
            acts.push((id.clone(), a));
            ui.close_menu();
        };
        if extra {
            if ui.add_enabled(!busy && e.exe.is_some(), egui::Button::new("▶ Launch")).clicked() {
                push(ui, Act::Launch);
            }
            let need = self.update_available(e) || (e.version.is_none() && e.managed);
            if need && ui.add_enabled(!busy, egui::Button::new(if e.version.is_some() { "⬆ Update" } else { "⬇ Install" })).clicked() {
                push(ui, if manual { Act::Import } else { Act::Update });
            }
            ui.separator();
        }
        if e.source.can_check() && ui.button("Check for update").clicked() {
            push(ui, Act::Check);
        } else if !e.source.can_check() && e.exe.is_some() && ui.button("Refresh version").on_hover_text("Read the version from the executable").clicked() {
            push(ui, Act::Check);
        }
        if e.managed && e.source.can_check() && ui.button("Reinstall latest").clicked() {
            push(ui, Act::Reinstall);
        }
        if e.managed && ui.button("Install from downloaded file…").clicked() {
            push(ui, Act::Import);
        }
        if e.managed && ui.button("Open folder").clicked() {
            push(ui, Act::OpenDir);
        }
        if e.managed && ui.button("Detect executable").clicked() {
            push(ui, Act::Detect);
        }
        if ui.button("Edit… (name, group, exe)").clicked() {
            push(ui, Act::Edit);
        }
        if ui.button("Set icon…").clicked() {
            push(ui, Act::SetIcon);
        }
        if e.custom_icon && ui.button("Reset icon").clicked() {
            push(ui, Act::ResetIcon);
        }
        ui.separator();
        if ui.button(RichText::new("Remove…").color(Color32::LIGHT_RED)).clicked() {
            push(ui, Act::Remove);
        }
    }

    /// Dissolve a group: its apps become ungrouped (nothing is deleted).
    fn ungroup(&mut self, name: &str) {
        let mut n = 0;
        for a in self.cfg.apps.iter_mut() {
            if a.group == name {
                a.group.clear();
                n += 1;
            }
        }
        if self.open_group.as_deref() == Some(name) {
            self.open_group = None;
        }
        self.save();
        self.say(format!("Ungrouped “{name}” ({n} app(s))"));
    }

    /// Rename a group. If the new name already exists the two groups are merged.
    fn rename_group(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = util::norm_group(new);
        if new.is_empty() {
            return Err("Give the group a name".into());
        }
        if new == old {
            return Ok(());
        }
        let merged = self.cfg.apps.iter().any(|a| a.group == new);
        for a in self.cfg.apps.iter_mut() {
            if a.group == old {
                a.group = new.clone();
            }
        }
        if self.open_group.as_deref() == Some(old) {
            self.open_group = Some(new.clone());
        }
        self.save();
        self.say(if merged {
            format!("Merged “{old}” into “{new}”")
        } else {
            format!("Renamed “{old}” to “{new}”")
        });
        Ok(())
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some((old, mut text)) = self.rename.clone() else { return };
        let (mut ok, mut cancel) = (false, false);
        let mut open = true;
        egui::Window::new("Rename group")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("New name for “{old}”:"));
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(260.0));
                if !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                if text.trim() != old && self.all_groups().iter().any(|g| *g == util::norm_group(&text)) {
                    ui.label(RichText::new("A group with this name exists - the apps will be merged into it.").weak());
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Rename").strong()).clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if ok {
            match self.rename_group(&old, &text) {
                Ok(()) => self.rename = None,
                Err(e) => {
                    self.say(e);
                    self.rename = Some((old, text));
                }
            }
        } else if cancel || !open {
            self.rename = None;
        } else {
            self.rename = Some((old, text));
        }
    }

    // ---- list view (collapsible groups, one level)

    fn list_level(&self, ui: &mut egui::Ui, acts: &mut Vec<(String, Act)>) {
        for name in self.all_groups() {
            let n = self.in_group(&name).count();
            let resp = egui::CollapsingHeader::new(RichText::new(format!("📁 {name}  ({n})")).strong())
                .id_salt(("grp", name.clone()))
                .default_open(true)
                .show(ui, |ui| {
                    let mut apps: Vec<&AppEntry> = self.in_group(&name).collect();
                    apps.sort_by_key(|a| a.name.to_lowercase());
                    for e in apps {
                        self.card(ui, e, acts);
                    }
                });
            let hr = resp.header_response;
            hr.context_menu(|ui| {
                if ui.button("Rename group…").clicked() {
                    acts.push((name.clone(), Act::RenameGroup));
                    ui.close_menu();
                }
                if ui.button("Ungroup (move apps out of this group)").clicked() {
                    acts.push((name.clone(), Act::Ungroup));
                    ui.close_menu();
                }
            });
            hr.on_hover_text("Right-click for options");
        }
        let mut apps: Vec<&AppEntry> = self.cfg.apps.iter().filter(|a| a.group.is_empty()).collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        for e in apps {
            self.card(ui, e, acts);
        }
    }

    // ---- tile view: group tiles + ungrouped apps; clicking a group opens a popup window

    fn tiles_level(&self, ui: &mut egui::Ui, ts: f32, acts: &mut Vec<(String, Act)>) {
        let mut apps: Vec<&AppEntry> = self.cfg.apps.iter().filter(|a| a.group.is_empty()).collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        let mut items: Vec<TileItem> = self.all_groups().into_iter().map(TileItem::Folder).collect();
        items.extend(apps.into_iter().map(TileItem::App));
        self.tile_grid(ui, ts, 0, &items, acts);
    }

    /// Lay tiles out: as many per row as fit the window (`cols` = 0), or exactly `cols` per row
    /// at full size (used by the group popup, which sizes itself to fit).
    fn tile_grid(&self, ui: &mut egui::Ui, ts: f32, cols: usize, items: &[TileItem<'_>], acts: &mut Vec<(String, Act)>) {
        let gap = 10.0;
        let draw = |ui: &mut egui::Ui, it: &TileItem<'_>, size: f32, acts: &mut Vec<(String, Act)>| match it {
            TileItem::Folder(name) => self.folder_tile(ui, name, size, acts),
            TileItem::App(e) => self.tile(ui, e, size, acts),
        };
        if cols == 0 {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for it in items {
                    draw(ui, it, ts, acts);
                }
            });
        } else {
            // centre the whole block of `cols` tiles horizontally (rows stay left-aligned inside it)
            let block = cols as f32 * ts + gap * (cols as f32 - 1.0);
            let pad = ((ui.available_width() - block) / 2.0).max(0.0);
            for row in items.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(pad);
                    ui.spacing_mut().item_spacing.x = gap;
                    for it in row {
                        draw(ui, it, ts, acts);
                    }
                });
                ui.add_space(gap);
            }
        }
    }

    fn folder_tile(&self, ui: &mut egui::Ui, name: &str, ts: f32, acts: &mut Vec<(String, Act)>) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ts, ts * 1.2), egui::Sense::click());
        let is_open = self.open_group.as_deref() == Some(name);
        let vis = ui.visuals();
        let fill = if resp.hovered() || is_open { vis.widgets.hovered.bg_fill } else { vis.widgets.inactive.weak_bg_fill };
        let stroke = if is_open { vis.widgets.hovered.bg_stroke } else { vis.widgets.noninteractive.bg_stroke };
        let (txt, weak) = (vis.text_color(), vis.weak_text_color());
        let p = ui.painter();
        p.rect_filled(rect, 8.0, fill);
        p.rect_stroke(rect, 8.0, stroke);

        let members: Vec<&AppEntry> = self.in_group(name).collect();
        let area = ts * 0.56;
        let origin = egui::pos2(rect.center().x - area / 2.0, rect.top() + ts * 0.1);
        p.rect_filled(egui::Rect::from_min_size(origin - egui::vec2(5.0, 5.0), egui::vec2(area + 10.0, area + 10.0)), 8.0, Color32::from_white_alpha(18));
        let cell = area / 2.0;
        for (i, a) in members.iter().take(4).enumerate() {
            let (cx, cy) = ((i % 2) as f32, (i / 2) as f32);
            let r = egui::Rect::from_min_size(egui::pos2(origin.x + cx * cell, origin.y + cy * cell), egui::vec2(cell - 4.0, cell - 4.0));
            self.paint_icon(p, r, a);
        }
        let y = rect.top() + ts * 0.1 + area + 14.0;
        let max = ((ts - 10.0) / 7.0) as usize;
        p.text(egui::pos2(rect.center().x, y), egui::Align2::CENTER_TOP, truncate(name, max), egui::FontId::proportional(13.0), txt);
        p.text(egui::pos2(rect.center().x, y + 17.0), egui::Align2::CENTER_TOP, format!("{} apps", members.len()), egui::FontId::proportional(11.0), weak);
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                acts.push((name.to_string(), Act::Group));
                ui.close_menu();
            }
            if ui.button("Rename group…").clicked() {
                acts.push((name.to_string(), Act::RenameGroup));
                ui.close_menu();
            }
            if ui.button("Ungroup (move apps out)").clicked() {
                acts.push((name.to_string(), Act::Ungroup));
                ui.close_menu();
            }
        });
        if resp.clicked() {
            acts.push((name.to_string(), Act::Group));
        }
    }

    /// The open group as a floating window (like a Start-menu folder): group title on top, tiles inside.
    fn group_window(&mut self, ctx: &egui::Context, acts: &mut Vec<(String, Act)>) {
        let Some(name) = self.open_group.clone() else { return };
        let mut apps: Vec<&AppEntry> = self.in_group(&name).collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        let items: Vec<TileItem> = apps.into_iter().map(TileItem::App).collect();
        let gap = 10.0;
        let screen = ctx.screen_rect();
        // max icons per row; never more columns than apps, so the window hugs its content
        let max_cols = if self.cfg.max_per_row == 0 { 3 } else { self.cfg.max_per_row as usize };
        let cols = max_cols.min(items.len()).max(1);
        // shrink the icons only if the chosen row would not fit on screen
        let avail = (screen.width() - 80.0 - gap * (cols as f32 - 1.0)) / cols as f32;
        let ts = self.cfg.group_tile_size.clamp(90.0, 200.0).min(avail).max(60.0);
        let width = (cols as f32 * ts + gap * (cols as f32 - 1.0)).max(190.0);
        let mut open = true;
        // a new id per opening: the window starts fitted to its icons, then stays freely resizable
        egui::Window::new(format!("📁 {name}"))
            .id(egui::Id::new(("group_popup", self.group_gen)))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(width + 24.0)
            .default_pos(screen.center() - egui::vec2(width / 2.0 + 14.0, 160.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} app(s)", items.len())).weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Ungroup").on_hover_text("Move these apps out of the group").clicked() {
                            acts.push((name.clone(), Act::Ungroup));
                        }
                        if ui.button("Rename").on_hover_text("Rename this group").clicked() {
                            acts.push((name.clone(), Act::RenameGroup));
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::both().max_height(screen.height() * 0.7).auto_shrink([true, true]).show(ui, |ui| {
                    self.tile_grid(ui, ts, cols, &items, acts);
                });
            });
        if !open {
            self.open_group = None;
        }
    }

    fn tile(&self, ui: &mut egui::Ui, e: &AppEntry, ts: f32, acts: &mut Vec<(String, Act)>) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ts, ts * 1.2), egui::Sense::click());
        let busy = self.busy.get(&e.id);
        let has_update = self.update_available(e);
        let installed = e.version.is_some();
        let err = self.errors.get(&e.id);
        let vis = ui.visuals();
        let fill = if resp.hovered() { vis.widgets.hovered.bg_fill } else { vis.widgets.inactive.weak_bg_fill };
        let stroke = if resp.hovered() { vis.widgets.hovered.bg_stroke } else { vis.widgets.noninteractive.bg_stroke };
        let (txt, weak) = (vis.text_color(), vis.weak_text_color());

        let p = ui.painter();
        p.rect_filled(rect, 8.0, fill);
        p.rect_stroke(rect, 8.0, stroke);

        let isz = ts * 0.5;
        let irect = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + ts * 0.12 + isz / 2.0), egui::vec2(isz, isz));
        self.paint_icon(p, irect, e);

        let max = ((ts - 10.0) / 7.0) as usize;
        let y = irect.bottom() + 8.0;
        p.text(egui::pos2(rect.center().x, y), egui::Align2::CENTER_TOP, truncate(&e.name, max), egui::FontId::proportional(13.0), txt);
        let status = match (busy, &e.version) {
            (Some((t, _)), _) => t.clone(),
            (None, Some(v)) => v.clone(),
            (None, None) => "Not installed".into(),
        };
        p.text(egui::pos2(rect.center().x, y + 17.0), egui::Align2::CENTER_TOP, truncate(&status, max), egui::FontId::proportional(11.0), weak);

        if has_update {
            p.circle_filled(rect.right_top() + egui::vec2(-12.0, 12.0), 5.0, Color32::LIGHT_GREEN);
        } else if !installed && e.managed {
            p.circle_filled(rect.right_top() + egui::vec2(-12.0, 12.0), 5.0, Color32::YELLOW);
        }
        if err.is_some() {
            p.text(rect.left_top() + egui::vec2(8.0, 6.0), egui::Align2::LEFT_TOP, "⚠", egui::FontId::proportional(13.0), Color32::LIGHT_RED);
        }
        if let Some((_, frac)) = busy {
            let bar = egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.bottom() - 10.0), egui::vec2(rect.width() - 16.0, 4.0));
            p.rect_filled(bar, 2.0, Color32::from_white_alpha(30));
            let f = frac.unwrap_or(0.15).clamp(0.0, 1.0);
            p.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * f, 4.0)), 2.0, Color32::LIGHT_GREEN);
        }

        if resp.clicked() && busy.is_none() {
            let manual = matches!(e.source, Source::Manual);
            if e.exe.is_some() {
                acts.push((e.id.clone(), Act::Launch));
            } else if !installed && e.managed {
                acts.push((e.id.clone(), if manual { Act::Import } else { Act::Update }));
            } else if e.managed {
                acts.push((e.id.clone(), Act::Detect));
            }
        }
        resp.context_menu(|ui| self.menu_items(ui, e, acts, true));

        let mut tip = format!("{}\n{}", e.name, e.describe());
        if has_update {
            if let Some(r) = self.latest.get(&e.id) {
                tip.push_str(&format!("\nUpdate available: {}", r.version));
            }
        }
        if let Some(er) = err {
            tip.push_str(&format!("\n⚠ {er}"));
        }
        tip.push_str("\nClick: launch · Right-click: more");
        resp.on_hover_text(tip);
    }

    // ---- list-view card

    fn card(&self, ui: &mut egui::Ui, e: &AppEntry, acts: &mut Vec<(String, Act)>) {
        let busy = self.busy.get(&e.id);
        let has_update = self.update_available(e);
        let installed = e.version.is_some();
        let id = e.id.clone();

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                self.icon(ui, e, 32.0);
                ui.label(RichText::new(&e.name).heading());
                ui.label(RichText::new(e.kind_label()).small().weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("⋮", |ui| {
                        self.menu_items(ui, e, acts, false);
                    });
                    if has_update || (!installed && e.managed) {
                        let manual = matches!(e.source, Source::Manual);
                        let label = if installed { "⬆ Update" } else if manual { "📂 Pick file" } else { "⬇ Install" };
                        let b = egui::Button::new(RichText::new(label).strong()).fill(Color32::from_rgb(30, 100, 60));
                        if ui.add_enabled(busy.is_none(), b).clicked() {
                            acts.push((id.clone(), if manual { Act::Import } else { Act::Update }));
                        }
                    }
                    if installed && e.exe.is_none() && e.managed && busy.is_none() {
                        let b = ui.button("🔍 Detect exe").on_hover_text("Find the executable inside this app's folder");
                        if b.clicked() {
                            acts.push((id.clone(), Act::Detect));
                        }
                    }
                    let can_launch = busy.is_none() && e.exe.is_some();
                    if ui.add_enabled(can_launch, egui::Button::new(RichText::new("▶ Launch").strong())).clicked() {
                        acts.push((id.clone(), Act::Launch));
                    }
                });
            });

            ui.horizontal_wrapped(|ui| {
                match &e.version {
                    Some(v) => ui.label(format!("Installed: {v}")),
                    None => ui.label(RichText::new("Not installed").color(Color32::YELLOW)),
                };
                if let Some(r) = self.latest.get(&e.id) {
                    if has_update {
                        ui.label(RichText::new(format!("→ new: {}", r.version)).color(Color32::LIGHT_GREEN));
                    } else if installed {
                        ui.label(RichText::new("✔ up to date").weak());
                    }
                }
            });
            ui.label(RichText::new(e.describe()).small().weak());

            if let Some((text, frac)) = busy {
                ui.horizontal(|ui| {
                    match frac {
                        Some(f) => {
                            ui.add(egui::ProgressBar::new(*f).show_percentage().desired_width(220.0));
                        }
                        None => {
                            ui.spinner();
                        }
                    }
                    ui.label(text);
                });
            }
            if let Some(err) = self.errors.get(&e.id) {
                ui.label(RichText::new(format!("⚠ {err}")).color(Color32::LIGHT_RED));
            }
        });
        ui.add_space(4.0);
    }

    fn form_window(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.form.take() else { return };
        let groups = self.all_groups();
        let (mut open, mut submit, mut cancel) = (true, false, false);
        let title = if f.editing.is_some() { "Edit app" } else { "Add app" };

        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if f.kind != Kind::Manual {
                    ui.label("Paste a GitHub repo / release link, a direct download link, or a download page:");
                    ui.add(
                        egui::TextEdit::singleline(&mut f.input)
                            .hint_text("owner/repo  ·  https://github.com/o/r/releases/download/v1.2.3/app-1.2.3-win64.zip  ·  https://site/app.zip")
                            .desired_width(f32::INFINITY),
                    );
                    f.on_input_changed();
                }

                ui.horizontal(|ui| {
                    ui.label("Type:");
                    let manual_label = if f.editing.is_some() && f.manual_mode == ManualMode::Link { "Link" } else { "Manual" };
                    for (k, l) in [(Kind::GitHub, "GitHub release"), (Kind::Direct, "Direct link"), (Kind::Site, "Download page"), (Kind::Manual, manual_label)] {
                        if ui.selectable_value(&mut f.kind, k, l).clicked() {
                            f.locked = true;
                        }
                    }
                });

                egui::Grid::new("form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY));
                    ui.end_row();

                    ui.label("Group");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.group)
                                .hint_text("optional – one level only, e.g. Games")
                                .desired_width(280.0),
                        );
                        if !groups.is_empty() {
                            egui::ComboBox::from_id_salt("grp_combo").selected_text("existing…").show_ui(ui, |ui| {
                                for g in &groups {
                                    if ui.selectable_label(f.group == *g, g).clicked() {
                                        f.group = g.clone();
                                    }
                                }
                            });
                        }
                    });
                    ui.end_row();

                    match f.kind {
                        Kind::GitHub => {
                            ui.label("Package name\n(no version)");
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut f.pattern)
                                        .hint_text("e.g. app-win64.zip  or  app-*-win64.zip  (empty = auto-pick for this OS)")
                                        .desired_width(f32::INFINITY),
                                );
                                ui.checkbox(&mut f.prerelease, "Include pre-releases");
                            });
                            ui.end_row();
                        }
                        Kind::Direct => {}
                        Kind::Site => {
                            ui.label("File filter");
                            ui.add(egui::TextEdit::singleline(&mut f.pattern).hint_text("name without version, supports *  (empty = any)").desired_width(f32::INFINITY));
                            ui.end_row();
                            ui.label("Link regex");
                            ui.add(egui::TextEdit::singleline(&mut f.link_regex).hint_text("optional – capture group 1 = URL (default: any .zip/.exe/... href)").desired_width(f32::INFINITY));
                            ui.end_row();
                            ui.label("Version regex");
                            ui.add(egui::TextEdit::singleline(&mut f.version_regex).hint_text(r"optional – e.g. app-(\d+\.\d+\.\d+)").desired_width(f32::INFINITY));
                            ui.end_row();
                        }
                        Kind::Manual => {
                            if f.editing.is_none() {
                                ui.label("Mode");
                                ui.vertical(|ui| {
                                    ui.radio_value(&mut f.manual_mode, ManualMode::Import, "Import a file I downloaded (zip / 7z / tar.gz / exe)");
                                    ui.radio_value(&mut f.manual_mode, ManualMode::Link, "Link an existing program (stays where it is)");
                                    ui.radio_value(&mut f.manual_mode, ManualMode::Folder, "Just create an empty folder (I'll install the app into it)");
                                });
                                ui.end_row();
                                if f.manual_mode == ManualMode::Folder {
                                    ui.label("");
                                    ui.label(RichText::new("An empty folder is created inside pam-data/apps and opened. Install or copy the portable app into it, then use ⋮ → Detect executable (or Edit) to select the exe.").weak());
                                    ui.end_row();
                                } else {
                                ui.label("File");
                                ui.horizontal(|ui| {
                                    if ui.button("Browse…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                                            if f.name.is_empty() || f.name == f.auto_name {
                                                let n = util::strip_version(&util::strip_ext(&p.file_name().unwrap_or_default().to_string_lossy()));
                                                f.name = n.trim_matches(|c: char| "-_. ".contains(c)).to_string();
                                                f.auto_name = f.name.clone();
                                            }
                                            f.manual_file = Some(p);
                                        }
                                    }
                                    ui.label(f.manual_file.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "nothing selected".into()));
                                });
                                ui.end_row();
                                }
                            }
                        }
                    }

                    ui.label("Icon");
                    ui.horizontal(|ui| {
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = rfd::FileDialog::new()
                                .set_title("Pick an icon (png / jpg / bmp / ico, or an exe / dll to take its icon)")
                                .add_filter("Images and programs", &["png", "jpg", "jpeg", "bmp", "ico", "exe", "dll"])
                                .pick_file()
                            {
                                f.icon_src = Some(p);
                                f.icon_clear = false;
                            }
                        }
                        if (f.had_icon || f.icon_src.is_some()) && !f.icon_clear && ui.button("Automatic").on_hover_text("Use the program's own icon").clicked() {
                            f.icon_src = None;
                            f.icon_clear = f.had_icon;
                        }
                        let txt = match (&f.icon_src, f.icon_clear, f.had_icon) {
                            (Some(p), _, _) => p.display().to_string(),
                            (None, true, _) => "automatic (on save)".to_string(),
                            (None, false, true) => "custom icon".to_string(),
                            _ => "automatic".to_string(),
                        };
                        ui.label(RichText::new(txt).weak());
                    });
                    ui.end_row();

                    ui.label("Launch args");
                    ui.add(egui::TextEdit::singleline(&mut f.args).hint_text("optional").desired_width(f32::INFINITY));
                    ui.end_row();

                    if f.editing.is_some() {
                        ui.label("Executable");
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("exe_combo")
                                .selected_text(if f.exe.is_empty() { "(auto)".to_string() } else { f.exe.clone() })
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for x in f.exes.clone() {
                                        ui.selectable_value(&mut f.exe, x.clone(), x);
                                    }
                                });
                            if ui.button("Browse…").clicked() {
                                if let Some(p) = rfd::FileDialog::new().pick_file() {
                                    f.exe = p.to_string_lossy().to_string();
                                }
                            }
                        });
                        ui.end_row();
                    }
                });

                if !f.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &f.error);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let label = if f.editing.is_some() {
                        "Save"
                    } else if f.kind == Kind::Manual && f.manual_mode == ManualMode::Folder {
                        "Create folder"
                    } else {
                        "Add & install"
                    };
                    if ui.button(RichText::new(label).strong()).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if !open || cancel {
            return;
        }
        if submit {
            match self.submit_form(ctx, &f) {
                Ok(()) => return,
                Err(e) => f.error = e,
            }
        }
        self.form = Some(f);
    }

    fn remove_window(&mut self, ctx: &egui::Context) {
        let Some((id, mut delete)) = self.remove.clone() else { return };
        let Some(e) = self.entry(&id).cloned() else {
            self.remove = None;
            return;
        };
        let (mut yes, mut no) = (false, false);
        egui::Window::new("Remove app")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Remove “{}” from the manager?", e.name));
                if e.managed {
                    ui.checkbox(&mut delete, "Also delete its files");
                    if delete {
                        ui.colored_label(
                            Color32::LIGHT_RED,
                            format!("⚠ This permanently deletes everything in:\n{}", e.dir(&self.paths).display()),
                        );
                    } else {
                        ui.label(RichText::new("Files stay on disk.").weak());
                    }
                } else {
                    ui.label(RichText::new("It is a linked program – its files are never deleted.").weak());
                }
                ui.horizontal(|ui| {
                    let label = if e.managed && delete { "Remove and delete files" } else { "Remove" };
                    if ui.button(RichText::new(label).color(Color32::LIGHT_RED)).clicked() {
                        yes = true;
                    }
                    if ui.button("Cancel").clicked() {
                        no = true;
                    }
                });
            });
        if yes {
            if e.managed && delete {
                if let Err(err) = fs::remove_dir_all(e.dir(&self.paths)) {
                    if e.dir(&self.paths).exists() {
                        self.say(format!("Could not delete files (is it running?): {err}"));
                    }
                }
            }
            let _ = fs::remove_file(e.icon_path(&self.paths)); // our own copy of a custom icon
            self.cfg.apps.retain(|a| a.id != id);
            self.latest.remove(&id);
            self.errors.remove(&id);
            self.icons.remove(&id);
            self.save();
            self.remove = None;
        } else if no {
            self.remove = None;
        } else {
            self.remove = Some((id, delete));
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut changed = false;
        let (mut change_loc, mut reset_loc) = (false, false);
        egui::Window::new("Settings").open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.label("GitHub token (optional – raises the API rate limit):");
            changed |= ui.add(egui::TextEdit::singleline(&mut self.cfg.github_token).password(true).desired_width(300.0)).changed();
            changed |= ui.checkbox(&mut self.cfg.auto_check, "Check for updates on startup").changed();
            ui.separator();
            ui.strong("Tiles & groups");
            changed |= ui.add(egui::Slider::new(&mut self.cfg.default_tile_size, 90.0..=200.0).text("Default icon size (main window)")).changed();
            changed |= ui.add(egui::Slider::new(&mut self.cfg.group_tile_size, 90.0..=200.0).text("Icon size (group window)")).changed();
            ui.horizontal(|ui| {
                ui.label("Max icons per row");
                changed |= ui.add(egui::DragValue::new(&mut self.cfg.max_per_row).range(1..=12)).changed();
                ui.label(RichText::new("group window; opens sized to fit, still resizable").weak());
            });
            if ui.button("Use the default icon size now").clicked() {
                self.cfg.tile_size = self.cfg.default_tile_size;
                changed = true;
            }
            ui.separator();
            ui.strong("Window");
            changed |= ui.checkbox(&mut self.cfg.remember_window, "Remember window size and icon size when closing").changed();
            let ws = match self.cfg.window_size {
                Some([w, h]) => format!("{w:.0} × {h:.0}{}", if self.cfg.window_maximized { " (maximized)" } else { "" }),
                None => "nothing saved yet".to_string(),
            };
            ui.label(RichText::new(format!("Saved: window {ws}, icon size {:.0}", self.cfg.tile_size)).weak());
            if ui.button("Forget saved window size").clicked() {
                self.cfg.window_size = None;
                self.cfg.window_maximized = false;
                changed = true;
            }
            ui.separator();
            ui.label(format!("Version: {VERSION}"));
            ui.strong("Data folder");
            ui.label(self.paths.root.display().to_string());
            if Paths::env_override().is_some() {
                ui.label(RichText::new("Set by the PAM_DATA environment variable - it can't be changed here.").weak());
            } else {
                ui.label(RichText::new("Pick a folder: the app keeps its data in a \"pam-data\" folder inside it.").weak());
                ui.horizontal(|ui| {
                    if ui.button("Change location…").clicked() {
                        change_loc = true;
                    }
                    if Paths::read_pointer().is_some() && ui.button("Reset to app folder").clicked() {
                        reset_loc = true;
                    }
                    if ui.button("Open").clicked() {
                        let _ = open::that(&self.paths.root);
                    }
                });
            }
        });
        if changed {
            self.save();
        }
        self.show_settings = open;
        if change_loc {
            if let Some(base) = rfd::FileDialog::new().set_title("Choose the folder that will contain pam-data").pick_folder() {
                self.begin_move(Some(base));
            }
        }
        if reset_loc {
            self.begin_move(None);
        }
    }

    /// Trust warning shown before an app list is imported.
    fn import_window(&mut self, ctx: &egui::Context) {
        if !self.import_confirm {
            return;
        }
        let (mut choose, mut cancel) = (false, false);
        egui::Window::new("Import app list")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(RichText::new("⚠ Only import lists from sources you trust.").strong().color(Color32::YELLOW));
                ui.add_space(4.0);
                ui.label("An imported list tells this app where to download programs from. When you install them, the files are downloaded from the internet, extracted and can then be launched on your PC. A list from an untrusted source can point to malicious downloads.");
                ui.add_space(4.0);
                ui.label(RichText::new("Entries are sanity-checked (unsafe folder names and paths are rejected, linked programs must be re-selected), but the downloaded files themselves are not verified.").weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("I trust it - choose file…").strong()).clicked() {
                        choose = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if choose {
            self.import_confirm = false;
            self.import_apps();
        } else if cancel {
            self.import_confirm = false;
        }
    }

    /// Confirmation (and progress) for moving the data folder.
    fn move_window(&mut self, ctx: &egui::Context) {
        if self.moving {
            egui::Window::new("Copying data…")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Copying your apps and settings to the new folder…");
                    });
                });
            return;
        }
        let Some(mut m) = self.data_move.take() else { return };
        let (mut go, mut cancel) = (false, false);
        egui::Window::new("Change data folder")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label("New data folder:");
                ui.label(RichText::new(m.root.display().to_string()).strong());
                ui.add_space(4.0);
                if m.can_copy {
                    ui.checkbox(&mut m.copy, "Copy my current apps and settings there");
                } else {
                    ui.label(RichText::new("A pam-data folder with settings already exists there - it will be used as it is (nothing is copied).").weak());
                }
                ui.label(RichText::new("The current data folder is not deleted.").weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Switch").strong()).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if go {
            if m.can_copy && m.copy {
                self.moving = true;
                let (tx, ctx2, from, to, base) = (self.tx.clone(), ctx.clone(), self.paths.root.clone(), m.root.clone(), m.base.clone());
                thread::spawn(move || {
                    let res = install::copy_dir_all(&from, &to);
                    let _ = tx.send(Msg::DataMoved { base, res });
                    ctx2.request_repaint();
                });
            } else {
                self.finish_switch(m.base);
            }
        } else if !cancel {
            self.data_move = Some(m);
        }
    }
}

impl eframe::App for Manager {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.cfg.remember_window {
            if let Some(sz) = self.last_win {
                self.cfg.window_size = Some(sz);
            }
            self.cfg.window_maximized = self.win_maxi;
            self.save();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.ensure_icons(ctx);
        // remember the (non-maximized) window size so it can be saved on exit
        ctx.input(|i| {
            let vp = i.viewport();
            self.win_maxi = vp.maximized.unwrap_or(false);
            if !self.win_maxi && vp.minimized != Some(true) {
                if let Some(r) = vp.inner_rect {
                    let sz = r.size();
                    if sz.x >= 300.0 && sz.y >= 200.0 {
                        self.last_win = Some([sz.x, sz.y]);
                    }
                }
            }
        });
        if let Some(g) = &self.open_group {
            if self.cfg.view != ViewMode::Tiles || !self.all_groups().contains(g) {
                self.open_group = None;
            }
        }

        let (mut add, mut check_all, mut update_all, mut export, mut import) = (false, false, false, false, false);
        let mut view_changed = false;
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            // row 1: title + actions (wraps when the window is narrow)
            ui.horizontal_wrapped(|ui| {
                ui.heading("Portable App Manager");
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small().weak())
                    .on_hover_text(format!("Version {VERSION}"));
                ui.separator();
                add = ui.button("➕ Add app").clicked();
                check_all = ui.button("🔄 Check all").clicked();
                update_all = ui.button("⬇ Install / update all").clicked();
                ui.menu_button("⇅ Backup", |ui| {
                    if ui.button("Export app list…").clicked() {
                        export = true;
                        ui.close_menu();
                    }
                    if ui.button("Import app list…").clicked() {
                        import = true;
                        ui.close_menu();
                    }
                });
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
            ui.add_space(2.0);
            // row 2: filter + view options
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 filter").desired_width(200.0));
                ui.separator();
                view_changed |= ui.selectable_value(&mut self.cfg.view, ViewMode::List, "☰ List").clicked();
                view_changed |= ui.selectable_value(&mut self.cfg.view, ViewMode::Tiles, "▦ Tiles").clicked();
                if self.cfg.view == ViewMode::Tiles {
                    ui.label("Size");
                    let r = ui.add(egui::Slider::new(&mut self.cfg.tile_size, 90.0..=200.0).show_value(false));
                    view_changed |= r.drag_stopped();
                }
            });
            ui.add_space(4.0);
        });

        if view_changed {
            self.save();
        }

        let mut acts: Vec<(String, Act)> = Vec::new();
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.cfg.apps.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("No apps yet");
                    ui.label("Click “Add app” and paste a GitHub repo, release link, download link or page.");
                });
                return;
            }
            let q = self.filter.to_lowercase();
            let ts = self.cfg.tile_size.clamp(90.0, 200.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if !q.is_empty() {
                    // searching: flat results, groups ignored
                    let mut apps: Vec<&AppEntry> = self.cfg.apps.iter().filter(|a| a.name.to_lowercase().contains(&q)).collect();
                    apps.sort_by_key(|a| a.name.to_lowercase());
                    if self.cfg.view == ViewMode::Tiles {
                        let items: Vec<TileItem> = apps.into_iter().map(TileItem::App).collect();
                        self.tile_grid(ui, ts, 0, &items, &mut acts);
                    } else {
                        for e in apps {
                            self.card(ui, e, &mut acts);
                        }
                    }
                } else if self.cfg.view == ViewMode::Tiles {
                    self.tiles_level(ui, ts, &mut acts);
                } else {
                    self.list_level(ui, &mut acts);
                }
            });
        });

        // bottom status line
        if let Some((msg, t)) = &self.toast {
            if t.elapsed() < Duration::from_secs(6) {
                egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                    ui.label(msg);
                });
                ctx.request_repaint_after(Duration::from_millis(500));
            } else {
                self.toast = None;
            }
        }

        if add {
            let mut f = Form::new();
            if self.cfg.view == ViewMode::Tiles {
                f.group = self.open_group.clone().unwrap_or_default();
            }
            self.form = Some(f);
        }
        if export {
            self.export_apps();
        }
        if import {
            self.import_confirm = true; // ask first, see import_window()
        }
        if check_all {
            self.check_all(ctx);
        }
        if update_all {
            let ids: Vec<String> = self
                .cfg
                .apps
                .iter()
                .filter(|a| {
                    self.update_available(a)
                        || (a.managed && a.version.is_none() && a.source.can_check() && !self.busy.contains_key(&a.id))
                })
                .map(|a| a.id.clone())
                .collect();
            for id in ids {
                let src = InstallSrc::Remote(self.latest.get(&id).cloned());
                self.start_install(ctx, &id, src);
            }
        }
        self.group_window(ctx, &mut acts);
        for (id, a) in acts {
            self.act(ctx, id, a);
        }
        self.rename_window(ctx);

        self.form_window(ctx);
        self.remove_window(ctx);
        self.settings_window(ctx);
        self.import_window(ctx);
        self.move_window(ctx);
    }
}

/// e.g. "0.1.0 (a1b2c3d, 2026-11-03)"  -  version comes from Cargo.toml, hash + date from build.rs
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PAM_GIT_HASH"), ", ", env!("PAM_BUILD_DATE"), ")");

/// Normalised 256x256 version of src/icon/icon.png, produced by build.rs. Replace the source file and rebuild to change the icon.
const ICON_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon256.png"));

fn window_icon() -> Option<egui::IconData> {
    let mut img = image::load_from_memory(ICON_PNG).ok()?;
    if img.width() > 256 || img.height() > 256 {
        img = img.resize(256, 256, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData { rgba: rgba.into_raw(), width, height })
}

fn main() -> eframe::Result<()> {
    let saved = Config::load(&Paths::detect());
    let size = saved
        .window_size
        .filter(|_| saved.remember_window)
        .map(|[w, h]| [w.clamp(620.0, 7680.0), h.clamp(380.0, 4320.0)])
        .unwrap_or([860.0, 580.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(size)
        .with_min_inner_size([620.0, 380.0])
        .with_title(format!("Portable App Manager v{}", env!("CARGO_PKG_VERSION")));
    if saved.remember_window && saved.window_maximized {
        viewport = viewport.with_maximized(true);
    }
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    // centered: open in the middle of the screen (not supported on Wayland)
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "Portable App Manager",
        options,
        Box::new(|cc| Ok(Box::new(Manager::new(cc)))),
    )
}
