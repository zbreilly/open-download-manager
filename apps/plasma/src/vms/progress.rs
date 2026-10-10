//! Per-download progress window.
use super::act;
use super::main::file_icon;
use crate::backend;
use crate::vm::{Cx, Vm};
use odm_linux::shell;
use odm_ui::store::Store;
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::time::Duration;

pub struct Progress {
    id: String,
    limit_on: bool,
    limit_text: String,
    limit_error: bool,
    /// it was unfinished while this window was open
    saw_incomplete: bool,
    /// seconds until the window closes by itself
    closing: Option<u64>,
    hovering: bool,
}

fn state_label(state: &str) -> &'static str {
    match state {
        "receiving" => "Receiving",
        "reconnecting" => "Reconnecting",
        "done" => "Done",
        "connecting" => "Connecting",
        _ => "Idle",
    }
}

impl Progress {
    pub fn new(params: &Value, store: &Store) -> Self {
        let id = params.as_str().unwrap_or("").to_string();
        let record = store.get(&id).cloned().unwrap_or(Value::Null);
        let limit = opt_n(&record, "speedLimitBps").filter(|l| *l > 0);
        Progress {
            limit_on: limit.is_some(),
            limit_text: limit.map(fmt::mbps).unwrap_or_else(|| "5".into()),
            limit_error: false,
            saw_incomplete: s(&record, "status") != "complete",
            closing: None,
            hovering: false,
            id,
        }
    }

    fn apply_limit(&mut self) {
        self.limit_error = false;
        if !self.limit_on {
            return act("set_download_limit", json!({ "id": self.id, "bps": null }));
        }
        // reject NaN / inf / 0 instead of silently turning the limit off
        match fmt::limit_bps(&self.limit_text) {
            Some(bps) => act("set_download_limit", json!({ "id": self.id, "bps": bps })),
            None => self.limit_error = true,
        }
    }
}

impl Vm for Progress {
    fn state(&self, store: &Store) -> Value {
        let Some(d) = store.get(&self.id) else { return json!({ "exists": false, "title": "Download" }) };
        let row = store.row(d);
        let status = row.status.as_str();
        let complete = status == "complete";
        let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        segs.sort_by_key(|x| n(x, "start"));
        let connections: Vec<Value> = segs
            .iter()
            .enumerate()
            .map(|(i, seg)| {
                let state = s(seg, "state");
                let mut received = fmt::bytes(Some(n(seg, "received")));
                if n(seg, "end") < u64::MAX / 2 {
                    received = format!("{received} / {}", fmt::bytes(Some(n(seg, "end").saturating_sub(n(seg, "start")))));
                }
                json!({
                    "number": format!("{:02}", i + 1),
                    "received": received,
                    "state": state_label(state),
                    "speed": if state == "receiving" { fmt::speed(n(seg, "speedBps")) } else { fmt::DASH.to_string() },
                })
            })
            .collect();
        let history: Vec<u64> = d.get("speedHistory").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        json!({
            "exists": true,
            "title": format!("{}% {}", row.percent, row.name),
            "name": row.name,
            "icon": file_icon(&row.category),
            "place": format!("{} · {}", fmt::host(d.get("finalUrl").and_then(Value::as_str).unwrap_or(s(d, "url"))), s(d, "saveDir")),
            "status": status,
            "statusLabel": row.status_label,
            "percentText": if complete { "✓ Complete".to_string() } else { format!("{}%", row.percent) },
            "stats": [
                ["DOWNLOADED", format!("{} / {}", fmt::bytes(Some(n(d, "receivedBytes"))), fmt::bytes(opt_n(d, "totalBytes")))],
                ["SPEED", row.speed],
                ["TIME LEFT", row.eta],
                ["RESUME", if b(d, "resumable") { "Supported" } else { "Not supported" }],
            ],
            "error": if status == "error" { d.get("error").cloned().unwrap_or(Value::Null) } else { Value::Null },
            "verified": d.get("checksumOk").and_then(Value::as_bool) == Some(true),
            "segmentsTitle": format!("Segment map · {} connections", (n(d, "activeConnections") as usize).max(segs.len())),
            "segments": { "status": status, "totalBytes": d.get("totalBytes"), "segments": d.get("segments") },
            "connections": connections,
            "speedHistory": history,
            "peak": fmt::speed(history.iter().copied().max().unwrap_or(0)),
            "limitOn": self.limit_on,
            "limitText": self.limit_text,
            "limitError": if self.limit_error { fmt::LIMIT_ERROR } else { "" },
            "whenFinished": s(d, "whenFinished"),
            "complete": complete,
            "toggleLabel": if fmt::is_active(status) || status == "queued" { "Pause" } else if status == "error" { "Retry" } else { "Resume" },
            "closing": self.closing.map(|left| format!("Closing in {left}s")),
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        let record = store.get(&self.id).cloned();
        match action {
            "limit-on" => {
                self.limit_on = payload.as_bool().unwrap_or(false);
                self.apply_limit();
            }
            "limit-text" => self.limit_text = payload.as_str().unwrap_or("").to_string(),
            "apply-limit" => self.apply_limit(),
            "when-finished" => act("update_download", json!({ "id": self.id, "patch": { "whenFinished": payload } })),
            "toggle" => {
                let status = record.as_ref().map(|d| s(d, "status").to_string()).unwrap_or_default();
                if fmt::is_active(&status) || status == "queued" {
                    act("pause", json!({ "ids": [self.id] }));
                } else {
                    act("resume", json!({ "ids": [self.id] }));
                }
            }
            // after the window asked "Cancel this download?"
            "cancel" => {
                act("delete", json!({ "ids": [self.id], "deleteFiles": false }));
                cx.close();
            }
            "reveal" => {
                if let Some(Err(e)) = record.as_ref().map(shell::reveal_download) {
                    backend::report(e);
                }
            }
            "open" => {
                if let Some(Err(e)) = record.as_ref().map(|d| odm_linux::open::open_path(&shell::download_path(d))) {
                    backend::report(e);
                }
                cx.close();
            }
            "hide" => cx.close(),
            "hover" => {
                self.hovering = payload.as_bool().unwrap_or(false);
                if self.hovering {
                    // looking at it: don't close it under the pointer
                    self.closing = None;
                }
            }
            "closing" => {
                if self.closing.is_none() {
                    return;
                }
                match payload.as_u64().unwrap_or(0) {
                    0 => {
                        self.closing = None;
                        if !self.hovering {
                            cx.close();
                        }
                    }
                    left => {
                        self.closing = Some(left);
                        cx.later(Duration::from_secs(1), "closing", json!(left - 1));
                    }
                }
            }
            _ => {}
        }
    }

    fn store_changed(&mut self, _kind: &str, store: &Store, cx: &mut Cx) {
        let complete = store.get(&self.id).map(|d| s(d, "status") == "complete").unwrap_or(false);
        if !complete {
            self.saw_incomplete = true;
            self.closing = None;
        } else if std::mem::replace(&mut self.saw_incomplete, false) {
            // finished while open: close by itself in a moment
            self.closing = Some(5);
            cx.later(Duration::from_secs(1), "closing", json!(4));
        }
    }
}
