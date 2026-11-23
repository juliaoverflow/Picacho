//! Account operations serialize protocol work and commit results to the local cache.
use crate::manage::status::{AccessFailure, SignInStep};
use crate::manage::{
    model::*,
    store::{self, Store},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use zeroize::Zeroize;

use crate::sync::transport::*;
#[path = "connection.rs"]
mod connection;
use connection::{Connection, Limits};
#[path = "watch.rs"]
mod watch;
use watch::{ProtonNotice, Watcher};
#[path = "offline.rs"]
mod offline;
#[path = "snooze.rs"]
mod snooze;
pub use snooze::Snoozed;
#[path = "undo.rs"]
mod undo;
pub use undo::Moved;
#[path = "remote_search.rs"]
mod remote_search;
pub use remote_search::ServerResults;
#[path = "large.rs"]
mod large;
#[path = "extras.rs"]
mod extras;
#[path = "archive.rs"]
pub mod archive;
#[path = "contacts.rs"]
pub mod contacts;
#[path = "calendar.rs"]
pub mod calendar;
#[path = "proton.rs"]
mod proton;
#[path = "proton_push.rs"]
mod proton_push;
pub use proton_push::ProtonPushed;
#[path = "jmap.rs"]
mod jmap;
pub use jmap::HeldSubmission;
pub use crate::sync::fido::{Method as KeyMethod, NeedsPin, Prompt as KeyPrompt};
pub use proton::ProtonNext;
pub use extras::ServerNotes;
#[cfg(test)]
#[path = "dovecot_tests.rs"]
mod dovecot_tests;
#[cfg(test)]
#[path = "incremental_tests.rs"]
mod incremental_tests;
#[cfg(test)]
#[path = "pinal_tests.rs"]
mod pinal_tests;
#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
/// What signs a POP3 account in: its access token with single sign-on,
/// else its password (empty with Kerberos).
fn pop_secret<'a>(a: &Account, s: &'a Secret) -> &'a str {
    if a.oauth { &s.access_token } else { &s.password }
}
/// A loopback TLS certificate and acceptor, for tests elsewhere.
#[cfg(test)]
pub(crate) fn protocol_tests_tls() -> Result<(Vec<u8>, native_tls::TlsAcceptor)> {
    protocol_tests::tls_fixture()
}
#[cfg(test)]
#[path = "extension_tests.rs"]
mod extension_tests;
#[cfg(test)]
#[path = "reliability_tests.rs"]
mod reliability_tests;
#[cfg(test)]
#[path = "jmap_tests.rs"]
mod jmap_tests;
#[cfg(all(test, unix, not(any(target_os = "android", target_os = "ios"))))]
#[path = "kerberos_tests.rs"]
mod kerberos_tests;
#[cfg(test)]
#[path = "sso_tests.rs"]
mod sso_tests;
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Change {
    Read(bool),
    Star(bool),
    Move(Folder),
    /// Moves mail into one of the account's own server folders, by name.
    MoveTo(String),
    /// Adds (true) or removes a label: a Gmail label or an IMAP keyword.
    Label(String, bool),
    /// Marks what was done with it, for this and other mail apps.
    Mark(Mark),
}
/// What was done with a message, kept as flags on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mark {
    /// Replied to: \\Answered.
    Answered,
    /// Forwarded: $Forwarded (RFC 5550).
    Forwarded,
    /// A read receipt was sent, or declined: $MDNSent (RFC 3503).
    ReceiptHandled,
}
impl Mark {
    /// The flag on the server, and whether it is a keyword (which servers
    /// keep only where PERMANENTFLAGS allows new ones).
    fn flag(self) -> (&'static str, bool) {
        match self {
            Self::Answered => ("\\Answered", false),
            Self::Forwarded => ("$Forwarded", true),
            Self::ReceiptHandled => ("$MDNSent", true),
        }
    }
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
                let mut labels: Vec<String> =
                    m.labels().iter().filter(|l| *l != label).cloned().collect();
                if *add {
                    labels.push(label.clone());
                    labels.sort();
                }
                m.set_labels(labels);
            }
            Self::Mark(Mark::Answered) => m.answered = true,
            Self::Mark(Mark::Forwarded) => m.forwarded = true,
            Self::Mark(Mark::ReceiptHandled) => m.mdn_sent = true,
        }
    }
    /// Whether `m` already shows this change (so it needn't be sent again).
    pub fn shown_by(&self, m: &Mail) -> bool {
        match self {
            Self::Read(read) => m.unread != *read,
            Self::Star(star) => m.starred == *star,
            Self::Label(label, add) => m.labels().contains(label) == *add,
            Self::Mark(Mark::Answered) => m.answered,
            Self::Mark(Mark::Forwarded) => m.forwarded,
            Self::Mark(Mark::ReceiptHandled) => m.mdn_sent,
            Self::Move(_) | Self::MoveTo(_) => self.moved_already(m),
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
fn subscribe(c: &mut Connection, name: &str) -> Result<()> {
    let wire = c.mailbox(name)?;
    match c.session.run_command_and_check_ok(format!("SUBSCRIBE {wire}")) {
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

/// The saved text of the calendar's events, and the events read from it.
type CalendarCache = (Arc<str>, Arc<Vec<crate::sync::caldav::Stored>>);

pub struct Backend {
    pub store: Arc<Store>,
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    credentials: Mutex<HashMap<String, Result<Secret, String>>>,
    /// Opened Pinal mail keys by account, held in memory only.
    mail_keys: Mutex<HashMap<String, Arc<Vec<pgp::composed::SignedSecretKey>>>>,
    /// Decrypted Pinal messages by id. Memory only: the cache on disk keeps
    /// them encrypted, so reloads from it get their bodies back from here.
    readable: Mutex<HashMap<String, Mail>>,
    /// The messages of `readable` that keep their decrypted text, the most
    /// recently read last (see `READABLE_BODIES`).
    readable_order: Mutex<std::collections::VecDeque<String>>,
    /// Your OpenPGP keyring (`openpgp.rs`), read from the keychain once.
    keyring: Mutex<Option<Arc<crate::manage::openpgp::Keyring>>>,
    /// What OpenPGP found in messages read with it, by id: decrypted, and
    /// whose signature. Memory only.
    verdicts: Mutex<HashMap<String, crate::manage::openpgp::Verdict>>,
    /// Your S/MIME certificates (`smime.rs`), read from the keychain once.
    certificates: Mutex<Option<Arc<crate::manage::smime::Certificates>>>,
    /// What S/MIME found in messages read with it, by id. Memory only.
    smime_verdicts: Mutex<HashMap<String, crate::manage::smime::Verdict>>,
    /// The calendar's events as last read, with the saved text they were
    /// read from: the calendar and invitations ask for them as they draw,
    /// and every event was parsed again for each.
    calendar_cache: Mutex<Option<CalendarCache>>,
    /// Signed-in IMAP connections by account; see `connection.rs`.
    connections: Mutex<HashMap<String, Connection>>,
    limits: Mutex<Limits>,
    /// Mailbox layouts by (account, mailbox key), from the latest search.
    layouts: Mutex<HashMap<(String, String), Layout>>,
    /// Each account's layout revision, moved on whenever one of its
    /// layouts changes; see `list_revision`.
    layout_revisions: Mutex<HashMap<String, u64>>,
    layout_changes: AtomicU64,
    /// A sign-in that failed after the browser, kept for one retry.
    pending: Mutex<Option<Pending>>,
    /// How far the current sign-in has come; see `sign_in_step`.
    sign_in_step: Mutex<SignInStep>,
    /// Messages being opened that wait for their account's connection, by
    /// account. Background downloads give way to them.
    waiting: Mutex<HashMap<String, usize>>,
    /// Inboxes watched for new mail with IDLE, by account; see `watch.rs`.
    watchers: Mutex<HashMap<String, Watcher>>,
    /// The mailbox open in the window, watched by account where NOTIFY
    /// doesn't cover it; see `watch_folder`.
    folder_watchers: Mutex<HashMap<String, Watcher>>,
    /// Signed-in Proton sessions (keys unlocked) by account, in memory
    /// only; see `proton.rs`.
    proton: Mutex<HashMap<String, Arc<crate::sync::proton::Session>>>,
    /// Held while an account's Proton session is resumed, so two threads
    /// never resume it at once: each would refresh the same refresh token,
    /// and Proton replaces it at the first refresh.
    proton_opening: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// A Proton sign-in between its steps (password, second factor,
    /// mailbox password); see `proton_begin`.
    proton_login: Mutex<Option<proton::ProtonLogin>>,
    /// What a security key or phone is waiting for during a sign-in.
    proton_prompt: Arc<Mutex<KeyPrompt>>,
    /// Called with Proton's account notices; see `on_proton_notice`.
    proton_notice: Mutex<Option<ProtonNotice>>,
    /// Called with Proton's pushes; see `on_proton_push`.
    proton_push: Mutex<Option<proton_push::ProtonPushHear>>,
    /// Why accounts' push notifications aren't registered; see
    /// `proton_push_failures`.
    proton_push_failures: Mutex<HashMap<String, String>>,
    /// Called when those change; see `on_proton_push_change`.
    proton_push_change: Mutex<Option<proton_push::ProtonPushChange>>,
    /// Signed-in JMAP sessions by account, in memory only, with when their
    /// access token expires (0 for a password); see `jmap.rs`.
    jmap: Mutex<HashMap<String, (Arc<crate::sync::jmap::Session>, i64)>>,
    /// Scheduled drafts being sent now, by id, by the window or the
    /// background monitor; see `send_scheduled`.
    scheduled_sending: Mutex<HashSet<String>>,
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
            readable_order: Mutex::default(),
            keyring: Mutex::new(None),
            verdicts: Mutex::new(HashMap::new()),
            certificates: Mutex::new(None),
            smime_verdicts: Mutex::new(HashMap::new()),
            calendar_cache: Mutex::new(None),
            connections: Mutex::new(HashMap::new()),
            limits: Mutex::new(Limits::default()),
            layouts: Mutex::new(HashMap::new()),
            layout_revisions: Mutex::new(HashMap::new()),
            layout_changes: AtomicU64::new(0),
            pending: Mutex::new(None),
            sign_in_step: Mutex::new(SignInStep::Browser),
            waiting: Mutex::new(HashMap::new()),
            watchers: Mutex::new(HashMap::new()),
            folder_watchers: Mutex::new(HashMap::new()),
            proton: Mutex::new(HashMap::new()),
            proton_opening: Mutex::new(HashMap::new()),
            proton_login: Mutex::new(None),
            proton_prompt: Arc::new(Mutex::new(KeyPrompt::Idle)),
            proton_notice: Mutex::new(None),
            proton_push: Mutex::new(None),
            proton_push_failures: Mutex::new(HashMap::new()),
            proton_push_change: Mutex::new(None),
            jmap: Mutex::new(HashMap::new()),
            scheduled_sending: Mutex::new(HashSet::new()),
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
        // Built once: each build loads the system's root certificates again
        // (milliseconds of CPU, for every connection and retry). TLS 1.2 at
        // least: native-tls would otherwise accept 1.0 and 1.1, which no
        // mail server needs and RFC 8996 retired.
        static CONNECTOR: std::sync::OnceLock<native_tls::TlsConnector> =
            std::sync::OnceLock::new();
        if let Some(connector) = CONNECTOR.get() {
            return Ok(connector.clone());
        }
        let connector = native_tls::TlsConnector::builder()
            .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
            .build()?;
        Ok(CONNECTOR.get_or_init(|| connector).clone())
    }
    /// The TLS settings network lookups outside an account use.
    pub fn tls_connector_pub(&self) -> Result<native_tls::TlsConnector> {
        self.tls_connector()
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
        // Proton signs in in steps, which `proton_begin` starts.
        ensure!(
            account.provider != Provider::Proton,
            "Sign in to Proton with its own steps."
        );
        if account.service == crate::manage::model::Service::Pinal {
            let (secret, who) = crate::sync::oauth::pinal(&account.email, cancel.clone(), &|| {
                self.step(&cancel, SignInStep::Finishing)
            })?;
            ensure!(
                account.email.is_empty() || account.email.eq_ignore_ascii_case(&who.email),
                "Choose the same email address to reconnect this mailbox."
            );
            let issued = crate::sync::oauth::pinal_account(&who.email, &who.name);
            // Reconnecting keeps the account's place, settings and names.
            account = Account {
                id: account.id.clone(),
                sync: account.sync,
                nickname: account.nickname.clone(),
                identities: account.identities.clone(),
                reply_to: account.reply_to.clone(),
                read_receipts: account.read_receipts,
                delivery_receipts: account.delivery_receipts,
                name: if account.name.is_empty() || account.name == account.email {
                    issued.name.clone()
                } else {
                    account.name.clone()
                },
                ..issued
            };
            self.verify(account, secret, taken, &cancel, true)
        } else if account.oauth {
            let grant = crate::sync::oauth::authorize(&account, cancel.clone())?;
            self.redeem(account, grant, taken, &cancel)
        } else {
            // Kerberos uses the system's ticket; there is no password.
            ensure!(
                account.kerberos || !password.is_empty(),
                "Enter your password or app password."
            );
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
        let (secret, mut who) = match crate::sync::oauth::redeem(&account, &grant) {
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
        if account.sso.is_some() {
            // The client the organization's sign-in knows Picacho by (given,
            // or registered for this sign-in), for renewing tokens.
            account.client_id = grant.client_id.clone();
        }
        if account.provider == Provider::Jmap {
            // The client the server knows Picacho by, for renewing tokens.
            account.client_id = grant.client_id.clone();
            // The address entered stays the account's (a JMAP sign-in can be
            // a user name, or an address the mailbox has as an alias). With
            // none entered, it is the one the token or the session names.
            if account.email.is_empty() {
                if !who.email.contains('@') {
                    who.email = self.jmap_username(&account, &secret).unwrap_or_default();
                }
                ensure!(
                    who.email.contains('@'),
                    "The server didn't say which address signed in. Enter your email address and try again."
                );
            } else {
                who.email = account.email.clone();
            }
        }
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
            let mut pop = self.pop_login(account, pop_secret(account, secret))?;
            // The mailbox opens, not just the sign-in (STAT, RFC 1939 §5).
            pop.stat()?;
            let _ = pop.command("QUIT");
            return Ok(());
        }
        // A retry can come long after the browser; renew an expiring token.
        crate::sync::oauth::refresh(account, secret)?;
        if account.provider == Provider::Jmap {
            return self.check_jmap(account, secret);
        }
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
        // A JMAP account shared with another's sign-in uses that one's.
        let owner = if a.credentials_of.is_empty() {
            None
        } else {
            Some(
                self.store
                    .accounts()?
                    .into_iter()
                    .find(|o| o.id == a.credentials_of)
                    .context("The account this one was shared with is gone. Remove this one.")?,
            )
        };
        let who = owner.as_ref().unwrap_or(a);
        if who.kerberos {
            // The ticket is the system's; nothing is kept in the keychain
            // (an entry from before Kerberos was chosen is used if present).
            return Ok(self.saved_credentials(&who.id).unwrap_or_default());
        }
        // One renewal at a time per sign-in: an account and those sharing it
        // renew the same token. Per sign-in, not one for all: a slow token
        // server held every account's sign-in, each waiting for its turn.
        static RENEWING: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>> = Mutex::new(None);
        let renewing = RENEWING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .entry(who.id.clone())
            .or_default()
            .clone();
        let _renewing = renewing.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.saved_credentials(&who.id)?;
        if crate::sync::oauth::refresh(who, &mut s)? {
            self.save_credentials(&who.id, &s)?;
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
    /// The password for a sending address's own outgoing server.
    pub(super) fn identity_secret(&self, i: &crate::manage::model::Identity) -> Result<Secret> {
        #[cfg(test)]
        if let Some(secret) = &self.test_secret {
            return Ok(secret.clone());
        }
        self.saved_credentials(&i.secret_id())
    }
    /// Adds or changes one of an account's sending addresses. With its own
    /// outgoing server, signing in there is checked first (with `password`,
    /// or the saved one when it is `None`), and the password is kept in the
    /// keychain. Returns the account as saved.
    pub fn save_identity(
        &self,
        account: &str,
        identity: crate::manage::model::Identity,
        password: Option<String>,
    ) -> Result<Account> {
        identity.validate()?;
        let lock = self.account_lock(account);
        let _operation = lock.lock().unwrap();
        let mut a = self
            .store
            .accounts()?
            .into_iter()
            .find(|a| a.id == account)
            .context("The account was removed.")?;
        ensure!(
            !a.email.eq_ignore_ascii_case(identity.email.trim())
                && !a.identities.iter().any(|i| {
                    i.id != identity.id && i.email.eq_ignore_ascii_case(identity.email.trim())
                }),
            "{} already sends as {}.",
            a.email,
            identity.email.trim()
        );
        if let Some(endpoint) = crate::sync::smtp::Endpoint::of_identity(&identity) {
            let secret = match password.filter(|p| !p.is_empty()) {
                Some(password) => Secret {
                    password,
                    ..Default::default()
                },
                None => self
                    .identity_secret(&identity)
                    .context("Enter the password for its outgoing server.")?,
            };
            crate::sync::smtp::check_server(&endpoint, &secret.password, &self.tls_connector()?)
                .with_context(|| format!("Couldn't sign in to {}", endpoint.host))?;
            #[cfg(test)]
            let saved = self.test_secret.is_some();
            #[cfg(not(test))]
            let saved = false;
            if !saved {
                self.save_credentials(&identity.secret_id(), &secret)?;
            }
        } else {
            self.forget_identity_secret(&identity);
        }
        let identity = crate::manage::model::Identity {
            email: identity.email.trim().to_owned(),
            name: identity.name.trim().to_owned(),
            reply_to: identity.reply_to.trim().to_owned(),
            ..identity
        };
        match a.identities.iter_mut().find(|i| i.id == identity.id) {
            Some(existing) => *existing = identity,
            None => a.identities.push(identity),
        }
        self.store.save_account(&a)?;
        Ok(a)
    }
    /// Removes one of an account's sending addresses, and its password.
    pub fn remove_identity(&self, account: &str, id: &str) -> Result<Account> {
        let lock = self.account_lock(account);
        let _operation = lock.lock().unwrap();
        let mut a = self
            .store
            .accounts()?
            .into_iter()
            .find(|a| a.id == account)
            .context("The account was removed.")?;
        if let Some(i) = a.identity(id).cloned() {
            self.forget_identity_secret(&i);
        }
        a.identities.retain(|i| i.id != id);
        self.store.save_account(&a)?;
        Ok(a)
    }
    fn forget_identity_secret(&self, i: &crate::manage::model::Identity) {
        let id = i.secret_id();
        self.credentials.lock().unwrap().remove(&id);
        #[cfg(test)]
        if self.test_secret.is_some() {
            return;
        }
        let _ = store::secret_entry(&id).and_then(|e| e.delete_credential().map_err(Into::into));
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
        let sharing: Vec<String> = self
            .store
            .accounts()?
            .into_iter()
            .filter(|a| a.credentials_of == id)
            .map(|a| a.label().to_owned())
            .collect();
        ensure!(
            sharing.is_empty(),
            "Remove {} first: {} with this account's sign-in.",
            sharing.join(", "),
            if sharing.len() == 1 { "it was shared" } else { "they were shared" }
        );
        let lock = self.account_lock(id);
        let _operation = lock.lock().unwrap();
        // Its sign-in given up at the provider, so the token kept by it is
        // no use to anyone (RFC 7009); best effort, in the background.
        if let Some(a) = self.store.accounts().ok().and_then(|all| all.into_iter().find(|a| a.id == id))
            && a.oauth
            && a.credentials_of.is_empty()
            && let Ok(secret) = self.credentials(&a)
            && let Some(endpoint) = crate::sync::oauth::provider_revocation(a.provider)
        {
            let client_id = a.client_id.clone();
            std::thread::spawn(move || {
                crate::sync::oauth::revoke(endpoint, &client_id, &secret.refresh_token)
            });
        }
        self.disconnect(id);
        self.stop_watching(id);
        if let Some(a) = self
            .store
            .accounts()
            .ok()
            .and_then(|all| all.into_iter().find(|a| a.id == id))
            .filter(|a| a.provider == Provider::Proton)
        {
            self.forget_proton(&a);
        }
        self.layouts
            .lock()
            .unwrap()
            .retain(|(account, _), _| account != id);
        self.layouts_changed(id);
        let identities: Vec<crate::manage::model::Identity> = self
            .store
            .accounts()
            .ok()
            .and_then(|all| all.into_iter().find(|a| a.id == id))
            .map(|a| a.identities)
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(self.sent_queue(id));
        // Its drafts' own files (attachments, perhaps large) and picture go
        // with their rows; nothing else refers to them.
        let drafts = self.store.drafts(id).unwrap_or_default();
        let avatar = self.avatar(id);
        self.store.remove_account(id)?;
        for d in &drafts {
            let _ = std::fs::remove_dir_all(self.draft_files(&d.id));
        }
        if let Some(avatar) = avatar {
            let _ = std::fs::remove_file(avatar);
        }
        for i in &identities {
            self.forget_identity_secret(i);
        }
        self.credentials.lock().unwrap().remove(id);
        self.mail_keys.lock().unwrap().remove(id);
        self.readable.lock().unwrap().retain(|_, m| m.account != id);
        // Message ids begin with their account's.
        let mine = format!("{id}:");
        self.readable_order.lock().unwrap().retain(|m| !m.starts_with(&mine));
        self.verdicts.lock().unwrap().retain(|m, _| !m.starts_with(&mine));
        self.smime_verdicts.lock().unwrap().retain(|m, _| !m.starts_with(&mine));
        // A missing/inaccessible keychain must not prevent removing local mail.
        let _ = store::secret_entry(id).and_then(|e| e.delete_credential().map_err(Into::into));
        // The account's mail, perhaps most of the cache: its space given back.
        let _ = self.store.give_back_space();
        Ok(())
    }
    /// Replaces credentials while preserving the account, cache, and preferences.
    /// Switches an account to signing in with Kerberos (on) or back to its
    /// password (off). Turning it on signs in with the system's ticket first
    /// and keeps nothing if that fails; the keychain's password is left as
    /// it was, for turning it off again.
    pub fn set_kerberos(&self, account: &Account, on: bool) -> Result<Account> {
        let mut next = account.clone();
        next.kerberos = on;
        next.validate()?;
        let lock = self.account_lock(&next.id);
        let _operation = lock.lock().unwrap();
        if on {
            let mut secret = Secret::default();
            self.check_login(&next, &mut secret)?;
        } else {
            // A password must be there to go back to.
            ensure!(
                self.saved_credentials(&next.id)
                    .is_ok_and(|s| !s.password.is_empty()),
                "This account has no saved password. Sign in again with the password to stop using Kerberos."
            );
        }
        // The connections signed in the old way are replaced.
        self.disconnect(&next.id);
        self.stop_watching(&next.id);
        self.store.save_account(&next)?;
        Ok(next)
    }
    pub fn reconnect(
        &self,
        account: &Account,
        password: String,
        cancel: Arc<AtomicBool>,
    ) -> Result<Account> {
        if !account.credentials_of.is_empty() {
            let owner = self
                .store
                .accounts()?
                .into_iter()
                .find(|o| o.id == account.credentials_of)
                .map(|o| o.label().to_owned())
                .unwrap_or_else(|| "the account it was shared with".into());
            bail!("This account signs in as {owner}: sign in again there.");
        }
        let mut next = account.clone();
        if next.oauth && next.client_secret.is_empty() {
            let registration = if next.provider == Provider::Jmap {
                crate::sync::oauth::jmap_client(&next)
            } else {
                crate::sync::oauth::client(next.provider)
            };
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
    /// Puts what lists show of decrypted messages (previews, attachments)
    /// back on messages reloaded from the disk cache, but not their text:
    /// a list holds messages without bodies (see `with_readable` for the
    /// one opened).
    pub fn with_readable_previews(&self, mut messages: Vec<Mail>) -> Vec<Mail> {
        let readable = self.readable.lock().unwrap();
        for m in messages.iter_mut().filter(|m| m.body.is_none()) {
            if let Some(r) = readable.get(&m.id) {
                m.subject = r.subject.clone();
                m.reply_to = r.reply_to.clone();
                m.snippet = r.snippet.clone();
                m.set_attachments(r.attachments().to_vec());
                m.has_attachments = r.has_attachments;
            }
        }
        messages
    }
    /// Puts decrypted bodies back on messages reloaded from the disk cache.
    pub fn with_readable(&self, mut messages: Vec<Mail>) -> Vec<Mail> {
        let readable = self.readable.lock().unwrap();
        for m in messages.iter_mut().filter(|m| m.body.is_none()) {
            if let Some(r) = readable.get(&m.id) {
                m.body = r.body.clone();
                m.html = r.html.clone();
                m.inline = r.inline.clone();
                m.subject = r.subject.clone();
                m.reply_to = r.reply_to.clone();
                m.snippet = r.snippet.clone();
                m.set_attachments(r.attachments().to_vec());
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
    /// Your OpenPGP keyring, read from the keychain the first time.
    pub fn keyring(&self) -> Arc<crate::manage::openpgp::Keyring> {
        let mut kept = self.keyring.lock().unwrap();
        if let Some(ring) = &*kept {
            return ring.clone();
        }
        #[cfg(test)]
        let ring = crate::manage::openpgp::Keyring::default();
        #[cfg(not(test))]
        let ring = crate::manage::openpgp::Keyring::load().unwrap_or_default();
        let ring = Arc::new(ring);
        *kept = Some(ring.clone());
        ring
    }
    /// Replaces the keyring, in the keychain and here. Messages read with
    /// the old one are read again with this one when next opened.
    pub fn set_keyring(&self, ring: crate::manage::openpgp::Keyring) -> Result<()> {
        #[cfg(not(test))]
        ring.save()?;
        *self.keyring.lock().unwrap() = Some(Arc::new(ring));
        self.verdicts.lock().unwrap().clear();
        Ok(())
    }
    /// Looks up keys for those of `emails` that have none here (WKD, then
    /// keys.openpgp.org), keeping those found. Returns the addresses a
    /// key was found for. Lookups that fail are skipped.
    pub fn find_keys(&self, emails: &[String]) -> Vec<String> {
        let ring = self.keyring();
        let missing: Vec<&String> = emails.iter().filter(|e| ring.key_for(e).is_none()).collect();
        if missing.is_empty() || cfg!(test) {
            return Vec::new();
        }
        let mut next = crate::manage::openpgp::Keyring::clone(&ring);
        let mut found = Vec::new();
        for email in missing {
            let Ok(tls) = self.tls_connector() else { break };
            if let Ok(Some((data, _))) = crate::sync::keys::lookup(email, tls)
                && next.add_found(email, &data).unwrap_or(false)
            {
                found.push(email.clone());
            }
        }
        if !found.is_empty() {
            let _ = self.set_keyring(next);
        }
        found
    }
    /// Keeps the key an Autocrypt header in a received message gives for
    /// its sender, if no key for them was imported by hand.
    fn learn_autocrypt(&self, raw: &[u8]) {
        let Some(header) = crate::sync::keys::autocrypt_full(raw) else {
            return;
        };
        let (email, key) = (header.addr.clone(), header.key.clone());
        // Peer state (Autocrypt 1.1 §2.3): an older message than the one
        // a key was last taken from doesn't replace it.
        let seen_key = format!("autocrypt_seen:{email}");
        let last: i64 = self
            .store
            .preference(&seen_key)
            .and_then(|s| s.split(' ').next()?.parse().ok())
            .unwrap_or(0);
        if header.date > 0 && header.date < last {
            return;
        }
        let _ = self.store.set_preference(
            &seen_key,
            &format!("{} {}", header.date.max(last), if header.mutual { "mutual" } else { "nopreference" }),
        );
        let ring = self.keyring();
        if ring.key_for(&email).is_some_and(|k| {
            use pgp::ser::Serialize as _;
            k.to_bytes().is_ok_and(|b| b == key)
        }) {
            return;
        }
        let mut next = crate::manage::openpgp::Keyring::clone(&ring);
        if next.add_found(&email, &key).unwrap_or(false) {
            let _ = self.set_keyring(next);
        }
    }
    /// What OpenPGP found when the message was last read, if it had any.
    pub fn verdict(&self, id: &str) -> Option<crate::manage::openpgp::Verdict> {
        self.verdicts.lock().unwrap().get(id).cloned()
    }
    /// Your S/MIME certificates, read from the keychain the first time.
    pub fn certificates(&self) -> Arc<crate::manage::smime::Certificates> {
        let mut kept = self.certificates.lock().unwrap();
        if let Some(c) = &*kept {
            return c.clone();
        }
        #[cfg(test)]
        let certs = crate::manage::smime::Certificates::default();
        #[cfg(not(test))]
        let certs = crate::manage::smime::Certificates::load().unwrap_or_default();
        let certs = Arc::new(certs);
        *kept = Some(certs.clone());
        certs
    }
    pub fn set_certificates(&self, certs: crate::manage::smime::Certificates) -> Result<()> {
        #[cfg(not(test))]
        certs.save()?;
        *self.certificates.lock().unwrap() = Some(Arc::new(certs));
        self.smime_verdicts.lock().unwrap().clear();
        Ok(())
    }
    /// What S/MIME found when the message was last read, if it had any.
    pub fn smime_verdict(&self, id: &str) -> Option<crate::manage::smime::Verdict> {
        self.smime_verdicts.lock().unwrap().get(id).cloned()
    }
    /// Opens an S/MIME message with your certificates, keeping the
    /// signer's certificate for writing back encrypted.
    fn open_smime(
        &self,
        id: Option<&str>,
        raw: &[u8],
    ) -> Option<(crate::manage::crypto::Protection, Vec<u8>)> {
        let certs = self.certificates();
        let (readable, verdict, signer) = crate::manage::smime::open(raw, &certs)?;
        if let Some(signer) = &signer {
            let mut next = crate::manage::smime::Certificates::clone(&certs);
            // The address the message says it is from: a certificate is
            // kept only for it.
            let from = mailparse::parse_headers(raw)
                .ok()
                .and_then(|(h, _)| {
                    use mailparse::MailHeaderMap as _;
                    h.get_first_value("From")
                })
                .map(|v| crate::sync::mime::receipt_address(&v))
                .unwrap_or_default();
            if crate::manage::smime::learn(&mut next, signer, &verdict.signature, &from)
                .unwrap_or(false)
            {
                let _ = self.set_certificates(next);
            }
        }
        let protection = if verdict.decrypted {
            crate::manage::crypto::Protection::Pinal
        } else {
            crate::manage::crypto::Protection::None
        };
        if let Some(id) = id {
            self.smime_verdicts.lock().unwrap().insert(id.to_owned(), verdict);
        }
        Some((protection, readable))
    }
    /// Opens a message with your own OpenPGP keys, where it has OpenPGP
    /// protection. Decrypted ones are kept from the disk cache as Pinal's are.
    fn open_openpgp(
        &self,
        id: Option<&str>,
        raw: &[u8],
    ) -> Option<(crate::manage::crypto::Protection, Vec<u8>)> {
        let ring = self.keyring();
        if ring.secret.is_empty() && ring.public.is_empty() {
            return None;
        }
        let (readable, verdict) = crate::manage::openpgp::open(raw, &ring)?;
        let protection = if verdict.decrypted {
            crate::manage::crypto::Protection::Pinal
        } else {
            crate::manage::crypto::Protection::None
        };
        if let Some(id) = id {
            self.verdicts.lock().unwrap().insert(id.to_owned(), verdict);
        }
        Some((protection, readable))
    }
    /// The readable message: decrypted here when Pinal stored it encrypted.
    fn plaintext(
        &self,
        a: &Account,
        raw: &[u8],
    ) -> Result<(crate::manage::crypto::Protection, Vec<u8>)> {
        self.plaintext_of(a, None, raw)
    }
    /// `plaintext`, noting what OpenPGP found for message `id`.
    fn plaintext_of(
        &self,
        a: &Account,
        id: Option<&str>,
        raw: &[u8],
    ) -> Result<(crate::manage::crypto::Protection, Vec<u8>)> {
        if crate::manage::crypto::encrypted_part(raw).is_none() {
            // Signed mail is shown with its signature checked.
            if let Some(opened) = self.open_openpgp(id, raw) {
                return Ok(opened);
            }
            if let Some(opened) = self.open_smime(id, raw) {
                return Ok(opened);
            }
            return crate::manage::crypto::open_message(raw, &[]);
        }
        // Not a Pinal account's: your own keys may open it.
        if a.service != Service::Pinal {
            if let Some(opened) = self.open_openpgp(id, raw) {
                return Ok(opened);
            }
            return Ok((crate::manage::crypto::Protection::Foreign, raw.to_vec()));
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
        // Background downloads and listing give way to a sync between
        // messages, so new mail isn't stuck behind a whole batch.
        let waiting = self.wait_for(&a.id);
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        drop(waiting);
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
            let result = self.sync_pop(a, folder, limit, pop_secret(a, &secret));
            return self.pop_limited(a, result);
        }
        if a.provider == Provider::Proton {
            return self.sync_proton(a, mailbox, limit);
        }
        if a.provider == Provider::Jmap {
            return self.sync_jmap(a, mailbox, limit);
        }
        if folder == Folder::Drafts {
            return Ok(vec![]);
        }
        self.with_imap(a, |c| {
            // Changes made while offline reach the server before anything
            // is read from it.
            self.replay_offline(c, a)?;
            // One LIST per sync keeps the folder list current.
            let names = c.names(true)?.to_vec();
            let folders = custom_folders(&names);
            let _ = self.store.save_folders(&a.id, &folders);
            if folder == Folder::All {
                return self.sync_all(c, a, &names, &folders, limit);
            }
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
    /// All Mail: brings each of the account's mailboxes up to date (Gmail's
    /// labels are in its All Mail already, so its own folders are left out),
    /// then lists the newest of everything cached but drafts.
    fn sync_all(
        &self,
        c: &mut Connection,
        a: &Account,
        names: &[Name],
        folders: &[crate::manage::model::RemoteFolder],
        limit: usize,
    ) -> Result<Vec<Mail>> {
        let mut visited = std::collections::HashSet::new();
        for source in [
            Folder::Inbox,
            Folder::Sent,
            Folder::Archive,
            Folder::Junk,
            Folder::Trash,
        ] {
            let remote = match resolve_in(names, a.provider, source) {
                Ok(remote) => remote,
                Err(_) if source != Folder::Inbox => continue,
                Err(e) => return Err(e),
            };
            if visited.insert(remote.clone()) && !self.unchanged(c, a, &remote) {
                self.sync_remote(c, a, &source.into(), &remote, limit)?;
            }
        }
        if a.provider != Provider::Gmail {
            for folder in folders {
                if !visited.insert(folder.name.clone()) || self.unchanged(c, a, &folder.name) {
                    continue;
                }
                let mailbox = Mailbox::Remote(folder.name.clone());
                // One folder that can't be read doesn't keep the rest from showing.
                if let Err(e) = self.sync_remote(c, a, &mailbox, &folder.name, limit)
                    && crate::sync::retry::transient(&e)
                {
                    return Err(e);
                }
            }
        }
        self.store.newest(&a.id, Folder::All, limit)
    }
    /// Whether a mailbox is as its cache last saw it, by the counters the
    /// listing gave (LIST-STATUS): the same UIDVALIDITY, UIDNEXT, message
    /// count and HIGHESTMODSEQ, so nothing arrived, left or changed flags,
    /// and it needn't even be selected. Without HIGHESTMODSEQ a flag change
    /// wouldn't show, so it's synced as usual.
    fn unchanged(&self, c: &Connection, a: &Account, remote: &str) -> bool {
        let Some(now) = c.statuses.get(remote) else {
            return false;
        };
        let Ok(Some(saved)) = self.store.mailbox_state(&a.id, remote) else {
            return false;
        };
        now.modseq.is_some()
            && saved.modseq == now.modseq
            && saved.validity == now.validity
            && saved.uid_next == now.uid_next
            && saved.exists == now.messages
            && saved.days == a.sync.days
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
        let state = self.store.mailbox_state(&a.id, remote)?;
        // With QRESYNC the SELECT itself says what was removed (VANISHED)
        // and which flags changed since the saved HIGHESTMODSEQ, so removals
        // no longer need a full SEARCH to reconcile.
        let resync_from = state
            .as_ref()
            .filter(|s| c.enabled.qresync && s.days == a.sync.days && s.uid_next > 0)
            .and_then(|s| s.modseq.map(|m| (s.validity, m)));
        let (selected, resync) = match resync_from {
            Some((validity, modseq)) => c.select_resync(remote, validity, modseq)?,
            None => (c.select(remote)?, None),
        };
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
        // QRESYNC lists removals instead, so the count needn't add up.
        if resync.is_none() && selected.exists != state.exists.saturating_add(fresh.len() as u32) {
            return self.full_sync(c, a, mailbox, remote, &selected, false, limit);
        }
        let prefix = format!("{}:{remote}:{validity}:", a.id);
        let Extensions {
            condstore, gmail, ..
        } = c.extensions()?;
        let items = flag_items(gmail);
        let (vanished, resynced) = match resync {
            Some(r) => (r.vanished, Some(r.changed)),
            None => (Vec::new(), None),
        };
        let rows = match (state.modseq, selected.modseq) {
            // The resynchronizing SELECT already sent the changed flags.
            _ if resynced.is_some() => resynced.unwrap_or_default(),
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
                    .newest(&a.id, mailbox, limit)?
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
            .map(|r| flag_update(r, gmail))
            .collect();
        let before = (a.sync.days > 0)
            .then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
        let removed: Vec<String> = vanished.iter().map(|uid| format!("{prefix}{uid}")).collect();
        self.store
            .apply_changes(mailbox, &a.id, &prefix, &fresh, &flags, &removed, before)?;
        if !removed.is_empty() {
            let gone: std::collections::HashSet<&String> = removed.iter().collect();
            self.readable.lock().unwrap().retain(|id, _| !gone.contains(id));
        }
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
        self.extend_layout(a, mailbox, validity, fresh.iter().map(|m| m.uid), &vanished);
        if !fresh.is_empty() {
            // Server threads for the new mail, where the server has them.
            let _ = self.thread_mailbox(c, a, remote, validity);
        }
        let mut result = self.store.newest(&a.id, mailbox, limit)?;
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
        // Drafts are local; Starred and All Mail are assembled from the
        // other mailboxes.
        if folder == Folder::Drafts || folder.is_virtual() {
            return Ok(Catalog::default());
        }
        self.ensure_background(a)?;
        if a.provider == Provider::Pop3 {
            let secret = self.credentials(a)?;
            let result = self.catalog_pop(a, folder, batch, pop_secret(a, &secret));
            return self.pop_limited(a, result);
        }
        if a.provider == Provider::Proton {
            return self.catalog_proton(a, mailbox, batch);
        }
        if a.provider == Provider::Jmap {
            return self.catalog_jmap(a, mailbox, batch);
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
        if wanted.is_empty() || !a.provider.imap() {
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
        let saved = if layout.is_none() && !mailbox.folder().is_virtual() {
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
        // The list is kept as long as it's shown; don't keep its growth room.
        messages.shrink_to_fit();
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
        let uids = search(c, a, false)?;
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
        let mut layouts = self.layouts.lock().unwrap();
        let key = (a.id.clone(), mailbox.key());
        // The same layout again keeps its identity: lists shown from it
        // compare layouts by pointer, and need not be built again.
        if let Some(known) = layouts.get(&key)
            && known.remote == remote
            && known.validity == validity
            && known.days == a.sync.days
            && *known.uids == uids
        {
            return known.clone();
        }
        let layout = Layout {
            remote: remote.into(),
            validity,
            days: a.sync.days,
            uids: Arc::new(uids),
        };
        layouts.insert(key, layout.clone());
        self.layouts_changed(&a.id);
        layout
    }
    pub(super) fn layouts_changed(&self, account: &str) {
        let change = self.layout_changes.fetch_add(1, Ordering::AcqRel) + 1;
        self.layout_revisions
            .lock()
            .unwrap()
            .insert(account.to_owned(), change);
    }
    /// Changes whenever what `account`'s message lists show may have: the
    /// cache's `Store::list_revision`, or one of its mailbox layouts (which
    /// place rows for mail not listed yet). Read before building a list; the
    /// list is current for as long as this stays the same.
    pub fn list_revision(&self, account: &str) -> (u64, u64) {
        let layout = self.layout_revisions.lock().unwrap().get(account).copied();
        (self.store.list_revision(account), layout.unwrap_or(0))
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
        let removed = self
            .layouts
            .lock()
            .unwrap()
            .remove(&(a.id.clone(), mailbox.key()));
        if removed.is_some() {
            self.layouts_changed(&a.id);
        }
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
        if !removed.is_empty() {
            let removed: std::collections::HashSet<u32> = removed.iter().copied().collect();
            uids.retain(|uid| !removed.contains(uid));
        }
        uids.sort_unstable();
        uids.dedup();
        // Nothing new and nothing gone (most checks): the layout keeps its
        // identity, so the list shown from it isn't rebuilt.
        if uids != *layout.uids {
            layout.uids = Arc::new(uids);
            drop(layouts);
            self.layouts_changed(&a.id);
        }
    }
    /// Older POP3 messages are listed by their headers (TOP n 0) where the
    /// server has TOP, many at a time, and download whole when opened;
    /// otherwise they are downloaded whole, a small batch at a time.
    /// Originals stay on the server.
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
        let ids = pop.identities(256 * 1024)?;
        let prefix = format!("{}:pop:", a.id);
        let cached = self.store.cached_ids(&a.id, &prefix)?;
        let missing: Vec<&(u32, String)> = ids
            .iter()
            .rev()
            .filter(|(_, uid)| !cached.contains(&format!("{prefix}{uid}")))
            .collect();
        let mut fetched = 0;
        // Headers alone where TOP works: a batch costs a few KB each.
        let sizes = if missing.is_empty() { Default::default() } else { pop.list()? };
        let mut headers = Vec::new();
        let mut whole = false;
        for (number, uid) in missing.iter().take(batch.min(200)) {
            let Some(top) = pop.top(*number, 256 * 1024)? else {
                whole = true;
                break;
            };
            let id = format!("{prefix}{uid}");
            let mut m = parse_message(&top, a, &id, Folder::Inbox, "INBOX", *number)?;
            // The header block isn't its text: no snippet or body yet.
            m.body = None;
            m.html = None;
            m.snippet.clear();
            m.size = sizes.get(number).copied().unwrap_or(top.len() as u64) as u32;
            headers.push(m);
        }
        if !headers.is_empty() {
            fetched += headers.len();
            self.store.save_messages(&headers)?;
        }
        if whole {
            for (number, uid) in missing.iter().take(batch.min(25)) {
                let id = format!("{prefix}{uid}");
                pop.command(&format!("RETR {number}"))?;
                let raw = pop.multiline(MAX_MESSAGE)?;
                let mut m = parse_message(&raw, a, &id, Folder::Inbox, "INBOX", *number)?;
                m.size = raw.len() as u32;
                self.store.save_download(&m, &raw)?;
                fetched += 1;
            }
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
        let all = search(c, a, starred_only)?;
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
                m.answered = row.answered();
                m.forwarded = row.forwarded();
                // Kept once handled here; see `Store::apply_changes`.
                m.mdn_sent |= row.mdn_sent();
                if let Some(labels) = row.labels(gmail) {
                    m.set_labels(labels);
                }
                result.push(m);
            }
        }
        if result.len() != newest.len() {
            // An expunge can race SEARCH/FETCH. Confirm vanished UIDs before
            // removing them; an incomplete successful FETCH is not a snapshot.
            let current: std::collections::HashSet<u32> = search(c, a, starred_only)?
                .into_iter()
                .collect();
            let fetched: std::collections::HashSet<u32> = result.iter().map(|m| m.uid).collect();
            ensure!(
                newest
                    .iter()
                    .all(|uid| !current.contains(uid) || fetched.contains(uid)),
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
            let _ = self.thread_mailbox(c, a, remote, validity);
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
        let ext = c.extensions()?;
        let gmail = ext.gmail;
        // Where the server has them: its thread and message ids (Gmail's,
        // or OBJECTID's), a preview before the body downloads, and when the
        // message was put here.
        let items = format!(
            "(UID FLAGS RFC822.SIZE INTERNALDATE{}{}{}{} {LIST_HEADERS})",
            if gmail { " X-GM-MSGID X-GM-THRID X-GM-LABELS" } else { "" },
            if ext.objectid && !gmail { " EMAILID THREADID" } else { "" },
            if ext.preview { " PREVIEW" } else { "" },
            if ext.savedate { " SAVEDATE" } else { "" },
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
            m.set_labels(row.labels(gmail).unwrap_or_default());
            m.answered = row.answered();
            m.forwarded = row.forwarded();
            m.mdn_sent = row.mdn_sent();
            m.set_thread_id(row.thread.clone().unwrap_or_default());
            m.set_saved(row.save_date.unwrap_or_default());
            if let Some(preview) = row.preview.as_deref().filter(|p| !p.is_empty()) {
                m.snippet = preview.to_owned();
            }
            if m.date == 0 {
                m.date = row.internal_date.unwrap_or_default();
            }
            result.push(m);
        }
        self.note_traffic(a, bytes);
        Ok(result)
    }
    /// A POP3 server's LOGIN-DELAY or SYS/TEMP (RFC 2449, 3206) pauses the
    /// account's background work as IMAP's `[THROTTLED]` does.
    fn pop_limited<T>(&self, a: &Account, result: Result<T>) -> Result<T> {
        result.map_err(|e| {
            let temporary = crate::sync::transport::rejection(&e).is_some_and(|r| r.temporary());
            if !temporary {
                return e;
            }
            self.note_limit(a, &e);
            match self.throttled_until(&a.id) {
                Some(until) => e.context(crate::manage::status::Throttled { until, budget: false }),
                None => e,
            }
        })
    }
    /// A POP3 message known by its headers, downloaded whole (RETR) by its
    /// UIDL identity and saved with them.
    fn download_pop(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let uidl = m
            .id
            .strip_prefix(&format!("{}:pop:", a.id))
            .context("This message isn't from this POP3 account.")?;
        let secret = self.credentials(a)?;
        let mut pop = self.pop_login(a, pop_secret(a, &secret))?;
        let ids = pop.identities(256 * 1024)?;
        // Message numbers last only for a session: find it by UIDL.
        let (number, _) = ids
            .iter()
            .find(|(_, u)| u == uidl)
            .context("This message is no longer on the POP3 server.")?;
        pop.command(&format!("RETR {number}"))?;
        let raw = pop.multiline(MAX_MESSAGE)?;
        let _ = pop.command("QUIT");
        let mut next = parse_message(&raw, a, &m.id, m.folder, "INBOX", *number)?;
        next.size = raw.len() as u32;
        next.folder = m.folder;
        next.unread = m.unread;
        next.starred = m.starred;
        self.store.save_download(&next, &raw)?;
        Ok(raw)
    }
    fn sync_pop(
        &self,
        a: &Account,
        folder: Folder,
        limit: usize,
        password: &str,
    ) -> Result<Vec<Mail>> {
        if !matches!(folder, Folder::Inbox | Folder::Starred | Folder::All) {
            return self.store.newest(&a.id, folder, limit);
        }
        let mut pop = self.pop_login(a, password)?;
        let ids = pop.identities(256 * 1024)?;
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
        // Sizes first (LIST), so a message too large to download is listed
        // by its headers instead of failing the whole sync before QUIT.
        let sizes = pop.list().unwrap_or_default();
        for (number, uid) in ids
            .iter()
            .rev()
            .filter(|(_, uid)| !erased.contains(uid))
            .take(limit)
        {
            let id = format!("{}:pop:{uid}", a.id);
            let cached = self.store.message(&id)?;
            let complete = cached.is_some() && self.store.has_raw(&id)?;
            let oversized = sizes.get(number).is_some_and(|n| *n > MAX_MESSAGE as u64);
            let mail = if let Some(m) = cached.as_ref().filter(|_| complete) {
                m.clone()
            } else if oversized {
                if let Some(m) = cached {
                    m
                } else {
                    let Some(top) = pop.top(*number, 256 * 1024)? else {
                        continue;
                    };
                    let mut m = parse_message(&top, a, &id, Folder::Inbox, "INBOX", *number)?;
                    m.body = None;
                    m.html = None;
                    m.snippet.clear();
                    m.size = sizes.get(number).copied().unwrap_or(0).min(u32::MAX as u64) as u32;
                    m
                }
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
        // Downloads above are committed before anything is deleted.
        let deleting = self.pop_deletions(a, &ids, &erased)?;
        if deleting.is_empty() {
            let _ = pop.command("QUIT");
        } else {
            self.pop_delete(&mut pop, &deleting)?;
        }
        self.store.save_messages(&messages)?;
        // POP3 has no folders. Keep local moves and messages removed from the
        // remote server; UIDL identities prevent duplicate downloads.
        messages = self.store.newest(&a.id, folder, limit)?;
        messages.sort_by(Mail::newest_first);
        if folder == Folder::Starred {
            messages.retain(|m| m.starred);
        }
        Ok(messages)
    }
    /// Downloads shipping mail known only by its headers, for the Summary to
    /// read: (account, message id) each, as `Store::parcels` lists them.
    /// Returns how many were downloaded; stops after a few failures in a
    /// row (offline, signed out), and when `cancel` is set.
    ///
    /// IMAP mail is fetched many messages per request, mailbox by mailbox,
    /// rather than one round trip (and perhaps a SELECT) per message.
    pub fn download_for_parcels(
        &self,
        messages: &[(String, String)],
        cancel: &AtomicBool,
    ) -> Result<usize> {
        let accounts = self.store.accounts()?;
        let mut downloaded = 0;
        let mut failed = 0;
        let mut last_error = None;
        // Each account's messages, in the order given.
        let mut by_account: Vec<(&Account, Vec<Mail>)> = Vec::new();
        for (account, id) in messages {
            let Some(a) = accounts.iter().find(|a| &a.id == account) else {
                continue;
            };
            if a.provider == Provider::Pop3 {
                continue;
            }
            let Some(m) = self.store.message(id)? else {
                continue;
            };
            match by_account.iter_mut().find(|(b, _)| b.id == a.id) {
                Some((_, list)) => list.push(m),
                None => by_account.push((a, vec![m])),
            }
        }
        let mut one_by_one: Vec<(&Account, Mail)> = Vec::new();
        for (a, mails) in by_account {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            // Larger ones download in parts (IMAP), or only their text.
            let limit = match a.provider {
                Provider::Proton | Provider::Jmap => MAX_MESSAGE,
                _ => large::CHUNK,
            };
            let (small, large): (Vec<Mail>, Vec<Mail>) =
                mails.into_iter().partition(|m| (m.size as usize) <= limit);
            one_by_one.extend(large.into_iter().map(|m| (a, m)));
            let fetched = match a.provider {
                Provider::Proton => self.fetch_sources_proton(a, &small, cancel),
                Provider::Jmap => self.fetch_sources_jmap(a, &small, cancel),
                _ => self.fetch_sources_for_parcels(a, &small, cancel),
            };
            match fetched {
                Ok(n) => downloaded += n,
                Err(e) => {
                    failed += 1;
                    last_error = Some(e);
                }
            }
        }
        for (a, m) in one_by_one {
            if cancel.load(Ordering::Relaxed) || failed >= 5 {
                break;
            }
            match self.read(a, &m) {
                Ok(_) => {
                    downloaded += 1;
                    failed = 0;
                }
                Err(e) => {
                    failed += 1;
                    last_error = Some(e);
                    if failed >= 5 {
                        break;
                    }
                }
            }
        }
        match last_error {
            Some(e) if downloaded == 0 && failed > 0 => Err(e),
            _ => Ok(downloaded),
        }
    }
    /// Downloads the sources of an IMAP account's messages known only by
    /// their headers, many per FETCH, for `download_for_parcels`. The
    /// account's connection is let go between requests, so opening a
    /// message doesn't wait for the whole download.
    fn fetch_sources_for_parcels(
        &self,
        a: &Account,
        mails: &[Mail],
        cancel: &AtomicBool,
    ) -> Result<usize> {
        /// Messages asked for per FETCH.
        const PER_FETCH: usize = 25;
        let mut stored = 0;
        let mut pending: Vec<&Mail> = Vec::new();
        for m in mails {
            // A Gmail message appears once per label: download it once.
            if m.gmail_id != 0 {
                if pending.iter().any(|p| p.gmail_id == m.gmail_id) {
                    continue;
                }
                if let Some(raw) = self.store.gmail_raw(&a.id, m.gmail_id)? {
                    self.store.save_raw(&m.id, &raw)?;
                    stored += 1;
                    continue;
                }
            }
            pending.push(m);
        }
        let mut groups: Vec<Vec<&Mail>> = Vec::new();
        for m in pending {
            match groups
                .iter_mut()
                .find(|g| g[0].remote_folder == m.remote_folder && same_validity(g[0], m))
            {
                Some(group) => group.push(m),
                None => groups.push(vec![m]),
            }
        }
        let mut last_error = None;
        let mut failed = 0;
        for group in &groups {
            for chunk in group.chunks(PER_FETCH) {
                if cancel.load(Ordering::Relaxed) || failed >= 3 {
                    return Ok(stored);
                }
                let waiting = self.wait_for(&a.id);
                let lock = self.account_lock(&a.id);
                let _operation = lock.lock().unwrap();
                drop(waiting);
                let result = self.with_imap(a, |c| {
                    c.select_for(chunk[0])?;
                    let uids: Vec<u32> = chunk.iter().map(|m| m.uid).collect();
                    let mut got = 0;
                    for f in c
                        .session
                        .uid_fetch(uid_set(&uids), "(UID BODY.PEEK[])")?
                        .iter()
                    {
                        let (Some(uid), Some(raw)) = (f.uid, f.body()) else {
                            continue;
                        };
                        let Some(m) = chunk.iter().find(|m| m.uid == uid) else {
                            continue;
                        };
                        if raw.len() > MAX_MESSAGE {
                            continue;
                        }
                        self.keep_source(a, m, raw)?;
                        got += 1;
                    }
                    Ok(got)
                });
                match result {
                    Ok(n) => {
                        stored += n;
                        failed = 0;
                    }
                    Err(e) => {
                        failed += 1;
                        last_error = Some(e);
                    }
                }
            }
        }
        match last_error {
            Some(e) if stored == 0 => Err(e),
            _ => Ok(stored),
        }
    }
    /// Keeps a message's downloaded source, and what only it tells: its
    /// attachments, preview, and the headers a message list may not carry.
    /// Encrypted mail stays ciphertext on disk.
    fn keep_source(&self, a: &Account, m: &Mail, raw: &[u8]) -> Result<()> {
        self.store.save_raw(&m.id, raw)?;
        // Any kind (S/MIME and inline PGP too, whose ciphertext was read
        // for a preview and an attachment list).
        if crate::manage::crypto::is_encrypted(raw) {
            return Ok(());
        }
        if let Ok(mut parsed) = parse_message(raw, a, &m.id, m.folder, &m.remote_folder, m.uid) {
            let mut next = m.clone();
            next.has_attachments = !parsed.attachments().is_empty();
            next.set_attachments(parsed.take_attachments());
            if next.snippet.is_empty() {
                next.snippet = std::mem::take(&mut parsed.snippet);
            }
            if next.message_id.is_empty() {
                next.message_id = std::mem::take(&mut parsed.message_id);
            }
            if next.references().is_empty() {
                next.set_references(parsed.references());
            }
            if next.receipt_to().is_empty() {
                next.set_receipt_to(parsed.receipt_to());
            }
            if next.list_unsubscribe.is_empty() {
                next.list_unsubscribe = std::mem::take(&mut parsed.list_unsubscribe);
                next.one_click = parsed.one_click;
            }
            self.store.save_messages(&[next])?;
        }
        Ok(())
    }
    /// Downloads Proton messages known only by their headers, for
    /// `download_for_parcels`: several asked for at once, rather than one
    /// round trip after another. The account is let go between requests.
    fn fetch_sources_proton(&self, a: &Account, mails: &[Mail], cancel: &AtomicBool) -> Result<usize> {
        /// Messages asked for at once.
        const AT_ONCE: usize = 8;
        let mut stored = 0;
        let mut last_error = None;
        let mut failed = 0;
        for chunk in mails.chunks(AT_ONCE) {
            if cancel.load(Ordering::Relaxed) || failed >= 3 {
                break;
            }
            let ids: Vec<(&Mail, String)> = chunk
                .iter()
                .filter_map(|m| Some((m, proton::proton_id(a, m)?.to_owned())))
                .collect();
            let waiting = self.wait_for(&a.id);
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            drop(waiting);
            let asked: Vec<String> = ids.iter().map(|(_, id)| id.clone()).collect();
            let result = self.with_proton(a, |s| {
                let mut got = s.literals(&asked);
                // A signed-out session fails them all: its error is the
                // request's, so the session is let go.
                let signed_out = !got.is_empty()
                    && got.iter().all(|r| {
                        r.as_ref().is_err_and(|e| e.downcast_ref::<AccessFailure>().is_some())
                    });
                if signed_out {
                    return Err(got.swap_remove(0).unwrap_err());
                }
                Ok(got)
            });
            match result {
                Ok(got) => {
                    let mut any = false;
                    for ((m, _), raw) in ids.iter().zip(got) {
                        match raw {
                            Ok(raw) if raw.len() <= large::MAX_DOWNLOAD => {
                                self.note_traffic(a, raw.len());
                                self.keep_source(a, m, &raw)?;
                                stored += 1;
                                any = true;
                            }
                            Ok(_) => {}
                            Err(e) => last_error = Some(e),
                        }
                    }
                    failed = if any { 0 } else { failed + 1 };
                }
                Err(e) => {
                    failed += 1;
                    last_error = Some(e);
                }
            }
        }
        match last_error {
            Some(e) if stored == 0 => Err(e),
            _ => Ok(stored),
        }
    }
    /// Downloads JMAP messages known only by their headers, for
    /// `download_for_parcels`: each batch's sources found in one request,
    /// and downloaded several at a time.
    fn fetch_sources_jmap(&self, a: &Account, mails: &[Mail], cancel: &AtomicBool) -> Result<usize> {
        /// Messages per batch, and downloads at a time.
        const PER_BATCH: usize = 25;
        const AT_ONCE: usize = 6;
        let mut stored = 0;
        let mut last_error = None;
        let mut failed = 0;
        for chunk in mails.chunks(PER_BATCH) {
            if cancel.load(Ordering::Relaxed) || failed >= 3 {
                break;
            }
            let waiting = self.wait_for(&a.id);
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            drop(waiting);
            let result = self.with_jmap(a, |s| {
                let ids: Vec<String> = chunk
                    .iter()
                    .filter_map(|m| jmap::jmap_id(a, m).map(str::to_owned))
                    .collect();
                let blobs = s.blob_ids(&ids)?;
                let wanted: Vec<(&Mail, &String)> = chunk
                    .iter()
                    .filter_map(|m| Some((m, blobs.get(jmap::jmap_id(a, m)?)?)))
                    .collect();
                let mut got: Vec<(&Mail, Result<Vec<u8>>)> = Vec::new();
                for part in wanted.chunks(AT_ONCE) {
                    let downloaded: Vec<Result<Vec<u8>>> = std::thread::scope(|scope| {
                        let handles: Vec<_> = part
                            .iter()
                            .map(|(_, blob)| {
                                scope.spawn(move || {
                                    s.download(blob, "message.eml", "message/rfc822", MAX_MESSAGE)
                                })
                            })
                            .collect();
                        handles
                            .into_iter()
                            .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("A download failed."))))
                            .collect()
                    });
                    got.extend(part.iter().map(|(m, _)| *m).zip(downloaded));
                }
                if !got.is_empty() && got.iter().all(|(_, r)| r.is_err()) {
                    let (_, first) = got.swap_remove(0);
                    return Err(first.unwrap_err());
                }
                Ok(got)
            });
            match result {
                Ok(got) => {
                    for (m, raw) in got {
                        match raw {
                            Ok(raw) => {
                                self.note_traffic(a, raw.len());
                                self.keep_source(a, m, &raw)?;
                                stored += 1;
                            }
                            Err(e) => last_error = Some(e),
                        }
                    }
                    failed = 0;
                }
                Err(e) => {
                    failed += 1;
                    last_error = Some(e);
                }
            }
        }
        match last_error {
            Some(e) if stored == 0 => Err(e),
            _ => Ok(stored),
        }
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
            // Flags, labels or a move may have been committed meanwhile;
            // those stay. The body comes from `next`: zero-access mail keeps
            // none in the cache.
            return Ok(self.store.refresh_message(&kept)?.map_or(next.clone(), |current| {
                let labels = current.labels().to_vec();
                let mut next = Mail {
                    unread: current.unread,
                    starred: current.starred,
                    folder: current.folder,
                    remote_folder: current.remote_folder,
                    ..next
                };
                next.set_labels(labels);
                next
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
        } else if a.provider == Provider::Proton {
            // Too large to download whole: its text alone, and attachments
            // when saved.
            if m.size as usize > large::MAX_DOWNLOAD {
                if m.partial && m.body.is_some() {
                    return Ok(m.clone());
                }
                let next = self.read_proton_parts(a, m)?;
                self.store.save_messages(std::slice::from_ref(&next))?;
                return Ok(next);
            }
            self.download_proton(a, m)?
        } else if a.provider == Provider::Jmap {
            if m.size as usize > large::MAX_DOWNLOAD {
                if m.partial && m.body.is_some() {
                    return Ok(m.clone());
                }
                let next = self.read_jmap_parts(a, m)?;
                self.store.save_messages(std::slice::from_ref(&next))?;
                return Ok(next);
            }
            self.download_jmap(a, m)?
        } else {
            if a.provider == Provider::Pop3 {
                // Listed by its headers (TOP): download it whole now.
                let raw = self.download_pop(a, m)?;
                let (next, kept) = self.parse_read(a, m, &raw)?;
                self.store.save_messages(&[kept])?;
                return Ok(next);
            }
            // Too large to download whole: its text is read from its parts
            // and attachments download when saved.
            if m.size as usize > large::MAX_DOWNLOAD {
                if m.partial && m.body.is_some() {
                    return Ok(m.clone());
                }
                let next = self.with_imap(a, |c| self.read_parts(c, a, m))?;
                self.store.save_messages(std::slice::from_ref(&next))?;
                return Ok(next);
            }
            let raw = match self.store.gmail_raw(&a.id, m.gmail_id)? {
                // The same Gmail message, already downloaded under another label.
                Some(raw) => raw,
                // Large messages download in pieces that resume after an
                // interruption; others in one request.
                None => self.with_imap(a, |c| {
                    c.select_for(m)?;
                    if m.size as usize > large::CHUNK {
                        return self.download_resumable(c, m);
                    }
                    let fetched = c
                        .session
                        .uid_fetch(m.uid.to_string(), "(UID BODY.PEEK[])")?;
                    let raw = fetched
                        .iter()
                        .filter(|f| f.uid == Some(m.uid))
                        .find_map(|f| f.body())
                        .context("Message no longer exists on the server")?
                        .to_vec();
                    ensure!(
                        raw.len() <= large::MAX_DOWNLOAD,
                        "Message exceeds the download limit."
                    );
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
        self.learn_autocrypt(raw);
        let (protection, plain) = self.plaintext_of(a, Some(&m.id), raw)?;
        // Keys the sender passed along inside the encryption for the
        // other recipients (Autocrypt-Gossip), where it was decrypted.
        if plain.len() != raw.len() || plain != raw {
            let recipients: Vec<String> = crate::sync::smtp::recipients(&format!("{}, {}", m.to, m.cc()))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, a)| a)
                .collect();
            let gossip = crate::sync::keys::autocrypt_gossip(&plain, &recipients);
            if !gossip.is_empty() {
                let mut next = crate::manage::openpgp::Keyring::clone(&self.keyring());
                let mut changed = false;
                for (email, key) in gossip {
                    // Gossip never replaces a key already kept for someone.
                    if next.key_for(&email).is_none() {
                        changed |= next.add_found(&email, &key).unwrap_or(false);
                    }
                }
                if changed {
                    let _ = self.set_keyring(next);
                }
            }
        }
        let mut next = parse_message(&plain, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        next.unread = m.unread;
        next.starred = m.starred;
        next.size = m.size;
        // What only the server's listing says, not the message's source.
        next.set_labels(m.labels().to_vec());
        if next.gmail_id == 0 {
            next.gmail_id = m.gmail_id;
        }
        if protection == crate::manage::crypto::Protection::Pinal {
            let mut kept = next.clone();
            kept.body = None;
            kept.html = None;
            kept.snippet.clear();
            kept.set_attachments(Vec::new());
            // The headers the encryption protected (a hidden subject) as
            // the envelope has them, which the server sees anyway: the
            // decrypted ones were kept on disk, and snoozing sent the
            // subject to the server in the clear. `with_readable` puts the
            // decrypted ones back while the message is in memory.
            if let Ok((outer, _)) = mailparse::parse_headers(raw) {
                use mailparse::MailHeaderMap;
                kept.subject = outer.get_first_value("Subject").unwrap_or_default();
                kept.reply_to = outer.get_first_value("Reply-To").unwrap_or_default();
            }
            self.keep_readable(next.clone());
            Ok((next, kept))
        } else {
            Ok((next.clone(), next))
        }
    }
    /// Keeps a decrypted message in memory (the disk cache has it only
    /// encrypted). Only the latest `READABLE_BODIES` keep their text, HTML
    /// and inline images; the rest keep what lists show, and are decrypted
    /// again from the cached source when opened. Reading through an
    /// encrypted mailbox no longer keeps every message read in memory.
    fn keep_readable(&self, mail: Mail) {
        const READABLE_BODIES: usize = 32;
        let mut readable = self.readable.lock().unwrap();
        let mut order = self.readable_order.lock().unwrap();
        order.retain(|id| id != &mail.id);
        order.push_back(mail.id.clone());
        readable.insert(mail.id.clone(), mail);
        while order.len() > READABLE_BODIES {
            if let Some(old) = order.pop_front().and_then(|id| readable.get_mut(&id)) {
                old.body = None;
                old.html = None;
                old.inline = crate::manage::model::InlineParts::none();
            }
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
        if a.provider == Provider::Proton {
            return self.create_folder_proton(a, parent, leaf);
        }
        if a.provider == Provider::Jmap {
            let change = self.create_folder_jmap(a, parent, leaf)?;
            self.store.save_folders(&a.id, &change.folders)?;
            return Ok(change);
        }
        let change = self.with_imap(a, |c| {
            let names = c.names(true)?.to_vec();
            let namespace = c.namespace()?;
            let name = new_folder_name(&names, a.provider, parent, leaf, namespace.as_deref())?;
            if let Some(parent) = parent {
                c.require_rights(parent, "k", "make folders")?;
            }
            let label = decode_name(&name);
            let wire = c.mailbox(&name)?;
            allowed(
                c.session.run_command_and_check_ok(format!("CREATE {wire}")),
                || format!("create “{label}”"),
            )?;
            subscribe(c, &name)?;
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
        if a.provider == Provider::Proton {
            return self.rename_folder_proton(a, name, leaf);
        }
        if a.provider == Provider::Jmap {
            return self.rename_folder_jmap(a, name, leaf);
        }
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
            let (from, to) = (c.mailbox(name)?, c.mailbox(&next)?);
            allowed(
                c.session
                    .run_command_and_check_ok(format!("RENAME {from} {to}")),
                || format!("rename “{label}”"),
            )?;
            for (old, new) in &moved {
                subscribe(c, new)?;
                let _ = c.session.unsubscribe(c.wire(old));
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
        if a.provider == Provider::Proton {
            return self.delete_folder_proton(a, name);
        }
        if a.provider == Provider::Jmap {
            return self.delete_folder_jmap(a, name);
        }
        let folders = self.with_imap(a, |c| {
            let names = c.names(true)?.to_vec();
            deletable(&names, a.provider, name)?;
            c.require_rights(name, "x", "delete the folder")?;
            c.leave(name)?;
            let label = decode_name(name);
            let wire = c.wire(name);
            allowed(c.session.delete(&wire), || format!("delete “{label}”"))?;
            // Its subscription goes too (RFC 3501 §6.3.4 leaves it): a
            // folder of that name made later isn't subscribed unasked.
            let _ = c.session.unsubscribe(&wire);
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
            // Copies stay on the server unless the account deletes them with
            // local deletions (`SyncSettings::pop_delete_with_local`), at the
            // next check; remember them so they don't download again.
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
        if a.provider == Provider::Proton {
            return self.delete_proton(a, mails);
        }
        if a.provider == Provider::Jmap {
            return self.delete_jmap(a, mails);
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
            // Deleted by id: their text needn't be read.
            let mails = self.store.summaries(&a.id, folder)?;
            self.delete_forever(a, &mails)?;
            return Ok(mails.len());
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        if a.provider == Provider::Proton {
            return self.empty_proton(a, folder);
        }
        if a.provider == Provider::Jmap {
            return self.empty_jmap(a, folder);
        }
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_imap(a, |c| {
            let remote = resolve_in(c.names(false)?, a.provider, folder)?;
            c.select(&remote)?;
            c.require_rights(&remote, "te", "delete messages")?;
            let uids: Vec<u32> = c.uid_search("ALL")?.into_iter().collect();
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
    /// Which of the server's messages (`ids`, from UIDL) this check deletes,
    /// by message number, under the account's POP3 deletion settings; none
    /// unless one is on. Only messages whose whole source is saved here are
    /// deleted by age; ones deleted here for good (`erased`) only with
    /// `pop_delete_with_local`. Also keeps the record of when each message
    /// was first seen downloaded, which ages count from.
    fn pop_deletions(
        &self,
        a: &Account,
        ids: &[(u32, String)],
        erased: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<(u32, String)>> {
        let key = format!("pop_downloaded:{}", a.id);
        let after = a.sync.pop_delete_after_days;
        let mut seen: std::collections::BTreeMap<String, i64> = self
            .store
            .preference(&key)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if after.is_none_or(|days| days == 0) && !seen.is_empty() {
            // Not counting ages: nothing to keep.
            seen.clear();
            self.store.set_preference(&key, "{}")?;
        }
        let mut deleting = Vec::new();
        if let Some(days) = after {
            let prefix = format!("{}:pop:", a.id);
            let downloaded = self.store.downloaded_ids(&a.id, &prefix)?;
            let now = chrono::Utc::now().timestamp();
            let before = seen.clone();
            let on_server: std::collections::HashSet<&str> =
                ids.iter().map(|(_, u)| u.as_str()).collect();
            seen.retain(|uid, _| on_server.contains(uid.as_str()));
            for (number, uid) in ids {
                if !downloaded.contains(&format!("{prefix}{uid}")) {
                    continue;
                }
                let since = if days == 0 {
                    now
                } else {
                    // Mail downloaded before this was turned on counts
                    // from now, not from when it arrived.
                    *seen.entry(uid.clone()).or_insert(now)
                };
                if now - since >= i64::from(days) * 86_400 {
                    deleting.push((*number, uid.clone()));
                }
            }
            if days > 0 && seen != before {
                self.store
                    .set_preference(&key, &serde_json::to_string(&seen)?)?;
            }
        }
        if a.sync.pop_delete_with_local {
            let mut chosen: std::collections::HashSet<u32> =
                deleting.iter().map(|(n, _)| *n).collect();
            for (number, uid) in ids {
                if erased.contains(uid) && chosen.insert(*number) {
                    deleting.push((*number, uid.clone()));
                }
            }
        }
        deleting.sort_unstable();
        Ok(deleting)
    }
    /// Deletes `deleting` (message numbers of this session) from the server:
    /// DELE each, then QUIT, which is when the server removes them. A
    /// failure before QUIT's reply deletes nothing for certain, and the
    /// next check decides again; DELE is never replayed in this session.
    fn pop_delete(
        &self,
        pop: &mut PopSession<native_tls::TlsStream<crate::sync::wire::Wire>>,
        deleting: &[(u32, String)],
    ) -> Result<()> {
        for (number, _) in deleting {
            // A refused DELE (already marked, say) leaves the rest to go.
            if let Err(e) = pop.dele(*number) {
                // Anything else is unknown: unmark what was marked (RSET,
                // RFC 1939 §5), so a QUIT can't remove only some of them.
                let _ = pop.rset();
                return Err(e);
            }
        }
        pop.quit()
            .context("The POP3 server didn't confirm deleting downloaded mail; it is checked again next time.")
    }
    /// UIDLs of POP3 mail deleted here for good, which stays on the server
    /// unless `pop_delete_with_local` deletes it at the next check.
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
        if a.provider == Provider::Proton {
            return self.quota_proton(a);
        }
        if a.provider == Provider::Jmap {
            return self.quota_jmap(a);
        }
        self.with_imap(a, |c| {
            if !c.extensions()?.quota {
                // Space used without a limit, where the server can say. Not
                // worth failing the account's settings over.
                return Ok(c.total_size_kib().ok().flatten().map(|used_kib| Quota {
                    used_kib,
                    limit_kib: 0,
                }));
            }
            // The standard folders' own roots too, where they differ from
            // the inbox's (archives kept apart, say).
            let names = c.names(false)?.to_vec();
            let mut others = Vec::new();
            for folder in [Folder::Archive, Folder::Sent] {
                if let Ok(remote) = resolve_in(&names, a.provider, folder)
                    && !remote.eq_ignore_ascii_case("INBOX")
                {
                    others.push(c.mailbox(&remote)?);
                }
            }
            quota_of(&mut c.session, &others)
        })
    }
    /// Where the same mailbox is reachable over JMAP (RFC 9698
    /// JMAPACCESS), for suggesting it in Settings: the session URL.
    pub fn jmap_access(&self, a: &Account) -> Result<Option<String>> {
        if !a.provider.imap() {
            return Ok(None);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            if !c.extensions()?.jmapaccess {
                return Ok(None);
            }
            jmap_access(&mut c.session)
        })
    }
    /// Applies one change to several messages, with one IMAP command per mailbox.
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
        if a.provider == Provider::Proton {
            // Changes made while offline go first, in order.
            let result = self
                .replay_offline_proton(a)
                .and_then(|()| self.change_proton(a, &current, &change));
            return match result {
                // Offline: the change shows here now and reaches Proton
                // with the next connection.
                Err(e) if crate::sync::retry::later(&e) => self.queue_offline(a, &current, &change),
                Err(e) => Err(e),
                // The message keeps its id at Proton wherever it is: its row
                // changes rather than going.
                Ok(()) => self.commit_change(&current, change, false),
            };
        }
        if a.provider == Provider::Jmap {
            // Changes made while offline go first, in order.
            let result = self
                .replay_offline_jmap(a)
                .and_then(|()| self.change_jmap(a, &current, &change));
            return match result {
                Err(e) if crate::sync::retry::later(&e) => self.queue_offline(a, &current, &change),
                Err(e) => Err(e),
                // The message keeps its id wherever it is: its row changes
                // rather than going.
                Ok(()) => self.commit_change(&current, change, false),
            };
        }
        if a.provider != Provider::Pop3 {
            let result = self.with_imap(a, |c| {
                // Changes made while offline go first, in order.
                self.replay_offline(c, a)?;
                self.change_remote(c, a, &current, change.clone(), true)
            });
            return match result {
                // Offline (or the connection dropped): the change shows
                // here now and reaches the server with the next connection.
                Err(e) if crate::sync::retry::later(&e) => {
                    self.queue_offline(a, &current, &change)
                }
                other => other,
            };
        }
        self.commit_change(&current, change, false)
    }
    /// Sends a change to the server, one command per mailbox and chunk;
    /// with `commit`, each acknowledged chunk is committed to the cache.
    fn change_remote(
        &self,
        c: &mut Connection,
        a: &Account,
        current: &[Mail],
        change: Change,
        commit: bool,
    ) -> Result<()> {
        let target = match change {
            Change::Move(folder) => Some(c.standard_folder(a.provider, folder)?),
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
            c.require_writable(match &change {
                Change::Move(_) | Change::MoveTo(_) => "move messages out",
                Change::Label(..) => "change labels",
                _ => "change messages",
            })?;
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
            // RFC 4314 §2.1: \Seen needs `s`, \Flagged and others `w`.
            match &change {
                Change::Read(_) => c.require_rights(&first.remote_folder, "s", "mark messages read")?,
                Change::Star(_) => c.require_rights(&first.remote_folder, "w", "star messages")?,
                _ => {}
            }
            // Moving mail into or out of Spam tells the server's spam
            // filter which it is ($Junk/$NotJunk, RFC 5788's registry), where
            // the mailbox keeps keywords. Out of Spam to Trash says nothing.
            let training = match &change {
                Change::Move(Folder::Junk) if first.folder != Folder::Junk => {
                    Some(("$Junk", "$NotJunk"))
                }
                Change::Move(to) if first.folder == Folder::Junk && *to != Folder::Trash =>
                {
                    Some(("$NotJunk", "$Junk"))
                }
                Change::MoveTo(_) if first.folder == Folder::Junk => Some(("$NotJunk", "$Junk")),
                _ => None,
            }
            // Gmail learns from the move into or out of Spam itself.
            .filter(|_| selected.keywords && !gmail);
            for chunk in uids.chunks(200) {
                let set = uid_set(chunk);
                let mut moved_to: Option<(u32, Vec<(u32, u32)>)> = None;
                if let Some((add, remove)) = training {
                    // A server that refuses the keywords still moves the mail.
                    optional(c.session.uid_store(&set, format!("+FLAGS.SILENT ({add})")).map(|_| ()))?;
                    optional(c.session.uid_store(&set, format!("-FLAGS.SILENT ({remove})")).map(|_| ()))?;
                }
                match change {
                    Change::Mark(mark) => {
                        let (flag, keyword) = mark.flag();
                        // Keywords stick only where the mailbox allows new
                        // ones; elsewhere the mark stays on this computer.
                        if !keyword || selected.keywords {
                            optional(
                                c.session
                                    .uid_store(&set, format!("+FLAGS.SILENT ({flag})"))
                                    .map(|_| ()),
                            )?;
                        }
                    }
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
                        let target = c.mailbox(target.as_deref().unwrap_or_default())?;
                        let heard = if c.extensions()?.moves {
                            // Atomic, and leaves other deleted messages alone.
                            c.heard(&format!("UID MOVE {set} {target}"))?
                        } else {
                            // RFC 6851's fallback: copy, then remove just these
                            // (refused up front where that can't be done).
                            c.can_erase(chunk)?;
                            let heard = c.heard(&format!("UID COPY {set} {target}"))?;
                            c.erase(chunk)?;
                            heard
                        };
                        // Where they went (UIDPLUS's COPYUID), so they show
                        // there before its next check.
                        moved_to = crate::sync::transport::copy_uids(&heard);
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
                if commit {
                    self.commit_change(&confirmed, change.clone(), true)?;
                    if let (Some((validity, pairs)), Some(target)) = (moved_to.take(), &target) {
                        self.place_moved(a, &confirmed, &change, target, validity, &pairs)?;
                    }
                } else if change.moves() {
                    // Replayed: the cache already shows it moved; the copy
                    // in its new mailbox arrives with that mailbox's sync.
                    self.store.commit_changes(&confirmed, true)?;
                }
            }
        }
        Ok(())
    }
    /// Saves moved messages in the mailbox they went to, under the UIDs the
    /// server gave them there (`pairs`: old UID, new UID; COPYUID), so the
    /// mailbox shows them, with what was downloaded of them, before its
    /// next check. That check lists them again under the same ids.
    fn place_moved(
        &self,
        a: &Account,
        moved: &[Mail],
        change: &Change,
        target: &str,
        validity: u32,
        pairs: &[(u32, u32)],
    ) -> Result<()> {
        let placed: Vec<Mail> = moved
            .iter()
            .filter_map(|m| {
                let (_, uid) = pairs.iter().find(|(old, _)| *old == m.uid)?;
                let mut next = m.clone();
                change.apply(&mut next);
                next.uid = *uid;
                next.remote_folder = target.to_owned();
                next.id = format!("{}:{target}:{validity}:{uid}", a.id);
                Some(next)
            })
            .collect();
        if placed.is_empty() {
            return Ok(());
        }
        self.store.save_messages(&placed)?;
        if let Some(mailbox) = placed.first().map(Mail::mailbox) {
            self.extend_layout(a, &mailbox, validity, placed.iter().map(|m| m.uid), &[]);
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
        // Proton: every unread message of the mailbox at Proton, Starred
        // and All Mail included (they are labels there).
        if a.provider == Provider::Proton && mailbox.folder() != Folder::Drafts {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            return self.mark_all_read_proton(a, mailbox);
        }
        // JMAP: the same, by query.
        if a.provider == Provider::Jmap && mailbox.folder() != Folder::Drafts {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            return self.mark_all_read_jmap(a, mailbox);
        }
        if mailbox.folder().is_virtual() {
            // Only the unread ones, without bodies, asked of the cache: every
            // message's text and HTML were read before to find them.
            let unread = self.store.unread_summaries(&a.id, mailbox.clone())?;
            return self.change_many(a, &unread, Change::Read(true));
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        if a.provider != Provider::Pop3 && mailbox.folder() != Folder::Drafts {
            self.with_imap(a, |c| {
                let remote = remote_name(c.names(false)?, a, mailbox)?;
                c.selected(&remote)?;
                c.require_writable("mark messages read")?;
                let unseen: Vec<u32> = c.uid_search("UNSEEN")?.into_iter().collect();
                if !unseen.is_empty() {
                    c.session
                        .uid_store(uid_ranges(&unseen), "+FLAGS.SILENT (\\Seen)")?;
                }
                Ok(())
            })?;
        }
        self.store.mark_mailbox_read(&a.id, mailbox.clone())
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
        if a.provider == Provider::Proton {
            return self.download_batch_proton(a, max, since);
        }
        if a.provider == Provider::Jmap {
            return self.download_batch_jmap(a, max, since);
        }
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
                        if crate::manage::crypto::is_encrypted(raw) {
                            stored += 1;
                            continue;
                        }
                        // Keep summaries small: list views load every row's JSON.
                        if let Ok(mut parsed) =
                            parse_message(raw, a, &m.id, m.folder, &m.remote_folder, m.uid)
                        {
                            let mut next = (*m).clone();
                            next.has_attachments = !parsed.attachments().is_empty();
                            next.set_attachments(parsed.take_attachments());
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
        // Each check waits on the sender's DNS (up to its timeout), so
        // checking the messages one after another could take over a minute.
        // Their sources are gathered first, then checked at once; the newest
        // that verifies is still the one used.
        let sources: Vec<Option<Vec<u8>>> =
            messages.iter().map(|m| self.original(a, m).ok()).collect();
        let checked: Vec<Option<Plan>> = std::thread::scope(|scope| {
            let checks: Vec<_> = sources
                .iter()
                .map(|raw| {
                    scope.spawn(move || {
                        crate::sync::auth::signed_unsubscribe(raw.as_deref()?)
                            .and_then(|signed| Plan::new(Some(&signed), ""))
                    })
                })
                .collect();
            checks.into_iter().map(|c| c.join().ok().flatten()).collect()
        });
        if let Some(plan) = checked.into_iter().flatten().next() {
            return Ok(plan);
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
    /// Sends the scheduled draft `id` if it is due. The window and the
    /// background monitor (one taking over from the other) may both find it
    /// due: whichever comes second while the first is sending, or after it
    /// sent (and so discarded) the draft, gets `Ok(None)` and sends nothing.
    /// It is read again here, so a time changed meanwhile counts.
    pub fn send_scheduled(
        &self,
        a: &Account,
        id: &str,
    ) -> Result<Option<(Draft, Vec<String>)>> {
        if !self.scheduled_sending.lock().unwrap().insert(id.to_owned()) {
            return Ok(None);
        }
        struct Claim<'a>(&'a Mutex<HashSet<String>>, &'a str);
        impl Drop for Claim<'_> {
            fn drop(&mut self) {
                self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(self.1);
            }
        }
        let _claim = Claim(&self.scheduled_sending, id);
        let now = chrono::Utc::now().timestamp();
        let Some(d) = self.store.draft(id)? else {
            return Ok(None);
        };
        if !d.send_at.is_some_and(|t| t <= now) || !d.send_error.is_empty() {
            return Ok(None);
        }
        let notes = self.send_noting(a, &d)?;
        Ok(Some((d, notes)))
    }
    /// Whether the scheduled draft `id` is being sent now (see `send_scheduled`).
    pub fn sending_scheduled(&self, id: &str) -> bool {
        self.scheduled_sending.lock().unwrap().contains(id)
    }
    /// Whether any scheduled draft is being sent now: a background process
    /// handing over to the window waits for it, so it isn't sent twice.
    pub fn sending_any_scheduled(&self) -> bool {
        !self.scheduled_sending.lock().unwrap().is_empty()
    }
    /// A scheduled draft that can't be sent stays in Drafts, saying why,
    /// for the person to fix and send.
    pub fn schedule_failed(&self, id: &str, error: &str) {
        if let Ok(Some(mut d)) = self.store.draft(id) {
            d.send_error = error.to_owned();
            let _ = self.store.save_draft(&d);
        }
    }
    pub fn send(&self, a: &Account, d: &Draft) -> Result<()> {
        self.send_noting(a, d).map(|_| ())
    }
    /// Sends a draft. Once the server accepts it, it is sent: anything that
    /// then didn't work (a copy in Sent, receipts) is a note, not an error.
    pub fn send_noting(&self, a: &Account, d: &Draft) -> Result<Vec<String>> {
        self.send_held(a, d, None)
    }
    /// Hands a scheduled draft to its server, which holds it and sends it at
    /// its time even with Picacho closed (SMTP FUTURERELEASE, JMAP
    /// HOLDUNTIL), where the account allows it and the server can. False
    /// when it stays scheduled here (nothing was sent). Once handed over it
    /// is sent as far as Picacho is concerned: it's no longer a draft, and
    /// can't be changed or taken back.
    pub fn hand_to_server(&self, a: &Account, id: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let Some(d) = self.store.draft(id)? else {
            return Ok(false);
        };
        let Some(at) = d.send_at.filter(|t| *t > now + 60) else {
            return Ok(false);
        };
        if !a.server_send_later || a.provider == Provider::Proton {
            return Ok(false);
        }
        match self.send_held(a, &d, Some(at)) {
            Ok(_) => Ok(true),
            Err(e) if e.downcast_ref::<crate::sync::smtp::NoServerHold>().is_some() => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn send_held(&self, a: &Account, d: &Draft, hold_until: Option<i64>) -> Result<Vec<String>> {
        // Sending as an alias asks its service for the routed addresses
        // first, outside the account's lock, once it knows the alias is
        // still there.
        // An account kept to its aliases may still send as its own other
        // addresses: they don't show the mailbox's address either.
        if d.send_as.trim().is_empty() && d.identity.is_empty() {
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
        // Encrypting: look up keys not yet known for its recipients first,
        // and note those whose key nobody checked.
        let mut unchecked = Vec::new();
        if d.encrypt && !d.smime {
            let mut to = Vec::new();
            for list in [&d.to, &d.cc, &d.bcc] {
                to.extend(
                    crate::sync::smtp::recipients(list)?
                        .into_iter()
                        .map(|(_, email)| email),
                );
            }
            self.find_keys(&to);
            unchecked = self.keyring().unverified(&to);
        }
        // Requiring encryption all the way: say which recipients' domains
        // publish nothing (MTA-STS or DANE) a server could insist on it by,
        // since mail to them will bounce. Looked up before the account is
        // locked: it can take a few seconds.
        // (Tests stay off the network.)
        let unprotected = if a.require_tls && !cfg!(test) {
            crate::sync::mta_sts::unprotected(&crate::sync::mta_sts::domains(&[
                &d.to, &d.cc, &d.bcc,
            ]))
        } else {
            Vec::new()
        };
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        use crate::sync::smtp::Endpoint;
        // A sending address with its own outgoing server goes through it,
        // signed in with its own password; anything else through the
        // account's server.
        let identity = (d.send_as.trim().is_empty() && !d.identity.is_empty())
            .then(|| a.identity(&d.identity))
            .flatten();
        let own_server = identity.and_then(|i| Endpoint::of_identity(i).map(|e| (i, e)));
        let (endpoint, secret) = match &own_server {
            Some((i, endpoint)) => (endpoint.clone(), self.identity_secret(i)?.password),
            None => {
                let s = self.credentials(a)?;
                (
                    Endpoint::of(a),
                    if a.oauth { s.access_token } else { s.password },
                )
            }
        };
        // Set where the message went to Sent before it was sent (BURL).
        let mut filed_already = false;
        let delivered = if a.provider.api_sending() && own_server.is_none() {
            // Through Proton's API: it encrypts for the recipients it can,
            // signs, and files the message in Sent itself. A JMAP server
            // submits it and files it in Sent.
            let message = crate::sync::smtp::build(a, d, false)?;
            let (ring, certs) = (self.keyring(), self.certificates());
            let raw = crate::sync::smtp::protected(
                &message,
                d,
                &crate::sync::smtp::Keys { ring: &ring, certs: &certs },
            )?;
            let envelope = message.envelope();
            let from = envelope
                .from()
                .map(|f| f.to_string())
                .context("The message has no sender.")?;
            let to: Vec<String> = envelope.to().iter().map(|t| t.to_string()).collect();
            let receipts = if a.provider == Provider::Jmap {
                // Encryption all the way and delivery reports, as SMTP
                // asks for them, where the JMAP server can.
                let options = crate::sync::jmap::SendOptions {
                    require_tls: a.require_tls,
                    receipts: a.delivery_receipts.then(|| d.id.clone()),
                    hold_until,
                };
                if let Some(at) = hold_until {
                    let ahead = (at - chrono::Utc::now().timestamp()).max(0) as u64;
                    let can = self.with_jmap(a, |s| Ok(s.max_delayed_send() >= ahead))?;
                    if !can {
                        return Err(crate::sync::smtp::NoServerHold.into());
                    }
                }
                let submitted = self.send_jmap(a, &from, &to, &raw, &options)?;
                // Kept by Message-ID, so a sent message can show what became
                // of it (each recipient's delivery, as the server knows it).
                self.note_submission(a, &message_id_of(&raw), &submitted.id);
                submitted.receipts
            } else {
                self.send_proton(a, &from, &to, &raw)?;
                false
            };
            crate::sync::smtp::Delivered {
                raw,
                receipts_unsupported: a.delivery_receipts && !receipts,
            }
        } else {
            let tls = self.tls_connector()?;
            let (ring, certs) = (self.keyring(), self.certificates());
            let keys = crate::sync::smtp::Keys { ring: &ring, certs: &certs };
            match self.send_by_reference(a, d, &endpoint, &secret, &tls, hold_until, &keys) {
                Some(delivered) => {
                    filed_already = true;
                    delivered?
                }
                None => crate::sync::smtp::deliver(a, d, &endpoint, &secret, &tls, hold_until, &keys)?,
            }
        };
        let sent_raw = delivered.raw;
        let mut notes = Vec::new();
        if !unchecked.is_empty() {
            notes.push(format!(
                "It was encrypted to {} found automatically for {}. Compare {} fingerprint with {} in Settings → Encryption.",
                if unchecked.len() == 1 { "a key" } else { "keys" },
                unchecked.join(", "),
                if unchecked.len() == 1 { "its" } else { "each" },
                if unchecked.len() == 1 { "its owner" } else { "their owners" },
            ));
        }
        if !unprotected.is_empty() {
            notes.push(format!(
                "It may bounce at {}: encryption all the way was required, and {} no MTA-STS policy or DANE records for servers to insist on it by.",
                unprotected.join(", "),
                if unprotected.len() == 1 { "that domain publishes" } else { "those domains publish" }
            ));
        }
        if delivered.receipts_unsupported {
            notes.push(format!(
                "{} can't request delivery receipts, so none were asked for.",
                a.smtp_host
            ));
        }
        // Gmail and Outlook save what goes through their own servers
        // themselves. Custom IMAP servers do not, nor does anyone save what
        // a sending address sent through another server.
        // Proton files what it sends itself; a copy of what went through
        // another server is added to its Sent, as Bridge does for a mail
        // app's copy.
        let proton_copy = a.provider == Provider::Proton && own_server.is_some();
        let jmap_copy = a.provider == Provider::Jmap && own_server.is_some();
        if !filed_already
            && (a.provider == Provider::Imap
                || (a.provider.imap() && own_server.is_some())
                || proton_copy
                || jmap_copy)
        {
            let raw = sent_raw.clone();
            let saved = if proton_copy {
                self.file_sent_copy_proton(a, &raw)
            } else if jmap_copy {
                self.file_sent_copy_jmap(a, &raw)
            } else {
                self.with_imap(a, |c| self.file_sent_copy(c, a, &raw))
            };
            match saved {
                Ok(()) => {}
                // It will never fit: nothing to try again.
                Err(e) if e.is::<TooLargeForSent>() => {
                    notes.push(format!("A copy wasn't saved in Sent: {e:#}"));
                }
                Err(e) => match self.queue_sent_copy(&a.id, &raw) {
                    Ok(()) => notes.push(format!(
                        "A copy couldn't be saved in Sent yet ({e:#}); Picacho will try again with the next mail check."
                    )),
                    Err(_) => notes.push(format!("A copy wasn't saved in Sent: {e:#}")),
                },
            }
        }
        // SMTP acceptance is the delivery boundary. Do not report a send failure
        // after this point, which could encourage a duplicate transmission.
        if a.provider == Provider::Pop3 {
            let raw = sent_raw;
            let id = format!("{}:sent:{}", a.id, d.id);
            if let Ok(mut sent) = parse_message(&raw, a, &id, Folder::Sent, "Sent", 0) {
                sent.unread = false;
                let _ = self.store.save_messages(&[sent]);
                let _ = self.store.save_raw(&id, &raw);
            }
        }
        let _ = self.discard_draft(&d.id);
        drop(_operation);
        // The message it answers or forwards says so, here and in other
        // mail apps (\\Answered, $Forwarded); offline, that waits.
        self.mark_original(a, d);
        Ok(notes)
    }
    /// Saves a sent message in the account's Sent folder, unless a copy
    /// with its Message-ID is already there (an earlier try may have
    /// succeeded without its reply arriving).
    fn file_sent_copy(&self, c: &mut Connection, a: &Account, raw: &[u8]) -> Result<()> {
        if let Some(limit) = c.extensions()?.append_limit
            && raw.len() as u64 > limit
        {
            return Err(TooLargeForSent(limit).into());
        }
        let sent = c.standard_folder(a.provider, Folder::Sent)?;
        let id = message_id_of(raw);
        if !id.is_empty() {
            c.select(&sent)?;
            let found = match c.uid_search(&format!("HEADER Message-ID {}", quote(&id)?))
            {
                Ok(found) => found,
                // A server that can't search headers: file it anyway.
                Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => Default::default(),
                Err(e) => return Err(e.into()),
            };
            if !found.is_empty() {
                return Ok(());
            }
        }
        let wire = c.wire(&sent);
        let heard = c.append_heard(&wire, raw, &[imap::types::Flag::Seen])?;
        // Where it went (UIDPLUS's APPENDUID): Sent shows it at once.
        if let Some((validity, uid)) = crate::sync::transport::append_uid(&heard) {
            let id = format!("{}:{sent}:{validity}:{uid}", a.id);
            if let Ok(mut m) = parse_message(raw, a, &id, Folder::Sent, &sent, uid) {
                m.unread = false;
                self.store.save_messages(&[m])?;
            }
        }
        Ok(())
    }
    /// Sends through the account's own IMAP server's copy (BURL, RFC 4468),
    /// for messages whose attachments add up to `BURL_FROM` or more:
    /// the message is filed in Sent first, and the outgoing server fetches
    /// it from there with an URL the IMAP server authorized (URLAUTH), so a
    /// large message crosses the network once rather than twice. For an
    /// IMAP account sending through its own outgoing server, when both
    /// servers can. `None` where they can't (nothing was done: send as
    /// usual). If the send fails the copy in Sent is removed again.
    #[allow(clippy::too_many_arguments)]
    fn send_by_reference(
        &self,
        a: &Account,
        d: &Draft,
        endpoint: &crate::sync::smtp::Endpoint,
        secret: &str,
        tls: &native_tls::TlsConnector,
        hold_until: Option<i64>,
        keys: &crate::sync::smtp::Keys,
    ) -> Option<Result<crate::sync::smtp::Delivered>> {
        // Only where it saves something: a message with attachments.
        let large = d
            .attachments
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum::<u64>()
            >= BURL_FROM;
        if a.provider != Provider::Imap || endpoint.host != a.smtp_host || !large {
            return None;
        }
        let ready = self.with_imap(a, |c| {
            let ext = c.extensions()?;
            Ok(ext.urlauth && ext.uidplus)
        });
        if !matches!(ready, Ok(true)) {
            return None;
        }
        if !crate::sync::smtp::takes_burl(endpoint, secret, tls).unwrap_or(false) {
            return None;
        }
        let built = crate::sync::smtp::build(a, d, false)
            .and_then(|message| Ok((crate::sync::smtp::protected(&message, d, keys)?, message)));
        let (raw, message) = match built {
            Ok(b) => b,
            Err(e) => return Some(Err(e)),
        };
        // A forward with the original attached whole, the original on this
        // server: the server puts it in itself (CATENATE), so it isn't sent
        // up again. Only where the attachment is the original as it is.
        let original = self.catenated_original(a, d, &raw);
        // Filed and authorized; nothing has been sent yet.
        let filed = self.with_imap(a, |c| {
            if let Some(limit) = c.extensions()?.append_limit
                && raw.len() as u64 > limit
            {
                bail!(TooLargeForSent(limit));
            }
            let sent = c.standard_folder(a.provider, Folder::Sent)?;
            let wire = c.wire(&sent);
            let flags = [imap::types::Flag::Seen];
            let catenated = match &original {
                Some((m, at)) => c
                    .catenate(
                        &wire,
                        &flags,
                        &raw[..*at],
                        (&c.wire(&m.remote_folder), validity_of(m), m.uid),
                        &raw[at + original_len(m, self)..],
                    )
                    .unwrap_or(None),
                None => None,
            };
            let heard = match catenated {
                Some(heard) => heard,
                None => c.append_heard(&wire, &raw, &flags)?,
            };
            let (validity, uid) = crate::sync::transport::append_uid(&heard)
                .context("The server didn't say where it filed the message.")?;
            let url = c.submit_url(&a.incoming_host, a.incoming_port, a.login(), &wire, validity, uid);
            Ok((sent, validity, uid, url))
        });
        let Ok((sent, validity, uid, url)) = filed else {
            return None;
        };
        let remove = || {
            let _ = self.with_imap(a, |c| {
                c.select(&sent)?;
                c.erase(&[uid])
            });
        };
        let Ok(url) = url else {
            remove();
            return None;
        };
        let delivered = crate::sync::smtp::deliver_bytes(
            a,
            d,
            &message,
            raw.clone(),
            endpoint,
            secret,
            tls,
            hold_until,
            Some(crate::sync::smtp::Burl { url: &url }),
        );
        match &delivered {
            Ok(_) => {
                let id = format!("{}:{sent}:{validity}:{uid}", a.id);
                if let Ok(mut m) = parse_message(&raw, a, &id, Folder::Sent, &sent, uid) {
                    m.unread = false;
                    let _ = self.store.save_messages(&[m]);
                }
            }
            // Not sent: it isn't in Sent either.
            Err(_) => remove(),
        }
        Some(delivered)
    }
    /// For a forward with its original attached whole (`d.forwarding`, an
    /// `.eml` whose bytes are the original's as kept here): the original,
    /// on this account's server, and where its bytes start in `raw`.
    fn catenated_original(&self, a: &Account, d: &Draft, raw: &[u8]) -> Option<(Mail, usize)> {
        if !d.forwarding || d.original.is_empty() || d.encrypt || d.sign {
            return None;
        }
        let m = self.store.message(&d.original).ok()??;
        if m.account != a.id || m.uid == 0 || validity_of(&m) == 0 {
            return None;
        }
        let source = self.store.raw(&m.id).ok()??;
        // The draft attaches exactly that source; only a file of its size
        // is read to compare.
        let attached = d.attachments.iter().any(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("eml"))
                && std::fs::metadata(p).is_ok_and(|m| m.len() == source.len() as u64)
                && std::fs::read(p).is_ok_and(|bytes| bytes == source)
        });
        if !attached || source.len() < 64 {
            return None;
        }
        // A substring search, not a comparison at every offset (slow on
        // repetitive mail).
        let at = memchr::memmem::find(raw, &source)?;
        Some((m, at))
    }
    /// Where copies for Sent that couldn't be saved yet wait, per account.
    fn sent_queue(&self, account: &str) -> std::path::PathBuf {
        let safe: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.store.path.join("sent-copies").join(safe)
    }
    fn queue_sent_copy(&self, account: &str, raw: &[u8]) -> Result<()> {
        let dir = self.sent_queue(account);
        std::fs::create_dir_all(&dir)?;
        // Named by when, so they are saved in the order they were sent.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("{nanos:020}-{}.eml", uuid::Uuid::new_v4());
        // Written whole, then named, so a half-written file is never filed.
        // One that fails is removed: queues read only `.eml` files, so it
        // would otherwise stay (see `tidy_files` for one a crash left).
        let partial = dir.join(format!("{name}.partial"));
        let written = std::fs::write(&partial, raw).and_then(|()| std::fs::rename(&partial, dir.join(name)));
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        Ok(written?)
    }
    /// Files left behind by work that was interrupted: downloads not
    /// resumed in a week, and copies for Sent whose writing a crash cut
    /// short. Run when the app starts, off the interface's thread.
    pub fn tidy_files(&self) {
        self.remove_stale_partials();
        let Ok(queues) = std::fs::read_dir(self.store.path.join("sent-copies")) else {
            return;
        };
        for queue in queues.flatten() {
            large::remove_older_than(
                &queue.path(),
                std::time::Duration::from_secs(86_400),
                |p| p.extension().is_some_and(|x| x == "partial"),
            );
        }
    }
    /// How many copies for Sent are waiting to be saved for an account.
    #[cfg(test)]
    pub fn queued_sent_copies(&self, account: &str) -> usize {
        std::fs::read_dir(self.sent_queue(account))
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "eml"))
                    .count()
            })
            .unwrap_or(0)
    }
    /// Saves the copies for Sent that are waiting, oldest first, stopping
    /// at the first that fails. Copies that never fit, or that have waited
    /// two weeks, are let go. Returns how many were saved.
    fn file_queued_sent_copies(&self, c: &mut Connection, a: &Account) -> Result<usize> {
        let Ok(entries) = std::fs::read_dir(self.sent_queue(&a.id)) else {
            return Ok(0);
        };
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "eml"))
            .collect();
        files.sort();
        let mut filed = 0;
        for path in files {
            let stale = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(14 * 86_400));
            let raw = std::fs::read(&path)?;
            match self.file_sent_copy(c, a, &raw) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&path);
                    filed += 1;
                }
                Err(e) if e.is::<TooLargeForSent>() || stale => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(filed)
    }
    /// Deletes a draft here, and its copy on the server at the next draft
    /// sync (it may have been uploaded since a composer last saved it).
    pub fn discard_draft(&self, id: &str) -> Result<()> {
        // Taken in one step: a sync uploading it now either sees it gone
        // (and removes its new copy, `finish_draft_upload`) or its new copy
        // is the one queued here.
        if let Some(d) = self.store.take_draft(id)? {
            if let Some(remote) = d.remote {
                self.queue_draft_removal(&d.account, remote)?;
            }
            // Files taken from a server draft were kept for it alone.
            let _ = std::fs::remove_dir_all(self.draft_files(id));
        }
        Ok(())
    }
    /// Notes a draft's server copy to be removed at the next draft sync.
    pub(super) fn queue_draft_removal(&self, account: &str, remote: RemoteDraft) -> Result<()> {
        let _guard = DRAFT_REMOVALS.lock().unwrap_or_else(|e| e.into_inner());
        let mut pending = self.draft_removals(account);
        if !pending.contains(&remote) {
            pending.push(remote);
        }
        self.save_draft_removals(account, &pending)
    }
    /// Forgets the removals `done`, keeping any queued since they were read.
    pub(super) fn clear_draft_removals(&self, account: &str, done: &[RemoteDraft]) -> Result<()> {
        if done.is_empty() {
            return Ok(());
        }
        let _guard = DRAFT_REMOVALS.lock().unwrap_or_else(|e| e.into_inner());
        let mut pending = self.draft_removals(account);
        pending.retain(|r| !done.contains(r));
        self.save_draft_removals(account, &pending)
    }
    /// Records a draft's new server copy after an upload. Returns false when
    /// the draft was discarded meanwhile: the caller removes that copy (or,
    /// failing that, queues it), so it doesn't come back as a draft.
    /// A composer may have saved a newer version meanwhile; it keeps its
    /// words, and uploads again next time.
    pub(super) fn finish_draft_upload(&self, d: &Draft, remote: RemoteDraft, fingerprint: String) -> Result<bool> {
        let Some(mut current) = self.store.draft(&d.id)? else {
            return Ok(false);
        };
        current.remote = Some(remote);
        current.uploaded = fingerprint;
        self.store.update_draft(&current)
    }
    /// The copy of a draft kept in the server's Drafts folder. A draft to
    /// be encrypted with OpenPGP is encrypted to your own key first, so the
    /// server never has it readable; other devices with your key open it.
    /// One to be encrypted with S/MIME, or without a key of yours to seal
    /// it with, isn't uploaded (`None`): it stays on this device.
    pub(super) fn draft_upload(&self, a: &Account, d: &Draft) -> Result<Option<Vec<u8>>> {
        if !d.encrypt {
            return Ok(Some(crate::sync::smtp::build(a, d, true)?.formatted()));
        }
        // Whether it can be sealed is known before the message (with every
        // attachment read from disk) is built, once.
        let Some((me, smime)) = self.draft_seal(a, d)? else {
            return Ok(None);
        };
        let raw = crate::sync::smtp::build(a, d, true)?.formatted();
        if smime {
            // Sealed to your own certificate, as S/MIME drafts are.
            let certs = self.certificates();
            return Ok(Some(crate::manage::smime::protect_with(&certs, &raw, &me, &[], false, true)?));
        }
        let ring = self.keyring();
        let how = crate::manage::openpgp::Protect {
            sign: false,
            encrypt: true,
            subject: d.encrypt_subject,
        };
        Ok(Some(crate::manage::openpgp::protect(&raw, &me, &[], how, &ring)?))
    }
    /// For a draft to be encrypted: the address it is sealed to and whether
    /// with S/MIME, or `None` when this device has no key of yours for it.
    /// The sender comes from the message without its attachments, which
    /// aren't read for it.
    pub(super) fn draft_seal(&self, a: &Account, d: &Draft) -> Result<Option<(String, bool)>> {
        let bare = Draft {
            attachments: Vec::new(),
            ..d.clone()
        };
        let me = crate::sync::smtp::build(a, &bare, true)?
            .envelope()
            .from()
            .map(|f| f.to_string())
            .unwrap_or_default();
        let ring = self.keyring();
        if d.smime || ring.secret.is_empty() {
            return Ok(self.certificates().has_own(&me).then_some((me, true)));
        }
        Ok(ring.own_key(&me).is_some().then_some((me, false)))
    }
    /// After a draft to be encrypted couldn't be sealed: its earlier copy on
    /// the server (perhaps readable) is no longer it, so the draft stays on
    /// this device alone. The caller removes that copy first.
    pub(super) fn keep_draft_here(&self, d: &Draft) -> Result<()> {
        if let Some(mut current) = self.store.draft(&d.id)? {
            current.remote = None;
            current.uploaded.clear();
            self.store.update_draft(&current)?;
        }
        Ok(())
    }
    /// An id for a draft found on the server: the one its header names,
    /// unless that is taken by a draft here (perhaps another account's; a
    /// server chooses the header), or not usable.
    pub(super) fn draft_id_for(&self, named: Option<&str>) -> Result<String> {
        if let Some(id) = named
            && !id.is_empty()
            && id.len() <= 200
            && self.store.draft(id)?.is_none()
        {
            return Ok(id.to_owned());
        }
        Ok(uuid::Uuid::new_v4().to_string())
    }
    /// A file the Android picker copied into the app's cache (`picked/<one
    /// pick>/<name>`, pruned after a week) moved among draft `id`'s own
    /// files, which last as long as the draft. Other paths are left as they are.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn adopt_picked(&self, id: &str, path: std::path::PathBuf) -> std::path::PathBuf {
        let picked = path
            .parent()
            .and_then(|p| p.parent())
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "picked"));
        let Some(name) = path.file_name().filter(|_| picked && path.is_file()) else {
            return path;
        };
        let dir = self.draft_files(id);
        if std::fs::create_dir_all(&dir).is_err() {
            return path;
        }
        let mut used: HashSet<String> = std::fs::read_dir(&dir)
            .map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
            .unwrap_or_default();
        let target = dir.join(distinct_name(&name.to_string_lossy(), &mut used));
        let moved = std::fs::rename(&path, &target).or_else(|_| std::fs::copy(&path, &target).map(|_| ()));
        match moved {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::remove_dir_all(parent);
                }
                target
            }
            Err(_) => path,
        }
    }
    /// Saves bytes pasted into draft `id` (a copied picture) among its own
    /// files, under `name` (made distinct), returning the file.
    pub fn save_pasted(&self, id: &str, name: &str, bytes: &[u8]) -> Result<std::path::PathBuf> {
        let dir = self.draft_files(id);
        std::fs::create_dir_all(&dir)?;
        let mut used: HashSet<String> = std::fs::read_dir(&dir)
            .map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
            .unwrap_or_default();
        let target = dir.join(distinct_name(name, &mut used));
        std::fs::write(&target, bytes)?;
        Ok(target)
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
        self.store.path.join("draft-files").join(draft_folder(id))
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
        if a.provider == Provider::Proton {
            return self.sync_drafts_proton(a);
        }
        if a.provider == Provider::Jmap {
            return self.sync_drafts_jmap(a);
        }
        self.with_imap(a, |c| {
            // Copies for Sent that couldn't be saved when sent go first; a
            // failure leaves them for the next check without holding up drafts.
            let _ = self.file_queued_sent_copies(c, a);
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
            self.clear_draft_removals(&a.id, &pending)?;
            // What the server has, and which of Picacho's drafts each is.
            let mut on_server: Vec<u32> = c.uid_search("ALL")?.into_iter().collect();
            on_server.sort_unstable();
            let present: HashSet<u32> = on_server.iter().copied().collect();
            let mut tagged: HashMap<u32, String> = HashMap::new();
            let mut sizes: HashMap<u32, u32> = HashMap::new();
            if !on_server.is_empty() {
                let items = format!(
                    "(UID RFC822.SIZE BODY.PEEK[HEADER.FIELDS ({})])",
                    crate::sync::smtp::DRAFT_HEADER
                );
                for row in uid_fetch_rows(&mut c.session, &uid_ranges(&on_server), &items)? {
                    if let Some(size) = row.size {
                        sizes.insert(row.uid, size);
                    }
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
                if here(&remote) && present.contains(&remote.uid) {
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
            // Drafts written elsewhere. Which server copies and ids drafts
            // here already have, looked up rather than searched for each.
            let claimed_uids: HashSet<u32> = locals
                .iter()
                .filter_map(|d| d.remote.as_ref().filter(|r| here(r)).map(|r| r.uid))
                .collect();
            let local_ids: HashSet<&str> = locals.iter().map(|d| d.id.as_str()).collect();
            let mut found = Vec::new();
            for &uid in &on_server {
                if claimed_uids.contains(&uid) || tagged.get(&uid).is_some_and(|id| local_ids.contains(id.as_str())) {
                    continue;
                }
                // Too large to open: skipped before it is downloaded, without
                // holding up the drafts after it.
                if sizes.get(&uid).is_some_and(|&n| n as usize > MAX_MESSAGE) {
                    continue;
                }
                let rows = uid_fetch_rows(&mut c.session, &uid.to_string(), "(UID RFC822.SIZE BODY.PEEK[])")?;
                let Some(raw) = rows.into_iter().find(|r| r.uid == uid).and_then(|r| r.header) else {
                    continue;
                };
                if raw.len() > MAX_MESSAGE {
                    continue;
                }
                // The id the copy names (a server sets it) is taken only when
                // no draft here, of any account, has it already.
                let id = self.draft_id_for(tagged.get(&uid).map(String::as_str))?;
                let mut d = self.draft_from(a, &id, &raw)?;
                d.remote = Some(RemoteDraft {
                    mailbox: mailbox.clone(),
                    validity: selected.validity,
                    uid,
                });
                d.uploaded = d.fingerprint();
                self.store.save_draft(&d)?;
                found.push(d);
                changed += 1;
            }
            locals.extend(found);
            // Drafts written or changed here.
            let limit = c.extensions()?.append_limit;
            // UIDs only grow: anything uploaded from here on is above this.
            let mut floor = on_server.last().copied().unwrap_or(0);
            for d in &locals {
                let fingerprint = d.fingerprint();
                if d.remote.is_some() && d.uploaded == fingerprint {
                    continue;
                }
                // Mail to be encrypted goes up encrypted, or not at all.
                let Some(raw) = self.draft_upload(a, d)? else {
                    // Its earlier copy (perhaps readable) is no longer it.
                    if let Some(old) = d.remote.as_ref().filter(|r| here(r)) {
                        c.erase(&[old.uid])?;
                        self.keep_draft_here(d)?;
                    }
                    continue;
                };
                if limit.is_some_and(|l| raw.len() as u64 > l) {
                    // Too large for the server; it stays here.
                    continue;
                }
                let wire = c.wire(&mailbox);
                let flags = [imap::types::Flag::Seen, imap::types::Flag::Draft];
                // Its copy here replaced in one step (REPLACE, RFC 8508), so
                // no moment has two copies or none; else append, then remove.
                let previous = d.remote.as_ref().filter(|r| here(r)).map(|r| r.uid);
                let replaced = match previous {
                    Some(old) => c.replace(old, &wire, &raw, &flags).unwrap_or(None),
                    None => None,
                };
                let given = replaced
                    .as_deref()
                    .and_then(crate::sync::transport::append_uid)
                    .filter(|(v, _)| *v == selected.validity)
                    .map(|(_, uid)| uid);
                if replaced.is_none() {
                    c.session.append_with_flags(&wire, &raw, &flags)?;
                }
                // The new copy is the newest message carrying this draft's
                // id: mail another app added meanwhile can't be taken for it.
                let esearch = c.extensions()?.esearch;
                let copies = draft_copies(&mut c.session, esearch, &d.id, floor)?;
                let Some(uid) = given.or_else(|| copies.iter().copied().filter(|u| *u > floor).max())
                else {
                    bail!("The server didn't keep the uploaded draft.");
                };
                floor = floor.max(uid);
                // Earlier copies of this draft, by its id or where it was.
                let mut old: Vec<u32> = copies
                    .iter()
                    .copied()
                    .chain(
                        tagged
                            .iter()
                            .filter(|(_, id)| **id == d.id)
                            .map(|(u, _)| *u),
                    )
                    .chain(d.remote.as_ref().filter(|r| here(r)).map(|r| r.uid))
                    .filter(|u| *u != uid)
                    // Replaced (REPLACE): already gone from the server.
                    .filter(|u| replaced.is_none() || Some(*u) != previous)
                    .collect();
                old.sort_unstable();
                old.dedup();
                c.erase(&old)?;
                let remote = RemoteDraft {
                    mailbox: mailbox.clone(),
                    validity: selected.validity,
                    uid,
                };
                if !self.finish_draft_upload(d, remote.clone(), fingerprint)? && c.erase(&[uid]).is_err() {
                    // Discarded while uploading: its new copy goes too.
                    self.queue_draft_removal(&a.id, remote)?;
                }
            }
            Ok(changed)
        })
    }
    /// A draft from a message in the server's Drafts folder.
    fn draft_from(&self, a: &Account, id: &str, raw: &[u8]) -> Result<Draft> {
        // A draft sealed with your OpenPGP key (`draft_upload`).
        // Sealed with your S/MIME certificate.
        if crate::manage::crypto::sender_encrypted(raw)
            && crate::manage::crypto::encrypted_part(raw).is_none()
        {
            let (plain, verdict, _) = crate::manage::smime::open(raw, &self.certificates())
                .context("A draft on the server is encrypted to a certificate this device doesn't have.")?;
            ensure!(verdict.decrypted, "A draft on the server couldn't be decrypted.");
            let mut d = self.draft_from(a, id, &plain)?;
            d.encrypt = true;
            d.smime = true;
            return Ok(d);
        }
        let sealed = crate::manage::crypto::encrypted_part(raw).is_some();
        if sealed {
            let ring = self.keyring();
            let (plain, verdict) = crate::manage::openpgp::open(raw, &ring)
                .context("A draft on the server is encrypted to a key this device doesn't have.")?;
            ensure!(verdict.decrypted, "A draft on the server couldn't be decrypted.");
            let mut d = self.draft_from(a, id, &plain)?;
            d.encrypt = true;
            d.encrypt_subject = mailparse::parse_headers(raw).is_ok_and(|(h, _)| {
                use mailparse::MailHeaderMap;
                h.get_first_value("Subject").as_deref() == Some(crate::manage::openpgp::HIDDEN_SUBJECT)
            });
            return Ok(d);
        }
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
        if !m.attachments().is_empty() {
            let dir = self.draft_files(id);
            std::fs::create_dir_all(&dir)?;
            let mut leaves = Vec::new();
            collect_leaves(&parsed, &mut leaves);
            // Names already given, lowercase as case-insensitive disks see them.
            let mut used = std::collections::HashSet::new();
            for (i, attachment) in m.attachments().iter().enumerate() {
                let Some(leaf) = leaves.get(attachment.part) else {
                    continue;
                };
                let name: String = std::path::Path::new(&attachment.name)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty() && n != "." && n != "..")
                    .unwrap_or_else(|| format!("attachment-{}", i + 1));
                // Two attachments of one name each keep their own file.
                let name = distinct_name(&name, &mut used);
                let path = dir.join(name);
                std::fs::write(&path, leaf.get_body_raw()?)?;
                d.attachments.push(path);
            }
        }
        Ok(d)
    }
    pub fn attachment(&self, a: &Account, m: &Mail, part: usize) -> Result<Vec<u8>> {
        // A message too large to download whole: just this part, from the
        // server, under the account's lock. One downloaded already needs no
        // lock: saving its attachment waited behind a whole sync.
        if let Some(attachment) = m.attachments().iter().find(|x| x.part == part && !x.section.is_empty())
            && !self.store.has_raw(&m.id)?
        {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            let attachment = attachment.clone();
            if a.provider == Provider::Proton {
                return self.proton_attachment(a, m, &attachment.section);
            }
            if a.provider == Provider::Jmap {
                return self.jmap_attachment(a, &attachment);
            }
            return self.with_imap(a, |c| {
                c.select_for(m)?;
                self.fetch_attachment(c, m, &attachment)
            });
        }
        let raw = self.store.raw(&m.id)?.context("Open this message first")?;
        // Decrypting may read the account's keys: briefly, under its lock.
        let (_, raw) = {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            self.plaintext(a, &raw)?
        };
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

/// The folder name of draft `id`'s files: the id itself when it is safe as
/// a name (Picacho's own, UUIDs), else a hash of it. Replacing what wasn't
/// safe with `_` gave "a.b" and "a_b" one folder, and a draft's id can come
/// from a server's copy, so one could share (or, discarded, delete)
/// another's files.
fn draft_folder(id: &str) -> String {
    let safe = !id.is_empty()
        && id.len() <= 100
        && !id.starts_with("h-")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if safe {
        return id.to_owned();
    }
    use sha2::Digest as _;
    let hash: String = sha2::Sha256::digest(id.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("h-{hash}")
}

/// `name`, or "name (2).ext" and so on when one in `used` is the same
/// ignoring case; the name returned is added to `used`.
fn distinct_name(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    let path = std::path::Path::new(name);
    let (stem, ext) = match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => (
            stem.to_string_lossy().into_owned(),
            format!(".{}", ext.to_string_lossy()),
        ),
        _ => (name.to_owned(), String::new()),
    };
    let mut candidate = name.to_owned();
    let mut n = 2;
    while !used.insert(candidate.to_lowercase()) {
        candidate = format!("{stem} ({n}){ext}");
        n += 1;
    }
    candidate
}
/// A cached message's UIDVALIDITY, from its id (`account:mailbox:validity:uid`).
fn validity_of(m: &Mail) -> u32 {
    m.id.rsplit(':').nth(1).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The length of the original as kept here.
fn original_len(m: &Mail, b: &Backend) -> usize {
    b.store.raw(&m.id).ok().flatten().map_or(0, |r| r.len())
}

/// Attachments adding up to this make a message worth sending by
/// reference (BURL), so it crosses the network once.
const BURL_FROM: u64 = 1024 * 1024;
/// A sent message larger than the server keeps (APPENDLIMIT).
#[derive(Debug)]
struct TooLargeForSent(u64);
impl std::fmt::Display for TooLargeForSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "it's larger than the server keeps ({} KB)",
            self.0 / 1024
        )
    }
}
impl std::error::Error for TooLargeForSent {}

/// A message's Message-ID header, or empty.
fn message_id_of(raw: &[u8]) -> String {
    use mailparse::MailHeaderMap;
    mailparse::parse_headers(raw)
        .ok()
        .and_then(|(headers, _)| headers.get_first_value("Message-ID"))
        .map(|id| id.trim().to_owned())
        .unwrap_or_default()
}

/// Held while a pending list of draft removals is read and written back.
static DRAFT_REMOVALS: Mutex<()> = Mutex::new(());

/// The UIDs of the selected Drafts folder's copies of draft `id`, by its
/// `X-Picacho-Draft` header. The server is asked to search for the header;
/// one that finds nothing (some don't search arbitrary headers) has the
/// messages above `floor` read instead, keeping those that carry the id.
fn draft_copies(session: &mut ImapSession, esearch: bool, id: &str, floor: u32) -> Result<Vec<u32>> {
    let header = crate::sync::smtp::DRAFT_HEADER;
    let found = match uid_search_any(session, esearch, &format!("HEADER {header} {}", quote(id)?)) {
        Ok(found) => found,
        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => Default::default(),
        Err(e) => return Err(e.into()),
    };
    if !found.is_empty() {
        let mut found: Vec<u32> = found.into_iter().collect();
        found.sort_unstable();
        return Ok(found);
    }
    // `n:*` always includes the highest UID, even below n.
    let newer: Vec<u32> = uid_search_any(session, esearch, &format!("UID {}:*", floor.saturating_add(1)))?
        .into_iter()
        .filter(|u| *u > floor)
        .collect();
    if newer.is_empty() {
        return Ok(vec![]);
    }
    let items = format!("(UID BODY.PEEK[HEADER.FIELDS ({header})])");
    let mut out: Vec<u32> = uid_fetch_rows(session, &uid_ranges(&newer), &items)?
        .into_iter()
        .filter(|row| newer.contains(&row.uid) && draft_id(row.header.as_deref()) == Some(id))
        .map(|row| row.uid)
        .collect();
    out.sort_unstable();
    Ok(out)
}
/// The draft id in a fetched `X-Picacho-Draft` header, if any.
fn draft_id(header: Option<&[u8]>) -> Option<&str> {
    let text = std::str::from_utf8(header?).ok()?;
    let (name, value) = text.split_once(':')?;
    name.trim()
        .eq_ignore_ascii_case(crate::sync::smtp::DRAFT_HEADER)
        .then(|| value.trim())
        .filter(|v| !v.is_empty())
}
/// What a flags-only FETCH asks for: with Gmail, its labels too.
fn flag_items(gmail: bool) -> &'static str {
    if gmail {
        "(UID FLAGS X-GM-LABELS)"
    } else {
        "(UID FLAGS)"
    }
}
/// A flag update from a FETCH row, for the cache.
fn flag_update(r: &crate::sync::transport::Row, gmail: bool) -> store::FlagUpdate {
    store::FlagUpdate {
        uid: r.uid,
        unread: !r.seen(),
        starred: r.flagged(),
        labels: r.labels(gmail),
        answered: r.answered(),
        forwarded: r.forwarded(),
        mdn_sent: r.mdn_sent(),
    }
}
/// UIDs of the selected mailbox's messages within the account's sync
/// window (only flagged ones for Starred), oldest first.
/// With ESEARCH the reply is compact (`1:90000`); without, a very large
/// mailbox is searched in windows of UIDs (see `search_uids`), so neither
/// outgrows the connection's size limit.
fn search(c: &mut Connection, a: &Account, starred_only: bool) -> Result<Vec<u32>> {
    let mut query = if starred_only { "FLAGGED" } else { "ALL" }.to_owned();
    if a.sync.days > 0 {
        // Older mail stays on the server; reconciling drops it from the cache.
        let since = chrono::Utc::now() - chrono::Duration::days(a.sync.days.into());
        query = format!("{query} SINCE {}", since.format("%-d-%b-%Y"));
    }
    let ext = c.extensions()?;
    let selected = c.current().cloned().unwrap_or_default();
    search_uids(&mut c.session, None, &ext, &selected, &query)
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

#[cfg(test)]
mod layout_tests {
    use super::*;
    use std::prelude::v1::test;

    /// Only the latest decrypted messages keep their text in memory; the
    /// rest keep their previews, and lists get previews only.
    #[test]
    fn decrypted_messages_keep_few_bodies() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-readable-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        for i in 0..40 {
            b.keep_readable(Mail {
                id: format!("m{i}"),
                body: Some("Secret text".into()),
                html: Some("<p>Secret text</p>".into()),
                snippet: "Secret".into(),
                ..Mail::default()
            });
        }
        let bare = |id: &str| Mail { id: id.into(), ..Mail::default() };
        let old = b.with_readable(vec![bare("m0")]).remove(0);
        assert!(old.body.is_none() && old.html.is_none());
        assert_eq!(old.snippet, "Secret");
        let latest = b.with_readable(vec![bare("m39")]).remove(0);
        assert_eq!(latest.body.as_deref(), Some("Secret text"));
        let listed = b.with_readable_previews(vec![bare("m39")]).remove(0);
        assert!(listed.body.is_none() && listed.html.is_none());
        assert_eq!(listed.snippet, "Secret");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// A mailbox laid out again the same way, or checked with nothing new
    /// or gone, keeps its layout (by identity) and the list revision, so
    /// the list shown from it isn't rebuilt; a real change moves both on.
    #[test]
    fn unchanged_layouts_keep_their_identity() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-layout-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let a = Account::new(Provider::Imap);
        let inbox = Mailbox::Standard(Folder::Inbox);
        b.set_test_layout(&a, &inbox, "INBOX", 1, vec![1, 2, 3]);
        let (first, revision) = (b.layout(&a.id, &inbox).unwrap(), b.list_revision(&a.id));
        b.set_test_layout(&a, &inbox, "INBOX", 1, vec![1, 2, 3]);
        b.extend_layout(&a, &inbox, 1, std::iter::empty(), &[]);
        b.extend_layout(&a, &inbox, 1, [2].into_iter(), &[9]);
        let same = b.layout(&a.id, &inbox).unwrap();
        assert!(Arc::ptr_eq(&first.uids, &same.uids));
        assert_eq!(b.list_revision(&a.id), revision);
        b.extend_layout(&a, &inbox, 1, [4].into_iter(), &[1]);
        let next = b.layout(&a.id, &inbox).unwrap();
        assert_eq!(*next.uids, [2, 3, 4]);
        assert_ne!(b.list_revision(&a.id), revision);
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// A server's draft naming the id of a draft here (another account's)
    /// gets an id of its own rather than replacing it; a draft discarded
    /// while it uploads isn't saved back; removals queued meanwhile stay.
    #[test]
    fn server_drafts_never_replace_or_revive_drafts_here() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-draft-ids-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let other = Draft {
            id: "shared-id".into(),
            account: "other".into(),
            body: "Mine".into(),
            ..Draft::default()
        };
        b.store.save_draft(&other)?;
        let id = b.draft_id_for(Some("shared-id"))?;
        assert_ne!(id, "shared-id");
        assert_eq!(b.draft_id_for(Some("fresh"))?, "fresh");
        assert_eq!(b.store.draft("shared-id")?.unwrap().body, "Mine");
        // Discarded while uploading: the upload's result isn't saved.
        let remote = |uid| RemoteDraft {
            mailbox: "Drafts".into(),
            validity: 1,
            uid,
        };
        b.discard_draft("shared-id")?;
        assert!(!b.finish_draft_upload(&other, remote(7), "x".into())?);
        assert!(b.store.draft("shared-id")?.is_none());
        // A removal queued while a sync works through earlier ones stays.
        b.queue_draft_removal("other", remote(1))?;
        // Draft ids that differ have folders that differ.
        assert_eq!(draft_folder("6f1c-uuid"), "6f1c-uuid");
        assert_ne!(draft_folder("a.b"), draft_folder("a_b"));
        assert_ne!(draft_folder("h-x"), "h-x");
        assert!(draft_folder("../../etc").starts_with("h-"));
        let seen = b.draft_removals("other");
        b.queue_draft_removal("other", remote(2))?;
        b.clear_draft_removals("other", &seen)?;
        assert_eq!(b.draft_removals("other"), vec![remote(2)]);
        drop(b);
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// A Proton account set not to ask for changes gets no event watcher;
    /// the regular check finds its new mail.
    #[test]
    fn proton_events_turned_off_start_no_watcher() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-events-{}", uuid::Uuid::new_v4()));
        let b = Arc::new(Backend::new(Arc::new(Store::open(&path)?)));
        let mut a = Account::new(Provider::Proton);
        a.sync.proton_events_seconds = 0;
        b.watch_inbox(&a, |_| {});
        assert!(b.watching().is_empty());
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }
}
