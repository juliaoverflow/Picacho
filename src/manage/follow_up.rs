//! Follow-up reminders ("Remind me if no reply"): a message sent with a
//! reminder is waited on until someone answers it. If nobody has by the
//! chosen time, Picacho reminds the person (a notice in the window, a
//! system notification from the background monitor) and keeps it in the
//! account's Waiting For list until a reply comes or they stop waiting.
//!
//! What is waited on is kept by Message-ID, per account, on this computer
//! and on the server, so Picacho on another computer waits on it too: as a
//! private annotation on the inbox where an IMAP server has METADATA (RFC
//! 5464), and otherwise as a note in Picacho's folder (`notes`: the Gmail
//! and Microsoft APIs, Exchange, JMAP, Proton, IMAP without METADATA). An
//! account waiting on nothing is looked at once a day, to notice another
//! computer starting to; one without a Picacho folder, not at all. Each entry
//! carries when it last changed, and stopping (or a reply) leaves a note of
//! when it stopped, so two computers' lists merge entry by entry: the newer
//! change wins, and a wait stopped on one isn't brought back by the other.
//!
//! A reply is any message of the account, not sent from one of its own
//! addresses nor automatic (out-of-office answers, bounces), that names the
//! message in its References or In-Reply-To, or that is in the same server
//! conversation (Gmail's, Microsoft's, JMAP's, OBJECTID's thread ids) and
//! came after it. A reply cancels the reminder by itself, before or after
//! it was due. Replies are looked for in downloaded mail as mail comes in;
//! before reminding, the server is asked too, so a reply not downloaded
//! yet, or filed away by a rule, still counts: an IMAP server searches its
//! inbox and archive; Gmail, Microsoft and Exchange list the conversation;
//! a JMAP server finds the mail naming the message; Proton's conversation
//! is read from All Mail. When it can't be asked (the account busy, the
//! server away), the reminder waits for it, half an hour at most.
//!
//! A message sent through an API (Gmail's, Microsoft's, Exchange's) may be
//! given another Message-ID by the service. If the one it was sent with
//! never shows up in Sent, the copy in Sent sent at the same moment with
//! the same subject is taken to be it, and its Message-ID is waited on.
use super::*;
use super::shared::{GONE_FOR_MS, ListKind, Stampable, Stamped, now_ms};
pub use super::shared::Gone;

/// A sent message waited on for a reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FollowUp {
    /// Its Message-ID, `<…>`.
    pub message_id: String,
    pub subject: String,
    /// Who it went to (its To field).
    #[serde(default)]
    pub to: String,
    /// When it was sent (or will be, for Send Later), seconds since the epoch.
    pub sent: i64,
    /// When to remind, if nobody has replied by then.
    pub due: i64,
    /// The person was reminded; it waits on, overdue, until a reply comes or
    /// they stop waiting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reminded: bool,
    /// When it last changed, on any computer: milliseconds since the epoch
    /// (the newer change wins when lists merge). 0 in lists from before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub changed: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// The METADATA entry the list is kept in.
const ENTRY: &str = "/private/vendor/tech.pinal.picacho/follow-ups";
/// How long past its time a reminder waits for the server to be asked
/// whether it was answered.
const HOLD_FOR: i64 = 30 * 60;
/// After the server couldn't be asked about replies, how long until it's
/// asked again (meanwhile reminders keep waiting, up to `HOLD_FOR`).
const ASK_AGAIN: i64 = 120;
/// When each account's server last couldn't be asked about replies.
static UNASKED: std::sync::LazyLock<Mutex<HashMap<String, i64>>> = std::sync::LazyLock::new(Default::default);

impl Stampable for FollowUp {
    fn key(&self) -> &str {
        &self.message_id
    }
    fn changed(&self) -> i64 {
        self.changed
    }
}

fn follow_up_state_key(account: &str) -> String {
    format!("follow_ups_synced:{account}")
}
fn load_follow_up_list(b: &Backend, account: &str) -> Stamped<FollowUp> {
    b.local_follow_ups(account)
}
fn store_follow_up_list(b: &Backend, account: &str, list: &Stamped<FollowUp>) -> Result<()> {
    b.save_follow_ups(account, &list.entries)?;
    b.save_follow_ups_gone(account, &list.gone)
}
/// The list of what's waited on, as it's shared (`shared`).
pub(crate) static FOLLOW_UP_LIST: ListKind<FollowUp> = ListKind {
    note: "follow-ups",
    entry: ENTRY,
    gone_entry: None,
    state_key: follow_up_state_key,
    load: load_follow_up_list,
    store: store_follow_up_list,
    guard: &FOLLOW_UPS,
    always_lock: false,
};

impl FollowUp {
    /// Its reminder's time has come.
    pub fn overdue(&self, now: i64) -> bool {
        self.due <= now
    }
}

/// What a look at an account's follow-ups found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FollowUpCheck {
    /// Answered, with who answered (their name, or address): no longer waited on.
    pub replied: Vec<(FollowUp, String)>,
    /// Their time came with no reply: to be told of (once).
    pub due: Vec<FollowUp>,
}

/// The reminder choices offered when sending, as seconds to wait: (label, seconds).
pub fn wait_choices() -> Vec<(String, i64)> {
    const DAY: i64 = 86400;
    vec![
        (t!("manage-follow-up-in-a-day").to_owned(), DAY),
        (t!("manage-follow-up-in-days", count = 2).to_owned(), 2 * DAY),
        (t!("manage-follow-up-in-days", count = 3).to_owned(), 3 * DAY),
        (t!("manage-follow-up-in-a-week").to_owned(), 7 * DAY),
        (t!("manage-follow-up-in-two-weeks").to_owned(), 14 * DAY),
    ]
}

/// The reminder choices for a message already sent, at `now`: (label,
/// when). Tomorrow, in three days and in a week, each in the morning (8:00),
/// and in two weeks.
pub fn remind_choices<Tz: chrono::TimeZone>(now: chrono::DateTime<Tz>) -> Vec<(String, i64)> {
    use chrono::Days;
    let at = |days: u64| -> Option<i64> {
        let date = now.date_naive().checked_add_days(Days::new(days))?;
        now.timezone()
            .from_local_datetime(&date.and_hms_opt(8, 0, 0)?)
            .earliest()
            .map(|t| t.timestamp())
    };
    let mut out = Vec::new();
    out.extend(at(1).map(|t| (t!("manage-follow-up-tomorrow").to_owned(), t)));
    out.extend(at(3).map(|t| (t!("manage-follow-up-in-days", count = 3).to_owned(), t)));
    out.extend(at(7).map(|t| (t!("manage-follow-up-in-a-week").to_owned(), t)));
    out.extend(at(14).map(|t| (t!("manage-follow-up-in-two-weeks").to_owned(), t)));
    out
}

/// How long it may seem a reply came before the message it answers: clocks
/// differ, and a Date header is the sender's.
const SKEW: i64 = 600;

/// Held while an account's list is read and written back: the window and
/// the background monitor may both look at once.
static FOLLOW_UPS: Mutex<()> = Mutex::new(());

/// The Message-IDs in a References or In-Reply-To field.
fn referenced(references: &str) -> impl Iterator<Item = &str> {
    references.split('<').skip(1).filter_map(|part| {
        let end = part.find('>')?;
        Some(&part[..end])
    })
}

/// `<id>`, however it was written.
fn bracketed(message_id: &str) -> String {
    let id = message_id.trim().trim_start_matches('<').trim_end_matches('>');
    if id.is_empty() { String::new() } else { format!("<{id}>") }
}

/// A server conversation id that is the same in every mailbox of the
/// account, so a reply in the inbox shares it with the copy in Sent. IMAP
/// THREAD's (`t:…`) is a mailbox's own.
fn account_wide_thread(thread: &str) -> bool {
    !thread.is_empty() && !thread.starts_with("t:")
}

/// Subjects automatic answers start with, lowercase, in the languages
/// mail servers write them in: for mail whose headers weren't read (a
/// listing that doesn't carry them) or don't say.
const AUTOMATIC_SUBJECTS: &[&str] = &[
    // English.
    "automatic reply",
    "auto-reply",
    "autoreply",
    "auto reply",
    "auto response",
    "auto-response",
    "out of office",
    "out of the office",
    "away from the office",
    "undeliverable",
    "undelivered mail",
    "delivery status notification",
    "delivery failure",
    "returned mail",
    "mail delivery failed",
    "delivery has failed",
    "failure notice",
    // German.
    "automatische antwort",
    "abwesenheitsnotiz",
    "abwesend",
    "unzustellbar",
    // French.
    "réponse automatique",
    "reponse automatique",
    "absence du bureau",
    "absent du bureau",
    "non remis",
    // Spanish, Portuguese, Italian.
    "respuesta automática",
    "respuesta automatica",
    "fuera de la oficina",
    "resposta automática",
    "resposta automatica",
    "ausência temporária",
    "risposta automatica",
    "fuori sede",
    "non recapitabile",
    // Dutch, Scandinavian, Polish.
    "automatisch antwoord",
    "afwezig",
    "automatiskt svar",
    "automatisk svar",
    "automatisk besked",
    "poza biurem",
    "odpowiedź automatyczna",
];

/// Mail sent by a machine rather than someone answering: out-of-office and
/// other automatic replies, and delivery reports. Its headers say so where
/// they were read (`Mail::automatic`); otherwise its sender or subject.
fn automatic(m: &Mail) -> bool {
    if m.automatic() {
        return true;
    }
    let local = m.from.split('@').next().unwrap_or("").to_ascii_lowercase();
    if matches!(local.as_str(), "mailer-daemon" | "postmaster") {
        return true;
    }
    let subject = m.subject.trim().to_lowercase();
    AUTOMATIC_SUBJECTS.iter().any(|p| subject.starts_with(p))
}

/// Whether `m` answers `f`: not the message itself, not mail of the
/// account's own (`own`: its addresses, lowercase) nor in Sent or Drafts,
/// not automatic, and naming it (or in its conversation, `thread`).
fn answers(f: &FollowUp, thread: &str, own: &HashSet<String>, m: &Mail) -> bool {
    if m.pending
        || m.message_id.trim() == f.message_id
        || matches!(m.folder, Folder::Sent | Folder::Drafts)
        || own.contains(&m.from.trim().to_lowercase())
        || automatic(m)
    {
        return false;
    }
    if referenced(m.references()).any(|id| bracketed(id) == f.message_id) {
        return true;
    }
    account_wide_thread(thread) && m.thread_id() == thread && m.date >= f.sent - SKEW
}

/// The account's own addresses, lowercase.
fn own_addresses(a: &Account) -> HashSet<String> {
    std::iter::once(a.email.as_str())
        .chain(a.identities.iter().map(|i| i.email.as_str()))
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

impl Backend {
    fn follow_up_key(account: &str) -> String {
        format!("follow_ups:{account}")
    }
    fn follow_up_gone_key(account: &str) -> String {
        format!("follow_ups_gone:{account}")
    }
    /// The account's stopped waits still remembered.
    fn follow_ups_gone(&self, account: &str) -> Vec<Gone> {
        self.store
            .preference(&Self::follow_up_gone_key(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save_follow_ups_gone(&self, account: &str, gone: &[Gone]) -> Result<()> {
        let key = Self::follow_up_gone_key(account);
        if gone.is_empty() {
            self.store.remove_preference(&key)
        } else {
            self.store.set_preference(&key, &serde_json::to_string(gone)?)
        }
    }
    /// Stops waiting on these (by Message-ID), noting when.
    fn forget_follow_ups(&self, account: &str, list: &mut Vec<FollowUp>, ids: &[String]) -> Result<()> {
        list.retain(|f| !ids.contains(&f.message_id));
        let now = now_ms();
        let mut gone = self.follow_ups_gone(account);
        gone.retain(|g| !ids.contains(&g.message_id) && g.at > now - GONE_FOR_MS);
        gone.extend(ids.iter().map(|id| Gone { message_id: id.clone(), at: now }));
        self.save_follow_ups(account, list)?;
        self.save_follow_ups_gone(account, &gone)
    }
    /// The account's messages waited on for a reply, soonest reminder first.
    pub fn follow_ups(&self, account: &str) -> Vec<FollowUp> {
        let mut list: Vec<FollowUp> = self
            .store
            .preference(&Self::follow_up_key(account))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        list.sort_by_key(|f| f.due);
        list
    }
    fn save_follow_ups(&self, account: &str, list: &[FollowUp]) -> Result<()> {
        let key = Self::follow_up_key(account);
        if list.is_empty() {
            self.store.remove_preference(&key)
        } else {
            self.store.set_preference(&key, &serde_json::to_string(list)?)
        }
    }
    /// Starts waiting on `f` (or waits on it until a new time): it replaces
    /// any wait on the same message.
    pub(crate) fn note_follow_up(&self, account: &str, mut f: FollowUp) -> Result<()> {
        let _held = FOLLOW_UPS.lock().unwrap_or_else(|e| e.into_inner());
        f.changed = now_ms();
        let mut list = self.follow_ups(account);
        list.retain(|x| x.message_id != f.message_id);
        let mut gone = self.follow_ups_gone(account);
        if gone.iter().any(|g| g.message_id == f.message_id) {
            gone.retain(|g| g.message_id != f.message_id);
            self.save_follow_ups_gone(account, &gone)?;
        }
        list.push(f);
        self.save_follow_ups(account, &list)
    }
    /// Reminds about sent message `m` at `due` if nobody has replied by then.
    /// Refused when someone already has.
    pub fn wait_for_reply(&self, a: &Account, m: &Mail, due: i64) -> Result<FollowUp> {
        let message_id = bracketed(&m.message_id);
        ensure!(!message_id.is_empty(), t!("manage-follow-up-no-message-id"));
        ensure!(m.account == a.id, t!("manage-follow-up-another-account"));
        ensure!(
            due > chrono::Utc::now().timestamp(),
            t!("manage-follow-up-choose-a-time-in-the-future")
        );
        // Kept when it waits already: when it was sent stays.
        let sent = self
            .follow_ups(&a.id)
            .into_iter()
            .find(|f| f.message_id == message_id)
            .map_or(m.date, |f| f.sent);
        let f = FollowUp {
            message_id,
            subject: m.subject.clone(),
            to: m.to.clone(),
            sent,
            due,
            reminded: false,
            changed: 0,
        };
        if let Some(reply) = self.reply_to(a, &f)? {
            bail!(t!("manage-follow-up-already-replied", sender = reply.sender));
        }
        self.note_follow_up(&a.id, f.clone())?;
        Ok(f)
    }
    /// Stops waiting on these messages (by Message-ID). Returns those that were.
    pub fn stop_waiting(&self, account: &str, message_ids: &[String]) -> Result<Vec<FollowUp>> {
        let _held = FOLLOW_UPS.lock().unwrap_or_else(|e| e.into_inner());
        let mut list = self.follow_ups(account);
        let stopped: Vec<FollowUp> = list
            .iter()
            .filter(|f| message_ids.iter().any(|id| bracketed(id) == f.message_id))
            .cloned()
            .collect();
        if !stopped.is_empty() {
            let ids: Vec<String> = stopped.iter().map(|f| f.message_id.clone()).collect();
            self.forget_follow_ups(account, &mut list, &ids)?;
        }
        Ok(stopped)
    }
    /// The first reply to `f` among the account's downloaded mail, if any.
    fn reply_to(&self, a: &Account, f: &FollowUp) -> Result<Option<Mail>> {
        let thread = self
            .store
            .thread_of(&a.id, &f.message_id)?
            .filter(|t| account_wide_thread(t))
            .unwrap_or_default();
        let own = own_addresses(a);
        Ok(self
            .store
            .replies_to(&a.id, &f.message_id, &thread, f.sent - SKEW)?
            .into_iter()
            .find(|m| answers(f, &thread, &own, m)))
    }
    /// Looks at the account's follow-ups: those answered stop being waited
    /// on (looked for only when `replies`: after mail came in), and those
    /// whose time came are returned once to be told of, and stay waiting.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn check_follow_ups(&self, a: &Account, replies: bool) -> Result<FollowUpCheck> {
        self.check_follow_ups_holding(a, replies, &HashSet::new())
    }
    /// `check_follow_ups`, leaving the reminders of `hold` (Message-IDs)
    /// for later.
    fn check_follow_ups_holding(&self, a: &Account, replies: bool, hold: &HashSet<String>) -> Result<FollowUpCheck> {
        let _held = FOLLOW_UPS.lock().unwrap_or_else(|e| e.into_inner());
        let mut list = self.follow_ups(&a.id);
        let mut check = FollowUpCheck::default();
        if list.is_empty() {
            return Ok(check);
        }
        let now = chrono::Utc::now().timestamp();
        let mut changed = false;
        if replies {
            // Sent through an API under another Message-ID: waited on by
            // the one it has in Sent.
            for f in list.iter_mut() {
                if let Some(id) = self.sent_as(&a.id, f, now)? {
                    f.message_id = id;
                    f.changed = now_ms();
                    changed = true;
                }
            }
            let mut answered = Vec::new();
            for f in &list {
                if let Some(reply) = self.reply_to(a, f)? {
                    let who = if reply.sender.trim().is_empty() { reply.from } else { reply.sender };
                    answered.push(f.message_id.clone());
                    check.replied.push((f.clone(), who));
                }
            }
            if !answered.is_empty() {
                self.forget_follow_ups(&a.id, &mut list, &answered)?;
            }
        }
        for f in list.iter_mut().filter(|f| !f.reminded && f.overdue(now) && !hold.contains(&f.message_id)) {
            f.reminded = true;
            f.changed = now_ms();
            check.due.push(f.clone());
        }
        if changed || !check.due.is_empty() {
            self.save_follow_ups(&a.id, &list)?;
        }
        Ok(check)
    }
}

impl Backend {
    /// Looks at the account's follow-ups as `check_follow_ups` does, and
    /// first, over the network: shares the list with other computers
    /// (`sync_follow_ups`), and asks the server whether the ones about to
    /// be reminded of were answered (`replies_on_server`). When the server
    /// can't be asked (busy, away), their reminders wait for it, up to
    /// `HOLD_FOR` past their time; then they're given anyway.
    pub fn check_follow_ups_online(&self, a: &Account, replies: bool) -> Result<FollowUpCheck> {
        let _ = self.sync_follow_ups(a, false);
        let now = chrono::Utc::now().timestamp();
        let due: Vec<FollowUp> = self
            .follow_ups(&a.id)
            .into_iter()
            .filter(|f| !f.reminded && f.overdue(now))
            .collect();
        let mut answered = Vec::new();
        let mut hold = HashSet::new();
        // Asked lately in vain (busy, away): not again yet; still held.
        let failed_lately = Self::shares_follow_ups(a)
            && UNASKED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&a.id)
            .is_some_and(|at| now - at < ASK_AGAIN);
        let asked = if due.is_empty() {
            Ok(Some(Vec::new()))
        } else if failed_lately {
            Ok(None)
        } else {
            self.replies_on_server(a, &due)
        };
        {
            let mut unasked = UNASKED.lock().unwrap_or_else(|e| e.into_inner());
            match &asked {
                Ok(Some(_)) => {
                    unasked.remove(&a.id);
                }
                _ if !failed_lately => {
                    unasked.insert(a.id.clone(), now);
                }
                _ => {}
            }
        }
        if !due.is_empty() {
            match asked {
                Ok(Some(found)) if !found.is_empty() => {
                    let _held = FOLLOW_UPS.lock().unwrap_or_else(|e| e.into_inner());
                    let mut list = self.follow_ups(&a.id);
                    let ids: Vec<String> = found.iter().map(|(id, _)| id.clone()).collect();
                    for (id, who) in found {
                        if let Some(f) = list.iter().find(|f| f.message_id == id) {
                            answered.push((f.clone(), who));
                        }
                    }
                    self.forget_follow_ups(&a.id, &mut list, &ids)?;
                }
                Ok(Some(_)) => {}
                // Not asked: wait for the server a while.
                Ok(None) | Err(_) => {
                    hold.extend(due.iter().filter(|f| now - f.due < HOLD_FOR).map(|f| f.message_id.clone()));
                }
            }
        }
        let mut check = self.check_follow_ups_holding(a, replies, &hold)?;
        answered.append(&mut check.replied);
        check.replied = answered;
        // What changed here reaches other computers now (a reminder given
        // here isn't given again there).
        if !check.replied.is_empty() || !check.due.is_empty() {
            let _ = self.sync_follow_ups(a, true);
        }
        Ok(check)
    }

    /// Whether the reminders reach the account's server at all (see
    /// `Backend::shares_lists`).
    fn shares_follow_ups(a: &Account) -> bool {
        Self::shares_lists(a)
    }
    /// Whether reaching the account takes its lock (one IMAP connection or
    /// Proton session at a time); the APIs' requests don't.
    fn locks_for_follow_ups(a: &Account) -> bool {
        a.imap() || a.provider == Provider::Proton
    }
    fn local_follow_ups(&self, account: &str) -> Stamped<FollowUp> {
        let mut out = Stamped {
            entries: self.follow_ups(account),
            gone: self.follow_ups_gone(account),
        };
        out.normalize();
        out
    }
    /// Shares what's waited on with the account's server, merging in what
    /// other computers changed there (`shared`): when it's time, or now
    /// with `force`. Returns whether the list here changed.
    pub fn sync_follow_ups(&self, a: &Account, force: bool) -> Result<bool> {
        self.sync_list(a, &FOLLOW_UP_LIST, force)
    }

    /// Asks the server whether these were answered, though the reply may
    /// not be downloaded (or was filed away): an IMAP server searches its
    /// inbox and archive; Gmail, Microsoft and Exchange list the message's
    /// conversation; a JMAP server finds the mail naming it; Proton's
    /// conversation is read from All Mail. Returns (Message-ID, who
    /// answered) for each that was; `None` when the server couldn't be
    /// asked now (the account busy).
    fn replies_on_server(&self, a: &Account, waiting: &[FollowUp]) -> Result<Option<Vec<(String, String)>>> {
        if !Self::shares_follow_ups(a) || waiting.is_empty() {
            return Ok(Some(vec![]));
        }
        let lock = self.account_lock(&a.id);
        let _operation = if Self::locks_for_follow_ups(a) {
            match lock.try_lock() {
                Ok(guard) => Some(guard),
                Err(_) => return Ok(None),
            }
        } else {
            None
        };
        if a.imap() {
            return self.replies_on_imap(a, waiting).map(Some);
        }
        let own = own_addresses(a);
        let mut found = Vec::new();
        for f in waiting {
            let thread = self.store.thread_of(&a.id, &f.message_id)?.unwrap_or_default();
            let (thread, candidates) = if a.provider == Provider::Jmap {
                (thread, self.jmap_replies(a, &f.message_id)?)
            } else if a.provider == Provider::Proton {
                self.proton_conversation(a, &f.message_id, f.sent - SKEW)?
            } else {
                let conversation = if a.gmail_api() {
                    thread.strip_prefix("gmail:")
                } else if a.graph() {
                    thread.strip_prefix("graph:")
                } else if a.ews() {
                    thread.strip_prefix("ews:")
                } else {
                    None
                };
                // Its copy in Sent not listed yet: its conversation isn't known.
                let Some(conversation) = conversation.filter(|c| !c.is_empty()) else {
                    continue;
                };
                let candidates = if a.gmail_api() {
                    self.gmail_conversation(a, conversation)?
                } else if a.graph() {
                    self.graph_conversation(a, conversation)?
                } else {
                    self.ews_conversation(a, conversation)?
                };
                (thread.clone(), candidates)
            };
            if let Some(reply) = candidates.iter().find(|m| answers(f, &thread, &own, m)) {
                let who = if reply.sender.trim().is_empty() { reply.from.clone() } else { reply.sender.clone() };
                found.push((f.message_id.clone(), who));
            }
        }
        Ok(Some(found))
    }

    /// IMAP: searches the inbox and archive for replies (see
    /// `replies_on_server`). The account's lock is held by the caller.
    fn replies_on_imap(&self, a: &Account, waiting: &[FollowUp]) -> Result<Vec<(String, String)>> {
        let own = own_addresses(a);
        self.with_imap(a, |c| {
            let names = c.names(false)?.to_vec();
            let mut folders = vec!["INBOX".to_owned()];
            if let Ok(archive) = crate::sync::transport::resolve_in(&names, a.provider, Folder::Archive)
                && !archive.eq_ignore_ascii_case("INBOX")
            {
                folders.push(archive);
            }
            let mut found: Vec<(String, String)> = Vec::new();
            for folder in folders {
                if c.select(&folder).is_err() {
                    continue;
                }
                for f in waiting {
                    if found.iter().any(|(id, _)| *id == f.message_id) {
                        continue;
                    }
                    let Some(since) = chrono::DateTime::from_timestamp(f.sent - 86_400, 0) else {
                        continue;
                    };
                    let id = quote(&f.message_id)?;
                    let mut criteria = format!(
                        "SINCE {} OR HEADER References {id} HEADER In-Reply-To {id} NOT HEADER Auto-Submitted auto",
                        since.format("%-d-%b-%Y")
                    );
                    for o in &own {
                        criteria.push_str(&format!(" NOT FROM {}", quote(o)?));
                    }
                    let mut uids: Vec<u32> = match c.uid_search(&criteria) {
                        Ok(uids) => uids.into_iter().collect(),
                        // A server that can't search headers: nothing learned.
                        Err(imap::error::Error::No(_) | imap::error::Error::Bad(_)) => continue,
                        Err(e) => return Err(e.into()),
                    };
                    if uids.is_empty() {
                        continue;
                    }
                    uids.sort_unstable();
                    uids.truncate(10);
                    let rows = uid_fetch_rows(
                        &mut c.session,
                        &uid_ranges(&uids),
                        "(UID BODY.PEEK[HEADER.FIELDS (FROM SUBJECT AUTO-SUBMITTED X-AUTOREPLY X-AUTORESPOND PRECEDENCE)])",
                    )?;
                    let who = rows.iter().find_map(|row| answerer(row.header.as_deref()?, &own));
                    if let Some(who) = who {
                        found.push((f.message_id.clone(), who));
                    }
                }
            }
            Ok(found)
        })
    }

    /// The Message-ID `f` is in Sent by, when the one it was sent with never
    /// showed up there (an API's service gave it another): the copy in Sent
    /// sent at the same moment, with the same subject, not waited on already.
    /// Looked for from a few minutes after sending, for three days.
    fn sent_as(&self, account: &str, f: &FollowUp, now: i64) -> Result<Option<String>> {
        if now - f.sent < 300 || now - f.sent > 3 * 86_400 {
            return Ok(None);
        }
        if !self.store.copies_of(account, &f.message_id)?.is_empty() {
            return Ok(None);
        }
        // Not one waited on already, nor one whose wait stopped or was answered.
        let waited: HashSet<String> = self
            .follow_ups(account)
            .into_iter()
            .map(|x| x.message_id)
            .chain(self.follow_ups_gone(account).into_iter().map(|g| g.message_id))
            .collect();
        let near: Vec<Mail> = self
            .store
            .sent_between(account, f.sent - 900, f.sent + 900)?
            .into_iter()
            .filter(|m| {
                !m.message_id.trim().is_empty()
                    && m.subject.trim() == f.subject.trim()
                    && !waited.contains(m.message_id.trim())
            })
            .collect();
        // Only when it can't be another message.
        Ok(match near.as_slice() {
            [one] => Some(bracketed(&one.message_id)),
            _ => None,
        })
    }
}

/// Who sent a reply, from its fetched headers (From, Subject and those that
/// mark automatic mail): their name or address, unless it is mail of the
/// account's own or automatic.
fn answerer(header: &[u8], own: &HashSet<String>) -> Option<String> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(header).ok()?;
    let value = |name: &str| headers.get_first_value(name).unwrap_or_default();
    if crate::sync::mime::machine_sent(&value) {
        return None;
    }
    let from = mailparse::addrparse(&value("From")).ok()?.extract_single_info()?;
    let m = Mail {
        from: from.addr.clone(),
        subject: value("Subject"),
        ..Mail::default()
    };
    if own.contains(&from.addr.to_lowercase()) || automatic(&m) {
        return None;
    }
    Some(from.display_name.filter(|n| !n.trim().is_empty()).unwrap_or(from.addr))
}


#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> (Backend, Account, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("picacho-follow-up-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(crate::manage::store::Store::open(&dir).unwrap());
        let mut a = Account::new(Provider::Imap);
        a.id = "acct".into();
        a.email = "me@example.com".into();
        (Backend::new(store), a, dir)
    }

    fn mail(id: &str, folder: Folder, from: &str, message_id: &str, date: i64) -> Mail {
        Mail {
            id: id.into(),
            account: "acct".into(),
            folder,
            remote_folder: folder.label().into(),
            from: from.into(),
            sender: from.split('@').next().unwrap_or("").into(),
            to: "ana@example.org".into(),
            subject: "Plans".into(),
            message_id: message_id.into(),
            date,
            ..Mail::default()
        }
    }

    fn waiting(due: i64, sent: i64) -> FollowUp {
        FollowUp {
            message_id: "<sent@me>".into(),
            subject: "Plans".into(),
            to: "ana@example.org".into(),
            sent,
            due,
            reminded: false,
            changed: 0,
        }
    }

    #[test]
    fn message_ids_are_read_from_references() {
        let ids: Vec<&str> = referenced("<a@x> <b@y>\r\n <c@z>").collect();
        assert_eq!(ids, ["a@x", "b@y", "c@z"]);
        assert_eq!(bracketed(" a@x "), "<a@x>");
        assert_eq!(bracketed("<a@x>"), "<a@x>");
        assert_eq!(bracketed(""), "");
    }

    #[test]
    fn only_someone_elses_answer_is_a_reply() {
        let now = 1_700_000_000;
        let f = waiting(now + 86400, now);
        let own: HashSet<String> = ["me@example.com".to_owned()].into();
        let mut reply = mail("r", Folder::Inbox, "ana@example.org", "<r@ana>", now + 60);
        reply.set_references("<earlier@x> <sent@me>");
        assert!(answers(&f, "", &own, &reply));
        // My own message in the thread, wherever it is, isn't.
        let mut mine = reply.clone();
        mine.from = "ME@example.com".into();
        assert!(!answers(&f, "", &own, &mine));
        let mut sent = reply.clone();
        sent.folder = Folder::Sent;
        assert!(!answers(&f, "", &own, &sent));
        // Nor is an out-of-office answer, or a bounce.
        let mut away = reply.clone();
        away.subject = "Automatic reply: Plans".into();
        assert!(!answers(&f, "", &own, &away));
        let mut bounce = reply.clone();
        bounce.from = "MAILER-DAEMON@example.org".into();
        assert!(!answers(&f, "", &own, &bounce));
        // A message naming another one isn't.
        let mut other = reply.clone();
        other.set_references("<sent@me.too>");
        assert!(!answers(&f, "", &own, &other));
        // In the same server conversation, after it was sent, it is; IMAP
        // THREAD's ids are a mailbox's own, and don't count.
        let mut threaded = mail("t", Folder::Inbox, "bo@example.org", "<t@bo>", now + 60);
        threaded.set_thread_id("gm:42");
        assert!(answers(&f, "gm:42", &own, &threaded));
        threaded.date = now - 2 * SKEW;
        assert!(!answers(&f, "gm:42", &own, &threaded));
        assert!(!account_wide_thread("t:INBOX:7:1"));
    }

    #[test]
    fn a_reply_cancels_the_reminder_and_one_due_is_told_once() -> Result<()> {
        let (b, a, dir) = backend();
        let now = chrono::Utc::now().timestamp();
        b.store.save_messages(&[mail("s", Folder::Sent, "me@example.com", "<sent@me>", now - 7200)])?;
        // Due an hour ago, nobody answered: told of once, and waits on.
        b.note_follow_up(&a.id, waiting(now - 3600, now - 7200))?;
        let check = b.check_follow_ups(&a, true)?;
        assert_eq!(check.due.len(), 1);
        assert!(check.replied.is_empty());
        assert!(b.check_follow_ups(&a, true)?.due.is_empty());
        assert!(b.follow_ups(&a.id)[0].reminded);
        // An answer arrives: no longer waited on.
        let mut reply = mail("r", Folder::Inbox, "ana@example.org", "<r@ana>", now - 60);
        reply.set_references("<sent@me>");
        b.store.save_messages(&[reply])?;
        // Not looked for unless asked (no mail came in).
        assert!(b.check_follow_ups(&a, false)?.replied.is_empty());
        let check = b.check_follow_ups(&a, true)?;
        assert_eq!(check.replied.len(), 1);
        assert_eq!(check.replied[0].1, "ana");
        assert!(b.follow_ups(&a.id).is_empty());
        // Waiting again on a message already answered is refused.
        let sent = b.store.message("s")?.unwrap();
        assert!(b.wait_for_reply(&a, &sent, now + 86400).is_err());
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn waiting_on_a_sent_message_and_stopping() -> Result<()> {
        let (b, a, dir) = backend();
        let now = chrono::Utc::now().timestamp();
        let sent = mail("s", Folder::Sent, "me@example.com", "<sent@me>", now - 60);
        assert!(b.wait_for_reply(&a, &sent, now - 1).is_err());
        let f = b.wait_for_reply(&a, &sent, now + 86400)?;
        assert_eq!((f.sent, f.due, f.to.as_str()), (now - 60, now + 86400, "ana@example.org"));
        // A new time replaces the old one.
        b.wait_for_reply(&a, &sent, now + 2 * 86400)?;
        assert_eq!(b.follow_ups(&a.id).len(), 1);
        assert_eq!(b.follow_ups(&a.id)[0].due, now + 2 * 86400);
        assert!(!b.check_follow_ups(&a, true)?.due.iter().any(|_| true));
        let stopped = b.stop_waiting(&a.id, &["sent@me".into()])?;
        assert_eq!(stopped.len(), 1);
        assert!(b.follow_ups(&a.id).is_empty());
        assert!(b.store.preference("follow_ups:acct").is_none());
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn reminders_offered_are_in_the_future_and_in_order() {
        use chrono::TimeZone as _;
        let tz = chrono::FixedOffset::east_opt(3600).unwrap();
        let now = tz.with_ymd_and_hms(2025, 11, 19, 10, 20, 0).unwrap();
        let choices = remind_choices(now);
        assert_eq!(choices.len(), 4);
        assert_eq!(choices[0].1, tz.with_ymd_and_hms(2025, 11, 20, 8, 0, 0).unwrap().timestamp());
        assert!(choices.windows(2).all(|w| w[0].1 < w[1].1));
        assert!(wait_choices().windows(2).all(|w| w[0].1 < w[1].1));
    }

    #[test]
    fn stopping_is_remembered_and_waiting_again_forgets_it() -> Result<()> {
        let (b, a, dir) = backend();
        let now = chrono::Utc::now().timestamp();
        b.note_follow_up(&a.id, waiting(now + 86400, now))?;
        b.stop_waiting(&a.id, &["<sent@me>".into()])?;
        assert_eq!(b.follow_ups_gone(&a.id).len(), 1);
        b.note_follow_up(&a.id, waiting(now + 86400, now))?;
        assert!(b.follow_ups_gone(&a.id).is_empty());
        assert!(b.follow_ups(&a.id)[0].changed > 0);
        // Not shared: an account without a server to share it with.
        assert!(!b.sync_follow_ups(&a, true)?);
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn mail_sent_under_another_message_id_is_found_in_sent() -> Result<()> {
        let (b, a, dir) = backend();
        let now = chrono::Utc::now().timestamp();
        let sent = now - 3600;
        // Sent with <sent@me>; the service filed it in Sent as <svc@api>.
        b.note_follow_up(&a.id, waiting(now + 86400, sent))?;
        b.store.save_messages(&[
            mail("s", Folder::Sent, "me@example.com", "<svc@api>", sent + 2),
            // Another message sent then, under another subject.
            Mail { subject: "Other".into(), ..mail("o", Folder::Sent, "me@example.com", "<o@api>", sent) },
        ])?;
        b.check_follow_ups(&a, true)?;
        assert_eq!(b.follow_ups(&a.id)[0].message_id, "<svc@api>");
        // Replies to it count.
        let mut reply = mail("r", Folder::Inbox, "ana@example.org", "<r@ana>", now);
        reply.set_references("<svc@api>");
        b.store.save_messages(&[reply])?;
        assert_eq!(b.check_follow_ups(&a, true)?.replied.len(), 1);
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn a_reminder_waits_a_while_for_a_server_that_cant_be_asked() -> Result<()> {
        let (b, mut a, dir) = backend();
        a.id = "busy-acct".into();
        // A server set up, though its account is busy (a long download).
        a.incoming_host = "127.0.0.1".into();
        a.incoming_port = 1;
        let now = chrono::Utc::now().timestamp();
        b.note_follow_up(&a.id, waiting(now - 60, now - 7200))?;
        let lock = b.account_lock(&a.id);
        let held = lock.lock().unwrap();
        // Held back: the server may know of a reply.
        assert!(b.check_follow_ups_online(&a, true)?.due.is_empty());
        assert!(!b.follow_ups(&a.id)[0].reminded);
        // Given once it has waited `HOLD_FOR`, asked or not.
        b.note_follow_up(&a.id, waiting(now - HOLD_FOR - 1, now - 7200))?;
        assert_eq!(b.check_follow_ups_online(&a, true)?.due.len(), 1);
        drop(held);
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn an_account_waiting_on_nothing_is_looked_at_daily() -> Result<()> {
        use super::super::shared::{DISCOVER_EVERY, RETRY_SHARING, SYNC_EVERY, SyncState};
        let (b, a, dir) = backend();
        let wait = |state: &SyncState| b.list_sync_wait(&a.id, &FOLLOW_UP_LIST, state);
        let idle = SyncState::default();
        // Nothing here or there: once a day.
        let shared = SyncState { shared: b.local_follow_ups(&a.id).fingerprint(), ..idle };
        assert_eq!(wait(&shared), DISCOVER_EVERY);
        // The server holds a list another computer may change.
        assert_eq!(wait(&SyncState { remote: true, ..shared }), SYNC_EVERY);
        // Changed here: soon.
        let now = chrono::Utc::now().timestamp();
        b.note_follow_up(&a.id, waiting(now + 86400, now))?;
        assert_eq!(wait(&shared), RETRY_SHARING);
        let shared = SyncState { shared: b.local_follow_ups(&a.id).fingerprint(), ..idle };
        assert_eq!(wait(&shared), SYNC_EVERY);
        // Without a Picacho folder known, nothing to read: no network.
        assert!(!b.has_picacho_folder(&a.id));
        b.store.save_folders(&a.id, &[RemoteFolder { name: "INBOX.Picacho".into(), label: "Picacho".into(), depth: 0, nestable: true }])?;
        assert!(b.has_picacho_folder(&a.id));
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn a_message_no_longer_waited_on_isnt_taken_for_another() -> Result<()> {
        let (b, a, dir) = backend();
        let now = chrono::Utc::now().timestamp();
        let sent = now - 3600;
        b.store.save_messages(&[mail("s", Folder::Sent, "me@example.com", "<done@me>", sent)])?;
        // Waited on, then answered (or stopped).
        b.note_follow_up(&a.id, FollowUp { message_id: "<done@me>".into(), ..waiting(now + 86400, sent) })?;
        b.stop_waiting(&a.id, &["<done@me>".into()])?;
        // Another wait, with the same subject and time, under an id never seen.
        b.note_follow_up(&a.id, waiting(now + 86400, sent))?;
        b.check_follow_ups(&a, true)?;
        assert_eq!(b.follow_ups(&a.id)[0].message_id, "<sent@me>");
        drop(b);
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn automatic_answers_are_told_by_headers_and_in_other_languages() {
        let now = 1_700_000_000;
        let f = waiting(now + 86400, now);
        let own: HashSet<String> = ["me@example.com".to_owned()].into();
        let mut reply = mail("r", Folder::Inbox, "ana@example.org", "<r@ana>", now + 60);
        reply.set_references("<sent@me>");
        assert!(answers(&f, "", &own, &reply));
        let mut flagged = reply.clone();
        flagged.set_automatic(true);
        assert!(!answers(&f, "", &own, &flagged));
        for subject in ["Abwesenheitsnotiz: Plans", "Réponse automatique : Plans", "Respuesta automática: Plans"] {
            let away = Mail { subject: subject.into(), ..reply.clone() };
            assert!(!answers(&f, "", &own, &away), "{subject}");
        }
        // Headers as a mail server writes them.
        let header = |h: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map_or(String::new(), |(_, v)| (*v).to_owned())
            }
        };
        assert!(crate::sync::mime::machine_sent(&header(&[("Auto-Submitted", "auto-replied")])));
        assert!(crate::sync::mime::machine_sent(&header(&[("X-Autoreply", "yes")])));
        assert!(crate::sync::mime::machine_sent(&header(&[("Precedence", "auto_reply")])));
        assert!(!crate::sync::mime::machine_sent(&header(&[("Auto-Submitted", "no")])));
        assert!(!crate::sync::mime::machine_sent(&header(&[("Precedence", "bulk")])));
        // What a server's search found.
        assert_eq!(
            answerer(b"From: Ana Lima <ana@example.org>\r\nSubject: Re: Plans\r\n\r\n", &own).as_deref(),
            Some("Ana Lima")
        );
        assert_eq!(answerer(b"From: ME@example.com\r\nSubject: Re: Plans\r\n\r\n", &own), None);
        assert_eq!(answerer(b"From: ana@example.org\r\nAuto-Submitted: auto-replied\r\n\r\n", &own), None);
    }

    #[test]
    fn a_parsed_out_of_office_answer_says_it_is_automatic() {
        let raw = b"From: Ana <ana@example.org>\r\nTo: me@example.com\r\nSubject: Re: Plans\r\nMessage-ID: <r@ana>\r\nIn-Reply-To: <sent@me>\r\nAuto-Submitted: auto-replied\r\n\r\nI'm away.\r\n";
        let a = Account::new(Provider::Imap);
        let m = parse_message(raw, &a, "x", Folder::Inbox, "INBOX", 1).unwrap();
        assert!(m.automatic());
        // Kept as it was through the cache.
        let back: Mail = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert!(back.automatic());
    }
}
