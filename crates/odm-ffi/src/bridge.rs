//! Local HTTP bridge for the browser extension (127.0.0.1 only).
//! Same protocol as the Tauri app; instead of opening windows it emits
//! `show_add` / `show_links` events for the native UI.
//!
//! * The `Host` header must be `127.0.0.1:<port>` or `localhost:<port>`
//!   (DNS-rebinding guard).
//! * Requests that carry an `Origin` must come from a browser-extension
//!   origin, so web pages cannot queue downloads. Requests without an Origin
//!   (local tools) are allowed but cannot add downloads silently.

use crate::core;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use odm_core::AddRequest;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Instant;

/// Longest capture pause accepted from the extension (one day).
const MAX_PAUSE_MINUTES: i64 = 1440;

fn host_ok(h: &HeaderMap, port: u16) -> bool {
    let Some(host) = h.get(header::HOST).and_then(|v| v.to_str().ok()) else { return false };
    let host = host.trim().to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn origin_ok(h: &HeaderMap) -> Option<Option<String>> {
    match h.get("origin").and_then(|v| v.to_str().ok()) {
        None => Some(None),
        Some(o)
            if o.starts_with("chrome-extension://")
                || o.starts_with("moz-extension://")
                || o.starts_with("safari-web-extension://")
                || o.starts_with("extension://") =>
        {
            Some(Some(o.to_string()))
        }
        Some(_) => None,
    }
}

fn with_cors(mut r: Response, origin: Option<String>) -> Response {
    if let Some(o) = origin {
        if let Ok(v) = HeaderValue::from_str(&o) {
            r.headers_mut().insert("access-control-allow-origin", v);
        }
        r.headers_mut().insert("access-control-allow-headers", HeaderValue::from_static("content-type, x-odm-browser"));
        r.headers_mut().insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, OPTIONS"));
    }
    r
}

fn note_browser(h: &HeaderMap) {
    let Some(c) = core() else { return };
    if h.get("origin").is_none() && h.get("x-odm-browser").is_none() {
        return;
    }
    let name: String = h
        .get("x-odm-browser")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Browser")
        .chars()
        .take(40)
        .collect();
    let was = c.extension_browsers();
    c.ext_seen.lock().insert(name, Instant::now());
    let now = c.extension_browsers();
    if was != now {
        c.emit("extension_status", &json!({ "connected": !now.is_empty(), "browsers": now }));
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn capture_info() -> Value {
    let Some(c) = core() else { return json!({ "ok": false }) };
    let s = c.mgr.settings();
    let paused = s.capture_paused_until.map(|t| t > now_ms()).unwrap_or(false);
    json!({
        "ok": true,
        "app": "Open Download Manager",
        "version": env!("CARGO_PKG_VERSION"),
        "capture": {
            "enabled": s.capture_enabled && !paused,
            "paused": paused,
            "pausedUntil": s.capture_paused_until,
            "extensions": s.capture_extensions,
            "bypassKey": s.bypass_key,
            "minSizeKb": s.capture_min_size_kb,
        }
    })
}

async fn ping(h: HeaderMap) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    note_browser(&h);
    with_cors(Json(capture_info()).into_response(), origin)
}

async fn preflight(h: HeaderMap) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    with_cors(StatusCode::NO_CONTENT.into_response(), origin)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct AddItem {
    url: String,
    cookies: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct AddBody {
    url: String,
    /// several links, each with the cookies for that URL
    items: Vec<AddItem>,
    /// several links without cookies (older extensions)
    urls: Vec<String>,
    referrer: Option<String>,
    file_name: Option<String>,
    /// cookies for `url` (or, in the older form, for the first link)
    cookies: Option<String>,
    user_agent: Option<String>,
    total_bytes: Option<u64>,
    mime: Option<String>,
    /// add & start without asking (browser extensions only)
    silent: bool,
}

fn is_http(u: &str) -> bool {
    url::Url::parse(u.trim()).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false)
}

/// Valid http(s) links in request order (`url`, then `items`, then `urls`),
/// without duplicates, each with its own cookies.
fn collect_links(b: &AddBody) -> Vec<(String, Option<String>)> {
    fn nz(c: &Option<String>) -> Option<String> {
        c.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
    }
    let mut seen = HashSet::new();
    let mut out: Vec<(String, Option<String>)> = vec![];
    let mut push = |u: &str, c: Option<String>| {
        let u = u.trim();
        if is_http(u) && seen.insert(u.to_string()) {
            out.push((u.to_string(), c));
        }
    };
    push(&b.url, nz(&b.cookies));
    for it in &b.items {
        push(&it.url, nz(&it.cookies));
    }
    for u in &b.urls {
        push(u, None);
    }
    // older extensions sent the first link's cookies at the top level
    if let Some(first) = out.first_mut() {
        if first.1.is_none() {
            first.1 = nz(&b.cookies);
        }
    }
    out
}

async fn add(h: HeaderMap, body: axum::body::Bytes) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    let json_ct = h
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    if !json_ct {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let Ok(b) = serde_json::from_slice::<AddBody>(&body) else {
        return with_cors((StatusCode::BAD_REQUEST, "bad json").into_response(), origin);
    };
    note_browser(&h);
    let Some(c) = core() else { return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    let links = collect_links(&b);
    if links.is_empty() {
        return with_cors((StatusCode::BAD_REQUEST, "no valid URL").into_response(), origin);
    }
    // no app setting allows starting captures without asking, so only a
    // browser extension (never a local client without Origin) may do it
    let silent = b.silent && origin.is_some();
    if links.len() > 1 {
        let links: Vec<Value> = links
            .iter()
            .map(|(u, ck)| match ck {
                Some(ck) => json!({ "url": u, "cookies": ck }),
                None => json!({ "url": u }),
            })
            .collect();
        c.emit("show_links", &json!({ "links": links, "referrer": b.referrer, "userAgent": b.user_agent }));
    } else if silent {
        let (url, cookies) = links.into_iter().next().unwrap_or_default();
        let req = AddRequest {
            url,
            file_name: b.file_name,
            referrer: b.referrer,
            cookies,
            user_agent: b.user_agent,
            total_bytes: b.total_bytes,
            mime: b.mime,
            start: true,
            ..Default::default()
        };
        if let Err(e) = c.mgr.add(req) {
            return with_cors((StatusCode::BAD_REQUEST, e.to_string()).into_response(), origin);
        }
    } else {
        let (url, cookies) = links.into_iter().next().unwrap_or_default();
        c.emit(
            "show_add",
            &json!({
                "url": url,
                "fileName": b.file_name,
                "referrer": b.referrer,
                "cookies": cookies,
                "userAgent": b.user_agent,
                "totalBytes": b.total_bytes,
                "mime": b.mime,
            }),
        );
    }
    with_cors(Json(json!({ "ok": true })).into_response(), origin)
}

#[derive(Deserialize)]
struct PauseBody {
    minutes: i64,
}

/// End of a capture pause of `minutes` (clamped to 0..=1440) from `now`.
fn pause_until(now: i64, minutes: i64) -> Option<i64> {
    let m = minutes.clamp(0, MAX_PAUSE_MINUTES);
    if m == 0 {
        return None;
    }
    m.checked_mul(60_000).and_then(|ms| now.checked_add(ms))
}

async fn pause_capture(h: HeaderMap, Json(b): Json<PauseBody>) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    if let Some(c) = core() {
        let mut s = c.mgr.settings();
        s.capture_paused_until = pause_until(now_ms(), b.minutes);
        let _ = c.mgr.save_settings(s);
    }
    with_cors(Json(capture_info()).into_response(), origin)
}

fn router(port: u16) -> Router {
    Router::new()
        .route("/api/ping", get(ping).post(ping))
        .route("/api/add", post(add))
        .route("/api/capture-pause", post(pause_capture))
        .layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| async move {
                if !host_ok(req.headers(), port) {
                    return (StatusCode::FORBIDDEN, "bad host").into_response();
                }
                if req.method() == Method::OPTIONS {
                    return preflight(req.headers().clone()).await;
                }
                next.run(req).await
            },
        ))
}

/// Listen on 127.0.0.1:`port` and stop the previous listener (if any).
/// Binding is synchronous, so this never blocks on the runtime. On a bind
/// error the previous listener keeps running, the error is logged, kept for
/// `extension_status` (`bridgeError`), emitted as a `bridge_error` event
/// `{port, error}` and returned.
pub fn start(port: u16) -> Result<(), String> {
    let c = core().ok_or("engine not started")?;
    let _g = c.rt.enter();
    let bound = std::net::TcpListener::bind(("127.0.0.1", port))
        .and_then(|l| l.set_nonblocking(true).map(|_| l))
        .and_then(tokio::net::TcpListener::from_std);
    let l = match bound {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("could not listen on 127.0.0.1:{port}: {e}");
            eprintln!("odm: extension bridge: {msg}");
            *c.bridge_error.lock() = Some(msg.clone());
            c.emit("bridge_error", &json!({ "port": port, "error": msg }));
            return Err(msg);
        }
    };
    if c.bridge_error.lock().take().is_some() {
        c.emit("bridge_error", &Value::Null);
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    if let Some(old) = c.bridge.lock().replace(tx) {
        let _ = old.send(());
    }
    c.rt.spawn(async move {
        let _ = axum::serve(l, router(port))
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    Ok(())
}

/// Stop the listener (on shutdown).
pub fn stop() {
    if let Some(c) = core() {
        if let Some(tx) = c.bridge.lock().take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_check() {
        let mut h = HeaderMap::new();
        assert!(!host_ok(&h, 8901));
        h.insert(header::HOST, HeaderValue::from_static("localhost:8901"));
        assert!(host_ok(&h, 8901));
        h.insert(header::HOST, HeaderValue::from_static("attacker.example:8901"));
        assert!(!host_ok(&h, 8901));
    }

    #[test]
    fn pause_clamp() {
        assert_eq!(pause_until(0, 5000), Some(1440 * 60_000));
        assert_eq!(pause_until(0, -1), None);
        assert_eq!(pause_until(i64::MAX - 1, 1), None);
    }

    #[test]
    fn per_url_cookies() {
        let b: AddBody = serde_json::from_value(json!({
            "items": [{ "url": "https://a/1", "cookies": "a=1" }, { "url": "https://b/2" }],
            "urls": ["https://a/1"]
        }))
        .unwrap();
        let l = collect_links(&b);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1.as_deref(), Some("a=1"));
        assert_eq!(l[1].1, None);
    }
}
