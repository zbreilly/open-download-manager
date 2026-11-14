//! Open Download Manager for Windows: WinUI 3 (through `windows-reactor`)
//! on the shared Rust engine, with the list state and form logic from
//! `odm-ui`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod add_window;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod grabber_window;
#[cfg(windows)]
mod main_window;
#[cfg(windows)]
mod progress_window;
#[cfg(windows)]
mod scheduler_window;
#[cfg(windows)]
mod settings_window;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod widgets;

#[cfg(windows)]
fn main() {
    // a GUI app has no console: what goes wrong at startup is written to a
    // log file and shown in a message box
    std::panic::set_hook(Box::new(|info| system::startup_failure(&format!("internal error: {info}"))));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // a second launch (an odm:// link, the shortcut again) hands its
    // arguments to the running instance and exits
    if !system::single_instance(&args) {
        return;
    }
    if let Err(e) = app::run(args) {
        system::startup_failure(&e);
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This is the Windows app. On Linux use apps/gnome or apps/plasma; on macOS apps/macos.");
}
