//! Desktop services shared by the native Linux front ends (`apps/gnome`,
//! GTK 4 + libadwaita, and `apps/plasma`, Kirigami/Qt 6). Everything here
//! is toolkit-agnostic and uses freedesktop standards (D-Bus, XDG), so it
//! behaves the same on Plasma and GNOME.
//!
//! Callbacks run on background threads; front ends marshal them to their
//! GUI thread (GTK: `glib::MainContext::invoke`, Qt: cxx-qt's
//! `CxxQtThread::queue`).
//!
//! The engine itself is reached through `odm_ffi::api` (same methods and
//! events as the macOS and Windows apps).

pub mod autostart;
pub mod awake;
pub mod desktop;
pub mod instance;
pub mod notify;
pub mod open;
pub mod paths;
pub mod power;
pub mod shell;
pub mod tray;

/// Reverse-DNS application id: D-Bus name, desktop file, icon name.
pub const APP_ID: &str = "app.opendm.Desktop";
/// Human-readable name.
pub use odm_ui::APP_NAME;
/// Formatting, and links handed to the app: shared with the other front
/// ends through `odm-ui`.
pub use odm_ui::{add, fmt, incoming};
