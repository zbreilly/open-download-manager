//! HLS and DASH: manifest parsing (fixtures in `data/streams/`) and stream
//! downloads end to end against a local web server.
mod common;

use odm_core::*;
use std::time::{Duration, Instant};

const HLS: &str = "application/vnd.apple.mpegurl";
const DASH: &str = "application/dash+xml";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/streams").join(name)).unwrap()
}

// ---------------------------------------------------------------- parsing

#[test]
fn hls_master_playlist() {
    let hls::Playlist::Master(m) = hls::parse(&fixture("hls-master.m3u8"), "https://site.example/vod/master.m3u8").unwrap() else {
        panic!("not a master playlist")
    };
    let v: Vec<&hls::Variant> = m.variants.iter().filter(|v| !v.iframe_only).collect();
    assert_eq!(v.len(), 3);
    assert_eq!(v[0].uri, "https://site.example/vod/360p/index.m3u8");
    assert_eq!(v[0].resolution, Some((640, 360)));
    assert_eq!(v[0].average_bandwidth, Some(700_000));
    assert_eq!(v[0].codecs.as_deref(), Some("avc1.4d401e,mp4a.40.2"));
    assert_eq!(v[1].frame_rate, Some(29.97));
    assert_eq!(v[2].uri, "https://cdn.example/720p/index.m3u8?token=abc");
    assert_eq!(v[2].audio.as_deref(), Some("aac"));
    assert_eq!(m.variants.iter().filter(|v| v.iframe_only).count(), 1);
    let audio: Vec<&hls::Rendition> = m.renditions.iter().filter(|r| r.kind == "AUDIO").collect();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0].uri.as_deref(), Some("https://site.example/vod/audio/en.m3u8"));
    assert!(audio[0].default && !audio[1].default);
    assert_eq!(audio[1].name, "Español");
    assert_eq!(audio[1].language.as_deref(), Some("es"));
}

#[test]
fn hls_media_playlist_with_keys_ranges_and_discontinuity() {
    let hls::Playlist::Media(m) = hls::parse(&fixture("hls-media-ts.m3u8"), "https://s.example/a/index.m3u8").unwrap() else {
        panic!("not a media playlist")
    };
    assert!(m.ended && !m.is_live() && !m.drm());
    assert_eq!(m.media_sequence, 10);
    assert_eq!(m.segments.len(), 5);
    assert!((m.duration() - 25.5).abs() < 1e-9);
    let s = &m.segments;
    assert_eq!(s[0].uri, "https://s.example/a/seg10.ts");
    assert_eq!(s[0].seq, 10);
    let k0 = s[0].key.as_ref().unwrap();
    assert_eq!((k0.method.as_str(), k0.uri.as_deref(), k0.iv), ("AES-128", Some("https://s.example/a/keys/k1.bin"), None));
    assert_eq!(s[1].key, s[0].key);
    assert!(s[2].discontinuity && !s[1].discontinuity);
    let mut iv = [0u8; 16];
    iv[14] = 0xAB;
    iv[15] = 0xCD;
    assert_eq!(s[2].key.as_ref().unwrap().iv, Some(iv));
    assert_eq!(s[3].key, None);
    assert_eq!(s[3].range, Some((0, 1000)));
    // no offset: continues where the previous range of the same file ended
    assert_eq!(s[4].range, Some((1000, 2000)));
}

#[test]
fn hls_fmp4_drm_and_live() {
    let hls::Playlist::Media(m) = hls::parse(&fixture("hls-media-fmp4.m3u8"), "https://s.example/f/p.m3u8").unwrap() else { panic!() };
    assert!(m.segments.iter().all(|s| s.map.as_ref().map(|x| x.uri.as_str()) == Some("https://s.example/f/init.mp4")));
    let hls::Playlist::Media(m) = hls::parse(&fixture("hls-fairplay.m3u8"), "https://s.example/f/p.m3u8").unwrap() else { panic!() };
    assert!(m.drm());
    let hls::Playlist::Media(m) = hls::parse(&fixture("hls-live.m3u8"), "https://s.example/f/p.m3u8").unwrap() else { panic!() };
    assert!(m.is_live());
    assert_eq!(m.segments[1].seq, 4712);
    assert!(hls::parse("not a playlist", "https://s.example/").is_err());
}

#[test]
fn dash_numbered_template() {
    let mpd = dash::parse(&fixture("dash-number.mpd"), "https://s.example/show/manifest.mpd").unwrap();
    assert!(!mpd.dynamic);
    assert_eq!(mpd.duration, Some(10.5));
    let p = &mpd.periods[0];
    let kinds: Vec<dash::Kind> = p.sets.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [dash::Kind::Video, dash::Kind::Audio, dash::Kind::Audio, dash::Kind::Text]);
    let v = &p.sets[0].reps[0];
    assert_eq!((v.id.as_str(), v.bandwidth, v.width, v.height), ("v1080", 4_800_000, Some(1920), Some(1080)));
    assert_eq!(v.init.as_ref().unwrap().url, "https://s.example/show/media/v1080/init.mp4");
    let urls: Vec<&str> = v.segs.iter().map(|s| s.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://s.example/show/media/v1080/seg-001.m4s",
            "https://s.example/show/media/v1080/seg-002.m4s",
            "https://s.example/show/media/v1080/seg-003.m4s"
        ]
    );
    assert!((v.segs[2].duration - 2.5).abs() < 1e-9);
    let a = &p.sets[1];
    assert_eq!((a.lang.as_deref(), a.label.as_deref()), (Some("en"), Some("English")));
    assert_eq!(a.reps[0].channels.as_deref(), Some("2"));
    assert_eq!(a.reps[0].segs.len(), 3);
    assert!(!a.protected);
}

#[test]
fn dash_timeline_list_and_base() {
    let mpd = dash::parse(&fixture("dash-timeline.mpd"), "https://s.example/t/m.mpd").unwrap();
    let r = &mpd.periods[0].sets[0].reps[0];
    assert_eq!(mpd.periods[0].sets[0].kind, dash::Kind::Video);
    assert_eq!(r.width, Some(1280));
    // four 2 s segments, then 1 s segments until the end (12 s)
    assert_eq!(r.segs.len(), 8);
    assert_eq!(r.segs[0].url, "https://s.example/t/chunk-720-0.m4s");
    assert_eq!(r.segs[3].url, "https://s.example/t/chunk-720-540000.m4s");
    assert_eq!(r.segs[4].url, "https://s.example/t/chunk-720-720000.m4s");
    assert_eq!(r.segs[7].url, "https://s.example/t/chunk-720-990000.m4s");
    assert_eq!(r.init.as_ref().unwrap().url, "https://s.example/t/init-720.mp4");

    let mpd = dash::parse(&fixture("dash-list.mpd"), "https://s.example/l/m.mpd").unwrap();
    let r = &mpd.periods[0].sets[0].reps[0];
    assert_eq!(mpd.periods[0].sets[0].kind, dash::Kind::Audio);
    let init = r.init.as_ref().unwrap();
    assert_eq!((init.url.as_str(), init.range), ("https://cdn.example/audio/track.mp4", Some((0, 800))));
    assert_eq!(r.segs[0].range, Some((800, 1000)));
    assert_eq!(r.segs[1].range, Some((1800, 1000)));
    assert_eq!((r.segs[2].url.as_str(), r.segs[2].range), ("https://cdn.example/audio/extra.m4s", None));

    let mpd = dash::parse(&fixture("dash-base.mpd"), "https://elsewhere.example/m.mpd").unwrap();
    let v = &mpd.periods[0].sets[0].reps[0];
    assert_eq!(v.init, None);
    assert_eq!(v.segs.len(), 1);
    assert_eq!(v.segs[0].url, "https://media.example/vod/video-720.webm");
    assert_eq!(mpd.periods[0].sets[1].reps[0].segs[0].url, "https://media.example/vod/audio.webm");
}

#[test]
fn dash_drm_and_live() {
    let mpd = dash::parse(&fixture("dash-drm.mpd"), "https://s.example/d/m.mpd").unwrap();
    assert!(mpd.periods[0].sets[0].protected);
    let mpd = dash::parse(&fixture("dash-live.mpd"), "https://s.example/d/m.mpd").unwrap();
    assert!(mpd.dynamic);
    assert!(dash::parse("<html/>", "https://s.example/").is_err());
}

// ---------------------------------------------------------------- end to end

fn mgr(dir: &std::path::Path) -> Manager {
    let m = Manager::open(&dir.join("odm.db")).unwrap();
    let mut s = m.settings();
    s.default_folder = dir.join("dl").to_string_lossy().into_owned();
    s.retry_delay_secs = 0;
    s.max_retries = 5;
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

fn body(tag: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(tag)).collect()
}

/// AES-128-CBC with PKCS#7 padding.
fn encrypt(key: &[u8; 16], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockCipherEncrypt, KeyInit};
    let c = aes::Aes128::new(key.into());
    let pad = 16 - plain.len() % 16;
    let mut data = plain.to_vec();
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev = *iv;
    for chunk in data.as_chunks_mut::<16>().0 {
        for (b, p) in chunk.iter_mut().zip(prev.iter()) {
            *b ^= p;
        }
        let blk: &mut aes::Block = chunk.into();
        c.encrypt_block(blk);
        prev = *chunk;
    }
    data
}

fn add_stream(m: &Manager, url: String, choice: Option<StreamChoice>, name: Option<&str>, conns: u32) -> String {
    m.add(AddRequest {
        url,
        start: true,
        connections: Some(conns),
        stream: choice,
        file_name: name.map(String::from),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hls_quality_choice_aes128_and_byte_ranges() {
    let web = common::web::start().await;
    let key = *b"0123456789abcdef";
    web.put("/k.bin", key.to_vec(), "application/octet-stream");
    web.put(
        "/v/master.m3u8",
        "#EXTM3U\n\
         #EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720\nhi/index.m3u8\n\
         #EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360\nlo/index.m3u8\n",
        HLS,
    );
    // the high quality's playlist is read by the probe; its segments are never fetched
    web.put("/v/hi/index.m3u8", "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nh0.ts\n#EXT-X-ENDLIST\n", HLS);
    // low: two encrypted segments (IV from the sequence number), then two
    // byte ranges of one plain file
    let plain: Vec<Vec<u8>> = vec![body(1, 30_000), body(2, 20_001), body(3, 1000), body(4, 2500)];
    let iv = |seq: u128| seq.to_be_bytes();
    web.put("/v/lo/s5.ts", encrypt(&key, &iv(5), &plain[0]), "video/mp2t");
    web.put("/v/lo/s6.ts", encrypt(&key, &iv(6), &plain[1]), "video/mp2t");
    web.put("/v/lo/all.ts", [plain[2].clone(), plain[3].clone()].concat(), "video/mp2t");
    web.put(
        "/v/lo/index.m3u8",
        "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:5\n#EXT-X-KEY:METHOD=AES-128,URI=\"/k.bin\"\n\
         #EXTINF:4,\ns5.ts\n#EXTINF:4,\ns6.ts\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-DISCONTINUITY\n\
         #EXT-X-BYTERANGE:1000@0\n#EXTINF:2,\nall.ts\n#EXT-X-BYTERANGE:2500\n#EXTINF:2,\nall.ts\n#EXT-X-ENDLIST\n",
        HLS,
    );

    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let p = m.probe(&web.url("/v/master.m3u8"), None, None, vec![], None, None).await.unwrap();
    let info = p.stream.clone().expect("stream info");
    assert_eq!(info.kind, "hls");
    assert!(!info.protected && !info.live && info.blocked.is_none());
    let labels: Vec<&str> = info.variants.iter().map(|v| v.label.as_str()).collect();
    assert_eq!(labels, ["720p · 3.0 Mbps", "360p · 900 kbps"]);
    assert_eq!(p.file_name, "master.ts");
    assert_eq!(p.category, "video");
    assert!(p.protocol.starts_with("HLS"));
    // 3 Mbps for 4 s
    assert_eq!(p.total_bytes, Some(1_500_000));

    let choice = StreamChoice { kind: "hls".into(), variant: Some(info.variants[1].id.clone()), audio: None };
    let id = add_stream(&m, web.url("/v/master.m3u8"), Some(choice), Some(&p.file_name), 4);
    let d = wait_status(&m, &id, Status::Complete, 30).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), plain.concat());
    assert_eq!(d.protocol.as_deref(), Some("HLS"));
    assert_eq!(d.total_bytes, Some(plain.concat().len() as u64));
    assert_eq!(web.hits("/v/hi/h0.ts"), 0);
    assert_eq!(web.hits("/k.bin"), 1, "the key is fetched once");
    assert!(!stream::work_dir(&util::part_path(&d.final_path())).exists());
}

// ---- tiny fragmented MP4 files

fn mbox(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn fbox(kind: &[u8; 4], version: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(rest);
    mbox(kind, &b)
}

fn init_segment(id: u32, scale: u32) -> Vec<u8> {
    let mut mv = vec![0u8; 92];
    mv.extend_from_slice(&(id + 1).to_be_bytes());
    let mut tk = vec![0u8; 8];
    tk.extend_from_slice(&id.to_be_bytes());
    tk.extend_from_slice(&[0u8; 72]);
    let mut md = vec![0u8; 8];
    md.extend_from_slice(&scale.to_be_bytes());
    md.extend_from_slice(&[0u8; 8]);
    let trak = mbox(b"trak", &[fbox(b"tkhd", 0, 3, &tk), mbox(b"mdia", &fbox(b"mdhd", 0, 0, &md))].concat());
    let mut tx = id.to_be_bytes().to_vec();
    tx.extend_from_slice(&[0u8; 16]);
    let moov = mbox(b"moov", &[fbox(b"mvhd", 0, 0, &mv), trak, mbox(b"mvex", &fbox(b"trex", 0, 0, &tx))].concat());
    [mbox(b"ftyp", b"iso6\0\0\0\x01iso6cmfc"), moov].concat()
}

fn media_segment(id: u32, n: u32, time: u64, payload: &[u8]) -> Vec<u8> {
    let traf = mbox(b"traf", &[fbox(b"tfhd", 0, 0x020000, &id.to_be_bytes()), fbox(b"tfdt", 1, 0, &time.to_be_bytes())].concat());
    let moof = mbox(b"moof", &[fbox(b"mfhd", 0, 0, &n.to_be_bytes()), traf].concat());
    [mbox(b"styp", b"msdh\0\0\0\0"), moof, mbox(b"mdat", payload)].concat()
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dash_separate_audio_is_muxed_into_one_mp4() {
    let web = common::web::start().await;
    web.put(
        "/show/manifest.mpd",
        r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT6S">
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <SegmentTemplate timescale="1000" duration="2000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$.m4s"/>
      <Representation id="hd" bandwidth="2000000" width="1280" height="720" codecs="avc1.64001f"/>
      <Representation id="sd" bandwidth="500000" width="640" height="360" codecs="avc1.4d401e"/>
    </AdaptationSet>
    <AdaptationSet contentType="audio" mimeType="audio/mp4" lang="en">
      <SegmentTemplate timescale="48000" duration="96000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$.m4s"/>
      <Representation id="aud" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#,
        DASH,
    );
    web.put("/show/sd/init.mp4", init_segment(1, 1000), "video/mp4");
    web.put("/show/aud/init.mp4", init_segment(1, 48000), "audio/mp4");
    for n in 1..=3u32 {
        let t = (n - 1) as u64;
        web.put(&format!("/show/sd/{n}.m4s"), media_segment(1, n, t * 2000, &[0xA0 + n as u8; 333]), "video/mp4");
        web.put(&format!("/show/aud/{n}.m4s"), media_segment(1, n, t * 96000, &[0xB0 + n as u8; 111]), "audio/mp4");
    }
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let p = m.probe(&web.url("/show/manifest.mpd"), None, None, vec![], None, None).await.unwrap();
    let info = p.stream.unwrap();
    assert_eq!(info.kind, "dash");
    assert_eq!(info.variants.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["hd", "sd"]);
    assert!(info.variants.iter().all(|v| v.separate_audio && v.ext == "mp4"));
    assert_eq!(info.audio.len(), 1);
    assert_eq!(p.file_name, "manifest.mp4");

    let choice = StreamChoice { kind: "dash".into(), variant: Some("sd".into()), audio: Some("aud".into()) };
    let id = add_stream(&m, web.url("/show/manifest.mpd"), Some(choice), Some("Show.mp4"), 3);
    let d = wait_status(&m, &id, Status::Complete, 30).await;
    assert_eq!(d.file_name, "Show.mp4");
    assert!(d.notice.is_none() && d.extra_files.is_empty());
    let out = std::fs::read(d.final_path()).unwrap();
    assert_eq!(&out[4..8], b"ftyp");
    assert_eq!(count(&out, b"moov"), 1);
    assert_eq!(count(&out, b"trak"), 2);
    assert_eq!(count(&out, b"trex"), 2);
    assert_eq!(count(&out, b"moof"), 6);
    assert_eq!(count(&out, b"styp"), 0);
    for n in 1..=3u8 {
        assert_eq!(count(&out, &[0xA0 + n; 333]), 1);
        assert_eq!(count(&out, &[0xB0 + n; 111]), 1);
    }
    assert_eq!(web.hits("/show/hd/init.mp4"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_pause_and_resume_keep_finished_segments() {
    let web = common::web::start().await;
    let n = 16;
    let mut pl = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-PLAYLIST-TYPE:VOD\n");
    let mut expected = vec![];
    for i in 0..n {
        pl.push_str(&format!("#EXTINF:2,\np{i}.ts\n"));
        let b = body(i as u8, 40_000);
        web.put(&format!("/s/p{i}.ts"), b.clone(), "video/mp2t");
        expected.extend(b);
    }
    pl.push_str("#EXT-X-ENDLIST\n");
    web.put("/s/clip.m3u8", pl, HLS);
    web.delay_ms.store(120, std::sync::atomic::Ordering::SeqCst);

    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    // no stream choice: a playlist link is downloaded as its video
    let id = add_stream(&m, web.url("/s/clip.m3u8"), None, None, 2);
    assert!(m.get(&id).unwrap().stream.is_some());
    // finished segments on disk
    let segs = |work: &std::path::Path| {
        std::fs::read_dir(work).map(|r| r.filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "seg")).count()).unwrap_or(0)
    };
    let t = Instant::now();
    let work = loop {
        let d = m.get(&id).unwrap();
        let work = stream::work_dir(&util::part_path(&d.final_path()));
        if segs(&work) >= 4 {
            break work;
        }
        assert!(t.elapsed() < Duration::from_secs(30), "no progress");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    m.pause(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Paused, 10).await;
    // the name took the stream's real extension
    assert_eq!(d.file_name, "clip.ts");
    let kept = segs(&work);
    assert!((4..n).contains(&kept), "kept {kept}");
    m.resume(std::slice::from_ref(&id));
    let d = wait_status(&m, &id, Status::Complete, 60).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), expected);
    let fetched: u64 = (0..n).map(|i| web.hits(&format!("/s/p{i}.ts"))).sum();
    // only the segments in flight at the pause were fetched twice
    assert!(fetched <= n as u64 + 2, "fetched {fetched}");
    assert!(!work.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_and_live_streams_are_refused() {
    let web = common::web::start().await;
    web.put("/drm.mpd", fixture("dash-drm.mpd"), DASH);
    web.put("/fp.m3u8", fixture("hls-fairplay.m3u8"), HLS);
    web.put("/live.m3u8", fixture("hls-live.m3u8"), HLS);
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    for (path, protected) in [("/drm.mpd", true), ("/fp.m3u8", true), ("/live.m3u8", false)] {
        let p = m.probe(&web.url(path), None, None, vec![], None, None).await.unwrap();
        let info = p.stream.expect("stream info");
        assert_eq!(info.protected, protected, "{path}");
        assert_eq!(info.live, !protected, "{path}");
        let msg = info.blocked.unwrap();
        assert!(msg.contains(if protected { "DRM" } else { "live" }), "{msg}");
        let id = add_stream(&m, web.url(path), None, None, 2);
        let d = wait_status(&m, &id, Status::Error, 10).await;
        assert_eq!(d.error.as_deref(), Some(if protected { stream::PROTECTED } else { stream::LIVE }));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_that_cannot_be_muxed_is_saved_beside_the_video() {
    let web = common::web::start().await;
    web.put(
        "/w/m.mpd",
        r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5S">
  <Period>
    <AdaptationSet mimeType="video/webm" contentType="video">
      <Representation id="v" bandwidth="800000" width="854" height="480"><BaseURL>v.webm</BaseURL></Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/webm" contentType="audio">
      <Representation id="a" bandwidth="96000"><BaseURL>a.webm</BaseURL></Representation>
    </AdaptationSet>
  </Period>
</MPD>"#,
        DASH,
    );
    web.put("/w/v.webm", body(9, 70_000), "video/webm");
    web.put("/w/a.webm", body(8, 9_000), "audio/webm");
    let dir = tempfile::tempdir().unwrap();
    let m = mgr(dir.path());
    let id = add_stream(&m, web.url("/w/m.mpd"), Some(StreamChoice { kind: "dash".into(), ..Default::default() }), Some("talk.webm"), 2);
    let d = wait_status(&m, &id, Status::Complete, 30).await;
    assert_eq!(std::fs::read(d.final_path()).unwrap(), body(9, 70_000));
    assert_eq!(d.extra_files, ["talk (audio).webm"]);
    let audio = std::path::Path::new(&d.save_dir).join("talk (audio).webm");
    assert_eq!(std::fs::read(&audio).unwrap(), body(8, 9_000));
    assert!(d.notice.as_deref().unwrap().contains("talk (audio).webm"));
    m.delete(std::slice::from_ref(&id), true);
    let t = Instant::now();
    while (audio.exists() || d.final_path().exists()) && t.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!audio.exists() && !d.final_path().exists());
}
