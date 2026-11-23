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
/// How long a mailbox's rights (MYRIGHTS) are trusted before asking again.
/// They rarely change, and the server still refuses what isn't allowed.
const RIGHTS_FOR: Duration = Duration::from_secs(5 * 60);

pub(super) struct Connection {
    pub(super) session: ImapSession,
    extensions: Option<Extensions>,
    names: Option<Vec<Name>>,
    /// The personal namespace's prefix, once asked (NAMESPACE).
    namespace: Option<Option<String>>,
    /// Other users' and shared namespaces' prefixes, asked with it.
    others: Vec<String>,
    /// Only subscribed folders are listed (the account's setting).
    pub(super) subscribed_only: bool,
    /// Each folder's counters from the last listing, where the server
    /// sends them with it (LIST-STATUS).
    pub(super) statuses: std::collections::HashMap<String, crate::sync::transport::FolderStatus>,
    /// Rights by mailbox, as MYRIGHTS gave them and when.
    rights: std::collections::HashMap<String, (Option<String>, Instant)>,
    selected: Option<Selected>,
    used: Instant,
    /// What ENABLE turned on (QRESYNC, UTF8=ACCEPT, UIDONLY).
    pub(super) enabled: crate::sync::transport::Enabled,
    /// A copy of what the server sends, when asked (`heard`).
    tap: Option<crate::sync::stream::Tap>,
}
impl Connection {
    pub(super) fn new(session: ImapSession) -> Self {
        Self {
            session,
            extensions: None,
            names: None,
            namespace: None,
            others: Vec::new(),
            subscribed_only: false,
            statuses: std::collections::HashMap::new(),
            rights: std::collections::HashMap::new(),
            selected: None,
            used: Instant::now(),
            enabled: Default::default(),
            tap: None,
        }
    }
    /// Runs `command` (a raw IMAP command) and returns what the server
    /// sent for it, tagged reply included, where the session keeps a copy
    /// (`Tap`); empty otherwise.
    pub(super) fn heard(&mut self, command: &str) -> imap::error::Result<Vec<u8>> {
        if let Some(tap) = &self.tap {
            tap.start();
        }
        let result = self.session.run_command_and_check_ok(command);
        let copy = self.tap.as_ref().and_then(|t| t.take()).unwrap_or_default();
        result.map(|()| copy)
    }
    /// Appends a message with `flags` to `mailbox` (a wire name), returning
    /// what the server sent for it (see `heard`).
    pub(super) fn append_heard(
        &mut self,
        mailbox: &str,
        raw: &[u8],
        flags: &[imap::types::Flag<'_>],
    ) -> imap::error::Result<Vec<u8>> {
        if let Some(tap) = &self.tap {
            tap.start();
        }
        let result = self.session.append_with_flags(mailbox, raw, flags);
        let copy = self.tap.as_ref().and_then(|t| t.take()).unwrap_or_default();
        result.map(|()| copy)
    }
    /// Runs a command with literals (`Part`s, text first), returning what
    /// the server sent for it. Needs the tap, which carries the literals.
    fn with_literals(
        &mut self,
        parts: &[crate::sync::stream::Part<'_>],
    ) -> Result<Vec<u8>> {
        let tap = self
            .tap
            .clone()
            .context("This connection can't send messages that way.")?;
        let line = tap.literals().command(parts);
        tap.start();
        let result = self.session.run_command_and_read_response(&line);
        let copy = tap.take().unwrap_or_default();
        result?;
        Ok(copy)
    }
    /// Appends several messages to `mailbox` (a wire name) in one command
    /// (MULTIAPPEND, RFC 3502), each with `flags`; one APPEND each where
    /// the server doesn't have it. Either all are added or, with
    /// MULTIAPPEND, none. Returns what the server sent for each command.
    pub(super) fn append_many(
        &mut self,
        mailbox: &str,
        messages: &[&[u8]],
        flags: &[imap::types::Flag<'_>],
    ) -> Result<Vec<Vec<u8>>> {
        use crate::sync::stream::Part;
        if messages.len() < 2 || !self.extensions()?.multiappend || self.tap.is_none() {
            return messages
                .iter()
                .map(|raw| Ok(self.append_heard(mailbox, raw, flags)?))
                .collect();
        }
        let flags = flag_list(flags);
        let head = format!("APPEND {}", crate::sync::transport::quote(mailbox)?);
        let each = format!(" ({flags}) ");
        let mut parts = vec![Part::Text(&head)];
        for raw in messages {
            parts.push(Part::Text(&each));
            parts.push(Part::Literal(raw));
        }
        Ok(vec![self.with_literals(&parts)?])
    }
    /// Replaces the message `uid` in the selected mailbox with `raw` in one
    /// step (REPLACE, RFC 8508), into `mailbox` (a wire name; the same one
    /// for a draft). Returns what the server sent, with APPENDUID. `None`
    /// where the server can't, so the caller appends and removes instead.
    pub(super) fn replace(
        &mut self,
        uid: u32,
        mailbox: &str,
        raw: &[u8],
        flags: &[imap::types::Flag<'_>],
    ) -> Result<Option<Vec<u8>>> {
        use crate::sync::stream::Part;
        if !self.extensions()?.replace || self.tap.is_none() {
            return Ok(None);
        }
        let head = format!(
            "UID REPLACE {uid} {} ({}) ",
            crate::sync::transport::quote(mailbox)?,
            flag_list(flags)
        );
        Ok(Some(self.with_literals(&[Part::Text(&head), Part::Literal(raw)])?))
    }
    /// Appends a message made of `before`, then the body of the message
    /// `uid` in `source` (a wire name) as it is on the server, then
    /// `after` (CATENATE, RFC 4469): a forward that includes the original
    /// whole, without downloading and uploading it again. `None` where the
    /// server can't.
    pub(super) fn catenate(
        &mut self,
        mailbox: &str,
        flags: &[imap::types::Flag<'_>],
        before: &[u8],
        source: (&str, u32, u32),
        after: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        use crate::sync::stream::Part;
        if !self.extensions()?.catenate || self.tap.is_none() {
            return Ok(None);
        }
        let (folder, validity, uid) = source;
        let head = format!(
            "APPEND {} ({}) CATENATE (TEXT ",
            crate::sync::transport::quote(mailbox)?,
            flag_list(flags)
        );
        let url = format!(
            " URL \"/{};UIDVALIDITY={validity}/;UID={uid}\" TEXT ",
            url_mailbox(folder)
        );
        Ok(Some(self.with_literals(&[
            Part::Text(&head),
            Part::Literal(before),
            Part::Text(&url),
            Part::Literal(after),
            Part::Text(")"),
        ])?))
    }
    /// An URL another server (the account's SMTP server, for BURL) may
    /// read the message `uid` of `mailbox` (a wire name) by, authorized for
    /// the account's own submission (URLAUTH, RFC 4467: GENURLAUTH with the
    /// `submit+` access).
    pub(super) fn submit_url(
        &mut self,
        host: &str,
        user: &str,
        mailbox: &str,
        validity: u32,
        uid: u32,
    ) -> Result<String> {
        ensure!(self.extensions()?.urlauth, "The server can't authorize URLs (no URLAUTH).");
        ensure!(
            crate::sync::transport::reads_urlauth(),
            "This build can't read the server's URL authorizations."
        );
        let url = format!(
            "imap://{}@{host}/{};UIDVALIDITY={validity}/;UID={uid};urlauth=submit+{}",
            url_mailbox(user),
            url_mailbox(mailbox),
            url_mailbox(user)
        );
        let heard = self.heard(&format!(
            "GENURLAUTH {} INTERNAL",
            crate::sync::transport::quote(&url)?
        ))?;
        authorized_url(&heard).context("The server didn't authorize the URL.")
    }
    /// The server's folder for `folder` (Sent, Archive, Trash, Junk,
    /// Drafts), making it where there is none and the server can mark it
    /// for that use (CREATE-SPECIAL-USE, RFC 6154), so other apps find it
    /// too. Elsewhere a missing folder stays an error.
    pub(super) fn standard_folder(&mut self, provider: Provider, folder: Folder) -> Result<String> {
        let found = crate::sync::transport::resolve_in(self.names(false)?, provider, folder);
        if found.is_ok() || !self.extensions()?.create_special_use {
            return found;
        }
        let (name, special) = match folder {
            Folder::Sent => ("Sent", "\\Sent"),
            Folder::Archive => ("Archive", "\\Archive"),
            Folder::Trash => ("Trash", "\\Trash"),
            Folder::Junk => ("Junk", "\\Junk"),
            Folder::Drafts => ("Drafts", "\\Drafts"),
            _ => return found,
        };
        // In the personal namespace (`INBOX.` on servers that want it).
        let name = format!("{}{name}", self.namespace()?.unwrap_or_default());
        let wire = self.mailbox(&name)?;
        self.session
            .run_command_and_check_ok(format!("CREATE {wire} (USE ({special}))"))
            .with_context(|| format!("The server has no {} folder and didn't make one.", folder.label()))?;
        let _ = self
            .session
            .run_command_and_check_ok(format!("SUBSCRIBE {wire}"));
        crate::sync::transport::resolve_in(self.names(true)?, provider, folder)
    }
    /// The space every mailbox takes, from STATUS=SIZE (RFC 8438), in KiB:
    /// storage used where the server has no QUOTA to say.
    pub(super) fn total_size_kib(&mut self) -> Result<Option<u64>> {
        if !self.extensions()?.status_size || !crate::sync::transport::reads_status_size() {
            return Ok(None);
        }
        let names: Vec<String> = self
            .names(false)?
            .iter()
            .filter(|n| !n.has("Noselect") && !n.has("NonExistent"))
            .map(|n| n.name.clone())
            .collect();
        let mut total: u64 = 0;
        for name in names {
            let wire = self.mailbox(&name)?;
            let heard = self.heard(&format!("STATUS {wire} (SIZE)"))?;
            total += status_size(&heard).unwrap_or(0);
        }
        Ok(Some(total.div_ceil(1024)))
    }
    /// A new session, with QRESYNC, UTF8=ACCEPT and UIDONLY turned on
    /// where the server has them (ENABLE comes before any SELECT).
    pub(super) fn open(session: ImapSession) -> Result<Self> {
        Self::open_tapped(session, None)
    }
    /// As `open`, keeping `tap` to read response codes with (`heard`).
    pub(super) fn open_tapped(
        session: ImapSession,
        tap: Option<crate::sync::stream::Tap>,
    ) -> Result<Self> {
        let mut c = Self::new(session);
        c.tap = tap;
        let ext = c.extensions()?;
        c.enabled = crate::sync::transport::enable(&mut c.session, &ext)?;
        if c.enabled.rev2 {
            c.extensions = Some(ext.as_rev2());
        }
        Ok(c)
    }
    /// A mailbox name (as the cache keeps it) as this session takes it, quoted.
    pub(super) fn mailbox(&self, name: &str) -> Result<String> {
        crate::sync::transport::quote(&self.wire(name))
    }
    /// A mailbox name as this session takes it, for the imap crate's own
    /// commands (which quote it themselves).
    pub(super) fn wire(&self, name: &str) -> String {
        crate::sync::transport::wire_name(name, self.enabled.utf8)
    }
    pub(super) fn extensions(&mut self) -> Result<Extensions> {
        if let Some(e) = self.extensions {
            return Ok(e);
        }
        let e = crate::sync::transport::extensions(&mut self.session)?;
        self.extensions = Some(e);
        Ok(e)
    }
    /// UID SEARCH in the selected mailbox, with ESEARCH where the server
    /// has it (see `uid_search_any`).
    pub(super) fn uid_search(
        &mut self,
        criteria: &str,
    ) -> imap::error::Result<std::collections::HashSet<u32>> {
        let esearch = self.extensions().map(|e| e.esearch).unwrap_or(false);
        crate::sync::transport::uid_search_any(&mut self.session, esearch, criteria)
    }
    /// The server's mailboxes, listed once per connection unless `fresh`.
    pub(super) fn names(&mut self, fresh: bool) -> Result<&[Name]> {
        if fresh || self.names.is_none() {
            let ext = self.extensions()?;
            // Other users' and shared folders, where the server has them.
            self.namespace()?;
            let options = crate::sync::transport::ListOptions {
                special_use: ext.special_use,
                list_status: ext.list_status,
                condstore: ext.condstore,
                subscribed_only: self.subscribed_only,
            };
            let others = self.others.clone();
            let (names, statuses) = crate::sync::transport::list_full(
                &mut self.session,
                &options,
                &others,
                self.enabled.utf8,
            )?;
            self.names = Some(names);
            self.statuses = statuses;
        }
        Ok(self.names.as_deref().unwrap_or_default())
    }
    /// Where the server keeps personal folders, if it says (NAMESPACE).
    pub(super) fn namespace(&mut self) -> Result<Option<String>> {
        if let Some(n) = &self.namespace {
            return Ok(n.clone());
        }
        let n = if self.extensions()?.namespace {
            let (personal, others) = crate::sync::transport::namespaces(&mut self.session)?;
            self.others = others;
            personal
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
        let rights = match self.rights.get(mailbox) {
            Some((rights, at)) if at.elapsed() < RIGHTS_FOR => rights.clone(),
            _ => {
                let wire = self.wire(mailbox);
                let rights = crate::sync::transport::my_rights(&mut self.session, &wire)?;
                self.rights
                    .insert(mailbox.to_owned(), (rights.clone(), Instant::now()));
                rights
            }
        };
        let Some(rights) = rights else {
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
        let marked = self.uid_search("DELETED")?;
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
        let selected =
            crate::sync::transport::select(&mut self.session, name, condstore, self.enabled.utf8)?;
        self.selected = Some(selected.clone());
        Ok(selected)
    }
    /// SELECT that resynchronizes (QRESYNC) from a saved state: also says
    /// which UIDs were removed and which messages' flags changed since.
    pub(super) fn select_resync(
        &mut self,
        name: &str,
        validity: u32,
        modseq: u64,
    ) -> Result<(Selected, Option<crate::sync::transport::Resync>)> {
        self.selected = None;
        let (selected, resync) = crate::sync::transport::select_qresync(
            &mut self.session,
            name,
            self.enabled.utf8,
            validity,
            modseq,
        )?;
        self.selected = Some(selected.clone());
        Ok((selected, resync))
    }
    /// The mailbox selected now, if any.
    pub(super) fn current(&self) -> Option<&Selected> {
        self.selected.as_ref()
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
            // UNSELECT leaves without expunging; CLOSE would remove messages
            // another app marked \Deleted but meant to keep for now.
            if self.extensions()?.unselect {
                self.session.run_command_and_check_ok("UNSELECT")?;
            } else {
                self.session.close()?;
            }
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
                let (session, tap) = crate::sync::transport::imap_login_tapped(
                    a,
                    &secret,
                    &self.tls_connector()?,
                )?;
                Connection::open_tapped(session, Some(tap))?
            }
        };
        // The account's choice of folders; a change lists them again.
        if connection.subscribed_only != a.sync.subscribed_only {
            connection.subscribed_only = a.sync.subscribed_only;
            connection.names = None;
        }
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
        // A JMAP session holds its credentials; a new sign-in makes a new one.
        self.jmap.lock().unwrap().remove(id);
        let pooled = self.connections.lock().unwrap().remove(id);
        if let Some(mut c) = pooled {
            let _ = c.session.logout();
        }
    }
    pub(super) fn note_limit(&self, a: &Account, error: &anyhow::Error) {
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

/// Flags as APPEND writes them, without \\Recent (servers set it).
fn flag_list(flags: &[imap::types::Flag<'_>]) -> String {
    flags
        .iter()
        .filter(|f| **f != imap::types::Flag::Recent)
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A mailbox name in an IMAP URL (RFC 5092): percent-encoded where it
/// isn't plain.
fn url_mailbox(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,=:@".contains(&b) || b == b'/' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// SIZE in a STATUS reply: `* STATUS "Sent" (SIZE 123456)`.
fn status_size(heard: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(heard).to_ascii_uppercase();
    let at = text.find("(SIZE ").or_else(|| text.find(" SIZE "))?;
    let rest = &text[at + 6..];
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// The URL in a GENURLAUTH reply: `* GENURLAUTH "imap://…:internal:…"`.
fn authorized_url(heard: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(heard);
    let line = text
        .lines()
        .find(|l| l.to_ascii_uppercase().starts_with("* GENURLAUTH "))?;
    let rest = line["* GENURLAUTH ".len()..].trim();
    let url = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(rest);
    url.starts_with("imap://").then(|| url.replace("\\\"", "\"").replace("\\\\", "\\"))
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn imap_urls_and_their_authorization() {
        assert_eq!(url_mailbox("Sent Items"), "Sent%20Items");
        assert_eq!(url_mailbox("INBOX/Work"), "INBOX/Work");
        assert_eq!(url_mailbox("ana@example.com"), "ana@example.com");
        let heard = b"* GENURLAUTH \"imap://ana@mail.example/Sent;UIDVALIDITY=7/;UID=3;urlauth=submit+ana:internal:91354a473744909de610943775f92038\"\r\na5 OK done\r\n";
        assert_eq!(
            authorized_url(heard).as_deref(),
            Some("imap://ana@mail.example/Sent;UIDVALIDITY=7/;UID=3;urlauth=submit+ana:internal:91354a473744909de610943775f92038")
        );
        assert_eq!(authorized_url(b"a5 NO no\r\n"), None);
        assert_eq!(status_size(b"* STATUS \"Sent Items\" (SIZE 44827)\r\na3 OK\r\n"), Some(44827));
        assert_eq!(status_size(b"* STATUS INBOX (MESSAGES 3 SIZE 9)\r\n"), Some(9));
        assert_eq!(status_size(b"a3 OK\r\n"), None);
        assert_eq!(flag_list(&[imap::types::Flag::Seen, imap::types::Flag::Recent]), "\\Seen");
    }
}
