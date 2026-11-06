//! Pane layout: the collapsible sidebar, its animations and toggle.
use super::*;

/// Ease-out cubic, the curve every pane animation shares.
pub(super) fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

impl MailApp {
    fn pane_animating(since: Option<std::time::Instant>) -> bool {
        since.is_some_and(|t| t.elapsed() < PANE_ANIMATION)
    }
    /// Re-renders once an animation finishes so its outgoing layer is dropped.
    fn after_animation(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            Timer::after(PANE_ANIMATION + Duration::from_millis(16)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    pub(super) fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.sidebar_collapsed == collapsed {
            return;
        }
        self.sidebar_from = self.sidebar_shown.get();
        self.sidebar_collapsed = collapsed;
        self.sidebar_anim += 1;
        self.sidebar_since = Some(std::time::Instant::now());
        Self::after_animation(cx);
        let _ = self
            .backend
            .store
            .set_preference("sidebar", if collapsed { "collapsed" } else { "expanded" });
        cx.notify();
    }
    /// Rows slide in one after another while the sidebar opens or closes.
    pub(super) fn stagger<E: IntoElement + Styled + 'static>(
        &self,
        el: E,
        index: usize,
    ) -> AnyElement {
        if !Self::pane_animating(self.sidebar_since) {
            return el.into_any_element();
        }
        let delay = index.min(10) as f32 * 0.045;
        el.with_animation(
            ElementId::NamedInteger(format!("sidebar-row-{index}").into(), self.sidebar_anim),
            Animation::new(PANE_ANIMATION + Duration::from_millis(120)),
            move |el, t| {
                let t = ease_out(((t - delay) / (1. - delay)).clamp(0., 1.));
                el.opacity(t).left(px((1. - t) * -10.))
            },
        )
        .into_any_element()
    }
    /// The sidebar toggle, beside Compose (or above it in the rail).
    pub(super) fn sidebar_toggle(&self, collapsed: bool, cx: &Context<Self>) -> Stateful<Div> {
        self.button(
            "toggle-sidebar",
            if collapsed {
                "@Show sidebar · ⌃⌘S"
            } else {
                "@Hide sidebar · ⌃⌘S"
            },
            if collapsed {
                "panel-left-open"
            } else {
                "panel-left-close"
            },
            Action::ToggleSidebar,
            false,
            cx,
        )
    }
    /// The sidebar at `width`. Toggling crossfades the folder tree and the
    /// rail while the width animates, so closing is as smooth as opening.
    pub(super) fn sidebar(&self, width: f32, collapsed: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        self.sidebar_shown.set(width);
        let layer = |content: AnyElement, w: f32, collapsed: bool| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .bottom_0()
                .w(px(w))
                .flex()
                .flex_col()
                .child(content)
                .child(self.download_status(collapsed, cx))
        };
        let tree = || {
            layer(
                self.sidebar_tree(cx).into_any_element(),
                self.sidebar_width,
                false,
            )
        };
        let rail = || layer(self.sidebar_rail(cx).into_any_element(), SIDEBAR_RAIL, true);
        let sidebar = div()
            .id("sidebar")
            .debug_selector(|| "sidebar".into())
            .relative()
            .flex_shrink_0()
            .h_full()
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(c.border));
        if !Self::pane_animating(self.sidebar_since) {
            return sidebar
                .w(px(width))
                .child(if collapsed { rail() } else { tree() })
                .into_any_element();
        }
        let (outgoing, incoming) = if collapsed {
            (tree(), rail())
        } else {
            (rail(), tree())
        };
        let from = self.sidebar_from;
        let animation = || Animation::new(PANE_ANIMATION).with_easing(ease_out);
        sidebar
            .w(px(width))
            .child(outgoing.with_animation(
                ("sidebar-out", self.sidebar_anim),
                animation(),
                |d, t| d.opacity(1. - t),
            ))
            .child(incoming.with_animation(
                ("sidebar-in", self.sidebar_anim),
                animation(),
                |d, t| d.opacity(t),
            ))
            .with_animation(("sidebar", self.sidebar_anim), animation(), move |d, t| {
                d.w(px(from + (width - from) * t))
            })
            .into_any_element()
    }
}

impl MailApp {
    /// An account's picture, or its initials.
    pub(super) fn account_avatar(&self, a: &Account, index: usize, size: f32) -> AnyElement {
        match self.avatars.get(&a.id) {
            Some(path) => img(path.clone())
                .size(px(size))
                .flex_shrink_0()
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => avatar(&initials(&a.name), size, index, self.colors()).into_any_element(),
        }
    }
    /// Shows a saved picture, and fetches a Google profile picture once.
    pub(super) fn load_avatar(&mut self, a: Account, cx: &mut Context<Self>) {
        if let Some(path) = self.backend.avatar(&a.id) {
            self.avatars.insert(a.id.clone(), path);
        }
        let backend = self.backend.clone();
        let id = a.id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.fetch_google_avatar(&a) });
        cx.spawn(async move |this, cx| {
            // A missing picture is not a mailbox problem; it is not reported.
            if let Ok(Some(path)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.avatars.insert(id, path);
                    cx.notify();
                });
            }
        })
        .detach();
    }
    pub(super) fn choose_avatar(&mut self, id: String, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a picture".into()),
        });
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let key = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path)?;
                    anyhow::ensure!(bytes.len() <= 20 << 20, "Choose a picture under 20 MB.");
                    backend.set_avatar(&key, &bytes, true)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        this.avatars.insert(id, path);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The message list never gets narrower than this while panes are resized.
pub(super) const LIST_MIN: f32 = 300.;
/// The list's width when the reader has not been resized yet.
const LIST_DEFAULT: f32 = 380.;
pub(super) const READER_MIN: f32 = 420.;
/// A collapsed reader's ribbon, as wide as the sidebar rail.
pub(super) const RIBBON: f32 = 60.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pane {
    Sidebar,
    Reader,
}
/// A drag of a pane's edge: where it started and the pane's width then.
#[derive(Clone, Copy, Debug)]
pub(super) struct Resize {
    pane: Pane,
    origin: f32,
    start: f32,
}

/// Pane widths for one layout of the three-pane window.
pub(super) struct Layout {
    pub sidebar: f32,
    pub sidebar_collapsed: bool,
    pub reader: f32,
}

impl MailApp {
    /// Fits the sidebar and reader around a list of at least `LIST_MIN`.
    pub(super) fn layout(&self, window: f32, compact: bool) -> Layout {
        let reader_min = if self.reader_collapsed {
            RIBBON
        } else {
            READER_MIN
        };
        let sidebar_collapsed =
            compact || self.sidebar_collapsed || window - LIST_MIN - reader_min < SIDEBAR_MIN;
        let sidebar = if sidebar_collapsed {
            SIDEBAR_RAIL
        } else {
            self.sidebar_width
                .min(window - LIST_MIN - reader_min)
                .clamp(SIDEBAR_MIN, SIDEBAR_MAX)
        };
        let reader = if self.reader_collapsed {
            RIBBON
        } else {
            let room = (window - sidebar - LIST_MIN).max(READER_MIN);
            self.reader_width
                .unwrap_or(window - sidebar - LIST_DEFAULT)
                .clamp(READER_MIN, room)
        };
        Layout {
            sidebar,
            sidebar_collapsed,
            reader,
        }
    }
    pub(super) fn begin_resize(&mut self, pane: Pane, x: f32, cx: &mut Context<Self>) {
        let start = match pane {
            Pane::Sidebar => self.sidebar_shown.get(),
            Pane::Reader => self.reader_shown.get(),
        };
        self.resizing = Some(Resize {
            pane,
            origin: x,
            start,
        });
        cx.notify();
    }
    /// Follows the pointer. Past half way below a pane's smallest useful
    /// width, the pane snaps into its rail; dragging back out reopens it.
    pub(super) fn resize_to(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(r) = self.resizing else {
            return;
        };
        let window = self.window_width.get();
        match r.pane {
            Pane::Sidebar => {
                let proposed = r.start + (x - r.origin);
                if proposed < (SIDEBAR_RAIL + SIDEBAR_MIN) / 2. {
                    self.set_sidebar_collapsed(true, cx);
                } else {
                    let room = window - self.reader_shown.get() - LIST_MIN;
                    self.sidebar_width = proposed.min(room).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                    self.set_sidebar_collapsed(false, cx);
                }
            }
            Pane::Reader => {
                let proposed = r.start - (x - r.origin);
                if proposed < (RIBBON + READER_MIN) / 2. {
                    self.set_reader_collapsed(true, cx);
                } else {
                    let room = window - self.sidebar_shown.get() - LIST_MIN;
                    self.reader_width = Some(proposed.min(room).max(READER_MIN));
                    self.set_reader_collapsed(false, cx);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn end_resize(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.resizing.take() else {
            return;
        };
        let store = &self.backend.store;
        match r.pane {
            Pane::Sidebar => {
                let _ = store.set_preference("sidebar_width", &self.sidebar_width.to_string());
            }
            Pane::Reader => {
                if let Some(w) = self.reader_width {
                    let _ = store.set_preference("reader_width", &w.to_string());
                }
            }
        }
        cx.notify();
    }
    pub(super) fn set_reader_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.reader_collapsed == collapsed {
            return;
        }
        self.reader_from = self.reader_shown.get();
        self.reader_collapsed = collapsed;
        self.reader_anim += 1;
        self.reader_since = Some(std::time::Instant::now());
        Self::after_animation(cx);
        let _ = self
            .backend
            .store
            .set_preference("reader", if collapsed { "collapsed" } else { "expanded" });
        cx.notify();
    }
    /// A pane edge the person can drag. It is zero-width in the layout and
    /// its grab area straddles the border.
    pub(super) fn resize_handle(&self, pane: Pane, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let active = self.resizing.is_some_and(|r| r.pane == pane);
        let selector = match pane {
            Pane::Sidebar => "resize-sidebar",
            Pane::Reader => "resize-reader",
        };
        div().relative().w(px(0.)).h_full().flex_shrink_0().child(
            div()
                .id(selector)
                .debug_selector(move || selector.into())
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(-4.))
                .w(px(8.))
                .flex()
                .justify_center()
                .cursor(CursorStyle::ResizeLeftRight)
                .group("resize-handle")
                .child(
                    div()
                        .w(px(2.))
                        .h_full()
                        .when(active, |d| d.bg(rgb(c.accent)))
                        .when(!active, |d| {
                            d.group_hover("resize-handle", |s| s.bg(rgb(c.border)))
                        }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.begin_resize(pane, f32::from(e.position.x), cx);
                    }),
                ),
        )
    }
    /// Keeps the resize cursor and follows the pointer anywhere in the window
    /// while a pane edge is dragged.
    pub(super) fn track_resize(&self, d: Stateful<Div>, cx: &Context<Self>) -> Stateful<Div> {
        if self.resizing.is_none() {
            return d;
        }
        d.child(
            canvas(
                |_, w, _| w.set_window_cursor_style(CursorStyle::ResizeLeftRight),
                |_, _, _, _| {},
            )
            .absolute()
            .size_0(),
        )
        .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                this.resize_to(f32::from(e.position.x), cx);
            } else {
                this.end_resize(cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.end_resize(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.end_resize(cx)),
        )
    }
    /// The reader at `width`, crossfading with its ribbon when it collapses.
    pub(super) fn reader_pane(
        &self,
        width: Option<f32>,
        corners: Corners<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let Some(width) = width else {
            return self.reader(corners, false, window, cx).into_any_element();
        };
        self.reader_shown.set(width);
        let pane = div()
            .id("reader-frame")
            .relative()
            .flex_shrink_0()
            .h_full()
            .overflow_hidden()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right);
        if !Self::pane_animating(self.reader_since) {
            return pane
                .w(px(width))
                .flex()
                .child(if self.reader_collapsed {
                    self.ribbon(cx).into_any_element()
                } else {
                    self.reader(corners, true, window, cx).into_any_element()
                })
                .into_any_element();
        }
        // The full reader keeps its widest width while it slides, so the
        // message is not laid out again on every frame.
        let full = self.reader_width_cache(width);
        let reader = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(full))
            .flex()
            .child(self.reader(corners, true, window, cx));
        let ribbon = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(RIBBON))
            .flex()
            .child(self.ribbon(cx));
        let (outgoing, incoming) = if self.reader_collapsed {
            (reader, ribbon)
        } else {
            (ribbon, reader)
        };
        let from = self.reader_from;
        let animation = || Animation::new(PANE_ANIMATION).with_easing(ease_out);
        pane.w(px(width))
            .child(outgoing.with_animation(
                ("reader-out", self.reader_anim),
                animation(),
                |d, t| d.opacity(1. - t),
            ))
            .child(
                incoming.with_animation(("reader-in", self.reader_anim), animation(), |d, t| {
                    d.opacity(t)
                }),
            )
            .with_animation(
                ("reader-width", self.reader_anim),
                animation(),
                move |d, t| d.w(px(from + (width - from) * t)),
            )
            .into_any_element()
    }
    fn reader_width_cache(&self, width: f32) -> f32 {
        width.max(self.reader_from).max(READER_MIN)
    }
}

impl MailApp {
    /// The collapsed reader: actions for the open message above a soft,
    /// blurred swatch of the message's own colors.
    pub(super) fn ribbon(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let mail = self.selected_mail();
        let action = |id: &'static str, tip: &str, icon: &str, action: Action| {
            let b = self.button(id, tip, icon, action, false, cx).size(px(36.));
            if mail.is_some() { b } else { b.opacity(0.35) }
        };
        let mut column = div()
            .id("reader-ribbon")
            .debug_selector(|| "reader-ribbon".into())
            .w(px(RIBBON))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_3()
            .child(self.reader_toggle(true, cx).size(px(36.)).rounded(px(10.)))
            .child(div().h(px(1.)).w(px(28.)).my_1().bg(rgb(c.border)));
        if let Some(m) = &mail {
            column = column
                .child(action(
                    "ribbon-reply",
                    "@Reply",
                    "reply",
                    Action::Reply(false),
                ))
                .child(action(
                    "ribbon-reply-all",
                    "@Reply all",
                    "reply-all",
                    Action::Reply(true),
                ))
                .child(action(
                    "ribbon-forward",
                    "@Forward",
                    "forward",
                    Action::Forward,
                ))
                .child(
                    action(
                        "ribbon-star",
                        if m.starred { "@Unstar" } else { "@Star" },
                        "",
                        Action::Star(m.id.clone()),
                    )
                    .child(if m.starred {
                        icon_el("star-filled", 16., STAR)
                    } else {
                        icon_el("star", 16., c.muted)
                    }),
                )
                .child(action(
                    "ribbon-read",
                    "@Mark read / unread",
                    "mail",
                    Action::Read(m.id.clone()),
                ))
                .child(action(
                    "ribbon-archive",
                    "@Archive message",
                    "archive",
                    Action::Move(Folder::Archive),
                ))
                .child(action(
                    "ribbon-trash",
                    "@Move to trash",
                    "trash",
                    Action::Move(Folder::Trash),
                ));
        }
        column.child(self.swatch(mail.as_ref()))
    }
    /// A rounded tile of blurred color, taken from the message's HTML (or
    /// its sender when it has none). Shadows give the blur; the tile clips.
    fn swatch(&self, mail: Option<&Mail>) -> impl IntoElement {
        let c = self.colors();
        let palette = match mail {
            Some(m) => {
                let mut cache = self.palette.borrow_mut();
                let key = format!("{}:{}", m.id, m.html.as_ref().map_or(0, String::len));
                if cache.0 != key {
                    *cache = (key, email_palette(m.html.as_deref(), &m.from));
                }
                cache.1.clone()
            }
            None => vec![c.accent, c.soft, c.border],
        };
        let base = palette[0];
        let mut tile = div()
            .id("reader-swatch")
            .debug_selector(|| "reader-swatch".into())
            .relative()
            .flex_1()
            .min_h(px(40.))
            .w(px(RIBBON - 16.))
            .mt_2()
            .mb_2()
            .rounded(px(16.))
            .overflow_hidden()
            .border_1()
            .border_color(rgba((c.border << 8) | 0xb0))
            // GPUI paints shadows beneath every fill in a layer, so the base
            // color is a shadow too rather than the tile's background.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(16.))
                    .shadow(vec![BoxShadow {
                        color: rgb(base).into(),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(0.),
                    }]),
            );
        // Shadows alone (no fill) paint soft, blurred pools of color.
        let spots = [
            (0.1, -0.4),
            (0.28, 0.5),
            (0.47, -0.3),
            (0.64, 0.45),
            (0.8, -0.2),
        ];
        for (i, (top, left)) in spots.iter().enumerate() {
            let color = palette[(i + 1) % palette.len()];
            tile = tile.child(
                div()
                    .absolute()
                    .top(relative(*top))
                    .left(px(left * 30. + 8.))
                    .size(px(28.))
                    .rounded_full()
                    .shadow(vec![BoxShadow {
                        color: rgb(color).into(),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(26.),
                        spread_radius: px(22.),
                    }]),
            );
        }
        // A frosted wash ties the colors together and keeps them quiet.
        tile.child(div().absolute().inset_0().bg(linear_gradient(
            180.,
            linear_color_stop(rgba((c.surface << 8) | 0x20), 0.),
            linear_color_stop(rgba(c.surface << 8), 1.),
        )))
    }
    pub(super) fn reader_toggle(&self, collapsed: bool, cx: &Context<Self>) -> Stateful<Div> {
        self.button(
            "toggle-reader",
            if collapsed {
                "@Show message"
            } else {
                "@Collapse message"
            },
            if collapsed {
                "panel-right-open"
            } else {
                "panel-right-close"
            },
            Action::ToggleReader,
            false,
            cx,
        )
    }
}

/// Up to four distinct colors that characterize an email's HTML, most used
/// first, falling back to tones derived from the sender.
pub(super) fn email_palette(html: Option<&str>, sender: &str) -> Vec<u32> {
    let mut counts: Vec<(u32, usize)> = Vec::new();
    let mut add = |color: u32| match counts.iter_mut().find(|(c, _)| *c == color) {
        Some((_, n)) => *n += 1,
        None => counts.push((color, 1)),
    };
    let text = html.unwrap_or_default();
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            let d = |k: usize| hex(bytes[i + 1 + k]).unwrap_or(0);
            if digits == 6 {
                add((0..6).fold(0, |v, k| v << 4 | d(k)));
            } else if digits == 3 {
                add((0..3).fold(0, |v, k| (v << 8) | (d(k) * 17)));
            }
            i += 1 + digits;
            continue;
        }
        if bytes.len() - i > 4
            && bytes[i..i + 4].eq_ignore_ascii_case(b"rgb(")
            && let Some(end) = text[i..].find(')')
        {
            let parts: Vec<u32> = text[i + 4..i + end]
                .split(',')
                .take(3)
                .filter_map(|p| p.trim().parse::<f32>().ok())
                .map(|v| v.clamp(0., 255.) as u32)
                .collect();
            if let [r, g, b] = parts[..] {
                add((r << 16) | (g << 8) | b);
            }
            i += end;
            continue;
        }
        i += 1;
    }
    let channels = |c: u32| {
        (
            (c >> 16) as i32 & 255,
            (c >> 8) as i32 & 255,
            c as i32 & 255,
        )
    };
    let colorful = |c: u32| {
        let (r, g, b) = channels(c);
        let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
        hi - lo > 40 && hi > 60
    };
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    let mut palette: Vec<u32> = Vec::new();
    for (color, _) in counts.iter().filter(|(c, _)| colorful(*c)) {
        let (r, g, b) = channels(*color);
        let distinct = palette.iter().all(|p| {
            let (pr, pg, pb) = channels(*p);
            (pr - r).abs() + (pg - g).abs() + (pb - b).abs() > 60
        });
        if distinct {
            palette.push(*color);
        }
        if palette.len() == 4 {
            break;
        }
    }
    if palette.is_empty() {
        // Sender tones: one hue from the address, at three lightnesses.
        let hue = sender
            .bytes()
            .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b.into()))
            % 360;
        palette = [(0.45, 0.55), (0.62, 0.5), (0.35, 0.7)]
            .iter()
            .enumerate()
            .map(|(k, (l, s))| {
                let rgba = hsla(((hue + k as u32 * 40) % 360) as f32 / 360., *s, *l, 1.).to_rgb();
                ((rgba.r * 255.) as u32) << 16
                    | ((rgba.g * 255.) as u32) << 8
                    | (rgba.b * 255.) as u32
            })
            .collect();
    }
    while palette.len() < 3 {
        palette.push(palette[palette.len() - 1]);
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::email_palette;

    #[test]
    fn palette_prefers_frequent_saturated_html_colors() {
        let html = r##"<body style="background:#ffffff;color:#222"><div bgcolor="#1f4e5f">
            <p style="color:#e07a5f">x</p><p style="color:#E07A5F">y</p><a style="background:rgb(129, 178, 154)">z</a>"##;
        let palette = email_palette(Some(html), "a@b.c");
        assert_eq!(palette[0], 0xe07a5f);
        assert!(palette.contains(&0x1f4e5f));
        assert!(palette.contains(&0x81b29a));
        assert!(!palette.contains(&0xffffff));
    }

    #[test]
    fn palette_falls_back_to_sender_tones() {
        let a = email_palette(None, "mara@example.org");
        assert_eq!(a.len(), 3);
        assert_eq!(a, email_palette(Some("<p>plain</p>"), "mara@example.org"));
        assert_ne!(a, email_palette(None, "owen@example.org"));
    }
}
