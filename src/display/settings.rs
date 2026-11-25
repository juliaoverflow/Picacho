//! Settings view components.
use super::*;

/// A sending address being added or changed in Settings.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct IdentityEdit {
    pub(super) account: String,
    /// The address being changed; `None` while adding one.
    pub(super) id: Option<String>,
    /// It goes through an outgoing server of its own.
    pub(super) own_server: bool,
    /// That server takes TLS from the start (true) or STARTTLS (false);
    /// `None` goes by its port.
    pub(super) tls: Option<bool>,
    /// It had its own server (and password) before: an empty password
    /// keeps the saved one.
    pub(super) had_password: bool,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
}

impl MailApp {
    pub(super) fn segmented(
        &self,
        id: &str,
        options: Vec<(String, Action, bool)>,
        cx: &Context<Self>,
    ) -> Div {
        let c = self.colors();
        let phone = self.mobile.on;
        let track = div()
            .id(SharedString::from(format!("{id}-track")))
            .role(Role::RadioGroup)
            .w_full()
            .min_w_0()
            .p(px(3.))
            .flex()
            // A phone keeps the choices on one line, scrolling sideways
            // when they don't fit, rather than dropping some to a new line.
            .when(!phone, |d| d.flex_wrap())
            .when(phone, |d| d.overflow_x_scroll())
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
                            .role(Role::RadioButton)
                            .aria_label(label.clone())
                            .aria_selected(active)
                            .aria_toggled(if active { Toggled::True } else { Toggled::False })
                            .aria_position_in_set(i + 1)
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
                            .flex_1()
                            .when(!phone, |d| d.min_w(px(76.)).h(px(28.)).text_size(px(12.)))
                            // Each choice keeps its whole label; they share
                            // what room is left.
                            .when(phone, |d| {
                                d.flex_shrink_0().h(px(36.)).px_3().text_size(px(14.)).whitespace_nowrap()
                            })
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
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
            );
        div().w_full().min_w_0().child(track)
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
            .role(Role::Switch)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .focusable()
            .tab_stop(true)
            .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
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
    /// A setting turned on and off with a switch beside its description.
    pub(super) fn switch_setting(
        &self,
        title: &str,
        caption: &str,
        id: String,
        on: bool,
        action: Action,
        cx: &Context<Self>,
    ) -> Div {
        let c = self.colors();
        div()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    // Wraps the caption instead of pushing the switch out.
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
            .child({
                let selector = id.clone();
                self.switch(id, on, action, cx)
                    .aria_label(title.to_owned())
                    .aria_description(caption.to_owned())
                    .debug_selector(move || selector)
            })
    }
    /// How much of the account's server storage is used, where it has a quota.
    fn storage(&self, a: &Account) -> Stateful<Div> {
        let c = self.colors();
        let size = |kib: u64| {
            let bytes = kib as f64 * 1024.0;
            if bytes >= 1024.0 * 1024.0 * 1024.0 {
                t!("settings-gb", size = format!("{:.1}", bytes / (1024.0 * 1024.0 * 1024.0)))
            } else {
                t!("settings-mb", size = format!("{:.0}", bytes / (1024.0 * 1024.0)))
            }
        };
        let (text, share) = match self.quotas.get(&a.id) {
            None => (t!("settings-checking-with-the-server").to_owned(), None),
            Some(None) => (
                t!("settings-the-server-doesnt-limit-this-mailboxs").to_owned(),
                None,
            ),
            Some(Some(q)) if q.limit_kib == 0 => {
                (t!("settings-used", used = size(q.used_kib)), None)
            }
            Some(Some(q)) => {
                let share = (q.used_kib as f64 / q.limit_kib as f64).clamp(0., 1.);
                (
                    t!("settings-of-used", used = size(q.used_kib), limit = size(q.limit_kib), percent = format!("{:.0}", share * 100.)),
                    Some(share),
                )
            }
        };
        div()
            .id("account-storage")
            .debug_selector(|| "account-storage".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(text),
            )
            .when_some(share, |d, share| {
                d.child(
                    div()
                        .h(px(6.))
                        .max_w(px(320.))
                        .rounded_full()
                        .bg(rgb(c.border))
                        .child(
                            div()
                                .h_full()
                                .w(relative(share as f32))
                                .rounded_full()
                                .bg(rgb(if share > 0.9 { c.danger } else { c.accent })),
                        ),
                )
            })
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
            .role(Role::Tab)
            .aria_label(label.clone())
            .aria_selected(active)
            .focusable()
            .tab_stop(true)
            .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
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
        let narrow = window::content_size(window).width < px(700.);
        let mut nav = div()
            .id("settings-nav")
            .role(Role::TabList)
            .aria_label(t!("settings-settings"))
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
                t!("settings-general").into(),
                icon_el("settings", 15., c.muted).into_any_element(),
                Page::General,
                cx,
            ))
            .child(self.settings_nav(
                "settings-reading".into(),
                t!("settings-reading").into(),
                icon_el("mail", 15., c.muted).into_any_element(),
                Page::Reading,
                cx,
            ))
            .child(self.settings_nav(
                "settings-composing".into(),
                t!("settings-composing").into(),
                icon_el("compose", 15., c.muted).into_any_element(),
                Page::Composing,
                cx,
            ))
            .child(self.settings_nav(
                "settings-forwarding".into(),
                t!("settings-forwarding").into(),
                icon_el("alias", 15., c.muted).into_any_element(),
                Page::Forwarding,
                cx,
            ))
            .child(self.settings_nav(
                "settings-contacts".into(),
                t!("settings-contacts").into(),
                icon_el("contact", 15., c.muted).into_any_element(),
                Page::Contacts,
                cx,
            ))
            .child(self.settings_nav(
                "settings-calendar".into(),
                t!("settings-calendar").into(),
                icon_el("clock", 15., c.muted).into_any_element(),
                Page::Calendar,
                cx,
            ))
            .child(self.settings_nav(
                "settings-encryption".into(),
                t!("settings-encryption").into(),
                icon_el("lock", 15., c.muted).into_any_element(),
                Page::Encryption,
                cx,
            ))
            .child(self.settings_nav(
                "settings-privacy".into(),
                t!("settings-privacy").into(),
                icon_el("shield", 15., c.muted).into_any_element(),
                Page::Privacy,
                cx,
            ))
            .child(self.settings_nav(
                "settings-local-ai".into(),
                t!("settings-local-ai").into(),
                icon_el("sparkle", 15., c.muted).into_any_element(),
                Page::LocalAi,
                cx,
            ))
            .child(self.settings_nav(
                "settings-organizing".into(),
                t!("settings-rules-layout").into(),
                icon_el("filter", 15., c.muted).into_any_element(),
                Page::Organizing,
                cx,
            ))
            .when(!self.mobile.on, |nav| {
                nav.child(self.settings_nav(
                    "settings-shortcuts".into(),
                    t!("settings-keyboard-shortcuts").into(),
                    icon_el("keyboard", 15., c.muted).into_any_element(),
                    Page::Shortcuts,
                    cx,
                ))
            })
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
        // In the order the sidebar shows them, as the person arranged it.
        for (i, a) in tree::sidebar_ordered(&self.tree, &self.accounts) {
            nav = nav.child(self.settings_nav(
                format!("settings-account-{}", a.id),
                a.label().to_owned(),
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
                .child(t!("settings-add-account"))
                .on_click(cx.listener(|this, _, w, cx| this.handle(Action::Connect, w, cx))),
        );
        // The reverse aliases subpage scrolls on its own, in place of
        // Forwarding, whose scroll is kept for coming back.
        let sub = matches!(self.page, Page::Forwarding)
            .then(|| self.reverse_page(cx))
            .flatten();
        let page = if let Some(sub) = sub {
            (sub, &self.fwd_reverse_scroll, "reverse-page")
        } else {
            (self.settings_body(cx), &self.settings_scroll, "settings-page")
        };
        let (page, scroll, page_id) = page;
        div()
            .size_full()
            .min_h_0()
            .relative()
            .flex()
            .when(narrow, |d| d.flex_col())
            .child(nav)
            .child(
                div()
                    .id(page_id)
                    .track_scroll(scroll)
                    .debug_selector(move || page_id.into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_6()
                    .when(narrow, |d| d.p_4())
                    .child(page)
                    .map(|page| super::panes::scroll_frame(page, scroll)),
            )
            .when_some(self.presets_for.as_deref(), |d, id| {
                d.child(self.presets_sheet(id, cx))
            })
            .when_some(self.model_confirm.as_deref(), |d, id| {
                d.child(self.delete_model_sheet(id, cx))
            })
            .into_any_element()
    }
    pub(super) fn general_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let appearance = self.segmented(
            "appearance",
            [
                (Appearance::System, t!("settings-automatic")),
                (Appearance::Light, t!("settings-light")),
                (Appearance::Dark, t!("settings-dark")),
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
            .child(self.section(t!("settings-appearance")))
            // Offered once Picacho has more than one language.
            .when(crate::i18n::languages().len() > 1, |d| {
                let chosen = self
                    .backend
                    .store
                    .preference(crate::i18n::PREFERENCE)
                    .unwrap_or_default();
                let choices = std::iter::once((
                    t!("settings-language-automatic").to_owned(),
                    Action::SetLanguage(String::new()),
                    chosen.is_empty(),
                ))
                .chain(crate::i18n::languages().iter().map(|(code, name)| {
                    (
                        (*name).to_owned(),
                        Action::SetLanguage((*code).to_owned()),
                        chosen == *code,
                    )
                }))
                .collect();
                let outgoing = self
                    .backend
                    .store
                    .preference(crate::i18n::OUTGOING_PREFERENCE)
                    .unwrap_or_default();
                let outgoing_choices = std::iter::once((
                    t!("settings-outgoing-language-same").to_owned(),
                    Action::SetOutgoingLanguage(String::new()),
                    outgoing.is_empty(),
                ))
                .chain(crate::i18n::languages().iter().map(|(code, name)| {
                    (
                        (*name).to_owned(),
                        Action::SetOutgoingLanguage((*code).to_owned()),
                        outgoing == *code,
                    )
                }))
                .collect();
                d.child(self.setting(
                    t!("settings-language"),
                    t!("settings-language-caption"),
                    self.segmented("language", choices, cx),
                ))
                .child(self.setting(
                    t!("settings-outgoing-language"),
                    t!("settings-outgoing-language-caption"),
                    self.segmented("outgoing-language", outgoing_choices, cx),
                ))
            })
            .child(self.setting(
                t!("settings-theme"),
                t!("settings-automatic-follows-your-systems-light-and"),
                appearance,
            ))
            .child(self.setting(
                t!("settings-theme-color"),
                &t!(
                    "settings-used-for-buttons-selection-and-highlights",
                    label = self.accent.label()
                ),
                swatches,
            ))
            .child(
                self.setting(
                    t!("settings-reduce-motion"),
                    t!("settings-panes-menus-and-messages-appear-without"),
                    self.segmented(
                        "reduce-motion",
                        [
                            super::motion::Motion::System,
                            super::motion::Motion::Reduced,
                            super::motion::Motion::Full,
                        ]
                        .into_iter()
                        .map(|m| (m.label().to_owned(), Action::SetMotion(m), self.motion == m))
                        .collect(),
                        cx,
                    )
                    .max_w(px(320.)),
                ),
            )
            .child(
                self.setting(
                    t!("settings-contrast-in-emails"),
                    t!("settings-emails-that-style-themselves-for-more"),
                    self.segmented(
                        "email-contrast",
                        [
                            super::motion::Contrast::System,
                            super::motion::Contrast::More,
                            super::motion::Contrast::Standard,
                        ]
                        .into_iter()
                        .map(|c| (c.label().to_owned(), Action::SetContrast(c), self.contrast == c))
                        .collect(),
                    cx,
                )
                .max_w(px(320.)),
            ))
            .child(self.section(t!("settings-mailboxes")))
            .child(
                self.toggle_row(
                    t!("settings-all-accounts"),
                    t!("settings-with-more-than-one-account-list"),
                    self.switch(
                        "all-mail".into(),
                        self.all_mail,
                        Action::SetAllMail(!self.all_mail),
                        cx,
                    )
                    .debug_selector(|| "all-mail".into()),
            ))
            .map(|d| self.background_settings(d, cx))
            // A phone lists About in Settings' own list instead.
            .when(!self.mobile.on, |d| {
                let is_default = self.find.default_mail_app == Some(true);
                let default_desc = if is_default {
                    t!("settings-picacho-is-currently-your-default-email")
                } else if cfg!(windows) {
                    t!("settings-picacho-is-registered-in-windows-default")
                } else if cfg!(target_os = "macos") {
                    t!("settings-use-picacho-for-mailto-links-eml")
                } else {
                    t!("settings-use-picacho-as-your-default-app")
                };
                let default_label = if cfg!(windows) {
                    if is_default {
                        t!("settings-manage-in-default-apps")
                    } else {
                        t!("settings-open-default-apps")
                    }
                } else if cfg!(target_os = "macos") {
                    t!("settings-choose-default-email-reader")
                } else if is_default {
                    t!("settings-reapply-as-default-email-app")
                } else {
                    t!("settings-set-as-default-email-app")
                };
                d.child(self.section("Picacho"))
                    .child(self.setting(
                        t!("settings-default-email-app"),
                        default_desc,
                        div().flex().items_center().gap_2().child(
                            self.button(
                                "set-default-app",
                                default_label,
                                if is_default { "check" } else { "external" },
                                Action::Tools(super::tools::ToolAction::SetDefaultApp),
                                !is_default,
                                cx,
                            ),
                        ),
                    ))
                    .child(
                        div().flex().flex_wrap().gap_2().child(
                            self.button(
                                "open-about",
                                t!("settings-about-picacho"),
                                "",
                                Action::About,
                                false,
                                cx,
                            )
                            .border_1()
                            .border_color(rgb(c.border)),
                        ),
                    )
            })
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
        // Red for a sign-in problem; otherwise yellow warnings.
        let critical = self.issues_critical(&a.id);
        let sign_in = issues.iter().any(|(_, issue)| issue.access.is_some());
        let tone = if critical { c.danger } else { c.warning };
        div()
            .id("account-error-details")
            .debug_selector(|| "account-error-details".into())
            .w_full()
            .min_w_0()
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(tone))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .text_color(rgb(tone))
                    .child(icon_el("warning", 16., tone))
                    .child(if critical {
                        t!("settings-mailbox-needs-attention")
                    } else {
                        t!("settings-something-didnt-work")
                    }),
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
                    .when(!issue.critical(), |d| {
                        d.child(
                            div().flex().child(
                                self.button(
                                    format!("dismiss-issue-{}", source.label()),
                                    t!("settings-dismiss"),
                                    "",
                                    Action::DismissIssue(a.id.clone(), source.clone()),
                                    false,
                                    cx,
                                )
                                .h(px(28.)),
                            ),
                        )
                    })
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        self.button(
                            "account-retry",
                            if busy {
                                t!("settings-checking")
                            } else {
                                t!("settings-try-again")
                            },
                            "refresh",
                            Action::RetryAccount(a.id.clone()),
                            false,
                            cx,
                        )
                        .when(busy, |d| d.opacity(0.6)),
                    )
                    // Signing in again fixes sign-in problems, not warnings
                    // or push registrations.
                    .when(sign_in, |d| {
                        d.child(self.button(
                            "account-reconnect",
                            t!("settings-sign-in-again"),
                            "",
                            Action::Reconnect(a.id.clone()),
                            false,
                            cx,
                        ))
                    }),
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
                    (t!("settings-light"), EmailTheme::Light),
                    (t!("settings-dark"), EmailTheme::Dark),
                    (t!("settings-match-app"), EmailTheme::App),
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
        let clean = self
            .switch(
                "clean-links".into(),
                self.clean_links,
                Action::SetCleanLinks(!self.clean_links),
                cx,
            )
            .debug_selector(|| "clean-links".into());
        div().flex().flex_col().gap_5()
            .child(self.section(t!("settings-reading")))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(self.colors().muted))
                    .child(t!("settings-defaults-for-all-accounts-each-email")),
            )
            .child(self.setting(
                t!("settings-render-html"),
                t!("settings-show-the-emails-original-formatting-and"),
                html,
            ))
            .child(self.setting(
                t!("settings-email-colors"),
                t!("settings-show-email-bodies-light-or-dark"),
                theme,
            ))
            .child(self.setting(
                t!("settings-block-remote-content"),
                t!("settings-prevent-images-stylesheets-and-fonts-from"),
                remote,
            ))
            .child(self.setting(
                t!("settings-remove-tracking-from-links"),
                t!("settings-take-campaign-and-click-identifiers-such"),
                clean,
            ))
            // Phones warn before opening a link; desktops show its address
            // on hover instead.
            .when(self.mobile.on, |d| d.child(self.link_warning_settings(cx)))
            .into_any_element()
    }
    /// Warn before opening links, and the sites trusted not to (phones).
    fn link_warning_settings(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let warn = self
            .switch(
                "link-warnings".into(),
                self.links.warn,
                Action::SetLinkWarnings(!self.links.warn),
                cx,
            )
            .debug_selector(|| "link-warnings".into());
        let trusted = &self.links.trusted;
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.setting(
                t!("settings-warn-before-opening-links"),
                t!("settings-show-where-a-link-in-an"),
                warn,
            ))
            .when(self.links.warn, |d| {
                d.child(self.setting(
                    t!("settings-trusted-sites"),
                    if trusted.is_empty() {
                        t!("settings-links-to-sites-you-trust-open")
                    } else {
                        t!("settings-links-to-these-sites-and-the")
                    },
                    div().flex().flex_col().gap_2().children(trusted.iter().enumerate().map(
                        |(i, site)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(icon_el("shield", 16., c.muted))
                                .child(div().flex_1().min_w_0().truncate().child(site.clone()))
                                .child(self.button(
                                    format!("untrust-site-{i}"),
                                    t!("settings-remove"),
                                    "",
                                    Action::UntrustSite(site.clone()),
                                    false,
                                    cx,
                                ))
                        },
                    )),
                ))
            })
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
                        t!("settings-below-the-message").into(),
                        set(&|o| o.reply_placement = ReplyPlacement::Inline),
                        o.reply_placement == ReplyPlacement::Inline,
                    ),
                    (
                        t!("settings-in-a-compose-window").into(),
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
                    (t!("settings-floating-panel"), ComposeOpen::Floating),
                    (t!("settings-full-size"), ComposeOpen::Maximized),
                    (t!("settings-new-window"), ComposeOpen::Window),
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
                        t!("settings-formatted").into(),
                        set(&|o| o.formatted = true),
                        o.formatted,
                    ),
                    (
                        t!("settings-plain-text").into(),
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
                                t!("settings-off").to_string()
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
            .child(self.section(t!("settings-composing")))
            .child(self.setting(
                t!("settings-replies"),
                t!("settings-write-replies-right-below-the-message"),
                replies,
            ))
            .child(self.setting(
                t!("settings-new-messages-open"),
                t!("settings-a-panel-you-can-move-minimize"),
                open_in,
            ))
            .child(self.setting(
                t!("settings-message-format"),
                t!("settings-formatted-messages-can-use-bold-lists"),
                format,
            ))
            .child(row(
                t!("settings-reply-to-everyone-by-default"),
                t!("settings-reply-answers-all-recipients-reply-all"),
                toggle("default-reply-all", o.default_reply_all, &|o| {
                    o.default_reply_all = !o.default_reply_all
                }),
            ))
            .child(row(
                t!("settings-include-the-original-message"),
                t!("settings-replies-quote-the-message-they-answer"),
                toggle("quote-original", o.quote_original, &|o| {
                    o.quote_original = !o.quote_original
                }),
            ))
            .child(row(
                t!("settings-paste-with-formatting"),
                t!("settings-text-copied-with-bold-links-or"),
                toggle("paste-formatted", o.paste_formatted, &|o| {
                    o.paste_formatted = !o.paste_formatted
                }),
            ))
            .child(self.section(t!("settings-sending")))
            .child(self.setting(
                t!("settings-undo-send"),
                t!("settings-wait-a-moment-before-a-message"),
                undo,
            ))
            .child(row(
                t!("settings-attachment-reminder"),
                t!("settings-ask-before-sending-a-message-that"),
                toggle("attachment-reminder", o.attachment_reminder, &|o| {
                    o.attachment_reminder = !o.attachment_reminder
                }),
            ))
            .child(row(
                t!("settings-missing-subject-warning"),
                t!("settings-ask-before-sending-a-message-without"),
                toggle("empty-subject", o.warn_empty_subject, &|o| {
                    o.warn_empty_subject = !o.warn_empty_subject
                }),
            ))
            .child(self.section(t!("settings-signature")))
            .children(self.signature_chooser(cx))
            .child(
                div()
                    .id("signature")
                    .debug_selector(|| "signature".into())
                    .h(px(110.))
                    .child(textarea_field(Textarea::new(&self.signature_input), c, false).h_full()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if crate::manage::composing::is_html(&self.signature_input.read(cx).value()) {
                        "HTML: formatted messages show it as written, plain-text ones its text. Changing From in a message changes its signature too."
                    } else {
                        t!("settings-markdown-bold-links-https-pictures-https")
                    }),
            )
            .child(row(
                t!("settings-sign-replies-and-forwards"),
                t!("settings-without-this-only-new-messages-get"),
                toggle("signature-replies", o.signature_on_replies, &|o| {
                    o.signature_on_replies = !o.signature_on_replies
                }),
            ))
            .child(self.templates_settings(cx))
            .into_any_element()
    }
    pub(super) fn account_page(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let a = &self.accounts[index];
        let sync = a.sync;
        let set = |next: SyncSettings| Action::SetSync(a.id.clone(), next);
        let auth = if a.service == Service::Pinal {
            t!("settings-signed-in-with-pinal")
        } else if a.oauth {
            t!("settings-browser-sign-in")
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
                                    .child(a.label().to_owned()),
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
            .child(self.section(t!("settings-nickname")))
            .child(
                self.setting(
                    t!("settings-nickname"),
                    &t!("settings-shown-for-this-mailbox-in-the", name = a.name),
                    div()
                        .id("account-nickname")
                        .debug_selector(|| "account-nickname".into())
                        .max_w(px(320.))
                        .child(field(Input::new(&self.nickname_input), c, false)),
                ),
            )
            .when(!a.oauth && a.service != Service::Pinal, |page| {
                page.child(self.section(t!("settings-sign-in"))).child(
                    self.setting(
                        t!("settings-sign-in-name"),
                        &t!("settings-the-user-name-your-mail-servers", email = a.email),
                        div()
                            .id("account-login")
                            .debug_selector(|| "account-login".into())
                            .max_w(px(320.))
                            .child(field(Input::new(&self.login_input), c, false)),
                    ),
                )
            })
            .child(self.section(t!("settings-sending")))
            .child(
                self.setting(
                    t!("settings-reply-to"),
                    if crate::sync::smtp::recipients(&a.reply_to).is_err() {
                        t!("settings-this-isnt-a-valid-address-messages")
                    } else {
                        t!("settings-where-replies-to-your-messages-go")
                    },
                    div()
                        .id("account-reply-to")
                        .debug_selector(|| "account-reply-to".into())
                        .max_w(px(320.))
                        .child(field(Input::new(&self.reply_to_input), c, false)),
                ),
            )
            .child(self.switch_setting(
                t!("settings-ask-for-read-receipts"),
                t!("settings-recipients-mail-apps-may-offer-to"),
                format!("read-receipts-{}", a.id),
                a.read_receipts,
                Action::ToggleReadReceipts(a.id.clone()),
                cx,
            ))
            .child(self.switch_setting(
                t!("settings-ask-for-delivery-receipts"),
                t!("settings-the-receiving-mail-servers-report-when"),
                format!("delivery-receipts-{}", a.id),
                a.delivery_receipts,
                Action::ToggleDeliveryReceipts(a.id.clone()),
                cx,
            ))
            // RFC 3461 §4.1, §4.3: which reports, and how much of the
            // message a failure report returns.
            .when(a.delivery_receipts, |page| {
                let dsn = a.dsn;
                let toggle =
                    |title: &str, caption: &str, key: &str, on: bool, change: DsnChange| {
                        self.switch_setting(
                            title,
                            caption,
                            format!("dsn-{key}-{}", a.id),
                            on,
                            Action::SetDsn(a.id.clone(), change),
                            cx,
                        )
                    };
                page.child(toggle(
                    t!("settings-report-delivery"),
                    t!("settings-when-it-arrives"),
                    "success",
                    dsn.success,
                    DsnChange::Success,
                ))
                .child(toggle(
                    t!("settings-report-failure"),
                    t!("settings-when-it-cant-be-delivered"),
                    "failure",
                    dsn.failure,
                    DsnChange::Failure,
                ))
                .child(toggle(
                    t!("settings-report-delays"),
                    t!("settings-when-delivery-is-taking-longer-than"),
                    "delay",
                    dsn.delay,
                    DsnChange::Delay,
                ))
                .child(toggle(
                    t!("settings-return-the-full-message"),
                    t!("settings-a-failure-report-includes-the-whole"),
                    "full",
                    dsn.return_full,
                    DsnChange::ReturnFull,
                ))
            })
            .when_some(a.sso.as_ref(), |page, sso| {
                let c = self.colors();
                page.child(
                    div()
                        .id(SharedString::from(format!("sso-{}", a.id)))
                        .debug_selector(|| "account-sso".into())
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(t!("settings-single-sign-on")),
                        )
                        .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!(
                            "settings-signed-in-through-in-the-browser",
                            issuer = sso.issuer
                        ))),
                )
            })
            .when(
                matches!(a.provider, Provider::Imap | Provider::Pop3)
                    && !a.oauth
                    && !cfg!(any(target_os = "android", target_os = "ios")),
                |page| {
                    page.child(self.switch_setting(
                        t!("settings-sign-in-with-kerberos"),
                        if a.kerberos {
                            t!("settings-picacho-signs-in-to-this-accounts")
                        } else {
                            t!("settings-use-this-computers-kerberos-ticket-kinit")
                        },
                        format!("kerberos-{}", a.id),
                        a.kerberos,
                        Action::ToggleKerberos(a.id.clone()),
                        cx,
                    ))
                },
            )
            .child(self.switch_setting(
                t!("settings-require-encryption-in-transit"),
                t!("settings-messages-only-go-if-every-server"),
                format!("require-tls-{}", a.id),
                a.require_tls,
                Action::ToggleRequireTls(a.id.clone()),
                cx,
            ))
            // A JMAP server takes it as its submissions' BY parameter
            // (RFC 8621 §7); Proton has no such option.
            .when(a.provider != Provider::Proton, |page| {
                page.child(
                    self.setting(
                        t!("settings-give-up-on-delivery-after"),
                        t!("settings-where-your-outgoing-server-supports-it"),
                        self.segmented(
                            &format!("deliver-within-{}", a.id),
                            [
                                (0, t!("settings-days-as-usual")),
                                (4, t!("settings-4-hours")),
                                (24, t!("settings-1-day")),
                            ]
                            .into_iter()
                            .map(|(hours, label)| {
                                (
                                    label.to_owned(),
                                    Action::SetDeliverWithin(a.id.clone(), hours),
                                    a.deliver_within_hours == hours,
                                )
                            })
                            .collect(),
                        cx,
                    ),
                ))
            })
            // RFC 2852 §4: what a late message does, and a trace of its
            // way; only with a time limit.
            .when(
                a.provider != Provider::Proton && a.deliver_within_hours > 0,
                |page| {
                    page.child(
                        self.setting(
                            t!("settings-if-its-late"),
                            t!("settings-return-it-to-you-or-deliver"),
                            self.segmented(
                                &format!("deliver-late-{}", a.id),
                                [
                                    (false, t!("settings-return-it")),
                                    (true, t!("settings-deliver-and-tell-me")),
                                ]
                                .into_iter()
                                .map(|(notify, label)| {
                                    (
                                        label.to_owned(),
                                        Action::SetDeliverLateNotify(a.id.clone(), notify),
                                        a.deliver_late_notify == notify,
                                    )
                                })
                                .collect(),
                            cx,
                        ),
                    ))
                    .child(self.switch_setting(
                        t!("settings-trace-its-delivery"),
                        t!("settings-each-server-on-the-way-reports"),
                        format!("deliver-by-trace-{}", a.id),
                        a.deliver_by_trace,
                        Action::ToggleDeliverByTrace(a.id.clone()),
                        cx,
                    ))
                },
            )
            .when(a.provider != Provider::Proton, |page| {
                page.child(self.switch_setting(
                    t!("settings-let-the-server-send-scheduled-messages"),
                    t!("settings-a-message-sent-later-goes-to"),
                    format!("server-send-later-{}", a.id),
                    a.server_send_later,
                    Action::ToggleServerSendLater(a.id.clone()),
                    cx,
                ))
            })
            .child(self.identities_setting(a, cx))
            .children(self.server_identities_setting(a, cx))
            .when(a.provider != Provider::Pop3, |page| {
                page.child(self.section(t!("settings-storage")))
                    .child(self.storage(a))
                    .children(self.server_notes_view(a))
                    .children(self.jmap_access_note(a))
                    .children(self.offline_note(a))
            })
            .when(Backend::may_filter(a), |page| {
                page.child(self.section(t!("settings-filters-vacation")))
                    .child(self.filters_setting(a, cx))
            })
            .child(self.section(t!("settings-profile-picture")))
            .child(
                self.setting(
                    t!("settings-picture"),
                    if a.provider == Provider::Gmail && a.oauth {
                        t!("settings-shown-beside-this-mailbox-your-google")
                    } else {
                        t!("settings-shown-beside-this-mailbox-load-one")
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
                                .tooltip(|w, cx| {
                                    Tooltip::new(t!("settings-change-picture")).build(w, cx)
                                })
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
                                t!("settings-change-picture-2"),
                                "chevron-down",
                                Action::AvatarMenu(a.id.clone()),
                                false,
                                cx,
                            )
                            .flex_row_reverse()
                            .border_1()
                            .border_color(rgb(c.border)),
                        ),
                ),
            )
            .when(self.encrypted.contains(&a.id), |page| {
                let locked = self.locked.contains(&a.id);
                page.child(self.section(t!("settings-encryption")))
                    .child(self.setting(
                        t!("settings-zero-access-encryption"),
                        if locked {
                            t!("settings-your-mail-is-stored-encrypted-open")
                        } else {
                            t!("settings-your-mail-is-stored-encrypted-and")
                        },
                        if locked {
                            div().into_any_element()
                        } else {
                            self.button(
                                format!("lock-{}", a.id),
                                t!("settings-forget-key"),
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
            .child(self.section(t!("settings-sync")));
        if a.provider != Provider::Pop3 {
            page = page.child(
                self.setting(
                    t!("settings-keep-mail-from"),
                    t!("settings-older-mail-stays-on-the-server"),
                    self.segmented(
                        &format!("days-{}", a.id),
                        [
                            (14, t!("settings-2-weeks")),
                            (30, t!("settings-1-month")),
                            (90, t!("settings-3-months")),
                            (365, t!("settings-1-year")),
                            (0, t!("settings-all")),
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
                t!("settings-download-for-offline-reading"),
                t!("settings-message-contents-in-this-range-download"),
                self.segmented(
                    &format!("offline-{}", a.id),
                    [
                        (30, t!("settings-1-month")),
                        (365, t!("settings-1-year")),
                        (0, t!("settings-all")),
                        (OFFLINE_WHEN_OPENED, t!("settings-when-opened")),
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
        if a.provider == Provider::Pop3 {
            page = page
                .child(self.setting(
                    t!("settings-remove-from-the-server"),
                    t!("settings-downloaded-mail-stays-on-this-computer"),
                    self.segmented(
                        &format!("pop-delete-{}", a.id),
                        [
                            (None, t!("settings-never")),
                            (Some(0), t!("settings-after-download")),
                            (Some(7), t!("settings-after-1-week")),
                            (Some(30), t!("settings-after-1-month")),
                        ]
                        .into_iter()
                        .map(|(days, label)| {
                            (
                                label.to_owned(),
                                set(SyncSettings {
                                    pop_delete_after_days: days,
                                    ..sync
                                }),
                                sync.pop_delete_after_days == days,
                            )
                        })
                        .collect(),
                        cx,
                    ),
                ))
                .child(self.switch_setting(
                    t!("settings-delete-on-the-server-too"),
                    t!("settings-deleting-a-message-for-good-here"),
                    format!("pop-delete-local-{}", a.id),
                    sync.pop_delete_with_local,
                    Action::SetSync(
                        a.id.clone(),
                        SyncSettings {
                            pop_delete_with_local: !sync.pop_delete_with_local,
                            ..sync
                        },
                    ),
                    cx,
                ));
        }
        page = page
            .child(self.setting(
                t!("settings-check-for-new-mail"),
                "",
                self.segmented(
                    &format!("refresh-{}", a.id),
                    [
                        (1, t!("settings-1-min")),
                        (5, t!("settings-5-min")),
                        (15, t!("settings-15-min")),
                        (30, t!("settings-30-min")),
                        (60, t!("settings-1-hour")),
                        (0, t!("settings-manually")),
                    ]
                    .into_iter()
                    .map(|(minutes, label)| {
                        (
                            label.to_owned(),
                            set(SyncSettings {
                                refresh_minutes: minutes,
                                ..sync
                            }),
                            sync.refresh_minutes == minutes,
                        )
                    })
                    .collect(),
                    cx,
                ),
            ));
        if a.provider == Provider::Proton {
            page = page.child(
                self.setting(
                    t!("settings-check-proton-for-changes"),
                    if sync.proton_events_seconds == 0 {
                        t!("settings-new-mail-moves-and-notices-from")
                    } else {
                        t!("settings-how-often-picacho-asks-proton-about")
                    },
                    self.segmented(
                        &format!("proton-events-{}", a.id),
                        [
                            (15, t!("settings-15-sec")),
                            (30, t!("settings-30-sec")),
                            (60, t!("settings-1-min")),
                            (300, t!("settings-5-min")),
                            (900, t!("settings-15-min")),
                            (0, t!("settings-off")),
                        ]
                        .into_iter()
                        .map(|(seconds, label)| {
                            (
                                label.to_owned(),
                                set(SyncSettings { proton_events_seconds: seconds, ..sync }),
                                sync.proton_events_seconds == seconds,
                            )
                        })
                        .collect(),
                    cx,
                ),
            ));
        }
        page = page
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
                                    .child(t!("settings-notifications")),
                            )
                            .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(
                                if a.pushes_new_mail() {
                                    t!("settings-announce-new-inbox-mail-protons-push")
                                } else if sync.refresh_minutes == 0 {
                                    t!("settings-announce-new-inbox-mail-only-works")
                                } else {
                                    t!("settings-announce-new-inbox-mail-while-picacho")
                                },
                            )),
                    )
                    .child(self.switch(
                        format!("notify-{}", a.id),
                        sync.notify,
                        // Push needs Notifications: turning them off turns it off too.
                        set(SyncSettings {
                            notify: !sync.notify,
                            proton_push: sync.proton_push && !sync.notify,
                            ..sync
                        }),
                        cx,
                    )),
            )
            .when(a.provider == Provider::Proton, |page| {
                page.child(self.proton_push_setting(a, cx))
            })
            .when(a.provider.imap(), |page| {
                page.child(self.switch_setting(
                    t!("settings-only-subscribed-folders"),
                    t!("settings-show-only-the-folders-subscribed-to"),
                    format!("subscribed-only-{}", a.id),
                    sync.subscribed_only,
                    Action::SetSync(
                        a.id.clone(),
                        SyncSettings {
                            subscribed_only: !sync.subscribed_only,
                            ..sync
                        },
                    ),
                    cx,
                ))
            })
            .when(a.provider == Provider::Jmap && a.credentials_of.is_empty(), |page| {
                page.child(self.jmap_shared_setting(a, cx))
            })
            .child(self.section(t!("settings-account")))
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
                            .child(t!("settings-removing-this-account-deletes-its-mail")),
                    )
                    .child(
                        self.button(
                            format!("remove-{}", a.id),
                            t!("settings-remove-account"),
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
    /// The JMAP accounts shared with this account's sign-in (a team's
    /// mailbox, another person's), each added as an account of its own.
    fn jmap_shared_setting(&self, a: &Account, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let looking = self.shared_looking.contains(&a.id);
        let find = self
            .button(
                format!("find-shared-{}", a.id),
                if looking {
                    t!("settings-looking")
                } else {
                    t!("settings-look-for-shared-accounts")
                },
                "",
                Action::FindShared(a.id.clone()),
                false,
                cx,
            )
            .border_1()
            .border_color(rgb(c.border))
            .when(looking, |d| d.opacity(0.6));
        let mut list = div().flex().flex_col().gap_2();
        match self.jmap_shared.get(&a.id) {
            Some(Ok(found)) if found.is_empty() => {
                list = list.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(t!("settings-nothing-else-is-shared-with-this")),
                );
            }
            Some(Ok(found)) => {
                for (jmap_id, name) in found {
                    let adding = self.shared_looking.contains(&format!("{}:{jmap_id}", a.id));
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                            .child(
                                self.button(
                                    format!("add-shared-{}-{jmap_id}", a.id),
                                    if adding {
                                        t!("settings-adding")
                                    } else {
                                        t!("settings-add")
                                    },
                                    "",
                                    Action::AddShared(a.id.clone(), jmap_id.clone(), name.clone()),
                                    false,
                                    cx,
                                )
                                .border_1()
                                .border_color(rgb(c.border))
                                .when(adding, |d| d.opacity(0.6)),
                            ),
                    );
                }
            }
            Some(Err(e)) => {
                list = list.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .child(e.clone()),
                );
            }
            None => {}
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.setting(
                t!("settings-shared-with-you"),
                t!("settings-mail-accounts-others-share-with-this"),
                find,
            ))
            .child(list)
    }
    /// Proton's push notifications for a Proton Direct account: offered
    /// while its Notifications are on, and announcing new mail in place of
    /// the regular checks once turned on.
    fn proton_push_setting(&self, a: &Account, cx: &Context<Self>) -> Div {
        let title = t!("settings-push-notifications");
        let sync = a.sync;
        if !crate::sync::proton_push::available() {
            return self.setting(title, t!("settings-this-copy-of-picacho-was-built"), div());
        }
        if !sync.notify {
            return self.setting(
                title,
                t!("settings-turn-on-notifications-to-receive-protons"),
                div(),
            );
        }
        self.switch_setting(
            title,
            t!("settings-proton-sends-new-mail-and-its"),
            format!("proton-push-{}", a.id),
            sync.proton_push,
            Action::SetSync(a.id.clone(), SyncSettings { proton_push: !sync.proton_push, ..sync }),
            cx,
        )
    }
    /// An account's other sending addresses, and the form for adding or
    /// changing one.
    fn identities_setting(&self, a: &Account, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let editing = self.identity_edit.as_ref().filter(|e| e.account == a.id);
        let adding = editing.is_some_and(|e| e.id.is_none());
        let mut list = div().flex().flex_col().gap_2();
        for (i, identity) in a.identities.iter().enumerate() {
            if editing.is_some_and(|e| e.id.as_deref() == Some(identity.id.as_str())) {
                list = list.child(self.identity_form(cx));
                continue;
            }
            let through = match &identity.smtp {
                Some(s) => t!("settings-through", host = s.host),
                None => t!("settings-through-s-server", email = a.email),
            };
            let caption = if identity.name.is_empty() {
                through
            } else {
                format!("{} · {through}", identity.name)
            };
            list = list.child(
                div()
                    .id(SharedString::from(format!("identity-{}-{i}", a.id)))
                    .debug_selector(move || format!("identity-{i}"))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(px(9.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(identity.email.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(caption),
                            ),
                    )
                    .child(
                        self.button(
                            format!("edit-identity-{i}"),
                            t!("settings-edit"),
                            "",
                            Action::EditIdentity(a.id.clone(), identity.id.clone()),
                            false,
                            cx,
                        )
                        .h(px(30.)),
                    )
                    .child(
                        self.button(
                            format!("remove-identity-{i}"),
                            t!("settings-remove-address"),
                            "trash",
                            Action::RemoveIdentity(a.id.clone(), identity.id.clone()),
                            false,
                            cx,
                        )
                        .h(px(30.)),
                    ),
            );
        }
        if adding {
            list = list.child(self.identity_form(cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.setting(
                t!("settings-other-addresses"),
                &t!("settings-addresses-also-sends-as-chosen-in", email = a.email),
                if adding {
                    div().into_any_element()
                } else {
                    self.button(
                        format!("add-identity-{}", a.id),
                        t!("settings-add-address"),
                        "",
                        Action::AddIdentity(a.id.clone()),
                        false,
                        cx,
                    )
                    .border_1()
                    .border_color(rgb(c.border))
                    .into_any_element()
                },
            ))
            .child(list)
    }

    /// The fields of the sending address being added or changed.
    fn identity_form(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(edit) = &self.identity_edit else {
            return div().into_any_element();
        };
        let labeled = |label: &str, index: usize, id: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w(px(140.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(c.muted))
                        .child(label.to_owned()),
                )
                .child(div().id(id).debug_selector(move || id.into()).child(field(
                    Input::new(&self.identity_inputs[index]),
                    c,
                    edit.saving,
                )))
        };
        let row = || div().flex().flex_wrap().gap_3();
        let mut form = div()
            .id("identity-form")
            .debug_selector(|| "identity-form".into())
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .child(
                row()
                    .child(labeled(t!("settings-address"), 0, "identity-email"))
                    .child(labeled(t!("settings-name"), 1, "identity-name")),
            )
            .child(row().child(labeled(
                t!("settings-reply-to-optional"),
                2,
                "identity-reply-to",
            )))
            .child(self.switch_setting(
                t!("settings-use-its-own-outgoing-server"),
                t!("settings-send-through-another-providers-smtp-server"),
                "identity-own-server".into(),
                edit.own_server,
                Action::ToggleIdentityServer,
                cx,
            ));
        if edit.own_server {
            form = form
                .child(
                    row()
                        .child(labeled(t!("settings-outgoing-server"), 3, "identity-host"))
                        .child(
                            labeled(t!("settings-port"), 4, "identity-port")
                                .max_w(px(100.))
                                .min_w(px(80.)),
                        ),
                )
                .child(
                    row()
                        .child(labeled(t!("settings-sign-in-name"), 5, "identity-login"))
                        .child(labeled(
                            if edit.had_password {
                                t!("settings-password-empty-keeps-the-saved-one")
                            } else {
                                t!("settings-password")
                            },
                            6,
                            "identity-password",
                        )),
                )
                .child(self.switch_setting(
                    t!("settings-tls-from-the-start"),
                    t!("settings-on-for-servers-that-encrypt-from"),
                    "identity-tls".into(),
                    self.identity_tls(cx),
                    Action::ToggleIdentityTls,
                    cx,
                ));
        }
        if let Some(error) = &edit.error {
            form = form.child(
                div()
                    .id("identity-error")
                    .debug_selector(|| "identity-error".into())
                    .text_size(px(12.))
                    .text_color(rgb(c.danger))
                    .child(error.clone()),
            );
        }
        form.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1())
                .child(self.button(
                    "identity-cancel",
                    t!("settings-cancel"),
                    "",
                    Action::CancelIdentity,
                    false,
                    cx,
                ))
                .child(
                    self.button(
                        "identity-save",
                        if edit.saving {
                            t!("settings-checking")
                        } else {
                            t!("settings-save-address")
                        },
                        "",
                        Action::SaveIdentity,
                        true,
                        cx,
                    )
                    .when(edit.saving, |d| d.opacity(0.7)),
                ),
        )
        .into_any_element()
    }

    /// Running in the background once Picacho's windows close, and its tray
    /// icon (desktop); its mail service (Android); background app refresh
    /// (iOS).
    fn background_settings(&self, page: Div, cx: &Context<Self>) -> Div {
        let background = self
            .switch(
                "background".into(),
                self.background,
                Action::SetBackground(!self.background),
                cx,
            )
            .debug_selector(|| "background".into());
        if cfg!(target_os = "ios") {
            return page
                .child(self.section(t!("settings-background")))
                .child(self.toggle_row(
                    t!("settings-check-for-mail-in-the-background"),
                    t!("settings-while-picacho-isnt-open-check-for"),
                    background,
                ));
        }
        if cfg!(target_os = "android") {
            return page
                .child(self.section(t!("settings-background")))
                .child(self.toggle_row(
                    t!("settings-check-for-mail-in-the-background"),
                    t!("settings-after-picacho-is-closed-keep-checking"),
                    background,
                ));
        }
        let tray = self
            .switch(
                "tray-icon".into(),
                self.tray_icon,
                Action::SetTrayIcon(!self.tray_icon),
                cx,
            )
            .debug_selector(|| "tray-icon".into());
        page.child(self.section(t!("settings-background")))
            .child(self.toggle_row(
                t!("settings-keep-running-in-the-background"),
                t!("settings-when-you-close-picachos-window-keep"),
                background,
            ))
            .child(self.toggle_row(
                t!("settings-show-tray-icon-while-open"),
                t!("settings-also-show-picachos-tray-icon-while"),
                tray,
            ))
    }
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
                    window::detached_options(t!("settings-settings").into(), bounds),
                    move |window, cx| {
                        let view = cx.new(|cx| SettingsWindow::new(app, colors, cx));
                        { let focus = view.read(cx).focus.clone(); focus.focus(window, cx); }
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
                    t!("settings-exit-full-size")
                } else {
                    t!("settings-full-size-2")
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
                t!("settings-open-in-new-window"),
                "popout",
                Action::PlaceSettings(SettingsPlace::Window),
                false,
                cx,
            ))
    }
    /// A dialog drawn over Settings, which stays open where it was: the
    /// main dialogs' panel, header and close button, its content scrolling
    /// between them, then `footer` if it has buttons.
    pub(super) fn sheet_over_settings(
        &self,
        sheet: Sheet,
        content: impl IntoElement,
        footer: Option<Div>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let Sheet {
            id,
            title,
            width,
            close,
        } = sheet;
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x101c2c55))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .on_mouse_down(MouseButton::Left, swallow_press)
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .w(px(width))
                    .max_w_full()
                    .max_h_full()
                    .flex()
                    .flex_col()
                    .rounded(px(18.))
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
                                    .child(title),
                            )
                            .child(self.button(
                                format!("{id}-close"),
                                t!("settings-close-dialog"),
                                "close",
                                close,
                                false,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-content")))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_5()
                            .child(content),
                    )
                    .children(footer.map(|footer| {
                        footer
                            .flex_shrink_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .px_5()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(c.border))
                    })),
            )
            .with_animation(
                (id, self.menu_anim),
                Animation::new(Duration::from_millis(140)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t),
            )
            .into_any_element()
    }
    /// Preset pictures drawn over Settings, so choosing one never closes
    /// Settings or loses its place.
    pub(super) fn presets_sheet(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        self.sheet_over_settings(
            Sheet {
                id: "presets-sheet",
                title: t!("settings-choose-a-picture"),
                width: 540.,
                close: Action::ClosePresets,
            },
            self.presets_modal(id, cx),
            None,
            cx,
        )
    }
}

/// A dialog over Settings: its id, title, width and what closes it.
pub(super) struct Sheet {
    pub id: &'static str,
    pub title: &'static str,
    pub width: f32,
    pub close: Action,
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
        super::window::set_title(window, t!("settings-settings"));
        if super::applock::covered(cx) {
            return super::applock::cover(c, window);
        }
        let id = window.window_handle().window_id();
        let (content, menu, status, error) = self.app.update(cx, |app, cx| {
            app.sync_nickname_input(window, cx);
            let menu = app
                .menu
                .as_ref()
                .is_some_and(|m| m.window == Some(id))
                .then(|| app.popup_menu(window, cx));
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
                .tooltip(|w, cx| Tooltip::new(t!("settings-move-into-main-window")).build(w, cx))
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
                    .child(t!("settings-settings")),
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
            .into_any_element()
    }
}

impl MailApp {
    /// The open settings page.
    pub(super) fn settings_body(&self, cx: &Context<Self>) -> AnyElement {
        match &self.page {
            Page::Account(id) => match self.accounts.iter().position(|a| &a.id == id) {
                Some(i) => self.account_page(i, cx),
                None => self.general_page(cx),
            },
            Page::General => self.general_page(cx),
            Page::Reading => self.reading_page(cx),
            Page::Composing => self.composing_page(cx),
            Page::Forwarding => self.forwarding_page(cx),
            Page::Contacts => self.contacts_page(cx),
            Page::Calendar => self.calendar_page(cx),
            Page::Encryption => self.encryption_page(cx),
            Page::Privacy => self.privacy_page(cx),
            Page::LocalAi => self.local_ai_page(cx),
            Page::Organizing => self.organizing_page(cx),
            Page::Shortcuts => self.shortcuts_page(cx),
            Page::About if self.mobile.on => self.mobile_about_page(cx),
            Page::About => self.general_page(cx),
        }
    }
}
