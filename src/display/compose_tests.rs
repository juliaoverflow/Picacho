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
