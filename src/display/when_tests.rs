//! Reading typed dates and times (Local AI), with the trained model copied
//! in where installing puts it (tests don't download). The model runs with
//! Picacho's own runtime everywhere, so these run on macOS too.
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::local_ai;
use std::prelude::v1::test;

/// A store with one IMAP account and a message in its inbox.
fn mailbox() -> Arc<Store> {
    let store = store();
    let mut a = Account::new(Provider::Imap);
    a.name = "Work".into();
    a.email = "work@example.com".into();
    store.save_account(&a).unwrap();
    store
        .save_messages(&[Mail {
            id: format!("{}:INBOX:1:1", a.id),
            account: a.id.clone(),
            folder: Folder::Inbox,
            remote_folder: "INBOX".into(),
            uid: 1,
            from: "jo@acme.test".into(),
            sender: "Jo".into(),
            to: "work@example.com".into(),
            subject: "Lunch?".into(),
            snippet: "Are you free this week?".into(),
            body: Some("Are you free this week?".into()),
            date: chrono::Utc::now().timestamp() - 60,
            unread: true,
            message_id: "<1@acme.test>".into(),
            ..Mail::default()
        }])
        .unwrap();
    store
}

/// The date and time model, where installing puts it.
fn install_when(store: &Store) {
    let dir = local_ai::models_dir(store);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(local_ai::test_model("tbs-v1.bin"), dir.join("tbs-v1.bin")).unwrap();
}

#[gpui::test]
fn typed_dates_are_off_until_turned_on_and_need_local_ai(cx: &mut TestAppContext) {
    let store = mailbox();
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.understand_dates && this.when.is_none())
    });
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::LocalAi), cx);
    frame(cx);
    assert!(cx.debug_bounds("local-ai-when").is_some());
    assert!(cx.debug_bounds("model-tbs").is_some());
    act(&view, Action::SetUnderstandDates(true), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.understand_dates && !this.when_busy)
    });
    assert!(!local_ai::models_dir(&store).exists());
}

#[gpui::test]
fn a_typed_time_sets_the_snooze_picker_and_the_model_goes_when_turned_off(cx: &mut TestAppContext) {
    let store = mailbox();
    install_when(&store);
    let (view, cx) = open(store.clone(), cx);
    // Off: the picker has no field to type in.
    let id = view.read_with(cx, |this, _| this.messages[0].id.clone());
    act(&view, Action::SnoozeMenu(vec![id.clone()]), cx);
    view.read_with(cx, |this, cx| {
        assert!(!this.picker.as_ref().unwrap().read(cx).can_type())
    });
    act(&view, Action::Close, cx);

    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetUnderstandDates(true), cx);
    view.read_with(cx, |this, _| {
        assert!(this.understand_dates && this.when.is_some() && this.when_error.is_none())
    });
    assert_eq!(
        store.preference(local_ai::WHEN_PREFERENCE).as_deref(),
        Some("true")
    );
    act(&view, Action::SnoozeMenu(vec![id.clone()]), cx);
    frame(cx);
    assert!(cx.debug_bounds("snooze-typed").is_some());
    let picker = view.read_with(cx, |this, _| this.picker.clone().unwrap());
    picker.update_in(cx, |p, w, cx| p.type_time("tomorow at 6pm pls", w, cx));
    frame(cx);
    assert!(cx.debug_bounds("snooze-typed-hint").is_some());
    let want = {
        use chrono::TimeZone;
        let day = chrono::Local::now().date_naive() + chrono::Days::new(1);
        chrono::Local
            .from_local_datetime(&day.and_hms_opt(18, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp()
    };
    picker.read_with(cx, |p, _| assert_eq!(p.chosen(), Some(want)));
    // Text without a time leaves the choice as it was.
    picker.update_in(cx, |p, w, cx| p.type_time("hello there", w, cx));
    picker.read_with(cx, |p, _| assert_eq!(p.chosen(), Some(want)));
    act(&view, Action::Close, cx);

    // Turning the feature off deletes the model and removes the field.
    act(&view, Action::SetUnderstandDates(false), cx);
    view.read_with(cx, |this, _| assert!(this.local_ai && this.when.is_none()));
    assert!(!local_ai::WHEN.is_installed(&store));
    assert_eq!(
        store.preference(local_ai::WHEN_PREFERENCE).as_deref(),
        Some("false")
    );
}

#[gpui::test]
fn the_date_model_loads_at_launch_and_deleting_it_turns_its_feature_off(cx: &mut TestAppContext) {
    let store = mailbox();
    install_when(&store);
    store
        .set_preference(local_ai::ENABLED_PREFERENCE, "true")
        .unwrap();
    store
        .set_preference(local_ai::WHEN_PREFERENCE, "true")
        .unwrap();
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.when.is_some(), "loaded at launch");
        assert!(this.otp.is_none(), "code hiding stays off");
    });
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::LocalAi), cx);
    frame(cx);
    assert!(cx.debug_bounds("model-delete-tbs").is_some());
    act(&view, Action::AskDeleteModel("tbs".into()), cx);
    act(&view, Action::DeleteModel("tbs".into()), cx);
    view.read_with(cx, |this, _| {
        assert!(this.local_ai && !this.understand_dates && this.when.is_none())
    });
    assert!(!local_ai::WHEN.is_installed(&store));
    // Turning Local AI off turns the feature off too.
    install_when(&store);
    act(&view, Action::SetUnderstandDates(true), cx);
    view.read_with(cx, |this, _| assert!(this.when.is_some()));
    act(&view, Action::SetLocalAi(false), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.understand_dates && this.when.is_none())
    });
    assert!(!local_ai::models_dir(&store).exists());
}
