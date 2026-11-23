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

    /// The warning before a link from an email opens (see `links`): where
    /// it goes, in large letters, what about it is a known trick, and Go
    /// Back as the easy choice. Opening it, trusting its site, or copying
    /// it are there too.
    pub(super) fn link_sheet(
        &self,
        insets: Insets,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        use super::super::links::{LinkChoice, link_dangers};
        let confirm = self.links.confirm.as_ref()?;
        let c = self.colors();
        let wide = Self::mobile_split(window);
        let height = f32::from(window.viewport_size().height);
        let parsed = url::Url::parse(&confirm.url).ok();
        let dangers = parsed.as_ref().map(link_dangers).unwrap_or_default();
        let trustable = dangers.is_empty();
        let choose = |choice: LinkChoice| {
            cx.listener(move |this, _: &ClickEvent, w, cx| {
                this.handle(Action::LinkChoice(choice), w, cx)
            })
        };
        let row_button = |id: &'static str, label: String, color: u32, bold: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_shrink_0()
                .min_h(px(TOUCH + 4.))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(14.))
                .cursor_pointer()
                .text_size(px(17.))
                .text_color(rgb(color))
                .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                .active(move |s| s.opacity(0.7))
                .child(div().truncate().child(label))
        };
        // The site, with the part that names it stressed (the name
        // registered under its public suffix: "evil.co.uk", not "co.uk"):
        // what comes before is anyone's to choose.
        let (lead, named) = match psl::domain_str(&confirm.site)
            .filter(|d| confirm.site.ends_with(d))
        {
            Some(d) => confirm.site.split_at(confirm.site.len() - d.len()),
            None => ("", confirm.site.as_str()),
        };
        let site = div()
            .flex()
            .flex_wrap()
            .justify_center()
            .text_size(px(22.))
            .line_height(relative(1.25))
            .child(div().text_color(rgb(c.muted)).child(lead.to_owned()))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(c.text))
                    .child(named.to_owned()),
            );
        let destination = div()
            .id("link-sheet-destination")
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .p_4()
            .rounded(px(14.))
            .border_2()
            .border_color(rgb(c.danger))
            .bg(rgb(c.bg))
            .child(site)
            .child(
                div()
                    .w_full()
                    .max_h(px(66.))
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .text_center()
                    // The system font: phones resolve no monospace family
                    // by name, and GPUI panics on a font it can't find.
                    .child(confirm.url.clone()),
            );
        let card = div()
            .id("link-sheet")
            .debug_selector(|| "link-sheet".into())
            .w_full()
            .max_h(px((height - insets.top - insets.below()) * 0.85))
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .px_5()
            .pt_6()
            .pb_4()
            .rounded(px(18.))
            .bg(rgb(c.raised))
            .border_t_4()
            .border_color(rgb(c.danger))
            .child(
                div()
                    .size(px(64.))
                    .flex_shrink_0()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba((c.danger << 8) | 0x26))
                    .child(icon_el("warning", 34., c.danger)),
            )
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(c.danger))
                    .text_center()
                    .child("Be careful with this link"),
            )
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(c.text))
                    .text_center()
                    .line_height(relative(1.4))
                    .child(
                        "Links in email are the most common way to steal passwords and money. \
                         This one takes you to:",
                    ),
            )
            .child(destination)
            .when(!dangers.is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(dangers.into_iter().map(|danger| {
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .text_size(px(14.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(c.danger))
                                .child(div().pt(px(2.)).child(icon_el("warning", 15., c.danger)))
                                .child(div().flex_1().min_w_0().child(danger))
                        })),
                )
            })
            .child(
                div()
                    .w_full()
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .line_height(relative(1.4))
                    .text_center()
                    .child(
                        "Open it only if you expected this email and know the site. \
                         Never sign in or pay on a page a link took you to: go to the site yourself.",
                    ),
            )
            .child(
                row_button("link-sheet-back", "Go Back".into(), c.on_accent, true)
                    .w_full()
                    .bg(rgb(c.accent))
                    .on_click(choose(LinkChoice::Cancel)),
            )
            .child(
                row_button("link-sheet-open", "Open Anyway".into(), c.danger, true)
                    .w_full()
                    .border_1()
                    .border_color(rgb(c.danger))
                    .on_click(choose(LinkChoice::Open)),
            )
            // Not offered for a link with any danger named above (plain
            // http, a bare address, look-alike letters, a hidden site):
            // trusting it would stop the warnings for what made it suspect.
            .when(trustable, |d| {
                d.child(
                    row_button(
                        "link-sheet-trust",
                        format!("Always Trust {}", confirm.site),
                        c.text,
                        false,
                    )
                    .w_full()
                    .on_click(choose(LinkChoice::TrustAndOpen)),
                )
            })
            .child(
                row_button("link-sheet-copy", "Copy Link".into(), c.muted, false)
                    .w_full()
                    .on_click(choose(LinkChoice::Copy)),
            );
        let sheet = div()
            .w_full()
            .when(wide, |d| d.max_w(px(SHEET_WIDTH)))
            .px_2()
            .pb(px(insets.below() + 8.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(card);
        Some(
            self.sheet_backdrop("link-sheet-backdrop")
                // Darker than other sheets', and reddened.
                .bg(rgba(0x2a050acc))
                .on_click(choose(LinkChoice::Cancel))
                .child(sheet.with_animation(
                    ("link-sheet", confirm.key),
                    Animation::new(Duration::from_millis(260))
                        .with_easing(|t| 1. - (1. - t).powi(3)),
                    |d, t| d.relative().top(px((1. - t) * 200.)).opacity(t),
                ))
                .into_any_element(),
        )
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
