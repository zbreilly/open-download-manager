//! odm-serve — Open Download Manager local control server + web UI.
//!
//! usage: odm-serve [--port 8080] [--jobs odm.jobs] [--parts 4] [--max-parallel 2]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut port = 8080u16;
    let mut jobs_path = std::path::PathBuf::from("odm.jobs");
    let mut parts = 4usize;
    let mut max_parallel = 2usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => port = it.next().and_then(|v| v.parse().ok()).unwrap_or(8080),
            "--jobs" => {
                jobs_path = it.next().map(std::path::PathBuf::from).unwrap_or(jobs_path)
            }
            "--parts" => parts = it.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--max-parallel" => {
                max_parallel = it.next().and_then(|v| v.parse().ok()).unwrap_or(2)
            }
            other => {
                eprintln!("unknown arg: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let cfg = dm_engine::server::ServerConfig { port, jobs_path, parts, max_parallel };
    match dm_engine::server::serve(cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("server error: {e}");
            ExitCode::FAILURE
        }
    }
}
