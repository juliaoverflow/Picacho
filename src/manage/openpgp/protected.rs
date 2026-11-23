//! Header Protection (RFC 9788): the inner header section a message
//! carries at the root of its Cryptographic Payload when sent signed or
//! encrypted.
use super::{HIDDEN_SUBJECT, from_latin1, latin1};

/// RFC 9788 §2 and §5.2.1: the header fields copied onto the root of the
/// Cryptographic Payload, so the signature covers them and the encryption
/// carries them. Bcc is never among them (RFC 9788 §11.4).
pub(super) const INNER: [&str; 9] = [
    "from",
    "to",
    "cc",
    "reply-to",
    "subject",
    "date",
    "message-id",
    "in-reply-to",
    "references",
];

/// Fields taken off the outside with a hidden subject, beside the Subject
/// itself (which becomes `HIDDEN_SUBJECT`): what says which conversation
/// the mail is part of. Keywords go inside then too (RFC 9788 §3.2.1).
const HIDDEN: [&str; 4] = ["in-reply-to", "references", "reply-to", "keywords"];

/// One header field as written: its name, lowercase, and its lines (any
/// folded ones too), each ended by CRLF.
struct Field {
    name: String,
    lines: String,
}

/// The fields of a header section (text, one char per byte).
fn fields(head: &str) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    for line in head.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if bare.is_empty() {
            continue;
        }
        let line = format!("{bare}\r\n");
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.lines.push_str(&line);
            }
            continue;
        }
        let name = bare.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        out.push(Field { name, lines: line });
    }
    out
}

/// RFC 9788 §2.2.1: `HP-Outer: <field-name>: <value>`, with the value's
/// folded lines kept as they were.
fn hp_outer(field: &Field) -> String {
    let (name, value) = field.lines.split_once(':').unwrap_or((&field.lines, "\r\n"));
    let value = value.trim_start_matches([' ', '\t']);
    format!("HP-Outer: {}: {value}", name.trim())
}

/// `head` (a part's Content-* fields) with `params` added at the end of
/// its Content-Type field, after any folded lines. A part without one is
/// text/plain (RFC 2045 §5.2), and says so now.
fn mark_content_type(head: &str, params: &str) -> String {
    let mut out = String::new();
    let mut in_type = false;
    let mut marked = false;
    let mark = |out: &mut String| {
        while out.ends_with(['\r', '\n']) {
            out.pop();
        }
        out.push_str(params);
        out.push_str("\r\n");
    };
    for line in head.split_inclusive('\n') {
        let continuation = line.starts_with([' ', '\t']);
        if in_type && !continuation {
            mark(&mut out);
            in_type = false;
            marked = true;
        }
        if !continuation && !marked && line.to_ascii_lowercase().starts_with("content-type:") {
            in_type = true;
        }
        out.push_str(line);
    }
    if in_type {
        mark(&mut out);
        marked = true;
    }
    if !marked {
        out.insert_str(0, &format!("Content-Type: text/plain; charset=us-ascii{params}\r\n"));
    }
    out
}

/// RFC 9788 §5.2.1 (Compose): the part to be signed or encrypted and the
/// outer header section, with Header Protection. `part` is the message's
/// body part (its Content-* fields, a blank line, its body; CRLF) and
/// `outer` the message's other fields.
///
/// - The fields in `INNER` are copied onto the part, Bcc never.
/// - The part's Content-Type gets hp="clear" when signed only and
///   hp="cipher" when `encrypted` (RFC 9788 §2.1.1), and the older draft's
///   protected-headers="v1" for readers of that.
/// - Encrypted, with `hide_subject` and a Subject: the Subject outside
///   becomes `HIDDEN_SUBJECT` and the `HIDDEN` fields are taken off the
///   outside. Otherwise the outside is unchanged.
/// - Encrypted: one HP-Outer field on the part for each field left outside
///   (RFC 9788 §2.2), so a reader can tell which were kept confidential.
pub(super) fn compose(
    part: &[u8],
    outer: &[u8],
    encrypted: bool,
    hide_subject: bool,
) -> (Vec<u8>, Vec<u8>) {
    let outer_text = latin1(outer);
    let outer_fields = fields(&outer_text);
    let hide = encrypted && hide_subject && outer_fields.iter().any(|f| f.name == "subject");
    let mut inner: String = outer_fields
        .iter()
        .filter(|f| INNER.contains(&f.name.as_str()) || (hide && f.name == "keywords"))
        .map(|f| f.lines.as_str())
        .collect();
    let new_outer = if hide {
        outer_fields
            .iter()
            .filter_map(|f| match f.name.as_str() {
                "subject" => Some(format!("Subject: {HIDDEN_SUBJECT}\r\n")),
                name if HIDDEN.contains(&name) => None,
                _ => Some(f.lines.clone()),
            })
            .collect()
    } else {
        outer_text
    };
    if encrypted {
        // RFC 9788 §2.2: only Non-Structural fields, and never a Bcc (a
        // draft's outer section may keep one).
        for field in fields(&new_outer) {
            let name = field.name.as_str();
            if name != "bcc" && name != "mime-version" && !name.starts_with("content-") {
                inner.push_str(&hp_outer(&field));
            }
        }
    }
    let part_text = latin1(part);
    let (head, body) = if part_text.starts_with("\r\n") {
        ("", part_text.as_str())
    } else {
        match part_text.find("\r\n\r\n") {
            Some(at) => part_text.split_at(at + 2),
            None => (part_text.as_str(), ""),
        }
    };
    let hp = if encrypted { "cipher" } else { "clear" };
    let head = mark_content_type(head, &format!("; protected-headers=\"v1\"; hp=\"{hp}\""));
    let mut new_part = inner;
    new_part.push_str(&head);
    new_part.push_str(body);
    (from_latin1(&new_part), from_latin1(&new_outer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailparse::MailHeaderMap as _;

    const OUTER: &str = "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nCc: Cy <cy@example.net>\r\nBcc: secret@example.com\r\nReply-To: Ana <ana@home.example>\r\nSubject: =?utf-8?q?Pl=C3=A4ne?=\r\n =?utf-8?q?_f=C3=BCr_morgen?=\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <1@example.org>\r\nIn-Reply-To: <0@example.se>\r\nReferences: <0@example.se>\r\nX-Priority: 1 (Highest)\r\n";
    const PART: &str = "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nMeet at noon.";

    fn composed(encrypted: bool, hide: bool) -> (String, String) {
        let (part, outer) = compose(PART.as_bytes(), OUTER.as_bytes(), encrypted, hide);
        (String::from_utf8(part).unwrap(), String::from_utf8(outer).unwrap())
    }

    /// The (name, value) pairs of `headers` named `name`, as parsed.
    fn values(raw: &str, name: &str) -> Vec<String> {
        let (headers, _) = mailparse::parse_headers(raw.as_bytes()).unwrap();
        headers.get_all_values(name)
    }

    /// RFC 9788 §5.2.1 step 4: signed only, the inner set is copied onto
    /// the part with hp="clear", no HP-Outer, no Bcc, and the outside is
    /// unchanged.
    #[test]
    fn signed_only_payloads_carry_the_inner_headers_with_hp_clear() {
        let (part, outer) = composed(false, false);
        assert_eq!(outer, OUTER);
        let parsed = mailparse::parse_mail(part.as_bytes()).unwrap();
        assert_eq!(parsed.ctype.mimetype, "text/plain");
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("clear"));
        assert_eq!(parsed.ctype.params.get("protected-headers").map(String::as_str), Some("v1"));
        assert_eq!(parsed.ctype.params.get("charset").map(String::as_str), Some("utf-8"));
        for (name, value) in [
            ("From", "Ana <ana@example.org>"),
            ("To", "Bo <bo@example.se>"),
            ("Cc", "Cy <cy@example.net>"),
            ("Reply-To", "Ana <ana@home.example>"),
            ("Subject", "Pläne für morgen"),
            ("Date", "Mon, 23 Nov 2026 10:00:00 +0000"),
            ("Message-ID", "<1@example.org>"),
            ("In-Reply-To", "<0@example.se>"),
            ("References", "<0@example.se>"),
        ] {
            assert_eq!(values(&part, name), [value], "{name} in {part}");
        }
        assert!(values(&part, "Bcc").is_empty(), "{part}");
        assert!(values(&part, "HP-Outer").is_empty(), "{part}");
        assert!(values(&part, "X-Priority").is_empty(), "only the inner set: {part}");
        assert_eq!(parsed.get_body().unwrap(), "Meet at noon.");
    }

    /// RFC 9788 §2.1.1 and §2.2: encrypted, hp="cipher", the outside as it
    /// was, and an HP-Outer for every field left outside but the Bcc.
    #[test]
    fn encrypted_payloads_list_every_outer_field_in_hp_outer() {
        let (part, outer) = composed(true, false);
        assert_eq!(outer, OUTER);
        let parsed = mailparse::parse_mail(part.as_bytes()).unwrap();
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("cipher"));
        assert_eq!(values(&part, "Subject"), ["Pläne für morgen"]);
        assert!(values(&part, "Bcc").is_empty(), "{part}");
        let hp_outer = values(&part, "HP-Outer");
        assert_eq!(
            hp_outer,
            [
                "From: Ana <ana@example.org>",
                "To: Bo <bo@example.se>",
                "Cc: Cy <cy@example.net>",
                "Reply-To: Ana <ana@home.example>",
                "Subject: Pläne für morgen",
                "Date: Mon, 23 Nov 2026 10:00:00 +0000",
                "Message-ID: <1@example.org>",
                "In-Reply-To: <0@example.se>",
                "References: <0@example.se>",
                "X-Priority: 1 (Highest)",
            ]
        );
        assert!(!part.contains("secret@example.com"), "{part}");
        // The folded Subject stays folded, inside and in its HP-Outer.
        assert!(part.contains("HP-Outer: Subject: =?utf-8?q?Pl=C3=A4ne?=\r\n =?utf-8?q?_f=C3=BCr_morgen?=\r\n"), "{part}");
    }

    /// RFC 9788 §3.2.1 and §5.2.1 step 5: with the subject hidden, the
    /// Subject outside is "..." and the thread fields are gone from the
    /// outside; HP-Outer says so, and the real ones are inside.
    #[test]
    fn hidden_subjects_are_obscured_outside_and_listed_in_hp_outer() {
        let (part, outer) = composed(true, true);
        assert_eq!(values(&outer, "Subject"), [HIDDEN_SUBJECT]);
        for name in ["In-Reply-To", "References", "Reply-To"] {
            assert!(values(&outer, name).is_empty(), "{name} outside: {outer}");
            assert_eq!(values(&part, name).len(), 1, "{name} inside: {part}");
        }
        assert_eq!(values(&outer, "Bcc"), ["secret@example.com"], "the envelope's business, not ours");
        assert_eq!(values(&part, "Subject"), ["Pläne für morgen"]);
        let hp_outer = values(&part, "HP-Outer");
        assert!(hp_outer.contains(&format!("Subject: {HIDDEN_SUBJECT}")), "{hp_outer:?}");
        assert!(!hp_outer.iter().any(|h| h.starts_with("In-Reply-To") || h.starts_with("References") || h.starts_with("Reply-To") || h.starts_with("Bcc")), "{hp_outer:?}");
        assert!(hp_outer.contains(&"From: Ana <ana@example.org>".to_owned()), "{hp_outer:?}");
        // Signed only, hiding asks for nothing: there is no encryption to
        // hide behind (RFC 9788 §5.2.1, the note after the procedure).
        let (_, outer) = composed(false, true);
        assert_eq!(outer, OUTER);
    }

    /// The parameters go at the end of a folded Content-Type, so a
    /// multipart's boundary stays its own; a part without a Content-Type
    /// is marked as the text/plain it is.
    #[test]
    fn hp_marks_folded_and_missing_content_types() {
        let multipart = "Content-Type: multipart/mixed;\r\n boundary=\"xx\"\r\n\r\n--xx\r\nContent-Type: text/plain\r\n\r\nHi\r\n--xx--\r\n";
        let (part, _) = compose(multipart.as_bytes(), OUTER.as_bytes(), true, false);
        let parsed = mailparse::parse_mail(&part).unwrap();
        assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        assert_eq!(parsed.ctype.params.get("boundary").map(String::as_str), Some("xx"));
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("cipher"));
        assert_eq!(parsed.subparts.len(), 1);
        assert!(parsed.subparts[0].ctype.params.get("hp").is_none(), "only on the root (RFC 9788 §5)");
        let (part, _) = compose(b"\r\nHi", OUTER.as_bytes(), false, false);
        let parsed = mailparse::parse_mail(&part).unwrap();
        assert_eq!(parsed.ctype.mimetype, "text/plain");
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("clear"));
        assert_eq!(parsed.get_body().unwrap(), "Hi");
    }
}
