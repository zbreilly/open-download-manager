//! Secrets at rest (cookies, passwords, the pairing key).
//!
//! On Windows a secret is encrypted with DPAPI, which ties it to the user's
//! logon: another account, or a copy of the database on another machine,
//! cannot read it.
//!
//! On macOS and Linux a secret is encrypted (AES-256-GCM) with a key that
//! the system's keystore holds: the login Keychain, or the Secret Service
//! (GNOME Keyring, KWallet). Another program running as the same user can
//! read the database, but not that key without the keystore letting it.
//! The app turns this on with [`use_keystore`]; until then, and wherever
//! there is no keystore, a value is stored as it is (the database is
//! private to the user's account).
//!
//! A keystore can be there and not answer: the user declines the Keychain
//! prompt, the keyring is still locked at login. Secrets sealed earlier
//! cannot be read then ([`locked`]); they are left in the database untouched
//! and are readable again the next time the keystore answers.

use crate::util;

/// Marks a value sealed by [`seal`] on Windows.
const PREFIX: &str = "odm-dpapi1:";
/// Marks a value sealed with the keystore's key: base64 of the nonce, the
/// ciphertext and its tag.
pub const KEY_PREFIX: &str = "odm-key1:";

/// A stored value, opened.
pub struct Opened {
    pub value: String,
    /// the stored form was not protected although this platform protects
    /// secrets (written by an older version): save it again
    pub was_plain: bool,
    /// the value is sealed with the keystore's key and the keystore did not
    /// answer: `value` is empty, and the stored form must be kept
    pub unreadable: bool,
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
    if let Some(sealed) = stored.strip_prefix(KEY_PREFIX) {
        return match keystore::key() {
            Some(key) => Opened { value: keystore::open(&key, sealed).unwrap_or_default(), was_plain: false, unreadable: false },
            None => Opened { value: String::new(), was_plain: false, unreadable: true },
        };
    }
    match stored.strip_prefix(PREFIX) {
        Some(hex) => Opened { value: open_impl(hex).unwrap_or_default(), was_plain: false, unreadable: false },
        None => Opened { value: stored.to_string(), was_plain: protects() && !stored.is_empty(), unreadable: false },
    }
}

/// Does [`seal`] protect what it is given right now?
fn protects() -> bool {
    cfg!(windows) || keystore::key().is_some()
}

/// Is `stored` a value sealed with the keystore's key?
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(KEY_PREFIX)
}

/// What became of asking the keystore for the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keystore {
    /// secrets are sealed with the keystore's key
    Ready,
    /// there is a keystore and it did not answer (declined, locked, timed
    /// out): secrets sealed earlier stay unread and untouched
    Locked,
    /// no keystore on this system: secrets are stored as they are
    Absent,
}

/// Seal secrets with a key the system's keystore holds, from now on. Call
/// once, before the database is opened. Does nothing on Windows (DPAPI needs
/// no key of ours). Asking may show the system's prompt for access.
pub fn use_keystore() -> Keystore {
    keystore::start()
}

/// There is no keystore key in this run (the keystore did not answer, or
/// was not asked): secrets sealed earlier cannot be read, and must be left
/// as they are stored.
pub fn locked() -> bool {
    !cfg!(windows) && keystore::key().is_none()
}

/// Whether an app should seal with the keystore: in the builds people use,
/// not in development builds (each rebuild is a new program to the Keychain,
/// which would ask again every time) and not in automated runs, which have
/// nobody to answer a prompt.
pub fn keystore_wanted() -> bool {
    !cfg!(debug_assertions) && std::env::var_os(crate::manager::PAIRING_KEY_ENV).is_none()
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
    match keystore::key() {
        Some(key) => keystore::seal(&key, plain),
        None => plain.to_string(),
    }
}

#[cfg(not(windows))]
fn open_impl(_hex: &str) -> Option<String> {
    // sealed on a Windows machine: not readable here
    let _ = util::from_hex;
    None
}

/// The key the system's keystore holds, and sealing with it.
mod keystore {
    use super::{Keystore, KEY_PREFIX};
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use parking_lot::RwLock;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
    use ring::rand::{SecureRandom, SystemRandom};

    pub const KEY_LEN: usize = 32;
    const AAD: &[u8] = b"odm-secret-v1";

    enum State {
        /// [`start`] was not called, or there is no keystore
        Off,
        Ready([u8; KEY_LEN]),
        Locked,
    }

    static STATE: RwLock<State> = RwLock::new(State::Off);

    pub fn key() -> Option<[u8; KEY_LEN]> {
        match &*STATE.read() {
            State::Ready(k) => Some(*k),
            _ => None,
        }
    }

    /// What a keystore said when asked for the key (no platform's keystore
    /// gives every one of these answers).
    #[allow(dead_code)]
    pub enum Lookup {
        Found(Vec<u8>),
        /// the keystore answered and has no key of ours
        Missing,
        /// the keystore did not answer, or refused
        Unavailable,
        /// this system has no keystore
        Absent,
    }

    fn new_key() -> Option<[u8; KEY_LEN]> {
        let mut k = [0u8; KEY_LEN];
        SystemRandom::new().fill(&mut k).ok()?;
        Some(k)
    }

    /// The key from `lookup`, made and handed to `store` first if the
    /// keystore has none. A key is only used once the keystore holds it.
    pub fn resolve(lookup: Lookup, store: impl FnOnce(&[u8; KEY_LEN]) -> bool) -> (Keystore, Option<[u8; KEY_LEN]>) {
        match lookup {
            Lookup::Found(bytes) => match <[u8; KEY_LEN]>::try_from(bytes.as_slice()) {
                Ok(k) => (Keystore::Ready, Some(k)),
                // not a key of ours: leave it alone and stay out
                Err(_) => (Keystore::Locked, None),
            },
            Lookup::Missing => match new_key() {
                Some(k) if store(&k) => (Keystore::Ready, Some(k)),
                _ => (Keystore::Locked, None),
            },
            Lookup::Unavailable => (Keystore::Locked, None),
            Lookup::Absent => (Keystore::Absent, None),
        }
    }

    pub fn start() -> Keystore {
        let (status, key) = resolve(backend::lookup(), backend::store);
        *STATE.write() = match (status, key) {
            (Keystore::Ready, Some(k)) => State::Ready(k),
            (Keystore::Locked, _) => {
                tracing::warn!("the system keystore did not answer: stored cookies and passwords stay sealed and unread in this run");
                State::Locked
            }
            _ => State::Off,
        };
        status
    }

    /// For tests: seal with `key` (or stop sealing, or act locked).
    #[doc(hidden)]
    pub fn set_for_tests(key: Option<[u8; KEY_LEN]>, locked: bool) {
        *STATE.write() = match key {
            Some(k) => State::Ready(k),
            None if locked => State::Locked,
            None => State::Off,
        };
    }

    #[cfg_attr(windows, allow(dead_code))]
    pub fn seal(key: &[u8; KEY_LEN], plain: &str) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        let sealed = SystemRandom::new().fill(&mut nonce).ok().and_then(|_| {
            let aead = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
            let mut buf = plain.as_bytes().to_vec();
            aead.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(AAD), &mut buf).ok()?;
            let mut out = nonce.to_vec();
            out.extend_from_slice(&buf);
            Some(format!("{KEY_PREFIX}{}", B64.encode(out)))
        });
        // a secret that cannot be protected is not stored
        sealed.unwrap_or_else(|| {
            tracing::warn!("a secret could not be protected and was not stored");
            String::new()
        })
    }

    /// The secret in `sealed` (the part after the prefix). `None`: sealed
    /// with another key, or damaged.
    pub fn open(key: &[u8; KEY_LEN], sealed: &str) -> Option<String> {
        let opened = (|| {
            let mut bytes = B64.decode(sealed).ok()?;
            if bytes.len() < NONCE_LEN {
                return None;
            }
            let mut buf = bytes.split_off(NONCE_LEN);
            let nonce: [u8; NONCE_LEN] = bytes.try_into().ok()?;
            let aead = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
            let plain = aead.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(AAD), &mut buf).ok()?;
            String::from_utf8(plain.to_vec()).ok()
        })();
        if opened.is_none() {
            tracing::warn!("a stored secret could not be read (the keystore's key has changed) and was dropped");
        }
        opened
    }

    /// The login Keychain: a generic password that belongs to the app that
    /// made it. Another program asking for it gets the system's prompt.
    #[cfg(target_os = "macos")]
    mod backend {
        use super::{Lookup, KEY_LEN};
        use security_framework::passwords::{get_generic_password, set_generic_password};

        const SERVICE: &str = "app.opendm.desktop";
        const ACCOUNT: &str = "secrets key";
        /// errSecItemNotFound
        const NOT_FOUND: i32 = -25300;

        pub fn lookup() -> Lookup {
            match get_generic_password(SERVICE, ACCOUNT) {
                Ok(bytes) => Lookup::Found(bytes),
                Err(e) if e.code() == NOT_FOUND => Lookup::Missing,
                Err(e) => {
                    tracing::warn!("keychain: {e}");
                    Lookup::Unavailable
                }
            }
        }

        pub fn store(key: &[u8; KEY_LEN]) -> bool {
            set_generic_password(SERVICE, ACCOUNT, key).is_ok()
        }
    }

    /// The Secret Service (GNOME Keyring, KWallet) on the session bus.
    #[cfg(target_os = "linux")]
    mod backend {
        pub use super::secret_service::{lookup, store};
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    mod backend {
        use super::{Lookup, KEY_LEN};

        pub fn lookup() -> Lookup {
            Lookup::Absent
        }

        pub fn store(_key: &[u8; KEY_LEN]) -> bool {
            false
        }
    }

    #[cfg(target_os = "linux")]
    mod secret_service {
        use super::{Lookup, KEY_LEN};
        use std::collections::HashMap;
        use std::time::Duration;
        use zbus::blocking::Connection;
        use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

        const DEST: &str = "org.freedesktop.secrets";
        const PATH: &str = "/org/freedesktop/secrets";
        const SERVICE: &str = "org.freedesktop.Secret.Service";
        const COLLECTION: &str = "org.freedesktop.Secret.Collection";
        const ITEM: &str = "org.freedesktop.Secret.Item";
        /// the keyring new items go to (the login keyring)
        const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
        /// How long the keystore gets to answer. Unlocking that needs the
        /// user is not waited for: the app starts without the key instead.
        const PATIENCE: Duration = Duration::from_secs(8);

        /// A secret as the service passes it: session, parameters, value,
        /// content type.
        type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

        fn attributes() -> HashMap<&'static str, &'static str> {
            HashMap::from([("application", "app.opendm.desktop"), ("purpose", "secrets-key")])
        }

        /// Run `job` on a thread of its own (the bus library must not be
        /// called from inside an async runtime) and give up after a while.
        fn patiently<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("odm-keystore".into())
                .spawn(move || {
                    let _ = tx.send(job());
                })
                .ok()?;
            rx.recv_timeout(PATIENCE).ok()
        }

        /// A session in which secrets cross the bus as they are (the bus is
        /// local to the user's session).
        fn session(conn: &Connection) -> zbus::Result<OwnedObjectPath> {
            let reply = conn.call_method(Some(DEST), PATH, Some(SERVICE), "OpenSession", &("plain", Value::from("")))?;
            let (_, session): (OwnedValue, OwnedObjectPath) = reply.body().deserialize()?;
            Ok(session)
        }

        fn find(conn: &Connection) -> zbus::Result<Lookup> {
            let reply = conn.call_method(Some(DEST), PATH, Some(SERVICE), "SearchItems", &(attributes(),))?;
            let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = reply.body().deserialize()?;
            let item = match unlocked.into_iter().next() {
                Some(item) => item,
                None if locked.is_empty() => return Ok(Lookup::Missing),
                None => {
                    // unlocks without asking where it can (a keyring opened at login)
                    let reply = conn.call_method(Some(DEST), PATH, Some(SERVICE), "Unlock", &(locked,))?;
                    let (opened, _prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = reply.body().deserialize()?;
                    match opened.into_iter().next() {
                        Some(item) => item,
                        None => return Ok(Lookup::Unavailable),
                    }
                }
            };
            let session = session(conn)?;
            let reply = conn.call_method(Some(DEST), item.as_str(), Some(ITEM), "GetSecret", &(session,))?;
            let (secret,): (Secret,) = reply.body().deserialize()?;
            Ok(Lookup::Found(secret.2))
        }

        pub fn lookup() -> Lookup {
            let found = patiently(|| {
                // no session bus (a console, a service): nothing to ask
                let Ok(conn) = Connection::session() else { return Lookup::Absent };
                match find(&conn) {
                    Ok(l) => l,
                    // nobody provides the service on this desktop
                    Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => Lookup::Absent,
                    Err(e) => {
                        tracing::warn!("secret service: {e}");
                        Lookup::Unavailable
                    }
                }
            });
            found.unwrap_or(Lookup::Unavailable)
        }

        fn create(conn: &Connection, key: &[u8; KEY_LEN]) -> zbus::Result<bool> {
            let session = session(conn)?;
            let mut properties: HashMap<&str, Value> = HashMap::new();
            properties.insert("org.freedesktop.Secret.Item.Label", Value::from("Open Download Manager"));
            properties.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attributes()));
            let secret = (ObjectPath::from(session), Vec::<u8>::new(), key.to_vec(), "application/octet-stream");
            let reply = conn.call_method(Some(DEST), DEFAULT_COLLECTION, Some(COLLECTION), "CreateItem", &(properties, secret, true))?;
            let (item, _prompt): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize()?;
            // "/" means the keyring wants to ask the user first: not stored
            Ok(item.as_str() != "/")
        }

        pub fn store(key: &[u8; KEY_LEN]) -> bool {
            let key = *key;
            let stored = patiently(move || {
                let conn = Connection::session().ok()?;
                create(&conn, &key).ok()
            });
            stored.flatten().unwrap_or(false)
        }
    }
}

#[doc(hidden)]
pub use keystore::set_for_tests as set_keystore_for_tests;

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
    fn keystore_answers() {
        use super::keystore::{resolve, Lookup};
        let never = |_: &[u8; 32]| -> bool { panic!("a key was stored") };
        // the key the keystore holds is the key
        assert_eq!(resolve(Lookup::Found(vec![5; 32]), never), (Keystore::Ready, Some([5; 32])));
        // something else under our name: not used, not replaced
        assert_eq!(resolve(Lookup::Found(vec![5; 7]), never), (Keystore::Locked, None));
        // no answer: never a new key, which would orphan what is sealed
        assert_eq!(resolve(Lookup::Unavailable, never), (Keystore::Locked, None));
        assert_eq!(resolve(Lookup::Absent, never), (Keystore::Absent, None));
        // none yet: a new one, used only once the keystore has it
        let (status, key) = resolve(Lookup::Missing, |_| true);
        assert!(status == Keystore::Ready && key.is_some_and(|k| k != [0; 32]));
        assert_eq!(resolve(Lookup::Missing, |_| false), (Keystore::Locked, None));
    }

    #[test]
    fn sealing_with_a_key() {
        use super::keystore::{open, seal};
        let sealed = seal(&[1; 32], "sid=abc; token=é✓");
        assert!(sealed.starts_with(KEY_PREFIX) && !sealed.contains("abc"), "{sealed}");
        assert!(is_sealed(&sealed));
        let body = sealed.strip_prefix(KEY_PREFIX).unwrap();
        assert_eq!(open(&[1; 32], body).as_deref(), Some("sid=abc; token=é✓"));
        // another key, a damaged value, something that is not one
        assert_eq!(open(&[2; 32], body), None);
        assert_eq!(open(&[1; 32], &body[..body.len() - 4]), None);
        assert_eq!(open(&[1; 32], "AAAA"), None);
        // the same secret never seals to the same text
        assert_ne!(seal(&[1; 32], "x"), seal(&[1; 32], "x"));
    }

    #[test]
    fn damaged_is_dropped() {
        assert_eq!(open(&format!("{PREFIX}zz")).value, "");
        assert_eq!(open(&format!("{PREFIX}00ff00")).value, "");
    }
}
