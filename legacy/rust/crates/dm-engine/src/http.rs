//! HTTP/1.1 connection: plain or TLS, request writer, response-head parser.

use crate::url::Url;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("Open-Download-Manager/", env!("CARGO_PKG_VERSION"));

enum ConnKind {
    Plain(TcpStream),
    Tls(rustls::StreamOwned<rustls::ClientConnection, TcpStream>),
}

pub struct Connection {
    inner: ConnKind,
    /// body bytes that arrived together with the response head
    leftover: Vec<u8>,
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.leftover.is_empty() {
            let n = self.leftover.len().min(buf.len());
            buf[..n].copy_from_slice(&self.leftover.drain(..n).collect::<Vec<u8>>());
            return Ok(n);
        }
        match &mut self.inner {
            ConnKind::Plain(s) => s.read(buf),
            ConnKind::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            ConnKind::Plain(s) => s.write(buf),
            ConnKind::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.inner {
            ConnKind::Plain(s) => s.flush(),
            ConnKind::Tls(s) => s.flush(),
        }
    }
}

pub fn connect(url: &Url) -> Result<Connection, crate::EngineError> {
    let tcp = TcpStream::connect((url.host.as_str(), url.port))?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))?;
    let inner = if !url.is_tls() {
        ConnKind::Plain(tcp)
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = std::sync::Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let name = rustls::pki_types::ServerName::try_from(url.host.clone())
            .map_err(|_| crate::EngineError::DnsName(url.host.clone()))?;
        let conn = rustls::ClientConnection::new(config, name)
            .map_err(|e| crate::EngineError::Tls(e.to_string()))?;
        ConnKind::Tls(rustls::StreamOwned::new(conn, tcp))
    };
    Ok(Connection { inner, leftover: Vec::new() })
}

pub struct Request<'a> {
    url: &'a Url,
    range_from: Option<u64>,
    extra_headers: Vec<(String, String)>,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a Url) -> Self {
        Self { url, range_from: None, extra_headers: Vec::new() }
    }
    pub fn range_from(mut self, offset: u64) -> Self {
        self.range_from = Some(offset);
        self
    }
    /// Arbitrary extra headers (auth, referrer, cookies, ...).
    pub fn headers(mut self, hs: Vec<(String, String)>) -> Self {
        self.extra_headers = hs;
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut s = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n",
            self.url.path,
            self.url.host_port(),
            USER_AGENT
        );
        if let Some(off) = self.range_from {
            s.push_str(&format!("Range: bytes={off}-\r\n"));
        }
        for (k, v) in &self.extra_headers {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        s.push_str("\r\n");
        s.into_bytes()
    }
}

/// Minimal base64 (standard alphabet, padded) for Basic auth.
pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

pub mod body_kind {
    /// Body framing decided from response headers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BodyKind {
        ContentLength(u64),
        Chunked,
        ToEof,
    }

    pub fn detect(te: Option<&str>, cl: Option<&str>) -> BodyKind {
        if let Some(te) = te {
            if te.to_ascii_lowercase().contains("chunked") {
                return BodyKind::Chunked;
            }
        }
        if let Some(cl) = cl {
            if let Ok(n) = cl.trim().parse::<u64>() {
                return BodyKind::ContentLength(n);
            }
        }
        BodyKind::ToEof
    }
}

pub struct ResponseHead {
    pub status: u16,
    headers: Vec<(String, String)>, // lowercased names
}

impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        let lname = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == lname)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_kind(&self) -> body_kind::BodyKind {
        body_kind::detect(
            self.header("transfer-encoding"),
            self.header("content-length"),
        )
    }

    /// Total resource size (Content-Range total on 206 replies).
    pub fn total_size(&self, resumed: bool) -> Option<u64> {
        if resumed {
            let cr = self.header("content-range")?;
            // "bytes <start>-<end>/<total>"
            let total = cr.rsplit('/').next()?.trim();
            return total.parse::<u64>().ok();
        }
        self.header("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
    }
}

impl Connection {
    pub fn write_request(&mut self, req: &Request) -> std::io::Result<()> {
        self.write_all(&req.encode())?;
        self.flush()
    }

    pub fn read_response_head(&mut self) -> Result<ResponseHead, crate::EngineError> {
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 512];
        loop {
            let n = self.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            if buf.len() > 64 * 1024 {
                return Err(crate::EngineError::BadUrl("response head too large".into()));
            }
        }
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| crate::EngineError::BadUrl("no head terminator".into()))?;
        let head_text = String::from_utf8_lossy(&buf[..split]).into_owned();
        let mut lines = head_text.split("\r\n");
        let status_line = lines.next().unwrap_or_default();
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| crate::EngineError::BadUrl(status_line.to_string()))?;
        let mut headers = Vec::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        self.leftover = buf[split + 4..].to_vec();
        Ok(ResponseHead { status, headers })
    }
}
