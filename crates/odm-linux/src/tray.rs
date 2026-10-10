//! Tray icon (StatusNotifierItem over D-Bus): native on Plasma, and on
//! GNOME with the AppIndicator extension (Ubuntu and Fedora ship or offer
//! it). Without a tray host `Tray::spawn` returns None and the app should
//! not hide itself on close.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    AddUrl,
    Grabber,
    StartMainQueue,
    PauseAll,
    ToggleLimiter,
    Scheduler,
    Settings,
    Quit,
}

/// Icon states, matching the design's tray icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Downloading,
    Paused,
    Error,
    Complete,
}

pub struct Tray {
    _private: (),
}

impl Tray {
    /// Show the tray icon. `on_action` runs on a background thread.
    pub fn spawn(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Option<Tray> {
        let _ = on_action;
        // STUB
        None
    }

    /// Icon, and tooltip text (e.g. "2 active · 4.1 MB/s").
    pub fn set_state(&self, state: TrayState, tooltip: &str) {
        let _ = (state, tooltip);
    }

    /// Check mark of the "Speed limiter" menu item.
    pub fn set_limiter(&self, on: bool) {
        let _ = on;
    }

    /// First menu line, e.g. "2 active · 4.1 MB/s" or "No active downloads".
    pub fn set_status_line(&self, text: &str) {
        let _ = text;
    }
}
