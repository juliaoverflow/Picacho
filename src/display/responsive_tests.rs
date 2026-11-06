//! Layout checks use GPUI's real layout engine at desktop and narrow sizes.
use super::tests::{act, frame, open, store, two_accounts};
use super::*;
use std::prelude::v1::test;

const SIZES: [(f32, f32); 8] = [
    (1440., 1000.),
    (1180., 700.),
    (960., 600.),
    (899., 600.),
    (700., 480.),
    (640., 480.),
    (480., 420.),
    (360., 320.),
];
fn resize(cx: &mut VisualTestContext, width: f32, height: f32) {
    cx.simulate_resize(size(px(width), px(height)));
    cx.run_until_parked();
    frame(cx);
}
fn bounds(cx: &mut VisualTestContext, name: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{name} is missing"))
}
fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>, description: &str) {
    let epsilon = px(1.);
    assert!(
        inner.left() >= outer.left() - epsilon
            && inner.right() <= outer.right() + epsilon
            && inner.top() >= outer.top() - epsilon
            && inner.bottom() <= outer.bottom() + epsilon,
        "{description}: {inner:?} outside {outer:?}"
    );
}
#[gpui::test]
fn settings_fit_short_and_narrow_windows_and_all_sync_choices_remain_reachable(
    cx: &mut TestAppContext,
) {
    let (view, cx, _) = two_accounts(cx);
    let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    act(&view, Action::Settings, cx);
    for (width, height) in SIZES {
        resize(cx, width, height);
        let panel = bounds(cx, "modal-panel");
        inside(
            panel,
            Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))),
            "settings dialog",
        );
        let close = bounds(cx, "modal-close");
        inside(close, panel, "close button");
        let page = bounds(cx, "settings-page");
        inside(page, panel, "scrollable settings page");
        assert!(
            page.size.width > px(200.) && page.size.height > px(80.),
            "{width}x{height}: {page:?}"
        );
        act(&view, Action::AccountSettings(id.clone()), cx);
        frame(cx);
        // Segments may wrap vertically, but never spill horizontally.
        for index in 0..6 {
            let selector = Box::leak(format!("refresh-{id}-{index}").into_boxed_str());
            let choice = bounds(cx, selector);
            assert!(
                choice.left() >= page.left() && choice.right() <= page.right(),
                "{width}x{height}: {choice:?} outside {page:?}"
            );
        }
        view.update_in(cx, |this, _, cx| {
            this.settings_scroll.scroll_to_bottom();
            cx.notify();
        });
        frame(cx);
        let remove = bounds(cx, Box::leak(format!("remove-{id}").into_boxed_str()));
        inside(
            remove,
            page,
            "account removal remains reachable after scrolling",
        );
        act(&view, Action::SettingsPage(Page::General), cx);
    }
}
#[gpui::test]
fn encrypted_reader_keeps_toolbar_and_password_controls_in_one_scrollable_pane(
    cx: &mut TestAppContext,
) {
    let (view, cx, _) = two_accounts(cx);
    view.update_in(cx, |this, _, cx| {
        let mut message = this.messages[0].clone();
        message.body = None;
        message.subject = "A long encrypted subject that must wrap across a narrow reader".into();
        message.sender = "A sender with a long display name".into();
        this.selected = Some(message.id.clone());
        this.locked.insert(message.account.clone());
        this.messages[0] = message;
        cx.notify();
    });
    for (width, height) in SIZES {
        resize(cx, width, height);
        let pane = bounds(cx, "reader-pane");
        inside(
            pane,
            Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))),
            "reader pane",
        );
        assert!(pane.size.width >= px(280.), "reader was squeezed: {pane:?}");
        inside(bounds(cx, "reader-toolbar"), pane, "locked reader toolbar");
        let field = bounds(cx, "unlock-field");
        assert!(
            field.size.width >= px(200.)
                && field.left() >= pane.left()
                && field.right() <= pane.right(),
            "password field: {field:?} in {pane:?}"
        );
        view.update_in(cx, |this, _, cx| {
            this.reader_scroll.scroll_to_bottom();
            cx.notify();
        });
        frame(cx);
        inside(
            bounds(cx, "unlock-mail"),
            pane,
            "unlock action is reachable by scrolling",
        );
        if width < 900. {
            let expected_width = if width < 480. {
                width
            } else {
                width - SIDEBAR_RAIL
            };
            assert!((pane.size.width - px(expected_width)).abs() < px(1.));
            inside(bounds(cx, "reader-back"), pane, "back navigation");
        }
    }
    // Empty submissions give feedback next to the field and keep focus there.
    act(&view, Action::UnlockMail, cx);
    assert!(cx.debug_bounds("unlock-error").is_some());
    view.update_in(cx, |this, w, cx| {
        assert!(this.unlock_input.read(cx).focus_handle(cx).is_focused(w))
    });
    act(&view, Action::BackToList, cx);
    frame(cx);
    inside(
        bounds(cx, "message-list"),
        Bounds::new(point(px(0.), px(0.)), size(px(360.), px(320.))),
        "back to message list",
    );
    view.read_with(cx, |this, _| assert!(this.selected.is_none()));
}
#[gpui::test]
fn onboarding_and_add_account_can_scroll_at_minimum_window_size(cx: &mut TestAppContext) {
    let (view, cx) = open(store(), cx);
    resize(cx, 360., 320.);
    act(&view, Action::Start, cx);
    let service = bounds(cx, "service-Pinal");
    assert!(
        service.left() >= px(0.) && service.right() <= px(360.),
        "provider card: {service:?}"
    );
    act(&view, Action::ChooseService(Service::Other), cx);
    act(&view, Action::Manual, cx);
    // Every modal shares the same viewport width and height constraints.
    act(&view, Action::Finish, cx);
    act(&view, Action::Connect, cx);
    frame(cx);
    inside(
        bounds(cx, "modal-panel"),
        Bounds::new(point(px(0.), px(0.)), size(px(360.), px(320.))),
        "add account dialog",
    );
    inside(
        bounds(cx, "modal-close"),
        bounds(cx, "modal-panel"),
        "add account close action",
    );
}

#[gpui::test]
fn bulk_selection_and_compose_controls_fit_a_narrow_window(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    resize(cx, 360., 320.);
    act(&view, Action::SelectAll, cx);
    frame(cx);
    let list = bounds(cx, "message-list");
    inside(bounds(cx, "bulk-archive"), list, "bulk archive control");
    inside(bounds(cx, "select-done"), list, "clear selection control");
    act(&view, Action::StopSelecting, cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    frame(cx);
    let panel = bounds(cx, "modal-panel");
    inside(
        panel,
        Bounds::new(point(px(0.), px(0.)), size(px(360.), px(320.))),
        "compose dialog",
    );
    inside(bounds(cx, "send"), panel, "send action");
    inside(bounds(cx, "save-close"), panel, "save draft action");
}
#[gpui::test]
fn dragging_panes_keeps_the_list_readable_and_snaps_into_rails(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    resize(cx, 1440., 900.);
    let drag = |cx: &mut VisualTestContext, pane: Pane, from: f32, to: f32| {
        view.update(cx, |this, cx| {
            this.begin_resize(pane, from, cx);
            this.resize_to(to, cx);
            this.end_resize(cx);
        });
        frame(cx);
    };
    // Widening the sidebar and reader as far as possible leaves the list usable.
    drag(cx, Pane::Sidebar, 228., 2000.);
    drag(cx, Pane::Reader, 1000., -2000.);
    let list = bounds(cx, "message-list");
    assert!(list.size.width >= px(LIST_MIN - 1.), "{list:?}");
    view.read_with(cx, |this, _| {
        assert!(!this.sidebar_collapsed && !this.reader_collapsed);
    });
    // Dragging past half the smallest useful width snaps into the rail.
    let sidebar = bounds(cx, "sidebar").size.width;
    drag(cx, Pane::Sidebar, 300., 300. - f32::from(sidebar));
    let reader = view.read_with(cx, |this, _| this.reader_shown.get());
    drag(cx, Pane::Reader, 900., 900. + reader);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.sidebar_collapsed && this.reader_collapsed);
    });
    assert!(cx.debug_bounds("reader-ribbon").is_some());
    // Dragging back out reopens a pane at its smallest useful width or more.
    drag(cx, Pane::Sidebar, 60., 400.);
    view.read_with(cx, |this, _| {
        assert!(!this.sidebar_collapsed);
        assert!(this.sidebar_width >= SIDEBAR_MIN);
    });
}
