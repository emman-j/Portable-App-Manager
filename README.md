# Portable App Manager (Rust + egui)

```
cargo run --release
```
Linux build deps for the file dialog / window: `libxkbcommon-dev libgtk-3-dev` (or rely on xdg-portal).

Everything is stored next to the executable in `pam-data/` (override with env `PAM_DATA`):
`config.json`, `apps/<id>/` (each app's files), `tmp/`.

## Adding apps (paste into the single box – the type is auto-detected)
| You paste | Detected as |
|---|---|
| `owner/repo` or `https://github.com/owner/repo` | GitHub release (latest) |
| `https://github.com/o/r/releases/download/v1.2.3/app-1.2.3-win64.zip` | GitHub release, package name auto-set to `app-win64.zip` (version stripped) |
| `https://example.com/files/app.zip` | Direct link (updates detected via ETag / Last-Modified / size) |
| `https://example.com/downloads` | Download page (scrapes links; optional link/version regex) |
| *Manual* tab | Import a file you downloaded yourself, or link an existing program |

Package name: `app-win64.zip`, or with wildcards `app-*-win64.zip`. Versions are ignored when matching.
Leave it empty to auto-pick the best file for your OS/CPU.

## Per app
Launch · Update/Install · Check · Reinstall · Install from downloaded file · Open folder · Edit · Remove.
Updates extract over the existing folder, so portable settings stored inside it survive.
Supported archives: `.zip`, `.tar.gz`/`.tgz`; any other file (exe, AppImage) is copied as-is.
