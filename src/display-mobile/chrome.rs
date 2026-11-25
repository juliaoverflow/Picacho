//! Touch-sized building blocks: bars that respect the safe area, buttons
//! with full-size hit areas, list cells and chips. Each control's visible
//! shape may be small, but the area that takes a tap is never smaller than
//! `TOUCH` points.
use super::*;

/// What a control does when tapped.
pub(super) type Tap = Rc<dyn Fn(&mut MailApp, &mut Window, &mut Context<MailApp>)>;

/// Runs a shared action.
pub(super) fn act(action: Action) -> Tap {
    Rc::new(move |this, w, cx| this.handle(action.clone(), w, cx))
}

/// Runs a shared action and slides in the screen it opens.
pub(super) fn push(action: Action) -> Tap {
    Rc::new(move |this, w, cx| this.mobile_push(action.clone(), w, cx))
}

impl MailApp {
    pub(super) fn tap_listener(
        &self,
        tap: Tap,
        cx: &Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _: &ClickEvent, w, cx| {
            cx.stop_propagation();
            tap(this, w, cx)
        })
    }

    /// A bar along the top, below the status bar: a back chevron (on
    /// screens that can be left), the screen's title beside it at the
    /// leading edge, and trailing controls.
    pub(super) fn nav_bar(
        &self,
        id: &'static str,
        insets: Insets,
        back: Option<AnyElement>,
        title: impl Into<SharedString>,
        subtitle: Option<String>,
        trailing: Vec<AnyElement>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let has_back = back.is_some();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_shrink_0()
            .pt(px(insets.top))
            .pl(px(insets.left + if has_back { 4. } else { GUTTER }))
            .pr(px(insets.right + 4.))
            .bg(rgb(c.bg))
            .child(
                div()
                    .min_h(px(NAV_BAR))
                    .flex()
                    .items_center()
                    .children(back)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(22.))
                                    .font_weight(FontWeight::BOLD)
                                    .line_height(relative(1.2))
                                    .child(title.into()),
                            )
                            .children(subtitle.map(|s| {
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(rgb(c.muted))
                                    .child(s)
                            })),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .children(trailing),
                    ),
            )
    }

    /// A bar along the bottom, inside the thumb's reach, above the home
    /// indicator (or the keyboard, when it is up). The leading and trailing
    /// groups take the same width, so whatever is between them is centered.
    pub(super) fn tool_row(
        &self,
        id: &'static str,
        insets: Insets,
        leading: Vec<AnyElement>,
        middle: Option<AnyElement>,
        trailing: Vec<AnyElement>,
    ) -> Stateful<Div> {
        let side = px(TOUCH * leading.len().max(trailing.len()) as f32);
        self.bottom_bar(id, insets).child(
            div()
                .h(px(TOOLBAR))
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(side).flex_shrink_0().flex().children(leading))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_center()
                        .children(middle),
                )
                .child(
                    div()
                        .w(side)
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .children(trailing),
                ),
        )
    }

    /// A bottom bar of actions spread evenly across its width.
    pub(super) fn action_row(
        &self,
        id: &'static str,
        insets: Insets,
        actions: Vec<AnyElement>,
    ) -> Stateful<Div> {
        self.bottom_bar(id, insets).child(
            div().h(px(TOOLBAR)).flex().items_center().children(
                actions
                    .into_iter()
                    .map(|a| div().flex_1().flex().justify_center().child(a)),
            ),
        )
    }

    pub(super) fn bottom_bar(&self, id: &'static str, insets: Insets) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .flex_shrink_0()
            .pb(px(insets.below()))
            .pl(px(insets.left + 8.))
            .pr(px(insets.right + 8.))
            .bg(rgb(c.bg))
            .border_t_1()
            .border_color(rgb(c.border))
    }

    /// An icon button with a full-size hit area.
    pub(super) fn touch_icon(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: &'static str,
        tap: Tap,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let id = id.into();
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector)
            .size(px(TOUCH))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .active(move |s| s.bg(rgb(c.border)))
            .child(icon_el(icon, 22., c.accent))
            .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
            .on_click(self.tap_listener(tap, cx))
    }

    /// A bottom bar's end button: the same tinted circle as Compose, so
    /// both ends of the bar match.
    pub(super) fn bar_icon(
        &self,
        id: &'static str,
        icon: &'static str,
        label: &'static str,
        tap: Tap,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        self.touch_icon(id, icon, label, tap, cx)
            .bg(rgb(c.soft))
            .active(|s| s.opacity(0.6))
            .into_any_element()
    }

    /// A text button, as in a navigation bar ("Done", "Cancel").
    pub(super) fn touch_text(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        strong: bool,
        tap: Tap,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let id = id.into();
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector)
            .h(px(TOUCH))
            .min_w(px(TOUCH))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.))
            .cursor_pointer()
            .text_size(px(17.))
            .text_color(rgb(c.accent))
            .when(strong, |d| d.font_weight(FontWeight::SEMIBOLD))
            .active(|s| s.opacity(0.5))
            .child(label.into())
            .on_click(self.tap_listener(tap, cx))
    }

    /// The chevron that goes back, leading a nav bar.
    /// Every back button goes back the same way: the screen slides off
    /// over the one below it, as the edge swipe does.
    pub(super) fn back_icon(&self, id: &'static str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        div()
            .id(id)
            .debug_selector(move || id.into())
            .size(px(TOUCH))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .active(|s| s.opacity(0.5))
            .child(icon_el("chevron-left", 28., c.accent))
            .tooltip(|w, cx| Tooltip::new(t!("mobile-chrome-back")).build(w, cx))
            .on_click(self.tap_listener(
                Rc::new(
                    |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                        this.mobile_dismiss_keyboard(w, cx);
                        this.mobile_animate_back(w, cx)
                    },
                ),
                cx,
            ))
            .into_any_element()
    }

    /// The compose button that ends a bottom bar: the same size and shape
    /// as the bar's other buttons, set apart only by its tinted circle.
    pub(super) fn compose_button(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        div()
            .id("mobile-compose")
            .debug_selector(|| "mobile-compose".into())
            .size(px(TOUCH))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(rgb(c.soft))
            .cursor_pointer()
            .active(|s| s.opacity(0.6))
            .child(icon_el("compose", 22., c.accent))
            .tooltip(|w, cx| Tooltip::new(t!("mobile-chrome-new-message")).build(w, cx))
            .on_click(self.tap_listener(act(Action::Compose), cx))
            .into_any_element()
    }

    /// A row in a grouped list: an icon, a label, an optional detail and a
    /// chevron when it leads somewhere.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cell(
        &self,
        id: impl Into<SharedString>,
        lead: AnyElement,
        label: impl Into<SharedString>,
        detail: Option<String>,
        chevron: bool,
        active: bool,
        tap: Tap,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let id = id.into();
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector)
            .min_h(px(TOUCH + 8.))
            .w_full()
            .px(px(GUTTER))
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            // Marks the open page beside a list on a wide screen; a phone,
            // showing one screen at a time, marks nothing.
            .when(active, |d| d.bg(rgb(c.soft)))
            // A brief, neutral press, as a native list's.
            .active(move |s| s.bg(rgb(c.border)))
            .child(
                div()
                    .size(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(lead),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(17.))
                    .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(label.into()),
            )
            .children(detail.map(|d| {
                div()
                    .flex_shrink_0()
                    .text_size(px(15.))
                    .text_color(rgb(c.muted))
                    .child(d)
            }))
            .when(chevron, |d| d.child(icon_el("chevron-right", 18., c.muted)))
            .on_click(self.tap_listener(tap, cx))
    }

    /// Rows on a rounded card, separated by hairlines (an inset grouped list).
    pub(super) fn group(&self, rows: Vec<AnyElement>) -> Div {
        let c = self.colors();
        let n = rows.len();
        let mut card = div()
            .w_full()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(rgb(c.surface))
            .border_1()
            .border_color(rgb(c.border));
        for (i, row) in rows.into_iter().enumerate() {
            card = card.child(row);
            if i + 1 < n {
                card = card.child(div().h(px(1.)).ml(px(GUTTER + 40.)).bg(rgb(c.border)));
            }
        }
        card
    }

    /// The small heading above a group.
    pub(super) fn group_label(&self, text: impl Into<SharedString>) -> Div {
        let c = self.colors();
        let text: SharedString = text.into();
        div()
            .pt_5()
            .pb_2()
            .px(px(GUTTER))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(c.muted))
            .child(text.to_uppercase())
    }

    /// A pill that turns a filter on: 34 points tall, inside a 44-point
    /// hit area.
    pub(super) fn chip(
        &self,
        id: impl Into<SharedString>,
        label: &'static str,
        on: bool,
        tap: Tap,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let id = id.into();
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector)
            .h(px(TOUCH))
            .flex_shrink_0()
            .flex()
            .items_center()
            .cursor_pointer()
            .child(
                div()
                    .h(px(34.))
                    .px_4()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_size(px(15.))
                    .border_1()
                    .when(on, |d| {
                        d.bg(rgb(c.accent))
                            .border_color(rgb(c.accent))
                            .text_color(rgb(c.on_accent))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!on, |d| {
                        d.bg(rgb(c.surface))
                            .border_color(rgb(c.border))
                            .text_color(rgb(c.text))
                    })
                    .child(label),
            )
            .active(|s| s.opacity(0.7))
            .on_click(self.tap_listener(tap, cx))
    }

    /// A count on a cell or button.
    pub(super) fn badge(&self, n: usize) -> Option<String> {
        (n > 0).then(|| fmt_count(n))
    }

    /// The notice the desktop shows in a corner, above the bottom bar.
    pub(super) fn mobile_toast(
        &self,
        insets: Insets,
        screen: Screen,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.colors();
        // Bars at the bottom of these screens; the toast sits above them.
        let bar = match screen {
            Screen::List | Screen::Reader | Screen::Mailboxes => TOOLBAR,
            _ => 0.,
        };
        let above = insets.below() + bar + 8.;
        if let Some(outbox) = self.outbox_toast(cx) {
            return Some(
                div()
                    .absolute()
                    .left(px(insets.left + GUTTER))
                    .right(px(insets.right + GUTTER))
                    .bottom(px(above))
                    .flex()
                    .justify_center()
                    .child(outbox)
                    .into_any_element(),
            );
        }
        if self.status.is_empty() || (self.modal.is_some() && self.modal != Some(Modal::Settings)) {
            return None;
        }
        Some(
            div()
                .id("notice-toast")
                .debug_selector(|| "notice-toast".into())
                .absolute()
                .left(px(insets.left + GUTTER))
                .right(px(insets.right + GUTTER))
                .bottom(px(above))
                .min_h(px(TOUCH + 8.))
                .pl_4()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(14.))
                .bg(rgb(c.raised))
                .border_1()
                .border_color(rgb(c.border))
                .shadow_lg()
                .text_size(px(15.))
                .text_color(rgb(if self.error { c.danger } else { c.text }))
                .child(div().flex_1().min_w_0().py_2().child(self.status.clone()))
                // Undo, then a clear gap and a rule before the close button,
                // so a thumb reaching for one doesn't land on the other.
                .when(self.undo.is_some() && !self.error, |d| {
                    d.child(
                        self.touch_text("toast-undo", t!("mobile-chrome-undo"), true, act(Action::Undo), cx)
                            .px_3(),
                    )
                    .child(div().w(px(16.)).flex_shrink_0())
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(24.))
                            .flex_shrink_0()
                            .bg(rgb(c.border)),
                    )
                })
                .child(self.touch_icon(
                    "toast-dismiss",
                    "close",
                    t!("mobile-chrome-dismiss"),
                    act(Action::Dismiss),
                    cx,
                ))
                .with_animation(
                    "toast-enter",
                    Animation::new(Duration::from_millis(200))
                        .with_easing(|t| 1. - (1. - t).powi(3)),
                    |d, t| d.opacity(t),
                )
                .into_any_element(),
        )
    }
}

impl MailApp {
    /// Opens `entries` as an action sheet when the element holding this is
    /// long-pressed (or right-clicked, in a desktop preview). Add it as the
    /// last child of a `relative` element.
    pub(super) fn press_menu(
        &self,
        entries: impl Fn(&MailApp) -> Vec<Entry> + 'static,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let this = cx.weak_entity();
        let entries = Rc::new(entries);
        let on_press = entries.clone();
        let right = this.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let this = this.clone();
                let entries = entries.clone();
                window.on_mouse_event(move |e: &LongPressEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || e.phase != TouchPhase::Started
                        || !bounds.contains(&e.start_position)
                        || window.default_prevented()
                    {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    platform::haptic(platform::Haptic::Selection);
                    let _ = this.update(cx, |this, cx| {
                        let entries = entries(this);
                        this.open_menu(e.start_position, entries, cx)
                    });
                });
                let right = right.clone();
                let on_press = on_press.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble
                        && e.button == MouseButton::Right
                        && bounds.contains(&e.position)
                    {
                        cx.stop_propagation();
                        let _ = right.update(cx, |this, cx| {
                            let entries = on_press(this);
                            this.open_menu(e.position, entries, cx)
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}
