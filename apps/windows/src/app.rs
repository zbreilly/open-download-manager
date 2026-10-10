//! Application state that outlives any window: the store, the windows that
//! are open and the notification-area icon.
use crate::engine;
use odm_ui::store::{Change, Store};
use serde_json::{json, Value};
use std::rc::Rc;
use windows_reactor::*;

#[derive(Clone)]
pub enum Message {
    /// an engine event, forwarded from the engine's events thread
    Engine(Rc<str>, Rc<str>),
    ShowMain,
    Tray(Key),
    Quit,
}

pub struct Odm {
    store: Store,
    /// bumped on every change, so window inputs differ and windows redraw
    revision: u64,
    start_error: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct MainInput {
    revision: u64,
    summary: String,
}

struct MainWindow;

impl Component for MainWindow {
    type Input = MainInput;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame(odm_ui::APP_NAME, TextBlock::new().text(&input.summary))
    }
}

impl Application for Odm {
    type Input = ();
    type Message = Message;

    fn create(_: &(), context: &ApplicationContext<Self>) -> Self {
        let mut store = Store::default();
        let sender = context.sender();
        let _ = sender;
        let start_error = engine::start(&engine::data_dir(), |_name, _json| {}).err();
        if start_error.is_none() {
            store.load(|m| engine::call(m, json!({})).ok());
        }
        context.show_window("main");
        Odm { store, revision: 0, start_error }
    }

    fn update(&mut self, message: Message, context: &ApplicationContext<Self>) {
        match message {
            Message::Engine(name, payload) => {
                let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
                if self.store.handle(&name, v).0 != Change::None {
                    self.revision += 1;
                }
            }
            Message::ShowMain => context.show_window("main"),
            Message::Tray(key) => match key.as_str() {
                Some("show") => context.show_window("main"),
                Some("quit") => context.exit(),
                _ => {}
            },
            Message::Quit => context.exit(),
        }
    }

    fn view(&self, _: &(), context: &ApplicationViewContext<Self>) -> ApplicationView {
        let summary = self.start_error.clone().unwrap_or_else(|| self.store.activity_line());
        ApplicationView::new()
            .window::<MainWindow>("main", MainInput { revision: self.revision, summary })
            .notify_icon(
                "tray",
                NotifyIcon::new("Assets\\icon.ico")
                    .tooltip(odm_ui::APP_NAME)
                    .on_activate(context.callback(|_| Message::ShowMain))
                    .menu(Menu::new(
                        [MenuItem::item("show", "Show Open Download Manager"), MenuItem::separator("s1"), MenuItem::item("quit", "Quit")],
                        context.callback(Message::Tray),
                    )),
            )
    }
}

pub fn run() -> Result<(), String> {
    App::run_application::<Odm>(()).map_err(|e| e.to_string())
}
