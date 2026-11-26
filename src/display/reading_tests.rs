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
    // clipped to the viewport put it within one screen of the header. (Tests
    // lay text out with GPUI's fixed test metrics, not the system's fonts.)
    let pane = cx.debug_bounds("reader-pane").unwrap();
    let footer = cx.debug_bounds("reply-bottom").unwrap();
    let screen = cx.update(|w, _| w.viewport_size().height);
    assert!(
        footer.top() - pane.top() > screen * 3.,
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
    cx.simulate_event(ScrollEvent {
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

#[gpui::test]
fn the_last_open_mailbox_opens_again_after_a_restart(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let (first, second) = view.read_with(cx, |this, _| {
        (this.accounts[0].id.clone(), this.accounts[1].id.clone())
    });
    // A server folder of the second account.
    let receipts = Mailbox::Remote("INBOX/Receipts".into());
    act(&view, Action::OpenMailbox(second.clone(), receipts.clone()), cx);
    cx.run_until_parked();
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.account.as_ref(), Some(&second));
        assert_eq!(this.mailbox(), receipts);
    });
    // All Accounts' Starred.
    act(&restarted, Action::OpenMailbox(fwd::ALL_MAIL.into(), Folder::Starred.into()), cx);
    cx.run_until_parked();
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.account.as_deref(), Some(fwd::ALL_MAIL));
        assert_eq!(this.folder, Folder::Starred);
        let accounts: HashSet<&str> = this.messages.iter().map(|m| m.account.as_str()).collect();
        assert_eq!(accounts.len(), 2, "both accounts' starred mail is listed");
    });
    // The first account's subscriptions.
    act(&restarted, Action::OpenSubscriptions(first.clone()), cx);
    cx.run_until_parked();
    store.flush();
    let (restarted, cx) = open(store.clone(), cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.account.as_ref(), Some(&first));
        assert!(this.subs_open);
    });
    // A place that is gone (its account removed) opens the first mailbox.
    store
        .set_preference(
            "last_place",
            r#"{"account":"gone","mailbox":{"Standard":"Archive"},"subscriptions":false}"#,
        )
        .unwrap();
    let (restarted, cx) = open(store, cx);
    cx.run_until_parked();
    restarted.read_with(cx, |this, _| {
        assert_eq!(this.account.as_ref(), Some(&first));
        assert_eq!(this.folder, Folder::Inbox);
        assert!(!this.subs_open);
        assert_eq!(this.messages.len(), 10, "its mail is listed");
    });
}

#[test]
fn copied_names_and_addresses_read_back_as_written() {
    for (name, email) in [
        ("Ana Silva", "ana@example.com"),
        ("Silva, Ana", "ana@example.com"),
        ("Ana \"the boss\" S.", "ana@example.com"),
    ] {
        let both = name_and_address(Some(name), email);
        let parsed = crate::sync::smtp::recipients(&both).unwrap();
        assert_eq!(parsed, vec![(Some(name.to_owned()), email.to_owned())], "{both}");
    }
    assert_eq!(name_and_address(None, "ana@example.com"), "ana@example.com");
    assert_eq!(name_and_address(Some(" "), "ana@example.com"), "ana@example.com");
}

#[test]
fn recipients_can_be_copied_but_only_senders_written_to_and_looked_up() {
    let labels = |entries: Vec<Entry>| -> Vec<String> {
        entries
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item { label, .. } => Some(label.to_string()),
                _ => None,
            })
            .collect()
    };
    let sender = labels(address_entries(Some("Ana"), "ana@example.com", Some("ana@example.com")));
    assert_eq!(
        sender,
        [
            "Copy Name",
            "Copy Address",
            "Copy Name and Address",
            "Send Email…",
            "Show Mail From This Address"
        ]
    );
    let recipient = labels(address_entries(Some("Ana"), "ana@example.com", None));
    assert_eq!(recipient, ["Copy Name", "Copy Address", "Copy Name and Address"]);
    let bare = labels(address_entries(None, "ana@example.com", None));
    assert_eq!(bare, ["Copy Address"]);
}

#[gpui::test]
fn right_clicking_the_sender_offers_to_write_and_find_their_mail(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    let (id, from) = view.read_with(cx, |this, _| {
        (this.messages[2].id.clone(), this.messages[2].from.clone())
    });
    act(&view, Action::Select(id), cx);
    frame(cx);
    let at = cx.debug_bounds("reader-from").expect("the sender is shown").center();
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    let send = view.read_with(cx, |this, _| {
        let menu = this.menu.as_ref().expect("right-clicking the sender opens a menu");
        let find = |name: &str| {
            menu.entries.iter().find_map(|e| match e {
                Entry::Item { label, action, .. } if label.as_ref() == name => Some(action.clone()),
                _ => None,
            })
        };
        assert!(find("Copy Address").is_some());
        assert!(matches!(
            find("Show Mail From This Address"),
            Some(Action::SearchFor(q)) if q == format!("from:{from}")
        ));
        find("Send Email…").expect("the menu offers to write to the sender")
    });
    act(&view, send, cx);
    view.read_with(cx, |this, cx| {
        let composer = this.composers.last().expect("a new message opens");
        let draft = &composer.read(cx).draft;
        assert!(draft.to.contains(&from), "{}", draft.to);
        assert!(draft.subject.is_empty());
    });
}

#[gpui::test]
fn banner_actions_stay_inside_their_banners_as_the_reader_narrows(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    // One message showing three of the banners: snoozed, asking for a read
    // receipt, and from a mailing list. (The sign-in code banner, drawn by
    // the same `banner`, needs the Local AI model.)
    view.update_in(cx, |this, _, cx| {
        let m = &mut this.messages[0];
        m.set_receipt_to("ana@example.com");
        m.list_unsubscribe = "<https://example.com/unsubscribe>".into();
        m.one_click = true;
        let m = m.clone();
        this.snoozed.insert(
            m.account.clone(),
            vec![crate::manage::backend::Snoozed {
                message_id: m.message_id.clone(),
                subject: m.subject.clone(),
                sender: m.sender.clone(),
                until: chrono::Local::now().timestamp() + 3 * 86_400,
                folder: "Snoozed".into(),
                ids: vec![],
            }],
        );
        this.selected = Some(m.id.clone());
        cx.notify();
    });
    let banners: [(&str, &[&str]); 3] = [
        ("snooze-banner", &["unsnooze", "resnooze"]),
        ("receipt-banner", &["receipt-decline", "receipt-send"]),
        ("unsubscribe-banner", &["unsubscribe-mail"]),
    ];
    // Three-pane windows with the reader at its narrowest, and narrow
    // windows' full-width readers.
    for (width, drag) in [
        (1440., false),
        (1180., true),
        (960., true),
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
        for (banner, actions) in banners {
            let selector: &'static str = banner;
            let b = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{width}: {banner} is not shown"));
            assert!(b.right() <= pane.right(), "{width}: {banner} past the reader");
            for action in actions {
                let a = cx
                    .debug_bounds(action)
                    .unwrap_or_else(|| panic!("{width}: {action} is not shown"));
                assert!(
                    a.left() >= b.left() && a.right() <= b.right(),
                    "{width}: {action} past {banner}: {a:?} in {b:?}"
                );
                assert!(
                    a.top() >= b.top() && a.bottom() <= b.bottom(),
                    "{width}: {action} outside {banner}: {a:?} in {b:?}"
                );
            }
        }
    }
}

#[gpui::test]
fn server_search_line_stays_inside_a_narrow_list(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    // One IMAP account's inbox, searched: the line offers to search the
    // server too.
    let account = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::OpenMailbox(account, Folder::Inbox.into()), cx);
    act(&view, Action::SearchFor("plans".into()), cx);
    // The list at its narrowest beside the reader, and narrow windows'
    // full-width lists.
    for (width, drag) in [
        (1440., false),
        (1440., true),
        (1180., true),
        (960., true),
        (480., false),
        (360., false),
    ] {
        cx.simulate_resize(size(px(width), px(700.)));
        view.update_in(cx, |this, _, cx| {
            // As wide as it goes: the list keeps only its narrowest.
            this.reader_width = drag.then_some(width);
            cx.notify();
        });
        frame(cx);
        frame(cx);
        let list = cx.debug_bounds("message-list").unwrap();
        let line = cx
            .debug_bounds("server-search")
            .unwrap_or_else(|| panic!("{width}: the server search line is not shown"));
        let link = cx.debug_bounds("server-search-start").unwrap();
        assert!(line.right() <= list.right(), "{width}: line past the list");
        assert!(
            link.left() >= line.left() && link.right() <= line.right(),
            "{width}: link past the line: {link:?} in {line:?}"
        );
        assert!(
            link.top() >= line.top() && link.bottom() <= line.bottom(),
            "{width}: link outside the line: {link:?} in {line:?}"
        );
    }
}

#[gpui::test]
fn a_long_press_selects_a_word_in_an_email_and_offers_to_copy_it(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = Some(
            "<p style='margin:0; font: 16px monospace'>Your code is ABCD1234 today</p>".into(),
        );
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    // The email is laid out on its own thread.
    let settle = |cx: &mut VisualTestContext| {
        for _ in 0..40 {
            frame(cx);
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    settle(cx);
    let email = cx.debug_bounds("html-email").unwrap();
    // "ABCD1234" starts 13 characters in, past the body's 12px margin.
    let word = point(email.left() + px(12. + 9.6 * 15.), email.top() + px(22.));
    let press = |phase, cx: &mut VisualTestContext| {
        cx.simulate_event(LongPressEvent {
            phase,
            start_position: word,
            position: word,
        })
    };
    press(TouchPhase::Started, cx);
    press(TouchPhase::Ended, cx);
    settle(cx);
    view.read_with(cx, |this, _| {
        let menu = this.menu.as_ref().expect("lifting the finger opens the menu");
        assert!(
            menu.entries.iter().any(|e| matches!(e,
                Entry::Item { label, action: Action::CopyText(text), .. }
                    if label.as_ref() == "Copy" && text == "ABCD1234")),
            "the pressed word is offered to copy"
        );
    });
}

#[test]
fn text_zoom_steps_like_a_browsers_and_stops_at_its_ends() {
    assert_eq!(next_text_zoom(1., 1), 1.1);
    assert_eq!(next_text_zoom(1.1, -1), 1.);
    assert_eq!(next_text_zoom(2., 1), 3.);
    assert_eq!(next_text_zoom(3., 1), 3., "no larger than 300%");
    assert_eq!(next_text_zoom(0.5, -1), 0.5, "no smaller than 50%");
    assert_eq!(next_text_zoom(1.75, 0), 1.);
    assert_eq!(
        next_text_zoom(1.3, 1),
        1.1,
        "a zoom between steps moves from 100%"
    );
}

/// Touch screens have no ⌘+ and ⌘−: the email's menu (a long press, or a
/// secondary click) zooms it, with the same steps and the same saved zoom.
#[gpui::test]
fn the_emails_menu_zooms_its_text(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html =
            Some("<body style=\"margin:0\"><p style=\"height:300px\">Hello</p></body>".into());
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
    let choose = |label: &str, cx: &mut VisualTestContext| {
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
        let (index, labels) = view.read_with(cx, |this, _| {
            let menu = this.menu.as_ref().expect("the email's menu opens");
            let labels: Vec<String> = menu
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    Entry::Separator => None,
                })
                .collect();
            let index = menu
                .entries
                .iter()
                .position(|e| matches!(e, Entry::Item { label: l, .. } if l.as_ref() == label));
            (index, labels)
        });
        let index = index.unwrap_or_else(|| panic!("{label} in {labels:?}"));
        let item: &'static str = Box::leak(format!("menu-item-{index}").into_boxed_str());
        let item = cx.debug_bounds(item).expect("the menu is drawn").center();
        cx.simulate_mouse_move(item, None, Modifiers::none());
        cx.simulate_click(item, Modifiers::none());
        settle(cx);
        labels
    };
    let zoom = |cx: &mut VisualTestContext| {
        view.read_with(cx, |this, cx| this.html_view.read(cx).text_zoom())
    };
    let labels = choose("Larger Text", cx);
    assert!(
        !labels.iter().any(|l| l == "Actual Size"),
        "at 100%: {labels:?}"
    );
    assert_eq!(zoom(cx), 1.1);
    choose("Larger Text", cx);
    assert_eq!(zoom(cx), 1.25);
    assert_eq!(
        store.preference(TEXT_ZOOM).as_deref(),
        Some("1.25"),
        "kept for every email"
    );
    choose("Smaller Text", cx);
    assert_eq!(zoom(cx), 1.1);
    choose("Actual Size", cx);
    assert_eq!(zoom(cx), 1.);
    view.read_with(cx, |this, _| assert!(this.menu.is_none()));
}

/// A downloaded delivery report (RFC 3464) shows each recipient's outcome
/// above its text; an ordinary message shows no such panel.
#[gpui::test]
fn delivery_reports_show_each_recipients_outcome(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    let ids = view.update_in(cx, |this, _, _| {
        this.messages.iter().take(2).map(|m| m.id.clone()).collect::<Vec<_>>()
    });
    let source = b"From: MAILER-DAEMON@mx.example\r\nSubject: Undelivered\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=z\r\n\r\n--z\r\nContent-Type: text/plain\r\n\r\nIt failed.\r\n--z\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; mx.example\r\n\r\nFinal-Recipient: rfc822; gone@example.org\r\nAction: failed\r\nStatus: 5.1.1\r\nDiagnostic-Code: smtp; 550 5.1.1 User unknown\r\n\r\n--z--\r\n";
    store.save_raw(&ids[0], source).unwrap();
    view.update_in(cx, |this, _, cx| this.reload_cached(cx));
    let show = |cx: &mut VisualTestContext, id: &str| {
        view.update_in(cx, |this, _, cx| {
            this.selected = Some(id.to_owned());
            cx.notify();
        });
        for _ in 0..4 {
            frame(cx);
            cx.run_until_parked();
        }
    };
    show(cx, &ids[0]);
    assert!(cx.debug_bounds("delivery-report").is_some());
    view.read_with(cx, |this, _| {
        let report = this.delivery_reports[&ids[0]].as_ref().unwrap();
        assert_eq!(report.recipients[0].recipient, "gone@example.org");
        assert_eq!(report.recipients[0].diagnostic, "550 5.1.1 User unknown");
    });
    show(cx, &ids[1]);
    assert!(cx.debug_bounds("delivery-report").is_none());
}

/// Drawn again (a hover, a scroll, a spinner), the reader hands the HTML
/// view the HTML it prepared once, which the view knows by its pointer: the
/// document isn't started again, nor the HTML compared. Changed, it is.
#[gpui::test]
fn the_reader_hands_the_same_html_to_the_view_each_frame(cx: &mut TestAppContext) {
    let (view, cx, _store) = two_accounts(cx);
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = Some("<p>First version of a long message</p>".into());
        this.selected = Some(this.messages[0].id.clone());
        cx.notify();
    });
    frame(cx);
    frame(cx);
    let prepared = |view: &Entity<MailApp>, cx: &mut VisualTestContext| {
        view.read_with(cx, |this, _| this.prepared_html.borrow().as_ref().map(|p| p.1.clone()).unwrap())
    };
    let html = prepared(&view, cx);
    let (started, same) = view.read_with(cx, |this, cx| this.html_view.read(cx).started(&html));
    assert!(same, "the view holds the reader's HTML itself");
    for _ in 0..3 {
        view.update(cx, |_, cx| cx.notify());
        frame(cx);
    }
    assert_eq!(view.read_with(cx, |this, cx| this.html_view.read(cx).started(&html)), (started, true));
    // Changed: a new document.
    view.update_in(cx, |this, _, cx| {
        this.messages[0].html = Some("<p>Second version, longer than the first one</p>".into());
        cx.notify();
    });
    frame(cx);
    let next = prepared(&view, cx);
    let (later, same) = view.read_with(cx, |this, cx| this.html_view.read(cx).started(&next));
    assert!(same && later > started);
}
