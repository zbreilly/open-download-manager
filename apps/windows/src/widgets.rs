//! View pieces shared by the windows.
use odm_ui::{fmt, n, opt_n, s};
use serde_json::Value;
use windows_reactor::*;

/// The color of a download status (the text always says it too).
pub fn status_color(status: &str) -> Color {
    match status {
        "downloading" | "connecting" | "assembling" => Color::rgb(0x3B, 0x8E, 0xEA),
        "paused" => Color::rgb(0xD6, 0x8A, 0x00),
        "error" => Color::rgb(0xE0, 0x43, 0x43),
        "complete" => Color::rgb(0x2E, 0xA0, 0x5A),
        _ => Color::rgb(0x8A, 0x8A, 0x8A),
    }
}

const TRACK: Color = Color::argb(0x30, 0x80, 0x80, 0x80);
pub const CRITICAL: Color = Color::rgb(0xE0, 0x43, 0x43);

/// Segoe Fluent glyph for a download's category.
pub fn file_glyph(category: &str) -> &'static str {
    match category {
        "video" => "\u{E714}",
        "music" => "\u{E8D6}",
        "documents" => "\u{E8A5}",
        "compressed" => "\u{F012}",
        "programs" => "\u{ECAA}",
        _ => "\u{E7C3}",
    }
}

/// Segoe Fluent glyph for a sidebar category (the settings' icon names).
pub fn category_glyph(icon: &str) -> &'static str {
    match icon {
        "cat-video" => "\u{E714}",
        "cat-music" => "\u{E8D6}",
        "cat-documents" => "\u{E8A5}",
        "cat-compressed" => "\u{F012}",
        "cat-programs" => "\u{ECAA}",
        "cat-images" => "\u{EB9F}",
        "file-disc" => "\u{E958}",
        _ => "\u{E8B7}",
    }
}

pub fn text(t: impl AsRef<str>) -> TextBlock {
    TextBlock::new().text(t).text_trimming(TextTrimming::CharacterEllipsis).vertical_alignment(VerticalAlignment::Center)
}

/// Secondary text.
pub fn dim(t: impl AsRef<str>) -> TextBlock {
    text(t).opacity(0.65)
}

/// Small secondary text that wraps (notes under settings).
pub fn note(t: impl AsRef<str>) -> TextBlock {
    TextBlock::new().text(t).font_size(12.0).opacity(0.65).text_wrapping(TextWrapping::Wrap)
}

pub fn heading(t: impl AsRef<str>) -> TextBlock {
    TextBlock::new().text(t).font_weight(FontWeight::SEMI_BOLD).margin(Thickness::new(0.0, 16.0, 0.0, 6.0))
}

pub fn error_text(t: impl AsRef<str>) -> TextBlock {
    TextBlock::new().text(t).foreground(CRITICAL).text_wrapping(TextWrapping::Wrap).is_text_selection_enabled(true)
}

/// "● Downloading" in the status color.
pub fn status_label(status: &str, label: &str) -> View {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(6.0)
        .vertical_alignment(VerticalAlignment::Center)
        .children((
            Ellipse::new().width(7.0).height(7.0).fill(status_color(status)).vertical_alignment(VerticalAlignment::Center),
            text(label),
        ))
        .into()
}

/// A card with a label (and an optional note) on the left and a control
/// on the right: one line of a settings-like form.
pub fn row(label: impl AsRef<str>, hint: Option<&str>, control: impl Into<View>) -> View {
    let mut left = vec![View::from(TextBlock::new().text(label).text_wrapping(TextWrapping::Wrap))];
    if let Some(h) = hint {
        left.push(note(h).into());
    }
    card(
        Grid::new().column_spacing(16.0).columns([GridLength::STAR, GridLength::Auto]).children((
            StackPanel::new().vertical_alignment(VerticalAlignment::Center).children(left),
            StackPanel::new().vertical_alignment(VerticalAlignment::Center).grid_column(1).children((control.into(),)),
        )),
    )
}

/// A card holding one full-width control under a label.
pub fn wide(label: impl AsRef<str>, control: impl Into<View>) -> View {
    card(StackPanel::new().spacing(6.0).children((TextBlock::new().text(label), control.into())))
}

pub fn card(content: impl Into<View>) -> View {
    Border::new()
        .background(ThemeBrush::CardBackground)
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(1.0)
        .corner_radius(4.0)
        .padding(Thickness::xy(16.0, 10.0))
        .margin(Thickness::new(0.0, 0.0, 0.0, 4.0))
        .content(content)
        .into()
}

/// A bar of buttons at the bottom of a window.
pub fn footer(children: Vec<View>) -> View {
    Border::new()
        .border_brush(ThemeBrush::CardStroke)
        .border_thickness(Thickness::new(0.0, 1.0, 0.0, 0.0))
        .padding(Thickness::xy(20.0, 12.0))
        .content(StackPanel::new().orientation(Orientation::Horizontal).spacing(8.0).horizontal_alignment(HorizontalAlignment::Right).children(children))
        .into()
}

/// One cell per connection segment, sized by its share of the file.
pub fn segment_map(d: &Value, height: f64, write_head: bool) -> View {
    let status = s(d, "status");
    let color = status_color(status);
    let mut segs: Vec<&Value> = d.get("segments").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    segs.sort_by_key(|x| n(x, "start"));
    if segs.is_empty() {
        let fill = if status == "complete" { Color::argb(0x8C, color.r, color.g, color.b) } else { TRACK };
        return Rectangle::new().height(height).radius_x(4.0).radius_y(4.0).fill(fill).into();
    }
    let known = |x: &Value| n(x, "end") < u64::MAX / 2;
    let total = opt_n(d, "totalBytes").unwrap_or_else(|| segs.iter().map(|x| n(x, "start") + n(x, "received")).max().unwrap_or(1)).max(1) as f64;
    let len = |x: &Value| {
        if known(x) {
            n(x, "end").saturating_sub(n(x, "start")) as f64
        } else {
            (total - n(x, "start") as f64).max(1.0)
        }
    };
    let mut columns = vec![];
    let mut cells: Vec<View> = vec![];
    for (i, seg) in segs.iter().enumerate() {
        // star widths share the row by segment size, with a floor so a
        // tiny segment stays visible
        columns.push(GridLength::Star((len(seg) / total).max(0.012)));
        let frac = if status == "complete" { 1.0 } else { (n(seg, "received") as f64 / len(seg).max(1.0)).min(1.0) };
        let done = s(seg, "state") == "done" || frac >= 1.0;
        let fill = Color::argb(if done { 0x8C } else { 0xFF }, color.r, color.g, color.b);
        let mut parts: Vec<View> = vec![Rectangle::new().radius_x(3.0).radius_y(3.0).fill(TRACK).grid_column_span(3).into()];
        if frac > 0.0 {
            parts.push(Rectangle::new().radius_x(3.0).radius_y(3.0).fill(fill).grid_column_span(2).into());
            if write_head && s(seg, "state") == "receiving" && frac < 1.0 {
                parts.push(Rectangle::new().width(4.0).fill(Color::argb(0x59, 0xFF, 0xFF, 0xFF)).grid_column(1).into());
            }
        }
        cells.push(
            Grid::new()
                .columns([GridLength::Star(frac.max(0.0001)), GridLength::Auto, GridLength::Star((1.0 - frac).max(0.0001))])
                .children(parts)
                .grid_column(i as i32)
                .into(),
        );
    }
    Grid::new().height(height).column_spacing(3.0).columns(columns).children(cells).into()
}

/// 60-second speed history as bars, with its peak.
pub fn sparkline(d: &Value, height: f64) -> View {
    let all: Vec<u64> = d.get("speedHistory").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let samples = &all[all.len().saturating_sub(60)..];
    let peak = samples.iter().copied().max().unwrap_or(0);
    let color = status_color("downloading");
    let mut bars: Vec<View> = vec![];
    for (i, v) in samples.iter().enumerate() {
        let h = if peak == 0 { 0.0 } else { (*v as f64 / peak as f64 * (height - 6.0)).max(1.0) };
        bars.push(
            Rectangle::new()
                .height(h)
                .fill(color)
                .vertical_alignment(VerticalAlignment::Bottom)
                .grid_column((60 - samples.len() + i) as i32)
                .into(),
        );
    }
    StackPanel::new()
        .spacing(4.0)
        .children((
            Grid::new().columns([GridLength::STAR, GridLength::Auto]).children((
                note("Speed, last 60 s"),
                note(format!("peak {}", fmt::speed(peak))).grid_column(1),
            )),
            Border::new().background(TRACK).corner_radius(4.0).padding(Thickness::xy(2.0, 3.0)).content(
                Grid::new().height(height).column_spacing(1.0).columns(std::iter::repeat_n(GridLength::STAR, 60)).children(bars),
            ),
        ))
        .into()
}

/// A drop-down over (value, label) pairs.
pub fn choice(items: &[(&str, &str)], value: &str, on_change: Callback<String>) -> ComboBox {
    let values: Vec<String> = items.iter().map(|(v, _)| v.to_string()).collect();
    let selected = values.iter().position(|v| v == value);
    ComboBox::new()
        .items_source(items.iter().map(|(_, l)| l.to_string()))
        .selected_index(selected)
        .on_selection_changed(Callback::new(move |i: Option<usize>| {
            if let Some(v) = i.and_then(|i| values.get(i)) {
                on_change.call(v.clone());
            }
        }))
}
