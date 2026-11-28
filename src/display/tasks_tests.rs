//! The Tasks view: its sidebar entry, Today, Upcoming, Overdue and All,
//! lists and tags, the editor, and "Add to Tasks" on a message.
use super::tests::{act, frame, open, store};
use super::*;
use crate::sync::caldav::{Calendar, Server, Stored};
use crate::sync::ical::Event;
use std::prelude::v1::test;

/// A CalDAV server (out of reach: nothing is read) with one calendar and
/// its tasks: one overdue, two today (one a subtask), one coming up, one
/// without a due date.
fn with_tasks() -> Arc<Store> {
    let store = store();
    let mut a = Account::new(Provider::Imap);
    a.email = "ana@example.org".into();
    store.save_account(&a).unwrap();
    let home = "http://127.0.0.1:9/cal/".to_owned();
    store
        .set_preference("calendar_server", &serde_json::to_string(&Server { url: "http://127.0.0.1:9".into(), user: "ana".into(), ..Default::default() }).unwrap())
        .unwrap();
    store
        .set_preference("calendars", &serde_json::to_string(&[Calendar { href: home.clone(), name: "Home".into(), writable: true, ..Default::default() }]).unwrap())
        .unwrap();
    let today = chrono::Local::now().date_naive();
    let day = |offset: i64| (today + chrono::Duration::days(offset)).and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    let task = |uid: &str, start: i64, tags: &[&str], parent: &str, url: &str| Stored {
        event: Event {
            uid: uid.into(),
            summary: uid.into(),
            start,
            end: start,
            all_day: start != 0,
            task: true,
            categories: tags.iter().map(|t| t.to_string()).collect(),
            parent: parent.into(),
            url: url.into(),
            ..Default::default()
        },
        calendar: home.clone(),
        href: format!("{home}{uid}.ics"),
        ..Default::default()
    };
    let tasks = vec![
        task("Rent", day(-2), &["home"], "", "mid:rent@example.org"),
        task("Taxes", day(0), &[], "", ""),
        task("Forms", day(0), &[], "Taxes", ""),
        task("Dentist", day(3), &["health"], "", ""),
        task("Someday", 0, &[], "", ""),
    ];
    store.set_preference("calendar_tasks", &serde_json::to_string(&tasks).unwrap()).unwrap();
    store
}

fn rows(cx: &mut VisualTestContext) -> usize {
    frame(cx);
    (0..20).take_while(|i| cx.debug_bounds(Box::leak(format!("task-row-{i}").into_boxed_str())).is_some()).count()
}

#[gpui::test]
fn the_tasks_view_lists_tasks_by_when_theyre_due(cx: &mut TestAppContext) {
    let (view, cx) = open(with_tasks(), cx);
    frame(cx);
    assert!(cx.debug_bounds("open-tasks").is_some(), "in the sidebar");
    view.read_with(cx, |this, _| assert_eq!(this.tasks_due_now(), 3, "overdue and today"));
    act(&view, Action::Calendar(calendar_views::CalAction::Open(calendar_views::AppView::Tasks)), cx);
    view.read_with(cx, |this, _| assert_eq!(this.app_view, Some(calendar_views::AppView::Tasks)));
    frame(cx);
    assert!(cx.debug_bounds("app-view").is_some());
    // Today: what's overdue, and today's with its subtask.
    assert_eq!(rows(cx), 3);
    act(&view, Action::Tasks(tasks::TaskAction::Filter(tasks::TaskFilter::Upcoming)), cx);
    assert_eq!(rows(cx), 1);
    act(&view, Action::Tasks(tasks::TaskAction::Filter(tasks::TaskFilter::Overdue)), cx);
    assert_eq!(rows(cx), 1);
    assert!(cx.debug_bounds("task-link-0").is_some(), "made from an email: it opens it");
    act(&view, Action::Tasks(tasks::TaskAction::Filter(tasks::TaskFilter::All)), cx);
    assert_eq!(rows(cx), 5);
    // By tag.
    act(&view, Action::Tasks(tasks::TaskAction::Tag(Some("health".into()))), cx);
    assert_eq!(rows(cx), 1);
    act(&view, Action::Tasks(tasks::TaskAction::Tag(None)), cx);
    // Nothing done here yet.
    act(&view, Action::Tasks(tasks::TaskAction::Filter(tasks::TaskFilter::Completed)), cx);
    frame(cx);
    assert!(cx.debug_bounds("tasks-empty").is_some());
    view.update_in(cx, |this, w, cx| this.handle(Action::Calendar(calendar_views::CalAction::Close), w, cx));
    view.read_with(cx, |this, _| assert!(this.app_view.is_none()));
}

#[gpui::test]
fn the_task_editor_opens_on_a_task_and_for_subtasks(cx: &mut TestAppContext) {
    let (view, cx) = open(with_tasks(), cx);
    act(&view, Action::Calendar(calendar_views::CalAction::Open(calendar_views::AppView::Tasks)), cx);
    act(&view, Action::Tasks(tasks::TaskAction::Edit("Rent".into())), cx);
    frame(cx);
    assert!(cx.debug_bounds("tasks-editor").is_some());
    view.read_with(cx, |this, cx| {
        assert_eq!(this.tasks.title.read(cx).value().as_ref(), "Rent");
        assert_eq!(this.tasks.tags.read(cx).value().as_ref(), "home");
        assert_eq!(this.tasks.url, "mid:rent@example.org");
        assert_eq!(this.tasks.editing, Some(tasks::Editing::Task("Rent".into())));
    });
    act(&view, Action::Tasks(tasks::TaskAction::Cancel), cx);
    frame(cx);
    assert!(cx.debug_bounds("tasks-editor").is_none());
    // A subtask: in its parent's list, with nothing filled in.
    act(&view, Action::Tasks(tasks::TaskAction::New(Some("Taxes".into()))), cx);
    view.read_with(cx, |this, cx| {
        assert_eq!(this.tasks.parent, "Taxes");
        assert_eq!(this.tasks.target, "http://127.0.0.1:9/cal/");
        assert!(this.tasks.title.read(cx).value().is_empty());
    });
    // Saved without a name: asked for one.
    act(&view, Action::Tasks(tasks::TaskAction::Save), cx);
    view.read_with(cx, |this, _| assert!(this.tasks.error.is_some() && this.tasks.editing.is_some()));
}

#[gpui::test]
fn messages_offer_add_to_tasks(cx: &mut TestAppContext) {
    let store = with_tasks();
    let a = store.accounts().unwrap().remove(0);
    let m = Mail {
        id: format!("{}:INBOX:1:1", a.id),
        account: a.id.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: 1,
        from: "bo@example.se".into(),
        subject: "Invoice".into(),
        message_id: "<inv@example.se>".into(),
        date: chrono::Utc::now().timestamp(),
        ..Mail::default()
    };
    store.save_messages(std::slice::from_ref(&m)).unwrap();
    let (view, cx) = open(store, cx);
    view.read_with(cx, |this, _| {
        let offered = this.mail_entries(&m.id).iter().any(|e| {
            matches!(e, Entry::Item { action: Action::Tasks(tasks::TaskAction::FromMail(id, None)), .. } if *id == m.id)
        });
        assert!(offered, "Add to Tasks is in the message's menu");
    });
}
