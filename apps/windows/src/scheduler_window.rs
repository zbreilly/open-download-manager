//! Queues & Scheduler: one queue at a time, with its schedule, behavior
//! and run order.
use crate::app::{act, Shared};
use crate::engine;
use crate::widgets::*;
use odm_ui::store::live_order;
use odm_ui::{b, fmt, n, s};
use serde_json::{json, Value};
use std::rc::Rc;
use windows_reactor::*;

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub enum Msg {
    Pick(String),
    Name(String),
    /// (field, value): startEnabled / stopEnabled
    Flag(&'static str, bool),
    /// (field, hour or minute, value)
    Time(&'static str, bool, f64),
    Day(usize, bool),
    Concurrent(f64),
    Retry(String),
    OnFinish(String),
    Move(usize, usize),
    Start,
    Stop,
    Revert,
    Save,
    Saved(Result<(), String>),
    Delete,
    NewQueue,
    NewName(String),
    NewAnswer(ContentDialogResult),
    Created(Result<String, String>),
}

pub struct SchedulerWindow {
    shared: Shared,
    selected: String,
    draft: Option<Value>,
    dirty: bool,
    /// the user reordered the list: only then is the order sent
    reordered: bool,
    saving: bool,
    error: String,
    /// naming a new queue
    naming: Option<String>,
}

impl SchedulerWindow {
    fn pick(&mut self, id: &str) {
        self.selected = id.to_string();
        self.draft = self.shared.store.borrow().queue(id).cloned();
        self.dirty = false;
        self.reordered = false;
        self.error.clear();
    }

    fn order(&self) -> Vec<String> {
        self.draft.as_ref().map(|q| odm_ui::strings(q, "order")).unwrap_or_default()
    }

    fn edit(&mut self, f: impl FnOnce(&mut Value)) {
        if let Some(q) = self.draft.as_mut() {
            f(q);
            self.dirty = true;
            self.error.clear();
        }
    }

    fn time(q: &Value, field: &str) -> (u32, u32) {
        let (h, m) = s(q, field).split_once(':').unwrap_or(("0", "0"));
        (h.parse().unwrap_or(0), m.parse().unwrap_or(0))
    }
}

impl Component for SchedulerWindow {
    type Input = Shared;
    type Message = Msg;

    fn create(input: &Self::Input, _: &ComponentContext<Self>) -> Self {
        let mut w = SchedulerWindow {
            shared: input.clone(),
            selected: "main".into(),
            draft: None,
            dirty: false,
            reordered: false,
            saving: false,
            error: String::new(),
            naming: None,
        };
        w.pick("main");
        w
    }

    fn input_changed(&mut self, input: &Self::Input, _: &ComponentContext<Self>) {
        let queues_changed = input.list_revision != self.shared.list_revision;
        self.shared = input.clone();
        if !queues_changed {
            return;
        }
        // the engine reported the queues: follow what the user didn't edit
        let live = self.shared.store.borrow().queue(&self.selected).cloned();
        match (self.draft.as_mut(), live) {
            (None, Some(_)) => {
                // a queue created here shows up once the engine reports it
                let id = self.selected.clone();
                self.pick(&id);
            }
            (Some(_), None) => self.pick("main"),
            (Some(q), Some(live)) => {
                q["running"] = live.get("running").cloned().unwrap_or(false.into());
                if !self.reordered {
                    q["order"] = live.get("order").cloned().unwrap_or_else(|| json!([]));
                }
            }
            (None, None) => {}
        }
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Pick(id) => self.pick(&id),
            Msg::Name(name) => self.edit(|q| {
                if !b(q, "builtin") {
                    q["name"] = name.into();
                }
            }),
            Msg::Flag(field, on) => self.edit(|q| q[field] = on.into()),
            Msg::Time(field, hour, value) => self.edit(|q| {
                let (h, m) = Self::time(q, field);
                let v = if value.is_finite() { value.round().max(0.0) as u32 } else { 0 };
                q[field] = if hour { format!("{:02}:{m:02}", v.min(23)) } else { format!("{h:02}:{:02}", v.min(59)) }.into();
            }),
            Msg::Day(i, on) => self.edit(|q| {
                let mut days: Vec<bool> = q.get("days").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_bool().unwrap_or(true)).collect()).unwrap_or_default();
                days.resize(7, true);
                days[i] = on;
                q["days"] = days.into();
            }),
            Msg::Concurrent(v) => self.edit(|q| q["maxConcurrent"] = (if v.is_finite() { v.round().clamp(1.0, 32.0) as u64 } else { 1 }).into()),
            Msg::Retry(v) => self.edit(|q| q["retryFailed"] = v.parse::<u64>().unwrap_or(0).into()),
            Msg::OnFinish(v) => self.edit(|q| q["onFinish"] = v.into()),
            Msg::Move(from, to) => {
                let mut order = self.order();
                if from < order.len() && to < order.len() {
                    let item = order.remove(from);
                    order.insert(to, item);
                    self.edit(|q| q["order"] = order.into());
                    self.reordered = true;
                }
            }
            Msg::Start => act("start_queue", json!({ "id": self.selected })),
            Msg::Stop => act("stop_queue", json!({ "id": self.selected })),
            Msg::Revert => {
                let id = self.selected.clone();
                self.pick(&id);
            }
            Msg::Save => {
                let Some(mut q) = self.draft.clone() else { return };
                if self.saving {
                    return;
                }
                self.saving = true;
                self.error.clear();
                let wanted = self.reordered.then(|| self.order());
                let live: Vec<String> = self.shared.store.borrow().queue(&self.selected).map(|q| odm_ui::strings(q, "order")).unwrap_or_default();
                // settings only: the engine keeps the queue's current order
                if let Some(o) = q.as_object_mut() {
                    o.remove("order");
                }
                context.spawn_background(move |_| {
                    let saved = engine::call("save_queue", json!({ "queue": q })).and_then(|saved| match wanted {
                        Some(wanted) => {
                            let id = s(&saved, "id").to_string();
                            engine::call("reorder_queue", json!({ "id": id, "order": live_order(&wanted, &live) })).map(|_| ())
                        }
                        None => Ok(()),
                    });
                    Msg::Saved(saved)
                });
            }
            Msg::Saved(result) => {
                self.saving = false;
                match result {
                    Ok(()) => {
                        self.dirty = false;
                        self.reordered = false;
                    }
                    // keep the edits (dirty) so the user can retry
                    Err(e) => self.error = format!("Could not save: {e}"),
                }
            }
            Msg::Delete => {
                act("delete_queue", json!({ "id": self.selected }));
                self.pick("main");
            }
            Msg::NewQueue => self.naming = Some(String::new()),
            Msg::NewName(name) => self.naming = Some(name),
            Msg::NewAnswer(result) => {
                let name = self.naming.take().unwrap_or_default().trim().to_string();
                if result != ContentDialogResult::Primary || name.is_empty() {
                    return;
                }
                // no order: the engine starts its own
                let q = json!({
                    "id": "", "name": name, "builtin": false, "maxConcurrent": 2, "retryFailed": 3,
                    "startEnabled": false, "startTime": "01:00", "stopEnabled": false, "stopTime": "07:00",
                    "days": [true, true, true, true, true, true, true], "onFinish": "none", "running": false,
                });
                context.spawn_background(move |_| Msg::Created(engine::call("save_queue", json!({ "queue": q })).map(|saved| s(&saved, "id").to_string())));
            }
            Msg::Created(Ok(id)) => {
                self.selected = id;
                self.draft = None;
            }
            Msg::Created(Err(e)) => self.shared.report(format!("Could not create the queue: {e}")),
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(self.shared.visuals(880.0, 700.0));
        let store = self.shared.store.clone();
        let store = store.borrow();

        let mut list: Vec<View> = vec![];
        for q in &store.queues {
            let id = s(q, "id").to_string();
            let glyph = if b(q, "running") {
                "\u{E768}"
            } else if b(q, "builtin") {
                "\u{E8FD}"
            } else {
                "\u{E823}"
            };
            let count = q.get("order").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            list.push(
                Button::new()
                    .style(if id == self.selected { ButtonStyle::Default } else { ButtonStyle::Subtle })
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .horizontal_content_alignment(HorizontalAlignment::Stretch)
                    .on_click(context.callback(move |()| Msg::Pick(id.clone())))
                    .content(Grid::new().column_spacing(10.0).columns([GridLength::Auto, GridLength::STAR, GridLength::Auto]).children((
                        FontIcon::new().glyph(glyph),
                        text(s(q, "name")).grid_column(1),
                        dim(count.to_string()).grid_column(2),
                    )))
                    .into(),
            );
        }
        let side = Border::new().border_brush(ThemeBrush::CardStroke).border_thickness(Thickness::new(0.0, 0.0, 1.0, 0.0)).content(
            Grid::new().rows([GridLength::STAR, GridLength::Auto]).children((
                ScrollViewer::new().content(StackPanel::new().spacing(2.0).margin(8.0).children(list)),
                Button::new()
                    .content("New Queue")
                    .grid_row(1)
                    .margin(12.0)
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .on_click(context.callback(|()| Msg::NewQueue)),
            )),
        );

        let editor: View = match &self.draft {
            None => dim("Select a queue").horizontal_alignment(HorizontalAlignment::Center).grid_column(1).into(),
            Some(q) => {
                let builtin = b(q, "builtin");
                let running = b(q, "running");
                let mut head: Vec<View> = vec![text(s(q, "name")).font_size(20.0).font_weight(FontWeight::SEMI_BOLD).into()];
                if running {
                    head.push(TextBlock::new().text("● Running").foreground(status_color("complete")).vertical_alignment(VerticalAlignment::Center).into());
                }
                let mut rows: Vec<View> = vec![Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .margin(Thickness::new(0.0, 0.0, 0.0, 8.0))
                    .children((
                        StackPanel::new().orientation(Orientation::Horizontal).spacing(10.0).children(head),
                        StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).grid_column(1).children((
                            Button::new().content("Start Now").is_enabled(!running).on_click(context.callback(|()| Msg::Start)),
                            Button::new().content("Stop").is_enabled(running).on_click(context.callback(|()| Msg::Stop)),
                        )),
                    ))
                    .into()];
                if !builtin {
                    rows.push(wide("Name", TextBox::new(s(q, "name")).on_text_changed(context.callback(|t: Rc<str>| Msg::Name(t.to_string())))));
                }
                rows.push(heading("Schedule").into());
                for (label, enabled, field) in [("Start at", "startEnabled", "startTime"), ("Stop at", "stopEnabled", "stopTime")] {
                    let (h, m) = Self::time(q, field);
                    let on = b(q, enabled);
                    rows.push(row(
                        label,
                        None,
                        StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((
                            NumberBox::new().minimum(0.0).maximum(23.0).value(h as f64).is_enabled(on).on_value_changed(context.callback(move |v: Option<f64>| Msg::Time(field, true, v.unwrap_or(0.0)))),
                            text(":"),
                            NumberBox::new().minimum(0.0).maximum(59.0).value(m as f64).is_enabled(on).on_value_changed(context.callback(move |v: Option<f64>| Msg::Time(field, false, v.unwrap_or(0.0)))),
                            ToggleSwitch::new().is_on(on).on_content("").off_content("").on_toggled(context.callback(move |v| Msg::Flag(enabled, v))),
                        )),
                    ));
                }
                let days: Vec<bool> = q.get("days").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_bool().unwrap_or(true)).collect()).unwrap_or_default();
                let toggles: Vec<View> = DAYS
                    .iter()
                    .enumerate()
                    .map(|(i, d)| {
                        ToggleButton::new()
                            .content(*d)
                            .is_checked(days.get(i).copied().unwrap_or(true))
                            .on_is_checked_changed(context.callback(move |v: Option<bool>| Msg::Day(i, v == Some(true))))
                            .into()
                    })
                    .collect();
                rows.push(wide("Repeat on", StackPanel::new().orientation(Orientation::Horizontal).spacing(4.0).children(toggles)));
                let next = fmt::next_run(q);
                if let Some(next) = &next {
                    rows.push(row("Next run", None, text(next)));
                }
                rows.push(heading("Behavior").into());
                rows.push(row(
                    "Files at the same time",
                    None,
                    NumberBox::new().minimum(1.0).maximum(32.0).value(n(q, "maxConcurrent").clamp(1, 32) as f64).on_value_changed(context.callback(|v: Option<f64>| Msg::Concurrent(v.unwrap_or(1.0)))),
                ));
                rows.push(row(
                    "Retry failed downloads",
                    None,
                    choice(&[("0", "Never"), ("1", "Once"), ("3", "3 times"), ("5", "5 times"), ("10", "10 times")], &n(q, "retryFailed").to_string(), context.callback(Msg::Retry)),
                ));
                let on_finish = s(q, "onFinish");
                rows.push(row(
                    "When the queue finishes",
                    matches!(on_finish, "sleep" | "shutdown").then_some("A 60-second countdown with a Cancel button appears first."),
                    choice(
                        &[("none", "Do nothing"), ("sleep", "Put the PC to sleep"), ("shutdown", "Shut down the PC"), ("exit", "Quit Open Download Manager")],
                        on_finish,
                        context.callback(Msg::OnFinish),
                    ),
                ));
                rows.push(heading("Order").into());
                let order = self.order();
                if order.is_empty() {
                    rows.push(note("No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding.").into());
                }
                for (i, id) in order.iter().enumerate() {
                    let (name, info) = match store.get(id) {
                        Some(d) => {
                            let r = store.row(d);
                            (r.name, format!("{} · {}", r.status_label, r.size))
                        }
                        None => (id.clone(), String::new()),
                    };
                    let last = order.len() - 1;
                    rows.push(card(Grid::new().column_spacing(8.0).columns([GridLength::STAR, GridLength::Auto, GridLength::Auto, GridLength::Auto]).children((
                        text(name),
                        dim(info).grid_column(1),
                        Button::new().content("↑").is_enabled(i > 0).grid_column(2).on_click(context.callback(move |()| Msg::Move(i, i.saturating_sub(1)))).tooltip("Move up"),
                        Button::new().content("↓").is_enabled(i < last).grid_column(3).on_click(context.callback(move |()| Msg::Move(i, i + 1))).tooltip("Move down"),
                    ))));
                }
                if !builtin {
                    rows.push(Button::new().content("Delete Queue").margin(Thickness::new(0.0, 16.0, 0.0, 0.0)).on_click(context.callback(|()| Msg::Delete)).into());
                }
                let (pending, left) = store.queue_remaining(&order);
                let summary: View = if self.error.is_empty() {
                    dim(format!("Next run: {} · {pending} files · {}", next.as_deref().unwrap_or("not scheduled"), fmt::bytes(Some(left)))).font_size(12.0).into()
                } else {
                    error_text(&self.error).into()
                };
                let can_save = self.dirty && !self.saving;
                Grid::new()
                    .grid_column(1)
                    .rows([GridLength::STAR, GridLength::Auto])
                    .children((
                        ScrollViewer::new().content(StackPanel::new().margin(Thickness::xy(20.0, 12.0)).children(rows)),
                        Border::new()
                            .grid_row(1)
                            .border_brush(ThemeBrush::CardStroke)
                            .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
                            .padding(Thickness::xy(20.0, 12.0))
                            .content(Grid::new().column_spacing(8.0).columns([GridLength::STAR, GridLength::Auto, GridLength::Auto]).children((
                                StackPanel::new().vertical_alignment(VerticalAlignment::Center).children((summary,)),
                                Button::new().content("Revert").is_enabled(can_save).grid_column(1).on_click(context.callback(|()| Msg::Revert)),
                                Button::new().style(ButtonStyle::Accent).content("Save Schedule").is_enabled(can_save).grid_column(2).on_click(context.callback(|()| Msg::Save)),
                            ))),
                    ))
                    .into()
            }
        };

        let naming = ContentDialog::new()
            .is_open(self.naming.is_some())
            .title("New Queue")
            .content(TextBox::new(self.naming.as_deref().unwrap_or("")).placeholder_text("Name").on_text_changed(context.callback(|t: Rc<str>| Msg::NewName(t.to_string()))))
            .primary_button_text("Create")
            .close_button_text("Cancel")
            .on_closed(context.callback(Msg::NewAnswer));
        let root = Grid::new().columns([GridLength::Pixel(230.0), GridLength::STAR]).children((side, editor));
        context.window_frame("Queues & Scheduler", root.content_dialog(naming))
    }
}
