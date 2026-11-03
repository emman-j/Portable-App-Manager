//! Reading the version of an installed program from its executable (Windows version resource).

use std::path::Path;

/// Version stored in a Windows executable's version resource, e.g. "3.1.2" (trailing ".0" parts dropped).
/// `None` for non-PE files, files without version info, or an all-zero version.
pub fn exe_version(path: &Path) -> Option<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext != "exe" && ext != "dll" {
        return None;
    }
    let map = pelite::FileMap::open(path).ok()?;
    if let Ok(f) = pelite::pe64::PeFile::from_bytes(&map) {
        use pelite::pe64::Pe;
        return version_of(f.resources().ok()?);
    }
    if let Ok(f) = pelite::pe32::PeFile::from_bytes(&map) {
        use pelite::pe32::Pe;
        return version_of(f.resources().ok()?);
    }
    None
}

fn version_of(res: pelite::resources::Resources<'_>) -> Option<String> {
    let vi = res.version_info().ok()?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(fixed) = vi.fixed() {
        let v = fixed.dwFileVersion;
        parts = vec![v.Major.to_string(), v.Minor.to_string(), v.Patch.to_string(), v.Build.to_string()];
    }
    if parts.iter().all(|p| p == "0") {
        // no usable fixed version: try the "ProductVersion" / "FileVersion" strings
        let lang = vi.translation().first().copied()?;
        let s = vi.value(lang, "ProductVersion").or_else(|| vi.value(lang, "FileVersion"))?;
        let s = s.trim().replace(", ", ".");
        parts = s.split('.').map(|p| p.trim().to_string()).collect();
        if s.is_empty() || parts.iter().any(|p| p.is_empty()) {
            return None;
        }
    }
    while parts.len() > 2 && parts.last().map(|p| p == "0").unwrap_or(false) {
        parts.pop();
    }
    let v = parts.join(".");
    (!v.is_empty() && v != "0" && v != "0.0").then_some(v)
}
