//! Tabs: several mailboxes or searches kept open at once, in the title bar.
//!
//! The open tab is the window's own state (the open mailbox, its search, the
//! open message); the others are kept as what they showed (`TabPlace`) and
//! opened again as they were when one is chosen. With one tab there are no
//! tabs to show: the title bar only offers the button that opens another.
//! The tabs are saved as they change and opened again at the next launch.
//!
//! A tab keeps a search of the server too: switching back shows what the
//! server found again without asking it, and after a restart the server is
//! asked again. Search All Mail on This Computer's results stay results in
//! their tab, with the mailbox they were opened from to go back to.
use super::views::Place;
use super::*;

/// The preference holding the open tabs (`Saved`), when there are several.
const TABS_PREFERENCE: &str = "open_tabs";

/// How long a tab's open message is waited for once the tab opens again:
/// its mailbox is listed from the cache, usually at once.
const SELECTED_WAIT: Duration = Duration::from_secs(20);

/// How wide the fade is at an end of the row of tabs with more past it.
const TABS_FADE: f32 = 48.;

/// What a tab shows: a mailbox (or Subscriptions, or Waiting For), its
/// search, and the message open in it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct TabPlace {
    /// Empty `account`: the sample mailbox, before any account is added.
    pub place: Place,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// The query was searched for on the server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub server: bool,
    /// What the server found, kept this session only.
    #[serde(skip)]
    pub server_found: Option<FoundOnServer>,
    /// The tab shows Search All Mail on This Computer's results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalPlace>,
    /// The list's filter, the inbox's category and how the inbox is looked
    /// at: kept this session only.
    #[serde(skip)]
    pub view: Option<(Filter, categories::Tab, inbox_control::Lens)>,
}
impl TabPlace {
    fn blank() -> Self {
        Self {
            place: Place {
                account: String::new(),
                mailbox: Folder::Inbox.into(),
                subscriptions: false,
                sender: None,
                waiting: false,
            },
            query: String::new(),
            selected: None,
            server: false,
            server_found: None,
            local: None,
            view: None,
        }
    }
}

/// A server search's results as a tab keeps them: the ids it found, how
/// many matched in all, and the rows (without their bodies, read again
/// when opened), which the mailbox's own list may not have.
#[derive(Clone)]
pub(super) struct FoundOnServer {
    ids: HashSet<String>,
    total: usize,
    rows: Vec<Mail>,
}

/// Search All Mail on This Computer's results: the mailbox they were opened
/// from, which Back returns to.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct LocalPlace {
    #[serde(default)]
    pub back: Option<(String, Mailbox)>,
}

/// What's left to restore of a tab once its mailbox is listed (by the
/// `load_generation` it was opened with).
enum Pending {
    /// The server's results, put back in the list.
    Rows(Vec<Mail>),
    /// The search, asked of the server again.
    Rerun,
}

/// The tabs as saved.
#[derive(serde::Deserialize)]
struct Saved {
    tabs: Vec<TabPlace>,
    active: usize,
}
/// `Saved`, written without copying the tabs.
#[derive(serde::Serialize)]
struct SavedRef<'a> {
    tabs: Vec<&'a TabPlace>,
    active: usize,
}

/// The window's tabs. The open one's entry is brought up to date from the
/// window's state when it is left or saved (see `MailApp::tab_snapshot`).
pub(super) struct Tabs {
    pub list: Vec<TabPlace>,
    pub active: usize,
    pub scroll: ScrollHandle,
    /// What was saved last, so an unchanged set isn't written again.
    saved: std::cell::RefCell<String>,
    pending: Option<(u64, Pending)>,
    /// Which ends of the row had more tabs past them when it was last
    /// drawn (earlier, later), to draw again once the row's laid out
    /// differently (a tab added, the window resized).
    edges: Cell<(bool, bool)>,
    /// The open tab is to be brought into view once the row is laid out:
    /// the first and last all the way, past the fades.
    reveal: Cell<bool>,
}
impl Tabs {
    pub(super) fn new() -> Self {
        Self {
            list: vec![TabPlace::blank()],
            active: 0,
            scroll: ScrollHandle::new(),
            saved: Default::default(),
            pending: None,
            edges: Cell::new((false, false)),
            reveal: Cell::new(false),
        }
    }
    /// Whether there are tabs to show and move between.
    pub(super) fn several(&self) -> bool {
        self.list.len() > 1
    }
}

/// What a tab is dragged as, to put it elsewhere in the row.
#[derive(Clone)]
pub(super) struct TabDrag {
    index: usize,
    label: String,
}

#[derive(Clone)]
pub(super) enum TabAction {
    /// A new tab after the open one: this mailbox of this account (or
    /// view), or the open mailbox afresh.
    New(Option<(String, Mailbox)>),
    Switch(usize),
    Close(usize),
    /// Closes every tab but this one.
    CloseOthers(usize),
    /// The open tab, closed (from the keyboard).
    CloseActive,
    /// The next tab (true) or the previous one, wrapping around.
    Cycle(bool),
    /// Puts the tab at the first index where the second one is.
    Move(usize, usize),
    /// Scrolls the row toward its later tabs (true) or its earlier ones.
    Scroll(bool),
}

impl MailApp {
    pub(super) fn tabs_action(
        &mut self,
        action: TabAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Tabs are a desktop window's; a phone shows one mailbox at a time.
        if self.mobile.on || self.onboarding {
            return;
        }
        match action {
            TabAction::New(target) => self.new_tab(target, w, cx),
            TabAction::Switch(i) => self.switch_tab(i, w, cx),
            TabAction::Close(i) => self.close_tab(i, w, cx),
            TabAction::CloseActive => {
                if self.tabs.several() {
                    self.close_tab(self.tabs.active, w, cx);
                }
            }
            TabAction::CloseOthers(i) => {
                if i >= self.tabs.list.len() {
                    return;
                }
                if i != self.tabs.active {
                    self.switch_tab(i, w, cx);
                }
                let kept = self.tabs.list.swap_remove(self.tabs.active);
                self.tabs.list = vec![kept];
                self.tabs.active = 0;
            }
            TabAction::Cycle(forward) => {
                let n = self.tabs.list.len();
                if n > 1 {
                    let i = if forward {
                        (self.tabs.active + 1) % n
                    } else {
                        (self.tabs.active + n - 1) % n
                    };
                    self.switch_tab(i, w, cx);
                }
            }
            TabAction::Scroll(later) => {
                let scroll = &self.tabs.scroll;
                let (offset, max) = (-scroll.offset().x, scroll.max_offset().x);
                let step = (scroll.bounds().size.width * 0.7).max(px(80.));
                let to = if later { offset + step } else { offset - step };
                let to = to.clamp(px(0.), max.max(px(0.)));
                scroll.set_offset(point(-to, scroll.offset().y));
            }
            TabAction::Move(from, to) => {
                let n = self.tabs.list.len();
                if from >= n || to >= n || from == to {
                    return;
                }
                let active = self.tabs.active;
                let tab = self.tabs.list.remove(from);
                self.tabs.list.insert(to, tab);
                // The open tab stays open wherever it went.
                self.tabs.active = if active == from {
                    to
                } else if from < active && to >= active {
                    active - 1
                } else if from > active && to <= active {
                    active + 1
                } else {
                    active
                };
            }
        }
        self.save_tabs(cx);
        cx.notify();
    }

    /// What the window shows now, as a tab; with `rows`, the rows a
    /// server search found too (to keep the tab, not to name it).
    pub(super) fn tab_snapshot(&self, cx: &App, rows: bool) -> TabPlace {
        let plain = !self.subs_open && !self.waiting_open && self.folder != Folder::Drafts;
        let query = if self.search_open {
            Self::value(&self.search, cx)
        } else {
            String::new()
        };
        let server = self
            .current_server_search(query.trim())
            .filter(|_| !query.trim().is_empty());
        let server_found = server
            .filter(|s| rows && !s.running && s.error.is_none())
            .map(|s| FoundOnServer {
                ids: s.ids.clone(),
                total: s.total,
                rows: self
                    .messages
                    .iter()
                    .filter(|m| s.ids.contains(&m.id))
                    .map(|m| Mail {
                        body: None,
                        html: None,
                        ..m.clone()
                    })
                    .collect(),
            });
        TabPlace {
            place: Place {
                account: self.account.clone().unwrap_or_default(),
                mailbox: self.mailbox(),
                subscriptions: self.subs_open,
                sender: self.subs_open.then(|| self.subs_sender.clone()).flatten(),
                waiting: self.waiting_open,
            },
            server: server.is_some(),
            server_found,
            local: self.local_results.as_ref().map(|r| LocalPlace {
                back: r.back.clone(),
            }),
            query,
            selected: self.selected.clone().filter(|_| plain),
            view: Some((self.filter_by, self.sorting.tab, self.lens)),
        }
    }

    fn new_tab(
        &mut self,
        target: Option<(String, Mailbox)>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.tab_snapshot(cx, true);
        let (account, mailbox) = match target {
            Some((id, mailbox)) => (id, mailbox),
            None => (current.place.account.clone(), current.place.mailbox.clone()),
        };
        let active = self.tabs.active;
        self.tabs.list[active] = current;
        let mut tab = TabPlace::blank();
        tab.place.account = account;
        tab.place.mailbox = mailbox;
        self.tabs.list.insert(active + 1, tab.clone());
        self.tabs.active = active + 1;
        self.open_tab_place(tab, w, cx);
        self.reveal_active_tab();
    }

    fn switch_tab(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.list.len() || i == self.tabs.active {
            return;
        }
        let active = self.tabs.active;
        self.tabs.list[active] = self.tab_snapshot(cx, true);
        self.tabs.active = i;
        self.open_tab_place(self.tabs.list[i].clone(), w, cx);
        self.reveal_active_tab();
    }

    fn close_tab(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.list.len();
        if n < 2 || i >= n {
            return;
        }
        if i != self.tabs.active {
            self.tabs.list.remove(i);
            if i < self.tabs.active {
                self.tabs.active -= 1;
            }
            return;
        }
        // The open tab goes: the one after it opens, else the one before.
        self.tabs.list.remove(i);
        self.tabs.active = i.min(self.tabs.list.len() - 1);
        self.open_tab_place(self.tabs.list[self.tabs.active].clone(), w, cx);
        self.reveal_active_tab();
    }

    /// Once the mailbox of a tab just opened is listed: the server's results
    /// it kept go back in the list, or its server search runs again.
    pub(super) fn finish_tab_restore(&mut self, cx: &mut Context<Self>) {
        let Some((generation, pending)) = self.tabs.pending.take() else {
            return;
        };
        if generation != self.load_generation {
            return;
        }
        match pending {
            Pending::Rows(rows) => {
                let wanted: HashSet<&str> = rows.iter().map(|m| m.id.as_str()).collect();
                let listed: HashSet<String> = self
                    .messages
                    .iter()
                    .filter(|m| wanted.contains(m.id.as_str()))
                    .map(|m| m.id.clone())
                    .collect();
                let before = self.messages.len();
                self.messages
                    .extend(rows.into_iter().filter(|m| !listed.contains(&m.id)));
                if self.messages.len() != before {
                    self.list_revision += 1;
                }
                self.apply_filter(cx);
            }
            Pending::Rerun => self.search_server(cx),
        }
        cx.notify();
    }

    /// Scrolls the row of tabs to the open one.
    fn reveal_active_tab(&self) {
        self.tabs.scroll.scroll_to_item(self.tabs.active);
        self.tabs.reveal.set(true);
    }

    /// After the row of tabs is laid out: the first or last tab, when
    /// open, goes all the way to its end (no fade over it), and the row is
    /// to be drawn again (true) if which ends have more tabs past them
    /// changed.
    fn settle_tab_row(&self) -> bool {
        let scroll = &self.tabs.scroll;
        let max = scroll.max_offset().x;
        let before = scroll.offset().x;
        if self.tabs.reveal.take() {
            let last = self.tabs.list.len().saturating_sub(1);
            if self.tabs.active == 0 {
                scroll.set_offset(point(px(0.), scroll.offset().y));
            } else if self.tabs.active == last {
                scroll.set_offset(point(-max, scroll.offset().y));
            }
        }
        let offset = -scroll.offset().x;
        let edges = (offset > px(0.5), max - offset > px(0.5));
        scroll.offset().x != before || edges != self.tabs.edges.get()
    }

    /// An end of the row of tabs with more past it (`later`: the trailing
    /// end): the tabs fade into the title bar under a round arrow that
    /// scrolls toward them. The fade takes no clicks; the tab under it
    /// still opens.
    fn tab_row_edge(&self, later: bool, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let solid: Hsla = rgb(c.bg).into();
        let clear = Hsla { a: 0., ..solid };
        let (id, label, icon) = if later {
            ("tabs-later", t!("tabs-later-tabs"), "chevron-right")
        } else {
            ("tabs-earlier", t!("tabs-earlier-tabs"), "chevron-left")
        };
        div()
            .debug_selector(move || format!("{id}-fade"))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(TABS_FADE))
            .map(|d| if later { d.right_0() } else { d.left_0() })
            .flex()
            .items_center()
            .map(|d| {
                if later {
                    d.justify_end()
                } else {
                    d.justify_start()
                }
            })
            .bg(linear_gradient(
                if later { 270. } else { 90. },
                linear_color_stop(solid, 0.45),
                linear_color_stop(clear, 1.),
            ))
            .child(window::no_drag(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(Role::Button)
                    .aria_label(label)
                    .size(px(22.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(c.raised))
                    .border_1()
                    .border_color(rgb(c.border))
                    .shadow_md()
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.soft)))
                    .child(icon_el(icon, 13., c.text))
                    .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.handle(Action::Tabs(TabAction::Scroll(later)), w, cx)
                    })),
            ))
    }

    /// Opens what `tab` showed. A mailbox that's gone (its account removed,
    /// its folder deleted) gives way to its account's Inbox, else the first
    /// account's.
    fn open_tab_place(&mut self, tab: TabPlace, w: &mut Window, cx: &mut Context<Self>) {
        self.tabs.pending = None;
        // Search All Mail on This Computer's results: every account's mail,
        // as that search lists it (All Accounts whether or not the sidebar
        // shows it).
        if let Some(local) = tab.local.clone()
            && !tab.query.trim().is_empty()
        {
            let account = if self.accounts.len() > 1 {
                Some(ALL_MAIL.to_owned())
            } else {
                self.accounts.first().map(|a| a.id.clone())
            };
            if let Some(account) = account {
                self.open_mailbox(Some(account), Folder::All.into(), w, cx);
                self.local_results = Some(organize::LocalResults { back: local.back });
                self.restore_tab_view(tab, w, cx);
                return;
            }
        }
        let place = tab.place.clone();
        let demo = place.account.is_empty() && self.accounts.is_empty();
        let exists = demo
            || (!place.account.is_empty() && self.place_exists(&place.account, &place.mailbox));
        if !exists {
            let account = if !place.account.is_empty()
                && self.place_exists(&place.account, &Folder::Inbox.into())
            {
                Some(place.account.clone())
            } else {
                self.first_account()
            };
            self.open_mailbox(account, Folder::Inbox.into(), w, cx);
            return;
        }
        let account = (!place.account.is_empty()).then(|| place.account.clone());
        if place.subscriptions && account.is_some() {
            self.open_subscriptions(place.account, w, cx);
            if let Some(sender) = place.sender {
                self.open_subscription(sender, cx);
            }
            return;
        }
        if place.waiting && account.is_some() {
            self.open_waiting(place.account, w, cx);
            return;
        }
        self.open_mailbox(account, place.mailbox, w, cx);
        self.restore_tab_view(tab, w, cx);
    }

    /// Puts back what a tab showed in its mailbox, just opened: its filter,
    /// its search (and the server's results for it), its open message.
    fn restore_tab_view(&mut self, tab: TabPlace, w: &mut Window, cx: &mut Context<Self>) {
        if let Some((filter, category, lens)) = tab.view {
            self.filter_by = filter;
            if self.sorting.tabs().contains(&category) {
                self.sorting.tab = category;
            }
            self.lens = lens;
        }
        if !tab.query.is_empty() {
            self.search_open = true;
            self.search_anim += 1;
            Self::set(&self.search, tab.query.clone(), w, cx);
            self.load_mail_kinds(cx);
            self.search_texts_soon(cx);
            if tab.server
                && let Some(a) = self.server_searchable()
            {
                // Kept this session: shown as found, once the mailbox is
                // listed. Otherwise (after a restart) the server is asked
                // again then, so its rows aren't replaced by the listing.
                let pending = match tab.server_found {
                    Some(found) => {
                        self.server_search = Some(later::ServerSearch {
                            account: a.id,
                            mailbox: self.mailbox(),
                            query: tab.query.trim().to_owned(),
                            running: false,
                            ids: found.ids,
                            total: found.total,
                            error: None,
                        });
                        Pending::Rows(found.rows)
                    }
                    None => Pending::Rerun,
                };
                self.tabs.pending = Some((self.load_generation, pending));
                if !self.cache_loading {
                    self.finish_tab_restore(cx);
                }
            }
        }
        self.filter(cx);
        if let Some(id) = tab.selected {
            self.open_listed = Some((
                self.load_generation,
                id,
                std::time::Instant::now() + SELECTED_WAIT,
            ));
            // The sample mailbox is listed already.
            self.apply_pending_listed(cx);
        }
        self.focus.focus(w, cx);
    }

    /// Saves the tabs, to open them again at the next launch; with one
    /// tab, the saved place (`save_place`) is enough.
    pub(super) fn save_tabs(&self, cx: &App) {
        let json = if self.tabs.several() {
            let current = self.tab_snapshot(cx, false);
            let tabs = self
                .tabs
                .list
                .iter()
                .enumerate()
                .map(|(i, t)| if i == self.tabs.active { &current } else { t })
                .collect();
            serde_json::to_string(&SavedRef {
                tabs,
                active: self.tabs.active,
            })
            .unwrap_or_default()
        } else {
            String::new()
        };
        if *self.tabs.saved.borrow() == json {
            return;
        }
        self.backend
            .store
            .set_preference_soon(TABS_PREFERENCE, &json);
        *self.tabs.saved.borrow_mut() = json;
    }

    /// Reads the tabs saved last, when there were several.
    fn load_tabs(&mut self) -> bool {
        let Some(json) = self.backend.store.preference(TABS_PREFERENCE) else {
            return false;
        };
        let Ok(saved) = serde_json::from_str::<Saved>(&json) else {
            return false;
        };
        // Tabs of accounts and views removed since are left out.
        let mut active = saved.active;
        let mut tabs = Vec::new();
        for (i, tab) in saved.tabs.into_iter().enumerate() {
            let account = tab.place.account.as_str();
            if !account.is_empty() && self.place_exists(account, &Folder::Inbox.into()) {
                tabs.push(tab);
            } else if i <= saved.active {
                active = active.saturating_sub(1);
            }
        }
        *self.tabs.saved.borrow_mut() = json;
        if tabs.len() < 2 {
            return false;
        }
        self.tabs.active = active.min(tabs.len() - 1);
        self.tabs.list = tabs;
        true
    }

    /// At launch: the tabs saved last, with the one that was open opened
    /// again. False when there were none (or one).
    pub(super) fn restore_tabs(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mobile.on || !self.load_tabs() {
            return false;
        }
        self.open_tab_place(self.tabs.list[self.tabs.active].clone(), w, cx);
        self.reveal_active_tab();
        true
    }

    /// At launch, when something else decides what opens (a tapped
    /// notification): the saved tabs, with it in the one that was open.
    pub(super) fn restore_tabs_only(&mut self) {
        if !self.mobile.on {
            self.load_tabs();
        }
    }

    /// The name of a tab's mailbox.
    fn tab_mailbox_label(&self, tab: &TabPlace) -> String {
        let place = &tab.place;
        if tab.local.is_some() {
            return t!("ui-results-on-this-computer").into();
        }
        if place.subscriptions {
            return t!("tabs-subscriptions").into();
        }
        if place.waiting {
            return t!("ui-waiting-for").into();
        }
        match &place.mailbox {
            Mailbox::Standard(folder) => folder.label().into(),
            Mailbox::Remote(name) => self
                .folders
                .get(&place.account)
                .and_then(|fs| fs.iter().find(|f| &f.name == name))
                .map(|f| f.label.clone())
                .unwrap_or_else(|| name.clone()),
        }
    }

    /// The name of a tab's account or view.
    fn tab_account_label(&self, account: &str) -> String {
        if account.is_empty() {
            return String::new();
        }
        if fwd::is_view(account) {
            return self
                .view_title(account)
                .map(|(name, _, _)| name)
                .unwrap_or_default();
        }
        self.accounts
            .iter()
            .find(|a| a.id == account)
            .map(|a| a.label().to_owned())
            .unwrap_or_default()
    }

    /// A tab's label, its tooltip and its icon. `accounts`: the tabs show
    /// more than one account, so each names its own.
    fn tab_title(&self, tab: &TabPlace, accounts: bool) -> (String, String, &'static str) {
        let mailbox = self.tab_mailbox_label(tab);
        let query = tab.query.trim();
        if tab.local.is_some() && !query.is_empty() {
            let tooltip = t!("tabs-search-on-this-computer", query = query);
            return (query.to_owned(), tooltip, "search");
        }
        let account = self.tab_account_label(&tab.place.account);
        let mailbox_and_account = if account.is_empty() {
            mailbox.clone()
        } else {
            t!(
                "tabs-mailbox-of-account",
                mailbox = mailbox,
                account = account
            )
        };
        if !query.is_empty() {
            let tooltip = t!(
                "tabs-search-in",
                query = query,
                mailbox = mailbox_and_account
            );
            return (query.to_owned(), tooltip, "search");
        }
        let icon = if tab.place.subscriptions {
            "subscriptions"
        } else if tab.place.waiting {
            "waiting"
        } else {
            folder_icon(tab.place.mailbox.folder())
        };
        let label = if accounts {
            mailbox_and_account.clone()
        } else {
            mailbox
        };
        (label, mailbox_and_account, icon)
    }

    /// The title bar's tabs, or with one tab only the button to open
    /// another. Takes the bar's free room.
    pub(super) fn tab_strip(&self, cx: &Context<Self>) -> AnyElement {
        let plus = window::no_drag(
            self.button(
                "new-tab",
                &shortcuts::tip(&format!("@{}", t!("tabs-new-tab")), "new-tab", cx),
                "plus",
                Action::Tabs(TabAction::New(None)),
                false,
                cx,
            )
            .h(px(28.))
            .w(px(28.))
            .rounded(px(7.)),
        );
        if self.mobile.on {
            return div().flex_1().into_any_element();
        }
        if !self.tabs.several() {
            return div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .pl_2()
                .child(plus)
                .into_any_element();
        }
        let shown: Vec<std::borrow::Cow<TabPlace>> = self
            .tabs
            .list
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == self.tabs.active {
                    std::borrow::Cow::Owned(self.tab_snapshot(cx, false))
                } else {
                    std::borrow::Cow::Borrowed(t)
                }
            })
            .collect();
        let accounts = shown
            .iter()
            .map(|t| t.place.account.as_str())
            .collect::<HashSet<_>>()
            .len()
            > 1;
        let n = shown.len();
        let row = div()
            .id("tabs")
            .debug_selector(|| "tabs".into())
            .role(Role::TabList)
            .aria_label(t!("tabs-tabs"))
            .track_scroll(&self.tabs.scroll)
            .overflow_x_scroll()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .children(
                shown
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| self.tab_element(tab, i, n, accounts, cx)),
            );
        // Ends with more tabs past them fade, as the inbox's categories do.
        let scroll = &self.tabs.scroll;
        let (offset, max) = (
            -f32::from(scroll.offset().x),
            f32::from(scroll.max_offset().x),
        );
        let (earlier, later) = (offset > 0.5, max - offset > 0.5);
        self.tabs.edges.set((earlier, later));
        let entity = cx.entity();
        let frame = div()
            .id("tabs-frame")
            .relative()
            .min_w_0()
            .flex()
            .items_center()
            .child(row)
            .when(earlier, |d| d.child(self.tab_row_edge(false, cx)))
            .when(later, |d| d.child(self.tab_row_edge(true, cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, _, cx| {
                        // Drawn again after this frame: a refresh asked for
                        // while drawing is dropped.
                        if entity.read(cx).settle_tab_row() {
                            cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
                        }
                    },
                )
                .absolute()
                .size_0(),
            );
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .pl_2()
            .child(frame)
            .child(plus)
            .child(div().flex_1().min_w(px(24.)).h_full())
            .into_any_element()
    }

    fn tab_element(
        &self,
        tab: &TabPlace,
        i: usize,
        count: usize,
        accounts: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let active = i == self.tabs.active;
        let (label, tooltip, icon) = self.tab_title(tab, accounts);
        let unread = if tab.query.trim().is_empty()
            && tab.local.is_none()
            && !tab.place.subscriptions
            && !tab.place.waiting
        {
            self.unread_for(&tab.place.account, &tab.place.mailbox)
        } else {
            0
        };
        let fg = if active { c.text } else { c.muted };
        let selector = format!("tab-{i}");
        let close_selector = format!("tab-close-{i}");
        let close_label = t!("tabs-close-tab");
        let menu_entries = vec![
            entry(
                t!("tabs-close-tab"),
                "close",
                Action::Tabs(TabAction::Close(i)),
            ),
            entry(
                t!("tabs-close-other-tabs"),
                "close",
                Action::Tabs(TabAction::CloseOthers(i)),
            ),
            Entry::Separator,
            entry(
                t!("tabs-new-tab"),
                "plus",
                Action::Tabs(TabAction::New(None)),
            ),
        ];
        let drag = TabDrag {
            index: i,
            label: label.clone(),
        };
        window::no_drag(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .role(Role::Tab)
                .aria_label(tooltip.clone())
                .aria_selected(active)
                .aria_position_in_set(i + 1)
                .aria_size_of_set(count)
                .focusable()
                .tab_stop(true)
                .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
                .flex_shrink_0()
                .h(px(28.))
                .min_w(px(72.))
                .max_w(px(200.))
                .pl_2()
                .pr(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(if active { c.border } else { c.bg }))
                .text_size(px(12.))
                .text_color(rgb(fg))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(c.surface)))
                .when(!active, |d| {
                    d.hover(move |s| s.bg(rgb(c.surface)).text_color(rgb(c.text)))
                })
                .child(icon_el(icon, 13., if active { c.accent } else { c.muted }))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when(unread > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if active { c.accent } else { c.muted }))
                            .child(fmt_count(unread)),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(close_selector.clone()))
                        .debug_selector(move || close_selector)
                        .role(Role::Button)
                        .aria_label(close_label)
                        .flex_shrink_0()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(move |s| s.bg(rgb(c.border)))
                        .child(icon_el("close", 10., c.muted))
                        .tooltip(move |w, cx| Tooltip::new(close_label).build(w, cx))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.handle(Action::Tabs(TabAction::Close(i)), w, cx)
                        })),
                )
                .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
                .on_click(cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.handle(Action::Tabs(TabAction::Switch(i)), w, cx)
                }))
                // The middle button closes a tab, as in a browser.
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Tabs(TabAction::Close(i)), w, cx)
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_menu(e.position, menu_entries.clone(), cx)
                    }),
                )
                // Dragged onto another tab, a tab takes its place.
                .on_drag(drag, move |drag: &TabDrag, offset, _, cx| {
                    cx.new(|_| organize::MailDragPreview {
                        label: drag.label.clone().into(),
                        c,
                        offset,
                    })
                })
                .drag_over::<TabDrag>(move |s, _, _, _| {
                    s.bg(rgb(c.soft)).border_color(rgb(c.accent))
                })
                .on_drop(cx.listener(move |this, d: &TabDrag, w, cx| {
                    this.handle(Action::Tabs(TabAction::Move(d.index, i)), w, cx)
                })),
        )
        .into_any_element()
    }
}
