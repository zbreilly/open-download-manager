//! "When the queue finishes": sleep / shut down / exit, after a
//! cancelable 60-second countdown shown in the main window.
//!
//! While counting down, a `power_countdown` event
//! `{action, seconds, queue, token}` is emitted every second with the
//! remaining seconds (the main window may not be listening yet when the
//! countdown starts, e.g. after `--minimized`), and `null` once it is
//! cancelled or done. `get_power_countdown` returns the same payload or null.

use crate::state::{AppState, PowerAction};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const COUNTDOWN_SECS: u64 = 60;

fn payload(pa: &PowerAction) -> Value {
    let left = pa.deadline.saturating_duration_since(Instant::now());
    let secs = left.as_millis().div_ceil(1000) as u64;
    json!({ "action": pa.action, "seconds": secs, "queue": pa.queue, "token": pa.token })
}

/// Current countdown (same shape as the `power_countdown` event) or null.
pub fn current(app: &AppHandle) -> Value {
    match app.state::<AppState>().power.lock().as_ref() {
        Some(pa) => payload(pa),
        None => Value::Null,
    }
}

pub fn schedule(app: &AppHandle, action: &str, queue_name: &str) {
    if !matches!(action, "sleep" | "shutdown" | "exit") {
        return;
    }
    let token = uuid::Uuid::new_v4().to_string();
    let pa = PowerAction {
        token: token.clone(),
        action: action.into(),
        queue: queue_name.into(),
        deadline: Instant::now() + Duration::from_secs(COUNTDOWN_SECS),
    };
    let first = payload(&pa);
    *app.state::<AppState>().power.lock() = Some(pa);
    crate::windows::show_main(app);
    let _ = app.emit("power_countdown", first);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        enum Step {
            Tick(Value),
            Run(String),
            Stop,
        }
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let step = {
                let st = app.state::<AppState>();
                let mut p = st.power.lock();
                match p.as_ref() {
                    Some(pa) if pa.token == token => {
                        if pa.deadline <= Instant::now() {
                            p.take().map(|pa| Step::Run(pa.action)).unwrap_or(Step::Stop)
                        } else {
                            Step::Tick(payload(pa))
                        }
                    }
                    // cancelled or replaced by a newer countdown
                    _ => Step::Stop,
                }
            };
            match step {
                Step::Tick(v) => {
                    let _ = app.emit("power_countdown", v);
                }
                Step::Run(a) => {
                    let _ = app.emit("power_countdown", Value::Null);
                    run(&app, &a).await;
                    break;
                }
                Step::Stop => break,
            }
        }
    });
}

pub fn cancel(app: &AppHandle) {
    app.state::<AppState>().power.lock().take();
    let _ = app.emit("power_countdown", json!(null));
}

async fn run(app: &AppHandle, action: &str) {
    match action {
        "exit" => crate::quit_async(app).await,
        "sleep" => {
            // an explicit sleep wins over our own keep-awake request
            app.state::<AppState>().awake.set(false);
            std::thread::spawn(sleep_now);
        }
        "shutdown" => {
            crate::shutdown_engine(app).await;
            if let Err(e) = shutdown_cmd().spawn() {
                tracing::warn!("shutdown failed: {e}");
            }
            app.exit(0);
        }
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn sleep_now() {
    if let Err(e) = std::process::Command::new("pmset").arg("sleepnow").spawn() {
        tracing::warn!("sleep failed: {e}");
    }
}
#[cfg(target_os = "macos")]
fn shutdown_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("osascript");
    c.args(["-e", "tell application \"System Events\" to shut down"]);
    c
}

#[cfg(windows)]
fn sleep_now() {
    // SetSuspendState(hibernate = FALSE): sleep, not hibernate
    win::enable_shutdown_privilege();
    // SAFETY: plain Win32 call without pointers
    let ok = unsafe { windows_sys::Win32::System::Power::SetSuspendState(false, false, false) };
    if !ok {
        tracing::warn!("sleep failed: {}", std::io::Error::last_os_error());
    }
}
#[cfg(windows)]
fn shutdown_cmd() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("shutdown");
    c.args(["/s", "/t", "0"]);
    c.creation_flags(win::CREATE_NO_WINDOW);
    c
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub const CREATE_NO_WINDOW: u32 = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    /// SetSuspendState needs SeShutdownPrivilege enabled in the process token.
    pub fn enable_shutdown_privilege() {
        let name: Vec<u16> = "SeShutdownPrivilege\0".encode_utf16().collect();
        // SAFETY: all pointers refer to live locals; the token handle is closed
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
                return;
            }
            let mut luid = LUID { LowPart: 0, HighPart: 0 };
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) != 0 {
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                AdjustTokenPrivileges(token, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut());
            }
            CloseHandle(token);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn sleep_now() {
    if let Err(e) = std::process::Command::new("systemctl").arg("suspend").spawn() {
        tracing::warn!("sleep failed: {e}");
    }
}
#[cfg(all(unix, not(target_os = "macos")))]
fn shutdown_cmd() -> std::process::Command {
    let mut c = std::process::Command::new("systemctl");
    c.arg("poweroff");
    c
}
