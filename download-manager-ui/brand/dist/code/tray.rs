// Tray icon state switching for Tauri 2.
// Copy tray/windows-linux/*.png and tray/macos/*Template*.png into src-tauri/icons/tray/.
use tauri::{image::Image, tray::TrayIcon, Runtime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayState { Idle, Downloading, Paused, Error, Complete, Limited }

impl TrayState {
    fn key(self) -> &'static str {
        match self {
            TrayState::Idle => "idle",
            TrayState::Downloading => "downloading",
            TrayState::Paused => "paused",
            TrayState::Error => "error",
            TrayState::Complete => "complete",
            TrayState::Limited => "limited",
        }
    }
}

/// Priority when several apply: Error > Downloading > Limited (while active) > Paused > Complete (5 s) > Idle.
pub fn set_tray_state<R: Runtime>(tray: &TrayIcon<R>, state: TrayState) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = match state {
        TrayState::Idle => include_bytes!("../icons/tray/tray-idleTemplate@2x.png"),
        TrayState::Downloading => include_bytes!("../icons/tray/tray-downloadingTemplate@2x.png"),
        TrayState::Paused => include_bytes!("../icons/tray/tray-pausedTemplate@2x.png"),
        TrayState::Error => include_bytes!("../icons/tray/tray-errorTemplate@2x.png"),
        TrayState::Complete => include_bytes!("../icons/tray/tray-completeTemplate@2x.png"),
        TrayState::Limited => include_bytes!("../icons/tray/tray-limitedTemplate@2x.png"),
    };
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = match state {
        TrayState::Idle => include_bytes!("../icons/tray/tray-idle-32.png"),
        TrayState::Downloading => include_bytes!("../icons/tray/tray-downloading-32.png"),
        TrayState::Paused => include_bytes!("../icons/tray/tray-paused-32.png"),
        TrayState::Error => include_bytes!("../icons/tray/tray-error-32.png"),
        TrayState::Complete => include_bytes!("../icons/tray/tray-complete-32.png"),
        TrayState::Limited => include_bytes!("../icons/tray/tray-limited-32.png"),
    };
    tray.set_icon(Some(Image::from_bytes(bytes)?))?;
    #[cfg(target_os = "macos")]
    tray.set_icon_as_template(true)?;
    let _ = state.key();
    Ok(())
}
