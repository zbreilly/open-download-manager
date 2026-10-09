//! Multi-segment parallel downloading with speed-based rebalancing.
//!
//! Design (architecture documented in docs/module-map.md, rebuilt clean):
//! probe the resource for size + range support, split into N byte ranges,
//! fetch segments in parallel over separate connections. A coordinator
//! watches per-segment throughput; slow segments get their remaining tail
//! split off onto fresh connections (the original's "grow the fast lanes"
//! behavior, rebuilt). Checkpoints persist per-segment progress so an
//! interrupted run resumes.

use crate::http::{self};
use crate::url::Url;
use crate::{EngineError, ProgressCb};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MIN_MULTI_SIZE: u64 = 1024 * 1024;
const MIN_SEGMENT: u64 = 256 * 1024;
const MAX_PARTS: usize = 32;
const SEGMENT_RETRIES: usize = 6;
const REBALANCE_TICK: Duration = Duration::from_millis(400);
/// a segment is "slow" below 25% of the fastest active segment's speed
const SLOW_RATIO: f64 = 0.25;
/// only split when the slow segment still has meaningful work left
const MIN_TAIL: u64 = 2 * MIN_SEGMENT;

#[derive(Debug, Clone, Copy)]
struct Segment {
    start: u64,
    end: u64, // inclusive
    done: u64,
}

impl Segment {
    fn size(&self) -> u64 {
        self.end - self.start + 1
    }
    fn remaining(&self) -> u64 {
        self.size() - self.done.min(self.size())
    }
}

struct MultiState {
    segs: Vec<Segment>,
    splits: u64,
}

type SharedState = Arc<Mutex<MultiState>>;

struct Probe {
    url: String,
    total: Option<u64>,
    range_ok: bool,
    content_type: Option<String>,
    file_name: Option<String>,
}

/// Public URL probe for the Add dialog: size, resume support, protocol.
pub fn probe_url(url: &str) -> Result<crate::ProbeInfo, EngineError> {
    let p = probe(url)?;
    Ok(crate::ProbeInfo {
        final_url: p.url,
        total_bytes: p.total,
        resumable: p.range_ok,
        content_type: p.content_type,
        file_name: p.file_name,
        protocol: "HTTP/1.1".to_string(),
    })
}

fn probe(url: &str) -> Result<Probe, EngineError> {
    let mut current = url.to_string();
    for _ in 0..6 {
        let u = Url::parse(&current)?;
        let mut conn = http::connect(&u)?;
        conn.write_request(&http::Request::get(&u).range_from(0))?;
        let head = conn.read_response_head()?;
        match head.status {
            301 | 302 | 303 | 307 | 308 => {
                let loc = head
                    .header("location")
                    .ok_or(EngineError::TooManyRedirects)?;
                current = crate::resolve_redirect(&current, loc)?;
                continue;
            }
            200..=299 => {
                let ctype = head.header("content-type").map(|s| s.to_string());
                let disp = head.header("content-disposition");
                let file_name = disp.and_then(|d| {
                    d.split(';')
                        .find_map(|part| {
                            let p = part.trim();
                            p.strip_prefix("filename=")
                                .map(|f| f.trim_matches('"').to_string())
                        })
                });
                let mut reader = crate::body::BodyReader::new(conn, head.body_kind());
                let mut sink = [0u8; 4096];
                while reader.read(&mut sink)? > 0 {}
                return if head.status == 206 {
                    Ok(Probe {
                        url: current,
                        total: head.total_size(true),
                        range_ok: true,
                        content_type: ctype,
                        file_name,
                    })
                } else {
                    Ok(Probe {
                        url: current,
                        total: head.total_size(false),
                        range_ok: false,
                        content_type: ctype,
                        file_name,
                    })
                };
            }
            s => return Err(EngineError::Status(s)),
        }
    }
    Err(EngineError::TooManyRedirects)
}

fn state_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_os_string();
    s.push(".state");
    PathBuf::from(s)
}

fn save_state(part: &Path, st: &MultiState) {
    let mut text = String::new();
    for (i, s) in st.segs.iter().enumerate() {
        text.push_str(&format!("{} {} {} {}\n", i, s.start, s.end, s.done));
    }
    let tmp = state_path(part).with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, state_path(part));
    }
}

fn load_state(part: &Path, expected: &[Segment]) -> Option<Vec<Segment>> {
    let text = std::fs::read_to_string(state_path(part)).ok()?;
    let mut segs = expected.to_vec();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let idx: usize = it.next()?.parse().ok()?;
        let _start: u64 = it.next()?.parse().ok()?;
        let _end: u64 = it.next()?.parse().ok()?;
        let done: u64 = it.next()?.parse().ok()?;
        let s = segs.get_mut(idx)?;
        s.done = done.min(s.size());
    }
    Some(segs)
}

/// Multi-segment download with rebalancing. Falls back to the single-stream
/// engine when the server lacks range support or the size is unknown/small.
pub fn download_multi<'a, 'cb>(
    url: &str,
    out: &'a Path,
    parts: usize,
    progress: Option<&'cb mut ProgressCb<'cb>>,
    limit: Option<u64>,
) -> Result<crate::DownloadStats, EngineError> {
    download_multi_ctl(url, out, parts, progress, limit, None, None, None)
}

pub fn download_multi_ctl<'a, 'cb>(
    url: &str,
    out: &'a Path,
    parts: usize,
    mut progress: Option<&'cb mut ProgressCb<'cb>>,
    limit: Option<u64>,
    control: Option<&'cb crate::ControlFlag>,
    limiter: Option<&'cb crate::SpeedLimiter>,
    headers: Option<&'cb [(String, String)]>,
) -> Result<crate::DownloadStats, EngineError> {
    if limit.is_some() {
        return crate::download_with_ctl(url, out, progress, limit, control, limiter, headers);
    }
    let p = probe(url)?;
    let eligible =
        p.range_ok && p.total.is_some() && p.total.unwrap() >= MIN_MULTI_SIZE && parts != 1;
    if !eligible {
        return crate::download_with(&p.url, out, progress, None);
    }
    let total = p.total.unwrap();
    let n = parts.clamp(2, MAX_PARTS).min((total / MIN_SEGMENT).max(1) as usize);
    let n = n.max(1);
    if n < 2 {
        return crate::download_with(&p.url, out, progress, None);
    }

    let chunk = total / n as u64;
    let expected: Vec<Segment> = (0..n)
        .map(|i| {
            let i = i as u64;
            let start = i * chunk;
            let end = if i == n as u64 - 1 { total - 1 } else { (i + 1) * chunk - 1 };
            Segment { start, end, done: 0 }
        })
        .collect();

    let part: PathBuf = crate::part_path(out);
    let segs0 = match load_state(&part, &expected) {
        Some(s) => s,
        None => {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part)?;
            f.set_len(total)?;
            expected.clone()
        }
    };

    let state: SharedState =
        Arc::new(Mutex::new(MultiState { segs: segs0, splits: 0 }));
    let active = Arc::new(AtomicUsize::new(0));
    let control: Option<crate::ControlFlag> = control.cloned();
    // lane-speed record (f64 bits); workers publish, coordinator observes.
    // Survives fast lanes finishing before the first rebalance tick.
    let peak_lane = Arc::new(AtomicU64::new(0));
    let url = Arc::new(p.url.clone());
    let part = Arc::new(part);

    // worker for segment index `idx`; returns when its segment is complete,
    // exhausted by rebalancing, or retries are spent
    let worker = |idx: usize| {
        let url = Arc::clone(&url);
        let part = Arc::clone(&part);
        let state = Arc::clone(&state);
        let active = Arc::clone(&active);
        let peak_lane = Arc::clone(&peak_lane);
        let control = control.clone();
        let limiter = limiter.cloned();
        let headers = headers.map(|h| h.to_vec());
        active.fetch_add(1, Ordering::SeqCst);
        let handle = std::thread::spawn(move || {
            let mut attempt = 0usize;
            loop {
                let (start, end, done) = {
                    let st = state.lock().unwrap();
                    let s = &st.segs[idx];
                    (s.start, s.end, s.done)
                };
                if done >= seg_size(start, end) {
                    break; // complete or fully stolen
                }
                if let Some(f) = control.as_ref() {
                    if f.load(Ordering::SeqCst) {
                        active.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                }
                match fetch_segment(&url, &part, idx, &state, &peak_lane, control.as_ref(), limiter.as_ref(), headers.as_deref()) {
                    Ok(()) => break,
                    Err(EngineError::Cancelled) => {
                        active.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => {
                        // segment may have been shrunk by rebalancing mid-run;
                        // re-read and only retry if work remains
                        let remaining = {
                            let st = state.lock().unwrap();
                            st.segs[idx].remaining()
                        };
                        if remaining == 0 {
                            break;
                        }
                        if matches!(e, EngineError::Status(s) if (400..500).contains(&s)) {
                            break; // server refuses ranges; coordinator reports
                        }
                        attempt += 1;
                        if attempt >= SEGMENT_RETRIES {
                            eprintln!("segment {idx} failed after {attempt} attempts: {e}");
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(
                            500u64 * (1 << (attempt - 1).min(4)),
                        ));
                    }
                }
            }
            active.fetch_sub(1, Ordering::SeqCst);
        });
        handle
    };

    let mut handles: Vec<std::thread::JoinHandle<()>> = (0..n).map(worker).collect();

    // coordinator: progress reporting + speed-based rebalancing
    let mut prev: Vec<(u64, Instant)> = Vec::new();
    let mut peak_speed = 0.0f64;
    let mut rescue_budget: usize = 3 * n;
    let file_handle = std::fs::OpenOptions::new().write(true).open(part.as_path())?;
    drop(file_handle);
    loop {
        std::thread::sleep(REBALANCE_TICK);
        if let Some(f) = control.as_ref() {
            if f.load(Ordering::SeqCst) {
                return Err(EngineError::Cancelled);
            }
        }
        let (sum, snapshot, splits) = {
            let st = state.lock().unwrap();
            let sum = st.segs.iter().map(|s| s.done).sum();
            (sum, st.segs.clone(), st.splits)
        };
        if let Some(cb) = progress.as_mut() {
            cb(sum, Some(total));
        }
        save_state(part.as_path(), &MultiState { segs: snapshot.clone(), splits });

        if active.load(Ordering::SeqCst) == 0 {
            let all_done = snapshot.iter().all(|s| s.remaining() == 0);
            if all_done {
                break;
            }
            // workers are gone but work remains: respawn (with a budget so
            // permanently-failing segments end the run instead of looping)
            if rescue_budget > 0 {
                let idxs: Vec<usize> = snapshot
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.remaining() > 0)
                    .map(|(i, _)| i)
                    .collect();
                rescue_budget -= idxs.len().max(1);
                for idx in idxs {
                    handles.push(worker(idx));
                }
                prev.clear();
                continue;
            }
            break;
        }

        // ---- rebalancing pass ----
        let now = Instant::now();
        let speeds: Vec<f64> = snapshot
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (pd, pt) = prev.get(i).copied().unwrap_or((s.done, now));
                let dt = now.duration_since(pt).as_secs_f64().max(0.001);
                (s.done.saturating_sub(pd)) as f64 / dt
            })
            .collect();
        // compare against the observed peak (worker-published + tick-derived)
        // so a lone slow survivor is still recognized as slow once the
        // fast lanes drain
        let shared_peak = f64::from_bits(peak_lane.load(Ordering::SeqCst));
        peak_speed = peak_speed.max(shared_peak);
        peak_speed = peak_speed.max(speeds.iter().cloned().fold(0.0f64, f64::max));
        let live = active.load(Ordering::SeqCst);
        if std::env::var("ODM_DEBUG").is_ok() {
            eprintln!(
                "tick: peak={peak_speed:.0} live={live} speeds={:?} rem={:?}",
                speeds.iter().map(|s| s.round() as u64).collect::<Vec<_>>(),
                snapshot.iter().map(|s| s.remaining()).collect::<Vec<_>>()
            );
        }

        if peak_speed > 0.0 && live < MAX_PARTS {
            // slowest segment with enough remaining work
            let mut slowest: Option<(usize, f64)> = None;
            for (i, s) in snapshot.iter().enumerate() {
                let sp = speeds[i];
                if s.remaining() >= MIN_TAIL && sp < peak_speed * SLOW_RATIO {
                    if slowest.map(|(_, s0)| sp < s0).unwrap_or(true) {
                        slowest = Some((i, sp));
                    }
                }
            }
            if let Some((idx, _)) = slowest {
                let do_split = {
                    let mut st = state.lock().unwrap();
                    let rem = st.segs[idx].remaining();
                    if rem >= MIN_TAIL && live < MAX_PARTS {
                        let mid = st.segs[idx].start
                            + st.segs[idx].done
                            + rem / 2;
                        let tail = Segment { start: mid + 1, end: st.segs[idx].end, done: 0 };
                        st.segs[idx].end = mid;
                        st.segs.push(tail);
                        st.splits += 1;
                        true
                    } else {
                        false
                    }
                };
                if do_split {
                    let new_idx = {
                        let st = state.lock().unwrap();
                        st.segs.len() - 1
                    };
                    handles.push(worker(new_idx));
                    // prev entries must align with segs indices
                    prev.push((0, now));
                }
            }
        }
        prev = snapshot.iter().map(|s| (s.done, now)).collect();
    }
    for h in handles {
        let _ = h.join();
    }

    let (all_done, splits, written) = {
        let st = state.lock().unwrap();
        (
            st.segs.iter().all(|s| s.remaining() == 0),
            st.splits,
            st.segs.iter().map(|s| s.done).sum(),
        )
    };
    if !all_done {
        return Err(EngineError::SegmentsIncomplete);
    }
    let meta = std::fs::metadata(part.as_path())?;
    if meta.len() != total {
        return Err(EngineError::SizeMismatch);
    }
    drop(std::fs::remove_file(state_path(&part)));
    std::fs::rename(part.as_path(), out)?;
    Ok(crate::DownloadStats {
        bytes_written: written,
        resumed_from: 0,
        final_url: url.to_string(),
        splits,
    })
}

fn seg_size(start: u64, end: u64) -> u64 {
    end - start + 1
}

/// Download this segment's remaining range to `part`, honoring any end
/// shrinkage performed by the rebalancer (checked per chunk).
#[allow(clippy::too_many_arguments)]
fn fetch_segment(
    url: &str,
    part: &Path,
    idx: usize,
    state: &SharedState,
    peak_lane: &AtomicU64,
    control: Option<&crate::ControlFlag>,
    limiter: Option<&crate::SpeedLimiter>,
    headers: Option<&[(String, String)]>,
) -> Result<(), EngineError> {
    let u = Url::parse(url)?;
    let mut conn = http::connect(&u)?;
    let (start, _end, done) = {
        let st = state.lock().unwrap();
        let s = &st.segs[idx];
        (s.start, s.end, s.done)
    };
    let mut req = http::Request::get(&u).range_from(start + done);
    if let Some(hs) = headers {
        req = req.headers(hs.to_vec());
    }
    conn.write_request(&req)?;
    let head = conn.read_response_head()?;
    if head.status != 206 {
        return Err(EngineError::Status(head.status));
    }
    let mut file = std::fs::OpenOptions::new().write(true).open(part)?;
    file.seek(std::io::SeekFrom::Start(start + done))?;
    let mut reader = crate::body::BodyReader::new(conn, head.body_kind());
    // 1 MB reads: ~16x fewer syscalls and mutex acquisitions per byte
    let mut buf = vec![0u8; 1024 * 1024];
    let mut pos = start + done;
    let started = std::time::Instant::now();
    let mut published = false;
    loop {
        let cur_end = {
            let st = state.lock().unwrap();
            st.segs[idx].end
        };
        if pos > cur_end {
            // tail was stolen; clamp our done marker to the new end
            let mut st = state.lock().unwrap();
            st.segs[idx].done = st.segs[idx].size();
            return Ok(());
        }
        if let Some(f) = control {
            if f.load(Ordering::SeqCst) {
                return Err(EngineError::Cancelled);
            }
        }
        let want = ((cur_end - pos + 1) as usize).min(buf.len());
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            return Err(EngineError::SegmentEof);
        }
        if let Some(l) = limiter {
            let d = l.acquire(n as u64);
            if d > Duration::ZERO {
                std::thread::sleep(d);
            }
        }
        file.write_all(&buf[..n])?;
        pos += n as u64;
        {
            let mut st = state.lock().unwrap();
            st.segs[idx].done = pos - start;
        }
        // publish this lane's average speed so the rebalancer can compare
        // against it even after this lane finishes
        let dt = started.elapsed().as_secs_f64().max(0.05);
        if !published && dt > 0.25 {
            let sp = (pos - start) as f64 / dt;
            peak_lane.fetch_max(sp.to_bits(), Ordering::SeqCst);
            published = true;
        }
        if pos == cur_end + 1 {
            let mut st = state.lock().unwrap();
            st.segs[idx].done = st.segs[idx].size();
            drop(st);
            // fast lanes finish fast: publish their average speed so the
            // rebalancer still has a benchmark to compare slow tails against
            let sp = (pos - start) as f64
                / started.elapsed().as_secs_f64().max(0.001);
            peak_lane.fetch_max(sp.to_bits(), Ordering::SeqCst);
            return Ok(());
        }
    }
}
