//! The inbox's category tabs (see `manage::categories`): Primary,
//! Transactions, Updates, Promotions, Social and Forums above an inbox's
//! list, each with its unread count; moving senders from one to another
//! (a message's menu, or dragging messages onto a tab); the filter menu's
//! switch and Settings → Rules & Layout's card; and announcing only
//! Primary's new mail when asked.
//!
//! Each listed message is sorted from a quick look at once, and from a
//! whole one (receipts, packages, codes, and the inbox category model when
//! Settings → Local AI → Sort the inbox is on) in the background, kept per
//! message (`Sorter`), so filtering the list never waits for it. Moving
//! senders also teaches the model, on this computer (`learn_from_moved`).
use super::*;
use crate::manage::categories::{self as cats, Categorizing, Category, Known, Sorter, SortingAi};
use crate::manage::local_ai;
use std::sync::atomic::{AtomicU64, Ordering};

/// A tab above the inbox: a category's, or All Mail at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Is(Category),
    /// The whole inbox, each row saying where it was sorted.
    All,
}

impl Tab {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Is(c) => c.key(),
            Self::All => "all",
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Is(c) => c.label(),
            Self::All => t!("categories-all-mail"),
        }
    }
    pub(super) fn about(self) -> &'static str {
        match self {
            Self::Is(c) => c.about(),
            Self::All => t!("categories-all-mail-about"),
        }
    }
    pub(super) fn icon(self) -> &'static str {
        match self {
            Self::Is(c) => c.icon(),
            Self::All => "all-mail",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CatAction {
    /// Shows a tab's mail.
    Tab(Category),
    /// Shows the whole inbox (All Mail, the last tab).
    AllMail,
    /// Scrolls the tabs toward their start (`false`) or end (`true`).
    ScrollTabs(bool),
    /// Turns the inbox's categories on or off.
    ToggleEnabled,
    /// Turns a category's tab on or off.
    ToggleTab(Category),
    /// Announces only Primary's new mail, or all of it again.
    ToggleNotifyPrimaryOnly,
    /// Makes All Mail the first tab and the one the inbox opens on, or
    /// puts it back last, after Primary.
    ToggleAllMailFirst,
    /// Lists the tabs to move these messages' senders to (none: the
    /// selection's).
    Menu(Vec<String>),
    /// Sorts these messages' senders (none: the selection's) into a
    /// category from now on.
    MoveSenders(Vec<String>, Category),
    /// Forgets where a sender (an address or `@domain`) was sorted.
    ForgetSender(String),
}

/// The inbox's sorting, as the window has it.
pub(super) struct Sorting {
    /// What sorts listed mail; shared with searches (`category:`).
    pub(super) sorter: Arc<Sorter>,
    /// The tab shown.
    pub(super) tab: Tab,
    /// Each tab's unread listed mail, by `Category as usize`, and the
    /// whole inbox's.
    pub(super) unread: [usize; Category::ALL.len()],
    pub(super) all_unread: usize,
    /// Where the tabs' row is scrolled, on a desktop.
    pub(super) tabs_scroll: ScrollHandle,
    /// Listed mail only looked at quickly so far.
    wanted: bool,
    /// A whole look at it is running.
    looking: bool,
    /// What was found in mail before (`Store::looks`) has been read: until
    /// then, nothing is looked at closely again.
    looks_loaded: bool,
    /// The accounts' list revisions when who is known was last read, and
    /// when.
    known_read: Option<(Vec<u64>, std::time::Instant)>,
    known_loading: bool,
    /// Settings → Local AI → Sort the inbox: the inbox category model
    /// sorts too. Its model downloading, how far it has got, and what went
    /// wrong getting it.
    pub(super) ai_on: bool,
    pub(super) ai_busy: bool,
    pub(super) ai_progress: Arc<AtomicU64>,
    pub(super) ai_error: Option<String>,
}

/// Who is known is read again at most this often while mail changes.
const KNOWN_EVERY: Duration = Duration::from_secs(60);
/// How many messages one background look looks at closely (see
/// `sort_in_background`).
const CLOSE_LOOK_PART: usize = 4000;
/// How many messages not looked at yet the window looks at quickly itself
/// as a list is sorted (mail just arrived); the rest are looked at in the
/// background.
const QUICK_AT_MOST: usize = 48;
/// How many of a moved sender's messages the model learns from.
const LESSONS_PER_SENDER: usize = 12;
/// How wide the fade is at an end of the row of tabs with more past it.
const TABS_FADE: f32 = 56.;

impl Sorting {
    pub(super) fn load(store: &Store) -> Self {
        let prefs = Categorizing::load(store);
        Self {
            tab: Self::first_tab(&prefs),
            sorter: Arc::new(Sorter::new(prefs)),
            unread: [0; Category::ALL.len()],
            all_unread: 0,
            tabs_scroll: ScrollHandle::new(),
            wanted: false,
            looking: false,
            looks_loaded: false,
            known_read: None,
            known_loading: false,
            ai_on: local_ai::categories_enabled(store),
            ai_busy: false,
            ai_progress: Arc::default(),
            ai_error: None,
        }
    }
    pub(super) fn prefs(&self) -> &Categorizing {
        &self.sorter.prefs
    }
    /// The tab the inbox opens on: All Mail where it comes first, else
    /// Primary.
    fn first_tab(prefs: &Categorizing) -> Tab {
        if prefs.all_mail_first {
            Tab::All
        } else {
            Tab::Is(Category::Primary)
        }
    }
    /// The tab the inbox opens on.
    pub(super) fn default_tab(&self) -> Tab {
        Self::first_tab(self.prefs())
    }
    /// The tabs shown, in order: the categories with one, and All Mail
    /// last, or first where the person chose (`all_mail_first`).
    pub(super) fn tabs(&self) -> Vec<Tab> {
        let categories = self.prefs().tabs().into_iter().map(Tab::Is);
        if self.prefs().all_mail_first {
            std::iter::once(Tab::All).chain(categories).collect()
        } else {
            categories.chain([Tab::All]).collect()
        }
    }
    /// A tab's unread listed mail.
    pub(super) fn unread_in(&self, tab: Tab) -> usize {
        match tab {
            Tab::Is(c) => self.unread[c as usize],
            Tab::All => self.all_unread,
        }
    }
}

impl MailApp {
    /// Whether the open mailbox shows category tabs: an inbox (one
    /// account's, or every account's together), sorted, and not results
    /// or subscriptions shown in its place.
    pub(super) fn shows_categories(&self) -> bool {
        self.sorting.prefs().sorting()
            && self.folder == Folder::Inbox
            && self.local_results.is_none()
            && !self.subs_open
            && !self.summary_open
    }

    /// Before the list is filtered: looks quickly at listed mail not looked
    /// at yet (a whole look follows, `sort_in_background`) and counts each
    /// tab's unread mail. Returns the sorter when the list is sorted into
    /// tabs or `searching` asks for a category.
    pub(super) fn sort_listed(&mut self, searching: bool) -> Option<Arc<Sorter>> {
        let tabs = self.shows_categories();
        self.sorting.unread = [0; Category::ALL.len()];
        self.sorting.all_unread = 0;
        if !tabs && !searching {
            return None;
        }
        let sorter = Arc::make_mut(&mut self.sorting.sorter);
        let wanted = sorter.look_quickly_at_most(self.messages.iter().filter(|m| !m.pending), QUICK_AT_MOST);
        self.sorting.wanted |= !wanted.is_empty();
        sorter.forget_unlisted(&self.messages);
        if tabs {
            let extras = &self.thread_extras;
            for m in self.messages.iter().filter(|m| {
                m.unread && !m.pending && !extras.contains(&m.id) && self.scope.admits(m)
            }) {
                self.sorting.unread[sorter.tab_known(m) as usize] += 1;
                self.sorting.all_unread += 1;
            }
        }
        Some(self.sorting.sorter.clone())
    }

    /// Keeps only the open tab's rows. Placeholders (rows not listed yet)
    /// stay in Primary, where they hold their places until they load.
    pub(super) fn keep_tab(&mut self, sorter: &Sorter) {
        let Tab::Is(tab) = self.sorting.tab else {
            // All Mail keeps every row.
            return;
        };
        let messages = &self.messages;
        self.visible.retain(|&i| {
            let m = &messages[i];
            if m.pending {
                tab == Category::Primary
            } else {
                sorter.tab_known(m) == tab
            }
        });
    }

    /// Looks closely, in the background, at listed mail only looked at
    /// quickly (for receipts, package updates and codes); the list is
    /// sorted again with what is found. Asked for as the window draws.
    pub(super) fn sort_in_background(&mut self, cx: &mut Context<Self>) {
        if !self.sorting.wanted || self.sorting.looking {
            return;
        }
        // Not while mail is being checked (at launch, every account at
        // once): the whole look, with the category model, competed with it
        // and with the window for the cores. Asked again as the window
        // draws once the checks are done.
        if !self.sync_requests.is_empty() || !self.sorting.looks_loaded {
            return;
        }
        self.sorting.wanted = false;
        let sorter = self.sorting.sorter.clone();
        // The newest first, a part at a time: a long list's whole look (at
        // launch, every message, with the category model where it's on)
        // kept the cores busy for minutes before any of it showed, and
        // copying the whole list for it held up the window. What's left
        // is asked for again once this part is kept (`sort_listed`).
        let wanted: Vec<Mail> = self
            .messages
            .iter()
            .filter(|m| !m.pending && sorter.wants_close_look(m))
            .take(CLOSE_LOOK_PART)
            .map(|m| {
                let mut copy = m.clone();
                copy.body = None;
                copy.html = None;
                copy
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.sorting.looking = true;
        let ai = sorter.model();
        let store = self.backend.store.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let found = cats::look_closely(&wanted, ai.as_deref());
                    // Kept, so the next launch needn't look again.
                    if let Err(e) = store.save_looks(&found) {
                        eprintln!("Picacho: couldn't keep the inbox tabs' looks: {e:#}");
                    }
                    found
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sorting.looking = false;
                Arc::make_mut(&mut this.sorting.sorter).keep(found);
                this.filter(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads what sorting found in mail before (`Store::looks`), off the
    /// window's thread, at launch: a large mailbox was looked at again,
    /// whole, every time Picacho started.
    pub(super) fn load_looks(&mut self, cx: &mut Context<Self>) {
        let store = self.backend.store.clone();
        cx.spawn(async move |this, cx| {
            let looks = cx.background_executor().spawn(async move { store.looks() }).await;
            let _ = this.update(cx, |this, cx| {
                this.sorting.looks_loaded = true;
                match looks {
                    Ok(looks) if !looks.is_empty() => {
                        Arc::make_mut(&mut this.sorting.sorter).keep(looks);
                        this.filter(cx);
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("Picacho: couldn't read the inbox tabs' looks: {e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads who is known (people written to, the address book, your own
    /// addresses) again when mail changed, at most every `KNOWN_EVERY`:
    /// their mail is Primary.
    pub(super) fn load_known(&mut self, cx: &mut Context<Self>) {
        if self.sorting.known_loading || !self.sorting.prefs().sorting() || self.accounts.is_empty()
        {
            return;
        }
        let revisions: Vec<u64> = self
            .accounts
            .iter()
            .map(|a| self.backend.store.list_revision(&a.id))
            .collect();
        if let Some((read, at)) = &self.sorting.known_read
            && (*read == revisions || (read.len() == revisions.len() && at.elapsed() < KNOWN_EVERY))
        {
            return;
        }
        self.sorting.known_loading = true;
        let accounts = self.accounts.clone();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let known = cx
                .background_executor()
                .spawn(async move { Known::load(&backend, &accounts) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sorting.known_loading = false;
                this.sorting.known_read = Some((revisions, std::time::Instant::now()));
                if *this.sorting.sorter.known != known {
                    Arc::make_mut(&mut this.sorting.sorter).known = Arc::new(known);
                    this.filter(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether new mail like `m` is announced, as far as categories go:
    /// with only Primary announced, mail in other tabs arrives quietly.
    /// New mail is few messages, each looked at closely here (a code is
    /// Primary, which a quick look can't tell).
    pub(super) fn announces(&self, m: &Mail) -> bool {
        let sorter = &self.sorting.sorter;
        let prefs = &sorter.prefs;
        if !prefs.notify_primary_only || !prefs.sorting() {
            return true;
        }
        let ai = sorter.model();
        let content = sorter
            .content(m)
            .filter(|c| c.complete)
            .unwrap_or_else(|| cats::Content::of_with(m, ai.as_deref()));
        prefs.fold(cats::decide(m, content, prefs, &sorter.known)) == Category::Primary
    }

    fn save_categories(&mut self, cx: &mut Context<Self>) {
        self.sorting.prefs().save(&self.backend.store);
        if !self.sorting.tabs().contains(&self.sorting.tab) {
            self.sorting.tab = self.sorting.default_tab();
        }
        self.filter(cx);
        cx.notify();
    }

    pub(super) fn cat_action(&mut self, action: CatAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            CatAction::Tab(tab) => self.show_tab(Tab::Is(tab), cx),
            CatAction::AllMail => self.show_tab(Tab::All, cx),
            CatAction::ScrollTabs(later) => self.scroll_tabs(later, cx),
            CatAction::ToggleEnabled => {
                let prefs = &mut Arc::make_mut(&mut self.sorting.sorter).prefs;
                prefs.enabled = !prefs.enabled;
                self.sorting.tab = self.sorting.default_tab();
                self.save_categories(cx);
                self.load_known(cx);
            }
            CatAction::ToggleTab(c) => {
                Arc::make_mut(&mut self.sorting.sorter).prefs.toggle_tab(c);
                self.save_categories(cx);
            }
            CatAction::ToggleNotifyPrimaryOnly => {
                let prefs = &mut Arc::make_mut(&mut self.sorting.sorter).prefs;
                prefs.notify_primary_only = !prefs.notify_primary_only;
                self.save_categories(cx);
            }
            CatAction::ToggleAllMailFirst => {
                let prefs = &mut Arc::make_mut(&mut self.sorting.sorter).prefs;
                prefs.all_mail_first = !prefs.all_mail_first;
                self.save_categories(cx);
                // Shown from the start of the row, where the first tab is.
                for scroll in [&self.sorting.tabs_scroll, &self.mobile.categories_scroll] {
                    scroll.set_offset(point(px(0.), scroll.offset().y));
                }
            }
            CatAction::Menu(ids) => {
                let entries = self.cat_menu_entries(&ids);
                if !entries.is_empty() {
                    self.open_menu_here(entries, w, cx);
                }
            }
            CatAction::MoveSenders(ids, c) => {
                let senders = self.senders_of(&ids);
                if senders.is_empty() {
                    return;
                }
                let prefs = &mut Arc::make_mut(&mut self.sorting.sorter).prefs;
                for s in &senders {
                    prefs.set_sender(s, Some(c));
                }
                self.learn_from_moved(&senders, c);
                let label = self.sorting.prefs().fold(c).label();
                let text = match senders.as_slice() {
                    [one] => t!("categories-moved-sender", from = one, category = label),
                    _ => t!(
                        "categories-moved-senders",
                        count = senders.len(),
                        category = label
                    ),
                };
                self.save_categories(cx);
                self.notice(text, false, cx);
            }
            CatAction::ForgetSender(address) => {
                Arc::make_mut(&mut self.sorting.sorter)
                    .prefs
                    .set_sender(&address, None);
                self.save_categories(cx);
            }
        }
    }

    /// Shows a tab's mail, bringing the tab into view where the row of
    /// tabs is scrolled.
    fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.sorting.tab != tab {
            self.sorting.tab = tab;
            self.selection.clear();
            self.filter(cx);
            self.scroll_list_to_top();
        }
        let tabs = self.sorting.tabs();
        if let Some(i) = tabs.iter().position(|t| *t == tab) {
            for scroll in [&self.sorting.tabs_scroll, &self.mobile.categories_scroll] {
                // The first and last go all the way, past the row's
                // padding, so no fade is left over them.
                if i == 0 {
                    scroll.set_offset(point(px(0.), scroll.offset().y));
                } else if i + 1 == tabs.len() {
                    scroll.set_offset(point(-scroll.max_offset().x, scroll.offset().y));
                } else {
                    scroll.scroll_to_item(i);
                }
            }
        }
        cx.notify();
    }

    /// Scrolls the row of tabs most of its width toward its end (`later`)
    /// or its start, as far as it goes.
    fn scroll_tabs(&mut self, later: bool, cx: &mut Context<Self>) {
        let scroll = &self.sorting.tabs_scroll;
        let (offset, max) = (-scroll.offset().x, scroll.max_offset().x);
        let step = (scroll.bounds().size.width * 0.7).max(px(80.));
        let to = if later { offset + step } else { offset - step };
        let to = to.clamp(px(0.), max.max(px(0.)));
        scroll.set_offset(point(-to, scroll.offset().y));
        cx.notify();
    }

    /// The senders of these messages (none: the selection's), lowercase,
    /// each once, in order.
    fn senders_of(&self, ids: &[String]) -> Vec<String> {
        let chosen: HashSet<&String> = if ids.is_empty() {
            self.selection.iter().collect()
        } else {
            ids.iter().collect()
        };
        let mut seen = HashSet::new();
        self.messages
            .iter()
            .filter(|m| chosen.contains(&m.id))
            .map(|m| m.from.trim().to_lowercase())
            .filter(|from| !from.is_empty() && seen.insert(from.clone()))
            .collect()
    }

    /// The message menu's entry for moving its sender to another tab.
    pub(super) fn cat_entries(&self, m: &Mail) -> Vec<Entry> {
        if !self.sorting.prefs().sorting()
            || m.from.trim().is_empty()
            || self.folder == Folder::Drafts
        {
            return vec![];
        }
        vec![entry(
            t!("categories-move-to-category"),
            "inboxes",
            Action::Categories(CatAction::Menu(vec![m.id.clone()])),
        )]
    }

    /// The tabs to move messages' senders to, the one they're in checked.
    pub(super) fn cat_menu_entries(&self, ids: &[String]) -> Vec<Entry> {
        let chosen: HashSet<&String> = if ids.is_empty() {
            self.selection.iter().collect()
        } else {
            ids.iter().collect()
        };
        let sorter = &self.sorting.sorter;
        let current: HashSet<Category> = self
            .messages
            .iter()
            .filter(|m| chosen.contains(&m.id))
            .map(|m| sorter.tab_known(m))
            .collect();
        let only = (current.len() == 1)
            .then(|| current.iter().next().copied())
            .flatten();
        sorter
            .prefs
            .tabs()
            .into_iter()
            .map(|c| {
                checked(
                    t!("categories-move-to", category = c.label()),
                    only == Some(c),
                    Action::Categories(CatAction::MoveSenders(ids.to_vec(), c)),
                )
            })
            .collect()
    }

    /// The filter menu's switch for the inbox's tabs.
    pub(super) fn cat_filter_entries(&self) -> Vec<Entry> {
        if self.folder != Folder::Inbox || self.local_results.is_some() {
            return vec![];
        }
        vec![
            Entry::Separator,
            checked(
                t!("categories-show-categories"),
                self.sorting.prefs().enabled,
                Action::Categories(CatAction::ToggleEnabled),
            ),
        ]
    }

    /// The tabs above an inbox's list, while nothing is searched for: each
    /// category's, then All Mail. Where they don't all fit, the row
    /// scrolls (the wheel scrolls it sideways), and the side with more
    /// tabs past it fades out under an arrow that scrolls toward them.
    pub(super) fn category_tabs(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.shows_categories() || !Self::value(&self.search, cx).trim().is_empty() {
            return None;
        }
        let tabs = self.sorting.tabs();
        let count = tabs.len();
        let scroll = &self.sorting.tabs_scroll;
        let (offset, max) = (
            -f32::from(scroll.offset().x),
            f32::from(scroll.max_offset().x),
        );
        let row = div()
            .id("category-tabs")
            .debug_selector(|| "category-tabs".into())
            .role(Role::TabList)
            .aria_label(t!("categories-tabs"))
            .track_scroll(scroll)
            .overflow_x_scroll()
            .flex()
            .items_center()
            .gap_1()
            .children(
                tabs.into_iter()
                    .enumerate()
                    .map(|(i, tab)| self.category_tab(tab, i, count, cx)),
            );
        Some(
            div()
                .id("category-tabs-frame")
                .relative()
                .mt_2()
                .w_full()
                .min_w_0()
                .child(row)
                .when(offset > 0.5, |d| d.child(self.tabs_edge(false, cx)))
                .when(max - offset > 0.5, |d| d.child(self.tabs_edge(true, cx)))
                .into_any_element(),
        )
    }

    /// One tab: its icon, name and unread count; a drop target for
    /// messages, whose senders go there (but All Mail).
    fn category_tab(&self, tab: Tab, i: usize, count: usize, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let active = self.sorting.tab == tab;
        let unread = self.sorting.unread_in(tab);
        let selector = format!("category-tab-{}", tab.key());
        let spoken = if unread > 0 {
            let shown = fmt_count(unread);
            let count = t!("categories-unread", shown = shown, count = unread);
            format!("{}, {count}", tab.label())
        } else {
            tab.label().to_owned()
        };
        let fg = if active { c.accent } else { c.muted };
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .role(Role::Tab)
            .aria_label(spoken)
            .aria_selected(active)
            .aria_position_in_set(i + 1)
            .aria_size_of_set(count)
            .focusable()
            .tab_stop(true)
            .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
            .flex_shrink_0()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(if active { c.accent } else { c.surface }))
            .text_size(px(12.))
            .text_color(rgb(fg))
            .font_weight(if active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .whitespace_nowrap()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(c.soft)))
            .when(!active, |d| {
                d.hover(move |s| s.bg(rgb(c.bg)).text_color(rgb(c.text)))
            })
            .child(icon_el(tab.icon(), 14., fg))
            .child(tab.label())
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .px(px(5.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(rgb(if active { c.accent } else { c.soft }))
                        .text_color(rgb(if active { c.on_accent } else { c.accent }))
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(fmt_count(unread)),
                )
            })
            .tooltip(move |w, cx| Tooltip::new(tab.about()).build(w, cx))
            .map(|d| match tab {
                // Messages dropped on a category's tab: their senders go
                // there.
                Tab::Is(category) => d
                    .drag_over::<organize::MailDrag>(move |s, _, _, _| {
                        s.bg(rgb(c.soft))
                            .border_color(rgb(c.accent))
                            .text_color(rgb(c.accent))
                    })
                    .on_drop(cx.listener(move |this, d: &organize::MailDrag, w, cx| {
                        this.cat_action(CatAction::MoveSenders(d.ids.clone(), category), w, cx)
                    }))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Categories(CatAction::Tab(category)), w, cx)
                    })),
                Tab::All => d.on_click(cx.listener(|this, _, w, cx| {
                    this.handle(Action::Categories(CatAction::AllMail), w, cx)
                })),
            })
    }

    /// The end of the row of tabs with more past it (`later`: the
    /// trailing end): the tabs fade into the list's background under a
    /// round arrow that scrolls toward them. The fade takes no clicks; the
    /// tab under it still opens.
    fn tabs_edge(&self, later: bool, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let solid: Hsla = rgb(c.surface).into();
        let clear = Hsla { a: 0., ..solid };
        let (id, label, icon) = if later {
            (
                "category-tabs-later",
                t!("categories-more-tabs"),
                "chevron-right",
            )
        } else {
            (
                "category-tabs-earlier",
                t!("categories-earlier-tabs"),
                "chevron-left",
            )
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
            .child(
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(Role::Button)
                    .aria_label(label)
                    .size(px(24.))
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
                    .child(icon_el(icon, 14., c.text))
                    .tooltip(move |w, cx| Tooltip::new(label).build(w, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.handle(Action::Categories(CatAction::ScrollTabs(later)), w, cx)
                    })),
            )
    }

    /// In All Mail, the tag a row shows of the tab it is sorted into (none
    /// for Primary's).
    pub(super) fn row_category(&self, m: &Mail, size: f32) -> Option<AnyElement> {
        if self.sorting.tab != Tab::All || m.pending || !self.shows_categories() {
            return None;
        }
        let tab = self.sorting.sorter.tab_known(m);
        if tab == Category::Primary {
            return None;
        }
        let c = self.colors();
        let selector = format!("row-category-{}", m.id);
        Some(
            div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector)
                .flex_shrink_0()
                .h(px(size + 6.))
                .px(px(5.))
                .flex()
                .items_center()
                .gap(px(3.))
                .rounded(px(4.))
                .bg(rgb(c.soft))
                .text_size(px(size))
                .text_color(rgb(c.accent))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .child(icon_el(tab.icon(), size, c.accent))
                .child(tab.label())
                .into_any_element(),
        )
    }

    /// What an empty tab says: its icon, its name and what goes in it.
    pub(super) fn empty_tab(&self) -> (&'static str, String, Option<String>) {
        let tab = self.sorting.tab;
        if tab == Tab::All {
            return (tab.icon(), t!("ui-no-mail-here").into(), None);
        }
        (
            tab.icon(),
            t!("categories-nothing-in", category = tab.label()),
            Some(tab.about().to_owned()),
        )
    }

    /// Teaches the inbox category model, where it is on, that mail from
    /// `senders` (as listed now, the newest few of each) belongs in
    /// `category`, and keeps what it learned. Mail like it from other
    /// senders then goes there too; it is all sorted again.
    fn learn_from_moved(&mut self, senders: &[String], category: Category) {
        let Some(ai) = self.sorting.sorter.model() else {
            return;
        };
        let mut moved: Vec<&Mail> = Vec::new();
        for sender in senders {
            moved.extend(
                self.messages
                    .iter()
                    .filter(|m| !m.pending && m.from.trim().eq_ignore_ascii_case(sender))
                    .take(LESSONS_PER_SENDER),
            );
        }
        if moved.is_empty() {
            return;
        }
        let taught = ai.learn(&moved, category);
        if let Ok(json) = serde_json::to_string(&taught.learned) {
            self.backend
                .store
                .set_preference_soon(local_ai::CATEGORIES_LEARNED_PREFERENCE, &json);
        }
        Arc::make_mut(&mut self.sorting.sorter).ai = Some(SortingAi(Arc::new(taught)));
    }

    /// Forgets what the model learned from the mail moved.
    pub(super) fn forget_sort_learning(&mut self, cx: &mut Context<Self>) {
        if let Some(ai) = self.sorting.sorter.model() {
            Arc::make_mut(&mut self.sorting.sorter).ai = Some(SortingAi(Arc::new(ai.forgetting())));
        }
        let _ = self
            .backend
            .store
            .remove_preference(local_ai::CATEGORIES_LEARNED_PREFERENCE);
        self.notice(t!("local-ai-picacho-forgot-how-you-sorted"), false, cx);
        self.filter(cx);
        cx.notify();
    }

    /// How many messages the model learned from here.
    pub(super) fn sort_lessons(&self) -> u32 {
        self.sorting
            .sorter
            .model()
            .map_or(0, |ai| ai.learned.lessons)
    }

    /// Loads the inbox category model at launch, or installs it where it
    /// should be there and isn't.
    pub(super) fn start_sort_ai(&mut self, cx: &mut Context<Self>) {
        if !self.sorting.ai_on {
            return;
        }
        let _ = local_ai::remove_stale(&self.backend.store);
        if local_ai::CATEGORIES.is_installed(&self.backend.store) {
            self.load_sort_ai(cx);
        } else {
            self.install_sort_ai(cx);
        }
    }
    fn load_sort_ai(&mut self, cx: &mut Context<Self>) {
        let ai = match local_ai::category_ai(&self.backend.store) {
            Ok(ai) => {
                self.sorting.ai_error = None;
                Some(SortingAi(Arc::new(ai)))
            }
            Err(e) => {
                self.sorting.ai_error = Some(format!("{e:#}"));
                None
            }
        };
        Arc::make_mut(&mut self.sorting.sorter).ai = ai;
        self.filter(cx);
        cx.notify();
    }
    fn install_sort_ai(&mut self, cx: &mut Context<Self>) {
        if self.sorting.ai_busy {
            return;
        }
        self.sorting.ai_busy = true;
        self.sorting.ai_error = None;
        self.sorting.ai_progress.store(0, Ordering::Relaxed);
        let received = self.sorting.ai_progress.clone();
        cx.notify();
        self.task(
            cx,
            move |b| local_ai::CATEGORIES.install(&b.store, &received),
            |this, result, cx| {
                this.sorting.ai_busy = false;
                match result {
                    // Turned off while installing: don't keep it.
                    Ok(()) if !this.sorting.ai_on => {
                        let _ = local_ai::CATEGORIES.remove(&this.backend.store);
                    }
                    Ok(()) => this.load_sort_ai(cx),
                    Err(e) => {
                        this.sorting.ai_error = Some(format!("{e:#}"));
                        this.set_sort_ai(false, cx);
                    }
                }
                cx.notify();
            },
        );
        self.follow_download(cx);
    }
    /// Turns Settings → Local AI → Sort the inbox on (installing the model)
    /// or off (deleting it; what was learned is kept for next time).
    pub(super) fn set_sort_ai(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.local_ai;
        self.sorting.ai_on = on;
        self.backend.store.set_preference_soon(
            local_ai::CATEGORIES_PREFERENCE,
            if on { "true" } else { "false" },
        );
        if on {
            self.start_sort_ai(cx);
        } else {
            if self.sorting.sorter.ai.is_some() {
                Arc::make_mut(&mut self.sorting.sorter).ai = None;
                self.filter(cx);
            }
            if let Err(e) = local_ai::CATEGORIES.remove(&self.backend.store) {
                self.notice(format!("{e:#}"), true, cx);
            }
        }
        cx.notify();
    }

    /// Settings → Rules & Layout's cards for the inbox's categories.
    pub(super) fn categories_settings(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let prefs = self.sorting.prefs();
        let act = |a: CatAction| Action::Categories(a);
        let mut rows = Rows::new().row(self.switch_setting(
            t!("categories-sort-the-inbox"),
            t!("categories-sort-the-inbox-caption"),
            "categories-on".into(),
            prefs.enabled,
            act(CatAction::ToggleEnabled),
            cx,
        ));
        if prefs.enabled {
            rows = rows.row(self.switch_setting(
                t!("categories-all-mail-first"),
                t!("categories-all-mail-first-caption"),
                "categories-all-mail-first".into(),
                prefs.all_mail_first,
                act(CatAction::ToggleAllMailFirst),
                cx,
            ));
            for category in Category::ALL
                .into_iter()
                .filter(|c| *c != Category::Primary)
            {
                rows = rows.row(self.switch_setting(
                    category.label(),
                    category.about(),
                    format!("category-shown-{}", category.key()),
                    !prefs.hidden.contains(&category),
                    act(CatAction::ToggleTab(category)),
                    cx,
                ));
            }
            rows = rows.row(self.switch_setting(
                t!("categories-notify-primary-only"),
                t!("categories-notify-primary-only-caption"),
                "categories-notify-primary".into(),
                prefs.notify_primary_only,
                act(CatAction::ToggleNotifyPrimaryOnly),
                cx,
            ));
        }
        let mut senders = Rows::new();
        if prefs.enabled {
            if prefs.senders.is_empty() {
                senders = senders.row(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(t!("categories-senders-none")),
                );
            }
            for (i, (address, category)) in prefs.senders.iter().enumerate() {
                senders = senders.row(
                    div()
                        .id(SharedString::from(format!("category-sender-{i}")))
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(icon_el(category.icon(), 14., c.muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .child(t!(
                                    "categories-sender-goes-to",
                                    from = address,
                                    category = category.label()
                                )),
                        )
                        .child(self.button(
                            format!("forget-category-sender-{i}"),
                            t!("categories-forget-sender"),
                            "",
                            act(CatAction::ForgetSender(address.clone())),
                            false,
                            cx,
                        )),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(self.settings_group(t!("categories-section"), rows))
            .child(self.settings_group(t!("categories-senders"), senders))
    }
}
