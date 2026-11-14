//! One view-model per window.
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_ui::store::Store;
use serde_json::{json, Value};

mod add;
mod grabber;
mod main;
mod progress;
mod scheduler;
mod settings;

pub fn create(kind: &str, params: &Value, store: &Store, cx: &mut Cx) -> Option<Box<dyn Vm>> {
    match kind {
        "main" => Some(Box::new(main::Main::new(store))),
        "add" => {
            let clipboard = crate::backend::ffi::clipboard_text().to_string();
            Some(Box::new(add::Add::new(params, store, &clipboard, cx)))
        }
        "progress" => Some(Box::new(progress::Progress::new(params, store))),
        "scheduler" => Some(Box::new(scheduler::Scheduler::new(store))),
        "grabber" => Some(Box::new(grabber::Grabber::new(params))),
        "settings" => Some(Box::new(settings::Settings::new(store))),
        _ => None,
    }
}

/// Fire an engine call off the GUI thread; an error is shown to the user.
pub fn act(method: &'static str, args: Value) {
    let _ = std::thread::Builder::new().name("odm-call".into()).spawn(move || {
        if let Err(e) = api::call(method, args) {
            crate::backend::report(e);
        }
    });
}

/// The engine's folder for a category.
pub fn folder_for(category: &str) -> Option<String> {
    api::call("folder_for", json!({ "category": category })).ok().and_then(|v| v.as_str().map(String::from))
}
