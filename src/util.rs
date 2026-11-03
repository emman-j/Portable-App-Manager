//! Small helpers: version stripping, wildcard matching, platform scoring.

use regex::Regex;
use std::sync::OnceLock;

fn ver_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // "-1.2.3", "_v2.0", ".1.2.3-rc1" ...  (needs at least one dot so "x86_64" / "win64" survive)
        Regex::new(r"(?i)[-_ .]?v?\d+(?:\.\d+){1,3}(?:[-_.]?(?:rc|beta|alpha)\d*)?").unwrap()
    })
}

fn plain_ver_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"v?(\d+(?:\.\d+)+)").unwrap())
}

/// "ripgrep-14.1.0-x86_64-pc-windows-msvc.zip" -> "ripgrep-x86_64-pc-windows-msvc.zip"
pub fn strip_version(s: &str) -> String {
    ver_re().replace_all(s, "").to_lowercase()
}

pub fn find_version(s: &str) -> Option<String> {
    plain_ver_re().captures(s).map(|c| c[1].to_string())
}

pub fn version_key(s: &str) -> Vec<u64> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// Case-insensitive `*` / `?` wildcard match.
pub fn glob_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut i, mut j, mut mark) = (0usize, 0usize, 0usize);
    let mut star: Option<usize> = None;
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// Does `asset` match the user's "package name without version" pattern?
/// - with `*`/`?` -> wildcard match
/// - otherwise    -> compare with versions stripped from both sides
pub fn name_matches(pattern: &str, asset: &str) -> bool {
    let p = pattern.trim();
    if p.is_empty() {
        return true;
    }
    if p.contains('*') || p.contains('?') {
        return glob_match(p, asset);
    }
    let (sp, sa) = (strip_version(p), strip_version(asset));
    sa == sp || sa.contains(&sp) || asset.to_lowercase().contains(&p.to_lowercase())
}

/// Heuristic score of how well a release file fits this OS / CPU (higher = better).
pub fn platform_score(name: &str) -> i32 {
    let n = name.to_lowercase();
    let tokens: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).collect();
    let has = |t: &str| tokens.iter().any(|x| *x == t);
    let mut s = 0;

    let (is_win, is_mac, is_linux) = (
        n.contains("windows") || has("win") || has("win64") || has("win32") || n.contains("msvc"),
        n.contains("darwin") || n.contains("macos") || n.contains("osx") || has("mac") || n.ends_with(".dmg"),
        n.contains("linux") || n.ends_with(".appimage") || n.contains("gnu") || n.contains("musl"),
    );
    let (want_win, want_mac) = (cfg!(windows), cfg!(target_os = "macos"));
    let want_linux = !want_win && !want_mac;
    if (want_win && is_win) || (want_mac && is_mac) || (want_linux && is_linux) {
        s += 10;
    }
    if (is_win && !want_win) || (is_mac && !want_mac) || (is_linux && !want_linux) {
        s -= 20;
    }

    let is_x64 = n.contains("x86_64") || n.contains("amd64") || has("x64") || n.contains("win64");
    let is_arm = n.contains("aarch64") || n.contains("arm64") || n.contains("armv");
    let is_32 = n.contains("i686") || n.contains("i386") || has("x86") && !is_x64 || has("win32");
    if cfg!(target_arch = "x86_64") {
        if is_x64 { s += 5 }
        if is_arm || is_32 { s -= 10 }
    } else if cfg!(target_arch = "aarch64") {
        if is_arm { s += 5 }
        if is_x64 || is_32 { s -= 10 }
    }

    if has("portable") { s += 6 }
    if n.ends_with(".zip") || n.ends_with(".tar.gz") || n.ends_with(".tgz") { s += 2 }
    if n.ends_with(".appimage") && want_linux { s += 2 }
    for bad in [".sha256", ".sha512", ".sig", ".asc", ".txt", ".sbom", ".json", ".minisig", ".pem", ".msi", ".deb", ".rpm", ".dmg", ".pkg", ".apk", ".snap", ".flatpak"] {
        if n.ends_with(bad) { s -= 30 }
    }
    for bad in ["checksum", "sha256sum", "source", "symbols", "debug", "setup", "installer"] {
        if n.contains(bad) { s -= 8 }
    }
    s
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') { c } else { '_' })
        .collect();
    let s = s.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if s.is_empty() { "download.bin".into() } else { s }
}

pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "app".into() } else { s }
}

/// Split a command-line string into args, honouring "double quotes".
pub fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut in_q, mut any) = (Vec::new(), String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' => { in_q = !in_q; any = true }
            c if c.is_whitespace() && !in_q => {
                if any || !cur.is_empty() { out.push(std::mem::take(&mut cur)); any = false }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() { out.push(cur) }
    out
}

pub fn strip_ext(name: &str) -> String {
    let l = name.to_lowercase();
    for ext in [".tar.gz", ".tar.xz", ".tgz", ".zip", ".7z", ".exe", ".msi", ".appimage"] {
        if l.ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.to_string()
}

/// Groups are ONE level only. Separators are flattened:  "Games / Emulators" -> "Games - Emulators"
pub fn norm_group(s: &str) -> String {
    s.split(|c: char| c == '/' || c == '\\')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

/// App ids become folder names, so they must be plain: letters, digits, '-' and '_'.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
