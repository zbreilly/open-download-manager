//! Open Download Manager — core engine.
//!
//! * `task`     — segmented, resumable HTTP(S)/FTP download with dynamic splitting
//! * `ftp`      — FTP/FTPS client (passive mode, REST, SIZE, MDTM)
//! * `stream`   — HLS/DASH downloads (`hls`, `dash` parse manifests, `mp4` muxes)
//! * `manager`  — download list, queues, scheduler, limits, events
//! * `store`    — SQLite persistence
//! * `grabber`  — site grabber (link extraction)
//! * `util`     — names, categories, batch patterns, hashing
//! * `pairing`  — shared key and sealed messages for the browser extension
//! * `secret`   — protection of stored cookies, passwords and keys

pub mod client;
pub mod dash;
pub mod ftp;
pub mod hls;
pub mod grabber;
pub mod limiter;
pub mod manager;
pub mod model;
pub mod mp4;
pub mod pairing;
pub mod secret;
pub mod store;
pub mod stream;
pub mod task;
pub mod util;

pub use manager::{Event, Manager};
pub use model::*;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid URL: {0}")]
    BadUrl(String),
    #[error("{}", client::status_message(*.0))]
    Http(u16),
    #[error("{0}")]
    Network(String),
    /// an https download was redirected to plain http (see
    /// `Settings::allow_insecure_redirects`)
    #[error("the server redirected this secure (https) download to an insecure http address; if you trust the source, allow it under Settings → Advanced")]
    InsecureRedirect,
    #[error("download not found")]
    NotFound,
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    pub fn from_reqwest(e: reqwest::Error) -> Self {
        if let Some(s) = e.status() {
            return CoreError::Http(s.as_u16());
        }
        // URLs may carry tokens or credentials: never put them in messages
        let e = e.without_url();
        let msg = if e.is_timeout() {
            "connection timed out".to_string()
        } else if e.is_connect() {
            let mut m = "could not connect to server".to_string();
            let mut src: Option<&dyn std::error::Error> = std::error::Error::source(&e);
            while let Some(s) = src {
                let t = s.to_string();
                if t.contains("dns") || t.contains("resolve") || t.contains("lookup") {
                    m = "server name could not be resolved".into();
                    break;
                }
                if t.contains("certificate") || t.contains("Certificate") {
                    m = format!("TLS certificate problem: {t}");
                    break;
                }
                src = s.source();
            }
            m
        } else if e.is_redirect() {
            "too many redirects".to_string()
        } else {
            let mut m = e.to_string();
            let mut src: Option<&dyn std::error::Error> = std::error::Error::source(&e);
            while let Some(s) = src {
                m = format!("{m}: {s}");
                src = s.source();
            }
            m
        };
        CoreError::Network(util::redact_urls(&msg))
    }
}

impl serde::Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
