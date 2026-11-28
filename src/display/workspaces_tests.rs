//! Workspaces in the window: making one and narrowing it on its page, the
//! sidebar and All Accounts narrowed to its accounts, leaving a mailbox it
//! hides, what's announced, switching (by hand, by keyboard, back to
//! everything), templates and schedules, address books, deleting, and where
//! new messages come from.
use super::tests::{act, frame, open, store};
use super::workspaces::{Preset, Template, WsAction};
use super::*;
use crate::manage::workspaces::{Alerts, Workspaces};
use std::prelude::v1::test;

/// A store with two accounts, Work and Home, each with the sample inbox.
fn two_accounts() -> (Arc<Store>, String, String) {
    let store = store();
    let mut ids = vec![];
    for (name, email) in [("Work", "work@example.com"), ("Home", "home@example.com")] {
        let mut a = Account::new(Provider::Imap);
        a.name = name.into();
        a.email = email.into();
        store.save_account(&a).unwrap();
        let mut mails = demo_mail();
        for m in &mut mails {
            m.account = a.id.clone();
            m.id = format!("{}:{}", a.id, m.id);
        }
        store.save_messages(&mails).unwrap();
        ids.push(a.id);
    }
    let home = ids.pop().unwrap();
    let work = ids.pop().unwrap();
    (store, work, home)
}

fn shows(cx: &mut VisualTestContext, selector: String) -> bool {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    cx.debug_bounds(selector).is_some()
}

fn only(ids: &[&String]) -> Option<std::collections::BTreeSet<String>> {
    Some(ids.iter().map(|i| (*i).clone()).collect())
}

/// The workspace whose page is open.
fn open_page(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |this, _| match &this.page {
        Page::Workspace(id) => id.clone(),
        other => panic!("not a workspace's page: {other:?}"),
    })
}

#[gpui::test]
fn a_workspace_narrows_the_sidebar_all_accounts_and_notifications(cx: &mut TestAppContext) {
    let (store, work, home) = two_accounts();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    frame(cx);
    // No workspaces yet: no switcher, both accounts and All Accounts.
    assert!(cx.debug_bounds("workspace-switcher").is_none());
    assert!(shows(cx, format!("account-{work}")) && shows(cx, format!("account-{home}")));
    view.read_with(cx, |this, _| assert!(this.shows_all_mail()));

    // A new workspace opens its page, turned on, showing every account.
    act(
        &view,
        Action::Workspaces(WsAction::New(Template::Blank)),
        cx,
    );
    let ws = open_page(&view, cx);
    view.read_with(cx, |this, _| {
        let w = this.workspace().expect("the new workspace is on");
        assert_eq!(w.id, ws);
        assert!(w.accounts.is_none());
    });
    frame(cx);
    assert!(cx.debug_bounds("workspace-name").is_some());
    assert!(
        !shows(cx, format!("workspace-account-{home}")),
        "every account: nothing to tick"
    );
    // Only these: every account ticked, to narrow.
    act(
        &view,
        Action::Workspaces(WsAction::EveryAccount(ws.clone(), false)),
        cx,
    );
    frame(cx);
    assert!(shows(cx, format!("workspace-account-{home}")));
    act(
        &view,
        Action::Workspaces(WsAction::ToggleAccount(ws.clone(), home.clone())),
        cx,
    );
    // The last account stays.
    act(
        &view,
        Action::Workspaces(WsAction::ToggleAccount(ws.clone(), work.clone())),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.workspace().unwrap().accounts, only(&[&work]));
        assert!(!this.shows_all_mail(), "one account: no All Accounts");
        assert_eq!(this.account.as_deref(), Some(work.as_str()));
    });
    // Its name is saved as it's typed.
    view.update_in(cx, |this, w, cx| {
        let name = this.ws_ui.name.clone();
        MailApp::set(&name, "Office".to_owned(), w, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(this.workspace().unwrap().name, "Office")
    });
    // Back to the list, which opens the page again.
    act(&view, Action::Workspaces(WsAction::Back), cx);
    view.read_with(cx, |this, _| assert_eq!(this.page, Page::Workspaces));
    frame(cx);
    assert!(cx.debug_bounds("workspace-row-0").is_some());
    assert!(cx.debug_bounds("workspace-everything").is_some());

    act(&view, Action::Close, cx);
    view.read_with(cx, |this, _| assert!(this.modal.is_none()));
    // The switcher's All Workspaces opens Settings at the list.
    act(&view, Action::Workspaces(WsAction::ShowAll), cx);
    view.read_with(cx, |this, _| {
        assert!(this.modal == Some(Modal::Settings));
        assert_eq!(this.page, Page::Workspaces);
    });
    act(&view, Action::Close, cx);
    frame(cx);
    assert!(cx.debug_bounds("workspace-switcher").is_some());
    assert!(shows(cx, format!("account-{work}")));
    assert!(!shows(cx, format!("account-{home}")), "Home is hidden");
    // Saved for the background monitor, the widgets and the next launch.
    assert_eq!(
        Workspaces::load(&store).current().map(|w| w.name.clone()),
        Some("Office".to_owned())
    );

    // Only Work's mail is announced; Home's is again with Everything.
    view.update(cx, |this, _| {
        let of = |account: &str| Mail {
            account: account.to_owned(),
            ..this.messages[0].clone()
        };
        assert!(this.announces(&of(&work)));
        assert!(!this.announces(&of(&home)));
    });
    act(
        &view,
        Action::Workspaces(WsAction::SetAlerts(ws.clone(), Alerts::Everything)),
        cx,
    );
    view.read_with(cx, |this, _| assert!(this.workspace_announces(&home)));

    // Back to everything: both accounts and All Accounts again.
    act(&view, Action::Workspaces(WsAction::Switch(None)), cx);
    frame(cx);
    assert!(shows(cx, format!("account-{home}")));
    view.read_with(cx, |this, _| {
        assert!(this.workspace().is_none() && this.shows_all_mail())
    });

    // Home open, then the workspace on (as from the tray): Home is left.
    act(
        &view,
        Action::OpenMailbox(home.clone(), Folder::Inbox.into()),
        cx,
    );
    view.update(cx, |this, cx| this.switch_workspace(Some(ws.clone()), cx));
    view.read_with(cx, |this, _| {
        assert_eq!(this.ws_ui.applied.as_deref(), Some(ws.as_str()));
        assert_eq!(this.account.as_deref(), Some(work.as_str()));
    });
    // Cycling on wraps around to everything.
    act(&view, Action::Workspaces(WsAction::Cycle(true)), cx);
    view.read_with(cx, |this, _| assert!(this.ws_ui.applied.is_none()));

    // Deleting asks first.
    act(&view, Action::Workspaces(WsAction::Open(ws.clone())), cx);
    act(&view, Action::Workspaces(WsAction::Delete(ws.clone())), cx);
    frame(cx);
    assert!(cx.debug_bounds("workspace-delete-confirm").is_some());
    act(&view, Action::Workspaces(WsAction::CancelDelete), cx);
    view.read_with(cx, |this, _| assert!(this.workspaces.get(&ws).is_some()));
    act(&view, Action::Workspaces(WsAction::Delete(ws.clone())), cx);
    act(
        &view,
        Action::Workspaces(WsAction::ConfirmDelete(ws.clone())),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(this.workspaces.list.is_empty());
        assert_eq!(this.page, Page::Workspaces, "back to the list");
    });
}

#[gpui::test]
fn templates_and_schedules(cx: &mut TestAppContext) {
    let (store, work, home) = two_accounts();
    let (view, cx) = open(store, cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    // Work: the account open, written from, on weekdays from 9 to 5.
    act(&view, Action::Workspaces(WsAction::New(Template::Work)), cx);
    let office = open_page(&view, cx);
    view.read_with(cx, |this, _| {
        let w = this.workspaces.get(&office).unwrap();
        assert_eq!(w.accounts, only(&[&work]));
        assert_eq!(w.compose_from.as_deref(), Some(work.as_str()));
        assert_eq!(w.schedule, vec![Preset::WorkHours.window()]);
    });
    frame(cx);
    assert!(cx.debug_bounds("workspace-window-0").is_some());
    // Personal: every other account; Focus: the one open, silent.
    act(
        &view,
        Action::Workspaces(WsAction::New(Template::Personal)),
        cx,
    );
    let personal = open_page(&view, cx);
    act(
        &view,
        Action::Workspaces(WsAction::New(Template::Focus)),
        cx,
    );
    let focus = open_page(&view, cx);
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.workspaces.get(&personal).unwrap().accounts,
            only(&[&home])
        );
        let f = this.workspaces.get(&focus).unwrap();
        assert_eq!(f.alerts, Alerts::Nothing);
        // Personal, on, left Work's mailbox for Home's: the account open.
        assert_eq!(f.accounts, only(&[&home]));
    });

    // A preset added once; a time of one's own typed in.
    let ws = |a: WsAction| Action::Workspaces(a);
    act(
        &view,
        ws(WsAction::AddPreset(focus.clone(), Preset::Evenings)),
        cx,
    );
    act(
        &view,
        ws(WsAction::AddPreset(focus.clone(), Preset::Evenings)),
        cx,
    );
    act(&view, ws(WsAction::ToggleCustom), cx);
    frame(cx);
    assert!(cx.debug_bounds("workspace-add-window").is_some());
    view.update_in(cx, |this, w, cx| {
        let (start, end) = (this.ws_ui.start.clone(), this.ws_ui.end.clone());
        MailApp::set(&start, "7:30".to_owned(), w, cx);
        MailApp::set(&end, "8:15".to_owned(), w, cx);
    });
    act(
        &view,
        ws(WsAction::NewDays(crate::manage::inbox_control::WEEKEND)),
        cx,
    );
    act(&view, ws(WsAction::AddWindow(focus.clone())), cx);
    view.read_with(cx, |this, _| {
        let f = this.workspaces.get(&focus).unwrap();
        assert_eq!(f.schedule.len(), 2, "the preset only once");
        assert_eq!((f.schedule[1].start, f.schedule[1].end), (450, 495));
        assert!(!this.ws_ui.custom, "the fields close once it's added");
    });
    act(&view, ws(WsAction::ToggleDay(focus.clone(), 1, 5)), cx);
    act(&view, ws(WsAction::RemoveWindow(focus.clone(), 0)), cx);
    view.read_with(cx, |this, _| {
        let f = this.workspaces.get(&focus).unwrap();
        assert_eq!(f.schedule.len(), 1);
        assert_eq!(f.schedule[0].days, 1 << 6, "Saturday taken out");
    });
    // Reordered: the first listed wins a time two schedules cover.
    act(&view, ws(WsAction::Shift(focus.clone(), -5)), cx);
    view.read_with(cx, |this, _| assert_eq!(this.workspaces.list[0].id, focus));
}

#[gpui::test]
fn all_accounts_address_books_and_writing_from_a_workspace(cx: &mut TestAppContext) {
    let (store, work, home) = two_accounts();
    // A third account the workspace leaves out.
    let mut other = Account::new(Provider::Imap);
    other.name = "Other".into();
    other.email = "other@example.com".into();
    store.save_account(&other).unwrap();
    let mut ws = Workspaces::default();
    let id = ws.add("Both");
    {
        let w = ws.get_mut(&id).unwrap();
        w.accounts = only(&[&work, &home]);
        w.compose_from = Some(home.clone());
        // The first address book only.
        w.contacts = Some([String::new()].into());
    }
    ws.choose(Some(id.clone()), chrono::Utc::now().timestamp());
    ws.save(&store);
    store.flush();
    // Two address books: one card on this computer, one in another source.
    let backend = Backend::new(store.clone());
    let dir = std::env::temp_dir().join(format!("picacho-ws-cards-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let card = |name: &str, email: &str| {
        let path = dir.join(format!("{name}.vcf"));
        std::fs::write(
            &path,
            format!("BEGIN:VCARD\nFN:{name}\nEMAIL:{email}\nUID:{name}\nEND:VCARD\n"),
        )
        .unwrap();
        path
    };
    backend
        .import_contacts(&[card("Ana", "ana@work.example")])
        .unwrap();
    backend
        .add_contacts_source(|| backend.import_contacts(&[card("Bo", "bo@home.example")]))
        .unwrap();
    drop(backend);

    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(ALL_MAIL.into(), Folder::Inbox.into()),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.ws_ui.applied.as_deref(), Some(id.as_str()));
        assert!(this.shows_all_mail());
        let mut scope = this.scope.accounts.clone();
        scope.sort();
        let mut both = vec![work.clone(), home.clone()];
        both.sort();
        assert_eq!(scope, both, "All Accounts draws on the workspace's");
        assert!(this.messages.iter().all(|m| m.account != other.id));
        // New messages in All Accounts come from the chosen account.
        assert_eq!(this.workspace_from().map(|a| a.id), Some(home.clone()));
        // Contacts and suggestions: the first address book's.
        let names: Vec<String> = this
            .workspace_contacts()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["Ana"]);
        assert!(this.backend.contacts().len() == 2);
    });
    let suggested = |view: &Entity<MailApp>, cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| {
            this.contacts().iter().any(|(_, e)| e == "bo@home.example")
        })
    };
    assert!(!suggested(&view, cx));
    act(
        &view,
        Action::Workspaces(WsAction::EveryContacts(id.clone(), true)),
        cx,
    );
    assert!(suggested(&view, cx), "every address book again");
    // Its page lists both address books, once narrowed.
    act(&view, Action::Workspaces(WsAction::Open(id.clone())), cx);
    act(
        &view,
        Action::Workspaces(WsAction::EveryContacts(id.clone(), false)),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("workspace-contacts-first").is_some());
    // In one account's mailbox, that account decides.
    act(&view, Action::Close, cx);
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    view.read_with(cx, |this, _| assert!(this.workspace_from().is_none()));
    // Home taken out: no longer written from.
    act(
        &view,
        Action::Workspaces(WsAction::ToggleAccount(id.clone(), home.clone())),
        cx,
    );
    view.read_with(cx, |this, _| {
        let w = this.workspace().unwrap();
        assert_eq!(w.accounts, only(&[&work]));
        assert!(w.compose_from.is_none());
        assert!(!this.shows_all_mail());
    });
    let _ = std::fs::remove_dir_all(dir);
}
