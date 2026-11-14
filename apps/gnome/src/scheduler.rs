//! Queues & Scheduler: one queue at a time, with its schedule, behavior
//! and run order.
use crate::app::{odm, call, App, Change};
use crate::row::{b, n, opt_n, s};
use crate::widgets::{self, Choice};
use adw::prelude::*;
use gtk::glib;
use odm_linux::fmt;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const DAYS: [(&str, &str); 7] =
    [("Mo", "Monday"), ("Tu", "Tuesday"), ("We", "Wednesday"), ("Th", "Thursday"), ("Fr", "Friday"), ("Sa", "Saturday"), ("Su", "Sunday")];

/// A switch with an hour:minute pair, for "Start at" / "Stop at".
struct TimeRow {
    row: adw::ActionRow,
    on: gtk::Switch,
    hour: gtk::SpinButton,
    minute: gtk::SpinButton,
}

impl TimeRow {
    fn new(title: &str) -> Self {
        let spin = |max: f64| {
            let s = gtk::SpinButton::with_range(0.0, max, 1.0);
            s.set_valign(gtk::Align::Center);
            s.set_orientation(gtk::Orientation::Horizontal);
            s.set_width_chars(2);
            s.set_wrap(true);
            // 01:05, not 1:5
            s.connect_output(|s| {
                s.set_text(&format!("{:02}", s.value_as_int()));
                glib::Propagation::Stop
            });
            s
        };
        let t = TimeRow {
            row: adw::ActionRow::builder().title(title).build(),
            on: gtk::Switch::builder().valign(gtk::Align::Center).build(),
            hour: spin(23.0),
            minute: spin(59.0),
        };
        t.row.add_suffix(&t.hour);
        t.row.add_suffix(&gtk::Label::new(Some(":")));
        t.row.add_suffix(&t.minute);
        t.row.add_suffix(&t.on);
        t.row.set_activatable_widget(Some(&t.on));
        t
    }

    fn set(&self, on: bool, time: &str) {
        let (h, m) = time.split_once(':').unwrap_or(("0", "0"));
        self.on.set_active(on);
        self.hour.set_value(h.parse().unwrap_or(0.0));
        self.minute.set_value(m.parse().unwrap_or(0.0));
        self.hour.set_sensitive(on);
        self.minute.set_sensitive(on);
    }

    fn time(&self) -> String {
        format!("{:02}:{:02}", self.hour.value_as_int(), self.minute.value_as_int())
    }

    fn on_change(&self, f: impl Fn() + Clone + 'static) {
        let (g, hour, minute) = (f.clone(), self.hour.clone(), self.minute.clone());
        self.on.connect_active_notify(move |sw| {
            hour.set_sensitive(sw.is_active());
            minute.set_sensitive(sw.is_active());
            g();
        });
        let g = f.clone();
        self.hour.connect_value_changed(move |_| g());
        self.minute.connect_value_changed(move |_| f());
    }
}

struct Scheduler {
    window: adw::Window,
    list: gtk::ListBox,
    stack: gtk::Stack,
    name: adw::EntryRow,
    title: adw::ActionRow,
    start_now: gtk::Button,
    stop_now: gtk::Button,
    start: TimeRow,
    stop: TimeRow,
    days: Vec<gtk::ToggleButton>,
    next: adw::ActionRow,
    concurrent: adw::SpinRow,
    retry: Choice,
    on_finish: Choice,
    countdown_note: gtk::Label,
    order: gtk::ListBox,
    order_group: adw::PreferencesGroup,
    delete_group: adw::PreferencesGroup,
    summary: gtk::Label,
    revert: gtk::Button,
    save: gtk::Button,

    selected: RefCell<String>,
    draft: RefCell<Option<Value>>,
    dirty: Cell<bool>,
    /// the user reordered the list: only then is the order sent
    reordered: Cell<bool>,
    saving: Cell<bool>,
    quiet: Cell<bool>,
}

pub fn open(app: &Rc<App>) {
    let app2 = app.clone();
    app.single("scheduler", move || build(&app2).upcast());
}

fn build(app: &Rc<App>) -> adw::Window {
    let window = adw::Window::builder().application(&app.gtk).title("Queues & Scheduler").default_width(860).default_height(680).build();
    let sc = Rc::new(Scheduler {
        window: window.clone(),
        list: gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build(),
        stack: gtk::Stack::new(),
        name: adw::EntryRow::builder().title("Name").build(),
        title: adw::ActionRow::new(),
        start_now: gtk::Button::builder().label("Start Now").valign(gtk::Align::Center).build(),
        stop_now: gtk::Button::builder().label("Stop").valign(gtk::Align::Center).build(),
        start: TimeRow::new("Start at"),
        stop: TimeRow::new("Stop at"),
        days: DAYS.iter().map(|(d, full)| gtk::ToggleButton::builder().label(*d).tooltip_text(*full).valign(gtk::Align::Center).build()).collect(),
        next: adw::ActionRow::builder().title("Next run").build(),
        concurrent: adw::SpinRow::with_range(1.0, 32.0, 1.0),
        retry: Choice::new("Retry failed downloads", &[("0", "Never"), ("1", "Once"), ("3", "3 times"), ("5", "5 times"), ("10", "10 times")]),
        on_finish: Choice::new(
            "When the queue finishes",
            &[("none", "Do nothing"), ("sleep", "Put the computer to sleep"), ("shutdown", "Shut down the computer"), ("exit", "Quit Open Download Manager")],
        ),
        countdown_note: widgets::caption("A 60-second countdown with a Cancel button appears first."),
        order: gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build(),
        order_group: adw::PreferencesGroup::builder().title("Order").build(),
        delete_group: adw::PreferencesGroup::new(),
        summary: widgets::caption(""),
        revert: gtk::Button::with_label("Revert"),
        save: gtk::Button::with_label("Save Schedule"),
        selected: RefCell::new("main".into()),
        draft: RefCell::default(),
        dirty: Cell::new(false),
        reordered: Cell::new(false),
        saving: Cell::new(false),
        quiet: Cell::new(false),
    });
    sc.list.add_css_class("navigation-sidebar");
    sc.order.add_css_class("boxed-list");
    sc.save.add_css_class("suggested-action");
    sc.concurrent.set_title("Files at the same time");
    sc.summary.set_hexpand(true);

    // -------------------------------------------------------------- layout
    let page = adw::PreferencesPage::new();
    let head = adw::PreferencesGroup::new();
    sc.title.add_suffix(&sc.start_now);
    sc.title.add_suffix(&sc.stop_now);
    head.add(&sc.title);
    head.add(&sc.name);
    page.add(&head);
    let schedule = adw::PreferencesGroup::builder().title("Schedule").build();
    schedule.add(&sc.start.row);
    schedule.add(&sc.stop.row);
    let repeat = adw::ActionRow::builder().title("Repeat on").title_lines(1).build();
    let days = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    days.add_css_class("linked");
    days.set_valign(gtk::Align::Center);
    for d in &sc.days {
        days.append(d);
    }
    repeat.add_suffix(&days);
    schedule.add(&repeat);
    schedule.add(&sc.next);
    page.add(&schedule);
    let behavior = adw::PreferencesGroup::builder().title("Behavior").build();
    behavior.add(&sc.concurrent);
    behavior.add(&sc.retry.row);
    behavior.add(&sc.on_finish.row);
    behavior.add(&sc.countdown_note);
    page.add(&behavior);
    sc.order_group.add(&sc.order);
    page.add(&sc.order_group);
    let delete = gtk::Button::builder().label("Delete Queue").halign(gtk::Align::Start).build();
    delete.add_css_class("destructive-action");
    sc.delete_group.add(&delete);
    page.add(&sc.delete_group);

    let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).build();
    bar.append(&sc.summary);
    bar.append(&sc.revert);
    bar.append(&sc.save);
    let editor = adw::ToolbarView::new();
    editor.add_top_bar(&adw::HeaderBar::new());
    editor.set_content(Some(&page));
    editor.add_bottom_bar(&bar);
    sc.stack.add_named(&editor, Some("editor"));
    let none = adw::ToolbarView::new();
    none.add_top_bar(&adw::HeaderBar::new());
    none.set_content(Some(&adw::StatusPage::builder().title("Select a queue").build()));
    sc.stack.add_named(&none, Some("none"));

    let side = adw::ToolbarView::new();
    side.add_top_bar(&adw::HeaderBar::builder().show_end_title_buttons(false).build());
    side.set_content(Some(&gtk::ScrolledWindow::builder().child(&sc.list).hscrollbar_policy(gtk::PolicyType::Never).build()));
    let new_queue = gtk::Button::builder().label("New Queue").icon_name("list-add-symbolic").margin_top(8).margin_bottom(8).margin_start(8).margin_end(8).build();
    let new_content = adw::ButtonContent::builder().label("New Queue").icon_name("list-add-symbolic").build();
    new_queue.set_child(Some(&new_content));
    new_queue.add_css_class("flat");
    side.add_bottom_bar(&new_queue);
    let split = adw::NavigationSplitView::builder()
        .sidebar(&adw::NavigationPage::builder().title("Queues").child(&side).build())
        .content(&adw::NavigationPage::builder().title("Queues & Scheduler").child(&sc.stack).build())
        .min_sidebar_width(200.0)
        .build();
    window.set_content(Some(&split));

    // -------------------------------------------------------------- wiring
    let this = sc.clone();
    sc.list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let id = row.widget_name().to_string();
        if !this.quiet.get() && *this.selected.borrow() != id {
            this.pick(&id);
        }
    });
    let edited = {
        let this = sc.clone();
        move || this.edited()
    };
    sc.name.connect_changed({
        let f = edited.clone();
        move |_| f()
    });
    sc.start.on_change(edited.clone());
    sc.stop.on_change(edited.clone());
    for d in &sc.days {
        let f = edited.clone();
        d.connect_toggled(move |_| f());
    }
    sc.concurrent.connect_value_notify({
        let f = edited.clone();
        move |_| f()
    });
    sc.retry.on_change({
        let f = edited.clone();
        move |_| f()
    });
    sc.on_finish.on_change({
        let f = edited.clone();
        move |_| f()
    });
    let this = sc.clone();
    sc.start_now.connect_clicked(move |_| odm().act("start_queue", json!({ "id": *this.selected.borrow() })));
    let this = sc.clone();
    sc.stop_now.connect_clicked(move |_| odm().act("stop_queue", json!({ "id": *this.selected.borrow() })));
    let this = sc.clone();
    sc.revert.connect_clicked(move |_| {
        let id = this.selected.borrow().clone();
        this.pick(&id);
    });
    let this = sc.clone();
    sc.save.connect_clicked(move |_| this.save());
    let this = sc.clone();
    delete.connect_clicked(move |_| {
        odm().act("delete_queue", json!({ "id": *this.selected.borrow() }));
        this.pick("main");
    });
    let this = sc.clone();
    new_queue.connect_clicked(move |_| {
        let this = this.clone();
        glib::spawn_future_local(async move {
            let Some(name) = widgets::ask_text(&this.window, "New Queue", "", "", "Create").await else { return };
            let mut q = json!({
                "id": "", "name": name, "builtin": false, "maxConcurrent": 2, "retryFailed": 3,
                "startEnabled": false, "startTime": "01:00", "stopEnabled": false, "stopTime": "07:00",
                "days": [true, true, true, true, true, true, true], "onFinish": "none", "running": false,
            });
            // no order: the engine keeps (here: starts) its own
            q.as_object_mut().map(|o| o.remove("order"));
            match call("save_queue", json!({ "queue": q })).await {
                Ok(saved) => {
                    *this.selected.borrow_mut() = s(&saved, "id").to_string();
                    *this.draft.borrow_mut() = None;
                    this.queues_changed();
                }
                Err(e) => odm().report(&format!("Could not create the queue: {e}")),
            }
        });
    });

    let this = sc.clone();
    app.subscribe_while_open(&window, move |c| match c {
        Change::Queues => this.queues_changed(),
        Change::List | Change::Progress => this.fill_order(),
        _ => {}
    });
    sc.fill_list();
    sc.pick("main");
    window
}

impl Scheduler {
    fn fill_list(&self) {
        self.quiet.set(true);
        self.list.remove_all();
        let selected = self.selected.borrow().clone();
        for q in odm().queues.borrow().iter() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let icon = if b(q, "running") {
                "media-playback-start-symbolic"
            } else if b(q, "builtin") {
                "view-list-symbolic"
            } else {
                "alarm-symbolic"
            };
            line.append(&gtk::Image::from_icon_name(icon));
            line.append(&gtk::Label::builder().label(s(q, "name")).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
            let count = gtk::Label::new(Some(&q.get("order").and_then(Value::as_array).map(Vec::len).unwrap_or(0).to_string()));
            count.add_css_class("dim-label");
            count.add_css_class("numeric");
            line.append(&count);
            let row = gtk::ListBoxRow::builder().child(&line).name(s(q, "id")).build();
            self.list.append(&row);
            if s(q, "id") == selected {
                self.list.select_row(Some(&row));
            }
        }
        self.quiet.set(false);
    }

    /// Load a queue into the editor, dropping unsaved edits.
    fn pick(self: &Rc<Self>, id: &str) {
        *self.selected.borrow_mut() = id.to_string();
        *self.draft.borrow_mut() = odm().queue(id);
        self.dirty.set(false);
        self.reordered.set(false);
        self.summary.remove_css_class("error");
        self.fill_list();
        self.show_draft();
    }

    fn show_draft(self: &Rc<Self>) {
        let draft = self.draft.borrow().clone();
        let Some(q) = draft else {
            self.stack.set_visible_child_name("none");
            return;
        };
        self.stack.set_visible_child_name("editor");
        self.quiet.set(true);
        let builtin = b(&q, "builtin");
        self.title.set_title(&glib::markup_escape_text(s(&q, "name")));
        self.name.set_visible(!builtin);
        self.name.set_text(s(&q, "name"));
        self.start.set(b(&q, "startEnabled"), s(&q, "startTime"));
        self.stop.set(b(&q, "stopEnabled"), s(&q, "stopTime"));
        let days: Vec<bool> = q.get("days").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_bool().unwrap_or(true)).collect()).unwrap_or_default();
        for (i, btn) in self.days.iter().enumerate() {
            btn.set_active(days.get(i).copied().unwrap_or(true));
        }
        self.concurrent.set_value(n(&q, "maxConcurrent").clamp(1, 32) as f64);
        self.retry.set_value(&n(&q, "retryFailed").to_string());
        self.on_finish.set_value(s(&q, "onFinish"));
        self.delete_group.set_visible(!builtin);
        self.quiet.set(false);
        self.refresh_status();
        self.fill_order();
    }

    /// Running state, next run, summary and button states.
    fn refresh_status(&self) {
        let draft = self.draft.borrow().clone();
        let Some(q) = draft else { return };
        let running = b(&q, "running");
        self.title.set_subtitle(if running { "Running" } else { "" });
        self.start_now.set_sensitive(!running);
        self.stop_now.set_sensitive(running);
        let next = fmt::next_run(&q);
        self.next.set_visible(next.is_some());
        self.next.set_subtitle(next.as_deref().unwrap_or(""));
        self.countdown_note.set_visible(matches!(s(&q, "onFinish"), "sleep" | "shutdown"));
        let app = odm();
        let pending: Vec<Value> = q
            .get("order")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|id| app.row(id.as_str()?))
            .map(|r| r.data())
            .filter(|d| s(d, "status") != "complete")
            .collect();
        let left: u64 = pending.iter().map(|d| opt_n(d, "totalBytes").unwrap_or(0).saturating_sub(n(d, "receivedBytes"))).sum();
        if !self.summary.has_css_class("error") {
            self.summary.set_label(&format!(
                "Next run: {} · {} files · {}",
                next.as_deref().unwrap_or("not scheduled"),
                pending.len(),
                fmt::bytes(Some(left))
            ));
        }
        self.revert.set_sensitive(self.dirty.get() && !self.saving.get());
        self.save.set_sensitive(self.dirty.get() && !self.saving.get());
    }

    /// A control changed: copy the controls into the draft.
    fn edited(&self) {
        if self.quiet.get() {
            return;
        }
        {
            let mut draft = self.draft.borrow_mut();
            let Some(q) = draft.as_mut() else { return };
            if !b(q, "builtin") {
                q["name"] = self.name.text().to_string().into();
            }
            q["startEnabled"] = self.start.on.is_active().into();
            q["startTime"] = self.start.time().into();
            q["stopEnabled"] = self.stop.on.is_active().into();
            q["stopTime"] = self.stop.time().into();
            q["days"] = self.days.iter().map(|d| d.is_active()).collect::<Vec<bool>>().into();
            q["maxConcurrent"] = (self.concurrent.value() as u64).clamp(1, 32).into();
            q["retryFailed"] = self.retry.value().parse::<u64>().unwrap_or(0).into();
            q["onFinish"] = self.on_finish.value().into();
        }
        self.dirty.set(true);
        self.summary.remove_css_class("error");
        self.refresh_status();
    }

    fn order(&self) -> Vec<String> {
        self.draft
            .borrow()
            .as_ref()
            .and_then(|q| q.get("order").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    }

    fn fill_order(self: &Rc<Self>) {
        let app = odm();
        let order = self.order();
        self.order.remove_all();
        if order.is_empty() {
            self.order_group.set_description(Some("No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding."));
            self.order.set_visible(false);
            return;
        }
        self.order_group.set_description(Some("Use the arrows to reorder."));
        self.order.set_visible(true);
        for (i, id) in order.iter().enumerate() {
            let row = adw::ActionRow::new();
            match app.row(id) {
                Some(r) => {
                    let d = r.data();
                    row.set_title(&glib::markup_escape_text(s(&d, "fileName")));
                    row.set_subtitle(&format!("{} · {}", r.status(), fmt::bytes(opt_n(&d, "totalBytes"))));
                    row.add_prefix(&gtk::Image::from_gicon(&crate::row::file_icon(s(&d, "fileName"))));
                }
                None => row.set_title(&glib::markup_escape_text(id)),
            }
            for (icon, tip, delta) in [("go-up-symbolic", "Move up", -1i32), ("go-down-symbolic", "Move down", 1)] {
                let btn = gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).build();
                btn.add_css_class("flat");
                let to = i as i32 + delta;
                btn.set_sensitive(to >= 0 && (to as usize) < order.len());
                let this = self.clone();
                btn.connect_clicked(move |_| this.move_item(i, to as usize));
                row.add_suffix(&btn);
            }
            self.order.append(&row);
        }
    }

    fn move_item(self: &Rc<Self>, from: usize, to: usize) {
        let mut order = self.order();
        if from >= order.len() || to >= order.len() {
            return;
        }
        let item = order.remove(from);
        order.insert(to, item);
        if let Some(q) = self.draft.borrow_mut().as_mut() {
            q["order"] = order.into();
        }
        self.reordered.set(true);
        self.dirty.set(true);
        self.fill_order();
        self.refresh_status();
    }

    /// The engine reported the queues: follow what the user didn't edit.
    fn queues_changed(self: &Rc<Self>) {
        self.fill_list();
        let id = self.selected.borrow().clone();
        let live = odm().queue(&id);
        if self.draft.borrow().is_none() {
            // a queue created here shows up once the engine reports it
            if live.is_some() {
                self.pick(&id);
            }
            return;
        }
        let Some(live) = live else { return self.pick("main") };
        if let Some(q) = self.draft.borrow_mut().as_mut() {
            q["running"] = live.get("running").cloned().unwrap_or(false.into());
            if !self.reordered.get() {
                q["order"] = live.get("order").cloned().unwrap_or_else(|| json!([]));
            }
        }
        self.refresh_status();
        self.fill_order();
    }

    /// The user's arrangement applied to the live membership: downloads
    /// that left the queue are dropped, ones that joined since go to the end.
    fn live_order(wanted: &[String], id: &str) -> Vec<String> {
        let live: Vec<String> = odm()
            .queue(id)
            .and_then(|q| q.get("order").and_then(Value::as_array).cloned())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_else(|| wanted.to_vec());
        let mut out: Vec<String> = wanted.iter().filter(|w| live.contains(w)).cloned().collect();
        for l in live {
            if !out.contains(&l) {
                out.push(l);
            }
        }
        out
    }

    fn save(self: &Rc<Self>) {
        let draft = self.draft.borrow().clone();
        let Some(mut q) = draft else { return };
        if self.saving.replace(true) {
            return;
        }
        let was_reordered = self.reordered.get();
        let wanted = self.order();
        // settings only: the engine keeps the queue's current order
        q.as_object_mut().map(|o| o.remove("order"));
        self.summary.remove_css_class("error");
        self.refresh_status();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut result = call("save_queue", json!({ "queue": q })).await;
            if let (Ok(saved), true) = (&result, was_reordered) {
                let id = s(saved, "id").to_string();
                let order = Scheduler::live_order(&wanted, &id);
                result = call("reorder_queue", json!({ "id": id, "order": order })).await;
            }
            this.saving.set(false);
            match result {
                Ok(_) => {
                    this.dirty.set(false);
                    this.reordered.set(false);
                }
                // keep the edits (dirty) so the user can retry
                Err(e) => {
                    this.summary.set_label(&format!("Could not save: {e}"));
                    this.summary.add_css_class("error");
                }
            }
            this.refresh_status();
        });
    }
}
