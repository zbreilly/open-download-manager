//! Open Download Manager for GNOME: GTK 4 + libadwaita on the shared Rust
//! engine (`odm_ffi::api`), with the desktop services from `odm-linux`.
mod add;
mod app;
#[cfg(debug_assertions)]
mod debug;
mod grabber;
mod progress;
mod row;
mod scheduler;
mod settings;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};
use odm_linux::shell::Shell;

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (tx, rx) = async_channel::unbounded::<app::Msg>();
    let shell_tx = tx.clone();
    let shell = match Shell::start(args, move |ev| {
        let _ = shell_tx.send_blocking(app::Msg::Shell(ev));
    }) {
        Ok(Some(s)) => s,
        // the running instance got our arguments
        Ok(None) => return glib::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("open-download-manager: {e}");
            return glib::ExitCode::FAILURE;
        }
    };

    // uniqueness is the shell's job (shared with the Plasma app), so GTK
    // must not claim the bus name itself
    let gtk_app = adw::Application::builder()
        .application_id(odm_linux::APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    glib::set_application_name(odm_linux::APP_NAME);

    let started = std::cell::Cell::new(false);
    gtk_app.connect_activate(move |gtk_app| {
        if started.replace(true) {
            app::odm().show_main();
            return;
        }
        gtk::Window::set_default_icon_name(odm_linux::APP_ID);
        // progress bars in list cells may be narrower than the theme's minimum
        let css = gtk::CssProvider::new();
        css.load_from_string(
            "progressbar.odm-cell > trough { min-width: 40px; }
             progressbar.odm-cell.warning > trough > progress { background-color: @warning_bg_color; }
             progressbar.odm-cell.error > trough > progress { background-color: @error_bg_color; }
             progressbar.odm-cell.success > trough > progress { background-color: @success_bg_color; }
             progressbar.odm-cell.dim-label > trough > progress { background-color: alpha(currentColor, 0.4); }",
        );
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        #[cfg(debug_assertions)]
        debug::start(gtk_app);
        app::App::start(gtk_app, shell.clone(), tx.clone(), rx.clone());
    });
    // our own arguments were parsed above; GTK gets none
    gtk_app.run_with_args::<&str>(&[])
}
