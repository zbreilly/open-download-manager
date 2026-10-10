//! Open files, folders and links with the user's default apps.
use std::path::Path;
use std::process::{Command, Stdio};

fn xdg_open(arg: &std::ffi::OsStr) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| {
            // reap it without blocking the caller
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        })
        .map_err(|e| format!("xdg-open: {e}"))
}

/// Open a file with its default application.
pub fn open_path(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err("The file is no longer there. It may have been moved or deleted.".into());
    }
    xdg_open(p.as_os_str())
}

/// Show a file selected in the file manager (org.freedesktop.FileManager1
/// ShowItems: Dolphin, Files/Nautilus), falling back to opening its folder.
pub fn reveal(p: &Path) -> Result<(), String> {
    if p.exists() {
        if let Ok(uri) = url::Url::from_file_path(p) {
            let shown = zbus::blocking::Connection::session().and_then(|c| {
                c.call_method(
                    Some("org.freedesktop.FileManager1"),
                    "/org/freedesktop/FileManager1",
                    Some("org.freedesktop.FileManager1"),
                    "ShowItems",
                    &(vec![uri.as_str()], ""),
                )
            });
            if shown.is_ok() {
                return Ok(());
            }
        }
    }
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    xdg_open(dir.as_os_str())
}

/// Open a web link in the default browser.
pub fn open_url(u: &str) -> Result<(), String> {
    let parsed = url::Url::parse(u.trim()).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http(s) links can be opened".into());
    }
    xdg_open(parsed.as_str().as_ref())
}
