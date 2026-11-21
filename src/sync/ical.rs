//! iCalendar (RFC 5545): events read from invitations and calendars, and
//! the iMIP replies (RFC 6047) that answer an invitation.
//!
//! Only what Picacho shows and answers is read: an event's summary, time,
//! place, organizer, attendees and description, and the method (REQUEST,
//! CANCEL, REPLY…) of the object it came in.
use anyhow::{Result, ensure};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// How an attendee answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PartStat {
    Accepted,
    Tentative,
    Declined,
    NeedsAction,
}

impl PartStat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Tentative => "TENTATIVE",
            Self::Declined => "DECLINED",
            Self::NeedsAction => "NEEDS-ACTION",
        }
    }
    fn parse(text: &str) -> Self {
        match text.to_ascii_uppercase().as_str() {
            "ACCEPTED" => Self::Accepted,
            "TENTATIVE" => Self::Tentative,
            "DECLINED" => Self::Declined,
            _ => Self::NeedsAction,
        }
    }
    /// The word a reply's subject starts with.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Accepted => "Accepted",
            Self::Tentative => "Tentatively accepted",
            Self::Declined => "Declined",
            Self::NeedsAction => "Invitation",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attendee {
    pub email: String,
    pub name: String,
    pub status: Option<PartStat>,
}

/// One event.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    /// Start and end, in Unix seconds (UTC). All-day events start at
    /// midnight UTC of their date.
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub organizer: Option<Attendee>,
    pub attendees: Vec<Attendee>,
    /// SEQUENCE: which revision of the event this is.
    pub sequence: u32,
    /// RRULE, as written, for showing that it repeats.
    pub recurrence: String,
    pub cancelled: bool,
    /// The VEVENT as it came, for saving to a calendar.
    #[serde(default)]
    pub source: String,
    /// The start's time zone (TZID), so repeats keep their local time
    /// across daylight saving changes. Empty for UTC or floating times.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tzid: String,
    /// The start was written in UTC (a trailing Z); without a TZID too,
    /// a floating time is the reader's own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub utc: bool,
    /// Occurrences of a repeating event left out (EXDATE), as start times.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<i64>,
    /// Extra times a repeating event happens (RDATE), as start times.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rdates: Vec<i64>,
    /// For a changed occurrence of a repeating event: which occurrence it
    /// replaces (RECURRENCE-ID), as its original start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<i64>,
}

/// An iCalendar object: its METHOD and events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    pub method: String,
    pub events: Vec<Event>,
    /// The whole object as text, unfolded lines joined by CRLF.
    pub text: String,
}

/// Unfolds content lines (RFC 5545 §3.1).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
        } else if !line.is_empty() {
            lines.push(line.to_owned());
        }
    }
    lines
}

/// A content line's name, parameters and value. Colons inside quoted
/// parameter values don't end the name.
/// A content line: its name, parameters and value.
type Line = (String, Vec<(String, String)>, String);

fn split_line(line: &str) -> Option<Line> {
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.trim_matches('"').to_owned()))
        })
        .collect();
    Some((name, params, value.to_owned()))
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// A TEXT value with its escapes undone.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A value escaped as TEXT.
pub fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// Common Windows time zone names (Outlook's TZID) as IANA zones.
fn windows_zone(name: &str) -> Option<&'static str> {
    Some(match name {
        "Pacific Standard Time" => "America/Los_Angeles",
        "Mountain Standard Time" => "America/Denver",
        "US Mountain Standard Time" => "America/Phoenix",
        "Central Standard Time" => "America/Chicago",
        "Eastern Standard Time" => "America/New_York",
        "Atlantic Standard Time" => "America/Halifax",
        "Alaskan Standard Time" => "America/Anchorage",
        "Hawaiian Standard Time" => "Pacific/Honolulu",
        "GMT Standard Time" => "Europe/London",
        "Greenwich Standard Time" => "Atlantic/Reykjavik",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "FLE Standard Time" => "Europe/Kiev",
        "GTB Standard Time" => "Europe/Bucharest",
        "Russian Standard Time" => "Europe/Moscow",
        "India Standard Time" => "Asia/Kolkata",
        "China Standard Time" => "Asia/Shanghai",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "Korea Standard Time" => "Asia/Seoul",
        "Singapore Standard Time" => "Asia/Singapore",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "New Zealand Standard Time" => "Pacific/Auckland",
        "UTC" | "Coordinated Universal Time" => "UTC",
        _ => return None,
    })
}

/// The IANA name of a TZID parameter's zone, if it is one chrono-tz knows
/// (Outlook's Windows names are translated).
fn zone_name(params: &[(String, String)]) -> Option<String> {
    let tzid = param(params, "TZID")?.trim_start_matches('/');
    if tzid.parse::<chrono_tz::Tz>().is_ok() {
        return Some(tzid.to_owned());
    }
    windows_zone(tzid).map(str::to_owned)
}

/// A DATE or DATE-TIME value as Unix seconds, and whether it is a date.
fn parse_time(value: &str, params: &[(String, String)]) -> Option<(i64, bool)> {
    let value = value.trim();
    if param(params, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some((date.and_hms_opt(0, 0, 0)?.and_utc().timestamp(), true));
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((t.and_utc().timestamp(), false));
    }
    let t = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    if let Some(tzid) = param(params, "TZID") {
        let tzid = tzid.trim_start_matches('/');
        let zone: Option<chrono_tz::Tz> = tzid
            .parse()
            .ok()
            .or_else(|| windows_zone(tzid).and_then(|z| z.parse().ok()));
        if let Some(zone) = zone
            && let Some(local) = zone.from_local_datetime(&t).earliest()
        {
            return Some((local.with_timezone(&Utc).timestamp(), false));
        }
    }
    // Floating time: the reader's own zone.
    let local = chrono::Local.from_local_datetime(&t).earliest()?;
    Some((local.timestamp(), false))
}

fn attendee(value: &str, params: &[(String, String)]) -> Attendee {
    let email = value
        .trim()
        .trim_start_matches("mailto:")
        .trim_start_matches("MAILTO:")
        .to_owned();
    Attendee {
        name: param(params, "CN").unwrap_or_default().to_owned(),
        status: param(params, "PARTSTAT").map(PartStat::parse),
        email,
    }
}

/// Reads an iCalendar object. Events without a start are left out.
pub fn parse(text: &str) -> Calendar {
    let lines = unfold(text);
    let mut cal = Calendar {
        text: lines.join("\r\n") + "\r\n",
        ..Default::default()
    };
    let mut depth: Vec<String> = Vec::new();
    let mut event: Option<Event> = None;
    let mut source: Vec<String> = Vec::new();
    let mut end_duration: Option<i64> = None;
    for line in &lines {
        let Some((name, params, value)) = split_line(line) else {
            continue;
        };
        match name.as_str() {
            "BEGIN" => {
                let kind = value.to_ascii_uppercase();
                if kind == "VEVENT" && depth.last().is_some_and(|d| d == "VCALENDAR") {
                    event = Some(Event::default());
                    source.clear();
                    end_duration = None;
                }
                depth.push(kind);
            }
            "END" => {
                let kind = value.to_ascii_uppercase();
                if kind == "VEVENT"
                    && depth.len() == 2
                    && let Some(mut e) = event.take()
                {
                    source.push(line.clone());
                    e.source = source.join("\r\n") + "\r\n";
                    if e.end == 0 {
                        e.end = match end_duration {
                            Some(d) => e.start + d,
                            None if e.all_day => e.start + 86_400,
                            None => e.start,
                        };
                    }
                    if e.start != 0 {
                        cal.events.push(e);
                    }
                }
                depth.pop();
            }
            "METHOD" if depth.len() == 1 => cal.method = value.trim().to_ascii_uppercase(),
            _ => {}
        }
        // Properties of the event itself, not of an alarm inside it.
        if depth.last().is_some_and(|d| d == "VEVENT") && depth.len() == 2 {
            let Some(e) = event.as_mut() else { continue };
            source.push(line.clone());
            match name.as_str() {
                "UID" => e.uid = value.trim().to_owned(),
                "SUMMARY" => e.summary = unescape(&value),
                "LOCATION" => e.location = unescape(&value),
                "DESCRIPTION" => e.description = unescape(&value),
                "DTSTART" => {
                    if let Some((t, all_day)) = parse_time(&value, &params) {
                        e.start = t;
                        e.all_day = all_day;
                        e.tzid = zone_name(&params).unwrap_or_default();
                        e.utc = value.trim().ends_with('Z');
                    }
                }
                "EXDATE" => {
                    for v in value.split(',') {
                        if let Some((t, _)) = parse_time(v, &params) {
                            e.exdates.push(t);
                        }
                    }
                }
                "RDATE"
                    if !param(&params, "VALUE")
                        .is_some_and(|v| v.eq_ignore_ascii_case("PERIOD")) =>
                {
                    for v in value.split(',') {
                        if let Some((t, _)) = parse_time(v, &params) {
                            e.rdates.push(t);
                        }
                    }
                }
                "RECURRENCE-ID" => {
                    e.recurrence_id = parse_time(&value, &params).map(|(t, _)| t);
                }
                "DTEND" | "DUE" => {
                    if let Some((t, _)) = parse_time(&value, &params) {
                        e.end = t;
                    }
                }
                "DURATION" => end_duration = parse_duration(&value),
                "ORGANIZER" => e.organizer = Some(attendee(&value, &params)),
                "ATTENDEE" => e.attendees.push(attendee(&value, &params)),
                "SEQUENCE" => e.sequence = value.trim().parse().unwrap_or(0),
                "RRULE" => e.recurrence = value.trim().to_owned(),
                "STATUS" => e.cancelled |= value.trim().eq_ignore_ascii_case("CANCELLED"),
                _ => {}
            }
        }
    }
    if cal.method == "CANCEL" {
        for e in &mut cal.events {
            e.cancelled = true;
        }
    }
    cal
}

/// A DURATION value (P1DT2H30M, PT45M, P1W) in seconds.
fn parse_duration(value: &str) -> Option<i64> {
    let value = value.trim();
    let (sign, rest) = match value.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, value.trim_start_matches('+')),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(sign * total)
}

/// The calendar parts of a message: `text/calendar` parts anywhere in it,
/// and `.ics` attachments.
pub fn from_message(raw: &[u8]) -> Option<Calendar> {
    let parsed = mailparse::parse_mail(raw).ok()?;
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&parsed, &mut leaves);
    for leaf in leaves {
        let mime = leaf.ctype.mimetype.to_ascii_lowercase();
        let named_ics = leaf
            .get_content_disposition()
            .params
            .get("filename")
            .or_else(|| leaf.ctype.params.get("name"))
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".ics"));
        if mime == "text/calendar" || mime == "application/ics" || named_ics {
            let text = leaf.get_body().ok()?;
            let mut cal = parse(&text);
            if cal.method.is_empty()
                && let Some(method) = leaf.ctype.params.get("method")
            {
                cal.method = method.to_ascii_uppercase();
            }
            if !cal.events.is_empty() {
                return Some(cal);
            }
        }
    }
    None
}

fn stamp(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

fn when(t: i64, all_day: bool) -> String {
    if all_day {
        format!(
            "VALUE=DATE:{}",
            DateTime::from_timestamp(t, 0)
                .unwrap_or_default()
                .format("%Y%m%d")
        )
    } else {
        format!(":{}", stamp(t))
    }
}

/// Folds a content line at 75 octets (RFC 5545 §3.1).
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if count + len > 75 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += len;
    }
    out.push_str("\r\n");
    out
}

/// The weekdays RRULE names.
fn weekday(code: &str) -> Option<chrono::Weekday> {
    use chrono::Weekday::*;
    Some(match code {
        "MO" => Mon,
        "TU" => Tue,
        "WE" => Wed,
        "TH" => Thu,
        "FR" => Fri,
        "SA" => Sat,
        "SU" => Sun,
        _ => return None,
    })
}

/// A parsed RRULE (RFC 5545 §3.3.10), as far as Picacho expands it:
/// DAILY, WEEKLY, MONTHLY and YEARLY with INTERVAL, COUNT, UNTIL, BYDAY
/// (with ordinals for MONTHLY, as in "the second Tuesday"), BYMONTHDAY and
/// BYMONTH.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub freq: String,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<i64>,
    /// (ordinal, weekday): ordinal 0 for every such day.
    pub by_day: Vec<(i32, chrono::Weekday)>,
    pub by_month_day: Vec<i32>,
    pub by_month: Vec<u32>,
}

impl Rule {
    pub fn parse(text: &str) -> Option<Self> {
        let mut rule = Rule {
            interval: 1,
            ..Default::default()
        };
        for part in text.trim().split(';') {
            let (k, v) = part.split_once('=')?;
            match k.to_ascii_uppercase().as_str() {
                "FREQ" => rule.freq = v.to_ascii_uppercase(),
                "INTERVAL" => rule.interval = v.parse().ok().filter(|n| *n > 0)?,
                "COUNT" => rule.count = v.parse().ok(),
                "UNTIL" => rule.until = parse_time(v, &[]).map(|(t, _)| t),
                "BYDAY" => {
                    for d in v.split(',') {
                        let d = d.trim().to_ascii_uppercase();
                        let (n, code) = d.split_at(d.len().saturating_sub(2));
                        rule.by_day.push((
                            if n.is_empty() { 0 } else { n.parse().ok()? },
                            weekday(code)?,
                        ));
                    }
                }
                "BYMONTHDAY" => {
                    rule.by_month_day = v.split(',').filter_map(|d| d.trim().parse().ok()).collect()
                }
                "BYMONTH" => {
                    rule.by_month = v.split(',').filter_map(|d| d.trim().parse().ok()).collect()
                }
                // WKST and the rest change nothing Picacho expands.
                _ => {}
            }
        }
        matches!(
            rule.freq.as_str(),
            "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY"
        )
        .then_some(rule)
    }

    /// Whether Picacho can say when this rule's repeats fall.
    pub fn understood(text: &str) -> bool {
        let upper = text.to_ascii_uppercase();
        Rule::parse(text).is_some()
            && ![
                "BYSETPOS",
                "BYYEARDAY",
                "BYWEEKNO",
                "BYHOUR",
                "BYMINUTE",
                "BYSECOND",
            ]
            .iter()
            .any(|k| upper.contains(k))
    }

    /// Whether the rule can be expanded at all (by the full RFC 5545
    /// expander), even if `describe` only says it repeats.
    pub fn expandable(text: &str) -> bool {
        text.parse::<rrule::RRule<rrule::Unvalidated>>().is_ok()
    }

    /// A short description: "Every week on Monday, Wednesday", "Every 2 days".
    pub fn describe(&self) -> String {
        let unit = match self.freq.as_str() {
            "DAILY" => "day",
            "WEEKLY" => "week",
            "MONTHLY" => "month",
            _ => "year",
        };
        let mut text = if self.interval == 1 {
            format!("Every {unit}")
        } else {
            format!("Every {} {unit}s", self.interval)
        };
        if !self.by_day.is_empty() {
            let days: Vec<String> = self
                .by_day
                .iter()
                .map(|(n, d)| {
                    let name = match d {
                        chrono::Weekday::Mon => "Monday",
                        chrono::Weekday::Tue => "Tuesday",
                        chrono::Weekday::Wed => "Wednesday",
                        chrono::Weekday::Thu => "Thursday",
                        chrono::Weekday::Fri => "Friday",
                        chrono::Weekday::Sat => "Saturday",
                        chrono::Weekday::Sun => "Sunday",
                    };
                    match n {
                        0 => name.to_owned(),
                        -1 => format!("the last {name}"),
                        1 => format!("the first {name}"),
                        2 => format!("the second {name}"),
                        3 => format!("the third {name}"),
                        4 => format!("the fourth {name}"),
                        n => format!("{name} #{n}"),
                    }
                })
                .collect();
            text.push_str(&format!(" on {}", days.join(", ")));
        }
        if let Some(n) = self.count {
            text.push_str(&format!(", {n} times"));
        } else if let Some(u) = self.until {
            let d = DateTime::from_timestamp(u, 0).unwrap_or_default();
            text.push_str(&format!(", until {}", d.format("%b %-d, %Y")));
        }
        text
    }
}

/// The days of `month` (a date in it) that `rule` picks, in order.
fn month_days(rule: &Rule, month: NaiveDate, start: NaiveDate) -> Vec<NaiveDate> {
    use chrono::Datelike;
    let first = month.with_day(1).unwrap_or(month);
    let next = first
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(first);
    let days: Vec<NaiveDate> = first.iter_days().take_while(|d| *d < next).collect();
    let mut out: Vec<NaiveDate> = Vec::new();
    if !rule.by_month_day.is_empty() {
        for &n in &rule.by_month_day {
            let picked = if n > 0 {
                days.get(n as usize - 1)
            } else {
                days.len()
                    .checked_sub((-n) as usize)
                    .and_then(|i| days.get(i))
            };
            out.extend(picked);
        }
    } else if !rule.by_day.is_empty() {
        for &(n, wd) in &rule.by_day {
            let matching: Vec<&NaiveDate> = days.iter().filter(|d| d.weekday() == wd).collect();
            match n {
                0 => out.extend(matching.into_iter().copied()),
                n if n > 0 => out.extend(matching.get(n as usize - 1).copied().copied()),
                n => out.extend(
                    matching
                        .len()
                        .checked_sub((-n) as usize)
                        .and_then(|i| matching.get(i))
                        .copied()
                        .copied(),
                ),
            }
        }
    } else {
        out.extend(days.get(start.day() as usize - 1));
    }
    out.sort();
    out.dedup();
    out
}

/// The local dates `rule` repeats on from `start` (inclusive), in order,
/// until one is after `to` or the rule ends. At most `limit` of them.
fn dates(rule: &Rule, start: NaiveDate, to: NaiveDate, limit: usize) -> Vec<NaiveDate> {
    use chrono::Datelike;
    let mut out = Vec::new();
    let interval = rule.interval.max(1);
    let in_month = |d: &NaiveDate| rule.by_month.is_empty() || rule.by_month.contains(&d.month());
    let mut step: u32 = 0;
    // Each period's dates; a period with none still counts toward the interval.
    'periods: for _ in 0..(limit * 8).max(400) {
        let candidates: Vec<NaiveDate> = match rule.freq.as_str() {
            "DAILY" => start
                .checked_add_days(chrono::Days::new(u64::from(step)))
                .into_iter()
                .collect(),
            "WEEKLY" => {
                let week = start
                    .checked_add_days(chrono::Days::new(u64::from(step) * 7))
                    .unwrap_or(start);
                if rule.by_day.is_empty() {
                    vec![week]
                } else {
                    let monday =
                        week - chrono::Days::new(u64::from(week.weekday().num_days_from_monday()));
                    let mut ds: Vec<NaiveDate> = rule
                        .by_day
                        .iter()
                        .filter_map(|(_, wd)| {
                            monday.checked_add_days(chrono::Days::new(u64::from(
                                wd.num_days_from_monday(),
                            )))
                        })
                        .collect();
                    ds.sort();
                    ds
                }
            }
            "MONTHLY" => match start
                .with_day(1)
                .and_then(|d| d.checked_add_months(chrono::Months::new(step)))
            {
                Some(month) => month_days(rule, month, start),
                None => break,
            },
            _ => {
                let Some(year) = start
                    .with_day(1)
                    .and_then(|d| d.checked_add_months(chrono::Months::new(step * 12)))
                else {
                    break;
                };
                if rule.by_month.is_empty()
                    && rule.by_day.is_empty()
                    && rule.by_month_day.is_empty()
                {
                    // The same day each year; Feb 29 only in leap years.
                    NaiveDate::from_ymd_opt(year.year(), start.month(), start.day())
                        .into_iter()
                        .collect()
                } else {
                    let months: Vec<u32> = if rule.by_month.is_empty() {
                        vec![start.month()]
                    } else {
                        rule.by_month.clone()
                    };
                    months
                        .iter()
                        .filter_map(|m| NaiveDate::from_ymd_opt(year.year(), *m, 1))
                        .flat_map(|m| month_days(rule, m, start))
                        .collect()
                }
            }
        };
        step += interval;
        for d in candidates {
            if d < start || !in_month(&d) {
                continue;
            }
            if d > to || rule.count.is_some_and(|c| out.len() as u32 >= c) || out.len() >= limit {
                break 'periods;
            }
            out.push(d);
        }
        if rule.freq == "DAILY"
            && start
                .checked_add_days(chrono::Days::new(u64::from(step)))
                .is_none_or(|d| d > to)
        {
            break;
        }
    }
    out
}

/// Each time a repeating event happens between `from` and `to` (Unix
/// seconds), as its own event with that start and end: those left out
/// (EXDATE) are skipped, and those changed on their own (`overrides`,
/// matched by RECURRENCE-ID) are shown changed. An event that doesn't
/// repeat is itself, if it falls in the range.
pub fn occurrences(series: &Event, overrides: &[Event], from: i64, to: i64) -> Vec<Event> {
    let length = series.end - series.start;
    let overlaps = |e: &Event| e.end.max(e.start + 1) > from && e.start < to;
    if series.recurrence.trim().is_empty() && series.rdates.is_empty() {
        return overlaps(series)
            .then(|| series.clone())
            .into_iter()
            .collect();
    }
    let starts = full_starts(series, from - length.max(0), to)
        .or_else(|| simple_starts(series, to))
        .unwrap_or_else(|| vec![series.start]);
    let mut out = Vec::new();
    for start in starts {
        if series.exdates.contains(&start) {
            continue;
        }
        let occurrence = match overrides.iter().find(|o| o.recurrence_id == Some(start)) {
            Some(changed) if changed.cancelled => continue,
            Some(changed) => changed.clone(),
            None => Event {
                start,
                end: start + length,
                recurrence_id: Some(start),
                ..series.clone()
            },
        };
        if overlaps(&occurrence) {
            out.push(occurrence);
        }
    }
    // A repeat moved into the range from outside it.
    for o in overrides {
        if !o.cancelled && overlaps(o) && !out.iter().any(|e| e.recurrence_id == o.recurrence_id) {
            out.push(o.clone());
        }
    }
    out.sort_by_key(|e| e.start);
    out
}

/// The start times of a repeating event between `from` and `to`, by the
/// full RFC 5545 expander (BYSETPOS, BYWEEKNO, BYYEARDAY, BYHOUR… and
/// RDATE included). `None` when the rule can't be read that way.
fn full_starts(series: &Event, from: i64, to: i64) -> Option<Vec<i64>> {
    use chrono::TimeZone;
    let zone: rrule::Tz = if series.all_day || series.utc {
        rrule::Tz::UTC
    } else {
        match series.tzid.parse::<chrono_tz::Tz>() {
            Ok(z) => z.into(),
            Err(_) => rrule::Tz::LOCAL,
        }
    };
    let at = |t: i64| -> Option<DateTime<rrule::Tz>> { zone.timestamp_opt(t, 0).single() };
    let start = at(series.start)?;
    let mut set = rrule::RRuleSet::new(start);
    if !series.recurrence.trim().is_empty() {
        let rule: rrule::RRule<rrule::Unvalidated> = series.recurrence.trim().parse().ok()?;
        set = set.rrule(rule.validate(start).ok()?);
    }
    for t in &series.rdates {
        set = set.rdate(at(*t)?);
    }
    // The first time counts, as RFC 5545 says (DTSTART is an occurrence).
    let to = to.min(series.start + 100 * 366 * 86_400);
    let set = set.after(at(from.max(series.start) - 1)?).before(at(to)?);
    let found = set.all(5000);
    let mut starts: Vec<i64> = found.dates.iter().map(|d| d.timestamp()).collect();
    if series.start >= from && series.start < to && !starts.contains(&series.start) {
        starts.insert(0, series.start);
    }
    Some(starts)
}

/// The start times by Picacho's own expander, for rules the full one
/// refuses (it is strict about some combinations servers still send).
fn simple_starts(series: &Event, to: i64) -> Option<Vec<i64>> {
    use chrono::TimeZone;
    let rule = Rule::parse(&series.recurrence)?;
    let zone: Option<chrono_tz::Tz> = series.tzid.parse().ok();
    // The start's wall clock, in its own zone (all-day events: the date).
    let (local_start, time) = if series.all_day {
        let d = DateTime::from_timestamp(series.start, 0)
            .unwrap_or_default()
            .naive_utc();
        (d.date(), d.time())
    } else {
        let t = DateTime::from_timestamp(series.start, 0).unwrap_or_default();
        let local = match zone {
            Some(z) => t.with_timezone(&z).naive_local(),
            None if series.utc => t.naive_utc(),
            None => t.with_timezone(&chrono::Local).naive_local(),
        };
        (local.date(), local.time())
    };
    // Far ends of a range are cut to a century ahead.
    let to = to.min(series.start + 100 * 366 * 86_400);
    let to_date = DateTime::from_timestamp(to, 0)
        .unwrap_or_default()
        .date_naive()
        + chrono::Days::new(2);
    let utc_start = zone.is_none() && series.utc;
    let mut out = Vec::new();
    for date in dates(&rule, local_start, to_date, 2000) {
        let naive = date.and_time(time);
        let start = if series.all_day || utc_start {
            naive.and_utc().timestamp()
        } else {
            match zone {
                Some(z) => z
                    .from_local_datetime(&naive)
                    .earliest()
                    .map(|t| t.timestamp()),
                None => chrono::Local
                    .from_local_datetime(&naive)
                    .earliest()
                    .map(|t| t.timestamp()),
            }
            .unwrap_or_else(|| naive.and_utc().timestamp())
        };
        if rule.until.is_some_and(|u| start > u) || start >= to {
            break;
        }
        out.push(start);
    }
    Some(out)
}

/// A calendar object with a repeating event and its changed occurrences,
/// written from their fields (`series` first).
pub fn to_store_all(series: &Event, overrides: &[Event]) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    out.push_str(concat!(
        "PRODID:-//Pinal Technologies//Picacho ",
        env!("CARGO_PKG_VERSION"),
        "//EN\r\n"
    ));
    out.push_str(&write_event(series));
    for o in overrides {
        out.push_str(&write_event(o));
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// When an event is, for an invitation's text: in the event's own zone
/// when it has one, else in UTC, with the zone named.
pub fn describe_when(e: &Event) -> String {
    let zone: chrono_tz::Tz = e.tzid.parse().unwrap_or(chrono_tz::UTC);
    let at = |t: i64| {
        DateTime::from_timestamp(t, 0)
            .unwrap_or_default()
            .with_timezone(&zone)
    };
    let mut text = if e.all_day {
        let start = DateTime::from_timestamp(e.start, 0).unwrap_or_default();
        format!("{}, all day", start.format("%A, %B %-d, %Y"))
    } else {
        format!(
            "{} – {} ({})",
            at(e.start).format("%A, %B %-d, %Y, %H:%M"),
            at(e.end).format("%H:%M"),
            zone.name()
        )
    };
    if let Some(rule) = Rule::parse(&e.recurrence) {
        text.push_str(&format!("\r\n{}", rule.describe()));
    }
    text
}

/// An invitation (`METHOD:REQUEST`) or its cancellation (`METHOD:CANCEL`)
/// for an event you organize, with its changed repeats (RFC 5546 §3.2).
pub fn request(series: &Event, overrides: &[Event], cancel: bool) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    out.push_str(concat!(
        "PRODID:-//Pinal Technologies//Picacho ",
        env!("CARGO_PKG_VERSION"),
        "//EN\r\n"
    ));
    out.push_str(if cancel {
        "METHOD:CANCEL\r\n"
    } else {
        "METHOD:REQUEST\r\n"
    });
    let mut series = series.clone();
    series.cancelled |= cancel;
    out.push_str(&write_event(&series));
    if !cancel {
        for o in overrides {
            out.push_str(&write_event(o));
        }
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// The iMIP REPLY to an invitation: `me` answers `status` (RFC 5546 §3.2.3).
pub fn reply(event: &Event, me: &str, my_name: &str, status: PartStat) -> Result<String> {
    ensure!(
        !event.uid.is_empty(),
        "This invitation has no identifier to answer."
    );
    let organizer = event
        .organizer
        .as_ref()
        .filter(|o| !o.email.is_empty())
        .ok_or_else(|| anyhow::anyhow!("This invitation doesn't say who to answer."))?;
    let mut out = String::new();
    for line in [
        "BEGIN:VCALENDAR".to_owned(),
        concat!(
            "PRODID:-//Pinal Technologies//Picacho ",
            env!("CARGO_PKG_VERSION"),
            "//EN"
        )
        .to_owned(),
        "VERSION:2.0".to_owned(),
        "METHOD:REPLY".to_owned(),
        "BEGIN:VEVENT".to_owned(),
        format!("UID:{}", event.uid),
        format!("DTSTAMP:{}", stamp(Utc::now().timestamp())),
        format!("SEQUENCE:{}", event.sequence),
        format!(
            "DTSTART;{}",
            when(event.start, event.all_day).trim_start_matches(';')
        ),
        format!("SUMMARY:{}", escape(&event.summary)),
        format!(
            "ORGANIZER{}:mailto:{}",
            if organizer.name.is_empty() {
                String::new()
            } else {
                format!(";CN=\"{}\"", organizer.name.replace('"', "'"))
            },
            organizer.email
        ),
        format!(
            "ATTENDEE;PARTSTAT={}{}:mailto:{me}",
            status.as_str(),
            if my_name.is_empty() {
                String::new()
            } else {
                format!(";CN=\"{}\"", my_name.replace('"', "'"))
            }
        ),
        "END:VEVENT".to_owned(),
        "END:VCALENDAR".to_owned(),
    ] {
        // DTSTART with a date value is written `DTSTART;VALUE=DATE:…`.
        let line = line.replace("DTSTART;:", "DTSTART:");
        out.push_str(&fold(&line));
    }
    Ok(out)
}

/// A guest's proposal of another time (METHOD:COUNTER, RFC 5546 §3.2.7):
/// `me` asks the organizer to move `event` to `start`..`end`, with `note`.
pub fn counter(
    event: &Event,
    me: &str,
    my_name: &str,
    start: i64,
    end: i64,
    note: &str,
) -> Result<String> {
    ensure!(
        !event.uid.is_empty(),
        "This invitation has no identifier to answer."
    );
    ensure!(end >= start, "The time proposed ends before it starts.");
    let organizer = event
        .organizer
        .as_ref()
        .filter(|o| !o.email.is_empty())
        .ok_or_else(|| anyhow::anyhow!("This invitation doesn't say who to answer."))?;
    let proposed = Event {
        start,
        end,
        sequence: event.sequence,
        organizer: Some(organizer.clone()),
        attendees: vec![Attendee {
            email: me.to_owned(),
            name: my_name.to_owned(),
            status: Some(PartStat::Tentative),
        }],
        description: note.to_owned(),
        recurrence: String::new(),
        exdates: Vec::new(),
        source: String::new(),
        cancelled: false,
        ..event.clone()
    };
    Ok(method_object("COUNTER", &write_event(&proposed)))
}

/// The organizer's no to a proposed time (METHOD:DECLINECOUNTER).
pub fn decline_counter(event: &Event, guest: &str) -> String {
    let mut lines = vec![
        "BEGIN:VEVENT".to_owned(),
        format!("UID:{}", event.uid),
        format!("DTSTAMP:{}", stamp(Utc::now().timestamp())),
        format!("SEQUENCE:{}", event.sequence),
    ];
    if let Some(o) = event.organizer.as_ref() {
        lines.push(format!("ORGANIZER:mailto:{}", o.email));
    }
    lines.push(format!("ATTENDEE:mailto:{guest}"));
    lines.push("END:VEVENT".to_owned());
    method_object(
        "DECLINECOUNTER",
        &lines.iter().map(|l| fold(l)).collect::<String>(),
    )
}

fn method_object(method: &str, events: &str) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    out.push_str(concat!(
        "PRODID:-//Pinal Technologies//Picacho ",
        env!("CARGO_PKG_VERSION"),
        "//EN\r\n"
    ));
    out.push_str(&format!("METHOD:{method}\r\n"));
    out.push_str(events);
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// A calendar object holding `event` as it came (METHOD left out), for
/// saving to a calendar server.
pub fn to_store(event: &Event) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    out.push_str(concat!(
        "PRODID:-//Pinal Technologies//Picacho ",
        env!("CARGO_PKG_VERSION"),
        "//EN\r\n"
    ));
    if event.source.is_empty() {
        out.push_str(&write_event(event));
    } else {
        for line in event.source.split("\r\n").filter(|l| !l.is_empty()) {
            out.push_str(&fold(line));
        }
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// A VEVENT written from its fields, for events made or edited here.
pub fn write_event(e: &Event) -> String {
    let mut lines = vec![
        "BEGIN:VEVENT".to_owned(),
        format!("UID:{}", e.uid),
        format!("DTSTAMP:{}", stamp(Utc::now().timestamp())),
        format!("SEQUENCE:{}", e.sequence),
    ];
    let at = |name: &str, t: i64| -> String {
        if e.all_day {
            format!("{name};{}", when(t, true))
        } else if let Ok(zone) = e.tzid.parse::<chrono_tz::Tz>() {
            // Kept in its zone, so repeats keep their local time.
            let local = DateTime::from_timestamp(t, 0)
                .unwrap_or_default()
                .with_timezone(&zone);
            format!("{name};TZID={}:{}", e.tzid, local.format("%Y%m%dT%H%M%S"))
        } else {
            format!("{name}{}", when(t, false))
        }
    };
    if let Some(id) = e.recurrence_id.filter(|_| e.recurrence.is_empty()) {
        lines.push(at("RECURRENCE-ID", id));
    }
    lines.push(at("DTSTART", e.start));
    lines.push(at(
        "DTEND",
        if e.all_day {
            e.end.max(e.start + 86_400)
        } else {
            e.end.max(e.start)
        },
    ));
    for x in &e.exdates {
        lines.push(at("EXDATE", *x));
    }
    for x in &e.rdates {
        lines.push(at("RDATE", *x));
    }
    if e.cancelled {
        lines.push("STATUS:CANCELLED".to_owned());
    }
    lines.push(format!("SUMMARY:{}", escape(&e.summary)));
    if !e.location.is_empty() {
        lines.push(format!("LOCATION:{}", escape(&e.location)));
    }
    if !e.description.is_empty() {
        lines.push(format!("DESCRIPTION:{}", escape(&e.description)));
    }
    if !e.recurrence.is_empty() {
        lines.push(format!("RRULE:{}", e.recurrence));
    }
    let person = |p: &Attendee| -> String {
        if p.name.is_empty() {
            String::new()
        } else {
            format!(";CN=\"{}\"", p.name.replace('"', "'"))
        }
    };
    if let Some(o) = e.organizer.as_ref().filter(|o| !o.email.is_empty()) {
        lines.push(format!("ORGANIZER{}:mailto:{}", person(o), o.email));
    }
    for at in &e.attendees {
        lines.push(format!(
            "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT={};RSVP=TRUE{}:mailto:{}",
            at.status.unwrap_or(PartStat::NeedsAction).as_str(),
            person(at),
            at.email
        ));
    }
    lines.push("END:VEVENT".to_owned());
    lines.iter().map(|l| fold(l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Europe/Berlin:20261124T100000\r\nDTEND;TZID=Europe/Berlin:20261124T110000\r\nDTSTAMP:20261120T090000Z\r\nORGANIZER;CN=Ana López:mailto:ana@example.org\r\nUID:abc123@google.com\r\nATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;CN=qa\r\n @example.com:mailto:qa@example.com\r\nSUMMARY:Planning\\, Q1\r\nLOCATION:Room 4\r\nDESCRIPTION:Agenda:\\n1. Budget\r\nSEQUENCE:2\r\nBEGIN:VALARM\r\nTRIGGER:-P0DT0H10M0S\r\nDESCRIPTION:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn invitations_are_read() {
        let cal = parse(INVITE);
        assert_eq!(cal.method, "REQUEST");
        let e = &cal.events[0];
        assert_eq!(e.summary, "Planning, Q1");
        assert_eq!(e.location, "Room 4");
        assert_eq!(e.description, "Agenda:\n1. Budget");
        // 10:00 in Berlin in November is 09:00 UTC.
        assert_eq!(e.start, 1795510800);
        assert_eq!(e.end - e.start, 3600);
        assert_eq!(e.sequence, 2);
        assert_eq!(e.organizer.as_ref().unwrap().name, "Ana López");
        assert_eq!(e.attendees[0].email, "qa@example.com");
        assert_eq!(e.attendees[0].status, Some(PartStat::NeedsAction));
        assert!(!e.source.contains("VALARM") || e.source.contains("END:VEVENT"));
    }

    #[test]
    fn replies_answer_the_organizer() {
        let e = parse(INVITE).events.remove(0);
        let r = reply(&e, "qa@example.com", "QA", PartStat::Accepted).unwrap();
        let back = parse(&r);
        assert_eq!(back.method, "REPLY");
        assert_eq!(back.events[0].uid, "abc123@google.com");
        assert_eq!(back.events[0].attendees[0].status, Some(PartStat::Accepted));
        assert_eq!(back.events[0].start, e.start);
        assert!(r.lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn all_day_outlook_and_durations() {
        let text = "BEGIN:VCALENDAR\nMETHOD:CANCEL\nBEGIN:VEVENT\nUID:x\nDTSTART;VALUE=DATE:20261225\nSUMMARY:Holiday\nEND:VEVENT\nBEGIN:VEVENT\nUID:y\nDTSTART;TZID=\"Eastern Standard Time\":20260105T090000\nDURATION:PT1H30M\nEND:VEVENT\nEND:VCALENDAR\n";
        let cal = parse(text);
        assert!(cal.events.iter().all(|e| e.cancelled));
        assert!(cal.events[0].all_day);
        assert_eq!(cal.events[0].end - cal.events[0].start, 86_400);
        // 09:00 in New York in January is 14:00 UTC.
        assert_eq!(cal.events[1].start, 1767621600);
        assert_eq!(cal.events[1].end - cal.events[1].start, 5400);
    }

    fn rrule_event(lines: &str) -> Vec<Event> {
        parse(&format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:r\nSUMMARY:Standup\n{lines}\nEND:VEVENT\nEND:VCALENDAR\n")).events
    }
    fn day(t: i64, zone: &str) -> String {
        let z: chrono_tz::Tz = zone.parse().unwrap();
        DateTime::from_timestamp(t, 0)
            .unwrap()
            .with_timezone(&z)
            .format("%a %Y-%m-%d %H:%M")
            .to_string()
    }

    #[test]
    fn weekly_repeats_keep_their_local_time_across_daylight_saving() {
        let e = rrule_event("DTSTART;TZID=Europe/Berlin:20261019T090000\nDTEND;TZID=Europe/Berlin:20261019T093000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=5\nEXDATE;TZID=Europe/Berlin:20261021T090000").remove(0);
        assert_eq!(e.tzid, "Europe/Berlin");
        let all = occurrences(&e, &[], 0, i64::MAX / 2);
        let shown: Vec<String> = all.iter().map(|o| day(o.start, "Europe/Berlin")).collect();
        // Clocks go back on Oct 25: still 09:00 in Berlin. The 21st is left
        // out, and COUNT counts it.
        assert_eq!(
            shown,
            [
                "Mon 2026-10-19 09:00",
                "Mon 2026-10-26 09:00",
                "Wed 2026-10-28 09:00",
                "Mon 2026-11-02 09:00"
            ]
        );
        assert!(all.iter().all(|o| o.end - o.start == 1800));
        assert_eq!(
            Rule::parse(&e.recurrence).unwrap().describe(),
            "Every week on Monday, Wednesday, 5 times"
        );
    }

    #[test]
    fn monthly_yearly_and_daily_rules() {
        let second_tuesday = rrule_event(
            "DTSTART:20260113T170000Z\nRRULE:FREQ=MONTHLY;BYDAY=2TU;UNTIL=20260501T000000Z",
        )
        .remove(0);
        let shown: Vec<String> = occurrences(&second_tuesday, &[], 0, i64::MAX / 2)
            .iter()
            .map(|o| day(o.start, "UTC"))
            .collect();
        assert_eq!(
            shown,
            [
                "Tue 2026-01-13 17:00",
                "Tue 2026-02-10 17:00",
                "Tue 2026-03-10 17:00",
                "Tue 2026-04-14 17:00"
            ]
        );
        let last_day =
            rrule_event("DTSTART;VALUE=DATE:20260131\nRRULE:FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3")
                .remove(0);
        let shown: Vec<String> = occurrences(&last_day, &[], 0, i64::MAX / 2)
            .iter()
            .map(|o| day(o.start, "UTC"))
            .collect();
        assert_eq!(
            shown,
            [
                "Sat 2026-01-31 00:00",
                "Sat 2026-02-28 00:00",
                "Tue 2026-03-31 00:00"
            ]
        );
        let birthday = rrule_event("DTSTART;VALUE=DATE:20240229\nRRULE:FREQ=YEARLY").remove(0);
        let years: Vec<String> = occurrences(&birthday, &[], 0, 1_900_000_000)
            .iter()
            .map(|o| day(o.start, "UTC")[4..8].to_owned())
            .collect();
        assert_eq!(years, ["2024", "2028"]);
        let every_other =
            rrule_event("DTSTART:20260101T080000Z\nRRULE:FREQ=DAILY;INTERVAL=2").remove(0);
        // Only those in range.
        let jan = occurrences(
            &every_other,
            &[],
            1_767_225_600 + 86_400 * 9,
            1_767_225_600 + 86_400 * 15,
        );
        assert_eq!(
            jan.iter().map(|o| day(o.start, "UTC")).collect::<Vec<_>>(),
            [
                "Sun 2026-01-11 08:00",
                "Tue 2026-01-13 08:00",
                "Thu 2026-01-15 08:00"
            ]
        );
        assert!(!Rule::understood("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU"));
        assert!(Rule::understood("FREQ=WEEKLY;BYDAY=FR"));
    }

    #[test]
    fn unusual_rules_are_expanded_too() {
        let all = |lines: &str| -> Vec<String> {
            let e = rrule_event(lines).remove(0);
            occurrences(&e, &[], 0, i64::MAX / 2)
                .iter()
                .map(|o| day(o.start, "UTC"))
                .collect()
        };
        // The last weekday of each month (BYSETPOS).
        assert_eq!(
            all(
                "DTSTART:20260130T160000Z\nRRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=3"
            ),
            [
                "Fri 2026-01-30 16:00",
                "Fri 2026-02-27 16:00",
                "Tue 2026-03-31 16:00"
            ]
        );
        // Twice a day (BYHOUR).
        assert_eq!(
            all("DTSTART:20260105T090000Z\nRRULE:FREQ=DAILY;BYHOUR=9,17;COUNT=4"),
            [
                "Mon 2026-01-05 09:00",
                "Mon 2026-01-05 17:00",
                "Tue 2026-01-06 09:00",
                "Tue 2026-01-06 17:00"
            ]
        );
        // The 100th day of the year (BYYEARDAY), and week 20's Monday (BYWEEKNO).
        assert_eq!(
            all("DTSTART:20260410T080000Z\nRRULE:FREQ=YEARLY;BYYEARDAY=100;COUNT=2"),
            ["Fri 2026-04-10 08:00", "Sat 2027-04-10 08:00"]
        );
        assert_eq!(
            all("DTSTART:20260511T080000Z\nRRULE:FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO;COUNT=2"),
            ["Mon 2026-05-11 08:00", "Mon 2027-05-17 08:00"]
        );
        // Extra dates (RDATE), with the rule.
        assert_eq!(
            all("DTSTART:20260101T120000Z\nRRULE:FREQ=YEARLY;COUNT=2\nRDATE:20260704T120000Z"),
            [
                "Thu 2026-01-01 12:00",
                "Sat 2026-07-04 12:00",
                "Fri 2027-01-01 12:00"
            ]
        );
        assert!(Rule::expandable("FREQ=MONTHLY;BYSETPOS=-1;BYDAY=MO,TU"));
        assert!(!Rule::expandable("FREQ=SOMETIMES"));
    }

    #[test]
    fn changed_and_cancelled_occurrences_take_their_place() {
        let mut events = parse("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:r\nSUMMARY:Standup\nDTSTART:20260105T090000Z\nDTEND:20260105T091500Z\nRRULE:FREQ=DAILY;COUNT=3\nEND:VEVENT\nBEGIN:VEVENT\nUID:r\nRECURRENCE-ID:20260106T090000Z\nSUMMARY:Standup (late)\nDTSTART:20260106T100000Z\nDTEND:20260106T101500Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:r\nRECURRENCE-ID:20260107T090000Z\nSTATUS:CANCELLED\nDTSTART:20260107T090000Z\nEND:VEVENT\nEND:VCALENDAR\n").events;
        let series = events.remove(0);
        let all = occurrences(&series, &events, 0, i64::MAX / 2);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].summary, "Standup (late)");
        assert_eq!(day(all[1].start, "UTC"), "Tue 2026-01-06 10:00");
        // Written back whole, they read the same.
        let again = parse(&to_store_all(&series, &events)).events;
        let (s2, o2) = again.split_first().unwrap();
        let back = occurrences(s2, o2, 0, i64::MAX / 2);
        let when = |v: &[Event]| {
            v.iter()
                .map(|o| (o.start, o.end, o.summary.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(when(&back), when(&all));
        assert!(o2.iter().any(|o| o.cancelled));
    }

    #[test]
    fn guests_propose_other_times() {
        let e = parse(INVITE).events.remove(0);
        let text = counter(
            &e,
            "qa@example.com",
            "QA",
            e.start + 3600,
            e.end + 3600,
            "Later?",
        )
        .unwrap();
        let back = parse(&text);
        assert_eq!(back.method, "COUNTER");
        let p = &back.events[0];
        assert_eq!(
            (p.uid.as_str(), p.start, p.end),
            (e.uid.as_str(), e.start + 3600, e.end + 3600)
        );
        assert_eq!(p.attendees[0].email, "qa@example.com");
        assert_eq!(p.description, "Later?");
        assert_eq!(p.organizer.as_ref().unwrap().email, "ana@example.org");
        let no = parse(&decline_counter(&e, "qa@example.com"));
        assert_eq!(no.method, "DECLINECOUNTER");
        assert_eq!(no.events.len(), 0, "no start: not an event to show");
        assert!(decline_counter(&e, "qa@example.com").contains("ATTENDEE:mailto:qa@example.com"));
    }

    #[test]
    fn events_written_here_read_back() {
        let e = Event {
            uid: "1@picacho".into(),
            summary: "Lunch; with Bo".into(),
            location: "Café".into(),
            start: 1_800_000_000,
            end: 1_800_003_600,
            ..Default::default()
        };
        let back = parse(&to_store(&e)).events.remove(0);
        assert_eq!(
            (back.summary.as_str(), back.location.as_str()),
            ("Lunch; with Bo", "Café")
        );
        assert_eq!((back.start, back.end), (e.start, e.end));
    }
}
