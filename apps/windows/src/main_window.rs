//! The main window: library, categories and queues on the left, the
//! download list, a details panel and a status bar.
use crate::app::{act, limit_label, toggle_limit, Action, Shared};
use crate::widgets::*;
use crate::{engine, system};
use odm_ui::store::{Row, SortKey, Store};
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows_reactor::*;

const LIMIT_PRESETS_MB: [u64; 7] = [1, 2, 5, 10, 25, 50, 100];
/// widths of the list's columns after the name
const COLUMNS: [(SortKey, &str, f64); 7] = [
    (SortKey::Size, "Size", 86.0),
    (SortKey::Progress, "Progress", 170.0),
    (SortKey::Speed, "Speed", 96.0),
    (SortKey::Eta, "ETA", 76.0),
    (SortKey::Status, "Status", 124.0),
    (SortKey::Queue, "Queue", 110.0),
    (SortKey::Added, "Added", 120.0),
];

fn column_widths() -> Vec<GridLength> {
    std::iter::once(GridLength::STAR).chain(COLUMNS.iter().map(|c| GridLength::Pixel(c.2))).collect()
}

/// A question the window is asking.
#[derive(Clone, PartialEq)]
enum Dialog {
    None,
    Delete(Vec<String>),
    /// refresh a download's link: (id, text, note or error)
    Refresh(String, String, String),
    /// custom global limit: (text, note or error)
    Limit(String, String),
    About,
}

pub enum Msg {
    Nav(Option<String>),
    Search(String),
    Sort(SortKey),
    /// a row was pressed: (id, pointer state)
    Press(String, PointerEventInfo),
    /// toolbar, menus and shortcuts, by key
    Command(String),
    DialogText(String),
    DialogClosed(ContentDialogResult),
    /// a background call finished: Err to show in the open dialog
    Refreshed(Result<(), String>),
    Imported(Result<Vec<String>, String>),
    DismissError,
    /// text dropped on the window
    Dropped(String),
}

pub struct MainWindow {
    shared: Shared,
    filter: String,
    search: String,
    sort: SortKey,
    descending: bool,
    selected: Vec<String>,
    /// where a Shift-click range starts
    anchor: Option<String>,
    last_press: Option<(String, Instant)>,
    details: bool,
    dialog: Dialog,
    /// serial of the last error the user closed
    dismissed: u64,
}

impl Drop for MainWindow {
    fn drop(&mut self) {
        self.shared.act(Action::MainClosed);
    }
}

#[derive(Clone)]
struct RowInput {
    row: Row,
    selected: bool,
    sender: LocalSender<Msg>,
    menu: Menu,
}

impl PartialEq for RowInput {
    fn eq(&self, other: &Self) -> bool {
        self.row == other.row && self.selected == other.selected
    }
}

/// One line of the list.
struct RowView;

impl Component for RowView {
    type Input = RowInput;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, input: &Self::Input, _: &mut ViewContext<Self>) -> View {
        let r = &input.row;
        let id = r.id.clone();
        let numeric = |t: &str, column: i32| text(t).horizontal_alignment(HorizontalAlignment::Right).grid_column(column);
        let cells = Grid::new().column_spacing(8.0).columns(column_widths()).children((
            StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).children((
                FontIcon::new().glyph(file_glyph(&r.category)).vertical_alignment(VerticalAlignment::Center),
                text(&r.name),
            )),
            numeric(&r.size, 1),
            Grid::new().column_spacing(8.0).columns([GridLength::STAR, GridLength::Pixel(40.0)]).grid_column(2).children((
                ProgressBar::new()
                    .maximum(100.0)
                    .value(r.percent as f64)
                    .show_paused(r.status == "paused")
                    .show_error(r.status == "error")
                    .vertical_alignment(VerticalAlignment::Center),
                text(format!("{}%", r.percent)).horizontal_alignment(HorizontalAlignment::Right).grid_column(1),
            )),
            numeric(&r.speed, 3),
            numeric(&r.eta, 4),
            StackPanel::new().grid_column(5).children((status_label(&r.status, &r.status_label),)),
            dim(&r.queue).grid_column(6),
            dim(&r.added).grid_column(7),
        ));
        Border::new()
            .background(if input.selected { Brush::from(Color::argb(0x40, 0x3B, 0x8E, 0xEA)) } else { Brush::from(Color::transparent()) })
            .corner_radius(4.0)
            .padding(Thickness::xy(12.0, 0.0))
            .height(36.0)
            .on_pointer_pressed(input.sender.callback(move |info: PointerEventInfo| Msg::Press(id.clone(), info)))
            .content(cells)
            .tooltip(&r.name)
            .menu(input.menu.clone())
    }
}

/// A menu or toolbar choice, by its key.
fn command(key: Key) -> Msg {
    Msg::Command(key.as_str().unwrap_or("").to_string())
}

fn item(key: &str, label: &str, enabled: bool) -> MenuItem {
    if enabled {
        MenuItem::item(key, label)
    } else {
        MenuItem::disabled(key, label)
    }
}

impl MainWindow {
    fn rows<'a>(&self, store: &'a Store) -> Vec<&'a Value> {
        store.visible(&self.filter, &self.search, self.sort, self.descending)
    }

    fn selected_records(&self, store: &Store) -> Vec<Value> {
        self.selected.iter().filter_map(|id| store.get(id).cloned()).collect()
    }

    /// The download the details panel shows: the first selected one, else
    /// the first in the list.
    fn focused(&self, store: &Store) -> Option<Value> {
        self.selected.iter().find_map(|id| store.get(id).cloned()).or_else(|| self.rows(store).first().map(|d| (*d).clone()))
    }

    fn current_queue(&self) -> String {
        match self.filter.split_once(':') {
            Some(("queue", q)) => q.to_string(),
            _ => "main".into(),
        }
    }

    /// The right-click menu for the current selection.
    fn row_menu(&self, store: &Store, sender: &LocalSender<Msg>) -> Menu {
        let rows = self.selected_records(store);
        let one = rows.len() == 1;
        let status = |d: &Value| s(d, "status").to_string();
        let first_complete = rows.first().map(|d| status(d) == "complete").unwrap_or(false);
        let ext = rows.first().map(|d| fmt::extension(s(d, "fileName"))).unwrap_or_default();
        let queues: Vec<MenuItem> = store
            .queues
            .iter()
            .map(|q| MenuItem::item(format!("queue:{}", s(q, "id")), s(q, "name")))
            .chain([MenuItem::separator("qs"), MenuItem::item("queue:", "Remove from Queue")])
            .collect();
        let categories = store.categories();
        let mut items = vec![
            item("open", "Open", one && first_complete),
            item("reveal", "Show in Folder", one),
            item("progress", "Show Progress Window", one && !first_complete),
            MenuItem::separator("s1"),
            item("resume", "Resume", rows.iter().any(|d| matches!(status(d).as_str(), "paused" | "error" | "queued"))),
            item("pause", "Pause", rows.iter().any(|d| fmt::is_active(&status(d)) || status(d) == "queued")),
            item("redownload", "Redownload", one),
            item("refresh", "Refresh Link…", one && !first_complete),
            item("copy", "Copy Link", one),
            MenuItem::separator("s2"),
            MenuItem::submenu("queues", "Move to Queue", queues),
            MenuItem::submenu("categories", "Category", categories.iter().map(|c| MenuItem::item(format!("category:{}", s(c, "id")), s(c, "name")))),
        ];
        if one && !ext.is_empty() {
            items.push(MenuItem::submenu(
                "always",
                format!("Always Put .{ext} Files In"),
                categories.iter().map(|c| MenuItem::item(format!("always:{}", s(c, "id")), s(c, "name"))),
            ));
        }
        items.push(MenuItem::separator("s3"));
        items.push(MenuItem::item("delete", if one { "Delete…".to_string() } else { format!("Delete {} Downloads…", rows.len()) }));
        Menu::new(items, sender.callback(command))
    }

    fn press(&mut self, id: String, info: PointerEventInfo) {
        let store = self.shared.store.clone();
        let store = store.borrow();
        if info.is_right_button_pressed {
            // the menu acts on the selection: a row outside it becomes it
            if !self.selected.contains(&id) {
                self.selected = vec![id.clone()];
                self.anchor = Some(id);
            }
            return;
        }
        let double = self.last_press.as_ref().map(|(last, at)| *last == id && at.elapsed() < Duration::from_millis(450)).unwrap_or(false);
        self.last_press = Some((id.clone(), Instant::now()));
        if double {
            if let Some(d) = store.get(&id) {
                if s(d, "status") == "complete" {
                    self.open_file(d);
                } else {
                    self.shared.act(Action::Progress(id.clone()));
                }
            }
            return;
        }
        let order: Vec<String> = self.rows(&store).iter().map(|d| s(d, "id").to_string()).collect();
        if info.modifiers.contains(InputModifiers::SHIFT) {
            let from = self.anchor.as_ref().and_then(|a| order.iter().position(|x| x == a)).unwrap_or(0);
            if let Some(to) = order.iter().position(|x| *x == id) {
                self.selected = order[from.min(to)..=from.max(to)].to_vec();
            }
        } else if info.modifiers.contains(InputModifiers::CONTROL) {
            match self.selected.iter().position(|x| *x == id) {
                Some(i) => {
                    self.selected.remove(i);
                }
                None => self.selected.push(id.clone()),
            }
            self.anchor = Some(id);
        } else {
            self.selected = vec![id.clone()];
            self.anchor = Some(id);
        }
    }

    fn open_file(&self, d: &Value) {
        if let Err(e) = system::open_path(&Path::new(s(d, "saveDir")).join(s(d, "fileName"))) {
            self.shared.report(e);
        }
    }

    fn ask_delete(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        if self.shared.store.borrow().setting("confirmDelete", true) {
            self.dialog = Dialog::Delete(self.selected.clone());
        } else {
            act("delete", json!({ "ids": self.selected, "deleteFiles": false }));
        }
    }

    fn command(&mut self, key: &str, context: &ComponentContext<Self>) {
        let store = self.shared.store.clone();
        let store = store.borrow();
        let ids = self.selected.clone();
        let first = ids.first().and_then(|id| store.get(id)).cloned();
        match key.split_once(':').unwrap_or((key, "")) {
            ("add", _) => self.shared.act(Action::Add(json!({}))),
            ("add-batch", _) => self.shared.act(Action::Add(json!({ "batch": true }))),
            ("resume", _) => act("resume", json!({ "ids": ids })),
            ("pause", _) => act("pause", json!({ "ids": ids })),
            ("pause-all", _) => act("pause_all", json!({})),
            ("delete", _) => self.ask_delete(),
            ("delete-completed", _) => act("delete_completed", json!({})),
            ("start-queue", _) => act("start_queue", json!({ "id": self.current_queue() })),
            ("stop-queue", _) => act("stop_queue", json!({ "id": self.current_queue() })),
            ("scheduler", _) => self.shared.act(Action::Scheduler),
            ("grabber", _) => self.shared.act(Action::Grabber(None)),
            ("settings", _) => self.shared.act(Action::Settings),
            ("quit", _) => self.shared.act(Action::Quit),
            ("about", _) => self.dialog = Dialog::About,
            ("details", _) => self.details = !self.details,
            ("limit-toggle", _) => toggle_limit(&store),
            ("limit-custom", _) => {
                self.dialog = Dialog::Limit(fmt::mbps(n(&store.settings, "globalLimitBps")), "Caps the combined speed of all downloads, in MB/s.".into());
            }
            ("limit", mb) => {
                if let Ok(mb) = mb.parse::<u64>() {
                    act("set_global_limit", json!({ "bps": mb * 1024 * 1024 }));
                }
            }
            ("open", _) => {
                if let Some(d) = &first {
                    self.open_file(d);
                }
            }
            ("reveal", _) => {
                if let Some(Err(e)) = first.as_ref().map(system::reveal_download) {
                    self.shared.report(e);
                }
            }
            ("progress", _) => {
                if let Some(id) = ids.first() {
                    self.shared.act(Action::Progress(id.clone()));
                }
            }
            ("redownload", _) => {
                if let Some(id) = ids.first() {
                    act("redownload", json!({ "id": id }));
                }
            }
            ("refresh", _) => {
                if let Some(d) = &first {
                    let note = "Paste a fresh address for the same file. Progress is kept when the server reports the same size.";
                    self.dialog = Dialog::Refresh(s(d, "id").into(), s(d, "url").into(), note.into());
                }
            }
            ("copy", _) => {
                if let Some(d) = &first {
                    system::set_clipboard_text(s(d, "url"));
                }
            }
            ("queue", q) => {
                let queue = if q.is_empty() { Value::Null } else { q.into() };
                act("move_to_queue", json!({ "ids": ids, "queueId": queue }));
            }
            ("category", c) => act("set_category", json!({ "ids": ids, "category": c })),
            ("always", c) => {
                if let Some(d) = &first {
                    let ext = fmt::extension(s(d, "fileName"));
                    act("assign_extensions", json!({ "category": c, "extensions": [ext] }));
                    let same = store.same_extension(&ext, c);
                    if !same.is_empty() {
                        act("set_category", json!({ "ids": same, "category": c }));
                    }
                }
            }
            ("import", _) => {
                // the picker blocks, so it and the read run off the UI thread
                context.spawn_background(|_| {
                    let Some(path) = system::pick_text_file() else { return Msg::Imported(Ok(vec![])) };
                    Msg::Imported(engine::call("read_url_list", json!({ "path": path })).map(|v| {
                        let found: Vec<String> = odm_ui::strings(&json!({ "u": v }), "u");
                        if found.is_empty() { vec![String::new()] } else { found }
                    }))
                });
            }
            ("export", _) => {
                context.spawn_background(|_| {
                    let Some(path) = system::pick_save_file("downloads.txt") else { return Msg::Refreshed(Ok(())) };
                    Msg::Refreshed(engine::call("export_list", json!({ "path": path, "onlyUnfinished": false })).map(|_| ()))
                });
            }
            _ => {}
        }
    }

    fn dialog_closed(&mut self, result: ContentDialogResult, context: &ComponentContext<Self>) {
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        match (dialog, result) {
            (Dialog::Delete(ids), ContentDialogResult::Primary) => act("delete", json!({ "ids": ids, "deleteFiles": false })),
            (Dialog::Delete(ids), ContentDialogResult::Secondary) => act("delete", json!({ "ids": ids, "deleteFiles": true })),
            (Dialog::Refresh(id, url, note), ContentDialogResult::Primary) => {
                // stays open (with the error) until the engine accepts it
                self.dialog = Dialog::Refresh(id.clone(), url.clone(), note);
                context.spawn_background(move |_| Msg::Refreshed(engine::call("refresh_link", json!({ "id": id, "newUrl": url.trim() })).map(|_| ())));
            }
            (Dialog::Limit(text, _), ContentDialogResult::Primary) => match fmt::limit_bps(&text) {
                Some(bps) => act("set_global_limit", json!({ "bps": bps })),
                // reject NaN / inf / 0 instead of silently doing nothing
                None => self.dialog = Dialog::Limit(text, fmt::LIMIT_ERROR.into()),
            },
            _ => {}
        }
    }

    // -------------------------------------------------------------- views

    fn sidebar(&self, store: &Store) -> Vec<View> {
        let counts = store.counts();
        let entry = |key: String, title: &str, glyph: &str| -> View {
            let count = counts.get(&key).copied().unwrap_or(0);
            let label = Grid::new().columns([GridLength::STAR, GridLength::Auto]).children((
                text(title),
                dim(if count > 0 { count.to_string() } else { String::new() }).grid_column(1).margin(Thickness::new(8.0, 0.0, 4.0, 0.0)),
            ));
            NavigationViewItem::new().tag(&key).icon(Icon::font(glyph)).is_selected(self.filter == key).content(label).into()
        };
        let header = |title: &str| -> View { dim(title).font_weight(FontWeight::SEMI_BOLD).margin(Thickness::new(16.0, 14.0, 0.0, 4.0)).into() };
        let mut items = vec![
            header("Library"),
            entry("library:all".into(), "All Downloads", "\u{E896}"),
            entry("library:active".into(), "Active", "\u{E768}"),
            entry("library:unfinished".into(), "Unfinished", "\u{E916}"),
            entry("library:finished".into(), "Finished", "\u{E73E}"),
            header("Categories"),
        ];
        for c in store.categories() {
            items.push(entry(format!("category:{}", s(&c, "id")), s(&c, "name"), category_glyph(s(&c, "icon"))));
        }
        items.push(header("Queues"));
        for q in &store.queues {
            let glyph = if b(q, "running") {
                "\u{E768}"
            } else if b(q, "builtin") {
                "\u{E8FD}"
            } else {
                "\u{E823}"
            };
            items.push(entry(format!("queue:{}", s(q, "id")), s(q, "name"), glyph));
        }
        items
    }

    fn toolbar(&self, store: &Store, context: &ViewContext<Self>) -> View {
        let rows = self.selected_records(store);
        let status = |d: &Value| s(d, "status").to_string();
        let can_resume = rows.iter().any(|d| matches!(status(d).as_str(), "paused" | "error" | "queued"));
        let can_pause = rows.iter().any(|d| fmt::is_active(&status(d)) || status(d) == "queued");
        let button = |key: &str, label: &str, symbol: Symbol, enabled: bool| {
            if enabled {
                CommandBarCommand::button_with_icon(key, label, symbol)
            } else {
                CommandBarCommand::disabled(key, label)
            }
        };
        let primary = vec![
            button("add", "Add URL", Symbol::Add, true),
            CommandBarCommand::separator("s1"),
            button("resume", "Resume", Symbol::Play, can_resume),
            button("pause", "Pause", Symbol::Pause, can_pause),
            button("delete", "Delete", Symbol::Delete, !rows.is_empty()),
            CommandBarCommand::separator("s2"),
            button("start-queue", "Start Queue", Symbol::List, true),
            button("grabber", "Grabber", Symbol::Globe, true),
            button("limit-toggle", &limit_label(store), Symbol::Clock, true),
            button("details", "Details", Symbol::DockBottom, true),
        ];
        let mut secondary = vec![
            CommandBarCommand::button("add-batch", "Add Batch Download…"),
            CommandBarCommand::button("pause-all", "Pause All"),
            CommandBarCommand::button("delete-completed", "Delete Completed"),
            CommandBarCommand::separator("s3"),
            CommandBarCommand::button("stop-queue", "Stop Queue"),
            CommandBarCommand::button("scheduler", "Queues & Scheduler…"),
            CommandBarCommand::separator("s4"),
        ];
        for mb in LIMIT_PRESETS_MB {
            secondary.push(CommandBarCommand::button(format!("limit:{mb}"), format!("Limit to {mb} MB/s")));
        }
        secondary.extend([
            CommandBarCommand::button("limit-custom", "Custom Limit…"),
            CommandBarCommand::separator("s5"),
            CommandBarCommand::button("import", "Import Link List…"),
            CommandBarCommand::button("export", "Export Link List…"),
            CommandBarCommand::separator("s6"),
            CommandBarCommand::button("settings", "Settings…"),
            CommandBarCommand::button("about", "About"),
            CommandBarCommand::button("quit", "Quit"),
        ]);
        CommandBar::new().owned_commands(primary, secondary, context.callback(command)).horizontal_alignment(HorizontalAlignment::Left).into()
    }

    fn header(&self, context: &ViewContext<Self>) -> View {
        let title = |key: SortKey, label: &str| {
            let arrow = if self.sort != key {
                ""
            } else if self.descending {
                " ↓"
            } else {
                " ↑"
            };
            Button::new().style(ButtonStyle::Subtle).content(format!("{label}{arrow}")).on_click(context.callback(move |()| Msg::Sort(key)))
        };
        let mut cells: Vec<View> = vec![title(SortKey::Name, "Name").into()];
        for (i, (key, label, _)) in COLUMNS.iter().enumerate() {
            cells.push(title(*key, label).grid_column(i as i32 + 1).into());
        }
        Border::new()
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::new(0.0, 0.0, 0.0, 1.0))
            .padding(Thickness::xy(12.0, 0.0))
            .content(Grid::new().column_spacing(8.0).columns(column_widths()).children(cells))
            .into()
    }

    fn list(&self, store: &Store, context: &ViewContext<Self>) -> View {
        let rows: Vec<Row> = self.rows(store).iter().map(|d| store.row(d)).collect();
        if rows.is_empty() {
            let message = if store.downloads().is_empty() {
                "No downloads yet"
            } else if self.search.is_empty() {
                "No downloads here"
            } else {
                "No downloads match your search"
            };
            let mut children: Vec<View> = vec![
                FontIcon::new().glyph("\u{E896}").opacity(0.6).into(),
                dim(message).horizontal_alignment(HorizontalAlignment::Center).into(),
                Button::new().content("Add URL…").horizontal_alignment(HorizontalAlignment::Center).on_click(context.callback(|()| Msg::Command("add".into()))).into(),
            ];
            if store.downloads().is_empty() {
                children.push(note("Or drag a link from your browser onto this window.").into());
            }
            return StackPanel::new()
                .spacing(10.0)
                .horizontal_alignment(HorizontalAlignment::Center)
                .vertical_alignment(VerticalAlignment::Center)
                .children(children)
                .into();
        }
        // the key revision tells the list when rows came, went or moved
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for r in &rows {
            r.id.hash(&mut hasher);
        }
        let sender = context.sender();
        let menu = self.row_menu(store, &sender);
        let selected: std::collections::HashSet<String> = self.selected.iter().cloned().collect();
        let rows = Rc::new(rows);
        let keys = rows.clone();
        let source = VirtualSource::new(
            hasher.finish(),
            rows.len(),
            move |i| keys[i].id.clone(),
            move |i| {
                let row = rows[i].clone();
                let selected = selected.contains(&row.id);
                View::component::<RowView>(RowInput { row, selected, sender: sender.clone(), menu: menu.clone() })
            },
        );
        ScrollViewer::new().content(ItemsRepeater::new().virtual_source(source)).into()
    }

    fn details(&self, d: &Value, store: &Store, context: &ViewContext<Self>) -> View {
        let row = store.row(d);
        let status = row.status.as_str();
        let id = row.id.clone();
        let (label, command) = match status {
            "complete" => ("Open", "detail-open"),
            "error" => ("Refresh Link…", "detail-refresh"),
            "paused" => ("Resume", "detail-resume"),
            _ => ("Pause", "detail-pause"),
        };
        let button = |label: &str, command: &'static str, style: ButtonStyle| {
            let id = id.clone();
            Button::new().style(style).content(label).on_click(context.callback(move |()| Msg::Command(format!("{command}:{id}"))))
        };
        let mut actions: Vec<View> = vec![status_label(status, &row.status_label)];
        if status == "error" {
            actions.push(button("Retry", "detail-resume", ButtonStyle::Default).into());
        }
        actions.push(button(label, command, ButtonStyle::Accent).into());
        actions.push(button("Show in Folder", "detail-reveal", ButtonStyle::Default).into());
        actions.push(button("Copy Link", "detail-copy", ButtonStyle::Default).into());
        let facts = [
            ("Downloaded", format!("{} of {}", fmt::bytes(Some(n(d, "receivedBytes"))), fmt::bytes(opt_n(d, "totalBytes")))),
            ("Speed", row.speed.clone()),
            ("Time left", row.eta.clone()),
            ("Connections", format!("{} active / {} max", n(d, "activeConnections"), n(d, "maxConnections"))),
            ("Resume", if b(d, "resumable") { "Supported (byte ranges)".into() } else { "Not supported by server".to_string() }),
            ("Save to", s(d, "saveDir").to_string()),
        ];
        let mut fact_cells: Vec<View> = vec![];
        for (i, (label, value)) in facts.iter().enumerate() {
            fact_cells.push(dim(label).grid_row(i as i32).into());
            fact_cells.push(text(value).is_text_selection_enabled(true).grid_row(i as i32).grid_column(1).into());
        }
        let mut body: Vec<View> = vec![Grid::new()
            .column_spacing(12.0)
            .columns([GridLength::Auto, GridLength::STAR, GridLength::Auto])
            .children((
                FontIcon::new().glyph(file_glyph(&row.category)).vertical_alignment(VerticalAlignment::Center),
                StackPanel::new().grid_column(1).vertical_alignment(VerticalAlignment::Center).children((
                    text(&row.name).font_weight(FontWeight::SEMI_BOLD).is_text_selection_enabled(true),
                    dim(s(d, "url")).font_size(12.0).is_text_selection_enabled(true),
                )),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).grid_column(2).children(actions),
            ))
            .into()];
        if let (true, Some(e)) = (status == "error", d.get("error").and_then(Value::as_str)) {
            body.push(error_text(e).into());
        }
        body.push(
            Grid::new()
                .column_spacing(24.0)
                .columns([GridLength::Pixel(380.0), GridLength::STAR])
                .children((
                    Grid::new().column_spacing(14.0).row_spacing(4.0).columns([GridLength::Auto, GridLength::STAR]).rows(std::iter::repeat_n(GridLength::Auto, facts.len())).children(fact_cells),
                    StackPanel::new().spacing(6.0).grid_column(1).children((
                        Grid::new().columns([GridLength::STAR, GridLength::Auto]).children((note("Segments"), note(format!("{}%", row.percent)).grid_column(1))),
                        segment_map(d, 18.0, false),
                        sparkline(d, 52.0),
                    )),
                ))
                .into(),
        );
        Border::new()
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
            .padding(Thickness::xy(16.0, 12.0))
            .content(StackPanel::new().spacing(10.0).children(body))
            .into()
    }

    fn status_bar(&self, store: &Store) -> View {
        let t = &store.totals;
        let ext = &store.ext;
        let error = ext.get("bridgeError").and_then(Value::as_str);
        let browsers = odm_ui::strings(ext, "browsers");
        let (ext_status, ext_text) = if error.is_some() {
            ("error", "Browser bridge offline: port in use".to_string())
        } else if b(ext, "connected") {
            ("complete", format!("Browser extension connected{}", if browsers.is_empty() { String::new() } else { format!(" ({})", browsers.join(", ")) }))
        } else {
            ("queued", "Browser extension not detected".to_string())
        };
        let limit = if b(t, "limitOn") { format!("Limit {} MB/s", fmt::mbps(n(t, "limitBps"))) } else { "No speed limit".into() };
        let small = |t: String| dim(t).font_size(12.0);
        Border::new()
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
            .padding(Thickness::xy(16.0, 6.0))
            .content(Grid::new().column_spacing(18.0).columns([GridLength::Auto, GridLength::Auto, GridLength::Auto, GridLength::STAR, GridLength::Auto]).children((
                small(fmt::totals_summary(t)),
                small(format!("↓ {}", fmt::speed(n(t, "speedBps")))).grid_column(1),
                small(limit).grid_column(2),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(6.0).grid_column(3).children((
                    Ellipse::new().width(7.0).height(7.0).fill(status_color(ext_status)).vertical_alignment(VerticalAlignment::Center),
                    small(ext_text).tooltip(error.unwrap_or("The extension talks to the app on this computer only.")),
                )),
                small(opt_n(t, "freeBytes").map(|f| format!("{} free", fmt::bytes(Some(f)))).unwrap_or_default()).grid_column(4),
            )))
            .into()
    }

    fn dialog(&self, context: &ViewContext<Self>) -> ContentDialog {
        let closed = context.callback(Msg::DialogClosed);
        let field = |value: &str, note_text: &str| {
            StackPanel::new().spacing(10.0).children((
                note(note_text),
                TextBox::new(value).on_text_changed(context.callback(|t: Rc<str>| Msg::DialogText(t.to_string()))),
            ))
        };
        match &self.dialog {
            Dialog::None => ContentDialog::new().is_open(false),
            Dialog::Delete(ids) => ContentDialog::new()
                .is_open(true)
                .title(if ids.len() == 1 { "Delete this download?".to_string() } else { format!("Delete {} downloads?", ids.len()) })
                .content(note("Unfinished parts are always removed. Finished files stay on disk unless you also delete files."))
                .primary_button_text("Remove from List")
                .secondary_button_text("Remove and Delete Files")
                .close_button_text("Cancel")
                .on_closed(closed),
            Dialog::Refresh(_, url, note_text) => ContentDialog::new()
                .is_open(true)
                .title("Refresh Download Link")
                .content(field(url, note_text))
                .primary_button_text("Update Link")
                .close_button_text("Cancel")
                .on_closed(closed),
            Dialog::Limit(value, note_text) => ContentDialog::new()
                .is_open(true)
                .title("Global Speed Limit")
                .content(field(value, note_text))
                .primary_button_text("Turn On Limit")
                .close_button_text("Cancel")
                .on_closed(closed),
            Dialog::About => ContentDialog::new()
                .is_open(true)
                .title(odm_ui::APP_NAME)
                .content(note(format!(
                    "Version {}\nFast, resumable, segmented downloads.\n\nFree software under the GNU Affero General Public License v3.0.",
                    env!("CARGO_PKG_VERSION")
                )))
                .close_button_text("OK")
                .on_closed(closed),
        }
    }
}

impl Component for MainWindow {
    type Input = Shared;
    type Message = Msg;

    fn create(input: &Self::Input, _: &ComponentContext<Self>) -> Self {
        MainWindow {
            shared: input.clone(),
            filter: "library:all".into(),
            search: String::new(),
            sort: SortKey::Added,
            descending: true,
            selected: vec![],
            anchor: None,
            last_press: None,
            details: true,
            dialog: Dialog::None,
            dismissed: 0,
        }
    }

    fn input_changed(&mut self, input: &Self::Input, _: &ComponentContext<Self>) {
        self.shared = input.clone();
        // downloads that are gone can't stay selected
        let store = self.shared.store.borrow();
        self.selected.retain(|id| store.get(id).is_some());
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Nav(tag) => {
                match tag.as_deref() {
                    // the settings entry at the bottom of the pane
                    None => self.shared.act(Action::Settings),
                    Some(key) => self.filter = key.to_string(),
                }
            }
            Msg::Search(text) => self.search = text,
            Msg::Sort(key) => {
                self.descending = self.sort == key && !self.descending;
                self.sort = key;
            }
            Msg::Press(id, info) => self.press(id, info),
            Msg::Command(key) => {
                // the details panel's buttons name their download
                if let Some((command, id)) = key.strip_prefix("detail-").and_then(|k| k.split_once(':')) {
                    self.selected = vec![id.to_string()];
                    let command = command.to_string();
                    return self.command(&command, context);
                }
                self.command(&key, context)
            }
            Msg::DialogText(text) => match &mut self.dialog {
                Dialog::Refresh(_, value, _) | Dialog::Limit(value, _) => *value = text,
                _ => {}
            },
            Msg::DialogClosed(result) => self.dialog_closed(result, context),
            Msg::Refreshed(Ok(())) => {
                if matches!(self.dialog, Dialog::Refresh(..)) {
                    self.dialog = Dialog::None;
                }
            }
            Msg::Refreshed(Err(e)) => match &mut self.dialog {
                Dialog::Refresh(_, _, note) => *note = e,
                _ => self.shared.report(e),
            },
            Msg::Imported(Ok(urls)) => match urls.as_slice() {
                [] => {}
                [only] if only.is_empty() => self.shared.report("No http(s) links found in that file."),
                many => {
                    let links: Vec<Value> = many.iter().map(|u| json!({ "url": u })).collect();
                    self.shared.act(Action::Grabber(Some(json!({ "links": links }))));
                }
            },
            Msg::Imported(Err(e)) => self.shared.report(e),
            Msg::DismissError => self.dismissed = self.shared.error.as_ref().map(|e| e.0).unwrap_or(0),
            Msg::Dropped(text) => {
                // links dragged from a browser
                let links: Vec<&str> = text.lines().map(str::trim).filter(|l| fmt::is_http(l)).collect();
                match links.as_slice() {
                    [] => {}
                    [one] => self.shared.act(Action::Add(json!({ "url": one }))),
                    many => {
                        let links: Vec<Value> = many.iter().map(|u| json!({ "url": u })).collect();
                        self.shared.act(Action::Grabber(Some(json!({ "links": links }))));
                    }
                }
            }
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        let store = self.shared.store.clone();
        let store = store.borrow();
        let active = n(&store.totals, "active");
        let title = if active > 0 {
            format!("{} — {active} active · ↓ {}", odm_ui::APP_NAME, fmt::speed(n(&store.totals, "speedBps")))
        } else {
            odm_ui::APP_NAME.to_string()
        };
        context.window_visuals(self.shared.visuals(1280.0, 820.0).constraints(WindowConstraints {
            min_width: Some(900.0),
            min_height: Some(560.0),
            max_width: None,
            max_height: None,
        }));

        let mut top: Vec<View> = vec![self.toolbar(&store, context)];
        if let Some(p) = &self.shared.power {
            let verb = match p.action.as_str() {
                "sleep" => "Sleeping",
                "shutdown" => "Shutting down",
                _ => "Quitting",
            };
            top.push(
                Grid::new()
                    .columns([GridLength::STAR, GridLength::Auto])
                    .children((
                        InfoBar::new()
                            .is_open(true)
                            .is_closable(false)
                            .severity(InfoBarSeverity::Warning)
                            .message(format!("“{}” finished. {verb} in {}s.", p.queue, p.seconds_left)),
                        Button::new()
                            .content("Cancel")
                            .grid_column(1)
                            .margin(Thickness::xy(8.0, 0.0))
                            .on_click({
                                let shared = self.shared.clone();
                                Callback::new(move |()| shared.act(Action::CancelPower))
                            }),
                    ))
                    .into(),
            );
        }
        if let Some(e) = &self.shared.start_error {
            top.push(InfoBar::new().is_open(true).is_closable(false).severity(InfoBarSeverity::Error).message(format!("The download engine failed to start: {e}")).into());
        }
        if let Some((serial, message)) = &self.shared.error {
            if *serial > self.dismissed {
                top.push(
                    InfoBar::new()
                        .is_open(true)
                        .severity(InfoBarSeverity::Error)
                        .title(odm_ui::APP_NAME)
                        .message(message)
                        .on_closed(context.callback(|()| Msg::DismissError))
                        .into(),
                );
            }
        }

        let mut rows = vec![GridLength::Auto, GridLength::Auto, GridLength::STAR];
        let mut body: Vec<View> = vec![
            StackPanel::new().children(top).into(),
            Border::new().content(self.header(context)).grid_row(1).into(),
            Border::new().content(self.list(&store, context)).grid_row(2).into(),
        ];
        if let (true, Some(d)) = (self.details, self.focused(&store)) {
            rows.push(GridLength::Auto);
            body.push(Border::new().content(self.details(&d, &store, context)).grid_row(3).into());
        }
        rows.push(GridLength::Auto);
        body.push(Border::new().content(self.status_bar(&store)).grid_row(rows.len() as i32 - 1).into());

        let content = Border::new()
            .drop_policy({
                let mut policy = DragDropPolicy::new();
                policy.text = Some(DragDropAction::new(DragDropOperation::Copy));
                policy
            })
            .on_drop(context.callback(|data: DroppedData| match data {
                DroppedData::Text(text) => Msg::Dropped(text),
                _ => Msg::Dropped(String::new()),
            }))
            .content(Grid::new().rows(rows).children(body));
        let nav = NavigationView::new()
            .pane_display_mode(NavigationViewPaneDisplayMode::Left)
            .open_pane_length(240.0)
            .is_back_button_visible(NavigationViewBackButtonVisible::Collapsed)
            .is_pane_toggle_button_visible(false)
            .is_settings_visible(false)
            .pane_header(
                AutoSuggestBox::new()
                    .placeholder_text("Search downloads, URLs, folders")
                    .query_icon(Symbol::Find)
                    .text(&self.search)
                    .on_text_changed(context.callback(|t: Rc<str>| Msg::Search(t.to_string())))
                    .margin(Thickness::xy(12.0, 8.0)),
            )
            .menu_items(self.sidebar(&store))
            .footer_menu_items((NavigationViewItem::new().tag("settings").icon(Symbol::Setting).selects_on_invoked(false).content("Settings"),))
            .on_selected_tag_changed(context.callback(|tag: Option<Rc<str>>| match tag.as_deref() {
                Some("settings") => Msg::Nav(None),
                other => Msg::Nav(other.map(String::from)),
            }))
            .content(content)
            .key_accelerators_view(context);
        context.window_frame(title, nav.content_dialog(self.dialog(context)))
    }
}

/// Keyboard shortcuts of the main window.
trait Accelerated {
    fn key_accelerators_view(self, context: &ViewContext<MainWindow>) -> View;
}

impl Accelerated for NavigationView {
    fn key_accelerators_view(self, context: &ViewContext<MainWindow>) -> View {
        let command = |key: AcceleratorKey, name: &'static str| {
            KeyAccelerator::new(key, AcceleratorModifiers::Control, context.callback(move |()| Msg::Command(name.into())))
        };
        Grid::new()
            .key_accelerators(KeyAccelerators::new([command(AcceleratorKey::N, "add"), command(AcceleratorKey::R, "resume"), command(AcceleratorKey::P, "pause")]))
            .children((self,))
            .into()
    }
}
