//! The inbox's category tabs in the window: sorting the sample inbox,
//! switching tabs and scrolling them (arrows, the wheel, fades at the ends,
//! on a desktop and a phone), All Mail, searching across them, moving
//! senders (from the menu and by dragging onto a tab), turning tabs off,
//! and announcing only Primary's new mail.
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::categories::{Categorizing, Category, PREFERENCE};
use std::prelude::v1::test;

/// A store whose one account's inbox is the sample inbox, with the
/// categories as they start.
fn sorted_inbox() -> (Arc<Store>, String) {
    let store = store();
    store
        .set_preference(
            PREFERENCE,
            &serde_json::to_string(&Categorizing::default()).unwrap(),
        )
        .unwrap();
    let mut a = Account::new(Provider::Imap);
    a.name = "Work".into();
    a.email = "work@example.com".into();
    store.save_account(&a).unwrap();
    let mut mails = demo_mail();
    for m in &mut mails {
        m.account = a.id.clone();
        m.id = format!("{}:{}", a.id, m.id);
    }
    store.save_messages(&mails).unwrap();
    (store, a.id)
}

/// The senders of the listed rows, in order.
fn listed(this: &MailApp) -> Vec<String> {
    this.visible
        .iter()
        .map(|&i| this.messages[i].sender.clone())
        .collect()
}

fn selector(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// Whether `inner` lies within `outer`, horizontally.
fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5) && inner.right() <= outer.right() + px(0.5)
}

#[gpui::test]
fn the_inbox_is_sorted_into_tabs(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    frame(cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("category-tabs").is_some());
    for tab in [
        "primary",
        "transactions",
        "travel",
        "finance",
        "updates",
        "newsletters",
        "promotions",
        "social",
        "all",
    ] {
        assert!(
            cx.debug_bounds(selector(format!("category-tab-{tab}")))
                .is_some(),
            "{tab}"
        );
    }
    // Forums has no tab at first.
    assert!(cx.debug_bounds("category-tab-forums").is_none());
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::Is(Category::Primary));
        assert_eq!(
            listed(this),
            [
                "Alex Morgan",
                "Sophie Chen",
                "Daniel Kim",
                "Emma Wilson",
                "Oliver James"
            ]
        );
        // Unread: Alex in Primary, Linear in Updates, Figma's offer in
        // Promotions; all three in All Mail.
        assert_eq!(this.sorting.unread[Category::Primary as usize], 1);
        assert_eq!(this.sorting.unread[Category::Updates as usize], 1);
        assert_eq!(this.sorting.unread[Category::Promotions as usize], 1);
        assert_eq!(this.sorting.unread_in(categories::Tab::All), 3);
    });

    // The tabs don't all fit the list: the far end fades out under an
    // arrow, the near end doesn't.
    let frame_bounds = cx.debug_bounds("category-tabs").unwrap();
    assert!(cx.debug_bounds("category-tabs-later").is_some());
    assert!(cx.debug_bounds("category-tabs-earlier").is_none());
    let fade = cx.debug_bounds("category-tabs-later-fade").unwrap();
    assert!((fade.right() - frame_bounds.right()).abs() < px(1.));
    assert!(!within(
        cx.debug_bounds("category-tab-all").unwrap(),
        frame_bounds
    ));
    // The arrow scrolls toward the rest; at the end, the arrow is at the
    // other end only, and All Mail is in view.
    for _ in 0..10 {
        let Some(arrow) = cx.debug_bounds("category-tabs-later") else {
            break;
        };
        cx.simulate_click(arrow.center(), Modifiers::none());
        cx.run_until_parked();
        frame(cx);
        assert!(cx.debug_bounds("category-tabs-earlier").is_some());
    }
    assert!(cx.debug_bounds("category-tabs-later").is_none());
    let all = cx.debug_bounds("category-tab-all").unwrap();
    assert!(within(all, frame_bounds));
    // Clicking the arrow didn't open the tab under it.
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::Is(Category::Primary))
    });

    // All Mail: the whole inbox, each row tagged with its tab (none for
    // Primary's).
    cx.simulate_click(all.center(), Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::All);
        assert_eq!(this.visible.len(), 10);
    });
    assert!(
        cx.debug_bounds(selector(format!("row-category-{id}:demo-3")))
            .is_some()
    );
    assert!(
        cx.debug_bounds(selector(format!("row-category-{id}:demo-0")))
            .is_none()
    );

    // Back to the start: the wheel scrolls the row sideways.
    let at = cx.debug_bounds("category-tab-all").unwrap().center();
    cx.simulate_event(ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(2000.))),
        ..Default::default()
    });
    cx.run_until_parked();
    frame(cx);
    assert!(cx.debug_bounds("category-tabs-earlier").is_none());
    assert!(cx.debug_bounds("category-tabs-later").is_some());

    // Opening a tab out of view brings it into view.
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Social)),
        cx,
    );
    frame(cx);
    frame(cx);
    assert!(within(
        cx.debug_bounds("category-tab-social").unwrap(),
        cx.debug_bounds("category-tabs").unwrap()
    ));
    view.read_with(cx, |this, _| assert!(this.visible.is_empty()));
    // An empty tab says what goes in it.
    assert!(cx.debug_bounds("empty-list-title").is_some());

    // Clicking a tab in view shows its mail.
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Primary)),
        cx,
    );
    frame(cx);
    let at = cx
        .debug_bounds("category-tab-transactions")
        .unwrap()
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.sorting.tab,
            categories::Tab::Is(Category::Transactions)
        )
    });
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Updates)),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(listed(this), ["Linear", "Notion", "GitHub", "The Browser"]);
    });
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Promotions)),
        cx,
    );
    view.read_with(cx, |this, _| assert_eq!(listed(this), ["Figma"]));

    // A search looks in every tab, without the tabs.
    act(&view, Action::SearchFor("weekend".into()), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(listed(this), ["Sophie Chen", "Daniel Kim", "The Browser"])
    });
    assert!(cx.debug_bounds("category-tabs").is_none());
    act(&view, Action::SearchFor("category:promotions".into()), cx);
    view.read_with(cx, |this, _| assert_eq!(listed(this), ["Figma"]));
    act(&view, Action::CloseSearch, cx);

    // Other mailboxes have no tabs, and going back to the inbox shows
    // Primary.
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Archive.into()),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("category-tabs").is_none());
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::Is(Category::Primary))
    });

    // Turned off from the filter menu: the whole inbox, and saved so.
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleEnabled),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("category-tabs").is_none());
    view.read_with(cx, |this, _| assert_eq!(this.visible.len(), 10));
    store.flush();
    assert!(!Categorizing::load(&store).enabled);
    view.read_with(cx, |this, _| {
        assert!(names(this.filter_entries()).contains(&"Show Categories".to_owned()));
    });
}

/// On a phone the tabs are chips above the filters, All Mail last; an
/// edge with chips past it fades out, as the filters' do.
#[gpui::test]
fn the_phones_category_chips_fade_where_more_are_past(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store, cx);
    view.update(cx, |this, cx| {
        this.mobile.on = true;
        cx.notify();
    });
    cx.simulate_resize(size(px(390.), px(844.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("mobile-categories").is_some());
    assert!(cx.debug_bounds("mobile-category-all").is_some());
    assert!(cx.debug_bounds("mobile-categories-fade-leading").is_none());
    assert!(cx.debug_bounds("mobile-categories-fade-trailing").is_some());
    let max = view.read_with(cx, |this, _| this.mobile.categories_scroll.max_offset().x);
    assert!(max > px(0.), "the chips don't overflow a phone");
    // Opening All Mail brings its chip into view: at the end, the
    // leading edge fades instead.
    act(
        &view,
        Action::Categories(categories::CatAction::AllMail),
        cx,
    );
    frame(cx);
    frame(cx);
    assert!(cx.debug_bounds("mobile-categories-fade-leading").is_some());
    assert!(cx.debug_bounds("mobile-categories-fade-trailing").is_none());
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::All);
        assert_eq!(this.visible.len(), 10);
    });
    // Rows say where they were sorted.
    assert!(
        cx.debug_bounds(selector(format!("row-category-{id}:demo-3")))
            .is_some()
    );
}

/// The labels of menu entries.
fn names(entries: Vec<Entry>) -> Vec<String> {
    entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Item { label, .. } => Some(label.to_string()),
            Entry::Separator => None,
        })
        .collect()
}

#[gpui::test]
fn senders_move_between_tabs_and_tabs_turn_off(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store.clone(), cx);
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    let github = format!("{id}:demo-7");
    // The message's menu offers moving its sender; the menu after lists
    // the tabs, the one it's in checked.
    view.read_with(cx, |this, _| {
        assert!(names(this.mail_entries(&github)).contains(&"Move to Category…".to_owned()));
        let tabs = this.cat_menu_entries(std::slice::from_ref(&github));
        let checked: Vec<String> = tabs
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item {
                    label,
                    checked: true,
                    ..
                } => Some(label.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, ["Move to Updates"]);
    });
    act(
        &view,
        Action::Categories(categories::CatAction::MoveSenders(
            vec![github.clone()],
            Category::Primary,
        )),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(listed(this).contains(&"GitHub".to_owned()));
        assert_eq!(
            this.sorting
                .prefs()
                .sender_choice("notifications@github.com"),
            Some(Category::Primary)
        );
    });
    store.flush();
    assert_eq!(
        Categorizing::load(&store)
            .senders
            .get("notifications@github.com"),
        Some(&Category::Primary)
    );
    // Forgotten in Settings: back to Updates.
    act(
        &view,
        Action::Categories(categories::CatAction::ForgetSender(
            "notifications@github.com".into(),
        )),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(!listed(this).contains(&"GitHub".to_owned()))
    });

    // Updates' tab off: its mail is in Primary, and the open tab, gone,
    // goes back to Primary.
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Updates)),
        cx,
    );
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleTab(Category::Updates)),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("category-tab-updates").is_none());
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::Is(Category::Primary));
        assert!(listed(this).contains(&"GitHub".to_owned()));
        assert_eq!(this.visible.len(), 9, "all but Figma's offer");
    });
}

#[gpui::test]
fn messages_dragged_onto_a_tab_take_their_sender_there(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store, cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    frame(cx);
    // Social is past the list's edge: the arrow scrolls it into view.
    let list = cx.debug_bounds("category-tabs").unwrap();
    for _ in 0..10 {
        if within(cx.debug_bounds("category-tab-social").unwrap(), list) {
            break;
        }
        let arrow = cx.debug_bounds("category-tabs-later").unwrap();
        cx.simulate_click(arrow.center(), Modifiers::none());
        cx.run_until_parked();
        frame(cx);
    }
    let social = cx.debug_bounds("category-tab-social").unwrap();
    assert!(within(social, list));
    let row = selector(format!("row-{id}:demo-8"));
    let from = cx.debug_bounds(row).expect("Oliver's row").center();
    let to = social.center();
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for i in 1..=8 {
        let t = i as f32 / 8.;
        let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
    }
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(!listed(this).contains(&"Oliver James".to_owned()));
        assert_eq!(
            this.sorting.prefs().sender_choice("oliver@example.com"),
            Some(Category::Social)
        );
    });
    act(
        &view,
        Action::Categories(categories::CatAction::Tab(Category::Social)),
        cx,
    );
    view.read_with(cx, |this, _| assert_eq!(listed(this), ["Oliver James"]));
}

#[gpui::test]
fn only_primary_is_announced_when_asked(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store, cx);
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    view.update(cx, |this, _| {
        let figma = this
            .messages
            .iter()
            .find(|m| m.sender == "Figma")
            .unwrap()
            .clone();
        let alex = this
            .messages
            .iter()
            .find(|m| m.sender == "Alex Morgan")
            .unwrap()
            .clone();
        let code = Mail {
            id: "code".into(),
            from: "no-reply@accounts.example".into(),
            subject: "Your verification code".into(),
            snippet: "Your code is 482913. It expires in 10 minutes.".into(),
            ..Mail::default()
        };
        assert!(this.announces(&figma) && this.announces(&alex));
        this.control.notifications.mode =
            crate::manage::inbox_control::NotifyMode::PrimaryAndVips;
        assert!(!this.announces(&figma));
        assert!(this.announces(&alex));
        // A code is Primary, though a quick look can't tell.
        assert!(this.announces(&code));
        // A VIP is, whatever the tab.
        this.control.toggle_vip(&figma.from);
        assert!(this.announces(&figma));
        this.control.toggle_vip(&figma.from);
        // Promotions kept silent, everything else announced.
        this.control.notifications.mode = crate::manage::inbox_control::NotifyMode::Everything;
        let tab = this.announced_tab(&figma);
        this.control.toggle_silent(tab);
        assert!(!this.announces(&figma));
        assert!(this.announces(&alex));
        // Categories off: everything is Primary.
        this.control.notifications.mode =
            crate::manage::inbox_control::NotifyMode::PrimaryAndVips;
        Arc::make_mut(&mut this.sorting.sorter).prefs.enabled = false;
        assert!(this.announces(&figma));
    });
}

#[gpui::test]
fn settings_list_the_tabs_and_sorted_senders(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store, cx);
    cx.simulate_resize(size(px(1440.), px(1000.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    let github = format!("{id}:demo-7");
    act(
        &view,
        Action::Categories(categories::CatAction::MoveSenders(
            vec![github],
            Category::Primary,
        )),
        cx,
    );
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::Organizing), cx);
    frame(cx);
    frame(cx);
    for s in [
        "categories-on",
        "categories-all-mail-first",
        "category-shown-forums",
    ] {
        assert!(cx.debug_bounds(s).is_some(), "{s}");
    }
    assert!(cx.debug_bounds("forget-category-sender-0").is_some());
    // The page's switches turn the categories off and tabs on.
    let at = cx.debug_bounds("categories-on").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    frame(cx);
    view.read_with(cx, |this, _| assert!(!this.sorting.prefs().enabled));
    // Off, only the switch for them shows.
    assert!(cx.debug_bounds("category-shown-forums").is_none());
    assert!(cx.debug_bounds("forget-category-sender-0").is_none());
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleEnabled),
        cx,
    );
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleTab(Category::Forums)),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(this.sorting.prefs().enabled);
        assert!(this.sorting.prefs().tabs().contains(&Category::Forums));
    });
}

/// Settings → Local AI → Sort the inbox installs the inbox category model
/// (from `models/categories`, until it is published), which then sorts
/// with the rules; moving a sender teaches it, saved for next time, and
/// Forget unteaches it. Turned off, the model goes and the rules sort
/// alone. Skipped where `models/categories/train.py` hasn't been run.
#[gpui::test]
fn the_model_sorts_learns_and_forgets(cx: &mut TestAppContext) {
    use crate::manage::local_ai;
    let model = local_ai::local_dir(&local_ai::CATEGORIES).join("categories-v1.bin");
    if !model.exists() {
        eprintln!(
            "skipping: {} isn't there (run models/categories/train.py)",
            model.display()
        );
        return;
    }
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store.clone(), cx);
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    act(&view, Action::SetLocalAi(true), cx);
    act(&view, Action::SetSortAi(true), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.sorting.ai_error.is_none(),
            "{:?}",
            this.sorting.ai_error
        );
        assert!(this.sorting.sorter.ai.is_some(), "the model is loaded");
        // People's mail is still Primary with it.
        assert_eq!(
            listed(this),
            [
                "Alex Morgan",
                "Sophie Chen",
                "Daniel Kim",
                "Emma Wilson",
                "Oliver James"
            ]
        );
    });
    assert!(local_ai::CATEGORIES.is_installed(&store));
    // Moving The Browser's newsletter to Promotions teaches the model.
    let browser = format!("{id}:demo-9");
    act(
        &view,
        Action::Categories(categories::CatAction::MoveSenders(
            vec![browser],
            Category::Promotions,
        )),
        cx,
    );
    view.read_with(cx, |this, _| assert_eq!(this.sort_lessons(), 1));
    store.flush();
    assert_eq!(local_ai::categories_learned(&store).lessons, 1);
    act(&view, Action::ForgetSortLearning, cx);
    view.read_with(cx, |this, _| assert_eq!(this.sort_lessons(), 0));
    store.flush();
    assert!(local_ai::categories_learned(&store).is_empty());
    // Off: the model is deleted and the rules sort alone.
    act(&view, Action::SetSortAi(false), cx);
    view.read_with(cx, |this, _| assert!(this.sorting.sorter.ai.is_none()));
    assert!(!local_ai::CATEGORIES.is_installed(&store));
}

/// With "Open the inbox on All Mail", All Mail is the first tab and the
/// one the inbox opens on (and comes back to), on a desktop and a phone;
/// saved, and read back at the next start. Off again, Primary leads.
#[gpui::test]
fn the_inbox_can_open_on_all_mail(cx: &mut TestAppContext) {
    let (store, id) = sorted_inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleAllMailFirst),
        cx,
    );
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tabs().first(), Some(&categories::Tab::All));
        assert_eq!(
            this.sorting.tabs().last(),
            Some(&categories::Tab::Is(Category::Social))
        );
    });
    let all = cx.debug_bounds("category-tab-all").unwrap();
    let primary = cx.debug_bounds("category-tab-primary").unwrap();
    assert!(all.right() <= primary.left(), "All Mail is left of Primary");
    // Opening the inbox (again) shows All Mail: every message.
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Archive.into()),
        cx,
    );
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::All);
        assert_eq!(this.visible.len(), 10);
    });
    // Turning a tab off and on, or categories off and on, comes back to it.
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleEnabled),
        cx,
    );
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleEnabled),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::All)
    });
    // Saved, and a new window opens the inbox on All Mail.
    store.flush();
    assert!(Categorizing::load(&store).all_mail_first);
    assert_eq!(categories::Sorting::load(&store).tab, categories::Tab::All);
    // On a phone, its chip leads too.
    view.update(cx, |this, cx| {
        this.mobile.on = true;
        cx.notify();
    });
    cx.simulate_resize(size(px(390.), px(844.)));
    frame(cx);
    let chip_all = cx.debug_bounds("mobile-category-all").unwrap();
    let chip_primary = cx.debug_bounds("mobile-category-primary").unwrap();
    assert!(chip_all.right() <= chip_primary.left());
    // Off: Primary first, All Mail last, and the inbox opens on Primary.
    act(
        &view,
        Action::Categories(categories::CatAction::ToggleAllMailFirst),
        cx,
    );
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Archive.into()),
        cx,
    );
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.sorting.tab, categories::Tab::Is(Category::Primary));
        assert_eq!(this.sorting.tabs().last(), Some(&categories::Tab::All));
    });
}
