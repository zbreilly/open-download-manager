//! The Rust API (used by the Linux apps) follows the same contract.

use odm_ffi::api;
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn start_call_events_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    api::start(dir.path(), move |name, _json| {
        let _ = tx.send(name.to_string());
        // the callback may call back into the engine
        let _ = api::call("version", json!({}));
    })
    .unwrap();
    let v: String = api::call_as("version", json!({})).unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));
    let mut s: serde_json::Value = api::call("get_settings", json!({})).unwrap();
    s["maxSimultaneous"] = json!(2);
    api::call("save_settings", json!({ "settings": s })).unwrap();
    // an event arrives on the events thread
    let mut got = false;
    while let Ok(n) = rx.recv_timeout(Duration::from_secs(5)) {
        if n == "settings_changed" {
            got = true;
            break;
        }
    }
    assert!(got, "no settings_changed event");
    assert!(api::call("no_such_method", json!({})).unwrap_err().contains("unknown method"));
    api::shutdown();
}
