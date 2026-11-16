//! Sheets from the bottom edge, in place of the desktop's popup menus and
//! centered dialogs: where a thumb already is, and dismissed by tapping
//! above them.
use super::*;

/// A sheet covers at most this share of the screen's height.
const SHEET_MAX: f32 = 0.9;
/// On a wide screen, sheets keep to this width, centered.
const SHEET_WIDTH: f32 = 560.;

impl MailApp {
    /// A menu's entries as an action sheet: large rows with their icons,
    /// then a separate Cancel.
    pub(super) fn action_sheet(
        &self,
        insets: Insets,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        let height = f32::from(window.viewport_size().height);
        let wide = Self::mobile_split(window);
        let any_checks = menu.entries.iter().any(|e| {
            matches!(
                e,
                Entry::Item {
                    icon: "",
                    checked: true,
                    ..
                }
            )
        });
        // Each run of entries between separators is a card of its own;
        // hairlines part the rows inside a card, gaps part the cards.
        let mut groups: Vec<Vec<AnyElement>> = vec![vec![]];
        for (i, e) in menu.entries.iter().enumerate() {
            match e {
                Entry::Separator => {
                    if groups.last().is_some_and(|g| !g.is_empty()) {
                        groups.push(vec![]);
                    }
                }
                Entry::Item {
                    label,
                    icon,
                    action,
                    checked,
                    danger,
                } => {
                    let action = action.clone();
                    let color = if *danger { c.danger } else { c.text };
                    let row = div()
                        .id(("menu-item", i))
                        .debug_selector(|| format!("menu-item-{i}"))
                        .flex_shrink_0()
                        .min_h(px(TOUCH + 12.))
                        .px(px(GUTTER + 4.))
                        .flex()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .text_size(px(17.))
                        .text_color(rgb(color))
                        .active(move |s| s.bg(rgb(c.border)))
                        .child(
                            div()
                                .size(px(24.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(if icon.is_empty() {
                                    if *checked && any_checks {
                                        icon_el("check", 20., c.accent).into_any_element()
                                    } else {
                                        div().into_any_element()
                                    }
                                } else {
                                    icon_el(icon, 22., if *danger { c.danger } else { c.accent })
                                        .into_any_element()
                                }),
                        )
                        .child(div().flex_1().min_w_0().py_2().child(label.clone()))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.menu = None;
                            this.handle(action.clone(), w, cx)
                        }));
                    if let Some(group) = groups.last_mut() {
                        group.push(row.into_any_element());
                    }
                }
            }
        }
        groups.retain(|g| !g.is_empty());
        let mut list = div()
            .id("menu-panel")
            .debug_selector(|| "menu-panel".into())
            .track_scroll(&self.mobile.sheet_scroll)
            .max_h(px((height - insets.top - insets.below()) * 0.7))
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .flex()
            .flex_col()
            .gap_2()
            .on_scroll(|_, _, cx| cx.stop_propagation());
        for rows in groups {
            let n = rows.len();
            let mut card = div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .rounded(px(14.))
                .overflow_hidden()
                .bg(rgb(c.raised));
            for (i, row) in rows.into_iter().enumerate() {
                card = card.child(row);
                if i + 1 < n {
                    card = card.child(
                        div()
                            .flex_shrink_0()
                            .h(px(1.))
                            .ml(px(GUTTER + 4. + 24. + 12.))
                            .bg(rgb(c.border)),
                    );
                }
            }
            list = list.child(card);
        }
        let cancel = div()
            .id("menu-cancel")
            .debug_selector(|| "menu-cancel".into())
            .flex_shrink_0()
            .h(px(TOUCH + 12.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(14.))
            .bg(rgb(c.raised))
            .cursor_pointer()
            .text_size(px(17.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(c.accent))
            .active(move |s| s.bg(rgb(c.border)))
            .child("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }));
        let sheet = div()
            .w_full()
            .when(wide, |d| d.max_w(px(SHEET_WIDTH)))
            .px_2()
            .pb(px(insets.below() + 8.))
            .flex()
            .flex_col()
            .gap_2()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list)
            .child(cancel);
        self.sheet_backdrop("menu-backdrop")
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(sheet.with_animation(
                ("action-sheet", menu.key),
                Animation::new(Duration::from_millis(220)).with_easing(|t| 1. - (1. - t).powi(3)),
                // Offset, not resized: the sheet keeps its layout as it rises.
                |d, t| d.relative().top(px((1. - t) * 160.)).opacity(t),
            ))
            .into_any_element()
    }

    /// Dims the screen behind a sheet, which sits at its bottom.
    fn sheet_backdrop(&self, id: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .bg(rgba(0x0a121c66))
    }

    /// A desktop dialog's panel restyled as a sheet: full width (or a
    /// centered column on a wide screen) with rounded top corners.
    pub(in crate::display::ui) fn mobile_sheet_panel(
        &self,
        d: Stateful<Div>,
        modal: &Modal,
        window: &Window,
    ) -> Stateful<Div> {
        let size = window.viewport_size();
        let tall = matches!(
            modal,
            Modal::Connect | Modal::Reconnect(_) | Modal::Original | Modal::Snooze(_)
        );
        let wide = Self::mobile_split(window);
        let max_h = f32::from(size.height) * SHEET_MAX;
        d.w_full()
            .max_w(if wide { px(SHEET_WIDTH) } else { size.width })
            .max_h(px(max_h))
            .when(tall, |d| d.h(px(max_h)))
            .when(!wide, |d| {
                d.rounded_t(px(16.)).rounded_b(px(0.)).border_b_0()
            })
            .border_l_0()
            .border_r_0()
            .text_size(px(16.))
    }

    /// The frame a dialog's sheet sits in: the dimmed screen, the sheet at
    /// the bottom, above the home indicator and the keyboard.
    pub(in crate::display::ui) fn mobile_sheet_frame(
        &self,
        panel: Stateful<Div>,
        window: &Window,
    ) -> AnyElement {
        let insets = self.mobile_insets(window);
        let wide = Self::mobile_split(window);
        div()
            .occlude()
            .on_mouse_down(MouseButton::Left, swallow_press)
            .absolute()
            .inset_0()
            .bg(rgba(0x0a121c66))
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .pt(px(insets.top))
            .pb(px(if wide {
                insets.below() + 16.
            } else {
                insets.keyboard
            }))
            .child(
                panel
                    .when(!wide, |d| d.pb(px(insets.bottom.max(0.))))
                    .with_animation(
                        ("modal-enter", self.anim),
                        Animation::new(PUSH).with_easing(|t| 1. - (1. - t).powi(3)),
                        |d, t| d.relative().top(px((1. - t) * 240.)).opacity(t),
                    ),
            )
            .into_any_element()
    }
}
