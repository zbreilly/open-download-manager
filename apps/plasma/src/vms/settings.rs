//! Settings. Changes apply as you make them (saved after a short pause),
//! and settings changed elsewhere (tray, toolbar, extension) flow in
//! without losing edits in progress.
use crate::backend;
use crate::vm::{Cx, Vm};
use odm_ffi::api;
use odm_ui::settings::{merge, parse_extensions, set_category_extensions};
use odm_ui::store::Store;
use odm_ui::{b, fmt, n, s};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

const EXTENSION_DIRS: [&str; 2] = ["/usr/share/open-download-manager/extension", "extension/dist"];

pub struct Settings {
    draft: Value,
    /// the engine's settings the draft is based on
    base: Value,
    /// what is typed in the speed fields, as typed
    texts: HashMap<String, String>,
    generation: u64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Settings {
    pub fn new(store: &Store) -> Self {
        Settings { draft: store.settings.clone(), base: store.settings.clone(), texts: HashMap::new(), generation: 0 }
    }

    fn put(&mut self, key: &str, value: Value, cx: &mut Cx) {
        if self.draft.get(key) == Some(&value) {
            return;
        }
        self.draft[key] = value;
        self.generation += 1;
        cx.later(Duration::from_millis(400), "settled", json!(self.generation));
    }

    /// Save the draft. When the engine refuses it (e.g. the new bridge
    /// port is taken) the error is shown and the engine's copy comes back.
    fn save(&mut self, store: &Store, cx: &mut Cx) {
        let latest = store.settings.clone();
        let mut merged = merge(&self.base, &self.draft, &latest);
        if merged == latest {
            return;
        }
        let want = b(&merged, "launchAtLogin");
        if b(&latest, "launchAtLogin") != want {
            if let Err(e) = odm_linux::autostart::set(want) {
                backend::report(e);
            }
            // keep the setting equal to what actually happened
            merged["launchAtLogin"] = odm_linux::autostart::is_enabled().into();
        }
        self.base = merged.clone();
        self.draft = merged.clone();
        cx.spawn(move || match api::call("save_settings", json!({ "settings": merged })) {
            Ok(_) => ("saved".into(), Value::Null),
            Err(e) => ("saved".into(), json!(e)),
        });
    }

    fn categories(&self) -> Vec<Value> {
        self.draft.get("categories").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    fn edit_category(&mut self, id: &str, f: impl Fn(&mut Value), cx: &mut Cx) {
        let mut cats = self.categories();
        for c in cats.iter_mut().filter(|c| s(c, "id") == id) {
            f(c);
        }
        self.put("categories", cats.into(), cx);
    }

    fn default_folder(&self, c: &Value) -> String {
        let root = s(&self.draft, "defaultFolder");
        if s(c, "id") == "general" {
            root.to_string()
        } else {
            std::path::Path::new(root).join(s(c, "name").replace('/', "_")).to_string_lossy().into_owned()
        }
    }
}

impl Vm for Settings {
    fn state(&self, store: &Store) -> Value {
        let ext = &store.ext;
        let bridge_error = ext.get("bridgeError").and_then(Value::as_str);
        let browsers = odm_ui::strings(ext, "browsers");
        let (ext_state, ext_text) = if bridge_error.is_some() {
            ("error", "Not listening".to_string())
        } else if b(ext, "connected") {
            ("complete", format!("Connected: {}", browsers.join(", ")))
        } else {
            ("idle", "Not detected".to_string())
        };
        let speed = |key: &str| {
            let shown = self.texts.get(key).cloned().unwrap_or_else(|| fmt::mbps(n(&self.draft, key)));
            json!({ "text": shown, "error": if fmt::limit_bps(&shown).is_none() { fmt::LIMIT_ERROR } else { "" } })
        };
        let categories: Vec<Value> = self
            .categories()
            .iter()
            .map(|c| {
                let own = c.get("folder").and_then(Value::as_str).filter(|f| !f.is_empty());
                let general = s(c, "id") == "general";
                let extensions = odm_ui::strings(c, "extensions");
                json!({
                    "id": s(c, "id"),
                    "name": s(c, "name"),
                    "general": general,
                    "summary": if general { "Everything else".to_string() } else { format!("{} types", extensions.len()) },
                    "folder": own.map(String::from).unwrap_or_else(|| self.default_folder(c)),
                    "ownFolder": own.is_some(),
                    "extensions": extensions.join(" "),
                })
            })
            .collect();
        json!({
            // the settings themselves, for plain switches, numbers and choices
            "v": self.draft,
            "globalLimit": speed("globalLimitBps"),
            "perDownloadLimit": speed("perDownloadLimitBps"),
            "capturePaused": self.draft.get("capturePausedUntil").and_then(Value::as_i64).map(|t| t > now_ms()).unwrap_or(false),
            "captureTypes": odm_ui::strings(&self.draft, "captureExtensions").join(" "),
            "extension": {
                "state": ext_state,
                "text": ext_text,
                "error": bridge_error.map(|e| format!("{e}. Another app may be using this port; choose a different one under Advanced.")),
            },
            "categories": categories,
            "manualProxy": matches!(s(&self.draft, "proxyMode"), "http" | "socks5"),
            "tlsWarning": !odm_ui::b_or(&self.draft, "verifyTls", true),
        })
    }

    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx) {
        let id = s(payload, "id").to_string();
        match action {
            // {key, value}: a switch, number, choice or text
            "set" => {
                let key = s(payload, "key").to_string();
                if !key.is_empty() {
                    self.put(&key, payload.get("value").cloned().unwrap_or(Value::Null), cx);
                }
            }
            // {key, text}: a speed in MB/s
            "speed" => {
                let (key, text) = (s(payload, "key").to_string(), s(payload, "text").to_string());
                // text that isn't a usable speed is kept as typed but not stored
                if let Some(bps) = fmt::limit_bps(&text) {
                    self.put(&key, bps.into(), cx);
                }
                self.texts.insert(key, text);
            }
            "settled" => {
                if payload.as_u64() == Some(self.generation) {
                    self.save(store, cx);
                }
            }
            "saved" => {
                if let Some(e) = payload.as_str() {
                    backend::report(e.to_string());
                    // nothing was stored: go back to what the engine has
                    self.base = store.settings.clone();
                    self.draft = store.settings.clone();
                    self.texts.clear();
                }
            }
            // the window is closing: pending edits are saved right away
            "flush" => self.save(store, cx),
            "pause-capture" => {
                let until = if payload.as_bool().unwrap_or(false) { json!(now_ms() + 600_000) } else { Value::Null };
                self.put("capturePausedUntil", until, cx);
            }
            "capture-types" => self.put("captureExtensions", parse_extensions(payload.as_str().unwrap_or("")).into(), cx),
            "show-extension" => match EXTENSION_DIRS.iter().map(std::path::Path::new).find(|d| d.join("chromium").is_dir()) {
                Some(dir) => {
                    if let Err(e) = odm_linux::open::reveal(&dir.join("chromium")) {
                        backend::report(e);
                    }
                }
                None => backend::report("The extension isn't installed with this build. Run extension/build.sh in the source folder and load extension/dist/chromium.".into()),
            },
            "copy-pairing" | "new-pairing" => {
                let fresh = action == "new-pairing";
                match api::call_as::<String>(if fresh { "regenerate_bridge_key" } else { "bridge_key" }, Value::Null) {
                    Ok(code) => {
                        backend::copy_text(&code);
                        if fresh {
                            backend::report("New pairing key copied. Paste it into the extension in each browser; the old key no longer works.".into());
                        }
                    }
                    Err(e) => backend::report(e),
                }
            }
            "category-name" => {
                let name = s(payload, "value").trim().to_string();
                if !name.is_empty() {
                    self.edit_category(&id, |c| c["name"] = name.clone().into(), cx);
                }
            }
            "category-folder" => {
                let folder = payload.get("value").cloned().filter(|v| v.as_str().map(|f| !f.is_empty()).unwrap_or(false)).unwrap_or(Value::Null);
                self.edit_category(&id, |c| c["folder"] = folder.clone(), cx);
            }
            "category-types" => {
                let mut cats = self.categories();
                // an extension belongs to one category: taking it here removes it elsewhere
                set_category_extensions(&mut cats, &id, &parse_extensions(s(payload, "value")));
                self.put("categories", cats.into(), cx);
            }
            "remove-category" => {
                let cats: Vec<Value> = self.categories().into_iter().filter(|c| s(c, "id") != id).collect();
                self.put("categories", cats.into(), cx);
            }
            "add-category" => {
                let mut cats = self.categories();
                let new_id = format!("c{:06x}", now_ms() as u64 & 0xff_ffff);
                let at = cats.iter().position(|c| s(c, "id") == "general").unwrap_or(cats.len());
                cats.insert(at, json!({ "id": new_id, "name": "New Category", "icon": "cat-general", "extensions": [], "folder": null }));
                self.put("categories", cats.into(), cx);
            }
            _ => {}
        }
    }

    fn store_changed(&mut self, kind: &str, store: &Store, _cx: &mut Cx) {
        if kind == "settings" {
            // settings changed elsewhere: take them without losing edits here
            self.draft = merge(&self.base, &self.draft, &store.settings);
            self.base = store.settings.clone();
        }
    }
}
