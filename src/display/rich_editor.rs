//! The formatted composer's editor: the message as it will look, edited in
//! place (`manage::richtext` is its document). Bold, lists, quotes,
//! headings, links and pictures show as they are sent; HTML from other
//! apps' drafts shows with its fonts, colors and alignment.
//!
//! Text is laid out a block at a time: each block's text is shaped and
//! wrapped by GPUI, a picture takes a row of its own at its size, and
//! blocks stack with their list markers, quote bars and spacing. Keys are
//! the text fields' own (the "Input" key context), so moving, selecting
//! and deleting work as in every other field; typing comes through GPUI's
//! input handler, so input methods and phones' keyboards work too.
use super::*;
use crate::manage::richtext::{self as rt, Doc, ListKind, Marker, Pos, html as rhtml};
use gpui_base::actions::{SelectDown, SelectLeft, SelectRight, SelectUp};
use gpui_component::input as ia;
use std::path::PathBuf;
use unicode_segmentation::UnicodeSegmentation as _;

/// What a rich editor tells its composer.
pub(super) enum RichEvent {
    Changed,
}

/// A selection: where it started and where it is now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Selection {
    pub(super) anchor: Pos,
    pub(super) head: Pos,
}
impl Selection {
    fn caret(p: Pos) -> Self {
        Selection { anchor: p, head: p }
    }
    pub(super) fn start(&self) -> Pos {
        self.anchor.min(self.head)
    }
    pub(super) fn end(&self) -> Pos {
        self.anchor.max(self.head)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone)]
struct Snapshot {
    doc: Doc,
    sel: Selection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

/// A text format the toolbar toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
}

/// What the selection is, for the toolbar's buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Active {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) underline: bool,
    pub(super) strike: bool,
    pub(super) code: bool,
    pub(super) link: bool,
    pub(super) heading: bool,
    pub(super) bullets: bool,
    pub(super) numbers: bool,
    pub(super) quote: bool,
    pub(super) align: rt::Align,
}

/// A picture the editor shows, decoded off the main thread.
enum Picture {
    /// Its size when known, while it decodes.
    Loading(Option<(f32, f32)>),
    Ready(Arc<RenderImage>, (f32, f32)),
    Failed,
}

/// The largest side a picture is decoded at: enough for any window.
const PICTURE_MAX: u32 = 2400;

thread_local! {
    /// What was last copied from a rich editor, so pasting it into one
    /// keeps its formatting (the system clipboard holds its text).
    static COPIED: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

/// The look the composer gives the editor.
#[derive(Clone, Copy)]
pub(super) struct Look {
    pub(super) text: u32,
    pub(super) muted: u32,
    pub(super) accent: u32,
    pub(super) border: u32,
    pub(super) soft: u32,
    pub(super) surface: u32,
    /// Misspelled words' underline.
    pub(super) danger: u32,
    pub(super) dark: bool,
    pub(super) size: f32,
}

pub(super) struct RichEditor {
    pub(super) doc: Doc,
    pub(super) sel: Selection,
    /// What an input method is composing.
    marked: Option<(Pos, Pos)>,
    /// The style typing continues in, set by a format toggled with nothing
    /// selected; cleared when the caret moves.
    pending: Option<Rc<rt::Style>>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, std::time::Instant, Pos)>,
    /// Counts edits.
    pub(super) revision: u64,
    focus: FocusHandle,
    pub(super) scroll: ScrollHandle,
    layout: Option<Rc<Layout>>,
    selecting: bool,
    /// The caret moved: bring it into view.
    reveal: bool,
    /// Where up and down aim, across lines of different lengths.
    goal_x: Option<Pixels>,
    /// The draft's attachments, which `file:` pictures name.
    files: Vec<PathBuf>,
    pictures: HashMap<String, Picture>,
    placeholder: SharedString,
    pub(super) disabled: bool,
    pub(super) look: Look,
    /// Underline words spelled wrong (Settings → Composing).
    pub(super) spellcheck: bool,
    /// A picture being resized by its grip.
    resize: Option<Resize>,
    /// Changes what is laid out without an edit (a resize, spelling).
    transient: u64,
    /// Where the pointer is while selecting, for scrolling the selection
    /// on past the edge.
    drag_point: Option<Point<Pixels>>,
    autoscrolling: bool,
    /// When a finger last touched it: a press then is a tap.
    last_touch: Option<std::time::Instant>,
    /// The selection a touch made, which shows handles while it lasts.
    touch_range: Option<Selection>,
    /// The handle a finger is dragging.
    handle_drag: Option<gpui_base::SelectionEdge>,
    /// The edit menu (right click, or after a touch selection).
    menu: Option<Menu>,
}

/// A picture's grip being dragged.
struct Resize {
    at: Pos,
    from: Point<Pixels>,
    start_width: f32,
    aspect: f32,
    width: f32,
    max: f32,
}

/// The edit menu, and the misspelled word it was opened on.
struct Menu {
    at: Point<Pixels>,
    word: Option<(Pos, Pos, String)>,
    suggestions: Vec<String>,
}

impl EventEmitter<RichEvent> for RichEditor {}

impl Focusable for RichEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RichEditor {
    pub(super) fn new(doc: Doc, placeholder: &str, look: Look, cx: &mut Context<Self>) -> Self {
        let mut doc = doc;
        doc.ensure_block();
        Self {
            doc,
            sel: Selection::default(),
            marked: None,
            pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            revision: 0,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            layout: None,
            selecting: false,
            reveal: false,
            goal_x: None,
            files: Vec::new(),
            pictures: HashMap::new(),
            placeholder: placeholder.to_owned().into(),
            disabled: false,
            look,
            spellcheck: true,
            resize: None,
            transient: 0,
            drag_point: None,
            autoscrolling: false,
            last_touch: None,
            touch_range: None,
            handle_drag: None,
            menu: None,
        }
    }

    // The document.

    /// Starts over with `doc`, as when a draft is loaded again; not undone.
    pub(super) fn set_doc(&mut self, doc: Doc, cx: &mut Context<Self>) {
        self.doc = doc;
        self.doc.ensure_block();
        self.sel = Selection::default();
        self.marked = None;
        self.pending = None;
        self.undo.clear();
        self.redo.clear();
        self.revision += 1;
        cx.notify();
    }
    pub(super) fn html(&self) -> String {
        rhtml::to_html(&self.doc)
    }
    pub(super) fn plain(&self) -> String {
        self.doc.to_plain()
    }
    /// The attachments `file:` pictures may name.
    pub(super) fn set_files(&mut self, files: &[PathBuf]) {
        if self.files != files {
            self.files = files.to_vec();
        }
    }
    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }
    pub(super) fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }
    pub(super) fn focus_handle_ref(&self) -> FocusHandle {
        self.focus.clone()
    }

    // Editing, with undo.

    fn begin(&mut self, kind: EditKind) {
        let now = std::time::Instant::now();
        // Typing (or deleting) on from where the last of it ended is one step.
        let coalesce = matches!(kind, EditKind::Typing | EditKind::Deleting)
            && self.sel.is_empty()
            && self.last_edit.is_some_and(|(k, at, pos)| {
                k == kind && now.duration_since(at) < Duration::from_secs(2) && pos == self.sel.head
            });
        if !coalesce {
            self.undo.push(Snapshot { doc: self.doc.clone(), sel: self.sel });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, now, self.sel.head));
    }
    fn finish(&mut self, kind: EditKind, window: Option<&Window>, cx: &mut Context<Self>) {
        self.doc.ensure_block();
        self.sel.anchor = self.doc.clamp(self.sel.anchor);
        self.sel.head = self.doc.clamp(self.sel.head);
        if let Some((k, _, pos)) = &mut self.last_edit
            && *k == kind
        {
            *pos = self.sel.head;
        }
        self.revision += 1;
        self.reveal = true;
        self.goal_x = None;
        self.menu = None;
        self.touch_range = None;
        if let Some(w) = window {
            w.notify_text_input_changed(&self.focus);
        }
        cx.emit(RichEvent::Changed);
        cx.notify();
    }
    /// Removes the selection; where the caret is after.
    fn delete_selection(&mut self) -> Pos {
        let at = self.doc.delete(self.sel.start(), self.sel.end());
        self.sel = Selection::caret(at);
        at
    }
    /// The style typed text takes.
    fn typing_style(&self) -> Rc<rt::Style> {
        if let Some(p) = &self.pending {
            return p.clone();
        }
        let p = self.sel.start();
        let style = self.doc.block(p.block).style_at(p.offset);
        // At the end of a link, typing doesn't lengthen it.
        let after_link = style.link.is_some() && {
            let b = self.doc.block(p.block);
            b.style_at(p.offset + 1).link != style.link || p.offset >= b.len()
        };
        if after_link { Rc::new(style.continued()) } else { style }
    }
    /// Types `text` over the selection; `\n` starts new lines.
    pub(super) fn insert_text(&mut self, text: &str, window: Option<&Window>, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let kind = if text.contains('\n') { EditKind::Other } else { EditKind::Typing };
        self.begin(kind);
        let style = self.typing_style();
        let mut at = self.delete_selection();
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                at = self.doc.split(at);
            }
            at = self.doc.insert_text(at, line, style.clone());
        }
        self.sel = Selection::caret(at);
        self.pending = Some(style);
        self.finish(kind, window, cx);
    }
    /// Puts `fragment` (pasted formatting) over the selection.
    pub(super) fn insert_doc(&mut self, fragment: &Doc, window: Option<&Window>, cx: &mut Context<Self>) {
        if self.disabled || fragment.blocks.is_empty() {
            return;
        }
        self.begin(EditKind::Other);
        let at = self.delete_selection();
        let end = self.doc.insert_doc(at, fragment);
        self.sel = Selection::caret(end);
        self.pending = None;
        self.finish(EditKind::Other, window, cx);
    }
    /// Puts pictures (attachments, by file name) at the caret, each on its
    /// own line.
    pub(super) fn insert_pictures(&mut self, paths: &[PathBuf], window: Option<&Window>, cx: &mut Context<Self>) {
        let names: Vec<String> = paths
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if names.is_empty() || self.disabled {
            return;
        }
        for p in paths {
            if !self.files.contains(p) {
                self.files.push(p.clone());
            }
        }
        self.begin(EditKind::Other);
        let style = self.typing_style();
        let mut at = self.delete_selection();
        for name in names {
            let alt = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem).to_owned();
            // A picture in a line of text starts a line of its own.
            if at.offset > 0 {
                at = self.doc.split(at);
            }
            at = self.doc.insert_image(at, rt::Image { src: format!("file:{name}"), alt, width: None, height: None, attrs: vec![] }, style.clone());
            if at.offset < self.doc.block(at.block).len() {
                at = self.doc.split(at);
            }
        }
        self.sel = Selection::caret(at);
        self.finish(EditKind::Other, window, cx);
    }
    /// Writes `fragment` at the very start (a suggested reply, a template),
    /// above what is there, and puts the caret after it.
    pub(super) fn insert_at_start(&mut self, fragment: &Doc, window: Option<&Window>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        let first_empty = self.doc.blocks.len() == 1 && self.doc.block(0).is_empty();
        let mut frag = fragment.clone();
        let mut added = frag.blocks.len();
        // What was written stays, below a blank line.
        if !first_empty && !self.doc.block(0).is_empty() {
            frag.blocks.push(Rc::new(rt::Block::paragraph("div", "")));
            added += 1;
        }
        if first_empty {
            self.doc.blocks.clear();
        }
        for (i, b) in frag.blocks.into_iter().enumerate() {
            self.doc.blocks.insert(i, b);
        }
        let last = fragment.blocks.len().saturating_sub(1).min(added.saturating_sub(1));
        self.sel = Selection::caret(Pos::new(last, self.doc.block(last).len()));
        self.finish(EditKind::Other, window, cx);
    }
    /// Replaces blocks `range` with `with` (a signature swapped).
    pub(super) fn replace_blocks(&mut self, range: std::ops::Range<usize>, with: Vec<Rc<rt::Block>>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        let n = with.len();
        let start = range.start;
        self.doc.blocks.splice(range, with);
        self.doc.ensure_block();
        if self.sel.head.block >= start + n || self.sel.anchor.block >= start {
            self.sel = Selection::caret(self.doc.clamp(self.sel.head));
        }
        self.finish(EditKind::Other, None, cx);
    }
    /// Removes the blocks from `from` on (a quote not to be sent).
    pub(super) fn truncate_blocks(&mut self, from: usize, cx: &mut Context<Self>) {
        if from >= self.doc.blocks.len() {
            return;
        }
        self.begin(EditKind::Other);
        self.doc.blocks.truncate(from);
        // The blank lines before it go too.
        while self.doc.blocks.len() > 1 && self.doc.blocks.last().is_some_and(|b| b.text().trim().is_empty()) {
            self.doc.blocks.pop();
        }
        self.doc.ensure_block();
        self.sel = Selection::caret(self.doc.clamp(self.sel.head));
        self.finish(EditKind::Other, None, cx);
    }

    pub(super) fn undo(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot { doc: self.doc.clone(), sel: self.sel });
            self.doc = s.doc;
            self.sel = s.sel;
            self.after_history(window, cx);
        }
    }
    fn redo(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot { doc: self.doc.clone(), sel: self.sel });
            self.doc = s.doc;
            self.sel = s.sel;
            self.after_history(window, cx);
        }
    }
    fn after_history(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        self.last_edit = None;
        self.marked = None;
        self.pending = None;
        self.revision += 1;
        self.reveal = true;
        if let Some(w) = window {
            w.notify_text_input_changed(&self.focus);
        }
        cx.emit(RichEvent::Changed);
        cx.notify();
    }

    // Formatting.

    fn has_mark(&self, s: &rt::Style, info: &rt::Info, mark: Mark) -> bool {
        match mark {
            Mark::Bold => s.bold.unwrap_or(info.bold),
            Mark::Italic => s.italic.unwrap_or(info.italic),
            Mark::Underline => s.underline,
            Mark::Strike => s.strike,
            Mark::Code => s.code,
        }
    }
    /// Turns `mark` on for the selection, or off where all of it has it;
    /// with nothing selected, for what is typed next.
    pub(super) fn toggle_mark(&mut self, mark: Mark, window: Option<&Window>, cx: &mut Context<Self>) {
        let infos = self.doc.infos(16.);
        if self.sel.is_empty() {
            let mut style = (*self.typing_style()).clone();
            let info = &infos[self.sel.head.block];
            let on = !self.has_mark(&style, info, mark);
            set_mark(&mut style, mark, on, info);
            self.pending = Some(Rc::new(style));
            cx.notify();
            return;
        }
        let styles = self.doc.styles_in(self.sel.start(), self.sel.end());
        let all = !styles.is_empty() && styles.iter().all(|(b, s)| self.has_mark(s, &infos[*b], mark));
        self.begin(EditKind::Other);
        let (start, end) = (self.sel.start(), self.sel.end());
        // Bold and italic are relative to the block (a heading is bold):
        // block by block.
        for (bi, info) in infos.iter().enumerate().take(end.block + 1).skip(start.block) {
            let from = if bi == start.block { start.offset } else { 0 };
            let to = if bi == end.block { end.offset } else { self.doc.block(bi).len() };
            let info = info.clone();
            self.doc.restyle(Pos::new(bi, from), Pos::new(bi, to), &|s| set_mark(s, mark, !all, &info));
        }
        self.finish(EditKind::Other, window, cx);
    }
    /// Links the selection to `url`, or unlinks it (`None`); with nothing
    /// selected, writes the address itself as a link.
    pub(super) fn set_link(&mut self, url: Option<String>, window: Option<&Window>, cx: &mut Context<Self>) {
        let url = url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
        if self.sel.is_empty() {
            // On a link: the whole link changes.
            if let Some((start, end)) = self.link_range() {
                self.begin(EditKind::Other);
                self.doc.restyle(start, end, &|s| s.link = url.clone());
                self.finish(EditKind::Other, window, cx);
                return;
            }
            let Some(url) = url else { return };
            self.begin(EditKind::Other);
            let style = Rc::new(rt::Style { link: Some(url.clone()), ..(*self.typing_style()).clone() });
            let at = self.doc.insert_text(self.sel.head, &url, style);
            self.sel = Selection::caret(at);
            self.pending = Some(Rc::new(rt::Style { link: None, ..(*self.typing_style()).clone() }));
            self.finish(EditKind::Other, window, cx);
            return;
        }
        self.begin(EditKind::Other);
        self.doc.restyle(self.sel.start(), self.sel.end(), &|s| s.link = url.clone());
        self.finish(EditKind::Other, window, cx);
    }
    /// The link at the caret, if any: its whole extent.
    fn link_range(&self) -> Option<(Pos, Pos)> {
        let p = self.sel.head;
        let b = self.doc.block(p.block);
        let link = b.style_at(p.offset).link.clone()?;
        let mut at = 0;
        let mut start = None;
        let mut end = None;
        for run in &b.runs {
            let (s, e) = (at, at + run.text.len());
            at = e;
            if run.style.link.as_ref() == Some(&link) {
                if start.is_none() || end.is_some_and(|x| x != s) {
                    if s > p.offset {
                        break;
                    }
                    start = Some(s);
                }
                end = Some(e);
            } else if end.is_some_and(|x| x >= p.offset) {
                break;
            } else {
                start = None;
                end = None;
            }
        }
        Some((Pos::new(p.block, start?), Pos::new(p.block, end?)))
    }
    /// The link at the selection, for editing it.
    pub(super) fn link(&self) -> Option<String> {
        let p = self.sel.start();
        let b = self.doc.block(p.block);
        b.style_at(if self.sel.is_empty() { p.offset } else { p.offset + 1 }).link.clone()
    }
    pub(super) fn selected_text(&self) -> String {
        self.doc.text_between(self.sel.start(), self.sel.end())
    }
    fn selected_blocks(&self) -> std::ops::RangeInclusive<usize> {
        let (s, e) = (self.sel.start(), self.sel.end());
        // A selection ending at a line's start leaves that line out.
        let last = if e.offset == 0 && e.block > s.block { e.block - 1 } else { e.block };
        s.block..=last
    }
    pub(super) fn toggle_list(&mut self, kind: ListKind, window: Option<&Window>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        self.doc.toggle_list(self.selected_blocks(), kind);
        self.finish(EditKind::Other, window, cx);
    }
    pub(super) fn toggle_quote(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        self.doc.toggle_quote(self.selected_blocks());
        self.finish(EditKind::Other, window, cx);
    }
    pub(super) fn toggle_heading(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        self.doc.toggle_heading(self.selected_blocks(), 2);
        self.finish(EditKind::Other, window, cx);
    }
    pub(super) fn set_align(&mut self, align: rt::Align, window: Option<&Window>, cx: &mut Context<Self>) {
        self.begin(EditKind::Other);
        self.doc.set_align(self.selected_blocks(), align);
        self.finish(EditKind::Other, window, cx);
    }
    /// Plain text again: every format and link taken off the selection.
    pub(super) fn clear_formatting(&mut self, window: Option<&Window>, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            self.pending = Some(rt::Style::plain());
            cx.notify();
            return;
        }
        self.begin(EditKind::Other);
        self.doc.restyle(self.sel.start(), self.sel.end(), &|s| *s = rt::Style::default());
        self.finish(EditKind::Other, window, cx);
    }

    pub(super) fn active(&self) -> Active {
        let infos = self.doc.infos(16.);
        let style = if self.sel.is_empty() {
            self.typing_style()
        } else {
            let p = self.sel.start();
            self.doc.block(p.block).style_at(p.offset + 1)
        };
        let blocks = self.selected_blocks();
        let info = &infos[self.sel.start().block.min(infos.len() - 1)];
        let in_list = |tag: &str| {
            blocks.clone().all(|i| {
                rt::innermost_list(&self.doc.block(i).path).is_some_and(|k| self.doc.block(i).path[k].is(tag))
            })
        };
        Active {
            bold: self.has_mark(&style, info, Mark::Bold),
            italic: self.has_mark(&style, info, Mark::Italic),
            underline: style.underline,
            strike: style.strike,
            code: style.code,
            link: style.link.is_some(),
            heading: info.heading.is_some(),
            bullets: in_list("ul"),
            numbers: in_list("ol"),
            quote: info.quotes > 0,
            align: info.align,
        }
    }

    // Moving.

    fn block_text(&self, i: usize) -> String {
        self.doc.block(i).text()
    }
    fn prev_pos(&self, p: Pos) -> Pos {
        if p.offset == 0 {
            if p.block == 0 {
                return p;
            }
            return Pos::new(p.block - 1, self.doc.block(p.block - 1).len());
        }
        let text = self.block_text(p.block);
        let at = text[..p.offset].grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
        Pos::new(p.block, at)
    }
    fn next_pos(&self, p: Pos) -> Pos {
        let text = self.block_text(p.block);
        if p.offset >= text.len() {
            if p.block + 1 >= self.doc.blocks.len() {
                return p;
            }
            return Pos::new(p.block + 1, 0);
        }
        let at = text[p.offset..].graphemes(true).next().map_or(text.len(), |g| p.offset + g.len());
        Pos::new(p.block, at)
    }
    fn prev_word(&self, p: Pos) -> Pos {
        if p.offset == 0 {
            return self.prev_pos(p);
        }
        let text = self.block_text(p.block);
        let at = text[..p.offset]
            .unicode_word_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        Pos::new(p.block, at)
    }
    fn next_word(&self, p: Pos) -> Pos {
        let text = self.block_text(p.block);
        if p.offset >= text.len() {
            return self.next_pos(p);
        }
        let at = text[p.offset..]
            .unicode_word_indices()
            .next()
            .map_or(text.len(), |(i, w)| p.offset + i + w.len());
        Pos::new(p.block, at)
    }
    /// The word around `p` (a double click).
    fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let text = self.block_text(p.block);
        for (i, w) in text.split_word_bound_indices() {
            if p.offset >= i && p.offset < i + w.len() || p.offset == text.len() && i + w.len() == text.len() {
                return (Pos::new(p.block, i), Pos::new(p.block, i + w.len()));
            }
        }
        (p, p)
    }

    fn move_to(&mut self, p: Pos, select: bool, cx: &mut Context<Self>) {
        let p = self.doc.clamp(p);
        if select {
            self.sel.head = p;
        } else {
            self.sel = Selection::caret(p);
        }
        self.pending = None;
        self.marked = None;
        self.last_edit = None;
        self.reveal = true;
        cx.notify();
    }
    fn horizontal(&mut self, forward: bool, select: bool, word: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        if !select && !self.sel.is_empty() {
            let p = if forward { self.sel.end() } else { self.sel.start() };
            return self.move_to(p, false, cx);
        }
        let h = self.sel.head;
        let p = match (forward, word) {
            (true, false) => self.next_pos(h),
            (false, false) => self.prev_pos(h),
            (true, true) => self.next_word(h),
            (false, true) => self.prev_word(h),
        };
        self.move_to(p, select, cx);
    }
    fn vertical(&mut self, down: bool, select: bool, page: bool, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.clone() else { return };
        let Some(caret) = layout.caret(&self.doc, self.sel.head) else { return };
        let x = *self.goal_x.get_or_insert(caret.origin.x);
        let step = if page { self.scroll.bounds().size.height.max(px(100.)) - px(20.) } else { px(1.) };
        let mut y = if down { caret.bottom() + step } else { caret.top() - step };
        let mut x = x;
        // From a block's first or last row, the nearest row of the block
        // below or above (past the space between paragraphs, in the next
        // row of a table).
        let bi = self.sel.head.block;
        if !page && let Some(b) = layout.blocks.get(bi) {
            let leaving = if down { y >= b.bottom } else { y < b.top };
            if leaving && let Some(n) = layout.neighbour(bi, x, down).and_then(|n| layout.blocks.get(n)) {
                y = if down { n.top + px(1.) } else { n.bottom - px(1.) };
                x = x.clamp(n.left, n.right);
            }
        }
        let p = if y < px(0.) {
            Pos::default()
        } else if y > layout.height {
            self.doc.end()
        } else {
            layout.hit(&self.doc, point(x, y))
        };
        let goal = self.goal_x;
        self.move_to(p, select, cx);
        self.goal_x = goal;
    }
    fn line_edge(&mut self, end: bool, select: bool, cx: &mut Context<Self>) {
        let h = self.sel.head;
        let p = self
            .layout
            .as_ref()
            .and_then(|l| l.row_edge(&self.doc, h, end))
            .unwrap_or_else(|| Pos::new(h.block, if end { self.doc.block(h.block).len() } else { 0 }));
        self.goal_x = None;
        self.move_to(p, select, cx);
    }

    // Deleting.

    fn delete_back(&mut self, to: Option<Pos>, window: &Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if self.sel.is_empty() {
            let h = self.sel.head;
            // At a line's start: a list item, quote or heading becomes an
            // ordinary line first, as in other editors.
            if h.offset == 0 && to.is_none() {
                let path = self.doc.block(h.block).path.clone();
                if rt::innermost_list(&path).is_some() {
                    self.begin(EditKind::Other);
                    self.doc.outdent_item(h.block);
                    return self.finish(EditKind::Other, Some(window), cx);
                }
                if path.last().is_some_and(|e| e.is("blockquote") || e.heading_level().is_some()) {
                    self.begin(EditKind::Other);
                    if path.last().is_some_and(|e| e.is("blockquote")) {
                        self.doc.toggle_quote(h.block..=h.block);
                    } else {
                        self.doc.toggle_heading(h.block..=h.block, 2);
                    }
                    return self.finish(EditKind::Other, Some(window), cx);
                }
                if h.block == 0 {
                    return;
                }
            }
            let target = to.unwrap_or_else(|| self.prev_pos(h));
            if target == h {
                return;
            }
            self.sel.anchor = target;
        }
        self.begin(EditKind::Deleting);
        self.delete_selection();
        self.finish(EditKind::Deleting, Some(window), cx);
    }
    fn delete_forward(&mut self, to: Option<Pos>, window: &Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        if self.sel.is_empty() {
            let target = to.unwrap_or_else(|| self.next_pos(self.sel.head));
            if target == self.sel.head {
                return;
            }
            self.sel.anchor = target;
        }
        self.begin(EditKind::Deleting);
        self.delete_selection();
        self.finish(EditKind::Deleting, Some(window), cx);
    }

    fn enter(&mut self, action: &ia::Enter, window: &mut Window, cx: &mut Context<Self>) {
        if action.secondary || self.disabled {
            cx.propagate();
            return;
        }
        if action.shift {
            return self.insert_text_raw("\n", window, cx);
        }
        let h = self.sel.start();
        let block = self.doc.block(h.block);
        if block.is_pre() {
            return self.insert_text_raw("\n", window, cx);
        }
        // Enter on an empty item ends the list.
        if self.sel.is_empty() && block.is_empty() && rt::innermost_list(&block.path).is_some() {
            self.begin(EditKind::Other);
            self.doc.outdent_item(h.block);
            return self.finish(EditKind::Other, Some(window), cx);
        }
        self.begin(EditKind::Other);
        let style = self.typing_style();
        let at = self.delete_selection();
        let at = self.doc.split(at);
        self.sel = Selection::caret(at);
        self.pending = Some(Rc::new(style.continued()));
        self.finish(EditKind::Other, Some(window), cx);
    }
    /// Text typed in place, `\n` a line break inside the block.
    fn insert_text_raw(&mut self, text: &str, window: &Window, cx: &mut Context<Self>) {
        self.begin(EditKind::Typing);
        let style = self.typing_style();
        let at = self.delete_selection();
        let at = self.doc.insert_text(at, text, style.clone());
        self.sel = Selection::caret(at);
        self.pending = Some(style);
        self.finish(EditKind::Typing, Some(window), cx);
    }

    // The clipboard.

    fn copy(&mut self, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            return;
        }
        let text = self.selected_text();
        let fragment = self.doc.slice(self.sel.start(), self.sel.end());
        COPIED.with(|c| *c.borrow_mut() = Some((text.clone(), rhtml::to_html(&fragment))));
        // Other apps get its formatting too, its pictures in it.
        let shared = rhtml::to_html(&self.shared_fragment(fragment));
        if !write_clipboard_html(&shared, &text) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    /// `fragment` as another app can show it: the draft's pictures carried
    /// in it (`data:`), as no other app can read its files.
    fn shared_fragment(&self, mut fragment: Doc) -> Doc {
        for i in 0..fragment.blocks.len() {
            if !fragment.blocks[i].runs.iter().any(|r| r.image.as_ref().is_some_and(|i| i.file_name().is_some())) {
                continue;
            }
            let files = self.files.clone();
            let b = fragment.block_mut(i);
            b.runs.retain_mut(|run| {
                let Some(image) = &run.image else { return true };
                let Some(name) = image.file_name() else { return true };
                let path = files.iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name));
                let data = path.filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() < 8 * 1024 * 1024)).and_then(|p| {
                    let bytes = std::fs::read(p).ok()?;
                    let mime = mime_guess::from_path(p).first_or_octet_stream();
                    use base64::Engine as _;
                    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
                });
                match data {
                    Some(src) => {
                        run.image = Some(Rc::new(rt::Image { src, ..(**image).clone() }));
                        true
                    }
                    None => false,
                }
            });
        }
        fragment
    }
    /// What a paste inserts when it was copied from a rich editor: its
    /// formatting, while the clipboard still holds that text.
    pub(super) fn copied_fragment(text: &str) -> Option<Doc> {
        COPIED.with(|c| {
            let c = c.borrow();
            let (copied, html) = c.as_ref()?;
            (copied == text).then(|| rhtml::parse(html, true))
        })
    }
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else { return };
        match Self::copied_fragment(&text) {
            Some(fragment) => self.insert_doc(&fragment, Some(window), cx),
            None => self.insert_plain(&text, Some(window), cx),
        }
    }
    /// Plain text pasted: each line its own line.
    pub(super) fn insert_plain(&mut self, text: &str, window: Option<&Window>, cx: &mut Context<Self>) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.insert_text(&text, window, cx);
    }

    // Mouse.

    /// Whether the press now is a finger's.
    fn touching(&self, cx: &App) -> bool {
        gpui_base::GlobalState::is_touch_press(cx)
            || self.last_touch.is_some_and(|t| t.elapsed() < Duration::from_millis(800))
    }
    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.default_prevented() {
            return;
        }
        let touch = self.touching(cx);
        window.focus(&self.focus, cx);
        if touch && !self.disabled {
            window.request_virtual_keyboard();
        }
        gpui_base::GlobalState::suppress_text_selection(cx);
        let had_menu = self.menu.take().is_some();
        let Some(layout) = self.layout.clone() else { return };
        let local = layout.local(event.position);
        // The selected picture's grip resizes it.
        if let Some(at) = self.selected_picture()
            && let Some(r) = layout.image_bounds(at)
        {
            let grip = Bounds::new(r.bottom_right() - point(px(GRIP), px(GRIP)), size(px(GRIP * 2.), px(GRIP * 2.)));
            if grip.contains(&local) {
                let image = self.doc.block(at.block).images().find(|(o, _)| *o == at.offset).map(|(_, i)| i.clone());
                let max = self.layout.as_ref().and_then(|l| l.blocks.get(at.block)).map_or(2000., |b| f32::from(b.right - b.left));
                let w = f32::from(r.size.width);
                self.resize = Some(Resize {
                    at,
                    from: event.position,
                    start_width: w,
                    aspect: f32::from(r.size.height) / w.max(1.),
                    width: w,
                    max: image.and(Some(max)).unwrap_or(max),
                });
                cx.notify();
                return;
            }
        }
        let p = layout.hit(&self.doc, local);
        // A click on a picture selects it.
        if let Some((image, _)) = layout.image_at(local) {
            self.sel = Selection { anchor: image, head: Pos::new(image.block, image.offset + rt::OBJECT.len_utf8()) };
            self.pending = None;
            self.touch_range = None;
            cx.notify();
            return;
        }
        // A finger places the caret (dragging scrolls); a tap on the caret
        // asks for the menu, as on phones.
        if touch {
            let on_caret = self.sel.is_empty() && self.sel.head == p && !had_menu;
            self.touch_range = None;
            self.move_to(p, false, cx);
            if on_caret {
                self.open_menu(event.position, cx);
            }
            return;
        }
        self.touch_range = None;
        match event.click_count {
            2 => {
                let (s, e) = self.word_at(p);
                self.sel = Selection { anchor: s, head: e };
                cx.notify();
            }
            n if n >= 3 => {
                let len = self.doc.block(p.block).len();
                self.sel = Selection { anchor: Pos::new(p.block, 0), head: Pos::new(p.block, len) };
                cx.notify();
            }
            _ => {
                self.selecting = true;
                self.move_to(p, event.modifiers.shift, cx);
            }
        }
        self.goal_x = None;
    }
    /// A right click: the edit menu, with spellings for a misspelled word.
    fn on_right_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(layout) = self.layout.clone() else { return };
        let p = layout.hit(&self.doc, layout.local(event.position));
        if self.sel.is_empty() || p < self.sel.start() || p > self.sel.end() {
            self.move_to(p, false, cx);
        }
        self.open_menu(event.position, cx);
    }
    /// Opens the edit menu at `at` (in the window), offering spellings when
    /// the caret is in a misspelled word.
    fn open_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let spelling = crate::manage::spelling::spelling();
        let mut word = None;
        let mut suggestions = vec![];
        if self.spellcheck && self.sel.is_empty() {
            let h = self.sel.head;
            let text = self.block_text(h.block);
            if let Some(r) = crate::manage::spelling::words(&text).into_iter().find(|r| h.offset >= r.start && h.offset <= r.end) {
                let w = &text[r.clone()];
                if spelling.misspelled(w) {
                    suggestions = spelling.suggest(w);
                    word = Some((Pos::new(h.block, r.start), Pos::new(h.block, r.end), w.to_owned()));
                }
            }
        }
        self.menu = Some(Menu { at, word, suggestions });
        cx.notify();
    }
    fn dragging(&self) -> bool {
        self.selecting || self.resize.is_some()
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Moves without the button held aren't a drag (a window refresh's).
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(r) = &mut self.resize {
            let dx = f32::from(event.position.x - r.from.x);
            let dy = f32::from(event.position.y - r.from.y) / r.aspect.max(0.01);
            // The larger move decides, keeping the picture's shape.
            let delta = if dx.abs() > dy.abs() { dx } else { dy };
            r.width = (r.start_width + delta).clamp(16., r.max.max(16.));
            self.transient += 1;
            cx.notify();
            return;
        }
        if !self.selecting {
            return;
        }
        let Some(layout) = self.layout.clone() else { return };
        let p = layout.hit(&self.doc, layout.local(event.position));
        if p != self.sel.head {
            self.sel.head = p;
            cx.notify();
        }
        self.drag_point = Some(event.position);
        let view = self.scroll.bounds();
        if (event.position.y < view.top() || event.position.y > view.bottom()) && !self.autoscrolling {
            self.autoscroll(window, cx);
        }
    }
    /// Scrolls on while a selection is dragged past the top or bottom, the
    /// selection following, faster the further out the pointer is.
    fn autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autoscrolling = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let go_on = this
                    .update(cx, |this, cx| {
                        let Some(at) = this.drag_point.filter(|_| this.selecting) else { return false };
                        let view = this.scroll.bounds();
                        let over = if at.y < view.top() {
                            at.y - view.top()
                        } else if at.y > view.bottom() {
                            at.y - view.bottom()
                        } else {
                            return false;
                        };
                        let step = (over / 4.).clamp(px(-40.), px(40.));
                        let mut offset = this.scroll.offset();
                        let max = this.scroll.max_offset();
                        offset.y = (offset.y - step).clamp(-max.y, px(0.));
                        this.scroll.set_offset(offset);
                        if let Some(layout) = this.layout.clone() {
                            // The layout moved with the scroll; aim where it will be.
                            let target = point(at.x, at.y.clamp(view.top(), view.bottom()));
                            let local = layout.local(target) + point(px(0.), step);
                            this.sel.head = layout.hit(&this.doc, local);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.autoscrolling = false);
        })
        .detach();
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        self.drag_point = None;
        if let Some(r) = self.resize.take() {
            if (r.width - r.start_width).abs() >= 1. {
                self.begin(EditKind::Other);
                self.doc.set_image_size(r.at, r.width, r.width * r.aspect);
                self.sel = Selection { anchor: r.at, head: Pos::new(r.at.block, r.at.offset + rt::OBJECT.len_utf8()) };
                self.finish(EditKind::Other, Some(window), cx);
            }
            self.transient += 1;
            cx.notify();
        }
    }

    // Touch.

    /// The picture alone selected, which gets a grip.
    fn selected_picture(&self) -> Option<Pos> {
        let (s, e) = (self.sel.start(), self.sel.end());
        (s.block == e.block && e.offset == s.offset + rt::OBJECT.len_utf8() && self.doc.block(s.block).images().any(|(o, _)| o == s.offset))
            .then_some(s)
    }
    /// Whether the selection shows a touch selection's handles.
    fn touch_handles(&self) -> bool {
        self.touch_range.is_some_and(|r| r == self.sel) && !self.sel.is_empty()
    }
    /// A long press: the word under the finger (or the caret, between
    /// words), then what the finger sweeps over; let go, the menu.
    fn long_press(&mut self, event: &LongPressEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.clone() else { return };
        match event.phase {
            TouchPhase::Started => {
                if self.disabled {
                    return;
                }
                window.focus(&self.focus, cx);
                gpui_base::GlobalState::suppress_text_selection(cx);
                self.menu = None;
                let p = layout.hit(&self.doc, layout.local(event.start_position));
                let (s, e) = self.word_at(p);
                let text = self.block_text(p.block);
                let word = s != e && !text[s.offset..e.offset].trim().is_empty();
                self.sel = if word { Selection { anchor: s, head: e } } else { Selection::caret(p) };
                self.touch_range = Some(self.sel);
                self.selecting = word;
                cx.notify();
            }
            TouchPhase::Moved => {
                let p = layout.hit(&self.doc, layout.local(event.position));
                if self.selecting {
                    let anchor = self.touch_range.map_or(self.sel.anchor, |r| r.start());
                    let end = if p >= anchor { self.word_at(p).1 } else { self.word_at(p).0 };
                    self.sel = Selection { anchor, head: end.max(p.min(end)) };
                } else {
                    self.sel = Selection::caret(p);
                }
                self.touch_range = Some(self.sel);
                self.reveal = true;
                cx.notify();
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.selecting = false;
                let at = layout.caret(&self.doc, self.sel.start()).map_or(event.position, |c| layout.origin.get() + c.origin);
                self.open_menu(at, cx);
            }
        }
    }
    /// A finger on one of the selection's handles drags that end; whether
    /// this touch is the editor's (not a scroll).
    fn touch_drag(&mut self, event: &TouchDragEvent, cx: &mut Context<Self>) -> bool {
        let Some(layout) = self.layout.clone() else { return false };
        match event.phase {
            TouchPhase::Started => {
                if !self.touch_handles() {
                    return false;
                }
                let o = layout.origin.get();
                for (edge, at) in [(gpui_base::SelectionEdge::Start, self.sel.start()), (gpui_base::SelectionEdge::End, self.sel.end())] {
                    if let Some(c) = layout.caret(&self.doc, at)
                        && gpui_base::TouchHandle::hit_bounds(edge, Bounds::new(o + c.origin, c.size)).contains(&event.start_position)
                    {
                        self.handle_drag = Some(edge);
                        self.menu = None;
                        cx.notify();
                        return true;
                    }
                }
                false
            }
            TouchPhase::Moved => {
                let Some(edge) = self.handle_drag else { return false };
                // The finger is below the handle's knob; aim at the text above.
                let at = event.position - point(px(0.), px(16.));
                let p = layout.hit(&self.doc, layout.local(at));
                let (s, e) = (self.sel.start(), self.sel.end());
                self.sel = match edge {
                    gpui_base::SelectionEdge::Start => Selection { anchor: e, head: p.min(e) },
                    gpui_base::SelectionEdge::End => Selection { anchor: s, head: p.max(s) },
                };
                if self.sel.is_empty() {
                    self.sel = Selection { anchor: s, head: e };
                }
                self.touch_range = Some(self.sel);
                self.reveal = true;
                cx.notify();
                true
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.handle_drag.take().is_none() {
                    return false;
                }
                let at = layout.caret(&self.doc, self.sel.start()).map_or(event.position, |c| layout.origin.get() + c.origin);
                self.open_menu(at, cx);
                true
            }
        }
    }

    /// The word being typed (caret at its end), which spelling leaves be.
    fn typing_word(&self) -> Option<(usize, usize)> {
        if !self.spellcheck || !self.sel.is_empty() {
            return None;
        }
        let h = self.sel.head;
        let text = self.block_text(h.block);
        crate::manage::spelling::words(&text)
            .into_iter()
            .find(|r| r.end == h.offset)
            .map(|r| (h.block, r.start))
    }

    // The menu.

    fn replace_word(&mut self, start: Pos, end: Pos, with: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.begin(EditKind::Other);
        let style = self.doc.block(start.block).style_at(start.offset + 1);
        let at = self.doc.delete(start, end);
        let at = self.doc.insert_text(at, with, style);
        self.sel = Selection::caret(at);
        self.finish(EditKind::Other, Some(window), cx);
    }
    fn spelling_changed(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.transient += 1;
        cx.notify();
    }
    fn menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let look = self.look;
        let touch = self.touching(cx) || self.touch_range.is_some();
        let item = |id: SharedString, label: SharedString| {
            let selector = id.to_string();
            div()
                .id(id)
                .debug_selector(move || selector.clone())
                .px_3()
                .h(px(if touch { 40. } else { 26. }))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(if touch { 15. } else { 13. }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(look.soft)))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(label)
        };
        let mut list = div()
            .id("rich-editor-menu")
            .debug_selector(|| "rich-editor-menu".into())
            .min_w(px(180.))
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
                this.menu = None;
                cx.notify();
            }));
        if let Some((start, end, word)) = menu.word.clone() {
            if menu.suggestions.is_empty() {
                list = list.child(
                    div().px_3().py_1().text_size(px(12.)).text_color(rgb(look.muted)).child(t!("composer-no-spelling-suggestions")),
                );
            }
            for (i, s) in menu.suggestions.iter().enumerate() {
                let with = s.clone();
                list = list.child(
                    item(format!("spelling-{i}").into(), s.clone().into())
                        .font_weight(FontWeight::SEMIBOLD)
                        .on_click(cx.listener(move |this, _, w, cx| this.replace_word(start, end, &with, w, cx))),
                );
            }
            let learn = word.clone();
            list = list
                .child(item("spelling-learn".into(), t!("composer-learn-spelling").into()).on_click(cx.listener(move |this, _, _, cx| {
                    crate::manage::spelling::spelling().learn(&learn);
                    this.spelling_changed(cx);
                })))
                .child(item("spelling-ignore".into(), t!("composer-ignore-spelling").into()).on_click(cx.listener(move |this, _, _, cx| {
                    crate::manage::spelling::spelling().ignore(&word);
                    this.spelling_changed(cx);
                })))
                .child(div().h(px(1.)).my_1().bg(rgb(look.border)));
        }
        let has_selection = !self.sel.is_empty();
        if has_selection && !self.disabled {
            list = list.child(item("menu-cut".into(), t!("composer-cut").into()).on_click(cx.listener(|this, _, w, cx| {
                this.menu = None;
                this.cut(w, cx);
            })));
        }
        if has_selection {
            list = list.child(item("menu-copy".into(), t!("composer-copy").into()).on_click(cx.listener(|this, _, _, cx| {
                this.menu = None;
                this.copy(cx);
                cx.notify();
            })));
        }
        if !self.disabled {
            list = list.child(item("menu-paste".into(), t!("composer-paste").into()).on_click(cx.listener(|this, _, w, cx| {
                this.menu = None;
                // As ⌘V would: the composer's paste first (pictures, formatting).
                w.dispatch_action(Box::new(ia::Paste), cx);
                cx.notify();
            })));
        }
        list = list.child(item("menu-select-all".into(), t!("composer-select-all").into()).on_click(cx.listener(|this, _, _, cx| {
            this.menu = None;
            this.sel = Selection { anchor: Pos::default(), head: this.doc.end() };
            this.touch_range = Some(this.sel);
            cx.notify();
        })));
        // Above the selection for a finger, which would cover it; at the
        // pointer for a mouse.
        let at = if touch { menu.at - point(px(0.), px(if touch { 8. } else { 0. })) } else { menu.at };
        Some(
            deferred(
                anchored()
                    .position(at)
                    .anchor(if touch { Anchor::BottomLeft } else { Anchor::TopLeft })
                    .snap_to_window_with_margin(px(8.))
                    .child(list),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() && !self.disabled {
            self.copy(cx);
            self.begin(EditKind::Other);
            self.delete_selection();
            self.finish(EditKind::Other, Some(window), cx);
        }
    }

    // Pictures.

    /// The address a picture is decoded from: a draft file, or a `data:`.
    fn picture_key(&self, image: &rt::Image) -> Option<String> {
        if let Some(name) = image.file_name() {
            return self
                .files
                .iter()
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))
                .map(|p| p.to_string_lossy().into_owned());
        }
        image.src.starts_with("data:image/").then(|| image.src.clone())
    }
    fn load_picture(&mut self, key: String, cx: &mut Context<Self>) {
        if self.pictures.contains_key(&key) {
            return;
        }
        let size = if key.starts_with("data:") {
            None
        } else {
            image::image_dimensions(&key).ok().map(|(w, h)| (w as f32, h as f32))
        };
        self.pictures.insert(key.clone(), Picture::Loading(size));
        let source = key.clone();
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { decode_picture(&source) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pictures.insert(
                    key,
                    match decoded {
                        Some((image, size)) => Picture::Ready(Arc::new(image), size),
                        None => Picture::Failed,
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    // Positions as input methods see them: UTF-16 offsets in the text,
    // blocks a line apart.

    fn utf16_of(&self, p: Pos) -> usize {
        let mut n = 0;
        for i in 0..p.block.min(self.doc.blocks.len()) {
            n += self.block_text(i).encode_utf16().count() + 1;
        }
        let text = self.block_text(p.block.min(self.doc.blocks.len() - 1));
        n + text[..p.offset.min(text.len())].encode_utf16().count()
    }
    fn pos_of_utf16(&self, u: usize) -> Pos {
        let mut left = u;
        for i in 0..self.doc.blocks.len() {
            let text = self.block_text(i);
            let len = text.encode_utf16().count();
            if left <= len || i + 1 == self.doc.blocks.len() {
                let mut count = 0;
                for (byte, ch) in text.char_indices() {
                    if count >= left {
                        return Pos::new(i, byte);
                    }
                    count += ch.len_utf16();
                }
                return Pos::new(i, text.len());
            }
            left -= len + 1;
        }
        self.doc.end()
    }
    fn total_utf16(&self) -> usize {
        self.utf16_of(self.doc.end())
    }
    /// Somewhere near byte `offset` of the plain text (where a suggested
    /// reply written at the start ends).
    pub(super) fn put_caret_near(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let infos = self.doc.infos(16.);
        let mut left = offset;
        let mut target = self.doc.end();
        for (i, b) in self.doc.blocks.iter().enumerate() {
            let len = b.len();
            if left <= len {
                target = Pos::new(i, left);
                break;
            }
            left -= len + if infos[i].spaced { 2 } else { 1 }.min(left - len);
        }
        window.focus(&self.focus, cx);
        self.move_to(target, false, cx);
    }
    pub(super) fn put_caret_at_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.move_to(Pos::default(), false, cx);
    }
}

#[cfg(test)]
impl RichEditor {
    /// Each block's box, in the window (tests).
    pub(super) fn block_rects(&self) -> Vec<Bounds<Pixels>> {
        let Some(l) = &self.layout else { return vec![] };
        let o = l.origin.get();
        l.blocks.iter().map(|b| Bounds::from_corners(o + point(b.left, b.top), o + point(b.right, b.bottom))).collect()
    }
    /// Each picture's box, in the window (tests).
    pub(super) fn picture_rects(&self) -> Vec<Bounds<Pixels>> {
        let Some(l) = &self.layout else { return vec![] };
        let o = l.origin.get();
        l.blocks.iter().flat_map(|b| b.images().map(|i| Bounds::new(o + i.bounds.origin, i.bounds.size)).collect::<Vec<_>>()).collect()
    }
    /// Where the caret at `p` is drawn, in the window (tests).
    pub(super) fn caret_rect(&self, p: Pos) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let c = l.caret(&self.doc, p)?;
        Some(Bounds::new(l.origin.get() + c.origin, c.size))
    }
    pub(super) fn menu_open(&self) -> bool {
        self.menu.is_some()
    }
    pub(super) fn shared_html(&self) -> String {
        rhtml::to_html(&self.shared_fragment(self.doc.slice(self.sel.start(), self.sel.end())))
    }
}

fn set_mark(s: &mut rt::Style, mark: Mark, on: bool, info: &rt::Info) {
    match mark {
        // `None` where it is what the block has anyway.
        Mark::Bold => s.bold = (on != info.bold).then_some(on),
        Mark::Italic => s.italic = (on != info.italic).then_some(on),
        Mark::Underline => s.underline = on,
        Mark::Strike => s.strike = on,
        Mark::Code => s.code = on,
    }
}

/// Puts `html` (and `text`, for apps that take only text) on the system
/// clipboard; false where it can't (phones, tablets, tests), leaving the
/// text to GPUI's clipboard.
#[cfg(not(any(target_os = "android", target_os = "ios", test)))]
fn write_clipboard_html(html: &str, text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_html(html, Some(text))).is_ok()
}
#[cfg(any(target_os = "android", target_os = "ios", test))]
fn write_clipboard_html(_: &str, _: &str) -> bool {
    false
}

/// A picture decoded for drawing (BGRA, at most `PICTURE_MAX` a side), and
/// its size before that.
fn decode_picture(source: &str) -> Option<(RenderImage, (f32, f32))> {
    let bytes = if let Some(data) = source.strip_prefix("data:") {
        let (meta, payload) = data.split_once(',')?;
        if !meta.ends_with(";base64") {
            return None;
        }
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?
    } else {
        std::fs::read(source).ok()?
    };
    let image = image::load_from_memory(&bytes).ok()?;
    let size = (image.width() as f32, image.height() as f32);
    let image = if image.width() > PICTURE_MAX || image.height() > PICTURE_MAX {
        image.thumbnail(PICTURE_MAX, PICTURE_MAX)
    } else {
        image
    };
    let mut rgba = image.to_rgba8();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some((RenderImage::new(vec![image::Frame::new(rgba)]), size))
}

impl EntityInputHandler for RichEditor {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        actual: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (s, e) = (self.pos_of_utf16(range.start), self.pos_of_utf16(range.end));
        actual.replace(self.utf16_of(s)..self.utf16_of(e));
        let mut out = String::new();
        for i in s.block..=e.block {
            if i > s.block {
                out.push('\n');
            }
            let text = self.block_text(i);
            let from = if i == s.block { s.offset } else { 0 };
            let to = if i == e.block { e.offset } else { text.len() };
            out.push_str(text.get(from..to).unwrap_or(""));
        }
        Some(out)
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let (a, h) = (self.utf16_of(self.sel.anchor), self.utf16_of(self.sel.head));
        Some(UTF16Selection { range: a.min(h)..a.max(h), reversed: h < a })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.marked.map(|(s, e)| self.utf16_of(s)..self.utf16_of(e))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        if let Some(r) = range {
            self.sel = Selection { anchor: self.pos_of_utf16(r.start), head: self.pos_of_utf16(r.end) };
        } else if let Some((s, e)) = self.marked {
            self.sel = Selection { anchor: s, head: e };
        }
        self.marked = None;
        // A phone's keyboard sends its return key as a new line.
        if text == "\n" {
            return self.enter(&ia::Enter { secondary: false, shift: false }, window, cx);
        }
        self.insert_text(text, Some(window), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        if let Some(r) = range {
            self.sel = Selection { anchor: self.pos_of_utf16(r.start), head: self.pos_of_utf16(r.end) };
        } else if let Some((s, e)) = self.marked {
            self.sel = Selection { anchor: s, head: e };
        }
        let start = self.sel.start();
        self.begin(EditKind::Typing);
        let style = self.typing_style();
        let at = self.delete_selection();
        let text = text.replace('\n', " ");
        let end = self.doc.insert_text(at, &text, style.clone());
        self.marked = (!text.is_empty()).then_some((start, end));
        let base = self.utf16_of(start);
        let len = text.encode_utf16().count();
        self.sel = match selected {
            Some(r) => Selection {
                anchor: self.pos_of_utf16(base + r.start.min(len)),
                head: self.pos_of_utf16(base + r.end.min(len)),
            },
            None => Selection::caret(end),
        };
        self.pending = Some(style);
        self.finish(EditKind::Typing, Some(window), cx);
    }
    fn bounds_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let p = self.pos_of_utf16(range.start);
        let caret = layout.caret(&self.doc, p)?;
        Some(Bounds::new(layout.origin.get() + caret.origin, caret.size))
    }
    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let p = layout.hit(&self.doc, layout.local(point));
        Some(self.utf16_of(p))
    }
    fn set_selected_text_range(&mut self, range: std::ops::Range<usize>, _: &mut Window, cx: &mut Context<Self>) {
        self.sel = Selection { anchor: self.pos_of_utf16(range.start), head: self.pos_of_utf16(range.end) };
        self.pending = None;
        cx.notify();
    }
    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.total_utf16())
    }
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        !self.disabled
    }
    fn text_input_configuration(&mut self, _: &mut Window, _: &mut Context<Self>) -> TextInputConfiguration {
        TextInputConfiguration {
            autocorrect: true,
            autocapitalize: Autocapitalize::Sentences,
            suggestions: true,
            input_action: TextInputAction::Enter,
        }
    }
}

impl Render for RichEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("rich-editor")
            .debug_selector(|| "rich-editor".into())
            .key_context("Input")
            .track_focus(&self.focus)
            .role(Role::MultilineTextInput)
            .aria_label(self.placeholder.clone())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|t, _: &ia::Backspace, w, cx| t.delete_back(None, w, cx)))
            .on_action(cx.listener(|t, _: &ia::Delete, w, cx| t.delete_forward(None, w, cx)))
            .on_action(cx.listener(|t, _: &ia::DeleteToPreviousWordStart, w, cx| {
                let to = t.prev_word(t.sel.head);
                t.delete_back(Some(to), w, cx)
            }))
            .on_action(cx.listener(|t, _: &ia::DeleteToNextWordEnd, w, cx| {
                let to = t.next_word(t.sel.head);
                t.delete_forward(Some(to), w, cx)
            }))
            .on_action(cx.listener(|t, _: &ia::DeleteToBeginningOfLine, w, cx| {
                let h = t.sel.head;
                let to = t.layout.as_ref().and_then(|l| l.row_edge(&t.doc, h, false)).unwrap_or(Pos::new(h.block, 0));
                t.delete_back(Some(to), w, cx)
            }))
            .on_action(cx.listener(|t, _: &ia::DeleteToEndOfLine, w, cx| {
                let h = t.sel.head;
                let to = t
                    .layout
                    .as_ref()
                    .and_then(|l| l.row_edge(&t.doc, h, true))
                    .unwrap_or(Pos::new(h.block, t.doc.block(h.block).len()));
                t.delete_forward(Some(to), w, cx)
            }))
            .on_action(cx.listener(|t, a: &ia::Enter, w, cx| t.enter(a, w, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveLeft, _, cx| t.horizontal(false, false, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveRight, _, cx| t.horizontal(true, false, false, cx)))
            .on_action(cx.listener(|t, _: &SelectLeft, _, cx| t.horizontal(false, true, false, cx)))
            .on_action(cx.listener(|t, _: &SelectRight, _, cx| t.horizontal(true, true, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToPreviousWord, _, cx| t.horizontal(false, false, true, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToNextWord, _, cx| t.horizontal(true, false, true, cx)))
            .on_action(cx.listener(|t, _: &ia::SelectToPreviousWordStart, _, cx| t.horizontal(false, true, true, cx)))
            .on_action(cx.listener(|t, _: &ia::SelectToNextWordEnd, _, cx| t.horizontal(true, true, true, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveUp, _, cx| t.vertical(false, false, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveDown, _, cx| t.vertical(true, false, false, cx)))
            .on_action(cx.listener(|t, _: &SelectUp, _, cx| t.vertical(false, true, false, cx)))
            .on_action(cx.listener(|t, _: &SelectDown, _, cx| t.vertical(true, true, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MovePageUp, _, cx| t.vertical(false, false, true, cx)))
            .on_action(cx.listener(|t, _: &ia::MovePageDown, _, cx| t.vertical(true, false, true, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveHome, _, cx| t.line_edge(false, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveEnd, _, cx| t.line_edge(true, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToStartOfLine, _, cx| t.line_edge(false, false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToEndOfLine, _, cx| t.line_edge(true, false, cx)))
            .on_action(cx.listener(|t, _: &ia::SelectToStartOfLine, _, cx| t.line_edge(false, true, cx)))
            .on_action(cx.listener(|t, _: &ia::SelectToEndOfLine, _, cx| t.line_edge(true, true, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToStart, _, cx| t.move_to(Pos::default(), false, cx)))
            .on_action(cx.listener(|t, _: &ia::MoveToEnd, _, cx| {
                let end = t.doc.end();
                t.move_to(end, false, cx)
            }))
            .on_action(cx.listener(|t, _: &ia::SelectToStart, _, cx| t.move_to(Pos::default(), true, cx)))
            .on_action(cx.listener(|t, _: &ia::SelectToEnd, _, cx| {
                let end = t.doc.end();
                t.move_to(end, true, cx)
            }))
            .on_action(cx.listener(|t, _: &ia::SelectAll, _, cx| {
                t.sel = Selection { anchor: Pos::default(), head: t.doc.end() };
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &ia::IndentInline, w, cx| {
                let b = t.sel.head.block;
                if rt::innermost_list(&t.doc.block(b).path).is_some() {
                    t.begin(EditKind::Other);
                    let blocks = t.selected_blocks();
                    for i in blocks {
                        t.doc.indent_item(i);
                    }
                    t.finish(EditKind::Other, Some(w), cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|t, _: &ia::OutdentInline, w, cx| {
                let b = t.sel.head.block;
                if rt::innermost_list(&t.doc.block(b).path).is_some() {
                    t.begin(EditKind::Other);
                    let blocks = t.selected_blocks();
                    for i in blocks {
                        t.doc.outdent_item(i);
                    }
                    t.finish(EditKind::Other, Some(w), cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|t, _: &ia::Copy, _, cx| t.copy(cx)))
            .on_action(cx.listener(|t, _: &ia::Cut, w, cx| t.cut(w, cx)))
            .on_action(cx.listener(|t, _: &ia::Paste, w, cx| t.paste(w, cx)))
            .on_action(cx.listener(|t, _: &ia::Undo, w, cx| t.undo(Some(w), cx)))
            .on_action(cx.listener(|t, _: &ia::Redo, w, cx| t.redo(Some(w), cx)))
            .on_action(cx.listener(|_, _: &ia::ShowCharacterPalette, w, _| w.show_character_palette()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(
                div()
                    .px_2()
                    .py_2()
                    .w_full()
                    .child(RichText { editor: cx.entity() }),
            )
            .children(self.menu_view(cx))
    }
}

// Layout.

/// A laid-out document, positions relative to its top left.
struct Layout {
    /// Where it was painted, in the window.
    origin: std::cell::Cell<Point<Pixels>>,
    width: Pixels,
    height: Pixels,
    /// One per block.
    blocks: Vec<BlockBox>,
    /// Table cells, for their borders.
    cells: Vec<Bounds<Pixels>>,
    /// What it was laid out for (see `layout_key`).
    key: LayoutKey,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    revision: u64,
    pictures: usize,
    look: (u32, u32, bool, u32),
    /// A picture being resized, and spelling's state.
    transient: u64,
    /// The word being typed, which isn't checked yet.
    typing: Option<(usize, usize)>,
}

#[derive(Default)]
struct BlockBox {
    top: Pixels,
    bottom: Pixels,
    left: Pixels,
    right: Pixels,
    pieces: Vec<Piece>,
    marker: Option<(ShapedLine, Point<Pixels>, Pixels)>,
    /// Quote bars' x.
    bars: Vec<Pixels>,
    rule: Option<Bounds<Pixels>>,
    /// A `<pre>` block's background.
    shade: Option<Bounds<Pixels>>,
}

enum Piece {
    /// Text wrapped by GPUI.
    Line(LineBox),
    /// Text and pictures flowed together, a block with pictures.
    Flow(FlowBox),
}

struct ImageBox {
    offset: usize,
    bounds: Bounds<Pixels>,
    picture: Option<Arc<RenderImage>>,
    alt: SharedString,
}

struct LineBox {
    /// Its first byte in the block, and length.
    start: usize,
    len: usize,
    top: Pixels,
    left: Pixels,
    width: Pixels,
    line_height: Pixels,
    shaped: Option<WrappedLine>,
    rows: Vec<RowBox>,
    align: TextAlign,
}

struct RowBox {
    /// Byte offsets in the line.
    start: usize,
    end: usize,
    /// The unwrapped x of its start.
    x0: Pixels,
    /// Its shift for centered or right-aligned text.
    shift: Pixels,
}

impl LineBox {
    fn rows_count(&self) -> usize {
        self.rows.len().max(1)
    }
    fn height(&self) -> Pixels {
        self.line_height * self.rows_count() as f32
    }
    fn x_of(&self, local: usize) -> (usize, Pixels) {
        let Some(shaped) = &self.shaped else { return (0, self.left) };
        let row = self.rows.iter().rposition(|r| local >= r.start).unwrap_or(0);
        let r = &self.rows[row];
        (row, self.left + r.shift + shaped.unwrapped_layout.x_for_index(local) - r.x0)
    }
    fn index_at(&self, p: Point<Pixels>) -> usize {
        let Some(shaped) = &self.shaped else { return 0 };
        let row = (((p.y - self.top) / self.line_height).floor().max(0.) as usize).min(self.rows.len().saturating_sub(1));
        let r = &self.rows[row];
        let x = p.x - self.left - r.shift + r.x0;
        let i = shaped.unwrapped_layout.closest_index_for_x(x);
        // The start of the next row is that row's, not this one's end.
        let end = if row + 1 < self.rows.len() { r.end.saturating_sub(1).max(r.start) } else { r.end };
        i.clamp(r.start, end)
    }
}

struct FlowBox {
    rows: Vec<FlowRow>,
}

struct FlowRow {
    top: Pixels,
    height: Pixels,
    /// Where an empty row's caret goes.
    start: usize,
    left: Pixels,
    line_height: Pixels,
    items: Vec<FlowItem>,
}

enum FlowItem {
    Text(FlowText),
    Image(ImageBox),
}

/// A stretch of a shaped line in a flowed row.
struct FlowText {
    line: Rc<ShapedLine>,
    /// The block offset where the shaped line starts.
    base: usize,
    /// Byte offsets in the shaped line.
    start: usize,
    end: usize,
    x: Pixels,
    top: Pixels,
    height: Pixels,
}

impl FlowText {
    fn x_of(&self, local: usize) -> Pixels {
        self.x + self.line.x_for_index(local) - self.line.x_for_index(self.start)
    }
    fn index_at(&self, x: Pixels) -> usize {
        let i = self.line.closest_index_for_x(x - self.x + self.line.x_for_index(self.start));
        i.clamp(self.start, self.end)
    }
    fn width(&self) -> Pixels {
        self.x_of(self.end) - self.x
    }
}

impl FlowItem {
    fn range(&self) -> (usize, usize) {
        match self {
            FlowItem::Text(t) => (t.base + t.start, t.base + t.end),
            FlowItem::Image(i) => (i.offset, i.offset + rt::OBJECT.len_utf8()),
        }
    }
    fn left(&self) -> Pixels {
        match self {
            FlowItem::Text(t) => t.x,
            FlowItem::Image(i) => i.bounds.left(),
        }
    }
    fn right(&self) -> Pixels {
        match self {
            FlowItem::Text(t) => t.x_of(t.end),
            FlowItem::Image(i) => i.bounds.right(),
        }
    }
}

impl FlowRow {
    /// The caret at `offset` if it is in this row.
    fn caret(&self, offset: usize) -> Option<Bounds<Pixels>> {
        let mut found = None;
        if self.items.is_empty() && offset == self.start {
            found = Some(Bounds::new(point(self.left, self.top + self.height - self.line_height), size(px(1.5), self.line_height)));
        }
        for item in &self.items {
            match item {
                FlowItem::Text(t) => {
                    if offset >= t.base + t.start && offset <= t.base + t.end {
                        found = Some(Bounds::new(point(t.x_of(offset - t.base), t.top), size(px(1.5), t.height)));
                    }
                }
                FlowItem::Image(i) => {
                    if offset == i.offset {
                        found = Some(Bounds::new(point(i.bounds.left() - px(2.), i.bounds.top()), size(px(1.5), i.bounds.size.height)));
                    } else if offset == i.offset + rt::OBJECT.len_utf8() {
                        found = Some(Bounds::new(point(i.bounds.right() + px(1.), i.bounds.top()), size(px(1.5), i.bounds.size.height)));
                    }
                }
            }
        }
        found
    }
    fn index_at(&self, x: Pixels) -> usize {
        let Some(first) = self.items.first() else { return self.start };
        if x <= first.left() {
            return first.range().0;
        }
        for item in &self.items {
            if x >= item.left() && x <= item.right() {
                return match item {
                    FlowItem::Text(t) => t.base + t.index_at(x),
                    FlowItem::Image(i) => {
                        if x < i.bounds.center().x { i.offset } else { i.offset + rt::OBJECT.len_utf8() }
                    }
                };
            }
        }
        // Between items, or past the end.
        let mut best = self.items.last().map_or(self.start, |i| i.range().1);
        for pair in self.items.windows(2) {
            if x > pair[0].right() && x < pair[1].left() {
                best = if x - pair[0].right() < pair[1].left() - x { pair[0].range().1 } else { pair[1].range().0 };
            }
        }
        best
    }
}

impl BlockBox {
    fn contains_x(&self, x: Pixels) -> bool {
        x >= self.left && x <= self.right
    }
    fn images(&self) -> impl Iterator<Item = &ImageBox> {
        self.pieces.iter().flat_map(|p| match p {
            Piece::Flow(f) => f.rows.iter().flat_map(|r| r.items.iter()).filter_map(|i| match i {
                FlowItem::Image(i) => Some(i),
                _ => None,
            }).collect::<Vec<_>>(),
            Piece::Line(_) => vec![],
        })
    }
}

impl Layout {
    fn local(&self, p: Point<Pixels>) -> Point<Pixels> {
        p - self.origin.get()
    }
    /// The block at `p`, or the nearest.
    fn block_at(&self, p: Point<Pixels>) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let inside = |b: &BlockBox| p.y >= b.top && p.y < b.bottom;
        if let Some(i) = self.blocks.iter().position(|b| inside(b) && b.contains_x(p.x)) {
            return Some(i);
        }
        let distance = |b: &BlockBox| {
            let dx = (b.left - p.x).max(p.x - b.right).max(px(0.));
            let dy = (b.top - p.y).max(p.y - b.bottom).max(px(0.));
            f32::from(dy) * 4. + f32::from(dx)
        };
        // At the height of some blocks (beside them, a table's cells): the
        // nearest of those; else the nearest at all, the later in a gap.
        let row: Vec<usize> = (0..self.blocks.len()).filter(|&i| inside(&self.blocks[i])).collect();
        let candidates: Vec<usize> = if row.is_empty() { (0..self.blocks.len()).collect() } else { row };
        candidates
            .into_iter()
            .min_by(|&a, &b| {
                let (da, db) = (distance(&self.blocks[a]), distance(&self.blocks[b]));
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal).then(b.cmp(&a))
            })
    }
    /// The position nearest `p`.
    fn hit(&self, doc: &Doc, p: Point<Pixels>) -> Pos {
        let Some(bi) = self.block_at(p) else { return Pos::default() };
        let b = &self.blocks[bi];
        let offset = match b.pieces.iter().position(|piece| match piece {
            Piece::Line(l) => p.y < l.top + l.height(),
            Piece::Flow(f) => f.rows.last().is_some_and(|r| p.y < r.top + r.height),
        }).or(b.pieces.len().checked_sub(1)) {
            None => 0,
            Some(i) => match &b.pieces[i] {
                Piece::Line(l) => l.start + l.index_at(p),
                Piece::Flow(f) => {
                    let row = f.rows.iter().position(|r| p.y < r.top + r.height).unwrap_or(f.rows.len().saturating_sub(1));
                    f.rows.get(row).map_or(0, |r| r.index_at(p.x))
                }
            },
        };
        doc.clamp(Pos::new(bi.min(doc.blocks.len().saturating_sub(1)), offset))
    }
    fn image_at(&self, p: Point<Pixels>) -> Option<(Pos, Bounds<Pixels>)> {
        for (bi, b) in self.blocks.iter().enumerate() {
            if p.y < b.top || p.y > b.bottom {
                continue;
            }
            for i in b.images() {
                if i.bounds.contains(&p) {
                    return Some((Pos::new(bi, i.offset), i.bounds));
                }
            }
        }
        None
    }
    fn image_bounds(&self, at: Pos) -> Option<Bounds<Pixels>> {
        self.blocks.get(at.block)?.images().find(|i| i.offset == at.offset).map(|i| i.bounds)
    }
    /// The caret's box at `p`.
    fn caret(&self, _doc: &Doc, p: Pos) -> Option<Bounds<Pixels>> {
        let b = self.blocks.get(p.block)?;
        let mut last = None;
        for piece in &b.pieces {
            match piece {
                Piece::Line(l) => {
                    if p.offset >= l.start && p.offset <= l.start + l.len {
                        let (row, x) = l.x_of(p.offset - l.start);
                        return Some(Bounds::new(point(x, l.top + l.line_height * row as f32), size(px(1.5), l.line_height)));
                    }
                    last = Some(Bounds::new(point(l.left, l.top), size(px(1.5), l.line_height)));
                }
                Piece::Flow(f) => {
                    let found = f.rows.iter().filter_map(|r| r.caret(p.offset)).next_back();
                    if found.is_some() {
                        return found;
                    }
                }
            }
        }
        last.or(Some(Bounds::new(point(b.left, b.top), size(px(1.5), (b.bottom - b.top).max(px(12.))))))
    }
    /// The start or end of the visual row `p` is on.
    fn row_edge(&self, doc: &Doc, p: Pos, end: bool) -> Option<Pos> {
        let b = self.blocks.get(p.block)?;
        let text = doc.block(p.block).text();
        let before_space = |at: usize| {
            text[..at.min(text.len())].char_indices().next_back().map_or(at, |(i, c)| if c == ' ' { i } else { at })
        };
        for piece in &b.pieces {
            match piece {
                Piece::Line(l) if p.offset >= l.start && p.offset <= l.start + l.len => {
                    let local = p.offset - l.start;
                    if l.rows.is_empty() {
                        return Some(Pos::new(p.block, if end { l.start + l.len } else { l.start }));
                    }
                    let row = l.rows.iter().rposition(|r| local >= r.start).unwrap_or(0);
                    let r = &l.rows[row];
                    let last = row + 1 == l.rows.len();
                    let at = if !end {
                        l.start + r.start
                    } else if last {
                        l.start + r.end
                    } else {
                        before_space(l.start + r.end)
                    };
                    return Some(Pos::new(p.block, at));
                }
                Piece::Flow(f) => {
                    let Some(ri) = f.rows.iter().rposition(|r| r.caret(p.offset).is_some()) else { continue };
                    let r = &f.rows[ri];
                    let (first, last) = match (r.items.first(), r.items.last()) {
                        (Some(a), Some(z)) => (a.range().0, z.range().1),
                        _ => (r.start, r.start),
                    };
                    let at = if !end {
                        first
                    } else if ri + 1 == f.rows.len() {
                        last
                    } else {
                        before_space(last)
                    };
                    return Some(Pos::new(p.block, at));
                }
                _ => {}
            }
        }
        None
    }
    /// The block below (or above) block `bi` at `x`, for moving down (up).
    fn neighbour(&self, bi: usize, x: Pixels, down: bool) -> Option<usize> {
        let b = self.blocks.get(bi)?;
        let found = if down {
            (0..self.blocks.len())
                .filter(|&i| i != bi && self.blocks[i].top >= b.bottom - px(0.5) && self.blocks[i].contains_x(x))
                .min_by(|&a, &c| self.blocks[a].top.partial_cmp(&self.blocks[c].top).unwrap_or(std::cmp::Ordering::Equal))
        } else {
            (0..self.blocks.len())
                .filter(|&i| i != bi && self.blocks[i].bottom <= b.top + px(0.5) && self.blocks[i].contains_x(x))
                .max_by(|&a, &c| self.blocks[a].bottom.partial_cmp(&self.blocks[c].bottom).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&c)))
        };
        found.or(if down { (bi + 1 < self.blocks.len()).then_some(bi + 1) } else { bi.checked_sub(1) })
    }
}

/// The look a block's text has, before its runs' own.
struct BlockLook<'a> {
    info: &'a rt::Info,
    look: Look,
    family: SharedString,
    mono: SharedString,
}

fn hsla_of(c: rt::Color) -> Hsla {
    Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: c.a as f32 / 255. }.into()
}

/// A color another app chose, kept readable on what it is drawn on.
fn readable(color: Option<rt::Color>, on: rt::Color, fallback: Hsla) -> Hsla {
    let Some(c) = color else { return fallback };
    let (lc, lb) = (c.luminance(), on.luminance());
    let contrast = (lc.max(lb) + 0.05) / (lc.min(lb) + 0.05);
    if contrast >= 2.5 {
        return hsla_of(c);
    }
    // Too close to the background (black text in the dark): its hue at
    // the opposite lightness.
    let mut h: Hsla = hsla_of(c);
    h.l = if lb > 0.4 { 0.25 } else { 0.8 };
    h
}

fn rgb_of(c: u32) -> rt::Color {
    rt::Color::rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl BlockLook<'_> {
    fn run(&self, s: &rt::Style, len: usize, marked: bool, misspelled: bool) -> TextRun {
        let info = self.info;
        let bold = s.bold.unwrap_or(info.bold);
        let italic = s.italic.unwrap_or(info.italic);
        let family = if s.code || info.pre {
            self.mono.clone()
        } else {
            s.family
                .clone()
                .or_else(|| info.family.clone())
                .filter(|f| !is_generic(f))
                .map(SharedString::from)
                .unwrap_or_else(|| self.family.clone())
        };
        let mut f = font(family);
        f.weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
        f.style = if italic { FontStyle::Italic } else { FontStyle::Normal };
        let surface = rgb_of(self.look.surface);
        let background = s.background.filter(|b| b.a > 0);
        let on = background.map_or(surface, |b| if b.a < 128 { surface } else { b });
        let text: Hsla = rgb(self.look.text).into();
        let color = if s.link.is_some() {
            readable(s.color.or(Some(rgb_of(self.look.accent))), on, rgb(self.look.accent).into())
        } else {
            readable(s.color.or(info.color), on, text)
        };
        let background = if s.code {
            Some(rgb(self.look.soft).into())
        } else {
            background.map(|b| {
                let mut h = hsla_of(b);
                // Highlights read in the dark too.
                if self.look.dark && h.l > 0.6 {
                    h.a = h.a.min(0.45);
                }
                h
            })
        };
        let underline = if misspelled {
            Some(UnderlineStyle { color: Some(rgb(self.look.danger).into()), thickness: px(1.), wavy: true })
        } else {
            (s.underline || s.link.is_some() || info.underline || marked).then(|| UnderlineStyle {
                color: Some(color),
                thickness: px(1.),
                wavy: false,
            })
        };
        TextRun {
            len,
            font: f,
            color,
            background_color: background,
            underline,
            strikethrough: s.strike.then(|| StrikethroughStyle { color: Some(color), thickness: px(1.) }),
        }
    }
}

fn is_generic(family: &str) -> bool {
    matches!(
        family.to_ascii_lowercase().as_str(),
        "sans-serif" | "serif" | "system-ui" | "-apple-system" | "blinkmacsystemfont" | "inherit" | "initial" | "cursive" | "fantasy"
    )
}

fn mono_family() -> &'static str {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "monospace"
    }
}

/// A run's text as styled pieces (cut where the input method's text and
/// misspelled words start and end), by block offset.
type PiecesOf<'a> = dyn Fn(usize, &rt::Run) -> Vec<(std::ops::Range<usize>, TextRun)> + 'a;

/// What laying a document out needs and gathers.
struct Builder<'a> {
    e: &'a RichEditor,
    infos: Vec<rt::Info>,
    family: SharedString,
    mono: SharedString,
    boxes: Vec<BlockBox>,
    cells: Vec<Bounds<Pixels>>,
    /// Pictures not decoded yet.
    wanted: Vec<String>,
    typing: Option<(usize, usize)>,
}

/// Lays the editor's document out at `width`.
fn lay_out(e: &RichEditor, width: Pixels, family: SharedString, typing: Option<(usize, usize)>, window: &mut Window, wanted: &mut Vec<String>) -> Layout {
    let mut b = Builder {
        e,
        infos: e.doc.infos(e.look.size),
        family,
        mono: mono_family().into(),
        boxes: (0..e.doc.blocks.len()).map(|_| BlockBox::default()).collect(),
        cells: Vec::new(),
        wanted: Vec::new(),
        typing,
    };
    let height = b.range(0..e.doc.blocks.len(), 0, px(0.), width, px(0.), window);
    wanted.append(&mut b.wanted);
    Layout {
        origin: std::cell::Cell::new(Point::default()),
        width,
        height: height.max(px(20.)),
        blocks: b.boxes,
        cells: b.cells,
        key: layout_key(e, typing),
    }
}

fn layout_key(e: &RichEditor, typing: Option<(usize, usize)>) -> LayoutKey {
    LayoutKey {
        revision: e.revision,
        pictures: e.pictures.iter().filter(|(_, p)| !matches!(p, Picture::Loading(None))).count(),
        look: (e.look.text, e.look.accent, e.look.dark, e.look.size.to_bits()),
        transient: e.transient,
        typing,
    }
}

/// How a cell spans columns.
fn colspan(e: &rt::Element) -> usize {
    e.attr("colspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 20)
}

impl Builder<'_> {
    /// Lays blocks `range` out from `y`, in `left..left + width`, `depth`
    /// being where their paths start mattering (inside a cell); where they end.
    fn range(&mut self, range: std::ops::Range<usize>, depth: usize, left: Pixels, width: Pixels, mut y: Pixels, window: &mut Window) -> Pixels {
        let blocks = &self.e.doc.blocks;
        let mut i = range.start;
        while i < range.end {
            let path = &blocks[i].path;
            if let Some(k) = (depth..path.len()).find(|&k| path[k].is("table")) {
                let table = path[k].clone();
                let mut j = i;
                while j < range.end && blocks[j].path.get(k).is_some_and(|e| Rc::ptr_eq(e, &table)) {
                    j += 1;
                }
                y = self.table(i..j, k, left, width, y, window);
                i = j;
            } else {
                let last = i + 1 == range.end;
                y = self.block(i, depth, left, width, y, i == range.start, last, window);
                i += 1;
            }
        }
        y
    }

    /// A table: its rows one below another, each row's cells beside each
    /// other in equal columns (spans kept), each as tall as the row's tallest.
    fn table(&mut self, range: std::ops::Range<usize>, k: usize, left: Pixels, width: Pixels, y: Pixels, window: &mut Window) -> Pixels {
        struct Cell {
            range: std::ops::Range<usize>,
            depth: usize,
            span: usize,
            key: Option<El>,
        }
        struct Row {
            key: Option<El>,
            cells: Vec<Cell>,
        }
        type El = rt::El;
        let blocks = &self.e.doc.blocks;
        let mut rows: Vec<Row> = Vec::new();
        for i in range.clone() {
            let path = &blocks[i].path;
            let r = (k + 1..path.len()).find(|&r| path[r].is("tr"));
            let c = r.and_then(|r| (r + 1..path.len()).find(|&c| matches!(path[c].tag.as_str(), "td" | "th")));
            let (row_key, cell_key, depth, span) = match (r, c) {
                (Some(r), Some(c)) => (Some(path[r].clone()), Some(path[c].clone()), c + 1, colspan(&path[c])),
                // A caption, or what isn't in a cell: across the table.
                _ => (None, None, k + 1, usize::MAX),
            };
            let same_row = rows.last().is_some_and(|row| match (&row.key, &row_key) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            });
            if !same_row {
                rows.push(Row { key: row_key, cells: Vec::new() });
            }
            let row = rows.last_mut().expect("a row");
            let same_cell = row.cells.last().is_some_and(|cell| match (&cell.key, &cell_key) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            });
            if same_cell {
                row.cells.last_mut().expect("a cell").range.end = i + 1;
            } else {
                row.cells.push(Cell { range: i..i + 1, depth, span, key: cell_key });
            }
        }
        let columns = rows
            .iter()
            .map(|r| r.cells.iter().map(|c| if c.span == usize::MAX { 1 } else { c.span }).sum::<usize>())
            .max()
            .unwrap_or(1)
            .max(1);
        let column = width / columns as f32;
        let pad = px(6.);
        let mut y = y + px(2.);
        for row in rows {
            let top = y;
            let mut bottom = top + px(8.);
            let mut col = 0;
            let mut rects = Vec::new();
            for cell in row.cells {
                let span = if cell.span == usize::MAX { columns } else { cell.span.min(columns - col.min(columns - 1)) };
                let x = left + column * col as f32;
                let w = column * span as f32;
                let inner = (w - pad * 2.).max(px(16.));
                let end = self.range(cell.range, cell.depth, x + pad, inner, top + pad, window) + pad;
                bottom = bottom.max(end);
                rects.push((x, w));
                col = (col + span).min(columns);
            }
            for (x, w) in rects {
                self.cells.push(Bounds::new(point(x, top), size(w, bottom - top)));
            }
            y = bottom;
        }
        y + px(6.)
    }

    /// One block at `y`; where it ends (with the space after it).
    #[allow(clippy::too_many_arguments)]
    fn block(&mut self, bi: usize, depth: usize, left: Pixels, width: Pixels, mut y: Pixels, first: bool, last: bool, window: &mut Window) -> Pixels {
        let e = self.e;
        let look = e.look;
        let b = &e.doc.blocks[bi];
        let info = self.infos[bi].clone();
        let font_px = px(info.size.clamp(8., 64.));
        let line_height = (font_px * 1.45).round();
        if info.heading.is_some() && !first {
            y += font_px * 0.3;
        }
        // Only what is inside the cell indents within it.
        let inner = &b.path[depth.min(b.path.len())..];
        let quotes = inner.iter().filter(|e| e.is("blockquote")).count();
        let lists = inner.iter().filter(|e| matches!(e.tag.as_str(), "ul" | "ol" | "dir" | "menu")).count();
        let quote_indent = px(14.) * quotes as f32;
        let left_in = left + quote_indent + px(24.) * lists as f32 + if info.pre { px(8.) } else { px(0.) };
        let avail = (width - (left_in - left) - if info.pre { px(8.) } else { px(0.) }).max(px(24.));
        let top = y;
        if info.pre {
            y += px(6.);
        }
        let blook = BlockLook { info: &info, look, family: self.family.clone(), mono: self.mono.clone() };
        let align = match info.align {
            rt::Align::Center => TextAlign::Center,
            rt::Align::Right => TextAlign::Right,
            _ => TextAlign::Left,
        };
        // Where each run's text is cut: the input method's text, and the
        // words spelled wrong.
        let text = b.text();
        let marked = e.marked.filter(|(s, _)| s.block == bi).map(|(s, z)| s.offset..z.offset);
        let misspelled = self.misspelled(bi, &text);
        let pieces_of = |offset: usize, run: &rt::Run| -> Vec<(std::ops::Range<usize>, TextRun)> {
            let end = offset + run.text.len();
            let mut cuts = vec![offset, end];
            for r in marked.iter().chain(misspelled.iter()) {
                for m in [r.start, r.end] {
                    if m > offset && m < end {
                        cuts.push(m);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            cuts.windows(2)
                .map(|w| {
                    let in_marked = marked.as_ref().is_some_and(|m| w[0] >= m.start && w[0] < m.end);
                    let wrong = misspelled.iter().any(|m| w[0] >= m.start && w[0] < m.end);
                    (w[0]..w[1], blook.run(&run.style, w[1] - w[0], in_marked, wrong))
                })
                .collect()
        };
        let mut pieces = Vec::new();
        let mut rule = None;
        if info.rule {
            rule = Some(Bounds::new(point(left_in, y + px(8.)), size(avail, px(1.))));
            y += px(17.);
        } else if b.runs.iter().any(|r| r.image.is_some()) {
            let (flow, height) = self.flow(bi, &pieces_of, font_px, line_height, left_in, avail, align, y, window);
            y += height;
            pieces.push(Piece::Flow(flow));
        } else if b.runs.is_empty() {
            pieces.push(Piece::Line(LineBox { start: 0, len: 0, top: y, left: left_in, width: avail, line_height, shaped: None, rows: vec![], align }));
            y += line_height;
        } else {
            let mut at = 0;
            let mut runs = Vec::new();
            for run in &b.runs {
                runs.extend(pieces_of(at, run).into_iter().map(|(_, r)| r));
                at += run.text.len();
            }
            let shaped = window.text_system().shape_text(text.clone().into(), font_px, &runs, Some(avail), None).unwrap_or_default();
            let mut line_start = 0;
            for line in shaped {
                let len = line.len();
                let rows = rows_of(&line, avail, align);
                let lb = LineBox { start: line_start, len, top: y, left: left_in, width: avail, line_height, shaped: Some(line), rows, align };
                y += lb.height();
                line_start += len + 1;
                pieces.push(Piece::Line(lb));
            }
        }
        if info.pre {
            y += px(6.);
        }
        let bottom = y;
        let marker = info.marker.as_ref().map(|m| {
            let label: SharedString = match m {
                Marker::Bullet(depth) => ["•", "◦", "▪"][(depth.saturating_sub(1)) % 3].into(),
                Marker::Number(n) => format!("{n}.").into(),
            };
            let run = blook.run(&rt::Style::default(), label.len(), false, false);
            let shaped = window.text_system().shape_line(label, font_px, &[run], None);
            let x = left_in - px(6.) - shaped.width;
            (shaped, point(x, top), line_height)
        });
        let bars = (0..quotes).map(|q| left + px(14.) * q as f32 + px(2.)).collect();
        let shade = info.pre.then(|| Bounds::new(point(left_in - px(8.), top), size(avail + px(16.), bottom - top)));
        // Space after paragraphs (not after the last line).
        if info.spaced && !last {
            y += (font_px * 0.6).round();
        }
        self.boxes[bi] = BlockBox { top, bottom, left, right: left + width, pieces, marker, bars, rule, shade };
        y
    }

    /// The words of block `bi` to underline as misspelled.
    fn misspelled(&self, bi: usize, text: &str) -> Vec<std::ops::Range<usize>> {
        let e = self.e;
        if !e.spellcheck || !crate::manage::spelling::spelling().available() {
            return vec![];
        }
        let info = &self.infos[bi];
        // Quoted mail and code are someone else's words.
        if info.pre || info.quotes > 0 {
            return vec![];
        }
        let b = e.doc.block(bi);
        let spelling = crate::manage::spelling::spelling();
        crate::manage::spelling::words(text)
            .into_iter()
            .filter(|r| self.typing != Some((bi, r.start)))
            .filter(|r| {
                let style = b.style_at(r.start + 1);
                style.link.is_none() && !style.code && spelling.misspelled(&text[r.clone()])
            })
            .collect()
    }

    /// Text and pictures of block `bi` flowed into rows: words wrap at
    /// spaces, a picture sits on the text's baseline and goes to the next
    /// row when it doesn't fit; `\n` ends a row.
    #[allow(clippy::too_many_arguments)]
    fn flow(
        &mut self,
        bi: usize,
        pieces_of: &PiecesOf,
        font_px: Pixels,
        line_height: Pixels,
        left: Pixels,
        avail: Pixels,
        align: TextAlign,
        top: Pixels,
        window: &mut Window,
    ) -> (FlowBox, Pixels) {
        enum Tok {
            /// A shaped line, from `start` to `end`; `fit` is where its
            /// width counts to (before trailing spaces).
            Word { line: Rc<ShapedLine>, base: usize, start: usize, end: usize, fit: usize },
            Image(ImageBox),
            Break(usize),
        }
        let e = self.e;
        let b = &e.doc.blocks[bi];
        // Text runs as styled pieces, by block offset.
        let mut toks: Vec<Tok> = Vec::new();
        let mut segment: Vec<(std::ops::Range<usize>, TextRun)> = Vec::new();
        let text = b.text();
        let flush_segment = |segment: &mut Vec<(std::ops::Range<usize>, TextRun)>, toks: &mut Vec<Tok>, window: &mut Window| {
            let (Some(first), Some(last)) = (segment.first(), segment.last()) else { return };
            let (start, end) = (first.0.start, last.0.end);
            // Each line of it (between `\n`s) shaped on its own.
            let mut line_start = start;
            let seg_text = &text[start..end];
            let mut lines: Vec<(usize, usize)> = Vec::new();
            for (i, ch) in seg_text.char_indices() {
                if ch == '\n' {
                    lines.push((line_start, start + i));
                    line_start = start + i + 1;
                }
            }
            lines.push((line_start, end));
            for (n, &(ls, le)) in lines.iter().enumerate() {
                if n > 0 {
                    toks.push(Tok::Break(ls));
                }
                if ls == le {
                    continue;
                }
                let runs: Vec<TextRun> = segment
                    .iter()
                    .filter_map(|(r, run)| {
                        let (a, z) = (r.start.max(ls), r.end.min(le));
                        (a < z).then(|| TextRun { len: z - a, ..run.clone() })
                    })
                    .collect();
                let s: SharedString = text[ls..le].to_owned().into();
                let line = Rc::new(window.text_system().shape_line(s.clone(), font_px, &runs, None));
                // Words with the spaces after them; ideographs one by one.
                let mut ws = 0;
                let chars: Vec<(usize, char)> = s.char_indices().collect();
                let mut k = 0;
                while k < chars.len() {
                    let (i, c) = chars[k];
                    let wide = is_wide(c);
                    let next = chars.get(k + 1).map(|&(_, n)| n);
                    let boundary = match next {
                        None => true,
                        Some(n) => (c.is_whitespace() && !n.is_whitespace()) || wide || is_wide(n),
                    };
                    if boundary {
                        let end = i + c.len_utf8();
                        let fit = s[ws..end].trim_end().len() + ws;
                        toks.push(Tok::Word { line: line.clone(), base: ls, start: ws, end, fit });
                        ws = end;
                    }
                    k += 1;
                }
            }
            segment.clear();
        };
        let mut at = 0;
        for run in &b.runs {
            if let Some(image) = &run.image {
                flush_segment(&mut segment, &mut toks, window);
                let (w, h, picture) = self.picture_size(bi, at, image, avail);
                toks.push(Tok::Image(ImageBox { offset: at, bounds: Bounds::new(Point::default(), size(w, h)), picture, alt: image.alt.clone().into() }));
            } else {
                segment.extend(pieces_of(at, run));
            }
            at += run.text.len();
        }
        flush_segment(&mut segment, &mut toks, window);

        // Rows, before alignment and heights.
        struct RowBuild {
            start: usize,
            items: Vec<FlowItem>,
            x: Pixels,
            /// The width without trailing spaces.
            used: Pixels,
        }
        let mut rows: Vec<RowBuild> = vec![RowBuild { start: 0, items: vec![], x: px(0.), used: px(0.) }];
        let new_row = |rows: &mut Vec<RowBuild>, start: usize| rows.push(RowBuild { start, items: vec![], x: px(0.), used: px(0.) });
        for tok in toks {
            match tok {
                Tok::Break(start) => new_row(&mut rows, start),
                Tok::Image(mut image) => {
                    let w = image.bounds.size.width;
                    let row = rows.last().expect("a row");
                    if row.x > px(0.) && row.x + w > avail {
                        new_row(&mut rows, image.offset);
                    }
                    let row = rows.last_mut().expect("a row");
                    image.bounds.origin.x = row.x;
                    row.x += w + px(2.);
                    row.used = row.x;
                    row.items.push(FlowItem::Image(image));
                }
                Tok::Word { line, base, mut start, end, fit } => {
                    loop {
                        let width_to = |i: usize| line.x_for_index(i) - line.x_for_index(start);
                        let row = rows.last().expect("a row");
                        if row.x > px(0.) && row.x + width_to(fit.max(start)) > avail {
                            new_row(&mut rows, base + start);
                            continue;
                        }
                        // Too long for a row of its own: as much as fits.
                        let mut stop = end;
                        if row.x == px(0.) && width_to(fit.max(start)) > avail {
                            let target = line.x_for_index(start) + avail;
                            let mut i = line.closest_index_for_x(target).clamp(start, end);
                            while i > start && line.x_for_index(i) > target {
                                i = line.text[..i].char_indices().next_back().map_or(start, |(j, _)| j);
                            }
                            if i <= start {
                                i = start + line.text[start..].chars().next().map_or(1, char::len_utf8);
                            }
                            stop = i.min(end);
                        }
                        let row = rows.last_mut().expect("a row");
                        let x = row.x;
                        let joined = matches!(row.items.last(), Some(FlowItem::Text(t)) if Rc::ptr_eq(&t.line, &line) && t.end == start);
                        if joined {
                            if let Some(FlowItem::Text(t)) = row.items.last_mut() {
                                t.end = stop;
                            }
                        } else {
                            row.items.push(FlowItem::Text(FlowText { line: line.clone(), base, start, end: stop, x, top: px(0.), height: line_height }));
                        }
                        row.x = x + (line.x_for_index(stop) - line.x_for_index(start));
                        row.used = x + (line.x_for_index(fit.clamp(start, stop)) - line.x_for_index(start));
                        if stop >= end {
                            break;
                        }
                        start = stop;
                        new_row(&mut rows, base + start);
                    }
                }
            }
        }
        // Heights, baselines and alignment.
        let descent = (line_height - font_px) / 2. + font_px * 0.2;
        let mut y = top;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let tallest = row
                .items
                .iter()
                .filter_map(|i| match i {
                    FlowItem::Image(i) => Some(i.bounds.size.height),
                    _ => None,
                })
                .fold(px(0.), |a, h| a.max(h));
            let height = line_height.max(tallest + descent + px(2.));
            let shift = match align {
                TextAlign::Center => ((avail - row.used) / 2.).max(px(0.)),
                TextAlign::Right => (avail - row.used).max(px(0.)),
                _ => px(0.),
            };
            let mut items = row.items;
            for item in &mut items {
                match item {
                    FlowItem::Text(t) => {
                        t.x += left + shift;
                        t.top = y + height - line_height;
                    }
                    FlowItem::Image(i) => {
                        i.bounds.origin.x += left + shift;
                        i.bounds.origin.y = y + height - descent - i.bounds.size.height;
                    }
                }
            }
            out.push(FlowRow { top: y, height, start: row.start, left: left + shift, line_height, items });
            y += height;
        }
        (FlowBox { rows: out }, y - top)
    }

    /// A picture's size on screen (its attributes, else its own size, at
    /// most the text's width; the size it is being dragged to), and its
    /// pixels once decoded.
    fn picture_size(&mut self, bi: usize, offset: usize, image: &rt::Image, avail: Pixels) -> (Pixels, Pixels, Option<Arc<RenderImage>>) {
        let e = self.e;
        let key = e.picture_key(image);
        let (picture, natural) = match key.as_ref().and_then(|k| e.pictures.get(k)) {
            Some(Picture::Ready(img, size)) => (Some(img.clone()), Some(*size)),
            Some(Picture::Loading(size)) => (None, *size),
            Some(Picture::Failed) => (None, None),
            None => {
                if let Some(k) = &key {
                    self.wanted.push(k.clone());
                }
                (None, None)
            }
        };
        let (mut w, mut h) = match (image.width, image.height, natural) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some((nw, nh))) => (w, w * nh / nw.max(1.)),
            (None, Some(h), Some((nw, nh))) => (h * nw / nh.max(1.), h),
            (None, None, Some((nw, nh))) => (nw, nh),
            (Some(w), None, None) => (w, w * 0.6),
            (None, Some(h), None) => (h * 1.6, h),
            (None, None, None) => (160., 100.),
        };
        if let Some(r) = &e.resize
            && r.at == Pos::new(bi, offset)
        {
            h = r.width * h / w.max(1.);
            w = r.width;
        }
        let max = f32::from(avail);
        if w > max {
            h *= max / w;
            w = max;
        }
        (px(w.max(4.)), px(h.max(4.)), picture)
    }
}

/// Characters a line may break around (ideographs, kana, hangul).
fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

/// The rows a wrapped line is shown in.
fn rows_of(line: &WrappedLine, width: Pixels, align: TextAlign) -> Vec<RowBox> {
    let layout = &line.unwrapped_layout;
    let mut starts = vec![0usize];
    for b in line.wrap_boundaries.iter() {
        if let Some(g) = layout.runs.get(b.run_ix).and_then(|r| r.glyphs.get(b.glyph_ix)) {
            starts.push(g.index);
        }
    }
    let len = line.len();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(len);
            let x0 = layout.x_for_index(start);
            let row_width = layout.x_for_index(end) - x0;
            let shift = match align {
                TextAlign::Center => ((width - row_width) / 2.).max(px(0.)),
                TextAlign::Right => (width - row_width).max(px(0.)),
                _ => px(0.),
            };
            RowBox { start, end, x0, shift }
        })
        .collect()
}

/// The element that lays out and paints a rich editor's document.
struct RichText {
    editor: Entity<RichEditor>,
}

impl IntoElement for RichText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// The size of the box a selected picture is resized by.
const GRIP: f32 = 10.;

impl Element for RichText {
    type RequestLayoutState = ();
    type PrepaintState = Option<(Rc<Layout>, Hitbox, Option<Hitbox>)>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let editor = self.editor.clone();
        let family = window.text_style().font_family.clone();
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(w) => w,
                _ => px(600.),
            });
            let layout = cached_layout(&editor, width, family.clone(), window, cx);
            size(width, layout.height)
        });
        (id, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(Rc<Layout>, Hitbox, Option<Hitbox>)> {
        let family = window.text_style().font_family.clone();
        let layout = cached_layout(&self.editor, bounds.size.width, family, window, cx);
        layout.origin.set(bounds.origin);
        // Bring the caret into view after it moved.
        let (reveal, head) = {
            let e = self.editor.read(cx);
            (e.reveal, e.sel.head)
        };
        if reveal {
            let doc = self.editor.read(cx).doc.clone();
            if let Some(caret) = layout.caret(&doc, head) {
                let scroll = self.editor.read(cx).scroll.clone();
                let view = scroll.bounds();
                let caret = Bounds::new(bounds.origin + caret.origin, caret.size);
                let mut offset = scroll.offset();
                let margin = px(8.);
                if view.size.height > px(0.) {
                    if caret.bottom() + margin > view.bottom() {
                        offset.y -= caret.bottom() + margin - view.bottom();
                    } else if caret.top() - margin < view.top() {
                        offset.y += view.top() - (caret.top() - margin);
                    }
                    offset.y = offset.y.min(px(0.));
                    if offset != scroll.offset() {
                        scroll.set_offset(offset);
                        window.refresh();
                    }
                }
            }
            self.editor.update(cx, |e, _| e.reveal = false);
        }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // A selected picture's grip shows the resize pointer.
        let grip = self.editor.read(cx).selected_picture().and_then(|at| layout.image_bounds(at)).map(|r| {
            let corner = bounds.origin + r.bottom_right();
            window.insert_hitbox(Bounds::new(corner - point(px(GRIP), px(GRIP)), size(px(GRIP * 2.), px(GRIP * 2.))), HitboxBehavior::Normal)
        });
        Some((layout, hitbox, grip))
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Option<(Rc<Layout>, Hitbox, Option<Hitbox>)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((layout, hitbox, grip_hitbox)) = state.clone() else { return };
        let (focus, sel, look, blank, placeholder, doc, handles, resizable) = {
            let e = self.editor.read(cx);
            (
                e.focus.clone(),
                e.sel,
                e.look,
                e.doc.blocks.len() == 1 && e.doc.block(0).is_empty(),
                e.placeholder.clone(),
                e.doc.clone(),
                e.touch_handles(),
                e.selected_picture(),
            )
        };
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        self.listen_for_touch(&hitbox, window);
        let o = bounds.origin;
        let focused = focus.is_focused(window);
        let selection: Hsla = {
            let mut h: Hsla = rgb(look.accent).into();
            h.a = if focused { 0.28 } else { 0.16 };
            h
        };
        let view = window.content_mask().bounds;
        for cell in &layout.cells {
            let r = Bounds::new(o + cell.origin, cell.size);
            if r.bottom() < view.top() || r.top() > view.bottom() {
                continue;
            }
            window.paint_quad(PaintQuad {
                bounds: r,
                corner_radii: Corners::default(),
                background: transparent_black().into(),
                border_widths: Edges::all(px(1.)),
                border_color: rgb(look.border).into(),
                border_style: BorderStyle::default(),
            });
        }
        let (s, e) = (sel.start(), sel.end());
        for (bi, b) in layout.blocks.iter().enumerate() {
            // Only what is in view is painted.
            if o.y + b.bottom < view.top() - px(40.) || o.y + b.top > view.bottom() + px(40.) {
                continue;
            }
            if let Some(shade) = b.shade {
                window.paint_quad(fill(Bounds::new(o + shade.origin, shade.size), rgb(look.soft)).corner_radii(px(6.)));
            }
            for x in &b.bars {
                let bar = Bounds::new(point(o.x + *x, o.y + b.top), size(px(3.), b.bottom - b.top));
                window.paint_quad(fill(bar, rgb(look.border)).corner_radii(px(1.5)));
            }
            if let Some(rule) = b.rule {
                window.paint_quad(fill(Bounds::new(o + rule.origin, rule.size), rgb(look.border)));
            }
            let selected = !sel.is_empty() && bi >= s.block && bi <= e.block;
            let from = if bi == s.block { s.offset } else { 0 };
            let to = if bi == e.block { e.offset } else { usize::MAX };
            let through = bi < e.block;
            for piece in &b.pieces {
                match piece {
                    Piece::Line(l) => {
                        if selected {
                            paint_line_selection(l, from, to, through, o, selection, window);
                        }
                        if let Some(shaped) = &l.shaped {
                            let origin = point(o.x + l.left, o.y + l.top);
                            let area = Bounds::new(origin, size(l.width, l.height()));
                            let _ = shaped.paint_background(origin, l.line_height, l.align, Some(area), window, cx);
                            let _ = shaped.paint(origin, l.line_height, l.align, Some(area), window, cx);
                        }
                    }
                    Piece::Flow(f) => {
                        for (ri, row) in f.rows.iter().enumerate() {
                            if o.y + row.top > view.bottom() || o.y + row.top + row.height < view.top() {
                                continue;
                            }
                            if selected {
                                let last_row = ri + 1 == f.rows.len();
                                for item in &row.items {
                                    if let FlowItem::Text(t) = item {
                                        let (a, z) = ((from.max(t.base + t.start)), to.min(t.base + t.end));
                                        if a < z || (a == z && through && last_row && to == usize::MAX) {
                                            let x1 = t.x_of(a - t.base);
                                            let x2 = t.x_of(z - t.base);
                                            let r = Bounds::from_corners(point(o.x + x1, o.y + t.top), point(o.x + x2, o.y + t.top + t.height));
                                            window.paint_quad(fill(r, selection));
                                        }
                                    }
                                }
                                if row.items.is_empty() && from <= row.start && to > row.start {
                                    let r = Bounds::new(point(o.x + row.left, o.y + row.top), size(px(6.), row.height));
                                    window.paint_quad(fill(r, selection));
                                }
                            }
                            for item in &row.items {
                                match item {
                                    FlowItem::Text(t) => {
                                        let clip = Bounds::new(point(o.x + t.x, o.y + t.top - px(2.)), size(t.width() + px(1.), t.height + px(4.)));
                                        let origin = point(o.x + t.x - t.line.x_for_index(t.start), o.y + t.top);
                                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                                            let _ = t.line.paint_background(origin, t.height, TextAlign::Left, None, window, cx);
                                            let _ = t.line.paint(origin, t.height, TextAlign::Left, None, window, cx);
                                        });
                                    }
                                    FlowItem::Image(i) => {
                                        let p = Pos::new(bi, i.offset);
                                        paint_picture(i, o, look, window, cx);
                                        // A selected picture is tinted.
                                        if !sel.is_empty() && sel.start() <= p && sel.end() > p {
                                            let r = Bounds::new(o + i.bounds.origin, i.bounds.size);
                                            window.paint_quad(fill(r, selection).corner_radii(px(4.)));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some((marker, at, line_height)) = &b.marker {
                let _ = marker.paint(o + *at, *line_height, TextAlign::Left, None, window, cx);
            }
        }
        if blank && !placeholder.is_empty() {
            let run = TextRun {
                len: placeholder.len(),
                font: font(window.text_style().font_family.clone()),
                color: rgb(look.muted).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(placeholder, px(look.size), &[run], None);
            let left = layout.blocks.first().and_then(|b| b.pieces.first()).map_or(px(0.), |p| match p {
                Piece::Line(l) => l.left,
                Piece::Flow(_) => px(0.),
            });
            let _ = line.paint(point(o.x + left, o.y), (px(look.size) * 1.45).round(), TextAlign::Left, None, window, cx);
        }
        if focused
            && sel.is_empty()
            && let Some(caret) = layout.caret(&doc, sel.head)
        {
            window.paint_quad(fill(Bounds::new(o + caret.origin, caret.size), rgb(look.accent)));
        }
        // A selected picture's grip, to resize it.
        if let Some(at) = resizable
            && let Some(r) = layout.image_bounds(at)
        {
            let r = Bounds::new(o + r.origin, r.size);
            window.paint_quad(PaintQuad {
                bounds: r,
                corner_radii: Corners::all(px(4.)),
                background: transparent_black().into(),
                border_widths: Edges::all(px(1.5)),
                border_color: rgb(look.accent).into(),
                border_style: BorderStyle::default(),
            });
            let grip = Bounds::new(r.bottom_right() - point(px(GRIP / 2.), px(GRIP / 2.)), size(px(GRIP), px(GRIP)));
            window.paint_quad(fill(grip, rgb(look.accent)).corner_radii(px(2.)).border_widths(px(1.5)).border_color(rgb(look.surface)));
            if let Some(h) = &grip_hitbox {
                window.set_cursor_style(CursorStyle::ResizeUpLeftDownRight, h);
            }
        }
        // A touch selection's handles.
        if handles {
            let color: Hsla = rgb(look.accent).into();
            for (edge, at) in [(gpui_base::SelectionEdge::Start, sel.start()), (gpui_base::SelectionEdge::End, sel.end())] {
                if let Some(caret) = layout.caret(&doc, at) {
                    gpui_base::TouchHandle::paint(edge, Bounds::new(o + caret.origin, caret.size), color, window);
                }
            }
        }
        self.editor.update(cx, |e, _| e.layout = Some(layout.clone()));
    }
}

impl RichText {
    /// Touch: a long press selects (the word, then what the finger sweeps
    /// over), and a selection's handles drag its ends. Both are claimed,
    /// so they don't scroll.
    fn listen_for_touch(&self, hitbox: &Hitbox, window: &mut Window) {
        // A selection or a grip being dragged follows the pointer anywhere
        // in the window (past the editor's edge, it scrolls).
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() && editor.read(cx).dragging() {
                editor.update(cx, |e, cx| e.on_mouse_move(event, window, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
            if phase.capture() {
                if event.phase == TouchPhase::Started {
                    editor.update(cx, |e, _| e.last_touch = Some(std::time::Instant::now()));
                }
                return;
            }
            let claimed = editor.update(cx, |e, cx| e.touch_drag(event, cx));
            if claimed {
                window.prevent_default();
                cx.stop_propagation();
            }
        });
        let editor = self.editor.clone();
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            if event.phase == TouchPhase::Started {
                if window.default_prevented() || !hitbox.is_hovered(window) {
                    return;
                }
                editor.update(cx, |e, cx| e.long_press(event, window, cx));
                window.capture_long_press(&editor);
            } else if window.has_long_press_capture(&editor) {
                editor.update(cx, |e, cx| e.long_press(event, window, cx));
            } else {
                return;
            }
            window.prevent_default();
            cx.stop_propagation();
        });
    }
}

fn paint_line_selection(l: &LineBox, from: usize, to: usize, through: bool, o: Point<Pixels>, color: Hsla, window: &mut Window) {
    let (ls, le) = (l.start, l.start + l.len);
    let (a, z) = (from.max(ls), to.min(le));
    let through = to > le && through;
    if a > z || (a == z && !through) {
        return;
    }
    if l.rows.is_empty() {
        let r = Bounds::new(point(o.x + l.left, o.y + l.top), size(px(6.), l.line_height));
        window.paint_quad(fill(r, color));
        return;
    }
    for (ri, row) in l.rows.iter().enumerate() {
        let (ra, rz) = ((a - ls).max(row.start), (z - ls).min(row.end));
        if ra > rz || (ra == rz && !(through && ri + 1 == l.rows.len())) {
            continue;
        }
        let (_, x1) = l.x_of(ra);
        let x2 = if rz == row.end && ri + 1 < l.rows.len() {
            l.left + row.shift + l.shaped.as_ref().map_or(px(0.), |s| s.unwrapped_layout.x_for_index(row.end)) - row.x0
        } else {
            l.x_of(rz).1
        };
        let extra = if through && ri + 1 == l.rows.len() { px(6.) } else { px(0.) };
        let top = o.y + l.top + l.line_height * ri as f32;
        let r = Bounds::from_corners(point(o.x + x1, top), point(o.x + x2 + extra, top + l.line_height));
        window.paint_quad(fill(r, color));
    }
}

fn paint_picture(i: &ImageBox, o: Point<Pixels>, look: Look, window: &mut Window, cx: &mut App) {
    let r = Bounds::new(o + i.bounds.origin, i.bounds.size);
    match &i.picture {
        Some(picture) => {
            let _ = window.paint_image(r, r, Corners::all(px(4.)), picture.clone(), 0, false);
        }
        None => {
            window.paint_quad(fill(r, rgb(look.soft)).corner_radii(px(4.)).border_widths(px(1.)).border_color(rgb(look.border)));
            if !i.alt.is_empty() {
                let run = TextRun {
                    len: i.alt.len(),
                    font: font(window.text_style().font_family.clone()),
                    color: rgb(look.muted).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(i.alt.clone(), px(12.), &[run], None);
                let at = point(r.left() + px(8.), r.top() + px(6.));
                window.with_content_mask(Some(ContentMask { bounds: r }), |window| {
                    let _ = line.paint(at, px(16.), TextAlign::Left, None, window, cx);
                });
            }
        }
    }
}

/// The editor's layout at `width`, laid out again only when the document,
/// its pictures, its look or the width changed.
fn cached_layout(editor: &Entity<RichEditor>, width: Pixels, family: SharedString, window: &mut Window, cx: &mut App) -> Rc<Layout> {
    let e = editor.read(cx);
    let typing = e.typing_word();
    if let Some(l) = &e.layout
        && l.key == layout_key(e, typing)
        && l.width == width
        && e.marked.is_none()
    {
        return l.clone();
    }
    let mut wanted = Vec::new();
    let layout = Rc::new(lay_out(e, width, family, typing, window, &mut wanted));
    editor.update(cx, |e, cx| {
        e.layout = Some(layout.clone());
        for key in wanted {
            e.load_picture(key, cx);
        }
    });
    layout
}
