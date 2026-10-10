//! Per-download progress window: big percentage, segment map, one line per
//! connection, the speed graph, a limit for this download and what to do
//! when it finishes.
use crate::app::{act, Action, Shared};
use crate::system;
use crate::widgets::*;
use odm_ui::{b, fmt, n, opt_n, s};
use serde_json::{json, Value};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    pub id: Rc<str>,
}

pub enum Msg {
    LimitOn(bool),
    LimitText(String),
    ApplyLimit,
    WhenFinished(String),
    Toggle,
    Cancel,
    CancelAnswer(ContentDialogResult),
    Reveal,
    Open,
    Hide,
    Hover(bool),
    /// the auto-close countdown (seconds left)
    Closing(u32),
}

pub struct ProgressWindow {
    shared: Shared,
    id: Rc<str>,
    limit_on: bool,
    limit_text: String,
    limit_error: bool,
    confirm_cancel: bool,
    /// it was unfinished while this window was open
    saw_incomplete: bool,
    /// seconds until the window closes by itself
    closing: Option<u32>,
    hovering: bool,
    /// the countdown's next second (dropping a timer cancels it)
    timer: Option<ComponentTimer>,
}

impl Drop for ProgressWindow {
    fn drop(&mut self) {
        self.shared.act(Action::ProgressDone(self.id.to_string()));
    }
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
    fn record(&self) -> Option<Value> {
        self.shared.store.borrow().get(&self.id).cloned()
    }

    fn apply_limit(&mut self) {
        self.limit_error = false;
        if !self.limit_on {
            return act("set_download_limit", json!({ "id": &*self.id, "bps": null }));
        }
        // reject NaN / inf / 0 instead of silently turning the limit off
        match fmt::limit_bps(&self.limit_text) {
            Some(bps) => act("set_download_limit", json!({ "id": &*self.id, "bps": bps })),
            None => self.limit_error = true,
        }
    }

    /// Finished while open: close by itself in a moment.
    fn follow_status(&mut self, context: &ComponentContext<Self>) {
        let complete = self.record().map(|d| s(&d, "status") == "complete").unwrap_or(false);
        if !complete {
            self.saw_incomplete = true;
            self.closing = None;
        } else if std::mem::replace(&mut self.saw_incomplete, false) {
            self.closing = Some(5);
            self.timer = Some(context.set_timeout(Duration::from_secs(1), Msg::Closing(4)));
        }
    }
}

impl Component for ProgressWindow {
    type Input = Input;
    type Message = Msg;

    fn create(input: &Self::Input, _: &ComponentContext<Self>) -> Self {
        let record = input.shared.store.borrow().get(&input.id).cloned().unwrap_or(Value::Null);
        let limit = opt_n(&record, "speedLimitBps").filter(|l| *l > 0);
        ProgressWindow {
            shared: input.shared.clone(),
            id: input.id.clone(),
            limit_on: limit.is_some(),
            limit_text: limit.map(fmt::mbps).unwrap_or_else(|| "5".into()),
            limit_error: false,
            confirm_cancel: false,
            saw_incomplete: s(&record, "status") != "complete",
            closing: None,
            hovering: false,
            timer: None,
        }
    }

    fn input_changed(&mut self, input: &Self::Input, context: &ComponentContext<Self>) {
        self.shared = input.shared.clone();
        self.follow_status(context);
    }

    fn update(&mut self, message: Msg, context: &ComponentContext<Self>) {
        let id = self.id.to_string();
        match message {
            Msg::LimitOn(on) => {
                self.limit_on = on;
                self.apply_limit();
            }
            Msg::LimitText(text) => self.limit_text = text,
            Msg::ApplyLimit => self.apply_limit(),
            Msg::WhenFinished(v) => act("update_download", json!({ "id": id, "patch": { "whenFinished": v } })),
            Msg::Toggle => {
                let status = self.record().map(|d| s(&d, "status").to_string()).unwrap_or_default();
                if fmt::is_active(&status) || status == "queued" {
                    act("pause", json!({ "ids": [id] }));
                } else {
                    act("resume", json!({ "ids": [id] }));
                }
            }
            Msg::Cancel => self.confirm_cancel = true,
            Msg::CancelAnswer(result) => {
                self.confirm_cancel = false;
                if result == ContentDialogResult::Primary {
                    act("delete", json!({ "ids": [id], "deleteFiles": false }));
                    let _ = context.close_window();
                }
            }
            Msg::Reveal => {
                if let Some(Err(e)) = self.record().as_ref().map(system::reveal_download) {
                    self.shared.report(e);
                }
            }
            Msg::Open => {
                if let Some(d) = self.record() {
                    if let Err(e) = system::open_path(&Path::new(s(&d, "saveDir")).join(s(&d, "fileName"))) {
                        self.shared.report(e);
                    }
                }
                let _ = context.close_window();
            }
            Msg::Hide => {
                let _ = context.close_window();
            }
            Msg::Hover(on) => {
                self.hovering = on;
                if on {
                    // looking at it: don't close it under the pointer
                    self.closing = None;
                }
            }
            Msg::Closing(left) => {
                if self.closing.is_none() {
                    return;
                }
                if left == 0 {
                    if !self.hovering {
                        let _ = context.close_window();
                    }
                    self.closing = None;
                } else {
                    self.closing = Some(left);
                    self.timer = Some(context.set_timeout(Duration::from_secs(1), Msg::Closing(left - 1)));
                }
            }
        }
    }

    fn view(&self, _: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(self.shared.visuals(780.0, 700.0));
        let store = self.shared.store.clone();
        let store = store.borrow();
        let Some(d) = store.get(&self.id) else {
            return context.window_frame("Download", dim("This download no longer exists.").horizontal_alignment(HorizontalAlignment::Center));
        };
        let row = store.row(d);
        let status = row.status.as_str();
        let complete = status == "complete";

        let header = Grid::new().column_spacing(12.0).columns([GridLength::Auto, GridLength::STAR, GridLength::Auto]).children((
            FontIcon::new().glyph(file_glyph(&row.category)).vertical_alignment(VerticalAlignment::Center),
            StackPanel::new().grid_column(1).children((
                text(&row.name).font_size(18.0).font_weight(FontWeight::SEMI_BOLD).is_text_selection_enabled(true),
                dim(format!("{} · {}", fmt::host(d.get("finalUrl").and_then(Value::as_str).unwrap_or(s(d, "url"))), s(d, "saveDir"))).font_size(12.0),
            )),
            StackPanel::new().grid_column(2).vertical_alignment(VerticalAlignment::Center).children((status_label(status, &row.status_label),)),
        ));

        let stats = [
            ("DOWNLOADED", format!("{} / {}", fmt::bytes(Some(n(d, "receivedBytes"))), fmt::bytes(opt_n(d, "totalBytes")))),
            ("SPEED", row.speed.clone()),
            ("TIME LEFT", row.eta.clone()),
            ("RESUME", if b(d, "resumable") { "Supported".into() } else { "Not supported".to_string() }),
        ];
        let mut stat_cells: Vec<View> = vec![];
        for (i, (title, value)) in stats.iter().enumerate() {
            stat_cells.push(note(title).grid_column(i as i32).into());
            stat_cells.push(text(value).grid_column(i as i32).grid_row(1).into());
        }
        let hero = StackPanel::new().orientation(Orientation::Horizontal).spacing(28.0).children((
            TextBlock::new()
                .text(if complete { "✓ Complete".to_string() } else { format!("{}%", row.percent) })
                .font_size(40.0)
                .font_weight(FontWeight::SEMI_BOLD)
                .vertical_alignment(VerticalAlignment::Center),
            Grid::new()
                .column_spacing(24.0)
                .row_spacing(2.0)
                .vertical_alignment(VerticalAlignment::Center)
                .columns(std::iter::repeat_n(GridLength::Auto, 4))
                .rows([GridLength::Auto, GridLength::Auto])
                .children(stat_cells),
        ));

        let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        segs.sort_by_key(|x| n(x, "start"));
        let mut connections: Vec<View> = vec![];
        for (i, seg) in segs.iter().enumerate() {
            let state = s(seg, "state");
            let mut received = fmt::bytes(Some(n(seg, "received")));
            if n(seg, "end") < u64::MAX / 2 {
                received = format!("{received} / {}", fmt::bytes(Some(n(seg, "end").saturating_sub(n(seg, "start")))));
            }
            connections.push(
                Grid::new()
                    .column_spacing(10.0)
                    .height(28.0)
                    .columns([GridLength::Pixel(28.0), GridLength::STAR, GridLength::Pixel(96.0), GridLength::Pixel(86.0)])
                    .children((
                        dim(format!("{:02}", i + 1)),
                        text(received).grid_column(1),
                        text(state_label(state)).grid_column(2),
                        text(if state == "receiving" { fmt::speed(n(seg, "speedBps")) } else { fmt::DASH.into() })
                            .horizontal_alignment(HorizontalAlignment::Right)
                            .grid_column(3),
                    ))
                    .into(),
            );
        }

        let mut limit: Vec<View> = vec![ToggleSwitch::new()
            .header("Limit this download")
            .is_on(self.limit_on)
            .on_toggled(context.callback(Msg::LimitOn))
            .into()];
        if self.limit_on {
            limit.push(
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((
                        TextBox::new(&self.limit_text).width(90.0).on_text_changed(context.callback(|t: Rc<str>| Msg::LimitText(t.to_string()))),
                        dim("MB/s"),
                        Button::new().content("Apply").on_click(context.callback(|()| Msg::ApplyLimit)),
                    ))
                    .into(),
            );
            if self.limit_error {
                limit.push(error_text(fmt::LIMIT_ERROR).into());
            }
        }
        limit.push(note("Overrides the per-download setting.").into());
        let side = StackPanel::new().spacing(10.0).grid_column(1).children((
            sparkline(d, 80.0),
            StackPanel::new().spacing(6.0).children(limit),
            choice(
                &[("notify", "Show notification"), ("openFile", "Open file"), ("openFolder", "Show in folder"), ("none", "Do nothing")],
                s(d, "whenFinished"),
                context.callback(Msg::WhenFinished),
            )
            .header("When finished")
            .horizontal_alignment(HorizontalAlignment::Stretch),
        ));

        let mut body: Vec<View> = vec![header.into(), hero.into()];
        if let (true, Some(e)) = (status == "error", d.get("error").and_then(Value::as_str)) {
            body.push(error_text(e).into());
        }
        if d.get("checksumOk").and_then(Value::as_bool) == Some(true) {
            body.push(TextBlock::new().text("✓ Checksum verified").foreground(status_color("complete")).into());
        }
        body.push(
            StackPanel::new()
                .spacing(6.0)
                .children((
                    Grid::new().columns([GridLength::STAR, GridLength::Auto]).children((
                        note(format!("Segment map · {} connections", (n(d, "activeConnections") as usize).max(segs.len()))),
                        note("Faded = segment done · light edge = write position").grid_column(1),
                    )),
                    segment_map(d, 26.0, true),
                ))
                .into(),
        );
        body.push(
            Grid::new()
                .column_spacing(16.0)
                .columns([GridLength::STAR, GridLength::Pixel(300.0)])
                .children((
                    Border::new()
                        .border_brush(ThemeBrush::CardStroke)
                        .border_thickness(1.0)
                        .corner_radius(4.0)
                        .padding(Thickness::xy(10.0, 4.0))
                        .min_height(170.0)
                        .content(StackPanel::new().children(connections)),
                    side,
                ))
                .into(),
        );

        let mut buttons: Vec<View> = vec![];
        if let Some(left) = self.closing {
            buttons.push(dim(format!("Closing in {left}s")).into());
        }
        buttons.push(Button::new().content(if complete { "Close" } else { "Hide" }).on_click(context.callback(|()| Msg::Hide)).into());
        if complete {
            buttons.push(Button::new().content("Show in Folder").on_click(context.callback(|()| Msg::Reveal)).into());
            buttons.push(Button::new().style(ButtonStyle::Accent).content("Open").on_click(context.callback(|()| Msg::Open)).into());
        } else {
            let label = if fmt::is_active(status) || status == "queued" {
                "Pause"
            } else if status == "error" {
                "Retry"
            } else {
                "Resume"
            };
            buttons.push(Button::new().content("Cancel Download").on_click(context.callback(|()| Msg::Cancel)).into());
            buttons.push(Button::new().style(ButtonStyle::Accent).content(label).on_click(context.callback(|()| Msg::Toggle)).into());
        }

        let root = Border::new()
            .on_pointer_entered(context.callback(|_| Msg::Hover(true)))
            .on_pointer_exited(context.callback(|_| Msg::Hover(false)))
            .content(Grid::new().rows([GridLength::STAR, GridLength::Auto]).children((
                ScrollViewer::new().content(StackPanel::new().spacing(14.0).margin(Thickness::xy(20.0, 16.0)).children(body)),
                Border::new().grid_row(1).content(footer(buttons)),
            )));
        let confirm = ContentDialog::new()
            .is_open(self.confirm_cancel)
            .title("Cancel this download?")
            .content(note("The partial file is deleted and the download is removed from the list."))
            .primary_button_text("Cancel Download")
            .close_button_text("Keep Downloading")
            .on_closed(context.callback(Msg::CancelAnswer));
        context.window_frame(format!("{}% {}", row.percent, row.name), root.content_dialog(confirm))
    }
}
