//! C interface to the Open Download Manager engine.
//!
//! The native apps (AppKit, WinUI, GTK, Kirigami) are Rust and use the same
//! contract through [`api`]; the C functions remain for front ends in other
//! languages. Every call takes a method name and JSON arguments and returns JSON, so the
//! interface stays small and stable while the engine evolves.
//!
//! Contract (see include/odm.h):
//! * No panic crosses the C boundary: every exported function catches
//!   panics and reports them as an error.
//! * Calls that wait for the engine (probe, grab, refresh, shutdown, start)
//!   are refused with an error when made from a thread that runs async
//!   (tokio) code, instead of panicking.
//! * Events are delivered one at a time, in order, on a single dedicated
//!   thread ("odm-events"). The callback may call `odm_call`.

mod bridge;

use odm_core::manager::DownloadPatch;
use odm_core::*;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

/// Event callback; NULL means "no events".
pub type EventCb = Option<extern "C" fn(name: *const c_char, json: *const c_char, ctx: *mut c_void)>;

/// How long `odm_shutdown` waits for downloads to stop and flush.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

const ON_ASYNC_THREAD: &str =
    "this call blocks and cannot be made from a thread that runs async (engine) code; call it from another thread";

pub(crate) struct Core {
    pub rt: tokio::runtime::Runtime,
    pub mgr: Manager,
    /// events go to the "odm-events" thread, which calls the sink
    events: Option<Mutex<mpsc::Sender<(CString, CString)>>>,
    pub ext_seen: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// stops the running extension bridge listener
    pub bridge: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// last bind failure of the bridge (None while it listens)
    pub bridge_error: Mutex<Option<String>>,
    shut_down: AtomicBool,
}

static CORE: OnceLock<Core> = OnceLock::new();
/// serializes odm_start so concurrent/duplicate starts are safe
static START: Mutex<()> = Mutex::new(());

thread_local! {
    /// odm_last_error() text; per thread, valid until the next call into the
    /// library on the same thread
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

pub(crate) fn core() -> Option<&'static Core> {
    CORE.get()
}

impl Core {
    pub fn emit(&self, name: &str, payload: &Value) {
        let Some(tx) = &self.events else { return };
        let (Ok(n), Ok(j)) = (CString::new(name), CString::new(payload.to_string())) else { return };
        let _ = tx.lock().send((n, j));
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

fn set_error(msg: impl Into<String>) {
    let s = CString::new(msg.into().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = s);
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

/// True when the current thread runs (or has entered) a tokio runtime, where
/// `Runtime::block_on` would panic.
fn on_async_thread() -> bool {
    tokio::runtime::Handle::try_current().is_ok()
}

/// Receives (event name, JSON payload).
type Sink = Box<dyn Fn(&CStr, &CStr) + Send>;

/// Deliver events in order on one thread. No sink: no thread, no events.
fn spawn_event_thread(sink: Option<Sink>) -> Option<Mutex<mpsc::Sender<(CString, CString)>>> {
    let sink = sink?;
    let (tx, rx) = mpsc::channel::<(CString, CString)>();
    std::thread::Builder::new()
        .name("odm-events".into())
        .spawn(move || {
            for (n, j) in rx {
                let _ = catch_unwind(AssertUnwindSafe(|| sink(&n, &j)));
            }
        })
        .ok()?;
    Some(Mutex::new(tx))
}

/// # Safety
/// `data_dir` must be a valid NUL-terminated UTF-8 string. `ctx` is passed
/// back to `cb` unchanged and must stay valid while the engine runs.
#[no_mangle]
pub unsafe extern "C" fn odm_start(data_dir: *const c_char, cb: EventCb, ctx: *mut c_void) -> i32 {
    let ctx = ctx as usize;
    match catch_unwind(AssertUnwindSafe(|| start(data_dir, cb, ctx))) {
        Ok(code) => code,
        Err(p) => {
            set_error(format!("internal error: {}", panic_text(&*p)));
            -5
        }
    }
}

unsafe fn start(data_dir: *const c_char, cb: EventCb, ctx: usize) -> i32 {
    if data_dir.is_null() {
        set_error("data_dir is null");
        return -1;
    }
    let dir = match CStr::from_ptr(data_dir).to_str() {
        Ok(s) => std::path::PathBuf::from(s),
        Err(_) => {
            set_error("data_dir is not UTF-8");
            return -1;
        }
    };
    let sink = cb.map(|cb| -> Sink {
        Box::new(move |n: &CStr, j: &CStr| cb(n.as_ptr(), j.as_ptr(), ctx as *mut c_void))
    });
    match start_with(dir, sink) {
        Ok(()) => 0,
        Err((code, msg)) => {
            set_error(msg);
            code
        }
    }
}

/// Start the engine once (later calls succeed and ignore their sink).
fn start_with(dir: std::path::PathBuf, sink: Option<Sink>) -> Result<(), (i32, String)> {
    let _once = START.lock();
    if CORE.get().is_some() {
        return Ok(());
    }
    if on_async_thread() {
        return Err((-6, format!("odm_start: {ON_ASYNC_THREAD}")));
    }
    std::fs::create_dir_all(&dir).map_err(|e| (-2, format!("cannot create {}: {e}", dir.display())))?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("odm-engine")
        .build()
        .map_err(|e| (-3, format!("runtime: {e}")))?;
    // before the database is opened: what is stored from here on is sealed
    // with the keystore's key (not under test: tests have no keystore)
    if !cfg!(feature = "test-hooks") && odm_core::secret::keystore_wanted() {
        odm_core::secret::use_keystore();
    }
    let mgr = rt.block_on(async { Manager::open(&dir.join("odm.db")) }).map_err(|e| (-4, e.to_string()))?;
    let core = Core {
        rt,
        mgr: mgr.clone(),
        events: spawn_event_thread(sink),
        ext_seen: Mutex::new(Default::default()),
        bridge: Mutex::new(None),
        bridge_error: Mutex::new(None),
        shut_down: AtomicBool::new(false),
    };
    if CORE.set(core).is_err() {
        return Ok(());
    }
    let c = CORE.get().expect("core set");
    c.rt.block_on(async { c.mgr.start_background() });
    c.rt.spawn(forward_events());
    // a bind error is reported as a bridge_error event and in extension_status
    let _ = bridge::start(c.mgr.settings().extension_port);
    Ok(())
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
    let out = catch_unwind(AssertUnwindSafe(|| call(method, args)))
        .unwrap_or_else(|p| Err(format!("internal error: {}", panic_text(&*p))));
    let v = match out {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e }),
    };
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

unsafe fn call(method: *const c_char, args: *const c_char) -> Result<Value, String> {
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
    call_value(m, a)
}

fn call_value(m: &str, a: Value) -> Result<Value, String> {
    let c = core().ok_or("engine not started")?;
    // decided before entering the runtime below
    let cx = Cx { c, can_block: !on_async_thread() };
    let _g = c.rt.enter();
    dispatch(&cx, m, a)
}

/// # Safety
/// `s` must come from `odm_call` and be freed only once.
#[no_mangle]
pub unsafe extern "C" fn odm_free(s: *mut c_char) {
    if !s.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(CString::from_raw(s))));
    }
}

/// Last error from `odm_start`/`odm_shutdown` on the calling thread ("" if
/// none). Owned by the library: do not free; valid until the next call into
/// the library on the same thread.
#[no_mangle]
pub extern "C" fn odm_last_error() -> *const c_char {
    catch_unwind(|| LAST_ERROR.with(|e| e.borrow().as_ptr())).unwrap_or(c"".as_ptr())
}

/// Stop the bridge and the downloads, waiting up to 5 s for them to flush,
/// then persist state. Runs once; later calls return at once.
#[no_mangle]
pub extern "C" fn odm_shutdown() {
    shutdown_impl();
}

fn shutdown_impl() {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let Some(c) = core() else { return };
        if c.shut_down.swap(true, Ordering::SeqCst) {
            return;
        }
        bridge::stop();
        if on_async_thread() {
            // cannot wait here: save state without waiting for writers
            set_error(format!("odm_shutdown: {ON_ASYNC_THREAD} (state saved without waiting)"));
            c.mgr.shutdown();
            return;
        }
        let waited = c.rt.block_on(async {
            tokio::time::timeout(SHUTDOWN_TIMEOUT + Duration::from_secs(1), c.mgr.shutdown_wait(SHUTDOWN_TIMEOUT)).await
        });
        if waited.is_err() {
            set_error("odm_shutdown: timed out waiting for downloads to stop");
        }
    }));
    if let Err(p) = r {
        set_error(format!("internal error: {}", panic_text(&*p)));
    }
}

// ------------------------------------------------------------------ dispatch

struct Cx {
    c: &'static Core,
    /// false when called from an async thread: blocking calls are refused
    can_block: bool,
}

impl Cx {
    fn block_on<F: std::future::Future>(&self, f: F) -> Result<F::Output, String> {
        if !self.can_block {
            return Err(ON_ASYNC_THREAD.into());
        }
        Ok(self.c.rt.block_on(f))
    }
}

fn arg<T: for<'de> Deserialize<'de>>(a: &Value, key: &str) -> Result<T, String> {
    let v = a.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("argument '{key}': {e}"))
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn done() -> Result<Value, String> {
    Ok(Value::Null)
}

fn absolute(path: &str) -> Result<&std::path::Path, String> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err("path must be absolute".into())
    }
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

fn dispatch(cx: &Cx, m: &str, a: Value) -> Result<Value, String> {
    let c = cx.c;
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
            cx.block_on(mgr.probe(&url, o.referrer, o.cookies, o.headers, o.username, o.password))?
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
        "pause" => {
            mgr.pause(&ids(&a)?);
            done()
        }
        "pause_all" => {
            mgr.pause_all();
            done()
        }
        "resume" => {
            mgr.resume(&ids(&a)?);
            done()
        }
        "delete" => {
            mgr.delete(&ids(&a)?, arg::<Option<bool>>(&a, "deleteFiles")?.unwrap_or(false));
            done()
        }
        "delete_completed" => ok(mgr.delete_completed().len()),
        "redownload" => {
            mgr.redownload(&arg::<String>(&a, "id")?);
            done()
        }
        "refresh_link" => {
            let id: String = arg(&a, "id")?;
            let u: String = arg(&a, "newUrl")?;
            cx.block_on(mgr.refresh_link(&id, &u))?.map_err(|e| e.to_string()).and_then(ok)
        }
        "set_global_limit" => {
            mgr.set_global_limit(arg(&a, "bps")?);
            done()
        }
        "set_download_limit" => {
            mgr.set_download_limit(&arg::<String>(&a, "id")?, arg(&a, "bps")?);
            done()
        }
        "update_download" => {
            let p: DownloadPatch = arg(&a, "patch")?;
            mgr.update_download(&arg::<String>(&a, "id")?, p);
            done()
        }
        "set_category" => {
            mgr.set_category(&ids(&a)?, &arg::<String>(&a, "category")?);
            done()
        }
        "assign_extensions" => mgr
            .assign_extensions(&arg::<String>(&a, "category")?, &arg::<Vec<String>>(&a, "extensions")?)
            .map_err(|e| e.to_string())
            .and_then(ok),
        "move_to_queue" => {
            let q: Option<String> = arg(&a, "queueId")?;
            mgr.move_to_queue(&ids(&a)?, q.as_deref());
            done()
        }
        "list_queues" => ok(mgr.queues()),
        "save_queue" => ok(mgr.save_queue(arg(&a, "queue")?)),
        "delete_queue" => mgr.delete_queue(&arg::<String>(&a, "id")?).map_err(|e| e.to_string()).and_then(ok),
        "start_queue" => {
            mgr.start_queue(&arg::<String>(&a, "id")?);
            done()
        }
        "stop_queue" => {
            mgr.stop_queue(&arg::<String>(&a, "id")?);
            done()
        }
        "reorder_queue" => {
            mgr.reorder_queue(&arg::<String>(&a, "id")?, arg(&a, "order")?);
            done()
        }
        "get_settings" => ok(mgr.settings()),
        "default_settings" => ok(Settings::default()),
        "save_settings" => {
            let s: Settings = arg(&a, "settings")?;
            let old_port = mgr.settings().extension_port;
            let new_port = s.extension_port;
            // reject bad settings before anything changes
            mgr.validate_settings(&s).map_err(|e| e.to_string())?;
            // listen on the new port first: if it is taken nothing is saved
            // and the old listener keeps running
            if old_port != new_port {
                bridge::start(new_port)?;
            }
            mgr.save_settings(s).map_err(|e| e.to_string())?;
            done()
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
            cx.block_on(grabber::grab(
                &client,
                &url,
                headers,
                grabber::GrabOptions { depth: depth.min(3), max_pages: 60, same_host: same, extensions: exts },
                &s.categories,
            ))?
            .map_err(|e| e.to_string())
            .and_then(ok)
        }
        "extension_status" => {
            let b = c.extension_browsers();
            ok(json!({
                "connected": !b.is_empty(),
                "browsers": b,
                "port": mgr.settings().extension_port,
                "bridgeError": c.bridge_error.lock().clone(),
            }))
        }
        // the key the browser extension is paired with (shown in settings)
        "bridge_key" => ok(mgr.bridge_key()),
        "regenerate_bridge_key" => {
            let key = mgr.regenerate_bridge_key();
            // nothing is paired any more
            c.ext_seen.lock().clear();
            c.emit("extension_status", &json!({ "connected": false, "browsers": [] }));
            ok(key)
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
            std::fs::write(absolute(&path)?, list.join("\n") + "\n").map_err(|e| e.to_string())?;
            ok(list.len())
        }
        "read_url_list" => {
            let path: String = arg(&a, "path")?;
            let text = std::fs::read_to_string(absolute(&path)?).map_err(|e| e.to_string())?;
            ok(text
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("http://") || l.starts_with("https://"))
                .map(String::from)
                .collect::<Vec<_>>())
        }
        // a read error is an error, not "nothing stored"
        "kv_get" => ok(mgr.kv_read(&format!("ui.{}", arg::<String>(&a, "key")?)).map_err(|e| e.to_string())?),
        "kv_set" => {
            mgr.kv_set(&format!("ui.{}", arg::<String>(&a, "key")?), &arg::<String>(&a, "value")?);
            done()
        }
        "version" => ok(env!("CARGO_PKG_VERSION")),
        // test hook: proves panics are caught at the C boundary
        #[cfg(feature = "test-hooks")]
        "__panic" => panic!("test panic"),
        other => Err(format!("unknown method: {other}")),
    }
}

/// The same contract as the C functions, for front ends written in Rust
/// (the GTK and Kirigami apps). Method names, arguments, results and event
/// payloads are exactly those of `odm_call` and the event callback.
pub mod api {
    use serde_json::Value;
    use std::path::Path;

    /// Start the engine and the browser bridge. `on_event(name, json)` runs
    /// on the "odm-events" thread, one event at a time, in order; it may
    /// call [`call`]. Later calls succeed and ignore their callback.
    pub fn start(data_dir: &Path, on_event: impl Fn(&str, &str) + Send + 'static) -> Result<(), String> {
        let sink: super::Sink = Box::new(move |n: &std::ffi::CStr, j: &std::ffi::CStr| {
            on_event(&n.to_string_lossy(), &j.to_string_lossy())
        });
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::start_with(data_dir.to_path_buf(), Some(sink))))
            .map_err(|p| format!("internal error: {}", super::panic_text(&*p)))?
            .map_err(|(_, msg)| msg)
    }

    /// Call an engine method (see `odm_call`). Blocking methods (probe_url,
    /// grab, refresh_link) must not be called from the GUI thread if that
    /// would freeze it, and are refused on engine threads.
    pub fn call(method: &str, args: Value) -> Result<Value, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::call_value(method, args)))
            .unwrap_or_else(|p| Err(format!("internal error: {}", super::panic_text(&*p))))
    }

    /// Typed convenience over [`call`].
    pub fn call_as<T: serde::de::DeserializeOwned>(method: &str, args: Value) -> Result<T, String> {
        serde_json::from_value(call(method, args)?).map_err(|e| format!("{method}: unexpected reply: {e}"))
    }

    /// Stop the bridge and downloads, waiting up to 5 s for them to flush.
    /// Runs once. Call it from a thread that doesn't run async code.
    pub fn shutdown() {
        super::odm_shutdown()
    }
}
