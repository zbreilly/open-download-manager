//! Open files, folders and links with the user's default apps.
use std::path::Path;

/// Open a file with its default application.
pub fn open_path(p: &Path) -> Result<(), String> {
    let _ = p;
    Err("not implemented yet".into()) // STUB
}

/// Show a file selected in the file manager (org.freedesktop.FileManager1
/// ShowItems: Dolphin, Files/Nautilus), falling back to opening its folder.
pub fn reveal(p: &Path) -> Result<(), String> {
    let _ = p;
    Err("not implemented yet".into()) // STUB
}

/// Open a web link in the default browser.
pub fn open_url(u: &str) -> Result<(), String> {
    let _ = u;
    Err("not implemented yet".into()) // STUB
}
