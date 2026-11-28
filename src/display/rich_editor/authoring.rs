//! The formatted editor's writing aids: Markdown typed as shortcuts, the
//! slash menu, find and replace, text colors and fonts, and tables.
use super::*;
use crate::manage::authoring::{self, BlockRule, InlineMark};

/// What the slash menu runs.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::display::ui) enum SlashCommand {
    Heading(u8),
    Bullets,
    Numbers,
    Quote,
    Code,
    Divider,
    Table,
    /// The composer's: the emoji picker.
    Emoji,
    /// The composer's: today's date, the time now.
    Date,
    Time,
    /// The composer's: template `n`, at the caret.
    Template(usize),
    /// The composer's: snippet `n`, at the caret.
    Snippet(usize),
}

/// An entry of the slash menu.
#[derive(Clone, Debug)]
pub(in crate::display::ui) struct SlashItem {
    pub(in crate::display::ui) command: SlashCommand,
    pub(in crate::display::ui) label: String,
    pub(in crate::display::ui) detail: String,
    pub(in crate::display::ui) icon: &'static str,
    /// More words it is found by.
    pub(in crate::display::ui) words: String,
}

/// The slash menu while it is open: where its "/" is, and the entry chosen.
#[derive(Clone, Copy, Debug)]
pub(super) struct Slash {
    pub(super) start: Pos,
    pub(super) selected: usize,
}

/// What a table's menu does to the table the caret is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::display::ui) enum TableOp {
    RowAbove,
    RowBelow,
    ColumnLeft,
    ColumnRight,
    DeleteRow,
    DeleteColumn,
    DeleteTable,
    Widen,
    Narrow,
    EqualWidths,
}

/// A table column's edge being dragged.
#[derive(Clone, Debug)]
pub(super) struct ColumnDrag {
    /// A block of the table.
    pub(super) block: usize,
    /// The edge right of this column.
    pub(super) column: usize,
    pub(super) from: Pixels,
    pub(super) width: f32,
    pub(super) start: Vec<f32>,
    /// The columns' percents as dragged so far.
    pub(super) percents: Vec<f32>,
}

/// The narrowest a column is made, in percent of its table.
const MIN_COLUMN: f32 = 5.;

fn builtin() -> Vec<SlashItem> {
    let item = |command, label: &str, detail: &str, icon, words: &str| SlashItem {
        command,
        label: label.to_owned(),
        detail: detail.to_owned(),
        icon,
        words: words.to_owned(),
    };
    vec![
        item(SlashCommand::Heading(1), t!("composer-slash-heading-1"), t!("composer-slash-heading-1-detail"), "heading", "h1 title"),
        item(SlashCommand::Heading(2), t!("composer-slash-heading-2"), t!("composer-slash-heading-2-detail"), "heading", "h2 subtitle"),
        item(SlashCommand::Heading(3), t!("composer-slash-heading-3"), t!("composer-slash-heading-3-detail"), "heading", "h3"),
        item(SlashCommand::Bullets, t!("composer-slash-bulleted-list"), t!("composer-slash-bulleted-list-detail"), "list", "ul bullets unordered"),
        item(SlashCommand::Numbers, t!("composer-slash-numbered-list"), t!("composer-slash-numbered-list-detail"), "list-ordered", "ol numbers ordered"),
        item(SlashCommand::Quote, t!("composer-slash-quote"), t!("composer-slash-quote-detail"), "quote", "blockquote citation"),
        item(SlashCommand::Code, t!("composer-slash-code-block"), t!("composer-slash-code-block-detail"), "code", "pre preformatted monospace"),
        item(SlashCommand::Table, t!("composer-slash-table"), t!("composer-slash-table-detail"), "table", "grid columns rows"),
        item(SlashCommand::Divider, t!("composer-slash-divider"), t!("composer-slash-divider-detail"), "minus", "hr rule line separator"),
        item(SlashCommand::Emoji, t!("composer-slash-emoji"), t!("composer-slash-emoji-detail"), "smile", "emoticon smiley"),
        item(SlashCommand::Date, t!("composer-slash-date"), t!("composer-slash-date-detail"), "calendar", "today"),
        item(SlashCommand::Time, t!("composer-slash-time"), t!("composer-slash-time-detail"), "clock", "now"),
    ]
}

fn mark_of(m: InlineMark) -> Mark {
    match m {
        InlineMark::Bold => Mark::Bold,
        InlineMark::Italic => Mark::Italic,
        InlineMark::Strike => Mark::Strike,
        InlineMark::Code => Mark::Code,
    }
}

impl RichEditor {
    // Markdown typed as shortcuts.

    /// After `typed` was typed: a line start that makes a list, quote or
    /// heading ("- ", "1. ", "> ", "## "), or markers that close formatted
    /// text (`**bold**`), turn into what they say. A step of their own, so
    /// undoing puts the Markdown back as typed.
    pub(super) fn apply_input_rules(&mut self, typed: &str, window: Option<&Window>, cx: &mut Context<Self>) {
        if !self.markdown || self.disabled || self.marked.is_some() || !self.sel.is_empty() {
            return;
        }
        let head = self.sel.head;
        let b = self.doc.block(head.block);
        if b.is_pre() || b.is_rule() {
            return;
        }
        let text = b.text();
        if typed == " " {
            let Some(before) = text.get(..head.offset.saturating_sub(1)) else { return };
            let Some(rule) = authoring::block_rule(before) else { return };
            let info = &self.doc.infos(16.)[head.block];
            let in_list = rt::innermost_list(&b.path).is_some();
            let blocked = match rule {
                BlockRule::Bullets | BlockRule::Numbers(_) => in_list,
                BlockRule::Quote => false,
                BlockRule::Heading(_) => info.heading.is_some() || in_list,
            };
            if blocked {
                return;
            }
            self.begin(EditKind::Other);
            let bi = head.block;
            self.doc.delete(Pos::new(bi, 0), head);
            match rule {
                BlockRule::Bullets => self.doc.toggle_list(bi..=bi, ListKind::Bullets),
                BlockRule::Numbers(n) => {
                    self.doc.toggle_list(bi..=bi, ListKind::Numbers);
                    // A new list starting elsewhere than 1 says so.
                    let shared = bi > 0
                        && rt::innermost_list(&self.doc.block(bi).path).is_some_and(|k| {
                            let list = &self.doc.block(bi).path[k];
                            self.doc.block(bi - 1).path.iter().any(|e| Rc::ptr_eq(e, list))
                        });
                    if n != 1 && !shared && let Some(k) = rt::innermost_list(&self.doc.block(bi).path) {
                        let b = self.doc.block_mut(bi);
                        let mut list = (*b.path[k]).clone();
                        list.attrs.retain(|(n, _)| n != "start");
                        list.attrs.push(("start".into(), n.to_string()));
                        b.path[k] = Rc::new(list);
                    }
                }
                BlockRule::Quote => self.doc.toggle_quote(bi..=bi),
                BlockRule::Heading(level) => self.doc.toggle_heading(bi..=bi, level),
            }
            self.sel = Selection::caret(Pos::new(bi, 0));
            self.pending = None;
            return self.finish(EditKind::Other, window, cx);
        }
        if !matches!(typed, "*" | "_" | "`" | "~") {
            return;
        }
        let Some(before) = text.get(..head.offset) else { return };
        let Some(rule) = authoring::inline_rule(before) else { return };
        // Not around pictures, nor inside code already.
        if before[rule.open.start..].contains(rt::OBJECT) || b.style_at(rule.open.end + 1).code {
            return;
        }
        let bi = head.block;
        let info = self.doc.infos(16.)[bi].clone();
        let mark = mark_of(rule.mark);
        self.begin(EditKind::Other);
        self.doc.delete(Pos::new(bi, rule.close.start), Pos::new(bi, rule.close.end));
        self.doc.delete(Pos::new(bi, rule.open.start), Pos::new(bi, rule.open.end));
        let start = rule.open.start;
        let end = rule.close.start - rule.open.len();
        self.doc.restyle(Pos::new(bi, start), Pos::new(bi, end), &|s| set_mark(s, mark, true, &info));
        self.sel = Selection::caret(Pos::new(bi, end));
        // Typing on is as before the markers.
        let mut after = (*self.doc.block(bi).style_at(end)).continued();
        set_mark(&mut after, mark, false, &info);
        self.pending = Some(Rc::new(after));
        self.finish(EditKind::Other, window, cx);
    }

    /// Enter at the end of "---" makes a rule, at the end of "```" a code
    /// block; whether it did.
    pub(super) fn enter_rules(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.markdown || !self.sel.is_empty() {
            return false;
        }
        let head = self.sel.head;
        let b = self.doc.block(head.block);
        if b.is_pre() || head.offset != b.len() || b.runs.iter().any(|r| r.image.is_some()) {
            return false;
        }
        let text = b.text();
        if authoring::is_rule_line(&text) {
            self.begin(EditKind::Other);
            self.doc.block_mut(head.block).runs.clear();
            let at = self.doc.insert_rule(head.block);
            self.sel = Selection::caret(at);
            self.pending = None;
            self.finish(EditKind::Other, Some(window), cx);
            return true;
        }
        if authoring::is_fence_line(&text) {
            self.begin(EditKind::Other);
            self.doc.block_mut(head.block).runs.clear();
            self.doc.toggle_pre(head.block..=head.block);
            self.sel = Selection::caret(Pos::new(head.block, 0));
            self.pending = None;
            self.finish(EditKind::Other, Some(window), cx);
            return true;
        }
        false
    }

    // The slash menu.

    /// After `typed` was typed: a "/" at a line's start or after a space
    /// opens the slash menu.
    pub(super) fn maybe_open_slash(&mut self, typed: &str, cx: &mut Context<Self>) {
        if typed != "/" || self.disabled || !self.sel.is_empty() {
            return;
        }
        let head = self.sel.head;
        let text = self.block_text(head.block);
        let start = head.offset - 1;
        let after_space = text[..start].chars().next_back().is_none_or(char::is_whitespace);
        if after_space && !self.doc.block(head.block).is_pre() {
            self.slash = Some(Slash { start: Pos::new(head.block, start), selected: 0 });
            cx.notify();
        }
    }
    /// What was typed after the "/", while the menu stands.
    pub(super) fn slash_query(&self) -> Option<String> {
        let slash = self.slash?;
        let head = self.sel.head;
        if !self.sel.is_empty() || head.block != slash.start.block || head.offset <= slash.start.offset {
            return None;
        }
        let text = self.block_text(head.block);
        if text.get(slash.start.offset..slash.start.offset + 1) != Some("/") {
            return None;
        }
        let query = text.get(slash.start.offset + 1..head.offset)?;
        (query.len() <= 32 && !query.contains('\n') && !query.starts_with(char::is_whitespace) && !query.contains("  "))
            .then(|| query.to_owned())
    }
    /// The entries for what was typed, best first.
    pub(super) fn slash_entries(&self) -> Vec<SlashItem> {
        let Some(query) = self.slash_query() else { return vec![] };
        let mut all: Vec<(u8, usize, SlashItem)> = builtin()
            .into_iter()
            .chain(self.slash_extra.iter().cloned())
            .enumerate()
            .filter_map(|(i, item)| authoring::rank(&query, &item.label, &item.words).map(|r| (r, i, item)))
            .collect();
        all.sort_by_key(|(r, i, _)| (*r, *i));
        all.into_iter().map(|(_, _, item)| item).take(12).collect()
    }
    /// Closes the menu once the caret leaves what it reads, or nothing
    /// matches what's typed (a path, "/usr", is just text).
    pub(super) fn check_slash(&mut self) {
        if self.slash.is_none() {
            return;
        }
        let close = match self.slash_query() {
            None => true,
            Some(_) => self.slash_entries().is_empty(),
        };
        if close {
            self.slash = None;
        } else {
            let last = self.slash_entries().len().saturating_sub(1);
            if let Some(s) = &mut self.slash {
                s.selected = s.selected.min(last);
            }
        }
    }
    pub(super) fn slash_open(&self) -> bool {
        self.slash.is_some() && self.slash_query().is_some()
    }
    /// Moves the menu's choice; whether the menu took the key.
    pub(super) fn slash_move(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        if !self.slash_open() {
            return false;
        }
        let n = self.slash_entries().len();
        if let Some(s) = &mut self.slash
            && n > 0
        {
            s.selected = (s.selected as isize + by).rem_euclid(n as isize) as usize;
        }
        cx.notify();
        true
    }
    pub(super) fn close_slash(&mut self, cx: &mut Context<Self>) -> bool {
        if self.slash.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }
    /// Runs entry `i` (the chosen one with `None`): the "/" and what was
    /// typed after it go, and the command does what it says, here or (the
    /// composer's) through `RichEvent::Slash`. Whether there was one.
    pub(super) fn choose_slash(&mut self, i: Option<usize>, window: Option<&Window>, cx: &mut Context<Self>) -> bool {
        let Some(slash) = self.slash else { return false };
        let entries = self.slash_entries();
        let Some(item) = entries.get(i.unwrap_or(slash.selected)).cloned() else {
            return self.close_slash(cx);
        };
        self.slash = None;
        self.begin(EditKind::Other);
        let at = self.doc.delete(slash.start, self.sel.head);
        self.sel = Selection::caret(at);
        let bi = at.block;
        match item.command {
            SlashCommand::Heading(level) => {
                let already = self.doc.block(bi).element().heading_level() == Some(level);
                if !already {
                    if self.doc.block(bi).element().heading_level().is_some() {
                        self.doc.toggle_heading(bi..=bi, level);
                    }
                    self.doc.toggle_heading(bi..=bi, level);
                }
            }
            SlashCommand::Bullets => self.doc.toggle_list(bi..=bi, ListKind::Bullets),
            SlashCommand::Numbers => self.doc.toggle_list(bi..=bi, ListKind::Numbers),
            SlashCommand::Quote => self.doc.toggle_quote(bi..=bi),
            SlashCommand::Code => self.doc.toggle_pre(bi..=bi),
            SlashCommand::Divider => {
                let at = if self.doc.block(bi).is_empty() {
                    self.doc.insert_rule(bi)
                } else {
                    let after = self.doc.split(at);
                    self.doc.insert_rule(after.block - 1)
                };
                self.sel = Selection::caret(at);
            }
            SlashCommand::Table => {
                let at = self.doc.insert_table(at, 2, 3);
                self.sel = Selection::caret(at);
            }
            _ => {}
        }
        self.pending = None;
        self.finish(EditKind::Other, window, cx);
        if matches!(
            item.command,
            SlashCommand::Emoji | SlashCommand::Date | SlashCommand::Time | SlashCommand::Template(_) | SlashCommand::Snippet(_)
        ) {
            cx.emit(RichEvent::Slash(item.command));
        }
        true
    }
    /// The slash menu, below the caret.
    pub(super) fn slash_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.slash_open() {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let caret = layout.caret(&self.doc, self.sel.head)?;
        let at = layout.origin.get() + caret.origin + point(px(0.), caret.size.height + px(4.));
        let look = self.look;
        let entries = self.slash_entries();
        let selected = self.slash.map_or(0, |s| s.selected);
        let mut list = div()
            .id("rich-slash-menu")
            .debug_selector(|| "rich-slash-menu".into())
            .w(px(280.))
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(look.border))
            .bg(rgb(look.surface))
            .text_color(rgb(look.text))
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_slash(cx);
            }));
        if entries.is_empty() {
            list = list.child(div().px_3().py_2().text_size(px(12.)).text_color(rgb(look.muted)).child(t!("composer-slash-no-match")));
        }
        for (i, item) in entries.into_iter().enumerate() {
            let on = i == selected;
            let selector = format!("rich-slash-{i}");
            list = list.child(
                div()
                    .id(("rich-slash", i))
                    .debug_selector(move || selector.clone())
                    .px_2()
                    .py(px(5.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(rgb(look.soft)))
                    .hover(move |s| s.bg(rgb(look.soft)))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(look.border))
                            .child(icon_el(item.icon, 15., look.muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).truncate().child(item.label.clone()))
                            .child(div().text_size(px(11.)).text_color(rgb(look.muted)).truncate().child(item.detail.clone())),
                    )
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.choose_slash(Some(i), Some(w), cx);
                    })),
            );
        }
        Some(
            deferred(anchored().position(at).anchor(Anchor::TopLeft).snap_to_window_with_margin(px(8.)).child(list))
                .with_priority(2)
                .into_any_element(),
        )
    }

    // Find and replace.

    /// Where `query` is in the text, block by block.
    pub(in crate::display::ui) fn find(&self, query: &str, match_case: bool) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        for (bi, b) in self.doc.blocks.iter().enumerate() {
            let text = b.text();
            for r in authoring::find_matches(&text, query, match_case) {
                out.push((Pos::new(bi, r.start), Pos::new(bi, r.end)));
            }
        }
        out
    }
    /// Shows `matches` highlighted (none to stop).
    pub(in crate::display::ui) fn set_highlights(&mut self, matches: Vec<(Pos, Pos)>, cx: &mut Context<Self>) {
        if self.highlights != matches {
            self.highlights = matches;
            cx.notify();
        }
    }
    /// Selects `start..end` and brings it into view.
    pub(in crate::display::ui) fn select_range(&mut self, start: Pos, end: Pos, cx: &mut Context<Self>) {
        self.sel = Selection { anchor: self.doc.clamp(start), head: self.doc.clamp(end) };
        self.pending = None;
        self.reveal = true;
        self.goal_x = None;
        cx.notify();
    }
    /// Puts `with` in place of `start..end`, in the style the text there
    /// had; where it ends.
    pub(in crate::display::ui) fn replace_range(&mut self, start: Pos, end: Pos, with: &str, window: Option<&Window>, cx: &mut Context<Self>) -> Pos {
        if self.disabled {
            return end;
        }
        self.begin(EditKind::Other);
        let at = self.replace_one(start, end, with);
        self.sel = Selection::caret(at);
        self.finish(EditKind::Other, window, cx);
        at
    }
    fn replace_one(&mut self, start: Pos, end: Pos, with: &str) -> Pos {
        let style = Rc::new(self.doc.block(start.block).style_at(start.offset + 1).continued());
        let at = self.doc.delete(start, end);
        self.doc.insert_text(at, with, style)
    }
    /// Replaces every one of `matches` with `with`, as one step; how many.
    pub(in crate::display::ui) fn replace_all(&mut self, matches: &[(Pos, Pos)], with: &str, window: Option<&Window>, cx: &mut Context<Self>) -> usize {
        if self.disabled || matches.is_empty() {
            return 0;
        }
        self.begin(EditKind::Other);
        // From the last, so earlier positions stay where they are.
        for (s, e) in matches.iter().rev() {
            self.replace_one(*s, *e, with);
        }
        self.sel = Selection::caret(self.doc.clamp(self.sel.head));
        self.finish(EditKind::Other, window, cx);
        matches.len()
    }

    // Colors, fonts and sizes.

    /// Changes the style of the selection, or of what's typed next.
    pub(in crate::display::ui) fn restyle_with(&mut self, change: &dyn Fn(&mut rt::Style), window: Option<&Window>, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let mut style = (*self.typing_style()).clone();
            change(&mut style);
            self.pending = Some(Rc::new(style));
            cx.notify();
            return;
        }
        self.begin(EditKind::Other);
        self.doc.restyle(self.sel.start(), self.sel.end(), change);
        self.finish(EditKind::Other, window, cx);
    }
    pub(in crate::display::ui) fn set_color(&mut self, color: Option<rt::Color>, window: Option<&Window>, cx: &mut Context<Self>) {
        self.restyle_with(&|s| s.color = color, window, cx);
    }
    pub(in crate::display::ui) fn set_highlight(&mut self, color: Option<rt::Color>, window: Option<&Window>, cx: &mut Context<Self>) {
        self.restyle_with(&|s| s.background = color, window, cx);
    }
    pub(in crate::display::ui) fn set_family(&mut self, family: Option<String>, window: Option<&Window>, cx: &mut Context<Self>) {
        self.restyle_with(&|s| s.family = family.clone(), window, cx);
    }
    pub(in crate::display::ui) fn set_size(&mut self, size: Option<f32>, window: Option<&Window>, cx: &mut Context<Self>) {
        self.restyle_with(&|s| s.size = size, window, cx);
    }

    // Tables.

    /// Whether the caret is in a table.
    pub(in crate::display::ui) fn in_table(&self) -> bool {
        self.doc.table_at(self.sel.head.block).is_some()
    }
    /// Puts a table of `rows` by `cols` at the caret.
    pub(in crate::display::ui) fn insert_table(&mut self, rows: usize, cols: usize, window: Option<&Window>, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.begin(EditKind::Other);
        let at = self.delete_selection();
        let at = self.doc.insert_table(at, rows, cols);
        self.sel = Selection::caret(at);
        self.pending = None;
        self.finish(EditKind::Other, window, cx);
    }
    /// Does `op` to the table the caret is in.
    pub(in crate::display::ui) fn table_op(&mut self, op: TableOp, window: Option<&Window>, cx: &mut Context<Self>) {
        let bi = self.sel.head.block;
        let Some(t) = self.doc.table_at(bi) else { return };
        if self.disabled {
            return;
        }
        self.begin(EditKind::Other);
        let caret = match op {
            TableOp::RowAbove => self.doc.table_insert_row(bi, false),
            TableOp::RowBelow => self.doc.table_insert_row(bi, true),
            TableOp::ColumnLeft => self.doc.table_insert_column(bi, false),
            TableOp::ColumnRight => self.doc.table_insert_column(bi, true),
            TableOp::DeleteRow => self.doc.table_delete_row(bi),
            TableOp::DeleteColumn => self.doc.table_delete_column(bi),
            TableOp::DeleteTable => self.doc.table_delete(bi),
            TableOp::EqualWidths => {
                self.doc.set_table_widths(bi, None);
                None
            }
            TableOp::Widen | TableOp::Narrow => {
                let (ri, ci) = t.locate(bi).unwrap_or((0, 0));
                let column = t.rows[ri].starts()[ci];
                if let Some(mut p) = self.doc.table_percents(bi) {
                    resize_column(&mut p, column, if op == TableOp::Widen { 10. } else { -10. });
                    self.doc.set_table_widths(bi, Some(&p));
                }
                None
            }
        };
        if let Some(at) = caret {
            self.sel = Selection::caret(at);
        }
        self.finish(EditKind::Other, window, cx);
    }
    /// Tab in a table: the next cell (a new row after the last); Shift-Tab
    /// the cell before. Whether the caret was in a table.
    pub(super) fn table_tab(&mut self, forward: bool, window: &Window, cx: &mut Context<Self>) -> bool {
        let bi = self.sel.head.block;
        if self.doc.table_at(bi).is_none() {
            return false;
        }
        match self.doc.table_step(bi, forward) {
            Some(at) => {
                let end = Pos::new(at.block, self.doc.block(at.block).len());
                // The cell's text selected, as other editors do.
                self.sel = if forward { Selection { anchor: at, head: end } } else { Selection { anchor: Pos::new(at.block, 0), head: at } };
                self.pending = None;
                self.reveal = true;
                cx.notify();
            }
            None if forward && !self.disabled => {
                // From the last cell: a new row.
                let last = self.doc.table_at(bi).and_then(|t| t.rows.last().map(|r| r.cells[0].blocks.start)).unwrap_or(bi);
                self.begin(EditKind::Other);
                if let Some(at) = self.doc.table_insert_row(last, true) {
                    self.sel = Selection::caret(at);
                }
                self.finish(EditKind::Other, Some(window), cx);
            }
            None => {}
        }
        true
    }
    /// Starts dragging a table column's edge at `local` (in the layout);
    /// whether there was one.
    pub(super) fn start_column_drag(&mut self, local: Point<Pixels>, position: Point<Pixels>) -> bool {
        let Some(layout) = self.layout.clone() else { return false };
        let Some((block, column, width)) = layout.column_edge_at(local) else { return false };
        let Some(percents) = self.doc.table_percents(block) else { return false };
        if column + 1 >= percents.len() {
            return false;
        }
        self.column_drag = Some(ColumnDrag { block, column, from: position.x, width, start: percents.clone(), percents });
        true
    }
    pub(super) fn drag_column(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(d) = &mut self.column_drag else { return false };
        let delta = f32::from(position.x - d.from) / d.width.max(1.) * 100.;
        let (a, b) = (d.start[d.column], d.start[d.column + 1]);
        let moved = delta.clamp(MIN_COLUMN - a, b - MIN_COLUMN);
        d.percents = d.start.clone();
        d.percents[d.column] = a + moved;
        d.percents[d.column + 1] = b - moved;
        self.transient += 1;
        cx.notify();
        true
    }
    pub(super) fn end_column_drag(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.column_drag.take() else { return false };
        if d.percents != d.start {
            self.begin(EditKind::Other);
            self.doc.set_table_widths(d.block, Some(&d.percents));
            self.finish(EditKind::Other, Some(window), cx);
        }
        self.transient += 1;
        cx.notify();
        true
    }
    /// The percents a table is shown with while its edge is dragged.
    pub(super) fn dragged_percents(&self, first_block: usize) -> Option<&[f32]> {
        let d = self.column_drag.as_ref()?;
        (d.block == first_block).then_some(d.percents.as_slice())
    }
}

/// `percents` with column `column` `by` points wider (narrower), the
/// others giving (taking) it in their proportions, none narrower than
/// `MIN_COLUMN`.
pub(super) fn resize_column(percents: &mut [f32], column: usize, by: f32) {
    let n = percents.len();
    if n < 2 || column >= n {
        return;
    }
    let others: f32 = percents.iter().enumerate().filter(|(i, _)| *i != column).map(|(_, p)| p).sum();
    let room = others - MIN_COLUMN * (n - 1) as f32;
    let by = by.clamp(MIN_COLUMN - percents[column], room.max(0.));
    if by == 0. || others <= 0. {
        return;
    }
    for (i, p) in percents.iter_mut().enumerate() {
        if i == column {
            *p += by;
        } else {
            *p -= by * *p / others;
        }
    }
    // Rounding: the columns add up to the table.
    let sum: f32 = percents.iter().sum();
    if let Some(last) = percents.iter_mut().filter(|p| **p > MIN_COLUMN).last() {
        *last += 100. - sum;
    }
}

#[cfg(test)]
#[std::prelude::v1::test]
fn columns_widen_from_the_others() {
    let mut p = vec![25., 25., 50.];
    resize_column(&mut p, 0, 10.);
    assert!((p[0] - 35.).abs() < 0.01, "{p:?}");
    assert!((p.iter().sum::<f32>() - 100.).abs() < 0.01);
    assert!(p[2] > p[1]);
    let mut narrow = vec![50., 50.];
    resize_column(&mut narrow, 0, -100.);
    assert!((narrow[0] - MIN_COLUMN).abs() < 0.01, "{narrow:?}");
}
