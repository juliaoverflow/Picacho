//! Settings view components.
use super::*;

impl MailApp {
    pub(super) fn segmented(
        &self,
        id: &str,
        options: Vec<(String, Action, bool)>,
        cx: &Context<Self>,
    ) -> Div {
        let c = self.colors();
        div()
            .w_full()
            .min_w_0()
            .p(px(3.))
            .flex()
            .flex_wrap()
            .gap(px(3.))
            .rounded(px(10.))
            .bg(rgb(c.bg))
            .border_1()
            .border_color(rgb(c.border))
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(i, (label, action, active))| {
                        let selector = format!("{id}-{i}");
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex_1()
                            .min_w(px(76.))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .when(active, |d| {
                                d.bg(rgb(c.raised)).text_color(rgb(c.text)).shadow_sm()
                            })
                            .when(!active, |d| {
                                d.text_color(rgb(c.muted))
                                    .hover(move |s| s.text_color(rgb(c.text)))
                            })
                            .child(label)
                            .on_click(
                                cx.listener(move |this, _, w, cx| {
                                    this.handle(action.clone(), w, cx)
                                }),
                            )
                    }),
            )
    }
    pub(super) fn switch(
        &self,
        id: String,
        on: bool,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id))
            .w(px(38.))
            .h(px(22.))
            .flex_shrink_0()
            .rounded_full()
            .p(px(3.))
            .flex()
            .when(on, |d| d.justify_end())
            .bg(rgb(if on { c.accent } else { c.border }))
            .cursor_pointer()
            .child(
                div()
                    .size(px(16.))
                    .rounded_full()
                    .bg(rgb(0xffffff))
                    .shadow_sm(),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    pub(super) fn setting(&self, title: &str, caption: &str, control: impl IntoElement) -> Div {
        let c = self.colors();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_owned()),
                    )
                    .when(!caption.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(caption.to_owned()),
                        )
                    }),
            )
            .child(control)
    }
    pub(super) fn section(&self, title: &str) -> Div {
        let c = self.colors();
        div()
            .pt_2()
            .pb_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(c.muted))
            .child(title.to_uppercase())
    }
    pub(super) fn settings_nav(
        &self,
        id: String,
        label: String,
        lead: AnyElement,
        page: Page,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let active = self.page == page;
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .max_w(px(180.))
            .h(px(32.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(7.))
            .cursor_pointer()
            .text_size(px(13.))
            .when(active, |d| d.bg(rgb(c.soft)).text_color(rgb(c.text)))
            .when(!active, |d| {
                d.text_color(rgb(c.muted))
                    .hover(move |s| s.bg(rgb(c.surface)).text_color(rgb(c.text)))
            })
            .child(lead)
            .child(div().flex_1().min_w_0().truncate().child(label))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::SettingsPage(page.clone()), w, cx)
            }))
    }
    pub(super) fn settings_modal(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let narrow = window.viewport_size().width < px(700.);
        let mut nav = div()
            .id("settings-nav")
            .debug_selector(|| "settings-nav".into())
            .w(px(210.))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap(px(2.))
            .when(narrow, |d| {
                d.w_full()
                    .h(px(56.))
                    .min_w_0()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .overflow_y_hidden()
                    .overflow_x_scroll()
                    .border_r_0()
                    .border_b_1()
            })
            .bg(rgb(c.bg))
            .border_r_1()
            .border_color(rgb(c.border))
            .child(self.settings_nav(
                "settings-general".into(),
                "General".into(),
                icon_el("settings", 15., c.muted).into_any_element(),
                Page::General,
                cx,
            ))
            .child(self.settings_nav(
                "settings-reading".into(),
                "Reading".into(),
                icon_el("mail", 15., c.muted).into_any_element(),
                Page::Reading,
                cx,
            ))
            .child(self.settings_nav(
                "settings-composing".into(),
                "Composing".into(),
                icon_el("compose", 15., c.muted).into_any_element(),
                Page::Composing,
                cx,
            ))
            .child(self.settings_nav(
                "settings-forwarding".into(),
                "Forwarding".into(),
                icon_el("alias", 15., c.muted).into_any_element(),
                Page::Forwarding,
                cx,
            ))
            .when(!narrow, |nav| {
                nav.child(
                    div()
                        .mt_3()
                        .mb_1()
                        .px_2()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(c.muted))
                        .child("MAILBOXES"),
                )
            });
        for (i, a) in self.accounts.iter().enumerate() {
            nav = nav.child(self.settings_nav(
                format!("settings-account-{}", a.id),
                a.name.clone(),
                self.account_avatar(a, i, 20.),
                Page::Account(a.id.clone()),
                cx,
            ));
        }
        nav = nav.child(
            div()
                .id("settings-add-account")
                .flex_shrink_0()
                .h(px(32.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .hover(move |s| s.bg(rgb(c.surface)).text_color(rgb(c.text)))
                .child(icon_el("plus", 15., c.muted))
                .child("Add Account")
                .on_click(cx.listener(|this, _, w, cx| this.handle(Action::Connect, w, cx))),
        );
        let page = match &self.page {
            Page::Account(id) => match self.accounts.iter().position(|a| &a.id == id) {
                Some(i) => self.account_page(i, cx),
                None => self.general_page(cx),
            },
            Page::General => self.general_page(cx),
            Page::Reading => self.reading_page(cx),
            Page::Composing => self.composing_page(cx),
            Page::Forwarding => self.forwarding_page(cx),
        };
        div()
            .size_full()
            .min_h_0()
            .relative()
            .flex()
            .when(narrow, |d| d.flex_col())
            .child(nav)
            .child(
                div()
                    .id("settings-page")
                    .track_scroll(&self.settings_scroll)
                    .debug_selector(|| "settings-page".into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_6()
                    .when(narrow, |d| d.p_4())
                    .child(page)
                    .map(|page| super::panes::scroll_frame(page, &self.settings_scroll)),
            )
            .when_some(self.presets_for.as_deref(), |d, id| {
                d.child(self.presets_sheet(id, cx))
            })
            .into_any_element()
    }
    pub(super) fn general_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let appearance = self.segmented(
            "appearance",
            [
                (Appearance::System, "Automatic"),
                (Appearance::Light, "Light"),
                (Appearance::Dark, "Dark"),
            ]
            .into_iter()
            .map(|(a, label)| {
                (
                    label.to_owned(),
                    Action::SetAppearance(a),
                    self.appearance == a,
                )
            })
            .collect(),
            cx,
        );
        let swatches = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .children(Accent::ALL.map(|accent| {
                let (color, ..) = accent.colors(self.dark);
                let active = self.accent == accent;
                div()
                    .id(SharedString::from(format!("accent-{}", accent.key())))
                    .size(px(30.))
                    .rounded_full()
                    .p(px(3.))
                    .border_2()
                    .border_color(rgb(if active { color } else { c.surface }))
                    .cursor_pointer()
                    .child(
                        div()
                            .size_full()
                            .rounded_full()
                            .bg(rgb(color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(active, |d| {
                                d.child(icon_el("check", 12., accent.colors(self.dark).3))
                            }),
                    )
                    .tooltip(move |w, cx| Tooltip::new(accent.label()).build(w, cx))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::SetAccent(accent), w, cx)
                    }))
            }));
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Appearance"))
            .child(self.setting(
                "Theme",
                "Automatic follows your system's light and dark setting.",
                appearance,
            ))
            .child(self.setting(
                "Theme color",
                &format!(
                    "Used for buttons, selection and highlights. {}",
                    self.accent.label()
                ),
                swatches,
            ))
            .child(self.section("Mailboxes"))
            .child(self.toggle_row(
                "All Mail",
                "With more than one account, list every account's inbox, drafts, spam and other mail together at the top of the sidebar.",
                self.switch("all-mail".into(), self.all_mail, Action::SetAllMail(!self.all_mail), cx)
                    .debug_selector(|| "all-mail".into()),
            ))
            .child(self.section("Picacho"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.button(
                            "open-shortcuts",
                            "Keyboard Shortcuts",
                            "info",
                            Action::Help,
                            false,
                            cx,
                        )
                        .border_1()
                        .border_color(rgb(c.border)),
                    )
                    .child(
                        self.button("open-about", "About Picacho", "", Action::About, false, cx)
                            .border_1()
                            .border_color(rgb(c.border)),
                    ),
            )
            .into_any_element()
    }
    /// Whether the open settings page lists an account's mail errors: only
    /// that account's own page does.
    pub(super) fn settings_page_has_errors(&self) -> bool {
        let Page::Account(id) = &self.page else {
            return false;
        };
        self.accounts.iter().any(|a| &a.id == id)
            && self
                .account_issues
                .get(id)
                .is_some_and(|issues| !issues.is_empty())
    }
    fn account_error_details(&self, a: &Account, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let mut issues: Vec<_> = self
            .account_issues
            .get(&a.id)
            .into_iter()
            .flat_map(|issues| issues.iter())
            .collect();
        issues.sort_by(|(left_source, left), (right_source, right)| {
            right
                .occurred_at
                .cmp(&left.occurred_at)
                .then_with(|| left_source.label().cmp(right_source.label()))
        });
        let busy =
            self.retrying.contains(&a.id) || self.sync_requests.iter().any(|(id, _)| id == &a.id);
        div()
            .id("account-error-details")
            .debug_selector(|| "account-error-details".into())
            .w_full()
            .min_w_0()
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.danger))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .text_color(rgb(c.danger))
                    .child(icon_el("warning", 16., c.danger))
                    .child("Mailbox needs attention"),
            )
            .children(issues.into_iter().map(|(source, issue)| {
                let label = match source {
                    IssueSource::Sync(mailbox) => format!("{} · {mailbox}", source.label()),
                    _ => source.label().into(),
                };
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(label))
                    .child(
                        div()
                            .text_color(rgb(c.muted))
                            .child(date_full(issue.occurred_at)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("issue-{}", source.label())))
                            .w_full()
                            .overflow_x_hidden()
                            .child(issue.details.clone()),
                    )
                    .child(div().text_color(rgb(c.muted)).child(issue.guidance(source)))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.button(
                            "account-retry",
                            if busy { "Checking…" } else { "Try Again" },
                            "refresh",
                            Action::RetryAccount(a.id.clone()),
                            false,
                            cx,
                        )
                        .when(busy, |d| d.opacity(0.6)),
                    )
                    .child(self.button(
                        "account-reconnect",
                        "Sign In Again",
                        "",
                        Action::Reconnect(a.id.clone()),
                        false,
                        cx,
                    )),
            )
            .into_any_element()
    }
    pub(super) fn reading_page(&self, cx: &Context<Self>) -> AnyElement {
        let options = self.reading_defaults;
        let html = self
            .switch(
                "default-html".into(),
                options.html,
                Action::SetReadingDefaults(ReadingOptions {
                    html: !options.html,
                    ..options
                }),
                cx,
            )
            .debug_selector(|| "default-html".into());
        let remote = self
            .switch(
                "default-remote".into(),
                options.block_remote,
                Action::SetReadingDefaults(ReadingOptions {
                    block_remote: !options.block_remote,
                    ..options
                }),
                cx,
            )
            .debug_selector(|| "default-remote".into());
        let theme = self
            .segmented(
                "default-theme",
                [
                    ("Light", EmailTheme::Light),
                    ("Dark", EmailTheme::Dark),
                    ("Match app", EmailTheme::App),
                ]
                .into_iter()
                .map(|(label, theme)| {
                    (
                        label.to_string(),
                        Action::SetReadingDefaults(ReadingOptions { theme, ..options }),
                        options.theme == theme,
                    )
                })
                .collect(),
                cx,
            )
            .max_w(px(300.));
        div().flex().flex_col().gap_5()
            .child(self.section("Reading"))
            .child(div().text_size(px(12.)).text_color(rgb(self.colors().muted))
                .child("Defaults for all accounts. Each email has its own controls above the message. Emails with saved choices keep those choices."))
            .child(self.setting(
                "Render HTML", "Show the email's original formatting and interactive content.", html,
            ))
            .child(self.setting(
                "Email colors", "Show email bodies light or dark. Emails designed for dark mode use their own dark colors; others are recolored.", theme,
            ))
            .child(self.setting(
                "Block remote content", "Prevent images, stylesheets and fonts from contacting remote servers. Loading them may reveal that you opened an email.", remote,
            ))
            .into_any_element()
    }
    pub(super) fn composing_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let o = self.compose_options.clone();
        let set = |change: &dyn Fn(&mut ComposeOptions)| {
            let mut next = o.clone();
            change(&mut next);
            Action::SetComposeOptions(next)
        };
        let replies = self
            .segmented(
                "reply-placement",
                vec![
                    (
                        "Below the message".into(),
                        set(&|o| o.reply_placement = ReplyPlacement::Inline),
                        o.reply_placement == ReplyPlacement::Inline,
                    ),
                    (
                        "In a compose window".into(),
                        set(&|o| o.reply_placement = ReplyPlacement::Window),
                        o.reply_placement == ReplyPlacement::Window,
                    ),
                ],
                cx,
            )
            .max_w(px(380.));
        let open_in = self
            .segmented(
                "compose-open",
                [
                    ("Floating panel", ComposeOpen::Floating),
                    ("Full size", ComposeOpen::Maximized),
                    ("New window", ComposeOpen::Window),
                ]
                .into_iter()
                .map(|(label, value)| {
                    (
                        label.to_string(),
                        set(&|o| o.open_in = value),
                        o.open_in == value,
                    )
                })
                .collect(),
                cx,
            )
            .max_w(px(420.));
        let format = self
            .segmented(
                "compose-format",
                vec![
                    (
                        "Formatted".into(),
                        set(&|o| o.formatted = true),
                        o.formatted,
                    ),
                    (
                        "Plain text".into(),
                        set(&|o| o.formatted = false),
                        !o.formatted,
                    ),
                ],
                cx,
            )
            .max_w(px(300.));
        let undo = self
            .segmented(
                "undo-send",
                [0u32, 5, 10, 20, 30]
                    .into_iter()
                    .map(|secs| {
                        (
                            if secs == 0 {
                                "Off".to_string()
                            } else {
                                format!("{secs} s")
                            },
                            set(&|o| o.undo_send_seconds = secs),
                            o.undo_send_seconds == secs,
                        )
                    })
                    .collect(),
                cx,
            )
            .max_w(px(420.));
        let toggle = |id: &str, on: bool, change: &dyn Fn(&mut ComposeOptions)| {
            let id = id.to_string();
            let selector = id.clone();
            self.switch(id, on, set(change), cx)
                .debug_selector(move || selector)
        };
        let row = |title: &str, caption: &str, control: Stateful<Div>| {
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(title.to_owned()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(caption.to_owned()),
                        ),
                )
                .child(control)
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Composing"))
            .child(self.setting(
                "Replies",
                "Write replies right below the message, like a conversation, or in a compose window.",
                replies,
            ))
            .child(self.setting(
                "New messages open",
                "A panel you can move, minimize and resize, the full window, or a separate window.",
                open_in,
            ))
            .child(self.setting(
                "Message format",
                "Formatted messages can use bold, lists, links and quotes, and are sent with a plain text version.",
                format,
            ))
            .child(row(
                "Reply to everyone by default",
                "Reply answers all recipients. Reply all stays available.",
                toggle("default-reply-all", o.default_reply_all, &|o| {
                    o.default_reply_all = !o.default_reply_all
                }),
            ))
            .child(row(
                "Include the original message",
                "Replies quote the message they answer.",
                toggle("quote-original", o.quote_original, &|o| {
                    o.quote_original = !o.quote_original
                }),
            ))
            .child(self.section("Sending"))
            .child(self.setting(
                "Undo send",
                "Wait a moment before a message leaves, so you can take it back.",
                undo,
            ))
            .child(row(
                "Attachment reminder",
                "Ask before sending a message that mentions an attachment but has none.",
                toggle("attachment-reminder", o.attachment_reminder, &|o| {
                    o.attachment_reminder = !o.attachment_reminder
                }),
            ))
            .child(row(
                "Missing subject warning",
                "Ask before sending a message without a subject.",
                toggle("empty-subject", o.warn_empty_subject, &|o| {
                    o.warn_empty_subject = !o.warn_empty_subject
                }),
            ))
            .child(self.section("Signature"))
            .child(
                div()
                    .id("signature")
                    .debug_selector(|| "signature".into())
                    .h(px(110.))
                    .child(field(Input::new(&self.signature_input), c, false).h_full()),
            )
            .child(row(
                "Sign replies and forwards",
                "Without this, only new messages get the signature.",
                toggle("signature-replies", o.signature_on_replies, &|o| {
                    o.signature_on_replies = !o.signature_on_replies
                }),
            ))
            .into_any_element()
    }
    pub(super) fn account_page(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let a = &self.accounts[index];
        let sync = a.sync;
        let set = |next: SyncSettings| Action::SetSync(a.id.clone(), next);
        let auth = if a.service == Service::Pinal {
            "Signed in with Pinal"
        } else if a.oauth {
            "Browser sign-in"
        } else {
            a.service.password_label()
        };
        let mut page = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(if self.avatars.contains_key(&a.id) {
                        self.account_avatar(a, index, 44.)
                    } else {
                        self.service_tile(a.service, 44.)
                            .when(a.service == Service::Other, |_| {
                                avatar(&initials(&a.name), 44., index, c)
                            })
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(a.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(format!("{} · {} · {auth}", a.email, a.service_label())),
                            ),
                    ),
            )
            .when(self.settings_page_has_errors(), |page| {
                page.child(self.account_error_details(a, cx))
            })
            .child(self.section("Profile picture"))
            .child(self.setting(
                "Picture",
                if a.provider == Provider::Gmail && a.oauth {
                    "Shown beside this mailbox. Your Google profile picture is used until you choose another."
                } else {
                    "Shown beside this mailbox. Load one from Gravatar, pick a preset or upload your own."
                },
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(SharedString::from(format!("avatar-picker-{}", a.id)))
                            .debug_selector(|| "avatar-picker".into())
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.85))
                            .child(self.account_avatar(a, index, 48.))
                            .tooltip(|w, cx| Tooltip::new("Change picture").build(w, cx))
                            .on_click({
                                let id = a.id.clone();
                                cx.listener(move |this, _, w, cx| {
                                    this.handle(Action::AvatarMenu(id.clone()), w, cx)
                                })
                            }),
                    )
                    .child(
                        self.button(
                            format!("choose-avatar-{}", a.id),
                            "Change Picture",
                            "chevron-down",
                            Action::AvatarMenu(a.id.clone()),
                            false,
                            cx,
                        )
                        .flex_row_reverse()
                        .border_1()
                        .border_color(rgb(c.border)),
                    ),
            ))
            .when(self.encrypted.contains(&a.id), |page| {
                let locked = self.locked.contains(&a.id);
                page.child(self.section("Encryption")).child(self.setting(
                    "Zero-access encryption",
                    if locked {
                        "Your mail is stored encrypted. Open an encrypted message and enter your Pinal password to read it here."
                    } else {
                        "Your mail is stored encrypted and decrypted on this computer. The key stays in the system keychain until you remove it."
                    },
                    if locked {
                        div().into_any_element()
                    } else {
                        self.button(
                            format!("lock-{}", a.id),
                            "Forget key",
                            "lock",
                            Action::LockMail(a.id.clone()),
                            false,
                            cx,
                        )
                        .border_1()
                        .border_color(rgb(c.border))
                        .into_any_element()
                    },
                ))
            })
            .child(self.section("Sync"));
        if a.provider != Provider::Pop3 {
            page = page.child(
                self.setting(
                    "Keep mail from",
                    "Older mail stays on the server and is removed from this computer.",
                    self.segmented(
                        &format!("days-{}", a.id),
                        [
                            (14, "2 weeks"),
                            (30, "1 month"),
                            (90, "3 months"),
                            (365, "1 year"),
                            (0, "All"),
                        ]
                        .into_iter()
                        .map(|(days, label)| {
                            (
                                label.to_owned(),
                                set(SyncSettings { days, ..sync }),
                                sync.days == days,
                            )
                        })
                        .collect(),
                        cx,
                    ),
                ),
            );
        }
        if a.provider != Provider::Pop3 {
            page = page.child(self.setting(
                "Download for offline reading",
                "Message contents in this range download in the background. Other messages download when you open them.",
                self.segmented(
                    &format!("offline-{}", a.id),
                    [
                        (30, "1 month"),
                        (365, "1 year"),
                        (0, "All"),
                        (OFFLINE_WHEN_OPENED, "When opened"),
                    ]
                    .into_iter()
                    .map(|(days, label)| {
                        (
                            label.to_owned(),
                            set(SyncSettings {
                                offline_days: days,
                                ..sync
                            }),
                            sync.offline_days == days,
                        )
                    })
                    .collect(),
                    cx,
                ),
            ));
        }
        page = page
            .child(self.setting(
                "Check for new mail",
                "",
                self.segmented(
                    &format!("refresh-{}", a.id),
                    [(1, "1 min"), (5, "5 min"), (15, "15 min"), (30, "30 min"), (60, "1 hour"), (0, "Manually")]
                        .into_iter()
                        .map(|(minutes, label)| {
                            (
                                label.to_owned(),
                                set(SyncSettings { refresh_minutes: minutes, ..sync }),
                                sync.refresh_minutes == minutes,
                            )
                        })
                        .collect(),
                    cx,
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Notifications"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(if sync.refresh_minutes == 0 {
                                        "Announce new inbox mail. Only works while Picacho checks for mail."
                                    } else {
                                        "Announce new inbox mail while Picacho is in the background."
                                    }),
                            ),
                    )
                    .child(self.switch(
                        format!("notify-{}", a.id),
                        sync.notify,
                        set(SyncSettings { notify: !sync.notify, ..sync }),
                        cx,
                    )),
            )
            .child(self.section("Account"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.))
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("Removing this account deletes its mail and drafts from this computer. Mail on the server is kept."),
                    )
                    .child(
                        self.button(
                            format!("remove-{}", a.id),
                            "Remove Account",
                            "trash",
                            Action::ConfirmRemove(a.id.clone()),
                            false,
                            cx,
                        )
                        .border_1()
                        .border_color(rgb(c.border))
                        .text_color(rgb(c.danger)),
                    ),
            );
        page.into_any_element()
    }
}

impl MailApp {
    /// Moves Settings between a dialog, the full main window and a window
    /// of its own, keeping the page and everything on it.
    pub(super) fn place_settings(
        &mut self,
        place: SettingsPlace,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if place == SettingsPlace::Window {
            if self.settings_place == SettingsPlace::Window {
                self.reveal_settings_window(cx);
                return;
            }
            self.main_window = Some(w.window_handle());
            self.settings_place = SettingsPlace::Window;
            if self.modal == Some(Modal::Settings) {
                self.modal = None;
            }
            let app = cx.entity();
            let weak = cx.weak_entity();
            let colors = self.colors();
            // The new window draws Settings from this view, so it opens once
            // this update has finished.
            cx.defer(move |cx| {
                let bounds = Bounds::centered(None, size(px(900.), px(680.)), cx);
                let opened = cx.open_window(
                    window::detached_options("Settings".into(), bounds),
                    move |window, cx| {
                        let view = cx.new(|cx| SettingsWindow::new(app, colors, cx));
                        view.read(cx).focus.focus(window);
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                );
                let handle = match opened {
                    Ok(handle) => AnyWindowHandle::from(handle),
                    Err(e) => {
                        let _ = weak.update(cx, |this, cx| {
                            this.settings_place = SettingsPlace::Dialog;
                            this.modal = Some(Modal::Settings);
                            this.notice(format!("{e:#}"), true, cx);
                        });
                        return;
                    }
                };
                let app = weak.clone();
                let _ = handle.update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, move |_, cx| {
                        let _ = app.update(cx, |this, cx| {
                            this.settings_window = None;
                            this.settings_place = SettingsPlace::Dialog;
                            this.presets_for = None;
                            cx.notify();
                        });
                        true
                    });
                });
                let _ = weak.update(cx, |this, cx| {
                    this.settings_window = Some(handle);
                    cx.notify();
                });
            });
            return;
        }
        // Back from its own window: close that window, show the dialog in
        // the main window, and bring the main window forward.
        let popped = self.settings_place == SettingsPlace::Window;
        if let Some(handle) = self.settings_window.take() {
            if handle.window_id() == w.window_handle().window_id() {
                w.remove_window();
            } else {
                let _ = handle.update(cx, |_, w, _| w.remove_window());
            }
        }
        if popped {
            if let Some(main) = self.main_window {
                let _ = main.update(cx, |_, w, _| w.activate_window());
            }
            self.anim += 1;
        }
        self.settings_place = place;
        self.modal = Some(Modal::Settings);
    }
    /// Brings popped-out Settings forward. False when Settings is not in a
    /// window of its own, so the caller shows the dialog.
    pub(super) fn reveal_settings_window(&self, cx: &mut App) -> bool {
        if let Some(handle) = self.settings_window {
            let _ = handle.update(cx, |_, w, _| w.activate_window());
        }
        self.settings_place == SettingsPlace::Window
    }
    /// Expand and pop-out controls for the Settings dialog's header, like a
    /// composer's.
    pub(super) fn settings_place_controls(&self, cx: &Context<Self>) -> Div {
        let maximized = self.settings_place == SettingsPlace::Maximized;
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(self.button(
                "settings-maximize",
                if maximized {
                    "@Exit full size"
                } else {
                    "@Full size"
                },
                if maximized { "shrink" } else { "expand" },
                Action::PlaceSettings(if maximized {
                    SettingsPlace::Dialog
                } else {
                    SettingsPlace::Maximized
                }),
                false,
                cx,
            ))
            .child(self.button(
                "settings-popout",
                "@Open in new window",
                "popout",
                Action::PlaceSettings(SettingsPlace::Window),
                false,
                cx,
            ))
    }
    /// Preset pictures drawn over Settings, so choosing one never closes
    /// Settings or loses its place.
    pub(super) fn presets_sheet(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x101c2c55))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("presets-sheet")
                    .debug_selector(|| "presets-sheet".into())
                    .w(px(540.))
                    .max_w_full()
                    .max_h_full()
                    .flex()
                    .flex_col()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.surface))
                    .shadow_2xl()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .px_5()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(c.border))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Choose a Picture"),
                            )
                            .child(self.button(
                                "presets-close",
                                "@Close",
                                "close",
                                Action::ClosePresets,
                                false,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .id("presets-sheet-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_5()
                            .child(self.presets_modal(id, cx)),
                    ),
            )
            .with_animation(
                ("presets-sheet", self.menu_anim),
                Animation::new(Duration::from_millis(140)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t),
            )
            .into_any_element()
    }
}

/// Settings popped out into a window of its own. It draws the app's
/// Settings under a Picacho title bar, like a compose window.
pub(super) struct SettingsWindow {
    app: Entity<MailApp>,
    colors: Colors,
    focus: FocusHandle,
    _observe: Subscription,
}
impl SettingsWindow {
    fn new(app: Entity<MailApp>, colors: Colors, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&app, |this, app, cx| {
            this.colors = app.read(cx).colors();
            cx.notify();
        });
        Self {
            app,
            colors,
            focus: cx.focus_handle(),
            _observe,
        }
    }
    fn act(&self, action: Action, w: &mut Window, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| app.handle(action, w, cx));
    }
}
/// Closes popped-out Settings from its own controls, which (unlike the
/// system's close button) don't ask `on_window_should_close`.
fn close_settings_window(app: &WeakEntity<MailApp>, w: &mut Window, cx: &mut App) {
    let _ = app.update(cx, |this, cx| {
        this.settings_window = None;
        this.settings_place = SettingsPlace::Dialog;
        this.presets_for = None;
        this.menu = None;
        cx.notify();
    });
    w.remove_window();
}
impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors;
        window.set_window_title("Settings");
        let id = window.window_handle().window_id();
        let (content, menu, status, error) = self.app.update(cx, |app, cx| {
            let menu = app
                .menu
                .as_ref()
                .is_some_and(|m| m.window == Some(id))
                .then(|| app.popup_menu(cx));
            (
                app.settings_modal(window, cx),
                menu,
                app.status.clone(),
                app.error,
            )
        });
        let dock = window::no_drag(
            div()
                .id("settings-dock")
                .debug_selector(|| "settings-dock".into())
                .size(px(28.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.raised)))
                .active(|s| s.opacity(0.75))
                .child(icon_el("shrink", 15., c.muted))
                .tooltip(|w, cx| Tooltip::new("Move into main window").build(w, cx))
                .on_click(cx.listener(|this, _, w, cx| {
                    this.act(Action::PlaceSettings(SettingsPlace::Dialog), w, cx)
                })),
        );
        let bar = window::detached_bar("settings-titlebar", c)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(dock);
        let app = self.app.downgrade();
        let bar = window::detached_titlebar(
            bar,
            c,
            move |_, w, cx| close_settings_window(&app, w, cx),
            window,
            cx,
        );
        div()
            .key_context("Detached")
            .track_focus(&self.focus)
            .map(|d| window::window_frame(d, c, window))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &Close, w, cx| {
                // Escape closes a menu or the pictures first, then the window.
                let closed = this.app.update(cx, |app, cx| {
                    let closed = app.menu.take().is_some() || app.presets_for.take().is_some();
                    cx.notify();
                    closed
                });
                if !closed {
                    close_settings_window(&this.app.downgrade(), w, cx);
                }
            }))
            .child(bar)
            .child(div().flex_1().min_h_0().flex().child(content))
            .when(!status.is_empty(), |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .px_5()
                        .py_2()
                        .border_t_1()
                        .border_color(rgb(c.border))
                        .text_size(px(12.))
                        .text_color(rgb(if error { c.danger } else { c.accent }))
                        .child(status),
                )
            })
            .children(menu)
    }
}
