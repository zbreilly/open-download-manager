//! Download manager: owns the list, starts/stops tasks, enforces
//! simultaneous-download and queue limits, runs the scheduler, measures
//! speed, and broadcasts events for the UI.

use crate::client;
use crate::limiter::Limiter;
use crate::model::*;
use crate::store::Store;
use crate::task::{self, Runtime, TaskCtx, TaskError, UNKNOWN};
use crate::util;
use crate::CoreError;
use chrono::{Datelike, Local, Timelike};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

pub const MAIN_QUEUE: &str = "main";
const HISTORY: usize = 60;

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
    events: broadcast::Sender<Event>,
    global: Limiter,
    client: RwLock<reqwest::Client>,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Inner>,
}

fn now_iso() -> String {
    Local::now().to_rfc3339()
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
        if !settings.limit_on_startup {
            // the toolbar toggle is a session switch unless "on at startup"
            settings.global_limit_enabled = false;
        } else {
            settings.global_limit_enabled = true;
        }
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
            if d.status.is_active() {
                d.status = Status::Paused;
            }
            if d.status == Status::Queued && d.manual {
                d.status = Status::Paused;
                d.manual = false;
            }
            order.push(d.id.clone());
            entries.insert(
                d.id.clone(),
                Entry { d, rt: None, history: VecDeque::new(), last_bytes: 0, ema: 0.0 },
            );
        }
        let recent: Vec<String> = store
            .get_kv("recent")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let client = client::build_client(&settings)?;
        let global = Limiter::new(if settings.global_limit_enabled { settings.global_limit_bps } else { 0 });
        let (tx, _) = broadcast::channel(4096);
        Ok(Self {
            inner: Arc::new(Inner {
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
                events: tx,
                global,
                client: RwLock::new(client),
            }),
        })
    }

    /// Spawn the progress ticker and the scheduler. Call inside a tokio runtime.
    pub fn start_background(&self) {
        let m = self.clone();
        tokio::spawn(async move { m.ticker().await });
        let m = self.clone();
        tokio::spawn(async move { m.scheduler().await });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    fn emit(&self, e: Event) {
        let _ = self.inner.events.send(e);
    }

    fn client(&self) -> reqwest::Client {
        self.inner.client.read().clone()
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

    fn changed(&self, id: &str) {
        let d = self.get(id);
        if let Some(d) = d {
            self.inner.store.save_download(&d);
            self.emit(Event::Changed(d));
        }
    }

    // ------------------------------------------------------------ settings

    pub fn settings(&self) -> Settings {
        self.inner.state.lock().settings.clone()
    }

    pub fn save_settings(&self, mut s: Settings) -> Result<(), CoreError> {
        if s.categories.is_empty() {
            s.categories = default_categories();
        }
        normalize_categories(&mut s.categories);
        s.default_connections = s.default_connections.clamp(1, 32);
        s.max_simultaneous = s.max_simultaneous.clamp(1, 64);
        let client = client::build_client(&s)?;
        *self.inner.client.write() = client;
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
        client::probe(&self.client(), url, h, &s.categories).await
    }

    /// Default save folder for a category.
    pub fn folder_for(&self, category: &str) -> String {
        category_folder(&self.settings(), category)
    }

    pub fn target_exists(&self, dir: &str, name: &str) -> bool {
        let p = Path::new(dir).join(name);
        p.exists()
    }

    pub fn add(&self, req: AddRequest) -> Result<String, CoreError> {
        let url = req.url.trim().to_string();
        let parsed = url::Url::parse(&url).map_err(|_| CoreError::BadUrl(url.clone()))?;
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
            .unwrap_or_else(|| self.folder_for(&category));

        let mut segments = vec![];
        let mut received = 0;
        let mut resumable = req.resumable.unwrap_or(true);
        let policy = req.on_exists.clone().unwrap_or_else(|| "rename".into());
        let target = Path::new(&save_dir).join(&name);
        if target.exists() || util::part_path(&target).exists() {
            match policy.as_str() {
                "overwrite" => {
                    let _ = std::fs::remove_file(util::part_path(&target));
                }
                "resume" => {
                    // continue an existing partial file from its current size
                    let part = util::part_path(&target);
                    if !part.exists() && target.exists() {
                        let _ = std::fs::rename(&target, &part);
                    }
                    if let Ok(m) = std::fs::metadata(&part) {
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
                }
                _ => {
                    let p = util::unique_path(Path::new(&save_dir), &name);
                    name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
                }
            }
        }

        let queue_id = req.queue_id.clone().filter(|q| !q.is_empty());
        let id = uuid::Uuid::new_v4().to_string();
        let (status, manual, queue_id) = if req.start {
            (Status::Queued, true, queue_id)
        } else if let Some(q) = queue_id {
            (Status::Queued, false, Some(q))
        } else {
            (Status::Paused, false, None)
        };
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
        };
        {
            let mut st = self.inner.state.lock();
            if let Some(q) = &queue_id {
                if let Some(q) = st.queues.iter_mut().find(|x| &x.id == q) {
                    q.order.push(id.clone());
                }
            }
            st.order.push(id.clone());
            st.entries.insert(
                id.clone(),
                Entry { d: d.clone(), rt: None, history: VecDeque::new(), last_bytes: received, ema: 0.0 },
            );
            if req.remember_folder && req.save_dir.is_some() {
                if let Some(c) = st.settings.categories.iter_mut().find(|c| c.id == category) {
                    c.folder = Some(save_dir.clone());
                }
                let s2 = st.settings.clone();
                drop(st);
                self.inner.store.save_settings(&s2);
            }
        }
        self.inner.store.save_download(&d);
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
            }
            self.changed(id);
        }
        self.pump();
    }

    pub fn redownload(&self, id: &str) {
        let rt = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            let rt = e.rt.clone();
            let p = e.d.final_path();
            let _ = std::fs::remove_file(util::part_path(&p));
            e.d.segments.clear();
            e.d.received_bytes = 0;
            e.d.completed_at = None;
            e.d.error = None;
            e.d.checksum_ok = None;
            e.d.status = Status::Queued;
            e.d.manual = true;
            e.d.restarts = 0;
            e.d.retries_used = 0;
            if p.exists() {
                // keep the finished copy; new download gets a fresh name
                let np = util::unique_path(Path::new(&e.d.save_dir), &e.d.file_name);
                if let Some(n) = np.file_name() {
                    e.d.file_name = n.to_string_lossy().into_owned();
                }
            }
            rt
        };
        if let Some(rt) = rt {
            rt.cancel.cancel();
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
            self.inner.store.delete_download(id);
            let final_path = e.d.final_path();
            let part = util::part_path(&final_path);
            let complete = e.d.status == Status::Complete;
            if let Some(rt) = e.rt.clone() {
                rt.cancel.cancel();
                // wait for writers to stop before touching the files
                tokio::spawn(async move {
                    for _ in 0..100 {
                        if rt.active.load(Ordering::SeqCst) == 0 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    let _ = tokio::fs::remove_file(&part).await;
                    if delete_files && complete {
                        let _ = tokio::fs::remove_file(&final_path).await;
                    }
                });
            } else {
                // unfinished parts are always cleaned up; finished files only on request
                let _ = std::fs::remove_file(&part);
                if delete_files && complete {
                    let _ = std::fs::remove_file(&final_path);
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
        let p = self
            .probe(new_url, d.referrer.clone(), d.cookies.clone(), d.headers.clone(), d.username.clone(), d.password.clone())
            .await?;
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
            e.d.final_url = Some(p.final_url.clone());
            e.d.etag = p.etag.clone();
            e.d.last_modified = p.last_modified.clone();
            e.d.resumable = p.resumable;
            e.d.error = None;
            let rt = e.rt.clone();
            if e.d.status == Status::Error {
                e.d.status = Status::Paused;
            }
            rt
        };
        let was_running = rt.is_some();
        if let Some(rt) = rt {
            rt.cancel.cancel();
        }
        self.changed(id);
        if was_running {
            self.resume(&[id.to_string()]);
        }
        Ok(())
    }

    pub fn set_category(&self, ids: &[String], cat: &str) {
        let s = self.settings();
        for id in ids {
            {
                let mut st = self.inner.state.lock();
                if let Some(e) = st.entries.get_mut(id) {
                    e.d.category = cat.to_string();
                    // a download that hasn't started yet moves to the new category's folder
                    if e.d.auto_dir && e.rt.is_none() && e.d.received_bytes == 0 && e.d.status != Status::Complete {
                        e.d.save_dir = category_folder(&s, cat);
                    }
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
            if let Some(n) = patch.file_name {
                if e.rt.is_none() && e.d.status != Status::Complete && !n.trim().is_empty() {
                    let n = util::sanitize_filename(&n);
                    let old_part = util::part_path(&e.d.final_path());
                    e.d.file_name = n;
                    e.d.ext = util::badge_ext(&e.d.file_name);
                    e.d.auto_name = false;
                    let _ = std::fs::rename(old_part, util::part_path(&e.d.final_path()));
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
        for q in &qs {
            self.inner.store.save_queue(q);
        }
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

    pub fn save_queue(&self, mut q: Queue) -> Queue {
        if q.id.is_empty() {
            q.id = uuid::Uuid::new_v4().to_string();
        }
        q.max_concurrent = q.max_concurrent.clamp(1, 32);
        if q.days.len() != 7 {
            q.days = vec![true; 7];
        }
        {
            let mut st = self.inner.state.lock();
            let known: HashSet<String> = st.entries.keys().cloned().collect();
            q.order.retain(|id| known.contains(id));
            match st.queues.iter_mut().find(|x| x.id == q.id) {
                Some(existing) => {
                    q.builtin = existing.builtin;
                    q.running = existing.running;
                    *existing = q.clone();
                }
                None => st.queues.push(q.clone()),
            }
            // queue membership follows the order list
            let members: HashSet<String> = q.order.iter().cloned().collect();
            for id in &members {
                if let Some(e) = st.entries.get_mut(id) {
                    e.d.queue_id = Some(q.id.clone());
                }
            }
        }
        self.inner.store.save_queue(&q);
        for id in &q.order {
            self.changed(id);
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
                    if e.d.status == Status::Queued && e.d.manual && e.rt.is_none() {
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
                    if free > 0 && !start.contains(id) {
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
        let s = self.settings();
        let rt = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            if e.rt.is_some() {
                return;
            }
            let rt = Runtime::new(
                e.d.segments.clone(),
                e.d.total_bytes,
                e.d.resumable,
                per_limit(&e.d, &s),
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
        let id = id.to_string();
        tokio::spawn(async move { m.drive(id, rt).await });
    }

    async fn drive(&self, id: String, rt: Arc<Runtime>) {
        let s = self.settings();
        let Some(d) = self.get(&id) else { return };
        let client = self.client();
        let headers = client::headers_for(&d, &s);

        // probe on first start (size, ranges, server-provided name)
        let mut d = d;
        if d.received_bytes == 0 && (d.total_bytes.is_none() || d.final_url.is_none()) {
            let probed = tokio::select! {
                _ = rt.cancel.cancelled() => None,
                p = client::probe(&client, &d.url, headers.clone(), &s.categories) => Some(p),
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
                    {
                        let mut st = self.inner.state.lock();
                        if let Some(e) = st.entries.get_mut(&id) {
                            e.d.total_bytes = p.total_bytes;
                            e.d.resumable = p.resumable;
                            e.d.final_url = Some(p.final_url.clone());
                            e.d.mime = p.mime.clone();
                            e.d.protocol = Some(p.protocol.clone());
                            e.d.etag = p.etag.clone();
                            e.d.last_modified = p.last_modified.clone();
                            if e.d.auto_name && p.file_name != e.d.file_name {
                                let cat = util::category_for(&p.file_name, &s.categories);
                                if e.d.category == "general" {
                                    e.d.category = cat;
                                    if e.d.auto_dir {
                                        e.d.save_dir = category_folder(&s, &e.d.category);
                                    }
                                }
                                let dir = PathBuf::from(&e.d.save_dir);
                                let np = util::unique_path(&dir, &p.file_name);
                                if let Some(n) = np.file_name() {
                                    e.d.file_name = n.to_string_lossy().into_owned();
                                    e.d.ext = util::badge_ext(&e.d.file_name);
                                }
                                e.d.auto_name = false;
                            }
                            rt.total.store(p.total_bytes.unwrap_or(UNKNOWN), Ordering::SeqCst);
                            rt.resumable.store(p.resumable, Ordering::SeqCst);
                            d = e.d.clone();
                        }
                    }
                    self.changed(&id);
                }
            }
        }

        let part = util::part_path(&d.final_path());
        let ctx = Arc::new(TaskCtx {
            client,
            url: d.final_url.clone().unwrap_or_else(|| d.url.clone()),
            headers,
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
            Ok(out) => self.complete(&id, &rt, out.total, &part).await,
            Err(TaskError::Cancelled) => self.finish_run(&id, &rt, None),
            Err(TaskError::Restart { resumable, reason }) => {
                let again = {
                    let mut st = self.inner.state.lock();
                    match st.entries.get_mut(&id) {
                        Some(e) if e.rt.as_ref().map(|r| Arc::ptr_eq(r, &rt)).unwrap_or(false) => {
                            e.d.restarts += 1;
                            e.d.resumable = resumable;
                            e.d.segments.clear();
                            e.d.received_bytes = 0;
                            if !resumable {
                                e.d.etag = None;
                                e.d.last_modified = None;
                            }
                            e.rt = None;
                            if e.d.restarts <= 2 {
                                e.d.status = Status::Queued;
                                true
                            } else {
                                e.d.status = Status::Error;
                                e.d.error = Some(reason.clone());
                                false
                            }
                        }
                        _ => false,
                    }
                };
                let _ = std::fs::remove_file(&part);
                tracing::warn!("restarting {id}: {reason}");
                self.changed(&id);
                if again {
                    self.launch(&id);
                } else {
                    self.pump();
                }
            }
            Err(TaskError::Fatal(msg)) => self.fail(&id, &rt, msg),
        }
    }

    /// Detach a finished/cancelled runtime, saving segment progress.
    fn finish_run(&self, id: &str, rt: &Arc<Runtime>, status: Option<Status>) {
        {
            let mut st = self.inner.state.lock();
            if let Some(e) = st.entries.get_mut(id) {
                if e.rt.as_ref().map(|r| Arc::ptr_eq(r, rt)).unwrap_or(false) {
                    e.d.segments = rt.views();
                    e.d.received_bytes = rt.received();
                    let t = rt.total.load(Ordering::SeqCst);
                    if t != UNKNOWN {
                        e.d.total_bytes = Some(t);
                    }
                    e.d.resumable = rt.resumable.load(Ordering::SeqCst);
                    e.rt = None;
                    e.ema = 0.0;
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
        self.changed(id);
        self.pump();
    }

    fn fail(&self, id: &str, rt: &Arc<Runtime>, msg: String) {
        {
            let mut st = self.inner.state.lock();
            if let Some(e) = st.entries.get_mut(id) {
                if e.rt.as_ref().map(|r| Arc::ptr_eq(r, rt)).unwrap_or(false) {
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

    async fn complete(&self, id: &str, rt: &Arc<Runtime>, total: u64, part: &Path) {
        rt.phase.store(2, Ordering::SeqCst);
        let (final_path, checksum, policy) = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            e.d.status = Status::Assembling;
            (e.d.final_path(), e.d.checksum.clone(), e.d.on_exists.clone())
        };
        self.changed(id);

        // place the file
        let mut dest = final_path.clone();
        if dest.exists() {
            if policy.as_deref() == Some("overwrite") || policy.as_deref() == Some("resume") {
                let _ = std::fs::remove_file(&dest);
            } else {
                let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
                let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                dest = util::unique_path(&dir, &name);
            }
        }
        if let Err(e) = std::fs::rename(part, &dest) {
            self.fail(id, rt, format!("could not move finished file into place: {e}"));
            return;
        }

        let mut checksum_ok = None;
        if let Some(c) = checksum {
            let algo = if c.algo.is_empty() { util::guess_algo(&c.expected).to_string() } else { c.algo.clone() };
            let p = dest.clone();
            let got = tokio::task::spawn_blocking(move || util::hash_file(&p, &algo)).await;
            checksum_ok = Some(matches!(got, Ok(Ok(ref h)) if h.eq_ignore_ascii_case(c.expected.trim())));
        }

        let (completed, queue_id) = {
            let mut st = self.inner.state.lock();
            let Some(e) = st.entries.get_mut(id) else { return };
            e.d.file_name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(e.d.file_name.clone());
            e.d.ext = util::badge_ext(&e.d.file_name);
            e.d.total_bytes = Some(total);
            e.d.received_bytes = total;
            e.d.segments = rt.views();
            for s in e.d.segments.iter_mut() {
                s.state = SegState::Done;
                s.speed_bps = 0;
            }
            e.d.completed_at = Some(now_iso());
            e.d.checksum_ok = checksum_ok;
            e.rt = None;
            e.ema = 0.0;
            if checksum_ok == Some(false) {
                e.d.status = Status::Error;
                e.d.error = Some("checksum mismatch — the file may be corrupt".into());
            } else {
                e.d.status = Status::Complete;
                e.d.error = None;
            }
            let q = e.d.queue_id.clone().filter(|_| !e.d.manual);
            e.d.manual = false;
            (e.d.status == Status::Complete, q)
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
            iv.tick().await;
            n += 1;
            let second = n % 4 == 0;
            let persist = n % 8 == 0;
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
                        e.d.segments = rt.views();
                        e.d.received_bytes = received;
                        to_save.push(e.d.clone());
                    }
                }
            }
            for id in status_changes {
                self.changed(&id);
            }
            if !batch.is_empty() {
                self.emit(Event::Progress(batch));
            }
            for d in to_save {
                self.inner.store.save_download(&d);
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
            iv.tick().await;
            self.scheduler_tick(Local::now().naive_local());
        }
    }

    /// One scheduler pass at local time `now` (public for tests).
    pub fn scheduler_tick(&self, now: chrono::NaiveDateTime) {
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

    /// Persist everything (call on exit).
    pub fn shutdown(&self) {
        let rts: Vec<(String, Arc<Runtime>)> = {
            let st = self.inner.state.lock();
            st.entries
                .values()
                .filter_map(|e| e.rt.clone().map(|r| (e.d.id.clone(), r)))
                .collect()
        };
        for (_, rt) in &rts {
            rt.cancel.cancel();
        }
        let st = self.inner.state.lock();
        for e in st.entries.values() {
            let mut d = e.d.clone();
            if let Some(rt) = &e.rt {
                d.segments = rt.views();
                d.received_bytes = rt.received();
                d.status = Status::Paused;
            }
            self.inner.store.save_download(&d);
        }
    }

    pub fn grab_client(&self) -> (reqwest::Client, Settings) {
        (self.client(), self.settings())
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
