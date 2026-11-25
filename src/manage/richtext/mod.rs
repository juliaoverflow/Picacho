//! The formatted composer's document: what it edits, read from and
//! written as HTML (`html`).
//!
//! A document is a list of blocks (paragraphs, headings, list items, quoted
//! lines, table cells…), each holding runs of styled text and pictures.
//! Every block keeps the path of HTML elements it sits in, outermost first,
//! ending with its own element; blocks that share an element share the
//! same `Rc`, so writing them out puts them back in one element. That is
//! what keeps formatting the editor doesn't know (tables, classes, styles
//! of other mail apps) through an edit: it stays on the elements and on
//! the runs (`Style::wrappers`) and is written back as it was.
//!
//! Positions are byte offsets in a block's text, where each picture is one
//! U+FFFC character and a line break inside a block (`<br>`) is `\n`.
pub mod css;
pub mod html;
#[cfg(test)]
mod tests;

use std::rc::Rc;

pub use css::Color;

/// A picture's character in a block's text.
pub const OBJECT: char = '\u{FFFC}';
const OBJECT_STR: &str = "\u{FFFC}";

/// An HTML element that holds blocks, or a block's own element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// Lowercase.
    pub tag: String,
    pub attrs: Vec<(String, String)>,
}
pub type El = Rc<Element>;

impl Element {
    pub fn new(tag: &str) -> El {
        Rc::new(Element { tag: tag.to_owned(), attrs: Vec::new() })
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    /// The same element with another tag, as a new element.
    pub fn renamed(&self, tag: &str) -> El {
        Rc::new(Element { tag: tag.to_owned(), attrs: self.attrs.clone() })
    }
    /// A new element like this one: a block split in two gets one.
    pub fn copy(&self) -> El {
        Rc::new(self.clone())
    }
    pub fn style(&self) -> Vec<(String, String)> {
        self.attr("style").map(css::declarations).unwrap_or_default()
    }
    /// A copy with style property `name` set to `value`, or removed.
    pub fn with_style(&self, name: &str, value: Option<&str>) -> El {
        let mut style = self.style();
        style.retain(|(n, _)| n != name);
        if let Some(v) = value {
            style.push((name.to_owned(), v.to_owned()));
        }
        let mut e = self.clone();
        e.attrs.retain(|(n, _)| n != "style");
        if !style.is_empty() {
            e.attrs.push(("style".into(), css::style_attr(&style)));
        }
        // `align` says the same as text-align, which now decides.
        if name == "text-align" {
            e.attrs.retain(|(n, _)| n != "align");
        }
        Rc::new(e)
    }
    pub fn is(&self, tag: &str) -> bool {
        self.tag == tag
    }
    pub fn is_list(&self) -> bool {
        matches!(self.tag.as_str(), "ul" | "ol")
    }
    pub fn heading_level(&self) -> Option<u8> {
        match self.tag.as_str() {
            "h1" => Some(1),
            "h2" => Some(2),
            "h3" => Some(3),
            "h4" => Some(4),
            "h5" => Some(5),
            "h6" => Some(6),
            _ => None,
        }
    }
    /// A block element whose tag says what kind of text it holds and
    /// nothing else: a list item or heading can take its place.
    fn is_plain_block(&self) -> bool {
        matches!(
            self.tag.as_str(),
            "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "address" | "center"
        )
    }
}

/// How a run of text looks. `None` for bold and italic means as the block
/// has it (headings are bold); `Some(false)` turns it off inside.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub sup: bool,
    pub sub: bool,
    pub link: Option<String>,
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub family: Option<String>,
    /// Pixels.
    pub size: Option<f32>,
    /// Inline elements kept as they came (another app's `<span class>`),
    /// outermost first, with what the fields above say taken out.
    pub wrappers: Vec<El>,
}

impl Style {
    pub fn plain() -> Rc<Style> {
        thread_local! {
            static PLAIN: Rc<Style> = Rc::new(Style::default());
        }
        PLAIN.with(Rc::clone)
    }
    /// What typing on from here continues with: not a link (typing after
    /// one doesn't lengthen it).
    pub fn continued(&self) -> Style {
        Style { link: None, ..self.clone() }
    }
}

/// A picture in the text.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// `file:<attachment name>` for the draft's own pictures, or the
    /// address another app gave (`https:`, `data:`, an unresolved `cid:`).
    pub src: String,
    pub alt: String,
    /// CSS pixels, from its attributes or style.
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Its other attributes, kept (title, style, class).
    pub attrs: Vec<(String, String)>,
}

impl Image {
    /// The attachment a `file:` picture shows.
    pub fn file_name(&self) -> Option<&str> {
        self.src.strip_prefix("file:")
    }
}

#[derive(Clone, Debug)]
pub struct Run {
    /// One OBJECT for a picture.
    pub text: String,
    pub style: Rc<Style>,
    pub image: Option<Rc<Image>>,
}

impl Run {
    pub fn text(text: impl Into<String>, style: Rc<Style>) -> Run {
        Run { text: text.into(), style, image: None }
    }
    pub fn image(image: Image, style: Rc<Style>) -> Run {
        Run { text: OBJECT_STR.into(), style, image: Some(Rc::new(image)) }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    /// Never empty: the containing elements and, last, its own.
    pub path: Vec<El>,
    pub runs: Vec<Run>,
}

impl Block {
    pub fn paragraph(tag: &str, text: &str) -> Block {
        let runs = if text.is_empty() { vec![] } else { vec![Run::text(text, Style::plain())] };
        Block { path: vec![Element::new(tag)], runs }
    }
    pub fn element(&self) -> &El {
        self.path.last().expect("a block has its element")
    }
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
    pub fn len(&self) -> usize {
        self.runs.iter().map(|r| r.text.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
    /// A horizontal rule, which holds no text.
    pub fn is_rule(&self) -> bool {
        self.element().is("hr")
    }
    pub fn is_pre(&self) -> bool {
        self.path.iter().any(|e| e.is("pre"))
    }
    /// Drops empty runs and joins neighbours that look the same.
    pub fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            if run.text.is_empty() {
                continue;
            }
            if let Some(last) = out.last_mut()
                && last.image.is_none()
                && run.image.is_none()
                && (Rc::ptr_eq(&last.style, &run.style) || last.style == run.style)
            {
                last.text.push_str(&run.text);
                continue;
            }
            out.push(run);
        }
        self.runs = out;
    }
    /// Splits runs so one starts at `offset`; the index of that run (or
    /// the number of runs at the end).
    fn split_at(&mut self, offset: usize) -> usize {
        let mut at = 0;
        for i in 0..self.runs.len() {
            let len = self.runs[i].text.len();
            if offset == at {
                return i;
            }
            if offset < at + len {
                let run = &mut self.runs[i];
                // A picture is never split.
                if run.image.is_some() {
                    return i;
                }
                let local = floor_char(&run.text, offset - at);
                let tail = run.text.split_off(local);
                let next = Run { text: tail, style: run.style.clone(), image: None };
                self.runs.insert(i + 1, next);
                return i + 1;
            }
            at += len;
        }
        self.runs.len()
    }
    /// The runs after `offset`, taken out.
    fn take_tail(&mut self, offset: usize) -> Vec<Run> {
        let i = self.split_at(offset);
        self.runs.split_off(i)
    }
    /// The style of the text just before `offset` (or after, at the start).
    pub fn style_at(&self, offset: usize) -> Rc<Style> {
        let mut at = 0;
        let mut last = None;
        for run in &self.runs {
            if offset > at && offset <= at + run.text.len() {
                return run.style.clone();
            }
            if last.is_none() && offset == at {
                last = Some(run.style.clone());
            }
            at += run.text.len();
        }
        last.unwrap_or_else(Style::plain)
    }
    pub fn images(&self) -> impl Iterator<Item = (usize, &Rc<Image>)> {
        let mut at = 0;
        self.runs.iter().filter_map(move |r| {
            let start = at;
            at += r.text.len();
            r.image.as_ref().map(|i| (start, i))
        })
    }
}

/// The largest char boundary of `s` at or before `i`.
fn floor_char(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub block: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(block: usize, offset: usize) -> Pos {
        Pos { block, offset }
    }
}

/// How a block is drawn, from its path.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub heading: Option<u8>,
    pub pre: bool,
    pub rule: bool,
    /// Blockquotes it is in.
    pub quotes: usize,
    /// Lists it is in.
    pub lists: usize,
    /// Its list item's bullet or number, on the item's first block.
    pub marker: Option<Marker>,
    pub align: Align,
    /// Space after it: paragraphs and headings have some, `div`s none.
    pub spaced: bool,
    /// Pixels.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<Color>,
    pub family: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    /// At a list depth from 1.
    Bullet(usize),
    Number(i64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    fn parse(v: &str) -> Option<Align> {
        match v.trim().to_ascii_lowercase().as_str() {
            "left" | "start" => Some(Align::Left),
            "center" | "-webkit-center" | "middle" => Some(Align::Center),
            "right" | "end" => Some(Align::Right),
            "justify" => Some(Align::Justify),
            _ => None,
        }
    }
    pub fn css(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "justify",
        }
    }
}

/// A list kind for `Doc::toggle_list`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullets,
    Numbers,
}
impl ListKind {
    fn tag(self) -> &'static str {
        match self {
            ListKind::Bullets => "ul",
            ListKind::Numbers => "ol",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub blocks: Vec<Rc<Block>>,
    /// Style sheets another app's message came with, kept as they were.
    pub sheets: Vec<String>,
}

impl Doc {
    /// One empty line to write on.
    pub fn empty() -> Doc {
        Doc { blocks: vec![Rc::new(Block::paragraph("div", ""))], sheets: vec![] }
    }
    /// Plain text, a block per line.
    pub fn from_text(text: &str) -> Doc {
        let blocks: Vec<Rc<Block>> = text
            .split('\n')
            .map(|line| Rc::new(Block::paragraph("div", line.trim_end_matches('\r'))))
            .collect();
        let mut doc = Doc { blocks, sheets: vec![] };
        doc.ensure_block();
        doc
    }
    pub fn ensure_block(&mut self) {
        if self.blocks.is_empty() {
            self.blocks.push(Rc::new(Block::paragraph("div", "")));
        }
    }
    pub fn block(&self, i: usize) -> &Block {
        &self.blocks[i]
    }
    pub fn block_mut(&mut self, i: usize) -> &mut Block {
        Rc::make_mut(&mut self.blocks[i])
    }
    pub fn end(&self) -> Pos {
        let last = self.blocks.len().saturating_sub(1);
        Pos::new(last, self.blocks.get(last).map_or(0, |b| b.len()))
    }
    /// `pos` moved onto the document and a char boundary.
    pub fn clamp(&self, pos: Pos) -> Pos {
        let block = pos.block.min(self.blocks.len().saturating_sub(1));
        let Some(b) = self.blocks.get(block) else {
            return Pos::default();
        };
        let text = b.text();
        Pos::new(block, floor_char(&text, pos.offset))
    }

    // Editing.

    /// Inserts `text` (which may hold `\n` line breaks) at `pos` in
    /// `style`; where it ends.
    pub fn insert_text(&mut self, pos: Pos, text: &str, style: Rc<Style>) -> Pos {
        if text.is_empty() {
            return pos;
        }
        let pos = self.clamp(pos);
        // Nothing is written in a rule: a new line after it takes it.
        if self.blocks[pos.block].is_rule() {
            let mut path = self.blocks[pos.block].path.clone();
            path.pop();
            path.push(Element::new("div"));
            self.blocks.insert(pos.block + 1, Rc::new(Block { path, runs: vec![] }));
            return self.insert_text(Pos::new(pos.block + 1, 0), text, style);
        }
        let b = self.block_mut(pos.block);
        let i = b.split_at(pos.offset);
        b.runs.insert(i, Run::text(text, style));
        b.normalize();
        Pos::new(pos.block, pos.offset + text.len())
    }

    pub fn insert_image(&mut self, pos: Pos, image: Image, style: Rc<Style>) -> Pos {
        let pos = self.clamp(pos);
        if self.blocks[pos.block].is_rule() {
            let after = self.insert_text(pos, " ", style.clone());
            let start = Pos::new(after.block, 0);
            self.delete(start, after);
            return self.insert_image(start, image, style);
        }
        let b = self.block_mut(pos.block);
        let i = b.split_at(pos.offset);
        b.runs.insert(i, Run::image(image, Rc::new(style.continued())));
        b.normalize();
        Pos::new(pos.block, pos.offset + OBJECT.len_utf8())
    }

    /// Removes what is between `start` and `end` (in either order),
    /// joining their blocks; where they met.
    pub fn delete(&mut self, start: Pos, end: Pos) -> Pos {
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        if start == end {
            return start;
        }
        if start.block == end.block {
            let b = self.block_mut(start.block);
            let i = b.split_at(start.offset);
            let j = b.split_at(end.offset);
            b.runs.drain(i..j);
            b.normalize();
            return start;
        }
        let tail = self.block_mut(end.block).take_tail(end.offset);
        // A whole rule (or an empty first block) gives way to what follows.
        let first = &self.blocks[start.block];
        let replace = start.offset == 0 && (first.is_rule() || first.is_empty());
        if replace {
            let path = self.blocks[end.block].path.clone();
            self.blocks.drain(start.block..end.block);
            let b = self.block_mut(start.block);
            b.path = path;
            b.runs = tail;
            b.normalize();
            return start;
        }
        let b = self.block_mut(start.block);
        b.take_tail(start.offset);
        b.runs.extend(tail);
        b.normalize();
        self.blocks.drain(start.block + 1..=end.block);
        start
    }

    /// Splits the block at `pos` (Enter); the start of the new block.
    pub fn split(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let b = &self.blocks[pos.block];
        let at_end = pos.offset >= b.len();
        let element = b.element().clone();
        let mut path = b.path.clone();
        path.pop();
        // A heading, or a rule, goes on with an ordinary line.
        if (element.heading_level().is_some() && at_end) || element.is("hr") {
            path.push(Element::new(if element.is("hr") { "div" } else { "p" }));
        } else {
            path.push(element.copy());
        }
        let tail = if element.is("hr") { vec![] } else { self.block_mut(pos.block).take_tail(pos.offset) };
        let mut block = Block { path, runs: tail };
        block.normalize();
        self.block_mut(pos.block).normalize();
        // An empty line keeps the look the text had there.
        self.blocks.insert(pos.block + 1, Rc::new(block));
        Pos::new(pos.block + 1, 0)
    }

    /// Inserts `fragment`'s blocks at `pos`: its first block's text joins
    /// the line at `pos`, its last the text after; where it ends.
    pub fn insert_doc(&mut self, pos: Pos, fragment: &Doc) -> Pos {
        let pos = self.clamp(pos);
        let Some(first) = fragment.blocks.first() else {
            return pos;
        };
        for sheet in &fragment.sheets {
            if !self.sheets.contains(sheet) {
                self.sheets.push(sheet.clone());
            }
        }
        if fragment.blocks.len() == 1 && !first.is_rule() {
            let b = self.block_mut(pos.block);
            let i = b.split_at(pos.offset);
            let len: usize = first.runs.iter().map(|r| r.text.len()).sum();
            let runs: Vec<Run> = first.runs.clone();
            b.runs.splice(i..i, runs);
            b.normalize();
            return Pos::new(pos.block, pos.offset + len);
        }
        let target_empty = self.blocks[pos.block].is_empty();
        let after = self.split(pos);
        // The line it went in: its start keeps the line's place.
        let mut at = pos.block;
        if target_empty {
            self.blocks[at] = first.clone();
        } else {
            let b = self.block_mut(at);
            b.runs.extend(first.runs.iter().cloned());
            b.normalize();
        }
        let middle = &fragment.blocks[1..fragment.blocks.len() - 1];
        for block in middle {
            at += 1;
            self.blocks.insert(at, block.clone());
        }
        let last = fragment.blocks.last().expect("more than one block");
        let tail_block = after.block + middle.len();
        let tail_empty = self.blocks[tail_block].is_empty();
        let len: usize = last.runs.iter().map(|r| r.text.len()).sum();
        if tail_empty || last.is_rule() {
            // Nothing followed on that line: the last block comes whole.
            if tail_empty {
                self.blocks[tail_block] = last.clone();
            } else {
                self.blocks.insert(tail_block, last.clone());
                return Pos::new(tail_block + 1, 0);
            }
        } else {
            let b = self.block_mut(tail_block);
            b.runs.splice(0..0, last.runs.iter().cloned());
            b.normalize();
        }
        Pos::new(tail_block, len)
    }

    /// A copy of what is between `start` and `end`.
    pub fn slice(&self, start: Pos, end: Pos) -> Doc {
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        let mut blocks = Vec::new();
        for (i, block) in self.blocks.iter().enumerate().take(end.block + 1).skip(start.block) {
            let mut b: Block = (**block).clone();
            if i == end.block {
                b.take_tail(end.offset);
            }
            if i == start.block {
                let tail = b.take_tail(start.offset);
                b.runs = tail;
            }
            b.normalize();
            blocks.push(Rc::new(b));
        }
        Doc { blocks, sheets: self.sheets.clone() }
    }

    /// Changes the style of the text between `start` and `end`.
    pub fn restyle(&mut self, start: Pos, end: Pos, change: &dyn Fn(&mut Style)) {
        let (start, end) = (self.clamp(start.min(end)), self.clamp(start.max(end)));
        for bi in start.block..=end.block {
            let b = self.block_mut(bi);
            let from = if bi == start.block { start.offset } else { 0 };
            let to = if bi == end.block { end.offset } else { b.len() };
            if from >= to {
                continue;
            }
            let i = b.split_at(from);
            let j = b.split_at(to);
            for run in &mut b.runs[i..j] {
                let mut style = (*run.style).clone();
                change(&mut style);
                run.style = Rc::new(style);
            }
            b.normalize();
        }
    }

    /// Gives the picture at `pos` a size in CSS pixels (dragged to it):
    /// its attributes say it, and any size its style said goes.
    pub fn set_image_size(&mut self, pos: Pos, width: f32, height: f32) -> bool {
        let Some(b) = self.blocks.get(pos.block) else { return false };
        let mut at = 0;
        let mut index = None;
        for (i, run) in b.runs.iter().enumerate() {
            if at == pos.offset && run.image.is_some() {
                index = Some(i);
                break;
            }
            at += run.text.len();
        }
        let Some(i) = index else { return false };
        let b = self.block_mut(pos.block);
        let image = b.runs[i].image.as_ref().expect("a picture");
        let mut new = (**image).clone();
        new.width = Some(width.round().max(1.));
        new.height = Some(height.round().max(1.));
        for (name, value) in &mut new.attrs {
            if name == "style" {
                let kept: Vec<(String, String)> = css::declarations(value)
                    .into_iter()
                    .filter(|(n, _)| !matches!(n.as_str(), "width" | "height" | "max-width" | "max-height" | "min-width" | "min-height"))
                    .collect();
                *value = css::style_attr(&kept);
            }
        }
        new.attrs.retain(|(n, v)| n != "style" || !v.is_empty());
        b.runs[i].image = Some(Rc::new(new));
        true
    }

    /// The styles of the text between `start` and `end` (pictures left out).
    pub fn styles_in(&self, start: Pos, end: Pos) -> Vec<(usize, Rc<Style>)> {
        let (start, end) = (start.min(end), start.max(end));
        let mut out = Vec::new();
        for (bi, b) in self.blocks.iter().enumerate().take(end.block + 1).skip(start.block) {
            let from = if bi == start.block { start.offset } else { 0 };
            let to = if bi == end.block { end.offset } else { b.len() };
            let mut at = 0;
            for run in &b.runs {
                let (s, e) = (at, at + run.text.len());
                at = e;
                if e <= from || s >= to || run.image.is_some() {
                    continue;
                }
                out.push((bi, run.style.clone()));
            }
        }
        out
    }

    /// The text between `start` and `end`, a line per block, pictures as
    /// their descriptions.
    pub fn text_between(&self, start: Pos, end: Pos) -> String {
        let slice = self.slice(start, end);
        slice
            .blocks
            .iter()
            .map(|b| inline_text(b))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // Blocks.

    /// Makes blocks `range` list items of `kind`, or ordinary lines again
    /// when they all are already.
    pub fn toggle_list(&mut self, range: std::ops::RangeInclusive<usize>, kind: ListKind) {
        let lists: Vec<Option<usize>> = range.clone().map(|i| innermost_list(&self.blocks[i].path)).collect();
        let all_this_kind = lists.iter().zip(range.clone()).all(|(l, i)| {
            l.is_some_and(|k| self.blocks[i].path[k].is(kind.tag()))
        });
        if all_this_kind {
            for (l, i) in lists.into_iter().zip(range) {
                let k = l.expect("in a list");
                let b = self.block_mut(i);
                let item = b.path[k + 1].clone();
                let rest: Vec<El> = b.path[k + 2..].to_vec();
                b.path.truncate(k);
                if rest.is_empty() {
                    b.path.push(item.renamed("div"));
                } else {
                    b.path.extend(rest);
                }
            }
            return;
        }
        // Lists of the other kind change kind, sharing one new list each.
        let mut renamed: Vec<(El, El)> = Vec::new();
        // New lists, one per run of blocks in the same container.
        let mut current: Option<(Vec<El>, El)> = None;
        // The list just before can take the new items.
        let first = *range.start();
        if first > 0
            && lists.first().is_some_and(|l| l.is_none())
        {
            let before = &self.blocks[first - 1];
            let container = container_of(&self.blocks[first].path);
            if let Some(k) = innermost_list(&before.path)
                && before.path[k].is(kind.tag())
                && k == container.len()
                && before.path[..k].iter().zip(&container).all(|(a, b)| Rc::ptr_eq(a, b))
            {
                current = Some((container, before.path[k].clone()));
            }
        }
        for (l, i) in lists.into_iter().zip(range) {
            let b = self.block_mut(i);
            match l {
                Some(k) => {
                    let old = b.path[k].clone();
                    let new = match renamed.iter().find(|(o, _)| Rc::ptr_eq(o, &old)) {
                        Some((_, n)) => n.clone(),
                        None => {
                            let n = old.renamed(kind.tag());
                            renamed.push((old, n.clone()));
                            n
                        }
                    };
                    b.path[k] = new;
                    current = None;
                }
                None => {
                    let container = container_of(&b.path);
                    let list = match &current {
                        Some((c, list))
                            if c.len() == container.len()
                                && c.iter().zip(&container).all(|(a, b)| Rc::ptr_eq(a, b)) =>
                        {
                            list.clone()
                        }
                        _ => {
                            let list = Element::new(kind.tag());
                            current = Some((container.clone(), list.clone()));
                            list
                        }
                    };
                    let own = b.element().clone();
                    let item = if own.is_plain_block() && own.heading_level().is_none() {
                        own.renamed("li")
                    } else {
                        Element::new("li")
                    };
                    b.path = container;
                    b.path.push(list);
                    b.path.push(item);
                    // A heading stays one, inside the item.
                    if own.heading_level().is_some() {
                        b.path.push(own);
                    }
                }
            }
        }
    }

    /// Quotes blocks `range`, or unquotes them when all are quoted.
    pub fn toggle_quote(&mut self, range: std::ops::RangeInclusive<usize>) {
        let all = range.clone().all(|i| self.blocks[i].path.iter().any(|e| e.is("blockquote")));
        if all {
            for i in range {
                let b = self.block_mut(i);
                let k = b.path.iter().rposition(|e| e.is("blockquote")).expect("quoted");
                if k == b.path.len() - 1 {
                    let own = b.path[k].renamed("div");
                    b.path[k] = own;
                } else {
                    b.path.remove(k);
                }
            }
            return;
        }
        // One quote around them all, inside what they all share (not
        // between a list and its items, or a table and its rows).
        let paths: Vec<&Vec<El>> = range.clone().map(|i| &self.blocks[i].path).collect();
        let mut common = paths
            .iter()
            .map(|p| p.len() - 1)
            .min()
            .unwrap_or(0);
        for p in &paths[1..] {
            let shared = paths[0].iter().zip(p.iter()).take_while(|(a, b)| Rc::ptr_eq(a, b)).count();
            common = common.min(shared);
        }
        while common > 0
            && matches!(
                paths[0][common - 1].tag.as_str(),
                "ul" | "ol" | "table" | "tbody" | "thead" | "tfoot" | "tr" | "dl"
            )
        {
            common -= 1;
        }
        let quote = Element::new("blockquote");
        for i in range {
            self.block_mut(i).path.insert(common, quote.clone());
        }
    }

    /// Makes blocks `range` headings, or ordinary lines when all are.
    pub fn toggle_heading(&mut self, range: std::ops::RangeInclusive<usize>, level: u8) {
        let all = range.clone().all(|i| self.blocks[i].element().heading_level().is_some());
        let tag = format!("h{}", level.clamp(1, 6));
        for i in range {
            let b = self.block_mut(i);
            let own = b.element().clone();
            let last = b.path.len() - 1;
            if all {
                b.path[last] = own.renamed("p");
            } else if own.is_plain_block() {
                b.path[last] = own.renamed(&tag);
            } else if !own.is("hr") {
                b.path.push(Element::new(&tag));
            }
        }
    }

    pub fn set_align(&mut self, range: std::ops::RangeInclusive<usize>, align: Align) {
        for i in range {
            let b = self.block_mut(i);
            let last = b.path.len() - 1;
            let value = (align != Align::Left).then_some(align.css());
            b.path[last] = b.path[last].with_style("text-align", value);
        }
    }

    /// Moves list item `i` into the item before it (Tab); false where it
    /// can't (not an item, or the first of its list).
    pub fn indent_item(&mut self, i: usize) -> bool {
        if i == 0 {
            return false;
        }
        let Some(k) = innermost_list(&self.blocks[i].path) else {
            return false;
        };
        let list = self.blocks[i].path[k].clone();
        // The item before, in the same list.
        let Some(prev) = (0..i).rev().find(|&j| {
            let p = &self.blocks[j].path;
            p.len() > k + 1 && Rc::ptr_eq(&p[k], &list) && !Rc::ptr_eq(&p[k + 1], &self.blocks[i].path[k + 1])
        }) else {
            return false;
        };
        let prev_path = self.blocks[prev].path.clone();
        // A list already inside that item takes it.
        let nested = if prev + 1 < i || prev_path.len() > k + 3 {
            prev_path.get(k + 2).filter(|e| e.is_list()).cloned()
        } else {
            None
        };
        let nested = nested.unwrap_or_else(|| Element::new(&list.tag));
        let b = self.block_mut(i);
        let rest: Vec<El> = b.path[k + 1..].to_vec();
        let mut path: Vec<El> = prev_path[..k + 2].to_vec();
        path.push(nested);
        path.extend(rest);
        b.path = path;
        true
    }

    /// Takes list item `i` out of its list, into the list around it if
    /// any (Shift-Tab, Enter on an empty item); false where it isn't one.
    pub fn outdent_item(&mut self, i: usize) -> bool {
        let Some(k) = innermost_list(&self.blocks[i].path) else {
            return false;
        };
        let b = self.block_mut(i);
        let item = b.path[k + 1].clone();
        let rest: Vec<El> = b.path[k + 2..].to_vec();
        b.path.truncate(k);
        // Inside an outer item: a new item of the outer list.
        let outer = k >= 2 && b.path[k - 1].is("li") && b.path[k - 2].is_list();
        if outer {
            b.path.pop();
            b.path.push(item.copy());
        } else if rest.is_empty() {
            b.path.push(item.renamed("div"));
        }
        b.path.extend(rest);
        true
    }

    // Reading.

    /// How each block is drawn, `base` being the text size in pixels.
    pub fn infos(&self, base: f32) -> Vec<Info> {
        let mut out = Vec::with_capacity(self.blocks.len());
        // Per list: how many items so far, and the last item seen.
        let mut counters: Vec<(El, i64, Option<El>)> = Vec::new();
        for (bi, b) in self.blocks.iter().enumerate() {
            let mut info = Info {
                heading: None,
                pre: false,
                rule: b.is_rule(),
                quotes: 0,
                lists: 0,
                marker: None,
                align: Align::Left,
                spaced: false,
                size: base,
                bold: false,
                italic: false,
                underline: false,
                color: None,
                family: None,
            };
            for (k, e) in b.path.iter().enumerate() {
                match e.tag.as_str() {
                    "blockquote" => info.quotes += 1,
                    "ul" | "ol" | "dir" | "menu" => info.lists += 1,
                    "pre" | "code" | "tt" => info.pre = e.is("pre") || info.pre,
                    "center" => info.align = Align::Center,
                    "th" => {
                        info.bold = true;
                        info.align = Align::Center;
                    }
                    "b" | "strong" => info.bold = true,
                    "i" | "em" => info.italic = true,
                    "font" => {
                        if let Some(c) = e.attr("color").and_then(css::color) {
                            info.color = Some(c);
                        }
                        if let Some(s) = e.attr("size").and_then(css::font_tag_size) {
                            info.size = s * base / 16.;
                        }
                        if let Some(f) = e.attr("face").and_then(css::first_family) {
                            info.family = Some(f);
                        }
                    }
                    _ => {}
                }
                if let Some(level) = e.heading_level() {
                    info.heading = Some(level);
                    info.bold = true;
                    info.size *= [2., 1.5, 1.17, 1., 0.83, 0.67][level as usize - 1];
                }
                if let Some(a) = e.attr("align").and_then(Align::parse) {
                    info.align = a;
                }
                for (name, value) in e.style() {
                    match name.as_str() {
                        "text-align" => {
                            if let Some(a) = Align::parse(&value) {
                                info.align = a;
                            }
                        }
                        "color" => {
                            if let Some(c) = css::color(&value) {
                                info.color = Some(c);
                            }
                        }
                        "font-size" => {
                            // Sizes are drawn relative to the composer's.
                            if let Some(s) = css::font_size(&value, info.size * 16. / base) {
                                info.size = s * base / 16.;
                            }
                        }
                        "font-family" => info.family = css::first_family(&value),
                        "font-weight" => info.bold = weight_bold(&value).unwrap_or(info.bold),
                        "font-style" => info.italic = value.contains("italic") || value.contains("oblique"),
                        "text-decoration" | "text-decoration-line" => {
                            info.underline = value.contains("underline")
                        }
                        _ => {}
                    }
                }
                // The list item's marker, on its first block.
                if e.is("li") && k > 0 && b.path[k - 1].is_list() {
                    let list = &b.path[k - 1];
                    let counter = match counters.iter_mut().find(|(l, _, _)| Rc::ptr_eq(l, list)) {
                        Some(c) => c,
                        None => {
                            let start = list.attr("start").and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(1);
                            counters.push((list.clone(), start - 1, None));
                            counters.last_mut().expect("just pushed")
                        }
                    };
                    let first_block = counter.2.as_ref().is_none_or(|last| !Rc::ptr_eq(last, e));
                    if first_block {
                        counter.1 += e.attr("value").and_then(|v| v.trim().parse::<i64>().ok()).map_or(1, |v| v - counter.1);
                        counter.2 = Some(e.clone());
                        if k == b.path.len() - 1 || bi == 0 || !self.blocks[bi - 1].path.iter().any(|x| Rc::ptr_eq(x, e)) {
                            info.marker = Some(if list.is("ol") {
                                Marker::Number(counter.1)
                            } else {
                                Marker::Bullet(info.lists)
                            });
                        }
                    }
                }
            }
            info.spaced = matches!(b.element().tag.as_str(), "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "ul" | "ol");
            out.push(info);
        }
        out
    }

    /// The document as plain text: the text version of a formatted
    /// message, and what a plain draft keeps of it.
    pub fn to_plain(&self) -> String {
        let infos = self.infos(16.);
        let mut out = String::new();
        for (i, (b, info)) in self.blocks.iter().zip(&infos).enumerate() {
            if i > 0 {
                out.push('\n');
                // Paragraphs read apart, as they're shown.
                let delimiter = self.blocks[i - 1].text().trim_end() == "--";
                if infos[i - 1].spaced && !delimiter && !b.is_empty() && info.marker.is_none() && infos[i - 1].marker.is_none() {
                    out.push('\n');
                }
            }
            let quote = "> ".repeat(info.quotes);
            let indent = "  ".repeat(info.lists.saturating_sub(1));
            let marker = match &info.marker {
                Some(Marker::Bullet(_)) => "- ".to_owned(),
                Some(Marker::Number(n)) => format!("{n}. "),
                None if info.lists > 0 => "  ".into(),
                None => String::new(),
            };
            if info.rule {
                out.push_str(&quote);
                out.push_str("----------");
                continue;
            }
            let mut text = inline_text(b);
            // The signature's line (RFC 3676 §4.3) keeps its space.
            if text.trim_end() == "--" {
                text = "-- ".into();
            }
            for (j, line) in text.split('\n').enumerate() {
                if j > 0 {
                    out.push('\n');
                }
                out.push_str(&quote);
                if !line.is_empty() || !marker.trim().is_empty() {
                    out.push_str(&indent);
                    out.push_str(if j == 0 { &marker } else { "  " });
                }
                out.push_str(line);
            }
        }
        // Quotes leave "> " on blank lines; the text keeps no trailing spaces
        // except the signature's "-- ".
        out.lines()
            .map(|l| if l == "-- " { l } else { l.trim_end() })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The `file:` pictures it shows, by attachment name.
    pub fn file_images(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in &self.blocks {
            for (_, image) in b.images() {
                if let Some(name) = image.file_name()
                    && !out.iter().any(|n| n == name)
                {
                    out.push(name.to_owned());
                }
            }
        }
        out
    }

    /// The blocks of the signature: after a line that reads just "--".
    pub fn signature_range(&self) -> Option<std::ops::Range<usize>> {
        let start = self.blocks.iter().position(|b| b.text().trim_end() == "--")?;
        // To the quote or forwarded message after it, or the end.
        let end = self.blocks[start + 1..]
            .iter()
            .position(|b| {
                let text = b.text();
                let first = text.lines().next().unwrap_or("");
                crate::manage::composing::introduces_quote(first)
                    || crate::manage::composing::starts_forward(first)
                    || b.path.iter().any(|e| e.is("blockquote"))
            })
            .map_or(self.blocks.len(), |i| start + 1 + i);
        Some(start..end)
    }

    /// Where a reply's quote or forwarded message starts, if it has one.
    pub fn quote_start(&self) -> Option<usize> {
        self.blocks.iter().position(|b| {
            let text = b.text();
            let first = text.lines().next().unwrap_or("");
            crate::manage::composing::introduces_quote(first) || crate::manage::composing::starts_forward(first)
        })
    }
}

/// A block's text with pictures as their descriptions.
fn inline_text(b: &Block) -> String {
    b.runs
        .iter()
        .map(|r| match &r.image {
            Some(i) if !i.alt.trim().is_empty() => format!("[{}]", i.alt.trim()),
            Some(_) => String::new(),
            None => r.text.clone(),
        })
        .collect()
}

/// The index of the innermost list a block is an item of (the list's
/// element in its path, followed by the item).
pub fn innermost_list(path: &[El]) -> Option<usize> {
    (0..path.len().saturating_sub(1))
        .rev()
        .find(|&k| path[k].is_list() && path[k + 1].is("li"))
}

/// Where a list made of a block goes: around a plain line or heading (which
/// becomes, or goes in, the item), inside anything else (a table cell).
fn container_of(path: &[El]) -> Vec<El> {
    let own = path.last().expect("a block has its element");
    if own.is_plain_block() {
        path[..path.len() - 1].to_vec()
    } else {
        path.to_vec()
    }
}

/// Whether a CSS font-weight is bold.
pub fn weight_bold(value: &str) -> Option<bool> {
    let v = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
    match v.as_str() {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        n => n.parse::<u32>().ok().map(|w| w >= 600),
    }
}
