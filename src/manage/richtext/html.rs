//! Reading a document from HTML and writing it back.
//!
//! Reading keeps what an email can show and drops what it mustn't run:
//! scripts, frames, forms, embedded objects, event handlers and script
//! addresses. Elements the editor doesn't know stay in the blocks' paths
//! and the runs' wrappers, and are written back as they came.

use super::{Block, Doc, El, Element, Image, Run, Style, css};
use html5ever::tendril::TendrilSink as _;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use std::rc::Rc;

/// Elements that hold blocks (or are one); everything else is inline.
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "address" | "article" | "aside" | "blockquote" | "center" | "dd" | "details" | "dialog"
            | "dir" | "div" | "dl" | "dt" | "fieldset" | "figcaption" | "figure" | "footer"
            | "form" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup" | "li"
            | "main" | "menu" | "nav" | "ol" | "p" | "pre" | "section" | "summary" | "table"
            | "tbody" | "thead" | "tfoot" | "tr" | "td" | "th" | "caption" | "ul" | "legend"
    )
}

/// Elements dropped with everything in them.
fn is_dropped(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "noscript" | "template" | "iframe" | "frame" | "frameset" | "object"
            | "embed" | "applet" | "param" | "input" | "button" | "select" | "option"
            | "optgroup" | "textarea" | "datalist" | "output" | "meta" | "link" | "base"
            | "title" | "head" | "svg" | "math" | "canvas" | "video" | "audio" | "source"
            | "track" | "map" | "area" | "colgroup" | "col" | "portal" | "dialog"
    )
}

/// Whether `url` may be a link's address.
fn safe_href(url: &str) -> bool {
    let u: String = url.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    let lower = u.to_ascii_lowercase();
    match lower.split_once(':') {
        Some((scheme, _)) if !scheme.contains(['/', '?', '#']) => {
            matches!(scheme, "http" | "https" | "mailto" | "tel" | "sms" | "cid" | "ftp")
        }
        _ => true,
    }
}

/// Whether `url` may be a picture's address; `files` lets the draft's own
/// `file:<name>` pictures through.
fn safe_src(url: &str, files: bool) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if let Some(name) = lower.strip_prefix("file:") {
        // A name among the draft's attachments, never a path.
        return files && !name.is_empty() && !name.contains(['/', '\\']);
    }
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("cid:")
        || (lower.starts_with("data:image/") && !lower.starts_with("data:image/svg"))
}

/// An element's attributes without handlers and unsafe values.
fn clean_attrs(tag: &str, attrs: &[html5ever::Attribute], files: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for a in attrs {
        let name = a.name.local.to_string().to_ascii_lowercase();
        let value = a.value.to_string();
        if name.starts_with("on")
            || name.starts_with("xmlns")
            || name.contains(':')
            || matches!(name.as_str(), "contenteditable" | "srcset" | "formaction" | "action" | "ping" | "autofocus")
        {
            continue;
        }
        match name.as_str() {
            "href" if !safe_href(&value) => continue,
            "src" if tag != "img" || !safe_src(&value, files) => continue,
            "background" | "lowsrc" | "dynsrc" if css::unsafe_value(&value) || !safe_src(&value, false) => continue,
            "style" => {
                let kept: Vec<(String, String)> = css::declarations(&value)
                    .into_iter()
                    .filter(|(n, v)| !css::unsafe_value(v) && !css::unsafe_value(n))
                    .collect();
                if !kept.is_empty() {
                    out.push((name, css::style_attr(&kept)));
                }
                continue;
            }
            _ if css::unsafe_value(&value) => continue,
            _ => {}
        }
        out.push((name, value));
    }
    out
}

struct Builder {
    blocks: Vec<Rc<Block>>,
    /// The block being filled, and whether a `<br>` was in it.
    current: Option<(Block, bool)>,
    sheets: Vec<String>,
    /// The element inline text outside any block goes in.
    root: Option<El>,
    files: bool,
}

/// A space that collapses (HTML white space), kept apart from typed
/// spaces until the block ends.
const SOFT: char = '\u{1}';

impl Builder {
    fn block_for(&mut self, path: &[El]) -> &mut (Block, bool) {
        let same = self.current.as_ref().is_some_and(|(b, _)| {
            b.path.len() == path.len().max(1) && b.path.iter().zip(path).all(|(a, b)| Rc::ptr_eq(a, b))
        });
        if !same {
            self.flush();
            let path = if path.is_empty() {
                let root = self.root.get_or_insert_with(|| Element::new("div")).clone();
                vec![root]
            } else {
                path.to_vec()
            };
            self.current = Some((Block { path, runs: vec![] }, false));
        }
        self.current.as_mut().expect("just set")
    }
    fn text(&mut self, path: &[El], text: &str, style: &Rc<Style>, pre: bool) {
        let (block, _) = self.block_for(path);
        let mut out = String::with_capacity(text.len());
        let mut last = block
            .runs
            .last()
            .and_then(|r| r.text.chars().last())
            .unwrap_or('\n');
        for ch in text.chars() {
            let ch = match ch {
                '\u{a0}' => ' ',
                '\r' if !pre => continue,
                c if !pre && c.is_whitespace() => {
                    if matches!(last, ' ' | SOFT | '\n') {
                        continue;
                    }
                    SOFT
                }
                '\u{1}' => continue,
                c if c.is_control() && c != '\n' && c != '\t' => continue,
                c => c,
            };
            out.push(ch);
            last = ch;
        }
        if !out.is_empty() {
            block.runs.push(Run::text(out, style.clone()));
        }
    }
    fn line_break(&mut self, path: &[El], style: &Rc<Style>) {
        let (block, br) = self.block_for(path);
        // A collapsing space before a break isn't shown.
        if let Some(run) = block.runs.last_mut()
            && run.image.is_none()
            && run.text.ends_with(SOFT)
        {
            run.text.pop();
        }
        block.runs.push(Run::text("\n", style.clone()));
        *br = true;
    }
    fn image(&mut self, path: &[El], image: Image, style: &Rc<Style>) {
        let (block, _) = self.block_for(path);
        block.runs.push(Run::image(image, style.clone()));
    }
    /// Ends the block being filled: its collapsing spaces settled, kept if
    /// it shows anything (text, a picture, a line break).
    fn flush(&mut self) {
        let Some((mut block, br)) = self.current.take() else {
            return;
        };
        // Leading and trailing collapsing spaces, and those after breaks.
        let mut text_seen = false;
        for run in &mut block.runs {
            if run.image.is_some() {
                text_seen = true;
                continue;
            }
            let mut out = String::with_capacity(run.text.len());
            for ch in run.text.chars() {
                if ch == SOFT && !text_seen {
                    continue;
                }
                text_seen = ch != '\n';
                out.push(ch);
            }
            run.text = out;
        }
        for run in block.runs.iter_mut().rev() {
            if run.image.is_some() {
                break;
            }
            let trimmed = run.text.trim_end_matches(SOFT).len();
            let all = trimmed == 0;
            run.text.truncate(trimmed);
            if !all {
                break;
            }
        }
        for run in &mut block.runs {
            if run.image.is_none() && run.text.contains(SOFT) {
                run.text = run.text.replace(SOFT, " ");
            }
        }
        block.normalize();
        // A last break shows no line of its own after text.
        if let Some(run) = block.runs.last_mut()
            && run.image.is_none()
            && run.text.ends_with('\n')
        {
            run.text.pop();
        }
        block.normalize();
        if !block.runs.is_empty() || br {
            self.blocks.push(Rc::new(block));
        }
    }
    /// A block of its own with nothing in it (a rule, an empty cell).
    fn empty_block(&mut self, path: &[El]) {
        self.flush();
        self.blocks.push(Rc::new(Block { path: path.to_vec(), runs: vec![] }));
    }
    fn produced_for(&self, element: &El, since: usize) -> bool {
        self.blocks[since..].iter().any(|b| b.path.iter().any(|e| Rc::ptr_eq(e, element)))
    }
}

fn tag_of(node: &Handle) -> Option<String> {
    match &node.data {
        NodeData::Element { name, .. } => Some(name.local.to_string().to_ascii_lowercase()),
        _ => None,
    }
}

/// The text inside `node`, without recursion (for absurdly deep markup).
fn flat_text(node: &Handle) -> String {
    let mut out = String::new();
    let mut stack = vec![node.clone()];
    while let Some(n) = stack.pop() {
        if let NodeData::Text { contents } = &n.data {
            out.push_str(&contents.borrow());
        }
        if !Rc::ptr_eq(&n, node) && tag_of(&n).is_some_and(|t| is_dropped(&t) || t == "style") {
            continue;
        }
        for child in n.children.borrow().iter().rev() {
            stack.push(child.clone());
        }
    }
    out
}

const MAX_DEPTH: usize = 160;

fn walk(node: &Handle, b: &mut Builder, path: &mut Vec<El>, style: &Rc<Style>, pre: bool, depth: usize) {
    if depth > MAX_DEPTH {
        b.text(path, &flat_text(node), style, pre);
        return;
    }
    match &node.data {
        NodeData::Text { contents } => {
            let text = contents.borrow();
            b.text(path, &text, style, pre);
        }
        NodeData::Document => {
            for child in node.children.borrow().iter() {
                walk(child, b, path, style, pre, depth + 1);
            }
        }
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.to_string().to_ascii_lowercase();
            if tag == "style" {
                let css = flat_text(node);
                if !css.trim().is_empty() && !css::unsafe_value(&css) {
                    b.sheets.push(css.replace("</", "<\\/"));
                }
                return;
            }
            if is_dropped(&tag) {
                return;
            }
            let children = || node.children.borrow().clone();
            // Another app's namespaced markup (Office's o:p): its text only.
            if tag.contains(':') {
                for child in children().iter() {
                    walk(child, b, path, style, pre, depth + 1);
                }
                return;
            }
            let attrs = clean_attrs(&tag, &attrs.borrow(), b.files);
            match tag.as_str() {
                "html" | "body" => {
                    for child in children().iter() {
                        walk(child, b, path, style, pre, depth + 1);
                    }
                }
                "br" => b.line_break(path, style),
                "wbr" => {}
                "img" => {
                    let Some(src) = attrs.iter().find(|(n, _)| n == "src").map(|(_, v)| v.clone()) else {
                        return;
                    };
                    b.image(path, image_from(&src, &attrs), style);
                }
                "hr" => {
                    let mut p = path.clone();
                    p.push(Rc::new(Element { tag, attrs }));
                    b.empty_block(&p);
                }
                t if is_block_tag(t) => {
                    // Picacho's own wrapper around a sent message is left out.
                    let ours = t == "div"
                        && attrs.iter().any(|(n, v)| n == "class" && v.split_whitespace().any(|c| c == "picacho-body"));
                    if ours && path.is_empty() {
                        for child in children().iter() {
                            walk(child, b, path, style, pre, depth + 1);
                        }
                        b.flush();
                        return;
                    }
                    b.flush();
                    let element = Rc::new(Element { tag: tag.clone(), attrs });
                    let since = b.blocks.len();
                    path.push(element.clone());
                    let pre = pre || tag == "pre";
                    for child in children().iter() {
                        walk(child, b, path, style, pre, depth + 1);
                    }
                    b.flush();
                    // Cells and items stay, even empty: a table keeps its shape.
                    if matches!(tag.as_str(), "td" | "th" | "li") && !b.produced_for(&element, since) {
                        b.empty_block(path);
                    }
                    path.pop();
                }
                _ => {
                    let inner = Rc::new(inline_style(&tag, &attrs, style));
                    for child in children().iter() {
                        walk(child, b, path, &inner, pre, depth + 1);
                    }
                }
            }
        }
        _ => {}
    }
}

/// A picture from an `<img>`'s cleaned attributes.
fn image_from(src: &str, attrs: &[(String, String)]) -> Image {
    let get = |n: &str| attrs.iter().find(|(a, _)| a == n).map(|(_, v)| v.as_str());
    let number = |v: &str| v.trim().trim_end_matches("px").trim().parse::<f32>().ok().filter(|n| *n > 0. && n.is_finite());
    let style = get("style").map(css::declarations).unwrap_or_default();
    let from_style = |n: &str| style.iter().find(|(a, _)| a == n).and_then(|(_, v)| v.trim().strip_suffix("px")).and_then(number);
    Image {
        src: src.to_owned(),
        alt: get("alt").unwrap_or("").to_owned(),
        width: get("width").and_then(number).or_else(|| from_style("width")),
        height: get("height").and_then(number).or_else(|| from_style("height")),
        attrs: attrs
            .iter()
            .filter(|(n, _)| !matches!(n.as_str(), "src" | "alt" | "width" | "height"))
            .cloned()
            .collect(),
    }
}

/// The style inside an inline element: what it says read into the
/// fields, what's left kept as a wrapper.
fn inline_style(tag: &str, attrs: &[(String, String)], parent: &Style) -> Style {
    let mut s = parent.clone();
    let mut known_tag = true;
    match tag {
        "b" | "strong" => s.bold = Some(true),
        "i" | "em" | "cite" | "var" | "dfn" => s.italic = Some(true),
        "u" | "ins" => s.underline = true,
        "s" | "strike" | "del" => s.strike = true,
        "code" | "tt" | "kbd" | "samp" => s.code = true,
        "sup" => s.sup = true,
        "sub" => s.sub = true,
        "a" => {}
        "span" | "font" => {}
        _ => known_tag = false,
    }
    let mut rest: Vec<(String, String)> = Vec::new();
    let mut rest_style: Vec<(String, String)> = Vec::new();
    for (name, value) in attrs {
        match (tag, name.as_str()) {
            ("a", "href") => s.link = Some(value.clone()),
            ("font", "color") => s.color = css::color(value).or(s.color),
            ("font", "face") => s.family = css::first_family(value).or(s.family.take()),
            ("font", "size") => s.size = css::font_tag_size(value).or(s.size),
            (_, "style") => {
                for (n, v) in css::declarations(value) {
                    let inherited = s.size.unwrap_or(16.);
                    let read = match n.as_str() {
                        "font-weight" => super::weight_bold(&v).map(|b| s.bold = Some(b)).is_some(),
                        "font-style" => {
                            s.italic = Some(v.contains("italic") || v.contains("oblique"));
                            true
                        }
                        "text-decoration" | "text-decoration-line" => {
                            let l = v.to_ascii_lowercase();
                            if l.contains("underline") {
                                s.underline = true;
                            }
                            if l.contains("line-through") {
                                s.strike = true;
                            }
                            if l.trim() == "none" {
                                s.underline = false;
                                s.strike = false;
                            }
                            l.split_whitespace().all(|w| matches!(w, "underline" | "line-through" | "none"))
                        }
                        "color" => css::color(&v).map(|c| s.color = Some(c)).is_some(),
                        "background-color" | "background" => css::color(&v).map(|c| s.background = Some(c)).is_some(),
                        "font-family" => css::first_family(&v).map(|f| s.family = Some(f)).is_some()
                            && !v.contains(','),
                        "font-size" => css::font_size(&v, inherited).map(|z| s.size = Some(z)).is_some(),
                        "vertical-align" => match v.trim() {
                            "super" => {
                                s.sup = true;
                                true
                            }
                            "sub" => {
                                s.sub = true;
                                true
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    if !read {
                        rest_style.push((n, v));
                    }
                }
            }
            ("a", "title" | "target" | "rel") => {}
            _ => rest.push((name.clone(), value.clone())),
        }
    }
    if !rest_style.is_empty() {
        rest.push(("style".into(), css::style_attr(&rest_style)));
    }
    // What isn't read stays as the element it came in (a `span` for the
    // tags read above), so it is written back.
    if !known_tag || !rest.is_empty() {
        let wrapper_tag = if known_tag || tag == "a" { "span" } else { tag };
        s.wrappers.push(Rc::new(Element { tag: wrapper_tag.to_owned(), attrs: rest }));
    }
    s
}

/// A document from HTML: a whole message or a fragment. `files` lets the
/// draft's own `file:` pictures through (Picacho's saved drafts), never
/// for HTML from elsewhere.
pub fn parse(html: &str, files: bool) -> Doc {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    let mut b = Builder { blocks: vec![], current: None, sheets: vec![], root: None, files };
    // The head's style sheets.
    let mut stack = vec![dom.document.clone()];
    let mut body = None;
    while let Some(n) = stack.pop() {
        match tag_of(&n).as_deref() {
            Some("head") => {
                for child in n.children.borrow().iter() {
                    if tag_of(child).as_deref() == Some("style") {
                        let css = flat_text(child);
                        if !css.trim().is_empty() && !css::unsafe_value(&css) {
                            b.sheets.push(css.replace("</", "<\\/"));
                        }
                    }
                }
            }
            Some("body") => body = Some(n.clone()),
            Some("html") | None => {
                for child in n.children.borrow().iter() {
                    stack.push(child.clone());
                }
            }
            _ => {}
        }
    }
    let Some(body) = body else {
        return Doc::empty();
    };
    // The body's own look (another app's font for the whole message)
    // holds everything.
    let mut path = Vec::new();
    if let NodeData::Element { attrs, .. } = &body.data {
        let mut kept = clean_attrs("div", &attrs.borrow(), files);
        let mut style: Vec<(String, String)> = Vec::new();
        kept.retain(|(n, v)| match n.as_str() {
            "style" => {
                style.extend(css::declarations(v));
                false
            }
            "text" => {
                style.push(("color".into(), v.clone()));
                false
            }
            "dir" | "lang" | "class" => true,
            _ => false,
        });
        // Margins and backgrounds belong to the window, not the text.
        style.retain(|(n, _)| !n.starts_with("margin") && !n.starts_with("padding") && !n.starts_with("background"));
        if !style.is_empty() {
            kept.push(("style".into(), css::style_attr(&style)));
        }
        if !kept.is_empty() {
            path.push(Rc::new(Element { tag: "div".into(), attrs: kept }));
        }
    }
    walk(&body, &mut b, &mut path, &Style::plain(), false, 0);
    b.flush();
    // An empty line is still somewhere to write.
    let mut doc = Doc { blocks: b.blocks, sheets: b.sheets };
    if doc.blocks.is_empty() {
        path.push(Element::new("div"));
        doc.blocks.push(Rc::new(Block { path, runs: vec![] }));
    }
    doc
}

fn escape(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn open_tag(e: &Element, out: &mut String) {
    out.push('<');
    out.push_str(&e.tag);
    write_attrs(&e.attrs, out);
    out.push('>');
}

fn write_attrs(attrs: &[(String, String)], out: &mut String) {
    for (n, v) in attrs {
        out.push(' ');
        out.push_str(n);
        out.push_str("=\"");
        escape(v, out);
        out.push('"');
    }
}

fn is_void(tag: &str) -> bool {
    matches!(tag, "hr" | "br" | "img" | "wbr")
}

/// A block's text as HTML: spaces that would collapse kept as `&nbsp;`,
/// line breaks as `<br>` (outside `<pre>`).
fn write_text(text: &str, pre: bool, prev: &mut char, next_of: impl Fn(usize) -> Option<char>, out: &mut String) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, ch)) in chars.iter().enumerate() {
        if pre {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c => out.push(c),
            }
            *prev = ch;
            continue;
        }
        let next = chars.get(k + 1).map(|(_, c)| *c).or_else(|| next_of(i + ch.len_utf8()));
        match ch {
            '\n' => out.push_str("<br>"),
            '\t' => out.push_str("&nbsp;&nbsp;&nbsp;&nbsp;"),
            ' ' => {
                let hard = matches!(*prev, ' ' | '\n') || next.is_none_or(|n| n == '\n');
                out.push_str(if hard { "&nbsp;" } else { " " });
            }
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
        *prev = ch;
    }
}

/// A run's opening and closing tags, outermost first.
fn run_tags(s: &Style) -> (String, String) {
    let mut open = String::new();
    let mut close: Vec<String> = Vec::new();
    for w in &s.wrappers {
        open_tag(w, &mut open);
        close.push(format!("</{}>", w.tag));
    }
    if let Some(link) = &s.link {
        open.push_str("<a href=\"");
        escape(link, &mut open);
        open.push_str("\">");
        close.push("</a>".into());
    }
    let mut css: Vec<(String, String)> = Vec::new();
    if let Some(c) = s.color {
        css.push(("color".into(), c.to_css()));
    }
    if let Some(c) = s.background {
        css.push(("background-color".into(), c.to_css()));
    }
    if let Some(f) = &s.family {
        let quoted = if f.contains(' ') { format!("'{}'", f.replace('\'', "")) } else { f.clone() };
        css.push(("font-family".into(), quoted));
    }
    if let Some(z) = s.size {
        css.push(("font-size".into(), format!("{}px", (z * 10.).round() / 10.)));
    }
    if s.bold == Some(false) {
        css.push(("font-weight".into(), "normal".into()));
    }
    if s.italic == Some(false) {
        css.push(("font-style".into(), "normal".into()));
    }
    if !css.is_empty() {
        open.push_str("<span style=\"");
        escape(&css::style_attr(&css), &mut open);
        open.push_str("\">");
        close.push("</span>".into());
    }
    for (on, tag) in [
        (s.bold == Some(true), "b"),
        (s.italic == Some(true), "i"),
        (s.underline, "u"),
        (s.strike, "s"),
        (s.code, "code"),
        (s.sup, "sup"),
        (s.sub, "sub"),
    ] {
        if on {
            open.push('<');
            open.push_str(tag);
            open.push('>');
            close.push(format!("</{tag}>"));
        }
    }
    close.reverse();
    (open, close.concat())
}

fn write_runs(b: &Block, out: &mut String) {
    let pre = b.is_pre();
    let text = b.text();
    let mut prev = '\n';
    let mut at = 0;
    for run in &b.runs {
        let start = at;
        at += run.text.len();
        if let Some(image) = &run.image {
            let (open, close) = run_tags(&run.style);
            out.push_str(&open);
            out.push_str("<img src=\"");
            escape(&image.src, out);
            out.push('"');
            out.push_str(" alt=\"");
            escape(&image.alt, out);
            out.push('"');
            if let Some(w) = image.width {
                out.push_str(&format!(" width=\"{}\"", w.round()));
            }
            if let Some(h) = image.height {
                out.push_str(&format!(" height=\"{}\"", h.round()));
            }
            write_attrs(&image.attrs, out);
            out.push('>');
            out.push_str(&close);
            prev = 'x';
            continue;
        }
        let (open, close) = run_tags(&run.style);
        out.push_str(&open);
        let next_of = |i: usize| text[start + i..].chars().next();
        write_text(&run.text, pre, &mut prev, next_of, out);
        out.push_str(&close);
    }
}

/// The document as HTML, without a document around it.
pub fn to_html(doc: &Doc) -> String {
    let mut out = String::new();
    for sheet in &doc.sheets {
        out.push_str("<style>");
        out.push_str(&sheet.replace("</", "<\\/"));
        out.push_str("</style>");
    }
    let mut open: Vec<El> = Vec::new();
    for (i, b) in doc.blocks.iter().enumerate() {
        let shared = open
            .iter()
            .zip(&b.path)
            .take_while(|(a, b)| Rc::ptr_eq(a, b))
            .count();
        // Two blocks written into one element (its text split around
        // what was between): a break keeps them on their lines.
        let same_element = shared == b.path.len() && shared == open.len() && i > 0;
        while open.len() > shared {
            let e = open.pop().expect("open");
            if !is_void(&e.tag) {
                out.push_str(&format!("</{}>", e.tag));
            }
        }
        if same_element {
            out.push_str("<br>");
        }
        for e in &b.path[shared..] {
            open_tag(e, &mut out);
            open.push(e.clone());
        }
        if b.is_rule() {
            continue;
        }
        if b.runs.is_empty() {
            // An empty line keeps its height, unless blocks follow inside
            // its element.
            let holds_next = doc
                .blocks
                .get(i + 1)
                .is_some_and(|n| n.path.iter().any(|e| Rc::ptr_eq(e, b.element())));
            if !holds_next {
                out.push_str("<br>");
            }
            continue;
        }
        write_runs(b, &mut out);
    }
    while let Some(e) = open.pop() {
        if !is_void(&e.tag) {
            out.push_str(&format!("</{}>", e.tag));
        }
    }
    out
}

/// A document from Markdown, as Picacho wrote formatted mail before (and
/// writes templates and signatures): `file:` pictures kept.
pub fn from_markdown(markdown: &str, signature_html: &str) -> Doc {
    parse(&crate::sync::smtp::markdown_fragment(markdown, signature_html), true)
}

/// Shorthand: a document's HTML after `change` (pictures' addresses
/// rewritten for sending).
pub fn rewrite_images(html: &str, change: &dyn Fn(&Image) -> Option<Image>) -> String {
    let mut doc = parse(html, true);
    for i in 0..doc.blocks.len() {
        let has = doc.blocks[i].runs.iter().any(|r| r.image.is_some());
        if !has {
            continue;
        }
        let b = doc.block_mut(i);
        let mut runs = Vec::with_capacity(b.runs.len());
        for run in b.runs.drain(..) {
            match &run.image {
                Some(image) => match change(image) {
                    Some(new) => runs.push(Run { image: Some(Rc::new(new)), ..run }),
                    // Gone: its description stays, as text.
                    None if !image.alt.trim().is_empty() => {
                        runs.push(Run::text(image.alt.trim().to_owned(), run.style.clone()))
                    }
                    None => {}
                },
                None => runs.push(run),
            }
        }
        b.runs = runs;
        b.normalize();
    }
    to_html(&doc)
}
