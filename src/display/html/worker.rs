//! Each document lives on its own thread. Parsing, styling, layout, input and
//! rasterization never run on the UI thread, so opening a heavy newsletter,
//! hovering links or scrolling never stalls the window.
use super::document::{ContextTarget, EmailDocument, Tone};
use crate::manage::model::InlineParts;
use std::sync::Arc;
use blitz_dom::Document;
use blitz_traits::events::{BlitzKeyEvent, UiEvent};
use futures::channel::mpsc::UnboundedSender;
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, mpsc},
    time::Duration,
};

/// The height of one cached raster strip, in CSS pixels.
pub const BAND: f32 = 512.;

pub enum Command {
    /// Lay the document out for this many CSS pixels of width.
    Layout(u32),
    Pointer(UiEvent),
    Key(BlitzKeyEvent),
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
    /// The document changed: its new revision, height, cursor and selection.
    State {
        revision: u64,
        width: u32,
        height: f32,
        cursor: gpui::CursorStyle,
        selection: Option<String>,
    },
    Tile {
        band: u32,
        revision: u64,
        width: u32,
        scale: f32,
        /// Device pixels.
        size: (u32, u32),
        pixels: Vec<u8>,
    },
    Context(ContextTarget),
    /// The renderer failed on this email; nothing more will come.
    Failed,
    Thumbnail {
        revision: u64,
        /// Device pixels.
        size: (u32, u32),
        pixels: Vec<u8>,
    },
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
    let mut doc = EmailDocument::with_parts(&html, inline, blocked, tone);
    drop(html);
    let mut width = 0u32;
    let mut height = 1f32;
    let mut revision = 0u64;
    let mut selection = None;
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
        for command in commands {
            match command {
                Command::Layout(w) => {
                    relayout |= w != width;
                    width = w;
                }
                Command::Pointer(event) => {
                    let interactive = !matches!(event, UiEvent::PointerMove(_))
                        || matches!(&event, UiEvent::PointerMove(e) if e.buttons != blitz_traits::events::MouseEventButtons::None);
                    let hover = doc.doc.get_hover_node_id();
                    let cursor = doc.cursor();
                    doc.doc.handle_ui_event(event);
                    restyle |= interactive
                        || hover != doc.doc.get_hover_node_id()
                        || cursor != doc.cursor();
                    selection_changed |= interactive;
                }
                Command::Key(event) => {
                    doc.doc.handle_ui_event(UiEvent::KeyDown(event.clone()));
                    doc.doc.handle_ui_event(UiEvent::KeyUp(BlitzKeyEvent {
                        state: blitz_traits::events::KeyState::Released,
                        ..event
                    }));
                    relayout = width > 0;
                }
                Command::Paint { band, width, scale } => {
                    paints.insert(band, (width, scale));
                }
                Command::Context { x, y } => contexts.push((x, y)),
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
        } else if restyle && width > 0 {
            doc.doc.resolve(0.);
            height = doc.content_height();
        }
        if selection_changed {
            let now = doc.doc.get_selected_text().filter(|s| !s.is_empty());
            selection_changed = now != selection;
            selection = now;
        }
        if relayout || restyle || selection_changed {
            revision += 1;
            if !send(Output::State {
                revision,
                width,
                height,
                cursor: doc.cursor(),
                selection: selection.clone(),
            }) {
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
            if !send(Output::Thumbnail {
                revision,
                size: (pw, ph),
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
            let top = band as f32 * BAND;
            if top >= height {
                continue;
            }
            let rows = (height - top).min(BAND);
            let pw = (width as f32 * scale).ceil().clamp(1., 8192.) as u32;
            let ph = (rows * scale).ceil().clamp(1., 8192.) as u32;
            let offset = (top * scale).round() as u32;
            let pixels = doc.pixels(pw, ph, scale, offset);
            if !send(Output::Tile {
                band,
                revision,
                width,
                scale,
                size: (pw, ph),
                pixels,
            }) {
                return;
            }
        }
    }
}
