//! Settings helpers: merging edits with changes made elsewhere, and the
//! small text formats settings use.
use crate::fmt;
use serde_json::Value;

/// Three-way merge of settings: start from `latest` and apply every field
/// whose value in `edited` differs from `base`. Keeps edits in progress
/// while taking changes made elsewhere (tray, toolbar, extension).
pub fn merge(base: &Value, edited: &Value, latest: &Value) -> Value {
    if edited == base {
        return latest.clone();
    }
    if latest == base {
        return edited.clone();
    }
    let (Some(b), Some(e), Some(l)) = (base.as_object(), edited.as_object(), latest.as_object()) else {
        return edited.clone();
    };
    let mut out = l.clone();
    for k in b.keys().chain(e.keys()) {
        if b.get(k) != e.get(k) {
            match e.get(k) {
                Some(v) => out.insert(k.clone(), v.clone()),
                None => out.remove(k),
            };
        }
    }
    Value::Object(out)
}

/// "mkv, .WEBM;zip" → ["mkv", "webm", "zip"]
pub fn parse_extensions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for e in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let e = e.trim_matches('.').to_lowercase();
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Put `extensions` in category `id` and take them away from every other
/// category (an extension belongs to one category).
pub fn set_category_extensions(categories: &mut [Value], id: &str, extensions: &[String]) {
    for c in categories.iter_mut() {
        if crate::s(c, "id") == id {
            c["extensions"] = extensions.to_vec().into();
        } else {
            let kept: Vec<String> = crate::strings(c, "extensions").into_iter().filter(|e| !extensions.contains(e)).collect();
            c["extensions"] = kept.into();
        }
    }
}

/// A copied link the clipboard monitor should offer in the Add window: a
/// single http(s) or FTP address whose file type is one the browser capture
/// handles, and that isn't in the list already.
pub fn clipboard_candidate(clip: &str, settings: &Value, downloads: &[Value]) -> Option<String> {
    if !crate::b(settings, "clipboardMonitor") {
        return None;
    }
    let t = clip.trim();
    if t.len() >= 4096 || t.chars().any(char::is_whitespace) || !fmt::is_link(t) {
        return None;
    }
    let ext = fmt::extension(&fmt::url_file_name(t));
    if ext.is_empty() || !crate::strings(settings, "captureExtensions").contains(&ext) {
        return None;
    }
    if downloads.iter().any(|d| crate::s(d, "url") == t && crate::s(d, "status") != "error") {
        return None;
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merges_three_ways() {
        let base = json!({ "a": 1, "b": 2, "c": 3 });
        let edited = json!({ "a": 1, "b": 20, "c": 3 });
        let latest = json!({ "a": 10, "b": 2, "c": 3 });
        assert_eq!(merge(&base, &edited, &latest), json!({ "a": 10, "b": 20, "c": 3 }));
        assert_eq!(merge(&base, &base, &latest), latest);
        assert_eq!(merge(&base, &edited, &base), edited);
    }

    #[test]
    fn clipboard_rules() {
        let s = json!({ "clipboardMonitor": true, "captureExtensions": ["zip"] });
        let have = vec![json!({ "url": "https://x.example/b.zip", "status": "paused" })];
        assert_eq!(clipboard_candidate(" https://x.example/a.zip ", &s, &have).as_deref(), Some("https://x.example/a.zip"));
        assert_eq!(clipboard_candidate("https://x.example/b.zip", &s, &have), None);
        assert_eq!(clipboard_candidate("https://x.example/a.html", &s, &have), None);
        assert_eq!(clipboard_candidate("see https://x.example/a.zip", &s, &have), None);
        assert_eq!(clipboard_candidate("https://x.example/a.zip", &json!({}), &have), None);
    }

    #[test]
    fn extensions_move_between_categories() {
        let mut cats = vec![json!({ "id": "video", "extensions": ["mkv", "ts"] }), json!({ "id": "docs", "extensions": ["pdf"] })];
        set_category_extensions(&mut cats, "docs", &parse_extensions("pdf, .TS"));
        assert_eq!(cats[0]["extensions"], json!(["mkv"]));
        assert_eq!(cats[1]["extensions"], json!(["pdf", "ts"]));
    }
}
