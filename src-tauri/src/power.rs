//! "When the queue finishes": sleep / shut down / exit, after a
//! cancelable 60-second countdown shown in the main window.

use crate::state::{AppState, PowerAction};
use serde_json::json;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub const COUNTDOWN_SECS: u64 = 60;

pub fn schedule(app: &AppHandle, action: &str, queue_name: &str) {
    if !matches!(action, "sleep" | "shutdown" | "exit") {
        return;
    }
    let token = uuid::Uuid::new_v4().to_string();
    *app.state::<AppState>().power.lock() = Some(PowerAction { token: token.clone(), action: action.into() });
    crate::windows::show_main(app);
    let _ = app.emit(
        "power_countdown",
        json!({ "action": action, "seconds": COUNTDOWN_SECS, "queue": queue_name, "token": token }),
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(COUNTDOWN_SECS)).await;
        let act = {
            let st = app.state::<AppState>();
            let mut p = st.power.lock();
            match p.as_ref() {
                Some(pa) if pa.token == token => p.take().map(|pa| pa.action),
                _ => None,
            }
        };
        if let Some(a) = act {
            run(&app, &a);
        }
    });
}

pub fn cancel(app: &AppHandle) {
    app.state::<AppState>().power.lock().take();
    let _ = app.emit("power_countdown", json!(null));
}

fn run(app: &AppHandle, action: &str) {
    match action {
        "exit" => crate::quit(app),
        "sleep" => {
            let _ = sleep_cmd().spawn();
        }
        "shutdown" => {
            app.state::<AppState>().mgr.shutdown();
            let _ = shutdown_cmd().spawn();
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn sleep_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("pmset");
    c.arg("sleepnow");
    c
}
#[cfg(target_os = "macos")]
fn shutdown_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("osascript");
    c.args(["-e", "tell application \"System Events\" to shut down"]);
    c
}

#[cfg(windows)]
fn sleep_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("rundll32.exe");
    c.args(["powrprof.dll,SetSuspendState", "0,1,0"]);
    c
}
#[cfg(windows)]
fn shutdown_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("shutdown");
    c.args(["/s", "/t", "0"]);
    c
}

#[cfg(all(unix, not(target_os = "macos")))]
fn sleep_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("systemctl");
    c.arg("suspend");
    c
}
#[cfg(all(unix, not(target_os = "macos")))]
fn shutdown_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("systemctl");
    c.arg("poweroff");
    c
}
