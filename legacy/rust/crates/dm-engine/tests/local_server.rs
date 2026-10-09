//! End-to-end engine tests against local HTTP servers (no network needed).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

/// deterministic pseudo-random body (reproducible across runs)
fn body(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x: u64 = 0x1234_5678_9abc_def0;
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        v.push((x >> 33) as u8);
    }
    v
}

/// Spawn a handler thread per connection; returns the port.
fn spawn_server<F>(handler: F) -> u16
where
    F: Fn(&mut std::net::TcpStream) + Send + 'static,
    F: Clone,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
            handler(&mut stream);
        }
    });
    port
}

/// Read the request head off the stream, returning (head, range_start).
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
            let v = l.split(':').nth(1)?.trim(); // "bytes=100-"
            v.trim_start_matches("bytes=").split('-').next()?.parse().ok()
        });
    (text, range)
}

fn respond(stream: &mut std::net::TcpStream, head: String, body: &[u8]) {
    let resp = format!(
        "{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        head,
        body.len()
    );
    // client may hang up mid-transfer (resume tests) — that's fine
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[test]
fn full_download_content_length() {
    let data = body(256 * 1024);
    let d2 = data.clone();
    let port = spawn_server(move |stream| {
        let (_, range) = read_request(stream);
        match range {
            Some(start) => {
                let mut stream = stream;
                respond(
                    &mut stream,
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                        d2.len() - 1,
                        d2.len()
                    ),
                    &d2[start as usize..],
                );
            }
            None => {
                let mut stream = stream;
                respond(&mut stream, "HTTP/1.1 200 OK".into(), &d2);
            }
        }
    });

    let out = tempfile("odm_full.bin");
    let stats = dm_engine::download(&format!("http://127.0.0.1:{port}/data"), &out).unwrap();
    assert_eq!(stats.bytes_written, data.len() as u64);
    assert_eq!(stats.resumed_from, 0);
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(!out.with_extension("bin.part").exists());
}

#[test]
fn resume_with_range_support() {
    let data = body(128 * 1024);
    let d2 = data.clone();
    let port = spawn_server(move |stream| {
        let (_, range) = read_request(stream);
        match range {
            Some(start) => {
                let mut stream = stream;
                respond(
                    &mut stream,
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                        d2.len() - 1,
                        d2.len()
                    ),
                    &d2[start as usize..],
                );
            }
            None => {
                let mut stream = stream;
                respond(&mut stream, "HTTP/1.1 200 OK".into(), &d2);
            }
        }
    });

    let out = tempfile("odm_resume.bin");
    // simulate an interrupted download
    let have = 50_000usize;
    std::fs::write(out.with_extension("bin.part"), &data[..have]).unwrap();

    let stats = dm_engine::download(&format!("http://127.0.0.1:{port}/data"), &out).unwrap();
    assert_eq!(stats.resumed_from, have as u64);
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

#[test]
fn resume_falls_back_when_server_ignores_range() {
    let data = body(64 * 1024);
    let d2 = data.clone();
    // this server ignores Range and always returns 200
    let port = spawn_server(move |stream| {
        let (_, _) = read_request(stream);
        let mut stream = stream;
        respond(&mut stream, "HTTP/1.1 200 OK".into(), &d2);
    });

    let out = tempfile("odm_norange.bin");
    std::fs::write(out.with_extension("bin.part"), &data[..10_000]).unwrap();

    let stats = dm_engine::download(&format!("http://127.0.0.1:{port}/data"), &out).unwrap();
    assert_eq!(stats.resumed_from, 0); // restarted from scratch
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

#[test]
fn chunked_download() {
    let data = body(96 * 1024);
    let d2 = data.clone();
    let port = spawn_server(move |stream| {
        let (_, _) = read_request(stream);
        let mut stream = stream;
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        for chunk in d2.chunks(7_777) {
            if stream
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .is_err()
            {
                break;
            }
            if stream.write_all(chunk).is_err() {
                break;
            }
            let _ = stream.write_all(b"\r\n");
        }
        let _ = stream.write_all(b"0\r\nX-Trailer: v\r\n\r\n");
        let _ = stream.flush();
    });

    let out = tempfile("odm_chunked.bin");
    dm_engine::download(&format!("http://127.0.0.1:{port}/data"), &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

#[test]
fn redirect_followed() {
    let data = body(32 * 1024);
    let d2 = data.clone();
    // final destination
    let data_port = spawn_server(move |stream| {
        let (_, _) = read_request(stream);
        let mut stream = stream;
        respond(&mut stream, "HTTP/1.1 200 OK".into(), &d2);
    });
    // redirector: /goto -> http://127.0.0.1:<data_port>/data
    let r2 = data_port;
    let port = spawn_server(move |stream| {
        let (_, _) = read_request(stream);
        let mut stream = stream;
        respond(
            &mut stream,
            format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{r2}/data"),
            b"",
        );
    });

    let out = tempfile("odm_redirect.bin");
    let stats = dm_engine::download(&format!("http://127.0.0.1:{port}/goto"), &out).unwrap();
    assert!(stats.final_url.ends_with("/data"));
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

#[test]
fn limit_leaves_part_then_resume_completes() {
    let data = body(80 * 1024);
    let d2 = data.clone();
    let port = spawn_server(move |stream| {
        let (_, range) = read_request(stream);
        match range {
            Some(start) => {
                let mut stream = stream;
                respond(
                    &mut stream,
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                        d2.len() - 1,
                        d2.len()
                    ),
                    &d2[start as usize..],
                );
            }
            None => {
                let mut stream = stream;
                respond(&mut stream, "HTTP/1.1 200 OK".into(), &d2);
            }
        }
    });
    let url = format!("http://127.0.0.1:{port}/data");

    let out = tempfile("odm_limit.bin");
    // first run: stop after 1000 bytes (leaves .part)
    let stats = dm_engine::download_with(&url, &out, None, Some(1_000)).unwrap();
    assert_eq!(stats.bytes_written, 1_000);
    let part = out.with_extension("bin.part");
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 1_000);
    assert!(!out.exists());

    // second run: no limit — resumes and completes
    let stats = dm_engine::download(&url, &out).unwrap();
    assert_eq!(stats.resumed_from, 1_000);
    assert_eq!(std::fs::read(&out).unwrap(), data);
}

fn tempfile(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("odm_test_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(p.with_extension("bin.part"));
    p
}
