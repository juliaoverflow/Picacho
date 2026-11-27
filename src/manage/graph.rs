//! Outlook and Microsoft 365 through Microsoft Graph (`sync/graph.rs`):
//! what the account operations do for an Outlook account with
//! `Account::api`, instead of IMAP and SMTP. For organizations that turn
//! IMAP off, it is the only way in.
//!
//! A message keeps one id wherever it goes (Graph's immutable ids), so it
//! is one row here (`{account}:graph:{id}`), in the folder Graph says.
//! Its categories are the row's labels; Flagged is Starred; whether it was
//! answered or forwarded is the mailbox's own (PR_LAST_VERB_EXECUTED), as
//! Outlook shows it. A listing carries all of that, so a sync lists the
//! newest messages and asks only about cached ones that weren't listed,
//! as for Proton.
use super::*;
use crate::sync::graph::{self, MailFolder, Message, Session, shown, well_known};
use crate::sync::rest::not_found;
use serde_json::json;

/// Messages listed at once.
const PAGE: usize = 100;
/// The most messages a server search returns.
const SEARCH_MOST: usize = 500;

fn graph_remote(id: &str) -> RemoteDraft {
    RemoteDraft {
        mailbox: format!("graph:{id}"),
        validity: 0,
        uid: 0,
    }
}
fn graph_draft(r: &RemoteDraft) -> Option<&str> {
    r.mailbox.strip_prefix("graph:")
}

/// The Graph message id of one of our rows.
pub(super) fn graph_id<'a>(a: &Account, m: &'a Mail) -> Option<&'a str> {
    m.id.strip_prefix(&a.id)?.strip_prefix(":graph:")
}
pub(super) fn row_id(a: &Account, id: &str) -> String {
    format!("{}:graph:{id}", a.id)
}
fn ids(a: &Account, mails: &[Mail]) -> Vec<String> {
    mails.iter().filter_map(|m| graph_id(a, m).map(str::to_owned)).collect()
}

/// The standard mailbox a well-known folder is.
fn standard(well_known: &str) -> Option<Folder> {
    Some(match well_known {
        well_known::INBOX => Folder::Inbox,
        well_known::SENT => Folder::Sent,
        well_known::DRAFTS => Folder::Drafts,
        well_known::TRASH => Folder::Trash,
        well_known::JUNK => Folder::Junk,
        well_known::ARCHIVE => Folder::Archive,
        _ => return None,
    })
}
/// A standard mailbox's well-known folder name.
fn well_known_of(folder: Folder) -> Option<&'static str> {
    Some(match folder {
        Folder::Inbox => well_known::INBOX,
        Folder::Sent => well_known::SENT,
        Folder::Drafts => well_known::DRAFTS,
        Folder::Trash => well_known::TRASH,
        Folder::Junk => well_known::JUNK,
        Folder::Archive => well_known::ARCHIVE,
        _ => return None,
    })
}

/// The account's own folders: not standard, not hidden.
fn own(folders: &[MailFolder]) -> impl Iterator<Item = &MailFolder> {
    folders.iter().filter(|f| f.well_known.is_none())
}

/// Where a message is: (folder, its name).
pub(super) fn location(m: &Message, folders: &[MailFolder]) -> (Folder, String) {
    match folders.iter().find(|f| f.id == m.folder_id) {
        Some(f) => match f.well_known.as_deref().and_then(standard) {
            Some(folder) => (folder, f.name.clone()),
            None => (Folder::Other, f.path.clone()),
        },
        // Not one Picacho listed (a hidden folder, or a new one): where
        // other mail apps show such mail.
        None => (Folder::Archive, "Archive".into()),
    }
}

/// How a mailbox is listed: a folder (by id or well-known name), or
/// across the mailbox with a filter (Starred, All Mail).
pub(super) enum Listing {
    Folder(String),
    Everywhere(Option<&'static str>),
}
pub(super) fn listing(folders: &[MailFolder], mailbox: &Mailbox) -> Result<Listing> {
    Ok(match mailbox {
        Mailbox::Standard(Folder::Starred) => Listing::Everywhere(Some("flag/flagStatus eq 'flagged'")),
        Mailbox::Standard(Folder::All) => Listing::Everywhere(None),
        Mailbox::Standard(folder) => Listing::Folder(
            well_known_of(*folder)
                .with_context(|| t!("manage-proton-choose-one-of-the-accounts-folders"))?
                .to_owned(),
        ),
        Mailbox::Remote(name) => Listing::Folder(folder_id(folders, name)?),
    })
}
fn folder_id(folders: &[MailFolder], path: &str) -> Result<String> {
    own(folders)
        .find(|f| f.path == path)
        .map(|f| f.id.clone())
        .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = path))
}

/// The account's folders, as the folder list shows them.
pub(super) fn remote_folders(folders: &[MailFolder]) -> Vec<RemoteFolder> {
    // Inside a hidden one (Sync Issues' folders) is hidden too.
    let hidden: Vec<&str> = folders
        .iter()
        .filter(|f| f.well_known.as_deref().is_some_and(|w| well_known::HIDDEN.contains(&w)))
        .map(|f| f.path.as_str())
        .collect();
    own(folders)
        .filter(|f| !hidden.iter().any(|h| f.path.starts_with(&format!("{h}/"))))
        .map(|f| RemoteFolder {
            name: f.path.clone(),
            label: f.name.clone(),
            depth: f.depth,
            nestable: true,
        })
        .collect()
}

impl Backend {
    /// The account's Graph session: opened on first use, then kept until
    /// its access token is about to expire (then renewed and opened again).
    fn graph_session(&self, a: &Account) -> Result<Arc<Session>> {
        let now = chrono::Utc::now().timestamp();
        if let Some((s, expires)) = self.graph.lock().unwrap().get(&a.id)
            && *expires > now + 120
        {
            return Ok(s.clone());
        }
        let secret = self.credentials(a)?;
        let session = Arc::new(self.open_graph(a, &secret)?);
        self.graph.lock().unwrap().insert(a.id.clone(), (session.clone(), secret.expires_at));
        Ok(session)
    }
    fn open_graph(&self, a: &Account, secret: &Secret) -> Result<Session> {
        Session::open(&gmail::api_base(a, graph::BASE), &secret.access_token, self.tls_connector()?)
    }
    /// Runs `work` with the account's session; a refused sign-in drops it.
    pub(super) fn with_graph<T>(&self, a: &Account, work: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        let session = self.graph_session(a)?;
        let result = work(&session);
        if let Err(e) = &result {
            if e.downcast_ref::<AccessFailure>().is_some() {
                self.graph.lock().unwrap().remove(&a.id);
            }
            self.note_limit(a, e);
        }
        result
    }
    /// Signs in to check the account (adding or reconnecting it).
    pub(super) fn check_graph(&self, a: &Account, secret: &Secret) -> Result<()> {
        let s = self.open_graph(a, secret)?;
        s.reload_folders()?;
        Ok(())
    }
    pub(super) fn graph_folders(&self, a: &Account, s: &Session, reload: bool) -> Result<Vec<MailFolder>> {
        let folders = if reload { s.reload_folders()? } else { s.folders()? };
        if reload {
            let _ = self.store.save_folders(&a.id, &remote_folders(&folders));
        }
        Ok(folders)
    }

    /// A row for a listed message: the cached one with its flags, folder
    /// and labels brought up to date, or a new one from the listing.
    pub(super) fn graph_mail(&self, a: &Account, folders: &[MailFolder], m: &Message, pre: Option<&Prefetched>) -> Result<Mail> {
        let id = row_id(a, &m.id);
        let (folder, remote) = location(m, folders);
        let mut mail = match Prefetched::cached(pre, &self.store, &id)? {
            Some(cached) => cached,
            None => Mail {
                id,
                account: a.id.clone(),
                from: m.from.as_ref().map(|f| f.email.clone()).unwrap_or_default(),
                sender: m
                    .from
                    .as_ref()
                    .map(|f| if f.name.trim().is_empty() { f.email.clone() } else { f.name.trim().to_owned() })
                    .unwrap_or_default(),
                to: shown(&m.to),
                reply_to: shown(&m.reply_to),
                subject: if m.subject.is_empty() { "(No subject)".into() } else { m.subject.clone() },
                snippet: m.preview.clone(),
                date: m.received,
                message_id: m.message_id.clone(),
                has_attachments: m.has_attachments,
                ..Default::default()
            },
        };
        if mail.date <= 0 {
            mail.date = m.received;
        }
        mail.set_cc(shown(&m.cc));
        if !m.conversation_id.is_empty() {
            mail.set_thread_id(format!("graph:{}", m.conversation_id));
        }
        // An out-of-office reply or a report, as Exchange classes it.
        if crate::sync::mime::automatic_class(&m.item_class) {
            mail.set_automatic(true);
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
        // The Focused Inbox's sorting, where the mailbox has it.
        let sorted = crate::manage::categories::outlook_category(&m.classification);
        mail.set_server_category(sorted);
        Ok(mail)
    }

    /// One page of a mailbox's messages, newest first.
    fn graph_page(
        &self,
        s: &Session,
        listing: &Listing,
        since: Option<i64>,
        next: Option<&str>,
        top: usize,
    ) -> Result<(Vec<Message>, Option<String>)> {
        match listing {
            Listing::Folder(id) => s.list(id, since, next, top),
            Listing::Everywhere(filter) => {
                let path = match next {
                    Some(next) => next.to_owned(),
                    None => {
                        // Sorted by arrival, the filter must start with it.
                        let mut f = format!("receivedDateTime ge {}", graph::graph_time(since.unwrap_or(0)));
                        if let Some(filter) = filter {
                            f.push_str(&format!(" and {filter}"));
                        }
                        format!(
                            "/v1.0/me/messages?$top={}&$select={}&$orderby=receivedDateTime%20desc&$filter={}",
                            top.clamp(1, 1000),
                            "id,conversationId,parentFolderId,subject,from,toRecipients,ccRecipients,replyTo,receivedDateTime,isRead,flag,categories,hasAttachments,bodyPreview,internetMessageId,isDraft,inferenceClassification",
                            crate::sync::rest::escape(&f)
                        )
                    }
                };
                let v = s.rest.get(&path)?;
                let messages = v["value"].as_array().map(|l| l.iter().map(Message::from_json).collect()).unwrap_or_default();
                Ok((messages, v["@odata.nextLink"].as_str().map(str::to_owned)))
            }
        }
    }

    /// Brings a mailbox up to date: lists its newest `limit` messages, and
    /// asks again about cached ones that weren't listed (moved elsewhere,
    /// or deleted).
    pub(super) fn sync_graph(&self, a: &Account, mailbox: &Mailbox, limit: usize) -> Result<Vec<Mail>> {
        if mailbox.folder() == Folder::Drafts {
            return Ok(vec![]);
        }
        self.replay_offline_graph(a)?;
        self.with_graph(a, |s| {
            let folders = self.graph_folders(a, s, true)?;
            let listing = listing(&folders, mailbox)?;
            let since = (a.sync.days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut listed: Vec<Message> = Vec::new();
            let mut next: Option<String> = None;
            let mut complete = false;
            while listed.len() < limit {
                let (page, more) = self.graph_page(s, &listing, since, next.as_deref(), (limit - listed.len()).min(PAGE))?;
                listed.extend(page);
                match more {
                    Some(more) => next = Some(more),
                    None => {
                        complete = true;
                        break;
                    }
                }
            }
            listed.truncate(limit);
            let mut rows = Vec::with_capacity(listed.len());
            let pre = Prefetched::new(&self.store, listed.iter().map(|m| row_id(a, &m.id)).collect())?;
            for m in &listed {
                rows.push(self.graph_mail(a, &folders, m, Some(&pre))?);
            }
            let oldest = listed.iter().map(|m| m.received).filter(|&t| t > 0).min().unwrap_or(i64::MIN);
            let listed_ids: HashSet<&str> = rows.iter().map(|m| m.id.as_str()).collect();
            let unlisted: Vec<Mail> = self
                .store
                .summaries(&a.id, mailbox.clone())?
                .into_iter()
                .filter(|m| graph_id(a, m).is_some() && !listed_ids.contains(m.id.as_str()))
                .filter(|m| complete || m.date >= oldest)
                .collect();
            if !unlisted.is_empty() {
                let current = s.messages(&ids(a, &unlisted))?;
                let still: HashSet<String> = current.iter().map(|m| row_id(a, &m.id)).collect();
                let pre = Prefetched::new(&self.store, current.iter().map(|m| row_id(a, &m.id)).collect())?;
                for m in &current {
                    rows.push(self.graph_mail(a, &folders, m, Some(&pre))?);
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

    /// Lists `batch` more of a mailbox's messages beyond those cached,
    /// newest first.
    pub(super) fn catalog_graph(&self, a: &Account, mailbox: &Mailbox, batch: usize) -> Result<Catalog> {
        self.with_graph(a, |s| {
            let folders = self.graph_folders(a, s, false)?;
            let listing = listing(&folders, mailbox)?;
            let total = match &listing {
                Listing::Folder(id) => Some(s.folder_total(id)? as usize),
                Listing::Everywhere(_) => None,
            };
            let cached = self.store.count_messages(&a.id, mailbox.clone())?;
            if total.is_some_and(|t| cached >= t) {
                return Ok(Catalog { cataloged: cached, total: cached, ..Default::default() });
            }
            let since = (a.sync.days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(a.sync.days) * 86_400);
            let mut added = Vec::new();
            let mut next: Option<String> = None;
            let mut ended = false;
            for _ in 0..8 {
                if !added.is_empty() && self.someone_waiting(&a.id) {
                    break;
                }
                let (page, more) = self.graph_page(s, &listing, since, next.as_deref(), 250)?;
                for m in &page {
                    if added.len() < batch.max(1) && !self.store.has_message(&row_id(a, &m.id))? {
                        added.push(self.graph_mail(a, &folders, m, None)?);
                    }
                }
                match more {
                    Some(more) if added.len() < batch => next = Some(more),
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
                total,
                added,
                layout: None,
            })
        })
    }

    /// Downloads one message's source and caches it.
    pub(super) fn download_graph(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let raw = self.fetch_graph_raw(a, m)?;
        self.store.save_raw(&m.id, &raw)?;
        Ok(raw)
    }

    /// One message's source, as Microsoft has it, not yet kept.
    fn fetch_graph_raw(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let id = graph_id(a, m).with_context(|| t!("manage-gmail-this-message-isnt-from-this-gmail"))?;
        let raw = self.with_graph(a, |s| s.raw(id))?;
        ensure!(raw.len() <= large::MAX_DOWNLOAD, t!("manage-proton-message-exceeds-the-download-limit"));
        self.note_traffic(a, raw.len());
        Ok(raw)
    }

    /// Background downloads: up to `max` messages only listed so far.
    pub(super) fn download_batch_graph(&self, a: &Account, max: usize, since: i64) -> Result<usize> {
        let mut stored = 0;
        for m in self.store.missing(&a.id, MAX_MESSAGE, max.min(20), since)? {
            if stored > 0 && self.someone_waiting(&a.id) {
                break;
            }
            // Kept once (`keep_source`), not compressed and written twice.
            let raw = match self.fetch_graph_raw(a, &m) {
                Ok(raw) => raw,
                Err(e) if not_found(&e) => {
                    self.store.delete_ids(&a.id, std::slice::from_ref(&m.id))?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            self.keep_source(a, &m, &raw)?;
            stored += 1;
        }
        Ok(stored)
    }

    /// Makes a change at Microsoft.
    pub(super) fn change_graph(&self, a: &Account, current: &[Mail], change: &Change) -> Result<()> {
        let all = ids(a, current);
        if all.is_empty() {
            return Ok(());
        }
        self.with_graph(a, |s| match change {
            Change::Read(read) => s.update(&all, &json!({"isRead": read})),
            Change::Star(star) => s.update(&all, &json!({"flag": {"flagStatus": if *star { "flagged" } else { "notFlagged" }}})),
            Change::Move(folder) => {
                let target = well_known_of(*folder)
                    .with_context(|| t!("manage-gmail-gmail-doesnt-move-mail-to", label = folder.label()))?;
                s.move_to(&all, target)
            }
            Change::MoveTo(name) => {
                let folders = s.folders()?;
                let target = match folder_id(&folders, name) {
                    Ok(id) => id,
                    // Made elsewhere since the folders were read.
                    Err(_) => folder_id(&s.reload_folders()?, name)?,
                };
                s.move_to(&all, &target)
            }
            Change::Label(name, on) => {
                // Categories are set whole: each message's own, changed.
                let patches: Vec<(String, serde_json::Value)> = current
                    .iter()
                    .filter_map(|m| {
                        let id = graph_id(a, m)?.to_owned();
                        let mut labels: Vec<String> =
                            m.labels().iter().filter(|l| !l.eq_ignore_ascii_case(name)).cloned().collect();
                        if *on {
                            labels.push(name.clone());
                        }
                        Some((id, json!({"categories": labels})))
                    })
                    .collect();
                s.update_each(&patches)
            }
            Change::Mark(Mark::Answered) => s.update(&all, &last_verb(102)),
            Change::Mark(Mark::Forwarded) => s.update(&all, &last_verb(104)),
            // No such flag at Microsoft: it stays on this computer.
            Change::Mark(Mark::ReceiptHandled) => Ok(()),
        })
    }
    pub(super) fn replay_offline_graph(&self, a: &Account) -> Result<()> {
        self.replay_offline_with(a, |mails, change| self.change_graph(a, mails, change))
    }

    /// Deletes messages for good.
    pub(super) fn delete_graph(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let ids = ids(a, mails);
        if !ids.is_empty() {
            self.with_graph(a, |s| s.delete(&ids))?;
        }
        self.store.commit_changes(mails, true)
    }

    /// Deletes everything in Deleted Items or Junk for good.
    pub(super) fn empty_graph(&self, a: &Account, folder: Folder) -> Result<usize> {
        let mailbox = Mailbox::Standard(folder);
        let target = well_known_of(folder).with_context(|| t!("manage-proton-choose-one-of-the-accounts-folders"))?;
        let count = self.with_graph(a, |s| {
            let mut count = 0;
            for _ in 0..100 {
                let page = s.ids_where(target, "", 1000)?;
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

    /// Server search: Microsoft's own, within the folder.
    pub(super) fn search_graph(&self, a: &Account, mailbox: &Mailbox, query: &str, limit: usize) -> Result<ServerResults> {
        self.with_graph(a, |s| {
            let folders = s.folders()?;
            let folder = match listing(&folders, mailbox)? {
                Listing::Folder(id) => id,
                // Across the mailbox: from the root of its folders.
                Listing::Everywhere(_) => "msgfolderroot".to_owned(),
            };
            let found = s.search(&folder, query.trim(), SEARCH_MOST)?;
            let mut out = ServerResults { total: found.len(), ..Default::default() };
            let pre = Prefetched::new(&self.store, found.iter().take(limit).map(|m| row_id(a, &m.id)).collect())?;
            for m in found.iter().take(limit) {
                out.mails.push(self.graph_mail(a, &folders, m, Some(&pre))?);
            }
            self.store.save_messages(&out.mails)?;
            out.mails.sort_by(Mail::newest_first);
            Ok(out)
        })
    }

    /// Marks everything unread in a mailbox read, listed or not.
    pub(super) fn mark_all_read_graph(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        self.with_graph(a, |s| {
            let folders = s.folders()?;
            if let Listing::Folder(id) = listing(&folders, mailbox)? {
                let unread = s.ids_where(&id, "isRead eq false", 20_000)?;
                if !unread.is_empty() {
                    s.update(&unread, &json!({"isRead": true}))?;
                }
            }
            Ok(())
        })?;
        self.store.mark_mailbox_read(&a.id, mailbox.clone())
    }

    /// Sends a finished message through Microsoft, which keeps it in Sent
    /// Items. Bcc recipients are named in it for Microsoft, which takes
    /// them out.
    /// With `hold_until`, Exchange keeps it and sends it then.
    pub(super) fn send_graph(&self, a: &Account, d: &Draft, raw: &[u8], hold_until: Option<i64>) -> Result<()> {
        let raw = crate::sync::smtp::with_bcc(raw.to_vec(), d)?;
        self.with_graph(a, |s| match hold_until {
            Some(at) => s.send_later(&raw, at),
            None => s.send(&raw),
        })
    }
    /// Saves a copy of a message sent through another server in Sent Items,
    /// as a sent message (see `Session::import`).
    pub(super) fn file_sent_copy_graph(&self, a: &Account, raw: &[u8]) -> Result<()> {
        self.with_graph(a, |s| s.import(raw, well_known::SENT, true, false).map(drop))
    }

    /// Keeps the account's drafts and its Drafts folder in step, as for
    /// Gmail and Proton. Returns how many drafts changed here.
    pub(super) fn sync_drafts_graph(&self, a: &Account) -> Result<usize> {
        let _ = self.file_queued_sent_copies_with(a, |raw| self.file_sent_copy_graph(a, raw));
        self.with_graph(a, |s| {
            let pending = self.draft_removals(&a.id);
            for id in pending.iter().filter_map(graph_draft) {
                match s.delete_message(id) {
                    Ok(()) => {}
                    Err(e) if crate::sync::retry::later(&e) && !not_found(&e) => return Err(e),
                    Err(_) => {}
                }
            }
            self.clear_draft_removals(&a.id, &pending)?;
            const MOST: usize = 20_000;
            let mut on_server = s.draft_ids(MOST + 1)?;
            let complete = on_server.len() <= MOST;
            on_server.truncate(MOST);
            let listed: HashSet<&str> = on_server.iter().map(String::as_str).collect();
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            for d in &mut locals {
                let Some(id) = d.remote.as_ref().and_then(graph_draft) else { continue };
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
                .filter_map(|d| d.remote.as_ref().and_then(graph_draft).map(str::to_owned))
                .collect();
            for id in &on_server {
                if claimed.contains(id) {
                    continue;
                }
                let raw = s.raw(id)?;
                ensure!(raw.len() <= MAX_MESSAGE, t!("manage-gmail-a-draft-at-gmail-is-too-large"));
                let mut d = self.draft_from(a, &uuid::Uuid::new_v4().to_string(), &raw)?;
                d.remote = Some(graph_remote(id));
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
                    if let Some(old) = d.remote.as_ref().and_then(graph_draft) {
                        if let Err(e) = s.delete_message(old)
                            && crate::sync::retry::later(&e)
                        {
                            return Err(e);
                        }
                        self.keep_draft_here(d)?;
                    }
                    continue;
                };
                let id = s.create_mime(&raw, None)?;
                if let Some(old) = d.remote.as_ref().and_then(graph_draft) {
                    let _ = s.delete_message(old);
                }
                if !self.finish_draft_upload(d, graph_remote(&id), fingerprint)? && s.delete_message(&id).is_err() {
                    self.queue_draft_removal(&a.id, graph_remote(&id))?;
                }
            }
            Ok(changed)
        })
    }

    // Folders.

    pub(super) fn create_folder_graph(&self, a: &Account, parent: Option<&str>, leaf: &str) -> Result<FolderChange> {
        self.with_graph(a, |s| {
            let folders = s.folders()?;
            let parent = parent.map(|p| folder_id(&folders, p)).transpose()?;
            s.create_folder(parent.as_deref(), leaf)?;
            let folders = remote_folders(&s.folders()?);
            self.store.save_folders(&a.id, &folders)?;
            Ok(FolderChange { moved: Vec::new(), folders })
        })
    }

    pub(super) fn rename_folder_graph(&self, a: &Account, name: &str, leaf: &str) -> Result<FolderChange> {
        let change = self.with_graph(a, |s| {
            let before = s.folders()?;
            s.rename_folder(&folder_id(&before, name)?, leaf)?;
            let after = s.folders()?;
            Ok(FolderChange { moved: moved_paths(&before, &after), folders: remote_folders(&after) })
        })?;
        self.store.save_folders(&a.id, &change.folders)?;
        for (old, new) in &change.moved {
            self.store.rename_remote(&a.id, old, new)?;
        }
        Ok(change)
    }

    pub(super) fn delete_folder_graph(&self, a: &Account, name: &str) -> Result<Vec<RemoteFolder>> {
        let folders = self.with_graph(a, |s| {
            s.delete_folder(&folder_id(&s.folders()?, name)?)?;
            Ok(remote_folders(&s.folders()?))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        // Microsoft moves the folder to Deleted Items; the next check finds
        // its mail there.
        let rows = self.store.summaries(&a.id, Mailbox::Remote(name.to_owned()))?;
        self.store.commit_changes(&rows, true)?;
        Ok(folders)
    }
}

/// PR_LAST_VERB_EXECUTED set to `verb` (102 replied, 104 forwarded).
fn last_verb(verb: i64) -> serde_json::Value {
    json!({"singleValueExtendedProperties": [{"id": "Integer 0x1081", "value": verb.to_string()}]})
}

/// Folders whose path changed, by id: (old path, new path).
fn moved_paths(before: &[MailFolder], after: &[MailFolder]) -> Vec<(String, String)> {
    own(before)
        .filter_map(|old| {
            let new = after.iter().find(|n| n.id == old.id)?;
            (new.path != old.path).then(|| (old.path.clone(), new.path.clone()))
        })
        .collect()
}

// Follow-up reminders and notes (`follow_up`, `notes`).
impl Backend {
    /// Picacho's folder at Microsoft (made with `create`), by id.
    fn picacho_folder_graph(&self, a: &Account, s: &Session, create: bool) -> Result<Option<String>> {
        let parent = super::snooze::PARENT;
        if let Ok(id) = folder_id(&s.folders()?, parent) {
            return Ok(Some(id));
        }
        if !create {
            return Ok(None);
        }
        let id = s.create_folder(None, parent)?;
        let _ = self.store.save_folders(&a.id, &remote_folders(&s.reload_folders()?));
        Ok(Some(id))
    }
    /// The notes in Picacho's folder, by message id.
    pub(super) fn read_notes_graph(&self, a: &Account, kind: &str) -> Result<super::notes::NoteSources> {
        self.with_graph(a, |s| {
            let Some(folder) = self.picacho_folder_graph(a, s, false)? else {
                return Ok(None);
            };
            let ids = s.ids_where(&folder, "", 50)?;
            let mut out = Vec::new();
            for m in s.messages(&ids)? {
                if super::notes::titled(&m.subject, kind) {
                    out.push((m.id.clone(), s.raw(&m.id)?));
                }
            }
            Ok(Some(out))
        })
    }
    /// Puts a note (read) in Picacho's folder, then deletes `old`.
    pub(super) fn write_note_graph(&self, a: &Account, raw: &[u8], old: &[String]) -> Result<()> {
        self.with_graph(a, |s| {
            let folder = self
                .picacho_folder_graph(a, s, true)?
                .with_context(|| t!("manage-snooze-the-server-didnt-keep-the-new"))?;
            s.import(raw, &folder, true, false)?;
            for id in old {
                match s.delete_message(id) {
                    Ok(()) => {}
                    Err(e) if not_found(&e) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
    }
    /// The messages of a Microsoft conversation, as rows (not saved).
    pub(super) fn graph_conversation(&self, a: &Account, conversation: &str) -> Result<Vec<Mail>> {
        self.with_graph(a, |s| {
            let folders = s.folders()?;
            s.conversation(conversation)?.iter().map(|m| self.graph_mail(a, &folders, m, None)).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, path: &str, well_known: Option<&str>) -> MailFolder {
        MailFolder {
            id: id.into(),
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            depth: path.matches('/').count(),
            well_known: well_known.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn folders_say_where_a_message_is() {
        let folders = vec![
            folder("f-in", "Inbox", Some("inbox")),
            folder("f-sync", "Sync Issues", Some("syncissues")),
            folder("f-con", "Sync Issues/Conflicts", None),
            folder("f-w", "Inbox/Work", None),
        ];
        let m = |id: &str| Message { folder_id: id.into(), ..Default::default() };
        assert_eq!(location(&m("f-in"), &folders), (Folder::Inbox, "Inbox".into()));
        assert_eq!(location(&m("f-w"), &folders), (Folder::Other, "Inbox/Work".into()));
        assert_eq!(location(&m("unknown"), &folders), (Folder::Archive, "Archive".into()));
        // Standard and hidden folders, and those inside hidden ones, aren't listed.
        let listed: Vec<String> = remote_folders(&folders).into_iter().map(|f| f.name).collect();
        assert_eq!(listed, ["Inbox/Work"]);
        assert!(matches!(listing(&folders, &Mailbox::Standard(Folder::Trash)), Ok(Listing::Folder(f)) if f == "deleteditems"));
        assert!(matches!(listing(&folders, &Mailbox::Remote("Inbox/Work".into())), Ok(Listing::Folder(f)) if f == "f-w"));
        assert!(matches!(listing(&folders, &Mailbox::Standard(Folder::Starred)), Ok(Listing::Everywhere(Some(_)))));
    }

    #[test]
    fn rows_and_drafts_know_their_ids() {
        let a = Account::new(Provider::Outlook);
        let m = Mail { id: row_id(&a, "AAMk="), ..Default::default() };
        assert_eq!(graph_id(&a, &m), Some("AAMk="));
        assert_eq!(graph_draft(&graph_remote("x")), Some("x"));
    }
}
