# User guide

## Where things are stored
Everything lives in a `pam-data` folder next to the exe by default. You can move it in **Settings → Data folder → Change location…**: pick a folder and the app keeps its data in a `pam-data` folder inside it (the choice is remembered in a small `pam-location.txt` next to the exe). The `PAM_DATA` environment variable, if set, overrides everything and is used as the exact data folder.

```
pam-data/
├─ config.json     your app list and settings
├─ apps/<id>/      the files of each managed app
├─ icons/<id>.png  custom icons you picked
└─ tmp/            downloads in progress
```
Copying the whole `pam-data` folder to another PC carries your apps, their files and their own settings.

## Adding an app
Click **➕ Add app** and paste into the top box. The type is detected for you; you can override it with the **Type** buttons.

| Type | What to paste | Notes |
|---|---|---|
| GitHub release | `owner/repo`, the repo URL, or a release download link | Tracks the latest release |
| Direct link | A URL to a file | Update detection via ETag, Last-Modified or size |
| Download page | A web page that links to downloads | Links are scraped from the page |
| Manual | nothing to paste | You provide the files yourself (or link a program that stays where it is) |

Then set:
- **Name**: shown in the list.
- **Group** (optional): a folder path such as `Games/Emulators`. See [Groups](#groups).
- **Package name (no version)**: which file to download, see [Sources and matching](sources-and-matching.md). Leave empty to auto-pick the best file for your OS and CPU.
- **Launch args** (optional): passed to the program when you launch it. Use quotes for values with spaces.

Press **Add & install**. The app is downloaded, extracted into `pam-data/apps/<id>/`, and its executable is picked automatically.

### Manual apps
Under the **Manual** type you can choose:
- **Import a file I downloaded**: pick a `.zip`, `.7z`, `.tar.gz`, exe or AppImage. It is installed like a download.
- **Link an existing program**: pick an exe that stays where it is. These apps are labelled **Link** and show the path they point to. Removing one never deletes its files.
- **Just create an empty folder**: for portable apps you install yourself. The folder opens; put the app in it, then use **⋮ → Detect executable** (or Edit) to pick the exe.

## Launching, updating, removing
- **▶ Launch** starts the selected exe with its folder as the working directory.
- **🔄 Check all** looks for updates for every app. A green **⬆ Update** button (or green dot in tile view) means a newer version exists.
- **⬇ Install / update all** installs apps that are not installed yet and updates those with a newer version.
- **⋮ menu** (right-click in tile view): Check for update, Reinstall latest, Install from downloaded file, Open folder, Detect executable, Edit, Set icon, Reset icon (only with a custom icon), Remove.
- **Edit** changes the name, source, group, icon, launch args and the executable (pick from the detected list or browse).
- **Remove** asks first. The **Also delete its files** box always starts unchecked; a red warning shows the folder that would be deleted. Linked programs are never deleted.

Updates are extracted over the existing folder, so settings a portable app stores inside its folder survive.

## Views
- **☰ List** shows one card per app.
- **▦ Tiles** shows icons. Click a tile to launch it (or install it if it is not installed). Right-click for the menu. A green dot means an update is available, a yellow dot means not installed, ⚠ marks an error. Use the **Size** slider to resize tiles.
- The **filter** box searches by name and shows a flat result list in either view.

### Icons
By default the icon is read from the selected executable. If it has none, an icon file near the exe is used (`icon.png`, `<name>.png`, `.DirIcon`, `.ico`), otherwise a coloured tile with the first letter.

**Your own icon:** right-click an app (or use **⋮**) and choose **Set icon…**, or use the **Icon** row in Add / Edit. Pick an image (`.png`, `.jpg`, `.bmp`, `.ico`) or an `.exe` / `.dll` to take its icon. It is converted to a small PNG and copied into `pam-data/icons/`, so it keeps working if the original file moves. **Reset icon** (or **Automatic** in Edit) goes back to the automatic icon. Custom icons are removed with the app and are not part of **Export app list**.

## Groups
Give an app a **Group** such as `Games`. Groups are **one level only** (no groups inside groups); a name with `/` is flattened (`Games/Emulators` becomes `Games - Emulators`).
- **List view**: collapsible folders with an app count; ungrouped apps are listed after them.
- **Tile view**: a group is a folder tile with a 2×2 icon preview. Click it and the group opens in a **floating window** with the group name as its title and the app tiles inside, like a Start-menu folder. Drag the window around, resize it, or close it with ✕ (click the folder tile again to close it too). Launching an app from it closes the window.
- **Ungroup**: right-click a group header (list view) or a folder tile (tile view), or press **Ungroup** in the group window. The apps become ungrouped; nothing is deleted.
- **Rename**: right-click a group header (list view) or a folder tile (tile view) and choose **Rename group…**, or press **Rename** in the group window. If the new name already exists, the two groups are merged.
- **Max icons per row** (Settings) sets how many icons the group window shows per row; it opens sized to fit and can still be resized. The group window has its own icon size.
- A group exists only while an app is in it.

## Backup and moving to another PC
**⇅ Backup → Export app list…** writes a JSON file with each app's name, source, package name, group, launch args and executable. It contains no downloaded files, no custom icons and no GitHub token.

On the other PC: **⇅ Backup → Import app list…**, then **⬇ Install / update all**.

> ⚠ **Only import lists from sources you trust.** A list tells the app where to download programs from; installing them downloads the files from the internet, extracts them, and you can then launch them. The app asks you to confirm this before importing. Imported entries are sanity-checked (unsafe ids and paths are rejected, linked programs must be re-selected), but downloads themselves are not verified.

 Apps that already exist (same id, or same name and source) are skipped. Manual apps have no download source, so they show **📂 Pick file**.

## Settings (⚙)
- **GitHub token** (optional): raises the GitHub API rate limit. Stored in `config.json` in plain text.
- **Check for updates on startup**.
- **Tiles & groups**: the default icon size (main window), the icon size used in the group window, and **Max icons per row** for the group window (the window opens sized to fit it, and tiles shrink only if the row would not fit on screen). The main tile view always fits as many tiles per row as the window allows.
- **Window**: *Remember window size and icon size when closing* saves them on exit and restores them on the next start. *Use the default icon size now* and *Forget saved window size* reset them.
- **Data folder**: shows where your data lives and lets you change it. When you switch, you can copy your apps and settings to the new place (the old folder is left untouched). Not available while `PAM_DATA` is set.
- Shows the version.

## Troubleshooting
| Problem | Fix |
|---|---|
| `GitHub rate limit hit` | Add a token in Settings |
| `No release file matches …` | Adjust the package name; the error lists the available files |
| `No matching download link found on that page` | Adjust the link regex or file filter (download pages) |
| Launch button is disabled | No executable selected: use **Detect executable** or Edit |
| Cannot delete files (is it running?) | Close the app and remove again |
| Update never detected for a direct link | The server sent no ETag, Last-Modified or size; use **Reinstall latest** |
