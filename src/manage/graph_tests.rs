// An Outlook account through Microsoft Graph, against the loopback fake
// (`sync/graph/tests.rs`): the production backend end to end, as for an
// organization that turned IMAP off.
use super::*;
use crate::sync::graph::tests::{Fake, serve};

pub(super) fn setup(port: u16) -> Result<(Backend, Account, std::path::PathBuf)> {
    let path = std::env::temp_dir().join(format!("picacho-graph-{}", uuid::Uuid::new_v4()));
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret {
        access_token: "token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        ..Default::default()
    });
    let mut a = Account::new(Provider::Outlook);
    a.api = true;
    a.client_id = "client".into();
    a.email = "qa@example.com".into();
    a.name = "QA".into();
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = port;
    a.validate()?;
    b.store.save_account(&a)?;
    Ok((b, a, path))
}

#[test]
fn graph_accounts_read_change_and_send_without_imap() -> Result<()> {
    let mut fake = Fake::new();
    let projects = fake.folder("Projects", "f-inbox");
    let budget = fake.add("Budget", "inbox", 5);
    let agenda = fake.add("Agenda", "inbox", 10);
    let filed = fake.add("Plan", &projects, 20);
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    assert!(a.graph() && !a.imap() && a.api_sending());

    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    assert_eq!(inbox.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Budget", "Agenda"]);
    assert_eq!((inbox[0].folder, inbox[0].unread, inbox[0].sender.as_str()), (Folder::Inbox, true, "Bo Berg"));
    assert!(inbox[0].size > 0);
    // Standard and hidden folders aren't listed as the account's own.
    assert_eq!(b.store.folders(&a.id).iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Inbox/Projects"]);
    let in_projects = b.sync(&a, Mailbox::Remote("Inbox/Projects".into()), 50)?;
    assert_eq!(in_projects[0].id, graph::row_id(&a, &filed));

    // Read, flagged, categorized, marked answered, moved: kept by id.
    b.change_many(&a, &inbox[..1], Change::Read(true))?;
    b.change_many(&a, &inbox[..1], Change::Star(true))?;
    let now = b.store.message(&inbox[0].id)?.unwrap();
    b.change_many(&a, &[now], Change::Label("Finance".into(), true))?;
    let now = b.store.message(&inbox[0].id)?.unwrap();
    b.change_many(&a, &[now], Change::Mark(Mark::Answered))?;
    {
        let f = fake.lock().unwrap();
        let m = f.message(&budget).unwrap();
        assert!(m.read && m.flagged);
        assert_eq!(m.categories, ["Finance"]);
        assert_eq!(m.verb, Some(102));
    }
    let now = b.store.message(&inbox[0].id)?.unwrap();
    b.change_many(&a, &[now], Change::MoveTo("Inbox/Projects".into()))?;
    assert_eq!(fake.lock().unwrap().message(&budget).unwrap().folder, projects);
    let moved = b.store.message(&graph::row_id(&a, &budget))?.unwrap();
    assert_eq!((moved.folder, moved.remote_folder.as_str()), (Folder::Other, "Inbox/Projects"));

    // Changed and deleted elsewhere: the next sync says so.
    {
        let mut f = fake.lock().unwrap();
        f.messages.iter_mut().find(|m| m.id == agenda).unwrap().read = true;
        f.messages.retain(|m| m.id != filed);
    }
    b.sync(&a, Folder::Inbox, 50)?;
    assert!(!b.store.message(&graph::row_id(&a, &agenda))?.unwrap().unread);
    b.sync(&a, Mailbox::Remote("Inbox/Projects".into()), 50)?;
    assert!(b.store.message(&graph::row_id(&a, &filed))?.is_none());

    // Opened: its MIME source.
    let row = b.store.message(&graph::row_id(&a, &agenda))?.unwrap();
    assert_eq!(b.read(&a, &row)?.body.as_deref().map(str::trim), Some("About Agenda."));

    // Sent through Microsoft, Bcc named for it.
    let d = Draft {
        id: "d1".into(),
        account: a.id.clone(),
        to: "ana@example.org".into(),
        bcc: "cy@example.net".into(),
        subject: "Hi".into(),
        body: "Hello".into(),
        ..Default::default()
    };
    b.send(&a, &d)?;
    let sent = String::from_utf8_lossy(&fake.lock().unwrap().sent[0]).into_owned();
    assert!(sent.contains("Bcc: cy@example.net") && sent.contains("Subject: Hi"), "{sent}");

    // Drafts: uploaded, then gone there when discarded here.
    b.store.save_draft(&Draft {
        id: "d2".into(),
        account: a.id.clone(),
        to: "ana@example.org".into(),
        subject: "Later".into(),
        body: "Not yet".into(),
        ..Default::default()
    })?;
    b.sync_drafts(&a)?;
    assert_eq!(fake.lock().unwrap().messages.iter().filter(|m| m.draft).count(), 1);
    b.discard_draft("d2")?;
    b.sync_drafts(&a)?;
    assert_eq!(fake.lock().unwrap().messages.iter().filter(|m| m.draft).count(), 0);

    // Folders made, renamed (rows follow) and deleted.
    b.create_remote_folder(&a, Some("Inbox/Projects"), "2026")?;
    assert!(b.store.folders(&a.id).iter().any(|f| f.name == "Inbox/Projects/2026"));
    let change = b.rename_remote_folder(&a, "Inbox/Projects", "Work")?;
    assert!(change.moved.contains(&("Inbox/Projects".into(), "Inbox/Work".into())));
    assert_eq!(b.store.message(&graph::row_id(&a, &budget))?.unwrap().remote_folder, "Inbox/Work");

    // Search, and everything read in a folder.
    assert_eq!(b.search_server(&a, &Folder::Inbox.into(), "agenda", 10)?.total, 1);
    fake.lock().unwrap().add("Unread one", "inbox", 1);
    b.mark_all_read(&a, &Folder::Inbox.into())?;
    assert!(fake.lock().unwrap().messages.iter().filter(|m| m.folder == "f-inbox").all(|m| m.read));

    // Inbox rules and automatic replies; a rule made in Outlook with two
    // tests is kept as it is.
    use crate::sync::sieve::{Act, Field, Filters, Rule, Vacation};
    fake.lock().unwrap().rules.push(serde_json::json!({
        "id": "outlook-1", "displayName": "Mine", "sequence": 1, "isEnabled": true,
        "conditions": {"subjectContains": ["a"], "bodyContains": ["b"]}, "actions": {"markAsRead": true},
    }));
    let wanted = Filters {
        rules: vec![Rule { field: Field::Subject, value: "invoice".into(), act: Act::Folder("Inbox/Work".into()), stop: true, ..Default::default() }],
        vacation: Vacation { on: true, body: "Back Monday".into(), days: 1, ..Default::default() },
        include: None,
    };
    let saved = b.save_filters(&a, &wanted)?;
    assert_eq!(saved.filters.rules, wanted.rules);
    assert_eq!(saved.kept_elsewhere, 1);
    assert_eq!(fake.lock().unwrap().rules.len(), 2);
    assert_eq!(fake.lock().unwrap().replies["status"], "alwaysEnabled");
    let loaded = b.filters(&a)?;
    assert!(loaded.filters.vacation.on && loaded.filters.vacation.body == "Back Monday");

    // Deleted for good.
    let row = b.store.message(&graph::row_id(&a, &agenda))?.unwrap();
    b.delete_forever(&a, &[row])?;
    assert!(fake.lock().unwrap().message(&agenda).is_none());
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

#[test]
fn exchange_calendars_contacts_and_directory_through_graph() -> Result<()> {
    use serde_json::json;
    let mut fake = Fake::new();
    fake.calendars.push(json!({"id": "cal1", "name": "Calendar", "canEdit": true, "hexColor": "#0078d4", "isDefaultCalendar": true}));
    fake.events.push(json!({
        "id": "ev1", "iCalUId": "uid-standup", "calendar": "cal1", "subject": "Standup",
        "start": {"dateTime": "2026-12-01T09:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-12-01T09:15:00.0000000", "timeZone": "UTC"},
        "isAllDay": false, "type": "occurrence", "seriesMasterId": "series1",
        "organizer": {"emailAddress": {"address": "lead@example.com", "name": "Lead"}},
        "attendees": [{"emailAddress": {"address": "qa@example.com"}, "status": {"response": "accepted"}}],
        "isReminderOn": true, "reminderMinutesBeforeStart": 10,
    }));
    fake.contacts.push(json!({"id": "ct1", "displayName": "Ana Lima", "emailAddresses": [{"address": "ana@example.org"}], "mobilePhone": "+1 555 0100"}));
    fake.people.push(json!({"id": "u1", "displayName": "Cy Park", "scoredEmailAddresses": [{"address": "cy@example.com"}], "jobTitle": "Engineer"}));
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let cancel = Arc::new(AtomicBool::new(false));

    // Calendars: Exchange's, the default one chosen; Exchange schedules.
    assert_eq!(b.use_mail_account_for_calendar(&a, cancel.clone())?, 1);
    assert_eq!(b.calendar_mail_account().as_deref(), Some(a.id.as_str()));
    assert_eq!(b.default_calendar().unwrap().href, "graph:cal1");
    assert!(b.server_schedules());
    let standup = b.stored_event("uid-standup").unwrap();
    assert_eq!((standup.event.summary.as_str(), standup.event.alarms.as_slice()), ("Standup", &[600][..]));
    assert_eq!(standup.event.attendees[0].status, Some(crate::sync::ical::PartStat::Accepted));
    // A new event that repeats goes with Exchange's repeat rule.
    let event = crate::sync::ical::Event {
        uid: "new-1".into(),
        summary: "Planning".into(),
        start: 1_796_000_000,
        end: 1_796_003_600,
        recurrence: "FREQ=WEEKLY;COUNT=4".into(),
        ..Default::default()
    };
    let saved = b.save_event(event, None)?;
    assert!(saved.href.starts_with("graph:ev"), "{}", saved.href);
    {
        let f = fake.lock().unwrap();
        let made = f.events.iter().find(|e| e["subject"] == "Planning").unwrap();
        assert_eq!(made["recurrence"]["range"]["numberOfOccurrences"], 4);
    }
    // An occurrence deleted: that one, at Exchange.
    b.delete_event("uid-standup")?;
    assert!(fake.lock().unwrap().events.iter().all(|e| e["id"] != "ev1"));
    // When people are busy, from Exchange.
    let busy = b.free_busy(&a.email, &["bo@example.com".into()], 1_796_000_000, 1_796_086_400)?;
    assert_eq!(busy[0].busy.len(), 1);

    // Contacts: Exchange's; saved and deleted there.
    assert_eq!(b.use_mail_account_for_contacts(&a, cancel)?, 1);
    assert!(b.contacts().iter().any(|c| c.name == "Ana Lima" && c.href == "graph:ct1"));
    let saved = b.save_contact(crate::sync::carddav::Card {
        name: "Bo Berg".into(),
        emails: vec!["bo@example.se".into()],
        ..Default::default()
    })?;
    assert!(saved.href.starts_with("graph:ct"));
    assert_eq!(fake.lock().unwrap().contacts.len(), 2);
    b.delete_contact(&saved.uid)?;
    assert_eq!(fake.lock().unwrap().contacts.len(), 1);
    // The organization's directory: found, offered, not written anywhere.
    assert_eq!(b.search_server_contacts("cy")?, 1);
    let found = b.contacts().into_iter().find(|c| c.name == "Cy Park").unwrap();
    assert_eq!(found.emails, ["cy@example.com"]);
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// A weekly meeting at Exchange is one series here: its rule, an
/// occurrence changed on its own, one cancelled; changing or deleting an
/// occurrence here changes just that one at Exchange.
#[test]
fn exchange_series_keep_their_rule_changes_and_cancellations() -> Result<()> {
    use serde_json::json;
    let day = 86_400;
    let now = chrono::Utc::now().timestamp();
    // The coming Monday, 09:00 UTC, and the three after.
    let midnight = now - now.rem_euclid(day);
    let weekday = ((midnight / day) + 3).rem_euclid(7); // 0 = Monday
    let first = midnight + ((7 - weekday) % 7) * day + 9 * 3600;
    let at = |n: i64| first + n * 7 * day;
    let when = |t: i64| json!({"dateTime": crate::sync::graph::graph_time(t).trim_end_matches('Z'), "timeZone": "UTC"});
    let mut fake = Fake::new();
    fake.calendars.push(json!({"id": "cal1", "name": "Calendar", "canEdit": true, "isDefaultCalendar": true}));
    fake.events.push(json!({
        "id": "series1", "iCalUId": "uid-weekly", "calendar": "cal1", "type": "seriesMaster", "subject": "Weekly",
        "start": when(at(0)), "end": when(at(0) + 1800), "originalStartTimeZone": "UTC",
        "recurrence": {"pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday"]},
                       "range": {"type": "numbered", "startDate": "2026-01-01", "numberOfOccurrences": 4}},
    }));
    let instance = |id: &str, n: i64, kind: &str, subject: &str| json!({
        "id": id, "iCalUId": format!("i-{id}"), "calendar": "cal1", "type": kind, "seriesMasterId": "series1",
        "subject": subject, "start": when(at(n)), "end": when(at(n) + 1800),
        "originalStart": crate::sync::graph::graph_time(at(n)),
    });
    fake.events.push(instance("o0", 0, "occurrence", "Weekly"));
    fake.events.push(instance("o1", 1, "exception", "Weekly (moved room)"));
    // The third was cancelled: Exchange no longer lists it.
    fake.events.push(instance("o3", 3, "occurrence", "Weekly"));
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    b.connect_graph_calendar(&a)?;
    let series = b.stored_event("uid-weekly").expect("one series");
    assert_eq!(series.href, "graph:series1");
    assert_eq!(series.event.recurrence, "FREQ=WEEKLY;BYDAY=MO;COUNT=4");
    assert_eq!(series.event.exdates, [at(2)]);
    assert_eq!(series.overrides.len(), 1);
    assert_eq!((series.overrides[0].recurrence_id, series.overrides[0].summary.as_str()), (Some(at(1)), "Weekly (moved room)"));
    let shown: Vec<String> = b.agenda(at(0) - day, at(3) + day).into_iter().map(|e| e.summary).collect();
    assert_eq!(shown, ["Weekly", "Weekly (moved room)", "Weekly"]);
    assert_eq!(b.calendar_events().len(), 1, "not one event per occurrence");

    // One occurrence changed here: that instance at Exchange.
    let changed = crate::sync::ical::Event { summary: "Weekly (demo)".into(), start: at(0), end: at(0) + 1800, ..Default::default() };
    b.save_occurrence("uid-weekly", at(0), changed)?;
    assert_eq!(fake.lock().unwrap().events.iter().find(|e| e["id"] == "o0").unwrap()["subject"], "Weekly (demo)");
    // One deleted here: that instance goes; the series stays, unchanged.
    b.delete_occurrence("uid-weekly", at(3))?;
    {
        let f = fake.lock().unwrap();
        assert!(f.events.iter().all(|e| e["id"] != "o3"));
        let master = f.events.iter().find(|e| e["id"] == "series1").unwrap();
        assert_eq!(master["subject"], "Weekly");
    }
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Imported mail and copies of mail sent elsewhere are messages at
/// Microsoft, not drafts: made from their parts, files and all.
#[test]
fn imported_mail_arrives_as_mail_not_drafts() -> Result<()> {
    let (server, fake) = serve(Fake::new());
    let (b, a, path) = setup(server.port)?;
    let dir = std::env::temp_dir().join(format!("picacho-graph-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let eml = dir.join("old.eml");
    std::fs::write(&eml, b"From: Ana <ana@example.org>\r\nTo: qa@example.com\r\nSubject: From the archive\r\nDate: Mon, 02 Mar 2026 10:00:00 +0000\r\nMessage-ID: <old-1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nKept for years.\r\n--b\r\nContent-Type: application/pdf; name=\"a.pdf\"\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERg==\r\n--b--\r\n")?;
    let moved = b.import_messages(&a, &Folder::Archive.into(), &[eml], &AtomicBool::new(false))?;
    assert_eq!(moved.done, 1);
    {
        let f = fake.lock().unwrap();
        let m = f.messages.iter().find(|m| m.subject == "From the archive").unwrap();
        assert!(!m.draft, "not a draft");
        assert_eq!(m.folder, "f-archive");
        assert!(String::from_utf8_lossy(&m.raw).contains("Kept for years."));
        assert_eq!(f.attachments, [(m.id.clone(), 1)]);
    }
    b.file_sent_copy_graph(&a, b"From: qa@example.com\r\nTo: bo@example.se\r\nSubject: Sent elsewhere\r\n\r\nHi\r\n")?;
    {
        let f = fake.lock().unwrap();
        let m = f.messages.iter().find(|m| m.subject == "Sent elsewhere").unwrap();
        assert!(!m.draft && m.read && m.folder == "f-sent");
    }
    drop(b);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Scheduled mail is handed to Exchange, which holds it (deferred
/// delivery); mail too large for one request goes as a draft whose files
/// upload in pieces.
#[test]
fn exchange_holds_scheduled_mail_and_takes_large_mail_in_pieces() -> Result<()> {
    let (server, fake) = serve(Fake::new());
    let (b, mut a, path) = setup(server.port)?;
    a.server_send_later = true;
    b.store.save_account(&a)?;
    let at = chrono::Utc::now().timestamp() + 3 * 3600;
    let d = Draft {
        id: "later".into(),
        account: a.id.clone(),
        to: "bo@example.se".into(),
        subject: "In the morning".into(),
        body: "Good morning".into(),
        send_at: Some(at),
        ..Default::default()
    };
    b.store.save_draft(&d)?;
    assert!(b.hand_to_server(&a, "later")?);
    {
        let f = fake.lock().unwrap();
        let (raw, deferred) = &f.sent_drafts[0];
        assert!(String::from_utf8_lossy(raw).contains("Subject: In the morning"));
        assert_eq!(deferred.as_deref(), Some(crate::sync::graph::graph_time(at).as_str()));
    }
    // Five megabytes of attachment: in pieces.
    let dir = std::env::temp_dir().join(format!("picacho-graph-large-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("big.bin");
    std::fs::write(&file, vec![7u8; 5 * 1024 * 1024])?;
    let big = Draft {
        id: "big".into(),
        account: a.id.clone(),
        to: "bo@example.se".into(),
        subject: "Big".into(),
        body: "Attached".into(),
        attachments: vec![file],
        ..Default::default()
    };
    b.send(&a, &big)?;
    {
        let f = fake.lock().unwrap();
        let upload = f.uploads.iter().find(|u| u.1 == "big.bin").unwrap();
        assert_eq!(upload.2.len(), 5 * 1024 * 1024);
        let (raw, deferred) = f.sent_drafts.last().unwrap();
        let raw = String::from_utf8_lossy(raw);
        assert!(raw.contains("Subject: Big") && raw.contains("[attached big.bin]") && deferred.is_none());
    }
    drop(b);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}
