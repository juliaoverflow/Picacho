//! Calendars: invitations in mail answered (iMIP), and a CalDAV server's
//! calendars read, added to and changed.
//!
//! An invitation's answer goes to its organizer as an iMIP REPLY (RFC
//! 6047) from the account it came to; with a calendar server set up, the
//! event is saved there too (or removed, when declined or cancelled).
//! Events are kept in the store's preferences for the Calendar view, and
//! read again from the server at startup and when asked.
use super::*;
use crate::sync::caldav::{self, Stored};
use crate::sync::ical::{self, Event, PartStat};
use super::pim_sources::{self, Pim};

const SERVER: &str = "calendar_server";
const CALENDARS: &str = "calendars";
const EVENTS: &str = "calendar_events";
const SYNCED: &str = "calendar_synced";
/// A JMAP server's CalendarEvent state when its events were last read,
/// and the end of the time range read then ("state@until").
const JMAP_STATE: &str = "calendar_jmap_state";
/// What a JMAP server said about shared calendars and events since the
/// last sync, not yet shown (`calendar_notices`).
const NOTICES: &str = "calendar_notices";
/// The addresses a JMAP server knows as yours for invitations
/// (ParticipantIdentity), beside the accounts' own.
const PARTICIPANTS: &str = "calendar_participants";
/// The most notices kept until shown: the newest, so one busy shared
/// calendar can't grow the saved list (and each sync's rewrite of it)
/// without end.
const MOST_NOTICES: usize = 200;
fn keep_recent(notices: &mut Vec<String>) {
    if notices.len() > MOST_NOTICES {
        notices.drain(..notices.len() - MOST_NOTICES);
    }
}
/// The calendar new events go to (its href).
const DEFAULT: &str = "calendar_default";
/// Invitations answered here: event UID → answer.
const ANSWERS: &str = "calendar_answers";
/// Open tasks (VTODO), as JSON.
const TASKS: &str = "calendar_tasks";
/// A JMAP server's task lists (JMAP for Task Lists), apart from its calendars:
/// the list new tasks go to by default first.
const TASK_LISTS: &str = "calendar_task_lists";
/// A JMAP server's Task state when its tasks were last read, after what
/// the task lists were then ("lists@state"): only changes are read while
/// the lists shown and their default alerts are the same.
const TASK_STATE: &str = "calendar_task_state";
/// Reminders already shown: "uid@start@before".
const REMINDED: &str = "calendar_reminded";
#[cfg_attr(test, allow(dead_code))]
const PASSWORD: &str = "calendar:caldav";

/// How far around today the server is read.
const BEHIND: i64 = 31 * 86_400;
const AHEAD: i64 = 366 * 86_400;

/// Which subscribed feed (or other source) new events go to by default,
/// when several sources are set up: a source id (`pim_sources`).
const DEFAULT_SOURCE: &str = "calendar_default_source";
/// A feed's version when last read (ETag and Last-Modified, `feed::fetch`).
const FEED_STATE: &str = "calendar_feed_state";
/// When a feed was last tried, read or not (Unix seconds).
const FEED_TRIED: &str = "calendar_feed_tried";
/// Every key a source keeps, emptied when it's disconnected.
const SOURCE_KEYS: &[&str] = &[SERVER, CALENDARS, EVENTS, SYNCED, DEFAULT, TASKS, TASK_LISTS, TASK_STATE, JMAP_STATE, NOTICES, PARTICIPANTS, FEED_STATE, FEED_TRIED];

/// One source's parsed events, shared.
type Events = Arc<Vec<Stored>>;

/// The parsed events of each source, by the key their text is kept under,
/// and every source's put together (`cached_events`).
#[derive(Default)]
pub struct EventCache {
    by_key: HashMap<String, (Arc<str>, Events)>,
    /// Each source's events it was made from (the same `Arc`s, so an
    /// unchanged source is told apart by them), and the events together.
    merged: Option<(Vec<Events>, Events)>,
}

/// `base`'s key in the calendar source worked within (`pim_sources`).
fn key(base: &str) -> String {
    pim_sources::key(Pim::Calendar, base)
}
/// Whether this thread works within one calendar source, rather than
/// outside all of them (where reading puts them together).
fn within_one() -> bool {
    pim_sources::inside(Pim::Calendar)
}

impl Backend {
    fn cal_pref(&self, base: &str) -> Option<String> {
        self.store.preference(&key(base))
    }
    fn set_cal_pref(&self, base: &str, value: &str) -> Result<()> {
        self.store.set_preference(&key(base), value)
    }
    /// Runs `work` within calendar source `id`.
    pub fn in_calendar_source<T>(&self, id: &str, work: impl FnOnce() -> T) -> T {
        pim_sources::within(Pim::Calendar, id, work)
    }
    /// Every calendar source's id: the first (""), then the others.
    pub fn calendar_source_ids(&self) -> Vec<String> {
        pim_sources::source_ids(&self.store, Pim::Calendar)
    }
    /// The sources set up besides the first: (id, server, when last read).
    pub fn calendar_sources(&self) -> Vec<(String, caldav::Server, Option<i64>)> {
        pim_sources::extra_sources(&self.store, Pim::Calendar)
            .into_iter()
            .filter_map(|id| {
                let (server, synced) = self.in_calendar_source(&id, || (self.calendar_server(), self.calendar_synced()));
                Some((id, server?, synced))
            })
            .collect()
    }
    /// The first calendar source's server, as Settings shows it.
    pub fn primary_calendar_server(&self) -> Option<caldav::Server> {
        self.in_calendar_source("", || self.calendar_server())
    }
    /// The source that has the event `uid`, if any does.
    fn calendar_source_of_event(&self, uid: &str) -> Option<String> {
        if uid.is_empty() {
            return None;
        }
        self.calendar_source_ids().into_iter().find(|id| {
            self.in_calendar_source(id, || {
                self.cached_events().iter().any(|s| s.event.uid == uid) || self.calendar_tasks().iter().any(|t| t.event.uid == uid)
            })
        })
    }
    /// The source that has calendar (or task list) `href`, if any does.
    fn calendar_source_of(&self, href: &str) -> Option<String> {
        self.calendar_source_ids().into_iter().find(|id| {
            self.in_calendar_source(id, || self.calendars().iter().chain(self.task_lists().iter()).any(|c| c.href == href))
        })
    }
    /// The source new events go to: the one chosen (`set_default_calendar`),
    /// else the first that can take them: the first source, else another.
    /// Feeds take none.
    pub fn default_calendar_source(&self) -> String {
        let writable: Vec<String> = self
            .calendar_source_ids()
            .into_iter()
            .filter(|id| self.in_calendar_source(id, || self.calendar_server().is_some_and(|s| !s.feed)))
            .collect();
        let chosen = self.store.preference(DEFAULT_SOURCE).unwrap_or_default();
        if writable.contains(&chosen) {
            return chosen;
        }
        writable.into_iter().next().unwrap_or_default()
    }
    /// Outside any source: the one to work on event `uid` within (the one
    /// that has it, else where new events go). `None` within one already.
    fn route_event(&self, uid: &str) -> Option<String> {
        (!within_one()).then(|| self.calendar_source_of_event(uid).unwrap_or_else(|| self.default_calendar_source()))
    }
    /// Outside any source: the one that has calendar `href`.
    fn route_calendar(&self, href: &str) -> Option<String> {
        (!within_one()).then(|| self.calendar_source_of(href).unwrap_or_else(|| self.default_calendar_source()))
    }
    /// Outside any source: where new events go.
    fn route_default(&self) -> Option<String> {
        (!within_one()).then(|| self.default_calendar_source())
    }
    /// Outside any source: the first one (set up in Settings as always).
    fn route_first(&self) -> Option<String> {
        (!within_one()).then(String::new)
    }
    /// Within each source with a server, `work`'s answers.
    fn each_calendar_source<T>(&self, work: impl Fn() -> T) -> Vec<(String, T)> {
        self.calendar_source_ids()
            .into_iter()
            .filter_map(|id| self.in_calendar_source(&id, || self.calendar_server().is_some().then(&work)).map(|t| (id, t)))
            .collect()
    }

    /// The server calendars come from. Outside any source, the first one
    /// set up.
    pub fn calendar_server(&self) -> Option<caldav::Server> {
        if !within_one() {
            return self
                .calendar_source_ids()
                .into_iter()
                .find_map(|id| self.in_calendar_source(&id, || self.calendar_server()));
        }
        self.cal_pref(SERVER)
            .and_then(|j| serde_json::from_str(&j).ok())
            .filter(|s: &caldav::Server| !s.url.is_empty())
    }
    /// The calendars; outside any source, every source's.
    pub fn calendars(&self) -> Vec<caldav::Calendar> {
        if !within_one() {
            return self.calendar_source_ids().into_iter().flat_map(|id| self.in_calendar_source(&id, || self.calendars())).collect();
        }
        self.cal_pref(CALENDARS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    /// Every event known here, soonest first.
    pub fn calendar_events(&self) -> Vec<Stored> {
        self.cached_events().as_ref().clone()
    }
    /// The events, parsed only when their saved text changed (see
    /// `calendar_cache`): whatever wrote it, the text read now decides.
    /// Outside any source, every source's together.
    fn cached_events(&self) -> Arc<Vec<Stored>> {
        if !within_one() {
            let parts: Vec<Arc<Vec<Stored>>> = self
                .calendar_source_ids()
                .into_iter()
                .map(|id| self.in_calendar_source(&id, || self.cached_events()))
                .collect();
            if parts.len() == 1 {
                return parts[0].clone();
            }
            let mut cache = self.calendar_cache.lock().unwrap();
            if let Some((from, merged)) = &cache.merged
                && from.len() == parts.len()
                && from.iter().zip(&parts).all(|(a, b)| Arc::ptr_eq(a, b))
            {
                return merged.clone();
            }
            let mut all: Vec<Stored> = parts.iter().flat_map(|p| p.iter().cloned()).collect();
            all.sort_by_key(|s| s.event.start);
            let merged = Arc::new(all);
            cache.merged = Some((parts, merged.clone()));
            return merged;
        }
        let at = key(EVENTS);
        let text = self.store.preference(&at).unwrap_or_default();
        let mut cache = self.calendar_cache.lock().unwrap();
        if let Some((saved, events)) = cache.by_key.get(&at)
            && **saved == *text
        {
            return events.clone();
        }
        let events: Arc<Vec<Stored>> = Arc::new(serde_json::from_str(&text).unwrap_or_default());
        cache.by_key.insert(at, (text.into(), events.clone()));
        events
    }
    fn save_events(&self, mut events: Vec<Stored>) -> Result<()> {
        // Outside any source, every source's events would be read together
        // and written into the first.
        ensure!(
            within_one() || pim_sources::extra_sources(&self.store, Pim::Calendar).is_empty(),
            "events were saved outside any calendar source"
        );
        events.sort_by_key(|s| s.event.start);
        self.set_cal_pref(EVENTS, &serde_json::to_string(&events)?)?;
        crate::manage::widgets::publish_soon(self.store.clone());
        Ok(())
    }
    pub fn calendar_synced(&self) -> Option<i64> {
        self.cal_pref(SYNCED)?.parse().ok()
    }
    /// The calendar new events go to: the one chosen, or the first that
    /// can be written (in the source new events go to, outside any).
    pub fn default_calendar(&self) -> Option<caldav::Calendar> {
        if let Some(id) = self.route_default() {
            return self.in_calendar_source(&id, || self.default_calendar());
        }
        let cals = self.calendars();
        let chosen = self.cal_pref(DEFAULT).unwrap_or_default();
        // One that takes events: not a calendar for tasks alone.
        let events = |c: &&caldav::Calendar| {
            c.writable && (c.components.is_empty() || c.components.iter().any(|k| k == "VEVENT"))
        };
        cals.iter()
            .find(|c| c.href == chosen && events(c))
            .or_else(|| cals.iter().find(events))
            .cloned()
    }
    /// The calendar new tasks go to: the default one where it takes tasks,
    /// else the first writable one that does. Calendars whose kinds aren't
    /// known are taken to take both.
    /// Over JMAP, tasks are in task lists of their own: the default one,
    /// else the first writable one.
    pub fn task_calendar(&self) -> Option<caldav::Calendar> {
        if !within_one() {
            let first = self.default_calendar_source();
            return self.in_calendar_source(&first, || self.task_calendar()).or_else(|| {
                self.calendar_source_ids().into_iter().find_map(|id| self.in_calendar_source(&id, || self.task_calendar()))
            });
        }
        let server = self.calendar_server();
        if server.as_ref().is_some_and(|s| !s.jmap_account.is_empty()) {
            return self.task_lists().into_iter().find(|l| l.writable);
        }
        // Google, Exchange, Proton and feeds keep no tasks here.
        if server.as_ref().is_some_and(|s| !s.dav()) {
            return None;
        }
        let takes = |c: &caldav::Calendar| {
            c.writable && (c.components.is_empty() || c.components.iter().any(|k| k == "VTODO"))
        };
        self.default_calendar()
            .filter(takes)
            .or_else(|| self.calendars().into_iter().find(takes))
    }
    /// A JMAP server's task lists, as calendars that take only tasks.
    pub fn task_lists(&self) -> Vec<caldav::Calendar> {
        if !within_one() {
            return self.calendar_source_ids().into_iter().flat_map(|id| self.in_calendar_source(&id, || self.task_lists())).collect();
        }
        self.cal_pref(TASK_LISTS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    /// Makes `href` the calendar new events go to, and its source the one
    /// they go to.
    pub fn set_default_calendar(&self, href: &str) -> Result<()> {
        if !within_one() {
            let id = self.calendar_source_of(href).unwrap_or_default();
            self.store.set_preference(DEFAULT_SOURCE, &id)?;
            return self.in_calendar_source(&id, || self.set_default_calendar(href));
        }
        self.set_cal_pref(DEFAULT, href)
    }
    fn calendar_password(&self) -> Result<String> {
        #[cfg(test)]
        if let Some(s) = &self.test_secret {
            return Ok(s.password.clone());
        }
        // Signed in as a mail account is: its token (renewed as needed) or
        // password, never a copy of it.
        if let Some(server) = self.calendar_server()
            && !server.mail_account.is_empty()
        {
            return self.mail_account_secret(&server.mail_account);
        }
        store::read_secret_text(&key(PASSWORD))?
            .with_context(|| t!("manage-calendar-the-calendar-servers-password-is-missing"))
    }
    /// Adds another calendar source beside those set up, set up by `work`
    /// (one of the `connect_…` functions, or `subscribe_calendar_feed`)
    /// within it. Returns what `work` did.
    pub fn add_calendar_source<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        self.add_source(Pim::Calendar, work)
    }
    /// Disconnects one of the sources added besides the first.
    pub fn remove_calendar_source(&self, id: &str) -> Result<()> {
        ensure!(!id.is_empty(), "the first calendar source is disconnected with disconnect_calendar");
        self.in_calendar_source(id, || self.disconnect_calendar())?;
        if self.store.preference(DEFAULT_SOURCE).as_deref() == Some(id) {
            self.store.set_preference(DEFAULT_SOURCE, "")?;
        }
        self.forget_source(Pim::Calendar, id)
    }
    /// Signs in to a CalDAV server and reads its calendars and events.
    pub fn connect_calendar(&self, server: caldav::Server, password: &str) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_calendar(server, password));
        }
        let cals = caldav::calendars(&server, password, self.tls_connector()?)?;
        // Shared with a mail account: its sign-in is read from it each time.
        #[cfg(not(test))]
        if server.mail_account.is_empty() {
            store::write_secret_text(&key(PASSWORD), password)?;
        }
        self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)?;
        // Who you are to the server (RFC 6638 §2.4.1), for telling your
        // own events, and where it puts accepted invitations (§9.2), as
        // the calendar new events go to unless one was chosen. Not every
        // server schedules: not worth failing for.
        if let Ok((addresses, default)) = caldav::scheduling_identity(&server, password, self.tls_connector()?) {
            if !addresses.is_empty() {
                self.set_cal_pref(PARTICIPANTS, &serde_json::to_string(&addresses)?)?;
            }
            if let Some(href) = default.filter(|h| cals.iter().any(|c| &c.href == h))
                && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
            {
                self.set_cal_pref(DEFAULT, &href)?;
            }
        }
        self.sync_calendar()
    }
    /// Lets the CalDAV server send invitations, updates and cancellations
    /// for events you organize (RFC 6638), or has Picacho email them
    /// again. Turning it on asks the server first: one that doesn't
    /// schedule is refused, so invitations aren't silently lost.
    pub fn set_server_scheduling(&self, on: bool) -> Result<()> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.set_server_scheduling(on));
        }
        let mut server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        ensure!(!server.feed, t!("manage-calendar-subscribed-calendars-are-read-only"));
        // Proton's invitations are Proton Calendar's to send.
        ensure!(!on || server.proton_account.is_empty(), t!("manage-calendar-proton-calendar-doesnt-send-invitations"));
        // A JMAP server sends them where it names the addresses it sends
        // as (ParticipantIdentity); CalDAV, where it auto-schedules.
        if self.calendar_graph(&server)?.is_some() || self.calendar_ews(&server)?.is_some() || !server.google_account.is_empty() {
            // Exchange sends them itself, always.
            ensure!(on, t!("manage-calendar-exchange-always-sends-invitations"));
        } else if on && let Some(a) = self.calendar_jmap(&server)? {
            ensure!(
                self.with_jmap_calendars(&a, crate::sync::jscal::server_can_schedule)?,
                t!("manage-calendar-this-jmap-server-doesnt-say-which")
            );
        } else if on {
            ensure!(
                caldav::schedules(&server, &self.calendar_password()?, self.tls_connector()?)?,
                t!("manage-calendar-this-calendar-server-doesnt-send-invitations")
            );
        }
        server.server_schedules = on;
        self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)
    }
    /// Whether an event is one you organize (or has no organizer): with
    /// server scheduling, the server sends its invitations. Invitations
    /// from others are still answered by mail, so the organizer isn't
    /// answered twice.
    fn organized_here(&self, e: &Event) -> bool {
        let Some(o) = &e.organizer else { return true };
        let known: Vec<String> = self.cal_pref(PARTICIPANTS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if known.iter().any(|k| k.eq_ignore_ascii_case(&o.email)) {
            return true;
        }
        self.store.accounts().unwrap_or_default().iter().any(|a| {
            a.email.eq_ignore_ascii_case(&o.email)
                || a.identities
                    .iter()
                    .any(|i| i.email.trim().eq_ignore_ascii_case(&o.email))
        })
    }
    /// Whether invitations for events you organize are the server's to send.
    pub fn server_schedules(&self) -> bool {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.server_schedules());
        }
        self.calendar_server().is_some_and(|s| s.server_schedules)
    }
    /// Uses the calendars of a JMAP account's server (JMAP for Calendars)
    /// instead of a CalDAV server: its sign-in is the account's own.
    pub fn connect_jmap_calendar(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_jmap_calendar(a));
        }
        ensure!(
            a.provider == Provider::Jmap,
            t!("manage-calendar-only-jmap-accounts-keep-calendars-this")
        );
        self.with_jmap_calendars(a, crate::sync::jscal::ensure_calendars)?;
        let server = caldav::Server {
            url: a.incoming_host.clone(),
            user: a.email.clone(),
            jmap_account: a.id.clone(),
            server_schedules: false,
            ..Default::default()
        };
        self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.set_cal_pref(CALENDARS, "[]")?;
        self.set_cal_pref(JMAP_STATE, "")?;
        // The last server's tasks aren't this one's.
        self.set_cal_pref(TASKS, "[]")?;
        self.set_cal_pref(TASK_LISTS, "[]")?;
        self.set_cal_pref(TASK_STATE, "")?;
        let count = self.sync_calendar()?;
        // Tasks aren't worth failing the calendars for.
        let _ = self.sync_tasks();
        Ok(count)
    }
    /// Runs `work` on the JMAP session of the account calendars come from.
    fn with_jmap_calendars<T>(
        &self,
        a: &Account,
        work: impl FnOnce(&crate::sync::jmap::Session) -> Result<T>,
    ) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_jmap(a, work)
    }
    /// The JMAP account calendars come from, if they come from one.
    fn calendar_jmap(&self, server: &caldav::Server) -> Result<Option<Account>> {
        if server.jmap_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.jmap_account)
                .with_context(|| t!("manage-calendar-the-account-your-calendars-come-from"))?,
        ))
    }
    /// Uses the Exchange calendars of an Outlook account that reaches
    /// Microsoft 365 through Microsoft Graph: its sign-in is the account's.
    /// Exchange sends invitations for events with guests itself.
    pub fn connect_graph_calendar(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_graph_calendar(a));
        }
        ensure!(a.graph(), t!("manage-calendar-only-accounts-that-use-microsoft"));
        let server = caldav::Server {
            url: crate::sync::graph::BASE.to_owned(),
            user: a.email.clone(),
            graph_account: a.id.clone(),
            server_schedules: true,
            ..Default::default()
        };
        self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.set_cal_pref(CALENDARS, "[]")?;
        self.sync_calendar()
    }
    /// Uses the calendars of an Exchange Server account's mailbox (EWS):
    /// its sign-in is the account's. Exchange sends invitations for events
    /// with guests itself.
    pub fn connect_ews_calendar(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_ews_calendar(a));
        }
        ensure!(a.ews(), t!("manage-calendar-only-exchange-accounts"));
        let server = caldav::Server {
            url: a.ews_endpoint(),
            user: a.email.clone(),
            ews_account: a.id.clone(),
            server_schedules: true,
            ..Default::default()
        };
        self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)?;
        self.set_cal_pref(CALENDARS, "[]")?;
        self.sync_calendar()
    }
    /// The Exchange account calendars come from, if they come from one.
    fn calendar_ews(&self, server: &caldav::Server) -> Result<Option<Account>> {
        if server.ews_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.ews_account)
                .with_context(|| t!("manage-calendar-the-account-your-calendars-come-from"))?,
        ))
    }
    /// Reads an Exchange mailbox's calendars and their events.
    fn sync_ews_calendar(&self, a: &Account) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (calendars, events) = self.ews_events(a, now - BEHIND)?;
        if let Some(first) = calendars.first()
            && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
        {
            self.set_cal_pref(DEFAULT, &first.href)?;
        }
        let count = events.len();
        self.save_events(events)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&calendars)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// The Graph account calendars come from, if they come from one.
    fn calendar_graph(&self, server: &caldav::Server) -> Result<Option<Account>> {
        if server.graph_account.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == server.graph_account)
                .with_context(|| t!("manage-calendar-the-account-your-calendars-come-from"))?,
        ))
    }
    fn with_graph_calendars<T>(&self, a: &Account, work: impl FnOnce(&crate::sync::graph::Session) -> Result<T>) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_graph(a, work)
    }
    /// Reads Exchange's calendars and their events in the range shown,
    /// each occurrence of a repeating event on its own.
    fn sync_graph_calendar(&self, a: &Account) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (cals, events) = self.with_graph_calendars(a, |s| {
            let cals = s.calendars()?;
            let events = s.events_in(&cals, now - BEHIND, now + AHEAD)?;
            Ok((cals, events))
        })?;
        if let Some(default) = cals.iter().find(|c| c.default)
            && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
        {
            self.set_cal_pref(DEFAULT, &format!("graph:{}", default.id))?;
        }
        let calendars: Vec<caldav::Calendar> = cals
            .into_iter()
            .map(|c| caldav::Calendar {
                href: format!("graph:{}", c.id),
                name: c.name,
                writable: c.writable,
                color: c.color,
                components: vec!["VEVENT".into()],
                ..Default::default()
            })
            .collect();
        let stored: Vec<Stored> = events
            .into_iter()
            .map(|e| Stored {
                event: e.event,
                overrides: e.overrides,
                calendar: format!("graph:{}", e.calendar),
                href: format!("graph:{}", e.id),
                etag: e.etag,
                schedule_tag: String::new(),
            })
            .collect();
        let count = stored.len();
        self.save_events(stored)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&calendars)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// The mail account `server` signs in as through `field`, if it does.
    fn calendar_account(&self, id: &str) -> Result<Option<Account>> {
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            self.store
                .accounts()?
                .into_iter()
                .find(|a| a.id == id)
                .with_context(|| t!("manage-calendar-the-account-your-calendars-come-from"))?,
        ))
    }
    /// The Google account calendars come from (Google Calendar API), if any.
    fn calendar_google(&self, server: &caldav::Server) -> Result<Option<Account>> {
        self.calendar_account(&server.google_account)
    }
    /// The Proton account calendars come from (Proton Calendar), if any.
    fn calendar_proton(&self, server: &caldav::Server) -> Result<Option<Account>> {
        self.calendar_account(&server.proton_account)
    }
    /// What a calendar source is called in Settings and messages: the name
    /// given, its account's address, or its server.
    pub fn calendar_source_label(&self, id: &str) -> String {
        self.in_calendar_source(id, || {
            let Some(server) = self.calendar_server() else {
                return String::new();
            };
            if !server.label.is_empty() {
                return server.label;
            }
            if server.feed
                && let Some(c) = self.calendars().into_iter().next()
                && !c.name.is_empty()
            {
                return c.name;
            }
            if let Some(a) = server
                .account()
                .and_then(|id| self.store.accounts().ok()?.into_iter().find(|a| a.id == id))
            {
                return a.email;
            }
            url::Url::parse(&server.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or(server.url)
        })
    }
    /// Starts a source afresh for `server`: what the last one kept goes.
    fn start_source(&self, server: &caldav::Server) -> Result<()> {
        self.set_cal_pref(SERVER, &serde_json::to_string(server)?)?;
        for base in [CALENDARS, TASKS, TASK_LISTS] {
            self.set_cal_pref(base, "[]")?;
        }
        for base in [DEFAULT, TASK_STATE, JMAP_STATE, FEED_STATE] {
            self.set_cal_pref(base, "")?;
        }
        Ok(())
    }

    /// Uses the calendars of a Google account signed in with OAuth, through
    /// the Google Calendar API: its sign-in is the account's. Google sends
    /// invitations for events with guests itself.
    pub fn connect_google_calendar(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_google_calendar(a));
        }
        ensure!(
            crate::manage::backend::dav_accounts::MailProvider::of(a) == Some(crate::manage::backend::dav_accounts::MailProvider::Google),
            t!("manage-calendar-only-google-accounts")
        );
        self.start_source(&caldav::Server {
            url: crate::sync::google::CALENDAR_BASE.to_owned(),
            user: a.email.clone(),
            google_account: a.id.clone(),
            server_schedules: true,
            ..Default::default()
        })?;
        self.sync_calendar()
    }
    /// Reads Google's calendars and their events in the range shown.
    fn sync_google_calendar(&self, a: &Account) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (cals, events) = self.with_google(a, |s| {
            let cals = s.calendars()?;
            let events = s.events_in(&cals, now - BEHIND, now + AHEAD)?;
            Ok((cals, events))
        })?;
        if let Some(own) = cals.iter().find(|c| c.primary && c.writable)
            && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
        {
            self.set_cal_pref(DEFAULT, &format!("google:{}", own.id))?;
        }
        let calendars: Vec<caldav::Calendar> = cals
            .into_iter()
            .map(|c| caldav::Calendar {
                href: format!("google:{}", c.id),
                name: c.name,
                writable: c.writable,
                color: c.color,
                components: vec!["VEVENT".into()],
                ..Default::default()
            })
            .collect();
        let stored: Vec<Stored> = events
            .into_iter()
            .map(|e| Stored {
                event: e.event,
                overrides: e.overrides,
                calendar: format!("google:{}", e.calendar),
                href: format!("google:{}", e.id),
                etag: e.etag,
                schedule_tag: String::new(),
            })
            .collect();
        let count = stored.len();
        self.save_events(stored)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&calendars)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// Saves an event to a Google calendar: the series, then each
    /// occurrence changed on its own since it was read.
    fn google_save_event(&self, a: &Account, calendar: &str, stored: &Stored, invite: bool) -> Result<Stored> {
        use crate::sync::google;
        let calendar_id = calendar.strip_prefix("google:").with_context(|| t!("manage-calendar-not-a-google-calendar"))?;
        let event_id = stored.href.strip_prefix("google:").filter(|i| !i.is_empty());
        let before = self.stored_event(&stored.event.uid).filter(|b| b.href == stored.href);
        let mut saved = stored.clone();
        self.with_google(a, |s| {
            let (id, etag) = s.save_event(calendar_id, event_id, &stored.event, true, invite)?;
            saved.calendar = calendar.to_owned();
            saved.href = format!("google:{id}");
            saved.etag = etag;
            let old = before.map(|b| b.overrides).unwrap_or_default();
            for o in saved.overrides.iter_mut().filter(|o| !old.contains(o)) {
                let Some(at) = o.recurrence_id else { continue };
                let known = google::occurrence_id(o).map(str::to_owned);
                let Some(occurrence) = known.map_or_else(|| s.occurrence_at(calendar_id, &id, at), |k| Ok(Some(k)))? else {
                    continue;
                };
                s.save_event(calendar_id, Some(&occurrence), &Event { recurrence: String::new(), ..o.clone() }, false, invite)?;
                if google::occurrence_id(o).is_none() {
                    o.kept.push_str(&format!("{}{occurrence}\r\n", google::GOOGLE_ID));
                }
            }
            Ok(())
        })?;
        Ok(saved)
    }

    /// Uses the Proton Calendar calendars of a Proton account signed in
    /// without Bridge: its session and keys are the account's.
    pub fn connect_proton_calendar(&self, a: &Account) -> Result<usize> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.connect_proton_calendar(a));
        }
        ensure!(a.provider == Provider::Proton, t!("manage-calendar-only-proton-accounts"));
        self.start_source(&caldav::Server {
            url: "https://calendar.proton.me".to_owned(),
            user: a.email.clone(),
            proton_account: a.id.clone(),
            ..Default::default()
        })?;
        self.sync_calendar()
    }
    fn with_proton_calendars<T>(&self, a: &Account, work: impl FnOnce(&crate::sync::proton::Session) -> Result<T>) -> Result<T> {
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_proton(a, work)
    }
    /// The Proton calendar `href` names, as the account is its member.
    fn proton_calendar(s: &crate::sync::proton::Session, href: &str) -> Result<crate::sync::proton::ProtonCalendar> {
        let id = href.strip_prefix("proton:").with_context(|| t!("manage-calendar-not-a-proton-calendar"))?;
        s.calendars()?
            .into_iter()
            .find(|c| c.id == id)
            .with_context(|| t!("manage-calendar-this-calendar-is-no-longer-on"))
    }
    /// Reads Proton's calendars and their events in the range shown,
    /// decrypted.
    fn sync_proton_calendar(&self, a: &Account) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (cals, events) = self.with_proton_calendars(a, |s| {
            let cals = s.calendars()?;
            let mut events = Vec::new();
            for c in &cals {
                events.extend(s.events_in(c, now - BEHIND, now + AHEAD)?);
            }
            Ok((cals, events))
        })?;
        if let Some(first) = cals.iter().find(|c| c.writable)
            && self.cal_pref(DEFAULT).is_none_or(|d| d.is_empty())
        {
            self.set_cal_pref(DEFAULT, &format!("proton:{}", first.id))?;
        }
        let calendars: Vec<caldav::Calendar> = cals
            .into_iter()
            .map(|c| caldav::Calendar {
                href: format!("proton:{}", c.id),
                name: c.name,
                writable: c.writable,
                color: c.color,
                components: vec!["VEVENT".into()],
                ..Default::default()
            })
            .collect();
        let stored: Vec<Stored> = events
            .into_iter()
            .map(|e| Stored {
                event: e.event,
                overrides: e.overrides,
                calendar: format!("proton:{}", e.calendar),
                href: format!("proton:{}", e.id),
                etag: e.version,
                schedule_tag: String::new(),
            })
            .collect();
        let count = stored.len();
        self.save_events(stored)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&calendars)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// The Proton ids of an event as kept: its series', and each changed
    /// occurrence's.
    fn proton_ids(stored: &Stored) -> Vec<String> {
        stored
            .href
            .strip_prefix("proton:")
            .filter(|i| !i.is_empty())
            .map(str::to_owned)
            .into_iter()
            .chain(stored.overrides.iter().filter_map(|o| crate::sync::proton::proton_occurrence_id(o).map(str::to_owned)))
            .collect()
    }
    /// Saves an event to a Proton calendar, encrypted and signed, in place
    /// of what it was there.
    fn proton_save_event(&self, a: &Account, calendar: &str, stored: &Stored) -> Result<Stored> {
        let replacing = if stored.calendar == calendar { Self::proton_ids(stored) } else { Vec::new() };
        let (id, occurrences) = self.with_proton_calendars(a, |s| {
            let cal = Self::proton_calendar(s, calendar)?;
            s.save_event(&cal, &replacing, &stored.event, &stored.overrides)
        })?;
        let mut saved = stored.clone();
        saved.calendar = calendar.to_owned();
        saved.href = format!("proton:{id}");
        saved.etag = String::new();
        for (o, own) in saved.overrides.iter_mut().zip(occurrences) {
            o.kept = o
                .kept
                .lines()
                .filter(|l| !l.starts_with(crate::sync::proton::PROTON_ID))
                .map(|l| format!("{l}\r\n"))
                .collect();
            o.kept.push_str(&format!("{}{own}\r\n", crate::sync::proton::PROTON_ID));
        }
        Ok(saved)
    }
    fn proton_delete_event(&self, a: &Account, stored: &Stored) -> Result<()> {
        let ids = Self::proton_ids(stored);
        if ids.is_empty() {
            return Ok(());
        }
        self.with_proton_calendars(a, |s| {
            let cal = Self::proton_calendar(s, &stored.calendar)?;
            s.delete_events(&cal, &ids)
        })
    }

    /// Subscribes to a calendar feed (`webcal://`, or an `.ics` address),
    /// read again every `minutes` (0: every six hours) and never written,
    /// as a source of its own beside those set up. `name`, when given, is
    /// what it's called instead of the feed's own name.
    pub fn subscribe_calendar_feed(&self, url: &str, name: &str, minutes: u32) -> Result<usize> {
        if !within_one() {
            return self.add_calendar_source(|| self.subscribe_calendar_feed(url, name, minutes));
        }
        let address = crate::sync::feed::address(url)?;
        self.start_source(&caldav::Server {
            url: address.to_string(),
            feed: true,
            refresh_minutes: if minutes == 0 { 0 } else { minutes.max(crate::sync::feed::SHORTEST_MINUTES) },
            label: name.trim().to_owned(),
            ..Default::default()
        })?;
        self.sync_calendar()
    }
    /// Changes how often a feed (source `id`) is read again.
    pub fn set_feed_interval(&self, id: &str, minutes: u32) -> Result<()> {
        self.in_calendar_source(id, || {
            let mut server = self
                .calendar_server()
                .filter(|s| s.feed)
                .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
            server.refresh_minutes = if minutes == 0 { 0 } else { minutes.max(crate::sync::feed::SHORTEST_MINUTES) };
            self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)
        })
    }
    /// Reads the feed this source subscribes to: all of it, unless the
    /// feed's server says it's unchanged.
    fn sync_feed(&self, server: &caldav::Server) -> Result<usize> {
        use crate::sync::feed;
        let id = pim_sources::current(Pim::Calendar).unwrap_or_default();
        let href = format!("feed:{}", if id.is_empty() { "first" } else { id.as_str() });
        let now = chrono::Utc::now().timestamp();
        // Nothing kept yet: read whole, whatever was read before.
        let validators = if self.calendars().is_empty() { String::new() } else { self.cal_pref(FEED_STATE).unwrap_or_default() };
        self.set_cal_pref(FEED_TRIED, &now.to_string())?;
        match feed::fetch(&server.url, &validators, self.tls_connector()?)? {
            feed::Fetched::Unchanged => {
                self.set_cal_pref(SYNCED, &now.to_string())?;
                Ok(self.cached_events().len())
            }
            feed::Fetched::Changed { text, validators } => {
                let read = feed::read(&text, &href, now - BEHIND);
                let host = url::Url::parse(&server.url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
                let name = [server.label.clone(), read.name, host].into_iter().find(|n| !n.is_empty()).unwrap_or_default();
                let calendars = vec![caldav::Calendar {
                    href,
                    name,
                    writable: false,
                    color: read.color,
                    components: vec!["VEVENT".into()],
                    ..Default::default()
                }];
                let count = read.events.len();
                self.save_events(read.events)?;
                self.set_cal_pref(CALENDARS, &serde_json::to_string(&calendars)?)?;
                self.set_cal_pref(FEED_STATE, &validators)?;
                self.set_cal_pref(SYNCED, &now.to_string())?;
                Ok(count)
            }
        }
    }
    /// Reads again each subscribed feed whose interval has passed since it
    /// was last read (or tried: a feed that can't be reached is tried again
    /// at its next interval, not at every check). Returns how many changed
    /// what's kept.
    pub fn sync_due_feeds(&self) -> usize {
        let now = chrono::Utc::now().timestamp();
        let mut read = 0;
        for id in self.calendar_source_ids() {
            let due = self.in_calendar_source(&id, || {
                let Some(server) = self.calendar_server().filter(|s| s.feed) else {
                    return false;
                };
                let minutes = if server.refresh_minutes == 0 {
                    crate::sync::feed::DEFAULT_MINUTES
                } else {
                    server.refresh_minutes.max(crate::sync::feed::SHORTEST_MINUTES)
                };
                let last = self
                    .calendar_synced()
                    .into_iter()
                    .chain(self.cal_pref(FEED_TRIED).and_then(|t| t.parse().ok()))
                    .max();
                last.is_none_or(|t| now - t >= i64::from(minutes) * 60)
            });
            if due && self.in_calendar_source(&id, || self.sync_calendar()).is_ok() {
                read += 1;
            }
        }
        read
    }
    /// Saves an event where the calendars are: CalDAV, or a JMAP server.
    fn calendar_save(&self, server: &caldav::Server, calendar: &str, stored: &Stored) -> Result<Stored> {
        ensure!(!server.feed, t!("manage-calendar-subscribed-calendars-are-read-only"));
        if let Some(a) = self.calendar_google(server)? {
            let invite = !stored.event.attendees.is_empty() && self.organized_here(&stored.event);
            return self.google_save_event(&a, calendar, stored, invite);
        }
        if let Some(a) = self.calendar_proton(server)? {
            return self.proton_save_event(&a, calendar, stored);
        }
        if let Some(a) = self.calendar_ews(server)? {
            let invite = !stored.event.attendees.is_empty() && self.organized_here(&stored.event);
            return self.ews_save_event(&a, calendar, stored, invite);
        }
        if let Some(a) = self.calendar_graph(server)? {
            let calendar_id = calendar.strip_prefix("graph:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
            let event_id = stored.href.strip_prefix("graph:").filter(|i| !i.is_empty());
            // What was there before: only occurrences changed or cancelled
            // since are sent.
            let before = self.stored_event(&stored.event.uid).filter(|b| b.href == stored.href);
            let mut saved = stored.clone();
            self.with_graph_calendars(&a, |s| {
                let id = s.save_event(calendar_id, event_id, &stored.event)?;
                saved.href = format!("graph:{id}");
                saved.etag = String::new();
                if event_id.is_none() || stored.event.recurrence.is_empty() {
                    return Ok(());
                }
                let old_overrides = before.as_ref().map(|b| b.overrides.clone()).unwrap_or_default();
                let old_exdates = before.as_ref().map(|b| b.event.exdates.clone()).unwrap_or_default();
                for o in saved.overrides.iter_mut() {
                    if old_overrides.contains(o) {
                        continue;
                    }
                    let Some(at) = o.recurrence_id else { continue };
                    let known = crate::sync::graph::dav::occurrence_id(o).map(str::to_owned);
                    let Some(occurrence) = known.map_or_else(|| s.occurrence_at(&id, at), |k| Ok(Some(k)))? else {
                        continue;
                    };
                    s.save_event(calendar_id, Some(&occurrence), &Event { recurrence: String::new(), ..o.clone() })?;
                    if crate::sync::graph::dav::occurrence_id(o).is_none() {
                        o.kept.push_str(&format!("X-PICACHO-GRAPH-ID:{occurrence}\r\n"));
                    }
                }
                for at in stored.event.exdates.iter().filter(|at| !old_exdates.contains(at)) {
                    if let Some(occurrence) = s.occurrence_at(&id, *at)? {
                        s.delete_event(&occurrence)?;
                    }
                }
                Ok(())
            })?;
            return Ok(saved);
        }
        let Some(a) = self.calendar_jmap(server)? else {
            let organized_here = self.organized_here(&stored.event);
            let target = self.calendars().into_iter().find(|c| c.href == calendar);
            if let Some(c) = &target {
                caldav::check_limits(&c.limits, stored)?;
            }
            let max_size = target.map(|c| c.max_size);
            return caldav::save(
                server,
                &self.calendar_password()?,
                self.tls_connector()?,
                calendar,
                stored,
                organized_here,
                max_size,
            );
        };
        let calendar_id = calendar
            .strip_prefix("jmap:")
            .with_context(|| t!("manage-calendar-not-a-jmap-calendar"))?;
        let event_id = stored.href.strip_prefix("jmap:").filter(|i| !i.is_empty());
        if stored.event.task {
            // JMAP for Task Lists: in a task list, not a calendar.
            let (id, version) = self.with_jmap_calendars(&a, |s| {
                let id = crate::sync::jscal::save_task(s, event_id, calendar_id, &stored.event, Some(&stored.etag))?;
                Ok((id.clone(), crate::sync::jscal::task_version_of(s, &id).unwrap_or_default()))
            })?;
            return Ok(Stored {
                calendar: calendar.to_owned(),
                href: format!("jmap:{id}"),
                etag: version,
                ..stored.clone()
            });
        }
        let scheduling = self.jmap_scheduling(server, &stored.event);
        let (id, version) = self.with_jmap_calendars(&a, |s| {
            let id = crate::sync::jscal::save_event_scheduled(
                s,
                event_id,
                calendar_id,
                &stored.event,
                &stored.overrides,
                Some(&stored.etag),
                scheduling,
            )?;
            Ok((id.clone(), crate::sync::jscal::version_of(s, &id).unwrap_or_default()))
        })?;
        Ok(Stored {
            calendar: calendar.to_owned(),
            href: format!("jmap:{id}"),
            etag: version,
            ..stored.clone()
        })
    }
    /// Removes an event where the calendars are.
    fn calendar_delete(&self, server: &caldav::Server, stored: &Stored) -> Result<()> {
        ensure!(!server.feed, t!("manage-calendar-subscribed-calendars-are-read-only"));
        if let Some(a) = self.calendar_google(server)? {
            let Some(id) = stored.href.strip_prefix("google:").filter(|i| !i.is_empty()) else {
                return Ok(());
            };
            let calendar = stored.calendar.strip_prefix("google:").unwrap_or_default().to_owned();
            let tell = !stored.event.attendees.is_empty() && self.organized_here(&stored.event);
            return self.with_google(&a, |s| s.delete_event(&calendar, id, tell));
        }
        if let Some(a) = self.calendar_proton(server)? {
            return self.proton_delete_event(&a, stored);
        }
        if let Some(a) = self.calendar_ews(server)? {
            let Some(id) = stored.href.strip_prefix("ews:").filter(|i| !i.is_empty()) else {
                return Ok(());
            };
            let tell = !stored.event.attendees.is_empty() && self.organized_here(&stored.event);
            return self.with_ews_locked(&a, |s| s.delete_event(id, tell));
        }
        if let Some(a) = self.calendar_graph(server)? {
            let Some(id) = stored.href.strip_prefix("graph:").filter(|i| !i.is_empty()) else {
                return Ok(());
            };
            return self.with_graph_calendars(&a, |s| s.delete_event(id));
        }
        let Some(a) = self.calendar_jmap(server)? else {
            return caldav::delete(
                server,
                &self.calendar_password()?,
                self.tls_connector()?,
                stored,
            );
        };
        let Some(id) = stored.href.strip_prefix("jmap:").filter(|i| !i.is_empty()) else {
            return Ok(());
        };
        if stored.event.task {
            return self.with_jmap_calendars(&a, |s| crate::sync::jscal::delete_task(s, id));
        }
        let scheduling = self.jmap_scheduling(server, &stored.event);
        self.with_jmap_calendars(&a, |s| crate::sync::jscal::delete_event_scheduled(s, id, scheduling))
    }
    /// Who tells a JMAP calendar event's participants: the server, where
    /// server scheduling is on and the event is organized here (then
    /// `send_invitations` sends nothing itself); otherwise Picacho, by mail.
    fn jmap_scheduling(&self, server: &caldav::Server, event: &Event) -> crate::sync::jscal::Scheduling {
        if server.server_schedules && self.organized_here(event) {
            crate::sync::jscal::Scheduling::ByServer
        } else {
            crate::sync::jscal::Scheduling::ByPicacho
        }
    }
    /// Makes a new calendar on the server (MKCALENDAR, or Calendar/set),
    /// returning it.
    pub fn create_calendar(&self, name: &str, color: &str) -> Result<caldav::Calendar> {
        ensure!(
            !name.trim().is_empty(),
            t!("manage-calendar-give-the-calendar-a-name")
        );
        if let Some(id) = self.route_default() {
            return self.in_calendar_source(&id, || self.create_calendar(name, color));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-set-up-a-calendar-server-in"))?;
        ensure!(!server.feed, t!("manage-calendar-subscribed-calendars-are-read-only"));
        ensure!(server.proton_account.is_empty(), t!("manage-calendar-proton-calendars-are-made-in-proton"));
        let made = if let Some(a) = self.calendar_google(&server)? {
            let id = self.with_google(&a, |s| {
                let id = s.create_calendar(name.trim())?;
                if !color.is_empty() {
                    s.update_calendar(&id, name.trim(), color)?;
                }
                Ok(id)
            })?;
            caldav::Calendar {
                href: format!("google:{id}"),
                name: name.trim().to_owned(),
                writable: true,
                color: color.to_owned(),
                components: vec!["VEVENT".into()],
                ..Default::default()
            }
        } else if let Some(a) = self.calendar_ews(&server)? {
            let id = self.with_ews_locked(&a, |s| s.create_calendar(name.trim()))?;
            caldav::Calendar {
                href: format!("ews:{id}"),
                name: name.trim().to_owned(),
                writable: true,
                components: vec!["VEVENT".into()],
                ..Default::default()
            }
        } else if let Some(a) = self.calendar_graph(&server)? {
            let id = self.with_graph_calendars(&a, |s| s.create_calendar(name.trim(), color))?;
            caldav::Calendar {
                href: format!("graph:{id}"),
                name: name.trim().to_owned(),
                writable: true,
                color: color.to_owned(),
                ..Default::default()
            }
        } else { match self.calendar_jmap(&server)? {
            Some(a) => {
                let id = self.with_jmap_calendars(&a, |s| {
                    crate::sync::jscal::create_calendar(s, name.trim(), color)
                })?;
                caldav::Calendar {
                    href: format!("jmap:{id}"),
                    name: name.trim().to_owned(),
                    writable: true,
                    color: color.to_owned(),
                    ..Default::default()
                }
            }
            None => caldav::create_calendar(
                &server,
                &self.calendar_password()?,
                self.tls_connector()?,
                name.trim(),
                color,
                // RFC 4791 §5.2.2: the calendar's floating times and
                // time-range queries are in this computer's zone.
                &crate::sync::dav::CollectionSetup { timezone: local_zone(), ..Default::default() },
            )?,
        } };
        let mut cals = self.calendars();
        cals.push(made.clone());
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)?;
        Ok(made)
    }
    /// Reads a JMAP server's calendars and their events: once read, only
    /// the events changed since (CalendarEvent/changes), until the time
    /// range has to move on or the server can't say; then all again.
    fn sync_jmap_calendar(&self, a: &Account) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let (saved_state, until) = self.cal_pref(JMAP_STATE)
            .and_then(|p| {
                let (state, until) = p.rsplit_once('@')?;
                Some((state.to_owned(), until.parse::<i64>().ok()?))
            })
            .unwrap_or_default();
        let mut known = self.calendar_events();
        let in_range = until > now + AHEAD / 2 && !saved_state.is_empty();
        let (cals, changed) = self.with_jmap_calendars(a, |s| {
            let cals = crate::sync::jscal::calendars(s)?;
            let ids: Vec<String> = cals.iter().map(|c| c.id.clone()).collect();
            let changed = if in_range {
                crate::sync::jscal::event_changes(s, &ids, &saved_state)?
            } else {
                None
            };
            // Neither is worth failing the sync over.
            if let Ok(people) = crate::sync::jscal::participant_identities(s) {
                let _ = self.set_cal_pref(PARTICIPANTS, &serde_json::to_string(&people).unwrap_or_default());
            }
            if let Ok(found) = crate::sync::jscal::notices(s)
                && !found.is_empty()
            {
                let mut kept = self.calendar_notices();
                kept.extend(found.iter().map(|n| n.text.clone()));
                keep_recent(&mut kept);
                // Shown here from now on; the server's copies go.
                if self.set_cal_pref(NOTICES, &serde_json::to_string(&kept).unwrap_or_default())
                    .is_ok()
                {
                    let _ = crate::sync::jscal::dismiss_notices(s, &found);
                }
            }
            let read = match changed {
                Some((events, gone, state)) => (events, Some(gone), state, until),
                None => {
                    let (events, state) =
                        crate::sync::jscal::events_in(s, &ids, now - BEHIND, now + AHEAD)?;
                    (events, None, state, now + AHEAD)
                }
            };
            Ok((cals, read))
        })?;
        let (events, gone, state, read_until) = changed;
        let cals: Vec<caldav::Calendar> = cals
            .into_iter()
            .map(|c| caldav::Calendar {
                href: format!("jmap:{}", c.id),
                name: c.name,
                writable: c.writable,
                color: c.color,
                ..Default::default()
            })
            .collect();
        let events: Vec<Stored> = events
            .into_iter()
            .map(|e| Stored {
                etag: crate::sync::jscal::event_version(&e.json),
                event: e.event,
                overrides: e.overrides,
                calendar: format!("jmap:{}", e.calendar),
                href: format!("jmap:{}", e.id),
                schedule_tag: String::new(),
            })
            .collect();
        let count = events.len();
        let all = match gone {
            // Only changes: those replace or remove what was kept.
            Some(gone) => {
                let gone: std::collections::HashSet<&str> = gone.iter().map(String::as_str).collect();
                let replaced: std::collections::HashSet<&str> =
                    events.iter().map(|e| e.href.as_str()).collect();
                known.retain(|k| {
                    let id = k.href.strip_prefix("jmap:").unwrap_or_default();
                    !gone.contains(id) && !replaced.contains(k.href.as_str())
                });
                known.extend(events);
                known
            }
            None => events,
        };
        self.save_events(all)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)?;
        self.set_cal_pref(JMAP_STATE, &format!("{state}@{read_until}"))?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        Ok(count)
    }
    /// What a JMAP server said about shared calendars and events (shared
    /// with you, changed by someone else…), not yet dismissed.
    pub fn calendar_notices(&self) -> Vec<String> {
        if !within_one() {
            return self.calendar_source_ids().into_iter().flat_map(|id| self.in_calendar_source(&id, || self.calendar_notices())).collect();
        }
        self.cal_pref(NOTICES)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    /// Forgets the notices once read.
    pub fn dismiss_calendar_notices(&self) -> Result<()> {
        if !within_one() {
            for id in self.calendar_source_ids() {
                self.in_calendar_source(&id, || self.dismiss_calendar_notices())?;
            }
            return Ok(());
        }
        self.set_cal_pref(NOTICES, "[]")
    }
    /// Open tasks from the calendar server, soonest due first.
    pub fn calendar_tasks(&self) -> Vec<Stored> {
        if !within_one() {
            let mut all: Vec<Stored> =
                self.calendar_source_ids().into_iter().flat_map(|id| self.in_calendar_source(&id, || self.calendar_tasks())).collect();
            caldav::sort_tasks(&mut all);
            return all;
        }
        self.cal_pref(TASKS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    /// Reads the open tasks again: CalDAV servers' VTODOs, or a JMAP
    /// server's tasks (JMAP for Task Lists), where it keeps them.
    pub fn sync_tasks(&self) -> Result<usize> {
        if !within_one() {
            return together(self.each_calendar_source(|| self.sync_tasks()), |id| self.calendar_source_label(id));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        if let Some(a) = self.calendar_jmap(&server)? {
            return self.sync_jmap_tasks(&a);
        }
        // Exchange's, Google's and Proton's tasks aren't kept here; feeds have none.
        if !server.dav() {
            return Ok(0);
        }
        let tasks = caldav::tasks(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &self.calendars(),
        )?;
        let count = tasks.len();
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        Ok(count)
    }
    /// Reads a JMAP server's task lists and the tasks still to do in them;
    /// a server without JMAP for Task Lists has none.
    fn sync_jmap_tasks(&self, a: &Account) -> Result<usize> {
        use crate::sync::jscal;
        let now = chrono::Utc::now().timestamp();
        let saved = self.cal_pref(TASK_STATE).unwrap_or_default();
        let (saved_lists, saved_state) = saved.split_once('@').unwrap_or_default();
        let known = self.calendar_tasks();
        // Repeating tasks are read again each time: the occurrence to do
        // moves on with time, not only with changes.
        let again: Vec<String> = known
            .iter()
            .filter(|t| !t.event.recurrence.is_empty())
            .filter_map(|t| t.href.strip_prefix("jmap:").map(str::to_owned))
            .collect();
        let (lists, read) = self.with_jmap_calendars(a, |s| {
            let lists = jscal::task_lists(s)?;
            let fingerprint = task_lists_fingerprint(&lists);
            let changed = if fingerprint == saved_lists && !saved_state.is_empty() && s.tasks_account().is_some() {
                jscal::task_changes(s, &lists, saved_state, &again, now)?
            } else {
                None
            };
            let read = match changed {
                Some((open, gone, state)) => (open, Some(gone), state),
                None => {
                    let (open, state) = jscal::open_tasks(s, &lists, now)?;
                    (open, None, state)
                }
            };
            Ok((lists, (read, fingerprint)))
        })?;
        let ((found, gone, state), fingerprint) = read;
        self.save_task_lists(lists)?;
        let found: Vec<Stored> = found
            .into_iter()
            .map(|t| Stored {
                etag: t.version,
                event: t.event,
                calendar: format!("jmap:{}", t.list),
                href: format!("jmap:{}", t.id),
                ..Default::default()
            })
            .collect();
        let mut tasks = match gone {
            // Only changes: those replace or remove what was kept.
            Some(gone) => {
                let mut kept = known;
                kept.retain(|k| {
                    let id = k.href.strip_prefix("jmap:").unwrap_or_default();
                    !gone.iter().any(|g| g == id) && !found.iter().any(|f| f.href == k.href)
                });
                kept.extend(found);
                kept
            }
            None => found,
        };
        caldav::sort_tasks(&mut tasks);
        let count = tasks.len();
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        self.set_cal_pref(TASK_STATE, &if state.is_empty() { String::new() } else { format!("{fingerprint}@{state}") })?;
        Ok(count)
    }
    /// Keeps a JMAP server's task lists that are shown: the default one
    /// first, then as the server orders them (`sortOrder`, then name).
    fn save_task_lists(&self, mut lists: Vec<crate::sync::jscal::JmapTaskList>) -> Result<()> {
        lists.sort_by(|a, b| {
            (!a.default, a.sort_order, &a.name).cmp(&(!b.default, b.sort_order, &b.name))
        });
        let lists: Vec<caldav::Calendar> = lists
            .into_iter()
            .filter(|l| l.shown)
            .map(|l| caldav::Calendar {
                href: format!("jmap:{}", l.id),
                name: l.name,
                writable: l.writable,
                color: l.color,
                components: vec!["VTODO".into()],
                ..Default::default()
            })
            .collect();
        self.set_cal_pref(TASK_LISTS, &serde_json::to_string(&lists)?)
    }
    /// The task list new tasks go to on a JMAP server, made where the
    /// account has none it can write to. One that is hidden or not
    /// subscribed to isn't used (the task would vanish from the list), nor
    /// another made beside it.
    fn jmap_task_list(&self, a: &Account) -> Result<String> {
        if let Some(list) = self.task_calendar() {
            return Ok(list.href);
        }
        let lists = self.with_jmap_calendars(a, |s| {
            ensure!(
                s.tasks_account().is_some(),
                t!("manage-calendar-this-jmap-server-doesnt-keep-tasks")
            );
            let mut lists = crate::sync::jscal::task_lists(s)?;
            if !lists.iter().any(|l| l.writable) {
                // A new list is subscribed to and shown (§3).
                let id = crate::sync::jscal::create_task_list(s, t!("manage-calendar-tasks"))?;
                lists = crate::sync::jscal::task_lists(s)?;
                // A server slow to list it: as made.
                if !lists.iter().any(|l| l.id == id) {
                    lists.push(crate::sync::jscal::JmapTaskList {
                        id,
                        name: t!("manage-calendar-tasks").to_owned(),
                        color: String::new(),
                        writable: true,
                        default: false,
                        shown: true,
                        ..Default::default()
                    });
                }
            }
            Ok(lists)
        })?;
        self.save_task_lists(lists)?;
        Ok(self
            .task_calendar()
            .with_context(|| t!("manage-calendar-none-of-your-calendars-can-take"))?
            .href)
    }
    /// Marks a task done (or not) on the server.
    pub fn complete_task(&self, uid: &str, done: bool) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.complete_task(uid, done));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut tasks = self.calendar_tasks();
        let task = tasks
            .iter_mut()
            .find(|t| t.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
        task.event.completed = done;
        // The source is kept: only what changed is rewritten (`ical::update_task`).
        task.event.sequence += 1;
        let calendar = task.calendar.clone();
        *task = self.calendar_save(&server, &calendar, task)?;
        let repeats = !task.event.recurrence.is_empty();
        if done {
            tasks.retain(|t| t.event.uid != uid);
        }
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        // A repeating JMAP task: only that occurrence is done (or not), so
        // the one to do next is read.
        if repeats && self.calendar_jmap(&server)?.is_some() {
            self.sync_tasks()?;
        }
        Ok(())
    }
    /// Changes a task's name, notes and due date (`due` 0: none), keeping
    /// the rest of it as the server has it.
    pub fn edit_task(
        &self,
        uid: &str,
        summary: &str,
        notes: &str,
        due: i64,
        all_day: bool,
    ) -> Result<Stored> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.edit_task(uid, summary, notes, due, all_day));
        }
        ensure!(
            !summary.trim().is_empty(),
            t!("manage-calendar-give-the-task-a-name")
        );
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut tasks = self.calendar_tasks();
        let task = tasks
            .iter_mut()
            .find(|t| t.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-task-is-no-longer-on"))?;
        task.event.summary = summary.trim().to_owned();
        task.event.description = notes.trim().to_owned();
        task.event.start = due;
        task.event.end = due;
        task.event.all_day = all_day && due != 0;
        task.event.sequence += 1;
        let calendar = task.calendar.clone();
        *task = self.calendar_save(&server, &calendar, task)?;
        // Its reminders move with its due date (or go with it), as its
        // VALARMs say: those at a time stay.
        if !task.event.source.is_empty() {
            task.event.alarms = ical::task_alarms(&task.event.source, due);
        }
        let saved = task.clone();
        caldav::sort_tasks(&mut tasks);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        // A repeating JMAP task's occurrence moved: the one to do may be
        // another now.
        if !saved.event.recurrence.is_empty() && self.calendar_jmap(&server)?.is_some() {
            self.sync_tasks()?;
        }
        Ok(saved)
    }
    /// Deletes a task from the server.
    pub fn delete_task(&self, uid: &str) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.delete_task(uid));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut tasks = self.calendar_tasks();
        if let Some(task) = tasks.iter().find(|t| t.event.uid == uid) {
            self.calendar_delete(&server, task)?;
        }
        tasks.retain(|t| t.event.uid != uid);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)
    }
    /// Adds a task due at `due` (0: no due date), in the calendar new
    /// events go to.
    pub fn add_task(&self, summary: &str, due: i64) -> Result<Stored> {
        if !within_one() {
            // Where new events go, where it keeps tasks; else the first that does.
            let takes = |id: &String| {
                self.in_calendar_source(id, || {
                    self.calendar_server().is_some_and(|s| !s.jmap_account.is_empty()) || self.task_calendar().is_some()
                })
            };
            let first = self.default_calendar_source();
            let id = std::iter::once(first.clone())
                .chain(self.calendar_source_ids())
                .find(takes)
                .unwrap_or(first);
            return self.in_calendar_source(&id, || self.add_task(summary, due));
        }
        ensure!(
            !summary.trim().is_empty(),
            t!("manage-calendar-give-the-task-a-name")
        );
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-set-up-a-calendar-server-in"))?;
        let calendar = match self.calendar_jmap(&server)? {
            Some(a) => self.jmap_task_list(&a)?,
            None => self
                .task_calendar()
                .with_context(|| t!("manage-calendar-none-of-your-calendars-can-take"))?
                .href,
        };
        let stored = Stored {
            event: Event {
                uid: format!("{}@picacho", uuid::Uuid::new_v4()),
                summary: summary.trim().to_owned(),
                start: due,
                end: due,
                task: true,
                utc: true,
                ..Default::default()
            },
            calendar: calendar.clone(),
            ..Default::default()
        };
        let saved = self.calendar_save(&server, &calendar, &stored)?;
        let mut tasks = self.calendar_tasks();
        tasks.push(saved.clone());
        caldav::sort_tasks(&mut tasks);
        self.set_cal_pref(TASKS, &serde_json::to_string(&tasks)?)?;
        Ok(saved)
    }
    /// The reminders due by `now` that haven't been shown: (event, start of
    /// the occurrence). Each is shown once; reminders more than a day late
    /// (the computer was off) are skipped.
    pub fn due_reminders(&self, now: i64) -> Vec<(Event, i64)> {
        // Ordered: a HashSet listed its keys in a new order each time, so
        // the unchanged list was written again on every check.
        let mut shown: std::collections::BTreeSet<String> = self
            .store
            .preference(REMINDED)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let mut due = Vec::new();
        // Tasks, whose reminders are before they're due (CalDAV's VALARMs
        // by `ical::task_alarms`, JMAP's alerts by `jscal::task_alarms`).
        let tasks = self
            .calendar_tasks()
            .into_iter()
            .filter(|t| t.event.start != 0)
            .map(|t| t.event);
        // Occurrences starting up to a week ahead can have reminders now.
        for e in self.agenda(now - 86_400, now + 7 * 86_400).into_iter().chain(tasks) {
            for before in &e.alarms {
                let at = e.start - before;
                let key = format!("{}@{}@{before}", e.uid, e.start);
                if at <= now && at > now - 86_400 && !shown.contains(&key) {
                    shown.insert(key);
                    due.push((e.clone(), e.start));
                }
            }
        }
        // Keep only keys for the last week.
        let before = shown.len();
        shown.retain(|k| {
            k.rsplit('@')
                .nth(1)
                .and_then(|t| t.parse::<i64>().ok())
                .is_some_and(|start| start > now - 7 * 86_400)
        });
        if (!due.is_empty() || shown.len() != before)
            && let Ok(json) = serde_json::to_string(&shown)
        {
            let _ = self.store.set_preference(REMINDED, &json);
        }
        due
    }
    /// When `attendees` are busy over `from`..`to`, as the calendar server
    /// knows (CalDAV scheduling). Asked as the account `organizer`.
    pub fn free_busy(
        &self,
        organizer: &str,
        attendees: &[String],
        from: i64,
        to: i64,
    ) -> Result<Vec<ical::FreeBusy>> {
        ensure!(
            !attendees.is_empty(),
            t!("manage-calendar-add-someone-to-see-when-theyre")
        );
        if let Some(id) = self.route_default() {
            return self.in_calendar_source(&id, || self.free_busy(organizer, attendees, from, to));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-set-up-a-calendar-server-in"))?;
        ensure!(!server.feed && server.proton_account.is_empty(), t!("manage-calendar-this-calendar-cant-say-when-people"));
        if let Some(a) = self.calendar_google(&server)? {
            return self.with_google(&a, |s| s.free_busy(attendees, from, to));
        }
        if let Some(a) = self.calendar_jmap(&server)? {
            return self.with_jmap_calendars(&a, |s| {
                crate::sync::jscal::free_busy(s, attendees, from, to)
            });
        }
        if let Some(a) = self.calendar_graph(&server)? {
            return self.with_graph_calendars(&a, |s| s.free_busy(attendees, from, to));
        }
        if let Some(a) = self.calendar_ews(&server)? {
            return self.with_ews_locked(&a, |s| s.free_busy(attendees, from, to));
        }
        let password = self.calendar_password()?;
        // Your own busy time from your calendars (RFC 4791 §7.10
        // free-busy-query), which every CalDAV server answers; the
        // scheduling outbox (RFC 6638 §5) for everyone else's.
        let (me, others): (Vec<String>, Vec<String>) =
            attendees.iter().cloned().partition(|a| a.eq_ignore_ascii_case(organizer));
        let mut out = Vec::new();
        if !me.is_empty() {
            let busy = caldav::own_free_busy(&server, &password, self.tls_connector()?, &self.calendars(), from, to)?;
            out.push(ical::FreeBusy { attendee: organizer.to_owned(), busy });
        }
        if !others.is_empty() {
            out.extend(caldav::free_busy(&server, &password, self.tls_connector()?, organizer, &others, from, to)?);
        }
        Ok(out)
    }
    /// Reads the calendars and their events again.
    pub fn sync_calendar(&self) -> Result<usize> {
        if !within_one() {
            return together(self.each_calendar_source(|| self.sync_calendar()), |id| self.calendar_source_label(id));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        if server.feed {
            return self.sync_feed(&server);
        }
        if let Some(a) = self.calendar_google(&server)? {
            return self.sync_google_calendar(&a);
        }
        if let Some(a) = self.calendar_proton(&server)? {
            return self.sync_proton_calendar(&a);
        }
        if let Some(a) = self.calendar_jmap(&server)? {
            return self.sync_jmap_calendar(&a);
        }
        if let Some(a) = self.calendar_graph(&server)? {
            return self.sync_graph_calendar(&a);
        }
        if let Some(a) = self.calendar_ews(&server)? {
            return self.sync_ews_calendar(&a);
        }
        let password = self.calendar_password()?;
        let previous = self.calendars();
        let cals = caldav::calendars(&server, &password, self.tls_connector()?)?;
        let now = chrono::Utc::now().timestamp();
        // Only what changed since the last sync is downloaded, where the
        // server can say (sync token or ctag).
        let (cals, events) = caldav::sync(
            &server,
            &password,
            self.tls_connector()?,
            &cals,
            &previous,
            &self.calendar_events(),
            (now - BEHIND, now + AHEAD),
        )?;
        let count = events.len();
        self.save_events(events)?;
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)?;
        self.set_cal_pref(SYNCED, &now.to_string())?;
        // With the server scheduling, what it delivered: not worth failing
        // the sync over.
        if server.server_schedules {
            let _ = self.read_schedule_inbox(&server, &password);
        }
        Ok(count)
    }
    /// The scheduling inbox (RFC 6638 §4) when the server sends invitations:
    /// guests' answers to your events and invitations to you arrive there,
    /// not by mail. The server has already applied answers to your events
    /// (and put invitations in your calendar, which the sync just read), so
    /// each is noted for showing, then removed from the inbox as handled.
    fn read_schedule_inbox(&self, server: &caldav::Server, password: &str) -> Result<usize> {
        let items = caldav::inbox(server, password, self.tls_connector()?)?;
        if items.is_empty() {
            return Ok(0);
        }
        let mut notes = self.calendar_notices();
        // Only what was told is removed: an item of another kind (a
        // counter-proposal, a refresh) stays for an app that handles it.
        let mut handled = Vec::new();
        for item in &items {
            let cal = ical::parse(&item.data);
            if matches!(cal.method.to_ascii_uppercase().as_str(), "REPLY" | "REQUEST" | "CANCEL") {
                handled.push(item);
            }
            for e in &cal.events {
                let who = |a: &ical::Attendee| {
                    if a.name.is_empty() { a.email.clone() } else { a.name.clone() }
                };
                let note = match cal.method.to_ascii_uppercase().as_str() {
                    "REPLY" => e.attendees.first().map(|a| {
                        let answer = match a.status {
                            Some(PartStat::Accepted) => "accepted",
                            Some(PartStat::Declined) => "declined",
                            Some(PartStat::Tentative) => "tentative",
                            _ => "other",
                        };
                        t!(
                            "manage-calendar-guest-answered",
                            answer = answer,
                            who = who(a),
                            event = e.summary
                        )
                    }),
                    "REQUEST" => Some(match e.organizer.as_ref() {
                        Some(o) => t!(
                            "manage-calendar-invited-you",
                            who = who(o),
                            event = e.summary
                        ),
                        None => t!("manage-calendar-someone-invited-you", event = e.summary),
                    }),
                    "CANCEL" => Some(t!("manage-calendar-was-cancelled", event = e.summary)),
                    _ => None,
                };
                notes.extend(note);
            }
        }
        keep_recent(&mut notes);
        self.set_cal_pref(NOTICES, &serde_json::to_string(&notes)?)?;
        let tls = self.tls_connector()?;
        for item in &handled {
            let _ = caldav::remove_from_inbox(server, password, tls.clone(), item);
        }
        Ok(handled.len())
    }
    /// Renames a calendar (and changes its colour; empty: as it is), on its
    /// server: PROPPATCH, or Calendar/set.
    pub fn rename_calendar(&self, href: &str, name: &str, color: &str) -> Result<()> {
        ensure!(
            !name.trim().is_empty(),
            t!("manage-calendar-give-the-calendar-a-name")
        );
        if let Some(id) = self.route_calendar(href) {
            return self.in_calendar_source(&id, || self.rename_calendar(href, name, color));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        if server.feed {
            // A feed's name is the person's to choose; nothing is written.
            let mut server = server;
            server.label = name.trim().to_owned();
            self.set_cal_pref(SERVER, &serde_json::to_string(&server)?)?;
        } else if !server.proton_account.is_empty() {
            bail!(t!("manage-calendar-proton-calendars-are-made-in-proton"));
        } else if let Some(a) = self.calendar_google(&server)? {
            let id = href.strip_prefix("google:").with_context(|| t!("manage-calendar-not-a-google-calendar"))?;
            self.with_google(&a, |s| s.update_calendar(id, name.trim(), color))?;
        } else if let Some(a) = self.calendar_ews(&server)? {
            let id = href.strip_prefix("ews:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
            self.with_ews_locked(&a, |s| s.rename_any_folder(id, name.trim()))?;
        } else if let Some(a) = self.calendar_graph(&server)? {
            let id = href.strip_prefix("graph:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
            self.with_graph_calendars(&a, |s| s.update_calendar(id, name.trim(), color))?;
        } else { match self.calendar_jmap(&server)? {
            Some(a) => {
                let id = href
                    .strip_prefix("jmap:")
                    .with_context(|| t!("manage-calendar-not-a-jmap-calendar"))?;
                self.with_jmap_calendars(&a, |s| {
                    crate::sync::jscal::update_calendar(s, id, name.trim(), color)
                })?
            }
            None => caldav::rename_calendar(
                &server,
                &self.calendar_password()?,
                self.tls_connector()?,
                href,
                name,
                color,
            )?,
        } }
        let mut cals = self.calendars();
        for c in cals.iter_mut().filter(|c| c.href == href) {
            c.name = name.trim().to_owned();
            if !color.is_empty() {
                c.color = color.to_owned();
            }
        }
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)
    }
    /// Deletes a calendar and its events from the server.
    pub fn delete_calendar(&self, href: &str) -> Result<()> {
        if let Some(id) = self.route_calendar(href) {
            return self.in_calendar_source(&id, || self.delete_calendar(href));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        if server.feed {
            // Unsubscribing: the feed goes, as a source of its own.
            let id = pim_sources::current(Pim::Calendar).unwrap_or_default();
            return if id.is_empty() { self.disconnect_calendar() } else { self.remove_calendar_source(&id) };
        }
        ensure!(server.proton_account.is_empty(), t!("manage-calendar-proton-calendars-are-made-in-proton"));
        if let Some(a) = self.calendar_google(&server)? {
            let id = href.strip_prefix("google:").with_context(|| t!("manage-calendar-not-a-google-calendar"))?;
            self.with_google(&a, |s| s.delete_calendar(id))?;
        } else if let Some(a) = self.calendar_ews(&server)? {
            let id = href.strip_prefix("ews:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
            self.with_ews_locked(&a, |s| s.delete_any_folder(id))?;
        } else if let Some(a) = self.calendar_graph(&server)? {
            let id = href.strip_prefix("graph:").with_context(|| t!("manage-calendar-not-an-exchange-calendar"))?;
            self.with_graph_calendars(&a, |s| s.delete_calendar(id))?;
        } else { match self.calendar_jmap(&server)? {
            Some(a) => {
                let id = href
                    .strip_prefix("jmap:")
                    .with_context(|| t!("manage-calendar-not-a-jmap-calendar"))?;
                self.with_jmap_calendars(&a, |s| crate::sync::jscal::delete_calendar(s, id))?
            }
            None => caldav::delete_calendar(
                &server,
                &self.calendar_password()?,
                self.tls_connector()?,
                href,
            )?,
        } }
        let mut cals = self.calendars();
        cals.retain(|c| c.href != href);
        self.set_cal_pref(CALENDARS, &serde_json::to_string(&cals)?)?;
        let mut events = self.calendar_events();
        events.retain(|e| e.calendar != href);
        self.save_events(events)?;
        if self.cal_pref(DEFAULT).as_deref() == Some(href) {
            self.set_cal_pref(DEFAULT, "")?;
        }
        Ok(())
    }
    /// The calendar server's storage, used and left in octets, where it
    /// says (CalDAV's RFC 4331 quota).
    pub fn calendar_quota(&self) -> Result<Option<(u64, u64)>> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.calendar_quota());
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        if !server.dav() {
            return Ok(None);
        }
        caldav::quota(&server, &self.calendar_password()?, self.tls_connector()?)
    }
    /// Attaches a file to an event on a calendar that keeps attachments
    /// itself (RFC 8607), then reads the calendar again to show it.
    pub fn attach_to_event(&self, uid: &str, path: &std::path::Path) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.attach_to_event(uid, path));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        ensure!(
            server.dav(),
            t!("manage-calendar-attach-files-to-events-on-a")
        );
        let stored = self
            .stored_event(uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in"))?;
        caldav::attach(&server, &self.calendar_password()?, self.tls_connector()?, &stored, path, None, None)?;
        self.sync_calendar()?;
        Ok(())
    }
    /// Takes a file the calendar server keeps (its MANAGED-ID, RFC 8607
    /// §3.6) off an event, then reads the calendar again.
    pub fn remove_event_attachment(&self, uid: &str, managed_id: &str) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.remove_event_attachment(uid, managed_id));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        ensure!(
            server.dav(),
            t!("manage-calendar-attachments-are-kept-on-caldav-calendars")
        );
        let stored = self
            .stored_event(uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in"))?;
        caldav::remove_attachment(&server, &self.calendar_password()?, self.tls_connector()?, &stored, managed_id, None)?;
        self.sync_calendar()?;
        Ok(())
    }
    /// Disconnects the calendar source worked within (the first, outside
    /// any), forgetting what it kept here.
    pub fn disconnect_calendar(&self) -> Result<()> {
        if let Some(id) = self.route_first() {
            return self.in_calendar_source(&id, || self.disconnect_calendar());
        }
        let id = pim_sources::current(Pim::Calendar).unwrap_or_default();
        self.clear_source_keys(&id, SOURCE_KEYS)?;
        #[cfg(not(test))]
        store::delete_secret(&key(PASSWORD))?;
        crate::manage::widgets::publish_soon(self.store.clone());
        Ok(())
    }
    /// Adds an event, or saves changes to one (by UID), on the server.
    pub fn save_event(&self, mut event: Event, calendar: Option<&str>) -> Result<Stored> {
        ensure!(
            !event.summary.trim().is_empty(),
            t!("manage-calendar-give-the-event-a-title")
        );
        ensure!(
            event.end >= event.start,
            t!("manage-calendar-the-event-ends-before-it-starts")
        );
        if !within_one() {
            let from = self.calendar_source_of_event(&event.uid);
            let to = calendar
                .and_then(|c| self.calendar_source_of(c))
                .or_else(|| from.clone())
                .unwrap_or_else(|| self.default_calendar_source());
            if let Some(from) = from.filter(|f| *f != to) {
                // To another account's calendar: made there, then taken out
                // of the one it was in.
                let uid = event.uid.clone();
                let mut fresh = event;
                fresh.source.clear();
                let saved = self.in_calendar_source(&to, || self.save_event(fresh, calendar))?;
                self.in_calendar_source(&from, || self.delete_event(&uid))?;
                return Ok(saved);
            }
            return self.in_calendar_source(&to, || self.save_event(event, calendar));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-set-up-a-calendar-server-in"))?;
        if event.tzid.is_empty() && !event.all_day && !event.utc {
            event.tzid = local_zone().unwrap_or_default();
        }
        if event.uid.is_empty() {
            let domain = self
                .store
                .accounts()?
                .first()
                .map(|a| a.email.rsplit('@').next().unwrap_or("picacho").to_owned())
                .unwrap_or_else(|| "picacho".into());
            event.uid = format!("{}@{domain}", uuid::Uuid::new_v4());
        }
        let mut events = self.calendar_events();
        let existing = events.iter().find(|s| s.event.uid == event.uid).cloned();
        let target = match (calendar, &existing) {
            (Some(c), _) => c.to_owned(),
            (None, Some(s)) => s.calendar.clone(),
            (None, None) => {
                self.default_calendar()
                    .with_context(|| t!("manage-calendar-none-of-your-calendars-can-take-2"))?
                    .href
            }
        };
        let mut stored = existing.unwrap_or_default();
        // Where it was, for taking it out of a calendar it moves from.
        let before = stored.clone();
        let moving = !stored.href.is_empty() && stored.calendar != target;
        // The old copy is left to delete where the server didn't move it.
        let mut left_behind = false;
        if moving {
            // RFC 4918 §9.9: moved on the server, keeping it whole, where
            // both calendars are there (CalDAV; JMAP moves by saving).
            match self.calendar_move(&server, &stored, &target) {
                Ok(Some(moved)) => stored = moved,
                // Else a new resource there, and the old one deleted.
                _ => {
                    stored.href.clear();
                    stored.etag.clear();
                    stored.schedule_tag.clear();
                    left_behind = true;
                }
            }
        }
        if !stored.href.is_empty() || moving {
            event.sequence = stored.event.sequence + 1;
            // Edited here: written from its fields.
            event.source.clear();
            // A series keeps its zone and the repeats left out.
            if event.tzid.is_empty() {
                event.tzid = stored.event.tzid.clone();
                event.utc = stored.event.utc;
            }
            if event.exdates.is_empty() {
                event.exdates = stored.event.exdates.clone();
            }
        }
        stored.event = event;
        let saved = self.calendar_save(&server, &target, &stored)?;
        if left_behind {
            let _ = self.calendar_delete(&server, &before);
        }
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved.clone());
        self.save_events(events)?;
        Ok(saved)
    }
    /// Moves an event's resource to the calendar `target` on the server
    /// (MOVE, RFC 4918 §9.9), returning it there. `None` where it can't
    /// be (JMAP), so it is saved anew instead.
    fn calendar_move(&self, server: &caldav::Server, stored: &Stored, target: &str) -> Result<Option<Stored>> {
        if !server.dav() || target.starts_with("jmap:") {
            return Ok(None);
        }
        caldav::move_event(server, &self.calendar_password()?, self.tls_connector()?, stored, target).map(Some)
    }
    /// Every time an event happens between `from` and `to`: repeating
    /// events once for each repeat, changed and cancelled repeats as they
    /// now are. Soonest first.
    pub fn agenda(&self, from: i64, to: i64) -> Vec<Event> {
        self.agenda_in(from, to, |_| true)
    }
    /// `agenda`, of the calendars (hrefs) `shown` keeps.
    pub fn agenda_in(&self, from: i64, to: i64, shown: impl Fn(&str) -> bool) -> Vec<Event> {
        let mut out: Vec<Event> = self
            .cached_events()
            .iter()
            .filter(|s| shown(&s.calendar))
            .flat_map(|s| ical::occurrences(&s.event, &s.overrides, from, to))
            .collect();
        out.sort_by_key(|e| e.start);
        out
    }
    /// The calendar (href) each event and task is in, by uid.
    pub fn event_calendars(&self) -> std::collections::HashMap<String, String> {
        self.cached_events()
            .iter()
            .map(|s| (s.event.uid.clone(), s.calendar.clone()))
            .chain(self.calendar_tasks().into_iter().map(|t| (t.event.uid, t.calendar)))
            .collect()
    }
    /// Changes one repeat of a repeating event (the one that started at
    /// `occurrence`), leaving the others as they are.
    pub fn save_occurrence(&self, uid: &str, occurrence: i64, mut changed: Event) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.save_occurrence(uid, occurrence, changed));
        }
        ensure!(
            !changed.summary.trim().is_empty(),
            t!("manage-calendar-give-the-event-a-title")
        );
        ensure!(
            changed.end >= changed.start,
            t!("manage-calendar-the-event-ends-before-it-starts")
        );
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut events = self.calendar_events();
        let stored = events
            .iter_mut()
            .find(|s| s.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in"))?;
        changed.uid = uid.to_owned();
        changed.recurrence = String::new();
        changed.exdates.clear();
        changed.recurrence_id = Some(occurrence);
        changed.tzid = stored.event.tzid.clone();
        changed.utc = stored.event.utc;
        changed.all_day = stored.event.all_day;
        changed.sequence = stored.event.sequence + 1;
        changed.source.clear();
        stored
            .overrides
            .retain(|o| o.recurrence_id != Some(occurrence));
        stored.overrides.push(changed);
        stored.event.sequence += 1;
        stored.event.source.clear();
        let calendar = stored.calendar.clone();
        let saved = self.calendar_save(&server, &calendar, stored)?;
        *stored = saved;
        self.save_events(events)
    }
    /// Takes one repeat of a repeating event out (EXDATE), leaving the others.
    pub fn delete_occurrence(&self, uid: &str, occurrence: i64) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.delete_occurrence(uid, occurrence));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut events = self.calendar_events();
        let stored = events
            .iter_mut()
            .find(|s| s.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in"))?;
        stored
            .overrides
            .retain(|o| o.recurrence_id != Some(occurrence));
        if !stored.event.exdates.contains(&occurrence) {
            stored.event.exdates.push(occurrence);
        }
        stored.event.sequence += 1;
        stored.event.source.clear();
        let calendar = stored.calendar.clone();
        let saved = self.calendar_save(&server, &calendar, stored)?;
        *stored = saved;
        self.save_events(events)
    }
    pub fn delete_event(&self, uid: &str) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.delete_event(uid));
        }
        let mut events = self.calendar_events();
        if let Some(stored) = events.iter().find(|s| s.event.uid == uid)
            && let Some(server) = self.calendar_server()
        {
            self.calendar_delete(&server, stored)?;
        }
        events.retain(|s| s.event.uid != uid);
        self.save_events(events)
    }

    /// How you answered this invitation here, if you did.
    pub fn invitation_answer(&self, uid: &str) -> Option<PartStat> {
        let answers: HashMap<String, PartStat> = self
            .store
            .preference(ANSWERS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        answers.get(uid).copied()
    }
    fn note_answer(&self, uid: &str, status: PartStat) -> Result<()> {
        let mut answers: HashMap<String, PartStat> = self
            .store
            .preference(ANSWERS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        answers.insert(uid.to_owned(), status);
        self.store
            .set_preference(ANSWERS, &serde_json::to_string(&answers)?)
    }

    /// The invitation (or cancellation, or answer) in a message, if it has one.
    pub fn invitation(&self, a: &Account, m: &Mail) -> Result<Option<ical::Calendar>> {
        let raw = self.original(a, m)?;
        let plain = {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            self.plaintext(a, &raw)?.1
        };
        Ok(ical::from_message(&plain))
    }

    /// Answers an invitation: tells its organizer, from the address it came
    /// to, and puts it in (or takes it out of) the calendar where one is set up.
    /// Returns a note when the calendar couldn't be changed.
    pub fn answer_invitation(
        &self,
        a: &Account,
        m: &Mail,
        status: PartStat,
    ) -> Result<Option<String>> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        let event = cal
            .events
            .first()
            .cloned()
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        ensure!(
            !event.cancelled,
            t!("manage-calendar-this-event-was-cancelled")
        );
        // Answer as whichever of the account's addresses was invited.
        let mut mine: Vec<(String, String)> = vec![(a.email.clone(), a.name.clone())];
        mine.extend(
            a.identities
                .iter()
                .map(|i| (i.email.trim().to_owned(), i.name.clone())),
        );
        let (me, my_name) = mine
            .iter()
            .find(|(e, _)| {
                event
                    .attendees
                    .iter()
                    .any(|at| at.email.eq_ignore_ascii_case(e))
            })
            .cloned()
            .unwrap_or_else(|| mine[0].clone());
        let organizer = event
            .organizer
            .clone()
            .with_context(|| t!("manage-calendar-this-invitation-doesnt-say-who-to"))?;
        // An invitation for an event already in the calendar (not Exchange's,
        // which answers only its own copy) can be written by anyone who
        // knows its UID. It must name the organizer the calendar has. Its
        // version is taken only when the organizer's domain vouches for the
        // mail; otherwise your answer goes on the calendar's own copy, and
        // declining, which takes the event out, is refused.
        let mut take_version = true;
        if self.calendar_server().is_some()
            && !self.exchange_calendar(&event.uid)?
            && let Some(kept) = self.stored_event(&event.uid)
        {
            match &kept.event.organizer {
                Some(o) => ensure!(
                    organizer_may_change(&kept.event, &event, &m.from),
                    t!("manage-calendar-this-update-is-from-not-the", from = m.from, email = o.email)
                ),
                None => bail!(t!("manage-calendar-this-update-would-replace-your-own")),
            }
            let verdict = self.dmarc_verdict(a, m)?;
            take_version = verdict.as_deref() == Some("pass");
            if status == PartStat::Declined {
                vouched_for(m, verdict)?;
            }
        }
        let reply = ical::reply(&event, &me, &my_name, status)?;
        let raw = imip_message(
            &me,
            &my_name,
            &organizer.email,
            &event,
            status,
            &reply,
            &m.message_id,
        );
        self.send_raw(a, &me, &organizer.email, &raw)?;
        self.note_answer(&event.uid, status)?;
        // The calendar, where there is one.
        if self.calendar_server().is_none() {
            return Ok(None);
        }
        Ok(self.file_answer(&event, &me, status, take_version).err().map(|e| {
            t!(
                "manage-calendar-the-answer-was-sent-but-the",
                error = format!("{:#}", e)
            )
        }))
    }
    /// Puts your answer to an invitation in the calendar: declined, it goes
    /// (only that occurrence, where the invitation is for one: RFC 5546
    /// §3.2.3, a REPLY with a RECURRENCE-ID answers just it); otherwise it
    /// is saved with your answer in it: the invitation's version of it when
    /// `take_version`, else the calendar's own copy, where it has one.
    pub(super) fn file_answer(&self, event: &Event, me: &str, status: PartStat, take_version: bool) -> Result<()> {
        if let Some(id) = self.route_event(&event.uid) {
            return self.in_calendar_source(&id, || self.file_answer(event, me, status, take_version));
        }
        // Google put the invitation in the calendar itself, as Exchange does.
        if let Some(server) = self.calendar_server()
            && let Some(a) = self.calendar_google(&server)?
        {
            let Some(stored) = self.stored_event(&event.uid).filter(|s| s.href.starts_with("google:")) else {
                return Ok(());
            };
            let calendar = stored.calendar.strip_prefix("google:").unwrap_or_default().to_owned();
            let id = match event.recurrence_id {
                Some(at) => self
                    .with_google(&a, |s| s.occurrence_at(&calendar, &stored.href["google:".len()..], at))?
                    .unwrap_or_else(|| stored.href["google:".len()..].to_owned()),
                None => stored.href["google:".len()..].to_owned(),
            };
            self.with_google(&a, |s| s.respond_event(&calendar, &id, status))?;
            return self.sync_calendar().map(drop);
        }
        // Exchange put the invitation in the calendar itself: it is
        // answered there (the reply already went by mail).
        if let Some(server) = self.calendar_server()
            && let Some(a) = self.calendar_ews(&server)?
        {
            let Some(id) = self.stored_event(&event.uid).and_then(|s| s.href.strip_prefix("ews:").map(str::to_owned)) else {
                return Ok(());
            };
            let response = match status {
                PartStat::Accepted => "AcceptItem",
                PartStat::Tentative => "TentativelyAcceptItem",
                PartStat::Declined => "DeclineItem",
                _ => return Ok(()),
            };
            self.with_ews_locked(&a, |s| s.respond_event(&id, response))?;
            return self.sync_calendar().map(drop);
        }
        if let Some(server) = self.calendar_server()
            && let Some(a) = self.calendar_graph(&server)?
        {
            let Some(id) = self.stored_event(&event.uid).and_then(|s| s.href.strip_prefix("graph:").map(str::to_owned)) else {
                return Ok(());
            };
            self.with_graph_calendars(&a, |s| s.respond_event(&id, status))?;
            return self.sync_calendar().map(drop);
        }
        match (status, event.recurrence_id) {
            (PartStat::Declined, Some(at)) => self.cancel_occurrence(&event.uid, at),
            (PartStat::Declined, None) => self.delete_event(&event.uid),
            _ => {
                let mut kept = match self.stored_event(&event.uid).filter(|_| !take_version) {
                    None => event.clone(),
                    Some(stored) => match event.recurrence_id {
                        None => stored.event,
                        Some(at) => ical::occurrences(&stored.event, &stored.overrides, at, at + 1)
                            .into_iter()
                            .find(|o| o.recurrence_id == Some(at))
                            .with_context(|| t!("manage-calendar-the-event-this-answers-isnt-in"))?,
                    },
                };
                for at in &mut kept.attendees {
                    if at.email.eq_ignore_ascii_case(me) {
                        at.status = Some(status);
                    }
                }
                // The event as it came is what's saved: the answer goes in
                // it too, or it would be lost on the server.
                with_partstat(&mut kept, me, status);
                self.save_invited(kept)
            }
        }
    }
    /// Saves an invitation's event as the organizer sent it.
    fn save_invited(&self, event: Event) -> Result<()> {
        if let Some(id) = self.route_event(&event.uid) {
            return self.in_calendar_source(&id, || self.save_invited(event));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let mut events = self.calendar_events();
        let mut stored = events
            .iter()
            .find(|s| s.event.uid == event.uid)
            .cloned()
            .unwrap_or_default();
        let target = if stored.calendar.is_empty() {
            self.default_calendar()
                .with_context(|| t!("manage-calendar-none-of-your-calendars-can-take-2"))?
                .href
        } else {
            stored.calendar.clone()
        };
        match event.recurrence_id {
            // One occurrence of a series already here (RFC 5545 §3.8.4.4):
            // it becomes that occurrence's own, the series stays.
            Some(at) if !stored.event.recurrence.is_empty() || !stored.event.rdates.is_empty() => {
                stored.overrides.retain(|o| o.recurrence_id != Some(at));
                stored.overrides.push(Event { source: String::new(), ..event });
            }
            _ => stored.event = event,
        }
        let saved = self.calendar_save(&server, &target, &stored)?;
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved);
        self.save_events(events)
    }
    /// The calendar's copy of the event an invitation message is about, if
    /// it's in the calendar.
    pub fn stored_event(&self, uid: &str) -> Option<Stored> {
        // From the parsed events, without copying them all for one.
        self.cached_events().iter().find(|s| s.event.uid == uid).cloned()
    }

    /// A guest's answer (REPLY) to an event you organize: their answer is
    /// noted on the calendar's copy. Returns who answered what.
    pub fn apply_reply(&self, a: &Account, m: &Mail) -> Result<String> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-has-no-answer-to"))?;
        ensure!(
            cal.method == "REPLY",
            t!("manage-calendar-this-message-isnt-an-answer-to")
        );
        let answer = cal
            .events
            .first()
            .with_context(|| t!("manage-calendar-this-message-has-no-answer-to"))?;
        let guest = answer
            .attendees
            .first()
            .with_context(|| t!("manage-calendar-the-answer-doesnt-say-who-its"))?;
        let status = guest.status.unwrap_or(PartStat::NeedsAction);
        // The sender must be the guest answering: anyone can write a REPLY.
        ensure!(
            m.from.eq_ignore_ascii_case(&guest.email),
            t!(
                "manage-calendar-this-answer-is-for-but-came",
                email = guest.email,
                from = m.from
            )
        );
        let mut stored = self
            .stored_event(&answer.uid)
            .with_context(|| t!("manage-calendar-the-event-this-answers-isnt-in"))?;
        self.require_vouched(a, m)?;
        let target = match answer.recurrence_id {
            Some(at) => stored
                .overrides
                .iter_mut()
                .find(|o| o.recurrence_id == Some(at))
                .map(|o| &mut o.attendees),
            None => None,
        };
        let attendees = match target {
            Some(t) => t,
            None => &mut stored.event.attendees,
        };
        match attendees
            .iter_mut()
            .find(|g| g.email.eq_ignore_ascii_case(&guest.email))
        {
            Some(g) => g.status = Some(status),
            None => bail!(t!(
                "manage-calendar-isnt-a-guest-of-this-event",
                email = guest.email
            )),
        }
        let changed = match answer.recurrence_id {
            Some(at) => stored.overrides.iter_mut().find(|o| o.recurrence_id == Some(at)),
            None => None,
        };
        match changed {
            Some(o) => with_partstat(o, &guest.email, status),
            None => with_partstat(&mut stored.event, &guest.email, status),
        }
        self.store_updated(stored)?;
        let who = if guest.name.is_empty() {
            &guest.email
        } else {
            &guest.name
        };
        Ok(format!("{who}: {}", status.verb().to_lowercase()))
    }

    /// Accepts a guest's proposed time (COUNTER) for an event you organize:
    /// the event moves, and every guest is told (a new REQUEST).
    pub fn accept_counter(&self, a: &Account, m: &Mail) -> Result<usize> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-proposes-no-time"))?;
        ensure!(
            cal.method == "COUNTER",
            t!("manage-calendar-this-message-proposes-no-time")
        );
        let proposed = cal
            .events
            .first()
            .with_context(|| t!("manage-calendar-this-message-proposes-no-time"))?;
        let guest = proposed
            .attendees
            .first()
            .map(|g| g.email.clone())
            .unwrap_or_default();
        ensure!(
            guest.is_empty() || m.from.eq_ignore_ascii_case(&guest),
            t!(
                "manage-calendar-this-proposal-is-for-but-came",
                guest = guest,
                from = m.from
            )
        );
        let mut stored = self
            .stored_event(&proposed.uid)
            .with_context(|| t!("manage-calendar-the-event-this-proposal-is-for"))?;
        // Only a guest proposes a time (RFC 5546 §3.2.7): accepting one
        // moves the event and tells everyone invited.
        let invited = |attendees: &[ical::Attendee]| attendees.iter().any(|a| a.email.eq_ignore_ascii_case(&guest));
        ensure!(
            !guest.is_empty()
                && (invited(&stored.event.attendees) || stored.overrides.iter().any(|o| invited(&o.attendees))),
            t!("manage-calendar-this-proposal-isnt-from-a-guest", from = m.from)
        );
        self.require_vouched(a, m)?;
        let length = proposed.end - proposed.start;
        match proposed.recurrence_id {
            // One repeat moves.
            Some(at) if !stored.event.recurrence.is_empty() => {
                let mut changed = ical::occurrences(&stored.event, &stored.overrides, at, at + 1)
                    .into_iter()
                    .find(|o| o.recurrence_id == Some(at))
                    .unwrap_or_else(|| stored.event.clone());
                changed.start = proposed.start;
                changed.end = proposed.start + length;
                changed.recurrence = String::new();
                changed.exdates.clear();
                changed.recurrence_id = Some(at);
                changed.source.clear();
                stored.overrides.retain(|o| o.recurrence_id != Some(at));
                stored.overrides.push(changed);
            }
            _ => {
                stored.event.start = proposed.start;
                stored.event.end = proposed.start + length;
            }
        }
        stored.event.sequence += 1;
        stored.event.source.clear();
        // Everyone answers again for the new time; the guest who proposed it accepts it.
        for g in &mut stored.event.attendees {
            g.status = Some(if g.email.eq_ignore_ascii_case(&guest) {
                PartStat::Accepted
            } else {
                PartStat::NeedsAction
            });
        }
        let uid = stored.event.uid.clone();
        self.store_updated(stored)?;
        self.send_invitations(a, &uid)
    }

    /// Turns down a guest's proposed time (DECLINECOUNTER); the event stays.
    pub fn decline_counter(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-proposes-no-time"))?;
        ensure!(
            cal.method == "COUNTER",
            t!("manage-calendar-this-message-proposes-no-time")
        );
        let proposed = cal
            .events
            .first()
            .with_context(|| t!("manage-calendar-this-message-proposes-no-time"))?;
        let guest = proposed
            .attendees
            .first()
            .map(|g| g.email.clone())
            .with_context(|| t!("manage-calendar-the-proposal-doesnt-say-who-its"))?;
        // As accepting does: the answer goes to the guest the proposal
        // names, who must be whoever sent it, or anyone's address could be
        // sent a reply.
        ensure!(
            m.from.eq_ignore_ascii_case(&guest),
            t!(
                "manage-calendar-this-proposal-is-for-but-came",
                guest = guest,
                from = m.from
            )
        );
        let stored = self.stored_event(&proposed.uid);
        let event = stored.as_ref().map(|s| &s.event).unwrap_or(proposed);
        let text = format!(
            "The time you proposed for {} wasn't taken; it stays as it was.\r\n",
            event.summary
        );
        let raw = imip(
            &a.email,
            &a.name,
            std::slice::from_ref(&guest),
            &format!("Declined: {}", event.summary),
            &text,
            "DECLINECOUNTER",
            &ical::decline_counter(event, &guest),
            &m.message_id,
        );
        self.send_raw(a, &a.email, &guest, &raw)
    }

    /// As a guest, proposes another time for an invitation (COUNTER).
    pub fn propose_time(
        &self,
        a: &Account,
        m: &Mail,
        start: i64,
        end: i64,
        note: &str,
    ) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        let event = cal
            .events
            .first()
            .cloned()
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        ensure!(
            !event.cancelled,
            t!("manage-calendar-this-event-was-cancelled")
        );
        let (me, my_name) = self.invited_as(a, &event);
        let organizer = event
            .organizer
            .clone()
            .with_context(|| t!("manage-calendar-this-invitation-doesnt-say-who-to"))?;
        let counter = ical::counter(&event, &me, &my_name, start, end, note)?;
        let when = ical::describe_when(&Event {
            start,
            end,
            ..event.clone()
        });
        let proposes = crate::i18n::in_outgoing(|| {
            t!(
                "outgoing-proposes",
                name = if my_name.is_empty() { &me } else { &my_name },
                event = event.summary,
            )
        });
        let text = format!("{proposes}\r\n{when}\r\n\r\n{note}\r\n");
        let raw = imip(
            &me,
            &my_name,
            std::slice::from_ref(&organizer.email),
            &format!("New time proposed: {}", event.summary),
            &text,
            "COUNTER",
            &counter,
            &m.message_id,
        );
        self.send_raw(a, &me, &organizer.email, &raw)?;
        self.note_answer(&event.uid, PartStat::Tentative)
    }

    /// An updated invitation (a REQUEST with a higher SEQUENCE) for an
    /// event in your calendar: whether its time, place or title changed.
    pub fn invitation_changed(&self, event: &Event) -> Option<Stored> {
        let stored = self.stored_event(&event.uid)?;
        let differs = event.sequence > stored.event.sequence
            || event.start != stored.event.start
            || event.end != stored.event.end
            || event.location != stored.event.location
            || event.summary != stored.event.summary;
        differs.then_some(stored)
    }

    /// Puts an updated invitation's version in the calendar, keeping your answer.
    pub fn take_update(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        let mut event = cal
            .events
            .first()
            .cloned()
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        let (me, _) = self.invited_as(a, &event);
        // Only from the organizer, and not older than what's kept (RFC
        // 5546 §2.1.5: SEQUENCE orders revisions).
        if let Some(organizer) = &event.organizer {
            ensure!(
                m.from.eq_ignore_ascii_case(&organizer.email),
                t!(
                    "manage-calendar-this-update-is-from-not-the",
                    from = m.from,
                    email = organizer.email
                )
            );
        }
        if let Some(kept) = self.stored_event(&event.uid) {
            ensure!(
                event.sequence >= kept.event.sequence,
                t!("manage-calendar-this-update-is-older-than-the")
            );
            // The organizer is the one the calendar has, not whoever the
            // update names: anyone can write an update naming themselves,
            // or no one, for an event whose UID they know; and the mail
            // must be theirs, as their domain vouches.
            self.require_organizer(a, m, &kept, &event)?;
        }
        if let Some(answer) = self.invitation_answer(&event.uid) {
            for g in &mut event.attendees {
                if g.email.eq_ignore_ascii_case(&me) {
                    g.status = Some(answer);
                }
            }
            with_partstat(&mut event, &me, answer);
        }
        self.save_invited(event)
    }

    /// Which of the account's addresses an event invites, and its name.
    fn invited_as(&self, a: &Account, event: &Event) -> (String, String) {
        let mut mine: Vec<(String, String)> = vec![(a.email.clone(), a.name.clone())];
        mine.extend(
            a.identities
                .iter()
                .map(|i| (i.email.trim().to_owned(), i.name.clone())),
        );
        mine.iter()
            .find(|(e, _)| {
                event
                    .attendees
                    .iter()
                    .any(|at| at.email.eq_ignore_ascii_case(e))
            })
            .cloned()
            .unwrap_or_else(|| mine[0].clone())
    }

    /// Takes one occurrence out of a repeating event (an EXDATE), keeping
    /// the rest of the series.
    fn cancel_occurrence(&self, uid: &str, at: i64) -> Result<()> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.cancel_occurrence(uid, at));
        }
        let Some(mut stored) = self.stored_event(uid) else {
            return Ok(());
        };
        if stored.event.recurrence.is_empty() {
            return self.delete_event(uid);
        }
        stored.overrides.retain(|o| o.recurrence_id != Some(at));
        if !stored.event.exdates.contains(&at) {
            stored.event.exdates.push(at);
        }
        // The source has no EXDATE for it: written from the fields.
        stored.event.source.clear();
        self.store_updated(stored)
    }
    /// A guest asks for the event again (METHOD:REFRESH, RFC 5546 §3.2.6):
    /// the organizer sends them the current version.
    pub fn answer_refresh(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-asks-for-no-event"))?;
        ensure!(
            cal.method == "REFRESH",
            t!("manage-calendar-this-message-asks-for-no-event")
        );
        let asked = cal
            .events
            .first()
            .with_context(|| t!("manage-calendar-this-message-asks-for-no-event"))?;
        let stored = self
            .stored_event(&asked.uid)
            .with_context(|| t!("manage-calendar-the-event-they-ask-about-isnt"))?;
        // Only to a guest of it, from the address they're invited at.
        let guest = stored
            .event
            .attendees
            .iter()
            .find(|g| g.email.eq_ignore_ascii_case(&m.from))
            .with_context(|| {
                t!(
                    "manage-calendar-isnt-a-guest-of-this-event-2",
                    from = m.from
                )
            })?
            .email
            .clone();
        // The event goes to whoever the From names: only to a guest who
        // really asked.
        self.require_vouched(a, m)?;
        let me = stored
            .event
            .organizer
            .as_ref()
            .map_or(a.email.clone(), |o| o.email.clone());
        let calendar = ical::request(&stored.event, &stored.overrides, false);
        let raw = imip(
            &me,
            &a.name,
            std::slice::from_ref(&guest),
            &crate::i18n::in_outgoing(|| t!("outgoing-invitation", event = stored.event.summary)),
            &format!(
                "{}\r\n\r\n{}\r\n",
                crate::i18n::in_outgoing(|| t!("outgoing-the-event-as-it-is-now")),
                ical::describe_when(&stored.event)
            ),
            "REQUEST",
            &calendar,
            &m.message_id,
        );
        self.send_raw(a, &me, &guest, &raw)
    }
    /// The organizer added occurrences to a repeating event (METHOD:ADD,
    /// RFC 5546 §3.2.4): each becomes an extra time (RDATE), with its own
    /// details where they differ.
    pub fn take_added(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-adds-nothing"))?;
        ensure!(
            cal.method == "ADD",
            t!("manage-calendar-this-message-adds-nothing")
        );
        let first = cal
            .events
            .first()
            .with_context(|| t!("manage-calendar-this-message-adds-nothing"))?;
        let mut stored = self
            .stored_event(&first.uid)
            .with_context(|| t!("manage-calendar-the-event-this-adds-to-isnt"))?;
        // From the organizer the calendar has, not whoever the mail names.
        ensure!(
            organizer_may_change(&stored.event, first, &m.from),
            t!("manage-calendar-this-addition-isnt-from-the-events")
        );
        self.require_vouched(a, m)?;
        for added in &cal.events {
            if !stored.event.rdates.contains(&added.start) {
                stored.event.rdates.push(added.start);
            }
            // Different details: kept as that occurrence's own.
            if added.summary != stored.event.summary
                || added.location != stored.event.location
                || added.end - added.start != stored.event.end - stored.event.start
            {
                let mut own = added.clone();
                own.recurrence.clear();
                own.recurrence_id = Some(added.start);
                own.source.clear();
                stored.overrides.retain(|o| o.recurrence_id != Some(added.start));
                stored.overrides.push(own);
            }
        }
        stored.event.sequence = stored.event.sequence.max(first.sequence);
        stored.event.source.clear();
        self.store_updated(stored)
    }
    /// Saves a changed event to the server and here.
    fn store_updated(&self, stored: Stored) -> Result<()> {
        if let Some(id) = self.route_event(&stored.event.uid) {
            return self.in_calendar_source(&id, || self.store_updated(stored));
        }
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let calendar = stored.calendar.clone();
        let saved = self.calendar_save(&server, &calendar, &stored)?;
        let mut events = self.calendar_events();
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved);
        self.save_events(events)
    }

    /// Takes a cancelled event out of the calendar.
    pub fn remove_cancelled(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .with_context(|| t!("manage-calendar-this-message-has-no-invitation"))?;
        for e in cal.events.iter().filter(|e| e.cancelled) {
            // Only the organizer cancels (RFC 5546 §3.2.5): the one the
            // calendar has, whether or not the cancellation names one.
            if let Some(kept) = self.stored_event(&e.uid) {
                ensure!(
                    organizer_may_change(&kept.event, e, &m.from),
                    t!("manage-calendar-this-cancellation-isnt-from-the-events")
                );
                self.require_vouched(a, m)?;
            }
            // One occurrence cancelled: only it goes, not the series.
            if let Some(at) = e.recurrence_id {
                self.cancel_occurrence(&e.uid, at)?;
            } else {
                self.delete_event(&e.uid)?;
            }
        }
        Ok(())
    }

    /// Invites an event's guests (or tells them it changed): one iMIP
    /// REQUEST to all of them, from `a` as the organizer. Returns how many
    /// were told.
    pub fn send_invitations(&self, a: &Account, uid: &str) -> Result<usize> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.send_invitations(a, uid));
        }
        if self.server_schedules() {
            // Saving it told the server, which invites the guests.
            return Ok(self
                .stored_event(uid)
                .map_or(0, |s| s.event.attendees.len()));
        }
        let stored = self
            .calendar_events()
            .into_iter()
            .find(|s| s.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in-2"))?;
        self.invite(a, &stored.event, &stored.overrides, false)
    }
    /// Sends an event's invitations again, though nothing changed: by mail,
    /// or on a scheduling server by saving it with SCHEDULE-FORCE-SEND
    /// (RFC 6638 §7.2), since a server sends only on a change that
    /// matters. For guests whose invitation got lost (SCHEDULE-STATUS 3.x
    /// or 5.x, `Attendee::delivery_failed`).
    pub fn resend_invitations(&self, a: &Account, uid: &str) -> Result<usize> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.resend_invitations(a, uid));
        }
        if !self.server_schedules()
            || self
                .calendar_jmap(
                    &self
                        .calendar_server()
                        .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?,
                )?
                .is_some()
        {
            return self.send_invitations(a, uid);
        }
        let mut stored = self
            .stored_event(uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in-2"))?;
        ensure!(
            self.organized_here(&stored.event),
            t!("manage-calendar-only-the-organizer-sends-the-invitations")
        );
        for g in std::iter::once(&mut stored.event)
            .chain(&mut stored.overrides)
            .flat_map(|e| &mut e.attendees)
        {
            g.force_send = "REQUEST".into();
        }
        let count = stored.event.attendees.len();
        // Written from its fields, so the parameter is in what's sent.
        stored.event.source.clear();
        let server = self
            .calendar_server()
            .with_context(|| t!("manage-calendar-no-calendar-server-is-set-up"))?;
        let calendar = stored.calendar.clone();
        let mut saved = self.calendar_save(&server, &calendar, &stored)?;
        // Once: a later save mustn't send them all again.
        for g in std::iter::once(&mut saved.event).chain(&mut saved.overrides).flat_map(|e| &mut e.attendees) {
            g.force_send.clear();
        }
        let mut events = self.calendar_events();
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved);
        self.save_events(events)?;
        Ok(count)
    }
    /// Tells an event's guests it's cancelled (before deleting it).
    pub fn cancel_invitations(&self, a: &Account, uid: &str) -> Result<usize> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.cancel_invitations(a, uid));
        }
        if self.server_schedules() {
            // Deleting it tells the server, which tells the guests.
            return Ok(self
                .stored_event(uid)
                .map_or(0, |s| s.event.attendees.len()));
        }
        let stored = self
            .calendar_events()
            .into_iter()
            .find(|s| s.event.uid == uid)
            .with_context(|| t!("manage-calendar-this-event-is-no-longer-in-2"))?;
        self.invite(a, &stored.event, &[], true)
    }
    fn invite(
        &self,
        a: &Account,
        event: &Event,
        overrides: &[Event],
        cancel: bool,
    ) -> Result<usize> {
        let mine = |e: &str| {
            e.eq_ignore_ascii_case(&a.email)
                || a.identities
                    .iter()
                    .any(|i| i.email.trim().eq_ignore_ascii_case(e))
        };
        ensure!(
            event.organizer.as_ref().is_none_or(|o| mine(&o.email)),
            t!("manage-calendar-only-the-organizer-can-send-this")
        );
        let guests: Vec<String> = event
            .attendees
            .iter()
            .map(|g| g.email.trim().to_owned())
            .filter(|g| !g.is_empty() && !mine(g))
            .collect();
        ensure!(
            !guests.is_empty(),
            t!("manage-calendar-this-event-has-no-guests-to")
        );
        let me = event
            .organizer
            .as_ref()
            .map_or(a.email.clone(), |o| o.email.clone());
        let mut event = event.clone();
        if event.organizer.is_none() {
            event.organizer = Some(ical::Attendee {
                email: me.clone(),
                name: a.name.clone(),
                status: None,
                ..Default::default()
            });
        }
        let calendar = ical::request(&event, overrides, cancel);
        let when = crate::sync::ical::describe_when(&event);
        let (subject, text) = crate::i18n::in_outgoing(|| {
            if cancel {
                (
                    t!("outgoing-cancelled", event = event.summary),
                    format!(
                        "{}\r\n\r\n{when}\r\n",
                        t!("outgoing-was-cancelled", event = event.summary)
                    ),
                )
            } else {
                let place = if event.location.is_empty() {
                    String::new()
                } else {
                    format!("\r\n{}", event.location)
                };
                (
                    t!("outgoing-invitation", event = event.summary),
                    format!(
                        "{}\r\n\r\n{when}{place}\r\n\r\n{}\r\n",
                        t!(
                            "outgoing-invites-you",
                            name = a.label(),
                            event = event.summary
                        ),
                        event.description
                    ),
                )
            }
        });
        let raw = imip(
            &me,
            &a.name,
            &guests,
            &subject,
            &text,
            if cancel { "CANCEL" } else { "REQUEST" },
            &calendar,
            "",
        );
        for guest in &guests {
            self.send_raw(a, &me, guest, &raw)?;
        }
        Ok(guests.len())
    }

    /// Sends a finished message from `from` to `to` through the account's
    /// own way of sending.
    pub(super) fn send_raw(&self, a: &Account, from: &str, to: &str, raw: &[u8]) -> Result<()> {
        // `to` comes from an invitation (its organizer): checked for every
        // provider, as `deliver_raw` checks SMTP's.
        ensure!(
            !to.contains(|c: char| c.is_control() || c.is_whitespace() || c == '<' || c == '>')
                && to.contains('@'),
            t!("manage-calendar-isnt-a-valid-address", to = to)
        );
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        match a.provider {
            Provider::Proton => self.send_proton(a, from, &[to.to_owned()], raw),
            Provider::Jmap => self
                .send_jmap(a, from, &[to.to_owned()], raw, &Default::default())
                .map(|_| ()),
            _ => {
                let s = self.credentials(a)?;
                let secret = if a.oauth { s.access_token } else { s.password };
                crate::sync::smtp::deliver_raw(
                    a,
                    from,
                    &[to.to_owned()],
                    raw,
                    &crate::sync::smtp::Endpoint::of(a),
                    &secret,
                    &self.tls_connector()?,
                )
            }
        }
    }
}

/// Each source's count, added up (`each_calendar_source`, and contacts'
/// alike). A source that failed is said by name, once the others were
/// read; none set up at all is an error too.
pub(super) fn together(results: Vec<(String, Result<usize>)>, label: impl Fn(&str) -> String) -> Result<usize> {
    ensure!(!results.is_empty(), t!("manage-calendar-no-calendar-server-is-set-up"));
    let mut total = 0;
    let mut failed = None;
    for (id, result) in results {
        match result {
            Ok(n) => total += n,
            Err(e) if failed.is_none() => failed = Some(e.context(t!("manage-pim-sources-couldnt-read", source = label(&id)))),
            Err(_) => {}
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(total),
    }
}

/// Notes `email`'s answer in the event's text as it came, so saving it
/// keeps the answer and everything Picacho doesn't read alike. Without
/// that text (or the attendee in it), the event is written from its fields.
fn with_partstat(event: &mut Event, email: &str, status: PartStat) {
    if event.source.is_empty() {
        return;
    }
    match crate::sync::ical::set_partstat_in_source(&event.source, email, status) {
        Some(source) => event.source = source,
        None => event.source.clear(),
    }
}

/// This computer's time zone, as an IANA name ("Europe/Berlin"): `TZ`
/// when set, else what the system says (macOS, Windows, Linux, Android
/// and iOS each keep it their own way).
/// What of a JMAP server's task lists decides which tasks are shown and
/// their reminders (`TASK_STATE`): the lists shown, and their default
/// alerts.
fn task_lists_fingerprint(lists: &[crate::sync::jscal::JmapTaskList]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for l in lists.iter().filter(|l| l.shown) {
        hash.update(format!("{}\n{}\n{}\n", l.id, l.alerts_with_time, l.alerts_without_time).as_bytes());
    }
    hash.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

pub fn local_zone() -> Option<String> {
    let valid = |z: &str| z.parse::<chrono_tz::Tz>().is_ok().then(|| z.to_owned());
    if let Ok(tz) = std::env::var("TZ")
        && let Some(z) = valid(tz.trim_start_matches(':'))
    {
        return Some(z);
    }
    iana_time_zone::get_timezone().ok().and_then(|z| valid(&z))
}

/// A header value, encoded when it isn't plain ASCII.
/// `text` without control characters (CR, LF), so a value from someone
/// else can't start a header of its own.
fn one_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn header_text(text: &str) -> String {
    crate::sync::mime::encoded_words(text)
}

/// The iMIP message carrying a REPLY: a line of text for people, and the
/// calendar part for their calendar (RFC 6047 §2.4).
fn imip_message(
    me: &str,
    my_name: &str,
    organizer: &str,
    event: &Event,
    status: PartStat,
    reply: &str,
    in_reply_to: &str,
) -> Vec<u8> {
    let subject = format!("{}: {}", status.verb(), event.summary);
    let who = if my_name.is_empty() { me } else { my_name };
    let text = format!(
        "{who} has {} the invitation to {}.\r\n",
        match status {
            PartStat::Accepted => "accepted",
            PartStat::Tentative => "tentatively accepted",
            PartStat::Declined => "declined",
            PartStat::Delegated => "delegated",
            PartStat::NeedsAction | PartStat::Completed | PartStat::InProcess => "answered",
        },
        event.summary
    );
    imip(
        me,
        my_name,
        &[organizer.to_owned()],
        &subject,
        &text,
        "REPLY",
        reply,
        in_reply_to,
    )
}

/// An iMIP message: `text` for people, `calendar` (with METHOD `method`)
/// for their calendars, from `me` to `to`.
#[allow(clippy::too_many_arguments)]
fn imip(
    me: &str,
    my_name: &str,
    to: &[String],
    subject: &str,
    text: &str,
    method: &str,
    calendar: &str,
    in_reply_to: &str,
) -> Vec<u8> {
    use base64::Engine as _;
    let boundary = format!("imip-{}", uuid::Uuid::new_v4().simple());
    let domain = me.rsplit('@').next().unwrap_or("localhost");
    let reply = calendar;
    let mut out = String::new();
    if my_name.is_empty() {
        out.push_str(&format!("From: <{me}>\r\n"));
    } else {
        out.push_str(&format!("From: {} <{me}>\r\n", crate::sync::mime::display_name(my_name)));
    }
    let to_line = to
        .iter()
        .map(|t| format!("<{}>", one_line(t).replace(['<', '>'], "")))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("To: {to_line}\r\n"));
    out.push_str(&format!("Subject: {}\r\n", header_text(subject)));
    out.push_str(&format!("Date: {}\r\n", chrono::Local::now().to_rfc2822()));
    out.push_str(&format!(
        "Message-ID: <{}@{domain}>\r\n",
        uuid::Uuid::new_v4()
    ));
    // The invitation's Message-ID is the sender's to write: one line.
    let original = one_line(in_reply_to);
    let original = original.trim();
    if !original.is_empty() {
        out.push_str(&format!(
            "In-Reply-To: {original}\r\nReferences: {original}\r\n"
        ));
    }
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str(&format!(
        "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n"
    ));
    out.push_str(&format!(
        "--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        wrap(&base64::engine::general_purpose::STANDARD.encode(text))
    ));
    out.push_str(&format!(
        "--{boundary}\r\nContent-Type: text/calendar; charset=utf-8; method={method}\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--{boundary}--\r\n",
        wrap(&base64::engine::general_purpose::STANDARD.encode(reply))
    ));
    out.into_bytes()
}

fn wrap(b64: &str) -> String {
    b64.as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n")
}

impl Backend {
    /// What DMARC (RFC 7489) says of `m`'s From address: `pass` when its
    /// domain vouches for the message, else `fail`, `none` (the domain asks
    /// nothing)… `None` when it couldn't be checked. Checked now: the
    /// domain's records are looked up in DNS.
    fn dmarc_verdict(&self, a: &Account, m: &Mail) -> Result<Option<String>> {
        #[cfg(test)]
        {
            let _ = a;
            let pass = self.test_dmarc_pass.iter().any(|s| s.eq_ignore_ascii_case(&m.from));
            Ok(Some(if pass { "pass" } else { "none" }.into()))
        }
        #[cfg(not(test))]
        {
            let raw = self.original(a, m)?;
            let mut details = crate::sync::mime::details(&raw, false);
            crate::sync::auth::check(&raw, &mut details);
            Ok(details.dmarc)
        }
    }

    /// Mail that changes what's in the calendar, or has Picacho send it on,
    /// must be from who its From says: anyone can write an organizer's or a
    /// guest's address there. Only mail its domain vouches for (DMARC pass)
    /// does; the rest is refused, saying why.
    fn require_vouched(&self, a: &Account, m: &Mail) -> Result<()> {
        vouched_for(m, self.dmarc_verdict(a, m)?)
    }

    /// Mail about `incoming`, an event already in the calendar as `kept`,
    /// may change it: from `kept`'s own organizer (`organizer_may_change`),
    /// vouched for by its domain (`require_vouched`).
    fn require_organizer(&self, a: &Account, m: &Mail, kept: &Stored, incoming: &Event) -> Result<()> {
        match &kept.event.organizer {
            Some(organizer) => ensure!(
                organizer_may_change(&kept.event, incoming, &m.from),
                t!(
                    "manage-calendar-this-update-is-from-not-the",
                    from = m.from,
                    email = organizer.email
                )
            ),
            None => bail!(t!("manage-calendar-this-update-would-replace-your-own")),
        }
        self.require_vouched(a, m)
    }

    /// Whether the calendar event `uid` is in (or would go to) is
    /// Exchange's (Web Services or Graph) or Google's: they file invitations
    /// themselves, and an answer only answers their own copy.
    fn exchange_calendar(&self, uid: &str) -> Result<bool> {
        if let Some(id) = self.route_event(uid) {
            return self.in_calendar_source(&id, || self.exchange_calendar(uid));
        }
        let Some(server) = self.calendar_server() else {
            return Ok(false);
        };
        Ok(self.calendar_ews(&server)?.is_some() || self.calendar_graph(&server)?.is_some() || !server.google_account.is_empty())
    }

    /// Adds an event an email carries (an invitation or a published event,
    /// added by hand). One already in the calendar is only replaced by mail
    /// from its organizer, vouched for (`require_organizer`).
    pub fn add_event_from_mail(&self, a: &Account, m: &Mail, event: Event) -> Result<Stored> {
        if let Some(kept) = self.stored_event(&event.uid) {
            ensure!(
                kept.event.organizer.is_some(),
                t!("manage-calendar-this-event-is-already-in-your-calendar")
            );
            self.require_organizer(a, m, &kept, &event)?;
        }
        self.save_event(event, None)
    }
}

/// `Ok` when `verdict`, DMARC's for `m`, is a pass; else why not.
fn vouched_for(m: &Mail, verdict: Option<String>) -> Result<()> {
    ensure!(
        verdict.as_deref() == Some("pass"),
        t!(
            "manage-calendar-sender-isnt-vouched-for",
            from = m.from,
            result = verdict.unwrap_or_else(|| "not checked".into())
        )
    );
    Ok(())
}

/// Whether a message from `from` may change or cancel `kept`, an event in
/// the calendar, as `incoming` says: only `kept`'s own organizer may (RFC
/// 5546 §2.1.5, §3.2.5), and `incoming` may name no other. An event with no
/// organizer, one of your own, isn't changed by mail at all.
fn organizer_may_change(kept: &Event, incoming: &Event, from: &str) -> bool {
    let Some(organizer) = &kept.organizer else {
        return false;
    };
    from.eq_ignore_ascii_case(&organizer.email)
        && incoming
            .organizer
            .as_ref()
            .is_none_or(|o| o.email.eq_ignore_ascii_case(&organizer.email))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheduling_server_sends_invitations_instead_of_mail() -> Result<()> {
        let path = std::env::temp_dir().join(format!("pinal-cal-{}", uuid::Uuid::new_v4()));
        let backend = Backend::new(std::sync::Arc::new(Store::open(&path)?));
        let mut a = Account::new(Provider::Imap);
        a.email = "qa@example.com".into();
        backend.store.save_account(&a)?;
        // No SMTP server: mailing an invitation would fail.
        a.smtp_host = "127.0.0.1".into();
        a.smtp_port = 9;
        let event = Stored {
            event: Event {
                uid: "lunch".into(),
                summary: "Lunch".into(),
                start: 1_800_000_000,
                end: 1_800_003_600,
                organizer: Some(ical::Attendee {
                    email: a.email.clone(),
                    ..Default::default()
                }),
                attendees: vec![ical::Attendee {
                    email: "bo@example.se".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            calendar: "https://dav.example/cal/".into(),
            href: "https://dav.example/cal/lunch.ics".into(),
            ..Default::default()
        };
        backend.save_events(vec![event.clone()])?;
        let server = |schedules: bool| {
            serde_json::to_string(&caldav::Server {
                url: "https://dav.example".into(),
                user: "qa".into(),
                server_schedules: schedules,
                ..Default::default()
            })
            .unwrap()
        };
        backend.store.set_preference(SERVER, &server(true))?;
        assert!(backend.server_schedules());
        assert_eq!(backend.send_invitations(&a, "lunch")?, 1);
        assert_eq!(backend.cancel_invitations(&a, "lunch")?, 1);
        // Your own events are the server's; others' invitations aren't.
        assert!(backend.organized_here(&event.event));
        let mut theirs = event.event.clone();
        theirs.organizer = Some(ical::Attendee {
            email: "ana@example.org".into(),
            ..Default::default()
        });
        assert!(!backend.organized_here(&theirs));
        // Off (the default): Picacho mails them, which needs the SMTP server.
        backend.store.set_preference(SERVER, &server(false))?;
        assert!(!backend.server_schedules());
        assert!(backend.send_invitations(&a, "lunch").is_err());
        // JMAP calendars can't be handed to the server.
        backend
            .store
            .set_preference(SERVER, r#"{"url":"x","user":"y","jmap_account":"z"}"#)?;
        assert!(backend.set_server_scheduling(true).is_err());
        drop(backend);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn replies_are_imip_messages_to_the_organizer() {
        let cal = ical::parse(
            "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:u1\nDTSTART:20261124T090000Z\nSUMMARY:Café\nORGANIZER;CN=Ana:mailto:ana@example.org\nATTENDEE:mailto:qa@example.com\nEND:VEVENT\nEND:VCALENDAR\n",
        );
        let e = &cal.events[0];
        let reply = ical::reply(e, "qa@example.com", "QA", PartStat::Declined).unwrap();
        let raw = imip_message(
            "qa@example.com",
            "QA",
            "ana@example.org",
            e,
            PartStat::Declined,
            &reply,
            "<x@y>",
        );
        let parsed = mailparse::parse_mail(&raw).unwrap();
        use mailparse::MailHeaderMap;
        assert_eq!(
            parsed.headers.get_first_value("To").unwrap(),
            "<ana@example.org>"
        );
        assert_eq!(
            parsed.headers.get_first_value("Subject").unwrap(),
            "Declined: Café"
        );
        assert_eq!(
            parsed.headers.get_first_value("In-Reply-To").unwrap(),
            "<x@y>"
        );
        let back = ical::from_message(&raw).unwrap();
        assert_eq!(back.method, "REPLY");
        assert_eq!(back.events[0].attendees[0].status, Some(PartStat::Declined));
    }

    #[test]
    fn invitations_go_to_every_guest() {
        let e = Event {
            uid: "party".into(),
            summary: "Party".into(),
            start: 1_800_000_000,
            end: 1_800_007_200,
            organizer: Some(ical::Attendee {
                email: "qa@example.com".into(),
                name: "QA".into(),
                status: None,
                ..Default::default()
            }),
            attendees: vec![
                ical::Attendee {
                    email: "ana@example.org".into(),
                    name: "Ana".into(),
                    status: None,
                    ..Default::default()
                },
                ical::Attendee {
                    email: "bo@example.se".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let cal = ical::request(&e, &[], false);
        let raw = imip(
            "qa@example.com",
            "QA",
            &["ana@example.org".into(), "bo@example.se".into()],
            "Invitation: Party",
            "x",
            "REQUEST",
            &cal,
            "",
        );
        let back = ical::from_message(&raw).unwrap();
        assert_eq!(back.method, "REQUEST");
        assert_eq!(back.events[0].attendees.len(), 2);
        assert_eq!(
            back.events[0].attendees[0].status,
            Some(PartStat::NeedsAction)
        );
        assert_eq!(
            back.events[0].organizer.as_ref().unwrap().email,
            "qa@example.com"
        );
        use mailparse::MailHeaderMap;
        let parsed = mailparse::parse_mail(&raw).unwrap();
        assert_eq!(
            parsed.headers.get_first_value("To").unwrap(),
            "<ana@example.org>, <bo@example.se>"
        );
        let cancel = ical::parse(&ical::request(&e, &[], true));
        assert_eq!(cancel.method, "CANCEL");
        assert!(cancel.events[0].cancelled);
    }

    #[test]
    fn events_need_a_server_and_a_title() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-cal-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        assert!(
            b.save_event(
                Event {
                    summary: "x".into(),
                    ..Default::default()
                },
                None
            )
            .is_err()
        );
        assert!(
            b.save_event(Event::default(), None)
                .unwrap_err()
                .to_string()
                .contains("title")
        );
        assert!(b.calendar_events().is_empty());
        assert_eq!(b.invitation_answer("u"), None);
        b.note_answer("u", PartStat::Tentative)?;
        assert_eq!(b.invitation_answer("u"), Some(PartStat::Tentative));
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn single_repeats_are_changed_and_left_out() -> Result<()> {
        use std::sync::{Arc as A, Mutex as M};
        // The server keeps what was last put.
        let kept: A<M<String>> = A::new(M::new(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:standup\r\nSUMMARY:Standup\r\nDTSTART;TZID=Europe/Berlin:20261102T090000\r\nDTEND;TZID=Europe/Berlin:20261102T091500\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".into(),
        ));
        let store_side = kept.clone();
        let (port, server) = crate::sync::dav::test_server(6, move |method, path, body| {
            match (method, path) {
            ("PROPFIND", _) => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:displayname>Home</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", _) => {
                let data = store_side.lock().unwrap().replace('&', "&amp;").replace('<', "&lt;");
                ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/standup.ics</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><c:calendar-data>{data}</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"))
            }
            ("PUT", "/cal/standup.ics") => {
                *store_side.lock().unwrap() = body.to_owned();
                ("204 No Content", String::new())
            }
            _ => ("404 Not Found", String::new()),
        }
        });
        let path = std::env::temp_dir().join(format!("picacho-cal-{}", uuid::Uuid::new_v4()));
        let mut b = Backend::new(Arc::new(Store::open(&path)?));
        b.test_secret = Some(Secret {
            password: "secret".into(),
            ..Default::default()
        });
        b.store.set_preference(
            SERVER,
            &serde_json::to_string(&caldav::Server {
                url: format!("http://127.0.0.1:{port}"),
                user: "ana".into(),
                ..Default::default()
            })?,
        )?;
        let cals = caldav::calendars(&b.calendar_server().unwrap(), "secret", b.tls_connector()?)?;
        b.store
            .set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        let events = caldav::events(
            &b.calendar_server().unwrap(),
            "secret",
            b.tls_connector()?,
            &cals,
            0,
            2_000_000_000,
        )?;
        b.save_events(events)?;
        let all = b.agenda(0, 2_000_000_000);
        assert_eq!(all.len(), 5);
        // Move the second one an hour later; leave the third out.
        let second = all[1].clone();
        let at = second.recurrence_id.unwrap();
        b.save_occurrence(
            "standup",
            at,
            Event {
                summary: "Standup (late)".into(),
                start: at + 3600,
                end: at + 3600 + 900,
                ..Default::default()
            },
        )?;
        b.delete_occurrence("standup", all[2].recurrence_id.unwrap())?;
        let after = b.agenda(0, 2_000_000_000);
        assert_eq!(after.len(), 4);
        assert_eq!(after[1].summary, "Standup (late)");
        assert_eq!(after[1].start, at + 3600);
        // What went to the server says the same: a RECURRENCE-ID and an EXDATE, in its zone.
        let put = kept.lock().unwrap().clone();
        assert!(
            put.contains("RECURRENCE-ID;TZID=Europe/Berlin:20261103T090000"),
            "{put}"
        );
        assert!(
            put.contains("EXDATE;TZID=Europe/Berlin:20261104T090000"),
            "{put}"
        );
        let mut back = ical::parse(&put).events;
        let series = back.remove(0);
        assert_eq!(ical::occurrences(&series, &back, 0, 2_000_000_000).len(), 4);
        drop(server);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// An event moved to another calendar is moved on the server (MOVE,
    /// RFC 4918 §9.9), then saved there; where the server can't, it is
    /// saved there anew and the old copy deleted, not left behind.
    #[test]
    fn moving_an_event_between_calendars() -> Result<()> {
        let (port, server) = crate::sync::dav::test_server(6, |method, path, _| match (method, path) {
            ("PROPFIND", _) => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/home/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("MOVE", "/home/a.ics") => ("201 Created", String::new()),
            ("PUT", "/work/a.ics") => ("204 No Content", String::new()),
            // This server can't move the second.
            ("MOVE", "/home/b.ics") => ("403 Forbidden", String::new()),
            ("PUT", "/work/b.ics") => ("201 Created", String::new()),
            ("DELETE", "/home/b.ics") => ("204 No Content", String::new()),
            _ => ("404 Not Found", String::new()),
        });
        let path = std::env::temp_dir().join(format!("picacho-cal-{}", uuid::Uuid::new_v4()));
        let mut b = Backend::new(Arc::new(Store::open(&path)?));
        b.test_secret = Some(Secret { password: "secret".into(), ..Default::default() });
        let base = format!("http://127.0.0.1:{port}");
        b.store.set_preference(SERVER, &serde_json::to_string(&caldav::Server { url: base.clone(), user: "ana".into(), ..Default::default() })?)?;
        let cals = caldav::calendars(&b.calendar_server().unwrap(), "secret", b.tls_connector()?)?;
        b.store.set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        let at = |uid: &str| Stored {
            event: Event { uid: uid.into(), summary: "Lunch".into(), start: 1_800_000_000, end: 1_800_003_600, ..Default::default() },
            calendar: format!("{base}/home/"),
            href: format!("{base}/home/{uid}.ics"),
            etag: "\"1\"".into(),
            ..Default::default()
        };
        b.save_events(vec![at("a"), at("b")])?;
        let work = format!("{base}/work/");
        let moved = b.save_event(at("a").event, Some(&work))?;
        assert_eq!((moved.calendar.as_str(), moved.href.as_str()), (work.as_str(), format!("{work}a.ics").as_str()));
        let saved = b.save_event(at("b").event, Some(&work))?;
        assert_eq!(saved.href, format!("{work}b.ics"));
        assert_eq!(b.calendar_events().len(), 2);
        let seen: Vec<String> = server.join().unwrap().iter().skip(1).map(|(m, p, _)| format!("{m} {p}")).collect();
        assert_eq!(seen, ["MOVE /home/a.ics", "PUT /work/a.ics", "MOVE /home/b.ics", "PUT /work/b.ics", "DELETE /home/b.ics"]);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// Declining one occurrence of an invitation takes only it out (RFC
    /// 5546 §3.2.3); accepting one keeps it as that occurrence's own.
    #[test]
    fn declining_one_occurrence_keeps_the_series() -> Result<()> {
        use std::sync::{Arc as A, Mutex as M};
        let series = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:weekly\r\nSUMMARY:Weekly\r\nDTSTART:20261102T090000Z\r\nDTEND:20261102T093000Z\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nORGANIZER:mailto:ana@example.org\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:qa@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let kept: A<M<String>> = A::new(M::new(series.into()));
        let store_side = kept.clone();
        let (port, server) = crate::sync::dav::test_server(5, move |method, path, body| {
            match (method, path) {
            ("PROPFIND", _) => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", _) => {
                let data = store_side.lock().unwrap().replace('&', "&amp;").replace('<', "&lt;");
                ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/weekly.ics</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><c:calendar-data>{data}</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"))
            }
            ("PUT", "/cal/weekly.ics") => {
                *store_side.lock().unwrap() = body.to_owned();
                ("204 No Content", String::new())
            }
            _ => ("404 Not Found", String::new()),
        }
        });
        let path = std::env::temp_dir().join(format!("picacho-cal-{}", uuid::Uuid::new_v4()));
        let mut b = Backend::new(Arc::new(Store::open(&path)?));
        b.test_secret = Some(Secret { password: "secret".into(), ..Default::default() });
        b.store.set_preference(
            SERVER,
            &serde_json::to_string(&caldav::Server {
                url: format!("http://127.0.0.1:{port}"),
                user: "ana".into(),
                ..Default::default()
            })?,
        )?;
        let cals = caldav::calendars(&b.calendar_server().unwrap(), "secret", b.tls_connector()?)?;
        b.store.set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        let events = caldav::events(&b.calendar_server().unwrap(), "secret", b.tls_connector()?, &cals, 0, 2_000_000_000)?;
        b.save_events(events)?;
        let all = b.agenda(0, 2_000_000_000);
        assert_eq!(all.len(), 4);
        // An invitation to the second occurrence only, declined.
        let second = all[1].recurrence_id.unwrap();
        let one = |at: i64| Event {
            recurrence: String::new(),
            recurrence_id: Some(at),
            start: at,
            end: at + 1800,
            source: String::new(),
            ..all[0].clone()
        };
        b.file_answer(&one(second), "qa@example.com", PartStat::Declined, true)?;
        let after = b.agenda(0, 2_000_000_000);
        assert_eq!(after.len(), 3, "only that occurrence went");
        assert!(after.iter().all(|e| e.start != second));
        assert!(kept.lock().unwrap().contains("EXDATE:20261109T090000Z"), "{}", kept.lock().unwrap());
        // The third accepted tentatively: its own, the series as it was.
        let third = all[2].recurrence_id.unwrap();
        b.file_answer(&one(third), "qa@example.com", PartStat::Tentative, true)?;
        let stored = b.stored_event("weekly").unwrap();
        assert_eq!(stored.event.recurrence, "FREQ=WEEKLY;COUNT=4");
        assert_eq!(stored.overrides.len(), 1);
        assert_eq!(stored.overrides[0].attendees[0].status, Some(PartStat::Tentative));
        assert_eq!(b.agenda(0, 2_000_000_000).len(), 3);
        let put = kept.lock().unwrap().clone();
        assert!(put.contains("RECURRENCE-ID:20261116T090000Z"), "{put}");
        // An answer to an invitation its organizer's domain didn't vouch
        // for: it goes on the calendar's own copy, whatever the mail said.
        let fourth = all[3].recurrence_id.unwrap();
        let rewritten = Event { summary: "Somewhere else".into(), location: "https://evil.example".into(), ..one(fourth) };
        b.file_answer(&rewritten, "qa@example.com", PartStat::Accepted, false)?;
        let stored = b.stored_event("weekly").unwrap();
        let own = stored.overrides.iter().find(|o| o.recurrence_id == Some(fourth)).unwrap();
        assert_eq!((own.summary.as_str(), own.location.as_str()), ("Weekly", ""));
        assert_eq!(own.attendees[0].status, Some(PartStat::Accepted));
        drop(server);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    #[test]
    fn guests_answers_and_proposed_times_reach_the_organizer() -> Result<()> {
        use std::sync::{Arc as A, Mutex as M};
        let kept: A<M<String>> = A::new(M::new(String::new()));
        let store_side = kept.clone();
        let (port, server) = crate::sync::dav::test_server(4, move |method, _, body| {
            match method {
            "PROPFIND" => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            "PUT" => {
                *store_side.lock().unwrap() = body.to_owned();
                ("204 No Content", String::new())
            }
            _ => ("404 Not Found", String::new()),
        }
        });
        let path = std::env::temp_dir().join(format!("picacho-cal-{}", uuid::Uuid::new_v4()));
        let mut b = Backend::new(Arc::new(Store::open(&path)?));
        b.test_secret = Some(Secret {
            password: "secret".into(),
            ..Default::default()
        });
        // Ana's domain vouches for her mail (DMARC); no one else's does yet.
        b.test_dmarc_pass = vec!["ana@example.org".into()];
        let mut a = Account::new(Provider::Pop3);
        a.email = "qa@example.com".into();
        a.name = "QA".into();
        b.store.set_preference(
            SERVER,
            &serde_json::to_string(&caldav::Server {
                url: format!("http://127.0.0.1:{port}"),
                user: "ana".into(),
                ..Default::default()
            })?,
        )?;
        let cals = caldav::calendars(&b.calendar_server().unwrap(), "secret", b.tls_connector()?)?;
        b.store
            .set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        let guest = |email: &str| ical::Attendee {
            email: email.into(),
            ..Default::default()
        };
        let party = Event {
            uid: "party".into(),
            summary: "Party".into(),
            start: 1_800_000_000,
            end: 1_800_007_200,
            organizer: Some(ical::Attendee {
                email: a.email.clone(),
                name: "QA".into(),
                status: None,
                ..Default::default()
            }),
            attendees: vec![guest("ana@example.org"), guest("bo@example.se")],
            ..Default::default()
        };
        b.save_event(party.clone(), None)?;
        // A mail from someone, carrying a calendar part, kept as downloaded.
        let mail = |id: &str, from: &str, calendar: &str| -> Result<Mail> {
            let raw = format!(
                "From: {from}\r\nTo: qa@example.com\r\nSubject: x\r\nMessage-ID: <{id}@x>\r\nMIME-Version: 1.0\r\nContent-Type: text/calendar; method=REPLY; charset=utf-8\r\n\r\n{calendar}"
            );
            let m = parse_message(raw.as_bytes(), &a, id, Folder::Inbox, "INBOX", 1)?;
            b.store.save_download(&m, raw.as_bytes())?;
            Ok(m)
        };
        // Ana accepts.
        let mut answer = party.clone();
        answer.attendees = vec![ical::Attendee {
            email: "ana@example.org".into(),
            name: "Ana".into(),
            status: Some(PartStat::Accepted),
            ..Default::default()
        }];
        let yes = ical::request(&answer, &[], false).replace("METHOD:REQUEST", "METHOD:REPLY");
        assert_eq!(
            b.apply_reply(&a, &mail("r1", "ana@example.org", &yes)?)?,
            "Ana: accepted"
        );
        let stored = b.stored_event("party").unwrap();
        assert_eq!(stored.event.attendees[0].status, Some(PartStat::Accepted));
        assert!(
            kept.lock().unwrap().replace("\r\n ", "").contains("PARTSTAT=ACCEPTED"),
            "the server has it: {}", kept.lock().unwrap()
        );
        // Someone else can't answer for her.
        let forged = mail(
            "r2",
            "mallory@evil.example",
            &yes.replace("ACCEPTED", "DECLINED"),
        )?;
        assert!(b.apply_reply(&a, &forged).is_err());
        assert_eq!(
            b.stored_event("party").unwrap().event.attendees[0].status,
            Some(PartStat::Accepted)
        );
        // Bo proposes an hour later; the card shows it, and it's read right.
        let counter = ical::counter(
            &party,
            "bo@example.se",
            "",
            party.start + 3600,
            party.end + 3600,
            "Later?",
        )?;
        let proposal = mail("c1", "bo@example.se", &counter)?;
        let cal = b.invitation(&a, &proposal)?.unwrap();
        assert_eq!(cal.method, "COUNTER");
        assert_eq!(cal.events[0].start, party.start + 3600);
        // A changed invitation, as a guest would see it.
        let mut later = party.clone();
        later.start += 3600;
        later.sequence = 1;
        assert!(b.invitation_changed(&later).is_some());
        assert!(
            b.invitation_changed(&b.stored_event("party").unwrap().event)
                .is_none()
        );
        // Someone who knows the event's UID can't rewrite or cancel it by
        // naming themselves as its organizer, or no organizer at all.
        let mut moved = party.clone();
        moved.summary = "Moved".into();
        moved.sequence = 9;
        for organizer in [None, Some(guest("mallory@evil.example"))] {
            moved.organizer = organizer;
            let update = mail("u1", "mallory@evil.example", &ical::request(&moved, &[], false))?;
            assert!(b.take_update(&a, &update).is_err());
            let cancel = mail("x1", "mallory@evil.example", &ical::request(&moved, &[], true))?;
            assert!(b.remove_cancelled(&a, &cancel).is_err());
            assert_eq!(b.stored_event("party").unwrap().event.summary, "Party");
        }
        // Nor accept a time for the event as if a guest had proposed it.
        let forged = ical::counter(&party, "mallory@evil.example", "", party.start + 7200, party.end + 7200, "")?;
        assert!(b.accept_counter(&a, &mail("c2", "mallory@evil.example", &forged)?).is_err());
        assert_eq!(b.stored_event("party").unwrap().event.start, party.start);
        // Nor add it again from mail, over the one in the calendar.
        assert!(b.add_event_from_mail(&a, &mail("p1", "mallory@evil.example", "")?, moved.clone()).is_err());
        // The right address in From isn't enough: its domain must vouch for
        // the mail (DMARC), or anyone could write it there.
        moved.organizer = party.organizer.clone();
        let update = mail("u2", "qa@example.com", &ical::request(&moved, &[], false))?;
        let refused = b.take_update(&a, &update).unwrap_err();
        assert!(format!("{refused:#}").contains("DMARC"), "{refused:#}");
        let cancel = mail("x2", "qa@example.com", &ical::request(&moved, &[], true))?;
        assert!(b.remove_cancelled(&a, &cancel).is_err());
        let bo = mail("r3", "bo@example.se", &yes.replace("ana@example.org", "bo@example.se"))?;
        assert!(b.apply_reply(&a, &bo).is_err());
        assert!(b.accept_counter(&a, &proposal).is_err());
        assert_eq!(b.stored_event("party").unwrap().event.summary, "Party");
        // Vouched for, the organizer's update is taken.
        b.test_dmarc_pass.push("qa@example.com".into());
        b.take_update(&a, &update)?;
        assert_eq!(b.stored_event("party").unwrap().event.summary, "Moved");
        drop(server);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
