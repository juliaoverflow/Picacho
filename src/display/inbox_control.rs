//! Inbox control in the window (see `manage::inbox_control`): the VIP mark
//! beside a sender, the inbox's VIPs and Screener views (`Lens`) with the
//! Screener's Approve / Block / Category decisions, their sidebar links,
//! the message menu's entries, which new mail is announced, and Settings →
//! Inbox Control (VIPs, the Screener, notifications and quiet hours).
use super::*;
use crate::manage::categories::Category;
use crate::manage::inbox_control::{EVERY_DAY, NotifyMode, QuietWindow, WEEKDAYS, WEEKEND};

/// How the inbox is looked at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Lens {
    /// The inbox, without what waits in the Screener.
    #[default]
    Inbox,
    /// Only VIPs' mail.
    Vips,
    /// Only first-time senders' mail, waiting for a decision.
    Screener,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ControlAction {
    /// Shows the inbox through a lens, or whole again.
    Lens(Lens),
    /// Makes a message's sender a VIP, or no longer one (its id).
    ToggleVip(String),
    /// Adds the address or @domain typed in Settings as a VIP.
    AddVip,
    RemoveVip(String),
    /// Lets these messages' senders into the inbox (none: the sender the
    /// Screener shows).
    Approve(Vec<String>),
    /// Blocks these messages' senders: their mail goes to Trash.
    Block(Vec<String>),
    /// Lists the categories to route these messages' senders to.
    RouteMenu(Vec<String>),
    /// Approves these messages' senders and sorts them into a category.
    Route(Vec<String>, Category),
    ToggleScreener,
    ForgetApproval(String),
    SetMode(NotifyMode),
    ToggleSilent(Category),
    ToggleQuiet,
    ToggleVipsBreak,
    /// Adds the quiet window typed in Settings.
    AddWindow,
    RemoveWindow(usize),
    /// Turns a window's day (0 = Monday) on or off.
    ToggleDay(usize, u32),
    /// The days a new window starts on.
    NewDays(u8),
    /// Pauses notifications for this many minutes (0: until tomorrow
    /// morning).
    Pause(i64),
    Resume,
}

/// Settings' fields for inbox control.
pub(super) struct ControlUi {
    pub(super) vip: Entity<InputState>,
    pub(super) quiet_start: Entity<InputState>,
    pub(super) quiet_end: Entity<InputState>,
    pub(super) new_days: u8,
}
impl ControlUi {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let quiet_start = input(t!("inbox-control-starts-placeholder"), false, window, cx);
        let quiet_end = input(t!("inbox-control-ends-placeholder"), false, window, cx);
        Self {
            vip: input(t!("inbox-control-vip-placeholder"), false, window, cx),
            quiet_start,
            quiet_end,
            new_days: EVERY_DAY,
        }
    }
}

/// The days of the week, Monday first, as a quiet window's buttons name
/// them.
pub(super) fn day_names() -> [&'static str; 7] {
    [
        t!("inbox-control-day-mon"),
        t!("inbox-control-day-tue"),
        t!("inbox-control-day-wed"),
        t!("inbox-control-day-thu"),
        t!("inbox-control-day-fri"),
        t!("inbox-control-day-sat"),
        t!("inbox-control-day-sun"),
    ]
}

/// A window's days in words: every day, weekdays, weekends, or each.
pub(super) fn days_text(days: u8) -> String {
    match days & EVERY_DAY {
        EVERY_DAY => t!("inbox-control-every-day").into(),
        WEEKDAYS => t!("inbox-control-weekdays").into(),
        WEEKEND => t!("inbox-control-weekends").into(),
        0 => t!("inbox-control-no-days").into(),
        d => {
            let names = day_names();
            (0..7)
                .filter(|i| d & (1 << i) != 0)
                .map(|i| names[i])
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

impl MailApp {
    fn save_control(&mut self, cx: &mut Context<Self>) {
        self.control.save(&self.backend.store);
        self.filter(cx);
        cx.notify();
    }

    /// Whether `m` waits in the Screener. Nothing does until who is known
    /// has been read (everyone would be new).
    pub(super) fn screened(&self, m: &Mail) -> bool {
        self.control.screener.enabled
            && !m.pending
            && self.sorting.known_ready()
            && self
                .control
                .screens(m, &self.sorting.sorter.known, self.sorting.prefs())
    }

    /// Whether the open list is an inbox the lenses apply to: not
    /// searched, nor results, subscriptions or a summary in its place.
    pub(super) fn lens_applies(&self) -> bool {
        self.folder == Folder::Inbox
            && self.local_results.is_none()
            && !self.subs_open
            && !self.summary_open
    }

    /// After the list is filtered: keeps the lens's rows (VIPs', or the
    /// Screener's), or leaves out the Screener's from the inbox.
    pub(super) fn keep_lens(&mut self, searching: bool) {
        self.screened_waiting = 0;
        if !self.lens_applies() {
            return;
        }
        let screening = self.control.screener.enabled && self.sorting.known_ready();
        if screening {
            self.screened_waiting = self.messages.iter().filter(|m| self.screened(m)).count();
        }
        if searching {
            return;
        }
        let messages = &self.messages;
        let lens = self.lens;
        let mut visible = std::mem::take(&mut self.visible);
        match lens {
            Lens::Vips => visible.retain(|&i| self.control.is_vip(&messages[i].from)),
            Lens::Screener => visible.retain(|&i| self.screened(&messages[i])),
            Lens::Inbox if screening => visible.retain(|&i| !self.screened(&messages[i])),
            Lens::Inbox => {}
        }
        self.visible = visible;
    }

    /// The tab new mail like `m` shows under, as far as announcing it
    /// goes; looked at closely (a code is Primary, which a quick look
    /// can't tell).
    pub(super) fn announced_tab(&self, m: &Mail) -> Category {
        let sorter = &self.sorting.sorter;
        let prefs = &sorter.prefs;
        if !prefs.sorting() {
            return Category::Primary;
        }
        let ai = sorter.model();
        let content = sorter
            .content(m)
            .filter(|c| c.complete)
            .unwrap_or_else(|| crate::manage::categories::Content::of_with(m, ai.as_deref()));
        prefs.fold(crate::manage::categories::decide(
            m,
            content,
            prefs,
            &sorter.known,
        ))
    }

    /// Whether new mail `m` is announced: to an account the workspace on
    /// lets speak up, not filed away or silenced by a rule, not waiting in
    /// the Screener, and as the notification settings and quiet hours say.
    pub(super) fn announces(&self, m: &Mail) -> bool {
        if !self.workspace_announces(&m.account) {
            return false;
        }
        let category = |m: &Mail| self.announced_tab(m);
        let facts = crate::manage::organize::Facts {
            category: Some(&category),
            header: None,
        };
        !self.org.silenced_with(m, &facts)
            && self.control.announces(
                m,
                self.screened(m),
                || self.announced_tab(m),
                &chrono::Local::now(),
            )
    }

    /// The mark beside a VIP's name in the list.
    pub(super) fn vip_mark(&self, m: &Mail, size: f32) -> Option<AnyElement> {
        if m.pending || !self.control.is_vip(&m.from) {
            return None;
        }
        let c = self.colors();
        let selector = format!("vip-mark-{}", m.id);
        Some(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex_shrink_0()
                .role(Role::Image)
                .aria_label(t!("inbox-control-vip"))
                .child(icon_el("vip", size, c.accent))
                .tooltip(|w, cx| Tooltip::new(t!("inbox-control-vip")).build(w, cx))
                .into_any_element(),
        )
    }

    /// The message menu's entries: VIP, and the Screener's decisions for
    /// mail waiting in it.
    pub(super) fn control_entries(&self, m: &Mail) -> Vec<Entry> {
        if m.from.trim().is_empty() || self.folder == Folder::Drafts {
            return vec![];
        }
        let ids = vec![m.id.clone()];
        let mut entries = vec![
            Entry::Separator,
            entry(
                if self.control.is_vip(&m.from) {
                    t!("inbox-control-remove-from-vips")
                } else {
                    t!("inbox-control-add-to-vips")
                },
                "vip",
                Action::Control(ControlAction::ToggleVip(m.id.clone())),
            ),
        ];
        if self.screened(m) {
            entries.push(entry(
                t!("inbox-control-approve-sender"),
                "check",
                Action::Control(ControlAction::Approve(ids.clone())),
            ));
            if self.sorting.prefs().sorting() {
                entries.push(entry(
                    t!("inbox-control-route-to-category"),
                    "inboxes",
                    Action::Control(ControlAction::RouteMenu(ids.clone())),
                ));
            }
            entries.push(danger(
                t!("inbox-control-block-sender"),
                "ban",
                Action::Control(ControlAction::Block(ids)),
            ));
        }
        entries
    }

    /// The senders of these messages (none: the Screener's current one),
    /// lowercase, each once.
    fn control_senders(&self, ids: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let chosen: HashSet<&String> = ids.iter().collect();
        let picked: Vec<&Mail> = if ids.is_empty() {
            self.screener_current().into_iter().collect()
        } else {
            self.messages
                .iter()
                .filter(|m| chosen.contains(&m.id))
                .collect()
        };
        picked
            .into_iter()
            .map(|m| m.from.trim().to_lowercase())
            .filter(|f| !f.is_empty() && seen.insert(f.clone()))
            .collect()
    }

    /// The message whose sender the Screener asks about: the one open, if
    /// it waits there, else the first listed.
    fn screener_current(&self) -> Option<&Mail> {
        let open = self
            .selected
            .as_ref()
            .and_then(|id| self.messages.iter().find(|m| &m.id == id))
            .filter(|m| self.screened(m));
        open.or_else(|| {
            self.visible
                .iter()
                .map(|&i| &self.messages[i])
                .find(|m| self.screened(m))
        })
    }

    /// Opens the inbox (every account's, where the sidebar shows them
    /// together) through `lens`.
    fn open_lens(&mut self, lens: Lens, w: &mut Window, cx: &mut Context<Self>) {
        if lens == Lens::Inbox {
            self.lens = Lens::Inbox;
            self.filter(cx);
            cx.notify();
            return;
        }
        if !self.lens_applies() || self.search_open {
            let account = if self.shows_all_mail() && self.accounts.len() > 1 {
                Some(ALL_MAIL.to_owned())
            } else {
                self.account
                    .clone()
                    .filter(|a| a != ALL_MAIL)
                    .or_else(|| self.first_account())
            };
            if account.is_none() {
                return;
            }
            self.open_mailbox(account, Folder::Inbox.into(), w, cx);
        }
        self.lens = lens;
        self.selection.clear();
        self.filter(cx);
        self.scroll_list_to_top();
        cx.notify();
    }

    pub(super) fn control_action(
        &mut self,
        action: ControlAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = chrono::Utc::now().timestamp();
        match action {
            ControlAction::Lens(lens) => self.open_lens(lens, w, cx),
            ControlAction::ToggleVip(id) => {
                let Some(from) = self
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.from.clone())
                else {
                    return;
                };
                match self.control.toggle_vip(&from) {
                    Some(true) => {
                        self.notice(t!("inbox-control-now-a-vip", from = from), false, cx)
                    }
                    Some(false) => {
                        self.notice(t!("inbox-control-no-longer-a-vip", from = from), false, cx)
                    }
                    None => return,
                }
                self.save_control(cx);
            }
            ControlAction::AddVip => {
                let text = Self::value(&self.control_ui.vip, cx);
                if self.control.is_vip(&text) && !text.trim().starts_with('@') {
                    Self::set(&self.control_ui.vip, "", w, cx);
                    return;
                }
                match self.control.toggle_vip(&text) {
                    Some(_) => {
                        Self::set(&self.control_ui.vip, "", w, cx);
                        self.save_control(cx);
                    }
                    None => self.notice(t!("organize-type-an-address-or-and-a"), true, cx),
                }
            }
            ControlAction::RemoveVip(address) => {
                self.control.vips.retain(|v| *v != address);
                self.save_control(cx);
            }
            ControlAction::Approve(ids) => {
                let senders = self.control_senders(&ids);
                if senders.is_empty() {
                    return;
                }
                for s in &senders {
                    self.control.approve(s);
                }
                let text = match senders.as_slice() {
                    [one] => t!("inbox-control-approved", from = one),
                    _ => t!("inbox-control-approved-several", count = senders.len()),
                };
                self.notice(text, false, cx);
                self.save_control(cx);
            }
            ControlAction::Block(ids) => {
                let senders = self.control_senders(&ids);
                if senders.is_empty() {
                    return;
                }
                for s in &senders {
                    if !self.org.is_blocked(s) {
                        self.org.blocked.push(s.clone());
                    }
                }
                self.org.save(&self.backend.store);
                let gone: Vec<String> = self
                    .messages
                    .iter()
                    .filter(|m| {
                        senders.contains(&m.from.trim().to_lowercase()) && m.folder != Folder::Trash
                    })
                    .map(|m| m.id.clone())
                    .collect();
                let text = match senders.as_slice() {
                    [one] => t!("organize-blocked-their-mail-goes-to-trash", from = one),
                    _ => t!("inbox-control-blocked-several", count = senders.len()),
                };
                self.notice(text, false, cx);
                if !gone.is_empty() {
                    self.apply(gone, Change::Move(Folder::Trash), cx);
                }
                self.filter(cx);
                cx.notify();
            }
            ControlAction::RouteMenu(ids) => {
                let entries: Vec<Entry> = self
                    .sorting
                    .prefs()
                    .tabs()
                    .into_iter()
                    .map(|c| {
                        entry(
                            t!("categories-move-to", category = c.label()),
                            c.icon(),
                            Action::Control(ControlAction::Route(ids.clone(), c)),
                        )
                    })
                    .collect();
                if !entries.is_empty() {
                    self.open_menu_here(entries, w, cx);
                }
            }
            ControlAction::Route(ids, category) => {
                let ids = if ids.is_empty() {
                    self.screener_current()
                        .map(|m| vec![m.id.clone()])
                        .unwrap_or_default()
                } else {
                    ids
                };
                let senders = self.control_senders(&ids);
                if senders.is_empty() {
                    return;
                }
                for s in &senders {
                    self.control.approve(s);
                }
                self.control.save(&self.backend.store);
                // Sorted there from now on, as moving them to a tab does.
                self.cat_action(categories::CatAction::MoveSenders(ids, category), w, cx);
                self.filter(cx);
                cx.notify();
            }
            ControlAction::ToggleScreener => {
                let on = !self.control.screener.enabled;
                self.control.set_screening(on, now);
                if on {
                    self.load_known(cx);
                } else if self.lens == Lens::Screener {
                    self.lens = Lens::Inbox;
                }
                self.save_control(cx);
            }
            ControlAction::ForgetApproval(address) => {
                self.control.forget_approval(&address);
                self.save_control(cx);
            }
            ControlAction::SetMode(mode) => {
                self.control.notifications.mode = mode;
                self.save_control(cx);
            }
            ControlAction::ToggleSilent(category) => {
                self.control.toggle_silent(category);
                self.save_control(cx);
            }
            ControlAction::ToggleQuiet => {
                let q = &mut self.control.notifications.quiet;
                q.enabled = !q.enabled;
                if q.enabled && q.windows.is_empty() {
                    q.windows.push(QuietWindow::default());
                }
                self.save_control(cx);
            }
            ControlAction::ToggleVipsBreak => {
                let q = &mut self.control.notifications.quiet;
                q.vips_break_through = !q.vips_break_through;
                self.save_control(cx);
            }
            ControlAction::AddWindow => {
                let start =
                    QuietWindow::parse_clock(&Self::value(&self.control_ui.quiet_start, cx));
                let end = QuietWindow::parse_clock(&Self::value(&self.control_ui.quiet_end, cx));
                let (Some(start), Some(end)) = (start, end) else {
                    self.notice(t!("inbox-control-enter-times-like"), true, cx);
                    return;
                };
                if self.control_ui.new_days == 0 {
                    self.notice(t!("inbox-control-choose-a-day"), true, cx);
                    return;
                }
                let q = &mut self.control.notifications.quiet;
                q.windows.push(QuietWindow {
                    days: self.control_ui.new_days,
                    start,
                    end: end % (24 * 60),
                });
                q.enabled = true;
                Self::set(&self.control_ui.quiet_start, "", w, cx);
                Self::set(&self.control_ui.quiet_end, "", w, cx);
                self.save_control(cx);
            }
            ControlAction::RemoveWindow(i) => {
                let q = &mut self.control.notifications.quiet;
                if i < q.windows.len() {
                    q.windows.remove(i);
                    self.save_control(cx);
                }
            }
            ControlAction::ToggleDay(i, day) => {
                if let Some(window) = self.control.notifications.quiet.windows.get_mut(i)
                    && day < 7
                {
                    window.days ^= 1 << day;
                    self.save_control(cx);
                }
            }
            ControlAction::NewDays(days) => {
                self.control_ui.new_days = days;
                cx.notify();
            }
            ControlAction::Pause(minutes) => {
                let until = if minutes > 0 {
                    now + minutes * 60
                } else {
                    // Until 8 in the morning, here: today's before it, else
                    // tomorrow's.
                    let local = chrono::Local::now();
                    let day = if chrono::Timelike::hour(&local) < 8 {
                        local.date_naive()
                    } else {
                        local.date_naive() + chrono::Days::new(1)
                    };
                    day.and_hms_opt(8, 0, 0)
                        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
                        .map_or(now + 12 * 3600, |t| t.timestamp())
                };
                self.control.notifications.quiet.paused_until = until;
                let at = chrono::DateTime::from_timestamp(until, 0)
                    .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                    .unwrap_or_default();
                self.notice(t!("inbox-control-paused-until", time = at), false, cx);
                self.save_control(cx);
            }
            ControlAction::Resume => {
                self.control.notifications.quiet.paused_until = 0;
                self.save_control(cx);
            }
        }
    }

    /// Links to the inbox's VIPs and Screener under the sidebar's
    /// mailboxes, while there are VIPs or the Screener is on.
    pub(super) fn control_sidebar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.control.vips.is_empty() && !self.control.screener.enabled {
            return None;
        }
        let c = self.colors();
        let item = |id: &'static str,
                    label: &'static str,
                    icon: &'static str,
                    lens: Lens,
                    count: usize| {
            let active = self.lens == lens && self.lens_applies();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::Link)
                .aria_label(label)
                .h(px(30.))
                .px_2()
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_size(px(13.))
                .when(active, |d| d.bg(rgb(c.soft)))
                .hover(move |s| s.bg(rgb(c.surface)))
                .text_color(rgb(if active { c.text } else { c.muted }))
                .child(div().size(px(12.)))
                .child(icon_el(icon, 15., if active { c.accent } else { c.muted }))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when(count > 0, |d| {
                    d.child(
                        div()
                            .px(px(6.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgb(c.soft))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.accent))
                            .child(fmt_count(count)),
                    )
                })
                .on_click(cx.listener(move |this, _, w, cx| {
                    let lens = if this.lens == lens && this.lens_applies() {
                        Lens::Inbox
                    } else {
                        lens
                    };
                    this.handle(Action::Control(ControlAction::Lens(lens)), w, cx)
                }))
        };
        let mut list = div().flex().flex_col().gap(px(1.)).mt_3().child(
            div()
                .px_2()
                .pb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(c.muted))
                .child(t!("inbox-control-inbox-control")),
        );
        if !self.control.vips.is_empty() {
            list = list.child(item(
                "sidebar-vips",
                t!("inbox-control-vips"),
                "vip",
                Lens::Vips,
                0,
            ));
        }
        if self.control.screener.enabled {
            let waiting = if self.lens_applies() {
                self.screened_waiting
            } else {
                0
            };
            list = list.child(item(
                "sidebar-screener",
                t!("inbox-control-screener"),
                "screener",
                Lens::Screener,
                waiting,
            ));
        }
        Some(list.into_any_element())
    }

    /// The bar above the inbox's list: what a lens shows (and the way back),
    /// the Screener's decision about the sender it shows, or how many new
    /// senders wait there.
    pub(super) fn lens_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.lens_applies() || !Self::value(&self.search, cx).trim().is_empty() {
            return None;
        }
        let c = self.colors();
        let act = |a: ControlAction| Action::Control(a);
        let bar = |id: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .mt_2()
                .px_3()
                .py_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .text_size(px(12.))
        };
        let small = |el: Stateful<Div>| el.h(px(28.)).px_2().text_size(px(12.));
        let back = small(self.button(
            "lens-back",
            t!("inbox-control-back-to-inbox"),
            "",
            act(ControlAction::Lens(Lens::Inbox)),
            false,
            cx,
        ));
        match self.lens {
            Lens::Inbox if self.screened_waiting > 0 => Some(
                bar("screener-waiting")
                    .child(icon_el("screener", 14., c.accent))
                    .child(div().flex_1().min_w_0().text_color(rgb(c.text)).child(t!(
                        "inbox-control-waiting-in-screener",
                        count = self.screened_waiting
                    )))
                    .child(small(self.button(
                        "screener-review",
                        t!("inbox-control-review"),
                        "",
                        act(ControlAction::Lens(Lens::Screener)),
                        true,
                        cx,
                    )))
                    .into_any_element(),
            ),
            Lens::Inbox => None,
            Lens::Vips => Some(
                bar("vips-bar")
                    .child(icon_el("vip", 14., c.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(c.text))
                            .child(t!("inbox-control-vips-about")),
                    )
                    .child(back)
                    .into_any_element(),
            ),
            Lens::Screener => {
                let mut b = bar("screener-bar").child(icon_el("screener", 14., c.accent));
                match self.screener_current() {
                    None => {
                        b = b.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(rgb(c.muted))
                                .child(t!("inbox-control-screener-empty")),
                        );
                    }
                    Some(m) => {
                        let from = m.from.trim().to_lowercase();
                        let count = self
                            .visible
                            .iter()
                            .filter(|&&i| self.messages[i].from.trim().eq_ignore_ascii_case(&from))
                            .count();
                        let name = if m.sender.trim().is_empty()
                            || m.sender.trim().eq_ignore_ascii_case(&from)
                        {
                            from.clone()
                        } else {
                            format!("{} <{from}>", m.sender.trim())
                        };
                        b = b
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(160.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(c.text))
                                            .child(name),
                                    )
                                    .child(div().text_color(rgb(c.muted)).child(t!(
                                        "inbox-control-first-time-sender",
                                        count = count
                                    ))),
                            )
                            .child(small(self.button(
                                "screener-approve",
                                t!("inbox-control-approve"),
                                "check",
                                act(ControlAction::Approve(vec![m.id.clone()])),
                                true,
                                cx,
                            )))
                            .when(self.sorting.prefs().sorting(), |d| {
                                d.child(small(self.button(
                                    "screener-route",
                                    t!("inbox-control-category"),
                                    "inboxes",
                                    act(ControlAction::RouteMenu(vec![m.id.clone()])),
                                    false,
                                    cx,
                                )))
                            })
                            .child(small(self.button(
                                "screener-block",
                                t!("inbox-control-block"),
                                "ban",
                                act(ControlAction::Block(vec![m.id.clone()])),
                                false,
                                cx,
                            )));
                    }
                }
                Some(b.child(back).into_any_element())
            }
        }
    }

    /// Settings → Inbox Control.
    pub(super) fn inbox_control_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let control = &self.control;
        let act = |a: ControlAction| Action::Control(a);
        let small = |t: String| div().text_size(px(12.)).text_color(rgb(c.muted)).child(t);
        let row = |id: String, text: String, trailing: AnyElement| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id)
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(text),
                )
                .child(trailing)
        };

        // VIPs.
        let mut vips = Rows::new().row(small(t!("inbox-control-vips-caption").into()));
        for (i, v) in control.vips.iter().enumerate() {
            vips = vips.row(row(
                format!("vip-row-{i}"),
                v.clone(),
                self.button(
                    format!("remove-vip-{i}"),
                    t!("inbox-control-remove"),
                    "",
                    act(ControlAction::RemoveVip(v.clone())),
                    false,
                    cx,
                )
                .into_any_element(),
            ));
        }
        vips = vips.row(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .child(field(Input::new(&self.control_ui.vip), c, false)),
                )
                .child(self.button(
                    "add-vip",
                    t!("inbox-control-add-vip"),
                    "vip",
                    act(ControlAction::AddVip),
                    true,
                    cx,
                )),
        );

        // The Screener.
        let mut screener = Rows::new().row(self.switch_setting(
            t!("inbox-control-screen-new-senders"),
            t!("inbox-control-screen-new-senders-caption"),
            "screener-on".into(),
            control.screener.enabled,
            act(ControlAction::ToggleScreener),
            cx,
        ));
        if control.screener.enabled {
            if control.screener.approved.is_empty() {
                screener = screener.row(small(t!("inbox-control-none-approved").into()));
            }
            for (i, a) in control.screener.approved.iter().enumerate() {
                screener = screener.row(row(
                    format!("approved-row-{i}"),
                    t!("inbox-control-approved-sender", from = a),
                    self.button(
                        format!("forget-approved-{i}"),
                        t!("inbox-control-forget"),
                        "",
                        act(ControlAction::ForgetApproval(a.clone())),
                        false,
                        cx,
                    )
                    .into_any_element(),
                ));
            }
        }

        // Notifications.
        let modes = self
            .segmented(
                "notify-mode",
                NotifyMode::ALL
                    .iter()
                    .map(|m| {
                        (
                            m.label().to_owned(),
                            act(ControlAction::SetMode(*m)),
                            control.notifications.mode == *m,
                        )
                    })
                    .collect(),
                cx,
            )
            .max_w(px(460.));
        let mut notifications = Rows::new().row(self.setting(
            t!("inbox-control-alert-for"),
            t!("inbox-control-alert-for-caption"),
            modes,
        ));
        if control.notifications.mode == NotifyMode::Everything && self.sorting.prefs().sorting() {
            for category in self.sorting.prefs().tabs() {
                notifications = notifications.row(self.switch_setting(
                    &t!("inbox-control-alert-category", category = category.label()),
                    "",
                    format!("notify-category-{}", category.key()),
                    !control.notifications.silent.contains(&category),
                    act(ControlAction::ToggleSilent(category)),
                    cx,
                ));
            }
        }

        // Quiet hours.
        let quiet = &control.notifications.quiet;
        let mut hours = Rows::new().row(self.switch_setting(
            t!("inbox-control-quiet-hours"),
            t!("inbox-control-quiet-hours-caption"),
            "quiet-on".into(),
            quiet.enabled,
            act(ControlAction::ToggleQuiet),
            cx,
        ));
        if quiet.enabled {
            let names = day_names();
            for (i, window) in quiet.windows.iter().enumerate() {
                let days = div().flex().gap_1().children((0..7u32).map(|d| {
                    let on = window.on(d);
                    let selector = format!("quiet-{i}-day-{d}");
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .role(Role::CheckBox)
                        .aria_label(names[d as usize])
                        .aria_toggled(if on { Toggled::True } else { Toggled::False })
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .bg(rgb(if on { c.accent } else { c.surface }))
                        .text_color(rgb(if on { c.on_accent } else { c.muted }))
                        .child(names[d as usize].chars().next().unwrap_or(' ').to_string())
                        .tooltip(move |w, cx| Tooltip::new(names[d as usize]).build(w, cx))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.handle(Action::Control(ControlAction::ToggleDay(i, d)), w, cx)
                        }))
                }));
                let selector = format!("quiet-row-{i}");
                hours = hours.row(
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.))
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.)).child(t!(
                                    "inbox-control-window",
                                    start = QuietWindow::clock(window.start),
                                    end = QuietWindow::clock(window.end)
                                )))
                                .child(small(days_text(window.days))),
                        )
                        .child(days)
                        .child(self.button(
                            format!("remove-quiet-{i}"),
                            t!("inbox-control-remove"),
                            "",
                            act(ControlAction::RemoveWindow(i)),
                            false,
                            cx,
                        )),
                );
            }
            let presets = self
                .segmented(
                    "quiet-new-days",
                    [
                        (t!("inbox-control-every-day"), EVERY_DAY),
                        (t!("inbox-control-weekdays"), WEEKDAYS),
                        (t!("inbox-control-weekends"), WEEKEND),
                    ]
                    .into_iter()
                    .map(|(label, days)| {
                        (
                            label.to_owned(),
                            act(ControlAction::NewDays(days)),
                            self.control_ui.new_days == days,
                        )
                    })
                    .collect(),
                    cx,
                )
                .max_w(px(360.));
            hours = hours.row(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(small(t!("inbox-control-add-quiet-time").into()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .items_center()
                            .child(div().w(px(110.)).child(field(
                                Input::new(&self.control_ui.quiet_start),
                                c,
                                false,
                            )))
                            .child(small("–".into()))
                            .child(div().w(px(110.)).child(field(
                                Input::new(&self.control_ui.quiet_end),
                                c,
                                false,
                            )))
                            .child(presets)
                            .child(self.button(
                                "add-quiet",
                                t!("inbox-control-add"),
                                "plus",
                                act(ControlAction::AddWindow),
                                true,
                                cx,
                            )),
                    ),
            );
            hours = hours.row(self.switch_setting(
                t!("inbox-control-vips-break-through"),
                t!("inbox-control-vips-break-through-caption"),
                "quiet-vips".into(),
                quiet.vips_break_through,
                act(ControlAction::ToggleVipsBreak),
                cx,
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let pause = if quiet.paused(now) {
            let at = chrono::DateTime::from_timestamp(quiet.paused_until, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default();
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .child(t!("inbox-control-paused-until", time = at)),
                )
                .child(self.button(
                    "quiet-resume",
                    t!("inbox-control-resume"),
                    "",
                    act(ControlAction::Resume),
                    true,
                    cx,
                ))
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .child(t!("inbox-control-pause-notifications")),
                )
                .child(self.button(
                    "quiet-pause-hour",
                    t!("inbox-control-for-an-hour"),
                    "",
                    act(ControlAction::Pause(60)),
                    false,
                    cx,
                ))
                .child(self.button(
                    "quiet-pause-morning",
                    t!("inbox-control-until-tomorrow"),
                    "",
                    act(ControlAction::Pause(0)),
                    false,
                    cx,
                ))
        };
        hours = hours.row(pause);

        self.settings_column()
            .child(self.settings_group(t!("inbox-control-vips"), vips))
            .child(self.settings_group(t!("inbox-control-screener"), screener))
            .child(self.settings_group(t!("inbox-control-notifications"), notifications))
            .child(self.settings_group(t!("inbox-control-quiet-hours"), hours))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn days_in_words() {
        assert_eq!(days_text(EVERY_DAY), "Every day");
        assert_eq!(days_text(WEEKEND), "Weekends");
        assert_eq!(days_text(0b101), "Mon Wed");
    }

    #[test]
    fn a_new_control_is_quiet_by_default() {
        let c = crate::manage::inbox_control::InboxControl::default();
        assert!(!c.screener.enabled && c.vips.is_empty() && !c.notifications.quiet.enabled);
    }
}
