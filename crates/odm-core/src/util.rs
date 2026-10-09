//! File-name, category, batch-pattern and hashing helpers.

use crate::model::Category;
use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

/// File name from a `Content-Disposition` header (RFC 6266 / 5987).
pub fn filename_from_disposition(cd: &str) -> Option<String> {
    // prefer filename*=UTF-8''...
    for part in cd.split(';') {
        let p = part.trim();
        let lower = p.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("filename*=").map(|_| &p[10..]) {
            let v = v.trim().trim_matches('"');
            let enc_val = match v.find("''") {
                Some(i) => &v[i + 2..],
                None => v,
            };
            let s = percent_decode_str(enc_val).decode_utf8_lossy().into_owned();
            if !s.trim().is_empty() {
                return Some(sanitize_filename(&s));
            }
        }
    }
    for part in cd.split(';') {
        let p = part.trim();
        if p.len() > 9 && p[..9].eq_ignore_ascii_case("filename=") {
            let v = p[9..].trim().trim_matches('"');
            let s = percent_decode_str(v).decode_utf8_lossy().into_owned();
            if !s.trim().is_empty() {
                return Some(sanitize_filename(&s));
            }
        }
    }
    None
}

/// Last path segment of a URL, percent-decoded.
pub fn filename_from_url(u: &str) -> Option<String> {
    let parsed = url::Url::parse(u).ok()?;
    let seg = parsed.path_segments()?.filter(|s| !s.is_empty()).next_back()?.to_string();
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

/// Remove characters that are illegal on any of Windows/macOS/Linux.
pub fn sanitize_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() || out == "." || out == ".." {
        out = "download".into();
    }
    // Windows reserved device names
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        out = format!("_{out}");
    }
    if out.len() > 240 {
        let ext = extension(&out);
        let keep = 240 - ext.len().min(20) - 1;
        let mut cut = keep;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out = if ext.is_empty() {
            out[..cut].to_string()
        } else {
            format!("{}.{}", &out[..cut], ext.to_ascii_lowercase())
        };
    }
    out
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
