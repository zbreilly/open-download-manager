//! Concurrent download queue: N jobs, at most `max_parallel` running.

use crate::{ControlFlag, ProgressCb};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

#[derive(Debug, Clone)]
pub enum JobEvent {
    Started { id: u64, url: String },
    Progress { id: u64, done: u64, total: Option<u64> },
    Done { id: u64, bytes: u64 },
    Failed { id: u64, error: String },
    /// cancelled via control flag; `.part`/`.state` kept for resume
    Paused { id: u64 },
}

/// Per-job control flags, keyed by the caller's job id (e.g. a store id).
/// Set a job's flag to cooperatively cancel/pause it mid-download.
#[derive(Clone, Default)]
pub struct JobControls {
    flags: Arc<Mutex<HashMap<u64, ControlFlag>>>,
}

impl JobControls {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&self, id: u64) -> ControlFlag {
        let f: ControlFlag = Arc::new(AtomicBool::new(false));
        self.flags.lock().unwrap().insert(id, Arc::clone(&f));
        f
    }
    pub fn flag(&self, id: u64) -> Option<ControlFlag> {
        self.flags.lock().unwrap().get(&id).cloned()
    }
    pub fn pause(&self, id: u64) -> bool {
        match self.flag(id) {
            Some(f) => {
                f.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
    pub fn release(&self, id: u64) {
        self.flags.lock().unwrap().remove(&id);
    }
}

/// Run `jobs` (url, output) with at most `max_parallel` concurrent
/// downloads. `on_event` receives lifecycle events on the calling thread.
///
/// Every job sends exactly one terminal event (`Done` or `Failed`); one
/// bad job never stops the others.
pub fn run_jobs(
    jobs: Vec<(String, PathBuf)>,
    max_parallel: usize,
    on_event: &mut dyn FnMut(JobEvent),
) {
    run_jobs_ctl(jobs, max_parallel, None, None, on_event)
}

/// Like [`run_jobs`], with optional store-id mapping (events then carry the
/// caller's ids) and per-job control flags for pause support.
pub fn run_jobs_ctl(
    jobs: Vec<(String, PathBuf)>,
    max_parallel: usize,
    ids: Option<&[u64]>,
    controls: Option<&JobControls>,
    on_event: &mut dyn FnMut(JobEvent),
) {
    run_jobs_opts(jobs, max_parallel, ids, controls, None, None, on_event)
}

/// Per-run options: shared bandwidth limiter and per-job extra headers.
#[derive(Default, Clone)]
pub struct JobRunOptions {
    pub limiter: Option<crate::SpeedLimiter>,
    pub per_job_limiter: std::collections::HashMap<u64, crate::SpeedLimiter>,
    pub headers: std::collections::HashMap<u64, Vec<(String, String)>>,
    /// >= 2 runs the download multi-segment (parallel connections)
    pub parts: usize,
}

pub fn run_jobs_opts(
    jobs: Vec<(String, PathBuf)>,
    max_parallel: usize,
    ids: Option<&[u64]>,
    controls: Option<&JobControls>,
    opts: Option<&JobRunOptions>,
    _limiter: Option<&crate::SpeedLimiter>,
    on_event: &mut dyn FnMut(JobEvent),
) {
    let total = jobs.len();
    if total == 0 {
        return;
    }
    let max_parallel = max_parallel.max(1);
    let next = Arc::new(AtomicUsize::new(0));
    let jobs = Arc::new(Mutex::new(jobs));
    let (tx, rx) = mpsc::channel::<JobEvent>();

    let workers = max_parallel.min(total);
    let ids_owned: Option<Arc<Vec<u64>>> = ids.map(|v| Arc::new(v.to_vec()));
    let controls_owned: Option<JobControls> = controls.cloned();
    let opts_owned: Option<JobRunOptions> = opts.cloned();
    let mut handles = Vec::new();
    for _w in 0..workers {
        let next = Arc::clone(&next);
        let jobs = Arc::clone(&jobs);
        let tx = tx.clone();
        let ids_owned = ids_owned.clone();
        let controls_owned = controls_owned.clone();
        let opts_owned = opts_owned.clone();
        handles.push(std::thread::spawn(move || loop {
            // claim the next job index
            let idx = next.fetch_add(1, Ordering::SeqCst);
            if idx >= total {
                return;
            }
            let id = ids_owned
                .as_ref()
                .and_then(|v| v.get(idx).copied())
                .unwrap_or(idx as u64);
            let control = controls_owned.as_ref().and_then(|c| c.flag(id));
            if std::env::var("ODM_DEBUG").is_ok() {
                eprintln!("[worker] claimed idx={idx} id={id} has_flag={}", control.is_some());
            }
            let (url, out) = {
                let guard = jobs.lock().unwrap();
                guard[idx].clone()
            };
            let limiter: Option<crate::SpeedLimiter> = opts_owned
                .as_ref()
                .map(|o| {
                    o.per_job_limiter
                        .get(&id)
                        .cloned()
                        .or_else(|| o.limiter.clone())
                })
                .flatten();
            let headers: Option<Vec<(String, String)>> = opts_owned
                .as_ref()
                .and_then(|o| o.headers.get(&id).cloned());
            let _ = tx.send(JobEvent::Started { id, url: url.clone() });

            let job_tx = tx.clone();
            let mut last = std::time::Instant::now();
            let mut cb = move |done: u64, total: Option<u64>| {
                if last.elapsed() > std::time::Duration::from_millis(250) {
                    let _ = job_tx.send(JobEvent::Progress { id, done, total });
                    last = std::time::Instant::now();
                }
            };
            let cb_ref: &mut ProgressCb = &mut cb;
            let parts = opts_owned.as_ref().map(|o| o.parts).unwrap_or(1);
            let result = if parts >= 2 {
                crate::multi::download_multi_ctl(
                    &url, &out, parts, Some(cb_ref), None,
                    control.as_ref(), limiter.as_ref(), headers.as_deref(),
                )
            } else {
                crate::download_with_ctl(
                    &url, &out, Some(cb_ref), None,
                    control.as_ref(), limiter.as_ref(), headers.as_deref(),
                )
            };
            match result {
                Ok(stats) => {
                    let _ = tx.send(JobEvent::Done { id, bytes: stats.bytes_written });
                }
                Err(crate::EngineError::Cancelled) => {
                    let _ = tx.send(JobEvent::Paused { id });
                }
                Err(e) => {
                    let _ = tx.send(JobEvent::Failed { id, error: e.to_string() });
                }
            }
        }));
    }
    drop(tx); // main holds no sender: recv() ends when all workers finish

    let mut terminal_seen = 0usize;
    while terminal_seen < total {
        match rx.recv() {
            Ok(ev @ (JobEvent::Done { .. } | JobEvent::Failed { .. })) => {
                terminal_seen += 1;
                on_event(ev);
            }
            Ok(ev) => on_event(ev),
            Err(_) => break, // all workers exited
        }
    }
    for h in handles {
        let _ = h.join();
    }
}
