//! The main window: which list is showing, what is selected, the details
//! panel, the status bar, and every command of its toolbar and menus.
use super::act;
use crate::backend;
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_linux::shell;
use odm_ui::store::{SortKey, Store};
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};

/// A question the window is asking.
enum Dialog {
    None,
    Delete(Vec<String>),
    /// refresh a download's link: (id, text, note or error)
    Refresh(String, String, String),
    /// custom global limit: (text, note or error)
    Limit(String, String),
}

pub struct Main {
    filter: String,
    search: String,
    sort: String,
    descending: bool,
    selected: Vec<String>,
    /// where a Shift-click range starts
    anchor: Option<String>,
    details: bool,
    dialog: Dialog,
}

fn sort_key(name: &str) -> SortKey {
    match name {
        "name" => SortKey::Name,
        "size" => SortKey::Size,
        "progress" => SortKey::Progress,
        "speed" => SortKey::Speed,
        "eta" => SortKey::Eta,
        "status" => SortKey::Status,
        "queue" => SortKey::Queue,
        _ => SortKey::Added,
    }
}

/// Theme icon for a download's category.
pub fn file_icon(category: &str) -> &'static str {
    match category {
        "video" => "video-x-generic",
        "music" => "audio-x-generic",
        "documents" => "x-office-document",
        "compressed" => "application-x-archive",
        "programs" => "application-x-executable",
        _ => "text-x-generic",
    }
}

fn sidebar_icon(icon: &str) -> &'static str {
    match icon {
        "cat-video" => "folder-videos-symbolic",
        "cat-music" => "folder-music-symbolic",
        "cat-documents" => "folder-documents-symbolic",
        "cat-compressed" => "application-x-archive",
        "cat-programs" => "applications-other",
        "cat-images" => "folder-pictures-symbolic",
        "file-disc" => "media-optical",
        _ => "folder-symbolic",
    }
}

impl Main {
    pub fn new(_store: &Store) -> Self {
        Main {
            filter: "library:all".into(),
            search: String::new(),
            sort: "added".into(),
            descending: true,
            selected: vec![],
            anchor: None,
            details: true,
            dialog: Dialog::None,
        }
    }

    fn order(&self, store: &Store) -> Vec<String> {
        store.visible(&self.filter, &self.search, sort_key(&self.sort), self.descending).iter().map(|d| s(d, "id").to_string()).collect()
    }

    fn records(&self, store: &Store) -> Vec<Value> {
        self.selected.iter().filter_map(|id| store.get(id).cloned()).collect()
    }

    fn current_queue(&self) -> String {
        match self.filter.split_once(':') {
            Some(("queue", q)) => q.to_string(),
            _ => "main".into(),
        }
    }

    fn sidebar(&self, store: &Store) -> Vec<Value> {
        let counts = store.counts();
        let entry = |key: String, title: &str, icon: &str| json!({ "key": key, "title": title, "icon": icon, "count": counts.get(&key).copied().unwrap_or(0) });
        let mut items = vec![
            json!({ "header": "Library" }),
            entry("library:all".into(), "All Downloads", "folder-download-symbolic"),
            entry("library:active".into(), "Active", "media-playback-start-symbolic"),
            entry("library:unfinished".into(), "Unfinished", "content-loading-symbolic"),
            entry("library:finished".into(), "Finished", "emblem-ok-symbolic"),
            json!({ "header": "Categories" }),
        ];
        for c in store.categories() {
            items.push(entry(format!("category:{}", s(&c, "id")), s(&c, "name"), sidebar_icon(s(&c, "icon"))));
        }
        items.push(json!({ "header": "Queues" }));
        for q in &store.queues {
            let icon = if b(q, "running") {
                "media-playback-start-symbolic"
            } else if b(q, "builtin") {
                "view-list-symbolic"
            } else {
                "alarm-symbolic"
            };
            items.push(entry(format!("queue:{}", s(q, "id")), s(q, "name"), icon));
        }
        items
    }

    fn details(&self, store: &Store) -> Value {
        // the first selected download, else the first in the list
        let shown = self.selected.iter().find_map(|id| store.get(id)).or_else(|| self.order(store).first().and_then(|id| store.get(id)));
        let (true, Some(d)) = (self.details, shown) else { return Value::Null };
        let row = store.row(d);
        let status = row.status.as_str();
        let (label, action) = match status {
            "complete" => ("Open", "open"),
            "error" => ("Refresh Link…", "refresh"),
            "paused" => ("Resume", "resume"),
            _ => ("Pause", "pause"),
        };
        let history: Vec<u64> = d.get("speedHistory").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        json!({
            "id": row.id,
            "name": row.name,
            "url": s(d, "url"),
            "icon": file_icon(&row.category),
            "status": status,
            "statusLabel": row.status_label,
            "error": if status == "error" { d.get("error").cloned().unwrap_or(Value::Null) } else { Value::Null },
            "primary": { "label": label, "action": action },
            "retry": status == "error",
            "facts": [
                ["Downloaded", format!("{} of {}", fmt::bytes(Some(n(d, "receivedBytes"))), fmt::bytes(opt_n(d, "totalBytes")))],
                ["Speed", row.speed],
                ["Time left", row.eta],
                ["Connections", format!("{} active / {} max", n(d, "activeConnections"), n(d, "maxConnections"))],
                ["Resume", if b(d, "resumable") { "Supported (byte ranges)" } else { "Not supported by server" }],
                ["Save to", s(d, "saveDir")],
            ],
            "percent": row.percent,
            "segments": { "status": status, "totalBytes": d.get("totalBytes"), "segments": d.get("segments") },
            "speedHistory": history,
            "peak": fmt::speed(history.iter().copied().max().unwrap_or(0)),
        })
    }

    fn menu(&self, store: &Store) -> Value {
        let rows = self.records(store);
        let status = |d: &Value| s(d, "status").to_string();
        let one = rows.len() == 1;
        let first_complete = rows.first().map(|d| status(d) == "complete").unwrap_or(false);
        json!({
            "count": rows.len(),
            "one": one,
            "canOpen": one && first_complete,
            "canProgress": one && !first_complete,
            "canResume": rows.iter().any(|d| matches!(status(d).as_str(), "paused" | "error" | "queued")),
            "canPause": rows.iter().any(|d| fmt::is_active(&status(d)) || status(d) == "queued"),
            "extension": if one { rows.first().map(|d| fmt::extension(s(d, "fileName"))).unwrap_or_default() } else { String::new() },
            "queues": store.queues.iter().map(|q| json!({ "id": s(q, "id"), "name": s(q, "name") })).collect::<Vec<_>>(),
            "categories": store.categories().iter().map(|c| json!({ "id": s(c, "id"), "name": s(c, "name") })).collect::<Vec<_>>(),
        })
    }

    fn select(&mut self, id: &str, ctrl: bool, shift: bool, store: &Store) {
        if shift {
            let order = self.order(store);
            let from = self.anchor.as_ref().and_then(|a| order.iter().position(|x| x == a)).unwrap_or(0);
            if let Some(to) = order.iter().position(|x| x == id) {
                self.selected = order[from.min(to)..=from.max(to)].to_vec();
            }
        } else if ctrl {
            match self.selected.iter().position(|x| x == id) {
                Some(i) => {
                    self.selected.remove(i);
                }
                None => self.selected.push(id.to_string()),
            }
            self.anchor = Some(id.to_string());
        } else {
            self.selected = vec![id.to_string()];
            self.anchor = Some(id.to_string());
        }
    }

    fn command(&mut self, key: &str, store: &Store, cx: &mut Cx) {
        let ids = self.selected.clone();
        let first = ids.first().and_then(|id| store.get(id)).cloned();
        match key.split_once(':').unwrap_or((key, "")) {
            ("add", _) => cx.request("add", json!({})),
            ("add-batch", _) => cx.request("add", json!({ "batch": true })),
            ("resume", _) => act("resume", json!({ "ids": ids })),
            ("pause", _) => act("pause", json!({ "ids": ids })),
            ("pause-all", _) => act("pause_all", json!({})),
            ("delete", _) => {
                if ids.is_empty() {
                } else if store.setting("confirmDelete", true) {
                    self.dialog = Dialog::Delete(ids);
                } else {
                    act("delete", json!({ "ids": ids, "deleteFiles": false }));
                }
            }
            ("delete-completed", _) => act("delete_completed", json!({})),
            ("start-queue", _) => act("start_queue", json!({ "id": self.current_queue() })),
            ("stop-queue", _) => act("stop_queue", json!({ "id": self.current_queue() })),
            ("scheduler", _) => cx.request("scheduler", Value::Null),
            ("grabber", _) => cx.request("grabber", Value::Null),
            ("settings", _) => cx.request("settings", Value::Null),
            ("quit", _) => cx.request("quit", Value::Null),
            ("details", _) => self.details = !self.details,
            ("limit-toggle", _) => {
                if let Err(e) = shell::toggle_limiter() {
                    backend::report(e);
                }
            }
            ("limit-off", _) => act("set_global_limit", json!({ "bps": null })),
            ("limit-custom", _) => {
                self.dialog = Dialog::Limit(fmt::mbps(n(&store.settings, "globalLimitBps")), "Caps the combined speed of all downloads, in MB/s.".into());
            }
            ("limit", mb) => {
                if let Ok(mb) = mb.parse::<u64>() {
                    act("set_global_limit", json!({ "bps": mb * 1024 * 1024 }));
                }
            }
            ("open", _) => {
                if let Some(Err(e)) = first.as_ref().map(|d| odm_linux::open::open_path(&shell::download_path(d))) {
                    backend::report(e);
                }
            }
            ("reveal", _) => {
                if let Some(Err(e)) = first.as_ref().map(shell::reveal_download) {
                    backend::report(e);
                }
            }
            ("progress", _) => {
                if let Some(id) = ids.first() {
                    cx.request("progress", json!(id));
                }
            }
            ("redownload", _) => {
                if let Some(id) = ids.first() {
                    act("redownload", json!({ "id": id }));
                }
            }
            ("refresh", _) => {
                if let Some(d) = &first {
                    let note = "Paste a fresh address for the same file. Progress is kept when the server reports the same size.";
                    self.dialog = Dialog::Refresh(s(d, "id").into(), s(d, "url").into(), note.into());
                }
            }
            ("copy", _) => {
                if let Some(d) = &first {
                    cx.request("copy", json!(s(d, "url")));
                }
            }
            ("queue", q) => {
                let queue = if q.is_empty() { Value::Null } else { q.into() };
                act("move_to_queue", json!({ "ids": ids, "queueId": queue }));
            }
            ("category", c) => act("set_category", json!({ "ids": ids, "category": c })),
            ("always", c) => {
                if let Some(d) = &first {
                    let ext = fmt::extension(s(d, "fileName"));
                    act("assign_extensions", json!({ "category": c, "extensions": [ext] }));
                    let same = store.same_extension(&ext, c);
                    if !same.is_empty() {
                        act("set_category", json!({ "ids": same, "category": c }));
                    }
                }
            }
            ("cancel-power", _) => backend::cancel_power(),
            _ => {}
        }
    }
}

impl Vm for Main {
    fn state(&self, store: &Store) -> Value {
        let t = &store.totals;
        let ext = &store.ext;
        let active = n(t, "active");
        let bridge_error = ext.get("bridgeError").and_then(Value::as_str);
        let browsers = odm_ui::strings(ext, "browsers");
        let (ext_state, ext_text) = if bridge_error.is_some() {
            ("error", "Browser bridge offline: port in use".to_string())
        } else if b(ext, "connected") {
            ("complete", format!("Browser extension connected{}", if browsers.is_empty() { String::new() } else { format!(" ({})", browsers.join(", ")) }))
        } else {
            ("idle", "Browser extension not detected".to_string())
        };
        let limit_on = b(&store.settings, "globalLimitEnabled");
        let shown = self.order(store).len();
        let empty = if shown > 0 {
            ""
        } else if store.downloads().is_empty() {
            "No downloads yet"
        } else if self.search.is_empty() {
            "No downloads here"
        } else {
            "No downloads match your search"
        };
        let dialog = match &self.dialog {
            Dialog::None => Value::Null,
            Dialog::Delete(ids) => json!({
                "kind": "delete",
                "title": if ids.len() == 1 { "Delete this download?".to_string() } else { format!("Delete {} downloads?", ids.len()) },
                "text": "Unfinished parts are always removed. Finished files stay on disk unless you also delete files.",
            }),
            Dialog::Refresh(_, value, note) => json!({ "kind": "text", "title": "Refresh Download Link", "text": note, "value": value, "accept": "Update Link" }),
            Dialog::Limit(value, note) => json!({ "kind": "text", "title": "Global Speed Limit", "text": note, "value": value, "accept": "Turn On Limit" }),
        };
        let power = backend::power().map(|(action, queue, left)| {
            let verb = match action.as_str() {
                "sleep" => "Sleeping",
                "shutdown" => "Shutting down",
                _ => "Quitting",
            };
            json!(format!("“{queue}” finished. {verb} in {left}s."))
        });
        json!({
            "filter": self.filter,
            "search": self.search,
            "sort": self.sort,
            "descending": self.descending,
            "selected": self.selected,
            "sidebar": self.sidebar(store),
            "title": if active > 0 { format!("{} — {active} active · ↓ {}", odm_ui::APP_NAME, fmt::speed(n(t, "speedBps"))) } else { odm_ui::APP_NAME.to_string() },
            "menu": self.menu(store),
            "details": self.details(store),
            "detailsOn": self.details,
            "limitOn": limit_on,
            "limitLabel": if limit_on { format!("{} MB/s", fmt::mbps(n(&store.settings, "globalLimitBps"))) } else { "No Limit".to_string() },
            "status": {
                "summary": fmt::totals_summary(t),
                "speed": format!("↓ {}", fmt::speed(n(t, "speedBps"))),
                "limit": if b(t, "limitOn") { format!("Limit {} MB/s", fmt::mbps(n(t, "limitBps"))) } else { "No speed limit".to_string() },
                "ext": { "state": ext_state, "text": ext_text, "tooltip": bridge_error.unwrap_or("") },
                "free": opt_n(t, "freeBytes").map(|f| format!("{} free", fmt::bytes(Some(f)))).unwrap_or_default(),
            },
            "empty": empty,
            "emptyHint": store.downloads().is_empty(),
            "dialog": dialog,
            "power": power,
            "startError": backend::start_error(),
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        match action {
            "nav" => self.filter = payload.as_str().unwrap_or("library:all").to_string(),
            "search" => self.search = payload.as_str().unwrap_or("").to_string(),
            "sort" => {
                let key = payload.as_str().unwrap_or("added");
                self.descending = self.sort == key && !self.descending;
                self.sort = key.to_string();
            }
            "select" => {
                let id = s(payload, "id").to_string();
                self.select(&id, b(payload, "ctrl"), b(payload, "shift"), store);
            }
            // a right-click: the menu acts on the selection, so a row
            // outside it becomes it
            "context" => {
                let id = payload.as_str().unwrap_or("").to_string();
                if !self.selected.contains(&id) {
                    self.selected = vec![id.clone()];
                    self.anchor = Some(id);
                }
            }
            "activate" => {
                let id = payload.as_str().unwrap_or("");
                if let Some(d) = store.get(id) {
                    if s(d, "status") == "complete" {
                        if let Err(e) = odm_linux::open::open_path(&shell::download_path(d)) {
                            backend::report(e);
                        }
                    } else {
                        cx.request("progress", json!(id));
                    }
                }
            }
            "command" => {
                let key = payload.as_str().unwrap_or("").to_string();
                // the details panel's buttons name their download
                if let Some((command, id)) = key.strip_prefix("detail-").and_then(|k| k.split_once(':')) {
                    self.selected = vec![id.to_string()];
                    return self.command(command, store, cx);
                }
                self.command(&key, store, cx);
            }
            "dialog-text" => {
                if let Dialog::Refresh(_, value, _) | Dialog::Limit(value, _) = &mut self.dialog {
                    *value = payload.as_str().unwrap_or("").to_string();
                }
            }
            "dialog-cancel" => self.dialog = Dialog::None,
            // "list" or "files" for the delete question, anything for the others
            "dialog-accept" => match std::mem::replace(&mut self.dialog, Dialog::None) {
                Dialog::Delete(ids) => act("delete", json!({ "ids": ids, "deleteFiles": payload.as_str() == Some("files") })),
                Dialog::Refresh(id, url, note) => {
                    // stays open (with the error) until the engine accepts it
                    self.dialog = Dialog::Refresh(id.clone(), url.clone(), note);
                    cx.spawn(move || match api::call("refresh_link", json!({ "id": id, "newUrl": url.trim() })) {
                        Ok(_) => ("refreshed".into(), Value::Null),
                        Err(e) => ("refreshed".into(), json!(e)),
                    });
                }
                Dialog::Limit(text, _) => match fmt::limit_bps(&text) {
                    Some(bps) => act("set_global_limit", json!({ "bps": bps })),
                    // reject NaN / inf / 0 instead of silently doing nothing
                    None => self.dialog = Dialog::Limit(text, fmt::LIMIT_ERROR.into()),
                },
                Dialog::None => {}
            },
            "refreshed" => {
                if let Dialog::Refresh(_, _, note) = &mut self.dialog {
                    match payload.as_str() {
                        Some(e) => *note = e.to_string(),
                        None => self.dialog = Dialog::None,
                    }
                }
            }
            // links dropped on the window, or imported from a file
            "links" => {
                let links: Vec<&str> = payload.as_str().unwrap_or("").lines().map(str::trim).filter(|l| fmt::is_http(l)).collect();
                match links.as_slice() {
                    [] => {}
                    [one] => cx.request("add", json!({ "url": one })),
                    many => cx.request("grabber", json!({ "links": many.iter().map(|u| json!({ "url": u })).collect::<Vec<_>>() })),
                }
            }
            "import" => {
                let path = payload.as_str().unwrap_or("").to_string();
                cx.spawn(move || match api::call("read_url_list", json!({ "path": path })) {
                    Ok(v) => ("imported".into(), v),
                    Err(e) => ("failed".into(), json!(e)),
                });
            }
            "imported" => {
                let urls = odm_ui::strings(&json!({ "u": payload }), "u");
                if urls.is_empty() {
                    backend::report("No http(s) links found in that file.".into());
                } else {
                    cx.request("grabber", json!({ "links": urls.iter().map(|u| json!({ "url": u })).collect::<Vec<_>>() }));
                }
            }
            "export" => act("export_list", json!({ "path": payload.as_str().unwrap_or(""), "onlyUnfinished": false })),
            "failed" => backend::report(payload.as_str().unwrap_or("").to_string()),
            // once a second, while a power countdown runs
            "tick" => {
                if let Some(action) = backend::power_due() {
                    if action == "exit" {
                        cx.request("quit", Value::Null);
                    } else if let Err(e) = backend::run_power(&action) {
                        backend::report(e);
                    }
                }
            }
            _ => {}
        }
    }

    fn store_changed(&mut self, _kind: &str, store: &Store, _cx: &mut Cx) {
        // downloads that are gone can't stay selected
        self.selected.retain(|id| store.get(id).is_some());
    }
}
