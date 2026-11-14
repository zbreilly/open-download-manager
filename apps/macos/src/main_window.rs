//! The main window: sidebar, download list, details panel and status bar.
use crate::app::app;
use crate::charts::{self, Shown};
use crate::kit::{self, item, mtm, ns, separator, submenu, Canvas, KeyTable, TableCallbacks};
use crate::{engine, menus, system};
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use odm_ui::store::{Change, SortKey};
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

enum Entry {
    Header(&'static str),
    /// `(sidebar key, title, SF Symbol)`
    Item(String, String, String),
}

struct State {
    /// the sidebar entry shown ("library:all", "category:video", …)
    filter: String,
    search: String,
    sort: SortKey,
    descending: bool,
    /// the rows of the list, in order
    ids: Vec<String>,
    selection: Vec<String>,
    entries: Vec<Entry>,
    counts: HashMap<String, usize>,
    show_details: bool,
}

struct Details {
    root: Retained<NSStackView>,
    icon: Retained<NSImageView>,
    name: Retained<NSTextField>,
    url: Retained<NSTextField>,
    status: Retained<NSTextField>,
    primary: Retained<NSButton>,
    secondary: Retained<NSButton>,
    error: Retained<NSTextField>,
    values: Vec<Retained<NSTextField>>,
    percent: Retained<NSTextField>,
    peak: Retained<NSTextField>,
    map: Retained<Canvas>,
    graph: Retained<Canvas>,
    shown: Shown,
}

pub struct MainWindow {
    window: Retained<NSWindow>,
    sidebar: Retained<KeyTable>,
    table: Retained<KeyTable>,
    state: RefCell<State>,
    tools: HashMap<&'static str, Retained<NSToolbarItem>>,
    banner: Retained<NSBox>,
    banner_label: Retained<NSTextField>,
    start_error: Retained<NSTextField>,
    empty: Retained<NSStackView>,
    empty_label: Retained<NSTextField>,
    empty_hint: Retained<NSTextField>,
    details: Details,
    details_scroll: Retained<NSScrollView>,
    summary: Retained<NSTextField>,
    speed: Retained<NSTextField>,
    limit: Retained<NSTextField>,
    bridge: Retained<NSTextField>,
    free: Retained<NSTextField>,
}

fn category_symbol(icon: &str) -> &'static str {
    match icon {
        "cat-video" => "film",
        "cat-music" => "music.note",
        "cat-documents" => "doc.text",
        "cat-compressed" => "archivebox",
        "cat-programs" => "app.badge",
        "cat-images" => "photo",
        "file-disc" => "opticaldisc",
        "cat-grabber" => "globe",
        _ => "doc",
    }
}

fn sort_key(column: &str) -> SortKey {
    match column {
        "name" => SortKey::Name,
        "size" => SortKey::Size,
        "progress" => SortKey::Progress,
        "speed" => SortKey::Speed,
        "eta" => SortKey::Eta,
        "status" => SortKey::Status,
        "queue" => SortKey::Queue,
        _ => SortKey::Added,
    }
}

/// The cell of the Progress column: a bar and a percentage.
fn progress_cell(table: &NSTableView, percent: u32) -> Retained<NSView> {
    let (cell, text, _) = kit::text_cell(table, "progress", false);
    let existing = unsafe { cell.subviews() }.iter().find_map(|v| v.downcast::<NSProgressIndicator>().ok());
    let bar = existing.unwrap_or_else(|| {
        let bar = NSProgressIndicator::new(mtm());
        unsafe {
            bar.setStyle(NSProgressIndicatorStyle::Bar);
            bar.setIndeterminate(false);
            bar.setControlSize(NSControlSize::Small);
            bar.setMinValue(0.0);
            bar.setMaxValue(100.0);
            bar.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&bar);
            bar.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true);
            bar.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            bar.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -46.0).setActive(true);
            text.setAlignment(NSTextAlignment::Right);
            text.widthAnchor().constraintEqualToConstant(40.0).setActive(true);
        }
        kit::digits(&text);
        bar
    });
    unsafe { bar.setDoubleValue(percent as f64) };
    kit::set_text(&text, &format!("{percent}%"));
    kit::view(&cell)
}

fn plain_cell(table: &NSTableView, kind: &str, text: &str, right: bool, dim: bool) -> Retained<NSView> {
    let (cell, label, _) = kit::text_cell(table, kind, false);
    unsafe {
        label.setAlignment(if right { NSTextAlignment::Right } else { NSTextAlignment::Left });
        let color = if dim { NSColor::secondaryLabelColor() } else { NSColor::labelColor() };
        label.setTextColor(Some(&color));
    }
    if right {
        kit::digits(&label);
    }
    kit::set_text(&label, text);
    kit::view(&cell)
}

impl Details {
    fn new(window: &Weak<MainWindow>) -> Details {
        let shown: Shown = Rc::new(RefCell::new(Value::Null));
        let icon = NSImageView::new(mtm());
        kit::width(&icon, 36.0);
        kit::height(&icon, 36.0);
        let name = kit::label("");
        kit::set_font(&name, 13.0, true);
        unsafe { name.setSelectable(true) };
        let url = kit::dim("");
        kit::mono(&url);
        unsafe { url.setSelectable(true) };
        let titles = kit::vstack(2.0, &[&name, &url]);
        kit::stretch(&titles, true);
        let status = kit::label("");
        status.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);

        let primary = kit::button("Pause", {
            let shown = shown.clone();
            move || {
                let d = shown.borrow().clone();
                let id = [s(&d, "id").to_string()];
                match s(&d, "status") {
                    "complete" => app().open_file(&d),
                    "error" | "paused" => app().resume(&id),
                    _ => app().pause(&id),
                }
            }
        });
        let secondary = kit::button("Refresh Link…", {
            let shown = shown.clone();
            move || refresh_link(&shown.borrow().clone())
        });
        let reveal = kit::button("Show in Finder", {
            let shown = shown.clone();
            move || system::reveal_download(&shown.borrow())
        });
        let copy = kit::button("Copy Link", {
            let shown = shown.clone();
            move || system::copy(s(&shown.borrow(), "url"))
        });
        for b in [&primary, &secondary, &reveal, &copy] {
            b.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);
        }
        let head = kit::hstack(10.0, &[&icon, &titles, &status, &secondary, &primary, &reveal, &copy]);

        let error = unsafe { NSTextField::wrappingLabelWithString(&ns(""), mtm()) };
        unsafe { error.setTextColor(Some(&NSColor::systemRedColor())) };
        kit::mono(&error);

        let values: Vec<Retained<NSTextField>> = (0..6).map(|_| kit::label("")).collect();
        for (i, v) in values.iter().enumerate() {
            unsafe { v.setSelectable(true) };
            if i != 4 {
                kit::mono(v);
            }
        }
        let labels = ["Downloaded", "Speed", "Time left", "Connections", "Resume", "Save to"];
        let rows: Vec<(&str, &NSView)> = labels.iter().zip(&values).map(|(l, v)| (*l, &***v as &NSView)).collect();
        let grid = kit::form(&rows);
        unsafe {
            grid.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Leading);
            grid.setRowSpacing(5.0);
        }
        kit::width(&grid, 380.0);

        let percent = kit::dim("");
        kit::digits(&percent);
        let peak = kit::dim("");
        kit::digits(&peak);
        let map = charts::segment_map(&shown, false);
        kit::height(&map, 18.0);
        let graph = charts::sparkline(&shown);
        kit::height(&graph, 52.0);
        let segments_head = kit::hstack(6.0, &[&kit::dim("Segments"), &kit::spacer(), &percent]);
        let speed_head = kit::hstack(6.0, &[&kit::dim("Speed, last 60 s"), &kit::spacer(), &peak]);
        let charts_column = kit::vfill(6.0, &[&segments_head, &map, &speed_head, &graph]);
        let body = kit::hstack(24.0, &[&grid, &charts_column]);
        unsafe { body.setAlignment(NSLayoutAttribute::Top) };

        let root = kit::vfill_in(12.0, (14.0, 16.0, 14.0, 16.0), &[&head, &error, &body]);
        let _ = window;
        Details { root, icon, name, url, status, primary, secondary, error, values, percent, peak, map, graph, shown }
    }

    fn show(&self, d: &Value) {
        let status = s(d, "status");
        let total = opt_n(d, "totalBytes");
        *self.shown.borrow_mut() = d.clone();
        unsafe {
            self.icon.setImage(Some(&system::file_icon(&fmt::extension(s(d, "fileName")))));
            self.status.setAttributedStringValue(&kit::dotted(&fmt::status_label(status, d.get("scheduledAt").and_then(Value::as_str)), &kit::status_color(status)));
        }
        kit::set_text(&self.name, s(d, "fileName"));
        kit::set_text(&self.url, s(d, "url"));
        let title = match status {
            "complete" => "Open",
            "error" => "Retry",
            "paused" => "Resume",
            _ => "Pause",
        };
        unsafe { self.primary.setTitle(&ns(title)) };
        kit::show(&self.secondary, status == "error");
        let error = d.get("error").and_then(Value::as_str).filter(|_| status == "error");
        // a finished stream download may carry a note (audio saved separately)
        let notice = d.get("notice").and_then(Value::as_str).filter(|t| !t.is_empty() && error.is_none());
        kit::show(&self.error, error.is_some() || notice.is_some());
        kit::set_text(&self.error, error.or(notice).unwrap_or(""));
        unsafe {
            self.error.setTextColor(Some(&*if error.is_some() { NSColor::systemRedColor() } else { NSColor::secondaryLabelColor() }));
        }
        let active = fmt::is_active(status);
        let texts = [
            format!("{} of {}", fmt::bytes(Some(n(d, "receivedBytes"))), fmt::bytes(total)),
            if active { fmt::speed(n(d, "speedBps")) } else { fmt::DASH.into() },
            if status == "downloading" { fmt::eta(opt_n(d, "etaSecs")) } else { fmt::DASH.into() },
            format!("{} active / {} max", n(d, "activeConnections"), n(d, "maxConnections")),
            if b(d, "resumable") { "Supported (byte ranges)".into() } else { "Not supported by server".to_string() },
            s(d, "saveDir").to_string(),
        ];
        for (label, text) in self.values.iter().zip(&texts) {
            kit::set_text(label, text);
        }
        kit::set_text(&self.percent, &format!("{}%", fmt::percent(n(d, "receivedBytes"), total, status == "complete")));
        let peak = charts::samples(d).into_iter().max().unwrap_or(0);
        kit::set_text(&self.peak, &format!("peak {}", fmt::speed(peak)));
        self.map.redraw();
        self.graph.redraw();
    }
}

/// Ask for a fresh address for the same file.
pub fn refresh_link(d: &Value) {
    let text = "Paste a fresh address for the same file. Progress is kept when the server reports the same size.";
    let Some(url) = kit::prompt("Refresh Download Link", text, s(d, "url"), "Update Link") else { return };
    engine::act("refresh_link", json!({ "id": s(d, "id"), "newUrl": url.trim() }));
}

/// Ask for the global speed limit and turn it on.
fn edit_limit() {
    let app = app();
    let mut value = fmt::mbps(n(&app.store.borrow().settings, "globalLimitBps"));
    let mut text = "Caps the combined speed of all downloads, in MB/s.".to_string();
    loop {
        let Some(entered) = kit::prompt("Global Speed Limit", &text, &value, "Turn On Limit") else { return };
        // reject NaN / inf / 0 instead of silently doing nothing
        match fmt::limit_bps(&entered) {
            Some(bps) => return app.set_global_limit(Some(bps)),
            None => {
                value = entered;
                text = fmt::LIMIT_ERROR.to_string();
            }
        }
    }
}

impl MainWindow {
    pub fn new() -> Rc<MainWindow> {
        Rc::new_cyclic(|weak: &Weak<MainWindow>| {
            let window = kit::window(odm_ui::APP_NAME, 1280.0, 820.0, (900.0, 560.0), true);
            unsafe {
                window.setStyleMask(window.styleMask() | NSWindowStyleMask::FullSizeContentView);
                window.setToolbarStyle(NSWindowToolbarStyle::Unified);
            }

            // sidebar
            let (side_scroll, sidebar) = kit::table(
                &[("name", "", 150.0, 100.0, false), ("count", "", 36.0, 30.0, false)],
                26.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.state.borrow().entries.len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |table, column, row| w.upgrade().and_then(|w| w.sidebar_cell(table, column, row))
                    }),
                    is_group: Box::new({
                        let w = weak.clone();
                        move |row| w.upgrade().map(|w| matches!(w.state.borrow().entries.get(row), Some(Entry::Header(_)))).unwrap_or(false)
                    }),
                    selection_changed: Box::new({
                        let w = weak.clone();
                        move || {
                            if let Some(w) = w.upgrade() {
                                w.sidebar_selected();
                            }
                        }
                    }),
                    ..Default::default()
                },
            );
            unsafe {
                sidebar.setStyle(NSTableViewStyle::SourceList);
                sidebar.setFloatsGroupRows(false);
                sidebar.setAllowsEmptySelection(false);
                side_scroll.setDrawsBackground(false);
                side_scroll.setHasHorizontalScroller(false);
                sidebar.setMenu(Some(&kit::dynamic_menu("", {
                    let w = weak.clone();
                    move |menu| {
                        if let Some(w) = w.upgrade() {
                            w.fill_sidebar_menu(menu);
                        }
                    }
                })));
            }

            // download list
            let (table_scroll, table) = kit::table(
                &[
                    ("name", "Name", 320.0, 140.0, true),
                    ("size", "Size", 86.0, 60.0, true),
                    ("progress", "Progress", 160.0, 90.0, true),
                    ("speed", "Speed", 92.0, 60.0, true),
                    ("eta", "ETA", 70.0, 50.0, true),
                    ("status", "Status", 120.0, 80.0, true),
                    ("queue", "Queue", 96.0, 50.0, true),
                    ("added", "Added", 110.0, 70.0, true),
                ],
                24.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.state.borrow().ids.len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |table, column, row| w.upgrade().and_then(|w| w.list_cell(table, column, row))
                    }),
                    selection_changed: Box::new({
                        let w = weak.clone();
                        move || {
                            if let Some(w) = w.upgrade() {
                                w.list_selected();
                            }
                        }
                    }),
                    sort_changed: Box::new({
                        let w = weak.clone();
                        move |column, ascending| {
                            if let Some(w) = w.upgrade() {
                                {
                                    let mut st = w.state.borrow_mut();
                                    st.sort = sort_key(column);
                                    st.descending = !ascending;
                                }
                                w.reload_rows();
                            }
                        }
                    }),
                    ..Default::default()
                },
            );
            unsafe {
                table.setAllowsMultipleSelection(true);
                table.setUsesAlternatingRowBackgroundColors(true);
                table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::NoColumnAutoresizing);
                table.setStyle(NSTableViewStyle::Inset);
                table.setMenu(Some(&kit::dynamic_menu("", {
                    let w = weak.clone();
                    move |menu| {
                        if let Some(w) = w.upgrade() {
                            w.fill_row_menu(menu);
                        }
                    }
                })));
                // a double click opens the file or the progress window
                let open = kit::Handler::simple({
                    let w = weak.clone();
                    move || {
                        if let Some(w) = w.upgrade() {
                            w.primary_action();
                        }
                    }
                });
                table.setTarget(Some(&open));
                table.setDoubleAction(Some(sel!(fire:)));
                kit::keep(&table, &open);
            }
            table.on_key({
                let w = weak.clone();
                move |code| {
                    let Some(w) = w.upgrade() else { return false };
                    match code {
                        kit::KEY_DELETE | kit::KEY_FORWARD_DELETE => {
                            app().ask_delete(&w.selected_ids());
                            true
                        }
                        kit::KEY_RETURN => {
                            w.primary_action();
                            true
                        }
                        _ => false,
                    }
                }
            });

            // nothing to show
            let empty_label = kit::dim("No downloads yet");
            let empty_hint = kit::note("Or drag a link from your browser onto this window.");
            let empty_icon = NSImageView::new(mtm());
            unsafe {
                empty_icon.setImage(kit::symbol("arrow.down.circle", "").as_deref());
                empty_icon.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(40.0, NSFontWeightLight)));
                empty_icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            }
            let empty_add = kit::button("Add URL…", || app().open_add(json!({})));
            let empty = kit::vstack(10.0, &[&empty_icon, &empty_label, &empty_add, &empty_hint]);
            unsafe { empty.setAlignment(NSLayoutAttribute::CenterX) };
            let list_area = NSView::new(mtm());
            kit::fill(&list_area, &table_scroll);
            unsafe {
                empty.setTranslatesAutoresizingMaskIntoConstraints(false);
                list_area.addSubview(&empty);
                empty.centerXAnchor().constraintEqualToAnchor(&list_area.centerXAnchor()).setActive(true);
                empty.centerYAnchor().constraintEqualToAnchor(&list_area.centerYAnchor()).setActive(true);
            }

            // details
            let details = Details::new(weak);
            let details_scroll = kit::scrolling(&details.root);
            unsafe { details_scroll.heightAnchor().constraintEqualToConstant(226.0).setActive(true) };
            kit::stretch_view(&list_area);
            list_area.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let split = kit::vfill(0.0, &[&list_area, &kit::hline(), &details_scroll]);
            kit::stretch(&split, false);

            // the countdown before sleep, shutdown or quit
            let banner_label = kit::label("");
            let banner_row = kit::hstack(10.0, &[&banner_label, &kit::spacer(), &kit::button("Cancel", || app().cancel_power())]);
            kit::pad(&banner_row, 6.0, 14.0, 6.0, 14.0);
            let banner = NSBox::new(mtm());
            unsafe {
                banner.setBoxType(NSBoxType::Custom);
                banner.setBorderWidth(0.0);
                banner.setCornerRadius(0.0);
                banner.setFillColor(&NSColor::systemOrangeColor().colorWithAlphaComponent(0.18));
                banner.setContentViewMargins(NSSize::new(0.0, 0.0));
                banner.setContentView(Some(&banner_row));
                banner.setHidden(true);
            }
            let start_error = unsafe { NSTextField::wrappingLabelWithString(&ns(""), mtm()) };
            unsafe {
                start_error.setTextColor(Some(&NSColor::systemRedColor()));
                start_error.setHidden(true);
            }

            // status bar
            let summary = kit::dim("");
            let speed = kit::dim("");
            kit::digits(&speed);
            let limit = kit::dim("");
            let bridge = kit::dim("");
            let free = kit::dim("");
            kit::digits(&free);
            let status_bar = kit::hstack(18.0, &[&summary, &speed, &limit, &bridge, &kit::spacer(), &free]);
            kit::pad(&status_bar, 5.0, 14.0, 5.0, 14.0);
            for l in [&summary, &speed, &limit, &bridge, &free] {
                unsafe { l.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize()))) };
            }

            let column = kit::vfill(0.0, &[&banner, &start_error, &split, &kit::hline(), &status_bar]);
            let content = kit::DropView::new(|links| {
                let app = app();
                if links.len() == 1 {
                    app.open_add(json!({ "url": links[0] }));
                } else {
                    let list: Vec<Value> = links.iter().map(|u| json!({ "url": u })).collect();
                    app.open_grabber(Some(json!({ "links": list })));
                }
            });
            unsafe {
                column.setTranslatesAutoresizingMaskIntoConstraints(false);
                content.addSubview(&column);
                let guide = content.safeAreaLayoutGuide();
                column.topAnchor().constraintEqualToAnchor(&guide.topAnchor()).setActive(true);
                column.bottomAnchor().constraintEqualToAnchor(&content.bottomAnchor()).setActive(true);
                column.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()).setActive(true);
                column.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()).setActive(true);
            }

            // sidebar | content
            let split_controller = NSSplitViewController::new(mtm());
            let side_controller = NSViewController::new(mtm());
            let content_controller = NSViewController::new(mtm());
            unsafe {
                side_controller.setView(&side_scroll);
                content_controller.setView(&content);
                let side_item = NSSplitViewItem::sidebarWithViewController(&side_controller);
                side_item.setMinimumThickness(180.0);
                side_item.setMaximumThickness(300.0);
                side_item.setCanCollapse(false);
                split_controller.addSplitViewItem(&side_item);
                split_controller.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(&content_controller));
                window.setContentViewController(Some(&split_controller));
                window.setContentSize(NSSize::new(1280.0, 820.0));
                window.center();
                window.setFrameAutosaveName(&ns("ODMMainWindow"));
            }

            // toolbar
            let mut tools: HashMap<&'static str, Retained<NSToolbarItem>> = HashMap::new();
            let sel_ids = {
                let w = weak.clone();
                move || w.upgrade().map(|w| w.selected_ids()).unwrap_or_default()
            };
            tools.insert("add", kit::tool("add", "Add URL", "plus", "Add URL (⌘N)", || app().open_add(json!({}))));
            tools.insert("resume", {
                let ids = sel_ids.clone();
                kit::tool("resume", "Resume", "play.fill", "Resume (⌘R)", move || app().resume(&ids()))
            });
            tools.insert("pause", {
                let ids = sel_ids.clone();
                kit::tool("pause", "Pause", "pause.fill", "Pause (⌘.)", move || app().pause(&ids()))
            });
            tools.insert("pauseAll", kit::tool("pauseAll", "Pause All", "pause.circle", "Pause all", || app().pause_all()));
            tools.insert("delete", {
                let ids = sel_ids.clone();
                kit::tool("delete", "Delete", "trash", "Delete (⌘⌫)", move || app().ask_delete(&ids()))
            });
            tools.insert(
                "deleteCompleted",
                kit::tool("deleteCompleted", "Delete Completed", "checkmark.circle.badge.xmark", "Remove completed downloads from the list", || app().delete_completed()),
            );
            let queue_menu = kit::dynamic_menu("", {
                let w = weak.clone();
                move |menu| {
                    let q = w.upgrade().map(|w| w.current_queue()).unwrap_or_else(|| "main".into());
                    let (q1, q2) = (q.clone(), q.clone());
                    menu.addItem(&item("Start Queue", true, move || app().start_queue(&q1)));
                    menu.addItem(&item("Stop Queue", true, move || app().stop_queue(&q2)));
                    menu.addItem(&separator());
                    menu.addItem(&item("Scheduler…", true, || app().open_scheduler()));
                }
            });
            let queue_tool = kit::menu_tool("queue", "Queue", "list.bullet.rectangle", "Start or stop the queue", &queue_menu, {
                let w = weak.clone();
                move || {
                    let Some(w) = w.upgrade() else { return };
                    let q = w.current_queue();
                    let running = app().store.borrow().queue(&q).map(|q| b(q, "running")).unwrap_or(false);
                    if running {
                        app().stop_queue(&q);
                    } else {
                        app().start_queue(&q);
                    }
                }
            });
            tools.insert("queue", Retained::into_super(queue_tool));
            tools.insert("grabber", kit::tool("grabber", "Grabber", "globe", "Site grabber", || app().open_grabber(None)));
            let limit_menu = kit::dynamic_menu("", |menu| {
                for mb in [1u64, 2, 5, 10, 25, 50, 100] {
                    menu.addItem(&item(&format!("{mb} MB/s"), true, move || app().set_global_limit(Some(mb * 1024 * 1024))));
                }
                menu.addItem(&separator());
                menu.addItem(&item("Custom…", true, edit_limit));
                if b(&app().store.borrow().settings, "globalLimitEnabled") {
                    menu.addItem(&separator());
                    menu.addItem(&item("Turn Off Limit", true, || app().set_global_limit(None)));
                }
            });
            let limit_tool = kit::menu_tool("limit", "No Limit", "speedometer", "Speed limit", &limit_menu, || app().toggle_limit());
            tools.insert("limit", Retained::into_super(limit_tool));
            tools.insert("details", {
                let w = weak.clone();
                kit::tool("details", "Details", "rectangle.bottomthird.inset.filled", "Show or hide details", move || {
                    if let Some(w) = w.upgrade() {
                        let show = !w.state.borrow().show_details;
                        w.state.borrow_mut().show_details = show;
                        w.update_details();
                    }
                })
            });
            let search = unsafe { NSSearchToolbarItem::initWithItemIdentifier(NSSearchToolbarItem::alloc(mtm()), &ns("search")) };
            let field = unsafe { search.searchField() };
            unsafe { field.setPlaceholderString(Some(&ns("Search downloads, URLs, folders"))) };
            kit::on_text(&field, {
                let w = weak.clone();
                let field = ObjcWeak::from_retained(&field);
                move || {
                    if let (Some(w), Some(field)) = (w.upgrade(), field.load()) {
                        w.state.borrow_mut().search = kit::text_of(&field);
                        w.reload_rows();
                    }
                }
            });
            tools.insert("search", Retained::into_super(search));
            let flexible = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.to_string();
            let tracking = unsafe { NSToolbarSidebarTrackingSeparatorItemIdentifier }.to_string();
            let order = [
                tracking.as_str(),
                "add",
                flexible.as_str(),
                "resume",
                "pause",
                "pauseAll",
                "delete",
                "deleteCompleted",
                "queue",
                "grabber",
                "limit",
                "details",
                "search",
            ];
            let items: Vec<(String, Retained<NSToolbarItem>)> = tools.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            unsafe { window.setToolbar(Some(&kit::toolbar("ODMMainToolbar", &order, items))) };

            kit::on_close(&window, || kit::after(0.0, || app().main_closed()));

            MainWindow {
                window,
                sidebar,
                table,
                state: RefCell::new(State {
                    filter: "library:all".into(),
                    search: String::new(),
                    sort: SortKey::Added,
                    descending: true,
                    ids: vec![],
                    selection: vec![],
                    entries: vec![],
                    counts: HashMap::new(),
                    show_details: true,
                }),
                tools,
                banner,
                banner_label,
                start_error,
                empty,
                empty_label,
                empty_hint,
                details,
                details_scroll,
                summary,
                speed,
                limit,
                bridge,
                free,
            }
        })
    }

    pub fn window(&self) -> &NSWindow {
        &self.window
    }

    pub fn show(&self) {
        // the first time: fill everything in
        if self.state.borrow().entries.is_empty() {
            unsafe {
                let newest_first = NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&ns("added")), false);
                self.table.setSortDescriptors(&NSArray::from_retained_slice(&[newest_first]));
            }
            self.refresh(Change::List);
            self.power_changed();
            kit::present(&self.window);
            // once laid out: give the details panel the room it needs
            return;
        }
        kit::present(&self.window);
    }

    // -- data ----------------------------------------------------------------

    fn current_queue(&self) -> String {
        self.state.borrow().filter.strip_prefix("queue:").unwrap_or("main").to_string()
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.state.borrow().selection.clone()
    }

    /// The selected downloads, as the store has them now.
    pub fn selected(&self) -> Vec<Value> {
        let app = app();
        let st = app.store.borrow();
        self.state.borrow().selection.iter().filter_map(|id| st.get(id).cloned()).collect()
    }

    pub fn refresh(&self, change: Change) {
        match change {
            Change::List | Change::Queues | Change::Settings => {
                self.reload_sidebar();
                self.reload_rows();
            }
            Change::Progress => {
                if self.state.borrow().sort.is_live() {
                    self.reload_rows();
                } else {
                    self.reload_visible();
                }
            }
            _ => {}
        }
        self.update_details();
        self.update_status();
        self.update_tools();
    }

    fn reload_sidebar(&self) {
        let app = app();
        let st = app.store.borrow();
        let mut entries = vec![
            Entry::Header("Library"),
            Entry::Item("library:all".into(), "All Downloads".into(), "tray.full".into()),
            Entry::Item("library:active".into(), "Active".into(), "arrow.down.circle".into()),
            Entry::Item("library:unfinished".into(), "Unfinished".into(), "circle.lefthalf.filled".into()),
            Entry::Item("library:finished".into(), "Finished".into(), "checkmark.circle".into()),
            Entry::Header("Categories"),
        ];
        for c in st.categories() {
            entries.push(Entry::Item(format!("category:{}", s(&c, "id")), s(&c, "name").into(), category_symbol(s(&c, "icon")).into()));
        }
        entries.push(Entry::Header("Queues"));
        for q in &st.queues {
            let symbol = if b(q, "running") {
                "play.circle.fill"
            } else if b(q, "builtin") {
                "list.bullet"
            } else {
                "clock"
            };
            entries.push(Entry::Item(format!("queue:{}", s(q, "id")), s(q, "name").into(), symbol.into()));
        }
        let counts = st.counts();
        drop(st);
        let selected = {
            let mut state = self.state.borrow_mut();
            // the entry that was showing is gone (a deleted queue or category)
            if !entries.iter().any(|e| matches!(e, Entry::Item(k, _, _) if *k == state.filter)) {
                state.filter = "library:all".into();
            }
            state.counts = counts;
            state.entries = entries;
            state.entries.iter().position(|e| matches!(e, Entry::Item(k, _, _) if *k == state.filter))
        };
        self.sidebar.reload();
        if let Some(row) = selected {
            self.sidebar.select(&[row]);
        }
    }

    fn sidebar_cell(&self, table: &NSTableView, column: &str, row: usize) -> Option<Retained<NSView>> {
        let state = self.state.borrow();
        match state.entries.get(row)? {
            Entry::Header(title) => {
                let (cell, label, _) = kit::text_cell(table, "header", false);
                kit::set_text(&label, title);
                unsafe {
                    label.setFont(Some(&NSFont::systemFontOfSize_weight(NSFont::smallSystemFontSize(), NSFontWeightSemibold)));
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                }
                Some(kit::view(&cell))
            }
            Entry::Item(key, title, symbol) => {
                if column == "count" {
                    let count = state.counts.get(key).copied().unwrap_or(0);
                    let text = if count > 0 { count.to_string() } else { String::new() };
                    return Some(plain_cell(table, "count", &text, true, true));
                }
                let (cell, label, image) = kit::text_cell(table, "entry", true);
                kit::set_text(&label, title);
                if let Some(image) = image {
                    unsafe { image.setImage(kit::symbol(symbol, title).as_deref()) };
                }
                Some(kit::view(&cell))
            }
        }
    }

    fn sidebar_selected(&self) {
        if self.sidebar.loading() {
            return;
        }
        let key = {
            let state = self.state.borrow();
            self.sidebar.selected().first().and_then(|row| match state.entries.get(*row) {
                Some(Entry::Item(key, _, _)) => Some(key.clone()),
                _ => None,
            })
        };
        if let Some(key) = key {
            self.state.borrow_mut().filter = key;
            self.reload_rows();
            self.update_details();
            self.update_tools();
        }
    }

    /// Recompute which downloads the list shows, and in what order.
    fn reload_rows(&self) {
        let app = app();
        let (ids, total) = {
            let st = app.store.borrow();
            let state = self.state.borrow();
            let rows = st.visible(&state.filter, &state.search, state.sort, state.descending);
            (rows.iter().map(|d| s(d, "id").to_string()).collect::<Vec<_>>(), st.downloads().len())
        };
        let same = self.state.borrow().ids == ids;
        if same {
            self.reload_visible();
        } else {
            let rows: Vec<usize> = {
                let mut state = self.state.borrow_mut();
                state.selection.retain(|id| ids.contains(id));
                state.ids = ids;
                state.selection.iter().filter_map(|id| state.ids.iter().position(|i| i == id)).collect()
            };
            self.table.reload();
            self.table.select(&rows);
        }
        let state = self.state.borrow();
        kit::show(&self.empty, state.ids.is_empty());
        let text = if total == 0 {
            "No downloads yet"
        } else if state.search.trim().is_empty() {
            "No downloads here"
        } else {
            "No downloads match your search"
        };
        kit::set_text(&self.empty_label, text);
        kit::show(&self.empty_hint, total == 0);
    }

    /// Redraw the rows on screen (their progress moved).
    fn reload_visible(&self) {
        unsafe {
            let range = self.table.rowsInRect(self.table.visibleRect());
            if range.length == 0 {
                return;
            }
            let rows = NSIndexSet::indexSetWithIndexesInRange(range);
            let columns = NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, self.table.numberOfColumns() as usize));
            self.table.reloadDataForRowIndexes_columnIndexes(&rows, &columns);
        }
    }

    fn list_cell(&self, table: &NSTableView, column: &str, row: usize) -> Option<Retained<NSView>> {
        let app = app();
        let st = app.store.borrow();
        let state = self.state.borrow();
        let d = st.get(state.ids.get(row)?)?;
        let r = st.row(d);
        Some(match column {
            "name" => {
                let (cell, label, image) = kit::text_cell(table, "name", true);
                kit::set_text(&label, &r.name);
                unsafe { cell.setToolTip(Some(&ns(&r.name))) };
                if let Some(image) = image {
                    unsafe { image.setImage(Some(&system::file_icon(&fmt::extension(&r.name)))) };
                }
                kit::view(&cell)
            }
            "size" => plain_cell(table, "number", &r.size, true, false),
            "progress" => progress_cell(table, r.percent),
            "speed" => plain_cell(table, "number", &r.speed, true, false),
            "eta" => plain_cell(table, "number", &r.eta, true, false),
            "status" => {
                let (cell, label, _) = kit::text_cell(table, "status", false);
                unsafe { label.setAttributedStringValue(&kit::dotted(&r.status_label, &kit::status_color(&r.status))) };
                kit::view(&cell)
            }
            "queue" => plain_cell(table, "text", &r.queue, false, true),
            _ => plain_cell(table, "text", &r.added, false, true),
        })
    }

    fn list_selected(&self) {
        if self.table.loading() {
            return;
        }
        let rows = self.table.selected();
        {
            let mut state = self.state.borrow_mut();
            state.selection = rows.iter().filter_map(|r| state.ids.get(*r).cloned()).collect();
        }
        self.update_details();
        self.update_tools();
    }

    /// Double click or Return: open a finished file, else its progress.
    fn primary_action(&self) {
        let clicked = unsafe { self.table.clickedRow() };
        let id = {
            let state = self.state.borrow();
            if clicked >= 0 {
                state.ids.get(clicked as usize).cloned()
            } else {
                state.selection.first().cloned()
            }
        };
        let Some(id) = id else { return };
        let app = app();
        let d = app.store.borrow().get(&id).cloned();
        if let Some(d) = d {
            if s(&d, "status") == "complete" {
                app.open_file(&d);
            } else {
                app.open_progress(&id);
            }
        }
    }

    // -- details, status bar, toolbar ----------------------------------------

    fn update_details(&self) {
        let app = app();
        let st = app.store.borrow();
        let state = self.state.borrow();
        let focused = state.selection.first().and_then(|id| st.get(id)).or_else(|| state.ids.first().and_then(|id| st.get(id)));
        match focused.filter(|_| state.show_details) {
            Some(d) => {
                self.details.show(d);
                kit::show(&self.details_scroll, true);
            }
            None => kit::show(&self.details_scroll, false),
        }
    }

    fn update_status(&self) {
        let app = app();
        let st = app.store.borrow();
        let t = &st.totals;
        kit::set_text(&self.summary, &fmt::totals_summary(t));
        kit::set_text(&self.speed, &format!("↓ {}", fmt::speed(n(t, "speedBps"))));
        let limit = if b(t, "limitOn") { format!("Limit {} MB/s", fmt::mbps(n(t, "limitBps"))) } else { "No speed limit".into() };
        kit::set_text(&self.limit, &limit);
        let error = st.ext.get("bridgeError").and_then(Value::as_str);
        let connected = b(&st.ext, "connected");
        let browsers = odm_ui::strings(&st.ext, "browsers");
        let (text, color) = match (error, connected) {
            (Some(_), _) => ("Browser bridge offline: port in use".to_string(), unsafe { NSColor::systemRedColor() }),
            (None, true) if browsers.is_empty() => ("Browser extension connected".to_string(), unsafe { NSColor::systemGreenColor() }),
            (None, true) => (format!("Browser extension connected ({})", browsers.join(", ")), unsafe { NSColor::systemGreenColor() }),
            (None, false) => ("Browser extension not detected".to_string(), unsafe { NSColor::tertiaryLabelColor() }),
        };
        unsafe {
            self.bridge.setAttributedStringValue(&kit::dotted(&text, &color));
            self.bridge.setToolTip(error.map(ns).as_deref());
        }
        let free = opt_n(t, "freeBytes").map(|f| format!("{} free", fmt::bytes(Some(f)))).unwrap_or_default();
        kit::set_text(&self.free, &free);
        let active = n(t, "active");
        let subtitle = if active > 0 { format!("{active} active · ↓ {}", fmt::speed(n(t, "speedBps"))) } else { String::new() };
        unsafe { self.window.setSubtitle(&ns(&subtitle)) };
        let failed = app.start_error.borrow().clone();
        kit::show(&self.start_error, failed.is_some());
        if let Some(e) = failed {
            kit::set_text(&self.start_error, &format!("  The download engine failed to start: {e}"));
        }
    }

    fn update_tools(&self) {
        let items = self.selected();
        let app = app();
        let st = app.store.borrow();
        let set = |name: &str, on: bool| {
            if let Some(t) = self.tools.get(name) {
                unsafe { t.setEnabled(on) };
            }
        };
        set("resume", menus::can_resume(&items));
        set("pause", menus::can_pause(&items));
        set("pauseAll", n(&st.totals, "active") > 0 || n(&st.totals, "queued") > 0);
        set("delete", !items.is_empty());
        set("deleteCompleted", n(&st.totals, "complete") > 0);
        let on = b(&st.settings, "globalLimitEnabled");
        drop(st);
        let label = app.limit_label();
        if let Some(t) = self.tools.get("limit") {
            let tip = if on { format!("Speed limit on: {label}. Click to turn off.") } else { "No speed limit. Click to turn on.".to_string() };
            unsafe {
                t.setLabel(&ns(&label));
                t.setTitle(&ns(&label));
                t.setToolTip(Some(&ns(&tip)));
                t.setImage(kit::symbol(if on { "speedometer" } else { "gauge.with.dots.needle.67percent" }, &label).as_deref());
            }
        }
        if let Some(t) = self.tools.get("queue") {
            let name = app.store.borrow().queue_name(Some(&self.current_queue()));
            unsafe { t.setToolTip(Some(&ns(&format!("Start or stop the {name} queue · the arrow has more")))) };
        }
    }

    /// The countdown before sleep, shutdown or quit.
    pub fn power_changed(&self) {
        let power = app().power.borrow().clone();
        match power {
            Some(p) => {
                let left = p.deadline.saturating_duration_since(std::time::Instant::now()).as_secs();
                let verb = match p.action.as_str() {
                    "sleep" => "Sleeping",
                    "shutdown" => "Shutting down",
                    _ => "Quitting",
                };
                kit::set_text(&self.banner_label, &format!("“{}” finished. {verb} in {left}s.", p.queue));
                kit::show(&self.banner, true);
            }
            None => kit::show(&self.banner, false),
        }
    }

    // -- context menus -------------------------------------------------------

    /// The downloads a right-click acts on: the selection when the click was
    /// inside it, else the row under the pointer.
    fn clicked(&self) -> Vec<String> {
        let row = unsafe { self.table.clickedRow() };
        let state = self.state.borrow();
        match (row >= 0).then(|| state.ids.get(row as usize)).flatten() {
            Some(id) if !state.selection.contains(id) => vec![id.clone()],
            _ => state.selection.clone(),
        }
    }

    fn fill_row_menu(&self, menu: &NSMenu) {
        let ids = self.clicked();
        let app = app();
        let st = app.store.borrow();
        let items: Vec<Value> = ids.iter().filter_map(|id| st.get(id).cloned()).collect();
        let Some(d) = items.first().cloned() else { return };
        let multi = items.len() > 1;
        let status = s(&d, "status").to_string();
        let complete = status == "complete";
        let id = s(&d, "id").to_string();
        let ext = fmt::extension(s(&d, "fileName"));
        let categories: Vec<(String, String)> = st.categories().iter().map(|c| (s(c, "id").to_string(), s(c, "name").to_string())).collect();
        let queues: Vec<(String, String)> = st.queues.iter().map(|q| (s(q, "id").to_string(), s(q, "name").to_string())).collect();
        drop(st);

        let mut list: Vec<Retained<NSMenuItem>> = vec![
            item("Open", !multi && complete, {
                let d = d.clone();
                move || crate::app::app().open_file(&d)
            }),
            item("Show in Finder", !multi, {
                let d = d.clone();
                move || system::reveal_download(&d)
            }),
            item("Show Progress Window", !multi && !complete, {
                let id = id.clone();
                move || crate::app::app().open_progress(&id)
            }),
            separator(),
            item("Resume", menus::can_resume(&items), {
                let ids = ids.clone();
                move || crate::app::app().resume(&ids)
            }),
            item("Pause", menus::can_pause(&items), {
                let ids = ids.clone();
                move || crate::app::app().pause(&ids)
            }),
            item("Redownload", !multi, {
                let id = id.clone();
                move || engine::act("redownload", json!({ "id": id }))
            }),
            item("Refresh Link…", !multi && !complete, {
                let d = d.clone();
                move || refresh_link(&d)
            }),
            item("Copy Link", !multi, {
                let d = d.clone();
                move || system::copy(s(&d, "url"))
            }),
            separator(),
        ];
        let mut to_queue: Vec<Retained<NSMenuItem>> = queues
            .iter()
            .map(|(qid, name)| {
                let (ids, qid) = (ids.clone(), qid.clone());
                item(name, true, move || crate::app::app().move_to_queue(&ids, Some(&qid)))
            })
            .collect();
        to_queue.push(separator());
        to_queue.push(item("Remove from Queue", true, {
            let ids = ids.clone();
            move || crate::app::app().move_to_queue(&ids, None)
        }));
        list.push(submenu("Move to Queue", to_queue));
        list.push(submenu(
            "Category",
            categories
                .iter()
                .map(|(cid, name)| {
                    let (ids, cid) = (ids.clone(), cid.clone());
                    item(name, true, move || crate::app::app().set_category(&ids, &cid))
                })
                .collect(),
        ));
        if !multi && !ext.is_empty() {
            list.push(submenu(
                &format!("Always Put .{ext} Files In"),
                categories
                    .iter()
                    .map(|(cid, name)| {
                        let (ext, cid) = (ext.clone(), cid.clone());
                        item(name, true, move || crate::app::app().assign_extension(&ext, &cid))
                    })
                    .collect(),
            ));
        }
        list.push(separator());
        let title = if multi { format!("Delete {} Downloads…", items.len()) } else { "Delete…".to_string() };
        list.push(item(&title, true, {
            let ids = ids.clone();
            move || crate::app::app().ask_delete(&ids)
        }));
        for i in &list {
            menu.addItem(i);
        }
    }

    fn fill_sidebar_menu(&self, menu: &NSMenu) {
        let row = unsafe { self.sidebar.clickedRow() };
        let key = match (row >= 0).then(|| self.state.borrow().entries.get(row as usize).map(|e| matches!(e, Entry::Item(..)).then(|| e.key()))).flatten().flatten() {
            Some(k) => k,
            None => return,
        };
        if key.starts_with("category:") {
            menu.addItem(&item("Edit File Types & Folders…", true, || app().open_settings()));
            return;
        }
        let Some(id) = key.strip_prefix("queue:").map(String::from) else { return };
        let app = app();
        let st = app.store.borrow();
        let Some(q) = st.queue(&id) else { return };
        let (running, builtin) = (b(q, "running"), b(q, "builtin"));
        drop(st);
        let (a, c, e) = (id.clone(), id.clone(), id.clone());
        menu.addItem(&item("Start", !running, move || crate::app::app().start_queue(&a)));
        menu.addItem(&item("Stop", running, move || crate::app::app().stop_queue(&c)));
        menu.addItem(&item("Edit Schedule…", true, || crate::app::app().open_scheduler()));
        if !builtin {
            menu.addItem(&separator());
            menu.addItem(&item("Delete Queue", true, move || engine::act("delete_queue", json!({ "id": e }))));
        }
    }
}

impl Entry {
    fn key(&self) -> String {
        match self {
            Entry::Item(k, _, _) => k.clone(),
            Entry::Header(_) => String::new(),
        }
    }
}

#[cfg(debug_assertions)]
impl MainWindow {
    /// Select the first row, as a click would (see `ODM_DO`).
    pub fn debug_select(&self) {
        unsafe { self.table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(0), false) };
        eprintln!("selected: {:?}", self.selected_ids());
    }

    /// Build the context menus and report what they hold.
    pub fn debug_menus(&self) {
        let titles = |m: &NSMenu| unsafe { m.itemArray() }.iter().map(|i| format!("{}{}", unsafe { i.title() }, if unsafe { i.isEnabled() } { "" } else { " (off)" })).collect::<Vec<_>>();
        let row = kit::menu("");
        self.fill_row_menu(&row);
        eprintln!("row menu: {:?}", titles(&row));
        let side = kit::menu("");
        self.fill_sidebar_menu(&side);
        eprintln!("sidebar menu: {:?}", titles(&side));
        let enabled: Vec<String> = self.tools.iter().map(|(k, t)| format!("{k}={}", unsafe { t.isEnabled() })).collect();
        eprintln!("tools: {enabled:?}");
    }
}
