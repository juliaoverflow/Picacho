use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, mpsc},
    time::Duration,
};

/// Connections that only read. In WAL mode they read while a write is in
/// progress, so listing mail, counting unread mail or opening a message
/// never waits for a long synchronization to commit.
const READERS: usize = 3;
/// How long a connection waits for SQLite's own write lock, held by
/// another connection, before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// The local cache.
///
/// Writes go through one connection, one at a time; reads use a small pool
/// of their own. Preferences changed by the interface are written by a
/// background thread (`set_preference_soon`), so a click never waits for
/// the cache.
pub struct Store {
    db: Mutex<Connection>,
    /// Set by the writer's triggers when a write changes what message lists
    /// show, by account; turned into new `list_revision`s once the write is
    /// committed.
    list_dirty: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Each account's list revision: the `list_changes` count at its latest
    /// list-changing write. Absent for an account not changed since launch.
    list_revisions: Mutex<HashMap<String, u64>>,
    list_changes: std::sync::atomic::AtomicU64,
    readers: Vec<Mutex<Connection>>,
    next_reader: std::sync::atomic::AtomicUsize,
    prefs: Arc<PrefQueue>,
    pref_tx: Mutex<Option<mpsc::Sender<()>>>,
    pref_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub path: PathBuf,
}

/// Preferences waiting to be written, newest value per key. Reads see them
/// before they reach the database.
#[derive(Default)]
struct PrefQueue {
    state: Mutex<PrefState>,
    idle: Condvar,
}
#[derive(Default)]
struct PrefState {
    /// Set but not yet taken by the writer.
    pending: HashMap<String, String>,
    /// Being written now.
    writing: HashMap<String, String>,
    /// Each key's latest `set_preference_soon` ticket.
    latest: HashMap<String, u64>,
    next_ticket: u64,
    /// Why the last write of a key failed, until one succeeds.
    failed: HashMap<String, String>,
}
impl PrefState {
    fn get(&self, key: &str) -> Option<&String> {
        self.pending.get(key).or_else(|| self.writing.get(key))
    }
    fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.writing.is_empty()
    }
    fn busy(&self, key: &str) -> bool {
        self.pending.contains_key(key) || self.writing.contains_key(key)
    }
}

/// The held writing connection. Dropped, after whatever it wrote was
/// committed (or rolled back) and before the connection is free again, it
/// publishes a list-changing write as a new `Store::list_revision`.
struct Writer<'a> {
    db: MutexGuard<'a, Connection>,
    store: &'a Store,
}
impl std::ops::Deref for Writer<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.db
    }
}
impl std::ops::DerefMut for Writer<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.db
    }
}
impl Drop for Writer<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        // A rolled-back write counts too: an extra rebuild, never a stale list.
        let dirty = std::mem::take(
            &mut *self.store.list_dirty.lock().unwrap_or_else(|e| e.into_inner()),
        );
        if !dirty.is_empty() {
            let change = self.store.list_changes.fetch_add(1, Ordering::AcqRel) + 1;
            let mut revisions = self
                .store
                .list_revisions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for account in dirty {
                revisions.insert(account, change);
            }
        }
    }
}

fn connect(file: &Path) -> Result<Connection> {
    let db = Connection::open(file)?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    // The write-ahead log is cut back to this after each checkpoint; left
    // alone it stayed as large as it had ever grown (24 MB here).
    db.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;
    Ok(db)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        let file = path.join("mail.sqlite3");
        let mut db = connect(&file)?;
        // Free pages go back to the system on `incremental_vacuum` (see
        // `pack_saved_sources`). Takes effect for a new database; an older
        // one is switched by the VACUUM after its sources are packed.
        db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS accounts (id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS messages (id TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, date INTEGER NOT NULL, data TEXT NOT NULL, raw BLOB);
            CREATE INDEX IF NOT EXISTS mailbox ON messages(account, folder, date DESC);
            -- Partial indexes for the counts and lists asked for most: unread mail
            -- (sidebar counts, on every check and download batch) and mail from
            -- subscriptions. Without them each scanned every message's JSON.
            -- Their conditions must match those queries' WHERE word for word.
            CREATE INDEX IF NOT EXISTS unread ON messages(account, folder) WHERE json_extract(data,'$.unread');
            CREATE INDEX IF NOT EXISTS subscribed ON messages(account) WHERE json_extract(data,'$.list_unsubscribe') IS NOT NULL;
            -- What background downloads count and pick, without reading rows:
            -- download progress is counted up to every 400 ms while mail downloads.
            CREATE INDEX IF NOT EXISTS downloads ON messages(account, date, json_extract(data,'$.size'), raw IS NULL);
            CREATE TABLE IF NOT EXISTS drafts (id TEXT PRIMARY KEY, account TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS mailbox_state (account TEXT NOT NULL, remote TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (account, remote));")?;
        // Other connections write too (preferences); taking the write lock
        // when a transaction begins means one that reads first can't find
        // the database changed under it when it comes to write.
        db.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
        // Every write that changes the rows lists are built from marks the
        // cache changed; a write that changes nothing (a sync with no news)
        // leaves the lists as they are, so they need not be built again.
        // Only this connection writes messages; temporary triggers are its own.
        // Changes are kept by account, so a view of every account rebuilds
        // only the part of its list whose account changed.
        let list_dirty: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
        let dirty = list_dirty.clone();
        db.create_scalar_function(
            "picacho_list_changed",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |ctx| {
                let account = ctx.get::<String>(0)?;
                let mut dirty = dirty.lock().unwrap_or_else(|e| e.into_inner());
                if !dirty.contains(&account) {
                    dirty.insert(account);
                }
                Ok(0i64)
            },
        )?;
        db.execute_batch(
            "CREATE TEMP TRIGGER list_inserted AFTER INSERT ON main.messages BEGIN SELECT picacho_list_changed(NEW.account); END;
             CREATE TEMP TRIGGER list_deleted AFTER DELETE ON main.messages BEGIN SELECT picacho_list_changed(OLD.account); END;
             CREATE TEMP TRIGGER list_updated AFTER UPDATE OF account, folder, date, data ON main.messages BEGIN SELECT picacho_list_changed(NEW.account); SELECT picacho_list_changed(OLD.account); END;",
        )?;
        let readers = (0..READERS)
            .map(|_| {
                let r = connect(&file)?;
                r.execute_batch("PRAGMA query_only=ON;")?;
                Ok(Mutex::new(r))
            })
            .collect::<Result<Vec<_>>>()?;
        let prefs = Arc::new(PrefQueue::default());
        let (tx, rx) = mpsc::channel::<()>();
        let writer = connect(&file)?;
        let queue = prefs.clone();
        let thread = std::thread::Builder::new()
            .name("preferences".into())
            .spawn(move || write_preferences(writer, &queue, rx))?;
        Ok(Self {
            db: Mutex::new(db),
            list_dirty,
            list_revisions: Default::default(),
            list_changes: Default::default(),
            readers,
            next_reader: Default::default(),
            prefs,
            pref_tx: Mutex::new(Some(tx)),
            pref_thread: Mutex::new(Some(thread)),
            path: path.into(),
        })
    }
    /// The writing connection, held until the returned guard is dropped.
    fn write(&self) -> Writer<'_> {
        Writer {
            db: self.db.lock().unwrap(),
            store: self,
        }
    }
    /// Changes whenever a committed write changes what `account`'s message
    /// lists show (its messages added, removed, or their data, folder or
    /// date changed). Read it before reading a list: an unchanged revision
    /// later means the list read is still current. Writes that change
    /// nothing, or only other accounts' mail, keep it.
    pub fn list_revision(&self, account: &str) -> u64 {
        self.list_revisions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(account)
            .copied()
            .unwrap_or(0)
    }
    /// A connection for reading: a free one if any, otherwise the next in turn.
    fn read(&self) -> MutexGuard<'_, Connection> {
        for r in &self.readers {
            if let Ok(db) = r.try_lock() {
                return db;
            }
        }
        let i = self
            .next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.readers.len();
        self.readers[i].lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Waits until preferences set with `set_preference_soon` are written.
    pub fn flush(&self) {
        let mut state = self.prefs.state.lock().unwrap_or_else(|e| e.into_inner());
        while !state.is_idle() {
            // The writer may have stopped; don't wait for it forever.
            let (next, timeout) = self
                .prefs
                .idle
                .wait_timeout(state, BUSY_TIMEOUT)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
            if timeout.timed_out() {
                break;
            }
        }
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let db = self.read();
        let mut q = db.prepare("SELECT data FROM accounts ORDER BY rowid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn save_account(&self, a: &Account) -> Result<()> {
        // OAuth client secrets are credentials, just like refresh tokens.
        let mut safe = a.clone();
        safe.client_secret.clear();
        self.write().execute(
            "INSERT OR REPLACE INTO accounts VALUES (?1,?2)",
            params![a.id, serde_json::to_string(&safe)?],
        )?;
        Ok(())
    }
    pub fn remove_account(&self, id: &str) -> Result<()> {
        self.flush();
        let mut db = self.write();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM preferences WHERE key=?1",
            [format!("folders:{id}")],
        )?;
        tx.execute(
            "DELETE FROM preferences WHERE substr(key,1,8)='reading:' AND json_valid(substr(key,9)) AND json_extract(substr(key,9),'$[0]')=?1", [id],
        )?;
        for table in ["accounts", "messages", "drafts", "mailbox_state"] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE {}=?1",
                    if table == "accounts" { "id" } else { "account" }
                ),
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn messages(
        &self,
        account: &str,
        mailbox: impl Into<Mailbox>,
        limit: usize,
    ) -> Result<Vec<Mail>> {
        self.query_messages(account, mailbox.into(), limit, false)
    }
    /// Every cached message in a mailbox, newest first, without bodies: the
    /// catalog a message list shows. Opening a message reads its body.
    pub fn summaries(&self, account: &str, mailbox: impl Into<Mailbox>) -> Result<Vec<Mail>> {
        self.query_messages(account, mailbox.into(), usize::MAX >> 1, true)
    }
    /// The newest `limit` cached messages in a mailbox, without bodies: what
    /// a check reports (new mail to announce, flags), not what is read.
    pub fn newest(&self, account: &str, mailbox: impl Into<Mailbox>, limit: usize) -> Result<Vec<Mail>> {
        self.query_messages(account, mailbox.into(), limit, true)
    }
    /// Cached message identities that start with `prefix`.
    pub fn cached_ids(
        &self,
        account: &str,
        prefix: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let db = self.read();
        let mut q =
            db.prepare("SELECT id FROM messages WHERE account=?1 AND substr(id,1,?2)=?3")?;
        let ids = q
            .query_map(params![account, prefix.chars().count(), prefix], |r| {
                r.get(0)
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
    fn query_messages(
        &self,
        account: &str,
        mailbox: Mailbox,
        limit: usize,
        summaries: bool,
    ) -> Result<Vec<Mail>> {
        let folder = mailbox.folder();
        let db = self.read();
        let data = if summaries {
            "json_remove(data,'$.body','$.html')"
        } else {
            "data"
        };
        // All Mail is everything but drafts; Starred, the starred mail. An
        // ordinary mailbox is asked for alone, so that SQLite reads it from
        // the (account, folder, date) index rather than scanning and
        // sorting every message of the account.
        let which = match folder {
            Folder::Starred => "(folder=?2 OR json_extract(data,'$.starred'))",
            Folder::All => "(folder=?2 OR folder<>'drafts')",
            _ => "folder=?2",
        };
        let mut q = db.prepare(&format!("SELECT {data} FROM messages WHERE account=?1 AND {which} ORDER BY date DESC, id DESC LIMIT ?3"))?;
        let mut rows = q.query(params![
            account,
            mailbox.key(),
            if folder.is_virtual() {
                usize::MAX >> 1
            } else {
                limit
            }
        ])?;
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while let Some(row) = rows.next()? {
            // Parsed where SQLite holds the text, without copying it first.
            let mut mail: Mail = serde_json::from_str(row.get_ref(0)?.as_str()?)?;
            if summaries {
                mail.body = None;
                mail.html = None;
            }
            if folder != Folder::Starred || mail.starred {
                // A message in several mailboxes (Gmail's labels) is listed once.
                if folder.is_virtual()
                    && !seen.insert(if mail.message_id.is_empty() {
                        mail.id.clone()
                    } else {
                        mail.message_id.clone()
                    })
                {
                    continue;
                }
                result.push(mail);
            }
        }
        result.truncate(limit);
        Ok(result)
    }
    pub fn save_messages(&self, messages: &[Mail]) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        write_messages(&tx, messages)?;
        tx.commit()?;
        Ok(())
    }
    /// Saves what reading a message found, keeping its flags and place as
    /// cached now: they may have changed since it was read. Returns the
    /// message as saved, or `None` when it is no longer cached (moved or
    /// deleted meanwhile), in which case nothing is saved.
    pub fn refresh_message(&self, read: &Mail) -> Result<Option<Mail>> {
        use rusqlite::OptionalExtension;
        let mut db = self.write();
        let tx = db.transaction()?;
        let current: Option<String> = tx
            .query_row("SELECT data FROM messages WHERE id=?1", [&read.id], |r| r.get(0))
            .optional()?;
        let Some(current) = current else {
            return Ok(None);
        };
        let current: Mail = serde_json::from_str(&current)?;
        let labels = current.labels().to_vec();
        let mut next = Mail {
            unread: current.unread,
            starred: current.starred,
            folder: current.folder,
            remote_folder: current.remote_folder,
            ..read.clone()
        };
        next.set_labels(labels);
        write_messages(&tx, std::slice::from_ref(&next))?;
        tx.commit()?;
        Ok(Some(next))
    }
    /// Commit a completed download and its stable identity together. A failed
    /// write must never leave POP3 thinking a missing body was downloaded.
    pub fn save_download(&self, mail: &Mail, raw: &[u8]) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        write_messages(&tx, std::slice::from_ref(mail))?;
        tx.execute(
            "UPDATE messages SET raw=?2 WHERE id=?1",
            params![mail.id, pack(raw)],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Apply one verified IMAP snapshot atomically. Search identities cover the
    /// entire retention window, even if only the newest headers were requested.
    pub fn apply_snapshot(&self, snapshot: &MailboxSnapshot<'_>, messages: &[Mail]) -> Result<()> {
        let MailboxSnapshot {
            account,
            mailbox,
            remote,
            validity,
            ids,
            starred_only,
        } = snapshot;
        ensure!(
            *validity > 0,
            "The server did not supply a valid UIDVALIDITY"
        );
        let prefix = format!("{account}:{remote}:{validity}:");
        let keep: std::collections::HashSet<_> = ids.iter().collect();
        ensure!(
            messages.iter().all(|m| m.account == *account
                && m.remote_folder == *remote
                && m.id.starts_with(&prefix)
                && keep.contains(&m.id)),
            "Invalid mailbox snapshot"
        );
        let mut db = self.write();
        let tx = db.transaction()?;
        // Invalidate old generations across standard and custom-folder views,
        // including snapshots obtained through the virtual Starred mailbox.
        tx.execute("DELETE FROM messages WHERE account=?1 AND json_extract(data,'$.remote_folder')=?2 AND substr(id,1,?3)<>?4", params![account, remote, prefix.chars().count(), prefix])?;
        if *starred_only {
            let old: Vec<(String, String)> = {
                let mut q = tx.prepare("SELECT id,data FROM messages WHERE account=?1 AND json_extract(data,'$.remote_folder')=?2 AND json_extract(data,'$.starred')")?;
                q.query_map(params![account, remote], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (id, data) in old {
                if !keep.contains(&id) {
                    let mut mail: Mail = serde_json::from_str(&data)?;
                    mail.starred = false;
                    write_messages(&tx, &[mail])?;
                }
            }
        } else {
            let old: Vec<String> = {
                let mut q = tx.prepare("SELECT id FROM messages WHERE account=?1 AND folder=?2")?;
                q.query_map(params![account, mailbox.key()], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for id in old {
                if !keep.contains(&id) {
                    tx.execute("DELETE FROM messages WHERE id=?1", [id])?;
                }
            }
        }
        write_messages(&tx, messages)?;
        tx.commit()?;
        Ok(())
    }
    /// The subscriptions detected in `accounts`' cached mail.
    pub fn subscriptions(
        &self,
        accounts: &[String],
    ) -> Result<Vec<crate::manage::subscriptions::Subscription>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT json_remove(data,'$.body','$.html') FROM messages WHERE account=?1 AND json_extract(data,'$.list_unsubscribe') IS NOT NULL",
        )?;
        let mut mails = Vec::new();
        for account in accounts {
            for row in q.query_map([account], |r| r.get::<_, String>(0))? {
                if let Ok(mail) = serde_json::from_str::<Mail>(&row?) {
                    mails.push(mail);
                }
            }
        }
        Ok(crate::manage::subscriptions::group(mails))
    }
    /// Every copy of the mail from `address` in `account` that offers to
    /// unsubscribe, outside Junk and Trash, newest first. Copies of one
    /// message are all listed; `subscriptions::one_copy_each` picks one.
    pub fn subscription_mail(&self, account: &str, address: &str) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT json_remove(data,'$.body','$.html') FROM messages WHERE account=?1 AND lower(trim(json_extract(data,'$.from')))=?2 AND json_extract(data,'$.list_unsubscribe') IS NOT NULL ORDER BY json_extract(data,'$.date') DESC",
        )?;
        let mut out = Vec::new();
        for row in q.query_map(params![account, address.trim().to_lowercase()], |r| r.get::<_, String>(0))? {
            if let Ok(mail) = serde_json::from_str::<Mail>(&row?)
                && crate::manage::subscriptions::Subscription::of(&mail).is_some()
            {
                out.push(mail);
            }
        }
        Ok(out)
    }
    /// The newest `limit` messages from `address` in `account` that offer
    /// to unsubscribe, newest first, each once, outside Junk and Trash.
    pub fn subscription_messages(&self, account: &str, address: &str, limit: usize) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT json_remove(data,'$.body','$.html') FROM messages WHERE account=?1 AND lower(trim(json_extract(data,'$.from')))=?2 AND json_extract(data,'$.list_unsubscribe') IS NOT NULL ORDER BY json_extract(data,'$.date') DESC",
        )?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for row in q.query_map(params![account, address.trim().to_lowercase()], |r| r.get::<_, String>(0))? {
            let Ok(mail) = serde_json::from_str::<Mail>(&row?) else {
                continue;
            };
            if crate::manage::subscriptions::Subscription::of(&mail).is_none() {
                continue;
            }
            let identity = if mail.message_id.is_empty() { mail.id.clone() } else { mail.message_id.clone() };
            if seen.insert(identity) {
                out.push(mail);
                if out.len() == limit {
                    break;
                }
            }
        }
        Ok(out)
    }
    /// Works out again the preview (and text, where kept) of cached
    /// messages from their downloaded source, after how mail is read as
    /// text changed (Office markup, character references and invisible
    /// padding used to show in previews). Background downloads keep only a
    /// preview, so it is worked out from the source whenever there is one.
    /// Returns how many changed.
    pub fn refresh_previews(&self) -> Result<usize> {
        let ids: Vec<String> = {
            let db = self.read();
            let mut q = db.prepare(
                "SELECT id FROM messages WHERE raw IS NOT NULL OR json_type(data,'$.body')='text' OR json_extract(data,'$.snippet')<>''",
            )?;
            q.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut changed = 0;
        // In batches, so other work gets the cache in between.
        for chunk in ids.chunks(100) {
            let rows: Vec<(String, String, Option<Vec<u8>>)> = {
                let db = self.read();
                let mut q = db.prepare("SELECT data, raw FROM messages WHERE id=?1")?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(row) = q
                        .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)))?
                        .next()
                    {
                        let (data, raw) = row?;
                        rows.push((id.clone(), data, raw.map(unpack).transpose()?));
                    }
                }
                rows
            };
            let mut updates = Vec::new();
            for (id, data, raw) in rows {
                let Ok(mail) = serde_json::from_str::<Mail>(&data) else {
                    continue;
                };
                // Encrypted sources hold no readable text; what is cached stays.
                let parsed = raw
                    .as_deref()
                    .filter(|raw| {
                        crate::manage::crypto::encrypted_part(raw).is_none()
                            && !raw.windows(27).any(|w| w == b"-----BEGIN PGP MESSAGE-----")
                    })
                    .and_then(|raw| {
                        let mut a = Account::new(Provider::Imap);
                        a.id = mail.account.clone();
                        crate::sync::mime::parse_message(
                            raw,
                            &a,
                            &mail.id,
                            mail.folder,
                            &mail.remote_folder,
                            mail.uid,
                        )
                        .ok()
                    });
                let (body, snippet) = match parsed {
                    Some(p) => (
                        // Only text that was kept is replaced.
                        mail.body.as_ref().and(p.body.clone()).or(mail.body.clone()),
                        p.snippet,
                    ),
                    None => {
                        let text = mail.body.as_deref().unwrap_or(&mail.snippet);
                        (mail.body.clone(), crate::sync::mime::snippet(text))
                    }
                };
                if body != mail.body || snippet != mail.snippet {
                    updates.push((id, body, snippet));
                }
            }
            if updates.is_empty() {
                continue;
            }
            let mut db = self.write();
            let tx = db.transaction()?;
            for (id, body, snippet) in &updates {
                match body {
                    Some(body) => tx.execute(
                        "UPDATE messages SET data=json_set(data,'$.body',?2,'$.snippet',?3) WHERE id=?1",
                        params![id, body, snippet],
                    )?,
                    None => tx.execute(
                        "UPDATE messages SET data=json_set(data,'$.snippet',?2) WHERE id=?1",
                        params![id, snippet],
                    )?,
                };
            }
            tx.commit()?;
            changed += updates.len();
        }
        Ok(changed)
    }
    /// Reads List-Unsubscribe from the downloaded source of messages cached
    /// before it was kept. Returns how many were found.
    pub fn backfill_subscriptions(&self) -> Result<usize> {
        self.backfill_subscriptions_of(None)
    }
    /// `backfill_subscriptions` for the mail of `accounts` only.
    pub fn backfill_subscriptions_in(&self, accounts: &[String]) -> Result<usize> {
        accounts
            .iter()
            .map(|id| self.backfill_subscriptions_of(Some(id)))
            .sum()
    }
    fn backfill_subscriptions_of(&self, account: Option<&str>) -> Result<usize> {
        let ids: Vec<String> = {
            let db = self.read();
            let mut q = db.prepare(
                "SELECT id FROM messages WHERE raw IS NOT NULL AND json_extract(data,'$.list_unsubscribe') IS NULL AND (?1 IS NULL OR account=?1)",
            )?;
            q.query_map([account], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut total = 0;
        // In batches, as refresh_previews does, so sync and the interface
        // get the cache in between.
        for chunk in ids.chunks(100) {
            let rows: Vec<(String, Vec<u8>)> = {
                let db = self.read();
                let mut q = db.prepare(
                    "SELECT raw FROM messages WHERE id=?1 AND json_extract(data,'$.list_unsubscribe') IS NULL",
                )?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(saved) = q.query_map([id], |r| r.get::<_, Vec<u8>>(0))?.next() {
                        // Only the headers are needed, which come first.
                        rows.push((id.clone(), unpack_up_to(saved?, 65536)?));
                    }
                }
                rows
            };
            let mut found = Vec::new();
            for (id, head) in rows {
                if !head.windows(16).any(|w| w.eq_ignore_ascii_case(b"list-unsubscribe")) {
                    continue;
                }
                let Ok((headers, _)) = mailparse::parse_headers(&head) else {
                    continue;
                };
                use mailparse::MailHeaderMap;
                let header = headers.get_first_value("List-Unsubscribe").unwrap_or_default();
                if header.trim().is_empty() {
                    continue;
                }
                let one_click = headers
                    .get_first_value("List-Unsubscribe-Post")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .replace(' ', "")
                    .contains("list-unsubscribe=one-click");
                found.push((id, header.trim().to_owned(), one_click));
            }
            if found.is_empty() {
                continue;
            }
            let mut db = self.write();
            let tx = db.transaction()?;
            for (id, header, one_click) in &found {
                tx.execute(
                    "UPDATE messages SET data=json_set(data,'$.list_unsubscribe',?2,'$.one_click',json(?3)) WHERE id=?1",
                    params![id, header, if *one_click { "true" } else { "false" }],
                )?;
            }
            tx.commit()?;
            total += found.len();
        }
        Ok(total)
    }
    pub fn message(&self, id: &str) -> Result<Option<Mail>> {
        use rusqlite::OptionalExtension;
        let json: Option<String> = self
            .read()
            .query_row("SELECT data FROM messages WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    #[cfg(test)]
    pub fn reconcile(
        &self,
        account: &str,
        mailbox: impl Into<Mailbox>,
        ids: &[String],
    ) -> Result<()> {
        let key = mailbox.into().key();
        let mut db = self.write();
        let tx = db.transaction()?;
        let old: Vec<String> = {
            let mut q = tx.prepare("SELECT id FROM messages WHERE account=?1 AND folder=?2")?;
            q.query_map(params![account, key], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let keep: std::collections::HashSet<_> = ids.iter().collect();
        for id in old {
            if !keep.contains(&id) {
                tx.execute("DELETE FROM messages WHERE id=?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Compresses message sources saved before they were kept compressed,
    /// a batch at a time, so other work gets the cache in between; then
    /// gives the space freed back to the system. Returns how many were.
    pub fn pack_saved_sources(&self) -> Result<usize> {
        let ids: Vec<String> = {
            let db = self.read();
            let mut q = db.prepare(
                "SELECT id FROM messages WHERE raw IS NOT NULL AND substr(raw,1,4)<>?1",
            )?;
            q.query_map([PACKED], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut packed = 0;
        for chunk in ids.chunks(50) {
            let rows: Vec<(String, Vec<u8>)> = {
                let db = self.read();
                let mut q = db.prepare("SELECT raw FROM messages WHERE id=?1 AND substr(raw,1,4)<>?2")?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(raw) = q.query_map(params![id, PACKED], |r| r.get::<_, Vec<u8>>(0))?.next() {
                        rows.push((id.clone(), pack(&raw?)));
                    }
                }
                rows
            };
            let mut db = self.write();
            let tx = db.transaction()?;
            for (id, saved) in &rows {
                // Unless it was saved again meanwhile (packed, then).
                packed += tx.execute(
                    "UPDATE messages SET raw=?2 WHERE id=?1 AND substr(raw,1,4)<>?3",
                    params![id, saved, PACKED],
                )?;
            }
            tx.commit()?;
        }
        self.reclaim_space()?;
        Ok(packed)
    }
    /// Gives free pages back to the system. A database made before the
    /// cache freed them is rebuilt once (VACUUM) to start doing so.
    fn reclaim_space(&self) -> Result<()> {
        let db = self.write();
        let mode: i64 = db.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        if mode == 2 {
            // Frees a page per step: run it to the end (execute_batch steps once).
            let mut q = db.prepare("PRAGMA incremental_vacuum")?;
            let mut rows = q.query([])?;
            while rows.next()?.is_some() {}
        } else {
            db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
        }
        // VACUUM writes the whole database through the log: cut it back.
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
    pub fn raw(&self, id: &str) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .query_row("SELECT raw FROM messages WHERE id=?1", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
            .optional()?
            .flatten()
            .map(unpack)
            .transpose()?)
    }
    pub fn save_raw(&self, id: &str, raw: &[u8]) -> Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, pack(raw)])?;
        Ok(())
    }
    /// One accepted remote command is one local transaction. Earlier confirmed
    /// groups remain committed if a later mailbox rejects a bulk operation.
    pub fn commit_changes(&self, messages: &[Mail], delete: bool) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        if delete {
            for mail in messages {
                tx.execute(
                    "DELETE FROM messages WHERE id=?1 AND account=?2",
                    params![mail.id, mail.account],
                )?;
            }
        } else {
            write_messages(&tx, messages)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Messages dated `since` or later whose full content has not been
    /// downloaded yet, newest first.
    pub fn missing(
        &self,
        account: &str,
        max_size: usize,
        limit: usize,
        since: i64,
    ) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT data FROM messages WHERE account=?1 AND raw IS NULL AND date >= ?4
             AND json_extract(data,'$.size') <= ?2 ORDER BY date DESC, id DESC LIMIT ?3",
        )?;
        q.query_map(params![account, max_size, limit, since], |r| {
            r.get::<_, String>(0)
        })?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect()
    }
    /// Messages that download in the background, and how many of those have
    /// not yet, across accounts. `since` gives each account's oldest date;
    /// accounts that download only on open are left out.
    pub fn download_counts(
        &self,
        max_size: usize,
        since: &[(String, i64)],
    ) -> Result<(usize, usize)> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT COUNT(*), COALESCE(SUM(raw IS NULL),0) FROM messages
             WHERE account=?1 AND date >= ?3 AND json_extract(data,'$.size') <= ?2",
        )?;
        let mut totals = (0, 0);
        for (account, since) in since {
            let (total, missing) = q.query_row(params![account, max_size, since], |r| {
                Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize))
            })?;
            totals.0 += total;
            totals.1 += missing;
        }
        Ok(totals)
    }
    /// Copies already-downloaded content to other copies of the same Gmail
    /// message (one per label). Returns how many copies were filled.
    pub fn share_gmail_raw(&self, account: &str) -> Result<usize> {
        // Two passes over the account rather than a correlated subquery: that
        // re-read every message's JSON once per missing copy, which held the
        // database for seconds on a few thousand messages and stalled the UI.
        let mut db = self.write();
        let tx = db.transaction()?;
        let mut downloaded: HashMap<i64, String> = HashMap::new();
        let mut missing: Vec<(String, i64)> = Vec::new();
        {
            let mut q = tx.prepare(
                "SELECT id, json_extract(data,'$.gmail_id'), raw IS NOT NULL FROM messages
                 WHERE account=?1 AND json_extract(data,'$.gmail_id') > 0",
            )?;
            let rows = q.query_map([account], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            for row in rows {
                let (id, gmail_id, has_raw) = row?;
                if has_raw {
                    downloaded.entry(gmail_id).or_insert(id);
                } else {
                    missing.push((id, gmail_id));
                }
            }
        }
        let mut filled = 0;
        {
            let mut copy = tx.prepare(
                "UPDATE messages SET raw=(SELECT raw FROM messages WHERE id=?2)
                 WHERE id=?1 AND raw IS NULL",
            )?;
            for (id, gmail_id) in &missing {
                if let Some(source) = downloaded.get(gmail_id) {
                    filled += copy.execute(params![id, source])?;
                }
            }
        }
        tx.commit()?;
        Ok(filled)
    }
    /// Downloaded content of another copy of a Gmail message, if any.
    pub fn gmail_raw(&self, account: &str, gmail_id: u64) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        if gmail_id == 0 {
            return Ok(None);
        }
        self.read()
            .query_row(
                "SELECT raw FROM messages WHERE account=?1 AND raw IS NOT NULL
                 AND json_extract(data,'$.gmail_id')=?2 LIMIT 1",
                params![account, gmail_id as i64],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(unpack)
            .transpose()
    }
    /// What the last synchronization of a server mailbox saw.
    pub fn mailbox_state(&self, account: &str, remote: &str) -> Result<Option<MailboxState>> {
        use rusqlite::OptionalExtension;
        let data: Option<String> = self
            .read()
            .query_row(
                "SELECT data FROM mailbox_state WHERE account=?1 AND remote=?2",
                params![account, remote],
                |r| r.get(0),
            )
            .optional()?;
        Ok(data.and_then(|d| serde_json::from_str(&d).ok()))
    }
    /// Saved only after the mail it describes has been committed.
    pub fn save_mailbox_state(
        &self,
        account: &str,
        remote: &str,
        state: Option<&MailboxState>,
    ) -> Result<()> {
        let db = self.write();
        match state {
            // Most checks leave it as it was: then nothing is written.
            Some(state) => db.execute(
                "INSERT INTO mailbox_state VALUES (?1,?2,?3) ON CONFLICT(account, remote) DO UPDATE SET data=excluded.data WHERE data IS NOT excluded.data",
                params![account, remote, serde_json::to_string(state)?],
            )?,
            None => db.execute(
                "DELETE FROM mailbox_state WHERE account=?1 AND remote=?2",
                params![account, remote],
            )?,
        };
        Ok(())
    }
    /// Commits one incremental synchronization: new messages, flag changes
    /// for cached ones (by UID, within `prefix`'s generation), and removal of
    /// mail older than the account's sync window (`before`).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_changes(
        &self,
        mailbox: &Mailbox,
        account: &str,
        prefix: &str,
        fresh: &[Mail],
        flags: &[FlagUpdate],
        removed: &[String],
        before: Option<i64>,
    ) -> Result<()> {
        ensure!(
            fresh
                .iter()
                .all(|m| m.account == account && m.id.starts_with(prefix)),
            "Invalid mailbox update"
        );
        let mut db = self.write();
        let tx = db.transaction()?;
        write_messages(&tx, fresh)?;
        for FlagUpdate {
            uid,
            unread,
            starred,
            labels,
            answered,
            forwarded,
            mdn_sent,
        } in flags
        {
            let id = format!("{prefix}{uid}");
            let data: Option<String> = {
                use rusqlite::OptionalExtension;
                tx.query_row("SELECT data FROM messages WHERE id=?1", [&id], |r| r.get(0))
                    .optional()?
            };
            if let Some(data) = data {
                let mut mail: Mail = serde_json::from_str(&data)?;
                let labels_changed = labels.as_ref().is_some_and(|l| *l != mail.labels());
                let marks = (*answered, *forwarded, *mdn_sent);
                if (mail.unread, mail.starred) != (*unread, *starred)
                    || labels_changed
                    || (mail.answered, mail.forwarded, mail.mdn_sent) != marks
                {
                    mail.unread = *unread;
                    mail.starred = *starred;
                    (mail.answered, mail.forwarded, mail.mdn_sent) = marks;
                    if let Some(labels) = labels {
                        mail.set_labels(labels.clone());
                    }
                    write_messages(&tx, &[mail])?;
                }
            }
        }
        // Removed on the server (QRESYNC's VANISHED), within this generation.
        for id in removed.iter().filter(|id| id.starts_with(prefix)) {
            tx.execute(
                "DELETE FROM messages WHERE id=?1 AND account=?2",
                params![id, account],
            )?;
        }
        if let Some(before) = before {
            tx.execute(
                "DELETE FROM messages WHERE account=?1 AND folder=?2 AND date < ?3",
                params![account, mailbox.key(), before],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Sets the server's conversation ids on cached messages (IMAP THREAD).
    pub fn set_thread_ids(&self, ids: &[(String, String)]) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        for (id, thread) in ids {
            let data: Option<String> = {
                use rusqlite::OptionalExtension;
                tx.query_row("SELECT data FROM messages WHERE id=?1", [id], |r| r.get(0))
                    .optional()?
            };
            if let Some(data) = data {
                let mut mail: Mail = serde_json::from_str(&data)?;
                if mail.thread_id() != thread.as_str() {
                    mail.set_thread_id(thread.clone());
                    write_messages(&tx, &[mail])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Deletes cached messages by id (removed on the server).
    pub fn delete_ids(&self, account: &str, ids: &[String]) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        for id in ids {
            tx.execute(
                "DELETE FROM messages WHERE id=?1 AND account=?2",
                params![id, account],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// The labels on an account's cached mail, sorted.
    pub fn labels(&self, account: &str) -> Result<Vec<String>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT DISTINCT l.value FROM messages, json_each(json_extract(messages.data,'$.labels')) AS l
             WHERE messages.account=?1 ORDER BY l.value",
        )?;
        q.query_map([account], |r| r.get::<_, String>(0))?
            .map(|r| Ok(r?))
            .collect()
    }
    /// Unread counts keyed by (account, mailbox key).
    pub fn unread_counts(&self) -> Result<HashMap<(String, String), usize>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT account, folder, COUNT(*) FROM messages
             WHERE json_extract(data,'$.unread') GROUP BY account, folder",
        )?;
        q.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, i64>(2)? as usize,
            ))
        })?
        .map(|r| Ok(r?))
        .collect()
    }
    /// Each unread message's account, mailbox key and recipients.
    pub fn unread_recipients(&self) -> Result<Vec<crate::manage::forwarding::UnreadRow>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT account, folder, coalesce(json_extract(data,'$.to'),''), coalesce(json_extract(data,'$.cc'),'')
             FROM messages WHERE json_extract(data,'$.unread')",
        )?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .map(|r| Ok(r?))
            .collect()
    }
    pub fn folders(&self, account: &str) -> Vec<RemoteFolder> {
        self.preference(&format!("folders:{account}"))
            .and_then(|s| serde_json::from_str::<Vec<RemoteFolder>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            // Lists saved before Gmail's system container was left out.
            .filter(|f| !crate::sync::transport::gmail_system(&f.name))
            .collect()
    }
    /// Forgets a deleted server folder: its cached mail and sync state.
    pub fn forget_mailbox(&self, account: &str, remote: &str) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE account=?1 AND json_extract(data,'$.remote_folder')=?2",
            params![account, remote],
        )?;
        tx.execute(
            "DELETE FROM mailbox_state WHERE account=?1 AND remote=?2",
            params![account, remote],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn save_folders(&self, account: &str, folders: &[RemoteFolder]) -> Result<()> {
        self.set_preference(
            &format!("folders:{account}"),
            &serde_json::to_string(folders)?,
        )
    }
    pub fn save_draft(&self, d: &Draft) -> Result<()> {
        self.write().execute(
            "INSERT OR REPLACE INTO drafts VALUES (?1,?2,?3)",
            params![d.id, d.account, serde_json::to_string(d)?],
        )?;
        Ok(())
    }
    pub fn drafts(&self, account: &str) -> Result<Vec<Draft>> {
        let db = self.read();
        let mut q = db.prepare("SELECT data FROM drafts WHERE account=?1 ORDER BY rowid DESC")?;
        q.query_map([account], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    /// Drafts of every account that are scheduled to be sent (Send Later).
    pub fn scheduled_drafts(&self) -> Result<Vec<Draft>> {
        let db = self.read();
        let mut q = db.prepare("SELECT data FROM drafts WHERE data LIKE '%\"send_at\"%'")?;
        let drafts: Vec<Draft> = q
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        Ok(drafts.into_iter().filter(|d| d.send_at.is_some()).collect())
    }
    pub fn draft(&self, id: &str) -> Result<Option<Draft>> {
        use rusqlite::OptionalExtension;
        let data: Option<String> = self
            .read()
            .query_row("SELECT data FROM drafts WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        Ok(match data {
            Some(data) => Some(serde_json::from_str(&data)?),
            None => None,
        })
    }
    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("DELETE FROM drafts WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn preference(&self, key: &str) -> Option<String> {
        if let Some(value) = self
            .prefs
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Some(value.clone());
        }
        self.read()
            .query_row("SELECT value FROM preferences WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .ok()
    }
    /// Writes a preference now, after any set to be written soon.
    pub fn set_preference(&self, key: &str, value: &str) -> Result<()> {
        self.flush();
        self.write().execute(
            PUT_PREFERENCE,
            params![key, value],
        )?;
        Ok(())
    }
    /// Sets a preference without waiting for the cache, which a long
    /// synchronization can hold for a while: for the interface, which must
    /// not stall. It reads back at once and is written in the background;
    /// `preference_saved` with the returned ticket says how that went.
    pub fn set_preference_soon(&self, key: &str, value: &str) -> u64 {
        let ticket = {
            let mut state = self.prefs.state.lock().unwrap_or_else(|e| e.into_inner());
            state.next_ticket += 1;
            let ticket = state.next_ticket;
            state.latest.insert(key.to_owned(), ticket);
            state.pending.insert(key.to_owned(), value.to_owned());
            ticket
        };
        let sent = self
            .pref_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|tx| tx.send(()).is_ok());
        if !sent {
            // No writer: write it here instead.
            let value = self.prefs.state.lock().unwrap().pending.remove(key);
            if let Some(value) = value {
                let result = self.set_preference(key, &value);
                let mut state = self.prefs.state.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => state.failed.remove(key),
                    Err(e) => state.failed.insert(key.to_owned(), format!("{e:#}")),
                };
                self.prefs.idle.notify_all();
            }
        }
        ticket
    }
    /// Waits until the preference set with `ticket` is written, and says
    /// whether it could be. A ticket since replaced by a newer value of the
    /// key answers `Ok` at once: the newer one's outcome is what counts.
    pub fn preference_saved(&self, key: &str, ticket: u64) -> Result<()> {
        let mut state = self.prefs.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.latest.get(key) != Some(&ticket) {
                return Ok(());
            }
            if !state.busy(key) {
                return match state.failed.get(key) {
                    Some(why) => Err(anyhow::anyhow!("Couldn't save this setting: {why}")),
                    None => Ok(()),
                };
            }
            let (next, timeout) = self
                .prefs
                .idle
                .wait_timeout(state, BUSY_TIMEOUT)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
            if timeout.timed_out() && state.busy(key) {
                anyhow::bail!("Couldn't save this setting: the cache stayed busy");
            }
        }
    }
}
#[cfg(test)]
impl Store {
    /// Makes every later preference write fail, as a full disk would.
    pub fn fail_preference_writes_for_test(&self) {
        self.db
            .lock()
            .unwrap()
            .execute_batch("CREATE TRIGGER fail_pref BEFORE INSERT ON preferences BEGIN SELECT RAISE(ABORT,'disk full'); END;")
            .unwrap();
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // Closing the channel ends the writer once the queue is written.
        self.pref_tx.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(thread) = self
            .pref_thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }
}
/// The preference writer: writes whatever is pending each time it is told
/// there is something, until the store closes.
/// Saved message sources are kept compressed (raw deflate) behind this
/// marker, a quarter of their size on disk. A source without it was saved
/// before they were, and reads as it is: no message starts with a NUL.
const PACKED: &[u8] = b"\0PZ1";
/// No source is larger than a download may be; a damaged one can't
/// inflate past this.
const MAX_SOURCE: u64 = 2 * crate::sync::transport::MAX_MESSAGE as u64;

/// A message source as it is saved: compressed, behind `PACKED`.
fn pack(raw: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut out = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(PACKED.len() + raw.len() / 3),
        flate2::Compression::default(),
    );
    // Writing to memory can't fail.
    let _ = out.get_mut().write_all(PACKED);
    let _ = out.write_all(raw);
    out.finish().unwrap_or_default()
}
/// A saved source as it was downloaded; at most its first `limit` bytes.
fn unpack_up_to(saved: Vec<u8>, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let Some(packed) = saved.strip_prefix(PACKED) else {
        let mut saved = saved;
        saved.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        return Ok(saved);
    };
    let mut raw = Vec::with_capacity(packed.len() * 3);
    flate2::read::DeflateDecoder::new(packed)
        .take(limit)
        .read_to_end(&mut raw)
        .context("A saved message source is damaged")?;
    Ok(raw)
}
fn unpack(saved: Vec<u8>) -> Result<Vec<u8>> {
    unpack_up_to(saved, MAX_SOURCE)
}

/// Saves a preference, writing nothing when it already has that value
/// (every check saves its account's folder list, nearly always unchanged).
const PUT_PREFERENCE: &str = "INSERT INTO preferences VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value";

fn write_preferences(db: Connection, queue: &PrefQueue, rx: mpsc::Receiver<()>) {
    let write = |batch: &HashMap<String, String>| -> Result<()> {
        let db = db.unchecked_transaction()?;
        for (key, value) in batch {
            db.execute(
                PUT_PREFERENCE,
                params![key, value],
            )?;
        }
        db.commit()?;
        Ok(())
    };
    let mut open = true;
    while open {
        open = rx.recv().is_ok();
        // Anything queued meanwhile goes in the same transaction.
        while rx.try_recv().is_ok() {}
        let batch = {
            let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
            let batch = std::mem::take(&mut state.pending);
            state.writing = batch.clone();
            batch
        };
        let mut result = Ok(());
        if !batch.is_empty() {
            // Retried, as an older value must not come back on the next launch.
            for attempt in 0..3 {
                result = write(&batch);
                if result.is_ok() {
                    break;
                }
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
        for key in batch.keys() {
            match &result {
                Ok(()) => state.failed.remove(key),
                Err(e) => state.failed.insert(key.clone(), format!("{e:#}")),
            };
        }
        state.writing.clear();
        // Waiters for a single key as well as for the whole queue.
        queue.idle.notify_all();
    }
}
// The service name predates the Picacho name; keep it so saved credentials
// stay readable.
const SECRET_SERVICE: &str = "com.pinal.native-mail";
#[cfg(not(target_os = "ios"))]
pub fn secret_entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SECRET_SERVICE, id).context("Cannot access the system keychain")
}
/// iOS keeps credentials in the app's own Keychain (Protected Data).
#[cfg(target_os = "ios")]
pub fn secret_entry(id: &str) -> Result<keyring_core::Entry> {
    static STORE: std::sync::LazyLock<std::result::Result<(), String>> =
        std::sync::LazyLock::new(|| {
            apple_native_keyring_store::protected::Store::new()
                .map(|store| keyring_core::set_default_store(store))
                .map_err(|e| e.to_string())
        });
    if let Err(e) = &*STORE {
        anyhow::bail!("Cannot access the system keychain: {e}");
    }
    keyring_core::Entry::new(SECRET_SERVICE, id).context("Cannot access the system keychain")
}
pub fn read_secret(id: &str) -> Result<Secret> {
    serde_json::from_str(
        &secret_entry(id)?
            .get_password()
            .context("The keychain entry is missing, locked, or access was denied")?,
    )
    .context("Saved credentials could not be read")
}
pub fn write_secret(id: &str, s: &Secret) -> Result<()> {
    secret_entry(id)?
        .set_password(&serde_json::to_string(s)?)
        .context("Could not securely save credentials to the system keychain")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gmail_copies_share_downloaded_content() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        let mut ms = demo_mail();
        // Two labels of one message, a second message, and one without an ID.
        for (m, gmail_id) in ms.iter_mut().zip([7, 7, 7, 8, 0]) {
            m.gmail_id = gmail_id;
        }
        s.save_messages(&ms[..5])?;
        s.save_raw(&ms[0].id, b"seven")?;
        assert_eq!(s.share_gmail_raw("demo")?, 2);
        assert_eq!(s.raw(&ms[1].id)?, Some(b"seven".to_vec()));
        assert_eq!(s.raw(&ms[2].id)?, Some(b"seven".to_vec()));
        assert_eq!(s.raw(&ms[3].id)?, None);
        assert_eq!(s.raw(&ms[4].id)?, None);
        // Other accounts never receive this one's content.
        assert_eq!(s.share_gmail_raw("other")?, 0);
        assert_eq!(s.share_gmail_raw("demo")?, 0);
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    #[test]
    fn preferences_set_soon_read_back_at_once_and_reach_the_cache() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        // The writer is held, as a long synchronization would.
        let held = s.db.lock().unwrap();
        held.execute_batch("BEGIN IMMEDIATE;")?;
        s.set_preference_soon("sort", "oldest");
        s.set_preference_soon("sort", "newest");
        assert_eq!(s.preference("sort").as_deref(), Some("newest"));
        // Reading mail doesn't wait for the writer either.
        assert!(s.messages("demo", Folder::Inbox, 10)?.is_empty());
        held.execute_batch("COMMIT;")?;
        drop(held);
        s.flush();
        drop(s);
        let s = Store::open(&p)?;
        assert_eq!(s.preference("sort").as_deref(), Some("newest"));
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    #[test]
    fn preferences_that_cannot_be_saved_say_why() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        s.set_preference("sort", "newest")?;
        let ok = s.set_preference_soon("theme", "dark");
        assert!(s.preference_saved("theme", ok).is_ok());
        s.fail_preference_writes_for_test();
        let older = s.set_preference_soon("sort", "oldest");
        let newer = s.set_preference_soon("sort", "oldest");
        let e = s.preference_saved("sort", newer).unwrap_err();
        assert!(format!("{e:#}").contains("Couldn't save this setting"), "{e:#}");
        // Only the latest change of a key reports, so a failure shows once.
        assert!(s.preference_saved("sort", older).is_ok());
        // The value saved before is what reads back.
        assert_eq!(s.preference("sort").as_deref(), Some("newest"));
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    #[test]
    fn all_mail_lists_every_folder_once_without_drafts() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        let mail = |id: &str, folder: Folder, remote: &str, message_id: &str, date: i64| Mail {
            id: id.into(),
            account: "a".into(),
            folder,
            remote_folder: remote.into(),
            message_id: message_id.into(),
            date,
            ..Mail::default()
        };
        s.save_messages(&[
            mail("a:INBOX:1:1", Folder::Inbox, "INBOX", "<1@x>", 5),
            mail("a:Sent:1:1", Folder::Sent, "Sent", "<2@x>", 4),
            mail("a:Trash:1:1", Folder::Trash, "Trash", "<3@x>", 3),
            mail("a:Projects:1:1", Folder::Other, "Projects", "<4@x>", 2),
            // The same message under another label is listed once.
            mail("a:All:1:1", Folder::Archive, "[Gmail]/All Mail", "<1@x>", 5),
            mail("a:Drafts:1:1", Folder::Drafts, "Drafts", "<5@x>", 6),
            Mail {
                account: "b".into(),
                ..mail("b:INBOX:1:1", Folder::Inbox, "INBOX", "<6@x>", 7)
            },
        ])?;
        let all = s.summaries("a", Folder::All)?;
        let ids: Vec<&str> = all.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, ["<1@x>", "<2@x>", "<3@x>", "<4@x>"]);
        // Each keeps its real place, so its actions apply where it is.
        assert_eq!(all[2].folder, Folder::Trash);
        assert_eq!(s.messages("a", Folder::All, 2)?.len(), 2);
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    #[test]
    fn cache_roundtrip_and_account_isolation() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        let mut ms = demo_mail();
        s.save_messages(&ms)?;
        assert_eq!(s.messages("demo", Folder::Inbox, 300)?.len(), 10);
        assert!(s.messages("other", Folder::Inbox, 300)?.is_empty());
        ms[0].unread = false;
        s.save_messages(&ms[..1])?;
        s.save_raw(&ms[0].id, b"raw")?;
        assert_eq!(s.raw(&ms[0].id)?, Some(b"raw".to_vec()));
        assert_eq!(s.messages("demo", Folder::Starred, 300)?.len(), 2);
        let everything = [("demo".to_owned(), i64::MIN)];
        assert_eq!(s.download_counts(1 << 20, &everything)?, (10, 9));
        assert_eq!(s.missing("demo", 1 << 20, 100, i64::MIN)?.len(), 9);
        // Only mail inside an account's offline range downloads ahead.
        let newest = ms.iter().map(|m| m.date).max().unwrap();
        let recent = [("demo".to_owned(), newest)];
        assert!(s.download_counts(1 << 20, &recent)?.0 < 10);
        assert!(s.missing("demo", 1 << 20, 100, newest)?.len() < 9);
        assert_eq!(s.download_counts(1 << 20, &[])?, (0, 0));
        assert_eq!(s.unread_counts()?[&("demo".into(), "inbox".into())], 2);
        let mut other = ms[1].clone();
        other.id = "remote-1".into();
        other.folder = Folder::Other;
        other.remote_folder = "Projects/2026".into();
        s.save_messages(&[other])?;
        let remote = Mailbox::Remote("Projects/2026".into());
        assert_eq!(s.messages("demo", &remote, 300)?.len(), 1);
        assert_eq!(s.messages("demo", Folder::Inbox, 300)?.len(), 10);
        let folders = vec![RemoteFolder {
            name: "Projects/2026".into(),
            label: "2026".into(),
            depth: 1,
            nestable: true,
        }];
        s.save_folders("demo", &folders)?;
        assert_eq!(s.folders("demo"), folders);
        s.reconcile("demo", Folder::Inbox, &[ms[0].id.clone()])?;
        assert_eq!(s.messages("demo", Folder::Inbox, 300)?.len(), 1);
        let d = Draft {
            id: "draft".into(),
            account: "demo".into(),
            body: "unsent".into(),
            ..Default::default()
        };
        s.save_draft(&d)?;
        assert_eq!(s.drafts("demo")?[0].body, "unsent");
        s.delete_draft("draft")?;
        let reading = crate::manage::reading::message_key("demo", "demo-0");
        let other_reading = crate::manage::reading::message_key("other", "demo-0");
        s.set_preference(&reading, "{}")?;
        s.set_preference(&other_reading, "{}")?;
        s.set_preference("reading_defaults", "{}")?;
        s.remove_account("demo")?;
        assert!(s.preference(&reading).is_none());
        assert!(s.preference(&other_reading).is_some());
        assert!(s.preference("reading_defaults").is_some());
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
}

/// Inputs validated before a mailbox snapshot is committed.
/// A cached message's state as the server reports it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagUpdate {
    pub uid: u32,
    pub unread: bool,
    pub starred: bool,
    /// Its labels, when the server said (`None` leaves them as they were).
    pub labels: Option<Vec<String>>,
    pub answered: bool,
    pub forwarded: bool,
    pub mdn_sent: bool,
}
/// A server mailbox as of its last synchronization: enough to ask only for
/// what changed since (new UIDs, CONDSTORE flag changes, removals).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MailboxState {
    pub validity: u32,
    pub uid_next: u32,
    pub exists: u32,
    #[serde(default)]
    pub modseq: Option<u64>,
    /// The account's sync window (days) these results used.
    pub days: u32,
}
pub struct MailboxSnapshot<'a> {
    pub account: &'a str,
    pub mailbox: &'a Mailbox,
    pub remote: &'a str,
    pub validity: u32,
    pub ids: &'a [String],
    pub starred_only: bool,
}
fn write_messages(tx: &rusqlite::Transaction<'_>, messages: &[Mail]) -> Result<()> {
    for m in messages {
        tx.execute("INSERT INTO messages(id,account,folder,date,data) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET folder=excluded.folder,date=excluded.date,data=excluded.data WHERE folder IS NOT excluded.folder OR date IS NOT excluded.date OR data IS NOT excluded.data",params![m.id,m.account,m.mailbox().key(),m.date,serde_json::to_string(m)?])?;
    }
    Ok(())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::sync::transport::parse_message;

    /// Sources are saved compressed and read back as downloaded; ones saved
    /// before still read as they are, until the one-time pass compresses
    /// them and gives the freed space back.
    #[test]
    fn sources_are_kept_compressed_and_read_back_whole() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-packed-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Gmail);
        let raw = |i: usize| {
            let mut raw = format!("Subject: Hi {i}\r\nList-Unsubscribe: <https://x/u>\r\n\r\n").into_bytes();
            raw.extend(b"The same paragraph, again and again. ".repeat(20_000));
            raw
        };
        let saved = |id: &str| -> Result<Vec<u8>> {
            Ok(store.read().query_row("SELECT raw FROM messages WHERE id=?1", [id], |r| r.get(0))?)
        };
        let mut ids = Vec::new();
        for i in 0..6 {
            let id = format!("{}:{i}", a.id);
            let mut m = parse_message(&raw(i), &a, &id, Folder::Inbox, "INBOX", i as u32)?;
            m.gmail_id = 100 + i as u64;
            m.list_unsubscribe.clear();
            store.save_messages(std::slice::from_ref(&m))?;
            ids.push(id);
        }
        // Saved now: compressed, and read back as it was.
        store.save_raw(&ids[0], &raw(0))?;
        assert!(saved(&ids[0])?.starts_with(PACKED));
        assert!(saved(&ids[0])?.len() < raw(0).len() / 10);
        assert_eq!(store.raw(&ids[0])?, Some(raw(0)));
        assert_eq!(store.gmail_raw(&a.id, 100)?, Some(raw(0)));
        // Saved before: as it is, and still read.
        for (i, id) in ids.iter().enumerate().skip(1) {
            store.write().execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, raw(i)])?;
        }
        assert_eq!(store.raw(&ids[1])?, Some(raw(1)));
        // Headers are read from either kind.
        assert_eq!(store.backfill_subscriptions()?, 6);
        store.write().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let before = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert_eq!(store.pack_saved_sources()?, 5);
        for (i, id) in ids.iter().enumerate() {
            assert!(saved(id)?.starts_with(PACKED));
            assert_eq!(store.raw(id)?, Some(raw(i)));
        }
        let mode: i64 = store.read().query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        assert_eq!(mode, 2, "frees pages from now on");
        let after = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(after < before / 2, "{before} -> {after}");
        assert_eq!(store.pack_saved_sources()?, 0, "once");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// Saving what is already saved (each check's folder list and mailbox
    /// state) writes nothing to the disk; a change is still saved.
    #[test]
    fn unchanged_saves_write_nothing() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-noop-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let state = MailboxState { validity: 1, uid_next: 9, exists: 8, modseq: Some(3), days: 0 };
        store.set_preference("folders:a", "[]")?;
        store.save_mailbox_state("a", "INBOX", Some(&state))?;
        let wal = path.join("mail.sqlite3-wal");
        let before = std::fs::metadata(&wal)?.len();
        for _ in 0..50 {
            store.set_preference("folders:a", "[]")?;
            store.save_mailbox_state("a", "INBOX", Some(&state))?;
        }
        assert_eq!(std::fs::metadata(&wal)?.len(), before, "nothing written");
        store.set_preference("folders:a", "[1]")?;
        let next = MailboxState { uid_next: 10, ..state };
        store.save_mailbox_state("a", "INBOX", Some(&next))?;
        assert_eq!(store.preference("folders:a").as_deref(), Some("[1]"));
        assert_eq!(store.mailbox_state("a", "INBOX")?.map(|s| s.uid_next), Some(10));
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// The most frequent counts read their partial indexes, rather than
    /// every message's JSON: their WHERE must match the indexes' exactly.
    #[test]
    fn frequent_counts_use_their_indexes() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-plans-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let plan = |sql: &str| -> Result<String> {
            let db = store.read();
            let mut q = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let rows: Vec<String> = q.query_map([], |r| r.get(3))?.collect::<rusqlite::Result<_>>()?;
            Ok(rows.join("; "))
        };
        for (sql, index) in [
            ("SELECT account, folder, COUNT(*) FROM messages WHERE json_extract(data,'$.unread') GROUP BY account, folder", "unread"),
            ("SELECT data FROM messages WHERE account='a' AND json_extract(data,'$.list_unsubscribe') IS NOT NULL", "subscribed"),
            ("SELECT COUNT(*), COALESCE(SUM(raw IS NULL),0) FROM messages WHERE account='a' AND date >= 0 AND json_extract(data,'$.size') <= 9", "COVERING INDEX downloads"),
            ("SELECT data FROM messages WHERE account='a' AND raw IS NULL AND date >= 0 AND json_extract(data,'$.size') <= 9 ORDER BY date DESC, id DESC LIMIT 5", "INDEX downloads"),
        ] {
            let plan = plan(sql)?;
            let index = if index.contains("INDEX") { index.to_owned() } else { format!("INDEX {index}") };
            assert!(plan.contains(&index), "{sql}: {plan}");
        }
        // The queries themselves, as the store runs them, still answer.
        store.unread_counts()?;
        store.subscriptions(&["a".into()])?;
        store.download_counts(9, &[("a".into(), 0)])?;
        store.missing("a", 9, 5, 0)?;
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// Mail downloaded without its List-Unsubscribe kept (Proton's, before)
    /// gets it from its saved source, for the accounts asked about only.
    #[test]
    fn subscriptions_are_found_in_saved_sources_of_the_accounts_asked() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-backfill-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let raw = b"From: News <news@shop.example>\r\nSubject: Sale\r\nList-Unsubscribe: <https://shop.example/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\nBody";
        let accounts = [Account::new(Provider::Proton), Account::new(Provider::Proton)];
        for a in &accounts {
            let id = format!("{}:1", a.id);
            let mut m = parse_message(raw, a, &id, Folder::Inbox, "INBOX", 1)?;
            // As the download used to save it: without the header.
            m.list_unsubscribe.clear();
            m.one_click = false;
            store.save_messages(std::slice::from_ref(&m))?;
            store.save_raw(&id, raw)?;
        }
        assert!(store.subscriptions(&[accounts[0].id.clone()])?.is_empty());
        assert_eq!(store.backfill_subscriptions_in(&[accounts[0].id.clone()])?, 1);
        let found = store.subscriptions(&[accounts[0].id.clone()])?;
        assert_eq!(found.len(), 1);
        assert!(found[0].one_click);
        assert!(store.subscriptions(&[accounts[1].id.clone()])?.is_empty(), "not asked about");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// Lists are rebuilt only when what they show changed: saving mail as
    /// it already is (a check with no news) keeps the revision; new mail,
    /// a changed flag, a removal or a rolled-back write moves it on.
    /// Writes elsewhere (raw sources, preferences, another account's mail)
    /// don't.
    #[test]
    fn list_revision_follows_changes_to_listed_mail() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-revision-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        let id = format!("{}:INBOX:1:7", a.id);
        let mail = parse_message(b"Subject: Hi\r\n\r\nBody", &a, &id, Folder::Inbox, "INBOX", 7)?;
        let other = Account::new(Provider::Imap);
        let other_id = format!("{}:INBOX:1:1", other.id);
        let other_mail = parse_message(b"Subject: Other\r\n\r\nBody", &other, &other_id, Folder::Inbox, "INBOX", 1)?;
        store.save_messages(std::slice::from_ref(&other_mail))?;
        let other_revision = store.list_revision(&other.id);
        assert!(other_revision > 0);
        let start = store.list_revision(&a.id);
        store.save_messages(std::slice::from_ref(&mail))?;
        let saved = store.list_revision(&a.id);
        assert!(saved > start, "new mail");
        assert_eq!(store.list_revision(&other.id), other_revision, "another account's mail");
        store.save_messages(std::slice::from_ref(&mail))?;
        assert_eq!(store.list_revision(&a.id), saved, "the same mail again");
        store.save_raw(&id, b"Subject: Hi\r\n\r\nBody")?;
        store.set_preference("x", "y")?;
        assert_eq!(store.list_revision(&a.id), saved, "not what lists show");
        let read = Mail { unread: !mail.unread, ..mail.clone() };
        store.save_messages(std::slice::from_ref(&read))?;
        let flagged = store.list_revision(&a.id);
        assert!(flagged > saved, "a changed flag");
        assert_eq!(store.summaries(&a.id, Folder::Inbox)?[0].unread, read.unread);
        store.delete_ids(&a.id, std::slice::from_ref(&id))?;
        let deleted = store.list_revision(&a.id);
        assert!(deleted > flagged, "a removal");
        // A failed write may have changed rows before it rolled back.
        store.write().execute_batch("CREATE TRIGGER fail_insert AFTER INSERT ON messages BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
        assert!(store.save_messages(std::slice::from_ref(&mail)).is_err());
        assert!(store.list_revision(&a.id) > deleted, "a rolled-back write");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    #[test]
    fn failed_snapshot_transaction_restores_removed_messages_and_raw_data() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-atomic-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        let remote = "受信箱:Projects";
        let mailbox = Mailbox::Remote(remote.into());
        let old_id = format!("{}:{remote}:42:7", a.id);
        let old = parse_message(
            b"Subject: Old\r\n\r\nOld body",
            &a,
            &old_id,
            Folder::Other,
            remote,
            7,
        )?;
        store.save_download(&old, b"original raw")?;
        let id = format!("{}:{remote}:43:9", a.id);
        let next = parse_message(
            b"Subject: New\r\n\r\nNew body",
            &a,
            &id,
            Folder::Other,
            remote,
            9,
        )?;
        let ids = vec![id];
        let snapshot = MailboxSnapshot {
            account: &a.id,
            mailbox: &mailbox,
            remote,
            validity: 43,
            ids: &ids,
            starred_only: false,
        };
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;")?;
        assert!(
            store
                .apply_snapshot(&snapshot, std::slice::from_ref(&next))
                .is_err()
        );
        assert_eq!(store.raw(&old_id)?.unwrap(), b"original raw");
        assert!(store.message(&old_id)?.is_some());
        store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_insert;")?;
        store.apply_snapshot(&snapshot, &[next])?;
        assert!(store.message(&old_id)?.is_none());
        assert_eq!(store.messages(&a.id, mailbox, 300)?.len(), 1);
        drop(store);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
    #[test]
    fn failed_raw_write_never_commits_a_pop3_download_identity() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("picacho-pop-atomic-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Pop3);
        let mail = parse_message(
            b"Subject: Test\r\n\r\nbody",
            &a,
            "pop-id",
            Folder::Inbox,
            "INBOX",
            1,
        )?;
        store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_raw BEFORE UPDATE OF raw ON messages BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;")?;
        assert!(store.save_download(&mail, b"raw").is_err());
        assert!(store.message(&mail.id)?.is_none());
        drop(store);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
