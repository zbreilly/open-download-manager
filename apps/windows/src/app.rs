//! Application state that outlives any window: the store fed by engine
//! events, which windows exist, the notification-area icon, and what
//! happens when downloads and queues finish.
use crate::system::{self, Posted};
use crate::{add_window, engine, grabber_window, main_window, progress_window, scheduler_window, settings_window};
use odm_ui::store::{Change, Effect, Store};
use odm_ui::{b, fmt, incoming, n, s};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows_reactor::*;

/// What a window asks of the application.
pub enum Action {
    /// open an Add window (the `show_add` payload, `{batch: true}`, …)
    Add(Value),
    Progress(String),
    Scheduler,
    /// the grabber, optionally with a list of links (`show_links` payload)
    Grabber(Option<Value>),
    Settings,
    /// an Add window is done (its key)
    AddDone(u64),
    ProgressDone(String),
    MainClosed,
    CancelPower,
    SaveSettings(Value),
    /// tell the user
    Error(String),
    Quit,
}

pub enum Message {
    Posted(Posted),
    Act(Action),
    Tray(Key),
    ShowMain,
}

#[derive(Clone, PartialEq)]
pub struct Power {
    pub action: String,
    pub queue: String,
    pub seconds_left: u64,
}

/// What every window gets from the application. Compared by revision, so
/// a window redraws exactly when something it may show has changed.
#[derive(Clone)]
pub struct Shared {
    pub store: Rc<RefCell<Store>>,
    /// bumped on every change
    pub revision: u64,
    /// bumped when list membership, queues or settings change
    pub list_revision: u64,
    pub act: Callback<Action>,
    pub power: Option<Power>,
    pub start_error: Option<Rc<str>>,
    /// the latest thing to tell the user, with a serial number
    pub error: Option<(u64, Rc<str>)>,
}

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision && self.power == other.power && self.error == other.error
    }
}

impl Shared {
    pub fn theme(&self) -> WindowTheme {
        match s(&self.store.borrow().settings, "theme") {
            "light" => WindowTheme::Light,
            "system" => WindowTheme::System,
            _ => WindowTheme::Dark,
        }
    }

    /// Standard look of every window: theme, backdrop, icon and size.
    pub fn visuals(&self, width: f64, height: f64) -> WindowVisuals {
        WindowVisuals::new()
            .theme(self.theme())
            .backdrop(WindowBackdrop::Mica)
            .icon(system::asset("icon.ico"))
            .client_size(width, height)
    }

    pub fn act(&self, a: Action) {
        self.act.call(a);
    }

    pub fn report(&self, message: impl Into<String>) {
        self.act(Action::Error(message.into()));
    }
}

pub struct Odm {
    store: Rc<RefCell<Store>>,
    revision: u64,
    list_revision: u64,
    start_error: Option<Rc<str>>,
    error: Option<(u64, Rc<str>)>,
    /// open Add windows: (key, params)
    adds: Vec<(u64, Rc<Value>)>,
    next_add: u64,
    /// ids with a progress window
    progress: Vec<String>,
    /// the list of links the grabber should take next, with a serial
    links: Option<(u64, Rc<Value>)>,
    power: Option<(String, String, Instant)>,
    clipboard: u32,
    last_tray: &'static str,
    /// there is a taskbar, so the notification-area icon is declared and
    /// closing the main window can leave the app running there
    has_tray: bool,
    quitting: bool,
}

/// Fire an engine call off the UI thread; an error is shown to the user.
pub fn act(method: &'static str, args: Value) {
    let _ = std::thread::Builder::new().name("odm-call".into()).spawn(move || {
        if let Err(e) = engine::call(method, args) {
            system::post(Posted::Error(e));
        }
    });
}

impl Odm {
    fn shared(&self, context: &ApplicationViewContext<Self>) -> Shared {
        Shared {
            store: self.store.clone(),
            revision: self.revision,
            list_revision: self.list_revision,
            act: context.callback(Message::Act),
            power: self.power.as_ref().map(|(action, queue, deadline)| Power {
                action: action.clone(),
                queue: queue.clone(),
                seconds_left: deadline.saturating_duration_since(Instant::now()).as_secs(),
            }),
            start_error: self.start_error.clone(),
            error: self.error.clone(),
        }
    }

    fn touch(&mut self, change: Change) {
        match change {
            Change::None => return,
            Change::List | Change::Queues | Change::Settings => self.list_revision += 1,
            _ => {}
        }
        self.revision += 1;
    }

    fn report(&mut self, message: String) {
        let serial = self.error.as_ref().map(|(n, _)| n + 1).unwrap_or(1);
        self.error = Some((serial, message.into()));
        self.revision += 1;
    }

    fn setting(&self, key: &str, default: bool) -> bool {
        self.store.borrow().setting(key, default)
    }

    fn open_add(&mut self, params: Value, context: &ApplicationContext<Self>) {
        self.next_add += 1;
        self.adds.push((self.next_add, Rc::new(params)));
        self.revision += 1;
        context.show_window(format!("add:{}", self.next_add));
    }

    fn open_progress(&mut self, id: &str, context: &ApplicationContext<Self>) {
        if self.store.borrow().get(id).is_none() {
            return;
        }
        if !self.progress.iter().any(|p| p == id) {
            self.progress.push(id.to_string());
            self.revision += 1;
        }
        context.show_window(format!("progress:{id}"));
    }

    fn open_grabber(&mut self, links: Option<Value>, context: &ApplicationContext<Self>) {
        if let Some(l) = links {
            let serial = self.links.as_ref().map(|(n, _)| n + 1).unwrap_or(1);
            self.links = Some((serial, Rc::new(l)));
            self.revision += 1;
        }
        context.show_window("grabber");
    }

    /// Arguments of this launch or a later one: links open the Add window,
    /// anything else brings the app up.
    fn handle_args(&mut self, args: &[String], later_launch: bool, context: &ApplicationContext<Self>) {
        let items = incoming::parse(args).items;
        if items.is_empty() && later_launch {
            context.show_window("main");
        }
        for item in items {
            match item {
                incoming::Incoming::Add(p) => self.open_add(p, context),
                incoming::Incoming::Show => context.show_window("main"),
            }
        }
    }

    fn engine_event(&mut self, name: &str, payload: &str, context: &ApplicationContext<Self>) {
        let v: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let (change, effect) = self.store.borrow_mut().handle(name, v);
        self.touch(change);
        if change == Change::Totals {
            system::keep_awake(n(&self.store.borrow().totals, "active") > 0);
        }
        match effect {
            Some(Effect::Completed(d)) => self.completed(&d, context),
            Some(Effect::Failed(d)) => {
                if self.setting("notifyFailure", true) {
                    let why = d.get("error").and_then(Value::as_str).unwrap_or("Unknown error");
                    system::notify(&format!("Download failed: {}", s(&d, "fileName")), why);
                }
            }
            Some(Effect::QueueFinished(q)) => {
                if self.setting("notifyQueueFinished", true) {
                    system::notify("Queue finished", &format!("{} has no more files to download", s(&q, "name")));
                }
                let action = s(&q, "onFinish");
                if matches!(action, "sleep" | "shutdown" | "exit") {
                    self.power = Some((action.into(), s(&q, "name").into(), Instant::now() + Duration::from_secs(60)));
                    self.revision += 1;
                    context.show_window("main");
                }
            }
            Some(Effect::ShowAdd(p)) => self.open_add(p, context),
            Some(Effect::ShowLinks(p)) => self.open_grabber(Some(p), context),
            None => {}
        }
    }

    fn completed(&mut self, d: &Value, context: &ApplicationContext<Self>) {
        let path = Path::new(s(d, "saveDir")).join(s(d, "fileName"));
        match s(d, "whenFinished") {
            "openFile" => {
                let _ = system::open_path(&path);
            }
            "openFolder" => {
                let _ = system::reveal_download(d);
            }
            "none" => {}
            _ => {
                if self.setting("notifyComplete", true) {
                    system::notify("Download complete", s(d, "fileName"));
                }
            }
        }
        if self.setting("showCompleteDialog", false) {
            self.open_progress(s(d, "id"), context);
        }
    }

    /// Once a second: the power countdown, the clipboard monitor and the
    /// tray icon going back to normal after a finished download.
    fn tick(&mut self, context: &ApplicationContext<Self>) {
        if let Some((action, _, deadline)) = self.power.clone() {
            self.revision += 1;
            if Instant::now() >= deadline {
                self.power = None;
                let done = match action.as_str() {
                    "exit" => return self.quit(context),
                    "sleep" => system::sleep(),
                    _ => {
                        engine::shutdown();
                        system::shut_down()
                    }
                };
                if let Err(e) = done {
                    self.report(e);
                }
            }
        }
        // the taskbar can come and go (Explorer restarting)
        let has_tray = system::tray_available();
        if has_tray != self.has_tray {
            self.has_tray = has_tray;
            self.revision += 1;
        }
        let tray = self.store.borrow().tray_state();
        if tray != self.last_tray {
            self.last_tray = tray;
            self.revision += 1;
        }
        let sequence = system::clipboard_sequence();
        if sequence != self.clipboard {
            self.clipboard = sequence;
            if self.setting("clipboardMonitor", false) {
                let found = system::clipboard_text().and_then(|text| {
                    let st = self.store.borrow();
                    odm_ui::settings::clipboard_candidate(&text, &st.settings, st.downloads())
                });
                if let Some(url) = found {
                    self.open_add(json!({ "url": url }), context);
                }
            }
        }
    }

    /// Save settings. When the engine refuses them (e.g. the new bridge
    /// port is taken) the error is shown and the engine's copy comes back
    /// with the next `settings_changed` or reload.
    fn save_settings(&mut self, mut wanted: Value) {
        let was = b(&self.store.borrow().settings, "launchAtLogin");
        let want = b(&wanted, "launchAtLogin");
        if was != want {
            if let Err(e) = system::set_autostart(want) {
                self.report(e);
            }
            // keep the setting equal to what actually happened
            wanted["launchAtLogin"] = system::autostart_enabled().into();
        }
        self.store.borrow_mut().settings = wanted.clone();
        self.touch(Change::Settings);
        let _ = std::thread::Builder::new().name("odm-call".into()).spawn(move || {
            if let Err(e) = engine::call("save_settings", json!({ "settings": wanted })) {
                system::post(Posted::Error(e));
                // nothing was stored: go back to what the engine has
                if let Ok(real) = engine::call("get_settings", json!({})) {
                    system::post(Posted::Engine("settings_changed".into(), real.to_string()));
                }
            }
        });
    }

    fn quit(&mut self, context: &ApplicationContext<Self>) {
        if std::mem::replace(&mut self.quitting, true) {
            return;
        }
        // stops downloads and saves state (waits up to 5 s)
        engine::shutdown();
        system::keep_awake(false);
        context.exit();
    }

    fn action(&mut self, a: Action, context: &ApplicationContext<Self>) {
        match a {
            Action::Add(p) => self.open_add(p, context),
            Action::Progress(id) => self.open_progress(&id, context),
            Action::Scheduler => context.show_window("scheduler"),
            Action::Grabber(links) => self.open_grabber(links, context),
            Action::Settings => context.show_window("settings"),
            Action::AddDone(key) => {
                self.adds.retain(|(k, _)| *k != key);
                self.revision += 1;
            }
            Action::ProgressDone(id) => {
                self.progress.retain(|p| *p != id);
                self.revision += 1;
            }
            Action::MainClosed => {
                // without "keep running in the notification area" (or an
                // area to run in) closing the main window ends the app
                if !self.quitting && !(self.setting("minimizeToTray", true) && self.has_tray) {
                    self.quit(context);
                }
            }
            Action::CancelPower => {
                self.power = None;
                self.revision += 1;
            }
            Action::SaveSettings(s) => self.save_settings(s),
            Action::Error(e) => self.report(e),
            Action::Quit => self.quit(context),
        }
    }
}

impl Application for Odm {
    type Input = Vec<String>;
    type Message = Message;

    fn create(args: &Vec<String>, context: &ApplicationContext<Self>) -> Self {
        let sender = context.sender();
        system::install_pump(move |p| {
            let _ = sender.send(Message::Posted(p));
        });
        let mut store = Store::default();
        let started = engine::start(&engine::data_dir(), |name, payload| {
            system::post(Posted::Engine(name.to_string(), payload.to_string()));
        });
        if started.is_ok() {
            store.load(|m| engine::call(m, json!({})).ok());
            // the Run entry can be changed outside the app: follow it
            let real = system::autostart_enabled();
            if store.settings.is_object() && b(&store.settings, "launchAtLogin") != real {
                store.settings["launchAtLogin"] = real.into();
                act("save_settings", json!({ "settings": store.settings.clone() }));
            }
            system::keep_awake(n(&store.totals, "active") > 0);
        }
        let mut app = Odm {
            store: Rc::new(RefCell::new(store)),
            revision: 0,
            list_revision: 0,
            start_error: started.err().map(Rc::from),
            error: None,
            adds: vec![],
            next_add: 0,
            progress: vec![],
            links: None,
            power: None,
            clipboard: system::clipboard_sequence(),
            last_tray: "idle",
            has_tray: system::tray_available(),
            quitting: false,
        };
        // started at sign-in: stay in the notification area, if there is one
        let minimized = incoming::parse(args).minimized && app.has_tray;
        if !minimized {
            context.show_window("main");
        }
        app.handle_args(args, false, context);
        app
    }

    fn update(&mut self, message: Message, context: &ApplicationContext<Self>) {
        match message {
            Message::Posted(Posted::Engine(name, payload)) => self.engine_event(&name, &payload, context),
            Message::Posted(Posted::Args(args)) => self.handle_args(&args, true, context),
            Message::Posted(Posted::Error(e)) => self.report(e),
            Message::Posted(Posted::Tick) => self.tick(context),
            Message::Act(a) => self.action(a, context),
            Message::ShowMain => context.show_window("main"),
            Message::Tray(key) => match key.as_str() {
                Some("show") => context.show_window("main"),
                Some("add") => self.open_add(json!({}), context),
                Some("grabber") => context.show_window("grabber"),
                Some("start") => act("start_queue", json!({ "id": "main" })),
                Some("pause") => act("pause_all", json!({})),
                Some("limit") => toggle_limit(&self.store.borrow()),
                Some("scheduler") => context.show_window("scheduler"),
                Some("settings") => context.show_window("settings"),
                Some("quit") => self.quit(context),
                _ => {}
            },
        }
    }

    fn view(&self, _: &Vec<String>, context: &ApplicationViewContext<Self>) -> ApplicationView {
        let shared = self.shared(context);
        let st = self.store.borrow();
        let mut view = ApplicationView::new()
            .window::<main_window::MainWindow>("main", shared.clone())
            .window::<scheduler_window::SchedulerWindow>("scheduler", shared.clone())
            .window::<settings_window::SettingsWindow>("settings", shared.clone())
            .window::<grabber_window::GrabberWindow>("grabber", grabber_window::Input { shared: shared.clone(), links: self.links.clone() });
        for (key, params) in &self.adds {
            view = view.window::<add_window::AddWindow>(
                format!("add:{key}"),
                add_window::Input { shared: shared.clone(), key: *key, params: params.clone() },
            );
        }
        for id in &self.progress {
            view = view.window::<progress_window::ProgressWindow>(
                format!("progress:{id}"),
                progress_window::Input { shared: shared.clone(), id: id.as_str().into() },
            );
        }
        if !self.has_tray {
            // creating the icon without a taskbar would fail the whole app
            return view;
        }
        let line = st.activity_line();
        let limiter = if b(&st.settings, "globalLimitEnabled") { "Speed Limiter: On" } else { "Speed Limiter: Off" };
        let tooltip = if n(&st.totals, "active") > 0 { format!("{}\n{line}", odm_ui::APP_NAME) } else { odm_ui::APP_NAME.to_string() };
        view.notify_icon(
            "tray",
            NotifyIcon::new(system::asset(&format!("tray\\tray-{}.ico", st.tray_state())))
                .tooltip(tooltip)
                .on_activate(context.callback(|_| Message::ShowMain))
                .menu(Menu::new(
                    [
                        MenuItem::disabled("status", line),
                        MenuItem::separator("s1"),
                        MenuItem::item("show", "Show Open Download Manager"),
                        MenuItem::item("add", "Add URL…"),
                        MenuItem::item("grabber", "Site Grabber…"),
                        MenuItem::separator("s2"),
                        MenuItem::item("start", "Start Main Queue"),
                        MenuItem::item("pause", "Pause All"),
                        MenuItem::item("limit", limiter),
                        MenuItem::separator("s3"),
                        MenuItem::item("scheduler", "Scheduler…"),
                        MenuItem::item("settings", "Settings…"),
                        MenuItem::separator("s4"),
                        MenuItem::item("quit", "Quit"),
                    ],
                    context.callback(Message::Tray),
                )),
        )
    }
}

/// Turn the global speed limit on (at the configured speed) or off.
pub fn toggle_limit(store: &Store) {
    let bps = if b(&store.settings, "globalLimitEnabled") { Value::Null } else { json!(n(&store.settings, "globalLimitBps")) };
    act("set_global_limit", json!({ "bps": bps }));
}

/// "25 MB/s" or "No Limit", for the toolbar.
pub fn limit_label(store: &Store) -> String {
    if b(&store.settings, "globalLimitEnabled") {
        format!("{} MB/s", fmt::mbps(n(&store.settings, "globalLimitBps")))
    } else {
        "No Limit".into()
    }
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    App::run_application::<Odm>(args).map_err(|e| e.to_string())
}
