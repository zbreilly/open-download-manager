//! Tests for the security review fixes: what is stored, what is kept and
//! what is sent where.

mod common;

use odm_core::*;
use std::path::Path;
use std::time::{Duration, Instant};

fn mgr(dir: &Path) -> Manager {
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Every byte of the database, its write-ahead log included.
fn database_bytes(dir: &Path) -> Vec<u8> {
    let mut all = vec![];
    for name in ["odm.db", "odm.db-wal"] {
        all.extend(std::fs::read(dir.join(name)).unwrap_or_default());
    }
    all
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_are_protected_in_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let mut s = m.settings();
    s.proxy_pass = "PROXY-SECRET-1".into();
    m.save_settings(s).unwrap();
    // not started: stays in the list with everything it was given
    let id = m
        .add(AddRequest {
            url: "http://127.0.0.1:9/f/a.bin".into(),
            cookies: Some("session=COOKIE-SECRET-2".into()),
            username: Some("user".into()),
            password: Some("PASSWORD-SECRET-3".into()),
            headers: vec![("X-Api-Key".into(), "HEADER-SECRET-4".into())],
            ..Default::default()
        })
        .unwrap();
    let key = m.bridge_key();
    assert_eq!(key.len(), 64);
    m.shutdown_wait(Duration::from_secs(5)).await;
    drop(m);

    let raw = database_bytes(dir.path());
    assert!(contains(&raw, "a.bin"), "the database was read");
    if cfg!(windows) {
        for secret in ["PROXY-SECRET-1", "COOKIE-SECRET-2", "PASSWORD-SECRET-3", "HEADER-SECRET-4", key.as_str()] {
            assert!(!contains(&raw, secret), "{secret} is readable in the database");
        }
    }

    // and everything comes back for the same user
    let m = Manager::open(&dir.path().join("odm.db")).unwrap();
    let d = m.get(&id).unwrap();
    assert_eq!(d.cookies.as_deref(), Some("session=COOKIE-SECRET-2"));
    assert_eq!(d.password.as_deref(), Some("PASSWORD-SECRET-3"));
    assert_eq!(d.headers, vec![("X-Api-Key".to_string(), "HEADER-SECRET-4".to_string())]);
    assert_eq!(m.settings().proxy_pass, "PROXY-SECRET-1");
    assert_eq!(m.bridge_key(), key, "the pairing key survives a restart");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_left_by_an_older_version_are_protected_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("odm.db");
    // a database as older versions wrote it: secrets as plain text
    let old = serde_json::json!({
        "id": "old-1", "url": "https://h.example/f.zip", "fileName": "f.zip", "ext": "ZIP",
        "category": "general", "queueId": null, "saveDir": dir.path().join("dl"), "totalBytes": null,
        "receivedBytes": 0, "status": "paused", "resumable": true, "maxConnections": 4,
        "error": null, "addedAt": "2026-01-01T00:00:00+00:00", "completedAt": null,
        "cookies": "session=OLD-COOKIE-SECRET", "password": "OLD-PASSWORD-SECRET",
        "headers": [["X-Token", "OLD-HEADER-SECRET"]]
    });
    drop(Manager::open(&db).unwrap()); // creates the tables
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO downloads (id, added, data) VALUES (?1, ?2, ?3)",
            rusqlite::params!["old-1", "2026-01-01T00:00:00+00:00", old.to_string()],
        )
        .unwrap();
    }
    assert!(contains(&database_bytes(dir.path()), "OLD-COOKIE-SECRET"));

    let m = Manager::open(&db).unwrap();
    let d = m.get("old-1").unwrap();
    assert_eq!(d.cookies.as_deref(), Some("session=OLD-COOKIE-SECRET"));
    assert_eq!(d.password.as_deref(), Some("OLD-PASSWORD-SECRET"));
    assert_eq!(d.headers[0].1, "OLD-HEADER-SECRET");
    m.shutdown_wait(Duration::from_secs(5)).await;
    drop(m);
    if cfg!(windows) {
        let raw = database_bytes(dir.path());
        for secret in ["OLD-COOKIE-SECRET", "OLD-PASSWORD-SECRET", "OLD-HEADER-SECRET"] {
            assert!(!contains(&raw, secret), "{secret} is still readable in the database");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_are_dropped_when_a_download_completes() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = m
        .add(AddRequest {
            url: format!("{}/f/done.bin?size=200000", srv.base),
            start: true,
            cookies: Some("session=SECRET".into()),
            username: Some("user".into()),
            password: Some("pw".into()),
            headers: vec![("X-Api-Key".into(), "KEY".into())],
            referrer: Some("https://site.example/page".into()),
            connections: Some(2),
            ..Default::default()
        })
        .unwrap();
    let d = wait_status(&m, &id, Status::Complete, 20).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(200_000));
    // they were used for the transfer...
    let first = srv.requests()[0].to_ascii_lowercase();
    assert!(first.contains("cookie: session=secret") && first.contains("authorization: basic") && first.contains("x-api-key: key"));
    // ...and are gone once the file is there
    assert_eq!(d.cookies, None);
    assert_eq!(d.password, None);
    assert!(d.headers.is_empty());
    assert_eq!(d.referrer.as_deref(), Some("https://site.example/page"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_link_keeps_credentials_on_the_same_site_only() {
    let site = common::start().await;
    let other = common::start().await; // another port = another origin
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let add = |name: &str| {
        m.add(AddRequest {
            url: format!("{}/f/{name}?size=150000", site.base),
            cookies: Some("session=SECRET".into()),
            username: Some("user".into()),
            password: Some("pw".into()),
            headers: vec![("X-Api-Key".into(), "KEY".into())],
            ..Default::default()
        })
        .unwrap()
    };

    // a new link on the same site still gets them
    let same = add("same.bin");
    m.refresh_link(&same, &format!("{}/f/same2.bin?size=150000", site.base)).await.unwrap();
    let d = m.get(&same).unwrap();
    assert_eq!(d.cookies.as_deref(), Some("session=SECRET"));
    assert_eq!(d.password.as_deref(), Some("pw"));
    assert!(site.requests().iter().any(|r| r.to_ascii_lowercase().contains("cookie: session=secret")));

    // a link on another site never sees them, and they are forgotten
    let moved = add("moved.bin");
    m.refresh_link(&moved, &format!("{}/f/moved.bin?size=150000", other.base)).await.unwrap();
    for r in other.requests() {
        let lower = r.to_ascii_lowercase();
        assert!(!lower.contains("cookie:") && !lower.contains("authorization:") && !lower.contains("x-api-key:"), "leaked: {r}");
    }
    let d = m.get(&moved).unwrap();
    assert_eq!((d.cookies, d.username, d.password), (None, None, None));
    assert!(d.headers.is_empty());
    m.resume(std::slice::from_ref(&moved));
    let d = wait_status(&m, &moved, Status::Complete, 20).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(150_000));
    for r in other.requests() {
        assert!(!r.to_ascii_lowercase().contains("cookie:"), "leaked: {r}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_hosts_only_see_the_referrer_origin() {
    let target = common::start().await;
    let front = common::start_redirector(&target.base).await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let page = format!("{}/account/inbox?session=PRIVATE", front.base);
    let id = m
        .add(AddRequest {
            url: format!("{}/f/r.bin?size=100000", front.base),
            start: true,
            referrer: Some(page.clone()),
            connections: Some(1),
            ..Default::default()
        })
        .unwrap();
    wait_status(&m, &id, Status::Complete, 20).await;
    // the page's own site gets the page address
    assert!(front.requests()[0].contains(&format!("referer: {page}")) || front.requests()[0].contains(&format!("Referer: {page}")));
    // the host it redirects to learns only where the page lives
    for r in target.requests() {
        assert!(!r.contains("PRIVATE") && !r.contains("/account/inbox"), "page address leaked: {r}");
        assert!(r.to_ascii_lowercase().contains(&format!("referer: {}/\r\n", front.base)), "{r}");
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finished_file_carries_the_web_mark() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = m
        .add(AddRequest { url: format!("{}/f/setup.exe?size=120000", srv.base), start: true, ..Default::default() })
        .unwrap();
    let d = wait_status(&m, &id, Status::Complete, 20).await;
    let mut ads = d.final_path().into_os_string();
    ads.push(":Zone.Identifier");
    let mark = std::fs::read_to_string(&ads).expect("Zone.Identifier stream");
    assert!(mark.contains("ZoneId=3"), "{mark}");
}
