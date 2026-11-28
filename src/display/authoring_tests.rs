//! The composer's writing aids: Markdown typed as shortcuts, the slash
//! menu, tables, find and replace, colors and fonts, emoji and mail merge.
use super::tests::{frame, two_accounts};
use super::*;
use crate::manage::richtext::{Doc, Pos};
use std::prelude::v1::test;

/// A new formatted draft, its editor empty and focused, with the app's keys.
fn formatted(cx: &mut TestAppContext) -> (Entity<Composer>, &mut VisualTestContext) {
    let (view, cx, _) = two_accounts(cx);
    cx.update(|_, cx| {
        let bindings = app_bindings(cx);
        cx.bind_keys(bindings);
    });
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    composer.update_in(cx, |c, w, cx| {
        c.set_formatted(true, w, cx);
        c.rich.update(cx, |r, cx| {
            r.set_doc(Doc::empty(), cx);
            r.focus(w, cx);
        });
    });
    frame(cx);
    (composer, cx)
}

/// Types `text` a character at a time, as a keyboard does.
fn type_chars(cx: &mut VisualTestContext, text: &str) {
    for ch in text.chars() {
        cx.simulate_input(&ch.to_string());
    }
    frame(cx);
}

fn html(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    composer.read_with(cx, |c, cx| c.rich.read(cx).html())
}

/// "- ", "1. ", "> " and "## " start lists, quotes and headings; markers
/// around words format them; "---" and Enter makes a rule. Undo puts the
/// Markdown back as typed. Turned off in Settings, Markdown stays text.
#[gpui::test]
fn markdown_typed_formats_as_it_is_typed(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    type_chars(cx, "- one");
    assert_eq!(html(&composer, cx), "<ul><li>one</li></ul>");
    cx.simulate_keystrokes("enter enter");
    type_chars(cx, "3. three");
    assert_eq!(html(&composer, cx), "<ul><li>one</li></ul><ol start=\"3\"><li>three</li></ol>");
    cx.simulate_keystrokes("enter enter");
    type_chars(cx, "> said");
    assert!(html(&composer, cx).ends_with("<blockquote><div>said</div></blockquote>"), "{}", html(&composer, cx));
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "## Title");
    assert_eq!(html(&composer, cx), "<h2>Title</h2>");
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "a **bold** and *slanted* `code` ~~gone~~ more");
    assert_eq!(html(&composer, cx), "<div>a <b>bold</b> and <i>slanted</i> <code>code</code> <s>gone</s> more</div>");
    // Undone, the markers come back (and the text typed after goes first).
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "**hi**");
    assert_eq!(html(&composer, cx), "<div><b>hi</b></div>");
    cx.simulate_keystrokes("secondary-z");
    frame(cx);
    assert_eq!(html(&composer, cx), "<div>**hi**</div>");
    // A rule.
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "---");
    cx.simulate_keystrokes("enter");
    type_chars(cx, "after");
    assert_eq!(html(&composer, cx), "<hr><div>after</div>");
    // Off: typed as it is.
    composer.update_in(cx, |c, _, cx| {
        c.options.markdown_shortcuts = false;
        c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx));
    });
    frame(cx);
    type_chars(cx, "- **x**");
    assert_eq!(html(&composer, cx), "<div>-&nbsp;**x**</div>".replace("&nbsp;", " "));
}

/// "/" opens the slash menu; typing narrows it, Enter runs the choice
/// (a table here, Tab moving between its cells and adding a row after the
/// last), and the "/" and what followed go. Escape closes it.
#[gpui::test]
fn the_slash_menu_inserts_blocks_and_tables(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    type_chars(cx, "/tab");
    assert!(cx.debug_bounds("rich-slash-menu").is_some());
    cx.simulate_keystrokes("enter");
    frame(cx);
    assert!(cx.debug_bounds("rich-slash-menu").is_none());
    let (rows, cols) = composer.read_with(cx, |c, cx| {
        let r = c.rich.read(cx);
        let t = r.doc.table_at(r.sel.head.block).expect("in a table");
        (t.rows.len(), t.columns())
    });
    assert_eq!((rows, cols), (2, 3));
    type_chars(cx, "a");
    cx.simulate_keystrokes("tab");
    type_chars(cx, "b");
    // Backspace at a cell's start doesn't join it to the cell before.
    cx.simulate_keystrokes("backspace backspace backspace");
    type_chars(cx, "c");
    let cells = |cx: &mut VisualTestContext| {
        composer.read_with(cx, |c, cx| {
            let r = c.rich.read(cx);
            let t = r.doc.table_at(0).expect("a table");
            t.rows.iter().map(|row| row.cells.iter().map(|cell| r.doc.block(cell.blocks.start).text()).collect::<Vec<_>>()).collect::<Vec<_>>()
        })
    };
    assert_eq!(cells(cx), [vec!["a", "c", ""], vec!["", "", ""]]);
    // From the last cell, Tab adds a row.
    cx.simulate_keystrokes("tab tab tab tab tab");
    frame(cx);
    assert_eq!(cells(cx).len(), 3);
    // Escape closes the menu, leaving what was typed.
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "x /qu");
    assert!(cx.debug_bounds("rich-slash-menu").is_some());
    cx.simulate_keystrokes("escape");
    frame(cx);
    assert!(cx.debug_bounds("rich-slash-menu").is_none());
    assert_eq!(html(&composer, cx), "<div>x /qu</div>");
    // A quote, from the menu.
    composer.update_in(cx, |c, _, cx| c.rich.update(cx, |r, cx| r.set_doc(Doc::empty(), cx)));
    type_chars(cx, "/quote");
    cx.simulate_keystrokes("enter");
    type_chars(cx, "q");
    assert_eq!(html(&composer, cx), "<blockquote><div>q</div></blockquote>");
}

/// The table menu's grid inserts a table of its size; its rows and columns
/// come and go, and a column widened is written in the HTML.
#[gpui::test]
fn tables_grow_shrink_and_take_widths(cx: &mut TestAppContext) {
    use super::rich_editor::TableOp;
    let (composer, cx) = formatted(cx);
    composer.update_in(cx, |c, w, cx| c.rich.update(cx, |r, cx| r.insert_table(2, 2, Some(w), cx)));
    frame(cx);
    composer.update_in(cx, |c, w, cx| {
        c.rich.update(cx, |r, cx| {
            r.table_op(TableOp::ColumnRight, Some(w), cx);
            r.table_op(TableOp::RowBelow, Some(w), cx);
            r.table_op(TableOp::Widen, Some(w), cx);
        })
    });
    frame(cx);
    let (rows, cols, html) = composer.read_with(cx, |c, cx| {
        let r = c.rich.read(cx);
        let t = r.doc.table_at(0).unwrap();
        (t.rows.len(), t.columns(), r.html())
    });
    assert_eq!((rows, cols), (3, 3));
    assert!(html.contains("width:43.3%"), "{html}");
    // The toolbar's table button shows the table's menu.
    composer.update_in(cx, |c, _, cx| {
        c.rich.update(cx, |r, _| r.sel = Default::default());
        c.table_open_for_test();
        cx.notify();
    });
    frame(cx);
    assert!(cx.debug_bounds("table-menu").is_some());
    assert!(cx.debug_bounds("table-delete-row").is_some());
    composer.update_in(cx, |c, w, cx| c.rich.update(cx, |r, cx| r.table_op(TableOp::DeleteTable, Some(w), cx)));
    assert!(!composer.read_with(cx, |c, cx| c.rich.read(cx).in_table()));
}

/// Find highlights and selects matches, steps through them, and replaces
/// one or all, in a formatted draft and in a plain one.
#[gpui::test]
fn find_and_replace_in_both_editors(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    type_chars(cx, "Cat, cat and CAT");
    cx.simulate_keystrokes("secondary-f");
    frame(cx);
    assert!(composer.read_with(cx, |c, _| c.find_open()));
    assert!(cx.debug_bounds("compose-find").is_some());
    composer.update_in(cx, |c, w, cx| c.find_typed("cat", "dog", w, cx));
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |c, cx| c.find_count(cx)), 3);
    composer.update_in(cx, |c, w, cx| c.find_next(w, cx));
    let selected = composer.read_with(cx, |c, cx| c.rich.read(cx).selected_text());
    assert!(selected.eq_ignore_ascii_case("cat"));
    composer.update_in(cx, |c, w, cx| c.replace_found(false, w, cx));
    assert_eq!(composer.read_with(cx, |c, cx| c.find_count(cx)), 2);
    composer.update_in(cx, |c, w, cx| c.replace_found(true, w, cx));
    assert_eq!(html(&composer, cx), "<div>dog, dog and dog</div>");
    // One step to undo.
    composer.update_in(cx, |c, w, cx| c.rich.update(cx, |r, cx| r.undo(Some(w), cx)));
    assert_eq!(composer.read_with(cx, |c, cx| c.rich.read(cx).plain()), "Cat, dog and CAT");
    // Plain text.
    composer.update_in(cx, |c, w, cx| {
        c.set_formatted(false, w, cx);
        c.body.update(cx, |s, cx| s.set_text("one two one", w, cx));
        c.find_typed("one", "1", w, cx);
    });
    cx.run_until_parked();
    assert_eq!(composer.read_with(cx, |c, cx| c.find_count(cx)), 2);
    composer.update_in(cx, |c, w, cx| c.replace_found(true, w, cx));
    assert_eq!(composer.read_with(cx, |c, cx| c.body.read(cx).value().to_string()), "1 two 1");
    assert_eq!(composer.read_with(cx, |c, _| c.find_current()), Some(0));
}

/// Colors, highlights, fonts and sizes apply to the selection, and are in
/// the HTML sent; the text of another size is laid out at its size.
#[gpui::test]
fn colors_fonts_and_sizes_style_the_selection(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    type_chars(cx, "big red");
    composer.update_in(cx, |c, w, cx| {
        c.rich.update(cx, |r, cx| {
            r.select_range(Pos::new(0, 4), Pos::new(0, 7), cx);
            r.set_color(Some(crate::manage::richtext::Color::rgb(0xd9, 0x30, 0x25)), Some(w), cx);
            r.set_highlight(Some(crate::manage::richtext::Color::rgb(0xff, 0xf4, 0x75)), Some(w), cx);
            r.select_range(Pos::new(0, 0), Pos::new(0, 3), cx);
            r.set_size(Some(32.), Some(w), cx);
            r.set_family(Some("Georgia".into()), Some(w), cx);
        })
    });
    frame(cx);
    let out = html(&composer, cx);
    assert!(out.contains("color:#d93025") && out.contains("background-color:#fff475"), "{out}");
    assert!(out.contains("font-family:Georgia") && out.contains("font-size:32px"), "{out}");
    let active = composer.read_with(cx, |c, cx| c.rich.read(cx).active());
    assert_eq!(active.size, Some(32.));
    assert_eq!(active.family.as_deref(), Some("Georgia"));
    // The line is as tall as its largest text.
    let height = composer.read_with(cx, |c, cx| c.rich.read(cx).block_rects()[0].size.height);
    assert!(height >= px(32.), "{height:?}");
    // The menu shows.
    composer.update_in(cx, |c, _, cx| {
        c.style_open_for_test();
        cx.notify();
    });
    frame(cx);
    assert!(cx.debug_bounds("style-menu").is_some());
    assert!(cx.debug_bounds("text-color-2").is_some());
}

/// Emoji go where the caret is, in either editor; "/emoji" opens the
/// picker.
#[gpui::test]
fn emoji_are_picked_into_the_text(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    type_chars(cx, "Hi ");
    composer.update_in(cx, |c, w, cx| c.toggle_emoji(w, cx));
    frame(cx);
    assert!(cx.debug_bounds("emoji-menu").is_some());
    composer.update_in(cx, |c, w, cx| c.pick("👋", w, cx));
    assert_eq!(composer.read_with(cx, |c, cx| c.rich.read(cx).plain()), "Hi 👋");
    assert!(!composer.read_with(cx, |c, _| c.emoji_open()));
    type_chars(cx, " /emo");
    cx.simulate_keystrokes("enter");
    frame(cx);
    assert!(composer.read_with(cx, |c, _| c.emoji_open()));
    composer.update_in(cx, |c, w, cx| {
        c.pick("🎉", w, cx);
        c.set_formatted(false, w, cx);
        c.body.update(cx, |s, cx| s.set_text("Plain ", w, cx));
        c.body.update(cx, |s, cx| s.set_selected_range(6..6, cx));
        c.pick("👍", w, cx);
    });
    assert_eq!(composer.read_with(cx, |c, cx| c.body.read(cx).value().to_string()), "Plain 👍");
}

/// Mail merge makes a message per row of the spreadsheet with an
/// address, its placeholders filled in, and sending them asks the app to.
#[gpui::test]
fn mail_merge_personalizes_a_message_per_row(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    composer.update_in(cx, |c, w, cx| {
        c.fill("", "Hello {first_name}", w, cx);
        c.rich.update(cx, |r, cx| r.insert_text("Dear {first_name} of {company},", Some(w), cx));
        c.merge_csv(b"First Name,Email,Company\nAna,ana@example.org,Acme\nBo,,Nowhere\nCy,cy@example.org,Beta\n");
        cx.notify();
    });
    frame(cx);
    assert!(cx.debug_bounds("merge-panel").is_some());
    assert!(cx.debug_bounds("merge-placeholder-2").is_some());
    let (drafts, skipped) = composer.update(cx, |c, cx| c.merge_drafts(cx));
    assert_eq!(skipped, 1);
    assert_eq!(drafts.len(), 2);
    assert_eq!(drafts[0].to, "Ana <ana@example.org>");
    assert_eq!(drafts[0].subject, "Hello Ana");
    assert!(drafts[0].html.contains("Dear Ana of Acme,"), "{}", drafts[0].html);
    assert_eq!(drafts[1].body, "Dear Cy of Beta,");
    assert_ne!(drafts[0].id, drafts[1].id);
}

/// Snippets go where the caret is, from the "/" menu or the templates
/// menu, their placeholders filled; a selection is kept as one.
#[gpui::test]
fn snippets_are_inserted_and_saved(cx: &mut TestAppContext) {
    let (view, cx, _) = two_accounts(cx);
    cx.update(|_, cx| {
        let bindings = app_bindings(cx);
        cx.bind_keys(bindings);
    });
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    composer.update_in(cx, |c, w, cx| {
        c.options.snippets = vec![crate::manage::composing::Snippet { name: "Office hours".into(), body: "**Mon–Fri**, 9–5 ({weekday})".into() }];
        c.set_formatted(true, w, cx);
        c.rich.update(cx, |r, cx| {
            r.set_doc(Doc::empty(), cx);
            r.focus(w, cx);
        });
    });
    frame(cx);
    type_chars(cx, "Hours: /office");
    cx.simulate_keystrokes("enter");
    frame(cx);
    let out = html(&composer, cx);
    assert!(out.starts_with("<div>Hours: <b>Mon–Fri</b>, 9–5 ("), "{out}");
    assert!(!out.contains("{weekday}") && !out.contains("/office"), "{out}");
    // Plain text, from the templates menu's list.
    composer.update_in(cx, |c, w, cx| {
        c.set_formatted(false, w, cx);
        c.body.update(cx, |s, cx| s.set_text("", w, cx));
        c.insert_snippet(0, w, cx);
    });
    let body = composer.read_with(cx, |c, cx| c.body.read(cx).value().to_string());
    assert!(body.starts_with("**Mon–Fri**, 9–5 ("), "{body}");
    // A selection kept as a snippet lands in Settings.
    composer.update_in(cx, |c, w, cx| {
        c.body.update(cx, |s, cx| {
            s.set_text("Best regards, Sam", w, cx);
            s.set_selected_range(0..13, cx);
        });
        c.save_selection_as_snippet(w, cx);
    });
    cx.run_until_parked();
    let kept = view.read_with(cx, |this, _| this.compose_options.snippets.clone());
    assert!(kept.iter().any(|s| s.body == "Best regards," && s.name == "Best regards,"), "{kept:?}");
}

/// In a plain message, find's matches are highlighted by the text field
/// itself, until find closes.
#[gpui::test]
fn plain_text_matches_are_highlighted(cx: &mut TestAppContext) {
    let (composer, cx) = formatted(cx);
    composer.update_in(cx, |c, w, cx| {
        c.set_formatted(false, w, cx);
        c.body.update(cx, |s, cx| s.set_text("one two one", w, cx));
        c.find_typed("one", "", w, cx);
    });
    cx.run_until_parked();
    let active = |cx: &mut VisualTestContext| composer.read_with(cx, |c, cx| c.body.read(cx).search_session().is_active());
    assert!(active(cx));
    composer.update_in(cx, |c, w, cx| c.find_next(w, cx));
    assert_eq!(composer.read_with(cx, |c, cx| c.body.read(cx).selected_range()), 8..11);
    composer.update_in(cx, |c, w, cx| c.close_find(w, cx));
    assert!(!active(cx));
}

/// A mail merge goes on when its composer is closed; what couldn't be
/// sent is said in a notice, and the draft stays.
#[gpui::test]
fn a_mail_merge_goes_on_with_its_composer_closed(cx: &mut TestAppContext) {
    let (view, cx, store) = two_accounts(cx);
    view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
    let composer = view.read_with(cx, |this, _| this.composers[0].clone());
    composer.update_in(cx, |c, w, cx| {
        c.fill("", "Hi {first_name}", w, cx);
        c.merge_csv(b"first_name,email\nAna,ana@example.org\nBo,bo@example.org\n");
    });
    let (drafts, _) = composer.update(cx, |c, cx| c.merge_drafts(cx));
    let id = composer.read_with(cx, |c, _| c.draft.id.clone());
    composer.update(cx, |_, cx| cx.emit(super::composer::ComposerEvent::Merge(drafts)));
    cx.run_until_parked();
    composer.update(cx, |_, cx| cx.emit(super::composer::ComposerEvent::Close));
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    // The test servers take no mail: both fail, said once all were tried.
    let (status, error, open) = view.read_with(cx, |this, _| (this.status.clone(), this.error, this.composers.len()));
    assert_eq!(open, 0);
    assert!(error && status.contains("Sent 0 of 2") && status.contains("ana@example.org") && status.contains("bo@example.org"), "{status}");
    let accounts = view.read_with(cx, |this, _| this.accounts.clone());
    assert!(accounts.iter().any(|a| store.drafts(&a.id).unwrap().iter().any(|d| d.id == id)), "the draft is kept");
}

/// A mail merge left unfinished (Picacho quit) goes on at the next check
/// after it starts, from where it stopped; once done, nothing is kept.
#[gpui::test]
fn an_unfinished_mail_merge_goes_on_after_a_restart(cx: &mut TestAppContext) {
    use crate::manage::authoring::merge::Queue;
    let (view, cx, store) = two_accounts(cx);
    let account = view.read_with(cx, |this, _| this.accounts[0].id.clone());
    let left = |to: &str| crate::manage::model::Draft {
        id: uuid::Uuid::new_v4().to_string(),
        account: account.clone(),
        to: to.into(),
        subject: "Hi".into(),
        body: "Hello".into(),
        ..Default::default()
    };
    Queue {
        draft: "original".into(),
        account: account.clone(),
        total: 3,
        remaining: vec![left("cy@example.org"), left("di@example.org")],
        failed: vec![],
    }
    .save(&store)
    .unwrap();
    view.update(cx, |this, cx| this.send_due(cx));
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    // The test servers take no mail: both are said to have failed, of 3.
    let (status, error) = view.read_with(cx, |this, _| (this.status.clone(), this.error));
    assert!(error && status.contains("Sent 1 of 3") && status.contains("cy@example.org") && status.contains("di@example.org"), "{status}");
    assert!(!status.contains("stopped while"), "{status}");
    assert!(Queue::load(&store).is_none());
    // Taken before sending, a message isn't sent twice after a stop.
    let q = Queue { draft: "d".into(), account: account.clone(), total: 1, remaining: vec![left("ed@example.org")], failed: vec![] };
    q.save(&store).unwrap();
    Queue::take(&store, &q.remaining[0].id).unwrap();
    let kept = Queue::load(&store).unwrap();
    assert!(kept.remaining.is_empty());
    assert!(kept.failed[0].starts_with("ed@example.org (Picacho stopped"), "{:?}", kept.failed);
    Queue::done(&store, "ed@example.org", None).unwrap();
    assert!(Queue::load(&store).unwrap().failed.is_empty());
}
