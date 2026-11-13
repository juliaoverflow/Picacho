//! GPUI input, clipping and strip caching for an HTML document whose work
//! happens on its own thread (see `worker.rs`).
use super::document::{Tone, page_color};
use super::worker::{self, BAND, Command, Output, Update};
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
    /// Logical (CSS pixel) height of the strip.
    rows: f32,
    image: Arc<RenderImage>,
}

/// A secondary click in the email, with what it landed on.
pub struct HtmlContextMenu {
    pub position: gpui::Point<Pixels>,
    pub link: Option<String>,
    pub image: Option<String>,
    pub selection: Option<String>,
}

/// The whole email, scaled down, for the collapsed reader.
struct Thumbnail {
    revision: u64,
    image: Arc<RenderImage>,
    /// Logical size.
    size: Size<Pixels>,
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
    /// The width last sent for layout, and the one the latest state used.
    width: u32,
    laid_width: u32,
    bounds: Bounds<Pixels>,
    focus: FocusHandle,
    cursor: CursorStyle,
    selection: Option<String>,
    tiles: HashMap<u32, Tile>,
    /// Strips asked of the worker: band -> (revision, width, scale bits).
    requested: HashMap<u32, (u64, u32, u32)>,
    retired: Vec<Arc<RenderImage>>,
    context_at: Option<gpui::Point<Pixels>>,
}
impl EventEmitter<HtmlContextMenu> for HtmlView {}

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
            laid_width: 0,
            bounds: Bounds::default(),
            focus: cx.focus_handle(),
            cursor: CursorStyle::Arrow,
            selection: None,
            tiles: HashMap::new(),
            requested: HashMap::new(),
            retired: Vec::new(),
            context_at: None,
        }
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
        self.source.clear();
        self.failed = false;
        self.retired
            .extend(self.tiles.drain().map(|(_, tile)| tile.image));
        self.retired
            .extend(self.thumbnail.take().map(|thumbnail| thumbnail.image));
        self.thumbnail_requested = None;
        self.requested.clear();
        self.width = 0;
        self.laid_width = 0;
        self.laid_out = false;
        self.selection = None;
        self.cursor = CursorStyle::Arrow;
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
        self.source = html.into();
        self.inline = inline.clone();
        self.blocked = blocked;
        self.clean_links = clean_links;
        self.tone = tone;
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
        cx.notify();
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
            } => {
                self.revision = revision;
                self.laid_width = width;
                self.laid_out = true;
                self.height = height;
                self.cursor = cursor;
                self.selection = selection;
                cx.notify();
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
                    rows: ph as f32 / scale,
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
                pixels,
            } => {
                let Some(buffer) = image::RgbaImage::from_raw(pw, ph, pixels) else {
                    return;
                };
                let scale = self
                    .thumbnail_requested
                    .map_or(1., |(_, _, _, scale)| f32::from_bits(scale));
                let thumbnail = Thumbnail {
                    revision,
                    image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
                    size: size(px(pw as f32 / scale), px(ph as f32 / scale)),
                };
                if let Some(old) = self.thumbnail.replace(thumbnail) {
                    self.retired.push(old.image);
                }
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
    fn pointer(&mut self, position: gpui::Point<Pixels>, kind: u8, pressed: bool) {
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
            mods: Default::default(),
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
    /// Lays out for the frame's width and picks the strips to draw, asking the
    /// worker for any that are missing or out of date.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Vec<(Bounds<Pixels>, Arc<RenderImage>)> {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.bounds = bounds;
        if self.worker.is_none() {
            return Vec::new();
        }
        let width = f32::from(bounds.size.width).ceil().max(1.) as u32;
        if self.width != width {
            self.width = width;
            self.send(Command::Layout(width));
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
        let first = (top / BAND).floor() as u32;
        // Prefetch one strip below so scrolling down shows finished content.
        let last = ((bottom / BAND).ceil() as u32)
            .min((self.height / BAND).ceil() as u32)
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
            if let Some(tile) = tile {
                drawn.push((
                    Bounds::new(
                        point(bounds.left(), bounds.top() + px(band as f32 * BAND)),
                        size(px(tile.width as f32), px(tile.rows)),
                    ),
                    tile.image.clone(),
                ));
            }
        }
        // Keep a few strips around the viewport; drop the rest.
        let keep = first.saturating_sub(3)..=last + 3;
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
        page_color(self.tone)
    }
    /// The whole email scaled to the width of `bounds`, cropped to its height.
    fn prepare_thumbnail(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) -> Option<(Bounds<Pixels>, Arc<RenderImage>)> {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
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
        Some((
            Bounds::new(
                bounds.origin,
                size(
                    thumbnail.size.width,
                    thumbnail.size.height.min(bounds.size.height),
                ),
            ),
            thumbnail.image.clone(),
        ))
    }
    /// The logical height of the current thumbnail, once painted.
    pub fn thumbnail_height(&self) -> Option<f32> {
        self.thumbnail.as_ref().map(|t| f32::from(t.size.height))
    }
}

/// Paints `view`'s email scaled down to fit this element's width.
pub fn thumbnail(view: Entity<HtmlView>) -> impl IntoElement {
    canvas(
        move |bounds, w, cx| view.update(cx, |this, _| this.prepare_thumbnail(bounds, w)),
        |_, image, w, _| {
            if let Some((bounds, image)) = image {
                let _ = w.paint_image(bounds, Corners::default(), image, 0, false);
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
        let loading = self.is_loading();
        div()
            .id("html-email")
            .debug_selector(|| "html-email".into())
            .relative()
            .w_full()
            .h(px(if self.laid_out { self.height } else { 160. }))
            .flex_shrink_0()
            .bg(rgb(page_color(self.tone)))
            .cursor(self.cursor)
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, w, _| {
                    this.focus.focus(w);
                    this.pointer(e.position, 1, true);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, _| this.pointer(e.position, 2, false)),
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
                this.pointer(e.position, 0, e.pressed_button == Some(MouseButton::Left))
            }))
            .on_hover(cx.listener(|this, hovered, w, _| {
                if !hovered {
                    this.pointer(w.mouse_position(), 3, false);
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if (if cfg!(target_os = "macos") {
                    e.keystroke.modifiers.platform
                } else {
                    e.keystroke.modifiers.control
                }) && e.keystroke.key == "c"
                {
                    if let Some(text) = this.selection.clone() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
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
            .child(
                canvas(
                    move |bounds, w, cx| view.update(cx, |this, _| this.prepare(bounds, w)),
                    |_, tiles, w, _| {
                        for (bounds, image) in tiles {
                            let _ = w.paint_image(bounds, Corners::default(), image, 0, false);
                        }
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
