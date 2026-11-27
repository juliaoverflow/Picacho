//! The Waiting For view, its sidebar entry, and the banner above sent mail
//! waited on.
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::backend::FollowUp;
use std::prelude::v1::test;

/// Two accounts; Work sent three messages, and waits on a reply to two of
/// them: one not due yet, one overdue.
fn waiting(cx: &mut TestAppContext) -> (Entity<MailApp>, &mut VisualTestContext, Arc<Store>, String, String) {
    let store = store();
    let mut ids = Vec::new();
    for (name, email) in [("Work", "work@example.com"), ("Home", "home@example.com")] {
        let mut a = Account::new(Provider::Imap);
        a.name = name.into();
        a.email = email.into();
        store.save_account(&a).unwrap();
        ids.push(a.id.clone());
    }
    let (work, home) = (ids[0].clone(), ids[1].clone());
    let now = chrono::Utc::now().timestamp();
    let sent: Vec<Mail> = (1..=3)
        .map(|i| Mail {
            id: format!("{work}:Sent:1:{i}"),
            account: work.clone(),
            folder: Folder::Sent,
            remote_folder: "Sent".into(),
            uid: i,
            from: "work@example.com".into(),
            sender: "Work".into(),
            to: "ana@example.org".into(),
            subject: format!("Question {i}"),
            snippet: "Any news?".into(),
            date: now - i as i64 * 86400,
            message_id: format!("<q{i}@work>"),
            ..Mail::default()
        })
        .collect();
    store.save_messages(&sent).unwrap();
    let backend = Backend::new(store.clone());
    for (i, due) in [(1, now + 86400), (2, now - 3600)] {
        backend
            .note_follow_up(
                &work,
                FollowUp {
                    message_id: format!("<q{i}@work>"),
                    subject: format!("Question {i}"),
                    to: "ana@example.org".into(),
                    sent: now - i as i64 * 86400,
                    due,
                    reminded: due < now,
                    changed: 0,
                },
            )
            .unwrap();
    }
    let (view, cx) = open(store.clone(), cx);
    (view, cx, store, work, home)
}

fn listed(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |this, _| {
        this.visible.iter().map(|&i| this.messages[i].message_id.clone()).collect()
    })
}

#[gpui::test]
fn waiting_for_lists_sent_mail_waited_on_until_it_is_answered(cx: &mut TestAppContext) {
    let (view, cx, store, work, home) = waiting(cx);
    frame(cx);
    // Only the account waiting on something shows Waiting For.
    let work_nav = format!("nav-{work}-waiting");
    let home_nav = format!("nav-{home}-waiting");
    assert!(cx.debug_bounds(Box::leak(work_nav.into_boxed_str())).is_some());
    assert!(cx.debug_bounds(Box::leak(home_nav.into_boxed_str())).is_none());
    view.read_with(cx, |this, _| assert_eq!(this.waiting_counts(&work), (2, 1)));

    act(&view, Action::OpenWaiting(work.clone()), cx);
    let mut shown = listed(&view, cx);
    shown.sort();
    assert_eq!(shown, ["<q1@work>", "<q2@work>"]);
    view.read_with(cx, |this, _| {
        assert!(this.waiting_open);
        assert_eq!(this.mailbox_label(), "Waiting For");
    });

    // The overdue one: a banner offering to follow up.
    view.update_in(cx, |this, _, cx| {
        this.selected = Some(format!("{work}:Sent:1:2"));
        cx.notify();
    });
    frame(cx);
    frame(cx);
    assert!(cx.debug_bounds("follow-up-banner").is_some());
    assert!(cx.debug_bounds("follow-up-reply").is_some());
    assert!(cx.debug_bounds("stop-waiting").is_some());

    // Stopping: it leaves the list.
    act(&view, Action::StopWaiting(work.clone(), "<q2@work>".into()), cx);
    assert_eq!(listed(&view, cx), ["<q1@work>"]);

    // Opened again where it was.
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert!(this.waiting_open);
        assert_eq!(this.account.as_ref(), Some(&work));
    });
    assert_eq!(listed(&restarted, cx), ["<q1@work>"]);

    // Ana answers: no longer waited on, and the list says so.
    let mut reply = Mail {
        id: format!("{work}:INBOX:1:99"),
        account: work.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: 99,
        from: "ana@example.org".into(),
        sender: "Ana".into(),
        to: "work@example.com".into(),
        subject: "Re: Question 1".into(),
        date: chrono::Utc::now().timestamp(),
        message_id: "<a1@ana>".into(),
        ..Mail::default()
    };
    reply.set_references("<q1@work>");
    store.save_messages(&[reply]).unwrap();
    restarted.update_in(cx, |this, _, cx| this.recheck_follow_ups(cx));
    cx.run_until_parked();
    assert!(listed(&restarted, cx).is_empty());
    restarted.read_with(cx, |this, _| {
        assert!(this.follow_ups.get(&work).is_none_or(|l| l.is_empty()));
        assert_eq!(this.status, "Ana replied to “Question 1”");
    });
    frame(cx);
    assert!(cx.debug_bounds("empty-list-title").is_some());
}

#[gpui::test]
fn sent_mail_can_be_waited_on_from_its_menu(cx: &mut TestAppContext) {
    let (view, cx, _store, work, _) = waiting(cx);
    act(&view, Action::OpenMailbox(work.clone(), Folder::Sent.into()), cx);
    let id = format!("{work}:Sent:1:3");
    view.read_with(cx, |this, _| {
        assert!(!this.waiting_open);
        assert!(this.messages.iter().any(|m| m.id == id), "Sent lists its mail");
    });
    act(&view, Action::FollowUpMenu(id.clone()), cx);
    view.read_with(cx, |this, _| assert!(this.modal == Some(Modal::FollowUp(id.clone()))));
    // As the picker would on a choice.
    let at = chrono::Utc::now().timestamp() + 2 * 86400;
    view.update_in(cx, |this, _, cx| {
        this.modal = None;
        this.wait_for_reply(id.clone(), at, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let m = this.messages.iter().find(|m| m.id == id).unwrap();
        assert_eq!(this.follow_up_of(m).map(|f| f.due), Some(at));
        assert_eq!(this.waiting_counts(&work), (3, 1));
    });
}
