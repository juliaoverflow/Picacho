//! Verified IMAP connections, stable identities, and remote folder resolution.
use crate::manage::model::*;
use anyhow::{Context, Result, bail, ensure};
use std::{
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};
pub(crate) type ImapSession = imap::Session<native_tls::TlsStream<TcpStream>>;
pub(crate) fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    let addrs = (host, port)
        .to_socket_addrs()
        .context("Server hostname could not be resolved")?;
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(12)) {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_secs(30)))?;
                s.set_write_timeout(Some(Duration::from_secs(30)))?;
                return Ok(s);
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
    let socket = tcp(&a.incoming_host, a.incoming_port)?;
    let client = if a.incoming_starttls {
        let mut plain = imap::Client::new(socket);
        plain.read_greeting()?;
        plain
            .secure(&a.incoming_host, tls)
            .context("IMAP STARTTLS failed. Check that Proton Mail Bridge is running and its certificate is trusted.")?
    } else {
        let stream = tls
            .connect(&a.incoming_host, socket)
            .context("IMAP TLS handshake failed")?;
        let mut client = imap::Client::new(stream);
        client.read_greeting()?;
        client
    };
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
    session.map_err(|(e, _)| anyhow::anyhow!("IMAP authentication failed: {e}"))
}
struct Xoauth {
    user: String,
    token: String,
}
pub(crate) fn select_message_mailbox(session: &mut ImapSession, mail: &Mail) -> Result<()> {
    let mailbox = session.select(&mail.remote_folder)?;
    let expected = mail
        .id
        .rsplit(':')
        .nth(1)
        .and_then(|s| s.parse::<u32>().ok())
        .context("Cached message identity is invalid. Refresh this mailbox.")?;
    ensure!(
        mailbox.uid_validity.unwrap_or(0) == expected,
        "The server reset this mailbox's message identities. Refresh before opening or changing this message."
    );
    Ok(())
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
pub(crate) fn resolve_folder(
    session: &mut ImapSession,
    provider: Provider,
    folder: Folder,
) -> Result<String> {
    if folder == Folder::Inbox {
        return Ok("INBOX".into());
    }
    resolve_in(&list(session)?, provider, folder)
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
