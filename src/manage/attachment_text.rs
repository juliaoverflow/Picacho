//! The text of a message's attachments, for the local full-text index
//! (`Store::index_attachments`): text files (plain text, CSV, Markdown,
//! calendars, contacts, JSON, XML…), HTML, attached emails, PDFs, and
//! office documents (Word, Excel and PowerPoint's .docx/.xlsx/.pptx, and
//! OpenDocument's .odt/.ods/.odp). Attachments' names are taken too.
//!
//! What can't be read (images, archives, encrypted mail, damaged files)
//! adds nothing. Text is only for finding the message: words in order,
//! not laid out as shown.
use crate::sync::mime::{collect_leaves, decode_entities, html_to_text};
use mailparse::MailHeaderMap;
use std::io::Read;

/// The most text kept of one message's attachments, and of one attachment.
const MAX_TEXT: usize = 4 << 20;
const MAX_PART_TEXT: usize = 1 << 20;
/// Attachments larger than this (decoded) aren't read.
const MAX_PART: usize = 25 << 20;
/// The most a file inside an office document, or a PDF stream, may
/// inflate to.
const MAX_INFLATED: u64 = 32 << 20;
/// The most one office document or PDF may inflate to in all, its files
/// or streams together: a zip's entries can all name the same deflated
/// data, and a file that yields no text never reaches `MAX_PART_TEXT`.
const MAX_DOCUMENT_INFLATED: u64 = 64 << 20;
/// The most entries of a zip's directory read, and streams of a PDF.
const MAX_ZIP_ENTRIES: usize = 4096;
const MAX_PDF_STREAMS: usize = 20_000;
/// The most codes all of a PDF's ToUnicode maps may name, and the longest
/// text (in hex digits) one code may stand for: a `bfrange` line of a few
/// bytes names up to 65,536 codes, each with its own copy of the text.
const MAX_CMAP_ENTRIES: usize = 65_536;
const MAX_CMAP_HEX: usize = 64;
/// How deep attached emails are read into (an email in an email…).
const MAX_DEPTH: usize = 3;
/// The most strings a PDF content stream may draw before its next
/// operator: a stream of nothing but `(a)(a)(a)…` would otherwise keep
/// millions of them.
const MAX_PENDING: usize = 4096;

/// Whether a part is too large to read: its encoded size, before any of it
/// is decoded. Decoding never makes a part larger (base64 is a third
/// larger encoded, with line breaks a little more).
fn too_large(part: &mailparse::ParsedMail) -> bool {
    part.raw_bytes.len() > MAX_PART + MAX_PART / 2
}

/// The text of the attachments in `raw` (a message's source), names
/// first, one attachment after another.
pub fn extract(raw: &[u8]) -> String {
    let mut out = String::new();
    message(raw, 0, false, &mut out);
    truncate(&mut out, MAX_TEXT);
    out
}

/// Adds what `raw` holds to `out`: its attachments' text, and, for an
/// attached email (`whole`), its headers and text as well.
fn message(raw: &[u8], depth: usize, whole: bool, out: &mut String) {
    let Ok(p) = mailparse::parse_mail(raw) else {
        return;
    };
    let top = p.ctype.mimetype.to_ascii_lowercase();
    // Sealed: nothing readable without the keys.
    if top == "multipart/encrypted" || top.contains("pkcs7-mime") {
        return;
    }
    if whole {
        for name in ["From", "To", "Subject"] {
            if let Some(v) = p.headers.get_first_value(name) {
                push(out, &v);
            }
        }
    }
    let mut leaves = Vec::new();
    collect_leaves(&p, &mut leaves);
    for part in leaves {
        if out.len() >= MAX_TEXT {
            return;
        }
        let disposition = part.get_content_disposition();
        let name = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"))
            .cloned();
        let attachment =
            name.is_some() || disposition.disposition == mailparse::DispositionType::Attachment;
        let mime = part.ctype.mimetype.to_ascii_lowercase();
        if !attachment {
            // An attached email's own text.
            if whole && mime == "text/plain" {
                push(out, &part.get_body().unwrap_or_default());
            } else if whole && mime == "text/html" {
                push(out, &html_to_text(&part.get_body().unwrap_or_default()));
            } else if mime == "message/rfc822"
                && depth < MAX_DEPTH
                && !too_large(part)
                && let Ok(bytes) = part.get_body_raw()
            {
                message(&bytes, depth + 1, true, out);
            }
            continue;
        }
        let name = name.unwrap_or_default();
        if !name.is_empty() {
            push(out, &name);
        }
        if too_large(part) {
            continue;
        }
        let Ok(bytes) = part.get_body_raw() else {
            continue;
        };
        if bytes.len() > MAX_PART {
            continue;
        }
        let mut text = match kind(&mime, &name) {
            Kind::Text => part
                .get_body()
                .ok()
                .filter(|_| mime.starts_with("text/"))
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned()),
            Kind::Html => html_to_text(
                &part
                    .get_body()
                    .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned()),
            ),
            Kind::Email if depth < MAX_DEPTH => {
                let mut inner = String::new();
                if crate::sync::msg::is_msg(&bytes) {
                    if let Ok(eml) = crate::sync::msg::to_eml(&bytes) {
                        message(&eml, depth + 1, true, &mut inner);
                    }
                } else {
                    message(&bytes, depth + 1, true, &mut inner);
                }
                inner
            }
            Kind::Pdf => pdf_text(&bytes),
            Kind::Office(entries) => office_text(&bytes, entries),
            Kind::Email | Kind::None => continue,
        };
        truncate(&mut text, MAX_PART_TEXT);
        push(out, &text);
    }
}

/// What an attachment is read as.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Html,
    Email,
    Pdf,
    /// A zipped office document, and which of its files hold its text.
    Office(&'static [&'static str]),
    None,
}

const WORD: &[&str] = &["word/document.xml", "word/footnotes.xml", "word/endnotes.xml"];
const EXCEL: &[&str] = &["xl/sharedStrings.xml"];
const POWERPOINT: &[&str] = &["ppt/slides/slide"];
const OPEN_DOCUMENT: &[&str] = &["content.xml"];

fn kind(mime: &str, name: &str) -> Kind {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match (mime, ext.as_str()) {
        (_, "html" | "htm") | ("text/html", _) => Kind::Html,
        ("message/rfc822" | "application/vnd.ms-outlook", _) | (_, "eml" | "msg") => Kind::Email,
        ("application/pdf", _) | (_, "pdf") => Kind::Pdf,
        (_, "docx" | "docm" | "dotx") => Kind::Office(WORD),
        (_, "xlsx" | "xlsm") => Kind::Office(EXCEL),
        (_, "pptx" | "pptm") => Kind::Office(POWERPOINT),
        (_, "odt" | "ods" | "odp" | "ott") => Kind::Office(OPEN_DOCUMENT),
        (m, _) if m.contains("wordprocessingml") => Kind::Office(WORD),
        (m, _) if m.contains("spreadsheetml") => Kind::Office(EXCEL),
        (m, _) if m.contains("presentationml") => Kind::Office(POWERPOINT),
        (m, _) if m.starts_with("application/vnd.oasis.opendocument") => Kind::Office(OPEN_DOCUMENT),
        (
            _,
            "txt" | "text" | "csv" | "tsv" | "md" | "markdown" | "log" | "json" | "xml" | "ics"
            | "vcf" | "yaml" | "yml" | "ini" | "conf" | "srt" | "vtt",
        ) => Kind::Text,
        (m, _)
            if m.starts_with("text/")
                || matches!(m, "application/json" | "application/xml" | "application/ics") =>
        {
            Kind::Text
        }
        _ => Kind::None,
    }
}

/// Adds `text` to `out` on a line of its own.
fn push(out: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

/// Cuts `text` to at most `max` bytes, at a character's start.
fn truncate(text: &mut String, max: usize) {
    if text.len() > max {
        let mut at = max;
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        text.truncate(at);
    }
}

// Office documents: ZIP archives of XML.

/// The text of the XML files named by `entries` (or starting so, ending
/// in `.xml`) in the ZIP archive `zip`.
fn office_text(zip: &[u8], entries: &[&str]) -> String {
    let mut out = String::new();
    let mut budget = MAX_DOCUMENT_INFLATED;
    // Entries that share their data (a zip bomb's trick) are read once.
    let mut read = std::collections::HashSet::new();
    let mut files = zip_entries(zip);
    // Slides in their order: slide2 before slide10.
    files.sort_by_key(|f| {
        let digits: String = f.name.chars().filter(char::is_ascii_digit).collect();
        (digits.len(), digits)
    });
    for file in files {
        let wanted = entries
            .iter()
            .any(|e| file.name == *e || (file.name.starts_with(e) && file.name.ends_with(".xml")));
        if !wanted || !read.insert(file.local) {
            continue;
        }
        if budget == 0 {
            break;
        }
        if let Some(xml) = zip_read(zip, &file, budget.min(MAX_INFLATED)) {
            budget = budget.saturating_sub(xml.len() as u64);
            push(&mut out, &xml_text(&String::from_utf8_lossy(&xml)));
        }
        if out.len() >= MAX_PART_TEXT {
            break;
        }
    }
    out
}

struct ZipEntry {
    name: String,
    method: u16,
    compressed: usize,
    local: usize,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The files a ZIP archive lists in its central directory.
fn zip_entries(zip: &[u8]) -> Vec<ZipEntry> {
    let mut out = Vec::new();
    // The end of central directory record, within the last 64 KiB (its
    // comment) and 22 bytes.
    let from = zip.len().saturating_sub(65_557);
    let Some(end) = (from..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(zip, i) == Some(0x0605_4b50))
    else {
        return out;
    };
    let (Some(count), Some(offset)) = (u16_at(zip, end + 10), u32_at(zip, end + 16)) else {
        return out;
    };
    let mut at = offset as usize;
    for _ in 0..usize::from(count).min(MAX_ZIP_ENTRIES) {
        if u32_at(zip, at) != Some(0x0201_4b50) {
            break;
        }
        let (Some(method), Some(compressed), Some(n), Some(extra), Some(comment), Some(local)) = (
            u16_at(zip, at + 10),
            u32_at(zip, at + 20),
            u16_at(zip, at + 28),
            u16_at(zip, at + 30),
            u16_at(zip, at + 32),
            u32_at(zip, at + 42),
        ) else {
            break;
        };
        let Some(name) = zip.get(at + 46..at + 46 + n as usize) else {
            break;
        };
        out.push(ZipEntry {
            name: String::from_utf8_lossy(name).into_owned(),
            method,
            compressed: compressed as usize,
            local: local as usize,
        });
        at += 46 + n as usize + extra as usize + comment as usize;
    }
    out
}

/// A file's contents, stored or deflated (up to `limit` bytes inflated);
/// None for other methods.
fn zip_read(zip: &[u8], file: &ZipEntry, limit: u64) -> Option<Vec<u8>> {
    if u32_at(zip, file.local)? != 0x0403_4b50 {
        return None;
    }
    let n = u16_at(zip, file.local + 26)? as usize;
    let extra = u16_at(zip, file.local + 28)? as usize;
    let start = file.local + 30 + n + extra;
    let data = zip.get(start..start.checked_add(file.compressed)?)?;
    match file.method {
        0 => Some(data.to_vec()),
        8 => {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(data)
                .take(limit)
                .read_to_end(&mut out)
                .ok()?;
            Some(out)
        }
        _ => None,
    }
}

/// The text of an office document's XML: what is between its tags, with
/// paragraphs, rows and cells apart.
fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or(name);
        let closing = tag.starts_with('/');
        match local {
            // Paragraphs, headings, shared strings, rows: a line each.
            "p" | "h" | "si" | "tr" | "row" | "br" | "line-break" if closing || tag.ends_with('/') => {
                out.push('\n')
            }
            "tab" | "tc" | "table-cell" | "s" if closing || tag.ends_with('/') => out.push(' '),
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(rest);
    decode_entities(&out)
}

// PDFs.

/// The text a PDF shows, as far as can be read without laying it out:
/// the strings its pages' content streams draw, decoded by the fonts'
/// ToUnicode maps where there are any, as Latin-1 otherwise.
fn pdf_text(pdf: &[u8]) -> String {
    let streams = pdf_streams(pdf);
    let mut map = CMap::default();
    for s in &streams {
        if find(s, b"begincmap").is_some() {
            map.read(s);
        }
    }
    let mut out = String::new();
    for s in &streams {
        if find(s, b"begincmap").is_some() {
            continue;
        }
        content_text(s, &map, &mut out, MAX_PART_TEXT);
        if out.len() >= MAX_PART_TEXT {
            break;
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The PDF's streams, inflated where they're FlateDecode; images and
/// streams in other encodings are left out.
fn pdf_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut total = 0u64;
    while out.len() < MAX_PDF_STREAMS
        && total < MAX_DOCUMENT_INFLATED
        && let Some(i) = find(&pdf[at..], b"stream")
    {
        let start_kw = at + i;
        at = start_kw + 6;
        // "endstream" contains "stream".
        if start_kw >= 3 && &pdf[start_kw - 3..start_kw] == b"end" {
            continue;
        }
        let mut data_start = at;
        if pdf.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if pdf.get(data_start) == Some(&b'\n') {
            data_start += 1;
        } else if data_start == at {
            // Not the keyword (it's followed by an end of line).
            continue;
        }
        let Some(len) = find(&pdf[data_start..], b"endstream") else {
            break;
        };
        let data = &pdf[data_start..data_start + len];
        at = data_start + len + 9;
        // The stream's dictionary: back to its object's start.
        let dict_from = start_kw.saturating_sub(2048);
        let head = &pdf[dict_from..start_kw];
        let head = match head.windows(3).rposition(|w| w == b"obj") {
            Some(o) => &head[o..],
            None => head,
        };
        if find(head, b"/Image").is_some() || find(head, b"/FontFile").is_some() {
            continue;
        }
        let flate = find(head, b"/FlateDecode").is_some() || find(head, b"/Fl ").is_some();
        let other = [&b"/DCTDecode"[..], b"/JPXDecode", b"/LZWDecode", b"/CCITTFax", b"/JBIG2"]
            .iter()
            .any(|f| find(head, f).is_some());
        if other {
            continue;
        }
        let bytes = if flate {
            let mut inflated = Vec::new();
            let read = flate2::read::ZlibDecoder::new(data)
                .take(MAX_INFLATED.min(MAX_DOCUMENT_INFLATED - total))
                .read_to_end(&mut inflated);
            // A damaged stream often still gives most of its text.
            if read.is_err() && inflated.is_empty() {
                continue;
            }
            inflated
        } else {
            let left = (MAX_DOCUMENT_INFLATED - total).min(MAX_INFLATED) as usize;
            data[..data.len().min(left)].to_vec()
        };
        total += bytes.len() as u64;
        out.push(bytes);
    }
    out
}

/// What fonts' ToUnicode maps say codes stand for, all fonts' together:
/// one-byte and two-byte codes apart.
#[derive(Default)]
struct CMap {
    one: std::collections::HashMap<u32, String>,
    two: std::collections::HashMap<u32, String>,
}

impl CMap {
    fn read(&mut self, s: &[u8]) {
        let text = String::from_utf8_lossy(s);
        let hexes = |line: &str| -> Vec<(u32, usize, String)> {
            let mut v = Vec::new();
            let mut rest = line;
            while let Some(a) = rest.find('<') {
                let Some(b) = rest[a..].find('>') else { break };
                let hex: String = rest[a + 1..a + b].chars().filter(|c| !c.is_whitespace()).collect();
                if let Ok(n) = u32::from_str_radix(hex.get(..hex.len().min(8)).unwrap_or(""), 16) {
                    v.push((n, hex.len() / 2, hex.clone()));
                }
                rest = &rest[a + b + 1..];
            }
            v
        };
        let utf16 = |hex: &str| -> String {
            let units: Vec<u16> = (0..hex.len() / 4)
                .filter_map(|i| u16::from_str_radix(&hex[i * 4..i * 4 + 4], 16).ok())
                .collect();
            String::from_utf16_lossy(&units)
        };
        let mut section = "";
        for line in text.lines() {
            let line = line.trim();
            if line.ends_with("beginbfchar") {
                section = "char";
                continue;
            }
            if line.ends_with("beginbfrange") {
                section = "range";
                continue;
            }
            if line.starts_with("endbf") {
                section = "";
                continue;
            }
            let h = hexes(line);
            if self.one.len() + self.two.len() >= MAX_CMAP_ENTRIES {
                break;
            }
            match (section, h.as_slice()) {
                (_, [.., (_, _, dst)]) if dst.len() > MAX_CMAP_HEX => {}
                ("char", [(code, len, _), (_, _, dst)]) => {
                    self.put(*code, *len, utf16(dst));
                }
                ("range", [(lo, len, _), (hi, _, _), (_, _, dst)]) if !line.contains('[') => {
                    let Ok(first) = u32::from_str_radix(&dst[dst.len().saturating_sub(4)..], 16) else {
                        continue;
                    };
                    let prefix = utf16(&dst[..dst.len().saturating_sub(4)]);
                    let room = MAX_CMAP_ENTRIES.saturating_sub(self.one.len() + self.two.len());
                    for (i, code) in (*lo..=*hi).take(room).enumerate() {
                        let c = char::from_u32(first + i as u32).map(String::from).unwrap_or_default();
                        self.put(code, *len, format!("{prefix}{c}"));
                    }
                }
                ("range", [(lo, len, _), (_, _, _), rest @ ..]) => {
                    // An array of destinations, one per code.
                    let room = MAX_CMAP_ENTRIES.saturating_sub(self.one.len() + self.two.len());
                    for (i, (_, _, dst)) in rest.iter().take(room).enumerate() {
                        if dst.len() > MAX_CMAP_HEX {
                            continue;
                        }
                        self.put(lo.wrapping_add(i as u32), *len, utf16(dst));
                    }
                }
                _ => {}
            }
        }
    }
    fn put(&mut self, code: u32, len: usize, text: String) {
        if len >= 2 {
            self.two.insert(code, text);
        } else {
            self.one.insert(code, text);
        }
    }
    /// A drawn string's text, added to `out` until it holds `limit` bytes:
    /// a code may stand for dozens of bytes of text, so a stream of one
    /// code repeated stands for many times its size.
    fn decode(&self, bytes: &[u8], out: &mut String, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if bytes.starts_with(&[0xFE, 0xFF]) {
            let room = limit - out.len();
            let units: Vec<u16> = bytes[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .take(room)
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            out.push_str(&String::from_utf16_lossy(&units));
            return;
        }
        if !self.two.is_empty() && bytes.len().is_multiple_of(2) && !bytes.is_empty() {
            let codes: Vec<u32> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u32::from(c[0]) << 8 | u32::from(c[1]))
                .collect();
            let known = codes.iter().filter(|c| self.two.contains_key(c)).count();
            if known * 2 >= codes.len() {
                for c in codes {
                    if out.len() >= limit {
                        return;
                    }
                    if let Some(t) = self.two.get(&c) {
                        out.push_str(t);
                    }
                }
                return;
            }
        }
        for &b in bytes {
            if out.len() >= limit {
                return;
            }
            match self.one.get(&u32::from(b)) {
                Some(t) => out.push_str(t),
                None if b >= 0x20 || b == b'\t' => out.push(char::from(b)),
                None => {}
            }
        }
    }
}

/// Adds the text a content stream draws (Tj, TJ, ' and ") to `out`,
/// stopping once `out` holds `limit` bytes.
fn content_text(s: &[u8], map: &CMap, out: &mut String, limit: usize) {
    let mut i = 0;
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut numbers: Vec<f32> = Vec::new();
    let mut drew = false;
    let mut line = String::new();
    while i < s.len() {
        let c = s[i];
        match c {
            b'(' => {
                let (bytes, next) = literal(s, i + 1);
                if pending.len() < MAX_PENDING {
                    pending.push(bytes);
                }
                i = next;
                continue;
            }
            b'<' if s.get(i + 1) == Some(&b'<') => i += 2,
            b'<' => {
                let end = s[i..].iter().position(|&b| b == b'>').map_or(s.len(), |p| i + p);
                let hex: Vec<u8> = s[i + 1..end].iter().copied().filter(u8::is_ascii_hexdigit).collect();
                let bytes = hex
                    .chunks(2)
                    .filter_map(|h| {
                        let h = if h.len() == 1 { [h[0], b'0'] } else { [h[0], h[1]] };
                        u8::from_str_radix(std::str::from_utf8(&h).ok()?, 16).ok()
                    })
                    .collect();
                if pending.len() < MAX_PENDING {
                    pending.push(bytes);
                }
                i = end + 1;
                continue;
            }
            b'[' => {
                // A TJ array: strings, and spacing between them.
                let mut j = i + 1;
                let mut parts: Vec<Vec<u8>> = Vec::new();
                while j < s.len() && s[j] != b']' {
                    if parts.len() >= MAX_PENDING {
                        parts.pop();
                    }
                    match s[j] {
                        b'(' => {
                            let (bytes, next) = literal(s, j + 1);
                            parts.push(bytes);
                            j = next;
                        }
                        b'<' => {
                            let end = s[j..].iter().position(|&b| b == b'>').map_or(s.len(), |p| j + p);
                            let hex: Vec<u8> =
                                s[j + 1..end].iter().copied().filter(u8::is_ascii_hexdigit).collect();
                            parts.push(
                                hex.chunks(2)
                                    .filter_map(|h| {
                                        let h = if h.len() == 1 { [h[0], b'0'] } else { [h[0], h[1]] };
                                        u8::from_str_radix(std::str::from_utf8(&h).ok()?, 16).ok()
                                    })
                                    .collect(),
                            );
                            j = end + 1;
                        }
                        b'-' | b'0'..=b'9' | b'.' => {
                            let end = s[j..]
                                .iter()
                                .position(|b| !matches!(b, b'-' | b'0'..=b'9' | b'.'))
                                .map_or(s.len(), |p| j + p);
                            let n: f32 = std::str::from_utf8(&s[j..end]).ok().and_then(|t| t.parse().ok()).unwrap_or(0.);
                            // A wide gap is a space between words.
                            if n < -200. {
                                parts.push(b" ".to_vec());
                            }
                            j = end.max(j + 1);
                        }
                        _ => j += 1,
                    }
                }
                pending = parts;
                i = j + 1;
                continue;
            }
            b'%' => {
                // A comment, to the end of the line.
                i = s[i..].iter().position(|&b| b == b'\n' || b == b'\r').map_or(s.len(), |p| i + p);
                continue;
            }
            b'-' | b'0'..=b'9' | b'.' => {
                let end = s[i..]
                    .iter()
                    .position(|b| !matches!(b, b'-' | b'0'..=b'9' | b'.'))
                    .map_or(s.len(), |p| i + p);
                if let Some(n) = std::str::from_utf8(&s[i..end]).ok().and_then(|t| t.parse().ok()) {
                    // Only the last few are read (Td's).
                    if numbers.len() >= MAX_PENDING {
                        numbers.clear();
                    }
                    numbers.push(n);
                }
                i = end.max(i + 1);
                continue;
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'\'' | b'"' | b'*' => {
                let end = s[i..]
                    .iter()
                    .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'*' | b'\'' | b'"')))
                    .map_or(s.len(), |p| i + p)
                    .max(i + 1);
                let op = &s[i..end];
                match op {
                    b"Tj" | b"TJ" | b"'" | b"\"" => {
                        if matches!(op, b"'" | b"\"") {
                            newline(&mut line, out);
                        }
                        // What the line may still hold: `limit` less the
                        // text already out.
                        let room = limit.saturating_sub(out.len());
                        for p in pending.drain(..) {
                            if p == b" " {
                                line.push(' ');
                            } else {
                                map.decode(&p, &mut line, room);
                            }
                            if line.len() >= room {
                                break;
                            }
                        }
                        drew = true;
                        if line.len() >= room {
                            newline(&mut line, out);
                            return;
                        }
                    }
                    b"T*" => newline(&mut line, out),
                    b"Td" | b"TD" => {
                        // Moving down starts a line; moving along, a word.
                        let ty = numbers.last().copied().unwrap_or(0.);
                        if ty.abs() > 0.1 {
                            newline(&mut line, out);
                        } else if drew && !line.ends_with(' ') {
                            line.push(' ');
                        }
                    }
                    b"Tm" | b"ET" => newline(&mut line, out),
                    _ => {}
                }
                pending.clear();
                numbers.clear();
                i = end;
                continue;
            }
            _ => i += 1,
        }
    }
    newline(&mut line, out);
}

fn newline(line: &mut String, out: &mut String) {
    let text = line.trim();
    if !text.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(text);
    }
    line.clear();
}

/// A literal string's bytes, from just past its `(`; and where it ends.
fn literal(s: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut depth = 1;
    while i < s.len() {
        let c = s[i];
        i += 1;
        match c {
            b'\\' => {
                let Some(&e) = s.get(i) else { break };
                i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'0'..=b'7' => {
                        let mut n = u32::from(e - b'0');
                        for _ in 0..2 {
                            match s.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    n = n * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(n as u8);
                    }
                    b'\r' | b'\n' => {
                        if e == b'\r' && s.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    other => out.push(other),
                }
            }
            b'(' => {
                depth += 1;
                out.push(c);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    (out, i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// A ZIP archive of `files`, deflated.
    fn zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, text) in files {
            let data = deflate(text.as_bytes());
            let local = out.len() as u32;
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend([20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((text.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend(&data);
            central.extend(0x0201_4b50u32.to_le_bytes());
            central.extend([20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((text.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend(local.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let offset = out.len() as u32;
        out.extend(&central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0, 0, 0, 0]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    fn base64(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn with_attachment(name: &str, mime: &str, data: &[u8]) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nSubject: Files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\nContent-Type: {mime}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--b--\r\n",
            base64(data)
        )
        .into_bytes()
    }

    #[test]
    fn text_and_html_attachments() {
        let text = extract(&with_attachment("notes.txt", "text/plain", b"Quarterly figures attached"));
        assert!(text.contains("notes.txt") && text.contains("Quarterly figures"), "{text}");
        assert!(!text.contains("See attached"), "the message's own text is indexed apart");
        let html = extract(&with_attachment("page.html", "text/html", b"<p>Hello <b>world</b></p>"));
        assert!(html.contains("Hello world"), "{html}");
    }

    #[test]
    fn office_documents() {
        let docx = zip(&[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", "<w:document><w:body><w:p><w:r><w:t>Tender &amp; bid</w:t></w:r></w:p><w:p><w:r><w:t>Deadline</w:t></w:r></w:p></w:body></w:document>"),
        ]);
        let text = extract(&with_attachment("bid.docx", "application/octet-stream", &docx));
        assert!(text.contains("Tender & bid\nDeadline"), "{text}");
        let pptx = zip(&[
            ("ppt/slides/slide10.xml", "<p:sld><a:p><a:t>Ten</a:t></a:p></p:sld>"),
            ("ppt/slides/slide2.xml", "<p:sld><a:p><a:t>Two</a:t></a:p></p:sld>"),
        ]);
        let text = extract(&with_attachment("deck.pptx", "application/octet-stream", &pptx));
        assert!(text.find("Two") < text.find("Ten"), "{text}");
        let odt = zip(&[("content.xml", "<office:text><text:p>Minutes of the meeting</text:p></office:text>")]);
        assert!(extract(&with_attachment("m.odt", "application/vnd.oasis.opendocument.text", &odt)).contains("Minutes of the meeting"));
    }

    #[test]
    fn pdf_text_is_read() {
        let content = b"BT /F1 12 Tf 72 712 Td (Invoice number) Tj 0 -14 Td [(Total) -300 (due)] TJ ET";
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(content).unwrap();
        let stream = z.finish().unwrap();
        let mut pdf = b"%PDF-1.4\n4 0 obj\n<< /Length ".to_vec();
        pdf.extend(stream.len().to_string().as_bytes());
        pdf.extend(b" /Filter /FlateDecode >>\nstream\n");
        pdf.extend(&stream);
        pdf.extend(b"\nendstream\nendobj\n%%EOF\n");
        let text = extract(&with_attachment("invoice.pdf", "application/pdf", &pdf));
        assert!(text.contains("Invoice number\nTotal due"), "{text}");
        // Codes mapped by a font's ToUnicode map.
        let mut map = CMap::default();
        map.read(b"begincmap\n2 beginbfchar\n<0003> <0048>\n<0004> <0069>\nendbfchar\n1 beginbfrange\n<0010> <0012> <0061>\nendbfrange\nendcmap");
        let mut out = String::new();
        content_text(b"BT <00030004> Tj 10 0 Td [<0010>-400<00110012>] TJ ET", &map, &mut out, MAX_PART_TEXT);
        assert_eq!(out, "Hi a bc");
    }

    #[test]
    fn attached_emails_and_unreadable_files() {
        let inner = b"From: b@example.com\r\nSubject: Original plan\r\n\r\nMeet at noon.";
        let text = extract(&with_attachment("fwd.eml", "message/rfc822", inner));
        assert!(text.contains("Original plan") && text.contains("Meet at noon"), "{text}");
        let image = extract(&with_attachment("photo.jpg", "image/jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]));
        assert_eq!(image, "photo.jpg");
    }

    /// A ToUnicode map's ranges name at most `MAX_CMAP_ENTRIES` codes in
    /// all, however many lines ask for more, and long texts are left out.
    #[test]
    fn font_maps_are_bounded() {
        let mut cmap = String::from("begincmap\nbeginbfrange\n");
        for k in 0..64u32 {
            cmap.push_str(&format!("<{:08x}><{:08x}><0041>\n", k * 65_536, k * 65_536 + 65_535));
        }
        cmap.push_str(&format!("<0000><0010><{}>\nendbfrange\n", "0041".repeat(1000)));
        let mut map = CMap::default();
        map.read(cmap.as_bytes());
        assert_eq!(map.one.len() + map.two.len(), MAX_CMAP_ENTRIES);
        let mut long = CMap::default();
        long.read(format!("beginbfchar\n<01> <{}>\nendbfchar\n", "0041".repeat(1000)).as_bytes());
        assert!(long.one.is_empty());
    }

    /// One code standing for a long text, drawn millions of times, adds at
    /// most `MAX_PART_TEXT` of it rather than dozens of times the stream.
    #[test]
    fn pdf_text_expansion_is_bounded() {
        let mut map = CMap::default();
        map.read(format!("beginbfchar\n<01> <{}>\nendbfchar\n", "4E2D".repeat(16)).as_bytes());
        assert!(!map.one.is_empty());
        let mut content = b"BT <".to_vec();
        content.extend(b"01".repeat(4 << 20));
        content.extend(b"> Tj ET");
        let mut out = String::new();
        content_text(&content, &map, &mut out, MAX_PART_TEXT);
        assert!(out.len() <= MAX_PART_TEXT + 64, "{}", out.len());
        // And strings piling up before any operator are kept to a few.
        let mut out = String::new();
        content_text(&b"(a)".repeat(1 << 20), &map, &mut out, MAX_PART_TEXT);
        assert!(out.is_empty());
    }

    /// Zip entries sharing one file's data are read once, and a document
    /// inflates to `MAX_DOCUMENT_INFLATED` at most.
    #[test]
    fn office_zip_bombs_are_bounded() {
        let filler = "<a/>".repeat(1 << 20);
        let mut z = zip(&[("ppt/slides/slide1.xml", &filler)]);
        // The directory again, with many entries all naming the first file.
        let end = z.len() - 22;
        let offset = u32::from_le_bytes(z[end + 16..end + 20].try_into().unwrap()) as usize;
        let entry = z[offset..end].to_vec();
        z.truncate(offset);
        let central_at = z.len() as u32;
        let count = 3000u16;
        for i in 0..count {
            let mut e = entry.clone();
            let name = format!("ppt/slides/slide{i:04}.xml");
            e.truncate(46);
            e[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            e.extend(name.as_bytes());
            z.extend(e);
        }
        let central_len = z.len() as u32 - central_at;
        z.extend(0x0605_4b50u32.to_le_bytes());
        z.extend([0, 0, 0, 0]);
        z.extend(count.to_le_bytes());
        z.extend(count.to_le_bytes());
        z.extend(central_len.to_le_bytes());
        z.extend(central_at.to_le_bytes());
        z.extend(0u16.to_le_bytes());
        let started = std::time::Instant::now();
        assert_eq!(zip_entries(&z).len(), count as usize);
        let _ = office_text(&z, POWERPOINT);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }
}
