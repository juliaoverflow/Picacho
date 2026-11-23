//! GPUI input, clipping and strip caching for an HTML document whose work
//! happens on its own thread (see `worker.rs`).
use super::document::{Scroller, Tone, page_color};
use super::worker::{self, Command, Output, Update, band_height};
use crate::manage::model::InlineParts;
use blitz_traits::events::*;
use futures::{StreamExt, channel::mpsc::UnboundedSender};
use gpui::{prelude::*, *};
use gpui_component::Sizable as _;
use std::{collections::HashMap, sync::Arc, sync::mpsc};

struct Tile {
    revision: u64,
    width: u32,
    scale: f32,
    /// Logical (CSS pixel) top of the strip in the email.
    top: f32,
    /// Logical size of the image, padded past the strip with transparency
    /// (see `worker::TILE`).
    size: Size<Pixels>,
    image: Arc<RenderImage>,
}

/// A secondary click in the email, with what it landed on.
pub struct HtmlContextMenu {
    pub position: gpui::Point<Pixels>,
    pub link: Option<String>,
    pub image: Option<String>,
    pub selection: Option<String>,
}

/// A link in the email: hovered (or no longer, `None`), or clicked. The
/// view opens nothing itself; its owner does, or asks first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtmlLink {
    Hovered(Option<String>),
    Opened(String),
}

/// Part of the email to bring into view, in window coordinates: a `#` link's
/// target (`start`: scroll it to the top) or the element Tab focused (just
/// into view). The view's owner scrolls (see `scroll_to_reveal`).
#[derive(Clone, Copy, Debug)]
pub struct HtmlReveal {
    pub top: Pixels,
    pub bottom: Pixels,
    pub start: bool,
}

/// Scrolls `handle`'s content to show `reveal`.
pub fn scroll_to_reveal(handle: &ScrollHandle, reveal: &HtmlReveal) {
    let view = handle.bounds();
    let offset = handle.offset();
    let max = handle.max_offset().y;
    let margin = px(16.);
    let by = if reveal.start || reveal.top < view.top() + margin {
        reveal.top - view.top() - margin
    } else if reveal.bottom > view.bottom() - margin {
        (reveal.bottom - view.bottom() + margin).min(reveal.top - view.top() - margin)
    } else {
        return;
    };
    let y = (offset.y - by).clamp(-max, px(0.));
    handle.set_offset(point(offset.x, y));
}

/// The whole email, scaled down, for the collapsed reader.
struct Thumbnail {
    revision: u64,
    image: Arc<RenderImage>,
    /// Logical size of the email in it.
    size: Size<Pixels>,
    /// Logical size of the image, padded with transparency.
    padded: Size<Pixels>,
}

pub struct HtmlView {
    source: String,
    /// Whether the renderer failed on `source`.
    failed: bool,
    /// The message parts `source` refers to by `cid:` address.
    inline: Arc<InlineParts>,
    blocked: bool,
    /// Tracking is taken out of the links (see `manage::tracking`).
    clean_links: bool,
    /// The reader's dark choice the current tone was chosen for.
    dark: bool,
    tone: Tone,
    thumbnail: Option<Thumbnail>,
    /// The thumbnail asked of the worker: (revision, width, max height, scale).
    thumbnail_requested: Option<(u64, u32, u32, u32)>,
    worker: Option<mpsc::Sender<Command>>,
    generation: u64,
    updates: UnboundedSender<Update>,
    /// The document's latest revision, and whether it has been laid out yet.
    revision: u64,
    laid_out: bool,
    height: f32,
    /// The layout last asked for (width, window height, scale bits), and the
    /// width the latest state used.
    width: u32,
    screen: (u32, u32),
    laid_width: u32,
    /// The email's page color (0xRRGGBB), and its scrollable boxes.
    page: u32,
    scrollers: Vec<Scroller>,
    bounds: Bounds<Pixels>,
    focus: FocusHandle,
    cursor: CursorStyle,
    selection: Option<String>,
    tiles: HashMap<u32, Tile>,
    /// Strips asked of the worker: band -> (revision, width, scale bits).
    requested: HashMap<u32, (u64, u32, u32)>,
    retired: Vec<Arc<RenderImage>>,
    context_at: Option<gpui::Point<Pixels>>,
    /// Where the current long press began, in document coordinates.
    pressed_at: Option<(f32, f32)>,
    /// The link under the pointer.
    hovered: Option<String>,
    /// The person's text zoom for emails (1: their own size).
    text_zoom: f32,
    /// What screen readers are given of the email (its painted strips say
    /// nothing): its text, and its links (text, address).
    accessible: Option<(SharedString, Vec<(String, String)>)>,
}
impl EventEmitter<HtmlContextMenu> for HtmlView {}
impl EventEmitter<HtmlLink> for HtmlView {}
impl EventEmitter<HtmlReveal> for HtmlView {}

impl HtmlView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (updates, mut rx) = futures::channel::mpsc::unbounded::<Update>();
        cx.spawn(async move |this, cx| {
            while let Some(update) = rx.next().await {
                // Apply everything already queued in one notify.
                let mut batch = vec![update];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| {
                        for update in batch {
                            this.apply(update, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            source: String::new(),
            failed: false,
            inline: Default::default(),
            blocked: true,
            clean_links: false,
            dark: false,
            tone: Tone::Light,
            thumbnail: None,
            thumbnail_requested: None,
            worker: None,
            generation: 0,
            updates,
            revision: 0,
            laid_out: false,
            height: 1.,
            width: 0,
            screen: (0, 0),
            laid_width: 0,
            page: page_color(Tone::Light),
            scrollers: Vec::new(),
            bounds: Bounds::default(),
            focus: cx.focus_handle(),
            cursor: CursorStyle::Arrow,
            selection: None,
            tiles: HashMap::new(),
            requested: HashMap::new(),
            retired: Vec::new(),
            context_at: None,
            pressed_at: None,
            hovered: None,
            text_zoom: 1.,
            accessible: None,
        }
    }
    /// Shows emails larger or smaller (WCAG 1.4.4), now and from now on.
    pub fn set_text_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(0.5, 3.);
        if (zoom - self.text_zoom).abs() < f32::EPSILON {
            return;
        }
        self.text_zoom = zoom;
        self.send(Command::TextZoom(zoom));
        cx.notify();
    }
    /// Says the pointer is over `link` now, if that changed.
    fn hover(&mut self, link: Option<String>, cx: &mut Context<Self>) {
        if self.hovered != link {
            self.hovered = link.clone();
            cx.emit(HtmlLink::Hovered(link));
        }
    }
    #[cfg(test)]
    pub fn text_zoom(&self) -> f32 {
        self.text_zoom
    }
    #[cfg(test)]
    pub fn has_document(&self) -> bool {
        self.worker.is_some()
    }
    /// Whether the document is still being parsed, laid out or first painted.
    pub fn is_loading(&self) -> bool {
        self.worker.is_some() && !self.failed && (!self.laid_out || self.tiles.is_empty())
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let had_document = self.worker.is_some();
        // Dropping the sender ends the thread and cancels its resources.
        self.worker = None;
        self.generation += 1;
        self.source = String::new();
        // The message's inline images can be large; don't keep them past it.
        self.inline = Default::default();
        self.failed = false;
        self.retired
            .extend(self.tiles.drain().map(|(_, tile)| tile.image));
        self.retired
            .extend(self.thumbnail.take().map(|thumbnail| thumbnail.image));
        self.thumbnail_requested = None;
        self.requested.clear();
        self.width = 0;
        self.screen = (0, 0);
        self.laid_width = 0;
        self.scrollers.clear();
        self.laid_out = false;
        self.selection = None;
        self.cursor = CursorStyle::Arrow;
        self.hover(None, cx);
        if had_document {
            cx.notify();
        }
    }
    /// Shows `html`, with remote content `blocked` or not, in light or `dark`
    /// colors, with tracking taken out of its links when `clean_links`. The
    /// same content again keeps the current document.
    /// `inline` holds the message's parts its HTML shows by `cid:` address;
    /// they arrive when the message has been read, which shows it again.
    pub fn set_content(
        &mut self,
        html: &str,
        inline: &Arc<InlineParts>,
        blocked: bool,
        dark: bool,
        clean_links: bool,
        cx: &mut Context<Self>,
    ) {
        // Called on every reader render: compare cheaply before scanning
        // the HTML for its color scheme.
        if self.worker.is_some()
            && self.dark == dark
            && self.blocked == blocked
            && self.clean_links == clean_links
            && self.source.len() == html.len()
            && (Arc::ptr_eq(&self.inline, inline) || self.inline == *inline)
            && self.source == html
        {
            return;
        }
        let tone = Tone::for_email(html, dark);
        self.dark = dark;
        self.clear(cx);
        self.accessible = None;
        self.source = html.into();
        self.inline = inline.clone();
        self.blocked = blocked;
        self.clean_links = clean_links;
        self.tone = tone;
        self.page = page_color(tone);
        self.height = 1.;
        let shown = if clean_links {
            crate::manage::tracking::clean_html_links(html)
        } else {
            html.into()
        };
        self.worker = Some(worker::spawn(
            shown.into_owned(),
            inline.clone(),
            blocked,
            tone,
            self.generation,
            self.updates.clone(),
        ));
        if (self.text_zoom - 1.).abs() > f32::EPSILON {
            self.send(Command::TextZoom(self.text_zoom));
        }
        cx.notify();
    }
    /// Takes images no longer shown out of the window's atlas. Strips and
    /// thumbnails are released as they are painted; a view that is not
    /// painted any more (the next message is plain text) is released here,
    /// from its owner's render: during a frame, so the previous frame, which
    /// may still show them, is never drawn again.
    pub fn release(&mut self, window: &mut Window) {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
    }
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.send(command);
        }
    }
    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        if update.generation != self.generation {
            return;
        }
        match update.output {
            Output::State {
                revision,
                width,
                height,
                cursor,
                selection,
                page,
                scrollers,
            } => {
                self.revision = revision;
                self.laid_width = width;
                self.laid_out = true;
                self.height = height;
                self.cursor = cursor;
                self.selection = selection;
                self.page = page;
                self.scrollers = scrollers;
                cx.notify();
            }
            Output::Reveal { top, bottom, start } => {
                cx.emit(HtmlReveal {
                    top: self.bounds.top() + px(top),
                    bottom: self.bounds.top() + px(bottom),
                    start,
                });
            }
            Output::Tile {
                band,
                revision,
                width,
                scale,
                size: (pw, ph),
                pixels,
            } => {
                let Some(buffer) = image::RgbaImage::from_raw(pw, ph, pixels) else {
                    return;
                };
                let tile = Tile {
                    revision,
                    width,
                    scale,
                    top: band as f32 * band_height(scale),
                    size: size(px(pw as f32 / scale), px(ph as f32 / scale)),
                    image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                };
                if let Some(old) = self.tiles.insert(band, tile) {
                    self.retired.push(old.image);
                }
                cx.notify();
            }
            Output::Thumbnail {
                revision,
                size: (pw, ph),
                content: (cw, ch),
                scale,
                pixels,
            } => {
                let Some(buffer) = image::RgbaImage::from_raw(pw, ph, pixels) else {
                    return;
                };
                let thumbnail = Thumbnail {
                    revision,
                    image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                    size: size(px(cw as f32 / scale), px(ch as f32 / scale)),
                    padded: size(px(pw as f32 / scale), px(ph as f32 / scale)),
                };
                if let Some(old) = self.thumbnail.replace(thumbnail) {
                    self.retired.push(old.image);
                }
                cx.notify();
            }
            Output::Hover(link) => self.hover(link, cx),
            Output::Open(url) => cx.emit(HtmlLink::Opened(url)),
            Output::Accessible { text, links } => {
                self.accessible = Some((text.into(), links));
                cx.notify();
            }
            Output::Failed => {
                self.failed = true;
                self.laid_out = true;
                self.height = 1.;
                cx.notify();
            }
            Output::Context(target) => {
                if let Some(position) = self.context_at.take() {
                    cx.emit(HtmlContextMenu {
                        position,
                        link: target.link,
                        image: target.image,
                        selection: self.selection.clone(),
                    });
                }
            }
        }
    }
    fn local(&self, position: gpui::Point<Pixels>) -> (f32, f32) {
        (
            f32::from(position.x - self.bounds.origin.x),
            f32::from(position.y - self.bounds.origin.y),
        )
    }
    fn pointer(
        &mut self,
        position: gpui::Point<Pixels>,
        kind: u8,
        pressed: bool,
        modifiers: Modifiers,
    ) {
        if self.worker.is_none() || !self.laid_out {
            return;
        }
        let (x, y) = self.local(position);
        let event = BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                page_x: x,
                page_y: y,
                client_x: x,
                client_y: y,
                screen_x: x,
                screen_y: y,
            },
            button: MouseEventButton::Main,
            buttons: if pressed {
                MouseEventButtons::Primary
            } else {
                MouseEventButtons::None
            },
            mods: blitz_modifiers(modifiers),
            details: Default::default(),
            element: Default::default(),
            active_pointers: Default::default(),
        };
        self.send(Command::Pointer(match kind {
            1 => UiEvent::PointerDown(event),
            2 => UiEvent::PointerUp(event),
            3 => UiEvent::PointerLeave(event),
            _ => UiEvent::PointerMove(event),
        }));
    }
    /// A long press on a touch screen, where there is no drag to select:
    /// it selects the word under the finger, moving the finger before
    /// lifting it extends the selection, and lifting it offers the menu a
    /// secondary click does (Copy, and the link or image pressed).
    /// `visible` is the part of the email on screen.
    fn long_press(
        &mut self,
        e: &LongPressEvent,
        visible: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity();
        match e.phase {
            TouchPhase::Started => {
                if window.default_prevented()
                    || self.worker.is_none()
                    || !self.laid_out
                    || !visible.contains(&e.start_position)
                {
                    return;
                }
                window.prevent_default();
                window.capture_long_press(&entity);
                cx.stop_propagation();
                let (x, y) = self.local(e.start_position);
                self.pressed_at = Some((x, y));
                self.send(Command::SelectWord { x, y });
            }
            _ if !window.has_long_press_capture(&entity) => {}
            TouchPhase::Moved => {
                cx.stop_propagation();
                let (x, y) = self.local(e.position);
                self.send(Command::ExtendWord { x, y });
            }
            TouchPhase::Ended => {
                cx.stop_propagation();
                if let Some((x, y)) = self.pressed_at.take() {
                    self.context_at = Some(e.position);
                    self.send(Command::Context { x, y });
                }
            }
            TouchPhase::Cancelled => {
                cx.stop_propagation();
                self.pressed_at = None;
            }
        }
    }
    /// Lays out for the frame's width and picks the strips to draw, asking the
    /// worker for any that are missing or out of date.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Vec<(Bounds<Pixels>, Arc<RenderImage>)> {
        self.release(window);
        self.bounds = bounds;
        if self.worker.is_none() {
            return Vec::new();
        }
        let width = f32::from(bounds.size.width).ceil().max(1.) as u32;
        let screen = (
            f32::from(window.viewport_size().height).ceil().max(1.) as u32,
            window.scale_factor().to_bits(),
        );
        if self.width != width || self.screen != screen {
            self.width = width;
            self.screen = screen;
            self.send(Command::Layout {
                width,
                height: screen.0,
                scale: window.scale_factor(),
            });
        }
        if !self.laid_out {
            return Vec::new();
        }
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
            return Vec::new();
        }
        let scale = window.scale_factor();
        let top = f32::from(visible.top() - bounds.top()).max(0.);
        let bottom = top + f32::from(visible.size.height);
        let strip = band_height(scale);
        let first = (top / strip).floor() as u32;
        // Prefetch one strip below so scrolling down shows finished content.
        let last = ((bottom / strip).ceil() as u32)
            .min((self.height / strip).ceil() as u32)
            .max(first + 1);
        let current = self.laid_width == width;
        let mut drawn = Vec::new();
        for band in first..=last {
            let tile = self.tiles.get(&band);
            let fresh = tile.is_some_and(|t| {
                t.revision == self.revision && t.width == width && t.scale == scale
            });
            let key = (self.revision, width, scale.to_bits());
            if !fresh && current && self.requested.get(&band) != Some(&key) {
                self.requested.insert(band, key);
                self.send(Command::Paint { band, width, scale });
            }
            // An older strip stays up until its replacement arrives.
            // Its transparent padding reaches past the email, showing nothing.
            if let Some(tile) = tile {
                drawn.push((
                    Bounds::new(point(bounds.left(), bounds.top() + px(tile.top)), tile.size),
                    tile.image.clone(),
                ));
            }
        }
        // Keep a few strips around the viewport; drop the rest.
        let keep = first.saturating_sub(2)..=last + 2;
        let far: Vec<u32> = self
            .tiles
            .keys()
            .copied()
            .filter(|b| !keep.contains(b))
            .collect();
        for band in far {
            if let Some(tile) = self.tiles.remove(&band) {
                self.retired.push(tile.image);
            }
            self.requested.remove(&band);
        }
        drawn
    }
}

impl HtmlView {
    /// The email's page color, for whatever frames it.
    pub fn page_color(&self) -> u32 {
        self.page
    }
    /// Whether a scroll wheel at `position` scrolls one of the email's own
    /// boxes, which can still scroll that way, rather than the reader.
    fn scrolls_inside(&self, position: gpui::Point<Pixels>, delta: gpui::Point<Pixels>) -> bool {
        let (x, y) = self.local(position);
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        self.scrollers.iter().rev().any(|s| {
            x >= s.x
                && x < s.x + s.width
                && y >= s.y
                && y < s.y + s.height
                && ((dy > 0. && s.up)
                    || (dy < 0. && s.down)
                    || (dx > 0. && s.left)
                    || (dx < 0. && s.right))
        })
    }
    /// The whole email scaled to the width of `bounds`, cropped to its height.
    fn prepare_thumbnail(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        self.release(window);
        self.worker.as_ref()?;
        let scale = window.scale_factor();
        let width = f32::from(bounds.size.width);
        let max_height = f32::from(bounds.size.height);
        let key = (
            self.revision,
            width.to_bits(),
            max_height.to_bits(),
            scale.to_bits(),
        );
        let fresh = self.thumbnail.as_ref().is_some_and(|t| {
            t.revision == self.revision && (f32::from(t.size.width) - width).abs() < 0.5
        });
        if !fresh && self.thumbnail_requested != Some(key) {
            self.thumbnail_requested = Some(key);
            self.send(Command::Thumbnail {
                width,
                max_height,
                scale,
            });
        }
        let thumbnail = self.thumbnail.as_ref()?;
        // The email fills at most `bounds`' height; the transparent padding
        // past it shows nothing.
        Some((
            Bounds::new(bounds.origin, thumbnail.padded),
            thumbnail.image.clone(),
        ))
    }
    /// The logical height of the current thumbnail, once painted.
    pub fn thumbnail_height(&self) -> Option<f32> {
        self.thumbnail.as_ref().map(|t| f32::from(t.size.height))
    }
}

/// The most links a screen reader is offered of one email: a newsletter's
/// footer can hold hundreds, each a node in every frame's tree.
const MAX_ACCESSIBLE_LINKS: usize = 200;

/// The email's text for screen readers (see `EmailDocument::accessible_text`,
/// a line a block) as its paragraphs: its non-blank lines.
pub(super) fn accessible_paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

/// The email's paragraphs for screen readers, one text node each, so a
/// reader moves through the email a paragraph at a time. Drawn nowhere.
fn accessible_text(text: SharedString) -> impl IntoElement {
    div()
        .id("html-email-text")
        .absolute()
        .size_0()
        .overflow_hidden()
        .role(gpui::accesskit::Role::Group)
        .a11y_synthetic_children(move |builder| {
            for (i, paragraph) in accessible_paragraphs(&text).enumerate() {
                let mut node = gpui::accesskit::Node::new(gpui::accesskit::Role::Label);
                node.set_value(paragraph);
                builder.push_child(builder.synthetic_node_id(i), node);
            }
        })
}

/// The email's links for screen readers (text, address), which open when
/// activated. Drawn nowhere.
fn accessible_links(
    links: Vec<(String, String)>,
    opener: WeakEntity<HtmlView>,
) -> impl IntoElement {
    let links = links.into_iter().take(MAX_ACCESSIBLE_LINKS).enumerate();
    div()
        .id("html-email-links")
        .absolute()
        .size_0()
        .overflow_hidden()
        .role(gpui::accesskit::Role::List)
        .aria_label("Links in this message")
        .children(links.map(move |(i, (label, href))| {
            let opener = opener.clone();
            // AccessKit's Click, handled here: GPUI's own presses the middle
            // of the node's bounds, which for this undrawn node is the
            // email's top left, not the link.
            div()
                .id(("html-email-link", i))
                .role(gpui::accesskit::Role::Link)
                .aria_label(label)
                .aria_description(href.clone())
                .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                    let _ = opener.update(cx, |_, cx| cx.emit(HtmlLink::Opened(href.clone())));
                })
        }))
}

/// GPUI's modifier keys as Blitz's.
fn blitz_modifiers(m: Modifiers) -> keyboard_types::Modifiers {
    let mut out = keyboard_types::Modifiers::empty();
    out.set(keyboard_types::Modifiers::SHIFT, m.shift);
    out.set(keyboard_types::Modifiers::CONTROL, m.control);
    out.set(keyboard_types::Modifiers::ALT, m.alt);
    out.set(keyboard_types::Modifiers::META, m.platform);
    out
}

/// Paints `view`'s email scaled down to fit this element's width.
pub fn thumbnail(view: Entity<HtmlView>) -> impl IntoElement {
    canvas(
        move |bounds, w, cx| view.update(cx, |this, _| this.prepare_thumbnail(bounds, w)),
        |_, image, w, _| {
            if let Some((bounds, image)) = image {
                let _ = w.paint_image(bounds, bounds, Corners::default(), image, 0, false);
            }
        },
    )
    .size_full()
}

impl Render for HtmlView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.failed {
            return div()
                .id("html-email")
                .debug_selector(|| "html-failed".into())
                .w_full()
                .py_6()
                .text_size(px(13.))
                .text_color(rgb(0x9aa0a6))
                .child("This email's formatting couldn't be shown. Choose plain text above to read it.")
                .into_any_element();
        }
        let view = cx.entity();
        let pressed = view.clone();
        let loading = self.is_loading();
        div()
            .id("html-email")
            .debug_selector(|| "html-email".into())
            .relative()
            .w_full()
            .h(px(if self.laid_out { self.height } else { 160. }))
            .flex_shrink_0()
            .bg(rgb(self.page))
            .cursor(self.cursor)
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, w, cx| {
                    this.focus.focus(w, cx);
                    this.pointer(e.position, 1, true, e.modifiers);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, _| {
                    this.pointer(e.position, 2, false, e.modifiers)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    if this.worker.is_some() && this.laid_out {
                        let (x, y) = this.local(e.position);
                        this.context_at = Some(e.position);
                        this.send(Command::Context { x, y });
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                this.pointer(
                    e.position,
                    0,
                    e.pressed_button == Some(MouseButton::Left),
                    e.modifiers,
                )
            }))
            .on_scroll(cx.listener(|this, e: &ScrollEvent, w, cx| {
                let delta = e.delta.pixel_delta(w.line_height());
                if this.worker.is_some() && this.laid_out && this.scrolls_inside(e.position, delta) {
                    let (x, y) = this.local(e.position);
                    this.send(Command::Wheel {
                        x,
                        y,
                        dx: f32::from(delta.x),
                        dy: f32::from(delta.y),
                    });
                    cx.stop_propagation();
                }
            }))
            .on_hover(cx.listener(|this, hovered, w, cx| {
                if !hovered {
                    this.pointer(w.mouse_position(), 3, false, Modifiers::default());
                    this.hover(None, cx);
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, w, cx| {
                // A way out for keyboard users (WCAG 2.1.2): Escape leaves
                // the message for the next control, as Tab would; Tab and
                // Shift-Tab then move among the message's links again.
                if e.keystroke.key == "escape" {
                    if e.keystroke.modifiers.shift {
                        w.focus_prev(cx);
                    } else {
                        w.focus_next(cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let command = if cfg!(target_os = "macos") {
                    e.keystroke.modifiers.platform
                } else {
                    e.keystroke.modifiers.control
                };
                if command && e.keystroke.key == "c" {
                    if let Some(text) = this.selection.clone() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                } else if command && e.keystroke.key == "a" {
                    this.send(Command::SelectAll);
                    cx.stop_propagation();
                } else if matches!(e.keystroke.key.as_str(), "tab" | "enter" | "space") {
                    let key = match e.keystroke.key.as_str() {
                        "tab" => keyboard_types::Key::Named(keyboard_types::NamedKey::Tab),
                        "enter" => keyboard_types::Key::Named(keyboard_types::NamedKey::Enter),
                        _ => keyboard_types::Key::Character(" ".into()),
                    };
                    this.send(Command::Key(BlitzKeyEvent {
                        key,
                        code: keyboard_types::Code::Unidentified,
                        modifiers: if e.keystroke.modifiers.shift {
                            keyboard_types::Modifiers::SHIFT
                        } else {
                            keyboard_types::Modifiers::empty()
                        },
                        location: keyboard_types::Location::Standard,
                        is_auto_repeating: e.is_held,
                        is_composing: false,
                        state: KeyState::Pressed,
                        text: None,
                    }));
                    cx.stop_propagation();
                }
            }))
            // For screen readers: the email as a document of its paragraphs,
            // then its links, which open when activated (WCAG 1.1.1, 1.3.1,
            // 2.1.1). Both are drawn nowhere: the painted strips are what
            // sighted readers see.
            .role(gpui::accesskit::Role::Document)
            .aria_label("Message")
            .when_some(self.accessible.clone(), |d, (text, links)| {
                let opener = cx.entity().downgrade();
                d.child(accessible_text(text))
                    .child(accessible_links(links, opener))
            })
            .child(
                canvas(
                    move |bounds, w, cx| view.update(cx, |this, _| this.prepare(bounds, w)),
                    move |bounds, tiles, w, _| {
                        for (bounds, image) in tiles {
                            let _ = w.paint_image(
                                bounds,
                                bounds,
                                Corners::default(),
                                image,
                                0,
                                false,
                            );
                        }
                        let visible = bounds.intersect(&w.content_mask().bounds);
                        let pressed = pressed.clone();
                        w.on_mouse_event(move |e: &LongPressEvent, phase, w, cx| {
                            if phase == DispatchPhase::Bubble {
                                pressed.update(cx, |this, cx| this.long_press(e, visible, w, cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
            .when(loading, |d| {
                d.child(
                    div()
                        .id("html-loading")
                        .debug_selector(|| "html-loading".into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(160.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            gpui_component::spinner::Spinner::new()
                                .color(rgb(0x9aa0a6).into())
                                .with_size(gpui_component::Size::Large),
                        ),
                )
            })
            .into_any_element()
    }
}
