//! Application state that outlives any window: the store fed by engine
//! events, which windows exist, and what happens when downloads and queues
//! finish. One instance, on the main thread.
use crate::add_window::AddWindow;
use crate::grabber_window::GrabberWindow;
use crate::main_window::MainWindow;
use crate::progress_window::ProgressWindow;
use crate::scheduler_window::SchedulerWindow;
use crate::settings_window::SettingsWindow;
use crate::{engine, kit, menus, system};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use odm_ui::store::{Change, Effect, Store};
use odm_ui::{b, fmt, incoming, n, s};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The countdown before the Mac sleeps, shuts down or the app quits.
#[derive(Clone)]
pub struct Power {
    pub action: String,
    pub queue: String,
    pub deadline: Instant,
}

pub struct App {
    pub store: RefCell<Store>,
    pub start_error: RefCell<Option<String>>,
    main: RefCell<Option<Rc<MainWindow>>>,
    adds: RefCell<Vec<Rc<AddWindow>>>,
    progress: RefCell<HashMap<String, Rc<ProgressWindow>>>,
    scheduler: RefCell<Option<Rc<SchedulerWindow>>>,
    grabber: RefCell<Option<Rc<GrabberWindow>>>,
    settings: RefCell<Option<Rc<SettingsWindow>>>,
    pub power: RefCell<Option<Power>>,
    clipboard: Cell<isize>,
    last_tray: Cell<&'static str>,
    status: RefCell<Option<menus::StatusItem>>,
    quitting: Cell<bool>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// The application. Only on the main thread, after launch.
pub fn app() -> Rc<App> {
    APP.with(|a| a.borrow().clone()).expect("the application has started")
}

fn try_app() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

impl App {
    fn start() -> Rc<App> {
        let mut store = Store::default();
        let started = engine::start(&engine::data_dir(), |name, payload| {
            let (name, payload) = (name.to_string(), payload.to_string());
            dispatch2::DispatchQueue::main().exec_async(move || {
                if let Some(app) = try_app() {
                    app.engine_event(&name, &payload);
                }
            });
        });
        if let Err(e) = &started {
            system::record_failure(&format!("the download engine failed to start: {e}"));
        } else {
            store.load(|m| engine::get(m, json!({})));
        }
        let app = Rc::new(App {
            store: RefCell::new(store),
            start_error: RefCell::new(started.err()),
            main: RefCell::new(None),
            adds: RefCell::new(vec![]),
            progress: RefCell::new(HashMap::new()),
            scheduler: RefCell::new(None),
            grabber: RefCell::new(None),
            settings: RefCell::new(None),
            power: RefCell::new(None),
            clipboard: Cell::new(system::clipboard_count()),
            last_tray: Cell::new("idle"),
            status: RefCell::new(None),
            quitting: Cell::new(false),
        });
        APP.with(|a| *a.borrow_mut() = Some(app.clone()));
        app
    }

    /// After the store is loaded: follow the system, build the menus, start
    /// the once-a-second tick.
    fn finish_launching(self: &Rc<Self>) {
        if self.start_error.borrow().is_none() {
            // the login item can be changed in System Settings: follow it
            let real = system::autostart_enabled();
            let differs = {
                let st = self.store.borrow();
                st.settings.is_object() && b(&st.settings, "launchAtLogin") != real
            };
            if differs {
                let mut wanted = self.store.borrow().settings.clone();
                wanted["launchAtLogin"] = real.into();
                self.store.borrow_mut().settings = wanted.clone();
                engine::act("save_settings", json!({ "settings": wanted }));
            }
            #[cfg(debug_assertions)]
            if let Some(port) = std::env::var("ODM_PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
                let mut wanted = self.store.borrow().settings.clone();
                wanted["extensionPort"] = port.into();
                self.save_settings(wanted);
            }
        }
        self.apply_appearance();
        menus::install_main_menu();
        *self.status.borrow_mut() = Some(menus::StatusItem::new());
        self.totals_changed();
        system::start_notifications();
        kit::every(1.0, || app().tick());
    }

    pub fn setting(&self, key: &str, default: bool) -> bool {
        self.store.borrow().setting(key, default)
    }

    // -- events ------------------------------------------------------------

    fn engine_event(self: &Rc<Self>, name: &str, payload: &str) {
        let v: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let (change, effect) = self.store.borrow_mut().handle(name, v);
        self.changed(change);
        match effect {
            Some(Effect::Completed(d)) => self.completed(&d),
            Some(Effect::Failed(d)) => {
                if self.setting("notifyFailure", true) {
                    let why = d.get("error").and_then(Value::as_str).unwrap_or("Unknown error");
                    system::notify(&format!("Download failed: {}", s(&d, "fileName")), why);
                }
            }
            Some(Effect::QueueFinished(q)) => self.queue_finished(&q),
            Some(Effect::ShowAdd(p)) => self.open_add(p),
            Some(Effect::ShowLinks(p)) => self.open_grabber(Some(p)),
            None => {}
        }
    }

    /// Tell every window what changed.
    pub fn changed(self: &Rc<Self>, change: Change) {
        if change == Change::None {
            return;
        }
        if change == Change::Settings {
            self.apply_appearance();
        }
        if matches!(change, Change::Totals | Change::Settings) {
            self.totals_changed();
        }
        let main = self.main.borrow().clone();
        if let Some(w) = main {
            w.refresh(change);
        }
        let progress: Vec<_> = self.progress.borrow().values().cloned().collect();
        for w in progress {
            w.refresh();
        }
        let scheduler = self.scheduler.borrow().clone();
        if let Some(w) = scheduler {
            w.refresh(change);
        }
        if matches!(change, Change::Settings | Change::Ext) {
            let settings = self.settings.borrow().clone();
            if let Some(w) = settings {
                w.store_changed(change);
            }
        }
        if matches!(change, Change::Queues | Change::Settings) {
            let adds: Vec<_> = self.adds.borrow().clone();
            for w in adds {
                w.lists_changed();
            }
            let grabber = self.grabber.borrow().clone();
            if let Some(w) = grabber {
                w.lists_changed();
            }
        }
    }

    /// Dock badge, menu-bar icon and idle sleep follow the totals.
    fn totals_changed(&self) {
        let (active, tray, speed) = {
            let st = self.store.borrow();
            let active = n(&st.totals, "active");
            let speed = (b(&st.settings, "traySpeed") && active > 0).then(|| fmt::speed(n(&st.totals, "speedBps")));
            (active, st.tray_state(), speed)
        };
        system::keep_awake(active > 0);
        let dock = unsafe { NSApplication::sharedApplication(kit::mtm()).dockTile() };
        let badge = (active > 0).then(|| kit::ns(&active.to_string()));
        unsafe { dock.setBadgeLabel(badge.as_deref()) };
        self.last_tray.set(tray);
        if let Some(item) = &*self.status.borrow() {
            item.update(tray, speed.as_deref());
        }
    }

    fn completed(self: &Rc<Self>, d: &Value) {
        match s(d, "whenFinished") {
            "openFile" => {
                system::open_download(d);
            }
            "openFolder" => system::reveal_download(d),
            "none" => {}
            _ => {
                if self.setting("notifyComplete", true) {
                    system::notify("Download complete", s(d, "fileName"));
                }
            }
        }
        if self.setting("showCompleteDialog", false) {
            self.open_progress(s(d, "id"));
        }
    }

    fn queue_finished(self: &Rc<Self>, q: &Value) {
        if self.setting("notifyQueueFinished", true) {
            system::notify("Queue finished", &format!("{} has no more files to download", s(q, "name")));
        }
        let action = s(q, "onFinish");
        if matches!(action, "sleep" | "shutdown" | "exit") {
            *self.power.borrow_mut() = Some(Power { action: action.into(), queue: s(q, "name").into(), deadline: Instant::now() + Duration::from_secs(60) });
            self.open_main();
            self.power_changed();
        }
    }

    fn power_changed(&self) {
        let main = self.main.borrow().clone();
        if let Some(w) = main {
            w.power_changed();
        }
    }

    pub fn cancel_power(&self) {
        *self.power.borrow_mut() = None;
        self.power_changed();
    }

    /// Once a second: the power countdown, the clipboard monitor and the
    /// menu-bar icon going back to normal after a finished download.
    fn tick(self: &Rc<Self>) {
        let power = self.power.borrow().clone();
        if let Some(p) = power {
            if Instant::now() >= p.deadline {
                *self.power.borrow_mut() = None;
                let done = match p.action.as_str() {
                    "exit" => {
                        self.quit();
                        Ok(())
                    }
                    "sleep" => system::sleep(),
                    _ => {
                        engine::shutdown();
                        system::shut_down()
                    }
                };
                if let Err(e) = done {
                    self.report(&e);
                }
            }
            self.power_changed();
        }
        if self.store.borrow().tray_state() != self.last_tray.get() {
            self.totals_changed();
        }
        let count = system::clipboard_count();
        if count != self.clipboard.replace(count) && self.setting("clipboardMonitor", false) {
            let found = system::clipboard_text().and_then(|text| {
                let st = self.store.borrow();
                odm_ui::settings::clipboard_candidate(&text, &st.settings, st.downloads())
            });
            if let Some(url) = found {
                self.open_add(json!({ "url": url }));
            }
        }
        let progress: Vec<_> = self.progress.borrow().values().cloned().collect();
        for w in progress {
            w.tick();
        }
    }

    // -- windows -----------------------------------------------------------

    pub fn main_window(&self) -> Option<Rc<MainWindow>> {
        self.main.borrow().clone()
    }

    pub fn open_main(self: &Rc<Self>) {
        let existing = self.main.borrow().clone();
        let w = existing.unwrap_or_else(|| {
            let w = MainWindow::new();
            *self.main.borrow_mut() = Some(w.clone());
            w
        });
        w.show();
    }

    /// Open an Add window (the engine's `show_add` payload, `{batch: true}`,
    /// `{url, source: "web"}` or `{}`).
    pub fn open_add(self: &Rc<Self>, params: Value) {
        let w = AddWindow::open(params);
        self.adds.borrow_mut().push(w);
    }

    pub fn add_closed(&self, window: *const AddWindow) {
        self.adds.borrow_mut().retain(|w| !std::ptr::eq(Rc::as_ptr(w), window));
    }

    pub fn open_progress(self: &Rc<Self>, id: &str) {
        if self.store.borrow().get(id).is_none() {
            return;
        }
        let existing = self.progress.borrow().get(id).cloned();
        let w = existing.unwrap_or_else(|| {
            let w = ProgressWindow::open(id);
            self.progress.borrow_mut().insert(id.to_string(), w.clone());
            w
        });
        w.show();
    }

    pub fn progress_closed(&self, id: &str) {
        self.progress.borrow_mut().remove(id);
    }

    pub fn open_scheduler(self: &Rc<Self>) {
        let existing = self.scheduler.borrow().clone();
        let w = existing.unwrap_or_else(|| {
            let w = SchedulerWindow::new();
            *self.scheduler.borrow_mut() = Some(w.clone());
            w
        });
        w.show();
    }

    /// The grabber, optionally with a list of links (`show_links` payload).
    pub fn open_grabber(self: &Rc<Self>, links: Option<Value>) {
        let existing = self.grabber.borrow().clone();
        let w = existing.unwrap_or_else(|| {
            let w = GrabberWindow::new();
            *self.grabber.borrow_mut() = Some(w.clone());
            w
        });
        w.show(links);
    }

    pub fn open_settings(self: &Rc<Self>) {
        let existing = self.settings.borrow().clone();
        let w = existing.unwrap_or_else(|| {
            let w = SettingsWindow::new();
            *self.settings.borrow_mut() = Some(w.clone());
            w
        });
        w.show();
    }

    /// The main window closed. Without "keep running in the menu bar" that
    /// ends the app.
    pub fn main_closed(self: &Rc<Self>) {
        if !self.quitting.get() && !self.setting("minimizeToTray", true) {
            self.quit();
        }
    }

    /// Links handed to the app (`odm://add?url=…`, http(s)): they come from
    /// web pages, so they only pre-fill the Add window.
    fn open_urls(self: &Rc<Self>, urls: Vec<String>) {
        for u in urls {
            match incoming::parse_url(&u) {
                Some(incoming::Incoming::Add(p)) => self.open_add(p),
                Some(incoming::Incoming::Show) => self.open_main(),
                None => {}
            }
        }
    }

    // -- telling the user --------------------------------------------------

    /// Show a message. Deferred, so it never runs a modal loop in the
    /// middle of an event being handled.
    pub fn report(&self, message: &str) {
        let message = message.to_string();
        kit::after(0.0, move || kit::tell(odm_ui::APP_NAME, &message));
    }

    // -- actions on downloads ----------------------------------------------

    pub fn resume(&self, ids: &[String]) {
        engine::act("resume", json!({ "ids": ids }));
    }

    pub fn pause(&self, ids: &[String]) {
        engine::act("pause", json!({ "ids": ids }));
    }

    pub fn pause_all(&self) {
        engine::act("pause_all", json!({}));
    }

    pub fn delete(&self, ids: &[String], files: bool) {
        engine::act("delete", json!({ "ids": ids, "deleteFiles": files }));
    }

    /// Delete after asking (unless the user turned the question off).
    pub fn ask_delete(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if !self.setting("confirmDelete", true) {
            return self.delete(ids, false);
        }
        let title = if ids.len() == 1 { "Delete this download?".to_string() } else { format!("Delete {} downloads?", ids.len()) };
        let text = "Unfinished parts are always removed. Finished files stay on disk unless you also delete files.";
        match kit::ask(None, &title, text, &["Remove from List", "Remove and Delete Files", "Cancel"], Some(1)) {
            0 => self.delete(ids, false),
            1 => self.delete(ids, true),
            _ => {}
        }
    }

    pub fn delete_completed(&self) {
        engine::act("delete_completed", json!({}));
    }

    pub fn start_queue(&self, id: &str) {
        engine::act("start_queue", json!({ "id": id }));
    }

    pub fn stop_queue(&self, id: &str) {
        engine::act("stop_queue", json!({ "id": id }));
    }

    pub fn move_to_queue(&self, ids: &[String], queue: Option<&str>) {
        engine::act("move_to_queue", json!({ "ids": ids, "queueId": queue }));
    }

    pub fn set_category(&self, ids: &[String], category: &str) {
        engine::act("set_category", json!({ "ids": ids, "category": category }));
    }

    /// Always put files with this extension in a category, and move the
    /// listed ones along.
    pub fn assign_extension(&self, ext: &str, category: &str) {
        engine::act("assign_extensions", json!({ "category": category, "extensions": [ext] }));
        let same = self.store.borrow().same_extension(ext, category);
        if !same.is_empty() {
            self.set_category(&same, category);
        }
    }

    pub fn set_global_limit(&self, bps: Option<u64>) {
        engine::act("set_global_limit", json!({ "bps": bps }));
    }

    /// Turn the global speed limit on (at the configured speed) or off.
    pub fn toggle_limit(&self) {
        let (on, bps) = {
            let st = self.store.borrow();
            (b(&st.settings, "globalLimitEnabled"), n(&st.settings, "globalLimitBps"))
        };
        self.set_global_limit((!on).then_some(bps));
    }

    /// "25 MB/s" or "No Limit".
    pub fn limit_label(&self) -> String {
        let st = self.store.borrow();
        if b(&st.settings, "globalLimitEnabled") {
            format!("{} MB/s", fmt::mbps(n(&st.settings, "globalLimitBps")))
        } else {
            "No Limit".into()
        }
    }

    pub fn open_file(&self, d: &Value) {
        if !system::open_download(d) {
            self.report("The file is no longer there. It may have been moved or deleted.");
        }
    }

    /// Save settings. When the engine refuses them (e.g. the new bridge
    /// port is taken) the error is shown and the engine's copy comes back.
    pub fn save_settings(self: &Rc<Self>, mut wanted: Value) {
        let was = b(&self.store.borrow().settings, "launchAtLogin");
        let want = b(&wanted, "launchAtLogin");
        if was != want {
            // keep the setting equal to what the system actually did
            let (now, message) = system::set_autostart(want);
            wanted["launchAtLogin"] = now.into();
            if let Some(m) = message {
                self.report(&m);
            }
        }
        self.store.borrow_mut().settings = wanted.clone();
        self.changed(Change::Settings);
        engine::spawn("save_settings", json!({ "settings": wanted }), |r| {
            if let Err(e) = r {
                let app = app();
                app.report(&e);
                // nothing was stored: go back to what the engine has
                if let Some(real) = engine::get("get_settings", json!({})) {
                    app.store.borrow_mut().settings = real;
                    app.changed(Change::Settings);
                }
            }
        });
    }

    fn apply_appearance(&self) {
        let name = match s(&self.store.borrow().settings, "theme") {
            "light" => Some(unsafe { NSAppearanceNameAqua }),
            "system" => None,
            _ => Some(unsafe { NSAppearanceNameDarkAqua }),
        };
        let appearance = name.and_then(|n| unsafe { NSAppearance::appearanceNamed(n) });
        unsafe { NSApplication::sharedApplication(kit::mtm()).setAppearance(appearance.as_deref()) };
    }

    // -- link lists --------------------------------------------------------

    pub fn import_list(self: &Rc<Self>) {
        let panel = unsafe { NSOpenPanel::openPanel(kit::mtm()) };
        unsafe {
            panel.setAllowsMultipleSelection(false);
            panel.setCanChooseDirectories(false);
            panel.setMessage(Some(&kit::ns("Choose a text file with one link per line.")));
        }
        if unsafe { panel.runModal() } != NSModalResponseOK {
            return;
        }
        let Some(path) = unsafe { panel.URL() }.and_then(|u| u.path()).map(|p| p.to_string()) else { return };
        engine::spawn("read_url_list", json!({ "path": path }), |r| {
            let app = app();
            match r {
                Ok(Value::Array(urls)) if !urls.is_empty() => {
                    let links: Vec<Value> = urls.iter().filter_map(Value::as_str).map(|u| json!({ "url": u })).collect();
                    app.open_grabber(Some(json!({ "links": links })));
                }
                Ok(_) => app.report("No http(s) links found in that file."),
                Err(e) => app.report(&e),
            }
        });
    }

    pub fn export_list(&self) {
        let panel = unsafe { NSSavePanel::savePanel(kit::mtm()) };
        unsafe { panel.setNameFieldStringValue(&kit::ns("downloads.txt")) };
        if unsafe { panel.runModal() } != NSModalResponseOK {
            return;
        }
        if let Some(path) = unsafe { panel.URL() }.and_then(|u| u.path()).map(|p| p.to_string()) {
            engine::act("export_list", json!({ "path": path, "onlyUnfinished": false }));
        }
    }

    // -- quitting ----------------------------------------------------------

    pub fn quit(&self) {
        if !self.quitting.replace(true) {
            unsafe { NSApplication::sharedApplication(kit::mtm()).terminate(None) };
        }
    }

    /// Stops downloads and saves state (waits up to 5 s).
    fn will_terminate(&self) {
        self.quitting.set(true);
        engine::shutdown();
        system::keep_awake(false);
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        /// Before links handed to the app at launch arrive.
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _n: &NSNotification) {
            App::start();
        }

        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            let app = app();
            app.finish_launching();
            app.open_main();
            #[cfg(debug_assertions)]
            {
                debug_open(&app);
                debug_shots();
                debug_script();
            }
        }

        /// Keep downloading in the background when the last window closes
        /// (the main window decides whether closing it quits).
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            false
        }

        /// A click on the Dock icon with no window showing.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, visible: bool) -> bool {
            if !visible {
                app().open_main();
            }
            true
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            if let Some(app) = try_app() {
                app.will_terminate();
            }
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let list: Vec<String> = urls.iter().filter_map(|u| unsafe { u.absoluteString() }).map(|s| s.to_string()).collect();
            if let Some(app) = try_app() {
                app.open_urls(list);
            }
        }
    }
);

/// Debug builds: `ODM_OPEN=add,batch,web,grabber,scheduler,settings,
/// progress` opens these windows at startup.
#[cfg(debug_assertions)]
fn debug_open(app: &Rc<App>) {
    let Ok(list) = std::env::var("ODM_OPEN") else { return };
    for w in list.split(',') {
        match w.trim() {
            "add" => app.open_add(json!({ "url": "http://127.0.0.1:8919/song.mp3" })),
            "batch" => app.open_add(json!({ "batch": true })),
            "web" => app.open_add(json!({ "url": "http://127.0.0.1:8919/song.mp3", "source": "web" })),
            "grabber" => app.open_grabber(None),
            "scheduler" => app.open_scheduler(),
            "settings" => app.open_settings(),
            // once a download started for the test (ODM_E2E_URL) is listed
            "progress" => kit::after(3.0, || {
                let app = crate::app::app();
                let id = app.store.borrow().downloads().first().map(|d| s(d, "id").to_string());
                if let Some(id) = id {
                    app.open_progress(&id);
                }
            }),
            _ => {}
        }
    }
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let nsapp = NSApplication::sharedApplication(mtm);
    nsapp.setActivationPolicy(if kit::quiet() { NSApplicationActivationPolicy::Accessory } else { NSApplicationActivationPolicy::Regular });
    let delegate: Retained<AppDelegate> = unsafe { msg_send![AppDelegate::alloc(mtm), init] };
    nsapp.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    nsapp.run();
}

/// Debug builds: `ODM_SHOTS=<folder>` saves every visible window of the
/// app there as a PNG every two seconds, so the interface can be checked
/// without capturing the screen.
#[cfg(debug_assertions)]
fn debug_shots() {
    let Some(dir) = std::env::var_os("ODM_SHOTS").map(std::path::PathBuf::from) else { return };
    kit::every(2.0, move || {
        let nsapp = NSApplication::sharedApplication(kit::mtm());
        for (i, w) in unsafe { nsapp.windows() }.iter().enumerate() {
            let Some(frame) = (unsafe { w.contentView() }).and_then(|c| unsafe { c.superview() }) else { continue };
            if !unsafe { w.isVisible() } || unsafe { w.level() } != 0 {
                continue;
            }
            let bounds = frame.bounds();
            let Some(rep) = (unsafe { frame.bitmapImageRepForCachingDisplayInRect(bounds) }) else { continue };
            unsafe { frame.cacheDisplayInRect_toBitmapImageRep(bounds, &rep) };
            let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) };
            let title: String = unsafe { w.title() }.to_string().chars().filter(|c| c.is_alphanumeric()).take(24).collect();
            if let Some(png) = png {
                let _ = std::fs::write(dir.join(format!("{i}-{title}.png")), png.to_vec());
            }
            // for `screencapture -l <number>`, which shows what a snapshot
            // can't (translucent sidebars, toolbars)
            let _ = std::fs::write(dir.join(format!("{title}.window")), unsafe { w.windowNumber() }.to_string());
        }
    });
}

/// Debug builds: `ODM_DO=step,step,…` runs these steps 1.5 s apart, to
/// exercise the app without a pointer: `add`, `start`, `select`, `pause`,
/// `resume`, `delete`, `menus`, `settings`, `queue`, `state`, `quit`.
#[cfg(debug_assertions)]
fn debug_script() {
    let Ok(script) = std::env::var("ODM_DO") else { return };
    for (i, step) in script.split(',').map(|s| s.trim().to_string()).enumerate() {
        kit::after(2.0 + 1.5 * i as f64, move || {
            eprintln!("do: {step}");
            app().debug_step(&step);
        });
    }
}

#[cfg(debug_assertions)]
impl App {
    fn debug_step(self: &Rc<Self>, step: &str) {
        let ids = || self.main_window().map(|m| m.selected_ids()).unwrap_or_default();
        match step {
            "add" => self.open_add(json!({ "url": "http://127.0.0.1:8919/song.mp3" })),
            "start" => {
                let last = self.adds.borrow().last().cloned();
                if let Some(w) = last {
                    w.debug_start();
                }
            }
            "select" => {
                if let Some(m) = self.main_window() {
                    m.debug_select();
                }
            }
            "pause" => self.pause(&ids()),
            "resume" => self.resume(&ids()),
            "delete" => self.delete(&ids(), true),
            "menus" => {
                if let Some(m) = self.main_window() {
                    m.debug_menus();
                }
                menus::debug_menus();
            }
            "settings" => {
                self.open_settings();
                let w = self.settings.borrow().clone();
                if let Some(w) = w {
                    w.debug_edit();
                }
            }
            "queue" => {
                self.open_scheduler();
                let w = self.scheduler.borrow().clone();
                if let Some(w) = w {
                    w.debug_edit();
                }
            }
            "state" => {
                let st = self.store.borrow();
                let list: Vec<String> = st.downloads().iter().map(|d| format!("{} {} {}/{}", s(d, "fileName"), s(d, "status"), n(d, "receivedBytes"), n(d, "totalBytes"))).collect();
                let queues: Vec<String> = st.queues.iter().map(|q| format!("{} start={} at {}", s(q, "name"), b(q, "startEnabled"), s(q, "startTime"))).collect();
                eprintln!("state: downloads {list:?}; queues {queues:?}; traySpeed {}; theme {}; tray {}", b(&st.settings, "traySpeed"), s(&st.settings, "theme"), st.tray_state());
            }
            "quit" => self.quit(),
            _ => eprintln!("unknown step"),
        }
    }
}
