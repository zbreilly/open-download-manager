//! Site Grabber: list the files linked from a page (or a list of links
//! handed over by the extension, a drop or an imported file) and add the
//! chosen ones.
use crate::app::Shared;
use crate::widgets::*;
use crate::{engine, system};
use odm_ui::{fmt, n, s};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::rc::Rc;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    /// a list to show instead of scanning a page, with a serial number
    pub links: Option<(u64, Rc<Value>)>,
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

#[derive(Clone, PartialEq)]
struct Link {
    url: String,
    name: String,
    on: bool,
    /// this link's own cookies (never shared with other links)
    cookies: Option<String>,
}

pub enum Msg {
    Page(String),
    Depth(String),
    Preset(String),
    Custom(String),
    SameHost(bool),
    Filter(String),
    Find,
    Found(Result<Vec<(String, String)>, String>, String),
    Toggle(usize, bool),
    All(bool),
    Choose,
    Picked(Option<String>),
    CategoryFolders,
    Queue(String),
    /// Add to Queue (false) or Download (true)
    Add(bool),
    Added(Result<(), String>),
    Cancel,
}

pub struct GrabberWindow {
    shared: Shared,
    taken: u64,
    from_list: bool,
    page: String,
    depth: u32,
    preset: String,
    custom: String,
    same_host: bool,
    filter: String,
    links: Vec<Link>,
    dir: String,
    queue: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    busy: bool,
    error: String,
}

impl GrabberWindow {
    /// Replace the list, keeping the first entry for each URL.
    fn set_links(&mut self, links: Vec<Link>) {
        let mut seen = HashSet::new();
        self.links = links.into_iter().filter(|l| seen.insert(l.url.clone())).collect();
    }

    /// A list of links from the extension, a drop or an imported file.
    fn take(&mut self, input: &Input) {
        let Some((serial, p)) = &input.links else { return };
        if *serial == self.taken {
            return;
        }
        self.taken = *serial;
        self.from_list = true;
        self.error.clear();
        // a new list is a new context: nothing carries over from the last one
        let text = |k: &str| p.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
        self.referrer = text("referrer");
        self.user_agent = text("userAgent");
        let links = p
            .get("links")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|l| !s(l, "url").is_empty())
            .map(|l| {
                let url = s(l, "url").to_string();
                let name = fmt::url_file_name(&url);
                Link { name: if name.is_empty() { url.clone() } else { name }, url, on: true, cookies: l.get("cookies").and_then(Value::as_str).filter(|c| !c.is_empty()).map(String::from) }
            })
            .collect();
        self.set_links(links);
    }

    fn extensions(&self) -> Vec<String> {
        let preset = PRESETS.iter().find(|p| p.0 == self.preset).copied().unwrap_or(PRESETS[0]);
        odm_ui::settings::parse_extensions(if preset.0 == "custom" { &self.custom } else { preset.2 })
    }
}

impl Component for GrabberWindow {
    type Input = Input;
    type Message = Msg;

    fn create(input: &Self::Input, _: &ComponentContext<Self>) -> Self {
        let mut w = GrabberWindow {
            shared: input.shared.clone(),
            taken: 0,
            from_list: false,
            page: String::new(),
            depth: 0,
            preset: "all".into(),
            custom: String::new(),
            same_host: true,
            filter: String::new(),
            links: vec![],
            dir: String::new(),
            queue: String::new(),
            referrer: None,
            user_agent: None,
            busy: false,
            error: String::new(),
        };
        w.take(input);
        w
    }

    fn input_changed(&mut self, input: &Self::Input, _: &ComponentContext<Self>) {
        self.shared = input.shared.clone();
        self.take(input);
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Page(t) => self.page = t,
            Msg::Depth(d) => self.depth = d.parse().unwrap_or(0),
            Msg::Preset(p) => self.preset = p,
            Msg::Custom(t) => self.custom = t,
            Msg::SameHost(on) => self.same_host = on,
            Msg::Filter(t) => self.filter = t,
            Msg::Find => {
                let url = self.page.trim().to_string();
                if url.is_empty() || self.busy {
                    return;
                }
                self.busy = true;
                self.from_list = false;
                self.error.clear();
                let args = json!({ "url": url, "depth": self.depth, "extensions": self.extensions(), "sameHost": self.same_host });
                context.spawn_background(move |_| {
                    let found = engine::call("grab", args).map(|v| {
                        v.as_array().into_iter().flatten().map(|l| (s(l, "url").to_string(), s(l, "fileName").to_string())).collect()
                    });
                    Msg::Found(found, url)
                });
            }
            Msg::Found(result, page) => {
                self.busy = false;
                match result {
                    Ok(found) => {
                        if found.is_empty() {
                            self.error = "No matching files were found on that page.".into();
                        }
                        self.referrer = Some(page);
                        self.user_agent = None;
                        self.set_links(found.into_iter().map(|(url, name)| Link { url, name, on: true, cookies: None }).collect());
                    }
                    Err(e) => self.error = e,
                }
            }
            Msg::Toggle(i, on) => {
                if let Some(l) = self.links.get_mut(i) {
                    l.on = on;
                }
            }
            Msg::All(on) => self.links.iter_mut().for_each(|l| l.on = on),
            Msg::Choose => {
                let start = self.dir.clone();
                context.spawn_background(move |_| Msg::Picked(system::pick_folder(&start)));
            }
            Msg::Picked(Some(dir)) => self.dir = dir,
            Msg::Picked(None) => {}
            Msg::CategoryFolders => self.dir.clear(),
            Msg::Queue(q) => self.queue = q,
            Msg::Add(start) => {
                if self.busy || !self.links.iter().any(|l| l.on) {
                    return;
                }
                self.busy = true;
                let connections = n(&self.shared.store.borrow().settings, "defaultConnections");
                let reqs: Vec<Value> = self
                    .links
                    .iter()
                    .filter(|l| l.on)
                    .map(|l| {
                        json!({
                            "url": l.url,
                            "saveDir": if self.dir.is_empty() { Value::Null } else { self.dir.clone().into() },
                            "queueId": if !self.queue.is_empty() { Value::String(self.queue.clone()) } else if start { Value::Null } else { "main".into() },
                            "start": start,
                            "connections": connections,
                            "referrer": self.referrer,
                            "cookies": l.cookies,
                            "userAgent": self.user_agent,
                        })
                    })
                    .collect();
                context.spawn_background(move |_| Msg::Added(engine::call("add_many", json!({ "reqs": reqs })).map(|_| ())));
            }
            Msg::Added(Ok(())) => {
                self.busy = false;
                self.links.clear();
                let _ = context.close_window();
            }
            Msg::Added(Err(e)) => {
                self.busy = false;
                self.error = e;
            }
            Msg::Cancel => {
                let _ = context.close_window();
            }
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(self.shared.visuals(920.0, 680.0));
        let store = self.shared.store.clone();
        let store = store.borrow();
        let chosen = self.links.iter().filter(|l| l.on).count();
        let mut rows: Vec<View> = vec![];

        if !self.from_list {
            let presets: Vec<(&str, &str)> = PRESETS.iter().map(|p| (p.0, p.1)).collect();
            rows.push(
                Grid::new()
                    .column_spacing(8.0)
                    .columns([GridLength::STAR, GridLength::Auto, GridLength::Auto, GridLength::Auto])
                    .children((
                        TextBox::new(&self.page).placeholder_text("https://example.com/gallery/").on_text_changed(context.callback(|t: Rc<str>| Msg::Page(t.to_string()))),
                        choice(&[("0", "This page"), ("1", "+1 level"), ("2", "+2 levels")], &self.depth.to_string(), context.callback(Msg::Depth)).grid_column(1),
                        choice(&presets, &self.preset, context.callback(Msg::Preset)).grid_column(2),
                        Button::new()
                            .style(ButtonStyle::Accent)
                            .content(if self.busy { "Scanning…" } else { "Find Files" })
                            .is_enabled(!self.busy && !self.page.trim().is_empty())
                            .grid_column(3)
                            .on_click(context.callback(|()| Msg::Find)),
                    ))
                    .into(),
            );
            let mut extra: Vec<View> = vec![];
            if self.preset == "custom" {
                extra.push(TextBox::new(&self.custom).placeholder_text("jpg png zip").width(240.0).on_text_changed(context.callback(|t: Rc<str>| Msg::Custom(t.to_string()))).into());
            }
            if self.depth > 0 {
                extra.push(
                    CheckBox::new()
                        .content("Stay on the same site")
                        .is_checked(self.same_host)
                        .on_is_checked_changed(context.callback(|v: Option<bool>| Msg::SameHost(v == Some(true))))
                        .into(),
                );
            }
            if !extra.is_empty() {
                rows.push(StackPanel::new().orientation(Orientation::Horizontal).spacing(12.0).children(extra).into());
            }
        }
        if !self.error.is_empty() {
            rows.push(error_text(&self.error).into());
        }
        rows.push(
            Grid::new()
                .column_spacing(8.0)
                .columns([GridLength::Auto, GridLength::Auto, GridLength::Auto, GridLength::STAR, GridLength::Pixel(220.0)])
                .children((
                    dim(format!("{chosen} of {} selected", self.links.len())),
                    Button::new().style(ButtonStyle::TextLink).content("Select All").grid_column(1).on_click(context.callback(|()| Msg::All(true))),
                    Button::new().style(ButtonStyle::TextLink).content("None").grid_column(2).on_click(context.callback(|()| Msg::All(false))),
                    TextBox::new(&self.filter).placeholder_text("Filter").grid_column(4).on_text_changed(context.callback(|t: Rc<str>| Msg::Filter(t.to_string()))),
                ))
                .into(),
        );

        let q = self.filter.trim().to_lowercase();
        let shown: Rc<Vec<(usize, Link)>> =
            Rc::new(self.links.iter().enumerate().filter(|(_, l)| q.is_empty() || l.url.to_lowercase().contains(&q)).map(|(i, l)| (i, l.clone())).collect());
        let list: View = if shown.is_empty() {
            dim(if self.busy { "Scanning…" } else { "Files found on the page appear here." })
                .horizontal_alignment(HorizontalAlignment::Center)
                .into()
        } else {
            let keys = shown.clone();
            let sender = context.sender();
            // the revision follows membership, so the list knows when rows moved
            let revision = shown.iter().fold(shown.len() as u64, |h, (i, _)| h.wrapping_mul(31).wrapping_add(*i as u64));
            let source = VirtualSource::new(
                revision,
                shown.len(),
                move |i| keys[i].1.url.clone(),
                move |i| {
                    let (index, link) = shown[i].clone();
                    Grid::new().column_spacing(10.0).height(32.0).columns([GridLength::Pixel(32.0), GridLength::Pixel(280.0), GridLength::STAR]).children((
                        CheckBox::new().is_checked(link.on).on_is_checked_changed(sender.callback(move |v: Option<bool>| Msg::Toggle(index, v == Some(true)))),
                        text(&link.name).grid_column(1),
                        dim(&link.url).font_size(12.0).grid_column(2),
                    ))
                },
            );
            ScrollViewer::new().content(ItemsRepeater::new().virtual_source(source)).into()
        };

        let mut queues: Vec<(String, String)> = vec![(String::new(), "None (start now)".to_string())];
        queues.extend(store.queues.iter().map(|q| (s(q, "id").to_string(), s(q, "name").to_string())));
        let pairs: Vec<(&str, &str)> = queues.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let mut target: Vec<View> = vec![
            dim("Save to").into(),
            text(if self.dir.is_empty() { "Category folders" } else { &self.dir }).into(),
            Button::new().content("Choose…").on_click(context.callback(|()| Msg::Choose)).into(),
        ];
        if !self.dir.is_empty() {
            target.push(Button::new().content("Use Category Folders").on_click(context.callback(|()| Msg::CategoryFolders)).into());
        }
        target.push(dim("Queue").into());
        target.push(choice(&pairs, &self.queue, context.callback(Msg::Queue)).into());

        let ready = chosen > 0 && !self.busy;
        let root = Grid::new()
            .margin(Thickness::xy(16.0, 12.0))
            .row_spacing(10.0)
            .rows([GridLength::Auto, GridLength::STAR, GridLength::Auto, GridLength::Auto])
            .children((
                StackPanel::new().spacing(8.0).children(rows),
                Border::new().grid_row(1).border_brush(ThemeBrush::CardStroke).border_thickness(1.0).corner_radius(4.0).padding(Thickness::xy(8.0, 4.0)).content(list),
                StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).grid_row(2).children(target),
                Grid::new().grid_row(3).column_spacing(8.0).columns([GridLength::Auto, GridLength::STAR, GridLength::Auto, GridLength::Auto]).children((
                    Button::new().content("Add to Queue").is_enabled(ready).on_click(context.callback(|()| Msg::Add(false))),
                    Button::new().content("Cancel").grid_column(2).on_click(context.callback(|()| Msg::Cancel)),
                    Button::new().style(ButtonStyle::Accent).content(format!("Download {chosen}")).is_enabled(ready).grid_column(3).on_click(context.callback(|()| Msg::Add(true))),
                )),
            ));
        context.window_frame("Site Grabber", root)
    }
}
