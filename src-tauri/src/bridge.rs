//! Local HTTP bridge for the browser extension (127.0.0.1 only).
//!
//! * The `Host` header must be `127.0.0.1:<port>` or `localhost:<port>`, so
//!   a web page cannot reach the bridge through DNS rebinding.
//! * Every request is sealed with the pairing key (see
//!   `odm_core::pairing`), and so is every answer: a caller without the key
//!   is refused, and nothing readable is sent to one.
//! * Requests from web pages are refused: a request carrying an `Origin`
//!   header must come from a browser-extension origin.
//! * A request never starts a download: it opens the Add window (or the
//!   link list), where the user decides.

use crate::state::{AppState, BridgeHandle};
use crate::windows;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

/// Longest capture pause accepted from the extension (one day).
const MAX_PAUSE_MINUTES: i64 = 1440;

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
}

/// `Host` must name the loopback listener itself (DNS-rebinding guard).
fn host_ok(h: &HeaderMap, port: u16) -> bool {
    let Some(host) = h.get(header::HOST).and_then(|v| v.to_str().ok()) else { return false };
    let host = host.trim().to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn is_extension_origin(o: &str) -> bool {
    o.starts_with("chrome-extension://")
        || o.starts_with("moz-extension://")
        || o.starts_with("safari-web-extension://")
        || o.starts_with("extension://")
}

/// `None`: refuse. `Some(None)`: no Origin (local tool). `Some(Some(o))`:
/// a browser-extension origin.
fn origin_ok(h: &HeaderMap) -> Option<Option<String>> {
    match h.get("origin").and_then(|v| v.to_str().ok()) {
        None => Some(None),
        Some(o) if is_extension_origin(o) => Some(Some(o.to_string())),
        Some(_) => None,
    }
}

fn with_cors(mut r: Response, origin: Option<String>) -> Response {
    if let Some(o) = origin {
        if let Ok(v) = HeaderValue::from_str(&o) {
            r.headers_mut().insert("access-control-allow-origin", v);
        }
        r.headers_mut().insert("access-control-allow-headers", HeaderValue::from_static("content-type"));
        r.headers_mut().insert("access-control-allow-methods", HeaderValue::from_static("POST, OPTIONS"));
    }
    r
}

/// A paired extension was heard from: remember its browser.
fn note_browser(app: &AppHandle, name: Option<&str>) {
    let name: String = name.filter(|n| !n.trim().is_empty()).unwrap_or("Browser").chars().take(40).collect();
    let st = app.state::<AppState>();
    let was = st.extension_browsers();
    st.ext_seen.lock().insert(name, Instant::now());
    let now = st.extension_browsers();
    if was != now {
        let _ = app.emit("extension_status", json!({ "connected": !now.is_empty(), "browsers": now }));
    }
}

fn capture_info(app: &AppHandle) -> serde_json::Value {
    let s = app.state::<AppState>().mgr.settings();
    let now = chrono_now();
    let paused = s.capture_paused_until.map(|t| t > now).unwrap_or(false);
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

pub fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    referrer: Option<String>,
    file_name: Option<String>,
    /// cookies for `url` (or, in the older form, for the first link)
    cookies: Option<String>,
    user_agent: Option<String>,
    total_bytes: Option<u64>,
    mime: Option<String>,
    /// add & start without showing the dialog (browser extensions only)
    silent: bool,
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
    out
}

fn links_json(links: &[(String, Option<String>)]) -> Vec<serde_json::Value> {
    links
        .iter()
        .map(|(u, c)| match c {
            Some(c) => json!({ "url": u, "cookies": c }),
            None => json!({ "url": u }),
        })
        .collect()
}

/// Hand links to the UI. Nothing is added or started here.
fn add(app: &AppHandle, b: AddBody) -> serde_json::Value {
    let links = collect_links(&b);
    if links.is_empty() {
        return json!({ "ok": false, "error": "no valid URL" });
    }
    let (window, params) = if links.len() > 1 {
        ("grabber", json!({ "links": links_json(&links), "referrer": b.referrer, "userAgent": b.user_agent }))
    } else {
        let (url, cookies) = links.into_iter().next().unwrap_or_default();
        (
            "add",
            json!({
                "url": url,
                "fileName": b.file_name,
                "referrer": b.referrer,
                "cookies": cookies,
                "userAgent": b.user_agent,
                "totalBytes": b.total_bytes,
                "mime": b.mime,
            }),
        )
    };
    let a2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = windows::open(&a2, window, None, Some(params));
    });
    json!({ "ok": true })
}

/// End of a capture pause of `minutes` (clamped to 0..=1440) from `now`.
fn pause_until(now: i64, minutes: i64) -> Option<i64> {
    let m = minutes.clamp(0, MAX_PAUSE_MINUTES);
    if m == 0 {
        return None;
    }
    m.checked_mul(60_000).and_then(|ms| now.checked_add(ms))
}

fn pause_capture(app: &AppHandle, minutes: i64) -> serde_json::Value {
    let mgr = app.state::<AppState>().mgr.clone();
    let mut s = mgr.settings();
    s.capture_paused_until = pause_until(chrono_now(), minutes);
    let _ = mgr.save_settings(s);
    capture_info(app)
}

/// One opened request: `{op, browser, ...}`.
fn handle(app: &AppHandle, req: &serde_json::Value) -> serde_json::Value {
    note_browser(app, req.get("browser").and_then(|v| v.as_str()));
    match req.get("op").and_then(|v| v.as_str()) {
        Some("ping") => capture_info(app),
        Some("add") => match serde_json::from_value::<AddBody>(req.get("body").cloned().unwrap_or_default()) {
            Ok(b) => add(app, b),
            Err(_) => json!({ "ok": false, "error": "bad request" }),
        },
        Some("pause") => pause_capture(app, req.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0)),
        _ => json!({ "ok": false, "error": "unknown request" }),
    }
}

/// The bridge's one endpoint: a sealed request in, a sealed answer out.
async fn sealed(State(c): State<Ctx>, h: HeaderMap, body: axum::body::Bytes) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    if !h
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false)
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let mgr = c.app.state::<AppState>().mgr.clone();
    let req = match mgr.pairing().open_request(&body, chrono_now()) {
        Ok(r) => r,
        // the same answer for every failure: a caller without the key learns nothing
        Err(_) => return with_cors(StatusCode::UNAUTHORIZED.into_response(), origin),
    };
    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let answer = mgr.pairing().seal_response(&id, handle(&c.app, &req));
    let mut r = answer.into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    with_cors(r, origin)
}

fn is_http(u: &str) -> bool {
    url::Url::parse(u.trim()).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false)
}

fn router(app: AppHandle, port: u16) -> Router {
    let ctx = Ctx { app };
    Router::new()
        .route("/api/v2", post(sealed))
        .layer(axum::extract::DefaultBodyLimit::max(odm_core::pairing::MAX_REQUEST_BYTES))
        .route(
            "/{*rest}",
            any(|m: Method, h: HeaderMap| async move {
                if m == Method::OPTIONS {
                    preflight(h).await
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }),
        )
        .with_state(ctx)
        // Host check first, then OPTIONS on the real routes too
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

/// Listen on 127.0.0.1:`port` and stop the previous listener (if any). On a
/// bind error the previous listener keeps running, the error is remembered
/// (see `extension_status`), a `bridge_error` event `{port, error}` is
/// emitted and the error is returned.
pub async fn start(app: AppHandle, port: u16) -> Result<(), String> {
    let st = app.state::<AppState>();
    let l = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("could not listen on 127.0.0.1:{port}: {e}");
            tracing::warn!("extension bridge: {msg}");
            *st.bridge_error.lock() = Some(msg.clone());
            let _ = app.emit("bridge_error", json!({ "port": port, "error": msg }));
            return Err(msg);
        }
    };
    if st.bridge_error.lock().take().is_some() {
        let _ = app.emit("bridge_error", serde_json::Value::Null);
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    if let Some(old) = st.bridge.lock().replace(BridgeHandle { stop: tx }) {
        let _ = old.stop.send(());
    }
    let r = router(app.clone(), port);
    tauri::async_runtime::spawn(async move {
        tracing::info!("extension bridge on 127.0.0.1:{port}");
        let _ = axum::serve(l, r)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
        tracing::info!("extension bridge on 127.0.0.1:{port} stopped");
    });
    Ok(())
}

/// Stop the listener (on quit).
pub fn stop(app: &AppHandle) {
    if let Some(h) = app.state::<AppState>().bridge.lock().take() {
        let _ = h.stop.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_check() {
        let mut h = HeaderMap::new();
        assert!(!host_ok(&h, 8901));
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8901"));
        assert!(host_ok(&h, 8901));
        assert!(!host_ok(&h, 8902));
        h.insert(header::HOST, HeaderValue::from_static("LOCALHOST:8901"));
        assert!(host_ok(&h, 8901));
        h.insert(header::HOST, HeaderValue::from_static("evil.example:8901"));
        assert!(!host_ok(&h, 8901));
    }

    #[test]
    fn pause_clamp() {
        assert_eq!(pause_until(1000, 0), None);
        assert_eq!(pause_until(1000, -5), None);
        assert_eq!(pause_until(1000, 10), Some(1000 + 600_000));
        assert_eq!(pause_until(1000, i64::MAX), Some(1000 + 1440 * 60_000));
        assert_eq!(pause_until(i64::MAX, 10), None);
    }

    #[test]
    fn links_per_url_cookies() {
        let b = AddBody {
            url: "https://c/3".into(),
            items: vec![
                AddItem { url: "https://a/1".into(), cookies: Some("a=1".into()) },
                AddItem { url: "https://b/2".into(), cookies: None },
                AddItem { url: "https://a/1".into(), cookies: Some("dup".into()) },
                AddItem { url: "javascript:x".into(), cookies: None },
            ],
            ..Default::default()
        };
        assert_eq!(
            collect_links(&b),
            vec![("https://c/3".into(), None), ("https://a/1".into(), Some("a=1".into())), ("https://b/2".into(), None)]
        );
    }
}
