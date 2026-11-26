//! Gmail through the Gmail API (`sync/gmail.rs`): what the account
//! operations do for a Gmail account with `Account::api`, instead of IMAP
//! and SMTP.
//!
//! A Gmail message keeps one id whatever its labels, so it is one row here
//! (`{account}:gmail:{id}`), in the mailbox its labels put it in: Inbox,
//! Trash, Spam or Drafts where it has those; else under the first of your
//! own labels (which act as folders, as Gmail's web pages show them); else
//! Sent, where you sent it; else Archive. Its other labels of yours are the
//! row's labels, and Starred and Unread are its flags. Moving a message
//! relabels it, and its row changes rather than going.
//!
//! Drafts are kept here and in step with Gmail's (`sync_drafts_gmail`), as
//! for Proton and JMAP. What changed since the last look comes from the
//! mailbox's history (`history`), so a sync asks only about those.
use super::*;
use crate::sync::gmail::{self, Label, Metadata, Session, label};
use crate::sync::rest::not_found;

/// Messages listed at once.
const PAGE: usize = 100;
/// The most messages a server search returns.
const SEARCH_MOST: usize = 500;
/// The Gmail search for Archive: mail in none of the other mailboxes.
const ARCHIVE_QUERY: &str = "-in:inbox -in:trash -in:spam -in:drafts -in:sent -has:userlabels";

/// A Gmail draft, as a `RemoteDraft` (which IMAP fills with its mailbox,
/// UIDVALIDITY and UID): the draft's id, after `gmail:`.
fn gmail_remote(id: &str) -> RemoteDraft {
    RemoteDraft {
        mailbox: format!("gmail:{id}"),
        validity: 0,
        uid: 0,
    }
}
fn gmail_draft(r: &RemoteDraft) -> Option<&str> {
    r.mailbox.strip_prefix("gmail:")
}

/// The Gmail message id of one of our rows.
pub(super) fn gmail_id<'a>(a: &Account, m: &'a Mail) -> Option<&'a str> {
    m.id.strip_prefix(&a.id)?.strip_prefix(":gmail:")
}
pub(super) fn row_id(a: &Account, id: &str) -> String {
    format!("{}:gmail:{id}", a.id)
}
fn ids(a: &Account, mails: &[Mail]) -> Vec<String> {
    mails.iter().filter_map(|m| gmail_id(a, m).map(str::to_owned)).collect()
}

/// The person's own labels, which act as folders.
fn own(labels: &[Label]) -> impl Iterator<Item = &Label> {
    labels.iter().filter(|l| !l.system)
}

/// Where a message is, from its labels: (folder, its name).
pub(super) fn location(m: &Metadata, labels: &[Label]) -> (Folder, String) {
    for (id, folder, name) in [
        (label::INBOX, Folder::Inbox, "Inbox"),
        (label::TRASH, Folder::Trash, "Trash"),
        (label::SPAM, Folder::Junk, "Spam"),
        (label::DRAFT, Folder::Drafts, "Drafts"),
    ] {
        if m.has(id) {
            return (folder, name.to_owned());
        }
    }
    // The first of your labels it has, by name (`labels` is sorted).
    if let Some(l) = own(labels).find(|l| m.has(&l.id)) {
        return (Folder::Other, l.name.clone());
    }
    if m.has(label::SENT) {
        return (Folder::Sent, "Sent".into());
    }
    (Folder::Archive, "Archive".into())
}

/// What lists a mailbox: a label, and a Gmail search (only mail of the
/// last `days` days, where that isn't 0).
pub(super) fn listing(labels: &[Label], mailbox: &Mailbox, days: u32) -> Result<(Option<String>, String)> {
    let (label, base) = match mailbox {
        Mailbox::Standard(folder) => match folder {
            Folder::Inbox => (Some(label::INBOX.to_owned()), ""),
            Folder::Sent => (Some(label::SENT.to_owned()), ""),
            Folder::Drafts => (Some(label::DRAFT.to_owned()), ""),
            Folder::Junk => (Some(label::SPAM.to_owned()), ""),
            Folder::Trash => (Some(label::TRASH.to_owned()), ""),
            Folder::Starred => (Some(label::STARRED.to_owned()), ""),
            Folder::Archive => (None, ARCHIVE_QUERY),
            Folder::All => (None, ""),
            Folder::Other => bail!(t!("manage-proton-choose-one-of-the-accounts-folders")),
        },
        Mailbox::Remote(name) => {
            let id = own(labels)
                .find(|l| &l.name == name)
                .map(|l| l.id.clone())
                .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = name))?;
            (Some(id), "")
        }
    };
    let query = [base.to_owned(), within(days)]
        .into_iter()
        .filter(|q| !q.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Ok((label, query))
}
/// The Gmail search for mail of the last `days` days (0: all).
fn within(days: u32) -> String {
    if days > 0 { format!("newer_than:{days}d") } else { String::new() }
}

/// The person's labels, as the folder list shows them.
pub(super) fn remote_folders(labels: &[Label]) -> Vec<RemoteFolder> {
    own(labels)
        .map(|l| RemoteFolder {
            name: l.name.clone(),
            label: l.name.rsplit('/').next().unwrap_or(&l.name).to_owned(),
            depth: l.name.matches('/').count(),
            nestable: true,
        })
        .collect()
}

/// The label a mailbox is (to add or take off), where it is one.
fn label_of(labels: &[Label], folder: Folder, remote: &str) -> Option<String> {
    Some(
        match folder {
            Folder::Inbox => label::INBOX,
            Folder::Trash => label::TRASH,
            Folder::Junk => label::SPAM,
            Folder::Other => return own(labels).find(|l| l.name == remote).map(|l| l.id.clone()),
            _ => return None,
        }
        .to_owned(),
    )
}

impl Backend {
    /// The account's Gmail API session: opened on first use, then kept
    /// until its access token is about to expire, when the token is renewed
    /// (`credentials`) and the session opened again.
    fn gmail_session(&self, a: &Account) -> Result<Arc<Session>> {
        let now = chrono::Utc::now().timestamp();
        if let Some((s, expires)) = self.gmail.lock().unwrap().get(&a.id)
            && *expires > now + 120
        {
            return Ok(s.clone());
        }
        let secret = self.credentials(a)?;
        let session = Arc::new(self.open_gmail(a, &secret)?);
        self.gmail
            .lock()
            .unwrap()
            .insert(a.id.clone(), (session.clone(), secret.expires_at));
        Ok(session)
    }
    fn open_gmail(&self, a: &Account, secret: &Secret) -> Result<Session> {
        Session::open(&api_base(a, gmail::BASE), &secret.access_token, self.tls_connector()?)
    }
    /// Runs `work` with the account's session; a refused sign-in drops it,
    /// so the next try signs in again.
    pub(super) fn with_gmail<T>(&self, a: &Account, work: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        let session = self.gmail_session(a)?;
        let result = work(&session);
        if let Err(e) = &result {
            if e.downcast_ref::<AccessFailure>().is_some() {
                self.gmail.lock().unwrap().remove(&a.id);
            }
            // 429 or 503: background work pauses, as for IMAP.
            self.note_limit(a, e);
        }
        result
    }
    /// Signs in to check the account (adding or reconnecting it): the
    /// mailbox must be the address signed in with.
    pub(super) fn check_gmail(&self, a: &Account, secret: &Secret) -> Result<()> {
        let s = self.open_gmail(a, secret)?;
        let profile = s.profile()?;
        ensure!(
            profile.email.is_empty() || profile.email.eq_ignore_ascii_case(&a.email),
            t!("manage-backend-choose-the-same-email-address-to")
        );
        s.reload_labels()?;
        Ok(())
    }
    fn gmail_labels(&self, a: &Account, s: &Session, reload: bool) -> Result<Vec<Label>> {
        let labels = if reload { s.reload_labels()? } else { s.labels()? };
        if reload {
            let _ = self.store.save_folders(&a.id, &remote_folders(&labels));
        }
        Ok(labels)
    }

    /// A row for a listed message: the cached one with its flags, folder
    /// and labels brought up to date, or a new one from its headers.
    fn gmail_mail(&self, a: &Account, labels: &[Label], m: &Metadata, pre: Option<&Prefetched>) -> Result<Mail> {
        let id = row_id(a, &m.id);
        let (folder, remote) = location(m, labels);
        let mut mail = match Prefetched::cached(pre, &self.store, &id)? {
            Some(cached) => cached,
            None => {
                let mut fresh = parse_message(&m.header_block(), a, &id, folder, &remote, 0)?;
                fresh.snippet = m.snippet.clone();
                // Only multipart/mixed may carry files; whether it does is
                // known once it is downloaded.
                fresh.has_attachments = m.mime_type.eq_ignore_ascii_case("multipart/mixed");
                fresh
            }
        };
        if mail.date <= 0 {
            mail.date = m.received;
        }
        if !m.thread_id.is_empty() {
            mail.set_thread_id(format!("gmail:{}", m.thread_id));
        }
        mail.folder = folder;
        mail.remote_folder = remote.clone();
        mail.unread = m.has(label::UNREAD);
        mail.starred = m.has(label::STARRED);
        mail.size = u32::try_from(m.size).unwrap_or(u32::MAX);
        let mut own_labels: Vec<String> = own(labels)
            .filter(|l| m.has(&l.id) && l.name != remote)
            .map(|l| l.name.clone())
            .collect();
        own_labels.sort();
        mail.set_labels(own_labels);
        Ok(mail)
    }

    fn gmail_history_key(a: &Account) -> String {
        format!("gmail_history:{}", a.id)
    }

    /// Brings cached rows up to date with what changed in the mailbox since
    /// the last look (Gmail's history): relabeled ones read again, deleted
    /// ones removed. Where Gmail can no longer say, the rows of `mailbox`
    /// are read again. Returns the rows read.
    fn gmail_changes(&self, a: &Account, s: &Session, labels: &[Label], mailbox: &Mailbox) -> Result<Vec<Mail>> {
        let key = Self::gmail_history_key(a);
        let saved = self.store.preference(&key).filter(|h| !h.is_empty());
        let (changed, deleted, next) = match saved.as_deref().map(|h| s.history(h)).transpose()? {
            Some(Some(h)) => (h.changed, h.deleted, h.history_id),
            // Never looked, or too long ago: what is cached here is read again.
            _ => {
                let next = s.profile()?.history_id;
                let cached: Vec<Mail> = self
                    .store
                    .summaries(&a.id, mailbox.clone())?
                    .into_iter()
                    .filter(|m| gmail_id(a, m).is_some())
                    .collect();
                (ids(a, &cached), Vec::new(), next)
            }
        };
        let mut rows = Vec::new();
        // Only rows cached here matter: others are listed when their
        // mailbox is.
        let cached: Vec<String> = changed
            .into_iter()
            .filter(|id| self.store.has_message(&row_id(a, id)).unwrap_or(false))
            .collect();
        let current = s.metadata(&cached)?;
        let still: HashSet<&str> = current.iter().map(|m| m.id.as_str()).collect();
        let mut gone: Vec<String> = deleted.iter().map(|id| row_id(a, id)).collect();
        gone.extend(cached.iter().filter(|id| !still.contains(id.as_str())).map(|id| row_id(a, id)));
        let pre = Prefetched::new(&self.store, current.iter().map(|m| row_id(a, &m.id)).collect())?;
        for m in &current {
            rows.push(self.gmail_mail(a, labels, m, Some(&pre))?);
        }
        self.forget_gmail_rows(a, &gone)?;
        self.store.save_messages(&rows)?;
        if !next.is_empty() {
            self.store.set_preference(&key, &next)?;
        }
        Ok(rows)
    }
    /// Removes rows of messages gone from Gmail.
    fn forget_gmail_rows(&self, a: &Account, gone: &[String]) -> Result<()> {
        if gone.is_empty() {
            return Ok(());
        }
        self.store.delete_ids(&a.id, gone)?;
        let gone: HashSet<&String> = gone.iter().collect();
        self.readable.lock().unwrap().retain(|id, _| !gone.contains(id));
        Ok(())
    }

    /// Brings a mailbox up to date: what changed since the last look, then
    /// its newest `limit` messages listed (those not cached read), and
    /// cached ones that weren't listed asked about again (moved elsewhere,
    /// or deleted).
    pub(super) fn sync_gmail(&self, a: &Account, mailbox: &Mailbox, limit: usize) -> Result<Vec<Mail>> {
        // Drafts are kept on this computer, and in step with Gmail's.
        if mailbox.folder() == Folder::Drafts {
            return Ok(vec![]);
        }
        // Changes made while offline reach Gmail before anything is read.
        self.replay_offline_gmail(a)?;
        self.with_gmail(a, |s| {
            let labels = self.gmail_labels(a, s, true)?;
            self.gmail_changes(a, s, &labels, mailbox)?;
            let (label, query) = listing(&labels, mailbox, a.sync.days)?;
            let mut listed: Vec<String> = Vec::new();
            let mut page = None;
            let mut complete = false;
            while listed.len() < limit {
                let (ids, next) = s.list(label.as_deref(), &query, page.as_deref(), (limit - listed.len()).min(PAGE))?;
                listed.extend(ids);
                match next {
                    Some(next) => page = Some(next),
                    None => {
                        complete = true;
                        break;
                    }
                }
            }
            listed.truncate(limit);
            let new: Vec<String> = listed
                .iter()
                .filter(|id| !self.store.has_message(&row_id(a, id)).unwrap_or(false))
                .cloned()
                .collect();
            let mut rows = Vec::new();
            for m in &s.metadata(&new)? {
                rows.push(self.gmail_mail(a, &labels, m, None)?);
            }
            self.store.save_messages(&rows)?;
            // What the cache has here that Gmail didn't list: within the
            // listed range (or anywhere, when the whole mailbox was
            // listed), it has moved or gone.
            let listed_rows: HashSet<String> = listed.iter().map(|id| row_id(a, id)).collect();
            let oldest = listed
                .iter()
                .map(|id| row_id(a, id))
                .collect::<Vec<_>>();
            // Their dates alone, in one query, not each row whole.
            let oldest = self.store.oldest_date(&oldest)?.unwrap_or(i64::MIN);
            let unlisted: Vec<Mail> = self
                .store
                .summaries(&a.id, mailbox.clone())?
                .into_iter()
                .filter(|m| gmail_id(a, m).is_some() && !listed_rows.contains(&m.id))
                .filter(|m| complete || m.date >= oldest)
                .collect();
            if !unlisted.is_empty() {
                let asked = ids(a, &unlisted);
                let current = s.metadata(&asked)?;
                let still: HashSet<String> = current.iter().map(|m| row_id(a, &m.id)).collect();
                let mut moved = Vec::new();
                for m in &current {
                    moved.push(self.gmail_mail(a, &labels, m, None)?);
                }
                self.store.save_messages(&moved)?;
                let gone: Vec<String> =
                    unlisted.iter().map(|m| m.id.clone()).filter(|id| !still.contains(id)).collect();
                self.forget_gmail_rows(a, &gone)?;
            }
            self.store.newest(&a.id, mailbox.clone(), limit)
        })
    }

    /// Lists `batch` more of a mailbox's messages beyond those cached,
    /// newest first.
    pub(super) fn catalog_gmail(&self, a: &Account, mailbox: &Mailbox, batch: usize) -> Result<Catalog> {
        self.with_gmail(a, |s| {
            let labels = self.gmail_labels(a, s, false)?;
            let (label, query) = listing(&labels, mailbox, a.sync.days)?;
            let cached = self.store.count_messages(&a.id, mailbox.clone())?;
            let total = match (&label, query.is_empty()) {
                (Some(id), true) => Some(s.label_total(id)? as usize),
                _ => None,
            };
            if total.is_some_and(|t| cached >= t) {
                return Ok(Catalog { cataloged: cached, total: cached, ..Default::default() });
            }
            let mut added = Vec::new();
            let mut page: Option<String> = None;
            let mut ended = false;
            // Pages already cached are passed over, a few at a time, until
            // new ones turn up.
            for _ in 0..8 {
                if !added.is_empty() && self.someone_waiting(&a.id) {
                    break;
                }
                let (listed, next) = s.list(label.as_deref(), &query, page.as_deref(), 500)?;
                let new: Vec<String> = listed
                    .into_iter()
                    .filter(|id| !self.store.has_message(&row_id(a, id)).unwrap_or(false))
                    .take(batch.max(1) - added.len().min(batch.max(1) - 1))
                    .collect();
                for m in &s.metadata(&new)? {
                    added.push(self.gmail_mail(a, &labels, m, None)?);
                }
                match next {
                    Some(n) if added.len() < batch => page = Some(n),
                    Some(_) => break,
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            self.store.save_messages(&added)?;
            let cataloged = cached + added.len();
            let done = ended || added.is_empty();
            let total = total.unwrap_or(cataloged).max(cataloged);
            Ok(Catalog {
                cataloged: if done { total } else { cataloged },
                total: if done { total.max(cataloged) } else { total },
                added,
                layout: None,
            })
        })
    }

    /// Downloads one message's source and caches it.
    pub(super) fn download_gmail(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let raw = self.fetch_gmail_raw(a, m)?;
        self.store.save_raw(&m.id, &raw)?;
        Ok(raw)
    }

    /// One message's source, as Gmail has it, not yet kept.
    fn fetch_gmail_raw(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let id = gmail_id(a, m).with_context(|| t!("manage-gmail-this-message-isnt-from-this-gmail"))?;
        let raw = self.with_gmail(a, |s| s.raw(id))?;
        ensure!(raw.len() <= large::MAX_DOWNLOAD, t!("manage-proton-message-exceeds-the-download-limit"));
        self.note_traffic(a, raw.len());
        Ok(raw)
    }

    /// Background downloads: up to `max` messages only listed so far.
    pub(super) fn download_batch_gmail(&self, a: &Account, max: usize, since: i64) -> Result<usize> {
        let mut stored = 0;
        for m in self.store.missing(&a.id, MAX_MESSAGE, max.min(20), since)? {
            if stored > 0 && self.someone_waiting(&a.id) {
                break;
            }
            // Kept once (`keep_source`), not compressed and written twice.
            let raw = match self.fetch_gmail_raw(a, &m) {
                Ok(raw) => raw,
                Err(e) if not_found(&e) => {
                    self.forget_gmail_rows(a, std::slice::from_ref(&m.id))?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            self.keep_source(a, &m, &raw)?;
            stored += 1;
        }
        Ok(stored)
    }

    /// Makes a change at Gmail: one relabeling for all the messages.
    pub(super) fn change_gmail(&self, a: &Account, current: &[Mail], change: &Change) -> Result<()> {
        if ids(a, current).is_empty() {
            return Ok(());
        }
        self.with_gmail(a, |s| {
            let labels = s.labels()?;
            let all = ids(a, current);
            match change {
                Change::Read(read) => {
                    if *read {
                        s.modify(&all, &[], &[label::UNREAD])
                    } else {
                        s.modify(&all, &[label::UNREAD], &[])
                    }
                }
                Change::Star(star) => {
                    if *star {
                        s.modify(&all, &[label::STARRED], &[])
                    } else {
                        s.modify(&all, &[], &[label::STARRED])
                    }
                }
                Change::Move(_) | Change::MoveTo(_) => {
                    let (target, target_name) = match change {
                        Change::MoveTo(name) => (Folder::Other, name.clone()),
                        Change::Move(f) => (*f, String::new()),
                        _ => unreachable!("a move"),
                    };
                    let add = match target {
                        Folder::Archive => None,
                        f => Some(label_of(&labels, f, &target_name).with_context(|| {
                            if f == Folder::Other {
                                t!("manage-proton-the-folder-no-longer-exists-at", name = target_name)
                            } else {
                                t!("manage-gmail-gmail-doesnt-move-mail-to", label = f.label())
                            }
                        })?),
                    };
                    // Each message leaves where it is (its folder's label),
                    // grouped by that.
                    let mut by_place: HashMap<Option<String>, Vec<String>> = HashMap::new();
                    for m in current {
                        let Some(id) = gmail_id(a, m) else { continue };
                        by_place
                            .entry(label_of(&labels, m.folder, &m.remote_folder))
                            .or_default()
                            .push(id.to_owned());
                    }
                    for (from, ids) in by_place {
                        let mut remove: Vec<&str> = Vec::new();
                        if let Some(from) = from.as_deref().filter(|f| Some(*f) != add.as_deref()) {
                            remove.push(from);
                        }
                        // Out of Inbox, Spam and Trash wherever it goes,
                        // except into them.
                        for system in [label::INBOX, label::SPAM, label::TRASH] {
                            if Some(system) != add.as_deref() && !remove.contains(&system) {
                                remove.push(system);
                            }
                        }
                        let add: Vec<&str> = add.iter().map(String::as_str).collect();
                        s.modify(&ids, &add, &remove)?;
                    }
                    Ok(())
                }
                Change::Label(name, on) => {
                    let id = match own(&labels).find(|l| l.name.eq_ignore_ascii_case(name)) {
                        Some(l) => l.id.clone(),
                        None if *on => s.create_label(name)?.id,
                        None => return Ok(()),
                    };
                    if *on { s.modify(&all, &[&id], &[]) } else { s.modify(&all, &[], &[&id]) }
                }
                // Gmail has no flags for replies, forwards or receipts:
                // these stay on this computer.
                Change::Mark(_) => Ok(()),
            }
        })
    }
    /// Sends queued changes to Gmail, oldest first. Callers hold the lock.
    pub(super) fn replay_offline_gmail(&self, a: &Account) -> Result<()> {
        self.replay_offline_with(a, |mails, change| self.change_gmail(a, mails, change))
    }

    /// Deletes messages for good at Gmail.
    pub(super) fn delete_gmail(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let ids = ids(a, mails);
        if !ids.is_empty() {
            self.with_gmail(a, |s| s.delete(&ids))?;
        }
        self.store.commit_changes(mails, true)
    }

    /// Deletes everything in Trash or Spam for good.
    pub(super) fn empty_gmail(&self, a: &Account, folder: Folder) -> Result<usize> {
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_gmail(a, |s| {
            let (label, query) = listing(&[], &mailbox, 0)?;
            let mut count = 0;
            // Bounded: a mailbox refilling as it empties can't keep this going.
            for _ in 0..100 {
                let (page, _) = s.list(label.as_deref(), &query, None, 500)?;
                if page.is_empty() {
                    break;
                }
                s.delete(&page)?;
                count += page.len();
            }
            Ok(count)
        })?;
        let cached = self.store.summaries(&a.id, mailbox)?;
        self.store.commit_changes(&cached, true)?;
        Ok(count)
    }

    /// Server search: Gmail's own, with Gmail's search words (`from:`,
    /// `has:attachment`, `older_than:`…), within the mailbox. Matches not
    /// yet listed here are kept like any other listed mail.
    pub(super) fn search_gmail(&self, a: &Account, mailbox: &Mailbox, query: &str, limit: usize) -> Result<ServerResults> {
        self.with_gmail(a, |s| {
            let labels = s.labels()?;
            let (label, scope) = listing(&labels, mailbox, 0)?;
            let q = if scope.is_empty() { query.trim().to_owned() } else { format!("{} {scope}", query.trim()) };
            let found = s.list_all(label.as_deref(), &q, SEARCH_MOST)?;
            let mut out = ServerResults { total: found.len(), ..Default::default() };
            let page = s.metadata(&found[..found.len().min(limit)])?;
            let pre = Prefetched::new(&self.store, page.iter().map(|m| row_id(a, &m.id)).collect())?;
            for m in &page {
                out.mails.push(self.gmail_mail(a, &labels, m, Some(&pre))?);
            }
            self.store.save_messages(&out.mails)?;
            out.mails.sort_by(Mail::newest_first);
            Ok(out)
        })
    }

    /// Marks everything unread in a mailbox read at Gmail, listed or not.
    pub(super) fn mark_all_read_gmail(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        self.with_gmail(a, |s| {
            let labels = s.labels()?;
            let (label, query) = listing(&labels, mailbox, 0)?;
            let q = if query.is_empty() { "is:unread".to_owned() } else { format!("{query} is:unread") };
            let unread = s.list_all(label.as_deref(), &q, 20_000)?;
            if !unread.is_empty() {
                s.modify(&unread, &[], &[label::UNREAD])?;
            }
            Ok(())
        })?;
        self.store.mark_mailbox_read(&a.id, mailbox.clone())
    }

    /// Sends a finished message through Gmail, which keeps it in Sent.
    /// Bcc recipients are named in it for Gmail, which takes them out.
    pub(super) fn send_gmail(&self, a: &Account, d: &Draft, raw: &[u8]) -> Result<()> {
        let raw = crate::sync::smtp::with_bcc(raw.to_vec(), d)?;
        self.with_gmail(a, |s| s.send(&raw).map(drop))
    }
    /// Saves a copy of a message sent through another server in Sent at
    /// Gmail (an identity with its own outgoing server).
    pub(super) fn file_sent_copy_gmail(&self, a: &Account, raw: &[u8]) -> Result<()> {
        self.with_gmail(a, |s| s.insert(raw, &[label::SENT]).map(drop))
    }

    /// Keeps the account's drafts and Gmail's in step, as for Proton:
    /// drafts written here are uploaded (replacing their earlier copy),
    /// drafts written elsewhere appear here, and drafts sent or deleted on
    /// either side go from the other. A draft changed here since its last
    /// upload is never thrown away. Returns how many drafts changed here.
    pub(super) fn sync_drafts_gmail(&self, a: &Account) -> Result<usize> {
        let _ = self.file_queued_sent_copies_with(a, |raw| self.file_sent_copy_gmail(a, raw));
        self.with_gmail(a, |s| {
            let pending = self.draft_removals(&a.id);
            for id in pending.iter().filter_map(gmail_draft) {
                match s.delete_draft(id) {
                    Ok(()) => {}
                    Err(e) if not_found(&e) => {}
                    Err(e) if crate::sync::retry::later(&e) => return Err(e),
                    Err(_) => {}
                }
            }
            self.clear_draft_removals(&a.id, &pending)?;
            const MOST: usize = 20_000;
            let mut on_server: Vec<String> = s.drafts(MOST + 1)?.into_iter().map(|(id, _)| id).collect();
            let complete = on_server.len() <= MOST;
            on_server.truncate(MOST);
            let listed: HashSet<&str> = on_server.iter().map(String::as_str).collect();
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            for d in &mut locals {
                let Some(id) = d.remote.as_ref().and_then(gmail_draft) else { continue };
                if !complete || listed.contains(id) {
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
            let claimed: HashSet<String> = locals
                .iter()
                .filter_map(|d| d.remote.as_ref().and_then(gmail_draft).map(str::to_owned))
                .collect();
            for id in &on_server {
                if claimed.contains(id) {
                    continue;
                }
                let raw = s.draft_raw(id)?;
                ensure!(raw.len() <= MAX_MESSAGE, t!("manage-gmail-a-draft-at-gmail-is-too-large"));
                let mut d = self.draft_from(a, &uuid::Uuid::new_v4().to_string(), &raw)?;
                d.remote = Some(gmail_remote(id));
                d.uploaded = d.fingerprint();
                self.store.save_draft(&d)?;
                locals.push(d);
                changed += 1;
            }
            for d in &locals {
                let fingerprint = d.fingerprint();
                if d.remote.is_some() && d.uploaded == fingerprint {
                    continue;
                }
                // Encrypted without a key of yours to seal it: it stays here.
                let Some(raw) = self.draft_upload(a, d)? else {
                    if let Some(old) = d.remote.as_ref().and_then(gmail_draft) {
                        if let Err(e) = s.delete_draft(old)
                            && crate::sync::retry::later(&e)
                        {
                            return Err(e);
                        }
                        self.keep_draft_here(d)?;
                    }
                    continue;
                };
                let id = s.create_draft(&raw)?;
                if let Some(old) = d.remote.as_ref().and_then(gmail_draft) {
                    let _ = s.delete_draft(old);
                }
                if !self.finish_draft_upload(d, gmail_remote(&id), fingerprint)? && s.delete_draft(&id).is_err() {
                    // Discarded while uploading: its new copy goes too.
                    self.queue_draft_removal(&a.id, gmail_remote(&id))?;
                }
            }
            Ok(changed)
        })
    }

    // Folders: your labels.

    pub(super) fn create_folder_gmail(&self, a: &Account, parent: Option<&str>, leaf: &str) -> Result<FolderChange> {
        self.with_gmail(a, |s| {
            let name = match parent {
                Some(p) => format!("{p}/{leaf}"),
                None => leaf.to_owned(),
            };
            s.create_label(&name)?;
            let folders = remote_folders(&s.labels()?);
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange { moved: Vec::new(), folders })
        })
    }

    pub(super) fn rename_folder_gmail(&self, a: &Account, name: &str, leaf: &str) -> Result<FolderChange> {
        let change = self.with_gmail(a, |s| {
            let before = s.labels()?;
            let id = own(&before)
                .find(|l| l.name == name)
                .map(|l| l.id.clone())
                .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = name))?;
            let renamed = match name.rsplit_once('/') {
                Some((parent, _)) => format!("{parent}/{leaf}"),
                None => leaf.to_owned(),
            };
            s.rename_label(&id, &renamed)?;
            // Labels inside it are named by their path: renamed with it.
            let prefix = format!("{name}/");
            for inner in own(&before).filter(|l| l.name.starts_with(&prefix)) {
                s.rename_label(&inner.id, &format!("{renamed}/{}", &inner.name[prefix.len()..]))?;
            }
            let after = s.labels()?;
            let moved: Vec<(String, String)> = own(&before)
                .filter_map(|old| {
                    let new = after.iter().find(|n| n.id == old.id)?;
                    (new.name != old.name).then(|| (old.name.clone(), new.name.clone()))
                })
                .collect();
            let folders = remote_folders(&after);
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange { moved, folders })
        })?;
        for (old, new) in &change.moved {
            self.store.rename_remote(&a.id, old, new)?;
        }
        Ok(change)
    }

    pub(super) fn delete_folder_gmail(&self, a: &Account, name: &str) -> Result<Vec<RemoteFolder>> {
        let folders = self.with_gmail(a, |s| {
            let id = own(&s.labels()?)
                .find(|l| l.name == name)
                .map(|l| l.id.clone())
                .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = name))?;
            s.delete_label(&id)?;
            Ok(remote_folders(&s.labels()?))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        // Its messages lose the label, nothing more: the next check finds
        // them where they now are.
        let rows = self.store.summaries(&a.id, Mailbox::Remote(name.to_owned()))?;
        self.store.commit_changes(&rows, true)?;
        Ok(folders)
    }
}

impl Backend {
    /// Reads the account's folders (Gmail's labels) again, for the folder
    /// list.
    pub(super) fn refresh_api_folders(&self, a: &Account) -> Result<()> {
        if a.gmail_api() {
            self.with_gmail(a, |s| self.gmail_labels(a, s, true).map(drop))
        } else if a.ews() {
            self.with_ews(a, |s| self.ews_folders(a, s, true).map(drop))
        } else {
            self.with_graph(a, |s| self.graph_folders(a, s, true).map(drop))
        }
    }
    /// The rows of messages by their ids at the API, as the server has
    /// them now (and saved so); those gone are left out.
    pub(super) fn api_rows(&self, a: &Account, ids: &[String]) -> Result<Vec<Mail>> {
        let rows: Vec<Mail> = if a.ews() {
            self.with_ews(a, |s| {
                let folders = s.folders()?;
                s.messages(ids)?.iter().map(|m| self.ews_mail(a, &folders, m, None)).collect()
            })?
        } else if a.gmail_api() {
            self.with_gmail(a, |s| {
                let labels = s.labels()?;
                s.metadata(ids)?.iter().map(|m| self.gmail_mail(a, &labels, m, None)).collect()
            })?
        } else {
            self.with_graph(a, |s| {
                let folders = s.folders()?;
                s.messages(ids)?.iter().map(|m| self.graph_mail(a, &folders, m, None)).collect()
            })?
        };
        self.store.save_messages(&rows)?;
        Ok(rows)
    }
}

/// Where an API's requests go: its own address; in tests, the account's
/// server (a loopback fake).
pub(super) fn api_base(a: &Account, official: &str) -> String {
    #[cfg(test)]
    if a.incoming_host.starts_with("127.0.0.1") || a.incoming_host == "localhost" {
        return format!("http://{}:{}", a.incoming_host, a.incoming_port);
    }
    let _ = a;
    official.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(id: &str, name: &str, system: bool) -> Label {
        Label { id: id.into(), name: name.into(), system }
    }
    fn message(labels: &[&str]) -> Metadata {
        Metadata { id: "m1".into(), label_ids: labels.iter().map(|l| l.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn labels_say_where_a_message_is() {
        let labels = vec![label("INBOX", "INBOX", true), label("L1", "Receipts", false), label("L2", "Work", false)];
        assert_eq!(location(&message(&["INBOX", "L2"]), &labels), (Folder::Inbox, "Inbox".into()));
        assert_eq!(location(&message(&["L2", "L1", "SENT"]), &labels), (Folder::Other, "Receipts".into()));
        assert_eq!(location(&message(&["SENT", "UNREAD"]), &labels), (Folder::Sent, "Sent".into()));
        assert_eq!(location(&message(&["TRASH", "L1"]), &labels), (Folder::Trash, "Trash".into()));
        assert_eq!(location(&message(&["IMPORTANT"]), &labels), (Folder::Archive, "Archive".into()));
        let folders = remote_folders(&[label("L3", "Work/2026", false), label("INBOX", "INBOX", true)]);
        assert_eq!(folders.len(), 1);
        assert_eq!((folders[0].label.as_str(), folders[0].depth), ("2026", 1));
    }

    #[test]
    fn mailboxes_are_labels_or_searches() -> Result<()> {
        let labels = vec![label("L1", "Receipts", false)];
        assert_eq!(listing(&labels, &Mailbox::Standard(Folder::Inbox), 0)?, (Some("INBOX".into()), String::new()));
        assert_eq!(
            listing(&labels, &Mailbox::Standard(Folder::Inbox), 30)?,
            (Some("INBOX".into()), "newer_than:30d".into())
        );
        assert_eq!(listing(&labels, &Mailbox::Standard(Folder::Archive), 0)?, (None, ARCHIVE_QUERY.into()));
        assert_eq!(listing(&labels, &Mailbox::Remote("Receipts".into()), 0)?, (Some("L1".into()), String::new()));
        assert!(listing(&labels, &Mailbox::Remote("Gone".into()), 0).is_err());
        Ok(())
    }

    #[test]
    fn rows_and_drafts_know_their_ids() {
        let a = Account::new(Provider::Gmail);
        let m = Mail { id: row_id(&a, "18c2"), ..Default::default() };
        assert_eq!(gmail_id(&a, &m), Some("18c2"));
        assert_eq!(gmail_draft(&gmail_remote("r-1")), Some("r-1"));
    }
}
