//! The Site Grabber: find the files a page links to, or take a list of
//! links (from the browser extension, a dropped selection, an imported
//! file), and add the chosen ones.
use crate::app::app;
use crate::kit::{self, ns, KeyTable, TableCallbacks};
use crate::{engine, system};
use objc2::rc::Retained;
use objc2_app_kit::*;
use odm_ui::{fmt, n, s};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// `(key, title, extensions)`
const PRESETS: [(&str, &str, &str); 8] = [
    ("all", "All files", ""),
    ("images", "Images", "jpg jpeg png gif webp bmp svg tif tiff avif heic"),
    ("video", "Video", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts"),
    ("audio", "Audio", "mp3 wav flac aac m4a ogg opus wma"),
    ("documents", "Documents", "pdf doc docx xls xlsx ppt pptx odt ods txt epub csv"),
    ("archives", "Archives", "zip rar 7z tar gz tgz bz2 xz zst"),
    ("programs", "Programs", "exe msi dmg pkg deb rpm appimage apk iso"),
    ("custom", "Custom…", ""),
];

#[derive(Clone)]
struct Link {
    url: String,
    name: String,
    ext: String,
    on: bool,
    /// this link's own cookies (never shared with other links)
    cookies: Option<String>,
}

pub struct GrabberWindow {
    window: Retained<NSWindow>,
    links: RefCell<Vec<Link>>,
    /// the rows shown: indexes into `links` that match the filter
    visible: RefCell<Vec<usize>>,
    /// showing a list handed to the app instead of a scanned page
    from_list: Cell<bool>,
    busy: Cell<bool>,
    referrer: RefCell<Option<String>>,
    user_agent: RefCell<Option<String>>,
    save_dir: RefCell<String>,
    queues: RefCell<Vec<String>>,

    scan_row: Retained<NSStackView>,
    options_row: Retained<NSStackView>,
    page: Retained<NSTextField>,
    depth: Retained<NSPopUpButton>,
    preset: Retained<NSPopUpButton>,
    custom: Retained<NSTextField>,
    same_host: Retained<NSButton>,
    find: Retained<NSButton>,
    error: Retained<NSTextField>,
    count: Retained<NSTextField>,
    filter: Retained<NSTextField>,
    table: Retained<KeyTable>,
    empty: Retained<NSTextField>,
    folder: Retained<NSTextField>,
    use_categories: Retained<NSButton>,
    queue: Retained<NSPopUpButton>,
    add_button: Retained<NSButton>,
    download_button: Retained<NSButton>,
}

/// Keep the first link for each address.
fn dedupe(links: Vec<Link>) -> Vec<Link> {
    let mut seen = std::collections::HashSet::new();
    links.into_iter().filter(|l| seen.insert(l.url.clone())).collect()
}

fn non_empty(v: &Value, key: &str) -> Option<String> {
    Some(s(v, key).to_string()).filter(|x| !x.is_empty())
}

impl GrabberWindow {
    pub fn new() -> Rc<GrabberWindow> {
        Rc::new_cyclic(|weak: &Weak<GrabberWindow>| {
            let window = kit::window("Site Grabber", 880.0, 640.0, (680.0, 500.0), true);
            let sync = kit::bound(weak, |w| w.sync());

            let page = kit::text_field("https://example.com/gallery/", sync.clone());
            kit::mono(&page);
            kit::on_action(&page, kit::bound(weak, |w| w.scan()));
            let depth = kit::popup(&["This page".to_string(), "+1 level".into(), "+2 levels".into()], sync.clone());
            let titles: Vec<String> = PRESETS.iter().map(|p| p.1.to_string()).collect();
            let preset = kit::popup(&titles, sync.clone());
            let find = kit::button("Find Files", kit::bound(weak, |w| w.scan()));
            kit::default_button(&find);
            let scan_row = kit::hstack(8.0, &[&page, &depth, &preset, &find]);
            let custom = kit::text_field("jpg png zip", || {});
            kit::width(&custom, 240.0);
            let same_host = kit::checkbox("Stay on the same site", || {});
            kit::set_on(&same_host, true);
            let options_row = kit::hstack(10.0, &[&custom, &same_host, &kit::spacer()]);

            let error = kit::wrapping("");
            kit::red(&error);
            unsafe { error.setSelectable(true) };

            let count = kit::dim("");
            let all = kit::button("Select All", kit::bound(weak, |w| w.set_all(true)));
            let none = kit::button("None", kit::bound(weak, |w| w.set_all(false)));
            for b in [&all, &none] {
                unsafe {
                    b.setBordered(false);
                    b.setContentTintColor(Some(&NSColor::linkColor()));
                }
            }
            let filter = kit::text_field("Filter", kit::bound(weak, |w| w.reload()));
            kit::width(&filter, 200.0);
            let pick_row = kit::hstack(10.0, &[&count, &all, &none, &kit::spacer(), &filter]);

            let (scroll, table) = kit::table(
                &[("on", " ", 26.0, 26.0, false), ("name", "Name", 240.0, 120.0, false), ("address", "Address", 520.0, 160.0, false)],
                24.0,
                TableCallbacks {
                    rows: Box::new({
                        let w = weak.clone();
                        move || w.upgrade().map(|w| w.visible.borrow().len()).unwrap_or(0)
                    }),
                    cell: Box::new({
                        let w = weak.clone();
                        move |table, column, row| w.upgrade().and_then(|w| w.cell(table, column, row))
                    }),
                    ..Default::default()
                },
            );
            unsafe {
                scroll.setBorderType(NSBorderType::BezelBorder);
                table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
            }
            scroll.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
            let empty = kit::dim("Files found on the page appear here.");
            let list = kit::overlay(&scroll, &empty);

            let folder = kit::label("Category folders");
            kit::mono(&folder);
            let choose = kit::button("Choose…", kit::bound(weak, |w| {
                let start = w.save_dir.borrow().clone();
                if let Some(dir) = kit::choose_folder(&start) {
                    *w.save_dir.borrow_mut() = dir;
                    w.sync();
                }
            }));
            let use_categories = kit::button("Use Category Folders", kit::bound(weak, |w| {
                w.save_dir.borrow_mut().clear();
                w.sync();
            }));
            let queue = kit::popup(&[], || {});
            let where_row = kit::hstack(8.0, &[&kit::dim("Save to"), &folder, &choose, &use_categories, &kit::spacer(), &kit::label("Queue"), &queue]);

            let add_button = kit::button("Add to Queue", kit::bound(weak, |w| w.add(false)));
            let cancel = kit::button("Cancel", kit::bound(weak, |w| w.window.performClose(None)));
            kit::cancel_button(&cancel);
            let download_button = kit::button("Download", kit::bound(weak, |w| w.add(true)));
            let buttons = kit::hstack(10.0, &[&add_button, &kit::spacer(), &cancel, &download_button]);

            let root = kit::vfill_in(10.0, (14.0, 14.0, 14.0, 14.0), &[&scan_row, &options_row, &error, &pick_row, &list, &where_row, &buttons]);
            unsafe {
                window.setContentView(Some(&root));
                window.setFrameAutosaveName(&ns("ODMGrabberWindow"));
            }

            GrabberWindow {
                window,
                links: RefCell::new(vec![]),
                visible: RefCell::new(vec![]),
                from_list: Cell::new(false),
                busy: Cell::new(false),
                referrer: RefCell::new(None),
                user_agent: RefCell::new(None),
                save_dir: RefCell::new(String::new()),
                queues: RefCell::new(vec![]),
                scan_row,
                options_row,
                page,
                depth,
                preset,
                custom,
                same_host,
                find,
                error,
                count,
                filter,
                table,
                empty,
                folder,
                use_categories,
                queue,
                add_button,
                download_button,
            }
        })
    }

    /// Bring the window up, optionally with a list of links to choose from
    /// (`{links: [{url, cookies}], referrer, userAgent}`).
    pub fn show(&self, list: Option<Value>) {
        if let Some(p) = list {
            self.from_list.set(true);
            kit::set_text(&self.error, "");
            // a new list is a new context: nothing carries over from the last
            *self.referrer.borrow_mut() = non_empty(&p, "referrer");
            *self.user_agent.borrow_mut() = non_empty(&p, "userAgent");
            let links = p.get("links").and_then(Value::as_array).cloned().unwrap_or_default();
            *self.links.borrow_mut() = dedupe(
                links
                    .iter()
                    .filter(|l| !s(l, "url").is_empty())
                    .map(|l| {
                        let url = s(l, "url").to_string();
                        let name = Some(fmt::url_file_name(&url)).filter(|n| !n.is_empty()).unwrap_or_else(|| url.clone());
                        Link { ext: fmt::extension(&name), name, cookies: non_empty(l, "cookies"), on: true, url }
                    })
                    .collect(),
            );
        }
        self.lists_changed();
        self.reload();
        kit::present(&self.window);
    }

    /// Queues changed: refill the pop-up.
    pub fn lists_changed(&self) {
        let app = app();
        let st = app.store.borrow();
        let current = self.queues.borrow().get(kit::popup_index(&self.queue)).cloned().unwrap_or_default();
        let mut ids = vec![String::new()];
        let mut titles = vec!["None (start now)".to_string()];
        for q in &st.queues {
            ids.push(s(q, "id").to_string());
            titles.push(s(q, "name").to_string());
        }
        let selected = ids.iter().position(|q| *q == current).unwrap_or(0);
        kit::set_popup(&self.queue, &titles, selected);
        *self.queues.borrow_mut() = ids;
    }

    fn cell(self: &Rc<Self>, table: &NSTableView, column: &str, row: usize) -> Option<Retained<NSView>> {
        let index = *self.visible.borrow().get(row)?;
        let links = self.links.borrow();
        let link = links.get(index)?;
        if column == "on" {
            let weak = Rc::downgrade(self);
            let url = link.url.clone();
            let check = kit::checkbox("", move || {
                let Some(w) = weak.upgrade() else { return };
                if let Some(l) = w.links.borrow_mut().iter_mut().find(|l| l.url == url) {
                    l.on = !l.on;
                }
                w.sync();
            });
            kit::set_on(&check, link.on);
            return Some(kit::view(&check));
        }
        let (cell, label, image) = kit::text_cell(table, column, column == "name");
        if column == "name" {
            kit::set_text(&label, &link.name);
            if let Some(image) = image {
                unsafe { image.setImage(Some(&system::file_icon(&link.ext))) };
            }
        } else {
            kit::set_text(&label, &link.url);
            kit::mono(&label);
            unsafe { label.setTextColor(Some(&NSColor::secondaryLabelColor())) };
        }
        Some(kit::view(&cell))
    }

    /// The list or the filter changed.
    fn reload(&self) {
        let filter = kit::text_of(&self.filter).to_lowercase();
        *self.visible.borrow_mut() = self.links.borrow().iter().enumerate().filter(|(_, l)| filter.is_empty() || l.url.to_lowercase().contains(&filter)).map(|(i, _)| i).collect();
        self.table.reload();
        self.sync();
    }

    fn set_all(&self, on: bool) {
        for l in self.links.borrow_mut().iter_mut() {
            l.on = on;
        }
        self.reload();
    }

    fn sync(&self) {
        let (chosen, total) = {
            let links = self.links.borrow();
            (links.iter().filter(|l| l.on).count(), links.len())
        };
        let busy = self.busy.get();
        let from_list = self.from_list.get();
        let depth = kit::popup_index(&self.depth);
        let custom = PRESETS.get(kit::popup_index(&self.preset)).map(|p| p.0 == "custom").unwrap_or(false);
        kit::show(&self.scan_row, !from_list);
        kit::show(&self.options_row, !from_list && (custom || depth > 0));
        kit::show(&self.custom, custom);
        kit::show(&self.same_host, depth > 0);
        unsafe { self.find.setTitle(&ns(if busy { "Scanning…" } else { "Find Files" })) };
        kit::enable(&self.find, !busy && !kit::text_of(&self.page).trim().is_empty());
        kit::show(&self.error, !kit::text_of(&self.error).is_empty());
        kit::set_text(&self.count, &format!("{chosen} of {total} selected"));
        kit::show(&self.empty, total == 0);
        kit::set_text(&self.empty, if busy { "Scanning…" } else { "Files found on the page appear here." });
        let dir = self.save_dir.borrow();
        kit::set_text(&self.folder, if dir.is_empty() { "Category folders" } else { &dir });
        kit::show(&self.use_categories, !dir.is_empty());
        kit::enable(&self.add_button, chosen > 0 && !busy);
        kit::enable(&self.download_button, chosen > 0 && !busy);
        unsafe { self.download_button.setTitle(&ns(&format!("Download {chosen}"))) };
    }

    /// Look for files on the page.
    fn scan(self: &Rc<Self>) {
        let url = kit::text_of(&self.page).trim().to_string();
        if url.is_empty() || self.busy.get() {
            return;
        }
        let preset = PRESETS.get(kit::popup_index(&self.preset)).copied().unwrap_or(PRESETS[0]);
        let source = if preset.0 == "custom" { kit::text_of(&self.custom) } else { preset.2.to_string() };
        let extensions: Vec<String> = source.split(|c: char| c.is_whitespace() || c == ',').filter(|e| !e.is_empty()).map(String::from).collect();
        let args = json!({ "url": url, "depth": kit::popup_index(&self.depth), "extensions": extensions, "sameHost": kit::is_on(&self.same_host) });
        self.busy.set(true);
        self.from_list.set(false);
        kit::set_text(&self.error, "");
        self.sync();
        let weak = Rc::downgrade(self);
        engine::spawn("grab", args, move |result| {
            let Some(w) = weak.upgrade() else { return };
            w.busy.set(false);
            match result {
                Ok(found) => {
                    let found = found.as_array().cloned().unwrap_or_default();
                    *w.links.borrow_mut() = dedupe(
                        found.iter().map(|l| Link { url: s(l, "url").into(), name: s(l, "fileName").into(), ext: s(l, "ext").to_lowercase(), on: true, cookies: None }).collect(),
                    );
                    *w.referrer.borrow_mut() = Some(url);
                    *w.user_agent.borrow_mut() = None;
                    if found.is_empty() {
                        kit::set_text(&w.error, "No matching files were found on that page.");
                    }
                }
                Err(e) => kit::set_text(&w.error, &e),
            }
            w.reload();
        });
    }

    fn add(self: &Rc<Self>, start: bool) {
        let queue = self.queues.borrow().get(kit::popup_index(&self.queue)).cloned().unwrap_or_default();
        let queue_id = if !queue.is_empty() {
            Value::String(queue)
        } else if start {
            Value::Null
        } else {
            "main".into()
        };
        let dir = Some(self.save_dir.borrow().clone()).filter(|d| !d.is_empty());
        let connections = n(&app().store.borrow().settings, "defaultConnections").max(1);
        let requests: Vec<Value> = self
            .links
            .borrow()
            .iter()
            .filter(|l| l.on)
            .map(|l| {
                json!({
                    "url": l.url, "saveDir": dir, "queueId": queue_id, "start": start, "connections": connections,
                    "referrer": &*self.referrer.borrow(), "cookies": l.cookies, "userAgent": &*self.user_agent.borrow(),
                })
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        self.busy.set(true);
        self.sync();
        let weak = Rc::downgrade(self);
        engine::spawn("add_many", json!({ "reqs": requests }), move |result| {
            let Some(w) = weak.upgrade() else { return };
            w.busy.set(false);
            match result {
                Ok(_) => w.window.performClose(None),
                Err(e) => kit::set_text(&w.error, &e),
            }
            w.sync();
        });
    }
}
