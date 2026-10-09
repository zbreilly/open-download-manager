# Download Manager — UI Implementation Spec

Audience: the coding model implementing this UI. Follow this document literally. Where it is silent, match the reference designs in `design/reference/`. Do not invent new colors, fonts, or spacing values; use `design/tokens.css`.

App name: **Open Download Manager** (short form **ODM**). Define `APP_NAME = "Open Download Manager"` in one place. All logos, icons, tray, extension and installer graphics are in `brand/` — read `brand/ASSETS.md`.

---

## 0. Stack (decided)

| Layer | Choice | Why |
|---|---|---|
| Shell | **Tauri 2** | Rust backend, native window, tray, notifications, deep links. The designs are HTML/CSS, so they port 1:1. |
| Frontend | **Svelte 5 + TypeScript + Vite** | Fine-grained reactivity for 10+ progress updates/sec without re-rendering the whole list. |
| Styling | Plain CSS using the custom properties in `design/tokens.css`. No Tailwind, no component library. |
| Fonts | IBM Plex Sans (400/500/600), JetBrains Mono (400/500). Bundle `.woff2` files in `src/assets/fonts/`. No runtime Google Fonts requests. |
| Icons | Use **only** the project icon set: `brand/dist/code/icons.ts` + `Icon.svelte` (88 icons, 24×24, `currentColor`). Do not add a third-party icon library. |
| Backend crates | `tokio`, `reqwest` (rustls, http2, stream), `rusqlite` (or `sqlx` sqlite) for persistence, `serde`, `tauri-plugin-notification`, `tauri-plugin-single-instance`, `tauri-plugin-deep-link`. |

If a pure-Rust GUI (Slint / iced / egui) is chosen instead, section 9 maps the tokens; screens and behavior in sections 3–7 still apply.

---

## 1. Project layout

```
src-tauri/
  src/
    main.rs
    engine/            # segmented downloader (out of scope for this doc except the IPC contract)
    ipc.rs             # commands + events listed in section 8
    store.rs           # sqlite persistence
    scheduler.rs
src/
  app.css              # imports design/tokens.css, base resets, font-face
  lib/
    stores/downloads.ts   # Map<id, Download>, updated from engine events
    stores/ui.ts          # selection, active filter, theme, density
    format.ts             # bytes, speed, eta, pct formatters (section 2.3)
    ipc.ts                # typed wrappers for invoke() and listen()
  components/
    AppHeader.svelte
    Sidebar.svelte
    Toolbar.svelte
    DownloadTable.svelte
    DownloadRow.svelte
    DetailsPanel.svelte
    SegmentMap.svelte
    SpeedSparkline.svelte
    StatusPill.svelte
    FileBadge.svelte
    StatusBar.svelte
    SegmentedControl.svelte
    Switch.svelte
    Dialog.svelte
  windows/
    Main.svelte
    AddDownload.svelte
    ProgressWindow.svelte
    Scheduler.svelte
    Settings.svelte
```

Each dialog in `windows/` opens as its own Tauri `WebviewWindow` (not an in-page modal) so it can sit outside the main window like the original app. Use a custom title bar only if `decorations: false`; default is native decorations.

---

## 2. Global rules

### 2.1 Theme
- Default theme is **dark**. Persist user choice: `"dark" | "light" | "system"`.
- Set `document.documentElement.dataset.theme` to `dark` or `light` before first paint (read the setting synchronously from Rust via an init script or a `?theme=` query param on window creation) to avoid a light flash.
- `system` follows `prefers-color-scheme` and updates live.
- Set the Tauri window background to `--bg` of the active theme so resize never flashes white.

### 2.2 Typography
- Body: `--font-ui`, `--fs-md`, `--lh`.
- **Every number uses `--font-mono`**: sizes, speeds, percentages, ETA, connection counts, times, file paths, URLs.
- Section labels: `--fs-xs`, weight 600, uppercase, letter-spacing 0.06em, `--text-muted`.

### 2.3 Formatting (`format.ts`)
- Bytes: base 1024. `< 1024 MB` → one decimal + ` MB`; otherwise two decimals + ` GB`. Below 1 MB → `KB`, one decimal.
- Speed: bytes/sec formatted as above + `/s` (e.g. `48.7 MB/s`). Zero or inactive → `—` (em dash).
- Percent: integer, floor (never show 100% until verified complete).
- ETA: `47s`, `4m 52s`, `1h 08m`, `> 1 day`. Unknown → `—`.
- Added date: `Today 14:02`, `Yesterday 22:41`, otherwise `Oct 6` (same year) / `Oct 6, 2025`.

### 2.4 Status model
| status | label | color token | progress fill |
|---|---|---|---|
| `downloading` | Downloading | `--accent` | accent |
| `paused` | Paused | `--warning` | warning |
| `queued` | Queued, or `Starts HH:MM` when scheduled | `--text-muted` | muted |
| `error` | Failed | `--danger` | danger |
| `complete` | Complete | `--success` | success |
| `connecting` | Connecting | `--accent` | accent, indeterminate stripe |
| `assembling` | Finishing | `--accent` | accent |

Status is never communicated by color alone: always render the text label in a `StatusPill` (7px dot + label, 1px `--border` outline, pill radius).

### 2.5 Interaction
- Hover on buttons/rows: background shifts toward `--surface-2`; transitions use `--motion`.
- Focus: `outline: 2px solid var(--accent); outline-offset: 2px` on every focusable element. Never remove focus outlines.
- Progress bars and segment fills update **without** CSS transitions (they receive updates at 4–10 Hz; transitions cause lag).
- All controls are real `<button>`, `<input>`, `<select>`, `<label>`. Icon-only buttons need `aria-label`.
- Minimum window size: 900×600. Layout must not break down to that size.

### 2.6 Performance
- Engine emits progress at most every 250 ms per download, batched (section 8). Frontend applies a whole batch in one store update.
- Download list virtualizes when > 200 rows (fixed row height makes this simple).

---

## 3. Main window (`design/reference/Main.dc.html`, light variant `MainLight.dc.html`)

Default size 1280×860. Structure, top to bottom:

### 3.1 Header (height ~48px, `--surface`, bottom border)
- Left: 26×26 rounded (7px) accent square containing the ODM mark (`odm-mark`, 19px, stroke 2.6, `--on-accent` color), then `APP_NAME`, 14px/600.
- Center: search field, flex 1 1 280px, max-width 460px, `--surface-2` bg, 1px border, radius 8. Placeholder: `Search downloads, URLs, folders`. Shortcut hint chip `Ctrl K` (mono 11px, bordered) at right of the field. Search filters the current list by file name, URL, and save path, case-insensitive, 120 ms debounce.
- Right: `Total` (muted) + down-arrow icon + total speed in mono, accent color.

### 3.2 Sidebar (220px, `--surface`, right border, 14px/10px padding)
Three sections with uppercase labels, 18px gap between sections:
- **Library**: All downloads · Active · Unfinished · Finished
- **Categories**: Video · Music · Documents · Compressed · Programs · General (user-editable; see Settings → File types & categories)
- **Queues**: one entry per queue (Main queue, Overnight, …)

Each item: full-width button, 7px/10px padding, radius 7; 16px category icon at left (`cat-all`, `cat-active`, `cat-unfinished`, `cat-finished`, `cat-video`, `cat-music`, `cat-documents`, `cat-compressed`, `cat-programs`, `cat-general`; queues use `q-main` / `st-scheduled`), label, count at right in mono 11px muted. Selected item: `--accent-soft` bg, text `--text`, weight 600, icon `--accent`. Unselected: transparent, text and icon `--text-muted`.
- Right-click on a category: Add / Edit / Delete category. Right-click on a queue: Start / Stop / Edit schedule / Delete.
- Drag rows from the table onto a queue to add them to that queue; onto a category to recategorize. Show `--accent` 1px outline on the drop target.

### 3.3 Toolbar (row of 34px buttons, 6px gap, wraps)
Order (icon name in parentheses): **Add URL** (`add-url`; primary: accent fill, `--on-accent` text, 600) · divider · Resume (`resume`) · Pause (`pause`) · Pause all (`pause-all`) · Delete (`delete`) · Delete completed (`delete-completed`) · divider · Start queue (`start-queue`) · Stop queue (`stop-queue`) · Scheduler (`scheduler`) · Grabber (`site-grabber`) · Speed-limit toggle (`speed-limit`) · Options (`options`, icon-only, pushed to the far right with `margin-left:auto`). Icons are 16px.
- Secondary buttons: 1px `--border`, `--surface` bg, icon 15px + label, radius 8, padding 0 12px.
- Resume/Pause/Delete act on the table selection; disabled (40% opacity, `cursor: default`) when the selection has no applicable item.
- Speed-limit toggle: label `No limit` when off; `Limit 25 MB/s` (the configured global value) when on, with `--warning` border and text. Click toggles; right-click opens a small popover to edit the value. `aria-pressed` reflects state.
- Delete opens a confirm: "Delete N downloads?" with checkbox "Also delete files from disk" (unchecked by default).

### 3.4 Download table
Container: `--surface`, 1px border, radius 10, horizontal scroll inside the container (min table width 980px).
Columns (header 12px muted, weight 500, 10px/14px padding, bottom border):

| Column | Content | Align |
|---|---|---|
| Name | FileBadge (34×22, radius 5, `--raised` bg, mono 10px muted extension text, max 4 chars) + file name (500 weight, ellipsis, max 300px) | left |
| Size | total size, mono | left |
| Progress | 120×6 bar (track `--track`, fill by status color, radius 3) + percent mono 12px right-aligned in a 38px box | left |
| Speed | mono, `—` when inactive | right |
| ETA | mono | right |
| Status | StatusPill | left |
| Queue | queue name or `—`, 12px muted | left |
| Added | 12px muted | left |

- Rows: 9px/14px cell padding (comfortable) or 5px/14px (compact); bottom border `--border` per cell.
- Selected row: `--row-selected` bg + inset 3px left bar in `--accent` (`box-shadow: inset 3px 0 0 var(--accent)`).
- Multi-select: Ctrl/Cmd-click toggles, Shift-click ranges, Ctrl/Cmd-A selects all visible.
- Double-click: completed → open file; otherwise open the Progress window.
- Columns are resizable and reorderable; right-click header shows a checklist to show/hide columns (persist).
- Click header to sort; show a small chevron on the sorted column. Default sort: Added, newest first.
- Context menu on rows: Open · Open folder · Resume · Pause · Redownload · Refresh link (for failed/expired) · Copy link · Move to queue ▸ · Category ▸ · Properties · Delete.
- Empty state (no rows in filter): centered muted text "No downloads here" and an **Add URL** secondary button.

### 3.5 Details panel (below the table, `--surface`, border, radius 10, padding 16)
Shows the **focused** row (last clicked). If nothing is selected, show the first row in the current filter.
- Top line: large FileBadge (40×40, radius 9, mono 11px) · name (15px/600, ellipsis) above URL (mono 12px muted, ellipsis) · StatusPill · context button · `Open folder` · `Copy link`.
  - Context button label: `Pause` (downloading), `Resume` (paused/queued), `Open file` (complete), `Refresh link` (error).
- If status is `error`: a full-width message box, `--danger-soft` bg, `--danger` text, mono 12px, radius 8, with the engine's error string.
- Two-column body (wraps below 640px):
  - Left `<dl>` grid (110px label column, 12px): Downloaded (`X of Y`) · Speed · Time left · Connections (`N active / M max`) · Resume (`Supported (byte ranges)` / `Not supported by server`) · Save to (mono path).
  - Right: **SegmentMap** (one cell per connection, 3px gap, height 22px, radius 4, track `--track`, fill = segment completion, status color; completed segments at 55% opacity), then **SpeedSparkline** (60 s window, 64px tall, `--surface-2` bg, radius 6, polyline 1.8px in `--accent`, area fill `--accent-soft`; caption shows `peak X MB/s`).

### 3.6 Status bar (bottom, `--surface`, top border, 12px muted, 7px/16px)
`3 active · 1 paused · 1 scheduled · 1 failed` · `↓ total speed` (mono) · `Speed limit: off | 25 MB/s total` · green dot + `Browser extension connected` (or muted dot + `Browser extension not detected`) · right-aligned free space of the default download drive (mono).

### 3.7 Responsive
Sidebar and main are a wrapping flex row (`sidebar: flex 1 1 220px`, `main: flex 999 1 560px; min-width: 0`). Below ~800px the sidebar stacks on top. The toolbar wraps.

---

## 4. Add download dialog (`AddDownload.dc.html`, 560×680)

Opened by: Add URL button, Ctrl/Cmd-N, clipboard URL detection (if enabled), browser extension handoff, deep link `app://add?url=…`.

Flow: URL field is focused and pre-filled from clipboard if it holds a URL. On URL change (300 ms debounce) call `probe_url`. While probing, the info card shows a skeleton line and "Checking link…".

Fields, top to bottom (16px gap, 18px/20px padding):
1. **URL** (mono 12px input; accent border while focused; red border + message below on invalid URL or probe failure).
2. **Info card** (`--surface-2`, radius 10): large FileBadge + `size · MIME type` (mono) + capability chips with green check icons: `Resumable`, `Byte ranges`, and protocol text (`HTTP/2 · TLS 1.3`). If the server doesn't support ranges, replace the chips with a `--warning` line: "Server doesn't support resume. Download will use 1 connection."
3. **File name** (editable; default from Content-Disposition, else URL path).
4. **Category** and **Queue** selects side by side (2-col grid). Category is auto-picked from the file extension.
5. **Save to**: path input (mono) + Browse button, then a row of **Recent** folder chips (last 4 distinct folders, mono 11px, 28px tall; selected chip has accent border and `--accent-soft` bg). Clicking a chip fills the path. This is a deliberate improvement over the original, whose users complain about having no quick list of recently used folders.
6. **Connections**: segmented control `1 · 4 · 8 · 16 · 32` (default from Settings; selected = accent fill, `--on-accent` text). Disabled at 1 when the server lacks range support.
7. Checkboxes: "Remember this folder for {Category}" (checked) · "Verify checksum after download" (reveals a SHA-256 input when checked).
8. Text button with chevron: "Advanced: authentication, referrer, cookies, headers" — expands a section with username/password, referrer, cookie string, and a key/value header editor.

Footer (`--surface-2`, top border): `Add to queue` (left) · spacer · `Cancel` · **Start download** (primary). Enter = Start download; Esc = Cancel. If the target file exists, Start shows an inline choice: Overwrite / Rename to `name (1).ext` / Resume existing.

---

## 5. Per-download progress window (`DownloadWindow.dc.html`, 760×680)

Opened by double-clicking an unfinished download, or automatically when a download starts if Settings → "Show progress window on start" is on (default off).

- Header: large badge · file name (15px/600) · `host · save folder` mono muted · StatusPill.
- Hero row: percent in mono 40px/500 (the `%` sign 22px muted), then a 4-column stat grid: Downloaded · Speed (accent) · Time left · Resume. Stat labels 11px uppercase muted.
- **Segment map**, 30px tall cells, one per connection. Caption: `Segment map · N connections` and legend "Faded = segment done · light edge = active write head". The active segment's last 10px is drawn at 40% accent to show the write head.
- Lower area, two columns:
  - **Connection table** (scrolls): `#` (zero-padded) · Received (`x / y MB`) · State (`Receiving` text color, `Reconnecting` warning, `Done` success, `Connecting` muted) · Speed (right).
  - Right column: 90px speed sparkline (60 s) · "Limit this download" card with checkbox and, when checked, a MB/s input (overrides global limit) · "When finished" select: Show notification / Open file / Open folder / Do nothing.
- Footer: `Hide` · spacer · `Cancel download` (danger text, confirm first) · **Pause** (primary; becomes Resume).
- On completion the window shows a completion state: hero shows a success check and `Complete`, buttons become `Open file` (primary), `Open folder`, `Close`. Auto-close after 5 s unless the pointer is over the window.

---

## 6. Queues & scheduler (`Scheduler.dc.html`, 820×680)

- Left rail (210px, slightly darker `#111418` in dark / `--surface-2` in light): queue list with counts, selected style as sidebar; dashed-border "New queue" button.
- Right pane for the selected queue:
  - Title + `Start now` / `Stop` buttons.
  - Two cards: **Start at** and **Stop at**, each with an enable checkbox and a time input (mono 14px).
  - **Repeat on**: 7 day toggle buttons (52×34, `aria-pressed`), selected = accent border + `--accent-soft`.
  - **Files at the same time** stepper (− / value / +) and **Retry failed downloads** select.
  - **When the queue finishes**: Put computer to sleep / Shut down computer / Exit app / Do nothing. Shutdown/sleep shows a 60 s cancelable countdown toast before acting.
  - **Order** list: drag handle (6-dot icon), index, file name, size (mono). Reorder by drag or Alt+↑/↓.
- Footer: summary text `Next run: tonight 01:00 · 3 files · 8.4 GB` · Cancel · **Save schedule**.

---

## 7. Settings (`Settings.dc.html`, 960×700)

Left nav (220px): General · Connection & speed · Browser integration · File types & categories · Save locations · Proxy · Notifications · Appearance · Advanced. The reference shows **Connection & speed** and **Appearance**; build the other pages with the same row-card pattern.

**Row-card pattern**: `--surface-2` card, 1px border, radius 10, 14px/16px padding; left: title (500) + one-line description (12px muted); right: the control. Group related rows in one card separated by 1px borders.

Connection & speed:
- Default connections per download: range 1–32 (default 16) + mono value.
- Simultaneous downloads (outside queues): number, default 4.
- **Global speed limit** (MB/s) + switch. This caps the combined speed of all downloads. The original only limits per file, which users find confusing; offer both.
- **Per-download limit** (MB/s) + switch. Input text is muted when its switch is off.
- Checkbox: "Turn on the limit when the app starts".

Appearance:
- Theme: segmented Dark / Light / System (**Dark default**). Applies immediately, no restart.
- List density: Comfortable / Compact.
- "Show speed in the system tray icon".

Other pages (build with the same pattern; no reference image):
- **General**: launch at login; minimize to tray on close; clipboard URL monitoring; show progress window on start; confirm before delete.
- **Browser integration**: extension connection status per browser with "Install extension" buttons; file types to capture (editable extension list); bypass modifier key (default Alt) and a reliable "pause capture for 10 minutes" switch. Users of the original complain that capture is hard to bypass; make this obvious.
- **File types & categories**: table of categories → extensions → default folder; add/edit/delete.
- **Save locations**: default folder; per-category folders; temp folder for in-progress parts.
- **Proxy**: none / system / HTTP / SOCKS5, host, port, auth.
- **Notifications**: on complete, on failure, on queue finished.
- **Advanced**: user agent, retry count and delay, timeout, max segment size, verify TLS (on, with a warning if turned off).

Switch component: 40×22, radius 11; on = `--accent` track with `--on-accent` knob at left 21px; off = `--switch-off` track with `--text-muted` knob at left 3px. `role="switch"` + `aria-checked`.

Footer: `Restore defaults` (muted) · spacer · Cancel · **Save**.

---

## 8. Frontend ↔ backend contract (Tauri IPC)

```ts
type DownloadStatus = 'queued' | 'connecting' | 'downloading' | 'paused' | 'assembling' | 'complete' | 'error';

interface Download {
  id: string;            // uuid
  url: string;
  fileName: string;
  ext: string;           // uppercase, max 4 chars, for FileBadge
  category: string;
  queueId: string | null;
  saveDir: string;
  totalBytes: number | null;   // null = unknown size
  receivedBytes: number;
  speedBps: number;            // smoothed, 1 s EMA
  etaSecs: number | null;
  status: DownloadStatus;
  scheduledAt: string | null;  // ISO, for "Starts HH:MM"
  resumable: boolean;
  maxConnections: number;
  activeConnections: number;
  segments: { start: number; end: number; received: number; state: 'idle'|'connecting'|'receiving'|'reconnecting'|'done' }[];
  speedHistory: number[];      // last 60 samples, 1 Hz, bytes/sec
  error: string | null;
  addedAt: string;             // ISO
  completedAt: string | null;
}
```

Commands (`invoke`):
`list_downloads() -> Download[]` · `probe_url(url, headers?) -> { fileName, totalBytes, mime, resumable, protocol }` · `add_download(req) -> id` · `pause(ids)` · `resume(ids)` · `delete(ids, deleteFiles)` · `refresh_link(id, newUrl)` · `set_global_limit(bps|null)` · `set_download_limit(id, bps|null)` · `open_file(id)` · `open_folder(id)` · `list_queues()` · `save_queue(queue)` · `start_queue(id)` · `stop_queue(id)` · `get_settings()` · `save_settings(settings)` · `recent_folders(limit) -> string[]`.

Events (`listen`):
- `progress_batch`: `{ id, receivedBytes, speedBps, etaSecs, activeConnections, segments }[]`, every 250 ms, only changed downloads.
- `download_changed`: full `Download` on any status change.
- `download_added`, `download_removed`.
- `totals`: `{ speedBps, active, paused, queued, failed, freeBytes }` every 1 s.
- `extension_status`: `{ connected: boolean, browsers: string[] }`.

Sparkline history is kept in Rust and sent in `download_changed`; the frontend appends one sample per second from `progress_batch` between full updates.

---

## 9. If using a native Rust GUI instead of Tauri

Load `design/tokens.json` into a theme struct. Mapping notes:
- **Slint**: put tokens in a `global Theme { in-out property <color> bg; … }` and switch the whole global between dark/light structs. Use `Palette.color-scheme` for system detection.
- **iced**: implement a custom `Theme` with an extended palette; style each widget via `style(|theme, status| …)` closures reading the tokens.
- **egui**: build `Visuals` from tokens (`panel_fill = surface`, `extreme_bg_color = surface_sunken`, `selection.bg_fill = accent_soft`, `widgets.*.rounding = 8`); load the two fonts into `FontDefinitions`; draw segment maps and sparklines with `Painter`.
In all cases keep: dark default, mono numerals, status labels with text, focus rings, and the layout in sections 3–7.

---

## 10. Acceptance checklist

- [ ] Launches in dark theme with no light flash; theme switch applies live and persists.
- [ ] All numbers render in JetBrains Mono; UI text in IBM Plex Sans; fonts load offline.
- [ ] Every status shows a text label, not color only.
- [ ] 50 simultaneous active downloads update smoothly (no dropped frames in the table at 60 Hz).
- [ ] Keyboard: Tab reaches every control with a visible focus ring; Ctrl/Cmd-N add, Ctrl/Cmd-K search, Space pause/resume selection, Delete deletes selection (with confirm), Enter opens.
- [ ] Main window usable at 900×600.
- [ ] Add dialog: recent-folder chips work; connections control disables to 1 for non-resumable servers.
- [ ] Global and per-download speed limits both work and are reflected in the toolbar and status bar.
- [ ] Context menus present on rows, categories, and queues.
- [ ] Light theme passes 4.5:1 contrast for all body text (tokens already do; don't add lighter greys).

## 11. Graphics

Read `brand/ASSETS.md`. It maps every IDM graphic to its ODM replacement and covers Tauri bundle config, tray state switching (`brand/dist/code/tray.rs`), the browser extension icons, and installer images. Status pills, file badges and menus should use the matching icons from the set (`st-*`, `file-*`, action icons).

## 12. Reference files

`design/reference/*.dc.html` are the exact design sources. They use a templating runtime (`<x-dc>`, `{{holes}}`, `<sc-for>`), so do not ship them; read them for layout, inline style values, and copy. Sample data in them (file names, speeds) is illustrative only.
