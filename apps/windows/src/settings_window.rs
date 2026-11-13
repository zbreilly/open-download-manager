//! Settings. Changes apply as you make them (saved after a short pause),
//! and settings changed elsewhere (tray, toolbar, extension) flow in
//! without losing edits in progress.
use crate::app::{Action, Shared};
use crate::system;
use crate::widgets::*;
use odm_ui::settings::{merge, parse_extensions, set_category_extensions};
use odm_ui::{b, b_or, fmt, n, s};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use windows_reactor::*;

const PAGES: [(&str, &str, Symbol); 7] = [
    ("general", "General", Symbol::Setting),
    ("connection", "Connection", Symbol::Sync),
    ("browser", "Browser", Symbol::Globe),
    ("types", "File Types", Symbol::Document),
    ("proxy", "Proxy", Symbol::MapDrive),
    ("notifications", "Notifications", Symbol::Message),
    ("advanced", "Advanced", Symbol::Repair),
];

pub enum Msg {
    Page(String),
    /// a setting changed: (key, value)
    Set(&'static str, Value),
    /// a speed field's text: (setting, text)
    Speed(&'static str, String),
    /// the pause after the last edit is over (generation)
    Settled(u32),
    PauseCapture(bool),
    CaptureTypes(String),
    ShowExtension,
    /// copy the pairing key (true: make a new one first)
    Pairing(bool),
    /// pick a folder for a setting, or for a category (its id)
    Browse(Option<String>, &'static str),
    Picked(Option<String>, &'static str, Option<String>),
    CategoryName(String, String),
    CategoryTypes(String, String),
    ApplyTypes(String),
    CategoryDefaultFolder(String),
    RemoveCategory(String),
    AddCategory,
}

pub struct SettingsWindow {
    shared: Shared,
    page: String,
    draft: Value,
    /// the engine's settings the draft is based on
    base: Value,
    /// what is typed in the speed and file-type fields, as typed
    texts: HashMap<String, String>,
    generation: u32,
    timer: Option<ComponentTimer>,
}

impl SettingsWindow {
    fn put(&mut self, key: &str, value: Value, context: &ComponentContext<Self>) {
        if self.draft.get(key) == Some(&value) {
            return;
        }
        self.draft[key] = value;
        self.generation = self.generation.wrapping_add(1);
        // kept: dropping a timer cancels it
        self.timer = Some(context.set_timeout(Duration::from_millis(400), Msg::Settled(self.generation)));
    }

    fn save(&mut self) {
        let latest = self.shared.store.borrow().settings.clone();
        let merged = merge(&self.base, &self.draft, &latest);
        if merged == latest {
            return;
        }
        self.base = merged.clone();
        self.draft = merged.clone();
        self.shared.act(Action::SaveSettings(merged));
    }

    fn categories(&self) -> Vec<Value> {
        self.draft.get("categories").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    fn edit_category(&mut self, id: &str, f: impl Fn(&mut Value), context: &ComponentContext<Self>) {
        let mut cats = self.categories();
        for c in cats.iter_mut().filter(|c| s(c, "id") == id) {
            f(c);
        }
        self.put("categories", cats.into(), context);
    }

    fn default_folder(&self, c: &Value) -> String {
        let root = s(&self.draft, "defaultFolder");
        if s(c, "id") == "general" {
            root.to_string()
        } else {
            std::path::Path::new(root).join(s(c, "name").replace(['/', '\\'], "_")).to_string_lossy().into_owned()
        }
    }

    // ------------------------------------------------------------ controls

    fn toggle(&self, key: &'static str, label: &str, hint: Option<&str>, context: &ViewContext<Self>) -> View {
        row(
            label,
            hint,
            ToggleSwitch::new().is_on(b(&self.draft, key)).on_content("").off_content("").on_toggled(context.callback(move |v: bool| Msg::Set(key, v.into()))),
        )
    }

    fn number(&self, key: &'static str, label: &str, min: f64, max: f64, hint: Option<&str>, context: &ViewContext<Self>) -> View {
        row(
            label,
            hint,
            NumberBox::new().minimum(min).maximum(max).value(n(&self.draft, key) as f64).min_width(150.0).on_value_changed(context.callback(move |v: Option<f64>| {
                Msg::Set(key, (v.filter(|v| v.is_finite()).unwrap_or(min).clamp(min, max).round() as u64).into())
            })),
        )
    }

    fn text_field(&self, key: &'static str, label: &str, context: &ViewContext<Self>) -> View {
        wide(label, TextBox::new(s(&self.draft, key)).on_text_changed(context.callback(move |t: Rc<str>| Msg::Set(key, t.to_string().into()))))
    }

    fn choice_row(&self, key: &'static str, label: &str, items: &[(&str, &str)], context: &ViewContext<Self>) -> View {
        row(label, None, choice(items, s(&self.draft, key), context.callback(move |v: String| Msg::Set(key, v.into()))))
    }

    /// A speed in MB/s with its on/off switch.
    fn speed(&self, key: &'static str, enabled: &'static str, label: &str, context: &ViewContext<Self>) -> View {
        let shown = self.texts.get(key).cloned().unwrap_or_else(|| fmt::mbps(n(&self.draft, key)));
        row(
            label,
            fmt::limit_bps(&shown).is_none().then_some(fmt::LIMIT_ERROR),
            StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((
                TextBox::new(shown).width(90.0).on_text_changed(context.callback(move |t: Rc<str>| Msg::Speed(key, t.to_string()))),
                dim("MB/s"),
                ToggleSwitch::new().is_on(b(&self.draft, enabled)).on_content("").off_content("").on_toggled(context.callback(move |v: bool| Msg::Set(enabled, v.into()))),
            )),
        )
    }

    // --------------------------------------------------------------- pages

    fn general(&self, c: &ViewContext<Self>) -> Vec<View> {
        vec![
            heading("General").into(),
            self.toggle("launchAtLogin", "Start when I sign in to Windows", None, c),
            self.toggle("minimizeToTray", "Keep running in the notification area when the window is closed", None, c),
            self.toggle("clipboardMonitor", "Watch the clipboard for download links", Some("Copying a link to a file type listed under Browser opens the Add window."), c),
            self.toggle("showProgressOnStart", "Show the progress window when a download starts", None, c),
            self.toggle("showCompleteDialog", "Show the download window when a download completes", None, c),
            self.toggle("confirmDelete", "Confirm before deleting downloads", None, c),
            heading("Appearance").into(),
            self.choice_row("theme", "Theme", &[("dark", "Dark"), ("light", "Light"), ("system", "Use Windows setting")], c),
        ]
    }

    fn connection(&self, c: &ViewContext<Self>) -> Vec<View> {
        vec![
            heading("Connections").into(),
            self.number(
                "defaultConnections",
                "Connections per download",
                1.0,
                32.0,
                Some("Files are split dynamically: a connection that finishes takes over half of the largest remaining piece."),
                c,
            ),
            self.number("maxSimultaneous", "Simultaneous downloads", 1.0, 64.0, Some("Outside of queues. Each queue sets its own limit."), c),
            heading("Speed limits").into(),
            self.speed("globalLimitBps", "globalLimitEnabled", "Global limit (all downloads combined)", c),
            self.speed("perDownloadLimitBps", "perDownloadLimitEnabled", "Per-download limit", c),
            self.toggle("limitOnStartup", "Turn on the global limit when the app starts", None, c),
        ]
    }

    fn browser(&self, c: &ViewContext<Self>) -> Vec<View> {
        let store = self.shared.store.borrow();
        let ext = &store.ext;
        let error = ext.get("bridgeError").and_then(Value::as_str);
        let browsers = odm_ui::strings(ext, "browsers");
        let (status, label) = if error.is_some() {
            ("error", "Not listening".to_string())
        } else if b(ext, "connected") {
            ("complete", format!("Connected: {}", browsers.join(", ")))
        } else {
            ("queued", "Not detected".to_string())
        };
        let mut rows: Vec<View> = vec![
            heading("Browser extension").into(),
            row(
                "Extension",
                Some("Chrome, Edge, Brave, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json."),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(10.0).children((
                    status_label(status, &label),
                    View::from(Button::new().content("Show Extension Folder").on_click(c.callback(|()| Msg::ShowExtension))),
                )),
            ),
        ];
        rows.push(
            row(
                "Pairing key",
                Some("Paste it into the extension’s popup, under Connection, and save it under Pairing key. Only an extension that holds the key can talk to the app, and other programs can’t read what it sends. A new key unpairs every browser."),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(10.0).children((
                    View::from(Button::new().content("Copy Key").on_click(c.callback(|()| Msg::Pairing(false)))),
                    View::from(Button::new().content("New Key").on_click(c.callback(|()| Msg::Pairing(true)))),
                )),
            )
            .into(),
        );
        if let Some(e) = error {
            rows.push(error_text(format!("{e}. Another app may be using this port; choose a different one under Advanced.")).into());
        }
        let paused = self.draft.get("capturePausedUntil").and_then(Value::as_i64).map(|t| t > now_ms()).unwrap_or(false);
        let types = self.texts.get("captureExtensions").cloned().unwrap_or_else(|| odm_ui::strings(&self.draft, "captureExtensions").join(" "));
        rows.extend([
            heading("Capture").into(),
            self.toggle("captureEnabled", "Take over browser downloads", None, c),
            row("Pause capture for 10 minutes", None, ToggleSwitch::new().is_on(paused).on_content("").off_content("").on_toggled(c.callback(Msg::PauseCapture))),
            self.choice_row("bypassKey", "Bypass key (hold while clicking)", &[("alt", "Alt"), ("shift", "Shift"), ("ctrl", "Ctrl"), ("none", "None")], c),
            self.number("captureMinSizeKb", "Skip files smaller than (KB)", 0.0, 10_000_000.0, None, c),
            heading("File types to capture").into(),
            TextBox::new(types)
                .accepts_return(true)
                .text_wrapping(TextWrapping::Wrap)
                .min_height(90.0)
                .on_text_changed(c.callback(|t: Rc<str>| Msg::CaptureTypes(t.to_string())))
                .into(),
        ]);
        rows
    }

    fn file_types(&self, c: &ViewContext<Self>) -> Vec<View> {
        let mut rows: Vec<View> = vec![heading("Categories").into(), note("Each category has its own file types and folder.").into()];
        for cat in self.categories() {
            let id = s(&cat, "id").to_string();
            let general = id == "general";
            let mut body: Vec<View> = vec![];
            if general {
                body.push(note("General holds every file type no other category claims, and saves to your download folder.").into());
            } else {
                let key = id.clone();
                body.push(TextBox::new(s(&cat, "name")).header("Name").on_text_changed(c.callback(move |t: Rc<str>| Msg::CategoryName(key.clone(), t.to_string()))).into());
            }
            let own = cat.get("folder").and_then(Value::as_str).filter(|f| !f.is_empty()).map(String::from);
            let mut folder: Vec<View> = vec![
                if own.is_some() { text(own.as_deref().unwrap_or("")) } else { dim(self.default_folder(&cat)) }.into(),
                Button::new().content("Choose…").grid_column(1).on_click({
                    let key = id.clone();
                    c.callback(move |()| Msg::Browse(Some(key.clone()), "folder"))
                }).into(),
            ];
            if own.is_some() {
                let key = id.clone();
                folder.push(Button::new().content("Use Default").grid_column(2).on_click(c.callback(move |()| Msg::CategoryDefaultFolder(key.clone()))).into());
            }
            body.push(text("Folder").into());
            body.push(Grid::new().column_spacing(8.0).columns([GridLength::STAR, GridLength::Auto, GridLength::Auto]).children(folder).into());
            if !general {
                let typed = self.texts.get(&format!("types:{id}")).cloned().unwrap_or_else(|| odm_ui::strings(&cat, "extensions").join(" "));
                let (key_a, key_b, key_c) = (id.clone(), id.clone(), id.clone());
                body.push(
                    Grid::new()
                        .column_spacing(8.0)
                        .columns([GridLength::STAR, GridLength::Auto])
                        .children((
                            TextBox::new(typed).header("File types, separated by spaces").on_text_changed(c.callback(move |t: Rc<str>| Msg::CategoryTypes(key_a.clone(), t.to_string()))),
                            Button::new().content("Apply").grid_column(1).vertical_alignment(VerticalAlignment::Bottom).on_click(c.callback(move |()| Msg::ApplyTypes(key_b.clone()))),
                        ))
                        .into(),
                );
                body.push(Button::new().content("Remove This Category").on_click(c.callback(move |()| Msg::RemoveCategory(key_c.clone()))).into());
            }
            let count = cat.get("extensions").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            rows.push(
                Expander::new()
                    .header(if general { format!("{} — everything else", s(&cat, "name")) } else { format!("{} — {count} types", s(&cat, "name")) })
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .horizontal_content_alignment(HorizontalAlignment::Stretch)
                    .margin(Thickness::new(0.0, 0.0, 0.0, 4.0))
                    .content(StackPanel::new().spacing(8.0).children(body))
                    .into(),
            );
        }
        rows.push(Button::new().content("Add Category").margin(Thickness::new(0.0, 8.0, 0.0, 0.0)).on_click(c.callback(|()| Msg::AddCategory)).into());
        rows
    }

    fn proxy(&self, c: &ViewContext<Self>) -> Vec<View> {
        let mut rows = vec![
            heading("Proxy").into(),
            self.choice_row("proxyMode", "Connection", &[("none", "No proxy"), ("system", "Use Windows settings"), ("http", "HTTP proxy"), ("socks5", "SOCKS5 proxy")], c),
        ];
        if matches!(s(&self.draft, "proxyMode"), "http" | "socks5") {
            rows.extend([
                self.text_field("proxyHost", "Host", c),
                self.number("proxyPort", "Port", 1.0, 65535.0, None, c),
                self.text_field("proxyUser", "Username", c),
                wide(
                    "Password",
                    PasswordBox::new().password(s(&self.draft, "proxyPass")).on_password_changed(c.callback(|t: Rc<str>| Msg::Set("proxyPass", t.to_string().into()))),
                ),
            ]);
        }
        rows
    }

    fn notifications(&self, c: &ViewContext<Self>) -> Vec<View> {
        vec![
            heading("Notifications").into(),
            self.toggle("notifyComplete", "Download complete", None, c),
            self.toggle("notifyFailure", "Download failed", None, c),
            self.toggle("notifyQueueFinished", "Queue finished", None, c),
        ]
    }

    fn advanced(&self, c: &ViewContext<Self>) -> Vec<View> {
        let mut rows = vec![
            heading("Save locations").into(),
            row(
                "Download folder",
                Some("Each category saves to a subfolder of this folder unless you choose another under File Types."),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(10.0).children((
                    text(s(&self.draft, "defaultFolder")),
                    Button::new().content("Choose…").on_click(c.callback(|()| Msg::Browse(None, "defaultFolder"))),
                )),
            ),
            heading("Network").into(),
            self.text_field("userAgent", "User agent", c),
            self.number("maxRetries", "Retries per connection", 0.0, 1000.0, None, c),
            self.number("retryDelaySecs", "Retry delay (seconds)", 0.0, 3600.0, None, c),
            self.number("timeoutSecs", "Timeout (seconds)", 1.0, 3600.0, None, c),
            self.number("minSegmentKb", "Minimum segment size (KB)", 1.0, 1_048_576.0, None, c),
            self.number("extensionPort", "Extension port", 1.0, 65535.0, None, c),
            heading("Security").into(),
            self.toggle("verifyTls", "Verify TLS certificates", None, c),
            self.toggle("allowInsecureRedirects", "Allow redirects from https to http", Some("Keep this off. A secure download that a server sends on to a plain http address fails instead of continuing unprotected."), c),
        ];
        if !b_or(&self.draft, "verifyTls", true) {
            rows.push(error_text("Certificate checks are off. Anyone on the network path can alter what you download.").into());
        }
        rows
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        // pending edits are saved right away
        self.save();
    }
}

impl Component for SettingsWindow {
    type Input = Shared;
    type Message = Msg;

    fn create(input: &Self::Input, _: &ComponentContext<Self>) -> Self {
        let settings = input.store.borrow().settings.clone();
        SettingsWindow { shared: input.clone(), page: "general".into(), draft: settings.clone(), base: settings, texts: HashMap::new(), generation: 0, timer: None }
    }

    fn input_changed(&mut self, input: &Self::Input, _: &ComponentContext<Self>) {
        let changed = input.list_revision != self.shared.list_revision;
        self.shared = input.clone();
        if changed {
            // settings changed elsewhere: take them without losing edits here
            let latest = self.shared.store.borrow().settings.clone();
            self.draft = merge(&self.base, &self.draft, &latest);
            self.base = latest;
        }
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Page(p) => self.page = p,
            Msg::Set(key, value) => self.put(key, value, context),
            Msg::Speed(key, text) => {
                // text that isn't a usable speed is kept as typed but not stored
                if let Some(bps) = fmt::limit_bps(&text) {
                    self.put(key, bps.into(), context);
                }
                self.texts.insert(key.to_string(), text);
            }
            Msg::Settled(generation) => {
                if generation == self.generation {
                    self.save();
                }
            }
            Msg::PauseCapture(on) => {
                let until = if on { json!(now_ms() + 600_000) } else { Value::Null };
                self.put("capturePausedUntil", until, context);
            }
            Msg::CaptureTypes(text) => {
                self.put("captureExtensions", parse_extensions(&text).into(), context);
                self.texts.insert("captureExtensions".into(), text);
            }
            Msg::ShowExtension => match system::extension_dir() {
                Some(dir) => {
                    if let Err(e) = system::open_path(&dir) {
                        self.shared.report(e);
                    }
                }
                None => self.shared.report("The extension isn't bundled in this build. Run extension/build.sh in the source folder and load extension/dist/chromium."),
            },
            Msg::Pairing(fresh) => {
                let code = crate::engine::call(if fresh { "regenerate_bridge_key" } else { "bridge_key" }, Value::Null);
                match code.as_ref().map(|v| v.as_str().unwrap_or_default()) {
                    Ok(code) if !code.is_empty() => {
                        system::set_clipboard_text(code);
                        if fresh {
                            self.shared.report("New pairing key copied. Paste it into the extension in each browser; the old key no longer works.");
                        }
                    }
                    Ok(_) => self.shared.report("The pairing key isn't available."),
                    Err(e) => self.shared.report(e.clone()),
                }
            }
            Msg::Browse(category, key) => {
                let start = match &category {
                    Some(id) => self.categories().iter().find(|c| s(c, "id") == id).map(|c| c.get("folder").and_then(Value::as_str).map(String::from).unwrap_or_else(|| self.default_folder(c))).unwrap_or_default(),
                    None => s(&self.draft, key).to_string(),
                };
                context.spawn_background(move |_| Msg::Picked(category, key, system::pick_folder(&start)));
            }
            Msg::Picked(_, _, None) => {}
            Msg::Picked(None, key, Some(path)) => self.put(key, path.into(), context),
            Msg::Picked(Some(id), _, Some(path)) => self.edit_category(&id, |c| c["folder"] = path.clone().into(), context),
            Msg::CategoryName(id, name) => {
                if !name.trim().is_empty() {
                    self.edit_category(&id, |c| c["name"] = name.trim().into(), context);
                }
            }
            Msg::CategoryTypes(id, text) => {
                self.texts.insert(format!("types:{id}"), text);
            }
            Msg::ApplyTypes(id) => {
                let Some(text) = self.texts.remove(&format!("types:{id}")) else { return };
                let mut cats = self.categories();
                // an extension belongs to one category: taking it here removes it elsewhere
                set_category_extensions(&mut cats, &id, &parse_extensions(&text));
                self.put("categories", cats.into(), context);
            }
            Msg::CategoryDefaultFolder(id) => self.edit_category(&id, |c| c["folder"] = Value::Null, context),
            Msg::RemoveCategory(id) => {
                let cats: Vec<Value> = self.categories().into_iter().filter(|c| s(c, "id") != id).collect();
                self.put("categories", cats.into(), context);
            }
            Msg::AddCategory => {
                let mut cats = self.categories();
                let id = format!("c{:06x}", now_ms() as u64 & 0xff_ffff);
                let at = cats.iter().position(|c| s(c, "id") == "general").unwrap_or(cats.len());
                cats.insert(at, json!({ "id": id, "name": "New Category", "icon": "cat-general", "extensions": [], "folder": null }));
                self.put("categories", cats.into(), context);
            }
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(self.shared.visuals(940.0, 680.0));
        let rows = match self.page.as_str() {
            "connection" => self.connection(context),
            "browser" => self.browser(context),
            "types" => self.file_types(context),
            "proxy" => self.proxy(context),
            "notifications" => self.notifications(context),
            "advanced" => self.advanced(context),
            _ => self.general(context),
        };
        let items: Vec<View> = PAGES
            .iter()
            .map(|(key, title, symbol)| NavigationViewItem::new().tag(key).icon(*symbol).is_selected(self.page == *key).content(*title).into())
            .collect();
        let nav = NavigationView::new()
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .open_pane_length(220.0)
            .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
            .is_pane_toggle_button_visible(false)
            .is_settings_visible(false)
            .menu_items(items)
            .on_selected_tag_changed(context.callback(|tag: Option<Rc<str>>| Msg::Page(tag.map(|t| t.to_string()).unwrap_or_else(|| "general".into()))))
            .content(ScrollViewer::new().content(StackPanel::new().max_width(760.0).horizontal_alignment(HorizontalAlignment::Left).margin(Thickness::new(24.0, 8.0, 24.0, 24.0)).children(rows)));
        context.window_frame("Settings", nav)
    }
}
