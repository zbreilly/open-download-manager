//! Open Download Manager — Tauri 2 desktop shell.

mod bridge;
mod commands;
mod power;
mod state;
mod tray;
mod windows;

use odm_core::{Event, Manager as Engine, Status};
use serde_json::json;
use state::AppState;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

pub fn quit(app: &AppHandle) {
    app.state::<AppState>().mgr.shutdown();
    app.exit(0);
}

/// Handle a URL handed to the app (deep link, CLI argument, second instance).
fn handle_incoming(app: &AppHandle, raw: &str) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    if let Ok(u) = url::Url::parse(raw) {
        match u.scheme() {
            "odm" => {
                let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
                let action = u.host_str().unwrap_or("add");
                if action == "add" {
                    if let Some(target) = q.get("url") {
                        let _ = windows::open(
                            app,
                            "add",
                            None,
                            Some(json!({ "url": target, "referrer": q.get("referrer"), "fileName": q.get("name") })),
                        );
                    } else {
                        let _ = windows::open(app, "add", None, None);
                    }
                } else {
                    windows::show_main(app);
                }
            }
            "http" | "https" => {
                let _ = windows::open(app, "add", None, Some(json!({ "url": raw })));
            }
            _ => {}
        }
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
            for a in argv.iter().skip(1) {
                if !a.starts_with('-') {
                    handle_incoming(app, a);
                }
            }
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
            bridge::spawn(handle.clone(), engine.settings().extension_port);

            match tray::build(&handle, engine.settings().global_limit_enabled) {
                Ok(t) => *app.state::<AppState>().tray.lock() = Some(t),
                Err(e) => tracing::warn!("tray unavailable: {e}"),
            }

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
            let minimized = args.iter().any(|a| a == "--minimized");
            if !minimized {
                windows::show_main(&handle);
            }
            for a in args.iter().filter(|a| !a.starts_with('-')) {
                handle_incoming(&handle, a);
            }
            Ok(())
        })
        .on_window_event(|w, ev| {
            if let WindowEvent::CloseRequested { api, .. } = ev {
                if w.label() == "main" {
                    let to_tray = w.app_handle().state::<AppState>().mgr.settings().minimize_to_tray;
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
                if code.is_none() && app.state::<AppState>().mgr.settings().minimize_to_tray {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => {
                app.state::<AppState>().mgr.shutdown();
            }
            _ => {}
        });
}
