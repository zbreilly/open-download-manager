//! Pairing between the app and its browser extension.
//!
//! The app holds a random 256-bit key. The user copies it (as the "pairing
//! code") from the app's settings into the extension once. From then on
//! every message between the two is sealed with AES-256-GCM under that key:
//!
//! * a program that listens on the app's port instead of the app can't read
//!   what the extension sends (URLs, cookies) and can't answer it;
//! * a program or another extension that doesn't have the key can't make
//!   the app do anything.
//!
//! Wire format, both directions: `{"v":2,"n":"<nonce>","c":"<ciphertext>"}`
//! (standard base64; the nonce is 12 random bytes, the ciphertext ends with
//! the GCM tag). The plaintext is JSON. A request carries `id` (random,
//! unique) and `ts` (milliseconds since 1970); the app refuses requests
//! older or newer than [`MAX_SKEW_MS`] and ids it has already seen, and the
//! response repeats the `id`, so neither side accepts a replayed message.
//! Requests and responses use different associated data, so one can't be
//! passed off as the other.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use parking_lot::Mutex;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const KEY_BYTES: usize = 32;
/// How far a request's `ts` may be from the app's clock.
pub const MAX_SKEW_MS: i64 = 120_000;
/// Largest sealed request accepted.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

const REQUEST_AAD: &[u8] = b"odm-bridge-v2 request";
const RESPONSE_AAD: &[u8] = b"odm-bridge-v2 response";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// not a sealed message
    Malformed,
    /// sealed with another key (the extension isn't paired with this app)
    WrongKey,
    /// too old, too new, or seen before
    Replayed,
}

struct Secret {
    raw: [u8; KEY_BYTES],
    key: LessSafeKey,
}

impl Secret {
    fn new(raw: [u8; KEY_BYTES]) -> Option<Self> {
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &raw).ok()?);
        Some(Self { raw, key })
    }

    fn random() -> Option<Self> {
        let mut raw = [0u8; KEY_BYTES];
        SystemRandom::new().fill(&mut raw).ok()?;
        Self::new(raw)
    }

    fn from_code(code: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(code.trim()).ok()?;
        Self::new(bytes.try_into().ok()?)
    }
}

pub struct Pairing {
    secret: Mutex<Secret>,
    /// request ids seen within the accepted time window -> their `ts`
    seen: Mutex<HashMap<String, i64>>,
}

impl Pairing {
    /// From a stored pairing code; a missing or unreadable one gives a new
    /// key. Returns the pairing and whether the code must be stored.
    pub fn load(stored: Option<&str>) -> Result<(Self, bool), String> {
        let (secret, fresh) = match stored.and_then(Secret::from_code) {
            Some(s) => (s, false),
            None => (Secret::random().ok_or("no secure random numbers available")?, true),
        };
        Ok((Self { secret: Mutex::new(secret), seen: Mutex::new(HashMap::new()) }, fresh))
    }

    /// The pairing code the user copies into the extension.
    pub fn code(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.secret.lock().raw)
    }

    /// Replace the key: every paired extension must be paired again.
    pub fn reset(&self) -> Result<String, String> {
        let next = Secret::random().ok_or("no secure random numbers available")?;
        *self.secret.lock() = next;
        self.seen.lock().clear();
        Ok(self.code())
    }

    /// Open a sealed request received at `now_ms`. The result is the
    /// request's JSON object (with its `id`).
    pub fn open_request(&self, body: &[u8], now_ms: i64) -> Result<Value, PairError> {
        if body.len() > MAX_REQUEST_BYTES {
            return Err(PairError::Malformed);
        }
        let plain = self.open(body, REQUEST_AAD)?;
        let v: Value = serde_json::from_slice(&plain).map_err(|_| PairError::Malformed)?;
        let id = v.get("id").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 64);
        let ts = v.get("ts").and_then(Value::as_i64);
        let (Some(id), Some(ts)) = (id, ts) else { return Err(PairError::Malformed) };
        if (now_ms - ts).abs() > MAX_SKEW_MS {
            return Err(PairError::Replayed);
        }
        let mut seen = self.seen.lock();
        seen.retain(|_, t| (now_ms - *t).abs() <= MAX_SKEW_MS);
        if seen.insert(id.to_string(), ts).is_some() {
            return Err(PairError::Replayed);
        }
        Ok(v)
    }

    /// Seal the response to the request with this `id`.
    pub fn seal_response(&self, id: &str, mut response: Value) -> String {
        if let Some(o) = response.as_object_mut() {
            o.insert("id".into(), json!(id));
        }
        self.seal(response.to_string().as_bytes(), RESPONSE_AAD)
    }

    fn open(&self, body: &[u8], aad: &'static [u8]) -> Result<Vec<u8>, PairError> {
        let env: Value = serde_json::from_slice(body).map_err(|_| PairError::Malformed)?;
        let field = |k: &str| env.get(k).and_then(Value::as_str).and_then(|s| STANDARD.decode(s).ok());
        let (Some(nonce), Some(mut data)) = (field("n"), field("c")) else { return Err(PairError::Malformed) };
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| PairError::Malformed)?;
        let secret = self.secret.lock();
        let plain = secret
            .key
            .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut data)
            .map_err(|_| PairError::WrongKey)?;
        Ok(plain.to_vec())
    }

    fn seal(&self, plain: &[u8], aad: &'static [u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        let mut data = plain.to_vec();
        let sealed = SystemRandom::new().fill(&mut nonce).is_ok()
            && self
                .secret
                .lock()
                .key
                .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut data)
                .is_ok();
        if !sealed {
            // nothing readable leaves the app
            return json!({ "v": 2, "error": "seal failed" }).to_string();
        }
        json!({ "v": 2, "n": STANDARD.encode(nonce), "c": STANDARD.encode(data) }).to_string()
    }

    /// What the extension does: seal a request (tests and tools).
    pub fn seal_request(&self, request: &Value) -> String {
        self.seal(request.to_string().as_bytes(), REQUEST_AAD)
    }

    /// What the extension does: open a response (tests and tools).
    pub fn open_response(&self, body: &[u8]) -> Result<Value, PairError> {
        serde_json::from_slice(&self.open(body, RESPONSE_AAD)?).map_err(|_| PairError::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Pairing {
        Pairing::load(None).unwrap().0
    }

    #[test]
    fn code_round_trips() {
        let (a, fresh) = Pairing::load(None).unwrap();
        assert!(fresh);
        assert_eq!(a.code().len(), 43);
        let (b, fresh) = Pairing::load(Some(&a.code())).unwrap();
        assert!(!fresh);
        assert_eq!(a.code(), b.code());
        // an unreadable stored code is replaced, never used
        assert!(Pairing::load(Some("short")).unwrap().1);
    }

    #[test]
    fn request_and_response() {
        let p = pair();
        let sealed = p.seal_request(&json!({ "id": "a1", "ts": 1000, "op": "ping" }));
        assert!(!sealed.contains("ping"));
        let v = p.open_request(sealed.as_bytes(), 1500).unwrap();
        assert_eq!(v["op"], "ping");
        let reply = p.seal_response("a1", json!({ "ok": true }));
        let r = p.open_response(reply.as_bytes()).unwrap();
        assert_eq!(r, json!({ "ok": true, "id": "a1" }));
    }

    #[test]
    fn refuses_other_keys_replays_and_stale_requests() {
        let (p, other) = (pair(), pair());
        let sealed = other.seal_request(&json!({ "id": "a", "ts": 0 }));
        assert_eq!(p.open_request(sealed.as_bytes(), 0), Err(PairError::WrongKey));

        let sealed = p.seal_request(&json!({ "id": "a", "ts": 0 }));
        assert!(p.open_request(sealed.as_bytes(), 0).is_ok());
        assert_eq!(p.open_request(sealed.as_bytes(), 1), Err(PairError::Replayed));

        let old = p.seal_request(&json!({ "id": "b", "ts": 0 }));
        assert_eq!(p.open_request(old.as_bytes(), MAX_SKEW_MS + 1), Err(PairError::Replayed));

        assert_eq!(p.open_request(b"{\"url\":\"https://x\"}", 0), Err(PairError::Malformed));
        assert_eq!(p.open_request(p.seal_request(&json!({ "ts": 0 })).as_bytes(), 0), Err(PairError::Malformed));
    }

    #[test]
    fn a_response_is_not_a_request() {
        let p = pair();
        let reply = p.seal_response("a", json!({ "ts": 0 }));
        assert_eq!(p.open_request(reply.as_bytes(), 0), Err(PairError::WrongKey));
    }

    #[test]
    fn reset_drops_the_old_key() {
        let p = pair();
        let sealed = p.seal_request(&json!({ "id": "a", "ts": 0 }));
        let old = p.code();
        assert_ne!(p.reset().unwrap(), old);
        assert_eq!(p.open_request(sealed.as_bytes(), 0), Err(PairError::WrongKey));
    }
}
