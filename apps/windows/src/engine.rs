//! The engine, as the rest of the app sees it: start, call, shutdown.
use serde_json::Value;
use std::path::Path;

#[cfg(feature = "engine")]
pub use odm_ffi::api::{call, shutdown, start};

/// Stand-ins with the engine's signatures, for type-checking the interface
/// without linking the engine (see the `engine` feature).
#[cfg(not(feature = "engine"))]
mod stub {
    use super::*;

    pub fn start(_data_dir: &Path, _on_event: impl Fn(&str, &str) + Send + 'static) -> Result<(), String> {
        Err("this build has no engine".into())
    }

    pub fn call(_method: &str, _args: Value) -> Result<Value, String> {
        Err("this build has no engine".into())
    }

    pub fn shutdown() {}
}
#[cfg(not(feature = "engine"))]
pub use stub::{call, shutdown, start};

/// Where the app keeps its data: `%APPDATA%\app.opendm.desktop` (the same
/// folder the earlier Tauri build used, so the list carries over).
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(|| Path::new(".").to_path_buf());
    base.join("app.opendm.desktop")
}
