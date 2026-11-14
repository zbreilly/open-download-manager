//! Joins a fragmented-MP4 video track and a fragmented-MP4 audio track
//! (the usual DASH / CMAF layout) into one playable MP4.
//!
//! No samples are touched: the audio track's `trak` and `trex` are added to
//! the video's `moov` under a new track ID, and the fragments (`moof` +
//! `mdat`) of both files are written in decode-time order, with the audio
//! fragments' `tfhd` track IDs and the `mfhd` sequence numbers rewritten.
//! Segment-level boxes (`styp`, `sidx`) are dropped: their byte offsets
//! would no longer be right.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
struct BoxPos {
    kind: [u8; 4],
    /// offset of the box header
    start: u64,
    /// total size including the header
    size: u64,
}

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

/// Top-level boxes of a file.
fn scan(f: &mut File) -> io::Result<Vec<BoxPos>> {
    let len = f.metadata()?.len();
    let mut out = vec![];
    let mut at = 0u64;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at))?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8])?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut header = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16])?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header || at + size > len {
            return Err(bad("damaged MP4 box"));
        }
        out.push(BoxPos { kind, start: at, size });
        at += size;
    }
    Ok(out)
}

/// Child boxes of an in-memory box body.
fn children(body: &[u8]) -> Vec<(usize, usize, [u8; 4])> {
    // (start, end, kind); start is the header
    let mut v = vec![];
    let mut at = 0usize;
    while at + 8 <= body.len() {
        let mut size = u32::from_be_bytes(body[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = body[at + 4..at + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 {
            if at + 16 > body.len() {
                break;
            }
            size = u64::from_be_bytes(body[at + 8..at + 16].try_into().unwrap()) as usize;
            hdr = 16;
        } else if size == 0 {
            size = body.len() - at;
        }
        if size < hdr || at + size > body.len() {
            break;
        }
        v.push((at, at + size, kind));
        at += size;
    }
    v
}

fn header_len(b: &[u8]) -> usize {
    if b.len() >= 4 && u32::from_be_bytes(b[0..4].try_into().unwrap()) == 1 {
        16
    } else {
        8
    }
}

fn make_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn find<'a>(b: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    let mut cur = b;
    for k in path {
        let body = &cur[header_len(cur)..];
        let (s, e, _) = children(body).into_iter().find(|c| &c.2 == *k)?;
        cur = &body[s..e];
    }
    Some(cur)
}

fn tkhd_id_offset(tkhd: &[u8]) -> Option<usize> {
    let h = header_len(tkhd);
    let v = *tkhd.get(h)?;
    Some(h + if v == 1 { 4 + 16 } else { 4 + 8 })
}

fn track_id(trak: &[u8]) -> Option<u32> {
    let tkhd = find(trak, &[b"tkhd"])?;
    let o = tkhd_id_offset(tkhd)?;
    Some(u32::from_be_bytes(tkhd.get(o..o + 4)?.try_into().ok()?))
}

fn timescale(trak: &[u8]) -> Option<u32> {
    let mdhd = find(trak, &[b"mdia", b"mdhd"])?;
    let h = header_len(mdhd);
    let o = h + if *mdhd.get(h)? == 1 { 4 + 16 } else { 4 + 8 };
    Some(u32::from_be_bytes(mdhd.get(o..o + 4)?.try_into().ok()?))
}

/// The `trak` with its track ID set to `id`.
fn retrack(trak: &[u8], id: u32) -> Option<Vec<u8>> {
    let mut t = trak.to_vec();
    let h = header_len(&t);
    let body = &t[h..];
    let (s, _, _) = children(body).into_iter().find(|c| &c.2 == b"tkhd")?;
    let tkhd_at = h + s;
    let o = tkhd_id_offset(&t[tkhd_at..])?;
    t[tkhd_at + o..tkhd_at + o + 4].copy_from_slice(&id.to_be_bytes());
    Some(t)
}

struct Track {
    moov: Vec<u8>,
    ftyp: Option<Vec<u8>>,
    /// fragments: (start, end) byte ranges (moof up to its last mdat)
    frags: Vec<(u64, u64)>,
}

fn read_range(f: &mut File, start: u64, len: u64) -> io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(start))?;
    let mut v = vec![0u8; len as usize];
    f.read_exact(&mut v)?;
    Ok(v)
}

fn load(f: &mut File) -> io::Result<Track> {
    let boxes = scan(f)?;
    let mut moov = None;
    let mut ftyp = None;
    let mut frags: Vec<(u64, u64)> = vec![];
    let mut open: Option<(u64, u64)> = None;
    for b in &boxes {
        match &b.kind {
            b"moov" if moov.is_none() => moov = Some(read_range(f, b.start, b.size)?),
            b"ftyp" if ftyp.is_none() => ftyp = Some(read_range(f, b.start, b.size)?),
            b"moof" => {
                if let Some(fr) = open.take() {
                    frags.push(fr);
                }
                open = Some((b.start, b.start + b.size));
            }
            b"mdat" => {
                if let Some(fr) = open.as_mut() {
                    fr.1 = b.start + b.size;
                }
            }
            _ => {}
        }
    }
    if let Some(fr) = open {
        frags.push(fr);
    }
    let moov = moov.ok_or_else(|| bad("no moov box"))?;
    Ok(Track { moov, ftyp, frags })
}

/// Is this a fragmented MP4 (has `moov` and at least one `moof`)?
pub fn is_fragmented(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else { return false };
    match scan(&mut f) {
        Ok(b) => b.iter().any(|x| &x.kind == b"moov") && b.iter().any(|x| &x.kind == b"moof"),
        Err(_) => false,
    }
}

/// Decode time (in seconds) of the first track fragment of a `moof`.
fn decode_time(moof: &[u8], scale: u32) -> Option<f64> {
    let tfdt = find(moof, &[b"traf", b"tfdt"])?;
    let h = header_len(tfdt);
    let v = *tfdt.get(h)?;
    let t = if v == 1 {
        u64::from_be_bytes(tfdt.get(h + 4..h + 12)?.try_into().ok()?)
    } else {
        u32::from_be_bytes(tfdt.get(h + 4..h + 8)?.try_into().ok()?) as u64
    };
    Some(t as f64 / scale.max(1) as f64)
}

/// Rewrite a fragment in place: the `mfhd` sequence number, every `tfhd`
/// track ID (when `new_id` is set) and absolute base data offsets (moved
/// by `delta`).
fn patch_fragment(frag: &mut [u8], seq: u32, new_id: Option<u32>, delta: i64) {
    let h = header_len(frag);
    let moof_end = {
        let size = u32::from_be_bytes(frag[0..4].try_into().unwrap()) as usize;
        if size == 1 {
            u64::from_be_bytes(frag[8..16].try_into().unwrap()) as usize
        } else {
            size
        }
    }
    .min(frag.len());
    for (s, e, k) in children(&frag[h..moof_end]) {
        let (s, e) = (h + s, h + e);
        match &k {
            b"mfhd" => {
                let o = s + header_len(&frag[s..e]) + 4;
                if o + 4 <= e {
                    frag[o..o + 4].copy_from_slice(&seq.to_be_bytes());
                }
            }
            b"traf" => {
                let th = header_len(&frag[s..e]);
                for (ts, te, tk) in children(&frag[s + th..e]) {
                    if &tk != b"tfhd" {
                        continue;
                    }
                    let (ts, te) = (s + th + ts, s + th + te);
                    let b = ts + header_len(&frag[ts..te]);
                    if b + 8 > te {
                        continue;
                    }
                    let flags = u32::from_be_bytes([0, frag[b + 1], frag[b + 2], frag[b + 3]]);
                    if let Some(id) = new_id {
                        frag[b + 4..b + 8].copy_from_slice(&id.to_be_bytes());
                    }
                    if flags & 1 != 0 && b + 16 <= te && delta != 0 {
                        let old = u64::from_be_bytes(frag[b + 8..b + 16].try_into().unwrap());
                        let new = (old as i64 + delta).max(0) as u64;
                        frag[b + 8..b + 16].copy_from_slice(&new.to_be_bytes());
                    }
                }
            }
            _ => {}
        }
    }
}

/// Merge `video` and `audio` (both fragmented MP4) into `out`.
pub fn merge(video: &Path, audio: &Path, out: &Path) -> io::Result<()> {
    let mut vf = File::open(video)?;
    let mut af = File::open(audio)?;
    let v = load(&mut vf)?;
    let a = load(&mut af)?;
    if v.frags.is_empty() || a.frags.is_empty() {
        return Err(bad("not a fragmented MP4"));
    }

    // tracks of each moov
    let traks = |moov: &[u8]| -> Vec<Vec<u8>> {
        let h = header_len(moov);
        children(&moov[h..]).into_iter().filter(|c| &c.2 == b"trak").map(|(s, e, _)| moov[h + s..h + e].to_vec()).collect()
    };
    let vtraks = traks(&v.moov);
    let atraks = traks(&a.moov);
    let atrak = atraks.first().ok_or_else(|| bad("audio has no track"))?;
    let vtrak = vtraks.first().ok_or_else(|| bad("video has no track"))?;
    let old_aid = track_id(atrak).ok_or_else(|| bad("audio track has no ID"))?;
    let new_aid = vtraks.iter().filter_map(|t| track_id(t)).max().unwrap_or(1) + 1;
    let vscale = timescale(vtrak).unwrap_or(1);
    let ascale = timescale(atrak).unwrap_or(1);
    let atrak = retrack(atrak, new_aid).ok_or_else(|| bad("audio track header damaged"))?;
    let atrex = find(&a.moov, &[b"mvex"]).and_then(|mvex| {
        let h = header_len(mvex);
        children(&mvex[h..]).into_iter().filter(|c| &c.2 == b"trex").find_map(|(s, e, _)| {
            let t = &mvex[h + s..h + e];
            let o = header_len(t) + 4;
            let id = u32::from_be_bytes(t.get(o..o + 4)?.try_into().ok()?);
            (id == old_aid).then(|| {
                let mut t = t.to_vec();
                t[o..o + 4].copy_from_slice(&new_aid.to_be_bytes());
                t
            })
        })
    });

    // the new moov: the video's children, with the audio trak after the
    // last video trak, the audio trex in mvex, and next_track_ID moved on
    let vh = header_len(&v.moov);
    let kids = children(&v.moov[vh..]);
    let last_trak = kids.iter().rposition(|c| &c.2 == b"trak");
    let mut body = vec![];
    let mut had_mvex = false;
    for (i, (s, e, k)) in kids.iter().enumerate() {
        let src = &v.moov[vh + s..vh + e];
        match k {
            b"mvhd" => {
                let mut m = src.to_vec();
                let h = header_len(&m);
                let o = h + if m.get(h) == Some(&1) { 108 } else { 96 };
                if o + 4 <= m.len() {
                    m[o..o + 4].copy_from_slice(&(new_aid + 1).to_be_bytes());
                }
                body.extend_from_slice(&m);
            }
            b"mvex" => {
                had_mvex = true;
                let mut mb = src[header_len(src)..].to_vec();
                if let Some(t) = &atrex {
                    mb.extend_from_slice(t);
                }
                body.extend_from_slice(&make_box(b"mvex", &mb));
            }
            _ => body.extend_from_slice(src),
        }
        if Some(i) == last_trak {
            body.extend_from_slice(&atrak);
        }
    }
    if !had_mvex {
        if let Some(t) = &atrex {
            body.extend_from_slice(&make_box(b"mvex", t));
        }
    }
    let moov = make_box(b"moov", &body);

    // fragments in decode-time order
    struct F {
        audio: bool,
        start: u64,
        end: u64,
        time: f64,
        n: usize,
    }
    let mut frags = vec![];
    for (audio, t, f, scale) in [(false, &v, &mut vf, vscale), (true, &a, &mut af, ascale)] {
        for (n, (s, e)) in t.frags.iter().enumerate() {
            let head = read_range(f, *s, (e - s).min(64 * 1024))?;
            let time = decode_time(&head, scale).unwrap_or(n as f64 * 1e-9);
            frags.push(F { audio, start: *s, end: *e, time, n });
        }
    }
    frags.sort_by(|x, y| x.time.partial_cmp(&y.time).unwrap_or(std::cmp::Ordering::Equal).then(x.audio.cmp(&y.audio)).then(x.n.cmp(&y.n)));

    let mut o = io::BufWriter::with_capacity(1 << 20, File::create(out)?);
    let mut pos = 0u64;
    if let Some(ft) = v.ftyp.as_ref().or(a.ftyp.as_ref()) {
        o.write_all(ft)?;
        pos += ft.len() as u64;
    }
    o.write_all(&moov)?;
    pos += moov.len() as u64;
    for (i, fr) in frags.iter().enumerate() {
        let f = if fr.audio { &mut af } else { &mut vf };
        let mut buf = read_range(f, fr.start, fr.end - fr.start)?;
        let delta = pos as i64 - fr.start as i64;
        patch_fragment(&mut buf, (i + 1) as u32, fr.audio.then_some(new_aid), delta);
        o.write_all(&buf)?;
        pos += buf.len() as u64;
    }
    o.flush()?;
    o.into_inner().map_err(|e| e.into_error())?.sync_all()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn full(kind: &[u8; 4], version: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
        let mut b = vec![version];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(rest);
        make_box(kind, &b)
    }

    fn trak(id: u32, scale: u32) -> Vec<u8> {
        let mut tk = vec![0u8; 8];
        tk.extend_from_slice(&id.to_be_bytes());
        tk.extend_from_slice(&[0u8; 72]);
        let tkhd = full(b"tkhd", 0, 3, &tk);
        let mut md = vec![0u8; 8];
        md.extend_from_slice(&scale.to_be_bytes());
        md.extend_from_slice(&[0u8; 8]);
        let mdia = make_box(b"mdia", &full(b"mdhd", 0, 0, &md));
        make_box(b"trak", &[tkhd, mdia].concat())
    }

    /// A tiny fragmented MP4 with one track: `frags` fragments of
    /// `per` time units each; the mdat payload is `fill` bytes.
    pub(crate) fn fmp4(id: u32, scale: u32, frags: u32, per: u32, fill: u8) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\x01isomiso6");
        let mut mv = vec![0u8; 92];
        mv.extend_from_slice(&(id + 1).to_be_bytes());
        let mvhd = full(b"mvhd", 0, 0, &mv);
        let mut tx = id.to_be_bytes().to_vec();
        tx.extend_from_slice(&[0u8; 16]);
        let mvex = make_box(b"mvex", &full(b"trex", 0, 0, &tx));
        let moov = make_box(b"moov", &[mvhd, trak(id, scale), mvex].concat());
        let mut out = [ftyp, moov].concat();
        for i in 0..frags {
            out.extend_from_slice(&make_box(b"styp", b"msdh\0\0\0\0"));
            let mfhd = full(b"mfhd", 0, 0, &(i + 1).to_be_bytes());
            let tfhd = full(b"tfhd", 0, 0x020000, &id.to_be_bytes());
            let tfdt = full(b"tfdt", 1, 0, &((i * per) as u64).to_be_bytes());
            let traf = make_box(b"traf", &[tfhd, tfdt].concat());
            out.extend_from_slice(&make_box(b"moof", &[mfhd, traf].concat()));
            out.extend_from_slice(&make_box(b"mdat", &[fill; 100]));
        }
        out
    }

    #[test]
    fn merges_two_tracks() {
        let d = tempfile::tempdir().unwrap();
        let (vp, ap, op) = (d.path().join("v.mp4"), d.path().join("a.mp4"), d.path().join("o.mp4"));
        // video: 3 fragments of 2 s at 90 kHz; audio: 4 fragments of 1.5 s at 48 kHz
        std::fs::write(&vp, fmp4(1, 90_000, 3, 180_000, 0xAA)).unwrap();
        std::fs::write(&ap, fmp4(1, 48_000, 4, 72_000, 0xBB)).unwrap();
        assert!(is_fragmented(&vp));
        merge(&vp, &ap, &op).unwrap();

        let mut f = File::open(&op).unwrap();
        let boxes = scan(&mut f).unwrap();
        let kinds: Vec<String> = boxes.iter().map(|b| String::from_utf8_lossy(&b.kind).into_owned()).collect();
        assert_eq!(kinds[..2], ["ftyp", "moov"]);
        assert!(!kinds.iter().any(|k| k == "styp"));
        assert_eq!(kinds.iter().filter(|k| *k == "moof").count(), 7);

        let moov = read_range(&mut f, boxes[1].start, boxes[1].size).unwrap();
        let h = header_len(&moov);
        let traks: Vec<u32> = children(&moov[h..])
            .into_iter()
            .filter(|c| &c.2 == b"trak")
            .map(|(s, e, _)| track_id(&moov[h + s..h + e]).unwrap())
            .collect();
        assert_eq!(traks, [1, 2]);
        let mvex = find(&moov, &[b"mvex"]).unwrap();
        assert_eq!(children(&mvex[8..]).len(), 2);
        let mvhd = find(&moov, &[b"mvhd"]).unwrap();
        assert_eq!(u32::from_be_bytes(mvhd[8 + 96..8 + 100].try_into().unwrap()), 3);

        // fragments interleaved by time, audio re-numbered, sequence 1..
        let mut seen = vec![];
        for (i, b) in boxes.iter().filter(|b| &b.kind == b"moof").enumerate() {
            let moof = read_range(&mut f, b.start, b.size).unwrap();
            let mfhd = find(&moof, &[b"mfhd"]).unwrap();
            assert_eq!(u32::from_be_bytes(mfhd[12..16].try_into().unwrap()), i as u32 + 1);
            let tfhd = find(&moof, &[b"traf", b"tfhd"]).unwrap();
            let id = u32::from_be_bytes(tfhd[12..16].try_into().unwrap());
            let scale = if id == 1 { 90_000 } else { 48_000 };
            seen.push((id, decode_time(&moof, scale).unwrap()));
        }
        assert_eq!(seen, [(1, 0.0), (2, 0.0), (2, 1.5), (1, 2.0), (2, 3.0), (1, 4.0), (2, 4.5)]);
    }
}
