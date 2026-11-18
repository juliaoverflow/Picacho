//! The phone presentation, laid out by GPUI's real layout engine at phone
//! and tablet sizes on the test platform.
use super::super::tests::{act, frame, two_accounts};
use super::*;
use std::prelude::v1::test;

const PHONE: (f32, f32) = (390., 844.);
const TABLET: (f32, f32) = (1024., 768.);
/// A notched phone: status bar above, home indicator below.
const NOTCH: Insets = Insets {
    top: 47.,
    bottom: 34.,
    left: 0.,
    right: 0.,
    keyboard: 0.,
};

/// Two accounts' demo mail, shown as on a phone of `size`.
fn phone(size_: (f32, f32), cx: &mut TestAppContext) -> (Entity<MailApp>, &mut VisualTestContext) {
    let (view, cx, _) = two_accounts(cx);
    view.update(cx, |this, cx| {
        this.mobile.on = true;
        this.mobile.fake_insets = Some(NOTCH);
        cx.notify();
    });
    cx.simulate_resize(size(px(size_.0), px(size_.1)));
    cx.run_until_parked();
    frame(cx);
    (view, cx)
}

/// Lets slides and sheets finish moving, then lays out a frame.
fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    frame(cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    frame(cx);
}

fn bounds(cx: &mut VisualTestContext, name: &'static str) -> Bounds<Pixels> {
    settle(cx);
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{name} is missing"))
}

fn shown(cx: &mut VisualTestContext, name: &'static str) -> bool {
    settle(cx);
    cx.debug_bounds(name).is_some()
}

fn screen(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> Screen {
    view.read_with(cx, |this, _| this.mobile_screen())
}

fn first_message(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |this, _| {
        this.visible
            .first()
            .map(|&i| this.messages[i].id.clone())
            .expect("the inbox lists mail")
    })
}

/// Every control here takes a fingertip: at least `TOUCH` square.
fn touchable(cx: &mut VisualTestContext, name: &'static str) {
    let b = bounds(cx, name);
    assert!(
        b.size.width >= px(TOUCH - 0.5) && b.size.height >= px(TOUCH - 0.5),
        "{name} is {:?}, smaller than a fingertip",
        b.size
    );
}

#[gpui::test]
fn a_phone_goes_from_mailboxes_to_a_message_and_back(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    assert_eq!(screen(&view, cx), Screen::List);
    assert!(shown(cx, "mobile-list"));
    // Back from the list: the mailboxes.
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert_eq!(screen(&view, cx), Screen::Mailboxes);
    assert!(shown(cx, "mobile-mailboxes"));
    // A folder opens its list again.
    // The first mailboxes listed: All Accounts, when shown.
    let inbox = view.read_with(cx, |this, _| {
        let first = if this.shows_all_mail() {
            ALL_MAIL.to_owned()
        } else {
            this.accounts[0].id.clone()
        };
        format!("mobile-nav-{first}-inbox")
    });
    let inbox: &'static str = Box::leak(inbox.into_boxed_str());
    let row = bounds(cx, inbox);
    assert!(row.bottom() <= bounds(cx, "mobile-mailboxes").bottom());
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(screen(&view, cx), Screen::List);
    // A message opens over the list, and goes back to it.
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    assert_eq!(screen(&view, cx), Screen::Reader);
    assert!(shown(cx, "mobile-reader"));
    assert!(
        !shown(cx, "mobile-list"),
        "a phone shows one screen at a time"
    );
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert_eq!(screen(&view, cx), Screen::List);
}

#[gpui::test]
fn bars_stay_out_of_the_notch_and_home_indicator(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let (w, h) = PHONE;
    let top = bounds(cx, "mobile-list-bar");
    assert!(top.top() <= px(0.5), "the bar's background reaches the top");
    let back = bounds(cx, "mobile-to-mailboxes");
    assert!(
        back.top() >= px(NOTCH.top),
        "{back:?} is under the status bar"
    );
    let toolbar = bounds(cx, "mobile-list-toolbar");
    assert!((toolbar.bottom() - px(h)).abs() <= px(0.5));
    for control in ["mobile-compose", "mobile-search"] {
        let b = bounds(cx, control);
        assert!(
            b.bottom() <= px(h - NOTCH.bottom),
            "{control} is under the home indicator: {b:?}"
        );
        // Frequent actions sit in the bottom third, within a thumb's reach.
        assert!(
            b.top() >= px(h * 2. / 3.),
            "{control} is out of reach: {b:?}"
        );
        assert!(b.right() <= px(w));
    }
    // With the keyboard up, the search field rides on top of it.
    act(&view, Action::OpenSearch, cx);
    view.update(cx, |this, cx| {
        this.mobile.fake_insets = Some(Insets {
            keyboard: 336.,
            ..NOTCH
        });
        cx.notify();
    });
    let search = bounds(cx, "mobile-search-bar");
    assert!(
        (search.bottom() - px(h)).abs() <= px(0.5)
            && search.bottom() - search.top() >= px(TOOLBAR + 336. - 1.),
        "the search bar is not above the keyboard: {search:?}"
    );
}

#[gpui::test]
fn controls_are_fingertip_sized(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    for name in [
        "mobile-compose",
        "mobile-search",
        "mobile-to-mailboxes",
        "mobile-select",
        "mobile-filter-unread",
    ] {
        touchable(cx, name);
    }
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    for name in [
        "mobile-reader-back",
        "mobile-reader-archive",
        "mobile-reader-trash",
        "mobile-reader-move",
        "mobile-reader-reply",
        "mobile-reader-more",
        "mobile-reader-next",
    ] {
        touchable(cx, name);
    }
}

#[gpui::test]
fn menus_open_as_sheets_from_the_bottom(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    act(&view, Action::FilterMenu, cx);
    let (w, h) = PHONE;
    let panel = bounds(cx, "menu-panel");
    let cancel = bounds(cx, "menu-cancel");
    assert!(
        panel.size.width >= px(w - 20.),
        "the sheet spans the screen: {panel:?}"
    );
    assert!(
        cancel.bottom() <= px(h - NOTCH.bottom) && cancel.bottom() >= px(h - NOTCH.bottom - 20.)
    );
    let item = bounds(cx, "menu-item-0");
    assert!(item.size.height >= px(TOUCH));
    // Tapping above the sheet closes it.
    cx.simulate_click(point(px(w / 2.), px(80.)), Modifiers::none());
    cx.run_until_parked();
    assert!(view.read_with(cx, |this, _| this.menu.is_none()));
}

#[gpui::test]
fn a_tablet_shows_the_list_beside_the_message(cx: &mut TestAppContext) {
    let (view, cx) = phone(TABLET, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    let list = bounds(cx, "mobile-list");
    let reader = bounds(cx, "mobile-reader");
    assert!(
        list.right() <= reader.left() + px(1.),
        "{list:?} | {reader:?}"
    );
    assert!(
        !shown(cx, "mobile-reader-back"),
        "beside the list, nothing to go back to"
    );
}

#[gpui::test]
fn swiping_a_row_archives_it(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    let row: &'static str = Box::leak(format!("mobile-row-{id}").into_boxed_str());
    let at = bounds(cx, row).center();
    let pan = |dx: f32, phase: TouchPhase, cx: &mut VisualTestContext| {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            ..Default::default()
        });
    };
    pan(-30., TouchPhase::Started, cx);
    pan(-50., TouchPhase::Moved, cx);
    pan(-40., TouchPhase::Moved, cx);
    view.read_with(cx, |this, _| {
        let (swiping, dx) = this
            .mobile
            .swipe
            .clone()
            .expect("the row follows the finger");
        assert_eq!(swiping, id);
        assert!(dx <= -100.);
    });
    // Toward the leading edge archives; the other way marks read.
    view.read_with(cx, |this, _| {
        assert!(matches!(
            this.swipe_action(&id, false),
            Some(Action::Apply(ids, Change::Move(Folder::Archive))) if ids == [id.clone()]
        ));
        assert!(matches!(
            this.swipe_action(&id, true),
            Some(Action::Apply(_, Change::Read(_)))
        ));
    });
    let account = view.read_with(cx, |this, _| {
        this.messages
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .account
            .clone()
    });
    pan(0., TouchPhase::Ended, cx);
    view.read_with(cx, |this, _| {
        assert!(this.mobile.swipe.is_none());
        // The test accounts have no server: the move was sent, and failed.
        assert!(
            this.account_issues
                .get(&account)
                .is_some_and(|issues| issues.contains_key(&IssueSource::Change)),
            "letting go past the threshold did not archive"
        );
    });
}

#[gpui::test]
fn a_short_swipe_springs_back(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    let row: &'static str = Box::leak(format!("mobile-row-{id}").into_boxed_str());
    let at = bounds(cx, row).center();
    for (dx, phase) in [
        (20., TouchPhase::Started),
        (20., TouchPhase::Moved),
        (0., TouchPhase::Ended),
    ] {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            ..Default::default()
        });
    }
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.mobile.swipe.is_none());
        let m = this.messages.iter().find(|m| m.id == id).unwrap();
        assert!(this.visible.iter().any(|&i| this.messages[i].id == id));
        let _ = m;
    });
}

#[gpui::test]
fn settings_list_pages_then_open_one_full_screen(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    view.update_in(cx, |this, w, cx| this.mobile_open_settings(w, cx));
    assert_eq!(screen(&view, cx), Screen::Settings);
    assert!(shown(cx, "mobile-settings-list"));
    let reading = bounds(cx, "mobile-settings-reading");
    cx.simulate_click(reading.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(shown(cx, "settings-page"));
    assert!(!shown(cx, "mobile-settings-list"));
    let page = bounds(cx, "settings-page");
    assert!(page.right() <= px(PHONE.0) && page.left() >= px(0.));
    // Back goes to the list of pages, then out of Settings.
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert!(shown(cx, "mobile-settings-list"));
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert_ne!(screen(&view, cx), Screen::Settings);
    // A page chosen by an action (a menu, a shortcut) opens over the list.
    view.update_in(cx, |this, w, cx| this.mobile_open_settings(w, cx));
    assert!(shown(cx, "mobile-settings-list"));
    act(&view, Action::SettingsPage(Page::Composing), cx);
    assert!(shown(cx, "settings-page"));
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert_ne!(screen(&view, cx), Screen::Settings);
    // An account's menu opens its page directly.
    let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::AccountSettings(id), cx);
    assert!(shown(cx, "settings-page"));
}

#[gpui::test]
fn the_composer_covers_the_screen_with_send_above_the_keyboard(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    act(&view, Action::Compose, cx);
    let (w, h) = PHONE;
    let panel = bounds(cx, "compose-panel");
    assert_eq!(panel.size, size(px(w), px(h)));
    view.read_with(cx, |this, cx| {
        let composer = this.mobile_composer(cx).expect("a composer is open");
        assert_eq!(composer.read(cx).placement, Placement::Maximized);
        assert!(composer.read(cx).mobile);
    });
    view.update(cx, |this, cx| {
        this.mobile.fake_insets = Some(Insets {
            keyboard: 300.,
            ..NOTCH
        });
        cx.notify();
    });
    let send = bounds(cx, "compose-send");
    assert!(
        send.bottom() <= px(h - 300.),
        "Send is under the keyboard: {send:?}"
    );
    assert!(send.size.height >= px(TOUCH - 0.5));
    let header = bounds(cx, "compose-header");
    assert!(header.top() >= px(NOTCH.top));
    // Back first lets go of the keyboard, then closes it, keeping the draft.
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert!(view.read_with(cx, |this, cx| this.mobile_composer(cx).is_some()));
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |this, cx| this.mobile_composer(cx).is_none()));
}

#[gpui::test]
fn fields_ask_for_the_keyboard_they_need(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    view.read_with(cx, |this, cx| {
        let fields = this.keyboard_fields(cx);
        let keys = |state: &Entity<InputState>| {
            let focus = state.read(cx).focus_handle(cx);
            fields.iter().find(|(f, _)| *f == focus).map(|(_, k)| *k)
        };
        assert_eq!(keys(&this.form[EMAIL]), Some(Keys::Email));
        assert_eq!(keys(&this.form[IMAP_PORT]), Some(Keys::Number));
        assert_eq!(keys(&this.form[TOTP]), Some(Keys::Number));
        assert_eq!(keys(&this.form[SMTP_HOST]), Some(Keys::Url));
        assert_eq!(keys(&this.search), Some(Keys::Search));
    });
    // Focusing a field is followed; scrolling away lets go of it.
    act(&view, Action::OpenSearch, cx);
    frame(cx);
    assert!(view.read_with(cx, |this, _| this.mobile.keyboard_for.is_some()));
    act(&view, Action::CloseSearch, cx);
    frame(cx);
    assert!(view.read_with(cx, |this, _| this.mobile.keyboard_for.is_none()));
}

#[gpui::test]
fn sliding_screens_keep_their_width(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    // Mid-slide: moved over, not squeezed.
    frame(cx);
    let screen = cx
        .debug_bounds("mobile-screen")
        .expect("the screen is drawn");
    assert_eq!(screen.size.width, px(PHONE.0));
    let reader = cx
        .debug_bounds("mobile-reader")
        .expect("the reader is drawn");
    assert_eq!(reader.size.width, px(PHONE.0));
}

#[gpui::test]
fn a_flings_momentum_does_not_swipe_a_row(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    let row: &'static str = Box::leak(format!("mobile-row-{id}").into_boxed_str());
    let at = bounds(cx, row).center();
    // Momentum after a pan arrives as Moved events, never Started.
    for _ in 0..6 {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(-30.), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
    }
    assert!(view.read_with(cx, |this, _| this.mobile.swipe.is_none()));
}

#[gpui::test]
fn screens_title_themselves_beside_a_back_chevron(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let back = bounds(cx, "mobile-to-mailboxes");
    let bar = bounds(cx, "mobile-list-bar");
    assert!(back.left() <= px(12.), "back leads the bar: {back:?}");
    assert!(back.top() >= bar.top());
    view.update_in(cx, |this, w, cx| this.mobile_open_settings(w, cx));
    assert!(
        shown(cx, "mobile-settings-done"),
        "Settings goes back from its leading chevron"
    );
    let back = bounds(cx, "mobile-settings-done");
    assert!(back.left() <= px(12.) && back.size.width >= px(TOUCH - 0.5));
}

#[gpui::test]
fn account_folders_fold_from_the_header(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    let fold: &'static str = Box::leak(format!("mobile-fold-{id}").into_boxed_str());
    let inbox: &'static str = Box::leak(format!("mobile-nav-{id}-inbox").into_boxed_str());
    assert!(shown(cx, inbox));
    let at = bounds(cx, fold).center();
    cx.simulate_click(at, Modifiers::none());
    assert!(!shown(cx, inbox), "folded away");
    // The account's menu no longer offers it.
    view.read_with(cx, |this, _| {
        let entries = super::mailboxes::without_folding(this.account_entries(&id, false));
        assert!(!entries.iter().any(|e| matches!(
            e,
            Entry::Item {
                action: Action::ToggleAccount(_),
                ..
            }
        )));
    });
}

#[gpui::test]
fn the_keyboard_goes_when_its_screen_does(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    act(&view, Action::Compose, cx);
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.mobile.keyboard_for.is_some()));
    // Sending or closing takes the composer away; the field it left
    // focused lets go of the keyboard.
    view.update_in(cx, |this, _, cx| {
        let composer = this.mobile_composer(cx).unwrap();
        this.close_composer(&composer, cx);
        cx.notify();
    });
    settle(cx);
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.mobile.keyboard_for.is_none()));
}

#[gpui::test]
fn snoozing_fits_a_phone(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::SnoozeMenu(vec![id]), cx);
    let picker = bounds(cx, "snooze-picker");
    let panel = bounds(cx, "modal-panel");
    assert!(
        picker.right() <= panel.right() + px(0.5) && picker.left() >= px(0.),
        "{picker:?}"
    );
    let confirm = bounds(cx, "snooze-confirm");
    assert!(confirm.right() <= px(PHONE.0) && confirm.size.height >= px(TOUCH - 0.5));
}

/// A message with long headers and lines, opened with its original
/// source loaded, as the development driver does.
const LONG_EML: &str = "Return-Path: <bounce-1234567890abcdef@mail.newsletter-service.example.com>\r
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=newsletter-service.example.com; s=selector2026; h=from:to:subject:date:message-id; b=averylongsignaturevaluethatdoesnotwrapanywhere\r
Message-ID: <CAJ+averyveryverylongmessageidentifier.1234567890@mail.newsletter-service.example.com>\r
Date: Mon, 16 Nov 2026 08:00:00 +0000\r
From: Newsletter Service <hello@newsletter-service.example.com>\r
To: Someone With A Long Name <someone.with.a.long.address@example.com>\r
Subject: A rather long subject line that keeps going to see how the original message dialog handles it\r
Content-Type: text/html; charset=utf-8\r
\r
<html><body><table width=600><tr><td>A very long HTML line without breaks that would overflow a phone</td></tr></table></body></html>\r
";

fn open_original(view: &Entity<MailApp>, cx: &mut VisualTestContext) {
    view.update(cx, |this, cx| {
        let raw = LONG_EML.as_bytes();
        let mut account = Account::new(Provider::Imap);
        account.id = "demo".into();
        let mut mail =
            crate::sync::mime::parse_message(raw, &account, "eml-0", Folder::Inbox, "INBOX", 0)
                .unwrap();
        mail.date = chrono::Utc::now().timestamp();
        let details = crate::sync::mime::details(raw, false);
        this.details = Some(MessageInfo {
            id: "eml-0".into(),
            result: Some(Ok(Rc::new(Original::new(details, raw)))),
        });
        this.messages.insert(0, mail.clone());
        this.demo.insert(0, mail);
        this.filter(cx);
    });
    act(view, Action::Select("eml-0".into()), cx);
    act(view, Action::ShowSource(SourceView::Headers), cx);
}

#[gpui::test]
fn the_original_message_fits_a_phone(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    open_original(&view, cx);
    let panel = bounds(cx, "modal-panel");
    assert!(
        panel.left() >= px(0.) && panel.right() <= px(PHONE.0) + px(0.5),
        "{panel:?}"
    );
    for (view_, name) in [
        (SourceView::Headers, "headers"),
        (SourceView::Html, "html"),
        (SourceView::Original, "original"),
    ] {
        act(&view, Action::ShowSource(view_), cx);
        for part in [
            "original-facts",
            "original-source",
            "source-tab-headers",
            "source-tab-html",
            "source-tab-original",
            "copy-original",
            "save-original",
        ] {
            let Some(b) = ({
                settle(cx);
                cx.debug_bounds(part)
            }) else {
                if part == "original-source" && name == "html" {
                    continue;
                }
                panic!("{part} is missing in {name}");
            };
            assert!(
                b.right() <= panel.right() + px(0.5) && b.left() >= panel.left() - px(0.5),
                "{name}: {part} overflows the sheet: {b:?} beyond {panel:?}"
            );
        }
    }
}

#[gpui::test]
fn switching_source_views_does_not_replay_the_sheet(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    open_original(&view, cx);
    let anim = view.read_with(cx, |this, _| this.anim);
    act(&view, Action::ShowSource(SourceView::Html), cx);
    act(&view, Action::ShowSource(SourceView::Original), cx);
    assert_eq!(view.read_with(cx, |this, _| this.anim), anim);
}

#[gpui::test]
fn the_bottom_bar_is_balanced(cx: &mut TestAppContext) {
    let (_, cx) = phone(PHONE, cx);
    let settings = bounds(cx, "mobile-settings");
    let compose = bounds(cx, "mobile-compose");
    let search = bounds(cx, "mobile-search");
    assert_eq!(settings.size, compose.size);
    assert!((settings.left() - (px(PHONE.0) - compose.right())).abs() <= px(0.5));
    // Search fills the middle, centered between them.
    assert!(
        (search.center().x - px(PHONE.0 / 2.)).abs() <= px(1.),
        "{search:?}"
    );
    assert!(search.left() > settings.right() && search.right() < compose.left());
}

#[gpui::test]
fn a_swipe_decided_by_its_first_movement_never_scrolls_the_list(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    let row: &'static str = Box::leak(format!("mobile-row-{id}").into_boxed_str());
    let at = bounds(cx, row).center();
    let before = view.read_with(cx, |this, _| {
        this.list_scroll.0.borrow().base_handle.offset()
    });
    let pan = |dx: f32, phase: TouchPhase, cx: &mut VisualTestContext| {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            source: ScrollSource::Touch,
        });
    };
    // As iOS can: a pan that starts without moving, then moves sideways.
    pan(0., TouchPhase::Started, cx);
    pan(-40., TouchPhase::Moved, cx);
    pan(-30., TouchPhase::Moved, cx);
    assert!(view.read_with(cx, |this, _| this.mobile.swipe.is_some()));
    pan(0., TouchPhase::Ended, cx);
    // The fling after it moves nothing: no swipe, no scroll.
    for _ in 0..5 {
        pan(-20., TouchPhase::Moved, cx);
    }
    view.read_with(cx, |this, _| {
        assert!(this.mobile.swipe.is_none());
        assert_eq!(this.list_scroll.0.borrow().base_handle.offset(), before);
    });
}

fn edge_pan(cx: &mut VisualTestContext, x: f32, steps: &[(f32, TouchPhase)]) {
    for &(dx, phase) in steps {
        cx.simulate_event(ScrollEvent {
            position: point(px(x), px(400.)),
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            source: ScrollSource::Touch,
        });
    }
}

#[gpui::test]
fn dragging_from_the_edge_goes_back_over_the_screen_below(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    settle(cx);
    // As iOS reports it: starts without moving, then moves.
    edge_pan(
        cx,
        6.,
        &[
            (0., TouchPhase::Started),
            (60., TouchPhase::Moved),
            (60., TouchPhase::Moved),
        ],
    );
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.mobile.back_drag, Some(120.));
        assert_eq!(
            this.mobile_screen(),
            Screen::Reader,
            "nothing applies until let go"
        );
    });
    // The list is already there beneath the dragged message.
    assert!(cx.debug_bounds("mobile-list").is_some());
    let screen = cx.debug_bounds("mobile-screen").unwrap();
    assert_eq!(screen.left(), px(120.));
    // Back toward the edge, then let go short of the middle: it returns.
    edge_pan(
        cx,
        6.,
        &[(-40., TouchPhase::Moved), (0., TouchPhase::Ended)],
    );
    settle(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.mobile_screen(), Screen::Reader);
        assert!(this.mobile.back_drag.is_none() && this.mobile.back_release.is_none());
    });
    // Far enough: it goes back once let go.
    edge_pan(
        cx,
        6.,
        &[
            (0., TouchPhase::Started),
            (120., TouchPhase::Moved),
            (120., TouchPhase::Moved),
            (0., TouchPhase::Ended),
        ],
    );
    settle(cx);
    assert_eq!(screen_of(&view, cx), Screen::List);
}

fn screen_of(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> Screen {
    view.read_with(cx, |this, _| this.mobile_screen())
}

#[gpui::test]
fn a_swipe_away_from_the_edge_is_the_rows_not_back(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    let row: &'static str = Box::leak(format!("mobile-row-{id}").into_boxed_str());
    let at = bounds(cx, row).center();
    edge_pan(cx, f32::from(at.x), &[]);
    for (dx, phase) in [(0., TouchPhase::Started), (40., TouchPhase::Moved)] {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            source: ScrollSource::Touch,
        });
    }
    view.read_with(cx, |this, _| {
        assert!(this.mobile.back_drag.is_none());
        assert!(this.mobile.swipe.is_some(), "the row took it");
    });
    // From the very edge, the same pan goes back instead.
    let (view, cx) = (view, cx);
    view.update(cx, |this, _| {
        this.mobile.swipe = None;
        this.mobile.pan = None;
    });
    edge_pan(
        cx,
        4.,
        &[(0., TouchPhase::Started), (40., TouchPhase::Moved)],
    );
    view.read_with(cx, |this, _| {
        assert!(this.mobile.back_drag.is_some());
        assert!(this.mobile.swipe.is_none());
    });
}

#[gpui::test]
fn the_back_button_slides_over_the_screen_below(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    settle(cx);
    let back = bounds(cx, "mobile-reader-back");
    cx.simulate_click(back.center(), Modifiers::none());
    frame(cx);
    // Sliding away: still the message, with the list beneath.
    assert!(view.read_with(cx, |this, _| this.mobile.back_release.is_some()));
    assert!(cx.debug_bounds("mobile-list").is_some());
    settle(cx);
    assert_eq!(screen_of(&view, cx), Screen::List);
}

#[gpui::test]
fn stepping_between_messages_is_not_a_new_screen(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id), cx);
    settle(cx);
    let next = bounds(cx, "mobile-reader-next");
    cx.simulate_click(next.center(), Modifiers::none());
    assert_eq!(view.read_with(cx, |this, _| this.mobile.slide), Slide::Next);
    let prev = bounds(cx, "mobile-reader-prev");
    cx.simulate_click(prev.center(), Modifiers::none());
    assert_eq!(
        view.read_with(cx, |this, _| this.mobile.slide),
        Slide::Previous
    );
}

#[gpui::test]
fn searching_done_lets_go_of_the_keyboard_and_cancel_ends_it(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let search = bounds(cx, "mobile-search");
    cx.simulate_click(search.center(), Modifiers::none());
    settle(cx);
    assert!(view.read_with(cx, |this, _| this.search_open
        && this.mobile.keyboard_for.is_some()));
    let button = bounds(cx, "mobile-search-done");
    cx.simulate_click(button.center(), Modifiers::none());
    settle(cx);
    view.read_with(cx, |this, _| {
        assert!(this.search_open, "Done keeps the search");
        assert!(this.mobile.keyboard_for.is_none());
    });
    let button = bounds(cx, "mobile-search-cancel");
    cx.simulate_click(button.center(), Modifiers::none());
    settle(cx);
    assert!(view.read_with(cx, |this, _| !this.search_open));
}

#[gpui::test]
fn the_snooze_buttons_stay_in_view(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    let id = first_message(&view, cx);
    act(&view, Action::Select(id.clone()), cx);
    act(&view, Action::SnoozeMenu(vec![id]), cx);
    let panel = bounds(cx, "modal-panel");
    for part in ["snooze-confirm", "snooze-cancel"] {
        let b = bounds(cx, part);
        assert!(
            b.bottom() <= panel.bottom() && b.top() >= panel.top(),
            "{part}: {b:?} {panel:?}"
        );
    }
    // The calendar starts within the first screenful, under the choices.
    let today = chrono::Local::now().date_naive();
    let day: &'static str =
        Box::leak(format!("snooze-day-{}", today.format("%Y-%m-%d")).into_boxed_str());
    if let Some(b) = {
        settle(cx);
        cx.debug_bounds(day)
    } {
        assert!(b.top() < panel.bottom(), "{b:?}");
    }
}

#[gpui::test]
fn all_accounts_lists_every_accounts_subscriptions(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    assert!(view.read_with(cx, |this, _| this.shows_all_mail()));
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    assert_eq!(screen(&view, cx), Screen::Mailboxes);
    let row = bounds(cx, "mobile-subscriptions-all-mail");
    cx.simulate_click(row.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(screen(&view, cx), Screen::Subscriptions);
    view.read_with(cx, |this, _| {
        assert_eq!(this.account.as_deref(), Some(ALL_MAIL));
    });
}

#[gpui::test]
fn accounts_sort_by_dragging_their_handles(cx: &mut TestAppContext) {
    let (view, cx) = phone(PHONE, cx);
    view.update_in(cx, |this, w, cx| this.close(w, cx));
    let before = view.read_with(cx, |this, _| this.mobile_sortable());
    assert_eq!(before.len(), 2);
    // Edit shows a row with a handle per account.
    let edit = bounds(cx, "mobile-reorder");
    cx.simulate_click(edit.center(), Modifiers::none());
    let handle: &'static str =
        Box::leak(format!("mobile-reorder-handle-{}", before[0]).into_boxed_str());
    touchable(cx, handle);
    // Its icon is one Picacho draws (the component library's set lacks it).
    assert!(matches!(
        gpui::AssetSource::load(&crate::display::assets::Assets, "mail/grip.svg"),
        Ok(Some(_))
    ));
    let at = bounds(cx, handle).center();
    let pan = |dy: f32, phase: TouchPhase, cx: &mut VisualTestContext| {
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: phase,
            ..Default::default()
        });
    };
    // Less than halfway past the next row: still in its place.
    pan(5., TouchPhase::Started, cx);
    pan(15., TouchPhase::Moved, cx);
    assert_eq!(view.read_with(cx, |this, _| this.mobile_sortable()), before);
    // Past halfway: the two swap, and the list didn't scroll.
    pan(20., TouchPhase::Moved, cx);
    pan(0., TouchPhase::Ended, cx);
    let after = view.read_with(cx, |this, _| this.mobile_sortable());
    assert_eq!(after, vec![before[1].clone(), before[0].clone()]);
    view.read_with(cx, |this, _| {
        assert!(this.mobile.reorder_drag.is_none());
        assert_eq!(this.mobile.mailboxes_scroll.offset().y, px(0.));
    });
    // Done goes back to the mailboxes, in the new order.
    let done = bounds(cx, "mobile-reorder");
    cx.simulate_click(done.center(), Modifiers::none());
    assert!(!view.read_with(cx, |this, _| this.mobile.reordering));
    let first: &'static str = Box::leak(format!("mobile-account-{}", after[0]).into_boxed_str());
    let second: &'static str = Box::leak(format!("mobile-account-{}", after[1]).into_boxed_str());
    assert!(bounds(cx, first).top() < bounds(cx, second).top());
}
