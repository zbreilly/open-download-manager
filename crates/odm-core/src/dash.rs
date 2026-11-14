//! DASH manifests (MPD, ISO/IEC 23009-1): periods, adaptation sets and
//! representations, with segments from `SegmentTemplate` (numbered or
//! with a `SegmentTimeline`), `SegmentList` or `SegmentBase` / a plain
//! `BaseURL`. Every segment URL is resolved here, so a representation is
//! simply its list of segments.

use roxmltree::Node;

#[derive(Debug, Clone, PartialEq)]
pub struct Mpd {
    /// `type="dynamic"`: a live stream
    pub dynamic: bool,
    pub duration: Option<f64>,
    pub periods: Vec<Period>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    pub id: Option<String>,
    pub duration: f64,
    pub sets: Vec<Set>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Text,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub kind: Kind,
    pub lang: Option<String>,
    pub label: Option<String>,
    /// carries `ContentProtection` (DRM)
    pub protected: bool,
    pub reps: Vec<Rep>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rep {
    pub id: String,
    pub bandwidth: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codecs: Option<String>,
    pub mime: Option<String>,
    pub channels: Option<String>,
    pub protected: bool,
    pub init: Option<Seg>,
    pub segs: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub url: String,
    /// (offset, length)
    pub range: Option<(u64, u64)>,
    pub duration: f64,
}

#[derive(Debug, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid DASH manifest: {}", self.0)
    }
}

/// Does this text look like a DASH manifest?
pub fn looks_like(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<?xml") || t.starts_with("<MPD") || t.starts_with("<!--")) && t.contains("<MPD")
}

/// ISO 8601 duration ("PT1H2M3.5S", "P1DT2H") in seconds.
pub fn duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let rest = s.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total = 0.0;
    let mut num = String::new();
    for c in date.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'Y' => total += num.parse::<f64>().ok()? * 365.0 * 86400.0,
            'M' => total += num.parse::<f64>().ok()? * 30.0 * 86400.0,
            'W' => total += num.parse::<f64>().ok()? * 7.0 * 86400.0,
            'D' => total += num.parse::<f64>().ok()? * 86400.0,
            _ => return None,
        }
        if c.is_ascii_alphabetic() {
            num.clear();
        }
    }
    for c in time.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' => total += num.parse::<f64>().ok()? * 3600.0,
            'M' => total += num.parse::<f64>().ok()? * 60.0,
            'S' => total += num.parse::<f64>().ok()?,
            _ => return None,
        }
        if c.is_ascii_alphabetic() {
            num.clear();
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(total)
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i: 'a>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn num<T: std::str::FromStr>(n: Node, a: &str) -> Option<T> {
    n.attribute(a).and_then(|v| v.trim().parse().ok())
}

/// Join the `BaseURL` of `n` (if any) onto `base`.
fn base_of(n: Node, base: &url::Url) -> url::Url {
    match child(n, "BaseURL").and_then(|b| b.text()).map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => base.join(t).unwrap_or_else(|_| base.clone()),
        None => base.clone(),
    }
}

/// "start-end" (inclusive) → (offset, length)
fn range(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.trim().split_once('-')?;
    let (a, b): (u64, u64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (b >= a).then(|| (a, b - a + 1))
}

/// The attributes and children of `SegmentTemplate`s, from the outermost
/// level in; inner levels override outer ones.
#[derive(Default, Clone)]
struct Template {
    media: Option<String>,
    init: Option<String>,
    start_number: Option<u64>,
    timescale: Option<u64>,
    duration: Option<u64>,
    pto: Option<u64>,
    /// (t, d, r)
    timeline: Option<Vec<(Option<u64>, u64, i64)>>,
}

impl Template {
    fn merge(&self, n: Option<Node>) -> Template {
        let Some(n) = n else { return self.clone() };
        let mut t = self.clone();
        if let Some(v) = n.attribute("media") {
            t.media = Some(v.into());
        }
        if let Some(v) = n.attribute("initialization") {
            t.init = Some(v.into());
        }
        t.start_number = num(n, "startNumber").or(t.start_number);
        t.timescale = num(n, "timescale").or(t.timescale);
        t.duration = num(n, "duration").or(t.duration);
        t.pto = num(n, "presentationTimeOffset").or(t.pto);
        if let Some(tl) = child(n, "SegmentTimeline") {
            t.timeline = Some(
                children(tl, "S")
                    .map(|s| (num(s, "t"), num(s, "d").unwrap_or(0), num(s, "r").unwrap_or(0)))
                    .collect(),
            );
        }
        t
    }
}

/// Fill in `$RepresentationID$`, `$Number$`, `$Time$`, `$Bandwidth$`
/// (with optional `%0Nd` widths) and `$$`.
pub fn fill(template: &str, id: &str, number: u64, time: u64, bandwidth: u64) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(a) = rest.find('$') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        let Some(b) = after.find('$') else {
            out.push_str(&rest[a..]);
            return out;
        };
        let tok = &after[..b];
        let (name, fmt) = match tok.split_once('%') {
            Some((n, f)) => (n, Some(f)),
            None => (tok, None),
        };
        let width = fmt.and_then(|f| f.trim_end_matches(['d', 'u']).trim_start_matches('0').parse::<usize>().ok()).unwrap_or(0);
        let num = |v: u64| format!("{v:0width$}");
        match name {
            "" => out.push('$'),
            "RepresentationID" => out.push_str(id),
            "Number" => out.push_str(&num(number)),
            "Time" => out.push_str(&num(time)),
            "Bandwidth" => out.push_str(&num(bandwidth)),
            _ => {
                out.push('$');
                out.push_str(tok);
                out.push('$');
            }
        }
        rest = &after[b + 1..];
    }
    out.push_str(rest);
    out
}

pub fn parse(text: &str, url: &str) -> Result<Mpd, ParseError> {
    let base = url::Url::parse(url).map_err(|_| ParseError("bad manifest URL".into()))?;
    let doc = roxmltree::Document::parse(text).map_err(|e| ParseError(e.to_string()))?;
    let mpd = doc.root_element();
    if mpd.tag_name().name() != "MPD" {
        return Err(ParseError("no <MPD> element".into()));
    }
    let dynamic = mpd.attribute("type").is_some_and(|t| t == "dynamic");
    let total = mpd.attribute("mediaPresentationDuration").and_then(duration);
    let mpd_base = base_of(mpd, &base);
    let pnodes: Vec<Node> = children(mpd, "Period").collect();
    let starts: Vec<Option<f64>> = pnodes.iter().map(|p| p.attribute("start").and_then(duration)).collect();
    let mut periods = vec![];
    let mut at = 0.0;
    for (i, p) in pnodes.iter().enumerate() {
        let start = starts[i].unwrap_or(at);
        let dur = p
            .attribute("duration")
            .and_then(duration)
            .or_else(|| starts.get(i + 1).copied().flatten().map(|n| n - start))
            .or_else(|| total.map(|t| t - start))
            .unwrap_or(0.0)
            .max(0.0);
        at = start + dur;
        let pbase = base_of(*p, &mpd_base);
        let ptpl = Template::default().merge(child(*p, "SegmentTemplate"));
        let mut sets = vec![];
        for a in children(*p, "AdaptationSet") {
            sets.push(adaptation(a, &pbase, &ptpl, *p, dur)?);
        }
        periods.push(Period { id: p.attribute("id").map(String::from), duration: dur, sets });
    }
    if periods.is_empty() {
        return Err(ParseError("no periods".into()));
    }
    Ok(Mpd { dynamic, duration: total.or_else(|| Some(at).filter(|d| *d > 0.0)), periods })
}

fn kind_of(content: Option<&str>, mime: Option<&str>, codecs: Option<&str>) -> Kind {
    let s = content.or(mime.and_then(|m| m.split('/').next())).unwrap_or("");
    match s {
        "video" => Kind::Video,
        "audio" => Kind::Audio,
        "text" | "application" if mime.is_some_and(|m| m.contains("ttml") || m.contains("vtt")) => Kind::Text,
        "text" => Kind::Text,
        _ => match codecs.unwrap_or("").split('.').next().unwrap_or("") {
            "avc1" | "avc3" | "hvc1" | "hev1" | "vp09" | "vp8" | "av01" => Kind::Video,
            "mp4a" | "ac-3" | "ec-3" | "opus" | "vorbis" | "flac" => Kind::Audio,
            "stpp" | "wvtt" => Kind::Text,
            _ => Kind::Other,
        },
    }
}

fn adaptation(a: Node, base: &url::Url, ptpl: &Template, period: Node, dur: f64) -> Result<Set, ParseError> {
    let abase = base_of(a, base);
    let atpl = ptpl.merge(child(a, "SegmentTemplate"));
    let protected = child(a, "ContentProtection").is_some();
    let a_mime = a.attribute("mimeType");
    let a_codecs = a.attribute("codecs");
    let mut reps = vec![];
    let mut kind = kind_of(a.attribute("contentType"), a_mime, a_codecs);
    for (i, r) in children(a, "Representation").enumerate() {
        let mime = r.attribute("mimeType").or(a_mime);
        let codecs = r.attribute("codecs").or(a_codecs);
        if kind == Kind::Other {
            kind = kind_of(None, mime, codecs);
        }
        let id = r.attribute("id").map(String::from).unwrap_or_else(|| format!("r{i}"));
        let bandwidth = num(r, "bandwidth").unwrap_or(0);
        let rbase = base_of(r, &abase);
        let rtpl = atpl.merge(child(r, "SegmentTemplate"));
        let list = child(r, "SegmentList").or_else(|| child(a, "SegmentList")).or_else(|| child(period, "SegmentList"));
        let sbase = child(r, "SegmentBase").or_else(|| child(a, "SegmentBase"));
        let (init, segs) = if rtpl.media.is_some() {
            template_segments(&rtpl, &rbase, &id, bandwidth, dur)?
        } else if let Some(l) = list {
            list_segments(l, &rbase)
        } else {
            // SegmentBase or a plain BaseURL: the whole file is the track
            // (its index and initialization are inside it)
            let _ = sbase;
            (None, vec![Seg { url: rbase.to_string(), range: None, duration: dur }])
        };
        let channels = child(r, "AudioChannelConfiguration")
            .or_else(|| child(a, "AudioChannelConfiguration"))
            .and_then(|c| c.attribute("value"))
            .map(String::from);
        reps.push(Rep {
            id,
            bandwidth,
            width: num(r, "width").or(num(a, "width")),
            height: num(r, "height").or(num(a, "height")),
            codecs: codecs.map(String::from),
            mime: mime.map(String::from),
            channels,
            protected: protected || child(r, "ContentProtection").is_some(),
            init,
            segs,
        });
    }
    Ok(Set {
        kind,
        lang: a.attribute("lang").map(String::from),
        label: child(a, "Label").and_then(|l| l.text()).map(|t| t.trim().to_string()),
        protected: protected || reps.iter().any(|r| r.protected),
        reps,
    })
}

/// Most segments one representation may have (a 24 h stream of 1 s
/// segments); more means a broken or hostile manifest.
const MAX_SEGMENTS: usize = 100_000;

fn template_segments(t: &Template, base: &url::Url, id: &str, bw: u64, dur: f64) -> Result<(Option<Seg>, Vec<Seg>), ParseError> {
    let join = |u: String| base.join(&u).map(|x| x.to_string()).unwrap_or(u);
    let media = t.media.clone().unwrap_or_default();
    let scale = t.timescale.unwrap_or(1).max(1);
    let start = t.start_number.unwrap_or(1);
    let init = t.init.as_ref().map(|i| Seg { url: join(fill(i, id, start, 0, bw)), range: None, duration: 0.0 });
    let mut segs = vec![];
    if let Some(tl) = &t.timeline {
        let mut time: u64 = t.pto.unwrap_or(0);
        let mut n = start;
        let end_time = t.pto.unwrap_or(0) + (dur * scale as f64).round() as u64;
        for (i, (st, d, r)) in tl.iter().enumerate() {
            if let Some(st) = st {
                time = *st;
            }
            if *d == 0 {
                continue;
            }
            let repeats: u64 = if *r >= 0 {
                *r as u64
            } else {
                // until the next S (or the end of the period)
                let until = tl.get(i + 1).and_then(|x| x.0).unwrap_or(end_time);
                until.saturating_sub(time).div_ceil(*d).saturating_sub(1)
            };
            for _ in 0..=repeats {
                if segs.len() >= MAX_SEGMENTS {
                    return Err(ParseError("too many segments".into()));
                }
                segs.push(Seg { url: join(fill(&media, id, n, time, bw)), range: None, duration: *d as f64 / scale as f64 });
                time += d;
                n += 1;
            }
        }
    } else {
        let d = t.duration.filter(|d| *d > 0).ok_or_else(|| ParseError("SegmentTemplate without duration".into()))?;
        let seg_secs = d as f64 / scale as f64;
        let count = (dur / seg_secs - 1e-6).ceil().max(1.0) as u64;
        if count as usize > MAX_SEGMENTS {
            return Err(ParseError("too many segments".into()));
        }
        for i in 0..count {
            let last = i + 1 == count;
            let len = if last { (dur - seg_secs * i as f64).max(0.0) } else { seg_secs };
            segs.push(Seg { url: join(fill(&media, id, start + i, i * d, bw)), range: None, duration: len });
        }
    }
    Ok((init, segs))
}

fn list_segments(l: Node, base: &url::Url) -> (Option<Seg>, Vec<Seg>) {
    let scale = num::<u64>(l, "timescale").unwrap_or(1).max(1);
    let d = num::<u64>(l, "duration").map(|d| d as f64 / scale as f64).unwrap_or(0.0);
    let join = |u: Option<&str>| match u {
        Some(u) => base.join(u).map(|x| x.to_string()).unwrap_or_else(|_| u.to_string()),
        None => base.to_string(),
    };
    let init = child(l, "Initialization").map(|i| Seg {
        url: join(i.attribute("sourceURL")),
        range: i.attribute("range").and_then(range),
        duration: 0.0,
    });
    let segs = children(l, "SegmentURL")
        .map(|s| Seg { url: join(s.attribute("media")), range: s.attribute("mediaRange").and_then(range), duration: d })
        .collect();
    (init, segs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(duration("PT0S"), Some(0.0));
        assert_eq!(duration("P1DT1S"), Some(86401.0));
        assert_eq!(duration("PT10M"), Some(600.0));
        assert_eq!(duration("1H"), None);
    }

    #[test]
    fn templates() {
        assert_eq!(fill("seg-$RepresentationID$-$Number%05d$.m4s", "v1", 7, 0, 0), "seg-v1-00007.m4s");
        assert_eq!(fill("t$Time$-$Bandwidth$$$.mp4", "a", 1, 90000, 128000), "t90000-128000$.mp4");
        assert_eq!(fill("odd$Unknown$", "a", 1, 0, 0), "odd$Unknown$");
        assert_eq!(range("100-199"), Some((100, 100)));
    }
}
