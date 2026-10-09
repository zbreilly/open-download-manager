//! Open Download Manager — download engine.
//!
//! Minimal working single-stream downloader proving the rebuild
//! architecture end-to-end: URL parsing, HTTP/1.1 over plain or TLS
//! connections, redirects, chunked bodies, and resume via byte ranges.

pub mod body;
pub mod http;
pub mod multi;
pub mod jobs;
pub mod queue;
pub mod server;
pub mod verify;
pub mod url;

use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("bad url: {0}")]
    BadUrl(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("http status {0}")]
    Status(u16),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(String),
    #[error(transparent)]
    UrlParse(#[from] crate::url::UrlError),
    #[error("invalid dns name: {0}")]
    DnsName(String),
    #[error("segments incomplete — state kept for resume")]
    SegmentsIncomplete,
    #[error("final size mismatch")]
    SizeMismatch,
    #[error("connection lost mid-segment")]
    SegmentEof,
    #[error("checksum mismatch: expected {expected}, got {got}")]
    ChecksumMismatch { expected: String, got: String },
    #[error("cancelled (progress saved for resume)")]
    Cancelled,
}

/// Progress callback: (bytes downloaded so far, total if known).
pub type ProgressCb<'a> = dyn FnMut(u64, Option<u64>) + 'a;

#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub final_url: String,
    pub total_bytes: Option<u64>,
    pub resumable: bool,
    pub content_type: Option<String>,
    pub file_name: Option<String>,
    pub protocol: String,
}

/// Token-bucket bandwidth limiter (bytes/sec). `acquire` returns how long
/// the caller should wait before consuming `bytes`. Clone to share one
/// bucket across connections; limit 0 means unlimited.
#[derive(Clone)]
pub struct SpeedLimiter {
    inner: Arc<LimiterInner>,
}

struct LimiterInner {
    limit: std::sync::atomic::AtomicU64,
    state: Mutex<(Instant, f64)>, // last refill, available tokens
}

impl SpeedLimiter {
    pub fn new(limit_bps: u64) -> Self {
        Self {
            inner: Arc::new(LimiterInner {
                limit: std::sync::atomic::AtomicU64::new(limit_bps),
                state: Mutex::new((Instant::now(), limit_bps as f64)),
            }),
        }
    }
    pub fn set_limit(&self, bps: u64) {
        self.inner
            .limit
            .store(bps, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn limit(&self) -> u64 {
        self.inner.limit.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Wait until `bytes` may be consumed; returns the waited duration.
    /// Consumes in slices of <= 1s of tokens so a chunk larger than the
    /// bucket cap (1.5x rate) can never deadlock the acquire loop.
    pub fn acquire(&self, bytes: u64) -> Duration {
        let limit = self.limit();
        if limit == 0 {
            return Duration::ZERO;
        }
        let mut waited = Duration::ZERO;
        let mut remaining = bytes;
        while remaining > 0 {
            let take = remaining.min(limit.max(1)); // <= 1s worth of tokens
            loop {
                let mut st = self.inner.state.lock().unwrap();
                let now = Instant::now();
                let dt = now.duration_since(st.0).as_secs_f64();
                st.0 = now;
                st.1 = (st.1 + limit as f64 * dt).min(limit as f64 * 1.5);
                if st.1 >= take as f64 {
                    st.1 -= take as f64;
                    break;
                }
                let need = (take as f64 - st.1) / limit as f64;
                drop(st);
                let d = Duration::from_secs_f64(need.min(0.05));
                std::thread::sleep(d);
                waited += d;
            }
            remaining -= take;
        }
        waited
    }
}

/// Cooperative cancel/pause flag: set it to stop a download gracefully.
/// Progress is always checkpointed (`.part` / `.state`), so "pause" is a
/// cancel whose job can resume from disk.
pub type ControlFlag = Arc<AtomicBool>;

#[derive(Debug)]
pub struct DownloadStats {
    pub bytes_written: u64,
    pub resumed_from: u64,
    pub final_url: String,
    /// how many times the rebalancer split a slow segment's tail
    pub splits: u64,
}

struct Opts<'cb> {
    progress: Option<&'cb mut ProgressCb<'cb>>,
    limit: Option<u64>,
    control: Option<&'cb ControlFlag>,
    limiter: Option<&'cb SpeedLimiter>,
    headers: Option<&'cb [(String, String)]>,
}

/// Download `url` to `out`. Resumes from `<out>.part` when possible.
pub fn download(url: &str, out: &Path) -> Result<DownloadStats, EngineError> {
    download_with(url, out, None, None)
}

pub fn download_with<'a, 'cb>(
    url: &str,
    out: &'a Path,
    progress: Option<&'cb mut ProgressCb<'cb>>,
    limit: Option<u64>,
) -> Result<DownloadStats, EngineError> {
    download_with_ctl(url, out, progress, limit, None, None, None)
}

pub fn download_with_ctl<'a, 'cb>(
    url: &str,
    out: &'a Path,
    progress: Option<&'cb mut ProgressCb<'cb>>,
    limit: Option<u64>,
    control: Option<&'cb ControlFlag>,
    limiter: Option<&'cb SpeedLimiter>,
    headers: Option<&'cb [(String, String)]>,
) -> Result<DownloadStats, EngineError> {
    let mut opts = Opts { progress, limit, control, limiter, headers };
    let mut current = url.to_string();
    let part = part_path(out);

    for _hop in 0..6 {
        let target = url::Url::parse(&current)?;
        let mut conn = http::connect(&target)?;
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = http::Request::get(&target);
        if have > 0 {
            req = req.range_from(have);
        }
        if let Some(hs) = opts.headers {
            req = req.headers(hs.to_vec());
        }
        conn.write_request(&req)?;

        let head = conn.read_response_head()?;
        match head.status {
            301 | 302 | 303 | 307 | 308 => {
                let loc = head
                    .header("location")
                    .ok_or(EngineError::TooManyRedirects)?;
                current = resolve_redirect(&current, loc)?;
                continue;
            }
            200..=299 => {}
            s => return Err(EngineError::Status(s)),
        }

        let server_resumed = head.status == 206;
        let total = head.total_size(server_resumed);

        // figure out where we actually start writing
        let write_from = if server_resumed && have > 0 { have } else { 0 };
        let mut file = open_output(&part, write_from, server_resumed)?;

        let mut written = write_from;
        let mut reader = body::BodyReader::new(conn, head.body_kind());
        let mut buf = vec![0u8; 1024 * 1024];
        let mut last_tick = Instant::now();
        let limit_left = opts.limit;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if let Some(f) = opts.control {
            if f.load(std::sync::atomic::Ordering::SeqCst) {
                // checkpoint lives in .part — a later run resumes from it
                return Err(EngineError::Cancelled);
            }
        }
        let take = match limit_left {
                Some(l) if written - write_from + (n as u64) > l => {
                    (l - (written - write_from)) as usize
                }
                _ => n,
            };
            file.write_all(&buf[..take])?;
            written += take as u64;
            if let Some(l) = opts.limiter {
                let wait = l.acquire(take as u64);
                if wait > Duration::ZERO {
                    std::thread::sleep(wait);
                }
            }
            if let Some(cb) = opts.progress.as_mut() {
                // notify in ~500ms steps or on the final chunk
                if last_tick.elapsed() > Duration::from_millis(500) || take < n {
                    cb(written, total);
                    last_tick = Instant::now();
                }
            }
            if take < n {
                // limit hit — leave .part in place for resume
                return Ok(DownloadStats {
                    bytes_written: written,
                    resumed_from: write_from,
                    final_url: current,
                    splits: 0,
                });
            }
        }
        if let Some(cb) = opts.progress.as_mut() {
            cb(written, total);
        }
        file.sync_all().ok();
        drop(file);
        std::fs::rename(&part, out)?;
        return Ok(DownloadStats {
            bytes_written: written,
            resumed_from: write_from,
            final_url: current,
            splits: 0,
        });
    }
    Err(EngineError::TooManyRedirects)
}

pub(crate) fn part_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn open_output(
    part: &Path,
    write_from: u64,
    resumed: bool,
) -> Result<std::fs::File, EngineError> {
    use std::io::Seek;
    if resumed && write_from > 0 {
        // note: O_APPEND files can't seek on all platforms — open for
        // plain write and position at the end explicitly
        let mut f = std::fs::OpenOptions::new().write(true).open(part)?;
        f.seek(std::io::SeekFrom::End(0))?;
        Ok(f)
    } else {
        Ok(std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(part)?)
    }
}

pub(crate) fn resolve_redirect(base: &str, loc: &str) -> Result<String, EngineError> {
    if loc.contains("://") {
        return Ok(loc.to_string());
    }
    let u = url::Url::parse(base)?;
    if loc.starts_with('/') {
        return Ok(format!("{}://{}{}", u.scheme, u.host_port(), loc));
    }
    let base_path = match u.path.rfind('/') {
        Some(i) => &u.path[..=i],
        None => "/",
    };
    Ok(format!("{}://{}{}{}", u.scheme, u.host_port(), base_path, loc))
}
