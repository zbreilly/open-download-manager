//! Local HTTP bridge for the browser extension (127.0.0.1 only).
//!
//! * The `Host` header must be `127.0.0.1:<port>` or `localhost:<port>`, so
//!   a web page cannot reach the bridge through DNS rebinding.
//! * Requests from web pages are refused: a request carrying an `Origin`
//!   header must come from a browser-extension origin. Requests without an
//!   Origin (local tools, curl) are allowed — they already run as the user —
//!   but `silent` (add and start without asking) is only honored for
//!   browser-extension origins.

use crate::state::{AppState, BridgeHandle};
use crate::windows;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
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
        r.headers_mut().insert("access-control-allow-headers", HeaderValue::from_static("content-type, x-odm-browser"));
        r.headers_mut().insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, OPTIONS"));
    }
    r
}

fn note_browser(app: &AppHandle, h: &HeaderMap) {
    // only real extensions count as "connected" (not curl / local tools)
    let from_ext = h.get("origin").is_some() || h.get("x-odm-browser").is_some();
    if !from_ext {
        return;
    }
    let name = h
        .get("x-odm-browser")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("Browser")
        .chars()
        .take(40)
        .collect::<String>();
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

async fn ping(State(c): State<Ctx>, h: HeaderMap) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    note_browser(&c.app, &h);
    with_cors(Json(capture_info(&c.app)).into_response(), origin)
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

fn links_json(links: &[(String, Option<String>)]) -> Vec<serde_json::Value> {
    links
        .iter()
        .map(|(u, c)| match c {
            Some(c) => json!({ "url": u, "cookies": c }),
            None => json!({ "url": u }),
        })
        .collect()
}

async fn add(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<AddBody>) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    if !h
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false)
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    note_browser(&c.app, &h);
    let app = c.app.clone();
    let links = collect_links(&b);
    if links.is_empty() {
        return with_cors((StatusCode::BAD_REQUEST, "no valid URL").into_response(), origin);
    }
    // no app setting allows starting captures without asking, so only a
    // browser extension (never a local client without Origin) may do it
    let silent = b.silent && origin.is_some();
    if links.len() > 1 {
        let params = json!({ "links": links_json(&links), "referrer": b.referrer, "userAgent": b.user_agent });
        let a2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = windows::open(&a2, "grabber", None, Some(params));
        });
    } else if silent {
        let (url, cookies) = links.into_iter().next().unwrap_or_default();
        let mgr = app.state::<AppState>().mgr.clone();
        let req = odm_core::AddRequest {
            url,
            file_name: b.file_name.clone(),
            referrer: b.referrer.clone(),
            cookies,
            user_agent: b.user_agent.clone(),
            total_bytes: b.total_bytes,
            mime: b.mime.clone(),
            start: true,
            ..Default::default()
        };
        if let Err(e) = mgr.add(req) {
            return with_cors((StatusCode::BAD_REQUEST, e.to_string()).into_response(), origin);
        }
    } else {
        let (url, cookies) = links.into_iter().next().unwrap_or_default();
        let params = json!({
            "url": url,
            "fileName": b.file_name,
            "referrer": b.referrer,
            "cookies": cookies,
            "userAgent": b.user_agent,
            "totalBytes": b.total_bytes,
            "mime": b.mime,
        });
        let a2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = windows::open(&a2, "add", None, Some(params));
        });
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

async fn pause_capture(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<PauseBody>) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    let mgr = c.app.state::<AppState>().mgr.clone();
    let mut s = mgr.settings();
    s.capture_paused_until = pause_until(chrono_now(), b.minutes);
    let _ = mgr.save_settings(s);
    with_cors(Json(capture_info(&c.app)).into_response(), origin)
}

fn is_http(u: &str) -> bool {
    url::Url::parse(u.trim()).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false)
}

fn router(app: AppHandle, port: u16) -> Router {
    let ctx = Ctx { app };
    Router::new()
        .route("/api/ping", get(ping).post(ping))
        .route("/api/add", post(add))
        .route("/api/capture-pause", post(pause_capture))
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
            items: vec![
                AddItem { url: "https://a/1".into(), cookies: Some("a=1".into()) },
                AddItem { url: "https://b/2".into(), cookies: None },
                AddItem { url: "https://a/1".into(), cookies: Some("dup".into()) },
            ],
            urls: vec!["https://b/2".into(), "https://c/3".into(), "javascript:x".into()],
            ..Default::default()
        };
        let l = collect_links(&b);
        assert_eq!(
            l,
            vec![
                ("https://a/1".into(), Some("a=1".into())),
                ("https://b/2".into(), None),
                ("https://c/3".into(), None)
            ]
        );
        let old = AddBody {
            urls: vec!["https://x/1".into(), "https://y/2".into()],
            cookies: Some("x=1".into()),
            ..Default::default()
        };
        let l = collect_links(&old);
        assert_eq!(l[0].1.as_deref(), Some("x=1"));
        assert_eq!(l[1].1, None);
    }
}
