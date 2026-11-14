//! `open-download-manager`: starts the front end that matches the desktop.
//!
//! Plasma gets the Kirigami app, everything else the GTK app. Override with
//! `--ui=gnome|plasma` or `ODM_UI=gnome|plasma`. If the preferred front end
//! isn't installed the other one is started.
use odm_linux::desktop::{self, Desktop};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

fn libexec_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(d) = std::env::var_os("ODM_LIBEXEC") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            // installed: /usr/bin → /usr/libexec/open-download-manager
            dirs.push(bin.join("../libexec/open-download-manager"));
            // a cargo target folder
            dirs.push(bin.to_path_buf());
        }
    }
    dirs.push(PathBuf::from("/usr/libexec/open-download-manager"));
    dirs
}

fn find(ui: &str) -> Option<PathBuf> {
    let name = format!("odm-{ui}");
    libexec_dirs().into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

fn main() {
    let mut wanted = std::env::var("ODM_UI").ok();
    let mut rest = vec![];
    for a in std::env::args().skip(1) {
        match a.strip_prefix("--ui=") {
            Some(v) => wanted = Some(v.to_string()),
            None => rest.push(a),
        }
    }
    let first = match wanted.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("plasma") | Some("kde") | Some("qt") => "plasma",
        Some("gnome") | Some("gtk") => "gnome",
        _ if desktop::current() == Desktop::Plasma => "plasma",
        _ => "gnome",
    };
    let second = if first == "plasma" { "gnome" } else { "plasma" };
    let Some(exe) = find(first).or_else(|| find(second)) else {
        eprintln!("open-download-manager: no front end found (looked for odm-{first} and odm-{second})");
        std::process::exit(127);
    };
    let err = Command::new(&exe).args(rest).exec();
    eprintln!("open-download-manager: could not start {}: {err}", exe.display());
    std::process::exit(126);
}
