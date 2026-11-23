//! JMAP accounts: what the account operations do when an account talks
//! JMAP (`sync/jmap.rs`) instead of IMAP/SMTP.
//!
//! A JMAP message keeps one id wherever it goes, so it is one row here
//! (`{account}:jmap:{id}`), in the mailbox it is in: the standard folders
//! are found by role (RFC 8457), the rest are the account's own folders,
//! named by their path ("Work/Clients"). Starred is the `$flagged`
//! keyword, and other keywords are the row's labels, as IMAP keywords are.
//! Moving a message changes its mailbox at the server and its row here,
//! rather than removing it and finding it again as IMAP does.
//!
//! Sources download as blobs and are cached as they are, so search,
//! attachments and offline reading work the same as for IMAP. Mail is
//! sent through the server itself (EmailSubmission), which files it in
//! Sent.
use super::*;
use crate::sync::jmap::{self, Email, Session};
use crate::sync::sieve;
use serde_json::{Map, Value, json};

/// The most messages one listing request asks for.
const PAGE: usize = 250;
/// The most messages of one mailbox a server search lists.
const SEARCH_MOST: usize = 500;

/// A mailbox with where it shows here: one of the standard folders, or
/// (`folder` None) one of the account's own, by `path`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Place {
    pub id: String,
    pub path: String,
    pub depth: usize,
    pub folder: Option<Folder>,
    /// Roles of mailboxes that collect others' mail (All Mail, Flagged…):
    /// neither standard folders nor folders of the account's own.
    pub collection: bool,
}

/// The standard folder a mailbox role stands for.
fn standard(role: &str) -> Option<Folder> {
    Some(match role.to_ascii_lowercase().as_str() {
        "inbox" => Folder::Inbox,
        "sent" => Folder::Sent,
        "drafts" => Folder::Drafts,
        "archive" => Folder::Archive,
        "junk" | "spam" => Folder::Junk,
        "trash" => Folder::Trash,
        _ => return None,
    })
}
/// Names servers without roles give their standard folders.
fn standard_by_name(name: &str) -> Option<Folder> {
    Some(match name.to_ascii_lowercase().as_str() {
        "inbox" => Folder::Inbox,
        "sent" | "sent items" | "sent mail" | "sent messages" => Folder::Sent,
        "drafts" => Folder::Drafts,
        "archive" | "archives" => Folder::Archive,
        "junk" | "spam" | "junk mail" | "junk e-mail" => Folder::Junk,
        "trash" | "deleted items" | "deleted messages" | "bin" => Folder::Trash,
        _ => return None,
    })
}

/// The account's mailboxes with their paths and where each shows here.
pub(super) fn places(mailboxes: &[jmap::Mailbox]) -> Vec<Place> {
    let by_id: HashMap<&str, &jmap::Mailbox> =
        mailboxes.iter().map(|m| (m.id.as_str(), m)).collect();
    // The order a person set (sortOrder, RFC 8621 §2), then by name, at
    // each level: a folder's children stay right under it.
    let order_of = |m: &jmap::Mailbox| -> Vec<(u32, String)> {
        let mut key = vec![(m.sort_order, m.name.to_lowercase())];
        let mut parent = m.parent_id.as_deref();
        while let Some(p) = parent.and_then(|p| by_id.get(p)) {
            if key.len() > 32 {
                break;
            }
            key.push((p.sort_order, p.name.to_lowercase()));
            parent = p.parent_id.as_deref();
        }
        key.reverse();
        key
    };
    let orders: HashMap<&str, Vec<(u32, String)>> =
        mailboxes.iter().map(|m| (m.id.as_str(), order_of(m))).collect();
    let path_of = |m: &jmap::Mailbox| -> (String, usize) {
        let mut names = vec![m.name.clone()];
        let mut parent = m.parent_id.as_deref();
        // Bounded, should a server's parents ever loop.
        while let Some(p) = parent.and_then(|p| by_id.get(p)) {
            if names.len() > 32 {
                break;
            }
            names.push(p.name.clone());
            parent = p.parent_id.as_deref();
        }
        names.reverse();
        let depth = names.len() - 1;
        (names.join("/"), depth)
    };
    let mut out: Vec<Place> = mailboxes
        .iter()
        .map(|m| {
            let (path, depth) = path_of(m);
            let role = m.role.as_deref().unwrap_or("");
            Place {
                id: m.id.clone(),
                path,
                depth,
                folder: standard(role),
                collection: matches!(
                    role.to_ascii_lowercase().as_str(),
                    "all" | "flagged" | "important" | "subscribed"
                ),
            }
        })
        .collect();
    // Standard folders without a role, found by name at the top level.
    for f in [
        Folder::Inbox,
        Folder::Sent,
        Folder::Drafts,
        Folder::Archive,
        Folder::Junk,
        Folder::Trash,
    ] {
        if out.iter().any(|p| p.folder == Some(f)) {
            continue;
        }
        if let Some(p) = out.iter_mut().find(|p| {
            p.folder.is_none()
                && !p.collection
                && p.depth == 0
                && standard_by_name(&p.path) == Some(f)
        }) {
            p.folder = Some(f);
        }
    }
    out.sort_by(|a, b| {
        orders
            .get(a.id.as_str())
            .cmp(&orders.get(b.id.as_str()))
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    out
}

/// The account's own folders, as the folder list shows them.
pub(super) fn remote_folders(places: &[Place]) -> Vec<RemoteFolder> {
    places
        .iter()
        .filter(|p| p.folder.is_none() && !p.collection)
        .map(|p| RemoteFolder {
            name: p.path.clone(),
            label: p.path.rsplit('/').next().unwrap_or(&p.path).to_owned(),
            depth: p.depth,
            nestable: true,
        })
        .collect()
}

/// The mailbox id behind a standard folder or one of the account's own.
pub(super) fn mailbox_id(places: &[Place], mailbox: &Mailbox) -> Result<String> {
    match mailbox {
        Mailbox::Standard(folder) => places
            .iter()
            .find(|p| p.folder == Some(*folder))
            .map(|p| p.id.clone())
            .with_context(|| format!("This account has no {} folder.", folder.label())),
        Mailbox::Remote(name) => places
            .iter()
            .find(|p| p.folder.is_none() && &p.path == name)
            .map(|p| p.id.clone())
            .with_context(|| format!("The folder “{name}” no longer exists on the server.")),
    }
}

/// What a mailbox lists, as a JMAP filter, within the sync window.
pub(super) fn filter_for(
    places: &[Place],
    mailbox: &Mailbox,
    days: u32,
    now: i64,
) -> Result<Value> {
    let condition = match mailbox.folder() {
        Folder::Starred => json!({ "hasKeyword": "$flagged" }),
        // Everything but drafts.
        Folder::All => {
            let drafts: Vec<&str> = places
                .iter()
                .filter(|p| p.folder == Some(Folder::Drafts))
                .map(|p| p.id.as_str())
                .collect();
            json!({ "inMailboxOtherThan": drafts })
        }
        _ => json!({ "inMailbox": mailbox_id(places, mailbox)? }),
    };
    Ok(if days > 0 {
        // From the start of a day: the same filter all day long, which
        // Email/queryChanges needs (RFC 8620 §5.6: the query's state is
        // for one filter). It moves on once a day, and the listing is read
        // again then.
        let start = (now - i64::from(days) * 86_400).div_euclid(86_400) * 86_400;
        let after = jmap::utc_date(start);
        json!({ "operator": "AND", "conditions": [condition, { "after": after }] })
    } else {
        condition
    })
}

/// Where a message shows, from its mailboxes: (folder, mailbox path). A
/// message in several is shown where it matters most: the inbox, then
/// Trash, Spam, Drafts, Sent and Archive, then the account's own folders.
pub(super) fn location(e: &Email, places: &[Place]) -> (Folder, String) {
    for f in [
        Folder::Inbox,
        Folder::Trash,
        Folder::Junk,
        Folder::Drafts,
        Folder::Sent,
        Folder::Archive,
    ] {
        if let Some(p) = places
            .iter()
            .find(|p| p.folder == Some(f) && e.in_mailbox(&p.id))
        {
            return (f, p.path.clone());
        }
    }
    if let Some(p) = places
        .iter()
        .find(|p| p.folder.is_none() && !p.collection && e.in_mailbox(&p.id))
    {
        return (Folder::Other, p.path.clone());
    }
    (Folder::Archive, "Archive".into())
}

/// The away reply as the filter settings show it, from JMAP's automatic
/// reply (whose dates are UTC date-times; the settings keep days).
fn vacation_state(reply: &jmap::VacationResponse) -> sieve::State {
    let day = |d: &Option<String>| d.as_deref().and_then(|d| d.get(..10)).map(str::to_owned);
    sieve::State {
        filters: sieve::Filters {
            rules: Vec::new(),
            vacation: sieve::Vacation {
                on: reply.is_enabled,
                subject: reply.subject.clone().unwrap_or_default(),
                body: reply.text_body.clone().unwrap_or_default(),
                days: 7,
                from: day(&reply.from_date),
                until: day(&reply.to_date),
            },
            include: None,
        },
        // What the settings offer for it: the reply, between two dates.
        extensions: vec!["vacation".into(), "date".into(), "relational".into()],
        other_active: None,
        active: reply.is_enabled,
        vacation_only: true,
        ..Default::default()
    }
}
/// JMAP's automatic reply for the away reply in the settings: from the
/// start of its first day until the end of its last, in UTC.
fn vacation_response(v: &sieve::Vacation) -> jmap::VacationResponse {
    let valid = |d: &Option<String>| {
        d.as_deref()
            .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            .map(str::to_owned)
    };
    jmap::VacationResponse {
        is_enabled: v.on,
        from_date: valid(&v.from).map(|d| format!("{d}T00:00:00Z")),
        to_date: valid(&v.until).map(|d| format!("{d}T23:59:59Z")),
        subject: Some(v.subject.clone()).filter(|s| !s.trim().is_empty()),
        text_body: Some(v.body.clone()),
    }
}

fn jmap_remote(id: &str) -> RemoteDraft {
    RemoteDraft {
        mailbox: format!("jmap:{id}"),
        validity: 0,
        uid: 0,
    }
}
/// A JMAP draft, as a `RemoteDraft` (which IMAP fills with its mailbox,
/// UIDVALIDITY and UID): the message's id, after `jmap:`.
fn jmap_draft(r: &RemoteDraft) -> Option<&str> {
    r.mailbox.strip_prefix("jmap:")
}

/// The JMAP message id of one of our rows.
pub(super) fn jmap_id<'a>(a: &Account, m: &'a Mail) -> Option<&'a str> {
    m.id.strip_prefix(&a.id)?.strip_prefix(":jmap:")
}
pub(super) fn row_id(a: &Account, id: &str) -> String {
    format!("{}:jmap:{id}", a.id)
}
fn ids(a: &Account, mails: &[Mail]) -> Vec<String> {
    mails
        .iter()
        .filter_map(|m| jmap_id(a, m).map(str::to_owned))
        .collect()
}
fn bracketed(id: &str) -> String {
    format!("<{}>", id.trim().trim_matches(['<', '>']))
}

/// A mailbox's last synchronization: the Email state it was read in.
#[derive(serde::Serialize, serde::Deserialize)]
struct Synced {
    state: String,
    days: u32,
    /// The mailbox query's state then (Email/queryChanges), where the
    /// server keeps one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    query_state: String,
}

/// The patch that makes a change to one message.
fn patch_for(places: &[Place], change: &Change) -> Result<Map<String, Value>> {
    let mut p = Map::new();
    let mut keyword = |k: &str, on: bool| {
        p.insert(
            format!("keywords/{k}"),
            if on { Value::Bool(true) } else { Value::Null },
        );
    };
    match change {
        Change::Read(read) => keyword("$seen", *read),
        Change::Star(star) => keyword("$flagged", *star),
        Change::Mark(Mark::Answered) => keyword("$answered", true),
        Change::Mark(Mark::Forwarded) => keyword("$forwarded", true),
        Change::Mark(Mark::ReceiptHandled) => keyword("$mdnsent", true),
        Change::Label(name, add) => {
            let k = keyword_of(name)?;
            keyword(&k, *add);
        }
        Change::Move(folder) => {
            ensure!(
                !folder.is_virtual() && *folder != Folder::Other,
                "Mail can't be moved to {}.",
                folder.label()
            );
            let id = mailbox_id(places, &Mailbox::Standard(*folder))?;
            p.insert("mailboxIds".into(), json!({ id: true }));
        }
        Change::MoveTo(name) => {
            let id = mailbox_id(places, &Mailbox::Remote(name.clone()))?;
            p.insert("mailboxIds".into(), json!({ id: true }));
        }
    }
    Ok(p)
}
/// A label as a JMAP keyword: an IMAP atom, which servers keep in lower case.
fn keyword_of(label: &str) -> Result<String> {
    Ok(keyword(label)?.to_ascii_lowercase())
}

impl Backend {
    /// The account's JMAP session, signed in on first use, then kept; with
    /// browser sign-in, until its access token is about to expire, when
    /// the token is renewed (`credentials`) and the session opened again.
    fn jmap_session(&self, a: &Account) -> Result<Arc<Session>> {
        let now = chrono::Utc::now().timestamp();
        // A session the server says changed (`stale`) is read again.
        if let Some((s, expires)) = self.jmap.lock().unwrap().get(&a.id)
            && (*expires == 0 || *expires > now + 120)
            && !s.stale()
        {
            return Ok(s.clone());
        }
        let secret = self.credentials(a)?;
        let session = Arc::new(self.open_jmap(a, &secret)?);
        let expires = if a.oauth { secret.expires_at } else { 0 };
        self.jmap
            .lock()
            .unwrap()
            .insert(a.id.clone(), (session.clone(), expires));
        Ok(session)
    }
    /// Signs in with the account's password, or its access token.
    fn open_jmap(&self, a: &Account, secret: &Secret) -> Result<Session> {
        if a.oauth {
            Session::open_token(
                &a.incoming_host,
                a.incoming_port,
                &secret.access_token,
                &a.jmap_account,
                self.tls_connector()?,
            )
        } else {
            Session::open(
                &a.incoming_host,
                a.incoming_port,
                a.login(),
                &secret.password,
                &a.jmap_account,
                self.tls_connector()?,
            )
        }
    }
    /// The name the server's session gives the account (its address, on
    /// most servers), for a browser sign-in that didn't say.
    pub(super) fn jmap_username(&self, a: &Account, secret: &Secret) -> Result<String> {
        Ok(self.open_jmap(a, secret)?.username().to_owned())
    }
    /// Runs `work` with the account's session; a refused sign-in drops it,
    /// so the next try signs in again.
    pub(super) fn with_jmap<T>(
        &self,
        a: &Account,
        work: impl FnOnce(&Session) -> Result<T>,
    ) -> Result<T> {
        let session = self.jmap_session(a)?;
        let result = work(&session);
        if let Err(e) = &result {
            if e.downcast_ref::<AccessFailure>().is_some() {
                self.jmap.lock().unwrap().remove(&a.id);
            }
            // 429 or 503: background work pauses, as for IMAP.
            self.note_limit(a, e);
        }
        result
    }
    /// The other JMAP accounts with mail that `a`'s sign-in reaches (shared
    /// with it), not added yet: (id at the server, name).
    pub fn jmap_shared(&self, a: &Account) -> Result<Vec<(String, String)>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP accounts are shared this way.");
        let added: Vec<String> = self
            .store
            .accounts()?
            .into_iter()
            .filter(|o| o.credentials_of == a.id)
            .map(|o| o.jmap_account)
            .collect();
        let owner = if a.credentials_of.is_empty() { a.id.as_str() } else { a.credentials_of.as_str() };
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| {
            Ok(s.other_accounts()
                .iter()
                .filter(|(id, _)| !added.contains(id) && owner == a.id)
                .cloned()
                .collect())
        })
    }
    /// Adds the JMAP account `jmap_id` (`name`) shared with `owner`'s
    /// sign-in, as an account of its own that signs in as `owner` does.
    pub fn add_jmap_shared(&self, owner: &Account, jmap_id: &str, name: &str) -> Result<Account> {
        ensure!(
            owner.provider == Provider::Jmap && owner.credentials_of.is_empty(),
            "Only a JMAP account's own sign-in has accounts shared with it."
        );
        ensure!(
            !self
                .store
                .accounts()?
                .iter()
                .any(|o| o.credentials_of == owner.id && o.jmap_account == jmap_id),
            "This shared account is already added."
        );
        let mut shared = owner.clone();
        shared.id = uuid::Uuid::new_v4().to_string();
        shared.jmap_account = jmap_id.to_owned();
        shared.credentials_of = owner.id.clone();
        shared.name = name.to_owned();
        shared.nickname = String::new();
        shared.identities = Vec::new();
        shared.server_send_later = false;
        // It must open before it's added.
        self.with_jmap(&shared, |s| s.reload_mailboxes().map(drop))?;
        self.store.save_account(&shared)?;
        Ok(shared)
    }
    /// Signs in to check the account (adding or reconnecting it).
    pub(super) fn check_jmap(&self, a: &Account, secret: &Secret) -> Result<()> {
        self.open_jmap(a, secret)?.reload_mailboxes()?;
        Ok(())
    }
    pub(super) fn jmap_places(&self, a: &Account, s: &Session, reload: bool) -> Result<Vec<Place>> {
        let mailboxes = if reload {
            s.reload_mailboxes()?
        } else {
            s.mailboxes()?
        };
        let places = places(&mailboxes);
        if reload {
            let _ = self.store.save_folders(&a.id, &remote_folders(&places));
        }
        Ok(places)
    }

    /// A row for a listed message: the cached one with its flags, folder
    /// and labels brought up to date, or a new one from the listing.
    fn jmap_mail(&self, a: &Account, places: &[Place], e: &Email) -> Result<Mail> {
        let id = row_id(a, &e.id);
        let (folder, remote) = location(e, places);
        let first = e.from.as_ref().and_then(|f| f.first());
        let mut mail = match self.store.message(&id)? {
            Some(cached) => cached,
            None => {
                let mut m = Mail {
                    id,
                    account: a.id.clone(),
                    from: first.map(|f| f.email().to_owned()).unwrap_or_default(),
                    sender: first
                        .map(|f| {
                            if f.name().is_empty() {
                                f.email().to_owned()
                            } else {
                                f.name().to_owned()
                            }
                        })
                        .unwrap_or_default(),
                    to: jmap::addresses(&e.to),
                    reply_to: jmap::addresses(&e.reply_to),
                    subject: e.subject.clone().unwrap_or_default(),
                    snippet: e.preview.clone().unwrap_or_default(),
                    date: e.time(),
                    message_id: e
                        .message_id
                        .as_ref()
                        .and_then(|ids| ids.first())
                        .map(|id| bracketed(id))
                        .unwrap_or_default(),
                    has_attachments: e.has_attachment,
                    list_unsubscribe: e.unsubscribe(),
                    one_click: e
                        .list_unsubscribe_post
                        .as_deref()
                        .is_some_and(|p| p.to_ascii_lowercase().contains("one-click")),
                    ..Default::default()
                };
                let references = e.references.as_ref().or(e.in_reply_to.as_ref());
                if let Some(refs) = references {
                    m.set_references(
                        refs.iter()
                            .map(|r| bracketed(r))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                if let Some(to) = e
                    .receipt_to
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    m.set_receipt_to(to);
                }
                m
            }
        };
        if mail.date <= 0 {
            mail.date = e.time();
        }
        mail.set_cc(jmap::addresses(&e.cc));
        if !e.thread_id.is_empty() {
            mail.set_thread_id(format!("jmap:{}", e.thread_id));
        }
        mail.folder = folder;
        mail.remote_folder = remote;
        mail.unread = !e.has_keyword("$seen");
        mail.starred = e.has_keyword("$flagged");
        mail.answered = e.has_keyword("$answered");
        mail.forwarded = e.has_keyword("$forwarded");
        mail.mdn_sent = e.has_keyword("$mdnsent");
        mail.size = u32::try_from(e.size).unwrap_or(u32::MAX);
        let mut labels: Vec<String> = e
            .keywords
            .iter()
            .filter(|(k, on)| **on && label_keyword(k))
            .map(|(k, _)| k.clone())
            .collect();
        labels.sort();
        mail.set_labels(labels);
        Ok(mail)
    }

    fn jmap_state_key(a: &Account, mailbox: &Mailbox) -> String {
        format!("jmap_state:{}:{}", a.id, mailbox.key())
    }

    /// Brings a mailbox up to date: when nothing changed since its last
    /// sync, that is one short request; otherwise its newest `limit`
    /// messages are listed, and cached ones that weren't are asked about
    /// again (moved elsewhere, or deleted).
    pub(super) fn sync_jmap(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        // Drafts are kept on this computer, as with IMAP accounts.
        if mailbox.folder() == Folder::Drafts {
            return Ok(vec![]);
        }
        // Changes made while offline reach the server before anything is read.
        self.replay_offline_jmap(a)?;
        let key = Self::jmap_state_key(a, mailbox);
        self.with_jmap(a, |s| {
            let saved: Option<Synced> = self
                .store
                .preference(&key)
                .and_then(|v| serde_json::from_str(&v).ok())
                .filter(|v: &Synced| v.days == a.sync.days && !v.state.is_empty());
            if let Some(saved) = &saved
                && let Some(changes) = s.changes(&saved.state)?
                && changes.is_empty()
                && !changes.has_more
            {
                // Nothing changed anywhere in the account.
                return self.store.newest(&a.id, mailbox.clone(), limit);
            }
            let places = self.jmap_places(a, s, true)?;
            let filter = filter_for(
                &places,
                mailbox,
                a.sync.days,
                chrono::Utc::now().timestamp(),
            )?;
            // What changed in this mailbox's listing since (Email/
            // queryChanges): those added are read and those removed asked
            // about, rather than the newest listed again.
            if let Some(saved) = saved.as_ref().filter(|s| !s.query_state.is_empty())
                && let Some(changes) = s.query_changes(&filter, &saved.query_state, None)?
                && changes.added.len() + changes.removed.len() <= PAGE
            {
                let added: Vec<String> = changes.added.iter().map(|(id, _)| id.clone()).collect();
                let mut rows = Vec::new();
                for e in &s.emails(&added)? {
                    rows.push(self.jmap_mail(a, &places, e)?);
                }
                // Removed from this mailbox's results: moved elsewhere
                // (asked again, to update the row) or gone.
                if !changes.removed.is_empty() {
                    let current = s.emails(&changes.removed)?;
                    let still: HashSet<String> = current.iter().map(|e| row_id(a, &e.id)).collect();
                    for e in &current {
                        rows.push(self.jmap_mail(a, &places, e)?);
                    }
                    let gone: Vec<String> = changes
                        .removed
                        .iter()
                        .map(|id| row_id(a, id))
                        .filter(|id| !still.contains(id))
                        .collect();
                    if !gone.is_empty() {
                        self.store.delete_ids(&a.id, &gone)?;
                    }
                }
                // Flags of what stayed: Email/changes said which changed.
                if let Some(c) = s.changes(&saved.state)?.filter(|c| !c.has_more) {
                    let listed: HashSet<String> = rows.iter().map(|m| m.id.clone()).collect();
                    let updated: Vec<String> = c
                        .updated
                        .into_iter()
                        .filter(|id| !listed.contains(&row_id(a, id)))
                        .filter(|id| self.store.message(&row_id(a, id)).ok().flatten().is_some())
                        .collect();
                    for e in &s.emails(&updated)? {
                        rows.push(self.jmap_mail(a, &places, e)?);
                    }
                    self.store.save_messages(&rows)?;
                    self.store.set_preference(
                        &key,
                        &serde_json::to_string(&Synced {
                            state: c.new_state,
                            days: a.sync.days,
                            query_state: changes.new_state,
                        })?,
                    )?;
                    return self.store.newest(&a.id, mailbox.clone(), limit);
                }
            }
            let mut listed: Vec<Email> = Vec::new();
            let mut state = String::new();
            let mut query_state = String::new();
            let mut complete = false;
            while listed.len() < limit {
                let want = (limit - listed.len()).min(PAGE);
                let page = s.query(&filter, listed.len(), want, false)?;
                if state.is_empty() {
                    state = page.state.clone();
                    query_state = page.query_state.clone();
                }
                let short = page.emails.len() < want;
                listed.extend(page.emails);
                if short {
                    complete = true;
                    break;
                }
            }
            let mut rows = Vec::with_capacity(listed.len());
            for e in &listed {
                rows.push(self.jmap_mail(a, &places, e)?);
            }
            // What the cache has here that wasn't listed: within the listed
            // range (or anywhere, when the whole mailbox was listed), it has
            // moved or gone.
            let oldest = listed
                .iter()
                .map(Email::time)
                .filter(|&t| t > 0)
                .min()
                .unwrap_or(i64::MIN);
            let listed_ids: HashSet<&str> = rows.iter().map(|m| m.id.as_str()).collect();
            // Only their ids are needed: without bodies, every sync no
            // longer reads (and parses) the whole cached mailbox's text.
            let unlisted: Vec<Mail> = self
                .store
                .summaries(&a.id, mailbox.clone())?
                .into_iter()
                .filter(|m| jmap_id(a, m).is_some() && !listed_ids.contains(m.id.as_str()))
                .filter(|m| complete || m.date >= oldest)
                .collect();
            if !unlisted.is_empty() {
                let current = s.emails(&ids(a, &unlisted))?;
                let still: HashSet<String> = current.iter().map(|e| row_id(a, &e.id)).collect();
                for e in &current {
                    rows.push(self.jmap_mail(a, &places, e)?);
                }
                let gone: Vec<String> = unlisted
                    .iter()
                    .map(|m| m.id.clone())
                    .filter(|id| !still.contains(id))
                    .collect();
                if !gone.is_empty() {
                    self.store.delete_ids(&a.id, &gone)?;
                    let gone: HashSet<&String> = gone.iter().collect();
                    self.readable
                        .lock()
                        .unwrap()
                        .retain(|id, _| !gone.contains(id));
                }
            }
            self.store.save_messages(&rows)?;
            if !state.is_empty() {
                self.store.set_preference(
                    &key,
                    &serde_json::to_string(&Synced {
                        state,
                        days: a.sync.days,
                        query_state,
                    })?,
                )?;
            }
            self.store.newest(&a.id, mailbox.clone(), limit)
        })
    }

    /// Lists `batch` more of a mailbox's messages beyond those cached,
    /// newest first.
    pub(super) fn catalog_jmap(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        batch: usize,
    ) -> Result<Catalog> {
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let filter = filter_for(
                &places,
                mailbox,
                a.sync.days,
                chrono::Utc::now().timestamp(),
            )?;
            let cached = self.store.count_messages(&a.id, mailbox.clone())?;
            let mut position = cached;
            let mut added = Vec::new();
            let mut total = None;
            let mut ended = false;
            // Pages already cached (messages arrived or went since) are
            // passed over, a few at a time, until new ones turn up.
            for _ in 0..8 {
                // A sync or an opened message is waiting: let it go first.
                if !added.is_empty() && self.someone_waiting(&a.id) {
                    break;
                }
                let want = batch.clamp(1, PAGE);
                let page = s.query(&filter, position, want, total.is_none())?;
                total = total.or(page.total);
                let short = page.emails.len() < want;
                position += page.emails.len();
                for e in &page.emails {
                    if self.store.message(&row_id(a, &e.id))?.is_none() {
                        added.push(self.jmap_mail(a, &places, e)?);
                    }
                }
                if short {
                    ended = true;
                    break;
                }
                if added.len() >= batch {
                    break;
                }
            }
            self.store.save_messages(&added)?;
            let cataloged = cached + added.len();
            let total = total.unwrap_or(cataloged);
            let done = ended || added.is_empty();
            Ok(Catalog {
                cataloged: if done {
                    total.max(cataloged)
                } else {
                    cataloged
                },
                total: if done { total.max(cataloged) } else { total },
                added,
                layout: None,
            })
        })
    }

    /// Downloads one message's source and caches it.
    pub(super) fn download_jmap(&self, a: &Account, m: &Mail) -> Result<Vec<u8>> {
        let id = jmap_id(a, m).context("This message isn't from this JMAP account.")?;
        let raw = self.with_jmap(a, |s| s.source(id, large::MAX_DOWNLOAD))?;
        self.note_traffic(a, raw.len());
        self.store.save_raw(&m.id, &raw)?;
        Ok(raw)
    }

    /// A message too large to download whole: its text is read alone, and
    /// each attachment downloads when it is saved (`jmap_attachment`).
    pub(super) fn read_jmap_parts(&self, a: &Account, m: &Mail) -> Result<Mail> {
        let id = jmap_id(a, m).context("This message isn't from this JMAP account.")?;
        let parts = self.with_jmap(a, |s| s.parts(id))?;
        let mut next = m.clone();
        next.body = Some(if parts.text.is_empty() {
            html_to_text(&parts.html)
        } else {
            parts.text
        });
        next.html = (!parts.html.is_empty()).then_some(parts.html);
        next.set_attachments(
            parts
                .attachments
                .into_iter()
                .enumerate()
                .map(|(i, (blob, name, size, mime))| Attachment {
                    name,
                    size,
                    mime,
                    part: 10_000 + i,
                    section: blob,
                    encoding: String::new(),
                })
                .collect(),
        );
        next.has_attachments = !next.attachments().is_empty();
        next.partial = true;
        Ok(next)
    }
    /// A JMAP message's header block alone, for one too large to keep
    /// (where the server reads parts of blobs).
    pub fn jmap_header_block(&self, a: &Account, m: &Mail) -> Result<Option<Vec<u8>>> {
        if a.provider != Provider::Jmap {
            return Ok(None);
        }
        let id = jmap_id(a, m).context("This message isn't from this JMAP account.")?;
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.header_block(id))
    }
    /// One attachment of a message too large to download whole.
    pub(super) fn jmap_attachment(&self, a: &Account, attachment: &Attachment) -> Result<Vec<u8>> {
        self.with_jmap(a, |s| {
            s.download(
                &attachment.section,
                &attachment.name,
                &attachment.mime,
                large::MAX_DOWNLOAD,
            )
        })
    }

    /// Background downloads: up to `max` messages only listed so far.
    pub(super) fn download_batch_jmap(&self, a: &Account, max: usize, since: i64) -> Result<usize> {
        let pending = self.store.missing(&a.id, MAX_MESSAGE, max.min(20), since)?;
        if pending.is_empty() {
            return Ok(0);
        }
        let blobs = self.with_jmap(a, |s| s.blob_ids(&ids(a, &pending)))?;
        let mut stored = 0;
        for m in pending {
            // Someone is opening a message: let them go first.
            if stored > 0 && self.someone_waiting(&a.id) {
                break;
            }
            let Some(blob) = jmap_id(a, &m).and_then(|id| blobs.get(id)) else {
                continue;
            };
            let raw = self.with_jmap(a, |s| {
                s.download(blob, "message.eml", "message/rfc822", MAX_MESSAGE)
            })?;
            self.note_traffic(a, raw.len());
            self.store.save_raw(&m.id, &raw)?;
            if let Ok(mut parsed) = parse_message(&raw, a, &m.id, m.folder, &m.remote_folder, m.uid)
            {
                let mut next = m.clone();
                next.has_attachments = !parsed.attachments().is_empty();
                next.set_attachments(parsed.take_attachments());
                if next.snippet.is_empty() {
                    next.snippet = std::mem::take(&mut parsed.snippet);
                }
                if next.references().is_empty() {
                    next.set_references(parsed.references());
                }
                self.store.save_messages(&[next])?;
            }
            stored += 1;
        }
        Ok(stored)
    }

    /// Makes a change at the server: one Email/set for all the messages.
    pub(super) fn change_jmap(&self, a: &Account, current: &[Mail], change: &Change) -> Result<()> {
        let ids = ids(a, current);
        if ids.is_empty() {
            return Ok(());
        }
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let patch = Value::Object(patch_for(&places, change)?);
            let patches: Map<String, Value> =
                ids.into_iter().map(|id| (id, patch.clone())).collect();
            s.update(patches)
        })
    }
    /// Sends queued changes to the server, oldest first, as
    /// `replay_offline` does for IMAP. Callers hold the lock.
    pub(super) fn replay_offline_jmap(&self, a: &Account) -> Result<()> {
        self.replay_offline_with(a, |mails, change| self.change_jmap(a, mails, change))
    }

    /// Deletes messages for good at the server.
    pub(super) fn delete_jmap(&self, a: &Account, mails: &[Mail]) -> Result<()> {
        let ids = ids(a, mails);
        if !ids.is_empty() {
            self.with_jmap(a, |s| s.destroy(&ids))?;
        }
        self.store.commit_changes(mails, true)
    }

    /// Deletes everything in Trash or Spam for good.
    pub(super) fn empty_jmap(&self, a: &Account, folder: Folder) -> Result<usize> {
        let mailbox = Mailbox::Standard(folder);
        let count = self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let filter = json!({ "inMailbox": mailbox_id(&places, &mailbox)? });
            let mut count = 0;
            // Bounded: a mailbox refilling as it empties can't keep this going.
            for _ in 0..1000 {
                let page = s.query_ids(&filter, 0, PAGE)?;
                if page.is_empty() {
                    break;
                }
                s.destroy(&page)?;
                count += page.len();
            }
            Ok(count)
        })?;
        // Deleted by id: their bodies needn't be read.
        let cached = self.store.summaries(&a.id, mailbox)?;
        self.store.commit_changes(&cached, true)?;
        Ok(count)
    }

    /// Server search: the server matches the whole mailbox, contents too.
    pub(super) fn search_jmap(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        query: &str,
        limit: usize,
    ) -> Result<ServerResults> {
        let search = crate::manage::search::Search::parse(query);
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let mut conditions = vec![filter_for(&places, mailbox, 0, 0)?];
            let searched = search.jmap_conditions();
            let text = !searched.is_empty();
            conditions.extend(searched);
            let filter = json!({ "operator": "AND", "conditions": conditions });
            let page = s.query(&filter, 0, limit.clamp(1, SEARCH_MOST), true)?;
            let mut out = ServerResults::default();
            for e in &page.emails {
                out.mails.push(self.jmap_mail(a, &places, e)?);
            }
            out.total = page.total.unwrap_or(out.mails.len());
            self.store.save_messages(&out.mails)?;
            // Each result shows the part of it that matched (SearchSnippet),
            // not just its beginning; kept out of the cache, which keeps
            // the message's own preview. Without them, the preview stays.
            if text {
                let ids: Vec<String> = page.emails.iter().map(|e| e.id.clone()).collect();
                if let Ok(snippets) = s.search_snippets(&filter, &ids) {
                    for (m, e) in out.mails.iter_mut().zip(&page.emails) {
                        if let Some(preview) = snippets.get(&e.id).filter(|p| !p.trim().is_empty()) {
                            m.snippet = preview.clone();
                        }
                    }
                }
            }
            out.mails.sort_by(Mail::newest_first);
            Ok(out)
        })
    }

    /// Marks everything unread in a mailbox read at the server, listed or not.
    pub(super) fn mark_all_read_jmap(&self, a: &Account, mailbox: &Mailbox) -> Result<()> {
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let filter = json!({
                "operator": "AND",
                "conditions": [filter_for(&places, mailbox, 0, 0)?, { "notKeyword": "$seen" }],
            });
            // Each round marks some read, so they leave the query.
            for _ in 0..1000 {
                let unread = s.query_ids(&filter, 0, PAGE)?;
                if unread.is_empty() {
                    break;
                }
                let patches: Map<String, Value> = unread
                    .into_iter()
                    .map(|id| (id, json!({ "keywords/$seen": true })))
                    .collect();
                s.update(patches)?;
            }
            Ok(())
        })?;
        self.store.mark_mailbox_read(&a.id, mailbox.clone())
    }

    pub(super) fn quota_jmap(&self, a: &Account) -> Result<Option<Quota>> {
        self.with_jmap(a, |s| {
            Ok(s.quota()?
                .filter(|(_, max)| *max > 0)
                .map(|(used, max)| Quota {
                    used_kib: used / 1024,
                    limit_kib: max / 1024,
                }))
        })
    }

    /// Sends a finished message through the server, as the identity that
    /// sends as `from`; the server files it in Sent.
    pub(super) fn send_jmap(
        &self,
        a: &Account,
        from: &str,
        to: &[String],
        raw: &[u8],
        options: &jmap::SendOptions,
    ) -> Result<jmap::Submitted> {
        ensure!(!to.is_empty(), "Add at least one recipient.");
        self.with_jmap(a, |s| {
            let identity = s
                .identities()?
                .into_iter()
                .find(|i| i.covers(from))
                .with_context(|| {
                    format!("The server has no sending identity for {from}. Add it in your mail service's settings.")
                })?;
            let places = self.jmap_places(a, s, false)?;
            let id = |f: Folder| mailbox_id(&places, &Mailbox::Standard(f)).ok();
            s.send(
                &identity.id,
                from,
                to,
                raw,
                id(Folder::Drafts).as_deref(),
                id(Folder::Sent).as_deref(),
                options,
            )
        })
    }

    /// Messages the JMAP server holds to send later (handed over with
    /// HOLDUNTIL) or is still trying: (submission id, subject, when).
    pub fn held_submissions(&self, a: &Account) -> Result<Vec<HeldSubmission>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers hold messages this way.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| {
            let ids = s.pending_submissions()?;
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            // RFC 8620 §5.1: at most `maxObjectsInGet` ids in one /get.
            let mut list = Vec::new();
            for chunk in ids.chunks(s.max_get()) {
                let v = s.call_one(
                    "EmailSubmission/get",
                    json!({ "accountId": s.account(), "ids": chunk, "properties": ["id", "emailId", "sendAt"] }),
                )?;
                list.extend(v["list"].as_array().cloned().unwrap_or_default());
            }
            let emails: Vec<String> = list
                .iter()
                .filter_map(|e| e["emailId"].as_str().map(str::to_owned))
                .collect();
            let subjects: std::collections::HashMap<String, String> = s
                .emails(&emails)?
                .into_iter()
                .map(|e| (e.id.clone(), e.subject.clone().unwrap_or_default()))
                .collect();
            Ok(list
                .iter()
                .filter_map(|e| {
                    Some(HeldSubmission {
                        id: e["id"].as_str()?.to_owned(),
                        subject: e["emailId"]
                            .as_str()
                            .and_then(|i| subjects.get(i).cloned())
                            .unwrap_or_default(),
                        send_at: e["sendAt"]
                            .as_str()
                            .and_then(crate::sync::jmap::timestamp)
                            .unwrap_or(0),
                    })
                })
                .collect())
        })
    }
    /// Takes back a message the JMAP server holds to send later: it isn't
    /// sent, and goes back to Drafts. False if it had already gone.
    pub fn cancel_held(&self, a: &Account, submission: &str) -> Result<bool> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers hold messages this way.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let taken = self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let drafts = mailbox_id(&places, &Mailbox::Standard(Folder::Drafts)).ok();
            s.cancel_submission(submission, drafts.as_deref())
        })?;
        drop(_operation);
        if taken {
            let _ = self.sync_drafts(a);
        }
        Ok(taken)
    }
    /// Remembers which submission sent the message with `message_id`.
    pub(super) fn note_submission(&self, a: &Account, message_id: &str, submission: &str) {
        if message_id.is_empty() || submission.is_empty() {
            return;
        }
        let key = format!("jmap_submissions:{}", a.id);
        let mut all: Vec<(String, String)> = self
            .store
            .preference(&key)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        all.push((message_id.to_owned(), submission.to_owned()));
        // The last few hundred: servers forget submissions after a while too.
        let excess = all.len().saturating_sub(500);
        all.drain(..excess);
        if let Ok(json) = serde_json::to_string(&all) {
            let _ = self.store.set_preference(&key, &json);
        }
    }
    /// Which conversations changed on a JMAP server since `since`
    /// (Thread/changes): ids changed and gone, and the new state; `None`
    /// where the server can't say. With an empty `since`, the current
    /// state alone.
    pub fn jmap_thread_changes(
        &self,
        a: &Account,
        since: &str,
    ) -> Result<Option<crate::sync::jmap::ThreadChanges>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep threads.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| {
            if since.is_empty() {
                let v = s.call_one("Thread/get", json!({ "accountId": s.account(), "ids": [] }))?;
                return Ok(Some((Vec::new(), Vec::new(), v["state"].as_str().unwrap_or_default().to_owned())));
            }
            s.thread_changes(since)
        })
    }
    /// The ids of messages sent from here whose delivery changed since
    /// `since` (EmailSubmission/changes), mapped to their Message-IDs, and
    /// the new state.
    pub fn jmap_delivery_changes(&self, a: &Account, since: &str) -> Result<Option<(Vec<String>, String)>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers report this.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        // Without a state yet: the current one, to ask from.
        if since.is_empty() {
            let state = self.with_jmap(a, |s| {
                let v = s.call_one(
                    "EmailSubmission/get",
                    json!({ "accountId": s.account(), "ids": [] }),
                )?;
                Ok(v["state"].as_str().unwrap_or_default().to_owned())
            })?;
            return Ok(Some((Vec::new(), state)));
        }
        let Some((changed, state)) = self.with_jmap(a, |s| s.submission_changes(since))? else {
            return Ok(None);
        };
        let all: Vec<(String, String)> = self
            .store
            .preference(&format!("jmap_submissions:{}", a.id))
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let ids = all
            .into_iter()
            .filter(|(_, sub)| changed.contains(sub))
            .map(|(id, _)| id)
            .collect();
        Ok(Some((ids, state)))
    }
    /// What became of a sent message, by its Message-ID, where it went over
    /// JMAP from here.
    pub fn delivery_of(&self, a: &Account, m: &Mail) -> Result<Option<crate::sync::jmap::SubmissionStatus>> {
        if a.provider != Provider::Jmap || m.message_id.is_empty() {
            return Ok(None);
        }
        let all: Vec<(String, String)> = self
            .store
            .preference(&format!("jmap_submissions:{}", a.id))
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let bare = |id: &str| id.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_lowercase();
        let wanted = bare(&m.message_id);
        let Some((_, submission)) = all.iter().rev().find(|(id, _)| bare(id) == wanted) else {
            return Ok(None);
        };
        self.submission_status(a, submission)
    }
    /// What became of a message sent over JMAP: each recipient's delivery
    /// as the server knows it.
    pub fn submission_status(
        &self,
        a: &Account,
        submission: &str,
    ) -> Result<Option<crate::sync::jmap::SubmissionStatus>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers report this.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.submission_status(submission))
    }
    /// The account's sending identities on the JMAP server.
    pub fn jmap_identities(&self, a: &Account) -> Result<Vec<crate::sync::jmap::Identity>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep identities.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.identities())
    }
    /// Changes a sending identity on the JMAP server (name, Reply-To,
    /// signatures).
    pub fn update_jmap_identity(
        &self,
        a: &Account,
        id: &str,
        change: &crate::sync::jmap::IdentityChange,
    ) -> Result<()> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep identities.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.update_identity(id, change))
    }
    /// Adds a sending identity on the JMAP server.
    pub fn create_jmap_identity(&self, a: &Account, email: &str, name: &str) -> Result<String> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep identities.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.create_identity(email, name))
    }
    /// Removes a sending identity from the JMAP server.
    pub fn destroy_jmap_identity(&self, a: &Account, id: &str) -> Result<()> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep identities.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| s.destroy_identity(id))
    }
    /// The whole conversation `m` belongs to on a JMAP server (Thread/get),
    /// across mailboxes: messages not listed here yet are fetched (listing
    /// only) and kept, and all of them returned oldest first.
    pub fn jmap_conversation(&self, a: &Account, m: &Mail) -> Result<Vec<Mail>> {
        ensure!(a.provider == Provider::Jmap, "Only JMAP servers keep threads.");
        let thread = m
            .thread_id()
            .strip_prefix("jmap:")
            .context("This message has no conversation on the server.")?
            .to_owned();
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, |s| {
            let ids = s.threads(std::slice::from_ref(&thread))?.remove(&thread).unwrap_or_default();
            let mut out = Vec::new();
            let mut missing = Vec::new();
            for id in &ids {
                match self.store.message(&row_id(a, id))? {
                    Some(m) => out.push(m),
                    None => missing.push(id.clone()),
                }
            }
            if !missing.is_empty() {
                let places = self.jmap_places(a, s, false)?;
                let rows: Vec<Mail> = s
                    .emails(&missing)?
                    .iter()
                    .map(|e| self.jmap_mail(a, &places, e))
                    .collect::<Result<_>>()?;
                self.store.save_messages(&rows)?;
                out.extend(rows);
            }
            out.sort_by_key(|m| m.date);
            Ok(out)
        })
    }
    /// Copies messages from another JMAP account this sign-in reaches
    /// (shared with it, and added here as `from`) into `a`'s `mailbox`, on
    /// the server (Email/copy): nothing is downloaded and uploaded again.
    pub fn copy_between_jmap(
        &self,
        from: &Account,
        mails: &[Mail],
        a: &Account,
        mailbox: &Mailbox,
    ) -> Result<usize> {
        ensure!(
            from.provider == Provider::Jmap && a.provider == Provider::Jmap,
            "Only JMAP accounts copy between themselves on the server."
        );
        let same_sign_in = |x: &Account| if x.credentials_of.is_empty() { x.id.clone() } else { x.credentials_of.clone() };
        ensure!(
            same_sign_in(from) == same_sign_in(a),
            "These accounts don't share a sign-in, so the server can't copy between them."
        );
        let ids: Vec<String> = mails
            .iter()
            .filter_map(|m| jmap_id(from, m).map(str::to_owned))
            .collect();
        // Where the sources' contents are, in case the copy goes by blob:
        // asked under `from`'s lock alone, before `a`'s is taken. Holding one
        // while waiting for the other let two copies the opposite ways wait
        // for each other for good (and one into the same account, itself).
        let from_blobs: HashMap<String, String> = {
            let ids = ids.clone();
            let flock = self.account_lock(&from.id);
            let _from = flock.lock().unwrap();
            self.with_jmap(from, |s| s.blob_ids(&ids)).unwrap_or_default()
        };
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let from_account = from.jmap_account.clone();
        ensure!(!from_account.is_empty() || from.credentials_of.is_empty(), "The account to copy from isn't known.");
        let copied = self.with_jmap(a, |s| {
            let from_account = if from_account.is_empty() {
                // The sign-in's own account.
                s.primary_mail_account().to_owned()
            } else {
                from_account.clone()
            };
            let places = self.jmap_places(a, s, false)?;
            let target = mailbox_id(&places, mailbox)?;
            match s.copy_from(&from_account, &ids, &target, &[]) {
                Ok(copied) => Ok(copied),
                // A server without Email/copy: the sources' blobs copied
                // across (Blob/copy), then imported as messages.
                Err(e) if e.downcast_ref::<crate::sync::jmap::MethodError>().is_some() => {
                    let blobs: Vec<String> = from_blobs.values().cloned().collect();
                    let copied = s.copy_blobs(&from_account, &blobs)?;
                    let mut out = HashMap::new();
                    for (id, blob) in &from_blobs {
                        let Some(new_blob) = copied.get(blob) else { continue };
                        let new = s.import_blob(new_blob, &[&target], &["$seen"])?;
                        out.insert(id.clone(), new);
                    }
                    Ok(out)
                }
                Err(e) => Err(e),
            }
        })?;
        Ok(copied.len())
    }
    /// A short summary of an attached message (`message/rfc822` or
    /// `.eml`): who wrote it, to whom, when, its subject and the start of
    /// its text. On a JMAP server with the message not downloaded, the
    /// server reads it (Email/parse); otherwise it is read here.
    pub fn preview_attached(&self, a: &Account, m: &Mail, part: usize) -> Result<String> {
        let attachment = m
            .attachments()
            .iter()
            .find(|x| x.part == part)
            .cloned()
            .context("Attachment not found")?;
        if a.provider == Provider::Jmap
            && !attachment.section.is_empty()
            && self.store.raw(&m.id)?.is_none()
        {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            if let Some(e) = self.with_jmap(a, |s| s.parse_blob(&attachment.section))? {
                return Ok(format!(
                    "From: {}\nTo: {}\nDate: {}\nSubject: {}\n\n{}",
                    crate::sync::jmap::addresses(&e.from),
                    crate::sync::jmap::addresses(&e.to),
                    e.sent_at.clone().unwrap_or_default(),
                    e.subject.clone().unwrap_or_default(),
                    e.preview.clone().unwrap_or_default()
                ));
            }
        }
        let bytes = self.attachment(a, m, part)?;
        let parsed = parse_message(&bytes, a, "preview", Folder::Other, "", 0)?;
        Ok(format!(
            "From: {}\nTo: {}\nSubject: {}\n\n{}",
            parsed.from,
            parsed.to,
            parsed.subject,
            parsed.body.unwrap_or_default().chars().take(600).collect::<String>()
        ))
    }

    /// Saves a copy of a message sent through another server in Sent (an
    /// identity with its own outgoing server), unless one is there already.
    pub(super) fn file_sent_copy_jmap(&self, a: &Account, raw: &[u8]) -> Result<()> {
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let sent = mailbox_id(&places, &Mailbox::Standard(Folder::Sent))?;
            let id = message_id_of(raw);
            if !id.is_empty() {
                let filter = json!({
                    "operator": "AND",
                    "conditions": [{ "inMailbox": sent }, { "header": ["Message-ID", id] }],
                });
                if !s.query_ids(&filter, 0, 1)?.is_empty() {
                    return Ok(());
                }
            }
            s.import(raw, &[&sent], &["$seen"]).map(|_| ())
        })
    }
    /// Files Sent copies that couldn't be saved when they were sent.
    fn file_queued_sent_copies_jmap(&self, a: &Account) -> Result<usize> {
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
            match self.file_sent_copy_jmap(a, &raw) {
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

    /// Keeps the account's drafts and the server's Drafts in step: drafts
    /// written here are uploaded (replacing their earlier copy), drafts
    /// written elsewhere appear here, and drafts sent or deleted on either
    /// side go from the other. A draft changed here since its last upload
    /// is never thrown away. Returns how many drafts changed here.
    pub(super) fn sync_drafts_jmap(&self, a: &Account) -> Result<usize> {
        // Copies for Sent that couldn't be saved when sent go first; a
        // failure leaves them for the next check without holding up drafts.
        let _ = self.file_queued_sent_copies_jmap(a);
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let Ok(drafts) = mailbox_id(&places, &Mailbox::Standard(Folder::Drafts)) else {
                // No Drafts on the server: drafts stay here.
                return Ok(0);
            };
            // Drafts sent or deleted here: their copies on the server go.
            let pending = self.draft_removals(&a.id);
            let gone: Vec<String> = pending
                .iter()
                .filter_map(jmap_draft)
                .map(str::to_owned)
                .collect();
            if !gone.is_empty()
                && let Err(e) = s.destroy(&gone)
                && crate::sync::retry::later(&e)
            {
                return Err(e);
            }
            self.clear_draft_removals(&a.id, &pending)?;
            // Every draft on the server, a page at a time. Only a complete
            // listing shows that a draft missing from it is gone.
            const PAGE: usize = 500;
            const MOST: usize = 20_000;
            let mut on_server: Vec<String> = Vec::new();
            let complete = loop {
                let page = s.query_ids(&json!({ "inMailbox": drafts }), on_server.len(), PAGE)?;
                if page.is_empty() {
                    break true;
                }
                on_server.extend(page);
                if on_server.len() >= MOST {
                    break false;
                }
            };
            let listed: std::collections::HashSet<&str> = on_server.iter().map(String::as_str).collect();
            let mut changed = 0;
            let mut locals = self.store.drafts(&a.id)?;
            // Copies gone from the server (sent or deleted elsewhere):
            // unchanged drafts go here too; changed ones upload again.
            for d in &mut locals {
                let Some(id) = d.remote.as_ref().and_then(jmap_draft) else {
                    continue;
                };
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
            // Drafts written elsewhere.
            let claimed: std::collections::HashSet<String> = locals
                .iter()
                .filter_map(|d| d.remote.as_ref().and_then(jmap_draft).map(str::to_owned))
                .collect();
            for id in &on_server {
                if claimed.contains(id) {
                    continue;
                }
                let raw = s.source(id, MAX_MESSAGE)?;
                let mut d = self.draft_from(a, &uuid::Uuid::new_v4().to_string(), &raw)?;
                d.remote = Some(jmap_remote(id));
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
                // Mail to be encrypted goes up encrypted, or not at all.
                let Some(raw) = self.draft_upload(a, d)? else {
                    // Its earlier copy (perhaps readable) is no longer it.
                    if let Some(old) = d.remote.as_ref().and_then(jmap_draft) {
                        if let Err(e) = s.destroy(&[old.to_owned()])
                            && crate::sync::retry::later(&e)
                        {
                            return Err(e);
                        }
                        self.keep_draft_here(d)?;
                    }
                    continue;
                };
                let id = s.import(&raw, &[&drafts], &["$draft", "$seen"])?;
                if let Some(old) = d.remote.as_ref().and_then(jmap_draft)
                    && old != id
                {
                    let _ = s.destroy(&[old.to_owned()]);
                }
                if !self.finish_draft_upload(d, jmap_remote(&id), fingerprint)?
                    && s.destroy(std::slice::from_ref(&id)).is_err()
                {
                    // Discarded while uploading: its new copy goes too.
                    self.queue_draft_removal(&a.id, jmap_remote(&id))?;
                }
            }
            Ok(changed)
        })
    }

    // Folders.

    pub(super) fn create_folder_jmap(
        &self,
        a: &Account,
        parent: Option<&str>,
        leaf: &str,
    ) -> Result<FolderChange> {
        ensure!(
            !leaf.trim().is_empty() && !leaf.contains('/'),
            "Enter a folder name without “/”."
        );
        self.with_jmap(a, |s| {
            let places = self.jmap_places(a, s, false)?;
            let parent = match parent {
                Some(p) => Some(mailbox_id(&places, &Mailbox::Remote(p.to_owned()))?),
                None => None,
            };
            s.create_mailbox(leaf.trim(), parent.as_deref())?;
            let folders = remote_folders(&self.jmap_places(a, s, true)?);
            Ok(FolderChange {
                moved: Vec::new(),
                folders,
            })
        })
    }

    pub(super) fn rename_folder_jmap(
        &self,
        a: &Account,
        name: &str,
        leaf: &str,
    ) -> Result<FolderChange> {
        ensure!(
            !leaf.trim().is_empty() && !leaf.contains('/'),
            "Enter a folder name without “/”."
        );
        let change = self.with_jmap(a, |s| {
            let before = self.jmap_places(a, s, false)?;
            let id = mailbox_id(&before, &Mailbox::Remote(name.to_owned()))?;
            s.rename_mailbox(&id, leaf.trim())?;
            let after = self.jmap_places(a, s, true)?;
            // The folder and those inside it, by id: (old path, new path).
            let moved: Vec<(String, String)> = before
                .iter()
                .filter(|p| p.folder.is_none())
                .filter_map(|old| {
                    let new = after.iter().find(|n| n.id == old.id)?;
                    (new.path != old.path).then(|| (old.path.clone(), new.path.clone()))
                })
                .collect();
            Ok(FolderChange {
                moved,
                folders: remote_folders(&after),
            })
        })?;
        // Rows keep their ids; only the folder they show in is renamed.
        for (old, new) in &change.moved {
            self.store.rename_remote(&a.id, old, new)?;
        }
        Ok(change)
    }

    pub(super) fn delete_folder_jmap(&self, a: &Account, name: &str) -> Result<Vec<RemoteFolder>> {
        let folders = self.with_jmap(a, |s| {
            let id = mailbox_id(
                &self.jmap_places(a, s, false)?,
                &Mailbox::Remote(name.to_owned()),
            )?;
            s.destroy_mailbox(&id)?;
            Ok(remote_folders(&self.jmap_places(a, s, true)?))
        })?;
        self.store.save_folders(&a.id, &folders)?;
        self.store.forget_mailbox(&a.id, name)?;
        Ok(folders)
    }

    /// The account's server filters and vacation reply, over JMAP for Sieve
    /// Scripts (RFC 9661): the same script as through ManageSieve.
    pub(super) fn filters_jmap(&self, a: &Account) -> Result<sieve::State> {
        self.with_jmap(a, |s| {
            // Without Sieve, the automatic reply while away alone.
            if s.sieve_extensions().is_none() && s.has_vacation() {
                let reply = s.vacation()?.unwrap_or_default();
                return Ok(vacation_state(&reply));
            }
            let scripts = s.sieve_scripts()?;
            let listed: Vec<(String, bool)> = scripts
                .iter()
                .map(|x| (x.name.clone().unwrap_or_default(), x.is_active))
                .collect();
            let own = match scripts
                .iter()
                .find(|x| x.name.as_deref() == Some(sieve::SCRIPT))
            {
                Some(x) => Some(s.sieve_text(x)?),
                None => None,
            };
            let extensions = s.sieve_extensions().unwrap_or_default().to_vec();
            let mut state = sieve::loaded(extensions, &listed, own.as_deref());
            // The other app's active script, to offer its rules.
            state.other_rules = scripts
                .iter()
                .find(|x| x.is_active && x.name.as_deref() != Some(sieve::SCRIPT))
                .and_then(|x| s.sieve_text(x).ok())
                .and_then(|text| crate::sync::sieve_parse::read(&text).ok());
            Ok(state)
        })
    }
    /// Saves Picacho's script and makes it the active one; without rules
    /// or a vacation reply it is turned off instead (back to the script it
    /// ran first, if any).
    pub(super) fn save_filters_jmap(
        &self,
        a: &Account,
        filters: &sieve::Filters,
        addresses: &[String],
    ) -> Result<sieve::State> {
        self.with_jmap(a, |s| {
            if s.sieve_extensions().is_none() && s.has_vacation() {
                ensure!(
                    filters.rules.is_empty(),
                    "This JMAP server has no server filters, only an away reply."
                );
                let reply = vacation_response(&filters.vacation);
                s.set_vacation(&reply)?;
                return Ok(vacation_state(&reply));
            }
            let extensions = s.sieve_extensions().unwrap_or_default().to_vec();
            ensure!(
                s.sieve_extensions().is_some(),
                "This JMAP server doesn't offer server filters (JMAP for Sieve Scripts)."
            );
            let text = sieve::script(filters, &extensions, addresses)?;
            let scripts = s.sieve_scripts()?;
            let own = scripts
                .iter()
                .find(|x| x.name.as_deref() == Some(sieve::SCRIPT));
            let empty = filters.rules.is_empty() && !filters.vacation.on;
            if empty {
                if own.is_some_and(|x| x.is_active) {
                    let back = filters.include.as_deref().and_then(|name| {
                        scripts
                            .iter()
                            .find(|x| x.name.as_deref() == Some(name))
                            .map(|x| x.id.as_str())
                    });
                    s.sieve_activate(back)?;
                }
            } else {
                let blob = s.sieve_upload(&text)?;
                s.sieve_validate(&blob)?;
                s.sieve_put(sieve::SCRIPT, &blob, own.map(|x| x.id.as_str()))?;
            }
            let other = scripts
                .iter()
                .find(|x| x.is_active && x.name.as_deref() != Some(sieve::SCRIPT))
                .and_then(|x| x.name.clone())
                .filter(|_| empty);
            Ok(sieve::State {
                filters: filters.clone(),
                extensions,
                other_active: other,
                active: !empty,
                vacation_only: false,
                ..Default::default()
            })
        })
    }

    /// The mailboxes changes since `since` touched, for the watcher, and
    /// the state to ask from next. `None` when the server can't say.
    pub(super) fn jmap_changed_mailboxes(
        &self,
        a: &Account,
        s: &Session,
        since: &str,
    ) -> Result<(Vec<Mailbox>, String)> {
        let Some(changes) = s.changes(since)? else {
            return Ok((vec![Mailbox::Standard(Folder::Inbox)], s.state()?));
        };
        let mut boxes: Vec<Mailbox> = Vec::new();
        let mut add = |b: Mailbox| {
            if !boxes.contains(&b) {
                boxes.push(b);
            }
        };
        let touched: Vec<String> = changes
            .created
            .iter()
            .chain(&changes.updated)
            .take(200)
            .cloned()
            .collect();
        if !touched.is_empty() {
            let places = places(&s.reload_mailboxes()?);
            for e in s.emails(&touched)? {
                let (folder, path) = location(&e, &places);
                add(if folder == Folder::Other {
                    Mailbox::Remote(path)
                } else {
                    Mailbox::Standard(folder)
                });
                if e.has_keyword("$flagged") {
                    add(Mailbox::Standard(Folder::Starred));
                }
            }
        }
        // Where they were before: deleted or moved messages.
        for id in changes.updated.iter().chain(&changes.destroyed) {
            if let Ok(Some(old)) = self.store.message(&row_id(a, id)) {
                add(old.mailbox());
            }
        }
        if changes.has_more {
            add(Mailbox::Standard(Folder::Inbox));
        }
        Ok((boxes, changes.new_state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox(id: &str, name: &str, parent: Option<&str>, role: Option<&str>) -> jmap::Mailbox {
        jmap::Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(Into::into),
            role: role.map(Into::into),
            ..Default::default()
        }
    }
    fn sample() -> Vec<Place> {
        places(&[
            mailbox("i", "Inbox", None, Some("inbox")),
            mailbox("s", "Sent Items", None, None),
            mailbox("t", "Bin", None, Some("trash")),
            mailbox("w", "Work", None, None),
            mailbox("c", "Clients", Some("w"), None),
            mailbox("a", "All Mail", None, Some("all")),
        ])
    }
    fn email(mailboxes: &[&str]) -> Email {
        Email {
            id: "M1".into(),
            mailbox_ids: mailboxes.iter().map(|m| ((*m).to_owned(), true)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn places_find_standard_folders_and_paths() {
        let p = sample();
        assert_eq!(mailbox_id(&p, &Folder::Inbox.into()).unwrap(), "i");
        // Found by name where the server gives no role.
        assert_eq!(mailbox_id(&p, &Folder::Sent.into()).unwrap(), "s");
        assert_eq!(mailbox_id(&p, &Folder::Trash.into()).unwrap(), "t");
        assert_eq!(
            mailbox_id(&p, &Mailbox::Remote("Work/Clients".into())).unwrap(),
            "c"
        );
        assert!(mailbox_id(&p, &Folder::Archive.into()).is_err());
        let folders = remote_folders(&p);
        let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        // Collections like All Mail aren't folders of the account's own.
        assert_eq!(names, ["Work", "Work/Clients"]);
        assert_eq!(folders[1].label, "Clients");
        assert_eq!(folders[1].depth, 1);
    }

    #[test]
    fn location_prefers_where_the_message_matters() {
        let p = sample();
        assert_eq!(location(&email(&["a", "i", "c"]), &p).0, Folder::Inbox);
        assert_eq!(location(&email(&["t", "s"]), &p).0, Folder::Trash);
        assert_eq!(
            location(&email(&["a", "c"]), &p),
            (Folder::Other, "Work/Clients".to_owned())
        );
        assert_eq!(location(&email(&["a"]), &p).0, Folder::Archive);
    }

    #[test]
    fn filters_and_patches() {
        let p = sample();
        assert_eq!(
            filter_for(&p, &Folder::Inbox.into(), 0, 0).unwrap(),
            json!({ "inMailbox": "i" })
        );
        assert_eq!(
            filter_for(&p, &Folder::Starred.into(), 0, 0).unwrap(),
            json!({ "hasKeyword": "$flagged" })
        );
        let windowed = filter_for(&p, &Folder::Inbox.into(), 1, 86_400).unwrap();
        assert_eq!(windowed["operator"], "AND");
        assert_eq!(windowed["conditions"][1]["after"], "1970-01-01T00:00:00Z");
        assert_eq!(
            Value::Object(patch_for(&p, &Change::Read(false)).unwrap()),
            json!({ "keywords/$seen": null })
        );
        assert_eq!(
            Value::Object(patch_for(&p, &Change::Move(Folder::Trash)).unwrap()),
            json!({ "mailboxIds": { "t": true } })
        );
        assert_eq!(
            Value::Object(patch_for(&p, &Change::Label("Later".into(), true)).unwrap()),
            json!({ "keywords/later": true })
        );
        assert!(patch_for(&p, &Change::Move(Folder::Archive)).is_err());
    }

    #[test]
    fn rows_and_drafts_know_their_ids() {
        let mut a = Account::new(Provider::Jmap);
        a.id = "acct".into();
        let m = Mail {
            id: row_id(&a, "Mabc"),
            ..Default::default()
        };
        assert_eq!(jmap_id(&a, &m), Some("Mabc"));
        assert_eq!(jmap_draft(&jmap_remote("Mabc")), Some("Mabc"));
        let imap = RemoteDraft {
            mailbox: "Drafts".into(),
            validity: 7,
            uid: 3,
        };
        assert_eq!(jmap_draft(&imap), None);
    }
}

/// A message the JMAP server holds to send later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSubmission {
    pub id: String,
    pub subject: String,
    /// When it goes (Unix seconds).
    pub send_at: i64,
}
