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
    let toggle = cx.debug_bounds("show-plain").unwrap();
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
    let toggle = cx.debug_bounds("show-formatted").unwrap();
    cx.simulate_click(toggle.center(), Modifiers::none());
    frame(cx);
    view.read_with(cx, |this, cx| {
        assert!(this.reading_options(&first).html);
        assert!(this.html_view.read(cx).has_document())
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
            html: true,
            block_remote: false,
            ..Default::default()
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
        ..Default::default()
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
        this.messages[0].html = Some("<p>Hi <img src='https://example.com/a.png'></p>".into());
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    frame(cx);
    let pane = cx.debug_bounds("reader-pane").unwrap();
    for name in [
        "toggle-email-theme",
        "toggle-email-html",
        "toggle-email-remote",
    ] {
        let button = cx.debug_bounds(name).unwrap();
        assert!(button.left() >= pane.left() && button.right() <= pane.right());
    }
}

#[gpui::test]
fn long_plain_text_mail_is_laid_out_in_full(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    let body = (1..=300)
        .map(|i| format!("Paragraph {i}: the quick brown fox jumps over the lazy dog."))
        .collect::<Vec<_>>()
        .join("\n\n");
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = None;
        this.messages[0].body = Some(body);
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    frame(cx);
    frame(cx);
    // The footer follows the whole body, far below the visible area; a body
    // clipped to the viewport put it within one screen of the header.
    let pane = cx.debug_bounds("reader-pane").unwrap();
    let footer = cx.debug_bounds("reply-bottom").unwrap();
    assert!(
        footer.top() - pane.top() > px(5000.),
        "footer at {:?}, pane at {:?}",
        footer.top(),
        pane.top()
    );
}

#[gpui::test]
fn reader_controls_stay_on_one_row(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = Some("<p>Hello</p>".into());
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    let controls = [
        "toggle-email-theme",
        "toggle-email-html",
        "toggle-email-remote",
    ];
    // Wide and narrow three-pane windows, the reader dragged to its
    // narrowest, and a narrow window's full-width reader.
    for (width, drag) in [
        (1440., false),
        (1440., true),
        (1180., true),
        (960., false),
        (899., false),
        (480., false),
        (360., false),
    ] {
        cx.simulate_resize(size(px(width), px(700.)));
        view.update_in(cx, |this, _, cx| {
            this.reader_width = drag.then_some(super::panes::READER_MIN);
            cx.notify();
        });
        frame(cx);
        frame(cx);
        let pane = cx.debug_bounds("reader-pane").unwrap();
        let bounds: Vec<_> = controls
            .iter()
            .map(|n| cx.debug_bounds(n).unwrap())
            .collect();
        for b in &bounds {
            assert_eq!(b.top(), bounds[0].top(), "{width}: controls wrapped");
            assert!(
                b.right() <= pane.right(),
                "{width}: control past the reader"
            );
            assert!(
                b.left() >= pane.left(),
                "{width}: control before the reader"
            );
        }
    }
}
