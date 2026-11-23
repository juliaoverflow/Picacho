//! Address books: vCard (RFC 6350, and the 3.0 cards most servers still
//! keep) and CardDAV (RFC 6352), read and written through `dav`.
//!
//! Picacho shows and edits a card's name, email addresses, phone numbers,
//! postal addresses, birthday, photo, organization and note, and groups
//! (vCard 4's `KIND:group` with `MEMBER`s, and Apple's
//! `X-ADDRESSBOOKSERVER-KIND`/`-MEMBER`). A card read from a server keeps
//! its original text, and an edit replaces only those properties in it,
//! so what other apps keep stays as it was.
use crate::sync::dav;
use anyhow::{Result, bail, ensure};

/// Someone in an address book.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Card {
    /// The card's UID, or one made from its addresses.
    pub uid: String,
    pub name: String,
    pub emails: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phones: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub org: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Where it is on the CardDAV server, if it came from (or went to) one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub href: String,
    /// The server's ETag for the version this is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub etag: String,
    /// The card as the server had it, so an edit keeps what Picacho
    /// doesn't show.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vcard: String,
    /// Postal addresses, one line each, as people write them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
    /// The birthday: `YYYY-MM-DD`, or `--MM-DD` without a year.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub birthday: String,
    /// The photo: a `data:` URI (embedded) or an address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub photo: String,
    /// JMAP: the blob the photo was read from, when the server keeps it
    /// as one (`photo` then holds its bytes inlined to show).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub photo_blob: String,
    /// What `photo` was as read, to tell an edit from the inlined blob.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub photo_read: String,
    /// A group rather than a person: its members, by UID.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub group: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// NICKNAME (RFC 6350 §6.2.3), every one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nicknames: Vec<String>,
    /// TITLE (RFC 6350 §6.6.1): the job title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// IMPP (RFC 6350 §6.4.3, RFC 4770): chat addresses as URIs
    /// (`xmpp:ana@example.org`, `sip:…`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impps: Vec<String>,
    /// URL (RFC 6350 §6.7.8): web pages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// ANNIVERSARY (RFC 6350 §6.2.6), as `birthday` is written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub anniversary: String,
    /// RELATED (RFC 6350 §6.6.6): people related, each (TYPE, value),
    /// such as (`spouse`, `urn:uuid:…`) or (``, `Bo`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<(String, String)>,
    /// KIND (RFC 6350 §6.1.4) other than an individual or a group:
    /// `org`, `location`, or another, lowercase. Empty for a person;
    /// groups are `group`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

impl Card {
    /// The photo's bytes, where it is embedded.
    pub fn photo_bytes(&self) -> Option<Vec<u8>> {
        use base64::Engine as _;
        let data = self.photo.strip_prefix("data:")?;
        let (_, b64) = data.split_once(";base64,")?;
        base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()
    }
}

/// A structured value's components (N, ADR, ORG), split at `;` but not
/// at an escaped `\;`, each still escaped.
fn components(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b';' => {
                out.push(&value[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(&value[start.min(value.len())..]);
    out
}
/// A content line's name and parameters, and its value: split at the first
/// colon outside a quoted parameter value (`TYPE="a:b"`).
fn split_content(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}
/// A value as written: quoted-printable decoded and in UTF-8 where vCard
/// 2.1's parameters say (`ENCODING=QUOTED-PRINTABLE`, `CHARSET=`).
fn decoded_value(head: &str, value: &str) -> String {
    let upper = head.to_ascii_uppercase();
    if !upper.contains("QUOTED-PRINTABLE") {
        return value.to_owned();
    }
    let charset = upper
        .split(';')
        .find_map(|p| p.strip_prefix("CHARSET="))
        .unwrap_or("UTF-8")
        .trim_matches('"')
        .to_owned();
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'='
            && let Some(b) = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let encoding = encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(&out).0.into_owned()
}

/// An ADR value (`;;street;city;region;code;country`) as one line.
fn address_line(value: &str) -> String {
    components(value)
        .into_iter()
        .map(|p| unescape(p).replace('\n', ", ").trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A one-line address as ADR's parts (`street, city, region, code,
/// country`, as people write them, and as `address_line` writes an ADR
/// back): each part in its field; with fewer parts, the street and the
/// rest as the city.
fn address_value(line: &str) -> String {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [street, city, region, code, country] => format!(
            ";;{};{};{};{};{}",
            escape(street),
            escape(city),
            escape(region),
            escape(code),
            escape(country)
        ),
        [street, city, code, country] => format!(
            ";;{};{};;{};{}",
            escape(street),
            escape(city),
            escape(code),
            escape(country)
        ),
        _ => {
            let mut it = line.splitn(2, ',').map(str::trim);
            let street = it.next().unwrap_or_default();
            let rest = it.next().unwrap_or_default();
            format!(";;{};{};;;", escape(street), escape(rest))
        }
    }
}

/// A BDAY value (`19900101`, `1990-01-01`, `--0101`, `--01-01`) as
/// `YYYY-MM-DD` or `--MM-DD`.
fn birthday_value(value: &str) -> String {
    let v = value.trim().split('T').next().unwrap_or_default();
    // Apple's placeholder year: no year.
    if let Some(md) = v.strip_prefix("1604-").or_else(|| v.strip_prefix("1604")) {
        let digits: String = md.chars().filter(char::is_ascii_digit).collect();
        if digits.len() == 4 {
            return format!("--{}-{}", &digits[..2], &digits[2..]);
        }
    }
    let digits: String = v.chars().filter(char::is_ascii_digit).collect();
    if v.starts_with("--") && digits.len() == 4 {
        return format!("--{}-{}", &digits[..2], &digits[2..]);
    }
    if digits.len() == 8 {
        return format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..]);
    }
    v.to_owned()
}

/// Lines of a vCard file unfolded (continuation lines start with a space or
/// tab), with CRLF or LF line ends. vCard 2.1's quoted-printable soft
/// breaks (a line ending in `=`) join too.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut soft = false;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if soft && let Some(last) = lines.last_mut() {
            last.pop();
            last.push_str(line);
        } else if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
        } else {
            lines.push(line.to_owned());
        }
        let last = lines.last().map(String::as_str).unwrap_or("");
        soft = last.ends_with('=') && last.to_ascii_uppercase().contains("QUOTED-PRINTABLE");
    }
    lines
}

/// A property value with vCard's escapes (`\,` `\;` `\n` `\\`) undone.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A line's property name, without a group prefix or parameters.
fn property(head: &str) -> String {
    let name = head.split(';').next().unwrap_or("");
    name.rsplit('.').next().unwrap_or(name).to_ascii_uppercase()
}

/// Every card in a vCard file (one or many `BEGIN:VCARD` … `END:VCARD`).
pub fn parse_vcards(text: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut current: Option<(Card, String, Vec<String>)> = None;
    for line in unfold(text) {
        let Some((head, value)) = split_content(&line) else {
            continue;
        };
        let decoded = decoded_value(head, value);
        let value = decoded.as_str();
        let name = property(head);
        match name.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => {
                current = Some((Card::default(), String::new(), vec![line.clone()]));
            }
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some((mut card, structured, mut lines)) = current.take() {
                    lines.push(line.clone());
                    card.vcard = lines.join("\r\n") + "\r\n";
                    if card.name.is_empty() {
                        card.name = structured;
                    }
                    if card.uid.is_empty() {
                        card.uid = card.emails.join(",").to_lowercase();
                    }
                    // No UID and no address (a phone-only card): one made
                    // from what it says, so it stays the same card.
                    if card.uid.is_empty() {
                        card.uid = format!("{}:{}", card.name, card.phones.join(",")).to_lowercase();
                    }
                    if !card.emails.is_empty() || !card.phones.is_empty() || !card.name.is_empty() || card.group {
                        cards.push(card);
                    }
                }
            }
            _ => {
                let Some((card, structured, lines)) = current.as_mut() else {
                    continue;
                };
                lines.push(line.clone());
                match name.as_str() {
                    "FN" => card.name = unescape(value).trim().to_owned(),
                    "N" => {
                        // Family;Given;Additional;Prefix;Suffix
                        let parts: Vec<String> = components(value)
                            .into_iter()
                            .map(|p| unescape(p).trim().to_owned())
                            .collect();
                        let given = parts.get(1).cloned().unwrap_or_default();
                        let family = parts.first().cloned().unwrap_or_default();
                        *structured = format!("{given} {family}").trim().to_owned();
                    }
                    "EMAIL" => {
                        let email = unescape(value)
                            .trim()
                            .trim_start_matches("mailto:")
                            .to_owned();
                        if email.contains('@')
                            && !card.emails.iter().any(|e| e.eq_ignore_ascii_case(&email))
                        {
                            card.emails.push(email);
                        }
                    }
                    "TEL" => {
                        let phone = unescape(value).trim().trim_start_matches("tel:").to_owned();
                        if !phone.is_empty() {
                            card.phones.push(phone);
                        }
                    }
                    "ORG" => {
                        card.org = components(value)
                            .into_iter()
                            .map(|p| unescape(p).trim().to_owned())
                            .filter(|p| !p.is_empty())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                    "NOTE" => card.note = unescape(value).trim().to_owned(),
                    "UID" => card.uid = unescape(value).trim().to_owned(),
                    "ADR" => {
                        let a = address_line(value);
                        if !a.is_empty() {
                            card.addresses.push(a);
                        }
                    }
                    "BDAY" => card.birthday = birthday_value(value),
                    "PHOTO" => {
                        // vCard 4: a URI. vCard 3: base64 with ENCODING=b.
                        let upper = head.to_ascii_uppercase();
                        card.photo = if upper.contains("ENCODING=B") || upper.contains("ENCODING=BASE64") {
                            let kind = upper
                                .split(';')
                                .find_map(|p| p.strip_prefix("TYPE="))
                                .map(|t| t.to_ascii_lowercase())
                                .unwrap_or_else(|| "jpeg".into());
                            format!("data:image/{kind};base64,{}", value.trim())
                        } else {
                            value.trim().to_owned()
                        };
                    }
                    "KIND" | "X-ADDRESSBOOKSERVER-KIND" => {
                        let kind = value.trim().to_ascii_lowercase();
                        card.group = kind == "group";
                        card.kind = if card.group || kind == "individual" { String::new() } else { kind };
                    }
                    // RFC 6350 §6.2.3: a list of nicknames.
                    "NICKNAME" => card.nicknames.extend(
                        text_list(value).into_iter().filter(|n| !n.is_empty()),
                    ),
                    "TITLE" => card.title = unescape(value).trim().to_owned(),
                    p if IMPP_NAMES.contains(&p) => {
                        let uri = impp_uri(p, value);
                        if !uri.is_empty() && !card.impps.contains(&uri) {
                            card.impps.push(uri);
                        }
                    }
                    "URL" => {
                        let v = unescape(value).trim().to_owned();
                        if !v.is_empty() {
                            card.urls.push(v);
                        }
                    }
                    "ANNIVERSARY" | "X-ANNIVERSARY" if card.anniversary.is_empty() => {
                        card.anniversary = birthday_value(value)
                    }
                    "RELATED" => {
                        let r = related_value(head, value);
                        if !r.1.is_empty() {
                            card.related.push(r);
                        }
                    }
                    "MEMBER" | "X-ADDRESSBOOKSERVER-MEMBER" => {
                        // A uid by `urn:uuid:` as the bare uid; any other
                        // URI (`mailto:`) as it is, so it's written back so.
                        let v = value.trim();
                        let m = v.strip_prefix("urn:uuid:").unwrap_or(v).to_owned();
                        if !m.is_empty() {
                            card.members.push(m);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    cards
}

/// The properties a chat address is written in: IMPP (RFC 6350 §6.4.3)
/// and the X- forms vCard 3 apps used before it (RFC 4770 §1).
const IMPP_NAMES: [&str; 7] = ["IMPP", "X-JABBER", "X-SKYPE", "X-ICQ", "X-AIM", "X-MSN", "X-YAHOO"];
/// A chat address line's value as an IMPP URI (`X-JABBER:ana@x` is
/// `xmpp:ana@x`).
fn impp_uri(property: &str, value: &str) -> String {
    let v = unescape(value).trim().to_owned();
    let scheme = match property {
        "X-JABBER" => "xmpp",
        "X-SKYPE" => "skype",
        "X-ICQ" => "icq",
        "X-AIM" => "aim",
        "X-MSN" => "msnim",
        "X-YAHOO" => "ymsgr",
        _ => return v,
    };
    if v.is_empty() || v.contains(':') { v } else { format!("{scheme}:{v}") }
}
/// A RELATED line's (TYPE, value), as `parse_vcards` reads it.
fn related_value(head: &str, value: &str) -> (String, String) {
    let kind = head
        .split(';')
        .skip(1)
        .find_map(|p| {
            let (k, v) = p.split_once('=')?;
            k.eq_ignore_ascii_case("TYPE").then(|| v.trim_matches('"').to_ascii_lowercase())
        })
        .unwrap_or_default();
    (kind, unescape(value).trim().to_owned())
}

/// A list value (NICKNAME, CATEGORIES: RFC 6350 §3.4) split at unescaped
/// commas, each unescaped.
fn text_list(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                current.push(c);
                current.extend(chars.next());
            }
            ',' => out.push(unescape(std::mem::take(&mut current).trim()).trim().to_owned()),
            c => current.push(c),
        }
    }
    out.push(unescape(current.trim()).trim().to_owned());
    out
}

/// A value escaped for a vCard property.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// Folds a line at 75 octets. A line break or NUL inside it (a value a
/// server or imported file gave, such as a bare CR, which `escape` leaves)
/// is left out: it would end the line and start a property.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')) {
        if count + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

/// The card's own properties, as Picacho writes them.
fn own_lines(card: &Card) -> Vec<String> {
    let name = if card.name.is_empty() {
        card.emails.first().cloned().unwrap_or_default()
    } else {
        card.name.clone()
    };
    let mut lines = vec![format!("FN:{}", escape(&name))];
    // N is required by vCard 3.0: family name last, as people write it.
    let (given, family) = match name.trim().rsplit_once(' ') {
        Some((g, f)) => (g.to_owned(), f.to_owned()),
        None => (name.trim().to_owned(), String::new()),
    };
    lines.push(format!("N:{};{};;;", escape(&family), escape(&given)));
    lines.extend(card.emails.iter().map(|e| format!("EMAIL:{}", escape(e))));
    lines.extend(card.phones.iter().map(|p| format!("TEL:{}", escape(p))));
    if !card.org.is_empty() {
        lines.push(format!("ORG:{}", escape(&card.org)));
    }
    if !card.note.is_empty() {
        lines.push(format!("NOTE:{}", escape(&card.note)));
    }
    lines.extend(card.addresses.iter().map(|a| format!("ADR:{}", address_value(a))));
    if !card.birthday.is_empty() {
        // 1990-01-01 → 19900101; --01-01 (no year) → --0101.
        // 1990-01-01 → 1990-01-01. Without a year: vCard 3.0 needs one, so
        // Apple's placeholder year and its marker (as iOS and macOS write
        // it); `to_vcard4` makes it vCard 4.0's --MMDD.
        let b = &card.birthday;
        match b.strip_prefix("--") {
            Some(md) => lines.push(format!("BDAY;X-APPLE-OMIT-YEAR=1604:1604-{md}")),
            None => lines.push(format!("BDAY:{b}")),
        }
    }
    if !card.photo.is_empty() {
        // vCard 3's form, which every address book reads.
        match card.photo.strip_prefix("data:").and_then(|d| d.split_once(";base64,")) {
            Some((kind, data)) => lines.push(format!(
                "PHOTO;ENCODING=b;TYPE={}:{data}",
                kind.trim_start_matches("image/").to_ascii_uppercase()
            )),
            None => lines.push(format!("PHOTO;VALUE=uri:{}", card.photo)),
        }
    }
    if !card.nicknames.is_empty() {
        lines.push(format!("NICKNAME:{}", card.nicknames.iter().map(|n| escape(n)).collect::<Vec<_>>().join(",")));
    }
    if !card.title.is_empty() {
        lines.push(format!("TITLE:{}", escape(&card.title)));
    }
    // RFC 4770 brought IMPP to vCard 3 as well.
    lines.extend(card.impps.iter().map(|i| format!("IMPP:{i}")));
    lines.extend(card.urls.iter().map(|u| format!("URL:{u}")));
    if !card.anniversary.is_empty() {
        // vCard 3 has no ANNIVERSARY: the X- form Evolution and KDE
        // write, which `to_vcard4` makes ANNIVERSARY.
        let a = &card.anniversary;
        lines.push(match a.strip_prefix("--") {
            Some(md) => format!("X-ANNIVERSARY:--{}", md.replace('-', "")),
            None => format!("X-ANNIVERSARY:{a}"),
        });
    }
    for (kind, value) in &card.related {
        let kind = if kind.is_empty() { String::new() } else { format!(";TYPE={kind}") };
        lines.push(format!("RELATED{kind}:{}", escape(value)));
    }
    if !card.kind.is_empty() && !card.group {
        lines.push(format!("KIND:{}", card.kind));
    }
    if card.group {
        // Apple's form, which vCard 3 servers (iCloud too) keep.
        lines.push("X-ADDRESSBOOKSERVER-KIND:group".into());
        lines.extend(card.members.iter().map(|m| {
            if m.contains(':') {
                format!("X-ADDRESSBOOKSERVER-MEMBER:{m}")
            } else {
                format!("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:{m}")
            }
        }));
    }
    lines
}

/// A line's group (`item1` in `item1.EMAIL;…`), uppercase, if it has one.
fn group(head: &str) -> Option<String> {
    let name = head.split(';').next().unwrap_or("");
    name.rsplit_once('.').map(|(g, _)| g.to_ascii_uppercase())
}

/// An EMAIL line's address, as `parse_vcards` reads it.
fn email_value(value: &str) -> String {
    unescape(value)
        .trim()
        .trim_start_matches("mailto:")
        .to_owned()
}

/// A TEL line's number, as `parse_vcards` reads it.
fn phone_value(value: &str) -> String {
    unescape(value).trim().trim_start_matches("tel:").to_owned()
}

/// One card as vCard text. A card read from a server keeps every property
/// Picacho doesn't edit, and the lines of those it does edit whose value
/// didn't change, with their labels (`TYPE=work`, `pref`, Apple's
/// `item1.X-ABLabel`) and structure (`N`'s prefixes, middle names and
/// suffixes, `ORG`'s units). Only changed values are written anew.
pub fn write_vcard(card: &Card) -> String {
    let original = unfold(&card.vcard);
    let before = parse_vcards(&card.vcard).into_iter().next();
    let Some(before) = before else {
        // Made here: nothing to keep.
        let mut out = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
        for line in own_lines(card) {
            out.push_str(&fold(&line));
        }
        if !card.uid.is_empty() {
            out.push_str(&fold(&format!("UID:{}", escape(&card.uid))));
        }
        out.push_str("END:VCARD\r\n");
        return out;
    };
    let name_same = before.name == card.name;
    let mut addresses_left: Vec<&String> = card.addresses.iter().collect();
    let mut members_left: Vec<&String> = card.members.iter().collect();
    let mut emails_left: Vec<&String> = card.emails.iter().collect();
    let mut phones_left: Vec<&String> = card.phones.iter().collect();
    let mut impps_left: Vec<&String> = card.impps.iter().collect();
    let mut urls_left: Vec<&String> = card.urls.iter().collect();
    let mut related_left: Vec<&(String, String)> = card.related.iter().collect();
    // Takes a value from what's left, where it is there: then its line
    // stays as it was.
    fn take<T: PartialEq + ?Sized>(left: &mut Vec<&T>, v: &T) -> bool {
        match left.iter().position(|x| *x == v) {
            Some(i) => {
                left.remove(i);
                true
            }
            None => false,
        }
    }
    let mut kept: Vec<String> = Vec::new();
    let mut dropped_groups: Vec<String> = Vec::new();
    let mut version = "VERSION:3.0".to_owned();
    let has = |p: &str, kept: &Vec<String>| {
        kept.iter()
            .any(|l| property(l.split(':').next().unwrap_or("")) == p)
    };
    for line in original.iter().filter(|l| !l.is_empty()) {
        let Some((head, value)) = split_content(line) else {
            continue;
        };
        let decoded = decoded_value(head, value);
        let value = decoded.as_str();
        let p = property(head);
        let keep = match p.as_str() {
            "BEGIN" | "END" => false,
            "VERSION" => {
                version = line.clone();
                false
            }
            "FN" | "N" => name_same,
            "ORG" => before.org == card.org,
            "NOTE" => before.note == card.note,
            "UID" => before.uid == card.uid,
            "EMAIL" => {
                let v = email_value(value);
                match emails_left.iter().position(|e| e.eq_ignore_ascii_case(&v)) {
                    Some(i) => {
                        emails_left.remove(i);
                        true
                    }
                    None => false,
                }
            }
            "TEL" => {
                let v = phone_value(value);
                match phones_left.iter().position(|t| **t == v) {
                    Some(i) => {
                        phones_left.remove(i);
                        true
                    }
                    None => false,
                }
            }
            "ADR" => {
                let v = address_line(value);
                match addresses_left.iter().position(|a| **a == v) {
                    Some(i) => {
                        addresses_left.remove(i);
                        true
                    }
                    None => false,
                }
            }
            "BDAY" => birthday_value(value) == card.birthday,
            "PHOTO" => before.photo == card.photo,
            "KIND" | "X-ADDRESSBOOKSERVER-KIND" => before.group == card.group && before.kind == card.kind,
            "NICKNAME" => before.nicknames == card.nicknames,
            "TITLE" => before.title == card.title,
            p if IMPP_NAMES.contains(&p) => take(&mut impps_left, &impp_uri(p, value)),
            "URL" => take(&mut urls_left, &unescape(value).trim().to_owned()),
            "ANNIVERSARY" | "X-ANNIVERSARY" => birthday_value(value) == card.anniversary,
            "RELATED" => take(&mut related_left, &related_value(head, value)),
            "MEMBER" | "X-ADDRESSBOOKSERVER-MEMBER" => {
                let v = value.trim();
                let v = v.strip_prefix("urn:uuid:").unwrap_or(v);
                match members_left.iter().position(|m| m.as_str() == v) {
                    Some(i) => {
                        members_left.remove(i);
                        true
                    }
                    None => false,
                }
            }
            _ => true,
        };
        if keep {
            kept.push(line.clone());
        } else if let Some(g) = group(head) {
            dropped_groups.push(g);
        }
    }
    // A label (`item1.X-ABLabel`) whose address or number went goes too.
    kept.retain(|l| {
        let head = l.split(':').next().unwrap_or("");
        match group(head) {
            Some(g) if dropped_groups.contains(&g) => !property(head).starts_with("X-AB"),
            _ => true,
        }
    });
    // What changed, or wasn't there, written from the card's fields.
    let mut added: Vec<String> = Vec::new();
    for line in own_lines(card) {
        let p = property(line.split(':').next().unwrap_or(""));
        let wanted = match p.as_str() {
            "FN" | "N" => !has(&p, &kept),
            "ORG" => !has("ORG", &kept),
            "NOTE" => !has("NOTE", &kept),
            "EMAIL" => {
                let v = email_value(line.split_once(':').map_or("", |(_, v)| v));
                emails_left.iter().any(|e| e.eq_ignore_ascii_case(&v))
            }
            "TEL" => {
                let v = phone_value(line.split_once(':').map_or("", |(_, v)| v));
                phones_left.iter().any(|t| **t == v)
            }
            "ADR" => {
                let v = address_line(line.split_once(':').map_or("", |(_, v)| v));
                addresses_left.iter().any(|a| **a == v)
            }
            "BDAY" => !has("BDAY", &kept),
            "PHOTO" => !has("PHOTO", &kept),
            "X-ADDRESSBOOKSERVER-KIND" | "KIND" => {
                !has("X-ADDRESSBOOKSERVER-KIND", &kept) && !has("KIND", &kept)
            }
            "NICKNAME" | "TITLE" => !has(&p, &kept),
            "IMPP" => impps_left.iter().any(|i| Some(i.as_str()) == line.split_once(':').map(|(_, v)| v)),
            "URL" => urls_left.iter().any(|u| Some(u.as_str()) == line.split_once(':').map(|(_, v)| v)),
            "X-ANNIVERSARY" => !has("ANNIVERSARY", &kept) && !has("X-ANNIVERSARY", &kept),
            "RELATED" => {
                let (head, value) = line.split_once(':').unwrap_or((&line, ""));
                related_left.contains(&&related_value(head, value))
            }
            "X-ADDRESSBOOKSERVER-MEMBER" => {
                let v = line.split_once(':').map_or("", |(_, v)| v);
                let v = v.strip_prefix("urn:uuid:").unwrap_or(v);
                members_left.iter().any(|m| m.as_str() == v)
            }
            _ => true,
        };
        if wanted {
            added.push(line);
        }
    }
    if !card.uid.is_empty() && !has("UID", &kept) {
        added.push(format!("UID:{}", escape(&card.uid)));
    }
    let mut out = String::from("BEGIN:VCARD\r\n");
    out.push_str(&fold(&version));
    for line in kept.iter().chain(&added) {
        out.push_str(&fold(line));
    }
    out.push_str("END:VCARD\r\n");
    // A vCard 4.0 card keeps its version's syntax for what was written
    // anew (no ENCODING=b, KIND and MEMBER rather than Apple's).
    if version.trim().eq_ignore_ascii_case("VERSION:4.0") {
        return to_vcard4(&out);
    }
    out
}

/// Cards written as one vCard file, which address books everywhere import.
pub fn write_vcards(cards: &[Card]) -> String {
    cards.iter().map(write_vcard).collect()
}

/// Where and how to reach a CardDAV server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    /// The address the person gave: the server, or an address book on it.
    pub url: String,
    pub user: String,
    /// The address book new contacts go to, once found.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub book: String,
    /// Contacts kept by one of the mail accounts' JMAP servers (JMAP for
    /// Contacts) instead of a CardDAV server: that account's id here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jmap_account: String,
    /// Each address book's state when last read: its sync token (RFC 6578)
    /// or ctag, so an unchanged book costs no download and a changed one
    /// only what changed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub states: std::collections::BTreeMap<String, dav::State>,
    /// The book new cards go to takes only vCard 4.0
    /// (`supported-address-data`): new cards are written that way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vcard4: bool,
    /// The largest card that book takes, in octets (`max-resource-size`);
    /// 0 where it doesn't say.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_size: u64,
    /// What each address book takes, by href (RFC 6352 §6.2.2
    /// `supported-address-data`, §6.2.3 `max-resource-size`): a card is
    /// saved by its own book's, not the one new cards go to.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub books: std::collections::BTreeMap<String, BookLimits>,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What one address book takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BookLimits {
    /// Only vCard 4.0 (RFC 6352 §6.2.2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vcard4: bool,
    /// The largest card in octets; 0 where it doesn't say (§6.2.3).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_size: u64,
}
impl Server {
    /// The limits of the book `href` is in (a card's href, or a book's),
    /// else those of the book new cards go to.
    pub fn limits_for(&self, href: &str) -> BookLimits {
        self.books
            .iter()
            .filter(|(book, _)| url::Url::parse(book).is_ok_and(|b| href == book.as_str() || dav::in_collection(href, &b)))
            .map(|(_, l)| l.clone())
            .next()
            .unwrap_or(BookLimits { vcard4: self.vcard4, max_size: self.max_size })
    }
}
/// A collection's limits as an address book's.
fn book_limits(b: &dav::Collection) -> BookLimits {
    BookLimits {
        vcard4: !b.vcard_versions.is_empty() && !b.vcard_versions.iter().any(|v| v == "3.0"),
        max_size: b.max_size,
    }
}

/// A card written as vCard 3.0, as vCard 4.0 (RFC 6350) for a book that
/// takes nothing else: VERSION, a photo as a data URI, a group's
/// KIND/MEMBER instead of Apple's X-ADDRESSBOOKSERVER ones, and no N
/// requirement (kept anyway; it is allowed).
pub fn to_vcard4(text: &str) -> String {
    let mut out = String::new();
    for line in unfold(text) {
        let upper = line.to_ascii_uppercase();
        let line = if upper.starts_with("VERSION:") {
            "VERSION:4.0".to_owned()
        } else if let Some(rest) = upper.strip_prefix("X-ADDRESSBOOKSERVER-KIND:") {
            format!("KIND:{}", rest.to_ascii_lowercase())
        } else if upper.starts_with("X-ADDRESSBOOKSERVER-MEMBER:") {
            format!("MEMBER:{}", &line["X-ADDRESSBOOKSERVER-MEMBER:".len()..])
        } else if upper.starts_with("PHOTO;ENCODING=B;TYPE=") {
            // PHOTO;ENCODING=b;TYPE=JPEG:<data> → PHOTO:data:image/jpeg;base64,<data>
            let (head, data) = line.split_once(':').unwrap_or((&line, ""));
            let kind = head.rsplit('=').next().unwrap_or("jpeg").to_ascii_lowercase();
            format!("PHOTO:data:image/{kind};base64,{data}")
        } else if upper.starts_with("PHOTO;VALUE=URI:") {
            format!("PHOTO:{}", &line["PHOTO;VALUE=uri:".len()..])
        } else if let Some(md) = upper.strip_prefix("BDAY;X-APPLE-OMIT-YEAR=1604:1604-") {
            format!("BDAY:--{}", md.replace('-', ""))
        } else if upper.starts_with("X-ANNIVERSARY:") {
            // RFC 6350 §6.2.6.
            format!("ANNIVERSARY:{}", &line["X-ANNIVERSARY:".len()..])
        } else {
            line
        };
        out.push_str(&fold(&line));
    }
    out
}

const QUERY: &str = "<?xml version=\"1.0\"?><c:addressbook-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:prop><d:getetag/><c:address-data/></d:prop></c:addressbook-query>";

/// Reads every address book the account can see, whole. Returns the
/// cards, each with its href and ETag, and the address book new cards go to
/// (the first one the account may write to).
#[cfg(test)]
pub fn fetch(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
) -> Result<(Vec<Card>, String)> {
    let fetched = fetch_changes(server, password, tls, &[])?;
    Ok((fetched.cards, fetched.book))
}

/// What `fetch_changes` found.
pub struct Fetched {
    /// Every card on the server.
    pub cards: Vec<Card>,
    /// The address book new cards go to.
    pub book: String,
    /// Each address book's state now, for the next sync.
    pub states: std::collections::BTreeMap<String, dav::State>,
    /// What that book takes (`Server::vcard4`, `Server::max_size`).
    pub vcard4: bool,
    pub max_size: u64,
    /// What every book takes, by href (`Server::books`).
    pub books: std::collections::BTreeMap<String, BookLimits>,
}

/// Reads the address books, downloading only what changed since `known`
/// (the cards read last time) where the server can say: by sync token
/// (RFC 6578), else not at all for a book whose ctag is the same.
pub fn fetch_changes(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    known: &[Card],
) -> Result<Fetched> {
    let client = dav::Client::new(&server.user, password, tls)?;
    let books = dav::discover(
        &client,
        &server.url,
        "carddav",
        "addressbook",
        "addressbook-home-set",
        dav::CARDDAV,
    )?;
    let mut cards = Vec::new();
    let mut states = std::collections::BTreeMap::new();
    for book in &books {
        let key = book.href.to_string();
        let before: Vec<&Card> = known
            .iter()
            .filter(|c| dav::in_collection(&c.href, &book.href))
            .collect();
        let previous = server.states.get(&key).filter(|_| !known.is_empty());
        let read = dav::read_collection(
            &client,
            book,
            previous,
            &before
                .iter()
                .map(|c| (c.href.clone(), c.etag.clone()))
                .collect::<Vec<_>>(),
            |c| c.list(&book.href, QUERY, "address-data"),
            |c, hrefs| {
                c.multiget(
                    &book.href,
                    "addressbook-multiget",
                    dav::CARDDAV,
                    "address-data",
                    hrefs,
                )
            },
        )?;
        // Cards kept as they were, then those read anew.
        cards.extend(
            before
                .into_iter()
                .filter(|c| read.kept.contains(&c.href))
                .cloned(),
        );
        for resource in read.fetched {
            for mut card in parse_vcards(&resource.data) {
                card.href = resource.href.to_string();
                card.etag = resource.etag.clone();
                cards.push(card);
            }
        }
        states.insert(key, read.state);
    }
    let writable = books.iter().find(|b| b.writable);
    let book = writable.map(|b| b.href.to_string()).unwrap_or_default();
    let chosen = writable.map(book_limits).unwrap_or_default();
    Ok(Fetched {
        cards,
        book,
        states,
        vcard4: chosen.vcard4,
        max_size: chosen.max_size,
        books: books.iter().map(|b| (b.href.to_string(), book_limits(b))).collect(),
    })
}

/// Makes an address book named `name` in the account's address book home.
/// Returns its address.
pub fn create_book(server: &Server, password: &str, tls: native_tls::TlsConnector, name: &str) -> Result<String> {
    let client = dav::Client::new(&server.user, password, tls)?;
    let home = client.home(&server.url, "carddav", "addressbook-home-set", dav::CARDDAV)?;
    let href = dav::new_href(&home, &uuid::Uuid::new_v4().to_string(), "x")?;
    let href = url::Url::parse(&format!("{}/", href.as_str().trim_end_matches(".x")))?;
    client.make_collection(&href, false, name, "")?;
    Ok(href.to_string())
}

/// The address books, by href and name, for choosing among them.
pub fn books(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<Vec<(String, String, bool)>> {
    let client = dav::Client::new(&server.user, password, tls)?;
    Ok(dav::discover(
        &client,
        &server.url,
        "carddav",
        "addressbook",
        "addressbook-home-set",
        dav::CARDDAV,
    )?
    .into_iter()
    .map(|b| (b.href.to_string(), b.name, b.writable))
    .collect())
}
/// The account's storage on the contacts server, used and left in
/// octets, where it says (RFC 4331 on the address book home).
pub fn quota(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<Option<(u64, u64)>> {
    let client = dav::Client::new(&server.user, password, tls)?;
    let home = client.home(&server.url, "carddav", "addressbook-home-set", dav::CARDDAV)?;
    client.quota(&home)
}
/// Renames an address book (PROPPATCH).
pub fn rename_book(server: &Server, password: &str, tls: native_tls::TlsConnector, href: &str, name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "Give the address book a name.");
    dav::Client::new(&server.user, password, tls)?.rename_collection(&url::Url::parse(href)?, name.trim(), "")
}
/// Deletes an address book and its cards.
pub fn delete_book(server: &Server, password: &str, tls: native_tls::TlsConnector, href: &str) -> Result<()> {
    dav::Client::new(&server.user, password, tls)?.delete_collection(&url::Url::parse(href)?)
}

/// Saves a card on the server: a new one in `book`, or a change to the
/// one at its href (if it is still the version with its ETag). Returns the
/// card as saved, with its href and new ETag.
pub fn save(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    card: &Card,
) -> Result<Card> {
    let client = dav::Client::new(&server.user, password, tls)?;
    let mut saved = card.clone();
    let href = if card.href.is_empty() {
        ensure!(
            !server.book.is_empty(),
            "No address book on the server can take new contacts."
        );
        dav::new_href(&url::Url::parse(&server.book)?, &card.uid, "vcf")?
    } else {
        url::Url::parse(&card.href)?
    };
    let limits = server.limits_for(href.as_str());
    let mut text = write_vcard(card);
    // A new card for a book that only takes vCard 4.0; edited ones keep
    // the version they came in.
    if card.href.is_empty() && limits.vcard4 {
        text = to_vcard4(&text);
    }
    if limits.max_size > 0 && text.len() as u64 > limits.max_size {
        bail!(
            "This contact is too large for the address book ({} KB at most). Use a smaller photo.",
            limits.max_size / 1024
        );
    }
    saved.etag = client.put(&href, "text/vcard; charset=utf-8", &card.etag, text.clone())?;
    saved.href = href.to_string();
    saved.vcard = text;
    Ok(saved)
}

/// Removes a card from the server.
pub fn delete(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    card: &Card,
) -> Result<()> {
    if card.href.is_empty() {
        return Ok(());
    }
    let client = dav::Client::new(&server.user, password, tls)?;
    client.delete(&url::Url::parse(&card.href)?, &card.etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cards_go_as_vcard4_where_only_that_is_taken() {
        let card = Card {
            uid: "g".into(),
            name: "Friends".into(),
            group: true,
            members: vec!["bo".into()],
            photo: "data:image/png;base64,aGVsbG8=".into(),
            ..Default::default()
        };
        let v4 = to_vcard4(&write_vcard(&card));
        assert!(v4.contains("VERSION:4.0\r\n") && !v4.contains("VERSION:3.0"), "{v4}");
        assert!(v4.contains("KIND:group") && v4.contains("MEMBER:urn:uuid:bo"), "{v4}");
        assert!(v4.contains("PHOTO:data:image/png;base64,aGVsbG8="), "{v4}");
        let back = parse_vcards(&v4).remove(0);
        assert!(back.group && back.members == ["bo"]);
    }

    #[test]
    fn vcards_of_both_versions_are_read() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Chen;Sophie;;;\r\nitem1.EMAIL;TYPE=INTERNET,pref:sophie@example.com\r\nEMAIL;TYPE=work:s.chen@\r\n work.example\r\nTEL;TYPE=cell:+1 555 0100\r\nORG:Acme;Research\r\nUID:abc-1\r\nEND:VCARD\r\nBEGIN:VCARD\nVERSION:4.0\nFN:O\\'Neil\\, Pat\nEMAIL:mailto:pat@example.org\nEND:VCARD\n";
        let cards = parse_vcards(text);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].name, "Sophie Chen");
        assert_eq!(
            cards[0].emails,
            ["sophie@example.com", "s.chen@work.example"]
        );
        assert_eq!(cards[0].phones, ["+1 555 0100"]);
        assert_eq!(cards[0].org, "Acme, Research");
        assert_eq!(cards[0].uid, "abc-1");
        assert_eq!(cards[1].name, "O'Neil, Pat");
        assert_eq!(cards[1].uid, "pat@example.org");
        let again = parse_vcards(&write_vcards(&cards));
        assert_eq!(again[0].emails, cards[0].emails);
        assert_eq!(again[1].name, cards[1].name);
    }

    /// RFC 6352 §6.2.2–§6.2.3: each book's vCard versions and size are
    /// kept, and a card is saved by its own book's; RFC 4331 quota is read
    /// from the address book home.
    #[test]
    fn each_book_has_its_own_limits_and_quota_is_read() -> Result<()> {
        let (port, server) = dav::test_server(6, |method, path, _| match (method, path) {
            ("PROPFIND", "/.well-known/carddav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:r=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/ab/big/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><r:addressbook/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/ab/small/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><r:addressbook/></d:resourcetype><r:max-resource-size>200</r:max-resource-size><r:supported-address-data><r:address-data-type content-type=\"text/vcard\" version=\"4.0\"/></r:supported-address-data><d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", _) => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"/>".into()),
            ("PROPFIND", "/p/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:r=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/p/</d:href><d:propstat><d:prop><r:addressbook-home-set><d:href>/ab/</d:href></r:addressbook-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/ab/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/ab/</d:href><d:propstat><d:prop><d:quota-used-bytes>1048576</d:quota-used-bytes><d:quota-available-bytes>9437184</d:quota-available-bytes></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            _ => ("404 Not Found", String::new()),
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let srv = Server { url: format!("http://127.0.0.1:{port}"), user: "ana".into(), ..Default::default() };
        let fetched = fetch_changes(&srv, "secret", tls(), &[])?;
        let base = format!("http://127.0.0.1:{port}");
        assert_eq!(fetched.book, format!("{base}/ab/big/"));
        assert_eq!((fetched.vcard4, fetched.max_size), (false, 0));
        let srv = Server { book: fetched.book.clone(), books: fetched.books.clone(), ..srv };
        // A card in the small, vCard-4-only book is held to its limits,
        // though new ones go to the big one.
        let small = srv.limits_for(&format!("{base}/ab/small/x.vcf"));
        assert_eq!(small, BookLimits { vcard4: true, max_size: 200 });
        assert_eq!(srv.limits_for(&format!("{base}/ab/big/y.vcf")), BookLimits::default());
        let big = Card { uid: "x".into(), name: "X".into(), note: "n".repeat(300), href: format!("{base}/ab/small/x.vcf"), ..Default::default() };
        assert!(save(&srv, "secret", tls(), &big).unwrap_err().to_string().contains("too large"));
        // Given the principal's address, which names no other principal.
        let srv = Server { url: format!("{base}/p/"), ..srv };
        assert_eq!(quota(&srv, "secret", tls())?, Some((1_048_576, 9_437_184)));
        // Discovery, two book reads; the principal twice (its principal,
        // its home set) and the home's quota.
        assert_eq!(server.join().unwrap().len(), 6);
        Ok(())
    }

    /// RFC 6350's NICKNAME, TITLE, IMPP, URL, ANNIVERSARY, RELATED and
    /// KIND are read, and survive an edit of something else exactly as
    /// written; changed, they're written anew in the card's version.
    #[test]
    fn vcard4_properties_are_modelled() {
        let v4 = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ana López\r\nNICKNAME:Anita,La Jefa\r\nTITLE:Head of Research\r\nIMPP;PREF=1:xmpp:ana@example.org\r\nURL;TYPE=work:https://example.org/ana\r\nANNIVERSARY:20100612\r\nRELATED;TYPE=spouse:urn:uuid:bo-1\r\nRELATED;VALUE=text:Cy\r\nUID:ana\r\nEND:VCARD\r\n";
        let ana = parse_vcards(v4).remove(0);
        assert_eq!(ana.nicknames, ["Anita", "La Jefa"]);
        assert_eq!(ana.title, "Head of Research");
        assert_eq!(ana.impps, ["xmpp:ana@example.org"]);
        assert_eq!(ana.urls, ["https://example.org/ana"]);
        assert_eq!(ana.anniversary, "2010-06-12");
        assert_eq!(ana.related, [("spouse".to_owned(), "urn:uuid:bo-1".to_owned()), (String::new(), "Cy".to_owned())]);
        // Another field edited: these lines stay as written, once each.
        let mut edited = ana.clone();
        edited.note = "Met at the conference".into();
        let written = write_vcard(&edited);
        for line in ["NICKNAME:Anita,La Jefa", "IMPP;PREF=1:xmpp:ana@example.org", "URL;TYPE=work:https://example.org/ana", "ANNIVERSARY:20100612", "RELATED;TYPE=spouse:urn:uuid:bo-1", "RELATED;VALUE=text:Cy"] {
            assert_eq!(written.matches(line).count(), 1, "{line} in {written}");
        }
        assert_eq!(written.matches("IMPP").count(), 1);
        // Changed: written anew, nothing doubled.
        edited.title = "Director".into();
        edited.impps.push("sip:ana@example.org".into());
        edited.urls.clear();
        edited.anniversary = "--06-12".into();
        edited.related.remove(1);
        let back = parse_vcards(&write_vcard(&edited)).remove(0);
        assert_eq!((back.title.as_str(), back.anniversary.as_str()), ("Director", "--06-12"));
        assert_eq!(back.impps, edited.impps);
        assert!(back.urls.is_empty());
        assert_eq!(back.related, edited.related);
        assert!(write_vcard(&edited).contains("ANNIVERSARY:--0612"), "{}", write_vcard(&edited));
        // vCard 3's chat forms read as IMPP; an organization's KIND.
        let v3 = parse_vcards("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Acme\r\nX-JABBER:acme@example.com\r\nX-SKYPE:acme.support\r\nX-ANNIVERSARY:1999-01-01\r\nKIND:org\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nFN:Room 4\r\nKIND:location\r\nEND:VCARD\r\n");
        assert_eq!(v3[0].impps, ["xmpp:acme@example.com", "skype:acme.support"]);
        assert_eq!((v3[0].kind.as_str(), v3[0].anniversary.as_str()), ("org", "1999-01-01"));
        assert_eq!(v3[1].kind, "location");
        // Made here: each written once, and read back the same.
        let made = Card { uid: "n".into(), name: "Acme".into(), kind: "org".into(), nicknames: vec!["A, Inc".into()], ..v3[0].clone() };
        let made = Card { vcard: String::new(), ..made };
        let text = write_vcard(&made);
        assert!(text.contains("KIND:org") && text.contains("X-ANNIVERSARY:1999-01-01") && text.contains("NICKNAME:A\\, Inc"), "{text}");
        let back = parse_vcards(&text).remove(0);
        assert_eq!((back.kind, back.impps, back.nicknames), (made.kind.clone(), made.impps.clone(), made.nicknames.clone()));
        assert!(to_vcard4(&text).contains("ANNIVERSARY:1999-01-01"));
    }

    #[test]
    fn edits_keep_what_picacho_doesnt_show() {
        let original = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bo\r\nEMAIL:bo@example.se\r\nPHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\nBDAY:1990-01-01\r\nUID:1\r\nEND:VCARD\r\n";
        let mut card = parse_vcards(original).remove(0);
        card.name = "Bo Lindqvist".into();
        card.emails.push("bo@work.se".into());
        let written = write_vcard(&card);
        assert!(
            written.contains("PHOTO;ENCODING=b;TYPE=JPEG:AAAA")
                && written.contains("BDAY:1990-01-01")
        );
        assert!(written.contains("VERSION:3.0") && written.contains("N:Lindqvist;Bo;;;"));
        let back = parse_vcards(&written).remove(0);
        assert_eq!(back.name, "Bo Lindqvist");
        assert_eq!(back.emails, ["bo@example.se", "bo@work.se"]);
        assert_eq!(written.matches("UID:").count(), 1);
    }

    #[test]
    fn addresses_birthdays_photos_and_groups() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bo\r\nEMAIL:bo@example.se\r\nADR;TYPE=home:;;Storgatan 1;Stockholm;;111 22;Sweden\r\nBDAY:1990-01-31\r\nPHOTO;ENCODING=b;TYPE=PNG:aGVsbG8=\r\nUID:bo\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:3.0\r\nFN:Friends\r\nX-ADDRESSBOOKSERVER-KIND:group\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:bo\r\nUID:g1\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nFN:Cy\r\nBDAY:--0704\r\nKIND:individual\r\nEND:VCARD\r\n";
        let cards = parse_vcards(text);
        assert_eq!(cards.len(), 3);
        let bo = &cards[0];
        assert_eq!(bo.addresses, ["Storgatan 1, Stockholm, 111 22, Sweden"]);
        assert_eq!(bo.birthday, "1990-01-31");
        assert_eq!(bo.photo_bytes().as_deref(), Some(&b"hello"[..]));
        assert!(cards[1].group && cards[1].members == ["bo"]);
        assert_eq!(cards[2].birthday, "--07-04");
        assert!(!cards[2].group);
        // Unchanged, they're kept as they were; changed, written anew.
        let same = write_vcard(bo);
        assert!(same.contains("ADR;TYPE=home:;;Storgatan 1;Stockholm;;111 22;Sweden\r\n"));
        assert!(same.contains("PHOTO;ENCODING=b;TYPE=PNG:aGVsbG8=\r\n"));
        let mut moved = bo.clone();
        moved.addresses = vec!["Kungsgatan 5, Göteborg".into()];
        moved.birthday = "--01-31".into();
        let written = write_vcard(&moved);
        assert!(!written.contains("Storgatan"));
        assert!(written.contains("ADR:;;Kungsgatan 5;Göteborg;;;\r\n"));
        // vCard 3.0 needs a year: Apple's placeholder, and its marker.
        assert!(written.contains("BDAY;X-APPLE-OMIT-YEAR=1604:1604-01-31\r\n") && !written.contains("1990"));
        assert!(to_vcard4(&written).contains("BDAY:--0131\r\n"));
        // An address with its region, code and country keeps them.
        let mut full = bo.clone();
        full.addresses = vec!["Kungsgatan 5, Göteborg, Västra Götaland, 411 19, Sweden".into()];
        assert!(write_vcard(&full).contains("ADR:;;Kungsgatan 5;Göteborg;Västra Götaland;411 19;Sweden\r\n"));
        // Escaped semicolons stay in their part; quoted colons don't split.
        let tricky = parse_vcards("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:X\r\nN:O\\;Brien;Ann;;;\r\nEMAIL;X-NOTE=\"a:b\":ann@x.org\r\nNOTE;ENCODING=QUOTED-PRINTABLE;CHARSET=ISO-8859-1:Caf=E9\r\nEND:VCARD\r\n");
        assert_eq!(tricky[0].emails, ["ann@x.org"]);
        assert_eq!(tricky[0].note, "Café");
        let back = parse_vcards(&written).remove(0);
        assert_eq!(back.addresses, ["Kungsgatan 5, Göteborg"]);
        assert_eq!(back.birthday, "--01-31");
        let mut group = cards[1].clone();
        group.members.push("cy".into());
        let written = write_vcard(&group);
        assert_eq!(written.matches("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:").count(), 2);
        let new_group = Card {
            uid: "g2".into(),
            name: "Team".into(),
            group: true,
            members: vec!["a".into()],
            ..Default::default()
        };
        assert!(write_vcard(&new_group).contains("X-ADDRESSBOOKSERVER-KIND:group"));
    }

    #[test]
    fn edits_keep_labels_names_and_organizations() {
        let original = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:García López;María;José;Dr.;PhD\r\nFN:Dr. María José García López\r\nitem1.EMAIL;TYPE=INTERNET,pref:maria@example.es\r\nitem1.X-ABLabel:_$!<Work>!$_\r\nitem2.EMAIL;TYPE=INTERNET:old@example.es\r\nitem2.X-ABLabel:School\r\nTEL;TYPE=CELL:+34 600 000 000\r\nORG:Acme;Research\r\nUID:m\r\nEND:VCARD\r\n";
        let mut card = parse_vcards(original).remove(0);
        card.emails.retain(|e| e != "old@example.es");
        card.emails.push("new@example.es".into());
        card.note = "Met in Madrid".into();
        let written = write_vcard(&card);
        // What wasn't edited is as it was, labels and all.
        assert!(written.contains("N:García López;María;José;Dr.;PhD\r\n"));
        assert!(written.contains("item1.EMAIL;TYPE=INTERNET,pref:maria@example.es\r\n"));
        assert!(written.contains("item1.X-ABLabel:_$!<Work>!$_\r\n"));
        assert!(written.contains("TEL;TYPE=CELL:+34 600 000 000\r\n"));
        assert!(written.contains("ORG:Acme;Research\r\n"));
        // The removed address goes with its label; the new one is added.
        assert!(!written.contains("old@example.es") && !written.contains("School"));
        assert!(written.contains("EMAIL:new@example.es\r\n"));
        assert!(written.contains("NOTE:Met in Madrid\r\n"));
        assert_eq!(written.matches("UID:").count(), 1);
        assert_eq!(written.matches("FN:").count(), 1);
        let back = parse_vcards(&written).remove(0);
        assert_eq!(back.emails, ["maria@example.es", "new@example.es"]);
        assert_eq!(back.org, "Acme, Research");
        // A changed name is written anew, once.
        let mut renamed = back.clone();
        renamed.name = "María García".into();
        let written = write_vcard(&renamed);
        assert!(written.contains("FN:María García\r\n") && written.contains("N:García;María;;;"));
        assert_eq!(written.matches("\r\nN:").count(), 1);
    }

    #[test]
    fn cards_are_found_read_saved_and_removed() -> Result<()> {
        let (port, server) = dav::test_server(8, |method, path, _| {
            match (method, path) {
            ("PROPFIND", "/.well-known/carddav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/.well-known/carddav</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/principals/ana/</d:href></d:current-user-principal><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/principals/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/principals/ana/</d:href><d:propstat><d:prop><c:addressbook-home-set><d:href>/books/ana/</d:href></c:addressbook-home-set></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/books/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/books/ana/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/books/ana/default/</d:href><d:propstat><d:prop><d:displayname>Contacts</d:displayname><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", "/books/ana/default/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/books/ana/default/1.vcf</d:href><d:propstat><d:prop><d:getetag>\"v1\"</d:getetag><c:address-data>BEGIN:VCARD&#13;\nVERSION:3.0&#13;\nFN:Bo Lindqvist&#13;\nEMAIL:bo@example.se&#13;\nUID:1&#13;\nEND:VCARD&#13;\n</c:address-data></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("PUT", _) => ("201 Created", String::new()),
            ("DELETE", _) => ("204 No Content", String::new()),
            _ => ("404 Not Found", String::new()),
        }
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let mut srv = Server {
            url: format!("http://127.0.0.1:{port}"),
            user: "ana".into(),
            ..Default::default()
        };
        let (cards, book) = fetch(&srv, "secret", tls())?;
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].name, "Bo Lindqvist");
        assert_eq!(cards[0].etag, "\"v1\"");
        assert!(cards[0].href.ends_with("/books/ana/default/1.vcf"));
        assert!(book.ends_with("/books/ana/default/"));
        srv.book = book;
        let mut edited = cards[0].clone();
        edited.phones.push("+46 8 123".into());
        let saved = save(&srv, "secret", tls(), &edited)?;
        assert_eq!(saved.etag, "\"e-5\"");
        let new = save(
            &srv,
            "secret",
            tls(),
            &Card {
                uid: "new 1".into(),
                name: "Cy".into(),
                emails: vec!["cy@x.net".into()],
                ..Default::default()
            },
        )?;
        assert!(new.href.ends_with("/books/ana/default/new_1.vcf"));
        delete(&srv, "secret", tls(), &saved)?;
        let seen = server.join().unwrap();
        // The well-known address, as a collection and then for its principal.
        assert_eq!(seen[0].1, "/.well-known/carddav");
        assert_eq!(seen[1].1, "/.well-known/carddav");
        assert_eq!(seen[5].0, "PUT");
        assert!(seen[5].2.contains("TEL:+46 8 123"));
        assert_eq!(seen[6].1, "/books/ana/default/new_1.vcf");
        assert_eq!(seen[7].0, "DELETE");
        Ok(())
    }
}
