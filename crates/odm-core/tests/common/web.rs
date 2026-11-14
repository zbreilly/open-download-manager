//! Static HTTP server for tests: serves fixed bodies by path (query
//! ignored), with Range support, a content type per path, a per-request
//! delay and a hit counter per path.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// path → (body, content type)
type Files = Arc<Mutex<HashMap<String, (Vec<u8>, String)>>>;

#[derive(Clone)]
pub struct Web {
    pub base: String,
    files: Files,
    pub hits: Arc<Mutex<HashMap<String, u64>>>,
    /// milliseconds to wait before answering each request
    pub delay_ms: Arc<AtomicU64>,
}

impl Web {
    pub fn put(&self, path: &str, body: impl Into<Vec<u8>>, ctype: &str) {
        self.files.lock().unwrap().insert(path.to_string(), (body.into(), ctype.to_string()));
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn hits(&self, path: &str) -> u64 {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

pub async fn start() -> Web {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web = Web {
        base: format!("http://{}", l.local_addr().unwrap()),
        files: Arc::new(Mutex::new(HashMap::new())),
        hits: Arc::new(Mutex::new(HashMap::new())),
        delay_ms: Arc::new(AtomicU64::new(0)),
    };
    let w2 = web.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { continue };
            let w = w2.clone();
            tokio::spawn(handle(s, w));
        }
    });
    web
}

async fn handle(mut s: tokio::net::TcpStream, w: Web) {
    let mut buf = vec![0u8; 8192];
    let mut n = 0;
    loop {
        match s.read(&mut buf[n..]).await {
            Ok(0) | Err(_) => return,
            Ok(k) => n += k,
        }
        if buf[..n].windows(4).any(|x| x == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let mut lines = head.split("\r\n");
    let path = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
    let mut range: Option<(u64, Option<u64>)> = None;
    for l in lines {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("range: bytes=") {
            let (a, b) = v.split_once('-').unwrap();
            range = Some((a.trim().parse().unwrap_or(0), b.trim().parse().ok()));
        }
    }
    *w.hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
    let d = w.delay_ms.load(Ordering::SeqCst);
    if d > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(d)).await;
    }
    let file = w.files.lock().unwrap().get(&path).cloned();
    let Some((body, ct)) = file else {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        return;
    };
    let size = body.len() as u64;
    let (status, a, e) = match range {
        Some((a, b)) if a < size => (206, a, b.map(|b| b + 1).unwrap_or(size).min(size)),
        _ => (200, 0, size),
    };
    let mut h = if status == 206 {
        format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{}/{size}\r\n", e - 1)
    } else {
        "HTTP/1.1 200 OK\r\n".to_string()
    };
    h.push_str(&format!("Content-Length: {}\r\nContent-Type: {ct}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n", e - a));
    if s.write_all(h.as_bytes()).await.is_ok() {
        let _ = s.write_all(&body[a as usize..e as usize]).await;
    }
}
