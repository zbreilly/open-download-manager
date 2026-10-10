//! Open Download Manager for Windows: WinUI 3 (through `windows-reactor`)
//! on the shared Rust engine, with the list state and form logic from
//! `odm-ui`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod engine;

#[cfg(windows)]
fn main() {
    if let Err(e) = app::run() {
        eprintln!("open-download-manager: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This is the Windows app. On Linux use apps/gnome or apps/plasma; on macOS apps/macos.");
}
