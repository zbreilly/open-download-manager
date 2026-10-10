//! Development aids (debug builds only).
//!
//! * `ODM_SHOTS=<folder>`: every open window is saved there as a PNG every
//!   two seconds, so the UI can be checked without a screen-capture tool.
//! * `ODM_OPEN=add,batch,web,grabber,scheduler,settings,progress`: open
//!   these windows at startup.
//! * `ODM_PORT=<port>`: move the browser bridge there, away from the port an
//!   installed copy (and the browser extension) uses.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::time::Duration;

pub fn start(app: &impl IsA<gtk::Application>) {
    let Some(dir) = std::env::var_os("ODM_SHOTS").map(std::path::PathBuf::from) else { return };
    let app = app.clone().upcast::<gtk::Application>();
    glib::timeout_add_local(Duration::from_secs(2), move || {
        for (i, w) in app.windows().iter().enumerate() {
            let (width, height) = (w.width(), w.height());
            if !w.is_visible() || width == 0 || height == 0 {
                continue;
            }
            let paintable = gtk::WidgetPaintable::new(Some(w));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, width as f64, height as f64);
            let (Some(node), Some(renderer)) = (snapshot.to_node(), w.renderer()) else { continue };
            let texture: gdk::Texture = renderer.render_texture(&node, None);
            let title = w.title().map(|t| t.replace(|c: char| !c.is_alphanumeric(), "-")).unwrap_or_default();
            let _ = texture.save_to_png(dir.join(format!("{i}-{}.png", &title[..title.len().min(24)])));
        }
        glib::ControlFlow::Continue
    });
}

/// Open the windows named in `ODM_OPEN`.
pub fn open_windows() {
    if let Some(port) = std::env::var("ODM_PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
        let mut s = crate::app::odm().settings.borrow().clone();
        s["extensionPort"] = port.into();
        crate::app::odm().save_settings(s);
    }
    let Ok(list) = std::env::var("ODM_OPEN") else { return };
    let app = crate::app::odm();
    for w in list.split(',') {
        match w.trim() {
            "add" => crate::add::open(&app, serde_json::json!({ "url": "http://127.0.0.1:8919/song.mp3" })),
            "batch" => crate::add::open(&app, serde_json::json!({ "batch": true })),
            "web" => crate::add::open(&app, serde_json::json!({ "url": "http://127.0.0.1:8919/song.mp3", "source": "web" })),
            "grabber" => crate::grabber::open(&app, None),
            "scheduler" => crate::scheduler::open(&app),
            "settings" => crate::settings::open(&app),
            "progress" => {
                if let Some(row) = app.all_rows().first() {
                    app.open_progress(&row.id());
                }
            }
            _ => {}
        }
    }
}
