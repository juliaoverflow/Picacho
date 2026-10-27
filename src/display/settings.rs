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
                avatar(&initials(&a.name), 20., i, c).into_any_element(),
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
        };
        div()
            .size_full()
            .min_h_0()
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
                    .child(page),
            )
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
                    .child(
                        self.service_tile(a.service, 44.)
                            .when(a.service == Service::Other, |_| {
                                avatar(&initials(&a.name), 44., index, c)
                            }),
                    )
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
            .when(self.account_issues.get(&a.id).is_some_and(|issues| !issues.is_empty()), |page| {
                page.child(self.account_error_details(a, cx))
            })
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
