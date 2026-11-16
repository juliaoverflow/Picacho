//! Proton Mail without Bridge: what the account operations do when an
//! account talks to Proton's API (`sync/proton.rs`) instead of IMAP/SMTP.
//!
//! A Proton message keeps one id wherever it goes, so it is one row here
//! (`{account}:proton:{id}`), in the mailbox its folder says: Inbox, Sent,
//! Archive, Spam, Trash, one of the account's own folders, or Drafts.
//! Starred is a label, and Proton's other labels are the row's labels.
//! Moving a message relabels it at Proton and changes its row, rather than
//! removing it and finding it again as IMAP does.
//!
//! Opened messages are decrypted on this computer and cached as standard
//! MIME, as mail through Proton Mail Bridge is, so search, attachments and
//! offline reading work the same.
use super::*;
use crate::sync::proton::{self, Session};
use protected_email_bridge::api::types::{MessageMetadata, label_id};

/// What a Proton sign-in needs next, or the account it added.
pub enum ProtonNext {
    /// A second factor: an authenticator app's code (`code`), and/or a
    /// security key or passkey (`key`).
    SecondFactor { code: bool, key: bool },
    /// The mailbox password (two-password mode).
    MailboxPassword,
    /// Signed in, and the account saved.
    Done(Box<Account>),
}

/// A sign-in between its steps.
pub(super) struct ProtonLogin {
    account: Account,
    taken: Vec<String>,
    login: proton::Login,
}

/// Proton lists at most this many messages per request.
const PAGE: usize = 150;
/// The most messages of one mailbox a server search goes through.
const SEARCH_MOST: usize = 20_000;

/// A Proton draft, as a `RemoteDraft` (which IMAP fills with its mailbox,
/// UIDVALIDITY and UID): the draft's id, after `proton:`.
fn proton_remote(id: &str) -> RemoteDraft {
    RemoteDraft {
        mailbox: format!("proton:{id}"),
        validity: 0,
        uid: 0,
    }
}
fn proton_draft(r: &RemoteDraft) -> Option<&str> {
    r.mailbox.strip_prefix("proton:")
}

/// The Proton message id of one of our rows.
pub(super) fn proton_id<'a>(a: &Account, m: &'a Mail) -> Option<&'a str> {
    m.id.strip_prefix(&a.id)?.strip_prefix(":proton:")
}
pub(super) fn row_id(a: &Account, id: &str) -> String {
    format!("{}:proton:{id}", a.id)
}
fn ids(a: &Account, mails: &[Mail]) -> Vec<String> {
    mails
        .iter()
        .filter_map(|m| proton_id(a, m).map(str::to_owned))
        .collect()
}

/// The Proton label behind a mailbox.
pub(super) fn label_for(folders: &[proton::Folder], mailbox: &Mailbox) -> Result<String> {
    Ok(match mailbox {
        Mailbox::Standard(folder) => match folder {
            Folder::Inbox => label_id::INBOX,
            Folder::Starred => label_id::STARRED,
            Folder::Sent => label_id::SENT,
            Folder::Drafts => label_id::DRAFTS,
            Folder::Archive => label_id::ARCHIVE,
            Folder::Junk => label_id::SPAM,
            Folder::Trash => label_id::TRASH,
            Folder::All => label_id::ALL_MAIL,
            Folder::Other => bail!("Choose one of the account's folders."),
        }
        .to_owned(),
        Mailbox::Remote(name) => folders
            .iter()
            .find(|f| f.exclusive && &f.path == name)
            .map(|f| f.id.clone())
            .with_context(|| format!("The folder “{name}” no longer exists at Proton."))?,
    })
}

/// Where a message is, from its labels: (folder, its name).
pub(super) fn location(labels: &[String], folders: &[proton::Folder]) -> (Folder, String) {
    let has = |id: &str| labels.iter().any(|l| l == id);
    for (id, folder, name) in [
        (label_id::INBOX, Folder::Inbox, "Inbox"),
        (label_id::TRASH, Folder::Trash, "Trash"),
        (label_id::SPAM, Folder::Junk, "Spam"),
        (label_id::DRAFTS, Folder::Drafts, "Drafts"),
        (label_id::SENT, Folder::Sent, "Sent"),
        (label_id::ARCHIVE, Folder::Archive, "Archive"),
    ] {
        if has(id) {
            return (folder, name.to_owned());
        }
    }
    if let Some(f) = folders.iter().find(|f| f.exclusive && has(&f.id)) {
        return (Folder::Other, f.path.clone());
    }
    if has(label_id::ALL_DRAFTS) {
        return (Folder::Drafts, "Drafts".into());
    }
    if has(label_id::ALL_SENT) {
        return (Folder::Sent, "Sent".into());
    }
    (Folder::Archive, "Archive".into())
}

fn address(a: &protected_email_bridge::api::types::Addr) -> String {
    if a.name.trim().is_empty() {
        a.address.clone()
    } else {
        format!("{} <{}>", a.name.trim(), a.address)
    }
}
fn addresses(list: &[protected_email_bridge::api::types::Addr]) -> String {
    list.iter().map(address).collect::<Vec<_>>().join(", ")
}

/// The account's own folders, as the folder list shows them.
pub(super) fn remote_folders(folders: &[proton::Folder]) -> Vec<RemoteFolder> {
    folders
        .iter()
        .filter(|f| f.exclusive)
        .map(|f| RemoteFolder {
            name: f.path.clone(),
            label: f.path.rsplit('/').next().unwrap_or(&f.path).to_owned(),
            depth: f.depth,
            nestable: true,
        })
        .collect()
}

impl Backend {
    /// Starts a Proton sign-in with the password (step one). Returns what
    /// Proton needs next; the sign-in is kept here between steps.
    pub fn proton_begin(
        &self,
        account: &Account,
        password: String,
        taken: &[String],
        cancel: Arc<AtomicBool>,
    ) -> Result<ProtonNext> {
        self.proton_abandon();
        ensure!(proton::available(), "{}", proton::UNAVAILABLE);
        ensure!(!password.is_empty(), "Enter your Proton password.");
        account.validate()?;
        self.step(&cancel, SignInStep::Mailbox);
        let login = proton::Login::begin(account.login(), &password, &cancel, &|| {
            self.step(&cancel, SignInStep::Verifying)
        })?;
        self.step(&cancel, SignInStep::Mailbox);
        ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
        self.proton_advance(
            ProtonLogin {
                account: account.clone(),
                taken: taken.to_vec(),
                login,
            },
            &cancel,
        )
    }
    /// The code from an authenticator app (step two).
    pub fn proton_code(&self, code: String, cancel: Arc<AtomicBool>) -> Result<ProtonNext> {
        let mut p = self.proton_waiting()?;
        if let Err(e) = p.login.submit_code(&code) {
            self.proton_keep(p);
            return Err(e);
        }
        self.proton_advance(p, &cancel)
    }
    /// A security key or passkey (step two). Fails with `NeedsPin` when the
    /// key wants its PIN.
    pub fn proton_key(
        &self,
        method: KeyMethod,
        pin: Option<String>,
        cancel: Arc<AtomicBool>,
    ) -> Result<ProtonNext> {
        let mut p = self.proton_waiting()?;
        let result = p
            .login
            .submit_key(method, pin, self.proton_prompt.clone(), &cancel);
        *self.proton_prompt.lock().unwrap() = KeyPrompt::Idle;
        if let Err(e) = result {
            self.proton_keep(p);
            return Err(e);
        }
        self.proton_advance(p, &cancel)
    }
    /// The mailbox password (step three). A wrong one can be tried again.
    pub fn proton_mailbox(&self, password: String, cancel: Arc<AtomicBool>) -> Result<ProtonNext> {
        let mut p = self.proton_waiting()?;
        match p.login.finish(Some(&password)) {
            Ok(signed) => self
                .save_proton(p, signed, &cancel)
                .map(|a| ProtonNext::Done(Box::new(a))),
            Err(e) => {
                self.proton_keep(p);
                Err(e)
            }
        }
    }
    /// Keeps a sign-in whose step failed, to try that step again, unless
    /// the failure ended it: then it starts again with the password.
    fn proton_keep(&self, p: ProtonLogin) {
        if !p.login.ended() {
            *self.proton_login.lock().unwrap() = Some(p);
        }
    }
    /// Gives up on the sign-in in progress, signing its session out.
    pub fn proton_abandon(&self) {
        let p = self.proton_login.lock().unwrap().take();
        if let Some(p) = p {
            p.login.abort();
        }
    }
    /// What a security key or phone is waiting for, while one is used.
    pub fn proton_prompt(&self) -> KeyPrompt {
        self.proton_prompt.lock().unwrap().clone()
    }
    fn proton_waiting(&self) -> Result<ProtonLogin> {
        self.proton_login
            .lock()
            .unwrap()
            .take()
            .context("This sign-in has ended. Start again with your password.")
    }
    /// Keeps the sign-in for its next step, or finishes it.
    fn proton_advance(&self, mut p: ProtonLogin, cancel: &AtomicBool) -> Result<ProtonNext> {
        match p.login.next()? {
            proton::Next::SecondFactor { code, key } => {
                *self.proton_login.lock().unwrap() = Some(p);
                Ok(ProtonNext::SecondFactor { code, key })
            }
            proton::Next::MailboxPassword => {
                *self.proton_login.lock().unwrap() = Some(p);
                Ok(ProtonNext::MailboxPassword)
            }
            proton::Next::Ready => {
                let signed = p.login.finish(None)?;
                self.save_proton(p, signed, cancel)
                    .map(|a| ProtonNext::Done(Box::new(a)))
            }
        }
    }
    /// Saves the signed-in account, with its session and the passphrase
    /// that unlocks its keys in the keychain.
    fn save_proton(
        &self,
        p: ProtonLogin,
        signed: proton::SignedIn,
        cancel: &AtomicBool,
    ) -> Result<Account> {
        let ProtonLogin {
            mut account, taken, ..
        } = p;
        if cancel.load(Ordering::Relaxed) {
            proton::sign_out(&signed.saved);
            bail!("Sign-in cancelled.");
        }
        let existing = self
            .store
            .accounts()?
            .into_iter()
            .find(|x| x.id == account.id);
        let owned = |email: &str| {
            signed
                .addresses
                .iter()
                .any(|x| x.eq_ignore_ascii_case(email))
        };
        if !owned(&account.email) {
            // Signed in with a user name, or an address that isn't on: use
            // the primary address, unless this reconnects another one.
            if existing.is_some() || signed.addresses.is_empty() {
                proton::sign_out(&signed.saved);
                bail!(
                    "That Proton account doesn't have {}. Sign in to the account it belongs to.",
                    account.email
                );
            }
            account.email = signed.addresses[0].clone();
        }
        if account.name.is_empty() || account.name == account.email {
            account.name = if signed.name.is_empty() {
                account.email.clone()
            } else {
                signed.name.clone()
            };
        }
        let result = (|| {
            account.validate()?;
            ensure!(
                !taken.iter().any(|e| e.eq_ignore_ascii_case(&account.email)),
                "{} is already added.",
                account.email
            );
            let lock = self.account_lock(&account.id);
            let _operation = lock.lock().unwrap();
            // A replaced sign-in must not keep using the old session.
            let old = self.proton.lock().unwrap().remove(&account.id);
            drop(old);
            let previous = existing
                .as_ref()
                .and_then(|_| self.saved_credentials(&account.id).ok())
                .map(|s| s.proton());
            let mut secret = Secret::default();
            secret.set_proton(&signed.saved);
            self.save_credentials(&account.id, &secret)?;
            self.store.save_account(&account)?;
            if let Some(previous) =
                previous.filter(|p| p.uid != signed.saved.uid && !p.uid.is_empty())
            {
                proton::sign_out(&previous);
            }
            Ok(())
        })();
        if let Err(e) = result {
            proton::sign_out(&signed.saved);
            return Err(e);
        }
        Ok(account)
    }
    /// The account's Proton session, resumed (and its keys unlocked) on
    /// first use, then kept in memory.
    fn proton_session(&self, a: &Account) -> Result<Arc<Session>> {
        if let Some(s) = self.proton.lock().unwrap().get(&a.id) {
            return Ok(s.clone());
        }
        let opening = self
            .proton_opening
            .lock()
            .unwrap()
            .entry(a.id.clone())
            .or_default()
            .clone();
        let _opening = opening.lock().unwrap_or_else(|e| e.into_inner());
        // Another thread may have resumed it while this one waited.
        if let Some(s) = self.proton.lock().unwrap().get(&a.id) {
            return Ok(s.clone());
        }
        let secret = self.credentials(a)?;
        let session = Arc::new(Session::open(&secret.proton())?);
        self.keep_proton_tokens(a, &session);
        self.proton
            .lock()
            .unwrap()
            .insert(a.id.clone(), session.clone());
        Ok(session)
    }
    /// Runs `work` with the account's session, then saves any tokens Proton
    /// renewed meanwhile: each refresh replaces the refresh token, so an
    /// unsaved one would sign the account out at the next launch.
    pub(super) fn with_proton<T>(
        &self,
        a: &Account,
        work: impl FnOnce(&Session) -> Result<T>,
    ) -> Result<T> {
        let session = self.proton_session(a)?;
        let result = work(&session);
        self.keep_proton_tokens(a, &session);
        if let Err(e) = &result
            && e.downcast_ref::<AccessFailure>().is_some()
        {
            self.proton.lock().unwrap().remove(&a.id);
        }
        result
    }
    fn keep_proton_tokens(&self, a: &Account, session: &Session) {
        let Some(saved) = session.renewed() else {
            return;
        };
        #[cfg(test)]
        if self.test_secret.is_some() {
            return;
        }
        let kept = self.saved_credentials(&a.id).and_then(|mut secret| {
            secret.set_proton(&saved);
            self.save_credentials(&a.id, &secret)
        });
        if kept.is_err() {
            // Not saved: the keychain still has the replaced refresh token,
            // which would sign the account out at the next launch. Keep the
            // new tokens to save again after the next request.
            session.unsaved(saved);
        }
    }
    /// Ends the account's Proton session (it is being removed).
    pub(super) fn forget_proton(&self, a: &Account) {
        self.proton.lock().unwrap().remove(&a.id);
        if let Ok(secret) = self.saved_credentials(&a.id) {
            let saved = secret.proton();
            if !saved.uid.is_empty() {
                proton::sign_out(&saved);
            }
        }
    }

    /// A row for a listed message: the cached one with its flags, folder
    /// and labels brought up to date, or a new one from the listing.
    fn proton_mail(
        &self,
        a: &Account,
        folders: &[proton::Folder],
        s: &Session,
        m: &MessageMetadata,
    ) -> Result<Mail> {
        let id = row_id(a, &m.id);
        let (folder, remote) = location(&m.label_ids, folders);
        let mut mail = match self.store.message(&id)? {
            Some(cached) => cached,
            None => Mail {
                id,
                account: a.id.clone(),
                from: m.sender.address.clone(),
                sender: if m.sender.name.trim().is_empty() {
                    m.sender.address.clone()
                } else {
                    m.sender.name.trim().to_owned()
                },
                to: addresses(&m.to_list),
                reply_to: addresses(&m.reply_tos),
                subject: m.subject.clone(),
                date: m.time,
                message_id: if m.external_id.is_empty() {
                    String::new()
                } else {
                    format!("<{}>", m.external_id.trim_matches(['<', '>']))
                },
                has_attachments: m.num_attachments > 0,
                ..Default::default()
            },
        };
        mail.set_cc(addresses(&m.cc_list));
        mail.folder = folder;
        mail.remote_folder = remote;
        mail.unread = m.unread;
        mail.starred = m.is_starred();
        mail.size = u32::try_from(m.size).unwrap_or(u32::MAX);
        mail.answered = m.is_replied || m.is_replied_all;
        mail.forwarded = m.is_forwarded;
        let mut labels: Vec<String> = m
            .label_ids
            .iter()
            .filter(|l| folders.iter().any(|f| !f.exclusive && &f.id == *l))
            .filter_map(|l| {
                s.label(l)
                    .map(|l| if l.path.is_empty() { l.name } else { l.path })
            })
            .collect();
        labels.sort();
        mail.set_labels(labels);
        Ok(mail)
    }

    /// Brings a mailbox up to date: lists its newest `limit` messages, and
    /// asks again about cached ones that weren't listed (moved elsewhere,
    /// or deleted).
    pub(super) fn sync_proton(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        // Drafts are kept on this computer, as with IMAP accounts.
        if mailbox.folder() == Folder::Drafts {
            return Ok(vec![]);
        }
        // Changes made while offline reach Proton before anything is read.
        self.replay_offline_proton(a)?;
        self.with_proton(a, |s| {
            s.reload_folders()?;
            let folders = s.folders();
            let _ = self.store.save_folders(&a.id, &remote_folders(&folders));
            let label = label_for(&folders, mailbox)?;
            let since = (a.sync.days > 0)
                .then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut listed: Vec<MessageMetadata> = Vec::new();
            let mut complete = false;
            for page in 0.. {
                let batch = s.list(&label, page, PAGE)?;
                let full = batch.len() >= PAGE;
                let mut older = false;
                for m in batch {
                    if since.is_some_and(|since| m.time < since) {
                        older = true;
                    } else {
                        listed.push(m);
                    }
                }
                if !full || older {
                    complete = true;
                    break;
                }
                if listed.len() >= limit {
                    break;
                }
            }
            // Cut short: older mail in this mailbox wasn't listed.
            if listed.len() > limit {
                listed.truncate(limit);
                complete = false;
            }
            let mut rows = Vec::with_capacity(listed.len());
            for m in &listed {
                rows.push(self.proton_mail(a, &folders, s, m)?);
            }
            // What the cache has here that Proton didn't list: within the
            // listed range (or anywhere, when the whole mailbox was listed),
            // it has moved or gone.
            let oldest = rows.iter().map(|m| m.date).min().unwrap_or(i64::MIN);
            let listed_ids: std::collections::HashSet<&str> =
                rows.iter().map(|m| m.id.as_str()).collect();
            let unlisted: Vec<Mail> = self
                .store
                .messages(&a.id, mailbox.clone(), usize::MAX >> 1)?
                .into_iter()
                .filter(|m| proton_id(a, m).is_some() && !listed_ids.contains(m.id.as_str()))
                .filter(|m| complete || m.date >= oldest)
                .collect();
            if !unlisted.is_empty() {
                let current = s.metadata(&ids(a, &unlisted))?;
                let still: std::collections::HashSet<String> =
                    current.iter().map(|m| row_id(a, &m.id)).collect();
                for m in &current {
                    rows.push(self.proton_mail(a, &folders, s, m)?);
                }
                let gone: Vec<String> = unlisted
                    .iter()
                    .map(|m| m.id.clone())
                    .filter(|id| !still.contains(id))
                    .collect();
                if !gone.is_empty() {
                    self.store.delete_ids(&a.id, &gone)?;
                    let gone: std::collections::HashSet<&String> = gone.iter().collect();
                    self.readable
                        .lock()
                        .unwrap()
                        .retain(|id, _| !gone.contains(id));
                }
            }
            self.store.save_messages(&rows)?;
            self.store.newest(&a.id, mailbox.clone(), limit)
        })
    }

    /// Lists `batch` more of a mailbox's messages beyond those cached, page
    /// by page, newest first.
    pub(super) fn catalog_proton(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        batch: usize,
    ) -> Result<Catalog> {
        self.with_proton(a, |s| {
            let folders = s.folders();
            let label = label_for(&folders, mailbox)?;
            let total = s.count(&label)?;
            let cached = self.store.summaries(&a.id, mailbox.clone())?.len();
            if cached >= total {
                return Ok(Catalog {
                    cataloged: total,
                    total,
                    ..Default::default()
                });
            }
            let since = (a.sync.days > 0)
                .then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut added = Vec::new();
            let mut page = cached / PAGE;
            // Pages already cached (messages arrived or went since) are
            // passed over, a few at a time, until new ones turn up.
            let mut ended = false;
            for _ in 0..8 {
                let listed = s.list(&label, page, PAGE)?;
                let last = listed.len() < PAGE;
                for m in &listed {
                    if since.is_some_and(|since| m.time < since) {
                        ended = true;
                        continue;
                    }
                    if self.store.message(&row_id(a, &m.id))?.is_none() {
                        added.push(self.proton_mail(a, &folders, s, m)?);
                    }
                }
                if last || ended || added.len() >= batch.min(PAGE) {
                    ended |= last;
                    break;
                }
                page += 1;
            }
            self.store.save_messages(&added)?;
            let cataloged = cached + added.len();
            Ok(Catalog {
                // Nothing older to list: what is cached is all there is.
                cataloged: if ended || added.is_empty() {
                    total.max(cataloged)
                } else {
                    cataloged
                },
                total: if ended || added.is_empty() {
                    total.max(cataloged)
                } else {
                    total
                },
                added,
                layout: None,
            })
        })
    }

    /// Downloads and decrypts one message as MIME, and caches it.
    pub(super) fn download_proton(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let id = proton_id(a, m).context("This message isn't from Proton.")?;
        let raw = self.with_proton(a, |s| s.literal(id))?;
        ensure!(
            raw.len() <= large::MAX_DOWNLOAD,
            "Message exceeds the download limit."
        );
        self.note_traffic(a, raw.len());
        self.store.save_raw(&m.id, &raw)?;
        Ok(raw)
    }

    /// A message too large to download whole: its text is read alone, and
    /// each attachment downloads when it is saved (`proton_attachment`),
    /// as for one through IMAP.
    pub(super) fn read_proton_parts(&self, a: &Account, m: &Mail) -> Result<Mail> {
        let id = proton_id(a, m).context("This message isn't from Proton.")?;
        let (text, attachments) = self.with_proton(a, |s| s.text_only(id))?;
        let mut next = parse_message(&text, a, &m.id, m.folder, &m.remote_folder, m.uid)?;
        next.set_attachments(
            attachments
                .into_iter()
                .enumerate()
                .map(|(i, (id, name, size, mime))| Attachment {
                    name,
                    size,
                    mime,
                    // Past the text's own parts.
                    part: 10_000 + i,
                    section: id,
                    encoding: String::new(),
                })
                .collect()
        );
        next.has_attachments = !next.attachments().is_empty();
        next.partial = true;
        next.unread = m.unread;
        next.starred = m.starred;
        next.size = m.size;
        next.set_labels(m.labels().to_vec());
        Ok(next)
    }
    /// One attachment of a message too large to download whole.
    pub(super) fn proton_attachment(
        &self,
        a: &Account,
        m: &Mail,
        section: &str,
    ) -> Result<Vec<u8>> {
        let id = proton_id(a, m).context("This message isn't from Proton.")?;
        self.with_proton(a, |s| s.attachment(id, section))
    }

    /// Background downloads: up to `max` messages only listed so far.
    pub(super) fn download_batch_proton(
        &self,
        a: &Account,
        max: usize,
        since: i64,
    ) -> Result<usize> {
        let mut stored = 0;
        for m in self.store.missing(&a.id, MAX_MESSAGE, max.min(20), since)? {
            // Someone is opening a message: let them go first.
            if stored > 0 && self.someone_waiting(&a.id) {
                break;
            }
            let raw = self.download_proton(a, &m)?;
            if let Ok(mut parsed) = parse_message(&raw, a, &m.id, m.folder, &m.remote_folder, m.uid) {
                let mut next = m.clone();
                next.has_attachments = !parsed.attachments().is_empty();
                next.set_attachments(parsed.take_attachments());
                if next.snippet.is_empty() {
                    next.snippet = parsed.snippet;
                }
                if !parsed.message_id.is_empty() {
                    next.message_id = parsed.message_id;
                }
                self.store.save_messages(&[next])?;
            }
            stored += 1;
        }
        Ok(stored)
    }

    /// Makes a change at Proton.
    pub(super) fn change_proton(
        &self,
        a: &Account,
        current: &[Mail],
        change: &Change,
    ) -> Result<()> {
        let ids = ids(a, current);
        if ids.is_empty() {
            return Ok(());
        }
        self.with_proton(a, |s| match change {
            Change::Read(read) => s.mark_read(&ids, *read),
            Change::Star(star) => s.label_messages(label_id::STARRED, &ids, *star),
            Change::Move(folder) => {
                let target = match folder {
                    Folder::Inbox => label_id::INBOX,
                    Folder::Archive => label_id::ARCHIVE,
                    Folder::Junk => label_id::SPAM,
                    Folder::Trash => label_id::TRASH,
                    other => bail!("Proton doesn't move mail to {}.", other.label()),
                };
                s.label_messages(target, &ids, true)
            }
            Change::MoveTo(name) => {
                let folders = s.folders();
                let target = label_for(&folders, &Mailbox::Remote(name.clone()))?;
                s.label_messages(&target, &ids, true)
            }
            Change::Label(name, add) => {
                let found = s
                    .folders()
                    .into_iter()
                    .find(|f| !f.exclusive && f.path.eq_ignore_ascii_case(name))
                    .map(|f| f.id);
                let id = match (found, add) {
                    (Some(id), _) => id,
                    (None, true) => s.create_label(name, false, None)?,
                    (None, false) => return Ok(()),
                };
                s.label_messages(&id, &ids, *add)
            }
            Change::Mark(Mark::Forwarded) => s.mark_forwarded(&ids),
            // Proton marks replies itself when they are sent, and has no
            // flag for read receipts: these stay on this computer.
            Change::Mark(_) => Ok(()),
        })
    }

    /// Deletes messages for good at Proton.
    pub(super) fn delete_proton(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let ids = ids(a, mails);
        if !ids.is_empty() {
            self.with_proton(a, |s| s.delete(&ids))?;
        }
        self.store.commit_changes(mails, true)
    }

    /// Deletes everything in Trash or Spam for good.
    pub(super) fn empty_proton(&self, a: &Account, folder: Folder) -> Result<usize> {
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_proton(a, |s| {
            let label = label_for(&[], &mailbox)?;
            let mut count = 0;
            // Bounded: a mailbox refilling as it empties can't keep this going.
            for _ in 0..1000 {
                let page: Vec<String> =
                    s.list(&label, 0, PAGE)?.into_iter().map(|m| m.id).collect();
                if page.is_empty() {
                    break;
                }
                s.delete(&page)?;
                count += page.len();
            }
            Ok(count)
        })?;
        let cached = self.store.messages(&a.id, mailbox, usize::MAX >> 1)?;
        self.store.commit_changes(&cached, true)?;
        Ok(count)
    }

    /// Server search, as Proton Mail Bridge answers a mail app's IMAP
    /// SEARCH: every message of the mailbox is listed from Proton (up to
    /// `SEARCH_MOST`) and matched here, on what Proton says about it and on
    /// its contents where they are downloaded. Matches not yet listed here
    /// are kept like any other listed mail.
    pub(super) fn search_proton(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        query: &str,
        limit: usize,
    ) -> Result<ServerResults> {
        let search = crate::manage::search::Search::parse(query);
        self.with_proton(a, |s| {
            let folders = s.folders();
            let label = label_for(&folders, mailbox)?;
            let mut out = ServerResults::default();
            for m in s.list_all(&label, SEARCH_MOST)? {
                let row = self.proton_mail(a, &folders, s, &m)?;
                if search.matches(&row) {
                    out.total += 1;
                    if out.mails.len() < limit {
                        out.mails.push(row);
                    }
                }
            }
            self.store.save_messages(&out.mails)?;
            out.mails.sort_by(Mail::newest_first);
            Ok(out)
        })
    }

    /// Marks everything unread in a mailbox read at Proton, listed or not.
    pub(super) fn mark_all_read_proton(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        self.with_proton(a, |s| {
            let label = label_for(&s.folders(), mailbox)?;
            let unread: Vec<String> = s
                .list_all(&label, usize::MAX >> 1)?
                .into_iter()
                .filter(|m| m.unread)
                .map(|m| m.id)
                .collect();
            if !unread.is_empty() {
                s.mark_read(&unread, true)?;
            }
            Ok(())
        })?;
        let mut cached = self
            .store
            .messages(&a.id, mailbox.clone(), usize::MAX >> 1)?;
        cached.retain(|m| m.unread);
        for m in &mut cached {
            m.unread = false;
        }
        self.store.save_messages(&cached)
    }

    /// Saves a copy of a message sent through another server in Sent at
    /// Proton (an identity with its own outgoing server).
    pub(super) fn file_sent_copy_proton(&self, a: &Account, raw: &[u8]) -> Result<()> {
        self.with_proton(a, |s| s.import(raw, label_id::SENT, true).map(|_| ()))
    }
    /// Files Sent copies that couldn't be saved when they were sent.
    fn file_queued_sent_copies_proton(&self, a: &Account) -> Result<usize> {
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
            match self.file_sent_copy_proton(a, &raw) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&path);
                    filed += 1;
                }
                Err(e) if stale || !crate::sync::retry::later(&e) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(filed)
    }

    /// Keeps the account's drafts and Proton's Drafts in step, as Proton
    /// Mail Bridge does for a mail app's Drafts folder: drafts written here
    /// are uploaded (replacing their earlier copy), drafts written
    /// elsewhere appear here, and drafts sent or deleted on either side go
    /// from the other. A draft changed here since its last upload is never
    /// thrown away. Returns how many drafts changed here.
    pub(super) fn sync_drafts_proton(&self, a: &Account) -> Result<usize> {
        // Copies for Sent that couldn't be saved when sent go first; a
        // failure leaves them for the next check without holding up drafts.
        let _ = self.file_queued_sent_copies_proton(a);
        self.with_proton(a, |s| {
            // Drafts sent or deleted here: their copies at Proton go.
            let pending = self.draft_removals(&a.id);
            let gone: Vec<String> = pending
                .iter()
                .filter_map(proton_draft)
                .map(str::to_owned)
                .collect();
            if !gone.is_empty()
                && let Err(e) = s.delete(&gone)
                && crate::sync::retry::later(&e)
            {
                return Err(e);
            }
            if !pending.is_empty() {
                self.save_draft_removals(&a.id, &[])?;
            }
            let on_server: Vec<String> = s
                .list_all(label_id::DRAFTS, 1000)?
                .into_iter()
                .map(|m| m.id)
                .collect();
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            // Copies gone from Proton (sent or deleted elsewhere): unchanged
            // drafts go here too; changed ones upload again.
            for d in &mut locals {
                let Some(id) = d.remote.as_ref().and_then(proton_draft).map(str::to_owned) else {
                    continue;
                };
                if on_server.contains(&id) {
                    continue;
                }
                if d.uploaded == d.fingerprint() {
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
            for id in &on_server {
                if locals
                    .iter()
                    .any(|d| d.remote.as_ref().and_then(proton_draft) == Some(id.as_str()))
                {
                    continue;
                }
                let raw = s.literal(id)?;
                ensure!(
                    raw.len() <= MAX_MESSAGE,
                    "A draft at Proton is too large to open here."
                );
                let mut d = self.draft_from(a, &uuid::Uuid::new_v4().to_string(), &raw)?;
                d.remote = Some(proton_remote(id));
                d.uploaded = d.fingerprint();
                self.store.save_draft(&d)?;
                locals.push(d);
                changed += 1;
            }
            // Drafts written or changed here.
            for d in &locals {
                let fingerprint = d.fingerprint();
                if d.remote.is_some() && d.uploaded == fingerprint {
                    continue;
                }
                let raw = crate::sync::smtp::build(a, d, true)?.formatted();
                let id = s.save_draft(&raw)?;
                if let Some(old) = d.remote.as_ref().and_then(proton_draft) {
                    let _ = s.delete(&[old.to_owned()]);
                }
                // A composer may have saved a newer version meanwhile; it
                // keeps its words, and uploads again next time.
                let mut current = self.store.draft(&d.id)?.unwrap_or_else(|| d.clone());
                current.remote = Some(proton_remote(&id));
                current.uploaded = fingerprint;
                self.store.save_draft(&current)?;
            }
            Ok(changed)
        })
    }

    pub(super) fn quota_proton(&self, a: &Account) -> Result<Option<Quota>> {
        self.with_proton(a, |s| {
            let (used, max) = s.quota();
            Ok((max > 0).then_some(Quota {
                used_kib: used / 1024,
                limit_kib: max / 1024,
            }))
        })
    }

    /// Sends a finished message through Proton, which encrypts it where it
    /// can and keeps a copy in Sent.
    pub(super) fn send_proton(
        &self,
        a: &Account,
        from: &str,
        to: &[String],
        raw: &[u8],
    ) -> Result<()> {
        ensure!(!to.is_empty(), "Add at least one recipient.");
        self.with_proton(a, |s| s.send(from, to, raw))
    }

    // Folders.

    pub(super) fn create_folder_proton(
        &self,
        a: &Account,
        parent: Option<&str>,
        leaf: &str,
    ) -> Result<FolderChange> {
        self.with_proton(a, |s| {
            let folders = s.folders();
            let parent = match parent {
                Some(p) => Some(label_for(&folders, &Mailbox::Remote(p.to_owned()))?),
                None => None,
            };
            s.create_label(leaf, true, parent.as_deref())?;
            s.reload_folders()?;
            let folders = remote_folders(&s.folders());
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange {
                moved: Vec::new(),
                folders,
            })
        })
    }

    pub(super) fn rename_folder_proton(
        &self,
        a: &Account,
        name: &str,
        leaf: &str,
    ) -> Result<FolderChange> {
        let change = self.with_proton(a, |s| {
            let before = s.folders();
            let id = label_for(&before, &Mailbox::Remote(name.to_owned()))?;
            s.rename_label(&id, leaf)?;
            s.reload_folders()?;
            let after = s.folders();
            // The folder and those inside it, by id: (old path, new path).
            let moved: Vec<(String, String)> = before
                .iter()
                .filter(|f| f.exclusive)
                .filter_map(|old| {
                    let new = after.iter().find(|n| n.id == old.id)?;
                    (new.path != old.path).then(|| (old.path.clone(), new.path.clone()))
                })
                .collect();
            let folders = remote_folders(&after);
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange { moved, folders })
        })?;
        // Rows keep their ids; only the folder they show in is renamed.
        for (old, new) in &change.moved {
            let mut rows =
                self.store
                    .messages(&a.id, Mailbox::Remote(old.clone()), usize::MAX >> 1)?;
            for m in &mut rows {
                m.remote_folder = new.clone();
            }
            self.store.save_messages(&rows)?;
        }
        Ok(change)
    }

    pub(super) fn delete_folder_proton(
        &self,
        a: &Account,
        name: &str,
    ) -> Result<Vec<RemoteFolder>> {
        let folders = self.with_proton(a, |s| {
            let id = label_for(&s.folders(), &Mailbox::Remote(name.to_owned()))?;
            s.delete_label(&id)?;
            s.reload_folders()?;
            Ok(remote_folders(&s.folders()))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        // Proton moves what was in it to the inbox; the next check finds it there.
        let rows = self
            .store
            .messages(&a.id, Mailbox::Remote(name.to_owned()), usize::MAX >> 1)?;
        self.store.commit_changes(&rows, true)?;
        Ok(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, path: &str, exclusive: bool) -> proton::Folder {
        proton::Folder {
            id: id.into(),
            path: path.into(),
            depth: path.matches('/').count(),
            exclusive,
        }
    }

    #[test]
    fn location_prefers_where_the_message_is() {
        let folders = [
            folder("f1", "Work/Clients", true),
            folder("l1", "Later", false),
        ];
        let labels = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            location(&labels(&["0", "5", "10"]), &folders).0,
            Folder::Inbox
        );
        // Sent mail that was trashed: Trash.
        assert_eq!(
            location(&labels(&["2", "3", "5"]), &folders).0,
            Folder::Trash
        );
        assert_eq!(
            location(&labels(&["5", "f1", "l1"]), &folders),
            (Folder::Other, "Work/Clients".to_owned())
        );
        // Labels alone don't make a folder.
        assert_eq!(location(&labels(&["5", "l1"]), &folders).0, Folder::Archive);
        assert_eq!(location(&labels(&["1", "5"]), &folders).0, Folder::Drafts);
    }

    #[test]
    fn mailboxes_map_to_labels() {
        let folders = [folder("f1", "Work", true), folder("l1", "Work", false)];
        assert_eq!(label_for(&folders, &Folder::Inbox.into()).unwrap(), "0");
        assert_eq!(label_for(&folders, &Folder::Junk.into()).unwrap(), "4");
        assert_eq!(label_for(&folders, &Folder::All.into()).unwrap(), "5");
        assert_eq!(
            label_for(&folders, &Mailbox::Remote("Work".into())).unwrap(),
            "f1"
        );
        assert!(label_for(&folders, &Mailbox::Remote("Gone".into())).is_err());
    }

    #[test]
    fn rows_know_their_proton_ids() {
        let mut a = Account::new(Provider::Proton);
        a.id = "acct".into();
        let m = Mail {
            id: row_id(&a, "abc=="),
            ..Default::default()
        };
        assert_eq!(proton_id(&a, &m), Some("abc=="));
        let other = Mail {
            id: "acct:INBOX:1:2".into(),
            ..Default::default()
        };
        assert_eq!(proton_id(&a, &other), None);
    }

    #[test]
    fn drafts_remember_their_proton_ids() {
        let r = proton_remote("abc==");
        assert_eq!(proton_draft(&r), Some("abc=="));
        // An IMAP server's draft isn't taken for one.
        let imap = RemoteDraft {
            mailbox: "Drafts".into(),
            validity: 7,
            uid: 3,
        };
        assert_eq!(proton_draft(&imap), None);
    }

    #[test]
    fn folder_list_has_only_folders() {
        let folders = [
            folder("f1", "Work/Clients", true),
            folder("l1", "Later", false),
        ];
        let list = remote_folders(&folders);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Work/Clients");
        assert_eq!(list[0].label, "Clients");
        assert_eq!(list[0].depth, 1);
    }
}
