//! Tests for the next-steps features: rebalancing, checksums, job store,
//! and the control server / web UI layer.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn body(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x: u64 = 0x0dd_ba11_1234_5678;
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(7);
        v.push((x >> 33) as u8);
    }
    v
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, Option<u64>) {
    let mut buf = Vec::new();
    let mut b = [0u8; 512];
    loop {
        let n = stream.read(&mut b).unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&b[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let range = text
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
        .and_then(|l| {
            l.split(':')
                .nth(1)?
                .trim()
                .trim_start_matches("bytes=")
                .split('-')
                .next()?
                .parse()
                .ok()
        });
    (text, range)
}

fn respond(stream: &mut std::net::TcpStream, head: String, body: &[u8]) {
    let resp = format!(
        "{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        head,
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn tempfile(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("odm_ns_{}_{}", std::process::id(), name));
    cleanup(&p);
    p
}

fn cleanup(p: &PathBuf) {
    let mut s = p.as_os_str().to_os_string();
    s.push(".part");
    let _ = std::fs::remove_file(PathBuf::from(&s));
    s.push(".state");
    let _ = std::fs::remove_file(PathBuf::from(s));
    let _ = std::fs::remove_file(p);
}

// ---------- 1. rebalancing ----------

/// Serves the first half at full speed; the second half trickles at
/// ~427 KB/s (64 KB per 150 ms) so the rebalancer has something to do.
#[test]
fn rebalancer_splits_slow_tail() {
    let data = body(4 * 1024 * 1024);
    let half = data.len() / 2;
    let d2 = Arc::new(data.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(30)));
            let (_, range) = read_request(&mut s);
            let d2 = Arc::clone(&d2);
            std::thread::spawn(move || match range {
                Some(start) if start as usize >= half => {
                    // slow zone: trickle
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        d2.len() - 1,
                        d2.len(),
                        d2.len() - start as usize
                    );
                    let _ = s.write_all(head.as_bytes());
                    for chunk in d2[start as usize..].chunks(64 * 1024) {
                        if s.write_all(chunk).is_err() {
                            break;
                        }
                        let _ = s.flush();
                        std::thread::sleep(std::time::Duration::from_millis(150));
                    }
                }
                Some(start) => {
                    respond(
                        &mut s,
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                            d2.len() - 1,
                            d2.len()
                        ),
                        &d2[start as usize..],
                    );
                }
                None => respond(&mut s, "HTTP/1.1 200 OK".into(), &d2),
            });
        }
    });

    let out = tempfile("rebal.bin");
    let started = std::time::Instant::now();
    let stats = dm_engine::multi::download_multi(
        &format!("http://127.0.0.1:{port}/data"),
        &out,
        2,
        None,
        None,
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(stats.splits >= 1, "rebalancer never split the slow tail");
    // without rebalancing the slow tail alone needs ~5s; assert we beat it
    // with margin for scheduler noise
    assert!(elapsed < std::time::Duration::from_secs(4), "took {elapsed:?}");
    cleanup(&out);
}

// ---------- 2. checksums ----------

#[test]
fn checksum_pass_and_corruption_detected() {
    let data = body(256 * 1024);
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&data);
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();

    // good server
    let d2 = data.clone();
    let l1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let good = l1.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l1.incoming() {
            let mut s = stream.unwrap();
            let (_, _) = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2);
        }
    });
    let out = tempfile("sha_ok.bin");
    let stats = dm_engine::verify::download_verified(
        &format!("http://127.0.0.1:{good}/data"),
        &out,
        1,
        &hex,
    )
    .unwrap();
    assert_eq!(stats.bytes_written, data.len() as u64);
    cleanup(&out);

    // corrupt server (one byte flipped)
    let mut bad = data.clone();
    bad[data.len() / 2] ^= 0xff;
    let d3 = bad;
    let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let evil = l2.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l2.incoming() {
            let mut s = stream.unwrap();
            let (_, _) = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d3);
        }
    });
    let out = tempfile("sha_bad.bin");
    let err = dm_engine::verify::download_verified(
        &format!("http://127.0.0.1:{evil}/data"),
        &out,
        1,
        &hex,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        dm_engine::EngineError::ChecksumMismatch { .. }
    ));
    cleanup(&out);
}

// ---------- 3. persistent job store ----------

#[test]
fn job_store_persists_and_runs() {
    let data = body(128 * 1024);
    let d2 = Arc::new(data.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let (_, _) = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
        }
    });

    let store_path = tempfile("jobs.store");
    let out1 = tempfile("job_a.bin");
    let out2 = tempfile("job_b.bin");
    {
        let mut store = dm_engine::jobs::JobStore::open(&store_path).unwrap();
        store
            .add(&format!("http://127.0.0.1:{port}/a"), &out1)
            .unwrap();
        store
            .add(&format!("http://127.0.0.1:{port}/b"), &out2)
            .unwrap();
        assert_eq!(store.list().len(), 2);
    }
    // reopen: persistence across process-style boundaries
    {
        let mut store = dm_engine::jobs::JobStore::open(&store_path).unwrap();
        let n = store.run_pending(1, 2, &mut |_| {}).unwrap();
        assert_eq!(n, 2);
    }
    let store = dm_engine::jobs::JobStore::open(&store_path).unwrap();
    let jobs = store.list();
    assert!(jobs.iter().all(|j| j.state == dm_engine::jobs::JobState::Done));
    assert_eq!(std::fs::read(&out1).unwrap(), data);
    assert_eq!(std::fs::read(&out2).unwrap(), data);

    // requeue failed: add a dead job, run, requeue it
    let out3 = tempfile("job_c.bin");
    let mut store = dm_engine::jobs::JobStore::open(&store_path).unwrap();
    store.add("http://127.0.0.1:9/x", &out3).unwrap(); // nothing listens there
    store.run_pending(1, 1, &mut |_| {}).unwrap();
    assert!(store
        .list()
        .iter()
        .any(|j| j.state == dm_engine::jobs::JobState::Failed));
    assert_eq!(store.requeue_failed(), 1);
    assert_eq!(
        store
            .list()
            .iter()
            .filter(|j| j.state == dm_engine::jobs::JobState::Pending)
            .count(),
        1
    );
    for f in [&store_path, &out1, &out2, &out3] {
        let _ = std::fs::remove_file(f);
    }
}

// ---------- 4. control server / web UI ----------

#[test]
fn server_end_to_end() {
    // backend file server
    let data = body(128 * 1024);
    let d2 = Arc::new(data.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let (_, _) = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
        }
    });

    // UI server on an ephemeral port
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs_path = tempfile("ui.jobs");
    let out = tempfile("ui_dl.bin");
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port,
        jobs_path: jobs_path.clone(),
        parts: 1,
        max_parallel: 1,
    };
    std::thread::spawn(move || {
        let _ = dm_engine::server::serve_on(cfg, ui);
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let token = std::fs::read_to_string(format!("{}.token", jobs_path.display()))
        .unwrap().trim().to_string();
    let mut api = |method: &str, target: &str| -> String {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", ui_port)).unwrap();
        let sep = if target.contains('?') { '&' } else { '?' };
        let req = format!(
            "{method} {target}{sep}token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).unwrap();
        String::from_utf8_lossy(&resp).into_owned()
    };

    // the page itself
    let page = api("GET", "/");
    assert!(page.contains("Open Download Manager"));
    assert!(page.contains("200"));

    // add + run + poll to completion
    api(
        "POST",
        &format!(
            "/api/add?url={}&out={}",
            percent(&format!("http://127.0.0.1:{port}/data")),
            percent(out.to_str().unwrap())
        ),
    );
    api("POST", "/api/run?max=1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        let body = api("GET", "/api/jobs");
        if body.contains(r#""state":"done""#) {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(done, "job never reached done state");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    let _ = std::fs::remove_file(&jobs_path);
    cleanup(&out);
}

fn percent(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-.~_/: ".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------- 5. browser-extension add flow ----------

#[test]
fn extension_add_enqueues_and_autostarts() {
    fn api_body(method: &str, target: &str, json: &str, port: u16, token: &str) -> String {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let sep = if target.contains('?') { '&' } else { '?' };
        let req = format!(
            "{} {}{}token={} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            method,
            target,
            sep,
            token,
            json.len(),
            json
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).unwrap();
        String::from_utf8_lossy(&resp).into_owned()
    }

    // backend file server
    let data = body(64 * 1024);
    let d2 = Arc::new(data.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let (_, _) = read_request(&mut s);
            respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
        }
    });

    // UI server
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs_path = tempfile("ext.jobs");
    let dir = tempfile("ext_dir");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("ext_file.bin");
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port,
        jobs_path: jobs_path.clone(),
        parts: 1,
        max_parallel: 1,
    };
    std::thread::spawn(move || {
        let _ = dm_engine::server::serve_on(cfg, ui);
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let token = std::fs::read_to_string(format!("{}.token", jobs_path.display()))
        .unwrap()
        .trim()
        .to_string();
    let api = |method: &str, target: &str| -> String {
        api_body(method, target, "", ui_port, &token)
    };

    // point the default download dir at our temp dir, then extension-add
    let _ = api(
        "POST",
        &format!("/api/settings?defaultDir={}", percent(&dir.to_string_lossy())),
    );
    let add_body = format!(
        r#"{{"url":"http://127.0.0.1:{port}/data","referrer":"http://127.0.0.1:{port}/page","fileName":"ext_file.bin"}}"#
    );
    let resp = api_body("POST", "/api/extension/add", &add_body, ui_port, &token);
    assert!(resp.contains("\"added\""), "extension add failed: {resp}");

    // auto-start should complete the job
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        let body = api("GET", "/api/jobs");
        if body.contains(r#""state":"done""#) {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(done, "extension job never completed");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    for f in [&jobs_path, &out] {
        let _ = std::fs::remove_file(f);
    }
}