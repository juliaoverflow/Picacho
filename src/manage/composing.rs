//! Application-wide composing preferences, saved as one Store preference.
use crate::i18n::dates::Local as _;
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
    /// Signatures of particular mailboxes and sending addresses, in place
    /// of `signature`: by account id, or by an address (lowercase) the
    /// account sends as. Written in Markdown, so formatted messages show
    /// them formatted (bold, links, pictures by address), or in HTML
    /// (`is_html`), which formatted messages send as written.
    pub account_signatures: std::collections::BTreeMap<String, String>,
    /// Reusable messages (canned responses), inserted from the composer.
    pub templates: Vec<Template>,
    /// Add the signature to replies and forwards, not only new messages.
    pub signature_on_replies: bool,
    /// Seconds a sent message waits, with an Undo, before it leaves. 0 sends
    /// at once.
    pub undo_send_seconds: u32,
    /// Ask before sending a message that mentions an attachment without one.
    pub attachment_reminder: bool,
    /// Ask before sending a message without a subject.
    pub warn_empty_subject: bool,
    /// Pasting copied rich text keeps its formatting; otherwise it pastes as
    /// plain text. Either way the composer offers the other just after.
    pub paste_formatted: bool,
    /// The formatted composer underlines words spelled wrong.
    pub spellcheck: bool,
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
            account_signatures: Default::default(),
            templates: Vec::new(),
            signature_on_replies: true,
            undo_send_seconds: 5,
            attachment_reminder: true,
            warn_empty_subject: true,
            paste_formatted: false,
            spellcheck: true,
        }
    }
}

pub const PREFERENCE: &str = "compose_options";

/// A signature as it goes in a draft: the text the body shows, and for an
/// HTML signature its HTML, which formatted messages send in its place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub text: String,
    pub html: String,
}
impl Signature {
    /// A signature as written in Settings: HTML when it is (`is_html`),
    /// otherwise Markdown (or plain text), as the body shows it.
    pub fn from_setting(written: &str) -> Self {
        let written = written.trim();
        if is_html(written) {
            Self {
                text: signature_text(written),
                html: written.to_owned(),
            }
        } else {
            Self {
                text: written.to_owned(),
                html: String::new(),
            }
        }
    }
    /// What the body gets for it: the `-- ` line (RFC 3676 §4.3) and the
    /// text, after a blank line; nothing for no signature.
    pub fn block(&self) -> String {
        if self.text.is_empty() {
            String::new()
        } else {
            format!("\n\n{DELIMITER}{}", self.text)
        }
    }
}

/// The line that starts a signature (RFC 3676 §4.3).
pub const DELIMITER: &str = "-- \n";

/// Whether a signature is written in HTML rather than Markdown: it starts
/// and ends with a tag, and has an element's end, a line break or a
/// picture (so an address in angle brackets isn't taken for HTML).
pub fn is_html(signature: &str) -> bool {
    let s = signature.trim();
    let lower = s.to_ascii_lowercase();
    s.starts_with('<')
        && s.ends_with('>')
        && (lower.contains("</") || lower.contains("<br") || lower.contains("<img") || lower.contains("<hr"))
}

/// An HTML signature's text, for the body and the plain-text version.
pub fn signature_text(html: &str) -> String {
    let text = crate::sync::mime::html_to_text(html);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    // Blank lines at either end, and runs of them, come from block
    // elements; one is kept between paragraphs.
    let mut out: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim().is_empty() && out.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// `body` with signature `old` (a `Signature::block`) swapped for `new`,
/// after the sender changed: where `old` is in the body unchanged, it is
/// replaced; where there was none, `new` goes above the quoted or
/// forwarded message, or at the end. `None` when the signature there was
/// changed by hand, which stays.
pub fn swap_signature(body: &str, old: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }
    if !old.is_empty() {
        // The whole signature, to its line's end.
        let at = body
            .match_indices(old)
            .map(|(i, _)| i)
            .find(|&i| body[i + old.len()..].is_empty() || body[i + old.len()..].starts_with('\n'))?;
        let mut out = body.to_owned();
        out.replace_range(at..at + old.len(), new);
        return Some(out);
    }
    if new.is_empty() {
        return None;
    }
    // A signature written by hand, or another's, is left alone.
    if body.contains(&format!("\n{DELIMITER}")) || body.starts_with(DELIMITER) {
        return None;
    }
    let at = quote_start(body).unwrap_or(body.trim_end().len());
    let mut out = body.to_owned();
    out.insert_str(at, new);
    Some(out)
}

/// Where a reply's quote or a forwarded message starts in `body`.
fn quote_start(body: &str) -> Option<usize> {
    // Each line break that ends a blank line (overlapping: "\n\n\n").
    let after_blank = || {
        body.as_bytes()
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair == b"\n\n")
            .map(|(i, _)| i)
    };
    let forwarded =
        after_blank().find(|&i| body[i + 2..].lines().next().is_some_and(starts_forward));
    let quoted =
        after_blank().find(|&i| body[i + 2..].lines().next().is_some_and(introduces_quote));
    match (forwarded, quoted) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The "On …, … wrote:" line Picacho writes before a reply's quote, in each
/// language it has: its text before the date and name, and after them.
fn attributions() -> &'static [(String, String)] {
    static ALL: std::sync::LazyLock<Vec<(String, String)>> = std::sync::LazyLock::new(|| {
        let mut all = crate::i18n::in_every_language(|| {
            let line = t!("outgoing-wrote", date = "\u{1}", name = "\u{2}");
            let first = line.find(['\u{1}', '\u{2}']).unwrap_or(0);
            let last = line.rfind(['\u{1}', '\u{2}']).map_or(line.len(), |i| i + 1);
            (
                line[..first].trim_start().to_owned(),
                line[last..].trim_end().to_owned(),
            )
        });
        // A line naming only the date and name would match any line.
        all.retain(|(before, after)| !before.is_empty() || !after.is_empty());
        all.dedup();
        all
    });
    &ALL
}

/// Whether `line` introduces a reply's quote, as Picacho writes it (in any
/// of its languages): "On Mon, Nov 23, Ana wrote:".
pub fn introduces_quote(line: &str) -> bool {
    let line = line.trim();
    attributions().iter().any(|(before, after)| {
        line.len() > before.len() + after.len()
            && line.starts_with(before.as_str())
            && line.ends_with(after.as_str())
    })
}

/// Whether `line` starts a forwarded message's header, as Picacho writes it
/// (in any of its languages): "---------- Forwarded message ----------".
pub fn starts_forward(line: &str) -> bool {
    static ALL: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        let mut all =
            crate::i18n::in_every_language(|| t!("outgoing-forwarded-message").trim().to_owned());
        all.dedup();
        all
    });
    let line = line.trim();
    ALL.iter().any(|marker| line == marker)
}

/// What a template's placeholders stand for, as it is inserted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placeholders {
    /// The first recipient's name (as written in To, or in Contacts) and
    /// address.
    pub name: String,
    pub email: String,
    /// The sender's.
    pub my_name: String,
    pub my_email: String,
    /// Today, the time now and the weekday, as the computer writes them.
    pub date: String,
    pub time: String,
    pub weekday: String,
    /// The message's subject.
    pub subject: String,
}

/// The placeholders templates can use, with what each stands for, for
/// Settings to list.
pub const PLACEHOLDERS: [(&str, &str); 11] = [
    ("{name}", "the recipient's name"),
    ("{first_name}", "their first name"),
    ("{last_name}", "their last name"),
    ("{email}", "their address"),
    ("{my_name}", "your name"),
    ("{my_first_name}", "your first name"),
    ("{my_email}", "your address"),
    ("{date}", "today's date"),
    ("{time}", "the time"),
    ("{weekday}", "today's weekday"),
    ("{subject}", "the subject"),
];

/// `text` with its placeholders (`PLACEHOLDERS`, any case, spaces inside
/// the braces allowed) filled in. Other text in braces stays as it is.
pub fn fill_placeholders(text: &str, p: &Placeholders) -> String {
    let first = |name: &str| name.split_whitespace().next().unwrap_or("").to_owned();
    let last = |name: &str| {
        let words: Vec<&str> = name.split_whitespace().collect();
        if words.len() > 1 { words.last().copied().unwrap_or("").to_owned() } else { String::new() }
    };
    let value = |key: &str| -> Option<String> {
        Some(match key {
            "name" => p.name.clone(),
            "first_name" | "firstname" => first(&p.name),
            "last_name" | "lastname" => last(&p.name),
            "email" => p.email.clone(),
            "my_name" | "myname" => p.my_name.clone(),
            "my_first_name" => first(&p.my_name),
            "my_email" => p.my_email.clone(),
            "date" => p.date.clone(),
            "time" => p.time.clone(),
            "weekday" | "day" => p.weekday.clone(),
            "subject" => p.subject.clone(),
            _ => return None,
        })
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let filled = after.find('}').and_then(|close| {
            let key = after[..close].trim().to_lowercase().replace([' ', '-'], "_");
            value(&key).map(|v| (v, close))
        });
        match filled {
            Some((v, close)) => {
                out.push_str(&v);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A reusable message: its name in menus, and the subject and text it
/// puts in a draft. Its subject and text may use `PLACEHOLDERS`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub subject: String,
    pub body: String,
}

impl ComposeOptions {
    /// The signature for a message sent from `account` as `address`: the
    /// address's own, the account's, or everyone's.
    pub fn signature_for(&self, account: &str, address: &str) -> &str {
        let address = address.trim().to_lowercase();
        [address.as_str(), account]
            .iter()
            .filter(|k| !k.is_empty())
            .find_map(|k| self.account_signatures.get(*k))
            .map(String::as_str)
            .unwrap_or(&self.signature)
            .trim()
    }
    /// `signature_for`, as it goes in a draft.
    pub fn signature(&self, account: &str, address: &str) -> Signature {
        Signature::from_setting(self.signature_for(account, address))
    }
}

#[cfg(test)]
#[test]
fn html_signatures_are_told_from_markdown() {
    assert!(is_html("<p><b>Sam</b><br>Acme</p>"));
    assert!(is_html("<table><tr><td>Sam</td></tr></table>"));
    assert!(!is_html("<sam@example.com>"));
    assert!(!is_html("**Sam**"));
    let s = Signature::from_setting("<div><b>Sam Lee</b><br>Acme Inc.</div>");
    assert_eq!(s.text, "Sam Lee\nAcme Inc.");
    assert!(s.html.starts_with("<div>"));
    assert_eq!(s.block(), "\n\n-- \nSam Lee\nAcme Inc.");
    let md = Signature::from_setting("**Sam**");
    assert!(md.html.is_empty());
    assert_eq!(Signature::default().block(), "");
}

#[cfg(test)]
#[test]
fn signatures_swap_when_the_sender_changes() {
    let work = "\n\n-- \nWork";
    let home = "\n\n-- \nHome";
    let body = format!("Hi{work}\n\nOn Monday, Ana wrote:\n> hello");
    assert_eq!(
        swap_signature(&body, work, home).unwrap(),
        "Hi\n\n-- \nHome\n\nOn Monday, Ana wrote:\n> hello"
    );
    // To none, and back above the quote.
    let none = swap_signature(&body, work, "").unwrap();
    assert_eq!(none, "Hi\n\nOn Monday, Ana wrote:\n> hello");
    assert_eq!(swap_signature(&none, "", home).unwrap(), "Hi\n\n-- \nHome\n\nOn Monday, Ana wrote:\n> hello");
    assert_eq!(swap_signature("Hi ", "", home).unwrap(), "Hi\n\n-- \nHome ");
    // Changed by hand: left alone.
    assert_eq!(swap_signature("Hi\n\n-- \nWork, at home", work, home), None);
    assert_eq!(swap_signature("Hi\n\n-- \nMine", "", home), None);
}

#[cfg(test)]
#[test]
fn templates_fill_their_placeholders() {
    let p = Placeholders {
        name: "Ana María López".into(),
        email: "ana@example.org".into(),
        my_name: "Sam Lee".into(),
        my_email: "sam@example.com".into(),
        date: "24 November 2026".into(),
        time: "10:30".into(),
        weekday: "Tuesday".into(),
        subject: "Your order".into(),
    };
    assert_eq!(
        fill_placeholders("Hi {first_name}, re {Subject} ({ last name }) on {weekday} {date}. {unknown} {x — {my_first_name}", &p),
        "Hi Ana, re Your order (López) on Tuesday 24 November 2026. {unknown} {x — Sam"
    );
    assert_eq!(fill_placeholders("{name} <{email}>, {my_name} {my_email} {time}", &p), "Ana María López <ana@example.org>, Sam Lee sam@example.com 10:30");
    assert_eq!(fill_placeholders("no braces {", &p), "no braces {");
    let unnamed = Placeholders::default();
    assert_eq!(fill_placeholders("Hi {first_name}", &unnamed), "Hi ");
}

#[cfg(test)]
#[test]
fn signatures_go_by_address_then_account() {
    let mut o = ComposeOptions {
        signature: "Everyone".into(),
        ..ComposeOptions::default()
    };
    o.account_signatures.insert("acct".into(), "Work".into());
    o.account_signatures.insert("me@alias.test".into(), "Alias".into());
    assert_eq!(o.signature_for("acct", "Me@Alias.test"), "Alias");
    assert_eq!(o.signature_for("acct", "other@x.test"), "Work");
    assert_eq!(o.signature_for("home", ""), "Everyone");
}

/// Words that suggest the writer meant to attach something.
pub fn mentions_attachment(body: &str) -> bool {
    // Quoted text is someone else's; only what was written now counts.
    let written: String = body
        .lines()
        .take_while(|l| !introduces_quote(l))
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
        out.push((t!("manage-composing-later-today").to_owned(), t));
    }
    out.extend(at(1, 8).map(|t| (t!("manage-composing-tomorrow-morning").to_owned(), t)));
    out.extend(at(1, 13).map(|t| (t!("manage-composing-tomorrow-afternoon").to_owned(), t)));
    // Next Monday (a week on, if today is Monday); tomorrow already covers
    // a Sunday.
    let to_monday = (7 - now.weekday().num_days_from_monday()) as u64;
    if to_monday > 1 {
        out.extend(at(to_monday, 8).map(|t| (t!("manage-composing-monday-morning").to_owned(), t)));
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
        out.extend(
            at(0, now.hour() + 3).map(|t| (t!("manage-composing-later-today").to_owned(), t)),
        );
    }
    if now.hour() < 17 {
        out.extend(at(0, 19).map(|t| (t!("manage-composing-this-evening").to_owned(), t)));
    }
    out.extend(at(1, 8).map(|t| (t!("manage-composing-tomorrow").to_owned(), t)));
    let weekday = now.weekday().num_days_from_monday() as u64;
    if weekday < 4 {
        out.extend(at(5 - weekday, 8).map(|t| (t!("manage-composing-this-weekend").to_owned(), t)));
    }
    out.extend(at(7 - weekday, 8).map(|t| (t!("manage-composing-next-week").to_owned(), t)));
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
        return t!("manage-composing-later").to_owned();
    };
    let time = at.local(t!("dates-later-time"));
    let day = at.date_naive();
    let today = now.date_naive();
    if day == today {
        t!("manage-composing-today-at", time = time)
    } else if Some(day) == today.checked_add_days(Days::new(1)) {
        t!("manage-composing-tomorrow-at", time = time)
    } else {
        t!(
            "manage-composing-at",
            day = at.local(t!("dates-weekday-month-day")),
            time = time
        )
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
        // Lists saved by builds that had quick replies are still read.
        let o: ComposeOptions =
            serde_json::from_str(r#"{"quick_replies":["Thanks!"],"signature":"x"}"#).unwrap();
        assert_eq!(o.signature, "x");
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
