use super::tests::{act, frame, open, two_accounts};
use super::*;
use std::prelude::v1::test;

#[gpui::test]
fn email_controls_persist_independently_and_global_defaults_survive_restart(
    cx: &mut TestAppContext,
) {
    let (view, cx, store) = two_accounts(cx);
    let (first, second) = view.update_in(cx, |this, _, cx| {
        this.messages[0].html =
            Some("<h1>Rich message</h1><details><summary>Expand</summary>Details</details>".into());
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
        (this.messages[0].clone(), this.messages[1].clone())
    });
    frame(cx);
    frame(cx);
    assert!(cx.debug_bounds("html-email").is_some());
    let toggle = cx.debug_bounds("toggle-email-html").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::none());
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(!this.reading_options(&first).html);
        assert!(this.reading_options(&first).block_remote);
        assert_eq!(this.reading_options(&second), ReadingOptions::default());
        assert_eq!(this.reading_defaults, ReadingOptions::default());
    });
    view.read_with(cx, |this, cx| {
        assert!(!this.html_view.read(cx).has_document())
    });
    let toggle = cx.debug_bounds("toggle-email-remote").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::none());
    let saved: ReadingOptions = serde_json::from_str(
        &store
            .preference(&message_key(&first.account, &first.id))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved,
        ReadingOptions {
            html: false,
            block_remote: false
        }
    );
    act(&view, Action::Settings, cx);
    frame(cx);
    let target = cx.debug_bounds("settings-reading").unwrap().center();
    cx.simulate_click(target, Modifiers::none());
    frame(cx);
    view.read_with(cx, |this, _| assert_eq!(this.page, Page::Reading));
    let target = cx.debug_bounds("default-html").unwrap().center();
    cx.simulate_click(target, Modifiers::none());
    let defaults = ReadingOptions {
        html: false,
        block_remote: true,
    };
    view.read_with(cx, |this, _| {
        assert_eq!(this.reading_defaults, defaults);
        assert_eq!(this.reading_options(&first), saved);
        assert_eq!(this.reading_options(&second), defaults);
        let mut another_account = second.clone();
        another_account.account = "another-account".into();
        another_account.id = first.id.clone();
        assert_eq!(this.reading_options(&another_account), defaults);
    });
    let (restarted, cx) = open(store, cx);
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.reading_defaults, defaults);
        assert_eq!(this.reading_options(&first), saved);
    });
}

#[gpui::test]
fn reading_settings_and_email_controls_fit_narrow_windows(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::Reading), cx);
    for (width, height) in [(1440., 1000.), (640., 480.), (360., 320.)] {
        cx.simulate_resize(size(px(width), px(height)));
        frame(cx);
        let panel = cx.debug_bounds("modal-panel").unwrap();
        let html = cx.debug_bounds("default-html").unwrap();
        assert!(html.left() >= panel.left() && html.right() <= panel.right());
        view.update_in(cx, |this, _, cx| {
            this.settings_scroll.scroll_to_bottom();
            cx.notify();
        });
        frame(cx);
        let remote = cx.debug_bounds("default-remote").unwrap();
        assert!(remote.left() >= panel.left() && remote.right() <= panel.right());
        assert!(remote.bottom() <= panel.bottom());
    }
    act(&view, Action::Close, cx);
    view.update_in(cx, |this, _, cx| {
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    frame(cx);
    let pane = cx.debug_bounds("reader-pane").unwrap();
    for name in ["toggle-email-html", "toggle-email-remote"] {
        let button = cx.debug_bounds(name).unwrap();
        assert!(button.left() >= pane.left() && button.right() <= pane.right());
    }
}
