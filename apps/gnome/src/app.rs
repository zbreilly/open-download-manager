//! App-wide state fed by engine events, and the actions windows share.
use crate::row::{b, s, Row};
use crate::window::MainWindow;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use odm_ffi::api;
use odm_linux::shell::{self, Shell, ShellEvent};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Everything that reaches the GUI thread from other threads.
pub enum Msg {
    Engine(String, String),
    Shell(ShellEvent),
}

/// What changed, for windows that keep themselves up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// list membership, or a row's status, name, category or queue
    List,
    /// progress of some rows
    Progress,
    Queues,
    Settings,
    Totals,
    /// browser extension / bridge status
    Ext,
    /// the power countdown started or was cancelled
    Power,
}

#[derive(Clone)]
pub struct PowerCountdown {
    pub action: String,
    pub queue: String,
    pub deadline: Instant,
}

type Listener = Box<dyn Fn(Change) -> bool>;

pub struct App {
    pub gtk: adw::Application,
    pub shell: Shell,
    /// every download, as `Row` objects (the model behind all lists)
    pub rows: gio::ListStore,
    index: RefCell<HashMap<String, Row>>,
    pub queues: RefCell<Vec<Value>>,
    pub settings: RefCell<Value>,
    pub totals: RefCell<Value>,
    pub ext: RefCell<Value>,
    pub power: RefCell<Option<PowerCountdown>>,
    pub start_error: RefCell<Option<String>>,
    listeners: RefCell<Vec<Listener>>,
    main: RefCell<Option<MainWindow>>,
    progress: RefCell<HashMap<String, glib::WeakRef<adw::Window>>>,
    singles: RefCell<HashMap<&'static str, glib::WeakRef<gtk::Window>>>,
    quitting: Cell<bool>,
    _hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// The app (GUI thread only, after startup).
pub fn odm() -> Rc<App> {
    APP.with(|a| a.borrow().clone().expect("app started"))
}

/// Run an engine call off the GUI thread and await its result.
pub async fn call(method: &'static str, args: Value) -> Result<Value, String> {
    gio::spawn_blocking(move || api::call(method, args))
        .await
        .unwrap_or_else(|_| Err("the engine call was interrupted".into()))
}

impl App {
    pub fn start(gtk_app: &adw::Application, shell: Shell, tx: async_channel::Sender<Msg>, rx: async_channel::Receiver<Msg>) {
        let app = Rc::new(App {
            gtk: gtk_app.clone(),
            shell,
            rows: gio::ListStore::new::<Row>(),
            index: RefCell::default(),
            queues: RefCell::default(),
            settings: RefCell::new(Value::Null),
            totals: RefCell::new(Value::Null),
            ext: RefCell::new(json!({ "connected": false, "browsers": [] })),
            power: RefCell::default(),
            start_error: RefCell::default(),
            listeners: RefCell::default(),
            main: RefCell::default(),
            progress: RefCell::default(),
            singles: RefCell::default(),
            quitting: Cell::new(false),
            // downloads keep running with every window closed
            _hold: RefCell::new(Some(gtk_app.hold())),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));

        let for_shell = app.shell.clone();
        let started = api::start(&odm_linux::paths::data_dir(), move |name, payload| {
            for_shell.engine_event(name, payload);
            let _ = tx.send_blocking(Msg::Engine(name.to_string(), payload.to_string()));
        });
        match started {
            Ok(()) => app.reload(),
            Err(e) => *app.start_error.borrow_mut() = Some(e),
        }
        app.sync_autostart();
        app.apply_theme();
        crate::window::install_actions(&app);

        let main = MainWindow::new(&app);
        *app.main.borrow_mut() = Some(main);
        if !(app.shell.minimized && app.shell.has_tray()) {
            app.show_main();
        }

        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let app = self::odm();
                match msg {
                    Msg::Engine(name, payload) => app.engine_event(&name, &payload),
                    Msg::Shell(ev) => app.shell_event(ev),
                }
            }
        });
        app.watch_clipboard();
        app.shell.ready();
        #[cfg(debug_assertions)]
        crate::debug::open_windows();
    }

    // ---------------------------------------------------------------- state

    fn reload(&self) {
        let get = |m: &str| api::call(m, json!({})).ok();
        if let Some(Value::Array(q)) = get("list_queues") {
            *self.queues.borrow_mut() = q;
        }
        if let Some(Value::Array(list)) = get("list_downloads") {
            self.rows.remove_all();
            self.index.borrow_mut().clear();
            for d in list {
                self.upsert(d);
            }
        }
        if let Some(v) = get("get_settings") {
            *self.settings.borrow_mut() = v;
        }
        if let Some(v) = get("totals") {
            *self.totals.borrow_mut() = v;
        }
        if let Some(v) = get("extension_status") {
            *self.ext.borrow_mut() = v;
        }
    }

    /// Call `f` on every change until it returns false (its window is gone).
    pub fn subscribe(&self, f: impl Fn(Change) -> bool + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Call `f` on every change until `window` is closed.
    pub fn subscribe_while_open(&self, window: &impl IsA<gtk::Window>, f: impl Fn(Change) + 'static) {
        let open = Rc::new(Cell::new(true));
        let flag = open.clone();
        window.connect_close_request(move |_| {
            flag.set(false);
            glib::Propagation::Proceed
        });
        self.subscribe(move |c| {
            if open.get() {
                f(c);
            }
            open.get()
        });
    }

    /// Call `f` once a second until `window` is closed.
    pub fn every_second_while_open(&self, window: &impl IsA<gtk::Window>, f: impl Fn() + 'static) {
        let open = Rc::new(Cell::new(true));
        let flag = open.clone();
        window.connect_close_request(move |_| {
            flag.set(false);
            glib::Propagation::Proceed
        });
        glib::timeout_add_local(Duration::from_secs(1), move || {
            if !open.get() {
                return glib::ControlFlow::Break;
            }
            f();
            glib::ControlFlow::Continue
        });
    }

    pub fn emit(&self, c: Change) {
        // listeners may subscribe others while they run
        let current = std::mem::take(&mut *self.listeners.borrow_mut());
        let mut kept: Vec<Listener> = current.into_iter().filter(|f| f(c)).collect();
        let mut l = self.listeners.borrow_mut();
        kept.append(&mut l);
        *l = kept;
    }

    pub fn row(&self, id: &str) -> Option<Row> {
        self.index.borrow().get(id).cloned()
    }

    pub fn all_rows(&self) -> Vec<Row> {
        (0..self.rows.n_items()).filter_map(|i| self.rows.item(i).and_downcast::<Row>()).collect()
    }

    pub fn queue_name(&self, id: Option<&str>) -> String {
        let Some(id) = id else { return odm_linux::fmt::DASH.into() };
        self.queues
            .borrow()
            .iter()
            .find(|q| s(q, "id") == id)
            .map(|q| s(q, "name").to_string())
            .unwrap_or_else(|| odm_linux::fmt::DASH.into())
    }

    pub fn queue(&self, id: &str) -> Option<Value> {
        self.queues.borrow().iter().find(|q| s(q, "id") == id).cloned()
    }

    pub fn categories(&self) -> Vec<Value> {
        self.settings.borrow().get("categories").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    pub fn setting_on(&self, key: &str, default: bool) -> bool {
        self.settings.borrow().get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    fn upsert(&self, d: Value) {
        let id = s(&d, "id").to_string();
        let queue = self.queue_name(d.get("queueId").and_then(Value::as_str));
        let existing = self.index.borrow().get(&id).cloned();
        match existing {
            Some(row) => row.apply(d, &queue),
            None => {
                let row = Row::new(d, &queue);
                self.rows.append(&row);
                self.index.borrow_mut().insert(id, row);
            }
        }
    }

    fn engine_event(&self, name: &str, payload: &str) {
        let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
        match name {
            "progress_batch" => {
                let mut status_changed = false;
                for p in v.as_array().into_iter().flatten() {
                    if let Some(row) = self.row(s(p, "id")) {
                        status_changed |= row.progress(p);
                    }
                }
                self.emit(if status_changed { Change::List } else { Change::Progress });
            }
            "download_added" | "download_changed" => {
                self.upsert(v);
                self.emit(Change::List);
            }
            "download_removed" => {
                let Some(id) = v.as_str() else { return };
                if let Some(row) = self.index.borrow_mut().remove(id) {
                    if let Some(pos) = self.rows.find(&row) {
                        self.rows.remove(pos);
                    }
                }
                self.emit(Change::List);
            }
            "totals" => {
                *self.totals.borrow_mut() = v;
                for row in self.all_rows() {
                    if matches!(row.state().as_str(), "downloading" | "connecting") {
                        row.sample_speed();
                    }
                }
                self.emit(Change::Totals);
            }
            "queues_changed" => {
                if let Value::Array(q) = v {
                    *self.queues.borrow_mut() = q;
                }
                for row in self.all_rows() {
                    let name = row.with(|d| self.queue_name(d.get("queueId").and_then(Value::as_str)));
                    if row.queue() != name {
                        row.set_queue(name);
                    }
                }
                self.emit(Change::Queues);
            }
            "settings_changed" => {
                *self.settings.borrow_mut() = v;
                self.apply_theme();
                self.emit(Change::Settings);
            }
            "extension_status" => {
                // the event carries connection state only; keep the bridge error
                let mut ext = v;
                ext["bridgeError"] = self.ext.borrow().get("bridgeError").cloned().unwrap_or(Value::Null);
                ext["port"] = self.ext.borrow().get("port").cloned().unwrap_or(Value::Null);
                if *self.ext.borrow() != ext {
                    *self.ext.borrow_mut() = ext;
                    self.emit(Change::Ext);
                }
            }
            "bridge_error" => {
                let msg = v.get("error").cloned().unwrap_or(Value::Null);
                self.ext.borrow_mut()["bridgeError"] = msg;
                self.emit(Change::Ext);
            }
            _ => {}
        }
    }

    fn shell_event(self: &Rc<Self>, ev: ShellEvent) {
        match ev {
            ShellEvent::Show => self.show_main(),
            ShellEvent::Add(p) => crate::add::open(self, p),
            ShellEvent::Links(p) => crate::grabber::open(self, Some(p)),
            ShellEvent::Grabber => crate::grabber::open(self, None),
            ShellEvent::Scheduler => crate::scheduler::open(self),
            ShellEvent::Settings => crate::settings::open(self),
            ShellEvent::Progress(id) => self.open_progress(&id),
            ShellEvent::Error(e) => self.report(&e),
            ShellEvent::Quit => self.quit(),
            ShellEvent::PowerCountdown { action, queue } => {
                let deadline = Instant::now() + Duration::from_secs(60);
                *self.power.borrow_mut() = Some(PowerCountdown { action, queue, deadline });
                self.show_main();
                self.emit(Change::Power);
            }
        }
    }

    // -------------------------------------------------------------- windows

    pub fn main_window(&self) -> Option<adw::ApplicationWindow> {
        self.main.borrow().as_ref().map(|m| m.window.clone())
    }

    pub fn show_main(&self) {
        if let Some(w) = self.main_window() {
            w.present();
        }
    }

    pub fn open_progress(self: &Rc<Self>, id: &str) {
        let open = self.progress.borrow().get(id).and_then(|w| w.upgrade());
        if let Some(w) = open {
            w.present();
            return;
        }
        if self.row(id).is_none() {
            return;
        }
        let w = crate::progress::build(self, id);
        self.progress.borrow_mut().insert(id.to_string(), w.downgrade());
        w.present();
    }

    /// Present the one window of this kind, building it when there is none.
    pub fn single(&self, kind: &'static str, build: impl FnOnce() -> gtk::Window) -> gtk::Window {
        let open = self.singles.borrow().get(kind).and_then(|w| w.upgrade());
        let w = open.unwrap_or_else(|| {
            let w = build();
            self.singles.borrow_mut().insert(kind, w.downgrade());
            w
        });
        w.present();
        w
    }

    /// Tell the user something went wrong.
    pub fn report(&self, msg: &str) {
        match self.main.borrow().as_ref() {
            Some(m) if m.window.is_visible() => m.toast(msg),
            _ => eprintln!("open-download-manager: {msg}"),
        }
    }

    /// The main window's close button: stay in the tray when there is one
    /// and the user wants that, otherwise quit.
    pub fn main_closed(&self) -> glib::Propagation {
        if self.setting_on("minimizeToTray", true) && self.shell.has_tray() {
            if let Some(w) = self.main_window() {
                w.set_visible(false);
            }
        } else {
            self.quit();
        }
        glib::Propagation::Stop
    }

    pub fn quit(&self) {
        if self.quitting.replace(true) {
            return;
        }
        for w in self.gtk.windows() {
            w.set_visible(false);
        }
        // stops downloads and saves state (waits up to 5 s)
        api::shutdown();
        self._hold.borrow_mut().take();
        self.gtk.quit();
    }

    // -------------------------------------------------------------- actions

    /// Fire an engine call; errors are shown to the user.
    pub fn act(&self, method: &'static str, args: Value) {
        glib::spawn_future_local(async move {
            if let Err(e) = call(method, args).await {
                odm().report(&e);
            }
        });
    }

    pub fn resume(&self, ids: &[String]) {
        self.act("resume", json!({ "ids": ids }));
    }

    pub fn pause(&self, ids: &[String]) {
        self.act("pause", json!({ "ids": ids }));
    }

    pub fn delete(&self, ids: &[String], files: bool) {
        self.act("delete", json!({ "ids": ids, "deleteFiles": files }));
    }

    pub fn set_global_limit(&self, bps: Option<u64>) {
        self.act("set_global_limit", json!({ "bps": bps }));
    }

    pub fn toggle_limit(&self) {
        glib::spawn_future_local(async {
            let done = gio::spawn_blocking(shell::toggle_limiter).await.unwrap_or_else(|_| Err("interrupted".into()));
            if let Err(e) = done {
                odm().report(&e);
            }
        });
    }

    /// Put an extension in a category, and move the listed downloads of
    /// that type along.
    pub fn assign_extension(&self, ext: &str, category: &str) {
        self.act("assign_extensions", json!({ "category": category, "extensions": [ext] }));
        let same: Vec<String> = self
            .all_rows()
            .iter()
            .filter(|r| r.with(|d| odm_linux::fmt::extension(s(d, "fileName")) == ext && s(d, "category") != category))
            .map(|r| r.id())
            .collect();
        if !same.is_empty() {
            self.act("set_category", json!({ "ids": same, "category": category }));
        }
    }

    pub fn open_file(&self, row: &Row) {
        if let Err(e) = odm_linux::open::open_path(&row.with(shell::download_path)) {
            self.report(&e);
        }
    }

    pub fn reveal(&self, row: &Row) {
        if let Err(e) = row.with(shell::reveal_download) {
            self.report(&e);
        }
    }

    pub fn copy_link(&self, row: &Row) {
        if let Some(d) = gdk::Display::default() {
            d.clipboard().set_text(&row.with(|d| s(d, "url").to_string()));
        }
    }

    /// Save settings. When the engine refuses them (e.g. the new bridge
    /// port is taken) the error is shown and the engine's copy comes back.
    pub fn save_settings(&self, mut new: Value) {
        let was = b(&self.settings.borrow(), "launchAtLogin");
        let want = b(&new, "launchAtLogin");
        if was != want {
            if let Err(e) = odm_linux::autostart::set(want) {
                self.report(&e);
            }
            // keep the setting equal to what actually happened
            new["launchAtLogin"] = odm_linux::autostart::is_enabled().into();
        }
        *self.settings.borrow_mut() = new.clone();
        self.apply_theme();
        glib::spawn_future_local(async move {
            if let Err(e) = call("save_settings", json!({ "settings": new })).await {
                let app = odm();
                app.report(&e);
                if let Ok(real) = call("get_settings", json!({})).await {
                    *app.settings.borrow_mut() = real;
                    app.apply_theme();
                    app.emit(Change::Settings);
                }
            }
        });
    }

    /// The autostart file can be removed outside the app: follow it.
    fn sync_autostart(&self) {
        let real = odm_linux::autostart::is_enabled();
        let mut s = self.settings.borrow().clone();
        if !s.is_object() || b(&s, "launchAtLogin") == real {
            return;
        }
        s["launchAtLogin"] = real.into();
        *self.settings.borrow_mut() = s.clone();
        self.act("save_settings", json!({ "settings": s }));
    }

    pub fn apply_theme(&self) {
        let scheme = match s(&self.settings.borrow(), "theme") {
            "light" => adw::ColorScheme::ForceLight,
            "system" => adw::ColorScheme::Default,
            _ => adw::ColorScheme::ForceDark,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    pub fn cancel_power(&self) {
        *self.power.borrow_mut() = None;
        self.emit(Change::Power);
    }

    /// The countdown ran out.
    pub fn run_power(&self) {
        let Some(p) = self.power.borrow_mut().take() else { return };
        self.emit(Change::Power);
        if p.action == "exit" {
            return self.quit();
        }
        let shell = self.shell.clone();
        glib::spawn_future_local(async move {
            let done = gio::spawn_blocking(move || shell.run_power(&p.action)).await;
            if let Ok(Err(e)) = done {
                odm().report(&e);
            }
        });
    }

    /// Offer copied download links in the Add window (when turned on).
    fn watch_clipboard(self: &Rc<Self>) {
        let Some(display) = gdk::Display::default() else { return };
        let clipboard = display.clipboard();
        clipboard.connect_changed(|cb| {
            if cb.is_local() {
                return;
            }
            let cb = cb.clone();
            glib::spawn_future_local(async move {
                let Ok(Some(text)) = cb.read_text_future().await else { return };
                let app = odm();
                let downloads: Vec<Value> = app.all_rows().iter().map(Row::data).collect();
                let found = shell::clipboard_candidate(&text, &app.settings.borrow(), &downloads);
                if let Some(url) = found {
                    crate::add::open(&app, json!({ "url": url }));
                }
            });
        });
    }
}
