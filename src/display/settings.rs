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
                format!("{:.1} GB", bytes / (1024.0 * 1024.0 * 1024.0))
            } else {
                format!("{:.0} MB", bytes / (1024.0 * 1024.0))
            }
        };
        let (text, share) = match self.quotas.get(&a.id) {
            None => ("Checking with the server…".to_owned(), None),
            Some(None) => ("The server doesn't limit this mailbox's storage, or doesn't say.".to_owned(), None),
            Some(Some(q)) if q.limit_kib == 0 => (format!("{} used", size(q.used_kib)), None),
            Some(Some(q)) => {
                let share = (q.used_kib as f64 / q.limit_kib as f64).clamp(0., 1.);
                (
                    format!(
                        "{} of {} used ({:.0}%)",
                        size(q.used_kib),
                        size(q.limit_kib),
                        share * 100.
                    ),
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
            .aria_label("Settings")
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
            .child(self.settings_nav(
                "settings-contacts".into(),
                "Contacts".into(),
                icon_el("contact", 15., c.muted).into_any_element(),
                Page::Contacts,
                cx,
            ))
            .child(self.settings_nav(
                "settings-calendar".into(),
                "Calendar".into(),
                icon_el("clock", 15., c.muted).into_any_element(),
                Page::Calendar,
                cx,
            ))
            .child(self.settings_nav(
                "settings-encryption".into(),
                "Encryption".into(),
                icon_el("lock", 15., c.muted).into_any_element(),
                Page::Encryption,
                cx,
            ))
            .child(self.settings_nav(
                "settings-local-ai".into(),
                "Local AI".into(),
                icon_el("sparkle", 15., c.muted).into_any_element(),
                Page::LocalAi,
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
                .child("Add Account")
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
            .child(self.setting(
                "Reduce motion",
                "Panes, menus and messages appear without sliding or fading, and spinners stand still. Automatic follows your system's accessibility setting.",
                self.segmented(
                    "reduce-motion",
                    [super::motion::Motion::System, super::motion::Motion::Reduced, super::motion::Motion::Full]
                        .into_iter()
                        .map(|m| (m.label().to_owned(), Action::SetMotion(m), self.motion == m))
                        .collect(),
                    cx,
                )
                .max_w(px(320.)),
            ))
            .child(self.section("Mailboxes"))
            .child(self.toggle_row(
                "All Accounts",
                "With more than one account, list every account's inbox, drafts, spam and other mail together at the top of the sidebar.",
                self.switch("all-mail".into(), self.all_mail, Action::SetAllMail(!self.all_mail), cx)
                    .debug_selector(|| "all-mail".into()),
            ))
            .map(|d| self.background_settings(d, cx))
            // A phone lists About in Settings' own list instead.
            .when(!self.mobile.on, |d| d.child(self.section("Picacho")).child(
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
            ))
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
                        "Mailbox needs attention"
                    } else {
                        "Something didn't work"
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
                                    "Dismiss",
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
                            if busy { "Checking…" } else { "Try Again" },
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
                            "Sign In Again",
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
        let clean = self
            .switch(
                "clean-links".into(),
                self.clean_links,
                Action::SetCleanLinks(!self.clean_links),
                cx,
            )
            .debug_selector(|| "clean-links".into());
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
            .child(self.setting(
                "Remove tracking from links", "Take campaign and click identifiers (such as utm_source and fbclid) out of links, and open the real address behind known redirects (Google, Outlook Safe Links, Facebook). Applies to every email.", clean,
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
                "Warn before opening links",
                "Show where a link in an email goes, and ask before opening it. Links are how fake sign-in pages steal passwords.",
                warn,
            ))
            .when(self.links.warn, |d| {
                d.child(self.setting(
                    "Trusted sites",
                    if trusted.is_empty() {
                        "Links to sites you trust open without asking. Trust a site from the warning."
                    } else {
                        "Links to these sites, and the sites under them, open without asking."
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
                                    "Remove",
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
            .child(row(
                "Paste with formatting",
                "Text copied with bold, links or lists keeps them. After pasting you can switch to the other.",
                toggle("paste-formatted", o.paste_formatted, &|o| {
                    o.paste_formatted = !o.paste_formatted
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
                    .child(textarea_field(Textarea::new(&self.signature_input), c, false).h_full()),
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
            .child(self.section("Nickname"))
            .child(self.setting(
                "Nickname",
                &format!(
                    "Shown for this mailbox in the sidebar and Settings. Leave empty to show {}.",
                    a.name
                ),
                div()
                    .id("account-nickname")
                    .debug_selector(|| "account-nickname".into())
                    .max_w(px(320.))
                    .child(field(Input::new(&self.nickname_input), c, false)),
            ))
            .when(!a.oauth && a.service != Service::Pinal, |page| {
                page.child(self.section("Sign-in")).child(self.setting(
                    "Sign-in name",
                    &format!(
                        "The user name your mail servers know you by, if it isn't {}. Leave empty to use the address.",
                        a.email
                    ),
                    div()
                        .id("account-login")
                        .debug_selector(|| "account-login".into())
                        .max_w(px(320.))
                        .child(field(Input::new(&self.login_input), c, false)),
                ))
            })
            .child(self.section("Sending"))
            .child(self.setting(
                "Reply-To",
                if crate::sync::smtp::recipients(&a.reply_to).is_err() {
                    "This isn't a valid address; messages can't be sent until it is fixed or removed."
                } else {
                    "Where replies to your messages go, if not to the address they're sent from. A message can set its own in the composer."
                },
                div()
                    .id("account-reply-to")
                    .debug_selector(|| "account-reply-to".into())
                    .max_w(px(320.))
                    .child(field(Input::new(&self.reply_to_input), c, false)),
            ))
            .child(self.switch_setting(
                "Ask for read receipts",
                "Recipients' mail apps may offer to tell you when they open your messages. Many people decline.",
                format!("read-receipts-{}", a.id),
                a.read_receipts,
                Action::ToggleReadReceipts(a.id.clone()),
                cx,
            ))
            .child(self.switch_setting(
                "Ask for delivery receipts",
                "The receiving mail servers report when your messages arrive, where your outgoing server supports it.",
                format!("delivery-receipts-{}", a.id),
                a.delivery_receipts,
                Action::ToggleDeliveryReceipts(a.id.clone()),
                cx,
            ))
            .child(self.switch_setting(
                "Require encryption in transit",
                "Messages only go if every server on the way can use encryption (REQUIRETLS); where that can't be promised they bounce instead. Your outgoing server has to support it, and Picacho tells you when a recipient's domain publishes no policy (MTA-STS or DANE) to insist on it by.",
                format!("require-tls-{}", a.id),
                a.require_tls,
                Action::ToggleRequireTls(a.id.clone()),
                cx,
            ))
            .when(a.provider != Provider::Proton, |page| {
                page.child(self.switch_setting(
                    "Let the server send scheduled messages",
                    "A message sent later goes to your outgoing server right away, which sends it at its time even with Picacho closed, where the server can hold it (FUTURERELEASE). Once handed over it can't be changed or taken back, and with SMTP its copy is in Sent from then. Where the server can't, Picacho sends it itself, as without this.",
                    format!("server-send-later-{}", a.id),
                    a.server_send_later,
                    Action::ToggleServerSendLater(a.id.clone()),
                    cx,
                ))
            })
            .child(self.identities_setting(a, cx))
            .when(a.provider != Provider::Pop3, |page| {
                page.child(self.section("Storage"))
                    .child(self.storage(a))
                    .children(self.server_notes_view(a))
                    .children(self.offline_note(a))
            })
            .when(Backend::may_filter(a), |page| {
                page.child(self.section("Filters & vacation"))
                    .child(self.filters_setting(a, cx))
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
            ));
        if a.provider == Provider::Proton {
            page = page.child(self.setting(
                "Check Proton for changes",
                if sync.proton_events_seconds == 0 {
                    "New mail, moves and notices from Proton appear at the next check for new mail."
                } else {
                    "How often Picacho asks Proton about new mail, moves and notices. Less often uses less data and battery."
                },
                self.segmented(
                    &format!("proton-events-{}", a.id),
                    [(15, "15 sec"), (30, "30 sec"), (60, "1 min"), (300, "5 min"), (900, "15 min"), (0, "Off")]
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
                                    .child("Notifications"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(if a.pushes_new_mail() {
                                        "Announce new inbox mail. Proton's push notifications announce it as it arrives, so checks for new mail don't."
                                    } else if sync.refresh_minutes == 0 {
                                        "Announce new inbox mail. Only works while Picacho checks for mail."
                                    } else {
                                        "Announce new inbox mail while Picacho is in the background."
                                    }),
                            ),
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
                    "Only subscribed folders",
                    "Show only the folders subscribed to on the server, as other mail apps do. Folders Picacho makes are subscribed. If none are, all show.",
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
    /// The JMAP accounts shared with this account's sign-in (a team's
    /// mailbox, another person's), each added as an account of its own.
    fn jmap_shared_setting(&self, a: &Account, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let looking = self.shared_looking.contains(&a.id);
        let find = self
            .button(
                format!("find-shared-{}", a.id),
                if looking { "Looking…" } else { "Look for Shared Accounts" },
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
                        .child("Nothing else is shared with this sign-in, or it's all added."),
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
                                    if adding { "Adding…" } else { "Add" },
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
                "Shared with you",
                "Mail accounts others share with this sign-in. Each is added as an account of its own, signing in as this one.",
                find,
            ))
            .child(list)
    }
    /// Proton's push notifications for a Proton Direct account: offered
    /// while its Notifications are on, and announcing new mail in place of
    /// the regular checks once turned on.
    fn proton_push_setting(&self, a: &Account, cx: &Context<Self>) -> Div {
        const TITLE: &str = "Push notifications";
        let sync = a.sync;
        if !crate::sync::proton_push::available() {
            return self.setting(
                TITLE,
                "This copy of Picacho was built without a Firebase project, so it can't receive Proton's push notifications.",
                div(),
            );
        }
        if !sync.notify {
            return self.setting(TITLE, "Turn on Notifications to receive Proton's push notifications.", div());
        }
        self.switch_setting(
            TITLE,
            "Proton sends new mail and its notices here as it would to its Android app, so they arrive at once. Picacho must be running, in the background too.",
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
                Some(s) => format!("Through {}", s.host),
                None => format!("Through {}'s server", a.email),
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
                            "Edit",
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
                            "@Remove address",
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
            .child(
                self.setting(
                    "Other addresses",
                    &format!(
                        "Addresses {} also sends as, chosen in From when writing. Each can go through this mailbox's outgoing server, if it lets you send as that address, or through a server of its own.",
                        a.email
                    ),
                    if adding {
                        div().into_any_element()
                    } else {
                        self.button(
                            format!("add-identity-{}", a.id),
                            "Add Address",
                            "",
                            Action::AddIdentity(a.id.clone()),
                            false,
                            cx,
                        )
                        .border_1()
                        .border_color(rgb(c.border))
                        .into_any_element()
                    },
                ),
            )
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
            .child(row().child(labeled("Address", 0, "identity-email")).child(labeled("Name", 1, "identity-name")))
            .child(row().child(labeled("Reply-To (optional)", 2, "identity-reply-to")))
            .child(self.switch_setting(
                "Use its own outgoing server",
                "Send through another provider's SMTP server, signing in there, instead of this mailbox's.",
                "identity-own-server".into(),
                edit.own_server,
                Action::ToggleIdentityServer,
                cx,
            ));
        if edit.own_server {
            form = form
                .child(
                    row()
                        .child(labeled("Outgoing server", 3, "identity-host"))
                        .child(labeled("Port", 4, "identity-port").max_w(px(100.)).min_w(px(80.))),
                )
                .child(
                    row()
                        .child(labeled("Sign-in name", 5, "identity-login"))
                        .child(labeled(
                            if edit.had_password { "Password (empty keeps the saved one)" } else { "Password" },
                            6,
                            "identity-password",
                        )),
                )
                .child(self.switch_setting(
                    "TLS from the start",
                    "On for servers that encrypt from the first byte (usually port 465); off for STARTTLS (usually 587). Picacho signs in once to check before saving.",
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
                    "Cancel",
                    "",
                    Action::CancelIdentity,
                    false,
                    cx,
                ))
                .child(
                    self.button(
                        "identity-save",
                        if edit.saving {
                            "Checking…"
                        } else {
                            "Save Address"
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
    /// icon (desktop); its mail service (Android). iOS has neither.
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
            return page;
        }
        if cfg!(target_os = "android") {
            return page.child(self.section("Background")).child(self.toggle_row(
                "Check for mail in the background",
                "After Picacho is closed, keep checking for new mail and notify you of it, send scheduled mail, and bring back snoozed mail. Android shows an ongoing notification while Picacho does.",
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
        page.child(self.section("Background"))
            .child(self.toggle_row(
                "Keep running in the background",
                "When you close Picacho's window, keep checking for new mail and notify you of it, send scheduled mail, and bring back snoozed mail. Open Picacho again or quit it from its icon in the tray.",
                background,
            ))
            .child(self.toggle_row(
                "Show tray icon while open",
                "Also show Picacho's tray icon while its window is open. It always shows while Picacho runs in the background.",
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
                    window::detached_options("Settings".into(), bounds),
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
            Page::LocalAi => self.local_ai_page(cx),
            Page::About if self.mobile.on => self.mobile_about_page(cx),
            Page::About => self.general_page(cx),
        }
    }
}
