# Sources and matching

## Update detection per source
| Source | Latest version is… | Compared with |
|---|---|---|
| GitHub release | the release tag (`tag_name`) | the tag stored at install |
| Direct link | `etag:<value>`, else the `Last-Modified` header, else `size:<bytes>` | the value stored at install |
| Download page | the version regex match; else a version found in the file name; else the file name | the value stored at install |
| Manual | none | no update checks |

An update is shown when the latest value differs from the stored one. If a direct link yields no usable header the version is `?` and is never flagged as an update.

GitHub: **Include pre-releases** switches from the latest stable release to the newest release of any kind.

## Package name matching (GitHub and download pages)
The package name is the file name **without its version**. Both sides have versions stripped before they are compared, so one name keeps working across releases.

```
ripgrep-14.1.0-x86_64-pc-windows-msvc.zip   ->   ripgrep-x86_64-pc-windows-msvc.zip
```
- Pasting a release download link fills the package name for you.
- A name with `*` or `?` is a wildcard match: `app-*-win64.zip`.
- A name without wildcards matches when the version-stripped file name equals or contains it.
- Empty means every file is a candidate.

Version stripping removes patterns like `-1.2.3`, `_v2.0`, `.1.2.3-rc1`. It needs at least one dot, so parts like `x86_64` and `win64` are kept.

## Choosing between several matches
When more than one file matches, the best score wins; ties prefer the shorter name (GitHub) or the higher version (download pages).

| Adds points | Removes points |
|---|---|
| Matches your OS | Other OS names (Linux, macOS, …) |
| Matches your CPU (x64 / arm64) | Wrong CPU, 32-bit builds |
| `portable` in the name | Checksums, signatures, source, symbols, debug builds |
| `.zip`, `.tar.gz`, AppImage on Linux | Installers: `.msi`, `.deb`, `.rpm`, `.dmg`, `.pkg`, `setup`, `installer` |

Files scoring very low are ignored. If nothing matches, the error lists the available files.

## Download pages
The page HTML is scanned for links. By default any `href` ending in `.zip .7z .exe .msi .tar.gz .tgz .appimage` is a candidate.
- **Link regex** (optional): your own pattern. Capture group 1 is the URL (or the whole match if there is no group).
- **File filter**: a package name as above.
- **Version regex** (optional): capture group 1 is the version. Tried on the link, then the file name.

Relative links are resolved against the page URL.

## What happens to the downloaded file
| File | Result |
|---|---|
| `.zip`, `.7z`, `.tar.gz`, `.tgz` | Extracted into the app folder; a single top-level folder is flattened |
| anything else (exe, AppImage, …) | Copied into the app folder as-is |

After installing, the executable is kept if it still exists. Otherwise one is detected: a file whose name matches the app name, else the shallowest one. Uninstallers, updaters and similar helper executables are ignored.
