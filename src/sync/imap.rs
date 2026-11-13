//! Verified IMAP connections, stable identities, and remote folder resolution.
use super::wire::Wire;
use crate::manage::model::*;
use anyhow::{Context, Result, bail, ensure};
use std::{
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};
pub(crate) type ImapSession = imap::Session<native_tls::TlsStream<Wire>>;
pub(crate) fn tcp(host: &str, port: u16) -> Result<Wire> {
    let addrs = (host, port)
        .to_socket_addrs()
        .context("Server hostname could not be resolved")?;
    let mut last = None;
    let deadline = Instant::now() + Duration::from_secs(12);
    for addr in addrs {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match TcpStream::connect_timeout(&addr, remaining.min(Duration::from_secs(3))) {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_secs(30)))?;
                s.set_write_timeout(Some(Duration::from_secs(30)))?;
                return Ok(Wire::new(s));
            }
            Err(e) => last = Some(e),
        }
    }
    bail!(
        "Could not connect to {host}:{port}: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    )
}
pub(crate) fn imap_login(
    a: &Account,
    s: &Secret,
    tls: &native_tls::TlsConnector,
) -> Result<ImapSession> {
    let mut socket = tcp(&a.incoming_host, a.incoming_port)?;
    if a.incoming_starttls {
        // The imap crate's secure() only accepts TcpStream. Perform the small
        // plaintext upgrade here so bounded I/O also covers STARTTLS sessions.
        starttls(&mut socket)?;
    }
    let stream = tls
        .connect(&a.incoming_host, socket)
        .map_err(|e| anyhow::anyhow!("IMAP TLS handshake failed: {e}"))?;
    let mut client = imap::Client::new(stream);
    if a.incoming_starttls {
        client.greeting_read = true; // IMAP sends no second greeting after STARTTLS.
    } else {
        client.read_greeting()?;
    }
    let session = if a.oauth {
        client.authenticate(
            "XOAUTH2",
            &Xoauth {
                user: a.email.clone(),
                token: s.access_token.clone(),
            },
        )
    } else {
        client.login(&a.email, &s.password)
    };
    session.map_err(|(e, _)| {
        let rejected = matches!(e, imap::Error::No(_) | imap::Error::Bad(_));
        let error = anyhow::Error::new(e).context("IMAP authentication failed");
        if rejected {
            error.context(crate::manage::status::AccessFailure::Authentication)
        } else {
            error
        }
    })
}
struct Xoauth {
    user: String,
    token: String,
}
impl imap::Authenticator for Xoauth {
    type Response = String;
    fn process(&self, _: &[u8]) -> String {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}
pub(crate) fn same_validity(a: &Mail, b: &Mail) -> bool {
    a.id.rsplit(':').nth(1) == b.id.rsplit(':').nth(1)
}
pub(crate) fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
#[derive(Clone)]
pub(crate) struct Name {
    pub(crate) name: String,
    attributes: Vec<String>,
    delimiter: Option<String>,
}
impl Name {
    fn has(&self, attribute: &str) -> bool {
        self.attributes.iter().any(|a| a.contains(attribute))
    }
}
pub(crate) fn list(session: &mut ImapSession) -> Result<Vec<Name>> {
    Ok(session
        .list(None, Some("*"))?
        .iter()
        .map(|n| Name {
            name: n.name().into(),
            attributes: n.attributes().iter().map(|a| format!("{a:?}")).collect(),
            delimiter: n.delimiter().map(Into::into),
        })
        .collect())
}
const STANDARD_ATTRIBUTES: [&str; 8] = [
    "Sent",
    "Drafts",
    "Trash",
    "Junk",
    "All",
    "Archive",
    "Flagged",
    "Important",
];
const STANDARD_NAMES: [&str; 14] = [
    "INBOX",
    "Sent",
    "Sent Items",
    "Sent Mail",
    "Drafts",
    "Trash",
    "Deleted Items",
    "Deleted",
    "Junk",
    "Junk Email",
    "Spam",
    "Archive",
    "Archives",
    "Outbox",
];
/// Folders shown beneath the inbox: everything selectable that is not one of
/// the standard mailboxes, nested by the server's hierarchy delimiter.
pub(crate) fn custom_folders(names: &[Name]) -> Vec<RemoteFolder> {
    let mut out: Vec<RemoteFolder> = names
        .iter()
        .filter(|n| !n.has("NoSelect") && !n.has("Noselect"))
        .filter(|n| !STANDARD_ATTRIBUTES.iter().any(|a| n.has(a)))
        .filter_map(|n| {
            let delimiter = n.delimiter.as_deref().filter(|d| !d.is_empty());
            let mut parts: Vec<&str> = match delimiter {
                Some(d) => n.name.split(d).collect(),
                None => vec![n.name.as_str()],
            };
            if parts.len() > 1 && parts[0].eq_ignore_ascii_case("INBOX") {
                parts.remove(0);
            }
            let leaf = parts.last()?;
            if parts.len() == 1 && STANDARD_NAMES.iter().any(|s| s.eq_ignore_ascii_case(leaf)) {
                return None;
            }
            Some(RemoteFolder {
                name: n.name.clone(),
                label: (*leaf).into(),
                depth: parts.len() - 1,
            })
        })
        .collect();
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}
pub(crate) fn resolve_in(names: &[Name], provider: Provider, folder: Folder) -> Result<String> {
    if folder == Folder::Inbox {
        return Ok("INBOX".into());
    }
    let special = match folder {
        Folder::Sent => "\\Sent",
        Folder::Drafts => "\\Drafts",
        Folder::Trash => "\\Trash",
        Folder::Junk => "\\Junk",
        Folder::Archive => {
            if provider == Provider::Gmail {
                "\\All"
            } else {
                "\\Archive"
            }
        }
        _ => "",
    };
    if !special.is_empty()
        && let Some(name) = names
            .iter()
            .find(|n| n.has(special.trim_start_matches('\\')))
    {
        return Ok(name.name.clone());
    }
    let candidates: &[&str] = match folder {
        Folder::Sent => &["[Gmail]/Sent Mail", "Sent", "Sent Items", "INBOX.Sent"],
        Folder::Drafts => &["[Gmail]/Drafts", "Drafts", "INBOX.Drafts"],
        Folder::Trash => &[
            "[Gmail]/Trash",
            "Trash",
            "Deleted Items",
            "Deleted",
            "INBOX.Trash",
        ],
        Folder::Junk => &["[Gmail]/Spam", "Junk", "Junk Email", "Spam", "INBOX.Junk"],
        Folder::Archive => &["[Gmail]/All Mail", "Archive", "Archives", "INBOX.Archive"],
        _ => &["INBOX"],
    };
    for c in candidates {
        if let Some(n) = names.iter().find(|n| n.name.eq_ignore_ascii_case(c)) {
            return Ok(n.name.clone());
        }
    }
    bail!("The server has no {} folder.", folder.label())
}

fn starttls(socket: &mut Wire) -> Result<()> {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut reader = BufReader::new(socket);
    let line = |reader: &mut BufReader<&mut Wire>| -> Result<String> {
        let mut line = String::new();
        reader.take(8193).read_line(&mut line)?;
        ensure!(
            line.len() <= 8192 && line.ends_with("\r\n"),
            "Invalid IMAP STARTTLS response"
        );
        Ok(line)
    };
    let greeting = line(&mut reader)?;
    ensure!(
        greeting.starts_with("* OK ") || greeting == "* OK\r\n",
        "IMAP server rejected connection"
    );
    reader.get_mut().write_all(b"p1 STARTTLS\r\n")?;
    reader.get_mut().flush()?;
    loop {
        let response = line(&mut reader)?;
        if response.starts_with("p1 ") {
            ensure!(
                response.starts_with("p1 OK ") || response == "p1 OK\r\n",
                "IMAP STARTTLS was rejected; credentials were not sent"
            );
            ensure!(
                reader.buffer().is_empty(),
                "Unexpected data following STARTTLS"
            );
            return Ok(());
        }
        ensure!(
            response.starts_with("* ") && !response.starts_with("* BYE"),
            "Invalid IMAP STARTTLS response"
        );
    }
}

/// Header fields the message list shows. Full headers carry Received, DKIM
/// and ARC chains that are often several times larger, and are only needed
/// once a message is opened (its whole source is downloaded then).
pub(crate) const LIST_HEADERS: &str =
    "BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID CONTENT-TYPE LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST)]";

/// Extensions beyond IMAP4rev1 that change how a mailbox is synchronized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extensions {
    /// RFC 7162: per-message change counters (MODSEQ) and CHANGEDSINCE.
    pub(crate) condstore: bool,
    /// Gmail's X-GM-EXT-1, including permanent message ids.
    pub(crate) gmail: bool,
}
pub(crate) fn extensions(session: &mut ImapSession) -> Result<Extensions> {
    let caps = session.capabilities()?;
    Ok(Extensions {
        condstore: caps.has_str("CONDSTORE"),
        gmail: caps.has_str("X-GM-EXT-1"),
    })
}

/// What SELECT reported about a mailbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selected {
    pub(crate) name: String,
    pub(crate) exists: u32,
    pub(crate) validity: u32,
    pub(crate) uid_next: Option<u32>,
    /// HIGHESTMODSEQ, when the server tracks changes for this mailbox.
    pub(crate) modseq: Option<u64>,
}

/// A mailbox name as an IMAP quoted string.
pub(crate) fn quote(name: &str) -> Result<String> {
    ensure!(
        !name.contains(['\r', '\n', '\0']),
        "Invalid mailbox name from the server"
    );
    Ok(format!(
        "\"{}\"",
        name.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// SELECT, asking for change tracking when the server supports it. The imap
/// crate's own SELECT discards HIGHESTMODSEQ, so this reads the response here.
pub(crate) fn select(session: &mut ImapSession, name: &str, condstore: bool) -> Result<Selected> {
    let command = format!(
        "SELECT {}{}",
        quote(name)?,
        if condstore { " (CONDSTORE)" } else { "" }
    );
    let data = session.run_command_and_read_response(command)?;
    let mut selected = Selected {
        name: name.into(),
        ..Default::default()
    };
    let mut validity = None;
    for response in responses(&data)? {
        use imap_proto::{MailboxDatum, Response, ResponseCode};
        match response {
            Response::Data {
                code: Some(code), ..
            } => match code {
                ResponseCode::UidValidity(v) => validity = Some(v),
                ResponseCode::UidNext(n) => selected.uid_next = Some(n),
                ResponseCode::HighestModSeq(m) if condstore => selected.modseq = Some(m),
                _ => {}
            },
            Response::MailboxData(MailboxDatum::Exists(n)) => selected.exists = n,
            _ => {}
        }
    }
    selected.validity = validity
        .filter(|v| *v > 0)
        .context("The server did not supply a valid UIDVALIDITY; cached mail has been kept.")?;
    // UIDNEXT 0 is not a valid UID; treat it as missing.
    selected.uid_next = selected.uid_next.filter(|n| *n > 0);
    Ok(selected)
}

/// One message from a UID FETCH, with only the attributes that were asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) uid: u32,
    pub(crate) flags: Option<Vec<String>>,
    pub(crate) size: Option<u32>,
    pub(crate) internal_date: Option<i64>,
    pub(crate) header: Option<Vec<u8>>,
    pub(crate) gmail_id: Option<u64>,
}
impl Row {
    fn flag(&self, name: &str) -> bool {
        self.flags
            .iter()
            .flatten()
            .any(|f| f.eq_ignore_ascii_case(name))
    }
    pub(crate) fn seen(&self) -> bool {
        self.flag("\\Seen")
    }
    pub(crate) fn flagged(&self) -> bool {
        self.flag("\\Flagged")
    }
}

/// UID FETCH that also understands MODSEQ and Gmail's X-GM-MSGID. Rows can
/// include unsolicited updates for other messages; callers filter by UID.
pub(crate) fn uid_fetch_rows(
    session: &mut ImapSession,
    set: &str,
    items: &str,
) -> Result<Vec<Row>> {
    ensure!(
        !set.contains(['\r', '\n']) && !items.contains(['\r', '\n']),
        "Invalid IMAP FETCH"
    );
    let data = session.run_command_and_read_response(format!("UID FETCH {set} {items}"))?;
    let mut rows = Vec::new();
    for response in responses(&data)? {
        let imap_proto::Response::Fetch(_, attributes) = response else {
            continue;
        };
        let mut row = Row::default();
        for attribute in attributes {
            use imap_proto::AttributeValue as A;
            match attribute {
                A::Uid(uid) => row.uid = uid,
                A::Flags(flags) => row.flags = Some(flags.iter().map(|f| (*f).into()).collect()),
                A::Rfc822Size(size) => row.size = Some(size),
                A::InternalDate(date) => row.internal_date = internal_date(date),
                A::GmailMsgId(id) => row.gmail_id = Some(id),
                A::BodySection {
                    data: Some(data), ..
                }
                | A::Rfc822Header(Some(data)) => row.header = Some(data.to_vec()),
                _ => {}
            }
        }
        if row.uid > 0 {
            rows.push(row);
        }
    }
    Ok(rows)
}
fn internal_date(date: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(date.trim(), "%d-%b-%Y %H:%M:%S %z")
        .ok()
        .map(|d| d.timestamp())
}
/// Untagged responses that the imap crate has already read and validated.
fn responses(data: &[u8]) -> Result<Vec<imap_proto::Response<'_>>> {
    let mut rest = data;
    let mut out = Vec::new();
    while !rest.is_empty() {
        match imap_proto::parse_response(rest) {
            Ok((next, response)) => {
                out.push(response);
                rest = next;
            }
            Err(_) => bail!("Invalid IMAP response"),
        }
    }
    Ok(out)
}

/// Compact UID set for a command ("1:4,7"), from UIDs in any order.
pub(crate) fn uid_ranges(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        while i + 1 < sorted.len() && sorted[i + 1] == sorted[i] + 1 {
            i += 1;
        }
        out.push(if sorted[i] == start {
            start.to_string()
        } else {
            format!("{start}:{}", sorted[i])
        });
        i += 1;
    }
    out.join(",")
}

/// Servers say this when an account is over its bandwidth or connection
/// limits; Gmail uses several of these. Retrying immediately makes it worse.
pub(crate) fn throttled(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_lowercase();
    [
        "[throttled]",
        "[overquota]",
        "[limit]",
        "bandwidth limits",
        "too many simultaneous connections",
        "account exceeded command",
        "[unavailable]",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    #[test]
    fn parses_condstore_and_gmail_fetch_rows() {
        let data = b"* 3 FETCH (X-GM-MSGID 1278455344230334865 UID 77 MODSEQ (12) FLAGS (\\Seen \\Flagged) RFC822.SIZE 420 INTERNALDATE \" 7-Jul-2026 02:44:25 -0700\" BODY[HEADER.FIELDS (SUBJECT)] {15}\r\nSubject: Hi\r\n\r\n)\r\n";
        let responses = responses(data).unwrap();
        assert_eq!(responses.len(), 1);
        let imap_proto::Response::Fetch(_, attributes) = &responses[0] else {
            panic!("not a fetch")
        };
        assert!(attributes.contains(&imap_proto::AttributeValue::GmailMsgId(1278455344230334865)));
        assert!(attributes.contains(&imap_proto::AttributeValue::ModSeq(12)));
        assert_eq!(
            internal_date(" 7-Jul-2026 02:44:25 -0700"),
            Some(1783417465)
        );
    }
    #[test]
    fn uid_ranges_are_compact() {
        assert_eq!(uid_ranges(&[9, 1, 2, 3, 7, 3]), "1:3,7,9");
        assert_eq!(uid_ranges(&[]), "");
    }
    #[test]
    fn gmail_limits_are_recognized() {
        assert!(throttled(&anyhow::anyhow!(
            "NO [ALERT] Account exceeded command or bandwidth limits. (Failure)"
        )));
        assert!(throttled(&anyhow::anyhow!("NO [THROTTLED] slow down")));
        assert!(!throttled(&anyhow::anyhow!(
            "NO [AUTHENTICATIONFAILED] bad"
        )));
    }
}
