//! Small custom widgets and builders shared by the windows.
use crate::row::{n, opt_n, s};
use adw::prelude::*;
use gtk::{cairo, gdk, glib};
use odm_linux::fmt;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

/// GNOME palette colors for a download status.
pub fn status_rgb(status: &str) -> (f64, f64, f64) {
    match status {
        "downloading" | "connecting" | "assembling" => (0.21, 0.52, 0.89),
        "paused" => (0.90, 0.65, 0.04),
        "error" => (0.88, 0.11, 0.14),
        "complete" => (0.18, 0.76, 0.49),
        _ => (0.55, 0.55, 0.55),
    }
}

/// CSS class that colors a status label.
pub fn status_class(status: &str) -> &'static str {
    match status {
        "downloading" | "connecting" | "assembling" => "accent",
        "paused" => "warning",
        "error" => "error",
        "complete" => "success",
        _ => "dim-label",
    }
}

pub const STATUS_CLASSES: [&str; 5] = ["accent", "warning", "error", "success", "dim-label"];

/// Give `label` the color of `status` (the text itself always says it too).
pub fn paint_status(label: &gtk::Label, status: &str) {
    for c in STATUS_CLASSES {
        label.remove_css_class(c);
    }
    label.add_css_class(status_class(status));
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

/// One cell per connection segment, sized by its share of the file.
#[derive(Clone)]
pub struct SegmentMap {
    pub area: gtk::DrawingArea,
    data: Rc<RefCell<Value>>,
}

impl SegmentMap {
    pub fn new(height: i32, write_head: bool) -> Self {
        let area = gtk::DrawingArea::builder().content_height(height).hexpand(true).build();
        area.set_accessible_role(gtk::AccessibleRole::Img);
        let data = Rc::new(RefCell::new(Value::Null));
        let d = data.clone();
        area.set_draw_func(move |area, cr, w, h| {
            let d = d.borrow();
            let fg: gdk::RGBA = area.color();
            let (w, h) = (w as f64, h as f64);
            let status = s(&d, "status");
            let (r, g, b) = status_rgb(status);
            let track = |cr: &cairo::Context| cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.15);
            let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
            segs.sort_by_key(|x| n(x, "start"));
            if segs.is_empty() {
                rounded(cr, 0.0, 0.0, w, h, 4.0);
                if status == "complete" {
                    cr.set_source_rgba(r, g, b, 0.55);
                } else {
                    track(cr);
                }
                let _ = cr.fill();
                return;
            }
            let known = |x: &Value| n(x, "end") < u64::MAX / 2;
            let total = opt_n(&d, "totalBytes")
                .unwrap_or_else(|| segs.iter().map(|x| n(x, "start") + n(x, "received")).max().unwrap_or(1))
                .max(1) as f64;
            let len = |x: &Value| {
                if known(x) {
                    n(x, "end").saturating_sub(n(x, "start")) as f64
                } else {
                    (total - n(x, "start") as f64).max(1.0)
                }
            };
            let sizes: Vec<f64> = segs.iter().map(|x| (len(x) / total).max(0.012)).collect();
            let sum: f64 = sizes.iter().sum();
            let gap = 3.0;
            let usable = w - gap * (segs.len() as f64 - 1.0);
            let mut x = 0.0;
            for (seg, size) in segs.iter().zip(&sizes) {
                let sw = (usable * size / sum).max(3.0);
                rounded(cr, x, 0.0, sw, h, 3.0);
                track(cr);
                let _ = cr.fill();
                let frac = if status == "complete" { 1.0 } else { (n(seg, "received") as f64 / len(seg).max(1.0)).min(1.0) };
                if frac > 0.0 {
                    let done = s(seg, "state") == "done" || frac >= 1.0;
                    rounded(cr, x, 0.0, sw * frac, h, 3.0);
                    cr.set_source_rgba(r, g, b, if done { 0.55 } else { 1.0 });
                    let _ = cr.fill();
                    if write_head && s(seg, "state") == "receiving" && frac < 1.0 {
                        let hw = (sw * frac).min(8.0);
                        cr.rectangle(x + sw * frac - hw, 0.0, hw, h);
                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
                        let _ = cr.fill();
                    }
                }
                x += sw + gap;
            }
        });
        SegmentMap { area, data }
    }

    /// Show this download's segments.
    pub fn set(&self, download: &Value) {
        let count = download.get("segments").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        self.area.update_property(&[gtk::accessible::Property::Label(&format!("Segment map, {count} segments"))]);
        *self.data.borrow_mut() = download.clone();
        self.area.queue_draw();
    }
}

/// 60-second speed history with its peak.
#[derive(Clone)]
pub struct Sparkline {
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    peak: gtk::Label,
    samples: Rc<RefCell<Vec<u64>>>,
}

impl Sparkline {
    pub fn new(height: i32) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::builder().label("Speed, last 60 s").xalign(0.0).hexpand(true).build();
        title.add_css_class("caption");
        title.add_css_class("dim-label");
        let peak = gtk::Label::new(None);
        peak.add_css_class("caption");
        peak.add_css_class("dim-label");
        peak.add_css_class("numeric");
        head.append(&title);
        head.append(&peak);
        let area = gtk::DrawingArea::builder().content_height(height).hexpand(true).build();
        area.add_css_class("card");
        let samples = Rc::new(RefCell::new(Vec::<u64>::new()));
        let data = samples.clone();
        area.set_draw_func(move |_, cr, w, h| {
            let all = data.borrow();
            let s = &all[all.len().saturating_sub(60)..];
            if s.len() < 2 {
                return;
            }
            let (w, h) = (w as f64, h as f64);
            let peak = s.iter().copied().max().unwrap_or(1).max(1) as f64;
            let off = 60 - s.len();
            let pt = |i: usize| (w * (off + i) as f64 / 59.0, h - 3.0 - (s[i] as f64 / peak) * (h - 8.0));
            let (r, g, b) = status_rgb("downloading");
            cr.move_to(pt(0).0, pt(0).1);
            for i in 1..s.len() {
                cr.line_to(pt(i).0, pt(i).1);
            }
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.6);
            let _ = cr.stroke_preserve();
            cr.line_to(pt(s.len() - 1).0, h);
            cr.line_to(pt(0).0, h);
            cr.close_path();
            cr.set_source_rgba(r, g, b, 0.18);
            let _ = cr.fill();
        });
        root.append(&head);
        root.append(&area);
        Sparkline { root, area, peak, samples }
    }

    pub fn set(&self, download: &Value) {
        let v: Vec<u64> = download
            .get("speedHistory")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        self.peak.set_label(&format!("peak {}", fmt::speed(v.iter().copied().max().unwrap_or(0))));
        *self.samples.borrow_mut() = v;
        self.area.queue_draw();
    }
}

/// Ask for a folder; None when the dialog is cancelled.
pub async fn choose_folder(parent: &impl IsA<gtk::Window>, start: &str) -> Option<String> {
    let dialog = gtk::FileDialog::builder().modal(true).title("Choose a Folder").build();
    if !start.is_empty() {
        dialog.set_initial_folder(Some(&gtk::gio::File::for_path(start)));
    }
    let folder = dialog.select_folder_future(Some(parent)).await.ok()?;
    folder.path().map(|p| p.to_string_lossy().into_owned())
}

/// A caption-sized, dimmed label.
pub fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("caption");
    l.add_css_class("dim-label");
    l
}

/// A label for numbers, paths and addresses (tabular digits, selectable).
pub fn value_label() -> gtk::Label {
    let l = gtk::Label::builder().xalign(0.0).selectable(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
    l.add_css_class("numeric");
    l
}

/// A drop-down row over (value, label) pairs.
#[derive(Clone)]
pub struct Choice {
    pub row: adw::ComboRow,
    values: Rc<RefCell<Vec<String>>>,
}

impl Choice {
    pub fn new(title: &str, items: &[(&str, &str)]) -> Self {
        let c = Choice { row: adw::ComboRow::builder().title(title).build(), values: Rc::default() };
        c.set_items(items.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect());
        c
    }

    pub fn set_items(&self, items: Vec<(String, String)>) {
        let keep = self.value();
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        *self.values.borrow_mut() = items.iter().map(|(v, _)| v.clone()).collect();
        self.row.set_model(Some(&gtk::StringList::new(&labels)));
        self.set_value(&keep);
    }

    pub fn value(&self) -> String {
        self.values.borrow().get(self.row.selected() as usize).cloned().unwrap_or_default()
    }

    pub fn set_value(&self, v: &str) {
        if let Some(i) = self.values.borrow().iter().position(|x| x == v) {
            if self.row.selected() != i as u32 {
                self.row.set_selected(i as u32);
            }
        }
    }

    pub fn on_change(&self, f: impl Fn(String) + 'static) {
        let me = self.clone();
        self.row.connect_selected_notify(move |_| f(me.value()));
    }
}

/// Ask a yes/no style question. Resolves to the chosen response id
/// ("cancel" when dismissed).
pub async fn ask(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    responses: &[(&str, &str, adw::ResponseAppearance)],
) -> glib::GString {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    for (id, label, look) in responses {
        dialog.add_response(id, label);
        dialog.set_response_appearance(id, *look);
    }
    dialog.set_close_response("cancel");
    dialog.choose_future(Some(parent)).await
}

/// Ask for one line of text; None when cancelled or left empty.
pub async fn ask_text(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str, initial: &str, ok: &str) -> Option<String> {
    let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("ok", ok);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    let choice = dialog.choose_future(Some(parent)).await;
    let text = entry.text().trim().to_string();
    (choice == "ok" && !text.is_empty()).then_some(text)
}
