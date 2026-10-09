//! Category-folder routing: downloads land in per-category subfolders of
//! the download dir based on file type (analyzer-verified original behavior).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

fn body(len: usize) -> Vec<u8> { vec![0x11; len] }

fn raw(port: u16, req: &str) -> String {
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

#[test]
fn categories_route_into_folders() {
    let data = body(8 * 1024);
    let d2 = std::sync::Arc::new(data.clone());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let mut s = stream.unwrap();
            let _ = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
        }
    });

    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs = {
        let mut p = std::env::temp_dir();
        p.push(format!("cats_{}.jobs", std::process::id()));
        p
    };
    let dl_root = std::env::temp_dir().join(format!("cats_dl_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dl_root);
    std::fs::create_dir_all(&dl_root).unwrap();
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port, jobs_path: jobs.clone(), parts: 1, max_parallel: 1,
    };
    std::thread::spawn(move || { let _ = dm_engine::server::serve_on(cfg, ui); });
    std::thread::sleep(std::time::Duration::from_millis(300));

    let token = std::fs::read_to_string(format!("{}.token", jobs.display()))
        .unwrap().trim().to_string();
    let api = |m: &str, t: &str| {
        let sep = if t.contains('?') { '&' } else { '?' };
        raw(ui_port, &format!(
            "{m} {t}{sep}token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"))
    };

    // point the download dir at our temp root
    api("POST", &format!("/api/settings?defaultDir={}", dl_root.to_str().unwrap()));

    // 1) extension add with a .zip -> lands in <root>/Compressed/
    let json = format!(
        "{{\"url\":\"http://127.0.0.1:{port}/a.zip\",\"fileName\":\"arch.zip\"}}");
    let r = raw(ui_port, &format!(
        "POST /api/extension/add?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(), json));
    assert!(r.contains("added"), "ext add failed: {}", r.split("\r\n").next().unwrap_or(""));
    let wait = || {
        for _ in 0..50 {
            if api("GET", "/api/jobs").contains("\"state\":\"done\"") { return true; }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    };
    assert!(wait(), "zip job never finished");
    assert!(dl_root.join("Compressed").join("arch.zip").exists(),
        "zip must land in Compressed/, have: {:?}",
        std::fs::read_dir(&dl_root).unwrap().map(|e| e.unwrap().path().to_string_lossy().to_string()).collect::<Vec<_>>());

    // clean job for next round
    api("POST", "/api/remove?id=1");

    // 2) unknown extension -> root
    let json = format!(
        "{{\"url\":\"http://127.0.0.1:{port}/b.xyzunknown\",\"fileName\":\"weird.xyzunknown\"}}");
    let _ = raw(ui_port, &format!(
        "POST /api/extension/add?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(), json));
    assert!(wait(), "xyz job never finished");
    assert!(dl_root.join("weird.xyzunknown").exists(), "unknown ext must stay in root");

    // 3) categories API roundtrip
    let cats = api("GET", "/api/categories");
    assert!(cats.contains("Compressed") && cats.contains("General"));
    let cat_body = r#"{"categories":[{"name":"Custom","folder":"MyFolder","extensions":["zzz"]}]}"#;
    let r = raw(ui_port, &format!(
        "POST /api/categories?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        cat_body.len(), cat_body));
    assert!(r.contains("saved"), "cat save failed: {}", r.split("\r\n").next().unwrap_or(""));
    let cats2 = api("GET", "/api/categories");
    assert!(cats2.contains("Custom") && cats2.contains("MyFolder"), "roundtrip lost data");

    let _ = std::fs::remove_dir_all(&dl_root);
    let _ = std::fs::remove_file(&jobs);
    let _ = std::fs::remove_file(format!("{}.token", jobs.display()));
}
