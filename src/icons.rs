//! Getting an app icon: from the exe's own resources (PE), else from an icon file next to it.

use eframe::egui::ColorImage;
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX: u32 = 96;

pub fn load(exe: &Path, app_dir: &Path) -> Option<ColorImage> {
    let ext = exe.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "exe" || ext == "dll" {
        if let Some(img) = from_pe(exe).and_then(|ico| decode(&ico)) {
            return Some(img);
        }
    }
    let file = find_icon_file(exe, app_dir)?;
    decode(&fs::read(file).ok()?)
}

/// Pull the first icon group out of a Windows executable and return it as .ico bytes.
fn from_pe(path: &Path) -> Option<Vec<u8>> {
    let map = pelite::FileMap::open(path).ok()?;
    if let Ok(f) = pelite::pe64::PeFile::from_bytes(&map) {
        use pelite::pe64::Pe;
        return first_icon(f.resources().ok()?);
    }
    if let Ok(f) = pelite::pe32::PeFile::from_bytes(&map) {
        use pelite::pe32::Pe;
        return first_icon(f.resources().ok()?);
    }
    None
}

fn first_icon(res: pelite::resources::Resources<'_>) -> Option<Vec<u8>> {
    for (_name, group) in res.icons().filter_map(Result::ok) {
        let mut buf = Vec::new();
        if group.write(&mut buf).is_ok() && !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let mut img = image::load_from_memory(bytes).ok()?;
    if img.width() > MAX || img.height() > MAX {
        img = img.resize(MAX, MAX, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw()))
}

/// png/ico next to the exe (or in the app folder) whose name looks like an app icon.
fn find_icon_file(exe: &Path, app_dir: &Path) -> Option<PathBuf> {
    let stem = exe.file_stem()?.to_string_lossy().to_lowercase();
    let mut best: Option<(i32, PathBuf)> = None;
    let mut dirs = vec![app_dir.to_path_buf()];
    if let Some(p) = exe.parent() {
        if p != app_dir {
            dirs.push(p.to_path_buf());
        }
    }
    for d in dirs {
        scan(&d, 0, &stem, &mut best);
    }
    best.map(|(_, p)| p)
}

fn scan(dir: &Path, depth: usize, stem: &str, best: &mut Option<(i32, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 2 {
                scan(&p, depth + 1, stem, best);
            }
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        if !matches!(ext.as_str(), "png" | "ico" | "bmp") && name != ".diricon" {
            continue;
        }
        if fs::metadata(&p).map(|m| m.len() > 4_000_000).unwrap_or(true) {
            continue;
        }
        let mut score = 0;
        if name == ".diricon" { score += 4 }
        if name.contains(stem) { score += 5 }
        if name.contains("icon") { score += 3 }
        if name.contains("logo") { score += 1 }
        if ext == "ico" { score += 1 }
        if score > 0 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            *best = Some((score, p));
        }
    }
}
