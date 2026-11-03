# User guide

## Where things are stored
Everything lives in a `pam-data` folder next to the exe (set the `PAM_DATA` environment variable to use another place):

```
pam-data/
├─ config.json     your app list and settings
├─ apps/<id>/      the files of each managed app
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
| Manual | nothing to paste | You provide the files yourself |

Then set:
- **Name**: shown in the list.
- **Group** (optional): a folder path such as `Games/Emulators`. See [Groups](#groups).
- **Package name (no version)**: which file to download, see [Sources and matching](sources-and-matching.md). Leave empty to auto-pick the best file for your OS and CPU.
- **Launch args** (optional): passed to the program when you launch it. Use quotes for values with spaces.

Press **Add & install**. The app is downloaded, extracted into `pam-data/apps/<id>/`, and its executable is picked automatically.

### Manual apps
Under the **Manual** type you can choose:
- **Import a file I downloaded**: pick a `.zip`, `.7z`, `.tar.gz`, exe or AppImage. It is installed like a download.
- **Link an existing program**: pick an exe that stays where it is. Removing it never deletes its files.
- **Just create an empty folder**: for portable apps you install yourself. The folder opens; put the app in it, then use **⋮ → Detect executable** (or Edit) to pick the exe.

## Launching, updating, removing
- **▶ Launch** starts the selected exe with its folder as the working directory.
- **🔄 Check all** looks for updates for every app. A green **⬆ Update** button (or green dot in tile view) means a newer version exists.
- **⬇ Install / update all** installs apps that are not installed yet and updates those with a newer version.
- **⋮ menu** (right-click in tile view): Check for update, Reinstall latest, Install from downloaded file, Open folder, Detect executable, Edit, Remove.
- **Edit** changes the name, source, group, launch args and the executable (pick from the detected list or browse).
- **Remove** asks first. The **Also delete its files** box always starts unchecked; a red warning shows the folder that would be deleted. Linked programs are never deleted.

Updates are extracted over the existing folder, so settings a portable app stores inside its folder survive.

## Views
- **☰ List** shows one card per app.
- **▦ Tiles** shows icons. Click a tile to launch it (or install it if it is not installed). Right-click for the menu. A green dot means an update is available, a yellow dot means not installed, ⚠ marks an error. Use the **Size** slider to resize tiles.
- The **filter** box searches by name and shows a flat result list in either view.

### Icons
The icon is read from the selected executable. If it has none, an icon file near the exe is used (`icon.png`, `<name>.png`, `.DirIcon`, `.ico`), otherwise a coloured tile with the first letter.

## Groups
Give an app a **Group** such as `Games/Emulators`; `/` creates nesting.
- **List view**: collapsible folders with an app count, sub-folders first.
- **Tile view**: folder tiles with a 2×2 icon preview. Click a folder to open it; use the **Home › Games › …** bar to go back.
- A group exists only while an app is in it. To rename a group, change the Group field of its apps.

## Backup and moving to another PC
**⇅ Backup → Export app list…** writes a JSON file with each app's name, source, package name, group, launch args and executable. It contains no downloaded files and no GitHub token.

On the other PC: **⇅ Backup → Import app list…**, then **⬇ Install / update all**. Apps that already exist (same id, or same name and source) are skipped. Manual apps have no download source, so they show **📂 Pick file**.

## Settings (⚙)
- **GitHub token** (optional): raises the GitHub API rate limit. Stored in `config.json` in plain text.
- **Check for updates on startup**.
- Shows the version and the data folder.

## Troubleshooting
| Problem | Fix |
|---|---|
| `GitHub rate limit hit` | Add a token in Settings |
| `No release file matches …` | Adjust the package name; the error lists the available files |
| `No matching download link found on that page` | Adjust the link regex or file filter (download pages) |
| Launch button is disabled | No executable selected: use **Detect executable** or Edit |
| Cannot delete files (is it running?) | Close the app and remove again |
| Update never detected for a direct link | The server sent no ETag, Last-Modified or size; use **Reinstall latest** |
