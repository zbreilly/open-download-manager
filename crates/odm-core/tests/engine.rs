mod common;

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

fn add(m: &Manager, url: String, start: bool, conns: u32) -> String {
    m.add(AddRequest { url, start, connections: Some(conns), ..Default::default() })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_small_file() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/f/small.bin?size=5000", srv.base), true, 8);
    let d = wait_status(&m, &id, Status::Complete, 10).await;
    let data = std::fs::read(d.final_path()).unwrap();
    assert_eq!(data, common::expected(5000));
    assert_eq!(d.file_name, "small.bin");
    assert!(d.resumable);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_segment_with_dynamic_split() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 6 * 1024 * 1024 + 123;
    let id = add(&m, format!("{}/f/big.iso?size={size}&delay=2&slow0=10", srv.base), true, 8);
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    let data = std::fs::read(d.final_path()).unwrap();
    assert_eq!(data.len() as u64, size);
    assert!(data == common::expected(size), "content mismatch");
    // 8 initial segments; finishing connections split the remaining work
    assert!(d.segments.len() > 8, "expected dynamic splits, got {} segments", d.segments.len());
    assert!(!odm_core::util::part_path(&d.final_path()).exists());
    assert!(d.save_dir.ends_with("Programs"), "iso goes to the Programs subfolder: {}", d.save_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 3 * 1024 * 1024;
    let id = add(&m, format!("{}/f/p.bin?size={size}&delay=30", srv.base), true, 4);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    m.pause(std::slice::from_ref(&id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let d = m.get(&id).unwrap();
    assert_eq!(d.status, Status::Paused);
    assert!(d.received_bytes > 0 && d.received_bytes < size, "received {}", d.received_bytes);
    let before = d.received_bytes;
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
    assert!(before > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_without_ranges() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 700_000;
    let id = add(&m, format!("{}/noranges/x.zip?size={size}", srv.base), true, 8);
    let d = wait_status(&m, &id, Status::Complete, 20).await;
    assert!(!d.resumable);
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flaky_connections_retry() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 2 * 1024 * 1024;
    let id = add(&m, format!("{}/f/flaky.bin?size={size}&drop=200000", srv.base), true, 4);
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn not_found_fails() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/404/missing.zip", srv.base), true, 4);
    let d = wait_status(&m, &id, Status::Error, 10).await;
    assert!(d.error.unwrap().contains("404"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn content_disposition_name_and_category() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/cd/dl?size=1000", srv.base), true, 2);
    let d = wait_status(&m, &id, Status::Complete, 10).await;
    assert_eq!(d.file_name, "server-name.bin");
    let p = m.probe(&format!("{}/f/movie.mp4?size=10", srv.base), None, None, vec![], None, None).await.unwrap();
    assert_eq!(p.category, "video");
    assert!(p.resumable);
    assert_eq!(p.total_bytes, Some(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let size = 3 * 1024 * 1024;
    let id;
    {
        let m = mgr(dir.path());
        id = add(&m, format!("{}/f/r.bin?size={size}&delay=30", srv.base), true, 4);
        wait_status(&m, &id, Status::Downloading, 10).await;
        let t = std::time::Instant::now();
        while m.get(&id).unwrap().received_bytes == 0 && t.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        m.pause(std::slice::from_ref(&id));
        // what the apps do on quit: wait for writers to flush and sync
        m.shutdown_wait(Duration::from_secs(10)).await;
    }
    let m = mgr(dir.path());
    let d = m.get(&id).unwrap();
    assert_eq!(d.status, Status::Paused);
    assert!(d.received_bytes > 0);
    let hits_before = srv.hits.load(std::sync::atomic::Ordering::SeqCst);
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
    assert!(srv.hits.load(std::sync::atomic::Ordering::SeqCst) > hits_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_runs_in_order_and_finishes() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let mut rx = m.subscribe();
    let mut q = Queue::new("", "Overnight");
    q.max_concurrent = 1;
    let q = m.save_queue(q);
    let mut ids = vec![];
    for i in 0..3 {
        let id = m
            .add(AddRequest {
                url: format!("{}/f/q{i}.bin?size=200000", srv.base),
                queue_id: Some(q.id.clone()),
                start: false,
                ..Default::default()
            })
            .unwrap();
        ids.push(id);
    }
    assert_eq!(m.get(&ids[0]).unwrap().status, Status::Queued);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(m.get(&ids[0]).unwrap().status, Status::Queued, "queue not started yet");
    m.start_queue(&q.id);
    for id in &ids {
        wait_status(&m, id, Status::Complete, 20).await;
    }
    let t = Instant::now();
    let mut finished = false;
    while t.elapsed() < Duration::from_secs(5) {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(Event::QueueFinished(fq))) if fq.id == q.id => {
                finished = true;
                break;
            }
            _ => {}
        }
    }
    assert!(finished, "QueueFinished event");
    assert!(!m.queues().iter().find(|x| x.id == q.id).unwrap().running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_speed_limit() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    m.set_global_limit(Some(400 * 1024));
    let size = 1024 * 1024;
    let t = Instant::now();
    let id = add(&m, format!("{}/f/lim.bin?size={size}", srv.base), true, 4);
    wait_status(&m, &id, Status::Complete, 20).await;
    let secs = t.elapsed().as_secs_f64();
    assert!(secs > 1.8, "limit not applied: {secs:.2}s");
    assert!(secs < 6.0, "too slow: {secs:.2}s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checksum_verification() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let data = common::expected(4096);
    use sha2::Digest;
    let good = odm_core::util::to_hex(&sha2::Sha256::digest(&data));
    let ok = m
        .add(AddRequest {
            url: format!("{}/f/a.bin?size=4096", srv.base),
            start: true,
            checksum: Some(Checksum { algo: "sha256".into(), expected: good }),
            ..Default::default()
        })
        .unwrap();
    let bad = m
        .add(AddRequest {
            url: format!("{}/f/b.bin?size=4096", srv.base),
            start: true,
            checksum: Some(Checksum { algo: "".into(), expected: "0".repeat(64) }),
            ..Default::default()
        })
        .unwrap();
    let d = wait_status(&m, &ok, Status::Complete, 10).await;
    assert_eq!(d.checksum_ok, Some(true));
    let d = wait_status(&m, &bad, Status::Error, 10).await;
    assert_eq!(d.checksum_ok, Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_limit() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let mut s = m.settings();
    s.max_simultaneous = 2;
    m.save_settings(s).unwrap();
    let ids: Vec<String> = (0..4)
        .map(|i| add(&m, format!("{}/f/s{i}.bin?size=400000&delay=20", srv.base), true, 1))
        .collect();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let active = ids.iter().filter(|id| m.get(id).unwrap().status.is_active()).count();
    assert_eq!(active, 2);
    for id in &ids {
        wait_status(&m, id, Status::Complete, 30).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_starts_queue() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let mut q = m.queues().into_iter().find(|q| q.id == "main").unwrap();
    q.start_enabled = true;
    q.start_time = "03:30".into();
    m.save_queue(q);
    let id = m
        .add(AddRequest {
            url: format!("{}/f/sched.bin?size=1000", srv.base),
            queue_id: Some("main".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(m.get(&id).unwrap().scheduled_at.is_some());
    let t = chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap().and_hms_opt(3, 30, 0).unwrap();
    m.scheduler_tick(t);
    wait_status(&m, &id, Status::Complete, 10).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_changed_on_server_restarts() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let size = 2 * 1024 * 1024;
    let id = add(&m, format!("{}/f/c.bin?size={size}&delay=5&etag=1", srv.base), true, 2);
    wait_status(&m, &id, Status::Downloading, 10).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    m.pause(std::slice::from_ref(&id));
    tokio::time::sleep(Duration::from_millis(300)).await;
    // the stored etag no longer matches what the server reports for If-Range
    m.refresh_link(&id, &format!("{}/f/c.bin?size={size}&etag=2", srv.base)).await.unwrap();
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert!(std::fs::read(d.final_path()).unwrap() == common::expected(size));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grabber_lists_files() {
    let srv = common::start().await;
    let client = reqwest::Client::new();
    odm_core::client::install_crypto();
    let links = odm_core::grabber::grab(
        &client,
        &format!("{}/page", srv.base),
        Default::default(),
        odm_core::grabber::GrabOptions { depth: 0, max_pages: 5, same_host: true, extensions: vec![] },
        &default_categories(),
    )
    .await
    .unwrap();
    let names: Vec<_> = links.iter().map(|l| l.file_name.as_str()).collect();
    assert!(names.contains(&"a.zip"));
    assert!(names.contains(&"b.pdf"));
    assert!(names.contains(&"c.png"));
    assert!(!names.contains(&"sub.html"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_removes_part_files() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add(&m, format!("{}/f/del.bin?size=3000000&delay=10", srv.base), true, 2);
    let d = wait_status(&m, &id, Status::Downloading, 10).await;
    let part = odm_core::util::part_path(&d.final_path());
    assert!(part.exists());
    m.delete(std::slice::from_ref(&id), true);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(m.get(&id).is_none());
    assert!(!part.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn category_subfolders_and_assignment() {
    let srv = common::start().await;
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let root = dir.path().join("dl");
    assert_eq!(m.folder_for("general"), root.to_string_lossy());
    assert_eq!(m.folder_for("video"), root.join("Video").to_string_lossy());
    // assign a new type to Music, and move mp4 from Video to Music
    m.assign_extensions("music", &["XYZ".into(), ".mp4".into()]).unwrap();
    let s = m.settings();
    let music = s.categories.iter().find(|c| c.id == "music").unwrap();
    let video = s.categories.iter().find(|c| c.id == "video").unwrap();
    assert!(music.extensions.contains(&"xyz".to_string()));
    assert!(music.extensions.contains(&"mp4".to_string()));
    assert!(!video.extensions.contains(&"mp4".to_string()));
    let id = add(&m, format!("{}/f/song.xyz?size=1000", srv.base), true, 2);
    let d = wait_status(&m, &id, Status::Complete, 10).await;
    assert_eq!(d.category, "music");
    assert_eq!(std::path::Path::new(&d.save_dir), root.join("Music"));
    assert!(root.join("Music/song.xyz").exists());
    // name learned from the server re-routes to that category's folder
    let id = add(&m, format!("{}/cd/x?size=1000", srv.base), true, 2);
    let d = wait_status(&m, &id, Status::Complete, 10).await;
    assert_eq!(d.file_name, "server-name.bin");
    assert_eq!(d.category, "programs");
    assert!(root.join("Programs/server-name.bin").exists());
}
