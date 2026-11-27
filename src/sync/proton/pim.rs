//! Proton Calendar and Proton Contacts, for a Proton account signed in
//! without Bridge: the same session and keys as its mail.
//!
//! Both are end-to-end encrypted, kept as parts ("cards") of each vCard or
//! event: some signed and in the clear (what Proton's servers need: a
//! card's name and addresses, an event's times and rule), the rest
//! encrypted and signed.
//!
//! - Contacts: a card's parts are encrypted to, and signed with, the
//!   account's own (user) key, and read and written by the library's
//!   contact-card functions (`mobile::contacts::cards`), which verify each
//!   signature. Read, the parts are put together into one vCard; a contact
//!   with a part that can't be read or verified is shown by its name and
//!   addresses only, and not written back (that would lose the rest).
//!   Written, the name, UID and addresses go in the signed part, the rest
//!   in the encrypted one.
//! - Calendars: each calendar has its own key, unlocked with the member
//!   address's keys (`mobile::calendar::keys`, which verifies the
//!   passphrase's signature). An event's parts share one session key,
//!   wrapped to the calendar key (`SharedKeyPacket`) or, for invitations
//!   not yet moved over, to the address's (`AddressKeyPacket`). Read, the
//!   parts' properties are put together into one VEVENT; written, an event
//!   is split into them as Proton's own apps do and encrypted by the
//!   library (`encrypt_event`). Reminders are each member's own
//!   (`Notifications`, `put_personal`). Events with guests are left to
//!   Proton's apps: their invitations need Proton's attendee tokens.
//!
//! The library has no calls for listing calendars, reading events in a time
//! range, writing events, or creating and changing contacts; those few are
//! made here with the session's tokens, renewed by the library when Proton
//! refuses them.
use super::{Explained, Session, block_on};
use crate::sync::carddav::{self, Card};
use crate::sync::ical::{self, Event};
use anyhow::{Context, Result, anyhow, bail, ensure};
use protected_email_bridge::config::limits::mobile as limits;
use protected_email_bridge::crypto::{KeyRing, UnlockedKey};
use protected_email_bridge::mobile::calendar::{self as pcal, CalendarEvent, Notification, NotificationsUpdate, keys as pkeys, part_type};
use protected_email_bridge::mobile::contacts::{self as pcontacts, cards as pcards};
use serde_json::{Value, json};

/// A calendar member's permission to change events (Proton's `EDIT`).
const EDIT: i64 = 16;
/// Where a changed occurrence keeps its own Proton id, in `Event::kept`.
pub const PROTON_ID: &str = "X-PICACHO-PROTON-ID:";
/// The properties of an event's parts, as Proton's apps split them.
const SHARED_SIGNED: &[&str] = &["UID", "DTSTAMP", "DTSTART", "DTEND", "RECURRENCE-ID", "RRULE", "EXDATE", "RDATE", "ORGANIZER", "SEQUENCE"];
const SHARED_ENCRYPTED: &[&str] = &["UID", "DTSTAMP", "CREATED", "DESCRIPTION", "SUMMARY", "LOCATION"];
const CALENDAR_SIGNED: &[&str] = &["UID", "DTSTAMP", "STATUS", "TRANSP"];
/// The properties of a vCard Proton keeps signed in the clear.
const CARD_SIGNED: &[&str] = &["FN", "UID", "EMAIL"];

const PRODID: &str = concat!("PRODID:-//Pinal Technologies//Picacho ", env!("CARGO_PKG_VERSION"), "//EN");

/// A calendar of the account's, as the member it is shared with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtonCalendar {
    pub id: String,
    pub name: String,
    pub color: String,
    pub writable: bool,
    /// The account's membership: its id, and the address it's shared with.
    pub member: String,
    pub address: String,
}

/// An event as read: the series (or single event) and its changed
/// occurrences, its calendar and id, and when it last changed.
#[derive(Clone, Debug, Default)]
pub struct ProtonEvent {
    pub event: Event,
    pub overrides: Vec<Event>,
    pub calendar: String,
    pub id: String,
    pub version: String,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// Content lines, unfolded (RFC 5545 §3.1, RFC 6350 §3.2).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match line.strip_prefix([' ', '\t']) {
            Some(rest) if !lines.is_empty() => lines.last_mut().unwrap().push_str(rest),
            _ if line.is_empty() => {}
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// A content line's property name, without its group (`item1.EMAIL`).
fn property(line: &str) -> String {
    let head = line.split([':', ';']).next().unwrap_or_default();
    head.rsplit('.').next().unwrap_or(head).to_ascii_uppercase()
}

/// The lines inside the first `component` (its own sub-components
/// included, as an event's alarms).
fn inside(text: &str, component: &str) -> Vec<String> {
    let begin = format!("BEGIN:{component}");
    let end = format!("END:{component}");
    let mut out = Vec::new();
    let mut depth = 0usize;
    for line in unfold(text) {
        let upper = line.to_ascii_uppercase();
        if depth == 0 {
            if upper == begin {
                depth = 1;
            }
            continue;
        }
        if upper == end && depth == 1 {
            break;
        }
        if upper.starts_with("BEGIN:") {
            depth += 1;
        } else if upper.starts_with("END:") {
            depth -= 1;
        }
        out.push(line);
    }
    out
}

/// Puts the properties of several parts together, each of `once` only
/// from the first part that has it.
fn merge(parts: &[Vec<String>], once: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        for line in part {
            let name = property(line);
            if once.contains(&name.as_str()) && out.iter().any(|l| property(l) == name) {
                continue;
            }
            out.push(line.clone());
        }
    }
    out
}

/// A vCard's parts put together, as one vCard.
pub fn merged_vcard(parts: &[String]) -> String {
    let lines: Vec<Vec<String>> = parts
        .iter()
        .map(|p| inside(p, "VCARD").into_iter().filter(|l| property(l) != "VERSION").collect())
        .collect();
    let mut out = String::from("BEGIN:VCARD\r\nVERSION:4.0\r\n");
    for line in merge(&lines, &["UID", "FN", "PRODID"]) {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out.push_str("END:VCARD\r\n");
    out
}

/// A vCard split as Proton keeps it: (signed in the clear, encrypted).
pub fn split_vcard(vcard: &str) -> (String, String) {
    let lines: Vec<String> = inside(vcard, "VCARD").into_iter().filter(|l| property(l) != "VERSION").collect();
    let card = |keep: &dyn Fn(&str) -> bool| {
        let mut out = String::from("BEGIN:VCARD\r\nVERSION:4.0\r\n");
        for line in lines.iter().filter(|l| keep(&property(l))) {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("END:VCARD\r\n");
        out
    };
    let signed = card(&|name| CARD_SIGNED.contains(&name));
    // Its UID in both, as Proton's own cards have it.
    let encrypted = card(&|name| !CARD_SIGNED.contains(&name) || name == "UID");
    (signed, encrypted)
}

/// An event's parts put together, as one VEVENT in a calendar object.
pub fn merged_event(parts: &[String]) -> String {
    let lines: Vec<Vec<String>> = parts.iter().map(|p| inside(p, "VEVENT")).collect();
    let mut out = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{PRODID}\r\nBEGIN:VEVENT\r\n");
    for line in merge(&lines, &["UID", "DTSTAMP", "CREATED", "SEQUENCE", "DTSTART", "DTEND"]) {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
    out
}

/// An event split as Proton keeps it: (shared signed, shared encrypted,
/// calendar signed). Each is a calendar object of its own. Alarms aren't
/// parts: they are each member's own (`notifications`).
pub fn split_event(vevent: &str) -> [String; 3] {
    // Sub-components (alarms) are left out; the rest are single lines.
    let mut plain: Vec<String> = Vec::new();
    let mut depth = 0usize;
    for line in inside(vevent, "VEVENT") {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
        } else if upper.starts_with("END:") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 {
            plain.push(line);
        }
    }
    let part = |names: &[&str]| {
        let mut out = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{PRODID}\r\nBEGIN:VEVENT\r\n");
        for line in plain.iter().filter(|l| names.contains(&property(l).as_str())) {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
        out
    };
    [part(SHARED_SIGNED), part(SHARED_ENCRYPTED), part(CALENDAR_SIGNED)]
}

/// An event's reminders as Proton's notifications: a push each, its
/// trigger a duration before the start (after, for negative ones).
pub fn notifications(e: &Event) -> Vec<Notification> {
    e.alarms
        .iter()
        .map(|before| {
            let (sign, seconds) = if *before >= 0 { ("-", *before) } else { ("", -before) };
            let trigger = if seconds % 60 == 0 { format!("{sign}PT{}M", seconds / 60) } else { format!("{sign}PT{seconds}S") };
            Notification { kind: 1, trigger }
        })
        .collect()
}

/// Proton's notifications as alarm components, for reading with the rest.
fn alarm_lines(notifications: &[Notification]) -> Vec<String> {
    notifications
        .iter()
        .filter(|n| !n.trigger.trim().is_empty() && !n.trigger.contains(['\r', '\n']))
        .flat_map(|n| {
            [
                "BEGIN:VALARM".to_owned(),
                "ACTION:DISPLAY".to_owned(),
                format!("TRIGGER:{}", n.trigger.trim()),
                "END:VALARM".to_owned(),
            ]
        })
        .collect()
}

/// The Proton id a changed occurrence was read with, if any.
pub fn occurrence_id(e: &Event) -> Option<&str> {
    e.kept.lines().find_map(|l| l.strip_prefix(PROTON_ID)).map(str::trim)
}

impl Session {
    /// A call to Proton's API with the session's sign-in, which the library
    /// renews (once) when Proton refuses it. Proton's answer, checked.
    fn pim(&self, method: reqwest::Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<Value> {
        use std::sync::OnceLock;
        static HTTP: OnceLock<reqwest::blocking::Client> = OnceLock::new();
        let http = match HTTP.get() {
            Some(h) => h,
            None => {
                let built = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(90))
                    .connect_timeout(std::time::Duration::from_secs(30))
                    .redirect(reqwest::redirect::Policy::none())
                    .https_only(true)
                    .build()?;
                HTTP.get_or_init(|| built)
            }
        };
        let client = &self.ctx.client;
        let url = format!("{}{path}", client.config().api_base_url.trim_end_matches('/'));
        for attempt in 0..2 {
            let tokens = block_on(client.tokens());
            let agent = block_on(client.user_agent());
            let mut request = http
                .request(method.clone(), &url)
                .header("x-pm-appversion", client.config().app_version_header())
                .header("x-pm-uid", &tokens.uid)
                .header(reqwest::header::USER_AGENT, agent)
                .bearer_auth(&tokens.access_token)
                .query(query);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request
                .send()
                .map_err(|e| anyhow::Error::new(crate::sync::retry::Unreachable(t!("sync-proton-couldnt-reach-proton", why = e.to_string()))))?;
            let status = response.status();
            let bytes = response.bytes().unwrap_or_default();
            if status.as_u16() == 401 && attempt == 0 {
                block_on(client.refresh()).explained()?;
                continue;
            }
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let code = v["Code"].as_i64().unwrap_or_default();
            if status.is_success() && (code == 1000 || code == 1001) {
                return Ok(v);
            }
            if status.as_u16() == 401 {
                return Err(anyhow!(t!("sync-proton-proton-ended-this-session-sign-in"))
                    .context(crate::manage::status::AccessFailure::Authentication));
            }
            bail!(t!(
                "sync-proton-pim-proton-answered",
                status = status.as_u16(),
                code = code,
                message = text(&v["Error"])
            ));
        }
        unreachable!("the loop returns on its second turn")
    }
    fn pim_get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.pim(reqwest::Method::GET, path, query, None)
    }

    // ---- Contacts ------------------------------------------------------

    /// Every contact, decrypted and verified. A contact whose version
    /// (`ModifyTime`) is the one in `known` (by `href`) is taken from there,
    /// not read again.
    pub fn contacts(&self, known: &[Card]) -> Result<Vec<Card>> {
        let api = &self.ctx.client;
        let size = limits::CONTACT_SYNC_PAGE_SIZE;
        let listed = block_on(pcontacts::paginate(size, |page| pcontacts::list_contacts(api, page, size, None))).explained()?;
        let mut out = Vec::new();
        for c in listed {
            let href = format!("proton:{}", c.id);
            let version = c.modify_time.to_string();
            if let Some(k) = known.iter().find(|k| k.href == href && k.etag == version) {
                out.push(k.clone());
                continue;
            }
            match block_on(pcontacts::get_contact(api, &c.id)) {
                Ok(full) => out.push(self.card_of(&full)),
                // Deleted meanwhile (422, code 2501).
                Err(e) if protected_email_bridge::mobile::is_not_found(&e) => {}
                Err(e) => return Err(e).explained(),
            }
        }
        Ok(out)
    }

    /// A contact as Proton keeps it, as a card here. One with a part that
    /// can't be read or verified keeps only its name and addresses, and an
    /// empty `vcard`, which `save_contact` refuses to write back.
    fn card_of(&self, contact: &pcontacts::ContactFull) -> Card {
        let ring = self.ctx.user_ring();
        let parts: Option<Vec<String>> = contact.cards.iter().map(|c| pcards::read(c, &ring).ok().map(|t| t.text)).collect();
        let read = parts.and_then(|parts| {
            let vcard = merged_vcard(&parts);
            let card = carddav::parse_vcards(&vcard).into_iter().next()?;
            Some(Card { vcard, ..card })
        });
        let mut card = read.unwrap_or_else(|| Card {
            name: contact.basic.name.clone(),
            emails: contact.contact_emails.iter().map(|e| e.email.clone()).filter(|e| !e.is_empty()).collect(),
            ..Default::default()
        });
        card.href = format!("proton:{}", contact.basic.id);
        card.etag = contact.basic.modify_time.to_string();
        if card.uid.is_empty() {
            card.uid = if contact.basic.uid.is_empty() { format!("proton-{}", contact.basic.id) } else { contact.basic.uid.clone() };
        }
        card
    }

    /// The parts Proton keeps of a vCard: signed in the clear, and
    /// encrypted and signed, with the account's key.
    fn contact_cards(&self, vcard: &str) -> Result<Value> {
        let ring = self.ctx.user_ring();
        let (signed, encrypted) = split_vcard(vcard);
        let cards = [pcards::signed(&signed, &ring).explained()?, pcards::encrypted_signed(&encrypted, &ring).explained()?];
        Ok(serde_json::to_value(cards)?)
    }

    /// Adds a contact, or changes one read from Proton; returns it as saved.
    pub fn save_contact(&self, card: &Card) -> Result<Card> {
        let existing = card.href.strip_prefix("proton:").filter(|i| !i.is_empty());
        ensure!(existing.is_none() || !card.vcard.is_empty(), t!("sync-proton-pim-this-contact-couldnt-be-read-whole"));
        let vcard = carddav::to_vcard4(&carddav::write_vcard(card));
        let cards = self.contact_cards(&vcard)?;
        let saved = match existing {
            Some(id) => self.pim(reqwest::Method::PUT, &format!("/contacts/v4/contacts/{id}"), &[], Some(&json!({"Cards": cards})))?["Contact"].clone(),
            None => {
                let v = self.pim(
                    reqwest::Method::POST,
                    "/contacts/v4/contacts",
                    &[],
                    Some(&json!({"Contacts": [{"Cards": cards}], "Overwrite": 0, "Labels": 0})),
                )?;
                let answer = &v["Responses"][0]["Response"];
                ensure!(
                    answer["Code"].as_i64() == Some(1000),
                    t!("sync-proton-pim-proton-didnt-take-the-contact", message = text(&answer["Error"]))
                );
                answer["Contact"].clone()
            }
        };
        let mut out = carddav::parse_vcards(&vcard).into_iter().next().unwrap_or_else(|| card.clone());
        out.href = format!("proton:{}", text(&saved["ID"]));
        out.etag = saved["ModifyTime"].as_i64().unwrap_or_default().to_string();
        out.vcard = vcard;
        Ok(out)
    }

    pub fn delete_contact(&self, card: &Card) -> Result<()> {
        let Some(id) = card.href.strip_prefix("proton:").filter(|i| !i.is_empty()) else {
            return Ok(());
        };
        let failed = block_on(pcontacts::delete_contacts(&self.ctx.client, &[id.to_owned()])).explained()?;
        ensure!(failed.is_empty(), t!("sync-proton-pim-proton-didnt-delete-the-contact"));
        Ok(())
    }

    // ---- Calendars -----------------------------------------------------

    /// The account's calendars, as the member each is shared with.
    pub fn calendars(&self) -> Result<Vec<ProtonCalendar>> {
        let v = self.pim_get("/calendar/v1", &[])?;
        let mine: Vec<String> = self.ctx.addresses().into_iter().map(|a| a.id).collect();
        Ok(v["Calendars"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let member = c["Members"].as_array()?.iter().find(|m| mine.contains(&text(&m["AddressID"])))?;
                Some(ProtonCalendar {
                    id: text(&c["ID"]),
                    name: text(&member["Name"]),
                    color: text(&member["Color"]),
                    // Subscribed and holiday calendars (types 1 and 2) are read-only.
                    writable: c["Type"].as_i64().unwrap_or_default() == 0
                        && member["Permissions"].as_i64().unwrap_or_default() & EDIT != 0,
                    member: text(&member["ID"]),
                    address: text(&member["AddressID"]),
                })
            })
            .filter(|c| !c.id.is_empty())
            .collect())
    }

    /// A calendar's primary key, unlocked with the member address's keys
    /// (its passphrase's signature verified), and those keys.
    fn calendar_key(&self, cal: &ProtonCalendar) -> Result<(UnlockedKey, std::sync::Arc<KeyRing>)> {
        let bootstrap = block_on(pcal::get_bootstrap(&self.ctx.client, &cal.id)).explained()?;
        let member = bootstrap.member().explained()?;
        let address = self.ctx.keyring(&member.address_id).explained()?;
        let key = pkeys::unlock_calendar_key(&bootstrap, &address)
            .explained()
            .context(t!("sync-proton-pim-this-calendars-keys-couldnt-be"))?;
        Ok((key, address))
    }

    /// The events of `cal` that happen between `from` and `to`, decrypted,
    /// each series with its changed occurrences.
    pub fn events_in(&self, cal: &ProtonCalendar, from: i64, to: i64) -> Result<Vec<ProtonEvent>> {
        let (key, address) = self.calendar_key(cal)?;
        let size = limits::CALENDAR_EVENTS_PAGE_SIZE;
        let mut raw: Vec<CalendarEvent> = Vec::new();
        // Proton lists part-day and all-day events apart, each those in
        // the range and those from before that may reach into it.
        for kind in 0..4 {
            for page in 0..50 {
                let v = self.pim_get(
                    &format!("/calendar/v1/{}/events", cal.id),
                    &[
                        ("Start", from.to_string()),
                        ("End", to.to_string()),
                        ("Timezone", "UTC".into()),
                        ("Type", kind.to_string()),
                        ("Page", page.to_string()),
                        ("PageSize", size.to_string()),
                    ],
                )?;
                let found: Vec<CalendarEvent> = serde_json::from_value(v["Events"].clone()).unwrap_or_default();
                let more = found.len() == size;
                for e in found {
                    if !raw.iter().any(|r| r.id == e.id) {
                        raw.push(e);
                    }
                }
                if !more {
                    break;
                }
            }
        }
        let mut read: Vec<(String, String, Event)> = raw
            .iter()
            .filter_map(|e| {
                let version = e.extra.get("LastEditTime").and_then(Value::as_i64).unwrap_or_default().to_string();
                Some((e.id.clone(), version, event_of(e, &address, &key)?))
            })
            .collect();
        // Series first, then their changed occurrences beside them.
        read.sort_by_key(|(_, _, e)| e.recurrence_id.is_some());
        let mut out: Vec<ProtonEvent> = Vec::new();
        for (id, version, mut event) in read {
            if event.recurrence_id.is_some()
                && let Some(series) = out.iter_mut().find(|s| s.event.uid == event.uid)
            {
                event.kept.push_str(&format!("{PROTON_ID}{id}\r\n"));
                series.overrides.push(event);
                continue;
            }
            event.recurrence_id = None;
            out.push(ProtonEvent { event, overrides: Vec::new(), calendar: cal.id.clone(), id, version });
        }
        Ok(out)
    }

    /// Writes an event (a series with its changed occurrences) to `cal`,
    /// in place of `replacing` (the Proton ids of what it was). Returns the
    /// new ids: the series', and each changed occurrence's.
    pub fn save_event(&self, cal: &ProtonCalendar, replacing: &[String], series: &Event, overrides: &[Event]) -> Result<(String, Vec<String>)> {
        ensure!(cal.writable, t!("sync-proton-pim-this-proton-calendar-is-read-only"));
        ensure!(
            series.attendees.is_empty() && overrides.iter().all(|o| o.attendees.is_empty()),
            t!("sync-proton-pim-events-with-guests-are-made-in")
        );
        let (key, address) = self.calendar_key(cal)?;
        let author = self.ctx.address(&cal.address).map(|a| a.email).unwrap_or_default();
        let mut writes: Vec<Value> = replacing.iter().filter(|i| !i.is_empty()).map(|id| json!({"ID": id})).collect();
        let deletions = writes.len();
        let all: Vec<&Event> = std::iter::once(series).chain(overrides).collect();
        for e in &all {
            writes.push(json!({"Overwrite": 0, "Event": event_body(e, &address, &key, &author)?}));
        }
        let answers = self.sync_events(cal, writes)?;
        let created: Vec<String> = answers
            .iter()
            .skip(deletions)
            .map(|r| {
                let answer = &r["Response"];
                if answer["Code"].as_i64() == Some(1000) {
                    Ok(text(&answer["Event"]["ID"]))
                } else {
                    Err(anyhow!(t!("sync-proton-pim-proton-didnt-take-the-event", message = text(&answer["Error"]))))
                }
            })
            .collect::<Result<_>>()?;
        ensure!(created.len() == all.len(), t!("sync-proton-pim-proton-didnt-take-the-event", message = ""));
        // Reminders are the member's own, set apart from the event.
        for (e, id) in all.iter().zip(&created) {
            if !e.alarms.is_empty() {
                let wanted = NotificationsUpdate::Replace(notifications(e));
                block_on(pcal::put_personal(&self.ctx.client, &cal.id, id, None, &wanted)).explained()?;
            }
        }
        let (first, rest) = created.split_first().context(t!("sync-proton-pim-proton-didnt-take-the-event", message = ""))?;
        Ok((first.clone(), rest.to_vec()))
    }

    /// Deletes events (by their Proton ids) from `cal`.
    pub fn delete_events(&self, cal: &ProtonCalendar, ids: &[String]) -> Result<()> {
        let writes: Vec<Value> = ids.iter().filter(|i| !i.is_empty()).map(|id| json!({"ID": id})).collect();
        if writes.is_empty() {
            return Ok(());
        }
        // An occurrence gone with its series is as asked.
        self.sync_events(cal, writes).map(drop)
    }

    /// Proton's batch of event changes for one calendar; each answer.
    fn sync_events(&self, cal: &ProtonCalendar, events: Vec<Value>) -> Result<Vec<Value>> {
        let v = self.pim(
            reqwest::Method::PUT,
            &format!("/calendar/v1/{}/events/sync", cal.id),
            &[],
            Some(&json!({"MemberID": cal.member, "Events": events})),
        )?;
        Ok(v["Responses"].as_array().cloned().unwrap_or_default())
    }
}

/// One event's parts, decrypted and put together, with the member's
/// reminders. `None` for one whose session key or parts can't be read.
fn event_of(e: &CalendarEvent, address: &KeyRing, key: &UnlockedKey) -> Option<Event> {
    let decryptor = pkeys::EventDecryptor::new(e, address, Some(key)).ok()?;
    let mut parts: Vec<String> = e
        .shared_events
        .iter()
        .chain(&e.calendar_events)
        .chain(&e.attendees_events)
        .filter_map(|p| decryptor.decrypt_part(p).ok())
        .collect();
    if let Some(n) = &e.notifications {
        let alarms = alarm_lines(n);
        if !alarms.is_empty() {
            parts.push(format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", alarms.join("\r\n")));
        }
    }
    let mut event = ical::parse(&merged_event(&parts)).events.into_iter().next()?;
    event.source.clear();
    Some(event)
}

/// The body Proton takes for a new event: its parts, signed with the
/// member address's key, the secret one encrypted with a session key
/// wrapped to the calendar key.
fn event_body(e: &Event, address: &KeyRing, key: &UnlockedKey, author: &str) -> Result<Value> {
    let [shared_signed, shared_secret, calendar_signed] = split_event(&ical::write_event(e));
    let calendar_public = key.public_key();
    let encrypted = pkeys::encrypt_event(
        &[
            (part_type::SIGNED, shared_signed),
            (part_type::ENCRYPTED_SIGNED, shared_secret),
            (part_type::SIGNED, calendar_signed),
        ],
        address,
        pkeys::WrapKey::Calendar(&calendar_public),
        author,
    )
    .explained()?;
    let parts = encrypted.parts;
    Ok(json!({
        "Permissions": 1,
        "IsOrganizer": 1,
        "SharedKeyPacket": encrypted.shared_key_packet,
        "SharedEventContent": [parts[0], parts[1]],
        "CalendarEventContent": [parts[2]],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A contact's parts make one vCard; written, it is split again with
    /// its name and addresses in the clear and the rest secret.
    #[test]
    fn contact_cards_split_and_join_as_proton_keeps_them() {
        let signed = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Ana Lind\r\nUID:proton-web-1\r\nitem1.EMAIL;PREF=1:ana@example.org\r\nEND:VCARD\r\n".to_owned();
        let secret = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:proton-web-1\r\nTEL;TYPE=cell:+46 8 123\r\nNOTE:Met at the\r\n  conference\r\nEND:VCARD\r\n".to_owned();
        let one = merged_vcard(&[signed, secret]);
        assert_eq!(one.matches("UID:").count(), 1);
        let card = carddav::parse_vcards(&one).into_iter().next().unwrap();
        assert_eq!(card.name, "Ana Lind");
        assert_eq!(card.emails, ["ana@example.org"]);
        assert_eq!(card.phones, ["+46 8 123"]);
        assert_eq!(card.note, "Met at the conference");
        let (clear, hidden) = split_vcard(&one);
        assert!(clear.contains("FN:Ana Lind") && clear.contains("item1.EMAIL;PREF=1:ana@example.org"), "{clear}");
        assert!(!clear.contains("TEL") && !clear.contains("NOTE"), "{clear}");
        assert!(hidden.contains("TEL;TYPE=cell") && hidden.contains("UID:proton-web-1"), "{hidden}");
        assert!(!hidden.contains("FN:") && !hidden.contains("EMAIL"), "{hidden}");
    }

    /// An event's parts make one event (alarms from the member's own
    /// part); written, each property goes in the part Proton keeps it in.
    #[test]
    fn event_parts_split_and_join_as_proton_keeps_them() {
        let wrap = |body: &str| format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n{body}END:VEVENT\r\nEND:VCALENDAR\r\n");
        let parts = [
            wrap("UID:u1\r\nDTSTAMP:20261101T000000Z\r\nDTSTART;TZID=Europe/Zurich:20261124T090000\r\nDTEND;TZID=Europe/Zurich:20261124T100000\r\nRRULE:FREQ=WEEKLY;COUNT=2\r\nSEQUENCE:1\r\n"),
            wrap("UID:u1\r\nDTSTAMP:20261101T000000Z\r\nSUMMARY:Planning\r\nLOCATION:Room 4\r\n"),
            wrap("UID:u1\r\nDTSTAMP:20261101T000000Z\r\nSTATUS:CONFIRMED\r\n"),
            wrap("UID:u1\r\nDTSTAMP:20261101T000000Z\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n"),
        ];
        let text = merged_event(&parts);
        let e = ical::parse(&text).events.into_iter().next().unwrap();
        assert_eq!((e.uid.as_str(), e.summary.as_str(), e.location.as_str()), ("u1", "Planning", "Room 4"));
        assert_eq!(e.tzid, "Europe/Zurich");
        assert_eq!(e.recurrence, "FREQ=WEEKLY;COUNT=2");
        assert_eq!(e.alarms, [900]);
        let [signed, secret, calendar] = split_event(&ical::write_event(&e));
        let has = |part: &str, name: &str| inside(part, "VEVENT").iter().any(|l| property(l) == name);
        assert!(has(&signed, "DTSTART") && has(&signed, "RRULE") && !has(&signed, "SUMMARY"), "{signed}");
        assert!(has(&secret, "SUMMARY") && has(&secret, "LOCATION") && !has(&secret, "DTSTART"), "{secret}");
        assert!(has(&calendar, "UID") && !has(&calendar, "SUMMARY"), "{calendar}");
        assert!(![&signed, &secret, &calendar].iter().any(|p| p.contains("VALARM")), "alarms aren't parts");
        // They are the member's notifications, read back as alarms.
        let n = notifications(&e);
        assert_eq!((n[0].kind, n[0].trigger.as_str()), (1, "-PT15M"));
        assert_eq!(notifications(&Event { alarms: vec![-30], ..Default::default() })[0].trigger, "PT30S");
        assert_eq!(alarm_lines(&n)[2], "TRIGGER:-PT15M");
    }
}
