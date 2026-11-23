//! Changes made while offline: flags, labels, marks and moves that couldn't
//! reach the server are shown in the cache at once and queued, per account,
//! in order. The next connection sends them first (before a sync or another
//! change), so the server catches up before anything is read from it.
//!
//! A STORE is the same however often it is sent. A MOVE whose reply was
//! lost may have happened already; sent again it finds no such UIDs and
//! changes nothing. A queued change whose mailbox was reset (UIDVALIDITY)
//! or whose messages are gone can't be applied and is dropped.
use super::*;

/// One queued change and the messages it applies to, as they were when
/// it was made (without their bodies).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QueuedChange {
    pub mails: Vec<Mail>,
    pub change: Change,
    /// When it was made, seconds since the epoch.
    pub at: i64,
}

/// Queued changes older than this are dropped rather than applied: the
/// server has long moved on.
const KEEP_FOR: i64 = 30 * 86_400;

impl Backend {
    fn offline_key(account: &str) -> String {
        format!("offline_changes:{account}")
    }
    /// The changes waiting for a connection, oldest first.
    #[cfg(test)]
    pub fn queued_changes(&self, account: &str) -> Vec<QueuedChange> {
        self.queued_rows(account).into_iter().map(|(_, c)| c).collect()
    }
    /// How many messages changes waiting for a connection are for.
    pub fn queued_messages(&self, account: &str) -> usize {
        self.adopt_saved_queue(account);
        self.store.queued_messages(account)
    }
    /// The queued changes with their numbers in the queue, oldest first. A
    /// change that can't be read (from another version) is let go.
    fn queued_rows(&self, account: &str) -> Vec<(i64, QueuedChange)> {
        self.adopt_saved_queue(account);
        let mut out = Vec::new();
        for (seq, data) in self.store.queued_changes(account).unwrap_or_default() {
            match serde_json::from_str(&data) {
                Ok(change) => out.push((seq, change)),
                Err(_) => {
                    let _ = self.store.dequeue_change(seq);
                }
            }
        }
        out
    }
    /// Moves a queue saved the earlier way (one preference holding them all,
    /// written again whole for every change) into the queue's table, once.
    fn adopt_saved_queue(&self, account: &str) {
        let key = Self::offline_key(account);
        let Some(saved) = self.store.preference(&key) else {
            return;
        };
        let queue: Vec<QueuedChange> = serde_json::from_str(&saved).unwrap_or_default();
        let older: Vec<(String, usize)> = queue
            .iter()
            .filter_map(|c| Some((serde_json::to_string(c).ok()?, c.mails.len())))
            .collect();
        // Ahead of anything queued since: they were made first.
        if self.store.queue_changes_first(account, &older).is_ok() {
            let _ = self.store.remove_preference(&key);
        }
    }
    /// Shows a change that couldn't reach the server in the cache, and
    /// queues what of it the server hasn't confirmed. Callers hold the
    /// account lock.
    pub(super) fn queue_offline(
        &self,
        a: &Account,
        current: &[Mail],
        change: &Change,
    ) -> Result<()> {
        // Chunks the server acknowledged before the connection dropped are
        // committed already; only the rest waits.
        let mut pending = Vec::new();
        for m in current {
            match self.store.message(&m.id)? {
                Some(now) if change.shown_by(&now) && !change.moves() => {}
                // Moved and confirmed: its row is gone.
                None if change.moves() => {}
                _ => pending.push(m.clone()),
            }
        }
        if pending.is_empty() {
            return Ok(());
        }
        self.commit_change(&pending, change.clone(), false)?;
        self.adopt_saved_queue(&a.id);
        // Added as a row of its own: the queue isn't read and written again
        // whole for each change.
        let queued = QueuedChange {
            mails: pending
                .into_iter()
                .map(|mut m| {
                    m.body = None;
                    m.html = None;
                    m.set_attachments(Vec::new());
                    m.snippet.clear();
                    m
                })
                .collect(),
            change: change.clone(),
            at: chrono::Utc::now().timestamp(),
        };
        self.store
            .queue_change(&a.id, queued.mails.len(), &serde_json::to_string(&queued)?)
    }
    /// Sends queued changes to Proton, oldest first, as `replay_offline`
    /// does for IMAP. Callers hold the lock.
    pub(super) fn replay_offline_proton(&self, a: &Account) -> Result<()> {
        self.replay_offline_with(a, |mails, change| self.change_proton(a, mails, change))
    }
    /// Sends queued changes with `apply` (an API's own change), oldest
    /// first. Callers hold the lock.
    pub(super) fn replay_offline_with(
        &self,
        a: &Account,
        apply: impl Fn(&[Mail], &Change) -> Result<()>,
    ) -> Result<()> {
        self.replay_queue(a, |mails, change| apply(mails, change))
    }
    /// Sends `a`'s queued changes with `apply`, oldest first, taking each off
    /// the queue once sent (or once it can't be: too old, or the server
    /// refused it). Stops, keeping it and the rest, at one that fails for
    /// now (the connection).
    fn replay_queue(&self, a: &Account, mut apply: impl FnMut(&[Mail], &Change) -> Result<()>) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        for (seq, next) in self.queued_rows(&a.id) {
            if now - next.at <= KEEP_FOR {
                match apply(&next.mails, &next.change) {
                    Ok(()) => {}
                    Err(e) if crate::sync::retry::later(&e) => return Err(e),
                    // A message or folder that's gone: the change can't be
                    // made as it was. The next sync shows the server's state.
                    Err(_) => {}
                }
            }
            self.store.dequeue_change(seq)?;
        }
        Ok(())
    }
    /// Sends queued changes, oldest first, on a connection that works.
    /// Stops (keeping the rest) if the connection fails again; drops
    /// changes the server can't take any more. Callers hold the lock.
    pub(super) fn replay_offline(&self, c: &mut Connection, a: &Account) -> Result<()> {
        self.replay_queue(a, |mails, change| {
            self.change_remote(c, a, mails, change.clone(), false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::store::Store;

    /// Changes are kept a row each, sent oldest first, and taken off as sent;
    /// one that fails for now stays with those after it. A queue saved the
    /// earlier way (one preference) is moved over.
    #[test]
    fn queued_changes_are_kept_and_sent_in_order() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-offline-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let a = Account::new(Provider::Imap);
        let mails = crate::manage::model::demo_mail();
        let change = |n: usize| QueuedChange {
            mails: mails[..n].to_vec(),
            change: Change::Read(true),
            at: chrono::Utc::now().timestamp(),
        };
        // Saved by an earlier version: one preference with the whole queue.
        b.store.set_preference(&Backend::offline_key(&a.id), &serde_json::to_string(&vec![change(1)])?)?;
        b.store.queue_change(&a.id, 2, &serde_json::to_string(&change(2))?)?;
        b.store.queue_change(&a.id, 3, &serde_json::to_string(&change(3))?)?;
        assert_eq!(b.queued_messages(&a.id), 6);
        assert!(b.store.preference(&Backend::offline_key(&a.id)).is_none(), "moved");
        // The first goes; the second fails for now: it and the third stay.
        let mut sent = Vec::new();
        let result = b.replay_queue(&a, |m, _| {
            sent.push(m.len());
            if m.len() == 3 { Err(anyhow::anyhow!(std::io::Error::from(std::io::ErrorKind::TimedOut))) } else { Ok(()) }
        });
        assert!(result.is_err());
        assert_eq!(sent, [1, 2, 3], "oldest (the moved one) first, stopping at the failure");
        assert_eq!(b.queued_changes(&a.id).iter().map(|c| c.mails.len()).collect::<Vec<_>>(), [3]);
        // Then the rest go.
        b.replay_queue(&a, |_, _| Ok(()))?;
        assert_eq!(b.queued_messages(&a.id), 0);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
