//! Snoozing: a message leaves the inbox for a "Snoozed" folder on the
//! server until a chosen time, then comes back to the inbox, unread. The
//! folder is inside a "Picacho" folder (made if missing, kept if there),
//! on every provider. (Proton refuses a top-level folder named "Snoozed":
//! the name is its own snooze's.)
//!
//! The times are kept on this computer and on the server, so Picacho on
//! another computer shows and wakes them too (`shared`): as a private
//! annotation on the inbox where the server has METADATA (RFC 5464), and
//! otherwise as a note in the Picacho folder. A message woken or unsnoozed
//! on one computer stops being snoozed on the others; one snoozed again
//! takes the newer time. Messages are found again by their Message-ID (or
//! their ids, which every computer shares): their UIDs change when they
//! move. One moved out of Snoozed by hand is simply forgotten.
use super::*;
use super::shared::{GONE_FOR_MS, Gone, ListKind, Stampable, Stamped, now_ms};

/// The folder snoozed mail waits in, inside `PARENT`.
pub const FOLDER: &str = "Snoozed";
/// Picacho's own folder, at the top of the account's folders.
pub const PARENT: &str = "Picacho";

/// Where Bridge keeps Proton's folders ("Folders/"): it makes nothing at
/// the top level.
fn bridge_folder(names: &[Name], leaf: &str) -> String {
    let delimiter = names
        .iter()
        .find(|n| n.name.eq_ignore_ascii_case("Folders"))
        .and_then(|n| n.delimiter())
        .unwrap_or("/");
    format!("Folders{delimiter}{leaf}")
}
/// The account's top-level folder called `leaf`, if there is one.
fn top_folder(names: &[Name], a: &Account, leaf: &str) -> Option<String> {
    if a.service == Service::ProtonBridge {
        let name = bridge_folder(names, leaf);
        return names
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(&name))
            .map(|n| n.name.clone());
    }
    custom_folders(names)
        .into_iter()
        .find(|f| f.depth == 0 && f.label.eq_ignore_ascii_case(leaf))
        .map(|f| f.name)
}
/// The folder called `leaf` inside `parent`, if there is one.
fn child_folder(names: &[Name], parent: &str, leaf: &str) -> Option<String> {
    let delimiter = names.iter().find(|n| n.name == parent)?.delimiter()?;
    let name = format!("{parent}{delimiter}{leaf}");
    names
        .iter()
        .find(|n| n.name.eq_ignore_ascii_case(&name))
        .map(|n| n.name.clone())
}
/// Where the times are kept on servers with METADATA, as an array (as
/// earlier versions wrote them, and read them still).
const ENTRY: &str = "/private/vendor/tech.pinal.picacho/snoozed";
/// Where the messages no longer snoozed are kept beside them.
const GONE_ENTRY: &str = "/private/vendor/tech.pinal.picacho/snoozed-gone";

/// Held while the list here is merged with the server's (snoozing and
/// waking hold the account's lock, as sharing does too).
static SNOOZED: Mutex<()> = Mutex::new(());

fn snooze_state_key(account: &str) -> String {
    format!("snoozed_synced:{account}")
}
fn load_snooze_list(b: &Backend, account: &str) -> Stamped<Snoozed> {
    let mut list = Stamped { entries: b.snoozed(account), gone: b.snoozed_gone(account) };
    list.normalize();
    list
}
fn store_snooze_list(b: &Backend, account: &str, list: &Stamped<Snoozed>) -> Result<()> {
    b.store_snoozed(account, &list.entries, &list.gone)
}
/// The snoozed messages, as they're shared (`shared`).
pub(crate) static SNOOZE_LIST: ListKind<Snoozed> = ListKind {
    note: "snoozed",
    entry: ENTRY,
    gone_entry: Some(GONE_ENTRY),
    state_key: snooze_state_key,
    load: load_snooze_list,
    store: store_snooze_list,
    guard: &SNOOZED,
    always_lock: true,
};

/// A snoozed message.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snoozed {
    pub message_id: String,
    pub subject: String,
    pub sender: String,
    /// When it comes back, seconds since the epoch.
    pub until: i64,
    /// The server folder it waits in.
    pub folder: String,
    /// Proton (without Bridge) and JMAP: its message ids, which don't change
    /// when it moves, so it is found again by them rather than its Message-ID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// When it was last snoozed, or its time changed, on any computer
    /// (milliseconds since the epoch); 0 in lists from before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub changed: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl Stampable for Snoozed {
    fn key(&self) -> &str {
        &self.message_id
    }
    fn changed(&self) -> i64 {
        self.changed
    }
}

impl Backend {
    fn snooze_key(account: &str) -> String {
        format!("snoozed:{account}")
    }
    fn snooze_gone_key(account: &str) -> String {
        format!("snoozed_gone:{account}")
    }
    /// Messages no longer snoozed, remembered a while (`shared::Gone`).
    fn snoozed_gone(&self, account: &str) -> Vec<Gone> {
        self.store
            .preference(&Self::snooze_gone_key(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// Writes the list here as it is (merged, or stamped).
    fn store_snoozed(&self, account: &str, list: &[Snoozed], gone: &[Gone]) -> Result<()> {
        self.store
            .set_preference(&Self::snooze_key(account), &serde_json::to_string(list)?)?;
        let key = Self::snooze_gone_key(account);
        if gone.is_empty() {
            self.store.remove_preference(&key)
        } else {
            self.store.set_preference(&key, &serde_json::to_string(gone)?)
        }
    }
    /// Shares the snoozed messages with the account's server, merging in
    /// what other computers changed there (`shared`): when it's time, or
    /// now with `force`. Returns whether the list here changed. Skipped
    /// while the account is busy.
    pub fn share_snoozed(&self, a: &Account, force: bool) -> Result<bool> {
        self.sync_list(a, &SNOOZE_LIST, force)
    }
    /// The account's snoozed messages, soonest first.
    pub fn snoozed(&self, account: &str) -> Vec<Snoozed> {
        let mut list: Vec<Snoozed> = self
            .store
            .preference(&Self::snooze_key(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        list.sort_by_key(|s| s.until);
        list
    }
    /// Saves the list after a change made here: entries new or changed
    /// (another time, folder or ids) are dated now, and those removed are
    /// remembered as gone, so other computers follow (`shared`). With a
    /// connection to a server with METADATA, it's shared at once.
    pub(super) fn save_snoozed(
        &self,
        c: Option<&mut Connection>,
        account: &str,
        list: &[Snoozed],
    ) -> Result<()> {
        let before = self.snoozed(account);
        let now = now_ms();
        let mut list = list.to_vec();
        for s in &mut list {
            s.changed = match before.iter().find(|b| b.message_id == s.message_id) {
                Some(b) if b.until == s.until && b.folder == s.folder && b.ids == s.ids => b.changed,
                _ => now,
            };
        }
        let mut gone = self.snoozed_gone(account);
        gone.retain(|g| g.at > now - GONE_FOR_MS && !list.iter().any(|s| s.message_id == g.message_id));
        for b in &before {
            if !list.iter().any(|s| s.message_id == b.message_id) {
                gone.retain(|g| g.message_id != b.message_id);
                gone.push(Gone { message_id: b.message_id.clone(), at: now });
            }
        }
        self.store_snoozed(account, &list, &gone)?;
        if let Some(c) = c
            && c.extensions()?.metadata
        {
            // Best effort: the times are kept here either way, and shared
            // again later.
            let shared = load_snooze_list(self, account);
            if super::shared::write_metadata(c, &SNOOZE_LIST, &shared).is_ok() {
                let state = super::shared::SyncState {
                    shared: shared.fingerprint(),
                    at: chrono::Utc::now().timestamp(),
                    remote: !shared.is_empty(),
                };
                let _ = self.store.set_preference(&snooze_state_key(account), &serde_json::to_string(&state)?);
            }
        }
        Ok(())
    }
    /// This computer's snoozed messages merged with the server's (from
    /// other computers), where it has METADATA: each message once, at its
    /// newest change; one woken elsewhere is no longer snoozed. Kept here.
    fn merged_snoozed(&self, c: &mut Connection, account: &str) -> Result<Vec<Snoozed>> {
        if c.extensions()?.metadata {
            let theirs = super::shared::read_metadata(c, &SNOOZE_LIST)?;
            let ours = load_snooze_list(self, account);
            let merged = ours.clone().merge(theirs, now_ms());
            if merged != ours {
                self.store_snoozed(account, &merged.entries, &merged.gone)?;
            }
        }
        Ok(self.snoozed(account))
    }
    /// The account's Snoozed folder, made (and subscribed to) if missing.
    /// Picacho's own folder on an IMAP server: the one there, or (with
    /// `create`) a new one; `None` when there is none and none was made.
    pub(super) fn picacho_folder(&self, c: &mut Connection, a: &Account, create: bool) -> Result<Option<String>> {
        let names = c.names(true)?.to_vec();
        if let Some(found) = top_folder(&names, a, PARENT) {
            return Ok(Some(found));
        }
        if !create {
            return Ok(None);
        }
        let name = if a.service == Service::ProtonBridge {
            bridge_folder(&names, PARENT)
        } else {
            let namespace = c.namespace()?;
            new_folder_name(&names, a.provider, None, PARENT, namespace.as_deref())?
        };
        self.create_folder(c, &name, PARENT)?;
        Ok(Some(name))
    }
    fn snooze_folder(&self, c: &mut Connection, a: &Account) -> Result<String> {
        // Picacho's folder: the one there, or a new one.
        let parent = self
            .picacho_folder(c, a, true)?
            .with_context(|| t!("manage-snooze-the-server-didnt-keep-the-new"))?;
        let names = c.names(true)?.to_vec();
        let nestable = custom_folders(&names)
            .iter()
            .any(|f| f.name == parent && f.nestable);
        let name = if let Some(found) = child_folder(&names, &parent, FOLDER) {
            found
        } else if nestable {
            let namespace = c.namespace()?;
            let name = new_folder_name(
                &names,
                a.provider,
                Some(&parent),
                FOLDER,
                namespace.as_deref(),
            )?;
            self.create_folder(c, &name, FOLDER)?;
            name
        } else if let Some(found) = top_folder(&names, a, FOLDER) {
            // A server without folders inside folders: Snoozed at the top.
            found
        } else {
            let namespace = c.namespace()?;
            let name = new_folder_name(&names, a.provider, None, FOLDER, namespace.as_deref())?;
            self.create_folder(c, &name, FOLDER)?;
            name
        };
        let folders = custom_folders(c.names(true)?);
        self.store.save_folders(&a.id, &folders)?;
        Ok(name)
    }
    /// Makes a folder on the server and subscribes to it, so other mail
    /// apps show it too.
    fn create_folder(&self, c: &mut Connection, name: &str, label: &str) -> Result<()> {
        let wire = c.mailbox(name)?;
        allowed(
            c.session.run_command_and_check_ok(format!("CREATE {wire}")),
            || t!("manage-snooze-create-the-folder", label = label),
        )?;
        subscribe(c, name)
    }
    /// Snoozes messages until `until`: they move to the Snoozed folder and
    /// come back to the inbox, unread, then. Returns the folder's name.
    pub fn snooze(&self, a: &Account, mails: &[Mail], until: i64) -> Result<String> {
        ensure!(
            a.provider != Provider::Pop3,
            t!("manage-snooze-snoozing-needs-mail-kept-on-the")
        );
        ensure!(
            until > chrono::Utc::now().timestamp(),
            t!("manage-snooze-choose-a-time-in-the-future")
        );
        ensure!(!mails.is_empty(), t!("manage-snooze-choose-messages-to-snooze"));
        ensure!(
            mails.iter().all(|m| m.account == a.id),
            t!("manage-snooze-message-belongs-to-another-account")
        );
        ensure!(
            mails.iter().all(|m| !m.message_id.trim().is_empty()),
            t!("manage-snooze-this-message-has-no-message-id")
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let mut current = Vec::new();
        for m in mails {
            current.push(self.store.message(&m.id)?.unwrap_or_else(|| m.clone()));
        }
        if a.provider == Provider::Proton {
            return self.snooze_proton(a, &current, until);
        }
        if a.provider == Provider::Jmap {
            return self.snooze_jmap(a, &current, until);
        }
        if a.native_api() || a.ews() {
            return self.snooze_api(a, &current, until);
        }
        self.with_imap(a, |c| {
            self.replay_offline(c, a)?;
            let folder = self.snooze_folder(c, a)?;
            let moving: Vec<Mail> = current
                .iter()
                .filter(|m| !(m.folder == Folder::Other && m.remote_folder == folder))
                .cloned()
                .collect();
            if !moving.is_empty() {
                self.change_remote(c, a, &moving, Change::MoveTo(folder.clone()), true)?;
            }
            let mut list = self.merged_snoozed(c, &a.id)?;
            for m in &current {
                list.retain(|s| s.message_id != m.message_id);
                list.push(Snoozed {
                    message_id: m.message_id.trim().to_owned(),
                    subject: m.subject.clone(),
                    sender: m.sender.clone(),
                    until,
                    folder: folder.clone(),
                    ids: Vec::new(),
                    changed: 0,
                });
            }
            self.save_snoozed(Some(c), &a.id, &list)?;
            Ok(folder)
        })
    }
    /// Brings snoozed messages back now.
    pub fn unsnooze(&self, a: &Account, message_ids: &[String]) -> Result<Vec<Snoozed>> {
        let now = chrono::Utc::now().timestamp();
        {
            // Read and written under the account's lock, as waking does: a
            // wake running meanwhile would otherwise save its own list over
            // this one, and the messages stayed snoozed.
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            let mut list = self.snoozed(&a.id);
            for s in &mut list {
                if message_ids.contains(&s.message_id) {
                    s.until = now;
                }
            }
            self.save_snoozed(None, &a.id, &list)?;
        }
        self.wake_due(a)
    }
    /// Brings back the messages whose time has come: each moves from the
    /// Snoozed folder to the inbox, marked unread. Returns those woken.
    pub fn wake_due(&self, a: &Account) -> Result<Vec<Snoozed>> {
        if a.provider == Provider::Pop3 {
            return Ok(vec![]);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        if a.provider == Provider::Proton {
            return self.wake_due_proton(a);
        }
        if a.provider == Provider::Jmap {
            return self.wake_due_jmap(a);
        }
        if a.native_api() || a.ews() {
            return self.wake_due_api(a);
        }
        self.with_imap(a, |c| {
            let list = self.merged_snoozed(c, &a.id)?;
            let now = chrono::Utc::now().timestamp();
            let (due, waiting): (Vec<Snoozed>, Vec<Snoozed>) =
                list.into_iter().partition(|s| s.until <= now);
            if due.is_empty() {
                return Ok(vec![]);
            }
            let names = c.names(true)?.to_vec();
            let mut woken = Vec::new();
            let mut kept = waiting;
            let mut due = due.into_iter();
            for s in due.by_ref() {
                if !names.iter().any(|n| n.name == s.folder) {
                    // Its folder is gone; so is the snooze.
                    continue;
                }
                match self.wake(c, &a.id, &s) {
                    Ok(true) => woken.push(s),
                    // Moved out of Snoozed by hand: nothing to bring back.
                    Ok(false) => {}
                    Err(e) if crate::sync::retry::later(&e) => {
                        // This one and those not reached yet wait for the next try;
                        // they were dropped, their mail left in Snoozed for good.
                        kept.push(s);
                        kept.extend(due);
                        self.save_snoozed(None, &a.id, &kept)?;
                        return Err(e);
                    }
                    Err(_) => {}
                }
            }
            self.save_snoozed(Some(c), &a.id, &kept)?;
            Ok(woken)
        })
    }
    /// Proton: snoozed mail waits in a "Snoozed" folder at Proton, made if
    /// missing, as it would through Proton Mail Bridge; the times are kept
    /// on this computer (Bridge has no METADATA to share them either).
    fn snooze_proton(&self, a: &Account, current: &[Mail], until: i64) -> Result<String> {
        let folder = self.with_proton(a, |s| {
            let made = |id: &str| -> Result<crate::sync::proton::Folder> {
                s.reload_folders()?;
                s.folders()
                    .into_iter()
                    .find(|f| f.id == id)
                    .with_context(|| t!("manage-snooze-proton-didnt-keep-the-new-folder"))
            };
            // Picacho's folder: the one there, or a new one.
            let parent = match s
                .folders()
                .into_iter()
                .find(|f| f.exclusive && f.depth == 0 && f.path.eq_ignore_ascii_case(PARENT))
            {
                Some(f) => f,
                None => made(&s.create_label(PARENT, true, None)?)?,
            };
            let inside = format!("{}/{FOLDER}", parent.path);
            let path = match s
                .folders()
                .into_iter()
                .find(|f| f.exclusive && f.path.eq_ignore_ascii_case(&inside))
            {
                Some(f) => f.path,
                None => made(&s.create_label(FOLDER, true, Some(&parent.id))?)?.path,
            };
            self.store
                .save_folders(&a.id, &super::proton::remote_folders(&s.folders()))?;
            Ok(path)
        })?;
        let moving: Vec<Mail> = current
            .iter()
            .filter(|m| !(m.folder == Folder::Other && m.remote_folder == folder))
            .cloned()
            .collect();
        if !moving.is_empty() {
            let change = Change::MoveTo(folder.clone());
            self.change_proton(a, &moving, &change)?;
            self.commit_change(&moving, change, false)?;
        }
        let mut list = self.snoozed(&a.id);
        for m in current {
            list.retain(|s| s.message_id != m.message_id);
            list.push(Snoozed {
                message_id: m.message_id.trim().to_owned(),
                subject: m.subject.clone(),
                sender: m.sender.clone(),
                until,
                folder: folder.clone(),
                ids: super::proton::proton_id(a, m)
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
                changed: 0,
            });
        }
        self.save_snoozed(None, &a.id, &list)?;
        Ok(folder)
    }
    /// Proton: brings back the messages whose time has come.
    fn wake_due_proton(&self, a: &Account) -> Result<Vec<Snoozed>> {
        let now = chrono::Utc::now().timestamp();
        let (due, waiting): (Vec<Snoozed>, Vec<Snoozed>) = self
            .snoozed(&a.id)
            .into_iter()
            .partition(|s| s.until <= now);
        if due.is_empty() {
            return Ok(vec![]);
        }
        let mut woken = Vec::new();
        let mut kept = waiting;
        let mut due = due.into_iter();
        for s in due.by_ref() {
            match self.wake_proton(a, &s) {
                Ok(true) => woken.push(s),
                // Moved out of Snoozed by hand, or its folder is gone.
                Ok(false) => {}
                Err(e) if crate::sync::retry::later(&e) => {
                    // This one and those not reached yet wait for the next try;
                    // they were dropped, their mail left in Snoozed for good.
                    kept.push(s);
                    kept.extend(due);
                    self.save_snoozed(None, &a.id, &kept)?;
                    return Err(e);
                }
                Err(_) => {}
            }
        }
        self.save_snoozed(None, &a.id, &kept)?;
        Ok(woken)
    }
    /// Proton: moves one snoozed message back to the inbox, unread.
    /// Whether it was still in the Snoozed folder.
    fn wake_proton(&self, a: &Account, s: &Snoozed) -> Result<bool> {
        use protected_email_bridge::api::types::label_id;
        let ids: Vec<String> = if s.ids.is_empty() {
            // Snoozed before ids were kept: found by Message-ID.
            self.store
                .summaries(&a.id, Mailbox::Remote(s.folder.clone()))?
                .iter()
                .filter(|m| m.message_id.trim() == s.message_id)
                .filter_map(|m| super::proton::proton_id(a, m).map(str::to_owned))
                .collect()
        } else {
            s.ids.clone()
        };
        if ids.is_empty() {
            return Ok(false);
        }
        self.with_proton(a, |p| {
            let Ok(label) =
                super::proton::label_for(&p.folders(), &Mailbox::Remote(s.folder.clone()))
            else {
                return Ok(false);
            };
            let still: Vec<String> = p
                .metadata(&ids)?
                .into_iter()
                .filter(|m| m.has_label(&label))
                .map(|m| m.id)
                .collect();
            if still.is_empty() {
                return Ok(false);
            }
            p.label_messages(label_id::INBOX, &still, true)?;
            p.mark_read(&still, false)?;
            let mut rows = Vec::new();
            for id in &still {
                if let Some(mut m) = self.store.message(&super::proton::row_id(a, id))? {
                    m.folder = Folder::Inbox;
                    m.remote_folder = "Inbox".into();
                    m.unread = true;
                    rows.push(m);
                }
            }
            self.store.save_messages(&rows)?;
            Ok(true)
        })
    }
    /// The Gmail API and Microsoft Graph: snoozed mail waits in
    /// Picacho/Snoozed (a label at Gmail, a folder at Microsoft), made if
    /// missing; the times are kept on this computer, with the messages'
    /// ids, which stay the same when they move.
    fn snooze_api(&self, a: &Account, current: &[Mail], until: i64) -> Result<String> {
        let folder = format!("{PARENT}/{FOLDER}");
        let known = |b: &Self| b.store.folders(&a.id).iter().any(|f| f.name == folder);
        if !known(self) {
            // Read again first: it may be there, made on another computer.
            self.refresh_api_folders(a)?;
        }
        if !known(self) {
            let parent_there = self.store.folders(&a.id).iter().any(|f| f.name == PARENT);
            if a.gmail_api() {
                if !parent_there {
                    self.create_folder_gmail(a, None, PARENT)?;
                }
                self.create_folder_gmail(a, Some(PARENT), FOLDER)?;
            } else if a.ews() {
                if !parent_there {
                    self.create_folder_ews(a, None, PARENT)?;
                }
                self.create_folder_ews(a, Some(PARENT), FOLDER)?;
            } else {
                if !parent_there {
                    self.create_folder_graph(a, None, PARENT)?;
                }
                self.create_folder_graph(a, Some(PARENT), FOLDER)?;
            }
        }
        let moving: Vec<Mail> = current
            .iter()
            .filter(|m| !(m.folder == Folder::Other && m.remote_folder == folder))
            .cloned()
            .collect();
        if !moving.is_empty() {
            let change = Change::MoveTo(folder.clone());
            if a.gmail_api() {
                self.change_gmail(a, &moving, &change)?;
                self.commit_change(&moving, change, false)?;
            } else if a.ews() {
                // The rows take their new ids.
                self.change_ews(a, &moving, &change)?;
            } else {
                self.change_graph(a, &moving, &change)?;
                self.commit_change(&moving, change, false)?;
            }
        }
        let mut list = self.snoozed(&a.id);
        for m in current {
            list.retain(|s| s.message_id != m.message_id);
            // Where it is now (an Exchange item's id changes as it moves).
            let now = if a.ews() { self.ews_current(a, m) } else { m.clone() };
            let id = if a.gmail_api() {
                super::gmail::gmail_id(a, &now).map(str::to_owned)
            } else if a.ews() {
                super::ews::ews_id(a, &now).map(str::to_owned)
            } else {
                super::graph::graph_id(a, &now).map(str::to_owned)
            };
            list.push(Snoozed {
                message_id: m.message_id.trim().to_owned(),
                subject: m.subject.clone(),
                sender: m.sender.clone(),
                until,
                folder: folder.clone(),
                ids: id.into_iter().collect(),
                changed: 0,
            });
        }
        self.save_snoozed(None, &a.id, &list)?;
        Ok(folder)
    }
    /// The Gmail API and Microsoft Graph: brings back the messages whose
    /// time has come.
    fn wake_due_api(&self, a: &Account) -> Result<Vec<Snoozed>> {
        let now = chrono::Utc::now().timestamp();
        let (due, waiting): (Vec<Snoozed>, Vec<Snoozed>) =
            self.snoozed(&a.id).into_iter().partition(|s| s.until <= now);
        if due.is_empty() {
            return Ok(vec![]);
        }
        let mut woken = Vec::new();
        let mut kept = waiting;
        let mut due = due.into_iter();
        for s in due.by_ref() {
            match self.wake_api(a, &s) {
                Ok(true) => woken.push(s),
                Ok(false) => {}
                Err(e) if crate::sync::retry::later(&e) => {
                    kept.push(s);
                    kept.extend(due);
                    self.save_snoozed(None, &a.id, &kept)?;
                    return Err(e);
                }
                Err(_) => {}
            }
        }
        self.save_snoozed(None, &a.id, &kept)?;
        Ok(woken)
    }
    /// Moves one snoozed message back to the inbox, unread, if it is still
    /// in the Snoozed folder (as the server says now). Whether it was.
    fn wake_api(&self, a: &Account, s: &Snoozed) -> Result<bool> {
        if s.ids.is_empty() {
            return Ok(false);
        }
        let current = self.api_rows(a, &s.ids)?;
        let still: Vec<Mail> = current
            .into_iter()
            .filter(|m| m.folder == Folder::Other && m.remote_folder == s.folder)
            .collect();
        if still.is_empty() {
            return Ok(false);
        }
        if a.ews() {
            // Read first: the move gives them new ids.
            self.change_ews(a, &still, &Change::Read(false))?;
            let still: Vec<Mail> = still.into_iter().map(|m| Mail { unread: true, ..m }).collect();
            self.change_ews(a, &still, &Change::Move(Folder::Inbox))?;
            return Ok(true);
        }
        for change in [Change::Move(Folder::Inbox), Change::Read(false)] {
            if a.gmail_api() {
                self.change_gmail(a, &still, &change)?;
            } else {
                self.change_graph(a, &still, &change)?;
            }
        }
        let rows: Vec<Mail> = still
            .into_iter()
            .map(|mut m| {
                m.folder = Folder::Inbox;
                m.remote_folder = "Inbox".into();
                m.unread = true;
                m
            })
            .collect();
        self.store.save_messages(&rows)?;
        Ok(true)
    }
    /// JMAP: snoozed mail waits in Picacho/Snoozed on the server, made if
    /// missing; the times are kept on this computer, with the messages'
    /// ids, which stay the same when they move.
    fn snooze_jmap(&self, a: &Account, current: &[Mail], until: i64) -> Result<String> {
        let folder = self.with_jmap(a, |s| {
            let places = super::jmap::places(&s.reload_mailboxes()?);
            let parent = match places
                .iter()
                .find(|p| p.folder.is_none() && p.depth == 0 && p.path.eq_ignore_ascii_case(PARENT))
            {
                Some(p) => p.clone(),
                None => {
                    let id = s.create_mailbox(PARENT, None)?;
                    super::jmap::places(&s.mailboxes()?)
                        .into_iter()
                        .find(|p| p.id == id)
                        .with_context(|| t!("manage-snooze-the-server-didnt-keep-the-new"))?
                }
            };
            let inside = format!("{}/{FOLDER}", parent.path);
            let places = super::jmap::places(&s.mailboxes()?);
            let path = match places
                .iter()
                .find(|p| p.folder.is_none() && p.path.eq_ignore_ascii_case(&inside))
            {
                Some(p) => p.path.clone(),
                None => {
                    s.create_mailbox(FOLDER, Some(&parent.id))?;
                    inside
                }
            };
            self.store.save_folders(
                &a.id,
                &super::jmap::remote_folders(&super::jmap::places(&s.mailboxes()?)),
            )?;
            Ok(path)
        })?;
        let moving: Vec<Mail> = current
            .iter()
            .filter(|m| !(m.folder == Folder::Other && m.remote_folder == folder))
            .cloned()
            .collect();
        if !moving.is_empty() {
            let change = Change::MoveTo(folder.clone());
            self.change_jmap(a, &moving, &change)?;
            self.commit_change(&moving, change, false)?;
        }
        let mut list = self.snoozed(&a.id);
        for m in current {
            list.retain(|s| s.message_id != m.message_id);
            list.push(Snoozed {
                message_id: m.message_id.trim().to_owned(),
                subject: m.subject.clone(),
                sender: m.sender.clone(),
                until,
                folder: folder.clone(),
                ids: super::jmap::jmap_id(a, m)
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
                changed: 0,
            });
        }
        self.save_snoozed(None, &a.id, &list)?;
        Ok(folder)
    }
    /// JMAP: brings back the messages whose time has come.
    fn wake_due_jmap(&self, a: &Account) -> Result<Vec<Snoozed>> {
        let now = chrono::Utc::now().timestamp();
        let (due, waiting): (Vec<Snoozed>, Vec<Snoozed>) = self
            .snoozed(&a.id)
            .into_iter()
            .partition(|s| s.until <= now);
        if due.is_empty() {
            return Ok(vec![]);
        }
        let mut woken = Vec::new();
        let mut kept = waiting;
        let mut due = due.into_iter();
        for s in due.by_ref() {
            match self.wake_jmap(a, &s) {
                Ok(true) => woken.push(s),
                // Moved out of Snoozed by hand, or its folder is gone.
                Ok(false) => {}
                Err(e) if crate::sync::retry::later(&e) => {
                    // This one and those not reached yet wait for the next try;
                    // they were dropped, their mail left in Snoozed for good.
                    kept.push(s);
                    kept.extend(due);
                    self.save_snoozed(None, &a.id, &kept)?;
                    return Err(e);
                }
                Err(_) => {}
            }
        }
        self.save_snoozed(None, &a.id, &kept)?;
        Ok(woken)
    }
    /// JMAP: moves one snoozed message back to the inbox, unread. Whether
    /// it was still in the Snoozed folder.
    fn wake_jmap(&self, a: &Account, s: &Snoozed) -> Result<bool> {
        if s.ids.is_empty() {
            return Ok(false);
        }
        self.with_jmap(a, |j| {
            let places = super::jmap::places(&j.mailboxes()?);
            let Ok(snoozed) = super::jmap::mailbox_id(&places, &Mailbox::Remote(s.folder.clone()))
            else {
                return Ok(false);
            };
            let inbox = super::jmap::mailbox_id(&places, &Mailbox::Standard(Folder::Inbox))?;
            let still: Vec<String> = j
                .emails(&s.ids)?
                .into_iter()
                .filter(|e| e.in_mailbox(&snoozed))
                .map(|e| e.id)
                .collect();
            if still.is_empty() {
                return Ok(false);
            }
            let patches: serde_json::Map<String, serde_json::Value> = still
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        serde_json::json!({ "mailboxIds": { &inbox: true }, "keywords/$seen": null }),
                    )
                })
                .collect();
            j.update(patches)?;
            let mut rows = Vec::new();
            for id in &still {
                if let Some(mut m) = self.store.message(&super::jmap::row_id(a, id))? {
                    m.folder = Folder::Inbox;
                    m.remote_folder = "Inbox".into();
                    m.unread = true;
                    rows.push(m);
                }
            }
            self.store.save_messages(&rows)?;
            Ok(true)
        })
    }
    /// Moves one snoozed message back to the inbox, unread. Whether it was
    /// still there.
    fn wake(&self, c: &mut Connection, account: &str, s: &Snoozed) -> Result<bool> {
        let selected = c.select(&s.folder)?;
        let criteria = format!("HEADER Message-ID {}", quote(&s.message_id)?);
        let ext = c.extensions()?;
        // With SEARCHRES the server keeps what it found as `$`: the STORE
        // and MOVE act on exactly those, not on whatever matches by then.
        let saved = ext.searchres && ext.esearch;
        let uids: Vec<u32> = if saved {
            crate::sync::transport::uid_search_saved(&mut c.session, &criteria, c.enabled.utf8)?
        } else {
            c.uid_search(&criteria)?.into_iter().collect()
        };
        if uids.is_empty() {
            return Ok(false);
        }
        let set = if saved && ext.moves { "$".to_owned() } else { uid_ranges(&uids) };
        c.require_writable(t!("manage-snooze-move-snoozed-mail-back"))?;
        c.session.uid_store(&set, "-FLAGS.SILENT (\\Seen)")?;
        let inbox = c.mailbox("INBOX")?;
        if c.extensions()?.moves {
            c.session
                .run_command_and_check_ok(format!("UID MOVE {set} {inbox}"))?;
        } else {
            c.can_erase(&uids)?;
            c.session
                .run_command_and_check_ok(format!("UID COPY {set} {inbox}"))?;
            c.erase(&uids)?;
        }
        // The Snoozed folder's cached copies are gone from it now, and its
        // saved count is one lower, so its next check stays incremental.
        let prefix = format!("{account}:{}:{}:", s.folder, selected.validity);
        let ids: Vec<String> = uids.iter().map(|u| format!("{prefix}{u}")).collect();
        self.store.delete_ids(account, &ids)?;
        if let Some(layout) = self
            .layouts
            .lock()
            .unwrap()
            .get_mut(&(account.to_owned(), Mailbox::Remote(s.folder.clone()).key()))
            .filter(|l| l.validity == selected.validity)
        {
            layout.uids = Arc::new(
                layout
                    .uids
                    .iter()
                    .copied()
                    .filter(|u| !uids.contains(u))
                    .collect(),
            );
            self.layouts_changed(account);
        }
        if let Some(mut state) = self.store.mailbox_state(account, &s.folder)?
            && state.validity == selected.validity
        {
            state.exists = state.exists.saturating_sub(uids.len() as u32);
            self.store
                .save_mailbox_state(account, &s.folder, Some(&state))?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod folder_name_tests {
    use super::*;

    #[test]
    fn snoozed_is_inside_picachos_folder() {
        let names = vec![
            Name::for_test("INBOX", "/", &[]),
            Name::for_test("Picacho", "/", &[]),
            Name::for_test("Picacho/Snoozed", "/", &[]),
            Name::for_test("Folders", "/", &["\\Noselect"]),
            Name::for_test("Folders/Picacho", "/", &[]),
        ];
        let mut imap = Account::new(Provider::Imap);
        imap.service = Service::Fastmail;
        assert_eq!(
            top_folder(&names, &imap, PARENT).as_deref(),
            Some("Picacho")
        );
        assert_eq!(
            child_folder(&names, "Picacho", FOLDER).as_deref(),
            Some("Picacho/Snoozed")
        );
        // Bridge keeps Proton's folders under "Folders/".
        let mut bridge = Account::new(Provider::Imap);
        bridge.service = Service::ProtonBridge;
        assert_eq!(
            top_folder(&names, &bridge, PARENT).as_deref(),
            Some("Folders/Picacho")
        );
        assert_eq!(bridge_folder(&names, PARENT), "Folders/Picacho");
        assert_eq!(child_folder(&names, "Folders/Picacho", FOLDER), None);
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    fn snoozed(id: &str, until: i64) -> Snoozed {
        Snoozed {
            message_id: id.into(),
            subject: "Plans".into(),
            sender: "Ana".into(),
            until,
            folder: "Picacho/Snoozed".into(),
            ids: vec![],
            changed: 0,
        }
    }

    /// Changes made here are dated, and messages no longer snoozed are
    /// remembered as gone, for other computers to follow.
    #[test]
    fn changes_here_are_dated_and_removals_remembered() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("picacho-snooze-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(crate::manage::store::Store::open(&dir)?));
        let now = chrono::Utc::now().timestamp();
        b.save_snoozed(None, "acct", &[snoozed("<a@x>", now + 60), snoozed("<b@x>", now + 60)])?;
        let first = b.snoozed("acct");
        assert!(first.iter().all(|s| s.changed > 0));
        // Unchanged: its date stays; snoozed again until later: dated anew.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut again = first.clone();
        again[1].until = now + 3600;
        b.save_snoozed(None, "acct", &again)?;
        let second = b.snoozed("acct");
        assert_eq!(second[0].changed, first[0].changed);
        assert!(second[1].changed > first[1].changed);
        // Woken: gone, and remembered as such.
        b.save_snoozed(None, "acct", &second[1..])?;
        let gone: Vec<String> = b.snoozed_gone("acct").into_iter().map(|g| g.message_id).collect();
        assert_eq!(gone, ["<a@x>"]);
        // Snoozed again: no longer gone.
        b.save_snoozed(None, "acct", &[snoozed("<a@x>", now + 60)])?;
        let gone: Vec<String> = b.snoozed_gone("acct").into_iter().map(|g| g.message_id).collect();
        assert_eq!(gone, ["<b@x>"]);
        // A list written by an earlier version (no dates) still reads.
        b.store.set_preference("snoozed:old", r#"[{"message_id":"<o@x>","subject":"","sender":"","until":1,"folder":"S"}]"#)?;
        assert_eq!(b.snoozed("old")[0].changed, 0);
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }
}
