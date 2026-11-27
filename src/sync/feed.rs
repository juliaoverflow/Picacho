//! Subscribed calendar feeds: an iCalendar file (RFC 5545) published at
//! an address, as `webcal://` links and `.ics` addresses are (holidays,
//! sports fixtures, a team's schedule). They are read whole, over HTTPS,
//! on an interval, and never written: a feed is someone else's calendar,
//! shown beside your own.
//!
//! A feed that hasn't changed costs no download where its server says
//! (`ETag` and `If-None-Match`, `Last-Modified` and `If-Modified-Since`,
//! RFC 9110 §13).
//!
//! This only talks to the feed's server; Manage keeps what it read
//! (`manage/calendar.rs`).
use crate::sync::caldav::Stored;
use crate::sync::ical;
use anyhow::{Context, Result, ensure};
use std::time::Duration;

/// How often a feed is read again unless the person chose: every six hours.
pub const DEFAULT_MINUTES: u32 = 6 * 60;
/// The most often a feed is read again: every quarter of an hour, so a
/// feed's server isn't asked more than any calendar app would.
pub const SHORTEST_MINUTES: u32 = 15;
/// The largest feed read: years of a busy calendar fit well within it.
const MOST: u64 = 32 * 1024 * 1024;
/// The most events kept from one feed.
const MOST_EVENTS: usize = 20_000;

/// The address a feed is read from: `webcal://` and `webcals://` (Apple's
/// names for subscribing) are HTTPS; `https://` and `http://` as given.
pub fn address(url: &str) -> Result<url::Url> {
    let given = url.trim();
    let lower = given.to_ascii_lowercase();
    let rewritten = ["webcals://", "webcal://"]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map_or_else(|| given.to_owned(), |p| format!("https://{}", &given[p.len()..]));
    let parsed = url::Url::parse(&rewritten).with_context(|| t!("sync-feed-isnt-a-calendar-address", url = given))?;
    ensure!(
        matches!(parsed.scheme(), "https" | "http") && parsed.host_str().is_some_and(|h| !h.is_empty()),
        t!("sync-feed-isnt-a-calendar-address", url = given)
    );
    Ok(parsed)
}

/// What reading a feed found.
#[derive(Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The same as when last read (304 Not Modified).
    Unchanged,
    /// The feed's text, and what tells its version next time
    /// (`validators`, kept as given back to `fetch`).
    Changed { text: String, validators: String },
}

/// Reads the feed at `url`. `validators` are what the last read returned
/// (empty the first time): with them, an unchanged feed isn't sent again.
pub fn fetch(url: &str, validators: &str, tls: native_tls::TlsConnector) -> Result<Fetched> {
    use reqwest::header;
    let address = address(url)?;
    let host = address.host_str().unwrap_or_default().to_owned();
    let client = reqwest::blocking::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let (etag, modified) = validators.split_once('\n').unwrap_or((validators, ""));
    let mut request = client
        .get(address)
        .header(header::ACCEPT, "text/calendar, text/plain;q=0.5, */*;q=0.1");
    if !etag.trim().is_empty() {
        request = request.header(header::IF_NONE_MATCH, etag.trim());
    }
    if !modified.trim().is_empty() {
        request = request.header(header::IF_MODIFIED_SINCE, modified.trim());
    }
    let response = request.send().with_context(|| t!("sync-feed-couldnt-reach", host = host))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::Unchanged);
    }
    ensure!(status.is_success(), t!("sync-feed-answered", host = host, status = status.to_string()));
    let value = |name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.replace(['\r', '\n'], ""))
            .unwrap_or_default()
    };
    let validators = format!("{}\n{}", value(header::ETAG), value(header::LAST_MODIFIED));
    let bytes = {
        use std::io::Read as _;
        let mut out = Vec::new();
        response.take(MOST + 1).read_to_end(&mut out)?;
        ensure!(out.len() as u64 <= MOST, t!("sync-feed-is-too-large", host = host));
        out
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    ensure!(text.contains("BEGIN:VCALENDAR"), t!("sync-feed-isnt-a-calendar", host = host));
    Ok(Fetched::Changed { text, validators })
}

/// A feed as kept: its name and colour where it gives them, and its events.
#[derive(Debug, Default)]
pub struct Feed {
    /// `X-WR-CALNAME` (or `NAME`, RFC 7986 §5.1).
    pub name: String,
    /// `X-APPLE-CALENDAR-COLOR` (or `COLOR`, RFC 7986 §5.9), as `#RRGGBB`.
    pub color: String,
    pub events: Vec<Stored>,
}

/// Reads a feed's text as events of `calendar`: each series with its
/// changed occurrences, ignoring tasks. Events over before `since` that
/// don't repeat are left out, as the calendar only shows from then on.
pub fn read(text: &str, calendar: &str, since: i64) -> Feed {
    let parsed = ical::parse(text);
    let mut feed = Feed {
        name: calendar_property(&parsed.text, &["X-WR-CALNAME", "NAME"]),
        color: calendar_property(&parsed.text, &["X-APPLE-CALENDAR-COLOR", "COLOR"])
            .get(..7)
            .filter(|c| c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()))
            .unwrap_or_default()
            .to_owned(),
        events: Vec::new(),
    };
    // By UID, in the order first seen: the series is the one without a
    // RECURRENCE-ID, the others its changed occurrences.
    let mut order: Vec<String> = Vec::new();
    let mut by_uid: std::collections::HashMap<String, (Option<ical::Event>, Vec<ical::Event>)> = Default::default();
    for mut e in parsed.events.into_iter().filter(|e| !e.task) {
        // Kept as fields: a feed's events are never written back.
        e.source.clear();
        if e.uid.is_empty() {
            e.uid = format!("{}-{}", e.start, e.summary);
        }
        let entry = by_uid.entry(e.uid.clone()).or_insert_with(|| {
            order.push(e.uid.clone());
            (None, Vec::new())
        });
        match e.recurrence_id {
            Some(_) => entry.1.push(e),
            None => entry.0 = Some(e),
        }
    }
    for uid in order {
        let Some((series, overrides)) = by_uid.remove(&uid) else { continue };
        let (event, overrides) = match series {
            Some(s) => (s, overrides),
            // Only changed occurrences: each stands alone.
            None => {
                for o in overrides {
                    feed.events.push(stored(calendar, Default::default(), o));
                }
                continue;
            }
        };
        let repeats = !event.recurrence.is_empty() || !event.rdates.is_empty();
        if !repeats && event.end.max(event.start) < since {
            continue;
        }
        feed.events.push(stored(calendar, overrides, event));
        if feed.events.len() >= MOST_EVENTS {
            break;
        }
    }
    feed
}

fn stored(calendar: &str, overrides: Vec<ical::Event>, event: ical::Event) -> Stored {
    Stored {
        href: format!("{calendar}/{}", event.uid),
        calendar: calendar.to_owned(),
        event,
        overrides,
        etag: String::new(),
        schedule_tag: String::new(),
    }
}

/// The first of `names` given for the calendar as a whole (outside its
/// components), unescaped.
fn calendar_property(text: &str, names: &[&str]) -> String {
    let mut depth = 0usize;
    let mut found: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            depth = depth.saturating_sub(1);
            continue;
        }
        // Inside VCALENDAR only, not its events.
        if depth != 1 {
            continue;
        }
        let Some((head, value)) = line.split_once(':') else { continue };
        let name = head.split(';').next().unwrap_or_default().to_ascii_uppercase();
        if let Some(rank) = names.iter().position(|n| *n == name) {
            found.push((rank, unescape(value)));
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, v)| v.trim().to_owned()).find(|v| !v.is_empty()).unwrap_or_default()
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_addresses_are_read_over_https() {
        assert_eq!(address("webcal://example.org/h.ics").unwrap().as_str(), "https://example.org/h.ics");
        assert_eq!(address(" WEBCALS://example.org/h.ics ").unwrap().as_str(), "https://example.org/h.ics");
        assert_eq!(address("https://example.org/h.ics").unwrap().scheme(), "https");
        assert!(address("ftp://example.org/h.ics").is_err());
        assert!(address("not an address").is_err());
        assert!(address("file:///etc/passwd").is_err());
    }

    const HOLIDAYS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Holidays in Sweden\r\nX-APPLE-CALENDAR-COLOR:#FF2968FF\r\nBEGIN:VEVENT\r\nUID:midsummer\r\nDTSTART;VALUE=DATE:20270625\r\nDTEND;VALUE=DATE:20270626\r\nSUMMARY:Midsommarafton\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:old\r\nDTSTART;VALUE=DATE:19990101\r\nDTEND;VALUE=DATE:19990102\r\nSUMMARY:Long ago\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:standup\r\nDTSTART:19990104T090000Z\r\nDTEND:19990104T091500Z\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Team standup\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:standup\r\nRECURRENCE-ID:19990111T090000Z\r\nDTSTART:19990111T100000Z\r\nDTEND:19990111T101500Z\r\nSUMMARY:Team standup (late)\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:task\r\nSUMMARY:Not an event\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// A feed's name and colour are its own; its events are kept with
    /// their changed occurrences, past single ones and tasks left out.
    #[test]
    fn feeds_are_read_as_series_with_their_occurrences() {
        let feed = read(HOLIDAYS, "feed:s1", 1_700_000_000);
        assert_eq!(feed.name, "Holidays in Sweden");
        assert_eq!(feed.color, "#FF2968");
        let uids: Vec<&str> = feed.events.iter().map(|s| s.event.uid.as_str()).collect();
        assert_eq!(uids, ["midsummer", "standup"]);
        let standup = &feed.events[1];
        assert_eq!(standup.overrides.len(), 1);
        assert_eq!(standup.calendar, "feed:s1");
        assert_eq!(standup.href, "feed:s1/standup");
        assert!(standup.event.source.is_empty());
    }

    /// Read from a server: an unchanged feed isn't downloaded again.
    #[test]
    fn unchanged_feeds_arent_downloaded_again() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let fresh = !head.to_ascii_lowercase().contains("if-none-match: \"v1\"");
                let reply = if fresh {
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{HOLIDAYS}", HOLIDAYS.len())
                } else {
                    "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_owned()
                };
                stream.write_all(reply.as_bytes()).unwrap();
                asked.push(fresh);
            }
            asked
        });
        let tls = native_tls::TlsConnector::new().unwrap();
        let url = format!("http://127.0.0.1:{port}/holidays.ics");
        let Fetched::Changed { text, validators } = fetch(&url, "", tls.clone()).unwrap() else {
            panic!("the first read gets the feed");
        };
        assert!(text.contains("Midsommarafton"));
        assert_eq!(validators, "\"v1\"\n");
        assert_eq!(fetch(&url, &validators, tls).unwrap(), Fetched::Unchanged);
        assert_eq!(server.join().unwrap(), [true, false]);
    }
}
