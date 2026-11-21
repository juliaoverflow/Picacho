//! Calendars over CalDAV (RFC 4791), through `dav`: finding the user's
//! calendars, reading their events in a time range, and saving and
//! removing single events.
use crate::sync::{dav, ical};
use anyhow::{Result, ensure};

/// Where and how to reach a CalDAV server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub url: String,
    pub user: String,
}

/// A calendar on the server.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Calendar {
    pub href: String,
    pub name: String,
    pub writable: bool,
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
    .map(|c| Calendar {
        href: c.href.to_string(),
        name: c.name,
        writable: c.writable,
    })
    .collect())
}

fn stamp(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// The events of `calendars` that overlap `from`..`to` (Unix seconds).
pub fn events(
    server: &Server,
    password: &str,
    tls: native_tls::TlsConnector,
    calendars: &[Calendar],
    from: i64,
    to: i64,
) -> Result<Vec<Stored>> {
    let c = client(server, password, tls)?;
    let query = format!(
        "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"><c:time-range start=\"{}\" end=\"{}\"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>",
        stamp(from),
        stamp(to)
    );
    let mut out = Vec::new();
    for cal in calendars {
        let href = url::Url::parse(&cal.href)?;
        for resource in c.list(&href, &query, "calendar-data")? {
            // A repeating event's changed occurrences share its resource:
            // the series is the one without a RECURRENCE-ID.
            let mut events = ical::parse(&resource.data).events;
            let series = events
                .iter()
                .position(|e| e.recurrence_id.is_none())
                .unwrap_or(0);
            if !events.is_empty() {
                let event = events.remove(series);
                out.push(Stored {
                    event,
                    overrides: events,
                    calendar: cal.href.clone(),
                    href: resource.href.to_string(),
                    etag: resource.etag.clone(),
                });
            }
        }
    }
    out.sort_by_key(|s| s.event.start);
    Ok(out)
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
