//! IMAP extensions beyond the basics in `imap.rs`: ENABLE (QRESYNC,
//! UTF8=ACCEPT), compact and windowed searches (ESEARCH), SORT and THREAD,
//! QRESYNC's resynchronizing SELECT, METADATA, NOTIFY, message structure
//! (BODYSTRUCTURE), single parts (BINARY) and partial downloads.
//!
//! These helpers have no database or UI access, like the rest of Sync.
use super::imap::{
    Extensions, ImapSession, Row, Selected, Token, decode_name, encode_name, other_lines, quote,
    responses, tokens, uid_fetch_rows,
};
use anyhow::{Context, Result, bail, ensure};

/// What ENABLE turned on for a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Enabled {
    pub(crate) qresync: bool,
    pub(crate) utf8: bool,
}
/// Turns on QRESYNC (RFC 7162) and UTF8=ACCEPT (RFC 6855) where offered.
/// Must come before any SELECT. A refusal turns on nothing.
pub(crate) fn enable(session: &mut ImapSession, ext: &Extensions) -> Result<Enabled> {
    if !ext.enable {
        return Ok(Enabled::default());
    }
    let mut wanted = Vec::new();
    if ext.qresync {
        wanted.push("QRESYNC");
    }
    if ext.utf8_accept {
        wanted.push("UTF8=ACCEPT");
    }
    if wanted.is_empty() {
        return Ok(Enabled::default());
    }
    let data = match session.run_command_and_read_response(format!("ENABLE {}", wanted.join(" "))) {
        Ok(data) => data,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => {
            return Ok(Enabled::default());
        }
        Err(e) => return Err(e.into()),
    };
    let mut enabled = Enabled::default();
    for line in other_lines(&data, "ENABLED")? {
        for word in line.split_ascii_whitespace() {
            enabled.qresync |= word.eq_ignore_ascii_case("QRESYNC");
            enabled.utf8 |= word.eq_ignore_ascii_case("UTF8=ACCEPT");
        }
    }
    Ok(enabled)
}

/// A mailbox name as the session takes it: the cache always keeps modified
/// UTF-7 (as servers without UTF8=ACCEPT send it); with UTF8=ACCEPT the
/// wire carries plain UTF-8, where `&` is no escape.
pub(crate) fn wire_name(name: &str, utf8: bool) -> String {
    if utf8 {
        decode_name(name)
    } else {
        name.to_owned()
    }
}
/// A name from a UTF8=ACCEPT session in the form the cache keeps.
pub(crate) fn cache_name(name: &str, utf8: bool) -> String {
    if utf8 {
        encode_name(name)
    } else {
        name.to_owned()
    }
}

/// A sequence set such as `1:5,9` as its numbers, refusing sets that would
/// be absurdly large (a broken or hostile server).
pub(crate) fn parse_set(set: &str) -> Result<Vec<u32>> {
    const MOST: usize = 20_000_000;
    let mut out = Vec::new();
    for part in set.split(',').filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once(':') {
            Some((a, b)) => (a, b),
            None => (part, part),
        };
        let (a, b): (u32, u32) = (
            a.trim().parse().context("Invalid IMAP sequence set")?,
            b.trim().parse().context("Invalid IMAP sequence set")?,
        );
        let (lo, hi) = (a.min(b), a.max(b));
        ensure!(lo > 0, "Invalid IMAP UID 0");
        ensure!(
            out.len() + (hi - lo) as usize <= MOST,
            "The server's search result is too large"
        );
        out.extend(lo..=hi);
    }
    Ok(out)
}

/// The UIDs an ESEARCH reply's `ALL` lists (RFC 4731).
fn esearch_uids(data: &[u8]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for line in other_lines(data, "ESEARCH")? {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        if let Some(i) = words.iter().position(|w| w.eq_ignore_ascii_case("ALL"))
            && let Some(set) = words.get(i + 1)
        {
            out.extend(parse_set(set)?);
        }
    }
    Ok(out)
}

/// UIDs of the selected mailbox's messages matching `criteria` (UID
/// SEARCH syntax, ASCII), oldest first. With ESEARCH the reply is a
/// compact set (`1:90000`), so even a very large mailbox fits in one reply.
/// Without it, a large mailbox is searched a window of UIDs at a time, so
/// no single reply outgrows the connection's size limit.
pub(crate) fn search_uids(
    session: &mut ImapSession,
    ext: &Extensions,
    selected: &Selected,
    criteria: &str,
) -> Result<Vec<u32>> {
    ensure!(!criteria.contains(['\r', '\n']), "Invalid IMAP search");
    let mut uids = if ext.esearch {
        let data =
            session.run_command_and_read_response(format!("UID SEARCH RETURN (ALL) {criteria}"))?;
        esearch_uids(&data)?
    } else {
        // Each UID costs up to 11 bytes in a SEARCH reply; windows of
        // 500,000 stay far below the 32 MB limit.
        const WINDOW: u32 = 500_000;
        let top = selected.uid_next.unwrap_or(0);
        if selected.exists <= WINDOW || top == 0 {
            session.uid_search(criteria)?.into_iter().collect()
        } else {
            let mut all = Vec::new();
            let mut start = 1u32;
            while start < top {
                let end = start.saturating_add(WINDOW - 1).min(top - 1);
                all.extend(
                    session
                        .uid_search(format!("UID {start}:{end} {criteria}"))?
                        .into_iter()
                        .filter(|uid| (start..=end).contains(uid)),
                );
                start = end.saturating_add(1);
            }
            // Anything that arrived meanwhile is above UIDNEXT; the next
            // check lists it.
            all
        }
    };
    ensure!(
        uids.iter().all(|uid| *uid > 0),
        "Invalid IMAP UID in search response"
    );
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

/// A search term's text as an IMAP string the session accepts: quoted
/// where it can be (ASCII, or any text once UTF8=ACCEPT is on), else a
/// literal that needs no go-ahead (LITERAL+), else quoted UTF-8, which
/// most servers searching in UTF-8 accept.
pub(crate) fn search_string(text: &str, ext: &Extensions, utf8: bool) -> Result<String> {
    let text = text.replace(['\r', '\n'], " ");
    if text.is_ascii() || utf8 || !ext.literal_plus {
        return quote(&text);
    }
    Ok(format!("{{{}+}}\r\n{text}", text.len()))
}
/// `CHARSET UTF-8 ` when a search has non-ASCII text and UTF8=ACCEPT is
/// off (with it on, a CHARSET is not used).
pub(crate) fn search_charset(criteria: &str, utf8: bool) -> &'static str {
    if !utf8 && !criteria.is_ascii() {
        "CHARSET UTF-8 "
    } else {
        ""
    }
}

/// UIDs matching `criteria`, newest first, ordered by the server (SORT,
/// RFC 5256) when it can; else by UID.
pub(crate) fn sorted_search(
    session: &mut ImapSession,
    ext: &Extensions,
    selected: &Selected,
    criteria: &str,
    utf8: bool,
) -> Result<Vec<u32>> {
    if ext.sort {
        let charset = if utf8 {
            "UTF-8"
        } else if criteria.is_ascii() {
            "US-ASCII"
        } else {
            "UTF-8"
        };
        let data = session.run_command_and_read_response(format!(
            "UID SORT (REVERSE DATE) {charset} {criteria}"
        ))?;
        let mut out = Vec::new();
        for line in other_lines(&data, "SORT")? {
            for word in line.split_ascii_whitespace() {
                out.push(word.parse::<u32>().context("Invalid IMAP SORT response")?);
            }
        }
        return Ok(out);
    }
    let charset = search_charset(criteria, utf8);
    let mut uids = search_uids(session, ext, selected, &format!("{charset}{criteria}"))?;
    uids.reverse();
    Ok(uids)
}

/// Conversations among the selected mailbox's messages matching
/// `criteria`, as the server threads them (THREAD=REFERENCES): each
/// thread's UIDs, in no particular order.
pub(crate) fn threads(session: &mut ImapSession, criteria: &str) -> Result<Vec<Vec<u32>>> {
    let data =
        session.run_command_and_read_response(format!("UID THREAD REFERENCES UTF-8 {criteria}"))?;
    let mut out = Vec::new();
    for line in other_lines(&data, "THREAD")? {
        out.extend(parse_threads(line)?);
    }
    Ok(out)
}
/// `(2)(3 6 (4 23)(44 7 96))` as `[[2], [3, 6, 4, 23, 44, 7, 96]]`: every
/// top-level parenthesis is one conversation, however it branches.
fn parse_threads(line: &str) -> Result<Vec<Vec<u32>>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut current: Vec<u32> = Vec::new();
    let mut number = String::new();
    let flush = |number: &mut String, current: &mut Vec<u32>| -> Result<()> {
        if !number.is_empty() {
            current.push(number.parse().context("Invalid IMAP THREAD response")?);
            number.clear();
        }
        Ok(())
    };
    for c in line.chars() {
        match c {
            '(' => {
                flush(&mut number, &mut current)?;
                depth += 1;
            }
            ')' => {
                flush(&mut number, &mut current)?;
                ensure!(depth > 0, "Invalid IMAP THREAD response");
                depth -= 1;
                if depth == 0 && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            ' ' => flush(&mut number, &mut current)?,
            d if d.is_ascii_digit() => number.push(d),
            _ => bail!("Invalid IMAP THREAD response"),
        }
    }
    ensure!(depth == 0, "Invalid IMAP THREAD response");
    Ok(out)
}

/// What a resynchronizing SELECT (QRESYNC) reported besides the counts:
/// UIDs removed since the given modseq, and the flags of messages changed.
#[derive(Debug, Default)]
pub(crate) struct Resync {
    pub(crate) vanished: Vec<u32>,
    pub(crate) changed: Vec<Row>,
}
/// SELECT with `(QRESYNC (validity modseq))` (RFC 7162). Where the
/// validity still matches, the server answers with VANISHED (EARLIER) for
/// what was removed and FETCH for what changed since `modseq`. Returns
/// the selected state, and the resync when the validity matched.
pub(crate) fn select_qresync(
    session: &mut ImapSession,
    name: &str,
    utf8: bool,
    validity: u32,
    modseq: u64,
) -> Result<(Selected, Option<Resync>)> {
    let wire = wire_name(name, utf8);
    let command = format!("SELECT {} (QRESYNC ({validity} {modseq}))", quote(&wire)?);
    let data = session.run_command_and_read_response(command)?;
    let selected = super::imap::selected_from(&data, name, true)?;
    if selected.validity != validity {
        return Ok((selected, None));
    }
    let mut resync = Resync::default();
    for line in other_lines(&data, "VANISHED")? {
        let set = line
            .trim_start()
            .strip_prefix("(EARLIER)")
            .or_else(|| line.trim_start().strip_prefix("(earlier)"))
            .unwrap_or(line)
            .trim();
        resync.vanished.extend(parse_set(set)?);
    }
    resync.changed = super::imap::rows_from(&data)?;
    Ok((selected, Some(resync)))
}

/// A server or mailbox annotation (METADATA, RFC 5464), such as
/// `/shared/comment` or `/private/vendor/…`. `mailbox` "" is the server.
pub(crate) fn get_metadata(
    session: &mut ImapSession,
    mailbox: &str,
    entries: &[&str],
) -> Result<Vec<(String, Option<String>)>> {
    ensure!(
        entries
            .iter()
            .all(|e| e.starts_with('/') && !e.contains([' ', '\r', '\n', '"'])),
        "Invalid METADATA entry"
    );
    let data = match session.run_command_and_read_response(format!(
        "GETMETADATA {} ({})",
        quote(mailbox)?,
        entries.join(" ")
    )) {
        Ok(data) => data,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for response in responses(&data)? {
        if let imap_proto::Response::MailboxData(imap_proto::MailboxDatum::MetadataSolicited {
            values,
            ..
        }) = response
        {
            out.extend(values.into_iter().map(|m| (m.entry, m.value)));
        }
    }
    Ok(out)
}
/// Sets (or, with `None`, removes) an annotation. Returns whether the
/// server kept it.
pub(crate) fn set_metadata(
    session: &mut ImapSession,
    mailbox: &str,
    entry: &str,
    value: Option<&str>,
) -> Result<bool> {
    ensure!(
        entry.starts_with('/') && !entry.contains([' ', '\r', '\n', '"']),
        "Invalid METADATA entry"
    );
    let value = match value {
        Some(v) => quote(v)?,
        None => "NIL".into(),
    };
    super::imap::optional(
        session
            .run_command_and_check_ok(format!("SETMETADATA {} ({entry} {value})", quote(mailbox)?)),
    )
}

/// Asks the server to report changes in the selected mailbox and every
/// personal one over this connection (NOTIFY, RFC 5465). Returns whether
/// it agreed.
pub(crate) fn notify_personal(session: &mut ImapSession) -> Result<bool> {
    super::imap::optional(session.run_command_and_check_ok(
        "NOTIFY SET STATUS (SELECTED (MessageNew MessageExpunge FlagChange)) \
         (PERSONAL (MessageNew MessageExpunge FlagChange))",
    ))
}
/// The mailboxes a NOTIFY session's untagged STATUS lines name, as the
/// cache keeps their names.
pub(crate) fn status_mailboxes(heard: &[u8], utf8: bool) -> Vec<String> {
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(heard).lines() {
        let Some(rest) = line
            .strip_prefix("* STATUS ")
            .or_else(|| line.strip_prefix("* status "))
        else {
            continue;
        };
        let name = match tokens(rest).into_iter().next() {
            Some(Token::Text(name)) => name,
            _ => continue,
        };
        let name = cache_name(&name, utf8);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// One leaf of a message's structure (BODYSTRUCTURE), in the order the
/// MIME parser lists leaves, with its section number for fetching it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Leaf {
    pub(crate) section: String,
    pub(crate) mime: String,
    pub(crate) charset: Option<String>,
    pub(crate) encoding: String,
    pub(crate) size: usize,
    pub(crate) filename: Option<String>,
    pub(crate) attachment: bool,
    pub(crate) content_id: Option<String>,
}
/// The leaves of a message's structure, depth first.
pub(crate) fn structure(session: &mut ImapSession, uid: u32) -> Result<Vec<Leaf>> {
    let fetched = session.uid_fetch(uid.to_string(), "(UID BODYSTRUCTURE)")?;
    let bs = fetched
        .iter()
        .filter(|f| f.uid == Some(uid))
        .find_map(|f| f.bodystructure())
        .context("The server didn't describe this message's parts.")?;
    let mut out = Vec::new();
    leaves(bs, "", &mut out);
    Ok(out)
}
fn leaves(bs: &imap_proto::BodyStructure<'_>, prefix: &str, out: &mut Vec<Leaf>) {
    use imap_proto::BodyStructure as B;
    let section = |i: usize| {
        if prefix.is_empty() {
            format!("{}", i + 1)
        } else {
            format!("{prefix}.{}", i + 1)
        }
    };
    match bs {
        B::Multipart { bodies, .. } => {
            for (i, body) in bodies.iter().enumerate() {
                match body {
                    B::Multipart { .. } => leaves(body, &section(i), out),
                    _ => out.push(leaf(body, section(i))),
                }
            }
        }
        // A message of one part: its body is section 1.
        single => out.push(leaf(
            single,
            if prefix.is_empty() {
                "1".into()
            } else {
                prefix.into()
            },
        )),
    }
}
fn leaf(bs: &imap_proto::BodyStructure<'_>, section: String) -> Leaf {
    use imap_proto::{BodyStructure as B, ContentEncoding as E};
    let (common, other) = match bs {
        B::Basic { common, other, .. }
        | B::Text { common, other, .. }
        | B::Message { common, other, .. } => (common, Some(other)),
        B::Multipart { common, .. } => (common, None),
    };
    let param = |params: &imap_proto::BodyParams<'_>, key: &str| {
        params.as_ref().and_then(|p| {
            p.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| mailparse_decode(v))
        })
    };
    let disposition = common.disposition.as_ref();
    let filename = disposition
        .and_then(|d| param(&d.params, "filename"))
        .or_else(|| param(&common.ty.params, "name"));
    let attachment = disposition.is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"))
        || matches!(bs, B::Message { .. });
    Leaf {
        section,
        mime: format!("{}/{}", common.ty.ty, common.ty.subtype).to_ascii_lowercase(),
        charset: param(&common.ty.params, "charset"),
        encoding: match other.map(|o| &o.transfer_encoding) {
            Some(E::Base64) => "base64".into(),
            Some(E::QuotedPrintable) => "quoted-printable".into(),
            Some(E::Binary) => "binary".into(),
            Some(E::EightBit) => "8bit".into(),
            Some(E::Other(o)) => o.to_ascii_lowercase(),
            _ => "7bit".into(),
        },
        size: other.map_or(0, |o| o.octets as usize),
        filename,
        attachment,
        content_id: other.and_then(|o| o.id).map(str::to_owned),
    }
}
/// A parameter value, with RFC 2047 encoded words decoded.
fn mailparse_decode(value: &str) -> String {
    let header = format!("X: {value}\r\n\r\n");
    mailparse::parse_header(header.as_bytes())
        .map(|(h, _)| h.get_value())
        .unwrap_or_else(|_| value.to_owned())
}

/// One part of a message, decoded: by the server where it can (BINARY,
/// RFC 3516), else fetched as it is and decoded here.
pub(crate) fn fetch_part(
    session: &mut ImapSession,
    ext: &Extensions,
    uid: u32,
    leaf: &Leaf,
    limit: usize,
) -> Result<Vec<u8>> {
    ensure!(
        leaf.section
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.'),
        "Invalid IMAP section"
    );
    ensure!(
        leaf.size <= limit,
        "This part is larger than Picacho downloads ({} MB).",
        limit / (1024 * 1024)
    );
    if ext.binary {
        let data = session.run_command_and_read_response(format!(
            "UID FETCH {uid} (UID BINARY.PEEK[{}])",
            leaf.section
        ));
        match data {
            Ok(data) => {
                for response in responses(&data)? {
                    if let imap_proto::Response::Fetch(_, attributes) = response {
                        let ours = attributes.contains(&imap_proto::AttributeValue::Uid(uid));
                        for a in &attributes {
                            if let imap_proto::AttributeValue::Binary { data: Some(d), .. } = a
                                && ours
                            {
                                return Ok(d.to_vec());
                            }
                        }
                    }
                }
            }
            // [UNKNOWN-CTE]: the server can't decode it; do it here.
            Err(imap::error::Error::No(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    let rows = uid_fetch_rows(
        session,
        &uid.to_string(),
        &format!("(UID BODY.PEEK[{}])", leaf.section),
    )?;
    let raw = rows
        .into_iter()
        .find(|r| r.uid == uid)
        .and_then(|r| r.header)
        .context("The message no longer exists on the server")?;
    decode_part(&raw, &leaf.encoding)
}
/// A part's body in its transfer encoding, decoded.
pub(crate) fn decode_part(raw: &[u8], encoding: &str) -> Result<Vec<u8>> {
    let mut wrapped = format!(
        "Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n"
    )
    .into_bytes();
    wrapped.extend_from_slice(raw);
    Ok(mailparse::parse_mail(&wrapped)?.get_body_raw()?)
}

/// Part of a message's source: `length` bytes from `offset`
/// (`BODY.PEEK[]<offset.length>`), for downloads that resume.
pub(crate) fn fetch_range(
    session: &mut ImapSession,
    uid: u32,
    offset: usize,
    length: usize,
) -> Result<Vec<u8>> {
    let rows = uid_fetch_rows(
        session,
        &uid.to_string(),
        &format!("(UID BODY.PEEK[]<{offset}.{length}>)"),
    )?;
    rows.into_iter()
        .find(|r| r.uid == uid)
        .and_then(|r| r.header)
        .context("The message no longer exists on the server")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_threads_and_notify_statuses() {
        assert_eq!(parse_set("1:3,7,9:8").unwrap(), vec![1, 2, 3, 7, 8, 9]);
        assert!(parse_set("0").is_err());
        assert!(parse_set("1:4294967295").is_err());
        assert_eq!(
            parse_threads("(2)(3 6 (4 23)(44 7 96))").unwrap(),
            vec![vec![2], vec![3, 6, 4, 23, 44, 7, 96]]
        );
        assert!(parse_threads("(2").is_err());
        assert_eq!(
            esearch_uids(b"* ESEARCH (TAG \"a7\") UID ALL 4:6,10\r\n").unwrap(),
            vec![4, 5, 6, 10]
        );
        assert_eq!(
            status_mailboxes(
                b"* STATUS \"Caf&AOk-\" (MESSAGES 3)\r\n* STATUS Work (UIDNEXT 9)\r\n",
                false
            ),
            vec!["Caf&AOk-".to_owned(), "Work".to_owned()]
        );
        assert_eq!(
            status_mailboxes("* STATUS \"Café\" (MESSAGES 3)\r\n".as_bytes(), true),
            vec!["Caf&AOk-".to_owned()]
        );
        assert_eq!(decode_part(b"aGVsbG8=", "base64").unwrap(), b"hello");
        assert_eq!(wire_name("Caf&AOk-", true), "Café");
        assert_eq!(cache_name("Café", true), "Caf&AOk-");
    }
}
