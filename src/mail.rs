use crate::{
    model::*,
    store::{self, Store},
};
use anyhow::{Context, Result, bail, ensure};
use mailparse::MailHeaderMap;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zeroize::Zeroize;

pub const MAX_MESSAGE: usize = 25 * 1024 * 1024;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
type ImapSession = imap::Session<native_tls::TlsStream<TcpStream>>;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Read(bool),
    Star(bool),
    Move(Folder),
}
pub struct Backend {
    pub store: Arc<Store>,
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Opened Pinal mail keys by account, held in memory only.
    mail_keys: Mutex<HashMap<String, Arc<Vec<pgp::composed::SignedSecretKey>>>>,
    /// Decrypted Pinal messages by id. Memory only: the cache on disk keeps
    /// them encrypted, so reloads from it get their bodies back from here.
    readable: Mutex<HashMap<String, Mail>>,
    #[cfg(test)]
    test_certificate: Option<Vec<u8>>,
    #[cfg(test)]
    test_secret: Option<Secret>,
}
impl Backend {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            operations: Mutex::new(HashMap::new()),
            mail_keys: Mutex::new(HashMap::new()),
            readable: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_certificate: None,
            #[cfg(test)]
            test_secret: None,
        }
    }
    fn account_lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.operations
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    fn tls_connector(&self) -> Result<native_tls::TlsConnector> {
        #[cfg(test)]
        if let Some(cert) = &self.test_certificate {
            return Ok(native_tls::TlsConnector::builder()
                .add_root_certificate(native_tls::Certificate::from_pem(cert)?)
                .build()?);
        }
        Ok(native_tls::TlsConnector::new()?)
    }
    fn imap_login(&self, a: &Account, s: &Secret) -> Result<ImapSession> {
        imap_login(a, s, &self.tls_connector()?)
    }
    fn pop_login(
        &self,
        a: &Account,
        password: &str,
    ) -> Result<PopSession<native_tls::TlsStream<TcpStream>>> {
        pop_login(a, password, &self.tls_connector()?)
    }
    /// Signs in and checks the incoming server, then saves the account. For
    /// browser and Pinal sign-in, the returned account names the address
    /// that was chosen; `taken` lists addresses already added.
    pub fn connect(
        &self,
        account: &Account,
        password: String,
        taken: &[String],
        cancel: Arc<AtomicBool>,
    ) -> Result<Account> {
        let mut account = account.clone();
        let mut secret = if account.service == crate::model::Service::Pinal {
            let mailbox = crate::oauth::pinal(cancel.clone())?;
            account = Account {
                id: account.id.clone(),
                ..mailbox.account()
            };
            Secret {
                password: mailbox.password,
                mail_bundle: mailbox
                    .encryption
                    .and_then(|e| e.bundle)
                    .map(|b| serde_json::to_string(&b))
                    .transpose()?
                    .unwrap_or_default(),
                ..Default::default()
            }
        } else if account.oauth {
            let (secret, who) = crate::oauth::authorize(&account, cancel.clone())?;
            if account.email.is_empty() {
                account.email = who.email;
            }
            if account.name.is_empty() || account.name == account.email {
                account.name = if who.name.is_empty() {
                    account.email.clone()
                } else {
                    who.name
                };
            }
            secret
        } else {
            ensure!(!password.is_empty(), "Enter your password or app password.");
            Secret {
                password,
                ..Default::default()
            }
        };
        account.validate()?;
        ensure!(
            !taken.iter().any(|e| e.eq_ignore_ascii_case(&account.email)),
            "{} is already added.",
            account.email
        );
        let lock = self.account_lock(&account.id);
        let _operation = lock.lock().unwrap();
        secret.client_secret = account.client_secret.clone();
        if account.provider == Provider::Pop3 {
            let mut pop = self.pop_login(&account, &secret.password)?;
            pop.command("QUIT")?;
        } else {
            let mut session = self.imap_login(&account, &secret)?;
            session.logout()?;
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
        store::write_secret(&account.id, &secret)?;
        self.store.save_account(&account)?;
        Ok(account)
    }
    fn credentials(&self, a: &Account) -> Result<Secret> {
        #[cfg(test)]
        if let Some(secret) = &self.test_secret {
            return Ok(secret.clone());
        }
        let mut s = store::read_secret(&a.id)?;
        crate::oauth::refresh(a, &mut s)?;
        Ok(s)
    }
    /// Whether this account's mail is encrypted with a Pinal key bundle.
    pub fn encrypted(&self, a: &Account) -> bool {
        self.credentials(a)
            .map(|s| !s.mail_bundle.is_empty())
            .unwrap_or(false)
    }
    /// Opens the key bundle with the Pinal password (or the recovery key) and
    /// keeps the mail key in the keychain, so later launches need no password.
    pub fn unlock(&self, a: &Account, secret: &str) -> Result<()> {
        let mut s = self.credentials(a)?;
        ensure!(
            !s.mail_bundle.is_empty(),
            "This account's mail is not encrypted."
        );
        let bundle: crate::crypto::KeyBundle = serde_json::from_str(&s.mail_bundle)?;
        let secret = secret.trim_end_matches(['\r', '\n']);
        let mk = if crate::crypto::looks_like_recovery_key(secret) {
            crate::crypto::unlock_with_recovery_key(&bundle, secret)?
        } else {
            crate::crypto::unlock_with_password(&bundle, secret)?
        };
        let keys = crate::crypto::open_keys(&bundle, &mk)?;
        s.mail_key = crate::crypto::encode_master_key(&mk);
        #[cfg(not(test))]
        store::write_secret(&a.id, &s)?;
        self.mail_keys
            .lock()
            .unwrap()
            .insert(a.id.clone(), Arc::new(keys));
        Ok(())
    }
    /// Forgets the mail key on this device; the password unlocks it again.
    pub fn lock_mail(&self, a: &Account) -> Result<()> {
        self.mail_keys.lock().unwrap().remove(&a.id);
        self.readable
            .lock()
            .unwrap()
            .retain(|_, m| m.account != a.id);
        let mut s = store::read_secret(&a.id)?;
        s.mail_key.zeroize();
        s.mail_key.clear();
        store::write_secret(&a.id, &s)
    }
    /// Puts decrypted bodies back on messages reloaded from the disk cache.
    pub fn with_readable(&self, mut messages: Vec<Mail>) -> Vec<Mail> {
        let readable = self.readable.lock().unwrap();
        for m in messages.iter_mut().filter(|m| m.body.is_none()) {
            if let Some(r) = readable.get(&m.id) {
                m.body = r.body.clone();
                m.snippet = r.snippet.clone();
                m.attachments = r.attachments.clone();
                m.has_attachments = r.has_attachments;
            }
        }
        messages
    }
    fn keys(&self, a: &Account) -> Result<Option<Arc<Vec<pgp::composed::SignedSecretKey>>>> {
        if let Some(keys) = self.mail_keys.lock().unwrap().get(&a.id) {
            return Ok(Some(keys.clone()));
        }
        let s = self.credentials(a)?;
        let Some(mk) = crate::crypto::decode_master_key(&s.mail_key) else {
            return Ok(None);
        };
        let bundle: crate::crypto::KeyBundle = serde_json::from_str(&s.mail_bundle)?;
        let keys = Arc::new(crate::crypto::open_keys(&bundle, &mk)?);
        self.mail_keys
            .lock()
            .unwrap()
            .insert(a.id.clone(), keys.clone());
        Ok(Some(keys))
    }
    /// The readable message: decrypted here when Pinal stored it encrypted.
    fn plaintext(&self, a: &Account, raw: &[u8]) -> Result<(crate::crypto::Protection, Vec<u8>)> {
        if crate::crypto::encrypted_part(raw).is_none() {
            return crate::crypto::open_message(raw, &[]);
        }
        let Some(keys) = self.keys(a)? else {
            // Accounts added before encryption existed have no key bundle.
            ensure!(
                a.service != Service::Pinal || !self.credentials(a)?.mail_bundle.is_empty(),
                "This message is encrypted. Remove this Pinal account and add it again so \
                 Picacho receives your encryption keys."
            );
            return Err(crate::crypto::Locked.into());
        };
        crate::crypto::open_message(raw, &keys)
    }
    pub fn sync(
        &self,
        a: &Account,
        mailbox: impl Into<Mailbox>,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        let mailbox = mailbox.into();
        let folder = mailbox.folder();
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let secret = self.credentials(a)?;
        if a.provider == Provider::Pop3 {
            return self.sync_pop(a, folder, limit, &secret.password);
        }
        if folder == Folder::Drafts {
            return Ok(vec![]);
        }
        let mut session = self.imap_login(a, &secret)?;
        let names = list(&mut session)?;
        let _ = self.store.save_folders(&a.id, &custom_folders(&names));
        let mut result = Vec::new();
        if folder == Folder::Starred {
            let folders = if a.provider == Provider::Gmail {
                vec![Folder::Archive]
            } else {
                vec![
                    Folder::Inbox,
                    Folder::Sent,
                    Folder::Archive,
                    Folder::Junk,
                    Folder::Trash,
                ]
            };
            let mut visited = std::collections::HashSet::new();
            for source in folders {
                let remote = match resolve_in(&names, a.provider, source) {
                    Ok(remote) => remote,
                    Err(_) if source != Folder::Inbox => continue,
                    Err(e) => return Err(e),
                };
                if visited.insert(remote.clone()) {
                    result.extend(self.fetch_mailbox(
                        &mut session,
                        a,
                        &source.into(),
                        &remote,
                        true,
                        limit,
                    )?);
                }
            }
            result.sort_by_key(|m| std::cmp::Reverse(m.date));
            let mut seen = std::collections::HashSet::new();
            result.retain(|m| {
                seen.insert(if m.message_id.is_empty() {
                    m.id.clone()
                } else {
                    m.message_id.clone()
                })
            });
            result.truncate(limit);
        } else {
            let remote = match &mailbox {
                Mailbox::Remote(name) => {
                    ensure!(
                        names.iter().any(|n| &n.name == name),
                        "The folder “{name}” no longer exists on the server."
                    );
                    name.clone()
                }
                Mailbox::Standard(folder) => resolve_in(&names, a.provider, *folder)?,
            };
            result = self.fetch_mailbox(&mut session, a, &mailbox, &remote, false, limit)?;
        }
        session.logout()?;
        Ok(result)
    }
    fn fetch_mailbox(
        &self,
        session: &mut ImapSession,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        starred_only: bool,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        let folder = mailbox.folder();
        let selected = session.select(remote)?;
        let validity = selected.uid_validity.unwrap_or(0);
        let mut query = if starred_only { "FLAGGED" } else { "ALL" }.to_owned();
        if a.sync.days > 0 {
            // Older mail stays on the server; reconciling drops it from the cache.
            let since = chrono::Utc::now() - chrono::Duration::days(a.sync.days.into());
            query = format!("{query} SINCE {}", since.format("%-d-%b-%Y"));
        }
        let mut uids: Vec<_> = session.uid_search(&query)?.into_iter().collect();
        uids.sort_unstable();
        let all_ids: Vec<_> = uids
            .iter()
            .map(|uid| format!("{}:{remote}:{validity}:{uid}", a.id))
            .collect();
        let start = uids.len().saturating_sub(limit);
        let uids = &uids[start..];
        let cached: HashMap<_, _> = self
            .store
            .messages(&a.id, mailbox, 5000)?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let mut result = Vec::new();
        // Batch headers; message bodies and attachments are fetched only when opened.
        for chunk in uids.chunks(100) {
            let set = chunk
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            for f in session
                .uid_fetch(
                    set,
                    "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])",
                )?
                .iter()
            {
                let Some(uid) = f.uid else {
                    continue;
                };
                let id = format!("{}:{remote}:{validity}:{uid}", a.id);
                let mut m = if let Some(old) = cached.get(&id) {
                    old.clone()
                } else {
                    parse_message(f.header().unwrap_or_default(), a, &id, folder, remote, uid)?
                };
                m.unread = !f
                    .flags()
                    .iter()
                    .any(|v| matches!(v, imap::types::Flag::Seen));
                m.starred = f
                    .flags()
                    .iter()
                    .any(|v| matches!(v, imap::types::Flag::Flagged));
                m.size = f.size.unwrap_or(0);
                if m.date == 0 {
                    m.date = f.internal_date().map(|d| d.timestamp()).unwrap_or_default();
                }
                result.push(m);
            }
        }
        self.store.save_messages(&result)?;
        if starred_only {
            let keep: std::collections::HashSet<_> = all_ids.iter().collect();
            let cleared: Vec<_> = cached
                .into_values()
                .filter(|m| m.starred && m.remote_folder == remote && !keep.contains(&m.id))
                .map(|mut m| {
                    m.starred = false;
                    m
                })
                .collect();
            self.store.save_messages(&cleared)?;
        } else {
            self.store.reconcile(&a.id, mailbox, &all_ids)?;
        }
        result.sort_by_key(|m| std::cmp::Reverse(m.date));
        Ok(result)
    }
    fn sync_pop(
        &self,
        a: &Account,
        folder: Folder,
        limit: usize,
        password: &str,
    ) -> Result<Vec<Mail>> {
        if folder != Folder::Inbox && folder != Folder::Starred {
            return self.store.messages(&a.id, folder, limit);
        }
        let mut pop = self.pop_login(a, password)?;
        let ids = pop.uidl()?;
        let mut messages = Vec::new();
        for (number, uid) in ids.iter().rev().take(limit) {
            let id = format!("{}:pop:{uid}", a.id);
            let mail = if let Some(m) = self.store.message(&id)? {
                m
            } else {
                pop.command(&format!("RETR {number}"))?;
                let raw = pop.multiline(MAX_MESSAGE)?;
                let mut m = parse_message(&raw, a, &id, Folder::Inbox, "INBOX", *number)?;
                m.size = raw.len() as u32;
                self.store.save_messages(&[m.clone()])?;
                self.store.save_raw(&id, &raw)?;
                m
            };
            messages.push(mail);
        }
        pop.command("QUIT")?;
        self.store.save_messages(&messages)?;
        // POP3 has no folders. Keep local moves and messages removed from the
        // remote server; UIDL identities prevent duplicate downloads.
        messages = self.store.messages(&a.id, folder, limit)?;
        messages.sort_by_key(|m| std::cmp::Reverse(m.date));
        if folder == Folder::Starred {
            messages.retain(|m| m.starred);
        }
        Ok(messages)
    }
    pub fn read(&self, a: &Account, m: &Mail) -> Result<Mail> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let latest = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
        let m = &latest;
        let raw = if let Some(raw) = self.store.raw(&m.id)? {
            raw
        } else {
            ensure!(
                m.size as usize <= MAX_MESSAGE,
                "This message exceeds the 25 MB download limit."
            );
            ensure!(
                a.provider != Provider::Pop3,
                "Message is not cached. Refresh the mailbox."
            );
            let secret = self.credentials(a)?;
            let mut session = self.imap_login(a, &secret)?;
            select_message_mailbox(&mut session, m)?;
            let fetched = session.uid_fetch(m.uid.to_string(), "BODY.PEEK[]")?;
            let raw = fetched
                .iter()
                .next()
                .and_then(|f| f.body())
                .context("Message no longer exists on the server")?
                .to_vec();
            ensure!(raw.len() <= MAX_MESSAGE, "Message exceeds the 25 MB limit.");
            session.logout()?;
            self.store.save_raw(&m.id, &raw)?;
            raw
        };
        let (protection, plain) = self.plaintext(a, &raw)?;
        let mut next = parse_message(&plain, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        next.unread = m.unread;
        next.starred = m.starred;
        next.size = m.size;
        if protection == crate::crypto::Protection::Pinal {
            // Zero-access: keep only what the server could already see in the
            // local cache. The body stays in memory and is decrypted again later.
            let mut kept = next.clone();
            kept.body = None;
            kept.snippet.clear();
            kept.attachments.clear();
            self.store.save_messages(&[kept])?;
            self.readable
                .lock()
                .unwrap()
                .insert(next.id.clone(), next.clone());
        } else {
            self.store.save_messages(&[next.clone()])?;
        }
        Ok(next)
    }
    pub fn change(&self, a: &Account, m: &Mail, change: Change) -> Result<Mail> {
        self.change_many(a, std::slice::from_ref(m), change)?;
        let mut next = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
        if let Change::Move(folder) = change {
            next.folder = folder;
        }
        Ok(next)
    }
    /// Applies one change to several messages, with one IMAP command per mailbox.
    pub fn change_many(&self, a: &Account, mails: &[Mail], change: Change) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mut current = Vec::new();
        for m in mails {
            let next = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
            if !matches!(change, Change::Move(folder) if next.folder == folder) {
                current.push(next);
            }
        }
        if current.is_empty() {
            return Ok(());
        }
        if a.provider != Provider::Pop3 {
            let secret = self.credentials(a)?;
            let mut session = self.imap_login(a, &secret)?;
            let target = match change {
                Change::Move(folder) => {
                    // UID MOVE is atomic and does not expunge unrelated deleted messages.
                    let caps = session.capabilities()?;
                    ensure!(
                        caps.has_str("MOVE"),
                        "This server does not support safe message moves (IMAP MOVE)."
                    );
                    Some(resolve_folder(&mut session, a.provider, folder)?)
                }
                _ => None,
            };
            let mut groups: Vec<(&Mail, Vec<u32>)> = Vec::new();
            for m in &current {
                match groups.iter_mut().find(|(first, _)| {
                    first.remote_folder == m.remote_folder && same_validity(first, m)
                }) {
                    Some((_, uids)) => uids.push(m.uid),
                    None => groups.push((m, vec![m.uid])),
                }
            }
            for (first, uids) in groups {
                select_message_mailbox(&mut session, first)?;
                let set = uid_set(&uids);
                match change {
                    Change::Read(read) => {
                        session.uid_store(
                            &set,
                            if read {
                                "+FLAGS.SILENT (\\Seen)"
                            } else {
                                "-FLAGS.SILENT (\\Seen)"
                            },
                        )?;
                    }
                    Change::Star(star) => {
                        session.uid_store(
                            &set,
                            if star {
                                "+FLAGS.SILENT (\\Flagged)"
                            } else {
                                "-FLAGS.SILENT (\\Flagged)"
                            },
                        )?;
                    }
                    Change::Move(_) => {
                        let escaped = target
                            .as_deref()
                            .unwrap_or_default()
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"");
                        session
                            .run_command_and_check_ok(format!("UID MOVE {set} \"{escaped}\""))?;
                    }
                }
            }
            session.logout()?;
        }
        let mut updated = Vec::new();
        for mut next in current {
            match change {
                Change::Read(read) => next.unread = !read,
                Change::Star(star) => next.starred = star,
                Change::Move(folder) => next.folder = folder,
            }
            if matches!(change, Change::Move(_)) && a.provider != Provider::Pop3 {
                self.store.delete_mail(&next.id)?;
            } else {
                updated.push(next);
            }
        }
        self.store.save_messages(&updated)?;
        Ok(())
    }
    /// Marks every message in a mailbox read, on the server and in the cache.
    pub fn mark_all_read(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        if a.provider != Provider::Pop3 && mailbox.folder() != Folder::Drafts {
            let secret = self.credentials(a)?;
            let mut session = self.imap_login(a, &secret)?;
            let remote = match mailbox {
                Mailbox::Remote(name) => name.clone(),
                Mailbox::Standard(folder) => resolve_folder(&mut session, a.provider, *folder)?,
            };
            session.select(&remote)?;
            let unseen: Vec<u32> = session.uid_search("UNSEEN")?.into_iter().collect();
            if !unseen.is_empty() {
                session.uid_store(uid_set(&unseen), "+FLAGS.SILENT (\\Seen)")?;
            }
            session.logout()?;
        }
        let mut cached = self.store.messages(&a.id, mailbox, usize::MAX >> 1)?;
        cached.retain(|m| m.unread);
        for m in &mut cached {
            m.unread = false;
        }
        self.store.save_messages(&cached)
    }
    /// Downloads the full content of up to `max` messages that are only known
    /// by their headers. Returns how many were stored.
    pub fn download_batch(&self, a: &Account, max: usize) -> Result<usize> {
        if a.provider == Provider::Pop3 {
            return Ok(0);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mut budget = 12 * 1024 * 1024usize;
        let mut pending = Vec::new();
        for m in self.store.missing(&a.id, MAX_MESSAGE, max)? {
            if !pending.is_empty() && m.size as usize > budget {
                break;
            }
            budget = budget.saturating_sub(m.size as usize);
            pending.push(m);
        }
        if pending.is_empty() {
            return Ok(0);
        }
        let secret = self.credentials(a)?;
        let mut session = self.imap_login(a, &secret)?;
        let mut groups: Vec<Vec<&Mail>> = Vec::new();
        for m in &pending {
            match groups
                .iter_mut()
                .find(|g| g[0].remote_folder == m.remote_folder && same_validity(g[0], m))
            {
                Some(group) => group.push(m),
                None => groups.push(vec![m]),
            }
        }
        let mut stored = 0;
        for group in groups {
            if select_message_mailbox(&mut session, group[0]).is_err() {
                continue;
            }
            let uids: Vec<u32> = group.iter().map(|m| m.uid).collect();
            for chunk in uids.chunks(10) {
                for f in session
                    .uid_fetch(uid_set(chunk), "(UID BODY.PEEK[])")?
                    .iter()
                {
                    let (Some(uid), Some(raw)) = (f.uid, f.body()) else {
                        continue;
                    };
                    let Some(m) = group.iter().find(|m| m.uid == uid) else {
                        continue;
                    };
                    if raw.len() > MAX_MESSAGE {
                        continue;
                    }
                    self.store.save_raw(&m.id, raw)?;
                    // Encrypted mail stays ciphertext on disk; it is read on open.
                    if crate::crypto::encrypted_part(raw).is_some() {
                        stored += 1;
                        continue;
                    }
                    // Keep summaries small: list views load every row's JSON.
                    if let Ok(parsed) =
                        parse_message(raw, a, &m.id, m.folder, &m.remote_folder, m.uid)
                    {
                        let mut next = (*m).clone();
                        next.has_attachments = !parsed.attachments.is_empty();
                        next.attachments = parsed.attachments;
                        if next.snippet.is_empty() {
                            next.snippet = parsed.snippet;
                        }
                        self.store.save_messages(&[next])?;
                    }
                    stored += 1;
                }
            }
        }
        session.logout()?;
        Ok(stored)
    }
    pub fn send(&self, a: &Account, d: &Draft) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        use lettre::{
            Message, Transport,
            message::{Attachment, Mailbox, MultiPart, SinglePart},
            transport::smtp::{
                authentication::{Credentials, Mechanism},
                client::{Tls, TlsParameters},
            },
        };
        let mut builder = Message::builder()
            .from(Mailbox::new(Some(a.name.clone()), a.email.parse()?))
            .subject(&d.subject);
        let recipients = mailparse::addrparse(&d.to)
            .context("Enter valid recipient email addresses, separated by commas")?;
        let mut count = 0;
        for address in recipients.iter() {
            match address {
                mailparse::MailAddr::Single(s) => {
                    builder = builder.to(Mailbox::new(s.display_name.clone(), s.addr.parse()?));
                    count += 1;
                }
                mailparse::MailAddr::Group(g) => {
                    for s in &g.addrs {
                        builder = builder.to(Mailbox::new(s.display_name.clone(), s.addr.parse()?));
                        count += 1;
                    }
                }
            }
        }
        ensure!(count > 0, "Add at least one recipient.");
        if !d.reply_id.is_empty() {
            builder = builder.in_reply_to(d.reply_id.clone());
        }
        let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(d.body.clone()));
        let mut total = d.body.len();
        for path in &d.attachments {
            let meta = std::fs::metadata(path)?;
            total += meta.len() as usize;
            ensure!(
                total < 18 * 1024 * 1024,
                "Attachments exceed the 18 MB sending limit."
            );
            let bytes = std::fs::read(path)?;
            multipart = multipart.singlepart(
                Attachment::new(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
                .body(bytes, "application/octet-stream".parse()?),
            );
        }
        let message = builder.multipart(multipart)?;
        let secret = self.credentials(a)?;
        let tls = TlsParameters::new(a.smtp_host.clone())?;
        #[cfg(test)]
        let tls = if let Some(cert) = &self.test_certificate {
            TlsParameters::builder(a.smtp_host.clone())
                .add_root_certificate(lettre::transport::smtp::client::Certificate::from_pem(
                    cert,
                )?)
                .build()?
        } else {
            tls
        };
        let transport = lettre::SmtpTransport::builder_dangerous(&a.smtp_host)
            .port(a.smtp_port)
            .tls(if a.smtp_port == 465 {
                Tls::Wrapper(tls)
            } else {
                Tls::Required(tls)
            })
            .timeout(Some(Duration::from_secs(45)))
            .credentials(Credentials::new(
                a.email.clone(),
                if a.oauth {
                    secret.access_token.clone()
                } else {
                    secret.password.clone()
                },
            ))
            .authentication(if a.oauth {
                vec![Mechanism::Xoauth2]
            } else {
                vec![Mechanism::Plain, Mechanism::Login]
            })
            .build();
        transport.send(&message).context("SMTP delivery failed. Your draft has been kept; check the server and account settings.")?;
        // Gmail and Outlook save sent messages themselves. Custom IMAP servers do not.
        if a.provider == Provider::Imap
            && let Ok(mut session) = self.imap_login(a, &secret)
        {
            if let Ok(sent) = resolve_folder(&mut session, a.provider, Folder::Sent) {
                let _ = session.append_with_flags(
                    sent,
                    message.formatted(),
                    &[imap::types::Flag::Seen],
                );
            }
            let _ = session.logout();
        }
        // SMTP acceptance is the delivery boundary. Do not report a send failure
        // after this point, which could encourage a duplicate transmission.
        if a.provider == Provider::Pop3 {
            let raw = message.formatted();
            let id = format!("{}:sent:{}", a.id, d.id);
            if let Ok(mut sent) = parse_message(&raw, a, &id, Folder::Sent, "Sent", 0) {
                sent.unread = false;
                let _ = self.store.save_messages(&[sent]);
                let _ = self.store.save_raw(&id, &raw);
            }
        }
        let _ = self.store.delete_draft(&d.id);
        Ok(())
    }
    pub fn attachment(&self, a: &Account, m: &Mail, part: usize) -> Result<Vec<u8>> {
        let raw = self.store.raw(&m.id)?.context("Open this message first")?;
        let (_, raw) = self.plaintext(a, &raw)?;
        let parsed = mailparse::parse_mail(&raw)?;
        let mut leaves = Vec::new();
        collect_leaves(&parsed, &mut leaves);
        leaves
            .get(part)
            .context("Attachment not found")?
            .get_body_raw()
            .map_err(Into::into)
    }
}
fn tcp(host: &str, port: u16) -> Result<TcpStream> {
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
fn imap_login(a: &Account, s: &Secret, tls: &native_tls::TlsConnector) -> Result<ImapSession> {
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
fn select_message_mailbox(session: &mut ImapSession, mail: &Mail) -> Result<()> {
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
fn same_validity(a: &Mail, b: &Mail) -> bool {
    a.id.rsplit(':').nth(1) == b.id.rsplit(':').nth(1)
}
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
struct Name {
    name: String,
    attributes: Vec<String>,
    delimiter: Option<String>,
}
impl Name {
    fn has(&self, attribute: &str) -> bool {
        self.attributes.iter().any(|a| a.contains(attribute))
    }
}
fn list(session: &mut ImapSession) -> Result<Vec<Name>> {
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
fn custom_folders(names: &[Name]) -> Vec<RemoteFolder> {
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
fn resolve_folder(session: &mut ImapSession, provider: Provider, folder: Folder) -> Result<String> {
    if folder == Folder::Inbox {
        return Ok("INBOX".into());
    }
    resolve_in(&list(session)?, provider, folder)
}
fn resolve_in(names: &[Name], provider: Provider, folder: Folder) -> Result<String> {
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

pub fn parse_message(
    raw: &[u8],
    account: &Account,
    id: &str,
    folder: Folder,
    remote: &str,
    uid: u32,
) -> Result<Mail> {
    let p = mailparse::parse_mail(raw)?;
    let header = |name: &str| p.headers.get_first_value(name).unwrap_or_default();
    let mixed = p.ctype.mimetype.eq_ignore_ascii_case("multipart/mixed");
    let from_header = header("From");
    let (sender, from) = mailparse::addrparse(&from_header)
        .ok()
        .and_then(|v| v.extract_single_info())
        .map(|s| (s.display_name.unwrap_or_else(|| s.addr.clone()), s.addr))
        .unwrap_or_else(|| (from_header.clone(), from_header));
    let mut body = String::new();
    let mut attachments = Vec::new();
    let mut leaves = Vec::new();
    collect_leaves(&p, &mut leaves);
    for (index, part) in leaves.iter().enumerate() {
        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"));
        if filename.is_some() || disposition.disposition == mailparse::DispositionType::Attachment {
            attachments.push(Attachment {
                name: filename.cloned().unwrap_or_else(|| "attachment".into()),
                size: part.get_body_raw().map(|b| b.len()).unwrap_or_default(),
                mime: part.ctype.mimetype.clone(),
                part: index,
            });
        } else if part.ctype.mimetype == "text/plain" && body.is_empty() {
            body = part.get_body().unwrap_or_default();
        }
    }
    if body.is_empty()
        && let Some(html) = leaves.iter().find(|p| p.ctype.mimetype == "text/html")
    {
        body = html_to_text(&html.get_body().unwrap_or_default());
    }
    let snippet = body
        .split_whitespace()
        .take(45)
        .collect::<Vec<_>>()
        .join(" ");
    let date = mailparse::dateparse(&header("Date")).unwrap_or_default();
    let subject = header("Subject");
    let has_attachments = mixed || !attachments.is_empty();
    Ok(Mail {
        id: id.into(),
        account: account.id.clone(),
        folder,
        remote_folder: remote.into(),
        uid,
        from,
        sender,
        to: header("To"),
        subject: if subject.is_empty() {
            "(No subject)".into()
        } else {
            subject
        },
        snippet,
        date,
        unread: true,
        starred: false,
        size: raw.len() as u32,
        body: if body.is_empty() { None } else { Some(body) },
        attachments,
        message_id: header("Message-ID"),
        has_attachments,
    })
}
fn collect_leaves<'a>(
    p: &'a mailparse::ParsedMail<'a>,
    out: &mut Vec<&'a mailparse::ParsedMail<'a>>,
) {
    if p.subparts.is_empty() {
        out.push(p);
    } else {
        for part in &p.subparts {
            collect_leaves(part, out);
        }
    }
}
fn html_to_text(html: &str) -> String {
    // Email is displayed as native text. No remote images, scripts, or web engine.
    let mut text = String::new();
    let mut tag = String::new();
    let mut inside = false;
    let mut hidden = 0;
    for ch in html.chars() {
        if ch == '<' {
            inside = true;
            tag.clear();
        } else if ch == '>' && inside {
            inside = false;
            let t = tag.to_lowercase();
            let t = t.trim();
            if t.starts_with("script") || t.starts_with("style") {
                hidden += 1;
            } else if t.starts_with("/script") || t.starts_with("/style") {
                hidden = 0;
            } else if hidden == 0
                && ["br", "/p", "/div", "/tr", "/h1", "/h2", "/li"]
                    .iter()
                    .any(|s| t.starts_with(s))
            {
                text.push('\n');
            }
        } else if inside {
            tag.push(ch);
        } else if hidden == 0 {
            text.push(ch);
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

pub struct PopSession<T: Read + Write> {
    reader: BufReader<T>,
}
impl<T: Read + Write> PopSession<T> {
    fn new(stream: T) -> Result<Self> {
        let mut s = Self {
            reader: BufReader::new(stream),
        };
        s.status()?;
        Ok(s)
    }
    fn status(&mut self) -> Result<String> {
        let mut line = String::new();
        self.reader.by_ref().take(8192).read_line(&mut line)?;
        ensure!(line.starts_with("+OK"), "POP3 server rejected the request.");
        Ok(line)
    }
    fn command(&mut self, command: &str) -> Result<String> {
        ensure!(!command.contains(['\r', '\n']), "Invalid POP3 command");
        write!(self.reader.get_mut(), "{command}\r\n")?;
        self.reader.get_mut().flush()?;
        self.status()
    }
    fn multiline(&mut self, max: usize) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        loop {
            let mut line = Vec::new();
            let n = self
                .reader
                .by_ref()
                .take((max + 1) as u64)
                .read_until(b'\n', &mut line)?;
            ensure!(n > 0, "POP3 connection closed unexpectedly");
            if line == b".\r\n" || line == b".\n" {
                break;
            }
            if line.starts_with(b"..") {
                line.remove(0);
            }
            ensure!(
                result.len() + line.len() <= max,
                "POP3 response exceeds the download limit"
            );
            result.extend(line);
        }
        Ok(result)
    }
    fn uidl(&mut self) -> Result<Vec<(u32, String)>> {
        self.command("UIDL")?;
        let bytes = self.multiline(4 * 1024 * 1024)?;
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|l| {
                let mut parts = l.split_whitespace();
                let n = parts.next().context("Invalid POP3 UIDL")?.parse()?;
                let uid = parts.next().context("Invalid POP3 UIDL")?.to_string();
                Ok((n, uid))
            })
            .collect()
    }
}
fn pop_login(
    a: &Account,
    password: &str,
    tls: &native_tls::TlsConnector,
) -> Result<PopSession<native_tls::TlsStream<TcpStream>>> {
    let tcp = tcp(&a.incoming_host, a.incoming_port)?;
    let stream = tls.connect(&a.incoming_host, tcp)?;
    let mut s = PopSession::new(stream)?;
    s.command(&format!("USER {}", a.email))?;
    s.command(&format!("PASS {password}"))?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_mime_body_and_attachment() -> Result<()> {
        let raw=b"From: =?UTF-8?Q?Sophie_Chen?= <sophie@example.com>\r\nTo: me@example.com\r\nSubject: =?UTF-8?Q?Hello_=E2=9C=93?=\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHi there!\r\n--x\r\nContent-Type: text/plain; name=hello.txt\r\nContent-Disposition: attachment; filename=hello.txt\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let m = parse_message(
            raw,
            &Account::new(Provider::Imap),
            "id",
            Folder::Inbox,
            "INBOX",
            1,
        )?;
        assert_eq!(m.sender, "Sophie Chen");
        assert_eq!(m.subject, "Hello ✓");
        assert!(m.body.unwrap().contains("Hi there!"));
        assert_eq!(m.attachments[0].name, "hello.txt");
        assert_eq!(m.attachments[0].size, 5);
        Ok(())
    }
    #[test]
    fn html_does_not_display_script() {
        assert_eq!(
            html_to_text(
                "<p>Hello &amp; welcome</p><script>bad()</script><style>css</style><p>World</p>"
            ),
            "Hello & welcome\nWorld"
        );
    }
    #[test]
    fn pop3_loopback_uidl_and_dot_unstuffing() -> Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"+OK ready\r\n").unwrap();
            let mut r = BufReader::new(s);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "UIDL\r\n");
            r.get_mut()
                .write_all(b"+OK\r\n1 abc\r\n2 def\r\n.\r\n")
                .unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "RETR 2\r\n");
            r.get_mut()
                .write_all(b"+OK\r\nSubject: Test\r\n\r\n..hello\r\n.\r\n")
                .unwrap();
        });
        let mut p = PopSession::new(TcpStream::connect(address)?)?;
        assert_eq!(p.uidl()?, vec![(1, "abc".into()), (2, "def".into())]);
        p.command("RETR 2")?;
        assert!(String::from_utf8(p.multiline(1024)?)?.contains("\r\n.hello\r\n"));
        server.join().unwrap();
        Ok(())
    }
    #[test]
    fn rejects_pop_command_injection() -> Result<()> {
        let mut p = PopSession::new(std::io::Cursor::new(b"+OK\r\n".to_vec()))?;
        assert!(p.command("USER a\r\nDELE 1").is_err());
        Ok(())
    }
}
