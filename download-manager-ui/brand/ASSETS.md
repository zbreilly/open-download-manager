# Open Download Manager — Graphics & Icons

Every graphic the app needs, generated from one source file. To change an icon, edit `src/icons.json` and run:

```
pip install cairosvg pillow fonttools
PLEX_SEMIBOLD=path/to/IBMPlexSans-SemiBold.otf python3 src/build.py dist
```

All icons are original drawings on a 24×24 grid, 2-unit round stroke, `currentColor`. No third-party icon set is included.

## Brand

| | |
|---|---|
| Name | Open Download Manager (short form: ODM) |
| Mark | Download arrow inside a progress ring that is open at the lower right; a dot marks the leading edge. |
| Tile color | `#5B9DFF` (`--accent`) |
| Mark ink on tile | `#0A1426` (`--on-accent`) |
| Wordmark | IBM Plex Sans SemiBold, outlined to paths (no font needed to display it) |

Files in `dist/logo/`: `odm-mark.svg` (currentColor), `odm-mark-accent.svg`, `odm-app-icon.svg`, `odm-app-icon-macos.svg`, `odm-wordmark-on-dark|on-light.svg` (+ `@2x.png`), `odm-lockup-stacked-on-dark|on-light.svg`.

## IDM → ODM graphic map

| IDM graphic | ODM asset |
|---|---|
| Program / desktop / taskbar icon | `app-icons/` (`icon.ico`, `icon.icns`, PNG set) |
| Tray icon (and its activity states) | `tray/windows-linux/tray-<state>-16|32.png` + `.ico`; `tray/macos/tray-<state>Template.png|@2x.png` |
| Toolbar: Add URL, Resume, Stop, Stop All, Delete, Delete Completed, Options, Scheduler, Start Queue, Stop Queue, Grabber, Tell a Friend | `icons/svg/`: `add-url`, `resume`, `pause` / `stop`, `pause-all`, `delete`, `delete-completed`, `options`, `scheduler`, `start-queue`, `stop-queue`, `site-grabber`, `share`; plus `speed-limit`, `search` |
| Categories tree: All Downloads, Compressed, Documents, Music, Programs, Video, General, Unfinished, Finished, Grabber projects, Queues | `cat-all`, `cat-compressed`, `cat-documents`, `cat-music`, `cat-programs`, `cat-video`, `cat-general`, `cat-unfinished`, `cat-finished`, `cat-grabber`, `cat-queues`; new: `cat-active`, `cat-images` |
| "Q" column: main queue (yellow), sync queue (green) | `q-main`, `q-sync`. Tint with `--warning` / `--success` if you want IDM's color cue. |
| Download status icons in list | `st-downloading`, `st-connecting`, `st-paused`, `st-queued`, `st-scheduled`, `st-error`, `st-complete`, `st-no-resume` |
| File icons in list | `file-generic`, `file-video`, `file-audio`, `file-image`, `file-archive`, `file-document`, `file-executable`, `file-disc` |
| Tasks / Downloads / context menus | `download`, `batch-download`, `add-from-clipboard`, `add-to-queue`, `remove-from-queue`, `refresh-link`, `redownload`, `open-file`, `open-folder`, `move-file`, `copy-link`, `properties`, `import`, `export`, `exit` |
| Options dialog tabs | `browser-integration`, `file-types`, `open-folder` (save to), `segments` (connection), `proxy`, `credentials` (site logins), `clipboard-monitor`, `notifications`, `theme-dark` / `theme-light`, `verify` |
| Scheduler "when done" actions | `shutdown`, `sleep`, `exit` |
| Help menu | `help`, `check-updates`, `properties` (about) |
| Browser "Download this video" panel | `browser-extension/download-panel-button.svg` (+ `@2x.png`) |
| Browser extension icon & context menu | `browser-extension/icon-{16,19,32,38,48,96,128}.png`, `icon-{16,32}-paused.png` |
| "Download complete" dialog | Rendered as an OS notification using the app icon (see Brand board on the canvas) |
| Installer graphics | `installer/nsis-header.bmp` (150×57), `nsis-sidebar.bmp` (164×314), `wix-banner.bmp` (493×58), `wix-dialog.bmp` (493×312), `dmg-background.png` (660×400, + `@2x`) |
| UI glyphs (close, chevrons, sort, drag handle, more, check, plus/minus, warning, external) | `icons/svg/` same names |

Full list with labels: `icons/manifest.json` (88 icons).

## Icon formats

- `icons/svg/<name>.svg`: one file per icon, `stroke="currentColor"`.
- `icons/sprite.svg`: all icons as `<symbol id="<name>">`. Use `<svg><use href="sprite.svg#resume"/></svg>`.
- `icons/png/{light,dark}/{16,24,32,48}/<name>.png`: for native menus or anywhere SVG isn't available. `light` = `#E7E9EC` ink (for dark UI), `dark` = `#14171C` ink.
- `code/icons.ts` + `code/Icon.svelte`: preferred for the Svelte frontend. Usage: `<Icon name="resume" size={16} />`. Icon-only buttons pass `label` for accessibility.

Sizes: toolbar 16px, sidebar 16px, menus 16px, dialog headers 20px, empty states 32–48px. Keep stroke at 2 for ≤24px; use 1.75 at 32px+ if it looks heavy.

## Wiring into Tauri 2

1. Copy `dist/app-icons/*` to `src-tauri/icons/` (or run `npx tauri icon dist/app-icons/app-icon.png`, then replace `icon.ico` and `icon.icns` with the ones here, which have hand-tuned small sizes and the macOS shadow).
2. `tauri.conf.json`:
```json
{
  "productName": "Open Download Manager",
  "identifier": "org.opendownloadmanager.app",
  "bundle": {
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
    "windows": {
      "nsis": { "headerImage": "installer/nsis-header.bmp", "sidebarImage": "installer/nsis-sidebar.bmp", "installerIcon": "icons/icon.ico" },
      "wix": { "bannerPath": "installer/wix-banner.bmp", "dialogImagePath": "installer/wix-dialog.bmp" }
    },
    "macOS": { "dmg": { "background": "installer/dmg-background.png", "windowSize": { "width": 660, "height": 400 }, "appPosition": { "x": 180, "y": 215 }, "applicationFolderPosition": { "x": 480, "y": 215 } } }
  }
}
```
The DMG background's arrow sits between those two icon positions. Check the key names against the Tauri 2 config schema for the version you install. The `identifier` is a placeholder; use a domain you control.
3. Tray: copy `dist/tray/windows-linux/*.png` and `dist/tray/macos/*.png` into `src-tauri/icons/tray/`. `code/tray.rs` has `set_tray_state()`. Call it whenever totals change.
   - States: `idle`, `downloading`, `paused`, `error`, `complete` (show 5 s after a download finishes, then revert), `limited` (speed limiter on while downloading).
   - Priority when several apply: error > downloading > limited > paused > complete > idle. (Limited is shown instead of downloading when the limiter is on.)
   - macOS uses the `Template` images with `set_icon_as_template(true)`, so the OS colors them for light and dark menu bars. The badge glyph is cut out of the badge, so state reads without color.
4. Linux: `app-icons/linux/hicolor/<size>/apps/open-download-manager.png` goes to `/usr/share/icons/hicolor/` (Tauri's deb/rpm/AppImage bundlers handle this from `bundle.icon`).

## Browser extension

`manifest.json`:
```json
"icons": { "16": "icons/icon-16.png", "32": "icons/icon-32.png", "48": "icons/icon-48.png", "96": "icons/icon-96.png", "128": "icons/icon-128.png" },
"action": { "default_icon": { "16": "icons/icon-16.png", "19": "icons/icon-19.png", "32": "icons/icon-32.png", "38": "icons/icon-38.png" } }
```
When capture is paused, call `chrome.action.setIcon({ path: { "16": "icons/icon-16-paused.png", "32": "icons/icon-32-paused.png" } })`.

The in-page "Download with ODM" button: render it in a shadow DOM from `download-panel-button.svg`, or rebuild it in HTML. It is 34px tall, radius 9, `#5B9DFF` fill, `#0A1426` text and mark, with a divider and a chevron that opens a quality/format menu. Position it 12px from the top-right corner of the video element.

## Canvas reference

The design canvas has a **Brand** board (logo, app icon sizes, tray states in all three environments, extension, notification, installer) and an **Icon library** board (all 88 icons, filterable, with tone/size/stroke tweaks).
