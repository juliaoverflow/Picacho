// Several calendar and address-book sources side by side (`pim_sources`):
// a CalDAV server, a Google account (against a loopback Google, as
// `sync/google/tests.rs` describes it) and a subscribed feed, read
// together and each changed where it is.
use super::*;
use crate::sync::caldav::{self, Stored};
use crate::sync::carddav::Card;
use crate::sync::ical::Event;
use crate::sync::rest::test_server::{self, json as answer};
use serde_json::{Value, json};
use std::sync::Mutex as M;

const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Holidays\r\nBEGIN:VEVENT\r\nUID:midsummer\r\nDTSTART;VALUE=DATE:20270625\r\nDTEND;VALUE=DATE:20270626\r\nSUMMARY:Midsummer Eve\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// A Google account whose APIs are the loopback server at `port`.
fn gmail(b: &Backend, port: u16) -> Result<Account> {
    let mut a = Account::new(Provider::Gmail);
    a.email = "ana@example.org".into();
    a.oauth = true;
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = port;
    b.store.save_account(&a)?;
    Ok(a)
}

fn backend() -> Result<(Backend, std::path::PathBuf)> {
    let path = std::env::temp_dir().join(format!("picacho-pim-{}", uuid::Uuid::new_v4()));
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret {
        password: "secret".into(),
        access_token: "token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        ..Default::default()
    });
    Ok((b, path))
}

/// A Google Calendar keeping events as they're inserted, changed and deleted.
fn google_calendar(events: Arc<M<Vec<Value>>>) -> test_server::Server {
    test_server::start(move |r| {
        let path = r.path.split('?').next().unwrap_or_default().to_owned();
        let mut kept = events.lock().unwrap();
        match (r.method.as_str(), path.as_str()) {
            ("GET", "/calendar/v3/users/me/calendarList") => answer(200, json!({"items": [
                {"id": "ana@example.org", "summary": "Ana", "accessRole": "owner", "primary": true},
            ]})),
            ("GET", "/calendar/v3/calendars/ana%40example.org/events") => answer(200, json!({"items": *kept})),
            ("POST", "/calendar/v3/calendars/ana%40example.org/events") => {
                let mut e = r.json();
                e["id"] = format!("n{}", kept.len() + 1).into();
                e["etag"] = "\"1\"".into();
                kept.push(e.clone());
                answer(200, e)
            }
            ("PATCH", p) if p.starts_with("/calendar/v3/calendars/ana%40example.org/events/") => {
                let id = p.rsplit('/').next().unwrap_or_default();
                let Some(e) = kept.iter_mut().find(|e| e["id"] == id) else {
                    return answer(404, json!({"error": {"message": "gone"}}));
                };
                for (k, v) in r.json().as_object().cloned().unwrap_or_default() {
                    e[k] = v;
                }
                answer(200, e.clone())
            }
            ("DELETE", p) if p.starts_with("/calendar/v3/calendars/ana%40example.org/events/") => {
                let id = p.rsplit('/').next().unwrap_or_default().to_owned();
                kept.retain(|e| e["id"] != id.as_str());
                (204, "application/json".into(), Vec::new())
            }
            _ => answer(404, json!({"error": {"message": "no such thing"}})),
        }
    })
}

/// A CalDAV server's calendar, a Google account's and a holiday feed, read
/// together: new events go where chosen, an event moved to another
/// account's calendar goes there and leaves the one it was in, the feed
/// can't be changed, and a source removed takes its events with it.
#[test]
fn calendars_of_several_sources_are_read_together_and_changed_where_they_are() -> Result<()> {
    let (b, path) = backend()?;
    // The first source: a CalDAV server, set up before (as it always was).
    let (dav_port, dav) = crate::sync::dav::test_server(2, |method, _, _| match method {
        "PUT" => ("201 Created", String::new()),
        _ => ("404 Not Found", String::new()),
    });
    let home = format!("http://127.0.0.1:{dav_port}/cal/");
    b.store.set_preference(
        "calendar_server",
        &serde_json::to_string(&caldav::Server { url: format!("http://127.0.0.1:{dav_port}"), user: "ana".into(), ..Default::default() })?,
    )?;
    b.store.set_preference(
        "calendars",
        &serde_json::to_string(&[caldav::Calendar { href: home.clone(), name: "Home".into(), writable: true, ..Default::default() }])?,
    )?;
    let lunch = Stored {
        event: Event { uid: "lunch".into(), summary: "Lunch".into(), start: 1_800_000_000, end: 1_800_003_600, utc: true, ..Default::default() },
        calendar: home.clone(),
        href: format!("{home}lunch.ics"),
        etag: "\"1\"".into(),
        ..Default::default()
    };
    b.store.set_preference("calendar_events", &serde_json::to_string(std::slice::from_ref(&lunch))?)?;

    // A Google account's calendars beside it.
    let google_events = Arc::new(M::new(vec![json!({
        "id": "g1", "iCalUID": "standup@google.com", "summary": "Standup", "etag": "\"1\"",
        "start": {"dateTime": "2027-01-04T09:00:00Z"}, "end": {"dateTime": "2027-01-04T09:15:00Z"},
    })]));
    let google = google_calendar(google_events.clone());
    let a = gmail(&b, google.port)?;
    assert_eq!(b.add_calendar_source(|| b.connect_google_calendar(&a))?, 1);
    // And a subscribed feed.
    let feed = test_server::start_raw(|_| (200, "text/calendar".into(), FEED.as_bytes().to_vec(), vec![("ETag".into(), "\"f1\"".into())]));
    assert_eq!(b.subscribe_calendar_feed(&format!("{}/holidays.ics", feed.base), "", 0)?, 1);

    let sources = b.calendar_sources();
    assert_eq!(sources.len(), 2);
    let google_id = sources.iter().find(|(_, s, _)| !s.google_account.is_empty()).unwrap().0.clone();
    let feed_id = sources.iter().find(|(_, s, _)| s.feed).unwrap().0.clone();
    assert_eq!(b.calendar_source_label(&google_id), "ana@example.org");
    assert_eq!(b.calendar_source_label(&feed_id), "Holidays");
    // Together: every source's calendars and events.
    let names: Vec<String> = b.calendars().into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Home", "Ana", "Holidays"]);
    let uids: Vec<String> = b.calendar_events().into_iter().map(|s| s.event.uid).collect();
    assert_eq!(uids, ["standup@google.com", "lunch", "midsummer"], "soonest first");
    assert_eq!(b.primary_calendar_server().unwrap().user, "ana");
    // New events go to the first source's calendar unless another is chosen.
    assert_eq!(b.default_calendar().unwrap().href, home);
    b.set_default_calendar("google:ana@example.org")?;
    assert_eq!(b.default_calendar().unwrap().href, "google:ana@example.org");
    let party = b.save_event(
        Event { summary: "Party".into(), start: 1_800_100_000, end: 1_800_103_600, utc: true, ..Default::default() },
        None,
    )?;
    assert_eq!(party.calendar, "google:ana@example.org");
    assert!(google_events.lock().unwrap().iter().any(|e| e["summary"] == "Party"));
    assert!(b.in_calendar_source(&google_id, || b.stored_event(&party.event.uid)).is_some());
    // Moved to the CalDAV calendar: made there, gone from Google.
    let moved = b.save_event(party.event.clone(), Some(&home))?;
    assert_eq!(moved.calendar, home);
    assert!(!google_events.lock().unwrap().iter().any(|e| e["summary"] == "Party"));
    assert!(b.in_calendar_source("", || b.stored_event(&party.event.uid)).is_some());
    assert!(b.in_calendar_source(&google_id, || b.stored_event(&party.event.uid)).is_none());
    // The first source's own event, changed where it is.
    b.save_event(Event { summary: "Long lunch".into(), ..lunch.event.clone() }, None)?;
    let seen: Vec<String> = dav.join().unwrap().iter().map(|(m, p, _)| format!("{m} {p}")).collect();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].starts_with("PUT /cal/") && seen[1] == "PUT /cal/lunch.ics", "{seen:?}");
    assert_eq!(b.stored_event("lunch").unwrap().event.summary, "Long lunch");
    // A feed's events are its publisher's.
    let holiday = b.stored_event("midsummer").unwrap();
    assert!(b.save_event(Event { summary: "Mine now".into(), ..holiday.event.clone() }, None).is_err());
    assert!(b.delete_event("midsummer").is_err());
    assert_eq!(b.stored_event("midsummer").unwrap().event.summary, "Midsummer Eve");
    // Read just now: not read again until its interval has passed.
    assert_eq!(b.sync_due_feeds(), 0);
    // Read again when asked.
    assert_eq!(b.in_calendar_source(&feed_id, || b.sync_calendar())?, 1);
    // A source removed takes its calendars and events with it.
    b.remove_calendar_source(&feed_id)?;
    assert!(b.stored_event("midsummer").is_none());
    assert_eq!(b.calendar_sources().len(), 1);
    assert!(b.store.preference(&format!("calendar_events@{feed_id}")).is_none());
    // The first disconnected, the other stays.
    b.disconnect_calendar()?;
    assert!(b.primary_calendar_server().is_none());
    assert_eq!(b.calendar_server().unwrap().google_account, a.id);
    assert_eq!(b.default_calendar().unwrap().href, "google:ana@example.org");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Contacts kept on this computer, and a Google account's beside them:
/// shown together, each saved and deleted where it is.
#[test]
fn contacts_of_several_sources_are_read_together_and_changed_where_they_are() -> Result<()> {
    let (b, path) = backend()?;
    let people = Arc::new(M::new(vec![json!({
        "resourceName": "people/c1", "etag": "e1",
        "names": [{"displayName": "Bo Berg"}], "emailAddresses": [{"value": "bo@example.se"}],
    })]));
    let kept = people.clone();
    let google = test_server::start(move |r| {
        let path = r.path.split('?').next().unwrap_or_default().to_owned();
        let mut kept = kept.lock().unwrap();
        match (r.method.as_str(), path.as_str()) {
            ("GET", "/v1/people/me/connections") => answer(200, json!({"connections": *kept})),
            ("POST", "/v1/people:createContact") => {
                let mut p = r.json();
                p["resourceName"] = format!("people/c{}", kept.len() + 1).into();
                p["names"][0]["displayName"] = p["names"][0]["unstructuredName"].clone();
                kept.push(p.clone());
                answer(200, p)
            }
            ("DELETE", p) if p.ends_with(":deleteContact") => {
                let resource = p.trim_start_matches("/v1/").trim_end_matches(":deleteContact").to_owned();
                kept.retain(|x| x["resourceName"] != resource.as_str());
                answer(200, json!({}))
            }
            _ => answer(404, json!({"error": {"message": "no such thing"}})),
        }
    });
    // One kept on this computer.
    let cy = b.save_contact(Card { name: "Cy".into(), emails: vec!["cy@x.net".into()], ..Default::default() })?;
    assert!(cy.href.is_empty());
    let a = gmail(&b, google.port)?;
    assert_eq!(b.add_contacts_source(|| b.connect_google_contacts(&a))?, 1);
    let (google_id, server, _) = b.contacts_sources().remove(0);
    assert!(server.google_account == a.id);
    let names: Vec<String> = b.contacts().into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Cy", "Bo Berg"]);
    // New contacts go to the account's server, the only one set up.
    assert_eq!(b.default_contacts_source(), google_id);
    let di = b.save_contact(Card { name: "Di".into(), emails: vec!["di@example.org".into()], ..Default::default() })?;
    assert_eq!(di.href, "google:people/c2");
    assert_eq!(people.lock().unwrap().len(), 2);
    // Cy, edited, stays on this computer.
    let cy = b.save_contact(Card { phones: vec!["555".into()], ..cy })?;
    assert!(cy.href.is_empty());
    assert_eq!(b.in_contacts_source("", || b.contacts()).len(), 1);
    assert_eq!(b.contacts().len(), 3);
    // Bo, deleted: from Google.
    let bo = b.contacts().into_iter().find(|c| c.name == "Bo Berg").unwrap();
    b.delete_contact(&bo.uid)?;
    assert_eq!(people.lock().unwrap().len(), 1);
    // Removed, the account's contacts go; this computer's stay.
    b.remove_contacts_source(&google_id)?;
    let names: Vec<String> = b.contacts().into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["Cy"]);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
