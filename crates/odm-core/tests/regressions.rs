//! Regression tests for engine defects (numbers refer to the review findings).

mod common;

use odm_core::*;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn mgr_with(dir: &Path, f: impl FnOnce(&mut Settings)) -> Manager {
    let m = Manager::open(&dir.join("odm.db")).unwrap();
    let mut s = m.settings();
    s.default_folder = dir.join("dl").to_string_lossy().into_owned();
    s.retry_delay_secs = 0;
    s.max_retries = 20;
    s.min_segment_kb = 64;
    s.proxy_mode = "none".into();
    f(&mut s);
    m.save_settings(s).unwrap();
    m.start_background();
    m
}

fn mgr(dir: &Path) -> Manager {
    mgr_with(dir, |_| {})
}

async fn wait_for(m: &Manager, id: &str, want: &[Status], secs: u64) -> Download {
    let t = Instant::now();
    loop {
        let d = m.get(id).expect("download exists");
        if want.contains(&d.status) {
            return d;
        }
        if t.elapsed() > Duration::from_secs(secs) {
            panic!("timeout waiting for {want:?}; now {:?} err={:?}", d.status, d.error);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_status(m: &Manager, id: &str, want: Status, secs: u64) -> Download {
    wait_for(m, id, &[want], secs).await
}

fn add(m: &Manager, url: String, start: bool, conns: u32) -> String {
    m.add(AddRequest { url, start, connections: Some(conns), ..Default::default() }).unwrap()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

// 1 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_while_running_produces_correct_file() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 3_000_000u64;
    let id = add(&m, format!("{}/f/r.bin?size={size}&delay=20", srv.base), true, 4);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(m.get(&id).unwrap().received_bytes > 0);
    m.redownload(&id);
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    let data = std::fs::read(d.final_path()).unwrap();
    assert_eq!(data.len() as u64, size);
    assert!(data == common::expected(size), "content mismatch after redownload");
    assert_eq!(d.file_name, "r.bin");
    assert_eq!(files_in(Path::new(&d.save_dir)), vec!["r.bin".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redownload_of_finished_file_keeps_the_copy() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/f/k.bin?size=50000", srv.base), true, 2);
    let first = wait_status(&m, &id, Status::Complete, 10).await;
    m.redownload(&id);
    let d = wait_status(&m, &id, Status::Complete, 10).await;
    assert_eq!(d.file_name, "k (1).bin");
    assert!(std::fs::read(first.final_path()).unwrap() == common::expected(50_000));
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(50_000));
}

// 2, 17 ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_name_downloads_get_distinct_files() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let a = add(&m, format!("{}/a/file.bin?size=900000&salt=1&delay=2", srv.base), true, 4);
    let b = add(&m, format!("{}/b/file.bin?size=600000&salt=2&delay=2", srv.base), true, 4);
    // an explicit "overwrite" must not take over another unfinished download's file
    let c = m
        .add(AddRequest {
            url: format!("{}/c/file.bin?size=300000&salt=3", srv.base),
            file_name: Some("file.bin".into()),
            on_exists: Some("overwrite".into()),
            start: true,
            connections: Some(2),
            ..Default::default()
        })
        .unwrap();
    let names: Vec<String> = [&a, &b, &c].iter().map(|id| m.get(id).unwrap().file_name).collect();
    assert_eq!(names, vec!["file.bin", "file (1).bin", "file (2).bin"]);
    for (id, size, salt) in [(&a, 900_000u64, 1u64), (&b, 600_000, 2), (&c, 300_000, 3)] {
        let d = wait_status(&m, id, Status::Complete, 30).await;
        let data = std::fs::read(d.final_path()).unwrap();
        assert!(data == common::expected_salt(size, salt), "{} has wrong content", d.file_name);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_names_are_reserved_too() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    // both learn "server-name.bin" from Content-Disposition
    let a = add(&m, format!("{}/cd/one?size=400000&salt=4&delay=2", srv.base), true, 2);
    let b = add(&m, format!("{}/cd/two?size=500000&salt=5&delay=2", srv.base), true, 2);
    let da = wait_status(&m, &a, Status::Complete, 30).await;
    let db = wait_status(&m, &b, Status::Complete, 30).await;
    assert_ne!(da.file_name, db.file_name);
    assert!(std::fs::read(da.final_path()).unwrap() == common::expected_salt(400_000, 4));
    assert!(std::fs::read(db.final_path()).unwrap() == common::expected_salt(500_000, 5));
    let mut names = vec![da.file_name, db.file_name];
    names.sort();
    assert_eq!(names, vec!["server-name (1).bin", "server-name.bin"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probed_name_keeps_own_reservation() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let _a = add(&m, format!("{}/a/same.bin?size=1000", srv.base), false, 1);
    // the server also calls it "same.bin"; "same (1).bin" is already this download's own
    let b = add(&m, format!("{}/b/same.bin?size=2000", srv.base), true, 1);
    let d = wait_status(&m, &b, Status::Complete, 10).await;
    assert_eq!(d.file_name, "same (1).bin");
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(2000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_uses_the_same_reservation() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let a = add(&m, format!("{}/f/x.bin?size=1000", srv.base), false, 1);
    let b = add(&m, format!("{}/f/y.bin?size=2000", srv.base), false, 1);
    m.update_download(&b, odm_core::manager::DownloadPatch { file_name: Some("x.bin".into()), ..Default::default() });
    let db = m.get(&b).unwrap();
    assert_eq!(db.file_name, "x (1).bin");
    let save = Path::new(&db.save_dir);
    // placeholders follow the names
    assert!(save.join("x.bin.odmpart").exists());
    assert!(save.join("x (1).bin.odmpart").exists());
    assert!(!save.join("y.bin.odmpart").exists());
    m.resume(&[a.clone(), b.clone()]);
    let da = wait_status(&m, &a, Status::Complete, 10).await;
    let db = wait_status(&m, &b, Status::Complete, 10).await;
    assert!(std::fs::read(da.final_path()).unwrap() == common::expected(1000));
    assert!(std::fs::read(db.final_path()).unwrap() == common::expected(2000));
}

// 3 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credentials_not_sent_across_hosts() {
    let target = common::start().await;
    let front = common::start_redirector(&target.base).await; // other port = other origin
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = m
        .add(AddRequest {
            url: format!("{}/f/x.bin?size=300000", front.base),
            start: true,
            cookies: Some("session=SECRET".into()),
            username: Some("user".into()),
            password: Some("pw".into()),
            headers: vec![("X-Api-Key".into(), "KEY".into())],
            connections: Some(2),
            ..Default::default()
        })
        .unwrap();
    let d = wait_status(&m, &id, Status::Complete, 20).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(300_000));
    let to_target = target.requests();
    assert!(!to_target.is_empty());
    for h in &to_target {
        let lower = h.to_ascii_lowercase();
        assert!(!lower.contains("cookie:"), "cookie leaked: {h}");
        assert!(!lower.contains("authorization:"), "authorization leaked: {h}");
        assert!(!lower.contains("x-api-key:"), "custom header leaked: {h}");
        assert!(lower.contains("user-agent:"));
    }
    // the origin itself did get them
    let first = front.requests()[0].to_ascii_lowercase();
    assert!(first.contains("cookie: session=secret") && first.contains("authorization: basic") && first.contains("x-api-key: key"));
}

// 4, 10 --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_file_restarts_with_new_size() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let (s1, s2) = (2_000_000u64, 2_500_000u64);
    let id = add(&m, format!("{}/v/f.bin?size={s1}&size2={s2}&delay=10", srv.base), true, 2);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    m.pause(std::slice::from_ref(&id));
    wait_status(&m, &id, Status::Paused, 5).await;
    srv.version.store(2, Ordering::SeqCst);
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    let data = std::fs::read(d.final_path()).unwrap();
    assert_eq!(d.total_bytes, Some(s2));
    assert!(d.resumable, "a changed file is still resumable");
    assert_eq!(data.len() as u64, s2);
    assert!(data == common::expected_salt(s2, 7), "content is not the new version");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_etag_on_partial_response_restarts() {
    // the server ignores If-Range and keeps sending 206 for the new version
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 2_000_000u64;
    let id = add(&m, format!("{}/v/g.bin?size={size}&delay=10&noifrange=1", srv.base), true, 2);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    m.pause(std::slice::from_ref(&id));
    wait_status(&m, &id, Status::Paused, 5).await;
    srv.version.store(2, Ordering::SeqCst);
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    let data = std::fs::read(d.final_path()).unwrap();
    assert!(data == common::expected_salt(size, 7), "mixed old and new content");
}

// 5 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_part_file_starts_over() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 3_000_000u64;
    let id = add(&m, format!("{}/f/m.bin?size={size}&delay=30", srv.base), true, 4);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    m.pause(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Paused, 5).await;
    assert!(d.received_bytes > 0);
    std::fs::remove_file(odm_core::util::part_path(&d.final_path())).unwrap();
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size), "zero-filled holes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checksum_failure_downloads_again_in_full() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = m
        .add(AddRequest {
            url: format!("{}/f/c.bin?size=300000", srv.base),
            start: true,
            checksum: Some(Checksum { algo: "sha256".into(), expected: "0".repeat(64) }),
            ..Default::default()
        })
        .unwrap();
    let d = wait_status(&m, &id, Status::Error, 20).await;
    assert_eq!(d.checksum_ok, Some(false));
    let hits = srv.hits.load(Ordering::SeqCst);
    m.resume(std::slice::from_ref(&id));
    wait_for(&m, &id, &[Status::Connecting, Status::Downloading], 5).await;
    let d = wait_for(&m, &id, &[Status::Error, Status::Complete], 20).await;
    assert_eq!(d.checksum_ok, Some(false));
    assert!(srv.hits.load(Ordering::SeqCst) > hits, "nothing was downloaded on retry");
    // the bad copy was replaced by a fresh one, not a zero-filled file
    assert_eq!(d.file_name, "c.bin");
    assert_eq!(files_in(Path::new(&d.save_dir)), vec!["c.bin".to_string()]);
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(300_000));
}

// 6 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_resumable_drops_give_up() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr_with(dir.path(), |s| s.max_retries = 3);
    let id = add(&m, format!("{}/noranges/x.bin?size=700000&drop=200000", srv.base), true, 4);
    let d = wait_status(&m, &id, Status::Error, 30).await;
    assert!(d.error.unwrap().contains("dropping"));
    let hits = srv.hits.load(Ordering::SeqCst);
    assert!(hits < 20, "{hits} requests for 3 retries");
}

// 7 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_wait_stops_everything_and_persists() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let size = 3_000_000u64;
    let (a, b, received);
    {
        let m = mgr_with(dir.path(), |s| s.max_simultaneous = 1);
        a = add(&m, format!("{}/f/a.bin?size={size}&delay=20", srv.base), true, 2);
        b = add(&m, format!("{}/f/b.bin?size={size}&delay=20", srv.base), true, 2);
        wait_status(&m, &a, Status::Downloading, 10).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let t = Instant::now();
        m.shutdown_wait(Duration::from_secs(5)).await;
        assert!(t.elapsed() < Duration::from_secs(3), "shutdown took {:?}", t.elapsed());
        let da = m.get(&a).unwrap();
        assert_eq!(da.status, Status::Paused);
        assert!(da.received_bytes > 0);
        received = da.received_bytes;
        let part = odm_core::util::part_path(&da.final_path());
        assert!(std::fs::metadata(&part).unwrap().len() >= received);
        // nothing starts any more
        m.resume(std::slice::from_ref(&b));
        m.start_queue("main");
        tokio::time::sleep(Duration::from_millis(600)).await;
        let db = m.get(&b).unwrap();
        assert!(!db.status.is_active(), "b started after shutdown: {:?}", db.status);
        assert_eq!(db.received_bytes, 0);
        let wal = dir.path().join("odm.db-wal");
        if wal.exists() {
            assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0, "WAL not checkpointed");
        }
    }
    let m = mgr(dir.path());
    let d = m.get(&a).unwrap();
    assert_eq!(d.status, Status::Paused);
    assert_eq!(d.received_bytes, received, "persisted progress differs from the final state");
    m.resume(std::slice::from_ref(&a));
    let d = wait_status(&m, &a, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

// 9 ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_single_segment_is_split() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 4_000_000u64;
    let save = dir.path().join("dl");
    std::fs::create_dir_all(&save).unwrap();
    std::fs::write(save.join("p.bin"), &common::expected(size)[..100_000]).unwrap();
    let id = m
        .add(AddRequest {
            url: format!("{}/f/p.bin?size={size}&delay=10", srv.base),
            start: true,
            connections: Some(8),
            save_dir: Some(save.to_string_lossy().into()),
            on_exists: Some("resume".into()),
            total_bytes: Some(size),
            ..Default::default()
        })
        .unwrap();
    let mut maxc = 0;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        maxc = maxc.max(m.get(&id).unwrap().active_connections);
        if maxc > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(maxc > 1, "only {maxc} connection(s) with max_connections=8");
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

// 11 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_server_reduces_connections() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr_with(dir.path(), |s| s.max_retries = 1);
    let size = 2_000_000u64;
    let id = add(&m, format!("{}/f/t.bin?size={size}&maxconn=2&delay=5", srv.base), true, 8);
    let d = wait_for(&m, &id, &[Status::Complete, Status::Error], 60).await;
    assert_eq!(d.status, Status::Complete, "error: {:?}", d.error);
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

// 13 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grabber_checks_content_type() {
    let srv = common::start().await;
    odm_core::client::install_crypto();
    let client = reqwest::Client::new();
    let grab = |path: &str| {
        let url = format!("{}{path}", srv.base);
        let client = client.clone();
        async move {
            odm_core::grabber::grab(
                &client,
                &url,
                Default::default(),
                odm_core::grabber::GrabOptions { depth: 0, max_pages: 5, same_host: true, extensions: vec![] },
                &default_categories(),
            )
            .await
            .unwrap()
        }
    };
    // no Content-Type, but the body is HTML
    assert_eq!(grab("/noct").await.len(), 3);
    // declared as binary: not parsed
    assert!(grab("/binct").await.is_empty());
    // a binary without Content-Type is not read as a page
    assert!(grab("/noctbin").await.is_empty());
}

// 14 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleted_downloads_stay_deleted() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    {
        let m = mgr_with(dir.path(), |s| s.max_simultaneous = 8);
        let ids: Vec<String> =
            (0..6).map(|i| add(&m, format!("{}/f/d{i}.bin?size=3000000&delay=20", srv.base), true, 2)).collect();
        let mut save_dir = String::new();
        for id in &ids {
            save_dir = wait_status(&m, id, Status::Downloading, 10).await.save_dir;
        }
        tokio::time::sleep(Duration::from_millis(1800)).await;
        // spread the deletes over a ticker persist
        for id in &ids {
            m.delete(std::slice::from_ref(id), true);
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        tokio::time::sleep(Duration::from_millis(2500)).await;
        m.shutdown_wait(Duration::from_secs(5)).await;
        let left = files_in(Path::new(&save_dir));
        assert!(left.is_empty(), "{left:?}");
    }
    let m = Manager::open(&dir.path().join("odm.db")).unwrap();
    assert!(m.list().is_empty(), "deleted downloads came back: {}", m.list().len());
}

// 15 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_file_completes() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/f/empty.txt?size=0", srv.base), true, 2);
    let d = wait_for(&m, &id, &[Status::Complete, Status::Error], 10).await;
    assert_eq!(d.status, Status::Complete, "error: {:?}", d.error);
    assert_eq!(d.total_bytes, Some(0));
    assert_eq!(std::fs::metadata(d.final_path()).unwrap().len(), 0);
    assert!(!odm_core::util::part_path(&d.final_path()).exists());
}

// 16 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_limit_holds_with_many_connections() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    m.set_global_limit(Some(400 * 1024));
    let size = 1024 * 1024;
    let t = Instant::now();
    let id = add(&m, format!("{}/f/lim16.bin?size={size}", srv.base), true, 16);
    wait_status(&m, &id, Status::Complete, 20).await;
    let secs = t.elapsed().as_secs_f64();
    assert!(secs > 2.0, "limit exceeded: {secs:.2}s");
    assert!(secs < 6.0, "too slow: {secs:.2}s");
}

// 18 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_gives_back_an_adopted_file() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let save = dir.path().join("dl");
    std::fs::create_dir_all(&save).unwrap();
    let mine = b"my own precious file".to_vec();
    std::fs::write(save.join("u.bin"), &mine).unwrap();
    let id = m
        .add(AddRequest {
            url: format!("{}/f/u.bin?size=100000", srv.base),
            start: false,
            save_dir: Some(save.to_string_lossy().into()),
            on_exists: Some("resume".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(!save.join("u.bin").exists() && save.join("u.bin.odmpart").exists());
    m.delete(&[id], true);
    assert_eq!(std::fs::read(save.join("u.bin")).unwrap(), mine);
    assert!(!save.join("u.bin.odmpart").exists());
}

// 19 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_during_rate_limit_wait_removes_part() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/f/slow.bin?size=3000000", srv.base), true, 4);
    m.set_download_limit(&id, Some(20 * 1024));
    let d = wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let part = odm_core::util::part_path(&d.final_path());
    assert!(part.exists());
    m.delete(&[id], true);
    let t = Instant::now();
    while part.exists() && t.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!part.exists(), "part file left behind");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!part.exists(), "part file recreated");
}

// 21 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_direct_link_falls_back_to_original_url() {
    let first = common::start().await;
    let second = common::start().await;
    let front = common::start_redirector(&first.base).await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 2_000_000u64;
    let id = add(&m, format!("{}/gone/f.bin?size={size}&delay=10", front.base), true, 2);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    m.pause(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Paused, 5).await;
    assert!(d.final_url.as_deref().unwrap().starts_with(&first.base));
    first.gone.store(true, Ordering::SeqCst);
    *front.redirect_to.lock().unwrap() = Some(second.base.clone());
    m.resume(std::slice::from_ref(&id));
    let d = wait_for(&m, &id, &[Status::Complete, Status::Error], 60).await;
    assert_eq!(d.status, Status::Complete, "error: {:?}", d.error);
    assert!(d.final_url.as_deref().unwrap().starts_with(&second.base));
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn errors_do_not_contain_url_secrets() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr_with(dir.path(), |s| s.max_retries = 0);
    let id = add(&m, format!("{}/garbage/x.bin?token=SECRET", srv.base), true, 1);
    let d = wait_status(&m, &id, Status::Error, 10).await;
    let err = d.error.unwrap();
    assert!(!err.contains("SECRET"), "{err}");
}

// 23 -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn save_queue_never_drops_members() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let q = m.save_queue(Queue::new("", "Night"));
    let ids: Vec<String> = (0..3)
        .map(|i| {
            m.add(AddRequest {
                url: format!("{}/f/q{i}.bin?size=1000", srv.base),
                queue_id: Some(q.id.clone()),
                ..Default::default()
            })
            .unwrap()
        })
        .collect();
    let order = |m: &Manager| m.queues().into_iter().find(|x| x.id == q.id).unwrap().order;
    assert_eq!(order(&m), ids);
    let mut json = serde_json::to_value(m.queues().into_iter().find(|x| x.id == q.id).unwrap()).unwrap();

    // schedule editor save without an order: membership and order kept
    json["order"] = serde_json::Value::Null;
    json["maxConcurrent"] = 3.into();
    let saved = m.save_queue(serde_json::from_value(json.clone()).unwrap());
    assert_eq!(saved.order, ids);
    assert_eq!(saved.max_concurrent, 3);
    json.as_object_mut().unwrap().remove("order");
    let saved = m.save_queue(serde_json::from_value(json.clone()).unwrap());
    assert_eq!(saved.order, ids);

    // a partial order reorders; missing members are appended, unknown ids ignored
    json["order"] = serde_json::json!([ids[2], "no-such-id"]);
    let saved = m.save_queue(serde_json::from_value(json.clone()).unwrap());
    assert_eq!(saved.order, vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    assert_eq!(order(&m), saved.order);
    for id in &ids {
        assert_eq!(m.get(id).unwrap().queue_id.as_deref(), Some(q.id.as_str()));
    }
}

// merge review -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_exists_ignores_names_held_by_unfinished_downloads() {
    let tmp = tempfile::tempdir().unwrap();
    let m = mgr(tmp.path());
    let dir = tmp.path().join("dl").join("held");
    let dir_s = dir.to_string_lossy().into_owned();
    // a paused download reserves x.bin (its empty part file is on disk)
    m.add(AddRequest {
        url: "http://127.0.0.1:9/x.bin".into(),
        save_dir: Some(dir_s.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(dir.join("x.bin.odmpart").exists());
    // resume/overwrite wouldn't apply, so no conflict is offered
    assert!(!m.target_exists(&dir_s, "x.bin"));
    // a file the user owns is offered
    std::fs::write(dir.join("mine.bin"), b"abc").unwrap();
    assert!(m.target_exists(&dir_s, "mine.bin"));
    assert!(!m.target_exists(&dir_s, "nothing.bin"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remembered_folder_emits_settings_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let m = mgr(tmp.path());
    let mut rx = m.subscribe();
    let dir = tmp.path().join("elsewhere").to_string_lossy().into_owned();
    m.add(AddRequest {
        url: "http://127.0.0.1:9/song.mp3".into(),
        save_dir: Some(dir.clone()),
        remember_folder: true,
        ..Default::default()
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(Event::SettingsChanged(s))) => {
                let c = s.categories.iter().find(|c| c.id == "music").unwrap();
                assert_eq!(c.folder.as_deref(), Some(dir.as_str()));
                break;
            }
            Ok(Ok(_)) => continue,
            other => panic!("no SettingsChanged: {other:?}"),
        }
    }
}
