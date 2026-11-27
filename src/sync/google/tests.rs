//! Google's calendar and contacts JSON, read and written as Google's
//! documentation describes it, and the session against a loopback server.
use super::*;
use crate::sync::rest::test_server::{json as answer, start};

fn tls() -> native_tls::TlsConnector {
    native_tls::TlsConnector::new().unwrap()
}

fn cal(id: &str) -> GoogleCalendar {
    GoogleCalendar { id: id.into(), name: id.into(), writable: true, default_alarms: vec![600], ..Default::default() }
}

/// A timed event keeps its zone, so repeats keep their wall time; an
/// all-day one is a date; the calendar's reminders are its own where it
/// keeps them.
#[test]
fn events_are_read_in_their_own_zone() {
    let timed = json!({
        "id": "e1", "iCalUID": "lunch@google.com", "summary": "Lunch; with Bo",
        "start": {"dateTime": "2026-11-24T12:00:00+01:00", "timeZone": "Europe/Stockholm"},
        "end": {"dateTime": "2026-11-24T13:00:00+01:00", "timeZone": "Europe/Stockholm"},
        "recurrence": ["RRULE:FREQ=WEEKLY;COUNT=3"],
        "organizer": {"email": "ana@example.org", "displayName": "Ana"},
        "attendees": [{"email": "qa@example.com", "responseStatus": "tentative", "self": true}],
        "reminders": {"useDefault": true},
        "sequence": 2,
    });
    let e = event_of(&timed, &[600]).unwrap();
    assert_eq!(e.uid, "lunch@google.com");
    assert_eq!(e.summary, "Lunch; with Bo");
    assert_eq!(e.tzid, "Europe/Stockholm");
    assert_eq!(e.start, 1_795_518_000);
    assert_eq!(e.end - e.start, 3600);
    assert_eq!(e.recurrence, "FREQ=WEEKLY;COUNT=3");
    assert_eq!(e.organizer.as_ref().unwrap().email, "ana@example.org");
    assert_eq!(e.attendees[0].status, Some(PartStat::Tentative));
    assert_eq!(e.alarms, [600]);
    assert_eq!(e.sequence, 2);
    let day = json!({"id": "e2", "summary": "Holiday", "start": {"date": "2026-12-24"}, "end": {"date": "2026-12-25"},
        "reminders": {"useDefault": false, "overrides": [{"method": "popup", "minutes": 30}]}});
    let d = event_of(&day, &[600]).unwrap();
    assert!(d.all_day);
    assert_eq!(d.uid, "e2", "without an iCalUID, its id");
    assert_eq!(d.alarms, [1800]);
    // Written back: the wall time in its zone, a date for all-day ones.
    let v = event_json(&e, true);
    assert_eq!(v["start"], json!({"dateTime": "2026-11-24T12:00:00", "timeZone": "Europe/Stockholm"}));
    assert_eq!(v["recurrence"], json!(["RRULE:FREQ=WEEKLY;COUNT=3"]));
    assert_eq!(v["attendees"][0]["responseStatus"], "tentative");
    assert_eq!(v["reminders"]["overrides"][0]["minutes"], 10);
    let v = event_json(&d, false);
    assert_eq!(v["start"], json!({"date": "2026-12-24"}));
    assert_eq!(v["end"], json!({"date": "2026-12-25"}));
    assert!(v.get("recurrence").is_none());
}

/// A series comes with its changed occurrences beside it: those join the
/// series, the cancelled ones become times it leaves out.
#[test]
fn changed_and_cancelled_occurrences_join_their_series() {
    let items = vec![
        json!({"id": "s", "iCalUID": "standup", "summary": "Standup", "etag": "\"1\"",
            "start": {"dateTime": "2026-11-02T09:00:00Z", "timeZone": "UTC"},
            "end": {"dateTime": "2026-11-02T09:15:00Z", "timeZone": "UTC"},
            "recurrence": ["RRULE:FREQ=DAILY;COUNT=5"]}),
        json!({"id": "s_20261103T090000Z", "recurringEventId": "s", "summary": "Standup (late)",
            "originalStartTime": {"dateTime": "2026-11-03T09:00:00Z", "timeZone": "UTC"},
            "start": {"dateTime": "2026-11-03T10:00:00Z"}, "end": {"dateTime": "2026-11-03T10:15:00Z"}}),
        json!({"id": "s_20261104T090000Z", "recurringEventId": "s", "status": "cancelled",
            "originalStartTime": {"dateTime": "2026-11-04T09:00:00Z", "timeZone": "UTC"}}),
        json!({"id": "gone", "status": "cancelled"}),
    ];
    let read = series_of(&items, &cal("primary"));
    assert_eq!(read.len(), 1);
    let s = &read[0];
    assert_eq!((s.id.as_str(), s.etag.as_str(), s.calendar.as_str()), ("s", "\"1\"", "primary"));
    assert_eq!(s.overrides.len(), 1);
    assert_eq!(occurrence_id(&s.overrides[0]), Some("s_20261103T090000Z"));
    assert_eq!(s.overrides[0].uid, "standup");
    let all = ical::occurrences(&s.event, &s.overrides, 0, 2_000_000_000);
    assert_eq!(all.len(), 4, "five, less the cancelled one");
    assert_eq!(all[1].summary, "Standup (late)");
    // Written back, the series names the time it leaves out.
    let v = event_json(&s.event, true);
    assert_eq!(v["recurrence"], json!(["RRULE:FREQ=DAILY;COUNT=5", "EXDATE;TZID=UTC:20261104T090000"]));
}

/// People: read as cards, written as Google takes them.
#[test]
fn people_are_cards() {
    let person = json!({
        "resourceName": "people/c42", "etag": "%E1",
        "names": [{"displayName": "Ana Lind", "unstructuredName": "Ana Lind"}],
        "emailAddresses": [{"value": "ana@example.org"}, {"value": " "}],
        "phoneNumbers": [{"value": "+46 8 123"}],
        "organizations": [{"name": "Pinal", "title": "Engineer"}],
        "birthdays": [{"date": {"month": 6, "day": 24}}],
        "photos": [{"url": "https://lh3/default", "default": true}],
    });
    let card = card_of(&person);
    assert_eq!(card.uid, "people/c42");
    assert_eq!(card.href, "google:people/c42");
    assert_eq!(card.emails, ["ana@example.org"]);
    assert_eq!((card.org.as_str(), card.title.as_str()), ("Pinal", "Engineer"));
    assert_eq!(card.birthday, "--06-24");
    assert!(card.photo.is_empty(), "Google's placeholder isn't a photo");
    let back = person_json(&card);
    assert_eq!(back["etag"], "%E1");
    assert_eq!(back["names"][0]["unstructuredName"], "Ana Lind");
    assert_eq!(back["birthdays"][0]["date"], json!({"month": 6, "day": 24}));
    let full = Card { birthday: "1990-06-24".into(), ..card };
    assert_eq!(person_json(&full)["birthdays"][0]["date"], json!({"year": 1990, "month": 6, "day": 24}));
}

/// Against a server: the calendar list (its pages), events in a range,
/// saving one (Google told to tell the guests), contacts.
#[test]
fn a_session_reads_and_writes_calendars_and_contacts() {
    let server = start(|r| {
        let path = r.path.as_str();
        match (r.method.as_str(), path.split('?').next().unwrap()) {
            ("GET", "/calendar/v3/users/me/calendarList") if !path.contains("pageToken") => answer(200, json!({
                "items": [{"id": "team@group.calendar.google.com", "summary": "Team", "accessRole": "reader"}],
                "nextPageToken": "p2",
            })),
            ("GET", "/calendar/v3/users/me/calendarList") => answer(200, json!({
                "items": [
                    {"id": "ana@example.org", "summary": "Ana", "accessRole": "owner", "primary": true, "backgroundColor": "#9fe1e7"},
                    {"id": "hidden", "summary": "Hidden", "accessRole": "owner", "hidden": true},
                ],
            })),
            ("GET", "/calendar/v3/calendars/ana%40example.org/events") => {
                assert!(path.contains("singleEvents=false") && path.contains("timeMin="), "{path}");
                answer(200, json!({"items": [{"id": "e1", "iCalUID": "u1", "summary": "Lunch",
                    "start": {"dateTime": "2026-11-24T12:00:00Z"}, "end": {"dateTime": "2026-11-24T13:00:00Z"}}]}))
            }
            ("GET", _) if path.starts_with("/calendar/v3/calendars/team") => answer(200, json!({"items": []})),
            ("POST", "/calendar/v3/calendars/ana%40example.org/events") => {
                assert!(path.ends_with("sendUpdates=all"), "{path}");
                assert_eq!(r.json()["iCalUID"], "party");
                answer(200, json!({"id": "new1", "etag": "\"7\""}))
            }
            ("GET", "/v1/people/me/connections") => answer(200, json!({"connections": [
                {"resourceName": "people/c1", "names": [{"displayName": "Bo"}], "emailAddresses": [{"value": "bo@example.se"}]},
            ]})),
            ("POST", "/v1/people:createContact") => {
                let mut v = r.json();
                v["resourceName"] = "people/c2".into();
                v["names"][0]["displayName"] = v["names"][0]["unstructuredName"].clone();
                answer(200, v)
            }
            ("PATCH", "/v1/people/c1:updateContact") => {
                assert!(path.contains("updatePersonFields="), "{path}");
                let mut v = r.json();
                v["resourceName"] = "people/c1".into();
                answer(200, v)
            }
            ("DELETE", "/v1/people/c1:deleteContact") => answer(200, json!({})),
            _ => answer(404, json!({"error": {"code": 404, "message": "no"}})),
        }
    });
    let s = Session::open(&server.base, &server.base, "token", tls()).unwrap();
    let cals = s.calendars().unwrap();
    let names: Vec<&str> = cals.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Ana", "Team"], "the person's own first, hidden ones left out");
    assert!(cals[0].writable && !cals[1].writable);
    let events = s.events_in(&cals, 1_790_000_000, 1_800_000_000).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].id.as_str(), events[0].event.uid.as_str()), ("e1", "u1"));
    let party = Event {
        uid: "party".into(),
        summary: "Party".into(),
        start: 1_800_000_000,
        end: 1_800_003_600,
        utc: true,
        attendees: vec![ical::Attendee { email: "bo@example.se".into(), ..Default::default() }],
        ..Default::default()
    };
    assert_eq!(s.save_event("ana@example.org", None, &party, true, true).unwrap(), ("new1".into(), "\"7\"".into()));
    let cards = s.contacts().unwrap();
    assert_eq!(cards[0].emails, ["bo@example.se"]);
    let made = s.save_contact(&Card { name: "Cy".into(), emails: vec!["cy@x.net".into()], ..Default::default() }).unwrap();
    assert_eq!((made.href.as_str(), made.name.as_str()), ("google:people/c2", "Cy"));
    let edited = s.save_contact(&Card { note: "hi".into(), ..cards[0].clone() }).unwrap();
    assert_eq!(edited.uid, "people/c1");
    s.delete_contact(&cards[0]).unwrap();
}
