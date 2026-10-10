//! Where the app keeps its data.
use std::path::PathBuf;

/// `$XDG_DATA_HOME/app.opendm.desktop` (normally
/// `~/.local/share/app.opendm.desktop`). The same folder the Tauri build
/// uses, so the download list carries over when switching.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("app.opendm.desktop")
}
