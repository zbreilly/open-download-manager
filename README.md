# Open Download Manager

A cross-platform download manager modeled on Internet Download Manager. One Rust engine drives the app on Windows, macOS and Linux. The app is Tauri 2 with a Svelte 5 interface, and includes the Theme studio (color profiles, fonts, panel art, button skins) and an app-drawn title bar. Native-toolkit apps without theming are also built for each platform.

## Platforms

The app (`src-tauri`, `ui`):

| Platform | Build |
|---|---|
| Windows 10/11 (x64) | `desktop` workflow (NSIS `.exe` and `.msi`) |
| macOS 11+ (Apple silicon, Intel) | `scripts/build-macos-tauri.sh --install`, or the `desktop` workflow (universal DMG) |
| Debian / Ubuntu (x64, ARM64) | `desktop` workflow (`.deb`) |
| Fedora (x64, ARM64) | `desktop` workflow (`.rpm`) |

The native-toolkit apps, written in Rust on each platform's own toolkit. They have the same downloads, queues, scheduler, grabber and browser integration, but not the Theme studio or the custom title bar; they offer the dark and light themes only:

| Platform | Interface | Build |
|---|---|---|
| macOS 13+ (Apple silicon, Intel) | AppKit through [objc2](https://github.com/madsmtm/objc2) (`apps/macos`) | `scripts/build-macos.sh --install`, or the `macos-native` workflow (DMG) |
| Windows 10/11 (x64, ARM64) | WinUI 3 through [windows-reactor](https://github.com/microsoft/windows-rs) (`apps/windows`) | `apps\windows\build.ps1 -Installer`, or the `windows-native` workflow (setup `.exe`) |
| GNOME and other Linux desktops (x64, ARM64) | GTK 4 + libadwaita (`apps/gnome`) | `packaging/linux/build-packages.sh`, or the `linux-native` workflow (`.deb`, `.rpm`) |
| KDE Plasma 6 (x64, ARM64) | Kirigami / Qt 6, logic in Rust through cxx-qt (`apps/plasma`) | same package as the GNOME interface |

All of them use the same Rust engine (`crates/odm-core`). The native-toolkit apps call it through one small interface (`crates/odm-ffi`), and share what no toolkit owns (the download list fed by engine events, filtering and sorting, the Add form's rules, settings merging, formatting) in `crates/odm-ui`. The Plasma app's windows are described in QML, because Kirigami has no Rust API; they only draw what their Rust view-models hand them.

Only one of the two can be installed at a time. They share the app's name, data folder and browser-bridge port, so the download list and settings carry over, and installing one replaces the other: the Linux packages have the same package name (`open-download-manager`), and the native Windows installer removes an installed copy of the app first. In a release, the native-toolkit files have `-native` in their names.

## Build the app

On macOS:

```bash
git clone https://github.com/zbreilly/open-download-manager
cd open-download-manager
scripts/build-macos-tauri.sh --install
```

This builds **Open Download Manager.app** and a DMG in `dist/macos/`, and installs the app in /Applications. You need the Xcode Command Line Tools and Rust; Node is optional, because the built interface in `ui/dist` is committed. The app is signed with your Developer ID if one is in your keychain, and ad-hoc signed otherwise. `--universal` builds one binary for Apple silicon and Intel, `--no-dmg` skips the DMG.

On Windows and Linux (Linux needs `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf`, Debian/Ubuntu names):

```bash
bash extension/build.sh
npm --prefix ui ci
npm --prefix ui run build
cd src-tauri && ../ui/node_modules/.bin/tauri build
```

Builds from GitHub aren't signed. On macOS, the first time you open one, right-click it and choose Open (or run `xattr -dr com.apple.quarantine "Open Download Manager.app"`).

## Build the native-toolkit apps

### macOS

```bash
scripts/build-macos.sh --install
```

This builds the AppKit app in `dist/macos-native/` and installs it in /Applications, replacing the app above. You need the Xcode Command Line Tools and Rust. `--dmg` also builds a DMG and `--universal` builds one binary for Apple silicon and Intel.

### Windows

```powershell
apps\windows\build.ps1 -Installer
```

You need Rust with the MSVC toolchain, and Inno Setup 6 for `-Installer`. The result is a self-contained folder (`dist\windows-native\<arch>\app`): the executable, the Windows App SDK runtime it needs, icons and the browser extension. Nothing else has to be installed on the target PC. The first build downloads that runtime from NuGet.

The installer sets the app up per user (no administrator prompt) and registers `odm://` links.

`windows-reactor` is a preview library. The version in use is pinned to a commit in `apps/windows/Cargo.toml`.

### Linux

```bash
# Debian 13 / Ubuntu 25.04 or later
sudo apt-get install build-essential pkg-config zip libgtk-4-dev libadwaita-1-dev qt6-base-dev qt6-declarative-dev qt6-declarative-dev-tools
# Fedora
sudo dnf install gcc gcc-c++ zip rpm-build gtk4-devel libadwaita-devel qt6-qtbase-devel qt6-qtdeclarative-devel

packaging/linux/build-packages.sh
```

This builds one package for your distribution family and CPU in `dist/linux-native/`. Installing it pulls in what the interfaces need at run time (GTK 4 and libadwaita; Qt 6, Kirigami and the Plasma desktop style). It holds both interfaces and a launcher, `open-download-manager`, that starts the Kirigami one on Plasma and the GTK one everywhere else. `--ui=gnome|plasma` or `ODM_UI=gnome|plasma` overrides the choice.

The tray icon is a StatusNotifierItem: built into Plasma; on GNOME it needs the AppIndicator extension (Ubuntu ships it). Without a tray, closing the window quits the app instead of hiding it.

## Install the browser extension

`bash extension/build.sh` builds the extensions into `extension/dist/`, and the installers and packages ship a copy. In the app, **Settings → Browser integration → Show extension folder** opens it (**Settings → Browser** in the native-toolkit apps).

- **Chrome / Edge / Brave / Opera / Vivaldi**: open `chrome://extensions`, turn on *Developer mode*, click *Load unpacked* and pick `extension/dist/chromium`.
- **Firefox**: open `about:debugging#/runtime/this-firefox`, click *Load Temporary Add-on* and pick `extension/dist/firefox/manifest.json`. A permanent install needs the add-on to be signed through addons.mozilla.org.

Then pair the extension with the app, once per browser: in the app open **Settings → Browser integration** (**Settings → Browser** in the native-toolkit apps) and press **Copy key**; click the extension's toolbar button, open **Connection**, paste the key under **Pairing key** and save.

The extension talks to the app on `127.0.0.1:8901`. Everything it sends (links and the cookies that go with them) is encrypted with the pairing key, and the app's answers are too. Another program that listens on that port learns nothing and is not believed, and nothing that lacks the key — a web page, another extension, a local script — can make the app start a download. **New key** in the same settings page unpairs every browser.

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
- Menu-bar / system-tray icon showing activity state, notifications, launch at login, dark and light themes, and a compact list density

**Browser extension**
- Takes over matching downloads (by file type and optional minimum size), passing along cookies, referrer and user agent so links that need a login still work
- Hold Option/Alt (configurable) while clicking to let the browser handle a download, or pause capture for 10 minutes from the toolbar popup
- Context menus: download a link, media, an image, the links in a selection, or every link on the page
- Detects audio and video files a page loads and lists them in the popup, and shows a "Download" button over videos

- Theme studio: color profiles that restyle every window (colors, outlines, fonts, panel art, button pictures), and a title bar drawn by the app that follows the profile

The native-toolkit apps have everything in this list except the Theme studio, the custom title bar and the list density; the Plasma app follows the desktop's color scheme.

If a colour profile from the Theme studio ever makes the windows hard to use, choose **Use the default theme** from the tray menu, or start the app with `--reset-theme`. The profile stays in the Theme studio's list.

## Not supported yet

- HLS/DASH streaming manifests (`.m3u8`, `.mpd`). Media detection only lists direct media files
- FTP

## Development

Run the app from the source tree (on Linux, install the libraries listed under "Build the app" first):

```bash
bash extension/build.sh                 # the app bundles extension/dist
npm --prefix ui ci
# terminal 1: UI with hot reload
npm --prefix ui run dev
# terminal 2: app pointing at the dev server
cargo run -p open-download-manager
```

`ui/dist` is committed so the macOS script works without Node. After changing the UI, run `npm --prefix ui run build` and commit `ui/dist` too; CI fails if it is stale.

Run a native-toolkit app:

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
cargo clippy -p odm-macos --all-targets -- -D warnings    # on a Mac
npm --prefix ui run check
```

A debug build of the native macOS app (`cargo run -p odm-macos`) reads a few variables for trying it without touching the installed copy: `ODM_DATA_DIR=<folder>` keeps its list and settings there, `ODM_PORT=<port>` moves the browser bridge off the port the extension uses, `ODM_OPEN=add,batch,web,grabber,scheduler,settings,progress` opens those windows at startup, and `ODM_SHOTS=<folder>` keeps the app in the background and saves a PNG of each of its windows there every two seconds.

The native Windows app can be type-checked from macOS or Linux without the engine, whose C dependencies don't cross-compile:

```bash
rustup target add x86_64-pc-windows-msvc
cargo check -p odm-windows --target x86_64-pc-windows-msvc --no-default-features
```

### CI and releases

- **`desktop`** (every push to `main` outside the native-toolkit apps): builds the app's Windows installers and Linux `.deb`/`.rpm` packages for x64 and ARM64. Each is installed (in clean Debian and Fedora containers for Linux), launched, and used for a real download through the browser bridge, which is checked byte for byte. For tags and manual runs it also builds the universal macOS DMG and runs the same check. The Tauri CLI version is pinned in `ui/package.json`.
- **`windows-native`**: builds the native Windows app for x64 and ARM64, packs the installer, installs it silently, launches it, opens an `odm://` link through the running instance, and runs the download check.
- **`linux-native`**: builds and lints both Linux interfaces in Debian 13 and Fedora containers (x64 and ARM64), then installs each package in a clean container, starts it once as KDE and once as GNOME, opens an `odm://` link, and runs the download check.
- **`macos-native`**: lints and builds the native macOS app and DMG, launches it, opens an `odm://` link, and runs the download check.
- **Releases**: push a tag such as `v0.2.0`. The version must match `Cargo.toml`, `tauri.conf.json`, `ui/package.json` and the extension manifests. The `desktop` workflow builds the app for every platform and the native-toolkit apps, and creates a draft release with all of the installers attached.

Builds made by CI are unsigned. Windows SmartScreen and macOS Gatekeeper will warn the first time you open one. To check a release download, compare it with `SHA256SUMS.txt` from the same release (`sha256sum -c SHA256SUMS.txt`, or `Get-FileHash` on Windows). Once the repository is public, each release file also carries a build provenance attestation: `gh attestation verify <file> --repo <owner>/<repo>` confirms it was built by this repository's workflow.

## Layout

```
src-tauri/, ui/      the app (Tauri 2 shell and commands; Svelte 5 interface, Theme studio)
apps/macos/          native macOS app (Rust, AppKit through objc2), build.sh, Info.plist
apps/windows/        native Windows app (Rust, WinUI 3 through windows-reactor), build.ps1, installer/
apps/gnome/          GTK 4 + libadwaita app (Rust)
apps/plasma/         Kirigami app: Rust backend, list model and view-models (src/), QML views (qml/)
crates/odm-core/     engine: task.rs (segmented downloader), manager.rs (list, queues, scheduler, limits, events),
                     client.rs (HTTP, probe), store.rs (SQLite), grabber.rs, util.rs
crates/odm-ffi/      the engine's interface: C functions (include/odm.h) and the same contract for Rust (api)
crates/odm-ui/       front-end logic shared by the native-toolkit apps: store, filtering and sorting, Add form, formatting
crates/odm-linux/    Linux desktop services (single instance, tray, notifications, keep-awake, power, autostart)
                     and the open-download-manager launcher
packaging/linux/     desktop file, AppStream metadata, build-packages.sh (.deb / .rpm)
extension/           browser extension source (src/), manifests, build.sh -> extension/dist
scripts/             build-macos-tauri.sh (the app), build-macos.sh (native), ci/ (test server and checks used by CI)
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
