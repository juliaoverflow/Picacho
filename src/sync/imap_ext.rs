//! IMAP extensions beyond the basics in `imap.rs`: ENABLE (QRESYNC,
//! UTF8=ACCEPT, UIDONLY), compact and windowed searches (ESEARCH), SORT and THREAD,
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
    /// UIDONLY (RFC 9586): the server reports messages by UID (UIDFETCH,
    /// VANISHED) and refuses sequence numbers, which Picacho never sends.
    pub(crate) uidonly: bool,
    /// IMAP4rev2 (RFC 9051) was turned on for a server offering both.
    pub(crate) rev2: bool,
}
/// Turns on QRESYNC (RFC 7162), UTF8=ACCEPT (RFC 6855) and UIDONLY (RFC
/// 9586) where offered. Must come before any SELECT. A refusal turns on
/// nothing.
pub(crate) fn enable(session: &mut ImapSession, ext: &Extensions) -> Result<Enabled> {
    // IMAP4rev2's mailbox names are UTF-8 (RFC 9051 §5.1), as UTF8=ACCEPT's.
    let base = Enabled {
        utf8: ext.rev2,
        ..Enabled::default()
    };
    if !ext.enable {
        return Ok(base);
    }
    let mut wanted = Vec::new();
    // rev2 first: its replies (ESEARCH for SEARCH, UTF-8 names, no
    // \Recent) are those Picacho already reads from rev2-only servers.
    if ext.offers_rev2 {
        wanted.push("IMAP4rev2");
    }
    if ext.qresync {
        wanted.push("QRESYNC");
    }
    if ext.utf8_accept && !ext.rev2 && !ext.offers_rev2 {
        wanted.push("UTF8=ACCEPT");
    }
    // Every command Picacho sends names messages by UID already; UIDONLY
    // spares the server its sequence numbers. Its replies come as
    // UIDFETCH, which only a parser with Picacho-Patches' imap-proto patch
    // 0006 reads; without it, every fetch would fail.
    if ext.uidonly && reads_uidfetch() {
        wanted.push("UIDONLY");
    }
    if wanted.is_empty() {
        return Ok(base);
    }
    let data = match session.run_command_and_read_response(format!("ENABLE {}", wanted.join(" "))) {
        Ok(data) => data,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => {
            return Ok(base);
        }
        Err(e) => return Err(e.into()),
    };
    let mut enabled = base;
    for line in other_lines(&data, "ENABLED")? {
        for word in line.split_ascii_whitespace() {
            enabled.qresync |= word.eq_ignore_ascii_case("QRESYNC");
            enabled.utf8 |= word.eq_ignore_ascii_case("UTF8=ACCEPT");
            enabled.uidonly |= word.eq_ignore_ascii_case("UIDONLY");
            enabled.rev2 |= word.eq_ignore_ascii_case("IMAP4rev2");
        }
    }
    // rev2's mailbox names are UTF-8 (RFC 9051 §5.1).
    enabled.utf8 |= enabled.rev2;
    Ok(enabled)
}
/// Whether the IMAP parser reads UIDONLY's `UIDFETCH` replies as UID
/// FETCH replies (RFC 9586; Picacho-Patches' imap-proto patch 0006).
pub(crate) fn reads_uidfetch() -> bool {
    matches!(
        imap_proto::parse_response(b"* 9 UIDFETCH (FLAGS ())\r\n"),
        Ok((rest, imap_proto::Response::Fetch(9, attributes)))
            if rest.is_empty() && attributes.contains(&imap_proto::AttributeValue::Uid(9))
    )
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

/// One mailbox's part of a MULTISEARCH reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Found {
    /// The mailbox as the session names it (UTF-8 or modified UTF-7).
    pub(crate) mailbox: String,
    pub(crate) validity: u32,
    pub(crate) uids: Vec<u32>,
    pub(crate) count: usize,
}
/// One search across `mailboxes` (MULTISEARCH, RFC 7377): `ESEARCH IN
/// (mailboxes …) RETURN (COUNT ALL) criteria`, without selecting each.
/// `names` are as the session takes them, quoted. Each mailbox's UIDs
/// come with its UIDVALIDITY, so they can name cached mail.
pub(crate) fn multisearch(
    session: &mut ImapSession,
    names: &[String],
    criteria: &str,
    utf8: bool,
) -> Result<Vec<Found>> {
    ensure!(!criteria.contains(['\r', '\n']) || criteria.contains("}\r\n"), "Invalid IMAP search");
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let charset = search_charset(criteria, utf8);
    let data = session.run_command_and_read_response(format!(
        "ESEARCH IN (mailboxes ({})) RETURN (COUNT ALL) {charset}{criteria}",
        names.join(" ")
    ))?;
    parse_multisearch(&data)
}
/// `* ESEARCH (TAG "a" MAILBOX "INBOX" UIDVALIDITY 9) UID COUNT 3 ALL 4:6`
fn parse_multisearch(data: &[u8]) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for line in other_lines(data, "ESEARCH")? {
        let tokens = tokens(line);
        let text = |i: usize| match tokens.get(i) {
            Some(Token::Text(t)) => Some(t.as_str()),
            _ => None,
        };
        let mut found = Found::default();
        let mut i = 0;
        // The correlator: (TAG "…" MAILBOX "…" UIDVALIDITY n)
        if tokens.first() == Some(&Token::Open) {
            i = 1;
            while i < tokens.len() && tokens[i] != Token::Close {
                match text(i).map(str::to_ascii_uppercase).as_deref() {
                    Some("MAILBOX") => found.mailbox = text(i + 1).unwrap_or_default().to_owned(),
                    Some("UIDVALIDITY") => {
                        found.validity = text(i + 1).and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    _ => {}
                }
                i += 1;
            }
            i += 1;
        }
        while i < tokens.len() {
            match (text(i).map(str::to_ascii_uppercase).as_deref(), text(i + 1)) {
                (Some("COUNT"), Some(n)) => found.count = n.parse().unwrap_or(0),
                (Some("ALL"), Some(set)) => found.uids = parse_set(set)?,
                _ => {}
            }
            i += 1;
        }
        if found.mailbox.is_empty() || found.validity == 0 {
            continue;
        }
        if found.count == 0 {
            found.count = found.uids.len();
        }
        out.push(found);
    }
    Ok(out)
}

/// UID SEARCH that also keeps its result on the server as `$` (SEARCHRES,
/// RFC 5182), for the next commands to act on exactly those messages; it
/// needs ESEARCH's form of reply. Returns the UIDs.
pub(crate) fn uid_search_saved(session: &mut ImapSession, criteria: &str) -> Result<Vec<u32>> {
    let charset = search_charset(criteria, false);
    let data = session.run_command_and_read_response(format!(
        "UID SEARCH RETURN (SAVE ALL) {charset}{criteria}"
    ))?;
    let mut uids = esearch_uids(&data)?;
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

/// UID SEARCH as the imap crate's `uid_search`, answered with ESEARCH
/// where the server has it (and must, speaking IMAP4rev2, whose SEARCH
/// replies are ESEARCH, which the imap crate doesn't read). Refusals come
/// back as the crate's `No` and `Bad`, as from `uid_search`.
pub(crate) fn uid_search_any(
    session: &mut ImapSession,
    esearch: bool,
    criteria: &str,
) -> imap::error::Result<std::collections::HashSet<u32>> {
    if !esearch {
        return session.uid_search(criteria);
    }
    let data =
        session.run_command_and_read_response(format!("UID SEARCH RETURN (ALL) {criteria}"))?;
    esearch_uids(&data)
        .map(|uids| uids.into_iter().collect())
        .map_err(|e| imap::error::Error::Bad(format!("{e:#}")))
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

/// The newest `limit` messages matching `criteria` (newest first) and how
/// many match in all. Sorted by the server where it can; otherwise, where
/// it has PARTIAL (RFC 9394), only the newest page is asked for, so a
/// search matching most of a large mailbox doesn't bring back every UID.
pub(crate) fn newest_search(
    session: &mut ImapSession,
    ext: &Extensions,
    selected: &Selected,
    criteria: &str,
    utf8: bool,
    limit: usize,
) -> Result<(Vec<u32>, usize)> {
    // Sorted by date with one page asked for (ESORT and PARTIAL, RFC 5267
    // and 9394): the newest `limit` and the count, without every match's
    // UID. ESORT's set loses the order, so the page is sorted again; it is
    // only `limit` UIDs.
    if ext.sort && ext.esort && ext.partial && limit > 0 {
        let charset = if utf8 || !criteria.is_ascii() { "UTF-8" } else { "US-ASCII" };
        let data = session.run_command_and_read_response(format!(
            "UID SORT RETURN (COUNT PARTIAL 1:{limit}) (REVERSE DATE) {charset} {criteria}"
        ))?;
        let (page, count) = partial_uids(&data)?;
        let count = count.unwrap_or(page.len());
        if page.is_empty() {
            return Ok((page, count));
        }
        let set = super::imap::uid_ranges(&page);
        let ordered = sorted_search(session, ext, selected, &format!("UID {set}"), utf8)?;
        return Ok((ordered.into_iter().take(limit).collect(), count));
    }
    if !ext.sort && ext.partial && ext.esearch && limit > 0 {
        let charset = search_charset(criteria, utf8);
        let data = session.run_command_and_read_response(format!(
            "UID SEARCH RETURN (COUNT PARTIAL -1:-{limit}) {charset}{criteria}"
        ))?;
        let (mut uids, count) = partial_uids(&data)?;
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.truncate(limit);
        let count = count.unwrap_or(uids.len());
        return Ok((uids, count));
    }
    let uids = sorted_search(session, ext, selected, criteria, utf8)?;
    let total = uids.len();
    Ok((uids.into_iter().take(limit).collect(), total))
}

/// The UIDs and match count of an ESEARCH reply with PARTIAL and COUNT:
/// `* ESEARCH (TAG "a1") UID COUNT 120 PARTIAL (-1:-100 3:90,200)`.
fn partial_uids(data: &[u8]) -> Result<(Vec<u32>, Option<usize>)> {
    let mut uids = Vec::new();
    let mut count = None;
    for line in other_lines(data, "ESEARCH")? {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        if let Some(i) = words.iter().position(|w| w.eq_ignore_ascii_case("COUNT")) {
            count = words.get(i + 1).and_then(|n| n.parse().ok());
        }
        if let Some(i) = words.iter().position(|w| w.eq_ignore_ascii_case("PARTIAL"))
            && let Some(set) = words.get(i + 2)
        {
            let set = set.trim_end_matches(')');
            if !set.eq_ignore_ascii_case("NIL") && !set.is_empty() {
                uids.extend(parse_set(set)?);
            }
        }
    }
    Ok((uids, count))
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
            partial_uids(b"* ESEARCH (TAG \"a8\") UID COUNT 120 PARTIAL (-1:-3 90,95:96)\r\n")
                .unwrap(),
            (vec![90, 95, 96], Some(120))
        );
        assert_eq!(
            partial_uids(b"* ESEARCH (TAG \"a9\") UID COUNT 0 PARTIAL (-1:-3 NIL)\r\n").unwrap(),
            (vec![], Some(0))
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
