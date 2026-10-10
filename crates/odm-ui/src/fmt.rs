//! Text formatting shared by the front ends (same rules as the macOS app
//! and the design spec: base-1024 sizes, integer percent, short ETA).
use chrono::{DateTime, Datelike, Local, Timelike};
use serde_json::Value;

const KB: f64 = 1024.0;
const MB: f64 = KB * 1024.0;
const GB: f64 = MB * 1024.0;

pub const DASH: &str = "—";

pub fn bytes(n: Option<u64>) -> String {
    let Some(n) = n else { return DASH.into() };
    let d = n as f64;
    if d < MB {
        format!("{:.1} KB", d / KB)
    } else if d < GB {
        format!("{:.1} MB", d / MB)
    } else {
        format!("{:.2} GB", d / GB)
    }
}

pub fn speed(bps: u64) -> String {
    if bps == 0 {
        DASH.into()
    } else {
        format!("{}/s", bytes(Some(bps)))
    }
}

/// 0..=99 while unfinished, 100 only when complete.
pub fn percent(received: u64, total: Option<u64>, complete: bool) -> u32 {
    if complete {
        return 100;
    }
    match total {
        Some(t) if t > 0 => ((received as f64 / t as f64) * 100.0).clamp(0.0, 99.0) as u32,
        _ => 0,
    }
}

pub fn eta(secs: Option<u64>) -> String {
    let Some(s) = secs.filter(|s| *s < u64::MAX / 2) else { return DASH.into() };
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else if s < 86400 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else {
        "> 1 day".into()
    }
}

fn local(iso: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(iso).ok().map(|d| d.with_timezone(&Local))
}

/// "Today 14:02", "Yesterday 22:41", "Oct 6", "Oct 6, 2025".
pub fn added(iso: &str) -> String {
    let Some(d) = local(iso) else { return DASH.into() };
    let now = Local::now();
    let days = (now.date_naive() - d.date_naive()).num_days();
    let hm = format!("{:02}:{:02}", d.hour(), d.minute());
    match days {
        0 => format!("Today {hm}"),
        1 => format!("Yesterday {hm}"),
        _ if d.year() == now.year() => d.format("%b %-d").to_string(),
        _ => d.format("%b %-d, %Y").to_string(),
    }
}

pub fn hhmm(iso: &str) -> String {
    local(iso).map(|d| format!("{:02}:{:02}", d.hour(), d.minute())).unwrap_or_default()
}

/// Label for a download's status ("Starts 01:00" for a scheduled one).
pub fn status_label(status: &str, scheduled_at: Option<&str>) -> String {
    match status {
        "downloading" => "Downloading".into(),
        "paused" => "Paused".into(),
        "queued" => match scheduled_at {
            Some(t) => format!("Starts {}", hhmm(t)),
            None => "Queued".into(),
        },
        "error" => "Failed".into(),
        "complete" => "Complete".into(),
        "connecting" => "Connecting".into(),
        "assembling" => "Finishing".into(),
        other => other.into(),
    }
}

pub fn is_active(status: &str) -> bool {
    matches!(status, "connecting" | "downloading" | "assembling")
}

/// Bytes per second as MB/s with at most one decimal ("25", "2.5").
pub fn mbps(bps: u64) -> String {
    let v = ((bps as f64 / MB) * 10.0).round() / 10.0;
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// Largest speed limit accepted from a text field, in MB/s.
pub const MAX_LIMIT_MB: f64 = 1_000_000.0;
pub const LIMIT_ERROR: &str = "Enter a speed between 0 and 1,000,000 MB/s.";

/// Parse a "MB/s" text field. None for anything that is not a finite number
/// above 0 and at most [`MAX_LIMIT_MB`], so callers can show an error
/// instead of silently turning a limit off.
pub fn limit_bps(text: &str) -> Option<u64> {
    let v: f64 = text.trim().replace(',', ".").parse().ok()?;
    if !v.is_finite() || v <= 0.0 || v > MAX_LIMIT_MB {
        return None;
    }
    Some(((v * MB).round() as u64).max(1))
}

pub fn host(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default()
}

/// A full http(s) address with a host.
pub fn is_http(s: &str) -> bool {
    url::Url::parse(s.trim()).map(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some()).unwrap_or(false)
}

/// Lowercase extension of a file name ("" when there is none or it is
/// longer than 10 characters).
pub fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() && !e.is_empty() && e.len() <= 10 && !e.contains('/') => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The last path segment of a link, percent-decoded.
pub fn url_file_name(link: &str) -> String {
    let Ok(u) = url::Url::parse(link.trim()) else { return String::new() };
    let last = u.path_segments().and_then(|mut p| p.next_back()).unwrap_or("");
    percent_encoding::percent_decode_str(last).decode_utf8_lossy().into_owned()
}

/// "2 active · 1 paused · 3 queued" for the status bar.
pub fn totals_summary(t: &Value) -> String {
    let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut parts = vec![format!("{} active", n("active"))];
    for (k, label) in [("paused", "paused"), ("queued", "queued"), ("scheduled", "scheduled"), ("failed", "failed")] {
        if n(k) > 0 {
            parts.push(format!("{} {label}", n(k)));
        }
    }
    parts.join(" · ")
}

/// When a queue's schedule next starts it: "today 01:00", "tomorrow 01:00",
/// "Friday 01:00". None when it has no start time.
pub fn next_run(queue: &Value) -> Option<String> {
    if !queue.get("startEnabled").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let (h, m) = queue.get("startTime")?.as_str()?.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    let days: Vec<bool> = queue
        .get("days")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_bool().unwrap_or(true)).collect())
        .unwrap_or_default();
    let now = Local::now();
    for add in 0..8 {
        let day = now.date_naive() + chrono::Duration::days(add);
        let Some(t) = day.and_hms_opt(h, m, 0).and_then(|t| t.and_local_timezone(Local).earliest()) else { continue };
        let wd = day.weekday().num_days_from_monday() as usize;
        if t > now && days.get(wd).copied().unwrap_or(true) {
            let hm = format!("{h:02}:{m:02}");
            return Some(match add {
                0 => format!("today {hm}"),
                1 => format!("tomorrow {hm}"),
                _ => format!("{} {hm}", day.format("%A")),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times() {
        assert_eq!(bytes(None), DASH);
        assert_eq!(bytes(Some(512)), "0.5 KB");
        assert_eq!(bytes(Some(5 * 1024 * 1024)), "5.0 MB");
        assert_eq!(bytes(Some(3 * 1024 * 1024 * 1024)), "3.00 GB");
        assert_eq!(speed(0), DASH);
        assert_eq!(eta(Some(47)), "47s");
        assert_eq!(eta(Some(292)), "4m 52s");
        assert_eq!(eta(Some(4080)), "1h 08m");
        assert_eq!(eta(Some(u64::MAX)), DASH);
        assert_eq!(percent(999, Some(1000), false), 99);
        assert_eq!(percent(0, None, true), 100);
    }

    #[test]
    fn limits() {
        assert_eq!(limit_bps("2,5"), Some(2_621_440));
        assert_eq!(limit_bps("0"), None);
        assert_eq!(limit_bps("inf"), None);
        assert_eq!(limit_bps("abc"), None);
        assert_eq!(mbps(26_214_400), "25");
        assert_eq!(mbps(2_621_440), "2.5");
    }

    #[test]
    fn names() {
        assert_eq!(extension("a.tar.GZ"), "gz");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("noext"), "");
        assert!(is_http("https://example.com/a"));
        assert!(!is_http("ftp://example.com/a"));
        assert_eq!(url_file_name("https://x.example/dir/My%20File%2B1.zip?x=1"), "My File+1.zip");
        assert_eq!(url_file_name("https://x.example/"), "");
    }
}
