//! Secrets at rest (cookies, passwords, the pairing key).
//!
//! On Windows a secret is encrypted with DPAPI, which ties it to the user's
//! logon: another account, or a copy of the database on another machine,
//! cannot read it. On other platforms the value is stored as it is (the
//! database lives in the user's own profile).

use crate::util;

/// Marks a value sealed by [`seal`] on Windows.
const PREFIX: &str = "odm-dpapi1:";

/// A stored value, opened.
pub struct Opened {
    pub value: String,
    /// the stored form was not protected although this platform protects
    /// secrets (written by an older version): save it again
    pub was_plain: bool,
}

/// The form of `plain` to store. If the secret cannot be protected it is not
/// stored at all (empty string).
pub fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    seal_impl(plain)
}

/// The secret behind a stored value. A protected value that cannot be opened
/// (other user, other machine, damaged) yields an empty string.
pub fn open(stored: &str) -> Opened {
    match stored.strip_prefix(PREFIX) {
        Some(hex) => Opened { value: open_impl(hex).unwrap_or_default(), was_plain: false },
        None => Opened { value: stored.to_string(), was_plain: cfg!(windows) && !stored.is_empty() },
    }
}

#[cfg(windows)]
fn seal_impl(plain: &str) -> String {
    match dpapi::run(plain.as_bytes(), true) {
        Some(blob) => format!("{PREFIX}{}", util::to_hex(&blob)),
        None => {
            tracing::warn!("a secret could not be protected and was not stored");
            String::new()
        }
    }
}

#[cfg(windows)]
fn open_impl(hex: &str) -> Option<String> {
    let opened = util::from_hex(hex).and_then(|blob| dpapi::run(&blob, false)).and_then(|b| String::from_utf8(b).ok());
    if opened.is_none() {
        tracing::warn!("a stored secret could not be read (it belongs to another user or machine) and was dropped");
    }
    opened
}

#[cfg(not(windows))]
fn seal_impl(plain: &str) -> String {
    plain.to_string()
}

#[cfg(not(windows))]
fn open_impl(_hex: &str) -> Option<String> {
    // sealed on a Windows machine: not readable here
    let _ = util::from_hex;
    None
}

#[cfg(windows)]
mod dpapi {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Protect (`true`) or unprotect `data` for the current user.
    pub fn run(data: &[u8], protect: bool) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len()).ok()?, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: `input` points at `data`, which outlives the call and is
        // only read; `out` is filled by the system with a LocalAlloc buffer
        // that is copied and freed below.
        let ok = unsafe {
            if protect {
                CryptProtectData(&input, null(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, null_mut(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            }
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        // SAFETY: the system returned `cbData` valid bytes at `pbData`.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        // SAFETY: `pbData` was allocated by the system with LocalAlloc.
        unsafe {
            LocalFree(out.pbData as _);
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let sealed = seal("sid=abc; token=é✓");
        if cfg!(windows) {
            assert!(sealed.starts_with(PREFIX), "{sealed}");
            assert!(!sealed.contains("abc"));
        }
        let o = open(&sealed);
        assert_eq!(o.value, "sid=abc; token=é✓");
        assert!(!o.was_plain);
    }

    #[test]
    fn empty_and_plain() {
        assert_eq!(seal(""), "");
        let o = open("");
        assert_eq!(o.value, "");
        assert!(!o.was_plain);
        let o = open("left by an older version");
        assert_eq!(o.value, "left by an older version");
        assert_eq!(o.was_plain, cfg!(windows));
    }

    #[test]
    fn damaged_is_dropped() {
        assert_eq!(open(&format!("{PREFIX}zz")).value, "");
        assert_eq!(open(&format!("{PREFIX}00ff00")).value, "");
    }
}
