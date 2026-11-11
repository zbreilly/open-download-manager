//! Preferences. Changes apply as you make them (saved after a short pause),
//! and settings changed elsewhere (tray, toolbar, extension) flow in without
//! losing edits in progress.
use crate::app::{odm, App, Change};
use crate::row::{b, n, s};
use crate::widgets::{self, Choice};
use adw::prelude::*;
use gtk::glib;
use odm_linux::{fmt, shell::merge_settings};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const EXTENSION_DIRS: [&str; 2] = ["/usr/share/open-download-manager/extension", "extension/dist"];

/// Puts a draft's value into one control.
type Loader = Box<dyn Fn(&Value)>;

struct Prefs {
    dialog: adw::PreferencesDialog,
    draft: RefCell<Value>,
    /// the engine's settings the draft is based on
    base: RefCell<Value>,
    /// puts the draft's values into the controls
    loaders: RefCell<Vec<Loader>>,
    quiet: Cell<bool>,
    save_generation: Cell<u32>,
    open: Cell<bool>,
    categories: adw::PreferencesGroup,
    category_rows: RefCell<Vec<adw::ExpanderRow>>,
    ext_status: adw::ActionRow,
    ext_error: gtk::Label,
    tls_warning: gtk::Label,
    proxy_rows: RefCell<Vec<gtk::Widget>>,
}

pub fn open(app: &Rc<App>) {
    thread_local! {
        static OPEN: RefCell<Option<glib::WeakRef<adw::PreferencesDialog>>> = const { RefCell::new(None) };
    }
    if OPEN.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade())).is_some() {
        return;
    }
    let Some(parent) = app.main_window() else { return };
    app.show_main();
    let dialog = build(app);
    OPEN.with(|o| *o.borrow_mut() = Some(dialog.downgrade()));
    dialog.present(Some(&parent));
}

fn build(app: &Rc<App>) -> adw::PreferencesDialog {
    let settings = app.settings.borrow().clone();
    let p = Rc::new(Prefs {
        dialog: adw::PreferencesDialog::builder().title("Preferences").content_width(720).content_height(620).build(),
        draft: RefCell::new(settings.clone()),
        base: RefCell::new(settings),
        loaders: RefCell::default(),
        quiet: Cell::new(false),
        save_generation: Cell::new(0),
        open: Cell::new(true),
        categories: adw::PreferencesGroup::builder().title("Categories").description("Each category has its own file types and folder.").build(),
        category_rows: RefCell::default(),
        ext_status: adw::ActionRow::builder().title("Extension").build(),
        ext_error: gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build(),
        tls_warning: gtk::Label::builder()
            .label("Certificate checks are off. Anyone on the network path can alter what you download.")
            .xalign(0.0)
            .wrap(true)
            .build(),
        proxy_rows: RefCell::default(),
    });
    p.ext_error.add_css_class("error");
    p.ext_error.add_css_class("caption");
    p.tls_warning.add_css_class("error");

    p.dialog.add(&p.general());
    p.dialog.add(&p.connection());
    p.dialog.add(&p.browser());
    p.dialog.add(&p.file_types());
    p.dialog.add(&p.network());
    p.dialog.add(&p.advanced());
    p.load();

    let this = p.clone();
    app.subscribe(move |c| {
        if !this.open.get() {
            return false;
        }
        match c {
            Change::Settings => this.settings_changed(),
            Change::Ext => this.show_extension(),
            _ => {}
        }
        true
    });
    let this = p.clone();
    p.dialog.connect_closed(move |_| {
        // pending edits are saved right away
        this.save_now();
        this.open.set(false);
    });
    p.dialog.clone()
}

fn page(title: &str, icon: &str) -> adw::PreferencesPage {
    adw::PreferencesPage::builder().title(title).icon_name(icon).build()
}

impl Prefs {
    // ------------------------------------------------------------- plumbing

    fn get(&self, key: &str) -> Value {
        self.draft.borrow().get(key).cloned().unwrap_or(Value::Null)
    }

    /// A control changed: put its value in the draft and save soon.
    fn put(self: &Rc<Self>, key: &str, v: Value) {
        if self.quiet.get() || self.get(key) == v {
            return;
        }
        self.draft.borrow_mut()[key] = v;
        self.after_edit();
        let generation = self.save_generation.get().wrapping_add(1);
        self.save_generation.set(generation);
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if this.save_generation.get() == generation {
                this.save_now();
            }
        });
    }

    fn save_now(&self) {
        self.save_generation.set(self.save_generation.get().wrapping_add(1));
        let app = odm();
        let latest = app.settings.borrow().clone();
        let merged = merge_settings(&self.base.borrow(), &self.draft.borrow(), &latest);
        if merged == latest {
            return;
        }
        app.save_settings(merged);
        // what was stored (e.g. launch at login adjusted to what happened)
        let stored = app.settings.borrow().clone();
        *self.draft.borrow_mut() = stored.clone();
        *self.base.borrow_mut() = stored;
        self.load();
        app.emit(Change::Settings);
    }

    /// Settings changed elsewhere: take them without losing edits here.
    fn settings_changed(&self) {
        let latest = odm().settings.borrow().clone();
        let merged = merge_settings(&self.base.borrow(), &self.draft.borrow(), &latest);
        *self.base.borrow_mut() = latest;
        if merged != *self.draft.borrow() {
            *self.draft.borrow_mut() = merged;
            self.load();
        }
    }

    fn load(&self) {
        self.quiet.set(true);
        let draft = self.draft.borrow().clone();
        for f in self.loaders.borrow().iter() {
            f(&draft);
        }
        self.quiet.set(false);
        self.after_edit();
        self.show_extension();
    }

    /// Things that depend on other values.
    fn after_edit(&self) {
        let d = self.draft.borrow();
        self.tls_warning.set_visible(!d.get("verifyTls").and_then(Value::as_bool).unwrap_or(true));
        let manual = matches!(s(&d, "proxyMode"), "http" | "socks5");
        for w in self.proxy_rows.borrow().iter() {
            w.set_visible(manual);
        }
    }

    fn switch(self: &Rc<Self>, key: &'static str, title: &str, subtitle: &str) -> adw::SwitchRow {
        let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).build();
        let this = self.clone();
        row.connect_active_notify(move |r| this.put(key, r.is_active().into()));
        let target = row.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| target.set_active(b(d, key))));
        row
    }

    fn number(self: &Rc<Self>, key: &'static str, title: &str, min: f64, max: f64) -> adw::SpinRow {
        let row = adw::SpinRow::with_range(min, max, 1.0);
        row.set_title(title);
        let this = self.clone();
        row.connect_value_notify(move |r| this.put(key, (r.value().round() as u64).into()));
        let target = row.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| target.set_value(n(d, key) as f64)));
        row
    }

    fn text(self: &Rc<Self>, key: &'static str, title: &str) -> adw::EntryRow {
        let row = adw::EntryRow::builder().title(title).build();
        let this = self.clone();
        row.connect_changed(move |r| this.put(key, r.text().to_string().into()));
        let target = row.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| {
            if target.text() != s(d, key) {
                target.set_text(s(d, key));
            }
        }));
        row
    }

    fn choice(self: &Rc<Self>, key: &'static str, title: &str, items: &[(&str, &str)]) -> Choice {
        let c = Choice::new(title, items);
        let this = self.clone();
        c.on_change(move |v| this.put(key, v.into()));
        let target = c.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| target.set_value(s(d, key))));
        c
    }

    /// A speed in MB/s with its on/off switch.
    fn speed(self: &Rc<Self>, key: &'static str, enabled: &'static str, title: &str) -> adw::EntryRow {
        let row = adw::EntryRow::builder().title(title).input_purpose(gtk::InputPurpose::Number).build();
        let on = gtk::Switch::builder().valign(gtk::Align::Center).build();
        row.add_suffix(&on);
        let this = self.clone();
        row.connect_changed(move |r| {
            // text that isn't a usable speed is ignored; the old value stays
            match fmt::limit_bps(&r.text()) {
                Some(bps) => {
                    r.remove_css_class("error");
                    this.put(key, bps.into());
                }
                None => r.add_css_class("error"),
            }
        });
        let this = self.clone();
        on.connect_active_notify(move |sw| this.put(enabled, sw.is_active().into()));
        let (target, switch) = (row.clone(), on.clone());
        self.loaders.borrow_mut().push(Box::new(move |d| {
            if fmt::limit_bps(&target.text()) != Some(n(d, key)) {
                target.set_text(&fmt::mbps(n(d, key)));
            }
            switch.set_active(b(d, enabled));
        }));
        row
    }

    fn folder(self: &Rc<Self>, key: &'static str, title: &str) -> adw::ActionRow {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_css_class("property");
        let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        row.add_suffix(&choose);
        let this = self.clone();
        choose.connect_clicked(move |btn| {
            let (this, start) = (this.clone(), s(&this.draft.borrow(), key).to_string());
            let Some(window) = btn.root().and_downcast::<gtk::Window>() else { return };
            glib::spawn_future_local(async move {
                if let Some(path) = widgets::choose_folder(&window, &start).await {
                    this.put(key, path.into());
                    this.load();
                }
            });
        });
        let target = row.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| target.set_subtitle(&glib::markup_escape_text(s(d, key)))));
        row
    }

    // ---------------------------------------------------------------- pages

    fn general(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("General", "emblem-system-symbolic");
        let g = adw::PreferencesGroup::new();
        g.add(&self.switch("launchAtLogin", "Open at login", ""));
        g.add(&self.switch("minimizeToTray", "Keep running in the tray when the window is closed", "Needs a system tray. On GNOME that is the AppIndicator extension."));
        g.add(&self.switch("clipboardMonitor", "Watch the clipboard for download links", "Copying a link to a file type listed under Browser opens the Add window."));
        g.add(&self.switch("showProgressOnStart", "Show the progress window when a download starts", ""));
        g.add(&self.switch("showCompleteDialog", "Show the download window when a download completes", ""));
        g.add(&self.switch("confirmDelete", "Confirm before deleting downloads", ""));
        page.add(&g);
        let look = adw::PreferencesGroup::builder().title("Appearance").build();
        look.add(&self.choice("theme", "Style", &[("system", "Follow system"), ("light", "Light"), ("dark", "Dark")]).row);
        page.add(&look);
        let notes = adw::PreferencesGroup::builder().title("Notifications").build();
        notes.add(&self.switch("notifyComplete", "Download complete", ""));
        notes.add(&self.switch("notifyFailure", "Download failed", ""));
        notes.add(&self.switch("notifyQueueFinished", "Queue finished", ""));
        page.add(&notes);
        page
    }

    fn connection(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("Connection", "network-transmit-receive-symbolic");
        let g = adw::PreferencesGroup::new();
        let per = self.number("defaultConnections", "Connections per download", 1.0, 32.0);
        per.set_subtitle("Files are split dynamically: a connection that finishes takes over half of the largest remaining piece.");
        g.add(&per);
        let same = self.number("maxSimultaneous", "Simultaneous downloads", 1.0, 64.0);
        same.set_subtitle("Outside of queues. Each queue sets its own limit.");
        g.add(&same);
        page.add(&g);
        let limits = adw::PreferencesGroup::builder().title("Speed limits").build();
        limits.add(&self.speed("globalLimitBps", "globalLimitEnabled", "Global limit, all downloads combined (MB/s)"));
        limits.add(&self.speed("perDownloadLimitBps", "perDownloadLimitEnabled", "Per-download limit (MB/s)"));
        limits.add(&self.switch("limitOnStartup", "Turn on the global limit when the app starts", ""));
        page.add(&limits);
        page
    }

    fn browser(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("Browser", "web-browser-symbolic");
        let ext = adw::PreferencesGroup::builder()
            .title("Browser extension")
            .description("Chrome, Edge, Brave, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json.")
            .build();
        let show = gtk::Button::builder().label("Show Extension Folder").valign(gtk::Align::Center).build();
        show.connect_clicked(|_| {
            let found = EXTENSION_DIRS.iter().map(std::path::Path::new).find(|d| d.join("chromium").is_dir());
            match found {
                Some(dir) => {
                    if let Err(e) = odm_linux::open::reveal(&dir.join("chromium")) {
                        odm().report(&e);
                    }
                }
                None => odm().report("The extension isn't installed with this build. Run extension/build.sh in the source folder and load extension/dist/chromium."),
            }
        });
        self.ext_status.add_suffix(&show);
        ext.add(&self.ext_status);
        let pair = adw::ActionRow::builder().title("Pairing code").subtitle("Paste it into the extension’s popup, under Connection. Only a paired extension can talk to the app, and other programs can’t read what it sends.").build();
        for (label, method, done) in [("Copy", "pairing_code", "Pairing code copied."), ("New Code", "reset_pairing", "New pairing code copied. Paste it into the extension in each browser; the old code no longer works.")] {
            let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
            button.connect_clicked(move |_| match odm_ffi::api::call_as::<String>(method, Value::Null) {
                Ok(code) => {
                    if let Some(d) = gtk::gdk::Display::default() {
                        d.clipboard().set_text(&code);
                    }
                    odm().report(done);
                }
                Err(e) => odm().report(&e),
            });
            pair.add_suffix(&button);
        }
        ext.add(&pair);
        ext.add(&self.ext_error);
        page.add(&ext);

        let capture = adw::PreferencesGroup::builder().title("Capture").build();
        capture.add(&self.switch("captureEnabled", "Take over browser downloads", ""));
        let pause = adw::SwitchRow::builder().title("Pause capture for 10 minutes").build();
        let this = self.clone();
        pause.connect_active_notify(move |r| {
            let until = if r.is_active() { json!(glib::real_time() / 1000 + 600_000) } else { Value::Null };
            this.put("capturePausedUntil", until);
        });
        let target = pause.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| {
            let paused = d.get("capturePausedUntil").and_then(Value::as_i64).map(|t| t > glib::real_time() / 1000).unwrap_or(false);
            target.set_active(paused);
        }));
        capture.add(&pause);
        capture.add(&self.choice("bypassKey", "Bypass key (hold while clicking)", &[("alt", "Alt"), ("shift", "Shift"), ("ctrl", "Control"), ("none", "None")]).row);
        capture.add(&self.number("captureMinSizeKb", "Skip files smaller than (KB)", 0.0, 10_000_000.0));
        page.add(&capture);

        let types = adw::PreferencesGroup::builder().title("File types to capture").build();
        let list = adw::EntryRow::builder().title("Extensions, separated by spaces").build();
        list.add_css_class("monospace");
        let this = self.clone();
        list.connect_changed(move |r| this.put("captureExtensions", parse_extensions(&r.text()).into()));
        let target = list.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| {
            let have = extension_list(d.get("captureExtensions"));
            if parse_extensions(&target.text()) != have {
                target.set_text(&have.join(" "));
            }
        }));
        types.add(&list);
        page.add(&types);
        page
    }

    fn file_types(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("File Types", "folder-documents-symbolic");
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add a category").valign(gtk::Align::Center).build();
        add.add_css_class("flat");
        let this = self.clone();
        add.connect_clicked(move |_| {
            let mut cats = this.get("categories").as_array().cloned().unwrap_or_default();
            let id = format!("c{:06x}", glib::random_int() & 0xff_ffff);
            let at = cats.iter().position(|c| s(c, "id") == "general").unwrap_or(cats.len());
            cats.insert(at, json!({ "id": id, "name": "New Category", "icon": "cat-general", "extensions": [], "folder": null }));
            this.put("categories", cats.into());
            this.fill_categories();
        });
        self.categories.set_header_suffix(Some(&add));
        page.add(&self.categories);
        let this = self.clone();
        let shown = RefCell::new(Vec::<String>::new());
        self.loaders.borrow_mut().push(Box::new(move |d| {
            // rebuilt only when categories come or go, so typing isn't interrupted
            let ids: Vec<String> = d.get("categories").and_then(Value::as_array).into_iter().flatten().map(|c| s(c, "id").to_string()).collect();
            if *shown.borrow() != ids {
                *shown.borrow_mut() = ids;
                this.fill_categories();
            }
        }));
        page
    }

    fn category(&self, id: &str) -> Value {
        self.get("categories").as_array().into_iter().flatten().find(|c| s(c, "id") == id).cloned().unwrap_or(Value::Null)
    }

    /// Change one category (and optionally take its new extensions away
    /// from the others: an extension belongs to one category).
    fn edit_category(self: &Rc<Self>, id: &str, f: impl Fn(&mut Value), take: &[String]) {
        let mut cats = self.get("categories").as_array().cloned().unwrap_or_default();
        for c in cats.iter_mut() {
            if s(c, "id") == id {
                f(c);
            } else if !take.is_empty() {
                let kept: Vec<String> = extension_list(c.get("extensions")).into_iter().filter(|e| !take.contains(e)).collect();
                c["extensions"] = kept.into();
            }
        }
        self.put("categories", cats.into());
    }

    fn default_folder(&self, c: &Value) -> String {
        let base = s(&self.draft.borrow(), "defaultFolder").to_string();
        if s(c, "id") == "general" {
            base
        } else {
            std::path::Path::new(&base).join(s(c, "name").replace('/', "_")).to_string_lossy().into_owned()
        }
    }

    fn fill_categories(self: &Rc<Self>) {
        for row in self.category_rows.borrow_mut().drain(..) {
            self.categories.remove(&row);
        }
        let cats = self.get("categories").as_array().cloned().unwrap_or_default();
        for c in cats {
            let id = s(&c, "id").to_string();
            let general = id == "general";
            let count = extension_list(c.get("extensions")).len();
            let row = adw::ExpanderRow::builder()
                .title(glib::markup_escape_text(s(&c, "name")))
                .subtitle(if general { "Everything else".to_string() } else { format!("{count} types") })
                .build();
            if general {
                let note = adw::ActionRow::builder()
                    .title("General holds every file type no other category claims, and saves to your download folder.")
                    .build();
                row.add_row(&note);
            } else {
                let name = adw::EntryRow::builder().title("Name").text(s(&c, "name")).build();
                let (this, cid, parent) = (self.clone(), id.clone(), row.clone());
                name.connect_changed(move |r| {
                    let text = r.text().to_string();
                    parent.set_title(&glib::markup_escape_text(&text));
                    this.edit_category(&cid, |c| c["name"] = text.clone().into(), &[]);
                });
                row.add_row(&name);
            }

            let folder = adw::ActionRow::builder().title("Folder").build();
            folder.add_css_class("property");
            let own = c.get("folder").and_then(Value::as_str).filter(|f| !f.is_empty()).map(String::from);
            folder.set_subtitle(&glib::markup_escape_text(&own.clone().unwrap_or_else(|| self.default_folder(&c))));
            let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
            let reset = gtk::Button::builder().label("Use Default").valign(gtk::Align::Center).visible(own.is_some()).build();
            folder.add_suffix(&reset);
            folder.add_suffix(&choose);
            let (this, cid) = (self.clone(), id.clone());
            choose.connect_clicked(move |btn| {
                let (this, cid) = (this.clone(), cid.clone());
                let c = this.category(&cid);
                let start = c.get("folder").and_then(Value::as_str).map(String::from).unwrap_or_else(|| this.default_folder(&c));
                let Some(window) = btn.root().and_downcast::<gtk::Window>() else { return };
                glib::spawn_future_local(async move {
                    if let Some(path) = widgets::choose_folder(&window, &start).await {
                        this.edit_category(&cid, |c| c["folder"] = path.clone().into(), &[]);
                        this.fill_categories();
                    }
                });
            });
            let (this, cid) = (self.clone(), id.clone());
            reset.connect_clicked(move |_| {
                this.edit_category(&cid, |c| c["folder"] = Value::Null, &[]);
                this.fill_categories();
            });
            row.add_row(&folder);

            if !general {
                let types = adw::EntryRow::builder()
                    .title("File types, separated by spaces")
                    .text(extension_list(c.get("extensions")).join(" "))
                    .show_apply_button(true)
                    .build();
                types.add_css_class("monospace");
                let (this, cid) = (self.clone(), id.clone());
                types.connect_apply(move |r| {
                    let list = parse_extensions(&r.text());
                    let wanted = list.clone();
                    this.edit_category(&cid, |c| c["extensions"] = wanted.clone().into(), &list);
                    this.fill_categories();
                });
                row.add_row(&types);
                let remove = adw::ActionRow::builder().title("Remove This Category").activatable(true).build();
                remove.add_css_class("error");
                let (this, cid) = (self.clone(), id.clone());
                remove.connect_activated(move |_| {
                    let cats: Vec<Value> = this.get("categories").as_array().into_iter().flatten().filter(|c| s(c, "id") != cid).cloned().collect();
                    this.put("categories", cats.into());
                    this.fill_categories();
                });
                row.add_row(&remove);
            }
            self.categories.add(&row);
            self.category_rows.borrow_mut().push(row);
        }
    }

    fn network(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("Proxy", "network-server-symbolic");
        let g = adw::PreferencesGroup::new();
        g.add(&self.choice("proxyMode", "Connection", &[("none", "No proxy"), ("system", "Use system settings"), ("http", "HTTP proxy"), ("socks5", "SOCKS5 proxy")]).row);
        let host = self.text("proxyHost", "Host");
        let port = self.number("proxyPort", "Port", 1.0, 65535.0);
        let user = self.text("proxyUser", "Username");
        let pass = adw::PasswordEntryRow::builder().title("Password").build();
        let this = self.clone();
        pass.connect_changed(move |r| this.put("proxyPass", r.text().to_string().into()));
        let target = pass.clone();
        self.loaders.borrow_mut().push(Box::new(move |d| {
            if target.text() != s(d, "proxyPass") {
                target.set_text(s(d, "proxyPass"));
            }
        }));
        g.add(&host);
        g.add(&port);
        g.add(&user);
        g.add(&pass);
        *self.proxy_rows.borrow_mut() = vec![host.upcast(), port.upcast(), user.upcast(), pass.upcast()];
        page.add(&g);
        page
    }

    fn advanced(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = page("Advanced", "applications-engineering-symbolic");
        let save = adw::PreferencesGroup::builder()
            .title("Save locations")
            .description("Each category saves to a subfolder of this folder unless you choose another under File Types.")
            .build();
        save.add(&self.folder("defaultFolder", "Download folder"));
        page.add(&save);
        let net = adw::PreferencesGroup::builder().title("Network").build();
        net.add(&self.text("userAgent", "User agent"));
        net.add(&self.number("maxRetries", "Retries per connection", 0.0, 1000.0));
        net.add(&self.number("retryDelaySecs", "Retry delay (seconds)", 0.0, 3600.0));
        net.add(&self.number("timeoutSecs", "Timeout (seconds)", 1.0, 3600.0));
        net.add(&self.number("minSegmentKb", "Minimum segment size (KB)", 1.0, 1_048_576.0));
        net.add(&self.number("extensionPort", "Extension port", 1.0, 65535.0));
        page.add(&net);
        let security = adw::PreferencesGroup::builder().title("Security").build();
        security.add(&self.switch("verifyTls", "Verify TLS certificates", ""));
        security.add(&self.tls_warning);
        page.add(&security);
        page
    }

    fn show_extension(&self) {
        let ext = odm().ext.borrow().clone();
        let browsers: Vec<&str> = ext.get("browsers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let err = ext.get("bridgeError").and_then(Value::as_str);
        self.ext_status.set_subtitle(&if err.is_some() {
            "Not listening".to_string()
        } else if b(&ext, "connected") {
            format!("Connected: {}", browsers.join(", "))
        } else {
            "Not detected".to_string()
        });
        self.ext_error.set_visible(err.is_some());
        if let Some(e) = err {
            self.ext_error.set_label(&format!("{e}. Another app may be using this port; choose a different one under Advanced."));
        }
    }
}

fn extension_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).into_iter().flatten().filter_map(|e| e.as_str().map(String::from)).collect()
}

/// "mkv, .WEBM;zip" → ["mkv", "webm", "zip"]
fn parse_extensions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for e in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let e = e.trim_matches('.').to_lowercase();
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}
