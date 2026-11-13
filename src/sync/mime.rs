//! MIME decoding shared by downloaded mail and sent copies.
use crate::manage::model::*;
use anyhow::Result;
use mailparse::MailHeaderMap;
pub fn parse_message(
    raw: &[u8],
    account: &Account,
    id: &str,
    folder: Folder,
    remote: &str,
    uid: u32,
) -> Result<Mail> {
    let p = mailparse::parse_mail(raw)?;
    let header = |name: &str| p.headers.get_first_value(name).unwrap_or_default();
    let mixed = p.ctype.mimetype.eq_ignore_ascii_case("multipart/mixed");
    let from_header = header("From");
    let (sender, from) = mailparse::addrparse(&from_header)
        .ok()
        .and_then(|v| v.extract_single_info())
        .map(|s| (s.display_name.unwrap_or_else(|| s.addr.clone()), s.addr))
        .unwrap_or_else(|| (from_header.clone(), from_header));
    let mut body = String::new();
    let mut html = None;
    let mut attachments = Vec::new();
    let mut leaves = Vec::new();
    collect_leaves(&p, &mut leaves);
    for (index, part) in leaves.iter().enumerate() {
        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"));
        if filename.is_some() || disposition.disposition == mailparse::DispositionType::Attachment {
            attachments.push(Attachment {
                name: filename.cloned().unwrap_or_else(|| "attachment".into()),
                size: part.get_body_raw().map(|b| b.len()).unwrap_or_default(),
                mime: part.ctype.mimetype.clone(),
                part: index,
            });
        } else if part.ctype.mimetype == "text/html" && html.is_none() {
            html = part.get_body().ok();
        } else if part.ctype.mimetype == "text/plain" && body.is_empty() {
            body = part.get_body().unwrap_or_default();
        }
    }
    if body.is_empty()
        && let Some(html) = &html
    {
        body = html_to_text(html);
    }
    let snippet = snippet(&body);
    let date = mailparse::dateparse(&header("Date")).unwrap_or_default();
    let subject = header("Subject");
    let has_attachments = mixed || !attachments.is_empty();
    Ok(Mail {
        id: id.into(),
        account: account.id.clone(),
        folder,
        remote_folder: remote.into(),
        uid,
        from,
        sender,
        to: header("To"),
        cc: header("Cc"),
        subject: if subject.is_empty() {
            "(No subject)".into()
        } else {
            subject
        },
        snippet,
        date,
        unread: true,
        starred: false,
        size: raw.len() as u32,
        body: if body.is_empty() { None } else { Some(body) },
        html,
        attachments,
        message_id: header("Message-ID"),
        references: {
            let refs = header("References");
            if refs.trim().is_empty() {
                header("In-Reply-To")
            } else {
                refs
            }
        },
        has_attachments,
        gmail_id: 0,
        list_unsubscribe: header("List-Unsubscribe").trim().to_owned(),
        one_click: header("List-Unsubscribe-Post")
            .to_ascii_lowercase()
            .replace(' ', "")
            .contains("list-unsubscribe=one-click"),
        pending: false,
        reply_to: header("Reply-To"),
    })
}
pub(crate) fn collect_leaves<'a>(
    p: &'a mailparse::ParsedMail<'a>,
    out: &mut Vec<&'a mailparse::ParsedMail<'a>>,
) {
    if p.subparts.is_empty() {
        out.push(p);
    } else {
        for part in &p.subparts {
            collect_leaves(part, out);
        }
    }
}
/// Elements whose content is never text a reader sees.
const HIDDEN_ELEMENTS: [&str; 8] = [
    "script", "style", "head", "title", "xml", "noscript", "template", "svg",
];
/// Elements that end a line of text.
const BLOCK_ELEMENTS: [&str; 17] = [
    "br", "p", "div", "tr", "li", "h1", "h2", "h3", "h4", "h5", "h6", "table", "ul", "ol",
    "blockquote", "hr", "section",
];

pub(crate) fn html_to_text(html: &str) -> String {
    // Search, snippets, replies and the plain-text reader use this fallback.
    let mut text = String::new();
    // Open hidden elements (their names), innermost last.
    let mut hidden: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        if hidden.is_empty() {
            text.push_str(&rest[..start]);
        }
        rest = &rest[start..];
        // Comments, including Outlook's conditional ones (`<!--[if mso]>
        // <xml>…96…</xml><![endif]-->`), are skipped whole.
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        // A `<` that can't open a tag (as in `1 < 2`) is text.
        if !rest[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?') {
            if hidden.is_empty() {
                text.push('<');
            }
            rest = &rest[1..];
            continue;
        }
        let Some(end) = rest.find('>') else {
            // A stray `<` that opens no tag is text.
            if hidden.is_empty() {
                text.push_str(rest);
            }
            rest = "";
            break;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            // `<![endif]>`, `<!DOCTYPE …>` and the like.
            continue;
        }
        // Namespaced Office elements (`<o:p>`, `<v:shape>`) hold no text of their own.
        let base = name.rsplit(':').next().unwrap_or(&name).to_owned();
        if HIDDEN_ELEMENTS.contains(&base.as_str()) {
            if closing {
                if let Some(at) = hidden.iter().rposition(|h| *h == base) {
                    hidden.truncate(at);
                }
            } else if !tag.trim_end().ends_with('/') {
                hidden.push(base);
            }
            continue;
        }
        if hidden.is_empty() && BLOCK_ELEMENTS.contains(&base.as_str()) && (closing || base == "br" || base == "hr") {
            text.push('\n');
        } else if hidden.is_empty() && (base == "td" || base == "th") && closing {
            text.push(' ');
        }
    }
    if hidden.is_empty() {
        text.push_str(rest);
    }
    // Templates sometimes escape their padding twice (`&amp;#847;`), which
    // would leave `&#847;` showing; those are dropped too.
    tidy(&strip_invisible_references(&decode_entities(&text)))
}
/// Removes character references that stand for invisible characters.
fn strip_invisible_references(text: &str) -> String {
    if !text.contains("&#") && !text.contains("&zwnj") && !text.contains("&zwj") && !text.contains("&shy") {
        return text.to_owned();
    }
    decode(text, true)
}

/// Characters mail templates pad preview text with that show as nothing
/// (zero-width joiners, soft hyphens, the combining grapheme joiner…).
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{034f}'
            | '\u{061c}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{17b4}'
            | '\u{17b5}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{206a}'..='\u{206f}'
            | '\u{3164}'
            | '\u{feff}'
            | '\u{ffa0}'
    )
}

/// Drops invisible characters, turns other spaces into plain ones, trims
/// each line, and keeps at most one blank line in a row.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line: String = line
            .chars()
            .filter(|c| !invisible(*c))
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        let mut words = line.split(' ').filter(|w| !w.is_empty()).peekable();
        if words.peek().is_none() {
            blank += 1;
            if blank == 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank = 0;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        let joined = words.collect::<Vec<_>>().join(" ");
        out.push_str(&joined);
    }
    out.trim().to_string()
}

/// A preview of a message's text: its first words, without entities or
/// invisible characters (some plain-text parts carry `&zwnj;` literally).
pub(crate) fn snippet(body: &str) -> String {
    // Decoded twice: a preview is better off without padding a template
    // escaped twice (`&amp;#847;&amp;nbsp;`) than showing it.
    let text: String = decode_entities(&decode_entities(body))
        .chars()
        .filter(|c| !invisible(*c))
        .collect();
    text.split_whitespace().take(45).collect::<Vec<_>>().join(" ")
}

/// Decodes HTML character references: numeric ones, and the named ones
/// mail uses. Unknown names are left as written.
pub(crate) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    decode(text, false)
}
/// Decodes references; with `only_invisible`, only those for invisible
/// characters (which are then left out), keeping the rest as written.
fn decode(text: &str, only_invisible: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // Names are short; a `;` further on belongs to something else.
        let end = rest[1..]
            .char_indices()
            .take(32)
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '#'))
            .map(|(i, c)| (i + 1, c));
        let decoded = match end {
            Some((end, ';')) => entity(&rest[1..end]).map(|c| (c, end + 1)),
            // Numeric ones, and a few names, are often written without the `;`.
            Some((end, _)) => {
                let name = &rest[1..end];
                (name.starts_with('#') || matches!(name, "nbsp" | "amp" | "lt" | "gt" | "quot"))
                    .then(|| entity(name))
                    .flatten()
                    .map(|c| (c, end))
            }
            None => None,
        };
        let decoded = if only_invisible {
            decoded
                .filter(|(c, _)| c.chars().all(invisible))
                .map(|(_, len)| (String::new(), len))
        } else {
            decoded
        };
        match decoded {
            Some((c, len)) => {
                out.push_str(&c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
fn entity(name: &str) -> Option<String> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        // Windows-1252's characters, as browsers read them in 128–159.
        let code = match code {
            0x80 => 0x20ac,
            0x91 => 0x2018,
            0x92 => 0x2019,
            0x93 => 0x201c,
            0x94 => 0x201d,
            0x95 => 0x2022,
            0x96 => 0x2013,
            0x97 => 0x2014,
            0x85 => 0x2026,
            0x99 => 0x2122,
            c => c,
        };
        return char::from_u32(code)
            .filter(|c| *c != '\0')
            .map(String::from);
    }
    let c = match name {
        "nbsp" => '\u{a0}',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "zwnj" => '\u{200c}',
        "zwj" => '\u{200d}',
        "lrm" => '\u{200e}',
        "rlm" => '\u{200f}',
        "shy" => '\u{ad}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "hairsp" => '\u{200a}',
        "numsp" => '\u{2007}',
        "puncsp" => '\u{2008}',
        "NoBreak" => '\u{2060}',
        "ZeroWidthSpace" => '\u{200b}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "bull" => '•',
        "middot" => '·',
        "dagger" => '†',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "iexcl" => '¡',
        "iquest" => '¿',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "hearts" => '♥',
        "star" => '☆',
        "check" => '✓',
        "agrave" => 'à',
        "aacute" => 'á',
        "acirc" => 'â',
        "atilde" => 'ã',
        "auml" => 'ä',
        "aring" => 'å',
        "aelig" => 'æ',
        "ccedil" => 'ç',
        "egrave" => 'è',
        "eacute" => 'é',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "igrave" => 'ì',
        "iacute" => 'í',
        "icirc" => 'î',
        "iuml" => 'ï',
        "ntilde" => 'ñ',
        "ograve" => 'ò',
        "oacute" => 'ó',
        "ocirc" => 'ô',
        "otilde" => 'õ',
        "ouml" => 'ö',
        "oslash" => 'ø',
        "ugrave" => 'ù',
        "uacute" => 'ú',
        "ucirc" => 'û',
        "uuml" => 'ü',
        "yacute" => 'ý',
        "yuml" => 'ÿ',
        "szlig" => 'ß',
        "Agrave" => 'À',
        "Aacute" => 'Á',
        "Acirc" => 'Â',
        "Auml" => 'Ä',
        "Aring" => 'Å',
        "Ccedil" => 'Ç',
        "Egrave" => 'È',
        "Eacute" => 'É',
        "Ecirc" => 'Ê',
        "Iacute" => 'Í',
        "Ntilde" => 'Ñ',
        "Oacute" => 'Ó',
        "Ouml" => 'Ö',
        "Oslash" => 'Ø',
        "Uacute" => 'Ú',
        "Uuml" => 'Ü',
        _ => return None,
    };
    Some(c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_html_alternative_without_treating_html_attachments_as_body() {
        let raw = b"Content-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\nContent-Type: multipart/alternative; boundary=a\r\n\r\n--a\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlain alternative\r\n--a\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nPGI+UmljaCBhbHRlcm5hdGl2ZTwvYj4=\r\n--a--\r\n--m\r\nContent-Type: text/html; name=attachment.html\r\nContent-Disposition: attachment; filename=attachment.html\r\n\r\n<script>attachment</script>\r\n--m--\r\n";
        let a = Account::new(Provider::Imap);
        let m = parse_message(raw, &a, "id", Folder::Inbox, "INBOX", 1).unwrap();
        assert_eq!(m.body.as_deref(), Some("Plain alternative"));
        assert_eq!(m.html.as_deref(), Some("<b>Rich alternative</b>"));
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments[0].part, 2);
        assert!(!m.snippet.contains("<b>"));
        let html_only = parse_message(
            b"Content-Type: text/html; charset=utf-8\r\n\r\n<h1>HTML only</h1>",
            &a,
            "id",
            Folder::Inbox,
            "INBOX",
            1,
        )
        .unwrap();
        assert!(html_only.html.is_some());
        assert!(html_only.body.unwrap().contains("HTML only"));
        let mut old = serde_json::to_value(&m).unwrap();
        old.as_object_mut().unwrap().remove("html");
        assert!(serde_json::from_value::<Mail>(old).unwrap().html.is_none());
    }

    #[test]
    fn previews_leave_out_office_markup_entities_and_invisible_padding() {
        let html = "<html><head><title>Weekly</title><!--[if gte mso 9]><xml><o:OfficeDocumentSettings><o:PixelsPerInch>96</o:PixelsPerInch></o:OfficeDocumentSettings></xml><![endif]--><style>p{color:red}</style></head><body><div style=\"display:none\">Big news inside&nbsp;&zwnj;&nbsp;&zwnj;&#847;&zwnj;&#8203;</div><p>Caf&eacute; &amp; more&#8217;s here&hellip;</p><table><tr><td>A</td><td>B</td></tr></table></body></html>";
        let text = html_to_text(html);
        assert_eq!(text, "Big news inside\nCafé & more’s here…\nA B");
        assert!(!text.contains("96"));
        assert_eq!(snippet(&text), "Big news inside Café & more’s here… A B");
        // Plain-text parts sometimes carry the entities literally.
        assert_eq!(snippet("Hello &zwnj;&nbsp;&zwnj; there\u{200c}\u{34f}"), "Hello there");
        // Without the `;`, or escaped twice.
        assert_eq!(snippet("A &#847 &#8204 B"), "A B");
        assert_eq!(snippet("A &amp;#847;&amp;zwnj;&amp;nbsp; B"), "A B");
        assert_eq!(html_to_text("<p>A &amp;#847; &amp;#x34f; B</p>"), "A B");
        // Escaped text that isn't padding stays as the sender wrote it.
        assert_eq!(html_to_text("<p>Write &amp;lt;b&amp;gt; or &amp;#169;</p>"), "Write &lt;b&gt; or &#169;");
        // Ampersands that aren't references stay.
        assert_eq!(decode_entities("Tom & Jerry &unknown; AT&T"), "Tom & Jerry &unknown; AT&T");
        assert_eq!(html_to_text("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
    }

    #[test]
    fn deeply_nested_parts_do_not_overflow_the_stack() {
        // Before mailparse 0.17 this recursed once per level and crashed.
        let depth = 50_000;
        let mut raw = String::new();
        for i in 0..depth {
            raw.push_str(&format!("Content-Type: multipart/mixed; boundary=b{i}\r\n\r\n--b{i}\r\n"));
        }
        raw.push_str("Content-Type: text/plain\r\n\r\nInnermost\r\n");
        let a = Account::new(Provider::Imap);
        // Parsing in a thread with a small stack, as a crash would be fatal.
        let result = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let _ = parse_message(raw.as_bytes(), &a, "id", Folder::Inbox, "INBOX", 1);
                let _ = details(raw.as_bytes(), false);
            })
            .unwrap()
            .join();
        assert!(result.is_ok(), "parsing a deeply nested message crashed");
    }
}

/// What a reader shows under "details": addressing, the sending domains and
/// how the message was protected in transit and by its sender.
///
/// Nothing here is taken on a header's word where a sender could forge it:
/// SPF, DKIM and DMARC are checked by `auth::check`, which fills `mailed_by`,
/// `signed_by` and the verdicts; until then they are empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub from: String,
    pub reply_to: String,
    pub to: String,
    pub cc: String,
    pub date: String,
    pub subject: String,
    pub message_id: String,
    /// The envelope sender's domain, when SPF passed for it.
    pub mailed_by: Option<String>,
    /// Domains whose DKIM signatures verified.
    pub signed_by: Vec<String>,
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    pub security: Security,
    /// How the mailbox's provider received the message, if recorded.
    pub arrival: Option<Arrival>,
    /// The envelope sender, from the delivering server's Return-Path.
    pub return_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Security {
    /// The message was encrypted by its sender, and decrypted here.
    EndToEnd,
    /// The message is encrypted, but could not be decrypted here, so
    /// nothing about it can be confirmed.
    Undecrypted,
    /// The hop into the mailbox's provider used TLS.
    Tls,
    /// That hop is recorded and did not use TLS.
    Plain,
    #[default]
    Unknown,
}
impl Security {
    pub fn label(self) -> &'static str {
        match self {
            Security::EndToEnd => "End-to-end encryption",
            Security::Undecrypted => "Encrypted, but not decrypted here",
            Security::Tls => "Standard encryption (TLS)",
            Security::Plain => "Not encrypted in transit",
            Security::Unknown => "Not recorded",
        }
    }
}

/// The connection over which the mailbox's provider received the message,
/// from its own `Received` header.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrival {
    /// The connecting server's address, as the provider saw it.
    pub ip: Option<std::net::IpAddr>,
    /// The name the connecting server gave in HELO/EHLO.
    pub helo: String,
    /// The provider's server that received it.
    pub by: String,
    /// Whether the provider recorded TLS for the connection.
    pub tls: bool,
}

/// A `Received` header in pieces: words, and comments, quoted strings,
/// angle addresses and address literals kept whole so their contents never
/// read as keywords.
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Word(&'a str),
    Comment(&'a str),
    Semicolon,
}
/// Where a run opened by `bytes[start]` ends: just past its `close`, after
/// escapes and (for comments) nested comments, or at the end of the text.
fn run_end(bytes: &[u8], start: usize, close: u8) -> (usize, bool) {
    let nests = bytes[start] == b'(';
    let mut depth = 1;
    let mut j = start + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 1,
            b'(' if nests => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return (j + 1, true);
                }
            }
            _ => {}
        }
        j += 1;
    }
    (bytes.len(), false)
}
fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b if b.is_ascii_whitespace() => i += 1,
            b';' => {
                out.push(Token::Semicolon);
                i += 1;
            }
            b'(' => {
                let (end, closed) = run_end(bytes, i, b')');
                out.push(Token::Comment(&text[i + 1..if closed { end - 1 } else { end }]));
                i = end;
            }
            open @ (b'"' | b'<' | b'[') => {
                let close = match open {
                    b'"' => b'"',
                    b'<' => b'>',
                    _ => b']',
                };
                let (end, _) = run_end(bytes, i, close);
                out.push(Token::Word(&text[i..end]));
                i = end;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b';' | b'"' | b'<' | b'[')
                {
                    i += 1;
                }
                out.push(Token::Word(&text[start..i]));
            }
        }
    }
    out
}
/// `with` protocols that mean TLS (RFC 3848, and Gmail's SMTPS).
const TLS_PROTOCOLS: [&str; 11] = [
    "esmtps", "esmtpsa", "lmtps", "lmtpsa", "utf8smtps", "utf8smtpsa", "utf8lmtps",
    "utf8lmtpsa", "smtps", "smtpsa", "https",
];
/// A clause keyword of a `Received` header (RFC 5321 Section 4.4).
fn keyword(token: &Token, name: &str) -> bool {
    matches!(token, Token::Word(w) if w.eq_ignore_ascii_case(name))
}
/// A client address in a `from` clause's comment: `[192.0.2.1]`,
/// `[IPv6:2001:db8::1]`, or a bare address as Exchange writes it.
fn comment_ip(comment: &str) -> Option<std::net::IpAddr> {
    let bracketed = comment
        .split('[')
        .skip(1)
        .filter_map(|rest| rest.split_once(']').map(|(ip, _)| ip));
    let bare = comment.split([' ', ',', '\t']);
    bracketed
        .chain(bare)
        .map(|w| {
            let w = w.trim().trim_end_matches(['.', ',']);
            w.strip_prefix("IPv6:").or_else(|| w.strip_prefix("ipv6:")).unwrap_or(w)
        })
        .find_map(|w| w.parse().ok())
}
/// One `Received` header. Only the parts the receiving server writes are
/// read: its `by` clause and `with` protocol, and the client address it
/// noted in a comment. The HELO name, and the `for` address, come from the
/// sender, so a name like `tls.example` means nothing.
fn hop(header: &str) -> Option<Arrival> {
    let all = tokens(header);
    let clauses = all
        .iter()
        .position(|t| *t == Token::Semicolon)
        .map_or(&all[..], |end| &all[..end]);
    if !clauses.first().is_some_and(|t| keyword(t, "from")) {
        return None;
    }
    // The HELO name may hold anything, even " by x with ESMTPS"; the
    // receiver's own `by` clause is the last one.
    let by = clauses.iter().rposition(|t| keyword(t, "by"))?;
    let (from, after) = (&clauses[1..by], &clauses[by + 1..]);
    let helo = match from.first() {
        Some(Token::Word(w)) => w.trim_matches(['[', ']']).to_owned(),
        _ => String::new(),
    };
    // The receiver notes the client's address in a comment after the HELO
    // name; the last such comment is its own.
    let ip = from.iter().rev().find_map(|t| match t {
        Token::Comment(c) => comment_ip(c),
        _ => None,
    });
    let by_host = match after.first() {
        Some(Token::Word(w)) => w.to_ascii_lowercase(),
        _ => String::new(),
    };
    let tls = after
        .iter()
        .position(|t| keyword(t, "with"))
        .is_some_and(|with| {
            let protocol = &after[with + 1..];
            let span = protocol
                .iter()
                .position(|t| ["id", "for", "via", "by"].iter().any(|k| keyword(t, k)))
                .map_or(protocol, |end| &protocol[..end]);
            let named = matches!(span.first(), Some(Token::Word(p))
                if TLS_PROTOCOLS.iter().any(|t| p.eq_ignore_ascii_case(t)));
            // Exchange: "with Microsoft SMTP Server (version=TLS1_2, cipher=…)".
            let noted = span.iter().any(|t| {
                matches!(t, Token::Comment(c) if {
                    let c = c.to_ascii_lowercase();
                    c.contains("version=tls") || c.starts_with("using tls")
                })
            });
            named || noted
        });
    Some(Arrival {
        ip,
        helo,
        by: by_host,
        tls,
    })
}
/// Whether an address could only belong to the provider's own network.
fn internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().is_some_and(|v4| internal(v4.into()))
        }
    }
}
/// The hop over which the mailbox's provider received the message.
///
/// `Received` headers are added on top, so those above the provider's own
/// hops are its own, and any below may be forged by the sender. Going down
/// from the top, the provider's internal hops (no `from` clause, or a
/// private client address) are skipped; the first hop from a public address
/// is the arrival. When the client's address isn't noted, nothing below can
/// be trusted, so that hop is used, without an address.
fn arrival(received: &[String]) -> Option<Arrival> {
    for header in received {
        let Some(hop) = hop(header) else {
            continue;
        };
        match hop.ip {
            Some(ip) if internal(ip) => continue,
            _ => return Some(hop),
        }
    }
    None
}

/// A message's details from its headers. `decrypted` is whether the
/// message could be decrypted here; an encrypted one that could not is not
/// called end-to-end encrypted, since nothing about it was confirmed.
///
/// SPF, DKIM and DMARC are left for `auth::check`: the verdicts a server
/// wrote in `Authentication-Results`, and the domains in `DKIM-Signature`,
/// can be written by anyone sending the message.
pub fn details(raw: &[u8], decrypted: bool) -> Details {
    let Ok((headers, _)) = mailparse::parse_headers(raw) else {
        return Details::default();
    };
    let one = |name: &str| headers.get_first_value(name).unwrap_or_default();
    let arrival = arrival(&headers.get_all_values("Received"));
    let encrypted = crate::manage::crypto::sender_encrypted(raw)
        || crate::manage::crypto::encrypted_part(raw).is_some();
    let security = match &arrival {
        _ if decrypted => Security::EndToEnd,
        _ if encrypted => Security::Undecrypted,
        Some(hop) if hop.tls => Security::Tls,
        Some(_) => Security::Plain,
        None => Security::Unknown,
    };
    // The delivering server writes the topmost Return-Path.
    let return_path = Some(one("Return-Path"))
        .map(|r| r.trim().trim_matches(['<', '>']).trim().to_owned())
        .filter(|r| !r.is_empty());
    Details {
        from: one("From"),
        reply_to: one("Reply-To"),
        to: one("To"),
        cc: one("Cc"),
        date: one("Date"),
        subject: one("Subject"),
        message_id: one("Message-ID"),
        security,
        arrival,
        return_path,
        ..Details::default()
    }
}

#[cfg(test)]
mod details_tests {
    use super::*;
    #[test]
    fn headers_alone_never_vouch_for_the_sender() {
        // Results and signatures a sender wrote into the message itself.
        let raw = b"Received: from mail.example.com (mail.example.com [192.0.2.1])\r\n\tby mx.local with ESMTPS id abc\r\n\t(version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384);\r\n\tMon, 1 Jan 2024 00:00:00 +0000\r\nAuthentication-Results: mx.local;\r\n\tdkim=pass header.i=@bank.example header.s=s1;\r\n\tspf=pass smtp.mailfrom=x@bank.example;\r\n\tdmarc=pass header.from=bank.example\r\nDKIM-Signature: d=bank.example; b=garbage\r\nReturn-Path: <bounce@bounces.example.com>\r\nFrom: News <hi@example.com>\r\nSubject: Hello\r\n\r\nBody";
        let d = details(raw, false);
        assert!(d.signed_by.is_empty());
        assert_eq!((d.spf, d.dkim, d.dmarc), (None, None, None));
        assert_eq!(d.mailed_by, None);
        assert_eq!(d.return_path.as_deref(), Some("bounce@bounces.example.com"));
        assert_eq!(d.security, Security::Tls);
        assert_eq!(d.subject, "Hello");
        let arrival = d.arrival.unwrap();
        assert_eq!(arrival.ip, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(arrival.helo, "mail.example.com");
        assert_eq!(arrival.by, "mx.local");
    }

    #[test]
    fn tls_is_read_from_the_receivers_with_clause_only() {
        let security = |received: &str| {
            details(format!("Received: {received}\r\n\r\n").as_bytes(), false).security
        };
        // A sender's HELO name, comment or recipient saying "tls" counts for nothing.
        assert_eq!(
            security("from tls.attacker.example (tls.attacker.example [203.0.113.9]) by mx.example with ESMTP id 1; Mon, 1 Jan 2024"),
            Security::Plain
        );
        assert_eq!(
            security("from x (version=TLS1_3) ([203.0.113.9]) by mx.example with SMTP for <\"with ESMTPS\"@x.example>; d"),
            Security::Plain
        );
        // Even a HELO holding a fake by-clause: the receiver's own is the last.
        assert_eq!(
            security("from evil by mx.example with ESMTPS (unknown [203.0.113.9]) by mx.example with SMTP; d"),
            Security::Plain
        );
        assert_eq!(
            security("from a (a [203.0.113.9]) by mx.example with ESMTPSA id 1; d"),
            Security::Tls
        );
        assert_eq!(
            security("from a.example (203.0.113.9) by b.outlook.com (10.0.0.1) with Microsoft SMTP Server (version=TLS1_2, cipher=TLS_ECDHE) id 15; d"),
            Security::Tls
        );
        assert_eq!(security("by mx.example with SMTP id 1; d"), Security::Unknown);
    }

    #[test]
    fn the_providers_own_hops_are_skipped_and_forged_ones_below_ignored() {
        let raw = concat!(
            "Received: by 2002:a05:6a10::1 with SMTP id x; d\r\n",
            "Received: from relay (localhost [127.0.0.1]) by mx.example with LMTP; d\r\n",
            "Received: from attacker.example (attacker.example [198.51.100.7]) by mx.example with ESMTP id y; d\r\n",
            // Forged by the sender, below the provider's hops.
            "Received: from mail.bank.example (mail.bank.example [192.0.2.80]) by mx.example with ESMTPS; d\r\n",
            "\r\n",
        );
        let d = details(raw.as_bytes(), false);
        let arrival = d.arrival.unwrap();
        assert_eq!(arrival.ip, Some("198.51.100.7".parse().unwrap()));
        assert_eq!(arrival.helo, "attacker.example");
        assert_eq!(d.security, Security::Plain);
        // An address literal HELO is not the address the provider noted.
        let literal = details(
            b"Received: from [192.0.2.80] (unknown [198.51.100.7]) by mx.example with ESMTPS; d\r\n\r\n",
            false,
        );
        assert_eq!(literal.arrival.unwrap().ip, Some("198.51.100.7".parse().unwrap()));
    }

    #[test]
    fn end_to_end_only_when_decrypted() {
        let smime = b"Received: from a (a [203.0.113.9]) by mx.example with ESMTPS; d\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data\r\n\r\nMIAGCSqGSIb3DQEHA6CAMIACAQAxggE";
        assert_eq!(details(smime, false).security, Security::Undecrypted);
        assert_eq!(details(smime, true).security, Security::EndToEnd);
        let pgp = b"Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b\r\n\r\n--b\r\n\r\n--b--\r\n";
        assert_eq!(details(pgp, false).security, Security::Undecrypted);
    }
}
