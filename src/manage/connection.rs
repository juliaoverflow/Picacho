//! One signed-in IMAP connection per account, reused between operations.
//!
//! Signing in is the most expensive thing a mail client does to a server,
//! and Gmail limits how often an account may do it. Operations already hold
//! the account lock, so a single connection per account is enough: it is
//! taken out of the pool for an operation and put back when that succeeds.
//! A failed operation drops its connection, since its state is unknown.
use super::*;
use crate::manage::status::Throttled;
use crate::sync::transport::{Extensions, ImapSession, Name, Selected};
use std::time::{Duration, Instant};

/// A connection unused this long is checked with NOOP before it is trusted,
/// so a command that is never replayed (STORE, MOVE) is not sent on a
/// connection the server has already closed.
const CHECK_AFTER: Duration = Duration::from_secs(20);
/// Servers drop idle connections (Gmail after about half an hour); replace
/// ones idle this long instead of waiting for a timeout.
const MAX_IDLE: Duration = Duration::from_secs(10 * 60);
/// Background traffic per Gmail account per day. Gmail allows 2,500 MB of
/// IMAP downloads a day and slows the account to a crawl past it; this
/// leaves room for opening messages and for the user's other mail apps.
pub(super) const GMAIL_DAILY_BUDGET: u64 = 1024 * 1024 * 1024;
/// First pause after the server reports a limit; doubles while it persists.
const FIRST_PAUSE: Duration = Duration::from_secs(10 * 60);
const LONGEST_PAUSE: Duration = Duration::from_secs(4 * 60 * 60);

pub(super) struct Connection {
    pub(super) session: ImapSession,
    extensions: Option<Extensions>,
    names: Option<Vec<Name>>,
    /// The personal namespace's prefix, once asked (NAMESPACE).
    namespace: Option<Option<String>>,
    selected: Option<Selected>,
    used: Instant,
}
impl Connection {
    pub(super) fn new(session: ImapSession) -> Self {
        Self {
            session,
            extensions: None,
            names: None,
            namespace: None,
            selected: None,
            used: Instant::now(),
        }
    }
    pub(super) fn extensions(&mut self) -> Result<Extensions> {
        if let Some(e) = self.extensions {
            return Ok(e);
        }
        let e = crate::sync::transport::extensions(&mut self.session)?;
        self.extensions = Some(e);
        Ok(e)
    }
    /// The server's mailboxes, listed once per connection unless `fresh`.
    pub(super) fn names(&mut self, fresh: bool) -> Result<&[Name]> {
        if fresh || self.names.is_none() {
            let special_use = self.extensions()?.special_use;
            self.names = Some(crate::sync::transport::list(
                &mut self.session,
                special_use,
            )?);
        }
        Ok(self.names.as_deref().unwrap_or_default())
    }
    /// Where the server keeps personal folders, if it says (NAMESPACE).
    pub(super) fn namespace(&mut self) -> Result<Option<String>> {
        if let Some(n) = &self.namespace {
            return Ok(n.clone());
        }
        let n = if self.extensions()?.namespace {
            crate::sync::transport::namespace(&mut self.session)?
        } else {
            None
        };
        self.namespace = Some(n.clone());
        Ok(n)
    }
    /// Refuses, before trying, what this sign-in has no right to do in
    /// `mailbox` on servers that say (ACL's MYRIGHTS). Elsewhere the server
    /// has its say when the command is sent.
    pub(super) fn require_rights(&mut self, mailbox: &str, needed: &str, what: &str) -> Result<()> {
        if !self.extensions()?.acl {
            return Ok(());
        }
        let Some(rights) = crate::sync::transport::my_rights(&mut self.session, mailbox)? else {
            return Ok(());
        };
        if let Some(missing) = crate::sync::transport::lacks_rights(&rights, needed) {
            let label = crate::sync::transport::decode_name(mailbox);
            bail!(
                "This account doesn't have permission to {what} in “{label}” (the server gives it “{rights}”, without “{missing}”). Ask the mailbox's owner or administrator."
            );
        }
        Ok(())
    }
    /// Whether `erase` can remove just `uids` from the selected mailbox: with
    /// UIDPLUS always; without, only if no other message there is already
    /// marked deleted (EXPUNGE would take those too). Checked before any
    /// change, so a refusal changes nothing.
    pub(super) fn can_erase(&mut self, uids: &[u32]) -> Result<()> {
        if self.extensions()?.uidplus {
            return Ok(());
        }
        let marked = self.session.uid_search("DELETED")?;
        ensure!(
            marked.iter().all(|uid| uids.contains(uid)),
            "Other messages in this folder are already marked for deletion, and this server can't remove only some (no UIDPLUS). Remove or unmark them in another mail app first."
        );
        Ok(())
    }
    /// Removes the selected mailbox's messages `uids` for good: marks them
    /// \Deleted and expunges just them (UID EXPUNGE). Without UIDPLUS, a
    /// plain EXPUNGE would also remove others already marked, possibly by
    /// another mail app; then nothing is changed and why is said.
    pub(super) fn erase(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let set = crate::sync::transport::uid_ranges(uids);
        let uidplus = self.extensions()?.uidplus;
        self.can_erase(uids)?;
        self.session.uid_store(&set, "+FLAGS.SILENT (\\Deleted)")?;
        if uidplus {
            self.session.uid_expunge(&set)?;
        } else {
            self.session.expunge()?;
        }
        Ok(())
    }
    /// SELECT, always asking the server again: for when the counts matter.
    pub(super) fn select(&mut self, name: &str) -> Result<Selected> {
        let condstore = self.extensions()?.condstore;
        self.selected = None;
        let selected = crate::sync::transport::select(&mut self.session, name, condstore)?;
        self.selected = Some(selected.clone());
        Ok(selected)
    }
    /// SELECT unless this mailbox is already selected. UIDVALIDITY cannot
    /// change while a mailbox stays selected.
    pub(super) fn selected(&mut self, name: &str) -> Result<Selected> {
        match &self.selected {
            Some(s) if s.name == name => Ok(s.clone()),
            _ => self.select(name),
        }
    }
    /// Leaves `name` if it is the selected mailbox, so it can be deleted:
    /// some servers refuse to delete the mailbox a session has open.
    pub(super) fn leave(&mut self, name: &str) -> Result<()> {
        if self.selected.as_ref().is_some_and(|s| s.name == name) {
            self.selected = None;
            self.session.close()?;
        }
        Ok(())
    }
    /// Selects a cached message's mailbox and checks its generation.
    pub(super) fn select_for(&mut self, mail: &Mail) -> Result<Selected> {
        let selected = self.selected(&mail.remote_folder)?;
        let expected = mail
            .id
            .rsplit(':')
            .nth(1)
            .and_then(|s| s.parse::<u32>().ok())
            .context("Cached message identity is invalid. Refresh this mailbox.")?;
        ensure!(
            expected > 0 && selected.validity == expected,
            "The server reset this mailbox's message identities. Refresh before opening or changing this message."
        );
        Ok(selected)
    }
}

#[derive(Default)]
pub(super) struct Limits {
    /// Background work waits until this time after the server limited us.
    paused: HashMap<String, (i64, Duration)>,
    /// Bytes downloaded in the background today (UTC day), per account.
    traffic: HashMap<String, (i64, u64)>,
}

impl Backend {
    /// Runs `work` on the account's connection, signing in only if there is
    /// no usable one. Callers hold the account lock.
    pub(super) fn with_imap<T>(
        &self,
        a: &Account,
        work: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut connection = match self.pooled(a) {
            Some(c) => c,
            None => {
                let secret = self.credentials(a)?;
                Connection::new(self.imap_login(a, &secret)?)
            }
        };
        let result = work(&mut connection);
        match &result {
            Ok(_) => {
                connection.used = Instant::now();
                // Unsolicited EXISTS/EXPUNGE/FETCH were already acted on (or
                // are re-read by the next SELECT); do not let them pile up.
                while connection.session.unsolicited_responses.try_recv().is_ok() {}
                self.connections
                    .lock()
                    .unwrap()
                    .insert(a.id.clone(), connection);
                self.limits.lock().unwrap().paused.remove(&a.id);
            }
            Err(error) => self.note_limit(a, error),
        }
        result.map_err(|e| match self.throttled_until(&a.id) {
            Some(until) if crate::sync::transport::throttled(&e) => e.context(Throttled {
                until,
                budget: false,
            }),
            _ => e,
        })
    }
    fn pooled(&self, a: &Account) -> Option<Connection> {
        let mut c = self.connections.lock().unwrap().remove(&a.id)?;
        let idle = c.used.elapsed();
        if idle >= MAX_IDLE {
            // Best effort; the server may already have closed it.
            let _ = c.session.logout();
            return None;
        }
        if idle >= CHECK_AFTER && c.session.noop().is_err() {
            return None;
        }
        Some(c)
    }
    /// Signs out of an account's pooled connection, if any.
    pub(super) fn disconnect(&self, id: &str) {
        let pooled = self.connections.lock().unwrap().remove(id);
        if let Some(mut c) = pooled {
            let _ = c.session.logout();
        }
    }
    fn note_limit(&self, a: &Account, error: &anyhow::Error) {
        if !crate::sync::transport::throttled(error) {
            return;
        }
        let mut limits = self.limits.lock().unwrap();
        let pause = limits
            .paused
            .get(&a.id)
            .map(|(_, last)| (*last * 2).min(LONGEST_PAUSE))
            .unwrap_or(FIRST_PAUSE);
        let until = chrono::Utc::now().timestamp() + pause.as_secs() as i64;
        limits.paused.insert(a.id.clone(), (until, pause));
    }
    /// When background work for this account may resume, while it is paused.
    pub fn throttled_until(&self, id: &str) -> Option<i64> {
        let now = chrono::Utc::now().timestamp();
        self.limits
            .lock()
            .unwrap()
            .paused
            .get(id)
            .map(|(until, _)| *until)
            .filter(|until| *until > now)
    }
    /// Background work refuses to run while the server is limiting us, or
    /// once the day's allowance is used.
    pub(super) fn ensure_background(&self, a: &Account) -> Result<()> {
        if let Some(until) = self.throttled_until(&a.id) {
            return Err(anyhow::Error::new(Throttled {
                until,
                budget: false,
            }));
        }
        if !self.within_budget(a) {
            let tomorrow = (chrono::Utc::now().timestamp() / 86_400 + 1) * 86_400;
            return Err(anyhow::Error::new(Throttled {
                until: tomorrow,
                budget: true,
            }));
        }
        Ok(())
    }
    /// Counts background downloads against the account's daily budget.
    pub(super) fn note_traffic(&self, a: &Account, bytes: usize) {
        let day = chrono::Utc::now().timestamp() / 86_400;
        let total = {
            let mut limits = self.limits.lock().unwrap();
            let entry = limits
                .traffic
                .entry(a.id.clone())
                .or_insert_with(|| self.saved_traffic(&a.id, day));
            if entry.0 != day {
                *entry = (day, 0);
            }
            entry.1 += bytes as u64;
            *entry
        };
        let _ = self.store.set_preference(
            &format!("traffic:{}", a.id),
            &format!("{} {}", total.0, total.1),
        );
    }
    fn saved_traffic(&self, id: &str, day: i64) -> (i64, u64) {
        self.store
            .preference(&format!("traffic:{id}"))
            .and_then(|s| {
                let (d, b) = s.split_once(' ')?;
                Some((d.parse().ok()?, b.parse().ok()?))
            })
            .filter(|(d, _)| *d == day)
            .unwrap_or((day, 0))
    }
    /// Whether background downloads for this account are within budget.
    /// Only Gmail publishes (and enforces) a daily download limit.
    pub(super) fn within_budget(&self, a: &Account) -> bool {
        if a.provider != Provider::Gmail {
            return true;
        }
        let day = chrono::Utc::now().timestamp() / 86_400;
        let mut limits = self.limits.lock().unwrap();
        let entry = limits
            .traffic
            .entry(a.id.clone())
            .or_insert_with(|| self.saved_traffic(&a.id, day));
        entry.0 != day || entry.1 < GMAIL_DAILY_BUDGET
    }
}
