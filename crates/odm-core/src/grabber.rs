//! Site grabber: fetch a page (optionally following same-site links one
//! or more levels deep) and list downloadable files it links to.

use crate::model::{Category, GrabbedLink};
use crate::util;
use crate::CoreError;
use reqwest::header::HeaderMap;
use scraper::{Html, Selector};
use std::collections::{HashSet, VecDeque};

pub struct GrabOptions {
    pub depth: u32,
    pub max_pages: usize,
    pub same_host: bool,
    /// lowercase extensions to keep; empty = anything that looks like a file
    pub extensions: Vec<String>,
}

pub async fn grab(
    client: &reqwest::Client,
    start: &str,
    headers: HeaderMap,
    opts: GrabOptions,
    cats: &[Category],
) -> Result<Vec<GrabbedLink>, CoreError> {
    let start_url = url::Url::parse(start.trim()).map_err(|_| CoreError::BadUrl(start.into()))?;
    let host = start_url.host_str().unwrap_or("").to_string();
    let exts: HashSet<String> = opts.extensions.iter().map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase()).collect();

    let mut seen_pages = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut out = vec![];
    let mut q = VecDeque::new();
    q.push_back((start_url.clone(), 0u32));
    let mut first_error = None;

    while let Some((page, depth)) = q.pop_front() {
        if seen_pages.len() >= opts.max_pages.max(1) {
            break;
        }
        let key = strip_fragment(&page);
        if !seen_pages.insert(key) {
            continue;
        }
        let resp = match client.get(page.clone()).headers(headers.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                first_error.get_or_insert(CoreError::from_reqwest(e));
                continue;
            }
        };
        if !resp.status().is_success() {
            first_error.get_or_insert(CoreError::Http(resp.status().as_u16()));
            continue;
        }
        let is_html = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|c| c.contains("html"))
            .unwrap_or(true);
        if !is_html {
            continue;
        }
        let base = resp.url().clone();
        let body = match resp.text().await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let found = extract(&body, &base);
        for (u, source) in found {
            let ext = util::extension(u.path());
            let path_lower = u.path().to_ascii_lowercase();
            let looks_page = ext.is_empty()
                || matches!(ext.as_str(), "html" | "htm" | "php" | "asp" | "aspx" | "jsp" | "shtml");
            let wanted = if exts.is_empty() { !looks_page } else { exts.contains(&ext) };
            if wanted {
                let k = strip_fragment(&u);
                if seen_files.insert(k.clone()) {
                    let name = util::filename_from_url(&k).unwrap_or_else(|| "download".into());
                    out.push(GrabbedLink {
                        category: util::category_for(&name, cats),
                        ext: util::badge_ext(&name),
                        file_name: name,
                        url: k,
                        source: source.to_string(),
                    });
                }
            } else if looks_page && depth < opts.depth && source == "link" {
                let same = u.host_str().unwrap_or("") == host;
                if (!opts.same_host || same) && !path_lower.ends_with(".css") {
                    q.push_back((u, depth + 1));
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    Ok(out)
}

fn strip_fragment(u: &url::Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.to_string()
}

fn extract(html: &str, base: &url::Url) -> Vec<(url::Url, &'static str)> {
    let doc = Html::parse_document(html);
    let mut v = vec![];
    let sel = |s: &str| Selector::parse(s).ok();
    let pairs: [(&str, &str, &'static str); 6] = [
        ("a[href]", "href", "link"),
        ("img[src]", "src", "image"),
        ("video[src]", "src", "video"),
        ("audio[src]", "src", "audio"),
        ("source[src]", "src", "media"),
        ("embed[src]", "src", "embed"),
    ];
    for (css, attr, source) in pairs {
        let Some(s) = sel(css) else { continue };
        for el in doc.select(&s) {
            if let Some(raw) = el.value().attr(attr) {
                let raw = raw.trim();
                if raw.starts_with("javascript:") || raw.starts_with("mailto:") || raw.starts_with('#') {
                    continue;
                }
                if let Ok(u) = base.join(raw) {
                    if matches!(u.scheme(), "http" | "https") {
                        v.push((u, source));
                    }
                }
            }
        }
    }
    v
}
