//! Minimal HTTP/1.1 test server with Range support and failure modes.
//!
//! Query parameters understood by file paths:
//! * `size=N`      file size (default 100 000; 0 = empty file)
//! * `salt=N`      changes the content (see [`byte_at_salt`])
//! * `delay=MS`    pause after every 16 KiB chunk (`slow0=K`: K times slower
//!   for the request starting at byte 0)
//! * `drop=N`      close the connection after sending N body bytes
//! * `etag=N`      send `ETag: "vN"` and honor If-Range against it
//! * `maxconn=N`   answer 503 while N bodies are already being served
//! * `noifrange=1` ignore If-Range (send 206 even when the validator is stale)
//!
//! Paths:
//! * `/noranges/…` never sends 206
//! * `/cd…`        sends a Content-Disposition name
//! * `/v/…`        versioned file: [`Server::version`] 2 switches to
//!   `size2` (if given) and different content; ETag follows the version
//! * `/gone/…`     404 once [`Server::gone`] is set
//! * `/404…`, `/garbage` (not HTTP), `/page`, `/noct` (HTML without
//!   Content-Type), `/binct` (HTML-looking body served as octet-stream),
//!   `/noctbin` (binary without Content-Type, markup after 4 KiB)
#![allow(dead_code)]

pub mod ftp;
pub mod web;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub fn byte_at_salt(i: u64, salt: u64) -> u8 {
    ((i * 31 + i / 7 + salt * 13) % 251) as u8
}

pub fn byte_at(i: u64) -> u8 {
    byte_at_salt(i, 0)
}

pub fn expected(size: u64) -> Vec<u8> {
    expected_salt(size, 0)
}

pub fn expected_salt(size: u64, salt: u64) -> Vec<u8> {
    (0..size).map(|i| byte_at_salt(i, salt)).collect()
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    pub hits: Arc<AtomicUsize>,
    /// request heads, in arrival order
    pub log: Arc<Mutex<Vec<String>>>,
    /// content version for `/v/` paths
    pub version: Arc<AtomicU64>,
    /// `/gone/` paths answer 404 once set
    pub gone: Arc<AtomicBool>,
    /// bodies currently being served
    pub active: Arc<AtomicUsize>,
    /// redirector only: where requests are sent (`target + path`)
    pub redirect_to: Arc<Mutex<Option<String>>>,
}

impl Server {
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn query(path: &str, key: &str) -> Option<u64> {
    let q = path.split_once('?')?.1;
    for kv in q.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            if k == key {
                return v.parse().ok();
            }
        }
    }
    None
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn start() -> Server {
    serve(None).await
}

/// A server that answers every request with a 302 to `target` + path.
pub async fn start_redirector(target: &str) -> Server {
    serve(Some(target.to_string())).await
}

async fn serve(redirect: Option<String>) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let srv = Server {
        base: format!("http://{addr}"),
        hits: Arc::new(AtomicUsize::new(0)),
        log: Arc::new(Mutex::new(vec![])),
        version: Arc::new(AtomicU64::new(1)),
        gone: Arc::new(AtomicBool::new(false)),
        active: Arc::new(AtomicUsize::new(0)),
        redirect_to: Arc::new(Mutex::new(redirect)),
    };
    let s2 = srv.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { continue };
            let srv = s2.clone();
            tokio::spawn(handle(s, srv));
        }
    });
    srv
}

async fn handle(mut s: tokio::net::TcpStream, srv: Server) {
    let mut buf = vec![0u8; 8192];
    let mut n = 0;
    loop {
        match s.read(&mut buf[n..]).await {
            Ok(0) | Err(_) => return,
            Ok(k) => n += k,
        }
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    srv.hits.fetch_add(1, Ordering::SeqCst);
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    srv.log.lock().unwrap().push(head.clone());
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let path = req_line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let redirect = srv.redirect_to.lock().unwrap().clone();
    if let Some(t) = redirect {
        let r = format!("HTTP/1.1 302 Found\r\nLocation: {t}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let _ = s.write_all(r.as_bytes()).await;
        return;
    }
    let mut range: Option<(u64, Option<u64>)> = None;
    let mut if_range: Option<String> = None;
    for l in lines {
        let lower = l.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("range: bytes=") {
            let (a, b) = v.split_once('-').unwrap();
            range = Some((a.trim().parse().unwrap(), b.trim().parse().ok()));
        }
        if let Some(v) = lower.strip_prefix("if-range: ") {
            if_range = Some(v.trim().to_string());
        }
    }
    let versioned = path.starts_with("/v/");
    let ver = srv.version.load(Ordering::SeqCst);
    let v2 = versioned && ver == 2;
    let size = if v2 { query(&path, "size2").or(query(&path, "size")) } else { query(&path, "size") }.unwrap_or(100_000);
    let salt = query(&path, "salt").unwrap_or(0) + if v2 { 7 } else { 0 };
    let delay = query(&path, "delay").unwrap_or(0);
    let drop_after = query(&path, "drop");
    let etag = if versioned { Some(format!("\"v{ver}\"")) } else { query(&path, "etag").map(|e| format!("\"v{e}\"")) };
    if path.starts_with("/404") || (path.starts_with("/gone/") && srv.gone.load(Ordering::SeqCst)) {
        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        return;
    }
    if path.starts_with("/garbage") {
        let _ = s.write_all(b"this is not HTTP at all\r\n\r\n").await;
        return;
    }
    if path.starts_with("/noctbin") {
        // binary without Content-Type that happens to contain markup later on
        let mut body: Vec<u8> = (0..4096u64).map(|i| byte_at_salt(i, 9)).collect();
        body.extend_from_slice(b"<a href=\"/f/z.zip\">z</a>");
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = s.write_all(head.as_bytes()).await;
        let _ = s.write_all(&body).await;
        return;
    }
    if path.starts_with("/page") || path.starts_with("/noct") || path.starts_with("/binct") {
        let html = "<html><body><a href=\"/f/a.zip?size=10\">a</a><a href='b.pdf'>b</a>\
                    <img src=\"/img/c.png\"><a href=\"/sub.html\">sub</a><a href=\"#top\">x</a></body></html>";
        let ct = if path.starts_with("/page") {
            "Content-Type: text/html\r\n"
        } else if path.starts_with("/binct") {
            "Content-Type: application/octet-stream\r\n"
        } else {
            ""
        };
        let resp = format!("HTTP/1.1 200 OK\r\n{ct}Content-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
        let _ = s.write_all(resp.as_bytes()).await;
        return;
    }
    let ranges_ok = !path.starts_with("/noranges");
    let mut extra = String::new();
    if path.starts_with("/cd") {
        extra.push_str("Content-Disposition: attachment; filename=\"server-name.bin\"\r\n");
    }
    if let Some(e) = &etag {
        extra.push_str(&format!("ETag: {e}\r\n"));
    }
    // If-Range mismatch => full 200
    let honor_if_range = query(&path, "noifrange").unwrap_or(0) == 0;
    let range = match (&range, &if_range, &etag) {
        (Some(_), Some(ir), Some(e)) if honor_if_range && ir != &e.to_ascii_lowercase() => None,
        _ => range,
    };
    let (status, start, end) = match range {
        Some((a, b)) if ranges_ok => {
            let e = b.map(|b| b + 1).unwrap_or(size).min(size);
            if a >= size {
                let _ = s
                    .write_all(
                        format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n")
                            .as_bytes(),
                    )
                    .await;
                return;
            }
            (206, a, e)
        }
        _ => (200, 0, size),
    };
    let _active = {
        let prev = srv.active.fetch_add(1, Ordering::SeqCst);
        let guard = ActiveGuard(srv.active.clone());
        if let Some(mc) = query(&path, "maxconn") {
            if prev as u64 >= mc {
                let _ = s
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                return;
            }
        }
        guard
    };
    let mut head = if status == 206 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\n",
            start,
            end - 1,
            size,
            end - start
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\n")
    };
    if ranges_ok {
        head.push_str("Accept-Ranges: bytes\r\n");
    }
    head.push_str("Content-Type: application/octet-stream\r\nConnection: close\r\n");
    head.push_str(&extra);
    head.push_str("\r\n");
    if s.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let slow0 = query(&path, "slow0").unwrap_or(1);
    let delay = if start == 0 { delay * slow0 } else { delay };
    let mut pos = start;
    let mut sent = 0u64;
    while pos < end {
        let n = (end - pos).min(16 * 1024);
        let chunk: Vec<u8> = (pos..pos + n).map(|i| byte_at_salt(i, salt)).collect();
        if s.write_all(&chunk).await.is_err() {
            return;
        }
        pos += n;
        sent += n;
        if let Some(d) = drop_after {
            if sent >= d {
                return; // abrupt close
            }
        }
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
    }
}
