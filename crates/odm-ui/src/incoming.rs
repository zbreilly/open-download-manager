//! Command-line arguments and URLs handed to the app (`odm://add?url=…`,
//! plain http(s) links, `--minimized`), with the same rules as the other
//! front ends: a link from a web page only carries its URL and is marked
//! `source: "web"` so the Add window doesn't contact it or start by itself.
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// open the Add window with these params (`{url, source}` as JSON)
    Add(Value),
    /// bring the main window up
    Show,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Args {
    /// start hidden in the tray (login item)
    pub minimized: bool,
    pub items: Vec<Incoming>,
}

/// Parse argv (without the program name).
pub fn parse(args: &[String]) -> Args {
    let mut out = Args::default();
    for a in args {
        if a == "--minimized" || a == "--hidden" {
            out.minimized = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        if let Some(i) = parse_url(a) {
            out.items.push(i);
        }
    }
    out
}

/// One URL: `odm://add?url=…`, `odm://` (show), or an http(s) link.
pub fn parse_url(s: &str) -> Option<Incoming> {
    let u = url::Url::parse(s.trim()).ok()?;
    match u.scheme() {
        "odm" => {
            if u.host_str().unwrap_or("add") != "add" && u.path() != "add" {
                return Some(Incoming::Show);
            }
            let target = u.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())?;
            let t = url::Url::parse(&target).ok()?;
            if !matches!(t.scheme(), "http" | "https") {
                return None;
            }
            Some(Incoming::Add(json!({ "url": t.as_str(), "source": "web" })))
        }
        "http" | "https" => Some(Incoming::Add(json!({ "url": u.as_str(), "source": "web" }))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses() {
        let a = parse(&["--minimized".into(), "odm://add?url=https%3A%2F%2Fx.example%2Fa.zip&name=evil.exe".into()]);
        assert!(a.minimized);
        assert_eq!(a.items, vec![Incoming::Add(json!({"url":"https://x.example/a.zip","source":"web"}))]);
        assert_eq!(parse_url("odm://add?url=file:///etc/passwd"), None);
        assert_eq!(parse_url("odm://show"), Some(Incoming::Show));
        assert_eq!(parse_url("ftp://x"), None);
    }
}
