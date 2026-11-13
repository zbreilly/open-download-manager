//! The Add Download window. What its fields mean and what they turn into
//! is `odm_ui::add::AddForm`; this file draws it.
use crate::app::app;
use crate::kit::{self, mtm, ns};
use crate::{engine, system};
use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::*;
use odm_ui::add::{AddForm, Info, CONNECTIONS};
use odm_ui::{fmt, n, s};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub struct AddWindow {
    window: Retained<NSWindow>,
    form: RefCell<AddForm>,
    /// bumped whenever the link changes: answers for an older link are dropped
    generation: Cell<u64>,
    busy: Cell<bool>,
    categories: RefCell<Vec<String>>,
    queues: RefCell<Vec<String>>,
    recents: Vec<String>,

    url: Retained<NSTextField>,
    url_row: Retained<NSStackView>,
    url_error: Retained<NSTextField>,
    batch_text: Retained<NSTextView>,
    batch_row: Retained<NSStackView>,
    batch_count: Retained<NSTextField>,
    info_row: Retained<NSStackView>,
    info_icon: Retained<NSImageView>,
    info_title: Retained<NSTextField>,
    info_detail: Retained<NSTextField>,
    info_spinner: Retained<NSProgressIndicator>,
    check: Retained<NSButton>,
    name: Retained<NSTextField>,
    name_row: Retained<NSStackView>,
    category: Retained<NSPopUpButton>,
    category_row: Retained<NSStackView>,
    queue: Retained<NSPopUpButton>,
    dir: Retained<NSTextField>,
    connections: Retained<NSSegmentedControl>,
    options_row: Retained<NSStackView>,
    always: Retained<NSButton>,
    remember: Retained<NSButton>,
    verify: Retained<NSButton>,
    checksum: Retained<NSTextField>,
    advanced: Retained<NSButton>,
    advanced_box: Retained<NSStackView>,
    username: Retained<NSTextField>,
    password: Retained<NSSecureTextField>,
    referrer: Retained<NSTextField>,
    cookies: Retained<NSTextField>,
    user_agent: Retained<NSTextField>,
    headers_box: Retained<NSStackView>,
    header_fields: RefCell<Vec<(Retained<NSTextField>, Retained<NSTextField>)>>,
    error: Retained<NSTextField>,
    batch: Retained<NSSwitch>,
    queue_button: Retained<NSButton>,
    start_button: Retained<NSButton>,
}

fn folder_for(category: &str) -> Option<String> {
    engine::get_string("folder_for", json!({ "category": category }))
}

fn default_connections() -> u64 {
    let c = n(&app().store.borrow().settings, "defaultConnections");
    if c == 0 {
        16
    } else {
        c
    }
}

fn call(weak: &Weak<AddWindow>, f: impl Fn(&Rc<AddWindow>) + Clone + 'static) -> impl Fn() + Clone + 'static {
    kit::bound(weak, f)
}

impl AddWindow {
    /// `params`: the engine's `show_add` payload, `{batch: true}`,
    /// `{url, source: "web"}` or `{}`.
    pub fn open(params: Value) -> Rc<AddWindow> {
        let mut form = AddForm::new(&params, default_connections(), &folder_for("general").unwrap_or_default());
        if form.url.is_empty() && !form.batch {
            // a link on the clipboard is most likely what the user wants
            if let Some(clip) = system::clipboard_text().map(|c| c.trim().to_string()) {
                if fmt::is_http(&clip) && !clip.chars().any(char::is_whitespace) {
                    form.url = clip;
                }
            }
        }
        let recents = engine::get("recent_folders", json!({ "limit": 6 })).map(|v| odm_ui::strings(&json!({ "r": v }), "r")).unwrap_or_default();
        let from_web = form.from_web;

        let this = Rc::new_cyclic(|weak: &Weak<AddWindow>| {
            let window = kit::window("Add Download", 560.0, 680.0, (480.0, 520.0), true);
            let edited = call(weak, |w| w.edited());

            let url = kit::text_field("https://", edited.clone());
            kit::mono(&url);
            let url_row = kit::row("URL", &url);
            let url_error = kit::note("Enter a full http:// or https:// address.");
            kit::red(&url_error);

            let (batch_scroll, batch_text) = kit::text_area(edited.clone());
            kit::min_height(&batch_scroll, 90.0);
            let batch_count = kit::dim("0 files");
            let batch_hint = kit::note("Links, one per line. Patterns like file[001-120].jpg or [a-z] expand.");
            let batch_column = kit::vfill(4.0, &[&batch_hint, &batch_scroll, &batch_count]);
            kit::stretch(&batch_column, true);
            let batch_row = kit::tall_row("Links", &batch_column);

            let info_icon = NSImageView::new(mtm());
            kit::width(&info_icon, 36.0);
            kit::height(&info_icon, 36.0);
            let info_title = kit::label("");
            kit::mono(&info_title);
            let info_detail = kit::note("");
            let info_spinner = kit::spinner();
            let check = kit::button("Check Link", call(weak, |w| w.probe_now()));
            let info_text = kit::vstack(3.0, &[&info_title, &info_detail]);
            kit::stretch(&info_text, true);
            let info_inner = kit::hstack(10.0, &[&info_icon, &info_spinner, &info_text, &check]);
            let info_row = kit::row("", &info_inner);

            let name = kit::text_field("File name", edited.clone());
            let name_row = kit::row("File name", &name);
            let category = kit::popup(&[], call(weak, |w| w.category_chosen()));
            let category_row = kit::row("Category", &category);
            let queue = kit::popup(&[], edited.clone());
            let queue_row = kit::row("Queue", &queue);

            let dir = kit::text_field("Folder", edited.clone());
            kit::mono(&dir);
            let choose = kit::button("Choose…", call(weak, |w| w.browse()));
            let dir_inner = kit::hstack(8.0, &[&dir, &choose]);
            if !recents.is_empty() {
                // a pull-down of the folders used lately
                let recent = unsafe { NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm()), objc2_foundation::NSRect::ZERO, true) };
                let mut titles = vec!["Recent".to_string()];
                titles.extend(recents.iter().cloned());
                kit::set_popup(&recent, &titles, 0);
                kit::on_action(&recent, {
                    let weak = weak.clone();
                    let recent = objc2::rc::Weak::from_retained(&recent);
                    move || {
                        let (Some(w), Some(recent)) = (weak.upgrade(), recent.load()) else { return };
                        let i = kit::popup_index(&recent);
                        if let Some(folder) = i.checked_sub(1).and_then(|i| w.recents.get(i)) {
                            kit::set_text(&w.dir, folder);
                            w.edited();
                        }
                    }
                });
                kit::width(&recent, 90.0);
                unsafe { dir_inner.addArrangedSubview(&recent) };
            }
            let dir_row = kit::row("Save to", &dir_inner);

            let labels: Vec<String> = CONNECTIONS.iter().map(|c| c.to_string()).collect();
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let connections = kit::segmented(&label_refs, edited.clone());
            let connections_row = kit::row("Connections", &connections);

            let always = kit::checkbox("Always put these files in this category", edited.clone());
            let remember = kit::checkbox("Remember this folder for this category", edited.clone());
            let verify = kit::checkbox("Verify checksum after download", edited.clone());
            let checksum = kit::text_field("SHA-256, SHA-1 or MD5", edited.clone());
            kit::mono(&checksum);
            let options = kit::vfill(6.0, &[&always, &remember, &verify, &checksum]);
            let options_row = kit::tall_row("", &options);

            let advanced = kit::button("", call(weak, |w| w.sync()));
            unsafe {
                advanced.setBezelStyle(NSBezelStyle::Disclosure);
                advanced.setButtonType(NSButtonType::PushOnPushOff);
                advanced.setTitle(&ns(""));
            }
            let advanced_head = kit::hstack(6.0, &[&advanced, &kit::label("Authentication, referrer, cookies, headers")]);
            let username = kit::text_field("", edited.clone());
            let password = kit::secure_field("", edited.clone());
            let referrer = kit::text_field("", edited.clone());
            let cookies = kit::text_field("name=value; other=value", edited.clone());
            let user_agent = kit::text_field("Default", edited.clone());
            let headers_box = kit::vfill(6.0, &[]);
            let add_header = kit::button("Add Header", call(weak, |w| w.add_header()));
            let headers_column = kit::vfill(6.0, &[&headers_box, &kit::hstack(0.0, &[&add_header, &kit::spacer()])]);
            let advanced_box = kit::vfill(
                8.0,
                &[
                    &kit::row("Username", &username),
                    &kit::row("Password", &password),
                    &kit::row("Referrer", &referrer),
                    &kit::row("Cookies", &cookies),
                    &kit::row("User agent", &user_agent),
                    &kit::tall_row("Headers", &headers_column),
                ],
            );

            let error = kit::wrapping("");
            kit::red(&error);
            unsafe { error.setSelectable(true) };

            let fields = kit::vfill_in(
                10.0,
                (18.0, 18.0, 18.0, 18.0),
                &[
                    &url_row,
                    &kit::row("", &url_error),
                    &batch_row,
                    &info_row,
                    &kit::hline(),
                    &name_row,
                    &category_row,
                    &queue_row,
                    &dir_row,
                    &kit::hline(),
                    &connections_row,
                    &options_row,
                    &kit::hline(),
                    &advanced_head,
                    &advanced_box,
                    &error,
                ],
            );
            let scroll = kit::scrolling(&fields);

            let batch = kit::toggle(call(weak, |w| w.batch_toggled()));
            unsafe { batch.setToolTip(Some(&ns("Add several links or a numbered pattern"))) };
            let queue_button = kit::button("Add to Queue", call(weak, |w| w.submit(false, None)));
            let cancel = kit::button("Cancel", call(weak, |w| w.window.performClose(None)));
            kit::cancel_button(&cancel);
            let start_button = kit::button("Start Download", call(weak, |w| w.submit(true, None)));
            if !from_web {
                // a page chose the link: Return must not start it
                kit::default_button(&start_button);
            }
            let bottom = kit::hstack(10.0, &[&batch, &kit::label("Batch"), &kit::spacer(), &queue_button, &cancel, &start_button]);
            kit::pad(&bottom, 12.0, 14.0, 12.0, 14.0);

            scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let root = kit::vfill(0.0, &[&scroll, &kit::hline(), &bottom]);
            unsafe { window.setContentView(Some(&root)) };

            kit::on_close(&window, {
                let weak = weak.clone();
                move || {
                    let ptr = weak.as_ptr();
                    // after the close has finished: this drops the window
                    kit::after(0.0, move || app().add_closed(ptr));
                }
            });

            AddWindow {
                window,
                form: RefCell::new(form),
                generation: Cell::new(0),
                busy: Cell::new(false),
                categories: RefCell::new(vec![]),
                queues: RefCell::new(vec![]),
                recents,
                url,
                url_row,
                url_error,
                batch_text,
                batch_row,
                batch_count,
                info_row,
                info_icon,
                info_title,
                info_detail,
                info_spinner,
                check,
                name,
                name_row,
                category,
                category_row,
                queue,
                dir,
                connections,
                options_row,
                always,
                remember,
                verify,
                checksum,
                advanced,
                advanced_box,
                username,
                password,
                referrer,
                cookies,
                user_agent,
                headers_box,
                header_fields: RefCell::new(vec![]),
                error,
                batch,
                queue_button,
                start_button,
            }
        });

        // what the form starts with
        {
            let f = this.form.borrow();
            kit::set_text(&this.url, &f.url);
            kit::set_text(&this.name, &f.name);
            kit::set_text(&this.dir, &f.dir);
            kit::set_text(&this.referrer, &f.referrer);
            kit::set_text(&this.cookies, &f.cookies);
            kit::set_text(&this.user_agent, &f.user_agent);
            kit::set_switch(&this.batch, f.batch);
            kit::set_on(&this.remember, f.remember);
        }
        this.lists_changed();
        this.link_changed();
        this.sync();
        kit::present(&this.window);
        // let a window that is being brought forward (by a link handed to
        // the app) settle first, so this one ends up on top
        kit::after(0.15, {
            let weak = Rc::downgrade(&this);
            move || {
                if let Some(w) = weak.upgrade() {
                    kit::present(&w.window);
                }
            }
        });
        this
    }

    /// Categories or queues changed: refill the pop-ups.
    pub fn lists_changed(&self) {
        let app = app();
        let st = app.store.borrow();
        let f = self.form.borrow();
        let categories: Vec<(String, String)> = st.categories().iter().map(|c| (s(c, "id").to_string(), s(c, "name").to_string())).collect();
        let titles: Vec<String> = categories.iter().map(|c| c.1.clone()).collect();
        let selected = categories.iter().position(|c| c.0 == f.category).unwrap_or(0);
        kit::set_popup(&self.category, &titles, selected);
        *self.categories.borrow_mut() = categories.into_iter().map(|c| c.0).collect();

        let mut ids = vec![String::new()];
        let mut titles = vec!["None (start manually)".to_string()];
        for q in &st.queues {
            ids.push(s(q, "id").to_string());
            titles.push(s(q, "name").to_string());
        }
        let selected = ids.iter().position(|q| *q == f.queue).unwrap_or(0);
        kit::set_popup(&self.queue, &titles, selected);
        *self.queues.borrow_mut() = ids;
    }

    /// Read the fields into the form.
    fn pull(&self) -> bool {
        let mut f = self.form.borrow_mut();
        let (url, batch_text) = (kit::text_of(&self.url), kit::text_view_string(&self.batch_text));
        let link_changed = url.trim() != f.link() || batch_text != f.batch_text;
        f.url = url;
        f.batch_text = batch_text;
        f.name = kit::text_of(&self.name);
        f.dir = kit::text_of(&self.dir);
        f.queue = self.queues.borrow().get(kit::popup_index(&self.queue)).cloned().unwrap_or_default();
        let segment = unsafe { self.connections.selectedSegment() };
        if let Some(c) = CONNECTIONS.get(segment.max(0) as usize) {
            f.connections = *c;
        }
        f.always = kit::is_on(&self.always);
        f.remember = kit::is_on(&self.remember);
        f.verify = kit::is_on(&self.verify);
        f.checksum = kit::text_of(&self.checksum);
        f.username = kit::text_of(&self.username);
        f.password = kit::text_of(&self.password);
        f.referrer = kit::text_of(&self.referrer);
        f.cookies = kit::text_of(&self.cookies);
        f.user_agent = kit::text_of(&self.user_agent);
        f.headers = self.header_fields.borrow().iter().map(|(k, v)| (kit::text_of(k), kit::text_of(v))).collect();
        link_changed
    }

    /// A field changed.
    fn edited(self: &Rc<Self>) {
        if self.pull() {
            self.link_changed();
        }
        self.sync();
    }

    /// Show the form's state in the controls.
    fn sync(&self) {
        let f = self.form.borrow();
        let app = app();
        let category_name = app.store.borrow().category_name(&f.category);
        kit::show(&self.url_row, !f.batch);
        kit::show(&self.batch_row, f.batch);
        kit::show(&self.name_row, !f.batch);
        kit::show(&self.category_row, !f.batch);
        if let Some(parent) = unsafe { self.url_error.superview() } {
            kit::show(&parent, f.url_error());
        }
        kit::set_text(&self.batch_count, &format!("{} files", f.batch_urls.len()));
        kit::set_text(&self.name, &f.name);
        kit::set_text(&self.dir, &f.dir);
        if let Some(i) = self.categories.borrow().iter().position(|c| *c == f.category) {
            unsafe { self.category.selectItemAtIndex(i as isize) };
        }
        if let Some(i) = CONNECTIONS.iter().position(|c| *c == f.connections) {
            unsafe { self.connections.setSelectedSegment(i as isize) };
        }
        kit::enable(&self.connections, f.can_split());

        // the line under the URL
        let info = f.info();
        kit::show(&self.info_row, info != Info::Hidden);
        unsafe { self.info_icon.setImage(Some(&system::file_icon(&f.name_ext()))) };
        let (title, detail, spinning, bad) = match &info {
            Info::Hidden => (String::new(), String::new(), false, false),
            Info::Checking => ("Checking link…".to_string(), String::new(), true, false),
            Info::Web { host } => (host.clone(), "Opened from a web page. The link has not been contacted yet.".to_string(), false, false),
            Info::Failed { error } => (error.clone(), "You can still add it; it will retry when started.".to_string(), false, true),
            Info::Probed { summary, detail, .. } => (summary.clone(), detail.clone(), false, false),
        };
        kit::set_text(&self.info_title, &title);
        kit::set_text(&self.info_detail, &detail);
        kit::show(&self.info_detail, !detail.is_empty());
        unsafe {
            self.info_title.setTextColor(Some(&*if bad { NSColor::systemRedColor() } else { NSColor::labelColor() }));
            if spinning {
                self.info_spinner.startAnimation(None);
            } else {
                self.info_spinner.stopAnimation(None);
            }
        }
        kit::show(&self.check, f.from_web && !spinning);
        let again = f.probe.is_some() || !f.probe_error.is_empty();
        unsafe { self.check.setTitle(&ns(if again { "Check Again" } else { "Check Link" })) };

        kit::show(&self.always, f.shows_always());
        unsafe {
            self.always.setTitle(&ns(&format!("Always put .{} files in {category_name}", f.name_ext())));
            self.remember.setTitle(&ns(&format!("Remember this folder for {category_name}")));
        }
        kit::show(&self.options_row, !f.batch);
        kit::show(&self.checksum, f.verify);
        kit::show(&self.advanced_box, kit::is_on(&self.advanced));

        let ready = f.valid() && !self.busy.get();
        kit::enable(&self.queue_button, ready);
        kit::enable(&self.start_button, ready);
        let title = if f.batch { format!("Start {} Downloads", f.batch_urls.len()) } else { "Start Download".to_string() };
        unsafe { self.start_button.setTitle(&ns(&title)) };
    }

    fn batch_toggled(self: &Rc<Self>) {
        self.form.borrow_mut().batch = kit::switch_on(&self.batch);
        self.link_changed();
        self.sync();
    }

    fn category_chosen(self: &Rc<Self>) {
        let id = self.categories.borrow().get(kit::popup_index(&self.category)).cloned();
        if let Some(id) = id {
            self.pull();
            let folder = folder_for(&id);
            self.form.borrow_mut().choose_category(&id, folder);
            self.sync();
        }
    }

    fn browse(self: &Rc<Self>) {
        let start = self.form.borrow().dir.clone();
        if let Some(folder) = kit::choose_folder(&start) {
            kit::set_text(&self.dir, &folder);
            self.edited();
        }
    }

    fn add_header(self: &Rc<Self>) {
        let edited = call(&Rc::downgrade(self), |w| w.edited());
        let key = kit::text_field("Header", edited.clone());
        let value = kit::text_field("Value", edited);
        let line = kit::hstack(6.0, &[&key, &value]);
        let remove = kit::icon_button("minus.circle", "Remove this header", {
            let weak = Rc::downgrade(self);
            let (line, key) = (objc2::rc::Weak::from_retained(&line), objc2::rc::Weak::from_retained(&key));
            move || {
                let (Some(w), Some(line), Some(key)) = (weak.upgrade(), line.load(), key.load()) else { return };
                w.header_fields.borrow_mut().retain(|(k, _)| !std::ptr::eq(&**k, &*key));
                unsafe { line.removeFromSuperview() };
                w.edited();
            }
        });
        unsafe {
            key.widthAnchor().constraintEqualToAnchor(&value.widthAnchor()).setActive(true);
            line.addArrangedSubview(&remove);
        }
        kit::add_row(&self.headers_box, &line);
        self.header_fields.borrow_mut().push((key, value));
    }

    // -- the link ------------------------------------------------------------

    /// The link (or the batch text) changed: forget what was known and,
    /// after a pause in typing, look again.
    fn link_changed(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let (batch, wants_probe) = {
            let mut f = self.form.borrow_mut();
            if !f.batch {
                f.link_changed();
            }
            (f.batch, f.wants_probe())
        };
        if !batch && !wants_probe {
            return;
        }
        let weak = Rc::downgrade(self);
        kit::after(0.3, move || {
            let Some(w) = weak.upgrade() else { return };
            if w.generation.get() != generation {
                return;
            }
            if batch {
                w.expand_batch(generation);
            } else {
                w.probe(generation);
            }
        });
    }

    /// Probe right away (the Check Link button).
    fn probe_now(self: &Rc<Self>) {
        self.pull();
        {
            let mut f = self.form.borrow_mut();
            if f.batch || !fmt::is_http(f.link()) || f.probing {
                return;
            }
            f.probe = None;
            f.probe_error.clear();
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.probe(generation);
    }

    fn probe(self: &Rc<Self>, generation: u64) {
        let (link, opts) = {
            let mut f = self.form.borrow_mut();
            f.probing = true;
            (f.link().to_string(), f.probe_opts())
        };
        self.sync();
        let weak = Rc::downgrade(self);
        engine::spawn("probe_url", json!({ "url": link, "opts": opts }), move |result| {
            let Some(w) = weak.upgrade() else { return };
            if w.generation.get() != generation {
                return;
            }
            match result {
                Ok(p) => w.form.borrow_mut().probed(p, default_connections(), folder_for),
                Err(e) => w.form.borrow_mut().probe_failed(e),
            }
            w.sync();
        });
    }

    fn expand_batch(self: &Rc<Self>, generation: u64) {
        let lines = self.form.borrow().batch_lines();
        let weak = Rc::downgrade(self);
        engine::background(
            move || {
                let mut links = vec![];
                for (line, pattern) in lines {
                    if pattern {
                        if let Ok(list) = engine::call("expand_batch", json!({ "pattern": line })) {
                            links.extend(odm_ui::strings(&json!({ "l": list }), "l"));
                        }
                    } else {
                        links.push(line);
                    }
                }
                links
            },
            move |links| {
                let Some(w) = weak.upgrade() else { return };
                if w.generation.get() == generation {
                    w.form.borrow_mut().set_batch_urls(links);
                    w.sync();
                }
            },
        );
    }

    // -- adding --------------------------------------------------------------

    fn submit(self: &Rc<Self>, start: bool, on_exists: Option<&'static str>) {
        self.pull();
        if !self.form.borrow().valid() || self.busy.get() {
            return;
        }
        kit::set_text(&self.error, "");
        if on_exists.is_none() {
            let target = {
                let f = self.form.borrow();
                f.target(folder_for(&f.category))
            };
            if let Some((dir, name)) = target {
                let exists = engine::get("target_exists", json!({ "dir": dir, "name": name })).and_then(|v| v.as_bool()).unwrap_or(false);
                if exists {
                    let choice = kit::ask(
                        Some(&self.window),
                        &format!("“{name}” already exists in this folder."),
                        "",
                        &["Rename New File", "Resume Existing File", "Overwrite", "Cancel"],
                        Some(2),
                    );
                    let how = match choice {
                        0 => "rename",
                        1 => "resume",
                        2 => "overwrite",
                        _ => return,
                    };
                    return self.submit(start, Some(how));
                }
            }
        }
        let (requests, assignment) = {
            let f = self.form.borrow();
            (f.requests(start, on_exists), f.assignment())
        };
        self.busy.set(true);
        self.sync();
        let show_progress = start && app().setting("showProgressOnStart", false);
        let weak = Rc::downgrade(self);
        engine::background(
            move || -> Result<Option<String>, String> {
                if let Some((ext, category)) = assignment {
                    engine::call("assign_extensions", json!({ "category": category, "extensions": [ext] }))?;
                }
                if let [one] = requests.as_slice() {
                    let id = engine::call("add_download", json!({ "req": one }))?;
                    Ok(id.as_str().map(String::from))
                } else {
                    engine::call("add_many", json!({ "reqs": requests })).map(|_| None)
                }
            },
            move |result| {
                let Some(w) = weak.upgrade() else { return };
                match result {
                    Ok(id) => {
                        if let (true, Some(id)) = (show_progress, id) {
                            app().open_progress(&id);
                        }
                        w.window.performClose(None);
                    }
                    Err(e) => {
                        w.busy.set(false);
                        kit::set_text(&w.error, &e);
                        w.sync();
                    }
                }
            },
        );
    }
}

#[cfg(debug_assertions)]
impl AddWindow {
    /// Press Start Download (see `ODM_DO`).
    pub fn debug_start(&self) {
        let f = self.form.borrow();
        eprintln!("add: valid {} info {:?} name {:?} dir {:?}", f.valid(), f.info(), f.name, f.dir);
        drop(f);
        unsafe { self.start_button.performClick(None) };
    }
}
