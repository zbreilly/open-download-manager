# Open Download Manager

A cross-platform download manager modeled on Internet Download Manager. One Rust engine drives a native app on each platform: SwiftUI on macOS, WinUI 3 on Windows, GTK/libadwaita on GNOME and Kirigami on KDE Plasma.

## Platforms

| Platform | Interface | Build |
|---|---|---|
| macOS 13+ (Apple silicon, Intel) | SwiftUI/AppKit (`apps/macos`) | `scripts/build-macos.sh --install`, or the `macos-native` workflow (DMG) |
| Windows 10/11 (x64, ARM64) | WinUI 3, written in Rust with [windows-reactor](https://github.com/microsoft/windows-rs) (`apps/windows`) | `apps\windows\build.ps1 -Installer`, or the `windows-native` workflow (setup `.exe`) |
| GNOME and other Linux desktops (x64, ARM64) | GTK 4 + libadwaita, in Rust (`apps/gnome`) | `packaging/linux/build-packages.sh`, or the `linux-native` workflow (`.deb`, `.rpm`) |
| KDE Plasma 6 (x64, ARM64) | Kirigami / Qt 6, logic in Rust through cxx-qt (`apps/plasma`) | same package as the GNOME interface |
| Older Linux distributions | Tauri 2 + Svelte webview (`src-tauri`, `ui`) | `linux-webview` workflow (`.deb`, `.rpm`) |

All of them use the same Rust engine (`crates/odm-core`) through one small interface (`crates/odm-ffi`): the macOS app links it as a static library and calls it through C, the Rust apps call it directly. What the Rust front ends have in common and no toolkit owns (the download list fed by engine events, filtering and sorting, the Add form's rules, settings merging, formatting) is in `crates/odm-ui`.

Apart from the Swift app everything is Rust. The Plasma app's windows are described in QML, because Kirigami has no Rust API; they only draw what their Rust view-models hand them.

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

## Build on Windows

```powershell
apps\windows\build.ps1 -Installer
```

You need Rust with the MSVC toolchain, and Inno Setup 6 for `-Installer`. The result is a self-contained folder (`dist\windows-native\<arch>\app`): the executable, the Windows App SDK runtime it needs, icons and the browser extension. Nothing else has to be installed on the target PC. The first build downloads that runtime from NuGet.

The installer sets the app up per user (no administrator prompt), registers `odm://` links, and replaces an installed copy of the earlier webview build; the download list and settings carry over.

`windows-reactor` is a preview library. The version in use is pinned to a commit in `apps/windows/Cargo.toml`.

## Build on Linux

```bash
# Debian 13 / Ubuntu 25.04 or later
sudo apt-get install build-essential pkg-config zip libgtk-4-dev libadwaita-1-dev qt6-base-dev qt6-declarative-dev qt6-declarative-dev-tools
# Fedora
sudo dnf install gcc gcc-c++ zip rpm-build gtk4-devel libadwaita-devel qt6-qtbase-devel qt6-qtdeclarative-devel

packaging/linux/build-packages.sh
```

This builds one package for your distribution family and CPU in `dist/linux-native/`. Installing it pulls in what the interfaces need at run time (GTK 4 and libadwaita; Qt 6, Kirigami and the Plasma desktop style). It holds both interfaces and a launcher, `open-download-manager`, that starts the Kirigami one on Plasma and the GTK one everywhere else. `--ui=gnome|plasma` or `ODM_UI=gnome|plasma` overrides the choice.

The tray icon is a StatusNotifierItem: built into Plasma; on GNOME it needs the AppIndicator extension (Ubuntu ships it). Without a tray, closing the window quits the app instead of hiding it.

The webview packages for older distributions have the same package name (`open-download-manager`), so installing one replaces the other. Their files are named `open-download-manager-webview-…`.

## Install the browser extension

`bash extension/build.sh` builds the extensions into `extension/dist/`, and the installers and packages ship a copy. In the app, **Settings → Browser → Show Extension Folder** opens it.

- **Chrome / Edge / Brave / Opera / Vivaldi**: open `chrome://extensions`, turn on *Developer mode*, click *Load unpacked* and pick `extension/dist/chromium`.
- **Firefox**: open `about:debugging#/runtime/this-firefox`, click *Load Temporary Add-on* and pick `extension/dist/firefox/manifest.json`. A permanent install needs the add-on to be signed through addons.mozilla.org.

Then pair it with the app, once per browser: in the app, **Settings → Browser → Pairing code → Copy**; in the extension's popup, open **Connection**, paste the code and click **Pair**. **New Code** in the app replaces the code and unpairs every browser.

The extension talks to the app on `127.0.0.1:8901`. Every message in both directions is encrypted and authenticated (AES-256-GCM) with the pairing code, so:

- a program that takes the app's port can't read what the extension sends (links and their cookies) and can't answer it;
- web pages, other extensions and other programs can't make the app do anything;
- a request from the extension only opens the Add window. Nothing starts until you confirm it.

Cookies, passwords and credential headers are erased from a download's record when it completes, and the database is readable only by your account.

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
- Main window with a library, categories and queues, a sortable list with multi-select, context menus and keyboard shortcuts, a details panel with the segment map and a 60-second speed graph, and a status bar
- Add dialog: probes the link (size, resumability, protocol), fills the name in from `Content-Disposition`, picks the category automatically, offers recent folders, a connections selector, checksum, and auth/cookies/headers. Asks to overwrite, rename or resume when the file already exists
- Batch downloads: several links, or patterns such as `img[001-120].jpg` and `[a-z]`
- Per-download progress window with a live segment map, a per-connection table, a speed graph, a speed limit for that download and a "when finished" action
- Queues and scheduler: start and stop times, weekdays, files at a time, retrying failed downloads, a reorderable queue, and sleep, shut down or quit when the queue finishes (after a 60-second countdown you can cancel)
- Site grabber: lists the files linked from a page, optionally following links one or two levels deep, filtered by type
- Clipboard monitor, drag links onto the window, import and export link lists, and the `odm://add?url=…` deep link
- Menu-bar / system-tray icon showing activity state, notifications, launch at login, and dark and light themes (the Plasma app follows the desktop's color scheme)

**Browser extension**
- Takes over matching downloads (by file type and optional minimum size), passing along cookies, referrer and user agent so links that need a login still work
- Hold Option/Alt (configurable) while clicking to let the browser handle a download, or pause capture for 10 minutes from the toolbar popup
- Context menus: download a link, media, an image, the links in a selection, or every link on the page
- Detects audio and video files a page loads and lists them in the popup, and shows a "Download" button over videos

## Not supported yet

- HLS/DASH streaming manifests (`.m3u8`, `.mpd`). Media detection only lists direct media files
- FTP

## Development

Run an app from the source tree:

```bash
cargo run -p odm-gnome                  # GTK/libadwaita
cargo run -p odm-plasma                 # Kirigami (needs Plasma's QML modules to run)
cargo run -p odm-windows                # on Windows
```

Tests and lints (the `checks` workflow runs the first group on every pull request; the Linux apps are linted in the `linux-native` workflow, where their toolkits are installed):

```bash
cargo test -p odm-core                       # engine, against a local HTTP server that can simulate failures
cargo test -p odm-ffi --features test-hooks  # the engine's interface
cargo test -p odm-ui -p odm-linux            # shared front-end logic, Linux services
cargo clippy -p odm-core -p odm-ffi -p odm-ui -p odm-linux --all-targets -- -D warnings
cargo clippy -p odm-gnome -p odm-plasma --all-targets -- -D warnings
```

The Windows app can be type-checked from macOS or Linux without the engine, whose C dependencies don't cross-compile:

```bash
rustup target add x86_64-pc-windows-msvc
cargo check -p odm-windows --target x86_64-pc-windows-msvc --no-default-features
```

The webview build still needs its own tools: on Linux `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf`, then `npm --prefix ui ci` and `cargo run -p open-download-manager`. `ui/dist` is committed; after changing the UI, run `npm --prefix ui run build` and commit it too (CI fails if it is stale).

### CI and releases

- **`windows-native`**: builds the Windows app for x64 and ARM64, packs the installer, installs it silently, launches it, opens an `odm://` link through the running instance, and runs a real download through the browser bridge, checked byte for byte.
- **`linux-native`**: builds and lints both Linux interfaces in Debian 13 and Fedora containers (x64 and ARM64), then installs each package in a clean container, starts it once as KDE and once as GNOME, opens an `odm://` link, and runs the same download check.
- **`macos-native`**: type-checks and builds the macOS app and DMG, then runs the download check.
- **`linux-webview`**: builds and tests the fallback webview packages.
- **Releases**: push a tag such as `v0.2.0`. The version must match `Cargo.toml`, `tauri.conf.json`, `ui/package.json` and the extension manifests. The workflow builds everything and creates a draft release with all of the installers attached.

Builds made by CI are unsigned. Windows SmartScreen and macOS Gatekeeper will warn the first time you open one.

## Layout

```
apps/macos/          macOS app (SwiftUI/AppKit), build.sh, Info.plist
apps/windows/        Windows app (Rust, WinUI 3 through windows-reactor), build.ps1, installer/
apps/gnome/          GTK 4 + libadwaita app (Rust)
apps/plasma/         Kirigami app: Rust backend, list model and view-models (src/), QML views (qml/)
crates/odm-core/     engine: task.rs (segmented downloader), manager.rs (list, queues, scheduler, limits, events),
                     client.rs (HTTP, probe), store.rs (SQLite), grabber.rs, util.rs
crates/odm-ffi/      the engine's interface: C functions (include/odm.h) and the same contract for Rust (api)
crates/odm-ui/       front-end logic shared by the Rust apps: store, filtering and sorting, Add form, formatting
crates/odm-linux/    Linux desktop services (single instance, tray, notifications, keep-awake, power, autostart)
                     and the open-download-manager launcher
packaging/linux/     desktop file, AppStream metadata, build-packages.sh (.deb / .rpm)
src-tauri/, ui/      the webview build (Tauri 2 + Svelte 5), kept for older Linux distributions
extension/           browser extension source (src/), manifests, build.sh -> extension/dist
scripts/             build-macos.sh (native), build-macos-tauri.sh, ci/ (test server and checks used by CI)
download-manager-ui/ design spec (HANDOFF.md), tokens, reference screens, brand assets
legacy/              the earlier prototype (std-only engine, Swift shell, v1 extension), kept for reference
```

App data (the download list, queues and settings) is stored in SQLite under `~/Library/Application Support/app.opendm.desktop/` on macOS, `%APPDATA%\app.opendm.desktop\` on Windows and `~/.local/share/app.opendm.desktop/` on Linux. While a download is in progress it is written to `name.odmpart` next to the final file, which is renamed when the download finishes.

## License

Open Download Manager is free software under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-only). You may use, study, modify and share it. Any version you distribute, or run as a network service, must be released under the same license with its complete source code.

Third-party components keep their own licenses: the Rust and npm dependencies (MIT, Apache-2.0, BSD, ISC, MPL-2.0 and similar), the toolkits the apps link at run time (GTK and libadwaita, Qt and KDE Frameworks, the Windows App SDK), and IBM Plex Sans and JetBrains Mono under the SIL Open Font License (`ui/src/assets/fonts/`).

### Name and logo

The AGPL covers the code, not the brand. "Open Download Manager", "ODM" and the ODM logo identify the official project. Forks are welcome but must use a different name and logo, and must not suggest they are the official release.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Opening a pull request means you agree to its contributor terms.
