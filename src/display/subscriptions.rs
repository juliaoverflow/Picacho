//! Mail subscriptions: an email from one offers to unsubscribe, and each
//! mailbox (and All Subscriptions, for every mailbox) lists the senders
//! detected with how many of their messages it holds.
use super::*;
use crate::manage::subscriptions::{self as subs, Outcome};

/// A subscription row's height; the list draws only the rows in view.
const SUB_ROW: f32 = 68.;

/// Which senders the subscriptions list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SubsTab {
    #[default]
    Subscribed,
    Unsubscribed,
}
/// Done once: reads List-Unsubscribe from mail downloaded before it was kept.
const BACKFILL_PREFERENCE: &str = "subscriptions_backfilled";

impl MailApp {
    pub(super) fn load_unsubscribed(&mut self) {
        self.unsubscribed = self
            .backend
            .store
            .preference(subs::UNSUBSCRIBED_PREFERENCE)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
    }
    fn save_unsubscribed(&self) {
        if let Ok(json) = serde_json::to_string(&self.unsubscribed) {
            self
                .backend
                .store.set_preference_soon(subs::UNSUBSCRIBED_PREFERENCE, &json);
        }
    }
    /// Finds the subscriptions in mail cached before they were detected.
    pub(super) fn backfill_subscriptions(&mut self, cx: &mut Context<Self>) {
        if self.backend.store.preference(BACKFILL_PREFERENCE).is_some() {
            return;
        }
        self.task(
            cx,
            |b| {
                let found = b.store.backfill_subscriptions()?;
                b.store.set_preference(BACKFILL_PREFERENCE, "1")?;
                Ok(found)
            },
            |this, result, cx| {
                if matches!(result, Ok(n) if n > 0) && this.subs_open {
                    this.load_subscriptions(cx);
                }
            },
        );
    }
    /// Cleans the previews of mail cached before Office markup, entities
    /// and invisible padding were left out of them; once.
    pub(super) fn refresh_previews(&mut self, cx: &mut Context<Self>) {
        // Bumped whenever previews are cleaned in a new way, to clean them again.
        const DONE: &str = "previews_cleaned_3";
        if self.backend.store.preference(DONE).is_some() {
            return;
        }
        self.task(
            cx,
            |b| {
                let changed = b.store.refresh_previews()?;
                b.store.set_preference(DONE, "1")?;
                Ok(changed)
            },
            |this, result, cx| {
                if matches!(result, Ok(n) if n > 0) {
                    this.reload_cached(cx);
                }
            },
        );
    }
    /// The mailboxes a subscriptions list covers: one account, or every
    /// account for All Mail's.
    fn subscription_accounts(&self, id: &str) -> Vec<String> {
        if id == ALL_MAIL {
            self.accounts.iter().map(|a| a.id.clone()).collect()
        } else {
            self.accounts
                .iter()
                .filter(|a| a.id == id)
                .map(|a| a.id.clone())
                .collect()
        }
    }
    pub(super) fn open_subscriptions(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        self.open_mailbox(Some(id), Folder::Inbox.into(), w, cx);
        // The list shows subscriptions, not the Inbox it was opened over.
        self.load_generation += 1;
        self.subs_open = true;
        self.subs_sender = None;
        self.subs.clear();
        self.subs_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.load_subscriptions(cx);
    }
    pub(super) fn load_subscriptions(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.account.clone() else {
            // The sample mailbox, before any account is added.
            self.subs = subs::group(self.demo.clone());
            return;
        };
        let accounts = self.subscription_accounts(&id);
        self.subs_generation += 1;
        let generation = self.subs_generation;
        self.subs_loading = true;
        self.task(
            cx,
            move |b| b.store.subscriptions(&accounts),
            move |this, result, cx| {
                if generation != this.subs_generation {
                    return;
                }
                this.subs_loading = false;
                match result {
                    Ok(list) => this.subs = list,
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }
    /// Reloads what the subscriptions pane shows after mail changed: the
    /// subscriptions, and the open subscription's mail.
    pub(super) fn reload_subscriptions(&mut self, cx: &mut Context<Self>) {
        self.load_subscriptions(cx);
        if self.subs_sender.is_some() {
            self.load_sender_mail(cx);
        }
    }
    /// The subscriptions on the open tab, as indexes into `subs`.
    pub(super) fn subs_shown(&self) -> Vec<usize> {
        let unsubscribed = self.subs_tab == SubsTab::Unsubscribed;
        (0..self.subs.len())
            .filter(|&i| self.unsubscribed.contains(&self.subs[i].key()) == unsubscribed)
            .collect()
    }
    /// Lists a subscription's mail in place of the subscriptions.
    pub(super) fn open_subscription(&mut self, sub: MailSubscription, cx: &mut Context<Self>) {
        // Clicking the open one again leaves its list as it is.
        if self.subs_sender.as_ref().is_some_and(|s| s.key() == sub.key()) {
            return;
        }
        self.html_view.update(cx, |view, cx| view.clear(cx));
        self.load_generation += 1;
        self.read_generation += 1;
        self.subs_sender = Some(sub);
        self.subs_return.set(None);
        self.selected = None;
        self.selection.clear();
        self.selecting = false;
        self.messages.clear();
        self.filter(cx);
        self.scroll_list_to_top();
        self.load_sender_mail(cx);
        cx.notify();
    }
    /// Back from a subscription's mail to the subscriptions, which keep
    /// their place.
    pub(super) fn close_subscription(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.html_view.update(cx, |view, cx| view.clear(cx));
        Self::set(&self.unlock_input, "", w, cx);
        self.load_generation += 1;
        self.read_generation += 1;
        self.subs_sender = None;
        self.subs_return.set(None);
        self.selected = None;
        self.selection.clear();
        self.selecting = false;
        self.messages.clear();
        self.filter(cx);
        self.load_subscriptions(cx);
        cx.notify();
    }
    /// Loads the open subscription's mail into the list, one copy of each
    /// message, keeping the list's place and the open message.
    fn load_sender_mail(&mut self, cx: &mut Context<Self>) {
        let Some(sub) = self.subs_sender.clone() else {
            return;
        };
        if self.account.is_none() {
            // The sample mailbox.
            let mails = self
                .demo
                .iter()
                .filter(|m| MailSubscription::of(m).is_some_and(|s| s.key() == sub.key()))
                .cloned()
                .collect();
            self.set_messages(subs::one_copy_each(mails), cx);
            return;
        }
        let generation = self.load_generation;
        self.subs_sender_loading = true;
        self.task(
            cx,
            move |b| {
                b.store
                    .subscription_mail(&sub.account, &sub.address)
                    .map(subs::one_copy_each)
            },
            move |this, result, cx| {
                if generation != this.load_generation || this.subs_sender.is_none() {
                    return;
                }
                this.subs_sender_loading = false;
                match result {
                    Ok(mails) => {
                        this.set_messages(mails, cx);
                        if this.selected_ref().is_none() {
                            this.selected = None;
                        }
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }
    /// What can be done with all of a subscription's mail: its row's
    /// context menu, and the More button by Unsubscribe.
    pub(super) fn subscription_entries(&self, sub: &MailSubscription) -> Vec<Entry> {
        let unread = if self.subs_sender.as_ref().is_some_and(|s| s.key() == sub.key()) {
            self.messages.iter().any(|m| m.unread)
        } else {
            sub.unread > 0
        };
        let change = |change| Action::ChangeSubscription(sub.clone(), change);
        let moved = |folder| change(Change::Move(folder));
        vec![
            if unread {
                entry("Mark All as Read", "mail-open", change(Change::Read(true)))
            } else {
                entry("Mark All as Unread", "mail", change(Change::Read(false)))
            },
            Entry::Separator,
            entry("Move All to Inbox", "inbox", moved(Folder::Inbox)),
            entry("Archive All", "archive", moved(Folder::Archive)),
            entry("Move All to Spam", "spam", moved(Folder::Junk)),
            danger("Move All to Trash", "trash", moved(Folder::Trash)),
        ]
    }
    /// Marks or moves every message from a subscription, at once here and
    /// then on the server; the lists reload from the cache when it is done.
    pub(super) fn change_subscription(
        &mut self,
        sub: MailSubscription,
        change: Change,
        cx: &mut Context<Self>,
    ) {
        let key = sub.key();
        if !self.subs_busy.insert(key.clone()) {
            return;
        }
        let touches = move |m: &Mail| match change {
            Change::Read(read) => m.unread == read,
            Change::Move(folder) => m.folder != folder,
            Change::Star(_) => false,
        };
        // The open subscription's list, and its row, change at once.
        if self.subs_sender.as_ref().is_some_and(|s| s.key() == key) {
            for m in self.messages.iter_mut().filter(|m| touches(m)) {
                if let Change::Read(read) = change {
                    m.unread = !read;
                }
            }
            if matches!(change, Change::Move(Folder::Junk | Folder::Trash)) {
                self.messages.clear();
                self.selected = None;
                self.selection.clear();
            } else if let Change::Move(folder) = change {
                for m in &mut self.messages {
                    m.folder = folder;
                }
            }
            self.filter(cx);
        }
        if let Some(i) = self.subs.iter().position(|s| s.key() == key) {
            match change {
                Change::Read(true) => self.subs[i].unread = 0,
                Change::Read(false) => self.subs[i].unread = self.subs[i].count,
                // Spam and Trash are not offered to unsubscribe from.
                Change::Move(Folder::Junk | Folder::Trash) => {
                    self.subs.remove(i);
                }
                _ => {}
            }
        }
        let who = if sub.name.trim().is_empty() {
            sub.address.clone()
        } else {
            sub.name.clone()
        };
        let Some(a) = self.accounts.iter().find(|a| a.id == sub.account).cloned() else {
            // The sample mailbox changes only here.
            for m in self
                .demo
                .iter_mut()
                .filter(|m| MailSubscription::of(m).is_some_and(|s| s.key() == key))
            {
                match change {
                    Change::Read(read) => m.unread = !read,
                    Change::Move(folder) => m.folder = folder,
                    Change::Star(_) => {}
                }
            }
            self.subs_busy.remove(&key);
            cx.notify();
            return;
        };
        let account_id = a.id.clone();
        self.task(
            cx,
            move |b| {
                let mails = b.store.subscription_mail(&a.id, &sub.address)?;
                // Flags go to every copy; a move to the one copy shown.
                let todo: Vec<Mail> = match change {
                    Change::Move(_) => subs::one_copy_each(mails),
                    _ => mails,
                }
                .into_iter()
                .filter(|m| touches(m))
                .collect();
                let count = subs::one_copy_each(todo.clone()).len();
                if !todo.is_empty() {
                    b.change_many(&a, &todo, change)?;
                }
                Ok(count)
            },
            move |this, result, cx| {
                this.subs_busy.remove(&key);
                this.account_result(&account_id, IssueSource::Change, &result);
                this.refresh_counts(cx);
                match result {
                    Ok(0) => {}
                    Ok(n) => {
                        let messages = format!("{} message{}", fmt_count(n), if n == 1 { "" } else { "s" });
                        let done = match change {
                            Change::Read(true) => format!("Marked {messages} from {who} as read"),
                            Change::Read(false) => format!("Marked {messages} from {who} as unread"),
                            Change::Move(folder) => {
                                format!("Moved {messages} from {who} to {}", folder.label())
                            }
                            Change::Star(_) => String::new(),
                        };
                        this.notice(done, false, cx);
                    }
                    Err(e) => this.notice(format!("Couldn't change {who}'s mail: {e:#}"), true, cx),
                }
                if this.subs_open {
                    this.reload_subscriptions(cx);
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    /// Asks a subscription's sender to stop, and remembers that it was.
    /// Works out how first (checking the sender's signature), and asks the
    /// person before sending mail from their account or opening a page.
    pub(super) fn unsubscribe(&mut self, sub: MailSubscription, w: &mut Window, cx: &mut Context<Self>) {
        let key = sub.key();
        if self.unsubscribed.contains(&key) || !self.unsubscribing.insert(key.clone()) {
            return;
        }
        let Some(account) = self.accounts.iter().find(|a| a.id == sub.account).cloned() else {
            self.unsubscribing.remove(&key);
            self.notice("Add this mailbox's account to unsubscribe.", true, cx);
            return;
        };
        let who = if sub.name.is_empty() || sub.name.eq_ignore_ascii_case(&sub.address) {
            sub.address.clone()
        } else {
            sub.name.clone()
        };
        let window = w.window_handle();
        let planning = account.clone();
        self.task(
            cx,
            move |b| b.unsubscribe_plan(&planning, &sub),
            move |this, result, cx| {
                let plan = match result {
                    Ok(plan) => plan,
                    Err(e) => {
                        this.unsubscribing.remove(&key);
                        this.notice(format!("Couldn't unsubscribe: {e:#}"), true, cx);
                        return;
                    }
                };
                if !plan.needs_confirmation() {
                    this.carry_out_unsubscribe(key, who, account, plan, cx);
                    return;
                }
                let (title, mut detail, go) = match &plan.method {
                    subs::Method::Mail { to, subject } => (
                        format!("Email {to} to unsubscribe?"),
                        format!(
                            "A message saying \"unsubscribe\", with the subject \"{subject}\", will be sent from {} to {to}.",
                            account.email
                        ),
                        "Send",
                    ),
                    subs::Method::Web(url) => (
                        format!("Open {who}'s unsubscribe page?"),
                        format!("{url} will open in your browser."),
                        "Open",
                    ),
                    subs::Method::OneClick(_) => unreachable!("one click needs no confirmation"),
                };
                if !plan.verified {
                    detail.push_str(&format!(
                        "\n\nThe sender's signature doesn't cover this request, so it may not come from {}.",
                        sub_address(&key)
                    ));
                }
                let answer = window.update(cx, |_, window, cx| {
                    window.prompt(PromptLevel::Info, &title, Some(&detail), &[go, "Cancel"], cx)
                });
                let Ok(answer) = answer else {
                    this.unsubscribing.remove(&key);
                    return;
                };
                cx.spawn(async move |this, cx| {
                    let go = matches!(answer.await, Ok(0));
                    let _ = this.update(cx, |this, cx| {
                        if go {
                            this.carry_out_unsubscribe(key, who, account, plan, cx);
                        } else {
                            this.unsubscribing.remove(&key);
                            cx.notify();
                        }
                    });
                })
                .detach();
            },
        );
        cx.notify();
    }
    fn carry_out_unsubscribe(
        &mut self,
        key: String,
        who: String,
        account: crate::manage::model::Account,
        plan: subs::Plan,
        cx: &mut Context<Self>,
    ) {
        self.task(
            cx,
            move |b| b.unsubscribe(&account, &plan),
            move |this, result, cx| {
                this.unsubscribing.remove(&key);
                match result {
                    Ok(outcome) => {
                        this.unsubscribed.insert(key);
                        this.save_unsubscribed();
                        match outcome {
                            Outcome::Done => {
                                this.notice(format!("Unsubscribed from {who}"), false, cx)
                            }
                            Outcome::Open(url) => {
                                cx.open_url(&url);
                                this.notice(
                                    format!("Finish unsubscribing from {who} on the page that opened"),
                                    false,
                                    cx,
                                );
                            }
                        }
                    }
                    Err(e) => this.notice(format!("Couldn't unsubscribe: {e:#}"), true, cx),
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    /// The Unsubscribe button for a subscription, as it stands.
    fn unsubscribe_button(&self, id: String, sub: &MailSubscription, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let key = sub.key();
        if self.unsubscribed.contains(&key) {
            return div()
                .id(SharedString::from(id))
                .h(px(28.))
                .px_2()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(icon_el("check", 13., c.muted))
                .child("Unsubscribed");
        }
        let busy = self.unsubscribing.contains(&key);
        self.button(
            id,
            if busy { "Unsubscribing…" } else { "Unsubscribe" },
            "",
            Action::Unsubscribe(sub.clone()),
            false,
            cx,
        )
        .h(px(28.))
        .flex_shrink_0()
        .text_size(px(12.))
        .border_1()
        .border_color(rgb(c.border))
        .when(busy, |d| d.opacity(0.6))
    }
    /// In an email from a subscription: a line offering to unsubscribe.
    pub(super) fn unsubscribe_banner(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        let sub = MailSubscription::of(m)?;
        let c = self.colors();
        Some(
            div()
                .id("unsubscribe-banner")
                .debug_selector(|| "unsubscribe-banner".into())
                .mb_3()
                .px_3()
                .py_2()
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(c.border))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(icon_el("subscriptions", 14., c.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child("This message is from a mailing list or subscription."),
                )
                .child(self.unsubscribe_button("unsubscribe-mail".into(), &sub, cx))
                .into_any_element(),
        )
    }
    /// "Subscriptions" under an account, or "All Subscriptions" under All Mail.
    pub(super) fn subscriptions_nav(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let active = self.subs_open && self.account.as_deref() == Some(id);
        let action = Action::OpenSubscriptions(id.to_owned());
        let selector = format!("nav-{id}-subscriptions");
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(|| selector)
            .h(px(30.))
            .pl(px(if self.sidebar_collapsed { 10. } else { 30. }))
            .pr_2()
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(if active { c.soft } else { c.surface })))
            .text_color(rgb(if active { c.text } else { c.muted }))
            .child(icon_el(
                "subscriptions",
                16.,
                if active { c.accent } else { c.muted },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .child("Subscriptions"),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    /// The same in the collapsed sidebar.
    pub(super) fn subscriptions_rail(&self, id: &str, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let active = self.subs_open && self.account.as_deref() == Some(id);
        self.rail_button(
            format!("rail-{id}-subscriptions"),
            "Subscriptions".into(),
            icon_el("subscriptions", 17., if active { c.accent } else { c.muted })
                .into_any_element(),
            active,
            Action::OpenSubscriptions(id.to_owned()),
            cx,
        )
    }
    /// The subscriptions, in the message list's place.
    pub(super) fn subscription_list(
        &self,
        single: bool,
        corners: Corners<Pixels>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id("subscription-list-pane")
            .debug_selector(|| "subscription-list".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(if single { 0. } else { LIST_MIN }))
            .when(single, |d| d.w_full())
            .min_h_0()
            .h_full()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .border_r_1()
            .border_color(rgb(c.border))
            .child(self.subscriptions_header(cx))
            .child(self.subscriptions_body(cx))
    }
    /// The open subscription's mail, in the reader's place; a message of it
    /// opens over the list, which keeps its place for coming back.
    pub(super) fn subscription_mail_pane(
        &self,
        width: Option<f32>,
        corners: Corners<Pixels>,
        single: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        if let Some(width) = width {
            // The reader's resize handle drags this pane.
            self.reader_shown.set(width);
        }
        let pane = div()
            .id("subscription-mail-pane")
            .debug_selector(|| "subscription-mail-pane".into())
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .min_w_0()
            .map(|d| match width {
                Some(w) => d.w(px(w)).flex_shrink_0(),
                None => d.flex_1(),
            })
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right);
        let Some(sub) = self.subs_sender.clone() else {
            return pane
                .items_center()
                .justify_center()
                .gap_3()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(icon_el("subscriptions", 36., c.border))
                .child("Choose a subscription to see its mail")
                .into_any_element();
        };
        if self.selected_ref().is_some() {
            return pane
                .child(self.subscription_message_bar(&sub, cx))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(self.reader(corners, false, window, cx)),
                )
                .into_any_element();
        }
        // Back from a message: the list where it was.
        if let Some(offset) = self.subs_return.take() {
            self.list_scroll.0.borrow().base_handle.set_offset(offset);
        }
        pane.child(self.sender_header(&sub, single, cx))
            .child(self.sender_body(cx))
            .into_any_element()
    }
    fn subscriptions_header(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let shown = self.subs_shown();
        let count = shown.len();
        let total: usize = shown.iter().map(|&i| self.subs[i].count).sum();
        let unread: usize = shown.iter().map(|&i| self.subs[i].unread).sum();
        let unsubscribed = self
            .subs
            .iter()
            .filter(|s| self.unsubscribed.contains(&s.key()))
            .count();
        let tab = |id: &'static str, label: String, on: bool, action: Action| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(26.))
                .px_3()
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .bg(rgb(if on { c.soft } else { c.surface }))
                .text_color(rgb(if on { c.text } else { c.muted }))
                .hover(move |s| s.bg(rgb(if on { c.soft } else { c.raised })))
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
                .child(label)
        };
        div()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .gap_2()
                    .pl_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Subscriptions"),
                    )
                    .when(self.subs_loading, |d| d.child(spinner(13., c.muted))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(tab(
                        "subs-tab-subscribed",
                        format!("Subscribed · {}", fmt_count(self.subs.len() - unsubscribed)),
                        self.subs_tab == SubsTab::Subscribed,
                        Action::SubscriptionsTab(false),
                    ))
                    .child(tab(
                        "subs-tab-unsubscribed",
                        format!("Unsubscribed · {}", fmt_count(unsubscribed)),
                        self.subs_tab == SubsTab::Unsubscribed,
                        Action::SubscriptionsTab(true),
                    )),
            )
            .child(
                div()
                    .pl_1()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(if count == 0 {
                        "Senders whose mail offers to unsubscribe".to_string()
                    } else {
                        format!(
                            "{} sender{} · {} message{} · {} unread",
                            fmt_count(count),
                            if count == 1 { "" } else { "s" },
                            fmt_count(total),
                            if total == 1 { "" } else { "s" },
                            fmt_count(unread),
                        )
                    }),
            )
    }
    fn subscriptions_body(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let all = self.account.as_deref() == Some(ALL_MAIL);
        let count = self.subs_shown().len();
        if count == 0 {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(if self.subs_loading {
                    spinner(20., c.muted).into_any_element()
                } else {
                    icon_el("subscriptions", 28., c.border).into_any_element()
                })
                .child(if self.subs_loading {
                    "Loading…"
                } else if self.subs_tab == SubsTab::Unsubscribed {
                    "You haven't unsubscribed from anything yet"
                } else {
                    "No subscriptions found"
                })
                .into_any_element();
        }
        let entity = cx.entity();
        uniform_list("subscription-list", count, move |range, _, cx| {
            entity.update(cx, |this, cx| {
                let cx: &Context<Self> = cx;
                let shown = this.subs_shown();
                range
                    .filter_map(|i| {
                        let sub = this.subs.get(*shown.get(i)?)?;
                        Some(this.subscription_row(i, sub, all, cx))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(self.subs_scroll.clone())
        .flex_1()
        .min_h_0()
        .map(|list| panes::scroll_frame(list, &self.subs_scroll))
        .into_any_element()
    }
    /// The More button that opens a subscription's menu.
    fn subscription_menu_button(&self, id: String, sub: &MailSubscription, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let busy = self.subs_busy.contains(&sub.key());
        div()
            .id(SharedString::from(format!("{id}-wrap")))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .when(busy, |d| d.child(spinner(13., c.muted)))
            .child(
                self.button(id, "@More actions", "ellipsis", Action::SubscriptionMenu(sub.clone()), false, cx)
                    .h(px(28.))
                    .w(px(28.)),
            )
            // The row under it opens the subscription; this must not.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
    }
    fn subscription_row(
        &self,
        index: usize,
        sub: &MailSubscription,
        all: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let name = if sub.name.trim().is_empty() {
            sub.address.clone()
        } else {
            sub.name.clone()
        };
        let mut detail = sub.address.clone();
        if all && let Some(a) = self.accounts.iter().find(|a| a.id == sub.account) {
            detail = format!("{detail} · {}", a.label());
        }
        let badge = |text: String, accent: bool| {
            div()
                .flex_shrink_0()
                .px(px(7.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(c.soft))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if accent { c.accent } else { c.muted }))
                .child(text)
        };
        let open = sub.clone();
        let menu = sub.clone();
        let current = self.subs_sender.as_ref().is_some_and(|s| s.key() == sub.key());
        div()
            .id(SharedString::from(format!("subscription-{index}")))
            .debug_selector(move || format!("subscription-{index}"))
            .h(px(SUB_ROW))
            .w_full()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .bg(rgb(if current { c.soft } else { c.surface }))
            .hover(move |s| s.bg(rgb(if current { c.soft } else { c.bg })))
            .when(index > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::OpenSubscription(open.clone()), w, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = this.subscription_entries(&menu);
                    this.open_menu(e.position, entries, cx)
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
                            .text_size(px(13.))
                            .font_weight(if sub.unread > 0 { FontWeight::BOLD } else { FontWeight::SEMIBOLD })
                            .truncate()
                            .child(name),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("subscription-{index}-address"))
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(detail),
                    )
                    // When its latest message came, under its address, as
                    // the mail list shows times.
                    .when(sub.latest > 0, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("subscription-{index}-latest")))
                                .debug_selector(move || format!("subscription-{index}-latest"))
                                .text_size(px(11.))
                                .text_color(rgb(if sub.unread > 0 { c.accent } else { c.muted }))
                                .font_weight(if sub.unread > 0 {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .truncate()
                                .child(format!("Last email {}", date_short(sub.latest))),
                        )
                    }),
            )
            .when(sub.unread > 0, |d| {
                d.child(badge(format!("{} unread", fmt_count(sub.unread)), true))
            })
            .child(badge(
                format!(
                    "{} message{}",
                    fmt_count(sub.count),
                    if sub.count == 1 { "" } else { "s" }
                ),
                false,
            ))
            .child(self.subscription_menu_button(format!("subscription-{index}-menu"), sub, cx))
            .child(
                div()
                    .id(SharedString::from(format!("subscription-{index}-unsubscribe")))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(self.unsubscribe_button(format!("unsubscribe-{index}"), sub, cx)),
            )
            .into_any_element()
    }
    /// Over a subscription's mail: back to the subscriptions, who it is,
    /// and what to do with all of it (or with the selected messages).
    fn sender_header(&self, sub: &MailSubscription, single: bool, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let name = if sub.name.trim().is_empty() {
            sub.address.clone()
        } else {
            sub.name.clone()
        };
        let total = self.messages.len();
        let unread = self.messages.iter().filter(|m| m.unread).count();
        let actions = if self.selecting && !self.selection.is_empty() {
            let n = self.selection.len();
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    div()
                        .pl_1()
                        .pr_2()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{} selected", fmt_count(n))),
                )
                .child(self.button("sender-selected-read", "@Mark as read", "mail-open", Action::ApplySelection(Change::Read(true)), false, cx).h(px(30.)))
                .child(self.button("sender-selected-archive", "@Archive", "archive", Action::Move(Folder::Archive), false, cx).h(px(30.)))
                .child(self.button("sender-selected-trash", "@Move to trash", "trash", Action::Move(Folder::Trash), false, cx).h(px(30.)))
                .child(div().flex_1())
                .child(self.button("sender-selected-done", "Done", "", Action::StopSelecting, false, cx).h(px(30.)).text_size(px(12.)))
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1())
                .child(self.subscription_menu_button("sender-menu".into(), sub, cx))
                .child(self.unsubscribe_button("unsubscribe-sender".into(), sub, cx))
        };
        div()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .gap_1()
                    // Narrow windows show one pane; wide ones keep the list.
                    .when(single, |d| {
                        d.child(
                            self.button(
                                "subscription-back",
                                "@Back to subscriptions",
                                "arrow-left",
                                Action::CloseSubscription,
                                false,
                                cx,
                            )
                            .h(px(30.)),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .pl_1()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .when(self.subs_sender_loading, |d| d.child(spinner(13., c.muted))),
            )
            .child(
                div()
                    .pl_1()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .truncate()
                    .child(format!(
                        "{} · {} message{} · {} unread",
                        sub.address,
                        fmt_count(total),
                        if total == 1 { "" } else { "s" },
                        fmt_count(unread),
                    )),
            )
            .child(actions)
    }
    fn sender_body(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let count = self.visible.len();
        if count == 0 {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(if self.subs_sender_loading {
                    spinner(20., c.muted).into_any_element()
                } else {
                    icon_el("mail", 28., c.border).into_any_element()
                })
                .child(if self.subs_sender_loading { "Loading…" } else { "No messages" })
                .into_any_element();
        }
        let entity = cx.entity();
        uniform_list("subscription-mail", count, move |range, _, cx| {
            entity.update(cx, |this, cx| {
                let cx: &Context<Self> = cx;
                range
                    .filter_map(|i| Some(this.message_row(*this.visible.get(i)?, cx)))
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(self.list_scroll.clone())
        .flex_1()
        .min_h_0()
        .map(|list| panes::scroll_frame(list, &self.list_scroll))
        .into_any_element()
    }
    /// Over a message opened from a subscription: back to its mail.
    fn subscription_message_bar(&self, sub: &MailSubscription, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let name = if sub.name.trim().is_empty() {
            sub.address.clone()
        } else {
            sub.name.clone()
        };
        div()
            .h(px(40.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                self.button(
                    "subscription-mail-back",
                    "@Back to the list",
                    "arrow-left",
                    Action::BackToList,
                    false,
                    cx,
                )
                .h(px(30.)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child(format!("Subscriptions › {name}")),
            )
    }
}

/// The sender's address in a subscription key.
fn sub_address(key: &str) -> &str {
    key.split_once('\u{1f}').map_or(key, |(_, address)| address)
}
