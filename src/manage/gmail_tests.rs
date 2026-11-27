// A Gmail account through the Gmail API, against the loopback fake
// (`sync/gmail/tests.rs`): the production backend end to end.
use super::*;
use crate::sync::gmail::tests::{Fake, serve};

/// A backend signed in with a token the fake takes, and a Gmail account
/// that uses the API at the fake's address.
fn setup(port: u16) -> Result<(Backend, Account, std::path::PathBuf)> {
    let path = std::env::temp_dir().join(format!("picacho-gmail-{}", uuid::Uuid::new_v4()));
    let mut b = Backend::new(Arc::new(Store::open(&path)?));
    b.test_secret = Some(Secret {
        access_token: "token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        ..Default::default()
    });
    let mut a = Account::new(Provider::Gmail);
    a.api = true;
    a.oauth = true;
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
fn gmail_api_accounts_read_change_and_send_without_imap() -> Result<()> {
    let mut fake = Fake::new();
    fake.labels.push(("Label_1".into(), "Receipts".into(), false));
    let lunch = fake.add("Lunch", &["INBOX", "UNREAD", "Label_1"], 5);
    let plans = fake.add("Plans", &["INBOX", "STARRED"], 10);
    let receipt = fake.add("Receipt", &["Label_1"], 20);
    let old = fake.add("Old news", &["IMPORTANT"], 30);
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    assert!(a.gmail_api() && !a.imap() && a.api_sending());

    // Inbox: its two messages, from their headers; the label is a folder.
    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    assert_eq!(inbox.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Lunch", "Plans"]);
    let first = &inbox[0];
    assert_eq!((first.folder, first.unread, first.starred), (Folder::Inbox, true, false));
    assert_eq!(first.sender, "Ana Lima");
    assert_eq!(first.labels(), ["Receipts"]);
    assert!(first.thread_id().starts_with("gmail:"));
    assert!(inbox[1].starred);
    assert_eq!(b.store.folders(&a.id).iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Receipts"]);
    // Archived mail with a label is under it; without, in Archive.
    let filed = b.sync(&a, Mailbox::Remote("Receipts".into()), 50)?;
    assert_eq!(filed.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Receipt"]);
    let archive = b.sync(&a, Folder::Archive, 50)?;
    assert_eq!(archive.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Old news"]);

    // Read, starred and archived here: Gmail relabels them.
    b.change_many(&a, &inbox[..1], Change::Read(true))?;
    b.change_many(&a, &inbox[..1], Change::Star(true))?;
    {
        let f = fake.lock().unwrap();
        let labels = &f.message(&lunch).unwrap().labels;
        assert!(!labels.contains(&"UNREAD".into()) && labels.contains(&"STARRED".into()), "{labels:?}");
    }
    let current = b.store.message(&inbox[1].id)?.unwrap();
    b.change_many(&a, &[current], Change::Move(Folder::Archive))?;
    assert!(!fake.lock().unwrap().message(&plans).unwrap().labels.contains(&"INBOX".into()));
    assert_eq!(b.store.message(&inbox[1].id)?.unwrap().folder, Folder::Archive);
    // Into a label of yours: out of where it was.
    let r = b.store.message(&gmail::row_id(&a, &receipt))?.unwrap();
    b.create_remote_folder(&a, None, "Taxes")?;
    b.change_many(&a, &[r], Change::MoveTo("Taxes".into()))?;
    {
        let f = fake.lock().unwrap();
        let taxes = f.labels.iter().find(|(_, n, _)| n == "Taxes").unwrap().0.clone();
        assert_eq!(f.message(&receipt).unwrap().labels, [taxes]);
    }

    // Changed elsewhere: the history says so, and only those are read.
    {
        let mut f = fake.lock().unwrap();
        f.messages.iter_mut().find(|m| m.id == lunch).unwrap().labels.push("UNREAD".into());
        f.changed(&lunch, false);
        f.messages.retain(|m| m.id != old);
        f.changed(&old, true);
    }
    b.sync(&a, Folder::Inbox, 50)?;
    assert!(b.store.message(&gmail::row_id(&a, &lunch))?.unwrap().unread);
    assert!(b.store.message(&gmail::row_id(&a, &old))?.is_none(), "deleted elsewhere");

    // Opened: its source, from Gmail.
    let row = b.store.message(&gmail::row_id(&a, &lunch))?.unwrap();
    let read = b.read(&a, &row)?;
    assert_eq!(read.body.as_deref().map(str::trim), Some("About Lunch."));

    // Sent through Gmail, Bcc named for it to take out; Gmail files it.
    let d = Draft {
        id: "d1".into(),
        account: a.id.clone(),
        to: "bo@example.se".into(),
        bcc: "cy@example.net".into(),
        subject: "Hello".into(),
        body: "Hi there".into(),
        ..Default::default()
    };
    b.send(&a, &d)?;
    {
        let f = fake.lock().unwrap();
        let sent = String::from_utf8_lossy(&f.sent[0]).into_owned();
        assert!(sent.contains("To: bo@example.se") && sent.contains("Bcc: cy@example.net"), "{sent}");
        assert!(sent.contains("Subject: Hello"));
    }

    // Drafts in step both ways.
    let mut draft = Draft {
        id: "d2".into(),
        account: a.id.clone(),
        to: "bo@example.se".into(),
        subject: "Later".into(),
        body: "Not yet".into(),
        ..Default::default()
    };
    b.store.save_draft(&draft)?;
    b.sync_drafts(&a)?;
    assert_eq!(fake.lock().unwrap().drafts.len(), 1);
    draft = b.store.draft("d2")?.unwrap();
    assert!(draft.remote.is_some());
    b.discard_draft("d2")?;
    b.sync_drafts(&a)?;
    assert!(fake.lock().unwrap().drafts.is_empty(), "discarded here, gone there");

    // Search: Gmail's own.
    let found = b.search_server(&a, &Folder::Inbox.into(), "Lunch", 10)?;
    assert_eq!(found.total, 1);

    // Deleted for good from Trash.
    let row = b.store.message(&gmail::row_id(&a, &lunch))?.unwrap();
    b.change_many(&a, &[row], Change::Move(Folder::Trash))?;
    assert_eq!(b.empty_mailbox(&a, Folder::Trash)?, 1);
    assert!(fake.lock().unwrap().message(&lunch).is_none());

    // Server filters: Gmail's own, and the away reply.
    use crate::sync::sieve::{Act, Field, Filters, Rule, Vacation};
    let wanted = Filters {
        rules: vec![Rule { field: Field::From, value: "shop@example.com".into(), act: Act::Folder("Taxes".into()), ..Default::default() }],
        vacation: Vacation { on: true, subject: "Away".into(), body: "Back soon".into(), days: 1, ..Default::default() },
        include: None,
    };
    let saved = b.save_filters(&a, &wanted)?;
    assert_eq!(saved.filters.rules, wanted.rules);
    assert_eq!(b.filters(&a)?.filters.vacation.subject, "Away");
    assert_eq!(fake.lock().unwrap().vacation["enableAutoReply"], true);
    // Saved again: replaced, not added twice.
    b.save_filters(&a, &wanted)?;
    assert_eq!(fake.lock().unwrap().filters.len(), 1);

    // IMAP is never tried.
    let error = b.with_imap(&a, |_| Ok(())).unwrap_err();
    assert!(error.to_string().contains("API"), "{error}");
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// New mail is noticed within moments, by asking Gmail whether anything
/// changed (its pushes can't reach a desktop app).
#[test]
fn new_gmail_mail_is_noticed_without_push() -> Result<()> {
    let (server, fake) = serve(Fake::new());
    let (b, a, path) = setup(server.port)?;
    let b = Arc::new(b);
    let (tell, heard) = std::sync::mpsc::channel();
    b.watch_inbox(&a, move |m| {
        let _ = tell.send(m);
    });
    // Its first answer is where it starts from.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(heard.try_recv().is_err(), "nothing changed yet");
    fake.lock().unwrap().add("Fresh", &["INBOX", "UNREAD"], 0);
    let m = heard.recv_timeout(std::time::Duration::from_secs(5)).expect("noticed");
    assert_eq!(m, Mailbox::Standard(Folder::Inbox));
    b.stop_watching(&a.id);
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Switching how an account is reached forgets its cached mail (other ids
/// now) and keeps its drafts, which upload afresh.
#[test]
fn switching_forgets_mail_and_keeps_drafts() -> Result<()> {
    let mut fake = Fake::new();
    fake.add("Kept on the server", &["INBOX"], 5);
    let (server, _fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    assert_eq!(b.sync(&a, Folder::Inbox, 50)?.len(), 1);
    b.store.save_draft(&Draft {
        id: "d".into(),
        account: a.id.clone(),
        subject: "Mine".into(),
        remote: Some(RemoteDraft { mailbox: "Drafts".into(), validity: 1, uid: 9 }),
        uploaded: "x".into(),
        ..Default::default()
    })?;
    b.store.forget_mail(&a.id)?;
    assert_eq!(b.store.count_messages(&a.id, Mailbox::Standard(Folder::Inbox))?, 0);
    let d = b.store.draft("d")?.unwrap();
    assert!(d.remote.is_none() && d.uploaded.is_empty());
    assert!(b.store.accounts()?.iter().any(|x| x.id == a.id));
    // Listed again, the new way.
    assert_eq!(b.sync(&a, Folder::Inbox, 50)?.len(), 1);
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Gmail's own categories (CATEGORY_PROMOTIONS…) come with each message,
/// and its inbox's tabs follow them.
#[test]
fn gmails_categories_sort_the_inbox() -> Result<()> {
    use crate::manage::categories::{Categorizing, Category, Known};
    let mut fake = Fake::new();
    for id in [
        "CATEGORY_PERSONAL",
        "CATEGORY_PROMOTIONS",
        "CATEGORY_UPDATES",
    ] {
        fake.labels.push((id.into(), id.into(), true));
    }
    let lunch = fake.add("Lunch", &["INBOX", "CATEGORY_PERSONAL"], 5);
    let sale = fake.add("A note from us", &["INBOX", "CATEGORY_PROMOTIONS"], 10);
    let friends = fake.add("New follower", &["INBOX", "CATEGORY_SOCIAL"], 15);
    let (server, _fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    b.sync(&a, Folder::Inbox, 50)?;
    let prefs = Categorizing::default();
    let category = |id: &str| {
        let m = b.store.message(&gmail::row_id(&a, id)).unwrap().unwrap();
        (
            m.server_category().to_owned(),
            prefs.category(&m, &Known::default()),
        )
    };
    assert_eq!(category(&lunch), ("personal".into(), Category::Primary));
    assert_eq!(category(&sale), ("promotions".into(), Category::Promotions));
    assert_eq!(category(&friends), ("social".into(), Category::Social));
    // Gmail's categories aren't the person's labels.
    let sale = b.store.message(&gmail::row_id(&a, &sale))?.unwrap();
    assert!(sale.labels().is_empty());
    drop(b);
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Follow-up reminders through the Gmail API: an automatic reply is told
/// by its headers; a reply not downloaded is found in the message's thread
/// before reminding; the list is kept in a note under a Picacho label, out
/// of the inbox, which another computer reads.
#[test]
fn gmail_follow_ups_find_replies_and_share_through_a_note() -> Result<()> {
    use crate::manage::backend::FollowUp;
    let mut fake = Fake::new();
    let sent = fake.add("Question", &["SENT"], 120);
    let reply = fake.add("Re: Question", &["Label_archive_none"], 30);
    let away = fake.add("Away", &["INBOX"], 10);
    let thread = fake.message(&sent).unwrap().thread.clone();
    for m in &mut fake.messages {
        if m.id == reply {
            m.thread = thread.clone();
        }
        if m.id == away {
            m.raw = [b"Auto-Submitted: auto-replied\r\n".as_slice(), &m.raw].concat();
        }
    }
    let (server, fake) = serve(fake);
    let (b, a, path) = setup(server.port)?;
    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    assert_eq!(inbox.iter().map(|m| (m.subject.as_str(), m.automatic())).collect::<Vec<_>>(), [("Away", true)]);
    b.sync(&a, Folder::Sent, 50)?;
    let now = chrono::Utc::now().timestamp();
    let waiting = |id: &str, subject: &str, due: i64| FollowUp {
        message_id: id.into(),
        subject: subject.into(),
        to: "ana@example.org".into(),
        sent: now - 7200,
        due,
        ..Default::default()
    };
    b.note_follow_up(&a.id, waiting(&format!("<{sent}@example.org>"), "Question", now - 60))?;
    b.note_follow_up(&a.id, waiting("<later@example.com>", "Later", now + 86400))?;
    let check = b.check_follow_ups_online(&a, true)?;
    assert!(check.due.is_empty(), "{check:?}");
    assert_eq!(check.replied.len(), 1);
    assert_eq!(check.replied[0].1, "Ana Lima");
    {
        let f = fake.lock().unwrap();
        let label = f.labels.iter().find(|(_, name, _)| name == "Picacho").map(|(id, _, _)| id.clone()).expect("a Picacho label");
        let notes: Vec<_> = f.messages.iter().filter(|m| m.labels.contains(&label)).collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].labels, std::slice::from_ref(&label), "out of the inbox");
    }
    let (b2, a2, path2) = setup(server.port)?;
    assert!(b2.sync_follow_ups(&a2, true)?);
    let ids: Vec<String> = b2.follow_ups(&a2.id).into_iter().map(|f| f.message_id).collect();
    assert_eq!(ids, ["<later@example.com>"]);
    drop((b, b2));
    let _ = std::fs::remove_dir_all(path);
    let _ = std::fs::remove_dir_all(path2);
    Ok(())
}
