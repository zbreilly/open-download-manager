//! Launch at login: `$XDG_CONFIG_HOME/autostart/app.opendm.Desktop.desktop`
//! running `open-download-manager --minimized` (works on Plasma and GNOME).

pub fn set(enabled: bool) -> Result<(), String> {
    let _ = enabled;
    Err("not implemented yet".into()) // STUB
}

pub fn is_enabled() -> bool {
    false // STUB
}
