//! Inbox control in the window: VIPs marked and gathered in their view,
//! the Screener holding first-time senders with its decisions (approve,
//! block, route to a category), what's announced, Settings → Inbox Control,
//! and putting together a rule of several conditions and actions.
use super::inbox_control::{ControlAction, Lens};
use super::tests::{act, frame, open, store};
use super::*;
use crate::manage::categories::Category;
use crate::manage::organize::{RuleAction, RuleField, RuleOp};
use std::prelude::v1::test;

/// A store whose one account's inbox is the sample inbox.
fn inbox() -> (Arc<Store>, String) {
    let store = store();
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

fn listed(this: &MailApp) -> Vec<String> {
    this.visible
        .iter()
        .map(|&i| this.messages[i].sender.clone())
        .collect()
}

fn id_of(this: &MailApp, sender: &str) -> String {
    this.messages
        .iter()
        .find(|m| m.sender == sender)
        .unwrap()
        .id
        .clone()
}

fn type_in(
    view: &Entity<MailApp>,
    field: fn(&MailApp) -> &Entity<InputState>,
    text: &str,
    cx: &mut VisualTestContext,
) {
    view.update_in(cx, |this, w, cx| {
        let state = field(this).clone();
        MailApp::set(&state, text.to_owned(), w, cx);
    });
}

#[gpui::test]
fn vips_are_marked_gathered_and_announced(cx: &mut TestAppContext) {
    let (store, id) = inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    let alex = view.read_with(cx, |this, _| id_of(this, "Alex Morgan"));
    let linear = view.read_with(cx, |this, _| id_of(this, "Linear"));
    act(
        &view,
        Action::Control(ControlAction::ToggleVip(alex.clone())),
        cx,
    );
    frame(cx);
    view.read_with(cx, |this, _| {
        assert!(this.control.is_vip("alex@studio.design"));
        assert!(listed(this).len() > 1);
    });
    let mark: &'static str = Box::leak(format!("vip-mark-{alex}").into_boxed_str());
    assert!(cx.debug_bounds(mark).is_some(), "the crown beside a VIP");
    assert!(cx.debug_bounds("sidebar-vips").is_some());
    // Saved for the background and the next launch.
    assert!(crate::manage::inbox_control::InboxControl::load(&store).is_vip("alex@studio.design"));
    // The VIPs view: only their mail, the tabs and the way back above it.
    act(&view, Action::Control(ControlAction::Lens(Lens::Vips)), cx);
    frame(cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.lens, Lens::Vips);
        assert!(
            listed(this).iter().all(|s| s == "Alex Morgan"),
            "{:?}",
            listed(this)
        );
    });
    assert!(cx.debug_bounds("vips-bar").is_some());
    // Only VIPs announced: Alex is, Linear isn't.
    view.update(cx, |this, _| {
        this.control.notifications.mode = crate::manage::inbox_control::NotifyMode::VipsOnly;
        let a = this.messages.iter().find(|m| m.id == alex).unwrap().clone();
        let l = this
            .messages
            .iter()
            .find(|m| m.id == linear)
            .unwrap()
            .clone();
        assert!(this.announces(&a));
        assert!(!this.announces(&l));
        // Paused: nobody, VIPs included.
        this.control.notifications.quiet.paused_until = chrono::Utc::now().timestamp() + 600;
        assert!(!this.announces(&a));
    });
    act(&view, Action::Control(ControlAction::Lens(Lens::Inbox)), cx);
    view.read_with(cx, |this, _| assert!(listed(this).len() > 1));
    // Another mailbox leaves the lens.
    act(&view, Action::Control(ControlAction::Lens(Lens::Vips)), cx);
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Sent.into()),
        cx,
    );
    view.read_with(cx, |this, _| assert_eq!(this.lens, Lens::Inbox));
}

#[gpui::test]
fn the_screener_holds_first_time_senders_until_decided(cx: &mut TestAppContext) {
    let (store, id) = inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    act(
        &view,
        Action::OpenMailbox(id.clone(), Folder::Inbox.into()),
        cx,
    );
    let everyone = view.read_with(cx, |this, _| listed(this).len());
    act(&view, Action::Control(ControlAction::ToggleScreener), cx);
    // The sample mail is older than the Screener: as if it had all just
    // arrived.
    view.update(cx, |this, cx| {
        this.control.screener.since = 0;
        this.filter(cx);
    });
    cx.run_until_parked();
    frame(cx);
    let known = view.read_with(cx, |this, _| {
        assert!(this.sorting.known_ready());
        // Only Sophie, written to before, is known: everyone else waits
        // in the Screener.
        assert_eq!(listed(this), ["Sophie Chen"]);
        assert_eq!(this.screened_waiting, everyone - 1);
        listed(this).len()
    });
    assert!(cx.debug_bounds("screener-waiting").is_some());
    assert!(cx.debug_bounds("sidebar-screener").is_some());
    act(
        &view,
        Action::Control(ControlAction::Lens(Lens::Screener)),
        cx,
    );
    frame(cx);
    assert!(cx.debug_bounds("screener-approve").is_some());
    let (alex, linear, figma) = view.read_with(cx, |this, _| {
        assert_eq!(listed(this).len(), everyone - known);
        assert!(!listed(this).contains(&"Sophie Chen".to_owned()));
        (
            id_of(this, "Alex Morgan"),
            id_of(this, "Linear"),
            id_of(this, "Figma"),
        )
    });
    // Waiting mail isn't announced.
    view.read_with(cx, |this, _| {
        let m = this.messages.iter().find(|m| m.id == alex).unwrap();
        assert!(this.screened(m) && !this.announces(m));
    });
    // Approve: their mail joins the inbox.
    act(
        &view,
        Action::Control(ControlAction::Approve(vec![alex.clone()])),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(!listed(this).contains(&"Alex Morgan".to_owned()));
        assert!(this.control.approved("alex@studio.design"));
    });
    // Route to a category: approved and sorted there from now on.
    act(
        &view,
        Action::Control(ControlAction::Route(
            vec![linear.clone()],
            Category::Updates,
        )),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(!listed(this).contains(&"Linear".to_owned()));
        assert_eq!(
            this.sorting.prefs().sender_choice("updates@linear.app"),
            Some(Category::Updates)
        );
    });
    // Block: their mail goes to Trash.
    act(
        &view,
        Action::Control(ControlAction::Block(vec![figma.clone()])),
        cx,
    );
    view.read_with(cx, |this, _| assert!(this.org.is_blocked("news@figma.com")));
    // Back in the inbox: the approved sender's mail is there.
    act(&view, Action::Control(ControlAction::Lens(Lens::Inbox)), cx);
    view.read_with(cx, |this, _| {
        assert!(listed(this).contains(&"Alex Morgan".to_owned()))
    });
    // Turned off, nothing is held back.
    act(&view, Action::Control(ControlAction::ToggleScreener), cx);
    view.read_with(cx, |this, _| assert_eq!(this.screened_waiting, 0));
}

#[gpui::test]
fn settings_show_vips_screener_and_quiet_hours(cx: &mut TestAppContext) {
    let (store, _) = inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(1000.)));
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::InboxControl), cx);
    frame(cx);
    for s in [
        "add-vip",
        "screener-on",
        "notify-mode-0",
        "quiet-on",
        "quiet-pause-hour",
    ] {
        assert!(cx.debug_bounds(s).is_some(), "{s}");
    }
    type_in(&view, |this| &this.control_ui.vip, "Boss@Work.example", cx);
    act(&view, Action::Control(ControlAction::AddVip), cx);
    act(&view, Action::Control(ControlAction::ToggleQuiet), cx);
    type_in(&view, |this| &this.control_ui.quiet_start, "12:00", cx);
    type_in(&view, |this| &this.control_ui.quiet_end, "1pm", cx);
    act(
        &view,
        Action::Control(ControlAction::NewDays(
            crate::manage::inbox_control::WEEKDAYS,
        )),
        cx,
    );
    act(&view, Action::Control(ControlAction::AddWindow), cx);
    act(&view, Action::Control(ControlAction::ToggleDay(1, 0)), cx);
    frame(cx);
    assert!(cx.debug_bounds("vip-row-0").is_some());
    assert!(cx.debug_bounds("quiet-row-1").is_some());
    view.read_with(cx, |this, _| {
        assert_eq!(this.control.vips, ["boss@work.example"]);
        let q = &this.control.notifications.quiet;
        assert!(q.enabled);
        assert_eq!((q.windows[1].start, q.windows[1].end), (12 * 60, 13 * 60));
        assert_eq!(
            q.windows[1].days,
            crate::manage::inbox_control::WEEKDAYS & !1,
            "Monday turned off"
        );
    });
    // A time that isn't one is refused.
    type_in(&view, |this| &this.control_ui.quiet_start, "lunch", cx);
    act(&view, Action::Control(ControlAction::AddWindow), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.control.notifications.quiet.windows.len(), 2)
    });
    act(&view, Action::Control(ControlAction::Pause(60)), cx);
    frame(cx);
    assert!(cx.debug_bounds("quiet-resume").is_some());
    let saved = crate::manage::inbox_control::InboxControl::load(&store);
    assert!(
        saved
            .notifications
            .quiet
            .paused(chrono::Utc::now().timestamp())
    );
}

#[gpui::test]
fn a_rule_of_several_conditions_and_actions_is_put_together(cx: &mut TestAppContext) {
    let (store, _) = inbox();
    let (view, cx) = open(store.clone(), cx);
    cx.simulate_resize(size(px(1440.), px(1200.)));
    act(&view, Action::Settings, cx);
    act(&view, Action::SettingsPage(Page::Organizing), cx);
    use organize::OrgAction as O;
    // From ends with @shop.example…
    act(&view, Action::Organize(O::RuleField(RuleField::From)), cx);
    act(&view, Action::Organize(O::RuleOp(RuleOp::EndsWith)), cx);
    type_in(&view, |this| &this.org_ui.rule_text, "@shop.example", cx);
    act(&view, Action::Organize(O::AddCondition), cx);
    // …and has attachments…
    act(
        &view,
        Action::Organize(O::RuleField(RuleField::Attachment)),
        cx,
    );
    act(&view, Action::Organize(O::AddCondition), cx);
    // …a pattern that doesn't compile is refused.
    act(
        &view,
        Action::Organize(O::RuleField(RuleField::Subject)),
        cx,
    );
    act(&view, Action::Organize(O::RuleOp(RuleOp::Regex)), cx);
    type_in(&view, |this| &this.org_ui.rule_text, "(", cx);
    act(&view, Action::Organize(O::AddCondition), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.org_ui.draft.conditions.len(), 2)
    });
    type_in(&view, |this| &this.org_ui.rule_text, "", cx);
    // Label, mark read, archive.
    act(
        &view,
        Action::Organize(O::RuleKind(organize::RuleKind::Label)),
        cx,
    );
    type_in(&view, |this| &this.org_ui.rule_name, "Receipts", cx);
    act(&view, Action::Organize(O::AddAction), cx);
    act(
        &view,
        Action::Organize(O::RuleKind(organize::RuleKind::MarkRead)),
        cx,
    );
    act(&view, Action::Organize(O::AddAction), cx);
    act(
        &view,
        Action::Organize(O::RuleKind(organize::RuleKind::Archive)),
        cx,
    );
    act(&view, Action::Organize(O::AddAction), cx);
    // A second move is refused.
    act(
        &view,
        Action::Organize(O::RuleKind(organize::RuleKind::Trash)),
        cx,
    );
    act(&view, Action::Organize(O::AddAction), cx);
    frame(cx);
    assert!(cx.debug_bounds("rule-condition-1").is_some());
    assert!(cx.debug_bounds("rule-action-2").is_some());
    assert!(
        cx.debug_bounds("rule-any-0").is_some(),
        "all or any, with two conditions"
    );
    act(&view, Action::Organize(O::SaveRule), cx);
    view.read_with(cx, |this, _| {
        let rule = &this.org.rules[0];
        assert_eq!(rule.conditions.len(), 2);
        assert_eq!(
            rule.actions,
            [
                RuleAction::Label("Receipts".into()),
                RuleAction::MarkRead,
                RuleAction::Archive
            ]
        );
        assert!(rule.enabled);
        assert!(
            this.org_ui.draft.conditions.is_empty(),
            "the form is cleared"
        );
    });
    // Edited: any condition is enough now.
    act(&view, Action::Organize(O::EditRule(0)), cx);
    act(&view, Action::Organize(O::RuleAny(true)), cx);
    act(&view, Action::Organize(O::RemoveAction(1)), cx);
    act(&view, Action::Organize(O::SaveRule), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.org.rules.len(), 1);
        assert!(this.org.rules[0].any);
        assert_eq!(this.org.rules[0].actions.len(), 2);
    });
    let saved = crate::manage::organize::Organizing::load(&store);
    assert_eq!(
        saved.rules,
        view.read_with(cx, |this, _| this.org.rules.clone())
    );
}
