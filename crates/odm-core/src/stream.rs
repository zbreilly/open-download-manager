//! HLS and DASH stream downloads.
//!
//! A probe of a manifest lists the qualities it offers ([`inspect`]). A
//! download resolves the chosen quality to one or two tracks of pieces
//! ([`resolve`]), fetches the pieces with several connections into a work
//! folder beside the part file ([`download`]) and joins them:
//!
//! * MPEG-TS and fragmented MP4 pieces are concatenated (with the
//!   initialization section first)
//! * a separate fragmented-MP4 audio track is muxed into the video
//!   ([`crate::mp4`])
//! * any other separate audio track is saved beside the video, and the
//!   download says so
//!
//! Finished pieces stay in the work folder, so a paused or interrupted
//! download continues where it stopped. HLS AES-128 encryption is
//! decrypted; DRM-protected streams (Widevine, PlayReady, FairPlay,
//! SAMPLE-AES) and live streams are refused with a clear message.

use crate::client::{self, HeaderSet};
use crate::limiter::Limiter;
use crate::model::{StreamAudio, StreamChoice, StreamInfo, StreamVariant};
use crate::task::{Runtime, TaskError, Units};
use crate::{dash, hls, util, CoreError};
use futures_util::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub const PROTECTED: &str =
    "This stream is protected with DRM (copy protection), so Open Download Manager can't download it.";
pub const LIVE: &str = "This is a live stream. Only complete (on-demand) streams can be downloaded.";

/// Largest manifest accepted.
const MAX_MANIFEST: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hls,
    Dash,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Hls => "hls",
            Kind::Dash => "dash",
        }
    }
}

/// Is this link an HLS playlist or DASH manifest, judging by its content
/// type or its name?
pub fn manifest_kind(url: &str, mime: Option<&str>) -> Option<Kind> {
    let m = mime.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "application/vnd.apple.mpegurl" | "application/x-mpegurl" | "audio/mpegurl" | "audio/x-mpegurl" => {
            return Some(Kind::Hls)
        }
        "application/dash+xml" => return Some(Kind::Dash),
        _ => {}
    }
    let path = url::Url::parse(url.trim()).map(|u| u.path().to_ascii_lowercase()).unwrap_or_default();
    if path.ends_with(".m3u8") {
        Some(Kind::Hls)
    } else if path.ends_with(".mpd") {
        Some(Kind::Dash)
    } else {
        None
    }
}

/// The kind of manifest a body is, by its first bytes.
pub fn sniff(text: &str) -> Option<Kind> {
    if hls::looks_like(text) {
        Some(Kind::Hls)
    } else if dash::looks_like(text) {
        Some(Kind::Dash)
    } else {
        None
    }
}

// ---------------------------------------------------------------- plans

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Ts,
    Mp4,
    WebM,
    /// packed audio (ADTS AAC, MP3, AC-3...) with this extension
    Raw(&'static str),
}

impl Container {
    fn ext(self, audio_only: bool) -> &'static str {
        match self {
            Container::Ts => "ts",
            Container::Mp4 if audio_only => "m4a",
            Container::Mp4 => "mp4",
            Container::WebM => "webm",
            Container::Raw(e) => e,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub uri: String,
    pub iv: [u8; 16],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub url: String,
    /// (offset, length)
    pub range: Option<(u64, u64)>,
    /// AES-128-CBC
    pub key: Option<Key>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub container: Container,
    pub audio_only: bool,
    /// initialization section first, if any, then the media segments
    pub pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub kind: Kind,
    /// the main track (video, or the only one), then a separate audio track
    pub tracks: Vec<Track>,
    pub est_bytes: Option<u64>,
    /// extension of the finished file
    pub ext: &'static str,
}

impl Plan {
    /// A fingerprint of the pieces: finished pieces in the work folder
    /// belong to this plan only if it matches.
    fn fingerprint(&self, choice: &StreamChoice) -> String {
        let counts: Vec<String> = self.tracks.iter().map(|t| t.pieces.len().to_string()).collect();
        format!(
            "1|{}|{}|{}|{}",
            self.kind.name(),
            choice.variant.as_deref().unwrap_or(""),
            choice.audio.as_deref().unwrap_or(""),
            counts.join(",")
        )
    }

    /// The two tracks can be joined into one file.
    fn can_mux(&self) -> bool {
        self.tracks.len() == 2 && self.tracks.iter().all(|t| t.container == Container::Mp4)
    }
}

fn mbps(bw: u64) -> String {
    let m = bw as f64 / 1_000_000.0;
    if m >= 10.0 {
        format!("{m:.0} Mbps")
    } else if m >= 1.0 {
        format!("{m:.1} Mbps")
    } else {
        format!("{:.0} kbps", bw as f64 / 1000.0)
    }
}

fn video_label(w: Option<u32>, h: Option<u32>, bw: Option<u64>, codecs: Option<&str>, dup: bool) -> String {
    let mut parts = vec![];
    match (w, h) {
        (_, Some(h)) => parts.push(format!("{h}p")),
        (Some(w), None) => parts.push(format!("{w}w")),
        _ => {}
    }
    if let Some(b) = bw.filter(|b| *b > 0) {
        parts.push(mbps(b));
    }
    if dup {
        if let Some(c) = codecs {
            parts.push(c.split(',').next().unwrap_or(c).split('.').next().unwrap_or(c).to_string());
        }
    }
    if parts.is_empty() {
        "Default".into()
    } else {
        parts.join(" · ")
    }
}

fn audio_label(name: Option<&str>, lang: Option<&str>, bw: Option<u64>, channels: Option<&str>) -> String {
    let mut parts = vec![];
    if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
        parts.push(n.trim().to_string());
    } else if let Some(l) = lang {
        parts.push(l.to_string());
    }
    if let Some(c) = channels.filter(|c| !c.is_empty()) {
        parts.push(format!("{c} ch"));
    }
    if let Some(b) = bw.filter(|b| *b > 0) {
        parts.push(mbps(b));
    }
    if parts.is_empty() {
        "Audio".into()
    } else {
        parts.join(" · ")
    }
}

fn est(bw: Option<u64>, secs: Option<f64>) -> Option<u64> {
    match (bw, secs) {
        (Some(b), Some(s)) if b > 0 && s > 0.0 => Some((b as f64 * s / 8.0) as u64),
        _ => None,
    }
}

// ---------------------------------------------------------------- HLS

fn hls_container(m: &hls::Media) -> Container {
    if m.segments.iter().any(|s| s.map.is_some()) {
        return Container::Mp4;
    }
    let ext = m.segments.first().map(|s| util::extension(&util::filename_from_url(&s.uri).unwrap_or_default())).unwrap_or_default();
    match ext.as_str() {
        "aac" => Container::Raw("aac"),
        "mp3" => Container::Raw("mp3"),
        "ac3" => Container::Raw("ac3"),
        "ec3" => Container::Raw("ec3"),
        "mp4" | "m4s" | "m4a" | "m4v" | "cmfv" | "cmfa" => Container::Mp4,
        _ => Container::Ts,
    }
}

/// Variant ids, unique within the playlist: "<bandwidth>-<height>[-n]".
fn hls_variants(m: &hls::Master) -> Vec<(String, &hls::Variant)> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut v: Vec<&hls::Variant> = m.variants.iter().filter(|v| !v.iframe_only).collect();
    v.sort_by_key(|x| std::cmp::Reverse((x.resolution.map(|r| r.1).unwrap_or(0), x.bandwidth.unwrap_or(0))));
    v.into_iter()
        .map(|x| {
            let base = format!("{}-{}", x.bandwidth.unwrap_or(0), x.resolution.map(|r| r.1).unwrap_or(0));
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let id = if *n == 1 { base } else { format!("{base}-{n}") };
            (id, x)
        })
        .collect()
}

fn audio_id(r: &hls::Rendition) -> String {
    format!("{}|{}", r.name, r.language.as_deref().unwrap_or(""))
}

/// The audio renditions with their own playlist for a group.
fn hls_audio<'a>(m: &'a hls::Master, group: Option<&str>) -> Vec<&'a hls::Rendition> {
    let Some(g) = group else { return vec![] };
    let mut seen = std::collections::HashSet::new();
    m.renditions
        .iter()
        .filter(|r| r.kind == "AUDIO" && r.group == g && r.uri.is_some())
        .filter(|r| seen.insert(audio_id(r)))
        .collect()
}

fn pick_variant<'a>(list: &'a [(String, &'a hls::Variant)], want: Option<&str>) -> Option<&'a (String, &'a hls::Variant)> {
    if let Some(w) = want {
        if let Some(v) = list.iter().find(|(id, _)| id == w) {
            return Some(v);
        }
        // same bandwidth and height under another id, else the nearest bandwidth
        let (bw, h) = w.split_once('-').map(|(b, r)| (b.parse::<u64>().unwrap_or(0), r.split('-').next().unwrap_or("0").parse::<u32>().unwrap_or(0))).unwrap_or((0, 0));
        return list.iter().min_by_key(|(_, v)| {
            let dh = v.resolution.map(|r| r.1).unwrap_or(0).abs_diff(h) as u64;
            (dh, v.bandwidth.unwrap_or(0).abs_diff(bw))
        });
    }
    list.first()
}

fn pick_audio<'a>(list: &[&'a hls::Rendition], want: Option<&str>) -> Option<&'a hls::Rendition> {
    if let Some(w) = want {
        if let Some(r) = list.iter().find(|r| audio_id(r) == w) {
            return Some(r);
        }
        let lang = w.split_once('|').map(|x| x.1).unwrap_or("");
        if let Some(r) = list.iter().find(|r| !lang.is_empty() && r.language.as_deref() == Some(lang)) {
            return Some(r);
        }
    }
    list.iter().find(|r| r.default).or(list.first()).copied()
}

fn hls_track(m: &hls::Media, audio_only: bool) -> Result<Track, CoreError> {
    if m.drm() {
        return Err(CoreError::Other(PROTECTED.into()));
    }
    let container = hls_container(m);
    let mut pieces = vec![];
    let mut last_map: Option<&hls::Map> = None;
    let mut maps = 0;
    for s in &m.segments {
        let key = s.key.as_ref().filter(|k| k.method.eq_ignore_ascii_case("AES-128")).map(|k| {
            Ok::<_, CoreError>(Key {
                uri: k.uri.clone().ok_or_else(|| CoreError::Other("an encrypted segment has no key address".into()))?,
                iv: k.iv.unwrap_or_else(|| (s.seq as u128).to_be_bytes()),
            })
        });
        let key = key.transpose()?;
        if let Some(map) = &s.map {
            if last_map != Some(map) {
                // only the first initialization section is used: a second,
                // different one (after a discontinuity) can't go into the
                // same file
                if maps == 0 {
                    pieces.push(Piece { url: map.uri.clone(), range: map.range, key: key.clone().filter(|_| s.key.as_ref().is_some_and(|k| k.iv.is_some())) });
                }
                maps += 1;
                last_map = Some(map);
            }
        }
        pieces.push(Piece { url: s.uri.clone(), range: s.range, key });
    }
    if pieces.is_empty() {
        return Err(CoreError::Other("the playlist has no segments".into()));
    }
    Ok(Track { container, audio_only, pieces })
}

fn dash_track(reps: &[&dash::Rep], audio_only: bool) -> Track {
    let mime = reps.first().and_then(|r| r.mime.clone()).unwrap_or_default();
    let container = if mime.contains("webm") {
        Container::WebM
    } else if mime.contains("mp2t") {
        Container::Ts
    } else {
        Container::Mp4
    };
    let mut pieces = vec![];
    for (i, r) in reps.iter().enumerate() {
        // the initialization of the first period; later periods are
        // expected to share it
        if i == 0 {
            if let Some(init) = &r.init {
                pieces.push(Piece { url: init.url.clone(), range: init.range, key: None });
            }
        }
        pieces.extend(r.segs.iter().map(|s| Piece { url: s.url.clone(), range: s.range, key: None }));
    }
    Track { container, audio_only, pieces }
}

// ---------------------------------------------------------------- fetching

/// GET a manifest. Returns (final URL, text).
pub async fn fetch_text(client: &reqwest::Client, url: &str, hs: &HeaderSet) -> Result<(String, String), CoreError> {
    let resp = client::send(client, url, hs, &[], Some(Duration::from_secs(60))).await?;
    let st = resp.status().as_u16();
    if !(200..300).contains(&st) {
        return Err(CoreError::Http(st));
    }
    let final_url = resp.url().to_string();
    let mut body = Vec::new();
    let mut s = resp.bytes_stream();
    while let Some(c) = s.next().await {
        let c = c.map_err(CoreError::from_reqwest)?;
        if body.len() + c.len() > MAX_MANIFEST {
            return Err(CoreError::Other("the manifest is too large".into()));
        }
        body.extend_from_slice(&c);
    }
    Ok((final_url, String::from_utf8_lossy(&body).into_owned()))
}

/// What a manifest offers, for the Add dialog. `text` is the manifest
/// fetched from `url`.
pub async fn inspect(client: &reqwest::Client, hs: &HeaderSet, url: &str, text: &str) -> Result<(Kind, StreamInfo), CoreError> {
    let kind = sniff(text).ok_or_else(|| CoreError::Other("not an HLS or DASH manifest".into()))?;
    let mut info = StreamInfo { kind: kind.name().into(), ..Default::default() };
    match kind {
        Kind::Hls => match hls::parse(text, url).map_err(|e| CoreError::Other(e.to_string()))? {
            hls::Playlist::Media(m) => {
                info.live = m.is_live();
                info.protected = m.drm();
                info.duration_secs = Some(m.duration());
                info.variants.push(StreamVariant {
                    id: String::new(),
                    label: "Default".into(),
                    ext: hls_container(&m).ext(false).into(),
                    ..Default::default()
                });
            }
            hls::Playlist::Master(m) => {
                info.protected = m.session_keys.iter().any(hls::Key::is_drm);
                let list = hls_variants(&m);
                let best = list.first().map(|x| x.1).ok_or_else(|| CoreError::Other("the playlist lists no qualities".into()))?;
                // the best quality's playlist tells the length, the format
                // and whether it is protected or live
                let (murl, mtext) = fetch_text(client, &best.uri, hs).await?;
                let mut ext = "ts";
                if let Ok(hls::Playlist::Media(mm)) = hls::parse(&mtext, &murl) {
                    info.live = mm.is_live();
                    info.protected |= mm.drm();
                    info.duration_secs = Some(mm.duration());
                    ext = hls_container(&mm).ext(false);
                }
                let audio = hls_audio(&m, best.audio.as_deref());
                let dup = |v: &hls::Variant| list.iter().filter(|(_, o)| o.resolution == v.resolution && o.bandwidth == v.bandwidth).count() > 1;
                for (id, v) in &list {
                    let sep = !hls_audio(&m, v.audio.as_deref()).is_empty();
                    info.variants.push(StreamVariant {
                        id: id.clone(),
                        label: video_label(v.resolution.map(|r| r.0), v.resolution.map(|r| r.1), v.bandwidth, v.codecs.as_deref(), dup(v)),
                        width: v.resolution.map(|r| r.0),
                        height: v.resolution.map(|r| r.1),
                        bandwidth: v.bandwidth,
                        codecs: v.codecs.clone(),
                        est_bytes: est(v.average_bandwidth.or(v.bandwidth), info.duration_secs),
                        separate_audio: sep,
                        ext: ext.into(),
                    });
                }
                info.audio = audio
                    .iter()
                    .map(|r| StreamAudio {
                        id: audio_id(r),
                        label: audio_label(Some(&r.name), r.language.as_deref(), None, r.channels.as_deref()),
                        language: r.language.clone(),
                        bandwidth: None,
                        default: r.default,
                    })
                    .collect();
            }
        },
        Kind::Dash => {
            let mpd = dash::parse(text, url).map_err(|e| CoreError::Other(e.to_string()))?;
            info.live = mpd.dynamic;
            info.duration_secs = mpd.duration;
            let p = &mpd.periods[0];
            info.protected = mpd.periods.iter().any(|p| p.sets.iter().any(|s| s.protected && matches!(s.kind, dash::Kind::Video | dash::Kind::Audio)));
            let mut videos: Vec<(&dash::Set, &dash::Rep)> =
                p.sets.iter().filter(|s| s.kind == dash::Kind::Video).flat_map(|s| s.reps.iter().map(move |r| (s, r))).collect();
            let mut audios: Vec<(&dash::Set, &dash::Rep)> =
                p.sets.iter().filter(|s| s.kind == dash::Kind::Audio).flat_map(|s| s.reps.iter().map(move |r| (s, r))).collect();
            videos.sort_by_key(|(_, r)| std::cmp::Reverse((r.height.unwrap_or(0), r.bandwidth)));
            audios.sort_by_key(|(_, r)| std::cmp::Reverse(r.bandwidth));
            let default_audio = audios.first().map(|(_, r)| r.bandwidth).unwrap_or(0);
            if videos.is_empty() {
                for (s, r) in &audios {
                    let c = dash_track(&[r], true).container;
                    info.variants.push(StreamVariant {
                        id: r.id.clone(),
                        label: audio_label(s.label.as_deref(), s.lang.as_deref(), Some(r.bandwidth), r.channels.as_deref()),
                        bandwidth: Some(r.bandwidth),
                        codecs: r.codecs.clone(),
                        est_bytes: est(Some(r.bandwidth), info.duration_secs),
                        separate_audio: false,
                        ext: c.ext(true).into(),
                        ..Default::default()
                    });
                }
            } else {
                let dup = |r: &dash::Rep| videos.iter().filter(|(_, o)| o.height == r.height && o.bandwidth == r.bandwidth).count() > 1;
                for (_, r) in &videos {
                    let c = dash_track(&[r], false).container;
                    let ext = c.ext(false);
                    info.variants.push(StreamVariant {
                        id: r.id.clone(),
                        label: video_label(r.width, r.height, Some(r.bandwidth), r.codecs.as_deref(), dup(r)),
                        width: r.width,
                        height: r.height,
                        bandwidth: Some(r.bandwidth),
                        codecs: r.codecs.clone(),
                        est_bytes: est(Some(r.bandwidth + default_audio), info.duration_secs),
                        separate_audio: !audios.is_empty(),
                        ext: ext.into(),
                    });
                }
                info.audio = audios
                    .iter()
                    .enumerate()
                    .map(|(i, (s, r))| StreamAudio {
                        id: r.id.clone(),
                        label: audio_label(s.label.as_deref(), s.lang.as_deref(), Some(r.bandwidth), r.channels.as_deref()),
                        language: s.lang.clone(),
                        bandwidth: Some(r.bandwidth),
                        default: i == 0,
                    })
                    .collect();
            }
            if info.variants.is_empty() {
                return Err(CoreError::Other("the manifest lists no video or audio".into()));
            }
        }
    }
    if info.protected {
        info.blocked = Some(PROTECTED.into());
    } else if info.live {
        info.blocked = Some(LIVE.into());
    }
    Ok((kind, info))
}

/// Fetch the manifest at `url` and work out what to download for `choice`.
pub async fn resolve(client: &reqwest::Client, hs: &HeaderSet, url: &str, choice: &StreamChoice) -> Result<Plan, CoreError> {
    let (url, text) = fetch_text(client, url, hs).await?;
    let kind = sniff(&text).ok_or_else(|| CoreError::Other("the link no longer returns an HLS or DASH manifest".into()))?;
    let mut tracks = vec![];
    let mut est_bytes = None;
    match kind {
        Kind::Hls => {
            let media = match hls::parse(&text, &url).map_err(|e| CoreError::Other(e.to_string()))? {
                hls::Playlist::Media(m) => m,
                hls::Playlist::Master(m) => {
                    if m.session_keys.iter().any(hls::Key::is_drm) {
                        return Err(CoreError::Other(PROTECTED.into()));
                    }
                    let list = hls_variants(&m);
                    let (_, v) = pick_variant(&list, choice.variant.as_deref()).ok_or_else(|| CoreError::Other("the playlist lists no qualities".into()))?;
                    let audio = hls_audio(&m, v.audio.as_deref());
                    let (murl, mtext) = fetch_text(client, &v.uri, hs).await?;
                    let hls::Playlist::Media(mm) = hls::parse(&mtext, &murl).map_err(|e| CoreError::Other(e.to_string()))? else {
                        return Err(CoreError::Other("a quality's playlist is not a media playlist".into()));
                    };
                    if mm.is_live() {
                        return Err(CoreError::Other(LIVE.into()));
                    }
                    est_bytes = est(v.average_bandwidth.or(v.bandwidth), Some(mm.duration()));
                    let main = hls_track(&mm, false)?;
                    tracks.push(main);
                    if let Some(r) = pick_audio(&audio, choice.audio.as_deref()) {
                        let au = r.uri.clone().unwrap_or_default();
                        let (aurl, atext) = fetch_text(client, &au, hs).await?;
                        if let hls::Playlist::Media(am) = hls::parse(&atext, &aurl).map_err(|e| CoreError::Other(e.to_string()))? {
                            tracks.push(hls_track(&am, true)?);
                        }
                    }
                    mm
                }
            };
            if tracks.is_empty() {
                if media.is_live() {
                    return Err(CoreError::Other(LIVE.into()));
                }
                tracks.push(hls_track(&media, false)?);
            }
        }
        Kind::Dash => {
            let mpd = dash::parse(&text, &url).map_err(|e| CoreError::Other(e.to_string()))?;
            if mpd.dynamic {
                return Err(CoreError::Other(LIVE.into()));
            }
            // per period: the representation with the chosen id, else the
            // one nearest in bandwidth to the first period's pick
            let pick = |kind: dash::Kind, want: Option<&str>| -> Result<Vec<&dash::Rep>, CoreError> {
                let mut out: Vec<&dash::Rep> = vec![];
                let mut target_bw: Option<u64> = None;
                for p in &mpd.periods {
                    let sets: Vec<&dash::Set> = p.sets.iter().filter(|s| s.kind == kind).collect();
                    if sets.iter().any(|s| s.protected) {
                        return Err(CoreError::Other(PROTECTED.into()));
                    }
                    let reps: Vec<&dash::Rep> = sets.iter().flat_map(|s| s.reps.iter()).collect();
                    if reps.is_empty() {
                        continue;
                    }
                    let r = want
                        .and_then(|w| reps.iter().find(|r| r.id == w).copied())
                        .or_else(|| target_bw.and_then(|bw| reps.iter().min_by_key(|r| r.bandwidth.abs_diff(bw)).copied()))
                        .or_else(|| {
                            if kind == dash::Kind::Video {
                                reps.iter().max_by_key(|r| (r.height.unwrap_or(0), r.bandwidth)).copied()
                            } else {
                                reps.iter().max_by_key(|r| r.bandwidth).copied()
                            }
                        });
                    if let Some(r) = r {
                        target_bw.get_or_insert(r.bandwidth);
                        out.push(r);
                    }
                }
                Ok(out)
            };
            let has_video = mpd.periods.iter().any(|p| p.sets.iter().any(|s| s.kind == dash::Kind::Video));
            let (main, audio) = if has_video {
                (pick(dash::Kind::Video, choice.variant.as_deref())?, pick(dash::Kind::Audio, choice.audio.as_deref())?)
            } else {
                (pick(dash::Kind::Audio, choice.variant.as_deref().or(choice.audio.as_deref()))?, vec![])
            };
            if main.is_empty() {
                return Err(CoreError::Other("the manifest lists no video or audio".into()));
            }
            let bw = main[0].bandwidth + audio.first().map(|r| r.bandwidth).unwrap_or(0);
            est_bytes = est(Some(bw), mpd.duration);
            tracks.push(dash_track(&main, !has_video));
            if !audio.is_empty() {
                tracks.push(dash_track(&audio, true));
            }
        }
    }
    let ext = tracks[0].container.ext(tracks[0].audio_only);
    Ok(Plan { kind, tracks, est_bytes, ext })
}

// ---------------------------------------------------------------- download

pub struct StreamCtx {
    pub client: reqwest::Client,
    pub headers: HeaderSet,
    pub part: PathBuf,
    pub max_conn: u32,
    pub global: Limiter,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub choice: StreamChoice,
    pub rt: Arc<Runtime>,
}

pub struct StreamOutcome {
    pub total: u64,
    /// a separate audio track that could not be joined with the video:
    /// (file in the work folder's parent, extension)
    pub companion: Option<(PathBuf, &'static str)>,
}

/// The folder that holds the finished pieces of a stream download.
pub fn work_dir(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_os_string();
    s.push(".d");
    PathBuf::from(s)
}

fn piece_path(dir: &Path, track: usize, i: usize) -> PathBuf {
    dir.join(format!("t{track}-{i:06}.seg"))
}

#[derive(Debug)]
enum PieceErr {
    Cancelled,
    /// may work later; this is the message once retries are used up
    Retry(String),
    Http(u16),
    Fatal(String),
}

fn aes128_cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], data: &mut Vec<u8>) -> Result<(), String> {
    use aes::cipher::{BlockCipherDecrypt, KeyInit};
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("an encrypted segment has an invalid length".into());
    }
    let c = aes::Aes128::new(key.into());
    let mut prev = *iv;
    for chunk in data.as_chunks_mut::<16>().0 {
        let cur = *chunk;
        let block: &mut aes::Block = chunk.into();
        c.decrypt_block(block);
        for (b, p) in chunk.iter_mut().zip(prev.iter()) {
            *b ^= p;
        }
        prev = cur;
    }
    // PKCS#7 padding
    let pad = *data.last().unwrap() as usize;
    if pad == 0 || pad > 16 || pad > data.len() || !data[data.len() - pad..].iter().all(|b| *b as usize == pad) {
        return Err("decryption failed (wrong key?)".into());
    }
    data.truncate(data.len() - pad);
    Ok(())
}

struct Shared {
    ctx: StreamCtx,
    dir: PathBuf,
    /// (track, index in track) for each unit
    units_map: Vec<(usize, usize)>,
    pieces: Vec<Piece>,
    units: Arc<Units>,
    queue: parking_lot::Mutex<VecDeque<usize>>,
    keys: tokio::sync::Mutex<HashMap<String, [u8; 16]>>,
}

async fn key_for(sh: &Shared, uri: &str) -> Result<[u8; 16], PieceErr> {
    let mut keys = sh.keys.lock().await;
    if let Some(k) = keys.get(uri) {
        return Ok(*k);
    }
    let resp = client::send(&sh.ctx.client, uri, &sh.ctx.headers, &[], Some(Duration::from_secs(30)))
        .await
        .map_err(|e| PieceErr::Retry(e.to_string()))?;
    let st = resp.status().as_u16();
    if !(200..300).contains(&st) {
        return Err(if matches!(st, 408 | 429 | 500..=599) { PieceErr::Retry(client::status_message(st)) } else { PieceErr::Http(st) });
    }
    let body = resp.bytes().await.map_err(|e| PieceErr::Retry(CoreError::from_reqwest(e).to_string()))?;
    let k: [u8; 16] = body.as_ref().try_into().map_err(|_| PieceErr::Fatal("the stream's decryption key is not a 16-byte AES-128 key".into()))?;
    keys.insert(uri.to_string(), k);
    Ok(k)
}

/// Fetch one piece into its file. Retries are the caller's.
async fn fetch_piece(sh: &Shared, u: usize, stop: &CancellationToken) -> Result<u64, PieceErr> {
    let (t, i) = sh.units_map[u];
    let piece = &sh.pieces[u];
    let done_path = piece_path(&sh.dir, t, i);
    let tmp = done_path.with_extension("tmp");
    sh.units.start(u);
    let mut extra: Vec<(&'static str, String)> = vec![];
    let (off, len) = match piece.range {
        Some((o, l)) => (o, Some(l)),
        None => (0, None),
    };
    if piece.range.is_some() {
        extra.push(("range", format!("bytes={}-{}", off, off + len.unwrap_or(1) - 1)));
    }
    let resp = tokio::select! {
        _ = stop.cancelled() => return Err(PieceErr::Cancelled),
        r = client::send(&sh.ctx.client, &piece.url, &sh.ctx.headers, &extra, None) => r,
    };
    let resp = match resp {
        Ok(r) => r,
        Err(e @ (CoreError::InsecureRedirect | CoreError::BadUrl(_))) => return Err(PieceErr::Fatal(e.to_string())),
        Err(e) => return Err(PieceErr::Retry(e.to_string())),
    };
    let st = resp.status().as_u16();
    // when a range was asked for but the whole file came back, skip to it
    let mut skip = 0u64;
    match st {
        206 if piece.range.is_some() => {
            let first = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(client::parse_content_range).and_then(|c| c.range).map(|r| r.0);
            if first != Some(off) {
                return Err(PieceErr::Fatal("the server returned the wrong byte range of a segment".into()));
            }
        }
        200..=299 => skip = if piece.range.is_some() { off } else { 0 },
        408 | 425 | 429 | 500..=599 => return Err(PieceErr::Retry(client::status_message(st))),
        _ => return Err(PieceErr::Http(st)),
    }
    let mut file = tokio::fs::File::create(&tmp).await.map_err(|e| PieceErr::Fatal(format!("cannot write file: {e}")))?;
    let mut want = len;
    let mut stream = resp.bytes_stream();
    let mut written = 0u64;
    let rt = &sh.ctx.rt;
    loop {
        let next = tokio::select! {
            _ = stop.cancelled() => return Err(PieceErr::Cancelled),
            n = stream.next() => n,
        };
        let mut chunk = match next {
            None => break,
            Some(Err(e)) => return Err(PieceErr::Retry(CoreError::from_reqwest(e).to_string())),
            Some(Ok(c)) => c,
        };
        if skip > 0 {
            let s = skip.min(chunk.len() as u64) as usize;
            skip -= s as u64;
            chunk = chunk.slice(s..);
        }
        if let Some(w) = want.as_mut() {
            let take = (*w).min(chunk.len() as u64) as usize;
            chunk = chunk.slice(..take);
            *w -= take as u64;
        }
        if chunk.is_empty() {
            if want == Some(0) {
                break;
            }
            continue;
        }
        let n = chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|e| PieceErr::Fatal(format!("write failed: {e}")))?;
        written += n;
        sh.units.add(u, n);
        if !sh.ctx.global.acquire_cancellable(n, stop).await || !rt.limiter.acquire_cancellable(n, stop).await {
            return Err(PieceErr::Cancelled);
        }
        if rt.phase.load(Ordering::SeqCst) == 0 {
            rt.phase.store(1, Ordering::SeqCst);
        }
        if want == Some(0) {
            break;
        }
    }
    if want.is_some_and(|w| w > 0) {
        return Err(PieceErr::Retry("the connection dropped during a segment".into()));
    }
    file.flush().await.map_err(|e| PieceErr::Fatal(format!("write failed: {e}")))?;
    file.sync_data().await.map_err(|e| PieceErr::Fatal(format!("write failed: {e}")))?;
    drop(file);
    let mut size = written;
    if let Some(k) = &piece.key {
        let key = key_for(sh, &k.uri).await?;
        let mut data = tokio::fs::read(&tmp).await.map_err(|e| PieceErr::Fatal(format!("cannot read file: {e}")))?;
        aes128_cbc_decrypt(&key, &k.iv, &mut data).map_err(PieceErr::Fatal)?;
        size = data.len() as u64;
        tokio::fs::write(&tmp, &data).await.map_err(|e| PieceErr::Fatal(format!("write failed: {e}")))?;
    }
    tokio::fs::rename(&tmp, &done_path).await.map_err(|e| PieceErr::Fatal(format!("write failed: {e}")))?;
    Ok(size)
}

async fn worker(sh: Arc<Shared>, stop: CancellationToken) -> Result<(), PieceErr> {
    loop {
        let Some(u) = sh.queue.lock().pop_front() else { return Ok(()) };
        let mut attempts = 0u32;
        loop {
            if stop.is_cancelled() {
                return Err(PieceErr::Cancelled);
            }
            match fetch_piece(&sh, u, &stop).await {
                Ok(size) => {
                    sh.units.finish(u, size);
                    sh.ctx.rt.total.store(sh.units.total(), Ordering::SeqCst);
                    break;
                }
                Err(PieceErr::Retry(msg)) => {
                    sh.units.reset(u);
                    attempts += 1;
                    if attempts > sh.ctx.max_retries {
                        return Err(PieceErr::Fatal(msg));
                    }
                    let base = sh.ctx.retry_delay.as_millis() as u64;
                    let ms = (base * (1u64 << attempts.saturating_sub(1).min(4))).min(60_000);
                    tokio::select! {
                        _ = stop.cancelled() => return Err(PieceErr::Cancelled),
                        _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
                    }
                }
                Err(e) => {
                    sh.units.reset(u);
                    return Err(e);
                }
            }
        }
    }
}

/// Download the pieces of `plan` and join them into the part file.
pub async fn download(ctx: StreamCtx, plan: Plan) -> Result<StreamOutcome, TaskError> {
    let rt = ctx.rt.clone();
    let dir = work_dir(&ctx.part);
    let fatal = |m: String| TaskError::Fatal(m);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| fatal(format!("cannot create folder: {e}")))?;
    // finished pieces from another plan (a different quality, a changed
    // stream) are of no use
    let fp = plan.fingerprint(&ctx.choice);
    let fp_path = dir.join("plan.txt");
    let old = tokio::fs::read_to_string(&fp_path).await.unwrap_or_default();
    if old != fp {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.map_err(|e| fatal(format!("cannot create folder: {e}")))?;
        tokio::fs::write(&fp_path, &fp).await.map_err(|e| fatal(format!("write failed: {e}")))?;
    }
    if let Some(p) = ctx.part.parent() {
        tokio::fs::create_dir_all(p).await.map_err(|e| fatal(format!("cannot create folder: {e}")))?;
    }

    let mut units_map = vec![];
    let mut pieces = vec![];
    let mut done = vec![];
    for (t, track) in plan.tracks.iter().enumerate() {
        for (i, p) in track.pieces.iter().enumerate() {
            units_map.push((t, i));
            pieces.push(p.clone());
            done.push(std::fs::metadata(piece_path(&dir, t, i)).ok().map(|m| m.len()));
        }
    }
    let units = Arc::new(Units::new(&done, plan.est_bytes));
    let _ = rt.units.set(units.clone());
    let units = rt.units.get().cloned().unwrap_or(units);
    rt.total.store(units.total(), Ordering::SeqCst);
    rt.resumable.store(true, Ordering::SeqCst);
    let queue: VecDeque<usize> = (0..pieces.len()).filter(|u| !units.is_done(*u)).collect();
    let conns = (ctx.max_conn.max(1) as usize).min(queue.len().max(1));
    let sh = Arc::new(Shared {
        ctx,
        dir: dir.clone(),
        units_map,
        pieces,
        units: units.clone(),
        queue: parking_lot::Mutex::new(queue),
        keys: tokio::sync::Mutex::new(HashMap::new()),
    });

    let stop = rt.cancel.child_token();
    let mut set = JoinSet::new();
    for _ in 0..conns {
        rt.active.fetch_add(1, Ordering::SeqCst);
        set.spawn(worker(sh.clone(), stop.clone()));
    }
    let mut failure: Option<PieceErr> = None;
    while let Some(j) = set.join_next().await {
        rt.active.fetch_sub(1, Ordering::SeqCst);
        match j {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if failure.is_none() || matches!(failure, Some(PieceErr::Cancelled)) {
                    failure = Some(e);
                }
                stop.cancel();
            }
            Err(e) => {
                failure.get_or_insert(PieceErr::Fatal(format!("internal error: {e}")));
                stop.cancel();
            }
        }
    }
    if rt.cancel.is_cancelled() {
        return Err(TaskError::Cancelled);
    }
    match failure {
        Some(PieceErr::Cancelled) => return Err(TaskError::Cancelled),
        Some(PieceErr::Http(c)) => return Err(TaskError::Http(c)),
        Some(PieceErr::Fatal(m)) | Some(PieceErr::Retry(m)) => return Err(TaskError::Fatal(m)),
        None => {}
    }
    if (0..units.len()).any(|u| !units.is_done(u)) {
        return Err(TaskError::Fatal("segments incomplete".into()));
    }

    // ---- join ----
    rt.phase.store(2, Ordering::SeqCst);
    let part = sh.ctx.part.clone();
    let counts: Vec<usize> = plan.tracks.iter().map(|t| t.pieces.len()).collect();
    let mux = plan.can_mux();
    let audio_ext = plan.tracks.get(1).map(|t| t.container.ext(true));
    let joined = tokio::task::spawn_blocking(move || assemble(&dir, &counts, &part, mux)).await;
    let companion = match joined {
        Ok(Ok(c)) => c.map(|p| (p, audio_ext.unwrap_or("m4a"))),
        Ok(Err(e)) => return Err(TaskError::Fatal(format!("could not join the stream's segments: {e}"))),
        Err(e) => return Err(TaskError::Fatal(format!("internal error: {e}"))),
    };
    let total = std::fs::metadata(&sh.ctx.part).map(|m| m.len()).map_err(|e| fatal(format!("cannot open file: {e}")))?;
    rt.total.store(total, Ordering::SeqCst);
    Ok(StreamOutcome { total, companion })
}

fn concat(dir: &Path, track: usize, count: usize, out: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(out)?);
    for i in 0..count {
        let mut f = std::fs::File::open(piece_path(dir, track, i))?;
        std::io::copy(&mut f, &mut o)?;
    }
    o.flush()?;
    o.into_inner().map_err(|e| e.into_error())?.sync_all()
}

/// Join the pieces into `part`; a second track that can't be muxed is
/// returned as its own file.
fn assemble(dir: &Path, counts: &[usize], part: &Path, mux: bool) -> std::io::Result<Option<PathBuf>> {
    let mut companion = None;
    if counts.len() == 1 {
        concat(dir, 0, counts[0], part)?;
    } else {
        let v = dir.join("video.track");
        let a = dir.join("audio.track");
        concat(dir, 0, counts[0], &v)?;
        concat(dir, 1, counts[1], &a)?;
        if mux && crate::mp4::is_fragmented(&v) && crate::mp4::is_fragmented(&a) {
            crate::mp4::merge(&v, &a, part)?;
        } else {
            std::fs::rename(&v, part)?;
            let mut s = part.as_os_str().to_os_string();
            s.push(".audio");
            let c = PathBuf::from(s);
            std::fs::rename(&a, &c)?;
            companion = Some(c);
        }
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(companion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        assert_eq!(manifest_kind("https://x/a/master.m3u8?t=1", None), Some(Kind::Hls));
        assert_eq!(manifest_kind("https://x/a/manifest.MPD", None), Some(Kind::Dash));
        assert_eq!(manifest_kind("https://x/a/play", Some("application/vnd.apple.mpegurl")), Some(Kind::Hls));
        assert_eq!(manifest_kind("https://x/a/play", Some("application/x-mpegURL; charset=utf-8")), Some(Kind::Hls));
        assert_eq!(manifest_kind("https://x/a/play", Some("application/dash+xml")), Some(Kind::Dash));
        assert_eq!(manifest_kind("https://x/a/video.mp4", Some("video/mp4")), None);
        assert_eq!(sniff("#EXTM3U\n"), Some(Kind::Hls));
        assert_eq!(sniff("<?xml version=\"1.0\"?><MPD></MPD>"), Some(Kind::Dash));
    }

    #[test]
    fn aes_cbc() {
        // NIST SP 800-38A F.2.2 (CBC-AES128.Decrypt), first block, plus a
        // full block of PKCS#7 padding encrypted after it
        let key: [u8; 16] = util::from_hex("2b7e151628aed2a6abf7158809cf4f3c").unwrap().try_into().unwrap();
        let iv: [u8; 16] = util::from_hex("000102030405060708090a0b0c0d0e0f").unwrap().try_into().unwrap();
        let mut ct = util::from_hex("7649abac8119b246cee98e9b12e9197d").unwrap();
        let pad = encrypt_block(&key, &ct[..16].try_into().unwrap(), &[16u8; 16]);
        ct.extend_from_slice(&pad);
        aes128_cbc_decrypt(&key, &iv, &mut ct).unwrap();
        assert_eq!(util::to_hex(&ct), "6bc1bee22e409f96e93d7e117393172a");
        let mut bad = vec![1u8; 16];
        assert!(aes128_cbc_decrypt(&key, &iv, &mut bad).is_err());
    }

    pub(crate) fn encrypt_block(key: &[u8; 16], prev: &[u8; 16], plain: &[u8; 16]) -> [u8; 16] {
        use aes::cipher::{BlockCipherEncrypt, KeyInit};
        let c = aes::Aes128::new(key.into());
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = plain[i] ^ prev[i];
        }
        let mut blk: aes::Block = b.into();
        c.encrypt_block(&mut blk);
        blk.into()
    }

    #[test]
    fn unit_layout() {
        let u = Units::new(&[Some(100), None, None, Some(50)], Some(400));
        // two finished pieces are too few to go by: the estimate stays 400 / 4
        let (v, total) = u.layout(false);
        assert_eq!(total, 100 + 100 + 100 + 50);
        assert_eq!(v.len(), 3);
        u.start(1);
        u.add(1, 30);
        let (v, _) = u.layout(false);
        assert_eq!(v[1].received, 30);
        assert_eq!(u.received(), 180);
        u.finish(1, 60);
        // three finished: their average (70) is the estimate now
        assert_eq!(u.total(), 100 + 60 + 70 + 50);
        // on disk: the finished pieces only
        let (v, _) = u.layout(true);
        assert_eq!(v.iter().map(|s| s.received).sum::<u64>(), 100 + 60 + 50);
    }
}
