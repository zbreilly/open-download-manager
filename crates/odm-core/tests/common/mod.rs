//! Minimal HTTP/1.1 test server with Range support and failure modes.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub fn byte_at(i: u64) -> u8 {
    ((i * 31 + i / 7) % 251) as u8
}

pub fn expected(size: u64) -> Vec<u8> {
    (0..size).map(byte_at).collect()
}

pub struct Server {
    pub base: String,
    pub hits: Arc<AtomicUsize>,
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

pub async fn start() -> Server {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let h2 = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { continue };
            let hits = h2.clone();
            tokio::spawn(async move {
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
                hits.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut lines = head.split("\r\n");
                let req_line = lines.next().unwrap_or("");
                let path = req_line.split_whitespace().nth(1).unwrap_or("/").to_string();
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
                let size = query(&path, "size").unwrap_or(100_000);
                let delay = query(&path, "delay").unwrap_or(0);
                let drop_after = query(&path, "drop");
                let etag = query(&path, "etag").map(|e| format!("\"v{e}\""));
                if path.starts_with("/404") {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                if path.starts_with("/page") {
                    let html = format!(
                        "<html><body><a href=\"/f/a.zip?size=10\">a</a><a href='b.pdf'>b</a>\
                         <img src=\"/img/c.png\"><a href=\"/sub.html\">sub</a><a href=\"#top\">x</a></body></html>"
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    );
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
                let range = match (&range, &if_range, &etag) {
                    (Some(_), Some(ir), Some(e)) if ir != &e.to_ascii_lowercase() => None,
                    _ => range,
                };
                let (status, start, end) = match range {
                    Some((a, b)) if ranges_ok => {
                        let e = b.map(|b| b + 1).unwrap_or(size).min(size);
                        if a >= size {
                            let _ = s
                                .write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                                .await;
                            return;
                        }
                        (206, a, e)
                    }
                    _ => (200, 0, size),
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
                    let chunk: Vec<u8> = (pos..pos + n).map(byte_at).collect();
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
            });
        }
    });
    Server { base: format!("http://{addr}"), hits }
}
