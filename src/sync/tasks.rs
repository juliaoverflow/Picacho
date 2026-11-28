//! Task lists kept by services with task APIs of their own, beside CalDAV's
//! VTODOs and JMAP for Task Lists: Microsoft To Do through Microsoft Graph
//! (`graph/todo.rs`) and Google Tasks (`google/tasks.rs`). What both read
//! into: lists, and tasks as `ical::Event`s with `task` set, as every other
//! source's tasks are, so Manage keeps and shows them all alike.
//!
//! A task's UID is its id at the service (both are unique and stable), so
//! a Google subtask's parent, which Google names by id, is its parent's UID.
//! Neither service keeps times on due dates: a task is due on a day.
use crate::sync::ical::Event;

/// A list of tasks at a service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteList {
    pub id: String,
    pub name: String,
    pub writable: bool,
    /// The list new tasks go to unless another is chosen.
    pub default: bool,
}

/// A task as read: its list, id and version, and the task.
#[derive(Clone, Debug, Default)]
pub struct RemoteTask {
    pub list: String,
    pub id: String,
    pub etag: String,
    pub event: Event,
}

/// A due date as the services write it (`2026-11-30`, or a date and time
/// whose day is the due date) as a task's all-day due time: midnight UTC
/// of that day, as all-day events start.
pub fn due_of(text: &str) -> Option<i64> {
    let day = text.get(..10)?;
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

/// The day a task is due (`YYYY-MM-DD`): an all-day one's own date, a
/// timed one's in this computer's time (the services keep no time).
pub fn due_day(e: &Event) -> Option<String> {
    if e.start == 0 {
        return None;
    }
    let at = chrono::DateTime::from_timestamp(e.start, 0)?;
    Some(if e.all_day {
        at.format("%Y-%m-%d").to_string()
    } else {
        at.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()
    })
}

/// Notes and the link a task was made with (`Event::url`) as one text, for
/// services that keep no link of their own: the link on a last line.
pub fn notes_with_link(e: &Event) -> String {
    let notes = e.description.trim_end();
    if e.url.is_empty() || notes.lines().any(|l| l.trim() == e.url) {
        return notes.to_owned();
    }
    if notes.is_empty() { e.url.clone() } else { format!("{notes}\n\n{}", e.url) }
}

/// Notes as `notes_with_link` writes them, apart again: (notes, link). A
/// last line that is a `mid:` link (an email's, RFC 2392) is the link.
pub fn link_from_notes(text: &str) -> (String, String) {
    let trimmed = text.trim_end();
    let (head, last) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
    let last = last.trim();
    if last.len() > 4 && last[..4].eq_ignore_ascii_case("mid:") && !last.contains(char::is_whitespace) {
        (head.trim_end().to_owned(), last.to_owned())
    } else {
        (text.to_owned(), String::new())
    }
}

/// RFC 5545's priority (1 highest … 9 lowest, 0 none) as a three-step
/// importance (Microsoft To Do's): high, normal or low.
pub fn importance(priority: u8) -> &'static str {
    match priority {
        1..=4 => "high",
        6..=9 => "low",
        _ => "normal",
    }
}
/// The other way: high 1, low 9, normal none.
pub fn priority_of(importance: &str) -> u8 {
    match importance {
        "high" => 1,
        "low" => 9,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_dates_are_days() {
        let at = due_of("2026-11-30").unwrap();
        assert_eq!(at % 86_400, 0);
        assert_eq!(due_of("2026-11-30T00:00:00.000Z"), Some(at));
        assert_eq!(due_of("2026-11-30T00:00:00.0000000"), Some(at));
        assert_eq!(due_of("soon"), None);
        let e = Event { start: at, end: at, all_day: true, ..Default::default() };
        assert_eq!(due_day(&e).as_deref(), Some("2026-11-30"));
        assert_eq!(due_day(&Event::default()), None);
    }

    #[test]
    fn links_ride_in_notes_and_come_back() {
        let e = Event { description: "Call back".into(), url: "mid:abc@example.org".into(), ..Default::default() };
        let text = notes_with_link(&e);
        assert_eq!(text, "Call back\n\nmid:abc@example.org");
        assert_eq!(link_from_notes(&text), ("Call back".into(), "mid:abc@example.org".into()));
        // Written again: not twice.
        let again = Event { description: text.clone(), ..e.clone() };
        assert_eq!(notes_with_link(&again), text);
        // Only a link alone.
        let bare = Event { url: "mid:x@y".into(), ..Default::default() };
        assert_eq!(link_from_notes(&notes_with_link(&bare)), (String::new(), "mid:x@y".into()));
        // Other text stays as it is.
        assert_eq!(link_from_notes("see mid: later"), ("see mid: later".into(), String::new()));
    }

    #[test]
    fn priorities_and_importance() {
        assert_eq!((importance(1), importance(5), importance(9), importance(0)), ("high", "normal", "low", "normal"));
        assert_eq!((priority_of("high"), priority_of("normal"), priority_of("low")), (1, 0, 9));
    }
}
