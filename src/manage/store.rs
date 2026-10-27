use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct Store {
    db: Mutex<Connection>,
    pub path: PathBuf,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        let db = Connection::open(path.join("mail.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS accounts (id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS messages (id TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, date INTEGER NOT NULL, data TEXT NOT NULL, raw BLOB);
            CREATE INDEX IF NOT EXISTS mailbox ON messages(account, folder, date DESC);
            CREATE TABLE IF NOT EXISTS drafts (id TEXT PRIMARY KEY, account TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        Ok(Self {
            db: Mutex::new(db),
            path: path.into(),
        })
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT data FROM accounts ORDER BY rowid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn save_account(&self, a: &Account) -> Result<()> {
        // OAuth client secrets are credentials, just like refresh tokens.
        let mut safe = a.clone();
        safe.client_secret.clear();
        self.db.lock().unwrap().execute(
            "INSERT OR REPLACE INTO accounts VALUES (?1,?2)",
            params![a.id, serde_json::to_string(&safe)?],
        )?;
        Ok(())
    }
    pub fn remove_account(&self, id: &str) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM preferences WHERE key=?1",
            [format!("folders:{id}")],
        )?;
        for table in ["accounts", "messages", "drafts"] {
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
        let mailbox = mailbox.into();
        let folder = mailbox.folder();
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT data FROM messages WHERE account=?1 AND (folder=?2 OR (?2='starred' AND json_extract(data,'$.starred'))) ORDER BY date DESC LIMIT ?3")?;
        let rows = q.query_map(
            params![
                account,
                mailbox.key(),
                if folder == Folder::Starred {
                    usize::MAX >> 1
                } else {
                    limit
                }
            ],
            |r| r.get::<_, String>(0),
        )?;
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let mail: Mail = serde_json::from_str(&row?)?;
            if folder != Folder::Starred || mail.starred {
                if folder == Folder::Starred
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
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        write_messages(&tx, messages)?;
        tx.commit()?;
        Ok(())
    }
    /// Commit a completed download and its stable identity together. A failed
    /// write must never leave POP3 thinking a missing body was downloaded.
    pub fn save_download(&self, mail: &Mail, raw: &[u8]) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        write_messages(&tx, std::slice::from_ref(mail))?;
        tx.execute(
            "UPDATE messages SET raw=?2 WHERE id=?1",
            params![mail.id, raw],
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
        let mut db = self.db.lock().unwrap();
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
    pub fn message(&self, id: &str) -> Result<Option<Mail>> {
        use rusqlite::OptionalExtension;
        let json: Option<String> = self
            .db
            .lock()
            .unwrap()
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
        let mut db = self.db.lock().unwrap();
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
    pub fn raw(&self, id: &str) -> Result<Option<Vec<u8>>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT raw FROM messages WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .flatten())
    }
    pub fn save_raw(&self, id: &str, raw: &[u8]) -> Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("UPDATE messages SET raw=?2 WHERE id=?1", params![id, raw])?;
        Ok(())
    }
    /// One accepted remote command is one local transaction. Earlier confirmed
    /// groups remain committed if a later mailbox rejects a bulk operation.
    pub fn commit_changes(&self, messages: &[Mail], delete: bool) -> Result<()> {
        let mut db = self.db.lock().unwrap();
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
    /// Messages whose full content has not been downloaded yet, newest first.
    pub fn missing(&self, account: &str, max_size: usize, limit: usize) -> Result<Vec<Mail>> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare(
            "SELECT data FROM messages WHERE account=?1 AND raw IS NULL
             AND json_extract(data,'$.size') <= ?2 ORDER BY date DESC LIMIT ?3",
        )?;
        q.query_map(params![account, max_size, limit], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    /// Total and not-yet-downloaded message counts across all accounts.
    pub fn download_counts(&self, max_size: usize) -> Result<(usize, usize)> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT COUNT(*), COALESCE(SUM(raw IS NULL),0) FROM messages
             WHERE json_extract(data,'$.size') <= ?1",
            [max_size],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )?)
    }
    /// Unread counts keyed by (account, mailbox key).
    pub fn unread_counts(&self) -> Result<HashMap<(String, String), usize>> {
        let db = self.db.lock().unwrap();
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
    pub fn folders(&self, account: &str) -> Vec<RemoteFolder> {
        self.preference(&format!("folders:{account}"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save_folders(&self, account: &str, folders: &[RemoteFolder]) -> Result<()> {
        self.set_preference(
            &format!("folders:{account}"),
            &serde_json::to_string(folders)?,
        )
    }
    pub fn save_draft(&self, d: &Draft) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT OR REPLACE INTO drafts VALUES (?1,?2,?3)",
            params![d.id, d.account, serde_json::to_string(d)?],
        )?;
        Ok(())
    }
    pub fn drafts(&self, account: &str) -> Result<Vec<Draft>> {
        let db = self.db.lock().unwrap();
        let mut q = db.prepare("SELECT data FROM drafts WHERE account=?1 ORDER BY rowid DESC")?;
        q.query_map([account], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.db
            .lock()
            .unwrap()
            .execute("DELETE FROM drafts WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn preference(&self, key: &str) -> Option<String> {
        self.db
            .lock()
            .ok()?
            .query_row("SELECT value FROM preferences WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .ok()
    }
    pub fn set_preference(&self, key: &str, value: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT OR REPLACE INTO preferences VALUES (?1,?2)",
            params![key, value],
        )?;
        Ok(())
    }
}
pub fn secret_entry(id: &str) -> Result<keyring::Entry> {
    // The service name predates the Picacho name; keep it so saved
    // credentials stay readable.
    keyring::Entry::new("com.pinal.native-mail", id).context("Cannot access the system keychain")
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
        assert_eq!(s.download_counts(1 << 20)?, (10, 9));
        assert_eq!(s.missing("demo", 1 << 20, 100)?.len(), 9);
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
        drop(s);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
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
    for m in messages {
        tx.execute("INSERT INTO messages(id,account,folder,date,data) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET folder=excluded.folder,date=excluded.date,data=excluded.data",params![m.id,m.account,m.mailbox().key(),m.date,serde_json::to_string(m)?])?;
    }
    Ok(())
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::sync::transport::parse_message;

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
