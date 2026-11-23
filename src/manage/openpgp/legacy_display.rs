//! Legacy Display Elements (RFC 9788 §2.1.2): the obscured or removed
//! header fields of an encrypted message repeated at the top of its main
//! text, for apps that decrypt but don't know Header Protection. RFC 9788
//! makes them optional and expects them to be phased out (§8.1).
use super::{from_latin1, latin1};
use base64::Engine as _;

/// A rewritten MIME part: `part`'s header fields other than Content-Type
/// and Content-Transfer-Encoding, then `content_type` and `content` as
/// base64 (which any charset survives), CRLF throughout.
fn rewritten(part_head: &str, content_type: &str, content: &[u8]) -> Vec<u8> {
    let mut out = String::new();
    let mut keep = true;
    for line in part_head.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if bare.is_empty() {
            continue;
        }
        if !bare.starts_with([' ', '\t']) {
            let name = bare.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
            keep = name != "content-type" && name != "content-transfer-encoding";
        }
        if keep {
            out.push_str(bare);
            out.push_str("\r\n");
        }
    }
    out.push_str(&format!("Content-Type: {content_type}\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
    let encoded = base64::engine::general_purpose::STANDARD.encode(content);
    let lines: Vec<&str> = encoded
        .as_bytes()
        .chunks(76)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    out.push_str(&lines.join("\r\n"));
    from_latin1(&out)
}

/// A Content-Type value: `mime` and `params` (quoted), with `drop` left out.
fn content_type(mime: &str, params: &std::collections::BTreeMap<String, String>, drop: &str) -> String {
    let mut out = mime.to_owned();
    for (name, value) in params.iter().filter(|(n, _)| *n != drop) {
        let value = value.replace(['\\', '"'], "");
        out.push_str(&format!("; {name}=\"{value}\""));
    }
    out
}

/// The byte range of `part` (a subpart of `root`, as mailparse slices
/// them) within `root`.
fn range_in(root: &[u8], part: &[u8]) -> Option<std::ops::Range<usize>> {
    let start = (part.as_ptr() as usize).checked_sub(root.as_ptr() as usize)?;
    let end = start.checked_add(part.len())?;
    (end <= root.len()).then_some(start..end)
}

/// `root` with each `(range, bytes)` put in place of its range.
fn spliced(root: &[u8], mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)>) -> Vec<u8> {
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = root.to_vec();
    for (range, bytes) in edits {
        out.splice(range, bytes);
    }
    out
}

/// The header section of a part's raw bytes, as text.
fn head_text(raw: &[u8]) -> String {
    let text = latin1(raw);
    match (text.find("\r\n\r\n"), text.find("\n\n")) {
        (Some(c), l) if l.is_none_or(|l| c <= l) => text[..c + 2].to_owned(),
        (_, Some(l)) => text[..l + 1].to_owned(),
        _ => text,
    }
}

// --- Reading ----------------------------------------------------------------

/// RFC 9788 §4.5.3.2: text/plain without its leading lines up to and
/// including the first blank one. Left whole when there is none: a
/// malformed element isn't a reason to hide the text.
fn strip_plain(content: &[u8]) -> Option<Vec<u8>> {
    let lf = memchr::memmem::find(content, b"\n\n").map(|at| at + 2);
    let crlf = memchr::memmem::find(content, b"\r\n\r\n").map(|at| at + 4);
    let at = match (crlf, lf) {
        (Some(c), Some(l)) => c.min(l),
        (c, l) => c.or(l)?,
    };
    Some(content[at..].to_vec())
}

/// RFC 9788 §4.5.3.3: text/html without each <div> whose class is
/// header-protection-legacy-display, with what it holds.
fn strip_html(content: &[u8]) -> Option<Vec<u8>> {
    let text = latin1(content);
    let lower = text.to_ascii_lowercase();
    let mut cut: Vec<std::ops::Range<usize>> = Vec::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find("<div").map(|i| i + at) {
        let tag_end = lower[found..].find('>').map(|i| i + found + 1)?;
        let tag = &lower[found..tag_end];
        let is_element = tag.contains("header-protection-legacy-display")
            && tag[4..].starts_with([' ', '\t', '\r', '\n', '>', '/']);
        if !is_element {
            at = found + 4;
            continue;
        }
        // Its end: the </div> that closes it, nested ones counted.
        let mut depth = 1;
        let mut scan = tag_end;
        let end = loop {
            let open = lower[scan..].find("<div").map(|i| i + scan);
            let close = lower[scan..].find("</div").map(|i| i + scan)?;
            match open {
                Some(o) if o < close => {
                    depth += 1;
                    scan = o + 4;
                }
                _ => {
                    depth -= 1;
                    let after = lower[close..].find('>').map_or(lower.len(), |i| i + close + 1);
                    if depth == 0 {
                        break after;
                    }
                    scan = after;
                }
            }
        };
        cut.push(found..end);
        at = end;
    }
    if cut.is_empty() {
        return None;
    }
    let mut out = text;
    for range in cut.into_iter().rev() {
        out.replace_range(range, "");
    }
    Some(from_latin1(&out))
}

/// RFC 9788 §4.5.3: `payload` (the root of a decrypted message's
/// Cryptographic Payload, which has Header Protection) with the Legacy
/// Display Elements of its text/plain and text/html parts marked
/// hp-legacy-display="1" left out, and that parameter dropped. Other
/// parts, and parts not so marked, are as they were.
pub(super) fn without_elements(payload: &[u8]) -> Vec<u8> {
    let Ok(root) = mailparse::parse_mail(payload) else {
        return payload.to_vec();
    };
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&root, &mut leaves);
    let mut edits = Vec::new();
    for leaf in leaves {
        let marked = leaf.ctype.params.get("hp-legacy-display").is_some_and(|v| v.trim() == "1");
        let mime = leaf.ctype.mimetype.to_ascii_lowercase();
        if !marked || (mime != "text/plain" && mime != "text/html") {
            continue;
        }
        let Ok(content) = leaf.get_body_raw() else { continue };
        let stripped = if mime == "text/plain" {
            strip_plain(&content)
        } else {
            strip_html(&content)
        };
        let (Some(stripped), Some(range)) = (stripped, range_in(payload, leaf.raw_bytes)) else {
            continue;
        };
        let ct = content_type(&mime, &leaf.ctype.params, "hp-legacy-display");
        edits.push((range, rewritten(&head_text(leaf.raw_bytes), &ct, &stripped)));
    }
    spliced(payload, edits)
}

// --- Sending ----------------------------------------------------------------

/// RFC 9787 §7.1: the Main Body Parts of `part`: the text leaves reached by
/// following the first child of every multipart but
/// multipart/alternative, all of whose children count; never an
/// attachment.
fn main_body_parts<'a>(part: &'a mailparse::ParsedMail<'a>, out: &mut Vec<&'a mailparse::ParsedMail<'a>>) {
    let mime = part.ctype.mimetype.to_ascii_lowercase();
    if mime == "multipart/alternative" {
        for child in &part.subparts {
            main_body_parts(child, out);
        }
    } else if mime.starts_with("multipart/") {
        if let Some(first) = part.subparts.first() {
            main_body_parts(first, out);
        }
    } else if (mime == "text/plain" || mime == "text/html")
        && part.get_content_disposition().disposition != mailparse::DispositionType::Attachment
    {
        out.push(part);
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// RFC 9788 §5.2.1 step 2: `part` (the body part to be encrypted, CRLF)
/// with a Legacy Display Element of `fields` ((name, decoded value): the
/// User-Facing fields the outside obscures or leaves out) at the top of
/// each Main Body Part, marked hp-legacy-display="1" (§5.2.2, §5.2.3).
/// A part whose charset can't hold the values as they are (UTF-8, or
/// ASCII values in US-ASCII) is left as it was.
pub(super) fn with_elements(part: &[u8], fields: &[(String, String)]) -> Vec<u8> {
    if fields.is_empty() {
        return part.to_vec();
    }
    let Ok(root) = mailparse::parse_mail(part) else {
        return part.to_vec();
    };
    let lines: String = fields
        .iter()
        .map(|(name, value)| format!("{name}: {}\r\n", value.trim()))
        .collect();
    let ascii = lines.is_ascii();
    let mut targets = Vec::new();
    main_body_parts(&root, &mut targets);
    let mut edits = Vec::new();
    for leaf in targets {
        let charset = leaf.ctype.charset.to_ascii_lowercase();
        if !(charset == "utf-8" || charset == "utf8" || (ascii && charset == "us-ascii")) {
            continue;
        }
        let (Ok(content), Some(range)) = (leaf.get_body_raw(), range_in(part, leaf.raw_bytes)) else {
            continue;
        };
        let mime = leaf.ctype.mimetype.to_ascii_lowercase();
        let with = if mime == "text/plain" {
            // RFC 9788 §5.2.2: the fields, one per line, then a blank line.
            let mut out = format!("{lines}\r\n").into_bytes();
            out.extend_from_slice(&content);
            out
        } else {
            // RFC 9788 §5.2.3.1: in a <div> as the first child of <body>.
            let element = format!(
                "<div class=\"header-protection-legacy-display\"><pre>{}</pre></div>",
                escape_html(lines.trim_end())
            );
            let text = String::from_utf8_lossy(&content).into_owned();
            let lower = text.to_ascii_lowercase();
            let at = lower
                .find("<body")
                .and_then(|b| lower[b..].find('>').map(|e| b + e + 1))
                .unwrap_or(0);
            let mut out = text;
            out.insert_str(at, &element);
            out.into_bytes()
        };
        let mut params = leaf.ctype.params.clone();
        params.insert("hp-legacy-display".into(), "1".into());
        let ct = content_type(&mime, &params, "");
        edits.push((range, rewritten(&head_text(leaf.raw_bytes), &ct, &with)));
    }
    spliced(part, edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(raw: &[u8]) -> String {
        mailparse::parse_mail(raw).unwrap().get_body().unwrap()
    }

    /// RFC 9788 §5.2.2 and §4.5.3.2: a text/plain root gets the element
    /// and the marker, and reading takes both away again.
    #[test]
    fn plain_text_elements_go_on_and_come_off() {
        let part = b"Content-Type: text/plain; charset=utf-8; format=flowed\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nMeet at noon. =E2=98=80\r\n";
        let fields = [("Subject".to_owned(), "Pläne".to_owned()), ("Reply-To".to_owned(), "a@b.example".to_owned())];
        let with = with_elements(part, &fields);
        let parsed = mailparse::parse_mail(&with).unwrap();
        assert_eq!(parsed.ctype.params.get("hp-legacy-display").map(String::as_str), Some("1"));
        assert_eq!(parsed.ctype.params.get("format").map(String::as_str), Some("flowed"));
        assert_eq!(body_of(&with), "Subject: Pläne\r\nReply-To: a@b.example\r\n\r\nMeet at noon. ☀\r\n");
        let without = without_elements(&with);
        let parsed = mailparse::parse_mail(&without).unwrap();
        assert!(parsed.ctype.params.get("hp-legacy-display").is_none());
        assert_eq!(body_of(&without), "Meet at noon. ☀\r\n");
        // No fields to show: nothing changes.
        assert_eq!(with_elements(part, &[]), part);
    }

    /// RFC 9788 §5.2.3 and §5.2.4: in multipart/mixed, only the first
    /// child's alternatives get one (the HTML in its body, first), never
    /// the attachment; reading takes them away and keeps the rest.
    #[test]
    fn html_and_alternative_parts_get_elements_but_attachments_never() {
        let part = b"Content-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\nContent-Type: multipart/alternative; boundary=\"a\"\r\n\r\n--a\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHi\r\n--a\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body class=\"x\"><p>Hi</p></body></html>\r\n--a--\r\n--m\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Disposition: attachment; filename=\"notes.txt\"\r\n\r\nNotes\r\n--m--\r\n";
        let fields = [("Subject".to_owned(), "<Plans> & more".to_owned())];
        let with = with_elements(part, &fields);
        let parsed = mailparse::parse_mail(&with).unwrap();
        let alternative = &parsed.subparts[0];
        assert_eq!(alternative.subparts[0].get_body().unwrap(), "Subject: <Plans> & more\r\n\r\nHi");
        assert_eq!(
            alternative.subparts[1].get_body().unwrap(),
            "<html><body class=\"x\"><div class=\"header-protection-legacy-display\"><pre>Subject: &lt;Plans&gt; &amp; more</pre></div><p>Hi</p></body></html>"
        );
        for p in &alternative.subparts {
            assert_eq!(p.ctype.params.get("hp-legacy-display").map(String::as_str), Some("1"));
        }
        assert!(parsed.subparts[1].ctype.params.get("hp-legacy-display").is_none());
        assert_eq!(parsed.subparts[1].get_body().unwrap(), "Notes");
        let without = without_elements(&with);
        let parsed = mailparse::parse_mail(&without).unwrap();
        assert_eq!(parsed.subparts[0].subparts[0].get_body().unwrap(), "Hi");
        assert_eq!(parsed.subparts[0].subparts[1].get_body().unwrap(), "<html><body class=\"x\"><p>Hi</p></body></html>");
        assert_eq!(parsed.subparts[1].get_body().unwrap(), "Notes");
    }

    /// RFC 9788 §4.5.3.1: only marked text parts lose anything; a nested
    /// <div> inside the element goes with it; one without a blank line
    /// is left whole.
    #[test]
    fn only_marked_elements_are_removed() {
        let unmarked = b"Content-Type: text/plain\r\n\r\nSubject: mine\r\n\r\nReal text\r\n";
        assert_eq!(without_elements(unmarked), unmarked);
        let no_blank = b"Content-Type: text/plain; hp-legacy-display=\"1\"\r\n\r\nAll of it\r\n";
        assert_eq!(without_elements(no_blank), no_blank);
        let html = b"Content-Type: text/html; hp-legacy-display=\"1\"\r\n\r\n<body><div class=\"header-protection-legacy-display\"><div>Subject: x</div></div><div class=\"keep\">Body</div></body>";
        assert_eq!(body_of(&without_elements(html)), "<body><div class=\"keep\">Body</div></body>");
    }
}
