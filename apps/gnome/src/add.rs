//! The Add Download window (also used for links handed over by the
//! browser extension, the clipboard monitor and `odm://` links).
use crate::app::{odm, call, App};
use crate::row::{b, n, opt_n, s};
use crate::widgets::{self, Choice};
use adw::prelude::*;
use gtk::glib;
use odm_ffi::api;
use odm_linux::fmt;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const CONNECTIONS: [&str; 5] = ["1", "4", "8", "16", "32"];

struct Add {
    window: adw::Window,
    /// opened from a web page: the page chose the URL, so it is not
    /// contacted until the user asks, and Start is not the default button
    from_web: bool,
    url: adw::EntryRow,
    batch_text: gtk::TextView,
    batch_group: adw::PreferencesGroup,
    batch_count: adw::ActionRow,
    url_group: adw::PreferencesGroup,
    info: adw::ActionRow,
    spinner: gtk::Spinner,
    check: gtk::Button,
    name: adw::EntryRow,
    category: Choice,
    queue: Choice,
    dir: adw::EntryRow,
    connections: Choice,
    always: adw::SwitchRow,
    remember: adw::SwitchRow,
    verify: adw::SwitchRow,
    checksum: adw::EntryRow,
    username: adw::EntryRow,
    password: adw::PasswordEntryRow,
    referrer: adw::EntryRow,
    cookies: adw::EntryRow,
    user_agent: adw::EntryRow,
    advanced: adw::ExpanderRow,
    headers: RefCell<Vec<adw::EntryRow>>,
    error: gtk::Label,
    batch: gtk::Switch,
    queue_button: gtk::Button,
    start_button: gtk::Button,

    batch_urls: RefCell<Vec<String>>,
    probe: RefCell<Option<Value>>,
    auto_name: RefCell<String>,
    auto_dir: RefCell<String>,
    auto_category: RefCell<String>,
    category_touched: Cell<bool>,
    /// set while the code (not the user) changes a field
    quiet: Cell<bool>,
    /// bumped to drop the result of an older probe
    generation: Cell<u32>,
    busy: Cell<bool>,
}

fn opt(text: glib::GString) -> Value {
    let t = text.trim();
    if t.is_empty() {
        Value::Null
    } else {
        t.into()
    }
}

/// Open an Add window. `params` is the engine's `show_add` payload (any
/// field may be missing), `{batch: true}`, or `{url, source: "web"}`.
pub fn open(app: &Rc<App>, params: Value) {
    let from_web = s(&params, "source") == "web";
    let window = adw::Window::builder()
        .application(&app.gtk)
        .title("Add Download")
        .default_width(560)
        .default_height(700)
        .build();
    let mono = |row: &adw::EntryRow| row.add_css_class("monospace");
    let url = adw::EntryRow::builder().title("URL").input_purpose(gtk::InputPurpose::Url).build();
    mono(&url);
    let dir = adw::EntryRow::builder().title("Save to").build();
    mono(&dir);
    let checksum = adw::EntryRow::builder().title("SHA-256, SHA-1 or MD5").visible(false).build();
    mono(&checksum);
    let cats: Vec<(String, String)> = app.categories().iter().map(|c| (s(c, "id").to_string(), s(c, "name").to_string())).collect();
    let category = Choice::new("Category", &[]);
    category.set_items(cats);
    let queue = Choice::new("Queue", &[]);
    let mut queues = vec![(String::new(), "None (start manually)".to_string())];
    queues.extend(app.queues.borrow().iter().map(|q| (s(q, "id").to_string(), s(q, "name").to_string())));
    queue.set_items(queues);
    let connections = Choice::new("Connections", &CONNECTIONS.map(|c| (c, c)));

    let batch_text = gtk::TextView::builder().monospace(true).wrap_mode(gtk::WrapMode::Char).top_margin(8).bottom_margin(8).left_margin(10).right_margin(10).build();
    let a = Rc::new(Add {
        window: window.clone(),
        from_web,
        url,
        batch_text,
        batch_group: adw::PreferencesGroup::builder()
            .title("Links")
            .description("One per line. Patterns like file[001-120].jpg or [a-z] expand.")
            .visible(false)
            .build(),
        batch_count: adw::ActionRow::builder().title("Files").build(),
        url_group: adw::PreferencesGroup::new(),
        info: adw::ActionRow::builder().visible(false).build(),
        spinner: gtk::Spinner::new(),
        check: gtk::Button::builder().label("Check Link").valign(gtk::Align::Center).visible(false).build(),
        name: adw::EntryRow::builder().title("File name").build(),
        category,
        queue,
        dir,
        connections,
        always: adw::SwitchRow::builder().visible(false).build(),
        remember: adw::SwitchRow::builder().active(true).build(),
        verify: adw::SwitchRow::builder().title("Verify checksum after download").build(),
        checksum,
        username: adw::EntryRow::builder().title("Username").build(),
        password: adw::PasswordEntryRow::builder().title("Password").build(),
        referrer: adw::EntryRow::builder().title("Referrer").build(),
        cookies: adw::EntryRow::builder().title("Cookies (name=value; other=value)").build(),
        user_agent: adw::EntryRow::builder().title("User agent (empty = default)").build(),
        advanced: adw::ExpanderRow::builder().title("Authentication, referrer, cookies, headers").build(),
        headers: RefCell::default(),
        error: gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).visible(false).margin_start(12).margin_end(12).build(),
        batch: gtk::Switch::builder().valign(gtk::Align::Center).tooltip_text("Add several links or a numbered pattern").build(),
        queue_button: gtk::Button::with_label("Add to Queue"),
        start_button: gtk::Button::with_label("Start Download"),
        batch_urls: RefCell::default(),
        probe: RefCell::default(),
        auto_name: RefCell::default(),
        auto_dir: RefCell::default(),
        auto_category: RefCell::new("general".into()),
        category_touched: Cell::new(false),
        quiet: Cell::new(false),
        generation: Cell::new(0),
        busy: Cell::new(false),
    });
    a.error.add_css_class("error");
    a.info.add_prefix(&a.spinner);
    a.info.add_suffix(&a.check);

    // -------------------------------------------------------------- layout
    let page = adw::PreferencesPage::new();
    a.url_group.add(&a.url);
    a.url_group.add(&a.info);
    page.add(&a.url_group);
    let frame = gtk::ScrolledWindow::builder().child(&a.batch_text).min_content_height(110).build();
    frame.add_css_class("card");
    a.batch_group.add(&frame);
    a.batch_group.add(&a.batch_count);
    page.add(&a.batch_group);

    let target = adw::PreferencesGroup::new();
    target.add(&a.name);
    target.add(&a.category.row);
    target.add(&a.queue.row);
    let browse = gtk::MenuButton::builder().icon_name("folder-open-symbolic").tooltip_text("Choose a folder").valign(gtk::Align::Center).build();
    browse.add_css_class("flat");
    a.dir.add_suffix(&browse);
    target.add(&a.dir);
    page.add(&target);

    let options = adw::PreferencesGroup::new();
    options.add(&a.connections.row);
    options.add(&a.always);
    options.add(&a.remember);
    options.add(&a.verify);
    options.add(&a.checksum);
    page.add(&options);

    let more = adw::PreferencesGroup::new();
    for row in [&a.username, &a.referrer, &a.cookies, &a.user_agent] {
        a.advanced.add_row(row);
    }
    a.advanced.add_row(&a.password);
    let add_header = adw::ActionRow::builder().title("Add Header").activatable(true).build();
    add_header.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
    a.advanced.add_row(&add_header);
    more.add(&a.advanced);
    page.add(&more);
    let errors = adw::PreferencesGroup::new();
    errors.add(&a.error);
    page.add(&errors);

    let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).build();
    let batch_label = gtk::Label::new(Some("Batch"));
    bar.append(&a.batch);
    bar.append(&batch_label);
    bar.append(&gtk::Box::builder().hexpand(true).build());
    let cancel = gtk::Button::with_label("Cancel");
    bar.append(&a.queue_button);
    bar.append(&cancel);
    bar.append(&a.start_button);
    a.start_button.add_css_class("suggested-action");

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    view.add_bottom_bar(&bar);
    window.set_content(Some(&view));
    if !from_web {
        // Enter starts the download, except for a link a web page chose
        window.set_default_widget(Some(&a.start_button));
        a.url.set_activates_default(true);
        a.name.set_activates_default(true);
    }

    // -------------------------------------------------------------- values
    let settings = app.settings.borrow().clone();
    a.connections.set_value(&nearest_connections(n(&settings, "defaultConnections")));
    let folder = api::call("folder_for", json!({ "category": "general" })).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    *a.auto_dir.borrow_mut() = folder.clone();
    a.dir.set_text(&folder);
    a.category.set_value("general");
    a.batch.set_active(b(&params, "batch"));
    if !from_web {
        // a web page only gets to suggest the URL
        a.name.set_text(s(&params, "fileName"));
        a.referrer.set_text(s(&params, "referrer"));
        a.cookies.set_text(s(&params, "cookies"));
        a.user_agent.set_text(s(&params, "userAgent"));
    }
    let recents = api::call("recent_folders", json!({ "limit": 6 })).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let menu = gtk::gio::Menu::new();
    let recent_section = gtk::gio::Menu::new();
    for r in recents.iter().filter_map(Value::as_str) {
        recent_section.append(Some(r), Some(&format!("add.folder::{r}")));
    }
    menu.append_section(None, &recent_section);
    let choose = gtk::gio::Menu::new();
    choose.append(Some("Choose…"), Some("add.browse"));
    menu.append_section(None, &choose);
    browse.set_menu_model(Some(&menu));
    let group = gtk::gio::SimpleActionGroup::new();
    let set_folder = gtk::gio::SimpleAction::new("folder", Some(glib::VariantTy::STRING));
    let this = a.clone();
    set_folder.connect_activate(move |_, p| {
        if let Some(path) = p.and_then(|v| v.str().map(String::from)) {
            this.dir.set_text(&path);
        }
    });
    group.add_action(&set_folder);
    let pick = gtk::gio::SimpleAction::new("browse", None);
    let this = a.clone();
    pick.connect_activate(move |_, _| {
        let this = this.clone();
        glib::spawn_future_local(async move {
            if let Some(p) = widgets::choose_folder(&this.window, &this.dir.text()).await {
                this.dir.set_text(&p);
            }
        });
    });
    group.add_action(&pick);
    window.insert_action_group("add", Some(&group));

    // -------------------------------------------------------------- wiring
    let this = a.clone();
    a.url.connect_changed(move |_| this.schedule_probe());
    let this = a.clone();
    a.batch_text.buffer().connect_changed(move |_| this.schedule_probe());
    let this = a.clone();
    a.batch.connect_active_notify(move |_| {
        this.layout();
        this.schedule_probe();
    });
    let this = a.clone();
    a.name.connect_changed(move |_| this.layout());
    let this = a.clone();
    a.dir.connect_changed(move |_| this.layout());
    let this = a.clone();
    a.category.on_change(move |c| {
        if !this.quiet.get() {
            this.category_touched.set(true);
        }
        this.category_changed(&c);
    });
    let this = a.clone();
    a.verify.connect_active_notify(move |v| this.checksum.set_visible(v.is_active()));
    let this = a.clone();
    a.check.connect_clicked(move |_| this.probe_now());
    let this = a.clone();
    add_header.connect_activated(move |_| this.add_header());
    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    let this = a.clone();
    a.queue_button.connect_clicked(move |_| this.submit(false, None));
    let this = a.clone();
    a.start_button.connect_clicked(move |_| this.submit(true, None));
    let esc = gtk::ShortcutController::new();
    esc.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("Escape"), Some(gtk::NamedAction::new("window.close"))));
    window.add_controller(esc);

    a.layout();
    let given = s(&params, "url").to_string();
    if !given.is_empty() {
        a.url.set_text(&given);
        window.present();
    } else if !a.batch.is_active() {
        // offer a link that is on the clipboard
        let this = a.clone();
        glib::spawn_future_local(async move {
            if let Some(display) = gtk::gdk::Display::default() {
                if let Ok(Some(text)) = display.clipboard().read_text_future().await {
                    let t = text.trim();
                    if this.url.text().is_empty() && fmt::is_http(t) && !t.chars().any(char::is_whitespace) {
                        this.url.set_text(t);
                    }
                }
            }
        });
        window.present();
    } else {
        window.present();
    }
}

fn nearest_connections(want: u64) -> String {
    CONNECTIONS
        .iter()
        .min_by_key(|c| c.parse::<u64>().unwrap_or(16).abs_diff(want.max(1)))
        .copied()
        .unwrap_or("16")
        .to_string()
}

impl Add {
    fn is_batch(&self) -> bool {
        self.batch.is_active()
    }

    fn link(&self) -> String {
        self.url.text().trim().to_string()
    }

    fn name_ext(&self) -> String {
        fmt::extension(self.name.text().trim())
    }

    fn dir_touched(&self) -> bool {
        self.dir.text() != *self.auto_dir.borrow()
    }

    fn valid(&self) -> bool {
        if self.is_batch() {
            !self.batch_urls.borrow().is_empty()
        } else {
            fmt::is_http(&self.link())
        }
    }

    fn category_name(&self) -> String {
        let id = self.category.value();
        odm().categories().iter().find(|c| s(c, "id") == id).map(|c| s(c, "name").to_string()).unwrap_or_else(|| "this category".into())
    }

    /// Show what applies to the current mode and input.
    fn layout(&self) {
        let batch = self.is_batch();
        self.url_group.set_visible(!batch);
        self.batch_group.set_visible(batch);
        self.name.set_visible(!batch);
        self.category.row.set_visible(!batch);
        let ext = self.name_ext();
        let cat = self.category_name();
        self.always.set_title(&glib::markup_escape_text(&format!("Always put .{ext} files in {cat}")));
        self.always.set_visible(!batch && !ext.is_empty() && self.category.value() != *self.auto_category.borrow());
        self.remember.set_title(&glib::markup_escape_text(&format!("Remember this folder for {cat}")));
        self.remember.set_visible(!batch);
        self.verify.set_visible(!batch);
        self.checksum.set_visible(!batch && self.verify.is_active());
        let resumable = self.probe.borrow().as_ref().map(|p| b(p, "resumable")).unwrap_or(true);
        self.connections.row.set_sensitive(resumable);
        let count = self.batch_urls.borrow().len();
        self.batch_count.set_subtitle(&count.to_string());
        self.start_button.set_label(&if batch { format!("Start {count} Downloads") } else { "Start Download".to_string() });
        let ok = self.valid() && !self.busy.get();
        self.start_button.set_sensitive(ok);
        self.queue_button.set_sensitive(ok);
        let text = self.link();
        if !batch && !text.is_empty() && !fmt::is_http(&text) {
            self.url.add_css_class("error");
        } else {
            self.url.remove_css_class("error");
        }
    }

    fn category_changed(&self, c: &str) {
        if !self.dir_touched() {
            if let Some(f) = api::call("folder_for", json!({ "category": c })).ok().and_then(|v| v.as_str().map(String::from)) {
                *self.auto_dir.borrow_mut() = f.clone();
                self.dir.set_text(&f);
            }
        }
        self.layout();
    }

    fn add_header(&self) {
        let row = adw::EntryRow::builder().title("Header (Name: value)").build();
        let remove = gtk::Button::builder().icon_name("list-remove-symbolic").valign(gtk::Align::Center).tooltip_text("Remove header").build();
        remove.add_css_class("flat");
        row.add_suffix(&remove);
        self.advanced.add_row(&row);
        let advanced = self.advanced.clone();
        let target = row.clone();
        remove.connect_clicked(move |_| {
            // emptied so `header_pairs` skips it
            target.set_text("");
            advanced.remove(&target);
        });
        self.headers.borrow_mut().push(row);
    }

    fn header_pairs(&self) -> Vec<Value> {
        self.headers
            .borrow()
            .iter()
            .filter_map(|r| {
                let text = r.text();
                let (k, v) = text.split_once(':')?;
                (!k.trim().is_empty()).then(|| json!([k.trim(), v.trim()]))
            })
            .collect()
    }

    fn batch_source(&self) -> String {
        let buf = self.batch_text.buffer();
        buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string()
    }

    fn show_info(&self, title: &str, subtitle: &str, spinning: bool) {
        self.info.set_visible(true);
        self.info.set_title(&glib::markup_escape_text(title));
        self.info.set_subtitle(&glib::markup_escape_text(subtitle));
        self.spinner.set_visible(spinning);
        self.spinner.set_spinning(spinning);
        self.check.set_visible(self.from_web && !spinning);
    }

    /// The input changed: expand the batch, or probe the link after a pause.
    fn schedule_probe(self: &Rc<Self>) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        if self.is_batch() {
            let this = self.clone();
            let source = self.batch_source();
            glib::timeout_add_local_once(Duration::from_millis(300), move || {
                if this.generation.get() != generation {
                    return;
                }
                glib::spawn_future_local(async move {
                    let mut out: Vec<String> = vec![];
                    for line in source.split_whitespace() {
                        let pattern = line.find('[').zip(line.rfind(']')).map(|(a, z)| a < z && line[a..z].contains('-')).unwrap_or(false);
                        if pattern {
                            if let Ok(Value::Array(list)) = call("expand_batch", json!({ "pattern": line })).await {
                                out.extend(list.iter().filter_map(Value::as_str).map(String::from));
                            }
                        } else {
                            out.push(line.to_string());
                        }
                    }
                    if this.generation.get() != generation {
                        return;
                    }
                    out.retain(|u| fmt::is_http(u));
                    out.truncate(10_000);
                    *this.batch_urls.borrow_mut() = out;
                    this.layout();
                });
            });
            self.layout();
            return;
        }
        *self.probe.borrow_mut() = None;
        let link = self.link();
        if !fmt::is_http(&link) {
            self.info.set_visible(false);
            self.layout();
            return;
        }
        if self.from_web {
            // probe only on request (Check Link)
            self.show_info(&fmt::host(&link), "Opened from a web page. The link has not been contacted yet.", false);
            self.check.set_label("Check Link");
            self.layout();
            return;
        }
        let this = self.clone();
        self.show_info("Checking link…", "", true);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if this.generation.get() == generation {
                this.start_probe(link, generation);
            }
        });
        self.layout();
    }

    /// Probe right away (the Check Link button).
    fn probe_now(self: &Rc<Self>) {
        let link = self.link();
        if self.is_batch() || !fmt::is_http(&link) {
            return;
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        *self.probe.borrow_mut() = None;
        self.show_info("Checking link…", "", true);
        self.start_probe(link, generation);
    }

    fn start_probe(self: &Rc<Self>, link: String, generation: u32) {
        let opts = json!({
            "referrer": opt(self.referrer.text()),
            "cookies": opt(self.cookies.text()),
            "username": opt(self.username.text()),
            "password": opt(self.password.text()),
            "headers": self.header_pairs(),
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = call("probe_url", json!({ "url": link, "opts": opts })).await;
            if this.generation.get() != generation {
                return;
            }
            let name_touched = this.name.text() != *this.auto_name.borrow();
            match result {
                Ok(p) => {
                    if !name_touched {
                        *this.auto_name.borrow_mut() = s(&p, "fileName").to_string();
                        this.name.set_text(s(&p, "fileName"));
                    }
                    *this.auto_category.borrow_mut() = s(&p, "category").to_string();
                    if !this.category_touched.get() {
                        this.quiet.set(true);
                        this.category.set_value(s(&p, "category"));
                        this.quiet.set(false);
                        this.category_changed(s(&p, "category"));
                    }
                    if !b(&p, "resumable") {
                        this.connections.set_value("1");
                    } else if this.connections.value() == "1" {
                        this.connections.set_value(&nearest_connections(n(&odm().settings.borrow(), "defaultConnections")));
                    }
                    let size = fmt::bytes(opt_n(&p, "totalBytes"));
                    let mime = p.get("mime").and_then(Value::as_str).map(|m| format!(" · {m}")).unwrap_or_default();
                    let detail = if b(&p, "resumable") {
                        format!("Resumable · byte ranges · {}", s(&p, "protocol"))
                    } else {
                        "Server doesn't support resume. Download will use 1 connection.".to_string()
                    };
                    this.show_info(&format!("{size}{mime}"), &detail, false);
                    *this.probe.borrow_mut() = Some(p);
                }
                Err(e) => {
                    if !name_touched {
                        let last = url_file_name(&link);
                        if !last.is_empty() {
                            *this.auto_name.borrow_mut() = last.clone();
                            this.name.set_text(&last);
                        }
                    }
                    this.show_info(&e, "You can still add it; it will retry when started.", false);
                }
            }
            this.check.set_label("Check Again");
            this.layout();
        });
    }

    fn request(&self, start: bool, link: &str, on_exists: Option<&str>) -> Value {
        let batch = self.is_batch();
        let queue = self.queue.value();
        let probe = self.probe.borrow();
        let from_probe = |k: &str| if batch { Value::Null } else { probe.as_ref().and_then(|p| p.get(k).cloned()).unwrap_or(Value::Null) };
        let dir_touched = self.dir_touched();
        let checksum = self.checksum.text().trim().to_string();
        json!({
            "url": link,
            "fileName": if batch { Value::Null } else { opt(self.name.text()) },
            "saveDir": if dir_touched { opt(self.dir.text()) } else { Value::Null },
            "category": if batch { Value::Null } else { self.category.value().into() },
            "queueId": if !queue.is_empty() { Value::String(queue) } else if start { Value::Null } else { "main".into() },
            "start": start,
            "connections": self.connections.value().parse::<u32>().unwrap_or(16),
            "referrer": opt(self.referrer.text()),
            "cookies": opt(self.cookies.text()),
            "userAgent": opt(self.user_agent.text()),
            "headers": self.header_pairs(),
            "username": opt(self.username.text()),
            "password": opt(self.password.text()),
            "checksum": if !batch && self.verify.is_active() && !checksum.is_empty() { json!({ "algo": "", "expected": checksum }) } else { Value::Null },
            "onExists": on_exists.unwrap_or("rename"),
            "rememberFolder": !batch && self.remember.is_active() && dir_touched,
            "totalBytes": from_probe("totalBytes"),
            "resumable": from_probe("resumable"),
            "mime": from_probe("mime"),
        })
    }

    fn fail(&self, msg: &str) {
        self.error.set_label(msg);
        self.error.set_visible(true);
        self.busy.set(false);
        self.layout();
    }

    fn submit(self: &Rc<Self>, start: bool, on_exists: Option<&'static str>) {
        if !self.valid() || self.busy.get() {
            return;
        }
        self.error.set_visible(false);
        let batch = self.is_batch();
        let name = self.name.text().trim().to_string();
        let category = self.category.value();
        if !batch && on_exists.is_none() && !name.is_empty() {
            let dir = if self.dir_touched() {
                self.dir.text().to_string()
            } else {
                api::call("folder_for", json!({ "category": category })).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| self.dir.text().to_string())
            };
            let exists = api::call("target_exists", json!({ "dir": dir, "name": name })).ok().and_then(|v| v.as_bool()).unwrap_or(false);
            if exists {
                let this = self.clone();
                glib::spawn_future_local(async move {
                    let choice = widgets::ask(
                        &this.window,
                        &format!("“{name}” already exists in this folder."),
                        "",
                        &[
                            ("overwrite", "Overwrite", adw::ResponseAppearance::Destructive),
                            ("resume", "Resume Existing File", adw::ResponseAppearance::Default),
                            ("rename", "Rename New File", adw::ResponseAppearance::Suggested),
                        ],
                    )
                    .await;
                    match choice.as_str() {
                        "overwrite" => this.submit(start, Some("overwrite")),
                        "resume" => this.submit(start, Some("resume")),
                        "rename" => this.submit(start, Some("rename")),
                        _ => {}
                    }
                });
                return;
            }
        }
        self.busy.set(true);
        self.layout();
        let reqs: Vec<Value> = if batch {
            self.batch_urls.borrow().iter().map(|u| self.request(start, u, None)).collect()
        } else {
            vec![self.request(start, &self.link(), on_exists)]
        };
        let ext = self.name_ext();
        let assign = !batch && self.always.is_active() && !ext.is_empty() && category != *self.auto_category.borrow();
        let show_progress = start && odm().setting_on("showProgressOnStart", false);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if assign {
                if let Err(e) = call("assign_extensions", json!({ "category": category, "extensions": [ext] })).await {
                    return this.fail(&e);
                }
            }
            let added = if reqs.len() == 1 {
                call("add_download", json!({ "req": reqs[0] })).await
            } else {
                call("add_many", json!({ "reqs": reqs })).await
            };
            match added {
                Ok(id) => {
                    if let (true, Some(id)) = (show_progress, id.as_str()) {
                        let id = id.to_string();
                        // the list learns about it through its event first
                        glib::timeout_add_local_once(Duration::from_millis(200), move || odm().open_progress(&id));
                    }
                    this.window.close();
                }
                Err(e) => this.fail(&e),
            }
        });
    }
}

/// The last path segment of a link, percent-decoded.
pub fn url_file_name(link: &str) -> String {
    gtk::glib::Uri::parse(link, glib::UriFlags::NONE)
        .ok()
        .map(|u| u.path().rsplit('/').next().unwrap_or("").to_string())
        .unwrap_or_default()
}
