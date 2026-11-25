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
    /// Passed on to someone else (RFC 5545 §3.2.12).
    Delegated,
    /// For tasks: done, or under way.
    Completed,
    InProcess,
}

impl PartStat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "ACCEPTED",
            Self::Tentative => "TENTATIVE",
            Self::Declined => "DECLINED",
            Self::NeedsAction => "NEEDS-ACTION",
            Self::Delegated => "DELEGATED",
            Self::Completed => "COMPLETED",
            Self::InProcess => "IN-PROCESS",
        }
    }
    pub fn parse(text: &str) -> Self {
        match text.to_ascii_uppercase().as_str() {
            "ACCEPTED" => Self::Accepted,
            "TENTATIVE" => Self::Tentative,
            "DECLINED" => Self::Declined,
            "DELEGATED" => Self::Delegated,
            "COMPLETED" => Self::Completed,
            "IN-PROCESS" => Self::InProcess,
            _ => Self::NeedsAction,
        }
    }
    /// The word a reply's subject starts with.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Accepted => t!("sync-ical-accepted"),
            Self::Tentative => t!("sync-ical-tentatively-accepted"),
            Self::Declined => t!("sync-ical-declined"),
            Self::NeedsAction => t!("sync-ical-invitation"),
            Self::Delegated => t!("sync-ical-delegated"),
            Self::Completed => t!("sync-ical-completed"),
            Self::InProcess => t!("sync-ical-in-progress"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attendee {
    pub email: String,
    pub name: String,
    pub status: Option<PartStat>,
    /// The attendee's other parameters as written (ROLE, RSVP, CUTYPE,
    /// DELEGATED-TO/-FROM, SENT-BY, X-…), `;`-joined, kept so that
    /// rewriting an event doesn't lose them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra: String,
    /// SCHEDULE-STATUS (RFC 6638 §7.3): how the server's delivery to
    /// them went, as its codes (`1.2`, `3.7`…). The server's to write; read
    /// only, and left out of what is written back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedule_status: Vec<String>,
    /// SCHEDULE-AGENT (RFC 6638 §7.1) as written: SERVER, CLIENT or NONE,
    /// uppercase; empty where not given (SERVER).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_agent: String,
    /// SCHEDULE-FORCE-SEND (RFC 6638 §7.2): REQUEST or REPLY, for the
    /// server to send even where it thinks nothing changed. Sent once, so
    /// not read back.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub force_send: String,
}

impl Attendee {
    /// Whether the server says it couldn't deliver to them (RFC 6638
    /// §7.3, RFC 5545 §3.8.8.3: a status of class 3 or 5).
    pub fn delivery_failed(&self) -> bool {
        self.schedule_status.iter().any(|s| s.starts_with('3') || s.starts_with('5'))
    }
    /// The parameters RFC 6638 defines, as written in front of `extra`.
    fn scheduling_params(&self) -> String {
        let mut out = String::new();
        if !self.schedule_agent.is_empty() {
            out.push_str(&format!(";SCHEDULE-AGENT={}", self.schedule_agent));
        }
        if !self.force_send.is_empty() {
            out.push_str(&format!(";SCHEDULE-FORCE-SEND={}", self.force_send));
        }
        out
    }
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
    /// Reminders (VALARM), as seconds before the start (negative: after).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<i64>,
    /// A task (VTODO) rather than an event: `start` is when it is due
    /// (`end` the same), and `completed` whether it is done.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub task: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub completed: bool,
    /// The component's properties and sub-components Picacho doesn't edit
    /// (CATEGORIES, CLASS, TRANSP, GEO, URL, ATTACH, PRIORITY, RESOURCES,
    /// COMMENT, CONTACT, RELATED-TO, COLOR, CONFERENCE, IMAGE, X-…, and
    /// alarms with more than a trigger), as written: put back whenever
    /// the event is written from its fields, so editing it loses none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kept: String,
    /// A changed occurrence that changes the later ones too
    /// (RECURRENCE-ID;RANGE=THISANDFUTURE, RFC 5545 §3.8.4.4).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub this_and_future: bool,
    /// Its times are floating (no zone, no Z: the same wall time in every
    /// zone), and are written back so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub floating: bool,
    /// COMMENT: a note with an iTIP message (a proposed time's reason).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// STATUS as written (RFC 5545 §3.8.1.11: TENTATIVE, CONFIRMED,
    /// IN-PROCESS…), uppercase, so writing the event from its fields keeps
    /// it; `cancelled` and `completed` say the two Picacho acts on.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// GEO (RFC 5545 §3.8.1.6): `latitude;longitude` as written; read it
    /// with `coordinates`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub geo: String,
    /// CATEGORIES (RFC 5545 §3.8.1.2), every such line's in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// CLASS (RFC 5545 §3.8.1.3): PUBLIC, PRIVATE, CONFIDENTIAL or
    /// another, uppercase; empty where not given (PUBLIC).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class: String,
    /// COLOR (RFC 7986 §5.9): a CSS3 colour name such as `turquoise`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// IMAGE (RFC 7986 §5.10): pictures of the event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    /// CONFERENCE (RFC 7986 §5.11): ways to join it (a video call, a
    /// phone number).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conferences: Vec<Conference>,
}

/// An event's IMAGE (RFC 7986 §5.10).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Image {
    /// Where it is (VALUE=URI), or its base64 data (VALUE=BINARY).
    pub uri: String,
    /// FMTTYPE: its media type, where given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    /// DISPLAY: BADGE, GRAPHIC, FULLSIZE or THUMBNAIL; empty for BADGE.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display: String,
    /// Its other parameters (VALUE, ENCODING, ALTREP, X-…) as written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra: String,
}

/// A way to join an event (CONFERENCE, RFC 7986 §5.11).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conference {
    /// Its URI: `https://…`, `tel:…`, `sip:…`.
    pub uri: String,
    /// LABEL: what to call it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// FEATURE: AUDIO, VIDEO, CHAT, PHONE, SCREEN, MODERATOR, FEED…
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Its other parameters (LANGUAGE, X-…) as written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra: String,
}

/// A reminder in full (VALARM, RFC 5545 §3.6.6), as `Event::reminders`
/// reads it: the plain ones of `Event::alarms` and those kept whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Alarm {
    /// ACTION: DISPLAY, AUDIO, EMAIL, or another (Apple's NONE), uppercase.
    pub action: String,
    /// Seconds before the start it goes off (negative: after); for an
    /// absolute trigger, before the event's start too.
    pub before: i64,
    /// REPEAT and DURATION: how many more times, and how far apart in
    /// seconds (RFC 5545 §3.8.6.2: both or neither).
    pub repeat: u32,
    pub interval: i64,
    /// DESCRIPTION, or for an email its SUMMARY's text too.
    pub description: String,
    pub summary: String,
    /// An email reminder's recipients (ATTENDEE), as addresses.
    pub attendees: Vec<String>,
    /// ACKNOWLEDGED (RFC 9074 §6): it was dismissed.
    pub acknowledged: bool,
}

/// A file attached to an event (ATTACH, RFC 5545 §3.8.1.1), as
/// `Event::attachments` reads it from what's kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachment {
    /// Where it is (a URI), or empty for one inline (VALUE=BINARY).
    pub uri: String,
    /// FILENAME (RFC 8607 §4.2), else the URI's last part.
    pub name: String,
    /// FMTTYPE: its media type, where given.
    pub media_type: String,
    /// SIZE (RFC 8607 §4.3), in octets, where given.
    pub size: Option<u64>,
    /// MANAGED-ID (RFC 8607 §4.1): the server keeps it, and updates or
    /// removes it by this.
    pub managed_id: String,
}

impl Event {
    /// The files attached to the event (ATTACH lines kept as they came).
    pub fn attachments(&self) -> Vec<Attachment> {
        let mut depth = 0;
        let mut out = Vec::new();
        for line in unfold(&self.kept) {
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth -= 1;
            } else if depth == 0
                && let Some((name, params, value)) = split_line(&line)
                && name == "ATTACH"
            {
                let inline = param(&params, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("BINARY"));
                let uri = if inline { String::new() } else { value.trim().to_owned() };
                let file = param(&params, "FILENAME").map(str::to_owned).unwrap_or_else(|| {
                    uri.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("").to_owned()
                });
                out.push(Attachment {
                    uri,
                    name: file,
                    media_type: param(&params, "FMTTYPE").unwrap_or_default().to_owned(),
                    size: param(&params, "SIZE").and_then(|s| s.parse().ok()),
                    managed_id: param(&params, "MANAGED-ID").unwrap_or_default().to_owned(),
                });
            }
        }
        out
    }
    /// GEO's latitude and longitude, where it holds two numbers in range.
    pub fn coordinates(&self) -> Option<(f64, f64)> {
        let (lat, lon) = self.geo.split_once(';')?;
        let (lat, lon): (f64, f64) = (lat.trim().parse().ok()?, lon.trim().parse().ok()?);
        ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
    }
    /// What the invitation card shows besides time, place and people, as
    /// (label, text) rows: ways to join (CONFERENCE), the map position
    /// (GEO, where no place is named), CATEGORIES, CLASS when not public,
    /// and the reminders that do more than show a note.
    pub fn details(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for c in &self.conferences {
            let label = if c.label.is_empty() {
                c.features.join(", ").to_lowercase()
            } else {
                c.label.clone()
            };
            out.push((
                t!("sync-ical-join"),
                if label.is_empty() {
                    c.uri.clone()
                } else {
                    format!("{label}: {}", c.uri)
                },
            ));
        }
        if let Some((lat, lon)) = self.coordinates().filter(|_| self.location.is_empty()) {
            out.push((t!("sync-ical-where"), format!("{lat:.5}, {lon:.5}")));
        }
        if !self.categories.is_empty() {
            out.push((t!("sync-ical-categories"), self.categories.join(", ")));
        }
        match self.class.as_str() {
            "" | "PUBLIC" => {}
            "CONFIDENTIAL" => {
                out.push((t!("sync-ical-privacy"), t!("sync-ical-confidential").into()))
            }
            _ => out.push((t!("sync-ical-privacy"), t!("sync-ical-private").into())),
        }
        let others: Vec<String> = self
            .reminders()
            .iter()
            .filter(|a| a.action != "DISPLAY" && a.action != "NONE" && !a.acknowledged)
            .map(|a| {
                let how = match a.action.as_str() {
                    "EMAIL" => "email",
                    "AUDIO" => "sound",
                    _ => "other",
                };
                let minutes = a.before / 60;
                let mut text = if minutes >= 0 {
                    t!("sync-ical-min-before", how = how, minutes = minutes)
                } else {
                    t!("sync-ical-min-after", how = how, minutes = -minutes)
                };
                if a.repeat > 0 {
                    text = t!(
                        "sync-ical-more-every-min",
                        reminder = text,
                        repeat = a.repeat,
                        minutes = a.interval / 60
                    );
                }
                text
            })
            .collect();
        if !others.is_empty() {
            out.push((t!("sync-ical-reminders"), others.join("; ")));
        }
        out
    }
    /// Every reminder in full: the plain ones Picacho edits (`alarms`)
    /// and the others kept as they came (an email, a sound, repeats).
    pub fn reminders(&self) -> Vec<Alarm> {
        let mut out: Vec<Alarm> = self
            .alarms
            .iter()
            .map(|before| Alarm {
                action: "DISPLAY".into(),
                before: *before,
                description: self.summary.clone(),
                ..Default::default()
            })
            .collect();
        let length = (self.end - self.start).max(0);
        let mut current: Option<Alarm> = None;
        let mut depth = 0;
        for line in unfold(&self.kept) {
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("BEGIN:") {
                depth += 1;
                if upper == "BEGIN:VALARM" && depth == 1 {
                    current = Some(Alarm::default());
                }
                continue;
            }
            if upper.starts_with("END:") {
                depth -= 1;
                if upper == "END:VALARM" && depth == 0 {
                    out.extend(current.take());
                }
                continue;
            }
            let (Some(a), Some((name, params, value))) = (current.as_mut(), split_line(&line)) else {
                continue;
            };
            if depth != 1 {
                continue;
            }
            match name.as_str() {
                "ACTION" => a.action = value.trim().to_ascii_uppercase(),
                "TRIGGER" => {
                    a.before = if param(&params, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) {
                        parse_time(&value, &[]).map_or(0, |(t, _)| self.start - t)
                    } else {
                        let d = parse_duration(&value).unwrap_or(0);
                        if param(&params, "RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END")) {
                            -(d + length)
                        } else {
                            -d
                        }
                    }
                }
                "REPEAT" => a.repeat = value.trim().parse().unwrap_or(0),
                "DURATION" => a.interval = parse_duration(&value).unwrap_or(0),
                "DESCRIPTION" => a.description = unescape(&value),
                "SUMMARY" => a.summary = unescape(&value),
                "ATTENDEE" => a.attendees.push(attendee(&line, &value, &params).email),
                "ACKNOWLEDGED" => a.acknowledged = true,
                _ => {}
            }
        }
        out
    }
}

/// Properties `write_event` writes from an event's fields; others are
/// kept as they came (`Event::kept`).
/// CREATED isn't one: RFC 5545 §3.8.7.1 has it stay as it was first
/// written, so it is kept; LAST-MODIFIED is written anew (§3.8.7.3).
const MODELLED: [&str; 30] = [
    "COMMENT", "BEGIN", "END", "UID", "DTSTAMP", "SEQUENCE", "DTSTART", "DTEND", "DUE", "DURATION",
    "EXDATE", "RDATE", "RECURRENCE-ID", "STATUS", "SUMMARY", "LOCATION", "DESCRIPTION",
    "RRULE", "ORGANIZER", "ATTENDEE", "COMPLETED", "PERCENT-COMPLETE", "LAST-MODIFIED",
    "X-PICACHO", "GEO", "CATEGORIES", "CLASS", "COLOR", "IMAGE", "CONFERENCE",
];

/// The lines of a component (unfolded, BEGIN and END included) that
/// `write_event` doesn't write: unmodelled properties, and sub-components
/// other than plain reminders, which `alarms` holds.
fn kept_lines(lines: &[String]) -> String {
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < lines.len() {
        let line = &lines[i];
        let upper = line.to_ascii_uppercase();
        if let Some(kind) = upper.strip_prefix("BEGIN:") {
            // A whole sub-component: to its END.
            let end = format!("END:{kind}");
            let mut depth = 0;
            let mut j = i;
            while j < lines.len() - 1 {
                let u = lines[j].to_ascii_uppercase();
                if u.starts_with("BEGIN:") {
                    depth += 1;
                } else if u.starts_with("END:") {
                    depth -= 1;
                    if depth == 0 && u == end {
                        break;
                    }
                }
                j += 1;
            }
            let block = &lines[i..=j.min(lines.len() - 1)];
            // A display reminder with only a trigger is in `alarms`.
            let simple = kind == "VALARM"
                && block.iter().all(|l| {
                    let n = split_line(l).map(|(n, ..)| n).unwrap_or_default();
                    matches!(n.as_str(), "BEGIN" | "END" | "TRIGGER" | "DESCRIPTION" | "UID" | "X-WR-ALARMUID")
                        || (n == "ACTION" && l.to_ascii_uppercase().ends_with(":DISPLAY"))
                });
            if !simple {
                out.extend(block.iter().cloned());
            }
            i = j + 1;
            continue;
        }
        let name = split_line(line).map(|(n, ..)| n).unwrap_or_default();
        if !MODELLED.contains(&name.as_str()) {
            out.push(line.clone());
        }
        i += 1;
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\r\n") + "\r\n"
    }
}

/// When someone is busy (a VFREEBUSY reply, RFC 5545 §3.6.4): their
/// address and busy periods (start, end), in Unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FreeBusy {
    pub attendee: String,
    pub busy: Vec<(i64, i64)>,
}

/// The busy periods in VFREEBUSY components (FREEBUSY lines, other than
/// FBTYPE=FREE), each with its ATTENDEE.
pub fn parse_free_busy(text: &str) -> Vec<FreeBusy> {
    let mut out = Vec::new();
    let mut current: Option<FreeBusy> = None;
    for line in unfold(text) {
        let Some((name, params, value)) = split_line(&line) else {
            continue;
        };
        match name.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VFREEBUSY") => current = Some(FreeBusy::default()),
            "END" if value.eq_ignore_ascii_case("VFREEBUSY") => out.extend(current.take()),
            "ATTENDEE" => {
                if let Some(fb) = current.as_mut() {
                    fb.attendee = value.trim().trim_start_matches("mailto:").trim_start_matches("MAILTO:").to_owned();
                }
            }
            "FREEBUSY" => {
                let Some(fb) = current.as_mut() else { continue };
                if param(&params, "FBTYPE").is_some_and(|t| t.eq_ignore_ascii_case("FREE")) {
                    continue;
                }
                for period in value.split(',') {
                    let Some((from, to)) = period.split_once('/') else { continue };
                    let Some((start, _)) = parse_time(from, &[]) else { continue };
                    let end = parse_time(to, &[])
                        .map(|(t, _)| t)
                        .or_else(|| parse_duration(to).map(|d| start + d));
                    if let Some(end) = end {
                        fb.busy.push((start, end));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A VFREEBUSY request (RFC 4791 §7.10, iTIP's REQUEST) for `attendees`
/// from `organizer` over `from`..`to`, for a scheduling outbox.
pub fn free_busy_request(organizer: &str, attendees: &[String], from: i64, to: i64) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_owned(),
        "VERSION:2.0".to_owned(),
        concat!("PRODID:-//Pinal Technologies//Picacho ", env!("CARGO_PKG_VERSION"), "//EN").to_owned(),
        "METHOD:REQUEST".to_owned(),
        "BEGIN:VFREEBUSY".to_owned(),
        format!("UID:{}", uuid::Uuid::new_v4()),
        format!("DTSTAMP:{}", stamp(Utc::now().timestamp())),
        format!("DTSTART:{}", stamp(from)),
        format!("DTEND:{}", stamp(to)),
        format!("ORGANIZER:mailto:{organizer}"),
    ];
    lines.extend(attendees.iter().map(|a| format!("ATTENDEE:mailto:{a}")));
    lines.push("END:VFREEBUSY".to_owned());
    lines.push("END:VCALENDAR".to_owned());
    lines.iter().map(|l| fold(l)).collect()
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

/// A content line: its name, parameters and value.
type Line = (String, Vec<(String, String)>, String);

/// Splits a content line into its name, parameters and value. Colons
/// inside quoted parameter values don't end the name.
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
    // Semicolons inside quoted values (`CN="Doe; Jane"`) don't end one.
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in head.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&head[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&head[start..]);
    let mut parts = parts.into_iter();
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), caret_decode(v.trim_matches('"'))))
        })
        .collect();
    Some((name, params, value.to_owned()))
}
/// A parameter value with RFC 6868's escapes undone: `^n` a line break,
/// `^'` a double quote, `^^` a caret.
fn caret_decode(value: &str) -> String {
    if !value.contains('^') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('\'') => {
                    chars.next();
                    out.push('"');
                }
                Some('^') => {
                    chars.next();
                    out.push('^');
                }
                _ => out.push('^'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
/// `source` (a component as it came) with the PARTSTAT of the attendee
/// `email` set to `status`, everything else as it was; RSVP is dropped
/// once answered. `None` where it doesn't name that attendee.
pub fn set_partstat_in_source(source: &str, email: &str, status: PartStat) -> Option<String> {
    let mut found = false;
    let mut out = String::with_capacity(source.len() + 32);
    // Only the first component's own attendees, not those in its alarms.
    let mut depth = 0usize;
    for line in unfold(source) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
        } else if upper.starts_with("END:") {
            depth = depth.saturating_sub(1);
        }
        let mine = depth == 1
            && upper.starts_with("ATTENDEE")
            && split_line(&line).is_some_and(|(_, _, v)| {
                v.trim()
                    .strip_prefix("mailto:")
                    .or_else(|| v.trim().strip_prefix("MAILTO:"))
                    .unwrap_or(v.trim())
                    .eq_ignore_ascii_case(email)
            });
        if !mine {
            out.push_str(&fold(&line));
            continue;
        }
        found = true;
        let colon = quote_aware_colon(&line).unwrap_or(line.len());
        let (head, value) = (&line[..colon], &line[colon..]);
        let mut kept: Vec<String> = Vec::new();
        let mut quoted = false;
        let mut start = 0;
        let mut pieces = Vec::new();
        for (i, c) in head.char_indices() {
            match c {
                '"' => quoted = !quoted,
                ';' if !quoted => {
                    pieces.push(&head[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        pieces.push(&head[start..]);
        for (i, piece) in pieces.iter().enumerate() {
            let key = piece.split('=').next().unwrap_or("").to_ascii_uppercase();
            if i > 0 && (key == "PARTSTAT" || key == "RSVP") {
                continue;
            }
            kept.push((*piece).to_owned());
        }
        kept.push(format!("PARTSTAT={}", status.as_str()));
        out.push_str(&fold(&format!("{}{value}", kept.join(";"))));
    }
    found.then_some(out)
}
fn quote_aware_colon(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
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

/// One observance of a calendar's own VTIMEZONE (STANDARD or DAYLIGHT):
/// from its start (local wall time), the offset in force, repeating
/// yearly where its RRULE says (`BYMONTH`, `BYDAY` such as `-1SU` or
/// `2SU`, or `BYMONTHDAY`), else once (and at its RDATEs).
#[derive(Clone, Debug, Default)]
struct Observance {
    start: NaiveDateTime,
    offset_to: i32,
    offset_from: i32,
    month: Option<u32>,
    /// (nth, weekday): -1 the last.
    day: Option<(i32, chrono::Weekday)>,
    month_day: Option<i32>,
    until: Option<NaiveDateTime>,
    rdates: Vec<NaiveDateTime>,
}
impl Observance {
    /// When it begins in `year`, as local wall time (by the offset before).
    fn onset_in(&self, year: i32) -> Option<NaiveDateTime> {
        use chrono::Datelike;
        if year < self.start.year() {
            return None;
        }
        let time = self.start.time();
        let at = match (self.month, self.day, self.month_day) {
            (Some(month), Some((nth, weekday)), _) => {
                let first = NaiveDate::from_ymd_opt(year, month, 1)?;
                let days = first
                    .with_month(month % 12 + 1)
                    .and_then(|d| if month == 12 { d.with_year(year + 1) } else { Some(d) })
                    .map_or(31, |next| (next - first).num_days() as u32);
                let matching: Vec<NaiveDate> = (0..days)
                    .filter_map(|d| first.checked_add_days(chrono::Days::new(d.into())))
                    .filter(|d| d.weekday() == weekday)
                    .collect();
                let pick = if nth < 0 {
                    matching.len().checked_sub(nth.unsigned_abs() as usize)?
                } else {
                    (nth as usize).checked_sub(1)?
                };
                matching.get(pick)?.and_time(time)
            }
            (Some(month), None, Some(day)) => NaiveDate::from_ymd_opt(year, month, day as u32)?.and_time(time),
            _ if year == self.start.year() => self.start,
            _ => return None,
        };
        (at >= self.start && self.until.is_none_or(|u| at <= u)).then_some(at)
    }
    /// Its onsets around `year`, as UTC instants.
    fn onsets(&self, year: i32) -> Vec<(i64, i32)> {
        let mut out = Vec::new();
        for y in [year - 1, year] {
            if let Some(at) = self.onset_in(y) {
                out.push((at.and_utc().timestamp() - i64::from(self.offset_from), self.offset_to));
            }
        }
        for r in &self.rdates {
            out.push((r.and_utc().timestamp() - i64::from(self.offset_from), self.offset_to));
        }
        out
    }
}

/// `+0130`/`-05` style UTC offset in seconds.
fn parse_offset(text: &str) -> Option<i32> {
    let text = text.trim();
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let n = |r: std::ops::Range<usize>| digits.get(r).map_or(Some(0), |d| d.parse::<i32>().ok());
    Some(sign * (n(0..2)? * 3600 + n(2..4)? * 60 + n(4..6)?))
}

/// A calendar's own time zones (VTIMEZONE, RFC 5545 §3.6.5), by TZID.
#[derive(Clone, Debug, Default)]
pub(crate) struct Zones(std::collections::HashMap<String, Vec<Observance>>);
impl Zones {
    fn read(lines: &[String]) -> Self {
        let mut zones: std::collections::HashMap<String, Vec<Observance>> = Default::default();
        let mut tzid = None;
        let mut current: Option<Observance> = None;
        for line in lines {
            let Some((name, _, value)) = split_line(line) else { continue };
            let upper = value.trim().to_ascii_uppercase();
            match name.as_str() {
                "BEGIN" if upper == "VTIMEZONE" => tzid = Some(String::new()),
                "END" if upper == "VTIMEZONE" => tzid = None,
                "TZID" if current.is_none() => {
                    if let Some(t) = tzid.as_mut() {
                        *t = value.trim().trim_start_matches('/').to_owned();
                    }
                }
                "BEGIN" if tzid.is_some() && (upper == "STANDARD" || upper == "DAYLIGHT") => {
                    current = Some(Observance::default())
                }
                "END" if upper == "STANDARD" || upper == "DAYLIGHT" => {
                    if let (Some(o), Some(t)) = (current.take(), tzid.as_ref()) {
                        zones.entry(t.clone()).or_default().push(o);
                    }
                }
                _ => {
                    let Some(o) = current.as_mut() else { continue };
                    match name.as_str() {
                        "DTSTART" => {
                            if let Ok(t) = NaiveDateTime::parse_from_str(value.trim(), "%Y%m%dT%H%M%S") {
                                o.start = t;
                            }
                        }
                        "TZOFFSETTO" => o.offset_to = parse_offset(&value).unwrap_or(0),
                        "TZOFFSETFROM" => o.offset_from = parse_offset(&value).unwrap_or(0),
                        "RDATE" => o.rdates.extend(
                            value
                                .split(',')
                                .filter_map(|v| NaiveDateTime::parse_from_str(v.trim(), "%Y%m%dT%H%M%S").ok()),
                        ),
                        "RRULE" => {
                            for part in value.split(';') {
                                let Some((k, v)) = part.split_once('=') else { continue };
                                match k.to_ascii_uppercase().as_str() {
                                    "BYMONTH" => o.month = v.parse().ok(),
                                    "BYMONTHDAY" => o.month_day = v.parse().ok(),
                                    "UNTIL" => {
                                        o.until = NaiveDateTime::parse_from_str(
                                            v.trim_end_matches('Z'),
                                            "%Y%m%dT%H%M%S",
                                        )
                                        .ok()
                                    }
                                    "BYDAY" => {
                                        let v = v.split(',').next().unwrap_or(v);
                                        if v.len() >= 2 && v.is_ascii() {
                                            let (n, d) = v.split_at(v.len() - 2);
                                            let n = if n.is_empty() { 1 } else { n.parse().unwrap_or(1) };
                                            o.day = weekday(&d.to_ascii_uppercase()).map(|w| (n, w));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Self(zones)
    }
    /// The UTC instant of local time `t` in the zone `tzid`, if this
    /// calendar defines it.
    fn to_utc(&self, tzid: &str, t: NaiveDateTime) -> Option<i64> {
        use chrono::Datelike;
        let observances = self.0.get(tzid)?;
        let year = t.year();
        let mut onsets: Vec<(i64, i32)> = observances.iter().flat_map(|o| o.onsets(year)).collect();
        onsets.sort();
        let wall = t.and_utc().timestamp();
        // The offset in force: the last onset before, tried with each
        // candidate offset (the result must agree with it).
        let mut offset = observances.first().map_or(0, |o| o.offset_from);
        for (at, to) in &onsets {
            if wall - i64::from(*to) >= *at {
                offset = *to;
            }
        }
        Some(wall - i64::from(offset))
    }
}
thread_local! {
    /// The VTIMEZONEs of the calendar being read, for `parse_time`.
    static ZONES: std::cell::RefCell<Zones> = Default::default();
}
/// Runs `f` with the time zones `lines` define in force for `parse_time`.
fn with_zones<T>(lines: &[String], f: impl FnOnce() -> T) -> T {
    let zones = Zones::read(lines);
    let before = ZONES.with(|z| std::mem::replace(&mut *z.borrow_mut(), zones));
    let out = f();
    ZONES.with(|z| *z.borrow_mut() = before);
    out
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
        // A zone the calendar defines itself (VTIMEZONE): by its rules,
        // not the reader's own zone.
        if let Some(at) = ZONES.with(|z| z.borrow().to_utc(tzid, t)) {
            return Some((at, false));
        }
    }
    // Floating time: the reader's own zone.
    let local = chrono::Local.from_local_datetime(&t).earliest()?;
    Some((local.timestamp(), false))
}

/// A TEXT list value (CATEGORIES, RESOURCES: RFC 5545 §3.1.1), split at
/// unescaped commas, each unescaped.
fn text_list(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                current.push(c);
                current.extend(chars.next());
            }
            ',' => out.push(unescape(std::mem::take(&mut current).trim())),
            c => current.push(c),
        }
    }
    out.push(unescape(current.trim()));
    out
}
/// A content line's parameters split into those named in `known` (by
/// name, values decoded) and the rest as written, `;`-joined.
fn split_params(line: &str, known: &[&str]) -> (Vec<(String, String)>, String) {
    let mut found = Vec::new();
    let mut rest = Vec::new();
    for piece in raw_params(line) {
        let (k, v) = piece.split_once('=').unwrap_or((piece, ""));
        let k = k.trim().to_ascii_uppercase();
        if known.contains(&k.as_str()) {
            // A list value (FEATURE=AUDIO,VIDEO) stays a list; quotes go.
            let v = v.split(',').map(|p| caret_decode(p.trim_matches('"'))).collect::<Vec<_>>().join(",");
            found.push((k, v));
        } else {
            rest.push(piece);
        }
    }
    (found, rest.join(";"))
}
/// The `;`-separated parameter pieces of a content line's head, as
/// written (quotes and escapes kept), without its name.
fn raw_params(line: &str) -> Vec<&str> {
    let head = &line[..quote_aware_colon(line).unwrap_or(line.len())];
    let mut out = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (i, c) in head.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&head[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&head[start..]);
    out.remove(0);
    out
}

fn attendee(line: &str, value: &str, params: &[(String, String)]) -> Attendee {
    let email = value
        .trim()
        .trim_start_matches("mailto:")
        .trim_start_matches("MAILTO:")
        .to_owned();
    // The parameters Picacho doesn't edit, exactly as written: a list
    // value (RFC 5545 §3.2.11 DELEGATED-TO="mailto:a","mailto:b") stays a
    // list. RFC 6638's parameters are kept apart: SCHEDULE-STATUS is the
    // server's to set (§7.3), SCHEDULE-FORCE-SEND a one-time request (§7.2).
    let extra = raw_params(line)
        .into_iter()
        .filter(|p| {
            let k = p.split('=').next().unwrap_or("").trim().to_ascii_uppercase();
            !matches!(k.as_str(), "CN" | "PARTSTAT" | "SCHEDULE-STATUS" | "SCHEDULE-AGENT" | "SCHEDULE-FORCE-SEND")
        })
        .collect::<Vec<_>>()
        .join(";");
    Attendee {
        name: param(params, "CN").unwrap_or_default().to_owned(),
        status: param(params, "PARTSTAT").map(PartStat::parse),
        email,
        extra,
        // RFC 6638 §7.3: a list of statuses, quoted where several.
        schedule_status: param(params, "SCHEDULE-STATUS")
            .map(|s| s.split(',').map(|c| c.trim().trim_matches('"').to_owned()).filter(|c| !c.is_empty()).collect())
            .unwrap_or_default(),
        schedule_agent: param(params, "SCHEDULE-AGENT").unwrap_or_default().to_ascii_uppercase(),
        force_send: String::new(),
    }
}
/// A parameter value as written: RFC 6868's escapes put in, quoted where
/// it holds `;`, `:` or `,`.
pub(crate) fn param_value(value: &str) -> String {
    let escaped = value
        .replace('^', "^^")
        .replace('\n', "^n")
        .replace('"', "^'");
    if escaped.contains([';', ':', ',']) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}
/// A parameter's value by name in an attendee's other parameters.
pub fn extra_param(extra: &str, name: &str) -> Option<String> {
    split_line(&format!("X;{extra}:x"))
        .and_then(|(_, params, _)| param(&params, &name.to_ascii_uppercase()).map(str::to_owned))
}

/// The tasks (VTODO) in an iCalendar object, those without a due date
/// too (`start` 0). A task's DUE is when it's due even where it also has
/// a DTSTART (when work on it starts).
pub fn parse_tasks(text: &str) -> Vec<Event> {
    let lines = unfold(text);
    with_zones(&lines, || parse_task_lines(&lines))
}
fn parse_task_lines(lines: &[String]) -> Vec<Event> {
    // Each VTODO alone, so `parse`'s rules for events don't apply: it
    // leaves out what has no start.
    let mut out = Vec::new();
    let mut inside: Option<Vec<String>> = None;
    for line in lines {
        let upper = line.to_ascii_uppercase();
        if upper == "BEGIN:VTODO" {
            inside = Some(vec![line.clone()]);
            continue;
        }
        if let Some(todo) = inside.as_mut() {
            todo.push(line.clone());
            if upper == "END:VTODO" {
                let todo = inside.take().unwrap_or_default();
                let source = todo.join("\r\n") + "\r\n";
                let mut due = None;
                let mut nested = 0;
                for l in &todo[1..todo.len() - 1] {
                    let u = l.to_ascii_uppercase();
                    if u.starts_with("BEGIN:") {
                        nested += 1;
                    } else if u.starts_with("END:") {
                        nested -= 1;
                    } else if nested == 0
                        && let Some((name, params, value)) = split_line(l)
                        && name == "DUE"
                    {
                        due = parse_time(&value, &params);
                    }
                }
                // Parsed with a stand-in start, so `parse` keeps it.
                let mut wrapped = vec!["BEGIN:VCALENDAR".to_owned()];
                wrapped.extend(
                    todo.iter()
                        .filter(|l| {
                            let u = l.to_ascii_uppercase();
                            !u.starts_with("DTSTART") && !u.starts_with("DUE")
                        })
                        .cloned(),
                );
                wrapped.insert(2, "DTSTART:19700101T000001Z".to_owned());
                wrapped.push("END:VCALENDAR".to_owned());
                let Some(mut e) = parse(&wrapped.join("\r\n")).events.pop() else {
                    continue;
                };
                let (start, all_day) = due.unwrap_or((0, false));
                e.start = start;
                e.end = start;
                e.all_day = all_day;
                let due_params = todo
                    .iter()
                    .filter_map(|l| split_line(l))
                    .find(|(n, ..)| n == "DUE");
                e.tzid = due_params
                    .as_ref()
                    .and_then(|(_, p, _)| zone_name(p))
                    .unwrap_or_default();
                e.utc = due_params.is_some_and(|(.., v)| v.trim().ends_with('Z'));
                e.task = true;
                e.source = source;
                if !e.uid.is_empty() {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// A task's VTODO with what's edited here changed (summary, notes, due,
/// done) and everything else kept as the server had it: priority,
/// categories, alarms, other apps' properties. Without a source, it is
/// written from its fields.
pub fn update_task(e: &Event) -> String {
    if e.source.is_empty() {
        return write_event(e);
    }
    let edited = [
        "SUMMARY",
        "DESCRIPTION",
        "DUE",
        "STATUS",
        "COMPLETED",
        "PERCENT-COMPLETE",
        "SEQUENCE",
        "DTSTAMP",
        "LAST-MODIFIED",
    ];
    let lines = unfold(&e.source);
    let mut out = Vec::new();
    let mut nested = 0;
    for line in &lines {
        let upper = line.to_ascii_uppercase();
        if upper == "END:VTODO" && nested == 0 {
            break;
        }
        if upper.starts_with("BEGIN:") && upper != "BEGIN:VTODO" {
            nested += 1;
        } else if upper.starts_with("END:") {
            nested -= 1;
        }
        let name = split_line(line).map(|(n, ..)| n).unwrap_or_default();
        if nested == 0 && upper != "BEGIN:VTODO" && edited.contains(&name.as_str()) {
            continue;
        }
        out.push(line.clone());
        if upper == "BEGIN:VTODO" {
            let now = stamp(Utc::now().timestamp());
            out.push(format!("DTSTAMP:{now}"));
            out.push(format!("LAST-MODIFIED:{now}"));
            out.push(format!("SEQUENCE:{}", e.sequence));
            out.push(format!("SUMMARY:{}", escape(&e.summary)));
            if !e.description.is_empty() {
                out.push(format!("DESCRIPTION:{}", escape(&e.description)));
            }
            if e.start != 0 {
                out.push(if e.all_day {
                    format!("DUE;{}", when(e.start, true))
                } else if let Ok(zone) = e.tzid.parse::<chrono_tz::Tz>() {
                    let local = DateTime::from_timestamp(e.start, 0)
                        .unwrap_or_default()
                        .with_timezone(&zone);
                    format!("DUE;TZID={}:{}", e.tzid, local.format("%Y%m%dT%H%M%S"))
                } else {
                    format!("DUE{}", when(e.start, false))
                });
            }
            if e.completed {
                out.push("STATUS:COMPLETED".to_owned());
                out.push(format!("COMPLETED:{now}"));
                out.push("PERCENT-COMPLETE:100".to_owned());
            } else {
                out.push("STATUS:NEEDS-ACTION".to_owned());
            }
        }
    }
    out.push("END:VTODO".to_owned());
    out.iter().map(|l| fold(l)).collect()
}

/// A task as a whole calendar object for its server.
pub fn task_to_store(e: &Event) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    out.push_str(concat!(
        "PRODID:-//Pinal Technologies//Picacho ",
        env!("CARGO_PKG_VERSION"),
        "//EN\r\n"
    ));
    out.push_str(&update_task(e));
    out.push_str("END:VCALENDAR\r\n");
    with_timezones(&out)
}

/// Reads an iCalendar object. Events without a start are left out.
pub fn parse(text: &str) -> Calendar {
    let lines = unfold(text);
    with_zones(&lines, || parse_lines(lines.clone()))
}
fn parse_lines(lines: Vec<String>) -> Calendar {
    let mut cal = Calendar {
        text: lines.join("\r\n") + "\r\n",
        ..Default::default()
    };
    let mut depth: Vec<String> = Vec::new();
    let mut event: Option<Event> = None;
    let mut source: Vec<String> = Vec::new();
    let mut end_duration: Option<i64> = None;
    // Inside a VALARM: its trigger once read, as it was written; it is
    // made relative to the start only once the event's times are known
    // (a VALARM may come before DTSTART and DTEND).
    let mut alarm: Option<Option<Trigger>> = None;
    let mut triggers: Vec<Trigger> = Vec::new();
    // Where the current alarm's lines start in `source`.
    let mut alarm_at = 0;
    for line in &lines {
        let Some((name, params, value)) = split_line(line) else {
            continue;
        };
        match name.as_str() {
            "BEGIN" => {
                let kind = value.to_ascii_uppercase();
                if (kind == "VEVENT" || kind == "VTODO")
                    && depth.last().is_some_and(|d| d == "VCALENDAR")
                {
                    event = Some(Event {
                        task: kind == "VTODO",
                        ..Default::default()
                    });
                    source.clear();
                    end_duration = None;
                    triggers.clear();
                }
                // A reminder: when, relative to the start.
                if kind == "VALARM" {
                    alarm = Some(None);
                    alarm_at = source.len();
                }
                depth.push(kind);
            }
            "END" => {
                let kind = value.to_ascii_uppercase();
                // Only plain reminders are edited as `alarms`; others (an
                // email or sound alarm, repeats) are kept whole.
                let block = source.get(alarm_at..).unwrap_or_default();
                let simple = kept_lines(
                    &[vec![String::new()], block.to_vec(), vec![line.clone(), String::new()]].concat(),
                )
                .is_empty();
                if kind == "VALARM"
                    && let Some(Some(t)) = alarm.take()
                    && event.is_some()
                    && simple
                {
                    triggers.push(t);
                }
                if (kind == "VEVENT" || kind == "VTODO")
                    && depth.len() == 2
                    && let Some(mut e) = event.take()
                {
                    source.push(line.clone());
                    e.source = source.join("\r\n") + "\r\n";
                    e.kept = kept_lines(&source);
                    // A task without a due date is due when it starts.
                    if e.task && e.start == 0 {
                        e.start = e.end;
                    }
                    if e.end == 0 {
                        e.end = match end_duration {
                            Some(d) => e.start + d,
                            None if e.all_day => e.start + 86_400,
                            None => e.start,
                        };
                    }
                    let length = (e.end - e.start).max(0);
                    for t in triggers.drain(..) {
                        let before = match t {
                            Trigger::At(at) => e.start - at,
                            Trigger::FromStart(d) => -d,
                            Trigger::FromEnd(d) => -(d + length),
                        };
                        if !e.alarms.contains(&before) {
                            e.alarms.push(before);
                        }
                    }
                    // Without a start it isn't an event to show; an iTIP
                    // message may still name one so (CANCEL, REFRESH:
                    // RFC 5546 lets DTSTART out), and those are kept.
                    if e.start != 0
                        || (matches!(cal.method.as_str(), "CANCEL" | "REFRESH" | "DECLINECOUNTER")
                            && !e.uid.is_empty())
                    {
                        cal.events.push(e);
                    }
                }
                depth.pop();
            }
            "METHOD" if depth.len() == 1 => cal.method = value.trim().to_ascii_uppercase(),
            _ => {}
        }
        // Everything inside the event goes in its source, alarms included,
        // so it is written back whole (the END:VEVENT line is added above).
        if event.is_some() && depth.len() >= 2 && (depth[1] == "VEVENT" || depth[1] == "VTODO") {
            source.push(line.clone());
        }
        if name == "TRIGGER"
            && depth.last().is_some_and(|d| d == "VALARM")
            && let Some(slot) = alarm.as_mut()
        {
            let from_end = param(&params, "RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
            // An absolute trigger (VALUE=DATE-TIME) is kept as relative.
            *slot = if param(&params, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) {
                parse_time(&value, &[]).map(|(t, _)| Trigger::At(t))
            } else {
                parse_duration(&value).map(|d| {
                    if from_end { Trigger::FromEnd(d) } else { Trigger::FromStart(d) }
                })
            };
        }
        // Properties of the event itself, not of an alarm inside it.
        if depth.last().is_some_and(|d| d == "VEVENT" || d == "VTODO") && depth.len() == 2 {
            let Some(e) = event.as_mut() else { continue };
            match name.as_str() {
                "UID" => e.uid = value.trim().to_owned(),
                "SUMMARY" => e.summary = unescape(&value),
                "LOCATION" => e.location = unescape(&value),
                "DESCRIPTION" => e.description = unescape(&value),
                "COMMENT" => e.comment = unescape(&value),
                "DTSTART" => {
                    if let Some((t, all_day)) = parse_time(&value, &params) {
                        e.start = t;
                        e.all_day = all_day;
                        e.tzid = zone_name(&params).unwrap_or_default();
                        e.utc = value.trim().ends_with('Z');
                        e.floating = !all_day && !e.utc && param(&params, "TZID").is_none();
                    }
                }
                "EXDATE" => {
                    for v in value.split(',') {
                        if let Some((t, _)) = parse_time(v, &params) {
                            e.exdates.push(t);
                        }
                    }
                }
                "RDATE" => {
                    // A PERIOD (start/end or start/duration): its start;
                    // its length is the series'.
                    for v in value.split(',') {
                        let v = v.split('/').next().unwrap_or(v);
                        let plain: Vec<(String, String)> = params
                            .iter()
                            .filter(|(k, _)| k != "VALUE")
                            .cloned()
                            .collect();
                        if let Some((t, _)) = parse_time(v, &plain) {
                            e.rdates.push(t);
                        }
                    }
                }
                "RECURRENCE-ID" => {
                    e.recurrence_id = parse_time(&value, &params).map(|(t, _)| t);
                    e.this_and_future = param(&params, "RANGE")
                        .is_some_and(|r| r.eq_ignore_ascii_case("THISANDFUTURE"));
                }
                "DTEND" | "DUE" => {
                    if let Some((t, _)) = parse_time(&value, &params) {
                        e.end = t;
                    }
                }
                "DURATION" => end_duration = parse_duration(&value),
                "ORGANIZER" => e.organizer = Some(attendee(line, &value, &params)),
                "ATTENDEE" => e.attendees.push(attendee(line, &value, &params)),
                "SEQUENCE" => e.sequence = value.trim().parse().unwrap_or(0),
                "RRULE" => e.recurrence = value.trim().to_owned(),
                "STATUS" => {
                    e.status = value.trim().to_ascii_uppercase();
                    e.cancelled |= e.status == "CANCELLED";
                    e.completed |= e.status == "COMPLETED";
                }
                "COMPLETED" => e.completed = true,
                "GEO" => e.geo = value.trim().to_owned(),
                "CATEGORIES" => e.categories.extend(
                    text_list(&value).into_iter().filter(|c| !c.is_empty()),
                ),
                "CLASS" => e.class = value.trim().to_ascii_uppercase(),
                "COLOR" => e.color = value.trim().to_owned(),
                "IMAGE" => {
                    let (known, extra) = split_params(line, &["FMTTYPE", "DISPLAY"]);
                    let get = |k: &str| known.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
                    e.images.push(Image { uri: value.trim().to_owned(), media_type: get("FMTTYPE"), display: get("DISPLAY"), extra });
                }
                "CONFERENCE" => {
                    let (known, extra) = split_params(line, &["LABEL", "FEATURE"]);
                    let get = |k: &str| known.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
                    e.conferences.push(Conference {
                        uri: value.trim().to_owned(),
                        label: get("LABEL"),
                        features: get("FEATURE").split(',').map(|f| f.trim().to_ascii_uppercase()).filter(|f| !f.is_empty()).collect(),
                        extra,
                    });
                }
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
            // Invitations are to events; a task sent by mail isn't one.
            cal.events.retain(|e| !e.task);
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

/// Folds a content line at 75 octets (RFC 5545 §3.1). A line break or NUL
/// inside it (a UID, organizer or attendee as an invitation gave it, which
/// isn't escaped as text is) is left out: it would end the line and start
/// a property of the sender's choosing.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in line.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')) {
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
    pub by_set_pos: Vec<i32>,
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
                        // Byte offsets below; a day is two ASCII letters.
                        if !d.is_ascii() {
                            return None;
                        }
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
                "BYSETPOS" => {
                    rule.by_set_pos = v.split(',').filter_map(|d| d.trim().parse().ok()).collect()
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
        let mut text = t!("sync-ical-every", unit = unit, interval = self.interval);
        // Each detail is a phrase of its own ("on Monday", "until Nov 26"),
        // joined to the rest as the language joins them.
        let detail = |text: String, detail: String| t!("sync-ical-repeat-detail", repeat = text, detail = detail);
        let end = |text: String, end: String| t!("sync-ical-repeat-end", repeat = text, end = end);
        if !self.by_month.is_empty() {
            let months: Vec<String> = self
                .by_month
                .iter()
                .filter(|&&m| (1..=12).contains(&m))
                .map(|&m| t!("sync-ical-month-name", month = m))
                .collect();
            if !months.is_empty() {
                text = detail(text, t!("sync-ical-in-months", months = months.join(", ")));
            }
        }
        let weekdays_only = self.by_day.len() == 5
            && self.by_day.iter().all(|(n, d)| {
                *n == 0
                    && matches!(
                        d,
                        chrono::Weekday::Mon
                            | chrono::Weekday::Tue
                            | chrono::Weekday::Wed
                            | chrono::Weekday::Thu
                            | chrono::Weekday::Fri
                    )
            });
        let weekend_only = self.by_day.len() == 2
            && self
                .by_day
                .iter()
                .all(|(n, d)| *n == 0 && matches!(d, chrono::Weekday::Sat | chrono::Weekday::Sun));
        if let [pos] = self.by_set_pos.as_slice()
            && (weekdays_only || weekend_only)
        {
            let kind = if weekdays_only { "weekday" } else { "weekend" };
            text = detail(text, t!("sync-ical-on-the-nth-kind-of-day", pos = *pos, kind = kind));
        } else if !self.by_day.is_empty() {
            let days: Vec<String> = self
                .by_day
                .iter()
                .map(|(n, d)| {
                    let day = t!("sync-ical-weekday-name", day = d.num_days_from_monday());
                    t!("sync-ical-nth-weekday", pos = *n, day = day)
                })
                .collect();
            text = detail(text, t!("sync-ical-on-days", days = days.join(", ")));
        } else if !self.by_month_day.is_empty() {
            let days: Vec<String> = self
                .by_month_day
                .iter()
                .map(|&d| {
                    let kind = match d {
                        -1 => "last",
                        ..-1 => "before-end",
                        _ => "day",
                    };
                    t!("sync-ical-month-day", kind = kind, day = d, before_end = -d)
                })
                .collect();
            text = detail(text, t!("sync-ical-on-days", days = days.join(", ")));
        }
        if let Some(n) = self.count {
            text = end(text, t!("sync-ical-times", count = n));
        } else if let Some(u) = self.until {
            let d = DateTime::from_timestamp(u, 0).unwrap_or_default();
            let date = crate::i18n::dates::format(&d, t!("dates-repeat-until"));
            text = end(text, t!("sync-ical-until", date = date));
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
    // The series without its source text, to copy for each repeat: each had
    // a copy of it (thousands, for a large invitation), and an occurrence is
    // shown, never saved from (saving reads the series).
    let bare = Event {
        source: String::new(),
        ..series.clone()
    };
    let mut out = Vec::new();
    for start in starts {
        if series.exdates.contains(&start) {
            continue;
        }
        // The latest THISANDFUTURE change at or before this one applies
        // to it: its fields, its time moved by as much as that one's.
        let future = overrides
            .iter()
            .filter(|o| o.this_and_future && o.recurrence_id.is_some_and(|r| r <= start))
            .max_by_key(|o| o.recurrence_id);
        let occurrence = match overrides.iter().find(|o| o.recurrence_id == Some(start)) {
            Some(changed) if changed.cancelled => continue,
            Some(changed) => changed.clone(),
            None => match future {
                Some(f) if f.cancelled => continue,
                Some(f) => {
                    let shift = f.start - f.recurrence_id.unwrap_or(f.start);
                    Event {
                        start: start + shift,
                        end: start + shift + (f.end - f.start),
                        recurrence_id: Some(start),
                        this_and_future: false,
                        source: String::new(),
                        ..f.clone()
                    }
                }
                None => Event {
                    start,
                    end: start + length,
                    recurrence_id: Some(start),
                    ..bare.clone()
                },
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
    let after = from.max(series.start) - 1;
    // Walked here with a budget of steps: the expander's own limit counts
    // the times it returns, not those before `from` it passes over, so an
    // invitation repeating every second since 1970 had it count through
    // billions of them first.
    const STEPS: usize = 500_000;
    let mut starts: Vec<i64> = Vec::new();
    // `limit` keeps the expander's own guard for rules that never match
    // (February 31st), which looked without end.
    for d in set.limit().into_iter().take(STEPS) {
        let t = d.timestamp();
        if t > to {
            break;
        }
        if t >= after {
            starts.push(t);
            if starts.len() >= 5000 {
                break;
            }
        }
    }
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
    with_timezones(&out)
}

/// When an event is, for an invitation's text: in the event's own zone
/// when it has one, else in UTC, with the zone named.
pub fn describe_when(e: &Event) -> String {
    // Written into invitations sent by mail: in their language (Settings).
    crate::i18n::in_outgoing(|| describe_when_here(e))
}
fn describe_when_here(e: &Event) -> String {
    use crate::i18n::dates;
    let zone: chrono_tz::Tz = e.tzid.parse().unwrap_or(chrono_tz::UTC);
    let at = |t: i64| {
        DateTime::from_timestamp(t, 0)
            .unwrap_or_default()
            .with_timezone(&zone)
    };
    let mut text = if e.all_day {
        let start = DateTime::from_timestamp(e.start, 0).unwrap_or_default();
        t!(
            "sync-ical-all-day",
            date = dates::format(&start, t!("dates-long-date"))
        )
    } else {
        format!(
            "{} – {} ({})",
            dates::format(&at(e.start), t!("dates-event-start")),
            dates::format(&at(e.end), t!("dates-event-end")),
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
    with_timezones(&out)
}

/// The iMIP REPLY to an invitation: `me` answers `status` (RFC 5546 §3.2.3).
pub fn reply(event: &Event, me: &str, my_name: &str, status: PartStat) -> Result<String> {
    ensure!(
        !event.uid.is_empty(),
        t!("sync-ical-this-invitation-has-no-identifier-to")
    );
    let organizer = event
        .organizer
        .as_ref()
        .filter(|o| !o.email.is_empty())
        .ok_or_else(|| anyhow::anyhow!(t!("sync-ical-this-invitation-doesnt-say-who-to")))?;
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
        // An answer for one occurrence names it (RFC 5546 §3.2.3).
        match event.recurrence_id {
            Some(id) => format!(
                "RECURRENCE-ID;{}",
                when(id, event.all_day).trim_start_matches(';')
            ),
            None => String::new(),
        },
        format!("SUMMARY:{}", escape(&event.summary)),
        format!(
            "ORGANIZER{}:mailto:{}",
            if organizer.name.is_empty() {
                String::new()
            } else {
                format!(";CN=\"{}\"", organizer.name.replace('^', "^^").replace('"', "^'").replace('\n', "^n"))
            },
            organizer.email
        ),
        format!(
            "ATTENDEE;PARTSTAT={}{}:mailto:{me}",
            status.as_str(),
            if my_name.is_empty() {
                String::new()
            } else {
                format!(";CN=\"{}\"", my_name.replace('^', "^^").replace('"', "^'").replace('\n', "^n"))
            }
        ),
        "END:VEVENT".to_owned(),
        "END:VCALENDAR".to_owned(),
    ] {
        if line.is_empty() {
            continue;
        }
        // DTSTART with a date value is written `DTSTART;VALUE=DATE:…`.
        let line = line
            .replace("DTSTART;:", "DTSTART:")
            .replace("RECURRENCE-ID;:", "RECURRENCE-ID:");
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
        t!("sync-ical-this-invitation-has-no-identifier-to")
    );
    ensure!(
        end >= start,
        t!("sync-ical-the-time-proposed-ends-before-it")
    );
    let organizer = event
        .organizer
        .as_ref()
        .filter(|o| !o.email.is_empty())
        .ok_or_else(|| anyhow::anyhow!(t!("sync-ical-this-invitation-doesnt-say-who-to")))?;
    let proposed = Event {
        start,
        end,
        sequence: event.sequence,
        organizer: Some(organizer.clone()),
        attendees: vec![Attendee {
            email: me.to_owned(),
            name: my_name.to_owned(),
            status: Some(PartStat::Tentative),
            ..Default::default()
        }],
        // The guest's note is a COMMENT (RFC 5546 §3.2.7); the event's
        // description stays the organizer's.
        comment: note.to_owned(),
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
        // DTSTART, so the guest's app can match it (and reads the event).
        format!(
            "DTSTART;{}",
            when(event.start, event.all_day).trim_start_matches(';')
        )
        .replace("DTSTART;:", "DTSTART:"),
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
    with_timezones(&out)
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
    with_timezones(&out)
}

/// A calendar object for a CalDAV server that Picacho does the inviting
/// for: every ORGANIZER and ATTENDEE gets `SCHEDULE-AGENT=CLIENT` (RFC 6638
/// §7.1), so a server that schedules (iCloud, Fastmail, Nextcloud, Google)
/// doesn't send its own invitations, answers and cancellations on top of
/// the ones Picacho emails. One that already names an agent is left alone.
pub fn scheduled_by_client(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    for line in unfold(text) {
        let name_end = line.find([';', ':']).unwrap_or(line.len());
        let name = line[..name_end].to_ascii_uppercase();
        let params = split_line(&line).map(|(_, p, _)| p).unwrap_or_default();
        let named = params
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("SCHEDULE-AGENT"));
        if (name == "ORGANIZER" || name == "ATTENDEE") && !named {
            let line = format!(
                "{};SCHEDULE-AGENT=CLIENT{}",
                &line[..name_end],
                &line[name_end..]
            );
            out.push_str(&fold(&line));
        } else {
            out.push_str(&fold(&line));
        }
    }
    out
}

/// `text` with every ORGANIZER's and ATTENDEE's SCHEDULE-STATUS left out:
/// RFC 6638 §3.2.3 has the server set it, and refuse a client's write
/// that includes one it didn't (or ignore it), so one read with the event
/// isn't sent back.
pub fn without_schedule_status(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in unfold(text) {
        let upper = line.to_ascii_uppercase();
        if (upper.starts_with("ORGANIZER;") || upper.starts_with("ATTENDEE;")) && upper.contains("SCHEDULE-STATUS=") {
            let colon = quote_aware_colon(&line).unwrap_or(line.len());
            let name = &line[..line.find(';').unwrap_or(colon)];
            let kept: Vec<&str> = raw_params(&line)
                .into_iter()
                .filter(|p| !p.trim().to_ascii_uppercase().starts_with("SCHEDULE-STATUS="))
                .collect();
            let head = std::iter::once(name).chain(kept).collect::<Vec<_>>().join(";");
            out.push_str(&fold(&format!("{head}{}", &line[colon..])));
        } else {
            out.push_str(&fold(&line));
        }
    }
    out
}

/// The UTC offset (seconds east) of `zone` at `t`.
fn offset_at(zone: chrono_tz::Tz, t: i64) -> i32 {
    use chrono::Offset;
    let utc = DateTime::from_timestamp(t, 0).unwrap_or_default();
    utc.with_timezone(&zone).offset().fix().local_minus_utc()
}
/// `+0130`-style offset for iCalendar.
fn utc_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let s = seconds.unsigned_abs();
    let (h, m, sec) = (s / 3600, s / 60 % 60, s % 60);
    if sec == 0 {
        format!("{sign}{h:02}{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}{sec:02}")
    }
}
/// The zone's changes of offset in `from..to` (Unix seconds), each as
/// (instant, offset before, offset after), found by searching each day
/// for a change and then the second.
fn transitions(zone: chrono_tz::Tz, from: i64, to: i64) -> Vec<(i64, i32, i32)> {
    const DAY: i64 = 86_400;
    let mut out = Vec::new();
    let mut t = from;
    let mut before = offset_at(zone, t);
    while t < to {
        let next = (t + DAY).min(to);
        let after = offset_at(zone, next);
        if after != before {
            // The second it changes, within this day.
            let (mut lo, mut hi) = (t, next);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if offset_at(zone, mid) == before {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            out.push((hi, before, after));
            before = after;
        }
        t = next;
    }
    out
}
/// A VTIMEZONE for `tzid` (an IANA name) covering `years` (RFC 5545
/// §3.6.5): its offset changes in those years as dated STANDARD and
/// DAYLIGHT observances, or one STANDARD where it doesn't change. `None`
/// for a name the time zone database doesn't know.
pub fn vtimezone(tzid: &str, years: std::ops::RangeInclusive<i32>) -> Option<String> {
    let zone: chrono_tz::Tz = tzid.parse().ok()?;
    let start = |y: i32| {
        chrono::NaiveDate::from_ymd_opt(y, 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp())
    };
    let (from, to) = (start(*years.start())?, start(years.end() + 1)?);
    let mut lines = vec!["BEGIN:VTIMEZONE".to_owned(), format!("TZID:{tzid}")];
    let changes = transitions(zone, from, to);
    let local = |t: i64, offset: i32| {
        DateTime::from_timestamp(t + i64::from(offset), 0)
            .unwrap_or_default()
            .format("%Y%m%dT%H%M%S")
            .to_string()
    };
    let abbreviation = |t: i64| {
        DateTime::from_timestamp(t, 0)
            .unwrap_or_default()
            .with_timezone(&zone)
            .format("%Z")
            .to_string()
    };
    if changes.is_empty() {
        let offset = utc_offset(offset_at(zone, from));
        lines.extend([
            "BEGIN:STANDARD".to_owned(),
            // Any start before the events will do.
            "DTSTART:19700101T000000".to_owned(),
            format!("TZOFFSETFROM:{offset}"),
            format!("TZOFFSETTO:{offset}"),
            format!("TZNAME:{}", abbreviation(from)),
            "END:STANDARD".to_owned(),
        ]);
    } else {
        // What held before the first change, so earlier times are covered.
        // In summer time where the first change lowers the offset (the
        // southern hemisphere's January).
        let (first, before, after_first) = changes[0];
        let opening = if after_first < before { "DAYLIGHT" } else { "STANDARD" };
        lines.extend([
            format!("BEGIN:{opening}"),
            "DTSTART:19700101T000000".to_owned(),
            format!("TZOFFSETFROM:{}", utc_offset(before)),
            format!("TZOFFSETTO:{}", utc_offset(before)),
            format!("TZNAME:{}", abbreviation(first - 1)),
            format!("END:{opening}"),
        ]);
        // The last year's changes also as yearly rules (by month and
        // weekday) from the end of the range, so repeats past it keep
        // their local time where the rules haven't changed.
        let last_year: Vec<&(i64, i32, i32)> = changes
            .iter()
            .filter(|(at, ..)| *at >= to - 366 * 86_400)
            .collect();
        for (at, before, after) in &last_year {
            use chrono::Datelike;
            let wall = DateTime::from_timestamp(at + i64::from(*before), 0).unwrap_or_default();
            let nth = (wall.day() as i32 - 1) / 7 + 1;
            let days_in_month = wall
                .date_naive()
                .with_day(1)
                .and_then(|d| d.checked_add_months(chrono::Months::new(1)))
                .map_or(31, |n| n.pred_opt().map_or(31, |p| p.day()));
            let nth = if wall.day() + 7 > days_in_month { -1 } else { nth };
            let day = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"][wall.weekday().num_days_from_monday() as usize];
            let kind = if after > before { "DAYLIGHT" } else { "STANDARD" };
            lines.extend([
                format!("BEGIN:{kind}"),
                format!("DTSTART:{}", local(*at, *before)),
                format!("RRULE:FREQ=YEARLY;BYMONTH={};BYDAY={nth}{day}", wall.month()),
                format!("TZOFFSETFROM:{}", utc_offset(*before)),
                format!("TZOFFSETTO:{}", utc_offset(*after)),
                format!("TZNAME:{}", abbreviation(*at)),
                format!("END:{kind}"),
            ]);
        }
        for (at, before, after) in changes.iter().filter(|c| !last_year.contains(c)) {
            // The later offset of a pair is summer time (DAYLIGHT).
            let kind = if after > before { "DAYLIGHT" } else { "STANDARD" };
            lines.extend([
                format!("BEGIN:{kind}"),
                // DTSTART is in the local time before the change.
                format!("DTSTART:{}", local(*at, *before)),
                format!("TZOFFSETFROM:{}", utc_offset(*before)),
                format!("TZOFFSETTO:{}", utc_offset(*after)),
                format!("TZNAME:{}", abbreviation(*at)),
                format!("END:{kind}"),
            ]);
        }
    }
    lines.push("END:VTIMEZONE".to_owned());
    Some(lines.iter().map(|l| fold(l)).collect())
}
/// `text` (a whole VCALENDAR) with a VTIMEZONE for each TZID its
/// components use that it doesn't define (RFC 5545 §3.2.19 requires one),
/// placed before the first component. Covers the years of the times it
/// has, from a year before to a few after (repeating events go on), so
/// Outlook and Exchange, which go by the VTIMEZONE rather than the name,
/// show the right times.
pub fn with_timezones(text: &str) -> String {
    let lines = unfold(text);
    let mut defined = std::collections::HashSet::new();
    let mut used: Vec<String> = Vec::new();
    let mut years: Vec<i32> = Vec::new();
    for line in &lines {
        let upper = line.to_ascii_uppercase();
        if let Some(id) = upper.strip_prefix("TZID:") {
            defined.insert(id.trim().to_owned());
            continue;
        }
        let Some((_, params, value)) = split_line(line) else {
            continue;
        };
        if let Some((_, tzid)) = params.iter().find(|(k, _)| k.eq_ignore_ascii_case("TZID")) {
            let tzid = tzid.trim_matches('"').to_owned();
            if !used.contains(&tzid) {
                used.push(tzid);
            }
            // Only plausible years: a server's or sender's far-off date
            // mustn't make the time zone search cover millennia.
            if let Some(y) = value
                .get(..4)
                .and_then(|y| y.parse::<i32>().ok())
                .filter(|y| (1900..=2200).contains(y))
            {
                years.push(y);
            }
        }
    }
    let missing: Vec<&String> = used
        .iter()
        .filter(|t| !defined.contains(&t.to_ascii_uppercase()))
        .collect();
    if missing.is_empty() {
        return text.to_owned();
    }
    // Bounded work however the object is made: at most this many zones,
    // over at most this many years.
    const MOST_ZONES: usize = 16;
    const MOST_YEARS: i32 = 40;
    let first = years.iter().min().copied().unwrap_or(1970) - 1;
    let last = (years.iter().max().copied().unwrap_or(first) + 5)
        .max(first)
        .min(first + MOST_YEARS);
    let blocks: String = missing
        .iter()
        .take(MOST_ZONES)
        .filter_map(|t| vtimezone(t, first..=last))
        .collect();
    // Before the first component, after the calendar's own properties.
    let at = ["\r\nBEGIN:VEVENT", "\r\nBEGIN:VTODO", "\r\nBEGIN:VFREEBUSY", "\r\nEND:VCALENDAR"]
        .iter()
        .filter_map(|m| text.find(m))
        .min();
    match at {
        Some(i) => format!("{}\r\n{}{}", &text[..i], blocks.trim_end_matches("\r\n"), &text[i..]),
        None => text.to_owned(),
    }
}

/// A VEVENT written from its fields, for events made or edited here.
pub fn write_event(e: &Event) -> String {
    let kind = if e.task { "VTODO" } else { "VEVENT" };
    let mut lines = vec![
        format!("BEGIN:{kind}"),
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
        } else if e.floating {
            // In the reader's wall time, as it was read, without a zone.
            let local = DateTime::from_timestamp(t, 0)
                .unwrap_or_default()
                .with_timezone(&chrono::Local);
            format!("{name}:{}", local.format("%Y%m%dT%H%M%S"))
        } else {
            format!("{name}{}", when(t, false))
        }
    };
    if let Some(id) = e.recurrence_id.filter(|_| e.recurrence.is_empty()) {
        let line = at("RECURRENCE-ID", id);
        lines.push(if e.this_and_future {
            line.replacen("RECURRENCE-ID", "RECURRENCE-ID;RANGE=THISANDFUTURE", 1)
        } else {
            line
        });
    }
    if e.task {
        // A task is due then (0: no due date); done or not.
        if e.start != 0 {
            lines.push(at("DUE", e.start));
        }
        if e.completed {
            lines.push("STATUS:COMPLETED".to_owned());
            lines.push(format!("COMPLETED:{}", stamp(Utc::now().timestamp())));
        } else if !e.cancelled {
            // RFC 5545 §3.8.1.11: a task's other open state (IN-PROCESS) stays.
            let open = matches!(e.status.as_str(), "IN-PROCESS" | "NEEDS-ACTION");
            lines.push(format!("STATUS:{}", if open { e.status.as_str() } else { "NEEDS-ACTION" }));
        }
    } else {
        lines.push(at("DTSTART", e.start));
        lines.push(at(
            "DTEND",
            if e.all_day {
                e.end.max(e.start + 86_400)
            } else {
                e.end.max(e.start)
            },
        ));
    }
    for x in &e.exdates {
        lines.push(at("EXDATE", *x));
    }
    for x in &e.rdates {
        lines.push(at("RDATE", *x));
    }
    if e.cancelled {
        lines.push("STATUS:CANCELLED".to_owned());
    } else if !e.task && matches!(e.status.as_str(), "TENTATIVE" | "CONFIRMED") {
        // RFC 5545 §3.8.1.11: an event's other states, as they came.
        lines.push(format!("STATUS:{}", e.status));
    }
    lines.push(format!("LAST-MODIFIED:{}", stamp(Utc::now().timestamp())));
    lines.push(format!("SUMMARY:{}", escape(&e.summary)));
    if !e.location.is_empty() {
        lines.push(format!("LOCATION:{}", escape(&e.location)));
    }
    if !e.description.is_empty() {
        lines.push(format!("DESCRIPTION:{}", escape(&e.description)));
    }
    if !e.comment.is_empty() {
        lines.push(format!("COMMENT:{}", escape(&e.comment)));
    }
    if !e.recurrence.is_empty() {
        lines.push(format!("RRULE:{}", e.recurrence));
    }
    if !e.class.is_empty() {
        lines.push(format!("CLASS:{}", e.class));
    }
    if !e.categories.is_empty() {
        lines.push(format!("CATEGORIES:{}", e.categories.iter().map(|c| escape(c)).collect::<Vec<_>>().join(",")));
    }
    if !e.geo.is_empty() {
        lines.push(format!("GEO:{}", e.geo));
    }
    if !e.color.is_empty() {
        lines.push(format!("COLOR:{}", e.color));
    }
    let with = |name: &str, params: &[(&str, String)], extra: &str, value: &str| {
        let mut head = name.to_owned();
        if !extra.is_empty() {
            head.push(';');
            head.push_str(extra);
        }
        for (k, v) in params.iter().filter(|(_, v)| !v.is_empty()) {
            head.push_str(&format!(";{k}={v}"));
        }
        format!("{head}:{value}")
    };
    for i in &e.images {
        // RFC 7986 §5.10: VALUE=URI unless the data is inline.
        let extra = if i.extra.is_empty() { "VALUE=URI".to_owned() } else { i.extra.clone() };
        lines.push(with("IMAGE", &[("FMTTYPE", param_value(&i.media_type)), ("DISPLAY", param_value(&i.display))], &extra, &i.uri));
    }
    for c in &e.conferences {
        // RFC 7986 §5.11: VALUE=URI is required.
        let extra = if c.extra.to_ascii_uppercase().contains("VALUE=") { c.extra.clone() } else if c.extra.is_empty() { "VALUE=URI".to_owned() } else { format!("VALUE=URI;{}", c.extra) };
        let features = c.features.iter().map(|f| param_value(f)).collect::<Vec<_>>().join(",");
        lines.push(with("CONFERENCE", &[("FEATURE", features), ("LABEL", param_value(&c.label))], &extra, &c.uri));
    }
    let person = |p: &Attendee| -> String {
        if p.name.is_empty() {
            String::new()
        } else {
            format!(";CN=\"{}\"", p.name.replace('^', "^^").replace('"', "^'").replace('\n', "^n"))
        }
    };
    if let Some(o) = e.organizer.as_ref().filter(|o| !o.email.is_empty()) {
        let extra = if o.extra.is_empty() { String::new() } else { format!(";{}", o.extra) };
        lines.push(format!("ORGANIZER{extra}{}{}:mailto:{}", o.scheduling_params(), person(o), o.email));
    }
    for at in &e.attendees {
        // Its own parameters where it came with them; a new guest is a
        // required one asked to answer.
        let extra = if at.extra.is_empty() {
            "ROLE=REQ-PARTICIPANT;RSVP=TRUE".to_owned()
        } else {
            at.extra.clone()
        };
        lines.push(format!(
            "ATTENDEE;{extra};PARTSTAT={}{}{}:mailto:{}",
            at.status.unwrap_or(PartStat::NeedsAction).as_str(),
            at.scheduling_params(),
            person(at),
            at.email
        ));
    }
    for before in &e.alarms {
        lines.push("BEGIN:VALARM".to_owned());
        lines.push("ACTION:DISPLAY".to_owned());
        lines.push(format!("DESCRIPTION:{}", escape(&e.summary)));
        lines.push(format!("TRIGGER:{}", trigger(*before)));
        lines.push("END:VALARM".to_owned());
    }
    // What Picacho doesn't edit, as it came.
    lines.extend(e.kept.split("\r\n").filter(|l| !l.is_empty()).map(str::to_owned));
    lines.push(format!("END:{kind}"));
    lines.iter().map(|l| fold(l)).collect()
}

/// A VALARM's TRIGGER as written, before the event's times are known.
#[derive(Clone, Copy)]
enum Trigger {
    /// At that moment (Unix seconds).
    At(i64),
    /// That long after the start (negative: before).
    FromStart(i64),
    /// That long after the end.
    FromEnd(i64),
}

/// A reminder's TRIGGER: seconds before the start as `-PT15M`, `-P1D`
/// (and `-PT90S` where it isn't whole minutes).
fn trigger(before: i64) -> String {
    let (sign, s) = if before >= 0 { ("-", before) } else { ("", -before) };
    if s % 86_400 == 0 && s > 0 {
        format!("{sign}P{}D", s / 86_400)
    } else if s % 3600 == 0 && s > 0 {
        format!("{sign}PT{}H", s / 3600)
    } else if s % 60 == 0 {
        format!("{sign}PT{}M", s / 60)
    } else {
        format!("{sign}PT{s}S")
    }
}

#[cfg(test)]
mod tests {
    /// Calendar objects as Google, iCloud and Nextcloud write them: several
    /// reminders of different kinds, X- properties, ATTACH, a changed
    /// occurrence (RECURRENCE-ID) and the calendar's own VTIMEZONE.
    const GOOGLE: &str = include_str!("../../tests/fixtures/ical/google.ics");
    const ICLOUD: &str = include_str!("../../tests/fixtures/ical/icloud.ics");
    const NEXTCLOUD: &str = include_str!("../../tests/fixtures/ical/nextcloud.ics");

    /// The top-level property lines of each component (not its
    /// sub-components), with DTSTAMP and LAST-MODIFIED left out since
    /// writing renews them.
    fn own_lines(component: &str) -> Vec<String> {
        let lines = unfold(component);
        let mut depth = 0;
        let mut out = Vec::new();
        for l in &lines {
            let u = l.to_ascii_uppercase();
            if u.starts_with("BEGIN:") {
                depth += 1;
                continue;
            }
            if u.starts_with("END:") {
                depth -= 1;
                continue;
            }
            if depth == 1 && !u.starts_with("DTSTAMP") && !u.starts_with("LAST-MODIFIED") {
                out.push(l.clone());
            }
        }
        out
    }
    /// The VALARM blocks of a component, each as its lines in order.
    fn alarm_blocks(component: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut current: Option<Vec<String>> = None;
        for l in unfold(component) {
            let u = l.to_ascii_uppercase();
            if u == "BEGIN:VALARM" {
                current = Some(Vec::new());
            } else if u == "END:VALARM" {
                out.extend(current.take());
            } else if let Some(c) = current.as_mut() {
                c.push(l);
            }
        }
        out
    }

    /// Every fixture is edited (its title changed) and written from its
    /// fields: what Picacho doesn't model comes back exactly once, the
    /// reminders aren't doubled, and a modelled property isn't also kept.
    #[test]
    fn edited_events_from_other_apps_lose_and_double_nothing() {
        for (name, text) in [("google", GOOGLE), ("icloud", ICLOUD), ("nextcloud", NEXTCLOUD)] {
            let mut events = parse(text).events;
            let series_at = events.iter().position(|e| e.recurrence_id.is_none()).unwrap();
            let original_series = events.remove(series_at);
            let original_overrides = events;
            for e in std::iter::once(&original_series).chain(&original_overrides) {
                // RFC 5545 §3.6.1: what `write_event` writes from fields
                // isn't also in `kept`, or it would be written twice.
                let mut depth = 0;
                for line in unfold(&e.kept) {
                    let u = line.to_ascii_uppercase();
                    if u.starts_with("BEGIN:") {
                        depth += 1;
                    } else if u.starts_with("END:") {
                        depth -= 1;
                    } else if depth == 0 {
                        let n = split_line(&line).unwrap().0;
                        assert!(!MODELLED.contains(&n.as_str()), "{name}: {n} is modelled and kept");
                    }
                }
            }
            // Edited here: written from the fields, each changed
            // occurrence too.
            let edit = |e: &Event| Event { summary: format!("{} (edited)", e.summary), source: String::new(), ..e.clone() };
            let series = edit(&original_series);
            let overrides: Vec<Event> = original_overrides.iter().map(edit).collect();
            let written = to_store_all(&series, &overrides);
            let back = parse(&written).events;
            assert_eq!(back.len(), 1 + overrides.len(), "{name}");
            let pairs = std::iter::once((&original_series, &back[0])).chain(original_overrides.iter().zip(&back[1..]));
            for (before, after) in pairs {
                assert_eq!(after.summary, format!("{} (edited)", before.summary), "{name}");
                assert_eq!(after.alarms, before.alarms, "{name}: reminders");
                assert_eq!(after.kept, before.kept, "{name}: kept");
                assert_eq!((after.start, after.end, after.recurrence_id), (before.start, before.end, before.recurrence_id), "{name}");
                assert_eq!(after.attendees, before.attendees, "{name}");
                assert_eq!(after.exdates, before.exdates, "{name}");
                // Each of the component's own properties is there as
                // many times as it was, SUMMARY aside.
                let was = own_lines(&before.source);
                let now = own_lines(&after.source);
                for line in &was {
                    let n = split_line(line).unwrap().0;
                    if matches!(n.as_str(), "SUMMARY" | "DTSTART" | "DTEND" | "RECURRENCE-ID" | "EXDATE" | "ATTENDEE" | "ORGANIZER") {
                        // Written from fields, perhaps with parameters in
                        // another order; compared through the fields above.
                        continue;
                    }
                    assert_eq!(
                        now.iter().filter(|l| *l == line).count(),
                        was.iter().filter(|l| *l == line).count(),
                        "{name}: {line} in\n{}",
                        now.join("\n")
                    );
                }
                // As many reminders, none doubled.
                let (alarms_was, alarms_now) = (alarm_blocks(&before.source), alarm_blocks(&after.source));
                assert_eq!(alarms_now.len(), alarms_was.len(), "{name}: {:#?}", alarms_now);
                // A rich reminder comes back exactly as it was.
                for block in alarms_was.iter().filter(|b| !kept_lines(&[vec![String::new(), "BEGIN:VALARM".into()], (*b).clone(), vec!["END:VALARM".into(), String::new()]].concat()).is_empty()) {
                    assert_eq!(alarms_now.iter().filter(|b| *b == block).count(), 1, "{name}: {block:?}");
                }
            }
            // Written again unchanged, nothing grows.
            let (s2, o2) = back.split_first().unwrap();
            let again = to_store_all(&Event { source: String::new(), ..s2.clone() }, &o2.iter().map(|o| Event { source: String::new(), ..o.clone() }).collect::<Vec<_>>());
            assert_eq!(unfold(&again).len(), unfold(&written).len(), "{name}");
        }
    }
    /// The fixtures' details as read.
    #[test]
    fn other_apps_fixtures_are_read() {
        let google = parse(GOOGLE).events;
        assert_eq!(google.len(), 2);
        assert_eq!(google[0].alarms, [600, 86_400]);
        assert!(google[0].kept.contains("ACTION:EMAIL"));
        assert!(google[0].kept.contains("ATTACH;FILENAME=Agenda.pdf"));
        assert_eq!(google[0].status, "CONFIRMED");
        assert!(google[0].kept.contains("CREATED:20261020T080000Z"));
        assert_eq!(google[1].recurrence_id, Some(google[0].start + 7 * 86_400));
        let icloud = parse(ICLOUD).events;
        assert_eq!(icloud[0].alarms, [1800], "{:?}", icloud[0].alarms);
        // The sound, the never-firing default and the acknowledged one
        // are kept whole.
        assert_eq!(icloud[0].kept.matches("BEGIN:VALARM").count(), 3);
        let nextcloud = parse(NEXTCLOUD).events;
        assert_eq!(nextcloud[0].alarms, [900]);
        assert_eq!(nextcloud[0].status, "TENTATIVE");
        assert!(nextcloud[0].kept.contains("REPEAT:2"));
    }
    /// GEO, CATEGORIES, CLASS and RFC 7986's COLOR, IMAGE and CONFERENCE
    /// are read into fields, written from them, and not also kept.
    #[test]
    fn descriptive_properties_are_modelled() {
        let e = parse(NEXTCLOUD).events.remove(0);
        assert_eq!(e.coordinates(), Some((48.208176, 16.373819)));
        assert_eq!(e.categories, ["Work", "Finance"]);
        assert_eq!((e.class.as_str(), e.color.as_str()), ("CONFIDENTIAL", "darkslateblue"));
        assert_eq!(e.conferences.len(), 1);
        let c = &e.conferences[0];
        assert_eq!((c.uri.as_str(), c.label.as_str()), ("https://cloud.example.at/call/x7y8z9", "Talk"));
        assert_eq!(c.features, ["AUDIO", "VIDEO"]);
        for name in ["GEO:", "CATEGORIES:", "CLASS:", "COLOR:", "CONFERENCE"] {
            assert!(!e.kept.contains(name), "{name} kept too");
        }
        // Several CATEGORIES lines, an escaped comma, an image.
        let mut e = parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:i\r\nDTSTART:20261124T090000Z\r\nCATEGORIES:A\\, B,C\r\nCATEGORIES:D\r\nIMAGE;VALUE=URI;DISPLAY=BADGE;FMTTYPE=image/png:https://x.example/i.png\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").events.remove(0);
        assert_eq!(e.categories, ["A, B", "C", "D"]);
        assert_eq!(e.images, [Image { uri: "https://x.example/i.png".into(), media_type: "image/png".into(), display: "BADGE".into(), extra: "VALUE=URI".into() }]);
        e.geo = "-33.8688;151.2093".into();
        e.conferences.push(Conference { uri: "tel:+1-555-0100".into(), label: "Dial in; PIN 42".into(), features: vec!["PHONE".into()], extra: "VALUE=URI".into() });
        e.source.clear();
        let back = parse(&to_store(&e)).events.remove(0);
        assert_eq!(back.categories, e.categories);
        assert_eq!(back.images, e.images);
        assert_eq!(back.conferences, e.conferences);
        assert_eq!(back.coordinates(), Some((-33.8688, 151.2093)));
        assert!(back.kept.is_empty(), "{}", back.kept);
        assert!(to_store(&e).contains("CONFERENCE;VALUE=URI;FEATURE=PHONE;LABEL=\"Dial in; PIN 42\":tel:+1-555-0100"));
    }

    /// An event cached by an earlier version (these properties still in
    /// `kept`, its new fields empty) is written with each of them once.
    #[test]
    fn events_cached_before_the_new_fields_write_them_once() {
        let old = Event {
            uid: "o".into(),
            summary: "Old".into(),
            start: 1_800_000_000,
            end: 1_800_003_600,
            kept: "CATEGORIES:Work\r\nCLASS:PRIVATE\r\nGEO:1.5;2.5\r\nCOLOR:red\r\nCONFERENCE;VALUE=URI:https://x.example/c\r\n".into(),
            ..Default::default()
        };
        let text = to_store(&old);
        for p in ["CATEGORIES:", "CLASS:", "GEO:", "COLOR:", "CONFERENCE"] {
            assert_eq!(text.matches(p).count(), 1, "{p} in {text}");
        }
        // Read again, they're fields, and still once each.
        let back = parse(&text).events.remove(0);
        assert_eq!((back.categories.len(), back.class.as_str(), back.conferences.len()), (1, "PRIVATE", 1));
        assert!(back.kept.is_empty());
    }

    /// Reminders in full (RFC 5545 §3.6.6): their action, repeats and
    /// recipients, the plain ones and those kept whole alike.
    #[test]
    fn reminders_are_read_in_full() {
        let e = parse(NEXTCLOUD).events.remove(0);
        let r = e.reminders();
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].action.as_str(), r[0].before), ("DISPLAY", 900));
        let email = r.iter().find(|a| a.action == "EMAIL").unwrap();
        assert_eq!((email.before, email.attendees.as_slice()), (3600, ["chris@cloud.example.at".to_owned()].as_slice()));
        assert_eq!(email.description, "Starts in an hour");
        let repeating = r.iter().find(|a| a.repeat > 0).unwrap();
        assert_eq!((repeating.before, repeating.repeat, repeating.interval), (300, 2, 300));
        let details = e.details();
        assert!(details.contains(&("Join", "Talk: https://cloud.example.at/call/x7y8z9".into())), "{details:?}");
        assert!(details.contains(&("Categories", "Work, Finance".into())));
        assert!(details.contains(&("Privacy", "Confidential".into())));
        assert!(details.contains(&("Reminders", "email 60 min before".into())));
        // A place is named, so GEO isn't shown as one.
        assert!(!details.iter().any(|(l, _)| *l == "Where"));
        let icloud = parse(ICLOUD).events.remove(0).reminders();
        assert!(icloud.iter().any(|a| a.action == "AUDIO" && a.before == 7200));
        assert!(icloud.iter().any(|a| a.action == "NONE"));
        assert!(icloud.iter().any(|a| a.acknowledged && a.before == 86_400));
    }

    /// RFC 6638 §7: SCHEDULE-STATUS is read and never sent back;
    /// SCHEDULE-AGENT stays as written; SCHEDULE-FORCE-SEND is written.
    #[test]
    fn scheduling_parameters() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s\r\nDTSTART:20261124T090000Z\r\nORGANIZER;SCHEDULE-AGENT=SERVER:mailto:ana@x.example\r\nATTENDEE;PARTSTAT=NEEDS-ACTION;SCHEDULE-STATUS=\"3.7,5.1\":mailto:gone@x.example\r\nATTENDEE;SCHEDULE-STATUS=1.2;SCHEDULE-AGENT=NONE:mailto:bo@x.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = parse(text).events.remove(0);
        assert_eq!(e.attendees[0].schedule_status, ["3.7", "5.1"]);
        assert!(e.attendees[0].delivery_failed() && !e.attendees[1].delivery_failed());
        assert_eq!((e.organizer.as_ref().unwrap().schedule_agent.as_str(), e.attendees[1].schedule_agent.as_str()), ("SERVER", "NONE"));
        // Sent as it came (its source): the server's status is taken out.
        let stored = without_schedule_status(&to_store(&e));
        assert!(!stored.contains("SCHEDULE-STATUS"), "{stored}");
        assert!(stored.contains("ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:gone@x.example") && stored.contains("ATTENDEE;SCHEDULE-AGENT=NONE:mailto:bo@x.example"), "{stored}");
        // Written from fields: the agent kept, the status not, a forced send.
        let mut edited = Event { source: String::new(), ..e };
        edited.attendees[0].force_send = "REQUEST".into();
        let written = to_store(&edited);
        assert!(!written.contains("SCHEDULE-STATUS"));
        assert!(written.contains("ORGANIZER;SCHEDULE-AGENT=SERVER:mailto:ana@x.example"), "{written}");
        assert!(unfold(&written).iter().any(|l| l.starts_with("ATTENDEE;") && l.contains("SCHEDULE-FORCE-SEND=REQUEST") && l.ends_with("gone@x.example")), "{written}");
        // An agent already named isn't overridden for Picacho's own.
        let ours = parse(&scheduled_by_client(&written)).events.remove(0);
        assert_eq!(ours.attendees[1].schedule_agent, "NONE");
        assert_eq!(ours.organizer.unwrap().schedule_agent, "SERVER");
        assert_eq!(scheduled_by_client(&written).matches("SCHEDULE-AGENT=CLIENT").count(), 1);
    }

    /// A guest's list parameter (RFC 5545 §3.2.4 DELEGATED-FROM) is
    /// written back as the list it was, not one quoted value.
    #[test]
    fn attendee_list_parameters_stay_lists() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d\r\nDTSTART:20261124T090000Z\r\nATTENDEE;DELEGATED-FROM=\"mailto:a@x.example\",\"mailto:b@x.example\";PARTSTAT=ACCEPTED;SCHEDULE-STATUS=2.0:mailto:c@x.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut e = parse(text).events.remove(0);
        assert_eq!(e.attendees[0].extra, "DELEGATED-FROM=\"mailto:a@x.example\",\"mailto:b@x.example\"");
        e.source.clear();
        let written = unfold(&to_store(&e)).join("\n");
        assert!(written.contains("ATTENDEE;DELEGATED-FROM=\"mailto:a@x.example\",\"mailto:b@x.example\";PARTSTAT=ACCEPTED:mailto:c@x.example"), "{written}");
    }

    #[test]
    fn edits_keep_what_picacho_doesnt_read() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:k\r\nDTSTART:20261124T090000Z\r\nDTEND:20261124T100000Z\r\nSUMMARY:Review\r\nCATEGORIES:Work,Q4\r\nCLASS:PRIVATE\r\nATTACH:https://x.example/agenda.pdf\r\nCOLOR:teal\r\nATTENDEE;ROLE=OPT-PARTICIPANT;CUTYPE=ROOM;PARTSTAT=ACCEPTED;CN=Room 4:mailto:room@x.example\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nTRIGGER:-PT1H\r\nATTENDEE:mailto:me@x.example\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut e = parse(text).events.remove(0);
        assert_eq!(e.alarms, vec![600], "only the plain reminder is edited");
        e.summary = "Review (moved)".into();
        e.source.clear();
        let written = unfold(&to_store(&e)).join("\n");
        for kept in ["CATEGORIES:Work,Q4", "CLASS:PRIVATE", "ATTACH:https://x.example/agenda.pdf", "COLOR:teal", "ACTION:EMAIL"] {
            assert!(written.contains(kept), "{kept} in {written}");
        }
        assert!(written.contains("ROLE=OPT-PARTICIPANT") && written.contains("CUTYPE=ROOM"));
        assert_eq!(written.matches("BEGIN:VALARM").count(), 2);
        assert_eq!(extra_param(&e.attendees[0].extra, "role").as_deref(), Some("OPT-PARTICIPANT"));
    }
    #[test]
    fn quoted_parameters_and_answers_in_source() {
        let (_, params, value) =
            split_line("ATTENDEE;CN=\"Doe; Jane\";X-NOTE=say ^'hi^';PARTSTAT=NEEDS-ACTION:mailto:j@x.org").unwrap();
        assert_eq!(param(&params, "CN"), Some("Doe; Jane"));
        assert_eq!(param(&params, "X-NOTE"), Some("say \"hi\""));
        assert_eq!(value, "mailto:j@x.org");
        let source = "BEGIN:VEVENT\r\nUID:a\r\nX-OTHER:kept\r\nATTENDEE;RSVP=TRUE;PARTSTAT=NEEDS-ACTION;CN=\"Doe; J\":mailto:j@x.org\r\nEND:VEVENT\r\n";
        let changed = set_partstat_in_source(source, "J@x.org", PartStat::Accepted).unwrap();
        assert!(changed.contains("X-OTHER:kept"));
        assert!(changed.contains("ATTENDEE;CN=\"Doe; J\";PARTSTAT=ACCEPTED:mailto:j@x.org"));
        assert!(set_partstat_in_source(source, "other@x.org", PartStat::Accepted).is_none());
        // An alarm before the start, and one from the end.
        let cal = parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\nBEGIN:VALARM\r\nTRIGGER;RELATED=END:-PT30S\r\nEND:VALARM\r\nDTSTART:20261124T090000Z\r\nDTEND:20261124T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
        assert_eq!(cal.events[0].alarms, vec![-3570]);
        assert_eq!(trigger(90), "-PT90S");
    }
    use super::*;

    /// A rule repeating every second for decades is expanded in bounded
    /// time, not counted through from its start.
    #[test]
    fn rapid_rules_from_long_ago_are_bounded() {
        let started = std::time::Instant::now();
        let now = chrono::Utc::now().timestamp();
        let rapid = rrule_event("DTSTART:19700102T000000Z\nDTEND:19700102T000001Z\nRRULE:FREQ=SECONDLY");
        let _ = occurrences(&rapid[0], &[], now, now + 86_400);
        // One that never happens ends too.
        let never = rrule_event("DTSTART:19700102T000000Z\nRRULE:FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=31");
        let _ = occurrences(&never[0], &[], now, now + 86_400);
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// A value an invitation gave can't break its line and add a property.
    #[test]
    fn written_lines_carry_no_line_breaks() {
        assert_eq!(fold("UID:abc\rATTACH:http://x\n"), "UID:abcATTACH:http://x\r\n");
    }

    /// A day that isn't ASCII is refused, not cut inside a character.
    #[test]
    fn non_ascii_days_are_refused() {
        assert!(Rule::parse("FREQ=WEEKLY;BYDAY=€").is_none());
        assert!(Rule::parse("FREQ=WEEKLY;BYDAY=1€").is_none());
        assert!(Rule::parse("FREQ=WEEKLY;BYDAY=MO").is_some());
    }

    #[test]
    fn zoned_events_carry_their_vtimezone() {
        let tz = vtimezone("Europe/Berlin", 2026..=2026).unwrap();
        assert!(tz.contains("TZID:Europe/Berlin"));
        // 2026: summer time from 29 March 02:00, back on 25 October 03:00.
        assert!(tz.contains("BEGIN:DAYLIGHT\r\nDTSTART:20260329T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200"), "{tz}");
        assert!(tz.contains("DTSTART:20261025T030000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100"), "{tz}");
        // Read back by Picacho's own VTIMEZONE reading: 1 July 2030 12:00
        // in Berlin is 10:00 UTC, past the range given.
        let custom = tz.replace("Europe/Berlin", "Custom Berlin");
        let cal = parse(&format!("BEGIN:VCALENDAR\r\n{custom}BEGIN:VEVENT\r\nUID:c\r\nDTSTART;TZID=Custom Berlin:20300701T120000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"));
        assert_eq!(cal.events[0].start, 1_909_130_400);
        // Sydney starts the year in summer time.
        let sydney = vtimezone("Australia/Sydney", 2026..=2026).unwrap();
        assert!(sydney.starts_with("BEGIN:VTIMEZONE\r\nTZID:Australia/Sydney\r\nBEGIN:DAYLIGHT"), "{sydney}");
        let fixed = vtimezone("Asia/Kolkata", 2026..=2026).unwrap();
        assert!(fixed.contains("TZOFFSETTO:+0530") && !fixed.contains("DAYLIGHT"));
        assert!(vtimezone("Not/AZone", 2026..=2026).is_none());
        let e = Event {
            uid: "z".into(),
            summary: "Call".into(),
            start: 1_795_000_000,
            end: 1_795_003_600,
            tzid: "America/New_York".into(),
            ..Event::default()
        };
        let text = to_store(&e);
        assert_eq!(text.matches("BEGIN:VTIMEZONE").count(), 1);
        assert!(text.find("BEGIN:VTIMEZONE").unwrap() < text.find("BEGIN:VEVENT").unwrap());
        // One already there isn't added twice; the object still parses.
        assert_eq!(with_timezones(&text), text);
        assert_eq!(parse(&text).events.len(), 1);
    }

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
        // Kept whole: the alarm's lines, and each BEGIN with its END.
        assert!(e.source.contains("BEGIN:VALARM\r\nTRIGGER:-P0DT0H10M0S\r\n"));
        assert_eq!(e.source.matches("BEGIN:").count(), e.source.matches("END:").count());
        assert!(e.source.ends_with("END:VALARM\r\nEND:VEVENT\r\n"));
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
        // RFC 5546 §3.2.3: an answer to one occurrence names it.
        let one = Event { recurrence_id: Some(e.start), ..e.clone() };
        let r = reply(&one, "qa@example.com", "QA", PartStat::Declined).unwrap();
        assert_eq!(parse(&r).events[0].recurrence_id, Some(e.start));
        assert!(r.contains("RECURRENCE-ID:20261124T090000Z"), "{r}");
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
        assert_eq!(p.comment, "Later?");
        assert_eq!(p.organizer.as_ref().unwrap().email, "ana@example.org");
        let no = parse(&decline_counter(&e, "qa@example.com"));
        assert_eq!(no.method, "DECLINECOUNTER");
        // With its DTSTART, the guest's app can show which event it was.
        assert_eq!(no.events.len(), 1);
        assert_eq!(no.events[0].start, e.start);
        assert!(decline_counter(&e, "qa@example.com").contains("ATTENDEE:mailto:qa@example.com"));
    }

    #[test]
    fn reminders_tasks_and_free_busy() {
        let cal = parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261124T090000Z\r\nDTEND:20261124T100000Z\r\nSUMMARY:Dentist\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER;RELATED=END:PT0M\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:t\r\nSUMMARY:Taxes\r\nDUE;VALUE=DATE:20261130\r\nEND:VTODO\r\nEND:VCALENDAR\r\n");
        assert_eq!(cal.events.len(), 2);
        let e = &cal.events[0];
        assert_eq!(e.alarms, [900, -3600]);
        assert!(!e.task);
        let t = &cal.events[1];
        assert!(t.task && !t.completed);
        assert_eq!(t.summary, "Taxes");
        assert!(t.start > 0);
        // Written again, they read the same.
        let back = parse(&to_store(&Event { source: String::new(), ..e.clone() })).events.remove(0);
        assert_eq!(back.alarms, [900, -3600]);
        let mut done = t.clone();
        done.completed = true;
        done.source.clear();
        let back = parse(&to_store(&done)).events.remove(0);
        assert!(back.task && back.completed);
        assert_eq!(trigger(86_400), "-P1D");
        assert_eq!(trigger(900), "-PT15M");
        // A task in a message isn't an invitation.
        let mail = "Content-Type: text/calendar\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTODO\r\nUID:t\r\nDUE:20261130T090000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        assert!(from_message(mail.as_bytes()).is_none_or(|c| c.events.is_empty()));
        let fb = parse_free_busy("BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nBEGIN:VFREEBUSY\r\nATTENDEE:mailto:bo@example.se\r\nFREEBUSY;FBTYPE=BUSY:20261124T090000Z/20261124T100000Z,20261124T130000Z/PT30M\r\nFREEBUSY;FBTYPE=FREE:20261124T100000Z/20261124T130000Z\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n");
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].attendee, "bo@example.se");
        assert_eq!(fb[0].busy.len(), 2);
        assert_eq!(fb[0].busy[1].1 - fb[0].busy[1].0, 1800);
        let req = free_busy_request("ana@example.org", &["bo@example.se".into()], 0, 3600);
        assert!(req.contains("BEGIN:VFREEBUSY") && req.contains("ATTENDEE:mailto:bo@example.se"));
    }

    #[test]
    fn stored_guests_are_scheduled_by_picacho() {
        let e = parse(INVITE).events.remove(0);
        let text = scheduled_by_client(&to_store(&e));
        let lines = unfold(&text);
        let people: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("ORGANIZER") || l.starts_with("ATTENDEE"))
            .collect();
        assert_eq!(people.len(), 2);
        assert!(
            people
                .iter()
                .all(|l| l.matches("SCHEDULE-AGENT=CLIENT").count() == 1)
        );
        // The people and their answers read the same, and doing it again
        // changes nothing.
        let back = parse(&text).events.remove(0);
        // Read back with the agent now named (RFC 6638 §7.1), else the same.
        let agentless = |a: &Attendee| Attendee { schedule_agent: String::new(), ..a.clone() };
        assert!(back.attendees.iter().chain(&back.organizer).all(|a| a.schedule_agent == "CLIENT"));
        assert_eq!(back.attendees.iter().map(agentless).collect::<Vec<_>>(), e.attendees);
        assert_eq!(back.organizer.as_ref().map(agentless), e.organizer);
        assert_eq!(scheduled_by_client(&text), text);
        let server = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nATTENDEE;SCHEDULE-AGENT=SERVER:mailto:a@b.c\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(scheduled_by_client(server), server);
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
