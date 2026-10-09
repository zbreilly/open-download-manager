//! System tray / menu-bar icon with activity states.

use crate::state::AppState;
use crate::windows;
use odm_core::Totals;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

fn icon_bytes(state: &str) -> &'static [u8] {
    #[cfg(target_os = "macos")]
    {
        match state {
            "downloading" => include_bytes!("../icons/tray/tray-downloadingTemplate@2x.png"),
            "paused" => include_bytes!("../icons/tray/tray-pausedTemplate@2x.png"),
            "error" => include_bytes!("../icons/tray/tray-errorTemplate@2x.png"),
            "complete" => include_bytes!("../icons/tray/tray-completeTemplate@2x.png"),
            "limited" => include_bytes!("../icons/tray/tray-limitedTemplate@2x.png"),
            _ => include_bytes!("../icons/tray/tray-idleTemplate@2x.png"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match state {
            "downloading" => include_bytes!("../icons/tray/tray-downloading-32.png"),
            "paused" => include_bytes!("../icons/tray/tray-paused-32.png"),
            "error" => include_bytes!("../icons/tray/tray-error-32.png"),
            "complete" => include_bytes!("../icons/tray/tray-complete-32.png"),
            "limited" => include_bytes!("../icons/tray/tray-limited-32.png"),
            _ => include_bytes!("../icons/tray/tray-idle-32.png"),
        }
    }
}

pub fn build(app: &AppHandle, limit_on: bool) -> tauri::Result<TrayIcon> {
    let show = MenuItem::with_id(app, "show", "Show Open Download Manager", true, None::<&str>)?;
    let add = MenuItem::with_id(app, "add", "Add URL…", true, None::<&str>)?;
    let grab = MenuItem::with_id(app, "grabber", "Site grabber…", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let start_q = MenuItem::with_id(app, "start_main", "Start main queue", true, None::<&str>)?;
    let pause_all = MenuItem::with_id(app, "pause_all", "Pause all", true, None::<&str>)?;
    let limit = CheckMenuItem::with_id(app, "limit", "Speed limiter", true, limit_on, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sched = MenuItem::with_id(app, "scheduler", "Scheduler…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&show, &add, &grab, &sep1, &start_q, &pause_all, &limit, &sep2, &sched, &settings, &sep3, &quit],
    )?;

    let tray = TrayIconBuilder::with_id("odm-tray")
        .icon(Image::from_bytes(icon_bytes("idle"))?)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Open Download Manager")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, ev| {
            let app = app.app_handle();
            match ev.id().as_ref() {
                "show" => windows_show(app),
                "add" => open_any(app, "add"),
                "grabber" => open_any(app, "grabber"),
                "scheduler" => open_any(app, "scheduler"),
                "settings" => open_any(app, "settings"),
                "start_main" => with_mgr(app, |m| m.start_queue(odm_core::manager::MAIN_QUEUE)),
                "pause_all" => with_mgr(app, |m| m.pause_all()),
                "limit" => with_mgr(app, |m| {
                    let on = m.settings().global_limit_enabled;
                    m.set_global_limit(if on { None } else { Some(0) });
                }),
                "quit" => crate::quit(app),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                if !cfg!(target_os = "macos") {
                    windows_show(tray.app_handle());
                }
            }
        })
        .build(app)?;
    Ok(tray)
}

fn with_mgr(app: &AppHandle, f: impl FnOnce(&odm_core::Manager) + Send + 'static) {
    let m = app.state::<AppState>().mgr.clone();
    tauri::async_runtime::spawn(async move { f(&m) });
}

fn windows_show(app: &AppHandle) {
    windows::show_main(app);
}

fn open_any(app: &AppHandle, view: &str) {
    let _ = windows::open(app, view, None, None);
}

/// Update icon/tooltip from the latest totals.
pub fn update(app: &AppHandle, t: &Totals) {
    let st = app.state::<AppState>();
    let recent_complete = st
        .last_complete
        .lock()
        .map(|i| i.elapsed().as_secs() < 5)
        .unwrap_or(false);
    let state: &'static str = if t.failed > 0 && t.active == 0 && !recent_complete {
        "error"
    } else if t.active > 0 && t.limit_on {
        "limited"
    } else if t.active > 0 {
        "downloading"
    } else if recent_complete {
        "complete"
    } else if t.paused > 0 {
        "paused"
    } else {
        "idle"
    };
    let tray = st.tray.lock().clone();
    let Some(tray) = tray else { return };
    {
        let mut cur = st.tray_state.lock();
        if *cur != state {
            *cur = state;
            if let Ok(img) = Image::from_bytes(icon_bytes(state)) {
                let _ = tray.set_icon(Some(img));
                #[cfg(target_os = "macos")]
                let _ = tray.set_icon_as_template(true);
            }
        }
    }
    let speed = fmt_speed(t.speed_bps);
    let tip = if t.active > 0 {
        format!("Open Download Manager — {} active · {speed}", t.active)
    } else {
        "Open Download Manager".to_string()
    };
    let _ = tray.set_tooltip(Some(tip));
    #[cfg(target_os = "macos")]
    {
        let show = st.mgr.settings().tray_speed && t.active > 0;
        let _ = tray.set_title(if show { Some(speed) } else { None::<String> });
    }
}

pub fn fmt_speed(bps: u64) -> String {
    let b = bps as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB/s", b / 1024f64.powi(3))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", b / 1024f64.powi(2))
    } else {
        format!("{:.1} KB/s", b / 1024.0)
    }
}
