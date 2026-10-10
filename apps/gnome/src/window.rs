//! The main window: sidebar (library, categories, queues), the download
//! list, a details panel and a status bar.
use crate::app::{odm, call, App, Change};
use crate::row::{b, n, opt_n, s, Row};
use crate::widgets::{self, SegmentMap, Sparkline};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use odm_linux::fmt;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

const LIMIT_PRESETS_MB: [i32; 7] = [1, 2, 5, 10, 25, 50, 100];

struct Details {
    root: gtk::Box,
    icon: gtk::Image,
    name: gtk::Label,
    url: gtk::Label,
    status: gtk::Label,
    error: gtk::Label,
    primary: gtk::Button,
    retry: gtk::Button,
    values: Vec<gtk::Label>,
    percent: gtk::Label,
    segments: SegmentMap,
    spark: Sparkline,
    /// what the primary button does right now
    primary_action: RefCell<&'static str>,
}

struct Inner {
    view: gtk::ColumnView,
    selection: gtk::MultiSelection,
    filter: gtk::CustomFilter,
    filter_key: RefCell<String>,
    search: RefCell<String>,
    sidebar: gtk::ListBox,
    counts: RefCell<HashMap<String, gtk::Label>>,
    page: adw::NavigationPage,
    details: Details,
    details_on: RefCell<bool>,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    limit_button: gtk::MenuButton,
    status_summary: gtk::Label,
    status_speed: gtk::Label,
    status_limit: gtk::Label,
    status_ext: gtk::Label,
    status_free: gtk::Label,
    power: adw::Banner,
    engine_error: adw::Banner,
    toasts: adw::ToastOverlay,
}

#[derive(Clone)]
pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    inner: Rc<Inner>,
}

fn action(map: &impl IsA<gio::ActionMap>, name: &str, f: impl Fn() + 'static) -> gio::SimpleAction {
    let a = gio::SimpleAction::new(name, None);
    a.connect_activate(move |_, _| f());
    map.add_action(&a);
    a
}

fn text_action(map: &impl IsA<gio::ActionMap>, name: &str, f: impl Fn(String) + 'static) {
    let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
    a.connect_activate(move |_, p| f(p.and_then(|v| v.str().map(String::from)).unwrap_or_default()));
    map.add_action(&a);
}

/// App-wide actions (menus, shortcuts, the tray).
pub fn install_actions(app: &Rc<App>) {
    let g = &app.gtk;
    action(g, "add", || crate::add::open(&odm(), json!({})));
    action(g, "add-batch", || crate::add::open(&odm(), json!({ "batch": true })));
    action(g, "grabber", || crate::grabber::open(&odm(), None));
    action(g, "scheduler", || crate::scheduler::open(&odm()));
    action(g, "preferences", || crate::settings::open(&odm()));
    action(g, "quit", || odm().quit());
    action(g, "pause-all", || odm().act("pause_all", json!({})));
    action(g, "delete-completed", || odm().act("delete_completed", json!({})));
    action(g, "toggle-limit", || odm().toggle_limit());
    action(g, "limit-off", || odm().set_global_limit(None));
    let preset = gio::SimpleAction::new("limit", Some(glib::VariantTy::INT32));
    preset.connect_activate(|_, p| {
        if let Some(mb) = p.and_then(|v| v.get::<i32>()) {
            odm().set_global_limit(Some(mb.max(1) as u64 * 1024 * 1024));
        }
    });
    g.add_action(&preset);
    g.set_accels_for_action("app.add", &["<Control>n"]);
    g.set_accels_for_action("app.add-batch", &["<Control><Shift>n"]);
    g.set_accels_for_action("app.grabber", &["<Control><Shift>g"]);
    g.set_accels_for_action("app.scheduler", &["<Control>j"]);
    g.set_accels_for_action("app.preferences", &["<Control>comma"]);
    g.set_accels_for_action("app.quit", &["<Control>q"]);
    g.set_accels_for_action("app.toggle-limit", &["<Control><Alt>l"]);
    g.set_accels_for_action("win.resume", &["<Control>r"]);
    g.set_accels_for_action("win.pause", &["<Control>period"]);
    g.set_accels_for_action("win.search", &["<Control>f"]);
    g.set_accels_for_action("win.open", &["<Control>o"]);
    g.set_accels_for_action("win.reveal", &["<Control><Shift>r"]);
    g.set_accels_for_action("win.copy-link", &["<Control><Shift>c"]);
}

fn category_icon(icon: &str) -> &'static str {
    match icon {
        "cat-video" => "folder-videos-symbolic",
        "cat-music" => "folder-music-symbolic",
        "cat-documents" => "folder-documents-symbolic",
        "cat-compressed" => "package-x-generic-symbolic",
        "cat-programs" => "application-x-executable-symbolic",
        "cat-images" => "folder-pictures-symbolic",
        "file-disc" => "media-optical-symbolic",
        _ => "folder-symbolic",
    }
}

fn matches(d: &Value, key: &str) -> bool {
    let status = s(d, "status");
    match key.split_once(':') {
        Some(("library", "active")) => fmt::is_active(status),
        Some(("library", "unfinished")) => status != "complete",
        Some(("library", "finished")) => status == "complete",
        Some(("category", c)) => s(d, "category") == c,
        Some(("queue", q)) => d.get("queueId").and_then(Value::as_str) == Some(q),
        _ => true,
    }
}

fn item_of(obj: &glib::Object) -> &gtk::ListItem {
    obj.downcast_ref::<gtk::ListItem>().expect("list item")
}

impl MainWindow {
    pub fn new(app: &Rc<App>) -> Self {
        // ------------------------------------------------------------ list
        let filter = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(Some(app.rows.clone()), Some(filter.clone()));
        let view = gtk::ColumnView::builder().reorderable(true).show_column_separators(false).build();
        let sorted = gtk::SortListModel::new(Some(filtered), view.sorter());
        let selection = gtk::MultiSelection::new(Some(sorted));
        view.set_model(Some(&selection));

        let details = Self::build_details();
        let toasts = adw::ToastOverlay::new();
        let sidebar = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        sidebar.add_css_class("navigation-sidebar");
        let empty = adw::StatusPage::builder().icon_name("folder-download-symbolic").title("No downloads yet").build();
        let add_first = gtk::Button::builder().label("Add URL…").halign(gtk::Align::Center).action_name("app.add").build();
        add_first.add_css_class("pill");
        add_first.add_css_class("suggested-action");
        empty.set_child(Some(&add_first));

        let numeric = |text: &str| {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("caption");
            l.add_css_class("dim-label");
            l.add_css_class("numeric");
            l
        };
        let inner = Rc::new(Inner {
            view: view.clone(),
            selection: selection.clone(),
            filter: filter.clone(),
            filter_key: RefCell::new("library:all".into()),
            search: RefCell::default(),
            sidebar: sidebar.clone(),
            counts: RefCell::default(),
            page: adw::NavigationPage::builder().title("All Downloads").tag("content").build(),
            details,
            details_on: RefCell::new(true),
            stack: gtk::Stack::new(),
            empty,
            limit_button: gtk::MenuButton::builder().tooltip_text("Global speed limit").build(),
            status_summary: numeric(""),
            status_speed: numeric(""),
            status_limit: numeric(""),
            status_ext: numeric(""),
            status_free: numeric(""),
            power: adw::Banner::builder().button_label("Cancel").build(),
            engine_error: adw::Banner::new(""),
            toasts: toasts.clone(),
        });
        let window = adw::ApplicationWindow::builder()
            .application(&app.gtk)
            .title(odm_linux::APP_NAME)
            .default_width(1180)
            .default_height(760)
            .width_request(360)
            .height_request(420)
            .build();
        let me = MainWindow { window: window.clone(), inner: inner.clone() };

        let weak = Rc::downgrade(&inner);
        filter.set_filter_func(move |obj| {
            let (Some(inner), Some(row)) = (weak.upgrade(), obj.downcast_ref::<Row>()) else { return true };
            let key = inner.filter_key.borrow();
            let q = inner.search.borrow();
            row.with(|d| {
                matches(d, &key)
                    && (q.is_empty()
                        || [s(d, "fileName"), s(d, "url"), s(d, "saveDir")].iter().any(|t| t.to_lowercase().contains(q.as_str())))
            })
        });

        me.add_columns();

        // ---------------------------------------------------------- header
        let header = adw::HeaderBar::new();
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add URL (Ctrl+N)").action_name("app.add").build();
        add.add_css_class("suggested-action");
        header.pack_start(&add);
        for (icon, tip, act) in [
            ("media-playback-start-symbolic", "Resume (Ctrl+R)", "win.resume"),
            ("media-playback-pause-symbolic", "Pause (Ctrl+.)", "win.pause"),
            ("user-trash-symbolic", "Delete (Delete)", "win.delete"),
        ] {
            header.pack_start(&gtk::Button::builder().icon_name(icon).tooltip_text(tip).action_name(act).build());
        }
        let menu = gtk::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text("Main Menu").primary(true).build();
        menu.set_menu_model(Some(&Self::primary_menu()));
        header.pack_end(&menu);
        let details_toggle = gtk::ToggleButton::builder()
            .icon_name("view-dual-symbolic")
            .tooltip_text("Show or hide details")
            .active(true)
            .build();
        header.pack_end(&details_toggle);
        let search_toggle = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search (Ctrl+F)").build();
        header.pack_end(&search_toggle);
        inner.limit_button.set_menu_model(Some(&Self::limit_menu(false)));
        header.pack_end(&inner.limit_button);

        let search_entry = gtk::SearchEntry::builder().placeholder_text("Search downloads, URLs, folders").hexpand(true).build();
        let search_bar = gtk::SearchBar::builder().child(&adw::Clamp::builder().maximum_size(480).child(&search_entry).build()).build();
        search_bar.connect_entry(&search_entry);
        search_bar.set_key_capture_widget(Some(&window));
        search_toggle.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().build();
        let weak = Rc::downgrade(&inner);
        search_entry.connect_search_changed(move |e| {
            let Some(inner) = weak.upgrade() else { return };
            *inner.search.borrow_mut() = e.text().trim().to_lowercase();
            inner.filter.changed(gtk::FilterChange::Different);
            MainWindow::refresh_empty(&inner);
        });

        // --------------------------------------------------------- content
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hscrollbar_policy(gtk::PolicyType::Automatic).build();
        inner.stack.add_named(&scroller, Some("list"));
        inner.stack.add_named(&inner.empty, Some("empty"));
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&inner.stack)
            .end_child(&inner.details.root)
            .resize_start_child(true)
            .resize_end_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .vexpand(true)
            .build();
        let details_root = inner.details.root.clone();
        let weak = Rc::downgrade(&inner);
        details_toggle.connect_toggled(move |t| {
            if let Some(inner) = weak.upgrade() {
                *inner.details_on.borrow_mut() = t.is_active();
            }
            details_root.set_visible(t.is_active());
            odm().emit(Change::Progress);
        });

        let status = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(18).build();
        status.add_css_class("toolbar");
        for l in [&inner.status_summary, &inner.status_speed, &inner.status_limit, &inner.status_ext] {
            status.append(l);
        }
        let spacer = gtk::Box::builder().hexpand(true).build();
        status.append(&spacer);
        status.append(&inner.status_free);

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.add_top_bar(&search_bar);
        content.add_top_bar(&inner.power);
        content.add_top_bar(&inner.engine_error);
        content.set_content(Some(&paned));
        content.add_bottom_bar(&status);
        inner.page.set_child(Some(&content));

        let side = adw::ToolbarView::new();
        side.add_top_bar(&adw::HeaderBar::new());
        side.set_content(Some(&gtk::ScrolledWindow::builder().child(&sidebar).hscrollbar_policy(gtk::PolicyType::Never).build()));
        let side_page = adw::NavigationPage::builder().title(odm_linux::APP_NAME).tag("sidebar").child(&side).build();
        let split = adw::NavigationSplitView::builder().sidebar(&side_page).content(&inner.page).min_sidebar_width(200.0).build();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));
        // one column on narrow windows
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            720.0,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(narrow);

        // ---------------------------------------------------------- wiring
        me.install_window_actions(&search_toggle);
        let weak = Rc::downgrade(&inner);
        let split_weak = split.downgrade();
        sidebar.connect_row_selected(move |_, row| {
            let (Some(inner), Some(row)) = (weak.upgrade(), row) else { return };
            let key = row.widget_name().to_string();
            if key.is_empty() || *inner.filter_key.borrow() == key {
                return;
            }
            *inner.filter_key.borrow_mut() = key;
            inner.filter.changed(gtk::FilterChange::Different);
            if let Some(title) = row.tooltip_text() {
                inner.page.set_title(&title);
            }
            MainWindow::refresh_empty(&inner);
            if let Some(split) = split_weak.upgrade() {
                split.set_show_content(true);
            }
        });
        let this = me.clone();
        selection.connect_selection_changed(move |_, _, _| {
            this.update_actions();
            this.update_details();
        });
        let this = me.clone();
        view.connect_activate(move |_, pos| {
            let Some(row) = this.inner.selection.item(pos).and_downcast::<Row>() else { return };
            if row.state() == "complete" {
                odm().open_file(&row);
            } else {
                odm().open_progress(&row.id());
            }
        });
        let keys = gtk::EventControllerKey::new();
        let this = me.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Delete || key == gdk::Key::KP_Delete {
                this.ask_delete();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        view.add_controller(keys);

        // links dragged from a browser
        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::COPY);
        drop.connect_drop(|_, value, _, _| {
            let Ok(text) = value.get::<String>() else { return false };
            let links: Vec<&str> = text.lines().map(str::trim).filter(|l| fmt::is_http(l)).collect();
            match links.as_slice() {
                [] => return false,
                [one] => crate::add::open(&odm(), json!({ "url": one })),
                many => {
                    let links: Vec<Value> = many.iter().map(|u| json!({ "url": u })).collect();
                    crate::grabber::open(&odm(), Some(json!({ "links": links })));
                }
            }
            true
        });
        window.add_controller(drop);

        window.connect_close_request(|_| odm().main_closed());
        inner.power.connect_button_clicked(|_| odm().cancel_power());
        let this = me.clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            this.tick_power();
            glib::ControlFlow::Continue
        });

        let this = me.clone();
        app.subscribe(move |c| {
            this.changed(c);
            true
        });
        me.rebuild_sidebar();
        me.changed(Change::Settings);
        me.changed(Change::Totals);
        me.changed(Change::Ext);
        me.update_actions();
        me.update_details();
        MainWindow::refresh_empty(&inner);
        if let Some(e) = app.start_error.borrow().as_ref() {
            inner.engine_error.set_title(&format!("The download engine failed to start: {e}"));
            inner.engine_error.set_revealed(true);
        }
        me
    }

    pub fn toast(&self, msg: &str) {
        self.inner.toasts.add_toast(adw::Toast::builder().title(msg).timeout(6).build());
    }

    // -------------------------------------------------------------- columns

    fn add_columns(&self) {
        let view = &self.inner.view;

        let name = gtk::SignalListItemFactory::new();
        let this = self.clone();
        name.connect_setup(move |_, obj| {
            let item = item_of(obj);
            let icon = gtk::Image::builder().pixel_size(16).build();
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).hexpand(true).build();
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            cell.append(&icon);
            cell.append(&label);
            item.property_expression("item").chain_property::<Row>("icon").bind(&icon, "gicon", gtk::Widget::NONE);
            item.property_expression("item").chain_property::<Row>("name").bind(&label, "label", gtk::Widget::NONE);
            item.property_expression("item").chain_property::<Row>("name").bind(&cell, "tooltip-text", gtk::Widget::NONE);
            this.attach_menu(&cell, item);
            item.set_child(Some(&cell));
        });
        let col = gtk::ColumnViewColumn::builder().title("Name").factory(&name).expand(true).resizable(true).build();
        col.set_sorter(Some(&Self::sorter(|a, b| s(a, "fileName").to_lowercase().cmp(&s(b, "fileName").to_lowercase()))));
        view.append_column(&col);

        self.text_column("Size", "size", true, 86, |a, b| n(a, "totalBytes").cmp(&n(b, "totalBytes")));

        let progress = gtk::SignalListItemFactory::new();
        let this = self.clone();
        progress.connect_setup(move |_, obj| {
            let item = item_of(obj);
            let bar = gtk::ProgressBar::builder().valign(gtk::Align::Center).hexpand(true).build();
            item.property_expression("item")
                .chain_property::<Row>("state")
                .chain_closure::<Vec<String>>(glib::closure!(|_: Option<glib::Object>, state: &str| {
                    vec!["odm-cell".to_string(), widgets::status_class(state).to_string()]
                }))
                .bind(&bar, "css-classes", gtk::Widget::NONE);
            let pct = gtk::Label::builder().xalign(1.0).width_chars(4).build();
            pct.add_css_class("numeric");
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            cell.append(&bar);
            cell.append(&pct);
            item.property_expression("item").chain_property::<Row>("fraction").bind(&bar, "fraction", gtk::Widget::NONE);
            item.property_expression("item").chain_property::<Row>("percent").bind(&pct, "label", gtk::Widget::NONE);
            this.attach_menu(&cell, item);
            item.set_child(Some(&cell));
        });
        let col = gtk::ColumnViewColumn::builder().title("Progress").factory(&progress).fixed_width(170).resizable(true).build();
        let fraction = |d: &Value| fmt::percent(n(d, "receivedBytes"), opt_n(d, "totalBytes"), s(d, "status") == "complete");
        col.set_sorter(Some(&Self::sorter(move |a, b| fraction(a).cmp(&fraction(b)))));
        view.append_column(&col);

        let live = |d: &Value| if fmt::is_active(s(d, "status")) { n(d, "speedBps") } else { 0 };
        self.text_column("Speed", "speed", true, 96, move |a, b| live(a).cmp(&live(b)));
        let left = |d: &Value| if s(d, "status") == "downloading" { opt_n(d, "etaSecs").unwrap_or(u64::MAX) } else { u64::MAX };
        self.text_column("ETA", "eta", true, 76, move |a, b| left(a).cmp(&left(b)));

        let status = gtk::SignalListItemFactory::new();
        let this = self.clone();
        status.connect_setup(move |_, obj| {
            let item = item_of(obj);
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            item.property_expression("item").chain_property::<Row>("status").bind(&label, "label", gtk::Widget::NONE);
            item.property_expression("item")
                .chain_property::<Row>("state")
                .chain_closure::<Vec<String>>(glib::closure!(|_: Option<glib::Object>, state: &str| {
                    vec![widgets::status_class(state).to_string()]
                }))
                .bind(&label, "css-classes", gtk::Widget::NONE);
            this.attach_menu(&label, item);
            item.set_child(Some(&label));
        });
        let col = gtk::ColumnViewColumn::builder().title("Status").factory(&status).fixed_width(124).resizable(true).build();
        col.set_sorter(Some(&Self::sorter(|a, b| s(a, "status").cmp(s(b, "status")))));
        view.append_column(&col);

        self.text_column("Queue", "queue", false, 110, |a, b| s(a, "queueId").cmp(s(b, "queueId")));
        let added = self.text_column("Added", "added", false, 120, |a, b| s(a, "addedAt").cmp(s(b, "addedAt")));
        view.sort_by_column(Some(&added), gtk::SortType::Descending);
    }

    fn sorter(cmp: impl Fn(&Value, &Value) -> std::cmp::Ordering + 'static) -> gtk::CustomSorter {
        gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (a.downcast_ref::<Row>(), b.downcast_ref::<Row>()) else { return gtk::Ordering::Equal };
            a.with(|da| b.with(|db| cmp(da, db))).into()
        })
    }

    fn text_column(
        &self,
        title: &str,
        prop: &'static str,
        numeric: bool,
        width: i32,
        cmp: impl Fn(&Value, &Value) -> std::cmp::Ordering + 'static,
    ) -> gtk::ColumnViewColumn {
        let factory = gtk::SignalListItemFactory::new();
        let this = self.clone();
        factory.connect_setup(move |_, obj| {
            let item = item_of(obj);
            let label = gtk::Label::builder()
                .xalign(if numeric { 1.0 } else { 0.0 })
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            if numeric {
                label.add_css_class("numeric");
            } else {
                label.add_css_class("dim-label");
            }
            item.property_expression("item").chain_property::<Row>(prop).bind(&label, "label", gtk::Widget::NONE);
            this.attach_menu(&label, item);
            item.set_child(Some(&label));
        });
        let col = gtk::ColumnViewColumn::builder().title(title).factory(&factory).fixed_width(width).resizable(true).build();
        col.set_sorter(Some(&Self::sorter(cmp)));
        self.inner.view.append_column(&col);
        col
    }

    // --------------------------------------------------------- context menu

    /// Right-click on a cell: select its row (unless it is part of the
    /// selection already) and show the row menu there.
    fn attach_menu(&self, cell: &impl IsA<gtk::Widget>, item: &gtk::ListItem) {
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let this = self.clone();
        let item = item.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(item), Some(cell)) = (item.upgrade(), g.widget()) else { return };
            let pos = item.position();
            if pos == gtk::INVALID_LIST_POSITION {
                return;
            }
            if !this.inner.selection.is_selected(pos) {
                this.inner.selection.select_item(pos, true);
            }
            let Some(at) = cell.compute_point(&this.inner.view, &gtk::graphene::Point::new(x as f32, y as f32)) else { return };
            this.show_row_menu(at.x() as i32, at.y() as i32);
            g.set_state(gtk::EventSequenceState::Claimed);
        });
        cell.add_controller(click);
    }

    fn show_row_menu(&self, x: i32, y: i32) {
        let rows = self.selected();
        let Some(first) = rows.first() else { return };
        let app = odm();
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str)]| {
            let m = gio::Menu::new();
            for (label, act) in items {
                m.append(Some(label), Some(act));
            }
            m
        };
        menu.append_section(None, &section(&[("Open", "win.open"), ("Show in Folder", "win.reveal"), ("Show Progress Window", "win.progress")]));
        menu.append_section(
            None,
            &section(&[
                ("Resume", "win.resume"),
                ("Pause", "win.pause"),
                ("Redownload", "win.redownload"),
                ("Refresh Link…", "win.refresh-link"),
                ("Copy Link", "win.copy-link"),
            ]),
        );
        let organize = gio::Menu::new();
        let queues = gio::Menu::new();
        for q in app.queues.borrow().iter() {
            queues.append(Some(s(q, "name")), Some(&format!("win.move-to-queue::{}", s(q, "id"))));
        }
        let none = gio::Menu::new();
        none.append(Some("Remove from Queue"), Some("win.move-to-queue::"));
        queues.append_section(None, &none);
        organize.append_submenu(Some("Move to Queue"), &queues);
        let cats = gio::Menu::new();
        let always = gio::Menu::new();
        for c in app.categories() {
            cats.append(Some(s(&c, "name")), Some(&format!("win.set-category::{}", s(&c, "id"))));
            always.append(Some(s(&c, "name")), Some(&format!("win.assign-extension::{}", s(&c, "id"))));
        }
        organize.append_submenu(Some("Category"), &cats);
        let ext = first.with(|d| fmt::extension(s(d, "fileName")));
        if rows.len() == 1 && !ext.is_empty() {
            organize.append_submenu(Some(&format!("Always Put .{ext} Files In")), &always);
        }
        menu.append_section(None, &organize);
        let label = if rows.len() > 1 { format!("Delete {} Downloads…", rows.len()) } else { "Delete…".to_string() };
        menu.append_section(None, &section(&[(label.as_str(), "win.delete")]));

        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(&self.inner.view);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, 1)));
        pop.connect_closed(|p| {
            // after the chosen action ran
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    // -------------------------------------------------------------- actions

    fn selected(&self) -> Vec<Row> {
        let sel = self.inner.selection.selection();
        (0..sel.size()).filter_map(|i| self.inner.selection.item(sel.nth(i as u32)).and_downcast::<Row>()).collect()
    }

    fn selected_ids(&self) -> Vec<String> {
        self.selected().iter().map(Row::id).collect()
    }

    fn current_queue(&self) -> String {
        match self.inner.filter_key.borrow().split_once(':') {
            Some(("queue", q)) => q.to_string(),
            _ => "main".to_string(),
        }
    }

    fn install_window_actions(&self, search_toggle: &gtk::ToggleButton) {
        let w = &self.window;
        let this = self.clone();
        action(w, "resume", move || odm().resume(&this.selected_ids()));
        let this = self.clone();
        action(w, "pause", move || odm().pause(&this.selected_ids()));
        let this = self.clone();
        action(w, "delete", move || this.ask_delete());
        let this = self.clone();
        action(w, "open", move || {
            if let Some(r) = this.selected().first() {
                odm().open_file(r);
            }
        });
        let this = self.clone();
        action(w, "reveal", move || {
            if let Some(r) = this.selected().first() {
                odm().reveal(r);
            }
        });
        let this = self.clone();
        action(w, "copy-link", move || {
            if let Some(r) = this.selected().first() {
                odm().copy_link(r);
            }
        });
        let this = self.clone();
        action(w, "progress", move || {
            if let Some(r) = this.selected().first() {
                odm().open_progress(&r.id());
            }
        });
        let this = self.clone();
        action(w, "redownload", move || {
            if let Some(r) = this.selected().first() {
                odm().act("redownload", json!({ "id": r.id() }));
            }
        });
        let this = self.clone();
        action(w, "refresh-link", move || {
            if let Some(r) = this.selected().first() {
                this.refresh_link(r);
            }
        });
        let this = self.clone();
        text_action(w, "move-to-queue", move |q| {
            let queue = if q.is_empty() { Value::Null } else { Value::String(q) };
            odm().act("move_to_queue", json!({ "ids": this.selected_ids(), "queueId": queue }));
        });
        let this = self.clone();
        text_action(w, "set-category", move |c| odm().act("set_category", json!({ "ids": this.selected_ids(), "category": c })));
        let this = self.clone();
        text_action(w, "assign-extension", move |c| {
            if let Some(r) = this.selected().first() {
                odm().assign_extension(&r.with(|d| fmt::extension(s(d, "fileName"))), &c);
            }
        });
        let this = self.clone();
        action(w, "start-queue", move || odm().act("start_queue", json!({ "id": this.current_queue() })));
        let this = self.clone();
        action(w, "stop-queue", move || odm().act("stop_queue", json!({ "id": this.current_queue() })));
        let toggle = search_toggle.clone();
        action(w, "search", move || toggle.set_active(!toggle.is_active()));
        let this = self.clone();
        action(w, "limit-custom", move || this.custom_limit());
        let this = self.clone();
        action(w, "import", move || this.import_list());
        let this = self.clone();
        action(w, "export", move || this.export_list());
        let this = self.clone();
        action(w, "about", move || this.about());
    }

    fn set_enabled(&self, name: &str, on: bool) {
        if let Some(a) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
            a.set_enabled(on);
        }
    }

    fn update_actions(&self) {
        let rows = self.selected();
        let one = rows.len() == 1;
        let state = |r: &Row| r.state();
        let first_complete = rows.first().map(|r| state(r) == "complete").unwrap_or(false);
        self.set_enabled("resume", rows.iter().any(|r| matches!(state(r).as_str(), "paused" | "error" | "queued")));
        self.set_enabled("pause", rows.iter().any(|r| r.is_active() || state(r) == "queued"));
        self.set_enabled("delete", !rows.is_empty());
        self.set_enabled("open", one && first_complete);
        self.set_enabled("reveal", one);
        self.set_enabled("copy-link", one);
        self.set_enabled("progress", one && !first_complete);
        self.set_enabled("redownload", one);
        self.set_enabled("refresh-link", one && !first_complete);
        for a in ["move-to-queue", "set-category"] {
            self.set_enabled(a, !rows.is_empty());
        }
        self.set_enabled("assign-extension", one);
    }

    fn ask_delete(&self) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        if !odm().setting_on("confirmDelete", true) {
            return odm().delete(&ids, false);
        }
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let heading = if ids.len() == 1 { "Delete this download?".to_string() } else { format!("Delete {} downloads?", ids.len()) };
            let choice = widgets::ask(
                &window,
                &heading,
                "Unfinished parts are always removed. Finished files stay on disk unless you also delete files.",
                &[
                    ("files", "Remove and Delete Files", adw::ResponseAppearance::Destructive),
                    ("list", "Remove from List", adw::ResponseAppearance::Default),
                ],
            )
            .await;
            match choice.as_str() {
                "list" => odm().delete(&ids, false),
                "files" => odm().delete(&ids, true),
                _ => {}
            }
        });
    }

    fn refresh_link(&self, row: &Row) {
        let (id, url) = (row.id(), row.with(|d| s(d, "url").to_string()));
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let mut url = url;
            let mut note = "Paste a fresh address for the same file. Progress is kept when the server reports the same size.".to_string();
            // ask again with the error until it works or the user gives up
            loop {
                let Some(new) = widgets::ask_text(&window, "Refresh Download Link", &note, &url, "Update Link").await else { return };
                match call("refresh_link", json!({ "id": id, "newUrl": new })).await {
                    Ok(_) => return,
                    Err(e) => {
                        note = e;
                        url = new;
                    }
                }
            }
        });
    }

    fn custom_limit(&self) {
        let current = fmt::mbps(n(&odm().settings.borrow(), "globalLimitBps"));
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let mut note = "Caps the combined speed of all downloads, in MB/s.".to_string();
            let mut text = current;
            loop {
                let Some(t) = widgets::ask_text(&window, "Global Speed Limit", &note, &text, "Turn On Limit").await else { return };
                match fmt::limit_bps(&t) {
                    Some(bps) => return odm().set_global_limit(Some(bps)),
                    // reject NaN / inf / 0 instead of silently doing nothing
                    None => {
                        note = fmt::LIMIT_ERROR.to_string();
                        text = t;
                    }
                }
            }
        });
    }

    fn import_list(&self) {
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Import Link List").modal(true).build();
            let Ok(file) = dialog.open_future(Some(&window)).await else { return };
            let Some(path) = file.path() else { return };
            match call("read_url_list", json!({ "path": path })).await {
                Ok(Value::Array(urls)) if !urls.is_empty() => {
                    let links: Vec<Value> = urls.iter().map(|u| json!({ "url": u })).collect();
                    crate::grabber::open(&odm(), Some(json!({ "links": links })));
                }
                Ok(_) => odm().report("No http(s) links found in that file."),
                Err(e) => odm().report(&e),
            }
        });
    }

    fn export_list(&self) {
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Export Link List").initial_name("downloads.txt").modal(true).build();
            let Ok(file) = dialog.save_future(Some(&window)).await else { return };
            let Some(path) = file.path() else { return };
            if let Err(e) = call("export_list", json!({ "path": path, "onlyUnfinished": false })).await {
                odm().report(&e);
            }
        });
    }

    fn about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(odm_linux::APP_NAME)
            .application_icon(odm_linux::APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Fast, resumable, segmented downloads")
            .license_type(gtk::License::Agpl30Only)
            .website("https://github.com/zbreilly/open-download-manager")
            .build();
        about.present(Some(&self.window));
    }

    fn primary_menu() -> gio::Menu {
        let menu = gio::Menu::new();
        let add = gio::Menu::new();
        add.append(Some("Add Batch Download…"), Some("app.add-batch"));
        add.append(Some("Site Grabber…"), Some("app.grabber"));
        menu.append_section(None, &add);
        let list = gio::Menu::new();
        list.append(Some("Pause All"), Some("app.pause-all"));
        list.append(Some("Delete Completed"), Some("app.delete-completed"));
        menu.append_section(None, &list);
        let queue = gio::Menu::new();
        queue.append(Some("Start Queue"), Some("win.start-queue"));
        queue.append(Some("Stop Queue"), Some("win.stop-queue"));
        queue.append(Some("Queues & Scheduler…"), Some("app.scheduler"));
        menu.append_section(None, &queue);
        let files = gio::Menu::new();
        files.append(Some("Import Link List…"), Some("win.import"));
        files.append(Some("Export Link List…"), Some("win.export"));
        menu.append_section(None, &files);
        let app = gio::Menu::new();
        app.append(Some("Preferences"), Some("app.preferences"));
        app.append(Some("About Open Download Manager"), Some("win.about"));
        app.append(Some("Quit"), Some("app.quit"));
        menu.append_section(None, &app);
        menu
    }

    fn limit_menu(on: bool) -> gio::Menu {
        let menu = gio::Menu::new();
        let presets = gio::Menu::new();
        for mb in LIMIT_PRESETS_MB {
            presets.append(Some(&format!("{mb} MB/s")), Some(&format!("app.limit({mb})")));
        }
        menu.append_section(None, &presets);
        let more = gio::Menu::new();
        more.append(Some("Custom…"), Some("win.limit-custom"));
        if on {
            more.append(Some("Turn Off Limit"), Some("app.limit-off"));
        }
        menu.append_section(None, &more);
        menu
    }

    // -------------------------------------------------------------- sidebar

    fn sidebar_row(&self, key: &str, title: &str, icon: &str) {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        count.add_css_class("numeric");
        count.add_css_class("caption");
        line.append(&count);
        let row = gtk::ListBoxRow::builder().child(&line).name(key).tooltip_text(title).build();
        self.inner.sidebar.append(&row);
        self.inner.counts.borrow_mut().insert(key.to_string(), count);
        if *self.inner.filter_key.borrow() == key {
            self.inner.sidebar.select_row(Some(&row));
        }
    }

    fn sidebar_heading(&self, title: &str) {
        let label = gtk::Label::builder().label(title).xalign(0.0).margin_top(12).margin_bottom(4).margin_start(6).build();
        label.add_css_class("heading");
        label.add_css_class("dim-label");
        let row = gtk::ListBoxRow::builder().child(&label).selectable(false).activatable(false).build();
        self.inner.sidebar.append(&row);
    }

    fn rebuild_sidebar(&self) {
        let app = odm();
        self.inner.sidebar.remove_all();
        self.inner.counts.borrow_mut().clear();
        self.sidebar_heading("Library");
        self.sidebar_row("library:all", "All Downloads", "folder-download-symbolic");
        self.sidebar_row("library:active", "Active", "media-playback-start-symbolic");
        self.sidebar_row("library:unfinished", "Unfinished", "content-loading-symbolic");
        self.sidebar_row("library:finished", "Finished", "object-select-symbolic");
        self.sidebar_heading("Categories");
        for c in app.categories() {
            self.sidebar_row(&format!("category:{}", s(&c, "id")), s(&c, "name"), category_icon(s(&c, "icon")));
        }
        self.sidebar_heading("Queues");
        for q in app.queues.borrow().iter() {
            let icon = if b(q, "running") {
                "media-playback-start-symbolic"
            } else if b(q, "builtin") {
                "view-list-symbolic"
            } else {
                "alarm-symbolic"
            };
            self.sidebar_row(&format!("queue:{}", s(q, "id")), s(q, "name"), icon);
        }
        // the selected category or queue is gone
        if self.inner.sidebar.selected_row().is_none() {
            *self.inner.filter_key.borrow_mut() = String::new();
            if let Some(first) = self.inner.sidebar.row_at_index(1) {
                self.inner.sidebar.select_row(Some(&first));
            }
        }
        self.update_counts();
    }

    fn update_counts(&self) {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for row in odm().all_rows() {
            row.with(|d| {
                let status = s(d, "status");
                *counts.entry("library:all".into()).or_default() += 1;
                if fmt::is_active(status) {
                    *counts.entry("library:active".into()).or_default() += 1;
                }
                let done = if status == "complete" { "library:finished" } else { "library:unfinished" };
                *counts.entry(done.into()).or_default() += 1;
                *counts.entry(format!("category:{}", s(d, "category"))).or_default() += 1;
                if let Some(q) = d.get("queueId").and_then(Value::as_str) {
                    *counts.entry(format!("queue:{q}")).or_default() += 1;
                }
            });
        }
        for (key, label) in self.inner.counts.borrow().iter() {
            let c = counts.get(key).copied().unwrap_or(0);
            label.set_label(&if c > 0 { c.to_string() } else { String::new() });
        }
    }

    fn refresh_empty(inner: &Rc<Inner>) {
        let total = odm().rows.n_items();
        let shown = inner.selection.n_items();
        if shown > 0 {
            inner.stack.set_visible_child_name("list");
            return;
        }
        inner.empty.set_title(if total == 0 {
            "No downloads yet"
        } else if inner.search.borrow().is_empty() {
            "No downloads here"
        } else {
            "No downloads match your search"
        });
        inner.empty.set_description(if total == 0 { Some("Add a link, or drag one from your browser onto this window.") } else { None });
        inner.stack.set_visible_child_name("empty");
    }

    // -------------------------------------------------------------- updates

    fn changed(&self, c: Change) {
        let app = odm();
        let inner = &self.inner;
        match c {
            Change::List => {
                // rows may have moved in or out of the current filter
                inner.filter.changed(gtk::FilterChange::Different);
                self.update_counts();
                self.update_actions();
                self.update_details();
                MainWindow::refresh_empty(inner);
            }
            Change::Progress => self.update_details(),
            Change::Queues => {
                self.rebuild_sidebar();
                self.update_details();
            }
            Change::Settings => {
                self.rebuild_sidebar();
                let st = app.settings.borrow();
                let on = b(&st, "globalLimitEnabled");
                inner.limit_button.set_label(&if on { format!("{} MB/s", fmt::mbps(n(&st, "globalLimitBps"))) } else { "No Limit".to_string() });
                inner.limit_button.set_menu_model(Some(&Self::limit_menu(on)));
                if on {
                    inner.limit_button.add_css_class("warning");
                } else {
                    inner.limit_button.remove_css_class("warning");
                }
            }
            Change::Totals => {
                let t = app.totals.borrow();
                inner.status_summary.set_label(&fmt::totals_summary(&t));
                inner.status_speed.set_label(&format!("↓ {}", fmt::speed(n(&t, "speedBps"))));
                inner.status_limit.set_label(&if b(&t, "limitOn") {
                    format!("Limit {} MB/s", fmt::mbps(n(&t, "limitBps")))
                } else {
                    "No speed limit".to_string()
                });
                inner.status_free.set_label(&opt_n(&t, "freeBytes").map(|f| format!("{} free", fmt::bytes(Some(f)))).unwrap_or_default());
                let active = n(&t, "active");
                self.window.set_title(Some(&if active > 0 {
                    format!("{} — {active} active · ↓ {}", odm_linux::APP_NAME, fmt::speed(n(&t, "speedBps")))
                } else {
                    odm_linux::APP_NAME.to_string()
                }));
                self.update_details();
            }
            Change::Ext => {
                let e = app.ext.borrow();
                let browsers: Vec<&str> = e.get("browsers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                let (text, class) = if let Some(err) = e.get("bridgeError").and_then(Value::as_str) {
                    inner.status_ext.set_tooltip_text(Some(err));
                    ("● Browser bridge offline: port in use".to_string(), "error")
                } else if b(&e, "connected") {
                    inner.status_ext.set_tooltip_text(None);
                    let list = if browsers.is_empty() { String::new() } else { format!(" ({})", browsers.join(", ")) };
                    (format!("● Browser extension connected{list}"), "success")
                } else {
                    inner.status_ext.set_tooltip_text(None);
                    ("○ Browser extension not detected".to_string(), "dim-label")
                };
                inner.status_ext.set_label(&text);
                for c in ["error", "success", "dim-label"] {
                    inner.status_ext.remove_css_class(c);
                }
                inner.status_ext.add_css_class(class);
            }
            Change::Power => self.tick_power(),
        }
    }

    fn tick_power(&self) {
        let app = odm();
        let current = app.power.borrow().clone();
        let Some(p) = current else {
            self.inner.power.set_revealed(false);
            return;
        };
        let left = p.deadline.saturating_duration_since(Instant::now()).as_secs();
        if left == 0 {
            return app.run_power();
        }
        let verb = match p.action.as_str() {
            "sleep" => "Sleeping",
            "shutdown" => "Shutting down",
            _ => "Quitting",
        };
        self.inner.power.set_title(&format!("“{}” finished. {verb} in {left}s.", p.queue));
        self.inner.power.set_revealed(true);
    }

    // -------------------------------------------------------------- details

    fn build_details() -> Details {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(16)
            .margin_end(16)
            .build();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = gtk::Image::builder().pixel_size(32).build();
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
        let name = gtk::Label::builder().xalign(0.0).selectable(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        name.add_css_class("heading");
        let url = widgets::value_label();
        url.add_css_class("caption");
        url.add_css_class("dim-label");
        titles.append(&name);
        titles.append(&url);
        let status = gtk::Label::new(None);
        let primary = gtk::Button::new();
        let retry = gtk::Button::builder().label("Retry").build();
        let reveal = gtk::Button::builder().icon_name("folder-symbolic").tooltip_text("Show in Folder").action_name("win.reveal").build();
        let copy = gtk::Button::builder().icon_name("edit-copy-symbolic").tooltip_text("Copy Link").action_name("win.copy-link").build();
        head.append(&icon);
        head.append(&titles);
        for w in [status.upcast_ref::<gtk::Widget>(), primary.upcast_ref(), retry.upcast_ref(), reveal.upcast_ref(), copy.upcast_ref()] {
            w.set_valign(gtk::Align::Center);
            head.append(w);
        }
        let error = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).visible(false).build();
        error.add_css_class("error");

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        let grid = gtk::Grid::builder().column_spacing(14).row_spacing(6).build();
        let mut values = vec![];
        for (i, label) in ["Downloaded", "Speed", "Time left", "Connections", "Resume", "Save to"].iter().enumerate() {
            let l = gtk::Label::builder().label(*label).xalign(0.0).build();
            l.add_css_class("dim-label");
            let v = widgets::value_label();
            v.set_max_width_chars(42);
            grid.attach(&l, 0, i as i32, 1, 1);
            grid.attach(&v, 1, i as i32, 1, 1);
            values.push(v);
        }
        let graphs = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).hexpand(true).build();
        let seg_head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let seg_title = widgets::caption("Segments");
        seg_title.set_hexpand(true);
        let percent = widgets::caption("");
        percent.add_css_class("numeric");
        seg_head.append(&seg_title);
        seg_head.append(&percent);
        let segments = SegmentMap::new(18, false);
        let spark = Sparkline::new(52);
        graphs.append(&seg_head);
        graphs.append(&segments.area);
        graphs.append(&spark.root);
        body.append(&grid);
        body.append(&graphs);

        root.append(&head);
        root.append(&error);
        root.append(&body);
        Details { root, icon, name, url, status, error, primary, retry, values, percent, segments, spark, primary_action: RefCell::new("") }
    }

    /// The download the details panel shows: the first selected one, else
    /// the first in the list.
    fn focused(&self) -> Option<Row> {
        self.selected().into_iter().next().or_else(|| self.inner.selection.item(0).and_downcast::<Row>())
    }

    fn update_details(&self) {
        let d = &self.inner.details;
        if !*self.inner.details_on.borrow() {
            return;
        }
        let Some(row) = self.focused() else {
            d.root.set_visible(false);
            return;
        };
        d.root.set_visible(true);
        let data = row.data();
        let status = s(&data, "status");
        d.icon.set_from_gicon(&crate::row::file_icon(s(&data, "fileName")));
        d.name.set_label(s(&data, "fileName"));
        d.url.set_label(s(&data, "url"));
        d.status.set_label(&row.status());
        widgets::paint_status(&d.status, status);
        let err = data.get("error").and_then(Value::as_str).filter(|_| status == "error");
        d.error.set_visible(err.is_some());
        d.error.set_label(err.unwrap_or(""));
        let (label, act) = match status {
            "complete" => ("Open", "open"),
            "error" => ("Refresh Link…", "refresh"),
            "paused" => ("Resume", "resume"),
            _ => ("Pause", "pause"),
        };
        d.primary.set_label(label);
        *d.primary_action.borrow_mut() = act;
        d.retry.set_visible(status == "error");
        let texts = [
            format!("{} of {}", fmt::bytes(Some(n(&data, "receivedBytes"))), fmt::bytes(opt_n(&data, "totalBytes"))),
            row.speed(),
            row.eta(),
            format!("{} active / {} max", n(&data, "activeConnections"), n(&data, "maxConnections")),
            if b(&data, "resumable") { "Supported (byte ranges)".to_string() } else { "Not supported by server".to_string() },
            s(&data, "saveDir").to_string(),
        ];
        for (label, text) in d.values.iter().zip(texts) {
            label.set_label(&text);
        }
        d.percent.set_label(&row.percent());
        d.segments.set(&data);
        d.spark.set(&data);
        // the buttons act on the row shown, which may not be selected
        let id = row.id();
        self.bind_detail_buttons(&id);
    }

    fn bind_detail_buttons(&self, id: &str) {
        thread_local! {
            static SHOWN: RefCell<String> = const { RefCell::new(String::new()) };
            static WIRED: RefCell<bool> = const { RefCell::new(false) };
        }
        SHOWN.with(|s| *s.borrow_mut() = id.to_string());
        if WIRED.with(|w| w.replace(true)) {
            return;
        }
        let shown = || SHOWN.with(|s| s.borrow().clone());
        let this = self.clone();
        self.inner.details.primary.connect_clicked(move |_| {
            let app = odm();
            let id = shown();
            let Some(row) = app.row(&id) else { return };
            let act = *this.inner.details.primary_action.borrow();
            match act {
                "open" => app.open_file(&row),
                "refresh" => this.refresh_link(&row),
                "resume" => app.resume(&[id]),
                _ => app.pause(&[id]),
            }
        });
        self.inner.details.retry.connect_clicked(move |_| odm().resume(&[shown()]));
    }
}
