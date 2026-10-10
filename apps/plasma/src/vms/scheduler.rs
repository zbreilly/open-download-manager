//! Queues & Scheduler: one queue at a time, with its schedule, behavior
//! and run order.
use super::act;
use crate::backend;
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_ui::store::{live_order, Store};
use odm_ui::{b, fmt, n, s};
use serde_json::{json, Value};

pub struct Scheduler {
    selected: String,
    draft: Option<Value>,
    dirty: bool,
    /// the user reordered the list: only then is the order sent
    reordered: bool,
    saving: bool,
    error: String,
}

fn time(q: &Value, field: &str) -> (u32, u32) {
    let (h, m) = s(q, field).split_once(':').unwrap_or(("0", "0"));
    (h.parse().unwrap_or(0), m.parse().unwrap_or(0))
}

impl Scheduler {
    pub fn new(store: &Store) -> Self {
        let mut vm = Scheduler { selected: "main".into(), draft: None, dirty: false, reordered: false, saving: false, error: String::new() };
        vm.pick("main", store);
        vm
    }

    /// Load a queue into the editor, dropping unsaved edits.
    fn pick(&mut self, id: &str, store: &Store) {
        self.selected = id.to_string();
        self.draft = store.queue(id).cloned();
        self.dirty = false;
        self.reordered = false;
        self.error.clear();
    }

    fn order(&self) -> Vec<String> {
        self.draft.as_ref().map(|q| odm_ui::strings(q, "order")).unwrap_or_default()
    }

    fn edit(&mut self, f: impl FnOnce(&mut Value)) {
        if let Some(q) = self.draft.as_mut() {
            f(q);
            self.dirty = true;
            self.error.clear();
        }
    }
}

impl Vm for Scheduler {
    fn state(&self, store: &Store) -> Value {
        let queues: Vec<Value> = store
            .queues
            .iter()
            .map(|q| {
                let icon = if b(q, "running") {
                    "media-playback-start-symbolic"
                } else if b(q, "builtin") {
                    "view-list-symbolic"
                } else {
                    "alarm-symbolic"
                };
                json!({ "id": s(q, "id"), "name": s(q, "name"), "icon": icon, "count": q.get("order").and_then(Value::as_array).map(Vec::len).unwrap_or(0) })
            })
            .collect();
        let editor = self.draft.as_ref().map(|q| {
            let order = self.order();
            let items: Vec<Value> = order
                .iter()
                .map(|id| match store.get(id) {
                    Some(d) => {
                        let r = store.row(d);
                        json!({ "name": r.name, "info": format!("{} · {}", r.status_label, r.size) })
                    }
                    None => json!({ "name": id, "info": "" }),
                })
                .collect();
            let (start_h, start_m) = time(q, "startTime");
            let (stop_h, stop_m) = time(q, "stopTime");
            let next = fmt::next_run(q);
            let (pending, left) = store.queue_remaining(&order);
            let days: Vec<bool> = (0..7).map(|i| q.get("days").and_then(|d| d.get(i)).and_then(Value::as_bool).unwrap_or(true)).collect();
            json!({
                "name": s(q, "name"),
                "builtin": b(q, "builtin"),
                "running": b(q, "running"),
                "startEnabled": b(q, "startEnabled"),
                "startHour": start_h,
                "startMinute": start_m,
                "stopEnabled": b(q, "stopEnabled"),
                "stopHour": stop_h,
                "stopMinute": stop_m,
                "days": days,
                "nextRun": next,
                "maxConcurrent": n(q, "maxConcurrent").clamp(1, 32),
                "retryFailed": n(q, "retryFailed").to_string(),
                "onFinish": s(q, "onFinish"),
                "countdownNote": matches!(s(q, "onFinish"), "sleep" | "shutdown"),
                "order": items,
                "summary": format!("Next run: {} · {pending} files · {}", next.as_deref().unwrap_or("not scheduled"), fmt::bytes(Some(left))),
            })
        });
        json!({
            "queues": queues,
            "selected": self.selected,
            "editor": editor,
            "canSave": self.dirty && !self.saving,
            "error": self.error,
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        match action {
            "pick" => self.pick(payload.as_str().unwrap_or("main"), store),
            "name" => {
                let name = payload.as_str().unwrap_or("").to_string();
                self.edit(|q| {
                    if !b(q, "builtin") {
                        q["name"] = name.into();
                    }
                });
            }
            // {field: "startEnabled" | "stopEnabled", value}
            "flag" => {
                let (field, on) = (s(payload, "field").to_string(), b(payload, "value"));
                if matches!(field.as_str(), "startEnabled" | "stopEnabled") {
                    self.edit(|q| q[field.as_str()] = on.into());
                }
            }
            // {field: "startTime" | "stopTime", hour, minute}
            "time" => {
                let field = s(payload, "field").to_string();
                let (h, m) = (n(payload, "hour").min(23), n(payload, "minute").min(59));
                if matches!(field.as_str(), "startTime" | "stopTime") {
                    self.edit(|q| q[field.as_str()] = format!("{h:02}:{m:02}").into());
                }
            }
            "day" => {
                let (i, on) = (n(payload, "index") as usize, b(payload, "value"));
                self.edit(|q| {
                    let mut days: Vec<bool> = (0..7).map(|i| q.get("days").and_then(|d| d.get(i)).and_then(Value::as_bool).unwrap_or(true)).collect();
                    if i < 7 {
                        days[i] = on;
                    }
                    q["days"] = days.into();
                });
            }
            "concurrent" => {
                let v = payload.as_u64().unwrap_or(1).clamp(1, 32);
                self.edit(|q| q["maxConcurrent"] = v.into());
            }
            "retry" => {
                let v = payload.as_str().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
                self.edit(|q| q["retryFailed"] = v.into());
            }
            "on-finish" => {
                let v = payload.as_str().unwrap_or("none").to_string();
                self.edit(|q| q["onFinish"] = v.into());
            }
            "move" => {
                let (from, to) = (n(payload, "from") as usize, n(payload, "to") as usize);
                let mut order = self.order();
                if from < order.len() && to < order.len() {
                    let item = order.remove(from);
                    order.insert(to, item);
                    self.edit(|q| q["order"] = order.into());
                    self.reordered = true;
                }
            }
            "start" => act("start_queue", json!({ "id": self.selected })),
            "stop" => act("stop_queue", json!({ "id": self.selected })),
            "revert" => {
                let id = self.selected.clone();
                self.pick(&id, store);
            }
            "save" => {
                let Some(mut q) = self.draft.clone() else { return };
                if self.saving {
                    return;
                }
                self.saving = true;
                self.error.clear();
                let wanted = self.reordered.then(|| self.order());
                let live: Vec<String> = store.queue(&self.selected).map(|q| odm_ui::strings(q, "order")).unwrap_or_default();
                // settings only: the engine keeps the queue's current order
                if let Some(o) = q.as_object_mut() {
                    o.remove("order");
                }
                cx.spawn(move || {
                    let saved = api::call("save_queue", json!({ "queue": q })).and_then(|saved| match wanted {
                        Some(wanted) => api::call("reorder_queue", json!({ "id": s(&saved, "id"), "order": live_order(&wanted, &live) })).map(|_| ()),
                        None => Ok(()),
                    });
                    ("saved".into(), saved.err().map(Value::from).unwrap_or(Value::Null))
                });
            }
            "saved" => {
                self.saving = false;
                match payload.as_str() {
                    // keep the edits (dirty) so the user can retry
                    Some(e) => self.error = format!("Could not save: {e}"),
                    None => {
                        self.dirty = false;
                        self.reordered = false;
                    }
                }
            }
            "delete" => {
                act("delete_queue", json!({ "id": self.selected }));
                self.pick("main", store);
            }
            "new" => {
                let name = payload.as_str().unwrap_or("").trim().to_string();
                if name.is_empty() {
                    return;
                }
                // no order: the engine starts its own
                let q = json!({
                    "id": "", "name": name, "builtin": false, "maxConcurrent": 2, "retryFailed": 3,
                    "startEnabled": false, "startTime": "01:00", "stopEnabled": false, "stopTime": "07:00",
                    "days": [true, true, true, true, true, true, true], "onFinish": "none", "running": false,
                });
                cx.spawn(move || match api::call("save_queue", json!({ "queue": q })) {
                    Ok(saved) => ("created".into(), json!({ "id": s(&saved, "id") })),
                    Err(e) => ("created".into(), json!({ "err": e })),
                });
            }
            "created" => match payload.get("err").and_then(Value::as_str) {
                Some(e) => backend::report(format!("Could not create the queue: {e}")),
                None => {
                    // shows up once the engine reports it
                    self.selected = s(payload, "id").to_string();
                    self.draft = store.queue(&self.selected).cloned();
                }
            },
            _ => {}
        }
    }

    fn store_changed(&mut self, kind: &str, store: &Store, _cx: &mut Cx) {
        if kind != "queues" {
            return;
        }
        // the engine reported the queues: follow what the user didn't edit
        let live = store.queue(&self.selected).cloned();
        match (self.draft.as_mut(), live) {
            (None, Some(_)) => {
                let id = self.selected.clone();
                self.pick(&id, store);
            }
            (Some(_), None) => self.pick("main", store),
            (Some(q), Some(live)) => {
                q["running"] = live.get("running").cloned().unwrap_or(false.into());
                if !self.reordered {
                    q["order"] = live.get("order").cloned().unwrap_or_else(|| json!([]));
                }
            }
            (None, None) => {}
        }
    }
}
