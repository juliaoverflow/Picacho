use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
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
    // Per connection, as SQLite keeps it: the preference writer's ran with
    // FULL, a disk flush on each of its many small writes. In WAL mode,
    // NORMAL loses no committed data on a crash of the app.
    db.pragma_update(None, "synchronous", "NORMAL")?;
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
        // The same on Windows: the folder's access list names only this
        // user. Best effort, as some file systems (FAT, network shares)
        // keep no access lists.
        #[cfg(windows)]
        if let Err(e) = crate::display::windows::restrict_to_user(path) {
            eprintln!("Picacho: couldn't limit the data directory to this user: {e}");
        }
        let file = path.join("mail.sqlite3");
        let mut db = connect(&file)?;
        // Free pages go back to the system on `incremental_vacuum` (see
        // `pack_saved_sources`). Takes effect for a new database; an older
        // one is switched by the VACUUM of `compact_on_quit`.
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
            CREATE TABLE IF NOT EXISTS drafts (id TEXT PRIMARY KEY, account TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS mailbox_state (account TEXT NOT NULL, remote TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (account, remote));
            -- What each message was found to say about parcels (see `parcels`).
            CREATE TABLE IF NOT EXISTS parcel_scan (id TEXT PRIMARY KEY, account TEXT NOT NULL, state INTEGER NOT NULL, version INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS parcel_scan_account ON parcel_scan(account);
            -- What each message was found to say about receipts (see `receipts`).
            CREATE TABLE IF NOT EXISTS receipt_scan (id TEXT PRIMARY KEY, account TEXT NOT NULL, state INTEGER NOT NULL, version INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS receipt_scan_account ON receipt_scan(account);
            -- Changes made offline, one row each, in order (see `offline.rs`):
            -- the whole queue was one preference, read and written again for
            -- every change.
            CREATE TABLE IF NOT EXISTS offline_changes (seq INTEGER PRIMARY KEY AUTOINCREMENT, account TEXT NOT NULL, messages INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS offline_changes_account ON offline_changes(account, seq);")?;
        // Compressed sources get a column of their own (see `pack`), so a
        // build from before them finds `raw` empty and downloads the
        // message again, rather than reading compressed bytes as mail.
        let has_source: i64 = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='source'",
            [],
            |r| r.get(0),
        )?;
        if has_source == 0 {
            db.execute_batch("ALTER TABLE messages ADD COLUMN source BLOB;")?;
        }
        db.execute_batch(&format!(
            // What background downloads count and pick, without reading
            // rows: download progress is counted up to every 400 ms while
            // mail downloads. (`downloads` was an earlier form of it.)
            "DROP INDEX IF EXISTS downloads;
             CREATE INDEX IF NOT EXISTS downloaded ON messages(account, date, json_extract(data,'$.size'), {NOT_DOWNLOADED});
             -- Copies of one Gmail message (one per label), for sharing a
             -- downloaded source between them (`share_gmail_raw`, `gmail_raw`).
             CREATE INDEX IF NOT EXISTS gmail ON messages(account, {GMAIL_ID}) WHERE {GMAIL_ID} > 0;
             -- A server mailbox's rows, for replacing or forgetting them
             -- (under the write lock, which a scan of the account held).
             CREATE INDEX IF NOT EXISTS remote_folder ON messages(account, json_extract(data,'$.remote_folder'));"
        ))?;
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
        for key in [
            format!("pop_erased:{id}"),
            format!("pop_downloaded:{id}"),
            // Changes and draft removals waiting for a connection that
            // will never come, the picture, and traffic and folder order.
            format!("offline_changes:{id}"),
            format!("draft_removals:{id}"),
            format!("avatar:{id}"),
            format!("avatar_source:{id}"),
            format!("traffic:{id}"),
            format!("folder_order:{id}"),
            format!("jmap_submissions:{id}"),
        ] {
            tx.execute("DELETE FROM preferences WHERE key=?1", [key])?;
        }
        // Each JMAP mailbox's state (`jmap_state:<account>:<mailbox>`); by
        // prefix with substr, as an id may hold LIKE's wildcards.
        let state = format!("jmap_state:{id}:");
        tx.execute(
            "DELETE FROM preferences WHERE substr(key,1,length(?1))=?1",
            [&state],
        )?;
        tx.execute(
            "DELETE FROM preferences WHERE substr(key,1,8)='reading:' AND json_valid(substr(key,9)) AND json_extract(substr(key,9),'$[0]')=?1", [id],
        )?;
        for table in [
            "accounts",
            "messages",
            "drafts",
            "mailbox_state",
            "parcel_scan",
            "receipt_scan",
            "offline_changes",
        ] {
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
    /// The newest `limit` cached messages of a mailbox, bodies included.
    #[cfg(test)]
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
        // A range of the primary key (ids sort bytewise): only the matching
        // rows are visited, not every message of the account.
        let mut q = db.prepare_cached(
            "SELECT id FROM messages WHERE id >= ?2 AND id < ?3 AND account=?1",
        )?;
        let end = format!("{prefix}\u{10FFFF}");
        let ids = q
            .query_map(params![account, prefix, end], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
    /// As `cached_ids`, only those whose whole source is saved here.
    pub fn downloaded_ids(
        &self,
        account: &str,
        prefix: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let db = self.read();
        let mut q = db.prepare_cached(&format!(
            "SELECT id FROM messages WHERE id >= ?2 AND id < ?3 AND account=?1 AND {DOWNLOADED}"
        ))?;
        let end = format!("{prefix}\u{10FFFF}");
        let ids = q
            .query_map(params![account, prefix, end], |r| r.get(0))?
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
        let which = virtual_rows(folder).unwrap_or("folder=?2");
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
                // Rows come newest first: the rest would only be cut off.
                // (Virtual mailboxes ask SQLite for every row, as some are
                // left out above; reading and parsing them all for All Mail
                // cost as much as the account's whole cache.)
                if result.len() >= limit {
                    break;
                }
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
    /// Marks every cached message in `mailbox` read, in the database itself:
    /// reading each whole (bodies included) only to flip one flag cost as
    /// much memory as the mailbox's text.
    pub fn mark_mailbox_read(&self, account: &str, mailbox: impl Into<Mailbox>) -> Result<()> {
        let mailbox = mailbox.into();
        // Starred and All Mail gather rows of other mailboxes; they are
        // marked in place too, not read whole and written back. Every copy
        // of a message (Gmail's labels) is marked, as it is on the server.
        let which = virtual_rows(mailbox.folder()).unwrap_or("folder=?2");
        // The same condition as the `unread` index, so it finds the rows.
        self.write().execute(
            &format!("UPDATE messages SET data=json_set(data,'$.unread',json('false')) WHERE account=?1 AND {which} AND json_extract(data,'$.unread')"),
            params![account, mailbox.key()],
        )?;
        Ok(())
    }
    /// The unread messages of `mailbox`, without bodies, newest first:
    /// what marking it read changes. Read mail isn't read at all.
    pub fn unread_summaries(&self, account: &str, mailbox: impl Into<Mailbox>) -> Result<Vec<Mail>> {
        let mailbox = mailbox.into();
        let which = virtual_rows(mailbox.folder()).unwrap_or("folder=?2");
        let db = self.read();
        let mut q = db.prepare(&format!(
            "SELECT json_remove(data,'$.body','$.html') FROM messages WHERE account=?1 AND {which} AND json_extract(data,'$.unread') ORDER BY date DESC, id DESC"
        ))?;
        let mut rows = q.query(params![account, mailbox.key()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(serde_json::from_str(row.get_ref(0)?.as_str()?)?);
        }
        Ok(out)
    }
    /// How many messages `mailbox` has cached, counted by the database
    /// rather than by reading them all.
    pub fn count_messages(&self, account: &str, mailbox: impl Into<Mailbox>) -> Result<usize> {
        let mailbox = mailbox.into();
        if mailbox.folder().is_virtual() {
            // Starred and All Mail count each message once, as they list it.
            return Ok(self.summaries(account, mailbox)?.len());
        }
        let n: i64 = self.read().query_row(
            "SELECT COUNT(*) FROM messages WHERE account=?1 AND folder=?2",
            params![account, mailbox.key()],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
    /// The ids of `mailbox`'s unread cached messages, from the `unread`
    /// index, without reading the messages.
    pub fn unread_ids(&self, account: &str, mailbox: impl Into<Mailbox>) -> Result<Vec<String>> {
        let mailbox = mailbox.into();
        if mailbox.folder().is_virtual() {
            return Ok(self
                .summaries(account, mailbox)?
                .into_iter()
                .filter(|m| m.unread)
                .map(|m| m.id)
                .collect());
        }
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT id FROM messages WHERE account=?1 AND folder=?2 AND json_extract(data,'$.unread')",
        )?;
        let ids = q
            .query_map(params![account, mailbox.key()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
    /// Moves the cached rows of server folder `old` to `new` (a rename),
    /// in the database itself rather than reading each message whole.
    pub fn rename_remote(&self, account: &str, old: &str, new: &str) -> Result<()> {
        self.write().execute(
            "UPDATE messages SET folder=?3, data=json_set(data,'$.remote_folder',?4) WHERE account=?1 AND folder=?2",
            params![
                account,
                Mailbox::Remote(old.to_owned()).key(),
                Mailbox::Remote(new.to_owned()).key(),
                new
            ],
        )?;
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
            "UPDATE messages SET raw=NULL, source=?2 WHERE id=?1",
            params![mail.id, pack(raw)?],
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
                    tx.prepare_cached("DELETE FROM messages WHERE id=?1")?.execute([id])?;
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
        // Counted as read: every message from a list was held at once
        // before, tens of thousands in a large mailbox.
        let mut grouping = crate::manage::subscriptions::Grouping::default();
        for account in accounts {
            let mut rows = q.query([account])?;
            while let Some(row) = rows.next()? {
                if let Ok(mail) = serde_json::from_str::<Mail>(row.get_ref(0)?.as_str()?) {
                    grouping.add(&mail);
                }
            }
        }
        Ok(grouping.finish())
    }
    /// The text of messages, lowercase, for searching the Summary's
    /// packages by what their emails say: by message id, as `parcels`
    /// reads them (the kept text, the HTML's text, or the downloaded
    /// source's; else the preview), up to `PARCEL_SEARCH_TEXT` characters
    /// each. Encrypted mail gives only its preview. Read on several
    /// threads, a batch at a time.
    pub fn parcel_search_texts(&self, ids: &[String]) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        self.source_batches(ids.iter().map(String::as_str), PARCEL_BATCH, |rows| {
            let texts = parallel_map(rows, |(id, data, raw, source)| {
                let raw = unpack_scan(raw, source);
                let text = serde_json::from_str::<Mail>(&data).ok().map(|mail| {
                    let read = ParcelText::of(mail, raw.as_deref());
                    let text: String = read.text().chars().take(PARCEL_SEARCH_TEXT).collect();
                    text.to_lowercase()
                });
                (id, text)
            });
            out.extend(texts.into_iter().filter_map(|(id, text)| Some((id, text?))));
            Ok(())
        })?;
        Ok(out)
    }
    /// Every parcel `accounts`' cached mail tells of, outside Sent, Drafts
    /// and Spam, last updated first (see `parcels::summarize`), and the
    /// shipping mail known only by its headers, to download (newest first).
    ///
    /// What was found in each message is kept in `parcel_scan`, with
    /// whether its text had been downloaded and how it was read, so a
    /// message is read again only once it is downloaded, or the way of
    /// reading (`PARCEL_SCAN`, the model) changes. Messages whose text
    /// wasn't kept are read from their downloaded source, or from their
    /// HTML's text; encrypted ones, and those not downloaded, only by their
    /// subject and preview.
    pub fn parcels(
        &self,
        accounts: &[String],
        model: Option<&crate::manage::local_ai::parcel::Confirmer>,
    ) -> Result<ParcelScan> {
        use crate::manage::parcels::{self, MailText, Parcel, Sighting};
        let version = PARCEL_SCAN * 2 + i64::from(model.is_some());
        let download_since = chrono::Utc::now().timestamp() - PARCEL_DOWNLOAD_DAYS * 86_400;
        let mut found = Vec::new();
        let mut to_download = Vec::new();
        let mut scanned = 0;
        let mut looked_at = HashSet::new();
        let mut with_parcels = HashSet::new();
        for account in accounts {
            let listed = self.scan_rows(account)?;
            // What was found before, without the text of the many messages
            // where nothing was ("[]"): one string kept per message of the
            // account added up.
            let cached: HashMap<String, (bool, i64, Option<String>)> = {
                let db = self.read();
                let mut q =
                    db.prepare("SELECT id, state, version, data FROM parcel_scan WHERE account=?1")?;
                q.query_map([account], |r| {
                    let data: String = r.get(3)?;
                    let data = (data != "[]").then_some(data);
                    Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? != 0, r.get(2)?, data)))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let mut fresh: Vec<(String, bool, Vec<Parcel>)> = Vec::new();
            let mut results: HashMap<String, Vec<Parcel>> = HashMap::new();
            let mut unread: Vec<&ScanRow> = Vec::new();
            for row in &listed {
                let folder = row.folder();
                if row.pending || matches!(folder, Folder::Sent | Folder::Drafts | Folder::Junk) {
                    continue;
                }
                scanned += 1;
                match cached.get(&row.id) {
                    Some((state, v, data)) if *state == row.content && *v == version => {
                        let list: Vec<Parcel> = data
                            .as_deref()
                            .and_then(|d| serde_json::from_str(d).ok())
                            .unwrap_or_default();
                        results.insert(row.id.clone(), list);
                    }
                    // Downloaded, but nothing in its subject, preview or
                    // sender is about orders or shipping: not worth
                    // reading whole (in a large mailbox, minutes of work).
                    _ if row.content
                        && !parcels::worth_reading(&row.subject, &row.snippet, &row.from) =>
                    {
                        fresh.push((row.id.clone(), true, Vec::new()));
                        results.insert(row.id.clone(), Vec::new());
                    }
                    _ if row.content => unread.push(row),
                    _ => {
                        let list = parcels::find_in_mail(
                            &MailText {
                                subject: &row.subject,
                                text: &row.snippet,
                                html: None,
                                sent: row.date,
                            },
                            model,
                        );
                        fresh.push((row.id.clone(), false, list.clone()));
                        results.insert(row.id.clone(), list);
                    }
                }
                // A message saying a delivery moved is downloaded however
                // old: without it an old parcel never reaches "Delivered".
                if !row.content
                    && (row.date >= download_since || parcels::delivery_news(&row.subject))
                    && parcels::looks_like_shipping(&row.subject, &row.snippet, &row.from)
                {
                    to_download.push((row.date, account.clone(), row.id.clone()));
                }
            }
            // In batches, so other work gets the cache in between. Each
            // batch's sources are inflated, parsed and read on several
            // threads; with the model, which reads on one thread, only the
            // first two.
            let with_model = model.is_some();
            self.source_batches(unread.iter().map(|r| r.id.as_str()), PARCEL_BATCH, |rows| {
                let read = parallel_map(rows, |(id, data, raw, source)| {
                    // A source that can't be read leaves the cached text.
                    let raw = unpack_scan(raw, source);
                    let readable = serde_json::from_str::<Mail>(&data)
                        .ok()
                        .map(|mail| ParcelText::of(mail, raw.as_deref()));
                    let found = readable.as_ref().filter(|_| !with_model).map(|r| r.find(None));
                    (id, readable, found)
                });
                for (id, readable, found) in read {
                    let list = match (found, readable) {
                        (Some(list), _) => list,
                        (None, Some(r)) => r.find(model),
                        (None, None) => Vec::new(),
                    };
                    fresh.push((id.clone(), true, list.clone()));
                    results.insert(id, list);
                }
                Ok(())
            })?;
            if !fresh.is_empty() {
                let mut db = self.write();
                let tx = db.transaction()?;
                {
                    let mut put = tx.prepare(
                        "INSERT OR REPLACE INTO parcel_scan (id, account, state, version, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?;
                    for (id, content, list) in &fresh {
                        put.execute(params![
                            id,
                            account,
                            i64::from(*content),
                            version,
                            serde_json::to_string(list)?
                        ])?;
                    }
                }
                // What was found in mail that is gone goes too.
                if cached.len() > listed.len() {
                    tx.execute(
                        "DELETE FROM parcel_scan WHERE account=?1 AND id NOT IN (SELECT id FROM messages WHERE account=?1)",
                        [account],
                    )?;
                }
                tx.commit()?;
            }
            for row in &listed {
                let Some(list) = results.get(&row.id) else {
                    continue;
                };
                looked_at.insert(row.id.clone());
                if !list.is_empty() {
                    with_parcels.insert(row.id.clone());
                }
                for parcel in list {
                    found.push((
                        parcel.clone(),
                        Sighting {
                            account: account.clone(),
                            mail_id: row.id.clone(),
                            message_id: row.message_id.clone(),
                            mailbox: row.mailbox(),
                            subject: row.subject.clone(),
                            sender: if row.sender.is_empty() {
                                row.from.clone()
                            } else {
                                row.sender.clone()
                            },
                            sent: row.date,
                        },
                    ));
                }
            }
        }
        to_download.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(ParcelScan {
            tracked: parcels::summarize(found),
            to_download: to_download.into_iter().map(|(_, a, id)| (a, id)).collect(),
            scanned,
            looked_at,
            with_parcels,
        })
    }
    /// Every receipt `accounts`' cached mail holds, outside Sent, Drafts
    /// and Spam, newest first (see `receipts::summarize`), and how many
    /// messages were looked at.
    ///
    /// As with `parcels`, what was found in each message is kept (in
    /// `receipt_scan`), so a message is read again only once its text is
    /// downloaded or the way of reading (`RECEIPT_SCAN`) changes. Only
    /// downloaded mail whose subject or sender may be a receipt's
    /// (`receipts::worth_reading`) is read whole; mail not downloaded is
    /// read by its subject and preview.
    pub fn receipts(&self, accounts: &[String]) -> Result<ReceiptScan> {
        use crate::manage::parcels::Sighting;
        use crate::manage::receipts::{self, MailText, Receipt};
        let download_since = chrono::Utc::now().timestamp() - PARCEL_DOWNLOAD_DAYS * 86_400;
        let mut found = Vec::new();
        let mut to_download = Vec::new();
        let mut scanned = 0;
        for account in accounts {
            let listed = self.scan_rows(account)?;
            // As `parcels`: none kept for messages that hold no receipt.
            let cached: HashMap<String, (bool, i64, Option<String>)> = {
                let db = self.read();
                let mut q = db
                    .prepare("SELECT id, state, version, data FROM receipt_scan WHERE account=?1")?;
                q.query_map([account], |r| {
                    let data: String = r.get(3)?;
                    let data = (data != "null").then_some(data);
                    Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? != 0, r.get(2)?, data)))
                })?
                .collect::<rusqlite::Result<_>>()?
            };
            let mut fresh: Vec<(String, bool, Option<Receipt>)> = Vec::new();
            let mut results: HashMap<String, Option<Receipt>> = HashMap::new();
            let mut unread: Vec<&ScanRow> = Vec::new();
            for row in &listed {
                if row.pending || matches!(row.folder(), Folder::Sent | Folder::Drafts | Folder::Junk)
                {
                    continue;
                }
                scanned += 1;
                let from = row.sender_line();
                if !row.content
                    && row.date >= download_since
                    && receipts::worth_downloading(&row.subject, &from)
                {
                    to_download.push((row.date, account.clone(), row.id.clone()));
                }
                match cached.get(&row.id) {
                    Some((state, v, data)) if *state == row.content && *v == RECEIPT_SCAN => {
                        let r: Option<Receipt> = data
                            .as_deref()
                            .and_then(|d| serde_json::from_str(d).ok())
                            .unwrap_or_default();
                        results.insert(row.id.clone(), r);
                    }
                    _ if !receipts::worth_reading(&row.subject, &from) => {
                        fresh.push((row.id.clone(), row.content, None));
                        results.insert(row.id.clone(), None);
                    }
                    _ if row.content => unread.push(row),
                    _ => {
                        let r = receipts::find(&MailText {
                            subject: &row.subject,
                            text: &row.snippet,
                            html: None,
                            from: &from,
                        });
                        fresh.push((row.id.clone(), false, r.clone()));
                        results.insert(row.id.clone(), r);
                    }
                }
            }
            self.source_batches(unread.iter().map(|r| r.id.as_str()), PARCEL_BATCH, |rows| {
                let read = parallel_map(rows, |(id, data, raw, source)| {
                    let raw = unpack_scan(raw, source);
                    let r = serde_json::from_str::<Mail>(&data).ok().and_then(|mail| {
                        let from = format!("{} <{}>", mail.sender, mail.from);
                        let read = ParcelText::of(mail, raw.as_deref());
                        receipts::find(&MailText {
                            subject: &read.mail.subject,
                            text: read.text(),
                            html: read.mail.html.as_deref(),
                            from: &from,
                        })
                    });
                    (id, r)
                });
                for (id, r) in read {
                    fresh.push((id.clone(), true, r.clone()));
                    results.insert(id, r);
                }
                Ok(())
            })?;
            if !fresh.is_empty() {
                let mut db = self.write();
                let tx = db.transaction()?;
                {
                    let mut put = tx.prepare(
                        "INSERT OR REPLACE INTO receipt_scan (id, account, state, version, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?;
                    for (id, content, r) in &fresh {
                        put.execute(params![
                            id,
                            account,
                            i64::from(*content),
                            RECEIPT_SCAN,
                            serde_json::to_string(r)?
                        ])?;
                    }
                }
                if cached.len() > listed.len() {
                    tx.execute(
                        "DELETE FROM receipt_scan WHERE account=?1 AND id NOT IN (SELECT id FROM messages WHERE account=?1)",
                        [account],
                    )?;
                }
                tx.commit()?;
            }
            for row in &listed {
                let Some(Some(receipt)) = results.remove(&row.id) else {
                    continue;
                };
                found.push((
                    receipt,
                    Sighting {
                        account: account.clone(),
                        mail_id: row.id.clone(),
                        message_id: row.message_id.clone(),
                        mailbox: row.mailbox(),
                        subject: row.subject.clone(),
                        sender: if row.sender.is_empty() {
                            row.from.clone()
                        } else {
                            row.sender.clone()
                        },
                        sent: row.date,
                    },
                ));
            }
        }
        to_download.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(ReceiptScan {
            listed: receipts::summarize(found),
            to_download: to_download.into_iter().map(|(_, a, id)| (a, id)).collect(),
            scanned,
        })
    }
    /// An account's messages as `parcels` and `receipts` look through
    /// them, without their text.
    fn scan_rows(&self, account: &str) -> Result<Vec<ScanRow>> {
        let db = self.read();
        let mut q = db.prepare(&format!(
            "SELECT id, json_extract(data,'$.folder'), json_extract(data,'$.remote_folder'), date, \
             coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.snippet'),''), \
             coalesce(json_extract(data,'$.from'),''), coalesce(json_extract(data,'$.sender'),''), \
             coalesce(json_extract(data,'$.message_id'),''), \
             (json_type(data,'$.body')='text' OR coalesce(json_extract(data,'$.html'),'')!='' OR {DOWNLOADED}), \
             coalesce(json_extract(data,'$.pending'),0) \
             FROM messages WHERE account=?1"
        ))?;
        Ok(q
            .query_map([account], |r| {
                Ok(ScanRow {
                    id: r.get(0)?,
                    folder: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    remote: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    date: r.get(3)?,
                    subject: r.get(4)?,
                    snippet: r.get(5)?,
                    from: r.get(6)?,
                    sender: r.get(7)?,
                    message_id: r.get(8)?,
                    content: r.get::<_, i64>(9)? != 0,
                    pending: r.get::<_, i64>(10)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// The kept data and saved source of messages, by id: (id, data, raw,
    /// source), for `unpack`, handed to `each` a batch at a time. A batch
    /// has at most `max_rows` rows and, unless one row alone is larger,
    /// `SOURCE_BATCH_BYTES` of saved data; the cache is free for other
    /// work between batches. Ids no longer cached are left out.
    fn source_batches<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
        max_rows: usize,
        mut each: impl FnMut(Vec<SourceRow>) -> Result<()>,
    ) -> Result<()> {
        let mut ids = ids.into_iter().peekable();
        while ids.peek().is_some() {
            let rows = {
                let db = self.read();
                let mut q = db.prepare_cached(&format!(
                    "SELECT data, {SOURCE_COLUMNS} FROM messages WHERE id=?1"
                ))?;
                let mut rows = Vec::new();
                let mut bytes = 0;
                while rows.len() < max_rows && bytes < SOURCE_BATCH_BYTES {
                    let Some(id) = ids.next() else { break };
                    if let Some(r) = q
                        .query_map([id], |r| Ok((r.get::<_, String>(0)?, source_columns(r, 1)?)))?
                        .next()
                    {
                        let (data, (raw, source)) = r?;
                        bytes += data.len()
                            + raw.as_ref().map_or(0, Vec::len)
                            + source.as_ref().map_or(0, Vec::len);
                        rows.push((id.to_owned(), data, raw, source));
                    }
                }
                rows
            };
            if !rows.is_empty() {
                each(rows)?;
            }
        }
        Ok(())
    }
    /// Which of `accounts`' messages have a sign-in or verification code,
    /// which tell where a package is, and which are receipts (see
    /// `mail_kinds`). Package updates and receipts are what the Summary
    /// finds (`parcels`, with the same `parcel_model`, and `receipts`, so
    /// the findings kept for it are used; receipts hidden by hand are left
    /// out); code mail is
    /// what its subject and preview suggest, confirmed by `detector` from
    /// its text, or else by `mail_kinds::code_by_rule`. Mail `previous`
    /// looked at (worked out the same way) isn't read again.
    pub fn mail_kinds(
        &self,
        accounts: &[String],
        parcel_model: Option<&crate::manage::local_ai::parcel::Confirmer>,
        detector: Option<&crate::manage::local_ai::otp::Detector>,
        previous: Option<&crate::manage::mail_kinds::Kinds>,
    ) -> Result<crate::manage::mail_kinds::Kinds> {
        use crate::manage::mail_kinds::{self, Kinds};
        let scan = self.parcels(accounts, parcel_model)?;
        // Receipts as the Summary lists them, without those hidden by hand
        // (taken for receipts wrongly).
        let hidden: Vec<String> = self
            .preference(crate::manage::receipts::HIDDEN_PREFERENCE)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let mut found = self.receipts(accounts)?.listed;
        crate::manage::receipts::apply_hidden(&mut found, &hidden);
        let receipts = found
            .iter()
            .filter(|l| !l.hidden)
            .flat_map(|l| l.emails.iter().map(|s| s.mail_id.clone()))
            .collect();
        let mut kinds = Kinds {
            packages: scan.with_parcels,
            receipts,
            checked: scan.looked_at,
            ..Kinds::default()
        };
        for account in accounts {
            // Judged a row at a time as SQLite reads them: every message's
            // subject and preview were held at once before.
            let mut maybe: Vec<String> = Vec::new();
            {
                let db = self.read();
                let mut q = db.prepare(
                    "SELECT id, coalesce(json_extract(data,'$.folder'),''), \
                     coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.snippet'),'') \
                     FROM messages WHERE account=?1 AND coalesce(json_extract(data,'$.pending'),0)=0",
                )?;
                let mut rows = q.query([account])?;
                while let Some(row) = rows.next()? {
                    let folder = row.get_ref(1)?.as_str()?;
                    // Codes are received, not sent or drafted.
                    if matches!(folder, "Sent" | "Drafts") {
                        continue;
                    }
                    let id: String = row.get(0)?;
                    match previous.filter(|p| p.checked.contains(&id)) {
                        Some(p) => {
                            if p.codes.contains(&id) {
                                kinds.codes.insert(id.clone());
                            }
                        }
                        None if mail_kinds::might_be_code(
                            row.get_ref(2)?.as_str()?,
                            row.get_ref(3)?.as_str()?,
                        ) =>
                        {
                            maybe.push(id.clone())
                        }
                        None => {}
                    }
                    kinds.checked.insert(id);
                }
            }
            // Each likely one read whole, then judged.
            let with_model = detector.is_some();
            self.source_batches(maybe.iter().map(String::as_str), PARCEL_BATCH, |rows| {
                let read = parallel_map(rows, |(id, data, raw, source)| {
                    let raw = unpack_scan(raw, source);
                    let read = serde_json::from_str::<Mail>(&data)
                        .ok()
                        .map(|mail| ParcelText::of(mail, raw.as_deref()));
                    let by_rule = read
                        .as_ref()
                        .filter(|_| !with_model)
                        .map(|r| mail_kinds::code_by_rule(&r.mail.subject, r.text()));
                    (id, read, by_rule)
                });
                for (id, read, by_rule) in read {
                    let code = match (by_rule, read, detector) {
                        (Some(code), _, _) => code,
                        // The model's code, given in a sentence naming it.
                        (None, Some(r), Some(d)) => {
                            mail_kinds::code_in_a_sentence(&r.mail.subject, r.text())
                                && !d.codes(&r.mail.subject, r.text()).is_empty()
                        }
                        _ => false,
                    };
                    if code {
                        kinds.codes.insert(id);
                    }
                }
                Ok(())
            })?;
        }
        // A tracking number is shaped like a code: package updates aren't
        // code mail.
        let Kinds { codes, packages, .. } = &mut kinds;
        codes.retain(|id| !packages.contains(id));
        Ok(kinds)
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
            let mut q = db.prepare(&format!(
                "SELECT id FROM messages WHERE {DOWNLOADED} OR json_type(data,'$.body')='text' OR json_extract(data,'$.snippet')<>''"
            ))?;
            q.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut changed = 0;
        // In batches, so other work gets the cache in between.
        self.source_batches(ids.iter().map(String::as_str), 100, |rows| {
            let mut updates = Vec::new();
            for (id, data, raw, source) in rows {
                let Ok(mail) = serde_json::from_str::<Mail>(&data) else {
                    continue;
                };
                // Inflated one at a time: a batch's sources were all held
                // inflated at once before.
                let raw = unpack_scan(raw, source);
                // Encrypted sources hold no readable text; what is cached stays.
                let parsed = raw
                    .as_deref()
                    .filter(|raw| {
                        !crate::manage::crypto::is_encrypted(raw)
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
                return Ok(());
            }
            let mut db = self.write();
            let tx = db.transaction()?;
            for (id, body, snippet) in &updates {
                match body {
                    Some(body) => tx.prepare_cached("UPDATE messages SET data=json_set(data,'$.body',?2,'$.snippet',?3) WHERE id=?1")?.execute(
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
            Ok(())
        })?;
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
            let mut q = db.prepare(&format!(
                "SELECT id FROM messages WHERE {DOWNLOADED} AND json_extract(data,'$.list_unsubscribe') IS NULL AND (?1 IS NULL OR account=?1)"
            ))?;
            q.query_map([account], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut total = 0;
        // In batches, as refresh_previews does, so sync and the interface
        // get the cache in between.
        for chunk in ids.chunks(100) {
            let rows: Vec<(String, Vec<u8>)> = {
                let db = self.read();
                let mut q = db.prepare(&format!(
                    "SELECT {SOURCE_COLUMNS} FROM messages WHERE id=?1 AND json_extract(data,'$.list_unsubscribe') IS NULL"
                ))?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(saved) = q.query_map([id], |r| source_columns(r, 0))?.next() {
                        let (raw, source) = saved?;
                        // Only the headers are needed, which come first.
                        if let Some(head) = unpack_head(raw, source, 65536)? {
                            rows.push((id.clone(), head));
                        }
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
                tx.prepare_cached("UPDATE messages SET data=json_set(data,'$.list_unsubscribe',?2,'$.one_click',json(?3)) WHERE id=?1")?.execute(
                    params![id, header, if *one_click { "true" } else { "false" }],
                )?;
            }
            tx.commit()?;
            total += found.len();
        }
        Ok(total)
    }
    /// The newest message with this Message-ID (`<…>`), in any account.
    pub fn by_message_id(&self, message_id: &str) -> Result<Option<Mail>> {
        use rusqlite::OptionalExtension;
        let json: Option<String> = self
            .read()
            .query_row(
                "SELECT data FROM messages WHERE json_extract(data,'$.message_id')=?1 ORDER BY date DESC LIMIT 1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
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
                tx.prepare_cached("DELETE FROM messages WHERE id=?1")?.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Compresses the sources kept as downloaded in `raw` into `source`, a
    /// batch at a time so other work gets the cache in between; then gives
    /// the freed pages back to the system, where the database can do that
    /// a few at a time (see `compact_on_quit` for one that can't yet).
    /// Returns how many sources were moved.
    pub fn pack_saved_sources(&self) -> Result<usize> {
        // Batches of at most 50 sources and about 16 MiB of them, so a few
        // large attachments don't make one batch hundreds of MiB.
        const BATCH_BYTES: u64 = 16 * 1024 * 1024;
        let ids: Vec<(String, u64)> = {
            let db = self.read();
            let mut q = db.prepare("SELECT id, length(raw) FROM messages WHERE raw IS NOT NULL")?;
            q.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)?.max(0) as u64)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut batches: Vec<Vec<String>> = Vec::new();
        let mut bytes = 0;
        for (id, len) in ids {
            match batches.last_mut() {
                Some(batch) if batch.len() < 50 && bytes + len <= BATCH_BYTES => {
                    bytes += len;
                    batch.push(id);
                }
                _ => {
                    bytes = len;
                    batches.push(vec![id]);
                }
            }
        }
        let mut packed = 0;
        for chunk in &batches {
            let rows: Vec<(String, Vec<u8>)> = {
                let db = self.read();
                let mut q = db.prepare("SELECT raw FROM messages WHERE id=?1 AND raw IS NOT NULL")?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(raw) = q.query_map([id], |r| r.get::<_, Vec<u8>>(0))?.next() {
                        let raw = raw?;
                        let source = match raw.strip_prefix(LEGACY_PACKED) {
                            // Compressed already. A damaged one is left as it
                            // is (reading it says so), not moved.
                            Some(packed) if inflate(packed, MAX_SOURCE, true).is_ok() => packed.to_vec(),
                            Some(_) => continue,
                            None => pack(&raw)?,
                        };
                        rows.push((id.clone(), source));
                    }
                }
                rows
            };
            let mut db = self.write();
            let tx = db.transaction()?;
            for (id, source) in &rows {
                // Unless it was saved again meanwhile (into `source`, then).
                packed += tx.prepare_cached("UPDATE messages SET source=?2, raw=NULL WHERE id=?1 AND raw IS NOT NULL")?.execute(
                    params![id, source],
                )?;
            }
            tx.commit()?;
        }
        self.free_pages()?;
        Ok(packed)
    }
    /// Gives free pages back to the system a batch at a time, releasing the
    /// cache in between, in a database that can (`auto_vacuum` incremental).
    /// Gives the space of deleted rows back to the system, where the cache
    /// does so when asked (see `free_pages`).
    pub fn give_back_space(&self) -> Result<()> {
        self.free_pages()
    }
    fn free_pages(&self) -> Result<()> {
        let mut left = i64::MAX;
        loop {
            let db = self.write();
            let mode: i64 = db.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
            let free: i64 = db.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            // None left, or none freed by the last batch.
            if mode != 2 || free == 0 || free >= left {
                break;
            }
            left = free;
            // Frees a page per step: run it to the end (execute_batch steps once).
            let mut q = db.prepare("PRAGMA incremental_vacuum(2048)")?;
            let mut rows = q.query([])?;
            while rows.next()?.is_some() {}
        }
        self.write().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
    /// Rebuilds a database made before free pages were given back, so that
    /// it gives back the space compressed sources freed, and from then on
    /// does so as it goes. Only when it would give back enough (a tenth of
    /// the file, and 16 MB) and nothing else is writing to the cache.
    ///
    /// Meant for quitting: the rebuild (VACUUM) holds the cache until it is
    /// done, some seconds for a large one, and anything wanting to write in
    /// the meantime would wait. It is all or nothing: stopped part way, the
    /// database is as it was. One that fails (often for want of disk space:
    /// it needs about twice the database's size) isn't tried again for a
    /// week. Returns whether it ran.
    pub fn compact_on_quit(&self) -> Result<bool> {
        const RETRY: &str = "compact_after";
        let not_before = self
            .preference(RETRY)
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0);
        let now = chrono::Utc::now().timestamp();
        if now < not_before {
            return Ok(false);
        }
        // A write running (a sync finishing): another time.
        let Ok(db) = self.db.try_lock() else {
            return Ok(false);
        };
        let pragma = |name: &str| -> Result<i64> {
            Ok(db.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?)
        };
        let (mode, pages, free, size) = (
            pragma("auto_vacuum")?,
            pragma("page_count")?,
            pragma("freelist_count")?,
            pragma("page_size")?,
        );
        if free * size < 16 * 1024 * 1024 || free * 10 < pages {
            return Ok(false);
        }
        if mode == 2 {
            // Gives pages back as it goes, but only when asked: the space
            // deleted mail (or a removed account) left was otherwise kept
            // for good, as only the one-time move of sources asked.
            drop(db);
            self.free_pages()?;
            return Ok(true);
        }
        if let Err(e) =
            db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        {
            let week = 7 * 24 * 60 * 60;
            db.execute(PUT_PREFERENCE, params![RETRY, (now + week).to_string()])?;
            return Err(e).context("Couldn't compact the mail cache");
        }
        Ok(true)
    }
    pub fn raw(&self, id: &str) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        let saved = self
            .read()
            .query_row(
                &format!("SELECT {SOURCE_COLUMNS} FROM messages WHERE id=?1"),
                [id],
                |r| source_columns(r, 0),
            )
            .optional()?;
        match saved {
            Some((raw, source)) => unpack(raw, source),
            None => Ok(None),
        }
    }
    /// Adds a change made offline to the end of `account`'s queue: `data`
    /// (JSON), for `messages` messages.
    pub fn queue_change(&self, account: &str, messages: usize, data: &str) -> Result<()> {
        self.write().execute(
            "INSERT INTO offline_changes (account, messages, data) VALUES (?1, ?2, ?3)",
            params![account, messages, data],
        )?;
        Ok(())
    }
    /// Puts changes queued before this table (an earlier version's) at the
    /// front of `account`'s queue, in their order: they were made before
    /// any queued since. (data, messages) each.
    pub fn queue_changes_first(&self, account: &str, older: &[(String, usize)]) -> Result<()> {
        if older.is_empty() {
            return Ok(());
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        let lowest: i64 = tx.query_row("SELECT COALESCE(MIN(seq), 1) FROM offline_changes", [], |r| r.get(0))?;
        // Numbers below every row's, in order (`seq` may go below 1).
        let first = lowest - older.len() as i64;
        for (i, (data, messages)) in older.iter().enumerate() {
            tx.execute(
                "INSERT INTO offline_changes (seq, account, messages, data) VALUES (?1, ?2, ?3, ?4)",
                params![first + i as i64, account, messages, data],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// `account`'s queued changes, oldest first: (its number, JSON).
    pub fn queued_changes(&self, account: &str) -> Result<Vec<(i64, String)>> {
        let db = self.read();
        let mut q = db.prepare_cached("SELECT seq, data FROM offline_changes WHERE account=?1 ORDER BY seq")?;
        let rows = q
            .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    /// How many messages `account`'s queued changes are for, without
    /// reading them (the account list shows it).
    pub fn queued_messages(&self, account: &str) -> usize {
        self.read()
            .query_row(
                "SELECT COALESCE(SUM(messages),0) FROM offline_changes WHERE account=?1",
                [account],
                |r| r.get::<_, i64>(0),
            )
            .map_or(0, |n| n as usize)
    }
    /// Takes a queued change off the queue (sent, or let go).
    pub fn dequeue_change(&self, seq: i64) -> Result<()> {
        self.write().execute("DELETE FROM offline_changes WHERE seq=?1", [seq])?;
        Ok(())
    }
    /// Whether message `id`'s source is saved here, without reading it (a
    /// source read whole, and inflated, only to say that it's there).
    pub fn has_raw(&self, id: &str) -> Result<bool> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .query_row(
                &format!("SELECT 1 FROM messages WHERE id=?1 AND {DOWNLOADED}"),
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
    pub fn save_raw(&self, id: &str, raw: &[u8]) -> Result<()> {
        let source = pack(raw)?;
        self.db.lock().unwrap().execute(
            "UPDATE messages SET raw=NULL, source=?2 WHERE id=?1",
            params![id, source],
        )?;
        Ok(())
    }
    /// One accepted remote command is one local transaction. Earlier confirmed
    /// groups remain committed if a later mailbox rejects a bulk operation.
    pub fn commit_changes(&self, messages: &[Mail], delete: bool) -> Result<()> {
        let mut db = self.write();
        let tx = db.transaction()?;
        if delete {
            let mut q = tx.prepare_cached("DELETE FROM messages WHERE id=?1 AND account=?2")?;
            for mail in messages {
                q.execute(params![mail.id, mail.account])?;
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
        let mut q = db.prepare(&format!(
            "SELECT data FROM messages WHERE account=?1 AND {NOT_DOWNLOADED} AND date >= ?4
             AND json_extract(data,'$.size') <= ?2 ORDER BY date DESC, id DESC LIMIT ?3"
        ))?;
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
        let mut q = db.prepare(&format!(
            "SELECT COUNT(*), COALESCE(SUM({NOT_DOWNLOADED}),0) FROM messages
             WHERE account=?1 AND date >= ?3 AND json_extract(data,'$.size') <= ?2"
        ))?;
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
        // Only the copies not downloaded are visited (on a reading
        // connection, so the cache stays free for writes meanwhile), and the
        // downloaded copy of each is looked up by the `gmail` index. Before,
        // every Gmail row of the account was read, under the write lock, on
        // every download batch, even one with nothing to fill.
        let pairs: Vec<(String, String)> = {
            let db = self.read();
            let mut missing = db.prepare_cached(&format!(
                "SELECT id, json_extract(data,'$.gmail_id') FROM messages
                 WHERE account=?1 AND {NOT_DOWNLOADED} AND {GMAIL_ID} > 0"
            ))?;
            let mut donor = db.prepare_cached(&format!(
                "SELECT id FROM messages WHERE account=?1 AND {GMAIL_ID} > 0
                 AND {GMAIL_ID}=?2 AND {DOWNLOADED} LIMIT 1"
            ))?;
            let wanted = missing
                .query_map([account], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut pairs = Vec::new();
            for (id, gmail_id) in wanted {
                if let Some(from) = donor
                    .query_map(params![account, gmail_id], |r| r.get::<_, String>(0))?
                    .next()
                {
                    pairs.push((id, from?));
                }
            }
            pairs
        };
        if pairs.is_empty() {
            return Ok(0);
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        let mut filled = 0;
        {
            let mut copy = tx.prepare(&format!(
                "UPDATE messages SET (raw, source)=(SELECT raw, source FROM messages WHERE id=?2)
                 WHERE id=?1 AND {NOT_DOWNLOADED}"
            ))?;
            for (id, from) in &pairs {
                filled += copy.execute(params![id, from])?;
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
        let saved = self
            .read()
            .query_row(
                &format!(
                    "SELECT {SOURCE_COLUMNS} FROM messages WHERE account=?1 AND {DOWNLOADED}
                     AND {GMAIL_ID} > 0 AND {GMAIL_ID}=?2 LIMIT 1"
                ),
                params![account, gmail_id as i64],
                |r| source_columns(r, 0),
            )
            .optional()?;
        match saved {
            Some((raw, source)) => unpack(raw, source),
            None => Ok(None),
        }
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
                // A receipt handled here stays handled: where the mailbox
                // can't keep $MDNSent (no \* in PERMANENTFLAGS, RFC 3503
                // §3.1), the server never shows it, and asking again would
                // send a second receipt.
                let marks = (*answered, *forwarded, *mdn_sent || mail.mdn_sent);
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
    /// Saves a draft only if it is still here (not discarded meanwhile).
    /// Returns whether it was.
    pub fn update_draft(&self, d: &Draft) -> Result<bool> {
        let changed = self.write().execute(
            "UPDATE drafts SET account=?2, data=?3 WHERE id=?1",
            params![d.id, d.account, serde_json::to_string(d)?],
        )?;
        Ok(changed > 0)
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
    /// Deletes a draft, returning it as it was when deleted: a sync saving
    /// it at the same moment either finds it gone or is returned here.
    pub fn take_draft(&self, id: &str) -> Result<Option<Draft>> {
        use rusqlite::OptionalExtension;
        let data: Option<String> = self
            .write()
            .query_row("DELETE FROM drafts WHERE id=?1 RETURNING data", [id], |r| r.get(0))
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
    /// Removes a preference, after any set to be written soon.
    pub fn remove_preference(&self, key: &str) -> Result<()> {
        self.flush();
        self.write().execute("DELETE FROM preferences WHERE key=?1", [key])?;
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
// Saved message sources.
//
// A source is saved compressed (raw deflate, about a quarter of its size)
// in the `source` column, leaving `raw` empty. `raw` holds a source as it
// was downloaded: saved by a build from before sources were compressed,
// including one run again after this one (it sees no source in `raw`, so
// downloads the message again). A row may then have both; `raw`, the
// later, is read. `pack_saved_sources` moves those into `source`.
//
// Builds of this change before the column was added kept the compressed
// source in `raw` behind `LEGACY_PACKED`; those still read, and are moved.

/// Saved sources' columns, in the order `unpack` and `unpack_head` take them.
const SOURCE_COLUMNS: &str = "raw, source";
/// The rows whose source has been downloaded.
const DOWNLOADED: &str = "(raw IS NOT NULL OR source IS NOT NULL)";
/// How messages are read for parcels (see `Store::parcels`): what was
/// found by an earlier way is found again. Raise it whenever `parcels`
/// finds differently, or the Summary keeps the old findings.
const PARCEL_SCAN: i64 = 8;
/// How far back shipping mail known only by its headers is downloaded for
/// the Summary (mail whose subject says a delivery moved is downloaded
/// whatever its age).
const PARCEL_DOWNLOAD_DAYS: i64 = 365;

/// How messages are read for receipts (see `Store::receipts`): raise it
/// whenever `receipts::find` finds differently.
const RECEIPT_SCAN: i64 = 3;

/// What a look through mail for receipts found.
pub struct ReceiptScan {
    pub listed: Vec<crate::manage::receipts::Listed>,
    /// Receipts known only by their headers, newest first: (account,
    /// message id), to download so their totals can be read.
    pub to_download: Vec<(String, String)>,
    /// How many messages were looked at.
    pub scanned: usize,
}

/// A message's id, kept data and saved source (see `Store::source_rows`).
type SourceRow = (String, String, Option<Vec<u8>>, Option<Vec<u8>>);

/// What a look through mail for parcels found.
pub struct ParcelScan {
    pub tracked: Vec<crate::manage::parcels::Tracked>,
    /// Shipping mail known only by its headers, newest first: (account,
    /// message id).
    pub to_download: Vec<(String, String)>,
    /// How many messages were looked at.
    pub scanned: usize,
    /// Which messages were looked at, and which a parcel was found in.
    pub looked_at: HashSet<String>,
    pub with_parcels: HashSet<String>,
}

/// A message as `Store::parcels` lists it, without its text.
struct ScanRow {
    id: String,
    folder: String,
    remote: String,
    date: i64,
    subject: String,
    snippet: String,
    from: String,
    sender: String,
    message_id: String,
    /// Its text or source was downloaded.
    content: bool,
    pending: bool,
}

impl ScanRow {
    /// Its sender as "Name <address>".
    fn sender_line(&self) -> String {
        format!("{} <{}>", self.sender, self.from)
    }
    fn folder(&self) -> Folder {
        serde_json::from_value(serde_json::Value::String(self.folder.clone()))
            .unwrap_or(Folder::Other)
    }
    fn mailbox(&self) -> Mailbox {
        match self.folder() {
            Folder::Other => Mailbox::Remote(self.remote.clone()),
            f => Mailbox::Standard(f),
        }
    }
}

/// Downloaded messages read for parcels at a time (see `Store::parcels`).
const PARCEL_BATCH: usize = 400;
/// The most saved data (kept JSON and compressed sources) read into one
/// batch of `Store::source_batches`: a count alone let 400 large
/// messages be held at once.
const SOURCE_BATCH_BYTES: usize = 32 * 1024 * 1024;
/// The most of a message's text the Summary's package search looks
/// through (`Store::parcel_search_texts`): a long newsletter's tail says
/// nothing of a parcel.
const PARCEL_SEARCH_TEXT: usize = 50_000;

/// `f` of each of `items`, in order, on as many threads as there are
/// cores (up to 8), each a part of the list.
fn parallel_map<T: Send, U: Send>(items: Vec<T>, f: impl Fn(T) -> U + Sync) -> Vec<U> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    if threads < 2 || items.len() < 2 {
        return items.into_iter().map(f).collect();
    }
    let size = items.len().div_ceil(threads);
    let mut parts: Vec<Vec<T>> = Vec::new();
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        parts.push(items.by_ref().take(size).collect());
    }
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .into_iter()
            .map(|part| scope.spawn(move || part.into_iter().map(f).collect::<Vec<U>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

/// A downloaded message as it is read for parcels: its text, or else its
/// HTML's text, or else what its source holds (unless encrypted).
struct ParcelText {
    mail: Mail,
    /// Its HTML's text, when it has HTML and no text.
    converted: Option<String>,
}

impl ParcelText {
    fn of(mut mail: Mail, raw: Option<&[u8]>) -> Self {
        fill_from(&mut mail, raw);
        let converted = match (&mail.body, &mail.html) {
            (None, Some(html)) => Some(crate::sync::mime::html_to_text(html)),
            _ => None,
        };
        Self { mail, converted }
    }
    /// Its text, or its HTML's, or else its preview.
    fn text(&self) -> &str {
        self.mail
            .body
            .as_deref()
            .or(self.converted.as_deref())
            .unwrap_or(&self.mail.snippet)
    }
    /// The parcels in it.
    fn find(
        &self,
        model: Option<&crate::manage::local_ai::parcel::Confirmer>,
    ) -> Vec<crate::manage::parcels::Parcel> {
        let mail = &self.mail;
        let text = self.text();
        crate::manage::parcels::find_in_mail(
            &crate::manage::parcels::MailText {
                subject: &mail.subject,
                text,
                html: mail.html.as_deref(),
                sent: mail.date,
            },
            model,
        )
    }
}

/// Gives a message without its text or HTML kept those of its source
/// (unless encrypted).
fn fill_from(mail: &mut Mail, raw: Option<&[u8]>) {
    // An empty HTML kept is as good as none: some mail was cached with
    // one and nothing else.
    if mail.html.as_deref().is_some_and(str::is_empty) {
        mail.html = None;
    }
    if mail.body.is_none()
        && mail.html.is_none()
        && let Some(raw) = raw.filter(|raw| {
            !crate::manage::crypto::is_encrypted(raw)
                && !raw.windows(27).any(|w| w == b"-----BEGIN PGP MESSAGE-----")
        })
    {
        let mut a = Account::new(Provider::Imap);
        a.id = mail.account.clone();
        if let Ok(parsed) = crate::sync::mime::parse_message(
            raw,
            &a,
            &mail.id,
            mail.folder,
            &mail.remote_folder,
            mail.uid,
        ) {
            mail.body = parsed.body;
            mail.html = parsed.html;
        }
    }
}
/// The rows whose source hasn't been. Written exactly as the `downloaded`
/// index has it, so that the download counts are read from the index.
const NOT_DOWNLOADED: &str = "(raw IS NULL AND source IS NULL)";
/// A row's Gmail message id, written exactly as the `gmail` index has it.
/// Queries also ask for it `> 0`, the index's condition, so SQLite uses it.
const GMAIL_ID: &str = "json_extract(data,'$.gmail_id')";

/// Which rows a virtual mailbox (Starred, All Mail) gathers, as SQL with
/// the account `?1` and the mailbox's key `?2`; `None` for a real one.
fn virtual_rows(folder: Folder) -> Option<&'static str> {
    match folder {
        Folder::Starred => Some("(folder=?2 OR json_extract(data,'$.starred'))"),
        Folder::All => Some("(folder=?2 OR folder<>'drafts')"),
        _ => None,
    }
}
/// What earlier builds of this change put before a compressed source they
/// kept in `raw`. No downloaded message starts with a NUL.
const LEGACY_PACKED: &[u8] = b"\0PZ1";
/// No source is larger than a download may be (`large::MAX_DOWNLOAD` is
/// this): one that inflates past it is damaged. Before they agreed, a
/// message downloaded whole could be saved and then not read back.
pub(crate) const MAX_SOURCE: u64 = 200 * 1024 * 1024;
/// The most of a source the background scans (parcels, receipts, codes,
/// previews) inflate: a larger one is read by its cached text, as one
/// not downloaded is, rather than holding hundreds of megabytes on each
/// of several threads.
const MAX_SCAN_SOURCE: u64 = 2 * crate::sync::transport::MAX_MESSAGE as u64;

/// A message source compressed, as it is saved in `source`.
fn pack(raw: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut out = flate2::write::DeflateEncoder::new(
        Vec::with_capacity(raw.len() / 3),
        flate2::Compression::default(),
    );
    out.write_all(raw)?;
    Ok(out.finish()?)
}
/// A compressed source inflated: whole (an error past `limit` bytes), or
/// just its first `limit` bytes.
fn inflate(packed: &[u8], limit: u64, whole: bool) -> Result<Vec<u8>> {
    use std::io::Read;
    let capacity = packed.len().saturating_mul(3).min(usize::try_from(limit).unwrap_or(usize::MAX));
    let mut raw = Vec::with_capacity(capacity);
    flate2::read::DeflateDecoder::new(packed)
        .take(limit.saturating_add(u64::from(whole)))
        .read_to_end(&mut raw)
        .context("A saved message source is damaged")?;
    ensure!(
        !whole || raw.len() as u64 <= limit,
        "A saved message source is damaged: it inflates past {limit} bytes"
    );
    Ok(raw)
}
/// A row's saved source as it was downloaded, from its `SOURCE_COLUMNS`;
/// see `inflate` for `limit` and `whole`.
fn read_source(
    raw: Option<Vec<u8>>,
    source: Option<Vec<u8>>,
    limit: u64,
    whole: bool,
) -> Result<Option<Vec<u8>>> {
    Ok(match (raw, source) {
        (Some(raw), _) => Some(match raw.strip_prefix(LEGACY_PACKED) {
            Some(packed) => inflate(packed, limit, whole)?,
            None if whole => raw,
            None => {
                let mut raw = raw;
                raw.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
                raw
            }
        }),
        (None, Some(source)) => Some(inflate(&source, limit, whole)?),
        (None, None) => None,
    })
}
/// A row's whole saved source, from its `SOURCE_COLUMNS`.
fn unpack(raw: Option<Vec<u8>>, source: Option<Vec<u8>>) -> Result<Option<Vec<u8>>> {
    read_source(raw, source, MAX_SOURCE, true)
}
/// A row's whole saved source for a background scan, or `None` when it
/// can't be read or is larger than `MAX_SCAN_SOURCE`.
fn unpack_scan(raw: Option<Vec<u8>>, source: Option<Vec<u8>>) -> Option<Vec<u8>> {
    read_source(raw, source, MAX_SCAN_SOURCE, true).ok().flatten()
}
/// The first `limit` bytes of a row's saved source (its headers come first).
fn unpack_head(raw: Option<Vec<u8>>, source: Option<Vec<u8>>, limit: u64) -> Result<Option<Vec<u8>>> {
    read_source(raw, source, limit, false)
}
/// The source columns of a row read with `SOURCE_COLUMNS` from `first` on.
fn source_columns(r: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
    Ok((r.get(first)?, r.get(first + 1)?))
}

/// Saves a preference, writing nothing when it already has that value
/// (every check saves its account's folder list, nearly always unchanged).
const PUT_PREFERENCE: &str = "INSERT INTO preferences VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value IS NOT excluded.value";

/// The preference writer: writes whatever is pending each time it is told
/// there is something, until the store closes.
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
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn secret_entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SECRET_SERVICE, id).context("Cannot access the system keychain")
}
/// iOS keeps credentials in the app's own Keychain (Protected Data); Android
/// keeps them in the app's SharedPreferences, encrypted with a key held by the
/// Android Keystore. keyring's own Entry has a default store on neither.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn secret_entry(id: &str) -> Result<keyring_core::Entry> {
    static STORE: std::sync::LazyLock<std::result::Result<(), String>> =
        std::sync::LazyLock::new(|| {
            #[cfg(target_os = "ios")]
            let store = apple_native_keyring_store::protected::Store::new();
            // Needs ndk-context, which android-activity sets up before android_main.
            #[cfg(target_os = "android")]
            let store = android_native_keyring_store::Store::new();
            store
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
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use keyring::Error as KeychainError;
#[cfg(any(target_os = "ios", target_os = "android"))]
use keyring_core::Error as KeychainError;
/// A keychain entry's text as stored, or `None` when there is none.
pub fn read_secret_text(id: &str) -> Result<Option<String>> {
    let Some(first) = read_one_secret(id)? else {
        return Ok(None);
    };
    // Kept in parts (see `write_secret_text`): the rest are `id#1`, `id#2`…
    let Some(parts) = first.strip_prefix(SPLIT).and_then(|n| n.parse::<usize>().ok()) else {
        return Ok(Some(first));
    };
    let mut text = String::new();
    for i in 1..=parts {
        text.push_str(
            &read_one_secret(&format!("{id}#{i}"))?
                .context("A part of a keychain entry is missing")?,
        );
    }
    Ok(Some(text))
}
fn read_one_secret(id: &str) -> Result<Option<String>> {
    match secret_entry(id)?.get_password() {
        Ok(text) => Ok(Some(text)),
        Err(KeychainError::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::Error::new(e)
            .context("The keychain entry is locked, or access was denied")),
    }
}
/// What begins an entry kept in parts, followed by how many there are.
const SPLIT: &str = "\u{0}picacho-parts:";
/// The most text kept in one entry: Windows' Credential Manager takes at most
/// 2,560 bytes, stored as UTF-16 (two bytes each for ASCII), and the OpenPGP
/// keyring, with an RSA key and a few people's keys, is larger. Elsewhere
/// entries take far more, but parts work there too.
const PART: usize = 1200;
/// Saves text to a keychain entry, replacing what it had. Text too long for
/// one entry (see `PART`) is kept in several.
pub fn write_secret_text(id: &str, text: &str) -> Result<()> {
    let entry = |name: &str, value: &str| {
        secret_entry(name)?
            .set_password(value)
            .context("Could not securely save to the system keychain")
    };
    if text.len() <= PART && !text.starts_with(SPLIT) {
        entry(id, text)?;
        // Parts of what was kept before, now unused.
        remove_parts_after(id, 0);
        return Ok(());
    }
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut at = rest.len().min(PART);
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        parts.push(&rest[..at]);
        rest = &rest[at..];
    }
    // The parts first, then the entry naming them: a save cut short leaves
    // what was there before readable.
    for (i, part) in parts.iter().enumerate() {
        entry(&format!("{id}#{}", i + 1), part)?;
    }
    entry(id, &format!("{SPLIT}{}", parts.len()))?;
    remove_parts_after(id, parts.len());
    Ok(())
}
/// Removes `id`'s parts numbered past `kept`, until one is missing.
fn remove_parts_after(id: &str, kept: usize) {
    for i in kept + 1.. {
        let Ok(entry) = secret_entry(&format!("{id}#{i}")) else {
            return;
        };
        if entry.delete_credential().is_err() {
            return;
        }
    }
}
/// Removes a keychain entry; one that isn't there is already removed.
pub fn delete_secret(id: &str) -> Result<()> {
    match secret_entry(id)?.delete_credential() {
        Ok(()) | Err(KeychainError::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context("Could not remove a keychain entry")),
    }
}
pub fn write_secret(id: &str, s: &Secret) -> Result<()> {
    secret_entry(id)?
        .set_password(&serde_json::to_string(s)?)
        .context("Could not securely save credentials to the system keychain")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_store() -> Result<(std::path::PathBuf, Store)> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        Ok((p, s))
    }
    /// Starred and All Mail are marked read in the cache without reading
    /// their mail whole; only the unread are listed for the server, and
    /// bodies stay.
    #[test]
    fn virtual_mailboxes_are_marked_read_in_place() -> Result<()> {
        let (p, s) = temp_store()?;
        let mut ms = demo_mail();
        for (i, m) in ms.iter_mut().enumerate() {
            m.unread = i % 2 == 0;
            m.starred = i < 3;
            m.folder = Folder::Inbox;
            m.body = Some(format!("body {i}"));
        }
        s.save_messages(&ms)?;
        let starred = s.unread_summaries("demo", Folder::Starred)?;
        assert!(starred.iter().all(|m| m.unread && m.starred && m.body.is_none()));
        assert_eq!(starred.len(), ms.iter().filter(|m| m.unread && m.starred).count());
        s.mark_mailbox_read("demo", Folder::Starred)?;
        for m in &ms {
            let now = s.message(&m.id)?.unwrap();
            assert_eq!(now.unread, m.unread && !m.starred, "{}", m.id);
            assert_eq!(now.body, m.body);
        }
        s.mark_mailbox_read("demo", Folder::All)?;
        assert!(s.unread_summaries("demo", Folder::All)?.is_empty());
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    /// Sources are read in batches bounded by their size, not only their
    /// count, and every row comes once.
    #[test]
    fn source_batches_are_bounded_by_bytes() -> Result<()> {
        let (p, s) = temp_store()?;
        let ms = demo_mail();
        s.save_messages(&ms)?;
        // Incompressible, so each saved source is about this large.
        let size = SOURCE_BATCH_BYTES / 3 + 1;
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let noise: Vec<u8> = (0..size)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        for m in &ms {
            s.save_raw(&m.id, &noise)?;
        }
        let mut batches = Vec::new();
        s.source_batches(ms.iter().map(|m| m.id.as_str()).chain(["gone"]), PARCEL_BATCH, |rows| {
            batches.push(rows.iter().map(|r| r.0.clone()).collect::<Vec<_>>());
            Ok(())
        })?;
        assert!(batches.iter().all(|b| b.len() <= 3), "{batches:?}");
        let all: Vec<String> = batches.concat();
        assert_eq!(all, ms.iter().map(|m| m.id.clone()).collect::<Vec<_>>());
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    /// A source as large as a download may be reads back whole; background
    /// scans pass it over rather than inflating it.
    #[test]
    fn large_saved_sources_read_back() -> Result<()> {
        let (p, s) = temp_store()?;
        let ms = demo_mail();
        s.save_messages(&ms[..1])?;
        let raw = vec![b'a'; MAX_SCAN_SOURCE as usize + 1024];
        s.save_raw(&ms[0].id, &raw)?;
        assert_eq!(s.raw(&ms[0].id)?.map(|r| r.len()), Some(raw.len()));
        let (kept, source) = s.read().query_row("SELECT raw, source FROM messages WHERE id=?1", [&ms[0].id], |r| source_columns(r, 0))?;
        assert!(unpack_scan(kept, source).is_none());
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
    /// Removing an account forgets its preferences, not other accounts'.
    #[test]
    fn removing_an_account_forgets_its_preferences() -> Result<()> {
        let (p, s) = temp_store()?;
        for id in ["gone", "gone2"] {
            for key in ["offline_changes", "draft_removals", "avatar", "traffic", "folder_order"] {
                s.set_preference(&format!("{key}:{id}"), "x")?;
            }
            s.set_preference(&format!("jmap_state:{id}:inbox"), "x")?;
        }
        s.remove_account("gone")?;
        for key in ["offline_changes", "draft_removals", "avatar", "traffic", "folder_order"] {
            assert_eq!(s.preference(&format!("{key}:gone")), None, "{key}");
            assert!(s.preference(&format!("{key}:gone2")).is_some(), "{key}");
        }
        assert_eq!(s.preference("jmap_state:gone:inbox"), None);
        assert!(s.preference("jmap_state:gone2:inbox").is_some());
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
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
/// Inputs validated before a mailbox snapshot is committed.
pub struct MailboxSnapshot<'a> {
    pub account: &'a str,
    pub mailbox: &'a Mailbox,
    pub remote: &'a str,
    pub validity: u32,
    pub ids: &'a [String],
    pub starred_only: bool,
}
fn write_messages(tx: &rusqlite::Transaction<'_>, messages: &[Mail]) -> Result<()> {
    // Compiled once per connection, not once per message.
    let mut q = tx.prepare_cached("INSERT INTO messages(id,account,folder,date,data) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET folder=excluded.folder,date=excluded.date,data=excluded.data WHERE folder IS NOT excluded.folder OR date IS NOT excluded.date OR data IS NOT excluded.data")?;
    for m in messages {
        q.execute(params![m.id,m.account,m.mailbox().key(),m.date,serde_json::to_string(m)?])?;
    }
    Ok(())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::sync::transport::parse_message;

    /// A message with a large, very compressible source, saved without it.
    fn big_source(store: &Store, a: &Account, i: usize) -> Result<(String, Vec<u8>)> {
        let mut raw = format!("Subject: Hi {i}\r\nList-Unsubscribe: <https://x/u>\r\n\r\n").into_bytes();
        raw.extend(b"The same paragraph, again and again. ".repeat(20_000));
        let id = format!("{}:{i}", a.id);
        let mut m = parse_message(&raw, a, &id, Folder::Inbox, "INBOX", i as u32)?;
        m.gmail_id = 100 + i as u64;
        m.list_unsubscribe.clear();
        store.save_messages(std::slice::from_ref(&m))?;
        Ok((id, raw))
    }
    fn columns(store: &Store, id: &str) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>)> {
        Ok(store.read().query_row("SELECT raw, source FROM messages WHERE id=?1", [id], |r| source_columns(r, 0))?)
    }

    /// Sources are saved compressed in `source` and read back as
    /// downloaded. Ones kept in `raw` (by older builds, as downloaded, or
    /// by earlier builds of this change, compressed) still read, until the
    /// one-time pass moves them, giving the freed space back.
    #[test]
    fn sources_are_kept_compressed_and_read_back_whole() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-packed-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Gmail);
        let mut saved = Vec::new();
        for i in 0..6 {
            saved.push(big_source(&store, &a, i)?);
        }
        // Saved now: compressed, and read back as it was.
        let (id, raw) = &saved[0];
        store.save_raw(id, raw)?;
        let (kept, source) = columns(&store, id)?;
        assert!(kept.is_none());
        assert!(source.unwrap().len() < raw.len() / 10);
        assert_eq!(store.raw(id)?.as_ref(), Some(raw));
        assert_eq!(store.gmail_raw(&a.id, 100)?.as_ref(), Some(raw));
        // Kept in `raw` before: as downloaded, or compressed behind the
        // earlier marker. Both still read.
        for (i, (id, raw)) in saved.iter().enumerate().skip(1) {
            let kept = if i % 2 == 0 { [LEGACY_PACKED, &pack(raw)?].concat() } else { raw.clone() };
            store.write().execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, kept])?;
        }
        for (id, raw) in &saved {
            assert_eq!(store.raw(id)?.as_ref(), Some(raw));
        }
        // Headers are read from every kind.
        assert_eq!(store.backfill_subscriptions()?, 6);
        store.write().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let before = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert_eq!(store.pack_saved_sources()?, 5);
        for (id, raw) in &saved {
            let (kept, source) = columns(&store, id)?;
            assert!(kept.is_none() && source.is_some(), "moved into `source`");
            assert_eq!(store.raw(id)?.as_ref(), Some(raw));
        }
        let mode: i64 = store.read().query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        assert_eq!(mode, 2, "a new database frees pages as it goes");
        // Five sources of some 740 KB each are given back (the text kept
        // in `data` stays).
        let after = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(before - after > 3_000_000, "{before} -> {after}");
        assert_eq!(store.pack_saved_sources()?, 0, "once");
        assert!(!store.compact_on_quit()?, "nothing to rebuild");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// A build from before sources were compressed reads only `raw`: it
    /// finds compressed ones absent, and downloads them again, rather than
    /// reading compressed bytes as mail. What it saves there is read
    /// first, and moved into `source` by the next pass.
    #[test]
    fn older_builds_see_compressed_sources_as_not_downloaded() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-older-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        let (id, raw) = big_source(&store, &a, 1)?;
        store.save_raw(&id, &raw)?;
        let older: Option<Vec<u8>> =
            store.read().query_row("SELECT raw FROM messages WHERE id=?1", [&id], |r| r.get(0))?;
        assert!(older.is_none(), "an older build sees nothing downloaded");
        assert_eq!(store.download_counts(1 << 40, &[(a.id.clone(), 0)])?, (1, 0));
        // Downloaded again by an older build, which knows only `raw`.
        let again = [raw.as_slice(), b"\r\nAgain"].concat();
        store.write().execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, again])?;
        assert_eq!(store.raw(&id)?, Some(again.clone()), "the later copy");
        assert_eq!(store.pack_saved_sources()?, 1);
        assert_eq!(columns(&store, &id)?.0, None);
        assert_eq!(store.raw(&id)?, Some(again));
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    /// A damaged source is an error, not a shorter message; one inflating
    /// past the largest a download may be is damaged. Reading only the
    /// headers stops early and is no error.
    #[test]
    fn damaged_sources_are_errors_not_short_messages() -> Result<()> {
        assert!(unpack(None, Some(b"not deflate at all".to_vec())).is_err());
        let large = pack(&vec![b'a'; 4096])?;
        assert!(inflate(&large, 4095, true).is_err(), "past the limit");
        assert_eq!(inflate(&large, 4096, true)?.len(), 4096);
        assert_eq!(unpack_head(None, Some(large.clone()), 100)?.map(|h| h.len()), Some(100));
        assert_eq!(unpack_head(Some(vec![b'b'; 300]), None, 100)?.map(|h| h.len()), Some(100));
        let legacy = [LEGACY_PACKED, &large].concat();
        assert_eq!(unpack(Some(legacy), None)?.map(|s| s.len()), Some(4096));
        Ok(())
    }

    /// A database made before free pages were given back keeps the space
    /// compressed sources freed, until the rebuild at quitting gives it
    /// back and switches it to giving pages back as it goes. Not while
    /// another write holds the cache, and only once.
    #[test]
    fn an_older_database_is_compacted_once_at_quitting() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-compact-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)?;
        // Made as earlier builds made it: without incremental vacuum.
        Connection::open(path.join("mail.sqlite3"))?.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE messages (id TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, date INTEGER NOT NULL, data TEXT NOT NULL, raw BLOB);",
        )?;
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        for i in 0..30 {
            let (id, raw) = big_source(&store, &a, i)?;
            store.write().execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, raw])?;
        }
        assert_eq!(store.pack_saved_sources()?, 30);
        // Read where the rebuild ran: a reader connection may still report
        // the database's mode as it found it.
        let pragma = |name: &str| -> Result<i64> {
            Ok(store.db.lock().unwrap().query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?)
        };
        assert_eq!(pragma("auto_vacuum")?, 0);
        let free = pragma("freelist_count")?;
        assert!(free * pragma("page_size")? > 16 * 1024 * 1024, "{free} pages free");
        {
            let _writing = store.write();
            assert!(!store.compact_on_quit()?, "another write holds the cache");
        }
        let before = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(store.compact_on_quit()?);
        assert_eq!(pragma("auto_vacuum")?, 2);
        assert_eq!(pragma("freelist_count")?, 0);
        let after = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(after < before / 2, "{before} -> {after}");
        assert!(!store.compact_on_quit()?, "once");
        assert_eq!(store.raw(&format!("{}:7", a.id))?.map(|s| s.len()), Some(big_source_len(7)));
        // Mail deleted later, in a database that gives pages back when
        // asked: they are given back at quitting too.
        let noise = |i: u32| -> Vec<u8> {
            let mut x = 0x9E37_79B9u32 ^ i;
            (0..1_000_000).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x as u8 }).collect()
        };
        for i in 0..30u32 {
            store.save_raw(&format!("{}:{i}", a.id), &noise(i))?;
        }
        store.write().execute("DELETE FROM messages", [])?;
        let before = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(store.compact_on_quit()?);
        assert_eq!(pragma("freelist_count")?, 0);
        let after = std::fs::metadata(path.join("mail.sqlite3"))?.len();
        assert!(after < before / 2, "{before} -> {after}");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }
    fn big_source_len(i: usize) -> usize {
        format!("Subject: Hi {i}\r\nList-Unsubscribe: <https://x/u>\r\n\r\n").len() + 37 * 20_000
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
            (format!("SELECT COUNT(*), COALESCE(SUM({NOT_DOWNLOADED}),0) FROM messages WHERE account='a' AND date >= 0 AND json_extract(data,'$.size') <= 9").as_str(), "COVERING INDEX downloaded"),
            (format!("SELECT data FROM messages WHERE account='a' AND {NOT_DOWNLOADED} AND date >= 0 AND json_extract(data,'$.size') <= 9 ORDER BY date DESC, id DESC LIMIT 5").as_str(), "INDEX downloaded"),
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
