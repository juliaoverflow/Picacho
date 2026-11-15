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

/// Snooze's choices at `now`: (label, when). Later today (in three hours)
/// and this evening while the day has room, tomorrow morning, this weekend
/// (Saturday morning, unless it is the weekend already) and next week
/// (Monday morning).
pub fn snooze_choices<Tz: chrono::TimeZone>(now: chrono::DateTime<Tz>) -> Vec<(String, i64)> {
    use chrono::{Datelike, Days, Timelike};
    let at = |days: u64, hour: u32| -> Option<i64> {
        let date = now.date_naive().checked_add_days(Days::new(days))?;
        now.timezone()
            .from_local_datetime(&date.and_hms_opt(hour, 0, 0)?)
            .earliest()
            .map(|t| t.timestamp())
    };
    let mut out = Vec::new();
    if now.hour() < 18 {
        out.extend(at(0, now.hour() + 3).map(|t| ("Later today".to_owned(), t)));
    }
    if now.hour() < 17 {
        out.extend(at(0, 19).map(|t| ("This evening".to_owned(), t)));
    }
    out.extend(at(1, 8).map(|t| ("Tomorrow".to_owned(), t)));
    let weekday = now.weekday().num_days_from_monday() as u64;
    if weekday < 4 {
        out.extend(at(5 - weekday, 8).map(|t| ("This weekend".to_owned(), t)));
    }
    out.extend(at(7 - weekday, 8).map(|t| ("Next week".to_owned(), t)));
    out.dedup_by_key(|(_, t)| *t);
    out
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
    fn send_later_offers_sensible_times() {
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
        assert_eq!(super::describe_later(at(19, 15, 0), now), "today at 15:00");
        assert_eq!(super::describe_later(at(20, 8, 0), now), "tomorrow at 8:00");
        assert_eq!(
            super::describe_later(at(24, 8, 0), now),
            "Mon, Nov 24 at 8:00"
        );
    }

    use super::*;

    #[test]
    fn snooze_offers_sensible_times() {
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        // Wednesday, November 19 2025, 10:20.
        let now = tz.with_ymd_and_hms(2025, 11, 19, 10, 20, 0).unwrap();
        let labels: Vec<String> = snooze_choices(now).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels, ["Later today", "This evening", "Tomorrow", "This weekend", "Next week"]);
        // Saturday evening: no weekend, no later today.
        let saturday = tz.with_ymd_and_hms(2025, 11, 22, 21, 0, 0).unwrap();
        let labels: Vec<String> = snooze_choices(saturday).into_iter().map(|(l, _)| l).collect();
        assert_eq!(labels, ["Tomorrow", "Next week"]);
    }

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
