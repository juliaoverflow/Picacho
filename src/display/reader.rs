//! Reader view components.
use super::*;

impl MailApp {
    /// Takes the reader's place while several messages are selected: how
    /// many, from whom, and the actions for all of them.
    fn selection_summary(
        &self,
        corners: Corners<Pixels>,
        framed: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let picked: Vec<&Mail> = self
            .messages
            .iter()
            .filter(|m| self.selection.contains(&m.id))
            .collect();
        let n = picked.len();
        let unread = picked.iter().filter(|m| m.unread).count();
        let all_starred = n > 0 && picked.iter().all(|m| m.starred);
        // The senders, most frequent first (counted in a map: a list
        // searched for each message was slow with thousands selected).
        let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
        for (i, m) in picked.iter().enumerate() {
            let name = if m.sender.trim().is_empty() { &m.from } else { &m.sender };
            counts.entry(name.as_str()).or_insert((0, i)).0 += 1;
        }
        // Ties keep the order they were first seen in, as before.
        let mut senders: Vec<(String, usize, usize)> =
            counts.into_iter().map(|(name, (n, first))| (name.to_owned(), n, first)).collect();
        senders.sort_by_key(|(_, count, first)| (std::cmp::Reverse(*count), *first));
        let senders: Vec<(String, usize)> = senders.into_iter().map(|(s, n, _)| (s, n)).collect();
        let shown = senders.len().min(3);
        let mut who = senders[..shown]
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if senders.len() > shown {
            let more = senders.len() - shown;
            who = t!("reader-names-and-more", names = who, count = more, shown = fmt_count(more));
        }
        let action = |id: &str, label: &str, icon: &str, change: Change| {
            self.button(id.to_owned(), label, icon, Action::ApplySelection(change), false, cx)
                .w(px(220.))
                .justify_start()
                .bg(rgb(c.bg))
        };
        div()
            .id("reader-pane")
            .role(Role::Main)
            .aria_label(t!("reader-message"))
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
            .gap_2()
            .p_6()
            .when(framed, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .left(px(8.))
                        .child(self.reader_toggle(false, cx)),
                )
            })
            .child(icon_el("select", 36., c.accent))
            .child(
                div()
                    .id("selection-summary")
                    .debug_selector(|| "selection-summary".into())
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("reader-messages-selected", shown = fmt_count(n), count = n)),
            )
            .child(
                div()
                    .max_w(px(360.))
                    .text_center()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if unread > 0 {
                        t!("reader-unread-from", shown = fmt_count(unread), who = who, count = unread)
                    } else {
                        t!("reader-from", who = who)
                    }),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(if unread > 0 {
                        action("summary-read", t!("reader-mark-as-read"), "mail-open", Change::Read(true))
                    } else {
                        action("summary-unread", t!("reader-mark-as-unread"), "mail", Change::Read(false))
                    })
                    .child(if all_starred {
                        action("summary-unstar", t!("reader-unstar"), "star", Change::Star(false))
                    } else {
                        action("summary-star", t!("reader-star"), "star", Change::Star(true))
                    })
                    .when(self.folder != Folder::Archive, |d| {
                        d.child(action(
                            "summary-archive",
                            t!("reader-archive"),
                            "archive",
                            Change::Move(Folder::Archive),
                        ))
                    })
                    .when(self.folder != Folder::Inbox, |d| {
                        d.child(action(
                            "summary-inbox",
                            t!("reader-move-to-inbox"),
                            "inbox",
                            Change::Move(Folder::Inbox),
                        ))
                    })
                    .when(self.folder != Folder::Junk, |d| {
                        d.child(action(
                            "summary-spam",
                            t!("reader-move-to-spam"),
                            "spam",
                            Change::Move(Folder::Junk),
                        ))
                    })
                    .when(self.folder != Folder::Trash, |d| {
                        d.child(action(
                            "summary-trash",
                            t!("reader-move-to-trash"),
                            "trash",
                            Change::Move(Folder::Trash),
                        ))
                    })
                    .when(
                        self.move_folder_entry("", MoveTarget::Selection).is_some(),
                        |d| {
                            d.child(
                                self.button(
                                    "summary-folder",
                                    t!("reader-move-to-folder"),
                                    "folder",
                                    Action::MoveMenu(MoveTarget::Selection),
                                    false,
                                    cx,
                                )
                                .w(px(220.))
                                .justify_start()
                                .bg(rgb(c.bg)),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .gap_1()
                    .child(
                        self.button(
                            "summary-done",
                            t!("reader-clear-selection"),
                            "",
                            Action::StopSelecting,
                            false,
                            cx,
                        )
                        .text_size(px(12.))
                        .h(px(30.)),
                    ),
            )
    }
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
        let max = f32::from(self.reader_scroll.max_offset().y);
        let viewport = f32::from(self.reader_scroll.bounds().size.height);
        if self.html_view.read(cx).is_loading() {
            // Laid out on a later frame.
            window.request_animation_frame();
            return;
        }
        if max <= 0. {
            // Its height can reach the scroll a frame or two after it's laid
            // out. A message that never scrolls (it fits) gives up after a
            // second: it asked for a frame on every frame, for as long as it
            // stayed open.
            self.reveal_waited += 1;
            if self.reveal_waited > 60 {
                self.reveal = None;
            } else {
                window.request_animation_frame();
            }
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
        // Several messages picked: what can be done with all of them.
        if self.selection.len() > 1 && !self.subs_open {
            return self.selection_summary(corners, framed, cx).into_any_element();
        }
        let Some(m) = self.selected_ref() else {
            self.html_view.update(cx, |view, cx| view.clear(cx));
            return div()
                .id("reader-pane")
            .role(Role::Main)
            .aria_label(t!("reader-message"))
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
                .child(t!("reader-no-message-selected"))
                .into_any_element();
        };
        let width = super::window::content_size(window).width;
        let compact = width < px(1180.);
        let single = width < px(900.);
        let toolbar = div()
            .id("reader-toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("reader-message-actions"))
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
                    t!("reader-back-to-messages"),
                    "arrow-left",
                    Action::BackToList,
                    false,
                    cx,
                ))
            })
            .child(self.button(
                "reply",
                &if compact { shortcuts::tip("@Reply", "reply", cx) } else { t!("reader-reply").into() },
                "reply",
                Action::Reply(false),
                false,
                cx,
            ))
            .child(self.button(
                "reply-all",
                &if compact { shortcuts::tip(t!("reader-reply-all"), "reply-all", cx) } else { t!("reader-reply-all-2").into() },
                "reply-all",
                Action::Reply(true),
                false,
                cx,
            ))
            .child(self.button(
                "forward",
                &if compact { shortcuts::tip("@Forward", "forward", cx) } else { t!("reader-forward").into() },
                "forward",
                Action::Forward,
                false,
                cx,
            ))
            .child(div().flex_1())
            .child(self.button(
                "print",
                &shortcuts::tip("@Print", "print", cx),
                "print",
                Action::Tools(super::tools::ToolAction::Print(m.id.clone())),
                false,
                cx,
            ))
            .child(self.button(
                "open-window",
                &shortcuts::tip(t!("reader-open-in-new-window"), "open-in-window", cx),
                "popout",
                Action::OpenWindow(m.id.clone()),
                false,
                cx,
            ))
            .child(self.button(
                "archive",
                &shortcuts::tip(t!("reader-archive-message"), "archive", cx),
                "archive",
                Action::Move(Folder::Archive),
                false,
                cx,
            ))
            .when(
                self.account_of(m).is_some_and(|a| a.provider != Provider::Pop3)
                    && !matches!(m.folder, Folder::Drafts | Folder::Sent),
                |d| {
                    d.child(self.button(
                        "snooze",
                        t!("reader-snooze-until"),
                        "snooze",
                        Action::SnoozeMenu(vec![m.id.clone()]),
                        false,
                        cx,
                    ))
                },
            )
            .child(self.button(
                "read",
                &shortcuts::tip(t!("reader-mark-read-unread"), "toggle-read", cx),
                "mail",
                Action::Read(m.id.clone()),
                false,
                cx,
            ))
            .child(self.button(
                "trash",
                &shortcuts::tip(t!("reader-move-to-trash-2"), "delete", cx),
                "trash",
                Action::Move(Folder::Trash),
                false,
                cx,
            ));
        div()
            .id("reader-pane")
            .role(Role::Main)
            .aria_label(t!("reader-message"))
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
            .child(self.reader_body(m, window, cx))
            .into_any_element()
    }

    /// The open message's scrolling body: subject, sender, banners, the
    /// email itself, attachments and the conversation. The desktop reader
    /// and the phone's reader screen put their own bars around it.
    pub(super) fn reader_body(
        &self,
        m: &Mail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let compact = self.mobile.on || super::window::content_size(window).width < px(1180.);
        // A mailbox kept to its aliases shows the alias used in its place.
        let mask = self.mask_for(m);
        let (sender, from) = match &mask {
            Some(k) => (
                if k.sent { k.sender() } else { m.sender.clone() },
                k.apply(&m.from).into_owned(),
            ),
            None => (m.sender.clone(), m.from.clone()),
        };
        let mut content = div()
            .id(SharedString::from(format!("reader-{}", m.id)))
            .track_scroll(&self.reader_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            // Narrow readers use narrow margins, wherever the window is.
            .px(px(if self.mobile.on {
                super::mobile::GUTTER
            } else if compact || self.reader_shown.get() < 600. {
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
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(23.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_height(relative(1.35))
                                    .child(self.hide_codes(m, &m.subject).into_owned()),
                            )
                            .children(self.label_chips(m, cx)),
                    )
                    .child(
                        self.button(
                            "reader-star",
                            &shortcuts::tip(
                                if m.starred { "@Unstar" } else { "@Star" },
                                "toggle-star",
                                cx,
                            ),
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
                                    .id("reader-from")
                                    .debug_selector(|| "reader-from".into())
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .items_center()
                                    .on_mouse_down(MouseButton::Right, {
                                        let (name, email) = (sender.clone(), from.clone());
                                        let real = m.from.clone();
                                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            let entries =
                                                address_entries(Some(&name), &email, Some(&real));
                                            this.open_menu(e.position, entries, cx)
                                        })
                                    })
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(sender),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(c.muted))
                                            .truncate()
                                            .child(format!("<{from}>")),
                                    )
                                    .when_some(self.sent_alias(m), |d, alias| {
                                        d.child(
                                            div()
                                                .text_size(px(11.))
                                                .child(alias_tag("reader-sent-alias", alias, c)),
                                        )
                                    }),
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
            .children(self.snooze_banner(m, cx))
            .children(self.receipt_banner(m, cx))
            .children(self.unsubscribe_banner(m, cx));
        let locked = m.body.is_none() && self.locked.contains(&m.account);
        let options = self.reading_options(m);
        let dark = self.email_dark(options);
        // A full-width reader in a narrow window shows the controls as icons.
        let narrow =
            super::window::content_size(window).width < px(super::panes::READER_MIN);
        content = content
            .when(!locked, |d| {
                d.child(self.rendering_controls(m, options, narrow, cx))
            })
            .child(div().h(px(1.)).bg(rgb(c.border)).mb_6())
            .children((!locked).then(|| self.codes_banner(m, cx)).flatten())
            .children(if locked { Vec::new() } else { self.parcel_banners(m, cx) })
            .children((!locked).then(|| self.invitation_card(m, cx)).flatten())
            .children((!locked).then(|| self.delivery_report_card(m)).flatten())
            .children((!locked).then(|| self.setup_message_card(m, cx)).flatten())
            .children((!locked).then(|| self.carried_certificates_card(m, cx)).flatten());
        if !locked && !self.invitations.contains_key(&m.id) {
            let m = m.clone();
            let view = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = view.update(cx, |this, cx| {
                    this.check_invitation(&m, cx);
                    this.check_delivery_report(&m, cx);
                    this.complete_jmap_thread(&m, cx);
                });
            });
        }
        if self.find.open && !locked {
            content = content.child(self.find_bar(cx));
        }
        // Find in Message shows a plain email as HTML, to mark what it finds.
        let find_plain = (self.find_query(cx).is_some() && !(options.html && m.html.is_some()))
            .then(|| m.body.clone())
            .flatten();
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
                let masked = mask.as_ref().map(|k| k.apply(shown));
                let shown = masked.as_deref().unwrap_or(shown);
                let shown = self.hide_codes_html(m, shown);
                let marked = self.find_marked(shown.as_ref(), false, cx);
                let shown = marked.as_deref().unwrap_or(shown.as_ref());
                self.html_view.update(cx, |view, cx| {
                    view.set_content(shown, &m.inline, options.block_remote, dark, self.clean_links, cx)
                });
                if marked.is_some() {
                    self.reveal_find(cx);
                }
                content = content.child(self.html_view.clone());
            } else if let Some(text) = find_plain {
                has_quotes = false;
                let marked = self.find_marked(&text, true, cx).unwrap_or_default();
                self.html_view.update(cx, |view, cx| {
                    view.set_content(&marked, &m.inline, options.block_remote, dark, self.clean_links, cx)
                });
                self.reveal_find(cx);
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
                                        body)
                                    .selectable(true)
                                    // Its links open as the email's own do
                                    // (warned of on a phone).
                                    .on_link_click({
                                        let app = cx.weak_entity();
                                        move |url, _, _, cx| {
                                            let url = url.to_string();
                                            let _ = app.update(cx, |this, cx| {
                                                this.open_email_link(url, cx)
                                            });
                                        }
                                    })
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
            let previews = self.attached_previews.clone();
            for (i, a) in m.attachments().iter().enumerate() {
                // RFC 6532 §3.7: message/global is an attached message too.
                let is_message = a.mime.eq_ignore_ascii_case("message/rfc822")
                    || a.mime.eq_ignore_ascii_case("message/global")
                    || a.name.to_ascii_lowercase().ends_with(".eml");
                let preview = previews.get(&(m.id.clone(), i)).cloned();
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
                                .child(format!("{}  ·  {}", a.name, human_size(a.size as u64))),
                        )
                        .when(is_message && preview.is_none(), |d| {
                            d.child(self.button(
                                format!("attachment-preview-{i}"),
                                t!("reader-preview"),
                                "preview",
                                Action::PreviewAttachment(i),
                                false,
                                cx,
                            ))
                        })
                        .children(self.attachment_buttons(i, a, cx))
                        .child(self.button(
                            format!("attachment-{i}"),
                            t!("reader-save"),
                            "download",
                            Action::SaveAttachment(i),
                            false,
                            cx,
                        )),
                );
                content = content.children(self.image_preview(i));
                if let Some(text) = preview {
                    content = content.child(
                        div()
                            .id(SharedString::from(format!("attachment-preview-text-{i}")))
                            .mt_1()
                            .p_3()
                            .rounded_lg()
                            .bg(rgb(c.raised))
                            .text_size(px(12.))
                            .whitespace_normal()
                            .children(text.lines().map(|l| div().child(l.to_owned())).collect::<Vec<_>>()),
                    );
                }
            }
            content = content.children(self.delivery_line(m, cx));
            let later = self.thread_cards(m, false, cx);
            if !later.is_empty() {
                content = content.child(div().mt_8().children(later));
            }
            if let Some(reply) = self.inline_composer(&m.id) {
                if self.reader_to_end.get() > 0 {
                    self.reader_to_end.set(self.reader_to_end.get() - 1);
                    self.reader_scroll.scroll_to_bottom();
                    window.refresh();
                }
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
                                t!("reader-reply"),
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
                                t!("reader-reply-all-2"),
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
                                t!("reader-forward"),
                                "forward",
                                Action::Forward,
                                false,
                                cx,
                            )
                            .border_1()
                            .border_color(rgb(c.border)),
                        ),
                );
                if let Some(chips) = self.smart_reply_chips(m, cx) {
                    content = content.child(chips);
                }
            }
            content = content.child(self.source_links(cx));
        }
        super::panes::scroll_frame(
            // Only a newly opened message fades in. Rebuilding the
            // reader (as when its pane finishes opening) must not
            // replay the fade.
            // A phone's message slides in; fading as well dims it mid-slide.
            if !self.mobile.on && self.open_at.is_some_and(|t| t.elapsed() < OPEN_FADE) {
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
        .w_full()
        // The hovered link's address, over the bottom left (desktop).
        .children(self.link_status())
        .into_any_element()
    }
}

const OPEN_FADE: Duration = Duration::from_millis(220);

impl MailApp {
    /// Locked messages use the same toolbar, scrolling region and pane sizing
    /// as readable mail. Keeping the shell shared prevents layout drift.
    /// Reads a downloaded message once for a delivery report (RFC 3464,
    /// RFC 6533), from its cached source: nothing is fetched for it.
    fn check_delivery_report(&mut self, m: &Mail, cx: &mut Context<Self>) {
        if self.delivery_reports.contains_key(&m.id) {
            return;
        }
        // Until its source is downloaded there's nothing to read; it is
        // looked at again once there is.
        let Some(report) = self.backend.delivery_report(m) else {
            return;
        };
        let found = report.is_some();
        self.delivery_reports.insert(m.id.clone(), report);
        if found {
            cx.notify();
        }
    }
    /// What a delivery report says of each recipient, above its text: who,
    /// what happened, the status code, and the server's words, muted.
    fn delivery_report_card(&self, m: &Mail) -> Option<AnyElement> {
        use crate::sync::dsn::Action as Outcome;
        let report = self.delivery_reports.get(&m.id)?.as_ref()?;
        let c = self.colors();
        let rows = report.recipients.iter().take(50).enumerate().map(|(i, r)| {
            let outcome = match r.action {
                Outcome::Failed => t!("reader-failed"),
                Outcome::Delayed => t!("reader-delayed"),
                Outcome::Delivered => t!("reader-delivered"),
                Outcome::Relayed => t!("reader-relayed"),
                Outcome::Expanded => t!("reader-expanded"),
                Outcome::Other => t!("reader-unknown"),
            };
            let status = r
                .status
                .map(|(class, subject, detail)| format!(" {class}.{subject}.{detail}"))
                .unwrap_or_default();
            div()
                .id(SharedString::from(format!("delivery-report-{i}")))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().font_weight(FontWeight::MEDIUM).child(r.recipient.clone()))
                        .child(
                            div()
                                .text_color(rgb(if r.action == Outcome::Failed { c.danger } else { c.muted }))
                                .child(format!("{outcome}{status}")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(r.summary()),
                )
                .when(!r.diagnostic.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(r.diagnostic.clone()),
                    )
                })
        });
        Some(
            div()
                .id("delivery-report")
                .debug_selector(|| "delivery-report".into())
                .role(Role::Region)
                .aria_label(t!("reader-delivery-report"))
                .m_3()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .child(div().font_weight(FontWeight::MEDIUM).child(t!("reader-delivery-report")))
                .children(rows)
                .into_any_element(),
        )
    }
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
            .child(div().w_full().text_center().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(t!("reader-this-message-is-encrypted")))
            .child(div().w_full().text_size(px(12.)).text_color(rgb(c.muted)).text_center().child(t!("reader-enter-your-pinal-password-to-read")))
            .child(div().w_full().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(t!("reader-password-or-recovery-key")))
            .child(div().id("unlock-field").debug_selector(|| "unlock-field".into()).w_full().min_w_0().child(field(Input::new(&self.unlock_input), c, unlocking).disabled(unlocking).w_full().h(px(39.)).text_size(px(13.))))
            .when_some(self.unlock_errors.get(account), |panel, error| panel.child(div().id("unlock-error").debug_selector(|| "unlock-error".into()).w_full().text_size(px(12.)).text_color(rgb(c.danger)).child(error.clone())))
            .child(self.button("unlock-mail", if unlocking { t!("reader-unlocking") } else { t!("reader-unlock") }, "lock", Action::UnlockMail, true, cx).w_full().flex_shrink_0().when(unlocking, |d| d.opacity(0.7)))
            .into_any_element()
    }
}

/// How a banner above the open message is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BannerStyle {
    /// On a soft fill, with an accent icon: about the message itself
    /// (snoozed, its sign-in code hidden).
    Filled,
    /// Outlined, with a muted icon: something asked of you that can be
    /// set aside (a read receipt, unsubscribing).
    Outlined,
}

impl MailApp {
    /// A banner above the open message: an icon, a line of text and its
    /// actions. Every banner wraps the same way: the text wraps, keeping
    /// room to be read, and past that the actions go below it, together
    /// and on the right, rather than past the reader's edge.
    pub(super) fn banner(
        &self,
        id: &'static str,
        icon: &'static str,
        style: BannerStyle,
        text: impl IntoElement,
        actions: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let icon_color = match style {
            BannerStyle::Filled => c.accent,
            BannerStyle::Outlined => c.muted,
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .mb_4()
            .px_3()
            .py_2()
            .rounded(px(10.))
            .map(|d| match style {
                BannerStyle::Filled => d.bg(rgb(c.soft)),
                BannerStyle::Outlined => d.border_1().border_color(rgb(c.border)),
            })
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_size(px(12.))
            .child(icon_el(icon, 15., icon_color))
            .child(div().flex_1().min_w(px(160.)).child(text))
            .child(
                div()
                    .debug_selector(move || format!("{id}-actions"))
                    .flex_shrink_0()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children(actions),
            )
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
                .child(t!("reader-loading-message")),
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
                Tooltip::new(if open { t!("reader-hide-details") } else { t!("reader-show-details") }).build(w, cx)
            })
            .on_click(cx.listener(|this, _, w, cx| this.handle(Action::ToggleDetails, w, cx)))
            .into_any_element()
    }

    /// A header's addresses, each with its own right-click menu: copying
    /// it, and for a sender's (`sender`), writing to it and finding its mail.
    fn address_list(
        &self,
        key: &'static str,
        list: &str,
        sender: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let parsed = crate::sync::smtp::recipients(list)
            .ok()
            .filter(|a| !a.is_empty());
        let Some(addresses) = parsed else {
            // Unreadable as addresses: shown as written.
            return div()
                .min_w_0()
                .overflow_hidden()
                .text_color(rgb(c.text))
                .child(list.to_owned())
                .into_any_element();
        };
        let last = addresses.len() - 1;
        div()
            .debug_selector(move || format!("details-{key}"))
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap_x_1()
            .text_color(rgb(c.text))
            .children(addresses.into_iter().enumerate().map(|(i, (name, email))| {
                let shown = super::name_and_address(name.as_deref(), &email);
                div()
                    .id(SharedString::from(format!("details-{key}-{i}")))
                    .debug_selector(move || format!("details-{key}-{i}"))
                    .min_w_0()
                    .overflow_hidden()
                    .child(if i < last { format!("{shown},") } else { shown })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let from = sender.then_some(email.as_str());
                            let entries = address_entries(name.as_deref(), &email, from);
                            this.open_menu(e.position, entries, cx)
                        }),
                    )
            }))
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
            .role(Role::Group)
            .aria_label(t!("reader-message-details"))
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
        let mask = self.mask_for(m);
        let masked = |v: &str| match &mask {
            Some(k) => k.apply(v).into_owned(),
            None => v.to_owned(),
        };
        let sender = match &mask {
            Some(k) if k.sent => k.sender(),
            _ => m.sender.clone(),
        };
        let from_email = masked(&m.from);
        let from = if sender != m.from && !sender.is_empty() {
            masked(&format!("{} <{}>", sender, m.from))
        } else {
            from_email.clone()
        };
        let from = div()
            .id("details-from")
            .debug_selector(|| "details-from".into())
            .min_w_0()
            .overflow_hidden()
            .text_color(rgb(c.text))
            .child(from)
            .on_mouse_down(MouseButton::Right, {
                let real = m.from.clone();
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let entries = address_entries(Some(&sender), &from_email, Some(&real));
                    this.open_menu(e.position, entries, cx)
                })
            })
            .into_any_element();
        let details = info
            .and_then(|i| i.result.as_ref())
            .and_then(|r| r.as_ref().ok());
        let header = |value: Option<&String>| value.filter(|v| !v.trim().is_empty()).cloned();
        let mut panel_rows: Vec<AnyElement> = vec![row("from:", from).into_any_element()];
        if let Some(v) = header(details.map(|d| &d.details.reply_to)) {
            let list = self.address_list("reply-to", &masked(&v), true, cx);
            panel_rows.push(row("reply-to:", list).into_any_element());
        }
        // Recipients a mailbox kept to its aliases wrote to through a
        // reverse alias show as the contacts themselves.
        let recipients = |v: &str| match &mask {
            Some(k) => k.recipients(v),
            None => v.to_owned(),
        };
        let to = self.address_list("to", &recipients(&m.to), false, cx);
        panel_rows.push(row("to:", to).into_any_element());
        if let Some(v) = header(details.map(|d| &d.details.cc)) {
            let cc = self.address_list("cc", &recipients(&v), false, cx);
            panel_rows.push(row("cc:", cc).into_any_element());
        }
        panel_rows.push(row("date:", text(date_full(m.date))).into_any_element());
        panel_rows.push(
            row("subject:", text(self.hide_codes(m, &m.subject).into_owned())).into_any_element(),
        );
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
                        .child(t!("reader-checking-the-sender"))
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
                if let Some(signature) = &d.signature {
                    let (icon, color) = if d.signature_good {
                        ("lock", 0x1e8e3e)
                    } else {
                        ("warning", c.danger)
                    };
                    panel_rows.push(
                        row(
                            "signature:",
                            div()
                                .id("openpgp-signature")
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(icon_el(icon, 13., color))
                                .child(div().text_color(rgb(c.text)).child(signature.clone()))
                                .into_any_element(),
                        )
                        .into_any_element(),
                    );
                }
                if !d.signers.is_empty() {
                    panel_rows.push(row("signers:", self.signer_list(&d.signers)).into_any_element());
                }
                // RFC 5280 §6.3, RFC 6960: what Check sender found of the
                // S/MIME signers' revocation.
                if let Some((line, good)) = &d.revocation {
                    let (icon, color) = if *good { ("lock", 0x1e8e3e) } else { ("warning", c.danger) };
                    panel_rows.push(
                        row(
                            "revocation:",
                            div()
                                .id("smime-revocation")
                                .debug_selector(|| "smime-revocation".into())
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(icon_el(icon, 13., color))
                                .child(div().text_color(rgb(c.text)).child(line.clone()))
                                .into_any_element(),
                        )
                        .into_any_element(),
                    );
                }
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
                                    .child(t!("reader-checking-the-sender"))
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
                                            t!("reader-check-sender"),
                                            "",
                                            Action::CheckSender,
                                            false,
                                            cx,
                                        )
                                        .when(!self.mobile.on, |d| {
                                            d.h(px(24.)).text_size(px(11.5))
                                        })
                                        .border_1()
                                        .border_color(rgb(c.border)),
                                    )
                                    .child(
                                        // Wraps within the panel rather than
                                        // widening it (a phone's width).
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_color(rgb(c.muted))
                                            .child(t!("reader-looks-up-spf-dkim-and-dmarc")),
                                    )
                                    .into_any_element()
                            },
                        )
                        .into_any_element(),
                    );
                } else if !checks.iter().any(|(_, v)| v.is_some()) {
                    panel_rows.push(
                        row("verified:", text(t!("reader-couldnt-check-the-sender-no-dns").into()))
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
                            t!("reader-show-raw"),
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
                        t!("reader-light-email"),
                        "sun",
                        !dark,
                        Action::ToggleEmailTheme,
                    ))
                    .child(segment(
                        "email-dark",
                        "",
                        t!("reader-dark-email"),
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
                        t!("reader-formatted"),
                        t!("reader-show-the-senders-formatting"),
                        "formatted",
                        showing_html,
                        Action::ShowHtml(true),
                    ))
                    .child(segment(
                        "show-plain",
                        t!("reader-plain-text"),
                        t!("reader-show-the-plain-text-version"),
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
                            t!("reader-remote-images-and-styles-load-from")
                        } else {
                            t!("reader-remote-images-are-blocked-so-senders")
                        })
                        .build(w, cx)
                    })
                    .child(icon_el(
                        if allowed { "image" } else { "image-off" },
                        14.,
                        if allowed { c.accent } else { c.muted },
                    ))
                    .when(!narrow, |d| d.child(t!("reader-remote-images")))
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
                SourceView::Headers => t!("reader-view-headers"),
                SourceView::Html => t!("reader-view-html"),
                SourceView::Original => t!("reader-view-raw"),
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
                                .w(px(if self.mobile.on { 86. } else { 110. }))
                                .flex_shrink_0()
                                .text_color(rgb(c.muted))
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(value))
                };
                let checked = d.checked;
                let verdict = |v: &Option<String>, domain: Option<&String>| match (v, domain) {
                    (Some(v), _) if v == crate::sync::auth::UNVERIFIABLE => {
                        t!("reader-couldnt-verify-the-signing-key-may").into()
                    }
                    (Some(v), Some(d)) => t!("reader-with-domain", to_uppercase = v.to_uppercase(), d = d),
                    (Some(v), None) => v.to_uppercase(),
                    (None, _) if !checked => t!("reader-not-checked-use-check-sender-in").into(),
                    (None, _) => t!("reader-not-recorded").into(),
                };
                let view = self.source_view;
                let shown = original.view(view);
                let lines = shown.map(|(_, l)| l.clone()).unwrap_or_default();
                let mono = c;
                let phone = self.mobile.on;
                // Switches between the message's headers, HTML and source.
                let mut tabs = div()
                    .flex()
                    .gap_1()
                    .ml_auto()
                    // A phone gives the tabs a row of their own, split evenly.
                    .when(self.mobile.on, |d| d.w_full().ml_0());
                for v in SourceView::ALL {
                    let on = v == view;
                    tabs = tabs.child(
                        div()
                            .id(SharedString::from(format!("source-tab-{}", v.label().to_lowercase())))
                            .debug_selector(move || format!("source-tab-{}", v.label().to_lowercase()))
                            .px_2()
                            .h(px(if self.mobile.on { super::mobile::TOUCH } else { 30. }))
                            .when(self.mobile.on, |d| d.flex_1().justify_center())
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
                    .id("original-body")
                    .debug_selector(|| "original-body".into())
                    .flex_1()
                    .min_h_0()
                    // Never wider than the dialog, however long a line is.
                    .min_w_0()
                    .w_full()
                    .max_w_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_shrink_0()
                            .id("original-facts")
                            .debug_selector(|| "original-facts".into())
                            .w_full()
                            .min_w_0()
                            .px(px(if self.mobile.on { super::mobile::GUTTER } else { 20. }))
                            .py_4()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .text_size(px(if self.mobile.on { 14. } else { 12. }))
                            .border_b_1()
                            .border_color(rgb(c.border))
                            .child(fact(t!("reader-message-id"), d.message_id.clone()))
                            .child(fact(t!("reader-created-at"), d.date.clone()))
                            .child(fact(t!("reader-from-2"), d.from.clone()))
                            .when(!d.sender.is_empty(), |e| e.child(fact(t!("reader-sent-by"), d.sender.clone())))
                            .child(fact(t!("reader-to"), d.to.clone()))
                            .child(fact(t!("reader-subject"), d.subject.clone()))
                            .child(fact("SPF", verdict(&d.spf, d.mailed_by.as_ref())))
                            .child(fact("DKIM", verdict(&d.dkim, d.signed_by.first())))
                            .child(fact(
                                "DMARC",
                                match &d.dmarc_policy {
                                    Some(p) if p != "none" => t!("reader-domain-asks-to-failures", verdict = verdict(&d.dmarc, None), p = p),
                                    _ => verdict(&d.dmarc, None),
                                },
                            ))
                            .when_some(d.arc.clone(), |e, arc| e.child(fact("ARC", arc)))
                            .when(d.partly_signed, |e| {
                                e.child(fact(t!("reader-note"), t!("reader-only-the-start-of-the-message").into()))
                            })
                            .child(fact(t!("reader-security"), d.security.label().into()))
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        self.button(
                                            "copy-original",
                                            t!("reader-copy-to-clipboard"),
                                            "copy",
                                            Action::CopyOriginal,
                                            false,
                                            cx,
                                        )
                                        .when(!self.mobile.on, |d| d.h(px(30.)).text_size(px(12.)))
                                        .border_1()
                                        .border_color(rgb(c.border)),
                                    )
                                    .child(
                                        self.button(
                                            "save-original",
                                            t!("reader-download-raw"),
                                            "download",
                                            Action::SaveOriginal,
                                            false,
                                            cx,
                                        )
                                        .when(!self.mobile.on, |d| d.h(px(30.)).text_size(px(12.)))
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
                                .child(t!("reader-this-message-has-no-html-part")),
                        )
                    })
                    .when(shown.is_some(), |d| d.child(
                        div()
                            .id("original-source")
                            .debug_selector(|| "original-source".into())
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            // The list measures its lines; laid over this
                            // area, it can't widen the dialog to fit them.
                            .relative()
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
                                                // Phones resolve no monospace
                                                // family; GPUI panics on a
                                                // font it can't find.
                                                .when(!phone, |d| d.font_family("Menlo"))
                                                .text_size(px(11.5))
                                                .text_color(rgb(mono.text))
                                                .child(lines[i].clone())
                                        })
                                        .collect()
                                })
                                .track_scroll(&self.original_scroll)
                                // A phone pans long lines sideways rather
                                // than cutting them off.
                                .when(self.mobile.on, |list| {
                                    list.with_horizontal_sizing_behavior(
                                        ListHorizontalSizingBehavior::Unconstrained,
                                    )
                                })
                                .absolute()
                                .inset_0()
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

impl MailApp {
    /// The replies the reply model suggests (Settings → Local AI) as chips
    /// under a message, each starting a reply with it written in. None for
    /// mail one doesn't answer (drafts, one's own sent mail) or when there
    /// is nothing to suggest.
    pub(super) fn smart_reply_chips(&self, m: &Mail, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let c = self.colors();
        if matches!(m.folder, Folder::Drafts | Folder::Sent) {
            return None;
        }
        let smart = self.smart_replies(m);
        if smart.replies.is_empty() {
            return None;
        }
        Some(
            div()
                .id("smart-replies")
                .role(Role::Group)
                .aria_label(t!("reader-suggested-replies"))
                .debug_selector(|| "smart-replies".into())
                .mt_3()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(smart.replies.iter().enumerate().map(|(i, s)| {
                    let intent = s.intent.to_owned();
                    let menu = intent.clone();
                    let selector = format!("smart-reply-{i}");
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(12.))
                        .py(px(5.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(c.soft))
                        .bg(rgb(c.soft))
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .hover(move |d| d.bg(rgb(c.surface)))
                        .child(icon_el("sparkle", 12., c.accent))
                        .child(s.text.to_owned())
                        .tooltip(|w, cx| {
                            Tooltip::new(
                                t!("reader-suggested-by-local-ai-right-click"),
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.handle(Action::SmartReply(intent.clone()), w, cx)
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, w, cx| {
                                cx.stop_propagation();
                                this.handle(Action::SmartReplyMenu(menu.clone()), w, cx)
                            }),
                        )
                })),
        )
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
            t!("reader-couldnt-verify-key-may-have-changed", name = name)
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
    /// including the message itself. The server's thread ids decide where
    /// both messages have one (Gmail's X-GM-THRID, OBJECTID's THREADID, or
    /// IMAP THREAD within a mailbox); otherwise messages join by
    /// References, or by subject when at least one is a reply (so
    /// newsletters that reuse a subject stay apart).
    pub(super) fn thread(&self, m: &Mail) -> Rc<Vec<usize>> {
        if let Some((id, revision, rows)) = &*self.thread_cache.borrow()
            && id == &m.id
            && *revision == self.list_revision
        {
            return rows.clone();
        }
        let rows = super::thread::conversation(&self.messages, m);
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
                    .child(t!("reader-conversation-messages", count = rows.len()))
                    .into_any_element(),
            );
        }
        let own: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        let card =
            |index: usize| {
                let x = &self.messages[index];
                let id = x.id.clone();
                let to = super::thread::short_names(&self.unreversed(x, &x.to), &own);
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
                                                .child(t!("reader-to-2", to = to.join(", "))),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(super::preview(&self.hide_codes(x, &x.snippet))),
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
                    .child(t!("reader-more-messages", hidden = hidden))
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
        let to = super::thread::short_names(&self.unreversed(m, &m.to), &own);
        let cc = super::thread::short_names(&self.unreversed(m, m.cc()), &own);
        match (to.is_empty(), cc.is_empty()) {
            (true, true) => t!("reader-to-undisclosed-recipients").to_string(),
            (true, false) => t!("reader-cc-only", cc = brief(cc, 2)),
            (false, true) => t!("reader-to-2", to = brief(to, 3)),
            (false, false) => t!("reader-to-cc", to = brief(to, 3), cc = brief(cc, 2)),
        }
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
    pub(super) fn plain_body(&self, m: &Mail) -> (SharedString, bool) {
        let mask = self.mask_for(m);
        let codes = self.hidden_codes(m);
        let key = format!(
            "{}:{}:{}:{}:{}:{}",
            m.id,
            self.quotes_open,
            self.clean_links,
            mask.as_ref().map_or("", |k| k.alias.as_str()),
            m.body.as_ref().map_or(0, String::len),
            codes.join("\u{1}")
        );
        if let Some((k, text, quoted)) = &*self.plain_cache.borrow()
            && k == &key
        {
            return (text.clone(), *quoted);
        }
        let body = m
            .body
            .clone()
            .unwrap_or_else(|| t!("reader-this-message-has-no-text-body").into());
        let (written, quoted) = super::thread::split_quoted(&body);
        let shown = if self.quotes_open {
            body.as_str()
        } else {
            written
        };
        let shown = match &mask {
            Some(k) => k.apply(shown),
            None => shown.into(),
        };
        let shown: std::borrow::Cow<str> = if codes.is_empty() {
            shown
        } else {
            crate::manage::local_ai::otp::mask(&shown, &codes).into_owned().into()
        };
        let shown = if self.clean_links {
            crate::manage::tracking::clean_text(&shown).into_owned().into()
        } else {
            shown
        };
        let text = SharedString::from(plain_markdown(&shown));
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
                t!("reader-hide-quoted-text")
            } else {
                t!("reader-show-quoted-text")
            })
            .on_click(cx.listener(|this, _, w, cx| this.handle(Action::ToggleQuotes, w, cx)))
            .into_any_element()
    }
}

impl MailApp {
    /// A message's labels, each removable; choosing one shows the mail
    /// that has it.
    fn label_chips(&self, m: &Mail, cx: &Context<Self>) -> Option<Stateful<Div>> {
        if m.labels().is_empty() {
            return None;
        }
        let c = self.colors();
        let gmail = self
            .account_of(m)
            .is_some_and(|a| a.provider == Provider::Gmail);
        let mut row = div()
            .id("reader-labels")
            .debug_selector(|| "reader-labels".into())
            .flex()
            .flex_wrap()
            .gap_1();
        for (i, label) in m.labels().iter().enumerate() {
            let shown = if gmail {
                crate::sync::transport::decode_name(label)
            } else {
                label.clone()
            };
            let search = Action::SearchFor(search_term("label", &shown));
            let remove = Action::Apply(vec![m.id.clone()], Change::Label(label.clone(), false));
            row = row.child(
                div()
                    .id(("reader-label", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_1()
                    .h(px(22.))
                    .rounded_full()
                    .bg(rgb(c.soft))
                    .text_size(px(11.))
                    .text_color(rgb(c.accent))
                    .cursor_pointer()
                    .child(icon_el("tag", 11., c.accent))
                    .child(shown.clone())
                    .tooltip(move |w, cx| Tooltip::new(t!("reader-show-mail-labeled", shown = shown)).build(w, cx))
                    .on_click(cx.listener(move |this, _, w, cx| this.handle(search.clone(), w, cx)))
                    .child(
                        div()
                            .id(("reader-label-remove", i))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .hover(move |s| s.bg(rgb(c.border)))
                            .child(icon_el("close", 9., c.muted))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.handle(remove.clone(), w, cx)
                            })),
                    ),
            );
        }
        Some(row)
    }
}
