//! The composer's writing aids: find and replace, the emoji picker, text
//! colors, fonts and sizes, tables, the slash menu's commands, how large
//! pictures are sent, files too large to send, and mail merge.
use super::*;
use crate::display::ui::rich_editor::{SlashCommand, SlashItem, TableOp};
use crate::manage::authoring::{emoji, merge};
use crate::manage::composing::ImageSize;
use std::ops::Range;

/// Find and replace, while its bar is open.
pub(super) struct FindBar {
    pub(super) query: Entity<InputState>,
    replace: Entity<InputState>,
    match_case: bool,
    pub(super) replacing: bool,
    /// The match the caret is on.
    current: usize,
    _subscriptions: Vec<Subscription>,
}

/// The emoji picker, while open.
pub(super) struct EmojiPicker {
    search: Entity<InputState>,
    category: emoji::Category,
    _subscription: Subscription,
}

/// Mail merge's panel, while open.
#[derive(Default)]
pub(super) struct MergePanel {
    sheet: Option<merge::Sheet>,
    file: String,
    /// The column with the addresses.
    column: usize,
    /// The row the preview shows.
    preview: usize,
    error: Option<String>,
}

/// Find's matches in the text being written.
enum Found {
    Rich(Vec<(rt::Pos, rt::Pos)>),
    Plain(Vec<Range<usize>>),
}
impl Found {
    fn len(&self) -> usize {
        match self {
            Found::Rich(m) => m.len(),
            Found::Plain(m) => m.len(),
        }
    }
}

thread_local! {
    /// Emoji picked lately, latest first (while Picacho runs).
    static RECENT_EMOJI: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Text colors the style menu offers.
const TEXT_COLORS: [u32; 10] = [0x000000, 0x5f6368, 0xd93025, 0xe8710a, 0xf9ab00, 0x188038, 0x1a73e8, 0x2b3bd1, 0x9334e6, 0xc5221f];
/// Highlight colors.
const HIGHLIGHTS: [u32; 6] = [0xfff475, 0xccff90, 0xa7ffeb, 0xcbf0f8, 0xfdcfe8, 0xffd8a8];
/// Font families: the menu's name, and the family written (`None` for the
/// message's own).
const FAMILIES: [(&str, Option<&str>); 7] = [
    ("", None),
    ("Arial", Some("Arial")),
    ("Georgia", Some("Georgia")),
    ("Times New Roman", Some("Times New Roman")),
    ("Verdana", Some("Verdana")),
    ("Trebuchet MS", Some("Trebuchet MS")),
    ("Courier New", Some("Courier New")),
];
/// Text sizes, in CSS pixels (`None` for the message's own).
const SIZES: [Option<f32>; 6] = [Some(11.), Some(13.), None, Some(18.), Some(24.), Some(32.)];
/// The largest table the menu's grid makes.
const GRID: (usize, usize) = (8, 10);

fn color_of(c: u32) -> rt::Color {
    rt::Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl Composer {
    // Text at the caret, in either editor.

    /// Types `text` at the caret (over the selection), formatted or plain.
    pub(super) fn type_at_caret(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = false;
        if self.draft.formatted {
            self.rich.update(cx, |r, cx| {
                r.insert_text(text, Some(window), cx);
                r.focus(window, cx);
            });
            return;
        }
        let (_, range) = self.selection(window, cx);
        let start = range.start;
        self.edit(range, text, start + text.len(), window, cx);
        self.body.update(cx, |s, cx| s.focus(window, cx));
    }

    // Find and replace.

    /// Opens find (and replace), its query the selected words if any.
    pub(super) fn open_find(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = false;
        let selected = if self.draft.formatted {
            self.rich.read(cx).selected_text()
        } else {
            let (text, range) = self.selection(window, cx);
            text.get(range).unwrap_or("").to_owned()
        };
        if let Some(find) = &mut self.find {
            find.replacing = find.replacing || replacing;
            let focus = if replacing { find.replace.clone() } else { find.query.clone() };
            focus.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t!("composer-find")).input_action(TextInputAction::Search));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder(t!("composer-replace-with")).input_action(TextInputAction::Done));
        if !selected.contains('\n') && !selected.trim().is_empty() && selected.len() < 200 {
            query.update(cx, |s, cx| s.set_text(selected.clone(), window, cx));
        }
        query.update(cx, |s, cx| s.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |this, _, e, w, cx| match e {
                InputEvent::Change => this.find_from_caret(w, cx),
                InputEvent::PressEnter { shift, .. } => this.find_step(!*shift, w, cx),
                _ => {}
            }),
            cx.subscribe_in(&replace, window, |this, _, e, w, cx| {
                if let InputEvent::PressEnter { secondary, .. } = e {
                    this.find_replace(*secondary, w, cx);
                }
            }),
        ];
        self.find = Some(FindBar { query, replace, match_case: false, replacing, current: 0, _subscriptions: subscriptions });
        self.find_from_caret(window, cx);
    }

    pub(in crate::display::ui) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_none() {
            return;
        }
        self.rich.update(cx, |r, cx| r.set_highlights(vec![], cx));
        self.body.update(cx, |s, cx| s.close_search(cx));
        if self.draft.formatted {
            self.rich.update(cx, |r, cx| r.focus(window, cx));
        } else {
            self.body.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    fn found(&self, cx: &App) -> Found {
        let Some(find) = &self.find else { return Found::Plain(vec![]) };
        let query = find.query.read(cx).value().to_string();
        if self.draft.formatted {
            Found::Rich(self.rich.read(cx).find(&query, find.match_case))
        } else {
            let text = self.body.read(cx).value().to_string();
            Found::Plain(crate::manage::authoring::find_matches(&text, &query, find.match_case))
        }
    }

    /// The query changed: the first match from the caret on.
    fn find_from_caret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.found(cx);
        let current = match &found {
            Found::Rich(m) => {
                let at = self.rich.read(cx).sel.start();
                m.iter().position(|(s, _)| *s >= at).unwrap_or(0)
            }
            Found::Plain(m) => {
                let (_, range) = self.selection(window, cx);
                m.iter().position(|r| r.start >= range.start).unwrap_or(0)
            }
        };
        if let Some(find) = &mut self.find {
            find.current = current;
        }
        self.find_show(found, cx);
    }

    /// Highlights the matches and selects the current one.
    fn find_show(&mut self, found: Found, cx: &mut Context<Self>) {
        let Some(current) = self.find.as_ref().map(|f| f.current) else { return };
        match found {
            Found::Rich(m) => {
                let chosen = m.get(current).copied();
                self.rich.update(cx, |r, cx| {
                    r.set_highlights(m, cx);
                    if let Some((s, e)) = chosen {
                        r.select_range(s, e, cx);
                    }
                });
            }
            Found::Plain(m) => {
                // The text field's own search engine highlights them, and
                // brings the current one into view.
                let (query, case) = self.find.as_ref().map(|f| (f.query.read(cx).value().to_string(), f.match_case)).unwrap_or_default();
                let chosen = m.get(current).cloned();
                self.body.update(cx, |s, cx| {
                    if query.is_empty() {
                        s.close_search(cx);
                        return;
                    }
                    s.set_search_query(query, !case, cx);
                    if let Some(r) = chosen {
                        for _ in 0..=m.len() {
                            match s.next_search_match(cx) {
                                Some(found) if found == r => break,
                                Some(_) => {}
                                None => break,
                            }
                        }
                        s.set_selected_range(r, cx);
                    }
                });
            }
        }
        cx.notify();
    }

    /// The text changed: its matches highlighted again, where they are now.
    pub(super) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return;
        }
        if let Found::Rich(m) = self.found(cx) {
            if let Some(f) = &mut self.find {
                f.current = f.current.min(m.len().saturating_sub(1));
            }
            self.rich.update(cx, |r, cx| r.set_highlights(m, cx));
        }
    }

    fn find_step(&mut self, forward: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let found = self.found(cx);
        let n = found.len();
        let Some(find) = &mut self.find else { return };
        if n == 0 {
            return;
        }
        find.current = if forward { (find.current + 1) % n } else { (find.current + n - 1) % n };
        self.find_show(found, cx);
    }

    /// Replaces the current match (and goes to the next), or every one.
    fn find_replace(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let with = find.replace.read(cx).value().to_string();
        let current = find.current;
        match self.found(cx) {
            Found::Rich(m) if !m.is_empty() => {
                if all {
                    let n = self.rich.update(cx, |r, cx| r.replace_all(&m, &with, Some(window), cx));
                    self.error = None;
                    self.find_notice = Some(t!("composer-replaced", count = n));
                } else if let Some((s, e)) = m.get(current.min(m.len() - 1)).copied() {
                    self.rich.update(cx, |r, cx| r.replace_range(s, e, &with, Some(window), cx));
                }
            }
            Found::Plain(m) if !m.is_empty() => {
                if all {
                    let mut text = self.body.read(cx).value().to_string();
                    for r in m.iter().rev() {
                        text.replace_range(r.clone(), &with);
                    }
                    let len = text.len();
                    self.edit(0..self.body.read(cx).value().len(), &text, len.min(m[0].start + with.len()), window, cx);
                    self.find_notice = Some(t!("composer-replaced", count = m.len()));
                } else if let Some(r) = m.get(current.min(m.len() - 1)).cloned() {
                    let end = r.start + with.len();
                    self.edit(r, &with, end, window, cx);
                }
            }
            _ => return,
        }
        // On to the next one, from where the replacement ended.
        let found = self.found(cx);
        if let Some(find) = &mut self.find {
            find.current = if all { 0 } else { current.min(found.len().saturating_sub(1)) };
        }
        self.find_show(found, cx);
    }

    /// The find (and replace) bar, above the text.
    pub(super) fn find_bar(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let find = self.find.as_ref()?;
        let c = self.colors;
        let found = self.found(cx).len();
        let has_query = !find.query.read(cx).value().is_empty();
        let count = if !has_query {
            String::new()
        } else if found == 0 {
            t!("composer-no-matches").to_owned()
        } else {
            t!("composer-match-of", current = find.current.min(found - 1) + 1, count = found)
        };
        let small = |id: &'static str, label: SharedString, enabled: bool| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .h(px(26.))
                .px_2()
                .flex()
                .items_center()
                .flex_shrink_0()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(c.border))
                .text_size(px(12.))
                .when(!enabled, |d| d.opacity(0.5))
                .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(rgb(c.soft))))
                .child(label)
        };
        let field = |input: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.surface))
                .child(Input::new(input).appearance(false).text_size(px(13.)))
        };
        let mut bar = div()
            .id("compose-find")
            .debug_selector(|| "compose-find".into())
            .flex_shrink_0()
            .px_3()
            .py_1()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .capture_action(cx.listener(|this, _: &gpui_component::input::Escape, w, cx| {
                this.close_find(w, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon_el("search", 14., c.muted))
                    .child(field(&find.query))
                    .child(div().min_w(px(64.)).flex_shrink_0().text_size(px(11.)).text_color(rgb(c.muted)).child(count))
                    .child(self.icon_button("find-previous", "chevron-up", t!("composer-previous-match"), false, cx, |t, w, cx| t.find_step(false, w, cx)))
                    .child(self.icon_button("find-next", "chevron-down", t!("composer-next-match"), false, cx, |t, w, cx| t.find_step(true, w, cx)))
                    .child(self.icon_button("find-case", "case-sensitive", t!("composer-match-case"), find.match_case, cx, |t, w, cx| {
                        if let Some(f) = &mut t.find {
                            f.match_case = !f.match_case;
                        }
                        t.find_from_caret(w, cx);
                    }))
                    .child(self.icon_button("find-replace", "replace", t!("composer-replace"), find.replacing, cx, |t, _, cx| {
                        if let Some(f) = &mut t.find {
                            f.replacing = !f.replacing;
                        }
                        cx.notify();
                    }))
                    .child(self.icon_button("find-close", "close", t!("composer-close-find"), false, cx, |t, w, cx| t.close_find(w, cx))),
            );
        if find.replacing {
            let enabled = found > 0 && !self.sending;
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon_el("replace", 14., c.muted))
                    .child(field(&find.replace))
                    .child(small("replace-one", t!("composer-replace").into(), enabled).on_click(cx.listener(|t, _, w, cx| t.find_replace(false, w, cx))))
                    .child(small("replace-all", t!("composer-replace-all").into(), enabled).on_click(cx.listener(|t, _, w, cx| t.find_replace(true, w, cx)))),
            );
        }
        if let Some(note) = &self.find_notice {
            bar = bar.child(div().text_size(px(11.)).text_color(rgb(c.muted)).child(note.clone()));
        }
        Some(bar)
    }

    // Emoji.

    pub(in crate::display::ui) fn toggle_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji.take().is_some() {
            cx.notify();
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("composer-find-an-emoji")));
        search.update(cx, |s, cx| s.focus(window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, search, e, w, cx| match e {
            InputEvent::Change => cx.notify(),
            // Enter takes the first found.
            InputEvent::PressEnter { .. } => {
                let query = search.read(cx).value().to_string();
                if let Some(first) = emoji::search(&query).first() {
                    this.pick_emoji(first.text, w, cx);
                }
            }
            _ => {}
        });
        self.emoji = Some(EmojiPicker { search, category: emoji::Category::Smileys, _subscription: subscription });
        cx.notify();
    }

    fn pick_emoji(&mut self, text: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        RECENT_EMOJI.with_borrow_mut(|recent| {
            recent.retain(|e| *e != text);
            recent.insert(0, text);
            recent.truncate(16);
        });
        self.emoji = None;
        self.type_at_caret(text, window, cx);
        cx.notify();
    }

    pub(super) fn emoji_menu(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let picker = self.emoji.as_ref()?;
        let c = self.colors;
        let query = picker.search.read(cx).value().to_string();
        let cell = |i: usize, e: &'static str, prefix: &'static str| {
            let selector = format!("{prefix}-{i}");
            div()
                .id(SharedString::from(format!("{prefix}-{i}")))
                .debug_selector(move || selector.clone())
                .size(px(34.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_size(px(20.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(e)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, w, cx| this.pick_emoji(e, w, cx)))
        };
        let shown: Vec<&'static str> = if query.trim().is_empty() {
            emoji::in_category(picker.category).into_iter().map(|e| e.text).collect()
        } else {
            emoji::search(&query).into_iter().map(|e| e.text).collect()
        };
        let mut menu = self
            .popup("emoji-menu", 352., cx, |this, cx| {
                this.emoji = None;
                cx.notify();
            })
            .child(
                div()
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.surface))
                    .child(icon_el("search", 13., c.muted))
                    .child(div().flex_1().child(Input::new(&picker.search).appearance(false).text_size(px(13.)))),
            );
        let recent = RECENT_EMOJI.with_borrow(|r| r.clone());
        if query.trim().is_empty() && !recent.is_empty() {
            menu = menu
                .child(div().px_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.muted)).child(t!("composer-recently-used")))
                .child(div().flex().flex_wrap().children(recent.into_iter().enumerate().map(|(i, e)| cell(i, e, "emoji-recent"))));
        }
        if query.trim().is_empty() {
            let mut tabs = div().flex().gap(px(2.)).border_b_1().border_color(rgb(c.border)).pb_1();
            for (i, category) in emoji::Category::ALL.into_iter().enumerate() {
                let on = category == picker.category;
                tabs = tabs.child(
                    div()
                        .id(("emoji-category", i))
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_size(px(17.))
                        .cursor_pointer()
                        .when(on, |d| d.bg(rgb(c.soft)))
                        .hover(move |s| s.bg(rgb(c.soft)))
                        .child(category.icon())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = &mut this.emoji {
                                p.category = category;
                            }
                            cx.notify();
                        })),
                );
            }
            menu = menu.child(tabs);
        }
        if shown.is_empty() {
            menu = menu.child(div().px_2().py_2().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("composer-no-emoji-found")));
        }
        Some(
            menu.child(
                div()
                    .id("emoji-grid")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .children(shown.into_iter().enumerate().map(|(i, e)| cell(i, e, "emoji"))),
            ),
        )
    }

    /// A menu popping up above the footer (or below the toolbar, `top`),
    /// closed by a press outside it.
    pub(super) fn popup(
        &self,
        id: &'static str,
        width: f32,
        cx: &Context<Self>,
        close: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let c = self.colors;
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .absolute()
            .bottom(px(48.))
            .left(px(12.))
            .w(px(width))
            .max_w(relative(0.95))
            .p_2()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .occlude()
            .flex()
            .flex_col()
            .gap_1()
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)))
    }

    /// A menu below the toolbar.
    fn toolbar_popup(&self, id: &'static str, width: f32, left: f32, cx: &Context<Self>, close: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> Stateful<Div> {
        self.popup(id, width, cx, close).bottom_auto().top(px(38.)).left(px(left))
    }

    // Colors, fonts and sizes.

    pub(super) fn style_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let active = self.rich.read(cx).active();
        let heading = |text: &'static str| div().px_1().pt_1().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.muted)).child(text);
        let swatch = |id: String, fill: Option<u32>, on: bool| {
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector.clone())
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(if on { c.accent } else { c.border }))
                .when(on, |d| d.border_2())
                .when_some(fill, |d, f| d.bg(rgb(f)))
                .when(fill.is_none(), |d| d.flex().items_center().justify_center().child(icon_el("ban", 13., c.muted)))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
        };
        let mut colors = div().flex().flex_wrap().gap_1().child(
            swatch("text-color-none".into(), None, active.color.is_none())
                .tooltip(|w, cx| Tooltip::new(t!("composer-automatic-color")).build(w, cx))
                .on_click(cx.listener(|t, _, w, cx| t.rich.update(cx, |r, cx| r.set_color(None, Some(w), cx)))),
        );
        for (i, value) in TEXT_COLORS.into_iter().enumerate() {
            let on = active.color == Some(color_of(value));
            colors = colors.child(swatch(format!("text-color-{i}"), Some(value), on).on_click(cx.listener(move |t, _, w, cx| {
                t.rich.update(cx, |r, cx| r.set_color(Some(color_of(value)), Some(w), cx))
            })));
        }
        let mut highlights = div().flex().flex_wrap().gap_1().child(
            swatch("highlight-none".into(), None, active.background.is_none())
                .tooltip(|w, cx| Tooltip::new(t!("composer-no-highlight")).build(w, cx))
                .on_click(cx.listener(|t, _, w, cx| t.rich.update(cx, |r, cx| r.set_highlight(None, Some(w), cx)))),
        );
        for (i, value) in HIGHLIGHTS.into_iter().enumerate() {
            let on = active.background == Some(color_of(value));
            highlights = highlights.child(swatch(format!("highlight-{i}"), Some(value), on).on_click(cx.listener(move |t, _, w, cx| {
                t.rich.update(cx, |r, cx| r.set_highlight(Some(color_of(value)), Some(w), cx))
            })));
        }
        let choice = |id: String, label: String, on: bool| {
            let selector = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || selector.clone())
                .h(px(26.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(on, |d| d.bg(rgb(c.soft)).text_color(rgb(c.accent)))
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
        };
        let mut families = div().flex().flex_col();
        for (i, (name, family)) in FAMILIES.into_iter().enumerate() {
            let label = if name.is_empty() { t!("composer-default-font").to_owned() } else { name.to_owned() };
            let on = active.family.as_deref() == family;
            let mut row = choice(format!("font-{i}"), label, on);
            if let Some(f) = family {
                row = row.font_family(SharedString::from(f));
            }
            families = families.child(row.on_click(cx.listener(move |t, _, w, cx| {
                t.rich.update(cx, |r, cx| r.set_family(family.map(str::to_owned), Some(w), cx))
            })));
        }
        let mut sizes = div().flex().flex_wrap().gap_1();
        for (i, size) in SIZES.into_iter().enumerate() {
            let label = match size {
                None => t!("composer-normal-size").to_owned(),
                Some(px) => format!("{px}"),
            };
            let on = active.size == size;
            sizes = sizes.child(
                choice(format!("size-{i}"), label, on)
                    .border_1()
                    .border_color(rgb(c.border))
                    .on_click(cx.listener(move |t, _, w, cx| t.rich.update(cx, |r, cx| r.set_size(size, Some(w), cx)))),
            );
        }
        self.toolbar_popup("style-menu", 300., 150., cx, |t, cx| {
            t.style_open = false;
            cx.notify();
        })
        .child(heading(t!("composer-text-color")))
        .child(colors)
        .child(heading(t!("composer-highlight")))
        .child(highlights)
        .child(heading(t!("composer-size")))
        .child(sizes)
        .child(heading(t!("composer-font")))
        .child(families)
    }

    // Tables.

    pub(super) fn table_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let in_table = self.rich.read(cx).in_table();
        let menu = self.toolbar_popup("table-menu", if in_table { 240. } else { 236. }, 190., cx, |t, cx| {
            t.table_open = None;
            cx.notify();
        });
        if !in_table {
            // A grid: the size pointed at is the table made.
            let (rows, cols) = self.table_open.flatten().unwrap_or((0, 0));
            let mut grid = div().flex().flex_col().gap(px(2.));
            for r in 0..GRID.0 {
                let mut line = div().flex().gap(px(2.));
                for col in 0..GRID.1 {
                    let on = r < rows && col < cols;
                    let selector = format!("table-size-{}x{}", r + 1, col + 1);
                    line = line.child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .size(px(18.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(if on { c.accent } else { c.border }))
                            .when(on, |d| d.bg(rgb(c.soft)))
                            .cursor_pointer()
                            .on_hover(cx.listener(move |t, hovered: &bool, _, cx| {
                                if *hovered {
                                    t.table_open = Some(Some((r + 1, col + 1)));
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |t, _, w, cx| {
                                t.table_open = None;
                                t.rich.update(cx, |ed, cx| {
                                    ed.insert_table(r + 1, col + 1, Some(w), cx);
                                    ed.focus(w, cx);
                                });
                            })),
                    );
                }
                grid = grid.child(line);
            }
            let label = if rows > 0 { t!("composer-table-size", rows = rows, columns = cols) } else { t!("composer-insert-table").to_owned() };
            return menu.child(div().px_1().text_size(px(12.)).text_color(rgb(c.muted)).child(label)).child(grid);
        }
        let row = |id: &'static str, label: &'static str, icon: &'static str, op: TableOp, danger: bool| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(danger, |d| d.text_color(rgb(c.danger)))
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(icon_el(icon, 14., if danger { c.danger } else { c.muted }))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |t, _, w, cx| {
                    // Width changes are tried again and again: the menu stays.
                    if !matches!(op, TableOp::Widen | TableOp::Narrow) {
                        t.table_open = None;
                    }
                    t.rich.update(cx, |r, cx| {
                        r.table_op(op, Some(w), cx);
                        r.focus(w, cx);
                    });
                }))
        };
        let sep = || div().h(px(1.)).my_1().bg(rgb(c.border));
        menu.child(row("table-row-above", t!("composer-insert-row-above"), "arrow-up", TableOp::RowAbove, false))
            .child(row("table-row-below", t!("composer-insert-row-below"), "arrow-down", TableOp::RowBelow, false))
            .child(row("table-column-left", t!("composer-insert-column-left"), "arrow-left", TableOp::ColumnLeft, false))
            .child(row("table-column-right", t!("composer-insert-column-right"), "arrow-right", TableOp::ColumnRight, false))
            .child(sep())
            .child(row("table-widen", t!("composer-widen-column"), "plus", TableOp::Widen, false))
            .child(row("table-narrow", t!("composer-narrow-column"), "minus", TableOp::Narrow, false))
            .child(row("table-equal", t!("composer-equal-columns"), "table", TableOp::EqualWidths, false))
            .child(sep())
            .child(row("table-delete-row", t!("composer-delete-row"), "close", TableOp::DeleteRow, true))
            .child(row("table-delete-column", t!("composer-delete-column"), "close", TableOp::DeleteColumn, true))
            .child(row("table-delete", t!("composer-delete-table"), "trash", TableOp::DeleteTable, true))
            .child(div().px_2().pt_1().text_size(px(11.)).text_color(rgb(c.muted)).child(t!("composer-drag-column-edges")))
    }

    // The slash menu.

    /// What the slash menu offers besides its blocks: the templates.
    pub(super) fn slash_items(&self) -> Vec<SlashItem> {
        let snippets = self.options.snippets.iter().enumerate().map(|(i, s)| SlashItem {
            command: SlashCommand::Snippet(i),
            label: s.name.clone(),
            detail: s.body.lines().next().unwrap_or_default().chars().take(60).collect(),
            icon: "text",
            words: "snippet".into(),
        });
        let templates = self.options.templates.iter().enumerate().map(|(i, t)| SlashItem {
            command: SlashCommand::Template(i),
            label: t.name.clone(),
            detail: t!("composer-slash-template").to_owned(),
            icon: "template",
            words: format!("template {}", t.subject),
        });
        snippets.chain(templates).collect()
    }

    /// Puts snippet `i` where the caret is, its placeholders filled in:
    /// formatted (its Markdown) in a formatted message.
    pub(in crate::display::ui) fn insert_snippet(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snippet) = self.options.snippets.get(i).cloned() else { return };
        let text = crate::manage::composing::fill_placeholders(&snippet.body, &self.placeholders(cx));
        if !self.draft.formatted {
            return self.type_at_caret(&text, window, cx);
        }
        self.preview = false;
        let fragment = rhtml::from_markdown(&text, "");
        self.rich.update(cx, |r, cx| {
            r.insert_doc(&fragment, Some(window), cx);
            r.focus(window, cx);
        });
    }

    /// Keeps the selected text as a snippet (its formatting as Markdown).
    pub(in crate::display::ui) fn save_selection_as_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = if self.draft.formatted {
            let r = self.rich.read(cx);
            if r.sel.is_empty() {
                String::new()
            } else {
                let slice = r.doc.slice(r.sel.start(), r.sel.end());
                super::super::paste::html_to_markdown(&rhtml::to_html(&slice)).unwrap_or_else(|| slice.to_plain())
            }
        } else {
            let (text, range) = self.selection(window, cx);
            text.get(range).unwrap_or_default().to_owned()
        };
        let body = body.trim().to_owned();
        if body.is_empty() {
            self.error = Some(t!("composer-select-text-for-a-snippet").into());
            cx.notify();
            return;
        }
        let first = body.lines().next().unwrap_or_default().trim_start_matches(['#', '>', '-', '*', ' ']);
        let mut name: String = first.chars().take(40).collect();
        if name.trim().is_empty() {
            name = t!("composer-snippet").into();
        }
        cx.emit(ComposerEvent::SaveSnippet(crate::manage::composing::Snippet { name, body }));
    }

    /// Carries out the composer's part of a slash command, at the caret.
    pub(super) fn on_slash(&mut self, command: &SlashCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            SlashCommand::Emoji => {
                if self.emoji.is_none() {
                    self.toggle_emoji(window, cx);
                }
            }
            SlashCommand::Date | SlashCommand::Time => {
                let p = self.placeholders(cx);
                let text = if *command == SlashCommand::Date { p.date } else { p.time };
                self.type_at_caret(&text, window, cx);
            }
            SlashCommand::Snippet(i) => self.insert_snippet(*i, window, cx),
            SlashCommand::Template(i) => {
                let Some(t) = self.options.templates.get(*i).cloned() else { return };
                let mut values = self.placeholders(cx);
                if self.subject.read(cx).value().trim().is_empty() && !t.subject.is_empty() {
                    let subject = crate::manage::composing::fill_placeholders(&t.subject, &values);
                    values.subject = subject.clone();
                    self.subject.update(cx, |s, cx| s.set_text(subject, window, cx));
                }
                let text = crate::manage::composing::fill_placeholders(&t.body, &values);
                // At the caret, as a snippet.
                let fragment = rhtml::from_markdown(&text, "");
                self.rich.update(cx, |r, cx| {
                    r.insert_doc(&fragment, Some(window), cx);
                    r.focus(window, cx);
                });
            }
            _ => {}
        }
    }

    // How large pictures are sent, and files too large to send.

    /// Whether the draft has pictures that could be sent smaller.
    fn has_photos(&self) -> bool {
        self.draft.attachments.iter().any(|p| {
            p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))
        })
    }

    /// The draft's picture size: its own, or Settings'.
    pub(super) fn image_size(&self) -> ImageSize {
        self.draft.image_size.unwrap_or(self.options.image_size)
    }

    /// The chip saying how large pictures go, which changes it.
    pub(super) fn pictures_chip(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        if !self.has_photos() {
            return None;
        }
        let c = self.colors;
        let size = self.image_size();
        Some(
            div()
                .id("compose-picture-size")
                .debug_selector(|| "compose-picture-size".into())
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(c.border))
                .text_size(px(12.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(icon_el("image", 13., c.muted))
                .child(t!("composer-pictures-size", size = size.label()))
                .tooltip(|w, cx| Tooltip::new(t!("composer-pictures-size-tip")).build(w, cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    let all = ImageSize::ALL;
                    let i = all.iter().position(|s| *s == this.image_size()).unwrap_or(0);
                    this.draft.image_size = Some(all[(i + 1) % all.len()]);
                    this.changed(cx);
                })),
        )
    }

    /// A note under attachments too large to send by mail: they go as links
    /// (cloud storage set up), or could.
    pub(super) fn large_files_note(&self) -> Option<Div> {
        let total: u64 = self.draft.attachments.iter().map(|p| crate::manage::sealed_files::len(p).unwrap_or(0)).sum();
        if (total as usize) * 137 / 100 < crate::sync::smtp::DEFAULT_LIMIT * 95 / 100 {
            return None;
        }
        let c = self.colors;
        let cloud = &self.options.cloud;
        let (icon, text) = if self.draft.encrypt {
            ("warning", t!("composer-too-large-encrypted").to_owned())
        } else if cloud.configured() {
            let host = match cloud.kind {
                crate::manage::composing::CloudKind::GoogleDrive => "Google Drive".to_owned(),
                crate::manage::composing::CloudKind::OneDrive => "OneDrive".to_owned(),
                _ => url::Url::parse(cloud.url.trim()).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| cloud.url.trim().to_owned()),
            };
            ("cloud", t!("composer-too-large-shared", host = host))
        } else {
            ("warning", t!("composer-too-large-set-up-cloud").to_owned())
        };
        Some(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(icon_el(icon, 13., if icon == "warning" { c.warning } else { c.accent }))
                .child(div().flex_1().min_w_0().child(text)),
        )
    }

    // Mail merge.

    pub(super) fn toggle_merge(&mut self, cx: &mut Context<Self>) {
        self.merge = match self.merge.take() {
            Some(_) => None,
            None => Some(MergePanel::default()),
        };
        cx.notify();
    }

    fn choose_csv(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("composer-merge-choose").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picker.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let read = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let bytes = std::fs::read(&path)?;
                        anyhow::ensure!(bytes.len() < 20 * 1024 * 1024, t!("manage-merge-too-large"));
                        merge::parse_csv(&bytes)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let panel = this.merge.get_or_insert_with(MergePanel::default);
                match read {
                    Ok(sheet) => {
                        panel.column = sheet.email_column().unwrap_or(0);
                        panel.sheet = Some(sheet);
                        panel.file = file;
                        panel.preview = 0;
                        panel.error = None;
                    }
                    Err(e) => panel.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The draft for each row with an address, and how many rows had none.
    fn merged(&mut self, cx: &App) -> (Vec<Draft>, usize) {
        let d = self.snapshot(cx);
        let base = self.placeholders(cx);
        let Some(panel) = &self.merge else { return (vec![], 0) };
        let Some(sheet) = &panel.sheet else { return (vec![], 0) };
        let drafts: Vec<Draft> = (0..sheet.rows.len()).filter_map(|i| merge::personalize(&d, &base, sheet, panel.column, i)).collect();
        let skipped = sheet.rows.len() - drafts.len();
        (drafts, skipped)
    }

    fn send_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let (drafts, skipped) = self.merged(cx);
        if drafts.is_empty() {
            if let Some(p) = &mut self.merge {
                p.error = Some(t!("composer-merge-no-addresses").into());
            }
            cx.notify();
            return;
        }
        if self.draft.subject.trim().is_empty() && self.options.warn_empty_subject {
            if let Some(p) = &mut self.merge {
                p.error = Some(t!("composer-this-message-has-no-subject").into());
            }
            cx.notify();
            return;
        }
        let detail = if skipped > 0 { Some(t!("composer-merge-skipped", count = skipped)) } else { None };
        let answer = window.prompt(
            PromptLevel::Info,
            &t!("composer-merge-confirm", count = drafts.len()),
            detail.as_deref(),
            &[t!("composer-send"), t!("composer-keep-editing")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = answer.await {
                let _ = this.update(cx, |this, cx| {
                    this.merge = None;
                    cx.emit(ComposerEvent::Merge(drafts));
                });
            }
        })
        .detach();
    }

    /// Puts `{key}` at the caret: a column of the spreadsheet.
    fn insert_placeholder(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.type_at_caret(&format!("{{{key}}}"), window, cx);
    }

    pub(super) fn merge_panel(&mut self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        self.merge.as_ref()?;
        let c = self.colors;
        let (drafts, skipped) = self.merged(cx);
        let panel = self.merge.as_ref()?;
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .rounded(px(8.))
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(primary, |d| d.bg(rgb(c.accent)).text_color(rgb(c.on_accent)).hover(move |s| s.bg(rgb(c.accent_hover))))
                .when(!primary, |d| d.border_1().border_color(rgb(c.border)).hover(move |s| s.bg(rgb(c.soft))))
                .child(label)
        };
        let mut menu = self
            .popup("merge-panel", 480., cx, |_, _| {})
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("people", 15., c.accent))
                    .child(div().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t!("composer-mail-merge")))
                    .child(self.icon_button("merge-close", "close", t!("composer-close-mail-merge"), false, cx, |t, _, cx| t.toggle_merge(cx))),
            )
            .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("composer-merge-explained")));
        let Some(sheet) = &panel.sheet else {
            menu = menu.child(div().flex().child(button("merge-choose", t!("composer-merge-choose").into(), true).on_click(cx.listener(|t, _, _, cx| t.choose_csv(cx)))));
            if let Some(e) = &panel.error {
                menu = menu.child(div().text_size(px(12.)).text_color(rgb(c.danger)).child(e.clone()));
            }
            return Some(menu);
        };
        // The file, and its address column (pressed: the next column).
        let column = sheet.columns.get(panel.column).cloned().unwrap_or_default();
        menu = menu.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .child(icon_el("file-text", 13., c.muted))
                .child(div().flex_1().min_w_0().truncate().child(t!("composer-merge-file", file = panel.file.clone(), count = sheet.rows.len())))
                .child(button("merge-other-file", t!("composer-merge-other-file").into(), false).on_click(cx.listener(|t, _, _, cx| t.choose_csv(cx)))),
        );
        menu = menu.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .child(div().text_color(rgb(c.muted)).child(t!("composer-merge-addresses-from")))
                .child(
                    button("merge-column", column, false)
                        .tooltip(|w, cx| Tooltip::new(t!("composer-merge-next-column")).build(w, cx))
                        .on_click(cx.listener(|t, _, _, cx| {
                            if let Some(p) = &mut t.merge
                                && let Some(s) = &p.sheet
                            {
                                p.column = (p.column + 1) % s.columns.len().max(1);
                            }
                            cx.notify();
                        })),
                ),
        );
        // The placeholders its columns make, put at the caret.
        let mut chips = div().flex().flex_wrap().gap_1();
        for (i, col) in sheet.columns.iter().enumerate() {
            let key = merge::key(col);
            if key.is_empty() {
                continue;
            }
            let label = format!("{{{key}}}");
            chips = chips.child(
                div()
                    .id(("merge-placeholder", i))
                    .debug_selector(move || format!("merge-placeholder-{i}"))
                    .h(px(24.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(rgb(c.soft))
                    .text_size(px(12.))
                    .text_color(rgb(c.accent))
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |t, _, w, cx| t.insert_placeholder(&key, w, cx))),
            );
        }
        menu = menu.child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.muted)).child(t!("composer-merge-placeholders"))).child(chips);
        // A preview of one of the messages.
        if let Some(d) = drafts.get(panel.preview.min(drafts.len().saturating_sub(1))) {
            let shown = panel.preview.min(drafts.len() - 1);
            let text: String = d.body.lines().take(4).collect::<Vec<_>>().join("\n");
            menu = menu.child(
                div()
                    .p_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.surface))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().flex_1().text_color(rgb(c.muted)).child(t!("composer-merge-preview", current = shown + 1, count = drafts.len())))
                            .child(self.icon_button("merge-previous", "chevron-left", t!("composer-previous-match"), false, cx, |t, _, cx| {
                                if let Some(p) = &mut t.merge {
                                    p.preview = p.preview.saturating_sub(1);
                                }
                                cx.notify();
                            }))
                            .child(self.icon_button("merge-next", "chevron-right", t!("composer-next-match"), false, cx, |t, _, cx| {
                                if let Some(p) = &mut t.merge {
                                    p.preview += 1;
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().truncate().child(t!("composer-merge-to", to = d.to.clone())))
                    .child(div().truncate().font_weight(FontWeight::SEMIBOLD).child(d.subject.clone()))
                    .child(div().text_color(rgb(c.muted)).max_h(px(72.)).overflow_hidden().child(text)),
            );
        }
        if skipped > 0 {
            menu = menu.child(div().text_size(px(12.)).text_color(rgb(c.warning)).child(t!("composer-merge-skipped", count = skipped)));
        }
        if !self.draft.cc.trim().is_empty() || !self.draft.bcc.trim().is_empty() {
            menu = menu.child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("composer-merge-no-copies")));
        }
        if let Some(e) = &panel.error {
            menu = menu.child(div().text_size(px(12.)).text_color(rgb(c.danger)).child(e.clone()));
        }
        let count = drafts.len();
        Some(
            menu.child(
                div().flex().justify_end().gap_2().child(
                    button("merge-send", t!("composer-merge-send", count = count), true)
                        .when(count == 0 || self.sending, |d| d.opacity(0.5))
                        .on_click(cx.listener(|t, _, w, cx| t.send_merge(w, cx))),
                ),
            ),
        )
    }

    // The More menu.

    pub(super) fn more_menu(&self, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors;
        let row = |id: &'static str, icon: &'static str, label: String| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .h(px(if self.mobile { super::super::mobile::TOUCH } else { 30. }))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .hover(move |s| s.bg(rgb(c.soft)))
                .child(icon_el(icon, 14., c.muted))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let tip = |label: &str, name: &str| keys(&super::super::shortcuts::tip(label, name, cx));
        self.popup("more-menu", 260., cx, |t, cx| {
            t.more_open = false;
            cx.notify();
        })
        .child(row("more-find", "search", tip(t!("composer-find-and-replace"), "find-in-draft")).on_click(cx.listener(|t, _, w, cx| {
            t.more_open = false;
            t.open_find(true, w, cx);
        })))
        .child(row("more-merge", "people", t!("composer-mail-merge-2").to_owned()).on_click(cx.listener(|t, _, _, cx| {
            t.more_open = false;
            t.toggle_merge(cx);
        })))
        .child(row("more-snippet", "text", t!("composer-save-selection-as-snippet").to_owned()).on_click(cx.listener(|t, _, w, cx| {
            t.more_open = false;
            t.save_selection_as_snippet(w, cx);
        })))
    }
}

#[cfg(test)]
impl Composer {
    /// Types `query` into find (and `with` into replace), as typing would.
    pub(in crate::display::ui) fn find_typed(&mut self, query: &str, with: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(true, window, cx);
        }
        let find = self.find.as_ref().expect("open");
        let (q, r) = (find.query.clone(), find.replace.clone());
        q.update(cx, |s, cx| s.set_text(query.to_owned(), window, cx));
        r.update(cx, |s, cx| s.set_text(with.to_owned(), window, cx));
    }
    pub(in crate::display::ui) fn find_count(&self, cx: &App) -> usize {
        self.found(cx).len()
    }
    pub(in crate::display::ui) fn find_current(&self) -> Option<usize> {
        self.find.as_ref().map(|f| f.current)
    }
    pub(in crate::display::ui) fn replace_found(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.find_replace(all, window, cx);
    }
    pub(in crate::display::ui) fn find_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(true, window, cx);
    }
    pub(in crate::display::ui) fn find_open(&self) -> bool {
        self.find.is_some()
    }
    pub(in crate::display::ui) fn emoji_open(&self) -> bool {
        self.emoji.is_some()
    }
    pub(in crate::display::ui) fn pick(&mut self, e: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_emoji(e, window, cx);
    }
    /// Opens mail merge with the spreadsheet `csv`, as choosing its file would.
    pub(in crate::display::ui) fn merge_csv(&mut self, csv: &[u8]) {
        let sheet = merge::parse_csv(csv).expect("a spreadsheet");
        self.merge = Some(MergePanel { column: sheet.email_column().unwrap_or(0), sheet: Some(sheet), file: "people.csv".into(), ..MergePanel::default() });
    }
    pub(in crate::display::ui) fn merge_drafts(&mut self, cx: &App) -> (Vec<Draft>, usize) {
        self.merged(cx)
    }
    pub(in crate::display::ui) fn table_open_for_test(&mut self) {
        self.table_open = Some(None);
    }
    pub(in crate::display::ui) fn style_open_for_test(&mut self) {
        self.style_open = true;
    }
}
