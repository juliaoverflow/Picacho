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
            CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS mailbox_state (account TEXT NOT NULL, remote TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (account, remote));")?;
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
    /// Cached message identities that start with `prefix`.
    pub fn cached_ids(
        &self,
        account: &str,
        prefix: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let db = self.db.lock().unwrap();
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
        let db = self.db.lock().unwrap();
        let data = if summaries {
            "json_remove(data,'$.body','$.html')"
        } else {
            "data"
        };
        let mut q = db.prepare(&format!("SELECT {data} FROM messages WHERE account=?1 AND (folder=?2 OR (?2='starred' AND json_extract(data,'$.starred'))) ORDER BY date DESC, id DESC LIMIT ?3"))?;
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
    /// The subscriptions detected in `accounts`' cached mail.
    pub fn subscriptions(
        &self,
        accounts: &[String],
    ) -> Result<Vec<crate::manage::subscriptions::Subscription>> {
        let db = self.db.lock().unwrap();
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
    /// Works out again the preview (and text, where kept) of cached
    /// messages from their downloaded source, after how mail is read as
    /// text changed (Office markup, character references and invisible
    /// padding used to show in previews). Background downloads keep only a
    /// preview, so it is worked out from the source whenever there is one.
    /// Returns how many changed.
    pub fn refresh_previews(&self) -> Result<usize> {
        let ids: Vec<String> = {
            let db = self.db.lock().unwrap();
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
                let db = self.db.lock().unwrap();
                let mut q = db.prepare("SELECT data, raw FROM messages WHERE id=?1")?;
                let mut rows = Vec::new();
                for id in chunk {
                    if let Some(row) = q
                        .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)))?
                        .next()
                    {
                        let (data, raw) = row?;
                        rows.push((id.clone(), data, raw));
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
            let mut db = self.db.lock().unwrap();
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
        let found: Vec<(String, String, bool)> = {
            let db = self.db.lock().unwrap();
            let mut q = db.prepare(
                "SELECT id, raw FROM messages WHERE raw IS NOT NULL AND json_extract(data,'$.list_unsubscribe') IS NULL AND CAST(substr(raw,1,65536) AS TEXT) LIKE '%list-unsubscribe%'",
            )?;
            let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
            let mut found = Vec::new();
            for row in rows {
                let (id, raw) = row?;
                let Ok((headers, _)) = mailparse::parse_headers(&raw) else {
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
            found
        };
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        for (id, header, one_click) in &found {
            tx.execute(
                "UPDATE messages SET data=json_set(data,'$.list_unsubscribe',?2,'$.one_click',json(?3)) WHERE id=?1",
                params![id, header, if *one_click { "true" } else { "false" }],
            )?;
        }
        tx.commit()?;
        Ok(found.len())
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
    /// Messages dated `since` or later whose full content has not been
    /// downloaded yet, newest first.
    pub fn missing(
        &self,
        account: &str,
        max_size: usize,
        limit: usize,
        since: i64,
    ) -> Result<Vec<Mail>> {
        let db = self.db.lock().unwrap();
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
        let db = self.db.lock().unwrap();
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
        let mut db = self.db.lock().unwrap();
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
        Ok(self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT raw FROM messages WHERE account=?1 AND raw IS NOT NULL
                 AND json_extract(data,'$.gmail_id')=?2 LIMIT 1",
                params![account, gmail_id as i64],
                |r| r.get(0),
            )
            .optional()?)
    }
    /// What the last synchronization of a server mailbox saw.
    pub fn mailbox_state(&self, account: &str, remote: &str) -> Result<Option<MailboxState>> {
        use rusqlite::OptionalExtension;
        let data: Option<String> = self
            .db
            .lock()
            .unwrap()
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
        let db = self.db.lock().unwrap();
        match state {
            Some(state) => db.execute(
                "INSERT OR REPLACE INTO mailbox_state VALUES (?1,?2,?3)",
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
    pub fn apply_changes(
        &self,
        mailbox: &Mailbox,
        account: &str,
        prefix: &str,
        fresh: &[Mail],
        flags: &[(u32, bool, bool)],
        before: Option<i64>,
    ) -> Result<()> {
        ensure!(
            fresh
                .iter()
                .all(|m| m.account == account && m.id.starts_with(prefix)),
            "Invalid mailbox update"
        );
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        write_messages(&tx, fresh)?;
        for (uid, unread, starred) in flags {
            let id = format!("{prefix}{uid}");
            let data: Option<String> = {
                use rusqlite::OptionalExtension;
                tx.query_row("SELECT data FROM messages WHERE id=?1", [&id], |r| r.get(0))
                    .optional()?
            };
            if let Some(data) = data {
                let mut mail: Mail = serde_json::from_str(&data)?;
                if (mail.unread, mail.starred) != (*unread, *starred) {
                    mail.unread = *unread;
                    mail.starred = *starred;
                    write_messages(&tx, &[mail])?;
                }
            }
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
    /// Each unread message's account, mailbox key and recipients.
    pub fn unread_recipients(&self) -> Result<Vec<crate::manage::forwarding::UnreadRow>> {
        let db = self.db.lock().unwrap();
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
