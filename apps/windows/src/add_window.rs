//! The Add Download window (also used for links handed over by the
//! browser extension, the clipboard monitor and `odm://` links). The form's
//! rules live in `odm_ui::add`; this draws it.
use crate::app::{Action, Shared};
use crate::widgets::*;
use crate::{engine, system};
use odm_ui::add::{AddForm, Info, CONNECTIONS};
use odm_ui::{n, s};
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::Duration;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    pub key: u64,
    pub params: Rc<Value>,
}

/// A text field of the form.
#[derive(Clone, Copy, PartialEq)]
pub enum Field {
    Url,
    BatchText,
    Name,
    Dir,
    Checksum,
    Username,
    Password,
    Referrer,
    Cookies,
    UserAgent,
    HeaderName(usize),
    HeaderValue(usize),
}

/// A switch of the form.
#[derive(Clone, Copy, PartialEq)]
pub enum Flag {
    Batch,
    Always,
    Remember,
    Verify,
    Advanced,
}

pub enum Msg {
    Text(Field, String),
    Flag(Flag, bool),
    Category(String),
    Queue(String),
    Connections(String),
    AddHeader,
    RemoveHeader(usize),
    /// the pause after typing is over (generation)
    Settled(u32),
    Probed(u32, Result<Value, String>),
    Expanded(u32, Vec<String>),
    Check,
    Browse,
    Picked(Option<String>),
    /// Add to Queue (false) or Start (true)
    Submit(bool),
    /// the answer to "already exists"
    Exists(ContentDialogResult),
    Overwrite,
    Added(Result<Option<String>, String>),
    Cancel,
}

pub struct AddWindow {
    shared: Shared,
    key: u64,
    form: AddForm,
    recents: Vec<String>,
    advanced: bool,
    /// bumped to drop the result of an older probe
    generation: u32,
    busy: bool,
    error: String,
    /// asking what to do about an existing file; holds "start"
    exists: Option<bool>,
    /// the pause after typing
    timer: Option<ComponentTimer>,
}

impl Drop for AddWindow {
    fn drop(&mut self) {
        self.shared.act(Action::AddDone(self.key));
    }
}

fn folder_for(category: &str) -> Option<String> {
    engine::call("folder_for", json!({ "category": category })).ok().and_then(|v| v.as_str().map(String::from))
}

impl AddWindow {
    fn default_connections(&self) -> u64 {
        n(&self.shared.store.borrow().settings, "defaultConnections")
    }

    /// The input changed: expand the batch, or probe the link, after a
    /// short pause.
    fn schedule(&mut self, context: &ComponentContext<Self>) {
        self.generation = self.generation.wrapping_add(1);
        if !self.form.batch {
            self.form.link_changed();
        }
        if self.form.batch || self.form.wants_probe() {
            // kept: dropping a timer cancels it
            self.timer = Some(context.set_timeout(Duration::from_millis(300), Msg::Settled(self.generation)));
        }
    }

    fn probe(&mut self, context: &ComponentContext<Self>) {
        let (generation, link, opts) = (self.generation, self.form.link().to_string(), self.form.probe_opts());
        self.form.probing = true;
        context.spawn_background(move |_| Msg::Probed(generation, engine::call("probe_url", json!({ "url": link, "opts": opts }))));
    }

    fn expand(&mut self, context: &ComponentContext<Self>) {
        let (generation, lines) = (self.generation, self.form.batch_lines());
        context.spawn_background(move |_| {
            let mut out = vec![];
            for (line, pattern) in lines {
                if pattern {
                    if let Ok(list) = engine::call("expand_batch", json!({ "pattern": line })) {
                        out.extend(odm_ui::strings(&json!({ "l": list }), "l"));
                    }
                } else {
                    out.push(line);
                }
            }
            Msg::Expanded(generation, out)
        });
    }

    fn submit(&mut self, start: bool, on_exists: Option<&'static str>, context: &ComponentContext<Self>) {
        if !self.form.valid() || self.busy {
            return;
        }
        self.error.clear();
        if on_exists.is_none() {
            if let Some((dir, name)) = self.form.target(folder_for(&self.form.category)) {
                let exists = engine::call("target_exists", json!({ "dir": dir, "name": name })).ok().and_then(|v| v.as_bool()).unwrap_or(false);
                if exists {
                    self.exists = Some(start);
                    return;
                }
            }
        }
        self.busy = true;
        let requests = self.form.requests(start, on_exists);
        let assign = self.form.assignment();
        let show_progress = start && self.shared.store.borrow().setting("showProgressOnStart", false);
        context.spawn_background(move |_| {
            let added = (|| {
                if let Some((ext, category)) = assign {
                    engine::call("assign_extensions", json!({ "category": category, "extensions": [ext] }))?;
                }
                if let [one] = requests.as_slice() {
                    let id = engine::call("add_download", json!({ "req": one }))?;
                    Ok(id.as_str().filter(|_| show_progress).map(String::from))
                } else {
                    engine::call("add_many", json!({ "reqs": requests })).map(|_| None)
                }
            })();
            Msg::Added(added)
        });
    }

    fn field(&self, field: Field, value: &str, placeholder: &str, context: &ViewContext<Self>) -> TextBox {
        TextBox::new(value).placeholder_text(placeholder).on_text_changed(context.callback(move |t: Rc<str>| Msg::Text(field, t.to_string())))
    }

    fn switch(&self, flag: Flag, on: bool, context: &ViewContext<Self>) -> ToggleSwitch {
        ToggleSwitch::new().is_on(on).on_content("").off_content("").on_toggled(context.callback(move |v| Msg::Flag(flag, v)))
    }

    fn info(&self, context: &ViewContext<Self>) -> Option<View> {
        let (title, detail, spinning) = match self.form.info() {
            Info::Hidden => return None,
            Info::Checking => ("Checking link…".to_string(), String::new(), true),
            Info::Web { host } => (host, "Opened from a web page. The link has not been contacted yet.".to_string(), false),
            Info::Failed { error } => (error, "You can still add it; it will retry when started.".to_string(), false),
            Info::Probed { summary, detail, .. } => (summary, detail, false),
        };
        let mut left: Vec<View> = vec![TextBlock::new().text(title).text_wrapping(TextWrapping::Wrap).into()];
        if !detail.is_empty() {
            left.push(note(detail).into());
        }
        let mut cells: Vec<View> = vec![];
        if spinning {
            cells.push(ProgressRing::new().is_active(true).width(18.0).height(18.0).into());
        }
        cells.push(StackPanel::new().grid_column(1).vertical_alignment(VerticalAlignment::Center).children(left).into());
        if self.form.from_web && !spinning {
            let label = if self.form.probe.is_none() && self.form.probe_error.is_empty() { "Check Link" } else { "Check Again" };
            cells.push(Button::new().content(label).grid_column(2).on_click(context.callback(|()| Msg::Check)).into());
        }
        Some(card(Grid::new().column_spacing(12.0).columns([GridLength::Auto, GridLength::STAR, GridLength::Auto]).children(cells)))
    }
}

impl Component for AddWindow {
    type Input = Input;
    type Message = Msg;

    fn create(input: &Self::Input, context: &ComponentContext<Self>) -> Self {
        let general = folder_for("general").unwrap_or_default();
        let connections = n(&input.shared.store.borrow().settings, "defaultConnections");
        let mut form = AddForm::new(&input.params, connections, &general);
        if form.url.is_empty() && !form.batch {
            // offer a link that is on the clipboard
            if let Some(clip) = system::clipboard_text().map(|t| t.trim().to_string()) {
                if odm_ui::fmt::is_http(&clip) && !clip.chars().any(char::is_whitespace) {
                    form.url = clip;
                }
            }
        }
        let recents = engine::call("recent_folders", json!({ "limit": 6 })).map(|v| odm_ui::strings(&json!({ "r": v }), "r")).unwrap_or_default();
        let mut window = AddWindow {
            shared: input.shared.clone(),
            key: input.key,
            form,
            recents,
            advanced: false,
            generation: 0,
            busy: false,
            error: String::new(),
            exists: None,
            timer: None,
        };
        window.schedule(context);
        window
    }

    fn input_changed(&mut self, input: &Self::Input, _: &ComponentContext<Self>) {
        self.shared = input.shared.clone();
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        match message {
            Msg::Text(field, text) => {
                let f = &mut self.form;
                match field {
                    Field::Url => f.url = text,
                    Field::BatchText => f.batch_text = text,
                    Field::Name => f.name = text,
                    Field::Dir => f.dir = text,
                    Field::Checksum => f.checksum = text,
                    Field::Username => f.username = text,
                    Field::Password => f.password = text,
                    Field::Referrer => f.referrer = text,
                    Field::Cookies => f.cookies = text,
                    Field::UserAgent => f.user_agent = text,
                    Field::HeaderName(i) => {
                        if let Some(h) = f.headers.get_mut(i) {
                            h.0 = text;
                        }
                    }
                    Field::HeaderValue(i) => {
                        if let Some(h) = f.headers.get_mut(i) {
                            h.1 = text;
                        }
                    }
                }
                if matches!(field, Field::Url | Field::BatchText) {
                    self.schedule(context);
                }
            }
            Msg::Flag(flag, on) => match flag {
                Flag::Batch => {
                    self.form.batch = on;
                    self.schedule(context);
                }
                Flag::Always => self.form.always = on,
                Flag::Remember => self.form.remember = on,
                Flag::Verify => self.form.verify = on,
                Flag::Advanced => self.advanced = on,
            },
            Msg::Category(id) => {
                let folder = folder_for(&id);
                self.form.choose_category(&id, folder);
            }
            Msg::Queue(id) => self.form.queue = id,
            Msg::Connections(c) => self.form.connections = c.parse().unwrap_or(16),
            Msg::AddHeader => self.form.headers.push(Default::default()),
            Msg::RemoveHeader(i) => {
                if i < self.form.headers.len() {
                    self.form.headers.remove(i);
                }
            }
            Msg::Settled(generation) if generation == self.generation => {
                if self.form.batch {
                    self.expand(context);
                } else if self.form.wants_probe() {
                    self.probe(context);
                }
            }
            Msg::Settled(_) => {}
            Msg::Probed(generation, result) if generation == self.generation => match result {
                Ok(p) => {
                    let connections = self.default_connections();
                    self.form.probed(p, connections, folder_for);
                }
                Err(e) => self.form.probe_failed(e),
            },
            Msg::Probed(..) => {}
            Msg::Expanded(generation, links) if generation == self.generation => self.form.set_batch_urls(links),
            Msg::Expanded(..) => {}
            Msg::Check => {
                // probe right away (a web link is only contacted on request)
                if !self.form.batch && odm_ui::fmt::is_http(self.form.link()) {
                    self.generation = self.generation.wrapping_add(1);
                    self.form.probe = None;
                    self.form.probe_error.clear();
                    self.probe(context);
                }
            }
            Msg::Browse => {
                let start = self.form.dir.clone();
                context.spawn_background(move |_| Msg::Picked(system::pick_folder(&start)));
            }
            Msg::Picked(Some(dir)) => self.form.dir = dir,
            Msg::Picked(None) => {}
            Msg::Submit(start) => self.submit(start, None, context),
            Msg::Exists(result) => {
                if let Some(start) = self.exists.take() {
                    match result {
                        ContentDialogResult::Primary => self.submit(start, Some("rename"), context),
                        ContentDialogResult::Secondary => self.submit(start, Some("resume"), context),
                        _ => {}
                    }
                }
            }
            Msg::Overwrite => {
                if let Some(start) = self.exists.take() {
                    self.submit(start, Some("overwrite"), context);
                }
            }
            Msg::Added(Ok(progress)) => {
                if let Some(id) = progress {
                    self.shared.act(Action::Progress(id));
                }
                let _ = context.close_window();
            }
            Msg::Added(Err(e)) => {
                self.error = e;
                self.busy = false;
            }
            Msg::Cancel => {
                let _ = context.close_window();
            }
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(self.shared.visuals(580.0, 760.0));
        let f = &self.form;
        let store = self.shared.store.clone();
        let store = store.borrow();
        let category_name = store.category_name(&f.category);
        let mut rows: Vec<View> = vec![];

        if f.batch {
            rows.push(wide(
                "Links",
                StackPanel::new().spacing(6.0).children((
                    note("One per line. Patterns like file[001-120].jpg or [a-z] expand."),
                    TextBox::new(&f.batch_text)
                        .accepts_return(true)
                        .text_wrapping(TextWrapping::Wrap)
                        .min_height(110.0)
                        .on_text_changed(context.callback(|t: Rc<str>| Msg::Text(Field::BatchText, t.to_string()))),
                    note(format!("{} files", f.batch_urls.len())),
                )),
            ));
        } else {
            let mut url: Vec<View> = vec![self.field(Field::Url, &f.url, "https://", context).into()];
            if f.url_error() {
                url.push(error_text("Enter a full http:// or https:// address.").into());
            }
            rows.push(wide("URL", StackPanel::new().spacing(6.0).children(url)));
            rows.extend(self.info(context));
        }

        rows.push(heading("Save").into());
        if !f.batch {
            rows.push(wide("File name", self.field(Field::Name, &f.name, "", context)));
            let categories: Vec<(String, String)> = store.categories().iter().map(|c| (s(c, "id").to_string(), s(c, "name").to_string())).collect();
            let pairs: Vec<(&str, &str)> = categories.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            rows.push(row("Category", None, choice(&pairs, &f.category, context.callback(Msg::Category))));
        }
        let mut queues: Vec<(String, String)> = vec![(String::new(), "None (start manually)".to_string())];
        queues.extend(store.queues.iter().map(|q| (s(q, "id").to_string(), s(q, "name").to_string())));
        let pairs: Vec<(&str, &str)> = queues.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        rows.push(row("Queue", None, choice(&pairs, &f.queue, context.callback(Msg::Queue))));
        let mut folder: Vec<View> = vec![Grid::new()
            .column_spacing(8.0)
            .columns([GridLength::STAR, GridLength::Auto])
            .children((
                self.field(Field::Dir, &f.dir, "", context),
                Button::new().content("Choose…").grid_column(1).on_click(context.callback(|()| Msg::Browse)),
            ))
            .into()];
        if !self.recents.is_empty() {
            let recents = self.recents.clone();
            folder.push(
                ComboBox::new()
                    .placeholder_text("Recent folders")
                    .items_source(self.recents.iter().cloned())
                    .selected_index(self.recents.iter().position(|r| *r == f.dir))
                    .on_selection_changed(context.callback(move |i: Option<usize>| {
                        Msg::Text(Field::Dir, i.and_then(|i| recents.get(i).cloned()).unwrap_or_default())
                    }))
                    .horizontal_alignment(HorizontalAlignment::Stretch)
                    .into(),
            );
        }
        rows.push(wide("Save to", StackPanel::new().spacing(6.0).children(folder)));

        rows.push(heading("Options").into());
        let choices: Vec<(String, String)> = CONNECTIONS.iter().map(|c| (c.to_string(), c.to_string())).collect();
        let pairs: Vec<(&str, &str)> = choices.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        rows.push(row(
            "Connections",
            (!f.can_split()).then_some("This server doesn't support byte ranges."),
            choice(&pairs, &f.connections.to_string(), context.callback(Msg::Connections)).is_enabled(f.can_split()),
        ));
        if !f.batch {
            if f.shows_always() {
                rows.push(row(format!("Always put .{} files in {category_name}", f.name_ext()), None, self.switch(Flag::Always, f.always, context)));
            }
            rows.push(row(format!("Remember this folder for {category_name}"), None, self.switch(Flag::Remember, f.remember, context)));
            rows.push(row("Verify checksum after download", None, self.switch(Flag::Verify, f.verify, context)));
            if f.verify {
                rows.push(wide("Checksum", self.field(Field::Checksum, &f.checksum, "SHA-256, SHA-1 or MD5", context)));
            }
        }

        let mut advanced: Vec<View> = vec![
            self.field(Field::Username, &f.username, "", context).header("Username").into(),
            PasswordBox::new()
                .header("Password")
                .password(&f.password)
                .on_password_changed(context.callback(|t: Rc<str>| Msg::Text(Field::Password, t.to_string())))
                .into(),
            self.field(Field::Referrer, &f.referrer, "", context).header("Referrer").into(),
            self.field(Field::Cookies, &f.cookies, "name=value; other=value", context).header("Cookies").into(),
            self.field(Field::UserAgent, &f.user_agent, "Default", context).header("User agent").into(),
            text("Headers").into(),
        ];
        for (i, (name, value)) in f.headers.iter().enumerate() {
            advanced.push(
                Grid::new()
                    .column_spacing(6.0)
                    .columns([GridLength::STAR, GridLength::Star(2.0), GridLength::Auto])
                    .children((
                        self.field(Field::HeaderName(i), name, "Header", context),
                        self.field(Field::HeaderValue(i), value, "Value", context).grid_column(1),
                        Button::new().content("Remove").grid_column(2).on_click(context.callback(move |()| Msg::RemoveHeader(i))),
                    ))
                    .into(),
            );
        }
        advanced.push(Button::new().content("Add Header").on_click(context.callback(|()| Msg::AddHeader)).into());
        rows.push(
            Expander::new()
                .header("Authentication, referrer, cookies, headers")
                .is_expanded(self.advanced)
                .on_is_expanded_changed(context.callback(|v| Msg::Flag(Flag::Advanced, v)))
                .horizontal_alignment(HorizontalAlignment::Stretch)
                .horizontal_content_alignment(HorizontalAlignment::Stretch)
                .margin(Thickness::new(0.0, 12.0, 0.0, 8.0))
                .content(StackPanel::new().spacing(8.0).children(advanced))
                .into(),
        );
        if !self.error.is_empty() {
            rows.push(error_text(&self.error).into());
        }

        let ready = f.valid() && !self.busy;
        let start_label = if f.batch { format!("Start {} Downloads", f.batch_urls.len()) } else { "Start Download".to_string() };
        let bar = Border::new()
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
            .padding(Thickness::xy(20.0, 12.0))
            .grid_row(1)
            .content(Grid::new().column_spacing(8.0).columns([GridLength::Auto, GridLength::STAR, GridLength::Auto, GridLength::Auto, GridLength::Auto]).children((
                ToggleSwitch::new()
                    .is_on(f.batch)
                    .on_content("Batch")
                    .off_content("Batch")
                    .on_toggled(context.callback(|v| Msg::Flag(Flag::Batch, v)))
                    .tooltip("Add several links or a numbered pattern"),
                Button::new().content("Add to Queue").is_enabled(ready).grid_column(2).on_click(context.callback(|()| Msg::Submit(false))),
                Button::new().content("Cancel").grid_column(3).on_click(context.callback(|()| Msg::Cancel)),
                Button::new().style(ButtonStyle::Accent).content(start_label).is_enabled(ready).grid_column(4).on_click(context.callback(|()| Msg::Submit(true))),
            )));

        let mut root = Grid::new().rows([GridLength::STAR, GridLength::Auto]).children((
            ScrollViewer::new().content(StackPanel::new().margin(Thickness::new(20.0, 16.0, 20.0, 8.0)).children(rows)),
            bar,
        ));
        if !f.from_web {
            // Enter starts the download, except for a link a web page chose
            root = root.key_accelerators(KeyAccelerators::new([KeyAccelerator::new(
                AcceleratorKey::Enter,
                AcceleratorModifiers::None,
                context.callback(|()| Msg::Submit(true)),
            )]));
        }
        let exists = ContentDialog::new()
            .is_open(self.exists.is_some())
            .title(format!("“{}” already exists in this folder.", f.name.trim()))
            .content(StackPanel::new().spacing(10.0).children((
                note("Rename the new file, resume the existing one, or replace it."),
                Button::new().content("Overwrite the Existing File").on_click(context.callback(|()| Msg::Overwrite)),
            )))
            .primary_button_text("Rename New File")
            .secondary_button_text("Resume Existing File")
            .close_button_text("Cancel")
            .on_closed(context.callback(Msg::Exists));
        context.window_frame("Add Download", root.content_dialog(exists))
    }
}
