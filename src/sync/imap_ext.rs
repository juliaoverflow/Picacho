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
    // UTF8=ACCEPT is enabled alongside rev2 too: rev2 alone allows UTF-8
    // mailbox names, but not UTF-8 in quoted strings and headers.
    if ext.utf8_accept {
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
            // UTF8=ONLY servers refuse a session that hasn't enabled
            // UTF-8 (RFC 6855 §7).
            ensure!(
                !ext.utf8_only,
                t!("sync-imap-ext-the-server-only-accepts-utf-8")
            );
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
    ensure!(
        !ext.utf8_only || enabled.utf8,
        t!("sync-imap-ext-the-server-only-accepts-utf-8-2")
    );
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
#[cfg(test)]
pub(crate) fn parse_set(set: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    parse_set_into(set, &mut out, 0)?;
    Ok(out)
}

/// The most UIDs one reply's sets give, together: a mailbox holds far
/// fewer. Counted across all of a reply's lines, which each could give as
/// many before.
const MOST_UIDS: usize = 20_000_000;

/// `parse_set`, added to `out`, which with them (and `elsewhere`, the
/// reply's UIDs kept apart from it) may hold at most `MOST_UIDS`.
fn parse_set_into(set: &str, out: &mut Vec<u32>, elsewhere: usize) -> Result<()> {
    for part in set.split(',').filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once(':') {
            Some((a, b)) => (a, b),
            None => (part, part),
        };
        let (a, b): (u32, u32) = (
            a.trim().parse().with_context(|| t!("sync-imap-ext-invalid-imap-sequence-set"))?,
            b.trim().parse().with_context(|| t!("sync-imap-ext-invalid-imap-sequence-set"))?,
        );
        let (lo, hi) = (a.min(b), a.max(b));
        ensure!(lo > 0, t!("sync-imap-ext-invalid-imap-uid-0"));
        ensure!(
            (out.len() + elsewhere) as u64 + u64::from(hi - lo) < MOST_UIDS as u64,
            t!("sync-imap-ext-the-servers-search-result-is-too")
        );
        out.extend(lo..=hi);
    }
    Ok(())
}

/// The UIDs an ESEARCH reply's `ALL` lists (RFC 4731).
fn esearch_uids(data: &[u8]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for line in other_lines(data, "ESEARCH")? {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        if let Some(i) = words.iter().position(|w| w.eq_ignore_ascii_case("ALL"))
            && let Some(set) = words.get(i + 1)
        {
            parse_set_into(set, &mut out, 0)?;
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
    literals: Option<&super::stream::Literals>,
    names: &[String],
    criteria: &str,
    utf8: bool,
) -> Result<Vec<Found>> {
    ensure!(literals_only(criteria), t!("sync-imap-ext-invalid-imap-search"));
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let charset = search_charset(criteria, utf8);
    let data = run_with_literals(
        session,
        literals,
        &format!(
            "ESEARCH IN (mailboxes ({})) RETURN (COUNT ALL) {charset}{criteria}",
            names.join(" ")
        ),
    )?;
    parse_multisearch(&data)
}
/// `* ESEARCH (TAG "a" MAILBOX "INBOX" UIDVALIDITY 9) UID COUNT 3 ALL 4:6`
fn parse_multisearch(data: &[u8]) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    // UIDs of the mailboxes already read, which count toward the limit.
    let mut listed = 0;
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
                // Keyword and value in pairs: a mailbox named "UIDVALIDITY"
                // is a value, not read as the keyword.
                match text(i).map(str::to_ascii_uppercase).as_deref() {
                    Some("MAILBOX") => {
                        found.mailbox = text(i + 1).unwrap_or_default().to_owned();
                        i += 1;
                    }
                    Some("TAG") => i += 1,
                    Some("UIDVALIDITY") => {
                        found.validity = text(i + 1).and_then(|v| v.parse().ok()).unwrap_or(0);
                        i += 1;
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
                (Some("ALL"), Some(set)) => {
                    found.uids.clear();
                    parse_set_into(set, &mut found.uids, listed)?;
                }
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
        listed += found.uids.len();
        out.push(found);
    }
    Ok(out)
}

/// UID SEARCH that also keeps its result on the server as `$` (SEARCHRES,
/// RFC 5182), for the next commands to act on exactly those messages; it
/// needs ESEARCH's form of reply. Returns the UIDs.
pub(crate) fn uid_search_saved(
    session: &mut ImapSession,
    criteria: &str,
    utf8: bool,
) -> Result<Vec<u32>> {
    let charset = search_charset(criteria, utf8);
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
    literals: Option<&super::stream::Literals>,
    ext: &Extensions,
    selected: &Selected,
    criteria: &str,
) -> Result<Vec<u32>> {
    ensure!(literals_only(criteria), t!("sync-imap-ext-invalid-imap-search"));
    let mut uids = if ext.esearch {
        let data = run_with_literals(session, literals, &format!("UID SEARCH RETURN (ALL) {criteria}"))?;
        esearch_uids(&data)?
    } else if criteria.contains('\n') {
        // A search with literals: its SEARCH reply read here, as the imap
        // crate's `uid_search` would.
        let data = run_with_literals(session, literals, &format!("UID SEARCH {criteria}"))?;
        search_reply_uids(&data)?
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
        t!("sync-imap-ext-invalid-imap-uid-in-search-response")
    );
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

/// The UIDs in a SEARCH reply (`* SEARCH 2 84 882`).
fn search_reply_uids(data: &[u8]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for response in responses(data)? {
        if let imap_proto::Response::IDs(ids) = response {
            out.extend(ids);
        }
    }
    Ok(out)
}
/// Whether every line break in `command` begins a literal (`{n}` or
/// LITERAL+'s `{n+}`, then CRLF and exactly `n` bytes, without line breaks
/// of their own). One literal anywhere let any other line break through
/// before, which would end the command and start another.
fn literals_only(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                // "{n+}" must come just before.
                if bytes[i] != b'\r' || bytes.get(i + 1) != Some(&b'\n') {
                    return false;
                }
                let Some(open) = command[..i].rfind('{') else {
                    return false;
                };
                let Some(n) = command[open + 1..i]
                    .strip_suffix('}')
                    .map(|n| n.strip_suffix('+').unwrap_or(n))
                    .and_then(|n| n.parse::<usize>().ok())
                else {
                    return false;
                };
                let start = i + 2;
                let Some(text) = bytes.get(start..start + n) else {
                    return false;
                };
                if text.iter().any(|b| matches!(b, b'\r' | b'\n')) {
                    return false;
                }
                i = start + n;
            }
            _ => i += 1,
        }
    }
    true
}

/// A search term's text as an IMAP string the session accepts: quoted
/// where it can be (ASCII, or any text once UTF8=ACCEPT is on), else a
/// literal. RFC 9051 §4.3 (RFC 3501 §4.3): a quoted string holds 7-bit
/// text only, so 8-bit text goes as a literal: one needing no go-ahead
/// (LITERAL+, or LITERAL- up to 4096 bytes; RFC 7888), else a synchronizing
/// one, which the command must be sent with `run_with_literals` for.
pub(crate) fn search_string(text: &str, ext: &Extensions, utf8: bool) -> Result<String> {
    let text = text.replace(['\r', '\n'], " ");
    if text.is_ascii() || utf8 {
        return quote(&text);
    }
    // LITERAL- literals hold at most 4096 bytes (RFC 7888 §5).
    let fits = !ext.literal_minus || text.len() <= 4096;
    if ext.literal_plus && fits {
        Ok(format!("{{{}+}}\r\n{text}", text.len()))
    } else {
        Ok(format!("{{{}}}\r\n{text}", text.len()))
    }
}
/// Runs `command`, which may hold synchronizing literals (`{n}` CRLF and
/// n bytes, as `search_string` makes them): each is sent once the server
/// says to go ahead (RFC 9051 §7.5). Without such literals, or without
/// `literals` to send them with, the command goes as one line.
pub(crate) fn run_with_literals(
    session: &mut ImapSession,
    literals: Option<&super::stream::Literals>,
    command: &str,
) -> imap::error::Result<Vec<u8>> {
    let parts = synchronizing_parts(command);
    match literals {
        Some(literals) if parts.iter().any(|p| matches!(p, super::stream::Part::Literal(_))) => {
            let line = literals.command(&parts);
            session.run_command_and_read_response(line)
        }
        Some(_) => session.run_command_and_read_response(command),
        None if parts.len() > 1 => Err(imap::error::Error::Bad(
            t!("sync-imap-ext-this-connection-cant-send-text-with").into(),
        )),
        None => session.run_command_and_read_response(command),
    }
}
/// `command` as text and its synchronizing literals' bytes, in order;
/// LITERAL+ literals (`{n+}`) stay in the text, needing no go-ahead.
fn synchronizing_parts(command: &str) -> Vec<super::stream::Part<'_>> {
    use super::stream::Part;
    let mut parts = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while let Some(found) = command[at..].find("}\r\n") {
        let close = at + found;
        let after = close + 3;
        let literal = command[..close].rfind('{').and_then(|open| {
            let digits = &command[open + 1..close];
            (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| digits.parse::<usize>().ok())
                .flatten()
                .filter(|n| after + n <= command.len() && command.is_char_boundary(after + n))
                .map(|n| (open, n))
        });
        match literal {
            Some((open, n)) => {
                // `Literals::command` announces each literal itself.
                parts.push(Part::Text(&command[start..open]));
                parts.push(Part::Literal(command[after..after + n].as_bytes()));
                start = after + n;
                at = start;
            }
            None => at = after,
        }
    }
    if parts.is_empty() {
        return vec![Part::Text(command)];
    }
    parts.push(Part::Text(&command[start..]));
    parts
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
    literals: Option<&super::stream::Literals>,
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
        let data = run_with_literals(
            session,
            literals,
            &format!("UID SORT (REVERSE DATE) {charset} {criteria}"),
        )?;
        let mut out = Vec::new();
        for line in other_lines(&data, "SORT")? {
            // With CONDSTORE, a MODSEQ search adds `(MODSEQ n)` at the
            // end (RFC 7162 §3.1.5); it is no UID.
            let line = match line.find('(') {
                Some(at) => &line[..at],
                None => line,
            };
            for word in line.split_ascii_whitespace() {
                out.push(word.parse::<u32>().with_context(|| t!("sync-imap-ext-invalid-imap-sort-response"))?);
            }
        }
        return Ok(out);
    }
    let charset = search_charset(criteria, utf8);
    let mut uids = search_uids(session, literals, ext, selected, &format!("{charset}{criteria}"))?;
    uids.reverse();
    Ok(uids)
}

/// The newest `limit` messages matching `criteria` (newest first) and how
/// many match in all. Sorted by the server where it can; otherwise, where
/// it has PARTIAL (RFC 9394), only the newest page is asked for, so a
/// search matching most of a large mailbox doesn't bring back every UID.
pub(crate) fn newest_search(
    session: &mut ImapSession,
    literals: Option<&super::stream::Literals>,
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
        let data = run_with_literals(
            session,
            literals,
            &format!("UID SORT RETURN (COUNT PARTIAL 1:{limit}) (REVERSE DATE) {charset} {criteria}"),
        )?;
        let (page, count) = partial_uids(&data)?;
        let count = count.unwrap_or(page.len());
        if page.is_empty() {
            return Ok((page, count));
        }
        let set = super::imap::uid_ranges(&page);
        let ordered = sorted_search(session, literals, ext, selected, &format!("UID {set}"), utf8)?;
        return Ok((ordered.into_iter().take(limit).collect(), count));
    }
    if !ext.sort && ext.partial && ext.esearch && limit > 0 {
        let charset = search_charset(criteria, utf8);
        let data = run_with_literals(
            session,
            literals,
            &format!("UID SEARCH RETURN (COUNT PARTIAL -1:-{limit}) {charset}{criteria}"),
        )?;
        let (mut uids, count) = partial_uids(&data)?;
        uids.sort_unstable_by(|a, b| b.cmp(a));
        uids.truncate(limit);
        let count = count.unwrap_or(uids.len());
        return Ok((uids, count));
    }
    let uids = sorted_search(session, literals, ext, selected, criteria, utf8)?;
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
                parse_set_into(set, &mut uids, 0)?;
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
            current.push(number.parse().with_context(|| t!("sync-imap-ext-invalid-imap-thread-response"))?);
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
                ensure!(depth > 0, t!("sync-imap-ext-invalid-imap-thread-response"));
                depth -= 1;
                if depth == 0 && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            ' ' => flush(&mut number, &mut current)?,
            d if d.is_ascii_digit() => number.push(d),
            _ => bail!(t!("sync-imap-ext-invalid-imap-thread-response")),
        }
    }
    ensure!(depth == 0, t!("sync-imap-ext-invalid-imap-thread-response"));
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
/// `known` are the UIDs kept here (RFC 7162 §3.2.5.1's known-uids): the
/// server reports as VANISHED only those, not every UID ever removed.
pub(crate) fn select_qresync(
    session: &mut ImapSession,
    name: &str,
    utf8: bool,
    validity: u32,
    modseq: u64,
    known: &[u32],
) -> Result<(Selected, Option<Resync>)> {
    let wire = wire_name(name, utf8);
    let known = if known.is_empty() {
        String::new()
    } else {
        format!(" {}", super::imap::uid_ranges(known))
    };
    let command = format!("SELECT {} (QRESYNC ({validity} {modseq}{known}))", quote(&wire)?);
    let data = session.run_command_and_read_response(command)?;
    // Where a mailbox was selected before, its end is marked `[CLOSED]`
    // (RFC 7162 §3.2.11): what came before is about that one, not this.
    let data = after_closed(&data);
    let selected = super::imap::selected_from(data, name, true)?;
    if selected.validity != validity || selected.modseq.is_none() {
        // A changed validity, or a mailbox that keeps no mod-sequences
        // ([NOMODSEQ]): nothing to resynchronize from.
        return Ok((selected, None));
    }
    let mut resync = Resync::default();
    for line in other_lines(data, "VANISHED")? {
        let set = line
            .trim_start()
            .strip_prefix("(EARLIER)")
            .or_else(|| line.trim_start().strip_prefix("(earlier)"))
            .unwrap_or(line)
            .trim();
        parse_set_into(set, &mut resync.vanished, 0)?;
    }
    resync.changed = super::imap::rows_from(data)?;
    Ok((selected, Some(resync)))
}
/// What follows the last `* OK [CLOSED]` line of a reply, or all of it.
pub(crate) fn after_closed(data: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut at = 0;
    while at < data.len() {
        let end = data[at..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map_or(data.len(), |p| at + p + 2);
        let line = &data[at..end];
        if line.len() > 13 && line[..13].eq_ignore_ascii_case(b"* OK [CLOSED]") {
            start = end;
        }
        at = end;
    }
    &data[start..]
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
        t!("sync-imap-ext-invalid-metadata-entry")
    );
    // Values past 64 KB aren't sent (MAXSIZE, RFC 5464 §4.2.1): the
    // server says how long the longest was, rather than a huge reply.
    let data = match session.run_command_and_read_response(format!(
        "GETMETADATA (MAXSIZE 65536) {} ({})",
        quote(mailbox)?,
        entries.join(" ")
    )) {
        Ok(data) => data,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    // A reply the stream couldn't make readable is an error, not no notes.
    other_lines(&data, "METADATA")?;
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
        t!("sync-imap-ext-invalid-metadata-entry")
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
pub(crate) fn notify_personal(session: &mut ImapSession, condstore: bool) -> Result<bool> {
    // FlagChange for mailboxes other than the selected one needs CONDSTORE
    // (RFC 5465 §5.4); a server would refuse it with [BADEVENT].
    let others = if condstore {
        "MessageNew MessageExpunge FlagChange"
    } else {
        "MessageNew MessageExpunge"
    };
    // Folders made, renamed or deleted, and subscriptions changed, come as
    // untagged LIST lines (MailboxName, SubscriptionChange; RFC 5465 §5.4
    // and §5.5); see `mailbox_list_changed`.
    let commands = [
        format!(
            "NOTIFY SET STATUS (SELECTED (MessageNew MessageExpunge FlagChange)) \
             (PERSONAL ({others} MailboxName SubscriptionChange))"
        ),
        format!(
            "NOTIFY SET STATUS (SELECTED (MessageNew MessageExpunge FlagChange)) \
             (PERSONAL ({others}))"
        ),
        "NOTIFY SET STATUS (SELECTED (MessageNew MessageExpunge)) \
         (PERSONAL (MessageNew MessageExpunge))"
            .to_owned(),
    ];
    for command in &commands {
        match session.run_command_and_check_ok(command) {
            Ok(()) => return Ok(true),
            // [BADEVENT] (RFC 5465 §6): the server can't report one of
            // those; ask again for less.
            Err(imap::error::Error::No(e) | imap::error::Error::Bad(e))
                if e.to_ascii_uppercase().contains("BADEVENT") => {}
            Err(e) => return super::imap::optional(Err(e)),
        }
    }
    Ok(false)
}
/// Whether a NOTIFY session heard a mailbox made, renamed, deleted or
/// (un)subscribed (an untagged LIST line, RFC 5465 §5.4 and §5.5): the
/// folder list should be read again. One answer for however many lines.
pub(crate) fn mailbox_list_changed(heard: &[u8]) -> bool {
    String::from_utf8_lossy(heard).lines().any(|line| {
        line.get(..7).is_some_and(|start| start.eq_ignore_ascii_case("* LIST "))
    })
}
/// Whether a NOTIFY session lost events ([NOTIFICATIONOVERFLOW], RFC 5465
/// §5.8): the server stopped notifying, and every mailbox may have
/// changed.
pub(crate) fn notification_overflow(heard: &[u8]) -> bool {
    const CODE: &[u8] = b"[NOTIFICATIONOVERFLOW]";
    heard.windows(CODE.len()).any(|w| w.eq_ignore_ascii_case(CODE))
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
        .with_context(|| t!("sync-imap-ext-the-server-didnt-describe-this-messages"))?;
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
            let pairs: Vec<(&str, &str)> = p.iter().map(|(k, v)| (&**k, &**v)).collect();
            rfc2231(&pairs, key).or_else(|| {
                p.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| mailparse_decode(v))
            })
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
/// A parameter in RFC 2231's forms, from a server's BODYSTRUCTURE:
/// `name*` (charset'language'%XX…), or continued as `name*0`, `name*1*`…
/// in order. `None` where it isn't written so.
fn rfc2231(params: &[(&str, &str)], key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    let mut pieces: Vec<(u32, bool, &str)> = Vec::new();
    for (k, v) in params {
        let k = k.to_ascii_lowercase();
        let Some(rest) = k.strip_prefix(&lower).and_then(|r| r.strip_prefix('*')) else {
            continue;
        };
        let (number, encoded) = match rest.strip_suffix('*') {
            Some(n) => (n, true),
            None if rest.is_empty() => ("", true),
            None => (rest, false),
        };
        let index = if number.is_empty() { 0 } else { number.parse().ok()? };
        pieces.push((index, encoded, v));
    }
    if pieces.is_empty() {
        return None;
    }
    pieces.sort_by_key(|p| p.0);
    let mut charset = String::from("utf-8");
    let mut bytes = Vec::new();
    for (i, (_, encoded, value)) in pieces.iter().enumerate() {
        let mut value = *value;
        if *encoded {
            if i == 0 {
                // charset'language'
                let mut parts = value.splitn(3, '\'');
                if let (Some(cs), Some(_), Some(rest)) = (parts.next(), parts.next(), parts.next()) {
                    if !cs.is_empty() {
                        charset = cs.to_owned();
                    }
                    value = rest;
                }
            }
            let raw = value.as_bytes();
            let mut j = 0;
            while j < raw.len() {
                if raw[j] == b'%'
                    && let Some(b) = raw
                        .get(j + 1..j + 3)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    bytes.push(b);
                    j += 3;
                } else {
                    bytes.push(raw[j]);
                    j += 1;
                }
            }
        } else {
            bytes.extend_from_slice(value.as_bytes());
        }
    }
    let encoding = encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    Some(encoding.decode(&bytes).0.into_owned())
}
/// A parameter value, with RFC 2047 encoded words decoded.
fn mailparse_decode(value: &str) -> String {
    let header = format!("X: {value}\r\n\r\n");
    mailparse::parse_header(header.as_bytes())
        .map(|(h, _)| h.get_value())
        .unwrap_or_else(|_| value.to_owned())
}

/// The decoded sizes of a message's parts `sections` ("1.2"), where the
/// server can say (BINARY.SIZE, RFC 3516 §4.2), in one FETCH: exact where
/// the transfer encoding's size is only an estimate. Parts the server
/// can't decode ([UNKNOWN-CTE]) or doesn't answer for are left out.
pub(crate) fn binary_sizes(
    session: &mut ImapSession,
    uid: u32,
    sections: &[&str],
) -> Result<std::collections::HashMap<String, u64>> {
    let mut out = std::collections::HashMap::new();
    let sections: Vec<&str> = sections
        .iter()
        .copied()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        .collect();
    if sections.is_empty() {
        return Ok(out);
    }
    let items: Vec<String> = sections.iter().map(|s| format!("BINARY.SIZE[{s}]")).collect();
    let data = match session.run_command_and_read_response(format!("UID FETCH {uid} (UID {})", items.join(" "))) {
        Ok(data) => data,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    for response in responses(&data)? {
        let imap_proto::Response::Fetch(_, attributes) = response else {
            continue;
        };
        if !attributes.contains(&imap_proto::AttributeValue::Uid(uid)) {
            continue;
        }
        for a in &attributes {
            if let imap_proto::AttributeValue::BinarySize { section, size } = a {
                let name: Vec<String> = section.iter().map(u32::to_string).collect();
                out.insert(name.join("."), u64::from(*size));
            }
        }
    }
    Ok(out)
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
        t!("sync-imap-ext-invalid-imap-section")
    );
    ensure!(
        leaf.size <= limit,
        t!("sync-imap-ext-this-part-is-larger-than-picacho", megabytes = limit / (1024 * 1024))
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
        .with_context(|| t!("sync-imap-ext-the-message-no-longer-exists-on"))?;
    decode_part(&raw, &leaf.encoding)
}
/// A part's body in its transfer encoding, decoded.
pub(crate) fn decode_part(raw: &[u8], encoding: &str) -> Result<Vec<u8>> {
    // The server names the encoding, and it is put in a header line here:
    // only a name (a line break would add headers of the server's own).
    ensure!(
        encoding.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        t!("sync-imap-ext-the-server-named-an-invalid-transfer")
    );
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
        .with_context(|| t!("sync-imap-ext-the-message-no-longer-exists-on"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc_2231_parameters_are_read() {
        assert_eq!(
            rfc2231(&[("filename*", "utf-8''Caf%C3%A9.pdf")], "filename").as_deref(),
            Some("Café.pdf")
        );
        assert_eq!(
            rfc2231(&[("NAME*1", "rt.txt"), ("NAME*0*", "iso-8859-1'en'%E9")], "name").as_deref(),
            Some("ért.txt")
        );
        assert_eq!(rfc2231(&[("name", "plain.txt")], "name"), None);
    }
    /// RFC 3516 §4.2: BINARY.SIZE replies, as the parser reads them.
    #[test]
    fn binary_sizes_are_read() {
        let data = b"* 3 FETCH (UID 9 BINARY.SIZE[2] 1024 BINARY.SIZE[3.1] 77)\r\n";
        let imap_proto::Response::Fetch(_, attributes) = &responses(data).unwrap()[0] else {
            panic!("not a fetch")
        };
        assert!(attributes.contains(&imap_proto::AttributeValue::BinarySize { section: vec![3, 1], size: 77 }));
    }
    #[test]
    fn closed_splits_a_reselect_and_overflow_is_seen() {
        let data = b"* 3 FETCH (UID 9 FLAGS ())\r\n* OK [CLOSED] previous mailbox\r\n* 1 EXISTS\r\n";
        assert_eq!(after_closed(data), b"* 1 EXISTS\r\n");
        assert_eq!(after_closed(b"* 1 EXISTS\r\n"), b"* 1 EXISTS\r\n");
        assert!(notification_overflow(b"* OK [NOTIFICATIONOVERFLOW] lost\r\n"));
        assert!(mailbox_list_changed(b"* LIST () \"/\" \"Projects/2027\"\r\n* list (\\NonExistent) \"/\" Old\r\n"));
        assert!(!mailbox_list_changed(b"* STATUS Work (MESSAGES 4)\r\n* 3 EXISTS\r\n"));
        assert!(!notification_overflow(b"* OK still here\r\n"));
    }
    use super::*;

    #[test]
    fn sets_threads_and_notify_statuses() {
        assert_eq!(parse_set("1:3,7,9:8").unwrap(), vec![1, 2, 3, 7, 8, 9]);
        assert!(parse_set("0").is_err());
        assert!(parse_set("1:4294967295").is_err());
        // The limit is for a whole reply, not each of its lines.
        let mut out = Vec::new();
        parse_set_into("1:15000000", &mut out, 0).unwrap();
        assert!(parse_set_into("1:15000000", &mut out, 0).is_err());
        assert!(parse_set_into("1:6000000", &mut Vec::new(), 15_000_000).is_err());
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
        assert!(decode_part(b"x", "7bit\r\nContent-Type: text/html").is_err());
        // Line breaks only as literals' own, of their stated length.
        assert!(literals_only("TEXT {3+}\r\nabc SUBJECT {2+}\r\nhi"));
        assert!(literals_only("TEXT {5}\r\ncaf\u{e9}"));
        // 8-bit text is a literal: LITERAL+ where it can, else synchronizing.
        let plain = Extensions::default();
        let plus = Extensions { literal_plus: true, ..Extensions::default() };
        assert_eq!(search_string("caf\u{e9}", &plain, false).unwrap(), "{5}\r\ncaf\u{e9}");
        assert_eq!(search_string("caf\u{e9}", &plus, false).unwrap(), "{5+}\r\ncaf\u{e9}");
        assert_eq!(search_string("caf\u{e9}", &plain, true).unwrap(), "\"caf\u{e9}\"");
        assert_eq!(search_string("a \"b\"", &plain, false).unwrap(), "\"a \\\"b\\\"\"");
        use super::super::stream::Part;
        let parts = synchronizing_parts("UID SEARCH TEXT {5}\r\ncaf\u{e9} FROM {2+}\r\n\u{e9}");
        assert!(matches!(
            parts.as_slice(),
            [Part::Text("UID SEARCH TEXT "), Part::Literal(l), Part::Text(" FROM {2+}\r\n\u{e9}")]
                if *l == "caf\u{e9}".as_bytes()
        ));
        assert!(matches!(synchronizing_parts("UID SEARCH ALL").as_slice(), [Part::Text("UID SEARCH ALL")]));
        assert!(literals_only("TEXT \"a\""));
        assert!(!literals_only("TEXT {3+}\r\nabc\r\nLOGOUT"));
        assert!(!literals_only("TEXT {9+}\r\nab\r\nLOGOUT"));
        assert!(!literals_only("TEXT x\r\n"));
        assert_eq!(wire_name("Caf&AOk-", true), "Café");
        assert_eq!(cache_name("Café", true), "Caf&AOk-");
    }
}
