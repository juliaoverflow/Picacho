use crate::manage::model::*;
use crate::manage::vault::{self, DatabaseKey, KeyStore};
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
    /// The preference writer's connection, its thread's own but for
    /// `set_encrypted`, which opens it again on the new file.
    pref_db: Arc<Mutex<Connection>>,
    pref_tx: Mutex<Option<mpsc::Sender<()>>>,
    pref_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// What `widget_scans` last found in each account, with the account's
    /// scan generation then.
    widget_scans: Mutex<HashMap<String, WidgetScan>>,
    /// How many times an account's messages were listed for `parcels` and
    /// `receipts` (`scan_rows`), for tests.
    #[cfg(test)]
    pub(crate) scan_rows_calls: std::sync::atomic::AtomicUsize,
    /// The database's key while it is encrypted (see `vault`).
    key: Mutex<Option<DatabaseKey>>,
    /// Where that key is kept: the system keychain, but in tests.
    keys: Arc<dyn KeyStore>,
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

fn connect(file: &Path, key: Option<&DatabaseKey>) -> Result<Connection> {
    let db = Connection::open(file)?;
    if let Some(key) = key {
        // First, before anything reads the file. A raw key: no passphrase
        // derivation, so opening each connection costs nothing.
        db.execute_batch(&zeroize::Zeroizing::new(format!(
            "PRAGMA key = \"{}\";",
            key.sql_literal().as_str()
        )))?;
        // A wrong key only shows once a page is read.
        db.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0))
            .context(vault::KeyProblem::Wrong)?;
    }
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

/// Makes `db` the writing connection: every write that changes the rows
/// lists are built from marks the cache changed (`list_dirty`).
fn prepare_writer(db: &mut Connection, list_dirty: &Arc<Mutex<HashSet<String>>>) -> Result<()> {
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
    Ok(())
}

/// A reading connection.
fn connect_reader(file: &Path, key: Option<&DatabaseKey>) -> Result<Connection> {
    let r = connect(file, key)?;
    r.execute_batch("PRAGMA query_only=ON;")?;
    Ok(r)
}

impl Store {
    /// Opens the store in `path`, its database decrypted with the key in
    /// the system keychain when it is encrypted.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_keys(path, Arc::new(vault::Keychain))
    }

    /// Opens the store in `path`, keeping its database key (when it has
    /// one, or is given one by `set_encrypted`) in `keys`.
    pub fn open_with_keys(path: &Path, keys: Arc<dyn KeyStore>) -> Result<Self> {
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
        let file = path.join(vault::DATABASE_FILE);
        // What a switch cut short by a crash left; the database it was
        // made from is whole.
        remove_database_files(&path.join(vault::CONVERTING_FILE));
        let key = if vault::is_encrypted(&file)? {
            Some(vault::load_key(path, &*keys)?)
        } else {
            None
        };
        crate::manage::sealed_files::set_key(path, key.as_ref().map(DatabaseKey::file_key));
        // The files beside the database follow its encryption: a switch
        // that a crash or a failure cut short carries on now, whichever
        // state the database is in. Not a reason not to open: it is tried
        // again on the next opening or switch.
        if let Err(e) = crate::manage::sealed_files::resume(path, &*keys, key.as_ref()) {
            eprintln!("Picacho: draft files haven't all followed the database's encryption yet: {e:#}");
        }
        // Partial downloads are raw mail: never kept in the clear beside an
        // encrypted database (`large.rs` seals them), even ones an older
        // version left.
        if key.is_some()
            && let Err(e) = crate::manage::sealed_files::discard_plain_partials(path)
        {
            eprintln!("Picacho: couldn't remove partial downloads: {e}");
        }
        let mut db = connect(&file, key.as_ref())?;
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
            -- What sorting each message into its inbox tab found (see
            -- `save_looks`), kept so it isn't worked out again each launch.
            CREATE TABLE IF NOT EXISTS looks (id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TRIGGER IF NOT EXISTS looks_forget AFTER DELETE ON messages BEGIN DELETE FROM looks WHERE id=OLD.id; END;
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
             -- Copies of one message in several mailboxes by OBJECTID's
             -- EMAILID (RFC 8474), for the same (`email_raw`).
             CREATE INDEX IF NOT EXISTS email_id ON messages(account, {EMAIL_ID}) WHERE {EMAIL_ID} <> '';
             -- A server mailbox's rows, for replacing or forgetting them
             -- (under the write lock, which a scan of the account held).
             CREATE INDEX IF NOT EXISTS remote_folder ON messages(account, json_extract(data,'$.remote_folder'));
             -- A message by its Message-ID (`by_message_id`: replies, a
             -- notification's message, mid: links), starred mail (Starred's
             -- list and count), and downloaded mail with attachments not yet
             -- indexed (`index_attachments`): each scanned every message's
             -- JSON, bodies and all, before.
             CREATE INDEX IF NOT EXISTS message_id ON messages(json_extract(data,'$.message_id'));
             CREATE INDEX IF NOT EXISTS starred ON messages(account, date) WHERE json_extract(data,'$.starred');
             CREATE INDEX IF NOT EXISTS with_attachments ON messages(account) WHERE json_extract(data,'$.has_attachments');"
        ))?;
        // Each account's scan generation (`scan_generation`): changed by
        // every write to its messages that changes what `parcels` and
        // `receipts` look at, in any process, so the widgets know when an
        // account's mail needn't be looked through again. Flags and the like
        // leave it.
        let fields = [
            "$.folder", "$.remote_folder", "$.subject", "$.snippet", "$.from", "$.sender",
            "$.message_id", "$.pending",
        ];
        let mut differs: Vec<String> = fields
            .iter()
            .map(|f| format!("json_extract(OLD.data,'{f}') IS NOT json_extract(NEW.data,'{f}')"))
            .collect();
        differs.push("(json_type(OLD.data,'$.body')='text') IS NOT (json_type(NEW.data,'$.body')='text')".into());
        differs.push(
            "(coalesce(json_extract(OLD.data,'$.html'),'')!='') IS NOT (coalesce(json_extract(NEW.data,'$.html'),'')!='')"
                .into(),
        );
        let bump = |who: &str| {
            format!(
                "INSERT INTO scan_generation (account, generation) VALUES ({who}.account, 1)
                   ON CONFLICT(account) DO UPDATE SET generation = generation + 1;"
            )
        };
        db.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS scan_generation (account TEXT PRIMARY KEY, generation INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TRIGGER IF NOT EXISTS scan_generation_insert AFTER INSERT ON messages BEGIN {new} END;
             CREATE TRIGGER IF NOT EXISTS scan_generation_delete AFTER DELETE ON messages BEGIN {old} END;
             CREATE TRIGGER IF NOT EXISTS scan_generation_update AFTER UPDATE OF account, folder, date, data, raw, source ON messages
               WHEN OLD.account IS NOT NEW.account OR OLD.folder IS NOT NEW.folder OR OLD.date IS NOT NEW.date
                 OR (OLD.raw IS NULL AND OLD.source IS NULL) IS NOT (NEW.raw IS NULL AND NEW.source IS NULL)
                 OR (OLD.data IS NOT NEW.data AND ({differs}))
             BEGIN {new} {old} END;",
            new = bump("NEW"),
            old = bump("OLD"),
            differs = differs.join(" OR "),
        ))?;
        // The local full-text index of messages' subjects, people, text and
        // attachments' text (FTS5, contentless: the text itself stays in
        // `messages`), kept by triggers. Its rows are tied to messages by id
        // in `message_text_ids`: `messages` has no INTEGER PRIMARY KEY, so
        // its rowids change with VACUUM. Attachments' text is read from
        // downloaded sources into `message_attachment_text` (see
        // `index_attachments`), so the triggers can index it again with the
        // rest when a message changes. An index from before attachments were
        // indexed is made again (`index_texts` fills it).
        let indexes_attachments: i64 = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('message_text') WHERE name='attachments'",
            [],
            |r| r.get(0),
        )?;
        if indexes_attachments == 0 {
            db.execute_batch(
                "DROP TRIGGER IF EXISTS message_text_insert;
                 DROP TRIGGER IF EXISTS message_text_delete;
                 DROP TRIGGER IF EXISTS message_text_renamed;
                 DROP TRIGGER IF EXISTS message_text_update;
                 DROP TABLE IF EXISTS message_text;
                 DROP TABLE IF EXISTS message_text_ids;",
            )?;
        }
        db.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS message_text USING fts5(subject, people, body, attachments, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');
             CREATE TABLE IF NOT EXISTS message_text_ids (id TEXT PRIMARY KEY, rid INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS message_text_rid ON message_text_ids(rid);
             CREATE TABLE IF NOT EXISTS message_attachment_text (id TEXT PRIMARY KEY, text TEXT NOT NULL);
             CREATE TRIGGER IF NOT EXISTS message_text_insert AFTER INSERT ON messages BEGIN
               DELETE FROM message_text WHERE rowid=(SELECT rid FROM message_text_ids WHERE id=NEW.id);
               DELETE FROM message_text_ids WHERE id=NEW.id;
               INSERT INTO message_text(subject, people, body, attachments) VALUES ({TEXT_COLUMNS_NEW});
               INSERT INTO message_text_ids VALUES (NEW.id, last_insert_rowid());
             END;
             CREATE TRIGGER IF NOT EXISTS message_text_delete AFTER DELETE ON messages BEGIN
               DELETE FROM message_text WHERE rowid=(SELECT rid FROM message_text_ids WHERE id=OLD.id);
               DELETE FROM message_text_ids WHERE id=OLD.id;
               DELETE FROM message_attachment_text WHERE id=OLD.id;
             END;
             CREATE TRIGGER IF NOT EXISTS message_text_renamed AFTER UPDATE OF id ON messages BEGIN
               UPDATE message_text_ids SET id=NEW.id WHERE id=OLD.id;
               UPDATE OR REPLACE message_attachment_text SET id=NEW.id WHERE id=OLD.id;
             END;
             CREATE TRIGGER IF NOT EXISTS message_text_update AFTER UPDATE OF data ON messages
               WHEN json_extract(OLD.data,'$.body') IS NOT json_extract(NEW.data,'$.body')
                 OR json_extract(OLD.data,'$.subject') IS NOT json_extract(NEW.data,'$.subject')
                 OR json_extract(OLD.data,'$.snippet') IS NOT json_extract(NEW.data,'$.snippet')
             BEGIN
               DELETE FROM message_text WHERE rowid IN (SELECT rid FROM message_text_ids WHERE id IN (OLD.id, NEW.id));
               DELETE FROM message_text_ids WHERE id IN (OLD.id, NEW.id);
               INSERT INTO message_text(subject, people, body, attachments) VALUES ({TEXT_COLUMNS_NEW});
               INSERT INTO message_text_ids VALUES (NEW.id, last_insert_rowid());
             END;"
        ))?;
        // The system's search (Spotlight, Windows Search; see
        // `system_search`): what it was given of each message, and the
        // messages to give it again. Triggers queue a message when it is
        // added, removed, moved, or its words change, and only while that
        // search is on where it is given copies (`SYSTEM_SEARCH_PUSH_PREFERENCE`:
        // Spotlight, Windows Search; Linux's ask Picacho instead), so flags
        // and the like cost nothing. A message queued
        // again goes to the back with a new `seq` (never one used before),
        // so one handed over meanwhile is seen to have changed. Removed and
        // added again rather than `OR REPLACE`: in a trigger, the statement
        // that set it off decides conflicts (the cache's upserts abort).
        db.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS system_search_insert;
             DROP TRIGGER IF EXISTS system_search_delete;
             DROP TRIGGER IF EXISTS system_search_update;
             CREATE TABLE IF NOT EXISTS system_search (id TEXT PRIMARY KEY, account TEXT NOT NULL, name TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS system_search_queue (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE);
             CREATE TRIGGER IF NOT EXISTS system_search_push_insert AFTER INSERT ON messages WHEN {on} BEGIN
               {queue_new}
             END;
             CREATE TRIGGER IF NOT EXISTS system_search_push_delete AFTER DELETE ON messages WHEN {on} BEGIN
               {queue_old}
             END;
             CREATE TRIGGER IF NOT EXISTS system_search_push_update AFTER UPDATE OF id, account, folder, date, data ON messages
               WHEN {on} AND (OLD.id IS NOT NEW.id OR OLD.account IS NOT NEW.account OR OLD.folder IS NOT NEW.folder
                 OR OLD.date IS NOT NEW.date OR (OLD.data IS NOT NEW.data AND ({words})))
             BEGIN
               {queue_new}
               {queue_old}
             END;",
            queue_new = "DELETE FROM system_search_queue WHERE id=NEW.id; INSERT INTO system_search_queue(id) VALUES (NEW.id);",
            queue_old = "DELETE FROM system_search_queue WHERE id=OLD.id; INSERT INTO system_search_queue(id) VALUES (OLD.id);",
            on = SYSTEM_SEARCH_ON,
            words = ["$.subject", "$.snippet", "$.body", "$.from", "$.sender", "$.to", "$.cc", "$.message_id"]
                .iter()
                .map(|f| format!("json_extract(OLD.data,'{f}') IS NOT json_extract(NEW.data,'{f}')"))
                .collect::<Vec<_>>()
                .join(" OR "),
        ))?;
        let list_dirty: Arc<Mutex<HashSet<String>>> = Arc::default();
        prepare_writer(&mut db, &list_dirty)?;
        let readers = (0..READERS)
            .map(|_| Ok(Mutex::new(connect_reader(&file, key.as_ref())?)))
            .collect::<Result<Vec<_>>>()?;
        let prefs = Arc::new(PrefQueue::default());
        let (tx, rx) = mpsc::channel::<()>();
        let pref_db = Arc::new(Mutex::new(connect(&file, key.as_ref())?));
        let writer = pref_db.clone();
        let queue = prefs.clone();
        let thread = std::thread::Builder::new()
            .name("preferences".into())
            .spawn(move || write_preferences(&writer, &queue, rx))?;
        let store = Self {
            db: Mutex::new(db),
            list_dirty,
            list_revisions: Default::default(),
            list_changes: Default::default(),
            readers,
            next_reader: Default::default(),
            prefs,
            pref_db,
            pref_tx: Mutex::new(Some(tx)),
            pref_thread: Mutex::new(Some(thread)),
            widget_scans: Default::default(),
            #[cfg(test)]
            scan_rows_calls: Default::default(),
            key: Mutex::new(key),
            keys,
            path: path.into(),
        };
        // Whatever opens the store (the window, the background process, a
        // phone's service or widget refresh) announces mail with Hide mail
        // outside Picacho as set.
        crate::manage::applock::load_hiding(&store);
        // Whether requests strangers chose may go through the system's
        // proxy (Settings → Reading), for this process's requests.
        crate::sync::public_net::load_proxy_setting(&store);
        Ok(store)
    }
    /// Whether the database is encrypted (Settings → Privacy).
    pub fn encrypted(&self) -> bool {
        self.key.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
    /// Encrypts the database with a new key kept in the keychain (`on`), or
    /// decrypts it and forgets its key.
    ///
    /// The whole database is copied into a new file (`sqlcipher_export`),
    /// which then replaces it; every connection is opened again on it. The
    /// cache is held meanwhile, for as long as copying takes (some seconds
    /// for a large one), and anything wanting it waits. All or nothing: on
    /// a failure, or a crash, the database is as it was. The new key is in
    /// the keychain before the new file replaces the old, and the old key
    /// is forgotten only after.
    ///
    /// The files kept beside the database (`sealed_files`) follow it as a
    /// journaled migration, begun before the database changes, with the
    /// old key kept apart until every file is converted and read back.
    /// Success means the database and every file are as asked; an error
    /// after the database changed leaves the migration pending, and asking
    /// again (the same switch included) or opening the store resumes it.
    pub fn set_encrypted(&self, on: bool) -> Result<()> {
        if on == self.encrypted() {
            return self.finish_file_migration();
        }
        // Preferences waiting to be written go into the copy.
        self.flush();
        let mut held = self.hold_all();
        // Again, holding everything: another switch may have finished while
        // this one waited, and going on would replace its key (and, on a
        // failure, forget the key the database is now encrypted with).
        if on == self.encrypted() {
            drop(held);
            return self.finish_file_migration();
        }
        let file = self.path.join(vault::DATABASE_FILE);
        let converting = self.path.join(vault::CONVERTING_FILE);
        remove_database_files(&converting);
        let old_key = self.key.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // Before anything changes: the files must follow, and the key they
        // may be sealed with now is kept until they have.
        crate::manage::sealed_files::begin(&self.path, &*self.keys, old_key.as_ref())?;
        let new_key = on.then(DatabaseKey::random);
        if let Some(key) = &new_key {
            vault::save_key(&self.path, &*self.keys, key)?;
        }
        let copied = (|| -> Result<()> {
            // Everything in the log goes into the file first, so the copy
            // is all of it.
            // The main database only: the writer's temporary triggers keep
            // its temporary database in use, and checkpointing every
            // database then failed ("database table is locked") right after
            // a switch, so a second switch in one session never worked.
            held.writer.execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE);")?;
            let literal = match &new_key {
                Some(key) => key.sql_literal(),
                None => zeroize::Zeroizing::new(String::new()),
            };
            held.writer.execute(
                "ATTACH DATABASE ?1 AS converted KEY ?2",
                params![converting.to_string_lossy(), literal.as_str()],
            )?;
            let exported = held
                .writer
                .execute_batch(
                    // Gives free pages back as it goes, as `open` sets up;
                    // only takes effect before the copy makes a table.
                    "PRAGMA converted.auto_vacuum=INCREMENTAL;
                     SELECT sqlcipher_export('converted');",
                )
                .with_context(|| t!("manage-store-couldnt-copy-the-local-database"));
            let detached = held.writer.execute_batch("DETACH DATABASE converted;");
            exported?;
            detached?;
            Ok(())
        })();
        if let Err(e) = copied {
            remove_database_files(&converting);
            if new_key.is_some() {
                let _ = vault::forget_key(&self.path, &*self.keys);
            }
            return Err(e);
        }
        // Every connection to the old file is closed before the new one
        // takes its place (Windows can't replace an open file).
        let closed = || Connection::open_in_memory();
        drop(std::mem::replace(&mut *held.writer, closed()?));
        for r in held.readers.iter_mut() {
            drop(std::mem::replace(&mut **r, closed()?));
        }
        drop(std::mem::replace(&mut *held.prefs, closed()?));
        let replaced = (|| -> Result<()> {
            std::fs::rename(&converting, &file).with_context(|| t!("manage-store-couldnt-replace-the-local-database"))?;
            // The old file's log and index, now empty, are not the new one's.
            for suffix in ["-wal", "-shm"] {
                let mut name = file.clone().into_os_string();
                name.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(name));
            }
            Ok(())
        })();
        let key = match &replaced {
            Ok(()) => new_key.clone(),
            Err(_) => old_key.clone(),
        };
        // Open again: the new file, or the old one if it couldn't be replaced.
        let reopened = (|| -> Result<()> {
            let mut db = connect(&file, key.as_ref())?;
            db.execute_batch("PRAGMA journal_mode=WAL;")?;
            prepare_writer(&mut db, &self.list_dirty)?;
            *held.writer = db;
            for r in held.readers.iter_mut() {
                **r = connect_reader(&file, key.as_ref())?;
            }
            *held.prefs = connect(&file, key.as_ref())?;
            Ok(())
        })();
        if let Err(e) = replaced {
            remove_database_files(&converting);
            if new_key.is_some() {
                let _ = vault::forget_key(&self.path, &*self.keys);
            }
            reopened?;
            return Err(e);
        }
        *self.key.lock().unwrap_or_else(|e| e.into_inner()) = key.clone();
        crate::manage::sealed_files::set_key(&self.path, key.as_ref().map(DatabaseKey::file_key));
        drop(held);
        // The old key opens no database now. The files that may still need
        // it have their own copy (`sealed_files::begin`), forgotten only
        // once they are all converted.
        if !on && let Err(e) = vault::forget_key(&self.path, &*self.keys) {
            eprintln!("Picacho: couldn't remove the old database key from the keychain: {e:#}");
        }
        reopened.with_context(|| t!("manage-store-couldnt-open-the-local-database-again"))?;
        // The files kept beside the database follow it: sealed with the new
        // key, or opened again with the old one kept.
        self.finish_file_migration()
    }
    /// Carries a pending migration of the files beside the database to its
    /// encryption as it is now (`sealed_files::resume`): Ok once nothing
    /// is pending.
    fn finish_file_migration(&self) -> Result<()> {
        let key = self.key.lock().unwrap_or_else(|e| e.into_inner()).clone();
        crate::manage::sealed_files::resume(&self.path, &*self.keys, key.as_ref())
    }
    /// Every connection, held: for replacing the file under them. Taken
    /// all at once or not at all, so this never waits holding one that a
    /// thread waiting for another holds (a write that reads, say).
    fn hold_all(&self) -> HeldConnections<'_> {
        loop {
            if let Ok(writer) = self.db.try_lock() {
                let readers: Vec<_> = self.readers.iter().map_while(|r| r.try_lock().ok()).collect();
                if readers.len() == self.readers.len()
                    && let Ok(prefs) = self.pref_db.try_lock()
                {
                    return HeldConnections {
                        writer,
                        readers,
                        prefs,
                    };
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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
            // Sent mail waited on for a reply.
            format!("follow_ups:{id}"),
            format!("follow_ups_gone:{id}"),
            format!("follow_ups_synced:{id}"),
            // Snoozed mail, as shared with other computers.
            format!("snoozed:{id}"),
            format!("snoozed_gone:{id}"),
            format!("snoozed_synced:{id}"),
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
    /// Forgets an account's cached mail, folders and server states, keeping
    /// the account and its drafts (which upload afresh): for an account that
    /// now reaches its mailbox another way, whose messages have other ids.
    pub fn forget_mail(&self, id: &str) -> Result<()> {
        self.flush();
        let mut db = self.write();
        let tx = db.transaction()?;
        for key in [
            format!("folders:{id}"),
            format!("offline_changes:{id}"),
            format!("draft_removals:{id}"),
            format!("folder_order:{id}"),
            format!("gmail_history:{id}"),
        ] {
            tx.execute("DELETE FROM preferences WHERE key=?1", [key])?;
        }
        tx.execute(
            "DELETE FROM preferences WHERE substr(key,1,length(?1))=?1",
            [format!("jmap_state:{id}:")],
        )?;
        for table in ["messages", "mailbox_state", "offline_changes", "parcel_scan", "receipt_scan"] {
            tx.execute(&format!("DELETE FROM {table} WHERE account=?1"), [id])?;
        }
        tx.commit()?;
        drop(db);
        for mut d in self.drafts(id)? {
            if d.remote.take().is_some() {
                d.uploaded = String::new();
                self.save_draft(&d)?;
            }
        }
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
    /// Adds up to `batch` cached messages not yet in the full-text index
    /// (cached before it, or by an earlier build) to it. Returns how many;
    /// 0 once every message is in it.
    pub fn index_texts(&self, batch: usize) -> Result<usize> {
        let mut db = self.write();
        let tx = db.transaction()?;
        let ids: Vec<String> = {
            let mut q = tx.prepare(
                "SELECT m.id FROM messages m LEFT JOIN message_text_ids t ON t.id=m.id WHERE t.id IS NULL LIMIT ?1",
            )?;
            q.query_map([batch as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            index_text(&tx, id)?;
        }
        tx.commit()?;
        Ok(ids.len())
    }
}

/// The system's search (`system_search`), which phones don't have.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
impl Store {
    /// Up to `limit` messages waiting for the system's search, first queued
    /// first: each as it is now and as it was listed there before.
    pub fn system_search_items(&self, limit: usize) -> Result<Vec<SystemSearchItem>> {
        use rusqlite::OptionalExtension;
        let db = self.read();
        let mut queued = db.prepare_cached(
            "SELECT q.id, q.seq, s.account, s.name FROM system_search_queue q
             LEFT JOIN system_search s ON s.id=q.id ORDER BY q.seq LIMIT ?1",
        )?;
        let rows = queued
            .query_map([limit as i64], |r| {
                let listed = match (r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?) {
                    (Some(account), Some(name)) => Some((account, name)),
                    _ => None,
                };
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, listed))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut message = db.prepare_cached(SYSTEM_SEARCH_MESSAGE)?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, queued, listed) in rows {
            let now = message
                .query_row(params![id, SYSTEM_SEARCH_TEXT], system_search_message)
                .optional()?;
            out.push(SystemSearchItem { id, queued, now, listed });
        }
        Ok(out)
    }
    /// Each of `ids` still here, as the system's search shows it, with
    /// at most `text` characters of its text, in their order.
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    pub fn system_search_messages(&self, ids: &[String], text: usize) -> Result<Vec<(String, SystemSearchMessage)>> {
        use rusqlite::OptionalExtension;
        let db = self.read();
        let mut message = db.prepare_cached(SYSTEM_SEARCH_MESSAGE)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(m) = message.query_row(params![id, text as i64], system_search_message).optional()? {
                out.push((id.clone(), m));
            }
        }
        Ok(out)
    }
    /// Records what the system's search now has of queued messages: each
    /// `(id, place in the queue, listed as)`, `None` for nothing. A message
    /// queued again meanwhile (a new place) stays queued.
    pub fn system_search_done(&self, done: &[(String, i64, Option<(String, String)>)]) -> Result<()> {
        if done.is_empty() {
            return Ok(());
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        {
            let mut keep = tx.prepare_cached("INSERT OR REPLACE INTO system_search (id, account, name) VALUES (?1, ?2, ?3)")?;
            let mut forget = tx.prepare_cached("DELETE FROM system_search WHERE id=?1")?;
            let mut dequeue = tx.prepare_cached("DELETE FROM system_search_queue WHERE id=?1 AND seq=?2")?;
            for (id, queued, listed) in done {
                match listed {
                    Some((account, name)) => keep.execute(params![id, account, name])?,
                    None => forget.execute([id])?,
                };
                dequeue.execute(params![id, queued])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Queues every message for the system's search, and every one it
    /// has (as when it is turned on, or its index was lost).
    pub fn system_search_queue_all(&self) -> Result<()> {
        self.write().execute_batch(
            "INSERT OR REPLACE INTO system_search_queue(id) SELECT id FROM messages;
             INSERT OR REPLACE INTO system_search_queue(id) SELECT id FROM system_search;",
        )?;
        Ok(())
    }
    /// How many messages wait for the system's search.
    pub fn system_search_waiting(&self) -> Result<usize> {
        let n: i64 = self
            .read()
            .query_row("SELECT COUNT(*) FROM system_search_queue", [], |r| r.get(0))?;
        Ok(n as usize)
    }
    /// Everything listed in the system's search: `(id, account, name)`.
    pub fn system_search_listed(&self) -> Result<Vec<(String, String, String)>> {
        let db = self.read();
        let mut q = db.prepare("SELECT id, account, name FROM system_search")?;
        let rows = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    /// Forgets what the system's search was given and what it waited for
    /// (once it has been emptied).
    pub fn system_search_forget(&self) -> Result<()> {
        self.write()
            .execute_batch("DELETE FROM system_search; DELETE FROM system_search_queue;")?;
        Ok(())
    }
}

impl Store {
    /// Reads the text of up to `batch` downloaded messages' attachments
    /// that haven't been read yet (`attachment_text::extract`), and indexes
    /// it with the rest of each message. Returns how many messages were
    /// read; 0 once every downloaded message with attachments has been.
    pub fn index_attachments(&self, batch: usize) -> Result<usize> {
        // Read without the write lock: inflating and parsing sources takes
        // a while, and checks and downloads write meanwhile.
        let found: Vec<(String, String)> = {
            let db = self.read();
            let mut q = db.prepare(&format!(
                "SELECT m.id, {SOURCE_COLUMNS} FROM messages m LEFT JOIN message_attachment_text a ON a.id=m.id
                 WHERE a.id IS NULL AND json_extract(m.data,'$.has_attachments') AND {DOWNLOADED_M} LIMIT ?1"
            ))?;
            let rows = q
                .query_map([batch as i64], |r| Ok((r.get::<_, String>(0)?, source_columns(r, 1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(q);
            drop(db);
            rows.into_iter()
                .map(|(id, (raw, source))| {
                    // A source that can't be read has no text to find. Nor
                    // has one whose reading panics: it is kept as read (with
                    // no text) rather than tried, and panicking, every time.
                    let text = unpack(raw, source)
                        .ok()
                        .flatten()
                        .and_then(|raw| {
                            std::panic::catch_unwind(|| crate::manage::attachment_text::extract(&raw)).ok()
                        })
                        .unwrap_or_default();
                    (id, text)
                })
                .collect()
        };
        if found.is_empty() {
            return Ok(0);
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        {
            let mut keep = tx.prepare_cached(
                "INSERT OR REPLACE INTO message_attachment_text (id, text) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM messages WHERE id=?1)",
            )?;
            for (id, text) in &found {
                if keep.execute(params![id, text])? > 0 && !text.is_empty() {
                    index_text(&tx, id)?;
                }
            }
        }
        tx.commit()?;
        Ok(found.len())
    }
    /// Messages whose subject, people, text or attachments' text hold every
    /// one of `words` (each a phrase, its last word's start enough) and
    /// none of `without`, in any account, best matches first: the local
    /// full-text search, which needs no server.
    pub fn search_texts(&self, words: &[String], without: &[String], limit: usize) -> Result<Vec<String>> {
        let Some(query) = fts_query(words, without) else {
            return Ok(Vec::new());
        };
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT t.id FROM message_text f JOIN message_text_ids t ON t.rid=f.rowid WHERE message_text MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let ids = q
            .query_map(params![query, limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
    /// Whether some of `account`'s mail in `mailbox` (any of it, for All
    /// Mail and Starred) has only its headers and preview here: its text
    /// is in no local index, so only a server's search finds it.
    pub fn has_undownloaded(&self, account: &str, mailbox: &Mailbox) -> Result<bool> {
        use rusqlite::OptionalExtension;
        let which = if mailbox.folder().is_virtual() { "1" } else { "folder=?2" };
        Ok(self
            .read()
            .query_row(
                &format!("SELECT 1 FROM messages WHERE account=?1 AND {which} AND {NOT_DOWNLOADED} LIMIT 1"),
                params![account, mailbox.key()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
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
        let which = virtual_listed(folder).unwrap_or("folder=?2");
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
        if let Some(rows) = virtual_listed(mailbox.folder()) {
            // Starred and All Mail count each message once, as they list it
            // (`query_messages`), by the database: reading every message of
            // the account into memory only to count them cost as much as
            // its whole cache, on every sidebar count.
            let n: i64 = self.read().query_row(
                &format!("SELECT COUNT(DISTINCT {LISTED_ONCE}) FROM messages WHERE account=?1 AND {rows}"),
                params![account, mailbox.key()],
                |r| r.get(0),
            )?;
            return Ok(n as usize);
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
        if let Some(rows) = virtual_listed(mailbox.folder()) {
            // As listed (`query_messages`): each message once, as its newest
            // copy, unread or not; by the database, not read whole.
            let db = self.read();
            let mut q = db.prepare_cached(&format!(
                "SELECT id FROM (
                    SELECT id, json_extract(data,'$.unread') AS unread,
                           ROW_NUMBER() OVER (PARTITION BY {LISTED_ONCE} ORDER BY date DESC, id DESC) AS n
                    FROM messages WHERE account=?1 AND {rows}
                 ) WHERE n=1 AND unread ORDER BY id"
            ))?;
            let ids = q
                .query_map(params![account, mailbox.key()], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            return Ok(ids);
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
    /// RFC 3501 §6.3.5 (RFC 9051 §6.3.6): a renamed IMAP folder keeps its
    /// UIDVALIDITY and UIDs, so its cache can follow it instead of being
    /// downloaded again. Re-keys every reference to the folder's messages
    /// (ids `{account}:{old}:{validity}:{uid}`) to `new`, in one
    /// transaction: the messages (id, remote folder, mailbox key, thread
    /// ids), their parcel and receipt findings, the folder's sync state,
    /// drafts answering them, changes queued offline (including moves into
    /// `old`), reading choices, and snoozed mail waiting in it. Nothing
    /// survives naming the old prefix. See docs/design-mailboxid-renames.md.
    pub fn rename_mailbox(&self, account: &str, old: &str, new: &str) -> Result<()> {
        let from = format!("{account}:{old}:");
        let to = format!("{account}:{new}:");
        let end = format!("{from}\u{10FFFF}");
        let rekey = |id: &str| -> Option<String> { id.strip_prefix(&from).map(|rest| format!("{to}{rest}")) };
        let (thread_from, thread_to) = (format!("t:{old}:"), format!("t:{new}:"));
        let mut db = self.write();
        let tx = db.transaction()?;
        // Messages: read whole (their JSON holds the id, folder and thread),
        // rewritten, and the source columns carried over untouched.
        let rows: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM messages WHERE id >= ?2 AND id < ?3 AND account=?1")?;
            q.query_map(params![account, from, end], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, data) in rows {
            let mut m: Mail = serde_json::from_str(&data)?;
            let Some(next) = rekey(&id) else { continue };
            m.id = next.clone();
            if m.remote_folder == old {
                m.remote_folder = new.to_owned();
            }
            if let Some(rest) = m.thread_id().strip_prefix(&thread_from) {
                let thread = format!("{thread_to}{rest}");
                m.set_thread_id(thread);
            }
            tx.execute(
                "UPDATE messages SET id=?2, folder=?3, data=?4 WHERE id=?1",
                params![id, next, m.mailbox().key(), serde_json::to_string(&m)?],
            )?;
        }
        // Parcel and receipt findings: keyed by message id, and naming it
        // in their sightings (`mail_id`).
        for table in ["parcel_scan", "receipt_scan"] {
            let rows: Vec<(String, String)> = {
                let mut q = tx.prepare(&format!("SELECT id, data FROM {table} WHERE id >= ?2 AND id < ?3 AND account=?1"))?;
                q.query_map(params![account, from, end], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (id, data) in rows {
                let Some(next) = rekey(&id) else { continue };
                let data = data.replace(&format!("\"{from}"), &format!("\"{to}"));
                tx.execute(&format!("UPDATE {table} SET id=?2, data=?3 WHERE id=?1"), params![id, next, data])?;
            }
        }
        // The folder's sync state (UIDVALIDITY, MODSEQ, …) moves with it.
        tx.execute(
            "UPDATE OR REPLACE mailbox_state SET remote=?3 WHERE account=?1 AND remote=?2",
            params![account, old, new],
        )?;
        // Drafts answering one of its messages.
        let drafts: Vec<(String, String)> = {
            let mut q = tx.prepare("SELECT id, data FROM drafts WHERE account=?1")?;
            q.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, data) in drafts {
            let mut d: Draft = serde_json::from_str(&data)?;
            if let Some(next) = rekey(&d.original) {
                d.original = next;
                tx.execute("UPDATE drafts SET data=?2 WHERE id=?1", params![id, serde_json::to_string(&d)?])?;
            }
        }
        // Changes queued offline replay against the new name, with the same
        // UIDs; a move into the old name moves into the new one.
        let queued: Vec<(i64, String)> = {
            let mut q = tx.prepare("SELECT seq, data FROM offline_changes WHERE account=?1")?;
            q.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (seq, data) in queued {
            let mut change: crate::manage::backend::offline::QueuedChange = serde_json::from_str(&data)?;
            let mut touched = false;
            for m in &mut change.mails {
                if let Some(next) = rekey(&m.id) {
                    m.id = next;
                    touched = true;
                }
                if m.remote_folder == old {
                    m.remote_folder = new.to_owned();
                    touched = true;
                }
            }
            if let crate::manage::backend::Change::MoveTo(target) = &mut change.change
                && target.as_str() == old
            {
                *target = new.to_owned();
                touched = true;
            }
            if touched {
                tx.execute("UPDATE offline_changes SET data=?2 WHERE seq=?1", params![seq, serde_json::to_string(&change)?])?;
            }
        }
        // Reading choices, keyed by (account, message id).
        let readings: Vec<(String, String)> = {
            let lo = format!("reading:[{},", serde_json::to_string(account)?);
            let hi = format!("{lo}\u{10FFFF}");
            let mut q = tx.prepare("SELECT key, value FROM preferences WHERE key >= ?1 AND key < ?2")?;
            q.query_map(params![lo, hi], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (key, value) in readings {
            let Ok((acct, id)) = serde_json::from_str::<(String, String)>(&key["reading:".len()..]) else { continue };
            if let Some(next) = rekey(&id) {
                tx.execute("DELETE FROM preferences WHERE key=?1", [&key])?;
                tx.execute(
                    "INSERT OR REPLACE INTO preferences VALUES (?1, ?2)",
                    params![crate::manage::reading::message_key(&acct, &next), value],
                )?;
            }
        }
        // Snoozed mail waits in a folder by name.
        let snooze_key = format!("snoozed:{account}");
        let snoozed: Option<String> = {
            use rusqlite::OptionalExtension;
            tx.query_row("SELECT value FROM preferences WHERE key=?1", [&snooze_key], |r| r.get(0)).optional()?
        };
        if let Some(value) = snoozed
            && let Ok(mut list) = serde_json::from_str::<Vec<crate::manage::backend::snooze::Snoozed>>(&value)
            && list.iter().any(|s| s.folder == old)
        {
            for s in &mut list {
                if s.folder == old {
                    s.folder = new.to_owned();
                }
            }
            tx.execute(
                "UPDATE preferences SET value=?2 WHERE key=?1",
                params![snooze_key, serde_json::to_string(&list)?],
            )?;
        }
        tx.commit()?;
        Ok(())
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
            t!("manage-store-the-server-did-not-supply-a")
        );
        let prefix = format!("{account}:{remote}:{validity}:");
        let keep: std::collections::HashSet<_> = ids.iter().collect();
        ensure!(
            messages.iter().all(|m| m.account == *account
                && m.remote_folder == *remote
                && m.id.starts_with(&prefix)
                && keep.contains(&m.id)),
            t!("manage-store-invalid-mailbox-snapshot")
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
    /// Everyone `accounts` have written to: the addresses in their sent
    /// mail's To and Cc, lowercase (`categories::Known`).
    pub fn correspondents(&self, accounts: &[String]) -> Result<HashSet<String>> {
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT json_extract(data,'$.to'), json_extract(data,'$.cc') FROM messages WHERE account=?1 AND folder='sent'",
        )?;
        let mut out = HashSet::new();
        for account in accounts {
            let mut rows = q.query([account])?;
            while let Some(row) = rows.next()? {
                for i in 0..2 {
                    if let Some(list) = row.get_ref(i)?.as_str_or_null()? {
                        out.extend(crate::manage::categories::addresses_in(list));
                    }
                }
            }
        }
        Ok(out)
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
        let mut out = ParcelsFound::default();
        for account in accounts {
            let listed = self.scan_rows(account)?;
            self.parcels_in(account, &listed, model, &mut out)?;
        }
        let ParcelsFound { found, mut to_download, scanned, looked_at, with_parcels } = out;
        to_download.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(ParcelScan {
            tracked: crate::manage::parcels::summarize(found),
            to_download: to_download.into_iter().map(|(_, a, id)| (a, id)).collect(),
            scanned,
            looked_at,
            with_parcels,
        })
    }
    /// The way of reading parcels `parcel_scan` rows were found with.
    fn parcel_version(model: Option<&crate::manage::local_ai::parcel::Confirmer>) -> i64 {
        PARCEL_SCAN * 2 + i64::from(model.is_some())
    }
    /// `parcels` for one account, whose messages are `listed`, added to `out`.
    fn parcels_in(
        &self,
        account: &str,
        listed: &[ScanRow],
        model: Option<&crate::manage::local_ai::parcel::Confirmer>,
        out: &mut ParcelsFound,
    ) -> Result<()> {
        use crate::manage::parcels::{self, MailText, Parcel, Sighting};
        let version = Self::parcel_version(model);
        let download_since = chrono::Utc::now().timestamp() - PARCEL_DOWNLOAD_DAYS * 86_400;
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
        for row in listed {
            let folder = row.folder();
            if row.pending || matches!(folder, Folder::Sent | Folder::Drafts | Folder::Junk) {
                continue;
            }
            out.scanned += 1;
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
                out.to_download.push((row.date, account.to_owned(), row.id.clone()));
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
        for row in listed {
            let Some(list) = results.get(&row.id) else {
                continue;
            };
            out.looked_at.insert(row.id.clone());
            if !list.is_empty() {
                out.with_parcels.insert(row.id.clone());
            }
            for parcel in list {
                out.found.push((
                    parcel.clone(),
                    Sighting {
                        account: account.to_owned(),
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
        Ok(())
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
        let mut out = ReceiptsFound::default();
        for account in accounts {
            let listed = self.scan_rows(account)?;
            self.receipts_in(account, &listed, &mut out)?;
        }
        let ReceiptsFound { found, mut to_download, scanned } = out;
        to_download.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(ReceiptScan {
            listed: crate::manage::receipts::summarize(found),
            to_download: to_download.into_iter().map(|(_, a, id)| (a, id)).collect(),
            scanned,
        })
    }
    /// `receipts` for one account, whose messages are `listed`, added to `out`.
    fn receipts_in(&self, account: &str, listed: &[ScanRow], out: &mut ReceiptsFound) -> Result<()> {
        use crate::manage::parcels::Sighting;
        use crate::manage::receipts::{self, MailText, Receipt};
        let download_since = chrono::Utc::now().timestamp() - PARCEL_DOWNLOAD_DAYS * 86_400;
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
        for row in listed {
            if row.pending || matches!(row.folder(), Folder::Sent | Folder::Drafts | Folder::Junk)
            {
                continue;
            }
            out.scanned += 1;
            let from = row.sender_line();
            if !row.content
                && row.date >= download_since
                && receipts::worth_downloading(&row.subject, &from)
            {
                out.to_download.push((row.date, account.to_owned(), row.id.clone()));
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
        for row in listed {
            let Some(Some(receipt)) = results.remove(&row.id) else {
                continue;
            };
            out.found.push((
                receipt,
                Sighting {
                    account: account.to_owned(),
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
        Ok(())
    }
    /// An account's scan generation: changes with every write, by any
    /// process, that changes what `parcels` and `receipts` look at in its
    /// messages (see the triggers made in `open`).
    fn scan_generation(&self, account: &str) -> Result<i64> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .query_row("SELECT generation FROM scan_generation WHERE account=?1", [account], |r| r.get(0))
            .optional()?
            .unwrap_or(0))
    }
    /// The parcels and receipts of `accounts`' mail, as `parcels` and
    /// `receipts` find them, for the widgets: an account whose scan
    /// generation hasn't changed since the last call (with the same way of
    /// reading parcels) isn't looked through again, and one that has is
    /// listed once for both. What is kept is what was found in the mail;
    /// it is summed up (which depends on the day) on every call.
    pub fn widget_scans(
        &self,
        accounts: &[String],
        model: Option<&crate::manage::local_ai::parcel::Confirmer>,
    ) -> Result<(Vec<crate::manage::parcels::Tracked>, Vec<crate::manage::receipts::Listed>)> {
        let version = Self::parcel_version(model);
        let mut parcels = Vec::new();
        let mut receipts = Vec::new();
        for account in accounts {
            // Read before looking: a change while looking makes the next
            // call look again.
            let generation = self.scan_generation(account)?;
            let kept = self
                .widget_scans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(account)
                .filter(|k| k.generation == generation)
                .cloned();
            let parcels_kept = kept.as_ref().and_then(|k| (k.parcel_version == version).then(|| k.parcels.clone()));
            let receipts_kept = kept.as_ref().map(|k| k.receipts.clone());
            let (found_parcels, found_receipts) = match (parcels_kept, receipts_kept) {
                (Some(p), Some(r)) => (p, r),
                (p, r) => {
                    let listed = self.scan_rows(account)?;
                    let p = match p {
                        Some(p) => p,
                        None => {
                            let mut out = ParcelsFound::default();
                            self.parcels_in(account, &listed, model, &mut out)?;
                            Arc::new(out.found)
                        }
                    };
                    let r = match r {
                        Some(r) => r,
                        None => {
                            let mut out = ReceiptsFound::default();
                            self.receipts_in(account, &listed, &mut out)?;
                            Arc::new(out.found)
                        }
                    };
                    self.widget_scans.lock().unwrap_or_else(|e| e.into_inner()).insert(
                        account.clone(),
                        WidgetScan { generation, parcel_version: version, parcels: p.clone(), receipts: r.clone() },
                    );
                    (p, r)
                }
            };
            parcels.extend(found_parcels.iter().cloned());
            receipts.extend(found_receipts.iter().cloned());
        }
        Ok((crate::manage::parcels::summarize(parcels), crate::manage::receipts::summarize(receipts)))
    }
    /// An account's messages as `parcels` and `receipts` look through
    /// them, without their text.
    fn scan_rows(&self, account: &str) -> Result<Vec<ScanRow>> {
        #[cfg(test)]
        self.scan_rows_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    /// The newest `limit` messages with attachments in `accounts`, without
    /// their text, newest first: in `mailbox` where given, else outside
    /// Drafts, Spam and Trash (`attachments::Library`). Copies of one
    /// message (Gmail's labels, several folders) are listed once. Also how
    /// many of them have attachments not listed yet: only headers were
    /// downloaded, which tell there are attachments but not which.
    pub fn attachment_mail(
        &self,
        accounts: &[String],
        mailbox: Option<&Mailbox>,
        limit: usize,
    ) -> Result<(Vec<Mail>, usize)> {
        // The `with_attachments` index's condition, word for word.
        let which = match mailbox {
            Some(m) => virtual_listed(m.folder()).unwrap_or("folder=?2"),
            None => "(folder NOT IN ('drafts','junk','trash') AND ?2 IS NOT NULL)",
        };
        let key = mailbox.map_or_else(String::new, Mailbox::key);
        let db = self.read();
        let mut q = db.prepare(&format!(
            "SELECT json_remove(data,'$.body','$.html') FROM messages WHERE account=?1 AND json_extract(data,'$.has_attachments') AND {which} ORDER BY date DESC, id DESC LIMIT ?3"
        ))?;
        let mut out = Vec::new();
        let mut unlisted = 0;
        for account in accounts {
            let mut mails = Vec::new();
            let mut rows = q.query(params![account, key, limit as i64])?;
            while let Some(row) = rows.next()? {
                let Ok(mail) = serde_json::from_str::<Mail>(row.get_ref(0)?.as_str()?) else {
                    continue;
                };
                if mail.attachments().is_empty() {
                    unlisted += 1;
                } else {
                    mails.push(mail);
                }
            }
            out.extend(crate::manage::subscriptions::one_copy_each(mails));
        }
        out.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.id.cmp(&a.id)));
        out.truncate(limit);
        Ok((out, unlisted))
    }
    /// The messages whose attachments' text (read by `index_attachments`)
    /// holds every one of `words`, each a phrase, its last word's start
    /// enough: up to `limit` of them, in any account.
    pub fn attachment_text_matches(&self, words: &[String], limit: usize) -> Result<HashSet<String>> {
        let Some(query) = fts_column_query("attachments", words) else {
            return Ok(HashSet::new());
        };
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT t.id FROM message_text f JOIN message_text_ids t ON t.rid=f.rowid WHERE message_text MATCH ?1 LIMIT ?2",
        )?;
        let ids = q
            .query_map(params![query, limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }
    /// The newest `limit` messages from or to `address` in `accounts`,
    /// without their text, outside Drafts, Spam and Trash, each once: what
    /// the reader's contact panel lists. Found through the full-text
    /// index's people (sender, From, To and Cc), then checked address by
    /// address, so `ana@example.com` doesn't find `diana@example.com`.
    pub fn mail_with(&self, address: &str, accounts: &[String], limit: usize) -> Result<Vec<Mail>> {
        let address = address.trim().to_lowercase();
        let Some(query) = fts_column_query("people", std::slice::from_ref(&address)) else {
            return Ok(Vec::new());
        };
        // The phrase, not its start: `ana@example.co` is someone else.
        let query = query.replace("\"*", "\"");
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT json_remove(m.data,'$.body','$.html') FROM message_text f JOIN message_text_ids t ON t.rid=f.rowid JOIN messages m ON m.id=t.id
             WHERE message_text MATCH ?1 AND m.folder NOT IN ('drafts','junk','trash') ORDER BY m.date DESC, m.id DESC LIMIT ?2",
        )?;
        // Some found are only near the address (another account's copy,
        // a similar address): a few more are read than are kept.
        let mut rows = q.query(params![query, (limit * 4).max(40) as i64])?;
        let mut mails = Vec::new();
        while let Some(row) = rows.next()? {
            let Ok(mail) = serde_json::from_str::<Mail>(row.get_ref(0)?.as_str()?) else {
                continue;
            };
            if accounts.contains(&mail.account) && crate::manage::attachments::involves(&mail, &address) {
                mails.push(mail);
            }
        }
        let mut mails = crate::manage::subscriptions::one_copy_each(mails);
        mails.truncate(limit);
        Ok(mails)
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
    /// Every cached copy of `account`'s message with this Message-ID (one
    /// per mailbox it is in), newest first.
    pub fn copies_of(&self, account: &str, message_id: &str) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT data FROM messages WHERE json_extract(data,'$.message_id')=?2 AND account=?1 ORDER BY date DESC",
        )?;
        let rows = q.query_map(params![account, message_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            if let Ok(m) = serde_json::from_str(&json?) {
                out.push(m);
            }
        }
        Ok(out)
    }
    /// `account`'s cached Sent mail dated from `from` to `to` (seconds).
    pub fn sent_between(&self, account: &str, from: i64, to: i64) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT data FROM messages WHERE account=?1 AND folder='sent' AND date BETWEEN ?2 AND ?3",
        )?;
        let rows = q.query_map(params![account, from, to], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            if let Ok(m) = serde_json::from_str(&json?) {
                out.push(m);
            }
        }
        Ok(out)
    }
    /// The server conversation id of `account`'s message with this
    /// Message-ID, where a copy of it has one.
    pub fn thread_of(&self, account: &str, message_id: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .query_row(
                "SELECT json_extract(data,'$.thread_id') FROM messages WHERE json_extract(data,'$.message_id')=?2 AND account=?1 AND coalesce(json_extract(data,'$.thread_id'),'')<>'' LIMIT 1",
                params![account, message_id],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// `account`'s messages dated `since` or later that name `message_id` in
    /// their References or In-Reply-To, or are in server conversation
    /// `thread` (when not empty): the replies to it and the rest of its
    /// conversation, its own copies too (`follow_up`).
    pub fn replies_to(
        &self,
        account: &str,
        message_id: &str,
        thread: &str,
        since: i64,
    ) -> Result<Vec<Mail>> {
        let db = self.read();
        let mut q = db.prepare_cached(
            "SELECT data FROM messages WHERE account=?1 AND date>=?2 AND (instr(coalesce(json_extract(data,'$.references'),''),?3)>0 OR (?4<>'' AND json_extract(data,'$.thread_id')=?4))",
        )?;
        let rows = q.query_map(params![account, since, message_id, thread], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            if let Ok(m) = serde_json::from_str(&json?) {
                out.push(m);
            }
        }
        Ok(out)
    }
    /// Whether message `id`'s whole source is kept here, without reading it.
    pub fn has_source(&self, id: &str) -> Result<bool> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .prepare_cached(&format!("SELECT 1 FROM messages WHERE id=?1 AND {DOWNLOADED}"))?
            .query_row([id], |_| Ok(()))
            .optional()?
            .is_some())
    }
    /// Whether message `id` is cached, without reading it (its JSON holds
    /// its whole text): what sync loops ask of each listed message.
    pub fn has_message(&self, id: &str) -> Result<bool> {
        use rusqlite::OptionalExtension;
        Ok(self
            .read()
            .prepare_cached("SELECT 1 FROM messages WHERE id=?1")?
            .query_row([id], |_| Ok(()))
            .optional()?
            .is_some())
    }
    /// Gives each row `to` the downloaded source row `from` has, as stored
    /// (compressed, not read back): messages whose ids changed (an EWS
    /// move). Both rows must exist.
    pub fn copy_sources(&self, pairs: &[(String, String)]) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        {
            let mut q = tx.prepare_cached(
                "UPDATE messages SET raw=(SELECT raw FROM messages WHERE id=?1), source=(SELECT source FROM messages WHERE id=?1) WHERE id=?2",
            )?;
            for (from, to) in pairs {
                q.execute(params![from, to])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// The earliest date (past 0) of the cached messages among `ids`.
    pub fn oldest_date(&self, ids: &[String]) -> Result<Option<i64>> {
        let db = self.read();
        let mut oldest: Option<i64> = None;
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let found: Option<i64> = db.query_row(
                &format!("SELECT MIN(date) FROM messages WHERE date > 0 AND id IN ({marks})"),
                rusqlite::params_from_iter(chunk.iter()),
                |r| r.get(0),
            )?;
            oldest = match (oldest, found) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        Ok(oldest)
    }
    /// The cached messages among `ids`, whole, by id: one query for a page
    /// of a listing rather than one for each message.
    pub fn messages_by_ids(&self, ids: &[String]) -> Result<HashMap<String, Mail>> {
        let mut out = HashMap::new();
        let db = self.read();
        // SQLite's limit on bound parameters, with room.
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut q = db.prepare(&format!("SELECT data FROM messages WHERE id IN ({marks})"))?;
            let mut rows = q.query(rusqlite::params_from_iter(chunk.iter()))?;
            while let Some(row) = rows.next()? {
                let mail: Mail = serde_json::from_str(row.get_ref(0)?.as_str()?)?;
                out.insert(mail.id.clone(), mail);
            }
        }
        Ok(out)
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
        // The main database only, as in `set_encrypted`.
        self.write().execute_batch("PRAGMA main.wal_checkpoint(TRUNCATE);")?;
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
            db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM; PRAGMA main.wal_checkpoint(TRUNCATE);")
        {
            let week = 7 * 24 * 60 * 60;
            db.execute(PUT_PREFERENCE, params![RETRY, (now + week).to_string()])?;
            return Err(e).with_context(|| t!("manage-store-couldnt-compact-the-mail-cache"));
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
    /// `save_raw` and, where given, the message's row brought up to date
    /// from it, in one transaction: background downloads committed two for
    /// every message. Compressed before the writer is taken.
    pub fn save_raw_with(&self, id: &str, raw: &[u8], row: Option<&Mail>) -> Result<()> {
        let source = pack(raw)?;
        let mut db = self.write();
        let tx = db.transaction()?;
        tx.execute("UPDATE messages SET raw=NULL, source=?2 WHERE id=?1", params![id, source])?;
        if let Some(row) = row {
            write_messages(&tx, std::slice::from_ref(row))?;
        }
        tx.commit()?;
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
    /// The downloaded source of another copy of the message `email_id`
    /// (OBJECTID's EMAILID) in the same account, if any: one id names one
    /// unchanging message across the account's mailboxes (RFC 8474 §5.1).
    /// Only copies downloaded whole have one; an empty id has none.
    pub fn email_raw(&self, account: &str, email_id: &str) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        if email_id.is_empty() {
            return Ok(None);
        }
        let saved = self
            .read()
            .query_row(
                &format!(
                    "SELECT {SOURCE_COLUMNS} FROM messages WHERE account=?1 AND {DOWNLOADED}
                     AND {EMAIL_ID} <> '' AND {EMAIL_ID}=?2 LIMIT 1"
                ),
                params![account, email_id],
                |r| source_columns(r, 0),
            )
            .optional()?;
        match saved {
            Some((raw, source)) => unpack(raw, source),
            None => Ok(None),
        }
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
    /// RFC 8474 §4.1: the account's mailboxes with a known MAILBOXID, as
    /// (name, id), for recognizing a rename made elsewhere.
    pub fn mailbox_ids(&self, account: &str) -> Result<Vec<(String, String)>> {
        let db = self.read();
        let mut q = db.prepare(
            "SELECT remote, json_extract(data,'$.mailbox_id') FROM mailbox_state WHERE account=?1 AND json_extract(data,'$.mailbox_id') <> ''",
        )?;
        let rows = q
            .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
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
            t!("manage-store-invalid-mailbox-update")
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
    /// What sorting messages into the inbox's tabs found in them, kept:
    /// (message id, fingerprint, what was found), as the window's sorter
    /// keeps them (`categories::Sorter::keep`). A message whose fingerprint
    /// changed since is looked at again; a deleted one's is forgotten with it.
    pub fn looks(&self) -> Result<Vec<(String, u64, crate::manage::categories::Content)>> {
        let db = self.read();
        let mut q = db.prepare("SELECT id, data FROM looks")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data) = row?;
            // One an older version wrote differently is looked at again.
            if let Ok((fingerprint, content)) = serde_json::from_str(&data) {
                out.push((id, fingerprint, content));
            }
        }
        Ok(out)
    }
    /// Keeps `looks` (see `looks`), in one write.
    pub fn save_looks(&self, looks: &[(String, u64, crate::manage::categories::Content)]) -> Result<()> {
        if looks.is_empty() {
            return Ok(());
        }
        let mut db = self.write();
        let tx = db.transaction()?;
        {
            let mut q = tx.prepare("INSERT OR REPLACE INTO looks (id, data) SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM messages WHERE id=?1)")?;
            for (id, fingerprint, content) in looks {
                q.execute(params![id, serde_json::to_string(&(fingerprint, content))?])?;
            }
        }
        tx.commit()?;
        Ok(())
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
                    Some(why) => Err(anyhow::anyhow!(t!("manage-store-couldnt-save-this-setting", why = why))),
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
                anyhow::bail!(t!("manage-store-couldnt-save-this-setting-the-cache"));
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
/// What `Store::hold_all` holds.
struct HeldConnections<'a> {
    writer: MutexGuard<'a, Connection>,
    readers: Vec<MutexGuard<'a, Connection>>,
    prefs: MutexGuard<'a, Connection>,
}

/// Removes a database file and its journal, log and index, if there.
fn remove_database_files(file: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = file.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
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

/// What `Store::parcels_in` found, added up over accounts: the parcels
/// with where each was seen, shipping mail to download (date, account,
/// message id), and the messages looked at.
#[derive(Default)]
struct ParcelsFound {
    found: Vec<(crate::manage::parcels::Parcel, crate::manage::parcels::Sighting)>,
    to_download: Vec<(i64, String, String)>,
    scanned: usize,
    looked_at: HashSet<String>,
    with_parcels: HashSet<String>,
}

/// What `Store::widget_scans` found in an account's mail, and the
/// account's scan generation and the way of reading parcels then.
#[derive(Clone)]
struct WidgetScan {
    generation: i64,
    parcel_version: i64,
    parcels: Arc<Vec<(crate::manage::parcels::Parcel, crate::manage::parcels::Sighting)>>,
    receipts: Arc<Vec<(crate::manage::receipts::Receipt, crate::manage::parcels::Sighting)>>,
}

/// What `Store::receipts_in` found, added up over accounts.
#[derive(Default)]
struct ReceiptsFound {
    found: Vec<(crate::manage::receipts::Receipt, crate::manage::parcels::Sighting)>,
    to_download: Vec<(i64, String, String)>,
    scanned: usize,
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
/// A row's EMAILID, written exactly as the `email_id` index has it; queries
/// ask for it `<> ''`, the index's condition, so SQLite uses it.
const EMAIL_ID: &str = "json_extract(data,'$.email_id')";
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
/// The rows a virtual mailbox lists (`query_messages`): `virtual_rows`,
/// and for Starred only those starred.
fn virtual_listed(folder: Folder) -> Option<&'static str> {
    match folder {
        // ?2 named, as every caller passes it; the condition alone is the
        // `starred` index's, so SQLite reads Starred from it.
        Folder::Starred => Some("(json_extract(data,'$.starred') AND ?2 IS NOT NULL)"),
        Folder::All => virtual_rows(folder),
        _ => None,
    }
}
/// What a virtual mailbox lists a message once by: its Message-ID, else its
/// id (as `query_messages` does).
const LISTED_ONCE: &str =
    "CASE WHEN json_extract(data,'$.message_id') <> '' THEN json_extract(data,'$.message_id') ELSE id END";
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
        .with_context(|| t!("manage-store-a-saved-message-source-is-damaged"))?;
    ensure!(
        !whole || raw.len() as u64 <= limit,
        t!("manage-store-a-saved-message-source-is-damaged-2", limit = limit)
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
fn write_preferences(db: &Mutex<Connection>, queue: &PrefQueue, rx: mpsc::Receiver<()>) {
    let write = |batch: &HashMap<String, String>| -> Result<()> {
        let db = db.lock().unwrap_or_else(|e| e.into_inner());
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
    keyring::Entry::new(SECRET_SERVICE, id).with_context(|| t!("manage-store-cannot-access-the-system-keychain"))
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
        anyhow::bail!(t!("manage-store-cannot-access-the-system-keychain-2", error = e));
    }
    keyring_core::Entry::new(SECRET_SERVICE, id).with_context(|| t!("manage-store-cannot-access-the-system-keychain"))
}
pub fn read_secret(id: &str) -> Result<Secret> {
    serde_json::from_str(
        &secret_entry(id)?
            .get_password()
            .with_context(|| t!("manage-store-the-keychain-entry-is-missing-locked"))?,
    )
    .with_context(|| t!("manage-store-saved-credentials-could-not-be-read"))
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
                .with_context(|| t!("manage-store-a-part-of-a-keychain-entry"))?,
        );
    }
    Ok(Some(text))
}
fn read_one_secret(id: &str) -> Result<Option<String>> {
    match secret_entry(id)?.get_password() {
        Ok(text) => Ok(Some(text)),
        Err(KeychainError::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::Error::new(e)
            .context(t!("manage-store-the-keychain-entry-is-locked-or"))),
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
            .with_context(|| t!("manage-store-could-not-securely-save-to-the"))
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
        Err(e) => Err(anyhow::Error::new(e).context(t!("manage-store-could-not-remove-a-keychain-entry"))),
    }
}
pub fn write_secret(id: &str, s: &Secret) -> Result<()> {
    secret_entry(id)?
        .set_password(&serde_json::to_string(s)?)
        .with_context(|| t!("manage-store-could-not-securely-save-credentials-to"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_store() -> Result<(std::path::PathBuf, Store)> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        Ok((p, s))
    }

    /// Starred and All Mail are counted, and their unread found, by the
    /// database as they list (each message once, by its newest copy), not
    /// by reading every message.
    #[test]
    fn virtual_mailboxes_count_as_they_list() -> Result<()> {
        let (p, store) = temp_store()?;
        let mail = |id: &str, folder: Folder, message_id: &str, date: i64, unread: bool, starred: bool| Mail {
            id: id.into(),
            account: id.split(':').next().unwrap_or_default().into(),
            folder,
            message_id: message_id.into(),
            date,
            unread,
            starred,
            subject: id.into(),
            ..Mail::default()
        };
        store.save_messages(&[
            // One message under two labels: its newer copy is read.
            mail("a:inbox:1", Folder::Inbox, "<1@x>", 10, true, true),
            mail("a:work:1", Folder::Archive, "<1@x>", 20, false, true),
            mail("a:inbox:2", Folder::Inbox, "<2@x>", 30, true, false),
            mail("a:inbox:3", Folder::Inbox, "", 40, true, true),
            mail("a:sent:4", Folder::Sent, "<4@x>", 50, false, false),
            mail("a:drafts:5", Folder::Drafts, "<5@x>", 60, true, true),
            mail("b:inbox:6", Folder::Inbox, "<6@x>", 70, true, true),
        ])?;
        for folder in [Folder::Starred, Folder::All] {
            let listed = store.summaries("a", folder)?;
            assert_eq!(store.count_messages("a", folder)?, listed.len(), "{folder:?}");
            let mut unread: Vec<String> = listed.into_iter().filter(|m| m.unread).map(|m| m.id).collect();
            unread.sort();
            assert_eq!(store.unread_ids("a", folder)?, unread, "{folder:?}");
        }
        assert_eq!(store.count_messages("a", Folder::Starred)?, 3);
        assert_eq!(store.count_messages("a", Folder::All)?, 4);
        assert_eq!(store.unread_ids("a", Folder::All)?, ["a:inbox:2", "a:inbox:3"]);
        assert_eq!(store.by_message_id("<2@x>")?.map(|m| m.id).as_deref(), Some("a:inbox:2"));
        drop(store);
        let _ = std::fs::remove_dir_all(p);
        Ok(())
    }

    /// Whether any file in `dir` holds `needle` as it is.
    fn files_hold(dir: &Path, needle: &[u8]) -> Result<bool> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                if files_hold(&path, needle)? {
                    return Ok(true);
                }
                continue;
            }
            let bytes = std::fs::read(path)?;
            if bytes.windows(needle.len()).any(|w| w == needle) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Settings → Privacy: encrypting the database keeps everything in it
    /// (messages, their sources, the search index, preferences, drafts),
    /// leaves none of it readable in its files, opens again only with the
    /// key, and decrypting puts it back as it was and forgets the key.
    #[test]
    fn the_database_is_encrypted_and_decrypted_in_place() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let keys: Arc<vault::MemoryKeys> = Arc::default();
        let store = Store::open_with_keys(&p, keys.clone())?;
        let mail = |id: &str, body: &str| Mail {
            id: id.into(),
            account: "a".into(),
            folder: Folder::Inbox,
            subject: "Itinerary".into(),
            from: "ana@example.com".into(),
            body: Some(body.into()),
            date: 1,
            ..Mail::default()
        };
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        store.save_messages(&[mail("1", "Meet at the zanzibarquay at noon")])?;
        store.save_raw("1", b"From: ana@example.com\r\n\r\nMeet at the zanzibarquay at noon\r\n")?;
        store.set_preference("secretpref", "pinecone-value")?;
        store.save_draft(&Draft { id: "d1".into(), account: "a".into(), ..Draft::default() })?;
        store.set_preference_soon("queued", "still-written");
        assert!(!store.encrypted());
        assert!(files_hold(&p, b"zanzibarquay")?, "a plain database holds its text as is");

        store.set_encrypted(true)?;
        assert!(store.encrypted());
        assert!(vault::is_encrypted(&p.join(vault::DATABASE_FILE))?);
        assert!(!p.join(vault::CONVERTING_FILE).exists());
        // Everything is read as before, and written as before.
        assert_eq!(store.message("1")?.and_then(|m| m.body).as_deref(), Some("Meet at the zanzibarquay at noon"));
        assert!(store.raw("1")?.is_some());
        assert_eq!(store.search_texts(&words(&["zanzibarquay"]), &[], 10)?, vec!["1".to_string()]);
        assert_eq!(store.preference("secretpref").as_deref(), Some("pinecone-value"));
        assert_eq!(store.preference("queued").as_deref(), Some("still-written"));
        assert!(store.drafts("a").is_ok_and(|d| d.iter().any(|d| d.id == "d1")));
        let before = store.list_revision("a");
        store.save_messages(&[mail("2", "A second note about the pinecone")])?;
        assert!(store.list_revision("a") > before, "lists still hear of changes");
        store.set_preference_soon("later", "written-encrypted");
        store.flush();
        store.free_pages()?;
        drop(store);
        assert!(!files_hold(&p, b"zanzibarquay")?, "no file holds the text");
        assert!(!files_hold(&p, b"pinecone")?);
        assert!(!files_hold(&p, b"SQLite format 3")?);
        // Not without the key, nor with another.
        let missing = Store::open_with_keys(&p, Arc::new(vault::MemoryKeys::default()));
        assert_eq!(missing.err().as_ref().and_then(vault::key_problem), Some(vault::KeyProblem::Missing));
        let other: Arc<vault::MemoryKeys> = Arc::default();
        let name = std::fs::read_to_string(p.join(vault::KEY_NAME_FILE))?;
        other.save(&name, &"ab".repeat(32))?;
        let wrong = Store::open_with_keys(&p, other);
        assert_eq!(wrong.err().as_ref().and_then(vault::key_problem), Some(vault::KeyProblem::Wrong));
        assert!(
            Connection::open(p.join(vault::DATABASE_FILE))?
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0))
                .is_err()
        );

        let store = Store::open_with_keys(&p, keys.clone())?;
        assert!(store.encrypted());
        assert_eq!(store.message("2")?.map(|m| m.id).as_deref(), Some("2"));
        assert_eq!(store.preference("later").as_deref(), Some("written-encrypted"));
        // Turning it on again changes nothing.
        store.set_encrypted(true)?;

        store.set_encrypted(false)?;
        // Switched twice in one session (on, then off again).
        store.set_encrypted(true)?;
        assert!(store.encrypted());
        store.set_encrypted(false)?;
        assert!(!store.encrypted());
        assert!(!vault::is_encrypted(&p.join(vault::DATABASE_FILE))?);
        assert_eq!(store.search_texts(&words(&["pinecone"]), &[], 10)?, vec!["2".to_string()]);
        let name = std::fs::read_to_string(p.join(vault::KEY_NAME_FILE))?;
        assert!(keys.load(&name)?.is_none(), "the key is forgotten");
        drop(store);
        // Plain again: opened without any key.
        let store = Store::open_with_keys(&p, Arc::new(vault::MemoryKeys::default()))?;
        assert_eq!(store.preference("secretpref").as_deref(), Some("pinecone-value"));
        let mode: String = store.read().query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        assert_eq!(mode, "wal");
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }

    /// The drafts' files follow the database's encryption as a migration
    /// that lasts failures and crashes: a switch whose files couldn't all
    /// be converted reports it, keeps the old key, and asking again (the
    /// same switch) or opening the store again carries on; partial
    /// downloads (raw mail) never stay beside an encrypted database.
    #[test]
    fn draft_files_follow_the_encryption_through_failures_and_crashes() -> Result<()> {
        use crate::manage::sealed_files;
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let keys: Arc<vault::MemoryKeys> = Arc::default();
        let store = Store::open_with_keys(&p, keys.clone())?;
        let dir = p.join(sealed_files::DIR).join("d");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("budget.txt");
        std::fs::write(&file, b"Secret budget")?;
        let partials = p.join("partial-downloads");
        std::fs::create_dir_all(&partials)?;
        std::fs::write(partials.join("0123"), b"Subject: raw mail")?;
        store.set_encrypted(true)?;
        assert!(!files_hold(&p, b"Secret budget")?, "the draft's file is sealed");
        assert!(!partials.join("0123").exists(), "plain partial downloads are gone");
        assert!(!p.join(sealed_files::JOURNAL).exists());

        // Decrypting with a file no key here opens: an error, the database
        // decrypted, what could be opened opened, the old key kept.
        let stranger = dir.join("stranger.bin");
        std::fs::write(&stranger, b"PICACHO-SEALED1\n0123456789ab0123456789abcdef")?;
        assert!(store.set_encrypted(false).is_err());
        assert!(!store.encrypted());
        assert_eq!(std::fs::read(&file)?, b"Secret budget");
        let journal = std::fs::read_to_string(p.join(sealed_files::JOURNAL))?;
        let kept = journal.lines().nth(1).context("a key kept")?.to_owned();
        assert!(keys.load(&kept)?.is_some());
        // The same switch again doesn't return at once: it tries again.
        assert!(store.set_encrypted(false).is_err());
        std::fs::remove_file(&stranger)?;
        store.set_encrypted(false)?;
        assert!(!p.join(sealed_files::JOURNAL).exists());
        assert!(keys.load(&kept)?.is_none(), "forgotten once every file followed");

        // A crash after the database was encrypted, before its files were:
        // opening the store again seals them.
        store.set_encrypted(true)?;
        drop(store);
        std::fs::write(&file, b"Secret budget")?;
        std::fs::write(p.join(sealed_files::JOURNAL), "picacho draft files migration 1\n")?;
        std::fs::create_dir_all(&partials)?;
        std::fs::write(partials.join("0123"), b"Subject: raw mail")?;
        let store = Store::open_with_keys(&p, keys.clone())?;
        assert!(store.encrypted());
        assert!(!files_hold(&p, b"Secret budget")?, "sealed on opening");
        assert!(!p.join(sealed_files::JOURNAL).exists());
        assert!(!partials.join("0123").exists(), "plain partial downloads are gone on opening");
        assert_eq!(sealed_files::read(&file)?, b"Secret budget");
        drop(store);
        sealed_files::set_key(&p, None);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }

    /// A database whose key is gone is moved aside, not deleted, and a new
    /// one opens in its place.
    #[test]
    fn an_unreadable_database_is_moved_aside_to_start_over() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let keys: Arc<vault::MemoryKeys> = Arc::default();
        let store = Store::open_with_keys(&p, keys.clone())?;
        store.set_preference("old", "yes")?;
        store.set_encrypted(true)?;
        drop(store);
        let none = Arc::new(vault::MemoryKeys::default());
        assert!(Store::open_with_keys(&p, none.clone()).is_err());
        let aside = vault::move_aside(&p)?;
        assert!(aside.exists() && vault::is_encrypted(&aside)?);
        let fresh = Store::open_with_keys(&p, none)?;
        assert!(!fresh.encrypted());
        assert_eq!(fresh.preference("old"), None);
        drop(fresh);
        // With its key, the file moved aside still opens.
        let back = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&back)?;
        std::fs::copy(&aside, back.join(vault::DATABASE_FILE))?;
        let mut name = aside.clone().into_os_string();
        name.push(".keychain");
        std::fs::copy(PathBuf::from(name), back.join(vault::KEY_NAME_FILE))?;
        let restored = Store::open_with_keys(&back, keys)?;
        assert_eq!(restored.preference("old").as_deref(), Some("yes"));
        drop(restored);
        std::fs::remove_dir_all(p)?;
        std::fs::remove_dir_all(back)?;
        Ok(())
    }

    /// A switch cut short by a crash leaves the database as it was: the
    /// half-written copy is removed when the store opens.
    #[test]
    fn a_copy_left_by_a_crash_is_removed() -> Result<()> {
        let (p, store) = temp_store()?;
        store.set_preference("kept", "yes")?;
        drop(store);
        std::fs::write(p.join(vault::CONVERTING_FILE), b"half a database")?;
        let store = Store::open_with_keys(&p, Arc::new(vault::MemoryKeys::default()))?;
        assert!(!p.join(vault::CONVERTING_FILE).exists());
        assert_eq!(store.preference("kept").as_deref(), Some("yes"));
        assert!(!store.encrypted());
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }

    /// RFC 3501 §6.3.5: a renamed folder keeps its UIDs, so its cache
    /// follows it (docs/design-mailboxid-renames.md's test plan): messages
    /// with their downloaded source, findings, sync state, drafts, offline
    /// changes (a move into the folder included), reading choices, thread
    /// ids and snoozes. Afterwards nothing names the old prefix, and mail
    /// of other folders, and of a folder whose name only starts the same,
    /// is untouched.
    #[test]
    fn renamed_folders_keep_their_cache() -> Result<()> {
        let (p, s) = temp_store()?;
        let (old, new) = ("Projects/2026", "Archive/2026");
        let mail = |remote: &str, uid: u32| {
            let mut m = demo_mail().remove(0);
            m.id = format!("demo:{remote}:7:{uid}");
            m.account = "demo".into();
            m.folder = Folder::Other;
            m.remote_folder = remote.into();
            m.uid = uid;
            m
        };
        let mut a = mail(old, 1);
        a.set_thread_id(format!("t:{old}:7:1"));
        let b = mail(old, 2);
        let neighbour = mail("Projects/2026-old", 1);
        let other = mail("INBOX", 3);
        s.save_messages(&[a.clone(), b.clone(), neighbour.clone(), other.clone()])?;
        s.save_raw(&a.id, b"From: a@example.com\r\n\r\nDownloaded.\r\n")?;
        let state = MailboxState { validity: 7, uid_next: 3, exists: 2, modseq: Some(9), days: 30, ..Default::default() };
        s.save_mailbox_state("demo", old, Some(&state))?;
        let sighting = format!(r#"[{{"mail_id":"{}"}}]"#, a.id);
        for table in ["parcel_scan", "receipt_scan"] {
            s.write().execute(
                &format!("INSERT INTO {table} (id, account, state, version, data) VALUES (?1,'demo',1,1,?2)"),
                params![a.id, sighting],
            )?;
        }
        s.save_draft(&Draft { id: "d1".into(), account: "demo".into(), original: b.id.clone(), ..Draft::default() })?;
        let queued = |mails: Vec<Mail>, change| crate::manage::backend::offline::QueuedChange { mails, change, at: 1 };
        use crate::manage::backend::Change;
        s.queue_change("demo", 1, &serde_json::to_string(&queued(vec![a.clone()], Change::Read(true)))?)?;
        s.queue_change("demo", 1, &serde_json::to_string(&queued(vec![other.clone()], Change::MoveTo(old.into())))?)?;
        s.set_preference(&crate::manage::reading::message_key("demo", &b.id), "remote")?;
        let snoozed = vec![crate::manage::backend::snooze::Snoozed {
            message_id: "<s@x>".into(),
            subject: "s".into(),
            sender: "x".into(),
            until: 1,
            folder: old.into(),
            ids: vec![],
            changed: 0,
        }];
        s.set_preference("snoozed:demo", &serde_json::to_string(&snoozed)?)?;

        s.rename_mailbox("demo", old, new)?;

        let a2 = format!("demo:{new}:7:1");
        let moved = s.message(&a2)?.expect("re-keyed");
        assert_eq!((moved.remote_folder.as_str(), moved.mailbox()), (new, Mailbox::Remote(new.into())));
        assert_eq!(moved.thread_id(), format!("t:{new}:7:1"));
        assert!(s.raw(&a2)?.is_some(), "the downloaded source stays: nothing to fetch again");
        assert!(s.message(&format!("demo:{new}:7:2"))?.is_some());
        assert!(s.message(&neighbour.id)?.is_some() && s.message(&other.id)?.is_some());
        assert_eq!(s.mailbox_state("demo", new)?, Some(state));
        assert!(s.mailbox_state("demo", old)?.is_none());
        assert_eq!(s.drafts("demo")?[0].original, format!("demo:{new}:7:2"));
        let changes: Vec<String> = s.queued_changes("demo")?.into_iter().map(|(_, d)| d).collect();
        assert!(changes[0].contains(&a2), "{}", changes[0]);
        assert!(changes[1].contains(&format!("\"MoveTo\":\"{new}\"")), "{}", changes[1]);
        assert_eq!(
            s.preference(&crate::manage::reading::message_key("demo", &format!("demo:{new}:7:2"))).as_deref(),
            Some("remote")
        );
        assert!(s.preference("snoozed:demo").unwrap().contains(new));
        // Nothing names the old prefix anywhere.
        let prefix = format!("demo:{old}:%");
        let db = s.read();
        for (table, column) in [
            ("messages", "id"),
            ("messages", "data"),
            ("parcel_scan", "id"),
            ("parcel_scan", "data"),
            ("receipt_scan", "id"),
            ("receipt_scan", "data"),
            ("drafts", "data"),
            ("offline_changes", "data"),
            ("preferences", "key"),
        ] {
            let n: i64 = db.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} LIKE ?1 OR {column} LIKE ?2"),
                params![prefix, format!("%\"{prefix}")],
                |r| r.get(0),
            )?;
            assert_eq!(n, 0, "{table}.{column} still names {old}");
        }
        drop(db);
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
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
    /// The full-text index follows messages as they are saved, changed and
    /// deleted, and takes in messages cached before it.
    #[test]
    fn message_text_is_searchable_on_this_computer() -> Result<()> {
        let p = std::env::temp_dir().join(format!("picacho-fts-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        let mail = |id: &str, subject: &str, body: &str| Mail {
            id: id.into(),
            account: "a".into(),
            subject: subject.into(),
            from: "ana@example.com".into(),
            body: Some(body.into()),
            date: 1,
            ..Mail::default()
        };
        let words = |w: &[&str]| w.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let search = |w: &[&str]| store.search_texts(&words(w), &[], 10);
        // Text far past the start of a long message is found too.
        let long = format!("{}needle at the end", "filler words here ".repeat(10_000));
        store.save_messages(&[
            mail("1", "Lunch", "Shall we try the ramen place on Friday?"),
            mail("2", "Invoice", "Your invoice for October is attached."),
            mail("3", "Logs", &long),
        ])?;
        assert_eq!(search(&["ramen"])?, vec!["1".to_string()]);
        assert_eq!(search(&["octo"])?, vec!["2".to_string()]);
        assert_eq!(search(&["ana@example.com", "invoice"])?, vec!["2".to_string()]);
        assert_eq!(search(&["needle"])?, vec!["3".to_string()]);
        // A phrase, and words left out.
        assert_eq!(search(&["ramen place"])?, vec!["1".to_string()]);
        assert!(search(&["place ramen"])?.is_empty());
        assert_eq!(
            store.search_texts(&words(&["ana"]), &words(&["ramen", "needle"]), 10)?,
            vec!["2".to_string()]
        );
        // Nothing to look for, or only punctuation: nothing found, no error.
        assert!(search(&[])?.is_empty());
        assert!(search(&["\"*"])?.is_empty());
        // Changed text is found as it is now.
        store.save_messages(&[mail("1", "Lunch", "Pizza instead?")])?;
        assert!(search(&["ramen"])?.is_empty());
        assert_eq!(search(&["pizza"])?, vec!["1".to_string()]);
        store.delete_ids("a", &["1".to_string()])?;
        assert!(search(&["pizza"])?.is_empty());
        // Cached before the index: taken in by `index_texts`.
        store.write().execute_batch("DELETE FROM message_text; DELETE FROM message_text_ids;")?;
        assert!(search(&["invoice"])?.is_empty());
        assert_eq!(store.index_texts(100)?, 2);
        assert_eq!(store.index_texts(100)?, 0);
        assert_eq!(search(&["invoice"])?, vec!["2".to_string()]);
        // Attachments' text, once the message is downloaded.
        let raw = "From: ana@example.com\r\nSubject: Report\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--b\r\nContent-Type: text/csv; name=\"q3.csv\"\r\nContent-Disposition: attachment; filename=\"q3.csv\"\r\n\r\nregion,revenue\r\nPatagonia,1200\r\n--b--\r\n";
        let mut report = mail("4", "Report", "See attached.");
        report.has_attachments = true;
        store.save_messages(&[report.clone()])?;
        assert!(search(&["patagonia"])?.is_empty());
        assert_eq!(store.index_attachments(10)?, 0, "not downloaded yet");
        store.save_raw("4", raw.as_bytes())?;
        assert_eq!(store.index_attachments(10)?, 1);
        assert_eq!(store.index_attachments(10)?, 0, "read once");
        assert_eq!(search(&["patagonia"])?, vec!["4".to_string()]);
        assert_eq!(search(&["q3.csv"])?, vec!["4".to_string()]);
        // Kept when the message changes, gone with it.
        store.save_messages(&[Mail { subject: "Report (final)".into(), ..report }])?;
        assert_eq!(search(&["patagonia", "final"])?, vec!["4".to_string()]);
        store.delete_ids("a", &["4".to_string()])?;
        assert!(search(&["patagonia"])?.is_empty());
        drop(store);
        // An index from before attachments were indexed is made again.
        {
            let db = rusqlite::Connection::open(p.join("mail.sqlite3"))?;
            db.execute_batch(
                "DROP TABLE message_text; CREATE VIRTUAL TABLE message_text USING fts5(subject, people, body, content='', contentless_delete=1);",
            )?;
        }
        let store = Store::open(&p)?;
        assert!(store.index_texts(100)? > 0);
        assert_eq!(store.search_texts(&words(&["invoice"]), &[], 10)?, vec!["2".to_string()]);
        drop(store);
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
    /// RFC 8474 §5.1: copies with one EMAILID share a downloaded source,
    /// in the same account only, found through the `email_id` index.
    #[test]
    fn email_id_copies_share_downloaded_content() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let s = Store::open(&p)?;
        let mut ms = demo_mail();
        for (m, id) in ms.iter_mut().zip(["M1", "M1", "M2", ""]) {
            m.set_email_id(id);
        }
        s.save_messages(&ms[..4])?;
        assert_eq!(s.email_raw("demo", "M1")?, None, "none downloaded yet");
        s.save_raw(&ms[0].id, b"one")?;
        assert_eq!(s.email_raw("demo", "M1")?, Some(b"one".to_vec()));
        assert_eq!(s.email_raw("demo", "M2")?, None);
        assert_eq!(s.email_raw("demo", "")?, None);
        assert_eq!(s.email_raw("other", "M1")?, None);
        let plan: Vec<String> = {
            let db = s.read();
            let mut q = db.prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT {SOURCE_COLUMNS} FROM messages WHERE account=?1 AND {DOWNLOADED} AND {EMAIL_ID} <> '' AND {EMAIL_ID}=?2 LIMIT 1"
            ))?;
            q.query_map(params!["demo", "M1"], |r| r.get::<_, String>(3))?
                .collect::<rusqlite::Result<_>>()?
        };
        assert!(plan.iter().any(|d| d.contains("INDEX email_id")), "{plan:?}");
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
    /// RFC 8474 §4.1: the mailbox's MAILBOXID, where the server has
    /// OBJECTID; how a rename made elsewhere is recognized.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mailbox_id: String,
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
/// What the full-text index takes of a message row (`data`): its subject,
/// the people on it, its whole text (or its preview where the text isn't
/// downloaded), and its attachments' text where it was read
/// (`message_attachment_text`).
const TEXT_COLUMNS: &str = "coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.sender'),'')||' '||coalesce(json_extract(data,'$.from'),'')||' '||coalesce(json_extract(data,'$.to'),'')||' '||coalesce(json_extract(data,'$.cc'),''), coalesce(json_extract(data,'$.body'), json_extract(data,'$.snippet'), ''), coalesce((SELECT text FROM message_attachment_text a WHERE a.id=messages.id),'')";
/// The same, of a trigger's NEW row.
const TEXT_COLUMNS_NEW: &str = "coalesce(json_extract(NEW.data,'$.subject'),''), coalesce(json_extract(NEW.data,'$.sender'),'')||' '||coalesce(json_extract(NEW.data,'$.from'),'')||' '||coalesce(json_extract(NEW.data,'$.to'),'')||' '||coalesce(json_extract(NEW.data,'$.cc'),''), coalesce(json_extract(NEW.data,'$.body'), json_extract(NEW.data,'$.snippet'), ''), coalesce((SELECT text FROM message_attachment_text a WHERE a.id=NEW.id),'')";

/// The preference that turns the system's search on (`system_search`).
pub const SYSTEM_SEARCH_PREFERENCE: &str = "system_search";
/// Set with it where the system's search is given copies of messages
/// (Spotlight, Windows Search), which the store then queues.
pub const SYSTEM_SEARCH_PUSH_PREFERENCE: &str = "system_search_push";
/// Whether messages are queued for the system's search, for the triggers
/// (word for word `SYSTEM_SEARCH_PUSH_PREFERENCE` set to "true").
const SYSTEM_SEARCH_ON: &str = "(SELECT value FROM preferences WHERE key='system_search_push')='true'";
/// The longest text of a message given to the system's search, in characters.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
const SYSTEM_SEARCH_TEXT: i64 = 32 * 1024;

/// A message as the system's search is given it (`Store::system_search_items`).
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemSearchMessage {
    pub account: String,
    /// Its mailbox's key (`Mailbox::key`).
    pub mailbox: String,
    pub date: i64,
    pub subject: String,
    pub from: String,
    pub sender: String,
    pub to: String,
    pub cc: String,
    /// Its text (or preview, when not downloaded), at most `SYSTEM_SEARCH_TEXT`.
    pub text: String,
    pub message_id: String,
}

/// A message as `SystemSearchMessage`, by id (?1), with at most ?2
/// characters of its text.
#[cfg_attr(target_os = "android", allow(dead_code))]
const SYSTEM_SEARCH_MESSAGE: &str = "SELECT account, folder, date,
   coalesce(json_extract(data,'$.subject'),''), coalesce(json_extract(data,'$.from'),''),
   coalesce(json_extract(data,'$.sender'),''), coalesce(json_extract(data,'$.to'),''),
   coalesce(json_extract(data,'$.cc'),''),
   substr(coalesce(json_extract(data,'$.body'), json_extract(data,'$.snippet'), ''), 1, ?2),
   coalesce(json_extract(data,'$.message_id'),'')
 FROM messages WHERE id=?1";

#[cfg_attr(target_os = "android", allow(dead_code))]
fn system_search_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<SystemSearchMessage> {
    Ok(SystemSearchMessage {
        account: r.get(0)?,
        mailbox: r.get(1)?,
        date: r.get(2)?,
        subject: r.get(3)?,
        from: r.get(4)?,
        sender: r.get(5)?,
        to: r.get(6)?,
        cc: r.get(7)?,
        text: r.get(8)?,
        message_id: r.get(9)?,
    })
}

/// A message queued for the system's search: what it is now, and what
/// the system's search was given of it before.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemSearchItem {
    pub id: String,
    /// Its place in the queue, for `Store::system_search_done`.
    pub queued: i64,
    /// The message now; `None` once it is gone.
    pub now: Option<SystemSearchMessage>,
    /// What it was listed as: its account and the name it was given.
    pub listed: Option<(String, String)>,
}

/// `DOWNLOADED`, of the row named `m`.
const DOWNLOADED_M: &str = "(m.raw IS NOT NULL OR m.source IS NOT NULL)";

/// Puts message `id` in the full-text index as it is now, in place of what
/// the index had of it.
fn index_text(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    let mut forget = tx.prepare_cached(
        "DELETE FROM message_text WHERE rowid=(SELECT rid FROM message_text_ids WHERE id=?1)",
    )?;
    forget.execute([id])?;
    let mut add = tx.prepare_cached(&format!(
        "INSERT INTO message_text(subject, people, body, attachments) SELECT {TEXT_COLUMNS} FROM messages WHERE id=?1"
    ))?;
    if add.execute([id])? == 0 {
        tx.execute("DELETE FROM message_text_ids WHERE id=?1", [id])?;
        return Ok(());
    }
    let mut link = tx.prepare_cached("INSERT OR REPLACE INTO message_text_ids VALUES (?1, ?2)")?;
    link.execute(params![id, tx.last_insert_rowid()])?;
    Ok(())
}

/// An FTS5 query for messages with every one of `words` and none of
/// `without`: each a phrase (quoted, so that nothing in it is taken as
/// FTS5's syntax), whose last word may be the start of one. None without
/// any word to look for: FTS5's NOT needs something to take away from.
fn fts_query(words: &[String], without: &[String]) -> Option<String> {
    let phrase = |w: &String| -> Option<String> {
        // Only what the tokenizer would keep counts: a phrase of nothing
        // but punctuation matches nothing, and `""*` is an error.
        w.chars()
            .any(char::is_alphanumeric)
            .then(|| format!("\"{}\"*", w.replace('"', "\"\"")))
    };
    let words: Vec<String> = words.iter().filter_map(phrase).collect();
    if words.is_empty() {
        return None;
    }
    let mut query = words.join(" AND ");
    for w in without.iter().filter_map(phrase) {
        query.push_str(" NOT ");
        query.push_str(&w);
    }
    Some(query)
}

/// A full-text query finding each of `words` (as `fts_query` does) in
/// one `column` of the index only.
fn fts_column_query(column: &str, words: &[String]) -> Option<String> {
    let words: Vec<String> = words
        .iter()
        .filter_map(|w| fts_query(std::slice::from_ref(w), &[]))
        .map(|phrase| format!("{column} : {phrase}"))
        .collect();
    (!words.is_empty()).then(|| words.join(" AND "))
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
        // The pages the moved sources freed are given back (what's kept,
        // the text and its full-text index, stays).
        let freed = (free * pragma("page_size")?) as u64;
        assert!(before.saturating_sub(after) >= freed * 3 / 4, "{before} -> {after}, {freed} free");
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
        let state = MailboxState { validity: 1, uid_next: 9, exists: 8, modseq: Some(3), days: 0, ..Default::default() };
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
