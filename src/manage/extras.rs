//! Smaller account operations: server threads (IMAP THREAD), marking the
//! message a reply or forward answers, read receipts, forwarding a
//! message's attachments or the message itself, server-side filters and
//! vacation replies (ManageSieve), and the server's own notes (METADATA).
use super::*;
use crate::sync::sieve;

/// Threads are worked out for this many days of mail at most.
const THREAD_DAYS: u32 = 90;
/// The server-wide annotations shown in Settings (RFC 5464).
const SERVER_NOTES: [&str; 2] = ["/shared/comment", "/shared/admin"];

/// What the server says about itself (METADATA's server annotations).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerNotes {
    /// A note from the server's administrators.
    pub comment: Option<String>,
    /// How to reach them (often a mailto: or https: address).
    pub admin: Option<String>,
}

impl Backend {
    /// Gives the selected mailbox's recent messages the server's
    /// conversation ids (THREAD=REFERENCES, RFC 5256), on servers that have
    /// no permanent thread ids (Gmail's and OBJECTID's come with headers).
    pub(super) fn thread_mailbox(
        &self,
        c: &mut Connection,
        a: &Account,
        remote: &str,
        validity: u32,
    ) -> Result<()> {
        let ext = c.extensions()?;
        if !ext.thread || ext.gmail || ext.objectid {
            return Ok(());
        }
        let days = match a.sync.days {
            0 => THREAD_DAYS,
            d => d.min(THREAD_DAYS),
        };
        let since = chrono::Utc::now() - chrono::Duration::days(days.into());
        let threads = threads(
            &mut c.session,
            &format!("SINCE {}", since.format("%-d-%b-%Y")),
        )?;
        let mut ids = Vec::new();
        for thread in threads.into_iter().filter(|t| t.len() > 1) {
            let root = thread.iter().copied().min().unwrap_or_default();
            let id = format!("t:{remote}:{validity}:{root}");
            for uid in thread {
                ids.push((format!("{}:{remote}:{validity}:{uid}", a.id), id.clone()));
            }
        }
        self.store.set_thread_ids(&ids)
    }
    /// Marks the message a sent draft replied to or forwarded (\\Answered or
    /// $Forwarded). Offline, the mark waits in the offline queue; a failure
    /// is no reason to say the message wasn't sent.
    pub(super) fn mark_original(&self, a: &Account, d: &Draft) {
        if d.original.is_empty() {
            return;
        }
        let Ok(Some(m)) = self.store.message(&d.original) else {
            return;
        };
        if m.account != a.id {
            return;
        }
        let mark = if d.forwarding {
            Mark::Forwarded
        } else {
            Mark::Answered
        };
        let _ = self.change_many(a, &[m], Change::Mark(mark));
    }
    /// Sends the read receipt `m`'s sender asked for (RFC 8098), then
    /// marks it $MDNSent so no mail app asks again.
    pub fn send_receipt(&self, a: &Account, m: &Mail) -> Result<()> {
        let to = m.receipt_to().trim().to_owned();
        ensure!(!to.is_empty(), "The sender didn't ask for a read receipt.");
        // The address is the sender's to write: checked for every provider,
        // not only for SMTP's envelope (`deliver_raw`).
        // One plain address: list separators, quotes or a second `@` could
        // make more than one recipient of it for some servers.
        ensure!(
            !to.contains(|c: char| {
                c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | ',' | ';' | '"' | '(' | ')')
            }) && to.matches('@').count() == 1
                && !to.starts_with('@')
                && !to.ends_with('@'),
            "The read receipt's address “{to}” isn't valid."
        );
        ensure!(m.account == a.id, "Message belongs to another account");
        // Original-Recipient, where the receiving server noted it (RFC
        // 8098 §3.2.3: copied into the report when present).
        let original_recipient = self
            .original(a, m)
            .ok()
            .and_then(|raw| {
                use mailparse::MailHeaderMap as _;
                mailparse::parse_headers(&raw)
                    .ok()
                    .and_then(|(h, _)| h.get_first_value("Original-Recipient"))
            })
            .unwrap_or_default();
        let raw = crate::sync::smtp::receipt(a, m, &to, &original_recipient);
        if a.provider == Provider::Proton {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            self.send_proton(a, &a.email, &[to], &raw)?;
        } else if a.provider == Provider::Jmap {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            // The server's own read receipts (MDN/send, RFC 9007) where it
            // has them: it writes the report and marks the message.
            let by_server = self.with_jmap(a, |s| {
                let Some(id) = super::jmap::jmap_id(a, m) else {
                    return Ok(false);
                };
                if !s.has(crate::sync::jmap::MDN) {
                    return Ok(false);
                }
                let identity = s.identities()?.into_iter().find(|i| i.covers(&a.email));
                let Some(identity) = identity else {
                    return Ok(false);
                };
                s.send_receipt(
                    id,
                    &identity.id,
                    &format!("Read: {}", m.subject),
                    &format!("Your message “{}” was displayed.", m.subject),
                )?;
                Ok(true)
            })?;
            if !by_server {
                self.send_jmap(a, &a.email, &[to], &raw, &Default::default())?;
            }
        } else {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            let s = self.credentials(a)?;
            let secret = if a.oauth { s.access_token } else { s.password };
            // A report goes with the null reverse-path (RFC 8098 §3), so
            // nothing ever answers it.
            crate::sync::smtp::deliver_raw(
                a,
                "",
                &[to],
                &raw,
                &crate::sync::smtp::Endpoint::of(a),
                &secret,
                &self.tls_connector()?,
            )?;
        }
        self.change_many(
            a,
            std::slice::from_ref(m),
            Change::Mark(Mark::ReceiptHandled),
        )
    }
    /// Declines a read receipt: marks the message $MDNSent without sending.
    pub fn decline_receipt(&self, a: &Account, m: &Mail) -> Result<()> {
        self.change_many(
            a,
            std::slice::from_ref(m),
            Change::Mark(Mark::ReceiptHandled),
        )
    }
    /// The readable source of a message, downloading it first if needed.
    fn readable_source(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        ensure!(
            !(m.partial || m.size as usize > large::MAX_DOWNLOAD),
            "This message is too large to forward whole. Forward it inline, with its attachments."
        );
        let raw = self.original(a, m)?;
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        Ok(self.plaintext(a, &raw)?.1)
    }
    /// Saves `m`'s attachments as files for draft `draft`, so a forward
    /// carries them. Downloads the message (or, when it is too large to
    /// keep, each attachment) as needed.
    pub fn forward_files(
        &self,
        a: &Account,
        m: &Mail,
        draft: &str,
    ) -> Result<Vec<std::path::PathBuf>> {
        let dir = self.draft_files(draft);
        let mut used = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut save = |name: &str, bytes: &[u8], i: usize| -> Result<()> {
            std::fs::create_dir_all(&dir)?;
            let name: String = std::path::Path::new(name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty() && n != "." && n != "..")
                .unwrap_or_else(|| format!("attachment-{}", i + 1));
            let path = dir.join(distinct_name(&name, &mut used));
            std::fs::write(&path, bytes)?;
            out.push(path);
            Ok(())
        };
        if m.partial || m.size as usize > large::MAX_DOWNLOAD {
            let current = self.read(a, m)?;
            for (i, attachment) in current.attachments().iter().enumerate() {
                let bytes = self.attachment(a, &current, attachment.part)?;
                save(&attachment.name, &bytes, i)?;
            }
            return Ok(out);
        }
        let plain = self.readable_source(a, m)?;
        let parsed = mailparse::parse_mail(&plain)?;
        let listed = parse_message(&plain, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        let mut leaves = Vec::new();
        collect_leaves(&parsed, &mut leaves);
        for (i, attachment) in listed.attachments().iter().enumerate() {
            let Some(leaf) = leaves.get(attachment.part) else {
                continue;
            };
            save(&attachment.name, &leaf.get_body_raw()?, i)?;
        }
        Ok(out)
    }
    /// Saves `m` itself as a file (message/rfc822, `.eml`) for draft
    /// `draft`, to forward it as an attachment.
    pub fn forward_as_attachment(
        &self,
        a: &Account,
        m: &Mail,
        draft: &str,
    ) -> Result<std::path::PathBuf> {
        let plain = self.readable_source(a, m)?;
        let dir = self.draft_files(draft);
        std::fs::create_dir_all(&dir)?;
        let stem: String = m
            .subject
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || " -_.,()".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .take(60)
            .collect::<String>()
            .trim()
            .to_owned();
        let stem = if stem.is_empty() {
            "Forwarded message".to_owned()
        } else {
            stem
        };
        let path = dir.join(format!("{stem}.eml"));
        std::fs::write(&path, plain)?;
        Ok(path)
    }
    /// Where the account's filter server is: ManageSieve on the incoming
    /// server's host.
    fn sieve_endpoint(a: &Account) -> sieve::Endpoint {
        // Where the domain says its filter server is (RFC 5804 §1.8: SRV
        // `_sieve._tcp`), when that's in the domain itself, as for JMAP:
        // DNS isn't authenticated. Else the incoming server, on 4190.
        let (host, port) = crate::sync::discover::domain(&a.email)
            .and_then(|domain| {
                crate::sync::auth::srv(&format!("_sieve._tcp.{domain}"))
                    .ok()?
                    .into_iter()
                    .find(|(host, _)| host == &domain || host.ends_with(&format!(".{domain}")))
            })
            .unwrap_or_else(|| (a.incoming_host.clone(), sieve::PORT));
        sieve::Endpoint {
            host,
            port,
            user: a.login().to_owned(),
            oauth: a.oauth,
            kerberos: a.kerberos,
        }
    }
    /// Whether the account's provider might have a filter server. Gmail,
    /// Outlook and POP3 accounts don't offer ManageSieve; a JMAP server may
    /// offer JMAP for Sieve Scripts.
    pub fn may_filter(a: &Account) -> bool {
        a.provider == Provider::Jmap
            || a.provider == Provider::Imap
                && !matches!(
                    a.service,
                    Service::ICloud | Service::ProtonBridge | Service::Zoho
                )
    }
    /// The account's server-side filters and vacation reply.
    pub fn filters(&self, a: &Account) -> Result<sieve::State> {
        ensure!(
            Self::may_filter(a),
            "This mail service doesn't offer server filters (ManageSieve)."
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        if a.provider == Provider::Jmap {
            return self.filters_jmap(a);
        }
        let s = self.credentials(a)?;
        let secret = if a.oauth { s.access_token } else { s.password };
        sieve::load(&Self::sieve_endpoint(a), &secret, &self.tls_connector()?)
    }
    /// Saves the account's server-side filters and vacation reply. The
    /// vacation reply answers mail to the account's own addresses.
    pub fn save_filters(&self, a: &Account, filters: &sieve::Filters) -> Result<sieve::State> {
        ensure!(
            Self::may_filter(a),
            "This mail service doesn't offer server filters (ManageSieve)."
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mut addresses = vec![a.email.clone()];
        addresses.extend(a.identities.iter().map(|i| i.email.trim().to_owned()));
        if a.provider == Provider::Jmap {
            return self.save_filters_jmap(a, filters, &addresses);
        }
        let s = self.credentials(a)?;
        let secret = if a.oauth { s.access_token } else { s.password };
        sieve::save(
            &Self::sieve_endpoint(a),
            &secret,
            &self.tls_connector()?,
            filters,
            &addresses,
        )
    }
    /// The server's notes for its users (METADATA), where it has any.
    pub fn server_notes(&self, a: &Account) -> Result<ServerNotes> {
        if !a.provider.imap() {
            return Ok(ServerNotes::default());
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            if !c.extensions()?.metadata_server {
                return Ok(ServerNotes::default());
            }
            let mut notes = ServerNotes::default();
            for (entry, value) in get_metadata(&mut c.session, "", &SERVER_NOTES)? {
                let value = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
                match entry.as_str() {
                    "/shared/comment" => notes.comment = value,
                    "/shared/admin" => notes.admin = value,
                    _ => {}
                }
            }
            Ok(notes)
        })
    }
}
