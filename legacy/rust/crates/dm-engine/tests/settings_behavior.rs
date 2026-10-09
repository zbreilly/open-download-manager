//! Settings drive real behavior: login item + clipboard candidate detection.

use std::io::{Read, Write};
use std::net::TcpListener;

fn raw(port: u16, req: &str) -> String {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).unwrap();
    String::from_utf8_lossy(&resp).into_owned()
}

#[test]
fn login_item_plist_created_and_removed() {
    let ui = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_port = ui.local_addr().unwrap().port();
    let jobs = std::env::temp_dir().join(format!("sb_{}.jobs", std::process::id()));
    let agent = std::env::temp_dir().join(format!("sb_{}.plist", std::process::id()));
    std::env::set_var("ODM_LAUNCH_AGENT_PATH", &agent);
    let _ = std::fs::remove_file(&agent);
    let cfg = dm_engine::server::ServerConfig {
        port: ui_port, jobs_path: jobs.clone(), parts: 1, max_parallel: 1,
    };
    std::thread::spawn(move || { let _ = dm_engine::server::serve_on(cfg, ui); });
    std::thread::sleep(std::time::Duration::from_millis(300));
    let token = std::fs::read_to_string(format!("{}.token", jobs.display()))
        .unwrap().trim().to_string();

    // enable -> plist exists with the app path
    raw(ui_port, &format!(
        "POST /api/settings?launchAtLogin=1&token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(agent.exists(), "LaunchAgent plist must be written");
    let plist = std::fs::read_to_string(&agent).unwrap();
    assert!(plist.contains("app.opendm.odm"), "plist has the label");
    assert!(plist.contains("OpenDM.app"), "plist launches the app");

    // disable -> plist removed
    raw(ui_port, &format!(
        "POST /api/settings?launchAtLogin=0&token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"));
    assert!(!agent.exists(), "LaunchAgent plist must be removed");

    std::env::remove_var("ODM_LAUNCH_AGENT_PATH");
    let _ = std::fs::remove_file(&jobs);
    let _ = std::fs::remove_file(format!("{}.token", jobs.display()));
}

#[test]
fn clipboard_candidate_detection() {
    // single download URL with monitored extension -> detected
    assert_eq!(
        dm_engine::server::clipboard_download_candidate(
            "https://example.com/files/setup.zip"),
        Some("https://example.com/files/setup.zip".into()));
    // unmonitored extension -> ignored
    assert_eq!(
        dm_engine::server::clipboard_download_candidate(
            "https://example.com/page.html"),
        None);
    // multi-line / text around URL -> ignored
    assert_eq!(
        dm_engine::server::clipboard_download_candidate(
            "check this https://example.com/f.rar out"),
        None);
    assert_eq!(
        dm_engine::server::clipboard_download_candidate(
            "https://a.com/1.zip\nhttps://b.com/2.zip"),
        None);
    // plain text -> ignored
    assert_eq!(
        dm_engine::server::clipboard_download_candidate(
            "meeting notes 10am"),
        None);
}
