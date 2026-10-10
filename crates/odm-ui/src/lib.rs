//! What the native front ends have in common, independent of any toolkit
//! and of the engine crate itself: they keep a [`store::Store`] up to date
//! from engine events, ask it which rows to show, and format values with
//! [`fmt`]. Engine calls stay with the front end (`odm_ffi::api`), so this
//! crate builds anywhere.

pub mod add;
pub mod fmt;
pub mod incoming;
pub mod settings;
pub mod store;

/// Human-readable name.
pub const APP_NAME: &str = "Open Download Manager";

use serde_json::Value;

/// A string field ("" when missing or not a string).
pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A non-negative number field (0 when missing).
pub fn n(v: &Value, key: &str) -> u64 {
    opt_n(v, key).unwrap_or(0)
}

/// A non-negative number field. Sizes past 2^53 that arrive as floats
/// (the "until the end" marker of a segment) saturate.
pub fn opt_n(v: &Value, key: &str) -> Option<u64> {
    let x = v.get(key)?;
    x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.0).map(|f| if f >= 1.8e19 { u64::MAX } else { f as u64 }))
}

/// A boolean field (false when missing).
pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A boolean field with a default for when it is missing.
pub fn b_or(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// The strings of an array field.
pub fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(|e| e.as_str().map(String::from)).collect()
}
