//! Account operations serialize protocol work and commit results to the local cache.
use crate::manage::status::AccessFailure;
use crate::manage::{
    model::*,
    store::{self, Store},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroize;

use crate::sync::transport::*;
#[cfg(test)]
#[path = "dovecot_tests.rs"]
mod dovecot_tests;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
#[cfg(test)]
#[path = "reliability_tests.rs"]
mod reliability_tests;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Read(bool),
    Star(bool),
    Move(Folder),
}
/// How much of a mailbox has been listed on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub cataloged: usize,
    pub total: usize,
}
impl Catalog {
    pub fn complete(&self) -> bool {
        self.cataloged >= self.total
    }
}

pub struct Backend {
    pub store: Arc<Store>,
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    credentials: Mutex<HashMap<String, Result<Secret, String>>>,
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
            credentials: Mutex::new(HashMap::new()),
            mail_keys: Mutex::new(HashMap::new()),
            readable: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_certificate: None,
            #[cfg(test)]
            test_secret: None,
        }
    }
    pub(super) fn account_lock(&self, id: &str) -> Arc<Mutex<()>> {
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
    ) -> Result<PopSession<native_tls::TlsStream<crate::sync::wire::Wire>>> {
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
            let issued = mailbox.account();
            ensure!(
                account.email.is_empty() || account.email.eq_ignore_ascii_case(&issued.email),
                "Choose the same email address to reconnect this mailbox."
            );
            account = Account {
                id: account.id.clone(),
                sync: account.sync,
                name: if account.name.is_empty() {
                    issued.name.clone()
                } else {
                    account.name.clone()
                },
                ..issued
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
            ensure!(
                account.email.is_empty() || account.email.eq_ignore_ascii_case(&who.email),
                "Choose the same email address to reconnect this mailbox."
            );
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
            let _ = pop.command("QUIT");
        } else {
            let mut session = self.imap_login(&account, &secret)?;
            let _ = session.logout();
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
        self.save_credentials(&account.id, &secret)?;
        self.store.save_account(&account)?;
        Ok(account)
    }
    pub(super) fn credentials(&self, a: &Account) -> Result<Secret> {
        #[cfg(test)]
        if let Some(secret) = &self.test_secret {
            return Ok(secret.clone());
        }
        let mut s = self.saved_credentials(&a.id)?;
        if crate::sync::oauth::refresh(a, &mut s)? {
            self.save_credentials(&a.id, &s)?;
        }
        Ok(s)
    }
    // Callers serialize by account. Cache failures too: a denied Keychain read
    // must not reopen system dialogs for every sync, body, or encryption check.
    fn saved_credentials(&self, id: &str) -> Result<Secret> {
        self.load_credentials(id, || store::read_secret(id))
    }
    fn load_credentials(&self, id: &str, read: impl FnOnce() -> Result<Secret>) -> Result<Secret> {
        if let Some(saved) = self.credentials.lock().unwrap().get(id).cloned() {
            return saved.map_err(|e| anyhow::anyhow!(e).context(AccessFailure::Keychain));
        }
        let saved = read().map_err(|e| format!("{e:#}"));
        self.credentials
            .lock()
            .unwrap()
            .insert(id.into(), saved.clone());
        saved.map_err(|e| anyhow::anyhow!(e).context(AccessFailure::Keychain))
    }
    fn save_credentials(&self, id: &str, secret: &Secret) -> Result<()> {
        if let Err(error) = store::write_secret(id, secret) {
            self.credentials
                .lock()
                .unwrap()
                .insert(id.into(), Err(format!("{error:#}")));
            return Err(error.context(AccessFailure::Keychain));
        }
        self.credentials
            .lock()
            .unwrap()
            .insert(id.into(), Ok(secret.clone()));
        Ok(())
    }
    /// Only an explicit user retry reopens denied/missing Keychain entries.
    pub fn retry_credentials(&self, id: &str) {
        let lock = self.account_lock(id);
        let _operation = lock.lock().unwrap();
        let mut saved = self.credentials.lock().unwrap();
        if saved.get(id).is_some_and(Result::is_err) {
            saved.remove(id);
        }
    }
    pub fn remove_account(&self, id: &str) -> Result<()> {
        let lock = self.account_lock(id);
        let _operation = lock.lock().unwrap();
        self.store.remove_account(id)?;
        self.credentials.lock().unwrap().remove(id);
        self.mail_keys.lock().unwrap().remove(id);
        self.readable.lock().unwrap().retain(|_, m| m.account != id);
        // A missing/inaccessible keychain must not prevent removing local mail.
        let _ = store::secret_entry(id).and_then(|e| e.delete_credential().map_err(Into::into));
        Ok(())
    }
    /// Replaces credentials while preserving the account, cache, and preferences.
    pub fn reconnect(
        &self,
        account: &Account,
        password: String,
        cancel: Arc<AtomicBool>,
    ) -> Result<Account> {
        let mut next = account.clone();
        if next.oauth && next.client_secret.is_empty() {
            let registration = crate::sync::oauth::client(next.provider);
            if !registration.id.is_empty() {
                next.client_id = registration.id;
                next.client_secret = registration.secret;
            } else if next.provider == Provider::Gmail {
                // Client secrets deliberately never live in the account row.
                let lock = self.account_lock(&next.id);
                let _operation = lock.lock().unwrap();
                next.client_secret = self.saved_credentials(&next.id)?.client_secret;
            }
        }
        self.connect(&next, password, &[], cancel)
    }
    /// Whether this account's mail is encrypted with a Pinal key bundle.
    pub fn encrypted(&self, a: &Account) -> bool {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        #[cfg(test)]
        if let Some(secret) = &self.test_secret {
            return !secret.mail_bundle.is_empty();
        }
        self.saved_credentials(&a.id)
            .map(|s| !s.mail_bundle.is_empty())
            .unwrap_or(false)
    }
    /// Opens the key bundle with the Pinal password (or the recovery key) and
    /// keeps the mail key in the keychain, so later launches need no password.
    pub fn unlock(&self, a: &Account, secret: &str) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
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
        self.save_credentials(&a.id, &s)?;
        self.mail_keys
            .lock()
            .unwrap()
            .insert(a.id.clone(), Arc::new(keys));
        Ok(())
    }
    /// Forgets the mail key on this device; the password unlocks it again.
    pub fn lock_mail(&self, a: &Account) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.mail_keys.lock().unwrap().remove(&a.id);
        self.readable
            .lock()
            .unwrap()
            .retain(|_, m| m.account != a.id);
        let mut s = self.saved_credentials(&a.id)?;
        s.mail_key.zeroize();
        s.mail_key.clear();
        self.save_credentials(&a.id, &s)
    }
    /// Puts decrypted bodies back on messages reloaded from the disk cache.
    pub fn with_readable(&self, mut messages: Vec<Mail>) -> Vec<Mail> {
        let readable = self.readable.lock().unwrap();
        for m in messages.iter_mut().filter(|m| m.body.is_none()) {
            if let Some(r) = readable.get(&m.id) {
                m.body = r.body.clone();
                m.html = r.html.clone();
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
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        match self.sync_once(a, &mailbox, limit) {
            Err(error) if crate::sync::retry::transient(&error) => {
                // Only downloads/snapshots are replayed, never MOVE or SMTP.
                std::thread::sleep(std::time::Duration::from_millis(250));
                self.sync_once(a, &mailbox, limit)
            }
            result => result,
        }
    }
    fn sync_once(&self, a: &Account, mailbox: &Mailbox, limit: usize) -> Result<Vec<Mail>> {
        let folder = mailbox.folder();
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
            let remote = remote_name(&names, a, mailbox)?;
            result = self.fetch_mailbox(&mut session, a, mailbox, &remote, false, limit)?;
        }
        let _ = session.logout();
        Ok(result)
    }
    /// Catalogs up to `batch` more of a mailbox's headers, newest first,
    /// alongside the newest mail that `sync` keeps current. Repeated calls
    /// list the whole mailbox (within its sync window) without any manual
    /// paging. Bodies still download separately.
    pub fn catalog(
        &self,
        a: &Account,
        mailbox: impl Into<Mailbox>,
        batch: usize,
    ) -> Result<Catalog> {
        let mailbox = mailbox.into();
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        match self.catalog_once(a, &mailbox, batch) {
            Err(error) if crate::sync::retry::transient(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(250));
                self.catalog_once(a, &mailbox, batch)
            }
            result => result,
        }
    }
    fn catalog_once(&self, a: &Account, mailbox: &Mailbox, batch: usize) -> Result<Catalog> {
        let folder = mailbox.folder();
        // Drafts are local; Starred is assembled from the other mailboxes.
        if matches!(folder, Folder::Drafts | Folder::Starred) {
            return Ok(Catalog::default());
        }
        let secret = self.credentials(a)?;
        if a.provider == Provider::Pop3 {
            return self.catalog_pop(a, folder, batch, &secret.password);
        }
        let mut session = self.imap_login(a, &secret)?;
        let names = list(&mut session)?;
        let remote = remote_name(&names, a, mailbox)?;
        let selected = session.select(&remote)?;
        let validity = selected
            .uid_validity
            .filter(|v| *v > 0)
            .context("The server did not supply a valid UIDVALIDITY; cached mail has been kept.")?;
        let mut query = "ALL".to_owned();
        if a.sync.days > 0 {
            let since = chrono::Utc::now() - chrono::Duration::days(a.sync.days.into());
            query = format!("{query} SINCE {}", since.format("%-d-%b-%Y"));
        }
        let mut uids: Vec<_> = session.uid_search(&query)?.into_iter().collect();
        ensure!(
            uids.iter().all(|uid| *uid > 0),
            "Invalid IMAP UID in search response"
        );
        uids.sort_unstable();
        let prefix = format!("{}:{remote}:{validity}:", a.id);
        let cached = self.store.cached_ids(&a.id, &prefix)?;
        let all_ids: Vec<String> = uids.iter().map(|uid| format!("{prefix}{uid}")).collect();
        let missing: Vec<u32> = uids
            .iter()
            .zip(&all_ids)
            .filter(|(_, id)| !cached.contains(*id))
            .map(|(uid, _)| *uid)
            .collect();
        let next = &missing[missing.len().saturating_sub(batch)..];
        let result = fetch_headers(
            &mut session,
            a,
            folder,
            &remote,
            validity,
            next,
            &HashMap::new(),
        )?;
        // UIDs expunged since SEARCH have no headers; the next search drops
        // them. A batch with no headers at all would never finish.
        ensure!(
            next.is_empty() || !result.is_empty(),
            "The server omitted requested headers; cached mail has been kept. Refresh to retry."
        );
        self.store.apply_snapshot(
            &store::MailboxSnapshot {
                account: &a.id,
                mailbox,
                remote: &remote,
                validity,
                ids: &all_ids,
                starred_only: false,
            },
            &result,
        )?;
        let _ = session.logout();
        Ok(Catalog {
            cataloged: uids.len() - missing.len() + result.len(),
            total: uids.len(),
        })
    }
    /// POP3 has no headers-only listing, so older messages are downloaded
    /// whole, a small batch at a time. Originals stay on the server.
    fn catalog_pop(
        &self,
        a: &Account,
        folder: Folder,
        batch: usize,
        password: &str,
    ) -> Result<Catalog> {
        if folder != Folder::Inbox {
            return Ok(Catalog::default());
        }
        let mut pop = self.pop_login(a, password)?;
        let ids = pop.uidl()?;
        let prefix = format!("{}:pop:", a.id);
        let cached = self.store.cached_ids(&a.id, &prefix)?;
        let missing: Vec<&(u32, String)> = ids
            .iter()
            .rev()
            .filter(|(_, uid)| !cached.contains(&format!("{prefix}{uid}")))
            .collect();
        let mut fetched = 0;
        for (number, uid) in missing.iter().take(batch.min(25)) {
            let id = format!("{prefix}{uid}");
            pop.command(&format!("RETR {number}"))?;
            let raw = pop.multiline(MAX_MESSAGE)?;
            let mut m = parse_message(&raw, a, &id, Folder::Inbox, "INBOX", *number)?;
            m.size = raw.len() as u32;
            self.store.save_download(&m, &raw)?;
            fetched += 1;
        }
        let _ = pop.command("QUIT");
        Ok(Catalog {
            cataloged: ids.len() - missing.len() + fetched,
            total: ids.len(),
        })
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
        let validity = selected
            .uid_validity
            .filter(|v| *v > 0)
            .context("The server did not supply a valid UIDVALIDITY; cached mail has been kept.")?;
        let mut query = if starred_only { "FLAGGED" } else { "ALL" }.to_owned();
        if a.sync.days > 0 {
            // Older mail stays on the server; reconciling drops it from the cache.
            let since = chrono::Utc::now() - chrono::Duration::days(a.sync.days.into());
            query = format!("{query} SINCE {}", since.format("%-d-%b-%Y"));
        }
        let mut uids: Vec<_> = session.uid_search(&query)?.into_iter().collect();
        ensure!(
            uids.iter().all(|uid| *uid > 0),
            "Invalid IMAP UID in search response"
        );
        uids.sort_unstable();
        let mut all_ids: Vec<_> = uids
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
        let result = fetch_headers(session, a, folder, remote, validity, uids, &cached)?;
        let mut result = result;
        if result.len() != uids.len() {
            // An expunge can race SEARCH/FETCH. Confirm vanished UIDs before
            // removing them; an incomplete successful FETCH is not a snapshot.
            let current = session.uid_search(&query)?;
            ensure!(
                uids.iter()
                    .all(|uid| !current.contains(uid) || result.iter().any(|m| m.uid == *uid)),
                "The server omitted requested headers; cached mail has been kept. Refresh to retry."
            );
            all_ids.retain(|id| {
                id.rsplit(':')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .is_some_and(|uid| current.contains(&uid))
            });
            result.retain(|m| current.contains(&m.uid));
        }
        self.store.apply_snapshot(
            &store::MailboxSnapshot {
                account: &a.id,
                mailbox,
                remote,
                validity,
                ids: &all_ids,
                starred_only,
            },
            &result,
        )?;
        let keep: std::collections::HashSet<_> = all_ids.iter().collect();
        let prefix = format!("{}:{remote}:{validity}:", a.id);
        self.readable.lock().unwrap().retain(|id, m| {
            m.account != a.id
                || m.remote_folder != remote
                || (id.starts_with(&prefix) && (starred_only || keep.contains(id)))
        });
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
            let cached = self.store.message(&id)?;
            let complete = cached.is_some() && self.store.raw(&id)?.is_some();
            let mail = if let Some(m) = cached.as_ref().filter(|_| complete) {
                m.clone()
            } else {
                pop.command(&format!("RETR {number}"))?;
                let raw = pop.multiline(MAX_MESSAGE)?;
                let mut m = parse_message(&raw, a, &id, Folder::Inbox, "INBOX", *number)?;
                m.size = raw.len() as u32;
                if let Some(old) = cached {
                    m.folder = old.folder;
                    m.unread = old.unread;
                    m.starred = old.starred;
                }
                self.store.save_download(&m, &raw)?;
                m
            };
            messages.push(mail);
        }
        let _ = pop.command("QUIT");
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
            let fetched = session.uid_fetch(m.uid.to_string(), "(UID BODY.PEEK[])")?;
            let raw = fetched
                .iter()
                .filter(|f| f.uid == Some(m.uid))
                .find_map(|f| f.body())
                .context("Message no longer exists on the server")?
                .to_vec();
            ensure!(raw.len() <= MAX_MESSAGE, "Message exceeds the 25 MB limit.");
            let _ = session.logout();
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
            kept.html = None;
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
            ensure!(m.account == a.id, "Message belongs to another account");
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
                for chunk in uids.chunks(200) {
                    let set = uid_set(chunk);
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
                            session.run_command_and_check_ok(format!(
                                "UID MOVE {set} \"{escaped}\""
                            ))?;
                        }
                    }
                    let confirmed: Vec<_> = current
                        .iter()
                        .filter(|m| {
                            m.remote_folder == first.remote_folder
                                && same_validity(m, first)
                                && chunk.contains(&m.uid)
                        })
                        .cloned()
                        .collect();
                    self.commit_change(&confirmed, change, true)?;
                }
            }
            let _ = session.logout();
            return Ok(());
        }
        self.commit_change(&current, change, false)
    }
    fn commit_change(&self, current: &[Mail], change: Change, remote: bool) -> Result<()> {
        let updated: Vec<_> = current
            .iter()
            .cloned()
            .map(|mut next| {
                match change {
                    Change::Read(read) => next.unread = !read,
                    Change::Star(star) => next.starred = star,
                    Change::Move(folder) => next.folder = folder,
                }
                next
            })
            .collect();
        self.store
            .commit_changes(&updated, remote && matches!(change, Change::Move(_)))
    }
    /// Marks every message in a mailbox read, on the server and in the cache.
    pub fn mark_all_read(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        if mailbox.folder() == Folder::Starred {
            let starred = self.store.messages(&a.id, mailbox, usize::MAX >> 1)?;
            return self.change_many(a, &starred, Change::Read(true));
        }
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
            let _ = session.logout();
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
            select_message_mailbox(&mut session, group[0]).context(
                "Could not download messages from their mailbox. Refresh that mailbox first.",
            )?;
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
        let _ = session.logout();
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
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
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

/// Headers, flags and sizes for `uids` in the selected mailbox. Messages
/// already cached keep their parsed fields; bodies are fetched when opened.
fn fetch_headers(
    session: &mut ImapSession,
    a: &Account,
    folder: Folder,
    remote: &str,
    validity: u32,
    uids: &[u32],
    cached: &HashMap<String, Mail>,
) -> Result<Vec<Mail>> {
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
            // Unsolicited flag updates are legal during FETCH. They are
            // not downloaded headers and must not create or replace mail.
            if !chunk.contains(&uid) || f.header().is_none() {
                continue;
            }
            ensure!(
                !result.iter().any(|m: &Mail| m.uid == uid),
                "Duplicate IMAP header response"
            );
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
    Ok(result)
}

/// The server mailbox behind a mailbox view.
fn remote_name(names: &[Name], a: &Account, mailbox: &Mailbox) -> Result<String> {
    Ok(match mailbox {
        Mailbox::Remote(name) => {
            ensure!(
                names.iter().any(|n| &n.name == name),
                "The folder “{name}” no longer exists on the server."
            );
            name.clone()
        }
        Mailbox::Standard(folder) => resolve_in(names, a.provider, *folder)?,
    })
}
