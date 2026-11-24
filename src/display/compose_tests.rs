//! Writing mail (replies, placement, drafts, sending with undo), reading
//! conversations, and account pictures.
use super::tests::{act, frame, two_accounts};
use super::*;
use std::prelude::v1::test;

fn type_into(composer: &Entity<Composer>, to: &str, subject: &str, cx: &mut VisualTestContext) {
    composer.update_in(cx, |c, w, cx| c.fill(to, subject, w, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn sending_waits_for_undo_and_failures_restore_the_draft(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    type_into(&composer, "ana@example.com", "Plans", cx);
    composer.update_in(cx, |c, w, cx| c.request_send(w, cx));
    cx.run_until_parked();
    let id = view.read_with(cx, |this, _| {
        assert!(this.composers.is_empty(), "the editor leaves while sending");
        assert_eq!(this.outbox.len(), 1);
        this.outbox[0].id
    });
    frame(cx);
    assert!(cx.debug_bounds("undo-send").is_some());
    // Undo puts the editor back with what was written.
    act(&view, Action::UndoSend(id), cx);
    view.read_with(cx, |this, cx| {
        assert!(this.outbox.is_empty());
        assert_eq!(this.composers.len(), 1);
        assert_eq!(this.composers[0].read(cx).draft.subject, "Plans");
        assert!(!this.composers[0].read(cx).sending);
    });
    // After the delay it is sent; the test server has no credentials, so it
    // fails and comes back with the error and the draft kept.
    composer.update_in(cx, |c, w, cx| c.request_send(w, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        assert!(this.outbox.is_empty());
        assert_eq!(this.composers.len(), 1);
        assert!(this.composers[0].read(cx).error.is_some());
    });
    let draft_id = composer.read_with(cx, |c, _| c.draft.id.clone());
    let accounts = view.read_with(cx, |this, _| this.accounts.clone());
    assert!(
        accounts.iter().any(|a| store
            .drafts(&a.id)
            .unwrap()
            .iter()
            .any(|d| d.id == draft_id)),
        "the draft is kept"
    );
}

#[gpui::test]
fn replies_open_inline_and_reply_all_copies_others(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    let id = view.read_with(cx, |this, _| this.messages[2].id.clone());
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::Reply(true), cx);
    view.read_with(cx, |this, cx| {
        let reply = this
            .inline_composer(&id)
            .expect("reply opens below the message");
        let draft = &reply.read(cx).draft;
        assert_eq!(reply.read(cx).placement, Placement::Inline);
        assert!(draft.to.contains("sophie@example.com"));
        assert!(draft.cc.contains("daniel@example.com"));
        assert!(draft.subject.starts_with("Re:"));
        assert!(draft.body.contains("> "), "the original is quoted");
    });
    frame(cx);
    assert!(cx.debug_bounds("inline-reply").is_some());
    // Replying again returns to the same reply.
    act(&view, Action::Reply(false), cx);
    view.read_with(cx, |this, _| assert_eq!(this.composers.len(), 1));
    // With replies set to open in a window of their own, the reply floats
    // or opens per "New messages open".
    let options = ComposeOptions {
        reply_placement: ReplyPlacement::Window,
        ..ComposeOptions::default()
    };
    act(&view, Action::SetComposeOptions(options), cx);
    let other = view.read_with(cx, |this, _| this.messages[3].id.clone());
    act(&view, Action::Select(other), cx);
    act(&view, Action::Reply(false), cx);
    view.read_with(cx, |this, cx| {
        assert_eq!(this.composers.len(), 2);
        assert_eq!(this.composers[1].read(cx).placement, Placement::Floating);
    });
}

#[gpui::test]
fn conversations_group_replies_and_fold_quoted_history(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    let daniel = view.read_with(cx, |this, _| this.messages[4].id.clone());
    act(&view, Action::Select(daniel), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        let m = this.selected_ref().unwrap();
        // Sophie's and Daniel's replies share a conversation in this
        // account only; newsletters with their own subjects stay apart.
        assert_eq!(this.thread(m).len(), 2);
        assert!(!this.quotes_open);
    });
    assert!(cx.debug_bounds("thread-label").is_some());
    assert!(cx.debug_bounds("toggle-quotes").is_some());
    act(&view, Action::ToggleQuotes, cx);
    view.read_with(cx, |this, _| assert!(this.quotes_open));
    let linear = view.read_with(cx, |this, _| this.messages[1].id.clone());
    act(&view, Action::Select(linear), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.quotes_open, "each message starts folded");
        assert_eq!(this.thread(this.selected_ref().unwrap()).len(), 1);
    });
}

#[gpui::test]
fn every_preset_picture_decodes_and_applies_from_settings(cx: &mut TestAppContext) {
    for (name, _, bytes) in crate::display::assets::PRESETS {
        let image = image::load_from_memory(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((image.width(), image.height()), (256, 256), "{name}");
    }
    let (view, cx, _) = two_accounts(cx);
    let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::AccountSettings(id.clone()), cx);
    // Opening the picture menu leaves Settings as it is: its entrance
    // animation does not replay under the menu.
    let before = view.read_with(cx, |this, _| this.anim);
    act(&view, Action::AvatarMenu(id.clone()), cx);
    view.read_with(cx, |this, _| {
        assert!(this.menu.is_some());
        assert_eq!(this.anim, before);
    });
    act(&view, Action::AvatarPresets(id.clone()), cx);
    frame(cx);
    assert!(cx.debug_bounds("preset-penguin").is_some());
    // The pictures open over Settings: it stays open and does not replay
    // its entrance (which looked like Settings reloading).
    view.read_with(cx, |this, _| {
        assert!(this.modal == Some(Modal::Settings));
        assert_eq!(this.anim, before);
    });
    assert!(cx.debug_bounds("presets-sheet").is_some());
    act(&view, Action::SetPresetAvatar(id.clone(), "penguin"), cx);
    view.read_with(cx, |this, _| {
        assert!(this.avatars.contains_key(&id));
        assert!(this.modal == Some(Modal::Settings), "back in Settings");
        let entries = this.avatar_entries(&id);
        let labels: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item { label, .. } => Some(label.to_string()),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Load from Gravatar",
                "Choose from Presets…",
                "Upload Your Own…",
                "Remove Picture"
            ]
        );
    });
}

#[gpui::test]
fn dragging_a_floating_composer_moves_it_and_never_selects_mail_text(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    let first = view.read_with(cx, |this, _| this.messages[0].id.clone());
    act(&view, Action::Select(first), cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    frame(cx);
    let header = cx
        .debug_bounds("compose-header")
        .expect("floating composer");
    // Grab the title, away from the header's buttons.
    let start = point(header.left() + px(40.), header.center().y);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for step in 1..=4 {
        let p = start - point(px(60. * step as f32), px(30. * step as f32));
        cx.simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::none());
    }
    let end = start - point(px(240.), px(120.));
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    let origin = composer.read_with(cx, |c, _| c.origin).expect("moved");
    assert!(origin.x < header.left(), "{origin:?} vs {header:?}");
}

#[gpui::test]
fn settings_expands_pops_out_and_comes_back(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::Forwarding), cx);
    act(&view, Action::PlaceSettings(SettingsPlace::Maximized), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(this.modal == Some(Modal::Settings));
        assert_eq!(this.settings_place, SettingsPlace::Maximized);
    });
    assert!(cx.debug_bounds("forwarding-add").is_some());
    // The list spans the page (inside its padding).
    let page = cx.debug_bounds("settings-page").unwrap();
    let list = cx.debug_bounds("forwarding-list").unwrap();
    assert!(
        list.size.width > page.size.width - px(60.),
        "{list:?} in {page:?}"
    );
    act(&view, Action::PlaceSettings(SettingsPlace::Window), cx);
    view.read_with(cx, |this, _| {
        assert!(this.modal.is_none(), "the dialog leaves the main window");
        assert!(this.settings_window.is_some());
        assert!(this.page == Page::Forwarding, "the page is kept");
    });
    // Opening Settings again goes to its window rather than a dialog.
    act(&view, Action::Settings, cx);
    view.read_with(cx, |this, _| assert!(this.modal.is_none()));
    act(&view, Action::PlaceSettings(SettingsPlace::Dialog), cx);
    view.read_with(cx, |this, _| {
        assert!(this.settings_window.is_none());
        assert!(this.modal == Some(Modal::Settings));
        assert!(this.page == Page::Forwarding);
    });
}

/// Send Later keeps the message in Drafts with its time, says when it goes,
/// and sends it once due; one that can't be sent stays, saying why.
#[gpui::test]
fn send_later_waits_in_drafts_until_due(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    type_into(&composer, "ana@example.com", "Tomorrow's plans", cx);
    let at = chrono::Utc::now().timestamp() + 3600;
    composer.update_in(cx, |c, w, cx| c.request_send_at(Some(at), w, cx));
    cx.run_until_parked();
    let (id, account) =
        composer.read_with(cx, |c, _| (c.draft.id.clone(), c.draft.account.clone()));
    view.read_with(cx, |this, _| {
        assert!(this.composers.is_empty(), "the editor closes");
        assert!(this.outbox.is_empty(), "nothing is sent yet");
        assert!(
            this.status.starts_with("Scheduled to send"),
            "{}",
            this.status
        );
    });
    let saved = store.draft(&id).unwrap().unwrap();
    assert_eq!(saved.send_at, Some(at));
    // Not due: nothing happens.
    view.update(cx, |this, cx| this.send_due(cx));
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.scheduled_sending.is_empty()));
    assert!(store.draft(&id).unwrap().unwrap().send_error.is_empty());
    // Opened again, it says when it goes and can be unscheduled.
    view.update_in(cx, |this, w, cx| this.open_draft(saved.clone(), w, cx));
    frame(cx);
    assert!(cx.debug_bounds("compose-unschedule").is_some());
    let reopened = view.read_with(cx, |this, _| this.composers[0].clone());
    reopened.update(cx, |_, cx| cx.emit(super::composer::ComposerEvent::Close));
    cx.run_until_parked();
    // Due: it is sent. The test account has no credentials, so sending
    // fails; it stays in Drafts with the reason, or waits to try again.
    let mut due = store.draft(&id).unwrap().unwrap();
    due.send_at = Some(chrono::Utc::now().timestamp() - 5);
    store.save_draft(&due).unwrap();
    // An account that can't sign in keeps it waiting, unchanged, until it can.
    let blocked = view.read_with(cx, |this, _| this.account_blocked(&account));
    if blocked {
        view.update(cx, |this, cx| this.send_due(cx));
        cx.run_until_parked();
        assert_eq!(store.draft(&id).unwrap().unwrap().send_at, due.send_at);
        assert!(store.draft(&id).unwrap().unwrap().send_error.is_empty());
        view.update(cx, |this, _| {
            this.account_issues.clear();
            this.retrying.clear();
        });
    }
    view.update(cx, |this, cx| this.send_due(cx));
    cx.run_until_parked();
    let after = store
        .draft(&id)
        .unwrap()
        .expect("a draft that wasn't sent is kept");
    let retrying = view.read_with(cx, |this, _| this.scheduled_retry.contains_key(&id));
    assert!(!after.send_error.is_empty() || retrying, "{after:?}");
    assert_eq!(after.account, account);
}

/// A sending address is added in an account's Settings, offered in From,
/// and answers mail sent to it; requiring encryption in transit is a switch
/// beside it.
#[gpui::test]
fn sending_addresses_are_added_in_settings_and_offered_in_from(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let work = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::AccountSettings(work.clone()), cx);
    frame(cx);
    let switch: &'static str = Box::leak(format!("require-tls-{work}").into_boxed_str());
    assert!(cx.debug_bounds(switch).is_some());
    act(&view, Action::ToggleRequireTls(work.clone()), cx);
    act(&view, Action::AddIdentity(work.clone()), cx);
    frame(cx);
    assert!(cx.debug_bounds("identity-form").is_some());
    // A wrong address is refused in the form.
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.identity_inputs[0], "not an address", w, cx);
        this.handle(Action::SaveIdentity, w, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.identity_edit.as_ref().unwrap().error.is_some());
    });
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.identity_inputs[0], "desk@shop.example", w, cx);
        MailApp::set(&this.identity_inputs[1], "Shop Desk", w, cx);
        this.handle(Action::SaveIdentity, w, cx);
    });
    cx.run_until_parked();
    let desk = view.read_with(cx, |this, _| {
        assert!(this.identity_edit.is_none(), "the form closes once saved");
        let a = this.accounts.iter().find(|a| a.id == work).unwrap();
        assert!(a.require_tls);
        assert_eq!(a.identities.len(), 1);
        assert_eq!(a.identities[0].email, "desk@shop.example");
        a.identities[0].clone()
    });
    let saved = store.accounts().unwrap();
    assert_eq!(
        saved.iter().find(|a| a.id == work).unwrap().identities,
        std::slice::from_ref(&desk)
    );
    frame(cx);
    assert!(cx.debug_bounds("identity-0").is_some());
    // Mail to the address is answered as it, and it isn't copied in.
    let id = view.read_with(cx, |this, _| {
        this.messages
            .iter()
            .find(|m| m.account == work)
            .unwrap()
            .id
            .clone()
    });
    view.update(cx, |this, _| {
        let m = this.messages.iter_mut().find(|m| m.id == id).unwrap();
        m.to = "Shop Desk <desk@shop.example>".into();
        m.set_cc("work@example.com, bo@example.com");
        store.save_messages(std::slice::from_ref(m)).unwrap();
    });
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::Reply(true), cx);
    view.read_with(cx, |this, cx| {
        let draft = &this.composers.last().unwrap().read(cx).draft;
        assert_eq!(draft.identity, desk.id);
        assert!(!draft.to.contains("desk@shop.example") && !draft.cc.contains("desk@shop.example"));
        assert!(draft.cc.contains("bo@example.com"), "{}", draft.cc);
    });
    // Removed, it goes from the account.
    act(
        &view,
        Action::RemoveIdentity(work.clone(), desk.id.clone()),
        cx,
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.accounts
                .iter()
                .find(|a| a.id == work)
                .unwrap()
                .identities
                .is_empty()
        );
    });
}

#[gpui::test]
fn pasting_rich_text_offers_the_other_formatting(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    let pasted = || paste::Pasted {
        plain: "bold".into(),
        formatted: "**bold**".into(),
    };
    let body = |cx: &mut VisualTestContext| {
        composer.read_with(cx, |c, cx| c.body.read(cx).value().to_string())
    };
    let label = |cx: &mut VisualTestContext| composer.read_with(cx, |c, _| c.paste_offer_label());

    // Off (the default): plain text goes in, formatting is offered.
    composer.update_in(cx, |c, w, cx| {
        c.body.update(cx, |s, cx| s.set_text("", w, cx));
        c.insert_pasted(pasted(), w, cx);
    });
    cx.run_until_parked();
    assert!(body(cx).starts_with("bold") && !body(cx).contains("**"));
    assert_eq!(label(cx), Some("Paste with formatting"));
    frame(cx);
    assert!(cx.debug_bounds("compose-paste-offer").is_some());
    composer.update_in(cx, |c, w, cx| c.swap_paste(w, cx));
    cx.run_until_parked();
    assert!(body(cx).starts_with("**bold**"));
    assert_eq!(label(cx), Some("Paste without formatting"));
    // Typing lets the offer go.
    composer.update_in(cx, |c, w, cx| {
        c.body.update(cx, |s, cx| s.insert("!", w, cx));
    });
    cx.run_until_parked();
    assert_eq!(label(cx), None);

    // On: the formatting goes in (turning a plain draft formatted), and
    // switching back to plain text leaves the draft plain again.
    composer.update_in(cx, |c, w, cx| {
        c.options.paste_formatted = true;
        c.draft.formatted = false;
        c.body.update(cx, |s, cx| s.set_text("", w, cx));
        c.insert_pasted(pasted(), w, cx);
    });
    cx.run_until_parked();
    assert!(body(cx).starts_with("**bold**"));
    assert!(composer.read_with(cx, |c, _| c.draft.formatted));
    assert_eq!(label(cx), Some("Paste without formatting"));
    composer.update_in(cx, |c, w, cx| c.swap_paste(w, cx));
    cx.run_until_parked();
    assert!(body(cx).starts_with("bold") && !body(cx).contains("**"));
    assert!(!composer.read_with(cx, |c, _| c.draft.formatted));
    assert_eq!(label(cx), Some("Paste with formatting"));
}

/// With delivery receipts on, Settings offers which reports to ask for
/// (RFC 3461 NOTIFY) and the whole message back (RET); with a time limit,
/// what a late message does and a trace (RFC 2852). Each switch is kept.
#[gpui::test]
fn delivery_report_and_time_limit_options_are_in_settings(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let work = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::AccountSettings(work.clone()), cx);
    frame(cx);
    let selector = |name: &str| -> &'static str { Box::leak(format!("{name}-{work}").into_boxed_str()) };
    assert!(cx.debug_bounds(selector("dsn-failure")).is_none());
    assert!(cx.debug_bounds(selector("deliver-by-trace")).is_none());
    act(&view, Action::ToggleDeliveryReceipts(work.clone()), cx);
    act(&view, Action::SetDeliverWithin(work.clone(), 4), cx);
    frame(cx);
    for name in ["dsn-success", "dsn-failure", "dsn-delay", "dsn-full", "deliver-by-trace"] {
        assert!(cx.debug_bounds(selector(name)).is_some(), "{name}");
    }
    act(&view, Action::SetDsn(work.clone(), DsnChange::Success), cx);
    act(&view, Action::SetDsn(work.clone(), DsnChange::ReturnFull), cx);
    act(&view, Action::SetDeliverLateNotify(work.clone(), true), cx);
    act(&view, Action::ToggleDeliverByTrace(work.clone()), cx);
    let saved = store.accounts().unwrap().into_iter().find(|a| a.id == work).unwrap();
    assert!(!saved.dsn.success && saved.dsn.failure && saved.dsn.delay && saved.dsn.return_full);
    assert!(saved.deliver_late_notify && saved.deliver_by_trace);
}

/// A picture pasted into a formatted draft goes in where it was pasted,
/// as Markdown naming its attachment; in a plain draft only attached.
#[gpui::test]
fn pasted_pictures_are_placed_in_formatted_drafts(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    let body = |cx: &mut VisualTestContext| {
        composer.read_with(cx, |c, cx| c.body.read(cx).value().to_string())
    };
    let picture = std::path::PathBuf::from("/drafts/x/Pasted image (2).png");
    composer.update_in(cx, |c, w, cx| {
        c.draft.formatted = false;
        c.body.update(cx, |s, cx| s.set_text("Look: ", w, cx));
        c.place_pictures(std::slice::from_ref(&picture), w, cx);
    });
    assert_eq!(body(cx), "Look: ");
    composer.update_in(cx, |c, w, cx| {
        c.draft.formatted = true;
        c.body.update(cx, |s, cx| s.set_text("", w, cx));
        c.place_pictures(std::slice::from_ref(&picture), w, cx);
    });
    assert_eq!(body(cx), "![Pasted image (2)](<file:Pasted image (2).png>)");
}

/// An encrypted message whose subject was hidden (RFC 9788 §5.2.1): its
/// outer Subject reads "...", and the reader shows the real one.
const SEALED: &[u8] = b"From: Ana <ana@example.org>\r\nTo: qa@example.com\r\nSubject: ...\r\nMessage-ID: <sealed@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=b\r\n\r\n--b\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--b\r\nContent-Type: application/octet-stream\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n\r\nwcBMA0\r\n-----END PGP MESSAGE-----\r\n--b--\r\n";

/// A message `SEALED` stands for, read as decrypted: its real subject and
/// text, as the reader keeps them.
fn sealed_message(view: &Entity<MailApp>, store: &crate::manage::store::Store, cx: &mut VisualTestContext) -> String {
    view.update(cx, |this, _| {
        let m = &mut this.messages[2];
        m.message_id = "<sealed@example.org>".into();
        // Cached as an encrypted message is: its outside, "...".
        m.subject = "...".into();
        store.save_messages(std::slice::from_ref(m)).unwrap();
        store.save_raw(&m.id, SEALED).unwrap();
        // Decrypted here, as the reader would have it.
        let mut opened = m.clone();
        opened.subject = "Plans".into();
        opened.body = Some("Meet at noon.\nAt the usual place.".into());
        this.backend.keep_readable_for_test(opened.clone());
        *m = opened;
        m.id.clone()
    })
}

/// RFC 9788 §6.1: a reply to encrypted mail starts encrypted, its subject
/// hidden as the original's was, and says the real subject inside; a
/// reply to unencrypted mail starts as before.
#[gpui::test]
fn replies_to_encrypted_mail_start_encrypted_with_the_subject_hidden(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let id = sealed_message(&view, &store, cx);
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::Reply(false), cx);
    view.read_with(cx, |this, cx| {
        let draft = &this.composers.last().unwrap().read(cx).draft;
        assert!(draft.encrypt && draft.encrypt_subject, "{draft:?}");
        assert_eq!(draft.subject, "Re: Plans");
    });
    // A forward too.
    act(&view, Action::Forward, cx);
    view.read_with(cx, |this, cx| {
        let draft = &this.composers.last().unwrap().read(cx).draft;
        assert!(draft.encrypt && draft.encrypt_subject && draft.forwarding, "{draft:?}");
    });
    let plain = view.read_with(cx, |this, _| this.messages[3].id.clone());
    act(&view, Action::Select(plain), cx);
    act(&view, Action::Reply(false), cx);
    view.read_with(cx, |this, cx| {
        let draft = &this.composers.last().unwrap().read(cx).draft;
        assert!(!draft.encrypt && !draft.encrypt_subject, "unchanged: {draft:?}");
    });
}

/// RFC 9788 §6.1: turned unencrypted, such a reply asks first; sent, its
/// subject reads "..." and it carries none of the original's text.
#[gpui::test]
fn unencrypted_replies_to_encrypted_mail_leave_out_its_text_and_subject(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let id = sealed_message(&view, &store, cx);
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::Reply(false), cx);
    let composer = view.read_with(cx, |this, _| this.composers.last().unwrap().clone());
    composer.update_in(cx, |c, w, cx| {
        c.draft.encrypt = false;
        c.draft.body = format!("Sounds good.{}", c.draft.body);
        c.reload_fields(w, cx);
    });
    composer.update_in(cx, |c, w, cx| c.request_send_at(None, w, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "asked before going unencrypted");
    cx.simulate_prompt_answer("Send Without the Quote");
    cx.run_until_parked();
    composer.read_with(cx, |c, _| {
        assert_eq!(c.draft.subject, "...");
        assert!(c.draft.body.starts_with("Sounds good."), "{}", c.draft.body);
        assert!(!c.draft.body.contains("Meet at noon") && !c.draft.body.contains("> "), "{}", c.draft.body);
        assert!(!c.draft.encrypt);
    });
    // Kept encrypted, nothing is asked or changed.
    act(&view, Action::Reply(false), cx);
    let again = view.read_with(cx, |this, _| this.composers.last().unwrap().clone());
    again.update_in(cx, |c, w, cx| c.request_send_at(None, w, cx));
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
}
