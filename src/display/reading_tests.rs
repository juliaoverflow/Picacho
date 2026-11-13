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

/// The USPS digest as downloaded: its HTML and a scan sent with it.
fn usps_source() -> Vec<u8> {
    use base64::Engine as _;
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let image = image::RgbImage::from_pixel(300, 120, image::Rgb([200, 30, 30]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    format!(
        "From: USPS Informed Delivery <USPSInformeddelivery@email.informeddelivery.usps.com>\r\n\
         Subject: Your Daily Digest\r\nMessage-ID: <digest-1@informeddelivery.usps.com>\r\n\
         Content-Type: multipart/related; boundary=\"rel\"\r\n\r\n\
         --rel\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n\
         --rel\r\nContent-Type: image/png\r\nContent-ID: <2484554579-510.jpg>\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n--rel--\r\n",
        b64(include_str!("../../tests/fixtures/usps-informed-delivery.html").as_bytes()),
        b64(&png.into_inner()),
    )
    .into_bytes()
}

#[gpui::test]
fn reader_buttons_work_after_opening_an_html_email_and_visiting_settings(
    cx: &mut TestAppContext,
) {
    let (view, cx, store) = two_accounts(cx);
    cx.update(|_, cx| cx.bind_keys([KeyBinding::new("escape", Close, Some("Mail"))]));
    cx.simulate_resize(size(px(1440.), px(900.)));
    let ids = view.update_in(cx, |this, _, _| {
        this.messages.iter().take(4).map(|m| m.id.clone()).collect::<Vec<_>>()
    });
    // As cached after a download: the list holds summaries; the cache the
    // HTML and the source, which opening the message reads.
    let mut usps = store.message(&ids[2]).unwrap().unwrap();
    usps.html = Some(include_str!("../../tests/fixtures/usps-informed-delivery.html").into());
    store.save_messages(&[usps]).unwrap();
    store.save_raw(&ids[2], &usps_source()).unwrap();
    view.update_in(cx, |this, _, cx| {
        this.reload_cached(cx);
        // A big mailbox is always syncing something: the app redraws for
        // its spinner all the while.
        this.syncing = true;
    });
    cx.run_until_parked();
    let settle = |cx: &mut VisualTestContext| {
        for _ in 0..15 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(cx);
            cx.run_until_parked();
        }
    };
    let bounds = |cx: &mut VisualTestContext, selector: String| {
        frame(cx);
        let leaked: &'static str = Box::leak(selector.into_boxed_str());
        cx.debug_bounds(leaked)
            .unwrap_or_else(|| panic!("{leaked} was not rendered"))
    };
    let click = |cx: &mut VisualTestContext, selector: String| {
        let at = bounds(cx, selector).center();
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
    };
    // A few emails, then the HTML one.
    for id in [&ids[0], &ids[1], &ids[3], &ids[2]] {
        click(cx, format!("row-{id}"));
        settle(cx);
    }
    view.read_with(cx, |this, _| {
        assert_eq!(this.selected.as_ref(), Some(&ids[2]));
        let m = this.selected_ref().unwrap();
        assert!(!m.lacks_inline_parts(), "opening it read its parts");
    });
    // Into the email: hover, click, scroll it.
    let email = bounds(cx, "html-email".into());
    let inside = point(email.center().x, email.top() + px(200.));
    cx.simulate_mouse_move(inside, None, Modifiers::none());
    cx.simulate_click(inside, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: inside,
        delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
        ..Default::default()
    });
    settle(cx);
    // Settings, a look around every page, then back with Escape.
    click(cx, "accounts".into());
    view.read_with(cx, |this, _| assert!(this.modal == Some(Modal::Settings)));
    let account = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    for page in [
        "settings-reading".to_string(),
        "settings-composing".into(),
        "settings-forwarding".into(),
        format!("settings-account-{account}"),
        "settings-general".into(),
    ] {
        click(cx, page);
        settle(cx);
    }
    cx.simulate_keystrokes("escape");
    settle(cx);
    view.read_with(cx, |this, _| assert!(this.modal.is_none()));
    // Every button in the reader's toolbar does what it says.
    // These test accounts have no server: a change shows at once, then is
    // put back, and the account notes the failure. Either way, it arrived.
    let reached = |cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| {
            this.account_issues
                .values()
                .any(|issues| issues.contains_key(&IssueSource::Change))
        })
    };
    view.update(cx, |this, _| this.account_issues.clear());
    click(cx, "read".into());
    assert!(reached(cx), "Mark unread");
    click(cx, "forward".into());
    view.read_with(cx, |this, _| assert_eq!(this.composers.len(), 1, "Forward"));
    settle(cx);
    let pane = bounds(cx, "reader-pane".into());
    let reply = bounds(cx, "inline-reply".into());
    assert!(
        reply.top() < pane.bottom() && reply.bottom() > pane.top() + px(57.),
        "the inline composer is scrolled into view: {reply:?} in {pane:?}"
    );
    // Reply while forwarding it inline: the reply opens beside it.
    click(cx, "reply".into());
    view.read_with(cx, |this, cx| {
        assert_eq!(this.composers.len(), 2, "Reply");
        assert_ne!(this.composers[1].read(cx).placement, Placement::Inline);
    });
    // Reply all goes back to that reply rather than start a third.
    click(cx, "reply-all".into());
    view.read_with(cx, |this, _| assert_eq!(this.composers.len(), 2, "Reply all"));
    view.update(cx, |this, _| this.account_issues.clear());
    click(cx, "archive".into());
    assert!(reached(cx), "Archive");
}

#[gpui::test]
fn copy_link_address_puts_the_link_on_the_clipboard(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = Some(
            "<body style=\"margin:0\"><a href=\"https://shop.example/p?id=7&amp;utm_source=mail\" \
             style=\"display:block;height:300px;font-size:40px\">Big link</a></body>"
                .into(),
        );
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    let settle = |cx: &mut VisualTestContext| {
        for _ in 0..15 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            frame(cx);
            cx.run_until_parked();
        }
    };
    settle(cx);
    let email = cx.debug_bounds("html-email").expect("the email is shown");
    let at = point(email.left() + px(40.), email.top() + px(30.));
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    settle(cx);
    let index = view.read_with(cx, |this, _| {
        let menu = this.menu.as_ref().expect("right-clicking a link opens a menu");
        menu.entries
            .iter()
            .position(|e| matches!(e, Entry::Item { label, .. } if label.as_ref() == "Copy Link Address"))
            .expect("the menu offers to copy the link")
    });
    let item: &'static str = Box::leak(format!("menu-item-{index}").into_boxed_str());
    let item = cx.debug_bounds(item).expect("the menu is drawn").center();
    cx.simulate_mouse_move(item, None, Modifiers::none());
    cx.simulate_click(item, Modifiers::none());
    settle(cx);
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied.as_deref(), Some("https://shop.example/p?id=7"));
    view.read_with(cx, |this, _| assert!(this.menu.is_none()));
}
