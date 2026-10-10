//! Per-download progress window: big percentage, segment map, one line per
//! connection, the speed graph, a limit for this download and what to do
//! when it finishes.
use crate::app::{odm, App, Change};
use crate::row::{b, n, opt_n, s, Row};
use crate::widgets::{self, Choice, SegmentMap, Sparkline};
use adw::prelude::*;
use gtk::glib;
use odm_linux::fmt;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Progress {
    window: adw::Window,
    id: String,
    icon: gtk::Image,
    name: gtk::Label,
    place: gtk::Label,
    status: gtk::Label,
    percent: gtk::Label,
    stats: [gtk::Label; 4],
    error: gtk::Label,
    verified: gtk::Label,
    seg_title: gtk::Label,
    segments: SegmentMap,
    connections: gtk::ListBox,
    rows: RefCell<Vec<(adw::ActionRow, gtk::Label)>>,
    spark: Sparkline,
    limit_on: adw::SwitchRow,
    limit: adw::EntryRow,
    limit_error: gtk::Label,
    finished: Choice,
    hide: gtk::Button,
    cancel: gtk::Button,
    toggle: gtk::Button,
    reveal: gtk::Button,
    open: gtk::Button,
    closing: gtk::Label,
    saw_incomplete: Cell<bool>,
    close_at: RefCell<Option<Instant>>,
    hovering: Cell<bool>,
    quiet: Cell<bool>,
}

fn state_label(state: &str) -> &'static str {
    match state {
        "receiving" => "Receiving",
        "reconnecting" => "Reconnecting",
        "done" => "Done",
        "connecting" => "Connecting",
        _ => "Idle",
    }
}

pub fn build(app: &Rc<App>, id: &str) -> adw::Window {
    let window = adw::Window::builder().application(&app.gtk).default_width(760).default_height(680).build();
    let big = gtk::Label::builder().xalign(0.0).build();
    big.add_css_class("title-1");
    big.add_css_class("numeric");
    let stat = || {
        let l = gtk::Label::builder().xalign(0.0).build();
        l.add_css_class("numeric");
        l
    };
    let p = Rc::new(Progress {
        window: window.clone(),
        id: id.to_string(),
        icon: gtk::Image::builder().pixel_size(40).build(),
        name: gtk::Label::builder().xalign(0.0).selectable(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build(),
        place: widgets::value_label(),
        status: gtk::Label::new(None),
        percent: big,
        stats: [stat(), stat(), stat(), stat()],
        error: gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).visible(false).build(),
        verified: gtk::Label::builder().label("✓ Checksum verified").xalign(0.0).visible(false).build(),
        seg_title: widgets::caption(""),
        segments: SegmentMap::new(26, true),
        connections: gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build(),
        rows: RefCell::default(),
        spark: Sparkline::new(80),
        limit_on: adw::SwitchRow::builder().title("Limit this download").subtitle("Overrides the per-download setting.").build(),
        limit: adw::EntryRow::builder().title("MB/s").show_apply_button(true).visible(false).build(),
        limit_error: gtk::Label::builder().xalign(0.0).visible(false).build(),
        finished: Choice::new(
            "When finished",
            &[("notify", "Show notification"), ("openFile", "Open file"), ("openFolder", "Show in folder"), ("none", "Do nothing")],
        ),
        hide: gtk::Button::with_label("Hide"),
        cancel: gtk::Button::with_label("Cancel Download"),
        toggle: gtk::Button::new(),
        reveal: gtk::Button::with_label("Show in Folder"),
        open: gtk::Button::with_label("Open"),
        closing: widgets::caption(""),
        saw_incomplete: Cell::new(false),
        close_at: RefCell::default(),
        hovering: Cell::new(false),
        quiet: Cell::new(false),
    });
    p.name.add_css_class("title-3");
    p.place.add_css_class("caption");
    p.place.add_css_class("dim-label");
    p.error.add_css_class("error");
    p.verified.add_css_class("success");
    p.limit_error.add_css_class("error");
    p.limit_error.add_css_class("caption");
    p.cancel.add_css_class("destructive-action");
    p.toggle.add_css_class("suggested-action");
    p.open.add_css_class("suggested-action");
    p.connections.add_css_class("boxed-list");

    // -------------------------------------------------------------- layout
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(20)
        .margin_end(20)
        .build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
    titles.append(&p.name);
    titles.append(&p.place);
    head.append(&p.icon);
    head.append(&titles);
    head.append(&p.status);
    body.append(&head);

    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    hero.append(&p.percent);
    let grid = gtk::Grid::builder().column_spacing(24).row_spacing(2).valign(gtk::Align::Center).build();
    for (i, title) in ["DOWNLOADED", "SPEED", "TIME LEFT", "RESUME"].iter().enumerate() {
        let l = widgets::caption(title);
        grid.attach(&l, i as i32, 0, 1, 1);
        grid.attach(&p.stats[i], i as i32, 1, 1, 1);
    }
    hero.append(&grid);
    body.append(&hero);
    body.append(&p.error);
    body.append(&p.verified);

    let seg_head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    p.seg_title.set_hexpand(true);
    seg_head.append(&p.seg_title);
    seg_head.append(&widgets::caption("Faded = segment done · light edge = write position"));
    body.append(&seg_head);
    body.append(&p.segments.area);

    let lower = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let conn_scroll = gtk::ScrolledWindow::builder()
        .child(&p.connections)
        .hexpand(true)
        .vexpand(true)
        .min_content_height(170)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    lower.append(&conn_scroll);
    let side = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).width_request(300).build();
    side.append(&p.spark.root);
    let options = adw::PreferencesGroup::new();
    options.add(&p.limit_on);
    options.add(&p.limit);
    options.add(&p.finished.row);
    side.append(&options);
    side.append(&p.limit_error);
    lower.append(&side);
    body.append(&lower);

    let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(8).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).build();
    bar.append(&p.hide);
    bar.append(&p.closing);
    bar.append(&gtk::Box::builder().hexpand(true).build());
    for btn in [&p.cancel, &p.toggle, &p.reveal, &p.open] {
        bar.append(btn);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&gtk::ScrolledWindow::builder().child(&body).hscrollbar_policy(gtk::PolicyType::Never).build()));
    view.add_bottom_bar(&bar);
    window.set_content(Some(&view));

    // -------------------------------------------------------------- wiring
    let w = window.clone();
    p.hide.connect_clicked(move |_| w.close());
    let this = p.clone();
    p.cancel.connect_clicked(move |_| {
        let this = this.clone();
        glib::spawn_future_local(async move {
            let choice = widgets::ask(
                &this.window,
                "Cancel this download?",
                "The partial file is deleted and the download is removed from the list.",
                &[("stop", "Cancel Download", adw::ResponseAppearance::Destructive)],
            )
            .await;
            if choice == "stop" {
                odm().delete(std::slice::from_ref(&this.id), false);
                this.window.close();
            }
        });
    });
    let this = p.clone();
    p.toggle.connect_clicked(move |_| {
        let app = odm();
        let Some(row) = app.row(&this.id) else { return };
        if row.is_active() || row.state() == "queued" {
            app.pause(std::slice::from_ref(&this.id));
        } else {
            app.resume(std::slice::from_ref(&this.id));
        }
    });
    let this = p.clone();
    p.reveal.connect_clicked(move |_| {
        if let Some(row) = odm().row(&this.id) {
            odm().reveal(&row);
        }
    });
    let this = p.clone();
    p.open.connect_clicked(move |_| {
        if let Some(row) = odm().row(&this.id) {
            odm().open_file(&row);
        }
        this.window.close();
    });
    let this = p.clone();
    p.limit_on.connect_active_notify(move |sw| {
        this.limit.set_visible(sw.is_active());
        if !this.quiet.get() {
            this.apply_limit();
        }
    });
    let this = p.clone();
    p.limit.connect_apply(move |_| this.apply_limit());
    let this = p.clone();
    p.finished.on_change(move |v| {
        if !this.quiet.get() {
            odm().act("update_download", json!({ "id": this.id, "patch": { "whenFinished": v } }));
        }
    });
    let hover = gtk::EventControllerMotion::new();
    let this = p.clone();
    hover.connect_enter(move |_, _, _| {
        this.hovering.set(true);
        // looking at it: don't close it under the pointer
        *this.close_at.borrow_mut() = None;
        this.closing.set_label("");
    });
    let this = p.clone();
    hover.connect_leave(move |_| this.hovering.set(false));
    window.add_controller(hover);
    let esc = gtk::ShortcutController::new();
    esc.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("Escape"), Some(gtk::NamedAction::new("window.close"))));
    window.add_controller(esc);

    if let Some(row) = app.row(id) {
        let d = row.data();
        p.saw_incomplete.set(s(&d, "status") != "complete");
        p.quiet.set(true);
        if let Some(l) = opt_n(&d, "speedLimitBps").filter(|l| *l > 0) {
            p.limit_on.set_active(true);
            p.limit.set_text(&fmt::mbps(l));
        } else {
            p.limit.set_text("5");
        }
        p.finished.set_value(s(&d, "whenFinished"));
        p.quiet.set(false);
    }
    p.update();
    let this = p.clone();
    app.subscribe_while_open(&window, move |c| {
        if matches!(c, Change::List | Change::Progress | Change::Totals) {
            this.update();
        }
    });
    let this = p.clone();
    app.every_second_while_open(&window, move || this.tick());
    window
}

impl Progress {
    fn row(&self) -> Option<Row> {
        odm().row(&self.id)
    }

    fn update(&self) {
        let Some(row) = self.row() else {
            self.window.set_title(Some("Download"));
            self.name.set_label("This download no longer exists.");
            return;
        };
        let d = row.data();
        let status = s(&d, "status");
        let complete = status == "complete";
        let pct = fmt::percent(n(&d, "receivedBytes"), opt_n(&d, "totalBytes"), complete);
        self.window.set_title(Some(&format!("{pct}% {}", s(&d, "fileName"))));
        self.icon.set_from_gicon(&crate::row::file_icon(s(&d, "fileName")));
        self.name.set_label(s(&d, "fileName"));
        let from = d.get("finalUrl").and_then(Value::as_str).unwrap_or_else(|| s(&d, "url"));
        self.place.set_label(&format!("{} · {}", fmt::host(from), s(&d, "saveDir")));
        self.status.set_label(&row.status());
        widgets::paint_status(&self.status, status);
        self.percent.set_label(&if complete { "✓ Complete".to_string() } else { format!("{pct}%") });
        let texts = [
            format!("{} / {}", fmt::bytes(Some(n(&d, "receivedBytes"))), fmt::bytes(opt_n(&d, "totalBytes"))),
            row.speed(),
            row.eta(),
            if b(&d, "resumable") { "Supported".to_string() } else { "Not supported".to_string() },
        ];
        for (l, t) in self.stats.iter().zip(texts) {
            l.set_label(&t);
        }
        let err = d.get("error").and_then(Value::as_str).filter(|_| status == "error");
        self.error.set_visible(err.is_some());
        self.error.set_label(err.unwrap_or(""));
        self.verified.set_visible(d.get("checksumOk").and_then(Value::as_bool) == Some(true));

        let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        segs.sort_by_key(|x| n(x, "start"));
        self.seg_title.set_label(&format!("Segment map · {} connections", (n(&d, "activeConnections") as usize).max(segs.len())));
        self.segments.set(&d);
        self.spark.set(&d);
        self.fill_connections(&segs);

        let active = fmt::is_active(status) || status == "queued";
        self.hide.set_label(if complete { "Close" } else { "Hide" });
        self.cancel.set_visible(!complete);
        self.toggle.set_visible(!complete);
        self.toggle.set_label(if active {
            "Pause"
        } else if status == "error" {
            "Retry"
        } else {
            "Resume"
        });
        self.reveal.set_visible(complete);
        self.open.set_visible(complete);

        if !complete {
            self.saw_incomplete.set(true);
            *self.close_at.borrow_mut() = None;
            self.closing.set_label("");
        } else if self.saw_incomplete.replace(false) {
            // finished while open: close by itself in a moment
            *self.close_at.borrow_mut() = Some(Instant::now() + Duration::from_secs(5));
            self.tick();
        }
    }

    /// One line per connection, reusing the rows that are there.
    fn fill_connections(&self, segs: &[&Value]) {
        let mut rows = self.rows.borrow_mut();
        while rows.len() > segs.len() {
            if let Some((row, _)) = rows.pop() {
                self.connections.remove(&row);
            }
        }
        while rows.len() < segs.len() {
            let row = adw::ActionRow::new();
            let speed = gtk::Label::new(None);
            speed.add_css_class("numeric");
            row.add_suffix(&speed);
            self.connections.append(&row);
            rows.push((row, speed));
        }
        for (i, (seg, (row, speed))) in segs.iter().zip(rows.iter()).enumerate() {
            let state = s(seg, "state");
            let size = (n(seg, "end") < u64::MAX / 2).then(|| n(seg, "end").saturating_sub(n(seg, "start")));
            let received = fmt::bytes(Some(n(seg, "received")));
            row.set_title(&format!("{:02}  {}", i + 1, state_label(state)));
            row.set_subtitle(&match size {
                Some(sz) => format!("{received} / {}", fmt::bytes(Some(sz))),
                None => received,
            });
            speed.set_label(&if state == "receiving" { fmt::speed(n(seg, "speedBps")) } else { fmt::DASH.to_string() });
        }
    }

    fn tick(&self) {
        let at = *self.close_at.borrow();
        let Some(at) = at else { return };
        let left = at.saturating_duration_since(Instant::now()).as_secs();
        if left == 0 && !self.hovering.get() {
            self.window.close();
        } else {
            self.closing.set_label(&format!("Closing in {left}s"));
        }
    }

    fn apply_limit(&self) {
        self.limit_error.set_visible(false);
        if !self.limit_on.is_active() {
            return odm().act("set_download_limit", json!({ "id": self.id, "bps": null }));
        }
        // reject NaN / inf / 0 instead of silently turning the limit off
        match fmt::limit_bps(&self.limit.text()) {
            Some(bps) => odm().act("set_download_limit", json!({ "id": self.id, "bps": bps })),
            None => {
                self.limit_error.set_label(fmt::LIMIT_ERROR);
                self.limit_error.set_visible(true);
            }
        }
    }
}
