//! One download as a GObject, so list cells can bind to its properties and
//! update in place while it progresses.
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;
use gtk::gio;
use odm_linux::fmt;
use serde_json::Value;
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Row)]
    pub struct Row {
        #[property(get, set)]
        pub id: RefCell<String>,
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub size: RefCell<String>,
        #[property(get, set)]
        pub fraction: Cell<f64>,
        #[property(get, set)]
        pub percent: RefCell<String>,
        #[property(get, set)]
        pub speed: RefCell<String>,
        #[property(get, set)]
        pub eta: RefCell<String>,
        /// label, e.g. "Downloading" or "Starts 01:00"
        #[property(get, set)]
        pub status: RefCell<String>,
        /// the engine's status value, e.g. "downloading"
        #[property(get, set)]
        pub state: RefCell<String>,
        #[property(get, set)]
        pub queue: RefCell<String>,
        #[property(get, set)]
        pub added: RefCell<String>,
        #[property(get, set, nullable)]
        pub icon: RefCell<Option<gio::Icon>>,
        /// bumped on every change, for views that redraw from `data`
        #[property(get, set)]
        pub version: Cell<u32>,
        /// the download as the engine reports it
        pub data: RefCell<Value>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "OdmRow";
        type Type = super::Row;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Row {}
}

glib::wrapper! {
    pub struct Row(ObjectSubclass<imp::Row>);
}

pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn n(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn opt_n(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The icon the desktop uses for this kind of file.
pub fn file_icon(name: &str) -> gio::Icon {
    let (ty, _) = gio::content_type_guess(Some(name), None::<&[u8]>);
    gio::content_type_get_icon(&ty)
}

impl Row {
    pub fn new(d: Value, queue_name: &str) -> Self {
        let row: Row = glib::Object::new();
        row.set_id(s(&d, "id"));
        row.apply(d, queue_name);
        row
    }

    pub fn data(&self) -> Value {
        self.imp().data.borrow().clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&Value) -> R) -> R {
        f(&self.imp().data.borrow())
    }

    pub fn is_active(&self) -> bool {
        fmt::is_active(&self.state())
    }

    /// Replace the whole record (keeping the speed history collected here
    /// when the new record has none).
    pub fn apply(&self, mut d: Value, queue_name: &str) {
        let had_history = d.get("speedHistory").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
        if !had_history {
            if let Some(h) = self.imp().data.borrow().get("speedHistory").cloned() {
                d["speedHistory"] = h;
            }
        }
        *self.imp().data.borrow_mut() = d;
        self.refresh(queue_name);
    }

    /// Merge a progress item into the record.
    pub fn progress(&self, p: &Value) -> bool {
        let status_changed = {
            let mut d = self.imp().data.borrow_mut();
            let changed = d.get("status") != p.get("status");
            for k in ["receivedBytes", "speedBps", "etaSecs", "activeConnections", "segments", "status"] {
                if let Some(v) = p.get(k) {
                    d[k] = v.clone();
                }
            }
            if let Some(t) = p.get("totalBytes").filter(|t| !t.is_null()) {
                d["totalBytes"] = t.clone();
            }
            changed
        };
        let q = self.queue();
        self.refresh(&q);
        status_changed
    }

    /// One more sample for the 60-second speed graph.
    pub fn sample_speed(&self) {
        {
            let mut d = self.imp().data.borrow_mut();
            let speed = n(&d, "speedBps");
            if !d.get("speedHistory").map(Value::is_array).unwrap_or(false) {
                d["speedHistory"] = Value::Array(vec![]);
            }
            if let Some(h) = d["speedHistory"].as_array_mut() {
                h.push(speed.into());
                if h.len() > 60 {
                    let extra = h.len() - 60;
                    h.drain(..extra);
                }
            }
        }
        self.set_version(self.version().wrapping_add(1));
    }

    fn put(&self, prop: &str, current: String, new: String) {
        if current != new {
            self.set_property(prop, new);
        }
    }

    fn refresh(&self, queue_name: &str) {
        let d = self.imp().data.borrow().clone();
        let status = s(&d, "status");
        let complete = status == "complete";
        let active = fmt::is_active(status);
        let pct = fmt::percent(n(&d, "receivedBytes"), opt_n(&d, "totalBytes"), complete);
        if self.name() != s(&d, "fileName") {
            self.set_name(s(&d, "fileName"));
            self.set_icon(Some(file_icon(s(&d, "fileName"))));
        }
        self.put("size", self.size(), fmt::bytes(opt_n(&d, "totalBytes")));
        if (self.fraction() - pct as f64 / 100.0).abs() > f64::EPSILON {
            self.set_fraction(pct as f64 / 100.0);
        }
        self.put("percent", self.percent(), format!("{pct}%"));
        self.put("speed", self.speed(), if active { fmt::speed(n(&d, "speedBps")) } else { fmt::DASH.into() });
        self.put(
            "eta",
            self.eta(),
            if status == "downloading" { fmt::eta(opt_n(&d, "etaSecs")) } else { fmt::DASH.into() },
        );
        self.put("status", self.status(), fmt::status_label(status, d.get("scheduledAt").and_then(Value::as_str)));
        self.put("state", self.state(), status.to_string());
        self.put("queue", self.queue(), queue_name.to_string());
        self.put("added", self.added(), fmt::added(s(&d, "addedAt")));
        self.set_version(self.version().wrapping_add(1));
    }
}
