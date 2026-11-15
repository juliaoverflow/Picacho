//! Local AI in the app, with the trained model copied in where installing
//! puts it (tests don't download). Core ML isn't available here, so these
//! run with Picacho's own runtime.
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::local_ai;
use std::prelude::v1::test;

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

    act(&view, Action::AskDeleteModel("otp".into()), cx);
    act(&view, Action::DeleteModel("otp".into()), cx);
    view.read_with(cx, |this, _| {
        assert!(this.model_confirm.is_none());
        assert!(this.local_ai, "Local AI itself stays on");
        assert!(!this.hide_codes && this.otp.is_none(), "its feature is off");
    });
    assert!(!local_ai::OTP.is_installed(&store));
}
