//! Account operations serialize protocol work and commit results to the local cache.
use crate::manage::status::{AccessFailure, SignInStep};
use crate::manage::{
    model::*,
    store::{self, Store},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroize;

use crate::sync::transport::*;
#[path = "connection.rs"]
mod connection;
use connection::{Connection, Limits};
#[path = "watch.rs"]
mod watch;
use watch::Watcher;
#[cfg(test)]
#[path = "dovecot_tests.rs"]
mod dovecot_tests;
#[cfg(test)]
#[path = "incremental_tests.rs"]
mod incremental_tests;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
#[cfg(test)]
#[path = "extension_tests.rs"]
mod extension_tests;
#[cfg(test)]
#[path = "reliability_tests.rs"]
mod reliability_tests;
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Read(bool),
    Star(bool),
    Move(Folder),
    /// Moves mail into one of the account's own server folders, by name.
    MoveTo(String),
    /// Adds (true) or removes a label: a Gmail label or an IMAP keyword.
    Label(String, bool),
}
impl Change {
    /// Whether this moves mail to another mailbox.
    pub fn moves(&self) -> bool {
        matches!(self, Self::Move(_) | Self::MoveTo(_))
    }
    /// Whether `m` is already in the mailbox this change moves mail to.
    pub fn moved_already(&self, m: &Mail) -> bool {
        match self {
            Self::Move(folder) => m.folder == *folder,
            Self::MoveTo(name) => m.folder == Folder::Other && &m.remote_folder == name,
            _ => false,
        }
    }
    /// Applies the change to a copy of a message.
    pub fn apply(&self, m: &mut Mail) {
        match self {
            Self::Read(read) => m.unread = !read,
            Self::Star(star) => m.starred = *star,
            Self::Move(folder) => m.folder = *folder,
            Self::MoveTo(name) => {
                m.folder = Folder::Other;
                m.remote_folder = name.clone();
            }
            Self::Label(label, add) => {
                m.labels.retain(|l| l != label);
                if *add {
                    m.labels.push(label.clone());
                    m.labels.sort();
                }
            }
        }
    }
}
/// How much of a mailbox has been listed on this computer.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub cataloged: usize,
    pub total: usize,
    /// Messages this request listed.
    pub added: Vec<Mail>,
    /// Every message in the mailbox, for drawing rows not yet listed.
    pub layout: Option<Layout>,
}
/// The UIDs of every message in a server mailbox (within the sync window),
/// oldest first: one row each, in arrival order, before their headers load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub remote: String,
    pub validity: u32,
    days: u32,
    pub uids: Arc<Vec<u32>>,
}
impl Layout {
    pub fn id(&self, account: &str, uid: u32) -> String {
        format!("{account}:{}:{}:{uid}", self.remote, self.validity)
    }
}
/// A folder made or renamed on the server.
#[derive(Clone, Debug)]
pub struct FolderChange {
    /// Renamed folders, the one asked for and those inside it: (old, new).
    pub moved: Vec<(String, String)>,
    /// The account's folders afterwards.
    pub folders: Vec<RemoteFolder>,
}
/// A folder command's result, with a refusal (NO or BAD, say for lacking
/// the right) told as the server's own reason.
fn allowed(result: imap::error::Result<()>, what: impl FnOnce() -> String) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(imap::error::Error::No(why) | imap::error::Error::Bad(why)) => {
            bail!("The server didn't allow this account to {}: {why}", what())
        }
        Err(e) => Err(e).with_context(|| format!("Couldn't {}", what())),
    }
}
/// Subscribes to a new folder so other mail apps, which often list only
/// subscribed folders, show it too. A server that refuses is no matter.
fn subscribe(session: &mut ImapSession, name: &str) -> Result<()> {
    match session.run_command_and_check_ok(format!("SUBSCRIBE {}", quote(name)?)) {
        Ok(()) | Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
impl Catalog {
    #[cfg(test)]
    pub fn complete(&self) -> bool {
        self.cataloged >= self.total
    }
}

/// Where a sign-in stopped after the browser answered.
enum Stage {
    /// The token request failed before the provider judged the code.
    Redeem(crate::sync::oauth::Grant),
    /// Tokens or a password were issued, but the mail server refused them.
    Verify(Secret),
}
struct Pending {
    account: Account,
    taken: Vec<String>,
    stage: Stage,
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
    /// Signed-in IMAP connections by account; see `connection.rs`.
    connections: Mutex<HashMap<String, Connection>>,
    limits: Mutex<Limits>,
    /// Mailbox layouts by (account, mailbox key), from the latest search.
    layouts: Mutex<HashMap<(String, String), Layout>>,
    /// A sign-in that failed after the browser, kept for one retry.
    pending: Mutex<Option<Pending>>,
    /// How far the current sign-in has come; see `sign_in_step`.
    sign_in_step: Mutex<SignInStep>,
    /// Messages being opened that wait for their account's connection, by
    /// account. Background downloads give way to them.
    waiting: Mutex<HashMap<String, usize>>,
    /// Inboxes watched for new mail with IDLE, by account; see `watch.rs`.
    watchers: Mutex<HashMap<String, Watcher>>,
    #[cfg(test)]
    test_certificate: Option<Vec<u8>>,
    #[cfg(test)]
    test_secret: Option<Secret>,
}
/// See `Backend::wait_for`.
struct Waiting<'a> {
    backend: &'a Backend,
    id: String,
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut waiting = self.backend.waiting.lock().unwrap();
        if let Some(n) = waiting.get_mut(&self.id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                waiting.remove(&self.id);
            }
        }
    }
}
impl Backend {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            operations: Mutex::new(HashMap::new()),
            credentials: Mutex::new(HashMap::new()),
            mail_keys: Mutex::new(HashMap::new()),
            readable: Mutex::new(HashMap::new()),
            connections: Mutex::new(HashMap::new()),
            limits: Mutex::new(Limits::default()),
            layouts: Mutex::new(HashMap::new()),
            pending: Mutex::new(None),
            sign_in_step: Mutex::new(SignInStep::Browser),
            waiting: Mutex::new(HashMap::new()),
            watchers: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_certificate: None,
            #[cfg(test)]
            test_secret: None,
        }
    }
    /// Notes that someone is waiting for `id`'s connection until the
    /// returned guard is dropped.
    fn wait_for(&self, id: &str) -> Waiting<'_> {
        *self
            .waiting
            .lock()
            .unwrap()
            .entry(id.to_owned())
            .or_default() += 1;
        Waiting {
            backend: self,
            id: id.to_owned(),
        }
    }
    /// Whether someone is waiting to open a message of `id`'s.
    fn someone_waiting(&self, id: &str) -> bool {
        self.waiting.lock().unwrap().get(id).is_some_and(|n| *n > 0)
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
    /// that was chosen; `taken` lists addresses already added. When a step
    /// after the browser fails in a way that can be tried again, the sign-in
    /// is kept for `retry_sign_in` (see `can_retry_sign_in`).
    pub fn connect(
        &self,
        account: &Account,
        password: String,
        taken: &[String],
        cancel: Arc<AtomicBool>,
    ) -> Result<Account> {
        self.forget_sign_in();
        self.step(&cancel, SignInStep::Browser);
        let mut account = account.clone();
        if account.service == crate::manage::model::Service::Pinal {
            let mailbox = crate::sync::oauth::pinal(cancel.clone(), &|| {
                self.step(&cancel, SignInStep::Finishing)
            })?;
            let issued = mailbox.account();
            ensure!(
                account.email.is_empty() || account.email.eq_ignore_ascii_case(&issued.email),
                "Choose the same email address to reconnect this mailbox."
            );
            account = Account {
                id: account.id.clone(),
                sync: account.sync,
                nickname: account.nickname.clone(),
                name: if account.name.is_empty() {
                    issued.name.clone()
                } else {
                    account.name.clone()
                },
                ..issued
            };
            let secret = Secret {
                password: mailbox.password,
                mail_bundle: mailbox
                    .encryption
                    .and_then(|e| e.bundle)
                    .map(|b| serde_json::to_string(&b))
                    .transpose()?
                    .unwrap_or_default(),
                ..Default::default()
            };
            self.verify(account, secret, taken, &cancel, true)
        } else if account.oauth {
            let grant = crate::sync::oauth::authorize(&account, cancel.clone())?;
            self.redeem(account, grant, taken, &cancel)
        } else {
            ensure!(!password.is_empty(), "Enter your password or app password.");
            let secret = Secret {
                password,
                ..Default::default()
            };
            self.verify(account, secret, taken, &cancel, false)
        }
    }
    /// How far the running sign-in has come.
    pub fn sign_in_step(&self) -> SignInStep {
        *self.sign_in_step.lock().unwrap()
    }
    /// Records progress unless this sign-in was abandoned, so a stale one
    /// finishing in the background cannot relabel a newer one.
    fn step(&self, cancel: &AtomicBool, step: SignInStep) {
        if !cancel.load(Ordering::Relaxed) {
            *self.sign_in_step.lock().unwrap() = step;
        }
    }
    /// Whether the last `connect` failed after the browser and can be tried
    /// again without signing in again.
    pub fn can_retry_sign_in(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }
    /// Drops a sign-in kept for retrying, along with its tokens.
    pub fn forget_sign_in(&self) {
        self.pending.lock().unwrap().take();
    }
    /// Repeats the step of the last sign-in that failed after the browser.
    pub fn retry_sign_in(&self, cancel: Arc<AtomicBool>) -> Result<Account> {
        let Pending {
            account,
            taken,
            stage,
        } = self
            .pending
            .lock()
            .unwrap()
            .take()
            .context("This sign-in can no longer be retried. Sign in again.")?;
        match stage {
            Stage::Redeem(grant) => self.redeem(account, grant, &taken, &cancel),
            Stage::Verify(secret) => self.verify(account, secret, &taken, &cancel, true),
        }
    }
    /// Turns the browser's code into tokens, keeping it for a retry when the
    /// provider never answered (a rejected code is spent).
    fn redeem(
        &self,
        mut account: Account,
        grant: crate::sync::oauth::Grant,
        taken: &[String],
        cancel: &AtomicBool,
    ) -> Result<Account> {
        self.step(cancel, SignInStep::Finishing);
        let (secret, who) = match crate::sync::oauth::redeem(&account, &grant) {
            Ok(signed_in) => signed_in,
            Err(e) => {
                if e.downcast_ref::<AccessFailure>().is_none() {
                    *self.pending.lock().unwrap() = Some(Pending {
                        account,
                        taken: taken.to_vec(),
                        stage: Stage::Redeem(grant),
                    });
                }
                return Err(e);
            }
        };
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
        self.verify(account, secret, taken, cancel, true)
    }
    /// Signs in to the incoming server and saves the account. With `keep`,
    /// a failed check keeps the credentials so the user can fix the mailbox
    /// (for example, turn on IMAP) and retry without the browser.
    fn verify(
        &self,
        account: Account,
        mut secret: Secret,
        taken: &[String],
        cancel: &AtomicBool,
        keep: bool,
    ) -> Result<Account> {
        account.validate()?;
        ensure!(
            !taken.iter().any(|e| e.eq_ignore_ascii_case(&account.email)),
            "{} is already added.",
            account.email
        );
        self.step(cancel, SignInStep::Mailbox);
        let lock = self.account_lock(&account.id);
        let _operation = lock.lock().unwrap();
        secret.client_secret = account.client_secret.clone();
        if let Err(e) = self.check_login(&account, &mut secret) {
            if keep {
                *self.pending.lock().unwrap() = Some(Pending {
                    account,
                    taken: taken.to_vec(),
                    stage: Stage::Verify(secret),
                });
            }
            return Err(e);
        }
        ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
        // A replaced sign-in must not keep using the old session.
        self.disconnect(&account.id);
        self.save_credentials(&account.id, &secret)?;
        self.store.save_account(&account)?;
        Ok(account)
    }
    fn check_login(&self, account: &Account, secret: &mut Secret) -> Result<()> {
        if account.provider == Provider::Pop3 {
            let mut pop = self.pop_login(account, &secret.password)?;
            let _ = pop.command("QUIT");
            return Ok(());
        }
        // A retry can come long after the browser; renew an expiring token.
        crate::sync::oauth::refresh(account, secret)?;
        let mut session = match self.imap_login(account, secret) {
            Err(e)
                if account.oauth
                    && account.provider == Provider::Outlook
                    && e.downcast_ref::<AccessFailure>().is_some() =>
            {
                // Microsoft issued a token, but its IMAP server refused the
                // mailbox: IMAP is off in Outlook's settings (new accounts),
                // or the sign-in address is not the mailbox address.
                return Err(e.context(format!(
                    "Microsoft signed you in, but Outlook's mail server refused {}. \
                     Outlook.com turns IMAP off for new accounts: at outlook.live.com, open \
                     Settings → Mail → Forwarding and IMAP, turn on IMAP, save, then choose Retry. If you sign in to \
                     Microsoft with a different address than your Outlook address, make the \
                     Outlook address your primary alias.",
                    account.email
                )));
            }
            other => other?,
        };
        let _ = session.logout();
        Ok(())
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
        self.disconnect(id);
        self.stop_watching(id);
        self.layouts
            .lock()
            .unwrap()
            .retain(|(account, _), _| account != id);
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
                m.inline = r.inline.clone();
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
    /// Whether this computer can decrypt the message: it is encrypted, and
    /// the account's keys opened it.
    pub fn decrypts(&self, a: &Account, raw: &[u8]) -> bool {
        matches!(
            self.plaintext(a, raw),
            Ok((crate::manage::crypto::Protection::Pinal, _))
        )
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
        if a.provider == Provider::Pop3 {
            let secret = self.credentials(a)?;
            return self.sync_pop(a, folder, limit, &secret.password);
        }
        if folder == Folder::Drafts {
            return Ok(vec![]);
        }
        self.with_imap(a, |c| {
            // One LIST per sync keeps the folder list current.
            let names = c.names(true)?.to_vec();
            let _ = self.store.save_folders(&a.id, &custom_folders(&names));
            if folder != Folder::Starred {
                let remote = remote_name(&names, a, mailbox)?;
                return self.sync_remote(c, a, mailbox, &remote, limit);
            }
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
            let mut result = Vec::new();
            let mut visited = std::collections::HashSet::new();
            for source in folders {
                let remote = match resolve_in(&names, a.provider, source) {
                    Ok(remote) => remote,
                    Err(_) if source != Folder::Inbox => continue,
                    Err(e) => return Err(e),
                };
                if visited.insert(remote.clone()) {
                    let selected = c.select(&remote)?;
                    result.extend(self.full_sync(
                        c,
                        a,
                        &source.into(),
                        &remote,
                        &selected,
                        true,
                        limit,
                    )?);
                }
            }
            result.sort_by(Mail::newest_first);
            let mut seen = std::collections::HashSet::new();
            result.retain(|m| {
                seen.insert(if m.message_id.is_empty() {
                    m.id.clone()
                } else {
                    m.message_id.clone()
                })
            });
            result.truncate(limit);
            Ok(result)
        })
    }
    /// Brings one server mailbox's cache up to date. After a first full
    /// synchronization, later ones ask only for what changed: messages past
    /// the last UIDNEXT, flags changed since the last HIGHESTMODSEQ (or the
    /// newest flags, without CONDSTORE), and a full search only when the
    /// message count shows something was removed.
    fn sync_remote(
        &self,
        c: &mut Connection,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        let selected = c.select(remote)?;
        let state = self.store.mailbox_state(&a.id, remote)?;
        let Some((state, uid_next)) = state.zip(selected.uid_next).filter(|(state, next)| {
            state.validity == selected.validity
                && state.days == a.sync.days
                && state.uid_next > 0
                && *next >= state.uid_next
                // Many new messages: list the newest; the catalog lists the rest.
                && selected.exists <= state.exists.saturating_add(limit as u32)
        }) else {
            return self.full_sync(c, a, mailbox, remote, &selected, false, limit);
        };
        let validity = selected.validity;
        let fresh = if uid_next > state.uid_next && selected.exists > 0 {
            // "n:*" always includes the newest message, even below n.
            self.list_messages(
                c,
                a,
                mailbox.folder(),
                remote,
                validity,
                &format!("{}:*", state.uid_next),
                |uid| uid >= state.uid_next,
            )?
        } else {
            Vec::new()
        };
        // Without removals, the count grows by exactly the new messages.
        if selected.exists != state.exists.saturating_add(fresh.len() as u32) {
            return self.full_sync(c, a, mailbox, remote, &selected, false, limit);
        }
        let prefix = format!("{}:{remote}:{validity}:", a.id);
        let Extensions {
            condstore, gmail, ..
        } = c.extensions()?;
        let items = flag_items(gmail);
        let rows = match (state.modseq, selected.modseq) {
            (Some(old), Some(new)) if condstore => {
                if new == old {
                    Vec::new()
                } else {
                    uid_fetch_rows(
                        &mut c.session,
                        "1:*",
                        &format!("{items} (CHANGEDSINCE {old})"),
                    )?
                }
            }
            _ => {
                let newest: Vec<u32> = self
                    .store
                    .messages(&a.id, mailbox, limit)?
                    .into_iter()
                    .filter(|m| m.id.starts_with(&prefix))
                    .map(|m| m.uid)
                    .collect();
                if newest.is_empty() {
                    Vec::new()
                } else {
                    uid_fetch_rows(&mut c.session, &uid_ranges(&newest), items)?
                }
            }
        };
        let flags: Vec<store::FlagUpdate> = rows
            .iter()
            .filter(|r| r.flags.is_some())
            .map(|r| store::FlagUpdate {
                uid: r.uid,
                unread: !r.seen(),
                starred: r.flagged(),
                labels: r.labels(gmail),
            })
            .collect();
        let before = (a.sync.days > 0)
            .then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
        self.store
            .apply_changes(mailbox, &a.id, &prefix, &fresh, &flags, before)?;
        self.store.save_mailbox_state(
            &a.id,
            remote,
            Some(&store::MailboxState {
                validity,
                uid_next,
                exists: selected.exists,
                modseq: selected.modseq,
                days: a.sync.days,
            }),
        )?;
        self.extend_layout(a, mailbox, validity, fresh.iter().map(|m| m.uid), &[]);
        let mut result = self.store.messages(&a.id, mailbox, limit)?;
        result.retain(|m| m.id.starts_with(&prefix));
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
        self.ensure_background(a)?;
        if a.provider == Provider::Pop3 {
            let secret = self.credentials(a)?;
            return self.catalog_pop(a, folder, batch, &secret.password);
        }
        self.with_imap(a, |c| {
            let remote = remote_name(c.names(false)?, a, mailbox)?;
            let selected = c.selected(&remote)?;
            let layout = self.layout_for(c, a, mailbox, &remote, &selected)?;
            let prefix = format!("{}:{remote}:{}:", a.id, layout.validity);
            let cached = self.store.cached_ids(&a.id, &prefix)?;
            let missing: Vec<u32> = layout
                .uids
                .iter()
                .copied()
                .filter(|uid| !cached.contains(&format!("{prefix}{uid}")))
                .collect();
            let next = &missing[missing.len().saturating_sub(batch)..];
            let added = self.list_uids(c, a, folder, &remote, layout.validity, next)?;
            // UIDs expunged since SEARCH have no headers; the next search drops
            // them. A batch with no headers at all would never finish.
            ensure!(
                next.is_empty() || !added.is_empty(),
                "The server omitted requested headers; cached mail has been kept. Refresh to retry."
            );
            self.store.save_messages(&added)?;
            Ok(Catalog {
                cataloged: layout.uids.len() - missing.len() + added.len(),
                total: layout.uids.len(),
                added,
                layout: Some(layout),
            })
        })
    }
    /// Lists specific messages of a mailbox now, for rows the list is
    /// showing before the catalog reaches them.
    pub fn fill(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        layout: &Layout,
        uids: &[u32],
    ) -> Result<Vec<Mail>> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let prefix = format!("{}:{}:{}:", a.id, layout.remote, layout.validity);
        let cached = self.store.cached_ids(&a.id, &prefix)?;
        let wanted: Vec<u32> = uids
            .iter()
            .copied()
            .filter(|uid| !cached.contains(&format!("{prefix}{uid}")))
            .collect();
        if wanted.is_empty() || a.provider == Provider::Pop3 {
            return Ok(Vec::new());
        }
        self.with_imap(a, |c| {
            let selected = c.selected(&layout.remote)?;
            ensure!(
                selected.validity == layout.validity,
                "The server reset this mailbox's message identities. Refresh this mailbox."
            );
            let added = self.list_uids(
                c,
                a,
                mailbox.folder(),
                &layout.remote,
                layout.validity,
                &wanted,
            )?;
            self.store.save_messages(&added)?;
            Ok(added)
        })
    }
    /// What the message list shows for a mailbox: its cached messages and,
    /// once its layout is known, a placeholder for every message not listed
    /// yet, all in arrival order (newest first) so rows keep their places
    /// as they load. Before this launch has a layout, the last sync's saved
    /// UIDVALIDITY gives the same arrival order, so the list doesn't reorder
    /// when the first sync after a relaunch finishes.
    pub fn listed(&self, account: &str, mailbox: &Mailbox) -> Result<(Vec<Mail>, Option<Layout>)> {
        let mut messages = self.store.summaries(account, mailbox.clone())?;
        let layout = self.layout(account, mailbox);
        let saved = if layout.is_none() && mailbox.folder() != Folder::Starred {
            match messages.first().map(|m| m.remote_folder.clone()) {
                Some(remote) => self
                    .store
                    .mailbox_state(account, &remote)?
                    .map(|state| format!("{account}:{remote}:{}:", state.validity)),
                None => None,
            }
        } else {
            None
        };
        if let Some(prefix) = &saved {
            sort_arrival(&mut messages, prefix);
        }
        if let Some(layout) = &layout {
            let prefix = layout.id(account, 0);
            let prefix = &prefix[..prefix.len() - 1];
            let listed: std::collections::HashSet<u32> = messages
                .iter()
                .filter(|m| m.id.starts_with(prefix))
                .map(|m| m.uid)
                .collect();
            messages.extend(
                layout
                    .uids
                    .iter()
                    .filter(|uid| !listed.contains(uid))
                    .map(|uid| {
                        Mail::placeholder(
                            layout.id(account, *uid),
                            account,
                            mailbox,
                            &layout.remote,
                            *uid,
                        )
                    }),
            );
            sort_arrival(&mut messages, prefix);
        }
        Ok((messages, layout))
    }
    /// The latest layout of a mailbox, if one is known.
    pub fn layout(&self, account: &str, mailbox: &Mailbox) -> Option<Layout> {
        self.layouts
            .lock()
            .unwrap()
            .get(&(account.to_owned(), mailbox.key()))
            .cloned()
    }
    /// Searches the mailbox once per generation (not once per batch), and
    /// reconciles the cache with what the search found.
    fn layout_for(
        &self,
        c: &mut Connection,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        selected: &crate::sync::transport::Selected,
    ) -> Result<Layout> {
        if let Some(layout) = self.layout(&a.id, mailbox).filter(|l| {
            l.remote == remote && l.validity == selected.validity && l.days == a.sync.days
        }) {
            return Ok(layout);
        }
        let uids = search(&mut c.session, a, false)?;
        let all_ids: Vec<String> = uids
            .iter()
            .map(|uid| format!("{}:{remote}:{}:{uid}", a.id, selected.validity))
            .collect();
        self.store.apply_snapshot(
            &store::MailboxSnapshot {
                account: &a.id,
                mailbox,
                remote,
                validity: selected.validity,
                ids: &all_ids,
                starred_only: false,
            },
            &[],
        )?;
        Ok(self.set_layout(a, mailbox, remote, selected.validity, uids))
    }
    fn set_layout(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        validity: u32,
        uids: Vec<u32>,
    ) -> Layout {
        let layout = Layout {
            remote: remote.into(),
            validity,
            days: a.sync.days,
            uids: Arc::new(uids),
        };
        self.layouts
            .lock()
            .unwrap()
            .insert((a.id.clone(), mailbox.key()), layout.clone());
        layout
    }
    /// Gives a mailbox a layout without a server, for display tests.
    #[cfg(test)]
    pub fn set_test_layout(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        validity: u32,
        uids: Vec<u32>,
    ) {
        self.set_layout(a, mailbox, remote, validity, uids);
    }
    /// Drops a mailbox's layout, so its next check lays it out again.
    fn forget_layout(&self, a: &Account, mailbox: &Mailbox) {
        self.layouts
            .lock()
            .unwrap()
            .remove(&(a.id.clone(), mailbox.key()));
    }
    /// Adds new and removes moved messages in a known layout.
    fn extend_layout(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        validity: u32,
        added: impl Iterator<Item = u32>,
        removed: &[u32],
    ) {
        let mut layouts = self.layouts.lock().unwrap();
        let Some(layout) = layouts.get_mut(&(a.id.clone(), mailbox.key())) else {
            return;
        };
        if layout.validity != validity {
            return;
        }
        let mut uids: Vec<u32> = layout.uids.iter().copied().chain(added).collect();
        uids.retain(|uid| !removed.contains(uid));
        uids.sort_unstable();
        uids.dedup();
        layout.uids = Arc::new(uids);
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
            ..Default::default()
        })
    }
    /// Searches the whole mailbox (within the sync window), lists the newest
    /// `limit` messages that are not cached, refreshes the flags of the cached
    /// ones among them, and reconciles removals: the first synchronization of
    /// a mailbox, and the fallback when changes cannot be read incrementally.
    #[allow(clippy::too_many_arguments)]
    fn full_sync(
        &self,
        c: &mut Connection,
        a: &Account,
        mailbox: &Mailbox,
        remote: &str,
        selected: &crate::sync::transport::Selected,
        starred_only: bool,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        let folder = mailbox.folder();
        let validity = selected.validity;
        let all = search(&mut c.session, a, starred_only)?;
        let mut all_ids: Vec<_> = all
            .iter()
            .map(|uid| format!("{}:{remote}:{validity}:{uid}", a.id))
            .collect();
        let newest = &all[all.len().saturating_sub(limit)..];
        // Only the newest rows are refreshed; do not load a whole cataloged
        // mailbox to find them.
        let mut cached: HashMap<u32, Mail> = HashMap::new();
        for uid in newest {
            if let Some(m) = self
                .store
                .message(&format!("{}:{remote}:{validity}:{uid}", a.id))?
            {
                cached.insert(*uid, m);
            }
        }
        let (known, unknown): (Vec<u32>, Vec<u32>) =
            newest.iter().partition(|uid| cached.contains_key(uid));
        let mut result = self.list_uids(c, a, folder, remote, validity, &unknown)?;
        let gmail = c.extensions()?.gmail;
        for chunk in known.chunks(500) {
            for row in uid_fetch_rows(&mut c.session, &uid_ranges(chunk), flag_items(gmail))? {
                // Unsolicited updates for other messages are not part of this.
                if !chunk.contains(&row.uid) || row.flags.is_none() {
                    continue;
                }
                let Some(old) = cached.get(&row.uid) else {
                    continue;
                };
                ensure!(
                    !result.iter().any(|m: &Mail| m.uid == row.uid),
                    "Duplicate IMAP flags response"
                );
                let mut m = old.clone();
                m.unread = !row.seen();
                m.starred = row.flagged();
                if let Some(labels) = row.labels(gmail) {
                    m.labels = labels;
                }
                result.push(m);
            }
        }
        if result.len() != newest.len() {
            // An expunge can race SEARCH/FETCH. Confirm vanished UIDs before
            // removing them; an incomplete successful FETCH is not a snapshot.
            let current: std::collections::HashSet<u32> = search(&mut c.session, a, starred_only)?
                .into_iter()
                .collect();
            ensure!(
                newest
                    .iter()
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
        if !starred_only {
            let state = selected.uid_next.map(|uid_next| store::MailboxState {
                validity,
                uid_next,
                exists: selected.exists,
                modseq: selected.modseq,
                days: a.sync.days,
            });
            self.store
                .save_mailbox_state(&a.id, remote, state.as_ref())?;
            let uids = all_ids
                .iter()
                .filter_map(|id| id.rsplit(':').next()?.parse().ok())
                .collect();
            self.set_layout(a, mailbox, remote, validity, uids);
        }
        let keep: std::collections::HashSet<_> = all_ids.iter().collect();
        let prefix = format!("{}:{remote}:{validity}:", a.id);
        self.readable.lock().unwrap().retain(|id, m| {
            m.account != a.id
                || m.remote_folder != remote
                || (id.starts_with(&prefix) && (starred_only || keep.contains(id)))
        });
        result.sort_by(Mail::newest_first);
        Ok(result)
    }
    /// Lists messages by UID, 100 per request.
    fn list_uids(
        &self,
        c: &mut Connection,
        a: &Account,
        folder: Folder,
        remote: &str,
        validity: u32,
        uids: &[u32],
    ) -> Result<Vec<Mail>> {
        let mut result = Vec::new();
        for chunk in uids.chunks(100) {
            result.extend(self.list_messages(
                c,
                a,
                folder,
                remote,
                validity,
                &uid_ranges(chunk),
                |uid| chunk.contains(&uid),
            )?);
        }
        Ok(result)
    }
    /// Headers (only the fields the list shows), flags, size and arrival date
    /// for the messages in `set` that `wanted` accepts. On Gmail this also
    /// reads each message's permanent id, which is shared across labels.
    #[allow(clippy::too_many_arguments)]
    fn list_messages(
        &self,
        c: &mut Connection,
        a: &Account,
        folder: Folder,
        remote: &str,
        validity: u32,
        set: &str,
        wanted: impl Fn(u32) -> bool,
    ) -> Result<Vec<Mail>> {
        if set.is_empty() {
            return Ok(Vec::new());
        }
        let gmail = c.extensions()?.gmail;
        let items = format!(
            "(UID FLAGS RFC822.SIZE INTERNALDATE{} {LIST_HEADERS})",
            if gmail { " X-GM-MSGID X-GM-LABELS" } else { "" }
        );
        let rows = uid_fetch_rows(&mut c.session, set, &items)?;
        let mut result: Vec<Mail> = Vec::new();
        let mut bytes = 0;
        for row in rows {
            // Unsolicited flag updates are legal during FETCH. They are
            // not downloaded headers and must not create or replace mail.
            let Some(header) = row.header.as_deref().filter(|_| wanted(row.uid)) else {
                continue;
            };
            ensure!(
                !result.iter().any(|m| m.uid == row.uid),
                "Duplicate IMAP header response"
            );
            bytes += header.len();
            let id = format!("{}:{remote}:{validity}:{}", a.id, row.uid);
            let mut m = parse_message(header, a, &id, folder, remote, row.uid)?;
            m.unread = !row.seen();
            m.starred = row.flagged();
            m.size = row.size.unwrap_or(0);
            m.gmail_id = row.gmail_id.unwrap_or(0);
            m.labels = row.labels(gmail).unwrap_or_default();
            if m.date == 0 {
                m.date = row.internal_date.unwrap_or_default();
            }
            result.push(m);
        }
        self.note_traffic(a, bytes);
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
        let mut erased = self.pop_erased(&a.id);
        // Forget deletions of mail no longer on the server.
        let before = erased.len();
        erased.retain(|uid| ids.iter().any(|(_, u)| u == uid));
        if erased.len() != before {
            self.store.set_preference(
                &format!("pop_erased:{}", a.id),
                &serde_json::to_string(&erased)?,
            )?;
        }
        let mut messages = Vec::new();
        for (number, uid) in ids
            .iter()
            .rev()
            .filter(|(_, uid)| !erased.contains(uid))
            .take(limit)
        {
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
        messages.sort_by(Mail::newest_first);
        if folder == Folder::Starred {
            messages.retain(|m| m.starred);
        }
        Ok(messages)
    }
    /// The message exactly as the server delivered it, downloading it first
    /// when only its headers are cached.
    pub fn original(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        if let Some(raw) = self.store.raw(&m.id)? {
            return Ok(raw);
        }
        self.read(a, m)?;
        self.store
            .raw(&m.id)?
            .context("The original message is not available offline.")
    }
    pub fn read(&self, a: &Account, m: &Mail) -> Result<Mail> {
        // Already downloaded: there is nothing to ask the server, so don't
        // wait for the account's connection, which a sync or a background
        // download can hold for a while.
        if let Some(raw) = self.store.raw(&m.id)? {
            let latest = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
            let (next, kept) = self.parse_read(a, &latest, &raw)?;
            // Flags or a move may have been committed meanwhile; those stay.
            return Ok(self.store.refresh_message(&kept)?.map_or(next.clone(), |current| {
                Mail {
                    unread: current.unread,
                    starred: current.starred,
                    folder: current.folder,
                    ..next
                }
            }));
        }
        let waiting = self.wait_for(&a.id);
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        drop(waiting);
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
            let raw = match self.store.gmail_raw(&a.id, m.gmail_id)? {
                // The same Gmail message, already downloaded under another label.
                Some(raw) => raw,
                None => self.with_imap(a, |c| {
                    c.select_for(m)?;
                    let fetched = c
                        .session
                        .uid_fetch(m.uid.to_string(), "(UID BODY.PEEK[])")?;
                    let raw = fetched
                        .iter()
                        .filter(|f| f.uid == Some(m.uid))
                        .find_map(|f| f.body())
                        .context("Message no longer exists on the server")?
                        .to_vec();
                    ensure!(raw.len() <= MAX_MESSAGE, "Message exceeds the 25 MB limit.");
                    Ok(raw)
                })?,
            };
            self.store.save_raw(&m.id, &raw)?;
            raw
        };
        let (next, kept) = self.parse_read(a, m, &raw)?;
        self.store.save_messages(&[kept])?;
        Ok(next)
    }
    /// A downloaded message read for showing, and what of it the cache
    /// keeps. Zero-access mail keeps only what the server could already
    /// see; its body stays in memory and is decrypted again later.
    fn parse_read(&self, a: &Account, m: &Mail, raw: &[u8]) -> Result<(Mail, Mail)> {
        let (protection, plain) = self.plaintext(a, raw)?;
        let mut next = parse_message(&plain, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        next.unread = m.unread;
        next.starred = m.starred;
        next.size = m.size;
        if protection == crate::manage::crypto::Protection::Pinal {
            let mut kept = next.clone();
            kept.body = None;
            kept.html = None;
            kept.snippet.clear();
            kept.attachments.clear();
            self.readable
                .lock()
                .unwrap()
                .insert(next.id.clone(), next.clone());
            Ok((next, kept))
        } else {
            Ok((next.clone(), next))
        }
    }
    pub fn change(&self, a: &Account, m: &Mail, change: Change) -> Result<Mail> {
        self.change_many(a, std::slice::from_ref(m), change.clone())?;
        let mut next = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
        if change.moves() {
            change.apply(&mut next);
        }
        Ok(next)
    }
    /// Applies one change to several messages, with one IMAP command per mailbox.
    /// Whether the account's folders live on a server that can create,
    /// rename and delete them: any IMAP server (CREATE, RENAME and DELETE
    /// are part of IMAP4rev1), but not POP3, which has only the inbox.
    pub fn can_manage_folders(a: &Account) -> bool {
        a.provider != Provider::Pop3
    }
    /// Makes a folder called `leaf` on the server, inside `parent` (one of
    /// the account's own folders) or at the top level. Whether this
    /// sign-in may is the server's to say.
    pub fn create_remote_folder(
        &self,
        a: &Account,
        parent: Option<&str>,
        leaf: &str,
    ) -> Result<FolderChange> {
        ensure!(
            Self::can_manage_folders(a),
            "This account's folders aren't kept on the server."
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let change = self.with_imap(a, |c| {
            let names = c.names(true)?.to_vec();
            let namespace = c.namespace()?;
            let name = new_folder_name(&names, a.provider, parent, leaf, namespace.as_deref())?;
            if let Some(parent) = parent {
                c.require_rights(parent, "k", "make folders")?;
            }
            let label = decode_name(&name);
            allowed(
                c.session
                    .run_command_and_check_ok(format!("CREATE {}", quote(&name)?)),
                || format!("create “{label}”"),
            )?;
            subscribe(&mut c.session, &name)?;
            Ok(FolderChange {
                folders: custom_folders(c.names(true)?),
                moved: vec![],
            })
        })?;
        self.store.save_folders(&a.id, &change.folders)?;
        Ok(change)
    }
    /// Renames one of the account's own folders on the server to `leaf`,
    /// keeping its place; the folders inside it move with it. Their
    /// cached mail is forgotten, since its identity names the old folder,
    /// and syncs again from the new one.
    pub fn rename_remote_folder(
        &self,
        a: &Account,
        name: &str,
        leaf: &str,
    ) -> Result<FolderChange> {
        ensure!(
            Self::can_manage_folders(a),
            "This account's folders aren't kept on the server."
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let change = self.with_imap(a, |c| {
            let names = c.names(true)?.to_vec();
            let namespace = c.namespace()?;
            let next = renamed_folder_name(&names, a.provider, name, leaf, namespace.as_deref())?;
            if next != name {
                c.require_rights(name, "x", "rename the folder")?;
            }
            if next == name {
                return Ok(FolderChange {
                    folders: custom_folders(&names),
                    moved: vec![],
                });
            }
            let moved: Vec<(String, String)> = folder_and_inside(&names, name)
                .into_iter()
                .map(|old| {
                    let new = format!("{next}{}", &old[name.len()..]);
                    (old, new)
                })
                .collect();
            for (old, _) in &moved {
                c.leave(old)?;
            }
            let label = decode_name(name);
            allowed(
                c.session.run_command_and_check_ok(format!(
                    "RENAME {} {}",
                    quote(name)?,
                    quote(&next)?
                )),
                || format!("rename “{label}”"),
            )?;
            for (_, new) in &moved {
                subscribe(&mut c.session, new)?;
            }
            Ok(FolderChange {
                folders: custom_folders(c.names(true)?),
                moved,
            })
        })?;
        self.store.save_folders(&a.id, &change.folders)?;
        for (old, _) in &change.moved {
            self.store.forget_mailbox(&a.id, old)?;
        }
        Ok(change)
    }
    /// Deletes one of the account's own folders on the server, then
    /// forgets its cached mail. Whether this sign-in may is the server's to
    /// say: DELETE is refused (NO) without the right, changing nothing.
    /// Returns the folders left.
    pub fn delete_remote_folder(&self, a: &Account, name: &str) -> Result<Vec<RemoteFolder>> {
        ensure!(
            Self::can_manage_folders(a),
            "This account's folders aren't kept on the server."
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let folders = self.with_imap(a, |c| {
            let names = c.names(true)?.to_vec();
            deletable(&names, a.provider, name)?;
            c.require_rights(name, "x", "delete the folder")?;
            c.leave(name)?;
            let label = decode_name(name);
            allowed(c.session.delete(name), || format!("delete “{label}”"))?;
            Ok(custom_folders(c.names(true)?))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        self.store.forget_mailbox(&a.id, name)?;
        Ok(folders)
    }
    /// Deletes messages for good: on the server (marked \\Deleted and
    /// expunged, just these) and from this computer. Offered in Trash and
    /// Spam, where Gmail too deletes rather than removes a label.
    pub fn delete_forever(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        ensure!(
            mails.iter().all(|m| m.account == a.id),
            "Message belongs to another account"
        );
        ensure!(
            a.provider != Provider::Gmail
                || mails
                    .iter()
                    .all(|m| matches!(m.folder, Folder::Trash | Folder::Junk)),
            "Gmail deletes mail for good only from Trash and Spam. Move it to Trash first."
        );
        if mails.is_empty() {
            return Ok(());
        }
        if a.provider == Provider::Pop3 {
            // Copies stay on the server (Picacho never sends DELE); remember
            // them so they don't download again.
            let mut erased = self.pop_erased(&a.id);
            for m in mails {
                if let Some(uidl) = m.id.strip_prefix(&format!("{}:pop:", a.id)) {
                    erased.insert(uidl.to_owned());
                }
            }
            self.store.set_preference(
                &format!("pop_erased:{}", a.id),
                &serde_json::to_string(&erased)?,
            )?;
            return self.store.commit_changes(mails, true);
        }
        self.with_imap(a, |c| {
            let mut groups: Vec<(&Mail, Vec<u32>)> = Vec::new();
            for m in mails {
                match groups.iter_mut().find(|(first, _)| {
                    first.remote_folder == m.remote_folder && same_validity(first, m)
                }) {
                    Some((_, uids)) => uids.push(m.uid),
                    None => groups.push((m, vec![m.uid])),
                }
            }
            for (first, uids) in groups {
                let selected = c.select_for(first)?;
                c.require_rights(&first.remote_folder, "te", "delete messages")?;
                for chunk in uids.chunks(200) {
                    c.erase(chunk)?;
                    if let Some(mut state) = self.store.mailbox_state(&a.id, &first.remote_folder)?
                        && state.validity == selected.validity
                    {
                        state.exists = state.exists.saturating_sub(chunk.len() as u32);
                        self.store
                            .save_mailbox_state(&a.id, &first.remote_folder, Some(&state))?;
                    }
                    self.extend_layout(
                        a,
                        &first.mailbox(),
                        selected.validity,
                        std::iter::empty(),
                        chunk,
                    );
                    let gone: Vec<Mail> = mails
                        .iter()
                        .filter(|m| {
                            m.remote_folder == first.remote_folder
                                && same_validity(m, first)
                                && chunk.contains(&m.uid)
                        })
                        .cloned()
                        .collect();
                    self.store.commit_changes(&gone, true)?;
                }
            }
            Ok(())
        })
    }
    /// Deletes everything in Trash or Spam for good. Returns how many
    /// messages were removed.
    pub fn empty_mailbox(&self, a: &Account, folder: Folder) -> Result<usize> {
        ensure!(
            matches!(folder, Folder::Trash | Folder::Junk),
            "Only Trash and Spam can be emptied."
        );
        if a.provider == Provider::Pop3 {
            let mails = self.store.messages(&a.id, folder, usize::MAX >> 1)?;
            self.delete_forever(a, &mails)?;
            return Ok(mails.len());
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_imap(a, |c| {
            let remote = resolve_in(c.names(false)?, a.provider, folder)?;
            c.select(&remote)?;
            c.require_rights(&remote, "te", "delete messages")?;
            let uids: Vec<u32> = c.session.uid_search("ALL")?.into_iter().collect();
            for chunk in uids.chunks(500) {
                c.erase(chunk)?;
            }
            // The folder is empty; its cached mail and state go too.
            self.store.forget_mailbox(&a.id, &remote)?;
            self.forget_layout(a, &mailbox);
            Ok(uids.len())
        })?;
        Ok(count)
    }
    /// UIDLs of POP3 mail deleted here for good, which stays on the server.
    fn pop_erased(&self, account: &str) -> std::collections::BTreeSet<String> {
        self.store
            .preference(&format!("pop_erased:{account}"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// The account's storage use and limit, where the server has quotas.
    pub fn quota(&self, a: &Account) -> Result<Option<Quota>> {
        if a.provider == Provider::Pop3 {
            return Ok(None);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            if !c.extensions()?.quota {
                return Ok(None);
            }
            quota(&mut c.session)
        })
    }
    pub fn change_many(&self, a: &Account, mails: &[Mail], change: Change) -> Result<()> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mut current = Vec::new();
        for m in mails {
            ensure!(m.account == a.id, "Message belongs to another account");
            let next = self.store.message(&m.id)?.unwrap_or_else(|| m.clone());
            if !change.moved_already(&next) {
                current.push(next);
            }
        }
        if current.is_empty() {
            return Ok(());
        }
        ensure!(
            !matches!(change, Change::MoveTo(_)) || Self::can_manage_folders(a),
            "This account's folders aren't kept on the server."
        );
        if a.provider != Provider::Pop3 {
            return self.with_imap(a, |c| self.change_remote(c, a, &current, change));
        }
        self.commit_change(&current, change, false)
    }
    fn change_remote(
        &self,
        c: &mut Connection,
        a: &Account,
        current: &[Mail],
        change: Change,
    ) -> Result<()> {
        let target = match change {
            Change::Move(folder) => Some(resolve_in(c.names(false)?, a.provider, folder)?),
            Change::MoveTo(ref name) => {
                // Listed afresh: the folder may have gone since the last sync.
                let names = c.names(true)?;
                ensure!(
                    custom_folders(names).iter().any(|f| &f.name == name),
                    "The folder “{}” is no longer on the server.",
                    decode_name(name)
                );
                Some(name.clone())
            }
            _ => None,
        };
        if let Some(target) = &target {
            c.require_rights(target, "i", "put messages")?;
        }
        let gmail = c.extensions()?.gmail;
        let label = match &change {
            Change::Label(label, _) if gmail => Some(gmail_label(label)?),
            Change::Label(label, _) => Some(keyword(label)?),
            _ => None,
        };
        let mut groups: Vec<(&Mail, Vec<u32>)> = Vec::new();
        for m in current {
            match groups.iter_mut().find(|(first, _)| {
                first.remote_folder == m.remote_folder && same_validity(first, m)
            }) {
                Some((_, uids)) => uids.push(m.uid),
                None => groups.push((m, vec![m.uid])),
            }
        }
        for (first, uids) in groups {
            let selected = c.select_for(first)?;
            if target.is_some() {
                c.require_rights(&first.remote_folder, "te", "remove messages")?;
            }
            if matches!(change, Change::Label(_, true)) && !gmail {
                // Servers say in SELECT whether new keywords stick.
                ensure!(
                    selected.keywords,
                    "This server doesn't allow new labels in “{}”.",
                    decode_name(&first.remote_folder)
                );
            }
            if label.is_some() {
                c.require_rights(&first.remote_folder, "w", "change labels")?;
            }
            for chunk in uids.chunks(200) {
                let set = uid_set(chunk);
                match change {
                    Change::Read(read) => {
                        c.session.uid_store(
                            &set,
                            if read {
                                "+FLAGS.SILENT (\\Seen)"
                            } else {
                                "-FLAGS.SILENT (\\Seen)"
                            },
                        )?;
                    }
                    Change::Star(star) => {
                        c.session.uid_store(
                            &set,
                            if star {
                                "+FLAGS.SILENT (\\Flagged)"
                            } else {
                                "-FLAGS.SILENT (\\Flagged)"
                            },
                        )?;
                    }
                    Change::Label(_, add) => {
                        let label = label.as_deref().unwrap_or_default();
                        let sign = if add { '+' } else { '-' };
                        if gmail {
                            c.session.run_command_and_check_ok(format!(
                                "UID STORE {set} {sign}X-GM-LABELS ({label})"
                            ))?;
                        } else {
                            c.session
                                .uid_store(&set, format!("{sign}FLAGS.SILENT ({label})"))?;
                        }
                    }
                    Change::Move(_) | Change::MoveTo(_) => {
                        let target = quote(target.as_deref().unwrap_or_default())?;
                        if c.extensions()?.moves {
                            // Atomic, and leaves other deleted messages alone.
                            c.session
                                .run_command_and_check_ok(format!("UID MOVE {set} {target}"))?;
                        } else {
                            // RFC 6851's fallback: copy, then remove just these
                            // (refused up front where that can't be done).
                            c.can_erase(chunk)?;
                            c.session
                                .run_command_and_check_ok(format!("UID COPY {set} {target}"))?;
                            c.erase(chunk)?;
                        }
                        // The source mailbox now has fewer messages; keep its
                        // saved state in step so the next check stays incremental.
                        if let Some(mut state) =
                            self.store.mailbox_state(&a.id, &first.remote_folder)?
                            && state.validity == selected.validity
                        {
                            state.exists = state.exists.saturating_sub(chunk.len() as u32);
                            self.store.save_mailbox_state(
                                &a.id,
                                &first.remote_folder,
                                Some(&state),
                            )?;
                        }
                        self.extend_layout(
                            a,
                            &first.mailbox(),
                            selected.validity,
                            std::iter::empty(),
                            chunk,
                        );
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
                self.commit_change(&confirmed, change.clone(), true)?;
            }
        }
        Ok(())
    }
    fn commit_change(&self, current: &[Mail], change: Change, remote: bool) -> Result<()> {
        let updated: Vec<_> = current
            .iter()
            .cloned()
            .map(|mut next| {
                change.apply(&mut next);
                next
            })
            .collect();
        self.store
            .commit_changes(&updated, remote && change.moves())
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
            self.with_imap(a, |c| {
                let remote = remote_name(c.names(false)?, a, mailbox)?;
                c.selected(&remote)?;
                let unseen: Vec<u32> = c.session.uid_search("UNSEEN")?.into_iter().collect();
                if !unseen.is_empty() {
                    c.session
                        .uid_store(uid_ranges(&unseen), "+FLAGS.SILENT (\\Seen)")?;
                }
                Ok(())
            })?;
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
        // Only mail inside the account's offline range downloads ahead of
        // time; the rest downloads when it is opened.
        let Some(since) = a.sync.offline_since(chrono::Utc::now().timestamp()) else {
            return Ok(0);
        };
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.ensure_background(a)?;
        // A Gmail message appears once per label; download it only once.
        let shared = if a.provider == Provider::Gmail {
            self.store.share_gmail_raw(&a.id)?
        } else {
            0
        };
        let mut budget = 12 * 1024 * 1024usize;
        let mut pending: Vec<Mail> = Vec::new();
        for m in self.store.missing(&a.id, MAX_MESSAGE, max, since)? {
            if !pending.is_empty() && m.size as usize > budget {
                break;
            }
            if m.gmail_id != 0 && pending.iter().any(|p| p.gmail_id == m.gmail_id) {
                continue;
            }
            budget = budget.saturating_sub(m.size as usize);
            pending.push(m);
        }
        if pending.is_empty() {
            return Ok(shared);
        }
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
        self.with_imap(a, |c| {
            let mut stored = shared;
            for group in groups {
                c.select_for(group[0]).context(
                    "Could not download messages from their mailbox. Refresh that mailbox first.",
                )?;
                let uids: Vec<u32> = group.iter().map(|m| m.uid).collect();
                for chunk in uids.chunks(10) {
                    // Someone is opening a message: let them have the
                    // connection; the download picks up after.
                    if stored > shared && self.someone_waiting(&a.id) {
                        return Ok(stored);
                    }
                    for f in c
                        .session
                        .uid_fetch(uid_set(chunk), "(UID BODY.PEEK[])")?
                        .iter()
                    {
                        let (Some(uid), Some(raw)) = (f.uid, f.body()) else {
                            continue;
                        };
                        let Some(m) = group.iter().find(|m| m.uid == uid) else {
                            continue;
                        };
                        self.note_traffic(a, raw.len());
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
            Ok(stored)
        })
    }
    /// Works out how to unsubscribe from `sub`: from the newest of its
    /// recent messages whose List-Unsubscribe its sender signed, so a forged
    /// message "from" the sender can't redirect it; failing that, from the
    /// latest, without one click. Looks the signatures' keys up in DNS, and
    /// may download a message's source.
    pub fn unsubscribe_plan(
        &self,
        a: &Account,
        sub: &crate::manage::subscriptions::Subscription,
    ) -> Result<crate::manage::subscriptions::Plan> {
        use crate::manage::subscriptions::Plan;
        let messages = self.store.subscription_messages(&a.id, &sub.address, 5)?;
        for m in &messages {
            let Ok(raw) = self.original(a, m) else {
                continue;
            };
            if let Some(plan) = crate::sync::auth::signed_unsubscribe(&raw)
                .and_then(|signed| Plan::new(Some(&signed), ""))
            {
                return Ok(plan);
            }
        }
        let latest = messages.first().map_or(sub.unsubscribe.as_str(), |m| m.list_unsubscribe.as_str());
        Plan::new(None, latest).context("This sender offers no way to unsubscribe.")
    }
    /// Carries out a plan from `unsubscribe_plan`: one click, or a message
    /// sent from `a`, or the page to open.
    pub fn unsubscribe(
        &self,
        a: &Account,
        plan: &crate::manage::subscriptions::Plan,
    ) -> Result<crate::manage::subscriptions::Outcome> {
        crate::manage::subscriptions::unsubscribe(plan, &a.id, |d| self.send(a, d))
    }
    pub fn send(&self, a: &Account, d: &Draft) -> Result<()> {
        self.send_noting(a, d).map(|_| ())
    }
    /// Sends a draft. Once the server accepts it, it is sent: anything that
    /// then didn't work (a copy in Sent, receipts) is a note, not an error.
    pub fn send_noting(&self, a: &Account, d: &Draft) -> Result<Vec<String>> {
        // Sending as an alias asks its service for the routed addresses
        // first, outside the account's lock, once it knows the alias is
        // still there.
        if d.send_as.trim().is_empty() {
            let forwarding = crate::manage::forwarding::load(&self.store);
            let accounts = self.store.accounts()?;
            ensure!(
                !crate::manage::forwarding::requires_alias(&forwarding, &accounts, &a.id),
                "{} sends only as its aliases (Settings → Forwarding). Choose an alias in From.",
                a.email
            );
        }
        crate::manage::forwarding::verify_alias(&self.store, d)?;
        let routed = crate::manage::forwarding::route(&self.store, d)?;
        let d = &routed;
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
        let delivered = crate::sync::smtp::deliver(a, d, &secret, tls)?;
        let message = delivered.message;
        let mut notes = Vec::new();
        if delivered.receipts_unsupported {
            notes.push(format!(
                "{} can't request delivery receipts, so none were asked for.",
                a.smtp_host
            ));
        }
        // Gmail and Outlook save sent messages themselves. Custom IMAP servers do not.
        if a.provider == Provider::Imap {
            let raw = message.formatted();
            let saved = self.with_imap(a, |c| {
                if let Some(limit) = c.extensions()?.append_limit {
                    ensure!(
                        raw.len() as u64 <= limit,
                        "it's larger than the server keeps ({} KB)",
                        limit / 1024
                    );
                }
                let sent = resolve_in(c.names(false)?, a.provider, Folder::Sent)?;
                c.session
                    .append_with_flags(sent, &raw, &[imap::types::Flag::Seen])?;
                Ok(())
            });
            if let Err(e) = saved {
                notes.push(format!("A copy wasn't saved in Sent: {e:#}"));
            }
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
        let _ = self.discard_draft(&d.id);
        Ok(notes)
    }
    /// Deletes a draft here, and its copy on the server at the next draft
    /// sync (it may have been uploaded since a composer last saved it).
    pub fn discard_draft(&self, id: &str) -> Result<()> {
        if let Some(d) = self.store.draft(id)? {
            if let Some(remote) = d.remote {
                let mut pending = self.draft_removals(&d.account);
                if !pending.contains(&remote) {
                    pending.push(remote);
                }
                self.save_draft_removals(&d.account, &pending)?;
            }
            // Files taken from a server draft were kept for it alone.
            let _ = std::fs::remove_dir_all(self.draft_files(id));
        }
        self.store.delete_draft(id)
    }
    fn draft_removals(&self, account: &str) -> Vec<RemoteDraft> {
        self.store
            .preference(&format!("draft_removals:{account}"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save_draft_removals(&self, account: &str, pending: &[RemoteDraft]) -> Result<()> {
        self.store.set_preference(
            &format!("draft_removals:{account}"),
            &serde_json::to_string(pending)?,
        )
    }
    /// Where the attachments of a draft found on the server are kept.
    fn draft_files(&self, id: &str) -> std::path::PathBuf {
        let safe: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        self.store.path.join("draft-files").join(safe)
    }
    /// Keeps the account's drafts and its server's Drafts folder in step:
    /// drafts written here are uploaded (replacing their earlier copy),
    /// drafts written elsewhere appear here, and drafts sent or deleted on
    /// either side go from the other. A draft changed here since its last
    /// upload is never thrown away. Returns how many drafts changed here.
    pub fn sync_drafts(&self, a: &Account) -> Result<usize> {
        if a.provider == Provider::Pop3 {
            return Ok(0);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            let Ok(mailbox) = resolve_in(c.names(false)?, a.provider, Folder::Drafts) else {
                // No Drafts folder on the server: drafts stay here.
                return Ok(0);
            };
            let selected = c.select(&mailbox)?;
            let here = |r: &RemoteDraft| r.mailbox == mailbox && r.validity == selected.validity;
            // Drafts sent or deleted here: their server copies go.
            let pending = self.draft_removals(&a.id);
            let gone: Vec<u32> = pending.iter().filter(|r| here(r)).map(|r| r.uid).collect();
            c.erase(&gone)?;
            if !pending.is_empty() {
                self.save_draft_removals(&a.id, &[])?;
            }
            // What the server has, and which of Picacho's drafts each is.
            let mut on_server: Vec<u32> = c.session.uid_search("ALL")?.into_iter().collect();
            on_server.sort_unstable();
            let mut tagged: HashMap<u32, String> = HashMap::new();
            if !on_server.is_empty() {
                let items = format!(
                    "(UID BODY.PEEK[HEADER.FIELDS ({})])",
                    crate::sync::smtp::DRAFT_HEADER
                );
                for row in uid_fetch_rows(&mut c.session, &uid_ranges(&on_server), &items)? {
                    let id = row.header.as_deref().and_then(|h| {
                        mailparse::parse_headers(h).ok().and_then(|(headers, _)| {
                            use mailparse::MailHeaderMap;
                            headers.get_first_value(crate::sync::smtp::DRAFT_HEADER)
                        })
                    });
                    if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
                        tagged.insert(row.uid, id.trim().to_owned());
                    }
                }
            }
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            // Copies removed from the server (sent or deleted elsewhere):
            // unchanged drafts go here too; changed ones upload again.
            for d in &mut locals {
                let Some(remote) = d.remote.clone() else {
                    continue;
                };
                if here(&remote) && on_server.contains(&remote.uid) {
                    continue;
                }
                if here(&remote) && d.uploaded == d.fingerprint() {
                    self.store.delete_draft(&d.id)?;
                    let _ = std::fs::remove_dir_all(self.draft_files(&d.id));
                    d.id.clear();
                    changed += 1;
                } else {
                    d.remote = None;
                }
            }
            locals.retain(|d| !d.id.is_empty());
            // Drafts written elsewhere.
            for &uid in &on_server {
                let claimed = locals.iter().any(|d| {
                    d.remote.as_ref().is_some_and(|r| here(r) && r.uid == uid)
                        || tagged.get(&uid) == Some(&d.id)
                });
                if claimed {
                    continue;
                }
                let rows = uid_fetch_rows(&mut c.session, &uid.to_string(), "(UID RFC822.SIZE BODY.PEEK[])")?;
                let Some(raw) = rows.into_iter().find(|r| r.uid == uid).and_then(|r| r.header) else {
                    continue;
                };
                ensure!(raw.len() <= MAX_MESSAGE, "A draft on the server is too large to open here.");
                let id = tagged.get(&uid).cloned().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let mut d = self.draft_from(a, &id, &raw)?;
                d.remote = Some(RemoteDraft {
                    mailbox: mailbox.clone(),
                    validity: selected.validity,
                    uid,
                });
                d.uploaded = d.fingerprint();
                self.store.save_draft(&d)?;
                locals.push(d);
                changed += 1;
            }
            // Drafts written or changed here.
            let limit = c.extensions()?.append_limit;
            for d in &locals {
                let fingerprint = d.fingerprint();
                if d.remote.is_some() && d.uploaded == fingerprint {
                    continue;
                }
                let raw = crate::sync::smtp::build(a, d, true)?.formatted();
                if limit.is_some_and(|l| raw.len() as u64 > l) {
                    // Too large for the server; it stays here.
                    continue;
                }
                let before: std::collections::HashSet<u32> =
                    c.session.uid_search("ALL")?.into_iter().collect();
                c.session.append_with_flags(
                    &mailbox,
                    &raw,
                    &[imap::types::Flag::Seen, imap::types::Flag::Draft],
                )?;
                let after: std::collections::HashSet<u32> =
                    c.session.uid_search("ALL")?.into_iter().collect();
                let Some(uid) = after.difference(&before).max().copied() else {
                    bail!("The server didn't keep the uploaded draft.");
                };
                // Earlier copies of this draft, by its id or where it was.
                let old: Vec<u32> = after
                    .iter()
                    .copied()
                    .filter(|u| {
                        *u != uid
                            && (tagged.get(u) == Some(&d.id)
                                || d.remote.as_ref().is_some_and(|r| here(r) && r.uid == *u))
                    })
                    .collect();
                c.erase(&old)?;
                // A composer may have saved a newer version meanwhile; it
                // keeps its words, and uploads again next time.
                let mut current = self.store.draft(&d.id)?.unwrap_or_else(|| d.clone());
                current.remote = Some(RemoteDraft {
                    mailbox: mailbox.clone(),
                    validity: selected.validity,
                    uid,
                });
                current.uploaded = fingerprint;
                self.store.save_draft(&current)?;
            }
            Ok(changed)
        })
    }
    /// A draft from a message in the server's Drafts folder.
    fn draft_from(&self, a: &Account, id: &str, raw: &[u8]) -> Result<Draft> {
        use mailparse::MailHeaderMap;
        let parsed = mailparse::parse_mail(raw)?;
        let header = |name: &str| parsed.headers.get_first_value(name).unwrap_or_default();
        let m = parse_message(raw, a, id, Folder::Drafts, "", 0)?;
        let mut d = Draft {
            id: id.into(),
            account: a.id.clone(),
            to: header("To"),
            cc: header("Cc"),
            bcc: header("Bcc"),
            subject: m.subject.clone(),
            body: m.body.clone().unwrap_or_default(),
            formatted: header(crate::sync::smtp::FORMAT_HEADER).trim() == "markdown",
            reply_id: header("In-Reply-To").trim().to_owned(),
            references: header("References").trim().to_owned(),
            reply_to: header("Reply-To").trim().to_owned(),
            ..Draft::default()
        };
        if !m.attachments.is_empty() {
            let dir = self.draft_files(id);
            std::fs::create_dir_all(&dir)?;
            let mut leaves = Vec::new();
            collect_leaves(&parsed, &mut leaves);
            for (i, attachment) in m.attachments.iter().enumerate() {
                let Some(leaf) = leaves.get(attachment.part) else {
                    continue;
                };
                let name: String = std::path::Path::new(&attachment.name)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty() && n != "." && n != "..")
                    .unwrap_or_else(|| format!("attachment-{}", i + 1));
                let path = dir.join(name);
                std::fs::write(&path, leaf.get_body_raw()?)?;
                d.attachments.push(path);
            }
        }
        Ok(d)
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

/// What a flags-only FETCH asks for: with Gmail, its labels too.
fn flag_items(gmail: bool) -> &'static str {
    if gmail {
        "(UID FLAGS X-GM-LABELS)"
    } else {
        "(UID FLAGS)"
    }
}
/// UIDs of the selected mailbox's messages within the account's sync
/// window (only flagged ones for Starred), oldest first.
fn search(session: &mut ImapSession, a: &Account, starred_only: bool) -> Result<Vec<u32>> {
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
    Ok(uids)
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

/// One mailbox generation (ids starting with `prefix`) newest-arrived
/// first by UID; anything else (not yet reconciled) after it, newest first.
fn sort_arrival(messages: &mut [Mail], prefix: &str) {
    messages.sort_by(|a, b| {
        let (ca, cb) = (a.id.starts_with(prefix), b.id.starts_with(prefix));
        cb.cmp(&ca).then_with(|| {
            if ca && cb {
                b.uid.cmp(&a.uid)
            } else {
                Mail::newest_first(a, b)
            }
        })
    });
}
