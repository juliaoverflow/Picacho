//! Reader view components.
use super::*;

impl MailApp {
    /// The message's saved choices or the defaults. The open message's are
    /// read once, not on every frame.
    pub(super) fn reading_options(&self, mail: &Mail) -> ReadingOptions {
        if let Some((id, options)) = &*self.options_cache.borrow()
            && id == &mail.id
        {
            return *options;
        }
        let options = self.read_options(mail);
        *self.options_cache.borrow_mut() = Some((mail.id.clone(), options));
        options
    }
    fn read_options(&self, mail: &Mail) -> ReadingOptions {
        let Some(saved) = self
            .backend
            .store
            .preference(&message_key(&mail.account, &mail.id))
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        else {
            return self.reading_defaults;
        };
        // Choices saved before emails had a theme follow the default theme.
        let theme = saved
            .get("theme")
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or(self.reading_defaults.theme);
        serde_json::from_value::<ReadingOptions>(saved)
            .map(|options| ReadingOptions { theme, ..options })
            .unwrap_or(self.reading_defaults)
    }
    /// Whether the open email's body is shown in dark colors.
    pub(super) fn email_dark(&self, options: ReadingOptions) -> bool {
        options.theme.is_dark(self.dark)
    }

    /// Scrolls a just-opened reader to the place picked in the collapsed
    /// reader's preview, once the message has its full height.
    pub(super) fn apply_reveal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(fraction) = self.reveal else {
            return;
        };
        if self.reader_collapsed || Self::pane_animating(self.reader_since) {
            return;
        }
        let max = f32::from(self.reader_scroll.max_offset().height);
        let viewport = f32::from(self.reader_scroll.bounds().size.height);
        if max <= 0. || self.html_view.read(cx).is_loading() {
            // Laid out on a later frame.
            window.request_animation_frame();
            return;
        }
        self.reveal = None;
        let target = (fraction * (max + viewport) - viewport * 0.3).clamp(0., max);
        self.reader_scroll.set_offset(point(px(0.), px(-target)));
        cx.notify();
    }

    pub(super) fn reader(
        &self,
        corners: Corners<Pixels>,
        framed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = self.colors();
        let Some(m) = self.selected_ref() else {
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
                "open-window",
                "@Open in new window · ⌘O",
                "popout",
                Action::OpenWindow(m.id.clone()),
                false,
                cx,
            ))
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
            // Narrow readers use narrow margins, wherever the window is.
            .px(px(if compact || self.reader_shown.get() < 600. {
                20.
            } else {
                32.
            }))
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
            .children(self.thread_cards(m, true, cx))
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
                            .child(self.details_toggle(m, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(c.muted))
                            .child(date_full(m.date)),
                    ),
            )
            .when(self.details_open, |d| d.child(self.details_panel(m, cx)))
            .children(self.unsubscribe_banner(m, cx));
        let locked = m.body.is_none() && self.locked.contains(&m.account);
        let options = self.reading_options(m);
        let dark = self.email_dark(options);
        // A full-width reader in a narrow window shows the controls as icons.
        let narrow = window.viewport_size().width < px(super::panes::READER_MIN);
        content = content
            .when(!locked, |d| {
                d.child(self.rendering_controls(m, options, narrow, cx))
            })
            .child(div().h(px(1.)).bg(rgb(c.border)).mb_6());
        if locked {
            self.html_view.update(cx, |view, cx| view.clear(cx));
            content = content.child(self.unlock_panel(&m.account, cx));
        } else {
            let has_quotes;
            if options.html
                && let Some(html) = &m.html
            {
                let hidden = self.html_without_quotes(m, html);
                has_quotes = hidden.is_some();
                let shown = match &hidden {
                    Some(hidden) if !self.quotes_open => hidden.as_str(),
                    _ => html.as_str(),
                };
                self.html_view.update(cx, |view, cx| {
                    view.set_content(shown, options.block_remote, dark, cx)
                });
                content = content.child(self.html_view.clone());
            } else {
                self.html_view.update(cx, |view, cx| view.clear(cx));
                let loading = m.body.is_none() && self.reading.as_ref() == Some(&m.id);
                let (body, quoted) = self.plain_body(m);
                has_quotes = quoted;
                // Plain text takes the email's theme; where that differs from
                // the app's, the text sits on a page of its own color.
                let page = (dark != self.dark).then_some(if dark {
                    (0x1f2023, 0xe8eaed)
                } else {
                    (0xffffff, 0x202124)
                });
                content = content
                    .when(loading, |d| d.child(loading_body(c)))
                    .when(!loading, |d| {
                        d.child(
                            div()
                                .id("plain-body")
                                .debug_selector(|| "plain-body".into())
                                .when_some(page, |d, (bg, fg)| {
                                    d.bg(rgb(bg)).text_color(rgb(fg)).rounded_lg().px_5().py_4()
                                })
                                .child(
                                    gpui_component::text::TextView::markdown(
                                        SharedString::from(format!(
                                            "body-{}-{}",
                                            m.id, self.quotes_open
                                        )),
                                        body,
                                        window,
                                        cx,
                                    )
                                    .selectable(true)
                                    // TextView fills its parent's height by default; in
                                    // the scrolling reader that is the visible area, so
                                    // long mail was cut off.
                                    .h_auto()
                                    .text_size(px(14.))
                                    .line_height(relative(1.8)),
                                ),
                        )
                    });
            }
            if has_quotes {
                content = content.child(div().flex().child(self.quotes_toggle(cx)));
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
            let later = self.thread_cards(m, false, cx);
            if !later.is_empty() {
                content = content.child(div().mt_8().children(later));
            }
            if let Some(reply) = self.inline_composer(&m.id) {
                content = content.child(
                    div()
                        .id("inline-reply")
                        .debug_selector(|| "inline-reply".into())
                        .mt_8()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgb(c.border))
                        .shadow_sm()
                        .overflow_hidden()
                        .child(reply),
                );
            } else {
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
                                "reply-all-bottom",
                                "Reply all",
                                "reply-all",
                                Action::Reply(true),
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
            content = content.child(self.source_links(cx));
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
                    // Only a newly opened message fades in. Rebuilding the
                    // reader (as when its pane finishes opening) must not
                    // replay the fade.
                    if self.open_at.is_some_and(|t| t.elapsed() < OPEN_FADE) {
                        content
                            .with_animation(
                                ("reader-enter", self.open_anim),
                                Animation::new(OPEN_FADE).with_easing(|t| 1. - (1. - t).powi(3)),
                                |d, t| d.opacity(t).top(px((1. - t) * 5.)),
                            )
                            .into_any_element()
                    } else {
                        content.into_any_element()
                    },
                    &self.reader_scroll,
                )
                .w_full(),
            )
            .into_any_element()
    }
}

const OPEN_FADE: Duration = Duration::from_millis(220);

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

/// A centered spinner where the body will appear.
fn loading_body(c: Colors) -> AnyElement {
    div()
        .id("body-loading")
        .debug_selector(|| "body-loading".into())
        .w_full()
        .h(px(160.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(spinner(24., c.muted))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child("Loading message…"),
        )
        .into_any_element()
}

impl MailApp {
    /// "to me ▾": opens the message's details.
    fn details_toggle(&self, m: &Mail, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let open = self.details_open;
        div()
            .id("message-details-toggle")
            .debug_selector(|| "message-details-toggle".into())
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .max_w_full()
            .px_1()
            .ml(px(-4.))
            .rounded(px(5.))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(rgb(c.muted))
            .hover(move |s| s.bg(rgb(c.raised)).text_color(rgb(c.text)))
            .child(div().min_w_0().truncate().child(self.recipients_line(m)))
            .child(div().flex_shrink_0().child(icon_el(
                if open { "chevron-up" } else { "chevron-down" },
                12.,
                c.muted,
            )))
            .tooltip(move |w, cx| {
                Tooltip::new(if open { "Hide details" } else { "Show details" }).build(w, cx)
            })
            .on_click(cx.listener(|this, _, w, cx| this.handle(Action::ToggleDetails, w, cx)))
            .into_any_element()
    }

    fn details_panel(&self, m: &Mail, cx: &Context<Self>) -> AnyElement {
        use crate::sync::mime::Security;
        let c = self.colors();
        let info = self.details.as_ref().filter(|d| d.id == m.id);
        let row = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .w(px(84.))
                        .flex_shrink_0()
                        .text_align(TextAlign::Right)
                        .text_color(rgb(c.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let text = |value: String| {
            div()
                .min_w_0()
                .overflow_hidden()
                .text_color(rgb(c.text))
                .child(value)
                .into_any_element()
        };
        let mut panel = div()
            .id("message-details")
            .debug_selector(|| "message-details".into())
            .mb_5()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.bg))
            .shadow_sm()
            .flex()
            .flex_col()
            .gap(px(7.))
            .text_size(px(12.));
        let from = if m.sender != m.from && !m.sender.is_empty() {
            format!("{} <{}>", m.sender, m.from)
        } else {
            m.from.clone()
        };
        let details = info
            .and_then(|i| i.result.as_ref())
            .and_then(|r| r.as_ref().ok());
        let header = |value: Option<&String>| value.filter(|v| !v.trim().is_empty()).cloned();
        let mut panel_rows: Vec<AnyElement> = vec![row("from:", text(from)).into_any_element()];
        if let Some(v) = header(details.map(|d| &d.details.reply_to)) {
            panel_rows.push(row("reply-to:", text(v)).into_any_element());
        }
        panel_rows.push(row("to:", text(m.to.clone())).into_any_element());
        if let Some(v) = header(details.map(|d| &d.details.cc)) {
            panel_rows.push(row("cc:", text(v)).into_any_element());
        }
        panel_rows.push(row("date:", text(date_full(m.date))).into_any_element());
        panel_rows.push(row("subject:", text(m.subject.clone())).into_any_element());
        match info.and_then(|i| i.result.as_ref()) {
            None => panel_rows.push(
                row(
                    "",
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(rgb(c.muted))
                        .child(spinner(13., c.muted))
                        .child("Checking the sender…")
                        .into_any_element(),
                )
                .into_any_element(),
            ),
            Some(Err(e)) => panel_rows.push(
                row(
                    "",
                    div()
                        .text_color(rgb(c.muted))
                        .child(e.clone())
                        .into_any_element(),
                )
                .into_any_element(),
            ),
            Some(Ok(original)) => {
                let d = &original.details;
                if let Some(domain) = &d.mailed_by {
                    panel_rows.push(row("mailed-by:", text(domain.clone())).into_any_element());
                }
                if !d.signed_by.is_empty() {
                    panel_rows
                        .push(row("signed-by:", text(d.signed_by.join(", "))).into_any_element());
                }
                let (icon, color) = match d.security {
                    Security::EndToEnd | Security::Tls => ("lock", 0x1e8e3e),
                    Security::Plain => ("warning", c.danger),
                    Security::Undecrypted | Security::Unknown => ("info", c.muted),
                };
                panel_rows.push(
                    row(
                        "security:",
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icon_el(icon, 13., color))
                            .child(div().text_color(rgb(c.text)).child(d.security.label()))
                            .into_any_element(),
                    )
                    .into_any_element(),
                );
                let checks: Vec<(&str, &Option<String>)> =
                    vec![("SPF", &d.spf), ("DKIM", &d.dkim), ("DMARC", &d.dmarc)];
                if !d.checked {
                    let checking = self.sender_checking.as_deref() == Some(m.id.as_str());
                    panel_rows.push(
                        row(
                            "verified:",
                            if checking {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(rgb(c.muted))
                                    .child(spinner(13., c.muted))
                                    .child("Checking the sender…")
                                    .into_any_element()
                            } else {
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        self.button(
                                            "check-sender",
                                            "Check sender",
                                            "",
                                            Action::CheckSender,
                                            false,
                                            cx,
                                        )
                                        .h(px(24.))
                                        .text_size(px(11.5))
                                        .border_1()
                                        .border_color(rgb(c.border)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(c.muted))
                                            .child("Looks up SPF, DKIM and DMARC; the sender's DNS can see when."),
                                    )
                                    .into_any_element()
                            },
                        )
                        .into_any_element(),
                    );
                } else if !checks.iter().any(|(_, v)| v.is_some()) {
                    panel_rows.push(
                        row("verified:", text("Couldn't check the sender (no DNS answer)".into()))
                            .into_any_element(),
                    );
                }
                if checks.iter().any(|(_, v)| v.is_some()) {
                    panel_rows.push(
                        row(
                            "verified:",
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .children(
                                    checks
                                        .into_iter()
                                        .filter_map(|(name, v)| v.as_ref().map(|v| (name, v)))
                                        .map(|(name, v)| verdict_chip(name, v, c)),
                                )
                                .into_any_element(),
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        panel = panel.children(panel_rows);
        panel
            .child(
                div()
                    .mt_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .flex()
                    .justify_end()
                    .child(
                        self.button(
                            "show-original",
                            "Show original",
                            "code",
                            Action::ShowOriginal,
                            false,
                            cx,
                        )
                        .h(px(30.))
                        .text_size(px(12.))
                        .border_1()
                        .border_color(rgb(c.border)),
                    ),
            )
            .map(|panel| {
                if self
                    .details_at
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(160))
                {
                    panel
                        .with_animation(
                            ("details-in", self.details_anim),
                            Animation::new(Duration::from_millis(160))
                                .with_easing(|t| 1. - (1. - t).powi(3)),
                            |d, t| d.opacity(t),
                        )
                        .into_any_element()
                } else {
                    panel.into_any_element()
                }
            })
    }

    /// One row above the body: the email's light or dark colors on the
    /// left; formatted or plain text, and remote images, on the right. It
    /// never wraps: the reader is at least as wide as the row, and a narrow
    /// window's full-width reader shows icons instead of labels.
    fn rendering_controls(
        &self,
        m: &Mail,
        options: ReadingOptions,
        narrow: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let showing_html = options.html && m.html.is_some();
        let dark = self.email_dark(options);
        let segment = |id: &'static str,
                       label: &'static str,
                       tip: &'static str,
                       icon: &'static str,
                       on: bool,
                       action: Action| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(26.))
                .when(label.is_empty() || narrow, |d| {
                    d.w(px(30.)).justify_center()
                })
                .when(!label.is_empty() && !narrow, |d| d.px(px(10.)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(7.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| {
                    d.bg(rgb(c.raised)).text_color(rgb(c.text)).shadow_sm()
                })
                .when(!on, |d| {
                    d.text_color(rgb(c.muted))
                        .hover(move |s| s.text_color(rgb(c.text)))
                })
                .child(icon_el(icon, 13., if on { c.text } else { c.muted }))
                .when(!label.is_empty() && !narrow, |d| d.child(label))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, w, cx| {
                    if !on {
                        this.handle(action.clone(), w, cx)
                    }
                }))
        };
        let group = |id: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(32.))
                .px(px(2.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(c.border))
        };
        let mut row = div()
            .id("email-rendering-controls")
            .debug_selector(|| "email-rendering-controls".into())
            .flex()
            .items_center()
            .gap_2()
            .mb_5()
            .child(
                group("toggle-email-theme")
                    .child(segment(
                        "email-light",
                        "",
                        "Light email",
                        "sun",
                        !dark,
                        Action::ToggleEmailTheme,
                    ))
                    .child(segment(
                        "email-dark",
                        "",
                        "Dark email",
                        "moon",
                        dark,
                        Action::ToggleEmailTheme,
                    )),
            )
            .child(div().flex_1());
        if m.html.is_some() {
            row = row.child(
                group("toggle-email-html")
                    .child(segment(
                        "show-formatted",
                        "Formatted",
                        "Show the sender's formatting",
                        "formatted",
                        showing_html,
                        Action::ShowHtml(true),
                    ))
                    .child(segment(
                        "show-plain",
                        "Plain text",
                        "Show the plain text version",
                        "text",
                        !showing_html,
                        Action::ShowHtml(false),
                    )),
            );
        }
        if showing_html {
            let allowed = !options.block_remote;
            row = row.child(
                div()
                    .id("toggle-email-remote")
                    .debug_selector(|| "toggle-email-remote".into())
                    .h(px(32.))
                    .flex_shrink_0()
                    .pl(px(10.))
                    .pr(px(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(c.text))
                    .hover(move |s| s.bg(rgb(c.raised)))
                    .tooltip(move |w, cx| {
                        Tooltip::new(if allowed {
                            "Remote images and styles load from the sender's servers"
                        } else {
                            "Remote images are blocked so senders can't tell you opened this"
                        })
                        .build(w, cx)
                    })
                    .child(icon_el(
                        if allowed { "image" } else { "image-off" },
                        14.,
                        if allowed { c.accent } else { c.muted },
                    ))
                    .when(!narrow, |d| d.child("Remote images"))
                    .child(switch(
                        allowed,
                        self.switch_anim,
                        self.switch_at
                            .is_some_and(|t| t.elapsed() < Duration::from_millis(300)),
                        c,
                    ))
                    .on_click(
                        cx.listener(|this, _, w, cx| this.handle(Action::ToggleRemote, w, cx)),
                    ),
            );
        }
        row.into_any_element()
    }

    /// Quiet links at the very bottom of a message to its headers, HTML
    /// and whole source.
    fn source_links(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let mut row = div()
            .id("message-source-links")
            .debug_selector(|| "message-source-links".into())
            .mt_6()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .text_color(rgb(c.muted));
        for (i, view) in SourceView::ALL.into_iter().enumerate() {
            if i > 0 {
                row = row.child("·");
            }
            let label = match view {
                SourceView::Headers => "View headers",
                SourceView::Html => "View HTML",
                SourceView::Original => "View original",
            };
            row = row.child(self.link(
                &format!("view-source-{}", view.label().to_lowercase()),
                label,
                Action::ShowSource(view),
                cx,
            ));
        }
        div().child(row)
    }

    pub(super) fn original_modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let info = self
            .details
            .as_ref()
            .filter(|d| Some(&d.id) == self.selected.as_ref());
        let body: AnyElement = match info.and_then(|i| i.result.as_ref()) {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner(24., c.muted))
                .into_any_element(),
            Some(Err(e)) => div()
                .flex_1()
                .p_5()
                .text_size(px(13.))
                .text_color(rgb(c.danger))
                .child(e.clone())
                .into_any_element(),
            Some(Ok(original)) => {
                let d = &original.details;
                let fact = |label: &'static str, value: String| {
                    div()
                        .flex()
                        .gap_3()
                        .min_w_0()
                        .child(
                            div()
                                .w(px(110.))
                                .flex_shrink_0()
                                .text_color(rgb(c.muted))
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(value))
                };
                let checked = d.checked;
                let verdict = |v: &Option<String>, domain: Option<&String>| match (v, domain) {
                    (Some(v), _) if v == crate::sync::auth::UNVERIFIABLE => {
                        "Couldn't verify (the signing key may have changed since)".into()
                    }
                    (Some(v), Some(d)) => format!("{} with domain {d}", v.to_uppercase()),
                    (Some(v), None) => v.to_uppercase(),
                    (None, _) if !checked => "Not checked (use Check sender in the message's details)".into(),
                    (None, _) => "Not recorded".into(),
                };
                let view = self.source_view;
                let shown = original.view(view);
                let lines = shown.map(|(_, l)| l.clone()).unwrap_or_default();
                let mono = c;
                // Switches between the message's headers, HTML and source.
                let mut tabs = div().flex().gap_1().ml_auto();
                for v in SourceView::ALL {
                    let on = v == view;
                    tabs = tabs.child(
                        div()
                            .id(SharedString::from(format!("source-tab-{}", v.label().to_lowercase())))
                            .px_2()
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if on { c.text } else { c.muted }))
                            .when(on, |d| d.bg(rgb(c.soft)))
                            .hover(move |s| s.bg(rgb(c.soft)))
                            .child(v.label())
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.handle(Action::ShowSource(v), w, cx)
                            })),
                    );
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_5()
                            .py_4()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .text_size(px(12.))
                            .border_b_1()
                            .border_color(rgb(c.border))
                            .child(fact("Message ID", d.message_id.clone()))
                            .child(fact("Created at", d.date.clone()))
                            .child(fact("From", d.from.clone()))
                            .child(fact("To", d.to.clone()))
                            .child(fact("Subject", d.subject.clone()))
                            .child(fact("SPF", verdict(&d.spf, d.mailed_by.as_ref())))
                            .child(fact("DKIM", verdict(&d.dkim, d.signed_by.first())))
                            .child(fact("DMARC", verdict(&d.dmarc, None)))
                            .child(fact("Security", d.security.label().into()))
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        self.button(
                                            "copy-original",
                                            "Copy to clipboard",
                                            "copy",
                                            Action::CopyOriginal,
                                            false,
                                            cx,
                                        )
                                        .h(px(30.))
                                        .text_size(px(12.))
                                        .border_1()
                                        .border_color(rgb(c.border)),
                                    )
                                    .child(
                                        self.button(
                                            "save-original",
                                            "Download original",
                                            "download",
                                            Action::SaveOriginal,
                                            false,
                                            cx,
                                        )
                                        .h(px(30.))
                                        .text_size(px(12.))
                                        .border_1()
                                        .border_color(rgb(c.border)),
                                    )
                                    .child(tabs),
                            ),
                    )
                    .when(shown.is_none(), |d| {
                        d.child(
                            div()
                                .p_5()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child("This message has no HTML part; it is plain text."),
                        )
                    })
                    .when(shown.is_some(), |d| d.child(
                        div()
                            .id("original-source")
                            .debug_selector(|| "original-source".into())
                            .flex_1()
                            .min_h_0()
                            .bg(rgb(c.bg))
                            .child(
                                uniform_list("original-lines", lines.len(), move |range, _, _| {
                                    range
                                        .map(|i| {
                                            div()
                                                .h(px(17.))
                                                .px_5()
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .font_family("Menlo")
                                                .text_size(px(11.5))
                                                .text_color(rgb(mono.text))
                                                .child(lines[i].clone())
                                        })
                                        .collect()
                                })
                                .track_scroll(self.original_scroll.clone())
                                .size_full()
                                .py_3(),
                            ),
                    ))
                    .into_any_element()
            }
        };
        div()
            .id("original-message")
            .debug_selector(|| "original-message".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .into_any_element()
    }
}

/// A pass/fail chip for an authentication result.
fn verdict_chip(name: &str, verdict: &str, c: Colors) -> AnyElement {
    let (fg, bg) = match verdict {
        "pass" => (0x1e8e3e, 0x1e8e3e),
        "fail" | "softfail" | "permerror" => (c.danger, c.danger),
        _ => (c.muted, c.muted),
    };
    div()
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .bg(rgba((bg << 8) | 0x1f))
        .child(if verdict == crate::sync::auth::UNVERIFIABLE {
            format!("{name} couldn't verify (key may have changed)")
        } else {
            format!("{name} {verdict}")
        })
        .into_any_element()
}

/// A small on/off switch; the knob slides when it changes.
fn switch(on: bool, key: u64, animate: bool, c: Colors) -> AnyElement {
    let knob = div()
        .absolute()
        .top(px(2.))
        .size(px(12.))
        .rounded_full()
        .bg(rgb(0xffffff))
        .shadow_sm();
    let (from, to) = if on { (2., 14.) } else { (14., 2.) };
    let knob = if animate {
        knob.with_animation(
            ("switch-knob", key),
            Animation::new(Duration::from_millis(140)).with_easing(|t| 1. - (1. - t).powi(3)),
            move |d, t| d.left(px(from + (to - from) * t)),
        )
        .into_any_element()
    } else {
        knob.left(px(to)).into_any_element()
    };
    div()
        .w(px(28.))
        .h(px(16.))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(if on { c.accent } else { c.border }))
        .relative()
        .child(knob)
        .into_any_element()
}

impl MailApp {
    /// The open message's conversation in the loaded list, oldest first,
    /// including the message itself. Messages join by References, or by
    /// subject when at least one is a reply (so newsletters that reuse a
    /// subject stay apart).
    pub(super) fn thread(&self, m: &Mail) -> Rc<Vec<usize>> {
        if let Some((id, revision, rows)) = &*self.thread_cache.borrow()
            && id == &m.id
            && *revision == self.list_revision
        {
            return rows.clone();
        }
        let key = super::thread::conversation_subject(&m.subject);
        let replyish = |x: &Mail, key: &str| {
            !x.references.is_empty() || x.subject.trim().to_lowercase() != key
        };
        let linked = |a: &Mail, b: &Mail| {
            !b.message_id.is_empty() && a.references.contains(b.message_id.as_str())
        };
        let mut seen = HashSet::new();
        let mut rows: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, x)| x.account == m.account && !x.pending)
            .filter(|(_, x)| {
                x.id == m.id
                    || linked(x, m)
                    || linked(m, x)
                    || (key.chars().count() > 2
                        && super::thread::conversation_subject(&x.subject) == key
                        && (replyish(x, &key) || replyish(m, &key)))
            })
            // The same message under several labels or folders shows once.
            .filter(|(_, x)| x.message_id.is_empty() || seen.insert(x.message_id.clone()))
            .map(|(i, _)| i)
            .collect();
        rows.sort_by_key(|i| self.messages[*i].date);
        let rows = Rc::new(rows);
        *self.thread_cache.borrow_mut() = Some((m.id.clone(), self.list_revision, rows.clone()));
        rows
    }

    /// Cards for the conversation's other messages, before or after `m`.
    /// A long history folds its middle behind a "more" row.
    fn thread_cards(&self, m: &Mail, before: bool, cx: &Context<Self>) -> Vec<AnyElement> {
        let c = self.colors();
        let rows = self.thread(m);
        if rows.len() < 2 {
            return vec![];
        }
        let position = rows
            .iter()
            .position(|i| self.messages[*i].id == m.id)
            .unwrap_or(rows.len());
        let side: Vec<usize> = if before {
            rows[..position.min(rows.len())].to_vec()
        } else {
            rows.get(position + 1..)
                .map(<[usize]>::to_vec)
                .unwrap_or_default()
        };
        let mut out = vec![];
        if before {
            out.push(
                div()
                    .id("thread-label")
                    .debug_selector(|| "thread-label".into())
                    .mb_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child(icon_el("thread", 13., c.muted))
                    .child(format!("CONVERSATION · {} MESSAGES", rows.len()))
                    .into_any_element(),
            );
        }
        let own: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        let card =
            |index: usize| {
                let x = &self.messages[index];
                let id = x.id.clone();
                let to = super::thread::short_names(&x.to, &own);
                div()
                    .id(SharedString::from(format!("thread-{}", x.id)))
                    .debug_selector(|| "thread-card".into())
                    .mb_2()
                    .px_3()
                    .py(px(9.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.bg))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.raised)))
                    .child(avatar(&x.initials(), 28., index, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(13.))
                                            .font_weight(if x.unread {
                                                FontWeight::BOLD
                                            } else {
                                                FontWeight::SEMIBOLD
                                            })
                                            .child(x.sender.clone()),
                                    )
                                    .when(!to.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(11.))
                                                .text_color(rgb(c.muted))
                                                .child(format!("to {}", to.join(", "))),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(super::preview(&x.snippet)),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(date_short(x.date)),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Select(id.clone()), w, cx)
                    }))
                    .into_any_element()
            };
        if before && side.len() > 4 && !self.thread_open {
            out.push(card(side[0]));
            let hidden = side.len() - 3;
            out.push(
                div()
                    .id("thread-more")
                    .debug_selector(|| "thread-more".into())
                    .mb_2()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.))
                    .border_1()
                    .border_dashed()
                    .border_color(rgb(c.border))
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.raised)).text_color(rgb(c.text)))
                    .child(format!("{hidden} more messages"))
                    .on_click(
                        cx.listener(|this, _, w, cx| this.handle(Action::ExpandThread, w, cx)),
                    )
                    .into_any_element(),
            );
            out.extend(side[side.len() - 2..].iter().map(|i| card(*i)));
        } else {
            out.extend(side.iter().map(|i| card(*i)));
        }
        if before {
            out.push(
                div()
                    .h(px(1.))
                    .bg(rgb(c.border))
                    .mt_2()
                    .mb_5()
                    .into_any_element(),
            );
        }
        out
    }

    /// "to Ana, Bo, me · cc Cy +2": who received the message, briefly.
    fn recipients_line(&self, m: &Mail) -> String {
        let own: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        let brief = |names: Vec<String>, keep: usize| {
            if names.len() > keep {
                format!("{} +{}", names[..keep].join(", "), names.len() - keep)
            } else {
                names.join(", ")
            }
        };
        let to = super::thread::short_names(&m.to, &own);
        let cc = super::thread::short_names(&m.cc, &own);
        let mut line = if to.is_empty() {
            "to undisclosed recipients".to_string()
        } else {
            format!("to {}", brief(to, 3))
        };
        if !cc.is_empty() {
            line.push_str(&format!(" · cc {}", brief(cc, 2)));
        }
        line
    }

    /// The open HTML email with its quoted history hidden, if it has one.
    /// Worked out once per message.
    fn html_without_quotes(&self, m: &Mail, html: &str) -> Option<Rc<String>> {
        // The body can arrive after the message opens: key by its length too.
        let key = format!("{}:{}", m.id, html.len());
        if let Some((k, hidden)) = &*self.quote_cache.borrow()
            && k == &key
        {
            return hidden.clone();
        }
        let hidden = super::thread::hide_html_quotes(html).map(Rc::new);
        *self.quote_cache.borrow_mut() = Some((key, hidden.clone()));
        hidden
    }

    /// The plain body as the reader's Markdown, without the quoted history
    /// unless it is open; and whether there is quoted history.
    fn plain_body(&self, m: &Mail) -> (SharedString, bool) {
        let key = format!(
            "{}:{}:{}",
            m.id,
            self.quotes_open,
            m.body.as_ref().map_or(0, String::len)
        );
        if let Some((k, text, quoted)) = &*self.plain_cache.borrow()
            && k == &key
        {
            return (text.clone(), *quoted);
        }
        let body = m
            .body
            .clone()
            .unwrap_or_else(|| "This message has no text body.".into());
        let (written, quoted) = super::thread::split_quoted(&body);
        let shown = if self.quotes_open {
            body.as_str()
        } else {
            written
        };
        let text = SharedString::from(plain_markdown(shown));
        *self.plain_cache.borrow_mut() = Some((key, text.clone(), quoted.is_some()));
        (text, quoted.is_some())
    }

    /// The "•••" pill that shows or hides a reply's quoted history.
    fn quotes_toggle(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let open = self.quotes_open;
        div()
            .id("toggle-quotes")
            .debug_selector(|| "toggle-quotes".into())
            .mt_3()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(7.))
            .bg(rgb(c.raised))
            .border_1()
            .border_color(rgb(c.border))
            .text_size(px(11.))
            .text_color(rgb(c.muted))
            .cursor_pointer()
            .hover(move |s| s.text_color(rgb(c.text)))
            .child(div().font_weight(FontWeight::BOLD).child("•••"))
            .child(if open {
                "Hide quoted text"
            } else {
                "Show quoted text"
            })
            .on_click(cx.listener(|this, _, w, cx| this.handle(Action::ToggleQuotes, w, cx)))
            .into_any_element()
    }
}
