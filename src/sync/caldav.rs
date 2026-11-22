//! Calendars over CalDAV (RFC 4791), through `dav`: finding the user's
//! calendars, reading their events in a time range, and saving and
//! removing single events.
use crate::sync::{dav, ical};
use anyhow::{Context, Result, ensure};

/// Where and how to reach a CalDAV server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub url: String,
    pub user: String,
    /// Calendars kept by one of the mail accounts' JMAP servers (JMAP for
    /// Calendars) instead of a CalDAV server: that account's id here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jmap_account: String,
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
}

fn client(server: &Server, password: &str, tls: native_tls::TlsConnector) -> Result<dav::Client> {
    dav::Client::new(&server.user, password, tls)
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
    // Calendars for tasks or notes only have no events to show or take.
    .filter(|c| {
        c.components.is_empty() || c.components.iter().any(|k| k == "VEVENT" || k == "VTODO")
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
        "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"><c:time-range start=\"{}\" end=\"{}\"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>",
        stamp(from),
        stamp(to)
    )
}

/// The query for tasks: all of them that aren't done (a calendar query
/// filters one component kind at a time, RFC 4791 §9.7.1).
const TASK_QUERY: &str = "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VTODO\"><c:prop-filter name=\"COMPLETED\"><c:is-not-defined/></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>";

/// The open tasks in `calendars` (VTODO without COMPLETED).
pub fn tasks(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
) -> Result<Vec<Stored>> {
    let c = client(server, password, tls)?;
    let mut out = Vec::new();
    for cal in calendars {
        let href = url::Url::parse(&cal.href)?;
        // A calendar that keeps no tasks may refuse the query; skip it.
        let Ok(found) = c.list(&href, TASK_QUERY, "calendar-data") else {
            continue;
        };
        out.extend(
            found
                .into_iter()
                .filter_map(|r| stored(&cal.href, r))
                .filter(|s| s.event.task),
        );
    }
    out.sort_by_key(|s| s.event.start);
    Ok(out)
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

/// A resource's events as kept: the series (or single event) and its
/// changed occurrences. Resources with no event (a task) give none.
fn stored(cal: &str, resource: dav::Resource) -> Option<Stored> {
    // A repeating event's changed occurrences share its resource:
    // the series is the one without a RECURRENCE-ID.
    let mut events = ical::parse(&resource.data).events;
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
        };
        let query = range_query(from, to + MARGIN);
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

/// Makes a calendar named `name` in the account's calendar home.
pub fn create_calendar(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    name: &str,
    color: &str,
) -> Result<Calendar> {
    let c = client(server, password, tls)?;
    let home = c.home(&server.url, "caldav", "calendar-home-set", dav::CALDAV)?;
    let href = dav::new_href(&home, &uuid::Uuid::new_v4().to_string(), "x")?;
    // A collection: the path without the extension, ending in '/'.
    let href = url::Url::parse(&format!("{}/", href.as_str().trim_end_matches(".x")))?;
    c.make_collection(&href, true, name, color)?;
    Ok(Calendar {
        href: href.to_string(),
        name: name.to_owned(),
        writable: true,
        color: color.to_owned(),
        ..Default::default()
    })
}

/// Saves an event: new in `calendar`, or a change to the one at its href.
pub fn save(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendar: &str,
    stored: &Stored,
) -> Result<Stored> {
    ensure!(!stored.event.uid.is_empty(), "The event has no identifier.");
    let c = client(server, password, tls)?;
    let href = if stored.href.is_empty() {
        dav::new_href(&url::Url::parse(calendar)?, &stored.event.uid, "ics")?
    } else {
        url::Url::parse(&stored.href)?
    };
    let text = if stored.overrides.is_empty() {
        ical::to_store(&stored.event)
    } else {
        ical::to_store_all(&stored.event, &stored.overrides)
    };
    // Picacho emails invitations and answers itself (iMIP), so the server
    // mustn't send them again.
    let text = ical::scheduled_by_client(&text);
    let etag = c.put(&href, "text/calendar; charset=utf-8", &stored.etag, text)?;
    Ok(Stored {
        event: stored.event.clone(),
        overrides: stored.overrides.clone(),
        calendar: calendar.to_owned(),
        href: href.to_string(),
        etag,
    })
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
        let saved = save(&srv, "secret", tls(), &cals[1].href, &new)?;
        assert!(saved.href.ends_with("/cal/ana/home/new.ics"));
        delete(&srv, "secret", tls(), &events[0])?;
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 5);
        Ok(())
    }
}
