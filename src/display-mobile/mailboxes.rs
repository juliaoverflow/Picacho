//! The mailboxes screen: the bottom of the navigation stack, in place of
//! the desktop's sidebar. Every account (and view) with its folders, server
//! folders and subscriptions; a long press on any of them opens the same
//! actions as the sidebar's context menus.
use super::chrome::{Tap, act};
use super::*;

impl MailApp {
    pub(super) fn mobile_mailboxes(
        &mut self,
        insets: Insets,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let mut body = div()
            .id("mobile-mailboxes")
            .debug_selector(|| "mobile-mailboxes".into())
            .track_scroll(&self.mobile.mailboxes_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .pl(px(insets.left + GUTTER))
            .pr(px(insets.right + GUTTER))
            .pb_6()
            .flex()
            .flex_col();
        if self.mobile.reordering {
            body = body.child(self.mobile_reorder_list(cx));
        } else {
            body = self.mobile_mailbox_sections(body, cx);
        }
        let settings = self.bar_icon(
            "mobile-settings",
            "settings",
            "Settings",
            Rc::new(
                |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                    this.mobile_open_settings(w, cx)
                },
            ),
            cx,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .child(self.nav_bar(
                "mobile-mailboxes-bar",
                insets,
                None,
                "Mailboxes",
                None,
                self.mobile_reorder_button(cx).into_iter().collect(),
            ))
            .child(super::super::panes::scroll_frame(
                body,
                &self.mobile.mailboxes_scroll,
            ))
            .child(self.tool_row(
                "mobile-mailboxes-toolbar",
                insets,
                vec![settings],
                Some(self.mobile_status_line()),
                vec![self.compose_button(cx)],
            ))
            .into_any_element()
    }

    /// Every account (and view) with its folders, then Add Account.
    fn mobile_mailbox_sections(&self, mut body: Stateful<Div>, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        if self.shows_all_mail() {
            body = body.child(self.mobile_view_section(ALL_MAIL, cx));
        }
        for id in tree::account_order(&self.tree) {
            if fwd::is_view(&id) {
                body = body.child(self.mobile_view_section(&id, cx));
            } else if let Some((index, a)) =
                self.accounts.iter().enumerate().find(|(_, a)| a.id == id)
            {
                body = body.child(self.mobile_account_section(a, index, cx));
            }
        }
        // Accounts the sidebar's tree doesn't hold yet.
        for (index, a) in self.accounts.iter().enumerate() {
            if !tree::account_order(&self.tree).contains(&a.id) {
                body = body.child(self.mobile_account_section(a, index, cx));
            }
        }
        // The sample inbox (development builds) has its folders but no account.
        #[cfg(feature = "sample-data")]
        if self.accounts.is_empty() && !self.messages.is_empty() {
            let rows = Folder::ALL
                .into_iter()
                .map(|f| {
                    self.mobile_folder_cell("", f.into(), f.label().into(), folder_icon(f), 0., cx)
                })
                .collect();
            body = body.child(div().pt_4().child(self.group(rows)));
        }
        body = body.child(div().pt_6().child(self.group(vec![
                    self.cell(
                        "mobile-add-account",
                        icon_el("plus", 20., c.accent).into_any_element(),
                        "Add Account",
                        None,
                        false,
                        false,
                        act(Action::Connect),
                        cx,
                    )
                    .into_any_element(),
                ])));
        body
    }

    /// Opens Settings at its list of pages.
    pub(super) fn mobile_open_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.handle(Action::Settings, w, cx);
        self.mobile.settings_page = false;
        self.mobile.settings_seen = true;
        self.mobile.settings_last = Some(self.page.clone());
        self.mobile_pushed(cx);
    }

    /// Opens a mailbox and slides its messages in.
    fn mobile_open_mailbox(account: String, mailbox: Mailbox) -> Tap {
        Rc::new(move |this, w, cx| {
            this.mobile.mailboxes = false;
            this.handle(Action::OpenMailbox(account.clone(), mailbox.clone()), w, cx);
            this.mobile_pushed(cx);
        })
    }

    /// A folder's row: its icon, name and unread count.
    fn mobile_folder_cell(
        &self,
        account: &str,
        mailbox: Mailbox,
        label: String,
        icon: &'static str,
        indent: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        // Only beside an open list (a wide screen) is the open one marked.
        let active = self.mobile.split
            && self.account.as_deref() == Some(account)
            && self.mailbox() == mailbox
            && !self.subs_open
            && !self.summary_open;
        let count = self.unread_for(account, &mailbox);
        let selector = format!("mobile-nav-{account}-{}", mailbox.key());
        let menu_account = account.to_owned();
        let menu_mailbox = mailbox.clone();
        self.cell(
            selector,
            div()
                .pl(px(indent))
                .child(icon_el(icon, 22., c.accent))
                .into_any_element(),
            label,
            self.badge(count),
            true,
            active,
            Self::mobile_open_mailbox(account.to_owned(), mailbox),
            cx,
        )
        .relative()
        .child(self.press_menu(
            move |this| this.mailbox_entries(&menu_account, menu_mailbox.clone()),
            cx,
        ))
        .into_any_element()
    }

    /// The row opening an account's subscriptions, or every account's.
    fn mobile_subscriptions_cell(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        // Only beside an open list (a wide screen) is the open one marked.
        let active = self.mobile.split && self.subs_open && self.account.as_deref() == Some(id);
        self.cell(
            format!("mobile-subscriptions-{id}"),
            icon_el("subscriptions", 22., c.accent).into_any_element(),
            "Subscriptions",
            None,
            true,
            active,
            super::chrome::push(Action::OpenSubscriptions(id.to_owned())),
            cx,
        )
        .into_any_element()
    }

    /// The row opening an account's Summary.
    fn mobile_summary_cell(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let active = self.mobile.split && self.summary_open && self.account.as_deref() == Some(id);
        self.cell(
            format!("mobile-summary-{id}"),
            icon_el("summary", 22., c.accent).into_any_element(),
            "Summary",
            None,
            true,
            active,
            super::chrome::push(Action::OpenSummary(id.to_owned())),
            cx,
        )
        .into_any_element()
    }

    /// An account: its picture and name, then its folders.
    fn mobile_account_section(&self, a: &Account, index: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let id = a.id.clone();
        let issue = self
            .account_issues
            .get(&a.id)
            .is_some_and(|issues| !issues.is_empty());
        let mut rows = vec![];
        for folder in Folder::ALL {
            rows.push(self.mobile_folder_cell(
                &a.id,
                folder.into(),
                folder.label().into(),
                folder_icon(folder),
                0.,
                cx,
            ));
            if folder == Folder::Inbox {
                for row in self.folder_rows(&a.id) {
                    if let tree::FolderRow::Folder(f) = row {
                        rows.push(self.mobile_folder_cell(
                            &a.id,
                            Mailbox::Remote(f.name.clone()),
                            f.label.clone(),
                            "folder",
                            12. + f.depth as f32 * 12.,
                            cx,
                        ));
                    }
                }
                rows.push(self.mobile_summary_cell(&a.id, cx));
            }
        }
        if a.provider != Provider::Pop3 {
            rows.push(self.mobile_subscriptions_cell(&id, cx));
        }
        let menu_id = id.clone();
        let header = div()
            .id(SharedString::from(format!("mobile-account-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("mobile-account-{id}")
            })
            .relative()
            .pt_6()
            .pb_2()
            .flex()
            .items_center()
            .gap_3()
            .child(self.account_avatar(a, index, 32.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(a.label().to_owned()),
                    )
                    .when(a.label() != a.email, |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(rgb(c.muted))
                                .child(a.email.clone()),
                        )
                    }),
            )
            .when(issue, |d| {
                d.child(self.touch_icon(
                    SharedString::from(format!("mobile-issue-{id}")),
                    "triangle-alert",
                    "This account needs attention",
                    super::chrome::push(Action::AccountSettings(id.clone())),
                    cx,
                ))
            })
            .child(self.fold_button(&id, cx))
            .child(self.touch_icon(
                SharedString::from(format!("mobile-account-menu-{id}")),
                "ellipsis",
                "Account actions",
                Rc::new(
                    move |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                        let entries = without_folding(this.account_entries(&menu_id, false));
                        this.open_menu_here(entries, w, cx)
                    },
                ),
                cx,
            ));
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(self.open(&id), |d| d.child(self.group(rows)))
            .into_any_element()
    }

    /// All Accounts, or an alias shown as an account of its own.
    fn mobile_view_section(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some((title, subtitle, icon)) = self.view_title(id) else {
            return div().into_any_element();
        };
        let mut rows: Vec<AnyElement> = Self::view_folders(id)
            .iter()
            .map(|&folder| {
                self.mobile_folder_cell(
                    id,
                    folder.into(),
                    folder.label().into(),
                    folder_icon(folder),
                    0.,
                    cx,
                )
            })
            .collect();
        // Every account's Summary (after the Inbox) and subscriptions, as
        // the sidebar lists under All Accounts (aliases' views have none of
        // their own).
        if id == ALL_MAIL {
            let inbox = Self::view_folders(id)
                .iter()
                .position(|f| *f == Folder::Inbox)
                .map_or(0, |i| i + 1);
            rows.insert(inbox.min(rows.len()), self.mobile_summary_cell(id, cx));
            rows.push(self.mobile_subscriptions_cell(id, cx));
        }
        let menu_id = id.to_owned();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pt_6()
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(32.))
                            .rounded_full()
                            .bg(rgb(c.soft))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon_el(icon, 18., c.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(rgb(c.muted))
                                    .child(subtitle),
                            ),
                    )
                    .child(self.fold_button(id, cx))
                    .child(self.touch_icon(
                        SharedString::from(format!("mobile-view-menu-{id}")),
                        "ellipsis",
                        "Actions",
                        Rc::new(
                            move |this: &mut MailApp, w: &mut Window, cx: &mut Context<MailApp>| {
                                let entries = without_folding(this.view_entries(&menu_id));
                                this.open_menu_here(entries, w, cx)
                            },
                        ),
                        cx,
                    )),
            )
            .when(self.open(id), |d| d.child(self.group(rows)))
            .into_any_element()
    }

    /// One line on what mail is doing: checking, downloading, or how much
    /// is unread.
    pub(super) fn mobile_status_line(&self) -> AnyElement {
        let c = self.colors();
        let (busy, text) = self.mobile_status();
        div()
            .id("mobile-status")
            .debug_selector(|| "mobile-status".into())
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(13.))
            .text_color(rgb(c.muted))
            .when(busy, |d| d.child(spinner(13., c.muted)))
            .child(div().truncate().child(text))
            .into_any_element()
    }

    /// What mail is doing, in a few words, and whether it is busy.
    pub(super) fn mobile_status(&self) -> (bool, String) {
        let d = &self.download;
        if self.checking_server() || self.mailbox_loading() {
            (true, "Checking for mail…".to_owned())
        } else if d.total > 0 && d.missing > 0 {
            (
                true,
                format!(
                    "Downloading {} of {}",
                    fmt_count(d.total - d.missing),
                    fmt_count(d.total)
                ),
            )
        } else if !self.unreachable_accounts().is_empty() {
            (false, "Offline".to_owned())
        } else {
            let unread = self.mailbox_unread();
            (
                false,
                if unread > 0 {
                    format!("{} unread", fmt_count(unread))
                } else {
                    "Up to date".to_owned()
                },
            )
        }
    }
}

impl MailApp {
    /// Opens or closes an account's (or view's) folders.
    fn fold_button(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let open = self.open(id);
        self.touch_icon(
            SharedString::from(format!("mobile-fold-{id}")),
            if open {
                "chevron-down"
            } else {
                "chevron-right"
            },
            if open { "Hide folders" } else { "Show folders" },
            act(Action::ToggleAccount(id.to_owned())),
            cx,
        )
        .into_any_element()
    }
}

/// An account's menu without Collapse and Expand: its header has a button
/// for that.
pub(super) fn without_folding(entries: Vec<Entry>) -> Vec<Entry> {
    entries
        .into_iter()
        .filter(|e| {
            !matches!(
                e,
                Entry::Item {
                    action: Action::ToggleAccount(_),
                    ..
                }
            )
        })
        .collect()
}
