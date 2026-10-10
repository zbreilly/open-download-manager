//! Open Download Manager — Tauri 2 desktop shell.

mod awake;
mod bridge;
mod commands;
mod power;
mod state;
mod tray;
mod windows;

use odm_core::{Event, Manager as Engine, Status};
use serde_json::json;
use state::AppState;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

/// How long quitting waits for downloads to stop and flush.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Stop the bridge, release keep-awake and shut the engine down, waiting
/// (bounded) for downloads to flush. Runs once; a call made while another
/// is in progress waits for it to finish.
pub async fn shutdown_engine(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.shut_down.swap(true, Ordering::SeqCst) {
        // someone else is shutting down: wait (bounded) until they're done
        let until = Instant::now() + SHUTDOWN_TIMEOUT + Duration::from_secs(2);
        while !shutdown_finished(&st) && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        return;
    }
    bridge::stop(app);
    st.awake.set(false);
    let mgr = st.mgr.clone();
    if tokio::time::timeout(SHUTDOWN_TIMEOUT + Duration::from_secs(1), mgr.shutdown_wait(SHUTDOWN_TIMEOUT))
        .await
        .is_err()
    {
        tracing::warn!("engine shutdown timed out");
    }
    let (lock, cv) = &st.shut_down_done;
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
    cv.notify_all();
}

fn shutdown_finished(st: &AppState) -> bool {
    *st.shut_down_done.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Blocking variant for the run loop's Exit event. If quit() already
/// finished this returns at once; if it is still flushing (e.g. the OS
/// ends the session mid-quit) this waits for it.
fn shutdown_engine_blocking(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.shut_down.load(Ordering::SeqCst) {
        let (lock, cv) = &st.shut_down_done;
        let g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cv.wait_timeout_while(g, SHUTDOWN_TIMEOUT + Duration::from_secs(2), |done| !*done);
        return;
    }
    if tokio::runtime::Handle::try_current().is_ok() {
        // inside the async runtime blocking would panic: wait on a helper
        // thread instead
        let a = app.clone();
        let _ = std::thread::spawn(move || tauri::async_runtime::block_on(shutdown_engine(&a))).join();
    } else {
        tauri::async_runtime::block_on(shutdown_engine(app));
    }
}

/// Quit from synchronous code (tray menu). The shutdown runs on the async
/// runtime so the main thread keeps serving the tray/windows meanwhile.
pub fn quit(app: &AppHandle) {
    let a = app.clone();
    tauri::async_runtime::spawn(async move { quit_async(&a).await });
}

/// Quit from async code (IPC command, power action).
pub async fn quit_async(app: &AppHandle) {
    shutdown_engine(app).await;
    app.exit(0);
}

/// URLs seen within this window are the same hand-off arriving twice (e.g.
/// the deep-link plugin and the single-instance argv on Windows/Linux).
const INCOMING_DEDUPE: Duration = Duration::from_secs(3);

fn first_time(app: &AppHandle, raw: &str) -> bool {
    let st = app.state::<AppState>();
    let mut m = st.recent_incoming.lock();
    m.retain(|_, t| t.elapsed() < INCOMING_DEDUPE);
    if m.contains_key(raw) {
        return false;
    }
    m.insert(raw.to_string(), Instant::now());
    true
}

fn is_http(u: &url::Url) -> bool {
    matches!(u.scheme(), "http" | "https")
}

/// Handle a URL handed to the app (deep link, CLI argument, second
/// instance). Links from outside the app open the Add window with
/// `source: "web"`; only the URL itself is taken from them.
fn handle_incoming(app: &AppHandle, raw: &str) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let Ok(u) = url::Url::parse(raw) else { return };
    if !matches!(u.scheme(), "odm" | "http" | "https") {
        return;
    }
    if !first_time(app, u.as_str()) {
        tracing::debug!("incoming {} link already handled", u.scheme());
        return;
    }
    tracing::debug!("incoming {} link", u.scheme());
    match u.scheme() {
        "odm" => {
            let action = u.host_str().unwrap_or("add");
            if action == "add" {
                let target = u
                    .query_pairs()
                    .find(|(k, _)| k == "url")
                    .map(|(_, v)| v.trim().to_string())
                    .filter(|t| url::Url::parse(t).map(|t| is_http(&t)).unwrap_or(false));
                let params = match target {
                    Some(t) => json!({ "url": t, "source": "web" }),
                    None => json!({ "source": "web" }),
                };
                let _ = windows::open(app, "add", None, Some(params));
            } else {
                windows::show_main(app);
            }
        }
        _ => {
            let _ = windows::open(app, "add", None, Some(json!({ "url": raw, "source": "web" })));
        }
    }
}

/// Handle command-line arguments (first launch or a second instance).
/// On Windows/Linux an `odm:` link passed as the only argument is a deep
/// link: the deep-link plugin delivers it (on_open_url / get_current), so it
/// is skipped here.
fn handle_args(app: &AppHandle, args: &[String]) {
    let only_one = args.len() == 1;
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        let deep_link = a.trim().to_ascii_lowercase().starts_with("odm:");
        if deep_link && only_one && cfg!(any(windows, target_os = "linux")) {
            continue;
        }
        handle_incoming(app, a);
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn forward_events(app: AppHandle, engine: Engine) {
    let mut rx = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            match ev {
                Event::Added(d) => {
                    let _ = app.emit("download_added", d);
                }
                Event::Changed(d) => {
                    let _ = app.emit("download_changed", d);
                }
                Event::Removed(id) => {
                    let _ = app.emit("download_removed", id);
                }
                Event::Progress(items) => {
                    let _ = app.emit("progress_batch", items);
                }
                Event::Totals(t) => {
                    tray::update(&app, &t);
                    // keep the system awake while anything downloads
                    app.state::<AppState>().awake.set(t.active > 0);
                    let st = app.state::<AppState>();
                    let b = st.extension_browsers();
                    let _ = app.emit("totals", t);
                    let _ = app.emit("extension_status", json!({ "connected": !b.is_empty(), "browsers": b }));
                }
                Event::Completed(d) => {
                    *app.state::<AppState>().last_complete.lock() = Some(Instant::now());
                    let s = engine.settings();
                    let path = d.final_path();
                    match d.when_finished.as_str() {
                        "openFile" => {
                            let _ = app.opener().open_path(path.to_string_lossy(), None::<&str>);
                        }
                        "openFolder" => {
                            let _ = app.opener().reveal_item_in_dir(&path);
                        }
                        "none" => {}
                        _ => {
                            if s.notify_complete {
                                notify(&app, "Download complete", &d.file_name);
                            }
                        }
                    }
                    if s.show_complete_dialog {
                        let a = app.clone();
                        let id = d.id.clone();
                        let _ = app.run_on_main_thread(move || {
                            let _ = windows::open(&a, "progress", Some(&id), None);
                        });
                    }
                }
                Event::Failed(d) => {
                    if engine.settings().notify_failure {
                        let why = d.error.clone().unwrap_or_else(|| "unknown error".into());
                        notify(&app, &format!("Download failed: {}", d.file_name), &why);
                    }
                }
                Event::QueueFinished(q) => {
                    if engine.settings().notify_queue_finished {
                        notify(&app, "Queue finished", &format!("{} has no more files to download", q.name));
                    }
                    let a = app.clone();
                    let action = q.on_finish.clone();
                    let name = q.name.clone();
                    let _ = app.run_on_main_thread(move || power::schedule(&a, &action, &name));
                }
                Event::QueuesChanged(qs) => {
                    let _ = app.emit("queues_changed", qs);
                }
                Event::SettingsChanged(s) => {
                    tray::sync_limit(&app, s.global_limit_enabled);
                    let _ = app.emit("settings_changed", s);
                }
            }
        }
    });
}

fn clipboard_monitor(app: AppHandle, engine: Engine) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    tauri::async_runtime::spawn(async move {
        let mut last = app.clipboard().read_text().unwrap_or_default();
        loop {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let s = engine.settings();
            if !s.clipboard_monitor {
                continue;
            }
            let Ok(text) = app.clipboard().read_text() else { continue };
            if text == last {
                continue;
            }
            last = text.clone();
            let t = text.trim();
            if t.contains(char::is_whitespace) || t.len() > 4096 {
                continue;
            }
            let Ok(u) = url::Url::parse(t) else { continue };
            if !matches!(u.scheme(), "http" | "https") {
                continue;
            }
            let ext = odm_core::util::extension(u.path());
            if ext.is_empty() || !s.capture_extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                continue;
            }
            // don't re-offer something already in the list
            if engine.list().iter().any(|d| d.url == t && d.status != Status::Error) {
                continue;
            }
            let a = app.clone();
            let url = t.to_string();
            let _ = app.run_on_main_thread(move || {
                let _ = windows::open(&a, "add", None, Some(json!({ "url": url, "fromClipboard": true })));
            });
        }
    });
}

pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,odm_core=info".into()),
        )
        .try_init();

    let mut builder = tauri::Builder::default();

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            windows::show_main(app);
            handle_args(app, argv.get(1..).unwrap_or_default());
        }));
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let engine = Engine::open(&data_dir.join("odm.db")).map_err(|e| e.to_string())?;
            app.manage(AppState::new(engine.clone()));

            let e2 = engine.clone();
            tauri::async_runtime::block_on(async move { e2.start_background() });

            forward_events(handle.clone(), engine.clone());
            clipboard_monitor(handle.clone(), engine.clone());
            let port = engine.settings().extension_port;
            let h2 = handle.clone();
            // a bind error is kept for extension_status and emitted as bridge_error
            let _ = tauri::async_runtime::block_on(bridge::start(h2, port));

            match tray::build(&handle, engine.settings().global_limit_enabled) {
                Ok((t, limit)) => {
                    *app.state::<AppState>().tray.lock() = Some(t);
                    *app.state::<AppState>().limit_item.lock() = Some(limit);
                }
                Err(e) => tracing::warn!("tray unavailable: {e}"),
            }

            commands::reconcile_autostart(&handle);

            {
                use tauri_plugin_deep_link::DeepLinkExt;
                #[cfg(any(windows, target_os = "linux"))]
                let _ = app.deep_link().register_all();
                let h = handle.clone();
                app.deep_link().on_open_url(move |ev| {
                    for u in ev.urls() {
                        handle_incoming(&h, u.as_str());
                    }
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for u in urls {
                        handle_incoming(&handle, u.as_str());
                    }
                }
            }

            let args: Vec<String> = std::env::args().skip(1).collect();
            // without a tray icon a minimized start would leave no way back in
            let minimized = args.iter().any(|a| a == "--minimized") && app.state::<AppState>().has_tray();
            if !minimized {
                windows::show_main(&handle);
            }
            handle_args(&handle, &args);
            Ok(())
        })
        .on_window_event(|w, ev| {
            if let WindowEvent::CloseRequested { api, .. } = ev {
                if w.label() == "main" {
                    // hide to the tray only when there is one to come back from
                    let st = w.app_handle().state::<AppState>();
                    let to_tray = st.mgr.settings().minimize_to_tray && st.has_tray();
                    if to_tray {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_downloads,
            commands::get_download,
            commands::probe_url,
            commands::add_download,
            commands::add_many,
            commands::pause,
            commands::pause_all,
            commands::resume,
            commands::delete,
            commands::delete_completed,
            commands::redownload,
            commands::refresh_link,
            commands::set_global_limit,
            commands::set_download_limit,
            commands::update_download,
            commands::set_category,
            commands::move_to_queue,
            commands::open_file,
            commands::open_folder,
            commands::open_path,
            commands::list_queues,
            commands::save_queue,
            commands::delete_queue,
            commands::start_queue,
            commands::stop_queue,
            commands::reorder_queue,
            commands::get_settings,
            commands::default_settings,
            commands::save_settings,
            commands::recent_folders,
            commands::totals,
            commands::target_exists,
            commands::folder_for,
            commands::expand_batch,
            commands::grab,
            commands::open_window,
            commands::take_params,
            commands::extension_status,
            commands::cancel_power_action,
            commands::get_power_countdown,
            commands::export_list,
            commands::read_url_list,
            commands::app_info,
            commands::kv_get,
            commands::kv_set,
            commands::quit_app,
            commands::broadcast,
            commands::extension_dir,
            commands::assign_extensions,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Open Download Manager")
        .run(|app, ev| match ev {
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => windows::show_main(app),
            RunEvent::ExitRequested { api, code, .. } => {
                // closing the last window keeps the app alive in the tray;
                // an explicit quit (code is Some) exits
                let st = app.state::<AppState>();
                if code.is_none() && st.mgr.settings().minimize_to_tray && st.has_tray() {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => {
                // no-op when quit()/quit_async() already did it
                shutdown_engine_blocking(app);
            }
            _ => {}
        });
}
