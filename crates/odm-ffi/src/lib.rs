//! C interface to the Open Download Manager engine.
//!
//! The native macOS app (SwiftUI) links this as a static library. Every
//! call takes a method name and JSON arguments and returns JSON, so the
//! interface stays small and stable while the engine evolves.

mod bridge;

use odm_core::manager::DownloadPatch;
use odm_core::*;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

pub type EventCb = extern "C" fn(name: *const c_char, json: *const c_char, ctx: *mut c_void);

struct Sink {
    cb: EventCb,
    ctx: usize,
}

pub(crate) struct Core {
    pub rt: tokio::runtime::Runtime,
    pub mgr: Manager,
    sink: Sink,
    pub ext_seen: Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

static CORE: OnceLock<Core> = OnceLock::new();
static LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);

pub(crate) fn core() -> Option<&'static Core> {
    CORE.get()
}

impl Core {
    pub fn emit(&self, name: &str, payload: &Value) {
        let (Ok(n), Ok(j)) = (CString::new(name), CString::new(payload.to_string())) else { return };
        (self.sink.cb)(n.as_ptr(), j.as_ptr(), self.sink.ctx as *mut c_void);
    }

    pub fn extension_browsers(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ext_seen
            .lock()
            .iter()
            .filter(|(_, t)| t.elapsed().as_secs() < 150)
            .map(|(b, _)| b.clone())
            .collect();
        v.sort();
        v
    }
}

fn set_error(msg: String) {
    *LAST_ERROR.lock() = CString::new(msg).ok();
}

/// # Safety
/// `data_dir` must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn odm_start(data_dir: *const c_char, cb: EventCb, ctx: *mut c_void) -> i32 {
    if CORE.get().is_some() {
        return 0;
    }
    if data_dir.is_null() {
        set_error("data_dir is null".into());
        return -1;
    }
    let dir = match CStr::from_ptr(data_dir).to_str() {
        Ok(s) => std::path::PathBuf::from(s),
        Err(_) => {
            set_error("data_dir is not UTF-8".into());
            return -1;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        set_error(format!("cannot create {}: {e}", dir.display()));
        return -2;
    }
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("odm-engine").build() {
        Ok(r) => r,
        Err(e) => {
            set_error(format!("runtime: {e}"));
            return -3;
        }
    };
    let mgr = match rt.block_on(async { Manager::open(&dir.join("odm.db")) }) {
        Ok(m) => m,
        Err(e) => {
            set_error(e.to_string());
            return -4;
        }
    };
    let core = Core {
        rt,
        mgr: mgr.clone(),
        sink: Sink { cb, ctx: ctx as usize },
        ext_seen: Mutex::new(Default::default()),
    };
    if CORE.set(core).is_err() {
        return 0;
    }
    let c = CORE.get().expect("core set");
    c.rt.block_on(async { c.mgr.start_background() });
    c.rt.spawn(forward_events());
    let port = c.mgr.settings().extension_port;
    c.rt.spawn(bridge::serve(port));
    0
}

async fn forward_events() {
    let Some(c) = core() else { return };
    let mut rx = c.mgr.subscribe();
    loop {
        let ev = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => break,
        };
        let (name, payload) = match ev {
            Event::Added(d) => ("download_added", json!(d)),
            Event::Changed(d) => ("download_changed", json!(d)),
            Event::Removed(id) => ("download_removed", json!(id)),
            Event::Progress(p) => ("progress_batch", json!(p)),
            Event::Totals(t) => {
                let b = c.extension_browsers();
                c.emit("extension_status", &json!({ "connected": !b.is_empty(), "browsers": b }));
                ("totals", json!(t))
            }
            Event::Completed(d) => ("download_completed", json!(d)),
            Event::Failed(d) => ("download_failed", json!(d)),
            Event::QueueFinished(q) => ("queue_finished", json!(q)),
            Event::QueuesChanged(q) => ("queues_changed", json!(q)),
            Event::SettingsChanged(s) => ("settings_changed", json!(s)),
        };
        c.emit(name, &payload);
    }
}

/// # Safety
/// `method` and `args` must be valid NUL-terminated UTF-8 strings (args may be null).
#[no_mangle]
pub unsafe extern "C" fn odm_call(method: *const c_char, args: *const c_char) -> *mut c_char {
    let out = (|| -> Result<Value, String> {
        let c = core().ok_or("engine not started")?;
        if method.is_null() {
            return Err("method is null".into());
        }
        let m = CStr::from_ptr(method).to_str().map_err(|_| "method not UTF-8")?;
        let a: Value = if args.is_null() {
            Value::Null
        } else {
            let s = CStr::from_ptr(args).to_str().map_err(|_| "args not UTF-8")?;
            if s.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(s).map_err(|e| format!("bad args: {e}"))?
            }
        };
        let _g = c.rt.enter();
        dispatch(c, m, a)
    })();
    let v = match out {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    };
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

/// # Safety
/// `s` must come from `odm_call` and be freed only once.
#[no_mangle]
pub unsafe extern "C" fn odm_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[no_mangle]
pub extern "C" fn odm_last_error() -> *const c_char {
    match LAST_ERROR.lock().as_ref() {
        Some(s) => s.as_ptr(),
        None => c"".as_ptr(),
    }
}

#[no_mangle]
pub extern "C" fn odm_shutdown() {
    if let Some(c) = core() {
        c.mgr.shutdown();
    }
}

// ------------------------------------------------------------------ dispatch

fn arg<T: for<'de> Deserialize<'de>>(a: &Value, key: &str) -> Result<T, String> {
    let v = a.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("argument '{key}': {e}"))
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ProbeOpts {
    referrer: Option<String>,
    cookies: Option<String>,
    headers: Vec<(String, String)>,
    username: Option<String>,
    password: Option<String>,
}

fn dispatch(c: &'static Core, m: &str, a: Value) -> Result<Value, String> {
    let mgr = &c.mgr;
    let ids = |a: &Value| arg::<Vec<String>>(a, "ids");
    match m {
        "list_downloads" => ok(mgr.list()),
        "get_download" => ok(mgr.get(&arg::<String>(&a, "id")?)),
        "final_path" => {
            let d = mgr.get(&arg::<String>(&a, "id")?).ok_or("download not found")?;
            let p = d.final_path();
            let part = util::part_path(&p);
            ok(json!({
                "path": p.to_string_lossy(),
                "exists": p.exists(),
                "partPath": part.to_string_lossy(),
                "partExists": part.exists(),
                "dir": d.save_dir,
            }))
        }
        "probe_url" => {
            let url: String = arg(&a, "url")?;
            let o: ProbeOpts = arg::<Option<ProbeOpts>>(&a, "opts")?.unwrap_or_default();
            c.rt
                .block_on(mgr.probe(&url, o.referrer, o.cookies, o.headers, o.username, o.password))
                .map_err(|e| e.to_string())
                .and_then(ok)
        }
        "add_download" => mgr.add(arg(&a, "req")?).map_err(|e| e.to_string()).and_then(ok),
        "add_many" => {
            let reqs: Vec<AddRequest> = arg(&a, "reqs")?;
            let mut out = vec![];
            let mut errs = vec![];
            for r in reqs {
                match mgr.add(r) {
                    Ok(id) => out.push(id),
                    Err(e) => errs.push(e.to_string()),
                }
            }
            if out.is_empty() && !errs.is_empty() {
                return Err(errs.join("; "));
            }
            ok(out)
        }
        "pause" => ok(mgr.pause(&ids(&a)?)),
        "pause_all" => ok(mgr.pause_all()),
        "resume" => ok(mgr.resume(&ids(&a)?)),
        "delete" => ok(mgr.delete(&ids(&a)?, arg::<Option<bool>>(&a, "deleteFiles")?.unwrap_or(false))),
        "delete_completed" => ok(mgr.delete_completed().len()),
        "redownload" => ok(mgr.redownload(&arg::<String>(&a, "id")?)),
        "refresh_link" => {
            let id: String = arg(&a, "id")?;
            let u: String = arg(&a, "newUrl")?;
            c.rt.block_on(mgr.refresh_link(&id, &u)).map_err(|e| e.to_string()).and_then(ok)
        }
        "set_global_limit" => ok(mgr.set_global_limit(arg(&a, "bps")?)),
        "set_download_limit" => ok(mgr.set_download_limit(&arg::<String>(&a, "id")?, arg(&a, "bps")?)),
        "update_download" => {
            let p: DownloadPatch = arg(&a, "patch")?;
            ok(mgr.update_download(&arg::<String>(&a, "id")?, p))
        }
        "set_category" => ok(mgr.set_category(&ids(&a)?, &arg::<String>(&a, "category")?)),
        "assign_extensions" => mgr
            .assign_extensions(&arg::<String>(&a, "category")?, &arg::<Vec<String>>(&a, "extensions")?)
            .map_err(|e| e.to_string())
            .and_then(ok),
        "move_to_queue" => {
            let q: Option<String> = arg(&a, "queueId")?;
            ok(mgr.move_to_queue(&ids(&a)?, q.as_deref()))
        }
        "list_queues" => ok(mgr.queues()),
        "save_queue" => ok(mgr.save_queue(arg(&a, "queue")?)),
        "delete_queue" => mgr.delete_queue(&arg::<String>(&a, "id")?).map_err(|e| e.to_string()).and_then(ok),
        "start_queue" => ok(mgr.start_queue(&arg::<String>(&a, "id")?)),
        "stop_queue" => ok(mgr.stop_queue(&arg::<String>(&a, "id")?)),
        "reorder_queue" => ok(mgr.reorder_queue(&arg::<String>(&a, "id")?, arg(&a, "order")?)),
        "get_settings" => ok(mgr.settings()),
        "default_settings" => ok(Settings::default()),
        "save_settings" => {
            let s: Settings = arg(&a, "settings")?;
            let old_port = mgr.settings().extension_port;
            let new_port = s.extension_port;
            mgr.save_settings(s).map_err(|e| e.to_string())?;
            if old_port != new_port {
                c.rt.spawn(bridge::serve(new_port));
            }
            ok(())
        }
        "recent_folders" => ok(mgr.recent_folders(arg::<Option<usize>>(&a, "limit")?.unwrap_or(4))),
        "totals" => ok(mgr.totals()),
        "target_exists" => ok(mgr.target_exists(&arg::<String>(&a, "dir")?, &arg::<String>(&a, "name")?)),
        "folder_for" => ok(mgr.folder_for(&arg::<String>(&a, "category")?)),
        "expand_batch" => ok(util::expand_batch(&arg::<String>(&a, "pattern")?)),
        "grab" => {
            let (client, s) = mgr.grab_client();
            let headers = client::request_headers(&s.user_agent, None, None, &[], None, None);
            let url: String = arg(&a, "url")?;
            let depth: u32 = arg::<Option<u32>>(&a, "depth")?.unwrap_or(0);
            let exts: Vec<String> = arg::<Option<Vec<String>>>(&a, "extensions")?.unwrap_or_default();
            let same: bool = arg::<Option<bool>>(&a, "sameHost")?.unwrap_or(true);
            c.rt
                .block_on(grabber::grab(
                    &client,
                    &url,
                    headers,
                    grabber::GrabOptions { depth: depth.min(3), max_pages: 60, same_host: same, extensions: exts },
                    &s.categories,
                ))
                .map_err(|e| e.to_string())
                .and_then(ok)
        }
        "extension_status" => {
            let b = c.extension_browsers();
            ok(json!({ "connected": !b.is_empty(), "browsers": b, "port": mgr.settings().extension_port }))
        }
        "export_list" => {
            let path: String = arg(&a, "path")?;
            let only: bool = arg::<Option<bool>>(&a, "onlyUnfinished")?.unwrap_or(false);
            let list: Vec<String> = mgr
                .list()
                .into_iter()
                .filter(|d| !only || d.status != Status::Complete)
                .map(|d| d.url)
                .collect();
            std::fs::write(&path, list.join("\n") + "\n").map_err(|e| e.to_string())?;
            ok(list.len())
        }
        "read_url_list" => {
            let text = std::fs::read_to_string(arg::<String>(&a, "path")?).map_err(|e| e.to_string())?;
            ok(text
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
                .map(String::from)
                .collect::<Vec<_>>())
        }
        "kv_get" => ok(mgr.kv_get(&format!("ui.{}", arg::<String>(&a, "key")?))),
        "kv_set" => ok(mgr.kv_set(&format!("ui.{}", arg::<String>(&a, "key")?), &arg::<String>(&a, "value")?)),
        "version" => ok(env!("CARGO_PKG_VERSION")),
        other => Err(format!("unknown method: {other}")),
    }
}
