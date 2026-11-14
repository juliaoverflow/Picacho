//! Verified IMAP connections, stable identities, and remote folder resolution.
use super::{stream::ImapStream, wire::Wire};
use crate::manage::model::*;
use anyhow::{Context, Result, bail, ensure};
use std::{
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};
pub(crate) type ImapSession = imap::Session<ImapStream>;
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
    imap_login_interruptible(a, s, tls).map(|(session, _)| session)
}
/// Signs in, also returning a handle that closes the connection from
/// another thread (to end a session waiting in IDLE).
pub(crate) fn imap_login_interruptible(
    a: &Account,
    s: &Secret,
    tls: &native_tls::TlsConnector,
) -> Result<(ImapSession, TcpStream)> {
    let mut socket = tcp(&a.incoming_host, a.incoming_port)?;
    if a.incoming_starttls {
        // The imap crate's secure() only accepts TcpStream. Perform the small
        // plaintext upgrade here so bounded I/O also covers STARTTLS sessions.
        starttls(&mut socket)?;
    }
    let stream = ImapStream::new(
        tls.connect(&a.incoming_host, socket)
            .map_err(|e| anyhow::anyhow!("IMAP TLS handshake failed: {e}"))?,
    );
    let compressing = stream.compression_switch();
    let interrupter = stream.interrupter()?;
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
        client.login(a.login(), &s.password)
    };
    let mut session = session.map_err(|(e, _)| {
        let rejected = matches!(e, imap::Error::No(_) | imap::Error::Bad(_));
        let error = anyhow::Error::new(e).context("IMAP authentication failed");
        if rejected {
            error.context(crate::manage::status::AccessFailure::Authentication)
        } else {
            error
        }
    })?;
    prepare(&mut session, &compressing)?;
    Ok((session, interrupter))
}
/// What the app tells servers that ask who is connecting (ID, RFC 2971).
const CLIENT_ID: &str = concat!(
    "ID (\"name\" \"Picacho\" \"version\" \"",
    env!("CARGO_PKG_VERSION"),
    "\" \"vendor\" \"Pinal\")"
);
/// After signing in: says who is connecting where the server asks (ID),
/// and compresses the session where it can (COMPRESS=DEFLATE). Neither is
/// required; a server that refuses one is used without it.
fn prepare(session: &mut ImapSession, compressing: &std::sync::atomic::AtomicBool) -> Result<()> {
    let caps = session.capabilities()?;
    let (id, deflate) = (caps.has_str("ID"), caps.has_str("COMPRESS=DEFLATE"));
    drop(caps);
    if id {
        optional(session.run_command_and_check_ok(CLIENT_ID))?;
    }
    if deflate && optional(session.run_command_and_check_ok("COMPRESS DEFLATE"))? {
        // Both directions are compressed from the next byte on.
        compressing.store(true, std::sync::atomic::Ordering::Release);
    }
    Ok(())
}
/// An optional command's result: whether the server accepted it. A refusal
/// is no error; a broken connection is.
pub(crate) fn optional(result: imap::error::Result<()>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => Ok(false),
        Err(e) => Err(e.into()),
    }
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
    fn delimiter(&self) -> Option<&str> {
        self.delimiter.as_deref().filter(|d| !d.is_empty())
    }
    /// Whether folders can be made inside this one: the server has a
    /// hierarchy and has not marked it \Noinferiors.
    fn nestable(&self) -> bool {
        self.delimiter().is_some() && !self.has("NoInferiors") && !self.has("Noinferiors")
    }
}
/// Every mailbox on the server. Servers with SPECIAL-USE (RFC 6154) are
/// asked to mark the standard ones (\\Sent, \\Trash…), so they are found by
/// role rather than guessed from their names.
pub(crate) fn list(session: &mut ImapSession, special_use: bool) -> Result<Vec<Name>> {
    if !special_use {
        return Ok(session
            .list(None, Some("*"))?
            .iter()
            .map(|n| Name {
                name: n.name().into(),
                attributes: n.attributes().iter().map(|a| format!("{a:?}")).collect(),
                delimiter: n.delimiter().map(Into::into),
            })
            .collect());
    }
    let data = session.run_command_and_read_response("LIST \"\" \"*\" RETURN (SPECIAL-USE)")?;
    let mut names = Vec::new();
    for response in responses(&data)? {
        if let imap_proto::Response::MailboxData(imap_proto::MailboxDatum::List {
            flags,
            delimiter,
            name,
        }) = response
        {
            names.push(Name {
                name: name.into(),
                attributes: flags.iter().map(|f| (*f).into()).collect(),
                delimiter: delimiter.map(Into::into),
            });
        }
    }
    Ok(names)
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
                label: decode_name(leaf),
                depth: parts.len() - 1,
                nestable: n.nestable(),
            })
        })
        .collect();
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}
/// Whether a server folder may be deleted: one of the account's own
/// folders (not the inbox, a standard mailbox, or Gmail's system labels)
/// without subfolders, which servers treat differently (some refuse, some
/// leave them behind).
pub(crate) fn deletable(names: &[Name], provider: Provider, name: &str) -> Result<()> {
    let Some(folder) = names.iter().find(|n| n.name == name) else {
        bail!("“{name}” is no longer on the server.");
    };
    ensure!(
        custom_folders(names).iter().any(|f| f.name == name)
            && !(provider == Provider::Gmail && name.starts_with("[Gmail]")),
        "“{name}” is one of the mailbox's standard folders and can't be deleted."
    );
    let delimiter = folder.delimiter.as_deref().filter(|d| !d.is_empty());
    if let Some(d) = delimiter {
        let prefix = format!("{name}{d}");
        ensure!(
            !names.iter().any(|n| n.name.starts_with(&prefix)),
            "“{name}” has folders inside it. Delete those first."
        );
    }
    Ok(())
}
/// The server's hierarchy delimiter, as the inbox (or else any folder) reports it.
fn hierarchy(names: &[Name]) -> Option<&str> {
    names
        .iter()
        .find(|n| n.name.eq_ignore_ascii_case("INBOX"))
        .and_then(Name::delimiter)
        .or_else(|| names.iter().find_map(Name::delimiter))
}
/// Where top-level folders go. Some servers (Courier, older Cyrus) keep
/// every folder inside the inbox ("INBOX.Receipts"); new ones follow suit.
fn top_prefix(names: &[Name]) -> String {
    let Some(d) = hierarchy(names) else {
        return String::new();
    };
    let prefix = format!("INBOX{d}");
    let mut others = names
        .iter()
        .filter(|n| !n.name.eq_ignore_ascii_case("INBOX"))
        .peekable();
    let inside = others.peek().is_some()
        && others.all(|n| {
            n.name.len() > prefix.len()
                && n.name.is_char_boundary(prefix.len())
                && n.name[..prefix.len()].eq_ignore_ascii_case(&prefix)
        });
    if inside {
        names
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case("INBOX"))
            .map_or(prefix, |n| format!("{}{d}", n.name))
    } else {
        String::new()
    }
}
/// A folder name as typed, checked for what servers refuse or would
/// misread (wildcards, the hierarchy delimiter), and encoded for IMAP.
fn leaf_name(leaf: &str, delimiter: Option<&str>) -> Result<String> {
    let leaf = leaf.trim();
    ensure!(!leaf.is_empty(), "Enter a name for the folder.");
    ensure!(
        !leaf.chars().any(char::is_control),
        "Folder names can't contain line breaks or control characters."
    );
    ensure!(
        !leaf.contains(['*', '%']),
        "Folder names can't contain * or %."
    );
    if let Some(d) = delimiter {
        ensure!(!leaf.contains(d), "Folder names can't contain “{d}”.");
    }
    let encoded = encode_name(leaf);
    ensure!(encoded.len() <= 200, "That folder name is too long.");
    Ok(encoded)
}
/// Whether `prefix` puts folders at the top of the folder list: nothing,
/// the inbox's own (on servers that keep everything inside it), or the
/// personal namespace's.
fn at_top(prefix: &str, namespace: Option<&str>) -> bool {
    prefix.is_empty()
        || namespace == Some(prefix)
        || (prefix.chars().count() == 6
            && prefix
                .get(..5)
                .is_some_and(|p| p.eq_ignore_ascii_case("INBOX")))
}
/// Refuses a full folder name that the server already has, or that would
/// be taken for one of the standard mailboxes.
fn check_new_name(
    names: &[Name],
    provider: Provider,
    prefix: &str,
    leaf: &str,
    top: bool,
) -> Result<String> {
    let name = format!("{prefix}{leaf}");
    let label = decode_name(leaf);
    ensure!(
        !(top
            && (leaf.eq_ignore_ascii_case("INBOX")
                || STANDARD_NAMES.iter().any(|s| s.eq_ignore_ascii_case(leaf))
                || (provider == Provider::Gmail && leaf.eq_ignore_ascii_case("[Gmail]")))),
        "“{label}” is the name of a standard folder. Choose another name."
    );
    ensure!(
        !names.iter().any(|n| n.name == name
            || (n.name.eq_ignore_ascii_case("INBOX") && name.eq_ignore_ascii_case("INBOX"))),
        "There's already a folder named “{label}” there."
    );
    Ok(name)
}
/// The server name for a new folder called `leaf`: inside `parent` (one of
/// the account's own folders) or at the top level, which is the personal
/// namespace's prefix where the server reports one (NAMESPACE, RFC 2342).
pub(crate) fn new_folder_name(
    names: &[Name],
    provider: Provider,
    parent: Option<&str>,
    leaf: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let prefix = match parent {
        None => namespace.map_or_else(|| top_prefix(names), str::to_owned),
        Some(parent) => {
            let Some(folder) = names.iter().find(|n| n.name == parent) else {
                bail!("“{}” is no longer on the server.", decode_name(parent));
            };
            ensure!(
                custom_folders(names).iter().any(|f| f.name == parent)
                    && !(provider == Provider::Gmail && parent.starts_with("[Gmail]")),
                "Folders can't be made inside “{}”.",
                decode_name(parent)
            );
            let Some(d) = folder.delimiter().filter(|_| folder.nestable()) else {
                bail!(
                    "The server doesn't allow folders inside “{}”.",
                    decode_name(parent)
                );
            };
            format!("{parent}{d}")
        }
    };
    let leaf = leaf_name(leaf, hierarchy(names))?;
    let top = at_top(&prefix, namespace);
    check_new_name(names, provider, &prefix, &leaf, top)
}
/// The server name a folder would have as `leaf`, in the same parent, if
/// it may be renamed: one of the account's own folders, as for deleting.
/// The folder's own subfolders move with it.
pub(crate) fn renamed_folder_name(
    names: &[Name],
    provider: Provider,
    name: &str,
    leaf: &str,
    namespace: Option<&str>,
) -> Result<String> {
    let Some(folder) = names.iter().find(|n| n.name == name) else {
        bail!("“{}” is no longer on the server.", decode_name(name));
    };
    ensure!(
        custom_folders(names).iter().any(|f| f.name == name)
            && !(provider == Provider::Gmail && name.starts_with("[Gmail]")),
        "“{}” is one of the mailbox's standard folders and can't be renamed.",
        decode_name(name)
    );
    let delimiter = folder.delimiter().or_else(|| hierarchy(names));
    let prefix = delimiter
        .and_then(|d| name.rfind(d).map(|i| &name[..i + d.len()]))
        .unwrap_or_default();
    let leaf = leaf_name(leaf, delimiter)?;
    if format!("{prefix}{leaf}") == name {
        return Ok(name.into());
    }
    let top = at_top(prefix, namespace);
    check_new_name(names, provider, prefix, &leaf, top)
}
/// A folder and the folders inside it, which move or go with it.
pub(crate) fn folder_and_inside(names: &[Name], name: &str) -> Vec<String> {
    let delimiter = names
        .iter()
        .find(|n| n.name == name)
        .and_then(Name::delimiter)
        .or_else(|| hierarchy(names));
    let mut out = vec![name.to_owned()];
    if let Some(d) = delimiter {
        let prefix = format!("{name}{d}");
        out.extend(
            names
                .iter()
                .filter(|n| n.name.starts_with(&prefix))
                .map(|n| n.name.clone()),
        );
    }
    out
}
/// A mailbox name as people read it. IMAP sends names in modified UTF-7
/// (RFC 3501, 5.1.3); a name that isn't valid is shown as it is.
pub(crate) fn decode_name(name: &str) -> String {
    use base64::Engine as _;
    if !name.contains('&') {
        return name.into();
    }
    let mut out = String::new();
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            return name.into();
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let Ok(bytes) =
                base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded.replace(',', "/"))
            else {
                return name.into();
            };
            if bytes.len() % 2 != 0 {
                return name.into();
            }
            let units: Vec<u16> = bytes
                .chunks(2)
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect();
            let Ok(text) = String::from_utf16(&units) else {
                return name.into();
            };
            out.push_str(&text);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}
/// A name as IMAP's modified UTF-7, the form servers expect in commands.
pub(crate) fn encode_name(name: &str) -> String {
    use base64::Engine as _;
    let mut out = String::new();
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(
            &base64::engine::general_purpose::STANDARD_NO_PAD
                .encode(bytes)
                .replace('/', ","),
        );
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut units = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut units));
        }
    }
    flush(&mut pending, &mut out);
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
/// once a message is opened (its whole source is downloaded then). Reply-To
/// and Cc are small and let the list be searched by them (`reply-to:`).
pub(crate) const LIST_HEADERS: &str =
    "BODY.PEEK[HEADER.FIELDS (FROM REPLY-TO TO CC SUBJECT DATE MESSAGE-ID CONTENT-TYPE LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST)]";

/// Extensions beyond IMAP4rev1 that the app uses where a server has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extensions {
    /// RFC 7162: per-message change counters (MODSEQ) and CHANGEDSINCE.
    pub(crate) condstore: bool,
    /// Gmail's X-GM-EXT-1, including permanent message ids and labels.
    pub(crate) gmail: bool,
    /// RFC 6851: atomic moves (UID MOVE).
    pub(crate) moves: bool,
    /// RFC 4315: UID EXPUNGE, removing only the messages named.
    pub(crate) uidplus: bool,
    /// RFC 2177: the server tells the session about new mail (IDLE).
    pub(crate) idle: bool,
    /// RFC 2342: where personal folders go (NAMESPACE).
    pub(crate) namespace: bool,
    /// RFC 9208 (2087): storage limits (GETQUOTAROOT).
    pub(crate) quota: bool,
    /// RFC 4314 (2086): access rights (MYRIGHTS).
    pub(crate) acl: bool,
    /// RFC 6154: standard folders marked by role in LIST.
    pub(crate) special_use: bool,
    /// RFC 7889: the largest message the server accepts (APPENDLIMIT).
    pub(crate) append_limit: Option<u64>,
}
pub(crate) fn extensions(session: &mut ImapSession) -> Result<Extensions> {
    let caps = session.capabilities()?;
    let atoms: Vec<&str> = caps
        .iter()
        .filter_map(|c| match c {
            imap_proto::Capability::Atom(atom) => Some(*atom),
            _ => None,
        })
        .collect();
    let with = |prefix: &str| {
        atoms.iter().find_map(|a| {
            a.get(..prefix.len())
                .filter(|p| p.eq_ignore_ascii_case(prefix))
                .map(|_| &a[prefix.len()..])
        })
    };
    let append_limit = with("APPENDLIMIT=").and_then(|n| n.parse::<u64>().ok());
    Ok(Extensions {
        condstore: caps.has_str("CONDSTORE"),
        gmail: caps.has_str("X-GM-EXT-1"),
        moves: caps.has_str("MOVE"),
        uidplus: caps.has_str("UIDPLUS"),
        idle: caps.has_str("IDLE"),
        namespace: caps.has_str("NAMESPACE"),
        quota: caps.has_str("QUOTA") || with("QUOTA=").is_some(),
        acl: caps.has_str("ACL"),
        special_use: caps.has_str("SPECIAL-USE"),
        append_limit,
    })
}

/// The untagged lines of an extension's reply that start with `name`, as
/// text after it (NAMESPACE, QUOTA…; see the vendored parser's `resp_other`).
fn other_lines<'a>(data: &'a [u8], name: &str) -> Result<Vec<&'a str>> {
    Ok(responses(data)?
        .into_iter()
        .filter_map(|r| match r {
            imap_proto::Response::Other(line) => line
                .get(..name.len())
                .filter(|n| n.eq_ignore_ascii_case(name))
                .map(|_| line[name.len()..].trim_start()),
            _ => None,
        })
        .collect())
}
/// Words, quoted strings, NIL and parentheses in an extension's reply.
#[derive(Debug, PartialEq)]
enum Token {
    Open,
    Close,
    Nil,
    Text(String),
}
fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Token::Open);
            }
            ')' => {
                chars.next();
                out.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => text.extend(chars.next()),
                        '"' => break,
                        c => text.push(c),
                    }
                }
                out.push(Token::Text(text));
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if matches!(c, ' ' | '(' | ')') {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                out.push(if word.eq_ignore_ascii_case("NIL") {
                    Token::Nil
                } else {
                    Token::Text(word)
                });
            }
        }
    }
    out
}
/// The personal namespace's prefix ("" or "INBOX."), from a NAMESPACE reply.
fn personal_namespace(line: &str) -> Option<String> {
    match tokens(line).as_slice() {
        [Token::Open, Token::Open, Token::Text(prefix), ..] => Some(prefix.clone()),
        _ => None,
    }
}
/// Where the server keeps personal folders, if it says (NAMESPACE).
pub(crate) fn namespace(session: &mut ImapSession) -> Result<Option<String>> {
    let data = session.run_command_and_read_response("NAMESPACE")?;
    Ok(other_lines(&data, "NAMESPACE")?
        .first()
        .and_then(|line| personal_namespace(line)))
}
/// How much of a mailbox's storage is used and allowed, in KiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Quota {
    pub used_kib: u64,
    pub limit_kib: u64,
}
/// STORAGE from a QUOTA reply: `"" (STORAGE 10 512 MESSAGE 4 1000)`.
fn storage_quota(line: &str) -> Option<Quota> {
    let tokens = tokens(line);
    let start = tokens.iter().position(|t| *t == Token::Open)?;
    let words: Vec<&str> = tokens[start + 1..]
        .iter()
        .take_while(|t| **t != Token::Close)
        .filter_map(|t| match t {
            Token::Text(w) => Some(w.as_str()),
            _ => None,
        })
        .collect();
    words.chunks(3).find_map(|c| match c {
        [resource, used, limit] if resource.eq_ignore_ascii_case("STORAGE") => Some(Quota {
            used_kib: used.parse().ok()?,
            limit_kib: limit.parse().ok()?,
        }),
        _ => None,
    })
}
/// The inbox's storage quota, if the server limits it (GETQUOTAROOT).
pub(crate) fn quota(session: &mut ImapSession) -> Result<Option<Quota>> {
    let data = session.run_command_and_read_response("GETQUOTAROOT INBOX")?;
    Ok(other_lines(&data, "QUOTA")?
        .into_iter()
        // QUOTAROOT lines start with "QUOTA" too; they have no list.
        .filter(|line| !line.to_ascii_uppercase().starts_with("ROOT"))
        .find_map(storage_quota))
}
/// What this sign-in may do in a mailbox (MYRIGHTS), as RFC 4314 letters.
pub(crate) fn my_rights(session: &mut ImapSession, mailbox: &str) -> Result<Option<String>> {
    let data = match session.run_command_and_read_response(format!("MYRIGHTS {}", quote(mailbox)?))
    {
        Ok(data) => data,
        // Not every mailbox answers (shared or virtual ones); no answer is
        // no restriction known.
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(other_lines(&data, "MYRIGHTS")?
        .first()
        .and_then(|line| match tokens(line).as_slice() {
            [_, Token::Text(rights)] => Some(rights.clone()),
            _ => None,
        }))
}
/// Whether `rights` (from MYRIGHTS) lacks one of `needed`. RFC 2086's
/// older letters count too: c for creating (k), d for deleting (x, t, e).
pub(crate) fn lacks_rights(rights: &str, needed: &str) -> Option<char> {
    needed.chars().find(|r| {
        let older = match r {
            'k' => Some('c'),
            'x' | 't' | 'e' => Some('d'),
            _ => None,
        };
        !rights.contains(*r) && !older.is_some_and(|o| rights.contains(o))
    })
}
/// Keywords that mark messages for mail apps and servers, not labels a
/// person set; these aren't shown.
const SYSTEM_KEYWORDS: [&str; 16] = [
    "$Forwarded",
    "$MDNSent",
    "$Junk",
    "$NotJunk",
    "Junk",
    "NonJunk",
    "NotJunk",
    "$Phishing",
    "$NotPhishing",
    "$SubmitPending",
    "$Submitted",
    "$HasAttachment",
    "$HasNoAttachment",
    "$AutoJunk",
    "$AutoNotJunk",
    "$Important",
];
/// A label as an IMAP keyword: an atom, which can't hold spaces or IMAP's
/// special characters.
pub(crate) fn keyword(label: &str) -> Result<String> {
    let label = label.trim();
    ensure!(!label.is_empty(), "Enter a name for the label.");
    ensure!(
        label.bytes().all(|b| b > b' '
            && b < 0x7f
            && !matches!(b, b'(' | b')' | b'{' | b'%' | b'*' | b'"' | b'\\' | b']')),
        "Labels on this server can't contain spaces, accents or ( ) {{ }} % * \" \\ ] — try “{}”.",
        label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.$".contains(c) {
                c
            } else {
                '_'
            })
            .collect::<String>()
    );
    ensure!(
        !SYSTEM_KEYWORDS
            .iter()
            .any(|k| k.eq_ignore_ascii_case(label)),
        "“{label}” is used by mail apps themselves. Choose another name."
    );
    Ok(label.into())
}
/// A label for Gmail's X-GM-LABELS, as Gmail names it (modified UTF-7,
/// see `encode_name`), quoted.
pub(crate) fn gmail_label(label: &str) -> Result<String> {
    let label = label.trim();
    ensure!(!label.is_empty(), "Enter a name for the label.");
    ensure!(
        !label.starts_with('\\'),
        "“{}” is one of Gmail's own labels.",
        decode_name(label)
    );
    quote(label)
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
    /// Whether new keywords may be set here (PERMANENTFLAGS has \\*).
    pub(crate) keywords: bool,
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
                ResponseCode::PermanentFlags(flags) => {
                    selected.keywords = flags.contains(&"\\*");
                }
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
    pub(crate) gmail_labels: Option<Vec<String>>,
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
    /// The labels a person set, when this row says: Gmail's labels (where
    /// asked for, `gmail`), else the keywords among its flags.
    pub(crate) fn labels(&self, gmail: bool) -> Option<Vec<String>> {
        let mut labels: Vec<String> = if gmail {
            self.gmail_labels
                .as_ref()?
                .iter()
                .filter(|l| !l.starts_with('\\'))
                .cloned()
                .collect()
        } else {
            self.flags
                .as_ref()?
                .iter()
                .filter(|f| {
                    !f.starts_with('\\')
                        && !SYSTEM_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(f))
                })
                .cloned()
                .collect()
        };
        labels.sort();
        labels.dedup();
        Some(labels)
    }
}
/// A quoted string's text as imap-proto leaves it: escapes still in.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
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
                A::GmailLabels(labels) => {
                    row.gmail_labels = Some(
                        labels
                            .iter()
                            .map(|l| {
                                if l.starts_with('\\') {
                                    (*l).into()
                                } else {
                                    unescape(l)
                                }
                            })
                            .collect(),
                    )
                }
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
    fn only_own_leaf_folders_can_be_deleted() {
        let name = |name: &str, attributes: &[&str]| Name {
            name: name.into(),
            attributes: attributes.iter().map(|a| (*a).into()).collect(),
            delimiter: Some("/".into()),
        };
        let names = [
            name("INBOX", &[]),
            name("Sent", &["Sent"]),
            name("Receipts", &[]),
            name("Projects", &[]),
            name("Projects/2024", &[]),
            name("[Gmail]", &["NoSelect"]),
            name("[Gmail]/Starred", &[]),
        ];
        assert!(deletable(&names, Provider::Imap, "Receipts").is_ok());
        assert!(deletable(&names, Provider::Imap, "Projects/2024").is_ok());
        let refused = |n: &str, provider| {
            deletable(&names, provider, n).unwrap_err().to_string()
        };
        assert!(refused("INBOX", Provider::Imap).contains("standard"));
        assert!(refused("Sent", Provider::Imap).contains("standard"));
        assert!(refused("Projects", Provider::Imap).contains("folders inside"));
        assert!(refused("[Gmail]/Starred", Provider::Gmail).contains("standard"));
        assert!(refused("Gone", Provider::Imap).contains("no longer"));
    }
    #[test]
    fn folder_names_are_modified_utf7() {
        for (text, wire) in [
            ("Receipts", "Receipts"),
            ("Tom & Jerry", "Tom &- Jerry"),
            ("Café", "Caf&AOk-"),
            ("日本語", "&ZeVnLIqe-"),
            ("~peter/mail/台北/日本語", "~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
        ] {
            assert_eq!(encode_name(text), wire);
            assert_eq!(decode_name(wire), text);
        }
        assert_eq!(decode_name("Broken &AOk"), "Broken &AOk");
    }
    #[test]
    fn new_and_renamed_folders_are_named_like_their_neighbours() {
        let name = |name: &str, attributes: &[&str], d: Option<&str>| Name {
            name: name.into(),
            attributes: attributes.iter().map(|a| (*a).into()).collect(),
            delimiter: d.map(Into::into),
        };
        let slash = [
            name("INBOX", &[], Some("/")),
            name("Sent", &["Sent"], Some("/")),
            name("Receipts", &[], Some("/")),
            name("Projects", &[], Some("/")),
            name("Projects/2024", &[], Some("/")),
            name("Flat", &["NoInferiors"], Some("/")),
        ];
        let new = |parent, leaf: &str| new_folder_name(&slash, Provider::Imap, parent, leaf, None);
        assert_eq!(new(None, " Travel ").unwrap(), "Travel");
        assert_eq!(new(Some("Projects"), "2025").unwrap(), "Projects/2025");
        assert_eq!(new(None, "Café").unwrap(), "Caf&AOk-");
        assert!(
            new(Some("Projects"), "2024")
                .unwrap_err()
                .to_string()
                .contains("already")
        );
        assert!(
            new(None, "sent")
                .unwrap_err()
                .to_string()
                .contains("standard")
        );
        assert!(new(None, "a/b").unwrap_err().to_string().contains("“/”"));
        assert!(new(None, "  ").unwrap_err().to_string().contains("Enter"));
        assert!(
            new(Some("Flat"), "x")
                .unwrap_err()
                .to_string()
                .contains("doesn't allow")
        );
        assert!(new(Some("Sent"), "x").is_err());
        let rename = |n, leaf: &str| renamed_folder_name(&slash, Provider::Imap, n, leaf, None);
        assert_eq!(rename("Projects/2024", "Old").unwrap(), "Projects/Old");
        assert_eq!(rename("Receipts", "Bills").unwrap(), "Bills");
        assert_eq!(rename("Receipts", "Receipts").unwrap(), "Receipts");
        assert!(
            rename("Receipts", "Projects")
                .unwrap_err()
                .to_string()
                .contains("already")
        );
        assert!(
            rename("Sent", "Posted")
                .unwrap_err()
                .to_string()
                .contains("standard")
        );
        assert_eq!(
            folder_and_inside(&slash, "Projects"),
            ["Projects", "Projects/2024"]
        );
        // Servers that keep every folder inside the inbox.
        let dotted = [
            name("INBOX", &[], Some(".")),
            name("INBOX.Sent", &["Sent"], Some(".")),
            name("INBOX.Receipts", &[], Some(".")),
        ];
        assert_eq!(
            new_folder_name(&dotted, Provider::Imap, None, "Travel", None).unwrap(),
            "INBOX.Travel"
        );
        assert!(
            new_folder_name(&dotted, Provider::Imap, None, "Trash", None)
                .unwrap_err()
                .to_string()
                .contains("standard")
        );
        assert_eq!(
            renamed_folder_name(&dotted, Provider::Imap, "INBOX.Receipts", "Bills", None).unwrap(),
            "INBOX.Bills"
        );
        // Without a hierarchy, folders can't be nested.
        let flat = [name("INBOX", &[], None), name("Receipts", &[], None)];
        assert!(new_folder_name(&flat, Provider::Imap, Some("Receipts"), "x", None).is_err());
        assert_eq!(
            new_folder_name(&flat, Provider::Imap, None, "a/b", None).unwrap(),
            "a/b"
        );
    }
    #[test]
    fn extension_replies_are_read() {
        assert_eq!(
            personal_namespace(r#"(("INBOX." ".")) NIL NIL"#).as_deref(),
            Some("INBOX.")
        );
        assert_eq!(
            personal_namespace(r#"(("" "/")) (("Other Users/" "/")) NIL"#).as_deref(),
            Some("")
        );
        assert_eq!(personal_namespace("NIL NIL NIL"), None);
        assert_eq!(
            storage_quota(r#""" (MESSAGE 4 1000 STORAGE 10 512)"#),
            Some(Quota {
                used_kib: 10,
                limit_kib: 512
            })
        );
        assert_eq!(storage_quota(r#""" (MESSAGE 4 1000)"#), None);
        // RFC 4314 rights, and RFC 2086's older letters.
        assert_eq!(lacks_rights("lrswipkxte", "kxte"), None);
        assert_eq!(lacks_rights("lrswi", "te"), Some('t'));
        assert_eq!(lacks_rights("lrswicd", "kxte"), None);
        // Untagged extension data reaches the client as text.
        let data = b"* NAMESPACE ((\"\" \"/\")) NIL NIL\r\n* MYRIGHTS \"INBOX\" lrswi\r\n";
        assert_eq!(other_lines(data, "MYRIGHTS").unwrap(), [r#""INBOX" lrswi"#]);
    }
    #[test]
    fn labels_are_gmail_labels_or_keywords() {
        let data = b"* 1 FETCH (UID 7 FLAGS (\\Seen $Forwarded Work) X-GM-LABELS (\\Inbox \"Tax \\\"24\\\"\" Caf&AOk-))\r\n";
        let imap_proto::Response::Fetch(_, attributes) = &responses(data).unwrap()[0] else {
            panic!("not a fetch")
        };
        assert!(
            attributes
                .iter()
                .any(|a| matches!(a, imap_proto::AttributeValue::GmailLabels(l) if l.len() == 3))
        );
        let row = Row {
            flags: Some(vec!["\\Seen".into(), "$Forwarded".into(), "Work".into()]),
            gmail_labels: Some(vec!["\\Inbox".into(), "Tax \"24\"".into()]),
            ..Row::default()
        };
        assert_eq!(row.labels(false).unwrap(), ["Work"]);
        assert_eq!(row.labels(true).unwrap(), ["Tax \"24\""]);
        assert_eq!(Row::default().labels(true), None);
        assert_eq!(keyword(" Work ").unwrap(), "Work");
        assert!(keyword("two words").is_err());
        assert!(keyword("$MDNSent").is_err());
        assert_eq!(gmail_label("Caf&AOk-").unwrap(), "\"Caf&AOk-\"");
        assert!(gmail_label("\\Starred").is_err());
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
