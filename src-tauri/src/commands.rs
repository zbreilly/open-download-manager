//! IPC commands (see HANDOFF.md §8). All async so they run on the tokio
//! runtime that the engine needs.

use crate::state::AppState;
use crate::windows;
use odm_core::manager::DownloadPatch;
use odm_core::*;
use serde::Deserialize;
use serde_json::{json, Value};
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

#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> R<()> {
    app.opener().open_path(path, None::<&str>).map_err(err)
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
    st.mgr.save_settings(settings.clone()).map_err(err)?;
    if old.launch_at_login != settings.launch_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let _ = if settings.launch_at_login { al.enable() } else { al.disable() };
    }
    if old.theme != settings.theme {
        windows::retheme_all(&app, &settings.theme);
    }
    if old.extension_port != settings.extension_port {
        crate::bridge::spawn(app.clone(), settings.extension_port);
    }
    Ok(())
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
    Ok(json!({ "connected": !b.is_empty(), "browsers": b, "port": st.mgr.settings().extension_port }))
}

#[tauri::command]
pub async fn cancel_power_action(app: AppHandle) -> R<()> {
    crate::power::cancel(&app);
    Ok(())
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
    std::fs::write(&path, list.join("\n") + "\n").map_err(err)?;
    Ok(list.len())
}

#[tauri::command]
pub async fn read_url_list(path: String) -> R<Vec<String>> {
    let text = std::fs::read_to_string(&path).map_err(err)?;
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
    crate::quit(&app);
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
    let mut cands = vec![];
    if let Ok(r) = app.path().resource_dir() {
        cands.push(r.join("extension"));
    }
    cands.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../extension/dist"));
    Ok(cands
        .into_iter()
        .find(|p| p.join("chromium").exists())
        .map(|p| p.canonicalize().unwrap_or(p).to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn assign_extensions(st: State<'_, AppState>, category: String, extensions: Vec<String>) -> R<()> {
    st.mgr.assign_extensions(&category, &extensions).map_err(err)
}
