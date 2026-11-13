//! IPC commands (see HANDOFF.md §8). All async so they run on the tokio
//! runtime that the engine needs.

use crate::state::AppState;
use crate::windows;
use odm_core::manager::DownloadPatch;
use odm_core::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn list_downloads(st: State<'_, AppState>) -> R<Vec<Download>> {
    Ok(st.mgr.list())
}

#[tauri::command]
pub async fn get_download(st: State<'_, AppState>, id: String) -> R<Option<Download>> {
    Ok(st.mgr.get(&id))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProbeOpts {
    referrer: Option<String>,
    cookies: Option<String>,
    headers: Vec<(String, String)>,
    username: Option<String>,
    password: Option<String>,
}

#[tauri::command]
pub async fn probe_url(st: State<'_, AppState>, url: String, opts: Option<ProbeOpts>) -> R<ProbeResult> {
    let o = opts.unwrap_or_default();
    st.mgr
        .probe(&url, o.referrer, o.cookies, o.headers, o.username, o.password)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn add_download(app: AppHandle, st: State<'_, AppState>, req: AddRequest) -> R<String> {
    let start = req.start;
    let id = st.mgr.add(req).map_err(err)?;
    if start && st.mgr.settings().show_progress_on_start {
        let _ = windows::open(&app, "progress", Some(&id), None);
    }
    Ok(id)
}

#[tauri::command]
pub async fn add_many(st: State<'_, AppState>, reqs: Vec<AddRequest>) -> R<Vec<String>> {
    let mut ids = vec![];
    let mut errors = vec![];
    for r in reqs {
        match st.mgr.add(r) {
            Ok(id) => ids.push(id),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if ids.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(ids)
}

#[tauri::command]
pub async fn pause(st: State<'_, AppState>, ids: Vec<String>) -> R<()> {
    st.mgr.pause(&ids);
    Ok(())
}

#[tauri::command]
pub async fn pause_all(st: State<'_, AppState>) -> R<()> {
    st.mgr.pause_all();
    Ok(())
}

#[tauri::command]
pub async fn resume(st: State<'_, AppState>, ids: Vec<String>) -> R<()> {
    st.mgr.resume(&ids);
    Ok(())
}

#[tauri::command]
pub async fn delete(st: State<'_, AppState>, ids: Vec<String>, delete_files: bool) -> R<()> {
    st.mgr.delete(&ids, delete_files);
    Ok(())
}

#[tauri::command]
pub async fn delete_completed(st: State<'_, AppState>) -> R<usize> {
    Ok(st.mgr.delete_completed().len())
}

#[tauri::command]
pub async fn redownload(st: State<'_, AppState>, id: String) -> R<()> {
    st.mgr.redownload(&id);
    Ok(())
}

#[tauri::command]
pub async fn refresh_link(st: State<'_, AppState>, id: String, new_url: String) -> R<()> {
    st.mgr.refresh_link(&id, &new_url).await.map_err(err)
}

#[tauri::command]
pub async fn set_global_limit(st: State<'_, AppState>, bps: Option<u64>) -> R<()> {
    st.mgr.set_global_limit(bps);
    Ok(())
}

#[tauri::command]
pub async fn set_download_limit(st: State<'_, AppState>, id: String, bps: Option<u64>) -> R<()> {
    st.mgr.set_download_limit(&id, bps);
    Ok(())
}

#[tauri::command]
pub async fn update_download(st: State<'_, AppState>, id: String, patch: DownloadPatch) -> R<()> {
    st.mgr.update_download(&id, patch);
    Ok(())
}

#[tauri::command]
pub async fn set_category(st: State<'_, AppState>, ids: Vec<String>, category: String) -> R<()> {
    st.mgr.set_category(&ids, &category);
    Ok(())
}

#[tauri::command]
pub async fn move_to_queue(st: State<'_, AppState>, ids: Vec<String>, queue_id: Option<String>) -> R<()> {
    st.mgr.move_to_queue(&ids, queue_id.as_deref());
    Ok(())
}

fn path_of(st: &AppState, id: &str) -> R<std::path::PathBuf> {
    let d = st.mgr.get(id).ok_or("download not found")?;
    Ok(d.final_path())
}

#[tauri::command]
pub async fn open_file(app: AppHandle, st: State<'_, AppState>, id: String) -> R<()> {
    let p = path_of(&st, &id)?;
    if !p.exists() {
        return Err("the file is no longer there — it may have been moved or deleted".into());
    }
    app.opener().open_path(p.to_string_lossy(), None::<&str>).map_err(err)
}

#[tauri::command]
pub async fn open_folder(app: AppHandle, st: State<'_, AppState>, id: String) -> R<()> {
    let d = st.mgr.get(&id).ok_or("download not found")?;
    let p = d.final_path();
    let part = odm_core::util::part_path(&p);
    if p.exists() {
        app.opener().reveal_item_in_dir(&p).map_err(err)
    } else if part.exists() {
        app.opener().reveal_item_in_dir(&part).map_err(err)
    } else {
        app.opener().open_path(d.save_dir, None::<&str>).map_err(err)
    }
}

/// Folders the UI may open with `open_path`: the configured download
/// folders (default, temporary, per category), the app data folder and the
/// bundled extension folder.
fn open_roots(app: &AppHandle, st: &AppState) -> Vec<PathBuf> {
    let s = st.mgr.settings();
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(&s.default_folder)];
    if let Some(t) = s.temp_folder.as_ref().filter(|t| !t.trim().is_empty()) {
        roots.push(PathBuf::from(t));
    }
    for c in &s.categories {
        roots.push(PathBuf::from(odm_core::model::category_folder(&s, &c.id)));
    }
    if let Ok(d) = app.path().app_data_dir() {
        roots.push(d);
    }
    roots.extend(extension_dirs(app));
    roots
}

/// Allow opening only paths that belong to a known download (its file,
/// partial file or folder) or lie inside one of [`open_roots`].
fn check_open_path(app: &AppHandle, st: &AppState, path: &str) -> R<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("path must be absolute".into());
    }
    let canon = |p: &Path| p.canonicalize().ok();
    let target = canon(p).ok_or("the file or folder is no longer there")?;
    for d in st.mgr.list() {
        let f = d.final_path();
        let part = odm_core::util::part_path(&f);
        for cand in [f.clone(), part, PathBuf::from(&d.save_dir)] {
            if canon(&cand).as_ref() == Some(&target) {
                return Ok(p.to_path_buf());
            }
        }
    }
    for root in open_roots(app, st) {
        if let Some(r) = canon(&root) {
            if target.starts_with(&r) {
                return Ok(p.to_path_buf());
            }
        }
    }
    Err("this path is not a download or a download folder".into())
}

fn absolute(path: &str) -> R<&Path> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err("path must be absolute".into())
    }
}

#[tauri::command]
pub async fn open_path(app: AppHandle, st: State<'_, AppState>, path: String) -> R<()> {
    let p = check_open_path(&app, &st, &path)?;
    app.opener().open_path(p.to_string_lossy(), None::<&str>).map_err(err)
}

#[tauri::command]
pub async fn list_queues(st: State<'_, AppState>) -> R<Vec<Queue>> {
    Ok(st.mgr.queues())
}

#[tauri::command]
pub async fn save_queue(st: State<'_, AppState>, queue: Queue) -> R<Queue> {
    Ok(st.mgr.save_queue(queue))
}

#[tauri::command]
pub async fn delete_queue(st: State<'_, AppState>, id: String) -> R<()> {
    st.mgr.delete_queue(&id).map_err(err)
}

#[tauri::command]
pub async fn start_queue(st: State<'_, AppState>, id: String) -> R<()> {
    st.mgr.start_queue(&id);
    Ok(())
}

#[tauri::command]
pub async fn stop_queue(st: State<'_, AppState>, id: String) -> R<()> {
    st.mgr.stop_queue(&id);
    Ok(())
}

#[tauri::command]
pub async fn reorder_queue(st: State<'_, AppState>, id: String, order: Vec<String>) -> R<()> {
    st.mgr.reorder_queue(&id, order);
    Ok(())
}

#[tauri::command]
pub async fn get_settings(st: State<'_, AppState>) -> R<Settings> {
    Ok(st.mgr.settings())
}

#[tauri::command]
pub async fn default_settings() -> R<Settings> {
    Ok(Settings::default())
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, st: State<'_, AppState>, settings: Settings) -> R<()> {
    let old = st.mgr.settings();
    let mut settings = settings;
    // reject bad settings before anything changes
    st.mgr.validate_settings(&settings).map_err(err)?;
    // listen on the new port first: if it is taken nothing is saved and the
    // old listener keeps running
    if old.extension_port != settings.extension_port {
        crate::bridge::start(app.clone(), settings.extension_port).await?;
    }
    // apply launch-at-login; on failure keep the setting in line with the OS
    let mut autostart_err = None;
    if old.launch_at_login != settings.launch_at_login {
        if let Err(e) = set_autostart(&app, settings.launch_at_login) {
            settings.launch_at_login = autostart_enabled(&app).unwrap_or(old.launch_at_login);
            autostart_err = Some(e);
        }
    }
    st.mgr.save_settings(settings.clone()).map_err(err)?;
    if old.theme != settings.theme {
        windows::retheme_all(&app, &settings.theme);
    }
    if old.title_bar != settings.title_bar {
        let (a, title_bar) = (app.clone(), settings.title_bar.clone());
        let _ = app.run_on_main_thread(move || windows::reframe_all(&a, &title_bar));
    }
    match autostart_err {
        Some(e) => Err(format!("Couldn't change launch at login: {e}")),
        None => Ok(()),
    }
}

fn set_autostart(app: &AppHandle, on: bool) -> R<()> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    if on { al.enable() } else { al.disable() }.map_err(err)?;
    if on {
        quote_autostart_entry(app);
    }
    Ok(())
}

/// The launch-at-login entry the autostart plugin writes on Windows is
/// `C:\path with spaces\app.exe --minimized`, without quotes. Windows
/// resolves such a command word by word, so a program planted at
/// `C:\path.exe` or `C:\path with.exe` would be started instead. Rewrite
/// the entry with the program path quoted (only if it is exactly the entry
/// the plugin wrote for this executable).
#[cfg(windows)]
fn quote_autostart_entry(app: &AppHandle) {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let Ok(exe) = std::env::current_exe() else { return };
    let (plain, quoted) = autostart_commands(&exe.display().to_string(), crate::AUTOSTART_ARGS);
    let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run");
    let name = wide(&app.package_info().name);

    let mut buf = vec![0u16; 4096];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: key and name are NUL-terminated; `buf` is valid for `size` bytes
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc != 0 {
        return; // no entry for this user
    }
    let len = (size as usize / 2).min(buf.len());
    let current = String::from_utf16_lossy(&buf[..len]);
    if current.trim_end_matches('\0').trim() != plain {
        return;
    }
    let data = wide(&quoted);
    // SAFETY: all strings are NUL-terminated and `data` is `data.len() * 2` bytes long
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            data.as_ptr() as *const c_void,
            (data.len() * 2) as u32,
        )
    };
    if rc != 0 {
        tracing::warn!("could not quote the launch-at-login entry (error {rc})");
    }
}

#[cfg(not(windows))]
fn quote_autostart_entry(_app: &AppHandle) {}

/// The launch-at-login command as the autostart plugin writes it, and the
/// same command with the program path quoted.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn autostart_commands(exe: &str, args: &[&str]) -> (String, String) {
    let args = args.join(" ");
    (format!("{exe} {args}").trim().to_string(), format!("\"{exe}\" {args}").trim().to_string())
}

fn autostart_enabled(app: &AppHandle) -> Option<bool> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().ok()
}

/// At startup: the OS login item is the truth (the user may have removed it
/// outside the app); store what the OS says.
pub fn reconcile_autostart(app: &AppHandle) {
    let Some(os) = autostart_enabled(app) else { return };
    if os {
        // entries written by earlier versions
        quote_autostart_entry(app);
    }
    let st = app.state::<AppState>();
    let mut s = st.mgr.settings();
    if s.launch_at_login != os {
        tracing::info!("launch at login is {os} in the OS; updating the setting");
        s.launch_at_login = os;
        let _ = st.mgr.save_settings(s);
    }
}

#[tauri::command]
pub async fn recent_folders(st: State<'_, AppState>, limit: Option<usize>) -> R<Vec<String>> {
    Ok(st.mgr.recent_folders(limit.unwrap_or(4)))
}

#[tauri::command]
pub async fn totals(st: State<'_, AppState>) -> R<Totals> {
    Ok(st.mgr.totals())
}

#[tauri::command]
pub async fn target_exists(st: State<'_, AppState>, dir: String, name: String) -> R<bool> {
    Ok(st.mgr.target_exists(&dir, &name))
}

#[tauri::command]
pub async fn folder_for(st: State<'_, AppState>, category: String) -> R<String> {
    Ok(st.mgr.folder_for(&category))
}

#[tauri::command]
pub async fn expand_batch(pattern: String) -> R<Vec<String>> {
    Ok(odm_core::util::expand_batch(&pattern))
}

#[tauri::command]
pub async fn grab(
    st: State<'_, AppState>,
    url: String,
    depth: u32,
    extensions: Vec<String>,
    same_host: Option<bool>,
) -> R<Vec<GrabbedLink>> {
    let (client, s) = st.mgr.grab_client();
    let headers = odm_core::client::request_headers(&s.user_agent, None, None, &[], None, None);
    odm_core::grabber::grab(
        &client,
        &url,
        headers,
        odm_core::grabber::GrabOptions {
            depth: depth.min(3),
            max_pages: 60,
            same_host: same_host.unwrap_or(true),
            extensions,
        },
        &s.categories,
    )
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn open_window(app: AppHandle, view: String, key: Option<String>, params: Option<Value>) -> R<()> {
    let a = app.clone();
    app.run_on_main_thread(move || {
        let _ = windows::open(&a, &view, key.as_deref(), params);
    })
    .map_err(err)
}

#[tauri::command]
pub async fn take_params(st: State<'_, AppState>, token: String) -> R<Option<Value>> {
    Ok(st.pending.lock().remove(&token))
}

#[tauri::command]
pub async fn extension_status(st: State<'_, AppState>) -> R<Value> {
    let b = st.extension_browsers();
    Ok(json!({
        "connected": !b.is_empty(),
        "browsers": b,
        "port": st.mgr.settings().extension_port,
        // why the bridge isn't listening (port taken), or null
        "bridgeError": st.bridge_error.lock().clone(),
    }))
}

/// The key the browser extension is paired with (shown in the settings).
#[tauri::command]
pub async fn bridge_key(st: State<'_, AppState>) -> R<String> {
    Ok(st.mgr.bridge_key())
}

/// How long the pairing key may stay on the clipboard.
const KEY_ON_CLIPBOARD: std::time::Duration = std::time::Duration::from_secs(60);

/// Put a secret on the clipboard. On Windows it is marked so that it is
/// kept out of the clipboard history (Win+V) and is not synced to other
/// devices; elsewhere it is an ordinary copy.
fn copy_secret(app: &AppHandle, text: &str) -> R<()> {
    #[cfg(windows)]
    {
        use arboard::SetExtWindows;
        let _ = app;
        let mut clipboard = arboard::Clipboard::new().map_err(err)?;
        clipboard.set().exclude_from_monitoring().text(text.to_string()).map_err(err)
    }
    #[cfg(not(windows))]
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        app.clipboard().write_text(text.to_string()).map_err(err)
    }
}

/// Copy the pairing key for pasting into the extension. The key never
/// passes through the settings window, and it is taken off the clipboard
/// again after a minute unless something else was copied meanwhile.
#[tauri::command]
pub async fn copy_bridge_key(app: AppHandle, st: State<'_, AppState>) -> R<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let key = st.mgr.bridge_key();
    copy_secret(&app, &key)?;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(KEY_ON_CLIPBOARD).await;
        if app.clipboard().read_text().ok().as_deref() == Some(key.as_str()) {
            let _ = app.clipboard().clear();
        }
    });
    Ok(())
}

/// Replace the pairing key; paired extensions must be given the new one.
#[tauri::command]
pub async fn regenerate_bridge_key(app: AppHandle, st: State<'_, AppState>) -> R<String> {
    let key = st.mgr.regenerate_bridge_key();
    // nothing is paired any more
    st.ext_seen.lock().clear();
    let _ = app.emit("extension_status", json!({ "connected": false, "browsers": [] }));
    Ok(key)
}

#[tauri::command]
pub async fn cancel_power_action(app: AppHandle) -> R<()> {
    crate::power::cancel(&app);
    Ok(())
}

/// The running power-action countdown (`{action, seconds, queue, token}`)
/// or null — for windows that missed the `power_countdown` event.
#[tauri::command]
pub async fn get_power_countdown(app: AppHandle) -> R<Value> {
    Ok(crate::power::current(&app))
}

#[tauri::command]
pub async fn export_list(st: State<'_, AppState>, path: String, only_unfinished: bool) -> R<usize> {
    let list: Vec<String> = st
        .mgr
        .list()
        .into_iter()
        .filter(|d| !only_unfinished || d.status != Status::Complete)
        .map(|d| d.url)
        .collect();
    std::fs::write(absolute(&path)?, list.join("\n") + "\n").map_err(err)?;
    Ok(list.len())
}

/// One installed font family.
#[derive(serde::Serialize)]
pub struct FontFamily {
    name: String,
    /// every face of the family is fixed-width
    mono: bool,
}

/// The font families installed on this computer, sorted by name, for the
/// Theme studio's font fields. Scans the system font folders each time, so
/// a font installed while the app runs shows up the next time it is asked.
#[tauri::command]
pub async fn list_fonts() -> R<Vec<FontFamily>> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        // keyed by lower-case name: sorted, and one entry per family
        let mut families = std::collections::BTreeMap::<String, FontFamily>::new();
        for face in db.faces() {
            // the first family name is the English one
            let Some((name, _)) = face.families.first() else { continue };
            let name = name.trim();
            // names starting with '.' are system-internal fonts
            if name.is_empty() || name.starts_with('.') {
                continue;
            }
            families
                .entry(name.to_lowercase())
                .and_modify(|f| f.mono &= face.monospaced)
                .or_insert_with(|| FontFamily { name: name.to_string(), mono: face.monospaced });
        }
        families.into_values().collect()
    })
    .await
    .map_err(err)
}

/// Largest colour profile the Theme studio may export (its images and
/// fonts are inside it).
const MAX_THEME_FILE: usize = 96 * 1024 * 1024;

/// Save an exported colour profile. The webview can only write a JSON
/// document, and only to a `.odmtheme` file (the path comes from the
/// native save dialog).
#[tauri::command]
pub async fn write_theme_file(path: String, contents: String) -> R<()> {
    let p = absolute(&path)?;
    let is_theme = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("odmtheme"));
    if !is_theme {
        return Err("theme files end in .odmtheme".into());
    }
    if contents.len() > MAX_THEME_FILE {
        return Err("the theme is too large to export".into());
    }
    let doc: Value = serde_json::from_str(&contents).map_err(|_| "not a theme".to_string())?;
    if doc.get("format").and_then(Value::as_str) != Some("odm-theme") {
        return Err("not a theme".into());
    }
    std::fs::write(p, contents).map_err(err)
}

#[tauri::command]
pub async fn read_url_list(path: String) -> R<Vec<String>> {
    let text = std::fs::read_to_string(absolute(&path)?).map_err(err)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
        .map(String::from)
        .collect())
}

#[tauri::command]
pub async fn app_info(app: AppHandle) -> R<Value> {
    let dir = app.path().app_data_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({
        "name": "Open Download Manager",
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "dataDir": dir,
    }))
}

#[tauri::command]
pub async fn kv_get(st: State<'_, AppState>, key: String) -> R<Option<String>> {
    Ok(st.mgr.kv_get(&format!("ui.{key}")))
}

#[tauri::command]
pub async fn kv_set(st: State<'_, AppState>, key: String, value: String) -> R<()> {
    st.mgr.kv_set(&format!("ui.{key}"), &value);
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) -> R<()> {
    crate::quit_async(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn broadcast(app: AppHandle, event: String, payload: Value) -> R<()> {
    app.emit(&event, payload).map_err(err)
}

/// Folder holding the unpacked browser extensions (bundled with the app;
/// falls back to the source tree in development).
#[tauri::command]
pub async fn extension_dir(app: AppHandle) -> R<Option<String>> {
    Ok(extension_dirs(&app)
        .into_iter()
        .find(|p| p.join("chromium").exists())
        .map(|p| p.canonicalize().unwrap_or(p).to_string_lossy().into_owned()))
}

fn extension_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut cands = vec![];
    if let Ok(r) = app.path().resource_dir() {
        cands.push(r.join("extension"));
    }
    cands.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../extension/dist"));
    cands
}

#[tauri::command]
pub async fn assign_extensions(st: State<'_, AppState>, category: String, extensions: Vec<String>) -> R<()> {
    st.mgr.assign_extensions(&category, &extensions).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_entry_is_quoted() {
        let exe = r"C:\Users\Jo Smith\AppData\Local\Open Download Manager\open-download-manager.exe";
        let (plain, quoted) = autostart_commands(exe, &["--minimized"]);
        assert_eq!(plain, format!("{exe} --minimized"));
        assert_eq!(quoted, format!("\"{exe}\" --minimized"));
        let (plain, quoted) = autostart_commands(exe, &[]);
        assert_eq!(plain, exe);
        assert_eq!(quoted, format!("\"{exe}\""));
    }
}
