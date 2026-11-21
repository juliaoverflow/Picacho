//! Address books: vCard (RFC 6350, and the 3.0 cards most servers still
//! keep) and CardDAV (RFC 6352), read and written through `dav`.
//!
//! Picacho shows and edits a card's name, email addresses, phone numbers,
//! organization and note. A card read from a server keeps its original
//! text, and an edit replaces only those properties in it, so photos,
//! addresses, birthdays and the like that other apps keep stay as they were.
use crate::sync::dav;
use anyhow::{Result, ensure};

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
}

/// Lines of a vCard file unfolded (continuation lines start with a space or
/// tab), with CRLF or LF line ends.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
        } else {
            lines.push(line.to_owned());
        }
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
        let Some((head, value)) = line.split_once(':') else {
            continue;
        };
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
                    if !card.emails.is_empty() || !card.name.is_empty() {
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
                        let parts: Vec<String> = value
                            .split(';')
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
                        card.org = value
                            .split(';')
                            .map(|p| unescape(p).trim().to_owned())
                            .filter(|p| !p.is_empty())
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                    "NOTE" => card.note = unescape(value).trim().to_owned(),
                    "UID" => card.uid = unescape(value).trim().to_owned(),
                    _ => {}
                }
            }
        }
    }
    cards
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

/// Folds a line at 75 octets.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars() {
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
    lines
}

/// One card as vCard text. A card read from a server keeps every property
/// Picacho doesn't edit; those it does are written from the card's fields.
pub fn write_vcard(card: &Card) -> String {
    const EDITED: [&str; 7] = ["FN", "N", "EMAIL", "TEL", "ORG", "NOTE", "UID"];
    let mut out = String::new();
    let kept: Vec<String> = unfold(&card.vcard)
        .into_iter()
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let head = l.split(':').next().unwrap_or("");
            let p = property(head);
            !EDITED.contains(&p.as_str()) && p != "BEGIN" && p != "END" && p != "VERSION"
        })
        .collect();
    let version = unfold(&card.vcard)
        .into_iter()
        .find(|l| property(l.split(':').next().unwrap_or("")) == "VERSION")
        .unwrap_or_else(|| "VERSION:3.0".into());
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str(&fold(&version));
    for line in own_lines(card) {
        out.push_str(&fold(&line));
    }
    if !card.uid.is_empty() {
        out.push_str(&fold(&format!("UID:{}", escape(&card.uid))));
    }
    for line in kept {
        out.push_str(&fold(&line));
    }
    out.push_str("END:VCARD\r\n");
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
}

const QUERY: &str = "<?xml version=\"1.0\"?><c:addressbook-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:prop><d:getetag/><c:address-data/></d:prop></c:addressbook-query>";

/// Reads every address book the account can see. Returns the cards, each
/// with its href and ETag, and the address book new cards go to (the first
/// one the account may write to).
pub fn fetch(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
) -> Result<(Vec<Card>, String)> {
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
    for book in &books {
        for resource in client.list(&book.href, QUERY, "address-data")? {
            for mut card in parse_vcards(&resource.data) {
                card.href = resource.href.to_string();
                card.etag = resource.etag.clone();
                cards.push(card);
            }
        }
    }
    let home = books
        .iter()
        .find(|b| b.writable)
        .map(|b| b.href.to_string())
        .unwrap_or_default();
    Ok((cards, home))
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
    let text = write_vcard(card);
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
            book: String::new(),
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
