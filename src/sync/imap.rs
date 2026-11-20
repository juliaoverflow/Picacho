//! Verified IMAP connections, stable identities, and remote folder resolution.
use super::{
    sasl::{self, Mechanism},
    stream::ImapStream,
    wire::Wire,
};
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
        .context(super::retry::Unreachable(
            "Server hostname could not be resolved".into(),
        ))?;
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
    Err(super::retry::Unreachable(format!(
        "Could not connect to {host}:{port}: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
    .into())
}
pub(crate) fn imap_login(
    a: &Account,
    s: &Secret,
    tls: &native_tls::TlsConnector,
) -> Result<ImapSession> {
    imap_login_interruptible(a, s, tls).map(|(session, ..)| session)
}
/// Signs in, also returning a copy of what the server sends when asked
/// (see `Tap`), for response codes the imap crate drops (COPYUID,
/// APPENDUID in a tagged OK).
pub(crate) fn imap_login_tapped(
    a: &Account,
    s: &Secret,
    tls: &native_tls::TlsConnector,
) -> Result<(ImapSession, super::stream::Tap)> {
    imap_login_interruptible(a, s, tls).map(|(session, _, tap)| (session, tap))
}
/// The new UIDs a server gave copied or moved messages (UIDPLUS's
/// `[COPYUID validity from to]`): the target's UIDVALIDITY, and each
/// source UID with its new one.
pub(crate) fn copy_uids(heard: &[u8]) -> Option<(u32, Vec<(u32, u32)>)> {
    let text = String::from_utf8_lossy(heard);
    let start = text.find("[COPYUID ")? + "[COPYUID ".len();
    let end = start + text[start..].find(']')?;
    let mut words = text[start..end].split_ascii_whitespace();
    let validity = words.next()?.parse().ok()?;
    let from = uid_set_list(words.next()?)?;
    let to = uid_set_list(words.next()?)?;
    (from.len() == to.len()).then(|| (validity, from.into_iter().zip(to).collect()))
}
/// The UID a server gave an appended message (`[APPENDUID validity uid]`).
pub(crate) fn append_uid(heard: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(heard);
    let start = text.find("[APPENDUID ")? + "[APPENDUID ".len();
    let end = start + text[start..].find(']')?;
    let mut words = text[start..end].split_ascii_whitespace();
    let validity = words.next()?.parse().ok()?;
    let uid = words.next()?;
    // One message appended: one UID (MULTIAPPEND isn't used).
    uid.parse().ok().map(|uid| (validity, uid))
}
/// A UID set ("4,7:9") in order, as COPYUID pairs them: a range counts
/// from its first number to its second, either way.
fn uid_set_list(set: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in set.split(',') {
        match part.split_once(':') {
            Some((a, b)) => {
                let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
                if b.abs_diff(a) > 100_000 {
                    return None;
                }
                if a <= b {
                    out.extend(a..=b);
                } else {
                    out.extend((b..=a).rev());
                }
            }
            None => out.push(part.parse().ok()?),
        }
    }
    Some(out)
}
/// Signs in, also returning a handle that closes the connection from
/// another thread (to end a session waiting in IDLE), and one that copies
/// what the server sends (to see what woke an IDLE).
pub(crate) fn imap_login_interruptible(
    a: &Account,
    s: &Secret,
    tls: &native_tls::TlsConnector,
) -> Result<(ImapSession, TcpStream, super::stream::Tap)> {
    let mut socket = tcp(&a.incoming_host, a.incoming_port)?;
    if a.incoming_starttls {
        // The imap crate's secure() only accepts TcpStream. Perform the small
        // plaintext upgrade here so bounded I/O also covers STARTTLS sessions.
        starttls(&mut socket)?;
    }
    let mut stream = ImapStream::new(
        tls.connect(&a.incoming_host, socket)
            .map_err(|e| anyhow::anyhow!("IMAP TLS handshake failed: {e}"))?,
    );
    let compressing = stream.compression_switch();
    let interrupter = stream.interrupter()?;
    let tap = stream.tap();
    // What the server offers for signing in, asked before the imap crate
    // takes the stream (it has no CAPABILITY before login). IMAP sends no
    // second greeting after STARTTLS.
    let caps = pre_auth_capabilities(&mut stream, !a.incoming_starttls)?;
    let mut client = imap::Client::new(stream);
    client.greeting_read = true;
    let offered = auth_mechanisms(&caps);
    let credentials = |user: &str, secret: &str| sasl::Credentials {
        user: user.to_owned(),
        secret: secret.to_owned(),
        host: a.incoming_host.clone(),
        port: a.incoming_port,
    };
    let session = if a.oauth {
        // XOAUTH2 when the server lists neither: Gmail and Outlook take it.
        let mechanism = Mechanism::choose(&sasl::TOKEN_ORDER, offered.iter().map(String::as_str))
            .unwrap_or(Mechanism::XOAuth2);
        authenticate(client, mechanism, credentials(&a.email, &s.access_token))
    } else if let Some(mechanism) = Mechanism::choose(
        &[Mechanism::ScramSha256, Mechanism::ScramSha1],
        offered.iter().map(String::as_str),
    ) {
        authenticate(client, mechanism, credentials(a.login(), &s.password))
    } else if !caps.iter().any(|c| c == "LOGINDISABLED") {
        client
            .login(a.login(), &s.password)
            .map_err(|(e, _)| e.into())
    } else {
        match Mechanism::choose(
            &[Mechanism::Plain, Mechanism::Login, Mechanism::CramMd5],
            offered.iter().map(String::as_str),
        ) {
            Some(mechanism) => authenticate(client, mechanism, credentials(a.login(), &s.password)),
            None => bail!(
                "{} offers no sign-in method Picacho supports.",
                a.incoming_host
            ),
        }
    };
    let mut session = session.map_err(|e| {
        let rejected = e
            .downcast_ref::<imap::Error>()
            .is_some_and(|e| matches!(e, imap::Error::No(_) | imap::Error::Bad(_)));
        let error = e.context("IMAP authentication failed");
        if rejected {
            error.context(crate::manage::status::AccessFailure::Authentication)
        } else {
            error
        }
    })?;
    prepare(&mut session, &compressing)?;
    Ok((session, interrupter, tap))
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
/// A SASL exchange as the imap crate drives it: each challenge in, each
/// answer out. A failed step answers with nothing, which the server refuses;
/// the step's own error is kept to say why.
struct Sasl {
    exchange: std::cell::RefCell<sasl::Exchange>,
    failed: std::cell::RefCell<Option<anyhow::Error>>,
}
impl imap::Authenticator for Sasl {
    type Response = Vec<u8>;
    fn process(&self, challenge: &[u8]) -> Vec<u8> {
        match self.exchange.borrow_mut().step(challenge) {
            Ok(answer) => answer,
            Err(e) => {
                *self.failed.borrow_mut() = Some(e);
                Vec::new()
            }
        }
    }
}
/// Signs in with AUTHENTICATE and a SASL mechanism.
fn authenticate(
    client: imap::Client<ImapStream>,
    mechanism: Mechanism,
    credentials: sasl::Credentials,
) -> Result<ImapSession> {
    let auth = Sasl {
        exchange: std::cell::RefCell::new(sasl::Exchange::new(mechanism, credentials)),
        failed: std::cell::RefCell::new(None),
    };
    let result = client.authenticate(mechanism.name(), &auth);
    if let Some(e) = auth.failed.borrow_mut().take() {
        return Err(e.context(crate::manage::status::AccessFailure::Authentication));
    }
    let mut session = match result {
        Ok(session) => session,
        Err((e, _)) => {
            let reason = auth.exchange.borrow().refusal().map(str::to_owned);
            let e = anyhow::Error::new(e);
            return Err(match reason {
                Some(reason) => e.context(format!("The server refused the token ({reason})")),
                None => e,
            });
        }
    };
    // SCRAM: the server must also have proved it knows the password.
    if let Err(e) = auth.exchange.borrow_mut().finish(None) {
        let _ = session.logout();
        return Err(e.context(crate::manage::status::AccessFailure::Authentication));
    }
    Ok(session)
}
/// The greeting (unless already read, after STARTTLS) and CAPABILITY,
/// before signing in, read straight from the stream so nothing is left
/// buffered for the imap crate. Capabilities in uppercase.
fn pre_auth_capabilities(stream: &mut ImapStream, greeting: bool) -> Result<Vec<String>> {
    use std::io::Write;
    let words = |text: &str| -> Vec<String> {
        text.split_ascii_whitespace()
            .map(str::to_ascii_uppercase)
            .collect()
    };
    if greeting {
        let hello = read_line(stream)?;
        ensure!(
            hello.starts_with("* OK") && !hello.starts_with("* OKAY"),
            "IMAP server rejected connection"
        );
        // Servers often list their capabilities in the greeting.
        if let Some((listed, _)) = hello
            .split_once("[CAPABILITY ")
            .and_then(|(_, rest)| rest.split_once(']'))
        {
            return Ok(words(listed));
        }
    }
    stream.write_all(b"p0 CAPABILITY\r\n")?;
    stream.flush()?;
    let mut caps = Vec::new();
    loop {
        let reply = read_line(stream)?;
        if let Some(rest) = reply.strip_prefix("p0 ") {
            ensure!(rest.starts_with("OK"), "The IMAP server refused CAPABILITY");
            return Ok(caps);
        }
        if let Some(listed) = reply.strip_prefix("* CAPABILITY ") {
            caps.extend(words(listed));
        }
    }
}
/// One line from the server, a byte at a time so nothing past it is read.
fn read_line(stream: &mut impl std::io::Read) -> Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    while !bytes.ends_with(b"\r\n") {
        ensure!(bytes.len() <= 8192, "The IMAP server's reply is too long");
        let n = stream.read(&mut byte)?;
        ensure!(n == 1, "The IMAP server closed the connection");
        bytes.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&bytes).trim_end().to_owned())
}
/// The SASL mechanisms in a capability list ("AUTH=PLAIN"…).
fn auth_mechanisms(caps: &[String]) -> Vec<String> {
    caps.iter()
        .filter_map(|c| c.strip_prefix("AUTH="))
        .map(str::to_owned)
        .collect()
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
    /// Whether the server gave this mailbox `attribute`. Attributes are
    /// case-insensitive (\\Noselect, \\NoSelect…).
    fn has(&self, attribute: &str) -> bool {
        let attribute = attribute.to_ascii_lowercase();
        self.attributes
            .iter()
            .any(|a| a.to_ascii_lowercase().contains(&attribute))
    }
    pub(crate) fn delimiter(&self) -> Option<&str> {
        self.delimiter.as_deref().filter(|d| !d.is_empty())
    }
    /// A mailbox as LIST would give it, for tests elsewhere.
    #[cfg(test)]
    pub(crate) fn for_test(name: &str, delimiter: &str, attributes: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            attributes: attributes.iter().map(|a| (*a).to_owned()).collect(),
            delimiter: Some(delimiter.to_owned()),
        }
    }
    /// Whether folders can be made inside this one: the server has a
    /// hierarchy and has not marked it \Noinferiors.
    fn nestable(&self) -> bool {
        self.delimiter().is_some() && !self.has("NoInferiors") && !self.has("Noinferiors")
    }
}
/// What a listing asks for besides the names.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ListOptions {
    /// Standard folders by role (SPECIAL-USE).
    pub(crate) special_use: bool,
    /// Each folder's counters in the same reply (LIST-STATUS, RFC 5819),
    /// with HIGHESTMODSEQ where the server has CONDSTORE.
    pub(crate) list_status: bool,
    pub(crate) condstore: bool,
    /// Only subscribed folders are shown: the others get `HIDDEN`, unless
    /// none of the account's own folders is subscribed (a server or person
    /// that doesn't use subscriptions).
    pub(crate) subscribed_only: bool,
}

/// A folder's counters from the listing (LIST-STATUS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FolderStatus {
    pub(crate) validity: u32,
    pub(crate) uid_next: u32,
    pub(crate) messages: u32,
    pub(crate) modseq: Option<u64>,
}

/// Picacho's own mark on a listed folder that isn't shown (an
/// unsubscribed one): never an attribute a server sends.
const HIDDEN: &str = "\\PicachoHidden";

/// Every mailbox on the server, also in the namespaces `others` names
/// (other users' and shared folders, where the server doesn't list them
/// with the rest), with what `options` asks for. Names as the cache keeps
/// them; counters by name too.
pub(crate) fn list_full(
    session: &mut ImapSession,
    options: &ListOptions,
    others: &[String],
    utf8: bool,
) -> Result<(Vec<Name>, std::collections::HashMap<String, FolderStatus>)> {
    let extended = options.special_use || options.list_status;
    let mut returns = Vec::new();
    if options.special_use {
        returns.push("SPECIAL-USE".to_owned());
    }
    if options.subscribed_only && extended {
        returns.push("SUBSCRIBED".to_owned());
    }
    if options.list_status {
        returns.push(format!(
            "STATUS (UIDVALIDITY UIDNEXT MESSAGES{})",
            if options.condstore { " HIGHESTMODSEQ" } else { "" }
        ));
    }
    let returns = returns.join(" ");
    let (mut names, mut status) = list_pattern(session, "*", &returns)?;
    for prefix in others {
        if prefix.is_empty() || names.iter().any(|n| n.name.starts_with(prefix.as_str())) {
            continue;
        }
        // A namespace the server lists apart; one it refuses is skipped.
        let pattern = format!("{prefix}*");
        let Ok((more, counted)) = list_pattern(session, &pattern, &returns) else {
            continue;
        };
        for n in more {
            if !names.iter().any(|m| m.name == n.name) {
                names.push(n);
            }
        }
        status.extend(counted);
    }
    if options.subscribed_only {
        let subscribed: std::collections::HashSet<String> = if extended {
            names
                .iter()
                .filter(|n| n.has("\\Subscribed"))
                .map(|n| n.name.clone())
                .collect()
        } else {
            session
                .lsub(Some(""), Some("*"))?
                .iter()
                .map(|n| n.name().to_owned())
                .collect()
        };
        let any = custom_folders(&names)
            .iter()
            .any(|f| subscribed.contains(&f.name));
        if any {
            for n in &mut names {
                if !subscribed.contains(&n.name) && !n.name.eq_ignore_ascii_case("INBOX") {
                    n.attributes.push(HIDDEN.to_owned());
                }
            }
        }
    }
    let cache = |name: &str| super::imap_ext::cache_name(name, utf8);
    Ok((
        names
            .into_iter()
            .map(|n| Name {
                name: cache(&n.name),
                ..n
            })
            .collect(),
        status.into_iter().map(|(k, v)| (cache(&k), v)).collect(),
    ))
}

/// The mailboxes `pattern` matches, with `returns` (RETURN options, empty
/// for none) and the counters LIST-STATUS sends with them. Wire names.
fn list_pattern(
    session: &mut ImapSession,
    pattern: &str,
    returns: &str,
) -> Result<(Vec<Name>, std::collections::HashMap<String, FolderStatus>)> {
    let mut status = std::collections::HashMap::new();
    if returns.is_empty() {
        return Ok((
            session
                .list(Some(""), Some(pattern))?
                .iter()
                .map(|n| Name {
                    name: n.name().into(),
                    attributes: n.attributes().iter().map(|a| format!("{a:?}")).collect(),
                    delimiter: n.delimiter().map(Into::into),
                })
                .collect(),
            status,
        ));
    }
    let data = session.run_command_and_read_response(format!(
        "LIST \"\" {} RETURN ({returns})",
        quote(pattern)?
    ))?;
    let mut names = Vec::new();
    for response in responses(&data)? {
        match response {
            imap_proto::Response::MailboxData(imap_proto::MailboxDatum::List {
                flags,
                delimiter,
                name,
            }) => names.push(Name {
                name: name.into(),
                attributes: flags.iter().map(|f| (*f).into()).collect(),
                delimiter: delimiter.map(Into::into),
            }),
            imap_proto::Response::MailboxData(imap_proto::MailboxDatum::Status {
                mailbox,
                status: attributes,
            }) => {
                let mut s = FolderStatus::default();
                for a in attributes {
                    match a {
                        imap_proto::StatusAttribute::UidValidity(v) => s.validity = v,
                        imap_proto::StatusAttribute::UidNext(v) => s.uid_next = v,
                        imap_proto::StatusAttribute::Messages(v) => s.messages = v,
                        imap_proto::StatusAttribute::HighestModSeq(v) => s.modseq = Some(v),
                        _ => {}
                    }
                }
                status.insert(mailbox.to_owned(), s);
            }
            _ => {}
        }
    }
    Ok((names, status))
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
        // Placeholders in the hierarchy can't be opened: \\Noselect, and
        // \\NonExistent from servers with LIST-EXTENDED.
        .filter(|n| !n.has("NoSelect") && !n.has("NonExistent") && !n.has(HIDDEN))
        .filter(|n| !STANDARD_ATTRIBUTES.iter().any(|a| n.has(a)))
        .filter(|n| !gmail_system(&n.name))
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
    // A folder whose parents aren't listed (another user's or a shared
    // folder, under a namespace that can't be opened, or one inside a
    // hidden folder) sits under the nearest one that is, its label naming
    // the path between: "alice/Projects" rather than an orphan "Projects".
    let delimiters: std::collections::HashMap<&str, &str> = names
        .iter()
        .filter_map(|n| Some((n.name.as_str(), n.delimiter()?)))
        .collect();
    let mut placed: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut by_length: Vec<usize> = (0..out.len()).collect();
    by_length.sort_by_key(|&i| out[i].name.len());
    for i in by_length {
        let name = out[i].name.clone();
        let Some(d) = delimiters.get(name.as_str()).copied() else {
            placed.insert(name, out[i].depth);
            continue;
        };
        let parts: Vec<&str> = name.split(d).collect();
        let first = usize::from(parts.len() > 1 && parts[0].eq_ignore_ascii_case("INBOX"));
        let parent = (first + 1..parts.len()).rev().find_map(|end| {
            let ancestor = parts[..end].join(d);
            placed.get(&ancestor).map(|&depth| (end, depth))
        });
        let (from, depth) = match parent {
            Some((end, depth)) => (end, depth + 1),
            None => (first, 0),
        };
        if from + 1 < parts.len() || depth != out[i].depth {
            out[i].depth = depth;
            out[i].label = parts[from..]
                .iter()
                .map(|p| decode_name(p))
                .collect::<Vec<_>>()
                .join("/");
        }
        placed.insert(name, depth);
    }
    out
}
/// Whether `name` is Gmail's system container ("[Gmail]", or "[Google
/// Mail]" in some countries) or a mailbox inside it. The container can't be
/// opened, everything in it is a standard mailbox, and Gmail allows no
/// labels of one's own there, so none of it is listed as a folder.
pub(crate) fn gmail_system(name: &str) -> bool {
    ["[Gmail]", "[Google Mail]"].iter().any(|root| {
        name.get(..root.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(root))
            && matches!(name.as_bytes().get(root.len()), None | Some(b'/' | b'.'))
    })
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
    // All Mail is every mailbox at once, not a server folder.
    ensure!(folder != Folder::All, "All Mail is not a folder on the server.");
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
/// and Cc are small and let the list be searched by them (`reply-to:`);
/// References and In-Reply-To join conversations before bodies download,
/// and Disposition-Notification-To says the sender asks for a read receipt.
pub(crate) const LIST_HEADERS: &str =
    "BODY.PEEK[HEADER.FIELDS (FROM REPLY-TO TO CC SUBJECT DATE MESSAGE-ID REFERENCES IN-REPLY-TO CONTENT-TYPE LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST DISPOSITION-NOTIFICATION-TO)]";

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
    /// RFC 7162: removed UIDs reported on SELECT (VANISHED), QRESYNC.
    pub(crate) qresync: bool,
    /// RFC 4731: compact search results (ESEARCH, `RETURN (ALL)`).
    pub(crate) esearch: bool,
    /// RFC 5256: server-side ordering (SORT) and threads (THREAD=REFERENCES).
    pub(crate) sort: bool,
    pub(crate) thread: bool,
    /// RFC 6855: UTF-8 mailbox names and headers (UTF8=ACCEPT).
    pub(crate) utf8_accept: bool,
    /// RFC 5161: ENABLE, needed for QRESYNC and UTF8=ACCEPT.
    pub(crate) enable: bool,
    /// RFC 5465: changes in every mailbox over one connection (NOTIFY).
    pub(crate) notify: bool,
    /// RFC 5464: annotations on mailboxes and the server (METADATA).
    pub(crate) metadata: bool,
    /// RFC 8474: permanent message and thread ids (OBJECTID).
    pub(crate) objectid: bool,
    /// RFC 8970: a short preview of each message (PREVIEW).
    pub(crate) preview: bool,
    /// RFC 8514: when a message was saved in its mailbox (SAVEDATE).
    pub(crate) savedate: bool,
    /// RFC 3516: parts decoded by the server (BINARY).
    pub(crate) binary: bool,
    /// RFC 7888: literals that need no go-ahead (LITERAL+ or LITERAL-).
    pub(crate) literal_plus: bool,
    /// RFC 9051: the server speaks only IMAP4rev2, whose base protocol has
    /// MOVE, UIDPLUS, ESEARCH, NAMESPACE, SPECIAL-USE, LIST-STATUS… without
    /// listing them (they are set here too), and answers SEARCH with
    /// ESEARCH.
    pub(crate) rev2: bool,
    /// RFC 5819: folder counts in the listing (LIST … RETURN (STATUS)).
    pub(crate) list_status: bool,
    /// RFC 9394: results a page at a time (PARTIAL).
    pub(crate) partial: bool,
    /// RFC 9586: messages named by UID alone, once enabled (UIDONLY).
    pub(crate) uidonly: bool,
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
    // A server that speaks only IMAP4rev2 runs its sessions as rev2 from
    // the start, with its folded-in extensions. One offering IMAP4rev1 too
    // stays rev1 (Picacho doesn't ENABLE rev2), with what it lists.
    let rev2 = caps.has_str("IMAP4rev2") && !caps.has_str("IMAP4rev1");
    Ok(Extensions {
        condstore: caps.has_str("CONDSTORE"),
        gmail: caps.has_str("X-GM-EXT-1"),
        moves: rev2 || caps.has_str("MOVE"),
        uidplus: rev2 || caps.has_str("UIDPLUS"),
        idle: rev2 || caps.has_str("IDLE"),
        namespace: rev2 || caps.has_str("NAMESPACE"),
        quota: caps.has_str("QUOTA") || with("QUOTA=").is_some(),
        acl: caps.has_str("ACL"),
        special_use: rev2 || caps.has_str("SPECIAL-USE"),
        append_limit,
        qresync: caps.has_str("QRESYNC"),
        esearch: rev2 || caps.has_str("ESEARCH"),
        sort: caps.has_str("SORT"),
        thread: caps.has_str("THREAD=REFERENCES"),
        utf8_accept: caps.has_str("UTF8=ACCEPT") || caps.has_str("UTF8=ONLY"),
        enable: rev2 || caps.has_str("ENABLE"),
        notify: caps.has_str("NOTIFY"),
        metadata: caps.has_str("METADATA") || caps.has_str("METADATA-SERVER"),
        objectid: caps.has_str("OBJECTID"),
        preview: caps.has_str("PREVIEW"),
        savedate: caps.has_str("SAVEDATE"),
        binary: rev2 || caps.has_str("BINARY"),
        literal_plus: rev2 || caps.has_str("LITERAL+") || caps.has_str("LITERAL-"),
        rev2,
        list_status: rev2 || caps.has_str("LIST-STATUS"),
        partial: caps.has_str("PARTIAL"),
        uidonly: caps.has_str("UIDONLY"),
    })
}

/// The untagged lines of an extension's reply that start with `name`, as
/// text after it (NAMESPACE, QUOTA…; see the vendored parser's `resp_other`).
pub(crate) fn other_lines<'a>(data: &'a [u8], name: &str) -> Result<Vec<&'a str>> {
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
pub(crate) enum Token {
    Open,
    Close,
    Nil,
    Text(String),
}
pub(crate) fn tokens(line: &str) -> Vec<Token> {
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
/// The personal namespace's prefix, and those of other users' and shared
/// folders (NAMESPACE).
pub(crate) fn namespaces(session: &mut ImapSession) -> Result<(Option<String>, Vec<String>)> {
    let data = session.run_command_and_read_response("NAMESPACE")?;
    let lines = other_lines(&data, "NAMESPACE")?;
    let Some(line) = lines.first() else {
        return Ok((None, Vec::new()));
    };
    Ok((personal_namespace(line), other_namespaces(line)))
}
/// The prefixes of the other users' and shared namespaces (the second and
/// third groups of a NAMESPACE reply).
fn other_namespaces(line: &str) -> Vec<String> {
    let tokens = tokens(line);
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < tokens.len() && groups.len() < 3 {
        match &tokens[i] {
            Token::Nil => {
                groups.push(Vec::new());
                i += 1;
            }
            Token::Open => {
                // (("prefix" "delim" ext…) …): each namespace's first word.
                let mut depth = 0;
                let mut prefixes = Vec::new();
                while i < tokens.len() {
                    match &tokens[i] {
                        Token::Open => {
                            depth += 1;
                            if depth == 2
                                && let Some(Token::Text(prefix)) = tokens.get(i + 1)
                            {
                                prefixes.push(prefix.clone());
                            }
                        }
                        Token::Close => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                groups.push(prefixes);
            }
            _ => i += 1,
        }
    }
    groups.into_iter().skip(1).flatten().collect()
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
/// Whether a JMAP keyword is a label a person set: not IMAP's system flags
/// (`$seen`, `$flagged`, `$answered`, `$draft`, which JMAP spells as
/// keywords) nor `SYSTEM_KEYWORDS`.
pub(crate) fn label_keyword(k: &str) -> bool {
    !["$seen", "$flagged", "$answered", "$draft", "$recent"]
        .iter()
        .chain(SYSTEM_KEYWORDS.iter())
        .any(|s| s.eq_ignore_ascii_case(k))
}
/// A label as an IMAP keyword: an atom, which can't hold spaces or IMAP's
/// special characters.
pub(crate) fn keyword(label: &str) -> Result<String> {
    let label = label.trim();
    ensure!(!label.is_empty(), "Enter a name for the label.");
    ensure!(
        label.bytes().all(|b| b > b' '
            && b < 0x7f
            && !matches!(b, b'(' | b')' | b'{' | b'}' | b'%' | b'*' | b'"' | b'\\' | b']')),
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
pub(crate) fn select(
    session: &mut ImapSession,
    name: &str,
    condstore: bool,
    utf8: bool,
) -> Result<Selected> {
    let command = format!(
        "SELECT {}{}",
        quote(&super::imap_ext::wire_name(name, utf8))?,
        if condstore { " (CONDSTORE)" } else { "" }
    );
    let data = session.run_command_and_read_response(command)?;
    selected_from(&data, name, condstore)
}
/// What a SELECT's reply says about the mailbox `name`.
pub(crate) fn selected_from(data: &[u8], name: &str, condstore: bool) -> Result<Selected> {
    let mut selected = Selected {
        name: name.into(),
        ..Default::default()
    };
    let mut validity = None;
    for response in responses(data)? {
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
    /// The server's thread: Gmail's X-GM-THRID or OBJECTID's THREADID.
    pub(crate) thread: Option<String>,
    /// OBJECTID's EMAILID: the same message in every mailbox.
    pub(crate) email_id: Option<String>,
    /// PREVIEW's text.
    pub(crate) preview: Option<String>,
    /// SAVEDATE: when it was put in this mailbox.
    pub(crate) save_date: Option<i64>,
    /// MODSEQ, with CONDSTORE.
    pub(crate) modseq: Option<u64>,
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
    /// Replied to (\\Answered).
    pub(crate) fn answered(&self) -> bool {
        self.flag("\\Answered")
    }
    /// Forwarded ($Forwarded, RFC 5550).
    pub(crate) fn forwarded(&self) -> bool {
        self.flag("$Forwarded")
    }
    /// A read receipt was sent or declined ($MDNSent, RFC 3503).
    pub(crate) fn mdn_sent(&self) -> bool {
        self.flag("$MDNSent")
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
pub(crate) fn unescape(text: &str) -> String {
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
    rows_from(&data)
}
/// The FETCH rows in a reply (with a UID; others are skipped).
pub(crate) fn rows_from(data: &[u8]) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for response in responses(data)? {
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
                A::GmailThrId(id) => row.thread = Some(format!("gm:{id}")),
                A::ThreadId(Some(id)) => row.thread = Some(format!("oid:{id}")),
                A::EmailId(id) => row.email_id = Some(id.to_owned()),
                A::Preview(Some(text)) => {
                    row.preview = Some(String::from_utf8_lossy(text).trim().to_owned())
                }
                A::SaveDate(Some(date)) => row.save_date = internal_date(date),
                A::ModSeq(m) => row.modseq = Some(m),
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
pub(crate) fn internal_date(date: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(date.trim(), "%d-%b-%Y %H:%M:%S %z")
        .ok()
        .map(|d| d.timestamp())
}
/// Untagged responses that the imap crate has already read and validated.
pub(crate) fn responses(data: &[u8]) -> Result<Vec<imap_proto::Response<'_>>> {
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
        // Gmail's container and its mailboxes are never listed as folders,
        // however the server marks them.
        let listed: Vec<String> = custom_folders(&[
            name("[Gmail]", &["\\HasChildren"]),
            name("[Google Mail]/Important", &[]),
            name("Placeholder", &["\\NONEXISTENT"]),
            name("Old", &["\\noselect"]),
            name("[Gmail] Receipts", &[]),
            name("Receipts", &[]),
        ])
        .into_iter()
        .map(|f| f.name)
        .collect();
        assert_eq!(listed, ["[Gmail] Receipts", "Receipts"]);
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
            copy_uids(b"* OK [COPYUID 38505 304,319:320 3956:3958] Done\r\n"),
            Some((38505, vec![(304, 3956), (319, 3957), (320, 3958)]))
        );
        assert_eq!(copy_uids(b"a3 OK [COPYUID 9 1:2 5] Odd\r\n"), None);
        assert_eq!(
            append_uid(b"a4 OK [APPENDUID 38505 3955] APPEND completed\r\n"),
            Some((38505, 3955))
        );
        assert_eq!(
            other_namespaces(r#"(("" "/")) (("Other Users/" "/")) (("Shared/" "/") ("Public/" "/"))"#),
            ["Other Users/", "Shared/", "Public/"]
        );
        assert!(other_namespaces(r#"(("INBOX." ".")) NIL NIL"#).is_empty());
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
        // Each character the message names is refused.
        for refused in ["a(b", "a)b", "a{b", "a}b", "a%b", "a*b", "a\"b", "a\\b", "a]b", "café"] {
            assert!(keyword(refused).is_err(), "{refused}");
        }
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
