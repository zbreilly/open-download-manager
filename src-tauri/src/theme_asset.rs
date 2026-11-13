//! Pictures and fonts a colour profile carries.
//!
//! Profiles are shared between people, so a file inside one is untrusted.
//! Nothing from such a file reaches the webview as it arrived:
//!
//! * A picture is decoded here (a memory-safe decoder with size limits) and
//!   written out again as a new PNG that holds the pixels and nothing else.
//!   Text and metadata chunks, colour profiles, extra animation frames,
//!   bytes after the end of the image and anything else a file can carry
//!   beside its pixels are gone, and a file that is only dressed up as a PNG
//!   does not decode at all. Pixels that are fully transparent are cleared,
//!   since their colour is invisible and can hold anything.
//! * A font cannot be rebuilt this way. It must start like the kind of font
//!   it is stored as, and is stored under that kind whatever the file said.
//!
//! What a picture's visible pixels look like is the picture: data woven into
//! them survives, as it does in any image. It is never read back: the app
//! only ever draws these files.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

/// Largest file accepted, in bytes.
const MAX_INPUT: usize = 6 * 1024 * 1024;
/// Largest side of a picture, in pixels.
const MAX_SIDE: u32 = 4096;
/// Most memory the decoder may use for one picture.
const MAX_DECODE_BYTES: usize = 160 * 1024 * 1024;
/// Largest picture written out, in bytes (the webview's limit for a stored
/// file is a little above this once it is text).
const MAX_OUTPUT: usize = 11 * 1024 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The bytes of a `data:<type>;base64,<data>` URL. The type it claims is
/// ignored: what the bytes are decides.
fn payload(data_url: &str) -> Result<Vec<u8>, String> {
    let rest = data_url.strip_prefix("data:").ok_or("not a file")?;
    let (_claimed, b64) = rest.split_once(";base64,").ok_or("not a file")?;
    // base64 is four characters for every three bytes
    if b64.len() / 4 * 3 > MAX_INPUT + 3 {
        return Err("that file is too large (limit 6 MB)".into());
    }
    B64.decode(b64).map_err(|_| "not a file".to_string())
}

fn data_url(kind: &str, bytes: &[u8]) -> String {
    format!("data:{kind};base64,{}", B64.encode(bytes))
}

/// The kind of font these bytes start like.
fn font_kind(bytes: &[u8]) -> Option<&'static str> {
    match bytes.get(..4)? {
        [0x00, 0x01, 0x00, 0x00] | b"true" => Some("font/ttf"),
        b"OTTO" => Some("font/otf"),
        b"wOFF" => Some("font/woff"),
        b"wOF2" => Some("font/woff2"),
        _ => None,
    }
}

/// A new PNG with the pixels of `bytes` and nothing else.
pub fn clean_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    fn bad<E>(_: E) -> String {
        "that picture could not be read".to_string()
    }
    let mut decoder = png::Decoder::new_with_limits(bytes, png::Limits { bytes: MAX_DECODE_BYTES });
    // palettes and low bit depths expanded, 16-bit samples reduced: what
    // comes out is 8-bit grey, grey + alpha, RGB or RGBA
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(bad)?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("pictures can be up to {MAX_SIDE} × {MAX_SIDE} pixels"));
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    // the first frame only: an animation's other frames are not carried over
    let frame = reader.next_frame(&mut buf).map_err(bad)?;
    if frame.bit_depth != png::BitDepth::Eight || frame.width != width || frame.height != height {
        return Err("that picture could not be read".into());
    }
    let pixels = (width as usize) * (height as usize);
    let channels = frame.color_type.samples();
    let data = buf.get(..pixels * channels).ok_or("that picture could not be read")?;

    let mut rgba = Vec::with_capacity(pixels * 4);
    let mut opaque = true;
    for px in data.chunks_exact(channels) {
        let (r, g, b, a) = match frame.color_type {
            png::ColorType::Grayscale => (px[0], px[0], px[0], 255),
            png::ColorType::GrayscaleAlpha => (px[0], px[0], px[0], px[1]),
            png::ColorType::Rgb => (px[0], px[1], px[2], 255),
            png::ColorType::Rgba => (px[0], px[1], px[2], px[3]),
            // expanded above
            png::ColorType::Indexed => return Err("that picture could not be read".into()),
        };
        opaque &= a == 255;
        // nobody sees the colour of a fully transparent pixel
        rgba.extend_from_slice(&if a == 0 { [0, 0, 0, 0] } else { [r, g, b, a] });
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_color(if opaque { png::ColorType::Rgb } else { png::ColorType::Rgba });
        let mut writer = encoder.write_header().map_err(bad)?;
        if opaque {
            let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            writer.write_image_data(&rgb).map_err(bad)?;
        } else {
            writer.write_image_data(&rgba).map_err(bad)?;
        }
        writer.finish().map_err(bad)?;
    }
    if out.len() > MAX_OUTPUT {
        return Err("that picture is too large; use a smaller one".into());
    }
    Ok(out)
}

/// The file in `data_url`, in the form the app keeps: a rebuilt PNG, or a
/// font stored as the kind its bytes say it is. Anything else is refused.
pub fn clean(data_url_in: &str) -> Result<String, String> {
    let bytes = payload(data_url_in)?;
    if bytes.len() > MAX_INPUT {
        return Err("that file is too large (limit 6 MB)".into());
    }
    if bytes.starts_with(PNG_SIGNATURE) {
        return Ok(data_url("image/png", &clean_png(&bytes)?));
    }
    match font_kind(&bytes) {
        Some(kind) => Ok(data_url(kind, &bytes)),
        None => Err("that file is not a PNG picture or a font".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG of `pixels` (RGBA), with a text chunk, and `tail` after its end.
    fn png_with_extras(width: u32, height: u32, pixels: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, width, height);
            e.set_depth(png::BitDepth::Eight);
            e.set_color(png::ColorType::Rgba);
            e.add_text_chunk("Comment".into(), "HIDDEN-IN-TEXT".into()).unwrap();
            let mut w = e.write_header().unwrap();
            w.write_image_data(pixels).unwrap();
            w.finish().unwrap();
        }
        out.extend_from_slice(tail);
        out
    }

    fn decode(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut r = png::Decoder::new(bytes).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info, buf)
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn only_the_pixels_survive() {
        // one visible pixel, one fully transparent pixel with a colour
        let pixels = [10, 20, 30, 255, 0xDE, 0xAD, 0xBE, 0x00];
        let dirty = png_with_extras(2, 1, &pixels, b"<script>HIDDEN-AFTER-END</script>");
        assert!(contains(&dirty, b"HIDDEN-IN-TEXT") && contains(&dirty, b"HIDDEN-AFTER-END"));

        let clean = clean_png(&dirty).unwrap();
        assert!(!contains(&clean, b"HIDDEN-IN-TEXT"));
        assert!(!contains(&clean, b"HIDDEN-AFTER-END"));
        // ends with the end-of-image chunk
        assert!(clean.ends_with(b"IEND\xaeB\x60\x82"));
        // no chunk but the header, the pixels and the end
        let mut at = 8;
        let mut kinds = vec![];
        while at + 8 <= clean.len() {
            let len = u32::from_be_bytes(clean[at..at + 4].try_into().unwrap()) as usize;
            kinds.push(String::from_utf8_lossy(&clean[at + 4..at + 8]).to_string());
            at += 12 + len;
        }
        assert!(kinds.iter().all(|k| matches!(k.as_str(), "IHDR" | "IDAT" | "IEND")), "{kinds:?}");

        let (info, out) = decode(&clean);
        assert_eq!((info.width, info.height, info.color_type), (2, 1, png::ColorType::Rgba));
        // the visible pixel as it was, the invisible one emptied
        assert_eq!(out, [10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn cleaning_twice_changes_nothing() {
        let once = clean_png(&png_with_extras(1, 1, &[1, 2, 3, 255], b"")).unwrap();
        assert_eq!(clean_png(&once).unwrap(), once);
        let (info, out) = decode(&once);
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(out, [1, 2, 3]);
    }

    #[test]
    fn refuses_what_is_not_a_picture_or_a_font() {
        let url = |kind: &str, bytes: &[u8]| data_url(kind, bytes);
        // claims to be a PNG
        assert!(clean(&url("image/png", b"<svg onload=alert(1)>")).is_err());
        assert!(clean(&url("image/svg+xml", b"<svg xmlns='http://www.w3.org/2000/svg'/>")).is_err());
        // starts like a PNG and is something else
        let mut fake = PNG_SIGNATURE.to_vec();
        fake.extend_from_slice(b"MZ\x90\x00 not really an image");
        assert!(clean(&url("image/png", &fake)).is_err());
        // cut short
        let whole = png_with_extras(4, 4, &[7; 64], b"");
        assert!(clean_png(&whole[..whole.len() - 20]).is_err());
        assert!(clean("data:image/png;base64,@@@").is_err());
        assert!(clean("https://example.com/a.png").is_err());
    }

    #[test]
    fn refuses_pictures_that_are_too_big() {
        // one row, wider than any picture may be
        let mut huge = Vec::new();
        {
            let mut e = png::Encoder::new(&mut huge, MAX_SIDE + 1, 1);
            e.set_color(png::ColorType::Grayscale);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&vec![0; MAX_SIDE as usize + 1]).unwrap();
            w.finish().unwrap();
        }
        assert!(clean_png(&huge).unwrap_err().contains("4096"));
    }

    #[test]
    fn a_font_is_stored_as_what_it_is() {
        let mut otf = b"OTTO".to_vec();
        otf.extend_from_slice(&[0; 32]);
        let cleaned = clean(&data_url("application/octet-stream", &otf)).unwrap();
        assert!(cleaned.starts_with("data:font/otf;base64,"));
        let mut woff2 = b"wOF2".to_vec();
        woff2.extend_from_slice(&[0; 32]);
        assert!(clean(&data_url("image/png", &woff2)).unwrap().starts_with("data:font/woff2;base64,"));
        assert!(clean(&data_url("font/ttf", b"MZ\x90\x00\x03\x00\x00\x00")).is_err());
    }

    #[test]
    fn a_picture_comes_back_as_a_png_url() {
        let dirty = png_with_extras(1, 1, &[9, 9, 9, 128], b"tail");
        let cleaned = clean(&data_url("application/octet-stream", &dirty)).unwrap();
        let bytes = payload(&cleaned).unwrap();
        assert!(cleaned.starts_with("data:image/png;base64,") && bytes.starts_with(PNG_SIGNATURE));
        assert_eq!(decode(&bytes).1, [9, 9, 9, 128]);
    }
}
