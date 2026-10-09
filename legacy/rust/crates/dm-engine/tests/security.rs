//! Security regression tests for the auth + path-guard hardening
//! (findings from analysis/dev-security-review.md).

use std::io::{Read, Write};
use std::net::TcpListener;

fn body(len: usize) -> Vec<u8> { vec![0x33; len] }

fn raw_request(port: u16, req: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).unwrap();
    String::from_utf8_lossy(&resp).into_owned()
}

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

fn respond(stream: &mut std::net::TcpStream, head: String, body: &[u8]) {
    let resp = format!("{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", head, body.len());
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
}

/// Serves one 200 response per connection (harmless backend).
fn dumb_server(data: Vec<u8>) -> u16 {
    let d2 = std::sync::Arc::new(data);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let mut s = stream.unwrap();
            let _ = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
        }
    });
    port
}

#[test]
fn security_auth_required_and_host_checked() {
    let data = body(4 * 1024);
    let port = dumb_server(data.clone());
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs_path = std::env::temp_dir().join(format!("sec_{}.jobs", std::process::id()));
    let _ = std::fs::remove_file(format!("{}.token", jobs_path.display()));
    let _ = std::fs::remove_file(&jobs_path);
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port,
        jobs_path: jobs_path.clone(),
        parts: 1,
        max_parallel: 1,
    };
    std::thread::spawn(move || {
        let _ = dm_engine::server::serve_on(cfg, ui);
    });
    std::thread::sleep(std::time::Duration::from_millis(300));

    // 1) API without token -> 401
    let resp = raw_request(ui_port,
        "GET /api/jobs HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(resp.starts_with("HTTP/1.1 401"), "unauthenticated API call must 401: {}",
        resp.split("\r\n").next().unwrap_or(""));

    // 2) wrong token -> 401
    let resp = raw_request(ui_port,
        "GET /api/jobs?token=deadbeefdeadbeef HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(resp.starts_with("HTTP/1.1 401"), "wrong token must 401");

    // 3) DNS-rebinding style Host -> 403 even with valid token
    let token = std::fs::read_to_string(format!("{}.token", jobs_path.display()))
        .unwrap().trim().to_string();
    let resp = raw_request(ui_port, &format!(
        "GET /api/jobs?token={token} HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 403"), "non-loopback Host must 403");

    // 4) correct token + loopback Host -> 200
    let resp = raw_request(ui_port, &format!(
        "GET /api/jobs?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 200"), "valid token must pass: {}",
        resp.split("\r\n").next().unwrap_or(""));

    // 5) token via header also works
    let resp = raw_request(ui_port, &format!(
        "GET /api/jobs HTTP/1.1\r\nHost: localhost\r\nX-ODM-Token: {token}\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 200"), "header token must pass");

    // 6) UI page embeds the token for same-origin use
    let resp = raw_request(ui_port,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(resp.contains(&format!("window.ODM_TOKEN=\"{token}\"")),
        "UI page must carry the injected token");

    let _ = std::fs::remove_file(&jobs_path);
    let _ = std::fs::remove_file(format!("{}.token", jobs_path.display()));
}

#[test]
fn security_path_traversal_rejected() {
    let data = body(4 * 1024);
    let port = dumb_server(data.clone());
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs_path = std::env::temp_dir().join(format!("sec2_{}.jobs", std::process::id()));
    let _ = std::fs::remove_file(format!("{}.token", jobs_path.display()));
    let _ = std::fs::remove_file(&jobs_path);
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port,
        jobs_path: jobs_path.clone(),
        parts: 1,
        max_parallel: 1,
    };
    std::thread::spawn(move || {
        let _ = dm_engine::server::serve_on(cfg, ui);
    });
    std::thread::sleep(std::time::Duration::from_millis(300));

    let token = std::fs::read_to_string(format!("{}.token", jobs_path.display()))
        .unwrap().trim().to_string();

    // /api/add with an out path outside allowed roots -> 403
    let resp = raw_request(ui_port, &format!(
        "POST /api/add?url=http%3A%2F%2F127.0.0.1%3A{port}%2Fdata&out=%2Fetc%2Fprivate%2Fodm_pwn&token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 403"), "out= outside allowed roots must 403: {}",
        resp.split("\r\n").next().unwrap_or(""));

    // ../ traversal in the same param -> 403 after normalization
    let resp = raw_request(ui_port, &format!(
        "POST /api/add?url=http%3A%2F%2F127.0.0.1%3A{port}%2Fdata&out=%2Ftmp%2F..%2F..%2Fetc%2Fodm_pwn&token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 403"), "normalized traversal must 403");

    // extension add with a path-carrying fileName -> 400
    let json = format!(
        "{{\"url\":\"http://127.0.0.1:{port}/data\",\"fileName\":\"../../.zshrc\"}}");
    let resp = raw_request(ui_port, &format!(
        "POST /api/extension/add?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(), json));
    assert!(resp.starts_with("HTTP/1.1 400"), "path-carrying fileName must 400: {}",
        resp.split("\r\n").next().unwrap_or(""));

    // /api/open on a disallowed path -> 403 (no process spawn)
    let resp = raw_request(ui_port, &format!(
        "POST /api/open?path=%2Fetc%2Fpasswd&token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(resp.starts_with("HTTP/1.1 403"), "open on disallowed path must 403");

    let _ = std::fs::remove_file(&jobs_path);
    let _ = std::fs::remove_file(format!("{}.token", jobs_path.display()));
}
