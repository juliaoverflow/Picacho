//! Pane layout: the collapsible sidebar, its animations and toggle.
use super::*;

/// Ease-out cubic, the curve every pane animation shares.
pub(super) fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

impl MailApp {
    pub(super) fn pane_animating(since: Option<std::time::Instant>) -> bool {
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
        self
            .backend
            .store.set_preference_soon("sidebar", if collapsed { "collapsed" } else { "expanded" });
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
    /// The picture menu: from the provider, a preset, or a file.
    pub(super) fn avatar_entries(&self, id: &str) -> Vec<Entry> {
        let Some(a) = self.accounts.iter().find(|a| a.id == id) else {
            return vec![];
        };
        let provider = if a.provider == Provider::Gmail && a.oauth {
            "Load from Google"
        } else {
            "Load from Gravatar"
        };
        let mut entries = vec![
            entry(provider, "cloud", Action::ProviderAvatar(id.into())),
            entry(
                "Choose from Presets…",
                "grid",
                Action::AvatarPresets(id.into()),
            ),
            entry(
                "Upload Your Own…",
                "upload",
                Action::ChooseAvatar(id.into()),
            ),
        ];
        if self.avatars.contains_key(id) {
            entries.push(Entry::Separator);
            entries.push(danger(
                "Remove Picture",
                "trash",
                Action::RemoveAvatar(id.into()),
            ));
        }
        entries
    }
    /// Replaces the picture with the provider's, if it has one.
    pub(super) fn provider_avatar(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        self.notice("Looking for your picture…", false, cx);
        self.task(
            cx,
            move |b| b.provider_avatar(&a),
            move |this, result, cx| match result {
                Ok(Some(path)) => {
                    this.avatars.insert(id, path);
                    this.notice("Picture updated", false, cx);
                }
                Ok(None) => this.notice("No picture was found for this address.", true, cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }
    pub(super) fn preset_avatar(&mut self, id: String, name: &'static str, cx: &mut Context<Self>) {
        let Some(bytes) = crate::display::assets::preset(name) else {
            return;
        };
        if matches!(self.modal, Some(Modal::AvatarPresets(_))) {
            self.modal = None;
        }
        self.presets_for = None;
        self.task(
            cx,
            move |b| b.set_avatar(&id, bytes, "preset").map(|path| (id, path)),
            |this, result, cx| match result {
                Ok((id, path)) => {
                    this.avatars.insert(id, path);
                }
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }
    /// A grid of preset pictures.
    pub(super) fn presets_modal(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        div()
            .id("avatar-presets")
            .debug_selector(|| "avatar-presets".into())
            .flex()
            .flex_wrap()
            .justify_center()
            .gap_4()
            .children(
                crate::display::assets::PRESETS
                    .iter()
                    .map(|(name, label, _)| {
                        let id = id.to_string();
                        let name: &'static str = name;
                        div()
                            .id(SharedString::from(format!("preset-{name}")))
                            .debug_selector(move || format!("preset-{name}"))
                            .w(px(92.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .group("preset")
                            .child(
                                div()
                                    .size(px(84.))
                                    .p(px(3.))
                                    .rounded(px(14.))
                                    .border_2()
                                    .border_color(rgb(c.border))
                                    .bg(rgb(c.bg))
                                    .shadow_sm()
                                    .hover(move |s| s.border_color(rgb(c.accent)))
                                    .child(
                                        img(SharedString::from(format!("avatars/{name}.jpg")))
                                            .size_full()
                                            .rounded(px(10.)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(*label),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.handle(Action::SetPresetAvatar(id.clone(), name), w, cx)
                            }))
                    }),
            )
            .into_any_element()
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
                    backend.set_avatar(&key, &bytes, "custom")
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
/// The narrowest reader: wide enough for the email's controls row (light
/// or dark, formatted or plain, remote images) on one line at the narrow
/// reader's 20-pixel margins. `reader_controls_stay_on_one_row` checks it.
pub(super) const READER_MIN: f32 = 528.;
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
                store.set_preference_soon("sidebar_width", &self.sidebar_width.to_string());
            }
            Pane::Reader => {
                if let Some(w) = self.reader_width {
                    store.set_preference_soon("reader_width", &w.to_string());
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
        self
            .backend
            .store.set_preference_soon("reader", if collapsed { "collapsed" } else { "expanded" });
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
            // The window's cursor can only be set while painting.
            canvas(
                |_, _, _| {},
                |_, _, w, _| w.set_window_cursor_style(CursorStyle::ResizeLeftRight),
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
    pub(super) fn ribbon(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        column.child(self.preview(mail.as_ref(), cx))
    }
    /// The open message in miniature, filling the collapsed reader below its
    /// actions: the HTML email itself scaled to the ribbon's width, or a
    /// minimap of a plain text message's subject and lines. Clicking a place
    /// in it opens the reader there.
    fn preview(&self, mail: Option<&Mail>, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors();
        let tile = div()
            .id("reader-preview")
            .debug_selector(|| "reader-preview".into())
            .relative()
            .flex_1()
            .min_h(px(40.))
            .w(px(RIBBON - 16.))
            .mt_2()
            .mb_2()
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(rgb(c.border));
        let Some(m) = mail else {
            return tile.bg(rgb(c.bg)).into_any_element();
        };
        let options = self.reading_options(m);
        let dark = self.email_dark(options);
        let anchor = self.preview_bounds.clone();
        let measure = canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| {})
            .absolute()
            .inset_0();
        let anchor = self.preview_bounds.clone();
        let reveal = move |height: f32| {
            let anchor = anchor.clone();
            move |this: &mut Self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>| {
                let y = f32::from(e.position.y - anchor.get().top());
                this.handle(Action::Reveal((y / height.max(1.)).min(1.)), w, cx);
            }
        };
        let tile = tile
            .cursor_pointer()
            .hover(move |s| s.border_color(rgb(c.accent)))
            .tooltip(|w, cx| Tooltip::new("Open the message here").build(w, cx))
            .child(measure);
        if options.html
            && let Some(html) = &m.html
        {
            self.html_view.update(cx, |view, cx| {
                view.set_content(html, &m.inline, options.block_remote, dark, self.clean_links, cx)
            });
            let view = self.html_view.read(cx);
            let page = view.page_color();
            let height = view.thumbnail_height();
            let cropped =
                height.is_some_and(|h| h > f32::from(self.preview_bounds.get().size.height));
            return tile
                .bg(rgb(page))
                .on_mouse_down(MouseButton::Left, cx.listener(reveal(height.unwrap_or(1.))))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(crate::display::html::thumbnail(self.html_view.clone())),
                )
                // A long email fades out at the bottom of the strip.
                .when(cropped, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(36.))
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(rgba(page << 8), 0.),
                                linear_color_stop(rgba((page << 8) | 0xff), 1.),
                            )),
                    )
                })
                .into_any_element();
        }
        // Plain text: the subject, then a bar for each wrapped line.
        let (bg, fg) = if dark == self.dark {
            (c.surface, c.text)
        } else if dark {
            (0x1f2023, 0xe8eaed)
        } else {
            (0xffffff, 0x202124)
        };
        let lines = minimap_lines(m.body.as_deref().unwrap_or(&m.snippet));
        const LINE: f32 = 5.;
        let width = RIBBON - 16. - 14.;
        let bar = move |fraction: f32, height: f32, alpha: u32| {
            div()
                .h(px(height))
                .w(px((width * fraction).max(3.)))
                .rounded_full()
                .bg(rgba((fg << 8) | alpha))
        };
        let height = 26. + lines.len() as f32 * LINE;
        tile.bg(rgb(bg))
            .on_mouse_down(MouseButton::Left, cx.listener(reveal(height)))
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(7.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(bar(
                        (m.subject.chars().count() as f32 / 24.).min(1.),
                        4.,
                        0xa0,
                    ))
                    .child(bar(0.55, 3., 0x60))
                    .child(div().h(px(6.)))
                    .children(lines.into_iter().map(move |line| {
                        div()
                            .h(px(LINE - 3.))
                            .when_some(line, |d, fraction| d.child(bar(fraction, 2., 0x55)))
                    })),
            )
            .into_any_element()
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

/// A plain text message as minimap lines: each wrapped line's length as a
/// fraction of the widest, or None for a blank line between paragraphs.
pub(super) fn minimap_lines(text: &str) -> Vec<Option<f32>> {
    const WRAP: usize = 48;
    const MAX: usize = 240;
    let mut lines = Vec::new();
    for line in text.lines() {
        let length = line.trim_end().chars().count();
        if length == 0 {
            if lines.last().is_some_and(|l: &Option<f32>| l.is_some()) {
                lines.push(None);
            }
            continue;
        }
        let mut rest = length;
        while rest > 0 && lines.len() < MAX {
            let part = rest.min(WRAP);
            // Wrapped lines run full width; a paragraph's last line is shorter.
            lines.push(Some(part as f32 / WRAP as f32));
            rest -= part;
        }
    }
    lines.truncate(MAX);
    lines
}

/// A vertically scrolling area with a scrollbar over its right edge. The
/// content tracks `handle` and keeps its own layout inside the frame.
pub(super) fn scroll_frame(
    content: impl IntoElement,
    handle: &(impl gpui_component::scroll::ScrollbarHandle + Clone),
) -> Div {
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .child(content)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(gpui_component::scroll::Scrollbar::vertical(handle)),
        )
}

#[cfg(test)]
mod tests {
    use super::minimap_lines;

    #[test]
    fn minimap_wraps_long_lines_and_keeps_one_gap_between_paragraphs() {
        let lines = minimap_lines(&format!("{}\n\n\n\nshort\n", "x".repeat(60)));
        assert_eq!(lines, vec![Some(1.), Some(12. / 48.), None, Some(5. / 48.)]);
        assert!(minimap_lines(&"word ".repeat(100_000)).len() <= 240);
    }
}
