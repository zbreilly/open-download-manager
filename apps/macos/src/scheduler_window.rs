//! Queues & Scheduler: the queues on the left, the chosen one's schedule,
//! behavior and order on the right.
use crate::app::app;
use crate::kit::{self, mtm, ns, KeyTable, TableCallbacks};
use crate::{engine, system};
use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::*;
use odm_ui::store::{live_order, Change};
use odm_ui::{b, fmt, n, opt_n, s, strings};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const RETRIES: [(u64, &str); 5] = [(0, "Never"), (1, "Once"), (3, "3 times"), (5, "5 times"), (10, "10 times")];
const ON_FINISH: [(&str, &str); 4] = [("none", "Do nothing"), ("sleep", "Put the Mac to sleep"), ("shutdown", "Shut down the Mac"), ("exit", "Quit Open Download Manager")];

pub struct SchedulerWindow {
    window: Retained<NSWindow>,
    selected: RefCell<String>,
    /// the queue being edited, as the engine's JSON
    draft: RefCell<Option<Value>>,
    dirty: Cell<bool>,
    /// the user rearranged the order: only then is it sent
    reordered: Cell<bool>,
    saving: Cell<bool>,
    save_error: RefCell<String>,

    queues: Retained<KeyTable>,
    editor: Retained<NSStackView>,
    placeholder: Retained<NSTextField>,
    name: Retained<NSTextField>,
    running: Retained<NSTextField>,
    start_now: Retained<NSButton>,
    stop_now: Retained<NSButton>,
    start_on: Retained<NSButton>,
    start_time: Retained<NSDatePicker>,
    stop_on: Retained<NSButton>,
    stop_time: Retained<NSDatePicker>,
    days: Vec<Retained<NSButton>>,
    next_run: Retained<NSTextField>,
    concurrent: Retained<NSStepper>,
    concurrent_label: Retained<NSTextField>,
    retry: Retained<NSPopUpButton>,
    on_finish: Retained<NSPopUpButton>,
    countdown_note: Retained<NSTextField>,
    order: Retained<KeyTable>,
    order_scroll: Retained<NSScrollView>,
    order_empty: Retained<NSTextField>,
    order_hint: Retained<NSTextField>,
    delete: Retained<NSButton>,
    summary: Retained<NSTextField>,
    revert: Retained<NSButton>,
    save: Retained<NSButton>,
}

fn time_picker(f: impl Fn() + 'static) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm());
    unsafe {
        p.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
        p.setDatePickerElements(NSDatePickerElementFlags::HourMinute);
    }
    kit::on_action(&p, f);
    p
}

/// Show "HH:MM" in a time picker.
fn set_time(p: &NSDatePicker, text: &str) {
    let mut parts = text.split(':').filter_map(|x| x.parse::<isize>().ok());
    let (hour, minute) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    unsafe {
        let c = NSDateComponents::new();
        c.setYear(2001);
        c.setMonth(1);
        c.setDay(1);
        c.setHour(hour);
        c.setMinute(minute);
        if let Some(date) = NSCalendar::currentCalendar().dateFromComponents(&c) {
            p.setDateValue(&date);
        }
    }
}

/// The "HH:MM" a time picker shows.
fn time_of(p: &NSDatePicker) -> String {
    unsafe {
        let c = NSCalendar::currentCalendar().components_fromDate(NSCalendarUnit::Hour | NSCalendarUnit::Minute, &p.dateValue());
        format!("{:02}:{:02}", c.hour(), c.minute())
    }
}

fn new_queue(name: &str) -> Value {
    json!({
        "id": "", "name": name, "builtin": false, "maxConcurrent": 2, "retryFailed": 3,
        "startEnabled": false, "startTime": "01:00", "stopEnabled": false, "stopTime": "07:00",
        "days": [true, true, true, true, true, true, true], "onFinish": "none", "running": false, "nextRun": null,
    })
}

impl SchedulerWindow {
    pub fn new() -> Rc<SchedulerWindow> {
        Rc::new_cyclic(|weak: &Weak<SchedulerWindow>| {
            let window = kit::window("Queues & Scheduler", 820.0, 640.0, (720.0, 540.0), true);
            let edited = kit::bound(weak, |w| w.edited());

            // queues
            let (queue_scroll, queues) = kit::table(
                &[("name", "", 140.0, 80.0, false), ("count", "", 36.0, 30.0, false)],
                26.0,
                TableCallbacks {
                    rows: Box::new(|| app().store.borrow().queues.len()),
                    cell: Box::new(|table, column, row| {
                        let app = app();
                        let st = app.store.borrow();
                        let q = st.queues.get(row)?;
                        if column == "count" {
                            let (cell, label, _) = kit::text_cell(table, "count", false);
                            kit::set_text(&label, &strings(q, "order").len().to_string());
                            unsafe {
                                label.setAlignment(NSTextAlignment::Right);
                                label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                            }
                            return Some(kit::view(&cell));
                        }
                        let (cell, label, image) = kit::text_cell(table, "queue", true);
                        kit::set_text(&label, s(q, "name"));
                        let symbol = if b(q, "running") {
                            "play.circle.fill"
                        } else if b(q, "builtin") {
                            "list.bullet"
                        } else {
                            "clock"
                        };
                        if let Some(image) = image {
                            unsafe { image.setImage(kit::symbol(symbol, "").as_deref()) };
                        }
                        Some(kit::view(&cell))
                    }),
                    selection_changed: Box::new(kit::bound(weak, |w| w.queue_selected())),
                    ..Default::default()
                },
            );
            unsafe {
                queues.setStyle(NSTableViewStyle::SourceList);
                queues.setAllowsEmptySelection(false);
                queue_scroll.setHasHorizontalScroller(false);
                queue_scroll.setDrawsBackground(false);
            }
            let add = kit::button("New Queue", kit::bound(weak, |w| w.create_queue()));
            unsafe {
                add.setImage(kit::symbol("plus", "").as_deref());
                add.setImagePosition(NSCellImagePosition::ImageLeading);
                add.setBordered(false);
            }
            let add_row = kit::hstack(0.0, &[&add, &kit::spacer()]);
            kit::pad(&add_row, 6.0, 10.0, 8.0, 10.0);
            queue_scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let left = kit::vfill(0.0, &[&queue_scroll, &add_row]);
            kit::width(&left, 210.0);

            // the queue itself
            let name = kit::text_field("Name", edited.clone());
            kit::set_font(&name, 15.0, true);
            let running = kit::label("● Running");
            unsafe {
                running.setTextColor(Some(&NSColor::systemGreenColor()));
                running.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
            }
            let start_now = kit::button("Start Now", kit::bound(weak, |w| app().start_queue(&w.selected.borrow())));
            let stop_now = kit::button("Stop", kit::bound(weak, |w| app().stop_queue(&w.selected.borrow())));
            let head = kit::hstack(10.0, &[&name, &running, &start_now, &stop_now]);

            // schedule
            let start_on = kit::checkbox("Start at", edited.clone());
            let start_time = time_picker(edited.clone());
            let stop_on = kit::checkbox("Stop at", edited.clone());
            let stop_time = time_picker(edited.clone());
            let days: Vec<Retained<NSButton>> = DAYS
                .iter()
                .map(|d| {
                    let button = kit::button(d, edited.clone());
                    unsafe { button.setButtonType(NSButtonType::PushOnPushOff) };
                    button
                })
                .collect();
            let day_refs: Vec<&NSView> = days.iter().map(|d| &***d as &NSView).collect();
            let day_row = kit::hstack(4.0, &day_refs);
            let next_run = kit::label("");

            // behavior
            let concurrent = NSStepper::new(mtm());
            unsafe {
                concurrent.setMinValue(1.0);
                concurrent.setMaxValue(32.0);
                concurrent.setIncrement(1.0);
                concurrent.setValueWraps(false);
            }
            kit::on_action(&concurrent, edited.clone());
            let concurrent_label = kit::label("2");
            kit::digits(&concurrent_label);
            let retry_titles: Vec<String> = RETRIES.iter().map(|r| r.1.to_string()).collect();
            let retry = kit::popup(&retry_titles, edited.clone());
            let finish_titles: Vec<String> = ON_FINISH.iter().map(|r| r.1.to_string()).collect();
            let on_finish = kit::popup(&finish_titles, edited.clone());
            let countdown_note = kit::note("A 60-second countdown with a Cancel button appears first.");

            // order
            let (order_scroll, order) = kit::table(
                &[("name", "", 300.0, 120.0, false), ("status", "", 100.0, 60.0, false), ("size", "", 80.0, 60.0, false)],
                24.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.order_ids().len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |table, column, row| w.upgrade().and_then(|w| w.order_cell(table, column, row))
                    }),
                    move_row: Some(Box::new({
                        let w = weak.clone();
                        move |from, to| {
                            if let Some(w) = w.upgrade() {
                                w.move_row(from, to);
                            }
                        }
                    })),
                    ..Default::default()
                },
            );
            kit::allow_reorder(&order);
            unsafe {
                order_scroll.setBorderType(NSBorderType::BezelBorder);
                order_scroll.setHasHorizontalScroller(false);
                order.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle);
            }
            kit::min_height(&order_scroll, 140.0);
            order_scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let order_empty = kit::note("No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding.");
            let order_hint = kit::note("Drag to reorder.");

            let delete = kit::button("Delete Queue", kit::bound(weak, |w| {
                engine::act("delete_queue", json!({ "id": &*w.selected.borrow() }));
                *w.selected.borrow_mut() = "main".into();
            }));
            unsafe { delete.setHasDestructiveAction(true) };

            let editor = kit::vfill_in(
                10.0,
                (16.0, 18.0, 12.0, 18.0),
                &[
                    &head,
                    &kit::hline(),
                    &kit::heading("Schedule"),
                    &kit::wide_row(180.0, "", &kit::hstack(8.0, &[&start_on, &start_time])),
                    &kit::wide_row(180.0, "", &kit::hstack(8.0, &[&stop_on, &stop_time])),
                    &kit::wide_row(180.0, "Repeat on", &day_row),
                    &kit::wide_row(180.0, "Next run", &next_run),
                    &kit::heading("Behavior"),
                    &kit::wide_row(180.0, "Files at the same time", &kit::hstack(6.0, &[&concurrent_label, &concurrent])),
                    &kit::wide_row(180.0, "Retry failed downloads", &retry),
                    &kit::wide_row(180.0, "When the queue finishes", &on_finish),
                    &kit::wide_row(180.0, "", &countdown_note),
                    &kit::heading("Order"),
                    &order_empty,
                    &order_scroll,
                    &order_hint,
                    &kit::hstack(0.0, &[&delete, &kit::spacer()]),
                ],
            );
            kit::stretch(&editor, false);
            kit::stretch(&editor, true);
            let placeholder = kit::dim("Select a queue");

            // footer
            let summary = kit::note("");
            let revert = kit::button("Revert", kit::bound(weak, |w| {
                let id = w.selected.borrow().clone();
                w.pick(&id);
            }));
            let save = kit::button("Save Schedule", kit::bound(weak, |w| w.save()));
            kit::default_button(&save);
            let footer = kit::hstack(10.0, &[&summary, &kit::spacer(), &revert, &save]);
            kit::pad(&footer, 10.0, 14.0, 10.0, 14.0);

            let right = kit::vfill(0.0, &[&editor, &placeholder, &kit::hline(), &footer]);
            kit::stretch(&right, true);
            let divider = NSBox::new(mtm());
            unsafe { divider.setBoxType(NSBoxType::Separator) };
            let root = kit::hstack(0.0, &[&left, &divider, &right]);
            unsafe {
                root.setAlignment(NSLayoutAttribute::Top);
                window.setContentView(Some(&root));
                right.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
                for v in [&**left as &NSView, &**right as &NSView, &**divider as &NSView] {
                    v.heightAnchor().constraintEqualToAnchor(&root.heightAnchor()).setActive(true);
                }
                window.setFrameAutosaveName(&ns("ODMSchedulerWindow"));
            }

            SchedulerWindow {
                window,
                selected: RefCell::new("main".into()),
                draft: RefCell::new(None),
                dirty: Cell::new(false),
                reordered: Cell::new(false),
                saving: Cell::new(false),
                save_error: RefCell::new(String::new()),
                queues,
                editor,
                placeholder,
                name,
                running,
                start_now,
                stop_now,
                start_on,
                start_time,
                stop_on,
                stop_time,
                days,
                next_run,
                concurrent,
                concurrent_label,
                retry,
                on_finish,
                countdown_note,
                order,
                order_scroll,
                order_empty,
                order_hint,
                delete,
                summary,
                revert,
                save,
            }
        })
    }

    pub fn show(&self) {
        if !unsafe { self.window.isVisible() } {
            self.reload_queues();
            let id = self.selected.borrow().clone();
            self.pick(&id);
        }
        kit::present(&self.window);
    }

    fn reload_queues(&self) {
        self.queues.reload();
        let row = app().store.borrow().queues.iter().position(|q| s(q, "id") == *self.selected.borrow());
        if let Some(row) = row {
            self.queues.select(&[row]);
        }
    }

    fn queue_selected(&self) {
        if self.queues.loading() {
            return;
        }
        let id = self.queues.selected().first().and_then(|row| app().store.borrow().queues.get(*row).map(|q| s(q, "id").to_string()));
        if let Some(id) = id {
            self.pick(&id);
        }
    }

    /// Edit a queue, starting from what the engine has.
    fn pick(&self, id: &str) {
        *self.selected.borrow_mut() = id.to_string();
        *self.draft.borrow_mut() = app().store.borrow().queue(id).cloned();
        self.dirty.set(false);
        self.reordered.set(false);
        self.save_error.borrow_mut().clear();
        self.push();
        self.sync();
    }

    /// Show the draft in the controls.
    fn push(&self) {
        let draft = self.draft.borrow();
        let Some(q) = &*draft else { return };
        kit::set_text(&self.name, s(q, "name"));
        kit::set_on(&self.start_on, b(q, "startEnabled"));
        kit::set_on(&self.stop_on, b(q, "stopEnabled"));
        set_time(&self.start_time, s(q, "startTime"));
        set_time(&self.stop_time, s(q, "stopTime"));
        let days = q.get("days").and_then(Value::as_array).cloned().unwrap_or_default();
        for (i, button) in self.days.iter().enumerate() {
            kit::set_on(button, days.get(i).and_then(Value::as_bool).unwrap_or(true));
        }
        unsafe {
            self.concurrent.setIntegerValue(n(q, "maxConcurrent").clamp(1, 32) as isize);
            self.retry.selectItemAtIndex(RETRIES.iter().position(|r| r.0 == n(q, "retryFailed")).unwrap_or(2) as isize);
            self.on_finish.selectItemAtIndex(ON_FINISH.iter().position(|r| r.0 == s(q, "onFinish")).unwrap_or(0) as isize);
        }
    }

    /// Read the controls into the draft.
    fn pull(&self) {
        let mut draft = self.draft.borrow_mut();
        let Some(q) = &mut *draft else { return };
        if !b(q, "builtin") {
            q["name"] = kit::text_of(&self.name).into();
        }
        q["startEnabled"] = kit::is_on(&self.start_on).into();
        q["stopEnabled"] = kit::is_on(&self.stop_on).into();
        q["startTime"] = time_of(&self.start_time).into();
        q["stopTime"] = time_of(&self.stop_time).into();
        q["days"] = self.days.iter().map(|d| kit::is_on(d)).collect::<Vec<bool>>().into();
        q["maxConcurrent"] = (unsafe { self.concurrent.integerValue() }.clamp(1, 32) as u64).into();
        if let Some(r) = RETRIES.get(kit::popup_index(&self.retry)) {
            q["retryFailed"] = r.0.into();
        }
        if let Some(r) = ON_FINISH.get(kit::popup_index(&self.on_finish)) {
            q["onFinish"] = r.0.into();
        }
    }

    fn edited(&self) {
        self.pull();
        self.dirty.set(true);
        self.sync();
    }

    fn order_ids(&self) -> Vec<String> {
        self.draft.borrow().as_ref().map(|q| strings(q, "order")).unwrap_or_default()
    }

    fn order_cell(&self, table: &NSTableView, column: &str, row: usize) -> Option<Retained<NSView>> {
        let id = self.order_ids().get(row)?.clone();
        let app = app();
        let st = app.store.borrow();
        let d = st.get(&id);
        let (cell, label, image) = kit::text_cell(table, column, column == "name");
        let text = match (column, d) {
            ("name", Some(d)) => s(d, "fileName").to_string(),
            ("name", None) => id.clone(),
            ("status", Some(d)) => fmt::status_label(s(d, "status"), d.get("scheduledAt").and_then(Value::as_str)),
            (_, Some(d)) => fmt::bytes(opt_n(d, "totalBytes")),
            _ => String::new(),
        };
        kit::set_text(&label, &text);
        if column != "name" {
            unsafe { label.setTextColor(Some(&NSColor::secondaryLabelColor())) };
        }
        if let (Some(image), Some(d)) = (image, d) {
            unsafe { image.setImage(Some(&system::file_icon(&fmt::extension(s(d, "fileName"))))) };
        }
        Some(kit::view(&cell))
    }

    /// A row of the order was dragged from `from` to before `to`.
    fn move_row(&self, from: usize, to: usize) {
        let mut order = self.order_ids();
        if from >= order.len() {
            return;
        }
        let id = order.remove(from);
        let at = if to > from { to - 1 } else { to }.min(order.len());
        order.insert(at, id);
        if let Some(q) = &mut *self.draft.borrow_mut() {
            q["order"] = order.into();
        }
        self.reordered.set(true);
        self.dirty.set(true);
        self.sync();
    }

    /// Everything derived from the draft: labels, what is enabled, the order.
    fn sync(&self) {
        let draft = self.draft.borrow().clone();
        kit::show(&self.editor, draft.is_some());
        kit::show(&self.placeholder, draft.is_none());
        let Some(q) = draft else { return };
        let builtin = b(&q, "builtin");
        let running = b(&q, "running");
        unsafe {
            self.name.setEditable(!builtin);
            self.name.setBezeled(!builtin);
            self.name.setDrawsBackground(!builtin);
        }
        kit::show(&self.running, running);
        kit::enable(&self.start_now, !running);
        kit::enable(&self.stop_now, running);
        kit::enable(&self.start_time, b(&q, "startEnabled"));
        kit::enable(&self.stop_time, b(&q, "stopEnabled"));
        let next = fmt::next_run(&q);
        if let Some(parent) = unsafe { self.next_run.superview() } {
            kit::show(&parent, next.is_some());
        }
        kit::set_text(&self.next_run, next.as_deref().unwrap_or(""));
        kit::set_text(&self.concurrent_label, &n(&q, "maxConcurrent").to_string());
        if let Some(parent) = unsafe { self.countdown_note.superview() } {
            kit::show(&parent, matches!(s(&q, "onFinish"), "sleep" | "shutdown"));
        }
        let order = strings(&q, "order");
        kit::show(&self.order_empty, order.is_empty());
        kit::show(&self.order_scroll, !order.is_empty());
        kit::show(&self.order_hint, !order.is_empty());
        self.order.reload();
        if let Some(parent) = unsafe { self.delete.superview() } {
            kit::show(&parent, !builtin);
        }
        let error = self.save_error.borrow().clone();
        if error.is_empty() {
            let (files, left) = app().store.borrow().queue_remaining(&order);
            kit::set_text(&self.summary, &format!("Next run: {} · {files} files · {}", next.as_deref().unwrap_or("not scheduled"), fmt::bytes(Some(left))));
            unsafe { self.summary.setTextColor(Some(&NSColor::secondaryLabelColor())) };
        } else {
            kit::set_text(&self.summary, &error);
            kit::red(&self.summary);
        }
        let can_save = self.dirty.get() && !self.saving.get();
        kit::enable(&self.revert, can_save);
        kit::enable(&self.save, can_save);
    }

    /// The engine reported a change.
    pub fn refresh(&self, change: Change) {
        if !unsafe { self.window.isVisible() } || !matches!(change, Change::Queues | Change::List) {
            return;
        }
        self.reload_queues();
        let id = self.selected.borrow().clone();
        let live = app().store.borrow().queue(&id).cloned();
        match live {
            // a queue created here shows up once the engine reports it
            Some(_) if self.draft.borrow().is_none() => self.pick(&id),
            Some(live) => {
                if let Some(q) = &mut *self.draft.borrow_mut() {
                    q["running"] = live["running"].clone();
                    if !self.reordered.get() {
                        q["order"] = live["order"].clone();
                    }
                }
                self.sync();
            }
            // the queue being edited was deleted
            None => self.pick("main"),
        }
    }

    fn save(self: &Rc<Self>) {
        self.pull();
        let Some(mut q) = self.draft.borrow().clone() else { return };
        if self.saving.get() {
            return;
        }
        let wanted = strings(&q, "order");
        let reordered = self.reordered.get();
        // settings only: without an order the engine keeps the queue's own
        if let Some(o) = q.as_object_mut() {
            o.remove("order");
        }
        self.saving.set(true);
        self.save_error.borrow_mut().clear();
        self.sync();
        let weak = Rc::downgrade(self);
        engine::background(
            move || -> Result<(), String> {
                let saved = engine::call("save_queue", json!({ "queue": q }))?;
                if reordered {
                    let live = strings(&saved, "order");
                    engine::call("reorder_queue", json!({ "id": s(&saved, "id"), "order": live_order(&wanted, &live) }))?;
                }
                Ok(())
            },
            move |result| {
                let Some(w) = weak.upgrade() else { return };
                w.saving.set(false);
                match result {
                    Ok(()) => {
                        w.dirty.set(false);
                        w.reordered.set(false);
                    }
                    // the edits stay, so the user can try again
                    Err(e) => *w.save_error.borrow_mut() = format!("Could not save: {e}"),
                }
                w.sync();
            },
        );
    }

    fn create_queue(self: &Rc<Self>) {
        let Some(name) = kit::prompt("New Queue", "", "", "Create") else { return };
        self.create_named(name.trim());
    }

    fn create_named(self: &Rc<Self>, name: &str) {
        let name = name.to_string();
        if name.is_empty() {
            return;
        }
        let weak = Rc::downgrade(self);
        engine::spawn("save_queue", json!({ "queue": new_queue(&name) }), move |result| match result {
            Ok(saved) => {
                if let Some(w) = weak.upgrade() {
                    let id = s(&saved, "id").to_string();
                    w.reload_queues();
                    w.pick(&id);
                    w.reload_queues();
                }
            }
            Err(e) => app().report(&format!("Could not create the queue: {e}")),
        });
    }
}

#[cfg(debug_assertions)]
impl SchedulerWindow {
    /// Create a queue, then give the main queue a start time and save it
    /// (see `ODM_DO`).
    pub fn debug_edit(self: &Rc<Self>) {
        self.create_named("Night");
        self.pick("main");
        kit::set_on(&self.start_on, true);
        set_time(&self.start_time, "03:30");
        self.edited();
        self.save();
    }
}
