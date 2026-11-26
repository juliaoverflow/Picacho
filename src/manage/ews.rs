//! Exchange Server through Exchange Web Services (`sync/ews.rs`): what the
//! account operations do for an Exchange account (`Provider::Ews`).
//!
//! A message is a row (`{account}:ews:{id}`) in the folder Exchange says.
//! An item's id changes when it moves, so a move gives its row the new id
//! (`MoveItem` returns it), and the old id is remembered for a while so
//! undoing a move finds the message (`ews_moved`). Categories are labels;
//! Flagged is Starred; replied and forwarded are Outlook's own marks
//! (PR_LAST_VERB_EXECUTED). A sync lists the newest messages, which carry
//! all of that, and asks only about cached ones that weren't listed.
//!
//! Calendars and contacts come from the same mailbox: events as iCalendar,
//! contacts as vCard (Exchange's MIME content for them); Exchange sends
//! invitations itself. The organization's directory answers people
//! searches (ResolveNames).
use super::*;
use crate::sync::caldav::{self, Stored};
use crate::sync::carddav::{self, Card};
use crate::sync::ews::{self, Auth, MailFolder, Message, Session};
use crate::sync::rest::not_found;

/// Messages listed at once.
const PAGE: usize = 100;
/// The most messages a server search returns.
const SEARCH_MOST: usize = 500;
/// Old ids of moved messages remembered (for undo).
const MOVED_KEPT: usize = 500;

fn ews_remote(id: &str) -> RemoteDraft {
    RemoteDraft { mailbox: format!("ews:{id}"), validity: 0, uid: 0 }
}
fn ews_draft(r: &RemoteDraft) -> Option<&str> {
    r.mailbox.strip_prefix("ews:")
}
pub(super) fn ews_id<'a>(a: &Account, m: &'a Mail) -> Option<&'a str> {
    m.id.strip_prefix(&a.id)?.strip_prefix(":ews:")
}
pub(super) fn row_id(a: &Account, id: &str) -> String {
    format!("{}:ews:{id}", a.id)
}
fn ids(a: &Account, mails: &[Mail]) -> Vec<String> {
    mails.iter().filter_map(|m| ews_id(a, m).map(str::to_owned)).collect()
}

fn standard(distinguished: &str) -> Option<Folder> {
    Some(match distinguished {
        "inbox" => Folder::Inbox,
        "sentitems" => Folder::Sent,
        "drafts" => Folder::Drafts,
        "deleteditems" => Folder::Trash,
        "junkemail" => Folder::Junk,
        "archive" => Folder::Archive,
        _ => return None,
    })
}
/// A standard mailbox's folder: its distinguished name, or the Archive
/// folder's id.
fn folder_for(folders: &[MailFolder], folder: Folder) -> Option<String> {
    Some(match folder {
        Folder::Inbox => "inbox".into(),
        Folder::Sent => "sentitems".into(),
        Folder::Drafts => "drafts".into(),
        Folder::Trash => "deleteditems".into(),
        Folder::Junk => "junkemail".into(),
        Folder::Archive => folders.iter().find(|f| f.distinguished.as_deref() == Some("archive"))?.id.clone(),
        _ => return None,
    })
}
/// Mail folders of the account's own: not standard, not calendars or
/// contacts, not Outbox.
fn own(folders: &[MailFolder]) -> impl Iterator<Item = &MailFolder> {
    folders
        .iter()
        .filter(|f| f.distinguished.is_none() && (f.class.is_empty() || f.class.starts_with("IPF.Note")))
}
pub(super) fn location(m: &Message, folders: &[MailFolder]) -> (Folder, String) {
    match folders.iter().find(|f| f.id == m.folder_id) {
        Some(f) => match f.distinguished.as_deref().and_then(standard) {
            Some(folder) => (folder, f.name.clone()),
            None => (Folder::Other, f.path.clone()),
        },
        None => (Folder::Archive, "Archive".into()),
    }
}
fn folder_id(folders: &[MailFolder], path: &str) -> Result<String> {
    own(folders)
        .find(|f| f.path == path)
        .map(|f| f.id.clone())
        .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = path))
}
/// The folder a mailbox is.
fn listing(folders: &[MailFolder], mailbox: &Mailbox) -> Result<String> {
    match mailbox {
        Mailbox::Standard(folder) => folder_for(folders, *folder).with_context(|| t!("manage-proton-choose-one-of-the-accounts-folders")),
        Mailbox::Remote(name) => folder_id(folders, name),
    }
}
pub(super) fn remote_folders(folders: &[MailFolder]) -> Vec<RemoteFolder> {
    own(folders)
        .map(|f| RemoteFolder { name: f.path.clone(), label: f.name.clone(), depth: f.depth, nestable: true })
        .collect()
}
/// The account's sign-in for EWS: the organization's (an access token),
/// Kerberos, Basic where the account chose it (`Account::ews_basic`), or
/// NTLM with its name and password.
fn auth(a: &Account, secret: &Secret) -> Auth {
    if a.sso.is_some() {
        Auth::Bearer(secret.access_token.clone())
    } else if a.kerberos {
        Auth::Kerberos
    } else if a.ews_basic {
        Auth::Basic { user: a.login().to_owned(), password: secret.password.clone() }
    } else {
        Auth::Ntlm(ews::ntlm::Identity::new(a.login(), &secret.password))
    }
}
/// The sign-in Autodiscover uses: the account's, except that Basic is
/// never sent to the servers it asks, so NTLM is tried there instead.
fn autodiscover_auth(a: &Account, secret: &Secret) -> Auth {
    match auth(a, secret) {
        Auth::Basic { .. } => Auth::Ntlm(ews::ntlm::Identity::new(a.login(), &secret.password)),
        other => other,
    }
}

impl Backend {
    fn ews_session(&self, a: &Account) -> Result<Arc<Session>> {
        let now = chrono::Utc::now().timestamp();
        if let Some((s, expires)) = self.ews.lock().unwrap().get(&a.id)
            && *expires > now + 120
        {
            return Ok(s.clone());
        }
        let secret = self.credentials(a)?;
        let session = Arc::new(Session::open(&a.ews_endpoint(), &a.email, auth(a, &secret), self.tls_connector()?)?);
        // A token's session lasts as long as the token.
        let expires = if a.sso.is_some() && secret.expires_at > 0 { secret.expires_at } else { i64::MAX };
        self.ews.lock().unwrap().insert(a.id.clone(), (session.clone(), expires));
        Ok(session)
    }
    /// Runs `work` with the account's session; a refused sign-in drops it.
    pub(super) fn with_ews<T>(&self, a: &Account, work: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        let session = self.ews_session(a)?;
        let result = work(&session);
        if let Err(e) = &result {
            if e.downcast_ref::<AccessFailure>().is_some() {
                self.ews.lock().unwrap().remove(&a.id);
            }
            self.note_limit(a, e);
        }
        result
    }
    /// Signs in to check the account (adding or reconnecting it). Where no
    /// EWS address was given, Autodiscover finds it (kept in the account).
    pub(super) fn check_ews(&self, a: &mut Account, secret: &Secret) -> Result<()> {
        if a.ews_url.is_empty()
            && let Ok(Some(found)) = ews::autodiscover(&a.email, &autodiscover_auth(a, secret), self.tls_connector()?)
        {
            a.ews_url = found;
            if let Some(host) = url::Url::parse(&a.ews_url).ok().and_then(|u| u.host_str().map(str::to_owned)) {
                a.incoming_host = host.clone();
                a.smtp_host = host;
            }
        }
        let s = Session::open(&a.ews_endpoint(), &a.email, auth(a, secret), self.tls_connector()?)?;
        s.reload_folders()?;
        Ok(())
    }
    pub(super) fn ews_folders(&self, a: &Account, s: &Session, reload: bool) -> Result<Vec<MailFolder>> {
        let folders = if reload { s.reload_folders()? } else { s.folders()? };
        if reload {
            let _ = self.store.save_folders(&a.id, &remote_folders(&folders));
        }
        Ok(folders)
    }

    pub(super) fn ews_mail(&self, a: &Account, folders: &[MailFolder], m: &Message, pre: Option<&Prefetched>) -> Result<Mail> {
        let id = row_id(a, &m.id);
        let (folder, remote) = location(m, folders);
        let mut mail = match Prefetched::cached(pre, &self.store, &id)? {
            Some(cached) => cached,
            None => Mail {
                id,
                account: a.id.clone(),
                from: m.from_email.clone(),
                sender: if m.from_name.trim().is_empty() { m.from_email.clone() } else { m.from_name.trim().to_owned() },
                to: m.display_to.clone(),
                subject: if m.subject.is_empty() { "(No subject)".into() } else { m.subject.clone() },
                date: m.received,
                message_id: m.message_id.clone(),
                has_attachments: m.has_attachments,
                ..Default::default()
            },
        };
        if mail.date <= 0 {
            mail.date = m.received;
        }
        if mail.cc().is_empty() && !m.display_cc.is_empty() {
            mail.set_cc(m.display_cc.clone());
        }
        if !m.conversation_id.is_empty() {
            mail.set_thread_id(format!("ews:{}", m.conversation_id));
        }
        mail.folder = folder;
        mail.remote_folder = remote;
        mail.unread = !m.read;
        mail.starred = m.flagged;
        mail.answered = matches!(m.last_verb, Some(102 | 103));
        mail.forwarded = m.last_verb == Some(104);
        if m.size > 0 {
            mail.size = u32::try_from(m.size).unwrap_or(u32::MAX);
        }
        let mut labels = m.categories.clone();
        labels.sort();
        mail.set_labels(labels);
        Ok(mail)
    }

    pub(super) fn sync_ews(&self, a: &Account, mailbox: &Mailbox, limit: usize) -> Result<Vec<Mail>> {
        if mailbox.folder() == Folder::Drafts {
            return Ok(vec![]);
        }
        self.replay_offline_ews(a)?;
        self.with_ews(a, |s| {
            let folders = self.ews_folders(a, s, true)?;
            // Starred and All Mail are assembled from the other mailboxes.
            if mailbox.folder().is_virtual() {
                return self.store.newest(&a.id, mailbox.clone(), limit);
            }
            let folder = listing(&folders, mailbox)?;
            let since = (a.sync.days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut listed: Vec<Message> = Vec::new();
            let mut complete = false;
            while listed.len() < limit {
                let (page, last) = s.list(&folder, listed.len(), (limit - listed.len()).min(PAGE), since, None)?;
                let short = page.is_empty();
                listed.extend(page);
                if last || short {
                    complete = true;
                    break;
                }
            }
            listed.truncate(limit);
            let mut rows = Vec::with_capacity(listed.len());
            let pre = Prefetched::new(&self.store, listed.iter().map(|m| row_id(a, &m.id)).collect())?;
            for m in &listed {
                rows.push(self.ews_mail(a, &folders, m, Some(&pre))?);
            }
            let oldest = listed.iter().map(|m| m.received).filter(|&t| t > 0).min().unwrap_or(i64::MIN);
            let listed_ids: HashSet<&str> = rows.iter().map(|m| m.id.as_str()).collect();
            let unlisted: Vec<Mail> = self
                .store
                .summaries(&a.id, mailbox.clone())?
                .into_iter()
                .filter(|m| ews_id(a, m).is_some() && !listed_ids.contains(m.id.as_str()))
                .filter(|m| complete || m.date >= oldest)
                .collect();
            if !unlisted.is_empty() {
                let current = s.messages(&ids(a, &unlisted))?;
                let still: HashSet<String> = current.iter().map(|m| row_id(a, &m.id)).collect();
                let pre = Prefetched::new(&self.store, current.iter().map(|m| row_id(a, &m.id)).collect())?;
                for m in &current {
                    rows.push(self.ews_mail(a, &folders, m, Some(&pre))?);
                }
                let gone: Vec<String> = unlisted.iter().map(|m| m.id.clone()).filter(|id| !still.contains(id)).collect();
                if !gone.is_empty() {
                    self.store.delete_ids(&a.id, &gone)?;
                    let gone: HashSet<&String> = gone.iter().collect();
                    self.readable.lock().unwrap().retain(|id, _| !gone.contains(id));
                }
            }
            self.store.save_messages(&rows)?;
            self.store.newest(&a.id, mailbox.clone(), limit)
        })
    }

    pub(super) fn catalog_ews(&self, a: &Account, mailbox: &Mailbox, batch: usize) -> Result<Catalog> {
        self.with_ews(a, |s| {
            let folders = self.ews_folders(a, s, false)?;
            let folder = listing(&folders, mailbox)?;
            let total = folders
                .iter()
                .find(|f| f.id == folder || f.distinguished.as_deref() == Some(folder.as_str()))
                .map(|f| f.total as usize);
            let cached = self.store.count_messages(&a.id, mailbox.clone())?;
            if total.is_some_and(|t| cached >= t) {
                return Ok(Catalog { cataloged: cached, total: cached, ..Default::default() });
            }
            let since = (a.sync.days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut added = Vec::new();
            let mut offset = cached.saturating_sub(PAGE);
            let mut ended = false;
            for _ in 0..8 {
                if !added.is_empty() && self.someone_waiting(&a.id) {
                    break;
                }
                let (page, last) = s.list(&folder, offset, 250, since, None)?;
                offset += page.len();
                for m in &page {
                    if added.len() < batch.max(1) && !self.store.has_message(&row_id(a, &m.id))? {
                        added.push(self.ews_mail(a, &folders, m, None)?);
                    }
                }
                if last || page.is_empty() {
                    ended = true;
                    break;
                }
                if added.len() >= batch {
                    break;
                }
            }
            self.store.save_messages(&added)?;
            let cataloged = cached + added.len();
            let done = ended || added.is_empty();
            let total = total.unwrap_or(cataloged).max(cataloged);
            Ok(Catalog { cataloged: if done { total } else { cataloged }, total, added, layout: None })
        })
    }

    pub(super) fn download_ews(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let id = ews_id(a, m).with_context(|| t!("manage-gmail-this-message-isnt-from-this-gmail"))?;
        let raw = self.with_ews(a, |s| s.mime(id))?;
        ensure!(raw.len() <= large::MAX_DOWNLOAD, t!("manage-proton-message-exceeds-the-download-limit"));
        self.note_traffic(a, raw.len());
        self.store.save_raw(&m.id, &raw)?;
        Ok(raw)
    }

    pub(super) fn download_batch_ews(&self, a: &Account, max: usize, since: i64) -> Result<usize> {
        let missing = self.store.missing(&a.id, MAX_MESSAGE, max.min(20), since)?;
        if missing.is_empty() {
            return Ok(0);
        }
        let asked = ids(a, &missing);
        let got = self.with_ews(a, |s| s.mimes(&asked))?;
        let mut stored = 0;
        for (id, raw) in got {
            if raw.len() > large::MAX_DOWNLOAD {
                continue;
            }
            if let Some(m) = missing.iter().find(|m| ews_id(a, m) == Some(id.as_str())) {
                self.note_traffic(a, raw.len());
                self.keep_source(a, m, &raw)?;
                stored += 1;
            }
        }
        Ok(stored)
    }

    /// The old ids of messages moved lately, by their new row ids.
    fn ews_moved_key(a: &Account) -> String {
        format!("ews_moved:{}", a.id)
    }
    /// A row as it is now, where it moved since (its id changed).
    pub(super) fn ews_current(&self, a: &Account, m: &Mail) -> Mail {
        if self.store.has_message(&m.id).unwrap_or(false) {
            return m.clone();
        }
        let moved: Vec<(String, String)> = self
            .store
            .preference(&Self::ews_moved_key(a))
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let mut id = m.id.clone();
        // Moved more than once: followed to where it is now.
        for _ in 0..8 {
            match moved.iter().find(|(old, _)| *old == id) {
                Some((_, new)) => id = new.clone(),
                None => break,
            }
        }
        self.store.message(&id).ok().flatten().unwrap_or_else(|| m.clone())
    }

    /// Makes a change at Exchange. A move gives the rows their new ids
    /// here (saying so in `moved`); other changes are committed by the caller.
    pub(super) fn change_ews(&self, a: &Account, current: &[Mail], change: &Change) -> Result<bool> {
        let all = ids(a, current);
        if all.is_empty() {
            return Ok(false);
        }
        self.with_ews(a, |s| match change {
            Change::Read(read) => s.set_read(&all, *read).map(|_| false),
            Change::Star(star) => s.set_flagged(&all, *star).map(|_| false),
            Change::Mark(Mark::Answered) => s.set_last_verb(&all, 102).map(|_| false),
            Change::Mark(Mark::Forwarded) => s.set_last_verb(&all, 104).map(|_| false),
            Change::Mark(Mark::ReceiptHandled) => Ok(false),
            Change::Label(name, on) => {
                let changes: Vec<(String, Vec<String>)> = current
                    .iter()
                    .filter_map(|m| {
                        let id = ews_id(a, m)?.to_owned();
                        let mut labels: Vec<String> = m.labels().iter().filter(|l| !l.eq_ignore_ascii_case(name)).cloned().collect();
                        if *on {
                            labels.push(name.clone());
                        }
                        Some((id, labels))
                    })
                    .collect();
                s.set_categories(&changes).map(|_| false)
            }
            Change::Move(_) | Change::MoveTo(_) => {
                let folders = s.folders()?;
                let (target, folder, remote) = match change {
                    Change::Move(f) => (
                        folder_for(&folders, *f).with_context(|| t!("manage-gmail-gmail-doesnt-move-mail-to", label = f.label()))?,
                        *f,
                        f.label().to_owned(),
                    ),
                    Change::MoveTo(name) => {
                        let id = match folder_id(&folders, name) {
                            Ok(id) => id,
                            Err(_) => folder_id(&s.reload_folders()?, name)?,
                        };
                        (id, Folder::Other, name.clone())
                    }
                    _ => unreachable!("a move"),
                };
                let new_ids = s.move_to(&all, &target)?;
                // The rows take their new ids; the old ones are kept a while.
                let mut moved: Vec<(String, String)> = self
                    .store
                    .preference(&Self::ews_moved_key(a))
                    .and_then(|v| serde_json::from_str(&v).ok())
                    .unwrap_or_default();
                let mut rows = Vec::new();
                let mut gone = Vec::new();
                let mut renamed = Vec::new();
                for (m, new) in current.iter().filter(|m| ews_id(a, m).is_some()).zip(new_ids) {
                    gone.push(m.id.clone());
                    if new.is_empty() {
                        continue;
                    }
                    let mut row = m.clone();
                    row.id = row_id(a, &new);
                    row.folder = folder;
                    row.remote_folder = if folder == Folder::Other {
                        remote.clone()
                    } else {
                        folders
                            .iter()
                            .find(|f| f.id == target || f.distinguished.as_deref() == Some(target.as_str()))
                            .map_or(remote.clone(), |f| f.name.clone())
                    };
                    renamed.push((m.id.clone(), row.id.clone()));
                    moved.push((m.id.clone(), row.id.clone()));
                    rows.push(row);
                }
                let skip = moved.len().saturating_sub(MOVED_KEPT);
                moved.drain(..skip);
                self.store.set_preference(&Self::ews_moved_key(a), &serde_json::to_string(&moved)?)?;
                // The new rows first, then their downloaded sources copied
                // over as stored (not inflated and compressed again), and
                // only then the old rows gone: saved before its row existed,
                // a source was dropped, and the message downloaded again.
                self.store.save_messages(&rows)?;
                self.store.copy_sources(&renamed)?;
                self.store.delete_ids(&a.id, &gone)?;
                Ok(true)
            }
        })
    }
    pub(super) fn replay_offline_ews(&self, a: &Account) -> Result<()> {
        self.replay_offline_with(a, |mails, change| {
            let now: Vec<Mail> = mails.iter().map(|m| self.ews_current(a, m)).collect();
            self.change_ews(a, &now, change).map(drop)
        })
    }

    pub(super) fn delete_ews(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let ids = ids(a, mails);
        if !ids.is_empty() {
            self.with_ews(a, |s| s.delete(&ids))?;
        }
        self.store.commit_changes(mails, true)
    }
    pub(super) fn empty_ews(&self, a: &Account, folder: Folder) -> Result<usize> {
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_ews(a, |s| {
            let folders = s.folders()?;
            let target = folder_for(&folders, folder).with_context(|| t!("manage-proton-choose-one-of-the-accounts-folders"))?;
            let count = s.ids_in(&target, false, 100_000)?.len();
            s.empty(&target)?;
            Ok(count)
        })?;
        let cached = self.store.summaries(&a.id, mailbox)?;
        self.store.commit_changes(&cached, true)?;
        Ok(count)
    }
    /// Server search: Exchange's own (AQS), within the folder.
    pub(super) fn search_ews(&self, a: &Account, mailbox: &Mailbox, query: &str, limit: usize) -> Result<ServerResults> {
        self.with_ews(a, |s| {
            let folders = s.folders()?;
            let folder = if mailbox.folder().is_virtual() { "msgfolderroot".to_owned() } else { listing(&folders, mailbox)? };
            let mut found = Vec::new();
            while found.len() < SEARCH_MOST {
                let (page, last) = s.list(&folder, found.len(), 250, None, Some(query.trim()))?;
                let short = page.is_empty();
                found.extend(page);
                if last || short {
                    break;
                }
            }
            let mut out = ServerResults { total: found.len(), ..Default::default() };
            let pre = Prefetched::new(&self.store, found.iter().take(limit).map(|m| row_id(a, &m.id)).collect())?;
            for m in found.iter().take(limit) {
                out.mails.push(self.ews_mail(a, &folders, m, Some(&pre))?);
            }
            self.store.save_messages(&out.mails)?;
            out.mails.sort_by(Mail::newest_first);
            Ok(out)
        })
    }
    pub(super) fn mark_all_read_ews(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        self.with_ews(a, |s| {
            let folders = s.folders()?;
            let folder = listing(&folders, mailbox)?;
            let unread = s.ids_in(&folder, true, 20_000)?;
            if !unread.is_empty() {
                s.set_read(&unread, true)?;
            }
            Ok(())
        })?;
        self.store.mark_mailbox_read(&a.id, mailbox.clone())
    }
    /// Sends through Exchange, which keeps it in Sent Items; with
    /// `hold_until`, Exchange holds it until then.
    pub(super) fn send_ews(&self, a: &Account, d: &Draft, raw: &[u8], hold_until: Option<i64>) -> Result<()> {
        let raw = crate::sync::smtp::with_bcc(raw.to_vec(), d)?;
        self.with_ews(a, |s| s.send(&raw, hold_until))
    }
    pub(super) fn file_sent_copy_ews(&self, a: &Account, raw: &[u8]) -> Result<()> {
        self.with_ews(a, |s| s.import(raw, "sentitems", true).map(drop))
    }
    /// Puts imported mail in a mailbox, as received mail.
    pub(super) fn import_ews(&self, a: &Account, mailbox: &Mailbox, raw: &[u8], read: bool) -> Result<()> {
        self.with_ews(a, |s| {
            let folder = listing(&s.folders()?, mailbox)?;
            s.import(raw, &folder, read).map(drop)
        })
    }

    pub(super) fn sync_drafts_ews(&self, a: &Account) -> Result<usize> {
        let _ = self.file_queued_sent_copies_with(a, |raw| self.file_sent_copy_ews(a, raw));
        self.with_ews(a, |s| {
            let pending = self.draft_removals(&a.id);
            let gone: Vec<String> = pending.iter().filter_map(ews_draft).map(str::to_owned).collect();
            if !gone.is_empty()
                && let Err(e) = s.delete(&gone)
                && crate::sync::retry::later(&e)
            {
                return Err(e);
            }
            self.clear_draft_removals(&a.id, &pending)?;
            const MOST: usize = 20_000;
            let mut on_server = s.ids_in("drafts", false, MOST + 1)?;
            let complete = on_server.len() <= MOST;
            on_server.truncate(MOST);
            let listed: HashSet<&str> = on_server.iter().map(String::as_str).collect();
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            for d in &mut locals {
                let Some(id) = d.remote.as_ref().and_then(ews_draft) else { continue };
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
            let claimed: HashSet<String> =
                locals.iter().filter_map(|d| d.remote.as_ref().and_then(ews_draft).map(str::to_owned)).collect();
            let new: Vec<String> = on_server.iter().filter(|id| !claimed.contains(*id)).cloned().collect();
            for (id, raw) in s.mimes(&new)? {
                ensure!(raw.len() <= MAX_MESSAGE, t!("manage-gmail-a-draft-at-gmail-is-too-large"));
                let mut d = self.draft_from(a, &uuid::Uuid::new_v4().to_string(), &raw)?;
                d.remote = Some(ews_remote(&id));
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
                let Some(raw) = self.draft_upload(a, d)? else {
                    if let Some(old) = d.remote.as_ref().and_then(ews_draft) {
                        if let Err(e) = s.delete(&[old.to_owned()])
                            && crate::sync::retry::later(&e)
                        {
                            return Err(e);
                        }
                        self.keep_draft_here(d)?;
                    }
                    continue;
                };
                let id = s.create_draft(&raw)?;
                if let Some(old) = d.remote.as_ref().and_then(ews_draft) {
                    let _ = s.delete(&[old.to_owned()]);
                }
                if !self.finish_draft_upload(d, ews_remote(&id), fingerprint)? && s.delete(std::slice::from_ref(&id)).is_err() {
                    self.queue_draft_removal(&a.id, ews_remote(&id))?;
                }
            }
            Ok(changed)
        })
    }

    // Folders.

    pub(super) fn create_folder_ews(&self, a: &Account, parent: Option<&str>, leaf: &str) -> Result<FolderChange> {
        self.with_ews(a, |s| {
            let folders = s.folders()?;
            let parent = match parent {
                Some(p) => folder_id(&folders, p)?,
                None => "msgfolderroot".to_owned(),
            };
            s.create_folder(&parent, leaf)?;
            let folders = remote_folders(&s.folders()?);
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange { moved: Vec::new(), folders })
        })
    }
    pub(super) fn rename_folder_ews(&self, a: &Account, name: &str, leaf: &str) -> Result<FolderChange> {
        let change = self.with_ews(a, |s| {
            let before = s.folders()?;
            s.rename_folder(&folder_id(&before, name)?, leaf)?;
            let after = s.folders()?;
            let moved = own(&before)
                .filter_map(|old| {
                    let new = after.iter().find(|n| n.id == old.id)?;
                    (new.path != old.path).then(|| (old.path.clone(), new.path.clone()))
                })
                .collect();
            Ok(FolderChange { moved, folders: remote_folders(&after) })
        })?;
        self.store.save_folders(&a.id, &change.folders)?;
        for (old, new) in &change.moved {
            self.store.rename_remote(&a.id, old, new)?;
        }
        Ok(change)
    }
    pub(super) fn delete_folder_ews(&self, a: &Account, name: &str) -> Result<Vec<RemoteFolder>> {
        let folders = self.with_ews(a, |s| {
            s.delete_folder(&folder_id(&s.folders()?, name)?)?;
            Ok(remote_folders(&s.folders()?))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        let rows = self.store.summaries(&a.id, Mailbox::Remote(name.to_owned()))?;
        self.store.commit_changes(&rows, true)?;
        Ok(folders)
    }

    // Filters: inbox rules and Out of Office, as for Microsoft 365.

    /// The folders as Graph's would be, for the rule conversion.
    fn ews_rule_folders(folders: &[MailFolder]) -> Vec<crate::sync::graph::MailFolder> {
        folders
            .iter()
            .map(|f| crate::sync::graph::MailFolder {
                id: f.id.clone(),
                name: f.name.clone(),
                path: f.path.clone(),
                depth: f.depth,
                well_known: f.distinguished.clone(),
                ..Default::default()
            })
            .collect()
    }
    fn ews_replies(o: &ews::Oof) -> crate::sync::graph::AutomaticReplies {
        crate::sync::graph::AutomaticReplies {
            enabled: o.enabled,
            internal: o.internal.clone(),
            external: o.external.clone(),
            contacts_only: false,
            start: o.start,
            end: o.end,
        }
    }
    pub(super) fn filters_ews(&self, a: &Account) -> Result<crate::sync::sieve::State> {
        self.with_ews(a, |s| {
            let folders = Self::ews_rule_folders(&s.folders()?);
            let rules: Vec<crate::sync::graph::Rule> =
                s.rules()?.into_iter().map(|r| crate::sync::graph::Rule { id: r.id, json: r.json }).collect();
            Ok(super::api_filters::graph_state(&rules, &folders, &Self::ews_replies(&s.oof()?)).0)
        })
    }
    pub(super) fn save_filters_ews(&self, a: &Account, filters: &crate::sync::sieve::Filters) -> Result<crate::sync::sieve::State> {
        self.with_ews(a, |s| {
            let folders = Self::ews_rule_folders(&s.reload_folders()?);
            let wanted = super::api_filters::graph_rules(&filters.rules, &folders)?;
            let current: Vec<crate::sync::graph::Rule> =
                s.rules()?.into_iter().map(|r| crate::sync::graph::Rule { id: r.id, json: r.json }).collect();
            let (_, ids) = super::api_filters::graph_state(&current, &folders, &Default::default());
            let mut operations = String::new();
            for (i, rule) in wanted.iter().enumerate() {
                match ids.get(i) {
                    Some(id) => operations.push_str(&format!("<t:SetRuleOperation>{}</t:SetRuleOperation>", ews::rule_xml(rule, Some(id)))),
                    None => operations.push_str(&format!("<t:CreateRuleOperation>{}</t:CreateRuleOperation>", ews::rule_xml(rule, None))),
                }
            }
            for id in ids.iter().skip(wanted.len()) {
                operations.push_str(&format!("<t:DeleteRuleOperation><t:RuleId>{}</t:RuleId></t:DeleteRuleOperation>", ews::esc(id)));
            }
            s.update_rules(&operations)?;
            let replies = super::api_filters::graph_replies(&filters.vacation);
            s.set_oof(&ews::Oof { enabled: replies.enabled, internal: replies.internal, external: replies.external, start: replies.start, end: replies.end })?;
            let rules: Vec<crate::sync::graph::Rule> =
                s.rules()?.into_iter().map(|r| crate::sync::graph::Rule { id: r.id, json: r.json }).collect();
            Ok(super::api_filters::graph_state(&rules, &folders, &Self::ews_replies(&s.oof()?)).0)
        })
    }

    // Calendars and contacts.

    pub(super) fn with_ews_locked<T>(&self, a: &Account, work: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_ews(a, work)
    }
    /// Every event of the mailbox's calendars ending after `from`, each
    /// series with its changed occurrences: (calendars, events).
    pub(super) fn ews_events(&self, a: &Account, from: i64) -> Result<(Vec<caldav::Calendar>, Vec<Stored>)> {
        self.with_ews_locked(a, |s| {
            let calendars = s.calendars()?;
            let mut stored = Vec::new();
            for (folder, _) in &calendars {
                let ids = s.item_ids(folder, Some(from), 20_000)?;
                for (id, ics) in s.mimes(&ids)? {
                    let mut events = crate::sync::ical::parse(&String::from_utf8_lossy(&ics)).events;
                    events.retain(|e| !e.task);
                    if events.is_empty() {
                        continue;
                    }
                    let series = events.iter().position(|e| e.recurrence_id.is_none()).unwrap_or(0);
                    let event = events.remove(series);
                    stored.push(Stored {
                        event,
                        overrides: events,
                        calendar: format!("ews:{folder}"),
                        href: format!("ews:{id}"),
                        etag: String::new(),
                        schedule_tag: String::new(),
                    });
                }
            }
            let calendars = calendars
                .into_iter()
                .map(|(id, name)| caldav::Calendar {
                    href: format!("ews:{id}"),
                    name,
                    writable: true,
                    components: vec!["VEVENT".into()],
                    ..Default::default()
                })
                .collect();
            Ok((calendars, stored))
        })
    }
    /// Saves an event in an Exchange calendar: made anew from iCalendar
    /// (Exchange takes events only so), then the earlier one goes without
    /// telling guests; the guests are invited (or told of the change) by
    /// Exchange where `invite`. Made first so that a failure leaves the
    /// event as it was. Where Exchange won't make it beside the earlier one
    /// (the same UID), the earlier one goes first, and is put back as it
    /// was if the new one still can't be made.
    pub(super) fn ews_save_event(&self, a: &Account, calendar: &str, stored: &Stored, invite: bool) -> Result<Stored> {
        let folder = calendar.strip_prefix("ews:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
        let ics = if stored.overrides.is_empty() {
            crate::sync::ical::to_store(&stored.event)
        } else {
            crate::sync::ical::to_store_all(&stored.event, &stored.overrides)
        };
        let id = self.with_ews_locked(a, |s| {
            let Some(old) = stored.href.strip_prefix("ews:").filter(|i| !i.is_empty()) else {
                return s.create_event(folder, &ics, invite);
            };
            match s.create_event(folder, &ics, invite) {
                Ok(id) => {
                    // Made: the earlier one left behind only shows twice.
                    if let Err(e) = s.delete_event(old, false) {
                        eprintln!("Picacho: the earlier copy of an Exchange event stayed: {e:#}");
                    }
                    Ok(id)
                }
                // Not reached, signed out or too fast: nothing to try instead.
                Err(e)
                    if crate::sync::retry::later(&e)
                        || e.chain().any(|c| c.is::<reqwest::Error>())
                        || e.downcast_ref::<AccessFailure>().is_some()
                        || e.to_string().starts_with("[THROTTLED]") =>
                {
                    Err(e)
                }
                Err(_) => {
                    let before = s.mime(old).ok();
                    s.delete_event(old, false)?;
                    s.create_event(folder, &ics, invite).inspect_err(|_| {
                        if let Some(before) = &before {
                            let _ = s.create_event(folder, &String::from_utf8_lossy(before), false);
                        }
                    })
                }
            }
        })?;
        Ok(Stored { calendar: calendar.to_owned(), href: format!("ews:{id}"), etag: String::new(), ..stored.clone() })
    }
    /// Contacts in the default contacts folder, as cards.
    pub(super) fn ews_contacts(&self, a: &Account) -> Result<Vec<Card>> {
        self.with_ews_locked(a, |s| {
            let ids = s.item_ids("contacts", None, 50_000)?;
            let mut out = Vec::new();
            for (id, vcard) in s.mimes(&ids)? {
                for mut card in carddav::parse_vcards(&String::from_utf8_lossy(&vcard)) {
                    card.uid = format!("ews-{id}");
                    card.href = format!("ews:{id}");
                    out.push(card);
                }
            }
            Ok(out)
        })
    }
    /// Saves a contact: made anew (Exchange's ids change anyway), then the
    /// earlier one goes, so a failure leaves it as it was.
    pub(super) fn ews_save_contact(&self, a: &Account, card: &Card) -> Result<Card> {
        let xml = contact_xml(card);
        let id = self.with_ews_locked(a, |s| {
            let id = s.create_contact("contacts", &xml)?;
            if let Some(old) = card.href.strip_prefix("ews:").filter(|i| !i.is_empty())
                && let Err(e) = s.delete(&[old.to_owned()])
            {
                eprintln!("Picacho: the earlier copy of an Exchange contact stayed: {e:#}");
            }
            Ok(id)
        })?;
        Ok(Card { uid: format!("ews-{id}"), href: format!("ews:{id}"), ..card.clone() })
    }
    pub(super) fn ews_delete_contact(&self, a: &Account, card: &Card) -> Result<()> {
        let Some(id) = card.href.strip_prefix("ews:") else { return Ok(()) };
        match self.with_ews_locked(a, |s| s.delete(&[id.to_owned()])) {
            Err(e) if not_found(&e) => Ok(()),
            other => other,
        }
    }
    /// People the directory knows by `text`, as cards (not saved there).
    pub(super) fn ews_directory(&self, a: &Account, text: &str) -> Result<Vec<Card>> {
        let found = self.with_ews_locked(a, |s| s.resolve_names(text))?;
        Ok(found
            .into_iter()
            .map(|(name, email, title, org, phones)| Card {
                uid: format!("ews-directory-{}", email.to_lowercase()),
                href: format!("ews-directory:{email}"),
                name,
                emails: vec![email],
                title,
                org,
                phones,
                ..Default::default()
            })
            .collect())
    }
}

/// A card as an Exchange contact (`t:Contact`), in the schema's order.
fn contact_xml(c: &Card) -> String {
    let esc = ews::esc;
    let mut x = String::from("<t:Contact>");
    if !c.note.is_empty() {
        x.push_str(&format!(r#"<t:Body BodyType="Text">{}</t:Body>"#, esc(&c.note)));
    }
    x.push_str(&format!("<t:FileAs>{0}</t:FileAs><t:DisplayName>{0}</t:DisplayName>", esc(&c.name)));
    if let Some(n) = c.nicknames.first() {
        x.push_str(&format!("<t:Nickname>{}</t:Nickname>", esc(n)));
    }
    if !c.org.is_empty() {
        x.push_str(&format!("<t:CompanyName>{}</t:CompanyName>", esc(&c.org)));
    }
    if !c.emails.is_empty() {
        x.push_str("<t:EmailAddresses>");
        for (i, e) in c.emails.iter().take(3).enumerate() {
            x.push_str(&format!(r#"<t:Entry Key="EmailAddress{}">{}</t:Entry>"#, i + 1, esc(e)));
        }
        x.push_str("</t:EmailAddresses>");
    }
    if !c.phones.is_empty() {
        x.push_str("<t:PhoneNumbers>");
        for (key, p) in ["MobilePhone", "BusinessPhone", "HomePhone", "BusinessPhone2", "OtherTelephone"].iter().zip(&c.phones) {
            x.push_str(&format!(r#"<t:Entry Key="{key}">{}</t:Entry>"#, esc(p)));
        }
        x.push_str("</t:PhoneNumbers>");
    }
    if !c.birthday.is_empty() {
        x.push_str(&format!("<t:Birthday>{}T12:00:00Z</t:Birthday>", esc(&c.birthday)));
    }
    if let Some(u) = c.urls.first() {
        x.push_str(&format!("<t:BusinessHomePage>{}</t:BusinessHomePage>", esc(u)));
    }
    if !c.title.is_empty() {
        x.push_str(&format!("<t:JobTitle>{}</t:JobTitle>", esc(&c.title)));
    }
    x.push_str("</t:Contact>");
    x
}
