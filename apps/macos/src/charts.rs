//! The two drawings the windows share: the segment map (one cell per
//! connection, sized by its share of the file) and the 60-second speed
//! graph.
use crate::kit::{self, Canvas};
use objc2::rc::Retained;
use objc2_app_kit::{NSBezierPath, NSColor};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use odm_ui::{n, opt_n, s};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;

/// The download a drawing shows; replaced by its owner, which then asks
/// for a redraw.
pub type Shown = Rc<RefCell<Value>>;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// A segment's length: to its end when known, else to the end of the file.
fn length(seg: &Value, total: f64) -> f64 {
    let start = n(seg, "start");
    match opt_n(seg, "end").filter(|e| *e < u64::MAX / 2) {
        Some(end) => end.saturating_sub(start) as f64,
        None => (total - start as f64).max(1.0),
    }
}

pub fn segment_map(shown: &Shown, write_head: bool) -> Retained<Canvas> {
    let shown = shown.clone();
    Canvas::new(move |bounds| {
        let d = shown.borrow();
        let status = s(&d, "status");
        let complete = status == "complete";
        let color = kit::status_color(status);
        let track = unsafe { NSColor::secondaryLabelColor().colorWithAlphaComponent(0.18) };
        let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        segs.sort_by_key(|g| n(g, "start"));
        let (w, h) = (bounds.size.width, bounds.size.height);
        if segs.is_empty() {
            kit::fill_round(rect(0.0, 0.0, w, h), 4.0, &track);
            if complete {
                kit::fill_round(rect(0.0, 0.0, w, h), 4.0, &*unsafe { color.colorWithAlphaComponent(0.55) });
            }
            return;
        }
        let total = opt_n(&d, "totalBytes").unwrap_or_else(|| segs.iter().map(|g| n(g, "start") + n(g, "received")).max().unwrap_or(1)) as f64;
        let shares: Vec<f64> = segs.iter().map(|g| (length(g, total) / total.max(1.0)).max(0.012)).collect();
        let sum: f64 = shares.iter().sum();
        let gap = 3.0;
        let usable = w - gap * (segs.len() as f64 - 1.0);
        let mut x = 0.0;
        for (g, share) in segs.iter().zip(&shares) {
            let cell = (usable * share / sum).max(3.0);
            kit::fill_round(rect(x, 0.0, cell, h), 3.0, &track);
            let part = if complete { 1.0 } else { (n(g, "received") as f64 / length(g, total).max(1.0)).min(1.0) };
            if part > 0.0 {
                let done = s(g, "state") == "done" || part >= 1.0;
                let filled = cell * part;
                kit::fill_round(rect(x, 0.0, filled, h), 3.0, &*unsafe { color.colorWithAlphaComponent(if done { 0.55 } else { 1.0 }) });
                if write_head && s(g, "state") == "receiving" && part < 1.0 {
                    let head = filled.min(8.0);
                    kit::fill_round(rect(x + filled - head, 0.0, head, h), 0.0, &*unsafe { NSColor::whiteColor().colorWithAlphaComponent(0.35) });
                }
            }
            x += cell + gap;
        }
    })
}

/// The last 60 speed samples of a download.
pub fn samples(d: &Value) -> Vec<u64> {
    let all: Vec<u64> = d.get("speedHistory").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    all[all.len().saturating_sub(60)..].to_vec()
}

pub fn sparkline(shown: &Shown) -> Retained<Canvas> {
    let shown = shown.clone();
    Canvas::new(move |bounds| {
        let (w, h) = (bounds.size.width, bounds.size.height);
        kit::fill_round(rect(0.0, 0.0, w, h), 6.0, &*unsafe { NSColor::secondaryLabelColor().colorWithAlphaComponent(0.08) });
        let points = samples(&shown.borrow());
        if points.len() < 2 {
            return;
        }
        let peak = points.iter().copied().max().unwrap_or(1).max(1) as f64;
        let offset = 60 - points.len();
        let at = |i: usize| NSPoint::new(w * (offset + i) as f64 / 59.0, h - 3.0 - (points[i] as f64 / peak) * (h - 8.0));
        unsafe {
            let accent = NSColor::controlAccentColor();
            let area = NSBezierPath::bezierPath();
            area.moveToPoint(at(0));
            for i in 1..points.len() {
                area.lineToPoint(at(i));
            }
            area.lineToPoint(NSPoint::new(at(points.len() - 1).x, h));
            area.lineToPoint(NSPoint::new(at(0).x, h));
            area.closePath();
            accent.colorWithAlphaComponent(0.18).setFill();
            area.fill();
            let line = NSBezierPath::bezierPath();
            line.moveToPoint(at(0));
            for i in 1..points.len() {
                line.lineToPoint(at(i));
            }
            line.setLineWidth(1.6);
            accent.setStroke();
            line.stroke();
        }
    })
}
