//! Site grabber: fetch a page (optionally following same-site links one
//! or more levels deep) and list downloadable files it links to.

use crate::model::{Category, GrabbedLink};
use crate::util;
use crate::CoreError;
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use scraper::{Html, Selector};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;

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
        let (base, body) = match fetch_page(client, &page, &headers).await {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                first_error.get_or_insert(e);
                continue;
            }
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

/// Largest page body read; anything beyond is ignored.
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
/// Time allowed for one page, body included.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Does a body without a Content-Type look like HTML?
fn sniff_html(body: &[u8]) -> bool {
    let head = &body[..body.len().min(1024)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with("<!doctype html") || t.starts_with("<html") || ["<head", "<body", "<a ", "<title"].iter().any(|m| t.contains(m))
}

/// Fetch one page. `Ok(None)` = not an HTML page.
async fn fetch_page(
    client: &reqwest::Client,
    page: &url::Url,
    headers: &HeaderMap,
) -> Result<Option<(url::Url, String)>, CoreError> {
    let fut = async {
        let resp = client.get(page.clone()).headers(headers.clone()).send().await.map_err(CoreError::from_reqwest)?;
        if !resp.status().is_success() {
            return Err(CoreError::Http(resp.status().as_u16()));
        }
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
        let need_sniff = match ct.as_deref() {
            Some("text/html") | Some("application/xhtml+xml") => false,
            None | Some("") => true,
            Some(_) => return Ok(None),
        };
        let base = resp.url().clone();
        let mut body: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(CoreError::from_reqwest)?;
            let room = MAX_PAGE_BYTES - body.len();
            body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            if body.len() >= MAX_PAGE_BYTES {
                break;
            }
            if need_sniff && body.len() >= 1024 && !sniff_html(&body) {
                return Ok(None);
            }
        }
        if need_sniff && !sniff_html(&body) {
            return Ok(None);
        }
        Ok(Some((base, String::from_utf8_lossy(&body).into_owned())))
    };
    match tokio::time::timeout(PAGE_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(CoreError::Network("page took too long to load".into())),
    }
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
