//! Open Download Manager for macOS: AppKit (through `objc2`) on the shared
//! Rust engine, with the list state and form logic from `odm-ui`.
// objc2 marks some AppKit methods safe and others not, and that changes from
// one release of the bindings to the next; calls are wrapped either way.
#![allow(unused_unsafe)]

#[cfg(target_os = "macos")]
mod add_window;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod charts;
#[cfg(target_os = "macos")]
mod engine;
#[cfg(target_os = "macos")]
mod grabber_window;
#[cfg(target_os = "macos")]
mod kit;
#[cfg(target_os = "macos")]
mod main_window;
#[cfg(target_os = "macos")]
mod menus;
#[cfg(target_os = "macos")]
mod progress_window;
#[cfg(target_os = "macos")]
mod scheduler_window;
#[cfg(target_os = "macos")]
mod settings_window;
#[cfg(target_os = "macos")]
mod system;

#[cfg(target_os = "macos")]
fn main() {
    // a GUI app has no console: what goes wrong is also written to a log
    // file in the data folder
    std::panic::set_hook(Box::new(|info| system::record_failure(&format!("internal error: {info}"))));
    app::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This is the macOS app. On Linux use apps/gnome or apps/plasma; on Windows apps/windows.");
}
