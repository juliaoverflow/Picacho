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

const SERVER: &str = "calendar_server";
const CALENDARS: &str = "calendars";
const EVENTS: &str = "calendar_events";
const SYNCED: &str = "calendar_synced";
/// The calendar new events go to (its href).
const DEFAULT: &str = "calendar_default";
/// Invitations answered here: event UID → answer.
const ANSWERS: &str = "calendar_answers";
#[cfg_attr(test, allow(dead_code))]
const PASSWORD: &str = "calendar:caldav";

/// How far around today the server is read.
const BEHIND: i64 = 31 * 86_400;
const AHEAD: i64 = 366 * 86_400;

impl Backend {
    pub fn calendar_server(&self) -> Option<caldav::Server> {
        self.store
            .preference(SERVER)
            .and_then(|j| serde_json::from_str(&j).ok())
            .filter(|s: &caldav::Server| !s.url.is_empty())
    }
    pub fn calendars(&self) -> Vec<caldav::Calendar> {
        self.store
            .preference(CALENDARS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    /// Every event known here, soonest first.
    pub fn calendar_events(&self) -> Vec<Stored> {
        self.store
            .preference(EVENTS)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    fn save_events(&self, mut events: Vec<Stored>) -> Result<()> {
        events.sort_by_key(|s| s.event.start);
        self.store
            .set_preference(EVENTS, &serde_json::to_string(&events)?)
    }
    pub fn calendar_synced(&self) -> Option<i64> {
        self.store.preference(SYNCED)?.parse().ok()
    }
    /// The calendar new events go to: the one chosen, or the first that
    /// can be written.
    pub fn default_calendar(&self) -> Option<caldav::Calendar> {
        let cals = self.calendars();
        let chosen = self.store.preference(DEFAULT).unwrap_or_default();
        cals.iter()
            .find(|c| c.href == chosen && c.writable)
            .or_else(|| cals.iter().find(|c| c.writable))
            .cloned()
    }
    pub fn set_default_calendar(&self, href: &str) -> Result<()> {
        self.store.set_preference(DEFAULT, href)
    }
    fn calendar_password(&self) -> Result<String> {
        #[cfg(test)]
        if let Some(s) = &self.test_secret {
            return Ok(s.password.clone());
        }
        store::read_secret_text(PASSWORD)?
            .context("The calendar server's password is missing. Set it up again.")
    }
    /// Signs in to a CalDAV server and reads its calendars and events.
    pub fn connect_calendar(&self, server: caldav::Server, password: &str) -> Result<usize> {
        let cals = caldav::calendars(&server, password, self.tls_connector()?)?;
        #[cfg(not(test))]
        store::write_secret_text(PASSWORD, password)?;
        self.store
            .set_preference(SERVER, &serde_json::to_string(&server)?)?;
        self.store
            .set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        self.sync_calendar()
    }
    /// Reads the calendars and their events again.
    pub fn sync_calendar(&self) -> Result<usize> {
        let server = self
            .calendar_server()
            .context("No calendar server is set up.")?;
        let password = self.calendar_password()?;
        let cals = caldav::calendars(&server, &password, self.tls_connector()?)?;
        self.store
            .set_preference(CALENDARS, &serde_json::to_string(&cals)?)?;
        let now = chrono::Utc::now().timestamp();
        let events = caldav::events(
            &server,
            &password,
            self.tls_connector()?,
            &cals,
            now - BEHIND,
            now + AHEAD,
        )?;
        let count = events.len();
        self.save_events(events)?;
        self.store.set_preference(SYNCED, &now.to_string())?;
        Ok(count)
    }
    pub fn disconnect_calendar(&self) -> Result<()> {
        for key in [SERVER, CALENDARS, EVENTS, SYNCED, DEFAULT] {
            self.store.set_preference(key, "")?;
        }
        #[cfg(not(test))]
        store::delete_secret(PASSWORD)?;
        Ok(())
    }
    /// Adds an event, or saves changes to one (by UID), on the server.
    pub fn save_event(&self, mut event: Event, calendar: Option<&str>) -> Result<Stored> {
        ensure!(!event.summary.trim().is_empty(), "Give the event a title.");
        ensure!(event.end >= event.start, "The event ends before it starts.");
        let server = self
            .calendar_server()
            .context("Set up a calendar server in Settings → Calendar first.")?;
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
                    .context("None of your calendars can take new events.")?
                    .href
            }
        };
        let mut stored = existing.unwrap_or_default();
        if stored.calendar != target {
            // Moving it to another calendar: a new resource there.
            stored.href.clear();
            stored.etag.clear();
        }
        if !stored.href.is_empty() {
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
        let old = stored.clone();
        stored.event = event;
        let saved = caldav::save(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &target,
            &stored,
        )?;
        if !old.href.is_empty() && old.calendar != target {
            let _ = caldav::delete(
                &server,
                &self.calendar_password()?,
                self.tls_connector()?,
                &old,
            );
        }
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved.clone());
        self.save_events(events)?;
        Ok(saved)
    }
    /// Every time an event happens between `from` and `to`: repeating
    /// events once for each repeat, changed and cancelled repeats as they
    /// now are. Soonest first.
    pub fn agenda(&self, from: i64, to: i64) -> Vec<Event> {
        let mut out: Vec<Event> = self
            .calendar_events()
            .iter()
            .flat_map(|s| ical::occurrences(&s.event, &s.overrides, from, to))
            .collect();
        out.sort_by_key(|e| e.start);
        out
    }
    /// Changes one repeat of a repeating event (the one that started at
    /// `occurrence`), leaving the others as they are.
    pub fn save_occurrence(&self, uid: &str, occurrence: i64, mut changed: Event) -> Result<()> {
        ensure!(
            !changed.summary.trim().is_empty(),
            "Give the event a title."
        );
        ensure!(
            changed.end >= changed.start,
            "The event ends before it starts."
        );
        let server = self
            .calendar_server()
            .context("No calendar server is set up.")?;
        let mut events = self.calendar_events();
        let stored = events
            .iter_mut()
            .find(|s| s.event.uid == uid)
            .context("This event is no longer in your calendar. Read it again.")?;
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
        let saved = caldav::save(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &calendar,
            stored,
        )?;
        *stored = saved;
        self.save_events(events)
    }
    /// Takes one repeat of a repeating event out (EXDATE), leaving the others.
    pub fn delete_occurrence(&self, uid: &str, occurrence: i64) -> Result<()> {
        let server = self
            .calendar_server()
            .context("No calendar server is set up.")?;
        let mut events = self.calendar_events();
        let stored = events
            .iter_mut()
            .find(|s| s.event.uid == uid)
            .context("This event is no longer in your calendar. Read it again.")?;
        stored
            .overrides
            .retain(|o| o.recurrence_id != Some(occurrence));
        if !stored.event.exdates.contains(&occurrence) {
            stored.event.exdates.push(occurrence);
        }
        stored.event.sequence += 1;
        stored.event.source.clear();
        let calendar = stored.calendar.clone();
        let saved = caldav::save(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &calendar,
            stored,
        )?;
        *stored = saved;
        self.save_events(events)
    }
    pub fn delete_event(&self, uid: &str) -> Result<()> {
        let mut events = self.calendar_events();
        if let Some(stored) = events.iter().find(|s| s.event.uid == uid)
            && let Some(server) = self.calendar_server()
        {
            caldav::delete(
                &server,
                &self.calendar_password()?,
                self.tls_connector()?,
                stored,
            )?;
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
            .context("This message has no invitation.")?;
        let event = cal
            .events
            .first()
            .cloned()
            .context("This message has no invitation.")?;
        ensure!(!event.cancelled, "This event was cancelled.");
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
            .context("This invitation doesn't say who to answer.")?;
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
        let result = match status {
            PartStat::Declined => self.delete_event(&event.uid),
            _ => {
                let mut kept = event.clone();
                for at in &mut kept.attendees {
                    if at.email.eq_ignore_ascii_case(&me) {
                        at.status = Some(status);
                    }
                }
                self.save_invited(kept)
            }
        };
        Ok(result
            .err()
            .map(|e| format!("The answer was sent, but the calendar wasn't changed: {e:#}")))
    }
    /// Saves an invitation's event as the organizer sent it.
    fn save_invited(&self, event: Event) -> Result<()> {
        let server = self
            .calendar_server()
            .context("No calendar server is set up.")?;
        let mut events = self.calendar_events();
        let mut stored = events
            .iter()
            .find(|s| s.event.uid == event.uid)
            .cloned()
            .unwrap_or_default();
        let target = if stored.calendar.is_empty() {
            self.default_calendar()
                .context("None of your calendars can take new events.")?
                .href
        } else {
            stored.calendar.clone()
        };
        stored.event = event;
        let saved = caldav::save(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &target,
            &stored,
        )?;
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved);
        self.save_events(events)
    }
    /// Takes a cancelled event out of the calendar.
    /// The calendar's copy of the event an invitation message is about, if
    /// it's in the calendar.
    pub fn stored_event(&self, uid: &str) -> Option<Stored> {
        self.calendar_events()
            .into_iter()
            .find(|s| s.event.uid == uid)
    }

    /// A guest's answer (REPLY) to an event you organize: their answer is
    /// noted on the calendar's copy. Returns who answered what.
    pub fn apply_reply(&self, a: &Account, m: &Mail) -> Result<String> {
        let cal = self
            .invitation(a, m)?
            .context("This message has no answer to an invitation.")?;
        ensure!(
            cal.method == "REPLY",
            "This message isn't an answer to an invitation."
        );
        let answer = cal
            .events
            .first()
            .context("This message has no answer to an invitation.")?;
        let guest = answer
            .attendees
            .first()
            .context("The answer doesn't say who it's from.")?;
        let status = guest.status.unwrap_or(PartStat::NeedsAction);
        // The sender must be the guest answering: anyone can write a REPLY.
        ensure!(
            m.from.eq_ignore_ascii_case(&guest.email),
            "This answer is for {}, but came from {}; it wasn't applied.",
            guest.email,
            m.from
        );
        let mut stored = self
            .stored_event(&answer.uid)
            .context("The event this answers isn't in your calendar.")?;
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
            None => bail!("{} isn't a guest of this event.", guest.email),
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
            .context("This message proposes no time.")?;
        ensure!(cal.method == "COUNTER", "This message proposes no time.");
        let proposed = cal
            .events
            .first()
            .context("This message proposes no time.")?;
        let guest = proposed
            .attendees
            .first()
            .map(|g| g.email.clone())
            .unwrap_or_default();
        ensure!(
            guest.is_empty() || m.from.eq_ignore_ascii_case(&guest),
            "This proposal is for {guest}, but came from {}.",
            m.from
        );
        let mut stored = self
            .stored_event(&proposed.uid)
            .context("The event this proposal is for isn't in your calendar.")?;
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
            .context("This message proposes no time.")?;
        ensure!(cal.method == "COUNTER", "This message proposes no time.");
        let proposed = cal
            .events
            .first()
            .context("This message proposes no time.")?;
        let guest = proposed
            .attendees
            .first()
            .map(|g| g.email.clone())
            .context("The proposal doesn't say who it's from.")?;
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
            .context("This message has no invitation.")?;
        let event = cal
            .events
            .first()
            .cloned()
            .context("This message has no invitation.")?;
        ensure!(!event.cancelled, "This event was cancelled.");
        let (me, my_name) = self.invited_as(a, &event);
        let organizer = event
            .organizer
            .clone()
            .context("This invitation doesn't say who to answer.")?;
        let counter = ical::counter(&event, &me, &my_name, start, end, note)?;
        let when = ical::describe_when(&Event {
            start,
            end,
            ..event.clone()
        });
        let text = format!(
            "{} proposes another time for {}:\r\n{when}\r\n\r\n{note}\r\n",
            if my_name.is_empty() { &me } else { &my_name },
            event.summary
        );
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
            .context("This message has no invitation.")?;
        let mut event = cal
            .events
            .first()
            .cloned()
            .context("This message has no invitation.")?;
        let (me, _) = self.invited_as(a, &event);
        if let Some(answer) = self.invitation_answer(&event.uid) {
            for g in &mut event.attendees {
                if g.email.eq_ignore_ascii_case(&me) {
                    g.status = Some(answer);
                }
            }
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

    /// Saves a changed event to the server and here.
    fn store_updated(&self, stored: Stored) -> Result<()> {
        let server = self
            .calendar_server()
            .context("No calendar server is set up.")?;
        let calendar = stored.calendar.clone();
        let saved = caldav::save(
            &server,
            &self.calendar_password()?,
            self.tls_connector()?,
            &calendar,
            &stored,
        )?;
        let mut events = self.calendar_events();
        events.retain(|s| s.event.uid != saved.event.uid);
        events.push(saved);
        self.save_events(events)
    }

    pub fn remove_cancelled(&self, a: &Account, m: &Mail) -> Result<()> {
        let cal = self
            .invitation(a, m)?
            .context("This message has no invitation.")?;
        for e in cal.events.iter().filter(|e| e.cancelled) {
            self.delete_event(&e.uid)?;
        }
        Ok(())
    }

    /// Invites an event's guests (or tells them it changed): one iMIP
    /// REQUEST to all of them, from `a` as the organizer. Returns how many
    /// were told.
    pub fn send_invitations(&self, a: &Account, uid: &str) -> Result<usize> {
        let stored = self
            .calendar_events()
            .into_iter()
            .find(|s| s.event.uid == uid)
            .context("This event is no longer in your calendar.")?;
        self.invite(a, &stored.event, &stored.overrides, false)
    }
    /// Tells an event's guests it's cancelled (before deleting it).
    pub fn cancel_invitations(&self, a: &Account, uid: &str) -> Result<usize> {
        let stored = self
            .calendar_events()
            .into_iter()
            .find(|s| s.event.uid == uid)
            .context("This event is no longer in your calendar.")?;
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
            "Only the organizer can send this event's invitations."
        );
        let guests: Vec<String> = event
            .attendees
            .iter()
            .map(|g| g.email.trim().to_owned())
            .filter(|g| !g.is_empty() && !mine(g))
            .collect();
        ensure!(!guests.is_empty(), "This event has no guests to invite.");
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
            });
        }
        let calendar = ical::request(&event, overrides, cancel);
        let when = crate::sync::ical::describe_when(&event);
        let (subject, text) = if cancel {
            (
                format!("Cancelled: {}", event.summary),
                format!("{} was cancelled.\r\n\r\n{when}\r\n", event.summary),
            )
        } else {
            let place = if event.location.is_empty() {
                String::new()
            } else {
                format!("\r\n{}", event.location)
            };
            (
                format!("Invitation: {}", event.summary),
                format!(
                    "{} invites you to {}.\r\n\r\n{when}{place}\r\n\r\n{}\r\n",
                    a.label(),
                    event.summary,
                    event.description
                ),
            )
        };
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

/// This computer's time zone, as an IANA name ("Europe/Berlin"): `TZ`
/// when set, else what the system says (macOS, Windows, Linux, Android
/// and iOS each keep it their own way).
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
fn header_text(text: &str) -> String {
    use base64::Engine as _;
    if text.is_ascii() && !text.contains(['\r', '\n']) {
        text.to_owned()
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(text.replace(['\r', '\n'], " "))
        )
    }
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
            PartStat::NeedsAction => "answered",
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
        out.push_str(&format!("From: {} <{me}>\r\n", header_text(my_name)));
    }
    let to_line = to
        .iter()
        .map(|t| format!("<{t}>"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("To: {to_line}\r\n"));
    out.push_str(&format!("Subject: {}\r\n", header_text(subject)));
    out.push_str(&format!("Date: {}\r\n", chrono::Local::now().to_rfc2822()));
    out.push_str(&format!(
        "Message-ID: <{}@{domain}>\r\n",
        uuid::Uuid::new_v4()
    ));
    let original = in_reply_to.trim();
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

#[cfg(test)]
mod tests {
    use super::*;

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
            }),
            attendees: vec![
                ical::Attendee {
                    email: "ana@example.org".into(),
                    name: "Ana".into(),
                    status: None,
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
        let mut a = Account::new(Provider::Pop3);
        a.email = "qa@example.com".into();
        a.name = "QA".into();
        b.store.set_preference(
            SERVER,
            &serde_json::to_string(&caldav::Server {
                url: format!("http://127.0.0.1:{port}"),
                user: "ana".into(),
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
        }];
        let yes = ical::request(&answer, &[], false).replace("METHOD:REQUEST", "METHOD:REPLY");
        assert_eq!(
            b.apply_reply(&a, &mail("r1", "ana@example.org", &yes)?)?,
            "Ana: accepted"
        );
        let stored = b.stored_event("party").unwrap();
        assert_eq!(stored.event.attendees[0].status, Some(PartStat::Accepted));
        assert!(
            kept.lock().unwrap().contains("PARTSTAT=ACCEPTED"),
            "the server has it"
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
        drop(server);
        drop(b);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
