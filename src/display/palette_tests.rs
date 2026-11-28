//! The command palette in the window: ⌘K (Ctrl+K) opens it over the
//! window; typing finds commands, accounts, folders, contacts and
//! templates; choosing one runs it; quick actions list their own choices
//! for the open or selected messages.
use super::palette::{Kind, Mode, PaletteAction};
use super::tests::{act, frame, two_accounts};
use super::*;
use std::prelude::v1::test;

fn palette(action: PaletteAction) -> Action {
    Action::Palette(action)
}

fn typed(view: &Entity<MailApp>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_owned();
    view.update_in(cx, |this, w, cx| {
        MailApp::set(&this.palette.input, text, w, cx)
    });
    cx.run_until_parked();
}

/// The results shown: what each is, its title and detail.
fn shown(view: &Entity<MailApp>, cx: &mut VisualTestContext) -> Vec<(Kind, String, String)> {
    view.read_with(cx, |this, _| {
        this.palette
            .found
            .iter()
            .map(|f| {
                let item = &this.palette.items[f.item];
                (item.kind, item.title.to_string(), item.detail.to_string())
            })
            .collect()
    })
}

/// Chooses the first result titled `title`.
fn choose(view: &Entity<MailApp>, title: &str, cx: &mut VisualTestContext) {
    let row = shown(view, cx)
        .iter()
        .position(|(_, t, _)| t == title)
        .unwrap_or_else(|| panic!("{title} isn't listed: {:?}", shown(view, cx)));
    act(view, palette(PaletteAction::Choose(row)), cx);
}

fn opened(cx: &mut TestAppContext) -> (Entity<MailApp>, &mut VisualTestContext, String, String) {
    let (view, cx, _) = two_accounts(cx);
    cx.simulate_resize(size(px(1440.), px(900.)));
    let (work, home) = view.read_with(cx, |this, _| {
        (this.accounts[0].id.clone(), this.accounts[1].id.clone())
    });
    act(
        &view,
        Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
        cx,
    );
    (view, cx, work, home)
}

#[gpui::test]
fn the_palette_finds_and_opens_folders_and_accounts(cx: &mut TestAppContext) {
    let (view, cx, _work, home) = opened(cx);
    act(&view, palette(PaletteAction::Toggle), cx);
    frame(cx);
    // Below the title bar, centered, inside the window; its rows inside it.
    let panel = cx.debug_bounds("palette").expect("the palette is shown");
    assert!(
        panel.top() >= px(TITLEBAR_HEIGHT) && panel.bottom() <= px(900.),
        "{panel:?}"
    );
    assert!((panel.center().x - px(720.)).abs() < px(2.), "{panel:?}");
    assert_eq!(panel.size.width, px(640.));
    let row = cx.debug_bounds("palette-row-0").expect("results are shown");
    assert!(
        row.left() >= panel.left() && row.right() <= panel.right(),
        "{row:?}"
    );
    let list = cx.debug_bounds("palette-list").unwrap();
    assert!(list.bottom() <= panel.bottom(), "{list:?}");

    // Before typing: commands with their shortcuts, accounts and folders,
    // but no contacts (there may be thousands).
    let all = shown(&view, cx);
    assert!(
        all.iter()
            .any(|(k, t, _)| *k == Kind::Account && t == "Work")
    );
    assert!(
        all.iter()
            .any(|(k, t, d)| *k == Kind::Folder && t == "Receipts" && d == "home@example.com")
    );
    assert!(!all.iter().any(|(k, ..)| *k == Kind::Contact));
    assert!(
        !all.iter().any(|(k, ..)| *k == Kind::Quick),
        "no message is open"
    );
    view.read_with(cx, |this, _| {
        let reply = this
            .palette
            .items
            .iter()
            .find(|i| i.title.as_ref() == t!("shortcuts-reply"))
            .unwrap();
        assert!(matches!(reply.run, palette::Run::Command(_)));
        // Composer-only commands (formatting) aren't listed; nor is the palette.
        assert!(
            !this
                .palette
                .items
                .iter()
                .any(|i| i.title.as_ref() == t!("shortcuts-bold"))
        );
        assert!(
            !this
                .palette
                .items
                .iter()
                .any(|i| i.title.as_ref() == t!("shortcuts-command-palette"))
        );
    });

    // A folder by a few letters of its name and of its account.
    typed(&view, "rcpt hom", cx);
    assert_eq!(
        shown(&view, cx)[0],
        (Kind::Folder, "Receipts".into(), "home@example.com".into())
    );
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.palette.open);
        assert_eq!(this.account.as_ref(), Some(&home));
        assert_eq!(this.mailbox(), Mailbox::Remote("INBOX/Receipts".into()));
    });

    // An account opens its inbox.
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "work", cx);
    assert_eq!(shown(&view, cx)[0].1, "Work");
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, _| {
        assert_ne!(this.account.as_ref(), Some(&home));
        assert_eq!(this.folder, Folder::Inbox);
    });

    // Escape closes it; ⌘K again too.
    act(&view, palette(PaletteAction::Toggle), cx);
    act(&view, Action::Close, cx);
    view.read_with(cx, |this, _| assert!(!this.palette.open));
    act(&view, palette(PaletteAction::Toggle), cx);
    act(&view, palette(PaletteAction::Toggle), cx);
    view.read_with(cx, |this, _| assert!(!this.palette.open));
    frame(cx);
    assert!(cx.debug_bounds("palette").is_none());
}

#[gpui::test]
fn quick_actions_move_snooze_and_label_the_open_message(cx: &mut TestAppContext) {
    let (view, cx, work, _home) = opened(cx);
    // A message only in the list, not the cache: there's no server here,
    // and a move it refuses puts back what the cache has.
    let first = "moved".to_owned();
    view.update(cx, |this, cx| {
        let mut m = this.messages[0].clone();
        m.id = first.clone();
        m.message_id = "<moved@example.com>".into();
        this.messages.insert(0, m);
        this.filter(cx);
        // Opened as is: opening it through `select` reads the list again
        // from the cache, which hasn't it.
        this.selected = Some(first.clone());
    });

    // Move to…: its own list of folders, not the one it's in.
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "move", cx);
    let top = &shown(&view, cx)[0];
    assert_eq!(
        (top.0, top.1.as_str()),
        (Kind::Quick, t!("palette-move-to"))
    );
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, cx| {
        assert!(this.palette.open);
        assert_eq!(this.palette.mode, Mode::MoveTo);
        assert_eq!(
            MailApp::value(&this.palette.input, cx),
            "",
            "the field starts empty"
        );
    });
    let choices: Vec<String> = shown(&view, cx).into_iter().map(|(_, t, _)| t).collect();
    assert!(
        choices.contains(&Folder::Archive.label().to_owned()),
        "{choices:?}"
    );
    assert!(choices.contains(&"Receipts".to_owned()), "{choices:?}");
    assert!(
        !choices.contains(&Folder::Inbox.label().to_owned()),
        "it's in the inbox"
    );
    frame(cx);
    assert!(cx.debug_bounds("palette-back").is_some());

    // Escape goes back to everything, then closes.
    act(&view, Action::Close, cx);
    view.read_with(cx, |this, _| {
        assert!(this.palette.open);
        assert_eq!(this.palette.mode, Mode::All);
    });
    choose(&view, t!("palette-move-to"), cx);
    typed(&view, "rec", cx);
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.palette.open);
        assert!(
            !this.messages.iter().any(|m| m.id == first),
            "moved out of the inbox"
        );
    });

    // Snooze until…: the dialog's choices, and the dialog for another time.
    let second = format!("{work}:demo-1");
    act(&view, Action::Select(second.clone()), cx);
    act(&view, palette(PaletteAction::Toggle), cx);
    choose(&view, t!("palette-snooze-until"), cx);
    let choices = shown(&view, cx);
    assert!(
        choices
            .iter()
            .any(|(_, t, _)| t == t!("manage-composing-tomorrow")),
        "{choices:?}"
    );
    choose(&view, t!("palette-pick-a-date-and-time"), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.palette.open);
        assert!(this.modal == Some(Modal::Snooze(vec![second.clone()])));
    });
    act(&view, Action::Close, cx);

    // Apply label…: the label menu's choices, a new label among them.
    act(&view, palette(PaletteAction::Toggle), cx);
    choose(&view, t!("palette-apply-label"), cx);
    view.read_with(cx, |this, _| assert_eq!(this.palette.mode, Mode::Label));
    choose(&view, t!("ui-new-label"), cx);
    view.read_with(cx, |this, _| {
        assert!(!this.palette.open);
        assert!(matches!(
            this.modal,
            Some(Modal::NewLabel(MoveTarget::Mail(_)))
        ));
    });
}

#[gpui::test]
fn commands_contacts_and_templates_run_from_the_palette(cx: &mut TestAppContext) {
    let (view, cx, _work, _home) = opened(cx);

    // A command runs as its shortcut would.
    let collapsed = view.read_with(cx, |this, _| this.sidebar_collapsed);
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "sidebar", cx);
    assert_eq!(shown(&view, cx)[0].1, t!("shortcuts-show-hide-sidebar"));
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, _| assert_ne!(this.sidebar_collapsed, collapsed));

    // A contact: a new message to them.
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "sophie", cx);
    let top = shown(&view, cx)[0].clone();
    assert_eq!(
        top,
        (
            Kind::Contact,
            "Sophie Chen".into(),
            "sophie@example.com".into()
        )
    );
    let before = view.read_with(cx, |this, _| this.composers.len());
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.read_with(cx, |this, cx| {
        assert_eq!(this.composers.len(), before + 1);
        let draft = &this.composers.last().unwrap().read(cx).draft;
        assert!(draft.to.contains("sophie@example.com"), "{}", draft.to);
    });

    // A template: a new message written from it.
    view.update(cx, |this, _| {
        this.compose_options
            .templates
            .push(crate::manage::composing::Template {
                name: "Thanks".into(),
                subject: "Thanks for applying".into(),
                body: "Hi, thanks for applying.".into(),
            });
    });
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "thanks", cx);
    assert_eq!(shown(&view, cx)[0].0, Kind::Template);
    act(&view, palette(PaletteAction::Choose(0)), cx);
    view.update(cx, |this, cx| {
        assert_eq!(this.composers.len(), before + 2);
        let draft = this
            .composers
            .last()
            .unwrap()
            .update(cx, |c, cx| c.snapshot(cx));
        assert_eq!(draft.subject, "Thanks for applying");
        assert!(draft.body.contains("thanks for applying"), "{}", draft.body);
    });

    // Nothing matching says so; the highlight wraps around.
    act(&view, palette(PaletteAction::Toggle), cx);
    typed(&view, "zzqqxx", cx);
    frame(cx);
    assert!(cx.debug_bounds("palette-empty").is_some());
    typed(&view, "", cx);
    act(&view, palette(PaletteAction::Step(-1)), cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.palette.chosen, this.palette.found.len() - 1)
    });
    act(&view, palette(PaletteAction::Close), cx);

    // Not over a dialog.
    act(&view, Action::About, cx);
    act(&view, palette(PaletteAction::Toggle), cx);
    view.read_with(cx, |this, _| assert!(!this.palette.open));
}

/// ⌘K (Ctrl+K) opens the palette in the window; ↓ and Enter in its field
/// choose; Escape closes it. In a composer ⌘K is still Insert Link.
#[gpui::test]
fn the_keyboard_drives_the_palette(cx: &mut TestAppContext) {
    let (view, cx, _work, home) = opened(cx);
    cx.update(|_, cx| {
        let bindings = super::app_bindings(cx);
        cx.bind_keys(bindings);
    });
    view.update_in(cx, |this, w, cx| w.focus(&this.focus, cx));
    cx.simulate_keystrokes("secondary-k");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.palette.open));

    cx.simulate_input("receipts");
    cx.run_until_parked();
    // Both accounts' Receipts, in the sidebar's order.
    let found = shown(&view, cx);
    assert_eq!(
        found[0],
        (Kind::Folder, "Receipts".into(), "work@example.com".into())
    );
    assert_eq!(
        found[1],
        (Kind::Folder, "Receipts".into(), "home@example.com".into())
    );
    cx.simulate_keystrokes("down");
    view.read_with(cx, |this, _| assert_eq!(this.palette.chosen, 1));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(!this.palette.open);
        assert_eq!(this.mailbox(), Mailbox::Remote("INBOX/Receipts".into()));
        assert_eq!(this.account.as_ref(), Some(&home));
    });

    cx.simulate_keystrokes("secondary-k");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.palette.open));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(!this.palette.open));

    // The two ⌘K shortcuts are in different places, so they don't clash.
    let all = shortcuts::bindable();
    let custom = HashMap::new();
    let b = all.iter().find(|b| b.name == "command-palette").unwrap();
    assert!(shortcuts::clashes(b, &shortcuts::effective(b, &custom), &all, &custom).is_empty());
}
