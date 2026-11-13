//! File-name, category, batch-pattern and hashing helpers.

use crate::model::Category;
use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

/// File name from a `Content-Disposition` header (RFC 6266 / 5987).
/// `filename*` (charset'lang'percent-encoded) wins over `filename`.
pub fn filename_from_disposition(cd: &str) -> Option<String> {
    let params = disposition_params(cd);
    if let Some((_, v)) = params.iter().find(|(k, _)| k == "filename*") {
        if let Some(s) = decode_ext_value(v).filter(|s| !s.trim().is_empty()) {
            return Some(sanitize_filename(&s));
        }
    }
    if let Some((_, v)) = params.iter().find(|(k, _)| k == "filename") {
        // some servers percent-encode plain `filename`; decode only when that
        // yields valid UTF-8
        let s = if v.contains('%') {
            percent_decode_str(v).decode_utf8().map(|c| c.into_owned()).unwrap_or_else(|_| v.clone())
        } else {
            v.clone()
        };
        if !s.trim().is_empty() {
            return Some(sanitize_filename(&s));
        }
    }
    None
}

/// Header bytes as text: UTF-8 when valid, otherwise ISO-8859-1.
pub fn header_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(raw),
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `type; k=v; k2="quoted \"v\""` -> [(k, v), (k2, quoted "v")], keys lowercase.
fn disposition_params(cd: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let b: Vec<char> = cd.chars().collect();
    let mut i = 0;
    // skip the disposition type
    while i < b.len() && b[i] != ';' {
        i += 1;
    }
    while i < b.len() {
        while i < b.len() && (b[i] == ';' || b[i].is_whitespace()) {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != '=' && b[i] != ';' {
            i += 1;
        }
        let key = b[ks..i].iter().collect::<String>().trim().to_ascii_lowercase();
        if i >= b.len() || b[i] == ';' {
            continue;
        }
        i += 1; // '='
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == '"' {
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    i += 1;
                }
                val.push(b[i]);
                i += 1;
            }
            // skip the closing quote and anything up to the next ';'
            while i < b.len() && b[i] != ';' {
                i += 1;
            }
        } else {
            while i < b.len() && b[i] != ';' {
                val.push(b[i]);
                i += 1;
            }
            val = val.trim().to_string();
        }
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

/// RFC 5987 `charset'lang'value` with percent-encoded bytes.
fn decode_ext_value(v: &str) -> Option<String> {
    let v = v.trim().trim_matches('"');
    let (charset, enc) = match v.split_once('\'') {
        Some((cs, rest)) => (cs.trim().to_ascii_lowercase(), rest.split_once('\'').map(|(_, e)| e).unwrap_or(rest)),
        None => ("utf-8".to_string(), v),
    };
    let bytes: Vec<u8> = percent_decode_str(enc).collect();
    Some(match charset.as_str() {
        "iso-8859-1" | "latin1" | "latin-1" => latin1(&bytes),
        _ => String::from_utf8(bytes.clone()).unwrap_or_else(|_| latin1(&bytes)),
    })
}

/// A URL without user info, query and fragment (safe to show and store).
pub fn redact_url(u: &str) -> String {
    match url::Url::parse(u) {
        Ok(mut p) => {
            let _ = p.set_username("");
            let _ = p.set_password(None);
            p.set_query(None);
            p.set_fragment(None);
            p.to_string()
        }
        Err(_) => u.split(['?', '#']).next().unwrap_or("").to_string(),
    }
}

/// A URL without user info (query kept).
pub fn strip_userinfo(u: &str) -> String {
    match url::Url::parse(u) {
        Ok(mut p) => {
            let _ = p.set_username("");
            let _ = p.set_password(None);
            p.to_string()
        }
        Err(_) => u.to_string(),
    }
}

/// Replace every http(s) and FTP URL inside `text` with its redacted form.
pub fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let at = ["http://", "https://", "ftp://", "ftps://", "ftpes://"].iter().filter_map(|p| rest.find(p)).min();
        let Some(at) = at else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '<' | '>' | ','))
            .unwrap_or(tail.len());
        out.push_str(&redact_url(&tail[..end]));
        rest = &tail[end..];
    }
}

/// Last path segment of a URL, percent-decoded.
pub fn filename_from_url(u: &str) -> Option<String> {
    let parsed = url::Url::parse(u).ok()?;
    let seg = parsed.path_segments()?.rfind(|s| !s.is_empty())?.to_string();
    let s = percent_decode_str(&seg).decode_utf8_lossy().into_owned();
    if s.trim().is_empty() {
        None
    } else {
        Some(sanitize_filename(&s))
    }
}

pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    Some(match m.as_str() {
        "application/zip" => "zip",
        "application/pdf" => "pdf",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-msdownload" | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-apple-diskimage" => "dmg",
        "application/x-iso9660-image" => "iso",
        "application/vnd.debian.binary-package" => "deb",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "text/plain" => "txt",
        "text/html" => "html",
        "application/json" => "json",
        _ => return None,
    })
}

/// Invisible or control characters that never belong in a file name:
/// C0/C1 controls, DEL, and bidirectional overrides (RTLO spoofing).
fn is_stripped_char(c: char) -> bool {
    let u = c as u32;
    u < 0x20
        || (0x7f..=0x9f).contains(&u)
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'
        )
}

/// Longest file name we produce, in bytes.
pub const MAX_NAME_BYTES: usize = 200;

/// Remove characters that are illegal or deceptive on any of
/// Windows/macOS/Linux and keep the name within [`MAX_NAME_BYTES`].
pub fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|&c| !is_stripped_char(c))
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let mut out = trim_name(&cleaned);
    if out.is_empty() || out == "." || out == ".." {
        out = "download".into();
    }
    // Windows reserved device names (on every platform: files travel)
    let stem = out.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out = format!("_{out}");
    }
    if out.len() > MAX_NAME_BYTES {
        let ext = extension(&out);
        // keep the extension as spelled
        let suffix = if ext.is_empty() { String::new() } else { out[out.len() - ext.len() - 1..].to_string() };
        let mut cut = MAX_NAME_BYTES - suffix.len();
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        let stem = trim_name(&out[..cut]);
        out = if stem.is_empty() { format!("download{suffix}") } else { format!("{stem}{suffix}") };
    }
    out
}

fn trim_name(s: &str) -> String {
    s.trim().trim_end_matches(['.', ' ']).trim().to_string()
}

/// Lowercase extension without the dot ("" if none).
pub fn extension(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for multi in ["tar.gz", "tar.bz2", "tar.xz", "tar.zst"] {
        if lower.ends_with(&format!(".{multi}")) {
            return multi.split('.').next_back().unwrap().to_string();
        }
    }
    match lower.rfind('.') {
        Some(i) if i + 1 < lower.len() && lower.len() - i <= 12 => lower[i + 1..].to_string(),
        _ => String::new(),
    }
}

/// Short uppercase badge text (max 4 chars).
pub fn badge_ext(name: &str) -> String {
    let e = extension(name).to_ascii_uppercase();
    if e.is_empty() {
        "FILE".into()
    } else {
        e.chars().take(4).collect()
    }
}

pub fn category_for(name: &str, cats: &[Category]) -> String {
    let e = extension(name);
    if !e.is_empty() {
        for c in cats {
            if c.extensions.iter().any(|x| x.eq_ignore_ascii_case(&e)) {
                return c.id.clone();
            }
        }
    }
    cats.iter()
        .find(|c| c.id == "general")
        .or_else(|| cats.last())
        .map(|c| c.id.clone())
        .unwrap_or_else(|| "general".into())
}

/// `name (1).ext`, `name (2).ext`, ... for the first path that doesn't exist.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() && !part_exists(&p) {
        return p;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10_000 {
        let cand = dir.join(format!("{stem} ({n}){ext}"));
        if !cand.exists() && !part_exists(&cand) {
            return cand;
        }
    }
    p
}

fn part_exists(p: &Path) -> bool {
    part_path(p).exists()
}

pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_os_string();
    s.push(".odmpart");
    PathBuf::from(s)
}

/// Mark a finished download as coming from the internet so the OS applies
/// its usual checks when it is opened: the `Zone.Identifier` stream
/// (Mark-of-the-Web) on Windows, the `com.apple.quarantine` attribute on
/// macOS. Failures (e.g. FAT/exFAT volumes) are ignored.
pub fn mark_downloaded(path: &Path, url: &str, referrer: Option<&str>) {
    #[cfg(windows)]
    {
        let mut ads = path.as_os_str().to_os_string();
        ads.push(":Zone.Identifier");
        if let Err(e) = std::fs::write(&ads, zone_identifier(url, referrer)) {
            tracing::debug!("zone identifier not written for {}: {e}", path.display());
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (url, referrer);
        set_quarantine(path);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, url, referrer);
    }
}

/// Contents of the Windows `Zone.Identifier` stream for a download.
pub fn zone_identifier(url: &str, referrer: Option<&str>) -> String {
    let mut s = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
    if let Some(r) = referrer.map(str::trim).filter(|r| !r.is_empty()) {
        s.push_str(&format!("ReferrerUrl={}\r\n", redact_url(r)));
    }
    s.push_str(&format!("HostUrl={}\r\n", strip_userinfo(url)));
    s
}

/// Value of the macOS `com.apple.quarantine` attribute.
pub fn quarantine_value(unix_secs: u64) -> String {
    format!(
        "0081;{unix_secs:x};Open Download Manager;{}",
        uuid::Uuid::new_v4().to_string().to_ascii_uppercase()
    )
}

#[cfg(target_os = "macos")]
fn set_quarantine(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(cpath) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let value = quarantine_value(secs);
    let name = c"com.apple.quarantine";
    // SAFETY: path and name are NUL-terminated C strings; value is valid for value.len() bytes.
    let rc = unsafe {
        libc::setxattr(cpath.as_ptr(), name.as_ptr(), value.as_ptr() as *const libc::c_void, value.len(), 0, 0)
    };
    if rc != 0 {
        tracing::debug!("quarantine attribute not set for {}: {}", path.display(), std::io::Error::last_os_error());
    }
}

/// Expand IDM-style batch patterns: `file[001-120].jpg`, `img[a-z].png`.
/// Multiple patterns expand as a cartesian product. Capped at 10,000 URLs.
pub fn expand_batch(pattern: &str) -> Vec<String> {
    let mut results = vec![String::new()];
    let mut rest = pattern;
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find(']') else { break };
        let close = open + close_rel;
        let inner = &rest[open + 1..close];
        let Some(values) = range_values(inner) else {
            // not a range — keep literally
            for r in results.iter_mut() {
                r.push_str(&rest[..=close]);
            }
            rest = &rest[close + 1..];
            continue;
        };
        let prefix = &rest[..open];
        let mut next = Vec::new();
        for r in &results {
            for v in &values {
                next.push(format!("{r}{prefix}{v}"));
                if next.len() >= 10_000 {
                    break;
                }
            }
        }
        results = next;
        rest = &rest[close + 1..];
    }
    for r in results.iter_mut() {
        r.push_str(rest);
    }
    results
}

fn range_values(inner: &str) -> Option<Vec<String>> {
    let (a, b) = inner.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if let (Ok(x), Ok(y)) = (a.parse::<u64>(), b.parse::<u64>()) {
        let width = if a.starts_with('0') && a.len() > 1 { a.len() } else { 0 };
        let (lo, hi) = (x.min(y), x.max(y));
        if hi - lo > 10_000 {
            return None;
        }
        return Some((lo..=hi).map(|n| format!("{n:0width$}")).collect());
    }
    let (ca, cb) = (a.chars().collect::<Vec<_>>(), b.chars().collect::<Vec<_>>());
    if ca.len() == 1 && cb.len() == 1 && ca[0].is_ascii_alphabetic() && cb[0].is_ascii_alphabetic() {
        let (lo, hi) = (ca[0].min(cb[0]) as u8, ca[0].max(cb[0]) as u8);
        return Some((lo..=hi).map(|c| (c as char).to_string()).collect());
    }
    None
}

pub fn to_hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

/// Bytes of a hex string (either case); None if it isn't one.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    b.chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// Hash a file with the named algorithm ("sha256" | "sha1" | "md5").
pub fn hash_file(path: &Path, algo: &str) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    macro_rules! run {
        ($h:expr) => {{
            use sha2::Digest;
            let mut h = $h;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(to_hex(&h.finalize()))
        }};
    }
    match algo.to_ascii_lowercase().as_str() {
        "md5" => run!(md5::Md5::new()),
        "sha1" => run!(sha1::Sha1::new()),
        _ => run!(sha2::Sha256::new()),
    }
}

/// Guess the hash algorithm from the hex length.
pub fn guess_algo(hex: &str) -> &'static str {
    match hex.trim().len() {
        32 => "md5",
        40 => "sha1",
        _ => "sha256",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition() {
        assert_eq!(
            filename_from_disposition("attachment; filename=\"a b.zip\"").as_deref(),
            Some("a b.zip")
        );
        assert_eq!(
            filename_from_disposition("attachment; filename=x.bin; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf").as_deref(),
            Some("résumé.pdf")
        );
    }

    #[test]
    fn url_names() {
        assert_eq!(filename_from_url("https://h/a/b%20c.iso?x=1").as_deref(), Some("b c.iso"));
        assert_eq!(filename_from_url("https://h/"), None);
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_filename("a/b:c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename("COM0.txt"), "_COM0.txt");
        assert_eq!(sanitize_filename("lpt9"), "_lpt9");
        assert_eq!(sanitize_filename("conin$.log"), "_conin$.log");
        assert_eq!(sanitize_filename("CONOUT$"), "_CONOUT$");
        assert_eq!(sanitize_filename("COM10.txt"), "COM10.txt");
        assert_eq!(sanitize_filename("name. . "), "name");
        // RTLO spoofing: would display as "invoiceexe.pdf"
        assert_eq!(sanitize_filename("invoice\u{202e}fdp.exe"), "invoicefdp.exe");
        assert_eq!(sanitize_filename("a\u{2066}b\u{200f}c\u{7f}d\u{85}e\u{1}f\tg"), "abcdefg");
        let long = format!("{}.tar.gz", "x".repeat(400));
        let s = sanitize_filename(&long);
        assert!(s.len() <= MAX_NAME_BYTES, "{}", s.len());
        assert!(s.ends_with("x.gz"), "{s}");
        let long = format!("{}.MKV", "é".repeat(300));
        let s = sanitize_filename(&long);
        assert!(s.len() <= MAX_NAME_BYTES && s.ends_with(".MKV"), "{s}");
        let s = sanitize_filename(&"y".repeat(500));
        assert_eq!(s.len(), MAX_NAME_BYTES);
    }

    #[test]
    fn disposition_variants() {
        assert_eq!(
            filename_from_disposition("attachment; filename*=UTF-8''%E2%82%AC%20rates.pdf").as_deref(),
            Some("€ rates.pdf")
        );
        assert_eq!(
            filename_from_disposition("attachment; filename*=iso-8859-1'en'%A3%20rates.pdf").as_deref(),
            Some("£ rates.pdf")
        );
        assert_eq!(
            filename_from_disposition(r#"attachment; filename="a;b \"q\".zip"; size=3"#).as_deref(),
            Some("a;b _q_.zip")
        );
        // raw UTF-8 bytes in the header
        let raw = "attachment; filename=\"r\u{e9}sum\u{e9}.pdf\"".as_bytes();
        assert_eq!(filename_from_disposition(&header_text(raw)).as_deref(), Some("résumé.pdf"));
        // raw latin-1 bytes
        let raw = b"attachment; filename=\"caf\xe9.txt\"";
        assert_eq!(filename_from_disposition(&header_text(raw)).as_deref(), Some("café.txt"));
        assert_eq!(filename_from_disposition("inline"), None);
    }

    #[test]
    fn redaction() {
        assert_eq!(redact_url("https://u:p@h.example/a/b.zip?token=1#x"), "https://h.example/a/b.zip");
        assert_eq!(
            redact_urls("error sending request for url (https://h/x.bin?sig=SECRET): reset"),
            "error sending request for url (https://h/x.bin): reset"
        );
        assert_eq!(strip_userinfo("https://u:p@h/x?y=1"), "https://h/x?y=1");
    }

    #[test]
    fn web_marks() {
        let z = zone_identifier("https://u:p@cdn.example/f.exe?x=1", Some("https://site.example/page?q=2"));
        assert_eq!(
            z,
            "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://site.example/page\r\nHostUrl=https://cdn.example/f.exe?x=1\r\n"
        );
        let q = quarantine_value(0x5f00_0000);
        assert!(q.starts_with("0081;5f000000;Open Download Manager;"), "{q}");
        assert_eq!(q.len(), "0081;5f000000;Open Download Manager;".len() + 36);
    }

    #[test]
    fn batch() {
        let v = expand_batch("http://x/f[01-03].jpg");
        assert_eq!(v, vec!["http://x/f01.jpg", "http://x/f02.jpg", "http://x/f03.jpg"]);
        let v = expand_batch("http://x/[a-b]/[1-2]");
        assert_eq!(v.len(), 4);
        assert_eq!(v[3], "http://x/b/2");
        assert_eq!(expand_batch("http://x/[keep]"), vec!["http://x/[keep]"]);
    }

    #[test]
    fn exts() {
        assert_eq!(extension("a.tar.gz"), "gz");
        assert_eq!(badge_ext("movie.webm"), "WEBM");
        assert_eq!(badge_ext("README"), "FILE");
    }
}
