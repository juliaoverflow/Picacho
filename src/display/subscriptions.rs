//! Mail subscriptions: an email from one offers to unsubscribe, and each
//! mailbox (and All Subscriptions, for every mailbox) lists the senders
//! detected with how many of their messages it holds.
use super::*;
use crate::manage::subscriptions::{self as subs, Outcome};

/// A subscription row's height; the list draws only the rows in view.
const SUB_ROW: f32 = 58.;
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
            let _ = self
                .backend
                .store
                .set_preference(subs::UNSUBSCRIBED_PREFERENCE, &json);
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
    /// The open subscriptions, in place of the message list: each sender,
    /// its address, how many of its messages there are, and Unsubscribe.
    pub(super) fn subscription_list(
        &self,
        single: bool,
        corners: Corners<Pixels>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let all = self.account.as_deref() == Some(ALL_MAIL);
        let count = self.subs.len();
        let total: usize = self.subs.iter().map(|s| s.count).sum();
        let header = div()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .gap(px(2.))
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
                    .pl_1()
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(if count == 0 {
                        "Senders whose mail offers to unsubscribe".to_string()
                    } else {
                        format!(
                            "{} sender{} · {} message{}",
                            fmt_count(count),
                            if count == 1 { "" } else { "s" },
                            fmt_count(total),
                            if total == 1 { "" } else { "s" },
                        )
                    }),
            );
        let body = if count == 0 {
            div()
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
                } else {
                    "No subscriptions found"
                })
                .into_any_element()
        } else {
            let entity = cx.entity();
            uniform_list("subscription-list", count, move |range, _, cx| {
                entity.update(cx, |this, cx| {
                    let cx: &Context<Self> = cx;
                    range
                        .filter_map(|i| Some(this.subscription_row(i, this.subs.get(i)?, all, cx)))
                        .collect::<Vec<_>>()
                })
            })
            .track_scroll(self.subs_scroll.clone())
            .flex_1()
            .min_h_0()
            .map(|list| panes::scroll_frame(list, &self.subs_scroll))
            .into_any_element()
        };
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
            .child(header)
            .child(body)
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
        div()
            .id(SharedString::from(format!("subscription-{index}")))
            .h(px(SUB_ROW))
            .w_full()
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .when(index > 0, |d| d.border_t_1().border_color(rgb(c.border)))
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(detail),
                    ),
            )
            .child(
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
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "{} message{}",
                        fmt_count(sub.count),
                        if sub.count == 1 { "" } else { "s" }
                    )),
            )
            .child(self.unsubscribe_button(format!("unsubscribe-{index}"), sub, cx))
            .into_any_element()
    }
}

/// The sender's address in a subscription key.
fn sub_address(key: &str) -> &str {
    key.split_once('\u{1f}').map_or(key, |(_, address)| address)
}
