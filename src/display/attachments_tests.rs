//! The attachment browser (`attachments.rs`) and the reader's contact
//! panel (`contact_panel.rs`).
use super::attachments::LibAction;
use super::contact_panel::PanelAction;
use super::tests::{act, frame, two_accounts};
use super::*;
use std::prelude::v1::test;

fn file(name: &str, mime: &str, size: usize, part: usize) -> Attachment {
    Attachment {
        name: name.into(),
        size,
        mime: mime.into(),
        part,
        section: String::new(),
        encoding: String::new(),
    }
}

/// A received message from `from` with `files` attached.
fn with_files(account: &str, id: &str, from: &str, date: i64, files: Vec<Attachment>) -> Mail {
    let mut m = Mail {
        id: format!("{account}:{id}"),
        account: account.into(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        from: from.into(),
        sender: from.split('@').next().unwrap_or_default().into(),
        to: "work@example.com".into(),
        subject: format!("About {id}"),
        snippet: format!("Files for {id}"),
        date,
        body: Some(format!("Files for {id}")),
        message_id: format!("<{id}@{account}.example.com>"),
        has_attachments: true,
        ..Mail::default()
    };
    m.set_attachments(files);
    m
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// How many attachment rows are drawn.
fn rows(cx: &mut VisualTestContext) -> usize {
    frame(cx);
    (0..50).take_while(|i| cx.debug_bounds(leak(format!("attachment-row-{i}"))).is_some()).count()
}

/// Mail with attachments in both accounts: Ana's (an invoice and a photo,
/// and a report later), Bo's (minutes), and one in the second account.
fn seeded(cx: &mut TestAppContext) -> (Entity<MailApp>, &mut VisualTestContext, Arc<Store>, String, String) {
    let (view, cx, store) = two_accounts(cx);
    let (first, second) = view.read_with(cx, |this, _| (this.accounts[0].id.clone(), this.accounts[1].id.clone()));
    let now = chrono::Utc::now().timestamp();
    store
        .save_messages(&[
            with_files(&first, "invoice", "ana@example.com", now - 300, vec![
                file("Invoice-0042.pdf", "application/pdf", 40_000, 1),
                file("photo.png", "image/png", 2_000, 2),
            ]),
            with_files(&first, "minutes", "bo@example.com", now - 200, vec![file("minutes.docx", "", 9_000, 1)]),
            with_files(&first, "report", "ana@example.com", now - 100, vec![file("q3.csv", "text/csv", 500, 1)]),
            with_files(&second, "home", "cy@example.com", now - 50, vec![file("lease.pdf", "application/pdf", 70_000, 1)]),
        ])
        .unwrap();
    (view, cx, store, first, second)
}

#[gpui::test]
fn the_attachment_browser_lists_filters_sorts_and_picks(cx: &mut TestAppContext) {
    let (view, cx, _store, first, second) = seeded(cx);
    frame(cx);
    assert!(cx.debug_bounds("open-attachments").is_some(), "in the sidebar");
    // From the sidebar: the account open, or All Accounts.
    act(&view, Action::Calendar(calendar_views::CalAction::Open(calendar_views::AppView::Attachments)), cx);
    frame(cx);
    assert!(cx.debug_bounds("attachments-page").is_some());
    // Every account's.
    act(&view, Action::Attachments(LibAction::Open { account: ALL_MAIL.into(), mailbox: None, person: None }), cx);
    view.read_with(cx, |this, _| assert_eq!(this.app_view, Some(calendar_views::AppView::Attachments)));
    assert_eq!(rows(cx), 5);
    // One account's.
    act(&view, Action::Attachments(LibAction::Open { account: second.clone(), mailbox: None, person: None }), cx);
    assert_eq!(rows(cx), 1);
    act(&view, Action::Attachments(LibAction::Open { account: first.clone(), mailbox: None, person: None }), cx);
    assert_eq!(rows(cx), 4);
    // By kind.
    act(&view, Action::Attachments(LibAction::Kind(Some(crate::manage::attachments::Kind::Pdf))), cx);
    assert_eq!(rows(cx), 1);
    frame(cx);
    assert!(cx.debug_bounds("attachments-kind-pdf").is_some());
    act(&view, Action::Attachments(LibAction::Kind(None)), cx);
    // By words in names and senders.
    view.update_in(cx, |this, w, cx| MailApp::set(&this.library.search, "ana", w, cx));
    cx.run_until_parked();
    assert_eq!(rows(cx), 3);
    // In a name, and in the subject of the message both files came with.
    view.update_in(cx, |this, w, cx| MailApp::set(&this.library.search, "invoice", w, cx));
    cx.run_until_parked();
    assert_eq!(rows(cx), 2);
    view.update_in(cx, |this, w, cx| MailApp::set(&this.library.search, "0042", w, cx));
    cx.run_until_parked();
    assert_eq!(rows(cx), 1);
    view.update_in(cx, |this, w, cx| MailApp::set(&this.library.search, "", w, cx));
    cx.run_until_parked();
    // Largest first.
    act(&view, Action::Attachments(LibAction::Sort(crate::manage::attachments::Sort::Largest)), cx);
    let names = |view: &Entity<MailApp>, cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| {
            this.library_shown_names()
        })
    };
    assert_eq!(names(&view, cx), ["Invoice-0042.pdf", "minutes.docx", "photo.png", "q3.csv"]);
    // Picked, then filtered: only what is shown stays picked.
    act(&view, Action::Attachments(LibAction::PickAll), cx);
    view.read_with(cx, |this, _| assert_eq!(this.library_picked(), 4));
    frame(cx);
    assert!(cx.debug_bounds("attachments-save-picked").is_some());
    act(&view, Action::Attachments(LibAction::Kind(Some(crate::manage::attachments::Kind::Image))), cx);
    view.read_with(cx, |this, _| assert_eq!(this.library_picked(), 1));
    act(&view, Action::Attachments(LibAction::PickNone), cx);
    view.read_with(cx, |this, _| assert_eq!(this.library_picked(), 0));
    act(&view, Action::Attachments(LibAction::Kind(None)), cx);
    // One person's: from the contact panel.
    act(&view, Action::Attachments(LibAction::Open { account: ALL_MAIL.into(), mailbox: None, person: Some("Ana@example.com".into()) }), cx);
    assert_eq!(rows(cx), 3);
    frame(cx);
    assert!(cx.debug_bounds("attachments-person").is_some());
    act(&view, Action::Attachments(LibAction::ClearPerson), cx);
    assert_eq!(rows(cx), 5);
    // Back to mail.
    act(&view, Action::Calendar(calendar_views::CalAction::Close), cx);
    view.read_with(cx, |this, _| assert!(this.app_view.is_none()));
}

#[gpui::test]
fn a_mailboxs_menu_browses_its_attachments_and_a_preview_opens_its_message(cx: &mut TestAppContext) {
    let (view, cx, _store, first, _) = seeded(cx);
    let entries = view.read_with(cx, |this, _| this.mailbox_entries(&first, Folder::Inbox.into()));
    let browse = entries
        .into_iter()
        .find_map(|e| match e {
            Entry::Item { action: action @ Action::Attachments(LibAction::Open { .. }), .. } => Some(action),
            _ => None,
        })
        .expect("Browse Attachments in the mailbox's menu");
    act(&view, browse, cx);
    assert_eq!(rows(cx), 4);
    frame(cx);
    assert!(cx.debug_bounds("attachments-every-mailbox").is_some(), "only the inbox's, and a way to every mailbox");
    // A file without a preview: said so, with what can be done with it.
    let item = view.read_with(cx, |this, _| this.library_item("minutes.docx"));
    act(&view, Action::Attachments(LibAction::Focus(item.clone())), cx);
    frame(cx);
    assert!(cx.debug_bounds("attachment-preview").is_some());
    assert!(cx.debug_bounds("attachment-preview-open").is_some());
    // Its message, in its mailbox.
    act(&view, Action::Attachments(LibAction::ShowMessage(item.clone())), cx);
    view.read_with(cx, |this, _| {
        assert!(this.app_view.is_none());
        assert_eq!(this.selected.as_deref(), Some(item.mail.id.as_str()));
    });
}

#[gpui::test]
fn the_browser_finds_attachments_by_what_they_say(cx: &mut TestAppContext) {
    let (view, cx, store, first, _) = seeded(cx);
    // The report's CSV, downloaded and read into the index.
    let raw = "From: ana@example.com\r\nSubject: About report\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nFiles.\r\n--b\r\nContent-Type: text/csv; name=\"q3.csv\"\r\nContent-Disposition: attachment; filename=\"q3.csv\"\r\n\r\nregion,revenue\r\nPatagonia,1200\r\n--b--\r\n";
    store.save_raw(&format!("{first}:report"), raw.as_bytes()).unwrap();
    while store.index_attachments(10).unwrap() > 0 {}
    act(&view, Action::Attachments(LibAction::Open { account: ALL_MAIL.into(), mailbox: None, person: None }), cx);
    view.update_in(cx, |this, w, cx| MailApp::set(&this.library.search, "patagonia", w, cx));
    cx.run_until_parked();
    // Nothing named so: found once typing pauses and the index answers.
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert_eq!(this.library_shown_names(), ["q3.csv"]));
    // Its text, previewed.
    let item = view.read_with(cx, |this, _| this.library_item("q3.csv"));
    act(&view, Action::Attachments(LibAction::Focus(item)), cx);
    frame(cx);
    assert!(cx.debug_bounds("attachment-preview-text").is_some());
}

#[gpui::test]
fn the_contact_panel_shows_the_person_their_mail_files_and_card(cx: &mut TestAppContext) {
    let (view, cx, store, first, _) = seeded(cx);
    let card = crate::sync::carddav::Card {
        uid: "ana-1".into(),
        name: "Ana Silva".into(),
        emails: vec!["ana@example.com".into()],
        phones: vec!["+1 555 0100".into()],
        org: "Example Co".into(),
        ..Default::default()
    };
    store.set_preference("contacts", &serde_json::to_string(&[card]).unwrap()).unwrap();
    view.update_in(cx, |this, w, cx| this.open_mailbox(Some(first.clone()), Folder::Inbox.into(), w, cx));
    cx.run_until_parked();
    act(&view, Action::Select(format!("{first}:report")), cx);
    frame(cx);
    assert!(cx.debug_bounds("contact-panel-toggle").is_some(), "in the reader's toolbar");
    assert!(cx.debug_bounds("contact-panel").is_none(), "closed at first");
    act(&view, Action::ContactPanel(PanelAction::Toggle), cx);
    frame(cx);
    cx.run_until_parked();
    frame(cx);
    assert!(cx.debug_bounds("contact-panel").is_some());
    view.read_with(cx, |this, _| {
        let p = this.contact_panel_person().expect("Ana, read");
        assert_eq!(p.name, "Ana Silva");
        // Her other mail (not the one open), and its attachments.
        assert_eq!(p.recent.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["About invoice"]);
        assert_eq!(
            p.attachments.iter().map(|i| i.attachment().name.as_str()).collect::<Vec<_>>(),
            ["Invoice-0042.pdf", "photo.png"]
        );
        assert!(p.events.is_empty());
    });
    assert!(cx.debug_bounds("contact-panel-mail-0").is_some());
    assert!(cx.debug_bounds("contact-panel-attachment-1").is_some());
    assert!(cx.debug_bounds("contact-panel-browse").is_some());
    // Kept open for the next message, and the person follows it.
    act(&view, Action::Select(format!("{first}:minutes")), cx);
    frame(cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let p = this.contact_panel_person().expect("Bo, read");
        assert_eq!(p.address, "bo@example.com");
        assert!(p.card.is_none());
    });
    // Closed again, and remembered.
    act(&view, Action::ContactPanel(PanelAction::Toggle), cx);
    frame(cx);
    assert!(cx.debug_bounds("contact-panel").is_none());
    store.flush();
    assert_eq!(store.preference("contact_panel").as_deref(), Some("0"));
}

/// The panel stays inside the reader at every desktop size: beside the
/// email where there's room, over its right edge where there isn't, and
/// not at all where the reader is alone in the window. The browser fits
/// too, its preview beside the list or in its place.
#[gpui::test]
fn the_panel_and_the_browser_fit_every_window_size(cx: &mut TestAppContext) {
    let (view, cx, _store, first, _) = seeded(cx);
    let inside = |inner: Bounds<Pixels>, outer: Bounds<Pixels>, what: &str| {
        let e = px(1.);
        assert!(
            inner.left() >= outer.left() - e && inner.right() <= outer.right() + e
                && inner.top() >= outer.top() - e && inner.bottom() <= outer.bottom() + e,
            "{what}: {inner:?} outside {outer:?}"
        );
    };
    view.update_in(cx, |this, w, cx| this.open_mailbox(Some(first.clone()), Folder::Inbox.into(), w, cx));
    cx.run_until_parked();
    act(&view, Action::Select(format!("{first}:report")), cx);
    act(&view, Action::ContactPanel(PanelAction::Toggle), cx);
    for (width, height) in [(1440., 1000.), (1180., 700.), (960., 600.), (899., 600.), (640., 480.)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        frame(cx);
        let panel = cx.debug_bounds("contact-panel");
        if width < 900. {
            assert!(panel.is_none(), "{width}: the reader is alone; no room for the panel");
            continue;
        }
        let panel = panel.unwrap_or_else(|| panic!("{width}: the panel is missing"));
        let reader = cx.debug_bounds("reader-pane").expect("reader");
        inside(panel, reader, &format!("{width}×{height}: panel in the reader"));
    }
    act(&view, Action::Attachments(LibAction::Open { account: ALL_MAIL.into(), mailbox: None, person: None }), cx);
    let item = view.read_with(cx, |this, _| this.library_item("minutes.docx"));
    act(&view, Action::Attachments(LibAction::Focus(item)), cx);
    for (width, height) in [(1440., 1000.), (960., 600.), (640., 480.), (360., 320.)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        frame(cx);
        let page = cx.debug_bounds("attachments-page").expect("the browser");
        let window = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
        inside(page, window, &format!("{width}×{height}: the browser in the window"));
        let preview = cx.debug_bounds("attachment-preview").expect("the preview");
        inside(preview, page, &format!("{width}×{height}: the preview in the browser"));
        // Side by side when wide (960 less the sidebar's rail is room
        // enough); the preview alone when narrow.
        assert_eq!(cx.debug_bounds("attachment-row-0").is_some(), width >= 960., "{width}");
    }
}

/// ⌥⌘I (Ctrl+Alt+I) opens and closes the contact panel, and its button
/// says so.
#[gpui::test]
fn a_shortcut_toggles_the_contact_panel(cx: &mut TestAppContext) {
    let (view, cx, _store, first, _) = seeded(cx);
    view.update_in(cx, |this, w, cx| this.open_mailbox(Some(first.clone()), Folder::Inbox.into(), w, cx));
    cx.run_until_parked();
    act(&view, Action::Select(format!("{first}:report")), cx);
    cx.update(|_, cx| {
        let bindings = super::app_bindings(cx);
        cx.bind_keys(bindings);
    });
    view.read_with(cx, |_, cx| {
        let keys = shortcuts::current("toggle-contact-panel", cx).expect("bound by default");
        assert!(keys.to_lowercase().contains('i'), "{keys}");
    });
    view.update_in(cx, |this, w, cx| w.focus(&this.focus, cx));
    cx.simulate_keystrokes("secondary-alt-i");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.contact_panel.open));
    frame(cx);
    assert!(cx.debug_bounds("contact-panel").is_some());
    cx.simulate_keystrokes("secondary-alt-i");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(!this.contact_panel.open));
}

/// ⌥⌘A (Ctrl+Alt+A) opens the attachment browser, and again goes back to
/// mail.
#[gpui::test]
fn a_shortcut_opens_the_attachment_browser(cx: &mut TestAppContext) {
    let (view, cx, _store, _, _) = seeded(cx);
    cx.update(|_, cx| {
        let bindings = super::app_bindings(cx);
        cx.bind_keys(bindings);
    });
    view.update_in(cx, |this, w, cx| w.focus(&this.focus, cx));
    cx.simulate_keystrokes("secondary-alt-a");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert_eq!(this.app_view, Some(calendar_views::AppView::Attachments)));
    assert!(rows(cx) > 0);
    cx.simulate_keystrokes("secondary-alt-a");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.app_view.is_none()));
}
