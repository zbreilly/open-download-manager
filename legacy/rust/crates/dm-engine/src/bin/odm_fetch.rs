//! odm-fetch — Open Download Manager command-line downloader.
//!
//! usage: odm-fetch [--limit BYTES] [--parts N] <url> <output-file>
//!
//! Downloads with automatic resume: an interrupted run leaves
//! `<output-file>.part` (plus a `.state` checkpoint for multi-segment
//! runs), and re-running the same command resumes. `--parts N >= 2`
//! enables multi-segment downloading (N connections in parallel).

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut limit: Option<u64> = None;
    let mut parts: usize = 1;
    let mut sha256: Option<String> = None;
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--limit" => {
                limit = it
                    .next()
                    .and_then(|v| v.parse::<u64>().ok());
            }
            "--parts" => {
                parts = it.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
            }
            "--sha256" => {
                sha256 = it.next().map(|v| v.to_string());
            }
            _ => positional.push(a.clone()),
        }
    }
    let (url, out) = match positional.as_slice() {
        [u, o] => (u.clone(), std::path::PathBuf::from(o)),
        _ => {
            eprintln!("usage: odm-fetch [--limit BYTES] <url> <output-file>");
            return ExitCode::from(2);
        }
    };

    let start = std::time::Instant::now();
    let mut cb = |done: u64, total: Option<u64>| match total {
        Some(t) if t > 0 => {
            let pct = (done as f64 / t as f64 * 100.0).min(100.0);
            eprintln!("{:>6.2}%  {}/{} bytes", pct, done, t);
        }
        _ => eprintln!("{} bytes", done),
    };
    let result = match sha256 {
        Some(ref hex) => dm_engine::verify::download_verified(&url, &out, parts, hex),
        None if parts >= 2 => {
            dm_engine::multi::download_multi(&url, &out, parts, Some(&mut cb), limit)
        }
        None => dm_engine::download_with(&url, &out, Some(&mut cb), limit),
    };

    match result {
        Ok(stats) => {
            eprintln!(
                "saved {} ({} bytes, resumed from {}, {} segments split, {:.1}s)",
                out.display(),
                stats.bytes_written,
                stats.resumed_from,
                stats.splits,
                start.elapsed().as_secs_f64()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
