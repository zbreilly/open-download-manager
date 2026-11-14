//! The engine, as the rest of the app sees it: start, call, shutdown, and
//! calls that run off the main thread and answer on it.
pub use odm_ffi::api::{call, shutdown, start};
use serde_json::Value;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

/// Where the app keeps its data: `~/Library/Application Support/
/// app.opendm.desktop` (the folder the earlier builds used, so the list
/// carries over). Debug builds take `ODM_DATA_DIR` instead, to try the app
/// without touching the real list.
pub fn data_dir() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(d) = std::env::var_os("ODM_DATA_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support/app.opendm.desktop")
}

/// A call whose result is only read (settings lookups and the like).
pub fn get(method: &str, args: Value) -> Option<Value> {
    call(method, args).ok()
}

pub fn get_string(method: &str, args: Value) -> Option<String> {
    get(method, args).and_then(|v| v.as_str().map(String::from))
}

type Done = Box<dyn FnOnce(Box<dyn Any + Send>)>;

thread_local! {
    /// What to do with the results of jobs in flight. The closures stay on
    /// the main thread; only their number travels to the worker and back.
    static PENDING: RefCell<(u64, HashMap<u64, Done>)> = RefCell::new((0, HashMap::new()));
}

/// Run `job` on a worker thread and hand its result to `done` on the main
/// thread. For engine calls that may block (probe_url, grab, refresh_link)
/// and for sequences of calls.
pub fn background<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let id = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        p.0 += 1;
        let id = p.0;
        p.1.insert(
            id,
            Box::new(move |result: Box<dyn Any + Send>| {
                if let Ok(result) = result.downcast::<T>() {
                    done(*result);
                }
            }),
        );
        id
    });
    let _ = std::thread::Builder::new().name("odm-call".into()).spawn(move || {
        let result: Box<dyn Any + Send> = Box::new(job());
        dispatch2::DispatchQueue::main().exec_async(move || {
            let done = PENDING.with(|p| p.borrow_mut().1.remove(&id));
            if let Some(done) = done {
                done(result);
            }
        });
    });
}

/// One engine call off the main thread, its result handed to `done` on it.
pub fn spawn(method: &'static str, args: Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    background(move || call(method, args), done);
}

/// Fire an engine call off the main thread; an error is shown to the user.
pub fn act(method: &'static str, args: Value) {
    spawn(method, args, |r| {
        if let Err(e) = r {
            crate::app::app().report(&e);
        }
    });
}
