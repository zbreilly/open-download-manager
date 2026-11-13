//! Download manager: owns the list, starts/stops tasks, enforces
//! simultaneous-download and queue limits, runs the scheduler, measures
//! speed, and broadcasts events for the UI.
//!
//! Concurrency rules:
//! * Every run of a download has its own [`Runtime`], tagged with the
//!   entry's generation. Results of a run are only applied while the entry
//!   still points at that runtime *and* has the same generation, so an
//!   operation that invalidates a run (redownload) can't be undone by the
//!   old run finishing late.
//! * Files of a run (part file, placed final file) are only touched by
//!   other operations after the run has detached ([`Runtime::detached`]).
//! * Database writes go through [`Manager::persist`], which snapshots and
//!   writes under one lock, so an older snapshot can never overwrite a newer
//!   one and deletions always win.

use crate::client::{self, Clients, HeaderSet};
use crate::limiter::Limiter;
use crate::model::*;
use crate::pairing::{self, Pairing};
use crate::store::Store;
use crate::task::{self, Runtime, TaskCtx, TaskError, UNKNOWN};
use crate::util;
use crate::CoreError;
use chrono::{Datelike, Local, Timelike};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const MAIN_QUEUE: &str = "main";
const HISTORY: usize = 60;
/// automatic restarts (file changed on server, ranges dropped) before giving up
const MAX_RESTARTS: u32 = 2;

#[derive(Debug, Clone)]
pub enum Event {
    Added(Download),
    Changed(Download),
    Removed(String),
    Progress(Vec<ProgressItem>),
    Totals(Totals),
    Completed(Download),
    Failed(Download),
    QueueFinished(Queue),
    QueuesChanged(Vec<Queue>),
    SettingsChanged(Settings),
}

struct Entry {
    d: Download,
    rt: Option<Arc<Runtime>>,
    history: VecDeque<u64>,
    last_bytes: u64,
    ema: f64,
    /// bumped whenever results of the current run must no longer be applied
    gen: u64,
    /// a file operation (redownload reset) is pending; don't start
    busy: bool,
    /// already fell back from `final_url` to the original URL this session
    fell_back: bool,
}

impl Entry {
    fn new(d: Download) -> Self {
        let last_bytes = d.received_bytes;
        Self { d, rt: None, history: VecDeque::new(), last_bytes, ema: 0.0, gen: 0, busy: false, fell_back: false }
    }

    /// `rt` is this entry's runtime.
    fn owns(&self, rt: &Arc<Runtime>) -> bool {
        self.rt.as_ref().is_some_and(|r| Arc::ptr_eq(r, rt))
    }

    /// `rt` is this entry's runtime and its results still apply.
    fn current(&self, rt: &Arc<Runtime>) -> bool {
        self.owns(rt) && self.gen == rt.gen
    }
}

struct State {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    queues: Vec<Queue>,
    settings: Settings,
    recent: Vec<String>,
    fired: HashSet<String>,
    queue_completed: HashMap<String, u32>,
}

struct Inner {
    store: Store,
    state: Mutex<State>,
    /// serializes database writes of downloads (see module docs).
    /// Lock order: `persist` before `state`.
    persist: Mutex<()>,
    events: broadcast::Sender<Event>,
    global: Limiter,
    clients: RwLock<Clients>,
    /// key shared with the browser extension
    pairing: Arc<Pairing>,
    /// the pairing key was supplied through the environment
    automated: bool,
    handle: Mutex<Option<Handle>>,
    shutting_down: AtomicBool,
    background_started: AtomicBool,
    /// stops the ticker and the scheduler
    bg: CancellationToken,
    /// download runs and file operations
    tracker: TaskTracker,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

fn now_iso() -> String {
    Local::now().to_rfc3339()
}

/// Where the pairing key is kept in the database.
const PAIRING_KEY: &str = "bridge.key";
/// Environment variable that supplies the pairing key instead (64 hex
/// digits), for automated tests that drive the bridge without the settings
/// window. A key given this way is not stored.
pub const PAIRING_KEY_ENV: &str = "ODM_BRIDGE_KEY";

/// The stored pairing key; a new one is created (and stored) the first time.
/// The flag says the key came from the environment (an automated run).
fn load_pairing_key(store: &Store) -> ([u8; pairing::KEY_LEN], bool) {
    if let Some(k) = std::env::var(PAIRING_KEY_ENV).ok().and_then(|t| pairing::parse_key(&t)) {
        tracing::info!("using the pairing key from {PAIRING_KEY_ENV}");
        return (k, true);
    }
    if let Some(k) = store.get_secret(PAIRING_KEY).and_then(|t| pairing::parse_key(&t)) {
        return (k, false);
    }
    let k = pairing::generate_key();
    store.set_secret(PAIRING_KEY, &pairing::key_text(&k));
    (k, false)
}

/// Marks a runtime detached when its drive task ends (however it ends).
struct DetachGuard(Arc<Runtime>);

impl Drop for DetachGuard {
    fn drop(&mut self) {
        self.0.mark_detached();
    }
}

impl Manager {
    /// Open (or create) the database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self, CoreError> {
        let store = Store::open(db_path).map_err(|e| CoreError::Other(format!("database: {e}")))?;
        Self::with_store(store)
    }

    pub fn in_memory() -> Result<Self, CoreError> {
        let store = Store::in_memory().map_err(|e| CoreError::Other(e.to_string()))?;
        Self::with_store(store)
    }

    fn with_store(store: Store) -> Result<Self, CoreError> {
        let mut settings = store.load_settings();
        if settings.categories.is_empty() {
            settings.categories = default_categories();
        }
        // the toolbar toggle is a session switch unless "on at startup"
        settings.global_limit_enabled = settings.limit_on_startup;
        let mut queues = store.load_queues();
        if !queues.iter().any(|q| q.id == MAIN_QUEUE) {
            let mut q = Queue::new(MAIN_QUEUE, "Main queue");
            q.builtin = true;
            queues.insert(0, q);
        }
        for q in queues.iter_mut() {
            q.running = false;
        }
        queues.sort_by_key(|q| if q.builtin { 0 } else { 1 });

        let mut entries = HashMap::new();
        let mut order = Vec::new();
        for mut d in store.load_downloads() {
            if d.status == Status::Assembling {
                // completion was interrupted: finish it (re-verifies) on start
                d.status = Status::Queued;
                d.manual = true;
            } else {
                if d.status.is_active() {
                    d.status = Status::Paused;
                }
                if d.status == Status::Queued && d.manual {
                    d.status = Status::Paused;
                    d.manual = false;
                }
            }
            order.push(d.id.clone());
            entries.insert(d.id.clone(), Entry::new(d));
        }
        let recent: Vec<String> = store
            .get_kv("recent")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let clients = client::build_clients(&settings)?;
        let global = Limiter::new(if settings.global_limit_enabled { settings.global_limit_bps } else { 0 });
        let (tx, _) = broadcast::channel(4096);
        let (key, automated) = load_pairing_key(&store);
        let pairing = Arc::new(Pairing::new(key));
        Ok(Self {
            inner: Arc::new(Inner {
                pairing,
                automated,
                store,
                state: Mutex::new(State {
                    entries,
                    order,
                    queues,
                    settings,
                    recent,
                    fired: HashSet::new(),
                    queue_completed: HashMap::new(),
                }),
                persist: Mutex::new(()),
                events: tx,
                global,
                clients: RwLock::new(clients),
                handle: Mutex::new(Handle::try_current().ok()),
                shutting_down: AtomicBool::new(false),
                background_started: AtomicBool::new(false),
                bg: CancellationToken::new(),
                tracker: TaskTracker::new(),
            }),
        })
    }

    /// Spawn the progress ticker and the scheduler. Call inside a tokio
    /// runtime; that runtime also runs every download.
    pub fn start_background(&self) {
        if let Ok(h) = Handle::try_current() {
            *self.inner.handle.lock() = Some(h);
        }
        if self.inner.background_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(h) = self.handle() else {
            tracing::error!("start_background called outside a tokio runtime");
            return;
        };
        let m = self.clone();
        h.spawn(async move { m.ticker().await });
        let m = self.clone();
        h.spawn(async move { m.scheduler().await });
        // finish completions that were interrupted by the last exit
        self.pump();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    fn emit(&self, e: Event) {
        let _ = self.inner.events.send(e);
    }

    fn handle(&self) -> Option<Handle> {
        self.inner.handle.lock().clone().or_else(|| Handle::try_current().ok())
    }

    /// Run `f` on the engine's runtime, tracked for shutdown. Safe to call
    /// from any thread; returns false if no runtime is known.
    fn spawn<F: Future<Output = ()> + Send + 'static>(&self, f: F) -> bool {
        match self.handle() {
            Some(h) => {
                h.spawn(self.inner.tracker.track_future(f));
                true
            }
            None => {
                tracing::error!("no tokio runtime: call Manager::start_background inside one");
                false
            }
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.load(Ordering::SeqCst)
    }

    fn clients(&self) -> Clients {
        self.inner.clients.read().clone()
    }

    fn client(&self) -> reqwest::Client {
        self.inner.clients.read().main.clone()
    }

    // ------------------------------------------------------------ views

    fn live_view(e: &Entry, queues: &[Queue]) -> Download {
        let mut d = e.d.clone();
        if let Some(rt) = &e.rt {
            d.received_bytes = rt.received();
            d.segments = rt.views();
            d.active_connections = rt.active.load(Ordering::SeqCst);
            let t = rt.total.load(Ordering::SeqCst);
            if t != UNKNOWN {
                d.total_bytes = Some(t);
            }
            d.resumable = rt.resumable.load(Ordering::SeqCst);
        }
        d.speed_bps = if e.rt.is_some() { e.ema as u64 } else { 0 };
        d.eta_secs = eta(d.total_bytes, d.received_bytes, d.speed_bps);
        d.speed_history = e.history.iter().copied().collect();
        d.scheduled_at = None;
        if d.status == Status::Queued && !d.manual {
            if let Some(q) = queues.iter().find(|q| Some(&q.id) == d.queue_id.as_ref()) {
                if !q.running && q.start_enabled {
                    d.scheduled_at = next_run(q);
                }
            }
        }
        d
    }

    /// What may be stored: progress of a running download only as far as
    /// it has been synced to disk.
    fn db_view(e: &Entry) -> Download {
        let mut d = e.d.clone();
        if let Some(rt) = &e.rt {
            let segs = rt.synced_views();
            d.received_bytes = segs.iter().map(|s| s.received).sum();
            d.segments = segs;
            let t = rt.total.load(Ordering::SeqCst);
            if t != UNKNOWN {
                d.total_bytes = Some(t);
            }
            d.resumable = rt.resumable.load(Ordering::SeqCst);
        }
        d
    }

    pub fn list(&self) -> Vec<Download> {
        let st = self.inner.state.lock();
        st.order
            .iter()
            .filter_map(|id| st.entries.get(id))
            .map(|e| Self::live_view(e, &st.queues))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Download> {
        let st = self.inner.state.lock();
        st.entries.get(id).map(|e| Self::live_view(e, &st.queues))
    }

    /// Write the current state of `ids` to the database (deleting rows of
    /// entries that no longer exist). Never call with the state lock held.
    fn persist(&self, ids: &[String]) {
        let _g = self.inner.persist.lock();
        let items: Vec<(String, Option<Download>)> = {
            let st = self.inner.state.lock();
            ids.iter().map(|id| (id.clone(), st.entries.get(id).map(Self::db_view))).collect()
        };
        self.inner.store.write_downloads(&items);
    }

    fn persist_all(&self) {
        let _g = self.inner.persist.lock();
        let (items, queues): (Vec<(String, Option<Download>)>, Vec<Queue>) = {
            let st = self.inner.state.lock();
            let items = st
                .entries
                .values()
                .map(|e| {
                    let mut d = Self::db_view(e);
                    if e.rt.is_some() && d.status != Status::Assembling {
                        d.status = Status::Paused;
                    }
                    (d.id.clone(), Some(d))
                })
                .collect();
            (items, st.queues.clone())
        };
        self.inner.store.write_downloads(&items);
        self.inner.store.save_queues(&queues);
    }

    fn changed(&self, id: &str) {
        self.persist(&[id.to_string()]);
        if let Some(d) = self.get(id) {
            self.emit(Event::Changed(d));
        }
    }

    // ------------------------------------------------------------ settings

    pub fn settings(&self) -> Settings {
        self.inner.state.lock().settings.clone()
    }

    /// Check that `save_settings` would accept `s` (e.g. the proxy parses),
    /// without applying anything. Front ends call this before side effects
    /// such as moving the browser bridge to a new port.
    pub fn validate_settings(&self, s: &Settings) -> Result<(), CoreError> {
        client::build_clients(s).map(|_| ())
    }

    pub fn save_settings(&self, mut s: Settings) -> Result<(), CoreError> {
        if s.categories.is_empty() {
            s.categories = default_categories();
        }
        normalize_categories(&mut s.categories);
        s.default_connections = s.default_connections.clamp(1, 32);
        s.max_simultaneous = s.max_simultaneous.clamp(1, 64);
        let clients = client::build_clients(&s)?;
        *self.inner.clients.write() = clients;
        self.inner.global.set_rate(if s.global_limit_enabled { s.global_limit_bps } else { 0 });
        {
            let mut st = self.inner.state.lock();
            st.settings = s.clone();
            // per-download limit applies to running tasks that have no override
            for e in st.entries.values() {
                if let Some(rt) = &e.rt {
                    rt.limiter.set_rate(per_limit(&e.d, &s));
                }
            }
        }
        self.inner.store.save_settings(&s);
        self.emit(Event::SettingsChanged(s));
        self.pump();
        Ok(())
    }

    /// Toolbar toggle. `None` = off; `Some(bps)` = on at that rate.
    pub fn set_global_limit(&self, bps: Option<u64>) {
        let mut s = self.settings();
        match bps {
            Some(b) => {
                s.global_limit_enabled = true;
                if b > 0 {
                    s.global_limit_bps = b;
                }
            }
            None => s.global_limit_enabled = false,
        }
        let _ = self.save_settings(s);
    }

    pub fn set_download_limit(&self, id: &str, bps: Option<u64>) {
        let s = self.settings();
        {
            let mut st = self.inner.state.lock();
            if let Some(e) = st.entries.get_mut(id) {
                e.d.speed_limit_bps = bps;
                if let Some(rt) = &e.rt {
                    rt.limiter.set_rate(per_limit(&e.d, &s));
                }
            }
        }
        self.changed(id);
    }

    pub fn recent_folders(&self, limit: usize) -> Vec<String> {
        self.inner.state.lock().recent.iter().take(limit).cloned().collect()
    }

    fn push_recent(&self, dir: &str) {
        let list = {
            let mut st = self.inner.state.lock();
            st.recent.retain(|d| d != dir);
            st.recent.insert(0, dir.to_string());
            st.recent.truncate(12);
            st.recent.clone()
        };
        if let Ok(j) = serde_json::to_string(&list) {
            self.inner.store.set_kv("recent", &j);
        }
    }

    /// Assign file extensions to a category (removing them from any other).
    pub fn assign_extensions(&self, category: &str, exts: &[String]) -> Result<(), CoreError> {
        let mut s = self.settings();
        if !s.categories.iter().any(|c| c.id == category) {
            return Err(CoreError::Other("unknown category".into()));
        }
        let clean: Vec<String> = exts
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        for c in s.categories.iter_mut() {
            c.extensions.retain(|e| !clean.contains(e));
        }
        if category != "general" {
            if let Some(c) = s.categories.iter_mut().find(|c| c.id == category) {
                for e in clean {
                    if !c.extensions.contains(&e) {
                        c.extensions.push(e);
                    }
                }
            }
        }
        self.save_settings(s)
    }

    /// The key the browser extension must be paired with (see [`pairing`](crate::pairing)).
    pub fn pairing(&self) -> Arc<Pairing> {
        self.inner.pairing.clone()
    }

    /// Is this an automated run (started with the pairing key in
    /// `ODM_BRIDGE_KEY`)? Only then may a bridge request start a download
    /// without showing the Add dialog: the browser extension never asks for
    /// that, so in normal use a download always passes through the user.
    pub fn automated(&self) -> bool {
        self.inner.automated
    }

    /// The pairing key as text, to show in the settings.
    pub fn bridge_key(&self) -> String {
        self.inner.pairing.key_text()
    }

    /// Replace the pairing key. Every extension paired with the old key
    /// stops working until it is given the new one.
    pub fn regenerate_bridge_key(&self) -> String {
        let key = pairing::generate_key();
        self.inner.store.set_secret(PAIRING_KEY, &pairing::key_text(&key));
        self.inner.pairing.set_key(key);
        self.inner.pairing.key_text()
    }

    pub fn kv_get(&self, key: &str) -> Option<String> {
        self.inner.store.get_kv(key)
    }

    pub fn kv_set(&self, key: &str, value: &str) {
        self.inner.store.set_kv(key, value)
    }

    // ------------------------------------------------------------ probe / add

    pub async fn probe(
        &self,
        url: &str,
        referrer: Option<String>,
        cookies: Option<String>,
        headers: Vec<(String, String)>,
        user: Option<String>,
        pass: Option<String>,
    ) -> Result<ProbeResult, CoreError> {
        let s = self.settings();
        let h = client::request_headers(
            &s.user_agent,
            referrer.as_deref(),
            cookies.as_deref(),
            &headers,
            user.as_deref(),
            pass.as_deref(),
        );
        let hs = HeaderSet::new(url, h).allow_downgrade(s.allow_insecure_redirects);
        let c = self.clients().probe;
        tokio::select! {
            _ = self.inner.bg.cancelled() => Err(CoreError::Other("shutting down".into())),
            r = client::probe_with(&c, url, &hs, &s.categories) => r,
        }
    }

    /// Default save folder for a category.
    pub fn folder_for(&self, category: &str) -> String {
        category_folder(&self.settings(), category)
    }

    /// Is the name taken, on disk or by another download in the list?
    /// True when `dir`/`name` (or its part file) is on disk and the user
    /// can choose to resume or overwrite it. A name held by another
    /// unfinished download is not offered: `add` always gives the new
    /// download a free variant of it instead.
    pub fn target_exists(&self, dir: &str, name: &str) -> bool {
        let p = Path::new(dir).join(name);
        if !(p.exists() || util::part_path(&p).exists()) {
            return false;
        }
        let st = self.inner.state.lock();
        !claimed(&st, &p, None, true)
    }

    pub fn add(&self, req: AddRequest) -> Result<String, CoreError> {
        let url = req.url.trim().to_string();
        let parsed = url::Url::parse(&url).map_err(|_| CoreError::BadUrl(util::redact_url(&url)))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CoreError::BadUrl(format!("unsupported scheme: {}", parsed.scheme())));
        }
        let s = self.settings();
        let (mut name, auto_name) = match req.file_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => (util::sanitize_filename(n), false),
            None => (util::filename_from_url(&url).unwrap_or_else(|| "download".into()), true),
        };
        let category = req
            .category
            .clone()
            .filter(|c| s.categories.iter().any(|x| &x.id == c))
            .unwrap_or_else(|| util::category_for(&name, &s.categories));
        let auto_dir = req.save_dir.as_deref().map(str::trim).unwrap_or("").is_empty();
        let save_dir = req
            .save_dir
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| category_folder(&s, &category));

        let mut segments = vec![];
        let mut received = 0;
        let mut resumable = req.resumable.unwrap_or(true);
        let mut adopted: Option<String> = None;
        let policy = req.on_exists.clone().unwrap_or_else(|| "rename".into());
        let queue_id = req.queue_id.clone().filter(|q| !q.is_empty());
        let id = uuid::Uuid::new_v4().to_string();
        let (status, manual, queue_id) = if req.start {
            (Status::Queued, true, queue_id)
        } else if let Some(q) = queue_id {
            (Status::Queued, false, Some(q))
        } else {
            (Status::Paused, false, None)
        };

        let mut settings_to_save = None;
        let d = {
            let mut st = self.inner.state.lock();
            // the name is chosen and reserved under the state lock, so two
            // downloads can never share a file
            let dir = PathBuf::from(&save_dir);
            let target = dir.join(&name);
            let target_part = util::part_path(&target);
            let mut placed = false;
            if policy == "resume" && !claimed(&st, &target, None, true) && (target.exists() || target_part.exists()) {
                // continue an existing partial file from its current size
                if target_part.exists() {
                    adopted = Some("part".into());
                } else if std::fs::rename(&target, &target_part).is_ok() {
                    adopted = Some("file".into());
                }
                if adopted.is_some() {
                    if let Ok(m) = std::fs::metadata(&target_part) {
                        received = m.len();
                        segments.push(SegmentView {
                            start: 0,
                            end: req.total_bytes.unwrap_or(UNKNOWN),
                            received,
                            state: SegState::Idle,
                            speed_bps: 0,
                        });
                        resumable = true;
                    }
                    placed = true;
                }
            }
            if !placed {
                name = reserve_name(&st, &dir, &name, None, policy == "overwrite", true);
            }
            let d = Download {
                id: id.clone(),
                url: url.clone(),
                final_url: None,
                ext: util::badge_ext(&name),
                file_name: name,
                category: category.clone(),
                queue_id: queue_id.clone(),
                save_dir: save_dir.clone(),
                total_bytes: req.total_bytes,
                received_bytes: received,
                speed_bps: 0,
                eta_secs: None,
                status,
                scheduled_at: None,
                resumable,
                max_connections: req.connections.unwrap_or(s.default_connections).clamp(1, 32),
                active_connections: 0,
                segments,
                speed_history: vec![],
                error: None,
                added_at: now_iso(),
                completed_at: None,
                mime: req.mime.clone(),
                protocol: None,
                referrer: req.referrer.clone().filter(|r| !r.is_empty()),
                cookies: req.cookies.clone().filter(|r| !r.is_empty()),
                user_agent: req.user_agent.clone().filter(|r| !r.is_empty()),
                headers: req.headers.clone(),
                username: req.username.clone().filter(|r| !r.is_empty()),
                password: req.password.clone().filter(|r| !r.is_empty()),
                checksum: req.checksum.clone().filter(|c| !c.expected.trim().is_empty()),
                checksum_ok: None,
                speed_limit_bps: None,
                when_finished: "notify".into(),
                manual,
                etag: None,
                last_modified: None,
                retries_used: 0,
                on_exists: Some(policy),
                description: req.description.clone(),
                auto_name,
                auto_dir,
                restarts: 0,
                adopted,
            };
            if let Some(q) = &queue_id {
                if let Some(q) = st.queues.iter_mut().find(|x| &x.id == q) {
                    q.order.push(id.clone());
                }
            }
            st.order.push(id.clone());
            st.entries.insert(id.clone(), Entry::new(d.clone()));
            if req.remember_folder && req.save_dir.is_some() {
                if let Some(c) = st.settings.categories.iter_mut().find(|c| c.id == category) {
                    c.folder = Some(save_dir.clone());
                }
                settings_to_save = Some(st.settings.clone());
            }
            d
        };
        if let Some(s2) = settings_to_save {
            self.inner.store.save_settings(&s2);
            self.emit(Event::SettingsChanged(s2));
        }
        self.persist(std::slice::from_ref(&id));
        if let Some(q) = &queue_id {
            self.persist_queue(q);
        }
        self.push_recent(&save_dir);
        self.emit(Event::Added(self.get(&id).unwrap_or(d)));
        self.pump();
        Ok(id)
    }

    // ------------------------------------------------------------ control

    pub fn pause(&self, ids: &[String]) {
        for id in ids {
            let rt = {
                let mut st = self.inner.state.lock();
                let Some(e) = st.entries.get_mut(id) else { continue };
                if matches!(e.d.status, Status::Complete) {
                    continue;
                }
                e.d.status = Status::Paused;
                e.d.manual = false;
                e.rt.clone()
            };
            if let Some(rt) = rt {
                rt.cancel.cancel();
            }
            self.changed(id);
        }
        self.pump();
    }

    pub fn pause_all(&self) {
        let ids: Vec<String> = {
            let st = self.inner.state.lock();
            st.entries
                .values()
                .filter(|e| e.d.status.is_active() || e.d.status == Status::Queued)
                .map(|e| e.d.id.clone())
                .collect()
        };
        {
            let mut st = self.inner.state.lock();
            for q in st.queues.iter_mut() {
                q.running = false;
            }
        }
        self.pause(&ids);
        self.emit_queues();
    }

    pub fn resume(&self, ids: &[String]) {
        for id in ids {
            {
                let mut st = self.inner.state.lock();
                let Some(e) = st.entries.get_mut(id) else { continue };
                if matches!(e.d.status, Status::Complete) || e.d.status.is_active() {
                    continue;
                }
                e.d.status = Status::Queued;
                e.d.manual = true;
                e.d.error = None;
                e.d.restarts = 0;
                e.fell_back = false;
            }
            self.changed(id);
        }
        self.pump();
    }

    /// Start a download over from scratch. A running download is stopped
    /// first; its files are reset only once its writers are gone.
    pub fn redownload(&self, id: &str) {
        let (rt, gen) = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            e.gen += 1;
            e.busy = true;
            e.d.status = Status::Queued;
            e.d.manual = true;
            e.d.error = None;
            e.d.checksum_ok = None;
            e.d.completed_at = None;
            (e.rt.clone(), e.gen)
        };
        self.changed(id);
        match rt {
            Some(rt) => {
                rt.cancel.cancel();
                let m = self.clone();
                let id2 = id.to_string();
                let spawned = self.spawn(async move {
                    rt.detached().await;
                    m.finish_redownload(&id2, gen);
                });
                if !spawned {
                    self.finish_redownload(id, gen);
                }
            }
            None => self.finish_redownload(id, gen),
        }
    }

    fn finish_redownload(&self, id: &str, gen: u64) {
        {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            if e.gen != gen || e.rt.is_some() {
                return;
            }
            let fp = e.d.final_path();
            let part = util::part_path(&fp);
            if e.d.adopted.as_deref() == Some("file") && !fp.exists() {
                // give the user's original file back; the new download gets a fresh name
                let _ = std::fs::rename(&part, &fp);
            } else if let Err(err) = std::fs::remove_file(&part) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot remove {}: {err}", part.display());
                }
            }
            e.d.segments.clear();
            e.d.received_bytes = 0;
            e.d.total_bytes = None;
            e.d.final_url = None;
            e.d.etag = None;
            e.d.last_modified = None;
            e.d.completed_at = None;
            e.d.error = None;
            e.d.checksum_ok = None;
            e.d.restarts = 0;
            e.d.retries_used = 0;
            e.d.adopted = None;
            e.last_bytes = 0;
            e.fell_back = false;
            let dir = PathBuf::from(&e.d.save_dir);
            let name = e.d.file_name.clone();
            // keeps the name unless a finished copy (or anything else) sits there
            let new_name = reserve_name(&st, &dir, &name, Some(id), false, true);
            if let Some(e) = st.entries.get_mut(id) {
                e.d.ext = util::badge_ext(&new_name);
                e.d.file_name = new_name;
                e.busy = false;
            }
        }
        self.changed(id);
        self.pump();
    }

    pub fn delete(&self, ids: &[String], delete_files: bool) {
        for id in ids {
            let removed = {
                let mut st = self.inner.state.lock();
                st.order.retain(|x| x != id);
                for q in st.queues.iter_mut() {
                    q.order.retain(|x| x != id);
                }
                st.entries.remove(id)
            };
            let Some(e) = removed else { continue };
            self.persist(std::slice::from_ref(id));
            let job = Cleanup {
                final_path: e.d.final_path(),
                adopted: e.d.adopted.clone(),
                complete: e.d.status == Status::Complete,
                delete_files,
            };
            match e.rt.clone() {
                Some(rt) => {
                    rt.cancel.cancel();
                    // touch the files only after the writers are gone
                    let spawned = self.spawn({
                        let job = job.clone();
                        let rt = rt.clone();
                        async move {
                            rt.detached().await;
                            let placed = rt.placed.lock().clone();
                            job.run(placed).await;
                        }
                    });
                    if !spawned {
                        job.attempt(None);
                    }
                }
                None => {
                    if !job.attempt(None) {
                        let j2 = job.clone();
                        self.spawn(async move { j2.run(None).await });
                    }
                }
            }
            self.emit(Event::Removed(id.clone()));
        }
        self.save_all_queues();
        self.pump();
    }

    pub fn delete_completed(&self) -> Vec<String> {
        let ids: Vec<String> = {
            let st = self.inner.state.lock();
            st.entries
                .values()
                .filter(|e| e.d.status == Status::Complete)
                .map(|e| e.d.id.clone())
                .collect()
        };
        self.delete(&ids, false);
        ids
    }

    pub async fn refresh_link(&self, id: &str, new_url: &str) -> Result<(), CoreError> {
        let d = self.get(id).ok_or(CoreError::NotFound)?;
        // cookies, credentials and custom headers were given for the old
        // link's site: a link on another site never receives them
        let same_site = match (url::Url::parse(d.url.trim()), url::Url::parse(new_url.trim())) {
            (Ok(old), Ok(new)) => client::credentials_allowed(&old, &new),
            _ => false,
        };
        let p = if same_site {
            self.probe(new_url, d.referrer.clone(), d.cookies.clone(), d.headers.clone(), d.username.clone(), d.password.clone())
                .await?
        } else {
            self.probe(new_url, d.referrer.clone(), None, vec![], None, None).await?
        };
        if let (Some(a), Some(b)) = (d.total_bytes, p.total_bytes) {
            if a != b {
                return Err(CoreError::Other(format!(
                    "the new link points to a different file ({b} bytes, expected {a})"
                )));
            }
        }
        let rt = {
            let mut st = self.inner.state.lock();
            let e = st.entries.get_mut(id).ok_or(CoreError::NotFound)?;
            e.d.url = new_url.trim().to_string();
            if !same_site {
                e.d.cookies = None;
                e.d.headers.clear();
                e.d.username = None;
                e.d.password = None;
            }
            e.d.final_url = Some(p.final_url.clone());
            e.d.etag = p.etag.clone();
            e.d.last_modified = p.last_modified.clone();
            e.d.resumable = p.resumable;
            e.d.error = None;
            e.fell_back = false;
            let rt = e.rt.clone();
            if rt.is_some() {
                // restart with the new link once the current run has stopped
                e.d.status = Status::Queued;
                e.d.manual = true;
            } else if e.d.status == Status::Error {
                e.d.status = Status::Paused;
            }
            rt
        };
        if let Some(rt) = rt {
            rt.cancel.cancel();
        }
        self.changed(id);
        Ok(())
    }

    pub fn set_category(&self, ids: &[String], cat: &str) {
        let s = self.settings();
        for id in ids {
            {
                let mut st = self.inner.state.lock();
                let mv = match st.entries.get_mut(id) {
                    Some(e) => {
                        e.d.category = cat.to_string();
                        // a download that hasn't started yet moves to the new category's folder
                        (e.d.auto_dir
                            && e.rt.is_none()
                            && !e.busy
                            && e.d.received_bytes == 0
                            && e.d.status != Status::Complete)
                            .then(|| e.d.file_name.clone())
                    }
                    None => None,
                };
                if let Some(name) = mv {
                    relocate(&mut st, id, &category_folder(&s, cat), &name);
                }
            }
            self.changed(id);
        }
    }

    pub fn update_download(&self, id: &str, patch: DownloadPatch) {
        {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            if let Some(w) = patch.when_finished {
                e.d.when_finished = w;
            }
            if let Some(desc) = patch.description {
                e.d.description = Some(desc);
            }
            if let Some(c) = patch.max_connections {
                e.d.max_connections = c.clamp(1, 32);
            }
            if let Some(r) = patch.referrer {
                e.d.referrer = Some(r).filter(|s| !s.is_empty());
            }
            if let Some(c) = patch.cookies {
                e.d.cookies = Some(c).filter(|s| !s.is_empty());
            }
            let rename = patch.file_name.filter(|n| {
                e.rt.is_none() && !e.busy && e.d.status != Status::Complete && !n.trim().is_empty()
            });
            if let Some(n) = rename {
                let n = util::sanitize_filename(&n);
                let dir = e.d.save_dir.clone();
                if relocate(&mut st, id, &dir, &n) {
                    if let Some(e) = st.entries.get_mut(id) {
                        e.d.auto_name = false;
                    }
                }
            }
        }
        self.changed(id);
    }

    // ------------------------------------------------------------ queues

    pub fn queues(&self) -> Vec<Queue> {
        let mut qs = self.inner.state.lock().queues.clone();
        for q in qs.iter_mut() {
            q.next_run = next_run(q);
        }
        qs
    }

    fn emit_queues(&self) {
        let qs = self.queues();
        self.inner.store.save_queues(&qs);
        self.emit(Event::QueuesChanged(qs));
    }

    fn persist_queue(&self, id: &str) {
        let q = self.inner.state.lock().queues.iter().find(|q| q.id == id).cloned();
        if let Some(q) = q {
            self.inner.store.save_queue(&q);
        }
        self.emit(Event::QueuesChanged(self.queues()));
    }

    fn save_all_queues(&self) {
        self.emit_queues();
    }

    /// Create or update a queue. `order` may be omitted (or null) to keep
    /// the current order; a supplied order reorders the queue and may add
    /// downloads to it, but never drops members: downloads of this queue
    /// that the order leaves out are appended, unknown ids are ignored.
    pub fn save_queue(&self, mut q: Queue) -> Queue {
        if q.id.is_empty() {
            q.id = uuid::Uuid::new_v4().to_string();
        }
        q.max_concurrent = q.max_concurrent.clamp(1, 32);
        if q.days.len() != 7 {
            q.days = vec![true; 7];
        }
        let members = {
            let mut st = self.inner.state.lock();
            let supplied = std::mem::take(&mut q.order);
            let current = st.queues.iter().find(|x| x.id == q.id).map(|x| x.order.clone()).unwrap_or_default();
            let mut seen = HashSet::new();
            let mut order = vec![];
            let in_this_queue = |st: &State, id: &String| st.entries.get(id).is_some_and(|e| e.d.queue_id.as_deref() == Some(q.id.as_str()));
            for id in supplied {
                if st.entries.contains_key(&id) && seen.insert(id.clone()) {
                    order.push(id);
                }
            }
            for id in current {
                if in_this_queue(&st, &id) && seen.insert(id.clone()) {
                    order.push(id);
                }
            }
            for id in st.order.clone() {
                if in_this_queue(&st, &id) && seen.insert(id.clone()) {
                    order.push(id);
                }
            }
            q.order = order;
            // downloads named in the order join this queue (and leave others)
            for other in st.queues.iter_mut().filter(|x| x.id != q.id) {
                other.order.retain(|x| !seen.contains(x));
            }
            match st.queues.iter_mut().find(|x| x.id == q.id) {
                Some(existing) => {
                    q.builtin = existing.builtin;
                    q.running = existing.running;
                    *existing = q.clone();
                }
                None => st.queues.push(q.clone()),
            }
            for id in &q.order {
                if let Some(e) = st.entries.get_mut(id) {
                    e.d.queue_id = Some(q.id.clone());
                }
            }
            q.order.clone()
        };
        self.persist(&members);
        for id in &members {
            if let Some(d) = self.get(id) {
                self.emit(Event::Changed(d));
            }
        }
        self.emit_queues();
        self.pump();
        q
    }

    pub fn delete_queue(&self, id: &str) -> Result<(), CoreError> {
        let members = {
            let mut st = self.inner.state.lock();
            let q = st.queues.iter().find(|q| q.id == id).cloned().ok_or(CoreError::NotFound)?;
            if q.builtin {
                return Err(CoreError::Other("the main queue can't be deleted".into()));
            }
            st.queues.retain(|q| q.id != id);
            let mut members = vec![];
            for e in st.entries.values_mut() {
                if e.d.queue_id.as_deref() == Some(id) {
                    e.d.queue_id = None;
                    members.push(e.d.id.clone());
                }
            }
            members
        };
        self.inner.store.delete_queue(id);
        for m in members {
            self.changed(&m);
        }
        self.emit_queues();
        Ok(())
    }

    /// Move downloads into a queue (None = remove from all queues).
    pub fn move_to_queue(&self, ids: &[String], queue: Option<&str>) {
        {
            let mut st = self.inner.state.lock();
            for q in st.queues.iter_mut() {
                q.order.retain(|x| !ids.contains(x));
            }
            if let Some(qid) = queue {
                if let Some(q) = st.queues.iter_mut().find(|q| q.id == qid) {
                    q.order.extend(ids.iter().cloned());
                }
            }
            for id in ids {
                if let Some(e) = st.entries.get_mut(id) {
                    e.d.queue_id = queue.map(String::from);
                    if queue.is_some() && matches!(e.d.status, Status::Paused | Status::Error) {
                        e.d.status = Status::Queued;
                        e.d.manual = false;
                    }
                }
            }
        }
        for id in ids {
            self.changed(id);
        }
        self.emit_queues();
        self.pump();
    }

    pub fn reorder_queue(&self, queue: &str, order: Vec<String>) {
        {
            let mut st = self.inner.state.lock();
            if let Some(q) = st.queues.iter_mut().find(|q| q.id == queue) {
                let set: HashSet<&String> = q.order.iter().collect();
                let mut new: Vec<String> = order.into_iter().filter(|i| set.contains(i)).collect();
                for id in &q.order {
                    if !new.contains(id) {
                        new.push(id.clone());
                    }
                }
                q.order = new;
            }
        }
        self.persist_queue(queue);
    }

    pub fn start_queue(&self, id: &str) {
        if self.is_shutting_down() {
            return;
        }
        let members: Vec<String> = {
            let mut st = self.inner.state.lock();
            let Some(q) = st.queues.iter_mut().find(|q| q.id == id) else { return };
            q.running = true;
            let order = q.order.clone();
            st.queue_completed.insert(id.to_string(), 0);
            let mut changed = vec![];
            for did in order {
                if let Some(e) = st.entries.get_mut(&did) {
                    if matches!(e.d.status, Status::Paused | Status::Error) && e.rt.is_none() {
                        e.d.status = Status::Queued;
                        e.d.manual = false;
                        e.d.retries_used = 0;
                        e.d.restarts = 0;
                        changed.push(did.clone());
                    }
                }
            }
            changed
        };
        for m in members {
            self.changed(&m);
        }
        self.emit_queues();
        self.pump();
    }

    pub fn stop_queue(&self, id: &str) {
        let to_stop: Vec<(String, Option<Arc<Runtime>>)> = {
            let mut st = self.inner.state.lock();
            let Some(q) = st.queues.iter_mut().find(|q| q.id == id) else { return };
            q.running = false;
            let order = q.order.clone();
            let mut v = vec![];
            for did in order {
                if let Some(e) = st.entries.get_mut(&did) {
                    if !e.d.manual && (e.d.status.is_active() || e.d.status == Status::Queued) {
                        e.d.status = Status::Queued;
                        v.push((did.clone(), e.rt.clone()));
                    }
                }
            }
            v
        };
        for (did, rt) in to_stop {
            if let Some(rt) = rt {
                rt.cancel.cancel();
            }
            self.changed(&did);
        }
        self.emit_queues();
        self.pump();
    }

    // ------------------------------------------------------------ scheduling

    /// Start whatever may run now (simultaneous limit + running queues).
    pub fn pump(&self) {
        if self.is_shutting_down() {
            return;
        }
        let to_start: Vec<String> = {
            let mut st = self.inner.state.lock();
            let max_manual = st.settings.max_simultaneous.max(1) as usize;
            let active_manual = st.entries.values().filter(|e| e.rt.is_some() && e.d.manual).count();
            let mut start = vec![];
            let mut free = max_manual.saturating_sub(active_manual);
            for id in st.order.clone() {
                if free == 0 {
                    break;
                }
                if let Some(e) = st.entries.get(&id) {
                    if e.d.status == Status::Queued && e.d.manual && e.rt.is_none() && !e.busy {
                        start.push(id.clone());
                        free -= 1;
                    }
                }
            }
            let mut finished_queues = vec![];
            let queues = st.queues.clone();
            for q in queues.iter().filter(|q| q.running) {
                let active = st
                    .entries
                    .values()
                    .filter(|e| e.rt.is_some() && !e.d.manual && e.d.queue_id.as_deref() == Some(&q.id))
                    .count();
                let mut free = (q.max_concurrent.max(1) as usize).saturating_sub(active);
                let mut pending = 0;
                for id in &q.order {
                    let Some(e) = st.entries.get_mut(id) else { continue };
                    if e.rt.is_some() || e.d.manual {
                        continue;
                    }
                    let eligible = e.d.status == Status::Queued
                        || (e.d.status == Status::Error && e.d.retries_used < q.retry_failed);
                    if !eligible {
                        continue;
                    }
                    pending += 1;
                    if free > 0 && !e.busy && !start.contains(id) {
                        if e.d.status == Status::Error {
                            e.d.retries_used += 1;
                        }
                        e.d.status = Status::Queued;
                        start.push(id.clone());
                        free -= 1;
                    }
                }
                if active == 0 && pending == 0 {
                    finished_queues.push(q.id.clone());
                }
            }
            let mut finished = vec![];
            for qid in finished_queues {
                let completed = st.queue_completed.remove(&qid).unwrap_or(0);
                if let Some(q) = st.queues.iter_mut().find(|q| q.id == qid) {
                    q.running = false;
                    if completed > 0 {
                        finished.push(q.clone());
                    }
                }
            }
            drop(st);
            if !finished.is_empty() {
                for q in finished {
                    self.emit(Event::QueueFinished(q));
                }
                self.emit_queues();
            }
            start
        };
        for id in to_start {
            self.launch(&id);
        }
    }

    fn launch(&self, id: &str) {
        if self.is_shutting_down() {
            return;
        }
        let s = self.settings();
        let rt = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            if e.rt.is_some() || e.busy {
                return;
            }
            let rt = Runtime::with_gen(
                e.d.segments.clone(),
                e.d.total_bytes,
                e.d.resumable,
                per_limit(&e.d, &s),
                e.gen,
            );
            e.rt = Some(rt.clone());
            e.d.status = Status::Connecting;
            e.d.error = None;
            e.last_bytes = e.d.received_bytes;
            e.ema = 0.0;
            rt
        };
        self.changed(id);
        let m = self.clone();
        let id2 = id.to_string();
        let rt2 = rt.clone();
        let spawned = self.spawn(async move {
            let _guard = DetachGuard(rt2.clone());
            m.drive(id2, rt2).await;
        });
        if !spawned {
            {
                let mut st = self.inner.state.lock();
                if let Some(e) = st.entries.get_mut(id) {
                    if e.owns(&rt) {
                        e.rt = None;
                        e.d.status = Status::Error;
                        e.d.error = Some("download engine is not running".into());
                    }
                }
            }
            rt.mark_detached();
            self.changed(id);
        }
    }

    async fn drive(&self, id: String, rt: Arc<Runtime>) {
        let s = self.settings();
        let Some(mut d) = self.get(&id) else { return };
        let clients = self.clients();

        // an earlier completion was interrupted after the file was moved
        // into place: only verification and bookkeeping are left
        if let Some(total) = d.total_bytes {
            let all_done = !d.segments.is_empty()
                && d.segments.iter().all(|v| v.end != UNKNOWN && v.start.saturating_add(v.received) >= v.end);
            let fp = d.final_path();
            if all_done
                && !util::part_path(&fp).exists()
                && std::fs::metadata(&fp).map(|m| m.len() == total).unwrap_or(false)
            {
                self.complete(&id, &rt, total, None).await;
                return;
            }
        }

        // probe when the size or the direct link are unknown
        if d.final_url.is_none() || d.total_bytes.is_none() {
            let hs = HeaderSet::for_download(&d, &s);
            let probed = tokio::select! {
                _ = rt.cancel.cancelled() => None,
                p = client::probe_with(&clients.probe, &d.url, &hs, &s.categories) => Some(p),
            };
            match probed {
                None => {
                    self.finish_run(&id, &rt, None);
                    return;
                }
                Some(Err(e)) => {
                    self.fail(&id, &rt, e.to_string());
                    return;
                }
                Some(Ok(p)) => {
                    let applied = {
                        let mut st = self.inner.state.lock();
                        let rename = match st.entries.get_mut(&id) {
                            Some(e) if e.current(&rt) => {
                                // progress from an earlier run is useless if the file changed
                                let had_data = e.d.received_bytes > 0;
                                let changed_file = had_data
                                    && (matches!((e.d.total_bytes, p.total_bytes), (Some(a), Some(b)) if a != b)
                                        || matches!((&e.d.etag, &p.etag), (Some(a), Some(b)) if strip_weak(a) != strip_weak(b)));
                                if changed_file {
                                    tracing::info!("{id}: file changed on server since the last run; starting over");
                                    e.d.segments.clear();
                                    e.d.received_bytes = 0;
                                    // the adopted bytes are being replaced, so they are ours now
                                    e.d.adopted = None;
                                    rt.segs.lock().clear();
                                }
                                e.d.total_bytes = p.total_bytes;
                                e.d.resumable = p.resumable;
                                e.d.final_url = Some(p.final_url.clone());
                                e.d.mime = p.mime.clone();
                                e.d.protocol = Some(p.protocol.clone());
                                e.d.etag = p.etag.clone();
                                e.d.last_modified = p.last_modified.clone();
                                rt.total.store(p.total_bytes.unwrap_or(UNKNOWN), Ordering::SeqCst);
                                rt.resumable.store(p.resumable, Ordering::SeqCst);
                                let mut rename = None;
                                if e.d.received_bytes == 0 && e.d.adopted.is_none() {
                                    if e.d.auto_name && p.file_name != e.d.file_name {
                                        let cat = util::category_for(&p.file_name, &s.categories);
                                        if e.d.category == "general" {
                                            e.d.category = cat;
                                        }
                                        let dir = if e.d.auto_dir {
                                            category_folder(&s, &e.d.category)
                                        } else {
                                            e.d.save_dir.clone()
                                        };
                                        rename = Some((dir, p.file_name.clone()));
                                    }
                                    e.d.auto_name = false;
                                }
                                Some(rename)
                            }
                            _ => None,
                        };
                        match rename {
                            Some(r) => {
                                if let Some((dir, name)) = r {
                                    // the server's name is reserved like any other
                                    relocate(&mut st, &id, &dir, &name);
                                }
                                if let Some(e) = st.entries.get(&id) {
                                    d = e.d.clone();
                                }
                                true
                            }
                            None => false,
                        }
                    };
                    if !applied {
                        self.finish_run(&id, &rt, None);
                        return;
                    }
                    self.changed(&id);
                }
            }
        }

        let part = util::part_path(&d.final_path());
        let url = d.final_url.clone().unwrap_or_else(|| d.url.clone());
        let via_final = url != d.url;
        let ctx = Arc::new(TaskCtx {
            client: clients.segment,
            url,
            headers: HeaderSet::for_download(&d, &s),
            part: part.clone(),
            max_conn: d.max_connections.max(1),
            min_segment: s.min_segment_kb.max(64) * 1024,
            global: self.inner.global.clone(),
            max_retries: s.max_retries,
            retry_delay: Duration::from_millis((s.retry_delay_secs * 1000).max(200)),
            etag: d.etag.clone(),
            last_modified: d.last_modified.clone(),
            rt: rt.clone(),
        });

        match task::run(ctx).await {
            Ok(out) => self.complete(&id, &rt, out.total, Some(part)).await,
            Err(TaskError::Cancelled) => self.finish_run(&id, &rt, None),
            Err(TaskError::Restart { resumable, reason }) => self.restart(&id, &rt, resumable, &reason, &part),
            Err(TaskError::Http(code)) if via_final && matches!(code, 401 | 403 | 404 | 410) && self.fall_back(&id, &rt) => {
                tracing::info!("{id}: direct link answered HTTP {code}; trying the original URL");
                self.changed(&id);
                self.launch(&id);
            }
            Err(TaskError::Http(code)) => self.fail(&id, &rt, client::status_message(code)),
            Err(TaskError::Fatal(msg)) => self.fail(&id, &rt, msg),
        }
    }

    /// The direct link stopped working: forget it so the next run probes the
    /// original URL again (once per session). Returns true to relaunch.
    fn fall_back(&self, id: &str, rt: &Arc<Runtime>) -> bool {
        let mut st = self.inner.state.lock();
        let Some(e) = st.entries.get_mut(id) else { return false };
        if !e.current(rt) || e.fell_back || rt.cancel.is_cancelled() {
            return false;
        }
        e.fell_back = true;
        e.d.final_url = None;
        e.d.segments = rt.views();
        e.d.received_bytes = rt.received();
        e.rt = None;
        e.ema = 0.0;
        e.d.status = Status::Queued;
        true
    }

    /// The task asked to start over (file changed, or ranges unusable).
    fn restart(&self, id: &str, rt: &Arc<Runtime>, resumable: bool, reason: &str, part: &Path) {
        let mut failed = false;
        let again = {
            let mut st = self.inner.state.lock();
            match st.entries.get_mut(id) {
                Some(e) if e.owns(rt) => {
                    e.rt = None;
                    e.ema = 0.0;
                    if e.gen != rt.gen {
                        false
                    } else {
                        e.d.restarts += 1;
                        e.d.segments.clear();
                        e.d.received_bytes = 0;
                        e.d.etag = None;
                        e.d.last_modified = None;
                        if resumable {
                            // the file changed: learn its new size and validators
                            e.d.total_bytes = None;
                            e.d.final_url = None;
                        } else {
                            e.d.resumable = false;
                        }
                        // the old bytes are of no use (this run's writers have stopped)
                        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(part) {
                            let _ = f.set_len(0);
                        }
                        e.d.adopted = None;
                        if rt.cancel.is_cancelled() {
                            // paused / stopped meanwhile: keep the user's choice
                            if e.d.status.is_active() {
                                e.d.status = Status::Paused;
                            }
                            false
                        } else if e.d.restarts <= MAX_RESTARTS {
                            e.d.status = Status::Queued;
                            true
                        } else {
                            e.d.status = Status::Error;
                            e.d.error = Some(util::redact_urls(reason));
                            e.d.manual = false;
                            failed = true;
                            false
                        }
                    }
                }
                _ => false,
            }
        };
        tracing::warn!("restarting {id}: {}", util::redact_urls(reason));
        self.changed(id);
        if failed {
            if let Some(d) = self.get(id) {
                self.emit(Event::Failed(d));
            }
        }
        if again {
            self.launch(id);
        } else {
            self.pump();
        }
    }

    /// Detach a finished/cancelled runtime, saving segment progress.
    fn finish_run(&self, id: &str, rt: &Arc<Runtime>, status: Option<Status>) {
        {
            let mut st = self.inner.state.lock();
            if let Some(e) = st.entries.get_mut(id) {
                if e.owns(rt) {
                    e.rt = None;
                    e.ema = 0.0;
                    if e.gen == rt.gen {
                        // writers have stopped and synced: `written` is on disk
                        e.d.segments = rt.views();
                        e.d.received_bytes = rt.received();
                        let t = rt.total.load(Ordering::SeqCst);
                        if t != UNKNOWN {
                            e.d.total_bytes = Some(t);
                        }
                        e.d.resumable = rt.resumable.load(Ordering::SeqCst);
                        if let Some(s) = status {
                            e.d.status = s;
                        } else if e.d.status.is_active() {
                            e.d.status = Status::Paused;
                        }
                        for sv in e.d.segments.iter_mut() {
                            sv.speed_bps = 0;
                            if sv.state != SegState::Done {
                                sv.state = SegState::Idle;
                            }
                        }
                    }
                }
            }
        }
        self.changed(id);
        self.pump();
    }

    fn fail(&self, id: &str, rt: &Arc<Runtime>, msg: String) {
        let msg = util::redact_urls(&msg);
        {
            let mut st = self.inner.state.lock();
            if let Some(e) = st.entries.get_mut(id) {
                if e.current(rt) {
                    e.d.error = Some(msg);
                    e.d.manual = false;
                }
            }
        }
        self.finish_run(id, rt, Some(Status::Error));
        if let Some(d) = self.get(id) {
            if d.status == Status::Error {
                self.emit(Event::Failed(d));
            }
        }
    }

    /// Move the finished part file into place, verify it and mark the
    /// download complete. `part` is None when the file was already moved by
    /// an interrupted earlier completion.
    async fn complete(&self, id: &str, rt: &Arc<Runtime>, total: u64, part: Option<PathBuf>) {
        rt.phase.store(2, Ordering::SeqCst);
        let info = {
            let mut st = self.inner.state.lock();
            match st.entries.get_mut(id) {
                Some(e) if e.current(rt) => {
                    e.d.status = Status::Assembling;
                    Some((
                        e.d.final_path(),
                        e.d.checksum.clone(),
                        e.d.on_exists.clone(),
                        // the file at the final path is our own earlier, corrupt result
                        e.d.checksum_ok == Some(false),
                        e.d.url.clone(),
                        e.d.referrer.clone(),
                    ))
                }
                _ => None,
            }
        };
        let Some((final_path, checksum, policy, prior_bad, url, referrer)) = info else {
            self.finish_run(id, rt, None);
            return;
        };
        self.changed(id);

        let dest = match part {
            Some(part) => {
                let mut dest = final_path.clone();
                if dest.exists() {
                    let replace = prior_bad || matches!(policy.as_deref(), Some("overwrite") | Some("resume"));
                    if replace {
                        if let Err(e) = std::fs::remove_file(&dest) {
                            self.fail(id, rt, format!("could not replace {}: {e}", dest.display()));
                            return;
                        }
                    } else {
                        let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
                        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let st = self.inner.state.lock();
                        dest = dir.join(reserve_name(&st, &dir, &name, Some(id), false, false));
                    }
                }
                // marked while it still has its part-file name, so the file
                // never exists under its final name without the mark (the
                // mark travels with the rename)
                util::mark_downloaded(&part, &url, referrer.as_deref());
                if let Err(e) = std::fs::rename(&part, &dest) {
                    self.fail(id, rt, format!("could not move finished file into place: {e}"));
                    return;
                }
                *rt.placed.lock() = Some(dest.clone());
                // record the move at once: if we're interrupted from here on,
                // the next start only re-verifies
                {
                    let mut st = self.inner.state.lock();
                    if let Some(e) = st.entries.get_mut(id) {
                        if e.current(rt) {
                            if let Some(n) = dest.file_name() {
                                e.d.file_name = n.to_string_lossy().into_owned();
                                e.d.ext = util::badge_ext(&e.d.file_name);
                            }
                            e.d.total_bytes = Some(total);
                            e.d.received_bytes = total;
                            e.d.segments = done_segments(rt, total);
                        }
                    }
                }
                self.changed(id);
                dest
            }
            None => final_path,
        };

        let mut checksum_ok = None;
        if let Some(c) = checksum {
            let algo = if c.algo.is_empty() { util::guess_algo(&c.expected).to_string() } else { c.algo.clone() };
            let p = dest.clone();
            let got = tokio::task::spawn_blocking(move || util::hash_file(&p, &algo)).await;
            checksum_ok = Some(matches!(got, Ok(Ok(ref h)) if h.eq_ignore_ascii_case(c.expected.trim())));
        }
        // again under the final name: covers a file moved into place by an
        // earlier, interrupted completion
        util::mark_downloaded(&dest, &url, referrer.as_deref());

        let outcome = {
            let mut st = self.inner.state.lock();
            match st.entries.get_mut(id) {
                Some(e) if e.current(rt) => {
                    e.d.file_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(e.d.file_name.clone());
                    e.d.ext = util::badge_ext(&e.d.file_name);
                    e.d.total_bytes = Some(total);
                    e.d.checksum_ok = checksum_ok;
                    e.rt = None;
                    e.ema = 0.0;
                    e.fell_back = false;
                    if checksum_ok == Some(false) {
                        e.d.status = Status::Error;
                        e.d.error = Some("checksum mismatch — the file may be corrupt".into());
                        // the corrupt file stays for inspection; a retry
                        // downloads everything again
                        e.d.segments.clear();
                        e.d.received_bytes = 0;
                        e.d.final_url = None;
                        e.d.etag = None;
                        e.d.last_modified = None;
                    } else {
                        e.d.received_bytes = total;
                        e.d.segments = done_segments(rt, total);
                        e.d.completed_at = Some(now_iso());
                        e.d.status = Status::Complete;
                        e.d.error = None;
                        // the file is here: the session cookies, password and
                        // custom headers that fetched it are not kept around
                        e.d.cookies = None;
                        e.d.password = None;
                        e.d.headers.clear();
                    }
                    let q = e.d.queue_id.clone().filter(|_| !e.d.manual);
                    e.d.manual = false;
                    Some((e.d.status == Status::Complete, q))
                }
                _ => None,
            }
        };
        let Some((completed, queue_id)) = outcome else {
            // deleted or reset while verifying; whoever did that handles the files
            self.finish_run(id, rt, None);
            return;
        };
        if let Some(q) = queue_id {
            *self.inner.state.lock().queue_completed.entry(q).or_insert(0) += 1;
        }
        self.changed(id);
        if let Some(d) = self.get(id) {
            if completed {
                self.emit(Event::Completed(d));
            } else {
                self.emit(Event::Failed(d));
            }
        }
        self.pump();
    }

    // ------------------------------------------------------------ background

    async fn ticker(&self) {
        let mut iv = tokio::time::interval(Duration::from_millis(250));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut n: u64 = 0;
        let alpha = 1.0 - (-0.25f64).exp(); // 1 s EMA at 4 Hz
        loop {
            tokio::select! {
                _ = self.inner.bg.cancelled() => return,
                _ = iv.tick() => {}
            }
            n += 1;
            let second = n.is_multiple_of(4);
            let persist = n.is_multiple_of(8);
            let mut batch = vec![];
            let mut status_changes = vec![];
            let mut to_save = vec![];
            {
                let mut st = self.inner.state.lock();
                for e in st.entries.values_mut() {
                    let Some(rt) = e.rt.clone() else { continue };
                    let received = rt.received();
                    let delta = received.saturating_sub(e.last_bytes);
                    e.last_bytes = received;
                    e.ema = e.ema + alpha * ((delta as f64 * 4.0) - e.ema);
                    for seg in rt.segs.lock().iter() {
                        let w = seg.written();
                        let last = seg.last_sample.swap(w, Ordering::Relaxed);
                        let inst = w.saturating_sub(last) as f64 * 4.0;
                        let old = seg.speed.load(Ordering::Relaxed) as f64;
                        seg.speed.store((old + alpha * (inst - old)) as u64, Ordering::Relaxed);
                    }
                    if second {
                        e.history.push_back(e.ema as u64);
                        while e.history.len() > HISTORY {
                            e.history.pop_front();
                        }
                    }
                    let phase = rt.phase.load(Ordering::SeqCst);
                    let want = match phase {
                        0 => Status::Connecting,
                        1 => Status::Downloading,
                        _ => Status::Assembling,
                    };
                    if e.d.status != want && e.d.status.is_active() {
                        e.d.status = want;
                        status_changes.push(e.d.id.clone());
                    }
                    let total = {
                        let t = rt.total.load(Ordering::SeqCst);
                        (t != UNKNOWN).then_some(t)
                    };
                    let speed = e.ema as u64;
                    batch.push(ProgressItem {
                        id: e.d.id.clone(),
                        received_bytes: received,
                        total_bytes: total,
                        speed_bps: speed,
                        eta_secs: eta(total, received, speed),
                        active_connections: rt.active.load(Ordering::SeqCst),
                        segments: rt.views(),
                        status: e.d.status,
                    });
                    if persist {
                        to_save.push(e.d.id.clone());
                    }
                }
            }
            for id in status_changes {
                self.changed(&id);
            }
            if !batch.is_empty() {
                self.emit(Event::Progress(batch));
            }
            // snapshots are taken inside persist(), never stale
            if !to_save.is_empty() {
                self.persist(&to_save);
            }
            if second {
                self.emit(Event::Totals(self.totals()));
            }
        }
    }

    pub fn totals(&self) -> Totals {
        let st = self.inner.state.lock();
        let mut t = Totals::default();
        for e in st.entries.values() {
            match e.d.status {
                Status::Connecting | Status::Downloading | Status::Assembling => {
                    t.active += 1;
                    t.speed_bps += e.ema as u64;
                }
                Status::Paused => t.paused += 1,
                Status::Error => t.failed += 1,
                Status::Complete => t.complete += 1,
                Status::Queued => {
                    let scheduled = !e.d.manual
                        && st
                            .queues
                            .iter()
                            .any(|q| Some(&q.id) == e.d.queue_id.as_ref() && !q.running && q.start_enabled);
                    if scheduled {
                        t.scheduled += 1;
                    } else {
                        t.queued += 1;
                    }
                }
            }
        }
        t.limit_on = st.settings.global_limit_enabled;
        t.limit_bps = st.settings.global_limit_bps;
        let folder = st.settings.default_folder.clone();
        drop(st);
        t.free_bytes = free_space(Path::new(&folder));
        t
    }

    async fn scheduler(&self) {
        let mut iv = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = self.inner.bg.cancelled() => return,
                _ = iv.tick() => {}
            }
            self.scheduler_tick(Local::now().naive_local());
        }
    }

    /// One scheduler pass at local time `now` (public for tests).
    pub fn scheduler_tick(&self, now: chrono::NaiveDateTime) {
        if self.is_shutting_down() {
            return;
        }
        let hm = format!("{:02}:{:02}", now.hour(), now.minute());
        let wd = now.weekday().num_days_from_monday() as usize;
        let date = now.date().to_string();
        let mut starts = vec![];
        let mut stops = vec![];
        {
            let mut st = self.inner.state.lock();
            let queues = st.queues.clone();
            for q in queues {
                if !q.days.get(wd).copied().unwrap_or(true) {
                    continue;
                }
                if q.start_enabled && q.start_time == hm {
                    let key = format!("{}:start:{date}:{hm}", q.id);
                    if st.fired.insert(key) && !q.running {
                        starts.push(q.id.clone());
                    }
                }
                if q.stop_enabled && q.stop_time == hm {
                    let key = format!("{}:stop:{date}:{hm}", q.id);
                    if st.fired.insert(key) && q.running {
                        stops.push(q.id.clone());
                    }
                }
            }
            if st.fired.len() > 1000 {
                st.fired.clear();
            }
        }
        for q in starts {
            tracing::info!("scheduler: starting queue {q}");
            self.start_queue(&q);
        }
        for q in stops {
            tracing::info!("scheduler: stopping queue {q}");
            self.stop_queue(&q);
        }
    }

    /// Stop starting things and ask every running download to stop.
    fn begin_shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.bg.cancel();
        let rts: Vec<Arc<Runtime>> = self.inner.state.lock().entries.values().filter_map(|e| e.rt.clone()).collect();
        for rt in rts {
            rt.cancel.cancel();
        }
    }

    /// Best-effort synchronous shutdown: stop everything and persist what
    /// is known to be on disk. Prefer [`shutdown_wait`](Self::shutdown_wait).
    pub fn shutdown(&self) {
        self.begin_shutdown();
        self.persist_all();
        self.inner.store.checkpoint();
    }

    /// Orderly shutdown: stop the scheduler and every download, wait (up to
    /// `timeout`) for writers to flush and sync and for in-progress
    /// completions to finish, then persist everything and checkpoint the
    /// database. Nothing starts afterwards.
    pub async fn shutdown_wait(&self, timeout: Duration) {
        self.begin_shutdown();
        self.inner.tracker.close();
        if tokio::time::timeout(timeout, self.inner.tracker.wait()).await.is_err() {
            tracing::warn!("shutdown: some downloads did not stop within {timeout:?}");
        }
        self.persist_all();
        self.inner.store.checkpoint();
    }

    pub fn grab_client(&self) -> (reqwest::Client, Settings) {
        (self.client(), self.settings())
    }
}

/// Segment views of a finished run, all marked done.
fn done_segments(rt: &Runtime, total: u64) -> Vec<SegmentView> {
    let mut v = rt.views();
    if v.is_empty() {
        v.push(SegmentView { start: 0, end: total, received: total, state: SegState::Done, speed_bps: 0 });
    }
    for s in v.iter_mut() {
        s.state = SegState::Done;
        s.speed_bps = 0;
    }
    v
}

fn strip_weak(etag: &str) -> &str {
    let e = etag.trim();
    e.strip_prefix("W/").unwrap_or(e)
}

/// Comparable form of a path (case-insensitive where the usual file
/// systems are).
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(any(windows, target_os = "macos")) {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

/// Is `final_path` (or its part file) used by a download other than
/// `except`? `ignore_complete`: finished downloads don't count.
fn claimed(st: &State, final_path: &Path, except: Option<&str>, ignore_complete: bool) -> bool {
    let k_final = path_key(final_path);
    let k_part = path_key(&util::part_path(final_path));
    st.entries.values().any(|e| {
        if Some(e.d.id.as_str()) == except || (ignore_complete && e.d.status == Status::Complete) {
            return false;
        }
        let f = e.d.final_path();
        let kf = path_key(&f);
        kf == k_final || kf == k_part || path_key(&util::part_path(&f)) == k_final
    })
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Pick `name`, or `name (1)`, `name (2)`, ... in `dir`: the first that no
/// other download uses and that doesn't exist on disk. With `create_part`
/// its empty `.odmpart` is created (`create_new`) so the name stays taken.
/// `overwrite`: `name` itself may replace an existing finished file.
/// Call with the state lock held.
fn reserve_name(st: &State, dir: &Path, name: &str, except: Option<&str>, overwrite: bool, create_part: bool) -> String {
    if create_part {
        let _ = std::fs::create_dir_all(dir);
    }
    let (stem, ext) = split_name(name);
    // the name `except` holds already is free for it (unless a finished file sits there)
    let own = except.and_then(|id| st.entries.get(id)).map(|e| path_key(&e.d.final_path()));
    for n in 0..10_000u32 {
        let cand = if n == 0 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let fp = dir.join(&cand);
        if own.as_deref() == Some(path_key(&fp).as_str()) && !fp.exists() {
            let pp = util::part_path(&fp);
            if create_part && !pp.exists() {
                let _ = std::fs::OpenOptions::new().write(true).create_new(true).open(&pp);
            }
            return cand;
        }
        let replace = overwrite && n == 0;
        if claimed(st, &fp, except, replace) || (fp.exists() && !replace) {
            continue;
        }
        let pp = util::part_path(&fp);
        if replace && pp.exists() {
            // leftover of an earlier attempt nobody owns
            let _ = std::fs::remove_file(&pp);
        }
        if !create_part {
            if pp.exists() {
                continue;
            }
            return cand;
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&pp) {
            Ok(_) => return cand,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                // e.g. the folder isn't writable yet; the run reports it
                tracing::warn!("cannot reserve {}: {e}", pp.display());
                return cand;
            }
        }
    }
    name.to_string()
}

/// Re-home a download that is not running to `dir`/`name` (or a free
/// variant of it), carrying its part file along. Returns false (nothing
/// changed) if the part file could not be moved.
fn relocate(st: &mut State, id: &str, dir: &str, name: &str) -> bool {
    let Some(e) = st.entries.get(id) else { return false };
    let old_final = e.d.final_path();
    let keep_data = e.d.received_bytes > 0 || e.d.adopted.is_some();
    let new_dir = PathBuf::from(dir);
    if path_key(&old_final) == path_key(&new_dir.join(name)) {
        return true;
    }
    let new_name = reserve_name(st, &new_dir, name, Some(id), false, true);
    let old_part = util::part_path(&old_final);
    let new_part = util::part_path(&new_dir.join(&new_name));
    if path_key(&old_part) != path_key(&new_part) && old_part.exists() {
        let has_bytes = std::fs::metadata(&old_part).map(|m| m.len() > 0).unwrap_or(true);
        if keep_data || has_bytes {
            if let Err(err) = std::fs::rename(&old_part, &new_part) {
                tracing::warn!("cannot move {} to {}: {err}", old_part.display(), new_part.display());
                let _ = std::fs::remove_file(&new_part);
                return false;
            }
        } else {
            // our own empty placeholder
            let _ = std::fs::remove_file(&old_part);
        }
    }
    if let Some(e) = st.entries.get_mut(id) {
        e.d.save_dir = dir.to_string();
        e.d.ext = util::badge_ext(&new_name);
        e.d.file_name = new_name;
    }
    true
}

/// File clean-up after a download was removed from the list.
#[derive(Clone)]
struct Cleanup {
    final_path: PathBuf,
    adopted: Option<String>,
    complete: bool,
    delete_files: bool,
}

impl Cleanup {
    /// One pass; true if nothing is left to do.
    fn attempt(&self, placed: Option<&Path>) -> bool {
        let mut ok = true;
        let mut remove = |p: &Path| {
            if let Err(e) = std::fs::remove_file(p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::debug!("cannot remove {}: {e}", p.display());
                    ok = false;
                }
            }
        };
        let part = util::part_path(&self.final_path);
        match self.adopted.as_deref() {
            // the user's own file was taken over: give it back
            Some("file") => {
                if part.exists() && !self.final_path.exists() {
                    let _ = std::fs::rename(&part, &self.final_path);
                }
            }
            // a part file the engine didn't create stays
            Some(_) => {}
            None => remove(&part),
        }
        if self.delete_files {
            if self.complete {
                remove(&self.final_path);
            }
            if let Some(p) = placed {
                remove(p);
            }
        }
        ok
    }

    /// Retry for a while (files may still be held open, e.g. by a virus
    /// scanner on Windows).
    async fn run(self, placed: Option<PathBuf>) {
        for _ in 0..20 {
            if self.attempt(placed.as_deref()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tracing::warn!("could not remove all files of {}", self.final_path.display());
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPatch {
    pub when_finished: Option<String>,
    pub description: Option<String>,
    pub max_connections: Option<u32>,
    pub referrer: Option<String>,
    pub cookies: Option<String>,
    pub file_name: Option<String>,
}

fn per_limit(d: &Download, s: &Settings) -> u64 {
    match d.speed_limit_bps {
        Some(b) => b,
        None if s.per_download_limit_enabled => s.per_download_limit_bps,
        None => 0,
    }
}

fn eta(total: Option<u64>, received: u64, speed: u64) -> Option<u64> {
    let t = total?;
    if speed < 1 {
        return None;
    }
    Some(t.saturating_sub(received) / speed)
}

pub fn free_space(p: &Path) -> Option<u64> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if c.exists() {
            return fs4::available_space(c).ok();
        }
        cur = c.parent();
    }
    None
}

/// Next scheduled start for a queue, as RFC 3339 local time.
pub fn next_run(q: &Queue) -> Option<String> {
    if !q.start_enabled {
        return None;
    }
    let (h, m) = q.start_time.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    let now = Local::now();
    for add in 0..8 {
        let day = now.date_naive() + chrono::Duration::days(add);
        let wd = day.weekday().num_days_from_monday() as usize;
        if !q.days.get(wd).copied().unwrap_or(true) {
            continue;
        }
        let t = day.and_hms_opt(h, m, 0)?;
        let local = t.and_local_timezone(Local).single()?;
        if local > now {
            return Some(local.to_rfc3339());
        }
    }
    None
}
