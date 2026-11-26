//! What an SVG from mail may be before usvg reads it.
//!
//! usvg loads whatever an `<image>` (or `<feImage>`) names: with its default
//! options, a path is read from disk (`std::fs::read`), whatever the
//! resources folder, so an email's SVG could show local files, or, on
//! Windows, name a UNC path (`\\host\share\x.png`) and connect to that
//! host over SMB, handing it the account's NTLM hash, all with remote
//! content blocked. And it inflates SVGZ (gzip) to its end, so a small
//! file can stand for gigabytes.
//!
//! Blitz (the HTML reader) parses SVG with those default options, both the
//! `<svg>` elements written in an email and SVG images it loads; the
//! attachment preview (`display/tools.rs`) with its own. So before either:
//! gzip is inflated here, up to `MAX_SVG_BYTES`; and an SVG naming
//! anything but a picture inside itself (a `data:` URL of a raster image of
//! bounded size, or another such SVG) is refused, or, for an email's own
//! `<svg>` elements, has those names removed.
use std::{borrow::Cow, io::Read};

/// The most an SVG may be, inflated.
pub(crate) const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;
/// The most pixels a raster image inside an SVG may have (as for an
/// email's own images: see `resources::MAX_IMAGE_PIXELS`).
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
/// How deep SVG images inside SVG images are looked into.
const MAX_DEPTH: usize = 2;

/// Whether `bytes` are gzip-compressed (SVGZ).
fn gzipped(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// `bytes` inflated, if they inflate to at most `MAX_SVG_BYTES`.
pub(crate) fn inflate(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(MAX_SVG_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    (out.len() <= MAX_SVG_BYTES).then_some(out)
}

/// The elements whose `href` usvg loads a picture from.
fn loads_image(local: &str) -> bool {
    local.eq_ignore_ascii_case("image") || local.eq_ignore_ascii_case("feImage")
}

/// What a resource (an image, a stylesheet, a font) may be handed over as,
/// as far as SVG goes: gzip inflated (within `MAX_SVG_BYTES`), and an SVG
/// that names anything outside itself refused (`None`). Anything that isn't
/// an SVG is as it was.
pub(crate) fn resource(bytes: &[u8]) -> Option<Cow<'_, [u8]>> {
    if gzipped(bytes) {
        let inflated = inflate(bytes)?;
        // Inflated, it is no longer taken for gzip: read as it is.
        return document_safe(&inflated, 0, false).then_some(Cow::Owned(inflated));
    }
    match std::str::from_utf8(bytes) {
        // Not text: not an SVG (usvg would refuse it as well).
        Err(_) => Some(Cow::Borrowed(bytes)),
        Ok(text) if !looks_like_svg(text) => Some(Cow::Borrowed(bytes)),
        Ok(_) => document_safe(bytes, 0, false).then_some(Cow::Borrowed(bytes)),
    }
}

/// Whether `text` may be an SVG document (as usvg would read it): an
/// `<svg` tag anywhere in it.
fn looks_like_svg(text: &str) -> bool {
    text.as_bytes()
        .windows(4)
        .any(|w| w.eq_ignore_ascii_case(b"<svg"))
}

/// Whether the SVG document `bytes` names no picture but those allowed
/// (`href_allowed`). One that can't be parsed is let through: usvg can't
/// parse it either (it reads with the same parser and options). With
/// `names_ignored` (a reader whose usvg resolves no names, only `data:`
/// URLs), a name that isn't a `data:` URL is let through: it loads nothing.
fn document_safe(bytes: &[u8], depth: usize, names_ignored: bool) -> bool {
    if bytes.len() > MAX_SVG_BYTES || gzipped(bytes) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return true;
    };
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let Ok(doc) = roxmltree::Document::parse_with_options(text, options) else {
        return true;
    };
    doc.descendants()
        .filter(|n| n.is_element() && loads_image(n.tag_name().name()))
        .all(|n| {
            n.attributes()
                .filter(|a| a.name() == "href")
                .all(|a| {
                    (names_ignored && !is_data_url(a.value())) || href_allowed_with(a.value(), depth, names_ignored)
                })
        })
}

/// Whether an `<image>`'s address in an SVG may be loaded by usvg: a
/// `data:` URL of a raster image of at most `MAX_IMAGE_PIXELS`, or of an
/// SVG that is itself safe (not gzip). Never a path or a remote address:
/// usvg would read them from disk (or the network, by a UNC path).
pub(crate) fn href_allowed(href: &str, depth: usize) -> bool {
    href_allowed_with(href, depth, false)
}

fn is_data_url(href: &str) -> bool {
    href.trim().get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:"))
}

fn href_allowed_with(href: &str, depth: usize, names_ignored: bool) -> bool {
    let href = href.trim();
    if !is_data_url(href) {
        return false;
    }
    let Ok(url) = data_url::DataUrl::process(href) else {
        return false;
    };
    let Ok((bytes, _)) = url.decode_to_vec() else {
        return false;
    };
    // usvg reads data it doesn't know as a picture as an SVG, inflating
    // gzip to its end.
    if gzipped(&bytes) {
        return false;
    }
    let mime = url.mime_type();
    let svg = mime.type_ == "image" && mime.subtype == "svg+xml";
    if svg || std::str::from_utf8(&bytes).is_ok_and(looks_like_svg) {
        return depth < MAX_DEPTH && document_safe(&bytes, depth + 1, names_ignored);
    }
    // A raster image: usvg knows PNG, JPEG, GIF and WebP by their bytes;
    // anything else it drops.
    image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .filter(|r| r.format().is_some())
        .and_then(|r| r.into_dimensions().ok())
        .is_none_or(|(w, h)| u64::from(w) * u64::from(h) <= MAX_IMAGE_PIXELS)
}

/// An attachment's SVG as the preview's usvg may read it: inflated if gzip
/// (within `MAX_SVG_BYTES`), and refused if too large or if a picture inside
/// it isn't one `href_allowed` lets through (gzip, too many pixels). Names
/// of files or remote pictures are left to the preview, whose usvg resolves
/// none (`tools::decode_image_preview`).
pub(crate) fn attachment(bytes: &[u8]) -> Option<Cow<'_, [u8]>> {
    let bytes = if gzipped(bytes) {
        Cow::Owned(inflate(bytes)?)
    } else if bytes.len() > MAX_SVG_BYTES {
        return None;
    } else {
        Cow::Borrowed(bytes)
    };
    document_safe(&bytes, 0, true).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use std::io::Write as _;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(bytes).unwrap();
        z.finish().unwrap()
    }

    #[test]
    fn local_and_remote_pictures_are_refused() {
        for href in [
            "/etc/passwd",
            "\\\\attacker.example\\share\\x.png",
            "file:///etc/hosts",
            "https://example.com/x.png",
            "/dev/zero",
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="{href}"/><image href="{href}"/></svg>"#
            );
            assert!(resource(svg.as_bytes()).is_none(), "{href}");
            // The preview's usvg resolves no names: left to it.
            assert!(attachment(svg.as_bytes()).is_some(), "{href}");
            assert!(!href_allowed(href, 0));
        }
        let fe = r#"<svg xmlns="http://www.w3.org/2000/svg"><filter><feImage href="/etc/passwd"/></filter></svg>"#;
        assert!(resource(fe.as_bytes()).is_none());
    }

    #[test]
    fn pictures_inside_are_kept() {
        let png = {
            let mut out = Vec::new();
            image::RgbaImage::new(4, 4)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/png;base64,{b64}"/><rect width="1" height="1"/></svg>"#
        );
        assert!(resource(svg.as_bytes()).is_some());
        assert!(href_allowed(&format!("data:image/png;base64,{b64}"), 0));
        // An SVG inside naming a file is refused, as the outer one would be.
        let inner = r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="/etc/passwd"/></svg>"#;
        let inner = base64::engine::general_purpose::STANDARD.encode(inner);
        assert!(!href_allowed(&format!("data:image/svg+xml;base64,{inner}"), 0));
        // Not an SVG: as it was.
        assert!(matches!(resource(b"body { color: red }"), Some(Cow::Borrowed(_))));
    }

    #[test]
    fn svgz_is_inflated_within_bounds() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#;
        let gz = gzip(svg);
        let out = resource(&gz).unwrap();
        assert_eq!(&*out, &svg[..]);
        // A bomb: 64 MiB of spaces, a few kilobytes gzipped.
        let mut bomb = b"<svg xmlns=\"http://www.w3.org/2000/svg\">".to_vec();
        bomb.resize(64 << 20, b' ');
        let bomb = gzip(&bomb);
        assert!(bomb.len() < 200 << 10);
        assert!(resource(&bomb).is_none());
        assert!(attachment(&bomb).is_none());
    }
}
