//! HTTP client construction (proxy, TLS, timeouts), request headers and
//! redirect handling.

use crate::model::{Download, Settings};
use crate::util;
use crate::{CoreError, ProbeResult};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION};
use std::time::Duration;

/// Most redirects followed for one request.
pub const MAX_REDIRECTS: usize = 15;

pub fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn base_builder(s: &Settings) -> Result<reqwest::ClientBuilder, CoreError> {
    install_crypto();
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(s.timeout_secs.max(5)))
        .read_timeout(Duration::from_secs(s.timeout_secs.max(5)))
        .pool_max_idle_per_host(32)
        .tcp_nodelay(true);
    if !s.verify_tls {
        b = b.tls_danger_accept_invalid_certs(true);
    }
    match s.proxy_mode.as_str() {
        "none" => b = b.no_proxy(),
        "http" | "socks5" if !s.proxy_host.trim().is_empty() => {
            let scheme = if s.proxy_mode == "http" { "http" } else { "socks5h" };
            let url = format!("{scheme}://{}:{}", s.proxy_host.trim(), s.proxy_port);
            let mut p = reqwest::Proxy::all(&url).map_err(|e| CoreError::Other(format!("proxy: {e}")))?;
            if !s.proxy_user.is_empty() {
                p = p.basic_auth(&s.proxy_user, &s.proxy_pass);
            }
            b = b.proxy(p);
        }
        _ => {} // "system": reqwest reads the OS / environment proxy config
    }
    Ok(b)
}

fn finish(b: reqwest::ClientBuilder) -> Result<reqwest::Client, CoreError> {
    b.build().map_err(|e| CoreError::Other(format!("http client: {e}")))
}

/// General-purpose client (follows redirects itself; used by the grabber).
pub fn build_client(s: &Settings) -> Result<reqwest::Client, CoreError> {
    finish(base_builder(s)?.redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS)))
}

/// The clients the engine uses.
#[derive(Clone)]
pub struct Clients {
    /// follows redirects automatically (grabber, misc)
    pub main: reqwest::Client,
    /// probing: redirects are followed by [`send`], which drops credentials
    /// when the origin changes
    pub probe: reqwest::Client,
    /// segment transfers: HTTP/1.1 only, so every connection is a real TCP
    /// connection (HTTP/2 would multiplex all segments over one)
    pub segment: reqwest::Client,
}

pub fn build_clients(s: &Settings) -> Result<Clients, CoreError> {
    Ok(Clients {
        main: build_client(s)?,
        probe: finish(base_builder(s)?.redirect(reqwest::redirect::Policy::none()))?,
        segment: finish(base_builder(s)?.redirect(reqwest::redirect::Policy::none()).http1_only())?,
    })
}

/// Headers sent with every request for a download.
pub fn request_headers(
    ua: &str,
    referrer: Option<&str>,
    cookies: Option<&str>,
    extra: &[(String, String)],
    user: Option<&str>,
    pass: Option<&str>,
) -> HeaderMap {
    let mut h = HeaderMap::new();
    let put = |h: &mut HeaderMap, k: &str, v: &str| {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            h.insert(k, v);
        }
    };
    put(&mut h, "user-agent", ua);
    put(&mut h, "accept", "*/*");
    // ranges must address raw bytes, never a compressed representation
    put(&mut h, "accept-encoding", "identity");
    if let Some(r) = referrer.filter(|s| !s.is_empty()) {
        put(&mut h, "referer", r);
    }
    if let Some(c) = cookies.filter(|s| !s.is_empty()) {
        put(&mut h, "cookie", c);
    }
    if let (Some(u), p) = (user.filter(|s| !s.is_empty()), pass) {
        let token = b64(format!("{u}:{}", p.unwrap_or("")).as_bytes());
        put(&mut h, "authorization", &format!("Basic {token}"));
    }
    for (k, v) in extra {
        if !k.trim().is_empty() {
            put(&mut h, k.trim(), v.trim());
        }
    }
    h
}

pub fn headers_for(d: &Download, s: &Settings) -> HeaderMap {
    request_headers(
        d.user_agent.as_deref().filter(|u| !u.is_empty()).unwrap_or(&s.user_agent),
        d.referrer.as_deref(),
        d.cookies.as_deref(),
        &d.headers,
        d.username.as_deref(),
        d.password.as_deref(),
    )
}

/// Request headers split by sensitivity. Cookies, credentials and the
/// user's custom headers were given for one origin and are only sent there.
/// The referrer is trimmed for each host the way browsers do it (see
/// [`referer_for`]).
#[derive(Clone, Debug, Default)]
pub struct HeaderSet {
    common: HeaderMap,
    private: HeaderMap,
    origin: Option<url::Url>,
    /// the page the download was started from
    referrer: Option<url::Url>,
    /// follow a redirect from https to plain http
    allow_downgrade: bool,
}

/// Headers that are safe to send to any host.
const COMMON_HEADERS: [&str; 3] = ["user-agent", "accept", "accept-encoding"];

impl HeaderSet {
    /// `origin`: the URL the headers were provided for.
    pub fn new(origin: &str, all: HeaderMap) -> Self {
        let mut common = HeaderMap::new();
        let mut private = HeaderMap::new();
        let mut referrer = None;
        for (k, v) in all.iter() {
            if k.as_str() == "referer" {
                referrer = v.to_str().ok().and_then(|r| url::Url::parse(r.trim()).ok());
            } else if COMMON_HEADERS.contains(&k.as_str()) {
                common.insert(k.clone(), v.clone());
            } else {
                private.append(k.clone(), v.clone());
            }
        }
        Self { common, private, origin: url::Url::parse(origin.trim()).ok(), referrer, allow_downgrade: false }
    }

    pub fn for_download(d: &Download, s: &Settings) -> Self {
        Self::new(&d.url, headers_for(d, s)).allow_downgrade(s.allow_insecure_redirects)
    }

    /// Follow redirects from https to plain http (off by default).
    pub fn allow_downgrade(mut self, allow: bool) -> Self {
        self.allow_downgrade = allow;
        self
    }

    /// Is going on from `from` to `to` refused? A secure (https) download
    /// must not continue over plain http, where anyone on the network path
    /// can alter it, unless the user allowed that. The rule for redirects,
    /// and for the addresses a stream manifest lists.
    pub fn refuses_downgrade(&self, from: &url::Url, to: &url::Url) -> bool {
        from.scheme() == "https" && to.scheme() == "http" && !self.allow_downgrade
    }

    /// Headers for a request to `target`.
    pub fn for_url(&self, target: &url::Url) -> HeaderMap {
        let mut h = self.common.clone();
        let referer = self.referrer.as_ref().and_then(|r| referer_for(r, target));
        if let Some(v) = referer.and_then(|r| HeaderValue::from_str(&r).ok()) {
            h.insert(reqwest::header::REFERER, v);
        }
        if self.origin.as_ref().is_some_and(|o| credentials_allowed(o, target)) {
            for k in self.private.keys() {
                h.remove(k);
            }
            for (k, v) in self.private.iter() {
                h.append(k.clone(), v.clone());
            }
        }
        h
    }
}

fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str().map(str::to_ascii_lowercase) == b.host_str().map(str::to_ascii_lowercase)
        && a.port_or_known_default() == b.port_or_known_default()
}

/// The `Referer` to send to `target` for a download started from the page
/// `referrer`, following the browsers' default policy
/// (strict-origin-when-cross-origin): the full page address (without user
/// info and fragment) to the page's own origin, only the origin to other
/// hosts, and nothing when going from https to plain http.
pub fn referer_for(referrer: &url::Url, target: &url::Url) -> Option<String> {
    if !matches!(referrer.scheme(), "http" | "https") {
        return None;
    }
    if referrer.scheme() == "https" && target.scheme() != "https" {
        return None;
    }
    let mut r = referrer.clone();
    let _ = r.set_username("");
    let _ = r.set_password(None);
    r.set_fragment(None);
    if !same_origin(&r, target) {
        r.set_path("/");
        r.set_query(None);
    }
    Some(r.to_string())
}

/// May credentials given for `origin` be sent to `target`? Same scheme,
/// host and port; or an upgrade from http to https on the same host with
/// default ports. Never across hosts and never from https down to http.
pub fn credentials_allowed(origin: &url::Url, target: &url::Url) -> bool {
    if origin.host_str().map(str::to_ascii_lowercase) != target.host_str().map(str::to_ascii_lowercase) {
        return false;
    }
    match (origin.scheme(), target.scheme()) {
        (a, b) if a == b => origin.port_or_known_default() == target.port_or_known_default(),
        ("http", "https") => origin.port_or_known_default() == Some(80) && target.port_or_known_default() == Some(443),
        _ => false,
    }
}

/// GET `url`, following redirects (up to [`MAX_REDIRECTS`]) with the
/// headers each hop is allowed to receive. `extra` headers (Range, If-Range)
/// go to every hop.
pub async fn send(
    client: &reqwest::Client,
    url: &str,
    hs: &HeaderSet,
    extra: &[(&'static str, String)],
    timeout: Option<Duration>,
) -> Result<reqwest::Response, CoreError> {
    let mut cur = url::Url::parse(url.trim()).map_err(|_| CoreError::BadUrl(util::redact_url(url)))?;
    for _ in 0..=MAX_REDIRECTS {
        if !matches!(cur.scheme(), "http" | "https") {
            return Err(CoreError::BadUrl(format!("unsupported scheme: {}", cur.scheme())));
        }
        let mut h = hs.for_url(&cur);
        for (k, v) in extra {
            if let Ok(v) = HeaderValue::from_str(v) {
                h.insert(*k, v);
            }
        }
        let mut rb = client.get(cur.clone()).headers(h);
        if let Some(t) = timeout {
            rb = rb.timeout(t);
        }
        let resp = rb.send().await.map_err(CoreError::from_reqwest)?;
        let st = resp.status();
        if st.is_redirection() && st != reqwest::StatusCode::NOT_MODIFIED {
            let next = resp
                .headers()
                .get(LOCATION)
                .map(|v| util::header_text(v.as_bytes()))
                .and_then(|loc| cur.join(loc.trim()).ok());
            if let Some(next) = next {
                // a secure download must not silently continue over plain
                // http, where anyone on the network path can alter the file
                if hs.refuses_downgrade(&cur, &next) {
                    return Err(CoreError::InsecureRedirect);
                }
                cur = next;
                continue;
            }
        }
        return Ok(resp);
    }
    Err(CoreError::Network("too many redirects".into()))
}

pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn protocol_label(v: reqwest::Version, tls: bool) -> String {
    let p = match v {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_11 => "HTTP/1.1",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "HTTP",
    };
    if tls {
        format!("{p} · TLS")
    } else {
        p.to_string()
    }
}

/// Map a failed HTTP status to a user-facing message.
pub fn status_message(code: u16) -> String {
    let why = match code {
        401 => "authentication required",
        403 => "access denied — the link may have expired; use Refresh link",
        404 => "file not found on server",
        410 => "link expired; use Refresh link",
        416 => "requested range not satisfiable",
        429 => "server is rate-limiting requests",
        503 => "server is busy",
        500..=599 => "server error",
        _ => "request failed",
    };
    format!("HTTP {code}: {why}")
}

/// A parsed `Content-Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    /// first and last byte (inclusive); None for `bytes */N`
    pub range: Option<(u64, u64)>,
    /// complete length; None for `/*`
    pub total: Option<u64>,
}

pub fn parse_content_range(v: &str) -> Option<ContentRange> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (r, t) = rest.split_once('/')?;
    let total = match t.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    let range = match r.trim() {
        "*" => None,
        r => {
            let (a, b) = r.split_once('-')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        }
    };
    Some(ContentRange { range, total })
}

/// Probe a URL: size, range support, name, type. Uses a 1-byte range GET
/// (HEAD is unreliable on many CDNs). All headers are treated as belonging
/// to `url`'s origin.
pub async fn probe(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    cats: &[crate::model::Category],
) -> Result<ProbeResult, CoreError> {
    probe_with(client, url, &HeaderSet::new(url, headers), cats).await
}

pub async fn probe_with(
    client: &reqwest::Client,
    url: &str,
    hs: &HeaderSet,
    cats: &[crate::model::Category],
) -> Result<ProbeResult, CoreError> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| CoreError::BadUrl(util::redact_url(url)))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CoreError::BadUrl(format!("unsupported scheme: {}", parsed.scheme())));
    }
    let resp = send(client, parsed.as_str(), hs, &[("range", "bytes=0-0".into())], None).await?;
    let status = resp.status().as_u16();
    let h = resp.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string());
    let content_range = get("content-range").and_then(|cr| parse_content_range(&cr));
    let (total, resumable) = match status {
        206 => match content_range {
            Some(ContentRange { range: Some((0, _)), total: Some(t) }) => (Some(t), true),
            _ => (None, false),
        },
        200..=299 => (get("content-length").and_then(|v| v.parse().ok()), false),
        // an empty file: no byte 0 to send
        416 if matches!(content_range, Some(ContentRange { range: None, total: Some(0) })) => (Some(0), false),
        _ => return Err(CoreError::Http(status)),
    };
    let final_url = resp.url().to_string();
    let mime = get("content-type").map(|m| m.split(';').next().unwrap_or("").trim().to_string());
    let mut name = h
        .get("content-disposition")
        .map(|v| util::header_text(v.as_bytes()))
        .and_then(|cd| util::filename_from_disposition(&cd))
        .or_else(|| util::filename_from_url(&final_url))
        .or_else(|| util::filename_from_url(url))
        .unwrap_or_else(|| "download".into());
    if util::extension(&name).is_empty() {
        if let Some(e) = mime.as_deref().and_then(util::ext_for_mime) {
            name = format!("{name}.{e}");
        }
    }
    let protocol = protocol_label(resp.version(), final_url.starts_with("https"));
    let etag = get("etag");
    let last_modified = get("last-modified");
    drop(resp);
    Ok(ProbeResult {
        category: util::category_for(&name, cats),
        final_url,
        file_name: name,
        total_bytes: total,
        mime,
        resumable,
        protocol,
        etag,
        last_modified,
        stream: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn credential_scope() {
        assert!(credentials_allowed(&u("https://a.example/x"), &u("https://a.example:443/y")));
        assert!(credentials_allowed(&u("http://a.example/x"), &u("https://a.example/y")));
        assert!(!credentials_allowed(&u("https://a.example/x"), &u("http://a.example/y")));
        assert!(!credentials_allowed(&u("https://a.example/x"), &u("https://cdn.example/y")));
        assert!(!credentials_allowed(&u("http://127.0.0.1:1000/x"), &u("http://127.0.0.1:1001/y")));
    }

    #[test]
    fn header_split() {
        let all = request_headers("ua", Some("https://ref/"), Some("s=1"), &[("X-Key".into(), "k".into())], Some("u"), Some("p"));
        let hs = HeaderSet::new("https://a.example/f", all);
        let same = hs.for_url(&u("https://a.example/g"));
        assert!(same.contains_key("cookie") && same.contains_key("authorization") && same.contains_key("x-key"));
        let other = hs.for_url(&u("https://b.example/g"));
        assert!(!other.contains_key("cookie") && !other.contains_key("authorization") && !other.contains_key("x-key"));
        assert_eq!(other.get("user-agent").unwrap(), "ua");
        assert!(other.contains_key("referer"));
    }

    #[test]
    fn downgrade_rule() {
        let hs = HeaderSet::new("https://a.example/f", HeaderMap::new());
        assert!(hs.refuses_downgrade(&u("https://a.example/f"), &u("http://a.example/f")));
        assert!(hs.refuses_downgrade(&u("https://a.example/f"), &u("http://cdn.example/f")));
        assert!(!hs.refuses_downgrade(&u("https://a.example/f"), &u("https://cdn.example/f")));
        assert!(!hs.refuses_downgrade(&u("http://a.example/f"), &u("http://cdn.example/f")));
        assert!(!hs.refuses_downgrade(&u("http://a.example/f"), &u("https://a.example/f")));
        let allowed = hs.allow_downgrade(true);
        assert!(!allowed.refuses_downgrade(&u("https://a.example/f"), &u("http://a.example/f")));
    }

    #[test]
    fn referer_policy() {
        let page = u("https://user:pw@site.example/inbox?session=SECRET#frag");
        // the page's own origin gets the page address, never user info or fragment
        assert_eq!(
            referer_for(&page, &u("https://site.example/files/a.zip")).as_deref(),
            Some("https://site.example/inbox?session=SECRET")
        );
        // other hosts only learn the origin
        assert_eq!(referer_for(&page, &u("https://cdn.example/a.zip")).as_deref(), Some("https://site.example/"));
        assert_eq!(referer_for(&page, &u("https://site.example:8443/a.zip")).as_deref(), Some("https://site.example/"));
        // nothing from https to plain http
        assert_eq!(referer_for(&page, &u("http://site.example/a.zip")), None);
        assert_eq!(referer_for(&u("http://a.example/p?q=1"), &u("https://b.example/x")).as_deref(), Some("http://a.example/"));
        assert_eq!(referer_for(&u("file:///C:/page.html"), &u("https://b.example/x")), None);

        let all = request_headers("ua", Some("https://site.example/inbox?session=SECRET"), None, &[], None, None);
        let hs = HeaderSet::new("https://site.example/files/a.zip", all);
        let to = |t: &str| hs.for_url(&u(t)).get("referer").map(|v| v.to_str().unwrap().to_string());
        assert_eq!(to("https://site.example/files/a.zip").as_deref(), Some("https://site.example/inbox?session=SECRET"));
        assert_eq!(to("https://cdn.example/a.zip").as_deref(), Some("https://site.example/"));
        assert_eq!(to("http://cdn.example/a.zip"), None);
    }

    #[test]
    fn content_range() {
        assert_eq!(
            parse_content_range("bytes 0-0/10"),
            Some(ContentRange { range: Some((0, 0)), total: Some(10) })
        );
        assert_eq!(parse_content_range("bytes */0"), Some(ContentRange { range: None, total: Some(0) }));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some(ContentRange { range: Some((5, 9)), total: None }));
        assert_eq!(parse_content_range("items 1-2/3"), None);
    }
}
