//! The Settings window. Changes apply as they are made: edits go into a
//! draft that is merged with whatever changed elsewhere (menu bar, toolbar,
//! the browser extension) and saved after a short pause.
use crate::app::app;
use crate::kit::{self, mtm, ns, KeyTable, TableCallbacks};
use crate::{engine, system};
use objc2::rc::Retained;
use objc2_app_kit::*;
use odm_ui::settings::{merge, parse_extensions, set_category_extensions};
use odm_ui::store::Change;
use odm_ui::{b, fmt, n, s, strings};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// A control and the setting it edits.
struct Binding {
    /// control → settings
    pull: Box<dyn Fn(&mut Value)>,
    /// settings → control
    push: Box<dyn Fn(&Value)>,
}

/// Builds controls bound to settings keys.
struct Builder {
    weak: Weak<SettingsWindow>,
    bindings: RefCell<Vec<Binding>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Builder {
    fn edited(&self) -> impl Fn() + Clone + 'static {
        kit::bound(&self.weak, |w| w.edited())
    }

    fn bind(&self, pull: impl Fn(&mut Value) + 'static, push: impl Fn(&Value) + 'static) {
        self.bindings.borrow_mut().push(Binding { pull: Box::new(pull), push: Box::new(push) });
    }

    fn check(&self, key: &'static str, title: &str) -> Retained<NSButton> {
        let c = kit::checkbox(title, self.edited());
        let (a, z) = (c.clone(), c.clone());
        self.bind(move |v| v[key] = kit::is_on(&a).into(), move |v| kit::set_on(&z, b(v, key)));
        c
    }

    fn text(&self, key: &'static str, placeholder: &str) -> Retained<NSTextField> {
        let t = kit::text_field(placeholder, self.edited());
        let (a, z) = (t.clone(), t.clone());
        self.bind(
            move |v| v[key] = kit::text_of(&a).into(),
            move |v| {
                if kit::text_of(&z) != s(v, key) {
                    kit::set_text(&z, s(v, key));
                }
            },
        );
        t
    }

    fn secret(&self, key: &'static str) -> Retained<NSSecureTextField> {
        let t = kit::secure_field("", self.edited());
        let (a, z) = (t.clone(), t.clone());
        self.bind(
            move |v| v[key] = kit::text_of(&a).into(),
            move |v| {
                if kit::text_of(&z) != s(v, key) {
                    kit::set_text(&z, s(v, key));
                }
            },
        );
        t
    }

    /// A whole number; text that isn't one leaves the setting alone.
    fn number(&self, key: &'static str, max: u64) -> Retained<NSTextField> {
        let t = kit::text_field("", self.edited());
        kit::width(&t, 90.0);
        unsafe { t.setAlignment(NSTextAlignment::Right) };
        let (a, z) = (t.clone(), t.clone());
        self.bind(
            move |v| {
                if let Ok(x) = kit::text_of(&a).trim().parse::<u64>() {
                    v[key] = x.min(max).into();
                }
            },
            move |v| {
                if kit::text_of(&z).trim().parse::<u64>().ok() != Some(n(v, key)) {
                    kit::set_text(&z, &n(v, key).to_string());
                }
            },
        );
        t
    }

    fn choice(&self, key: &'static str, options: &'static [(&'static str, &'static str)]) -> Retained<NSPopUpButton> {
        let titles: Vec<String> = options.iter().map(|o| o.1.to_string()).collect();
        let p = kit::popup(&titles, self.edited());
        let (a, z) = (p.clone(), p.clone());
        self.bind(
            move |v| {
                if let Some(o) = options.get(kit::popup_index(&a)) {
                    v[key] = o.0.into();
                }
            },
            move |v| {
                if let Some(i) = options.iter().position(|o| o.0 == s(v, key)) {
                    unsafe { z.selectItemAtIndex(i as isize) };
                }
            },
        );
        p
    }

    /// A speed limit: MB/s and whether it is on.
    fn limit(&self, bps: &'static str, enabled: &'static str) -> Retained<NSStackView> {
        let t = kit::text_field("MB/s", self.edited());
        kit::width(&t, 70.0);
        unsafe { t.setAlignment(NSTextAlignment::Right) };
        let on = kit::toggle(self.edited());
        let (a, z) = (t.clone(), t.clone());
        let (on_a, on_z) = (on.clone(), on.clone());
        self.bind(
            move |v| {
                // text that isn't a usable speed (NaN, inf, 0) is not stored
                if let Some(x) = fmt::limit_bps(&kit::text_of(&a)) {
                    v[bps] = x.into();
                }
                v[enabled] = kit::switch_on(&on_a).into();
            },
            move |v| {
                if fmt::limit_bps(&kit::text_of(&z)) != Some(n(v, bps)) {
                    kit::set_text(&z, &fmt::mbps(n(v, bps)));
                }
                kit::set_switch(&on_z, b(v, enabled));
            },
        );
        kit::hstack(8.0, &[&t, &kit::dim("MB/s"), &on])
    }
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    draft: RefCell<Value>,
    /// the engine's settings the draft is based on: a field whose draft
    /// value still equals this one was not touched here
    base: RefCell<Value>,
    bindings: Vec<Binding>,
    /// bumped by every edit, so only the last one of a burst saves
    generation: Cell<u64>,

    bridge: Retained<NSTextField>,
    bridge_error: Retained<NSTextField>,
    pairing_note: Retained<NSTextField>,

    category: RefCell<String>,
    categories: Retained<KeyTable>,
    remove_category: Retained<NSButton>,
    category_editor: Retained<NSStackView>,
    category_name: Retained<NSTextField>,
    category_folder: Retained<NSTextField>,
    use_default: Retained<NSButton>,
    general_note: Retained<NSTextField>,
    types_box: Retained<NSStackView>,
    tags: Retained<NSStackView>,
    new_types: Retained<NSTextField>,
    types_note: Retained<NSTextField>,
}

const THEMES: [(&str, &str); 3] = [("dark", "Dark"), ("light", "Light"), ("system", "Match System")];
const BYPASS: [(&str, &str); 4] = [("alt", "Option"), ("shift", "Shift"), ("ctrl", "Control"), ("none", "None")];
const PROXY: [(&str, &str); 4] = [("none", "No proxy"), ("system", "Use system settings"), ("http", "HTTP proxy"), ("socks5", "SOCKS5 proxy")];

fn pane(views: &[&NSView]) -> Retained<NSView> {
    let column = kit::vfill_in(10.0, (18.0, 20.0, 18.0, 20.0), views);
    kit::view(&kit::scrolling(&column))
}

fn tab(tabs: &NSTabView, title: &str, content: &NSView) {
    let item = NSTabViewItem::new();
    unsafe {
        item.setLabel(&ns(title));
        item.setView(Some(content));
        tabs.addTabViewItem(&item);
    }
}

impl SettingsWindow {
    pub fn new() -> Rc<SettingsWindow> {
        Rc::new_cyclic(|weak: &Weak<SettingsWindow>| {
            let window = kit::window("Settings", 700.0, 560.0, (700.0, 560.0), false);
            let build = Builder { weak: weak.clone(), bindings: RefCell::new(vec![]) };
            let tabs = NSTabView::new(mtm());

            // General
            let theme = build.choice("theme", &THEMES);
            tab(
                &tabs,
                "General",
                &pane(&[
                    &build.check("launchAtLogin", "Open at login"),
                    &build.check("minimizeToTray", "Keep running in the menu bar when the window is closed"),
                    &build.check("clipboardMonitor", "Watch the clipboard for download links"),
                    &kit::note("Copying a link to a file type listed under Browser opens the Add window."),
                    &build.check("showProgressOnStart", "Show the progress window when a download starts"),
                    &build.check("showCompleteDialog", "Show the download window when a download completes"),
                    &build.check("confirmDelete", "Confirm before deleting downloads"),
                    &kit::hline(),
                    &kit::heading("Appearance"),
                    &kit::row("Theme", &theme),
                    &build.check("traySpeed", "Show download speed in the menu bar"),
                ]),
            );

            // Connection
            let connections = unsafe { NSSlider::sliderWithValue_minValue_maxValue_target_action(16.0, 1.0, 32.0, None, None, mtm()) };
            kit::on_action(&connections, build.edited());
            kit::width(&connections, 200.0);
            let connections_label = kit::label("16");
            kit::digits(&connections_label);
            {
                let (a, z, l) = (connections.clone(), connections.clone(), connections_label.clone());
                build.bind(
                    move |v| v["defaultConnections"] = (unsafe { a.doubleValue() }.round().clamp(1.0, 32.0) as u64).into(),
                    move |v| {
                        unsafe { z.setDoubleValue(n(v, "defaultConnections") as f64) };
                        kit::set_text(&l, &n(v, "defaultConnections").to_string());
                    },
                );
            }
            let simultaneous = NSStepper::new(mtm());
            unsafe {
                simultaneous.setMinValue(1.0);
                simultaneous.setMaxValue(64.0);
                simultaneous.setValueWraps(false);
            }
            kit::on_action(&simultaneous, build.edited());
            let simultaneous_label = kit::label("4");
            kit::digits(&simultaneous_label);
            {
                let (a, z, l) = (simultaneous.clone(), simultaneous.clone(), simultaneous_label.clone());
                build.bind(
                    move |v| v["maxSimultaneous"] = (unsafe { a.integerValue() }.clamp(1, 64) as u64).into(),
                    move |v| {
                        unsafe { z.setIntegerValue(n(v, "maxSimultaneous") as isize) };
                        kit::set_text(&l, &n(v, "maxSimultaneous").to_string());
                    },
                );
            }
            tab(
                &tabs,
                "Connection",
                &pane(&[
                    &kit::row("Connections per download", &kit::hstack(8.0, &[&connections, &connections_label])),
                    &kit::note("Files are split dynamically: a connection that finishes takes over half of the largest remaining piece."),
                    &kit::row("Simultaneous downloads", &kit::hstack(6.0, &[&simultaneous_label, &simultaneous])),
                    &kit::note("Outside of queues. Each queue sets its own limit."),
                    &kit::hline(),
                    &kit::heading("Speed limits"),
                    &kit::row("All downloads combined", &build.limit("globalLimitBps", "globalLimitEnabled")),
                    &kit::row("Per download", &build.limit("perDownloadLimitBps", "perDownloadLimitEnabled")),
                    &build.check("limitOnStartup", "Turn on the global limit when the app starts"),
                ]),
            );

            // Browser
            let bridge = kit::label("");
            let bridge_error = kit::note("");
            kit::red(&bridge_error);
            let pairing_note = kit::note("");
            let copy = kit::button("Copy Key", kit::bound(weak, |w| w.copy_pairing_key(false)));
            let new_key = kit::button("New Key…", kit::bound(weak, |w| w.copy_pairing_key(true)));
            let paused = kit::checkbox("Pause capture for 10 minutes", build.edited());
            {
                let (a, z) = (paused.clone(), paused.clone());
                let is_paused = |v: &Value| v.get("capturePausedUntil").and_then(Value::as_i64).map(|t| t > now_ms()).unwrap_or(false);
                build.bind(
                    move |v| {
                        if kit::is_on(&a) != is_paused(v) {
                            v["capturePausedUntil"] = if kit::is_on(&a) { (now_ms() + 600_000).into() } else { Value::Null };
                        }
                    },
                    move |v| kit::set_on(&z, is_paused(v)),
                );
            }
            let (types_scroll, types_text) = kit::text_area(build.edited());
            kit::min_height(&types_scroll, 80.0);
            {
                let (a, z) = (types_text.clone(), types_text.clone());
                build.bind(
                    move |v| {
                        let list = parse_extensions(&kit::text_view_string(&a));
                        if list != strings(v, "captureExtensions") {
                            v["captureExtensions"] = list.into();
                        }
                    },
                    move |v| {
                        if parse_extensions(&kit::text_view_string(&z)) != strings(v, "captureExtensions") {
                            unsafe { z.setString(&ns(&strings(v, "captureExtensions").join(" "))) };
                        }
                    },
                );
            }
            let how = "Chrome, Edge, Brave, Arc, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json.";
            tab(
                &tabs,
                "Browser",
                &pane(&[
                    &kit::row("Extension", &bridge),
                    &bridge_error,
                    &kit::note(how),
                    &kit::hstack(0.0, &[&kit::button("Show Extension Folder", kit::bound(weak, |w| w.reveal_extension())), &kit::spacer()]),
                    &kit::row("Pairing key", &kit::hstack(8.0, &[&copy, &new_key, &pairing_note])),
                    &kit::note("Paste it into the extension’s popup, under Connection, and save it under Pairing key. Only an extension that holds the key can talk to the app, and other programs can’t read what it sends. A new key unpairs every browser."),
                    &kit::hline(),
                    &kit::heading("Capture"),
                    &build.check("captureEnabled", "Take over browser downloads"),
                    &paused,
                    &kit::row("Bypass key", &kit::hstack(8.0, &[&build.choice("bypassKey", &BYPASS), &kit::dim("hold while clicking")])),
                    &kit::row("Skip files smaller than", &kit::hstack(8.0, &[&build.number("captureMinSizeKb", u32::MAX as u64), &kit::dim("KB")])),
                    &kit::heading("File types to capture"),
                    &types_scroll,
                ]),
            );

            // File Types
            let (category_scroll, categories) = kit::table(
                &[("name", "", 150.0, 100.0, false)],
                36.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.category_list().len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |_, _, row| {
                            let c = w.upgrade()?.category_list().get(row)?.clone();
                            let kinds = if s(&c, "id") == "general" { "Everything else".to_string() } else { format!("{} types", strings(&c, "extensions").len()) };
                            let lines = kit::vstack(1.0, &[&kit::label(s(&c, "name")), &kit::note(&kinds)]);
                            kit::pad(&lines, 2.0, 4.0, 2.0, 4.0);
                            Some(kit::view(&lines))
                        }
                    }),
                    selection_changed: Box::new(kit::bound(weak, |w| w.category_selected())),
                    ..Default::default()
                },
            );
            unsafe {
                category_scroll.setBorderType(NSBorderType::BezelBorder);
                category_scroll.setHasHorizontalScroller(false);
                categories.setAllowsEmptySelection(false);
            }
            category_scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let add_category = kit::icon_button("plus", "Add a category", kit::bound(weak, |w| w.add_category()));
            let remove_category = kit::icon_button("minus", "Remove this category", kit::bound(weak, |w| w.remove_category()));
            let list_column = kit::vfill(6.0, &[&category_scroll, &kit::hstack(10.0, &[&add_category, &remove_category, &kit::spacer()])]);
            kit::width(&list_column, 180.0);

            let category_name = kit::text_field("Name", kit::bound(weak, |w| w.category_renamed()));
            let category_folder = kit::label("");
            kit::mono(&category_folder);
            kit::stretch_view(&category_folder);
            let choose = kit::button("Choose…", kit::bound(weak, |w| w.choose_category_folder()));
            let use_default = kit::button("Use Default", kit::bound(weak, |w| w.change_category(|c| c["folder"] = Value::Null)));
            let general_note = kit::note("General holds every file type no other category claims, and saves to your download folder.");
            let tags = kit::vfill(4.0, &[]);
            let new_types = kit::text_field("Add types (e.g. mkv webm)", || {});
            kit::on_action(&new_types, kit::bound(weak, |w| w.add_types()));
            let add_types = kit::button("Add", kit::bound(weak, |w| w.add_types()));
            let types_note = kit::note("");
            let types_box = kit::vfill(8.0, &[&kit::heading("File types"), &tags, &kit::hstack(8.0, &[&new_types, &add_types]), &types_note]);
            let category_editor = kit::vfill(
                10.0,
                &[&kit::row("Name", &category_name), &kit::row("Folder", &kit::hstack(8.0, &[&category_folder, &choose, &use_default])), &general_note, &types_box, &kit::spacer()],
            );
            kit::stretch(&category_editor, true);
            let file_types = kit::hstack(16.0, &[&list_column, &category_editor]);
            unsafe {
                file_types.setAlignment(NSLayoutAttribute::Top);
                list_column.heightAnchor().constraintEqualToAnchor_constant(&file_types.heightAnchor(), -36.0).setActive(true);
            }
            kit::pad(&file_types, 18.0, 20.0, 18.0, 20.0);
            tab(&tabs, "File Types", &file_types);

            // Proxy
            let proxy_fields = kit::vfill(
                10.0,
                &[
                    &kit::row("Host", &build.text("proxyHost", "")),
                    &kit::row("Port", &kit::hstack(0.0, &[&build.number("proxyPort", 65535), &kit::spacer()])),
                    &kit::row("Username", &build.text("proxyUser", "")),
                    &kit::row("Password", &build.secret("proxyPass")),
                ],
            );
            {
                let fields = proxy_fields.clone();
                build.bind(|_| {}, move |v| kit::show(&fields, matches!(s(v, "proxyMode"), "http" | "socks5")));
            }
            tab(&tabs, "Proxy", &pane(&[&kit::row("Connection", &build.choice("proxyMode", &PROXY)), &proxy_fields]));

            // Notifications
            tab(
                &tabs,
                "Notifications",
                &pane(&[&build.check("notifyComplete", "Download complete"), &build.check("notifyFailure", "Download failed"), &build.check("notifyQueueFinished", "Queue finished")]),
            );

            // Advanced
            let folder = kit::label("");
            kit::mono(&folder);
            kit::stretch_view(&folder);
            {
                let folder = folder.clone();
                build.bind(|_| {}, move |v| kit::set_text(&folder, s(v, "defaultFolder")));
            }
            let choose_folder = kit::button("Choose…", kit::bound(weak, |w| {
                let start = s(&w.draft.borrow(), "defaultFolder").to_string();
                if let Some(dir) = kit::choose_folder(&start) {
                    w.draft.borrow_mut()["defaultFolder"] = dir.into();
                    w.commit();
                }
            }));
            let tls_warning = kit::note("⚠ Certificate checks are off. Anyone on the network path can alter what you download.");
            kit::red(&tls_warning);
            {
                let warning = tls_warning.clone();
                build.bind(|_| {}, move |v| kit::show(&warning, !odm_ui::b_or(v, "verifyTls", true)));
            }
            let numbers = |key: &'static str, max: u64| kit::hstack(0.0, &[&build.number(key, max), &kit::spacer()]);
            tab(
                &tabs,
                "Advanced",
                &pane(&[
                    &kit::heading("Save locations"),
                    &kit::row("Download folder", &kit::hstack(8.0, &[&folder, &choose_folder])),
                    &kit::note("Each category saves to a subfolder of this folder unless you choose another under File Types."),
                    &kit::hline(),
                    &kit::heading("Network"),
                    &kit::row("User agent", &build.text("userAgent", "")),
                    &kit::row("Retries per connection", &numbers("maxRetries", 1000)),
                    &kit::row("Retry delay (seconds)", &numbers("retryDelaySecs", 86_400)),
                    &kit::row("Timeout (seconds)", &numbers("timeoutSecs", 86_400)),
                    &kit::row("Minimum segment (KB)", &numbers("minSegmentKb", u32::MAX as u64)),
                    &kit::row("Extension port", &numbers("extensionPort", 65535)),
                    &kit::hline(),
                    &kit::heading("Security"),
                    &build.check("verifyTls", "Verify TLS certificates"),
                    &tls_warning,
                    &build.check("allowInsecureRedirects", "Allow redirects from https to http"),
                    &kit::note("Keep this off. A secure download that a server sends on to a plain http address fails instead of continuing unprotected."),
                ]),
            );

            let root = NSView::new(mtm());
            unsafe {
                tabs.setTranslatesAutoresizingMaskIntoConstraints(false);
                root.addSubview(&tabs);
                tabs.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 12.0).setActive(true);
                tabs.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -12.0).setActive(true);
                tabs.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 12.0).setActive(true);
                tabs.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -12.0).setActive(true);
                window.setContentView(Some(&root));
                window.setContentSize(objc2_foundation::NSSize::new(700.0, 560.0));
            }

            SettingsWindow {
                window,
                draft: RefCell::new(Value::Null),
                base: RefCell::new(Value::Null),
                bindings: build.bindings.into_inner(),
                generation: Cell::new(0),
                bridge,
                bridge_error,
                pairing_note,
                category: RefCell::new(String::new()),
                categories,
                remove_category,
                category_editor,
                category_name,
                category_folder,
                use_default,
                general_note,
                types_box,
                tags,
                new_types,
                types_note,
            }
        })
    }

    pub fn show(self: &Rc<Self>) {
        if !unsafe { self.window.isVisible() } {
            let latest = app().store.borrow().settings.clone();
            *self.draft.borrow_mut() = latest.clone();
            *self.base.borrow_mut() = latest;
            kit::set_text(&self.pairing_note, "");
            self.push();
        }
        kit::present(&self.window);
    }

    /// Show the draft in the controls.
    fn push(self: &Rc<Self>) {
        {
            let draft = self.draft.borrow();
            for binding in &self.bindings {
                (binding.push)(&draft);
            }
        }
        self.show_bridge();
        self.show_categories();
    }

    /// A control changed: read them all, then save after a pause.
    fn edited(self: &Rc<Self>) {
        {
            let mut draft = self.draft.borrow_mut();
            for binding in &self.bindings {
                (binding.pull)(&mut draft);
            }
        }
        self.commit();
    }

    /// The draft changed: show what follows from it and save after a pause
    /// in editing.
    fn commit(self: &Rc<Self>) {
        self.push();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        kit::after(0.4, move || {
            let Some(w) = weak.upgrade() else { return };
            if w.generation.get() != generation {
                return;
            }
            let app = app();
            // changes made elsewhere since the draft was taken come along
            let latest = app.store.borrow().settings.clone();
            let merged = merge(&w.base.borrow(), &w.draft.borrow(), &latest);
            app.save_settings(merged);
            // what was stored (the login item follows the system)
            let stored = app.store.borrow().settings.clone();
            *w.draft.borrow_mut() = stored.clone();
            *w.base.borrow_mut() = stored;
            w.push();
        });
    }

    /// Settings or the browser bridge changed elsewhere.
    pub fn store_changed(self: &Rc<Self>, change: Change) {
        if !unsafe { self.window.isVisible() } {
            return;
        }
        if change == Change::Settings {
            // take the change without losing edits in progress
            let latest = app().store.borrow().settings.clone();
            let merged = merge(&self.base.borrow(), &self.draft.borrow(), &latest);
            *self.draft.borrow_mut() = merged;
            *self.base.borrow_mut() = latest;
            self.push();
        } else {
            self.show_bridge();
        }
    }

    // -- browser -------------------------------------------------------------

    fn show_bridge(&self) {
        let app = app();
        let st = app.store.borrow();
        let error = st.ext.get("bridgeError").and_then(Value::as_str);
        let (text, color) = match (error, b(&st.ext, "connected")) {
            (Some(_), _) => ("Not listening".to_string(), unsafe { NSColor::systemRedColor() }),
            (None, true) => (format!("Connected: {}", strings(&st.ext, "browsers").join(", ")), unsafe { NSColor::systemGreenColor() }),
            (None, false) => ("Not detected".to_string(), unsafe { NSColor::tertiaryLabelColor() }),
        };
        unsafe { self.bridge.setAttributedStringValue(&kit::dotted(&text, &color)) };
        kit::show(&self.bridge_error, error.is_some());
        if let Some(e) = error {
            kit::set_text(&self.bridge_error, &format!("{e}. Another app may be using this port; choose a different one under Advanced."));
        }
    }

    /// Put the pairing key (a new one when asked) on the clipboard.
    fn copy_pairing_key(&self, new: bool) {
        if new {
            let text = "Every browser paired with the current key stops sending downloads to the app until you paste the new key into its extension. The new key is copied to the clipboard.";
            if kit::ask(None, "Make a new pairing key?", text, &["New Key", "Cancel"], None) != 0 {
                return;
            }
        }
        match engine::call(if new { "regenerate_bridge_key" } else { "bridge_key" }, json!({})) {
            Ok(key) => {
                system::copy_secret(key.as_str().unwrap_or(""));
                kit::set_text(&self.pairing_note, if new { "New key copied. Pair each browser again." } else { "Copied." });
            }
            Err(e) => app().report(&e),
        }
    }

    fn reveal_extension(&self) {
        match system::resource("extension/chromium") {
            Some(dir) => system::reveal(&dir),
            None => app().report("The extension isn't bundled in this build. Run extension/build.sh in the source folder and load extension/dist/chromium."),
        }
    }

    // -- file types ----------------------------------------------------------

    fn category_list(&self) -> Vec<Value> {
        self.draft.borrow().get("categories").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    fn current_category(&self) -> Option<Value> {
        let id = self.category.borrow().clone();
        self.category_list().into_iter().find(|c| s(c, "id") == id)
    }

    /// Change the selected category in the draft.
    fn change_category(self: &Rc<Self>, f: impl FnOnce(&mut Value)) {
        let id = self.category.borrow().clone();
        {
            let mut draft = self.draft.borrow_mut();
            let found = draft.get_mut("categories").and_then(Value::as_array_mut).and_then(|list| list.iter_mut().find(|c| s(c, "id") == id));
            match found {
                Some(c) => f(c),
                None => return,
            }
        }
        self.commit();
    }

    fn default_folder(&self, c: &Value) -> String {
        let draft = self.draft.borrow();
        let base = s(&draft, "defaultFolder");
        if s(c, "id") == "general" {
            base.to_string()
        } else {
            std::path::Path::new(base).join(s(c, "name").replace('/', "_")).to_string_lossy().into_owned()
        }
    }

    fn show_categories(self: &Rc<Self>) {
        let list = self.category_list();
        if !list.iter().any(|c| s(c, "id") == *self.category.borrow()) {
            *self.category.borrow_mut() = list.first().map(|c| s(c, "id").to_string()).unwrap_or_default();
        }
        self.categories.reload();
        if let Some(row) = list.iter().position(|c| s(c, "id") == *self.category.borrow()) {
            self.categories.select(&[row]);
        }
        let current = self.current_category();
        kit::show(&self.category_editor, current.is_some());
        let Some(c) = current else { return };
        let general = s(&c, "id") == "general";
        kit::enable(&self.remove_category, !general);
        if kit::text_of(&self.category_name) != s(&c, "name") {
            kit::set_text(&self.category_name, s(&c, "name"));
        }
        kit::enable(&self.category_name, !general);
        let custom = c.get("folder").and_then(Value::as_str);
        kit::set_text(&self.category_folder, &custom.map(String::from).unwrap_or_else(|| self.default_folder(&c)));
        unsafe { self.category_folder.setTextColor(Some(&*if custom.is_some() { NSColor::labelColor() } else { NSColor::secondaryLabelColor() })) };
        kit::show(&self.use_default, custom.is_some());
        kit::show(&self.general_note, general);
        kit::show(&self.types_box, !general);
        kit::show(&self.types_note, !kit::text_of(&self.types_note).is_empty());

        // one removable tag per file type, seven to a line
        for old in unsafe { self.tags.arrangedSubviews() }.iter() {
            unsafe { old.removeFromSuperview() };
        }
        for line in strings(&c, "extensions").chunks(7) {
            let row = kit::hstack(6.0, &[]);
            for ext in line {
                let tag = kit::button(&format!(".{ext} ✕"), {
                    let (weak, ext) = (Rc::downgrade(self), ext.clone());
                    move || {
                        if let Some(w) = weak.upgrade() {
                            let ext = ext.clone();
                            w.change_category(move |c| c["extensions"] = strings(c, "extensions").into_iter().filter(|e| *e != ext).collect::<Vec<_>>().into());
                        }
                    }
                });
                unsafe {
                    tag.setBezelStyle(NSBezelStyle::Badge);
                    tag.setToolTip(Some(&ns(&format!("Remove .{ext}"))));
                    row.addArrangedSubview(&tag);
                }
            }
            unsafe { row.addArrangedSubview(&kit::spacer()) };
            kit::add_row(&self.tags, &row);
        }
    }

    fn category_selected(self: &Rc<Self>) {
        if self.categories.loading() {
            return;
        }
        let id = self.categories.selected().first().and_then(|row| self.category_list().get(*row).map(|c| s(c, "id").to_string()));
        if let Some(id) = id {
            *self.category.borrow_mut() = id;
            kit::set_text(&self.types_note, "");
            self.show_categories();
        }
    }

    fn category_renamed(self: &Rc<Self>) {
        let name = kit::text_of(&self.category_name);
        self.change_category(move |c| c["name"] = name.into());
    }

    fn choose_category_folder(self: &Rc<Self>) {
        let Some(c) = self.current_category() else { return };
        let start = c.get("folder").and_then(Value::as_str).map(String::from).unwrap_or_else(|| self.default_folder(&c));
        if let Some(dir) = kit::choose_folder(&start) {
            self.change_category(move |c| c["folder"] = dir.into());
        }
    }

    /// Add file types to the selected category; a type belongs to one
    /// category, so it leaves the one that had it.
    fn add_types(self: &Rc<Self>) {
        let added = parse_extensions(&kit::text_of(&self.new_types));
        kit::set_text(&self.new_types, "");
        let id = self.category.borrow().clone();
        if added.is_empty() {
            return;
        }
        let mut list = self.category_list();
        let mut moved = vec![];
        for c in list.iter().filter(|c| s(c, "id") != id) {
            for e in strings(c, "extensions").iter().filter(|e| added.contains(e)) {
                moved.push(format!(".{e} moved from {}", s(c, "name")));
            }
        }
        let mut all = list.iter().find(|c| s(c, "id") == id).map(|c| strings(c, "extensions")).unwrap_or_default();
        for e in added {
            if !all.contains(&e) {
                all.push(e);
            }
        }
        set_category_extensions(&mut list, &id, &all);
        self.draft.borrow_mut()["categories"] = list.into();
        kit::set_text(&self.types_note, &moved.join(" · "));
        self.commit();
    }

    fn add_category(self: &Rc<Self>) {
        let id = format!("c{:06x}", now_ms() & 0xff_ffff);
        let mut list = self.category_list();
        let at = list.iter().position(|c| s(c, "id") == "general").unwrap_or(list.len());
        list.insert(at, json!({ "id": id, "name": "New Category", "icon": "cat-general", "extensions": [], "folder": null }));
        self.draft.borrow_mut()["categories"] = list.into();
        *self.category.borrow_mut() = id;
        self.commit();
    }

    fn remove_category(self: &Rc<Self>) {
        let id = self.category.borrow().clone();
        if id == "general" {
            return;
        }
        let list: Vec<Value> = self.category_list().into_iter().filter(|c| s(c, "id") != id).collect();
        self.draft.borrow_mut()["categories"] = list.into();
        self.category.borrow_mut().clear();
        self.commit();
    }
}

#[cfg(debug_assertions)]
impl SettingsWindow {
    /// Change two settings the way their controls would (see `ODM_DO`).
    pub fn debug_edit(self: &Rc<Self>) {
        {
            let mut draft = self.draft.borrow_mut();
            draft["traySpeed"] = true.into();
            draft["theme"] = "light".into();
        }
        self.push();
        // as if a control had fired: read them all back and save
        self.edited();
    }
}
