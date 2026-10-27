//! GPUI input, clipping and visible-tile caching for an HTML document.
use super::document::EmailDocument;
use blitz_dom::Document;
use blitz_traits::events::*;
use gpui::{prelude::*, *};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

struct Tile {
    key: (u32, u32, u32, u32, u64),
    image: Arc<RenderImage>,
}

pub struct HtmlView {
    source: String,
    blocked: bool,
    document: Option<EmailDocument>,
    height: f32,
    width: u32,
    bounds: Bounds<Pixels>,
    focus: FocusHandle,
    revision: u64,
    tile: Option<Tile>,
    retired: Vec<Arc<RenderImage>>,
}
impl HtmlView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                if this
                    .update(cx, |this, cx| {
                        if this
                            .document
                            .as_ref()
                            .is_some_and(|d| d.changed.swap(false, Ordering::Acquire))
                        {
                            this.width = 0;
                            this.revision += 1;
                            cx.notify();
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
            blocked: true,
            document: None,
            height: 1.,
            width: 0,
            bounds: Bounds::default(),
            focus: cx.focus_handle(),
            revision: 0,
            tile: None,
            retired: Vec::new(),
        }
    }
    #[cfg(test)]
    pub fn has_document(&self) -> bool {
        self.document.is_some()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let had_document = self.document.is_some();
        self.document = None;
        self.source.clear();
        if let Some(tile) = self.tile.take() {
            self.retired.push(tile.image);
        }
        self.width = 0;
        if had_document {
            cx.notify();
        }
    }
    pub fn set_content(&mut self, html: &str, blocked: bool, cx: &mut Context<Self>) {
        if self.document.is_some() && self.source == html && self.blocked == blocked {
            return;
        }
        self.clear(cx);
        self.source = html.into();
        self.blocked = blocked;
        self.document = Some(EmailDocument::new(html, blocked));
        self.height = 1.;
        self.revision += 1;
        cx.notify();
    }
    fn pointer(
        &mut self,
        position: gpui::Point<Pixels>,
        kind: u8,
        pressed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let x = f32::from(position.x - self.bounds.origin.x);
        let y = f32::from(position.y - self.bounds.origin.y);
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
        let event = match kind {
            1 => UiEvent::PointerDown(event),
            2 => UiEvent::PointerUp(event),
            3 => UiEvent::PointerLeave(event),
            _ => UiEvent::PointerMove(event),
        };
        let previous_hover = document.doc.get_hover_node_id();
        document.doc.handle_ui_event(event);
        if kind == 0 && !pressed && previous_hover == document.doc.get_hover_node_id() {
            return;
        }
        document.doc.resolve(0.);
        self.height = document.content_height();
        self.revision += 1;
        cx.notify();
    }
}
impl Render for HtmlView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .id("html-email")
            .debug_selector(|| "html-email".into())
            .w_full()
            .h(px(self.height))
            .flex_shrink_0()
            .bg(rgb(0xffffff))
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, w, cx| {
                    this.focus.focus(w);
                    this.pointer(e.position, 1, true, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, cx| this.pointer(e.position, 2, false, cx)),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                this.pointer(
                    e.position,
                    0,
                    e.pressed_button == Some(MouseButton::Left),
                    cx,
                )
            }))
            .on_hover(cx.listener(|this, hovered, w, cx| {
                if !hovered {
                    this.pointer(w.mouse_position(), 3, false, cx);
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if (if cfg!(target_os = "macos") {
                    e.keystroke.modifiers.platform
                } else {
                    e.keystroke.modifiers.control
                }) && e.keystroke.key == "c"
                {
                    if let Some(doc) = &this.document
                        && let Some(text) = doc.doc.get_selected_text()
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                } else if matches!(e.keystroke.key.as_str(), "tab" | "enter" | "space") {
                    let key = match e.keystroke.key.as_str() {
                        "tab" => keyboard_types::Key::Named(keyboard_types::NamedKey::Tab),
                        "enter" => keyboard_types::Key::Named(keyboard_types::NamedKey::Enter),
                        _ => keyboard_types::Key::Character(" ".into()),
                    };
                    if let Some(doc) = this.document.as_mut() {
                        let event = BlitzKeyEvent {
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
                        };
                        doc.doc.handle_ui_event(UiEvent::KeyDown(event.clone()));
                        doc.doc.handle_ui_event(UiEvent::KeyUp(BlitzKeyEvent {
                            state: KeyState::Released,
                            ..event
                        }));
                        this.width = 0;
                        this.revision += 1;
                        cx.notify();
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    move |bounds, w, cx| {
                        view.update(cx, |this, cx| {
                            for image in this.retired.drain(..) {
                                let _ = w.drop_image(image);
                            }
                            this.bounds = bounds;
                            let width = f32::from(bounds.size.width).ceil().max(1.) as u32;
                            let doc = this.document.as_mut()?;
                            if this.width != width {
                                this.height = doc.layout(width);
                                this.width = width;
                                this.revision += 1;
                                cx.notify();
                            }
                            // Rasterize only the visible strip. Long emails never allocate a whole-page bitmap.
                            let visible = bounds.intersect(&w.content_mask().bounds);
                            if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
                                return None;
                            }
                            let scale = w.scale_factor();
                            let offset = (f32::from(visible.top() - bounds.top()).max(0.) * scale)
                                .floor() as u32;
                            let pw = (width as f32 * scale).ceil().clamp(1., 8192.) as u32;
                            let ph = (f32::from(visible.size.height) * scale)
                                .ceil()
                                .clamp(1., 8192.) as u32;
                            let key = (pw, ph, offset, scale.to_bits(), this.revision);
                            let image = if let Some(tile) = &this.tile
                                && tile.key == key
                            {
                                tile.image.clone()
                            } else {
                                if let Some(tile) = this.tile.take() {
                                    let _ = w.drop_image(tile.image);
                                }
                                let pixels = doc.pixels(pw, ph, scale, offset);
                                let buffer = image::RgbaImage::from_raw(pw, ph, pixels).unwrap();
                                let image =
                                    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                                this.tile = Some(Tile {
                                    key,
                                    image: image.clone(),
                                });
                                image
                            };
                            Some((
                                Bounds::new(
                                    point(bounds.left(), bounds.top() + px(offset as f32 / scale)),
                                    size(px(pw as f32 / scale), px(ph as f32 / scale)),
                                ),
                                image,
                            ))
                        })
                    },
                    |_, image, w, _| {
                        if let Some((bounds, image)) = image {
                            let _ = w.paint_image(bounds, Corners::default(), image, 0, false);
                        }
                    },
                )
                .size_full(),
            )
    }
}
