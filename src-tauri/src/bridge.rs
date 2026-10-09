//! Local HTTP bridge for the browser extension (127.0.0.1 only).
//!
//! Requests from web pages are refused: a request carrying an `Origin`
//! header must come from a browser-extension origin. Requests without an
//! Origin (local tools, curl) are allowed — they already run as the user.

use crate::state::AppState;
use crate::windows;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
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
struct AddBody {
    url: String,
    urls: Vec<String>,
    referrer: Option<String>,
    file_name: Option<String>,
    cookies: Option<String>,
    user_agent: Option<String>,
    total_bytes: Option<u64>,
    mime: Option<String>,
    /// add & start without showing the dialog
    silent: bool,
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
    let mut urls: Vec<String> = b.urls.iter().filter(|u| is_http(u)).cloned().collect();
    if is_http(&b.url) {
        urls.insert(0, b.url.clone());
    }
    urls.dedup();
    if urls.is_empty() {
        return with_cors((StatusCode::BAD_REQUEST, "no valid URL").into_response(), origin);
    }
    if urls.len() > 1 {
        let links: Vec<serde_json::Value> = urls.iter().map(|u| json!({ "url": u })).collect();
        let params = json!({ "links": links, "referrer": b.referrer, "cookies": b.cookies, "userAgent": b.user_agent });
        let a2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = windows::open(&a2, "grabber", None, Some(params));
        });
    } else if b.silent {
        let mgr = app.state::<AppState>().mgr.clone();
        let req = odm_core::AddRequest {
            url: urls[0].clone(),
            file_name: b.file_name.clone(),
            referrer: b.referrer.clone(),
            cookies: b.cookies.clone(),
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
        let params = json!({
            "url": urls[0],
            "fileName": b.file_name,
            "referrer": b.referrer,
            "cookies": b.cookies,
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

async fn pause_capture(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<PauseBody>) -> Response {
    let Some(origin) = origin_ok(&h) else { return StatusCode::FORBIDDEN.into_response() };
    let mgr = c.app.state::<AppState>().mgr.clone();
    let mut s = mgr.settings();
    s.capture_paused_until = if b.minutes > 0 { Some(chrono_now() + b.minutes * 60_000) } else { None };
    let _ = mgr.save_settings(s);
    with_cors(Json(capture_info(&c.app)).into_response(), origin)
}

fn is_http(u: &str) -> bool {
    url::Url::parse(u.trim()).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false)
}

pub fn spawn(app: AppHandle, port: u16) {
    tauri::async_runtime::spawn(async move {
        let ctx = Ctx { app: app.clone() };
        let router = Router::new()
            .route("/api/ping", get(ping).post(ping))
            .route("/api/add", post(add))
            .route("/api/capture-pause", post(pause_capture))
            .route("/{*rest}", any(|m: Method, h: HeaderMap| async move {
                if m == Method::OPTIONS {
                    preflight(h).await
                } else {
                    StatusCode::NOT_FOUND.into_response()
                }
            }))
            .with_state(ctx);
        // handle OPTIONS on the real routes too
        let router = router.layer(axum::middleware::from_fn(|req: axum::extract::Request, next: axum::middleware::Next| async move {
            if req.method() == Method::OPTIONS {
                return preflight(req.headers().clone()).await;
            }
            next.run(req).await
        }));
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => {
                tracing::info!("extension bridge on 127.0.0.1:{port}");
                let _ = axum::serve(l, router).await;
            }
            Err(e) => tracing::warn!("extension bridge could not bind 127.0.0.1:{port}: {e}"),
        }
    });
}
