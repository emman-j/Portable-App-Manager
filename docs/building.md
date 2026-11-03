# Building

## Requirements
- A recent stable Rust toolchain.
- Windows: the Windows SDK (`rc.exe`, installed with Visual Studio Build Tools) to embed the exe icon.
- Linux: `libxkbcommon-dev` and `libgtk-3-dev` (or rely on xdg-portal) for the window and file dialogs.

```
cargo build --release
```
Output: `target\release\portable-app-manager.exe`. `cargo run --release` builds and starts it.

## build.rs
`build.rs` must sit next to `Cargo.toml` (not in `src/`). It:
1. reads `src/icon/icon.png` and writes a square 256×256 PNG used as the window / taskbar icon;
2. on Windows, builds a multi-size `.ico` and embeds it in the exe (shown by Explorer and shortcuts);
3. exposes the git short hash and build date to the program.

Problems with the icon are build **errors** so a missing icon is never silent. To build without the exe icon (for example no Windows SDK), set `PAM_SKIP_ICON=1`:
```powershell
$env:PAM_SKIP_ICON = "1"; cargo build --release
```
After a successful build you should see `app icon loaded from src/icon/icon.png` and `exe icon embedded`.

Explorer caches icons per path. If the exe icon looks stale, copy the exe to a new name or run `ie4uinit.exe -show`.

## Changing the icon
Replace `src/icon/icon.png` (any common image format, ideally square and 256×256 or larger) and rebuild.

## Version
Edit `version` in `Cargo.toml`. It is shown in the window title, the top bar and Settings (with the git hash and build date), and in the exe's file properties.

## Releasing
1. Update `version` in `Cargo.toml` and build.
2. Copy `target\release\portable-app-manager.exe` to `portable-app-manager-windows-x64.exe` (keep the name the same every release).
3. Create a release tagged `vX.Y.Z` and upload that file.
4. Optional checksum: `Get-FileHash portable-app-manager-windows-x64.exe -Algorithm SHA256`.
