//! FTP downloads against a local test server: split transfers, resume
//! after dropped connections and after a pause, login failures.
mod common;

use common::ftp::{self as ftpsrv, FtpOpts};
use odm_core::*;
use std::time::{Duration, Instant};

fn mgr(dir: &std::path::Path) -> Manager {
    let m = Manager::open(&dir.join("odm.db")).unwrap();
    let mut s = m.settings();
    s.default_folder = dir.join("dl").to_string_lossy().into_owned();
    s.retry_delay_secs = 0;
    s.max_retries = 20;
    s.min_segment_kb = 64;
    s.proxy_mode = "none".into();
    m.save_settings(s).unwrap();
    m.start_background();
    m
}

async fn wait_status(m: &Manager, id: &str, want: Status, secs: u64) -> Download {
    let t = Instant::now();
    loop {
        let d = m.get(id).expect("download exists");
        if d.status == want {
            return d;
        }
        if t.elapsed() > Duration::from_secs(secs) {
            panic!("timeout waiting for {want:?}; now {:?} err={:?}", d.status, d.error);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

fn opts() -> FtpOpts {
    FtpOpts { user: "alice".into(), pass: "s3cret".into(), ..Default::default() }
}

fn add(m: &Manager, url: String, conns: u32, user: Option<&str>, pass: Option<&str>) -> String {
    m.add(AddRequest {
        url,
        start: true,
        connections: Some(conns),
        username: user.map(String::from),
        password: pass.map(String::from),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_reports_size_and_resume() {
    let srv = ftpsrv::start(opts()).await;
    srv.put("/pub/file.zip", common::expected(123_456));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let p = m
        .probe(&srv.url("/pub/file.zip"), None, None, vec![], Some("alice".into()), Some("s3cret".into()))
        .await
        .unwrap();
    assert_eq!(p.total_bytes, Some(123_456));
    assert!(p.resumable);
    assert_eq!(p.protocol, "FTP");
    assert_eq!(p.file_name, "file.zip");
    assert_eq!(p.category, "compressed");
    assert_eq!(p.last_modified.as_deref(), Some("20260101120000"));
    // a missing file is an error
    let e = m.probe(&srv.url("/nope.bin"), None, None, vec![], Some("alice".into()), Some("s3cret".into())).await;
    assert!(e.unwrap_err().to_string().contains("550"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn split_download_with_credentials_from_the_url() {
    let srv = ftpsrv::start(opts()).await;
    let size = 3 * 1024 * 1024 + 77;
    srv.put("/big.iso", common::expected(size));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let url = format!("ftp://alice:s3cret@127.0.0.1:{}/big.iso", srv.port);
    let id = add(&m, url, 4, None, None);
    // the password lives in the credential fields, never in the stored URL
    let d = m.get(&id).unwrap();
    assert!(!d.url.contains("s3cret") && d.url.ends_with("/big.iso"));
    assert_eq!((d.username.as_deref(), d.password.as_deref()), (Some("alice"), Some("s3cret")));
    let d = wait_status(&m, &id, Status::Complete, 30).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), common::expected(size));
    assert_eq!(d.protocol.as_deref(), Some("FTP"));
    // several connections, each starting where its segment begins
    assert!(srv.rests().iter().any(|r| *r > 0), "no REST: {:?}", srv.commands());
    assert!(srv.transfers.load(std::sync::atomic::Ordering::SeqCst) >= 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_after_dropped_connections() {
    let srv = ftpsrv::start(FtpOpts { drop_after: Some(100_000), drops: 3, no_epsv: true, ..opts() }).await;
    let size = 400_000;
    srv.put("/f.bin", common::expected(size as u64));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, srv.url("/f.bin"), 1, Some("alice"), Some("s3cret"));
    let d = wait_status(&m, &id, Status::Complete, 30).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), common::expected(size as u64));
    // each reconnect continued from what was already written
    // (the probe's `REST 0` only checks for resume support)
    let rests: Vec<u64> = srv.rests().into_iter().filter(|r| *r > 0).collect();
    assert_eq!(&rests[..3], &[100_000, 200_000, 300_000], "{:?}", srv.commands());
    // PASV was used (EPSV refused) and worked with the bogus host
    assert!(srv.commands().iter().any(|c| c == "PASV"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_continue_from_the_offset() {
    let srv = ftpsrv::start(FtpOpts { delay_ms: 20, ..opts() }).await;
    let size = 600_000u64;
    srv.put("/slow.bin", common::expected(size));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, srv.url("/slow.bin"), 1, Some("alice"), Some("s3cret"));
    let t = Instant::now();
    while m.get(&id).unwrap().received_bytes < 100_000 {
        assert!(t.elapsed() < Duration::from_secs(20), "no progress");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    m.pause(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Paused, 10).await;
    let at = d.received_bytes;
    assert!(at > 0 && at < size);
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), common::expected(size));
    assert!(srv.rests().contains(&at), "expected REST {at}: {:?}", srv.rests());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_password_fails_clearly() {
    let srv = ftpsrv::start(opts()).await;
    srv.put("/f.bin", common::expected(1000));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, srv.url("/f.bin"), 1, Some("alice"), Some("wrong"));
    let d = wait_status(&m, &id, Status::Error, 10).await;
    assert!(d.error.as_deref().unwrap_or("").contains("login failed"), "{:?}", d.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_and_no_resume_servers() {
    let srv = ftpsrv::start(FtpOpts { allow_anonymous: true, no_rest: true, ..opts() }).await;
    srv.put("/pub/a.txt", common::expected(250_000));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, srv.url("/pub/a.txt"), 8, None, None);
    let d = wait_status(&m, &id, Status::Complete, 20).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), common::expected(250_000));
    assert!(!d.resumable);
    assert!(srv.commands().iter().any(|c| c == "USER anonymous"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_limits_reduce_connections() {
    let srv = ftpsrv::start(FtpOpts { max_sessions: Some(2), delay_ms: 2, ..opts() }).await;
    let size = 2 * 1024 * 1024;
    srv.put("/lim.bin", common::expected(size));
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, srv.url("/lim.bin"), 8, Some("alice"), Some("s3cret"));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), common::expected(size));
}
