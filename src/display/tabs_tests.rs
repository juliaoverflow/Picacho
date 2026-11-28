//! Tabs in the window: one tab shows only the New Tab button; several show
//! a row of tabs, each keeping its mailbox, search and open message; tabs
//! close, move, and open again after a restart.
use super::tabs::TabAction;
use super::tests::{act, frame, open, two_accounts};
use super::*;
use std::prelude::v1::test;

fn tab(action: TabAction) -> Action {
    Action::Tabs(action)
}

#[gpui::test]
fn mailboxes_and_searches_stay_open_in_tabs(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    let (work, home) = view.read_with(cx, |this, _| {
        (this.accounts[0].id.clone(), this.accounts[1].id.clone())
    });
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    cx.run_until_parked();
    frame(cx);

    // One tab isn't shown as a tab: only the button for another.
    assert!(cx.debug_bounds("new-tab").is_some());
    assert!(cx.debug_bounds("tabs").is_none());
    assert!(cx.debug_bounds("tab-0").is_none());

    // A second tab opens the same mailbox afresh, and shows both.
    let first = view.read_with(cx, |this, _| this.messages[0].id.clone());
    act(&view, Action::Select(first.clone()), cx);
    act(&view, tab(TabAction::New(None)), cx);
    cx.run_until_parked();
    frame(cx);
    assert!(cx.debug_bounds("tabs").is_some());
    assert!(cx.debug_bounds("tab-0").is_some());
    assert!(cx.debug_bounds("tab-1").is_some());
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 2);
        assert_eq!(this.tabs.active, 1);
        assert_eq!(this.account.as_ref(), Some(&work));
        assert_eq!(this.folder, Folder::Inbox);
        assert!(this.selected.is_none(), "a new tab opens no message");
    });

    // The second tab searches another account's Receipts.
    let receipts = Mailbox::Remote("INBOX/Receipts".into());
    act(
        &view,
        Action::OpenMailbox(home.clone(), receipts.clone()),
        cx,
    );
    act(&view, Action::SearchFor("weekend".into()), cx);
    cx.run_until_parked();

    // Back in the first: its Inbox, without the search, its message open.
    act(&view, tab(TabAction::Switch(0)), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        assert_eq!(this.tabs.active, 0);
        assert_eq!(this.account.as_ref(), Some(&work));
        assert_eq!(this.folder, Folder::Inbox);
        assert!(!this.search_open);
        assert_eq!(MailApp::value(&this.search, cx), "");
        assert_eq!(this.selected.as_ref(), Some(&first));
    });

    // And the second as it was left.
    act(&view, tab(TabAction::Cycle(true)), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        assert_eq!(this.tabs.active, 1);
        assert_eq!(this.account.as_ref(), Some(&home));
        assert_eq!(this.mailbox(), receipts);
        assert!(this.search_open);
        assert_eq!(MailApp::value(&this.search, cx), "weekend");
    });

    // A sidebar folder opens in a third tab, after the open one.
    act(
        &view,
        tab(TabAction::New(Some((work.clone(), Folder::Sent.into())))),
        cx,
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 3);
        assert_eq!(this.tabs.active, 2);
        assert_eq!(this.folder, Folder::Sent);
    });

    // Moved first, the open tab stays open.
    act(&view, tab(TabAction::Move(2, 0)), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.active, 0);
        assert_eq!(this.folder, Folder::Sent);
        assert_eq!(this.tabs.list[1].place.mailbox, Folder::Inbox.into());
        assert_eq!(this.tabs.list[2].place.mailbox, receipts);
    });

    // The tabs open again after a restart, with the open one's search.
    act(&view, tab(TabAction::Switch(2)), cx);
    cx.run_until_parked();
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, cx| {
        assert_eq!(this.tabs.list.len(), 3);
        assert_eq!(this.tabs.active, 2);
        assert_eq!(this.account.as_ref(), Some(&home));
        assert_eq!(this.mailbox(), receipts);
        assert!(this.search_open);
        assert_eq!(MailApp::value(&this.search, cx), "weekend");
    });

    // Closing the open tab opens the one before it; closing all but one
    // leaves no tabs shown.
    act(&restarted, tab(TabAction::CloseActive), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 2);
        assert_eq!(this.tabs.active, 1);
        assert_eq!(this.account.as_ref(), Some(&work));
        assert_eq!(this.folder, Folder::Inbox);
    });
    act(&restarted, tab(TabAction::CloseOthers(0)), cx);
    cx.run_until_parked();
    frame(cx);
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 1);
        assert_eq!(this.folder, Folder::Sent);
    });
    assert!(cx.debug_bounds("tabs").is_none());
    assert!(cx.debug_bounds("new-tab").is_some());
}

#[gpui::test]
fn a_tab_whose_folder_is_gone_opens_its_inbox(cx: &mut TestAppContext) {
    let (view, cx, _store) = two_accounts(cx);
    let home = view.read_with(cx, |this, _| this.accounts[1].id.clone());
    act(
        &view,
        Action::OpenMailbox(home.clone(), Mailbox::Remote("INBOX/Receipts".into())),
        cx,
    );
    act(
        &view,
        tab(TabAction::New(Some((home.clone(), Folder::Sent.into())))),
        cx,
    );
    cx.run_until_parked();
    // The folder is deleted elsewhere meanwhile.
    view.update(cx, |this, _| {
        this.folders.insert(home.clone(), Vec::new());
    });
    act(&view, tab(TabAction::Switch(0)), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(this.account.as_ref(), Some(&home));
        assert_eq!(this.folder, Folder::Inbox);
    });
}

#[gpui::test]
fn tabs_are_clicked_dragged_and_closed(cx: &mut TestAppContext) {
    let (view, cx, _store) = two_accounts(cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    let work = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    cx.run_until_parked();
    frame(cx);

    // The button opens a second tab.
    let at = cx.debug_bounds("new-tab").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Sent.into()),
        cx,
    );
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 2);
        assert_eq!(this.tabs.active, 1);
    });

    // Clicking the first opens it.
    let at = cx.debug_bounds("tab-0").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.active, 0);
        assert_eq!(this.folder, Folder::Inbox);
    });

    // Dragged after the second, it moves there and stays open.
    let from = cx.debug_bounds("tab-0").unwrap().center();
    let to = cx.debug_bounds("tab-1").unwrap().center();
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for i in 1..=8 {
        let t = i as f32 / 8.;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
    }
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.active, 1);
        assert_eq!(this.folder, Folder::Inbox);
        assert_eq!(this.tabs.list[0].place.mailbox, Folder::Sent.into());
    });

    // Its close button closes it, and the other opens.
    let at = cx.debug_bounds("tab-close-1").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.list.len(), 1);
        assert_eq!(this.folder, Folder::Sent);
    });
    assert!(cx.debug_bounds("tabs").is_none());
}

#[gpui::test]
fn server_searches_and_results_on_this_computer_stay_in_tabs(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let work = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    cx.run_until_parked();
    act(&view, Action::SearchFor("weekend".into()), cx);
    // The server found a message the mailbox's list doesn't have.
    let found = format!("{work}:server-1");
    view.update(cx, |this, cx| {
        this.messages.push(Mail {
            id: found.clone(),
            account: work.clone(),
            folder: Folder::Inbox,
            sender: "Old Friend".into(),
            from: "old@example.com".into(),
            subject: "That weekend, years ago".into(),
            date: 1,
            ..Mail::default()
        });
        this.server_search = Some(later::ServerSearch {
            account: work.clone(),
            mailbox: Folder::Inbox.into(),
            query: "weekend".into(),
            running: false,
            ids: [found.clone()].into_iter().collect(),
            total: 7,
            error: None,
        });
        this.apply_filter(cx);
    });
    let shows_found = |this: &MailApp| this.visible.iter().any(|&i| this.messages[i].id == found);
    view.read_with(cx, |this, _| assert!(shows_found(this)));

    // Another tab, searching every account's mail on this computer.
    act(&view, tab(TabAction::New(None)), cx);
    cx.run_until_parked();
    act(&view, Action::SearchFor("plans".into()), cx);
    act(
        &view,
        Action::Organize(organize::OrgAction::SearchEverywhere),
        cx,
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.local_results.is_some());
        assert!(this.server_search.is_none());
    });

    // Back in the first tab, the server's results show again, unasked.
    act(&view, tab(TabAction::Switch(0)), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        assert!(this.local_results.is_none());
        assert_eq!(MailApp::value(&this.search, cx), "weekend");
        let search = this.server_search.as_ref().expect("the server search");
        assert!(!search.running);
        assert_eq!(search.total, 7);
        assert!(search.ids.contains(&found));
        assert!(shows_found(this), "the server's result is listed again");
    });

    // And the second tab's results on this computer, with where they
    // were opened from.
    act(&view, tab(TabAction::Switch(1)), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        let results = this
            .local_results
            .as_ref()
            .expect("results on this computer");
        assert_eq!(results.back, Some((work.clone(), Folder::Inbox.into())));
        assert_eq!(this.account.as_deref(), Some(fwd::ALL_MAIL));
        assert_eq!(this.folder, Folder::All);
        assert_eq!(MailApp::value(&this.search, cx), "plans");
    });

    // After a restart, both come back: the results on this computer as
    // they were, the server search asked of the server again.
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, cx| {
        assert_eq!(this.tabs.active, 1);
        assert!(this.local_results.is_some());
        assert_eq!(MailApp::value(&this.search, cx), "plans");
    });
    act(&restarted, tab(TabAction::Switch(0)), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, cx| {
        assert!(this.local_results.is_none());
        assert_eq!(MailApp::value(&this.search, cx), "weekend");
        let search = this.server_search.as_ref().expect("asked again");
        assert_eq!(search.query, "weekend");
        assert_eq!(search.account, work);
    });

    // Back from the results returns to where they were opened from.
    act(&restarted, tab(TabAction::Switch(1)), cx);
    cx.run_until_parked();
    act(&restarted, Action::CloseSearch, cx);
    restarted.read_with(cx, |this, _| {
        assert!(this.local_results.is_none());
        assert_eq!(this.account.as_ref(), Some(&work));
        assert_eq!(this.folder, Folder::Inbox);
    });
}

#[gpui::test]
fn ends_of_the_tab_row_with_more_tabs_fade(cx: &mut TestAppContext) {
    let (view, cx, _store) = two_accounts(cx);
    cx.simulate_resize(size(px(900.), px(700.)));
    let work = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    let settle = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        frame(cx);
        frame(cx);
    };
    // Two tabs fit: no fades.
    act(&view, tab(TabAction::New(None)), cx);
    settle(cx);
    assert!(cx.debug_bounds("tabs-earlier").is_none());
    assert!(cx.debug_bounds("tabs-later").is_none());

    // Many don't. The last, just opened, shows whole, past the fade
    // before it.
    for folder in [
        Folder::Sent,
        Folder::Archive,
        Folder::Trash,
        Folder::Junk,
        Folder::Starred,
        Folder::Drafts,
        Folder::All,
    ] {
        act(
            &view,
            tab(TabAction::New(Some((work.clone(), folder.into())))),
            cx,
        );
    }
    settle(cx);
    let last = view.read_with(cx, |this, _| this.tabs.list.len() - 1);
    let row = cx.debug_bounds("tabs").unwrap();
    let shown = cx
        .debug_bounds(Box::leak(format!("tab-{last}").into_boxed_str()))
        .unwrap();
    assert!(
        shown.right() <= row.right() + px(0.5),
        "{shown:?} past {row:?}"
    );
    assert!(cx.debug_bounds("tabs-earlier").is_some());
    assert!(cx.debug_bounds("tabs-later").is_none());

    // The first, opened, goes to the start: the fade moves to the end.
    act(&view, tab(TabAction::Switch(0)), cx);
    settle(cx);
    assert!(cx.debug_bounds("tabs-earlier").is_none());
    assert!(cx.debug_bounds("tabs-later").is_some());

    // The arrow scrolls toward the later tabs, and both ends fade.
    let at = cx.debug_bounds("tabs-later").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
    assert!(cx.debug_bounds("tabs-earlier").is_some());
    view.read_with(cx, |this, _| {
        assert_eq!(this.tabs.active, 0, "the arrow opens no tab")
    });

    // The wheel scrolls the row too: back to the start.
    let at = cx.debug_bounds("tabs").unwrap().center();
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(4000.))),
        ..Default::default()
    });
    settle(cx);
    assert!(cx.debug_bounds("tabs-earlier").is_none());
    assert!(cx.debug_bounds("tabs-later").is_some());
}
