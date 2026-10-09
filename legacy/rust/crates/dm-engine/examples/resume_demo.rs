//! In-process resume demo: spawns a Range-capable server thread, makes an
//! interrupted 1 MB download, then resumes to completion.
//!
//! Run with: cargo run -p dm-engine --example resume_demo

use std::io::{Read, Write};
use std::net::TcpListener;

fn body(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x: u64 = 0xfeed_beef_cafe_f00d;
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        v.push((x >> 33) as u8);
    }
    v
}

fn main() {
    let data = body(4 * 1024 * 1024);

    // range-capable server (same shape as the test suite's)
    let d2 = data.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let mut buf = Vec::new();
            let mut b = [0u8; 512];
            loop {
                match s.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&b[..n]),
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            let range = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                .and_then(|l| {
                    l.split(':').nth(1)?.trim()[6..]
                        .split('-')
                        .next()?
                        .parse::<usize>()
                        .ok()
                });
            // writes may fail when the client hangs up mid-transfer
            // (resume demos do that on purpose) — just serve the next one
            let _ = match range {
                Some(start) => {
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        d2.len() - 1,
                        d2.len(),
                        d2.len() - start
                    );
                    s.write_all(head.as_bytes()).and_then(|_| s.write_all(&d2[start..]))
                }
                None => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        d2.len()
                    );
                    s.write_all(head.as_bytes()).and_then(|_| s.write_all(&d2))
                }
            };
        }
    });

    let url = format!("http://127.0.0.1:{port}/big.bin");
    let out = std::env::temp_dir().join("odm_resume_demo.bin");
    let part = std::env::temp_dir().join("odm_resume_demo.bin.part");
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&part);

    println!("== interrupted download (limit 1 MiB) ==");
    let stats = dm_engine::download_with(&url, &out, None, Some(1024 * 1024)).unwrap();
    println!("stopped at {} bytes; .part present: {}", stats.bytes_written, part.exists());

    println!("== resuming ==");
    let stats = dm_engine::download(&url, &out).unwrap();
    println!("resumed from byte {} and completed", stats.resumed_from);

    let got = std::fs::read(&out).unwrap();
    println!(
        "verify: {} bytes, content matches: {}",
        got.len(),
        got == data
    );
    let _ = std::fs::remove_file(&out);
}
