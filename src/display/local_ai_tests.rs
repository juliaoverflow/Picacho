//! Local AI in the app, with the trained model copied in where installing
//! puts it (tests don't download). Core ML isn't available here, so these
//! run with Picacho's own runtime.
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::local_ai;
use std::prelude::v1::test;

/// The replies suggested for `m`, the model having read it off the
/// window's thread as drawing starts it.
fn replies_for(view: &Entity<MailApp>, m: &Mail, cx: &mut VisualTestContext) -> Rc<local_ai::replies::Suggestions> {
    view.update(cx, |this, cx| {
        this.smart_replies_for(m, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| this.smart_replies(m))
}

/// The parcels the reader finds in `m`, the model (when on) having read it
/// off the window's thread as drawing starts it.
fn parcels_for(view: &Entity<MailApp>, m: &Mail, cx: &mut VisualTestContext) -> Rc<Vec<crate::manage::parcels::Parcel>> {
    view.update(cx, |this, cx| {
        this.found_parcels_for(m, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| this.found_parcels(m))
}

/// A store with one account whose inbox has a sign-in code and an order.
fn mailbox() -> (Arc<Store>, String) {
    let store = store();
    let mut a = Account::new(Provider::Imap);
    a.name = "Work".into();
    a.email = "work@example.com".into();
    store.save_account(&a).unwrap();
    let mail = |n: u32, subject: &str, body: &str| Mail {
        id: format!("{}:INBOX:1:{n}", a.id),
        account: a.id.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: n,
        from: "no-reply@acme.test".into(),
        sender: "Acme".into(),
        to: "work@example.com".into(),
        subject: subject.into(),
        snippet: body.replace('\n', " "),
        body: Some(body.into()),
        date: chrono::Utc::now().timestamp() - i64::from(n) * 60,
        unread: true,
        message_id: format!("<{n}@acme.test>"),
        ..Mail::default()
    };
    store
        .save_messages(&[
            mail(
                1,
                "482913 is your Acme verification code",
                "Hi,\n\nYour verification code is 482913.\nIt expires in 10 minutes.",
            ),
            mail(
                2,
                "Your order #839201 has shipped",
                "Order #839201 is on its way. Total: $45.10",
            ),
        ])
        .unwrap();
    (store, a.id)
}

fn install(store: &Store) {
    let dir = local_ai::models_dir(store);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(local_ai::test_model("otp-v1.bin"), dir.join("otp-v1.bin")).unwrap();
}

#[gpui::test]
fn local_ai_is_off_and_has_nothing_installed_by_default(cx: &mut TestAppContext) {
    let (store, _) = mailbox();
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.local_ai && !this.hide_codes && this.otp.is_none());
        let m = this
            .messages
            .iter()
            .find(|m| m.subject.contains("482913"))
            .unwrap();
        assert_eq!(this.hide_codes(m, &m.subject), m.subject);
    });
    assert!(!local_ai::models_dir(&store).exists());
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::LocalAi), cx);
    frame(cx);
    assert!(cx.debug_bounds("local-ai").is_some());
    assert!(cx.debug_bounds("local-ai-otp").is_some());
    act(&view, Action::Close, cx);
    // Code hiding needs Local AI on first.
    act(&view, Action::SetHideCodes(true), cx);
    view.read_with(cx, |this, _| assert!(!this.hide_codes && !this.model_busy));
}

#[gpui::test]
fn codes_are_hidden_until_shown_and_the_model_goes_when_turned_off(cx: &mut TestAppContext) {
    let (store, _) = mailbox();
    // Installed earlier: turning the feature on loads it rather than
    // downloading.
    install(&store);
    let (view, cx) = open(store.clone(), cx);
    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetHideCodes(true), cx);
    let id = view.read_with(cx, |this, _| {
        assert!(this.hide_codes && this.otp.is_some() && this.model_error.is_none());
        let code = this
            .messages
            .iter()
            .find(|m| m.subject.contains("482913"))
            .unwrap();
        assert_eq!(
            this.hide_codes(code, &code.subject),
            "•••••• is your Acme verification code"
        );
        // The list holds summaries; the reader has the body.
        assert!(this.hide_codes(code, &code.snippet).contains("is ••••••."));
        let full = this.backend.store.message(&code.id).unwrap().unwrap();
        assert_eq!(
            this.hide_codes(&full, full.body.as_deref().unwrap()),
            "Hi,\n\nYour verification code is ••••••.\nIt expires in 10 minutes."
        );
        let order = this
            .messages
            .iter()
            .find(|m| m.subject.contains("839201"))
            .unwrap();
        assert_eq!(this.hide_codes(order, &order.subject), order.subject);
        code.id.clone()
    });
    assert_eq!(
        store.preference(local_ai::OTP_PREFERENCE).as_deref(),
        Some("true")
    );

    // Opened, it says the code is hidden and offers to show it.
    act(&view, Action::Select(id.clone()), cx);
    frame(cx);
    assert!(cx.debug_bounds("codes-banner").is_some());
    // Copied as typed, while it stays hidden.
    act(&view, Action::CopyCode(id.clone()), cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("482913")
    );
    view.read_with(cx, |this, _| assert!(!this.codes_shown.contains(&id)));
    act(&view, Action::ShowCodes(id.clone(), true), cx);
    view.read_with(cx, |this, _| {
        let m = this.messages.iter().find(|m| m.id == id).unwrap();
        assert_eq!(this.hide_codes(m, &m.subject), m.subject);
    });
    act(&view, Action::ShowCodes(id.clone(), false), cx);
    view.read_with(cx, |this, _| {
        let m = this.messages.iter().find(|m| m.id == id).unwrap();
        assert!(this.hide_codes(m, &m.subject).contains('•'));
    });

    // Turning Local AI off turns the feature off and deletes the model.
    act(&view, Action::SetLocalAi(false), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.local_ai && !this.hide_codes && this.otp.is_none());
        let m = this.messages.iter().find(|m| m.id == id).unwrap();
        assert_eq!(this.hide_codes(m, &m.subject), m.subject);
        // Nothing found, so no banner either.
        assert!(this.found_codes(m).is_empty());
    });
    assert!(!local_ai::models_dir(&store).exists());
    assert_eq!(
        store.preference(local_ai::OTP_PREFERENCE).as_deref(),
        Some("false")
    );
}

#[gpui::test]
fn turning_code_hiding_off_deletes_its_model_and_it_loads_again_at_launch(cx: &mut TestAppContext) {
    let (store, _) = mailbox();
    install(&store);
    store
        .set_preference(local_ai::ENABLED_PREFERENCE, "true")
        .unwrap();
    store
        .set_preference(local_ai::OTP_PREFERENCE, "true")
        .unwrap();
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.otp.is_some(), "loaded at launch")
    });
    act(&view, Action::SetHideCodes(false), cx);
    view.read_with(cx, |this, _| assert!(this.local_ai && this.otp.is_none()));
    assert!(!local_ai::OTP.is_installed(&store));
}

#[gpui::test]
fn the_models_list_shows_progress_and_deleting_asks_which_features_stop(cx: &mut TestAppContext) {
    let (store, _) = mailbox();
    let (view, cx) = open(store.clone(), cx);
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::LocalAi), cx);
    act(&view, Action::SetLocalAi(true), cx);
    frame(cx);
    // Listed before it is installed, with nothing to delete.
    assert!(cx.debug_bounds("model-otp").is_some());
    assert!(cx.debug_bounds("model-delete-otp").is_none());

    // Downloading: a ring shows how far it has got.
    view.update(cx, |this, cx| {
        this.model_busy = true;
        this.model_progress
            .store(local_ai::OTP.size / 2, std::sync::atomic::Ordering::Relaxed);
        assert!((this.model_fraction(&local_ai::OTP) - 0.5).abs() < 0.01);
        cx.notify();
    });
    frame(cx);
    assert!(cx.debug_bounds("model-progress").is_some());
    view.update(cx, |this, _| this.model_busy = false);

    // Installed and in use: it can be deleted, after saying what stops.
    install(&store);
    act(&view, Action::SetHideCodes(true), cx);
    view.read_with(cx, |this, _| assert!(this.otp.is_some()));
    frame(cx);
    assert!(cx.debug_bounds("model-delete-otp").is_some());
    act(&view, Action::AskDeleteModel("otp".into()), cx);
    frame(cx);
    assert!(cx.debug_bounds("delete-model-sheet").is_some());
    view.read_with(cx, |this, _| {
        assert_eq!(this.model_confirm.as_deref(), Some("otp"));
        assert!(this.modal == Some(Modal::Settings), "Settings stays open");
    });
    // Escape keeps the model.
    act(&view, Action::Close, cx);
    view.read_with(cx, |this, _| {
        assert!(this.model_confirm.is_none() && this.modal == Some(Modal::Settings))
    });
    assert!(local_ai::OTP.is_installed(&store));
    // Like the app's other dialogs, its header has a close button.
    act(&view, Action::AskDeleteModel("otp".into()), cx);
    frame(cx);
    let close = cx.debug_bounds("delete-model-sheet-close").expect("a close button");
    cx.simulate_click(close.center(), Modifiers::none());
    view.read_with(cx, |this, _| {
        assert!(this.model_confirm.is_none() && this.modal == Some(Modal::Settings))
    });
    assert!(local_ai::OTP.is_installed(&store));

    act(&view, Action::AskDeleteModel("otp".into()), cx);
    act(&view, Action::DeleteModel("otp".into()), cx);
    view.read_with(cx, |this, _| {
        assert!(this.model_confirm.is_none());
        assert!(this.local_ai, "Local AI itself stays on");
        assert!(!this.hide_codes && this.otp.is_none(), "its feature is off");
    });
    assert!(!local_ai::OTP.is_installed(&store));
}

/// Turning on Find tracking numbers installs the tracking number model from
/// its training directory (it isn't published yet), and the reader then
/// takes a tracking number given without a shipping word beside it.
#[gpui::test]
fn finding_tracking_numbers_installs_the_unpublished_model_from_its_training_directory(
    cx: &mut TestAppContext,
) {
    let (store, account) = mailbox();
    let parcel = |n: u32, body: &str| Mail {
        id: format!("{account}:INBOX:1:{n}"),
        account: account.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: n,
        from: "orders@shop.test".into(),
        sender: "Shop".into(),
        subject: "A note from Shop".into(),
        snippet: body.into(),
        body: Some(body.into()),
        date: chrono::Utc::now().timestamp(),
        ..Mail::default()
    };
    // A bare 12-digit number: the rule without the model doesn't take it.
    let m = parcel(
        3,
        "Hi Sam, use 986578788855 to follow your parcel on the FedEx website.",
    );
    store.save_messages(std::slice::from_ref(&m)).unwrap();
    // The file the trained model is installed from must be the expected one.
    local_ai::test_model("parcel-v1.bin");
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| assert!(this.found_parcels(&m).is_empty()));
    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetFindParcels(true), cx);
    view.read_with(cx, |this, _| {
        assert!(this.parcel_error.is_none(), "{:?}", this.parcel_error);
        assert!(this.find_parcels && this.parcel_model.is_some());
        // The model reads the message off the window's thread: nothing yet.
        assert!(this.found_parcels(&m).is_empty());
    });
    let found: Vec<String> = parcels_for(&view, &m, cx).iter().map(|p| p.tracking_number.clone()).collect();
    assert_eq!(found, ["986578788855"]);
    assert!(local_ai::PARCEL.is_installed(&store));
    assert_eq!(
        store.preference(local_ai::PARCEL_PREFERENCE).as_deref(),
        Some("true")
    );
    // Off again: the model is deleted and the rule is back.
    act(&view, Action::SetFindParcels(false), cx);
    view.read_with(cx, |this, _| {
        assert!(this.parcel_model.is_none());
        assert!(this.found_parcels(&m).is_empty());
    });
    assert!(!local_ai::PARCEL.is_installed(&store));
}

#[gpui::test]
fn suggested_replies_are_worded_in_the_message_language_and_learn(cx: &mut TestAppContext) {
    // The file the model is installed from must be the published one.
    local_ai::test_model("replies-v1.bin");
    let store = store();
    let mut a = Account::new(Provider::Imap);
    a.name = "Work".into();
    a.email = "work@example.com".into();
    store.save_account(&a).unwrap();
    let mail = |n: u32, from: &str, subject: &str, body: &str| Mail {
        id: format!("{}:INBOX:1:{n}", a.id),
        account: a.id.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: n,
        from: from.into(),
        sender: "Ana".into(),
        to: "work@example.com".into(),
        subject: subject.into(),
        snippet: body.replace('\n', " "),
        body: Some(body.into()),
        date: chrono::Utc::now().timestamp() - i64::from(n) * 60,
        message_id: format!("<{n}@example.com>"),
        ..Mail::default()
    };
    let meeting = mail(
        1,
        "ana@example.com",
        "Termin",
        "Hallo,\n\nkönnen wir uns am Dienstag um 10:00 treffen, um das Budget durchzugehen?\n\nViele Grüße\nAna",
    );
    let newsletter = mail(
        2,
        "news@shop.test",
        "Unsere Angebote",
        "Neue Angebote diese Woche!",
    );
    let mut newsletter = newsletter;
    newsletter.list_unsubscribe = "<mailto:leave@shop.test>".into();
    store
        .save_messages(&[meeting.clone(), newsletter.clone()])
        .unwrap();
    let (view, cx) = open(store.clone(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.smart_replies(&meeting).replies.is_empty())
    });
    // Turned on, the model is installed (from the trained file) and loaded.
    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetSuggestReplies(true), cx);
    cx.run_until_parked();
    assert!(local_ai::REPLIES.is_installed(&store));
    view.read_with(cx, |this, _| {
        assert!(this.replies_error.is_none(), "{:?}", this.replies_error);
        // The model reads the message off the window's thread: nothing yet.
        assert!(this.smart_replies(&meeting).replies.is_empty());
    });
    let found = replies_for(&view, &meeting, cx);
    // Mailing lists get none.
    assert!(replies_for(&view, &newsletter, cx).replies.is_empty());
    let first = {
        assert_eq!(found.language, "de");
        assert!(
            !found.replies.is_empty(),
            "a meeting request gets suggestions"
        );
        let intents: Vec<&str> = found.replies.iter().map(|s| s.intent).collect();
        assert!(
            intents.iter().any(|i| [
                "works_for_me",
                "see_you_then",
                "check_calendar",
                "another_time"
            ]
            .contains(i)),
            "{intents:?}"
        );
        found.replies[0].clone()
    };
    act(&view, Action::Select(meeting.id.clone()), cx);
    frame(cx);
    assert!(cx.debug_bounds("smart-replies").is_some());
    assert!(cx.debug_bounds("smart-reply-0").is_some());
    // Choosing one starts a reply with its German wording, and it's
    // remembered.
    act(&view, Action::SmartReply(first.intent.to_owned()), cx);
    view.read_with(cx, |this, cx| {
        let reply = this
            .inline_composer(&meeting.id)
            .expect("the reply opens below the message");
        let draft = &reply.read(cx).draft;
        assert!(draft.body.starts_with(first.text), "{:?}", draft.body);
        assert_eq!(this.reply_history.uses[first.intent].chosen, 1);
    });
    let saved = store
        .preference(local_ai::REPLY_HISTORY_PREFERENCE)
        .expect("the history is kept");
    assert!(saved.contains(first.intent));
    // Turned away, it isn't offered again.
    act(
        &view,
        Action::DismissSmartReply(first.intent.to_owned()),
        cx,
    );
    assert!(!replies_for(&view, &meeting, cx).replies.iter().any(|s| s.intent == first.intent));
    act(&view, Action::ForgetReplyHistory, cx);
    view.read_with(cx, |this, _| assert!(this.reply_history.is_empty()));
    assert!(replies_for(&view, &meeting, cx).replies.iter().any(|s| s.intent == first.intent));
    // Off again: the model is deleted and nothing is suggested.
    act(&view, Action::SetSuggestReplies(false), cx);
    view.read_with(cx, |this, _| {
        assert!(this.reply_model.is_none());
        assert!(this.smart_replies(&meeting).replies.is_empty());
    });
    assert!(!local_ai::REPLIES.is_installed(&store));
}

/// What a model found arrives only while it is still wanted: a result
/// started before the cache was cleared, or for text since changed, is
/// dropped.
#[test]
fn inferred_results_arrive_only_while_still_wanted() {
    let mut cache = ai::Inferred::<usize, &str>::default();
    assert!(cache.start("a", &1));
    assert!(!cache.start("a", &1), "already running");
    cache.clear();
    assert!(!cache.finish("a", 1, "old"));
    assert!(cache.get("a", &1).is_none());
    assert!(cache.start("a", &2));
    assert!(!cache.finish("a", 1, "stale"));
    assert!(cache.finish("a", 2, "new"));
    assert_eq!(cache.get("a", &2).as_deref(), Some(&"new"));
    assert!(cache.get("a", &1).is_none());
}

/// While codes are hidden, a new-mail notification (on the lock screen)
/// never shows one, even after Show code was clicked in the app: those the
/// model finds are hidden, or, while it isn't loaded, every string shaped
/// like a code. In the app a code shows only once Show code is clicked, the
/// same strings hidden until then.
#[gpui::test]
fn codes_show_only_on_show_code_and_never_in_notifications(cx: &mut TestAppContext) {
    let (store, _) = mailbox();
    install(&store);
    let (view, cx) = open(store.clone(), cx);
    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetHideCodes(true), cx);
    let (code, order) = view.read_with(cx, |this, _| {
        let code = this.messages.iter().find(|m| m.subject.contains("482913")).unwrap().clone();
        let order = this.messages.iter().find(|m| m.subject.contains("839201")).unwrap().clone();
        assert_eq!(this.announced_subject(&code), "•••••• is your Acme verification code");
        // Not a code to the model: shown.
        assert_eq!(this.announced_subject(&order), order.subject);
        assert_eq!(this.hide_codes(&order, &order.subject), order.subject);
        (code, order)
    });
    // Shown in the app: still hidden in notifications.
    act(&view, Action::ShowCodes(code.id.clone(), true), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.hide_codes(&code, &code.subject), code.subject);
        assert!(!this.announced_subject(&code).contains("482913"));
    });
    act(&view, Action::ShowCodes(code.id.clone(), false), cx);
    // The model not loaded (installing, or it couldn't load): everything
    // shaped like a code is hidden in the app too, until Show code.
    view.update(cx, |this, _| {
        this.otp = None;
        this.otp_found.borrow_mut().clear();
    });
    view.read_with(cx, |this, _| {
        assert!(!this.announced_subject(&code).contains("482913"));
        assert!(!this.announced_subject(&order).contains("839201"), "shaped like a code");
        assert!(!this.hide_codes(&code, &code.subject).contains("482913"));
        assert!(!this.hide_codes(&order, &order.subject).contains("839201"));
        let full = this.backend.store.message(&code.id).unwrap().unwrap();
        assert!(!this.hide_codes(&full, full.body.as_deref().unwrap()).contains("482913"));
    });
    // Copying waits for the model, which alone can tell which is the code.
    act(&view, Action::CopyCode(code.id.clone()), cx);
    assert!(cx.read_from_clipboard().and_then(|c| c.text()).is_none_or(|t| t != "839201"));
    act(&view, Action::ShowCodes(code.id.clone(), true), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.hide_codes(&code, &code.subject), code.subject);
        assert!(!this.announced_subject(&code).contains("482913"));
    });
}
