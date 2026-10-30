#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod install;
mod model;
mod net;
mod util;

use eframe::egui::{self, Color32, RichText};
use model::{AppEntry, Config, Paths, Source};
use std::{
    collections::HashMap,
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
    Installed { id: String, res: Result<Installed, String> },
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
            let dest = tmp.join(format!("{}-{}", entry.id, util::sanitize(&r.filename)));
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
    let repo = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
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
            error: String::new(),
        }
    }

    fn from_entry(e: &AppEntry, paths: &Paths) -> Self {
        let mut f = Form::new();
        f.editing = Some(e.id.clone());
        f.name = e.name.clone();
        f.locked = true;
        f.args = e.args.clone();
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
    Remove,
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
        };
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
        let ids: Vec<String> = self.cfg.apps.iter().filter(|a| a.source.can_check()).map(|a| a.id.clone()).collect();
        for id in ids {
            self.start_check(ctx, &id);
        }
    }

    fn start_check(&mut self, ctx: &egui::Context, id: &str) {
        let Some(e) = self.entry(id).cloned() else { return };
        if !e.source.can_check() || self.busy.contains_key(id) {
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

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress { id, text, frac } => {
                    if let Some(b) = self.busy.get_mut(&id) {
                        *b = (text, frac);
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
                if let Some(f) = rfd::FileDialog::new().set_title("Pick the downloaded file (zip / tar.gz / exe / AppImage)").pick_file() {
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
            Act::Remove => {
                let managed = self.entry(&id).map(|e| e.managed).unwrap_or(true);
                self.remove = Some((id, managed));
            }
        }
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
                e.exe = (!f.exe.trim().is_empty()).then(|| f.exe.trim().to_string());
            }
            self.latest.remove(id);
            self.save();
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
        };

        let mut to_install: Option<InstallSrc> = None;
        match (&source, f.manual_mode) {
            (Source::Manual, ManualMode::Link) => {
                let p = f.manual_file.as_ref().ok_or("Pick the executable to link")?;
                entry.managed = false;
                entry.exe = Some(p.to_string_lossy().to_string());
                entry.version = Some("external".into());
            }
            (Source::Manual, ManualMode::Import) => {
                let p = f.manual_file.clone().ok_or("Pick the file you downloaded")?;
                to_install = Some(InstallSrc::File(p));
            }
            _ => to_install = Some(InstallSrc::Remote(None)),
        }
        self.cfg.apps.push(entry);
        self.save();
        if let Some(src) = to_install {
            self.start_install(ctx, &id, src);
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

    fn import_apps(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import app list")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let res: Result<(usize, usize), String> = (|| {
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("Not valid JSON: {e}"))?;
            let list = v.get("apps").cloned().ok_or("This file has no \"apps\" list")?;
            let apps: Vec<AppEntry> = serde_json::from_value(list).map_err(|e| format!("Unreadable app list: {e}"))?;
            let (mut added, mut skipped) = (0, 0);
            for mut a in apps {
                let dup = self
                    .cfg
                    .apps
                    .iter()
                    .any(|x| x.id == a.id || (x.name.eq_ignore_ascii_case(&a.name) && x.source == a.source));
                if dup {
                    skipped += 1;
                    continue;
                }
                if a.managed {
                    a.version = None;
                }
                self.cfg.apps.push(a);
                added += 1;
            }
            Ok((added, skipped))
        })();
        match res {
            Ok((added, skipped)) => {
                self.save();
                self.say(format!(
                    "Imported {added} app(s), skipped {skipped} already present. Use “Install all” to download them."
                ));
            }
            Err(e) => self.say(format!("Import failed: {e}")),
        }
    }

    // ---- UI pieces

    fn card(&self, ui: &mut egui::Ui, e: &AppEntry, acts: &mut Vec<(String, Act)>) {
        let busy = self.busy.get(&e.id);
        let has_update = self.update_available(e);
        let installed = e.version.is_some();
        let id = e.id.clone();

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&e.name).heading());
                ui.label(RichText::new(e.source.label()).small().weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("⋮", |ui| {
                        if e.source.can_check() && ui.button("Check for update").clicked() {
                            acts.push((id.clone(), Act::Check));
                            ui.close_menu();
                        }
                        if e.managed && e.source.can_check() && ui.button("Reinstall latest").clicked() {
                            acts.push((id.clone(), Act::Reinstall));
                            ui.close_menu();
                        }
                        if e.managed && ui.button("Install from downloaded file…").clicked() {
                            acts.push((id.clone(), Act::Import));
                            ui.close_menu();
                        }
                        if e.managed && ui.button("Open folder").clicked() {
                            acts.push((id.clone(), Act::OpenDir));
                            ui.close_menu();
                        }
                        if ui.button("Edit…").clicked() {
                            acts.push((id.clone(), Act::Edit));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(RichText::new("Remove…").color(Color32::LIGHT_RED)).clicked() {
                            acts.push((id.clone(), Act::Remove));
                            ui.close_menu();
                        }
                    });
                    if has_update || (!installed && e.managed) {
                        let manual = matches!(e.source, Source::Manual);
                        let label = if installed { "⬆ Update" } else if manual { "📂 Pick file" } else { "⬇ Install" };
                        let b = egui::Button::new(RichText::new(label).strong()).fill(Color32::from_rgb(30, 100, 60));
                        if ui.add_enabled(busy.is_none(), b).clicked() {
                            acts.push((id.clone(), if manual { Act::Import } else { Act::Update }));
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
            ui.label(RichText::new(e.source.describe()).small().weak());

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
                    for (k, l) in [(Kind::GitHub, "GitHub release"), (Kind::Direct, "Direct link"), (Kind::Site, "Download page"), (Kind::Manual, "Manual")] {
                        if ui.selectable_value(&mut f.kind, k, l).clicked() {
                            f.locked = true;
                        }
                    }
                });

                egui::Grid::new("form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY));
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
                                    ui.radio_value(&mut f.manual_mode, ManualMode::Import, "Import a file I downloaded (zip / tar.gz / exe)");
                                    ui.radio_value(&mut f.manual_mode, ManualMode::Link, "Link an existing program (stays where it is)");
                                });
                                ui.end_row();
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
                    let label = if f.editing.is_some() { "Save" } else { "Add & install" };
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
                } else {
                    ui.label(RichText::new("It is a linked program – its files are never deleted.").weak());
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Remove").color(Color32::LIGHT_RED)).clicked() {
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
            self.cfg.apps.retain(|a| a.id != id);
            self.latest.remove(&id);
            self.errors.remove(&id);
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
        egui::Window::new("Settings").open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.label("GitHub token (optional – raises the API rate limit):");
            changed |= ui.add(egui::TextEdit::singleline(&mut self.cfg.github_token).password(true).desired_width(300.0)).changed();
            changed |= ui.checkbox(&mut self.cfg.auto_check, "Check for updates on startup").changed();
            ui.separator();
            ui.label(format!("Data folder: {}", self.paths.root.display()));
            if ui.button("Open data folder").clicked() {
                let _ = open::that(&self.paths.root);
            }
        });
        if changed {
            self.save();
        }
        self.show_settings = open;
    }
}

impl eframe::App for Manager {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();

        let (mut add, mut check_all, mut update_all, mut export, mut import) = (false, false, false, false, false);
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Portable App Manager");
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 filter").desired_width(160.0));
                });
            });
            ui.add_space(4.0);
        });

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
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut apps: Vec<&AppEntry> = self.cfg.apps.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q)).collect();
                apps.sort_by_key(|a| a.name.to_lowercase());
                for e in apps {
                    self.card(ui, e, &mut acts);
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
            self.form = Some(Form::new());
        }
        if export {
            self.export_apps();
        }
        if import {
            self.import_apps();
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
        for (id, a) in acts {
            self.act(ctx, id, a);
        }

        self.form_window(ctx);
        self.remove_window(ctx);
        self.settings_window(ctx);
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([860.0, 580.0])
            .with_min_inner_size([620.0, 380.0])
            .with_title("Portable App Manager"),
        ..Default::default()
    };
    eframe::run_native(
        "Portable App Manager",
        options,
        Box::new(|cc| Ok(Box::new(Manager::new(cc)))),
    )
}
