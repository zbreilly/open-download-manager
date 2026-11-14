//! The parts of the app that are the same in both Linux front ends: one
//! instance per session, the tray icon, notifications, keep-awake and what
//! happens when a download or a queue finishes. A front end creates one
//! [`Shell`], passes it every engine event, and reacts to [`ShellEvent`]s.
use crate::incoming::{self, Incoming};
use crate::tray::{Tray, TrayAction, TrayState};
use crate::{awake::Awake, fmt, instance, notify, open, power};
use odm_ffi::api;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the tray shows "complete" after a download finishes.
const COMPLETE_FLASH: Duration = Duration::from_secs(5);

/// Something the front end has to do. Delivered on background threads:
/// marshal to the GUI thread before touching widgets.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEvent {
    /// bring the main window up
    Show,
    /// open the Add window with these params (`show_add` payload, or
    /// `{url, source: "web"}` for a link handed to the app)
    Add(Value),
    /// open the grabber with a list of links (`show_links` payload)
    Links(Value),
    Grabber,
    Scheduler,
    Settings,
    /// open the progress window of this download
    Progress(String),
    /// a queue finished with a power action ("sleep", "shutdown", "exit"):
    /// show the 60 s countdown, then call [`Shell::run_power`] unless the
    /// user cancels
    PowerCountdown { action: String, queue: String },
    /// tell the user
    Error(String),
    Quit,
}

type Sink = Arc<dyn Fn(ShellEvent) + Send + Sync>;

#[derive(Default)]
struct State {
    settings: Option<Value>,
    totals: Value,
    last_complete: Option<Instant>,
    /// links from this launch's command line, until the UI is ready
    pending: Vec<Incoming>,
}

struct Inner {
    sink: Sink,
    tray: Option<Tray>,
    awake: Awake,
    state: Mutex<State>,
    _instance: instance::Instance,
}

#[derive(Clone)]
pub struct Shell {
    inner: Arc<Inner>,
    /// started with `--minimized` (login item): stay in the tray
    pub minimized: bool,
}

fn deliver(sink: &Sink, items: Vec<Incoming>) {
    if items.is_empty() {
        sink(ShellEvent::Show);
    }
    for i in items {
        sink(match i {
            Incoming::Add(v) => ShellEvent::Add(v),
            Incoming::Show => ShellEvent::Show,
        });
    }
}

fn flag(v: &Option<Value>, key: &str, default: bool) -> bool {
    v.as_ref().and_then(|s| s.get(key)).and_then(Value::as_bool).unwrap_or(default)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Where a download's file is (or will be).
pub fn download_path(d: &Value) -> PathBuf {
    Path::new(text(d, "saveDir")).join(text(d, "fileName"))
}

/// Show a download in the file manager: the finished file, else its part
/// file, else the folder.
pub fn reveal_download(d: &Value) -> Result<(), String> {
    let full = download_path(d);
    let part = PathBuf::from(format!("{}.odmpart", full.display()));
    if full.exists() {
        open::reveal(&full)
    } else if part.exists() {
        open::reveal(&part)
    } else {
        open::reveal(Path::new(text(d, "saveDir")))
    }
}

/// Turn the global speed limit on (at the configured speed) or off.
pub fn toggle_limiter() -> Result<(), String> {
    let s = api::call("get_settings", json!({}))?;
    let on = s.get("globalLimitEnabled").and_then(Value::as_bool).unwrap_or(false);
    let bps = if on { Value::Null } else { s.get("globalLimitBps").cloned().unwrap_or(Value::Null) };
    api::call("set_global_limit", json!({ "bps": bps })).map(|_| ())
}

fn tray_action(sink: &Sink, a: TrayAction) {
    let done = match a {
        TrayAction::Show => return sink(ShellEvent::Show),
        TrayAction::AddUrl => return sink(ShellEvent::Add(json!({}))),
        TrayAction::Grabber => return sink(ShellEvent::Grabber),
        TrayAction::Scheduler => return sink(ShellEvent::Scheduler),
        TrayAction::Settings => return sink(ShellEvent::Settings),
        TrayAction::Quit => return sink(ShellEvent::Quit),
        TrayAction::StartMainQueue => api::call("start_queue", json!({ "id": "main" })).map(|_| ()),
        TrayAction::PauseAll => api::call("pause_all", json!({})).map(|_| ()),
        TrayAction::ToggleLimiter => toggle_limiter(),
    };
    if let Err(e) = done {
        sink(ShellEvent::Error(e));
    }
}

impl Shell {
    /// Become the app's one instance. `args` is argv without the program
    /// name. Returns `Ok(None)` when another instance was running and got
    /// the arguments: exit then. Links in `args` are delivered through
    /// `sink` when [`Shell::ready`] is called.
    pub fn start(args: Vec<String>, sink: impl Fn(ShellEvent) + Send + Sync + 'static) -> Result<Option<Shell>, String> {
        let sink: Sink = Arc::new(sink);
        let later = sink.clone();
        let acquired = instance::acquire(args.clone(), move |argv| deliver(&later, incoming::parse(&argv).items))?;
        let instance::Acquired::Primary(instance) = acquired else { return Ok(None) };
        let parsed = incoming::parse(&args);
        let for_tray = sink.clone();
        let tray = Tray::spawn(move |a| tray_action(&for_tray, a));
        let state = State { pending: parsed.items, ..Default::default() };
        let inner = Inner { sink, tray, awake: Awake::new(), state: Mutex::new(state), _instance: instance };
        Ok(Some(Shell { inner: Arc::new(inner), minimized: parsed.minimized }))
    }

    /// The UI can open windows now: deliver the links this launch was
    /// started with.
    pub fn ready(&self) {
        let items = std::mem::take(&mut self.inner.state.lock().pending);
        if !items.is_empty() {
            deliver(&self.inner.sink, items);
        }
    }

    /// A tray icon is showing, so closing the main window may hide the app
    /// instead of quitting it.
    pub fn has_tray(&self) -> bool {
        self.inner.tray.is_some()
    }

    /// Feed every engine event here (from the events thread).
    pub fn engine_event(&self, name: &str, payload: &str) {
        let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
        match name {
            "totals" => {
                self.inner.state.lock().totals = v;
                self.refresh();
            }
            "settings_changed" => {
                self.inner.state.lock().settings = Some(v);
                self.refresh();
            }
            "download_completed" => self.completed(&v),
            "download_failed" => {
                if flag(&self.settings(), "notifyFailure", true) {
                    let body = v.get("error").and_then(Value::as_str).unwrap_or("Unknown error");
                    let id = text(&v, "id").to_string();
                    let (sink, target) = (self.inner.sink.clone(), id.clone());
                    notify::show(&id, &format!("Download failed: {}", text(&v, "fileName")), body, &[], move |_| {
                        sink(ShellEvent::Progress(target.clone()))
                    });
                }
            }
            "queue_finished" => self.queue_finished(&v),
            "show_add" => (self.inner.sink)(ShellEvent::Add(v)),
            "show_links" => (self.inner.sink)(ShellEvent::Links(v)),
            _ => {}
        }
    }

    fn settings(&self) -> Option<Value> {
        let cached = self.inner.state.lock().settings.clone();
        cached.or_else(|| {
            let s = api::call("get_settings", json!({})).ok()?;
            self.inner.state.lock().settings = Some(s.clone());
            Some(s)
        })
    }

    fn completed(&self, d: &Value) {
        let settings = self.settings();
        self.inner.state.lock().last_complete = Some(Instant::now());
        self.refresh();
        // the tray goes back to its normal icon after the flash
        let again = self.clone();
        let _ = std::thread::Builder::new().name("odm-tray-flash".into()).spawn(move || {
            std::thread::sleep(COMPLETE_FLASH + Duration::from_millis(200));
            again.refresh();
        });
        let id = text(d, "id").to_string();
        let path = download_path(d);
        match text(d, "whenFinished") {
            "openFile" => {
                let _ = open::open_path(&path);
            }
            "openFolder" => {
                let _ = reveal_download(d);
            }
            "none" => {}
            _ => {
                if flag(&settings, "notifyComplete", true) {
                    let owned = d.clone();
                    notify::show(
                        &id,
                        "Download complete",
                        text(d, "fileName"),
                        &[("folder", "Show in Folder")],
                        move |action| {
                            let _ = if action == "folder" {
                                reveal_download(&owned)
                            } else {
                                open::open_path(&download_path(&owned))
                            };
                        },
                    );
                }
            }
        }
        if flag(&settings, "showCompleteDialog", false) {
            (self.inner.sink)(ShellEvent::Progress(id));
        }
    }

    fn queue_finished(&self, q: &Value) {
        let name = text(q, "name").to_string();
        if flag(&self.settings(), "notifyQueueFinished", true) {
            let sink = self.inner.sink.clone();
            notify::show(
                &format!("q-{}", text(q, "id")),
                "Queue finished",
                &format!("{name} has no more files to download"),
                &[],
                move |_| sink(ShellEvent::Show),
            );
        }
        let action = text(q, "onFinish");
        if matches!(action, "sleep" | "shutdown" | "exit") {
            (self.inner.sink)(ShellEvent::PowerCountdown { action: action.into(), queue: name });
        }
    }

    /// Bring the tray icon and the sleep inhibitor in line with the totals.
    fn refresh(&self) {
        let (totals, settings, recent) = {
            let st = self.inner.state.lock();
            (st.totals.clone(), st.settings.clone(), st.last_complete.map(|t| t.elapsed() < COMPLETE_FLASH).unwrap_or(false))
        };
        let active = count(&totals, "active");
        self.inner.awake.set(active > 0);
        let Some(tray) = &self.inner.tray else { return };
        let state = if count(&totals, "failed") > 0 && active == 0 && !recent {
            TrayState::Error
        } else if active > 0 {
            TrayState::Downloading
        } else if recent {
            TrayState::Complete
        } else if count(&totals, "paused") > 0 {
            TrayState::Paused
        } else {
            TrayState::Idle
        };
        let line = if active > 0 {
            format!("{active} active · {}", fmt::speed(count(&totals, "speedBps")))
        } else {
            "No active downloads".to_string()
        };
        tray.set_state(state, &line);
        tray.set_status_line(&line);
        tray.set_limiter(flag(&settings, "globalLimitEnabled", false));
    }

    /// Run a queue's power action when its countdown ran out. "exit" is the
    /// front end's own quit; the engine is shut down first for "shutdown".
    pub fn run_power(&self, action: &str) -> Result<(), String> {
        match power::PowerAction::from_setting(action) {
            Some(power::PowerAction::Shutdown) => {
                api::shutdown();
                power::run(power::PowerAction::Shutdown)
            }
            Some(a) => power::run(a),
            None => Ok(()),
        }
    }
}

pub use odm_ui::settings::{clipboard_candidate, merge as merge_settings};
