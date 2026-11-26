//! Exchange calendars, contacts and the organization's directory through
//! Microsoft Graph, for an Outlook account that uses it: what CalDAV and
//! CardDAV are for other servers.
//!
//! Events are read as they happen in the range shown (`calendarView`). A
//! repeating event is kept as its series: the series' own event (with its
//! repeat rule as an RRULE, in its own time zone), the occurrences changed
//! on their own (overrides, by their original start), and those cancelled
//! (EXDATE: expected by the rule, but not at Exchange). Changing a series,
//! one occurrence or deleting one changes just that at Exchange. Times are
//! asked for in UTC. Exchange sends invitations, updates and
//! cancellations itself for events with guests.
use super::{Session, graph_time};
use crate::sync::carddav::Card;
use crate::sync::ical::{Attendee, Event, FreeBusy, PartStat};
use crate::sync::rest::escape;
use anyhow::{Context, Result};
use serde_json::{Value, json};

/// What an event read for the calendar shows.
const EVENT_SELECT: &str = "id,iCalUId,subject,body,bodyPreview,start,end,isAllDay,location,organizer,attendees,isCancelled,isReminderOn,reminderMinutesBeforeStart,categories,sensitivity,type,seriesMasterId,showAs,originalStart,originalStartTimeZone,recurrence";
/// Asks for times in UTC and text bodies.
const PREFER: &str = "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"";
/// Where an occurrence keeps its id at Exchange, in `Event::kept`.
const GRAPH_ID: &str = "X-PICACHO-GRAPH-ID:";
/// How far an occurrence may be from where its rule puts it and still be
/// that one (a daylight saving hour, with room).
const SLACK: i64 = 2 * 3600;
/// What a contact read shows.
const CONTACT_SELECT: &str = "id,displayName,emailAddresses,businessPhones,homePhones,mobilePhone,companyName,jobTitle,personalNotes,birthday,nickName,businessHomePage,changeKey";

/// A calendar: (id, name, whether it can be written, colour, the default).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphCalendar {
    pub id: String,
    pub name: String,
    pub writable: bool,
    pub color: String,
    pub default: bool,
}

/// An event as read: the event (a series' own, for a repeating one), the
/// occurrences changed on their own, its calendar, its id and version.
#[derive(Clone, Debug, Default)]
pub struct GraphEvent {
    pub event: Event,
    pub overrides: Vec<Event>,
    pub calendar: String,
    pub id: String,
    pub etag: String,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
/// A `dateTimeTimeZone` asked for in UTC, as Unix seconds.
fn utc(v: &Value) -> i64 {
    let date = v["dateTime"].as_str().unwrap_or_default();
    let date = date.split('.').next().unwrap_or(date);
    chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S").map_or(0, |d| d.and_utc().timestamp())
}
fn date_time(at: i64) -> Value {
    json!({"dateTime": graph_time(at).trim_end_matches('Z'), "timeZone": "UTC"})
}
fn status(response: &str) -> Option<PartStat> {
    Some(match response {
        "accepted" | "organizer" => PartStat::Accepted,
        "tentativelyAccepted" => PartStat::Tentative,
        "declined" => PartStat::Declined,
        "notResponded" | "none" => PartStat::NeedsAction,
        _ => return None,
    })
}
fn person(v: &Value) -> Attendee {
    Attendee {
        email: text(&v["emailAddress"]["address"]),
        name: text(&v["emailAddress"]["name"]),
        status: status(v["status"]["response"].as_str().unwrap_or_default()),
        ..Default::default()
    }
}

/// An event as Exchange describes it, as an event here.
pub fn event_of(v: &Value) -> Event {
    let all_day = v["isAllDay"].as_bool().unwrap_or(false);
    Event {
        uid: v["iCalUId"].as_str().filter(|u| !u.is_empty()).map_or_else(|| text(&v["id"]), str::to_owned),
        summary: text(&v["subject"]),
        location: text(&v["location"]["displayName"]),
        description: if v["body"]["contentType"].as_str() == Some("text") {
            text(&v["body"]["content"]).trim().to_owned()
        } else {
            text(&v["bodyPreview"])
        },
        start: utc(&v["start"]),
        end: utc(&v["end"]),
        all_day,
        utc: !all_day,
        organizer: v["organizer"]["emailAddress"]["address"].as_str().map(|_| person(&v["organizer"])),
        attendees: v["attendees"].as_array().into_iter().flatten().map(person).collect(),
        cancelled: v["isCancelled"].as_bool().unwrap_or(false),
        alarms: if v["isReminderOn"].as_bool().unwrap_or(false) {
            vec![v["reminderMinutesBeforeStart"].as_i64().unwrap_or(15) * 60]
        } else {
            Vec::new()
        },
        categories: v["categories"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
        class: match v["sensitivity"].as_str() {
            Some("private") => "PRIVATE".into(),
            Some("confidential") => "CONFIDENTIAL".into(),
            _ => String::new(),
        },
        status: if v["isCancelled"].as_bool() == Some(true) { "CANCELLED".into() } else { "CONFIRMED".into() },
        ..Default::default()
    }
}

/// An event here as Exchange takes it. `new` events may repeat (from the
/// RRULE); an occurrence or a single event is changed as it is.
pub fn event_json(e: &Event, new: bool) -> Value {
    let mut v = json!({
        "subject": e.summary,
        "body": {"contentType": "text", "content": e.description},
        "location": {"displayName": e.location},
        "isAllDay": e.all_day,
        "start": date_time(e.start),
        "end": date_time(if e.end > e.start { e.end } else { e.start + if e.all_day { 86_400 } else { 3_600 } }),
        "attendees": e.attendees.iter().map(|a| json!({
            "emailAddress": {"address": a.email, "name": a.name},
            "type": "required",
        })).collect::<Vec<_>>(),
        "categories": e.categories,
        "isReminderOn": !e.alarms.is_empty(),
    });
    if let Some(before) = e.alarms.first() {
        v["reminderMinutesBeforeStart"] = (before / 60).max(0).into();
    }
    if new && let Some(r) = recurrence(&e.recurrence, e.start) {
        v["recurrence"] = r;
    }
    v
}

/// An RRULE as Exchange's repeat pattern, where Exchange can say it: daily,
/// weekly (on days), monthly (on a day, or the nth weekday) and yearly,
/// every so often, until a date or a number of times.
pub fn recurrence(rrule: &str, start: i64) -> Option<Value> {
    if rrule.trim().is_empty() {
        return None;
    }
    let parts: std::collections::HashMap<String, String> = rrule
        .trim()
        .trim_start_matches("RRULE:")
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.to_owned()))
        .collect();
    let day_name = |d: &str| -> Option<&'static str> {
        Some(match d.trim_start_matches(|c: char| c == '-' || c == '+' || c.is_ascii_digit()) {
            "MO" => "monday",
            "TU" => "tuesday",
            "WE" => "wednesday",
            "TH" => "thursday",
            "FR" => "friday",
            "SA" => "saturday",
            "SU" => "sunday",
            _ => return None,
        })
    };
    let first = chrono::DateTime::from_timestamp(start, 0)?;
    let interval: i64 = parts.get("INTERVAL").and_then(|i| i.parse().ok()).unwrap_or(1);
    let days: Vec<&str> = parts.get("BYDAY").map(|d| d.split(',').filter_map(day_name).collect()).unwrap_or_default();
    use chrono::Datelike as _;
    let pattern = match parts.get("FREQ")?.as_str() {
        "DAILY" => json!({"type": "daily", "interval": interval}),
        "WEEKLY" => {
            let days = if days.is_empty() {
                vec![first.weekday().to_string().to_lowercase()]
            } else {
                days.iter().map(|d| d.to_string()).collect()
            };
            let days: Vec<String> = days
                .into_iter()
                .map(|d| match d.as_str() {
                    "mon" => "monday".into(),
                    "tue" => "tuesday".into(),
                    "wed" => "wednesday".into(),
                    "thu" => "thursday".into(),
                    "fri" => "friday".into(),
                    "sat" => "saturday".into(),
                    "sun" => "sunday".into(),
                    other => other.to_owned(),
                })
                .collect();
            json!({"type": "weekly", "interval": interval, "daysOfWeek": days, "firstDayOfWeek": "monday"})
        }
        "MONTHLY" => match parts.get("BYDAY") {
            Some(by) => {
                let n: i64 = by.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
                let index = match n {
                    1 => "first",
                    2 => "second",
                    3 => "third",
                    4 => "fourth",
                    -1 => "last",
                    _ => return None,
                };
                json!({"type": "relativeMonthly", "interval": interval, "daysOfWeek": [day_name(by)?], "index": index})
            }
            None => json!({"type": "absoluteMonthly", "interval": interval, "dayOfMonth": first.day()}),
        },
        "YEARLY" => json!({"type": "absoluteYearly", "interval": interval, "dayOfMonth": first.day(), "month": first.month()}),
        _ => return None,
    };
    let start_date = first.format("%Y-%m-%d").to_string();
    let range = if let Some(count) = parts.get("COUNT").and_then(|c| c.parse::<i64>().ok()) {
        json!({"type": "numbered", "startDate": start_date, "numberOfOccurrences": count})
    } else if let Some(until) = parts.get("UNTIL") {
        // The first eight characters (a date), by characters: a byte slice
        // through a non-ASCII one panicked.
        let day: String = until.chars().take(8).collect();
        let date = chrono::NaiveDate::parse_from_str(&day, "%Y%m%d").ok()?;
        json!({"type": "endDate", "startDate": start_date, "endDate": date.format("%Y-%m-%d").to_string()})
    } else {
        json!({"type": "noEnd", "startDate": start_date})
    };
    Some(json!({"pattern": pattern, "range": range}))
}

/// Exchange's repeat pattern as an RRULE (the reverse of `recurrence`).
pub fn rrule_of(r: &Value) -> Option<String> {
    let p = &r["pattern"];
    let day = |d: &Value| -> Option<&'static str> {
        Some(match d.as_str()? {
            "monday" => "MO",
            "tuesday" => "TU",
            "wednesday" => "WE",
            "thursday" => "TH",
            "friday" => "FR",
            "saturday" => "SA",
            "sunday" => "SU",
            _ => return None,
        })
    };
    let days: Vec<&str> = p["daysOfWeek"].as_array().into_iter().flatten().filter_map(day).collect();
    let index = match p["index"].as_str().unwrap_or("first") {
        "first" => 1,
        "second" => 2,
        "third" => 3,
        "fourth" => 4,
        "last" => -1,
        _ => return None,
    };
    // The nth of several days ("the first weekday") is a position in the set.
    let relative = |days: &[&str]| -> String {
        if days.len() == 1 {
            format!("BYDAY={index}{}", days[0])
        } else {
            format!("BYDAY={};BYSETPOS={index}", days.join(","))
        }
    };
    let mut rule = match p["type"].as_str()? {
        "daily" => "FREQ=DAILY".to_owned(),
        "weekly" => format!("FREQ=WEEKLY;BYDAY={}", days.join(",")),
        "absoluteMonthly" => format!("FREQ=MONTHLY;BYMONTHDAY={}", p["dayOfMonth"].as_i64()?),
        "relativeMonthly" if !days.is_empty() => format!("FREQ=MONTHLY;{}", relative(&days)),
        "absoluteYearly" => format!("FREQ=YEARLY;BYMONTH={};BYMONTHDAY={}", p["month"].as_i64()?, p["dayOfMonth"].as_i64()?),
        "relativeYearly" if !days.is_empty() => format!("FREQ=YEARLY;BYMONTH={};{}", p["month"].as_i64()?, relative(&days)),
        _ => return None,
    };
    let interval = p["interval"].as_i64().unwrap_or(1);
    if interval > 1 {
        rule.push_str(&format!(";INTERVAL={interval}"));
    }
    let range = &r["range"];
    match range["type"].as_str() {
        Some("numbered") => rule.push_str(&format!(";COUNT={}", range["numberOfOccurrences"].as_i64()?)),
        Some("endDate") => {
            let end = chrono::NaiveDate::parse_from_str(range["endDate"].as_str()?, "%Y-%m-%d").ok()?;
            rule.push_str(&format!(";UNTIL={}T235959Z", end.format("%Y%m%d")));
        }
        _ => {}
    }
    Some(rule)
}

/// The id at Exchange of an occurrence read from it (see `GRAPH_ID`).
pub fn occurrence_id(e: &Event) -> Option<&str> {
    e.kept.lines().find_map(|l| l.strip_prefix(GRAPH_ID)).map(str::trim)
}

/// A contact as Exchange describes it, as a card here.
pub fn card_of(v: &Value) -> Card {
    let id = text(&v["id"]);
    let mut phones: Vec<String> = v["mobilePhone"].as_str().filter(|p| !p.is_empty()).map(str::to_owned).into_iter().collect();
    for key in ["businessPhones", "homePhones"] {
        phones.extend(v[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
    }
    Card {
        uid: format!("graph-{id}"),
        name: text(&v["displayName"]),
        emails: v["emailAddresses"].as_array().into_iter().flatten().map(|e| text(&e["address"])).filter(|e| !e.is_empty()).collect(),
        phones,
        org: text(&v["companyName"]),
        title: text(&v["jobTitle"]),
        note: text(&v["personalNotes"]),
        birthday: v["birthday"].as_str().map(|b| b.get(..10).unwrap_or(b).to_owned()).unwrap_or_default(),
        nicknames: v["nickName"].as_str().filter(|n| !n.is_empty()).map(|n| vec![n.to_owned()]).unwrap_or_default(),
        urls: v["businessHomePage"].as_str().filter(|u| !u.is_empty()).map(|u| vec![u.to_owned()]).unwrap_or_default(),
        href: format!("graph:{id}"),
        etag: text(&v["changeKey"]),
        ..Default::default()
    }
}
/// A card here as Exchange takes a contact.
pub fn contact_json(c: &Card) -> Value {
    let mut v = json!({
        "displayName": c.name,
        "emailAddresses": c.emails.iter().map(|e| json!({"address": e, "name": c.name})).collect::<Vec<_>>(),
        "companyName": c.org,
        "jobTitle": c.title,
        "personalNotes": c.note,
        "nickName": c.nicknames.first().cloned().unwrap_or_default(),
    });
    let mut phones = c.phones.iter();
    if let Some(p) = phones.next() {
        v["mobilePhone"] = p.clone().into();
    }
    v["businessPhones"] = json!(phones.cloned().collect::<Vec<_>>());
    if !c.birthday.is_empty() {
        v["birthday"] = format!("{}T00:00:00Z", c.birthday).into();
    }
    v
}

/// An RRULE's parts in a fixed order, to compare two.
fn normal_rule(rule: &str) -> String {
    let mut parts: Vec<String> = rule
        .trim()
        .trim_start_matches("RRULE:")
        .split(';')
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("INTERVAL=1"))
        .map(|p| p.to_ascii_uppercase())
        .collect();
    parts.sort();
    parts.join(";")
}
/// A time Graph gives as `DateTimeOffset` text, in Unix seconds.
fn time_of(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp()).or_else(|| {
        chrono::NaiveDateTime::parse_from_str(s.split('.').next()?.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S")
            .ok()
            .map(|d| d.and_utc().timestamp())
    })
}
fn single(v: &Value, calendar: &str) -> GraphEvent {
    GraphEvent {
        event: event_of(v),
        overrides: Vec::new(),
        calendar: calendar.to_owned(),
        id: text(&v["id"]),
        etag: v["@odata.etag"].as_str().map(str::to_owned).unwrap_or_default(),
    }
}
/// A series from its own event and its occurrences in the range read.
fn series_of(master: &Value, instances: &[Value], calendar: &str, from: i64, to: i64) -> GraphEvent {
    let mut event = event_of(master);
    event.recurrence = rrule_of(&master["recurrence"]).unwrap_or_default();
    if let Some(zone) = master["originalStartTimeZone"].as_str().and_then(crate::sync::ical::iana_zone) {
        event.tzid = zone;
        event.utc = false;
    }
    let started = |v: &Value| time_of(&v["originalStart"]).unwrap_or_else(|| utc(&v["start"]));
    let overrides: Vec<Event> = instances
        .iter()
        .filter(|v| v["type"].as_str() == Some("exception"))
        .map(|v| {
            let mut o = event_of(v);
            o.uid = event.uid.clone();
            o.recurrence_id = Some(started(v));
            o.tzid = event.tzid.clone();
            o.utc = event.utc;
            o.kept = format!("{GRAPH_ID}{}\r\n", text(&v["id"]));
            o
        })
        .collect();
    // Expected by the rule in the range, but not at Exchange: cancelled.
    let present: Vec<i64> = instances.iter().map(started).collect();
    if !event.recurrence.is_empty() {
        event.exdates = crate::sync::ical::occurrences(&event, &[], from, to)
            .into_iter()
            .map(|o| o.start)
            .filter(|at| !present.iter().any(|p| (p - at).abs() < SLACK))
            .collect();
    }
    GraphEvent {
        event,
        overrides,
        calendar: calendar.to_owned(),
        id: text(&master["id"]),
        etag: master["@odata.etag"].as_str().map(str::to_owned).unwrap_or_default(),
    }
}

impl Session {
    /// The calendars.
    pub fn calendars(&self) -> Result<Vec<GraphCalendar>> {
        Ok(self
            .all_pages("/me/calendars?$top=250&$select=id,name,canEdit,hexColor,isDefaultCalendar", 1000)?
            .iter()
            .map(|c| GraphCalendar {
                id: text(&c["id"]),
                name: text(&c["name"]),
                writable: c["canEdit"].as_bool().unwrap_or(true),
                color: text(&c["hexColor"]),
                default: c["isDefaultCalendar"].as_bool().unwrap_or(false),
            })
            .collect())
    }
    /// Every event in each calendar between `from` and `to`: single ones,
    /// and repeating ones as their series (see the module comment).
    pub fn events_in(&self, calendars: &[GraphCalendar], from: i64, to: i64) -> Result<Vec<GraphEvent>> {
        let mut out = Vec::new();
        for c in calendars {
            let path = format!(
                "/me/calendars/{}/calendarView?startDateTime={}&endDateTime={}&$top=250&$select={EVENT_SELECT}",
                escape(&c.id),
                escape(&graph_time(from)),
                escape(&graph_time(to))
            );
            let view = self.all_pages_with(&path, 20_000, &[("Prefer", PREFER)])?;
            let mut series: Vec<(String, Vec<Value>)> = Vec::new();
            for v in view {
                let master = text(&v["seriesMasterId"]);
                if master.is_empty() || v["type"].as_str() == Some("singleInstance") {
                    out.push(single(&v, &c.id));
                    continue;
                }
                match series.iter_mut().find(|(m, _)| *m == master) {
                    Some((_, list)) => list.push(v),
                    None => series.push((master, vec![v])),
                }
            }
            let requests: Vec<Value> = series
                .iter()
                .map(|(m, _)| {
                    json!({"method": "GET", "url": format!("/me/events/{}?$select={EVENT_SELECT}", escape(m)),
                           "headers": {"Prefer": PREFER}})
                })
                .collect();
            let masters = self.batch(requests)?;
            for ((_, instances), (status, master)) in series.into_iter().zip(masters) {
                match (status, master["recurrence"].is_object()) {
                    (200, true) => out.push(series_of(&master, &instances, &c.id, from, to)),
                    // A series Exchange won't show (a shared calendar's):
                    // its occurrences on their own.
                    _ => out.extend(instances.iter().map(|v| single(v, &c.id))),
                }
            }
        }
        Ok(out)
    }
    /// A series' occurrence that started (by its rule) at `at`, where it
    /// still happens: its id.
    pub fn occurrence_at(&self, master: &str, at: i64) -> Result<Option<String>> {
        let path = format!(
            "/me/events/{}/instances?startDateTime={}&endDateTime={}&$select=id,originalStart,start",
            escape(master),
            escape(&graph_time(at - 86_400)),
            escape(&graph_time(at + 86_400))
        );
        Ok(self
            .all_pages_with(&path, 50, &[("Prefer", PREFER)])?
            .iter()
            .find(|v| (time_of(&v["originalStart"]).unwrap_or_else(|| utc(&v["start"])) - at).abs() < SLACK)
            .map(|v| text(&v["id"])))
    }
    /// A series' rule as Exchange has it now (an RRULE).
    pub fn series_rule(&self, master: &str) -> Result<Option<String>> {
        let v = self.rest.get(&Self::p(&format!("/me/events/{}?$select=recurrence", escape(master))))?;
        Ok(rrule_of(&v["recurrence"]))
    }
    /// Saves an event: a new one in `calendar`, or changes to `id`. Its id.
    /// A series' rule is sent where it changed (sending it again would
    /// undo the occurrences changed on their own).
    pub fn save_event(&self, calendar: &str, id: Option<&str>, e: &Event) -> Result<String> {
        let v = match id {
            Some(id) => {
                let rule_changed = !e.recurrence.is_empty()
                    && self.series_rule(id)?.as_deref().map(normal_rule) != Some(normal_rule(&e.recurrence));
                self.rest.patch(&Self::p(&format!("/me/events/{}", escape(id))), &event_json(e, rule_changed))?
            }
            None => self.rest.post(&Self::p(&format!("/me/calendars/{}/events", escape(calendar))), &event_json(e, true))?,
        };
        Ok(v["id"].as_str().map(str::to_owned).or_else(|| id.map(str::to_owned)).unwrap_or_default())
    }
    pub fn delete_event(&self, id: &str) -> Result<()> {
        match self.rest.delete(&Self::p(&format!("/me/events/{}", escape(id)))) {
            Err(e) if crate::sync::rest::not_found(&e) => Ok(()),
            other => other,
        }
    }
    /// Answers an invitation in the calendar (accepted, maybe, declined),
    /// without Exchange sending the organizer a reply of its own.
    pub fn respond_event(&self, id: &str, status: PartStat) -> Result<()> {
        let action = match status {
            PartStat::Accepted => "accept",
            PartStat::Tentative => "tentativelyAccept",
            PartStat::Declined => "decline",
            _ => return Ok(()),
        };
        self.rest.post(&Self::p(&format!("/me/events/{}/{action}", escape(id))), &json!({"sendResponse": false}))?;
        Ok(())
    }
    pub fn create_calendar(&self, name: &str, color: &str) -> Result<String> {
        let mut body = json!({"name": name});
        if !color.is_empty() {
            body["hexColor"] = color.into();
        }
        let v = self.rest.post(&Self::p("/me/calendars"), &body)?;
        Ok(text(&v["id"]))
    }
    pub fn update_calendar(&self, id: &str, name: &str, color: &str) -> Result<()> {
        let mut body = json!({"name": name});
        if !color.is_empty() {
            body["hexColor"] = color.into();
        }
        self.rest.patch(&Self::p(&format!("/me/calendars/{}", escape(id))), &body)?;
        Ok(())
    }
    pub fn delete_calendar(&self, id: &str) -> Result<()> {
        self.rest.delete(&Self::p(&format!("/me/calendars/{}", escape(id))))
    }
    /// When people are busy (`getSchedule`), from their calendars at
    /// Exchange.
    pub fn free_busy(&self, people: &[String], from: i64, to: i64) -> Result<Vec<FreeBusy>> {
        let v = self.rest.post(
            &Self::p("/me/calendar/getSchedule"),
            &json!({"schedules": people, "startTime": date_time(from), "endTime": date_time(to), "availabilityViewInterval": 30}),
        )?;
        Ok(v["value"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| FreeBusy {
                attendee: text(&s["scheduleId"]),
                busy: s["scheduleItems"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| i["status"].as_str() != Some("free"))
                    .map(|i| (utc(&i["start"]), utc(&i["end"])))
                    .collect(),
            })
            .collect())
    }

    /// The contacts in the default folder.
    pub fn contacts(&self) -> Result<Vec<Card>> {
        Ok(self
            .all_pages(&format!("/me/contacts?$top=500&$select={CONTACT_SELECT}"), 50_000)?
            .iter()
            .map(card_of)
            .collect())
    }
    /// Saves a contact: a new one, or changes to the one it came from.
    pub fn save_contact(&self, c: &Card) -> Result<Card> {
        let v = match c.href.strip_prefix("graph:") {
            Some(id) => self.rest.patch(&Self::p(&format!("/me/contacts/{}", escape(id))), &contact_json(c))?,
            None => self.rest.post(&Self::p("/me/contacts"), &contact_json(c))?,
        };
        let mut saved = card_of(&v);
        if saved.href == "graph:" {
            saved = Card { href: c.href.clone(), ..c.clone() };
        }
        Ok(saved)
    }
    pub fn delete_contact(&self, c: &Card) -> Result<()> {
        let Some(id) = c.href.strip_prefix("graph:") else { return Ok(()) };
        match self.rest.delete(&Self::p(&format!("/me/contacts/{}", escape(id)))) {
            Err(e) if crate::sync::rest::not_found(&e) => Ok(()),
            other => other,
        }
    }
    /// People whose name or address has `text`: those you work with and,
    /// for a work or school account, the organization's directory (People
    /// search). As cards, not saved anywhere.
    pub fn search_directory(&self, text_: &str, most: usize) -> Result<Vec<Card>> {
        let path = format!("/me/people?$search={}&$top={most}", escape(&format!("\"{}\"", text_.replace('"', ""))));
        let mut out: Vec<Card> = Vec::new();
        for p in self.all_pages(&path, most)? {
            let email = p["scoredEmailAddresses"][0]["address"].as_str().unwrap_or_default().to_owned();
            if email.is_empty() || out.iter().any(|c| c.emails.iter().any(|e| e.eq_ignore_ascii_case(&email))) {
                continue;
            }
            let mut phones: Vec<String> = Vec::new();
            for phone in p["phones"].as_array().into_iter().flatten() {
                if let Some(n) = phone["number"].as_str().filter(|n| !n.is_empty()) {
                    phones.push(n.to_owned());
                }
            }
            out.push(Card {
                uid: format!("graph-person-{}", text(&p["id"])),
                name: text(&p["displayName"]),
                emails: vec![email],
                phones,
                org: text(&p["companyName"]),
                title: text(&p["jobTitle"]),
                href: format!("graph-directory:{}", text(&p["id"])),
                ..Default::default()
            });
        }
        out.truncate(most);
        Ok(out)
    }

    /// As `all_pages`, sending `headers` with each page's request.
    pub fn all_pages_with(&self, first: &str, most: usize, headers: &[(&str, &str)]) -> Result<Vec<Value>> {
        /// As `all_pages`: no page asked for twice, nor without end.
        const MAX_PAGES: usize = 1000;
        let mut out = Vec::new();
        let mut next = Some(Self::p(first));
        let mut seen = std::collections::HashSet::new();
        while let Some(path) = next.take() {
            if seen.len() >= MAX_PAGES || !seen.insert(path.clone()) {
                break;
            }
            let mut builder = self.rest.request(reqwest::Method::GET, &path)?;
            for (k, v) in headers {
                builder = builder.header(*k, *v);
            }
            let response = self.rest.send(builder)?;
            // Read within the limit any answer has (`rest`), not without end.
            let v: Value = crate::sync::rest::bounded_json(response)
                .context("Microsoft 365 sent something other than JSON")?;
            let page = v["value"].as_array().cloned().unwrap_or_default();
            let empty = page.is_empty();
            out.extend(page);
            if out.len() >= most {
                out.truncate(most);
                break;
            }
            if empty {
                break;
            }
            next = v["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sync::graph::tests::Fake;
    use crate::sync::rest::test_server::{Answer, Request, json as answer};

    /// The fake's calendars, events, contacts and people.
    pub(crate) fn respond(f: &mut Fake, r: &Request, parts: &[&str], params: &[(String, String)], _base: &str) -> Answer {
        let param = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let ok = |v: Value| answer(200, v);
        let missing = || answer(404, json!({"error": {"code": "ErrorItemNotFound", "message": "Not found"}}));
        match (r.method.as_str(), parts) {
            ("GET", ["me", "calendars"]) => ok(json!({"value": f.calendars})),
            ("POST", ["me", "calendars"]) => {
                let mut c = r.json();
                c["id"] = format!("cal{}", f.calendars.len() + 1).into();
                c["canEdit"] = true.into();
                f.calendars.push(c.clone());
                ok(c)
            }
            ("PATCH", ["me", "calendars", id]) => {
                let v = r.json();
                let Some(c) = f.calendars.iter_mut().find(|c| c["id"] == *id) else { return missing() };
                c["name"] = v["name"].clone();
                ok(c.clone())
            }
            ("DELETE", ["me", "calendars", id]) => {
                f.calendars.retain(|c| c["id"] != *id);
                answer(204, Value::Null)
            }
            ("GET", ["me", "calendars", id, "calendarView"]) => {
                assert!(param("startDateTime").is_some() && param("endDateTime").is_some());
                ok(json!({"value": f.events.iter()
                    .filter(|e| e["calendar"] == *id && e["type"] != "seriesMaster")
                    .cloned().collect::<Vec<_>>()}))
            }
            ("GET", ["me", "events", id]) => match f.events.iter().find(|e| e["id"] == *id) {
                Some(e) => ok(e.clone()),
                None => missing(),
            },
            ("GET", ["me", "events", id, "instances"]) => ok(json!({"value": f.events.iter()
                .filter(|e| e["seriesMasterId"] == *id).cloned().collect::<Vec<_>>()})),
            ("POST", ["me", "calendars", id, "events"]) => {
                let mut e = r.json();
                e["id"] = format!("ev{}", f.events.len() + 1).into();
                e["iCalUId"] = format!("uid-{}", f.events.len() + 1).into();
                e["calendar"] = (*id).into();
                e["@odata.etag"] = "W/\"1\"".into();
                f.events.push(e.clone());
                ok(e)
            }
            ("PATCH", ["me", "events", id]) => {
                let v = r.json();
                let Some(e) = f.events.iter_mut().find(|e| e["id"] == *id) else { return missing() };
                for (k, value) in v.as_object().unwrap() {
                    e[k] = value.clone();
                }
                e["@odata.etag"] = "W/\"2\"".into();
                ok(e.clone())
            }
            ("DELETE", ["me", "events", id]) => {
                let before = f.events.len();
                f.events.retain(|e| e["id"] != *id);
                if before == f.events.len() { missing() } else { answer(204, Value::Null) }
            }
            ("POST", ["me", "calendar", "getSchedule"]) => ok(json!({"value": r.json()["schedules"].as_array().unwrap().iter().map(|s| json!({
                "scheduleId": s, "scheduleItems": [{"status": "busy", "start": {"dateTime": "2026-12-01T10:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-12-01T11:00:00.0000000", "timeZone": "UTC"}}]
            })).collect::<Vec<_>>()})),
            ("GET", ["me", "contacts"]) => ok(json!({"value": f.contacts})),
            ("POST", ["me", "contacts"]) => {
                let mut c = r.json();
                c["id"] = format!("ct{}", f.contacts.len() + 1).into();
                c["changeKey"] = "1".into();
                f.contacts.push(c.clone());
                ok(c)
            }
            ("PATCH", ["me", "contacts", id]) => {
                let v = r.json();
                let Some(c) = f.contacts.iter_mut().find(|c| c["id"] == *id) else { return missing() };
                for (k, value) in v.as_object().unwrap() {
                    c[k] = value.clone();
                }
                c["changeKey"] = "2".into();
                ok(c.clone())
            }
            ("DELETE", ["me", "contacts", id]) => {
                f.contacts.retain(|c| c["id"] != *id);
                answer(204, Value::Null)
            }
            ("GET", ["me", "people"]) => {
                let q = param("$search").unwrap_or_default().trim_matches('"').to_lowercase();
                ok(json!({"value": f.people.iter().filter(|p| {
                    p["displayName"].as_str().unwrap_or_default().to_lowercase().contains(&q)
                }).cloned().collect::<Vec<_>>()}))
            }
            _ => answer(400, json!({"error": {"code": "BadRequest", "message": format!("unknown {} {}", r.method, r.path)}})),
        }
    }

    #[test]
    fn exchange_repeats_become_rrules() {
        let weekly = json!({"pattern": {"type": "weekly", "interval": 2, "daysOfWeek": ["monday", "friday"]},
                            "range": {"type": "numbered", "numberOfOccurrences": 6}});
        assert_eq!(rrule_of(&weekly).as_deref(), Some("FREQ=WEEKLY;BYDAY=MO,FR;INTERVAL=2;COUNT=6"));
        let weekday = json!({"pattern": {"type": "relativeMonthly", "interval": 1, "index": "last",
                             "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"]},
                             "range": {"type": "endDate", "endDate": "2027-03-31"}});
        assert_eq!(rrule_of(&weekday).as_deref(), Some("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;UNTIL=20270331T235959Z"));
        let yearly = json!({"pattern": {"type": "absoluteYearly", "interval": 1, "month": 4, "dayOfMonth": 2}, "range": {"type": "noEnd"}});
        assert_eq!(rrule_of(&yearly).as_deref(), Some("FREQ=YEARLY;BYMONTH=4;BYMONTHDAY=2"));
        // There and back.
        let start = chrono::DateTime::parse_from_rfc3339("2026-12-01T10:00:00Z").unwrap().timestamp();
        let rule = "FREQ=WEEKLY;BYDAY=TU,TH;COUNT=8";
        assert_eq!(normal_rule(&rrule_of(&recurrence(rule, start).unwrap()).unwrap()), normal_rule(rule));
    }

    #[test]
    fn rrules_become_exchange_repeats() {
        let start = chrono::DateTime::parse_from_rfc3339("2026-12-01T10:00:00Z").unwrap().timestamp();
        let weekly = recurrence("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10", start).unwrap();
        assert_eq!(weekly["pattern"]["type"], "weekly");
        assert_eq!(weekly["pattern"]["interval"], 2);
        assert_eq!(weekly["pattern"]["daysOfWeek"], json!(["monday", "wednesday"]));
        assert_eq!(weekly["range"], json!({"type": "numbered", "startDate": "2026-12-01", "numberOfOccurrences": 10}));
        let monthly = recurrence("FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20270601T000000Z", start).unwrap();
        assert_eq!(monthly["pattern"]["index"], "last");
        assert_eq!(monthly["range"]["endDate"], "2027-06-01");
        assert_eq!(recurrence("FREQ=WEEKLY", start).unwrap()["pattern"]["daysOfWeek"], json!(["tuesday"]));
        assert!(recurrence("FREQ=HOURLY", start).is_none());
        assert!(recurrence("", start).is_none());
    }

    #[test]
    fn events_and_contacts_round_trip() {
        let e = Event {
            summary: "Review".into(),
            location: "Room 4".into(),
            description: "Bring notes".into(),
            start: 1_796_000_000,
            end: 1_796_003_600,
            attendees: vec![Attendee { email: "bo@example.se".into(), name: "Bo".into(), ..Default::default() }],
            alarms: vec![900],
            ..Default::default()
        };
        let mut v = event_json(&e, true);
        v["id"] = "ev1".into();
        v["iCalUId"] = "uid-1".into();
        v["attendees"][0]["status"] = json!({"response": "accepted"});
        let back = event_of(&v);
        assert_eq!((back.summary.as_str(), back.location.as_str(), back.description.as_str()), ("Review", "Room 4", "Bring notes"));
        assert_eq!((back.start, back.end, back.alarms.as_slice()), (e.start, e.end, &[900][..]));
        assert_eq!(back.attendees[0].status, Some(PartStat::Accepted));
        assert_eq!(back.uid, "uid-1");
        let card = Card {
            name: "Ana Lima".into(),
            emails: vec!["ana@example.org".into()],
            phones: vec!["+1 555 0100".into(), "+1 555 0101".into()],
            org: "Example".into(),
            birthday: "1990-04-02".into(),
            ..Default::default()
        };
        let mut v = contact_json(&card);
        v["id"] = "ct1".into();
        let back = card_of(&v);
        assert_eq!((back.name.as_str(), back.emails.as_slice(), back.phones.len()), ("Ana Lima", &["ana@example.org".to_owned()][..], 2));
        assert_eq!((back.birthday.as_str(), back.href.as_str()), ("1990-04-02", "graph:ct1"));
    }
}
