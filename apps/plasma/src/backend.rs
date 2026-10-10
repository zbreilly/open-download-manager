//! The application object QML sees: it starts the engine, keeps the store
//! up to date from engine events, and passes on what the desktop asks for
//! (tray, second launches, links, finished queues).
use crate::state::{with_store, with_store_mut};
use core::pin::Pin;
use cxx_qt::Threading;
use cxx_qt_lib::QString;
use odm_ffi::api;
use odm_linux::shell::{Shell, ShellEvent};
use odm_ui::store::Change;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("odm-plasma/cpp/helpers.h");
        #[namespace = "odm"]
        #[rust_name = "keep_running_without_windows"]
        fn keepRunningWithoutWindows();
        #[namespace = "odm"]
        #[rust_name = "use_desktop_style"]
        fn useDesktopStyle();
        #[namespace = "odm"]
        #[rust_name = "set_window_icon"]
        fn setWindowIcon(name: &QString);
        #[namespace = "odm"]
        #[rust_name = "copy_to_clipboard"]
        fn copyToClipboard(text: &QString);
        #[namespace = "odm"]
        #[rust_name = "clipboard_text"]
        fn clipboardText() -> QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Backend = super::BackendRust;

        /// The store changed: "list", "progress", "queues", "settings",
        /// "totals" or "ext". Models and view-models refresh on it.
        #[qsignal]
        fn changed(self: Pin<&mut Backend>, kind: QString);

        /// Something to do for the windows: "show", "add", "links",
        /// "grabber", "scheduler", "settings", "progress", "error", "quit",
        /// with its parameters as JSON.
        #[qsignal]
        #[cxx_name = "shellEvent"]
        fn shell_event(self: Pin<&mut Backend>, kind: QString, json: QString);

        /// Start the engine and load the store.
        #[qinvokable]
        fn start(self: Pin<&mut Backend>);

        /// The windows can open now: deliver the links this launch got.
        #[qinvokable]
        fn ready(self: &Backend);

        /// Stop downloads and save state before quitting (up to 5 s).
        #[qinvokable]
        fn shutdown(self: &Backend);

        /// Closing the main window hides the app (there is a tray icon and
        /// the user wants that) instead of quitting it.
        #[qinvokable]
        #[cxx_name = "hidesOnClose"]
        fn hides_on_close(self: &Backend) -> bool;

        /// Started as a login item with a tray to sit in: no window.
        #[qinvokable]
        #[cxx_name = "startsHidden"]
        fn starts_hidden(self: &Backend) -> bool;

        #[qinvokable]
        fn copy(self: &Backend, text: &QString);
    }

    impl cxx_qt::Threading for Backend {}
}

#[derive(Default)]
pub struct BackendRust;

static SHELL: OnceLock<Shell> = OnceLock::new();
static QT: OnceLock<cxx_qt::CxxQtThread<ffi::Backend>> = OnceLock::new();
/// shell events that arrived before the QML side was up
static PENDING: Mutex<Vec<ShellEvent>> = Mutex::new(Vec::new());

thread_local! {
    /// a queue's countdown: (action, queue name, when it runs out)
    static POWER: RefCell<Option<(String, String, Instant)>> = const { RefCell::new(None) };
    static START_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
    static LAST_CLIPBOARD: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn set_shell(shell: Shell) {
    let _ = SHELL.set(shell);
}

fn encode(ev: ShellEvent) -> (&'static str, String) {
    match ev {
        ShellEvent::Show => ("show", String::new()),
        ShellEvent::Add(v) => ("add", v.to_string()),
        ShellEvent::Links(v) => ("links", v.to_string()),
        ShellEvent::Grabber => ("grabber", String::new()),
        ShellEvent::Scheduler => ("scheduler", String::new()),
        ShellEvent::Settings => ("settings", String::new()),
        ShellEvent::Progress(id) => ("progress", json!(id).to_string()),
        ShellEvent::Error(e) => ("error", json!(e).to_string()),
        ShellEvent::Quit => ("quit", String::new()),
        // handled in `deliver`
        ShellEvent::PowerCountdown { .. } => ("show", String::new()),
    }
}

fn deliver(ev: ShellEvent) {
    let Some(qt) = QT.get() else {
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
        return;
    };
    let _ = qt.queue(move |mut backend| {
        if let ShellEvent::PowerCountdown { action, queue } = &ev {
            POWER.with(|p| *p.borrow_mut() = Some((action.clone(), queue.clone(), Instant::now() + Duration::from_secs(60))));
            backend.as_mut().changed(QString::from("totals"));
        }
        let (kind, payload) = encode(ev);
        backend.shell_event(QString::from(kind), QString::from(&payload));
    });
}

/// The shell's sink (called on background threads).
pub fn shell_event(ev: ShellEvent) {
    deliver(ev);
}

/// Tell the user something went wrong (from any thread).
pub fn report(message: String) {
    deliver(ShellEvent::Error(message));
}

/// The running countdown: (action, queue name, seconds left).
pub fn power() -> Option<(String, String, u64)> {
    POWER.with(|p| p.borrow().as_ref().map(|(a, q, at)| (a.clone(), q.clone(), at.saturating_duration_since(Instant::now()).as_secs())))
}

pub fn cancel_power() {
    POWER.with(|p| *p.borrow_mut() = None);
}

/// The action of a countdown that just ran out (once).
pub fn power_due() -> Option<String> {
    POWER.with(|p| {
        let due = p.borrow().as_ref().map(|(_, _, at)| Instant::now() >= *at).unwrap_or(false);
        if due {
            p.borrow_mut().take().map(|(action, _, _)| action)
        } else {
            None
        }
    })
}

/// Sleep or shut down (the engine is stopped first for a shutdown).
pub fn run_power(action: &str) -> Result<(), String> {
    match SHELL.get() {
        Some(shell) => shell.run_power(action),
        None => Ok(()),
    }
}

pub fn start_error() -> String {
    START_ERROR.with(|e| e.borrow().clone())
}

fn kind(change: Change) -> Option<&'static str> {
    Some(match change {
        Change::List => "list",
        Change::Progress => "progress",
        Change::Queues => "queues",
        Change::Settings => "settings",
        Change::Totals => "totals",
        Change::Ext => "ext",
        Change::None => return None,
    })
}

/// Offer a copied download link in the Add window (when turned on).
fn check_clipboard() {
    let text = ffi::clipboard_text().to_string();
    let changed = LAST_CLIPBOARD.with(|last| {
        let changed = *last.borrow() != text;
        if changed {
            *last.borrow_mut() = text.clone();
        }
        changed
    });
    if !changed {
        return;
    }
    if let Some(url) = with_store(|st| odm_ui::settings::clipboard_candidate(&text, &st.settings, st.downloads())) {
        deliver(ShellEvent::Add(json!({ "url": url })));
    }
}

impl ffi::Backend {
    fn start(mut self: Pin<&mut Self>) {
        let _ = QT.set(self.qt_thread());
        let started = api::start(&odm_linux::paths::data_dir(), |name, payload| {
            if let Some(shell) = SHELL.get() {
                shell.engine_event(name, payload);
            }
            let Some(qt) = QT.get() else { return };
            let (name, payload) = (name.to_string(), payload.to_string());
            let _ = qt.queue(move |backend| {
                let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
                // what a finished download or queue calls for is the shell's
                // job (notifications, opening files, the countdown)
                let (change, _effect) = with_store_mut(|st| st.handle(&name, v));
                if change == Change::Totals {
                    check_clipboard();
                }
                if let Some(kind) = kind(change) {
                    backend.changed(QString::from(kind));
                }
            });
        });
        match started {
            Ok(()) => {
                with_store_mut(|st| st.load(|m| api::call(m, json!({})).ok()));
                // the autostart file can be removed outside the app: follow it
                let real = odm_linux::autostart::is_enabled();
                let stale = with_store(|st| st.settings.is_object() && odm_ui::b(&st.settings, "launchAtLogin") != real);
                if stale {
                    let settings = with_store_mut(|st| {
                        st.settings["launchAtLogin"] = real.into();
                        st.settings.clone()
                    });
                    crate::vms::act("save_settings", json!({ "settings": settings }));
                }
            }
            Err(e) => START_ERROR.with(|s| *s.borrow_mut() = e),
        }
        LAST_CLIPBOARD.with(|last| *last.borrow_mut() = ffi::clipboard_text().to_string());
        self.as_mut().changed(QString::from("list"));
    }

    fn ready(&self) {
        let waiting = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
        for ev in waiting {
            deliver(ev);
        }
        if let Some(shell) = SHELL.get() {
            shell.ready();
        }
    }

    fn shutdown(&self) {
        api::shutdown();
    }

    fn hides_on_close(&self) -> bool {
        let has_tray = SHELL.get().map(Shell::has_tray).unwrap_or(false);
        has_tray && with_store(|st| st.setting("minimizeToTray", true))
    }

    fn starts_hidden(&self) -> bool {
        SHELL.get().map(|s| s.minimized && s.has_tray()).unwrap_or(false)
    }

    fn copy(&self, text: &QString) {
        ffi::copy_to_clipboard(text);
    }
}
