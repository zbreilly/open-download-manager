//! HTTP client construction (proxy, TLS, timeouts) and request headers.

use crate::model::{Download, Settings};
use crate::util;
use crate::{CoreError, ProbeResult};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;

pub fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn build_client(s: &Settings) -> Result<reqwest::Client, CoreError> {
    install_crypto();
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(s.timeout_secs.max(5)))
        .read_timeout(Duration::from_secs(s.timeout_secs.max(5)))
        .redirect(reqwest::redirect::Policy::limited(15))
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
    b.build().map_err(|e| CoreError::Other(format!("http client: {e}")))
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
        500..=599 => "server error",
        _ => "request failed",
    };
    format!("HTTP {code}: {why}")
}

/// Probe a URL: size, range support, name, type. Uses a 1-byte range GET
/// (HEAD is unreliable on many CDNs).
pub async fn probe(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    cats: &[crate::model::Category],
) -> Result<ProbeResult, CoreError> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| CoreError::BadUrl(url.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CoreError::BadUrl(format!("unsupported scheme: {}", parsed.scheme())));
    }
    let resp = client
        .get(parsed.clone())
        .headers(headers)
        .header("range", "bytes=0-0")
        .send()
        .await
        .map_err(CoreError::from_reqwest)?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(CoreError::Http(status));
    }
    let h = resp.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let final_url = resp.url().to_string();
    let (total, resumable) = if status == 206 {
        let total = get("content-range")
            .and_then(|cr| cr.rsplit('/').next().map(|t| t.trim().to_string()))
            .and_then(|t| t.parse::<u64>().ok());
        (total, total.is_some())
    } else {
        (get("content-length").and_then(|v| v.trim().parse().ok()), false)
    };
    let mime = get("content-type").map(|m| m.split(';').next().unwrap_or("").trim().to_string());
    let mut name = get("content-disposition")
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
    })
}
