//! Follow-up reminders in the window: "Remind me if no reply" on sent mail
//! (set in the composer, or afterwards from the message), the banner above
//! a message waited on, the reminder when nobody answered in time, and
//! the Waiting For view: the Sent mail still waited on, for an account or
//! All Accounts. The waiting itself is kept by the backend
//! (`manage::backend::follow_up`); the background monitor reminds while no
//! window is open.
use super::*;
use crate::manage::backend::follow_up::{FollowUp, FollowUpCheck};

/// How often replies are looked for even when nothing changed here:
/// another process (the background monitor's) may have synced, or waited.
const REPLIES_AGAIN: Duration = Duration::from_secs(10 * 60);

impl MailApp {
    /// Loads each account's sent mail waited on (kept on this computer).
    pub(super) fn load_follow_ups(&mut self) {
        self.follow_ups = self
            .accounts
            .iter()
            .map(|a| (a.id.clone(), self.backend.follow_ups(&a.id)))
            .collect();
    }
    /// The wait on a sent message, if it is waited on.
    pub(super) fn follow_up_of(&self, m: &Mail) -> Option<&FollowUp> {
        let id = m.message_id.trim();
        if id.is_empty() {
            return None;
        }
        self.follow_ups
            .get(&m.account)?
            .iter()
            .find(|f| f.message_id == id)
    }
    /// The accounts the Waiting For of `id` covers: the account, or every
    /// account for All Accounts'.
    fn waiting_accounts(&self, id: &str) -> Vec<&str> {
        self.accounts
            .iter()
            .filter(|a| id == ALL_MAIL || a.id == id)
            .map(|a| a.id.as_str())
            .collect()
    }
    /// How many messages `id` (an account, or All Accounts) waits on, and
    /// how many of them are overdue.
    pub(super) fn waiting_counts(&self, id: &str) -> (usize, usize) {
        let now = chrono::Utc::now().timestamp();
        self.waiting_accounts(id)
            .into_iter()
            .filter_map(|a| self.follow_ups.get(a))
            .flatten()
            .fold((0, 0), |(all, over), f| (all + 1, over + usize::from(f.overdue(now))))
    }
    /// The (account, Message-ID)s the open Waiting For lists.
    pub(super) fn waited_on(&self) -> HashSet<(String, String)> {
        self.follow_ups
            .iter()
            .filter(|(a, _)| self.scope.accounts.contains(a))
            .flat_map(|(a, list)| list.iter().map(move |f| (a.clone(), f.message_id.clone())))
            .collect()
    }

    /// Looks at what is waited on (from the regular timer, and after
    /// sending): replies stop the wait (looked for when mail changed), and
    /// reminders whose time came are given, once. Reads the cache only.
    pub(super) fn check_follow_ups(&mut self, cx: &mut Context<Self>) {
        if self.follow_ups_checking {
            return;
        }
        let now = chrono::Utc::now().timestamp();
        let mut jobs: Vec<(Account, bool, (u64, u64))> = Vec::new();
        for a in &self.accounts {
            let revision = self.backend.list_revision(&a.id);
            let list = self.follow_ups.get(&a.id).map_or(&[][..], Vec::as_slice);
            let seen = self.follow_up_replies.get(&a.id);
            // Never looked at, or not for a while: read again whatever this
            // window knows (what's waited on may have been set elsewhere).
            let stale = seen.is_none_or(|(_, at)| at.elapsed() >= REPLIES_AGAIN);
            // Mail changed since: replies are looked for, where something waits.
            let changed = seen.is_some_and(|(r, _)| *r != revision);
            let replies = stale || (changed && !list.is_empty());
            let due = list.iter().any(|f| !f.reminded && f.overdue(now));
            if replies || due {
                jobs.push((a.clone(), replies, revision));
            }
        }
        if jobs.is_empty() {
            return;
        }
        self.follow_ups_checking = true;
        self.task(
            cx,
            move |b| {
                let mut out = Vec::new();
                for (a, replies, revision) in jobs {
                    // Nothing waited on here: only what another computer
                    // may have shared (now and then) is read.
                    let check = if b.follow_ups(&a.id).is_empty() && !b.sync_follow_ups(&a, false).unwrap_or(false) {
                        FollowUpCheck::default()
                    } else {
                        b.check_follow_ups_online(&a, replies)?
                    };
                    out.push((a.id.clone(), replies, revision, check, b.follow_ups(&a.id)));
                }
                Ok(out)
            },
            move |this, result, cx| {
                this.follow_ups_checking = false;
                let Ok(results) = result else {
                    return;
                };
                let mut changed = false;
                let (mut due, mut replied) = (Vec::new(), Vec::new());
                for (account, replies, revision, check, list) in results {
                    if replies {
                        this.follow_up_replies
                            .insert(account.clone(), (revision, std::time::Instant::now()));
                    }
                    changed |= this.follow_ups.get(&account) != Some(&list);
                    // Out of sight, the system tells, as it does of new mail.
                    if !this.active
                        && !this.retired
                        && let Some(latest) = check.due.iter().max_by_key(|f| f.due)
                        && let Some(a) = this.accounts.iter().find(|a| a.id == account)
                    {
                        let subject = this.hide_codes_in(&latest.subject);
                        crate::display::notify::announce(
                            crate::manage::background::Announcement::NoReply {
                                account: &a.email,
                                account_id: &a.id,
                                message_id: &latest.message_id,
                                to: &latest.to,
                                subject: &subject,
                                count: check.due.len(),
                            },
                        );
                    }
                    this.follow_ups.insert(account, list);
                    due.extend(check.due);
                    replied.extend(check.replied);
                }
                match due.as_slice() {
                    [] => {}
                    // A notice has no Show code to click.
                    [one] => this.notice(
                        t!("follow-up-no-reply-yet-to", subject = this.hide_codes_in(&one.subject)),
                        false,
                        cx,
                    ),
                    many => this.notice(t!("follow-up-messages-without-a-reply", count = many.len()), false, cx),
                }
                if due.is_empty() {
                    match replied.as_slice() {
                        [] => {}
                        [(f, who)] => this.notice(
                            t!(
                                "follow-up-replied-no-longer-waiting",
                                sender = who.clone(),
                                subject = this.hide_codes_in(&f.subject)
                            ),
                            false,
                            cx,
                        ),
                        many => this.notice(t!("follow-up-replies-came", count = many.len()), false, cx),
                    }
                }
                if changed {
                    this.follow_ups_changed(cx);
                }
            },
        );
    }
    /// Looks at every account's follow-ups again, whatever changed (after
    /// sending: what was sent may be waited on now).
    pub(super) fn recheck_follow_ups(&mut self, cx: &mut Context<Self>) {
        self.follow_up_replies.clear();
        self.check_follow_ups(cx);
    }
    /// After what's waited on changed: the Waiting For list follows.
    fn follow_ups_changed(&mut self, cx: &mut Context<Self>) {
        if self.waiting_open {
            self.reload_cached(cx);
            self.filter(cx);
        }
        cx.notify();
    }

    /// Asks when to be reminded if sent message `id` gets no reply.
    pub(super) fn follow_up_menu(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        if m.message_id.trim().is_empty() {
            self.notice(t!("follow-up-no-message-id"), true, cx);
            return;
        }
        let presets = crate::manage::backend::follow_up::remind_choices(chrono::Local::now());
        let due = self.follow_up_of(&m).map(|f| f.due);
        let colors = self.colors();
        let parser = self.when.clone();
        let mobile = self.mobile.on;
        let picker = cx.new(|cx| {
            let picker = DateTimePicker::new("follow-up", t!("follow-up-remind-me"), presets, colors)
                .phone(mobile);
            let picker = match due {
                Some(at) => picker.at(at),
                None => picker,
            };
            picker.typed(parser, w, cx)
        });
        let chosen = id.clone();
        self.picker_subscription = Some(cx.subscribe_in(&picker, w, move |this, _, e, w, cx| {
            match e {
                PickerEvent::Picked(at) => {
                    this.modal = None;
                    this.picker = None;
                    this.wait_for_reply(chosen.clone(), *at, cx);
                }
                PickerEvent::Cancel => this.close(w, cx),
            }
            cx.notify();
        }));
        self.picker = Some(picker);
        self.modal = Some(Modal::FollowUp(id));
    }
    /// The reminder dialog: the picker.
    pub(super) fn follow_up_modal(&self) -> AnyElement {
        self.snooze_modal()
    }
    /// Reminds about sent message `id` at `due` if nobody has replied by then.
    pub(super) fn wait_for_reply(&mut self, id: String, due: i64, cx: &mut Context<Self>) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(a) = self.account_of(&m) else {
            return;
        };
        let account_id = a.id.clone();
        let when = crate::manage::composing::describe_later(due, chrono::Local::now());
        self.task(
            cx,
            move |b| {
                b.wait_for_reply(&a, &m, due)?;
                // Other computers wait on it too.
                let _ = b.sync_follow_ups(&a, true);
                Ok(b.follow_ups(&a.id))
            },
            move |this, result, cx| match result {
                Ok(list) => {
                    this.follow_ups.insert(account_id, list);
                    this.notice(t!("follow-up-reminds-you", when = when), false, cx);
                    this.follow_ups_changed(cx);
                }
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }
    /// Stops waiting for a reply to a sent message.
    pub(super) fn stop_waiting(&mut self, account: String, message_id: String, cx: &mut Context<Self>) {
        // Gone from the list at once.
        if let Some(list) = self.follow_ups.get_mut(&account) {
            list.retain(|f| f.message_id != message_id.trim());
        }
        self.follow_ups_changed(cx);
        let account_id = account.clone();
        let a = self.accounts.iter().find(|a| a.id == account).cloned();
        self.task(
            cx,
            move |b| {
                b.stop_waiting(&account, &[message_id])?;
                // Other computers stop waiting too.
                if let Some(a) = a {
                    let _ = b.sync_follow_ups(&a, true);
                }
                Ok(b.follow_ups(&account))
            },
            move |this, result, cx| match result {
                Ok(list) => {
                    this.follow_ups.insert(account_id, list);
                    this.notice(t!("follow-up-no-longer-waiting"), false, cx);
                    this.follow_ups_changed(cx);
                }
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// Above a sent message waited on: when the reminder comes (or that it
    /// came, with no reply), a way to follow up, to choose another time, and
    /// to stop waiting.
    pub(super) fn follow_up_banner(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        let f = self.follow_up_of(m)?;
        let c = self.colors();
        let now = chrono::Local::now();
        let overdue = f.overdue(now.timestamp());
        let stop = self
            .button(
                "stop-waiting",
                t!("follow-up-stop-waiting"),
                "close",
                Action::StopWaiting(m.account.clone(), f.message_id.clone()),
                false,
                cx,
            )
            .border_1()
            .border_color(rgb(c.border));
        let again = self
            .button(
                "remind-again",
                if overdue { t!("follow-up-remind-me-again") } else { t!("follow-up-change-reminder") },
                "clock",
                Action::FollowUpMenu(m.id.clone()),
                false,
                cx,
            )
            .border_1()
            .border_color(rgb(c.border));
        let mut actions = vec![stop.into_any_element(), again.into_any_element()];
        if overdue {
            // Replying to your own message answers its recipients.
            actions.push(
                self.button(
                    "follow-up-reply",
                    t!("follow-up-follow-up"),
                    "reply-all",
                    Action::ReplyTo(m.id.clone(), 2),
                    true,
                    cx,
                )
                .into_any_element(),
            );
        }
        let when = crate::manage::composing::describe_later(f.due, now);
        let text = if overdue {
            t!("follow-up-no-reply-since", sent = describe_since(f.sent, now))
        } else {
            t!("follow-up-waiting-reminds-you", when = when)
        };
        Some(
            self.banner(
                "follow-up-banner",
                "waiting",
                BannerStyle::Filled,
                text,
                actions,
            )
            .into_any_element(),
        )
    }

    // The Waiting For view.

    /// Shows the sent mail `id` (an account, or All Accounts) waits on.
    pub(super) fn open_waiting(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        self.open_mailbox(Some(id), Folder::Sent.into(), w, cx);
        // Listed afresh: what's waited on, not the whole of Sent.
        self.load_generation += 1;
        self.waiting_open = true;
        self.messages.clear();
        self.visible.clear();
        self.layout = None;
        let generation = self.load_generation;
        self.cache_loading = true;
        let job = self.list_job(Folder::Sent.into());
        self.list_task(cx, job, move |this, result, cx| {
            if generation != this.load_generation {
                return;
            }
            this.cache_loading = false;
            if let Ok((ms, layout, key)) = result {
                this.layout = layout;
                this.set_messages(ms, cx);
                this.listed = Some(key);
            }
            // Sent is checked, so mail just sent is listed.
            this.refresh(cx);
        });
        self.save_place();
        cx.notify();
    }
    /// "Waiting For" under an account, or under All Accounts: shown while
    /// it waits on something, or is open.
    pub(super) fn waiting_nav(&self, id: &str, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let c = self.colors();
        let active = self.waiting_open && self.account.as_deref() == Some(id);
        let (all, overdue) = self.waiting_counts(id);
        if all == 0 && !active {
            return None;
        }
        let action = Action::OpenWaiting(id.to_owned());
        let selector = format!("nav-{id}-waiting");
        let spoken = match overdue {
            0 => t!("follow-up-waiting-for").to_owned(),
            n => t!("follow-up-waiting-for-overdue", count = n),
        };
        Some(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(|| selector)
                .role(Role::TreeItem)
                .aria_label(spoken)
                .aria_selected(active)
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
                .child(icon_el("waiting", 16., if active { c.accent } else { c.muted }))
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
                        .child(t!("follow-up-waiting-for")),
                )
                // Overdue ones stand out; otherwise how many wait, quietly.
                .when(all > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if overdue > 0 || active { c.accent } else { c.muted }))
                            .child(fmt_count(if overdue > 0 { overdue } else { all })),
                    )
                })
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx))),
        )
    }
    /// The same in the collapsed sidebar.
    pub(super) fn waiting_rail(&self, id: &str, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let c = self.colors();
        let active = self.waiting_open && self.account.as_deref() == Some(id);
        let (all, overdue) = self.waiting_counts(id);
        if all == 0 && !active {
            return None;
        }
        Some(self.rail_button(
            format!("rail-{id}-waiting"),
            t!("follow-up-waiting-for").into(),
            icon_el("waiting", 17., if active || overdue > 0 { c.accent } else { c.muted })
                .into_any_element(),
            active,
            Action::OpenWaiting(id.to_owned()),
            cx,
        ))
    }
}

/// The sent mail a Waiting For of `accounts` lists, from the cache: each
/// message waited on, its copy in Sent where there is one, newest first.
pub(super) fn waiting_list(b: &Backend, accounts: &[String]) -> anyhow::Result<Vec<Mail>> {
    let mut all = Vec::new();
    for id in accounts {
        for f in b.follow_ups(id) {
            let mut copies = b.store.copies_of(id, &f.message_id)?;
            if let Some(i) = copies.iter().position(|m| m.folder == Folder::Sent) {
                all.push(copies.swap_remove(i));
            } else if !copies.is_empty() {
                all.push(copies.swap_remove(0));
            }
        }
    }
    all.sort_by(Mail::newest_first);
    Ok(b.with_readable_previews(all))
}

/// When something was sent, as a sentence part: "today", "yesterday", "3
/// days ago".
fn describe_since<Tz: chrono::TimeZone>(sent: i64, now: chrono::DateTime<Tz>) -> String {
    let Some(at) = chrono::DateTime::from_timestamp(sent, 0).map(|t| t.with_timezone(&now.timezone()))
    else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days().max(0);
    match days {
        0 => t!("follow-up-sent-today").to_owned(),
        1 => t!("follow-up-sent-yesterday").to_owned(),
        n => t!("follow-up-sent-days-ago", count = n),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    #[test]
    fn how_long_ago_it_was_sent() {
        let tz = chrono::FixedOffset::east_opt(3600).unwrap();
        let now = tz.with_ymd_and_hms(2025, 11, 19, 10, 20, 0).unwrap();
        let at = |d: u32, h: u32| tz.with_ymd_and_hms(2025, 11, d, h, 0, 0).unwrap().timestamp();
        assert_eq!(super::describe_since(at(19, 8), now), "today");
        assert_eq!(super::describe_since(at(18, 23), now), "yesterday");
        assert_eq!(super::describe_since(at(15, 9), now), "4 days ago");
    }
}
