# Architecture

A small native GUI app: [eframe/egui](https://github.com/emilk/egui) for the interface, plain threads for network and disk work.

```
build.rs            icon pipeline + build info
src/
├─ main.rs          UI, the Manager state, add/edit form, background tasks
├─ model.rs         config types and the on-disk layout
├─ net.rs           resolving the latest file for each source, downloading
├─ install.rs       extracting archives, finding the exe, launching
├─ icons.rs         getting an icon for an app
├─ util.rs          version stripping, wildcard matching, platform scoring, small helpers
└─ icon/icon.png    the app's own icon
```

## Data model (`model.rs`)
- `Source`: `GitHub { repo, pattern, prerelease }`, `Direct { url }`, `Site { page_url, link_regex, pattern, version_regex }`, `Manual`.
- `AppEntry`: id, name, source, installed version, executable (relative to the app folder, or absolute for linked programs), launch args, `managed` flag, group, `custom_icon` flag (a linked program is `Manual` with `managed = false`; the UI calls it **Link**).
- `Config`: the app list, GitHub token, auto-check flag, view mode, tile size / default size, group-window icon size and max icons per row, and the remembered window size. Saved as `pam-data/config.json` (written to a temp file, then renamed).
- `Paths`: `pam-data`, `apps/`, `icons/`, `tmp/`. The data folder is `PAM_DATA` if set, else the folder chosen in Settings (stored in `pam-location.txt` next to the exe), else `pam-data` next to the exe. App folders are always `apps/<slug(id)>`, so an id can never point outside `apps/`.

## The UI loop (`main.rs`)
`Manager` implements `eframe::App`. egui is immediate mode: `update()` runs every frame and draws everything from the current state.

1. `poll()` drains messages from background threads.
2. `ensure_icons()` starts icon loads that are needed.
3. The top panel (two rows), then the central panel (list tree or tile grid) are drawn.
4. Widgets do not change state directly. They push `(app id, Act)` pairs into a `Vec`, and the actions are applied after drawing. This avoids borrowing `self` mutably while it is being read to draw.
5. Windows: the group popup, the add/edit form, rename group, the remove confirmation, settings and the import / data-folder dialogs.

## Background work
Network, extraction and icon loading run on `std::thread` workers so the UI never blocks. Workers report back through an `mpsc` channel with `Msg`:

| Message | Meaning |
|---|---|
| `Progress` | status text and download fraction for an app |
| `Checked` | result of an update check (`net::Remote` or an error) |
| `Installed` | result of an install / update |
| `Icon` | a decoded icon image (turned into a texture on the UI thread) |

After sending, a worker calls `ctx.request_repaint()` so the result appears immediately. The `busy` map marks apps with work in progress.

## Install flow
`run_install` in `main.rs`:
1. resolve the latest file (`net::resolve`) unless one is already known;
2. download to `pam-data/tmp/` with throttled progress;
3. `install::install_file` extracts or copies into `pam-data/apps/<id>/`;
4. keep the existing exe or detect one;
5. send `Installed` with the version and exe.

Manual imports skip steps 1 and 2.

## Networking (`net.rs`)
Uses `ureq` (blocking). GitHub uses the REST API; direct links use a `HEAD` request (falling back to `GET`); download pages are fetched and scanned with a regex. See [sources-and-matching.md](sources-and-matching.md).

## Icons (`icons.rs`)
1. A custom icon picked by the user (`<data>/icons/<id>.png`, written by `icons::import_custom`, which accepts png / jpg / bmp / ico or the icon of an exe / dll and stores a PNG of at most 256 px). If it cannot be read, the automatic icon is used.
2. For `.exe` / `.dll`: the first icon group in the PE resources is read with `pelite`, rebuilt as an `.ico`, and decoded with `image`.
3. Otherwise a nearby icon file is chosen by a name score.
4. Otherwise the UI draws a coloured tile with the first letter.

Icons are cached per executable path, version and custom-icon state; setting or resetting a custom icon bumps a revision counter so every cached icon is reloaded. Export / import never carries custom icons (the flag is cleared).

## Dependencies
| Crate | Used for |
|---|---|
| `eframe` / `egui` | window and UI |
| `ureq` | HTTP |
| `serde`, `serde_json` | config and export |
| `regex`, `url`, `percent-encoding` | matching and URLs |
| `zip`, `flate2`, `tar`, `sevenz-rust` | archives |
| `pelite`, `image` | reading and decoding icons |
| `rfd` | native file dialogs |
| `open` | opening folders |
| `winresource` (build, Windows) | embedding the exe icon |

## Ideas for learning / extending
- Self-update support (needs a helper process because a running exe is locked).
- Drag and drop between groups.
- `.icns` icons on macOS, icons inside AppImages.
- Parallel installs with a limit; retry and resume downloads.
