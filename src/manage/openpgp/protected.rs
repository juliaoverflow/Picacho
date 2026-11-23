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

/// The header section of `part` (text), and where its body starts.
fn head_of(part: &str) -> (&str, usize) {
    if part.starts_with("\r\n") {
        return ("", 2);
    }
    if part.starts_with('\n') {
        return ("", 1);
    }
    match (part.find("\r\n\r\n"), part.find("\n\n")) {
        (Some(c), l) if l.is_none_or(|l| c <= l) => (&part[..c + 2], c + 4),
        (_, Some(l)) => (&part[..l + 1], l + 2),
        _ => (part, part.len()),
    }
}

/// RFC 9788 §4.1: whether a payload root's Content-Type says it has
/// Header Protection (hp="clear" or hp="cipher"), or the older draft's
/// protected-headers="v1" (RFC 9788 Appendix F.3).
fn has_header_protection(head: &str) -> bool {
    let Some(field) = fields(head).into_iter().find(|f| f.name == "content-type") else {
        return false;
    };
    let value = field.lines.split_once(':').map_or("", |(_, v)| v);
    let ct = mailparse::parse_content_type(&value.replace("\r\n", ""));
    let hp = ct.params.get("hp").map(|v| v.to_ascii_lowercase());
    matches!(hp.as_deref(), Some("clear" | "cipher"))
        || ct.params.get("protected-headers").is_some_and(|v| v.eq_ignore_ascii_case("v1"))
}

/// RFC 9788 §4.1: whether `payload` (the root of a Cryptographic
/// Payload) has an hp parameter ("clear" or "cipher") on its Content-Type.
pub(super) fn has_hp_parameter(payload: &[u8]) -> bool {
    let text = latin1(payload);
    let (head, _) = head_of(&text);
    let Some(field) = fields(head).into_iter().find(|f| f.name == "content-type") else {
        return false;
    };
    let value = field.lines.split_once(':').map_or("", |(_, v)| v);
    let ct = mailparse::parse_content_type(&value.replace("\r\n", ""));
    ct.params
        .get("hp")
        .is_some_and(|v| v.eq_ignore_ascii_case("clear") || v.eq_ignore_ascii_case("cipher"))
}

/// RFC 9788 §4.10.1: the message an RFC8551HP payload wraps, when
/// `payload` (the root of a Cryptographic Payload) is one: a single
/// message/rfc822 part, the message it holds not itself a Cryptographic
/// Layer, and no hp parameter on either. That message's header fields are
/// the protected ones and its body is what is shown (§4.10.2).
pub(super) fn unwrapped(payload: &[u8]) -> Option<Vec<u8>> {
    let parsed = mailparse::parse_mail(payload).ok()?;
    if !parsed.ctype.mimetype.eq_ignore_ascii_case("message/rfc822") || parsed.ctype.params.contains_key("hp") {
        return None;
    }
    let message = parsed.get_body_raw().ok()?;
    let wrapped = mailparse::parse_mail(&message).ok()?;
    let kind = wrapped.ctype.mimetype.to_ascii_lowercase();
    let layer = matches!(
        kind.as_str(),
        "multipart/signed" | "multipart/encrypted" | "application/pkcs7-mime" | "application/x-pkcs7-mime"
    );
    (!layer && !wrapped.ctype.params.contains_key("hp")).then_some(message)
}

/// The protected From of `payload` (the root of a Cryptographic
/// Payload), as written, and its address: from its own header section
/// with Header Protection, or from the message it wraps (RFC 9788
/// §4.10). `None` without Header Protection or a From.
fn protected_from(payload: &[u8]) -> Option<(String, String)> {
    let wrapped = unwrapped(payload);
    let text = latin1(wrapped.as_deref().unwrap_or(payload));
    let (head, _) = head_of(&text);
    if wrapped.is_none() && !has_header_protection(head) {
        return None;
    }
    let field = fields(head).into_iter().find(|f| f.name == "from")?;
    let value = field.lines.split_once(':')?.1.replace("\r\n", "").trim().to_owned();
    let address = first_address(head, "from")?;
    let decoded = mailparse::parse_header(format!("From: {value}").as_bytes())
        .map_or(value, |(h, _)| h.get_value());
    Some((decoded, address))
}

/// RFC 9788 §4.4.2: the warning for a message whose protected From is for
/// another address than its outer From (§4.4.1.1) and that has no valid
/// signature by a key bound to the protected address (§4.4.1.2), which
/// `bound` tells for an address. `outer` is the outer header section.
pub(crate) fn from_mismatch(
    outer: &[u8],
    payload: &[u8],
    bound: impl Fn(&str) -> bool,
) -> Option<super::FromMismatch> {
    let (protected, inner_address) = protected_from(payload)?;
    let outer_text = latin1(outer);
    let outer_address = first_address(&outer_text, "from");
    if same_address(outer_address.as_deref(), Some(&inner_address)) || bound(&inner_address) {
        return None;
    }
    use mailparse::MailHeaderMap as _;
    let outer_from = mailparse::parse_headers(outer)
        .ok()
        .and_then(|(h, _)| h.get_first_value("From"))
        .unwrap_or_default();
    Some(super::FromMismatch { outer: outer_from, protected })
}

/// RFC 9788 §5.2.1 step 2 and RFC 9787 §1.1.2: the User-Facing fields of
/// `outer` that `compose` obscures or takes off the outside, (name,
/// decoded value), for a Legacy Display Element. Only an encrypted
/// message with its subject hidden has any.
pub(super) fn legacy_display_fields(outer: &[u8], encrypted: bool, hide_subject: bool) -> Vec<(String, String)> {
    let Ok((headers, _)) = mailparse::parse_headers(outer) else {
        return Vec::new();
    };
    use mailparse::MailHeaderMap as _;
    if !encrypted || !hide_subject || headers.get_first_value("Subject").is_none() {
        return Vec::new();
    }
    // Of `HIDDEN` and the Subject, the User-Facing ones.
    ["Subject", "Reply-To"]
        .into_iter()
        .filter_map(|name| headers.get_first_value(name).map(|v| (name.to_owned(), v)))
        .collect()
}

/// The address of the first `name` field in `head`, if it has one.
fn first_address(head: &str, name: &str) -> Option<String> {
    let field = fields(head).into_iter().find(|f| f.name == name)?;
    let value = field.lines.split_once(':')?.1.replace("\r\n", "");
    let address = crate::sync::mime::receipt_address(&value);
    (!address.is_empty()).then_some(address)
}

/// RFC 9788 §4.4.5: whether two addr-specs match: the domains as ASCII
/// (IDNA A-labels) compared case-insensitively, then the local parts,
/// case-insensitively.
fn same_address(a: Option<&str>, b: Option<&str>) -> bool {
    let ascii = |address: &str| -> Option<(String, String)> {
        let (local, domain) = address.trim().rsplit_once('@')?;
        let domain = idna::domain_to_ascii(domain).ok()?;
        Some((local.to_lowercase(), domain.to_ascii_lowercase()))
    };
    match (a.and_then(ascii), b.and_then(ascii)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The message to show for an opened one: `outer` (its outer header
/// fields, CRLF) and `payload` (the root of its Cryptographic Payload).
/// RFC 9788 §4 and §4.5: with Header Protection, a field the payload
/// carries is shown as it is there, in place of any outer one of its
/// name, and the HP-Outer fields, which record what went outside, aren't
/// shown. Without, the outer fields as they are, then the payload.
/// `inferred`: the payload has Header Protection by its structure (an
/// RFC8551HP one unwrapped, RFC 9788 §4.10.2) rather than a parameter.
pub(super) fn readable(outer: &[u8], payload: &[u8], inferred: bool) -> Vec<u8> {
    let payload_text = latin1(payload);
    let (head, body_at) = head_of(&payload_text);
    if !inferred && !has_header_protection(head) {
        let mut out = outer.to_vec();
        out.extend_from_slice(payload);
        return out;
    }
    let mut inner = fields(head);
    // RFC 9788 §4.4.3: a protected From whose address isn't the outer
    // one's isn't shown in its place: the outer From is what the
    // sender's server vouched for.
    let outer_from = first_address(&latin1(outer), "from");
    let inner_from = first_address(head, "from");
    if outer_from.is_some() && !same_address(outer_from.as_deref(), inner_from.as_deref()) {
        inner.retain(|f| f.name != "from");
    }
    let carried = |name: &str| {
        !name.starts_with("content-") && name != "mime-version" && inner.iter().any(|f| f.name == name)
    };
    let mut out: String = fields(&latin1(outer))
        .into_iter()
        .filter(|f| !carried(&f.name))
        .map(|f| f.lines)
        .collect();
    let outer_mime_version = fields(&out).iter().any(|f| f.name == "mime-version");
    for field in inner
        .iter()
        .filter(|f| f.name != "hp-outer" && !(outer_mime_version && f.name == "mime-version"))
    {
        out.push_str(&field.lines);
    }
    out.push_str("\r\n");
    out.push_str(&payload_text[body_at.min(payload_text.len())..]);
    from_latin1(&out)
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

    /// A keyring with `who`'s secret key, and one with only its public key.
    fn rings(who: &str) -> (super::super::Keyring, super::super::Keyring) {
        use pgp::composed::SignedPublicKey;
        let key = super::super::test_key(who, "");
        let mut own = super::super::Keyring::default();
        own.import(&key.to_armored_string(Default::default()).unwrap(), "").unwrap();
        let mut theirs = super::super::Keyring::default();
        let public = SignedPublicKey::from(key).to_armored_string(Default::default()).unwrap();
        theirs.import(&public, "").unwrap();
        (own, theirs)
    }

    const MESSAGE: &str = "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nMeet at noon.\r\n";

    /// RFC 9788 §5.2.1 step 4 with RFC 3156 §5: signed only, the outer
    /// headers are as they were, the signed part carries the inner ones
    /// with hp="clear", and the signature covers them: it verifies, and a
    /// changed inner Subject breaks it.
    #[test]
    fn signed_only_mail_signs_its_inner_headers() {
        use super::super::{Protect, Signature, open, protect, signed_parts};
        let (anas, bos) = rings("Ana <ana@example.org>");
        let how = Protect { sign: true, ..Default::default() };
        let sent = protect(MESSAGE.as_bytes(), "ana@example.org", &[], how, &anas).unwrap();
        let text = String::from_utf8(sent.clone()).unwrap();
        let outer = text.split("Content-Type: multipart/signed").next().unwrap();
        assert_eq!(
            outer,
            "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\n"
        );
        let (signed, _) = signed_parts(&sent).unwrap();
        let signed = String::from_utf8(signed).unwrap();
        let parsed = mailparse::parse_mail(signed.as_bytes()).unwrap();
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("clear"));
        assert_eq!(values(&signed, "Subject"), ["Plans"]);
        assert_eq!(values(&signed, "From"), ["Ana <ana@example.org>"]);
        assert!(values(&signed, "HP-Outer").is_empty(), "{signed}");
        let (readable, verdict) = open(&sent, &bos).unwrap();
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        // Only the inner Subject changed: the outer one is as sent.
        let at = text.find("multipart/signed").unwrap();
        let forged = format!("{}{}", &text[..at], text[at..].replacen("Subject: Plans", "Subject: Pay now", 1));
        assert_ne!(forged, text);
        assert_eq!(open(forged.as_bytes(), &bos).unwrap().1.signature, Signature::Bad);
    }

    /// RFC 9788 §5.2.1 step 5: signed and encrypted, the inner headers
    /// with hp="cipher" and HP-Outer are inside the signature, inside the
    /// encryption; the outside is unchanged without hide-subject.
    #[test]
    fn signed_and_encrypted_mail_carries_hp_cipher_inside_the_signature() {
        use super::super::{Protect, Signature, open, protect};
        let (mut anas, mut bos) = rings("Ana <ana@example.org>");
        let (bos_own, bos_public) = rings("Bo <bo@example.se>");
        anas.public.extend(bos_public.public);
        bos.secret.extend(bos_own.secret);
        let how = Protect { sign: true, encrypt: true, ..Default::default() };
        let sent = protect(MESSAGE.as_bytes(), "ana@example.org", &["bo@example.se".into()], how, &anas).unwrap();
        let text = String::from_utf8_lossy(&sent);
        assert!(text.contains("\r\nSubject: Plans\r\n") && !text.contains("Meet at noon"), "{text}");
        let (readable, verdict) = open(&sent, &bos).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        let readable = String::from_utf8_lossy(&readable);
        assert!(readable.contains("hp=\"cipher\""), "{readable}");
        // RFC 9788 §4.2: HP-Outer is for the reader, not shown; each field
        // is shown once.
        assert!(!readable.contains("HP-Outer"), "{readable}");
        assert_eq!(values(&readable, "Subject"), ["Plans"], "{readable}");
        assert_eq!(values(&readable, "From").len(), 1, "{readable}");
        // The signed part inside the encryption carries the HP-Outer list.
        let (armored, _) = crate::manage::crypto::encrypted_part(&sent).unwrap();
        let (message, _) = pgp::composed::Message::from_string(&armored).unwrap();
        let key = &bos.secret_keys()[0];
        let mut opened = message.decrypt(&pgp::types::Password::empty(), key).unwrap();
        let payload = String::from_utf8(opened.as_data_vec().unwrap()).unwrap();
        assert!(payload.contains("HP-Outer: Subject: Plans\r\n"), "{payload}");
        assert!(payload.contains("HP-Outer: From: Ana <ana@example.org>\r\n"), "{payload}");
    }

    /// RFC 9788 §5.2.1 step 2 and §4.5.3: with `legacy_display` and the
    /// subject hidden, the decrypted text starts with the real Subject for
    /// apps that don't know Header Protection; Picacho shows the text
    /// without it. Off (the default), no element is added.
    #[test]
    fn legacy_display_elements_are_sent_on_request_and_never_shown() {
        use super::super::{Protect, open, protect};
        let (anas, _) = rings("Ana <ana@example.org>");
        let how = Protect { encrypt: true, subject: true, legacy_display: true, ..Default::default() };
        let sent = protect(MESSAGE.as_bytes(), "ana@example.org", &[], how, &anas).unwrap();
        let (armored, _) = crate::manage::crypto::encrypted_part(&sent).unwrap();
        let (message, _) = pgp::composed::Message::from_string(&armored).unwrap();
        let key = &anas.secret_keys()[0];
        let payload = message.decrypt(&pgp::types::Password::empty(), key).unwrap().as_data_vec().unwrap();
        let parsed = mailparse::parse_mail(&payload).unwrap();
        assert_eq!(parsed.ctype.params.get("hp-legacy-display").map(String::as_str), Some("1"));
        assert_eq!(parsed.get_body().unwrap(), "Subject: Plans\r\n\r\nMeet at noon.");
        let (readable, _) = open(&sent, &anas).unwrap();
        let parsed = mailparse::parse_mail(&readable).unwrap();
        assert_eq!(parsed.get_body().unwrap(), "Meet at noon.");
        assert!(parsed.ctype.params.get("hp-legacy-display").is_none());
        assert_eq!(values(&String::from_utf8_lossy(&readable), "Subject"), ["Plans"]);
        // Off: the text is as written.
        let how = Protect { encrypt: true, subject: true, ..Default::default() };
        let sent = protect(MESSAGE.as_bytes(), "ana@example.org", &[], how, &anas).unwrap();
        let (armored, _) = crate::manage::crypto::encrypted_part(&sent).unwrap();
        let (message, _) = pgp::composed::Message::from_string(&armored).unwrap();
        let payload = message.decrypt(&pgp::types::Password::empty(), key).unwrap().as_data_vec().unwrap();
        assert!(!String::from_utf8_lossy(&payload).contains("hp-legacy-display"));
    }

    /// `content` signed by `key` as RFC 3156 §5 multipart/signed (headers
    /// and body, no outer fields).
    fn signed_mime(content: &str, key: &pgp::composed::SignedSecretKey) -> String {
        use pgp::composed::DetachedSignature;
        let sub = key.secret_subkeys.iter().find(|s| s.signatures.iter().any(|g| g.key_flags().sign())).unwrap();
        let signature = DetachedSignature::sign_binary_data(
            rand08::thread_rng(),
            &sub.key,
            &pgp::types::Password::empty(),
            pgp::crypto::hash::HashAlgorithm::Sha256,
            content.as_bytes(),
        )
        .unwrap()
        .to_armored_string(Default::default())
        .unwrap()
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
        format!("Content-Type: multipart/signed; micalg=pgp-sha256; protocol=\"application/pgp-signature\"; boundary=\"s\"\r\n\r\n--s\r\n{content}\r\n--s\r\nContent-Type: application/pgp-signature\r\n\r\n{signature}\r\n--s--\r\n")
    }

    /// `payload` encrypted to `key` as RFC 3156 §4 multipart/encrypted,
    /// after the fields `outer`.
    fn encrypted_mime(outer: &str, payload: &str, key: &pgp::composed::SignedSecretKey) -> String {
        let public = pgp::composed::SignedPublicKey::from(key.clone());
        let sub = super::super::encryption_subkey(&public).unwrap();
        let mut builder = pgp::composed::MessageBuilder::from_bytes("", payload.as_bytes().to_vec())
            .seipd_v1(rand08::thread_rng(), pgp::crypto::sym::SymmetricKeyAlgorithm::AES256);
        builder.encrypt_to_key(rand08::thread_rng(), &sub.key).unwrap();
        let armored = builder
            .to_armored_string(rand08::thread_rng(), Default::default())
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\n', "\r\n");
        format!("{outer}MIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"e\"\r\n\r\n--e\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n\r\n--e\r\nContent-Type: application/octet-stream\r\n\r\n{armored}\r\n--e--\r\n")
    }

    const WRAPPED: &str = "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Hidden plans\r\nMessage-ID: <1@example.org>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.\r\n";
    const WRAPPED_OUTER: &str = "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: ...\r\nMessage-ID: <1@example.org>\r\n";

    /// RFC 9788 §4.10: a payload that is one message/rfc822 with no hp
    /// parameter (RFC 8551's wrapped form) is read as the message it
    /// holds: its Subject, not the outer one, and its body, not the
    /// wrapper. Encrypted, signed and encrypted, and signed only; the
    /// signature over the wrapper counts.
    #[test]
    fn wrapped_rfc822_payloads_are_read_as_their_message() {
        use super::super::{Signature, open};
        let ana = super::super::test_key("Ana <ana@example.org>", "");
        let mut ring = super::super::Keyring::default();
        ring.import(&ana.to_armored_string(Default::default()).unwrap(), "").unwrap();
        let payload = format!("Content-Type: message/rfc822\r\n\r\n{WRAPPED}");
        let signed = signed_mime(&payload, &ana);
        for (raw, signature) in [
            (encrypted_mime(WRAPPED_OUTER, &payload, &ana), Signature::None),
            (encrypted_mime(WRAPPED_OUTER, &signed, &ana), Signature::Good("Ana <ana@example.org>".into())),
            (format!("{WRAPPED_OUTER}MIME-Version: 1.0\r\n{signed}"), Signature::Good("Ana <ana@example.org>".into())),
        ] {
            let (readable, verdict) = open(raw.as_bytes(), &ring).unwrap();
            let readable = String::from_utf8(readable).unwrap();
            assert_eq!(verdict.signature, signature, "{readable}");
            assert_eq!(values(&readable, "Subject"), ["Hidden plans"], "{readable}");
            assert_eq!(values(&readable, "From"), ["Ana <ana@example.org>"], "{readable}");
            let parsed = mailparse::parse_mail(readable.as_bytes()).unwrap();
            assert_eq!(parsed.ctype.mimetype, "text/plain", "{readable}");
            assert_eq!(parsed.get_body().unwrap(), "Meet at noon.\r\n");
        }
        // A signed message inside is a layer of its own, and one with an
        // hp parameter is RFC 9788's: neither is unwrapped.
        let layer = format!("Content-Type: message/rfc822\r\n\r\n{signed}");
        assert!(unwrapped(layer.as_bytes()).is_none());
        let marked = format!("Content-Type: message/rfc822; hp=\"cipher\"\r\n\r\n{WRAPPED}");
        assert!(unwrapped(marked.as_bytes()).is_none());
        assert_eq!(unwrapped(payload.as_bytes()).as_deref(), Some(WRAPPED.as_bytes()));
    }

    /// RFC 9788 §4.4.2: a protected From for another address than the
    /// outer one warns, showing both, unless a valid signature by a key
    /// for the protected address vouches for it; a matching address
    /// (§4.4.5: case and IDNA aside) never warns.
    #[test]
    fn protected_from_mismatch_warns_without_a_bound_signature() {
        use super::super::{FromMismatch, Keyring, Signature, Verdict, open, test_key};
        let ana = test_key("Ana <ana@example.org>", "");
        let boss = test_key("Boss <boss@bank.example>", "");
        let mut ring = Keyring::default();
        ring.import(&ana.to_armored_string(Default::default()).unwrap(), "").unwrap();
        ring.import(
            &pgp::composed::SignedPublicKey::from(boss.clone()).to_armored_string(Default::default()).unwrap(),
            "",
        )
        .unwrap();
        let outer = "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: ...\r\n";
        let payload = |from: &str| {
            format!("From: {from}\r\nSubject: Pay now\r\nContent-Type: text/plain; hp=\"cipher\"\r\n\r\nPay.\r\n")
        };
        // Unsigned, the protected From names the bank: a warning with both.
        let forged = encrypted_mime(outer, &payload("Boss <boss@bank.example>"), &ana);
        let (readable, verdict) = open(forged.as_bytes(), &ring).unwrap();
        let warning = FromMismatch { outer: "Ana <ana@example.org>".into(), protected: "Boss <boss@bank.example>".into() };
        assert_eq!(verdict.from_mismatch.as_ref(), Some(&warning));
        let (line, good) = verdict.signature_line().unwrap();
        assert!(!good && line.contains("boss@bank.example") && line.contains("ana@example.org"), "{line}");
        // RFC 9788 §4.4.3: the outer From is the one shown.
        assert_eq!(values(&String::from_utf8_lossy(&readable), "From"), ["Ana <ana@example.org>"]);
        // Signed by Ana's key (bound to the outer address, not the
        // protected one): still a warning, beside the signature.
        let signed = encrypted_mime(outer, &signed_mime(&payload("Boss <boss@bank.example>"), &ana), &ana);
        let verdict = open(signed.as_bytes(), &ring).unwrap().1;
        assert_eq!(verdict.from_mismatch, Some(warning.clone()));
        let (line, good) = verdict.signature_line().unwrap();
        assert!(!good && line.starts_with("Signed by Ana") && line.contains("Warning"), "{line}");
        // Signed by the bank's own key: the signature vouches for it.
        let vouched = encrypted_mime(outer, &signed_mime(&payload("Boss <boss@bank.example>"), &boss), &ana);
        assert_eq!(open(vouched.as_bytes(), &ring).unwrap().1.from_mismatch, None);
        // Signed only, the same rules.
        let clear = format!("{outer}MIME-Version: 1.0\r\n{}", signed_mime(&payload("Boss <boss@bank.example>"), &ana));
        assert_eq!(open(clear.as_bytes(), &ring).unwrap().1.from_mismatch, Some(warning));
        // The same address, written otherwise: no warning, and a plain
        // verdict reads as before.
        let same = encrypted_mime(outer, &payload("Ana Lima <ANA@Example.ORG>"), &ana);
        assert_eq!(open(same.as_bytes(), &ring).unwrap().1.from_mismatch, None);
        assert_eq!(Verdict::default().signature_line(), None);
        let good = Verdict { signature: Signature::Good("Ana".into()), ..Default::default() };
        assert_eq!(good.signature_line(), Some(("Signed by Ana".into(), true)));
    }

    /// Brief Q5: a payload without an hp parameter is never stripped, even
    /// with a part marked hp-legacy-display="1" (RFC 9788 §4.1: the
    /// marker means something only with Header Protection).
    #[test]
    fn legacy_display_is_never_stripped_from_unprotected_payloads() {
        let marked = b"Content-Type: text/plain; hp-legacy-display=\"1\"\r\n\r\nSubject: mine\r\n\r\nReal text\r\n";
        assert!(!has_hp_parameter(marked));
        assert_eq!(super::super::shown_payload(marked), marked);
        let protected = b"Content-Type: text/plain; hp=\"cipher\"; hp-legacy-display=\"1\"\r\n\r\nSubject: mine\r\n\r\nReal text\r\n";
        assert!(has_hp_parameter(protected));
        let shown = super::super::shown_payload(protected);
        assert_eq!(mailparse::parse_mail(&shown).unwrap().get_body().unwrap(), "Real text\r\n");
        // The older draft's marker alone isn't an hp parameter.
        let draft = b"Content-Type: text/plain; protected-headers=\"v1\"; hp-legacy-display=\"1\"\r\n\r\nSubject: mine\r\n\r\nReal text\r\n";
        assert_eq!(super::super::shown_payload(draft), draft);
    }

    /// RFC 9788 §4 and §4.5: with Header Protection, the payload's fields
    /// are shown in place of the outer ones, HP-Outer isn't, and outer
    /// fields the payload doesn't carry (added in transit) stay. Without
    /// it, nothing of the payload's header replaces anything outside.
    #[test]
    fn readable_messages_show_the_protected_headers() {
        let outer = b"From: Ana <ana@example.org>\r\nSubject: ...\r\nReceived: by mx\r\nMIME-Version: 1.0\r\n";
        let payload = b"Subject: Plans\r\nFrom: Ana Lima <ANA@EXAMPLE.ORG>\r\nHP-Outer: Subject: ...\r\nContent-Type: text/plain; hp=\"cipher\"\r\n\r\nHi\r\n";
        let shown = String::from_utf8(readable(outer, payload, false)).unwrap();
        assert_eq!(values(&shown, "Subject"), ["Plans"], "{shown}");
        assert_eq!(values(&shown, "From"), ["Ana Lima <ANA@EXAMPLE.ORG>"], "{shown}");
        assert_eq!(values(&shown, "Received"), ["by mx"], "{shown}");
        assert!(values(&shown, "HP-Outer").is_empty(), "{shown}");
        assert!(shown.ends_with("\r\n\r\nHi\r\n"), "{shown}");
        // A protected From for another address isn't shown in place of the
        // outer one (RFC 9788 §4.4.3).
        let other = String::from_utf8_lossy(payload).replace("ANA@EXAMPLE.ORG", "boss@bank.example");
        let shown = String::from_utf8(readable(outer, other.as_bytes(), false)).unwrap();
        assert_eq!(values(&shown, "From"), ["Ana <ana@example.org>"], "{shown}");
        assert_eq!(values(&shown, "Subject"), ["Plans"], "{shown}");
        // No hp parameter: a Subject in the payload's header is just a
        // header there, and the outer one is what's shown.
        let plain = b"Subject: Not me\r\nContent-Type: text/plain\r\n\r\nHi\r\n";
        let shown = String::from_utf8(readable(outer, plain, false)).unwrap();
        assert_eq!(values(&shown, "Subject")[0], "...", "{shown}");
        assert!(same_address(Some("ana@bücher.example"), Some("ANA@xn--bcher-kva.example")));
        assert!(!same_address(Some("ana@example.org"), Some("ana@example.org.evil")));
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
