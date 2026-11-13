//! Pairing between the app and its browser extension.
//!
//! The extension talks to the app over a loopback port. A port proves
//! nothing about who listens on it or who connects to it, so both sides
//! share a secret key (shown in the app's settings, pasted into the
//! extension once) and every message is sealed with it (AES-256-GCM):
//!
//! * a process that merely owns the port cannot read what the extension
//!   sends (URLs and cookies) and cannot produce an answer the extension
//!   accepts;
//! * a client that does not hold the key (another extension, a local tool)
//!   cannot make the app do anything.
//!
//! Wire format, both directions: `{"v":1,"n":"<base64 nonce>","c":"<base64
//! ciphertext and tag>"}`. A request seals `{"ts":<unix ms>,"op":"…",
//! "body":…}`; requests older or newer than [`MAX_SKEW_MS`], or whose nonce
//! was seen before, are refused. A response seals the result and is bound to
//! the request's nonce, so an old response cannot be replayed to the
//! extension.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub const KEY_LEN: usize = 32;
/// How far a request's timestamp may be from the app's clock.
pub const MAX_SKEW_MS: i64 = 120_000;
/// Most request nonces remembered inside the skew window.
const MAX_SEEN: usize = 8192;

const AAD_REQUEST: &[u8] = b"odm-bridge-v1 request";
const AAD_RESPONSE: &str = "odm-bridge-v1 response ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// not a sealed message at all
    Malformed,
    /// sealed with another key (not paired, or paired with an old key)
    BadKey,
    /// timestamp outside the accepted window
    Stale,
    /// this request was already handled
    Replay,
}

impl PairError {
    /// Short machine-readable reason sent to the client.
    pub fn code(self) -> &'static str {
        match self {
            PairError::Malformed => "malformed",
            PairError::BadKey => "pairing",
            PairError::Stale => "stale",
            PairError::Replay => "replay",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u32,
    n: String,
    c: String,
}

#[derive(Deserialize)]
struct Inner {
    ts: i64,
    op: String,
    #[serde(default)]
    body: Value,
}

/// An authenticated request from the extension.
#[derive(Debug)]
pub struct Request {
    pub op: String,
    pub body: Value,
    nonce: [u8; NONCE_LEN],
}

pub struct Pairing {
    key: RwLock<[u8; KEY_LEN]>,
    /// request nonce -> its timestamp, for the skew window
    seen: Mutex<HashMap<[u8; NONCE_LEN], i64>>,
}

/// A new random key.
pub fn generate_key() -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    // the system random generator does not fail on supported platforms
    SystemRandom::new().fill(&mut k).expect("system random generator");
    k
}

/// The key as shown to the user: 64 hex digits.
pub fn key_text(key: &[u8; KEY_LEN]) -> String {
    crate::util::to_hex(key)
}

/// Parse a key typed or pasted by the user (spaces and dashes ignored).
pub fn parse_key(text: &str) -> Option<[u8; KEY_LEN]> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    crate::util::from_hex(&clean)?.try_into().ok()
}

fn aead(key: &[u8; KEY_LEN]) -> LessSafeKey {
    // the key length matches the algorithm, so this cannot fail
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("AES-256 key"))
}

fn fresh_nonce() -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut n).expect("system random generator");
    n
}

fn seal_with(key: &[u8; KEY_LEN], nonce: [u8; NONCE_LEN], aad: &[u8], plain: &[u8]) -> String {
    let mut buf = plain.to_vec();
    aead(key)
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf)
        .expect("sealing a message");
    let env = Envelope { v: 1, n: B64.encode(nonce), c: B64.encode(&buf) };
    serde_json::to_string(&env).unwrap_or_default()
}

/// Open an envelope; returns the nonce and the plaintext.
fn open_with(key: &[u8; KEY_LEN], envelope: &[u8], aad: &[u8]) -> Result<([u8; NONCE_LEN], Vec<u8>), PairError> {
    let env: Envelope = serde_json::from_slice(envelope).map_err(|_| PairError::Malformed)?;
    if env.v != 1 {
        return Err(PairError::Malformed);
    }
    let nonce: [u8; NONCE_LEN] =
        B64.decode(env.n.as_bytes()).ok().and_then(|n| n.try_into().ok()).ok_or(PairError::Malformed)?;
    let mut buf = B64.decode(env.c.as_bytes()).map_err(|_| PairError::Malformed)?;
    let plain = aead(key)
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut buf)
        .map_err(|_| PairError::BadKey)?
        .to_vec();
    Ok((nonce, plain))
}

fn response_aad(request_nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    format!("{AAD_RESPONSE}{}", B64.encode(request_nonce)).into_bytes()
}

impl Pairing {
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Self { key: RwLock::new(key), seen: Mutex::new(HashMap::new()) }
    }

    pub fn key_text(&self) -> String {
        key_text(&self.key.read())
    }

    /// Replace the key: everything paired with the old one stops working.
    pub fn set_key(&self, key: [u8; KEY_LEN]) {
        *self.key.write() = key;
        self.seen.lock().clear();
    }

    /// Authenticate and open a request received at `now_ms`.
    pub fn open(&self, envelope: &[u8], now_ms: i64) -> Result<Request, PairError> {
        let key = *self.key.read();
        let (nonce, plain) = open_with(&key, envelope, AAD_REQUEST)?;
        let inner: Inner = serde_json::from_slice(&plain).map_err(|_| PairError::Malformed)?;
        if inner.ts.checked_sub(now_ms).map(i64::abs).is_none_or(|d| d > MAX_SKEW_MS) {
            return Err(PairError::Stale);
        }
        let mut seen = self.seen.lock();
        seen.retain(|_, ts| (*ts - now_ms).abs() <= MAX_SKEW_MS);
        if seen.contains_key(&nonce) || seen.len() >= MAX_SEEN {
            return Err(PairError::Replay);
        }
        seen.insert(nonce, inner.ts);
        Ok(Request { op: inner.op, body: inner.body, nonce })
    }

    /// Seal the answer to `req`.
    pub fn seal(&self, req: &Request, value: &Value) -> String {
        let key = *self.key.read();
        let plain = serde_json::to_vec(value).unwrap_or_default();
        seal_with(&key, fresh_nonce(), &response_aad(&req.nonce), &plain)
    }

    /// Seal a request the way the extension does (for Rust clients and
    /// tests). Returns the envelope and the nonce to open the answer with.
    pub fn seal_request(&self, op: &str, body: &Value, now_ms: i64) -> (String, [u8; NONCE_LEN]) {
        let key = *self.key.read();
        let nonce = fresh_nonce();
        let plain = serde_json::to_vec(&serde_json::json!({ "ts": now_ms, "op": op, "body": body })).unwrap_or_default();
        (seal_with(&key, nonce, AAD_REQUEST, &plain), nonce)
    }

    /// Open the answer to a request sealed with [`seal_request`](Self::seal_request).
    pub fn open_response(&self, request_nonce: &[u8; NONCE_LEN], envelope: &[u8]) -> Result<Value, PairError> {
        let key = *self.key.read();
        let (_, plain) = open_with(&key, envelope, &response_aad(request_nonce))?;
        serde_json::from_slice(&plain).map_err(|_| PairError::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn key_text_round_trip() {
        let k = generate_key();
        let t = key_text(&k);
        assert_eq!(t.len(), 64);
        assert_eq!(parse_key(&t), Some(k));
        assert_eq!(parse_key(&format!("  {}-{} \n", &t[..32], &t[32..])), Some(k));
        assert_eq!(parse_key("abcd"), None);
        assert_eq!(parse_key(&"g".repeat(64)), None);
        assert_ne!(generate_key(), k);
    }

    #[test]
    fn request_and_response() {
        let p = Pairing::new(generate_key());
        let (env, nonce) = p.seal_request("add", &json!({ "url": "https://h/f.zip", "cookies": "sid=SECRET" }), NOW);
        assert!(!env.contains("SECRET") && !env.contains("f.zip"), "{env}");
        let req = p.open(env.as_bytes(), NOW + 500).unwrap();
        assert_eq!(req.op, "add");
        assert_eq!(req.body["cookies"], "sid=SECRET");
        let answer = p.seal(&req, &json!({ "ok": true }));
        assert_eq!(p.open_response(&nonce, answer.as_bytes()).unwrap(), json!({ "ok": true }));
        // an answer to another request is not accepted
        let (_, other) = p.seal_request("ping", &Value::Null, NOW);
        assert_eq!(p.open_response(&other, answer.as_bytes()), Err(PairError::BadKey));
    }

    #[test]
    fn refuses_other_keys_replays_and_old_requests() {
        let app = Pairing::new(generate_key());
        let stranger = Pairing::new(generate_key());
        let (env, _) = stranger.seal_request("add", &json!({}), NOW);
        assert_eq!(app.open(env.as_bytes(), NOW).unwrap_err(), PairError::BadKey);

        let (env, _) = app.seal_request("ping", &Value::Null, NOW);
        assert!(app.open(env.as_bytes(), NOW).is_ok());
        assert_eq!(app.open(env.as_bytes(), NOW + 1).unwrap_err(), PairError::Replay);

        let (env, _) = app.seal_request("ping", &Value::Null, NOW);
        assert_eq!(app.open(env.as_bytes(), NOW + MAX_SKEW_MS + 1).unwrap_err(), PairError::Stale);
        let (env, _) = app.seal_request("ping", &Value::Null, i64::MIN);
        assert_eq!(app.open(env.as_bytes(), NOW).unwrap_err(), PairError::Stale);

        for junk in ["", "{}", "not json", r#"{"v":2,"n":"","c":""}"#, r#"{"v":1,"n":"AAAA","c":"AAAA"}"#] {
            assert_eq!(app.open(junk.as_bytes(), NOW).unwrap_err(), PairError::Malformed, "{junk}");
        }
        // a request is not accepted as a response and the other way round
        let (env, nonce) = app.seal_request("ping", &Value::Null, NOW);
        assert_eq!(app.open_response(&nonce, env.as_bytes()), Err(PairError::BadKey));
    }

    #[test]
    fn new_key_unpairs() {
        let p = Pairing::new(generate_key());
        let (env, _) = p.seal_request("ping", &Value::Null, NOW);
        p.set_key(generate_key());
        assert_eq!(p.open(env.as_bytes(), NOW).unwrap_err(), PairError::BadKey);
    }

    /// Fixed messages shared with the extension's own test
    /// (`extension/test/pairing.test.mjs`): the request was sealed by the
    /// extension's code (WebCrypto) and must open here; the response is
    /// sealed here and must open there. Run with `ODM_WRITE_VECTORS=1` to
    /// rewrite the response file after a deliberate format change.
    #[test]
    fn interoperates_with_the_extension() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
        let key = parse_key(&"01".repeat(32)).unwrap();
        let p = Pairing::new(key);

        let request = std::fs::read_to_string(dir.join("pairing-request.json")).unwrap();
        let req = p.open(request.trim().as_bytes(), NOW).unwrap();
        assert_eq!(req.op, "ping");
        assert_eq!(req.body, json!({ "hello": "wörld" }));
        assert_eq!(req.nonce, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);

        let plain = serde_json::to_vec(&json!({ "ok": true, "app": "Open Download Manager" })).unwrap();
        let response = seal_with(&key, [0x10; NONCE_LEN], &response_aad(&req.nonce), &plain);
        let file = dir.join("pairing-response.json");
        if std::env::var_os("ODM_WRITE_VECTORS").is_some() {
            std::fs::write(&file, format!("{response}\n")).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), response);
    }
}
