//! HLS playlists (RFC 8216): master playlists (variants, `#EXT-X-MEDIA`
//! renditions) and media playlists (segments, `#EXT-X-BYTERANGE`,
//! `#EXT-X-DISCONTINUITY`, `#EXT-X-KEY`, `#EXT-X-MAP`).
//!
//! URIs are resolved against the playlist's own URL.

#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    Master(Master),
    Media(Media),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Master {
    pub variants: Vec<Variant>,
    pub renditions: Vec<Rendition>,
    /// `#EXT-X-SESSION-KEY`s (announce the keys the media playlists use)
    pub session_keys: Vec<Key>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: Option<u64>,
    pub average_bandwidth: Option<u64>,
    pub resolution: Option<(u32, u32)>,
    pub codecs: Option<String>,
    pub frame_rate: Option<f64>,
    /// GROUP-ID of the audio renditions
    pub audio: Option<String>,
    /// from `#EXT-X-I-FRAME-STREAM-INF` (trick play only, never downloaded)
    pub iframe_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendition {
    /// AUDIO, VIDEO, SUBTITLES, CLOSED-CAPTIONS
    pub kind: String,
    pub group: String,
    pub name: String,
    pub language: Option<String>,
    pub default: bool,
    pub autoselect: bool,
    /// None: the rendition is part of the variant's own stream
    pub uri: Option<String>,
    pub channels: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Key {
    /// NONE, AES-128, SAMPLE-AES, SAMPLE-AES-CTR, ...
    pub method: String,
    pub uri: Option<String>,
    pub iv: Option<[u8; 16]>,
    /// "identity" when absent
    pub keyformat: Option<String>,
}

impl Key {
    /// A DRM key (FairPlay, Widevine, PlayReady...) or sample encryption:
    /// nothing the app can decrypt.
    pub fn is_drm(&self) -> bool {
        let m = self.method.to_ascii_uppercase();
        if m == "NONE" {
            return false;
        }
        let identity = self.keyformat.as_deref().is_none_or(|f| f.eq_ignore_ascii_case("identity"));
        m != "AES-128" || !identity
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    pub uri: String,
    /// (offset, length)
    pub range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub uri: String,
    pub duration: f64,
    /// (offset, length)
    pub range: Option<(u64, u64)>,
    /// a discontinuity comes before this segment
    pub discontinuity: bool,
    /// AES-128 key in effect (None or method NONE: not encrypted)
    pub key: Option<Key>,
    pub map: Option<Map>,
    /// media sequence number (the default IV)
    pub seq: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Media {
    pub target_duration: Option<f64>,
    pub media_sequence: u64,
    pub segments: Vec<Segment>,
    /// `#EXT-X-ENDLIST`: the playlist is complete (not live)
    pub ended: bool,
    /// VOD or EVENT
    pub playlist_type: Option<String>,
}

impl Media {
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(|s| s.duration).sum()
    }

    pub fn is_live(&self) -> bool {
        !self.ended && !self.playlist_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("VOD"))
    }

    /// Any segment uses a DRM system.
    pub fn drm(&self) -> bool {
        self.segments.iter().any(|s| s.key.as_ref().is_some_and(Key::is_drm))
    }
}

#[derive(Debug, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid HLS playlist: {}", self.0)
    }
}

/// Attribute list (`KEY=value,KEY="quoted, value"`) → pairs (keys upper-case).
pub fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b',' || b[i] == b' ') {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && b[i] != b',' {
            i += 1;
        }
        let key = s[ks..i].trim().to_ascii_uppercase();
        if i >= b.len() || b[i] == b',' {
            if !key.is_empty() {
                out.push((key, String::new()));
            }
            continue;
        }
        i += 1; // '='
        let val = if i < b.len() && b[i] == b'"' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            let v = s[vs..i].to_string();
            i += 1;
            while i < b.len() && b[i] != b',' {
                i += 1;
            }
            v
        } else {
            let vs = i;
            while i < b.len() && b[i] != b',' {
                i += 1;
            }
            s[vs..i].trim().to_string()
        };
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

fn attr<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str())
}

fn resolve(base: &url::Url, uri: &str) -> String {
    base.join(uri.trim()).map(|u| u.to_string()).unwrap_or_else(|_| uri.trim().to_string())
}

/// "n[@o]" → (length, offset)
fn byterange(s: &str) -> Option<(u64, Option<u64>)> {
    let (n, o) = match s.trim().split_once('@') {
        Some((n, o)) => (n, Some(o.trim().parse().ok()?)),
        None => (s.trim(), None),
    };
    Some((n.trim().parse().ok()?, o))
}

fn parse_iv(s: &str) -> Option<[u8; 16]> {
    let h = s.trim().strip_prefix("0x").or_else(|| s.trim().strip_prefix("0X"))?;
    if h.is_empty() || h.len() > 32 {
        return None;
    }
    let h = format!("{h:0>32}");
    let mut iv = [0u8; 16];
    for (i, b) in iv.iter_mut().enumerate() {
        *b = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(iv)
}

fn key_from(a: &[(String, String)], base: &url::Url) -> Key {
    Key {
        method: attr(a, "METHOD").unwrap_or("NONE").to_string(),
        uri: attr(a, "URI").map(|u| resolve(base, u)),
        iv: attr(a, "IV").and_then(parse_iv),
        keyformat: attr(a, "KEYFORMAT").map(String::from),
    }
}

/// Does this text look like an HLS playlist?
pub fn looks_like(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').trim_start().starts_with("#EXTM3U")
}

pub fn parse(text: &str, url: &str) -> Result<Playlist, ParseError> {
    let base = url::Url::parse(url).map_err(|_| ParseError("bad playlist URL".into()))?;
    if !looks_like(text) {
        return Err(ParseError("missing #EXTM3U".into()));
    }
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let master = lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF"));
    if master {
        let mut m = Master::default();
        let mut pending: Option<Variant> = None;
        for l in &lines {
            if let Some(rest) = l.strip_prefix("#EXT-X-STREAM-INF:") {
                let a = attributes(rest);
                pending = Some(variant_from(&a));
            } else if let Some(rest) = l.strip_prefix("#EXT-X-I-FRAME-STREAM-INF:") {
                let a = attributes(rest);
                if let Some(u) = attr(&a, "URI") {
                    let mut v = variant_from(&a);
                    v.uri = resolve(&base, u);
                    v.iframe_only = true;
                    m.variants.push(v);
                }
            } else if let Some(rest) = l.strip_prefix("#EXT-X-MEDIA:") {
                let a = attributes(rest);
                m.renditions.push(Rendition {
                    kind: attr(&a, "TYPE").unwrap_or("").to_ascii_uppercase(),
                    group: attr(&a, "GROUP-ID").unwrap_or("").to_string(),
                    name: attr(&a, "NAME").unwrap_or("").to_string(),
                    language: attr(&a, "LANGUAGE").map(String::from),
                    default: attr(&a, "DEFAULT").is_some_and(|v| v.eq_ignore_ascii_case("YES")),
                    autoselect: attr(&a, "AUTOSELECT").is_some_and(|v| v.eq_ignore_ascii_case("YES")),
                    uri: attr(&a, "URI").map(|u| resolve(&base, u)),
                    channels: attr(&a, "CHANNELS").map(String::from),
                });
            } else if let Some(rest) = l.strip_prefix("#EXT-X-SESSION-KEY:") {
                m.session_keys.push(key_from(&attributes(rest), &base));
            } else if !l.starts_with('#') {
                if let Some(mut v) = pending.take() {
                    v.uri = resolve(&base, l);
                    m.variants.push(v);
                }
            }
        }
        if m.variants.iter().all(|v| v.iframe_only) {
            return Err(ParseError("no variants".into()));
        }
        return Ok(Playlist::Master(m));
    }

    let mut p = Media::default();
    let mut seq: u64 = 0;
    let mut seq_set = false;
    let mut duration: Option<f64> = None;
    let mut range: Option<(u64, Option<u64>)> = None;
    let mut disc = false;
    let mut key: Option<Key> = None;
    let mut map: Option<Map> = None;
    // where the next byte range of a URI starts when no offset is given
    let mut next_offset: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for l in &lines {
        if let Some(v) = l.strip_prefix("#EXT-X-TARGETDURATION:") {
            p.target_duration = v.trim().parse().ok();
        } else if let Some(v) = l.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            if !seq_set {
                seq = v.trim().parse().unwrap_or(0);
                p.media_sequence = seq;
                seq_set = true;
            }
        } else if let Some(v) = l.strip_prefix("#EXTINF:") {
            duration = v.split(',').next().and_then(|d| d.trim().parse().ok());
            if duration.is_none() {
                return Err(ParseError(format!("bad #EXTINF: {v}")));
            }
        } else if let Some(v) = l.strip_prefix("#EXT-X-BYTERANGE:") {
            range = byterange(v);
        } else if *l == "#EXT-X-DISCONTINUITY" {
            disc = true;
        } else if let Some(v) = l.strip_prefix("#EXT-X-KEY:") {
            let k = key_from(&attributes(v), &base);
            key = if k.method.eq_ignore_ascii_case("NONE") { None } else { Some(k) };
        } else if let Some(v) = l.strip_prefix("#EXT-X-MAP:") {
            let a = attributes(v);
            if let Some(u) = attr(&a, "URI") {
                let r = attr(&a, "BYTERANGE").and_then(byterange).map(|(n, o)| (o.unwrap_or(0), n));
                map = Some(Map { uri: resolve(&base, u), range: r });
            }
        } else if *l == "#EXT-X-ENDLIST" {
            p.ended = true;
        } else if let Some(v) = l.strip_prefix("#EXT-X-PLAYLIST-TYPE:") {
            p.playlist_type = Some(v.trim().to_string());
        } else if !l.starts_with('#') {
            let uri = resolve(&base, l);
            let r = range.take().map(|(n, o)| {
                let off = o.unwrap_or_else(|| next_offset.get(&uri).copied().unwrap_or(0));
                next_offset.insert(uri.clone(), off + n);
                (off, n)
            });
            p.segments.push(Segment {
                uri,
                duration: duration.take().unwrap_or(0.0),
                range: r,
                discontinuity: std::mem::take(&mut disc),
                key: key.clone(),
                map: map.clone(),
                seq,
            });
            seq += 1;
        }
    }
    if p.segments.is_empty() && p.ended {
        return Err(ParseError("no segments".into()));
    }
    Ok(Playlist::Media(p))
}

fn variant_from(a: &[(String, String)]) -> Variant {
    Variant {
        uri: String::new(),
        bandwidth: attr(a, "BANDWIDTH").and_then(|v| v.parse().ok()),
        average_bandwidth: attr(a, "AVERAGE-BANDWIDTH").and_then(|v| v.parse().ok()),
        resolution: attr(a, "RESOLUTION").and_then(|r| {
            let (w, h) = r.split_once(['x', 'X'])?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        }),
        codecs: attr(a, "CODECS").map(String::from),
        frame_rate: attr(a, "FRAME-RATE").and_then(|v| v.parse().ok()),
        audio: attr(a, "AUDIO").map(String::from),
        iframe_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_lists() {
        let a = attributes(r#"BANDWIDTH=1280000,CODECS="avc1.4d401f,mp4a.40.2",RESOLUTION=640x360,AUDIO="aud""#);
        assert_eq!(attr(&a, "CODECS"), Some("avc1.4d401f,mp4a.40.2"));
        assert_eq!(attr(&a, "RESOLUTION"), Some("640x360"));
        assert_eq!(attr(&a, "AUDIO"), Some("aud"));
        assert_eq!(parse_iv("0x1"), Some({
            let mut v = [0u8; 16];
            v[15] = 1;
            v
        }));
        assert_eq!(byterange("100@20"), Some((100, Some(20))));
    }

    #[test]
    fn drm_keys() {
        let k = |m: &str, f: Option<&str>| Key { method: m.into(), uri: None, iv: None, keyformat: f.map(String::from) };
        assert!(!k("AES-128", None).is_drm());
        assert!(!k("AES-128", Some("identity")).is_drm());
        assert!(k("SAMPLE-AES", Some("com.apple.streamingkeydelivery")).is_drm());
        assert!(k("AES-128", Some("urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed")).is_drm());
        assert!(!k("NONE", None).is_drm());
    }
}
