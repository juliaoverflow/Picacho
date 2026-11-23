//! Calendars over CalDAV (RFC 4791), through `dav`: finding the user's
//! calendars, reading their events in a time range, and saving and
//! removing single events.
use crate::sync::{dav, ical};
use anyhow::{Context, Result, bail, ensure};

/// Where and how to reach a CalDAV server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub url: String,
    pub user: String,
    /// Calendars kept by one of the mail accounts' JMAP servers (JMAP for
    /// Calendars) instead of a CalDAV server: that account's id here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jmap_account: String,
    /// The server sends invitations, updates and cancellations for events
    /// you organize (RFC 6638 implicit scheduling), instead of Picacho
    /// emailing them. Only set where the server says it schedules
    /// (`calendar-auto-schedule`); off unless chosen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub server_schedules: bool,
}

/// A calendar on the server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Calendar {
    pub href: String,
    pub name: String,
    pub writable: bool,
    /// Its colour (`#RRGGBB`), where the server keeps one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// Its state when last read, to read only what changed since.
    #[serde(default)]
    pub state: dav::State,
    /// The end of the time range last read whole (Unix seconds); later
    /// syncs read only changes until the range needs to move on.
    #[serde(default)]
    pub read_until: i64,
    /// The kinds of item it takes (`VEVENT`, `VTODO`), where the server
    /// says (supported-calendar-component-set); empty: any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// The largest event it takes, in octets (`max-resource-size`); 0
    /// where it doesn't say.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_size: u64,
    /// What else the calendar takes (RFC 4791 §5.2.4–§5.2.9).
    #[serde(default, skip_serializing_if = "is_unlimited")]
    pub limits: dav::CalendarLimits,
}
fn is_unlimited(l: &dav::CalendarLimits) -> bool {
    *l == dav::CalendarLimits::default()
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// An event as kept here: where it is on the server, and its version.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stored {
    /// The event, or for a repeating one, the series.
    pub event: ical::Event,
    /// A repeating event's occurrences changed or cancelled on their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ical::Event>,
    /// The calendar it's in.
    pub calendar: String,
    pub href: String,
    pub etag: String,
    /// Its scheduling version (`Schedule-Tag`, RFC 6638 §3.2), where the
    /// server schedules: guests' answers arriving don't change it, so an
    /// edit is refused only for a change that matters.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_tag: String,
}

fn client(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<dav::Client> {
    dav::Client::new(&server.user, password, tls)
}

/// Whether the server schedules (RFC 6638 §2: `calendar-auto-schedule` in
/// the DAV header of the calendar home), so it can send invitations itself.
pub fn schedules(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<bool> {
    let c = client(server, password, tls)?;
    let home = c.home(&server.url, "caldav", "calendar-home-set", dav::CALDAV)?;
    Ok(c.compliance(&home)?
        .iter()
        .any(|class| class == "calendar-auto-schedule"))
}

/// The account's calendars.
pub fn calendars(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
) -> Result<Vec<Calendar>> {
    let c = client(server, password, tls)?;
    Ok(dav::discover(
        &c,
        &server.url,
        "caldav",
        "calendar",
        "calendar-home-set",
        dav::CALDAV,
    )?
    .into_iter()
    // Calendars for tasks or notes only have no events to show or take,
    // nor those that don't take iCalendar 2.0 (RFC 4791 §5.2.4).
    .filter(|c| {
        (c.components.is_empty() || c.components.iter().any(|k| k == "VEVENT" || k == "VTODO"))
            && c.limits.takes_icalendar()
    })
    .map(|c| Calendar {
        href: c.href.to_string(),
        name: c.name,
        writable: c.writable,
        color: c.color.chars().take(7).collect(),
        state: dav::State {
            sync_token: c.sync_token,
            ctag: c.ctag,
        },
        read_until: 0,
        components: c.components,
        max_size: c.max_size,
        limits: c.limits,
    })
    .collect())
}

fn stamp(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// The query for the events overlapping `from`..`to`.
fn range_query(from: i64, to: i64) -> String {
    format!(
        "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:schedule-tag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"><c:time-range start=\"{}\" end=\"{}\"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>",
        stamp(from),
        stamp(to)
    )
}

/// The query for tasks: all of them that aren't done (a calendar query
/// filters one component kind at a time, RFC 4791 §9.7.1).
const TASK_QUERY: &str = "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:schedule-tag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VTODO\"><c:prop-filter name=\"COMPLETED\"><c:is-not-defined/></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>";

/// The open tasks in `calendars` (VTODO without COMPLETED), soonest due
/// first, then those with no due date.
pub fn tasks(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
) -> Result<Vec<Stored>> {
    let c = client(server, password, tls)?;
    let mut out = Vec::new();
    let mut refused = 0;
    let mut last_error = None;
    for cal in calendars {
        let href = url::Url::parse(&cal.href)?;
        // A calendar that keeps no tasks may refuse the query; skip it.
        let found = match c.list(&href, TASK_QUERY, "calendar-data") {
            Ok(found) => found,
            Err(e) => {
                refused += 1;
                last_error = Some(e);
                continue;
            }
        };
        out.extend(
            found
                .into_iter()
                .filter_map(|r| stored_task(&cal.href, r))
                .filter(|s| !s.event.completed),
        );
    }
    // Every calendar refusing is the server failing, not a lack of tasks.
    if refused > 0
        && refused == calendars.len()
        && let Some(e) = last_error
    {
        return Err(e.context("The calendar server didn't list your tasks."));
    }
    sort_tasks(&mut out);
    Ok(out)
}

/// Tasks soonest due first; those with no due date (0) last, by name.
pub fn sort_tasks(tasks: &mut [Stored]) {
    tasks.sort_by(|a, b| {
        let due = |s: &Stored| {
            if s.event.start == 0 {
                i64::MAX
            } else {
                s.event.start
            }
        };
        due(a)
            .cmp(&due(b))
            .then_with(|| a.event.summary.cmp(&b.event.summary))
    });
}

/// A resource's task (VTODO), if it holds one.
fn stored_task(cal: &str, resource: dav::Resource) -> Option<Stored> {
    let event = ical::parse_tasks(&resource.data).into_iter().next()?;
    Some(Stored {
        event,
        overrides: Vec::new(),
        calendar: cal.to_owned(),
        href: resource.href.to_string(),
        etag: resource.etag,
        schedule_tag: resource.schedule_tag,
    })
}

/// When `attendees` are busy over `from`..`to`, asked of the account's
/// scheduling outbox (RFC 6638 §5: a POST of a VFREEBUSY request). The
/// servers that schedule answer for their own users, and often others'.
pub fn free_busy(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    organizer: &str,
    attendees: &[String],
    from: i64,
    to: i64,
) -> Result<Vec<ical::FreeBusy>> {
    let c = client(server, password, tls)?;
    let outbox = c.outbox(&server.url)?;
    let body = ical::free_busy_request(organizer, attendees, from, to);
    let text = c.post(&outbox, "text/calendar; charset=utf-8", body, &format!("mailto:{organizer}"), attendees)?;
    // A schedule-response: each recipient's calendar-data.
    let doc = roxmltree::Document::parse(&text).context("The server's answer couldn't be read.")?;
    let mut out = Vec::new();
    for data in doc.descendants().filter(|n| n.tag_name().name() == "calendar-data") {
        out.extend(ical::parse_free_busy(data.text().unwrap_or_default()));
    }
    Ok(out)
}

/// When the account is busy in `calendars` over `from`..`to`, by each
/// calendar's free-busy-query REPORT (RFC 4791 §7.10): the server works
/// out the busy time itself, repeats and all, and gives only the periods,
/// not the events. Periods from every calendar, merged and in order.
pub fn own_free_busy(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
    from: i64,
    to: i64,
) -> Result<Vec<(i64, i64)>> {
    ensure!(to > from, "The time asked about ends before it starts.");
    let c = client(server, password, tls)?;
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><c:free-busy-query xmlns:c=\"{}\"><c:time-range start=\"{}\" end=\"{}\"/></c:free-busy-query>",
        dav::CALDAV,
        stamp(from),
        stamp(to)
    );
    let mut busy = Vec::new();
    for cal in calendars.iter().filter(|c| c.components.is_empty() || c.components.iter().any(|k| k == "VEVENT")) {
        // RFC 4791 §7.10: a 200 with a text/calendar VFREEBUSY.
        let text = c.report_text(&url::Url::parse(&cal.href)?, &body)?;
        busy.extend(ical::parse_free_busy(&text).into_iter().flat_map(|f| f.busy));
    }
    busy.sort();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in busy {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    Ok(merged)
}

/// A resource's events as kept: the series (or single event) and its
/// changed occurrences. Resources with no event (a task) give none.
fn stored(cal: &str, resource: dav::Resource) -> Option<Stored> {
    // A repeating event's changed occurrences share its resource:
    // the series is the one without a RECURRENCE-ID.
    let mut events = ical::parse(&resource.data).events;
    // Tasks are kept apart (`tasks`); a calendar holding both lists events here.
    events.retain(|e| !e.task);
    let series = events
        .iter()
        .position(|e| e.recurrence_id.is_none())
        .unwrap_or(0);
    if events.is_empty() {
        return None;
    }
    let event = events.remove(series);
    Some(Stored {
        event,
        overrides: events,
        calendar: cal.to_owned(),
        href: resource.href.to_string(),
        etag: resource.etag,
        schedule_tag: resource.schedule_tag,
    })
}

/// The events of `calendars` that overlap `from`..`to` (Unix seconds).
#[cfg(test)]
pub fn events(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
    from: i64,
    to: i64,
) -> Result<Vec<Stored>> {
    let c = client(server, password, tls)?;
    let query = range_query(from, to);
    let mut out = Vec::new();
    for cal in calendars {
        let href = url::Url::parse(&cal.href)?;
        out.extend(
            c.list(&href, &query, "calendar-data")?
                .into_iter()
                .filter_map(|r| stored(&cal.href, r)),
        );
    }
    out.sort_by_key(|s| s.event.start);
    Ok(out)
}

/// How much further than asked a whole read goes, so the syncs after it
/// can read only changes for a while before the range must move on.
const MARGIN: i64 = 30 * 86_400;

/// Reads `calendars`' events overlapping `from`..`to`, downloading only
/// what changed since `previous` (the calendars as last read) and `known`
/// (the events read then) where the server can say: an unchanged ctag or
/// sync token costs nothing, a sync token gives just the changes. A
/// calendar read whole is read `MARGIN` further ahead. Returns the
/// calendars with their new state, and every event.
pub fn sync(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
    previous: &[Calendar],
    known: &[Stored],
    (from, to): (i64, i64),
) -> Result<(Vec<Calendar>, Vec<Stored>)> {
    let c = client(server, password, tls)?;
    let mut out = Vec::new();
    let mut cals = Vec::with_capacity(calendars.len());
    for cal in calendars {
        // A calendar only for tasks has no events to ask about (its tasks
        // are read by `tasks`).
        if !cal.components.is_empty() && !cal.components.iter().any(|k| k == "VEVENT") {
            cals.push(cal.clone());
            continue;
        }
        let href = url::Url::parse(&cal.href)?;
        let before = previous
            .iter()
            .find(|p| p.href == cal.href)
            .filter(|p| p.read_until >= to);
        let kept_events: Vec<&Stored> = known.iter().filter(|s| s.calendar == cal.href).collect();
        let collection = dav::Collection {
            href: href.clone(),
            name: cal.name.clone(),
            writable: cal.writable,
            components: Vec::new(),
            color: cal.color.clone(),
            sync_token: cal.state.sync_token.clone(),
            ctag: cal.state.ctag.clone(),
            max_size: 0,
            vcard_versions: Vec::new(),
            limits: cal.limits.clone(),
        };
        // RFC 4791 §5.2.6–§5.2.7: a time range outside the calendar's
        // dates is refused (min-date-time, max-date-time).
        let (q_from, q_to) = cal.limits.clamp(from, to + MARGIN);
        let query = range_query(q_from, q_to);
        let read = dav::read_collection(
            &c,
            &collection,
            before.map(|p| &p.state),
            &kept_events
                .iter()
                .map(|s| (s.href.clone(), s.etag.clone()))
                .collect::<Vec<_>>(),
            |c| c.list(&href, &query, "calendar-data"),
            |c, hrefs| c.multiget(&href, "calendar-multiget", dav::CALDAV, "calendar-data", hrefs),
        )?;
        let whole = read.whole;
        out.extend(
            kept_events
                .into_iter()
                .filter(|s| read.kept.contains(&s.href))
                .cloned(),
        );
        out.extend(read.fetched.into_iter().filter_map(|r| stored(&cal.href, r)));
        cals.push(Calendar {
            state: read.state,
            read_until: if whole {
                to + MARGIN
            } else {
                before.map_or(0, |p| p.read_until)
            },
            ..cal.clone()
        });
    }
    out.sort_by_key(|s| s.event.start);
    Ok((cals, out))
}

/// Makes a calendar named `name` in the account's calendar home, with
/// `setup`'s description and time zone (RFC 4791 §5.3.1).
pub fn create_calendar(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    name: &str,
    color: &str,
    setup: &dav::CollectionSetup,
) -> Result<Calendar> {
    let c = client(server, password, tls)?;
    let home = c.home(&server.url, "caldav", "calendar-home-set", dav::CALDAV)?;
    let href = dav::new_href(&home, &uuid::Uuid::new_v4().to_string(), "x")?;
    // A collection: the path without the extension, ending in '/'.
    let href = url::Url::parse(&format!("{}/", href.as_str().trim_end_matches(".x")))?;
    c.make_collection_with(&href, true, name, color, setup)?;
    Ok(Calendar {
        href: href.to_string(),
        name: name.to_owned(),
        writable: true,
        color: color.to_owned(),
        ..Default::default()
    })
}

/// Whether `stored` fits `limits` (RFC 4791 §5.2.6–§5.2.9), checked
/// before it is sent: the server would refuse it with a precondition
/// (`min-date-time`, `max-instances`, `max-attendees-per-instance`) that
/// says less.
pub fn check_limits(limits: &dav::CalendarLimits, stored: &Stored) -> Result<()> {
    let events = std::iter::once(&stored.event).chain(&stored.overrides);
    if let Some(most) = limits.max_attendees {
        let guests = events.clone().map(|e| e.attendees.len()).max().unwrap_or(0) as u64;
        ensure!(guests <= most, "This calendar takes at most {most} guests an event.");
    }
    for e in events.filter(|e| e.start != 0) {
        let (from, to) = limits.clamp(e.start, e.end.max(e.start));
        ensure!(
            from == e.start && to == e.end.max(e.start),
            "This calendar doesn't take events at that date."
        );
    }
    if let Some(most) = limits.max_instances
        && (!stored.event.recurrence.is_empty() || !stored.event.rdates.is_empty())
    {
        // Counted over the calendar's dates, or a century: a rule without
        // an end is past any limit.
        let to = limits.max_time.unwrap_or(stored.event.start + 100 * 366 * 86_400);
        let count = ical::occurrences(&stored.event, &[], stored.event.start, to).len() as u64
            + stored.event.exdates.len() as u64;
        ensure!(
            count <= most,
            "This calendar takes at most {most} repeats of an event. Give the repeats an end."
        );
    }
    Ok(())
}

/// Saves an event: new in `calendar`, or a change to the one at its href.
/// `server_schedules_this`: with `Server::server_schedules`, the server
/// sends this one's invitations (an event organized here); otherwise
/// every ORGANIZER and ATTENDEE is marked for Picacho to schedule.
pub fn save(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendar: &str,
    stored: &Stored,
    server_schedules_this: bool,
    max_size: Option<u64>,
) -> Result<Stored> {
    ensure!(!stored.event.uid.is_empty(), "The event has no identifier.");
    let c = client(server, password, tls)?;
    let href = if stored.href.is_empty() {
        dav::new_href(&url::Url::parse(calendar)?, &stored.event.uid, "ics")?
    } else {
        url::Url::parse(&stored.href)?
    };
    let text = if stored.event.task {
        // Changed in place, keeping what other apps set on it.
        ical::task_to_store(&stored.event)
    } else if stored.overrides.is_empty() {
        ical::to_store(&stored.event)
    } else {
        ical::to_store_all(&stored.event, &stored.overrides)
    };
    // Picacho emails invitations and answers itself (iMIP), so the server
    // mustn't send them again; unless the server is asked to schedule, in
    // which case it sends them and Picacho doesn't.
    let text = if server.server_schedules && server_schedules_this {
        text
    } else {
        ical::scheduled_by_client(&text)
    };
    if let Some(max) = max_size.filter(|m| *m > 0)
        && text.len() as u64 > max
    {
        bail!(
            "This event is too large for the calendar ({} KB at most). Shorten its notes or remove guests.",
            max / 1024
        );
    }
    // With a Schedule-Tag, only a change that matters stops the write; a
    // guest's answer arriving meanwhile doesn't (RFC 6638 §3.2.10).
    let (etag, schedule_tag) = c.put_scheduled(
        &href,
        "text/calendar; charset=utf-8",
        &stored.etag,
        &stored.schedule_tag,
        text,
    )?;
    Ok(Stored {
        event: stored.event.clone(),
        overrides: stored.overrides.clone(),
        calendar: calendar.to_owned(),
        href: href.to_string(),
        etag,
        schedule_tag,
    })
}

/// Renames a calendar, and changes its colour (empty: as it is).
pub fn rename_calendar(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    href: &str,
    name: &str,
    color: &str,
) -> Result<()> {
    ensure!(!name.trim().is_empty(), "Give the calendar a name.");
    client(server, password, tls)?.rename_collection(&url::Url::parse(href)?, name.trim(), color)
}

/// Deletes a calendar and its events from the server.
pub fn delete_calendar(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    href: &str,
) -> Result<()> {
    client(server, password, tls)?.delete_collection(&url::Url::parse(href)?)
}

/// The account's storage on the calendar server, used and left in
/// octets, where it says (RFC 4331).
pub fn quota(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<Option<(u64, u64)>> {
    let c = client(server, password, tls)?;
    let home = c.home(&server.url, "caldav", "calendar-home-set", dav::CALDAV)?;
    c.quota(&home)
}

/// What the server delivered to the scheduling inbox (RFC 6638 §4): with
/// the server scheduling, invitations and answers from others arrive
/// here rather than by mail. Each is (its href and ETag, the iTIP object).
pub fn inbox(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
) -> Result<Vec<dav::Resource>> {
    let c = client(server, password, tls)?;
    let inbox = c.inbox(&server.url)?;
    const ALL: &str = "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:schedule-tag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"/></c:filter></c:calendar-query>";
    c.list(&inbox, ALL, "calendar-data")
}

/// Removes a handled message from the scheduling inbox.
pub fn remove_from_inbox(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    item: &dav::Resource,
) -> Result<()> {
    client(server, password, tls)?.delete(&item.href, &item.etag)
}

/// Attaches a file to an event where the calendar keeps attachments itself
/// (managed attachments, RFC 8607). Returns the event's new ETag; the
/// server adds the ATTACH to it, so read the event again to show it.
pub fn attach(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    stored: &Stored,
    path: &std::path::Path,
) -> Result<String> {
    ensure!(!stored.href.is_empty(), "Save the event before attaching files.");
    let c = client(server, password, tls)?;
    let calendar = url::Url::parse(&stored.calendar)?;
    ensure!(
        c.manages_attachments(&calendar)?,
        "This calendar server can't keep attachments."
    );
    // Servers refuse large files anyway; don't read a huge one into memory.
    const MOST: u64 = 100 * 1024 * 1024;
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= MOST, "This file is too large to attach to an event (100 MB at most).");
    let data = std::fs::read(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_owned();
    let kind = mime_guess::from_path(path).first_or_octet_stream().to_string();
    c.add_attachment(&url::Url::parse(&stored.href)?, &stored.etag, &name, &kind, data)
}

/// Removes an event.
pub fn delete(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    stored: &Stored,
) -> Result<()> {
    if stored.href.is_empty() {
        return Ok(());
    }
    client(server, password, tls)?.delete(&url::Url::parse(&stored.href)?, &stored.etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendars_renamed_deleted_and_scheduled() -> Result<()> {
        let (port, server) = dav::test_server(7, |method, path, body| match (method, path) {
            ("PROPPATCH", "/cal/ana/home/") => {
                assert!(body.contains("<d:displayname>Family</d:displayname>"));
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/cal/ana/home/</d:href><d:propstat><d:prop><d:displayname/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into())
            }
            ("DELETE", "/cal/ana/old/") => ("204 No Content", String::new()),
            ("PUT", "/cal/ana/home/e.ics") => {
                // Edited over its scheduling version, not its ETag.
                assert!(body.to_ascii_lowercase().starts_with("if-schedule-tag-match: \"s-0\""), "{body}");
                assert!(body.contains("BEGIN:VTIMEZONE"), "{body}");
                ("204 No Content", String::new())
            }
            ("PROPFIND", "/") | ("PROPFIND", "/.well-known/caldav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/p/ana/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/ana/</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/p/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/p/ana/</d:href><d:propstat><d:prop><c:schedule-inbox-URL><d:href>/inbox/ana/</d:href></c:schedule-inbox-URL></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", "/inbox/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/inbox/ana/r1.ics</d:href><d:propstat><d:prop><d:getetag>\"r1\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR\nMETHOD:REPLY\nBEGIN:VEVENT\nUID:e\nDTSTART:20261124T090000Z\nSUMMARY:Lunch\nATTENDEE;PARTSTAT=ACCEPTED;CN=Bo:mailto:bo@example.se\nEND:VEVENT\nEND:VCALENDAR\n</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("DELETE", "/inbox/ana/r1.ics") => ("204 No Content", String::new()),
            _ => ("404 Not Found", String::new()),
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let srv = Server {
            url: format!("http://127.0.0.1:{port}"),
            user: "ana".into(),
            ..Default::default()
        };
        let home = format!("http://127.0.0.1:{port}/cal/ana/home/");
        rename_calendar(&srv, "secret", tls(), &home, "Family", "")?;
        delete_calendar(&srv, "secret", tls(), &format!("http://127.0.0.1:{port}/cal/ana/old/"))?;
        let edited = Stored {
            event: ical::Event {
                uid: "e".into(),
                summary: "Lunch".into(),
                start: 1_795_000_000,
                end: 1_795_003_600,
                tzid: "Europe/Berlin".into(),
                ..Default::default()
            },
            calendar: home.clone(),
            href: format!("{home}e.ics"),
            etag: "\"e1\"".into(),
            schedule_tag: "\"s-0\"".into(),
            ..Default::default()
        };
        let saved = save(&srv, "secret", tls(), &home, &edited, false, Some(1_000_000))?;
        assert_eq!(saved.schedule_tag, "\"s-1\"");
        // Too large for the calendar: refused before anything is sent.
        assert!(save(&srv, "secret", tls(), &home, &edited, false, Some(10)).is_err());
        let items = inbox(&srv, "secret", tls())?;
        assert_eq!(items.len(), 1);
        assert!(items[0].data.contains("METHOD:REPLY"));
        remove_from_inbox(&srv, "secret", tls(), &items[0])?;
        let seen = server.join().unwrap();
        assert_eq!(seen.iter().filter(|(m, ..)| m == "PUT").count(), 1);
        Ok(())
    }

    /// RFC 4791 §5.2.4–§5.2.9: a calendar's limits are read with it, and
    /// an event past them is refused before it is sent. MKCALENDAR sends
    /// a description and time zone (§5.3.1) and reads what was refused.
    #[test]
    fn calendar_limits_and_new_calendars() -> Result<()> {
        let (port, server) = dav::test_server(7, |method, path, body| match (method, path) {
            ("PROPFIND", "/.well-known/caldav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/.well-known/caldav</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/cal/a/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-data><c:calendar-data content-type=\"text/calendar\" version=\"2.0\"/></c:supported-calendar-data><c:min-date-time>19000101T000000Z</c:min-date-time><c:max-date-time>20491231T235959Z</c:max-date-time><c:max-instances>100</c:max-instances><c:max-attendees-per-instance>2</c:max-attendees-per-instance></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/cal/json/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-data><c:calendar-data content-type=\"application/calendar+json\"/></c:supported-calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", _) => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/p/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/cal/</d:href></c:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("MKCALENDAR", _) => {
                assert!(body.contains("<c:calendar-timezone>BEGIN:VCALENDAR") && body.contains("TZID:Europe/Vienna"), "{body}");
                assert!(body.contains("<c:calendar-description>Kids &amp; school</c:calendar-description>"), "{body}");
                if body.contains("refuse-me") {
                    ("403 Forbidden", "<c:mkcalendar-response xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:propstat><d:prop><c:calendar-timezone/></d:prop><d:status>HTTP/1.1 403 Forbidden</d:status></d:propstat><d:propstat><d:prop><d:displayname/></d:prop><d:status>HTTP/1.1 424 Failed Dependency</d:status></d:propstat></c:mkcalendar-response>".into())
                } else {
                    ("201 Created", String::new())
                }
            }
            _ => ("404 Not Found", String::new()),
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let srv = Server { url: format!("http://127.0.0.1:{port}"), user: "ana".into(), ..Default::default() };
        let cals = calendars(&srv, "secret", tls())?;
        // The jCal-only calendar can't take Picacho's events.
        assert_eq!(cals.len(), 1);
        let l = &cals[0].limits;
        assert_eq!((l.max_instances, l.max_attendees), (Some(100), Some(2)));
        assert_eq!(l.max_time, Some(2_524_607_999));
        assert_eq!(l.clamp(-3_000_000_000, 3_000_000_000), (-2_208_988_800, 2_524_607_999));
        let setup = dav::CollectionSetup { description: "Kids & school".into(), timezone: Some("Europe/Vienna".into()) };
        create_calendar(&srv, "secret", tls(), "School", "", &setup)?;
        let refused = create_calendar(&srv, "secret", tls(), "refuse-me", "", &setup).unwrap_err();
        assert!(refused.to_string().contains("calendar-timezone"), "{refused}");
        // Discovery, then for each new calendar the principal, its home
        // and MKCALENDAR.
        assert_eq!(server.join().unwrap().len(), 7);
        // An event within them goes; too many guests, too late, or
        // repeating without end don't.
        let guest = |e: &str| ical::Attendee { email: e.into(), ..Default::default() };
        let ok = Stored { event: ical::Event { uid: "e".into(), start: 1_800_000_000, end: 1_800_003_600, attendees: vec![guest("a@x"), guest("b@x")], ..Default::default() }, ..Default::default() };
        check_limits(l, &ok)?;
        let mut crowded = ok.clone();
        crowded.event.attendees.push(guest("c@x"));
        assert!(check_limits(l, &crowded).unwrap_err().to_string().contains("2 guests"));
        let mut late = ok.clone();
        late.event.start = 2_600_000_000;
        late.event.end = 2_600_003_600;
        assert!(check_limits(l, &late).is_err());
        let mut endless = ok.clone();
        endless.event.recurrence = "FREQ=WEEKLY".into();
        assert!(check_limits(l, &endless).unwrap_err().to_string().contains("100 repeats"));
        endless.event.recurrence = "FREQ=WEEKLY;COUNT=10".into();
        check_limits(l, &endless)?;
        Ok(())
    }

    /// RFC 4791 §7.10: each calendar is asked for its busy time over the
    /// range; overlapping periods from two calendars become one.
    #[test]
    fn own_busy_time_comes_from_free_busy_query() -> Result<()> {
        let (port, server) = dav::test_server(2, |method, path, body| {
            assert_eq!(method, "REPORT");
            assert!(body.contains("<c:free-busy-query") && body.contains("start=\"20261124T000000Z\" end=\"20261125T000000Z\""), "{body}");
            let periods = if path == "/cal/work/" { "20261124T090000Z/20261124T100000Z" } else { "20261124T093000Z/PT1H,20261124T150000Z/20261124T160000Z" };
            ("200 OK", format!("BEGIN:VCALENDAR\r\nBEGIN:VFREEBUSY\r\nDTSTART:20261124T000000Z\r\nDTEND:20261125T000000Z\r\nFREEBUSY:{periods}\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n"))
        });
        let srv = Server { url: format!("http://127.0.0.1:{port}"), user: "ana".into(), ..Default::default() };
        let cal = |p: &str| Calendar { href: format!("http://127.0.0.1:{port}{p}"), ..Default::default() };
        // A task list isn't asked.
        let tasks = Calendar { components: vec!["VTODO".into()], ..cal("/cal/tasks/") };
        let from = 1_795_478_400; // 2026-11-24T00:00Z
        let busy = own_free_busy(&srv, "secret", native_tls::TlsConnector::new()?, &[cal("/cal/work/"), cal("/cal/home/"), tasks], from, from + 86_400)?;
        assert_eq!(busy, [(from + 9 * 3600, from + 10 * 3600 + 1800), (from + 15 * 3600, from + 16 * 3600)]);
        assert_eq!(server.join().unwrap().len(), 2);
        Ok(())
    }

    #[test]
    fn calendars_events_and_changes() -> Result<()> {
        let (port, server) = dav::test_server(5, |method, path, body| {
            match (method, path) {
            ("PROPFIND", "/.well-known/caldav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/ana/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/cal/ana/work/</d:href><d:propstat><d:prop><d:displayname>Work</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set></d:prop></d:propstat></d:response><d:response><d:href>/cal/ana/home/</d:href><d:propstat><d:prop><d:displayname>Home</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("REPORT", "/cal/ana/home/") => {
                assert!(body.contains("time-range start=\"20261101T000000Z\""));
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/ana/home/a.ics</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART:20261124T090000Z\nDTEND:20261124T100000Z\nSUMMARY:Dentist\nEND:VEVENT\nEND:VCALENDAR\n</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>".into())
            }
            ("REPORT", "/cal/ana/work/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"/>".into()),
            ("PUT", "/cal/ana/home/new.ics") => {
                assert!(body.contains("SUMMARY:Lunch"));
                assert!(body.contains("ATTENDEE;SCHEDULE-AGENT=CLIENT;"));
                ("201 Created", String::new())
            }
            ("DELETE", "/cal/ana/home/a.ics") => ("204 No Content", String::new()),
            _ => ("404 Not Found", String::new()),
        }
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let srv = Server {
            url: format!("http://127.0.0.1:{port}"),
            user: "ana".into(),
            ..Default::default()
        };
        let cals = calendars(&srv, "secret", tls())?;
        assert_eq!(
            cals.iter()
                .map(|c| (c.name.as_str(), c.writable))
                .collect::<Vec<_>>(),
            [("Work", false), ("Home", true)]
        );
        let events = events(&srv, "secret", tls(), &cals, 1_793_491_200, 1_796_083_200)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.summary, "Dentist");
        assert_eq!(events[0].etag, "\"1\"");
        let new = Stored {
            event: ical::Event {
                uid: "new".into(),
                summary: "Lunch".into(),
                start: 1_795_000_000,
                end: 1_795_003_600,
                organizer: Some(ical::Attendee {
                    email: "ana@example.org".into(),
                    ..Default::default()
                }),
                attendees: vec![ical::Attendee {
                    email: "bo@example.se".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let saved = save(&srv, "secret", tls(), &cals[1].href, &new, false, None)?;
        assert!(saved.href.ends_with("/cal/ana/home/new.ics"));
        delete(&srv, "secret", tls(), &events[0])?;
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 5);
        Ok(())
    }

    #[test]
    fn tasks_are_listed_kept_apart_from_events_and_edited_in_place() -> Result<()> {
        let todo = "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:t1\nSUMMARY:Taxes\nDTSTART:20261101T090000Z\nDUE:20261130T170000Z\nPRIORITY:1\nCATEGORIES:Home\nX-APPLE-SORT-ORDER:7\nEND:VTODO\nEND:VCALENDAR\n";
        let undated =
            "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:t2\nSUMMARY:Someday\nEND:VTODO\nEND:VCALENDAR\n";
        let done = "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:t3\nSUMMARY:Old\nSTATUS:COMPLETED\nEND:VTODO\nEND:VCALENDAR\n";
        let response = move |items: &[(&str, &str)]| {
            let mut body = String::from(
                "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">",
            );
            for (href, data) in items {
                body.push_str(&format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>\"1\"</d:getetag><c:calendar-data>{data}</c:calendar-data></d:prop></d:propstat></d:response>"));
            }
            body + "</d:multistatus>"
        };
        let (port, server) = dav::test_server(2, move |method, path, body| match (method, path) {
            ("REPORT", "/cal/ana/home/") => {
                assert!(body.contains("name=\"VTODO\""));
                (
                    "207 Multi-Status",
                    response(&[
                        ("/cal/ana/home/t1.ics", todo),
                        ("/cal/ana/home/t2.ics", undated),
                        ("/cal/ana/home/t3.ics", done),
                    ]),
                )
            }
            ("PUT", "/cal/ana/home/t1.ics") => {
                // What other apps set is kept; what changed is rewritten.
                assert!(body.contains("PRIORITY:1"), "{body}");
                assert!(body.contains("CATEGORIES:Home"));
                assert!(body.contains("X-APPLE-SORT-ORDER:7"));
                assert!(body.contains("STATUS:COMPLETED"));
                assert!(body.contains("SUMMARY:Taxes 2026"));
                assert_eq!(body.matches("SUMMARY:").count(), 1);
                assert_eq!(body.matches("BEGIN:VTODO").count(), 1);
                assert!(!body.contains("BEGIN:VEVENT"));
                ("204 No Content", String::new())
            }
            _ => ("404 Not Found", String::new()),
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let srv = Server {
            url: format!("http://127.0.0.1:{port}"),
            user: "ana".into(),
            ..Default::default()
        };
        let cal = Calendar {
            href: format!("http://127.0.0.1:{port}/cal/ana/home/"),
            writable: true,
            ..Default::default()
        };
        let found = tasks(&srv, "secret", tls(), std::slice::from_ref(&cal))?;
        // Open ones only; the dated first, the undated last.
        assert_eq!(
            found
                .iter()
                .map(|t| t.event.uid.as_str())
                .collect::<Vec<_>>(),
            ["t1", "t2"]
        );
        let t1 = &found[0].event;
        // DUE is when it's due, though it has a DTSTART too.
        assert_eq!(t1.start, 1_796_058_000);
        assert_eq!(found[1].event.start, 0);
        let mut changed = found[0].clone();
        changed.event.summary = "Taxes 2026".into();
        changed.event.completed = true;
        save(&srv, "secret", tls(), &cal.href, &changed, false, None)?;
        server.join().unwrap();
        // A calendar's events don't include its tasks.
        let both = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:e\nDTSTART:20261124T090000Z\nDTEND:20261124T100000Z\nSUMMARY:Dentist\nEND:VEVENT\nEND:VCALENDAR\n".to_owned();
        let r = dav::Resource {
            href: url::Url::parse("https://x/e.ics")?,
            etag: String::new(),
            data: both,
            schedule_tag: String::new(),
        };
        assert!(!stored("c", r).unwrap().event.task);
        let r = dav::Resource {
            href: url::Url::parse("https://x/t.ics")?,
            etag: String::new(),
            data: todo.into(),
            schedule_tag: String::new(),
        };
        assert!(stored("c", r).is_none());
        Ok(())
    }

    #[test]
    fn the_server_schedules_only_where_asked_and_only_events_organized_here() -> Result<()> {
        let (port, server) = dav::test_server(5, |method, path, body| {
            match (method, path) {
            ("PROPFIND", "/.well-known/caldav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/.well-known/caldav</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/ana/</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/p/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/p/ana/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/cal/ana/</d:href></c:calendar-home-set></d:prop></d:propstat></d:response></d:multistatus>".into()),
            ("OPTIONS", "/cal/ana/") => ("200 OK", String::new()),
            ("PUT", "/cal/ana/home/mine.ics") => {
                // The server is left to schedule this one.
                assert!(!body.contains("SCHEDULE-AGENT"), "{body}");
                ("201 Created", String::new())
            }
            ("PUT", "/cal/ana/home/theirs.ics") => {
                assert!(body.contains("ATTENDEE;SCHEDULE-AGENT=CLIENT;"), "{body}");
                assert!(body.contains("ORGANIZER;SCHEDULE-AGENT=CLIENT"), "{body}");
                ("201 Created", String::new())
            }
            _ => ("404 Not Found", String::new()),
        }
        });
        let tls = || native_tls::TlsConnector::new().unwrap();
        let mut srv = Server {
            url: format!("http://127.0.0.1:{port}"),
            user: "ana".into(),
            ..Default::default()
        };
        assert!(schedules(&srv, "secret", tls())?);
        srv.server_schedules = true;
        let event = |uid: &str| Stored {
            event: ical::Event {
                uid: uid.into(),
                summary: "Lunch".into(),
                start: 1_795_000_000,
                end: 1_795_003_600,
                organizer: Some(ical::Attendee {
                    email: "ana@example.org".into(),
                    ..Default::default()
                }),
                attendees: vec![ical::Attendee {
                    email: "bo@example.se".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let home = format!("http://127.0.0.1:{port}/cal/ana/home/");
        save(&srv, "secret", tls(), &home, &event("mine"), true, None)?;
        // An invitation from someone else is still answered by mail.
        save(&srv, "secret", tls(), &home, &event("theirs"), false, None)?;
        let seen = server.join().unwrap();
        assert_eq!(seen[2].0, "OPTIONS");
        Ok(())
    }
}
