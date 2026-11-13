//! The Rust API (used by the Linux apps) follows the same contract.

use odm_core::pairing::{parse_key, Pairing};
use odm_ffi::api;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

/// One HTTP request to the bridge; returns the status and the body.
fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("bridge is listening");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let status = raw.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

const JSON: (&str, &str) = ("Content-Type", "application/json");

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// Seal `op`, send it, and open the answer.
fn rpc(port: u16, key: &Pairing, op: &str, body: Value) -> Result<Value, u16> {
    let (envelope, nonce) = key.seal_request(op, &body, now_ms());
    let (status, answer) = http(port, "POST", "/api/rpc", &[JSON, ("X-ODM-Browser", "Test")], &envelope);
    if status != 200 {
        return Err(status);
    }
    Ok(key.open_response(&nonce, answer.as_bytes()).expect("answer sealed for this request"))
}

fn wait_event(rx: &mpsc::Receiver<(String, String)>, name: &str) -> Value {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while let Ok((n, j)) = rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now())) {
        if n == name {
            return serde_json::from_str(&j).unwrap_or(Value::Null);
        }
    }
    panic!("no {name} event");
}

/// The engine is one per process, so everything runs in a single test.
#[test]
fn start_call_events_bridge_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel::<(String, String)>();
    api::start(dir.path(), move |name, json| {
        let _ = tx.send((name.to_string(), json.to_string()));
        // the callback may call back into the engine
        let _ = api::call("version", json!({}));
    })
    .unwrap();
    let v: String = api::call_as("version", json!({})).unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));

    // move the bridge to a free port (8901 may belong to a running app)
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut s: Value = api::call("get_settings", json!({})).unwrap();
    s["maxSimultaneous"] = json!(2);
    s["extensionPort"] = json!(port);
    api::call("save_settings", json!({ "settings": s })).unwrap();
    // an event arrives on the events thread
    wait_event(&rx, "settings_changed");
    assert!(api::call("no_such_method", json!({})).unwrap_err().contains("unknown method"));

    // ---- the browser bridge ----
    // without the key: only "the app is here"
    let (status, body) = http(port, "GET", "/api/ping", &[], "");
    assert_eq!(status, 200);
    let hello: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(hello["pairing"], "required");
    assert!(hello.get("capture").is_none(), "settings are not given out unsealed: {body}");
    assert!(hello.get("version").is_none(), "the version is not given out unsealed: {body}");
    // web pages and forged hosts are refused before anything else
    assert_eq!(http(port, "GET", "/api/ping", &[("Origin", "https://evil.example")], "").0, 403);
    assert_eq!(http(port, "GET", "/api/ping", &[("Host", "evil.example")], "").0, 403);
    // the unsealed routes of earlier versions are gone
    let plain = json!({ "url": "http://127.0.0.1:9/x.bin", "silent": true }).to_string();
    let as_extension = [JSON, ("Origin", "chrome-extension://someoneelse")];
    assert_eq!(http(port, "POST", "/api/add", &as_extension, &plain).0, 404);
    assert_eq!(http(port, "POST", "/api/capture-pause", &as_extension, r#"{"minutes":10}"#).0, 404);
    // an unsealed or wrongly sealed request does nothing
    assert_eq!(http(port, "POST", "/api/rpc", &as_extension, &plain).0, 401);
    let stranger = Pairing::new(parse_key(&"ab".repeat(32)).unwrap());
    assert_eq!(rpc(port, &stranger, "add", json!({ "url": "http://127.0.0.1:9/x.bin", "silent": true })), Err(401));
    let list: Vec<Value> = api::call_as("list_downloads", json!({})).unwrap();
    assert!(list.is_empty(), "nothing was added without the key");
    let status: Value = api::call("extension_status", json!({})).unwrap();
    assert_eq!(status["connected"], false, "unpaired clients do not count as connected");

    // with the key from the settings
    let key_text: String = api::call_as("bridge_key", json!({})).unwrap();
    let key = Pairing::new(parse_key(&key_text).expect("a pairing key"));
    let info = rpc(port, &key, "ping", Value::Null).unwrap();
    assert_eq!(info["ok"], true);
    assert!(info["capture"]["extensions"].is_array());
    let status: Value = api::call("extension_status", json!({})).unwrap();
    assert_eq!(status["browsers"], json!(["Test"]));
    // a captured request cannot be sent again
    let (envelope, _) = key.seal_request("ping", &Value::Null, now_ms());
    assert_eq!(http(port, "POST", "/api/rpc", &[JSON], &envelope).0, 200);
    assert_eq!(http(port, "POST", "/api/rpc", &[JSON], &envelope).0, 401);
    // nor one from long ago
    let (old, _) = key.seal_request("ping", &Value::Null, now_ms() - 10 * 60_000);
    assert_eq!(http(port, "POST", "/api/rpc", &[JSON], &old).0, 401);

    // a link with its cookies reaches the app's Add dialog
    let added = rpc(
        port,
        &key,
        "add",
        json!({ "url": "https://h.example/f.zip", "cookies": "sid=1", "referrer": "https://h.example/page", "fileName": "f.zip" }),
    )
    .unwrap();
    assert_eq!(added["ok"], true);
    let shown = wait_event(&rx, "show_add");
    assert_eq!(shown["url"], "https://h.example/f.zip");
    assert_eq!(shown["cookies"], "sid=1");
    // outside an automated run a paired client cannot skip the dialog either
    let asked = rpc(port, &key, "add", json!({ "url": "https://h.example/quiet.zip", "silent": true })).unwrap();
    assert_eq!(asked["ok"], true);
    assert_eq!(wait_event(&rx, "show_add")["url"], "https://h.example/quiet.zip");
    let list: Vec<Value> = api::call_as("list_downloads", json!({})).unwrap();
    assert!(list.is_empty(), "a silent request was started without the dialog");
    // several links go to the link list, each with its own cookies
    let items = json!([{ "url": "https://a.example/1.zip", "cookies": "a=1" }, { "url": "https://b.example/2.zip" }]);
    rpc(port, &key, "add", json!({ "items": items })).unwrap();
    let shown = wait_event(&rx, "show_links");
    assert_eq!(shown["links"][0]["cookies"], "a=1");
    assert!(shown["links"][1].get("cookies").is_none());
    // errors are reported inside the sealed answer
    let bad = rpc(port, &key, "add", json!({ "url": "javascript:alert(1)" })).unwrap();
    assert_eq!(bad["ok"], false);
    let bad = rpc(port, &key, "format-disk", Value::Null).unwrap();
    assert_eq!(bad["ok"], false);
    // pausing capture
    let paused = rpc(port, &key, "capture-pause", json!({ "minutes": 10 })).unwrap();
    assert_eq!(paused["capture"]["paused"], true);
    let resumed = rpc(port, &key, "capture-pause", json!({ "minutes": 0 })).unwrap();
    assert_eq!(resumed["capture"]["paused"], false);

    // a new key unpairs the old one
    let new_text: String = api::call_as("regenerate_bridge_key", json!({})).unwrap();
    assert_ne!(new_text, key_text);
    assert_eq!(rpc(port, &key, "ping", Value::Null), Err(401));
    let new_key = Pairing::new(parse_key(&new_text).unwrap());
    assert!(rpc(port, &new_key, "ping", Value::Null).is_ok());

    api::shutdown();
}
