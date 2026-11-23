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
/// A method refused for a reason worth handling: `stateMismatch`
/// (`ifInState` no longer holds), `unknownMethod`, `forbidden`…
fn refused(e: &anyhow::Error, kind: &str) -> bool {
    e.downcast_ref::<crate::sync::jmap::MethodError>()
        .is_some_and(|m| m.is(kind))
}
/// What `Foo/changes` said since a state: created, updated and destroyed
/// ids, and the new state. `None` when the server can't say (too old a
/// state, or more changes than asked for at once), and everything is to
/// be read again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub changed: Vec<String>,
    pub destroyed: Vec<String>,
    pub state: String,
}
fn changes(s: &Session, method: &str, account: &str, since: &str) -> Result<Option<Changed>> {
    let mut out = Changed::default();
    let mut since = since.to_owned();
    // Several pages where there is more (hasMoreChanges), up to a bound:
    // past it, reading whole is cheaper.
    for _ in 0..20 {
        let v = match s.call_one(
            method,
            json!({ "accountId": account, "sinceState": since, "maxChanges": 500 }),
        ) {
            Ok(v) => v,
            // RFC 8620 §5.2: too old a state, or a server without the
            // method: everything is read again.
            Err(e) if crate::sync::jmap::must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let ids = |key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect()
        };
        out.changed.extend(ids("created"));
        out.changed.extend(ids("updated"));
        out.destroyed.extend(ids("destroyed"));
        since = v["newState"].as_str().unwrap_or_default().to_owned();
        out.state = since.clone();
        if !v["hasMoreChanges"].as_bool().unwrap_or(false) {
            let destroyed: std::collections::HashSet<&String> = out.destroyed.iter().collect();
            out.changed.retain(|id| !destroyed.contains(id));
            out.changed.sort();
            out.changed.dedup();
            return Ok(Some(out));
        }
    }
    Ok(None)
}
/// `Foo/get` of `ids` in pieces the server takes (`maxObjectsInGet`):
/// the objects and the state they were read in.
fn get_all(s: &Session, method: &str, account: &str, ids: &[String]) -> Result<(Vec<Value>, String)> {
    let mut list = Vec::new();
    let mut state = String::new();
    for chunk in ids.chunks(s.max_get()) {
        let v = s.call_one(method, json!({ "accountId": account, "ids": chunk }))?;
        list.extend(v["list"].as_array().cloned().unwrap_or_default());
        state = v["state"].as_str().unwrap_or_default().to_owned();
    }
    Ok((list, state))
}
/// Every id `Foo/query` lists for `filter`, a page (`position`) at a time,
/// so a server's own limit on results doesn't cut the list short.
fn query_all(s: &Session, method: &str, account: &str, filter: Value) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let page = s.max_get().min(500);
    loop {
        let v = s.call_one(
            method,
            json!({ "accountId": account, "filter": filter, "position": ids.len(), "limit": page, "calculateTotal": true }),
        )?;
        let got: Vec<String> = v["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect();
        let n = got.len();
        ids.extend(got);
        let total = v["total"].as_u64().map(|t| t as usize);
        // Done: all listed, or a short page from a server without totals.
        if n == 0 || total.is_some_and(|t| ids.len() >= t) || (total.is_none() && n < page) {
            break;
        }
        // A bound against a server that never says it's done.
        if ids.len() > 200_000 {
            break;
        }
    }
    Ok(ids)
}
/// An event's version on the server now (`event_version`), by id.
pub fn version_of(s: &Session, id: &str) -> Result<String> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let (list, _) = get_all(s, "CalendarEvent/get", account, &[id.to_owned()])?;
    Ok(list.first().map(event_version).unwrap_or_default())
}
/// An event's version for `Stored::etag`: as a card's.
pub fn event_version(v: &Value) -> String {
    card_version(v)
}
/// Events created or changed, ids gone, and the new state.
pub type EventChanges = (Vec<JmapEvent>, Vec<String>, String);
/// Cards created or changed, hrefs gone, and the new state.
pub type CardChanges = (Vec<Card>, Vec<String>, String);
/// A card's version for `Card::etag`: JSContact's `updated` where the
/// server keeps it, else a hash of the card as given (any change changes it).
fn card_version(v: &Value) -> String {
    if let Some(u) = v["updated"].as_str().filter(|u| !u.is_empty()) {
        return u.to_owned();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(v.to_string().as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}
/// A write refused because the server's copy changed since it was read
/// (`stateMismatch`): read it again rather than overwrite what changed.
#[derive(Debug)]
pub struct Conflict(pub &'static str);
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Conflict {}

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
    // Fractional seconds are allowed (RFC 8984 §1.4.4); only whole ones
    // matter here.
    let local = local.split('.').next().unwrap_or(local);
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
/// `zone` is the event's: a rule's `until` is a LocalDateTime in it (RFC
/// 8984 §4.3.3), and iCalendar's UNTIL is in UTC.
fn rrule(rule: &Value, zone: Option<&str>) -> Option<String> {
    let freq = rule["frequency"].as_str()?.to_ascii_uppercase();
    let mut parts = vec![format!("FREQ={freq}")];
    if let Some(n) = rule["interval"].as_u64().filter(|n| *n > 1) {
        parts.push(format!("INTERVAL={n}"));
    }
    if let Some(n) = rule["count"].as_u64() {
        parts.push(format!("COUNT={n}"));
    }
    if let Some(until) = rule["until"].as_str()
        && let Some(t) = local_to_utc(until, zone)
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
/// `zone` is the event's: a UTC UNTIL becomes a LocalDateTime in it.
fn write_rrule(text: &str, zone: &str) -> Option<Value> {
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
                // A UTC UNTIL, in the event's own zone.
                let local = if v.ends_with('Z') && !zone.is_empty() {
                    utc_to_local(t.and_utc().timestamp(), zone)
                } else {
                    t.format("%Y-%m-%dT%H:%M:%S").to_string()
                };
                rule.insert("until".into(), json!(local));
            }
            "BYDAY" => {
                let days: Vec<Value> = v
                    .split(',')
                    .filter_map(|d| {
                        // Byte offsets below; a day is two ASCII letters.
                        if !d.is_ascii() {
                            return None;
                        }
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
        e.recurrence = rrule(rule, if all_day { None } else { zone }).unwrap_or_default();
    }
    for (_, p) in v["participants"].as_object().into_iter().flatten() {
        let roles = &p["roles"];
        let person = ical::Attendee {
            email: participant_email(p),
            name: p["name"].as_str().unwrap_or_default().to_owned(),
            status: p["participationStatus"]
                .as_str()
                .map(ical::PartStat::parse),
            ..Default::default()
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
                // Patches name properties by JSON pointer (RFC 8984
                // §4.3.5): top-level ones replace, deeper ones are set in
                // place (`locations/l1/name`).
                apply_patch(w, k, val);
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

/// Sets `value` at `path` (a JSON pointer without the leading `/`, its
/// `~1` and `~0` escapes undone) in `object`; `null` removes it.
fn apply_patch(object: &mut Map<String, Value>, path: &str, value: &Value) {
    let keys: Vec<String> = path
        .split('/')
        .map(|k| k.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some((last, parents)) = keys.split_last() else {
        return;
    };
    let mut at = object;
    for key in parents {
        let next = at.entry(key.clone()).or_insert_with(|| json!({}));
        if !next.is_object() {
            *next = json!({});
        }
        at = next.as_object_mut().expect("just made an object");
    }
    if value.is_null() {
        at.remove(last);
    } else {
        at.insert(last.clone(), value.clone());
    }
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
        match write_rrule(&e.recurrence, zone).filter(|_| !e.recurrence.is_empty()) {
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
fn stamp_utc(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}
/// As `events`, with the CalendarEvent state they were read in, for
/// `event_changes` later.
pub fn events_in(
    s: &Session,
    calendars: &[String],
    from: i64,
    to: i64,
) -> Result<(Vec<JmapEvent>, String)> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let mut out = Vec::new();
    let mut state = String::new();
    for calendar in calendars {
        // Every page of the query, then the events in pieces.
        let ids = query_all(
            s,
            "CalendarEvent/query",
            account,
            json!({ "inCalendar": calendar, "after": stamp_utc(from), "before": stamp_utc(to) }),
        )?;
        let (list, read_in) = get_all(s, "CalendarEvent/get", account, &ids)?;
        if !read_in.is_empty() {
            state = read_in;
        }
        for v in &list {
            if let Some(e) = jmap_event(v, calendar) {
                out.push(e);
            }
        }
    }
    if state.is_empty() {
        // No events: the state now, so changes can be asked for later.
        let v = s.call_one("CalendarEvent/get", json!({ "accountId": account, "ids": [] }))?;
        state = v["state"].as_str().unwrap_or_default().to_owned();
    }
    Ok((out, state))
}
/// One event from its JSON, in `calendar`.
fn jmap_event(v: &Value, calendar: &str) -> Option<JmapEvent> {
    let mut event = from_jscalendar(v)?;
    let (changed, excluded) = overrides(v, &event);
    event.exdates = excluded;
    Some(JmapEvent {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        calendar: calendar.to_owned(),
        event,
        overrides: changed,
        json: v.clone(),
    })
}
/// What changed among the account's events since `since`
/// (CalendarEvent/changes): the events created or changed (in any of
/// `calendars`, whichever they're in now), the ids gone, and the new
/// state. `None` when the server can't say; then read them whole.
pub fn event_changes(
    s: &Session,
    calendars: &[String],
    since: &str,
) -> Result<Option<EventChanges>> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let Some(changed) = changes(s, "CalendarEvent/changes", account, since)? else {
        return Ok(None);
    };
    let (list, _) = get_all(s, "CalendarEvent/get", account, &changed.changed)?;
    let mut events = Vec::new();
    let mut gone = changed.destroyed;
    for v in &list {
        // The first of its calendars that is shown.
        let calendar = v["calendarIds"]
            .as_object()
            .and_then(|ids| ids.keys().find(|k| calendars.contains(k)).cloned());
        match calendar.and_then(|c| jmap_event(v, &c)) {
            Some(e) => events.push(e),
            // Moved out of every shown calendar: gone from here.
            None => gone.extend(v["id"].as_str().map(str::to_owned)),
        }
    }
    Ok(Some((events, gone, changed.state)))
}

/// Creates or changes an event in `calendar`; returns its id. With
/// `in_state` (its version when read), only if the event is still as it
/// was read: a change made elsewhere meanwhile is reported (`Conflict`)
/// rather than overwritten.
pub fn save_event_in(
    s: &Session,
    id: Option<&str>,
    calendar: &str,
    event: &ical::Event,
    overrides: &[ical::Event],
    in_state: Option<&str>,
) -> Result<String> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let mut props = to_jscalendar(event, overrides);
    props.insert("calendarIds".into(), json!({ calendar: true }));
    // An edit keeps what Picacho doesn't model in the server's copy: the
    // other properties of each location and participant (their links,
    // roles, invitedBy…), and the rest of the event isn't sent at all.
    let (current, read_state) = match id {
        Some(id) => get_all(s, "CalendarEvent/get", account, &[id.to_owned()])
            .map(|(list, state)| (list.into_iter().next(), state))
            .unwrap_or_default(),
        None => (None, String::new()),
    };
    if let Some(current) = &current {
        merge_entries(&mut props, current, "participants", &["sendTo", "roles", "name", "participationStatus", "scheduleAgent", "@type"]);
        if let (Some(Value::Object(new)), Some(old)) = (props.get_mut("locations"), current["locations"].as_object())
            && let Some((_, mine)) = new.iter_mut().next()
            && let Some((_, theirs)) = old.iter().next()
            && let (Some(mine), Some(theirs)) = (mine.as_object_mut(), theirs.as_object())
        {
            for (k, v) in theirs {
                mine.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
    let args = match id {
        Some(id) => {
            // Moving to another calendar replaces the set.
            json!({ "accountId": account, "update": { id: props }, "sendSchedulingMessages": false })
        }
        None => json!({ "accountId": account, "create": { "e": props }, "sendSchedulingMessages": false }),
    };
    // Only over the version it was read in (`in_state`: the event's
    // version, `event_version`): one changed elsewhere meanwhile isn't
    // overwritten.
    const CHANGED: &str = "This event was changed elsewhere since it was read. Refresh the calendar and make the change again.";
    let v = match (in_state.filter(|s| !s.is_empty()), id, &current) {
        (Some(version), Some(id), Some(current)) => {
            ensure!(event_version(current) == version, Conflict(CHANGED));
            set_if_unchanged(s, "CalendarEvent", account, id, version, &read_state, args, event_version, CHANGED)?
        }
        _ => s.call_one("CalendarEvent/set", args)?,
    };
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

/// `{kind}/set` with `args`, changing `id` only while it is still at
/// `version` (as `version_of` reads it): sent with `ifInState`, the state
/// it was read in (RFC 8620 §5.3), so nothing changes between the check and
/// the write. The state covers every object of the kind, so a mismatch is
/// read again: where `id` itself is unchanged, the change is tried once
/// more in the new state; where it changed, it is a `Conflict`.
#[allow(clippy::too_many_arguments)]
fn set_if_unchanged(
    s: &Session,
    kind: &str,
    account: &str,
    id: &str,
    version: &str,
    read_state: &str,
    args: Value,
    version_of: fn(&Value) -> String,
    changed: &'static str,
) -> Result<Value> {
    let mut state = read_state.to_owned();
    for _ in 0..2 {
        let mut call = args.clone();
        if !state.is_empty() {
            call["ifInState"] = json!(state);
        }
        match s.call_one(&format!("{kind}/set"), call) {
            Err(e) if refused(&e, "stateMismatch") => {
                let (now, new_state) = get_all(s, &format!("{kind}/get"), account, &[id.to_owned()])?;
                match now.first() {
                    Some(current) if version_of(current) == version => state = new_state,
                    _ => bail!(Conflict(changed)),
                }
            }
            other => return other,
        }
    }
    bail!(Conflict(changed))
}

/// Each entry of `key` in `props` (as Picacho wrote it) takes the other
/// fields of its entry in `current` (the server's): matched by the same
/// address for participants. Fields in `ours` are Picacho's to set.
fn merge_entries(props: &mut Map<String, Value>, current: &Value, key: &str, ours: &[&str]) {
    let (Some(Value::Object(new)), Some(old)) = (props.get_mut(key), current[key].as_object()) else {
        return;
    };
    let mut merged = Map::new();
    for (k, mine) in new.iter() {
        let email = participant_email(mine).to_ascii_lowercase();
        let theirs = old
            .iter()
            .find(|(_, t)| !email.is_empty() && participant_email(t).eq_ignore_ascii_case(&email));
        match (theirs, mine.as_object()) {
            (Some((their_key, t)), Some(m)) => {
                let mut entry = t.as_object().cloned().unwrap_or_default();
                for field in ours {
                    match m.get(*field) {
                        Some(v) => entry.insert((*field).to_owned(), v.clone()),
                        None => entry.remove(*field),
                    };
                }
                merged.insert(their_key.clone(), Value::Object(entry));
            }
            _ => {
                merged.insert(k.clone(), mine.clone());
            }
        }
    }
    *new = merged;
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
    // Members by their uid as written: `urn:uuid:` ones without it (as
    // vCard's MEMBER has them), others (`mailto:`…) whole.
    let members = v["members"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, on)| on.as_bool().unwrap_or(false))
        .map(|(m, _)| m.strip_prefix("urn:uuid:").unwrap_or(m).to_owned())
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
                // The first organization changed; others stay.
                let mut all: Map<String, Value> = before
                    .and_then(|b| b["organizations"].as_object().cloned())
                    .unwrap_or_default();
                let first = all.keys().next().cloned().unwrap_or_else(|| "o1".into());
                all.insert(first, json!({ "@type": "Organization", "name": card.org }));
                Value::Object(all)
            },
        );
    }
    let old = before.map(from_jscontact).unwrap_or_default();
    if old.addresses != card.addresses {
        // Each address that is as it was keeps its entry (its parts,
        // labels and coordinates); new ones are written in full.
        let old_entries: Vec<(String, Value, String)> = before
            .and_then(|b| b["addresses"].as_object())
            .into_iter()
            .flatten()
            .map(|(k, e)| {
                let one = from_jscontact(&json!({ "addresses": { k.clone(): e.clone() } }));
                (k.clone(), e.clone(), one.addresses.first().cloned().unwrap_or_default())
            })
            .collect();
        let list: Map<String, Value> = card
            .addresses
            .iter()
            .enumerate()
            .map(|(i, a)| match old_entries.iter().find(|(_, _, line)| line == a) {
                Some((k, e, _)) => (k.clone(), e.clone()),
                None => (format!("a{}", i + 1), json!({ "@type": "Address", "full": a })),
            })
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
            {
                // Only the birthday changes; weddings and other dates stay.
                let mut all: Map<String, Value> = before
                    .and_then(|b| b["anniversaries"].as_object().cloned())
                    .unwrap_or_default();
                let birth = all
                    .iter()
                    .find(|(_, a)| a["kind"].as_str() == Some("birth"))
                    .map(|(k, _)| k.clone());
                match (card.birthday.is_empty(), birth) {
                    (true, Some(k)) => {
                        all.remove(&k);
                    }
                    (true, None) => {}
                    (false, k) => {
                        all.insert(
                            k.unwrap_or_else(|| "b".into()),
                            json!({ "@type": "Anniversary", "kind": "birth", "date": date }),
                        );
                    }
                }
                if all.is_empty() { Value::Null } else { Value::Object(all) }
            },
        );
    }
    // A photo kept as a blob and shown inlined is unchanged.
    let unchanged_blob = !card.photo_blob.is_empty() && card.photo == card.photo_read;
    if old.photo != card.photo && !unchanged_blob {
        p.insert(
            "media".into(),
            {
                // Only the photo changes; logos and sounds stay.
                let mut all: Map<String, Value> = before
                    .and_then(|b| b["media"].as_object().cloned())
                    .unwrap_or_default();
                all.retain(|_, m| m["kind"].as_str() != Some("photo"));
                if !card.photo.is_empty() {
                    all.insert("p".into(), json!({ "@type": "Media", "kind": "photo", "uri": card.photo }));
                }
                if all.is_empty() { Value::Null } else { Value::Object(all) }
            },
        );
    }
    if card.group {
        p.insert("kind".into(), json!("group"));
        // A uid that isn't a bare UUID (a `mailto:` or other URI) is kept
        // as it is (RFC 9553 §2.1.6: members are by uid).
        let members: Map<String, Value> = card
            .members
            .iter()
            .map(|m| {
                let uri = if m.contains(':') { m.clone() } else { format!("urn:uuid:{m}") };
                (uri, json!(true))
            })
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
                // The first note changed, keeping its author and date
                // fields; others stay.
                let mut all: Map<String, Value> = before
                    .and_then(|b| b["notes"].as_object().cloned())
                    .unwrap_or_default();
                let first = all.keys().next().cloned().unwrap_or_else(|| "n1".into());
                let mut entry = all.get(&first).cloned().unwrap_or_else(|| json!({ "@type": "Note" }));
                entry["note"] = json!(card.note);
                all.insert(first, entry);
                Value::Object(all)
            },
        );
    }
    p
}

/// Every card in the account's address books, each with its id (in
/// `href`, as `jmap:<id>`) and its own version (`etag`). Read in pieces
/// the server takes (`maxObjectsInGet`). Returns the cards, the book new
/// cards go to, and the ContactCard state they were read in. Photos kept
/// as blobs are reused from `known` (the cards as last read) where the
/// blob is the same.
pub fn cards_in(s: &Session, known: &[Card]) -> Result<(Vec<Card>, String, String)> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let books = address_books(s)?;
    // Every id by query, then the cards in pieces; a server without
    // ContactCard/query is asked for them all at once, as before.
    let (list, mut state) = match query_all(s, "ContactCard/query", account, json!({})) {
        Ok(ids) => get_all(s, "ContactCard/get", account, &ids)?,
        Err(e) if refused(&e, "unknownMethod") => {
            let v = s.call_one("ContactCard/get", json!({ "accountId": account, "ids": null }))?;
            (
                v["list"].as_array().cloned().unwrap_or_default(),
                v["state"].as_str().unwrap_or_default().to_owned(),
            )
        }
        Err(e) => return Err(e),
    };
    if state.is_empty() {
        let v = s.call_one("ContactCard/get", json!({ "accountId": account, "ids": [] }))?;
        state = v["state"].as_str().unwrap_or_default().to_owned();
    }
    let mut out: Vec<Card> = list.iter().map(jmap_card).collect();
    fill_photos(s, account, &mut out, &list, known);
    let book = books
        .iter()
        .find(|b| b.writable)
        .map(|b| format!("jmap:{}", b.id))
        .unwrap_or_default();
    Ok((out, book, state))
}
/// The most photos downloaded in one read; the rest come on later reads.
const PHOTOS_PER_READ: usize = 200;
/// A card's photo kept as a blob (RFC 9610 §2.2: `media`'s `blobId`):
/// the blob id and its media type.
fn photo_blob(raw: &Value) -> Option<(String, String)> {
    raw["media"].as_object().into_iter().flatten().find_map(|(_, m)| {
        (m["kind"].as_str() == Some("photo"))
            .then(|| Some((m["blobId"].as_str()?.to_owned(), m["mediaType"].as_str().unwrap_or("image/jpeg").to_owned())))
            .flatten()
    })
}
/// Inlines the photos the server keeps as blobs, as the vCard path has
/// them. A photo already read from the same blob is reused from `known`
/// (RFC 8620 §6: a blob id names data that never changes); the others are
/// downloaded several at once, up to `PHOTOS_PER_READ`.
fn fill_photos(s: &Session, account: &str, cards: &mut [Card], raw: &[Value], known: &[Card]) {
    use base64::Engine as _;
    let cached: std::collections::HashMap<&str, &Card> = known
        .iter()
        .filter(|k| !k.photo_blob.is_empty() && !k.photo_read.is_empty())
        .map(|k| (k.photo_blob.as_str(), k))
        .collect();
    let mut wanted: Vec<(usize, String, String)> = Vec::new();
    for (i, (card, raw)) in cards.iter_mut().zip(raw).enumerate() {
        if !card.photo.is_empty() {
            continue;
        }
        let Some((blob, kind)) = photo_blob(raw) else { continue };
        if let Some(k) = cached.get(blob.as_str()) {
            // What was read from the blob, not a local edit of it.
            card.photo = k.photo_read.clone();
            card.photo_read = card.photo.clone();
            card.photo_blob = blob;
        } else if wanted.len() < PHOTOS_PER_READ {
            wanted.push((i, blob, kind));
        }
    }
    let downloaded = s.in_parallel(&wanted, |(_, blob, kind)| {
        s.download_in(account, blob, "photo", kind, 1 << 20).ok()
    });
    for ((i, blob, kind), bytes) in wanted.into_iter().zip(downloaded) {
        let Some(bytes) = bytes else { continue };
        let card = &mut cards[i];
        card.photo = format!(
            "data:{kind};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        // Not a change of the photo: the card still came with a blob.
        card.photo_blob = blob;
        card.photo_read = card.photo.clone();
    }
}
fn jmap_card(c: &Value) -> Card {
    let mut card = from_jscontact(c);
    card.href = format!("jmap:{}", c["id"].as_str().unwrap_or_default());
    // The JSON as it came, so an edit keeps what Picacho doesn't show.
    card.vcard = c.to_string();
    card.etag = card_version(c);
    card
}
/// What changed among the cards since `since` (ContactCard/changes): the
/// cards created or changed, the hrefs (`jmap:<id>`) of those gone, and the
/// new state. `None` when the server can't say. Photos are as `cards_in`
/// reads them.
pub fn card_changes(s: &Session, since: &str, known: &[Card]) -> Result<Option<CardChanges>> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let Some(changed) = changes(s, "ContactCard/changes", account, since)? else {
        return Ok(None);
    };
    let (list, _) = get_all(s, "ContactCard/get", account, &changed.changed)?;
    let gone = changed
        .destroyed
        .iter()
        .map(|id| format!("jmap:{id}"))
        .collect();
    let mut cards: Vec<Card> = list.iter().map(jmap_card).collect();
    fill_photos(s, account, &mut cards, &list, known);
    Ok(Some((cards, gone, changed.state)))
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
            // Only over the version it was read in: a card changed
            // elsewhere meanwhile isn't overwritten.
            const CHANGED: &str = "This contact was changed elsewhere since it was read. Refresh contacts and make the change again.";
            let args = json!({ "accountId": account, "update": { id: props } });
            let v = if card.etag.is_empty() {
                s.call_one("ContactCard/set", args)?
            } else {
                let (now, state) = get_all(s, "ContactCard/get", account, &[id.to_owned()])?;
                if let Some(current) = now.first() {
                    ensure!(card_version(current) == card.etag, Conflict(CHANGED));
                }
                set_if_unchanged(s, "ContactCard", account, id, &card.etag, &state, args, card_version, CHANGED)?
            };
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
        saved.etag = card_version(c);
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

/// When `attendees` are busy over `from..to`, from the server's directory
/// of people (JMAP Principals, RFC 9670): each attendee's principal is
/// found by address (`Principal/query`), and its busy times asked
/// (`Principal/getAvailability`). People the server doesn't know are left
/// out.
pub fn free_busy(
    s: &Session,
    attendees: &[String],
    from: i64,
    to: i64,
) -> Result<Vec<ical::FreeBusy>> {
    ensure!(
        s.has(crate::sync::jmap::PRINCIPALS),
        "Your calendar server can't say when others are free."
    );
    let account = s
        .principals_account()
        .or_else(|| s.calendars_account())
        .context("Your calendar server can't say when others are free.")?
        .to_owned();
    let mut out = Vec::new();
    for email in attendees {
        let v = s.call_one(
            "Principal/query",
            json!({ "accountId": account, "filter": { "email": email }, "limit": 1 }),
        )?;
        let Some(id) = v["ids"].as_array().and_then(|i| i.first()).and_then(|i| i.as_str()) else {
            continue;
        };
        let v = match s.call_one(
            "Principal/getAvailability",
            json!({ "accountId": account, "id": id, "utcStart": stamp_utc(from), "utcEnd": stamp_utc(to) }),
        ) {
            Ok(v) => v,
            // One person's calendar not shared: the others still show.
            Err(e) if refused(&e, "forbidden") || refused(&e, "notFound") => continue,
            Err(e) => return Err(e),
        };
        let parse = |t: &Value| {
            t.as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.timestamp())
        };
        let busy = v["list"]
            .as_array()
            .into_iter()
            .flatten()
            // "unavailable", "confirmed" and "tentative" all count as busy.
            .filter_map(|b| Some((parse(&b["utcStart"])?, parse(&b["utcEnd"])?)))
            .collect();
        out.push(ical::FreeBusy {
            attendee: email.clone(),
            busy,
        });
    }
    Ok(out)
}

/// The addresses the server sends and answers invitations as for this
/// sign-in (ParticipantIdentity/get), so an event's organizer or attendee
/// is recognized as you. Empty where the server has none.
pub fn participant_identities(s: &Session) -> Result<Vec<String>> {
    let Some(account) = s.calendars_account() else {
        return Ok(Vec::new());
    };
    let v = match s.call_one("ParticipantIdentity/get", json!({ "accountId": account, "ids": null })) {
        Ok(v) => v,
        Err(e) if refused(&e, "unknownMethod") => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(v["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| participant_email(p).into())
        .filter(|e: &String| !e.is_empty())
        .collect())
}

/// What happened to shared calendars and events, as the server tells
/// (CalendarEventNotification and ShareNotification, RFC 9670 and JMAP for
/// Calendars): one line each, for showing, and their ids to dismiss.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    /// "CalendarEventNotification" or "ShareNotification".
    pub kind: String,
    pub text: String,
}
pub fn notices(s: &Session) -> Result<Vec<Notice>> {
    let mut out = Vec::new();
    let account = s.calendars_account().map(str::to_owned);
    let principals = s.principals_account().map(str::to_owned);
    for (method, account) in [
        ("CalendarEventNotification/get", account.as_deref()),
        ("ShareNotification/get", principals.as_deref()),
    ] {
        let Some(account) = account else { continue };
        let v = match s.call_one(method, json!({ "accountId": account, "ids": null })) {
            Ok(v) => v,
            Err(e) if refused(&e, "unknownMethod") => continue,
            Err(e) => return Err(e),
        };
        let kind = method.trim_end_matches("/get");
        for n in v["list"].as_array().into_iter().flatten() {
            let who = n["changedBy"]["name"]
                .as_str()
                .or(n["changedBy"]["email"].as_str())
                .filter(|w| !w.is_empty())
                .unwrap_or("Someone");
            let text = if kind == "ShareNotification" {
                let name = n["name"].as_str().unwrap_or("a calendar");
                if n["newRights"].is_null() {
                    format!("{who} stopped sharing {name} with you.")
                } else if n["oldRights"].is_null() {
                    format!("{who} shared {name} with you.")
                } else {
                    format!("{who} changed what you may do with {name}.")
                }
            } else {
                let title = n["event"]["title"]
                    .as_str()
                    .or(n["eventPatch"]["title"].as_str())
                    .unwrap_or("an event");
                match n["type"].as_str() {
                    Some("created") => format!("{who} added {title}."),
                    Some("destroyed") => format!("{who} removed {title}."),
                    _ => format!("{who} changed {title}."),
                }
            };
            out.push(Notice {
                id: n["id"].as_str().unwrap_or_default().to_owned(),
                kind: kind.to_owned(),
                text,
            });
        }
    }
    Ok(out)
}
/// Dismisses notices once shown (`Foo/set destroy`).
pub fn dismiss_notices(s: &Session, notices: &[Notice]) -> Result<()> {
    for (kind, account) in [
        ("CalendarEventNotification", s.calendars_account()),
        ("ShareNotification", s.principals_account()),
    ] {
        let ids: Vec<&str> = notices
            .iter()
            .filter(|n| n.kind == kind)
            .map(|n| n.id.as_str())
            .collect();
        let Some(account) = account else { continue };
        // RFC 8620 §5.3: at most `maxObjectsInSet` in one /set.
        for chunk in ids.chunks(s.max_set()) {
            s.call_one(&format!("{kind}/set"), json!({ "accountId": account, "destroy": chunk }))?;
        }
    }
    Ok(())
}

/// Renames a calendar (and changes its colour; empty: as it is).
pub fn update_calendar(s: &Session, id: &str, name: &str, color: &str) -> Result<()> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let mut patch = json!({ "name": name });
    if !color.is_empty() {
        patch["color"] = json!(color);
    }
    let v = s.call_one(
        "Calendar/set",
        json!({ "accountId": account, "update": { id: patch } }),
    )?;
    if let Some(e) = v["notUpdated"].get(id) {
        bail!("The server didn't rename the calendar: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(())
}
/// Deletes a calendar with its events (`onDestroyRemoveEvents`).
pub fn delete_calendar(s: &Session, id: &str) -> Result<()> {
    let account = s.calendars_account().context("This JMAP server has no calendars.")?;
    let v = s.call_one(
        "Calendar/set",
        json!({ "accountId": account, "destroy": [id], "onDestroyRemoveEvents": true }),
    )?;
    if let Some(e) = v["notDestroyed"].get(id)
        && e["type"] != "notFound"
    {
        bail!("The server didn't delete the calendar: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(())
}
/// Renames an address book.
pub fn rename_address_book(s: &Session, id: &str, name: &str) -> Result<()> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let v = s.call_one(
        "AddressBook/set",
        json!({ "accountId": account, "update": { id: { "name": name } } }),
    )?;
    if let Some(e) = v["notUpdated"].get(id) {
        bail!("The server didn't rename the address book: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(())
}
/// Deletes an address book with its cards (`onDestroyRemoveContents`).
pub fn delete_address_book(s: &Session, id: &str) -> Result<()> {
    let account = s.contacts_account().context("This JMAP server has no contacts.")?;
    let v = s.call_one(
        "AddressBook/set",
        json!({ "accountId": account, "destroy": [id], "onDestroyRemoveContents": true }),
    )?;
    if let Some(e) = v["notDestroyed"].get(id)
        && e["type"] != "notFound"
    {
        bail!("The server didn't delete the address book: {}", e["type"].as_str().unwrap_or("refused"));
    }
    Ok(())
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

    /// A day that isn't ASCII is left out, not cut inside a character.
    #[test]
    fn non_ascii_days_do_not_panic() {
        let _ = write_rrule("FREQ=WEEKLY;BYDAY=€,MO", "");
        let _ = write_rrule("FREQ=WEEKLY;BYDAY=1€", "");
    }

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
