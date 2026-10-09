use odm_core::Manager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::Instant;

pub struct PowerAction {
    pub token: String,
    pub action: String,
}

pub struct AppState {
    pub mgr: Manager,
    /// one-time parameters for newly opened windows
    pub pending: Mutex<HashMap<String, serde_json::Value>>,
    /// browser name -> last time its extension pinged
    pub ext_seen: Mutex<HashMap<String, Instant>>,
    pub power: Mutex<Option<PowerAction>>,
    pub tray: Mutex<Option<tauri::tray::TrayIcon>>,
    pub last_complete: Mutex<Option<Instant>>,
    pub tray_state: Mutex<&'static str>,
}

impl AppState {
    pub fn new(mgr: Manager) -> Self {
        Self {
            mgr,
            pending: Mutex::new(HashMap::new()),
            ext_seen: Mutex::new(HashMap::new()),
            power: Mutex::new(None),
            tray: Mutex::new(None),
            last_complete: Mutex::new(None),
            tray_state: Mutex::new(""),
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
}
