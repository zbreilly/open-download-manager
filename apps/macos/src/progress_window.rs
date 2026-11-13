//! The per-download progress window.
use crate::app::app;
use crate::charts::{self, Shown};
use crate::kit::{self, mtm, ns, Canvas, KeyTable, TableCallbacks};
use crate::{engine, system};
use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSPoint;
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

const WHEN_FINISHED: [(&str, &str); 4] = [("notify", "Show notification"), ("openFile", "Open file"), ("openFolder", "Show in Finder"), ("none", "Do nothing")];

pub struct ProgressWindow {
    window: Retained<NSWindow>,
    id: String,
    shown: Shown,
    /// the download was unfinished while this window was open, so the
    /// window closes by itself a few seconds after it completes
    saw_incomplete: Cell<bool>,
    close_at: Cell<Option<Instant>>,
    segments: RefCell<Vec<Value>>,

    icon: Retained<NSImageView>,
    name: Retained<NSTextField>,
    place: Retained<NSTextField>,
    status: Retained<NSTextField>,
    percent: Retained<NSTextField>,
    percent_sign: Retained<NSTextField>,
    complete: Retained<NSTextField>,
    stats: Vec<Retained<NSTextField>>,
    error: Retained<NSTextField>,
    verified: Retained<NSTextField>,
    map_title: Retained<NSTextField>,
    map: Retained<Canvas>,
    table: Retained<KeyTable>,
    peak: Retained<NSTextField>,
    graph: Retained<Canvas>,
    limit_on: Retained<NSButton>,
    limit_row: Retained<NSStackView>,
    limit_text: Retained<NSTextField>,
    limit_error: Retained<NSTextField>,
    when_finished: Retained<NSPopUpButton>,
    done_bar: Retained<NSStackView>,
    closing: Retained<NSTextField>,
    busy_bar: Retained<NSStackView>,
    toggle: Retained<NSButton>,
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

impl ProgressWindow {
    pub fn open(id: &str) -> Rc<ProgressWindow> {
        let this = Rc::new_cyclic(|weak: &Weak<ProgressWindow>| {
            let window = kit::window("Download", 760.0, 660.0, (620.0, 520.0), true);
            let shown: Shown = Rc::new(RefCell::new(Value::Null));

            // header
            let icon = NSImageView::new(mtm());
            kit::width(&icon, 40.0);
            kit::height(&icon, 40.0);
            let name = kit::label("");
            kit::set_font(&name, 16.0, true);
            unsafe { name.setSelectable(true) };
            let place = kit::dim("");
            kit::mono(&place);
            let titles = kit::vstack(2.0, &[&name, &place]);
            kit::stretch(&titles, true);
            let status = kit::label("");
            status.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);
            let header = kit::hstack(12.0, &[&icon, &titles, &status]);

            // the big number and the figures next to it
            let percent = kit::label("0");
            unsafe { percent.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(44.0, NSFontWeightMedium))) };
            let percent_sign = kit::dim("%");
            kit::set_font(&percent_sign, 20.0, false);
            let complete = kit::label("Complete");
            kit::set_font(&complete, 28.0, true);
            unsafe { complete.setTextColor(Some(&NSColor::systemGreenColor())) };
            let stats: Vec<Retained<NSTextField>> = (0..4).map(|_| kit::label("")).collect();
            let mut columns: Vec<Retained<NSStackView>> = vec![];
            for (title, value) in ["DOWNLOADED", "SPEED", "TIME LEFT", "RESUME"].iter().zip(&stats) {
                let caption = kit::dim(title);
                unsafe { caption.setFont(Some(&NSFont::systemFontOfSize_weight(10.0, NSFontWeightSemibold))) };
                kit::digits(value);
                columns.push(kit::vstack(2.0, &[&caption, value]));
            }
            unsafe { stats[1].setTextColor(Some(&NSColor::controlAccentColor())) };
            let column_refs: Vec<&NSView> = columns.iter().map(|c| &***c as &NSView).collect();
            let figures = kit::hstack(24.0, &column_refs);
            let hero = kit::hstack(4.0, &[&percent, &percent_sign, &complete, &kit::spacer(), &figures]);
            unsafe {
                hero.setAlignment(NSLayoutAttribute::LastBaseline);
                hero.setCustomSpacing_afterView(20.0, &percent_sign);
            }

            let error = kit::wrapping("");
            kit::red(&error);
            kit::mono(&error);
            unsafe { error.setSelectable(true) };
            let verified = kit::label("✓ Checksum verified");
            unsafe { verified.setTextColor(Some(&NSColor::systemGreenColor())) };

            // segment map
            let map_title = kit::note("");
            let map_head = kit::hstack(6.0, &[&map_title, &kit::spacer(), &kit::note("Faded = segment done · light edge = write position")]);
            let map = charts::segment_map(&shown, true);
            kit::height(&map, 26.0);

            // connections
            let (table_scroll, table) = kit::table(
                &[("number", "#", 34.0, 30.0, false), ("received", "Received", 170.0, 100.0, false), ("state", "State", 90.0, 70.0, false), ("speed", "Speed", 110.0, 90.0, false)],
                22.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.segments.borrow().len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |table, column, row| w.upgrade().and_then(|w| w.connection_cell(table, column, row))
                    }),
                    ..Default::default()
                },
            );
            unsafe {
                table.setAllowsEmptySelection(true);
                table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
                table_scroll.setBorderType(NSBorderType::BezelBorder);
            }
            kit::min_height(&table_scroll, 160.0);

            // speed graph, limit, what to do afterwards
            let peak = kit::note("");
            let graph_head = kit::hstack(6.0, &[&kit::note("Speed, last 60 s"), &kit::spacer(), &peak]);
            let graph = charts::sparkline(&shown);
            kit::height(&graph, 80.0);
            let limit_on = kit::checkbox("Limit this download", kit::bound(weak, |w| w.apply_limit()));
            let limit_text = kit::text_field("MB/s", || {});
            kit::set_text(&limit_text, "5");
            kit::width(&limit_text, 80.0);
            kit::on_action(&limit_text, kit::bound(weak, |w| w.apply_limit()));
            let apply = kit::button("Apply", kit::bound(weak, |w| w.apply_limit()));
            let limit_row = kit::hstack(8.0, &[&limit_text, &kit::dim("MB/s"), &apply]);
            let limit_error = kit::note("");
            kit::red(&limit_error);
            let limit_box = kit::vstack(6.0, &[&limit_on, &limit_row, &limit_error, &kit::note("Overrides the per-download setting.")]);
            let titles: Vec<String> = WHEN_FINISHED.iter().map(|w| w.1.to_string()).collect();
            let when_finished = kit::popup(&titles, kit::bound(weak, |w| w.when_finished_chosen()));
            let when_row = kit::hstack(8.0, &[&kit::label("When finished"), &when_finished]);
            let side = kit::vfill(10.0, &[&graph_head, &graph, &limit_box, &when_row]);
            kit::width(&side, 290.0);
            let lower = kit::hstack(16.0, &[&table_scroll, &side]);
            unsafe { lower.setAlignment(NSLayoutAttribute::Top) };
            table_scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
            unsafe {
                table_scroll.heightAnchor().constraintEqualToAnchor(&lower.heightAnchor()).setActive(true);
                table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::UniformColumnAutoresizingStyle);
                table_scroll.setHasHorizontalScroller(false);
            }
            kit::stretch(&lower, false);

            let body = kit::vfill_in(14.0, (20.0, 20.0, 16.0, 20.0), &[&header, &hero, &error, &verified, &kit::vfill(6.0, &[&map_head, &map]), &lower]);

            // footer: finished
            let close = kit::button("Close", kit::bound(weak, |w| w.window.performClose(None)));
            let closing = kit::note("");
            let reveal = kit::button("Show in Finder", kit::bound(weak, |w| system::reveal_download(&w.shown.borrow())));
            let open = kit::button("Open", kit::bound(weak, |w| {
                let d = w.shown.borrow().clone();
                app().open_file(&d);
                w.window.performClose(None);
            }));
            let done_bar = kit::hstack(10.0, &[&close, &closing, &kit::spacer(), &reveal, &open]);
            kit::pad(&done_bar, 12.0, 14.0, 12.0, 14.0);

            // footer: unfinished
            let hide = kit::button("Hide", kit::bound(weak, |w| w.window.performClose(None)));
            kit::cancel_button(&hide);
            let cancel = kit::button("Cancel Download", kit::bound(weak, |w| w.cancel_download()));
            unsafe { cancel.setHasDestructiveAction(true) };
            let toggle = kit::button("Pause", kit::bound(weak, |w| {
                let d = w.shown.borrow().clone();
                let id = [w.id.clone()];
                if fmt::is_active(s(&d, "status")) || s(&d, "status") == "queued" {
                    app().pause(&id);
                } else {
                    app().resume(&id);
                }
            }));
            kit::default_button(&toggle);
            let busy_bar = kit::hstack(10.0, &[&hide, &kit::spacer(), &cancel, &toggle]);
            kit::pad(&busy_bar, 12.0, 14.0, 12.0, 14.0);

            kit::stretch(&body, false);
            let root = kit::vfill(0.0, &[&body, &kit::hline(), &done_bar, &busy_bar]);
            unsafe { window.setContentView(Some(&root)) };

            kit::on_close(&window, {
                let weak = weak.clone();
                move || {
                    if let Some(w) = weak.upgrade() {
                        let id = w.id.clone();
                        // after the close has finished: this drops the window
                        kit::after(0.0, move || app().progress_closed(&id));
                    }
                }
            });

            ProgressWindow {
                window,
                id: id.to_string(),
                shown,
                saw_incomplete: Cell::new(false),
                close_at: Cell::new(None),
                segments: RefCell::new(vec![]),
                icon,
                name,
                place,
                status,
                percent,
                percent_sign,
                complete,
                stats,
                error,
                verified,
                map_title,
                map,
                table,
                peak,
                graph,
                limit_on,
                limit_row,
                limit_text,
                limit_error,
                when_finished,
                done_bar,
                closing,
                busy_bar,
                toggle,
            }
        });

        // what the download has now
        let d = app().store.borrow().get(id).cloned().unwrap_or(Value::Null);
        this.saw_incomplete.set(s(&d, "status") != "complete");
        if let Some(limit) = opt_n(&d, "speedLimitBps").filter(|l| *l > 0) {
            kit::set_on(&this.limit_on, true);
            kit::set_text(&this.limit_text, &fmt::mbps(limit));
        }
        this.refresh();
        this
    }

    pub fn show(&self) {
        kit::present(&self.window);
    }

    fn connection_cell(&self, table: &NSTableView, column: &str, row: usize) -> Option<Retained<NSView>> {
        let segments = self.segments.borrow();
        let seg = segments.get(row)?;
        let state = s(seg, "state");
        let (cell, label, _) = kit::text_cell(table, column, false);
        let size = opt_n(seg, "end").filter(|e| *e < u64::MAX / 2).map(|e| e.saturating_sub(n(seg, "start")));
        let text = match column {
            "number" => format!("{:02}", row + 1),
            "received" => match size {
                Some(size) => format!("{} / {}", fmt::bytes(Some(n(seg, "received"))), fmt::bytes(Some(size))),
                None => fmt::bytes(Some(n(seg, "received"))),
            },
            "state" => state_label(state).to_string(),
            _ if state == "receiving" => fmt::speed(n(seg, "speedBps")),
            _ => fmt::DASH.to_string(),
        };
        let color = unsafe {
            match (column, state) {
                ("number", _) => NSColor::secondaryLabelColor(),
                ("state", "reconnecting") => NSColor::systemOrangeColor(),
                ("state", "done") => NSColor::systemGreenColor(),
                ("state", "receiving") => NSColor::labelColor(),
                ("state", _) => NSColor::secondaryLabelColor(),
                _ => NSColor::labelColor(),
            }
        };
        unsafe {
            label.setTextColor(Some(&color));
            label.setAlignment(if column == "speed" { NSTextAlignment::Right } else { NSTextAlignment::Left });
        }
        if column != "state" {
            kit::digits(&label);
        }
        kit::set_text(&label, &text);
        Some(kit::view(&cell))
    }

    /// Show the download as the store has it now.
    pub fn refresh(&self) {
        let Some(d) = app().store.borrow().get(&self.id).cloned() else {
            // deleted from the list: nothing left to show
            return self.window.performClose(None);
        };
        let status = s(&d, "status").to_string();
        let was = s(&self.shown.borrow(), "status").to_string();
        let complete = status == "complete";
        if !complete {
            self.saw_incomplete.set(true);
            self.close_at.set(None);
        } else if was != "complete" && self.saw_incomplete.get() {
            self.close_at.set(Some(Instant::now() + Duration::from_secs(5)));
        }
        *self.shown.borrow_mut() = d.clone();

        let total = opt_n(&d, "totalBytes");
        let percent = fmt::percent(n(&d, "receivedBytes"), total, complete);
        let name = s(&d, "fileName");
        unsafe {
            self.window.setTitle(&ns(&format!("{percent}% {name}")));
            self.icon.setImage(Some(&system::file_icon(&fmt::extension(name))));
            self.status.setAttributedStringValue(&kit::dotted(&fmt::status_label(&status, d.get("scheduledAt").and_then(Value::as_str)), &kit::status_color(&status)));
        }
        kit::set_text(&self.name, name);
        let address = d.get("finalUrl").and_then(Value::as_str).unwrap_or(s(&d, "url"));
        kit::set_text(&self.place, &format!("{} · {}", fmt::host(address), s(&d, "saveDir")));
        kit::set_text(&self.percent, &percent.to_string());
        kit::show(&self.percent, !complete);
        kit::show(&self.percent_sign, !complete);
        kit::show(&self.complete, complete);
        let active = fmt::is_active(&status);
        let figures = [
            format!("{} / {}", fmt::bytes(Some(n(&d, "receivedBytes"))), fmt::bytes(total)),
            if active { fmt::speed(n(&d, "speedBps")) } else { fmt::DASH.into() },
            if status == "downloading" { fmt::eta(opt_n(&d, "etaSecs")) } else { fmt::DASH.into() },
            if b(&d, "resumable") { "Supported".into() } else { "Not supported".to_string() },
        ];
        for (label, text) in self.stats.iter().zip(&figures) {
            kit::set_text(label, text);
        }
        let error = d.get("error").and_then(Value::as_str).filter(|_| status == "error");
        // a finished stream download may carry a note (audio saved separately)
        let notice = d.get("notice").and_then(Value::as_str).filter(|t| !t.is_empty() && error.is_none());
        kit::show(&self.error, error.is_some() || notice.is_some());
        kit::set_text(&self.error, error.or(notice).unwrap_or(""));
        unsafe {
            self.error.setTextColor(Some(&*if error.is_some() { NSColor::systemRedColor() } else { NSColor::secondaryLabelColor() }));
        }
        kit::show(&self.verified, d.get("checksumOk").and_then(Value::as_bool) == Some(true));

        let mut segments: Vec<Value> = d.get("segments").and_then(Value::as_array).cloned().unwrap_or_default();
        segments.sort_by_key(|g| n(g, "start"));
        let connections = n(&d, "activeConnections").max(segments.len() as u64);
        kit::set_text(&self.map_title, &format!("Segment map · {connections} connections"));
        *self.segments.borrow_mut() = segments;
        self.table.reload();
        self.map.redraw();
        self.graph.redraw();
        let peak = charts::samples(&d).into_iter().max().unwrap_or(0);
        kit::set_text(&self.peak, &format!("peak {}", fmt::speed(peak)));

        kit::show(&self.limit_row, kit::is_on(&self.limit_on));
        kit::show(&self.limit_error, !kit::text_of(&self.limit_error).is_empty());
        if let Some(i) = WHEN_FINISHED.iter().position(|w| w.0 == s(&d, "whenFinished")) {
            unsafe { self.when_finished.selectItemAtIndex(i as isize) };
        }
        kit::show(&self.done_bar, complete);
        kit::show(&self.busy_bar, !complete);
        let title = if active || status == "queued" {
            "Pause"
        } else if status == "error" {
            "Retry"
        } else {
            "Resume"
        };
        unsafe { self.toggle.setTitle(&ns(title)) };
        self.tick();
    }

    /// Once a second: the countdown to closing after the download finished.
    /// The pointer over the window keeps it open.
    pub fn tick(&self) {
        let Some(at) = self.close_at.get() else { return kit::set_text(&self.closing, "") };
        let pointer: NSPoint = unsafe { NSEvent::mouseLocation() };
        let frame = self.window.frame();
        let inside = pointer.x >= frame.origin.x && pointer.x <= frame.origin.x + frame.size.width && pointer.y >= frame.origin.y && pointer.y <= frame.origin.y + frame.size.height;
        if inside && unsafe { self.window.isVisible() } && !kit::quiet() {
            self.close_at.set(None);
            return kit::set_text(&self.closing, "");
        }
        let left = at.saturating_duration_since(Instant::now()).as_secs();
        kit::set_text(&self.closing, &format!("Closing in {left}s"));
        if left == 0 {
            self.close_at.set(None);
            self.window.performClose(None);
        }
    }

    fn apply_limit(&self) {
        kit::set_text(&self.limit_error, "");
        let bps = if kit::is_on(&self.limit_on) {
            // reject NaN / inf / 0 instead of silently turning the limit off
            match fmt::limit_bps(&kit::text_of(&self.limit_text)) {
                Some(bps) => Some(bps),
                None => {
                    kit::set_text(&self.limit_error, fmt::LIMIT_ERROR);
                    return self.refresh();
                }
            }
        } else {
            None
        };
        engine::act("set_download_limit", json!({ "id": self.id, "bps": bps }));
        self.refresh();
    }

    fn when_finished_chosen(&self) {
        if let Some(choice) = WHEN_FINISHED.get(kit::popup_index(&self.when_finished)) {
            engine::act("update_download", json!({ "id": self.id, "patch": { "whenFinished": choice.0 } }));
        }
    }

    fn cancel_download(&self) {
        let text = "The partial file is deleted and the download is removed from the list.";
        if kit::ask(Some(&self.window), "Cancel this download?", text, &["Cancel Download", "Keep Downloading"], Some(0)) == 0 {
            app().delete(std::slice::from_ref(&self.id), false);
            self.window.performClose(None);
        }
    }
}
