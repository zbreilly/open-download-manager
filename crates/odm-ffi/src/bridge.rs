//! Local HTTP bridge for the browser extension (127.0.0.1 only).
//! Same protocol as the Tauri app; instead of opening windows it emits
//! `show_add` / `show_links` events for the native UI.
//!
//! Requests that carry an `Origin` must come from a browser-extension
//! origin, so web pages cannot queue downloads.

use crate::core;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use odm_core::AddRequest;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

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
struct AddBody {
    url: String,
    urls: Vec<String>,
    referrer: Option<String>,
    file_name: Option<String>,
    cookies: Option<String>,
    user_agent: Option<String>,
    total_bytes: Option<u64>,
    mime: Option<String>,
    silent: bool,
}

fn is_http(u: &str) -> bool {
    url::Url::parse(u.trim()).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false)
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
    let mut urls: Vec<String> = b.urls.iter().filter(|u| is_http(u)).cloned().collect();
    if is_http(&b.url) {
        urls.insert(0, b.url.clone());
    }
    urls.dedup();
    if urls.is_empty() {
        return with_cors((StatusCode::BAD_REQUEST, "no valid URL").into_response(), origin);
    }
    if urls.len() > 1 {
        let links: Vec<Value> = urls.iter().map(|u| json!({ "url": u })).collect();
        c.emit("show_links", &json!({ "links": links, "referrer": b.referrer, "cookies": b.cookies, "userAgent": b.user_agent }));
    } else if b.silent {
        let req = AddRequest {
            url: urls[0].clone(),
            file_name: b.file_name,
            referrer: b.referrer,
            cookies: b.cookies,
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
        c.emit(
            "show_add",
            &json!({
                "url": urls[0],
                "fileName": b.file_name,
                "referrer": b.referrer,
                "cookies": b.cookies,
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

async fn pause_capture(h: HeaderMap, Json(b): Json<PauseBody>) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    if let Some(c) = core() {
        let mut s = c.mgr.settings();
        s.capture_paused_until = if b.minutes > 0 { Some(now_ms() + b.minutes * 60_000) } else { None };
        let _ = c.mgr.save_settings(s);
    }
    with_cors(Json(capture_info()).into_response(), origin)
}

pub async fn serve(port: u16) {
    let router = Router::new()
        .route("/api/ping", get(ping).post(ping))
        .route("/api/add", post(add))
        .route("/api/capture-pause", post(pause_capture))
        .layer(axum::middleware::from_fn(
            |req: axum::extract::Request, next: axum::middleware::Next| async move {
                if req.method() == Method::OPTIONS {
                    return preflight(req.headers().clone()).await;
                }
                next.run(req).await
            },
        ));
    if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        let _ = axum::serve(l, router).await;
    }
}
