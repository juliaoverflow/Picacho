//! Workspaces in the window (see `manage::workspaces`): the switcher at the
//! top of the sidebar (the rail's top button, the top of phones' Mailboxes),
//! what the workspace on hides (the sidebar's other accounts, All Accounts'
//! share of them, other calendars' events, tasks and reminders, other
//! address books' contacts, other accounts' notifications), the account new
//! messages are written from, switching by schedule or from the tray, and
//! Settings → Workspaces: a list of workspaces, each opening a page of its
//! own (`Page::Workspace`), the same on a phone, where it's pushed like any
//! other page.
use super::inbox_control::{day_names, days_text};
use super::*;
use crate::manage::inbox_control::{EVERY_DAY, QuietWindow, WEEKDAYS, WEEKEND};
use crate::manage::workspaces::{Alerts, COLORS, Workspace};
use crate::sync::ical::Event;
use std::borrow::Cow;

/// The colors workspaces are told apart by, legible on light and dark.
const PALETTE: [u32; COLORS as usize] = [
    0x4f7cff, 0x18a865, 0xe0951a, 0xe5484d, 0x8e5cf6, 0x1497ab, 0xd94f93, 0x6b7a90,
];

/// What a new workspace starts as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Template {
    /// Every account, nothing else set.
    Blank,
    /// The account open, written from, on weekdays from 9 to 5.
    Work,
    /// Every account but the one open.
    Personal,
    /// The account open, with notifications off.
    Focus,
}
impl Template {
    const ALL: [Self; 4] = [Self::Work, Self::Personal, Self::Focus, Self::Blank];
    fn label(self) -> &'static str {
        match self {
            Self::Blank => t!("workspaces-template-blank"),
            Self::Work => t!("workspaces-template-work"),
            Self::Personal => t!("workspaces-template-personal"),
            Self::Focus => t!("workspaces-template-focus"),
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Blank => "plus",
            Self::Work => "building-2",
            Self::Personal => "user",
            Self::Focus => "bell-off",
        }
    }
}

/// A schedule set with one tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Preset {
    /// Monday to Friday, 9:00 to 17:00.
    WorkHours,
    /// Every evening, 18:00 to 22:00.
    Evenings,
    /// Saturday and Sunday, all day.
    Weekends,
}
impl Preset {
    const ALL: [Self; 3] = [Self::WorkHours, Self::Evenings, Self::Weekends];
    pub(super) fn window(self) -> QuietWindow {
        match self {
            Self::WorkHours => QuietWindow {
                days: WEEKDAYS,
                start: 9 * 60,
                end: 17 * 60,
            },
            Self::Evenings => QuietWindow {
                days: EVERY_DAY,
                start: 18 * 60,
                end: 22 * 60,
            },
            Self::Weekends => QuietWindow {
                days: WEEKEND,
                start: 0,
                end: 0,
            },
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::WorkHours => t!("workspaces-preset-work-hours"),
            Self::Evenings => t!("workspaces-preset-evenings"),
            Self::Weekends => t!("workspaces-preset-weekends"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum WsAction {
    /// Turns a workspace on (`None`: everything).
    Switch(Option<String>),
    /// The switcher's menu.
    Menu,
    /// The next (or previous) workspace, everything among them.
    Cycle(bool),
    /// A new workspace from a template, turned on and opened.
    New(Template),
    /// Settings → Workspaces, at this workspace's page.
    Open(String),
    /// Settings → Workspaces, the list.
    ShowAll,
    /// Back from a workspace's page to the list.
    Back,
    SetColor(String, u8),
    /// Every account (true), or those chosen.
    EveryAccount(String, bool),
    ToggleAccount(String, String),
    EveryCalendar(String, bool),
    ToggleCalendar(String, String),
    EveryContacts(String, bool),
    ToggleContacts(String, String),
    SetAlerts(String, Alerts),
    SetFrom(String, Option<String>),
    /// Adds a schedule window set with one tap.
    AddPreset(String, Preset),
    /// Shows (or hides) the fields for a schedule of one's own.
    ToggleCustom,
    /// Adds the schedule window typed in.
    AddWindow(String),
    RemoveWindow(String, usize),
    ToggleDay(String, usize, u32),
    /// The days a schedule window of one's own starts on.
    NewDays(u8),
    ToggleSchedules,
    Shift(String, i32),
    /// Asks before deleting a workspace.
    Delete(String),
    ConfirmDelete(String),
    CancelDelete,
}

/// The window's state for workspaces.
pub(super) struct WorkspaceUi {
    /// The name of the workspace whose page is open, saved as it's typed.
    pub(super) name: Entity<InputState>,
    pub(super) start: Entity<InputState>,
    pub(super) end: Entity<InputState>,
    pub(super) new_days: u8,
    /// The fields for a schedule of one's own are shown.
    pub(super) custom: bool,
    /// The workspace asked about before it's deleted.
    pub(super) deleting: Option<String>,
    /// The workspace the window shows, as last applied.
    pub(super) applied: Option<String>,
}
impl WorkspaceUi {
    pub(super) fn new(applied: Option<String>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            name: input(t!("workspaces-name-placeholder"), false, window, cx),
            start: input(t!("inbox-control-starts-placeholder"), false, window, cx),
            end: input(t!("inbox-control-ends-placeholder"), false, window, cx),
            new_days: WEEKDAYS,
            custom: false,
            deleting: None,
            applied,
        }
    }
}

/// The letter a workspace's mark shows: its name's first letter or digit.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "•".into())
}

/// A workspace's mark: its initial on its color.
pub(super) fn badge(w: &Workspace, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size * 0.3))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(PALETTE[(w.color % COLORS) as usize]))
        .text_color(rgb(0xffffff))
        .text_size(px(size * 0.5))
        .font_weight(FontWeight::BOLD)
        .child(initial(&w.name))
}

/// A schedule window in words: its days and times.
fn window_text(w: &QuietWindow) -> String {
    let days = days_text(w.days);
    if w.start == w.end {
        return t!("workspaces-all-day", days = days);
    }
    t!(
        "workspaces-window",
        days = days,
        start = QuietWindow::clock(w.start),
        end = QuietWindow::clock(w.end)
    )
}

impl MailApp {
    // --- What the workspace on shows ---

    /// The workspace the window shows.
    pub(super) fn workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.ws_ui.applied.as_deref()?)
    }
    fn account_ids(&self) -> Vec<String> {
        self.accounts.iter().map(|a| a.id.clone()).collect()
    }
    /// Whether `w` shows account `id`, among the accounts set up.
    fn ws_shows(&self, w: &Workspace, id: &str) -> bool {
        match &w.accounts {
            None => true,
            Some(set) => set.contains(id) || !self.accounts.iter().any(|a| set.contains(&a.id)),
        }
    }
    /// Whether the sidebar lists `id` (an account, or an alias shown as
    /// one, which goes with the account it's delivered to).
    pub(super) fn in_workspace(&self, id: &str) -> bool {
        let Some(w) = self.workspace() else {
            return true;
        };
        if id == ALL_MAIL {
            return true;
        }
        if fwd::is_view(id) {
            return match fwd::find_alias(&self.forwarding, id) {
                Some((f, _)) => f.deliver_to.as_deref().is_none_or(|a| self.ws_shows(w, a)),
                None => true,
            };
        }
        self.ws_shows(w, id)
    }
    /// The accounts the workspace on shows, in their saved order.
    pub(super) fn workspace_accounts(&self) -> Vec<Account> {
        self.accounts
            .iter()
            .filter(|a| self.in_workspace(&a.id))
            .cloned()
            .collect()
    }
    /// The sidebar's arrangement, less what the workspace hides: its other
    /// accounts, and folders left with none.
    pub(super) fn shown_tree(&self) -> Cow<'_, [tree::Node]> {
        if self.workspace().is_none() {
            return Cow::Borrowed(&self.tree);
        }
        fn prune(this: &MailApp, nodes: &[tree::Node]) -> Vec<tree::Node> {
            nodes
                .iter()
                .filter_map(|n| match n {
                    tree::Node::Account { id } => this.in_workspace(id).then(|| n.clone()),
                    tree::Node::Folder { id, name, children } => {
                        let kept = prune(this, children);
                        let had = !tree::account_order(children).is_empty();
                        (kept
                            .iter()
                            .any(|k| !matches!(k, tree::Node::Separator { .. }))
                            || !had)
                            .then(|| tree::Node::Folder {
                                id: id.clone(),
                                name: name.clone(),
                                children: kept,
                            })
                    }
                    tree::Node::Separator { .. } => Some(n.clone()),
                })
                .collect()
        }
        Cow::Owned(prune(self, &self.tree))
    }
    /// Whether the workspace on shows calendar `href`.
    pub(super) fn shows_calendar(&self, href: &str) -> bool {
        self.workspace().is_none_or(|w| w.shows_calendar(href))
    }
    /// Every time an event of the workspace's calendars happens between
    /// `from` and `to`.
    pub(super) fn workspace_agenda(&self, from: i64, to: i64) -> Vec<Event> {
        match self.workspace().filter(|w| w.calendars.is_some()) {
            Some(w) => self.backend.agenda_in(from, to, |c| w.shows_calendar(c)),
            None => self.backend.agenda(from, to),
        }
    }
    /// The workspace's address books' cards.
    pub(super) fn workspace_contacts(&self) -> Vec<crate::sync::carddav::Card> {
        match self.workspace().filter(|w| w.contacts.is_some()) {
            Some(w) => self.backend.contacts_in(|id| w.shows_contacts(id)),
            None => self.backend.contacts(),
        }
    }
    /// Whether new mail to `account` is announced in the workspace on.
    pub(super) fn workspace_announces(&self, account: &str) -> bool {
        self.workspace()
            .is_none_or(|w| w.announces_account(account))
    }
    /// Whether a reminder from `calendar` is shown in the workspace on.
    pub(super) fn workspace_reminds(&self, calendar: Option<&str>) -> bool {
        self.workspace().is_none_or(|w| w.reminds(calendar))
    }
    /// The account a new message is written from where the open view
    /// doesn't say: the workspace's choice, in All Accounts.
    pub(super) fn workspace_from(&self) -> Option<Account> {
        if self.account.as_deref().is_some_and(|a| a != ALL_MAIL) {
            return None;
        }
        let id = self.workspace()?.compose_from.as_ref()?;
        self.accounts.iter().find(|a| &a.id == id).cloned()
    }
    /// Unread inbox mail of the accounts `w` shows (`None`: every one).
    fn ws_unread(&self, w: Option<&Workspace>) -> usize {
        self.accounts
            .iter()
            .filter(|a| w.is_none_or(|w| self.ws_shows(w, &a.id)))
            .map(|a| self.unread_for(&a.id, &Folder::Inbox.into()))
            .sum()
    }
    /// Unread inbox mail the workspace on hides.
    fn unread_elsewhere(&self) -> usize {
        if self.workspace().is_none() {
            return 0;
        }
        self.ws_unread(None)
            .saturating_sub(self.ws_unread(self.workspace()))
    }

    // --- Switching ---

    /// Brings the window up to the workspace on now (a schedule may have
    /// turned it); `announce` says so in a notice when it changed.
    pub(super) fn apply_workspace(&mut self, announce: bool, cx: &mut Context<Self>) {
        let now = self.workspaces.current().map(|w| w.id.clone());
        if now == self.ws_ui.applied {
            return;
        }
        self.ws_ui.applied = now;
        if announce {
            let name = self.workspace().map_or_else(
                || t!("workspaces-everything").to_owned(),
                |w| w.name.clone(),
            );
            self.notice(t!("workspaces-now-showing", name = name), false, cx);
        }
        self.workspace_shown(cx);
    }
    /// Turns workspace `id` on (`None`: everything): from the switcher, the
    /// keyboard or the tray.
    pub(super) fn switch_workspace(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.workspaces.choose(id, chrono::Utc::now().timestamp());
        self.workspaces.save(&self.backend.store);
        self.apply_workspace(true, cx);
        // The tray and widgets show it even when nothing else changed.
        crate::manage::widgets::publish_soon(self.backend.store.clone());
    }
    /// After what the workspace on shows changed: leaves a mailbox it
    /// hides, lists All Accounts' mail and suggestions again, counts it,
    /// and has widgets and the tray count again.
    fn workspace_shown(&mut self, cx: &mut Context<Self>) {
        *self.contacts_cache.borrow_mut() = None;
        self.views_moved(cx);
        self.refresh_counts(cx);
        crate::manage::widgets::publish_soon(self.backend.store.clone());
        cx.notify();
    }
    fn save_workspaces(&mut self, cx: &mut Context<Self>) {
        self.workspaces.save(&self.backend.store);
        // An edit can change what the workspace on shows.
        self.ws_ui.applied = self.workspaces.current().map(|w| w.id.clone());
        self.workspace_shown(cx);
    }
    /// Saves the open workspace page's name as it's typed.
    pub(super) fn rename_workspace(&mut self, cx: &mut Context<Self>) {
        let Page::Workspace(id) = &self.page else {
            return;
        };
        let name = Self::value(&self.ws_ui.name, cx).trim().to_owned();
        let Some(w) = self.workspaces.get_mut(id) else {
            return;
        };
        // An empty name keeps the one it had until another is typed.
        if name.is_empty() || w.name == name {
            return;
        }
        w.name = name;
        self.workspaces.save(&self.backend.store);
        crate::manage::widgets::publish_soon(self.backend.store.clone());
        cx.notify();
    }
    /// Changes workspace `id`; false when there's none.
    fn ws_edit(&mut self, id: &str, change: impl FnOnce(&mut Workspace)) -> bool {
        match self.workspaces.get_mut(id) {
            Some(ws) => {
                change(ws);
                true
            }
            None => false,
        }
    }
    fn calendar_hrefs(&self) -> Vec<String> {
        self.workspace_calendars().into_iter().map(|(c, _)| c.href).collect()
    }
    /// The calendars a workspace can show, and the task lists kept apart
    /// from them (JMAP's, Microsoft To Do's, Google Tasks'): whether each
    /// is a task list.
    fn workspace_calendars(&self) -> Vec<(crate::sync::caldav::Calendar, bool)> {
        let mut out: Vec<(crate::sync::caldav::Calendar, bool)> = self.backend.calendars().into_iter().map(|c| (c, false)).collect();
        for list in self.backend.task_lists() {
            if !out.iter().any(|(c, _)| c.href == list.href) {
                out.push((list, true));
            }
        }
        out
    }
    /// Opens workspace `id`'s page in Settings.
    fn open_workspace(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.workspaces.get(&id).map(|ws| ws.name.clone()) else {
            return;
        };
        self.ws_ui.deleting = None;
        self.ws_ui.custom = false;
        self.open_workspaces_page(Page::Workspace(id), w, cx);
        Self::set(&self.ws_ui.name, &name, w, cx);
    }
    /// Opens Settings (or brings it forward) at `page`: Workspaces or a
    /// workspace's.
    fn open_workspaces_page(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        if self.modal != Some(Modal::Settings) || self.settings_window.is_some() {
            self.handle(Action::Settings, w, cx);
        }
        self.handle(Action::SettingsPage(page), w, cx);
        if self.mobile.on {
            self.mobile.settings_page = true;
            self.mobile_pushed(cx);
        }
    }

    /// The switcher's menu: everything, each workspace (with its unread
    /// inbox mail), and making or editing them.
    fn workspace_menu(&self) -> Vec<Entry> {
        let current = self.ws_ui.applied.clone();
        let label = |name: &str, unread: usize| {
            if unread > 0 {
                format!("{name} · {}", fmt_count(unread))
            } else {
                name.to_owned()
            }
        };
        let mut entries = vec![checked(
            label(t!("workspaces-everything"), self.ws_unread(None)),
            current.is_none(),
            Action::Workspaces(WsAction::Switch(None)),
        )];
        for w in &self.workspaces.list {
            entries.push(checked(
                label(&w.name, self.ws_unread(Some(w))),
                current.as_deref() == Some(w.id.as_str()),
                Action::Workspaces(WsAction::Switch(Some(w.id.clone()))),
            ));
        }
        entries.push(Entry::Separator);
        if let Some(w) = self.workspace() {
            entries.push(entry(
                t!("workspaces-edit-workspace", name = w.name.clone()),
                "settings",
                Action::Workspaces(WsAction::Open(w.id.clone())),
            ));
        }
        entries.push(entry(
            t!("workspaces-all-workspaces"),
            "workspace",
            Action::Workspaces(WsAction::ShowAll),
        ));
        entries.push(entry(
            t!("workspaces-new-workspace"),
            "plus",
            Action::Workspaces(WsAction::New(Template::Blank)),
        ));
        entries
    }

    /// A new workspace from `template`, of the account open where it asks.
    fn new_workspace(&mut self, template: Template) -> String {
        let open = self
            .account
            .clone()
            .filter(|a| !fwd::is_view(a) && self.accounts.iter().any(|x| &x.id == a))
            .filter(|_| self.accounts.len() > 1);
        let name = match template {
            Template::Blank => t!(
                "workspaces-new-name",
                number = self.workspaces.list.len() + 1
            ),
            t => t.label().to_owned(),
        };
        let all = self.account_ids();
        let id = self.workspaces.add(&name);
        let w = self.workspaces.get_mut(&id).expect("just added");
        match template {
            Template::Blank => {}
            Template::Work => {
                if let Some(a) = open {
                    w.accounts = Some([a.clone()].into());
                    w.compose_from = Some(a);
                }
                w.schedule.push(Preset::WorkHours.window());
            }
            Template::Personal => {
                if let Some(a) = open {
                    w.accounts = Some(all.into_iter().filter(|x| *x != a).collect());
                }
            }
            Template::Focus => {
                if let Some(a) = open {
                    w.accounts = Some([a].into());
                }
                w.alerts = Alerts::Nothing;
            }
        }
        id
    }

    pub(super) fn workspace_action(
        &mut self,
        action: WsAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = chrono::Utc::now().timestamp();
        // Accounts removed since are forgotten before any change.
        if !self.accounts.is_empty() {
            let ids = self.account_ids();
            self.workspaces.reconcile(&ids);
        }
        match action {
            WsAction::Switch(id) => self.switch_workspace(id, cx),
            WsAction::Menu => {
                let entries = self.workspace_menu();
                self.open_menu_here(entries, w, cx);
            }
            WsAction::Cycle(forward) => {
                if self.workspaces.list.is_empty() {
                    return;
                }
                let mut order: Vec<Option<String>> = vec![None];
                order.extend(self.workspaces.list.iter().map(|w| Some(w.id.clone())));
                let at = order
                    .iter()
                    .position(|id| *id == self.ws_ui.applied)
                    .unwrap_or(0);
                let n = order.len();
                let next = order[if forward {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                }]
                .clone();
                self.switch_workspace(next, cx);
            }
            WsAction::New(template) => {
                let id = self.new_workspace(template);
                self.workspaces.choose(Some(id.clone()), now);
                self.save_workspaces(cx);
                self.open_workspace(id, w, cx);
            }
            WsAction::Open(id) => self.open_workspace(id, w, cx),
            WsAction::ShowAll => self.open_workspaces_page(Page::Workspaces, w, cx),
            WsAction::Back => {
                self.ws_ui.deleting = None;
                self.handle(Action::SettingsPage(Page::Workspaces), w, cx);
                if self.mobile.on {
                    self.mobile_popped(cx);
                }
            }
            WsAction::SetColor(id, color) => {
                if self.ws_edit(&id, |ws| ws.color = color % COLORS) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::EveryAccount(id, every) => {
                let all = self.account_ids();
                if self.ws_edit(&id, |ws| ws.every_account(every, &all)) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::ToggleAccount(id, account) => {
                let mut changed = false;
                self.ws_edit(&id, |ws| changed = ws.toggle_account(&account));
                if changed {
                    self.save_workspaces(cx);
                } else {
                    self.notice(t!("workspaces-keeps-one-account"), true, cx);
                }
            }
            WsAction::EveryCalendar(id, every) => {
                let all = self.calendar_hrefs();
                if self.ws_edit(&id, |ws| ws.every_calendar(every, &all)) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::ToggleCalendar(id, href) => {
                if self.ws_edit(&id, |ws| ws.toggle_calendar(&href)) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::EveryContacts(id, every) => {
                let all = self.backend.contacts_source_ids();
                if self.ws_edit(&id, |ws| ws.every_contacts(every, &all)) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::ToggleContacts(id, source) => {
                if self.ws_edit(&id, |ws| ws.toggle_contacts(&source)) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::SetAlerts(id, alerts) => {
                if self.ws_edit(&id, |ws| ws.alerts = alerts) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::SetFrom(id, account) => {
                if self.ws_edit(&id, |ws| ws.compose_from = account.clone()) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::AddPreset(id, preset) => {
                let window = preset.window();
                if self.ws_edit(&id, |ws| {
                    if !ws.schedule.contains(&window) {
                        ws.schedule.push(window);
                    }
                }) {
                    // A schedule just made applies from now on.
                    self.workspaces.follow_schedule = true;
                    self.save_workspaces(cx);
                }
            }
            WsAction::ToggleCustom => {
                self.ws_ui.custom = !self.ws_ui.custom;
                cx.notify();
            }
            WsAction::AddWindow(id) => {
                let start = QuietWindow::parse_clock(&Self::value(&self.ws_ui.start, cx));
                let end = QuietWindow::parse_clock(&Self::value(&self.ws_ui.end, cx));
                let (Some(start), Some(end)) = (start, end) else {
                    self.notice(t!("inbox-control-enter-times-like"), true, cx);
                    return;
                };
                if self.ws_ui.new_days == 0 {
                    self.notice(t!("inbox-control-choose-a-day"), true, cx);
                    return;
                }
                let window = QuietWindow {
                    days: self.ws_ui.new_days,
                    start,
                    end: end % (24 * 60),
                };
                if self.ws_edit(&id, |ws| ws.schedule.push(window)) {
                    self.workspaces.follow_schedule = true;
                    self.ws_ui.custom = false;
                    Self::set(&self.ws_ui.start, "", w, cx);
                    Self::set(&self.ws_ui.end, "", w, cx);
                    self.save_workspaces(cx);
                }
            }
            WsAction::RemoveWindow(id, i) => {
                let mut removed = false;
                self.ws_edit(&id, |ws| {
                    if i < ws.schedule.len() {
                        ws.schedule.remove(i);
                        removed = true;
                    }
                });
                if removed {
                    self.save_workspaces(cx);
                }
            }
            WsAction::ToggleDay(id, i, day) => {
                let mut changed = false;
                self.ws_edit(&id, |ws| {
                    if let Some(window) = ws.schedule.get_mut(i)
                        && day < 7
                    {
                        window.days ^= 1 << day;
                        changed = true;
                    }
                });
                if changed {
                    self.save_workspaces(cx);
                }
            }
            WsAction::NewDays(days) => {
                self.ws_ui.new_days = days;
                cx.notify();
            }
            WsAction::ToggleSchedules => {
                self.workspaces.follow_schedule = !self.workspaces.follow_schedule;
                // What shows now holds, whichever way.
                self.workspaces.choose(self.ws_ui.applied.clone(), now);
                self.save_workspaces(cx);
            }
            WsAction::Shift(id, by) => {
                if self.workspaces.shift(&id, by) {
                    self.save_workspaces(cx);
                }
            }
            WsAction::Delete(id) => {
                self.ws_ui.deleting = Some(id);
                cx.notify();
            }
            WsAction::CancelDelete => {
                self.ws_ui.deleting = None;
                cx.notify();
            }
            WsAction::ConfirmDelete(id) => {
                let name = self
                    .workspaces
                    .get(&id)
                    .map(|ws| ws.name.clone())
                    .unwrap_or_default();
                self.workspaces.remove(&id);
                self.ws_ui.deleting = None;
                self.notice(t!("workspaces-deleted", name = name), false, cx);
                self.save_workspaces(cx);
                if matches!(&self.page, Page::Workspace(open) if *open == id) {
                    self.handle(Action::Workspaces(WsAction::Back), w, cx);
                }
            }
        }
    }

    // --- The sidebar ---

    /// The expanded sidebar's switcher, once there's a workspace: the one
    /// on (or Everything), and how much unread mail it hides.
    pub(super) fn workspace_switcher(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.workspaces.list.is_empty() {
            return None;
        }
        let c = self.colors();
        let (name, mark) = match self.workspace() {
            Some(w) => (w.name.clone(), badge(w, 20.).into_any_element()),
            None => (
                t!("workspaces-everything").to_owned(),
                div()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon_el("workspace", 15., c.muted))
                    .into_any_element(),
            ),
        };
        let hidden = self.unread_elsewhere();
        let label = t!("workspaces-switch-workspace");
        Some(
            div()
                .id("workspace-switcher")
                .debug_selector(|| "workspace-switcher".into())
                .role(Role::Button)
                .aria_label(format!("{label}: {name}"))
                .mb_1()
                .h(px(34.))
                .px_2()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.surface)))
                .child(mark)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(c.text))
                        .child(name),
                )
                .when(hidden > 0, |d| {
                    let tip = t!("workspaces-unread-elsewhere", count = hidden);
                    d.child(
                        div()
                            .id("workspace-elsewhere")
                            .px(px(6.))
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .bg(rgb(c.soft))
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .child(fmt_count(hidden))
                            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)),
                    )
                })
                .child(icon_el("chevrons-up-down", 14., c.muted))
                .tooltip(move |w, cx| {
                    Tooltip::new(shortcuts::tip(label, "next-workspace", cx)).build(w, cx)
                })
                .on_click(cx.listener(|this, _, w, cx| {
                    this.handle(Action::Workspaces(WsAction::Menu), w, cx)
                }))
                .into_any_element(),
        )
    }

    /// The rail's switcher: the workspace's mark.
    pub(super) fn workspace_rail(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.workspaces.list.is_empty() {
            return None;
        }
        let c = self.colors();
        let (name, mark) = match self.workspace() {
            Some(w) => (w.name.clone(), badge(w, 22.).into_any_element()),
            None => (
                t!("workspaces-everything").to_owned(),
                icon_el("workspace", 16., c.muted).into_any_element(),
            ),
        };
        Some(
            self.rail_button(
                "rail-workspace".into(),
                format!("{}: {name}", t!("workspaces-switch-workspace")),
                mark,
                false,
                Action::Workspaces(WsAction::Menu),
                cx,
            )
            .role(Role::Button)
            .into_any_element(),
        )
    }

    // --- Settings → Workspaces ---

    /// What a workspace shows, in a line: its accounts, calendars and
    /// schedule.
    fn workspace_summary(&self, ws: &Workspace) -> String {
        let accounts = match &ws.accounts {
            None => t!("workspaces-every-account").to_owned(),
            Some(set) => {
                let names: Vec<&str> = self
                    .accounts
                    .iter()
                    .filter(|a| set.contains(&a.id))
                    .map(|a| a.label())
                    .collect();
                match names.as_slice() {
                    [] => t!("workspaces-every-account").to_owned(),
                    [one] => (*one).to_owned(),
                    [one, two] => format!("{one}, {two}"),
                    _ => t!("workspaces-accounts", count = names.len()),
                }
            }
        };
        let mut parts = vec![accounts];
        if let Some(set) = &ws.calendars {
            parts.push(t!("workspaces-calendars", count = set.len()));
        }
        match ws.alerts {
            Alerts::Workspace => {}
            Alerts::Everything => parts.push(t!("workspaces-all-notifications").to_owned()),
            Alerts::Nothing => parts.push(t!("workspaces-no-notifications").to_owned()),
        }
        if let Some(first) = ws.schedule.first() {
            parts.push(window_text(first));
        }
        parts.join(" · ")
    }

    /// A row of the list: `lead`, a title and caption, then `actions`
    /// (buttons of their own), and where `action` is given, a chevron and
    /// the whole row opening it.
    #[allow(clippy::too_many_arguments)]
    fn ws_row(
        &self,
        id: String,
        lead: AnyElement,
        title: String,
        caption: String,
        actions: Vec<AnyElement>,
        action: Option<Action>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let selector = id.clone();
        let opens = action.is_some();
        // Their own clicks don't open the row too.
        let actions = div()
            .id(SharedString::from(format!("{id}-actions")))
            .flex()
            .items_center()
            .gap_2()
            .children(actions)
            .on_click(|_, _, cx| cx.stop_propagation());
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .when(opens, |d| d.role(Role::Button).aria_label(title.clone()))
            .min_h(px(if self.mobile.on { mobile::TOUCH } else { 40. }))
            .flex()
            .items_center()
            .gap_3()
            .when(opens, |d| d.cursor_pointer())
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if self.mobile.on { 16. } else { 13. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.text))
                            .child(title),
                    )
                    .when(!caption.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(caption),
                        )
                    }),
            )
            .child(actions)
            .when(opens, |d| d.child(icon_el("chevron-right", 16., c.muted)))
            .when_some(action, |d, action| {
                d.on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
            })
    }

    /// A small "On" mark.
    fn on_pill(&self) -> AnyElement {
        let c = self.colors();
        div()
            .flex_shrink_0()
            .px(px(8.))
            .py(px(2.))
            .rounded_full()
            .bg(rgb(c.soft))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(c.accent))
            .child(t!("workspaces-on"))
            .into_any_element()
    }

    /// A choice in a list, with a check mark when chosen.
    #[allow(clippy::too_many_arguments)]
    fn check_row(
        &self,
        id: String,
        lead: AnyElement,
        title: String,
        caption: String,
        on: bool,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let selector = id.clone();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector)
            .role(Role::CheckBox)
            .aria_label(title.clone())
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .min_h(px(if self.mobile.on { mobile::TOUCH } else { 34. }))
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if self.mobile.on { 15. } else { 13. }))
                            .text_color(rgb(if on { c.text } else { c.muted }))
                            .child(title),
                    )
                    .when(!caption.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(caption),
                        )
                    }),
            )
            .child(
                div()
                    .size(px(20.))
                    .flex_shrink_0()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(on, |d| d.bg(rgb(c.accent)))
                    .when(!on, |d| d.border_2().border_color(rgb(c.border)))
                    .when(on, |d| d.child(icon_el("check", 13., c.on_accent))),
            )
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }

    /// Every one, or only those chosen, as a segmented choice.
    fn every_or_chosen(
        &self,
        id: &str,
        every_label: &str,
        every: bool,
        set: impl Fn(bool) -> Action,
        cx: &Context<Self>,
    ) -> Div {
        self.segmented(
            id,
            vec![
                (every_label.to_owned(), set(true), every),
                (t!("workspaces-only-these").to_owned(), set(false), !every),
            ],
            cx,
        )
        .max_w(px(360.))
    }

    /// Settings → Workspaces: the workspaces (and everything), each opening
    /// its page; templates to start a new one from; switching by schedule.
    pub(super) fn workspaces_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let act = |a: WsAction| Action::Workspaces(a);
        let small = |t: &str| {
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(t.to_owned())
        };
        let count = self.workspaces.list.len();
        let mut list = Rows::new();
        if count > 0 {
            // Everything: no workspace.
            let on = self.ws_ui.applied.is_none();
            list = list.row(
                self.ws_row(
                    "workspace-everything".into(),
                    div()
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon_el("workspace", 18., c.muted))
                        .into_any_element(),
                    t!("workspaces-everything").into(),
                    t!("workspaces-everything-caption").into(),
                    vec![if on {
                        self.on_pill()
                    } else {
                        self.button(
                            "workspace-everything-switch",
                            t!("workspaces-switch"),
                            "",
                            act(WsAction::Switch(None)),
                            false,
                            cx,
                        )
                        .into_any_element()
                    }],
                    None,
                    cx,
                ),
            );
        }
        for (i, ws) in self.workspaces.list.iter().enumerate() {
            let on = self.ws_ui.applied.as_deref() == Some(ws.id.as_str());
            let id = ws.id.clone();
            let mut trailing = vec![if on {
                self.on_pill()
            } else {
                self.button(
                    format!("workspace-switch-{i}"),
                    t!("workspaces-switch"),
                    "",
                    act(WsAction::Switch(Some(id.clone()))),
                    false,
                    cx,
                )
                .into_any_element()
            }];
            // Reordering decides which schedule wins; on a phone, from the
            // workspace's page.
            if count > 1 && !self.mobile.on {
                trailing.push(
                    self.button(
                        format!("workspace-up-{i}"),
                        &format!("@{}", t!("workspaces-move-up")),
                        "arrow-up",
                        act(WsAction::Shift(id.clone(), -1)),
                        false,
                        cx,
                    )
                    .when(i == 0, |d| d.opacity(0.35))
                    .into_any_element(),
                );
                trailing.push(
                    self.button(
                        format!("workspace-down-{i}"),
                        &format!("@{}", t!("workspaces-move-down")),
                        "arrow-down",
                        act(WsAction::Shift(id.clone(), 1)),
                        false,
                        cx,
                    )
                    .when(i + 1 == count, |d| d.opacity(0.35))
                    .into_any_element(),
                );
            }
            list = list.row(self.ws_row(
                format!("workspace-row-{i}"),
                badge(ws, 28.).into_any_element(),
                ws.name.clone(),
                self.workspace_summary(ws),
                trailing,
                Some(act(WsAction::Open(id))),
                cx,
            ));
        }
        if count == 0 {
            list = list.row(small(t!("workspaces-caption")));
        }

        // Starting a new one.
        let templates = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(Template::ALL.into_iter().map(|t| {
                self.button(
                    format!("workspace-template-{t:?}").to_lowercase(),
                    t.label(),
                    t.icon(),
                    act(WsAction::New(t)),
                    count == 0 && t == Template::Work,
                    cx,
                )
            }));
        let new = Rows::new().row(self.setting(
            t!("workspaces-start-from"),
            t!("workspaces-start-from-caption"),
            templates,
        ));

        let mut column = self
            .settings_column()
            .child(self.settings_group(t!("workspaces-workspaces"), list))
            .child(self.settings_group(t!("workspaces-new-workspace"), new));
        if self.workspaces.list.iter().any(|w| !w.schedule.is_empty()) {
            column = column.child(self.settings_group(
                "",
                Rows::new().row(self.switch_setting(
                    t!("workspaces-follow-schedules"),
                    t!("workspaces-follow-schedules-caption"),
                    "workspace-schedules".into(),
                    self.workspaces.follow_schedule,
                    act(WsAction::ToggleSchedules),
                    cx,
                )),
            ));
        }
        column.into_any_element()
    }

    /// A workspace's page: its name and color, what it shows, its
    /// notifications, where new messages come from, its schedule.
    pub(super) fn workspace_page(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let Some(ws) = self.workspaces.get(id) else {
            return self.workspaces_page(cx);
        };
        let c = self.colors();
        let act = |a: WsAction| Action::Workspaces(a);
        let id = ws.id.clone();
        let small = |t: &str| {
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(t.to_owned())
        };
        let on = self.ws_ui.applied.as_deref() == Some(id.as_str());
        let mut column = self.settings_column();

        // Back to the list (a phone's bar has its own back button).
        if !self.mobile.on {
            column = column.child(div().flex().child(self.link(
                "workspace-back",
                &format!("‹ {}", t!("workspaces-workspaces")),
                act(WsAction::Back),
                cx,
            )));
        }

        // Its mark, name, color, and whether it's on.
        let swatches = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children((0..COLORS).map(|n| {
                let selector = format!("workspace-color-{n}");
                let chosen = ws.color % COLORS == n;
                let id = id.clone();
                // A fingertip's room around each on a phone.
                let (area, size) = if self.mobile.on {
                    (mobile::TOUCH, 30.)
                } else {
                    (22., 22.)
                };
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .role(Role::RadioButton)
                    .aria_label(t!("workspaces-color", number = n as usize + 1))
                    .aria_toggled(if chosen {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .size(px(area))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(size))
                            .rounded_full()
                            .bg(rgb(PALETTE[n as usize]))
                            .border_2()
                            .border_color(rgb(if chosen { c.text } else { c.raised })),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Workspaces(WsAction::SetColor(id.clone(), n)), w, cx)
                    }))
            }));
        let header = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(badge(ws, 40.))
                    .child(
                        div()
                            .id("workspace-name")
                            .debug_selector(|| "workspace-name".into())
                            .flex_1()
                            .min_w_0()
                            .child(field(Input::new(&self.ws_ui.name), c, false)),
                    ),
            )
            .child(swatches)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(if on {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.on_pill())
                            .child(small(t!("workspaces-on-now")))
                            .into_any_element()
                    } else {
                        self.button(
                            "workspace-switch-here",
                            t!("workspaces-switch-to-this"),
                            "check",
                            act(WsAction::Switch(Some(id.clone()))),
                            true,
                            cx,
                        )
                        .into_any_element()
                    }),
            );
        column = column.child(self.settings_group("", Rows::new().row(header)));

        // Its accounts.
        let mut accounts = Rows::new().row(self.setting(
            t!("workspaces-accounts-title"),
            t!("workspaces-accounts-caption"),
            self.every_or_chosen(
                "workspace-accounts-mode",
                t!("workspaces-every-account"),
                ws.accounts.is_none(),
                |every| act(WsAction::EveryAccount(id.clone(), every)),
                cx,
            ),
        ));
        if ws.accounts.is_some() {
            for (i, a) in tree::sidebar_ordered(&self.tree, &self.accounts) {
                let caption = if a.label() == a.email {
                    String::new()
                } else {
                    a.email.clone()
                };
                accounts = accounts.row(self.check_row(
                    format!("workspace-account-{}", a.id),
                    self.account_avatar(a, i, 24.),
                    a.label().to_owned(),
                    caption,
                    ws.shows_account(&a.id),
                    act(WsAction::ToggleAccount(id.clone(), a.id.clone())),
                    cx,
                ));
            }
        }
        column = column.child(self.settings_group(t!("workspaces-shows"), accounts));

        // Its calendars.
        let calendars = self.workspace_calendars();
        let mut cal_rows = Rows::new();
        if calendars.is_empty() {
            cal_rows = cal_rows.row(self.setting(
                t!("workspaces-calendars-title"),
                t!("workspaces-no-calendars"),
                div(),
            ));
        } else {
            cal_rows = cal_rows.row(self.setting(
                t!("workspaces-calendars-title"),
                t!("workspaces-calendars-caption"),
                self.every_or_chosen(
                    "workspace-calendars-mode",
                    t!("workspaces-every-calendar"),
                    ws.calendars.is_none(),
                    |every| act(WsAction::EveryCalendar(id.clone(), every)),
                    cx,
                ),
            ));
            if ws.calendars.is_some() {
                for (cal, task_list) in &calendars {
                    let name = if *task_list { t!("workspaces-task-list", name = cal.name) } else { cal.name.clone() };
                    let color = u32::from_str_radix(
                        cal.color.trim_start_matches('#').get(..6).unwrap_or(""),
                        16,
                    )
                    .unwrap_or(c.accent);
                    cal_rows = cal_rows.row(
                        self.check_row(
                            format!("workspace-calendar-{}", cal.href),
                            div()
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(12.)).rounded_full().bg(rgb(color)))
                                .into_any_element(),
                            name,
                            String::new(),
                            ws.shows_calendar(&cal.href),
                            act(WsAction::ToggleCalendar(id.clone(), cal.href.clone())),
                            cx,
                        ),
                    );
                }
            }
        }

        // Its address books, where there's more than one.
        let sources = self.backend.contacts_source_ids();
        if sources.len() > 1 {
            cal_rows = cal_rows.row(self.setting(
                t!("workspaces-contacts-title"),
                t!("workspaces-contacts-caption"),
                self.every_or_chosen(
                    "workspace-contacts-mode",
                    t!("workspaces-every-address-book"),
                    ws.contacts.is_none(),
                    |every| act(WsAction::EveryContacts(id.clone(), every)),
                    cx,
                ),
            ));
            if ws.contacts.is_some() {
                for source in &sources {
                    let label = self.backend.contacts_source_label(source);
                    let label = if label.is_empty() {
                        t!("workspaces-on-this-computer").to_owned()
                    } else {
                        label
                    };
                    cal_rows = cal_rows.row(
                        self.check_row(
                            format!(
                                "workspace-contacts-{}",
                                if source.is_empty() { "first" } else { source }
                            ),
                            div()
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon_el("contact", 16., c.muted))
                                .into_any_element(),
                            label,
                            String::new(),
                            ws.shows_contacts(source),
                            act(WsAction::ToggleContacts(id.clone(), source.clone())),
                            cx,
                        ),
                    );
                }
            }
        }
        column = column.child(self.settings_group("", cal_rows));

        // Notifications, and where new messages come from.
        let alerts = self
            .segmented(
                "workspace-alerts",
                Alerts::ALL
                    .iter()
                    .map(|a| {
                        (
                            a.label().to_owned(),
                            act(WsAction::SetAlerts(id.clone(), *a)),
                            ws.alerts == *a,
                        )
                    })
                    .collect(),
                cx,
            )
            .max_w(px(460.));
        let about = match ws.alerts {
            Alerts::Workspace => t!("workspaces-alerts-workspace-caption"),
            Alerts::Everything => t!("workspaces-alerts-everything-caption"),
            Alerts::Nothing => t!("workspaces-alerts-nothing-caption"),
        };
        let mut behavior =
            Rows::new().row(self.setting(t!("workspaces-notifications"), about, alerts));
        let shown: Vec<(usize, &Account)> = tree::sidebar_ordered(&self.tree, &self.accounts)
            .into_iter()
            .filter(|(_, a)| self.ws_shows(ws, &a.id))
            .collect();
        // Only where All Accounts shows: more than one account.
        if shown.len() > 1 {
            let mut options = vec![(
                t!("workspaces-from-the-open-mailbox").to_owned(),
                act(WsAction::SetFrom(id.clone(), None)),
                ws.compose_from.is_none(),
            )];
            options.extend(shown.iter().map(|(_, a)| {
                (
                    a.label().to_owned(),
                    act(WsAction::SetFrom(id.clone(), Some(a.id.clone()))),
                    ws.compose_from.as_deref() == Some(a.id.as_str()),
                )
            }));
            behavior = behavior.row(self.setting(
                t!("workspaces-write-from"),
                t!("workspaces-write-from-caption"),
                self.segmented("workspace-from", options, cx),
            ));
        }
        column = column.child(self.settings_group(t!("workspaces-behavior"), behavior));

        // Its schedule.
        let names = day_names();
        let mut schedule = Rows::new().row(small(t!("workspaces-schedule-caption")));
        for (i, window) in ws.schedule.iter().enumerate() {
            let days = div().flex().gap_1().children((0..7u32).map(|d| {
                let on = window.on(d);
                let selector = format!("workspace-window-{i}-day-{d}");
                let id = id.clone();
                let size = if self.mobile.on { 32. } else { 24. };
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .role(Role::CheckBox)
                    .aria_label(names[d as usize])
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .size(px(size))
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
                        this.handle(
                            Action::Workspaces(WsAction::ToggleDay(id.clone(), i, d)),
                            w,
                            cx,
                        )
                    }))
            }));
            let selector = format!("workspace-window-{i}");
            schedule = schedule.row(
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
                            .min_w(px(140.))
                            .text_size(px(13.))
                            .child(window_text(window)),
                    )
                    .child(days)
                    .child(self.button(
                        format!("workspace-remove-window-{i}"),
                        t!("inbox-control-remove"),
                        "",
                        act(WsAction::RemoveWindow(id.clone(), i)),
                        false,
                        cx,
                    )),
            );
        }
        let presets = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(Preset::ALL.into_iter().map(|p| {
                self.button(
                    format!("workspace-preset-{p:?}").to_lowercase(),
                    p.label(),
                    "plus",
                    act(WsAction::AddPreset(id.clone(), p)),
                    false,
                    cx,
                )
            }))
            .child(self.button(
                "workspace-custom",
                t!("workspaces-custom-time"),
                if self.ws_ui.custom {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                act(WsAction::ToggleCustom),
                false,
                cx,
            ));
        schedule = schedule.row(presets);
        if self.ws_ui.custom {
            let days = self
                .segmented(
                    "workspace-new-days",
                    [
                        (t!("inbox-control-weekdays"), WEEKDAYS),
                        (t!("inbox-control-weekends"), WEEKEND),
                        (t!("inbox-control-every-day"), EVERY_DAY),
                    ]
                    .into_iter()
                    .map(|(label, days)| {
                        (
                            label.to_owned(),
                            act(WsAction::NewDays(days)),
                            self.ws_ui.new_days == days,
                        )
                    })
                    .collect(),
                    cx,
                )
                .max_w(px(360.));
            schedule = schedule.row(
                div().flex().flex_col().gap_2().child(days).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(110.)).child(field(
                            Input::new(&self.ws_ui.start),
                            c,
                            false,
                        )))
                        .child(small("–"))
                        .child(div().w(px(110.)).child(field(
                            Input::new(&self.ws_ui.end),
                            c,
                            false,
                        )))
                        .child(self.button(
                            "workspace-add-window",
                            t!("inbox-control-add"),
                            "plus",
                            act(WsAction::AddWindow(id.clone())),
                            true,
                            cx,
                        )),
                ),
            );
        }
        column = column.child(self.settings_group(t!("workspaces-schedule"), schedule));

        // Order (which schedule wins, on a phone) and deleting.
        let count = self.workspaces.list.len();
        let place = self
            .workspaces
            .list
            .iter()
            .position(|w| w.id == id)
            .unwrap_or(0);
        let mut end = Rows::new();
        if self.mobile.on && count > 1 {
            end = end.row(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(place > 0, |d| {
                        d.child(self.button(
                            "workspace-earlier",
                            t!("workspaces-move-up"),
                            "arrow-up",
                            act(WsAction::Shift(id.clone(), -1)),
                            false,
                            cx,
                        ))
                    })
                    .when(place + 1 < count, |d| {
                        d.child(self.button(
                            "workspace-later",
                            t!("workspaces-move-down"),
                            "arrow-down",
                            act(WsAction::Shift(id.clone(), 1)),
                            false,
                            cx,
                        ))
                    }),
            );
        }
        let delete = if self.ws_ui.deleting.as_deref() == Some(id.as_str()) {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.))
                        .text_size(px(13.))
                        .child(t!("workspaces-delete-question", name = ws.name.clone())),
                )
                .child(self.button(
                    "workspace-delete-cancel",
                    t!("workspaces-cancel"),
                    "",
                    act(WsAction::CancelDelete),
                    false,
                    cx,
                ))
                .child(
                    self.button(
                        "workspace-delete-confirm",
                        t!("workspaces-delete"),
                        "trash",
                        act(WsAction::ConfirmDelete(id.clone())),
                        false,
                        cx,
                    )
                    .text_color(rgb(c.danger)),
                )
        } else {
            div().flex().child(
                self.button(
                    "workspace-delete",
                    t!("workspaces-delete-workspace"),
                    "trash",
                    act(WsAction::Delete(id.clone())),
                    false,
                    cx,
                )
                .text_color(rgb(c.danger)),
            )
        };
        end = end.row(delete);
        column
            .child(self.settings_group("", end))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn badges_take_the_first_letter_or_digit() {
        assert_eq!(initial("work"), "W");
        assert_eq!(initial("#1 project"), "1");
        assert_eq!(initial("ärzte"), "Ä");
        assert_eq!(initial("  "), "•");
    }

    #[test]
    fn schedule_windows_in_words() {
        assert_eq!(
            window_text(&Preset::WorkHours.window()),
            "Weekdays 09:00–17:00"
        );
        assert_eq!(window_text(&Preset::Weekends.window()), "Weekends, all day");
    }
}
