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
    /// The server's namespaces, once asked (NAMESPACE; empty where it has
    /// none or its reply couldn't be used).
    namespaces: Option<crate::sync::transport::Namespaces>,
    /// Only subscribed folders are listed (the account's setting).
    pub(super) subscribed_only: bool,
    /// Each folder's counters from the last listing, where the server
    /// sends them with it (LIST-STATUS).
    pub(super) statuses: std::collections::HashMap<String, crate::sync::transport::FolderStatus>,
    /// Rights by mailbox, as MYRIGHTS gave them and when.
    rights: std::collections::HashMap<String, (Option<String>, Instant)>,
    /// Each mailbox's own APPENDLIMIT, once asked (`None`: no limit there).
    append_limits: std::collections::HashMap<String, Option<u64>>,
    selected: Option<Selected>,
    used: Instant,
    /// What ENABLE turned on (QRESYNC, UTF8=ACCEPT, UIDONLY).
    pub(super) enabled: crate::sync::transport::Enabled,
    /// A copy of what the server sends, when asked (`heard`).
    tap: Option<crate::sync::stream::Tap>,
    /// `[ALERT]`s the server sent that haven't been shown yet.
    pub(super) alerts: Vec<String>,
}
impl Connection {
    pub(super) fn new(session: ImapSession) -> Self {
        Self {
            session,
            extensions: None,
            names: None,
            namespaces: None,
            subscribed_only: false,
            statuses: std::collections::HashMap::new(),
            rights: std::collections::HashMap::new(),
            append_limits: std::collections::HashMap::new(),
            selected: None,
            used: Instant::now(),
            enabled: Default::default(),
            tap: None,
            alerts: Vec::new(),
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
        self.alerts.extend(crate::sync::transport::alerts_in(&copy));
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
    /// As `append_heard`, keeping the message's own date as when it
    /// arrived (APPEND's date-time, RFC 3501 §6.3.11): imported mail sorts
    /// by when it was sent, not when it was imported. `received` (UTC
    /// seconds; an mbox `From ` line's) is used where it has no usable Date.
    pub(super) fn append_dated(
        &mut self,
        mailbox: &str,
        raw: &[u8],
        flags: &[imap::types::Flag<'_>],
        received: Option<i64>,
    ) -> imap::error::Result<Vec<u8>> {
        if let Some(tap) = &self.tap {
            tap.start();
        }
        let result = self
            .session
            .append_with_flags_and_date(mailbox, raw, flags, append_date(raw, received));
        let copy = self.tap.as_ref().and_then(|t| t.take()).unwrap_or_default();
        result.map(|()| copy)
    }
    /// What sends this session's synchronizing literals (see
    /// `run_with_literals`), where it has the tap that carries them.
    pub(super) fn literals(&self) -> Option<crate::sync::stream::Literals> {
        self.tap.as_ref().map(|t| t.literals())
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
    /// (MULTIAPPEND, RFC 3502), each with its flags and dated as
    /// `append_dated` dates it; one APPEND each where the server doesn't
    /// have it. Either all are added or, with MULTIAPPEND, none. Returns
    /// what the server sent for each command.
    pub(super) fn append_many(
        &mut self,
        mailbox: &str,
        messages: &[(&[u8], &[imap::types::Flag<'_>], Option<i64>)],
    ) -> Result<Vec<Vec<u8>>> {
        use crate::sync::stream::Part;
        if messages.len() < 2 || !self.extensions()?.multiappend || self.tap.is_none() {
            return messages
                .iter()
                .map(|(raw, flags, received)| Ok(self.append_dated(mailbox, raw, flags, *received)?))
                .collect();
        }
        let head = format!("APPEND {}", crate::sync::transport::quote(mailbox)?);
        let each: Vec<String> = messages
            .iter()
            .map(|(raw, flags, received)| {
                let flags = flag_list(flags);
                match append_date(raw, *received) {
                    Some(d) => format!(" ({flags}) \"{}\" ", d.format("%d-%b-%Y %H:%M:%S %z")),
                    None => format!(" ({flags}) "),
                }
            })
            .collect();
        let mut parts = vec![Part::Text(&head)];
        for ((raw, ..), each) in messages.iter().zip(&each) {
            parts.push(Part::Text(each));
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
        port: u16,
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
        // RFC 5092: the user is `enc-user` (`@`, `/`, `;`, `:` escaped);
        // the port is given where it isn't IMAP's 143.
        let authority = if port == 0 || port == 143 {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        };
        // Valid for a day (RFC 4467 §3: `;EXPIRE=`), long enough for the
        // submission server to fetch it, not forever.
        let expire = (chrono::Utc::now() + chrono::Duration::days(1))
            .format("%Y-%m-%dT%H:%M:%SZ");
        let url = format!(
            "imap://{}@{authority}/{};UIDVALIDITY={validity}/;UID={uid};EXPIRE={expire};urlauth=submit+{}",
            url_user(user),
            url_mailbox(mailbox),
            url_user(user)
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
    /// The largest message `mailbox` takes (RFC 7889): its own APPENDLIMIT
    /// where the server says per mailbox (STATUS … (APPENDLIMIT), which a
    /// server listing APPENDLIMIT without a number gives), else the
    /// server's (`APPENDLIMIT=n`). Asked once per connection.
    pub(super) fn append_limit(&mut self, mailbox: &str) -> Result<Option<u64>> {
        let ext = self.extensions()?;
        if !ext.append_limit_status || !crate::sync::transport::reads_status_appendlimit() {
            return Ok(ext.append_limit);
        }
        if let Some(limit) = self.append_limits.get(mailbox) {
            return Ok(*limit);
        }
        let wire = self.mailbox(mailbox)?;
        let limit = match self.heard(&format!("STATUS {wire} (APPENDLIMIT)")) {
            Ok(heard) => crate::sync::transport::status_append_limit(&heard).unwrap_or(ext.append_limit),
            // A mailbox that can't say: the server's limit, if any.
            Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => ext.append_limit,
            Err(e) => return Err(e.into()),
        };
        self.append_limits.insert(mailbox.to_owned(), limit);
        Ok(limit)
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
                list_extended: ext.list_extended,
                special_use: ext.special_use,
                list_status: ext.list_status,
                condstore: ext.condstore,
                subscribed_only: self.subscribed_only,
            };
            let others = self.namespaces.as_ref().map(|n| n.listed_apart()).unwrap_or_default();
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
    /// Where new personal folders go, if the server says (NAMESPACE's
    /// first personal namespace).
    pub(super) fn namespace(&mut self) -> Result<Option<String>> {
        Ok(self.namespaces()?.default_prefix())
    }
    /// The server's namespaces (NAMESPACE, RFC 2342), asked once.
    pub(super) fn namespaces(&mut self) -> Result<&crate::sync::transport::Namespaces> {
        if self.namespaces.is_none() {
            let found = self.ask_namespaces()?;
            self.namespaces = Some(found);
        }
        Ok(self.namespaces.as_ref().expect("just set"))
    }
    fn ask_namespaces(&mut self) -> Result<crate::sync::transport::Namespaces> {
        if !self.extensions()?.namespace {
            return Ok(Default::default());
        }
        match crate::sync::transport::namespaces(&mut self.session) {
                Ok(found) => Ok(found),
                // A broken connection is no answer about namespaces.
                Err(e) if connection_failed(&e) => return Err(e),
                Err(e) => {
                    // RFC 2342 is only advice on where folders go: without
                    // it, the hierarchy's delimiter (LIST "" "", RFC 9051
                    // §6.3.9) is all that is needed, and new folders go at
                    // the top, as on servers without NAMESPACE.
                    let delimiter = self
                        .session
                        .list(Some(""), Some("\"\""))?
                        .iter()
                        .find_map(|n| n.delimiter().map(str::to_owned));
                    eprintln!(
                        "Picacho: the server's NAMESPACE reply couldn't be used ({e:#}); folders go at the top, delimiter {delimiter:?}."
                    );
                    Ok(Default::default())
                }
        }
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
    /// Refuses, before sending anything, to change the selected mailbox
    /// where the server opened it read-only (`[READ-ONLY]` on SELECT, RFC
    /// 9051 §6.3.2 and §7.1): STORE, MOVE and EXPUNGE would be refused.
    pub(super) fn require_writable(&self, what: &str) -> Result<()> {
        if let Some(s) = self.selected.as_ref().filter(|s| s.read_only) {
            bail!(
                "“{}” can only be read on the server, so Picacho can't {what} there. Another mail app or the server's settings may be holding it.",
                crate::sync::transport::decode_name(&s.name)
            );
        }
        Ok(())
    }
    /// Whether `erase` can remove just `uids` from the selected mailbox: with
    /// UIDPLUS always; without, only if no other message there is already
    /// marked deleted (EXPUNGE would take those too). Checked before any
    /// change, so a refusal changes nothing.
    pub(super) fn can_erase(&mut self, uids: &[u32]) -> Result<()> {
        self.require_writable("remove messages")?;
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
        if let Some(tap) = &self.tap {
            tap.start();
        }
        let result =
            crate::sync::transport::select(&mut self.session, name, condstore, self.enabled.utf8);
        let heard = self.tap.as_ref().and_then(|t| t.take()).unwrap_or_default();
        let mut selected = result?;
        // RFC 9051 §6.3.2: `[READ-ONLY]` comes in the tagged OK, which the
        // imap crate reads and drops.
        selected.read_only |= crate::sync::transport::opened_read_only(&heard);
        self.alerts.extend(selected.alerts.iter().cloned());
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
        known: &[u32],
    ) -> Result<(Selected, Option<crate::sync::transport::Resync>)> {
        self.selected = None;
        if let Some(tap) = &self.tap {
            tap.start();
        }
        let result = crate::sync::transport::select_qresync(
            &mut self.session,
            name,
            self.enabled.utf8,
            validity,
            modseq,
            known,
        );
        let heard = self.tap.as_ref().and_then(|t| t.take()).unwrap_or_default();
        let (mut selected, resync) = result?;
        selected.read_only |= crate::sync::transport::opened_read_only(&heard);
        self.alerts.extend(selected.alerts.iter().cloned());
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
    /// When a POP3 account may sign in again, where its server said how
    /// often it may (CAPA's LOGIN-DELAY, RFC 2449 §6.5).
    pop_next_login: HashMap<String, i64>,
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
        // The server's [ALERT]s are shown to the person (RFC 3501 §7.1).
        let alerts = std::mem::take(&mut connection.alerts);
        if !alerts.is_empty()
            && let Some(hear) = self.proton_notice.lock().unwrap().clone()
        {
            let mut seen = std::collections::HashSet::new();
            for alert in alerts.iter().filter(|t| seen.insert(t.as_str())) {
                hear(a, "Message from your mail server", alert);
            }
        }
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
            // A full mailbox (RFC 9208's [OVERQUOTA]) isn't a rate limit:
            // waiting won't help, making room will.
            _ if crate::sync::transport::over_quota(&e) => e.context(
                "The mailbox is full: the server's storage quota is used up. \
                 Delete or archive mail to make room.",
            ),
            // Other response codes, said plainly (the server's own words
            // stay in the chain).
            _ => match crate::sync::transport::response_code_meaning(&e) {
                Some(meaning) => e.context(meaning),
                None => e,
            },
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
    /// Notes a POP3 sign-in: where the server allows one only every so
    /// many seconds (LOGIN-DELAY), background work waits that long before
    /// the next, rather than be refused with `[LOGIN-DELAY]`.
    pub(super) fn note_pop_login(&self, a: &Account, login_delay: Option<u64>) {
        let mut limits = self.limits.lock().unwrap();
        match login_delay.filter(|d| *d > 0) {
            Some(delay) => {
                let next = chrono::Utc::now().timestamp() + delay.min(i64::MAX as u64 / 2) as i64;
                limits.pop_next_login.insert(a.id.clone(), next);
            }
            None => {
                limits.pop_next_login.remove(&a.id);
            }
        }
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
        let next_login = self.limits.lock().unwrap().pop_next_login.get(&a.id).copied();
        if a.provider == Provider::Pop3
            && let Some(until) = next_login.filter(|t| *t > chrono::Utc::now().timestamp())
        {
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
        // Written in the background, batched with other settings: as its
        // own write, every downloaded message cost a transaction more. The
        // count is kept in memory meanwhile, and reads back from it.
        let _ = self.store.set_preference_soon(
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

/// Whether `error` is the connection failing (rather than the server
/// refusing or saying something unreadable): nothing more can be asked on it.
fn connection_failed(error: &anyhow::Error) -> bool {
    error.downcast_ref::<imap::error::Error>().is_some_and(|e| {
        matches!(
            e,
            imap::error::Error::Io(_)
                | imap::error::Error::ConnectionLost
                | imap::error::Error::Parse(_)
                | imap::error::Error::Tls(_)
                | imap::error::Error::TlsHandshake(_)
        )
    })
}

/// APPEND's date-time for a message: its Date header, if it has a sane
/// one, else `received` (UTC seconds) where sane; else none, and the
/// server dates it now.
fn append_date(raw: &[u8], received: Option<i64>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    // Not before 1971 nor in the future: a server refuses those.
    let now = chrono::Utc::now().timestamp();
    let sane = |t: i64| (31_536_000..=now + 86_400).contains(&t);
    message_date(raw).filter(|d| sane(d.timestamp())).or_else(|| {
        let at = received.filter(|t| sane(*t))?;
        Some(chrono::DateTime::from_timestamp(at, 0)?.fixed_offset())
    })
}
/// A message's Date header, if it reads as one.
fn message_date(raw: &[u8]) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    use mailparse::MailHeaderMap as _;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    let date = headers.get_first_value("Date")?;
    chrono::DateTime::parse_from_rfc2822(date.trim()).ok()
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

/// A user name in an IMAP URL (RFC 5092 `enc-user`): only unreserved
/// characters, sub-delims but `;`, and `&` and `=` are left as they are;
/// `@`, `/`, `:` and `;` are escaped, so an address can be a user name.
fn url_user(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,=".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// SIZE in a STATUS reply: `* STATUS "Sent" (SIZE 123456)`. Read from the
/// attribute list after the mailbox name, which may itself say SIZE.
fn status_size(heard: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(heard);
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("* ")?;
        if !rest.get(..7)?.eq_ignore_ascii_case("STATUS ") {
            return None;
        }
        // The list is the line's last parenthesized group.
        let open = rest.rfind('(')?;
        let list = rest[open + 1..].trim_end().strip_suffix(')')?;
        let words: Vec<&str> = list.split_ascii_whitespace().collect();
        words
            .chunks(2)
            .find(|c| c.len() == 2 && c[0].eq_ignore_ascii_case("SIZE"))
            .and_then(|c| c[1].parse().ok())
    })
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
        // APPEND's date: Date where it reads, else the mbox line's time.
        let dated = b"Date: Mon, 1 Jan 2024 10:00:00 +0100\r\n\r\nx";
        assert_eq!(append_date(dated, Some(1)).unwrap().timestamp(), 1_704_099_600);
        let undated = b"Date: someday\r\n\r\nx";
        assert_eq!(append_date(undated, Some(1_704_103_200)).unwrap().timestamp(), 1_704_103_200);
        assert_eq!(append_date(undated, None), None);
        assert_eq!(append_date(undated, Some(5)), None);
    }
}
