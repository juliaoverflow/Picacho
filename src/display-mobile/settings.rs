//! Settings, setup and subscriptions on a phone. Each page's content is
//! the desktop's own; on a phone, Settings is a list of pages that each
//! open full screen, and on a tablet the list stays beside the page.
use super::chrome::{Tap, act};
use super::*;

impl MailApp {
    pub(super) fn mobile_settings(
        &mut self,
        insets: Insets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Opened at a page (an account's, from its menu) shows that page;
        // opened from the gear, the list.
        if !self.mobile.settings_seen {
            self.mobile.settings_seen = true;
            self.mobile.settings_page = self.page != Page::General;
        } else if self.mobile.settings_last.as_ref() != Some(&self.page) {
            self.mobile.settings_page = true;
        }
        self.mobile.settings_last = Some(self.page.clone());
        let c = self.colors();
        if Self::mobile_split(window) {
            // Going to another page, or back from one, moves only its column.
            let page = self.mobile_pane_motion(
                SplitPane::Reader,
                self.mobile_settings_page(Insets { left: 0., ..insets }, false, cx),
            );
            return div()
                .size_full()
                .flex()
                .bg(rgb(c.bg))
                .child(
                    div()
                        .w(px(SPLIT_LIST + insets.left))
                        .flex_shrink_0()
                        .h_full()
                        .border_r_1()
                        .border_color(rgb(c.border))
                        .child(self.mobile_settings_list(
                            Insets {
                                right: 0.,
                                ..insets
                            },
                            true,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(page),
                )
                .into_any_element();
        }
        if self.mobile.settings_page {
            self.mobile_settings_page(insets, true, cx)
        } else {
            self.mobile_settings_list(insets, false, cx)
        }
    }

    fn page_title(&self, page: &Page) -> String {
        match page {
            Page::General => "General".into(),
            Page::Reading => "Reading".into(),
            Page::Composing => "Composing".into(),
            Page::Forwarding => "Forwarding".into(),
            Page::LocalAi => "Local AI".into(),
            Page::About => "About Picacho".into(),
            Page::Account(id) => self
                .accounts
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.label().to_owned())
                .unwrap_or_else(|| "Mailbox".into()),
        }
    }

    fn open_page(page: Page) -> Tap {
        Rc::new(move |this, w, cx| {
            this.handle(Action::SettingsPage(page.clone()), w, cx);
            this.mobile.settings_page = true;
            this.mobile_pushed(cx);
        })
    }

    /// Settings' pages as a grouped list. `beside` marks the open page,
    /// shown next to it on a wide screen.
    pub(super) fn mobile_settings_list(
        &self,
        insets: Insets,
        beside: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let row = |id: &'static str, icon: &'static str, page: Page| {
            let active = beside && self.page == page;
            self.cell(
                id,
                icon_el(icon, 22., c.accent).into_any_element(),
                self.page_title(&page),
                None,
                !beside,
                active,
                Self::open_page(page),
                cx,
            )
            .into_any_element()
        };
        let mut mailboxes: Vec<AnyElement> = tree::sidebar_ordered(&self.tree, &self.accounts)
            .into_iter()
            .map(|(i, a)| {
                let page = Page::Account(a.id.clone());
                let active = beside && self.page == page;
                let issue = self
                    .account_issues
                    .get(&a.id)
                    .is_some_and(|issues| !issues.is_empty());
                self.cell(
                    SharedString::from(format!("mobile-settings-account-{}", a.id)),
                    self.account_avatar(a, i, 28.),
                    a.label().to_owned(),
                    issue.then(|| "Needs attention".to_owned()),
                    !beside,
                    active,
                    Self::open_page(page),
                    cx,
                )
                .into_any_element()
            })
            .collect();
        mailboxes.push(
            self.cell(
                "mobile-settings-add-account",
                icon_el("plus", 22., c.accent).into_any_element(),
                "Add Account",
                None,
                false,
                false,
                act(Action::Connect),
                cx,
            )
            .into_any_element(),
        );
        let body = div()
            .id("mobile-settings-list")
            .debug_selector(|| "mobile-settings-list".into())
            .track_scroll(&self.mobile.settings_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .pl(px(insets.left + GUTTER))
            .pr(px(insets.right + GUTTER))
            .pb(px(insets.below() + 24.))
            .flex()
            .flex_col()
            .child(self.group(vec![
                row("mobile-settings-general", "settings", Page::General),
                row("mobile-settings-reading", "mail", Page::Reading),
                row("mobile-settings-composing", "compose", Page::Composing),
                row("mobile-settings-forwarding", "alias", Page::Forwarding),
                row("mobile-settings-local-ai", "sparkle", Page::LocalAi),
            ]))
            .child(self.group_label("Mailboxes"))
            .child(self.group(mailboxes))
            .child(div().h_6())
            .child(self.group(vec![row("mobile-settings-about", "info", Page::About)]));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .child(self.nav_bar(
                "mobile-settings-bar",
                insets,
                Some(self.back_icon("mobile-settings-done", cx)),
                "Settings",
                None,
                vec![],
            ))
            .child(super::super::panes::scroll_frame(
                body,
                &self.mobile.settings_scroll,
            ))
            .into_any_element()
    }

    /// The open Settings page, full screen (`stacked`) or beside the list.
    fn mobile_settings_page(
        &self,
        insets: Insets,
        stacked: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        // The reverse aliases subpage scrolls on its own, in place of
        // Forwarding, whose scroll is kept for coming back.
        let sub = matches!(self.page, Page::Forwarding)
            .then(|| self.reverse_page(cx))
            .flatten();
        let (page, scroll, page_id) = match sub {
            Some(sub) => (sub, &self.fwd_reverse_scroll, "reverse-page"),
            None => (
                self.settings_body(cx),
                &self.settings_scroll,
                "settings-page",
            ),
        };
        // Back to the list of pages, or from reverse aliases to Forwarding.
        let leading = (stacked || (self.fwd_reverse.is_some() && sub_open(self)))
            .then(|| self.back_icon("mobile-settings-back", cx));
        div()
            .id("mobile-settings-page")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .child(self.nav_bar(
                "mobile-settings-page-bar",
                insets,
                leading,
                self.page_title(&self.page),
                None,
                vec![],
            ))
            .child(
                div()
                    .id(page_id)
                    .track_scroll(scroll)
                    .debug_selector(move || page_id.into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .restrict_scroll_to_axis()
                    .pt_4()
                    .pl(px(insets.left + GUTTER))
                    .pr(px(insets.right + GUTTER))
                    // Room to scroll a field out from under the keyboard.
                    .pb(px(insets.below() + 32.))
                    // Scrolling the page lets go of the keyboard.
                    .on_scroll(cx.listener(|this, _: &ScrollEvent, w, cx| {
                        this.mobile_dismiss_keyboard(w, cx)
                    }))
                    .text_size(px(15.))
                    .child(page)
                    .map(|page| super::super::panes::scroll_frame(page, scroll)),
            )
            .when_some(self.presets_for.as_deref(), |d, id| {
                d.child(self.presets_sheet(id, cx))
            })
            .when_some(self.model_confirm.as_deref(), |d, id| {
                d.child(self.delete_model_sheet(id, cx))
            })
            .into_any_element()
    }

    /// Setting up the first account, full screen.
    pub(super) fn mobile_setup(
        &mut self,
        insets: Insets,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let current = match self.step {
            Step::Welcome => 0,
            Step::Provider => 1,
            Step::Email | Step::Credentials => 2,
            Step::Done => 3,
        };
        let dots = div().flex().gap_2().children((0..4).map(|i| {
            div()
                .h(px(8.))
                .w(px(if i == current { 24. } else { 8. }))
                .rounded_full()
                .bg(rgb(if i <= current { c.accent } else { c.border }))
        }));
        div()
            .id("onboarding")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .pt(px(insets.top))
            .pl(px(insets.left))
            .pr(px(insets.right))
            .child(
                div()
                    .id("onboarding-body")
                    .track_scroll(&self.onboarding_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .restrict_scroll_to_axis()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(GUTTER))
                    .py_6()
                    .text_size(px(16.))
                    .child(
                        div()
                            .my_auto()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(self.setup_view(cx)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(40.))
                    .mb(px(insets.below()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(dots),
            )
            .into_any_element()
    }

    /// Subscriptions, then one sender's mail, then a message of it.
    pub(super) fn mobile_subscriptions(
        &mut self,
        insets: Insets,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        if self.selected.is_some() {
            return self.mobile_reader(insets, true, window, cx);
        }
        let (back, title, content) = match self.subs_sender.clone() {
            Some(sub) => (
                self.back_icon("mobile-subs-back", cx),
                if sub.name.is_empty() {
                    sub.address.clone()
                } else {
                    sub.name.clone()
                },
                self.subscription_mail_pane(None, Corners::default(), true, window, cx),
            ),
            None => (
                self.back_icon("mobile-subs-back", cx),
                "Subscriptions".to_owned(),
                self.subscription_list(true, Corners::default(), cx)
                    .border_r_0()
                    .into_any_element(),
            ),
        };
        div()
            .id("mobile-subscriptions")
            .debug_selector(|| "mobile-subscriptions".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .child(self.nav_bar("mobile-subs-bar", insets, Some(back), title, None, vec![]))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(px(insets.left))
                    .pr(px(insets.right))
                    .pb(px(insets.below()))
                    .flex()
                    .flex_col()
                    .text_size(px(15.))
                    .child(content),
            )
            .into_any_element()
    }
}

/// Forwarding's reverse aliases subpage is open.
fn sub_open(this: &MailApp) -> bool {
    matches!(this.page, Page::Forwarding)
}

impl MailApp {
    /// About Picacho, full screen within Settings: its mark, links to the
    /// source and website, and the licence. No keyboard shortcuts on a phone.
    pub(crate) fn mobile_about_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let link = |id: &'static str, icon: &'static str, label: &'static str, url: &'static str| {
            self.cell(
                id,
                icon_el(icon, 22., c.accent).into_any_element(),
                label,
                None,
                false,
                false,
                act(Action::OpenLink(url)),
                cx,
            )
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .pt_6()
            .child(self.about_heading())
            .child(
                div().mt_8().w_full().child(self.group(vec![
                    link(
                        "mobile-about-source",
                        "github",
                        "Source Code",
                        super::super::REPO_URL,
                    ),
                    link(
                        "mobile-about-website",
                        "globe",
                        "Website",
                        super::super::WEBSITE_URL,
                    ),
                ])),
            )
            .child(div().mt_6().px_4().child(self.about_footer()))
            .into_any_element()
    }
}
