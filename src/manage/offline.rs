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
    pub fn queued_changes(&self, account: &str) -> Vec<QueuedChange> {
        self.store
            .preference(&Self::offline_key(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save_queue(&self, account: &str, queue: &[QueuedChange]) -> Result<()> {
        if queue.is_empty() {
            return self.store.set_preference(&Self::offline_key(account), "[]");
        }
        self.store
            .set_preference(&Self::offline_key(account), &serde_json::to_string(queue)?)
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
        let mut queue = self.queued_changes(&a.id);
        queue.push(QueuedChange {
            mails: pending
                .into_iter()
                .map(|mut m| {
                    m.body = None;
                    m.html = None;
                    m.attachments.clear();
                    m.snippet.clear();
                    m
                })
                .collect(),
            change: change.clone(),
            at: chrono::Utc::now().timestamp(),
        });
        self.save_queue(&a.id, &queue)
    }
    /// Sends queued changes, oldest first, on a connection that works.
    /// Stops (keeping the rest) if the connection fails again; drops
    /// changes the server can't take any more. Callers hold the lock.
    pub(super) fn replay_offline(&self, c: &mut Connection, a: &Account) -> Result<()> {
        let mut queue = self.queued_changes(&a.id);
        if queue.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        while let Some(next) = queue.first().cloned() {
            if now - next.at > KEEP_FOR {
                queue.remove(0);
                continue;
            }
            match self.change_remote(c, a, &next.mails, next.change.clone(), false) {
                Ok(()) => {
                    queue.remove(0);
                }
                Err(e) if crate::sync::retry::later(&e) => {
                    self.save_queue(&a.id, &queue)?;
                    return Err(e);
                }
                // Reset mailbox, removed folder, refused command: it can't
                // be applied as it was made. The next sync shows the
                // server's state.
                Err(_) => {
                    queue.remove(0);
                }
            }
            self.save_queue(&a.id, &queue)?;
        }
        Ok(())
    }
}
