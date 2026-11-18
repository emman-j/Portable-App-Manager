use crate::util;
use std::{
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
    process::Command,
};
pub fn install_file(file: &Path, filename: &str, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let l = filename.to_lowercase();
    if l.ends_with(".zip") {
        extract_zip(file, dest)
    } else if l.ends_with(".tar.gz") || l.ends_with(".tgz") {
        extract_targz(file, dest)
    } else if l.ends_with(".7z") {
        extract_7z(file, dest)
    } else {
        let target = dest.join(util::sanitize(filename));
        fs::copy(file, &target).map_err(|e| format!("Copy failed: {e}"))?;
        set_exec(&target);
        Ok(())
    }
}
#[cfg(unix)]
fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = fs::metadata(p) {
        let mut perm = m.permissions();
        perm.set_mode(perm.mode() | 0o755);
        let _ = fs::set_permissions(p, perm);
    }
}
#[cfg(not(unix))]
fn set_exec(_: &Path) {}
fn common_root(paths: &[PathBuf]) -> Option<PathBuf> {
    if paths.len() < 2 {
        return None;
    }
    let first = paths[0].components().next()?;
    let mut nested = false;
    for p in paths {
        let mut c = p.components();
        if c.next()? != first {
            return None;
        }
        if c.next().is_some() {
            nested = true;
        }
    }
    nested.then(|| PathBuf::from(first.as_os_str()))
}
fn rel_path(p: &Path, root: Option<&Path>) -> Option<PathBuf> {
    let p = match root {
        Some(r) => p.strip_prefix(r).ok()?,
        None => p,
    };
    if p.as_os_str().is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(p.to_path_buf())
}
fn extract_zip(file: &Path, dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(File::open(file).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Not a valid zip: {e}"))?;
    let names: Vec<PathBuf> = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().and_then(|e| e.enclosed_name()))
        .collect();
    let root = common_root(&names);
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(p) = e.enclosed_name() else { continue };
        let Some(rel) = rel_path(&p, root.as_deref()) else { continue };
        let out = dest.join(rel);
        if e.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        io::copy(&mut e, &mut f).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = e.unix_mode() {
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777));
            }
        }
    }
    Ok(())
}
fn extract_targz(file: &Path, dest: &Path) -> Result<(), String> {
    let open = || -> Result<_, String> {
        let f = File::open(file).map_err(|e| e.to_string())?;
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(f)))
    };
    let mut names = Vec::new();
    for e in open()?.entries().map_err(|e| e.to_string())? {
        names.push(e.map_err(|e| e.to_string())?.path().map_err(|e| e.to_string())?.into_owned());
    }
    let root = common_root(&names);
    for e in open()?.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        let p = e.path().map_err(|e| e.to_string())?.into_owned();
        let Some(rel) = rel_path(&p, root.as_deref()) else { continue };
        let out = dest.join(rel);
        let t = e.header().entry_type();
        if t.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else if t.is_file() {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            e.unpack(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        }
    }
    Ok(())
}
fn extract_7z(file: &Path, dest: &Path) -> Result<(), String> {
    {
        let rd = sevenz_rust::SevenZReader::open(file, sevenz_rust::Password::empty())
            .map_err(|e| format!("7z open failed: {e}"))?;
        for entry in &rd.archive().files {
            if rel_path(Path::new(entry.name()), None).is_none() {
                return Err(format!("Refusing to extract: unsafe path in archive: {}", entry.name()));
            }
        }
    }
    let scratch = dest.join(".pam-7z-extract");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        sevenz_rust::decompress_file(file, &scratch).map_err(|e| format!("7z extract failed: {e}"))?;
        let entries: Vec<PathBuf> = fs::read_dir(&scratch)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .collect();
        let single_dir = entries.len() == 1 && fs::symlink_metadata(&entries[0]).map(|m| m.is_dir()).unwrap_or(false);
        let src = if single_dir { entries[0].clone() } else { scratch.clone() };
        merge_move(&src, dest)
    })();
    let _ = fs::remove_dir_all(&scratch);
    result
}
fn merge_move(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let src = e.path();
        let dst = to.join(e.file_name());
        if ft.is_dir() {
            merge_move(&src, &dst)?;
        } else {
            if fs::symlink_metadata(&dst).is_ok() {
                fs::remove_file(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            }
            if fs::rename(&src, &dst).is_err() {
                fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            }
        }
    }
    Ok(())
}
pub fn copy_dir_all(from: &Path, to: &Path) -> Result<(), String> {
    fn inner(from: &Path, to: &Path, top: bool) -> Result<(), String> {
        fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
        for e in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let name = e.file_name();
            if top && name.to_string_lossy().eq_ignore_ascii_case("tmp") {
                continue;
            }
            let (s, d) = (e.path(), to.join(&name));
            if ft.is_dir() {
                inner(&s, &d, false)?;
            } else {
                fs::copy(&s, &d).map_err(|e| format!("{}: {e}", s.display()))?;
            }
        }
        Ok(())
    }
    inner(from, to, true)
}
fn is_exe(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ["unins", "uninstall", "crashpad", "vc_redist", "vcredist", "updater", "update."]
        .iter()
        .any(|b| name.contains(b))
    {
        return false;
    }
    exec_kind(&name, path)
}
#[cfg(windows)]
fn exec_kind(name: &str, _path: &Path) -> bool {
    name.ends_with(".exe") || name.ends_with(".bat") || name.ends_with(".cmd")
}
#[cfg(not(windows))]
fn exec_kind(name: &str, path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if name.ends_with(".appimage") {
        return true;
    }
    if name.ends_with(".so") || name.contains(".so.") || name.ends_with(".dll") || name.ends_with(".dylib") {
        return false;
    }
    path.is_file() && fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
pub fn find_executables(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() {
                if cfg!(target_os = "macos") && name.ends_with(".app") {
                    out.push(rel(base, &p));
                } else if depth < 4 {
                    walk(base, &p, depth + 1, out);
                }
            } else if is_exe(&p) {
                out.push(rel(base, &p));
            }
        }
    }
    fn rel(base: &Path, p: &Path) -> String {
        p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out.sort_by_key(|p| (p.matches('/').count(), p.to_lowercase()));
    out
}
pub fn pick_default(app_name: &str, list: &[String]) -> Option<String> {
    let key = util::slug(app_name).replace('-', "");
    list.iter()
        .find(|p| {
            let f = p.rsplit('/').next().unwrap_or(p).to_lowercase().replace(['-', '_', ' '], "");
            !key.is_empty() && f.contains(&key)
        })
        .or_else(|| list.first())
        .cloned()
}
// ---------------------------------------------------------------- launching
/// Start a program. `admin` = ask Windows for elevation (UAC prompt) right away. Without it, a program that
/// needs administrator rights (Windows error 740) is retried with the elevation prompt automatically.
pub fn launch(exe: &Path, args: &str, admin: bool) -> Result<(), String> {
    if !exe.exists() {
        return Err(format!("Not found: {}", exe.display()));
    }
    let args = util::split_args(args);
    if admin {
        return run_elevated(exe, &args);
    }
    let mut cmd;
    if cfg!(target_os = "macos") && exe.extension().map(|e| e == "app").unwrap_or(false) {
        cmd = Command::new("open");
        cmd.arg("-a").arg(exe);
        if !args.is_empty() {
            cmd.arg("--args").args(&args);
        }
    } else {
        cmd = Command::new(exe);
        cmd.args(&args);
        if let Some(parent) = exe.parent() {
            cmd.current_dir(parent);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        // ERROR_ELEVATION_REQUIRED: the program wants administrator rights
        Err(e) if cfg!(windows) && e.raw_os_error() == Some(740) => return run_elevated(exe, &args),
        Err(e) => return Err(format!("Launch failed: {e}")),
    };
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Run `exe` as administrator: PowerShell's `Start-Process -Verb RunAs` shows the normal Windows UAC prompt.
#[cfg(windows)]
fn run_elevated(exe: &Path, args: &[String]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // single-quoted PowerShell string: only ' needs doubling
    fn q(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }
    let mut script = format!("Start-Process -FilePath {} -Verb RunAs", q(&exe.to_string_lossy()));
    if let Some(dir) = exe.parent() {
        script.push_str(&format!(" -WorkingDirectory {}", q(&dir.to_string_lossy())));
    }
    if !args.is_empty() {
        let list: Vec<String> = args
            .iter()
            .map(|a| if a.contains(char::is_whitespace) { q(&format!("\"{a}\"")) } else { q(a) })
            .collect();
        script.push_str(&format!(" -ArgumentList {}", list.join(",")));
    }
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("Could not start PowerShell: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.to_lowercase().contains("canceled") || err.to_lowercase().contains("cancelled") {
        Err("Cancelled: the administrator prompt was declined".into())
    } else {
        Err(format!("Run as administrator failed: {}", err.lines().next().unwrap_or("unknown error")))
    }
}

#[cfg(not(windows))]
fn run_elevated(_exe: &Path, _args: &[String]) -> Result<(), String> {
    Err("Run as administrator is only available on Windows".into())
}
