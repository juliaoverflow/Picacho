//! When a message says a parcel was last updated.
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// How long before a message an update it tells of may be (the message
/// may be a weekly digest), and how far after it, for clocks and time
/// zones that disagree.
const UPDATE_BEFORE: i64 = 90 * 86_400;
const UPDATE_AFTER: i64 = 86_400;

pub(super) fn plausible(t: i64, sent: i64) -> bool {
    sent <= 0 || (sent - UPDATE_BEFORE..=sent + UPDATE_AFTER).contains(&t)
}

/// "Updated: 2026-03-04 10:22", "Last update on March 4, 2026 at 10:22 AM",
/// "Status as of 4 March 2026", "Aktualisiert am 04.03.2026": the date a
/// shipping message says it was updated.
static UPDATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:last\s+update[ds]?|updated|status\s+as\s+of|as\s+of|aktualisiert(?:\s+am)?|stand|actualizado(?:\s+el)?|mis\s+à\s+jour(?:\s+le)?)(?:\s+(?:on|at))?\s*[:\-]?\s*(\d{4}-\d{2}-\d{2}(?:[T ]\d{1,2}:\d{2}(?::\d{2})?)?|[a-z]{3,9}\.?\s+\d{1,2},?\s+\d{4}(?:,?\s+(?:at\s+)?\d{1,2}:\d{2}\s*(?:[ap]\.?m\.?)?)?|\d{1,2}\.?\s+[a-z]{3,9}\.?\s+\d{4}(?:,?\s+\d{1,2}:\d{2})?|\d{1,2}\.\d{1,2}\.\d{4}(?:,?\s+\d{1,2}:\d{2})?)",
    )
    .expect("pattern")
});
/// A time's AM or PM written against its digits ("10:22AM").
static MERIDIEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d)([ap]m)$").expect("pattern"));

/// The update date a message's text gives, read in this computer's time
/// zone, if it is plausible for a message sent at `sent`.
pub fn update_date(text: &str, sent: i64) -> Option<i64> {
    UPDATED
        .captures_iter(text)
        .filter_map(|c| parse_date(&c[1]))
        .filter(|&t| plausible(t, sent))
        .max()
}

/// A date (and time) as mail writes it, as Unix seconds: ISO 8601 (with or
/// without a zone), "March 4, 2026 10:22 AM", "4 March 2026", "04.03.2026".
fn parse_date(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp());
    }
    let s = s
        .replace(" at ", " ")
        .replace(',', "")
        .replace("a.m.", "AM")
        .replace("p.m.", "PM")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // "Mar. 4", "4. März": the abbreviation's or ordinal's dot goes.
    let s = s.replace(". ", " ");
    let s = MERIDIEM
        .replace(s.trim_end_matches('.'), "$1 $2")
        .into_owned();
    const DATETIMES: [&str; 11] = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%B %d %Y %I:%M %p",
        "%b %d %Y %I:%M %p",
        "%B %d %Y %H:%M",
        "%b %d %Y %H:%M",
        "%d %B %Y %H:%M",
        "%d %b %Y %H:%M",
        "%d.%m.%Y %H:%M",
    ];
    for f in DATETIMES {
        if let Ok(t) = NaiveDateTime::parse_from_str(&s, f) {
            return local(t);
        }
    }
    for f in [
        "%Y-%m-%d", "%B %d %Y", "%b %d %Y", "%d %B %Y", "%d %b %Y", "%d.%m.%Y",
    ] {
        if let Ok(d) = NaiveDate::parse_from_str(&s, f) {
            return local(d.and_hms_opt(12, 0, 0)?);
        }
    }
    None
}

/// A time on this computer's clock, as Unix seconds.
pub(super) fn local(t: NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&t)
        .earliest()
        .map(|t| t.timestamp())
}

/// When a delivery event, the markup or its order says it was updated.
pub(super) fn markup_updated(value: &Value) -> Option<i64> {
    let event = value.get("deliveryStatus").filter(|v| v.is_object());
    let keys = [
        "dateTime",
        "startDate",
        "availableFrom",
        "eventDate",
        "date",
    ];
    event
        .and_then(|e| keys.iter().find_map(|k| e.get(*k)?.as_str()))
        .or_else(|| value.get("dateModified")?.as_str())
        .or_else(|| value.get("partOfOrder")?.get("dateModified")?.as_str())
        .and_then(parse_date)
}

/// "Expected Delivery on Friday, October 30, 2026", "Estimated delivery:
/// Mar 6", "Scheduled delivery date Tuesday, 03/10/2026".
static EXPECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:expected|estimated|scheduled|new)\s+delivery(?:\s+date)?\s*(?:on|by|is|:)?\s*(?:[a-z]+day,?\s+)?([a-z]{3,9}\.?\s+\d{1,2}(?:,?\s+\d{4})?|\d{1,2}/\d{1,2}/\d{4})",
    )
    .expect("pattern")
});
/// Phrases giving the day it arrives by the day the message was sent.
static WHEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:arriving|arrives|delivery|delivered|out for delivery)\s+(today|tomorrow)\b|\bout for delivery\b",
    )
    .expect("pattern")
});

/// When a message says its parcel should arrive, as an ISO 8601 date: the
/// date it gives, or the day it was sent ("arriving today") or the next.
pub(super) fn expected_date(subject: &str, text: &str, sent: i64) -> Option<String> {
    let head: String = text.chars().take(2000).collect();
    let sent_day = DateTime::from_timestamp(sent, 0)?
        .with_timezone(&Local)
        .date_naive();
    for s in [subject, head.as_str()] {
        if let Some(c) = EXPECTED.captures(s) {
            let raw = c[1].replace(['.', ','], "");
            let raw = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            let full = ["%B %d %Y", "%b %d %Y", "%m/%d/%Y"]
                .iter()
                .find_map(|f| NaiveDate::parse_from_str(&raw, f).ok());
            // Without a year: the one that puts it nearest after sending.
            let date = full.or_else(|| {
                let with = |y: i32| {
                    ["%B %d %Y", "%b %d %Y"]
                        .iter()
                        .find_map(|f| NaiveDate::parse_from_str(&format!("{raw} {y}"), f).ok())
                };
                let this = with(chrono::Datelike::year(&sent_day))?;
                Some(if (sent_day - this).num_days() > 180 {
                    with(chrono::Datelike::year(&sent_day) + 1)?
                } else {
                    this
                })
            });
            if let Some(d) = date
                && (d - sent_day).num_days().abs() <= 120
            {
                return Some(d.format("%Y-%m-%d").to_string());
            }
        }
    }
    for s in [subject, head.as_str()] {
        if let Some(c) = WHEN.captures(s) {
            let tomorrow = c
                .get(1)
                .is_some_and(|m| m.as_str().eq_ignore_ascii_case("tomorrow"));
            let d = sent_day + chrono::Duration::days(i64::from(tomorrow));
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    None
}
