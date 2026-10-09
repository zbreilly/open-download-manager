//! Pause/resume + progress plumbing tests.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn body(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x: u64 = 0x1bad_cafe_0000_0001;
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(3);
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

/// trickle server: 64 KB every 50 ms (~1.3 MB/s), range-capable
fn slow_server(data: Vec<u8>) -> u16 {
    let d2 = Arc::new(data);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let (text, range) = read_request(&mut s);
            if std::env::var("ODM_DEBUG").is_ok() {
                let first = text.lines().next().unwrap_or("").to_string();
                eprintln!("[srv] {first} range={range:?}");
            }
            let d2 = Arc::clone(&d2);
            std::thread::spawn(move || {
                let (head, slice) = match range {
                    Some(start) => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                            d2.len() - 1,
                            d2.len()
                        ),
                        d2[start as usize..].to_vec(),
                    ),
                    None => ("HTTP/1.1 200 OK".to_string(), (*d2).clone()),
                };
                let _ = s.write_all(
                    format!(
                        "{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        head,
                        slice.len()
                    )
                    .as_bytes(),
                );
                for chunk in slice.chunks(64 * 1024) {
                    if s.write_all(chunk).is_err() {
                        break;
                    }
                    let _ = s.flush();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
        }
    });
    port
}

fn tempfile(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("odm_pp_{}_{}", std::process::id(), name));
    let mut s = p.as_os_str().to_os_string();
    s.push(".part");
    let _ = std::fs::remove_file(PathBuf::from(&s));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn engine_pause_checkpoints_and_resumes() {
    let data = body(1024 * 1024);
    let port = slow_server(data.clone());
    let out = tempfile("pause.bin");

    let flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
    }
    let err = dm_engine::download_with_ctl(
        &format!("http://127.0.0.1:{port}/data"),
        &out,
        None,
        None,
        Some(&flag),
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(err, dm_engine::EngineError::Cancelled));

    let mut part = out.as_os_str().to_os_string();
    part.push(".part");
    let partial = std::fs::metadata(PathBuf::from(&part)).unwrap().len();
    assert!(partial > 0 && partial < data.len() as u64, "partial={partial} full={}", data.len());
    assert!(!out.exists());

    // resume: same command without the flag completes byte-correct
    dm_engine::download(&format!("http://127.0.0.1:{port}/data"), &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    let _ = std::fs::remove_file(PathBuf::from(part));
    let _ = std::fs::remove_file(&out);
}

#[test]
fn queue_emits_paused_event() {
    let data = body(1024 * 1024);
    let port = slow_server(data.clone());
    let out = tempfile("qpause.bin");
    let jobs = vec![(format!("http://127.0.0.1:{port}/data"), out.clone())];

    let controls = dm_engine::queue::JobControls::new();
    controls.register(0); // queue index 0
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let controls = controls.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            stop.store(true, Ordering::SeqCst);
            controls.pause(0);
        });
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = Arc::clone(&events);
    dm_engine::queue::run_jobs_ctl(jobs, 1, None, Some(&controls), &mut move |ev| {
        if matches!(
            ev,
            dm_engine::queue::JobEvent::Done { .. }
                | dm_engine::queue::JobEvent::Failed { .. }
                | dm_engine::queue::JobEvent::Paused { .. }
        ) {
            ev2.lock().unwrap().push(format!("{ev:?}"));
        }
        let _ = stop.load(Ordering::SeqCst);
    });

    let ev = events.lock().unwrap();
    assert!(
        ev.iter().any(|e| e.contains("Paused")),
        "no Paused event; got {ev:?}"
    );
    assert_eq!(ev.len(), 1, "job should terminate exactly once");
}

#[test]
fn server_pause_resume_roundtrip() {
    let data = body(8 * 1024 * 1024); // 8 MiB
    let port = slow_server(data.clone());
    let out = tempfile("pp2_dl.bin");

    // UI server
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs_path = tempfile("pp2.jobs");
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
        .unwrap()
        .trim()
        .to_string();
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

    // add + start
    let _ = api(
        "POST",
        &format!(
            "/api/add?url={}&out={}",
            format!("http://127.0.0.1:{port}/data"),
            out.to_str().unwrap()
        ),
    );
    let _ = api("POST", "/api/run?max=1");

    // DETERMINISTIC: wait until the job is actively downloading with real
    // bytes on disk (the trickle guarantees a wide window: 8 MiB at
    // 64 KB/50 ms needs ~6.4 s), then pause.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut seen_bytes = 0u64;
    while std::time::Instant::now() < deadline {
        let body = api("GET", "/api/jobs");
        if body.contains("\"status\":\"downloading\"")
            && body.contains("\"receivedBytes\":")
        {
            // parse receivedBytes
            if let Some(pos) = body.find("\"receivedBytes\":") {
                let rest = &body[pos + 16..];
                let num: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(v) = num.parse::<u64>() {
                    seen_bytes = v;
                    if v >= 64 * 1024 {
                        break; // comfortably mid-transfer
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        seen_bytes >= 64 * 1024,
        "download never showed progress (seen={seen_bytes})"
    );

    // pause → paused state + partial file
    let _ = api("POST", "/api/pause?id=1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut paused = false;
    while std::time::Instant::now() < deadline {
        if api("GET", "/api/jobs").contains("\"state\":\"paused\"") {
            paused = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    assert!(paused, "never paused");

    let mut part = out.as_os_str().to_os_string();
    part.push(".part");
    let partial = std::fs::metadata(PathBuf::from(&part))
        .map(|m| m.len())
        .unwrap_or(0);
    assert!(partial > 0 && partial < data.len() as u64);

    // resume → re-queue, run again, completes byte-correct
    let _ = api("POST", "/api/resume?id=1");
    let _ = api("POST", "/api/run?max=1");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        if api("GET", "/api/jobs").contains("\"state\":\"done\"") {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(done, "never completed after resume");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    for f in [&jobs_path, &out] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(PathBuf::from(part));
}


// keep Mutex import used (events shared through Arc<Mutex>)
#[allow(unused_imports)]
use std::sync::Mutex;
