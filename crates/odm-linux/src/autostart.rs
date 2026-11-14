//! Launch at login: `$XDG_CONFIG_HOME/autostart/app.opendm.Desktop.desktop`
//! running `open-download-manager --minimized` (works on Plasma and GNOME).
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join(format!("{}.desktop", crate::APP_ID)))
}

pub fn set(enabled: bool) -> Result<(), String> {
    let f = file().ok_or("no home folder")?;
    if !enabled {
        return match std::fs::remove_file(&f) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not remove {}: {e}", f.display())),
        };
    }
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let text = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=open-download-manager --minimized\nIcon={}\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
        crate::APP_NAME,
        crate::APP_ID
    );
    std::fs::write(&f, text).map_err(|e| format!("could not write {}: {e}", f.display()))
}

pub fn is_enabled() -> bool {
    file().map(|f| f.exists()).unwrap_or(false)
}
