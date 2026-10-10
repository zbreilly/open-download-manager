use crate::awake::Awake;
use odm_core::Manager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub struct PowerAction {
    pub token: String,
    pub action: String,
    pub queue: String,
    pub deadline: Instant,
}

/// The running extension bridge listener.
pub struct BridgeHandle {
    /// dropping or sending stops the listener (graceful shutdown)
    pub stop: tokio::sync::oneshot::Sender<()>,
}

pub struct AppState {
    pub mgr: Manager,
    /// one-time parameters for newly opened windows
    pub pending: Mutex<HashMap<String, serde_json::Value>>,
    /// browser name -> last time its extension pinged
    pub ext_seen: Mutex<HashMap<String, Instant>>,
    pub power: Mutex<Option<PowerAction>>,
    pub tray: Mutex<Option<tauri::tray::TrayIcon>>,
    /// the tray's "Speed limiter" check item, kept in sync with the settings
    pub limit_item: Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>,
    pub last_complete: Mutex<Option<Instant>>,
    pub tray_state: Mutex<&'static str>,
    pub bridge: Mutex<Option<BridgeHandle>>,
    /// last bind failure of the extension bridge (None while it listens)
    pub bridge_error: Mutex<Option<String>>,
    /// URLs handed to the app recently (deep link / argv), to handle each once
    pub recent_incoming: Mutex<HashMap<String, Instant>>,
    /// set when shutting the engine down for quitting starts
    pub shut_down: AtomicBool,
    /// set (and waiters woken) when that shutdown has finished
    pub shut_down_done: (std::sync::Mutex<bool>, std::sync::Condvar),
    pub awake: Awake,
}

impl AppState {
    pub fn new(mgr: Manager) -> Self {
        Self {
            mgr,
            pending: Mutex::new(HashMap::new()),
            ext_seen: Mutex::new(HashMap::new()),
            power: Mutex::new(None),
            tray: Mutex::new(None),
            limit_item: Mutex::new(None),
            last_complete: Mutex::new(None),
            tray_state: Mutex::new(""),
            bridge: Mutex::new(None),
            bridge_error: Mutex::new(None),
            recent_incoming: Mutex::new(HashMap::new()),
            shut_down: AtomicBool::new(false),
            shut_down_done: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
            awake: Awake::new(),
        }
    }

    pub fn extension_browsers(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ext_seen
            .lock()
            .iter()
            .filter(|(_, t)| t.elapsed().as_secs() < 150)
            .map(|(b, _)| b.clone())
            .collect();
        v.sort();
        v
    }

    pub fn has_tray(&self) -> bool {
        self.tray.lock().is_some()
    }
}
