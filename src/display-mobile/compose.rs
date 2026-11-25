//! Writing a message on a phone: the shared composer, covering the screen.
//! Its fields and editor are the desktop's; the header becomes a navigation
//! bar (Cancel, the title) and the footer, with Send, rides on top of the
//! keyboard where a thumb reaches it.
use super::composer::ComposerEvent;
use super::*;

impl MailApp {
    /// The open composer as a full-screen cover, sliding up from the bottom.
    pub(super) fn mobile_composers(&self, insets: Insets, cx: &Context<Self>) -> Vec<AnyElement> {
        let c = self.colors();
        let Some(composer) = self.mobile_composer(cx) else {
            return vec![];
        };
        let key = composer.entity_id().as_u64();
        vec![
            div()
                .id(("mobile-composer", key))
                .debug_selector(|| "compose-panel".into())
                .absolute()
                .inset_0()
                .occlude()
                .bg(rgb(c.surface))
                .pt(px(insets.top))
                .pl(px(insets.left))
                .pr(px(insets.right))
                // The footer, with Send, stays right above the keyboard.
                .pb(px(insets.below()))
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().flex().flex_col().child(composer))
                .with_animation(
                    ("mobile-composer-enter", key),
                    Animation::new(PUSH).with_easing(|t| 1. - (1. - t).powi(3)),
                    // Rises at its full height rather than growing into it.
                    move |d, t| {
                        d.top(px((1. - t) * 120.))
                            .bottom(px(-(1. - t) * 120.))
                            .opacity(t * 0.4 + 0.6)
                    },
                )
                .into_any_element(),
        ]
    }
}

impl Composer {
    /// The composer's bar on a phone: Cancel (keeping the draft), the
    /// message's title and its preview switch.
    pub(in crate::display::ui) fn mobile_header(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors;
        let title = self.title(cx);
        let formatted = self.draft.formatted;
        div()
            .id("compose-header")
            .debug_selector(|| "compose-header".into())
            .h(px(NAV_BAR))
            .flex_shrink_0()
            .px_1()
            .relative()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .absolute()
                    .left(px(TOUCH * 2.))
                    .right(px(TOUCH * 2.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .id("compose-cancel")
                    .debug_selector(|| "compose-cancel".into())
                    .h(px(TOUCH))
                    .px_3()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(17.))
                    .text_color(rgb(c.accent))
                    .active(|s| s.opacity(0.5))
                    .child(t!("mobile-compose-cancel"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Close))),
            )
            .child(div().flex_1())
            .when(formatted, |d| {
                d.child(
                    div()
                        .id("compose-preview-toggle")
                        .h(px(TOUCH))
                        .px_3()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(17.))
                        .text_color(rgb(c.accent))
                        .active(|s| s.opacity(0.5))
                        .child(if self.previewing() { t!("mobile-compose-edit") } else { t!("mobile-compose-preview") })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_preview(cx))),
                )
            })
    }
}
