//! Site Grabber: list the files linked from a page (or a list of links
//! handed over by the extension, a drop or an imported file) and add the
//! chosen ones.
use crate::app::{odm, call, App};
use crate::row::s;
use crate::widgets::{self, Choice};
use adw::prelude::*;
use gtk::glib;
use gtk::glib::subclass::prelude::*;
use gtk::{gio, glib::Properties};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::Link)]
    pub struct Link {
        #[property(get, set)]
        pub url: RefCell<String>,
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub on: Cell<bool>,
        /// this link's own cookies (never shared with other links)
        #[property(get, set, nullable)]
        pub cookies: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Link {
        const NAME: &'static str = "OdmLink";
        type Type = super::Link;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Link {}
}

glib::wrapper! {
    pub struct Link(ObjectSubclass<imp::Link>);
}

impl Link {
    fn new(url: &str, name: &str, cookies: Option<&str>) -> Self {
        glib::Object::builder().property("url", url).property("name", name).property("on", true).property("cookies", cookies).build()
    }
}

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

struct Grabber {
    window: adw::Window,
    scan: gtk::Box,
    page: gtk::Entry,
    depth: gtk::DropDown,
    types: gtk::DropDown,
    find: gtk::Button,
    custom: gtk::Entry,
    same_host: gtk::CheckButton,
    error: gtk::Label,
    count: gtk::Label,
    links: gio::ListStore,
    filter: gtk::CustomFilter,
    filter_text: RefCell<String>,
    stack: gtk::Stack,
    placeholder: adw::StatusPage,
    dir_label: gtk::Label,
    dir: RefCell<String>,
    use_categories: gtk::Button,
    queue: Choice,
    add_queue: gtk::Button,
    download: gtk::Button,
    referrer: RefCell<Option<String>>,
    user_agent: RefCell<Option<String>>,
    busy: Cell<bool>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Grabber>>> = const { RefCell::new(None) };
}

/// Show the grabber. With `links` (a `show_links` payload) it lists those
/// instead of scanning a page.
pub fn open(app: &Rc<App>, links: Option<Value>) {
    let app2 = app.clone();
    app.single("grabber", move || build(&app2).upcast());
    if let (Some(p), Some(g)) = (links, OPEN.with(|o| o.borrow().clone())) {
        g.take_list(&p);
    }
}

fn build(app: &Rc<App>) -> adw::Window {
    let window = adw::Window::builder().application(&app.gtk).title("Site Grabber").default_width(900).default_height(660).build();
    let queue = Choice::new("Queue", &[]);
    let mut queues = vec![(String::new(), "None (start now)".to_string())];
    queues.extend(app.queues.borrow().iter().map(|q| (s(q, "id").to_string(), s(q, "name").to_string())));
    queue.set_items(queues);
    let g = Rc::new(Grabber {
        window: window.clone(),
        scan: gtk::Box::new(gtk::Orientation::Vertical, 6),
        page: gtk::Entry::builder().placeholder_text("https://example.com/gallery/").hexpand(true).input_purpose(gtk::InputPurpose::Url).build(),
        depth: gtk::DropDown::from_strings(&["This page", "+1 level", "+2 levels"]),
        types: gtk::DropDown::from_strings(&PRESETS.map(|p| p.1)),
        find: gtk::Button::with_label("Find Files"),
        custom: gtk::Entry::builder().placeholder_text("jpg png zip").width_chars(24).visible(false).build(),
        same_host: gtk::CheckButton::builder().label("Stay on the same site").active(true).visible(false).build(),
        error: gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).visible(false).build(),
        count: gtk::Label::new(None),
        links: gio::ListStore::new::<Link>(),
        filter: gtk::CustomFilter::new(|_| true),
        filter_text: RefCell::default(),
        stack: gtk::Stack::new(),
        placeholder: adw::StatusPage::builder().icon_name("folder-download-symbolic").title("Files found on the page appear here.").build(),
        dir_label: widgets::value_label(),
        dir: RefCell::default(),
        use_categories: gtk::Button::builder().label("Use Category Folders").visible(false).build(),
        queue,
        add_queue: gtk::Button::with_label("Add to Queue"),
        download: gtk::Button::new(),
        referrer: RefCell::default(),
        user_agent: RefCell::default(),
        busy: Cell::new(false),
    });
    g.error.add_css_class("error");
    g.count.add_css_class("dim-label");
    g.find.add_css_class("suggested-action");
    g.download.add_css_class("suggested-action");
    g.page.add_css_class("monospace");

    // -------------------------------------------------------------- layout
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&g.page);
    top.append(&g.depth);
    top.append(&g.types);
    top.append(&g.find);
    let extra = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    extra.append(&g.custom);
    extra.append(&g.same_host);
    g.scan.append(&top);
    g.scan.append(&extra);

    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let all = gtk::Button::with_label("Select All");
    let none = gtk::Button::with_label("None");
    all.add_css_class("flat");
    none.add_css_class("flat");
    let filter_entry = gtk::SearchEntry::builder().placeholder_text("Filter").width_chars(22).build();
    tools.append(&g.count);
    tools.append(&all);
    tools.append(&none);
    tools.append(&gtk::Box::builder().hexpand(true).build());
    tools.append(&filter_entry);

    let filtered = gtk::FilterListModel::new(Some(g.links.clone()), Some(g.filter.clone()));
    let view = gtk::ColumnView::builder().model(&gtk::NoSelection::new(Some(filtered))).build();
    let check = gtk::SignalListItemFactory::new();
    check.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        item.set_child(Some(&gtk::CheckButton::new()));
    });
    let this = g.clone();
    check.connect_bind(move |_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let (Some(link), Some(btn)) = (item.item().and_downcast::<Link>(), item.child().and_downcast::<gtk::CheckButton>()) else { return };
        btn.set_active(link.on());
        let binding = link.bind_property("on", &btn, "active").bidirectional().build();
        let this = this.clone();
        let handler = btn.connect_toggled(move |_| this.refresh());
        // undone when the cell is reused for another link
        unsafe {
            item.set_data("binding", binding);
            item.set_data("handler", handler);
        }
    });
    check.connect_unbind(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        unsafe {
            if let Some(b) = item.steal_data::<glib::Binding>("binding") {
                b.unbind();
            }
            if let (Some(h), Some(btn)) = (item.steal_data::<glib::SignalHandlerId>("handler"), item.child()) {
                btn.disconnect(h);
            }
        }
    });
    view.append_column(&gtk::ColumnViewColumn::builder().factory(&check).fixed_width(40).build());
    let text = |prop: &'static str, dim: bool| {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
            if dim {
                label.add_css_class("dim-label");
                label.add_css_class("caption");
            }
            item.property_expression("item").chain_property::<Link>(prop).bind(&label, "label", gtk::Widget::NONE);
            item.set_child(Some(&label));
        });
        f
    };
    view.append_column(&gtk::ColumnViewColumn::builder().title("Name").factory(&text("name", false)).fixed_width(280).resizable(true).build());
    view.append_column(&gtk::ColumnViewColumn::builder().title("Address").factory(&text("url", true)).expand(true).build());
    g.stack.add_named(&gtk::ScrolledWindow::builder().child(&view).vexpand(true).build(), Some("list"));
    g.stack.add_named(&g.placeholder, Some("empty"));

    let target = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let save_to = gtk::Label::new(Some("Save to"));
    save_to.add_css_class("dim-label");
    let choose = gtk::Button::with_label("Choose…");
    g.dir_label.set_hexpand(true);
    target.append(&save_to);
    target.append(&g.dir_label);
    target.append(&choose);
    target.append(&g.use_categories);
    let queue_drop = gtk::DropDown::builder().model(&g.queue.row.model().expect("queue model")).build();
    g.queue.row.bind_property("selected", &queue_drop, "selected").bidirectional().sync_create().build();
    let queue_label = gtk::Label::new(Some("Queue"));
    queue_label.add_css_class("dim-label");
    target.append(&queue_label);
    target.append(&queue_drop);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cancel = gtk::Button::with_label("Cancel");
    bar.append(&g.add_queue);
    bar.append(&gtk::Box::builder().hexpand(true).build());
    bar.append(&cancel);
    bar.append(&g.download);

    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).margin_top(12).margin_bottom(12).margin_start(14).margin_end(14).build();
    body.append(&g.scan);
    body.append(&g.error);
    body.append(&tools);
    let frame = gtk::Frame::builder().child(&g.stack).vexpand(true).build();
    body.append(&frame);
    body.append(&target);
    body.append(&bar);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&body));
    window.set_content(Some(&tv));
    window.set_default_widget(Some(&g.find));
    g.page.set_activates_default(true);

    // -------------------------------------------------------------- wiring
    let weak = Rc::downgrade(&g);
    g.filter.set_filter_func(move |obj| {
        let (Some(g), Some(link)) = (weak.upgrade(), obj.downcast_ref::<Link>()) else { return true };
        let q = g.filter_text.borrow();
        q.is_empty() || link.url().to_lowercase().contains(q.as_str())
    });
    let this = g.clone();
    filter_entry.connect_search_changed(move |e| {
        *this.filter_text.borrow_mut() = e.text().to_lowercase();
        this.filter.changed(gtk::FilterChange::Different);
    });
    let this = g.clone();
    all.connect_clicked(move |_| this.set_all(true));
    let this = g.clone();
    none.connect_clicked(move |_| this.set_all(false));
    let this = g.clone();
    g.find.connect_clicked(move |_| this.run());
    let this = g.clone();
    g.types.connect_selected_notify(move |_| this.refresh());
    let this = g.clone();
    g.depth.connect_selected_notify(move |_| this.refresh());
    let this = g.clone();
    g.page.connect_changed(move |_| this.refresh());
    let this = g.clone();
    choose.connect_clicked(move |_| {
        let this = this.clone();
        glib::spawn_future_local(async move {
            let start = this.dir.borrow().clone();
            if let Some(p) = widgets::choose_folder(&this.window, &start).await {
                *this.dir.borrow_mut() = p;
                this.refresh();
            }
        });
    });
    let this = g.clone();
    g.use_categories.connect_clicked(move |_| {
        this.dir.borrow_mut().clear();
        this.refresh();
    });
    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    let this = g.clone();
    g.add_queue.connect_clicked(move |_| this.add(false));
    let this = g.clone();
    g.download.connect_clicked(move |_| this.add(true));
    let esc = gtk::ShortcutController::new();
    esc.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("Escape"), Some(gtk::NamedAction::new("window.close"))));
    window.add_controller(esc);
    window.connect_close_request(|_| {
        OPEN.with(|o| o.borrow_mut().take());
        glib::Propagation::Proceed
    });
    OPEN.with(|o| *o.borrow_mut() = Some(g.clone()));
    g.refresh();
    window
}

impl Grabber {
    fn all(&self) -> Vec<Link> {
        (0..self.links.n_items()).filter_map(|i| self.links.item(i).and_downcast::<Link>()).collect()
    }

    fn chosen(&self) -> Vec<Link> {
        self.all().into_iter().filter(Link::on).collect()
    }

    fn set_all(&self, on: bool) {
        for l in self.all() {
            l.set_on(on);
        }
        self.refresh();
    }

    fn extensions(&self) -> Vec<String> {
        let preset = PRESETS.get(self.types.selected() as usize).copied().unwrap_or(PRESETS[0]);
        let source = if preset.0 == "custom" { self.custom.text().to_string() } else { preset.2.to_string() };
        source.split(|c: char| c.is_whitespace() || c == ',').filter(|e| !e.is_empty()).map(String::from).collect()
    }

    fn fail(&self, msg: &str) {
        self.error.set_label(msg);
        self.error.set_visible(!msg.is_empty());
    }

    fn refresh(&self) {
        let total = self.links.n_items();
        let chosen = self.chosen().len();
        let busy = self.busy.get();
        self.count.set_label(&format!("{chosen} of {total} selected"));
        self.download.set_label(&format!("Download {chosen}"));
        self.download.set_sensitive(chosen > 0 && !busy);
        self.add_queue.set_sensitive(chosen > 0 && !busy);
        self.find.set_label(if busy { "Scanning…" } else { "Find Files" });
        self.find.set_sensitive(!busy && !self.page.text().trim().is_empty());
        self.custom.set_visible(PRESETS.get(self.types.selected() as usize).map(|p| p.0) == Some("custom"));
        self.same_host.set_visible(self.depth.selected() > 0);
        let dir = self.dir.borrow();
        self.dir_label.set_label(if dir.is_empty() { "Category folders" } else { dir.as_str() });
        self.use_categories.set_visible(!dir.is_empty());
        self.placeholder.set_title(if busy { "Scanning…" } else { "Files found on the page appear here." });
        self.stack.set_visible_child_name(if total == 0 { "empty" } else { "list" });
    }

    /// Replace the list, keeping the first entry for each URL.
    fn set_links(&self, links: Vec<Link>) {
        let mut seen = HashSet::new();
        self.links.remove_all();
        for l in links {
            if seen.insert(l.url()) {
                self.links.append(&l);
            }
        }
        self.refresh();
    }

    /// A list of links from the extension, a drop or an imported file.
    fn take_list(&self, p: &Value) {
        self.scan.set_visible(false);
        self.fail("");
        // a new list is a new context: nothing carries over from the last one
        let text = |k: &str| p.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
        *self.referrer.borrow_mut() = text("referrer");
        *self.user_agent.borrow_mut() = text("userAgent");
        let links = p
            .get("links")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|l| !s(l, "url").is_empty())
            .map(|l| {
                let url = s(l, "url");
                let name = crate::add::url_file_name(url);
                let cookies = l.get("cookies").and_then(Value::as_str).filter(|c| !c.is_empty());
                Link::new(url, if name.is_empty() { url } else { &name }, cookies)
            })
            .collect();
        self.set_links(links);
    }

    fn run(self: &Rc<Self>) {
        let url = self.page.text().trim().to_string();
        if url.is_empty() || self.busy.replace(true) {
            return;
        }
        self.fail("");
        self.refresh();
        let args = json!({
            "url": url,
            "depth": self.depth.selected(),
            "extensions": self.extensions(),
            "sameHost": self.same_host.is_active(),
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let found = call("grab", args).await;
            this.busy.set(false);
            match found {
                Ok(Value::Array(list)) => {
                    if list.is_empty() {
                        this.fail("No matching files were found on that page.");
                    }
                    *this.referrer.borrow_mut() = Some(url);
                    *this.user_agent.borrow_mut() = None;
                    this.set_links(list.iter().map(|l| Link::new(s(l, "url"), s(l, "fileName"), None)).collect());
                }
                Ok(_) => this.fail("The engine sent an unexpected reply."),
                Err(e) => this.fail(&e),
            }
            this.refresh();
        });
    }

    fn add(self: &Rc<Self>, start: bool) {
        let chosen = self.chosen();
        if chosen.is_empty() || self.busy.replace(true) {
            return;
        }
        let dir = self.dir.borrow().clone();
        let queue = self.queue.value();
        let connections = odm().settings.borrow().get("defaultConnections").cloned().unwrap_or(Value::Null);
        let reqs: Vec<Value> = chosen
            .iter()
            .map(|l| {
                json!({
                    "url": l.url(),
                    "saveDir": if dir.is_empty() { Value::Null } else { dir.clone().into() },
                    "queueId": if !queue.is_empty() { Value::String(queue.clone()) } else if start { Value::Null } else { "main".into() },
                    "start": start,
                    "connections": connections,
                    "referrer": *self.referrer.borrow(),
                    "cookies": l.cookies(),
                    "userAgent": *self.user_agent.borrow(),
                })
            })
            .collect();
        self.refresh();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let added = call("add_many", json!({ "reqs": reqs })).await;
            this.busy.set(false);
            match added {
                Ok(_) => this.window.close(),
                Err(e) => {
                    this.fail(&e);
                    this.refresh();
                }
            }
        });
    }
}
