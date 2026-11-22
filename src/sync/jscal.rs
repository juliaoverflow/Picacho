//! Calendars and contacts over JMAP: JMAP for Calendars (JSCalendar
//! events, RFC 8984) and JMAP for Contacts (RFC 9610, JSContact cards, RFC
//! 9553), turned into and from the same events and cards CalDAV and
//! CardDAV give (`ical::Event`, `carddav::Card`), so the rest of Picacho
//! treats them alike.
//!
//! What Picacho doesn't show is kept: an event or card read from the server
//! keeps its JSON, and a change patches only the properties Picacho edits.
use crate::sync::{carddav::Card, ical, jmap::Session};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde_json::{Map, Value, json};

// Calendars.

/// A calendar on a JMAP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JmapCalendar {
    pub id: String,
    pub name: String,
    pub color: String,
    pub writable: bool,
}

/// The calendars of the sign-in's calendar account.
pub fn calendars(s: &Session) -> Result<Vec<JmapCalendar>> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let v = s.call_one(
        "Calendar/get",
        json!({ "accountId": account, "ids": null }),
    )?;
    Ok(v["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(JmapCalendar {
                id: c["id"].as_str()?.to_owned(),
                name: c["name"].as_str().unwrap_or("Calendar").to_owned(),
                color: c["color"].as_str().unwrap_or_default().to_owned(),
                // Without rights listed, assume it can be written.
                writable: c["myRights"]["mayWriteAll"].as_bool().unwrap_or(true)
                    || c["myRights"]["mayWriteOwn"].as_bool().unwrap_or(false),
            })
        })
        .collect())
}

/// An event as kept here, with where it is on the JMAP server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JmapEvent {
    pub id: String,
    pub calendar: String,
    pub event: ical::Event,
    pub overrides: Vec<ical::Event>,
    /// The event's JSON as the server gave it.
    pub json: Value,
}

/// A JSCalendar duration (`PT1H30M`, `P1D`) in seconds.
fn duration(text: &str) -> Option<i64> {
    let text = text.strip_prefix('P')?;
    let (days, time) = match text.split_once('T') {
        Some((d, t)) => (d, t),
        None => (text, ""),
    };
    let mut total = 0i64;
    let mut number = String::new();
    for c in days.chars() {
        if c.is_ascii_digit() {
            number.push(c);
        } else {
            let n: i64 = number.parse().ok()?;
            number.clear();
            total += match c {
                'W' => n * 7 * 86_400,
                'D' => n * 86_400,
                _ => return None,
            };
        }
    }
    for c in time.chars() {
        if c.is_ascii_digit() {
            number.push(c);
        } else {
            let n: i64 = number.parse().ok()?;
            number.clear();
            total += match c {
                'H' => n * 3600,
                'M' => n * 60,
                'S' => n,
                _ => return None,
            };
        }
    }
    Some(total)
}

/// Seconds as a JSCalendar duration.
fn write_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let mut out = String::from("P");
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if rest > 0 || days == 0 {
        out.push('T');
        let (h, m, s) = (rest / 3600, rest % 3600 / 60, rest % 60);
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 || (h == 0 && m == 0) {
            out.push_str(&format!("{s}S"));
        }
    }
    out
}

/// A JSCalendar LocalDateTime in `zone` (or UTC, or floating as UTC) as a
/// Unix time.
fn local_to_utc(local: &str, zone: Option<&str>) -> Option<i64> {
    let naive = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M:%S").ok()?;
    match zone.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        Some(tz) => tz.from_local_datetime(&naive).earliest().map(|t| t.timestamp()),
        None => Some(Utc.from_utc_datetime(&naive).timestamp()),
    }
}

/// A Unix time as a LocalDateTime in `zone` (UTC where there's none).
fn utc_to_local(t: i64, zone: &str) -> String {
    let at = DateTime::from_timestamp(t, 0).unwrap_or_default();
    match zone.parse::<chrono_tz::Tz>() {
        Ok(tz) => at.with_timezone(&tz).format("%Y-%m-%dT%H:%M:%S").to_string(),
        Err(_) => at.format("%Y-%m-%dT%H:%M:%S").to_string(),
    }
}

/// JSCalendar's recurrence rule (an object) as an RRULE, for the
/// properties the RRULE has.
fn rrule(rule: &Value) -> Option<String> {
    let freq = rule["frequency"].as_str()?.to_ascii_uppercase();
    let mut parts = vec![format!("FREQ={freq}")];
    if let Some(n) = rule["interval"].as_u64().filter(|n| *n > 1) {
        parts.push(format!("INTERVAL={n}"));
    }
    if let Some(n) = rule["count"].as_u64() {
        parts.push(format!("COUNT={n}"));
    }
    if let Some(until) = rule["until"].as_str()
        && let Some(t) = local_to_utc(until, None)
    {
        parts.push(format!(
            "UNTIL={}",
            DateTime::from_timestamp(t, 0)?.format("%Y%m%dT%H%M%SZ")
        ));
    }
    let days: Vec<String> = rule["byDay"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let day = d["day"].as_str()?.to_ascii_uppercase();
            Some(match d["nthOfPeriod"].as_i64() {
                Some(n) => format!("{n}{day}"),
                None => day,
            })
        })
        .collect();
    if !days.is_empty() {
        parts.push(format!("BYDAY={}", days.join(",")));
    }
    for (key, name) in [
        ("byMonthDay", "BYMONTHDAY"),
        ("byYearDay", "BYYEARDAY"),
        ("byWeekNo", "BYWEEKNO"),
        ("byHour", "BYHOUR"),
        ("byMinute", "BYMINUTE"),
        ("bySecond", "BYSECOND"),
        ("bySetPosition", "BYSETPOS"),
    ] {
        let values: Vec<String> = rule[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_i64().map(|n| n.to_string()))
            .collect();
        if !values.is_empty() {
            parts.push(format!("{name}={}", values.join(",")));
        }
    }
    let months: Vec<String> = rule["byMonth"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().map(str::to_owned))
        .collect();
    if !months.is_empty() {
        parts.push(format!("BYMONTH={}", months.join(",")));
    }
    Some(parts.join(";"))
}

/// An RRULE as JSCalendar's recurrence rule.
fn write_rrule(text: &str) -> Option<Value> {
    let mut rule = Map::new();
    rule.insert("@type".into(), json!("RecurrenceRule"));
    for part in text.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => {
                rule.insert("frequency".into(), json!(v.to_ascii_lowercase()));
            }
            "INTERVAL" => {
                rule.insert("interval".into(), json!(v.parse::<u64>().ok()?));
            }
            "COUNT" => {
                rule.insert("count".into(), json!(v.parse::<u64>().ok()?));
            }
            "UNTIL" => {
                let t = NaiveDateTime::parse_from_str(v.trim_end_matches('Z'), "%Y%m%dT%H%M%S")
                    .ok()
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(v, "%Y%m%d")
                            .ok()?
                            .and_hms_opt(23, 59, 59)
                    })?;
                rule.insert("until".into(), json!(t.format("%Y-%m-%dT%H:%M:%S").to_string()));
            }
            "BYDAY" => {
                let days: Vec<Value> = v
                    .split(',')
                    .filter_map(|d| {
                        let split = d.len().checked_sub(2)?;
                        let (n, day) = d.split_at(split);
                        let mut o = json!({ "@type": "NDay", "day": day.to_ascii_lowercase() });
                        if let Ok(n) = n.parse::<i64>() {
                            o["nthOfPeriod"] = json!(n);
                        }
                        Some(o)
                    })
                    .collect();
                rule.insert("byDay".into(), Value::Array(days));
            }
            "BYMONTH" => {
                rule.insert(
                    "byMonth".into(),
                    json!(v.split(',').collect::<Vec<_>>()),
                );
            }
            k => {
                let key = match k {
                    "BYMONTHDAY" => "byMonthDay",
                    "BYYEARDAY" => "byYearDay",
                    "BYWEEKNO" => "byWeekNo",
                    "BYHOUR" => "byHour",
                    "BYMINUTE" => "byMinute",
                    "BYSECOND" => "bySecond",
                    "BYSETPOS" => "bySetPosition",
                    _ => continue,
                };
                let values: Vec<i64> = v.split(',').filter_map(|n| n.parse().ok()).collect();
                rule.insert(key.into(), json!(values));
            }
        }
    }
    Some(Value::Object(rule))
}

/// A JSCalendar participant's address (`sendTo.imip`, `mailto:` stripped).
fn participant_email(p: &Value) -> String {
    p["sendTo"]["imip"]
        .as_str()
        .or(p["email"].as_str())
        .unwrap_or_default()
        .trim_start_matches("mailto:")
        .to_owned()
}

/// A JSCalendar Event (RFC 8984) as an event.
pub fn from_jscalendar(v: &Value) -> Option<ical::Event> {
    let zone = v["timeZone"].as_str().filter(|z| !z.is_empty());
    let all_day = v["showWithoutTime"].as_bool().unwrap_or(false);
    let start = local_to_utc(v["start"].as_str()?, if all_day { None } else { zone })?;
    let length = v["duration"].as_str().and_then(duration).unwrap_or(if all_day { 86_400 } else { 0 });
    let mut e = ical::Event {
        uid: v["uid"].as_str().unwrap_or_default().to_owned(),
        summary: v["title"].as_str().unwrap_or_default().to_owned(),
        description: v["description"].as_str().unwrap_or_default().to_owned(),
        start,
        end: start + length,
        all_day,
        sequence: v["sequence"].as_u64().unwrap_or(0) as u32,
        cancelled: v["status"].as_str() == Some("cancelled"),
        tzid: if all_day { String::new() } else { zone.unwrap_or_default().to_owned() },
        utc: zone == Some("Etc/UTC") || zone == Some("UTC"),
        ..Default::default()
    };
    e.location = v["locations"]
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(_, l)| l["name"].as_str().map(str::to_owned))
        .unwrap_or_default();
    if let Some(rule) = v["recurrenceRules"].as_array().and_then(|r| r.first()) {
        e.recurrence = rrule(rule).unwrap_or_default();
    }
    for (_, p) in v["participants"].as_object().into_iter().flatten() {
        let roles = &p["roles"];
        let person = ical::Attendee {
            email: participant_email(p),
            name: p["name"].as_str().unwrap_or_default().to_owned(),
            status: p["participationStatus"]
                .as_str()
                .map(ical::PartStat::parse),
        };
        if person.email.is_empty() {
            continue;
        }
        if roles["owner"].as_bool().unwrap_or(false) {
            e.organizer = Some(ical::Attendee { status: None, ..person.clone() });
        }
        if roles["attendee"].as_bool().unwrap_or(false) {
            e.attendees.push(person);
        }
    }
    Some(e)
}

/// The JSCalendar event's own occurrences changed or left out
/// (`recurrenceOverrides`), as events and excluded start times.
fn overrides(v: &Value, series: &ical::Event) -> (Vec<ical::Event>, Vec<i64>) {
    let zone = if series.tzid.is_empty() { None } else { Some(series.tzid.as_str()) };
    let mut changed = Vec::new();
    let mut excluded = Vec::new();
    for (at, patch) in v["recurrenceOverrides"].as_object().into_iter().flatten() {
        let Some(when) = local_to_utc(at, zone) else { continue };
        if patch["excluded"].as_bool().unwrap_or(false) {
            excluded.push(when);
            continue;
        }
        // The series with the patch's properties.
        let mut whole = v.clone();
        if let (Some(w), Some(p)) = (whole.as_object_mut(), patch.as_object()) {
            w.remove("recurrenceRules");
            w.remove("recurrenceOverrides");
            for (k, val) in p {
                // Patches name properties by path; top-level ones suffice.
                if !k.contains('/') {
                    w.insert(k.clone(), val.clone());
                }
            }
            if !p.contains_key("start") {
                w.insert("start".into(), json!(at));
            }
        }
        if let Some(mut e) = from_jscalendar(&whole) {
            e.recurrence_id = Some(when);
            e.recurrence.clear();
            changed.push(e);
        }
    }
    (changed, excluded)
}

/// An event as JSCalendar properties (those Picacho edits), to create it
/// or to patch the server's copy.
pub fn to_jscalendar(e: &ical::Event, overrides: &[ical::Event]) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("@type".into(), json!("Event"));
    if !e.uid.is_empty() {
        p.insert("uid".into(), json!(e.uid));
    }
    p.insert("title".into(), json!(e.summary));
    p.insert("description".into(), json!(e.description));
    p.insert("showWithoutTime".into(), json!(e.all_day));
    let zone = if e.all_day || e.tzid.is_empty() {
        if e.utc || !e.all_day { "Etc/UTC" } else { "" }
    } else {
        e.tzid.as_str()
    };
    p.insert("start".into(), json!(utc_to_local(e.start, zone)));
    p.insert(
        "timeZone".into(),
        if zone.is_empty() { Value::Null } else { json!(zone) },
    );
    p.insert("duration".into(), json!(write_duration(e.end - e.start)));
    p.insert("sequence".into(), json!(e.sequence));
    p.insert(
        "status".into(),
        json!(if e.cancelled { "cancelled" } else { "confirmed" }),
    );
    p.insert(
        "locations".into(),
        if e.location.is_empty() {
            Value::Null
        } else {
            json!({ "l1": { "@type": "Location", "name": e.location } })
        },
    );
    p.insert(
        "recurrenceRules".into(),
        match write_rrule(&e.recurrence).filter(|_| !e.recurrence.is_empty()) {
            Some(rule) => json!([rule]),
            None => Value::Null,
        },
    );
    let mut people = Map::new();
    let mut add = |key: String, a: &ical::Attendee, owner: bool, attendee: bool| {
        let mut roles = Map::new();
        if owner {
            roles.insert("owner".into(), json!(true));
        }
        if attendee {
            roles.insert("attendee".into(), json!(true));
        }
        let mut o = json!({
            "@type": "Participant",
            "sendTo": { "imip": format!("mailto:{}", a.email) },
            "roles": roles,
            // Picacho emails invitations itself (iMIP): the server mustn't.
            "scheduleAgent": "client",
        });
        if !a.name.is_empty() {
            o["name"] = json!(a.name);
        }
        if let Some(s) = a.status {
            o["participationStatus"] = json!(s.as_str().to_ascii_lowercase());
        }
        people.insert(key, o);
    };
    if let Some(o) = e.organizer.as_ref().filter(|o| !o.email.is_empty()) {
        let also = e.attendees.iter().any(|a| a.email.eq_ignore_ascii_case(&o.email));
        add("organizer".into(), o, true, also);
    }
    for (i, a) in e.attendees.iter().enumerate() {
        if e.organizer.as_ref().is_some_and(|o| o.email.eq_ignore_ascii_case(&a.email)) {
            continue;
        }
        add(format!("p{i}"), a, false, true);
    }
    p.insert(
        "participants".into(),
        if people.is_empty() { Value::Null } else { Value::Object(people) },
    );
    // Changed and left-out repeats.
    let zone_of = if e.tzid.is_empty() { "Etc/UTC" } else { e.tzid.as_str() };
    let mut over = Map::new();
    for x in &e.exdates {
        over.insert(utc_to_local(*x, zone_of), json!({ "excluded": true }));
    }
    for o in overrides {
        let Some(at) = o.recurrence_id else { continue };
        let mut patch = Map::new();
        if o.cancelled {
            patch.insert("excluded".into(), json!(true));
        } else {
            patch.insert("start".into(), json!(utc_to_local(o.start, zone_of)));
            patch.insert("duration".into(), json!(write_duration(o.end - o.start)));
            patch.insert("title".into(), json!(o.summary));
            if o.location != e.location {
                patch.insert(
                    "locations".into(),
                    json!({ "l1": { "@type": "Location", "name": o.location } }),
                );
            }
            if o.description != e.description {
                patch.insert("description".into(), json!(o.description));
            }
        }
        over.insert(utc_to_local(at, zone_of), Value::Object(patch));
    }
    p.insert(
        "recurrenceOverrides".into(),
        if over.is_empty() { Value::Null } else { Value::Object(over) },
    );
    p
}

/// The events of `calendars` (ids) overlapping `from`..`to`, with
/// repeating ones as series (the server's `expandRecurrences` left off).
pub fn events(s: &Session, calendars: &[String], from: i64, to: i64) -> Result<Vec<JmapEvent>> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let stamp = |t: i64| {
        DateTime::from_timestamp(t, 0)
            .unwrap_or_default()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    };
    let mut out = Vec::new();
    for calendar in calendars {
        let responses = s.call_many(&[
            (
                "CalendarEvent/query",
                json!({
                    "accountId": account,
                    "filter": { "inCalendar": calendar, "after": stamp(from), "before": stamp(to) },
                }),
            ),
            (
                "CalendarEvent/get",
                json!({
                    "accountId": account,
                    "#ids": { "resultOf": "c0", "name": "CalendarEvent/query", "path": "/ids" },
                }),
            ),
        ])?;
        let get = responses
            .iter()
            .find(|(n, _)| n == "CalendarEvent/get")
            .map(|(_, v)| v)
            .context("The JMAP server didn't answer for its events.")?;
        for v in get["list"].as_array().into_iter().flatten() {
            let Some(mut event) = from_jscalendar(v) else { continue };
            let (changed, excluded) = overrides(v, &event);
            event.exdates = excluded;
            out.push(JmapEvent {
                id: v["id"].as_str().unwrap_or_default().to_owned(),
                calendar: calendar.clone(),
                event,
                overrides: changed,
                json: v.clone(),
            });
        }
    }
    Ok(out)
}

/// Creates or changes an event in `calendar`. Returns its id.
pub fn save_event(
    s: &Session,
    id: Option<&str>,
    calendar: &str,
    event: &ical::Event,
    overrides: &[ical::Event],
) -> Result<String> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let mut props = to_jscalendar(event, overrides);
    props.insert("calendarIds".into(), json!({ calendar: true }));
    let args = match id {
        Some(id) => {
            // Moving to another calendar replaces the set.
            json!({ "accountId": account, "update": { id: props }, "sendSchedulingMessages": false })
        }
        None => json!({ "accountId": account, "create": { "e": props }, "sendSchedulingMessages": false }),
    };
    let v = s.call_one("CalendarEvent/set", args)?;
    if let Some(id) = id {
        if let Some(e) = v["notUpdated"].get(id) {
            bail!("The server didn't change the event: {}", e["description"].as_str().or(e["type"].as_str()).unwrap_or("refused"));
        }
        return Ok(id.to_owned());
    }
    if let Some(e) = v["notCreated"].get("e") {
        bail!("The server didn't add the event: {}", e["description"].as_str().or(e["type"].as_str()).unwrap_or("refused"));
    }
    Ok(v["created"]["e"]["id"]
        .as_str()
        .context("The server didn't say which event it added.")?
        .to_owned())
}

/// Removes an event.
pub fn delete_event(s: &Session, id: &str) -> Result<()> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let v = s.call_one(
        "CalendarEvent/set",
        json!({ "accountId": account, "destroy": [id], "sendSchedulingMessages": false }),
    )?;
    if let Some(e) = v["notDestroyed"].get(id)
        && e["type"] != "notFound"
    {
        bail!("The server didn't remove the event.");
    }
    Ok(())
}

/// Makes a calendar named `name`, returning its id.
pub fn create_calendar(s: &Session, name: &str, color: &str) -> Result<String> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let mut props = json!({ "name": name });
    if !color.is_empty() {
        props["color"] = json!(color);
    }
    let v = s.call_one("Calendar/set", json!({ "accountId": account, "create": { "c": props } }))?;
    if let Some(e) = v["notCreated"].get("c") {
        bail!("The server didn't make the calendar: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(v["created"]["c"]["id"].as_str().context("No calendar id.")?.to_owned())
}

// Contacts.

/// An address book on a JMAP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JmapBook {
    pub id: String,
    pub name: String,
    pub writable: bool,
}

/// The address books of the sign-in's contacts account.
pub fn address_books(s: &Session) -> Result<Vec<JmapBook>> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let v = s.call_one("AddressBook/get", json!({ "accountId": account, "ids": null }))?;
    Ok(v["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            Some(JmapBook {
                id: b["id"].as_str()?.to_owned(),
                name: b["name"].as_str().unwrap_or("Contacts").to_owned(),
                writable: b["myRights"]["mayWrite"].as_bool().unwrap_or(true),
            })
        })
        .collect())
}

/// A JSContact card's name: `name.full`, else its components.
fn card_name(v: &Value) -> String {
    if let Some(full) = v["name"]["full"].as_str().filter(|f| !f.is_empty()) {
        return full.to_owned();
    }
    let parts: Vec<&str> = v["name"]["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| matches!(c["kind"].as_str(), Some("given" | "given2" | "surname" | "surname2")))
        .filter_map(|c| c["value"].as_str())
        .collect();
    parts.join(" ")
}

/// A JSContact Card (RFC 9553) as a card.
pub fn from_jscontact(v: &Value) -> Card {
    let values = |key: &str, field: &str| -> Vec<String> {
        let mut entries: Vec<(i64, String)> = v[key]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, e)| {
                let value = e[field].as_str()?.trim().trim_start_matches("tel:").to_owned();
                Some((e["pref"].as_i64().unwrap_or(100), value))
            })
            .filter(|(_, v)| !v.is_empty())
            .collect();
        entries.sort_by_key(|(pref, _)| *pref);
        entries.into_iter().map(|(_, v)| v).collect()
    };
    let org = v["organizations"]
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(_, o)| {
            let mut parts = vec![o["name"].as_str().unwrap_or_default().to_owned()];
            parts.extend(
                o["units"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|u| u["name"].as_str().map(str::to_owned)),
            );
            let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        })
        .unwrap_or_default();
    let note = v["notes"]
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(_, n)| n["note"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let emails = values("emails", "address");
    let addresses = v["addresses"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, a)| {
            if let Some(full) = a["full"].as_str().filter(|f| !f.is_empty()) {
                return Some(full.replace('\n', ", "));
            }
            let parts: Vec<&str> = a["components"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["kind"].as_str() != Some("separator"))
                .filter_map(|c| c["value"].as_str())
                .collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        })
        .collect();
    let birthday = v["anniversaries"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(_, a)| a["kind"].as_str() == Some("birth"))
        .and_then(|(_, a)| {
            let d = &a["date"];
            if let Some(s) = d.as_str() {
                return Some(s.split('T').next().unwrap_or(s).to_owned());
            }
            let (m, day) = (d["month"].as_u64()?, d["day"].as_u64()?);
            Some(match d["year"].as_u64() {
                Some(y) => format!("{y:04}-{m:02}-{day:02}"),
                None => format!("--{m:02}-{day:02}"),
            })
        })
        .unwrap_or_default();
    let photo = v["media"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(_, m)| m["kind"].as_str() == Some("photo"))
        .and_then(|(_, m)| m["uri"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let group = v["kind"].as_str() == Some("group");
    let members = v["members"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, on)| on.as_bool().unwrap_or(false))
        .map(|(m, _)| m.trim_start_matches("urn:uuid:").to_owned())
        .collect();
    Card {
        addresses,
        birthday,
        photo,
        group,
        members,
        uid: v["uid"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| emails.join(",").to_lowercase()),
        name: card_name(v),
        emails,
        phones: values("phones", "number"),
        org,
        note,
        ..Default::default()
    }
}

/// The properties of a card Picacho edits as JSContact, keeping each
/// unchanged address's and number's entry (its labels) from `before`.
pub fn to_jscontact(card: &Card, before: Option<&Value>) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("@type".into(), json!("Card"));
    p.insert("version".into(), json!("1.0"));
    if !card.uid.is_empty() {
        p.insert("uid".into(), json!(card.uid));
    }
    let old_name = before.map(card_name).unwrap_or_default();
    if old_name != card.name {
        let (given, surname) = match card.name.trim().rsplit_once(' ') {
            Some((g, s)) => (g.to_owned(), s.to_owned()),
            None => (card.name.trim().to_owned(), String::new()),
        };
        let mut components = vec![json!({ "kind": "given", "value": given })];
        if !surname.is_empty() {
            components.push(json!({ "kind": "surname", "value": surname }));
        }
        p.insert(
            "name".into(),
            json!({ "@type": "Name", "full": card.name, "components": components }),
        );
    }
    let keep = |key: &str, field: &str, list: &[String], make: &dyn Fn(&str) -> Value| {
        let mut out = Map::new();
        let old = before.and_then(|b| b[key].as_object());
        for (i, value) in list.iter().enumerate() {
            let same = old.and_then(|o| {
                o.iter().find(|(_, e)| {
                    e[field]
                        .as_str()
                        .is_some_and(|v| v.trim().trim_start_matches("tel:").eq_ignore_ascii_case(value))
                })
            });
            match same {
                Some((k, e)) => {
                    out.insert(k.clone(), e.clone());
                }
                None => {
                    out.insert(format!("{key}{}", i + 1), make(value));
                }
            }
        }
        if out.is_empty() { Value::Null } else { Value::Object(out) }
    };
    p.insert(
        "emails".into(),
        keep("emails", "address", &card.emails, &|e| json!({ "@type": "EmailAddress", "address": e })),
    );
    p.insert(
        "phones".into(),
        keep("phones", "number", &card.phones, &|n| json!({ "@type": "Phone", "number": n })),
    );
    let old_org = before.map(|b| from_jscontact(b).org).unwrap_or_default();
    if old_org != card.org {
        p.insert(
            "organizations".into(),
            if card.org.is_empty() {
                Value::Null
            } else {
                json!({ "o1": { "@type": "Organization", "name": card.org } })
            },
        );
    }
    let old = before.map(from_jscontact).unwrap_or_default();
    if old.addresses != card.addresses {
        let list: Map<String, Value> = card
            .addresses
            .iter()
            .enumerate()
            .map(|(i, a)| (format!("a{}", i + 1), json!({ "@type": "Address", "full": a })))
            .collect();
        p.insert(
            "addresses".into(),
            if list.is_empty() { Value::Null } else { Value::Object(list) },
        );
    }
    if old.birthday != card.birthday {
        let date = match card.birthday.strip_prefix("--") {
            Some(md) => {
                let (m, d) = md.split_once('-').unwrap_or((md, ""));
                json!({ "@type": "PartialDate", "month": m.parse::<u64>().unwrap_or(1), "day": d.parse::<u64>().unwrap_or(1) })
            }
            None => {
                let mut it = card.birthday.split('-').map(|n| n.parse::<u64>().unwrap_or(1));
                json!({ "@type": "PartialDate", "year": it.next(), "month": it.next(), "day": it.next() })
            }
        };
        p.insert(
            "anniversaries".into(),
            if card.birthday.is_empty() {
                Value::Null
            } else {
                json!({ "b": { "@type": "Anniversary", "kind": "birth", "date": date } })
            },
        );
    }
    if old.photo != card.photo {
        p.insert(
            "media".into(),
            if card.photo.is_empty() {
                Value::Null
            } else {
                json!({ "p": { "@type": "Media", "kind": "photo", "uri": card.photo } })
            },
        );
    }
    if card.group {
        p.insert("kind".into(), json!("group"));
        let members: Map<String, Value> = card
            .members
            .iter()
            .map(|m| (format!("urn:uuid:{m}"), json!(true)))
            .collect();
        p.insert("members".into(), Value::Object(members));
    }
    let old_note = before.map(|b| from_jscontact(b).note).unwrap_or_default();
    if old_note != card.note {
        p.insert(
            "notes".into(),
            if card.note.is_empty() {
                Value::Null
            } else {
                json!({ "n1": { "@type": "Note", "note": card.note } })
            },
        );
    }
    p
}

/// Every card in the account's address books, each with its id (in
/// `href`, as `jmap:<id>`) and the book it is in.
pub fn cards(s: &Session) -> Result<(Vec<Card>, String)> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let books = address_books(s)?;
    let v = s.call_one("ContactCard/get", json!({ "accountId": account, "ids": null }))?;
    let mut out = Vec::new();
    for c in v["list"].as_array().into_iter().flatten() {
        let mut card = from_jscontact(c);
        card.href = format!("jmap:{}", c["id"].as_str().unwrap_or_default());
        // The JSON as it came, so an edit keeps what Picacho doesn't show.
        card.vcard = c.to_string();
        card.etag = v["state"].as_str().unwrap_or_default().to_owned();
        out.push(card);
    }
    let book = books
        .iter()
        .find(|b| b.writable)
        .map(|b| format!("jmap:{}", b.id))
        .unwrap_or_default();
    Ok((out, book))
}

/// Creates or changes a card; returns it as saved.
pub fn save_card(s: &Session, card: &Card, book: &str) -> Result<Card> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let before: Option<Value> = serde_json::from_str(&card.vcard).ok().filter(|v: &Value| v.is_object());
    let mut props = to_jscontact(card, before.as_ref());
    let existing = card.href.strip_prefix("jmap:").filter(|i| !i.is_empty());
    let mut saved = card.clone();
    match existing {
        Some(id) => {
            let v = s.call_one(
                "ContactCard/set",
                json!({ "accountId": account, "update": { id: props } }),
            )?;
            if let Some(e) = v["notUpdated"].get(id) {
                bail!("The server didn't change the contact: {}", e["type"].as_str().unwrap_or("refused"));
            }
        }
        None => {
            let book = book.strip_prefix("jmap:").filter(|b| !b.is_empty());
            let book = book.context("No address book on the server can take new contacts.")?;
            props.insert("addressBookIds".into(), json!({ book: true }));
            let v = s.call_one(
                "ContactCard/set",
                json!({ "accountId": account, "create": { "c": props } }),
            )?;
            if let Some(e) = v["notCreated"].get("c") {
                bail!("The server didn't add the contact: {}", e["type"].as_str().unwrap_or("refused"));
            }
            let id = v["created"]["c"]["id"].as_str().context("No contact id.")?;
            saved.href = format!("jmap:{id}");
        }
    }
    // What the server keeps now, for the next edit.
    let id = saved.href.trim_start_matches("jmap:").to_owned();
    let v = s.call_one("ContactCard/get", json!({ "accountId": account, "ids": [id] }))?;
    if let Some(c) = v["list"].as_array().and_then(|l| l.first()) {
        saved.vcard = c.to_string();
        saved.etag = v["state"].as_str().unwrap_or_default().to_owned();
    }
    Ok(saved)
}

/// Removes a card.
pub fn delete_card(s: &Session, card: &Card) -> Result<()> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let id = card.href.strip_prefix("jmap:").context("This contact isn't on the JMAP server.")?;
    let v = s.call_one(
        "ContactCard/set",
        json!({ "accountId": account, "destroy": [id] }),
    )?;
    if let Some(e) = v["notDestroyed"].get(id)
        && e["type"] != "notFound"
    {
        bail!("The server didn't remove the contact.");
    }
    Ok(())
}

/// Makes an address book named `name`, returning its id.
pub fn create_address_book(s: &Session, name: &str) -> Result<String> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let v = s.call_one(
        "AddressBook/set",
        json!({ "accountId": account, "create": { "b": { "name": name } } }),
    )?;
    if let Some(e) = v["notCreated"].get("b") {
        bail!("The server didn't make the address book: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(v["created"]["b"]["id"].as_str().context("No address book id.")?.to_owned())
}

/// Whether a session can be used for calendars or contacts at all.
pub fn ensure_calendars(s: &Session) -> Result<()> {
    ensure!(
        s.calendars_account().is_some(),
        "This JMAP server doesn't offer calendars."
    );
    Ok(())
}
pub fn ensure_contacts(s: &Session) -> Result<()> {
    ensure!(
        s.contacts_account().is_some(),
        "This JMAP server doesn't offer contacts."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_both_ways() {
        assert_eq!(duration("PT1H30M"), Some(5400));
        assert_eq!(duration("P1DT2H"), Some(93_600));
        assert_eq!(duration("P2W"), Some(1_209_600));
        assert_eq!(write_duration(5400), "PT1H30M");
        assert_eq!(write_duration(86_400), "P1D");
        assert_eq!(write_duration(0), "PT0S");
    }

    #[test]
    fn jscalendar_events_read_and_write() {
        let v = json!({
            "@type": "Event",
            "id": "e1",
            "uid": "abc@x",
            "title": "Planning",
            "start": "2026-11-24T10:00:00",
            "timeZone": "Europe/Berlin",
            "duration": "PT1H",
            "locations": { "a": { "@type": "Location", "name": "Room 4" } },
            "recurrenceRules": [{ "@type": "RecurrenceRule", "frequency": "weekly", "byDay": [{ "day": "tu" }], "count": 5 }],
            "recurrenceOverrides": { "2026-12-01T10:00:00": { "excluded": true } },
            "participants": {
                "a": { "@type": "Participant", "name": "Ana", "sendTo": { "imip": "mailto:ana@example.org" }, "roles": { "owner": true, "attendee": true }, "participationStatus": "accepted" },
                "b": { "@type": "Participant", "sendTo": { "imip": "mailto:bo@example.se" }, "roles": { "attendee": true }, "participationStatus": "needs-action" }
            }
        });
        let mut e = from_jscalendar(&v).unwrap();
        let (changed, excluded) = overrides(&v, &e);
        e.exdates = excluded;
        assert!(changed.is_empty());
        assert_eq!(e.summary, "Planning");
        assert_eq!(e.location, "Room 4");
        assert_eq!(e.tzid, "Europe/Berlin");
        // 10:00 in Berlin in November is 09:00 UTC.
        assert_eq!(e.start % 86_400, 9 * 3600);
        assert_eq!(e.end - e.start, 3600);
        assert_eq!(e.recurrence, "FREQ=WEEKLY;COUNT=5;BYDAY=TU");
        assert_eq!(e.organizer.as_ref().unwrap().email, "ana@example.org");
        assert_eq!(e.attendees.len(), 2);
        assert_eq!(e.attendees[1].status, Some(ical::PartStat::NeedsAction));
        assert_eq!(e.exdates.len(), 1);
        // Written back, it reads the same.
        let back = from_jscalendar(&Value::Object(to_jscalendar(&e, &[]))).unwrap();
        assert_eq!((back.start, back.end, &back.tzid), (e.start, e.end, &e.tzid));
        assert_eq!(back.recurrence, e.recurrence);
        assert_eq!(back.location, e.location);
        assert_eq!(back.attendees.len(), 2);
        let written = Value::Object(to_jscalendar(&e, &[]));
        assert_eq!(written["participants"]["p1"]["scheduleAgent"], "client");
        assert_eq!(written["recurrenceOverrides"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn jscontact_cards_keep_their_labels() {
        let v = json!({
            "@type": "Card", "version": "1.0", "uid": "u1",
            "name": { "components": [{ "kind": "given", "value": "Sophie" }, { "kind": "surname", "value": "Chen" }] },
            "emails": {
                "work": { "@type": "EmailAddress", "address": "s.chen@work.example", "contexts": { "work": true }, "pref": 2 },
                "home": { "@type": "EmailAddress", "address": "sophie@example.com", "pref": 1 }
            },
            "phones": { "m": { "@type": "Phone", "number": "tel:+1 555 0100", "features": { "mobile": true } } },
            "organizations": { "o": { "name": "Acme", "units": [{ "name": "Research" }] } }
        });
        let card = from_jscontact(&v);
        assert_eq!(card.name, "Sophie Chen");
        assert_eq!(card.emails, ["sophie@example.com", "s.chen@work.example"]);
        assert_eq!(card.phones, ["+1 555 0100"]);
        assert_eq!(card.org, "Acme, Research");
        let mut edited = card.clone();
        edited.emails.push("new@example.com".into());
        let patch = to_jscontact(&edited, Some(&v));
        // The work address keeps its key and contexts; the name is unchanged.
        assert_eq!(patch["emails"]["work"]["contexts"]["work"], true);
        assert_eq!(patch["emails"]["emails3"]["address"], "new@example.com");
        assert!(!patch.contains_key("name") && !patch.contains_key("organizations"));
        assert_eq!(patch["phones"]["m"]["features"]["mobile"], true);
    }
}
