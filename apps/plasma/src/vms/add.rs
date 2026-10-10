//! The Add Download window. The form's rules are `odm_ui::add`'s; this
//! runs the probe and the batch expansion around it and adds the result.
use super::folder_for;
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_ui::add::{AddForm, Info, CONNECTIONS};
use odm_ui::store::Store;
use odm_ui::{b, n, s};
use serde_json::{json, Value};
use std::time::Duration;

pub struct Add {
    form: AddForm,
    recents: Vec<String>,
    /// bumped to drop the result of an older probe
    generation: u64,
    busy: bool,
    error: String,
    /// asking what to do about an existing file; holds "start"
    exists: Option<bool>,
}

impl Add {
    pub fn new(params: &Value, store: &Store, clipboard: &str, cx: &mut Cx) -> Self {
        let general = folder_for("general").unwrap_or_default();
        let mut form = AddForm::new(params, n(&store.settings, "defaultConnections"), &general);
        // offer a link that is on the clipboard
        let clip = clipboard.trim();
        if form.url.is_empty() && !form.batch && odm_ui::fmt::is_http(clip) && !clip.chars().any(char::is_whitespace) {
            form.url = clip.to_string();
        }
        let recents = api::call("recent_folders", json!({ "limit": 6 })).map(|v| odm_ui::strings(&json!({ "r": v }), "r")).unwrap_or_default();
        let mut add = Add { form, recents, generation: 0, busy: false, error: String::new(), exists: None };
        add.schedule(cx);
        add
    }

    /// The input changed: expand the batch, or probe the link, after a
    /// short pause.
    fn schedule(&mut self, cx: &mut Cx) {
        self.generation += 1;
        if !self.form.batch {
            self.form.link_changed();
        }
        if self.form.batch || self.form.wants_probe() {
            cx.later(Duration::from_millis(300), "settled", json!(self.generation));
        }
    }

    fn probe(&mut self, cx: &mut Cx) {
        let (generation, link, opts) = (self.generation, self.form.link().to_string(), self.form.probe_opts());
        self.form.probing = true;
        cx.spawn(move || {
            let result = match api::call("probe_url", json!({ "url": link, "opts": opts })) {
                Ok(p) => json!({ "generation": generation, "ok": p }),
                Err(e) => json!({ "generation": generation, "err": e }),
            };
            ("probed".into(), result)
        });
    }

    fn expand(&mut self, cx: &mut Cx) {
        let (generation, lines) = (self.generation, self.form.batch_lines());
        cx.spawn(move || {
            let mut out: Vec<String> = vec![];
            for (line, pattern) in lines {
                if pattern {
                    if let Ok(list) = api::call("expand_batch", json!({ "pattern": line })) {
                        out.extend(odm_ui::strings(&json!({ "l": list }), "l"));
                    }
                } else {
                    out.push(line);
                }
            }
            ("expanded".into(), json!({ "generation": generation, "links": out }))
        });
    }

    fn submit(&mut self, start: bool, on_exists: Option<&'static str>, store: &Store, cx: &mut Cx) {
        if !self.form.valid() || self.busy {
            return;
        }
        self.error.clear();
        if on_exists.is_none() {
            if let Some((dir, name)) = self.form.target(folder_for(&self.form.category)) {
                let exists = api::call("target_exists", json!({ "dir": dir, "name": name })).ok().and_then(|v| v.as_bool()).unwrap_or(false);
                if exists {
                    self.exists = Some(start);
                    return;
                }
            }
        }
        self.busy = true;
        let requests = self.form.requests(start, on_exists);
        let assign = self.form.assignment();
        let show_progress = start && store.setting("showProgressOnStart", false);
        cx.spawn(move || {
            let added = (|| {
                if let Some((ext, category)) = assign {
                    api::call("assign_extensions", json!({ "category": category, "extensions": [ext] }))?;
                }
                if let [one] = requests.as_slice() {
                    let id = api::call("add_download", json!({ "req": one }))?;
                    Ok(if show_progress { id } else { Value::Null })
                } else {
                    api::call("add_many", json!({ "reqs": requests })).map(|_| Value::Null)
                }
            })();
            match added {
                Ok(progress) => ("added".into(), json!({ "progress": progress })),
                Err(e) => ("added".into(), json!({ "err": e })),
            }
        });
    }
}

impl Vm for Add {
    fn state(&self, store: &Store) -> Value {
        let f = &self.form;
        let info = match f.info() {
            Info::Hidden => Value::Null,
            Info::Checking => json!({ "title": "Checking link…", "detail": "", "spinning": true }),
            Info::Web { host } => json!({ "title": host, "detail": "Opened from a web page. The link has not been contacted yet.", "spinning": false }),
            Info::Failed { error } => json!({ "title": error, "detail": "You can still add it; it will retry when started.", "spinning": false }),
            Info::Probed { summary, detail, .. } => json!({ "title": summary, "detail": detail, "spinning": false }),
        };
        let check = if !f.from_web || f.probing {
            ""
        } else if f.probe.is_none() && f.probe_error.is_empty() {
            "Check Link"
        } else {
            "Check Again"
        };
        let mut queues = vec![json!({ "id": "", "name": "None (start manually)" })];
        queues.extend(store.queues.iter().map(|q| json!({ "id": s(q, "id"), "name": s(q, "name") })));
        json!({
            "fromWeb": f.from_web,
            "batch": f.batch,
            "url": f.url,
            "urlError": f.url_error(),
            "batchText": f.batch_text,
            "batchCount": f.batch_urls.len(),
            "info": info,
            "check": check,
            "name": f.name,
            "category": f.category,
            "categories": store.categories().iter().map(|c| json!({ "id": s(c, "id"), "name": s(c, "name") })).collect::<Vec<_>>(),
            "categoryName": store.category_name(&f.category),
            "queue": f.queue,
            "queues": queues,
            "dir": f.dir,
            "recents": self.recents,
            "connections": f.connections,
            "connectionChoices": CONNECTIONS,
            "canSplit": f.can_split(),
            "always": f.always,
            "showsAlways": f.shows_always(),
            "nameExt": f.name_ext(),
            "remember": f.remember,
            "verify": f.verify,
            "checksum": f.checksum,
            "username": f.username,
            "password": f.password,
            "referrer": f.referrer,
            "cookies": f.cookies,
            "userAgent": f.user_agent,
            "headers": f.headers,
            "error": self.error,
            "ready": f.valid() && !self.busy,
            "startLabel": if f.batch { format!("Start {} Downloads", f.batch_urls.len()) } else { "Start Download".to_string() },
            "exists": self.exists.map(|_| f.name.trim().to_string()),
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        let text = || payload.as_str().unwrap_or("").to_string();
        match action {
            "text" => {
                let value = s(payload, "value").to_string();
                let f = &mut self.form;
                let field = s(payload, "field");
                match field {
                    "url" => f.url = value,
                    "batchText" => f.batch_text = value,
                    "name" => f.name = value,
                    "dir" => f.dir = value,
                    "checksum" => f.checksum = value,
                    "username" => f.username = value,
                    "password" => f.password = value,
                    "referrer" => f.referrer = value,
                    "cookies" => f.cookies = value,
                    "userAgent" => f.user_agent = value,
                    _ => {}
                }
                if matches!(field, "url" | "batchText") {
                    self.schedule(cx);
                }
            }
            "header" => {
                if let Some(h) = self.form.headers.get_mut(n(payload, "index") as usize) {
                    let value = s(payload, "value").to_string();
                    if s(payload, "part") == "name" {
                        h.0 = value;
                    } else {
                        h.1 = value;
                    }
                }
            }
            "add-header" => self.form.headers.push(Default::default()),
            "remove-header" => {
                let i = payload.as_u64().unwrap_or(u64::MAX) as usize;
                if i < self.form.headers.len() {
                    self.form.headers.remove(i);
                }
            }
            "flag" => {
                let on = b(payload, "value");
                match s(payload, "flag") {
                    "batch" => {
                        self.form.batch = on;
                        self.schedule(cx);
                    }
                    "always" => self.form.always = on,
                    "remember" => self.form.remember = on,
                    "verify" => self.form.verify = on,
                    _ => {}
                }
            }
            "category" => {
                let id = text();
                self.form.choose_category(&id, folder_for(&id));
            }
            "queue" => self.form.queue = text(),
            "connections" => self.form.connections = payload.as_u64().unwrap_or(16) as u32,
            "settled" => {
                if payload.as_u64() == Some(self.generation) {
                    if self.form.batch {
                        self.expand(cx);
                    } else if self.form.wants_probe() {
                        self.probe(cx);
                    }
                }
            }
            "probed" => {
                if n(payload, "generation") == self.generation {
                    match payload.get("ok") {
                        Some(p) => self.form.probed(p.clone(), n(&store.settings, "defaultConnections"), folder_for),
                        None => self.form.probe_failed(s(payload, "err").to_string()),
                    }
                }
            }
            "expanded" => {
                if n(payload, "generation") == self.generation {
                    self.form.set_batch_urls(odm_ui::strings(payload, "links"));
                }
            }
            // probe right away (a web link is only contacted on request)
            "check" => {
                if !self.form.batch && odm_ui::fmt::is_http(self.form.link()) {
                    self.generation += 1;
                    self.form.probe = None;
                    self.form.probe_error.clear();
                    self.probe(cx);
                }
            }
            "submit" => self.submit(payload.as_bool().unwrap_or(true), None, store, cx),
            "exists" => {
                if let Some(start) = self.exists.take() {
                    match payload.as_str() {
                        Some("rename") => self.submit(start, Some("rename"), store, cx),
                        Some("resume") => self.submit(start, Some("resume"), store, cx),
                        Some("overwrite") => self.submit(start, Some("overwrite"), store, cx),
                        _ => {}
                    }
                }
            }
            "added" => match payload.get("err").and_then(Value::as_str) {
                Some(e) => {
                    self.error = e.to_string();
                    self.busy = false;
                }
                None => {
                    if let Some(id) = payload.get("progress").and_then(Value::as_str) {
                        cx.request("progress", json!(id));
                    }
                    cx.close();
                }
            },
            "cancel" => cx.close(),
            _ => {}
        }
    }
}
