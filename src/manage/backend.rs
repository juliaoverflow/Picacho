//! Account operations serialize protocol work and commit results to the local cache.
use crate::manage::{
    model::*,
    store::{self, Store},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    net::TcpStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroize;

use crate::sync::transport::*;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
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
        let mut secret = if account.service == crate::manage::model::Service::Pinal {
            let mailbox = crate::sync::oauth::pinal(cancel.clone())?;
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
            let (secret, who) = crate::sync::oauth::authorize(&account, cancel.clone())?;
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
        crate::sync::oauth::refresh(a, &mut s)?;
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
        let bundle: crate::manage::crypto::KeyBundle = serde_json::from_str(&s.mail_bundle)?;
        let secret = secret.trim_end_matches(['\r', '\n']);
        let mk = if crate::manage::crypto::looks_like_recovery_key(secret) {
            crate::manage::crypto::unlock_with_recovery_key(&bundle, secret)?
        } else {
            crate::manage::crypto::unlock_with_password(&bundle, secret)?
        };
        let keys = crate::manage::crypto::open_keys(&bundle, &mk)?;
        s.mail_key = crate::manage::crypto::encode_master_key(&mk);
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
        let Some(mk) = crate::manage::crypto::decode_master_key(&s.mail_key) else {
            return Ok(None);
        };
        let bundle: crate::manage::crypto::KeyBundle = serde_json::from_str(&s.mail_bundle)?;
        let keys = Arc::new(crate::manage::crypto::open_keys(&bundle, &mk)?);
        self.mail_keys
            .lock()
            .unwrap()
            .insert(a.id.clone(), keys.clone());
        Ok(Some(keys))
    }
    /// The readable message: decrypted here when Pinal stored it encrypted.
    fn plaintext(
        &self,
        a: &Account,
        raw: &[u8],
    ) -> Result<(crate::manage::crypto::Protection, Vec<u8>)> {
        if crate::manage::crypto::encrypted_part(raw).is_none() {
            return crate::manage::crypto::open_message(raw, &[]);
        }
        let Some(keys) = self.keys(a)? else {
            // Accounts added before encryption existed have no key bundle.
            ensure!(
                a.service != Service::Pinal || !self.credentials(a)?.mail_bundle.is_empty(),
                "This message is encrypted. Remove this Pinal account and add it again so \
                 Picacho receives your encryption keys."
            );
            return Err(crate::manage::crypto::Locked.into());
        };
        crate::manage::crypto::open_message(raw, &keys)
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
        if protection == crate::manage::crypto::Protection::Pinal {
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
                    if crate::manage::crypto::encrypted_part(raw).is_some() {
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
        use lettre::transport::smtp::client::TlsParameters;
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
        let message = crate::sync::smtp::deliver(a, d, &secret, tls)?;
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
