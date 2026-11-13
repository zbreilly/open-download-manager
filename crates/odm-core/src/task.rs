//! The download task: segmented, resumable, with IDM-style dynamic
//! splitting. When a connection finishes its segment it takes over the
//! back half of the segment with the most bytes remaining, so every
//! connection stays busy until the file is done.

use crate::client::{self, ContentRange, HeaderSet};
use crate::limiter::Limiter;
use crate::model::{SegState, SegmentView};
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use parking_lot::Mutex;
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
/// How often a writer makes its progress durable (`sync_data`). Only
/// synced progress is persisted while a download runs.
const SYNC_EVERY: Duration = Duration::from_secs(3);

struct Bounds {
    end: u64,
    /// next byte to hand out (reserved, may be ahead of `written`)
    pos: u64,
}

pub struct Seg {
    pub start: u64,
    bounds: Mutex<Bounds>,
    /// absolute offset: every byte below this has been written to the file
    pub written: AtomicU64,
    /// absolute offset: every byte below this has been synced to disk
    pub synced: AtomicU64,
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
            synced: AtomicU64::new(w),
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
        self.synced.fetch_min(to, Ordering::SeqCst);
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

    /// View limited to what is known to be on disk.
    pub fn synced_view(&self) -> SegmentView {
        let end = self.end();
        let synced = self.synced.load(Ordering::SeqCst).clamp(self.start, self.written().max(self.start));
        SegmentView {
            start: self.start,
            end,
            received: synced - self.start,
            state: if end != UNKNOWN && synced >= end { SegState::Done } else { SegState::Idle },
            speed_bps: 0,
        }
    }
}

/// Progress of a download made of many small pieces (the segments of an
/// HLS/DASH stream). Piece sizes are known only once fetched; until then
/// a piece counts with the average size of the finished ones.
pub struct Units {
    got: Vec<AtomicU64>,
    done: Vec<AtomicBool>,
    active: Vec<AtomicBool>,
    /// estimated size of a piece not fetched yet
    est: AtomicU64,
}

impl Units {
    /// `done[i]` = size of a piece already on disk.
    pub fn new(done: &[Option<u64>], est_total: Option<u64>) -> Self {
        let n = done.len().max(1) as u64;
        let u = Self {
            got: done.iter().map(|d| AtomicU64::new(d.unwrap_or(0))).collect(),
            done: done.iter().map(|d| AtomicBool::new(d.is_some())).collect(),
            active: done.iter().map(|_| AtomicBool::new(false)).collect(),
            est: AtomicU64::new(est_total.map(|t| t / n).unwrap_or(0)),
        };
        u.refresh_estimate();
        u
    }

    pub fn len(&self) -> usize {
        self.got.len()
    }

    pub fn is_empty(&self) -> bool {
        self.got.is_empty()
    }

    pub fn is_done(&self, i: usize) -> bool {
        self.done[i].load(Ordering::SeqCst)
    }

    pub fn start(&self, i: usize) {
        self.got[i].store(0, Ordering::SeqCst);
        self.active[i].store(true, Ordering::SeqCst);
    }

    pub fn add(&self, i: usize, n: u64) {
        self.got[i].fetch_add(n, Ordering::SeqCst);
    }

    /// The piece failed: forget its bytes.
    pub fn reset(&self, i: usize) {
        self.got[i].store(0, Ordering::SeqCst);
        self.active[i].store(false, Ordering::SeqCst);
    }

    pub fn finish(&self, i: usize, size: u64) {
        self.got[i].store(size, Ordering::SeqCst);
        self.done[i].store(true, Ordering::SeqCst);
        self.active[i].store(false, Ordering::SeqCst);
        self.refresh_estimate();
    }

    fn refresh_estimate(&self) {
        let (mut sum, mut n) = (0u64, 0u64);
        for (g, d) in self.got.iter().zip(&self.done) {
            if d.load(Ordering::SeqCst) {
                sum += g.load(Ordering::SeqCst);
                n += 1;
            }
        }
        if n >= 3 || (n > 0 && self.est.load(Ordering::SeqCst) == 0) {
            self.est.store(sum / n, Ordering::SeqCst);
        }
    }

    pub fn received(&self) -> u64 {
        self.got.iter().map(|g| g.load(Ordering::SeqCst)).sum()
    }

    pub fn done_bytes(&self) -> u64 {
        self.got.iter().zip(&self.done).filter(|(_, d)| d.load(Ordering::SeqCst)).map(|(g, _)| g.load(Ordering::SeqCst)).sum()
    }

    /// Estimated size of the whole download.
    pub fn total(&self) -> u64 {
        self.layout(false).1
    }

    /// The pieces as segment views (finished and waiting runs merged), and
    /// the total. `synced`: count only finished pieces (what is on disk).
    pub fn layout(&self, synced: bool) -> (Vec<SegmentView>, u64) {
        let est = self.est.load(Ordering::SeqCst);
        let mut out: Vec<SegmentView> = vec![];
        let mut at = 0u64;
        for i in 0..self.len() {
            let got = self.got[i].load(Ordering::SeqCst);
            let done = self.done[i].load(Ordering::SeqCst);
            let active = !synced && self.active[i].load(Ordering::SeqCst);
            let size = if done { got } else { est.max(got) };
            let (received, state) = if done {
                (size, SegState::Done)
            } else if active {
                (got.min(size), SegState::Receiving)
            } else {
                (0, SegState::Idle)
            };
            let merge = state != SegState::Receiving
                && out.last().is_some_and(|l| l.state == state && l.end == at);
            if merge {
                let l = out.last_mut().unwrap();
                l.end = at + size;
                l.received += received;
            } else if size > 0 || done {
                out.push(SegmentView { start: at, end: at + size, received, state, speed_bps: 0 });
            }
            at += size;
        }
        (out, at)
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
    /// generation of the download entry this run belongs to
    pub gen: u64,
    /// where the finished file was moved to (set during completion)
    pub placed: Mutex<Option<PathBuf>>,
    /// set for a stream download: progress is counted per piece
    pub units: std::sync::OnceLock<Arc<Units>>,
    done: CancellationToken,
}

impl Runtime {
    pub fn new(segs: Vec<SegmentView>, total: Option<u64>, resumable: bool, limit: u64) -> Arc<Self> {
        Self::with_gen(segs, total, resumable, limit, 0)
    }

    pub fn with_gen(segs: Vec<SegmentView>, total: Option<u64>, resumable: bool, limit: u64, gen: u64) -> Arc<Self> {
        Arc::new(Self {
            cancel: CancellationToken::new(),
            segs: Mutex::new(segs.iter().map(|v| Arc::new(Seg::from_view(v))).collect()),
            limiter: Limiter::new(limit),
            active: AtomicU32::new(0),
            phase: AtomicU8::new(0),
            total: AtomicU64::new(total.unwrap_or(UNKNOWN)),
            resumable: AtomicBool::new(resumable),
            started: Instant::now(),
            gen,
            placed: Mutex::new(None),
            units: std::sync::OnceLock::new(),
            done: CancellationToken::new(),
        })
    }

    pub fn received(&self) -> u64 {
        if let Some(u) = self.units.get() {
            return u.received();
        }
        self.segs.lock().iter().map(|s| s.received()).sum()
    }

    pub fn views(&self) -> Vec<SegmentView> {
        if let Some(u) = self.units.get() {
            return u.layout(false).0;
        }
        let mut v: Vec<SegmentView> = self.segs.lock().iter().map(|s| s.view()).collect();
        v.sort_by_key(|s| s.start);
        v
    }

    /// Segment views limited to synced data (what may be persisted while
    /// writers are still running).
    pub fn synced_views(&self) -> Vec<SegmentView> {
        if let Some(u) = self.units.get() {
            return u.layout(true).0;
        }
        let mut v: Vec<SegmentView> = self.segs.lock().iter().map(|s| s.synced_view()).collect();
        v.sort_by_key(|s| s.start);
        v
    }

    /// Signal that the run is over: no writer touches the files any more
    /// and the manager has recorded the outcome.
    pub fn mark_detached(&self) {
        self.done.cancel();
    }

    pub fn is_detached(&self) -> bool {
        self.done.is_cancelled()
    }

    /// Wait for [`mark_detached`](Self::mark_detached).
    pub async fn detached(&self) {
        self.done.cancelled().await
    }
}

pub struct TaskCtx {
    /// segment client (HTTP/1.1, redirects followed by `client::send`)
    pub client: reqwest::Client,
    /// set for an FTP download: segments are fetched from there instead
    pub ftp: Option<Arc<crate::ftp::Endpoint>>,
    pub url: String,
    pub headers: HeaderSet,
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
    /// Start over from zero. `resumable: true` = the file changed on the
    /// server (probe again); `false` = the server can't do byte ranges.
    Restart { resumable: bool, reason: String },
    /// the server answered with this (non-retryable) HTTP status
    Http(u16),
    Fatal(String),
}

pub struct TaskOutcome {
    pub total: u64,
}

enum SegErr {
    Cancelled,
    Restart { resumable: bool, reason: String },
    Http(u16),
    /// rate-limited / busy after all retries (HTTP status, or the FTP
    /// server's message)
    Throttled(u16, Option<String>),
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

    let fresh = {
        let mut segs = rt.segs.lock();
        // a stream-to-EOF segment from an earlier run learns the real size
        if total != UNKNOWN && segs.len() == 1 && segs[0].start == 0 && segs[0].end() == UNKNOWN && segs[0].written() <= total {
            segs[0].bounds.lock().end = total;
            if segs[0].written() >= total {
                segs[0].set_state(SegState::Done);
            }
        }
        let has_data = segs.iter().any(|s| s.received() > 0);
        let mut valid = resumable
            && !segs.is_empty()
            && (!multi || segs.iter().all(|s| s.end() != UNKNOWN && s.end() <= total));
        if valid && has_data {
            // the recorded progress must actually be in the part file
            let need = segs.iter().filter(|s| s.received() > 0).map(|s| s.written()).max().unwrap_or(0);
            let have = std::fs::metadata(&ctx.part).map(|m| m.len()).unwrap_or(0);
            if have < need {
                tracing::warn!(
                    "{}: part file holds {have} bytes but {need} were recorded; starting over",
                    ctx.part.display()
                );
                valid = false;
            }
        }
        if !valid {
            let n = if multi {
                let by_size = (total / ctx.min_segment.max(1)).max(1);
                (ctx.max_conn as u64).min(by_size).max(1)
            } else {
                1
            };
            segs.clear();
            if n == 1 {
                segs.push(Arc::new(Seg::new(0, total, 0)));
            } else {
                let size = total / n;
                for i in 0..n {
                    let s = i * size;
                    let e = if i == n - 1 { total } else { (i + 1) * size };
                    segs.push(Arc::new(Seg::new(s, e, s)));
                }
            }
        }
        !valid || !has_data
    };

    // create / size the part file
    {
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

    // internal stop (a failed segment stops the others) — a child of the
    // user's cancel token so the manager can tell the two apart
    let stop = rt.cancel.child_token();
    let mut set: JoinSet<(Arc<Seg>, Result<(), SegErr>)> = JoinSet::new();
    let mut max = if multi { ctx.max_conn.max(1) as usize } else { 1 };
    fill(&mut set, &ctx, &stop, max);

    let mut failure: Option<SegErr> = None;
    while let Some(joined) = set.join_next().await {
        rt.active.fetch_sub(1, Ordering::SeqCst);
        let (seg, res) = match joined {
            Ok(v) => v,
            Err(e) => {
                failure.get_or_insert(SegErr::Fatal(format!("internal error: {e}")));
                stop.cancel();
                continue;
            }
        };
        seg.assigned.store(false, Ordering::SeqCst);
        match res {
            Ok(()) => seg.set_state(SegState::Done),
            Err(SegErr::Throttled(code, _)) if failure.is_none() && !set.is_empty() => {
                // other connections are still working: hand this segment
                // back to the pool and use one connection fewer
                seg.set_state(SegState::Idle);
                max = max.saturating_sub(1).max(1);
                tracing::info!("{}: HTTP {code}; continuing with {max} connections", ctx.part.display());
                continue;
            }
            Err(e) => {
                seg.set_state(SegState::Idle);
                failure.get_or_insert(e);
                stop.cancel();
                continue;
            }
        }
        if failure.is_some() || stop.is_cancelled() {
            continue;
        }
        fill(&mut set, &ctx, &stop, max);
    }

    match failure {
        Some(SegErr::Cancelled) => return Err(TaskError::Cancelled),
        Some(SegErr::Restart { resumable, reason }) => return Err(TaskError::Restart { resumable, reason }),
        Some(SegErr::Throttled(_, Some(m))) => return Err(TaskError::Fatal(m)),
        Some(SegErr::Http(c)) | Some(SegErr::Throttled(c, None)) => return Err(TaskError::Http(c)),
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
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&ctx.part)
        .map_err(|e| TaskError::Fatal(format!("cannot open file: {e}")))?;
    f.set_len(final_total).map_err(|e| TaskError::Fatal(format!("write failed: {e}")))?;
    f.sync_all().map_err(|e| TaskError::Fatal(format!("write failed: {e}")))?;
    Ok(TaskOutcome { total: final_total })
}

fn spawn_seg(set: &mut JoinSet<(Arc<Seg>, Result<(), SegErr>)>, ctx: &Arc<TaskCtx>, stop: &CancellationToken, seg: Arc<Seg>) {
    seg.assigned.store(true, Ordering::SeqCst);
    ctx.rt.active.fetch_add(1, Ordering::SeqCst);
    let c = ctx.clone();
    let stop = stop.clone();
    set.spawn(async move {
        let r = run_segment(&c, &seg, &stop).await;
        (seg, r)
    });
}

/// Keep every connection busy: unassigned segments first, then split.
fn fill(set: &mut JoinSet<(Arc<Seg>, Result<(), SegErr>)>, ctx: &Arc<TaskCtx>, stop: &CancellationToken, max: usize) {
    while set.len() < max {
        let next = {
            let segs = ctx.rt.segs.lock();
            segs.iter().find(|s| !s.assigned.load(Ordering::SeqCst) && !s.is_done()).cloned()
        };
        if let Some(s) = next {
            spawn_seg(set, ctx, stop, s);
            continue;
        }
        match split_largest(&ctx.rt, ctx.min_segment) {
            Some(s) => spawn_seg(set, ctx, stop, s),
            None => break,
        }
    }
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

fn strip_weak(etag: &str) -> &str {
    let e = etag.trim();
    e.strip_prefix("W/").unwrap_or(e)
}

/// Validator for If-Range: a strong ETag, else Last-Modified.
fn if_range_value(ctx: &TaskCtx) -> Option<String> {
    if let Some(et) = ctx.etag.as_ref().map(|e| e.trim()).filter(|e| !e.is_empty() && !e.starts_with("W/")) {
        return Some(et.to_string());
    }
    ctx.last_modified.as_ref().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

async fn write_out(file: &mut tokio::fs::File, seg: &Seg, buf: &mut Vec<u8>) -> std::io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    file.write_all(buf).await?;
    // tokio hands writes to a blocking thread; flush waits for them so an
    // error surfaces here and progress counts only data the OS accepted
    file.flush().await?;
    seg.written.fetch_add(buf.len() as u64, Ordering::SeqCst);
    buf.clear();
    Ok(())
}

async fn sync(file: &mut tokio::fs::File, seg: &Seg) -> std::io::Result<()> {
    let w = seg.written();
    file.sync_data().await?;
    seg.synced.fetch_max(w, Ordering::SeqCst);
    Ok(())
}

async fn run_segment(ctx: &TaskCtx, seg: &Seg, stop: &CancellationToken) -> Result<(), SegErr> {
    let rt = &ctx.rt;
    // consecutive failures without new data; reset only by progress past
    // the furthest point reached so far (a non-resumable stream that keeps
    // dropping at the same spot must give up eventually)
    let mut attempts: u32 = 0;
    let mut reconnects: u32 = 0;
    let mut high_water = seg.written();
    loop {
        if stop.is_cancelled() {
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
        seg.set_state(if attempts == 0 && reconnects == 0 { SegState::Connecting } else { SegState::Reconnecting });

        let opened = tokio::select! {
            _ = stop.cancelled() => return Err(SegErr::Cancelled),
            r = open(ctx, seg, from, end) => r,
        };
        let mut stream = match opened {
            Ok(Opened::Body(b)) => b,
            Ok(Opened::Done) => return Ok(()),
            Err(OpenFail::Seg(e)) => return Err(e),
            Err(OpenFail::Retry(last)) => {
                attempts += 1;
                if attempts > ctx.max_retries {
                    return Err(last);
                }
                backoff(ctx, seg, attempts, stop).await?;
                continue;
            }
        };
        if rt.phase.load(Ordering::SeqCst) == 0 {
            rt.phase.store(1, Ordering::SeqCst);
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
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_BYTES + 64 * 1024);
        let mut last_flush = Instant::now();
        let mut last_sync = Instant::now();
        let mut stream_err = false;
        let mut cancelled = false;
        let mut io_err: Option<std::io::Error> = None;
        loop {
            let next = tokio::select! {
                _ = stop.cancelled() => { cancelled = true; break; }
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
            buf.extend_from_slice(&chunk[..take as usize]);
            let reached = seg.written() + buf.len() as u64;
            if reached > high_water {
                high_water = reached;
                attempts = 0;
            }
            // reached the (possibly shrunk) segment end
            let mut finished = (take as usize) < chunk.len();
            let e = seg.end();
            if e != UNKNOWN && reached >= e {
                finished = true;
            }
            if buf.len() >= FLUSH_BYTES || last_flush.elapsed() >= FLUSH_EVERY {
                if let Err(e) = write_out(&mut file, seg, &mut buf).await {
                    io_err = Some(e);
                    break;
                }
                last_flush = Instant::now();
                if last_sync.elapsed() >= SYNC_EVERY {
                    if let Err(e) = sync(&mut file, seg).await {
                        io_err = Some(e);
                        break;
                    }
                    last_sync = Instant::now();
                }
            }
            if take > 0
                && (!ctx.global.acquire_cancellable(take, stop).await || !rt.limiter.acquire_cancellable(take, stop).await)
            {
                cancelled = true;
                break;
            }
            if finished {
                break;
            }
        }
        drop(stream);
        // everything received is written and made durable before the run
        // reports its progress
        if io_err.is_none() {
            if let Err(e) = write_out(&mut file, seg, &mut buf).await {
                io_err = Some(e);
            }
        }
        if io_err.is_none() {
            if let Err(e) = sync(&mut file, seg).await {
                io_err = Some(e);
            }
        }
        drop(file);
        if let Some(e) = io_err {
            return Err(SegErr::Fatal(format!("write failed: {e}")));
        }
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
        reconnects += 1;
        let resumable = rt.resumable.load(Ordering::SeqCst);
        if attempts > ctx.max_retries || (!resumable && reconnects > ctx.max_retries.max(1) * 3) {
            return Err(SegErr::Fatal("connection kept dropping; retries exhausted".into()));
        }
        backoff(ctx, seg, attempts, stop).await?;
    }
}

/// A connection that delivers the bytes from `from`, or the news that
/// there is nothing left to fetch.
enum Opened {
    Body(BoxStream<'static, Result<Bytes, String>>),
    Done,
}

enum OpenFail {
    /// may work later; this is the error once retries are used up
    Retry(SegErr),
    Seg(SegErr),
}

async fn open(ctx: &TaskCtx, seg: &Seg, from: u64, end: u64) -> Result<Opened, OpenFail> {
    match &ctx.ftp {
        Some(ep) => open_ftp(ctx, ep, from).await,
        None => open_http(ctx, seg, from, end).await,
    }
}

async fn open_ftp(ctx: &TaskCtx, ep: &crate::ftp::Endpoint, from: u64) -> Result<Opened, OpenFail> {
    use crate::ftp::FtpError;
    let rt = &ctx.rt;
    let known_total = rt.total.load(Ordering::SeqCst);
    if known_total != UNKNOWN && from >= known_total {
        return Ok(Opened::Done);
    }
    let resumable = rt.resumable.load(Ordering::SeqCst);
    let expect = (known_total != UNKNOWN).then_some(known_total);
    match ep.open(if resumable { from } else { 0 }, expect).await {
        Ok(body) => Ok(Opened::Body(body)),
        Err(FtpError::Changed { expected, now }) => Err(OpenFail::Seg(SegErr::Restart {
            resumable: true,
            reason: format!("file changed on server (size {expected} -> {now}); restarting"),
        })),
        Err(FtpError::NoResume) => {
            Err(OpenFail::Seg(SegErr::Restart { resumable: false, reason: "the FTP server can't resume transfers".into() }))
        }
        Err(e) if e.throttled() => Err(OpenFail::Retry(SegErr::Throttled(421, Some(e.message())))),
        Err(e) if e.retryable() => Err(OpenFail::Retry(SegErr::Fatal(e.message()))),
        Err(e) => Err(OpenFail::Seg(SegErr::Fatal(e.message()))),
    }
}

async fn open_http(ctx: &TaskCtx, seg: &Seg, from: u64, end: u64) -> Result<Opened, OpenFail> {
    let rt = &ctx.rt;
    let resumable = rt.resumable.load(Ordering::SeqCst);
    let mut extra: Vec<(&'static str, String)> = vec![];
    let mut sent_range = false;
    let mut sent_if_range = false;
    if resumable && (from > 0 || end != UNKNOWN) {
        let range = if end == UNKNOWN { format!("bytes={from}-") } else { format!("bytes={from}-{}", end - 1) };
        extra.push(("range", range));
        sent_range = true;
        if let Some(v) = if_range_value(ctx) {
            extra.push(("if-range", v));
            sent_if_range = true;
        }
    }
    let restart = |resumable: bool, reason: String| Err(OpenFail::Seg(SegErr::Restart { resumable, reason }));
    let resp = match client::send(&ctx.client, &ctx.url, &ctx.headers, &extra, None).await {
        Ok(r) => r,
        // refused by policy or malformed: trying again cannot help
        Err(e @ (crate::CoreError::InsecureRedirect | crate::CoreError::BadUrl(_))) => {
            return Err(OpenFail::Seg(SegErr::Fatal(e.to_string())));
        }
        Err(e) => return Err(OpenFail::Retry(SegErr::Fatal(e.to_string()))),
    };
    let status = resp.status().as_u16();
    let header = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string());
    let known_total = rt.total.load(Ordering::SeqCst);
    match status {
        206 => {
            let cr = header("content-range").and_then(|v| client::parse_content_range(&v));
            let Some(ContentRange { range: Some((first, _)), total: got_total }) = cr else {
                return restart(false, "server sent an invalid byte range".into());
            };
            if first != from {
                return restart(false, "server returned the wrong byte range".into());
            }
            if let Some(t) = got_total {
                if known_total != UNKNOWN && t != known_total {
                    return restart(true, format!("file changed on server (size {known_total} -> {t}); restarting"));
                }
            }
            if let (Some(stored), Some(got)) = (ctx.etag.as_deref(), header("etag")) {
                if strip_weak(stored) != strip_weak(&got) {
                    return restart(true, "file changed on server; restarting".into());
                }
            }
        }
        200 => {
            // a full response is only usable by a segment that covers the whole file
            let whole = from == 0 && seg.start == 0 && (end == UNKNOWN || end == known_total);
            if sent_range && !whole {
                return restart(
                    sent_if_range,
                    if sent_if_range {
                        "file changed on server; restarting".into()
                    } else {
                        "server stopped honoring byte ranges".into()
                    },
                );
            }
            let len = header("content-length").and_then(|v| v.parse::<u64>().ok());
            if let (Some(len), true) = (len, known_total != UNKNOWN) {
                if len != known_total {
                    return restart(true, format!("file changed on server (size {known_total} -> {len}); restarting"));
                }
            }
        }
        416 => {
            if known_total != UNKNOWN && from >= known_total {
                return Ok(Opened::Done);
            }
            return restart(true, "requested range is no longer available; restarting".into());
        }
        408 | 425 | 429 | 500..=599 => {
            return Err(OpenFail::Retry(if matches!(status, 429 | 503) {
                SegErr::Throttled(status, None)
            } else {
                SegErr::Http(status)
            }));
        }
        _ => return Err(OpenFail::Seg(SegErr::Http(status))),
    }
    Ok(Opened::Body(resp.bytes_stream().map(|r| r.map_err(|e| e.to_string())).boxed()))
}

async fn backoff(ctx: &TaskCtx, seg: &Seg, attempt: u32, stop: &CancellationToken) -> Result<(), SegErr> {
    seg.set_state(SegState::Reconnecting);
    let base = ctx.retry_delay.as_millis() as u64;
    let ms = (base * (1u64 << (attempt.saturating_sub(1)).min(4))).min(60_000);
    tokio::select! {
        _ = stop.cancelled() => Err(SegErr::Cancelled),
        _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
    }
}
