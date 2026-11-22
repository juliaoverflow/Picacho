//! Snoozing messages, searching the whole mailbox on the server, read
//! receipts, and forwarding a message's attachments or the message itself.
use super::*;
use crate::manage::backend::{ServerResults, Snoozed};

/// A server-side search shown in place of the list's own search.
pub(super) struct ServerSearch {
    pub(super) account: String,
    pub(super) mailbox: Mailbox,
    pub(super) query: String,
    pub(super) running: bool,
    pub(super) ids: HashSet<String>,
    pub(super) total: usize,
    pub(super) error: Option<String>,
}

/// The newest matches a server search lists.
const SERVER_RESULTS: usize = 200;

impl MailApp {
    // Snoozing.

    /// Asks when to bring messages back (the selection, when `ids` is empty).
    pub(super) fn snooze_menu(&mut self, ids: Vec<String>, w: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = if ids.is_empty() {
            self.selection.iter().cloned().collect()
        } else {
            ids
        };
        let snoozable: Vec<String> = self
            .messages
            .iter()
            .filter(|m| ids.contains(&m.id) && !m.pending)
            .filter(|m| {
                self.account_of(m)
                    .is_some_and(|a| a.provider != Provider::Pop3)
            })
            .map(|m| m.id.clone())
            .collect();
        if snoozable.is_empty() {
            self.notice("Snoozing needs mail kept on the server (IMAP).", true, cx);
            return;
        }
        let presets = crate::manage::composing::snooze_choices(chrono::Local::now());
        let colors = self.colors();
        let parser = self.when.clone();
        let mobile = self.mobile.on;
        let picker = cx.new(|cx| {
            DateTimePicker::new("snooze", "Snooze until", presets, colors)
                .phone(mobile)
                .typed(parser, w, cx)
        });
        let chosen = snoozable.clone();
        self.picker_subscription = Some(cx.subscribe_in(&picker, w, move |this, _, e, w, cx| {
            match e {
                PickerEvent::Picked(at) => {
                    this.modal = None;
                    this.picker = None;
                    this.snooze(chosen.clone(), *at, cx);
                }
                PickerEvent::Cancel => this.close(w, cx),
            }
            cx.notify();
        }));
        self.picker = Some(picker);
        self.modal = Some(Modal::Snooze(snoozable));
    }
    /// The snooze dialog: the picker.
    pub(super) fn snooze_modal(&self) -> AnyElement {
        match &self.picker {
            Some(picker) => picker.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }
    fn snooze(&mut self, ids: Vec<String>, until: i64, cx: &mut Context<Self>) {
        let ids: HashSet<String> = ids.into_iter().collect();
        let chosen: Vec<Mail> = self
            .messages
            .iter()
            .filter(|m| ids.contains(&m.id))
            .cloned()
            .collect();
        let mut groups: Vec<(Account, Vec<Mail>)> = vec![];
        for m in chosen {
            let Some(a) = self.account_of(&m) else {
                continue;
            };
            match groups.iter_mut().find(|(g, _)| g.id == a.id) {
                Some((_, ms)) => ms.push(m),
                None => groups.push((a, vec![m])),
            }
        }
        // They leave the list at once; a failure brings them back.
        if self.folder != Folder::All {
            self.messages.retain(|m| !ids.contains(&m.id));
        }
        if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
            self.selected = None;
        }
        self.selection.retain(|s| !ids.contains(s));
        self.filter(cx);
        let when = crate::manage::composing::describe_later(until, chrono::Local::now());
        for (a, mails) in groups {
            let count = mails.len();
            let account_id = a.id.clone();
            let when = when.clone();
            let message_ids: Vec<String> =
                mails.iter().map(|m| m.message_id.trim().to_owned()).collect();
            self.task(
                cx,
                move |b| {
                    b.snooze(&a, &mails, until)?;
                    Ok(b.snoozed(&a.id))
                },
                move |this, result, cx| {
                    this.account_result(&account_id, IssueSource::Change, &result);
                    match result {
                        Ok(list) => {
                            this.snoozed.insert(account_id.clone(), list);
                            this.notice_undo(
                                if count == 1 {
                                    format!("Snoozed until {when}")
                                } else {
                                    format!("Snoozed {count} messages until {when}")
                                },
                                Undo::Snooze(account_id.clone(), message_ids),
                                cx,
                            );
                            if let Some(a) = this.accounts.iter().find(|a| a.id == account_id) {
                                this.folders
                                    .insert(a.id.clone(), this.backend.store.folders(&a.id));
                            }
                            this.views_changed(cx);
                        }
                        Err(e) => this.notice(format!("Couldn't snooze: {e:#}"), true, cx),
                    }
                    this.reload_cached(cx);
                    this.refresh_counts(cx);
                },
            );
        }
        cx.notify();
    }
    /// Loads each account's snoozed messages (kept on this computer).
    pub(super) fn load_snoozed(&mut self) {
        self.snoozed = self
            .accounts
            .iter()
            .map(|a| (a.id.clone(), self.backend.snoozed(&a.id)))
            .collect();
    }
    /// When a message comes back, if it is snoozed.
    pub(super) fn snoozed_until(&self, m: &Mail) -> Option<i64> {
        if m.message_id.is_empty() {
            return None;
        }
        self.snoozed
            .get(&m.account)?
            .iter()
            .find(|s| s.message_id == m.message_id)
            .map(|s| s.until)
    }
    /// Brings back snoozed messages whose time has come (checked with the
    /// regular timer; accounts that may have some snoozed on another
    /// computer are asked now and then too).
    pub(super) fn wake_snoozed(&mut self, cx: &mut Context<Self>) {
        let now = chrono::Utc::now().timestamp();
        let due: Vec<Account> = self
            .accounts
            .iter()
            .filter(|a| a.provider != Provider::Pop3 && !self.account_blocked(&a.id))
            .filter(|a| !self.waking.contains(&a.id))
            .filter(|a| {
                self.snoozed
                    .get(&a.id)
                    .is_some_and(|list| list.iter().any(|s| s.until <= now))
            })
            .cloned()
            .collect();
        for a in due {
            self.wake_account(a, Vec::new(), cx);
        }
    }
    /// Wakes an account's due snoozes, and `now` (by Message-ID) at once.
    pub(super) fn wake_account(&mut self, a: Account, now: Vec<String>, cx: &mut Context<Self>) {
        if !self.waking.insert(a.id.clone()) {
            return;
        }
        let account_id = a.id.clone();
        self.task(
            cx,
            move |b| {
                let woken: Vec<Snoozed> = if now.is_empty() {
                    b.wake_due(&a)?
                } else {
                    b.unsnooze(&a, &now)?
                };
                Ok((woken, b.snoozed(&a.id)))
            },
            move |this, result, cx| {
                this.waking.remove(&account_id);
                match result {
                    Ok((woken, list)) => {
                        this.snoozed.insert(account_id.clone(), list);
                        if !woken.is_empty() {
                            let text = match woken.as_slice() {
                                [one] => format!("Back from snooze: {}", one.subject),
                                many => {
                                    format!("{} snoozed messages are back in the inbox", many.len())
                                }
                            };
                            this.notice(text, false, cx);
                            if let Some(a) =
                                this.accounts.iter().find(|a| a.id == account_id).cloned()
                            {
                                this.check_inbox(a, cx);
                            }
                            if this.shows(&account_id) {
                                this.reload_cached(cx);
                            }
                        }
                    }
                    Err(e) => {
                        if !crate::sync::retry::later(&e) {
                            this.notice(
                                format!("Couldn't bring back snoozed mail: {e:#}"),
                                true,
                                cx,
                            );
                        }
                    }
                }
            },
        );
    }
    /// Brings a snoozed message back to the inbox now.
    pub(super) fn unsnooze(&mut self, account: String, message_id: String, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else {
            return;
        };
        self.selected = None;
        self.wake_account(a, vec![message_id], cx);
    }
    /// Above a snoozed message: when it comes back, and a way to bring it
    /// back now.
    pub(super) fn snooze_banner(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        let until = self.snoozed_until(m)?;
        let c = self.colors();
        let when = crate::manage::composing::describe_later(until, chrono::Local::now());
        let unsnooze = self
            .button(
                "unsnooze",
                "Unsnooze",
                "inbox",
                Action::Unsnooze(m.account.clone(), m.message_id.clone()),
                false,
                cx,
            )
            .border_1()
            .border_color(rgb(c.border));
        let resnooze = self.button(
            "resnooze",
            "@Snooze until another time",
            "clock",
            Action::SnoozeMenu(vec![m.id.clone()]),
            false,
            cx,
        );
        Some(
            self.banner(
                "snooze-banner",
                "snooze",
                BannerStyle::Filled,
                format!("Snoozed until {when}. It comes back to the inbox, unread."),
                [unsnooze.into_any_element(), resnooze.into_any_element()],
            )
            .into_any_element(),
        )
    }

    // Server search.

    /// The account a server search of the open list would search, when it
    /// can: one IMAP account's mailbox (not Drafts).
    pub(super) fn server_searchable(&self) -> Option<Account> {
        let id = self.account.as_ref().filter(|id| !fwd::is_view(id))?;
        if self.folder == Folder::Drafts || self.subs_open {
            return None;
        }
        self.accounts
            .iter()
            .find(|a| &a.id == id && a.provider != Provider::Pop3)
            .cloned()
    }
    /// The server search for the current query and mailbox, if one was made.
    pub(super) fn current_server_search(&self, query: &str) -> Option<&ServerSearch> {
        self.server_search.as_ref().filter(|s| {
            s.query == query
                && Some(&s.account) == self.account.as_ref()
                && s.mailbox == self.mailbox()
        })
    }
    /// Searches the whole mailbox on the server for what is typed.
    pub(super) fn search_server(&mut self, cx: &mut Context<Self>) {
        let query = Self::value(&self.search, cx).trim().to_string();
        let Some(a) = self.server_searchable() else {
            return;
        };
        if query.is_empty() {
            return;
        }
        if self
            .current_server_search(&query)
            .is_some_and(|s| s.running)
        {
            return;
        }
        let mailbox = self.mailbox();
        self.server_search = Some(ServerSearch {
            account: a.id.clone(),
            mailbox: mailbox.clone(),
            query: query.clone(),
            running: true,
            ids: HashSet::new(),
            total: 0,
            error: None,
        });
        let account_id = a.id.clone();
        let generation = self.load_generation;
        let asked = query.clone();
        self.task(
            cx,
            move |b| b.search_server(&a, &mailbox, &asked, SERVER_RESULTS),
            move |this, result: anyhow::Result<ServerResults>, cx| {
                if generation != this.load_generation {
                    return;
                }
                let Some(search) = this
                    .server_search
                    .as_mut()
                    .filter(|s| s.query == query && s.account == account_id)
                else {
                    return;
                };
                search.running = false;
                match result {
                    Ok(found) => {
                        search.total = found.total;
                        search.ids = found.mails.iter().map(|m| m.id.clone()).collect();
                        let found = this.backend.with_readable(found.mails);
                        for m in found {
                            if !this.messages.iter().any(|x| x.id == m.id) {
                                this.messages.push(m);
                            }
                        }
                        this.list_revision += 1;
                    }
                    Err(e) => search.error = Some(format!("{e:#}")),
                }
                this.apply_filter(cx);
                this.scroll_list_to_top();
            },
        );
        self.apply_filter(cx);
        cx.notify();
    }
    /// Below the search field: search the server, or what it found.
    pub(super) fn server_search_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let query = Self::value(&self.search, cx).trim().to_string();
        if query.is_empty() {
            return None;
        }
        let a = self.server_searchable()?;
        let c = self.colors();
        let gmail = a.provider == Provider::Gmail;
        // The list can be narrow: the text wraps, and past that the link
        // goes below it, on the right, rather than past the list's edge.
        let line = div()
            .id("server-search")
            .debug_selector(|| "server-search".into())
            .pt_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .text_size(px(12.))
            .text_color(rgb(c.muted));
        let text = |t: String| div().flex_1().min_w(px(120.)).child(t);
        let action = |link: Stateful<Div>| div().ml_auto().flex_shrink_0().child(link);
        Some(match self.current_server_search(&query) {
            Some(s) if s.running => line
                .child(spinner(12., c.accent))
                .child(text(format!("Searching {} on the server…", self.mailbox_label())))
                .into_any_element(),
            Some(s) if s.error.is_some() => line
                .child(icon_el("warning", 13., c.danger))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(s.error.clone().unwrap_or_default()),
                )
                .child(self.link("server-search-retry", "Try Again", Action::SearchServer, cx))
                .into_any_element(),
            Some(s) => line
                .child(icon_el("cloud", 13., c.accent))
                .child(text(if s.total > s.ids.len() {
                    format!(
                        "The newest {} of {} matches on the server",
                        fmt_count(s.ids.len()),
                        fmt_count(s.total)
                    )
                } else if s.total == 0 {
                    "No matches on the server".to_owned()
                } else {
                    format!("{} matches on the server", fmt_count(s.total))
                }))
                .child(action(self.link(
                    "server-search-clear",
                    "Show Local Matches",
                    Action::ClearServerSearch,
                    cx,
                )))
                .into_any_element(),
            None => line
                .child(icon_el("cloud", 13., c.muted))
                .child(text("Only mail on this computer is searched.".to_owned()))
                .child(action(self.link(
                    "server-search-start",
                    if gmail {
                        "Search Gmail · ↩"
                    } else {
                        "Search the Server · ↩"
                    },
                    Action::SearchServer,
                    cx,
                )))
                .into_any_element(),
        })
    }

    // Read receipts.

    /// Above a message whose sender asked for a read receipt: send it, or
    /// don't (either way, no mail app asks again).
    pub(super) fn receipt_banner(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        if m.receipt_to().is_empty()
            || m.mdn_sent
            || matches!(m.folder, Folder::Sent | Folder::Drafts | Folder::Junk)
        {
            return None;
        }
        let a = self.account_of(m)?;
        // Not for mail it sent itself.
        if m.from.eq_ignore_ascii_case(&a.email) {
            return None;
        }
        let c = self.colors();
        let decline = self
            .button(
                "receipt-decline",
                "Don't Send",
                "",
                Action::Receipt(m.id.clone(), false),
                false,
                cx,
            )
            .border_1()
            .border_color(rgb(c.border));
        let send = self.button(
            "receipt-send",
            "Send Receipt",
            "",
            Action::Receipt(m.id.clone(), true),
            true,
            cx,
        );
        Some(
            self.banner(
                "receipt-banner",
                "mail-open",
                BannerStyle::Outlined,
                format!("{} asks to be told when you've read this message.", m.sender),
                [decline.into_any_element(), send.into_any_element()],
            )
            .into_any_element(),
        )
    }
    pub(super) fn answer_receipt(&mut self, id: String, send: bool, cx: &mut Context<Self>) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let Some(a) = self.account_of(&m) else {
            return;
        };
        for x in self.messages.iter_mut().filter(|x| x.id == id) {
            x.mdn_sent = true;
        }
        let account_id = a.id.clone();
        self.task(
            cx,
            move |b| {
                if send {
                    b.send_receipt(&a, &m)
                } else {
                    b.decline_receipt(&a, &m)
                }
            },
            move |this, result, cx| match result {
                Ok(()) if send => this.notice("Read receipt sent", false, cx),
                Ok(()) => {}
                Err(e) => {
                    let _ = &account_id;
                    for x in this.messages.iter_mut().filter(|x| x.id == id) {
                        x.mdn_sent = false;
                    }
                    this.notice(format!("The read receipt wasn't sent: {e:#}"), true, cx);
                }
            },
        );
        cx.notify();
    }

    // Forwarding.

    /// Attaches files to a composer once they are ready: a forward's
    /// attachments (`whole`: false) or the message itself (true).
    pub(super) fn attach_forwarded(
        &mut self,
        composer: Entity<Composer>,
        m: Mail,
        whole: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(a) = self.account_of(&m) else {
            return;
        };
        let draft = composer.read(cx).draft.id.clone();
        let weak = composer.downgrade();
        self.task(
            cx,
            move |b| {
                if whole {
                    Ok(vec![b.forward_as_attachment(&a, &m, &draft)?])
                } else {
                    b.forward_files(&a, &m, &draft)
                }
            },
            move |this, result, cx| match result {
                Ok(paths) if !paths.is_empty() => {
                    let _ = weak.update(cx, |c, cx| c.add_files(paths, cx));
                }
                Ok(_) => {}
                Err(e) => this.notice(
                    format!("The original's attachments couldn't be added: {e:#}"),
                    true,
                    cx,
                ),
            },
        );
    }
}
