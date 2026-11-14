//! View-models: the logic behind each window, in Rust. A window's QML
//! creates a `ViewModel`, tells it which window it is (`init`), draws
//! whatever `state` (JSON) describes and reports what the user does with
//! `send`. Nothing is decided in QML.
use crate::state::with_store;
use crate::vms;
use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use odm_ui::store::Store;
use serde_json::Value;
use std::time::Duration;

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, state)]
        type ViewModel = super::ViewModelRust;

        /// Something the window itself has to do: "close", or open another
        /// window ("add", "progress", "grabber", "scheduler", "settings",
        /// "main"), with its parameters as JSON.
        #[qsignal]
        fn request(self: Pin<&mut ViewModel>, what: QString, payload: QString);

        /// Become the view-model of window `kind` ("main", "add",
        /// "progress", "scheduler", "grabber", "settings").
        #[qinvokable]
        fn init(self: Pin<&mut ViewModel>, kind: &QString, params: &QString);

        /// The user did something: `action` with a JSON `payload`.
        #[qinvokable]
        fn send(self: Pin<&mut ViewModel>, action: &QString, payload: &QString);

        /// The store changed (`Backend.changed`'s kind).
        #[qinvokable]
        #[cxx_name = "storeChanged"]
        fn store_changed(self: Pin<&mut ViewModel>, kind: &QString);
    }

    impl cxx_qt::Threading for ViewModel {}
}

/// What a view-model asks for while handling an action.
#[derive(Default)]
pub struct Cx {
    /// for the window: (what, payload)
    pub requests: Vec<(String, Value)>,
    /// work for another thread; its result comes back as an action
    pub jobs: Vec<Box<dyn FnOnce() -> (String, Value) + Send>>,
    /// actions to send to itself after a delay
    pub timers: Vec<(Duration, String, Value)>,
}

impl Cx {
    pub fn request(&mut self, what: &str, payload: Value) {
        self.requests.push((what.into(), payload));
    }

    pub fn close(&mut self) {
        self.request("close", Value::Null);
    }

    /// Run `work` off the GUI thread; what it returns is sent back.
    pub fn spawn(&mut self, work: impl FnOnce() -> (String, Value) + Send + 'static) {
        self.jobs.push(Box::new(work));
    }

    pub fn later(&mut self, after: Duration, action: &str, payload: Value) {
        self.timers.push((after, action.into(), payload));
    }
}

/// The logic of one window.
pub trait Vm {
    /// Everything the window shows.
    fn state(&self, store: &Store) -> Value;
    fn send(&mut self, action: &str, payload: &Value, store: &Store, cx: &mut Cx);
    /// The store changed (`kind`: "list", "progress", "queues", "settings",
    /// "totals", "ext").
    fn store_changed(&mut self, _kind: &str, _store: &Store, _cx: &mut Cx) {}
}

#[derive(Default)]
pub struct ViewModelRust {
    state: QString,
    vm: Option<Box<dyn Vm>>,
}

impl ffi::ViewModel {
    fn init(mut self: Pin<&mut Self>, kind: &QString, params: &QString) {
        let params: Value = serde_json::from_str(&params.to_string()).unwrap_or(Value::Null);
        let mut cx = Cx::default();
        let vm = with_store(|st| vms::create(&kind.to_string(), &params, st, &mut cx));
        self.as_mut().rust_mut().vm = vm;
        self.finish(cx);
    }

    fn send(mut self: Pin<&mut Self>, action: &QString, payload: &QString) {
        let payload: Value = serde_json::from_str(&payload.to_string()).unwrap_or(Value::Null);
        self.as_mut().dispatch(&action.to_string(), &payload);
    }

    fn dispatch(mut self: Pin<&mut Self>, action: &str, payload: &Value) {
        let mut cx = Cx::default();
        if let Some(vm) = self.as_mut().rust_mut().vm.as_mut() {
            with_store(|st| vm.send(action, payload, st, &mut cx));
        }
        self.finish(cx);
    }

    fn store_changed(mut self: Pin<&mut Self>, kind: &QString) {
        let mut cx = Cx::default();
        if let Some(vm) = self.as_mut().rust_mut().vm.as_mut() {
            with_store(|st| vm.store_changed(&kind.to_string(), st, &mut cx));
        }
        self.finish(cx);
    }

    /// Publish the new state and carry out what the view-model asked for.
    fn finish(mut self: Pin<&mut Self>, cx: Cx) {
        let state = self.rust().vm.as_ref().map(|vm| with_store(|st| vm.state(st)).to_string()).unwrap_or_default();
        if self.state().to_string() != state {
            self.as_mut().set_state(QString::from(&state));
        }
        for job in cx.jobs {
            let qt = self.qt_thread();
            let _ = std::thread::Builder::new().name("odm-vm".into()).spawn(move || {
                let (action, payload) = job();
                let _ = qt.queue(move |vm| vm.dispatch(&action, &payload));
            });
        }
        for (after, action, payload) in cx.timers {
            let qt = self.qt_thread();
            let _ = std::thread::Builder::new().name("odm-vm-timer".into()).spawn(move || {
                std::thread::sleep(after);
                let _ = qt.queue(move |vm| vm.dispatch(&action, &payload));
            });
        }
        for (what, payload) in cx.requests {
            self.as_mut().request(QString::from(&what), QString::from(&payload.to_string()));
        }
    }
}
