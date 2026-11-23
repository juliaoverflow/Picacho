//! Blitz document and software painting, independent of GPUI and mailbox state.
use super::resources;
use crate::manage::model::InlineParts;
use anyrender::{ImageRenderer, PaintScene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, LocalName, NodeId, QualName, UserAgentStylesheet, ns};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use kurbo::Shape as _;
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// The fonts every email is laid out with, found and loaded once.
///
/// Left to itself, Blitz gives each document a font collection of its own:
/// every opened email scanned the system's fonts and loaded the files it
/// used again, some 80 ms before its first paint even for a short email.
/// Each document gets a copy of this one instead: the system's fonts and
/// the loaded font files are shared, while fonts an email brings itself
/// (`@font-face`) stay in its own copy, unseen by other emails.
static FONTS: LazyLock<Mutex<blitz_dom::FontContext>> = LazyLock::new(|| {
    use fontique::{Blob, Collection, CollectionOptions, SourceCache};
    let mut fonts = blitz_dom::FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: true,
        }),
    };
    // As Blitz does for a collection of its own: list markers' font.
    fonts
        .collection
        .register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);
    Mutex::new(fonts)
});

fn fonts() -> blitz_dom::FontContext {
    FONTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Finds and loads the system's common fonts in the background, so that
/// the first email opened needn't wait for them: Latin text, and an emoji,
/// as so much mail has (its fallback font took some 70 ms to find).
#[cfg_attr(test, allow(dead_code))]
pub fn warm_up() {
    let _ = std::thread::Builder::new()
        .name("picacho-fonts".into())
        .spawn(|| {
            let mut doc = EmailDocument::with_parts(
                "<p>Picacho <b>mail</b> <i>reader</i> 0123456789 🎁</p>",
                Default::default(),
                true,
                Tone::Light,
            );
            doc.layout(600);
        });
}

/// `break-word`, not `anywhere`: both break a word too long for its line,
/// but `anywhere` also lets a table cell shrink to one letter's width, so
/// "Unsubscribe" in a narrow cell came out a letter per line.
///
/// Forms stay (whole newsletters sit inside a `<form>`); only their controls
/// are hidden. The `data-picacho-*` markers are set by `EmailDocument`:
/// a fixed box is placed like an absolute one, since the email is one long
/// page rather than a scrolling viewport; a sticky one stays in its place;
/// and a block holding only images has no line height of its own, as in
/// browsers' quirks modes, so sliced images in table cells meet without gaps.
const READER_CSS: &str = "body { margin: 12px; font: 14px sans-serif; overflow-wrap: break-word; } \
img { max-width: 100%; height: auto; } \
iframe, object, embed, input, button, textarea, select { display: none !important; } \
[data-picacho-fixed] { position: absolute !important; } \
[data-picacho-sticky] { position: relative !important; } \
[data-picacho-images] { line-height: 0 !important; }";

/// The page and text colors for each tone. The page color itself is painted
/// under the email (see `pixels_at`) rather than set on `html`, so that an
/// email's own `body` background reaches the edges, as in browsers.
const LIGHT_CSS: &str = "html { color: #202124; }";
/// For emails with their own dark styles, which Blitz then applies.
const DARK_CSS: &str =
    "html { color: #e8eaed; } a[href] { color: #8ab4f8; } a[href]:visited { color: #c58af9; }";
const LIGHT_PAGE: u32 = 0xffffff;
const DARK_PAGE: u32 = 0x1f2023;
/// Blocked remote images keep their size as an empty outline, so the layout
/// is the one the sender designed; one without a size gets a small box, and
/// one with alt text shows it instead (see `prepare_blocked_images`).
/// Tracking pixels stay invisible.
const BLOCKED_CSS: &str = "img[data-picacho-remote] { outline: 1px dashed rgba(128, 128, 128, 0.55); outline-offset: -1px; } \
img[data-picacho-remote]:not([data-picacho-sized]) { min-width: 16px; min-height: 16px; } \
[data-picacho-remote-background] { outline: 1px dashed rgba(128, 128, 128, 0.35); outline-offset: -1px; } \
[data-picacho-tracker] { outline: none !important; min-width: 0 !important; min-height: 0 !important; } \
[data-picacho-alt] { display: inline-block; box-sizing: border-box; max-width: 100%; overflow: hidden; vertical-align: bottom; \
outline: 1px dashed rgba(128, 128, 128, 0.55); outline-offset: -1px; padding: 2px 4px; \
font: 12px/1.35 sans-serif; color: #80868b; text-align: center; text-decoration: none; white-space: normal; }";

/// An email's HTML, so that Blitz parses it as HTML. Blitz takes a document
/// for XML when it starts with an XML declaration or an XHTML DOCTYPE, or its
/// `<html>` declares the XHTML namespace. Countless emails begin with
/// `<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" ...>`, but
/// a message's HTML part is `text/html`, which browsers and mail apps parse
/// as HTML whatever its DOCTYPE says. Parsed as XML, its unclosed `<meta>` and
/// `<br>` tags swallow everything after them and the `<` in its CSS starts
/// elements, and the email shows nothing. A comment first stops the sniffing
/// and changes nothing else: HTML allows one before the DOCTYPE.
pub(super) fn as_html(html: &str) -> String {
    format!("<!---->{html}")
}

/// How the body is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Light,
    /// The email's own `prefers-color-scheme: dark` styles.
    NativeDark,
    /// Light styles with every painted color's lightness inverted.
    InvertedDark,
}
impl Tone {
    pub fn for_email(html: &str, dark: bool) -> Self {
        if !dark || light_only(html) {
            Tone::Light
        } else if has_dark_backgrounds(html) {
            Tone::NativeDark
        } else {
            Tone::InvertedDark
        }
    }
}

/// Whether the email says it is made only for light pages: a
/// `<meta name="color-scheme" content="light">` (or `only light`), as
/// mail apps that respect it leave such an email uncolored.
fn light_only(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("<meta") {
        rest = &rest[at + 5..];
        let tag = &rest[..rest.find('>').unwrap_or(rest.len())];
        if !(tag.contains("name=\"color-scheme\"") || tag.contains("name='color-scheme'")
            || tag.contains("name=color-scheme"))
        {
            continue;
        }
        let Some(at) = tag.find("content=") else { continue };
        let value = tag[at + 8..].trim_start_matches(['"', '\'']);
        let value = &value[..value.find(['"', '\'']).unwrap_or(value.len())];
        let words: Vec<&str> = value.split_whitespace().collect();
        return words.contains(&"light") && !words.contains(&"dark");
    }
    false
}

/// Whether the email has `prefers-color-scheme: dark` rules that set
/// backgrounds. Many emails have such a rule only to swap a logo; with
/// their light page kept and the text made light, they were unreadable.
fn has_dark_backgrounds(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("prefers-color-scheme") {
        rest = &rest[at + "prefers-color-scheme".len()..];
        let condition = rest.trim_start();
        let Some(condition) = condition.strip_prefix(':') else {
            continue;
        };
        if !condition.trim_start().starts_with("dark") {
            continue;
        }
        // The rule's block, to its matching brace.
        let Some(open) = rest.find('{') else {
            break;
        };
        let mut depth = 0usize;
        let mut end = rest.len();
        for (i, c) in rest[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        if rest[open..end].contains("background") {
            return true;
        }
        rest = &rest[end..];
    }
    false
}

/// The rendering mode browsers pick from an HTML document's DOCTYPE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Compat {
    Standards,
    /// "Almost standards": table cells holding only images have no strut.
    LimitedQuirks,
    /// Any block holding only images has no strut.
    Quirks,
}

/// The mode the HTML spec gives a document with this DOCTYPE (a subset of
/// its rules that covers what email is written with).
/// https://html.spec.whatwg.org/multipage/parsing.html#the-initial-insertion-mode
pub(super) fn compat_mode(html: &str) -> Compat {
    let mut rest = html.trim_start_matches('\u{feff}').trim_start();
    // Comments may come first.
    while let Some(after) = rest.strip_prefix("<!--") {
        rest = after.find("-->").map_or("", |end| &after[end + 3..]).trim_start();
    }
    // `get`: the ninth byte may fall inside a character ("<!DOCTYé").
    if !rest.get(..9).is_some_and(|start| start.eq_ignore_ascii_case("<!doctype")) {
        return Compat::Quirks;
    }
    let doctype = &rest[9..rest.find('>').unwrap_or(rest.len())];
    let lower = doctype.to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    if words.next() != Some("html") {
        return Compat::Quirks;
    }
    let Some(public) = lower.find("public") else {
        return Compat::Standards;
    };
    let quoted: Vec<&str> = lower[public..].split(['"', '\'']).skip(1).step_by(2).collect();
    let public_id = quoted.first().copied().unwrap_or("");
    let has_system_id = quoted.len() > 1;
    if public_id.starts_with("-//w3c//dtd xhtml 1.0 transitional//")
        || public_id.starts_with("-//w3c//dtd xhtml 1.0 frameset//")
    {
        Compat::LimitedQuirks
    } else if public_id.starts_with("-//w3c//dtd html 4.01 transitional//")
        || public_id.starts_with("-//w3c//dtd html 4.01 frameset//")
    {
        if has_system_id {
            Compat::LimitedQuirks
        } else {
            Compat::Quirks
        }
    } else if public_id.starts_with("-//w3c//dtd html 4.01//")
        || public_id.starts_with("-//w3c//dtd xhtml 1.0 strict//")
        || public_id.starts_with("-//w3c//dtd xhtml 1.1//")
        || public_id.starts_with("-//w3c//dtd xhtml basic")
        || public_id.starts_with("-//wapforum//dtd xhtml mobile")
        || public_id.is_empty()
    {
        Compat::Standards
    } else {
        Compat::Quirks
    }
}

fn attr_name(name: &str) -> QualName {
    QualName::new(None, ns!(), LocalName::from(name))
}

/// How far each settling step moves the animation clock, in seconds: far
/// enough that every animation and transition an email starts has finished.
/// Just short of a whole hour, so that an endless animation of a common
/// length (0.5 s, 1 s, 2 s, 3 s) settles near its last keyframe, which is
/// usually its resting state, rather than its first.
const SETTLE: f64 = 3600. - 0.001;

/// A scrollable box in the email, in document coordinates, and the ways it
/// can still scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scroller {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

pub(super) struct EmailDocument {
    pub(super) doc: HtmlDocument,
    tone: Tone,
    cancelled: Arc<AtomicBool>,
    pub(super) changed: Arc<AtomicBool>,
    /// Remote fetches in flight (see `Resources::pending`).
    pending: Arc<std::sync::atomic::AtomicUsize>,
    /// Links clicked since last taken (see `take_clicked`).
    clicked: Arc<Mutex<Vec<String>>>,
    /// The animation clock (see `SETTLE`).
    clock: f64,
    /// The width asked for, and the one laid out at: wider when the email's
    /// content is (see `layout`).
    width: u32,
    laid_width: u32,
    /// The person's text zoom (WCAG 1.4.4): the email laid out this much
    /// narrower and shown this much larger. 1 is the email's own size.
    text_zoom: f32,
    /// The height of the window, which `vh` and height media queries see,
    /// and its scale, which resolution media queries see.
    screen_height: u32,
    screen_scale: f32,
}
impl Drop for EmailDocument {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
impl EmailDocument {
    #[cfg(test)]
    pub(super) fn new(html: &str, blocked: bool) -> Self {
        Self::with_tone(html, blocked, Tone::Light)
    }
    #[cfg(test)]
    pub(super) fn with_tone(html: &str, blocked: bool, tone: Tone) -> Self {
        Self::with_parts(html, Default::default(), blocked, tone)
    }
    /// `inline` holds the parts the HTML shows by `cid:` address.
    pub(super) fn with_parts(
        html: &str,
        inline: Arc<InlineParts>,
        blocked: bool,
        tone: Tone,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resources = resources::Resources::new(
            blocked,
            inline,
            changed.clone(),
            cancelled.clone(),
            pending.clone(),
        );
        let links = resources::Links::default();
        let clicked = links.clicked.clone();
        let doc = HtmlDocument::from_html(
            &as_html(html),
            DocumentConfig {
                // Relative resources have no implicit sender-controlled origin.
                base_url: Some("about:blank".into()),
                font_ctx: Some(fonts()),
                net_provider: Some(Arc::new(resources)),
                navigation_provider: Some(Arc::new(links)),
                ua_stylesheets: [
                    Some(UserAgentStylesheet::default_sheet()),
                    Some(UserAgentStylesheet::parse(if tone == Tone::NativeDark {
                        DARK_CSS
                    } else {
                        LIGHT_CSS
                    })),
                    Some(UserAgentStylesheet::parse(READER_CSS)),
                    blocked.then(|| UserAgentStylesheet::parse(BLOCKED_CSS)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                ..Default::default()
            },
        );
        let mut email = Self {
            doc,
            tone,
            cancelled,
            changed,
            pending,
            clicked,
            clock: 0.,
            width: 0,
            laid_width: 0,
            text_zoom: 1.,
            screen_height: 600,
            screen_scale: 1.,
        };
        if blocked {
            email.prepare_blocked_images();
        }
        email.mark_image_lines(compat_mode(html));
        email
    }
    /// Whether nothing but a command can change the document now: no
    /// fetch in flight, and no change waiting to be laid out.
    pub(super) fn settled(&self) -> bool {
        // `pending` first: a fetch that finished set `changed` before it.
        self.pending.load(Ordering::Acquire) == 0 && !self.changed.load(Ordering::Acquire)
    }
    /// The elements in document order.
    fn elements(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.doc.root_node().id];
        while let Some(id) = stack.pop() {
            let Some(node) = self.doc.get_node(id) else {
                continue;
            };
            if node.is_element() {
                out.push(id);
            }
            stack.extend(node.children.iter().rev().copied());
        }
        out
    }
    /// Marks the blocks whose lines hold only images (see `READER_CSS`):
    /// table cells in "almost standards" mode, any block in quirks mode.
    fn mark_image_lines(&mut self, compat: Compat) {
        if compat == Compat::Standards {
            return;
        }
        let blocks: &[&str] = match compat {
            Compat::Quirks => &["td", "th", "div", "p", "center", "li", "body"],
            _ => &["td", "th"],
        };
        let marked: Vec<NodeId> = self
            .elements()
            .into_iter()
            .filter(|&id| {
                let tag = self.doc.get_node(id).and_then(|n| n.element_data());
                tag.is_some_and(|e| blocks.contains(&&*e.name.local)) && self.holds_only_images(id)
            })
            .collect();
        if marked.is_empty() {
            return;
        }
        let mut mutator = self.doc.mutate();
        for id in marked {
            mutator.set_attribute(id, attr_name("data-picacho-images"), "");
        }
    }
    /// Whether an element's content is images alone (in inline wrappers such
    /// as links), with at least one image and no text.
    fn holds_only_images(&self, id: NodeId) -> bool {
        const WRAPPERS: &[&str] = &["a", "span", "font", "b", "strong", "i", "em", "u", "br", "center"];
        let mut images = 0;
        let mut stack: Vec<NodeId> = self
            .doc
            .get_node(id)
            .map(|n| n.children.to_vec())
            .unwrap_or_default();
        while let Some(child) = stack.pop() {
            let Some(node) = self.doc.get_node(child) else {
                continue;
            };
            if let Some(text) = node.text_data() {
                if !text.content.chars().all(|c| c.is_ascii_whitespace()) {
                    return false;
                }
            } else if let Some(element) = node.element_data() {
                let tag: &str = &element.name.local;
                if tag == "img" {
                    images += 1;
                } else if WRAPPERS.contains(&tag) {
                    stack.extend(node.children.iter().copied());
                } else {
                    return false;
                }
            }
        }
        images > 0
    }
    /// Marks blocked remote images for `BLOCKED_CSS`: whether each has a
    /// size and whether it is a tracking pixel. One with alt text and no
    /// tracking pixel's size is replaced by a box of its size showing the
    /// text, as browsers show an image that can't be loaded.
    fn prepare_blocked_images(&mut self) {
        struct Blocked {
            id: NodeId,
            sized: bool,
            tracker: bool,
            alt: Option<String>,
        }
        let mut found = Vec::new();
        for id in self.elements() {
            let Some(element) = self.doc.get_node(id).and_then(|n| n.element_data()) else {
                continue;
            };
            if &*element.name.local != "img" {
                continue;
            }
            let remote = |v: &str| {
                let lower = v.trim().to_ascii_lowercase();
                lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("//")
            };
            let src = element.attr(LocalName::from("src")).unwrap_or("").trim();
            // Its other sources too: `srcset`, and a `<picture>`'s `<source>`s.
            let srcset = element.attr(LocalName::from("srcset")).unwrap_or("");
            let in_picture = self
                .doc
                .get_node(id)
                .and_then(|n| n.parent)
                .and_then(|p| self.doc.get_node(p))
                .and_then(|p| p.element_data())
                .is_some_and(|p| &*p.name.local == "picture");
            if !(remote(src) || srcset.split(',').any(remote) || (src.is_empty() && in_picture)) {
                continue;
            }
            let attr = |name: &str| element.attr(LocalName::from(name));
            let style = attr("style").map(inline_dimensions).unwrap_or_default();
            let width = attr("width").and_then(parse_dimension).or(style.0);
            let height = attr("height").and_then(parse_dimension).or(style.1);
            let tiny = |d: Option<Dimension>| matches!(d, Some(Dimension::Px(px)) if px <= 1.);
            let tracker = tiny(width) || tiny(height) || style.2;
            let alt = attr("alt")
                .map(str::trim)
                .filter(|alt| !alt.is_empty() && !tracker)
                .map(String::from);
            found.push(Blocked {
                id,
                sized: width.is_some() || height.is_some(),
                tracker,
                alt,
            });
        }
        // Boxes painted with a remote picture (`background=`, or a style's
        // `background-image: url(…)`) keep an outline, so a blocked banner
        // reads as one.
        let backgrounds: Vec<NodeId> = self
            .elements()
            .into_iter()
            .filter(|&id| {
                let Some(e) = self.doc.get_node(id).and_then(|n| n.element_data()) else {
                    return false;
                };
                let remote = |v: &str| {
                    let lower = v.to_ascii_lowercase();
                    lower.contains("http:") || lower.contains("https:") || lower.contains("url(//")
                };
                e.attr(LocalName::from("background")).is_some_and(|b| remote(b.trim()))
                    || e.attr(LocalName::from("style")).is_some_and(|st| {
                        let lower = st.to_ascii_lowercase();
                        lower.contains("background") && lower.contains("url(") && remote(&lower)
                    })
            })
            .collect();
        if !backgrounds.is_empty() {
            let mut mutator = self.doc.mutate();
            for id in backgrounds {
                mutator.set_attribute(id, attr_name("data-picacho-remote-background"), "");
            }
        }
        if found.is_empty() {
            return;
        }
        // The alt text boxes, built while the images can still be read.
        let replacements: Vec<(NodeId, Vec<blitz_dom::Attribute>, String)> = found
            .iter()
            .filter_map(|image| {
                let alt = image.alt.clone()?;
                let element = self.doc.get_node(image.id)?.element_data()?;
                let attr = |name: &str| element.attr(LocalName::from(name));
                let mut style = String::new();
                for (name, property) in [("width", "width"), ("height", "height")] {
                    match attr(name).and_then(parse_dimension) {
                        Some(Dimension::Px(px)) => style += &format!("{property}: {px}px; "),
                        Some(Dimension::Percent(p)) => style += &format!("{property}: {p}%; "),
                        None => {}
                    }
                }
                match attr("align").map(|a| a.trim().to_ascii_lowercase()).as_deref() {
                    Some("left") => style += "float: left; ",
                    Some("right") => style += "float: right; ",
                    _ => {}
                }
                if let Some(own) = attr("style") {
                    style += own;
                }
                let mut attrs = vec![
                    blitz_dom::Attribute {
                        name: attr_name("data-picacho-alt"),
                        value: String::new(),
                    },
                    blitz_dom::Attribute {
                        name: attr_name("data-picacho-src"),
                        value: attr("src").unwrap_or("").to_owned(),
                    },
                    blitz_dom::Attribute {
                        name: attr_name("style"),
                        value: style,
                    },
                ];
                for name in ["id", "class", "title"] {
                    if let Some(value) = attr(name) {
                        attrs.push(blitz_dom::Attribute {
                            name: attr_name(name),
                            value: value.to_owned(),
                        });
                    }
                }
                Some((image.id, attrs, alt))
            })
            .collect();
        let mut mutator = self.doc.mutate();
        for image in &found {
            mutator.set_attribute(image.id, attr_name("data-picacho-remote"), "");
            if image.sized {
                mutator.set_attribute(image.id, attr_name("data-picacho-sized"), "");
            }
            if image.tracker {
                mutator.set_attribute(image.id, attr_name("data-picacho-tracker"), "");
            }
        }
        for (id, attrs, alt) in replacements {
            let span = mutator.create_element(
                QualName::new(None, ns!(html), LocalName::from("span")),
                attrs,
            );
            let text = mutator.create_text_node(&alt);
            mutator.append_children(span, &[text]);
            mutator.replace_node_with(id, &[span]);
        }
    }
    /// The links clicked since the last call, oldest first.
    pub(super) fn take_clicked(&self) -> Vec<String> {
        std::mem::take(&mut *self.clicked.lock().unwrap_or_else(|e| e.into_inner()))
    }
    /// The `href` of the link under the pointer, as written.
    pub(super) fn hovered_href(&self) -> Option<String> {
        let mut next = self
            .doc
            .get_hover_node_id()
            .and_then(|id| self.doc.get_node(id));
        while let Some(node) = next {
            if let Some(element) = node.element_data()
                && element.is_link()
            {
                return element.attr(blitz_dom::local_name!("href")).map(String::from);
            }
            next = node.parent.and_then(|id| self.doc.get_node(id));
        }
        None
    }
    /// The openable link under the pointer, if any (see `openable_url`).
    pub(super) fn hovered_link(&self) -> Option<String> {
        self.hovered_href()
            .as_deref()
            .and_then(super::openable_url)
            .map(String::from)
    }
    /// The window's height and scale, for `vh` units and media queries.
    pub(super) fn set_screen(&mut self, height: u32, scale: f32) {
        self.screen_height = height.max(1);
        self.screen_scale = if scale > 0. { scale } else { 1. };
    }
    fn set_viewport(&mut self, width: u32) {
        self.doc.set_viewport(Viewport::new(
            width.max(1),
            self.screen_height,
            1.,
            if self.tone == Tone::NativeDark {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
        ));
        self.doc
            .set_media_device_pixel_ratio(Some(self.screen_scale));
    }
    /// Styles and lays the document out. Animations and transitions are
    /// settled at their end rather than played, so that nothing is left
    /// invisible at an animation's first frame and nothing repaints on its
    /// own (see `SETTLE`).
    pub(super) fn resolve(&mut self) {
        self.clock += SETTLE;
        self.doc.resolve(self.clock);
        for _ in 0..3 {
            if !self.doc.is_animating() {
                break;
            }
            self.clock += SETTLE;
            self.doc.resolve(self.clock);
        }
        self.place_fixed_boxes();
    }
    /// Fixed and sticky boxes are placed against the viewport, which here is
    /// the whole email: painted strip by strip, a fixed box showed in every
    /// strip. They are given `READER_CSS`'s markers instead.
    fn place_fixed_boxes(&mut self) {
        use style::computed_values::position::T as Position;
        let mut fixed = Vec::new();
        for (id, node) in self.doc.tree().iter() {
            let Some(element) = node.element_data() else {
                continue;
            };
            let position = node.primary_styles().map(|s| s.get_box().clone_position());
            let marker = match position {
                Some(Position::Fixed) => "data-picacho-fixed",
                Some(Position::Sticky) => "data-picacho-sticky",
                _ => continue,
            };
            if element.attr(LocalName::from(marker)).is_none() {
                fixed.push((id, marker));
            }
        }
        if fixed.is_empty() {
            return;
        }
        {
            let mut mutator = self.doc.mutate();
            for (id, marker) in fixed {
                mutator.set_attribute(id, attr_name(marker), "");
            }
        }
        self.clock += SETTLE;
        self.doc.resolve(self.clock);
    }
    /// Lays the email out `width` CSS pixels wide and returns its height in
    /// document pixels. An email whose content is wider (a fixed 700-pixel
    /// table on a phone) is laid out at its content's width, up to three
    /// times the asked width, and shown scaled down by `zoom`, as mobile
    /// mail apps do, rather than cut off at the right.
    pub(super) fn layout(&mut self, width: u32) -> f32 {
        let width = width.max(1);
        self.width = width;
        // Zoomed in: laid out narrower in CSS pixels, then shown larger.
        let css = ((width as f32 / self.text_zoom).round() as u32).max(1);
        self.set_viewport(css);
        self.resolve();
        self.laid_width = css;
        let content = self.content_width();
        if content > css as f32 + 1. {
            let wide = (content.ceil() as u32).min(css.saturating_mul(3));
            if wide > css {
                self.set_viewport(wide);
                self.resolve();
                self.laid_width = wide;
            }
        }
        self.content_height()
    }
    /// The person's text zoom, 0.5 to 3 (see `text_zoom`).
    pub(super) fn set_text_zoom(&mut self, zoom: f32) {
        self.text_zoom = zoom.clamp(0.5, 3.);
    }
    pub(super) fn text_zoom(&self) -> f32 {
        self.text_zoom
    }
    /// How much the email is scaled: down to fit the width asked for, and
    /// up by the person's text zoom.
    pub(super) fn zoom(&self) -> f32 {
        if self.laid_width == 0 {
            1.
        } else {
            self.width as f32 / self.laid_width as f32
        }
    }
    /// The horizontal extent of the email's content column, (left, width).
    /// Most templates center a fixed-width table in a page-wide background;
    /// this follows the tallest child down from the body while it fills
    /// its parent, and stops at the first one that is clearly narrower.
    pub(super) fn content_column(&self) -> (f32, f32) {
        let full = self.doc.root_element().final_layout().size.width.max(1.);
        let Some(mut node) = self
            .doc
            .query_selector("body")
            .ok()
            .flatten()
            .and_then(|id| self.doc.get_node(id))
        else {
            return (0., full);
        };
        for _ in 0..12 {
            let size = node.final_layout().size;
            let Some(child) = node
                .children
                .iter()
                .filter_map(|id| self.doc.get_node(*id))
                .filter(|n| n.is_element())
                .max_by(|a, b| {
                    a.final_layout()
                        .size
                        .height
                        .total_cmp(&b.final_layout().size.height)
                })
            else {
                break;
            };
            let child_size = child.final_layout().size;
            if child_size.height < size.height * 0.6 || child_size.width < 200. {
                break;
            }
            node = child;
            if child_size.width < full * 0.9 {
                break;
            }
        }
        let width = node.final_layout().size.width;
        if width < full * 0.9 && width >= 200. {
            (node.absolute_position(0., 0.).x, width)
        } else {
            (0., full)
        }
    }
    /// The right and bottom edges of everything the email shows: Taffy's
    /// scrollable overflow leaves out absolutely positioned boxes, so the
    /// boxes are walked too, as far as their overflow-clipping ancestors
    /// let them show.
    fn content_extent(&self) -> (f32, f32) {
        use style::computed_values::overflow_x::T as Overflow;
        use style::computed_values::visibility::T as Visibility;
        let root = self.doc.root_element();
        let layout = root.final_layout();
        let mut right = layout.size.width.max(layout.scrollable_overflow_rect.right);
        let mut bottom = layout.size.height.max(layout.scrollable_overflow_rect.bottom);
        let unclipped = kurbo::Rect::new(f64::MIN, f64::MIN, f64::MAX, f64::MAX);
        let mut stack = vec![(root.id, unclipped)];
        while let Some((id, clip)) = stack.pop() {
            let Some(node) = self.doc.get_node(id) else {
                continue;
            };
            let Some(styles) = node.primary_styles() else {
                continue;
            };
            let size = node.final_layout().size;
            let at = node.absolute_position(0., 0.);
            let rect = kurbo::Rect::new(
                at.x as f64,
                at.y as f64,
                (at.x + size.width) as f64,
                (at.y + size.height) as f64,
            );
            let shown = rect.intersect(clip);
            if size.width > 0.
                && size.height > 0.
                && shown.width() > 0.
                && shown.height() > 0.
                && styles.get_inherited_box().clone_visibility() == Visibility::Visible
            {
                right = right.max(shown.x1 as f32);
                bottom = bottom.max(shown.y1 as f32);
            }
            let style_box = styles.get_box();
            let mut inner = clip;
            if style_box.clone_overflow_x() != Overflow::Visible {
                inner.x0 = inner.x0.max(rect.x0);
                inner.x1 = inner.x1.min(rect.x1);
            }
            if style_box.clone_overflow_y() != Overflow::Visible {
                inner.y0 = inner.y0.max(rect.y0);
                inner.y1 = inner.y1.min(rect.y1);
            }
            drop(styles);
            for child in node.children.iter() {
                if self.doc.get_node(*child).is_some_and(|c| c.is_element()) {
                    stack.push((*child, inner));
                }
            }
        }
        (right, bottom)
    }
    pub(super) fn content_width(&self) -> f32 {
        self.content_extent().0.max(1.)
    }
    pub(super) fn content_height(&self) -> f32 {
        self.content_extent().1.max(1.)
    }
    /// The page color behind the email as 0xRRGGBB, as painted: the root's
    /// or body's background over the tone's page.
    pub(super) fn page_color(&self) -> u32 {
        let page = if self.tone == Tone::NativeDark {
            DARK_PAGE
        } else {
            LIGHT_PAGE
        };
        let channel = |shift: u32| ((page >> shift) & 0xff) as f32 / 255.;
        let mut color = [channel(16), channel(8), channel(0)];
        if let Some([r, g, b, a]) = self.canvas_background() {
            for (c, v) in color.iter_mut().zip([r, g, b]) {
                *c = *c * (1. - a) + v * a;
            }
        }
        if self.tone == Tone::InvertedDark {
            let [r, g, b, _] = dark_color([color[0], color[1], color[2], 1.], Role::Surface);
            color = [r, g, b];
        }
        let c = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
        c(color[0]) << 16 | c(color[1]) << 8 | c(color[2])
    }
    /// The background Blitz paints over the whole canvas, straight sRGBA:
    /// the root element's, or failing that the body's (CSS's propagation).
    fn canvas_background(&self) -> Option<[f32; 4]> {
        let color = |node: &blitz_dom::Node| {
            let styles = node.primary_styles()?;
            let current = styles.clone_color();
            let color = styles
                .clone_background_color()
                .resolve_to_absolute(&current)
                .to_color_space(style::color::ColorSpace::Srgb);
            let [r, g, b, a] = *color.raw_components();
            (a > 0.).then_some([r, g, b, a])
        };
        let root = self.doc.try_root_element()?;
        color(root).or_else(|| {
            root.children
                .iter()
                .filter_map(|id| self.doc.get_node(*id))
                .find(|n| n.data.is_element_with_tag_name(&blitz_dom::local_name!("body")))
                .and_then(color)
        })
    }
    /// Whether most of the email's first screens are light. An email whose
    /// dark styles leave its light page as it was (only a logo swapped) is
    /// then read with inverted colors instead.
    pub(super) fn mostly_light(&mut self) -> bool {
        let width = self.laid_width.max(1) as f32;
        let height = self.content_height().min(1600.);
        let scale = 0.125;
        let (pw, ph) = (
            (width * scale).ceil().max(1.) as u32,
            (height * scale).ceil().max(1.) as u32,
        );
        let pixels = self.pixels_at(pw, ph, scale, 0., 0.);
        let light = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| {
                let [b, g, r] = [p[0], p[1], p[2]].map(|v| v as f32 / 255.);
                0.2126 * r + 0.7152 * g + 0.0722 * b > 0.75
            })
            .count();
        light * 2 > (pw * ph) as usize
    }
    pub(super) fn pixels(&mut self, width: u32, height: u32, scale: f32, offset: u32) -> Vec<u8> {
        self.pixels_at(width, height, scale, 0., offset as f32 / scale)
    }
    /// Paints the region whose top-left is (`x`, `y`) in document pixels.
    pub(super) fn pixels_at(
        &mut self,
        width: u32,
        height: u32,
        scale: f32,
        x: f32,
        y: f32,
    ) -> Vec<u8> {
        let mut renderer = VelloCpuImageRenderer::new(width, height);
        let mut pixels = Vec::new();
        self.doc
            .set_viewport_scroll(blitz_dom::Point { x: 0., y: y as f64 });
        let (paint_width, paint_height) = (
            (width as f32 / scale + x).ceil() as u32,
            (height as f32 / scale).ceil() as u32,
        );
        // Keep Blitz's layout/culling in CSS pixels. Apply DPI scaling to the
        // recorded vector commands so geometry and text remain crisp at every scale.
        let mut scene = anyrender::Scene::new();
        // The tone's page, under the email's own canvas background if any.
        let page = if self.tone == Tone::NativeDark {
            DARK_PAGE
        } else {
            LIGHT_PAGE
        };
        scene.fill(
            peniko::Fill::NonZero,
            kurbo::Affine::IDENTITY,
            peniko::Color::from_rgb8((page >> 16) as u8, (page >> 8) as u8, page as u8),
            None,
            &kurbo::Rect::new(0., 0., paint_width as f64, paint_height as f64),
        );
        blitz_paint::paint_scene(
            &mut scene,
            &mut self.doc,
            1.,
            paint_width,
            paint_height,
            0,
            0,
        );
        if self.tone == Tone::InvertedDark {
            darken_scene(&mut scene);
        }
        renderer.render_to_vec(
            |painter| {
                painter.append_scene(
                    scene,
                    kurbo::Affine::scale(scale as f64) * kurbo::Affine::translate((-x as f64, 0.)),
                )
            },
            &mut pixels,
        );
        self.doc
            .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
        // Vello produces premultiplied RGBA; GPUI's atlas expects straight BGRA.
        to_straight_bgra(&mut pixels);
        pixels
    }
}

/// The page color behind the email for a tone, as 0xRRGGBB, before the
/// email is laid out (see `EmailDocument::page_color`).
pub fn page_color(tone: Tone) -> u32 {
    match tone {
        Tone::Light => LIGHT_PAGE,
        Tone::NativeDark => DARK_PAGE,
        Tone::InvertedDark => {
            let [r, g, b, _] = dark_color([1., 1., 1., 1.], Role::Surface);
            let c = |v: f32| (v * 255.).round() as u32;
            c(r) << 16 | c(g) << 8 | c(b)
        }
    }
}

/// A width or height attribute's value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Dimension {
    Px(f32),
    Percent(f32),
}
fn parse_dimension(value: &str) -> Option<Dimension> {
    let value = value.trim();
    let number: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let n: f32 = number.parse().ok()?;
    let unit = value[number.len()..].trim_start();
    if unit.starts_with('%') {
        Some(Dimension::Percent(n))
    } else if unit.is_empty() || unit.eq_ignore_ascii_case("px") {
        Some(Dimension::Px(n))
    } else {
        None
    }
}
/// The width and height an inline `style` gives (in pixels or percent), and
/// whether it hides the element.
fn inline_dimensions(style: &str) -> (Option<Dimension>, Option<Dimension>, bool) {
    let (mut width, mut height, mut hidden) = (None, None, false);
    for declaration in style.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches("!important").trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "width" => width = parse_dimension(value).or(width),
            "height" => height = parse_dimension(value).or(height),
            "display" if value.eq_ignore_ascii_case("none") => hidden = true,
            "visibility" if value.eq_ignore_ascii_case("hidden") => hidden = true,
            _ => {}
        }
    }
    (width, height, hidden)
}

/// Recolors a painted scene for dark reading, the way mail apps do: light
/// page and panel backgrounds turn dark and dark text turns light, while
/// saturated or mid-tone surfaces (brand banners, buttons) keep their
/// colors, only slightly dimmed. Text keeps its contrast with what it is
/// painted on: on a surface that stays light (a pastel brand color), dark
/// text stays dark. Thin fills (borders, rules, underlines) are treated as
/// lines. Hue and chroma are kept throughout. Images keep their own colors,
/// as photos and logos must, SVG images included (`is_image_layer`).
fn darken_scene(scene: &mut anyrender::Scene) {
    use anyrender::{Paint, recording::RenderCommand};
    /// A surface already painted, and the lightness it was given; `None`
    /// for an image, over which text is left as it is.
    struct Under {
        rect: kurbo::Rect,
        lightness: Option<f32>,
    }
    let recolor = |brush: &mut Paint, role: Role| -> Option<f32> {
        match brush {
            Paint::Solid(color) => {
                color.components = dark_color(color.components, role);
                (color.components[3] > 0.6).then(|| lightness(color.components))
            }
            Paint::Gradient(gradient) => {
                let mut sum = 0.;
                let mut opaque = true;
                for stop in gradient.stops.iter_mut() {
                    let mut color: peniko::Color = stop.color.to_alpha_color();
                    color.components = dark_color(color.components, role);
                    sum += lightness(color.components);
                    opaque &= color.components[3] > 0.6;
                    stop.color = peniko::color::DynamicColor::from_alpha_color(color);
                }
                (opaque && !gradient.stops.is_empty()).then(|| sum / gradient.stops.len() as f32)
            }
            _ => None,
        }
    };
    // What a point was last painted with: the light page if nothing.
    let page = lightness(dark_color([1., 1., 1., 1.], Role::Surface));
    let under_at = |under: &[Under], p: kurbo::Point| -> Option<f32> {
        under
            .iter()
            .rev()
            .find(|u| u.rect.contains(p))
            .map_or(Some(page), |u| u.lightness)
    };
    let mut under: Vec<Under> = Vec::new();
    // A surface painted over others entirely hides them from `under_at`
    // (it looks for the newest that contains a point): they are let go as
    // it is added, so the search stays short. A newsletter's thousands of
    // cells, each painted over its row and table, were all searched for
    // every run of text.
    let add = |under: &mut Vec<Under>, new: Under| {
        let r = new.rect;
        under.retain(|u| !(r.x0 <= u.rect.x0 && r.y0 <= u.rect.y0 && r.x1 >= u.rect.x1 && r.y1 >= u.rect.y1));
        under.push(new);
    };
    // Each open layer, and whether it holds an image.
    let mut layers: Vec<bool> = Vec::new();
    let mut images = 0usize;
    for command in &mut scene.commands {
        match command {
            RenderCommand::PushLayer(layer) => {
                let image = blitz_paint::is_image_layer(layer);
                images += image as usize;
                layers.push(image);
                continue;
            }
            RenderCommand::PushClipLayer(_) => {
                layers.push(false);
                continue;
            }
            RenderCommand::PopLayer => {
                if layers.pop() == Some(true) {
                    images -= 1;
                }
                continue;
            }
            _ if images > 0 => continue,
            _ => {}
        }
        match command {
            RenderCommand::Fill(fill) => {
                let rect = fill.transform.transform_rect_bbox(fill.shape.bounding_box());
                if matches!(fill.brush, Paint::Image(_)) {
                    add(&mut under, Under {
                        rect,
                        lightness: None,
                    });
                    continue;
                }
                let area = rect.area();
                // A border or rule: a narrow bar, or the edges of a box.
                let (short, long) = (rect.width().min(rect.height()), rect.width().max(rect.height()));
                let thin = short <= 3.
                    || (short <= 6. && long >= short * 4.)
                    || (area > 0.
                        && fill.shape.area().abs() * fill.transform.determinant().abs() < area * 0.35);
                if thin {
                    let center = rect.center();
                    if under_at(&under, center).is_some_and(|l| l < 0.6) {
                        recolor(&mut fill.brush, Role::Line);
                    }
                } else if let Some(lightness) = recolor(&mut fill.brush, Role::Surface) {
                    add(&mut under, Under {
                        rect,
                        lightness: Some(lightness),
                    });
                }
            }
            RenderCommand::Stroke(stroke) => {
                let rect = stroke
                    .transform
                    .transform_rect_bbox(stroke.shape.bounding_box());
                if under_at(&under, rect.center()).is_some_and(|l| l < 0.6) {
                    recolor(&mut stroke.brush, Role::Line);
                }
            }
            RenderCommand::GlyphRun(run) => {
                let Some(glyph) = run.glyphs.get(run.glyphs.len() / 2) else {
                    continue;
                };
                let at = run.transform * kurbo::Point::new(glyph.x as f64, glyph.y as f64 - run.font_size as f64 * 0.3);
                // Over an image, or a surface that stays light, the text
                // keeps the contrast its sender gave it.
                if let Some(surface) = under_at(&under, at).filter(|l| *l < 0.6) {
                    recolor(&mut run.brush, Role::Text);
                    // At least WCAG's 4.5:1 against the surface (1.4.3).
                    if let Paint::Solid(color) = &mut run.brush {
                        color.components = readable_on(color.components, surface);
                    }
                }
            }
            RenderCommand::BoxShadow(shadow) => {
                shadow.brush.components = dark_color(shadow.brush.components, Role::Surface)
            }
            _ => {}
        }
    }
}

/// WCAG 2.1's relative luminance of a straight sRGB color.
fn luminance([r, g, b, _]: [f32; 4]) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}
/// WCAG's contrast ratio of two luminances.
fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// `text` made lighter, keeping its hue, until it has 4.5:1 contrast with
/// a dark surface of OKLab lightness `surface` (WCAG 2.1 §1.4.3).
pub(super) fn readable_on(text: [f32; 4], surface: f32) -> [f32; 4] {
    let ground = from_oklab([surface, 0., 0.]);
    let ground = luminance([ground[0], ground[1], ground[2], 1.]);
    if contrast(luminance(text), ground) >= 4.5 {
        return text;
    }
    let [mut l, a, b] = to_oklab([text[0], text[1], text[2]]);
    let mut out = text;
    for _ in 0..20 {
        l = (l + 0.05).min(1.);
        let [r, g, bb] = from_oklab([l, a, b]);
        out = [r, g, bb, text[3]];
        if contrast(luminance(out), ground) >= 4.5 || l >= 1. {
            break;
        }
    }
    out
}

/// OKLab lightness of a straight sRGB color.
fn lightness([r, g, b, _]: [f32; 4]) -> f32 {
    to_oklab([r, g, b])[0]
}

/// What a painted color is for, which decides how dark mode treats it.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Role {
    Surface,
    Line,
    Text,
}

// OKLab's published matrices, digit for digit.
#[allow(clippy::excessive_precision)]
fn to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}
#[allow(clippy::excessive_precision)]
fn from_oklab([lightness, oa, ob]: [f32; 3]) -> [f32; 3] {
    let l = (lightness + 0.3963377774 * oa + 0.2158037573 * ob).powi(3);
    let m = (lightness - 0.1055613458 * oa - 0.0638541728 * ob).powi(3);
    let s = (lightness - 0.0894841775 * oa - 1.2914855480 * ob).powi(3);
    let gamma = |c: f32| {
        let c = c.clamp(0., 1.);
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        }
    };
    [
        gamma(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        gamma(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        gamma(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    ]
}

/// A straight sRGB color for dark reading (see `darken_scene`). Lightness
/// is OKLab's, so "light" and "dark" match what the eye sees.
pub(super) fn dark_color([r, g, b, alpha]: [f32; 4], role: Role) -> [f32; 4] {
    let [l, a, bb] = to_oklab([r, g, b]);
    let chroma = (a * a + bb * bb).sqrt();
    // Flipped lightness: white becomes a soft charcoal, black near-white.
    let flipped = 0.95 - 0.73 * l;
    let next = match role {
        // Grey and pale tinted surfaces are page and panel backgrounds:
        // darken them. Saturated surfaces, even light ones, are brand color:
        // keep, a little dimmer.
        Role::Surface if chroma < 0.04 || (l > 0.85 && chroma < 0.1) => flipped.min(l),
        Role::Surface => l * 0.9,
        // Dark text turns light; light text sits on a kept color: keep it.
        Role::Text if l < 0.6 => flipped.max(l),
        Role::Text => l,
        // Rules and borders keep their contrast with what is around them.
        Role::Line => flipped,
    };
    let [r, g, b] = from_oklab([next, a, bb]);
    [r, g, b, alpha]
}

/// Converts premultiplied RGBA to straight BGRA in place. Email backgrounds
/// are opaque almost everywhere, so those pixels only swap two channels,
/// a word at a time.
fn to_straight_bgra(pixels: &mut [u8]) {
    let (head, words, tail) = bytemuck::pod_align_to_mut::<u8, u32>(pixels);
    if !head.is_empty() || !tail.is_empty() {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            unpremultiply(pixel);
        }
        return;
    }
    for word in words {
        let p = u32::from_le(*word);
        if p >> 24 == 0xff {
            *word = (p & 0xff00_ff00 | (p & 0xff) << 16 | (p >> 16) & 0xff).to_le();
        } else {
            let mut bytes = word.to_ne_bytes();
            unpremultiply(&mut bytes);
            *word = u32::from_ne_bytes(bytes);
        }
    }
}
fn unpremultiply(pixel: &mut [u8]) {
    let alpha = pixel[3] as u32;
    for c in &mut pixel[..3] {
        *c = (*c as u32 * 255).checked_div(alpha).unwrap_or(0).min(255) as u8;
    }
    pixel.swap(0, 2);
}

/// What a secondary click landed on, for the reader's context menu.
#[derive(Clone, Debug, Default)]
pub struct ContextTarget {
    pub link: Option<String>,
    pub image: Option<String>,
}

/// The word a long press selected: its text block and byte range.
#[derive(Clone, Copy, Debug)]
pub(super) struct Word {
    root: blitz_dom::NodeId,
    start: usize,
    end: usize,
    /// Where the press was, in document coordinates.
    at: (f32, f32),
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '\'' | '\u{2019}' | '-')
}

/// Whether a character's script is written without spaces between words,
/// so that its words are found with a dictionary.
fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32,
        0x0E00..=0x0EFF // Thai, Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1780..=0x17FF // Khmer
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF // Han
        | 0xFF66..=0xFF9F) // halfwidth Katakana
}

/// The dictionary word around `offset` in `text`, as a byte range.
fn dictionary_word(text: &str, offset: usize) -> Option<(usize, usize)> {
    use icu_segmenter::{WordSegmenter, options::WordBreakInvariantOptions};
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut breaks = segmenter.segment_str(text);
    let mut start = 0;
    while let Some(end) = breaks.next() {
        if end > offset {
            return breaks.is_word_like().then_some((start, end));
        }
        start = end;
    }
    None
}

impl EmailDocument {
    /// The text of an inline root (a block's run of text).
    fn inline_text(&self, root: NodeId) -> Option<&str> {
        Some(
            &self
                .doc
                .get_node(root)?
                .element_data()?
                .inline_layout_data
                .as_ref()?
                .text,
        )
    }
    /// Selects the word at a point, as a long press or a double click does.
    /// On a space or punctuation it selects that one character.
    pub(super) fn select_word_at(&mut self, x: f32, y: f32) -> Option<Word> {
        let Some((root, offset)) = self.doc.find_text_position(x, y) else {
            self.doc.clear_text_selection();
            return None;
        };
        let text = self.inline_text(root)?;
        let offset = offset.min(text.len());
        let (before, after) = text.split_at(offset);
        if after.chars().next().is_some_and(is_unspaced_script)
            && let Some((start, end)) = dictionary_word(text, offset)
        {
            self.doc.set_text_selection(root, start, root, end);
            return Some(Word {
                root,
                start,
                end,
                at: (x, y),
            });
        }
        let start = offset
            - before
                .chars()
                .rev()
                .take_while(|c| is_word_char(*c))
                .map(char::len_utf8)
                .sum::<usize>();
        let mut end = offset
            + after
                .chars()
                .take_while(|c| is_word_char(*c))
                .map(char::len_utf8)
                .sum::<usize>();
        if start == end {
            // Not on a word: the character under the press, if any.
            match after.chars().next().filter(|c| !c.is_whitespace()) {
                Some(c) => end += c.len_utf8(),
                None => {
                    self.doc.clear_text_selection();
                    return None;
                }
            }
        }
        self.doc.set_text_selection(root, start, root, end);
        Some(Word {
            root,
            start,
            end,
            at: (x, y),
        })
    }
    /// Selects the whole block of text at a point, as a triple click does.
    pub(super) fn select_block_at(&mut self, x: f32, y: f32) {
        let Some((root, _)) = self.doc.find_text_position(x, y) else {
            return;
        };
        let len = self.inline_text(root).map_or(0, str::len);
        self.doc.set_text_selection(root, 0, root, len);
    }
    /// Moves the end of the selection to a point, as a shift-click does.
    /// Returns false when there was no selection to extend.
    pub(super) fn extend_selection_to(&mut self, x: f32, y: f32) -> bool {
        if self
            .doc
            .get_selected_text()
            .is_none_or(|text| text.is_empty())
        {
            return false;
        }
        if let Some((node, offset)) = self.doc.find_text_position(x, y) {
            self.doc.update_selection_focus(node, offset);
        }
        true
    }
    /// Selects all of the email's text.
    /// The email as a screen reader reads it: its text, block by block,
    /// with images by their alt text, and its links (text, address) in
    /// order. Hidden content (scripts, styles, `display: none` templates)
    /// is left out.
    pub(super) fn accessible_text(&self) -> (String, Vec<(String, String)>) {
        const SKIP: &[&str] = &["script", "style", "head", "title", "template", "noscript"];
        const BLOCK: &[&str] = &[
            "p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "table", "blockquote",
            "section", "article", "header", "footer", "hr",
        ];
        let mut text = String::new();
        let mut links = Vec::new();
        let mut stack = vec![(self.doc.root_node().id, false)];
        while let Some((id, after)) = stack.pop() {
            let Some(node) = self.doc.get_node(id) else { continue };
            if after {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                continue;
            }
            if let Some(t) = node.text_data() {
                let words = t.content.split_whitespace().collect::<Vec<_>>().join(" ");
                if !words.is_empty() {
                    if !text.is_empty() && !text.ends_with(['\n', ' ']) {
                        text.push(' ');
                    }
                    text.push_str(&words);
                }
                continue;
            }
            let Some(e) = node.element_data() else {
                stack.extend(node.children.iter().rev().map(|c| (*c, false)));
                continue;
            };
            let tag: &str = &e.name.local;
            if SKIP.contains(&tag)
                || e.attr(LocalName::from("aria-hidden")) == Some("true")
                || node.primary_styles().is_some_and(|s| s.get_box().display.is_none())
            {
                continue;
            }
            if tag == "img"
                && let Some(alt) = e.attr(LocalName::from("alt")).map(str::trim).filter(|a| !a.is_empty())
            {
                text.push_str(&format!(" [{alt}] "));
            }
            if tag == "a"
                && let Some(href) = e.attr(LocalName::from("href"))
            {
                let label = node.text_content().split_whitespace().collect::<Vec<_>>().join(" ");
                links.push((if label.is_empty() { href.to_owned() } else { label }, href.to_owned()));
            }
            if BLOCK.contains(&tag) {
                stack.push((id, true));
            }
            stack.extend(node.children.iter().rev().map(|c| (*c, false)));
        }
        (text.trim().to_owned(), links)
    }
    pub(super) fn select_all(&mut self) {
        let roots: Vec<NodeId> = self
            .elements()
            .into_iter()
            .filter(|&id| self.inline_text(id).is_some_and(|t| !t.trim().is_empty()))
            .collect();
        if let (Some(&first), Some(&last)) = (roots.first(), roots.last()) {
            let len = self.inline_text(last).map_or(0, str::len);
            self.doc.set_text_selection(first, 0, last, len);
        }
    }
    /// Extends a long press's word selection to a point the finger moved to,
    /// keeping the whole word selected whichever way it goes.
    pub(super) fn extend_word_to(&mut self, word: Word, x: f32, y: f32) {
        let Some((node, offset)) = self.doc.find_text_position(x, y) else {
            return;
        };
        let before = if node == word.root {
            offset < word.start
        } else {
            // Another text block: above the press, or beside it to the left.
            y < word.at.1 - 8. || (y <= word.at.1 + 8. && x < word.at.0)
        };
        if node == word.root && (word.start..=word.end).contains(&offset) {
            self.doc
                .set_text_selection(word.root, word.start, word.root, word.end);
        } else if before {
            self.doc
                .set_text_selection(word.root, word.end, node, offset);
        } else {
            self.doc
                .set_text_selection(word.root, word.start, node, offset);
        }
    }
    /// The box of the element a link's `#fragment` names, (top, bottom) in
    /// document coordinates.
    pub(super) fn fragment_box(&self, fragment: &str) -> Option<(f32, f32)> {
        let fragment = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
        if fragment.is_empty() {
            return None;
        }
        let node = self.doc.get_node(self.doc.get_fragment_target(&fragment)?)?;
        let top = node.absolute_position(0., 0.).y;
        Some((top, top + node.final_layout().size.height))
    }
    /// The focused element (after Tab), (top, bottom) in document coordinates.
    pub(super) fn focused_box(&self) -> Option<(f32, f32)> {
        let node = self.doc.get_node(self.doc.get_focussed_node_id()?)?;
        let top = node.absolute_position(0., 0.).y;
        Some((top, top + node.final_layout().size.height.max(1.)))
    }
    pub(super) fn focused(&self) -> Option<NodeId> {
        self.doc.get_focussed_node_id()
    }
    /// The email's own scrollable boxes (`overflow: auto` or `scroll`)
    /// whose content overflows them.
    pub(super) fn scrollers(&self) -> Vec<Scroller> {
        use style::computed_values::overflow_x::T as Overflow;
        let scrolls = |o: Overflow| matches!(o, Overflow::Auto | Overflow::Scroll);
        let root = self.doc.root_element().id;
        let mut out = Vec::new();
        for (id, node) in self.doc.tree().iter() {
            if id == root || !node.is_element() {
                continue;
            }
            let Some(styles) = node.primary_styles() else {
                continue;
            };
            let (sx, sy) = (
                scrolls(styles.get_box().clone_overflow_x()),
                scrolls(styles.get_box().clone_overflow_y()),
            );
            drop(styles);
            if !sx && !sy {
                continue;
            }
            let layout = node.final_layout();
            let (max_x, max_y) = (layout.scroll_width(), layout.scroll_height());
            if !(sx && max_x > 0.5 || sy && max_y > 0.5) {
                continue;
            }
            let at = node.absolute_position(0., 0.);
            let scrolled = node.scroll_offset();
            out.push(Scroller {
                x: at.x + scrolled.x as f32,
                y: at.y + scrolled.y as f32,
                width: layout.size.width,
                height: layout.size.height,
                up: sy && scrolled.y > 0.5,
                down: sy && (scrolled.y as f32) < max_y - 0.5,
                left: sx && scrolled.x > 0.5,
                right: sx && (scrolled.x as f32) < max_x - 0.5,
            });
        }
        out
    }
    /// The cursor the author's CSS (or the content under the pointer) asks for.
    pub(super) fn cursor(&self) -> gpui::CursorStyle {
        use cursor_icon::CursorIcon as I;
        use gpui::CursorStyle as C;
        match self.doc.get_cursor() {
            Some(I::Pointer) => C::PointingHand,
            Some(I::Text) => C::IBeam,
            Some(I::VerticalText) => C::IBeamCursorForVerticalLayout,
            Some(I::Crosshair) => C::Crosshair,
            Some(I::Grab) => C::OpenHand,
            Some(I::Grabbing) => C::ClosedHand,
            Some(I::NotAllowed | I::NoDrop) => C::OperationNotAllowed,
            Some(I::ContextMenu) => C::ContextualMenu,
            Some(I::Alias) => C::DragLink,
            Some(I::Copy) => C::DragCopy,
            Some(I::EwResize | I::ColResize) => C::ResizeLeftRight,
            Some(I::NsResize | I::RowResize) => C::ResizeUpDown,
            _ => C::Arrow,
        }
    }
    /// The link and image under a point in document coordinates.
    pub(super) fn context_at(&self, x: f32, y: f32) -> ContextTarget {
        let mut target = ContextTarget::default();
        let Some(hit) = self.doc.hit(x, y) else {
            return target;
        };
        let mut next = self.doc.get_node(hit.node_id);
        while let Some(node) = next {
            if let Some(element) = node.element_data() {
                let tag: &str = &element.name.local;
                if target.link.is_none() && element.is_link() {
                    target.link = element.attr(blitz_dom::local_name!("href")).map(Into::into);
                }
                if target.image.is_none() {
                    // A blocked image shown as its alt text keeps its address.
                    let src = if tag == "img" {
                        element.attr(blitz_dom::local_name!("src"))
                    } else {
                        element.attr(LocalName::from("data-picacho-src"))
                    };
                    target.image = src.filter(|s| !s.starts_with("data:")).map(Into::into);
                }
            }
            next = node.parent.and_then(|id| self.doc.get_node(id));
        }
        // Only addresses that are safe to open: web pages and email links, and
        // for images only web pages. A `file:` or `smb:` source could open or
        // run a program from this computer or a network share.
        target.link = target.link.filter(|href| super::openable_url(href).is_some());
        target.image = target
            .image
            .filter(|src| url::Url::parse(src.trim()).is_ok_and(|u| resources::remote_url(&u)));
        target
    }
}
