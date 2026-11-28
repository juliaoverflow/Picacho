//! Google's calendars and contacts through their own APIs, for a Google
//! account signed in with OAuth: the Google Calendar API
//! (`www.googleapis.com/calendar/v3`) and the People API
//! (`people.googleapis.com/v1`). What CalDAV and CardDAV are for other
//! servers, as `graph/dav.rs` is for Exchange.
//!
//! Events are read as Google keeps them (`singleEvents=false`): a
//! repeating event is its series, with its rule as RFC 5545 lines
//! (`recurrence`), and the occurrences changed on their own as separate
//! events naming the series (`recurringEventId`, `originalStartTime`);
//! those cancelled are left out of the series (EXDATE). Each is read into
//! an event here by writing it as the iCalendar text it stands for and
//! reading that, so time zones, rules and alarms are read as everywhere
//! else. Google sends invitations, updates and cancellations itself
//! (`sendUpdates`), as Exchange does.
//!
//! These helpers only talk to Google; Manage keeps the result
//! (`manage/google_pim.rs`).
pub mod tasks;

use super::rest::{Rest, escape};
use crate::sync::carddav::Card;
use crate::sync::ical::{self, Event, FreeBusy, PartStat};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub const CALENDAR_BASE: &str = "https://www.googleapis.com";
pub const PEOPLE_BASE: &str = "https://people.googleapis.com";
const CALENDAR: &str = "/calendar/v3";
/// Where an occurrence changed on its own keeps its id at Google, in
/// `Event::kept`.
pub const GOOGLE_ID: &str = "X-PICACHO-GOOGLE-ID:";
/// What a contact read shows, and what saving one writes.
const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations,biographies,addresses,birthdays,photos,nicknames,urls,metadata";
const UPDATE_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations,biographies,addresses,birthdays,nicknames,urls";

/// A calendar in the person's list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoogleCalendar {
    pub id: String,
    pub name: String,
    pub writable: bool,
    pub color: String,
    /// The account's own calendar (its id is the address).
    pub primary: bool,
    /// The reminders its events have unless they say otherwise, in
    /// seconds before.
    pub default_alarms: Vec<i64>,
}

/// An event as read: the series (or single event), its changed
/// occurrences, its calendar, id and version.
#[derive(Clone, Debug, Default)]
pub struct GoogleEvent {
    pub event: Event,
    pub overrides: Vec<Event>,
    pub calendar: String,
    pub id: String,
    pub etag: String,
}

/// A signed-in connection to Google's calendar and contacts APIs.
pub struct Session {
    calendar: Rest,
    people: Rest,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// Unix seconds as Google writes times: RFC 3339 in UTC.
pub fn rfc3339(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A Google time (`{date}` or `{dateTime, timeZone}`) as an iCalendar
/// property: `name;VALUE=DATE:…`, `name;TZID=…:…` in its zone, or UTC.
fn ical_time(name: &str, v: &Value) -> Option<String> {
    if let Some(date) = v["date"].as_str() {
        return Some(format!("{name};VALUE=DATE:{}", date.replace('-', "")));
    }
    let at = chrono::DateTime::parse_from_rfc3339(v["dateTime"].as_str()?).ok()?;
    if let Some(zone) = v["timeZone"].as_str().and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        return Some(format!("{name};TZID={zone}:{}", at.with_timezone(&zone).format("%Y%m%dT%H%M%S")));
    }
    Some(format!("{name}:{}", at.with_timezone(&chrono::Utc).format("%Y%m%dT%H%M%SZ")))
}

/// A Google time as Unix seconds, read as `ical_time` writes it.
fn unix_of(v: &Value) -> Option<i64> {
    let line = ical_time("DTSTART", v)?;
    let text = format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\n{line}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
    ical::parse(&text).events.into_iter().next().map(|e| e.start)
}

fn partstat(response: &str) -> &'static str {
    match response {
        "accepted" => "ACCEPTED",
        "declined" => "DECLINED",
        "tentative" => "TENTATIVE",
        _ => "NEEDS-ACTION",
    }
}
fn response(status: Option<PartStat>) -> &'static str {
    match status {
        Some(PartStat::Accepted) => "accepted",
        Some(PartStat::Declined) => "declined",
        Some(PartStat::Tentative) => "tentative",
        _ => "needsAction",
    }
}

/// A person (organizer or guest) as an iCalendar property.
fn person(name: &str, v: &Value, with_status: bool) -> String {
    let mut line = name.to_owned();
    let display = text(&v["displayName"]);
    if !display.is_empty() {
        line.push_str(&format!(";CN=\"{}\"", display.replace(['"', '\r', '\n'], "")));
    }
    if with_status {
        line.push_str(&format!(";PARTSTAT={}", partstat(v["responseStatus"].as_str().unwrap_or_default())));
    }
    let email = text(&v["email"]).replace(['\r', '\n'], "");
    format!("{line}:mailto:{email}")
}

/// An event as Google describes it, as an event here. `alarms` are its
/// calendar's reminders, for events that keep them (`useDefault`).
pub fn event_of(v: &Value, default_alarms: &[i64]) -> Option<Event> {
    let mut lines = vec!["BEGIN:VCALENDAR".to_owned(), "VERSION:2.0".to_owned(), "BEGIN:VEVENT".to_owned()];
    let uid = v["iCalUID"].as_str().filter(|u| !u.is_empty()).map_or_else(|| text(&v["id"]), str::to_owned);
    lines.push(format!("UID:{}", ical::escape(&uid)));
    lines.push(ical_time("DTSTART", &v["start"])?);
    lines.extend(ical_time("DTEND", &v["end"]));
    if v["originalStartTime"].is_object() {
        lines.extend(ical_time("RECURRENCE-ID", &v["originalStartTime"]));
    }
    for (property, key) in [("SUMMARY", "summary"), ("DESCRIPTION", "description"), ("LOCATION", "location")] {
        let value = text(&v[key]);
        if !value.is_empty() {
            lines.push(format!("{property}:{}", ical::escape(&value)));
        }
    }
    // Its rule and the times left out or added, as Google keeps them.
    for rule in v["recurrence"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let rule = rule.replace(['\r', '\n'], "");
        let upper = rule.to_ascii_uppercase();
        if ["RRULE", "EXDATE", "RDATE", "EXRULE"].iter().any(|p| upper.starts_with(p)) {
            lines.push(rule);
        }
    }
    if v["organizer"]["email"].is_string() {
        lines.push(person("ORGANIZER", &v["organizer"], false));
    }
    for guest in v["attendees"].as_array().into_iter().flatten() {
        if guest["email"].is_string() {
            lines.push(person("ATTENDEE", guest, true));
        }
    }
    if let Some(sequence) = v["sequence"].as_i64() {
        lines.push(format!("SEQUENCE:{sequence}"));
    }
    match v["status"].as_str() {
        Some("cancelled") => lines.push("STATUS:CANCELLED".into()),
        Some("tentative") => lines.push("STATUS:TENTATIVE".into()),
        Some("confirmed") => lines.push("STATUS:CONFIRMED".into()),
        _ => {}
    }
    if v["transparency"].as_str() == Some("transparent") {
        lines.push("TRANSP:TRANSPARENT".into());
    }
    if matches!(v["visibility"].as_str(), Some("private" | "confidential")) {
        lines.push("CLASS:PRIVATE".into());
    }
    let link = text(&v["hangoutLink"]);
    if !link.is_empty() {
        lines.push(format!("URL:{}", link.replace(['\r', '\n'], "")));
    }
    let alarms: Vec<i64> = if v["reminders"]["useDefault"].as_bool().unwrap_or(false) {
        default_alarms.to_vec()
    } else {
        v["reminders"]["overrides"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["minutes"].as_i64())
            .map(|m| m * 60)
            .collect()
    };
    for before in alarms {
        lines.extend([
            "BEGIN:VALARM".to_owned(),
            "ACTION:DISPLAY".to_owned(),
            format!("TRIGGER:-PT{}M", before / 60),
            "END:VALARM".to_owned(),
        ]);
    }
    lines.extend(["END:VEVENT".to_owned(), "END:VCALENDAR".to_owned(), String::new()]);
    let mut event = ical::parse(&lines.join("\r\n")).events.into_iter().next()?;
    // Written back as Google's JSON, never as this text.
    event.source.clear();
    Some(event)
}

/// The Google id an occurrence changed on its own was read with, if any.
pub fn occurrence_id(e: &Event) -> Option<&str> {
    e.kept.lines().find_map(|l| l.strip_prefix(GOOGLE_ID)).map(str::trim)
}

/// A time of `e` as Google takes it: a date for all-day events, else the
/// wall time in the event's zone (or UTC).
fn google_time(e: &Event, at: i64) -> Value {
    let utc = chrono::DateTime::from_timestamp(at, 0).unwrap_or_default();
    if e.all_day {
        return json!({"date": utc.format("%Y-%m-%d").to_string()});
    }
    match e.tzid.parse::<chrono_tz::Tz>() {
        Ok(zone) if !e.utc => json!({
            "dateTime": utc.with_timezone(&zone).format("%Y-%m-%dT%H:%M:%S").to_string(),
            "timeZone": zone.name(),
        }),
        _ => json!({"dateTime": rfc3339(at)}),
    }
}

/// A time a series leaves out or adds (EXDATE, RDATE), as Google keeps it.
fn rule_time(name: &str, e: &Event, at: i64) -> String {
    let utc = chrono::DateTime::from_timestamp(at, 0).unwrap_or_default();
    if e.all_day {
        return format!("{name};VALUE=DATE:{}", utc.format("%Y%m%d"));
    }
    match e.tzid.parse::<chrono_tz::Tz>() {
        Ok(zone) if !e.utc => format!("{name};TZID={}:{}", zone.name(), utc.with_timezone(&zone).format("%Y%m%dT%H%M%S")),
        _ => format!("{name}:{}", utc.format("%Y%m%dT%H%M%SZ")),
    }
}

/// An event here as Google takes it. `series`: its rule, and the times it
/// leaves out and adds, go too (not for one occurrence).
pub fn event_json(e: &Event, series: bool) -> Value {
    let end = if e.end > e.start { e.end } else { e.start + if e.all_day { 86_400 } else { 3_600 } };
    let mut v = json!({
        "summary": e.summary,
        "description": e.description,
        "location": e.location,
        "start": google_time(e, e.start),
        "end": google_time(e, end),
        "attendees": e.attendees.iter().map(|a| {
            let mut guest = json!({"email": a.email, "responseStatus": response(a.status)});
            if !a.name.is_empty() {
                guest["displayName"] = a.name.clone().into();
            }
            guest
        }).collect::<Vec<_>>(),
        "reminders": {
            "useDefault": false,
            "overrides": e.alarms.iter().filter(|b| **b >= 0).take(5).map(|b| json!({"method": "popup", "minutes": b / 60})).collect::<Vec<_>>(),
        },
    });
    match e.status.as_str() {
        "TENTATIVE" => v["status"] = "tentative".into(),
        "CONFIRMED" => v["status"] = "confirmed".into(),
        _ => {}
    }
    if series {
        let mut rules = Vec::new();
        let rule = e.recurrence.trim();
        if !rule.is_empty() {
            rules.push(if rule.to_ascii_uppercase().starts_with("RRULE:") { rule.to_owned() } else { format!("RRULE:{rule}") });
        }
        rules.extend(e.exdates.iter().map(|at| rule_time("EXDATE", e, *at)));
        rules.extend(e.rdates.iter().map(|at| rule_time("RDATE", e, *at)));
        v["recurrence"] = json!(rules);
    }
    v
}

/// The `sendUpdates` parameter: Google tells the guests, or no one.
fn send_updates(tell: bool) -> &'static str {
    if tell { "all" } else { "none" }
}

/// A person as Google describes them, as a card here.
pub fn card_of(v: &Value) -> Card {
    let resource = text(&v["resourceName"]);
    let all = |key: &str, field: &str| -> Vec<String> {
        v[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x[field].as_str())
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let first = |key: &str, field: &str| all(key, field).into_iter().next().unwrap_or_default();
    let name = Some(first("names", "displayName"))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| first("names", "unstructuredName"));
    let birthday = v["birthdays"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|b| {
            let d = &b["date"];
            let (month, day) = (d["month"].as_u64()?, d["day"].as_u64()?);
            Some(match d["year"].as_u64() {
                Some(year) if year > 0 => format!("{year:04}-{month:02}-{day:02}"),
                _ => format!("--{month:02}-{day:02}"),
            })
        })
        .unwrap_or_default();
    let photo = v["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| !p["default"].as_bool().unwrap_or(false))
        .map(|p| text(&p["url"]))
        .unwrap_or_default();
    Card {
        uid: resource.clone(),
        name,
        emails: all("emailAddresses", "value"),
        phones: all("phoneNumbers", "value"),
        org: first("organizations", "name"),
        title: first("organizations", "title"),
        note: first("biographies", "value"),
        addresses: all("addresses", "formattedValue").into_iter().map(|a| a.replace('\n', ", ")).collect(),
        birthday,
        photo,
        nicknames: all("nicknames", "value"),
        urls: all("urls", "value"),
        href: format!("google:{resource}"),
        etag: text(&v["etag"]),
        ..Default::default()
    }
}

/// A card here as Google takes a person.
pub fn person_json(c: &Card) -> Value {
    let values = |items: &[String]| items.iter().map(|x| json!({"value": x})).collect::<Vec<_>>();
    let mut v = json!({
        "names": if c.name.is_empty() { json!([]) } else { json!([{"unstructuredName": c.name}]) },
        "emailAddresses": values(&c.emails),
        "phoneNumbers": values(&c.phones),
        "organizations": if c.org.is_empty() && c.title.is_empty() { json!([]) } else { json!([{"name": c.org, "title": c.title}]) },
        "biographies": if c.note.is_empty() { json!([]) } else { json!([{"value": c.note, "contentType": "TEXT_PLAIN"}]) },
        "addresses": c.addresses.iter().map(|a| json!({"formattedValue": a, "streetAddress": a})).collect::<Vec<_>>(),
        "nicknames": values(&c.nicknames),
        "urls": values(&c.urls),
    });
    let birthday = c.birthday.trim();
    let parts: Vec<&str> = birthday.trim_start_matches("--").split('-').collect();
    let date = match parts.as_slice() {
        [y, m, d] if !birthday.starts_with("--") => {
            Some(json!({"year": y.parse::<u32>().ok(), "month": m.parse::<u32>().ok(), "day": d.parse::<u32>().ok()}))
        }
        [m, d] => Some(json!({"month": m.parse::<u32>().ok(), "day": d.parse::<u32>().ok()})),
        _ => None,
    };
    v["birthdays"] = match date {
        Some(d) => json!([{"date": d}]),
        None => json!([]),
    };
    if !c.etag.is_empty() {
        v["etag"] = c.etag.clone().into();
    }
    v
}

impl Session {
    pub fn open(calendar_base: &str, people_base: &str, token: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self {
            calendar: Rest::new(calendar_base, token, "Google Calendar", tls.clone())?,
            people: Rest::new(people_base, token, "Google Contacts", tls)?,
        })
    }

    /// Every page of a list: `path` asked again with each `pageToken`.
    fn pages(rest: &Rest, path: &str, items: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut token = String::new();
        // A server that keeps answering with another page stops somewhere.
        for _ in 0..200 {
            let sep = if path.contains('?') { '&' } else { '?' };
            let page = if token.is_empty() {
                rest.get(path)?
            } else {
                rest.get(&format!("{path}{sep}pageToken={}", escape(&token)))?
            };
            out.extend(page[items].as_array().cloned().unwrap_or_default());
            token = text(&page["nextPageToken"]);
            if token.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    /// The calendars in the person's list, theirs first, hidden ones left out.
    pub fn calendars(&self) -> Result<Vec<GoogleCalendar>> {
        let items = Self::pages(&self.calendar, &format!("{CALENDAR}/users/me/calendarList?maxResults=250"), "items")?;
        let mut out: Vec<GoogleCalendar> = items
            .iter()
            .filter(|c| !c["deleted"].as_bool().unwrap_or(false) && !c["hidden"].as_bool().unwrap_or(false))
            .map(|c| GoogleCalendar {
                id: text(&c["id"]),
                name: Some(text(&c["summaryOverride"])).filter(|s| !s.is_empty()).unwrap_or_else(|| text(&c["summary"])),
                writable: matches!(c["accessRole"].as_str(), Some("owner" | "writer")),
                color: text(&c["backgroundColor"]),
                primary: c["primary"].as_bool().unwrap_or(false),
                default_alarms: c["defaultReminders"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r["minutes"].as_i64())
                    .map(|m| m * 60)
                    .collect(),
            })
            .filter(|c| !c.id.is_empty())
            .collect();
        out.sort_by_key(|c| !c.primary);
        Ok(out)
    }

    /// The events of `calendars` that happen between `from` and `to`, each
    /// series with its changed occurrences.
    pub fn events_in(&self, calendars: &[GoogleCalendar], from: i64, to: i64) -> Result<Vec<GoogleEvent>> {
        let mut out = Vec::new();
        for cal in calendars {
            let path = format!(
                "{CALENDAR}/calendars/{}/events?singleEvents=false&showDeleted=true&maxResults=2500&timeMin={}&timeMax={}",
                escape(&cal.id),
                escape(&rfc3339(from)),
                escape(&rfc3339(to)),
            );
            out.extend(series_of(&Self::pages(&self.calendar, &path, "items")?, cal));
        }
        Ok(out)
    }

    /// Adds an event to `calendar` (`id` none), or changes it; returns its
    /// id and version. `tell`: Google tells its guests.
    pub fn save_event(&self, calendar: &str, id: Option<&str>, e: &Event, series: bool, tell: bool) -> Result<(String, String)> {
        let mut body = event_json(e, series);
        let saved = match id {
            Some(id) => self.calendar.patch(
                &format!("{CALENDAR}/calendars/{}/events/{}?sendUpdates={}", escape(calendar), escape(id), send_updates(tell)),
                &body,
            )?,
            None => {
                body["iCalUID"] = e.uid.clone().into();
                self.calendar.post(
                    &format!("{CALENDAR}/calendars/{}/events?sendUpdates={}", escape(calendar), send_updates(tell)),
                    &body,
                )?
            }
        };
        Ok((text(&saved["id"]), text(&saved["etag"])))
    }

    /// The id of the occurrence of series `id` that started at `at`.
    pub fn occurrence_at(&self, calendar: &str, id: &str, at: i64) -> Result<Option<String>> {
        let found = self.calendar.get(&format!(
            "{CALENDAR}/calendars/{}/events/{}/instances?showDeleted=true&originalStart={}",
            escape(calendar),
            escape(id),
            escape(&rfc3339(at)),
        ))?;
        if let Some(first) = found["items"].as_array().and_then(|i| i.first()) {
            return Ok(Some(text(&first["id"])).filter(|i| !i.is_empty()));
        }
        // Around it, for all-day events and zones that write it otherwise.
        let found = self.calendar.get(&format!(
            "{CALENDAR}/calendars/{}/events/{}/instances?timeMin={}&timeMax={}",
            escape(calendar),
            escape(id),
            escape(&rfc3339(at - 1)),
            escape(&rfc3339(at + 86_400)),
        ))?;
        Ok(found["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| unix_of(&i["originalStartTime"]) == Some(at))
            .map(|i| text(&i["id"])))
    }

    pub fn delete_event(&self, calendar: &str, id: &str, tell: bool) -> Result<()> {
        match self.calendar.delete(&format!(
            "{CALENDAR}/calendars/{}/events/{}?sendUpdates={}",
            escape(calendar),
            escape(id),
            send_updates(tell)
        )) {
            // Gone already (410 Gone is Google's, 404 a moved one's): as asked.
            Err(e) if super::rest::not_found(&e) || format!("{e:#}").contains("410") => Ok(()),
            other => other,
        }
    }

    /// Answers an invitation in a calendar of yours: your answer on Google's
    /// copy, told to the organizer.
    pub fn respond_event(&self, calendar: &str, id: &str, status: PartStat) -> Result<()> {
        let path = format!("{CALENDAR}/calendars/{}/events/{}", escape(calendar), escape(id));
        let mut event = self.calendar.get(&path)?;
        let mut guests = event["attendees"].as_array().cloned().unwrap_or_default();
        let me = guests
            .iter_mut()
            .find(|g| g["self"].as_bool().unwrap_or(false))
            .context(t!("sync-google-youre-not-a-guest-of-this-event"))?;
        me["responseStatus"] = response(Some(status)).into();
        event = json!({"attendees": guests});
        self.calendar.patch(&format!("{path}?sendUpdates=all"), &event)?;
        Ok(())
    }

    pub fn create_calendar(&self, name: &str) -> Result<String> {
        let made = self.calendar.post(&format!("{CALENDAR}/calendars"), &json!({"summary": name}))?;
        Ok(text(&made["id"]))
    }
    /// Renames a calendar of yours, and colours it (empty: as it is).
    pub fn update_calendar(&self, id: &str, name: &str, color: &str) -> Result<()> {
        self.calendar
            .patch(&format!("{CALENDAR}/calendars/{}", escape(id)), &json!({"summary": name}))?;
        if !color.is_empty() {
            self.calendar.patch(
                &format!("{CALENDAR}/users/me/calendarList/{}?colorRgbFormat=true", escape(id)),
                &json!({"backgroundColor": color, "foregroundColor": "#000000"}),
            )?;
        }
        Ok(())
    }
    pub fn delete_calendar(&self, id: &str) -> Result<()> {
        self.calendar.delete(&format!("{CALENDAR}/calendars/{}", escape(id)))
    }

    /// When each of `people` is busy over `from`..`to`, as Google knows.
    pub fn free_busy(&self, people: &[String], from: i64, to: i64) -> Result<Vec<FreeBusy>> {
        let answer = self.calendar.post(
            &format!("{CALENDAR}/freeBusy"),
            &json!({
                "timeMin": rfc3339(from),
                "timeMax": rfc3339(to),
                "items": people.iter().map(|p| json!({"id": p})).collect::<Vec<_>>(),
            }),
        )?;
        let at = |v: &Value| chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap_or_default()).map(|t| t.timestamp()).ok();
        Ok(people
            .iter()
            .map(|p| FreeBusy {
                attendee: p.clone(),
                busy: answer["calendars"][p.as_str()]["busy"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| Some((at(&b["start"])?, at(&b["end"])?)))
                    .collect(),
            })
            .collect())
    }

    /// Every contact of the person's.
    pub fn contacts(&self) -> Result<Vec<Card>> {
        let people = Self::pages(
            &self.people,
            &format!("/v1/people/me/connections?pageSize=1000&personFields={PERSON_FIELDS}"),
            "connections",
        )?;
        Ok(people.iter().map(card_of).filter(|c| !c.uid.is_empty()).collect())
    }
    /// Adds a contact, or changes one read from Google; returns it as saved.
    pub fn save_contact(&self, c: &Card) -> Result<Card> {
        let resource = c.href.strip_prefix("google:").filter(|r| r.starts_with("people/"));
        let saved = match resource {
            Some(resource) => self.people.patch(
                &format!("/v1/{resource}:updateContact?updatePersonFields={UPDATE_FIELDS}&personFields={PERSON_FIELDS}"),
                &person_json(c),
            )?,
            None => {
                let mut body = person_json(c);
                if let Some(o) = body.as_object_mut() {
                    o.remove("etag");
                }
                self.people.post(&format!("/v1/people:createContact?personFields={PERSON_FIELDS}"), &body)?
            }
        };
        Ok(card_of(&saved))
    }
    pub fn delete_contact(&self, c: &Card) -> Result<()> {
        let Some(resource) = c.href.strip_prefix("google:").filter(|r| r.starts_with("people/")) else {
            return Ok(());
        };
        match self.people.delete(&format!("/v1/{resource}:deleteContact")) {
            Err(e) if super::rest::not_found(&e) => Ok(()),
            other => other,
        }
    }
}

/// A calendar's events as read (`singleEvents=false`): each series with its
/// occurrences changed on their own, those cancelled left out of it.
fn series_of(items: &[Value], cal: &GoogleCalendar) -> Vec<GoogleEvent> {
    let mut out: Vec<GoogleEvent> = Vec::new();
    let mut exceptions: Vec<&Value> = Vec::new();
    for v in items {
        if v["recurringEventId"].is_string() {
            exceptions.push(v);
        } else if v["status"].as_str() != Some("cancelled")
            && let Some(event) = event_of(v, &cal.default_alarms)
        {
            out.push(GoogleEvent {
                event,
                overrides: Vec::new(),
                calendar: cal.id.clone(),
                id: text(&v["id"]),
                etag: text(&v["etag"]),
            });
        }
    }
    for v in exceptions {
        let series = text(&v["recurringEventId"]);
        let cancelled = v["status"].as_str() == Some("cancelled");
        let Some(master) = out.iter_mut().find(|s| s.id == series) else {
            // Its series is outside the range read: a single event, unless
            // cancelled.
            if !cancelled && let Some(mut event) = event_of(v, &cal.default_alarms) {
                event.recurrence_id = None;
                out.push(GoogleEvent { event, calendar: cal.id.clone(), id: text(&v["id"]), etag: text(&v["etag"]), ..Default::default() });
            }
            continue;
        };
        if cancelled {
            // A cancelled occurrence names only when it was.
            if let Some(at) = unix_of(&v["originalStartTime"])
                && !master.event.exdates.contains(&at)
            {
                master.event.exdates.push(at);
            }
            continue;
        }
        if let Some(mut occurrence) = event_of(v, &cal.default_alarms) {
            occurrence.uid = master.event.uid.clone();
            occurrence.kept.push_str(&format!("{GOOGLE_ID}{}\r\n", text(&v["id"])));
            master.overrides.push(occurrence);
        }
    }
    out
}

#[cfg(test)]
mod tests;
