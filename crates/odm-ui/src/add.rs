//! The Add Download form: what its fields mean, how they follow a probe of
//! the link, and the request it turns into. The front ends only draw it.
use crate::{b, fmt, opt_n, s};
use serde_json::{json, Value};

/// The choices of the connections selector.
pub const CONNECTIONS: [u32; 5] = [1, 4, 8, 16, 32];

/// The selector choice closest to a setting.
pub fn nearest_connections(want: u64) -> u32 {
    CONNECTIONS.iter().copied().min_by_key(|c| (*c as u64).abs_diff(want.max(1))).unwrap_or(16)
}

/// What the line under the URL field says.
#[derive(Debug, Clone, PartialEq)]
pub enum Info {
    /// nothing to say (no valid link yet)
    Hidden,
    Checking,
    /// opened from a web page and not contacted yet
    Web { host: String },
    /// the probe failed; the download can still be added
    Failed { error: String },
    /// "48.0 MB · application/zip" and "Resumable · byte ranges · HTTP/2"
    Probed { summary: String, detail: String, resumable: bool },
    /// the link can't be downloaded (a DRM-protected or live stream)
    Blocked { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AddForm {
    /// Opened from a web page (`source: "web"`): the page chose the URL, so
    /// it is not contacted until the user asks (Check Link), Enter doesn't
    /// start the download, and only the URL is taken from the request.
    pub from_web: bool,
    /// several links, or a numbered pattern
    pub batch: bool,
    pub url: String,
    pub batch_text: String,
    /// the links `batch_text` expands to
    pub batch_urls: Vec<String>,
    pub name: String,
    pub category: String,
    /// "" = none (start manually)
    pub queue: String,
    pub dir: String,
    pub connections: u32,
    /// always put this file type in the chosen category
    pub always: bool,
    /// remember the folder for the category
    pub remember: bool,
    pub verify: bool,
    pub checksum: String,
    pub username: String,
    pub password: String,
    pub referrer: String,
    pub cookies: String,
    pub user_agent: String,
    pub headers: Vec<(String, String)>,
    pub probe: Option<Value>,
    pub probe_error: String,
    pub probing: bool,
    /// HLS/DASH: the chosen quality (variant id) and audio track id
    pub quality: String,
    pub audio: String,
    /// the name, folder and category the form filled in by itself; a field
    /// that still equals its value here was not touched by the user
    auto_name: String,
    auto_dir: String,
    auto_category: String,
    category_touched: bool,
}

fn opt(text: &str) -> Value {
    let t = text.trim();
    if t.is_empty() {
        Value::Null
    } else {
        t.into()
    }
}

/// A line of the batch field that the engine has to expand
/// (`file[001-120].jpg`, `[a-z]`).
pub fn is_pattern(line: &str) -> bool {
    line.find('[').zip(line.rfind(']')).map(|(a, z)| a < z && line[a..z].contains('-')).unwrap_or(false)
}

impl AddForm {
    /// `params` is the engine's `show_add` payload (any field may be
    /// missing), `{batch: true}`, or `{url, source: "web"}`.
    /// `general_folder` is where the General category saves.
    pub fn new(params: &Value, default_connections: u64, general_folder: &str) -> Self {
        let from_web = s(params, "source") == "web";
        let mut f = AddForm {
            from_web,
            batch: b(params, "batch"),
            url: s(params, "url").trim().to_string(),
            category: "general".into(),
            auto_category: "general".into(),
            dir: general_folder.into(),
            auto_dir: general_folder.into(),
            connections: nearest_connections(default_connections),
            remember: true,
            ..Default::default()
        };
        if !from_web {
            // a web page only gets to suggest the URL
            f.name = s(params, "fileName").into();
            f.referrer = s(params, "referrer").into();
            f.cookies = s(params, "cookies").into();
            f.user_agent = s(params, "userAgent").into();
        }
        f
    }

    pub fn link(&self) -> &str {
        self.url.trim()
    }

    pub fn name_ext(&self) -> String {
        fmt::extension(self.name.trim())
    }

    pub fn dir_touched(&self) -> bool {
        self.dir != self.auto_dir
    }

    fn name_touched(&self) -> bool {
        self.name != self.auto_name
    }

    pub fn valid(&self) -> bool {
        if self.batch {
            !self.batch_urls.is_empty()
        } else {
            fmt::is_link(self.link()) && self.blocked().is_none()
        }
    }

    /// What an HLS/DASH manifest offers (the probe's `stream`).
    pub fn stream(&self) -> Option<&Value> {
        if self.batch {
            return None;
        }
        self.probe.as_ref().and_then(|p| p.get("stream")).filter(|v| v.is_object())
    }

    /// Why the link can't be downloaded (a protected or live stream).
    pub fn blocked(&self) -> Option<String> {
        self.stream().map(|st| s(st, "blocked").to_string()).filter(|r| !r.is_empty())
    }

    fn variants(&self) -> Vec<&Value> {
        self.stream().and_then(|st| st.get("variants")).and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
    }

    /// The qualities to choose from: `(id, label)`, best first, with the
    /// estimated size.
    pub fn qualities(&self) -> Vec<(String, String)> {
        self.variants()
            .into_iter()
            .map(|v| {
                let label = match opt_n(v, "estBytes") {
                    Some(b) => format!("{} — about {}", s(v, "label"), fmt::bytes(Some(b))),
                    None => s(v, "label").to_string(),
                };
                (s(v, "id").to_string(), label)
            })
            .collect()
    }

    /// Separate audio tracks to choose from: `(id, label)`.
    pub fn audio_tracks(&self) -> Vec<(String, String)> {
        self.stream()
            .and_then(|st| st.get("audio"))
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|t| (s(t, "id").to_string(), s(t, "label").to_string())).collect())
            .unwrap_or_default()
    }

    /// Show the quality selector (more than one choice and not blocked).
    pub fn shows_quality(&self) -> bool {
        self.blocked().is_none() && self.variants().len() > 1
    }

    pub fn shows_audio(&self) -> bool {
        self.blocked().is_none() && self.audio_tracks().len() > 1
    }

    fn chosen_variant(&self) -> Option<&Value> {
        let v = self.variants();
        v.iter().find(|x| s(x, "id") == self.quality).or(v.first()).copied()
    }

    /// The user picked a quality: the name follows its file type.
    pub fn choose_quality(&mut self, id: &str) {
        self.quality = id.to_string();
        let ext = self.chosen_variant().map(|v| s(v, "ext").to_string()).unwrap_or_default();
        if !ext.is_empty() && self.name_ext() != ext && !self.name.trim().is_empty() {
            // a page title such as "Ep. 5 The End" has no extension to replace
            let cur = self.name_ext();
            let has_ext = !cur.is_empty() && cur.len() <= 5 && cur.chars().all(|c| c.is_ascii_alphanumeric());
            let stem = match self.name.rfind('.') {
                Some(i) if i > 0 && has_ext => self.name[..i].to_string(),
                _ => self.name.trim().to_string(),
            };
            let touched = self.name_touched();
            self.name = format!("{stem}.{ext}");
            if !touched {
                self.auto_name = self.name.clone();
            }
        }
    }

    pub fn choose_audio(&mut self, id: &str) {
        self.audio = id.to_string();
    }

    /// The URL field holds something that is not a usable address.
    pub fn url_error(&self) -> bool {
        !self.batch && !self.link().is_empty() && !fmt::is_link(self.link())
    }

    /// Offer "Always put .ext files in <category>".
    pub fn shows_always(&self) -> bool {
        !self.batch && !self.name_ext().is_empty() && self.category != self.auto_category
    }

    /// The connections selector applies (the server supports ranges, or
    /// we don't know yet).
    pub fn can_split(&self) -> bool {
        self.probe.as_ref().map(|p| b(p, "resumable")).unwrap_or(true)
    }

    /// Should a change of the link be probed automatically?
    pub fn wants_probe(&self) -> bool {
        !self.batch && !self.from_web && fmt::is_link(self.link())
    }

    /// The link changed: forget what was known about the old one.
    pub fn link_changed(&mut self) {
        self.probe = None;
        self.quality.clear();
        self.audio.clear();
        self.probe_error.clear();
        self.probing = self.wants_probe();
    }

    /// The user picked a category. `folder` is where that category saves
    /// (the engine's `folder_for`).
    pub fn choose_category(&mut self, id: &str, folder: Option<String>) {
        self.category_touched = true;
        self.set_category(id, folder);
    }

    fn set_category(&mut self, id: &str, folder: Option<String>) {
        self.category = id.into();
        if let (false, Some(f)) = (self.dir_touched(), folder) {
            self.dir = f.clone();
            self.auto_dir = f;
        }
    }

    /// Options for the engine's `probe_url`.
    pub fn probe_opts(&self) -> Value {
        json!({
            "referrer": opt(&self.referrer),
            "cookies": opt(&self.cookies),
            "username": opt(&self.username),
            "password": opt(&self.password),
            "headers": self.header_pairs(),
        })
    }

    fn header_pairs(&self) -> Vec<Value> {
        self.headers.iter().filter(|(k, _)| !k.trim().is_empty()).map(|(k, v)| json!([k.trim(), v.trim()])).collect()
    }

    /// The probe answered. `folder_for(category)` is the engine's lookup.
    pub fn probed(&mut self, p: Value, default_connections: u64, folder_for: impl Fn(&str) -> Option<String>) {
        self.probing = false;
        self.probe_error.clear();
        if !self.name_touched() {
            self.name = s(&p, "fileName").into();
            self.auto_name = self.name.clone();
        }
        self.auto_category = s(&p, "category").into();
        if !self.category_touched {
            let id = self.auto_category.clone();
            let folder = folder_for(&id);
            self.set_category(&id, folder);
        }
        if !b(&p, "resumable") {
            self.connections = 1;
        } else if self.connections == 1 {
            self.connections = nearest_connections(default_connections);
        }
        self.probe = Some(p);
        // a stream starts at its best quality and default audio
        self.quality = self.variants().first().map(|v| s(v, "id").to_string()).unwrap_or_default();
        self.audio = self
            .stream()
            .and_then(|st| st.get("audio"))
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|t| b(t, "default")).or(a.first()))
            .map(|t| s(t, "id").to_string())
            .unwrap_or_default();
        // a name given with the link (a page title) gets the stream's file type
        if self.stream().is_some() {
            let q = self.quality.clone();
            self.choose_quality(&q);
        }
    }

    /// The probe failed: the download can still be added, named after the
    /// link.
    pub fn probe_failed(&mut self, error: String) {
        self.probing = false;
        self.probe = None;
        self.quality.clear();
        self.audio.clear();
        self.probe_error = error;
        let last = fmt::url_file_name(self.link());
        if !self.name_touched() && !last.is_empty() {
            self.name = last;
            self.auto_name = self.name.clone();
        }
    }

    pub fn info(&self) -> Info {
        if self.batch || !fmt::is_link(self.link()) {
            Info::Hidden
        } else if self.probing {
            Info::Checking
        } else if let Some(reason) = self.blocked() {
            Info::Blocked { reason }
        } else if let (Some(p), Some(st)) = (&self.probe, self.stream()) {
            let kind = s(st, "kind").to_ascii_uppercase();
            let size = self.chosen_variant().and_then(|v| opt_n(v, "estBytes"));
            let n = self.variants().len();
            let audio = if self.chosen_variant().is_some_and(|v| b(v, "separateAudio")) { " · separate audio track" } else { "" };
            Info::Probed {
                summary: match size {
                    Some(b) => format!("{kind} stream · about {}", fmt::bytes(Some(b))),
                    None => format!("{kind} stream"),
                },
                detail: format!(
                    "{} {}{audio} · resumable · {}",
                    n,
                    if n == 1 { "quality" } else { "qualities" },
                    s(p, "protocol")
                ),
                resumable: true,
            }
        } else if let Some(p) = &self.probe {
            let mime = p.get("mime").and_then(Value::as_str).map(|m| format!(" · {m}")).unwrap_or_default();
            let resumable = b(p, "resumable");
            Info::Probed {
                summary: format!("{}{mime}", fmt::bytes(opt_n(p, "totalBytes"))),
                detail: if resumable {
                    format!("Resumable · byte ranges · {}", s(p, "protocol"))
                } else {
                    "Server doesn't support resume. Download will use 1 connection.".into()
                },
                resumable,
            }
        } else if !self.probe_error.is_empty() {
            Info::Failed { error: self.probe_error.clone() }
        } else if self.from_web {
            Info::Web { host: fmt::host(self.link()) }
        } else {
            Info::Hidden
        }
    }

    /// The lines of the batch field: `(text, needs expanding by the engine)`.
    pub fn batch_lines(&self) -> Vec<(String, bool)> {
        self.batch_text.split_whitespace().map(|l| (l.to_string(), is_pattern(l))).collect()
    }

    /// The batch field expanded to links (keeps valid http(s) and FTP
    /// ones, at most 10,000).
    pub fn set_batch_urls(&mut self, links: Vec<String>) {
        self.batch_urls = links.into_iter().filter(|u| fmt::is_link(u)).take(10_000).collect();
    }

    /// The engine's `AddRequest` for one link.
    pub fn request(&self, start: bool, link: &str, on_exists: Option<&str>) -> Value {
        let batch = self.batch;
        let from_probe = |k: &str| match (&self.probe, batch) {
            (Some(p), false) => p.get(k).cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let dir_touched = self.dir_touched();
        let checksum = self.checksum.trim();
        json!({
            "url": link,
            "fileName": if batch { Value::Null } else { opt(&self.name) },
            "saveDir": if dir_touched { opt(&self.dir) } else { Value::Null },
            "category": if batch { Value::Null } else { self.category.clone().into() },
            "queueId": if !self.queue.is_empty() { Value::String(self.queue.clone()) } else if start { Value::Null } else { "main".into() },
            "start": start,
            "connections": self.connections,
            "referrer": opt(&self.referrer),
            "cookies": opt(&self.cookies),
            "userAgent": opt(&self.user_agent),
            "headers": self.header_pairs(),
            "username": opt(&self.username),
            "password": opt(&self.password),
            "checksum": if !batch && self.verify && !checksum.is_empty() { json!({ "algo": "", "expected": checksum }) } else { Value::Null },
            "onExists": on_exists.unwrap_or("rename"),
            "rememberFolder": !batch && self.remember && dir_touched,
            "totalBytes": match (self.stream(), self.chosen_variant()) {
                (Some(_), Some(v)) => v.get("estBytes").cloned().unwrap_or(Value::Null),
                _ => from_probe("totalBytes"),
            },
            "resumable": from_probe("resumable"),
            "mime": from_probe("mime"),
            "stream": match self.stream() {
                Some(st) => json!({
                    "kind": s(st, "kind"),
                    "variant": opt(&self.quality),
                    "audio": opt(&self.audio),
                }),
                None => Value::Null,
            },
        })
    }

    /// Every request this form adds.
    pub fn requests(&self, start: bool, on_exists: Option<&str>) -> Vec<Value> {
        if self.batch {
            self.batch_urls.iter().map(|u| self.request(start, u, None)).collect()
        } else {
            vec![self.request(start, self.link(), on_exists)]
        }
    }

    /// `(extension, category)` to assign before adding, when the user asked
    /// to always put this file type there.
    pub fn assignment(&self) -> Option<(String, String)> {
        (self.always && self.shows_always()).then(|| (self.name_ext(), self.category.clone()))
    }

    /// Where to look for an existing file of the same name: `(folder,
    /// name)`, or None when there is nothing to check. `category_folder`
    /// is the engine's folder for the chosen category.
    pub fn target(&self, category_folder: Option<String>) -> Option<(String, String)> {
        let name = self.name.trim();
        if self.batch || name.is_empty() {
            return None;
        }
        let dir = if self.dir_touched() { self.dir.clone() } else { category_folder.unwrap_or_else(|| self.dir.clone()) };
        Some((dir, name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_only_carry_the_url() {
        let f = AddForm::new(&json!({ "url": "https://x.example/a.zip", "source": "web", "fileName": "evil.exe", "referrer": "r" }), 16, "/dl");
        assert!(f.from_web && f.name.is_empty() && f.referrer.is_empty());
        assert!(!f.wants_probe());
        assert_eq!(f.info(), Info::Web { host: "x.example".into() });
    }

    #[test]
    fn probe_fills_untouched_fields() {
        let mut f = AddForm::new(&json!({ "url": "https://x.example/a" }), 16, "/dl");
        f.link_changed();
        assert_eq!(f.info(), Info::Checking);
        let p = json!({ "fileName": "movie.mkv", "category": "video", "resumable": false, "totalBytes": 1048576, "protocol": "HTTP/1.1" });
        f.probed(p, 16, |c| Some(format!("/dl/{c}")));
        assert_eq!((f.name.as_str(), f.category.as_str(), f.dir.as_str(), f.connections), ("movie.mkv", "video", "/dl/video", 1));
        assert!(!f.dir_touched() && !f.shows_always());
        f.choose_category("general", Some("/dl".into()));
        assert!(f.shows_always());
        f.always = true;
        assert_eq!(f.assignment(), Some(("mkv".into(), "general".into())));
        let r = f.request(false, f.link(), None);
        assert_eq!(r["queueId"], "main");
        assert_eq!(r["saveDir"], Value::Null);
        assert_eq!(r["totalBytes"], 1048576);
    }

    #[test]
    fn user_edits_survive_a_probe() {
        let mut f = AddForm::new(&json!({ "url": "https://x.example/a" }), 8, "/dl");
        f.name = "mine.bin".into();
        f.dir = "/elsewhere".into();
        f.probed(json!({ "fileName": "theirs.bin", "category": "programs", "resumable": true }), 8, |_| Some("/dl/Programs".into()));
        assert_eq!((f.name.as_str(), f.dir.as_str()), ("mine.bin", "/elsewhere"));
        assert_eq!(f.request(true, f.link(), Some("resume"))["saveDir"], "/elsewhere");
        assert_eq!(f.target(None), Some(("/elsewhere".into(), "mine.bin".into())));
    }

    #[test]
    fn streams_offer_qualities() {
        let mut f = AddForm::new(&json!({ "url": "https://x.example/master.m3u8" }), 16, "/dl");
        f.link_changed();
        let p = json!({
            "fileName": "master.ts", "category": "video", "resumable": true, "totalBytes": 3_000_000, "protocol": "HLS · HTTP/1.1",
            "stream": {
                "kind": "hls", "protected": false, "live": false,
                "variants": [
                    { "id": "5000000-1080", "label": "1080p · 5.0 Mbps", "estBytes": 3_000_000, "separateAudio": true, "ext": "ts" },
                    { "id": "800000-360", "label": "360p · 800 kbps", "estBytes": 500_000, "separateAudio": true, "ext": "ts" }
                ],
                "audio": [
                    { "id": "English|en", "label": "English", "default": true },
                    { "id": "Español|es", "label": "Español", "default": false }
                ]
            }
        });
        f.probed(p, 16, |c| Some(format!("/dl/{c}")));
        assert!(f.shows_quality() && f.shows_audio() && f.valid());
        assert_eq!(f.quality, "5000000-1080");
        assert_eq!(f.audio, "English|en");
        assert_eq!(f.qualities()[1].1, "360p · 800 kbps — about 488.3 KB");
        f.choose_quality("800000-360");
        f.choose_audio("Español|es");
        let r = f.request(true, f.link(), None);
        assert_eq!(r["stream"], json!({ "kind": "hls", "variant": "800000-360", "audio": "Español|es" }));
        assert_eq!(r["totalBytes"], 500_000);
        match f.info() {
            Info::Probed { summary, detail, .. } => {
                assert!(summary.starts_with("HLS stream · about"), "{summary}");
                assert!(detail.starts_with("2 qualities · separate audio track"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn page_titles_get_the_stream_file_type() {
        let stream = json!({ "kind": "hls", "variants": [{ "id": "a", "label": "720p", "ext": "ts" }, { "id": "b", "label": "1080p", "ext": "mp4" }], "audio": [] });
        let mut f = AddForm::new(&json!({ "url": "https://x.example/m.m3u8", "fileName": "Ep. 5 The End" }), 16, "/dl");
        f.probed(json!({ "fileName": "m.ts", "category": "video", "resumable": true, "stream": stream }), 16, |_| None);
        assert_eq!(f.name, "Ep. 5 The End.ts");
        f.choose_quality("b");
        assert_eq!(f.name, "Ep. 5 The End.mp4");
    }

    #[test]
    fn protected_streams_cannot_be_added() {
        let mut f = AddForm::new(&json!({ "url": "https://x.example/a.mpd" }), 16, "/dl");
        let p = json!({ "fileName": "a.mp4", "category": "video", "resumable": true,
            "stream": { "kind": "dash", "protected": true, "blocked": "This stream is protected with DRM", "variants": [{ "id": "v", "label": "720p", "ext": "mp4" }], "audio": [] } });
        f.probed(p, 16, |_| None);
        assert!(!f.valid() && !f.shows_quality());
        assert_eq!(f.info(), Info::Blocked { reason: "This stream is protected with DRM".into() });
        // plain links have no stream part
        let mut g = AddForm::new(&json!({ "url": "ftp://files.example/a.iso" }), 16, "/dl");
        assert!(g.valid() && g.wants_probe());
        g.probed(json!({ "fileName": "a.iso", "category": "programs", "resumable": true }), 16, |_| None);
        assert_eq!(g.request(true, g.link(), None)["stream"], Value::Null);
    }

    #[test]
    fn batches() {
        let mut f = AddForm::new(&json!({ "batch": true }), 16, "/dl");
        f.batch_text = "https://x.example/a[1-3].jpg\nhttps://x.example/b.zip".into();
        assert_eq!(f.batch_lines().iter().map(|l| l.1).collect::<Vec<_>>(), [true, false]);
        f.set_batch_urls(vec!["https://x.example/b.zip".into(), "mailto:x@y".into()]);
        assert!(f.valid());
        assert_eq!(f.requests(true, None).len(), 1);
        assert_eq!(nearest_connections(12), 8);
    }
}
