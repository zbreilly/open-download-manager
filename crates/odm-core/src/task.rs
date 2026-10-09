//! The download task: segmented, resumable, with IDM-style dynamic
//! splitting. When a connection finishes its segment it takes over the
//! back half of the segment with the most bytes remaining, so every
//! connection stays busy until the file is done.

use crate::limiter::Limiter;
use crate::model::{SegState, SegmentView};
use futures_util::StreamExt;
use parking_lot::Mutex;
use reqwest::header::HeaderMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub const UNKNOWN: u64 = u64::MAX;
const FLUSH_BYTES: usize = 256 * 1024;
const FLUSH_EVERY: Duration = Duration::from_millis(250);

struct Bounds {
    end: u64,
    /// next byte to hand out (reserved, may be ahead of `written`)
    pos: u64,
}

pub struct Seg {
    pub start: u64,
    bounds: Mutex<Bounds>,
    /// absolute offset: every byte below this is on disk
    pub written: AtomicU64,
    state: AtomicU8,
    assigned: AtomicBool,
    pub speed: AtomicU64,
    pub last_sample: AtomicU64,
}

impl Seg {
    pub fn new(start: u64, end: u64, written: u64) -> Self {
        let w = written.clamp(start, end.max(start));
        Self {
            start,
            bounds: Mutex::new(Bounds { end, pos: w }),
            written: AtomicU64::new(w),
            state: AtomicU8::new(if w >= end { 4 } else { 0 }),
            assigned: AtomicBool::new(false),
            speed: AtomicU64::new(0),
            last_sample: AtomicU64::new(w),
        }
    }

    pub fn from_view(v: &SegmentView) -> Self {
        Self::new(v.start, v.end, v.start.saturating_add(v.received))
    }

    pub fn end(&self) -> u64 {
        self.bounds.lock().end
    }

    pub fn written(&self) -> u64 {
        self.written.load(Ordering::SeqCst)
    }

    pub fn received(&self) -> u64 {
        self.written() - self.start
    }

    pub fn is_done(&self) -> bool {
        let end = self.end();
        end != UNKNOWN && self.written() >= end
    }

    pub fn state(&self) -> SegState {
        match self.state.load(Ordering::SeqCst) {
            1 => SegState::Connecting,
            2 => SegState::Receiving,
            3 => SegState::Reconnecting,
            4 => SegState::Done,
            _ => SegState::Idle,
        }
    }

    fn set_state(&self, s: SegState) {
        let v = match s {
            SegState::Idle => 0,
            SegState::Connecting => 1,
            SegState::Receiving => 2,
            SegState::Reconnecting => 3,
            SegState::Done => 4,
        };
        self.state.store(v, Ordering::SeqCst);
    }

    /// Reserve up to `n` bytes at the current position. Returns the
    /// absolute offset and how many bytes may be written.
    fn reserve(&self, n: u64) -> (u64, u64) {
        let mut b = self.bounds.lock();
        let at = b.pos;
        let take = n.min(b.end.saturating_sub(b.pos));
        b.pos += take;
        (at, take)
    }

    fn reset_pos(&self, to: u64) {
        let mut b = self.bounds.lock();
        b.pos = to;
        self.written.store(to, Ordering::SeqCst);
    }

    fn remaining_unreserved(&self) -> u64 {
        let b = self.bounds.lock();
        if b.end == UNKNOWN {
            0
        } else {
            b.end.saturating_sub(b.pos)
        }
    }

    pub fn view(&self) -> SegmentView {
        SegmentView {
            start: self.start,
            end: self.end(),
            received: self.received(),
            state: self.state(),
            speed_bps: self.speed.load(Ordering::Relaxed),
        }
    }
}

/// Shared live state of a running download (read by the UI ticker).
pub struct Runtime {
    pub cancel: CancellationToken,
    pub segs: Mutex<Vec<Arc<Seg>>>,
    pub limiter: Limiter,
    pub active: AtomicU32,
    /// 0 connecting, 1 downloading, 2 assembling
    pub phase: AtomicU8,
    pub total: AtomicU64,
    pub resumable: AtomicBool,
    pub started: Instant,
}

impl Runtime {
    pub fn new(segs: Vec<SegmentView>, total: Option<u64>, resumable: bool, limit: u64) -> Arc<Self> {
        Arc::new(Self {
            cancel: CancellationToken::new(),
            segs: Mutex::new(segs.iter().map(|v| Arc::new(Seg::from_view(v))).collect()),
            limiter: Limiter::new(limit),
            active: AtomicU32::new(0),
            phase: AtomicU8::new(0),
            total: AtomicU64::new(total.unwrap_or(UNKNOWN)),
            resumable: AtomicBool::new(resumable),
            started: Instant::now(),
        })
    }

    pub fn received(&self) -> u64 {
        self.segs.lock().iter().map(|s| s.received()).sum()
    }

    pub fn views(&self) -> Vec<SegmentView> {
        let mut v: Vec<SegmentView> = self.segs.lock().iter().map(|s| s.view()).collect();
        v.sort_by_key(|s| s.start);
        v
    }
}

pub struct TaskCtx {
    pub client: reqwest::Client,
    pub url: String,
    pub headers: HeaderMap,
    pub part: PathBuf,
    pub max_conn: u32,
    pub min_segment: u64,
    pub global: Limiter,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub rt: Arc<Runtime>,
}

#[derive(Debug)]
pub enum TaskError {
    Cancelled,
    /// server content changed or ranges unsupported: restart from zero
    Restart { resumable: bool, reason: String },
    Fatal(String),
}

pub struct TaskOutcome {
    pub total: u64,
}

enum SegErr {
    Cancelled,
    Restart { resumable: bool, reason: String },
    Fatal(String),
}

pub async fn run(ctx: Arc<TaskCtx>) -> Result<TaskOutcome, TaskError> {
    let rt = ctx.rt.clone();
    if let Some(dir) = ctx.part.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| TaskError::Fatal(format!("cannot create folder: {e}")))?;
    }
    let total = rt.total.load(Ordering::SeqCst);
    let resumable = rt.resumable.load(Ordering::SeqCst);
    let multi = resumable && total != UNKNOWN && total > 0;

    {
        let mut segs = rt.segs.lock();
        let valid = !segs.is_empty()
            && (!multi || segs.iter().all(|s| s.end() != UNKNOWN && s.end() <= total));
        if !valid || !resumable {
            // fresh layout
            let n = if multi {
                let by_size = (total / ctx.min_segment.max(1)).max(1);
                (ctx.max_conn as u64).min(by_size).max(1)
            } else {
                1
            };
            segs.clear();
            if n == 1 {
                let end = if total == UNKNOWN { UNKNOWN } else { total };
                segs.push(Arc::new(Seg::new(0, end, 0)));
            } else {
                let size = total / n;
                for i in 0..n {
                    let s = i * size;
                    let e = if i == n - 1 { total } else { (i + 1) * size };
                    segs.push(Arc::new(Seg::new(s, e, s)));
                }
            }
        }
    }

    // create / size the part file
    {
        let fresh = !rt.segs.lock().iter().any(|s| s.received() > 0);
        let f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(fresh)
            .open(&ctx.part)
            .map_err(|e| TaskError::Fatal(format!("cannot write file: {e}")))?;
        if multi {
            let cur = f.metadata().map(|m| m.len()).unwrap_or(0);
            if cur < total {
                f.set_len(total)
                    .map_err(|e| TaskError::Fatal(format!("not enough disk space: {e}")))?;
            }
        }
    }

    let mut set: JoinSet<(Arc<Seg>, Result<(), SegErr>)> = JoinSet::new();
    let spawn = |set: &mut JoinSet<_>, seg: Arc<Seg>| {
        seg.assigned.store(true, Ordering::SeqCst);
        rt.active.fetch_add(1, Ordering::SeqCst);
        let c = ctx.clone();
        set.spawn(async move {
            let r = run_segment(&c, &seg).await;
            (seg, r)
        });
    };

    let max = if multi { ctx.max_conn.max(1) as usize } else { 1 };
    for seg in rt.segs.lock().clone() {
        if set.len() >= max {
            break;
        }
        if !seg.is_done() {
            spawn(&mut set, seg);
        }
    }

    let mut failure: Option<SegErr> = None;
    while let Some(joined) = set.join_next().await {
        rt.active.fetch_sub(1, Ordering::SeqCst);
        let (seg, res) = match joined {
            Ok(v) => v,
            Err(e) => {
                failure.get_or_insert(SegErr::Fatal(format!("internal error: {e}")));
                rt.cancel.cancel();
                continue;
            }
        };
        seg.assigned.store(false, Ordering::SeqCst);
        match res {
            Ok(()) => {
                seg.set_state(SegState::Done);
            }
            Err(e) => {
                seg.set_state(SegState::Idle);
                if failure.is_none() {
                    failure = Some(e);
                }
                rt.cancel.cancel();
                continue;
            }
        }
        if failure.is_some() || rt.cancel.is_cancelled() {
            continue;
        }
        // keep every connection busy: unassigned segments first, then split
        while set.len() < max {
            let next = {
                let segs = rt.segs.lock();
                segs.iter()
                    .find(|s| !s.assigned.load(Ordering::SeqCst) && !s.is_done())
                    .cloned()
            };
            if let Some(s) = next {
                spawn(&mut set, s);
                continue;
            }
            match split_largest(&rt, ctx.min_segment) {
                Some(s) => spawn(&mut set, s),
                None => break,
            }
        }
    }

    match failure {
        Some(SegErr::Cancelled) => return Err(TaskError::Cancelled),
        Some(SegErr::Restart { resumable, reason }) => return Err(TaskError::Restart { resumable, reason }),
        Some(SegErr::Fatal(m)) => return Err(TaskError::Fatal(m)),
        None => {}
    }
    if rt.cancel.is_cancelled() {
        return Err(TaskError::Cancelled);
    }

    let segs = rt.segs.lock().clone();
    let final_total = if total == UNKNOWN {
        segs.iter().map(|s| s.written()).max().unwrap_or(0)
    } else {
        if !segs.iter().all(|s| s.is_done()) {
            return Err(TaskError::Fatal("segments incomplete".into()));
        }
        total
    };
    rt.total.store(final_total, Ordering::SeqCst);
    // trim any preallocation beyond the real size
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&ctx.part) {
        let _ = f.set_len(final_total);
        let _ = f.sync_all();
    }
    Ok(TaskOutcome { total: final_total })
}

/// Split the back half of the segment with the most unreserved bytes into
/// a new segment. Returns it, or None if nothing is worth splitting.
fn split_largest(rt: &Runtime, min_segment: u64) -> Option<Arc<Seg>> {
    let mut segs = rt.segs.lock();
    let victim = segs
        .iter()
        .filter(|s| !s.is_done())
        .max_by_key(|s| s.remaining_unreserved())?
        .clone();
    let new = {
        let mut b = victim.bounds.lock();
        if b.end == UNKNOWN {
            return None;
        }
        let rem = b.end.saturating_sub(b.pos);
        if rem < min_segment.max(64 * 1024) * 2 {
            return None;
        }
        let mid = b.pos + rem / 2;
        let old_end = b.end;
        b.end = mid;
        Arc::new(Seg::new(mid, old_end, mid))
    };
    segs.push(new.clone());
    Some(new)
}

fn parse_content_range_start(v: &str) -> Option<u64> {
    // "bytes 100-199/1000"
    let rest = v.trim().strip_prefix("bytes")?.trim();
    rest.split('-').next()?.trim().parse().ok()
}

async fn run_segment(ctx: &TaskCtx, seg: &Seg) -> Result<(), SegErr> {
    let rt = &ctx.rt;
    let mut attempts: u32 = 0;
    loop {
        if rt.cancel.is_cancelled() {
            return Err(SegErr::Cancelled);
        }
        let resumable = rt.resumable.load(Ordering::SeqCst);
        let mut from = seg.written();
        if !resumable && from > 0 {
            // can't resume: start over
            seg.reset_pos(0);
            from = 0;
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&ctx.part) {
                let _ = f.set_len(0);
            }
        } else {
            seg.reset_pos(from);
        }
        let end = seg.end();
        if end != UNKNOWN && from >= end {
            return Ok(());
        }
        seg.set_state(if attempts == 0 { SegState::Connecting } else { SegState::Reconnecting });

        let mut req = ctx.client.get(&ctx.url).headers(ctx.headers.clone());
        let mut sent_if_range = false;
        if resumable && (from > 0 || end != UNKNOWN) {
            let range = if end == UNKNOWN {
                format!("bytes={from}-")
            } else {
                format!("bytes={from}-{}", end - 1)
            };
            req = req.header("range", range);
            if from > 0 {
                if let Some(et) = ctx.etag.as_ref().filter(|e| !e.starts_with("W/")) {
                    req = req.header("if-range", et.as_str());
                    sent_if_range = true;
                } else if let Some(lm) = ctx.last_modified.as_ref() {
                    req = req.header("if-range", lm.as_str());
                    sent_if_range = true;
                }
            }
        }

        let resp = tokio::select! {
            _ = rt.cancel.cancelled() => return Err(SegErr::Cancelled),
            r = req.send() => r,
        };
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                attempts += 1;
                if attempts > ctx.max_retries {
                    return Err(SegErr::Fatal(crate::CoreError::from_reqwest(e).to_string()));
                }
                backoff(ctx, seg, attempts).await?;
                continue;
            }
        };
        let status = resp.status().as_u16();
        match status {
            206 => {
                let start = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range_start);
                if start != Some(from) {
                    return Err(SegErr::Restart {
                        resumable: false,
                        reason: "server returned the wrong byte range".into(),
                    });
                }
                if rt.phase.load(Ordering::SeqCst) == 0 {
                    rt.phase.store(1, Ordering::SeqCst);
                }
            }
            200 => {
                if from > 0 || seg.start > 0 {
                    return Err(SegErr::Restart {
                        resumable: !sent_if_range,
                        reason: if sent_if_range {
                            "file changed on server; restarting".into()
                        } else {
                            "server stopped honoring byte ranges".into()
                        },
                    });
                }
                if resumable && end != UNKNOWN && rt.segs.lock().len() > 1 {
                    return Err(SegErr::Restart {
                        resumable: false,
                        reason: "server does not support byte ranges".into(),
                    });
                }
                if rt.phase.load(Ordering::SeqCst) == 0 {
                    rt.phase.store(1, Ordering::SeqCst);
                }
            }
            416 => {
                let total = rt.total.load(Ordering::SeqCst);
                if total != UNKNOWN && from >= total {
                    return Ok(());
                }
                return Err(SegErr::Restart { resumable: false, reason: "range not satisfiable".into() });
            }
            408 | 429 | 500..=599 => {
                attempts += 1;
                if attempts > ctx.max_retries {
                    return Err(SegErr::Fatal(crate::client::status_message(status)));
                }
                backoff(ctx, seg, attempts).await?;
                continue;
            }
            _ => return Err(SegErr::Fatal(crate::client::status_message(status))),
        }

        // ---- stream the body ----
        let mut file = match tokio::fs::OpenOptions::new().write(true).open(&ctx.part).await {
            Ok(f) => f,
            Err(e) => return Err(SegErr::Fatal(format!("cannot open file: {e}"))),
        };
        if let Err(e) = file.seek(std::io::SeekFrom::Start(from)).await {
            return Err(SegErr::Fatal(format!("seek failed: {e}")));
        }
        seg.set_state(SegState::Receiving);
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_BYTES + 64 * 1024);
        let mut last_flush = Instant::now();
        let mut finished = false;
        let mut stream_err = false;
        let mut cancelled = false;
        loop {
            let next = tokio::select! {
                _ = rt.cancel.cancelled() => { cancelled = true; break; }
                n = stream.next() => n,
            };
            let chunk = match next {
                None => break,
                Some(Err(_)) => {
                    stream_err = true;
                    break;
                }
                Some(Ok(c)) => c,
            };
            let (_, take) = seg.reserve(chunk.len() as u64);
            let take = take as usize;
            buf.extend_from_slice(&chunk[..take]);
            if take > 0 {
                attempts = 0;
            }
            if take < chunk.len() {
                finished = true; // reached (possibly shrunk) segment end
            }
            if buf.len() >= FLUSH_BYTES || last_flush.elapsed() >= FLUSH_EVERY || finished {
                if let Err(e) = file.write_all(&buf).await {
                    return Err(SegErr::Fatal(format!("write failed: {e}")));
                }
                seg.written.fetch_add(buf.len() as u64, Ordering::SeqCst);
                buf.clear();
                last_flush = Instant::now();
            }
            if take > 0 {
                ctx.global.acquire(take as u64).await;
                rt.limiter.acquire(take as u64).await;
            }
            if finished {
                break;
            }
            let e = seg.end();
            if e != UNKNOWN && seg.written() + buf.len() as u64 >= e {
                finished = true;
            }
        }
        if !buf.is_empty() {
            if let Err(e) = file.write_all(&buf).await {
                return Err(SegErr::Fatal(format!("write failed: {e}")));
            }
            seg.written.fetch_add(buf.len() as u64, Ordering::SeqCst);
        }
        let _ = file.flush().await;
        drop(file);
        if cancelled {
            return Err(SegErr::Cancelled);
        }
        let end = seg.end();
        if end == UNKNOWN {
            if !stream_err {
                // stream-to-EOF download complete
                let w = seg.written();
                seg.bounds.lock().end = w;
                return Ok(());
            }
        } else if seg.written() >= end {
            return Ok(());
        }
        // premature end: reconnect
        attempts += 1;
        if attempts > ctx.max_retries {
            return Err(SegErr::Fatal("connection kept dropping; retries exhausted".into()));
        }
        backoff(ctx, seg, attempts).await?;
    }
}

async fn backoff(ctx: &TaskCtx, seg: &Seg, attempt: u32) -> Result<(), SegErr> {
    seg.set_state(SegState::Reconnecting);
    let base = ctx.retry_delay.as_millis() as u64;
    let ms = (base * (1u64 << (attempt.saturating_sub(1)).min(4))).min(60_000);
    tokio::select! {
        _ = ctx.rt.cancel.cancelled() => Err(SegErr::Cancelled),
        _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
    }
}
