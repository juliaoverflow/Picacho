//! Each document lives on its own thread. Parsing, styling, layout, input and
//! rasterization never run on the UI thread, so opening a heavy newsletter,
//! hovering links or scrolling never stalls the window.
use super::document::{ContextTarget, EmailDocument, Scroller, Tone};
use crate::manage::model::InlineParts;
use blitz_dom::Document;
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzWheelDelta, BlitzWheelEvent, MouseEventButtons,
    PointerCoords, UiEvent,
};
use futures::channel::mpsc::UnboundedSender;
use std::sync::Arc;
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

/// The side, in device pixels, of the textures GPUI's image atlas makes.
///
/// GPUI's atlas never reuses the space of an image dropped from a texture;
/// it frees a texture only once every image in it is gone. Strips and
/// thumbnails sharing a texture with images kept for good (avatars, emoji)
/// would leave it growing by their size with every email opened. So every
/// image here is at least this big both ways: it gets a texture of its own,
/// which it fills, and dropping it frees the texture.
pub const TILE: u32 = 1024;

/// The height of one cached raster strip, in CSS pixels: one texture tall.
pub fn band_height(scale: f32) -> f32 {
    TILE as f32 / scale
}

/// An image side in device pixels as uploaded: at least a texture's side,
/// and rounded up so that sizes repeat and GPUI's upload buffers are reused
/// rather than one kept for each new size.
pub fn padded(side: u32) -> u32 {
    side.max(TILE).next_multiple_of(256)
}

/// `pixels` (BGRA, `width` by `height`) at the top left of a transparent
/// image of the padded size.
fn pad(width: u32, height: u32, pixels: Vec<u8>) -> ((u32, u32), Vec<u8>) {
    let size = (padded(width), padded(height));
    if size == (width, height) {
        return (size, pixels);
    }
    let row = width as usize * 4;
    let stride = size.0 as usize * 4;
    let mut out = vec![0; stride * size.1 as usize];
    for (y, line) in pixels.chunks_exact(row).take(height as usize).enumerate() {
        out[y * stride..y * stride + row].copy_from_slice(line);
    }
    (size, out)
}

/// Coordinates in commands and outputs are the view's: logical pixels from
/// the email's top left. They are the document's own unless the email is
/// shown scaled down to fit (see `EmailDocument::layout`).
pub enum Command {
    /// Lay the document out for this many CSS pixels of width, in a window
    /// this tall at this scale (for `vh` units and media queries).
    Layout {
        width: u32,
        height: u32,
        scale: f32,
    },
    Pointer(UiEvent),
    Key(BlitzKeyEvent),
    /// Select all of the email's text.
    SelectAll,
    /// A scroll wheel over one of the email's scrollable boxes, by this many
    /// pixels.
    Wheel {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    /// Rasterize one strip for the current layout.
    Paint {
        band: u32,
        width: u32,
        scale: f32,
    },
    /// Rasterize the whole document scaled to `width` logical pixels wide
    /// and at most `max_height` tall, at the window's `scale`.
    Thumbnail {
        width: f32,
        max_height: f32,
        scale: f32,
    },
    /// A long press: select the word at this point.
    SelectWord {
        x: f32,
        y: f32,
    },
    /// The finger still down after a long press moved here: extend its
    /// selection.
    ExtendWord {
        x: f32,
        y: f32,
    },
    /// Report what is under a secondary click.
    Context {
        x: f32,
        y: f32,
    },
}

pub struct Update {
    pub generation: u64,
    pub output: Output,
}

pub enum Output {
    /// The document changed: its new revision, height, cursor, selection,
    /// page color (0xRRGGBB) and scrollable boxes.
    State {
        revision: u64,
        width: u32,
        height: f32,
        cursor: gpui::CursorStyle,
        selection: Option<String>,
        page: u32,
        scrollers: Vec<Scroller>,
    },
    /// Bring this part of the email into view: the target of a `#` link
    /// (`start`: to the top), or the element Tab focused (just into view).
    Reveal {
        top: f32,
        bottom: f32,
        start: bool,
    },
    Tile {
        band: u32,
        revision: u64,
        width: u32,
        scale: f32,
        /// Device pixels, padded (see `TILE`).
        size: (u32, u32),
        pixels: Vec<u8>,
    },
    Context(ContextTarget),
    /// The pointer is over this link now (or over none).
    Hover(Option<String>),
    /// A link was clicked: this address, for the view's owner to open.
    Open(String),
    /// The renderer failed on this email; nothing more will come.
    Failed,
    Thumbnail {
        revision: u64,
        /// Device pixels, padded (see `TILE`), and those the email fills.
        size: (u32, u32),
        content: (u32, u32),
        /// The window scale it was painted at.
        scale: f32,
        pixels: Vec<u8>,
    },
}

/// A pointer event's coordinates in the document's own pixels.
fn unzoom(event: UiEvent, zoom: f32) -> UiEvent {
    if zoom == 1. {
        return event;
    }
    let scaled = |mut e: BlitzPointerEvent| {
        let c = e.coords;
        e.coords = PointerCoords {
            page_x: c.page_x / zoom,
            page_y: c.page_y / zoom,
            client_x: c.client_x / zoom,
            client_y: c.client_y / zoom,
            screen_x: c.screen_x / zoom,
            screen_y: c.screen_y / zoom,
        };
        e
    };
    match event {
        UiEvent::PointerMove(e) => UiEvent::PointerMove(scaled(e)),
        UiEvent::PointerUp(e) => UiEvent::PointerUp(scaled(e)),
        UiEvent::PointerDown(e) => UiEvent::PointerDown(scaled(e)),
        UiEvent::PointerCancel(e) => UiEvent::PointerCancel(scaled(e)),
        UiEvent::PointerLeave(e) => UiEvent::PointerLeave(scaled(e)),
        other => other,
    }
}

/// Successive presses counted as one double or triple click.
struct Clicks {
    at: Instant,
    x: f32,
    y: f32,
    count: u32,
}
impl Clicks {
    fn press(last: &mut Option<Clicks>, x: f32, y: f32) -> u32 {
        let count = match last {
            Some(c)
                if c.at.elapsed() < Duration::from_millis(500)
                    && (c.x - x).abs() <= 4.
                    && (c.y - y).abs() <= 4. =>
            {
                c.count + 1
            }
            _ => 1,
        };
        *last = Some(Clicks {
            at: Instant::now(),
            x,
            y,
            count,
        });
        count
    }
}

/// The width an email is laid out at before the reader has measured it,
/// for a thumbnail: most email templates are designed for 600 pixels.
const THUMBNAIL_LAYOUT: u32 = 640;

/// Starts a document thread. Dropping the returned sender ends it, which
/// drops the document and cancels its resource work.
pub fn spawn(
    html: String,
    inline: Arc<InlineParts>,
    blocked: bool,
    tone: Tone,
    generation: u64,
    out: UnboundedSender<Update>,
) -> mpsc::Sender<Command> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("picacho-html".into())
        .spawn(move || {
            // A renderer panic on some email must not leave its view waiting
            // forever: say so, and the reader offers the plain text.
            let failed = out.clone();
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(html, inline, blocked, tone, generation, rx, out)
            }));
            if ran.is_err() {
                let _ = failed.unbounded_send(Update {
                    generation,
                    output: Output::Failed,
                });
            }
        });
    tx
}

fn run(
    html: String,
    inline: Arc<InlineParts>,
    blocked: bool,
    tone: Tone,
    generation: u64,
    rx: mpsc::Receiver<Command>,
    out: UnboundedSender<Update>,
) {
    let send = |output| out.unbounded_send(Update { generation, output }).is_ok();
    let mut doc = EmailDocument::with_parts(&html, inline.clone(), blocked, tone);
    // Kept until the first layout shows whether the email's own dark styles
    // darken it (see `EmailDocument::mostly_light`).
    let mut source = (tone == Tone::NativeDark).then_some(html);
    let mut width = 0u32;
    let mut screen = (600u32, 1f32);
    // In the document's own pixels (see `EmailDocument::zoom`).
    let mut height = 1f32;
    let mut revision = 0u64;
    let mut selection = None;
    let mut word = None;
    let mut hovered: Option<String> = None;
    let mut clicks: Option<Clicks> = None;
    loop {
        let first = match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        // Coalesce everything queued: a burst of pointer moves resolves once.
        let commands: Vec<Command> = first.into_iter().chain(rx.try_iter()).collect();
        let mut relayout = doc.changed.swap(false, Ordering::Acquire) && width > 0;
        let mut restyle = false;
        let mut selection_changed = false;
        let mut paints = BTreeMap::new();
        let mut contexts = Vec::new();
        let mut thumbnail = None;
        let mut pointed = false;
        // The pointer left the email: over no link, whatever blitz keeps.
        let mut left = false;
        // A `#` link clicked, and whether Tab moved the focus.
        let mut fragment = None;
        let mut focus_moved = false;
        for command in commands {
            let zoom = doc.zoom();
            match command {
                Command::Layout {
                    width: w,
                    height: h,
                    scale,
                } => {
                    relayout |= w != width || (h, scale) != screen;
                    width = w;
                    screen = (h, scale);
                    doc.set_screen(h, scale);
                }
                Command::Pointer(event) => {
                    let event = unzoom(event, zoom);
                    pointed = true;
                    left = matches!(event, UiEvent::PointerLeave(_));
                    let interactive = !matches!(event, UiEvent::PointerMove(_))
                        || matches!(&event, UiEvent::PointerMove(e) if e.buttons != MouseEventButtons::None);
                    let mut presses = 0;
                    if let UiEvent::PointerDown(e) = &event {
                        let (x, y) = (e.coords.page_x, e.coords.page_y);
                        // Shift-click extends the selection.
                        if e.mods.contains(keyboard_types::Modifiers::SHIFT)
                            && doc.extend_selection_to(x, y)
                        {
                            selection_changed = true;
                            restyle = true;
                            continue;
                        }
                        presses = Clicks::press(&mut clicks, x, y);
                    }
                    if matches!(event, UiEvent::PointerUp(_)) {
                        fragment = doc
                            .hovered_href()
                            .and_then(|href| href.trim().strip_prefix('#').map(String::from))
                            .or(fragment);
                    }
                    let at = match &event {
                        UiEvent::PointerDown(e) => Some((e.coords.page_x, e.coords.page_y)),
                        _ => None,
                    };
                    let hover = doc.doc.get_hover_node_id();
                    let cursor = doc.cursor();
                    doc.doc.handle_ui_event(event);
                    // Blitz scrolls its viewport to a `#` link's target; the
                    // reader scrolls instead (`Output::Reveal`).
                    doc.doc
                        .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
                    if let Some((x, y)) = at {
                        match presses {
                            2 => word = doc.select_word_at(x, y),
                            3.. => doc.select_block_at(x, y),
                            _ => {}
                        }
                    }
                    restyle |= interactive
                        || hover != doc.doc.get_hover_node_id()
                        || cursor != doc.cursor();
                    selection_changed |= interactive;
                }
                Command::Key(event) => {
                    let focused = doc.focused();
                    doc.doc.handle_ui_event(UiEvent::KeyDown(event.clone()));
                    doc.doc.handle_ui_event(UiEvent::KeyUp(BlitzKeyEvent {
                        state: blitz_traits::events::KeyState::Released,
                        ..event
                    }));
                    doc.doc
                        .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
                    focus_moved |= doc.focused() != focused;
                    // Enter on a `<summary>` opens its details: a restyle
                    // lays out what changed, without a new viewport.
                    restyle = true;
                }
                Command::SelectAll => {
                    doc.select_all();
                    selection_changed = true;
                    restyle = true;
                }
                Command::Wheel { x, y, dx, dy } => {
                    let coords = PointerCoords {
                        page_x: x / zoom,
                        page_y: y / zoom,
                        client_x: x / zoom,
                        client_y: y / zoom,
                        screen_x: x / zoom,
                        screen_y: y / zoom,
                    };
                    doc.doc.handle_ui_event(UiEvent::Wheel(BlitzWheelEvent {
                        delta: BlitzWheelDelta::Pixels((dx / zoom) as f64, (dy / zoom) as f64),
                        coords,
                        buttons: MouseEventButtons::None,
                        mods: Default::default(),
                        element: Default::default(),
                    }));
                    // A box scrolled to its end passes the rest to the
                    // viewport, which is not scrolled here.
                    doc.doc
                        .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
                    restyle = true;
                }
                Command::SelectWord { x, y } => {
                    word = doc.select_word_at(x / zoom, y / zoom);
                    // Repaint even when the same text is selected elsewhere.
                    selection_changed = true;
                    restyle = true;
                }
                Command::ExtendWord { x, y } => {
                    if let Some(word) = word {
                        doc.extend_word_to(word, x / zoom, y / zoom);
                        selection_changed = true;
                        restyle = true;
                    }
                }
                Command::Paint { band, width, scale } => {
                    paints.insert(band, (width, scale));
                }
                Command::Context { x, y } => contexts.push((x / zoom, y / zoom)),
                Command::Thumbnail {
                    width: w,
                    max_height,
                    scale,
                } => {
                    // A collapsed reader has not measured the email yet.
                    if width == 0 {
                        width = THUMBNAIL_LAYOUT;
                        relayout = true;
                    }
                    thumbnail = Some((w, max_height, scale));
                }
            }
        }
        if relayout {
            height = doc.layout(width);
            // An email whose dark styles leave it light is read inverted.
            if let Some(html) = source.take()
                && doc.mostly_light()
            {
                doc = EmailDocument::with_parts(&html, inline.clone(), blocked, Tone::InvertedDark);
                doc.set_screen(screen.0, screen.1);
                height = doc.layout(width);
            }
        } else if restyle && width > 0 {
            doc.resolve();
            height = doc.content_height();
        }
        let zoom = doc.zoom();
        if selection_changed {
            let now = doc.doc.get_selected_text().filter(|s| !s.is_empty());
            selection_changed = now != selection;
            selection = now;
        }
        if relayout || restyle || selection_changed {
            revision += 1;
            let scrollers = doc
                .scrollers()
                .into_iter()
                .map(|s| Scroller {
                    x: s.x * zoom,
                    y: s.y * zoom,
                    width: s.width * zoom,
                    height: s.height * zoom,
                    ..s
                })
                .collect();
            if !send(Output::State {
                revision,
                width,
                height: height * zoom,
                cursor: doc.cursor(),
                selection: selection.clone(),
                page: doc.page_color(),
                scrollers,
            }) {
                return;
            }
        }
        let reveal = fragment
            .and_then(|f| doc.fragment_box(&f).map(|b| (b, true)))
            .or_else(|| {
                focus_moved
                    .then(|| doc.focused_box().map(|b| (b, false)))
                    .flatten()
            });
        if let Some(((top, bottom), start)) = reveal
            && !send(Output::Reveal {
                top: top * zoom,
                bottom: bottom * zoom,
                start,
            })
        {
            return;
        }
        if pointed {
            let now = if left { None } else { doc.hovered_link() };
            if now != hovered {
                hovered = now.clone();
                if !send(Output::Hover(now)) {
                    return;
                }
            }
        }
        for url in doc.take_clicked() {
            if !send(Output::Open(url)) {
                return;
            }
        }
        for (x, y) in contexts {
            if !send(Output::Context(doc.context_at(x, y))) {
                return;
            }
        }
        if let Some((w, max_height, scale)) = thumbnail
            && width > 0
        {
            // Scale the content column, not the page around it, to the strip.
            let (left, column) = doc.content_column();
            let factor = w / column;
            let pw = (w * scale).ceil().clamp(1., 4096.) as u32;
            let ph = ((height * factor).min(max_height) * scale)
                .ceil()
                .clamp(1., 8192.) as u32;
            let pixels = doc.pixels_at(pw, ph, factor * scale, left, 0.);
            let (size, pixels) = pad(pw, ph, pixels);
            if !send(Output::Thumbnail {
                revision,
                size,
                content: (pw, ph),
                scale,
                pixels,
            }) {
                return;
            }
        }
        for (band, (w, scale)) in paints {
            // A request for an older width waits for the layout it belongs to.
            if w != width || width == 0 {
                continue;
            }
            let top = band as f32 * band_height(scale);
            let shown = height * zoom;
            if top >= shown {
                continue;
            }
            let rows = (shown - top).min(band_height(scale));
            let pw = (width as f32 * scale).ceil().clamp(1., 8192.) as u32;
            let ph = ((rows * scale).ceil() as u32).clamp(1, TILE);
            let pixels = doc.pixels(pw, ph, scale * zoom, band * TILE);
            let (size, pixels) = pad(pw, ph, pixels);
            if !send(Output::Tile {
                band,
                revision,
                width,
                scale,
                size,
                pixels,
            }) {
                return;
            }
        }
    }
}
