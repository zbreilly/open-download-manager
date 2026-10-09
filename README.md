# Open Download Manager

A cross-platform download manager modeled on Internet Download Manager. It runs on macOS, Windows and Linux from one codebase: a Rust engine with a Tauri 2 + Svelte 5 interface.

## Platforms

| Platform | Interface | Build |
|---|---|---|
| macOS 13+ | Native SwiftUI/AppKit (`apps/macos`) | `scripts/build-macos.sh --install` |
| Windows, Linux | Tauri 2 + Svelte (`src-tauri`, `ui`) | `.github/workflows/build.yml` |

All of them use the same Rust engine (`crates/odm-core`). The macOS app links it as a static library through a small C interface (`crates/odm-ffi`).

## Build on macOS

```bash
git clone https://github.com/zbreilly/open-download-manager
cd open-download-manager
scripts/build-macos.sh --install
```

This builds **Open Download Manager.app** and installs it in /Applications. You need Xcode (or the Command Line Tools) and Rust. The app is signed with your Developer ID if one is in your keychain, and ad-hoc signed otherwise.

| Option | Effect |
|---|---|
| `--install` | copy the app to /Applications and open it |
| `--dmg` | also build a DMG |
| `--universal` | one binary for Apple silicon and Intel |

Every push to `main` that touches the macOS app or the engine also builds it on GitHub Actions (`macos-native` workflow). The DMG and a zipped app are attached to each run. Builds from GitHub aren't signed with your Developer ID, so the first time you open one, right-click it and choose Open (or run `xattr -dr com.apple.quarantine "Open Download Manager.app"`).

The Tauri version still builds on macOS with `scripts/build-macos-tauri.sh`.

## Install the browser extension

The extensions are built into `extension/dist/`. In the app, **Settings → Browser integration → Show extension folder** opens it.

- **Chrome / Edge / Brave / Opera / Vivaldi**: open `chrome://extensions`, turn on *Developer mode*, click *Load unpacked* and pick `extension/dist/chromium`.
- **Firefox**: open `about:debugging#/runtime/this-firefox`, click *Load Temporary Add-on* and pick `extension/dist/firefox/manifest.json`. A permanent install needs the add-on to be signed through addons.mozilla.org.

The extension talks to the app on `127.0.0.1:8901`. The app only accepts requests that come from browser-extension origins, so web pages cannot queue downloads.

## Features

**Engine**
- Downloads with up to 32 connections per file, with IDM-style dynamic segmentation: when a connection finishes its range, it takes over the back half of the largest remaining segment, so every connection stays busy until the end
- Resume across pauses, crashes and restarts. Progress is checkpointed per segment, and `If-Range` with ETag/Last-Modified detects files that changed on the server
- Falls back to one connection, and restarts on failure, when a server doesn't support byte ranges
- Retries with backoff per connection. Clear errors for 403/404/410 and expired links
- **Refresh link**: point an expired download at a new URL and keep the progress already made
- Global speed limit (combined) and per-download limits, both adjustable while downloading
- Proxy settings: system, none, HTTP or SOCKS5, with authentication
- Basic auth, cookies, referrer, custom user agent and custom headers
- Checksum verification (SHA-256, SHA-1, MD5)
- Uses the system trust store for TLS (rustls with the platform verifier)

**App**
- Main window with a library, categories and queues, a sortable, resizable, reorderable table, multi-select, context menus, keyboard shortcuts, a details panel with the segment map and a 60-second speed graph, and a status bar
- Add dialog: probes the link (size, resumability, protocol), fills the name in from `Content-Disposition`, picks the category automatically, offers recent-folder chips, a connections selector, checksum, and auth/cookies/headers. Asks to overwrite, rename or resume when the file already exists
- Batch downloads: several links, or patterns such as `img[001-120].jpg` and `[a-z]`
- Per-download progress window with a live segment map, a per-connection table, a speed graph, a speed limit for that download and a "when finished" action
- Queues and scheduler: start and stop times, weekdays, files at a time, retrying failed downloads, a reorderable queue, and sleep, shut down or quit when the queue finishes (after a 60-second countdown you can cancel)
- Site grabber: lists the files linked from a page, optionally following links one or two levels deep, filtered by type
- Clipboard monitor, drag links onto the window, import and export link lists, and the `odm://add?url=…` deep link
- Menu-bar / system-tray icon showing activity state (and speed, if you turn that on), notifications, launch at login, dark and light themes, and a compact list density

**Browser extension**
- Takes over matching downloads (by file type and optional minimum size), passing along cookies, referrer and user agent so links that need a login still work
- Hold Option/Alt (configurable) while clicking to let the browser handle a download, or pause capture for 10 minutes from the toolbar popup
- Context menus: download a link, media, an image, the links in a selection, or every link on the page
- Detects audio and video files a page loads and lists them in the popup, and shows a "Download" button over videos

## Not supported yet

- HLS/DASH streaming manifests (`.m3u8`, `.mpd`). Media detection only lists direct media files
- FTP

## Development

```bash
# terminal 1: UI with hot reload
npm --prefix ui install && npm --prefix ui run dev
# terminal 2: app pointing at the dev server
cargo run -p open-download-manager
```

Engine tests run a local HTTP server that supports ranges and can simulate failures:

```bash
cargo test -p odm-core
```

To build installers for every platform, push a `v*` tag or run the **build** workflow by hand (`.github/workflows/build.yml`). It produces a DMG (universal), NSIS/MSI installers, and deb/rpm/AppImage packages.

## Layout

```
apps/macos/          native macOS app (SwiftUI/AppKit), build.sh, Info.plist
crates/odm-ffi/      C interface to the engine for native front ends (include/odm.h)
crates/odm-core/     engine: task.rs (segmented downloader), manager.rs (list, queues, scheduler, limits, events),
                     client.rs (HTTP, probe), store.rs (SQLite), grabber.rs, util.rs
src-tauri/           desktop shell: IPC commands, windows, tray, extension bridge (bridge.rs), power actions
ui/                  Svelte 5 frontend (one bundle, routed per window); ui/dist is the prebuilt copy
extension/           browser extension source (src/), manifests, build.sh -> extension/dist
scripts/             build-macos.sh
download-manager-ui/ design spec (HANDOFF.md), tokens, reference screens, brand assets
legacy/              the earlier prototype (std-only engine, Swift shell, v1 extension), kept for reference
```

App data (the download list, queues and settings) is stored in SQLite under `~/Library/Application Support/app.opendm.desktop/` on macOS, `%APPDATA%\app.opendm.desktop\` on Windows and `~/.local/share/app.opendm.desktop/` on Linux. While a download is in progress it is written to `name.odmpart` next to the final file, which is renamed when the download finishes.

## License

Open Download Manager is free software under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only). You may use, study, modify and share it. Any version you distribute, or run as a network service, must be released under the same license with its complete source code.

Third-party components keep their own licenses: the Rust and npm dependencies (MIT, Apache-2.0, BSD, ISC, MPL-2.0 and similar), and IBM Plex Sans and JetBrains Mono under the SIL Open Font License (`ui/src/assets/fonts/`).

### Name and logo

The AGPL covers the code, not the brand. "Open Download Manager", "ODM" and the ODM logo identify the official project. Forks are welcome but must use a different name and logo, and must not suggest they are the official release.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Opening a pull request means you agree to its contributor terms.
