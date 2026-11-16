//! Questions asked with `window.prompt` (Unsubscribe, Send anyway?) are
//! drawn as the app's own dialogs, over the window that asked, instead of
//! the system's alerts.
use super::*;

thread_local! {
    /// The palette prompts are drawn in; kept current by `apply_theme`.
    static PROMPT_COLORS: Cell<Option<Colors>> = const { Cell::new(None) };
}
pub(super) fn set_colors(c: Colors) {
    PROMPT_COLORS.with(|p| p.set(Some(c)));
}
fn colors() -> Colors {
    PROMPT_COLORS
        .with(|p| p.get())
        .unwrap_or_else(|| Colors::new(false, Accent::Green))
}

/// Draws every prompt in the app as one of its dialogs.
pub(super) fn install(cx: &mut App) {
    cx.set_prompt_builder(|_level, message, detail, answers, handle, window, cx| {
        let view = cx.new(|cx| AppPrompt {
            message: message.to_owned().into(),
            detail: detail.map(|d| d.to_owned().into()),
            answers: answers.iter().map(|a| a.label().clone()).collect(),
            focus: cx.focus_handle(),
        });
        handle.with_view(view, window, cx)
    });
}

struct AppPrompt {
    message: SharedString,
    detail: Option<SharedString>,
    /// In the order asked; the first is the one the question offers.
    answers: Vec<SharedString>,
    focus: FocusHandle,
}
impl EventEmitter<PromptResponse> for AppPrompt {}
impl Focusable for AppPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl AppPrompt {
    /// The answer that changes nothing: Cancel, or else the last one.
    fn dismissal(&self) -> usize {
        self.answers
            .iter()
            .position(|a| a.eq_ignore_ascii_case("cancel"))
            .unwrap_or(self.answers.len().saturating_sub(1))
    }
}
impl Render for AppPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let corners = window::window_corners(window);
        // GPUI lays prompts over the whole window, not inside the app's
        // frame: they cover the frame, clear of the shadow band a window
        // drawing its own decorations has around it.
        let frame = window::content_bounds(window);
        let dismissal = self.dismissal();
        let button = |index: usize, label: SharedString, primary: bool| {
            div()
                .id(("prompt-answer", index))
                .debug_selector(move || format!("prompt-answer-{index}"))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .h(px(36.))
                .px_3()
                .rounded(px(9.))
                .cursor_pointer()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .bg(rgb(if primary { c.accent } else { c.surface }))
                .text_color(rgb(if primary { c.on_accent } else { c.muted }))
                .hover(move |s| s.bg(rgb(if primary { c.accent_hover } else { c.raised })))
                .active(|s| s.opacity(0.75))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(PromptResponse(index))))
                .child(label)
        };
        // The offered answer is the primary button, on the right as in the
        // app's other dialogs; the rest sit to its left.
        let mut footer = div()
            .flex_shrink_0()
            .flex_wrap()
            .border_t_1()
            .border_color(rgb(c.border))
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1());
        for (i, answer) in self.answers.iter().enumerate().skip(1).rev() {
            footer = footer.child(button(i, answer.clone(), false));
        }
        if let Some(first) = self.answers.first() {
            footer = footer.child(button(0, first.clone(), true));
        }
        let panel = div()
            .id("prompt-panel")
            .debug_selector(|| "prompt-panel".into())
            .flex()
            .flex_col()
            .w(px(400.))
            .max_w((frame.size.width - px(32.)).max(px(0.)))
            .max_h((frame.size.height - px(32.)).max(px(0.)))
            .rounded(px(18.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.surface))
            .text_color(rgb(c.text))
            .shadow_2xl()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_5()
                    .py_3()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.message.clone()),
                    )
                    .child(
                        div()
                            .id("prompt-close")
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(34.))
                            .rounded(px(9.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(c.raised)))
                            .on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(PromptResponse(dismissal))),
                            )
                            .child(icon_el("close", 16., c.muted)),
                    ),
            )
            .when_some(self.detail.clone(), |d, detail| {
                d.child(
                    div()
                        .id("prompt-detail")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        // Paragraphs come separated by blank lines.
                        .children(
                            detail
                                .split("\n\n")
                                .map(|p| div().child(p.trim().to_owned()))
                                .collect::<Vec<_>>(),
                        ),
                )
            })
            .child(footer);
        div()
            .id("app-prompt")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => cx.emit(PromptResponse(dismissal)),
                    "enter" => cx.emit(PromptResponse(0)),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .occlude()
            .absolute()
            .left(frame.origin.x)
            .top(frame.origin.y)
            .w(frame.size.width)
            .h(frame.size.height)
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .bg(rgba(0x101c2c66))
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.))
            .font_family(".SystemUIFont")
            .child(panel.with_animation(
                "prompt-enter",
                Animation::new(Duration::from_millis(180)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t).mt(px((1. - t) * 16.)),
            ))
    }
}
