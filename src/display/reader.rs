//! Reader view components.
use super::*;

impl MailApp {
    pub(super) fn reading_options(&self, mail: &Mail) -> ReadingOptions {
        self.backend
            .store
            .preference(&message_key(&mail.account, &mail.id))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(self.reading_defaults)
    }

    pub(super) fn reader(
        &self,
        corners: Corners<Pixels>,
        framed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = self.colors();
        let Some(m) = self.selected_mail() else {
            self.html_view.update(cx, |view, cx| view.clear(cx));
            return div()
                .id("reader-pane")
                .debug_selector(|| "reader-pane".into())
                .relative()
                .flex_1()
                .min_w_0()
                .bg(rgb(c.surface))
                .rounded_bl(corners.bottom_left)
                .rounded_br(corners.bottom_right)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .when(framed, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(10.))
                            .left(px(8.))
                            .child(self.reader_toggle(false, cx)),
                    )
                })
                .child(icon_el("mail", 36., c.border))
                .child("No message selected")
                .into_any_element();
        };
        let compact = window.viewport_size().width < px(1180.);
        let single = window.viewport_size().width < px(900.);
        let toolbar = div()
            .id("reader-toolbar")
            .debug_selector(|| "reader-toolbar".into())
            .h(px(57.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .when(framed, |d| {
                d.child(self.reader_toggle(false, cx))
                    .child(div().w(px(1.)).h(px(20.)).mx_1().bg(rgb(c.border)))
            })
            .when(single, |d| {
                d.child(self.button(
                    "reader-back",
                    "@Back to messages",
                    "arrow-left",
                    Action::BackToList,
                    false,
                    cx,
                ))
            })
            .child(self.button(
                "reply",
                if compact { "@Reply" } else { "Reply" },
                "reply",
                Action::Reply(false),
                false,
                cx,
            ))
            .child(self.button(
                "reply-all",
                if compact { "@Reply all" } else { "Reply all" },
                "reply-all",
                Action::Reply(true),
                false,
                cx,
            ))
            .child(self.button(
                "forward",
                if compact { "@Forward" } else { "Forward" },
                "forward",
                Action::Forward,
                false,
                cx,
            ))
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
            .min_w_0()
            .w_full()
            .px(px(if compact { 20. } else { 32. }))
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
                    .flex_wrap()
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
                                    .flex_wrap()
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
            self.html_view.update(cx, |view, cx| view.clear(cx));
            content = content.child(self.unlock_panel(&m.account, cx));
        } else {
            let options = self.reading_options(&m);
            content = content.child(
                div()
                    .id("email-rendering-controls")
                    .debug_selector(|| "email-rendering-controls".into())
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .mb_4()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.bg))
                    .child(
                        self.button(
                            "toggle-email-html",
                            if options.html { "HTML on" } else { "HTML off" },
                            "",
                            Action::ToggleHtml,
                            options.html,
                            cx,
                        )
                        .debug_selector(|| "toggle-email-html".into()),
                    )
                    .child(
                        self.button(
                            "toggle-email-remote",
                            if options.block_remote {
                                "Remote blocked"
                            } else {
                                "Remote allowed"
                            },
                            "",
                            Action::ToggleRemote,
                            !options.block_remote,
                            cx,
                        )
                        .debug_selector(|| "toggle-email-remote".into()),
                    ),
            );
            if options.html
                && let Some(html) = &m.html
            {
                self.html_view.update(cx, |view, cx| {
                    view.set_content(html, options.block_remote, cx)
                });
                content = content.child(self.html_view.clone());
            } else {
                self.html_view.update(cx, |view, cx| view.clear(cx));
                let body = m.body.clone().unwrap_or_else(|| {
                    if self.busy > 0 {
                        "Loading message…".into()
                    } else {
                        "This message has no text body.".into()
                    }
                });
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
            }
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
        }
        div()
            .id("reader-pane")
            .debug_selector(|| "reader-pane".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                super::panes::scroll_frame(
                    content.with_animation(
                        ("reader-enter", self.anim),
                        Animation::new(Duration::from_millis(220))
                            .with_easing(|t| 1. - (1. - t).powi(3)),
                        |d, t| d.opacity(t).top(px((1. - t) * 5.)),
                    ),
                    &self.reader_scroll,
                )
                .w_full(),
            )
            .into_any_element()
    }
}

impl MailApp {
    /// Locked messages use the same toolbar, scrolling region and pane sizing
    /// as readable mail. Keeping the shell shared prevents layout drift.
    fn unlock_panel(&self, account: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let unlocking = self.unlocking.contains(account);
        div()
            .id("unlock-panel")
            .debug_selector(|| "unlock-panel".into())
            .w_full()
            .min_w_0()
            .max_w(px(340.))
            .mx_auto()
            .mt_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(div().size(px(52.)).flex_shrink_0().rounded(px(16.)).bg(rgb(c.soft)).flex().items_center().justify_center().child(icon_el("lock",22.,c.accent)))
            .child(div().w_full().text_center().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("This message is encrypted"))
            .child(div().w_full().text_size(px(12.)).text_color(rgb(c.muted)).text_center().child("Enter your Pinal password to read encrypted mail on this computer. After a password reset, use your previous password or your recovery key."))
            .child(div().w_full().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child("Password or recovery key"))
            .child(div().id("unlock-field").debug_selector(|| "unlock-field".into()).w_full().min_w_0().child(field(Input::new(&self.unlock_input), c, unlocking).disabled(unlocking).w_full().h(px(39.)).text_size(px(13.))))
            .when_some(self.unlock_errors.get(account), |panel, error| panel.child(div().id("unlock-error").debug_selector(|| "unlock-error".into()).w_full().text_size(px(12.)).text_color(rgb(c.danger)).child(error.clone())))
            .child(self.button("unlock-mail", if unlocking { "Unlocking…" } else { "Unlock" }, "lock", Action::UnlockMail, true, cx).w_full().flex_shrink_0().when(unlocking, |d| d.opacity(0.7)))
            .into_any_element()
    }
}
