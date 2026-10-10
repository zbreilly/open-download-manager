//! Site Grabber: list the files linked from a page (or a list of links
//! handed over by the extension, a drop or an imported file) and add the
//! chosen ones.
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_ui::store::Store;
use odm_ui::{b, fmt, n, s};
use serde_json::{json, Value};
use std::collections::HashSet;

const PRESETS: [(&str, &str, &str); 8] = [
    ("all", "All files", ""),
    ("images", "Images", "jpg jpeg png gif webp bmp svg tif tiff avif heic"),
    ("video", "Video", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts"),
    ("audio", "Audio", "mp3 wav flac aac m4a ogg opus wma"),
    ("documents", "Documents", "pdf doc docx xls xlsx ppt pptx odt ods txt epub csv"),
    ("archives", "Archives", "zip rar 7z tar gz tgz bz2 xz zst"),
    ("programs", "Programs", "exe msi dmg pkg deb rpm appimage apk iso"),
    ("custom", "Custom…", ""),
];

struct Link {
    url: String,
    name: String,
    on: bool,
    /// this link's own cookies (never shared with other links)
    cookies: Option<String>,
}

#[derive(Default)]
pub struct Grabber {
    from_list: bool,
    page: String,
    depth: u64,
    preset: String,
    custom: String,
    same_host: bool,
    filter: String,
    links: Vec<Link>,
    dir: String,
    queue: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    busy: bool,
    error: String,
}

impl Grabber {
    pub fn new(params: &Value) -> Self {
        let mut vm = Grabber { preset: "all".into(), same_host: true, ..Default::default() };
        if params.get("links").is_some() {
            vm.take(params);
        }
        vm
    }

    /// Replace the list, keeping the first entry for each URL.
    fn set_links(&mut self, links: Vec<Link>) {
        let mut seen = HashSet::new();
        self.links = links.into_iter().filter(|l| seen.insert(l.url.clone())).collect();
    }

    /// A list of links from the extension, a drop or an imported file.
    fn take(&mut self, p: &Value) {
        self.from_list = true;
        self.error.clear();
        // a new list is a new context: nothing carries over from the last one
        let text = |k: &str| p.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
        self.referrer = text("referrer");
        self.user_agent = text("userAgent");
        let links = p
            .get("links")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|l| !s(l, "url").is_empty())
            .map(|l| {
                let url = s(l, "url").to_string();
                let name = fmt::url_file_name(&url);
                Link { name: if name.is_empty() { url.clone() } else { name }, url, on: true, cookies: l.get("cookies").and_then(Value::as_str).filter(|c| !c.is_empty()).map(String::from) }
            })
            .collect();
        self.set_links(links);
    }

    fn extensions(&self) -> Vec<String> {
        let preset = PRESETS.iter().find(|p| p.0 == self.preset).copied().unwrap_or(PRESETS[0]);
        odm_ui::settings::parse_extensions(if preset.0 == "custom" { &self.custom } else { preset.2 })
    }
}

impl Vm for Grabber {
    fn state(&self, store: &Store) -> Value {
        let q = self.filter.trim().to_lowercase();
        let chosen = self.links.iter().filter(|l| l.on).count();
        let shown: Vec<Value> = self
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| q.is_empty() || l.url.to_lowercase().contains(&q))
            .map(|(i, l)| json!({ "index": i, "name": l.name, "url": l.url, "on": l.on }))
            .collect();
        let mut queues = vec![json!({ "id": "", "name": "None (start now)" })];
        queues.extend(store.queues.iter().map(|q| json!({ "id": s(q, "id"), "name": s(q, "name") })));
        json!({
            "fromList": self.from_list,
            "page": self.page,
            "depth": self.depth,
            "preset": self.preset,
            "presets": PRESETS.iter().map(|p| json!({ "id": p.0, "name": p.1 })).collect::<Vec<_>>(),
            "custom": self.custom,
            "sameHost": self.same_host,
            "filter": self.filter,
            "links": shown,
            "count": format!("{chosen} of {} selected", self.links.len()),
            "empty": if self.busy { "Scanning…" } else { "Files found on the page appear here." },
            "dir": self.dir,
            "dirLabel": if self.dir.is_empty() { "Category folders" } else { self.dir.as_str() },
            "queue": self.queue,
            "queues": queues,
            "busy": self.busy,
            "canFind": !self.busy && !self.page.trim().is_empty(),
            "findLabel": if self.busy { "Scanning…" } else { "Find Files" },
            "ready": chosen > 0 && !self.busy,
            "downloadLabel": format!("Download {chosen}"),
            "error": self.error,
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        let text = || payload.as_str().unwrap_or("").to_string();
        match action {
            "page" => self.page = text(),
            "depth" => self.depth = payload.as_u64().unwrap_or(0).min(2),
            "preset" => self.preset = text(),
            "custom" => self.custom = text(),
            "same-host" => self.same_host = payload.as_bool().unwrap_or(true),
            "filter" => self.filter = text(),
            // a new list from the extension, a drop or an import
            "list" => self.take(payload),
            "find" => {
                let url = self.page.trim().to_string();
                if url.is_empty() || self.busy {
                    return;
                }
                self.busy = true;
                self.from_list = false;
                self.error.clear();
                let args = json!({ "url": url, "depth": self.depth, "extensions": self.extensions(), "sameHost": self.same_host });
                cx.spawn(move || match api::call("grab", args) {
                    Ok(found) => ("found".into(), json!({ "links": found, "page": url })),
                    Err(e) => ("found".into(), json!({ "err": e })),
                });
            }
            "found" => {
                self.busy = false;
                if let Some(e) = payload.get("err").and_then(Value::as_str) {
                    self.error = e.to_string();
                    return;
                }
                let found: Vec<Link> = payload
                    .get("links")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|l| Link { url: s(l, "url").into(), name: s(l, "fileName").into(), on: true, cookies: None })
                    .collect();
                if found.is_empty() {
                    self.error = "No matching files were found on that page.".into();
                }
                self.referrer = Some(s(payload, "page").to_string());
                self.user_agent = None;
                self.set_links(found);
            }
            "toggle" => {
                if let Some(l) = self.links.get_mut(n(payload, "index") as usize) {
                    l.on = b(payload, "value");
                }
            }
            "all" => {
                let on = payload.as_bool().unwrap_or(true);
                self.links.iter_mut().for_each(|l| l.on = on);
            }
            "dir" => self.dir = text(),
            "queue" => self.queue = text(),
            // Add to Queue (false) or Download (true)
            "add" => {
                let start = payload.as_bool().unwrap_or(true);
                if self.busy || !self.links.iter().any(|l| l.on) {
                    return;
                }
                self.busy = true;
                let connections = n(&store.settings, "defaultConnections");
                let reqs: Vec<Value> = self
                    .links
                    .iter()
                    .filter(|l| l.on)
                    .map(|l| {
                        json!({
                            "url": l.url,
                            "saveDir": if self.dir.is_empty() { Value::Null } else { self.dir.clone().into() },
                            "queueId": if !self.queue.is_empty() { Value::String(self.queue.clone()) } else if start { Value::Null } else { "main".into() },
                            "start": start,
                            "connections": connections,
                            "referrer": self.referrer,
                            "cookies": l.cookies,
                            "userAgent": self.user_agent,
                        })
                    })
                    .collect();
                cx.spawn(move || ("added".into(), api::call("add_many", json!({ "reqs": reqs })).err().map(Value::from).unwrap_or(Value::Null)));
            }
            "added" => {
                self.busy = false;
                match payload.as_str() {
                    Some(e) => self.error = e.to_string(),
                    None => cx.close(),
                }
            }
            "cancel" => cx.close(),
            _ => {}
        }
    }
}
