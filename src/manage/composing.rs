//! Application-wide composing preferences, saved as one Store preference.
use serde::{Deserialize, Serialize};

/// Where a reply to the open message is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPlacement {
    /// Below the message in the reader, like a conversation.
    #[default]
    Inline,
    /// In a compose window of its own.
    Window,
}

/// Where new messages open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeOpen {
    /// A panel floating over the lower right of the main window.
    #[default]
    Floating,
    /// Filling the main window.
    Maximized,
    /// A separate window.
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposeOptions {
    pub reply_placement: ReplyPlacement,
    pub open_in: ComposeOpen,
    /// New messages start formatted (bold, lists, links…) rather than plain.
    pub formatted: bool,
    /// The reader's Reply button answers everyone.
    pub default_reply_all: bool,
    /// Replies and forwards include the original message.
    pub quote_original: bool,
    pub signature: String,
    /// Add the signature to replies and forwards, not only new messages.
    pub signature_on_replies: bool,
    /// Seconds a sent message waits, with an Undo, before it leaves. 0 sends
    /// at once.
    pub undo_send_seconds: u32,
    /// Ask before sending a message that mentions an attachment without one.
    pub attachment_reminder: bool,
    /// Ask before sending a message without a subject.
    pub warn_empty_subject: bool,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        Self {
            reply_placement: ReplyPlacement::Inline,
            open_in: ComposeOpen::Floating,
            formatted: true,
            default_reply_all: false,
            quote_original: true,
            signature: String::new(),
            signature_on_replies: true,
            undo_send_seconds: 5,
            attachment_reminder: true,
            warn_empty_subject: true,
        }
    }
}

pub const PREFERENCE: &str = "compose_options";

/// Words that suggest the writer meant to attach something.
pub fn mentions_attachment(body: &str) -> bool {
    // Quoted text is someone else's; only what was written now counts.
    let written: String = body
        .lines()
        .take_while(|l| !(l.starts_with("On ") && l.trim_end().ends_with("wrote:")))
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    [
        "attached",
        "attachment",
        "enclosed",
        "i've attached",
        "see attached",
    ]
    .iter()
    .any(|w| written.contains(w))
}

/// Send Later's choices at `now`: (label, when). Later today (in three
/// hours, rounded to the hour) while the day has room for it, tomorrow
/// morning and afternoon, and next Monday morning.
pub fn later_choices<Tz: chrono::TimeZone>(now: chrono::DateTime<Tz>) -> Vec<(String, i64)> {
    use chrono::{Datelike, Days, Timelike};
    let at = |days: u64, hour: u32| -> Option<i64> {
        let date = now.date_naive().checked_add_days(Days::new(days))?;
        let time = date.and_hms_opt(hour, 0, 0)?;
        now.timezone()
            .from_local_datetime(&time)
            .earliest()
            .map(|t| t.timestamp())
    };
    let mut out = Vec::new();
    if now.hour() < 17
        && let Some(t) = at(0, now.hour() + 3)
    {
        out.push(("Later today".to_owned(), t));
    }
    out.extend(at(1, 8).map(|t| ("Tomorrow morning".to_owned(), t)));
    out.extend(at(1, 13).map(|t| ("Tomorrow afternoon".to_owned(), t)));
    // Next Monday (a week on, if today is Monday); tomorrow already covers
    // a Sunday.
    let to_monday = (7 - now.weekday().num_days_from_monday()) as u64;
    if to_monday > 1 {
        out.extend(at(to_monday, 8).map(|t| ("Monday morning".to_owned(), t)));
    }
    out
}

/// A time typed for Send Later: "2026-11-20 14:30", "2026-11-20" (8:00),
/// "14:30" or "2:30 pm" (today, or tomorrow once past), or "tomorrow 9:00".
pub fn parse_later<Tz: chrono::TimeZone>(text: &str, now: chrono::DateTime<Tz>) -> Option<i64> {
    use chrono::{Days, NaiveDate, NaiveTime};
    let text = text.trim().to_lowercase();
    let time = |t: &str| -> Option<NaiveTime> {
        let t = t.trim().replace('.', ":");
        ["%H:%M", "%H", "%I:%M %p", "%I:%M%p", "%I %p", "%I%p"]
            .iter()
            .find_map(|f| NaiveTime::parse_from_str(&t, f).ok())
    };
    let local = |date: NaiveDate, at: NaiveTime| {
        now.timezone()
            .from_local_datetime(&date.and_time(at))
            .earliest()
            .map(|t| t.timestamp())
    };
    let today = now.date_naive();
    let eight = NaiveTime::from_hms_opt(8, 0, 0)?;
    if let Some(rest) = text.strip_prefix("tomorrow") {
        let at = if rest.trim().is_empty() {
            eight
        } else {
            time(
                rest.trim_start_matches([' ', ','])
                    .trim_start_matches("at ")
                    .trim(),
            )?
        };
        return local(today.checked_add_days(Days::new(1))?, at);
    }
    // A date, with or without a time.
    let (date, rest) = match text.split_once([' ', 'T', 't']) {
        Some((d, r)) => (d, r),
        None => (text.as_str(), ""),
    };
    if let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        let at = if rest.trim().is_empty() {
            eight
        } else {
            time(rest)?
        };
        return local(date, at);
    }
    // A time alone: the next one.
    let at = time(&text)?;
    let candidate = local(today, at)?;
    if candidate > now.timestamp() {
        Some(candidate)
    } else {
        local(today.checked_add_days(Days::new(1))?, at)
    }
}

/// When a scheduled message goes, as a sentence part: "today at 15:00",
/// "tomorrow at 8:00", "Mon, Nov 24 at 8:00".
pub fn describe_later<Tz: chrono::TimeZone>(when: i64, now: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Days;
    let Some(at) =
        chrono::DateTime::from_timestamp(when, 0).map(|t| t.with_timezone(&now.timezone()))
    else {
        return "later".into();
    };
    let time = at.format("%-H:%M");
    let day = at.date_naive();
    let today = now.date_naive();
    if day == today {
        format!("today at {time}")
    } else if Some(day) == today.checked_add_days(Days::new(1)) {
        format!("tomorrow at {time}")
    } else {
        format!("{} at {time}", at.format("%a, %b %-d"))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    #[test]
    fn send_later_offers_sensible_times_and_reads_typed_ones() {
        let tz = chrono::FixedOffset::east_opt(3600).unwrap();
        // Wednesday, November 19 2025, 10:20.
        let now = tz.with_ymd_and_hms(2025, 11, 19, 10, 20, 0).unwrap();
        let at = |d: u32, h: u32, m: u32| {
            tz.with_ymd_and_hms(2025, 11, d, h, m, 0)
                .unwrap()
                .timestamp()
        };
        let choices = super::later_choices(now);
        let labels: Vec<&str> = choices.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Later today",
                "Tomorrow morning",
                "Tomorrow afternoon",
                "Monday morning"
            ]
        );
        assert_eq!(choices[0].1, at(19, 13, 0));
        assert_eq!(choices[1].1, at(20, 8, 0));
        assert_eq!(choices[3].1, at(24, 8, 0));
        // In the evening, later today isn't offered.
        let evening = tz.with_ymd_and_hms(2025, 11, 19, 20, 0, 0).unwrap();
        assert_eq!(super::later_choices(evening)[0].0, "Tomorrow morning");
        // Typed times.
        assert_eq!(
            super::parse_later("2025-11-21 14:30", now),
            Some(at(21, 14, 30))
        );
        assert_eq!(super::parse_later("2025-11-21", now), Some(at(21, 8, 0)));
        assert_eq!(super::parse_later("15:45", now), Some(at(19, 15, 45)));
        assert_eq!(
            super::parse_later("9:00", now),
            Some(at(20, 9, 0)),
            "past today: tomorrow"
        );
        assert_eq!(super::parse_later("2:30 pm", now), Some(at(19, 14, 30)));
        assert_eq!(
            super::parse_later("tomorrow 7:15", now),
            Some(at(20, 7, 15))
        );
        assert_eq!(super::parse_later("Tomorrow", now), Some(at(20, 8, 0)));
        assert_eq!(super::parse_later("next week", now), None);
        assert_eq!(super::describe_later(at(19, 15, 0), now), "today at 15:00");
        assert_eq!(super::describe_later(at(20, 8, 0), now), "tomorrow at 8:00");
        assert_eq!(
            super::describe_later(at(24, 8, 0), now),
            "Mon, Nov 24 at 8:00"
        );
    }

    use super::*;

    #[test]
    fn older_or_partial_preferences_take_defaults() {
        let o: ComposeOptions = serde_json::from_str(r#"{"signature":"— Sam"}"#).unwrap();
        assert_eq!(o.signature, "— Sam");
        assert_eq!(o.reply_placement, ReplyPlacement::Inline);
        assert_eq!(o.undo_send_seconds, 5);
    }

    #[test]
    fn attachment_reminder_ignores_quoted_text() {
        assert!(mentions_attachment("Hi, the report is attached."));
        assert!(!mentions_attachment(
            "Thanks!\n\nOn Mon, Ana wrote:\n> see the attached file"
        ));
        assert!(!mentions_attachment("Thanks!\n> attachment below"));
    }
}
