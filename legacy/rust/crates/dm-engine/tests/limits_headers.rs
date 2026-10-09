//! Speed-limiter pacing + custom headers + extension endpoint.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn body(len: usize) -> Vec<u8> { vec![0x5a; len] }

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut b = [0u8; 512];
    loop {
        let n = stream.read(&mut b).unwrap();
        if n == 0 { break; }
        buf.extend_from_slice(&b[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") { break; }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[test]
fn limiter_paces_downloads() {
    let data = body(1024 * 1024); // 1 MB
    let d2 = data.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let _ = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2);
        }
    });
    // 256 KB/s with a full 256 KB startup bucket → >= ~3 s
    let limiter = Arc::new(dm_engine::SpeedLimiter::new(256 * 1024));
    let out = std::env::temp_dir().join(format!("odm_lim_{}.bin", std::process::id()));
    let started = std::time::Instant::now();
    dm_engine::download_with_ctl(
        &format!("http://127.0.0.1:{port}/data"), &out, None, None,
        Some(&Arc::new(AtomicBool::new(false))), Some(&limiter), None,
    ).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(elapsed >= std::time::Duration::from_millis(2800),
            "throttled download finished too fast: {elapsed:?}");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn auth_and_custom_headers_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let text = read_request(&mut s);
            let has_auth = text.contains("Authorization: Basic dXNlcjpwYXNz"); // user:pass
            let has_ref = text.contains("Referer: https://example.com/page");
            let has_cookie = text.contains("Cookie: k=v");
            let body = if has_auth && has_ref && has_cookie {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            } else {
                "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            };
            s.write_all(body.as_bytes()).unwrap();
        }
    });
    let out = std::env::temp_dir().join(format!("odm_hdr_{}.bin", std::process::id()));
    let headers = vec![
        ("Authorization".to_string(), "Basic dXNlcjpwYXNz".to_string()),
        ("Referer".to_string(), "https://example.com/page".to_string()),
        ("Cookie".to_string(), "k=v".to_string()),
    ];
    dm_engine::download_with_ctl(
        &format!("http://127.0.0.1:{port}/data"), &out, None, None,
        Some(&Arc::new(AtomicBool::new(false))), None, Some(&headers[..]),
    ).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"ok");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn limiter_zero_is_unlimited() {
    let l = dm_engine::SpeedLimiter::new(0);
    assert_eq!(l.acquire(10 * 1024 * 1024), std::time::Duration::ZERO);
    l.set_limit(1024);
    assert_eq!(l.limit(), 1024);
}

fn respond(stream: &mut std::net::TcpStream, head: String, body: &[u8]) {
    let resp = format!("{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", head, body.len());
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
