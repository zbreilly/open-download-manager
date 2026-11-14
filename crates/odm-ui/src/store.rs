//! The download list and everything else the engine reports, kept up to
//! date from its events. One per app, on the GUI thread.
use crate::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long the tray shows "complete" after a download finishes.
const COMPLETE_FLASH: Duration = Duration::from_secs(5);

/// What an event changed, so views refresh only what they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// list membership, or a row's status, name, category or queue
    List,
    /// progress of some rows
    Progress,
    Queues,
    Settings,
    Totals,
    /// browser extension / bridge status
    Ext,
    /// nothing a view shows
    None,
}

/// Something the front end has to do in response to an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// a download finished (the record): notify, open the file, …
    Completed(Value),
    Failed(Value),
    /// a queue ran out of files (the queue)
    QueueFinished(Value),
    /// open the Add window with these params
    ShowAdd(Value),
    /// open the grabber with this list of links
    ShowLinks(Value),
}

/// Sort column of the download list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Progress,
    Speed,
    Eta,
    Status,
    Queue,
    Added,
}

impl SortKey {
    /// Columns whose values move with every progress tick.
    pub fn is_live(self) -> bool {
        matches!(self, SortKey::Size | SortKey::Progress | SortKey::Speed | SortKey::Eta)
    }
}

/// One download as the list shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub size: String,
    /// 0..=100
    pub percent: u32,
    pub speed: String,
    pub eta: String,
    /// label, e.g. "Downloading" or "Starts 01:00"
    pub status_label: String,
    /// the engine's value, e.g. "downloading"
    pub status: String,
    pub queue: String,
    pub added: String,
    pub category: String,
}

pub struct Store {
    /// every download, in the engine's order
    downloads: Vec<Value>,
    index: HashMap<String, usize>,
    pub queues: Vec<Value>,
    pub settings: Value,
    pub totals: Value,
    /// `{connected, browsers, port, bridgeError}`
    pub ext: Value,
    last_complete: Option<Instant>,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            downloads: vec![],
            index: HashMap::new(),
            queues: vec![],
            settings: Value::Null,
            totals: Value::Null,
            ext: json!({ "connected": false, "browsers": [] }),
            last_complete: None,
        }
    }
}

/// Does a download belong under a sidebar entry ("library:active",
/// "category:video", "queue:main", …)?
pub fn matches(d: &Value, filter: &str) -> bool {
    let status = s(d, "status");
    match filter.split_once(':') {
        Some(("library", "active")) => fmt::is_active(status),
        Some(("library", "unfinished")) => status != "complete",
        Some(("library", "finished")) => status == "complete",
        Some(("category", c)) => s(d, "category") == c,
        Some(("queue", q)) => d.get("queueId").and_then(Value::as_str) == Some(q),
        _ => true,
    }
}

impl Store {
    /// Fill the store from the engine. `call(method)` returns its result.
    pub fn load(&mut self, call: impl Fn(&str) -> Option<Value>) {
        if let Some(Value::Array(q)) = call("list_queues") {
            self.queues = q;
        }
        if let Some(Value::Array(list)) = call("list_downloads") {
            self.downloads = list;
            self.reindex();
        }
        if let Some(v) = call("get_settings") {
            self.settings = v;
        }
        if let Some(v) = call("totals") {
            self.totals = v;
        }
        if let Some(v) = call("extension_status") {
            self.ext = v;
        }
    }

    fn reindex(&mut self) {
        self.index = self.downloads.iter().enumerate().map(|(i, d)| (s(d, "id").to_string(), i)).collect();
    }

    pub fn downloads(&self) -> &[Value] {
        &self.downloads
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.index.get(id).and_then(|i| self.downloads.get(*i))
    }

    pub fn queue(&self, id: &str) -> Option<&Value> {
        self.queues.iter().find(|q| s(q, "id") == id)
    }

    pub fn queue_name(&self, id: Option<&str>) -> String {
        id.and_then(|id| self.queue(id)).map(|q| s(q, "name").to_string()).unwrap_or_else(|| fmt::DASH.into())
    }

    pub fn categories(&self) -> Vec<Value> {
        self.settings.get("categories").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    pub fn category_name(&self, id: &str) -> String {
        self.categories().iter().find(|c| s(c, "id") == id).map(|c| s(c, "name").to_string()).unwrap_or_else(|| "this category".into())
    }

    pub fn setting(&self, key: &str, default: bool) -> bool {
        crate::b_or(&self.settings, key, default)
    }

    fn upsert(&mut self, mut d: Value) {
        let id = s(&d, "id").to_string();
        match self.index.get(&id).copied() {
            Some(i) => {
                // keep the speed history collected here when the record has none
                let empty = d.get("speedHistory").and_then(Value::as_array).map(Vec::is_empty).unwrap_or(true);
                if empty {
                    if let Some(h) = self.downloads[i].get("speedHistory").cloned() {
                        d["speedHistory"] = h;
                    }
                }
                self.downloads[i] = d;
            }
            None => {
                self.index.insert(id, self.downloads.len());
                self.downloads.push(d);
            }
        }
    }

    /// Apply an engine event. Returns what changed and what, if anything,
    /// the front end has to do about it.
    pub fn handle(&mut self, name: &str, v: Value) -> (Change, Option<Effect>) {
        match name {
            "progress_batch" => {
                let mut status_changed = false;
                for p in v.as_array().into_iter().flatten() {
                    let Some(i) = self.index.get(s(p, "id")).copied() else { continue };
                    let d = &mut self.downloads[i];
                    status_changed |= d.get("status") != p.get("status");
                    for k in ["receivedBytes", "speedBps", "etaSecs", "activeConnections", "segments", "status"] {
                        if let Some(x) = p.get(k) {
                            d[k] = x.clone();
                        }
                    }
                    if let Some(t) = p.get("totalBytes").filter(|t| !t.is_null()) {
                        d["totalBytes"] = t.clone();
                    }
                }
                (if status_changed { Change::List } else { Change::Progress }, None)
            }
            "download_added" | "download_changed" => {
                self.upsert(v);
                (Change::List, None)
            }
            "download_removed" => {
                let Some(i) = v.as_str().and_then(|id| self.index.get(id).copied()) else { return (Change::None, None) };
                self.downloads.remove(i);
                self.reindex();
                (Change::List, None)
            }
            "totals" => {
                self.totals = v;
                // one more sample for each 60-second speed graph
                for d in self.downloads.iter_mut().filter(|d| matches!(s(d, "status"), "downloading" | "connecting")) {
                    let speed = n(d, "speedBps");
                    if !d.get("speedHistory").map(Value::is_array).unwrap_or(false) {
                        d["speedHistory"] = json!([]);
                    }
                    if let Some(h) = d["speedHistory"].as_array_mut() {
                        h.push(speed.into());
                        if h.len() > 60 {
                            let extra = h.len() - 60;
                            h.drain(..extra);
                        }
                    }
                }
                (Change::Totals, None)
            }
            "queues_changed" => {
                if let Value::Array(q) = v {
                    self.queues = q;
                }
                (Change::Queues, None)
            }
            "settings_changed" => {
                self.settings = v;
                (Change::Settings, None)
            }
            "extension_status" => {
                // the event carries connection state only; keep the rest
                let mut ext = v;
                for k in ["bridgeError", "port"] {
                    ext[k] = self.ext.get(k).cloned().unwrap_or(Value::Null);
                }
                if self.ext == ext {
                    return (Change::None, None);
                }
                self.ext = ext;
                (Change::Ext, None)
            }
            "bridge_error" => {
                self.ext["bridgeError"] = v.get("error").cloned().unwrap_or(Value::Null);
                (Change::Ext, None)
            }
            "download_completed" => {
                self.last_complete = Some(Instant::now());
                (Change::Totals, Some(Effect::Completed(v)))
            }
            "download_failed" => (Change::None, Some(Effect::Failed(v))),
            "queue_finished" => (Change::None, Some(Effect::QueueFinished(v))),
            "show_add" => (Change::None, Some(Effect::ShowAdd(v))),
            "show_links" => (Change::None, Some(Effect::ShowLinks(v))),
            _ => (Change::None, None),
        }
    }

    /// The row a list shows for a download.
    pub fn row(&self, d: &Value) -> Row {
        let status = s(d, "status");
        let complete = status == "complete";
        Row {
            id: s(d, "id").into(),
            name: s(d, "fileName").into(),
            size: fmt::bytes(opt_n(d, "totalBytes")),
            percent: fmt::percent(n(d, "receivedBytes"), opt_n(d, "totalBytes"), complete),
            speed: if fmt::is_active(status) { fmt::speed(n(d, "speedBps")) } else { fmt::DASH.into() },
            eta: if status == "downloading" { fmt::eta(opt_n(d, "etaSecs")) } else { fmt::DASH.into() },
            status_label: fmt::status_label(status, d.get("scheduledAt").and_then(Value::as_str)),
            status: status.into(),
            queue: self.queue_name(d.get("queueId").and_then(Value::as_str)),
            added: fmt::added(s(d, "addedAt")),
            category: s(d, "category").into(),
        }
    }

    /// The downloads under a sidebar entry that match the search text,
    /// sorted. `search` is matched against name, address and folder.
    pub fn visible(&self, filter: &str, search: &str, key: SortKey, descending: bool) -> Vec<&Value> {
        let q = search.trim().to_lowercase();
        let mut rows: Vec<&Value> = self
            .downloads
            .iter()
            .filter(|d| matches(d, filter))
            .filter(|d| q.is_empty() || ["fileName", "url", "saveDir"].iter().any(|k| s(d, k).to_lowercase().contains(&q)))
            .collect();
        let percent = |d: &Value| fmt::percent(n(d, "receivedBytes"), opt_n(d, "totalBytes"), s(d, "status") == "complete");
        let speed = |d: &Value| if fmt::is_active(s(d, "status")) { n(d, "speedBps") } else { 0 };
        let eta = |d: &Value| if s(d, "status") == "downloading" { opt_n(d, "etaSecs").unwrap_or(u64::MAX) } else { u64::MAX };
        rows.sort_by(|a, b| {
            let order = match key {
                SortKey::Name => s(a, "fileName").to_lowercase().cmp(&s(b, "fileName").to_lowercase()),
                SortKey::Size => n(a, "totalBytes").cmp(&n(b, "totalBytes")),
                SortKey::Progress => percent(a).cmp(&percent(b)),
                SortKey::Speed => speed(a).cmp(&speed(b)),
                SortKey::Eta => eta(a).cmp(&eta(b)),
                SortKey::Status => s(a, "status").cmp(s(b, "status")),
                SortKey::Queue => s(a, "queueId").cmp(s(b, "queueId")),
                SortKey::Added => s(a, "addedAt").cmp(s(b, "addedAt")),
            };
            if descending {
                order.reverse()
            } else {
                order
            }
        });
        rows
    }

    /// How many downloads each sidebar entry holds, by its key.
    pub fn counts(&self) -> HashMap<String, usize> {
        let mut c: HashMap<String, usize> = HashMap::new();
        for d in &self.downloads {
            let status = s(d, "status");
            *c.entry("library:all".into()).or_default() += 1;
            if fmt::is_active(status) {
                *c.entry("library:active".into()).or_default() += 1;
            }
            *c.entry(if status == "complete" { "library:finished" } else { "library:unfinished" }.into()).or_default() += 1;
            *c.entry(format!("category:{}", s(d, "category"))).or_default() += 1;
            if let Some(q) = d.get("queueId").and_then(Value::as_str) {
                *c.entry(format!("queue:{q}")).or_default() += 1;
            }
        }
        c
    }

    /// Ids of the other listed downloads with this file extension that are
    /// not in `category` yet (to move along when the type is reassigned).
    pub fn same_extension(&self, ext: &str, category: &str) -> Vec<String> {
        self.downloads
            .iter()
            .filter(|d| fmt::extension(s(d, "fileName")) == ext && s(d, "category") != category)
            .map(|d| s(d, "id").to_string())
            .collect()
    }

    /// Tray icon state, by the brand priority rules: "error",
    /// "limited", "downloading", "complete", "paused" or "idle".
    pub fn tray_state(&self) -> &'static str {
        let recent = self.last_complete.map(|t| t.elapsed() < COMPLETE_FLASH).unwrap_or(false);
        let active = n(&self.totals, "active");
        if n(&self.totals, "failed") > 0 && active == 0 && !recent {
            "error"
        } else if active > 0 && b(&self.totals, "limitOn") {
            "limited"
        } else if active > 0 {
            "downloading"
        } else if recent {
            "complete"
        } else if n(&self.totals, "paused") > 0 {
            "paused"
        } else {
            "idle"
        }
    }

    /// "2 active · 4.1 MB/s" or "No active downloads".
    pub fn activity_line(&self) -> String {
        let active = n(&self.totals, "active");
        if active > 0 {
            format!("{active} active · {}", fmt::speed(n(&self.totals, "speedBps")))
        } else {
            "No active downloads".into()
        }
    }

    /// What a queue still has to download: (files, bytes left).
    pub fn queue_remaining(&self, order: &[String]) -> (usize, u64) {
        let pending: Vec<&Value> = order.iter().filter_map(|id| self.get(id)).filter(|d| s(d, "status") != "complete").collect();
        let left = pending.iter().map(|d| opt_n(d, "totalBytes").unwrap_or(0).saturating_sub(n(d, "receivedBytes"))).sum();
        (pending.len(), left)
    }
}

/// The user's arrangement of a queue applied to its live membership:
/// downloads that left the queue are dropped, ones that joined since go to
/// the end.
pub fn live_order(wanted: &[String], live: &[String]) -> Vec<String> {
    let mut out: Vec<String> = wanted.iter().filter(|w| live.contains(w)).cloned().collect();
    for l in live {
        if !out.contains(l) {
            out.push(l.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &str, status: &str, name: &str) -> Value {
        json!({ "id": id, "status": status, "fileName": name, "url": format!("https://x.example/{name}"), "saveDir": "/tmp",
                "category": "general", "queueId": null, "receivedBytes": 0, "totalBytes": 100, "addedAt": id })
    }

    fn store() -> Store {
        let mut st = Store::default();
        st.handle("download_added", d("1", "downloading", "b.zip"));
        st.handle("download_added", d("2", "complete", "a.zip"));
        st.handle("download_added", d("3", "paused", "c.iso"));
        st
    }

    #[test]
    fn events_keep_the_list() {
        let mut st = store();
        assert_eq!(st.downloads().len(), 3);
        let (change, _) = st.handle("progress_batch", json!([{ "id": "1", "status": "downloading", "receivedBytes": 50, "speedBps": 10 }]));
        assert_eq!(change, Change::Progress);
        assert_eq!(st.row(st.get("1").unwrap()).percent, 50);
        let (change, _) = st.handle("progress_batch", json!([{ "id": "1", "status": "paused", "receivedBytes": 60 }]));
        assert_eq!(change, Change::List);
        st.handle("download_removed", json!("2"));
        assert!(st.get("2").is_none());
        assert_eq!(st.get("3").map(|d| s(d, "fileName")), Some("c.iso"));
    }

    #[test]
    fn filters_sorts_and_counts() {
        let st = store();
        let names = |f: &str, q: &str, k, desc| st.visible(f, q, k, desc).iter().map(|d| s(d, "fileName").to_string()).collect::<Vec<_>>();
        assert_eq!(names("library:all", "", SortKey::Name, false), ["a.zip", "b.zip", "c.iso"]);
        assert_eq!(names("library:all", "", SortKey::Added, true), ["c.iso", "a.zip", "b.zip"]);
        assert_eq!(names("library:unfinished", "", SortKey::Name, false), ["b.zip", "c.iso"]);
        assert_eq!(names("library:all", "ZIP", SortKey::Name, false), ["a.zip", "b.zip"]);
        let c = st.counts();
        assert_eq!(c["library:all"], 3);
        assert_eq!(c["library:active"], 1);
        assert_eq!(c["library:finished"], 1);
    }

    #[test]
    fn speed_history_is_kept() {
        let mut st = store();
        st.handle("progress_batch", json!([{ "id": "1", "status": "downloading", "speedBps": 7 }]));
        st.handle("totals", json!({ "active": 1 }));
        st.handle("download_changed", d("1", "downloading", "b.zip"));
        assert_eq!(st.get("1").unwrap()["speedHistory"], json!([7]));
    }

    #[test]
    fn reorders_against_live_membership() {
        let w = ["c".to_string(), "a".into(), "gone".into()];
        let l = ["a".to_string(), "b".into(), "c".into()];
        assert_eq!(live_order(&w, &l), ["c", "a", "b"]);
    }
}
