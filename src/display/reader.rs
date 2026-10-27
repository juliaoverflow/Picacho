//! Reader view components.
use super::*;

impl MailApp {
    pub(super) fn reader(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let Some(m) = self.selected_mail() else {
            return div()
                .flex_1()
                .min_w_0()
                .bg(rgb(c.surface))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(icon_el("mail", 36., c.border))
                .child("No message selected")
                .into_any_element();
        };
        let toolbar = div()
            .h(px(57.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(self.button("reply", "Reply", "reply", Action::Reply(false), false, cx))
            .child(self.button(
                "reply-all",
                "Reply all",
                "reply-all",
                Action::Reply(true),
                false,
                cx,
            ))
            .child(self.button("forward", "Forward", "forward", Action::Forward, false, cx))
            .child(div().flex_1())
            .child(self.button(
                "archive",
                "@Archive message",
                "archive",
                Action::Move(Folder::Archive),
                false,
                cx,
            ))
            .child(self.button(
                "read",
                "@Mark read / unread",
                "mail",
                Action::Read(m.id.clone()),
                false,
                cx,
            ))
            .child(self.button(
                "trash",
                "@Move to trash",
                "trash",
                Action::Move(Folder::Trash),
                false,
                cx,
            ));
        let mut content = div()
            .id(SharedString::from(format!("reader-{}", m.id)))
            .track_scroll(&self.reader_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px(px(32.))
            .pt_6()
            .pb_10()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .mb_5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(23.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(relative(1.35))
                            .child(m.subject.clone()),
                    )
                    .child(
                        self.button(
                            "reader-star",
                            if m.starred {
                                "@Unstar · ⌘⇧S"
                            } else {
                                "@Star · ⌘⇧S"
                            },
                            "",
                            Action::Star(m.id.clone()),
                            false,
                            cx,
                        )
                        .child(if m.starred {
                            icon_el("star-filled", 17., STAR)
                        } else {
                            icon_el("star", 17., c.muted)
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .mb_6()
                    .child(avatar(&m.initials(), 40., 0, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(m.sender.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(c.muted))
                                            .truncate()
                                            .child(format!("<{}>", m.from)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(format!("to {}", m.to)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(c.muted))
                            .child(date_full(m.date)),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(c.border)).mb_6());
        if m.body.is_none() && self.locked.contains(&m.account) {
            return content
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .mt_6()
                        .max_w(px(340.))
                        .mx_auto()
                        .child(
                            div()
                                .size(px(52.))
                                .rounded(px(16.))
                                .bg(rgb(c.soft))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon_el("lock", 22., c.accent)),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("This message is encrypted"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .text_center()
                                .child(
                                    "Enter your Pinal password to read encrypted mail on this \
                                     computer. After a password reset, use your previous password \
                                     or your recovery key.",
                                ),
                        )
                        .child(
                            Input::new(&self.unlock_input)
                                .w_full()
                                .h(px(39.))
                                .text_size(px(13.)),
                        )
                        .child(
                            self.button(
                                "unlock-mail",
                                "Unlock",
                                "lock",
                                Action::UnlockMail,
                                true,
                                cx,
                            )
                            .w_full(),
                        ),
                )
                .into_any_element();
        }
        let body = m.body.unwrap_or_else(|| {
            if self.busy > 0 {
                "Loading message…".into()
            } else {
                "This message has no text body.".into()
            }
        });
        // Native selectable text preserves paragraphs without running untrusted HTML.
        content = content.child(
            gpui_component::text::TextView::markdown(
                SharedString::from(format!("body-{}", m.id)),
                plain_markdown(&body),
                window,
                cx,
            )
            .selectable(true)
            .text_size(px(14.))
            .line_height(relative(1.8)),
        );
        for (i, a) in m.attachments.iter().enumerate() {
            content = content.child(
                div()
                    .mt_3()
                    .border_1()
                    .border_color(rgb(c.border))
                    .rounded_lg()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(icon_el("paperclip", 20., c.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .truncate()
                            .child(format!("{}  ·  {} KB", a.name, a.size / 1024)),
                    )
                    .child(self.button(
                        format!("attachment-{i}"),
                        "Save",
                        "download",
                        Action::SaveAttachment(i),
                        false,
                        cx,
                    )),
            );
        }
        content = content.child(
            div()
                .mt_10()
                .pt_5()
                .border_t_1()
                .border_color(rgb(c.border))
                .flex()
                .gap_2()
                .child(
                    self.button(
                        "reply-bottom",
                        "Reply",
                        "reply",
                        Action::Reply(false),
                        false,
                        cx,
                    )
                    .border_1()
                    .border_color(rgb(c.border)),
                )
                .child(
                    self.button(
                        "forward-bottom",
                        "Forward",
                        "forward",
                        Action::Forward,
                        false,
                        cx,
                    )
                    .border_1()
                    .border_color(rgb(c.border)),
                ),
        );
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(c.surface))
            .flex()
            .flex_col()
            .child(toolbar)
            .child(content.with_animation(
                ("reader-enter", self.anim),
                Animation::new(Duration::from_millis(220)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t).top(px((1. - t) * 5.)),
            ))
            .into_any_element()
    }
}
