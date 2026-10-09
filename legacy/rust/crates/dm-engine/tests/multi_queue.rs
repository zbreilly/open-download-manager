//! Multi-segment + queue tests against local servers.

use std::io::{Read, Seek, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn body(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x: u64 = 0xdead_beef_cafe_babe;
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(99);
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

/// Range-capable server.
fn range_server(data: Vec<u8>) -> u16 {
    let data = Arc::new(data);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let (_, range) = read_request(&mut s);
            match range {
                Some(start) => respond(
                    &mut s,
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                        data.len() - 1,
                        data.len()
                    ),
                    &data[start as usize..],
                ),
                None => respond(&mut s, "HTTP/1.1 200 OK".into(), &data),
            }
        }
    });
    port
}

fn tempfile(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("odm_mq_{}_{}", std::process::id(), name));
    cleanup(&p);
    p
}

fn cleanup(p: &PathBuf) {
    let mut s = p.as_os_str().to_os_string();
    s.push(".part");
    let _ = std::fs::remove_file(PathBuf::from(s.clone()));
    let mut st = s;
    st.push(".state");
    let _ = std::fs::remove_file(PathBuf::from(st));
    let _ = std::fs::remove_file(p);
}

#[test]
fn multi_download_matches() {
    let data = body(4 * 1024 * 1024);
    let port = range_server(data.clone());
    let out = tempfile("multi.bin");
    let mut cb = |done: u64, total: Option<u64>| {
        assert!(total.unwrap() == data.len() as u64);
        assert!(done <= total.unwrap());
    };
    let stats = dm_engine::multi::download_multi(
        &format!("http://127.0.0.1:{port}/data"),
        &out,
        4,
        Some(&mut cb),
        None,
    )
    .unwrap();
    assert_eq!(stats.bytes_written, data.len() as u64);
    assert_eq!(std::fs::read(&out).unwrap(), data);
    cleanup(&out);
}

#[test]
fn multi_falls_back_on_small_files() {
    let data = body(64 * 1024); // below MIN_MULTI_SIZE
    let port = range_server(data.clone());
    let out = tempfile("small.bin");
    dm_engine::multi::download_multi(
        &format!("http://127.0.0.1:{port}/data"),
        &out,
        8,
        None,
        None,
    )
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    cleanup(&out);
}

#[test]
fn multi_resume_from_state() {
    let data = body(4 * 1024 * 1024);
    let port = range_server(data.clone());
    let out = tempfile("mresume.bin");
    let part = {
        let mut s = out.as_os_str().to_os_string();
        s.push(".part");
        PathBuf::from(s)
    };
    let state = {
        let mut s = part.as_os_str().to_os_string();
        s.push(".state");
        PathBuf::from(s)
    };

    // simulate an interrupted multi run: seg0 complete, seg1 half, rest none
    let mib = 1024 * 1024u64;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part)
        .unwrap();
    f.set_len(data.len() as u64).unwrap();
    f.seek(std::io::SeekFrom::Start(0)).unwrap();
    f.write_all(&data[..mib as usize]).unwrap();
    f.seek(std::io::SeekFrom::Start(mib)).unwrap();
    f.write_all(&data[mib as usize..(mib as usize + 500_000)]).unwrap();
    drop(f);
    let state_text = format!(
        "0 0 {a} {b}\n1 {c} {d} 500000\n2 {e} {f} 0\n3 {g} {h} 0\n",
        a = mib - 1,
        b = mib,
        c = mib,
        d = 2 * mib - 1,
        e = 2 * mib,
        f = 3 * mib - 1,
        g = 3 * mib,
        h = 4 * mib - 1
    );
    std::fs::write(&state, state_text).unwrap();

    dm_engine::multi::download_multi(
        &format!("http://127.0.0.1:{port}/data"),
        &out,
        4,
        None,
        None,
    )
    .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert!(!state.exists());
    cleanup(&out);
}

#[test]
fn queue_runs_all_and_limits_parallelism() {
    let data = body(200 * 1024);
    let d2 = Arc::new(data.clone());
    let cur = Arc::new(AtomicUsize::new(0));
    let max = Arc::new(Mutex::new(0usize));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    {
        let cur = Arc::clone(&cur);
        let max = Arc::clone(&max);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let cur = Arc::clone(&cur);
                let max = Arc::clone(&max);
                let d2 = Arc::clone(&d2);
                let mut s = stream.unwrap();
                std::thread::spawn(move || {
                    let c = cur.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut m = max.lock().unwrap();
                    if c > *m {
                        *m = c;
                    }
                    drop(m);
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let (_, _) = read_request(&mut s);
                    respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
                    cur.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }

    let jobs: Vec<(String, PathBuf)> = (0..4)
        .map(|i| {
            (
                format!("http://127.0.0.1:{port}/j{i}"),
                tempfile(&format!("q{i}.bin")),
            )
        })
        .collect();

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = Arc::clone(&events);
    dm_engine::queue::run_jobs(jobs.clone(), 2, &mut move |ev| {
        ev2.lock().unwrap().push(ev)
    });

    let ev = events.lock().unwrap();
    let done = ev
        .iter()
        .filter(|e| matches!(e, dm_engine::queue::JobEvent::Done { .. }))
        .count();
    let failed = ev
        .iter()
        .filter(|e| matches!(e, dm_engine::queue::JobEvent::Failed { .. }))
        .count();
    assert_eq!(done, 4);
    assert_eq!(failed, 0);
    assert!(*max.lock().unwrap() <= 2, "parallelism cap violated");
    for (i, (_, out)) in jobs.iter().enumerate() {
        let _ = i;
        assert_eq!(std::fs::read(out).unwrap(), data);
        cleanup(out);
    }
}

#[test]
fn queue_failure_isolation() {
    // one 404 among four jobs must not stop the rest
    let data = body(32 * 1024);
    let d2 = Arc::new(data.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let (text, _) = read_request(&mut s);
            let path = text.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("");
            if path == "/bad" {
                respond(&mut s, "HTTP/1.1 404 Not Found".into(), b"nope");
            } else {
                respond(&mut s, "HTTP/1.1 200 OK".into(), &d2[..]);
            }
        }
    });

    let mut jobs: Vec<(String, PathBuf)> = (0..4)
        .map(|i| {
            let path = if i == 1 { "/bad" } else { &format!("/ok{i}") };
            (format!("http://127.0.0.1:{port}{path}"), tempfile(&format!("qf{i}.bin")))
        })
        .collect();
    jobs[1].0 = format!("http://127.0.0.1:{port}/bad");

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = Arc::clone(&events);
    dm_engine::queue::run_jobs(jobs.clone(), 3, &mut move |ev| {
        ev2.lock().unwrap().push(ev)
    });
    let ev = events.lock().unwrap();
    let done = ev
        .iter()
        .filter(|e| matches!(e, dm_engine::queue::JobEvent::Done { .. }))
        .count();
    let failed = ev
        .iter()
        .filter(|e| matches!(e, dm_engine::queue::JobEvent::Failed { .. }))
        .count();
    assert_eq!(done, 3);
    assert_eq!(failed, 1);
    for (i, (_, out)) in jobs.iter().enumerate() {
        if i == 1 {
            assert!(!out.exists());
        } else {
            assert_eq!(std::fs::read(out).unwrap(), data);
        }
        cleanup(out);
    }
}
