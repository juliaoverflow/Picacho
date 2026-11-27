// An Exchange Server account through Exchange Web Services, against the
// loopback fake (`sync/ews/tests.rs`), signing in with NTLMv2.
use super::*;
use crate::sync::ews::tests::{Fake, serve};

fn setup(port: u16) -> Result<(Backend, Account, std::path::PathBuf)> {
    let path = std::env::temp_dir().join(format!("picacho-ews-{}", uuid::Uuid::new_v4()));
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret { password: "secret".into(), ..Default::default() });
    let mut a = Service::Exchange.account("qa@corp.example").unwrap();
    a.name = "QA".into();
    a.username = "CORP\\qa".into();
    a.incoming_host = "127.0.0.1".into();
    a.incoming_port = port;
    a.smtp_host = "127.0.0.1".into();
    a.validate()?;
    b.store.save_account(&a)?;
    Ok((b, a, path))
}

#[test]
fn exchange_accounts_read_change_and_send_through_ews() -> Result<()> {
    let mut fake = Fake::new("secret");
    let projects = fake.folder("Projects", "f-inbox");
    let budget = fake.add("Budget", "f-inbox", 5);
    let agenda = fake.add("Agenda", "f-inbox", 10);
    fake.add("Plan", &projects, 20);
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    assert!(a.ews() && !a.imap() && a.api_sending());

    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    assert_eq!(inbox.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Budget", "Agenda"]);
    assert_eq!((inbox[0].unread, inbox[0].sender.as_str()), (true, "Bo Berg"));
    assert_eq!(b.store.folders(&a.id).iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Inbox/Projects"]);
    assert_eq!(b.sync(&a, Mailbox::Remote("Inbox/Projects".into()), 50)?.len(), 1);

    // Read, flagged, categorized, answered.
    b.change_many(&a, &inbox[..1], Change::Read(true))?;
    b.change_many(&a, &inbox[..1], Change::Star(true))?;
    let now = b.store.message(&inbox[0].id)?.unwrap();
    b.change_many(&a, &[now], Change::Label("Finance".into(), true))?;
    let now = b.store.message(&inbox[0].id)?.unwrap();
    b.change_many(&a, &[now], Change::Mark(Mark::Answered))?;
    {
        let f = fake.lock().unwrap();
        let m = f.item(&budget).unwrap();
        assert!(m.read && m.flagged && m.verb == Some(102));
        assert_eq!(m.categories, ["Finance"]);
    }
    let now = b.store.message(&inbox[0].id)?.unwrap();
    assert!(!now.unread && now.starred && now.answered && now.labels() == ["Finance"]);

    // Moved: its row takes the new id, with its downloaded source; undo
    // finds it there.
    let before = b.store.message(&inbox[0].id)?.unwrap();
    let source = b"From: a@corp.example\r\nSubject: Budget\r\n\r\nThe numbers.\r\n".to_vec();
    b.store.save_raw(&before.id, &source)?;
    b.change_many(&a, std::slice::from_ref(&before), Change::MoveTo("Inbox/Projects".into()))?;
    assert!(b.store.message(&before.id)?.is_none());
    let moved = b.store.summaries(&a.id, Mailbox::Remote("Inbox/Projects".into()))?;
    let budget_now = moved.iter().find(|m| m.subject == "Budget").unwrap().clone();
    assert!(budget_now.starred, "kept what it was");
    assert_eq!(b.store.raw(&budget_now.id)?.as_deref(), Some(&source[..]), "its source moved with it");
    assert!(fake.lock().unwrap().items.iter().any(|i| i.subject == "Budget" && i.folder == projects));
    let undo = vec![Moved { before: before.clone(), change: Change::MoveTo("Inbox/Projects".into()) }];
    assert_eq!(b.undo_move(&a, &undo)?, 1);
    assert!(fake.lock().unwrap().items.iter().any(|i| i.subject == "Budget" && i.folder == "f-inbox"));

    // Changed and deleted elsewhere.
    {
        let mut f = fake.lock().unwrap();
        f.items.iter_mut().find(|i| i.id == agenda).unwrap().read = true;
    }
    b.sync(&a, Folder::Inbox, 50)?;
    assert!(!b.store.message(&ews::row_id(&a, &agenda))?.unwrap().unread);

    // Opened, from its MIME content.
    let row = b.store.message(&ews::row_id(&a, &agenda))?.unwrap();
    assert_eq!(b.read(&a, &row)?.body.as_deref().map(str::trim), Some("About Agenda."));

    // Sent, Bcc named for Exchange; scheduled mail held by it.
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
    let sent = String::from_utf8_lossy(&fake.lock().unwrap().sent[0].0).into_owned();
    assert!(sent.contains("Bcc: cy@example.net") && sent.contains("Subject: Hi"), "{sent}");
    let mut held = a.clone();
    held.server_send_later = true;
    let at = chrono::Utc::now().timestamp() + 7200;
    b.store.save_draft(&Draft { id: "later".into(), account: a.id.clone(), to: "ana@example.org".into(), subject: "Later".into(), body: "x".into(), send_at: Some(at), ..Default::default() })?;
    assert!(b.hand_to_server(&held, "later")?);
    assert_eq!(fake.lock().unwrap().sent[1].1.as_deref(), Some(crate::sync::ews::ews_time(at).as_str()));

    // Drafts both ways.
    b.store.save_draft(&Draft { id: "d2".into(), account: a.id.clone(), to: "ana@example.org".into(), subject: "Draft".into(), body: "Not yet".into(), ..Default::default() })?;
    b.sync_drafts(&a)?;
    assert!(fake.lock().unwrap().items.iter().any(|i| i.folder == "f-drafts" && i.subject == "Draft"));
    b.discard_draft("d2")?;
    b.sync_drafts(&a)?;
    assert!(!fake.lock().unwrap().items.iter().any(|i| i.folder == "f-drafts" && i.subject == "Draft"));

    // Search, mark all read, folders, empty Deleted Items.
    assert_eq!(b.search_server(&a, &Folder::Inbox.into(), "agenda", 10)?.total, 1);
    fake.lock().unwrap().add("Unread", "f-inbox", 1);
    b.mark_all_read(&a, &Folder::Inbox.into())?;
    assert!(fake.lock().unwrap().items.iter().filter(|i| i.folder == "f-inbox").all(|i| i.read));
    b.create_remote_folder(&a, None, "Receipts")?;
    let change = b.rename_remote_folder(&a, "Inbox/Projects", "Work")?;
    assert!(change.moved.contains(&("Inbox/Projects".into(), "Inbox/Work".into())));
    let row = b.store.message(&ews::row_id(&a, &agenda))?.unwrap();
    b.change_many(&a, &[row], Change::Move(Folder::Trash))?;
    assert!(b.empty_mailbox(&a, Folder::Trash)? >= 1);
    assert!(!fake.lock().unwrap().items.iter().any(|i| i.folder == "f-trash"));

    // Imported mail arrives as mail.
    let dir = std::env::temp_dir().join(format!("picacho-ews-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let eml = dir.join("old.eml");
    std::fs::write(&eml, b"From: x@example.org\r\nSubject: From the archive\r\n\r\nOld.\r\n")?;
    b.import_messages(&a, &Folder::Archive.into(), &[eml], &AtomicBool::new(false))?;
    assert!(fake.lock().unwrap().items.iter().any(|i| i.subject == "From the archive" && i.folder == "f-archive" && i.flags & 8 == 0));

    // Inbox rules and Out of Office.
    use crate::sync::sieve::{Act, Field, Filters, Rule, Vacation};
    let wanted = Filters {
        rules: vec![Rule { field: Field::From, value: "shop".into(), act: Act::Folder("Receipts".into()), stop: true, ..Default::default() }],
        vacation: Vacation { on: true, body: "Away until Monday".into(), days: 1, ..Default::default() },
        include: None,
    };
    let saved = b.save_filters(&a, &wanted)?;
    assert_eq!(saved.filters.rules, wanted.rules);
    assert!(saved.filters.vacation.on && saved.filters.vacation.body == "Away until Monday");
    b.save_filters(&a, &wanted)?;
    assert_eq!(fake.lock().unwrap().rules.len(), 1, "changed in place");

    // IMAP is never tried.
    assert!(b.with_imap(&a, |_| Ok(())).is_err());
    drop(b);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

#[test]
fn exchange_calendars_contacts_and_directory_through_ews() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.items.push(crate::sync::ews::tests::FakeItem {
        id: "cal1".into(),
        folder: "f-cal".into(),
        mime: b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:weekly-1\r\nSUMMARY:Weekly\r\nDTSTART:20261130T090000Z\r\nDTEND:20261130T093000Z\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nORGANIZER:mailto:lead@corp.example\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:qa@corp.example\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:weekly-1\r\nRECURRENCE-ID:20261207T090000Z\r\nSUMMARY:Weekly (moved)\r\nDTSTART:20261207T100000Z\r\nDTEND:20261207T103000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n".to_vec(),
        subject: "Weekly".into(),
        read: true,
        flagged: false,
        categories: Vec::new(),
        verb: None,
        received: 0,
        flags: 1,
        kind: "CalendarItem",
        deferred: None,
    });
    fake.items.push(crate::sync::ews::tests::FakeItem {
        id: "ct1".into(),
        folder: "f-contacts".into(),
        mime: b"BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Ana Lima\r\nEMAIL:ana@example.org\r\nEND:VCARD\r\n".to_vec(),
        subject: "Ana Lima".into(),
        read: true,
        flagged: false,
        categories: Vec::new(),
        verb: None,
        received: 0,
        flags: 1,
        kind: "Contact",
        deferred: None,
    });
    fake.people.push(("Cy Park".into(), "cy@corp.example".into()));
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let cancel = Arc::new(AtomicBool::new(false));

    // A series with its changed occurrence, from the item's iCalendar.
    assert_eq!(b.use_mail_account_for_calendar(&a, cancel.clone())?, 1);
    assert!(b.server_schedules());
    let weekly = b.stored_event("weekly-1").unwrap();
    assert_eq!(weekly.event.recurrence, "FREQ=WEEKLY;COUNT=4");
    assert_eq!(weekly.overrides.len(), 1);
    assert_eq!(weekly.href, "ews:cal1");
    // Answered here: answered in the calendar too (the reply went by mail).
    b.file_answer(&weekly.event, &a.email, crate::sync::ical::PartStat::Accepted, true)?;
    assert_eq!(fake.lock().unwrap().answered, [("cal1".to_owned(), "AcceptItem".to_owned())]);
    // A new event: made from iCalendar.
    let made = b.save_event(crate::sync::ical::Event { uid: "new-1".into(), summary: "Planning".into(), start: 1_796_000_000, end: 1_796_003_600, ..Default::default() }, None)?;
    assert!(made.href.starts_with("ews:"));
    assert!(fake.lock().unwrap().items.iter().any(|i| i.kind == "CalendarItem" && String::from_utf8_lossy(&i.mime).contains("SUMMARY:Planning")));
    // Changed where Exchange won't keep two with the same UID: the earlier
    // one goes first, and only the changed one stays.
    fake.lock().unwrap().unique_uids = true;
    let changed = b.save_event(crate::sync::ical::Event { uid: "new-1".into(), summary: "Planning (moved)".into(), start: 1_796_007_200, end: 1_796_010_800, ..Default::default() }, None)?;
    assert_ne!(changed.href, made.href);
    {
        let f = fake.lock().unwrap();
        let planning: Vec<String> = f.items.iter().map(|i| String::from_utf8_lossy(&i.mime).into_owned()).filter(|m| m.contains("SUMMARY:Planning")).collect();
        assert_eq!(planning.len(), 1, "{planning:?}");
        assert!(planning[0].contains("SUMMARY:Planning (moved)"));
    }
    b.delete_event("new-1")?;
    assert!(!fake.lock().unwrap().items.iter().any(|i| String::from_utf8_lossy(&i.mime).contains("SUMMARY:Planning")));
    // Busy times.
    let busy = b.free_busy(&a.email, &["bo@corp.example".into()], 1_796_000_000, 1_796_086_400)?;
    assert_eq!(busy[0].busy.len(), 1);

    // Contacts, and the directory.
    assert_eq!(b.use_mail_account_for_contacts(&a, cancel)?, 1);
    assert!(b.contacts().iter().any(|c| c.name == "Ana Lima" && c.href == "ews:ct1"));
    let saved = b.save_contact(crate::sync::carddav::Card { name: "Bo Berg".into(), emails: vec!["bo@example.se".into()], ..Default::default() })?;
    assert!(saved.href.starts_with("ews:"));
    b.delete_contact(&saved.uid)?;
    assert!(!fake.lock().unwrap().items.iter().any(|i| i.kind == "Contact" && i.subject == "Bo Berg"));
    assert_eq!(b.search_server_contacts("cy")?, 1);
    assert!(b.contacts().iter().any(|c| c.emails == ["cy@corp.example"]));
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Snoozed mail waits in Picacho/Snoozed and comes back to the inbox,
/// unread, though each move gives it a new id.
#[test]
fn exchange_snoozes_and_wakes_mail() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.add("Later please", "f-inbox", 5);
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    let until = chrono::Utc::now().timestamp() + 1;
    let folder = b.snooze(&a, &inbox, until)?;
    assert_eq!(folder, "Picacho/Snoozed");
    {
        let f = fake.lock().unwrap();
        let snoozed = f.folders.iter().find(|x| x.name == "Snoozed").unwrap().id.clone();
        assert!(f.items.iter().any(|i| i.subject == "Later please" && i.folder == snoozed));
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let woken = b.wake_due(&a)?;
    assert_eq!(woken.len(), 1);
    {
        let f = fake.lock().unwrap();
        let item = f.items.iter().find(|i| i.subject == "Later please").unwrap();
        assert!(item.folder == "f-inbox" && !item.read);
    }
    let rows = b.store.summaries(&a.id, Mailbox::Standard(Folder::Inbox))?;
    assert!(rows.iter().any(|m| m.subject == "Later please" && m.unread));
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// New mail is heard of through Exchange's streaming subscription; the
/// mailbox's size is the folders' sizes added up.
#[test]
fn exchange_streams_new_mail_and_says_how_much_it_takes() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.add("Old", "f-inbox", 5);
    fake.stream = vec!["event"];
    let size: usize = fake.items.iter().map(|i| i.mime.len()).sum();
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let b = Arc::new(b);
    let (tell, heard) = std::sync::mpsc::channel();
    b.watch_inbox(&a, move |m| {
        let _ = tell.send(m);
    });
    let m = heard.recv_timeout(std::time::Duration::from_secs(5)).expect("heard");
    assert_eq!(m, Mailbox::Standard(Folder::Inbox));
    b.stop_watching(&a.id);
    assert!(fake.lock().unwrap().calls.iter().any(|c| c == "Subscribe"));
    let quota = b.quota(&a)?.expect("a size");
    assert_eq!((quota.used_kib, quota.limit_kib), (size as u64 / 1024, 0));
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Follow-up reminders at Exchange: an out-of-office reply is told by its
/// item class; a reply not downloaded is found in the conversation
/// (GetConversationItems) before reminding; the list is kept in a note in a
/// Picacho folder, which another computer reads.
#[test]
fn exchange_follow_ups_find_replies_and_share_through_a_note() -> Result<()> {
    use crate::manage::backend::FollowUp;
    let mut fake = Fake::new("secret");
    let sent = fake.add("Question", "f-sent", 120);
    fake.add("Question", "f-archive", 30);
    let away = fake.add("Out of office", "f-inbox", 10);
    fake.classes.push((away, "IPM.Note.Rules.OofTemplate.Microsoft".into()));
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    assert!(inbox.iter().all(|m| m.automatic() == (m.subject == "Out of office")));
    b.sync(&a, Folder::Sent, 50)?;
    let now = chrono::Utc::now().timestamp();
    let waiting = |id: &str, subject: &str, due: i64| FollowUp {
        message_id: id.into(),
        subject: subject.into(),
        to: "bo@example.se".into(),
        sent: now - 7200,
        due,
        ..Default::default()
    };
    b.note_follow_up(&a.id, waiting(&format!("<{sent}@corp.example>"), "Question", now - 60))?;
    b.note_follow_up(&a.id, waiting("<later@corp.example>", "Later", now + 86400))?;
    let check = b.check_follow_ups_online(&a, true)?;
    assert!(check.due.is_empty(), "{check:?}");
    assert_eq!(check.replied.iter().map(|(f, who)| (f.message_id.as_str(), who.as_str())).collect::<Vec<_>>(),
        [(format!("<{sent}@corp.example>").as_str(), "Bo Berg")]);
    let notes = fake
        .lock()
        .unwrap()
        .items
        .iter()
        .filter(|i| i.subject == "Picacho: follow-up reminders")
        .count();
    assert_eq!(notes, 1);
    // Another computer learns of the wait still on.
    let (b2, a2, path2) = setup(server.port)?;
    assert!(b2.sync_follow_ups(&a2, true)?);
    let ids: Vec<String> = b2.follow_ups(&a2.id).into_iter().map(|f| f.message_id).collect();
    assert_eq!(ids, ["<later@corp.example>"]);
    drop((b, b2));
    let _ = std::fs::remove_dir_all(path);
    let _ = std::fs::remove_dir_all(path2);
    Ok(())
}
