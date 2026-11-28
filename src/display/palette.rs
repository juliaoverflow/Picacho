//! The command palette (⌘K, Ctrl+K elsewhere): one field that finds and
//! runs what Picacho can do, by typing a few letters of it. It lists the
//! actions shortcuts can be given to (`shortcuts::bindable`) and a few more,
//! the workspace's accounts and their folders, labels, saved searches,
//! contacts and templates, and, with a message open or some selected, quick
//! actions on them: Move to…, Snooze until… and Apply label…, each a list of
//! its own choices.
//!
//! What it lists is gathered when it opens (and when a quick action's
//! choices open), and filtered as you type with [`fuzzy`]: the letters typed
//! in order, each word of the query anywhere in a result, words and their
//! starts counting most.
use super::*;
use crate::manage::composing::{describe_later, snooze_choices};
use calendar_views::{AppView, CalAction};
use unicode_normalization::UnicodeNormalization;

/// A result row's height.
const ROW: f32 = 38.;
/// The most rows shown before the list scrolls.
const ROWS_SHOWN: usize = 10;
/// The panel's width, where the window has room.
const WIDTH: f32 = 640.;

/// What the palette lists: everything, or one quick action's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    All,
    /// The folders the messages can be moved to.
    MoveTo,
    /// When to bring the messages back.
    Snooze,
    /// Labels to add to the messages or take off them.
    Label,
}

/// What a result is, shown beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Quick,
    Command,
    Account,
    Folder,
    SavedSearch,
    Label,
    Template,
    Contact,
    /// One of a quick action's choices.
    Choice,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Quick => t!("palette-kind-quick-action"),
            Self::Command => t!("palette-kind-command"),
            Self::Account => t!("palette-kind-account"),
            Self::Folder => t!("palette-kind-folder"),
            Self::SavedSearch => t!("palette-kind-saved-search"),
            Self::Label => t!("palette-kind-label"),
            Self::Template => t!("palette-kind-template"),
            Self::Contact => t!("palette-kind-contact"),
            Self::Choice => "",
        }
    }
}

/// What choosing a result does.
pub(super) enum Run {
    /// One of the window's own actions (boxed: `Action` is large).
    Action(Box<Action>),
    /// A menu or keyboard command, dispatched as its shortcut would be.
    Command(Box<dyn gpui::Action>),
    /// Searches every account's mail on this computer for this.
    Search(String),
    /// A new message from this template (its place in the list).
    Template(usize),
    /// Snoozes the messages until then (a Unix time).
    Snooze(i64),
    /// Shows a quick action's choices.
    Mode(Mode),
}

impl Run {
    fn action(action: Action) -> Self {
        Self::Action(Box::new(action))
    }
}

/// One thing the palette can find.
pub(super) struct Item {
    pub(super) kind: Kind,
    pub(super) title: SharedString,
    /// Shown after the title, dimmer: the account, the address, the group.
    pub(super) detail: SharedString,
    /// Other words it's found by, not shown.
    keywords: String,
    icon: &'static str,
    /// Its shortcut, as shown.
    keys: Option<String>,
    /// A label the messages all have already.
    checked: bool,
    pub(super) run: Run,
}
impl Item {
    fn new(kind: Kind, title: impl Into<SharedString>, icon: &'static str, run: Run) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: SharedString::default(),
            keywords: String::new(),
            icon,
            keys: None,
            checked: false,
            run,
        }
    }
    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = detail.into();
        self
    }
    fn keywords(mut self, keywords: impl Into<String>) -> Self {
        self.keywords = keywords.into();
        self
    }
}

/// A result for the query: the item, how well it matched, and which
/// characters of its title and detail did (by character).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Found {
    pub(super) item: usize,
    score: i32,
    title_hits: Vec<usize>,
    detail_hits: Vec<usize>,
}

/// The palette's state.
pub(super) struct Palette {
    pub(super) open: bool,
    pub(super) mode: Mode,
    pub(super) input: Entity<InputState>,
    pub(super) items: Vec<Item>,
    /// The results for the query typed, best first.
    pub(super) found: Vec<Found>,
    /// The highlighted result (in `found`).
    pub(super) chosen: usize,
    /// The messages quick actions act on, as the palette opened.
    target: Option<MoveTarget>,
    scroll: UniformListScrollHandle,
    anim: u64,
}
impl Palette {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            open: false,
            mode: Mode::All,
            input: input(t!("palette-type-a-command-or-search"), false, window, cx),
            items: Vec::new(),
            found: Vec::new(),
            chosen: 0,
            target: None,
            scroll: UniformListScrollHandle::new(),
            anim: 0,
        }
    }
}

/// What the palette does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PaletteAction {
    /// Opens the palette, or closes it when it's open.
    Toggle,
    Close,
    /// Runs the result in this row.
    Choose(usize),
    /// Moves the highlight this many rows.
    Step(isize),
    /// From a quick action's choices back to everything.
    Back,
}

impl MailApp {
    pub(super) fn palette_action(
        &mut self,
        action: PaletteAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PaletteAction::Toggle if self.palette.open => self.close_palette(w, cx),
            PaletteAction::Toggle => self.open_palette(w, cx),
            PaletteAction::Close => self.close_palette(w, cx),
            PaletteAction::Choose(row) => self.choose_in_palette(row, w, cx),
            PaletteAction::Step(by) => {
                let n = self.palette.found.len();
                if n > 0 {
                    let at = (self.palette.chosen as isize + by).rem_euclid(n as isize) as usize;
                    self.palette.chosen = at;
                    self.palette
                        .scroll
                        .scroll_to_item(at, ScrollStrategy::Nearest);
                }
            }
            PaletteAction::Back => self.palette_mode(Mode::All, w, cx),
        }
        cx.notify();
    }

    /// Opens the palette over the window, listing everything.
    fn open_palette(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        // Not over a dialog (Settings, a confirmation), the lock screen or
        // the first account's setup.
        if self.onboarding || self.lock.locked || self.modal.is_some() {
            return;
        }
        self.menu = None;
        self.palette.open = true;
        self.palette.anim += 1;
        self.palette.target = self.palette_target();
        self.palette_mode(Mode::All, w, cx);
    }

    pub(super) fn close_palette(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.palette.open {
            return;
        }
        self.palette.open = false;
        self.palette.items.clear();
        self.palette.found.clear();
        self.palette.target = None;
        self.focus.focus(w, cx);
        cx.notify();
    }

    /// Escape: back from a quick action's choices, else closed. Whether
    /// the palette was open to take it.
    pub(super) fn palette_escape(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.palette.open {
            return false;
        }
        if self.palette.mode == Mode::All {
            self.close_palette(w, cx);
        } else {
            self.palette_mode(Mode::All, w, cx);
        }
        true
    }

    /// Lists `mode`'s items, with the field emptied.
    fn palette_mode(&mut self, mode: Mode, w: &mut Window, cx: &mut Context<Self>) {
        self.palette.mode = mode;
        self.palette.items = self.palette_items(mode, cx);
        self.palette.input.update(cx, |s, cx| {
            s.set_placeholder(
                match mode {
                    Mode::All => t!("palette-type-a-command-or-search"),
                    Mode::MoveTo => t!("palette-find-a-folder"),
                    Mode::Snooze => t!("palette-snooze-until-when"),
                    Mode::Label => t!("palette-find-a-label"),
                },
                w,
                cx,
            );
            s.set_text("", w, cx);
            s.focus(w, cx);
        });
        self.refilter_palette(cx);
    }

    /// The field changed (typing) or was entered (Enter).
    pub(super) fn palette_input(
        &mut self,
        event: &InputEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.palette.open {
            return;
        }
        match event {
            InputEvent::Change => {
                self.refilter_palette(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let row = self.palette.chosen;
                self.handle(Action::Palette(PaletteAction::Choose(row)), w, cx);
            }
            _ => {}
        }
    }

    /// The results for what's typed now, the best highlighted.
    fn refilter_palette(&mut self, cx: &App) {
        let query = Self::value(&self.palette.input, cx);
        self.palette.found = rank(&self.palette.items, &query);
        self.palette.chosen = 0;
        self.palette.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// The messages quick actions act on: those selected, else the open one.
    fn palette_target(&self) -> Option<MoveTarget> {
        if self.selecting && !self.selection.is_empty() {
            Some(MoveTarget::Selection)
        } else {
            let id = self.selected.clone()?;
            self.messages
                .iter()
                .any(|m| m.id == id)
                .then(|| MoveTarget::Mail(vec![id]))
        }
    }

    /// An action on the palette's messages: on the selection as chosen
    /// when it opened, or on those messages.
    fn on_target(target: &MoveTarget, ids: &[String], change: Change) -> Action {
        match target {
            MoveTarget::Selection => Action::ApplySelection(change),
            _ => Action::Apply(ids.to_vec(), change),
        }
    }

    /// The messages Snooze until… acts on: those that can be snoozed, as
    /// the message menu offers it (not drafts or sent mail).
    fn palette_snoozable(&self, target: &MoveTarget) -> Vec<String> {
        let ids: HashSet<String> = self
            .snoozable(self.target_ids(target))
            .into_iter()
            .collect();
        self.messages
            .iter()
            .filter(|m| ids.contains(&m.id) && !matches!(m.folder, Folder::Drafts | Folder::Sent))
            .map(|m| m.id.clone())
            .collect()
    }

    /// Runs the result in `row`.
    fn choose_in_palette(&mut self, row: usize, w: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.palette.found.get(row) else {
            return;
        };
        let index = found.item;
        // A quick action's choices replace the list; anything else is done
        // with the palette closed, focus back on the window.
        if let Run::Mode(mode) = self.palette.items[index].run {
            self.palette_mode(mode, w, cx);
            return;
        }
        let target = self.palette.target.clone();
        let item = self.palette.items.swap_remove(index);
        self.close_palette(w, cx);
        match item.run {
            Run::Action(action) => self.handle(*action, w, cx),
            // To the window, as its shortcut would have been, once this
            // update is done (its handler updates the window again).
            Run::Command(command) => {
                let focus = self.focus.clone();
                let window = w.window_handle();
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, w, cx| {
                        focus.dispatch_action(command.as_ref(), w, cx)
                    });
                });
            }
            Run::Search(query) => self.search_everywhere(query, w, cx),
            Run::Template(i) => self.compose_from_template(i, w, cx),
            Run::Snooze(until) => {
                if let Some(target) = target {
                    let ids = self.palette_snoozable(&target);
                    if !ids.is_empty() {
                        self.snooze(ids, until, cx);
                    }
                }
            }
            Run::Mode(_) => {}
        }
    }

    /// Searches every account's mail on this computer, as a saved search does.
    fn search_everywhere(&mut self, query: String, w: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open {
            self.search_open = true;
            self.search_anim += 1;
        }
        Self::set(&self.search, query, w, cx);
        self.org_action(organize::OrgAction::SearchEverywhere, w, cx);
    }

    /// A new message, written from a template.
    fn compose_from_template(&mut self, i: usize, w: &mut Window, cx: &mut Context<Self>) {
        let Some(template) = self.compose_options.templates.get(i).cloned() else {
            return;
        };
        let before = self.composers.len();
        self.new_compose(0, w, cx);
        if self.composers.len() == before {
            return;
        }
        // The composer just opened is the newest, maybe in its own window.
        let Some(composer) = self.composers.last().cloned() else {
            return;
        };
        match composer.read(cx).window {
            Some(handle) => {
                let _ = handle.update(cx, |_, w, cx| {
                    composer.update(cx, |c, cx| c.insert_template(&template, w, cx))
                });
            }
            None => composer.update(cx, |c, cx| c.insert_template(&template, w, cx)),
        }
    }

    /// What `mode` lists, in the order shown before anything is typed.
    pub(super) fn palette_items(&self, mode: Mode, cx: &App) -> Vec<Item> {
        match mode {
            Mode::All => self.everything(cx),
            Mode::MoveTo => self.move_choices(),
            Mode::Snooze => self.snooze_choices(),
            Mode::Label => self.label_choices(),
        }
    }

    fn everything(&self, cx: &App) -> Vec<Item> {
        let mut items = Vec::new();
        // Quick actions, when there are messages to act on.
        if let Some(target) = &self.palette.target {
            let ids = self.target_ids(target);
            let n = ids.len();
            let detail = if n == 1 {
                self.messages
                    .iter()
                    .find(|m| m.id == ids[0])
                    .map(|m| m.subject.clone())
                    .unwrap_or_default()
            } else {
                t!("palette-messages", count = n).to_owned()
            };
            if !self.move_choices().is_empty() {
                items.push(
                    Item::new(
                        Kind::Quick,
                        t!("palette-move-to"),
                        "folder",
                        Run::Mode(Mode::MoveTo),
                    )
                    .detail(detail.clone())
                    .keywords(t!("palette-move-to-keywords")),
                );
            }
            if !self.palette_snoozable(target).is_empty() {
                items.push(
                    Item::new(
                        Kind::Quick,
                        t!("palette-snooze-until"),
                        "snooze",
                        Run::Mode(Mode::Snooze),
                    )
                    .detail(detail.clone())
                    .keywords(t!("palette-snooze-keywords")),
                );
            }
            if self.label_account(target).is_some() {
                items.push(
                    Item::new(
                        Kind::Quick,
                        t!("palette-apply-label"),
                        "tag",
                        Run::Mode(Mode::Label),
                    )
                    .detail(detail)
                    .keywords(t!("palette-label-keywords")),
                );
            }
        }
        let accounts = self.workspace_accounts();
        let several = accounts.len() > 1;
        // Accounts, each opening its inbox.
        if self.shows_all_mail() {
            items.push(Item::new(
                Kind::Account,
                t!("palette-all-accounts"),
                "inboxes",
                Run::action(Action::OpenMailbox(ALL_MAIL.into(), Folder::Inbox.into())),
            ));
        }
        for a in &accounts {
            let name = [&a.nickname, &a.name]
                .into_iter()
                .find(|n| !n.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| a.email.clone());
            items.push(
                Item::new(
                    Kind::Account,
                    name,
                    "user",
                    Run::action(Action::OpenMailbox(a.id.clone(), Folder::Inbox.into())),
                )
                .detail(a.email.clone()),
            );
        }
        for (i, s) in self.org.saved_searches.iter().enumerate() {
            items.push(
                Item::new(
                    Kind::SavedSearch,
                    s.name.clone(),
                    "search",
                    Run::action(Action::Organize(organize::OrgAction::OpenSavedSearch(i))),
                )
                .detail(s.query.clone()),
            );
        }
        items.extend(self.commands(cx));
        // Folders: every account's, and All Accounts' standard ones.
        if self.shows_all_mail() {
            for folder in Folder::ALL {
                items.push(
                    Item::new(
                        Kind::Folder,
                        folder.label(),
                        folder_icon(folder),
                        Run::action(Action::OpenMailbox(ALL_MAIL.into(), folder.into())),
                    )
                    .detail(t!("palette-all-accounts")),
                );
            }
        }
        for a in &accounts {
            let detail: SharedString = if several {
                a.email.clone().into()
            } else {
                SharedString::default()
            };
            for folder in Folder::ALL {
                items.push(
                    Item::new(
                        Kind::Folder,
                        folder.label(),
                        folder_icon(folder),
                        Run::action(Action::OpenMailbox(a.id.clone(), folder.into())),
                    )
                    .detail(detail.clone())
                    .keywords(a.email.clone()),
                );
            }
            for f in self.ordered_folders(&a.id) {
                items.push(
                    Item::new(
                        Kind::Folder,
                        f.label.clone(),
                        "folder",
                        Run::action(Action::OpenMailbox(
                            a.id.clone(),
                            Mailbox::Remote(f.name.clone()),
                        )),
                    )
                    .detail(detail.clone())
                    .keywords(format!("{} {}", f.name, a.email)),
                );
            }
        }
        // Labels kept on messages (Gmail's labels are its folders, above),
        // each searching every account's mail for it.
        let mut labels: Vec<String> = accounts
            .iter()
            .filter(|a| !matches!(a.provider, Provider::Gmail | Provider::Pop3))
            .flat_map(|a| self.backend.store.labels(&a.id).unwrap_or_default())
            .filter(|l| !l.trim().is_empty() && !l.starts_with('$') && !l.starts_with('\\'))
            .collect();
        labels.sort_by_key(|l| l.to_lowercase());
        labels.dedup_by_key(|l| l.to_lowercase());
        for label in labels {
            let query = label_query(&label);
            items.push(Item::new(Kind::Label, label, "tag", Run::Search(query)));
        }
        for (i, template) in self.compose_options.templates.iter().enumerate() {
            items.push(
                Item::new(
                    Kind::Template,
                    template.name.clone(),
                    "template",
                    Run::Template(i),
                )
                .detail(template.subject.clone())
                .keywords(t!("palette-template-keywords")),
            );
        }
        for (name, email) in self.contacts().iter() {
            let item = if name.trim().is_empty() {
                Item::new(
                    Kind::Contact,
                    email.clone(),
                    "contact",
                    Run::action(Action::ComposeTo(email.clone())),
                )
            } else {
                Item::new(
                    Kind::Contact,
                    name.clone(),
                    "contact",
                    Run::action(Action::ComposeTo(email.clone())),
                )
                .detail(email.clone())
            };
            items.push(item);
        }
        items
    }

    /// The actions shortcuts can be given to that work in the window, with
    /// their shortcuts, and a few more without one.
    fn commands(&self, cx: &App) -> Vec<Item> {
        let mut items = Vec::new();
        for b in shortcuts::bindable() {
            // Escape closes the palette itself; the palette doesn't list itself.
            if matches!(b.name, "close" | "command-palette") {
                continue;
            }
            // Composer-only commands (formatting) need a composer focused.
            let Some((command, _)) = b.targets.iter().find(|(_, context)| *context == "Mail")
            else {
                continue;
            };
            let mut item = Item::new(
                Kind::Command,
                b.label,
                command_icon(b.name),
                Run::Command(command.boxed_clone()),
            )
            .detail(b.group.title())
            .keywords(b.name.replace('-', " "));
            item.keys = shortcuts::current(b.name, cx);
            items.push(item);
        }
        let more: [(&'static str, &'static str, Run); 11] = [
            (
                t!("palette-mark-all-as-read"),
                "mail-open",
                Run::Command(Box::new(MarkAllRead)),
            ),
            (
                t!("palette-show-unread-only"),
                "filter",
                Run::Command(Box::new(ShowUnread)),
            ),
            (
                t!("palette-show-starred-only"),
                "filter",
                Run::Command(Box::new(ShowStarred)),
            ),
            (
                t!("palette-show-messages-with-attachments"),
                "filter",
                Run::Command(Box::new(ShowAttachments)),
            ),
            (
                t!("palette-show-all-messages"),
                "filter",
                Run::Command(Box::new(ShowAllMail)),
            ),
            (
                t!("palette-save-as-pdf"),
                "download",
                Run::Command(Box::new(SaveAsPdf)),
            ),
            (
                t!("palette-open-calendar"),
                "calendar",
                Run::action(Action::Calendar(CalAction::Open(AppView::Calendar))),
            ),
            (
                t!("palette-open-contacts"),
                "people",
                Run::action(Action::Calendar(CalAction::Open(AppView::Contacts))),
            ),
            (
                t!("palette-open-tasks"),
                "tasks",
                Run::action(Action::Calendar(CalAction::Open(AppView::Tasks))),
            ),
            (
                t!("palette-add-account"),
                "plus",
                Run::Command(Box::new(AddAccount)),
            ),
            (
                t!("palette-about-picacho"),
                "info",
                Run::action(Action::About),
            ),
        ];
        items.extend(
            more.into_iter()
                .map(|(title, icon, run)| Item::new(Kind::Command, title, icon, run)),
        );
        items
    }

    /// Move to…: the standard folders the messages aren't all in, then
    /// their account's own server folders.
    fn move_choices(&self) -> Vec<Item> {
        let Some(target) = &self.palette.target else {
            return vec![];
        };
        let ids = self.target_ids(target);
        let chosen: HashSet<&String> = ids.iter().collect();
        let folders: Vec<Folder> = self
            .messages
            .iter()
            .filter(|m| chosen.contains(&m.id))
            .map(|m| m.folder)
            .collect();
        if folders.is_empty() {
            return vec![];
        }
        let mut items: Vec<Item> = [Folder::Inbox, Folder::Archive, Folder::Junk, Folder::Trash]
            .into_iter()
            .filter(|f| !folders.iter().all(|m| m == f))
            .map(|f| {
                Item::new(
                    Kind::Choice,
                    f.label(),
                    folder_icon(f),
                    Run::action(Self::on_target(target, &ids, Change::Move(f))),
                )
            })
            .collect();
        let account = self.move_account(target).map(|(a, _)| a);
        let email = account
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| &a.id == id))
            .map(|a| a.email.clone())
            .unwrap_or_default();
        for e in self.move_folder_entries(target) {
            if let Entry::Item { label, action, .. } = e {
                // The menu indents subfolders with em spaces.
                let label = label.trim_start_matches('\u{2003}').to_owned();
                items.push(
                    Item::new(Kind::Choice, label, "folder", Run::action(action))
                        .detail(email.clone()),
                );
            }
        }
        items
    }

    /// Snooze until…: the snooze dialog's choices, and the dialog itself
    /// for another time.
    fn snooze_choices(&self) -> Vec<Item> {
        let Some(target) = &self.palette.target else {
            return vec![];
        };
        let ids = self.palette_snoozable(target);
        if ids.is_empty() {
            return vec![];
        }
        let now = chrono::Local::now();
        let mut items: Vec<Item> = snooze_choices(now)
            .into_iter()
            .map(|(label, at)| {
                Item::new(Kind::Choice, label, "snooze", Run::Snooze(at))
                    .detail(describe_later(at, now))
            })
            .collect();
        items.push(Item::new(
            Kind::Choice,
            t!("palette-pick-a-date-and-time"),
            "calendar",
            Run::action(Action::SnoozeMenu(ids)),
        ));
        items
    }

    /// Apply label…: the label menu's labels, those the messages all have
    /// checked (choosing one takes it off), and a new one.
    fn label_choices(&self) -> Vec<Item> {
        let Some(target) = &self.palette.target else {
            return vec![];
        };
        self.label_entries(target)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item {
                    label,
                    icon,
                    action,
                    checked,
                    ..
                } => {
                    let mut item = Item::new(
                        Kind::Choice,
                        label,
                        if icon.is_empty() { "tag" } else { icon },
                        Run::action(action),
                    );
                    item.checked = checked;
                    Some(item)
                }
                Entry::Separator => None,
            })
            .collect()
    }

    /// The palette over the window, when it's open.
    pub(super) fn palette_layer(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.palette.open || self.modal.is_some() {
            return None;
        }
        let c = self.colors();
        let frame = window::content_size(window);
        let width = px(WIDTH).min((frame.width - px(32.)).max(px(0.)));
        let count = self.palette.found.len();
        let list_height = px(ROW * count.clamp(1, ROWS_SHOWN) as f32);
        let mode = self.palette.mode;
        let entity = cx.entity();
        let list = if count == 0 {
            div()
                .id("palette-empty")
                .debug_selector(|| "palette-empty".into())
                .h(list_height)
                .px_4()
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(t!("palette-nothing-matches"))
                .into_any_element()
        } else {
            let list = uniform_list("palette-results", count, move |range, _, cx| {
                entity.update(cx, |this, cx| {
                    range
                        .map(|row| this.palette_row(row, cx))
                        .collect::<Vec<_>>()
                })
            })
            .track_scroll(&self.palette.scroll)
            .w_full()
            .px_1()
            .h(list_height);
            // As wide as the panel: rows otherwise take their text's
            // width, past a narrow (phone) panel's edge.
            div()
                .id("palette-list")
                .w_full()
                .debug_selector(|| "palette-list".into())
                .role(Role::ListBox)
                .aria_label(t!("palette-results"))
                .h(list_height)
                .flex()
                .flex_col()
                .child(panes::scroll_frame(list, &self.palette.scroll))
                .into_any_element()
        };
        let back = (mode != Mode::All).then(|| {
            div()
                .id("palette-back")
                .debug_selector(|| "palette-back".into())
                .role(Role::Button)
                .aria_label(t!("palette-back-to-everything"))
                .flex_shrink_0()
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(6.))
                .bg(rgb(c.soft))
                .text_size(px(12.))
                .text_color(rgb(c.accent))
                .cursor_pointer()
                .child(icon_el("chevron-left", 12., c.accent))
                .child(match mode {
                    Mode::MoveTo => t!("palette-move-to"),
                    Mode::Snooze => t!("palette-snooze-until"),
                    Mode::Label | Mode::All => t!("palette-apply-label"),
                })
                .on_click(cx.listener(|this, _, w, cx| {
                    this.handle(Action::Palette(PaletteAction::Back), w, cx)
                }))
        });
        let hint = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .px_1()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(rgb(c.border))
                        .child(keys),
                )
                .child(what)
        };
        let panel = div()
            .id("palette")
            .debug_selector(|| "palette".into())
            .role(Role::Dialog)
            .aria_label(t!("palette-command-palette"))
            .occlude()
            .on_mouse_down(MouseButton::Left, swallow_press)
            .w(width)
            .flex()
            .flex_col()
            .rounded(px(14.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.surface))
            .shadow_2xl()
            .overflow_hidden()
            // The field's own up and down move its cursor; here they move
            // through the results, before the field sees them.
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveUp, w, cx| {
                    cx.stop_propagation();
                    this.handle(Action::Palette(PaletteAction::Step(-1)), w, cx);
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveDown, w, cx| {
                    cx.stop_propagation();
                    this.handle(Action::Palette(PaletteAction::Step(1)), w, cx);
                }),
            )
            // Backspace in an empty field leaves a quick action's choices.
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Backspace, w, cx| {
                    if this.palette.mode != Mode::All
                        && Self::value(&this.palette.input, cx).is_empty()
                    {
                        cx.stop_propagation();
                        this.handle(Action::Palette(PaletteAction::Back), w, cx);
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .children(back)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.palette.input)
                                .appearance(false)
                                .prefix(icon_el("search", 15., c.muted))
                                .h(px(36.))
                                .text_size(px(15.)),
                        ),
                    ),
            )
            .child(div().w_full().py_1().child(list))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .child(hint("↑↓", t!("palette-to-move")))
                    .child(hint(
                        if cfg!(target_os = "macos") {
                            "↩"
                        } else {
                            t!("shortcuts-enter")
                        },
                        t!("palette-to-choose"),
                    ))
                    .child(hint(
                        t!("shortcuts-esc"),
                        if mode == Mode::All {
                            t!("palette-to-close")
                        } else {
                            t!("palette-to-go-back")
                        },
                    )),
            );
        let corners = window::window_corners(window);
        Some(
            div()
                .id("palette-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .rounded_tl(corners.top_left)
                .rounded_tr(corners.top_right)
                .rounded_bl(corners.bottom_left)
                .rounded_br(corners.bottom_right)
                .bg(rgba(0x101c2c40))
                .flex()
                .flex_col()
                .items_center()
                .pt(px(TITLEBAR_HEIGHT + 44.).min(frame.height / 6.))
                .px_4()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| {
                        cx.stop_propagation();
                        this.handle(Action::Palette(PaletteAction::Close), w, cx)
                    }),
                )
                .child(
                    panel.with_animation(
                        ("palette-enter", self.palette.anim),
                        Animation::new(Duration::from_millis(140))
                            .with_easing(|t| 1. - (1. - t).powi(3)),
                        |d, t| d.opacity(t).mt(px((1. - t) * -8.)),
                    ),
                )
                .into_any_element(),
        )
    }

    /// One result's row.
    fn palette_row(&self, row: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(found) = self.palette.found.get(row) else {
            return div().into_any_element();
        };
        let item = &self.palette.items[found.item];
        let chosen = row == self.palette.chosen;
        let marked = |text: &SharedString, hits: &[usize], color: u32| {
            StyledText::new(text.clone()).with_highlights(hit_ranges(text, hits).into_iter().map(
                |r| {
                    (
                        r,
                        HighlightStyle {
                            color: Some(rgb(color).into()),
                            font_weight: Some(FontWeight::BOLD),
                            ..Default::default()
                        },
                    )
                },
            ))
        };
        let right = if let Some(keys) = &item.keys {
            div()
                .flex_shrink_0()
                .px_1()
                .rounded(px(4.))
                .border_1()
                .border_color(rgb(c.border))
                .text_size(px(11.))
                .text_color(rgb(c.muted))
                .child(keys.clone())
                .into_any_element()
        } else if item.checked {
            icon_el("check", 14., c.accent).into_any_element()
        } else {
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(c.muted))
                .child(item.kind.label())
                .into_any_element()
        };
        let label = if item.detail.is_empty() {
            item.title.clone()
        } else {
            format!("{} · {}", item.title, item.detail).into()
        };
        div()
            .id(("palette-row", row))
            .debug_selector(move || format!("palette-row-{row}"))
            .role(Role::ListBoxOption)
            .aria_label(label)
            .aria_selected(chosen)
            // Laid out on its own by the list (`layout_as_root`): without a
            // width it takes its whole title's, past a narrow panel's edge.
            .w_full()
            .h(px(ROW))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .when(chosen, |d| d.bg(rgb(c.soft)))
            .when(!chosen, |d| d.hover(move |s| s.bg(rgb(c.raised))))
            .child(
                div()
                    .size(px(18.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .when(!item.icon.is_empty(), |d| {
                        d.child(icon_el(
                            item.icon,
                            15.,
                            if chosen { c.accent } else { c.muted },
                        ))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w_0()
                            .max_w(relative(0.7))
                            .truncate()
                            .child(marked(&item.title, &found.title_hits, c.accent)),
                    )
                    .when(!item.detail.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(marked(&item.detail, &found.detail_hits, c.text)),
                        )
                    }),
            )
            .child(right)
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::Palette(PaletteAction::Choose(row)), w, cx)
            }))
            .into_any_element()
    }
}

/// A search for mail with `label`, quoted when it has spaces.
fn label_query(label: &str) -> String {
    if label.chars().any(char::is_whitespace) && !label.contains('"') {
        format!("label:\"{label}\"")
    } else {
        format!("label:{label}")
    }
}

/// An icon for a command, by its name in `shortcuts.txt`.
fn command_icon(name: &str) -> &'static str {
    match name {
        "reply" => "reply",
        "reply-all" => "reply-all",
        "forward" => "forward",
        "archive" | "go-archive" => "archive",
        "delete" | "go-trash" => "trash",
        "move-to-spam" | "go-spam" => "spam",
        "toggle-read" => "mail-open",
        "toggle-star" | "go-starred" => "star",
        "open-in-window" => "popout",
        "find-in-message" | "search-mailbox" => "search",
        "print" => "print",
        "toggle-contact-panel" => "contact",
        "refresh" => "refresh",
        "select-all" => "select",
        "attachments" => "paperclip",
        "group-by-conversation" => "thread",
        "go-inbox" => "inbox",
        "go-sent" | "send" => "send",
        "go-drafts" | "new-message" => "compose",
        "go-all-mail" => "all-mail",
        "next-workspace" | "previous-workspace" => "workspace",
        "settings" => "settings",
        "lock" => "lock",
        "toggle-dark-mode" => "monitor",
        _ => "keyboard",
    }
}

/// A character as matching sees it: lowercase, without accents.
fn fold(c: char) -> char {
    let base = std::iter::once(c).nfd().next().unwrap_or(c);
    base.to_lowercase().next().unwrap_or(base)
}

/// A text as matching sees it: each character folded, and whether a word
/// starts there (after a space or punctuation, or a capital after a
/// lowercase letter).
fn prepared(text: &str) -> Vec<(char, bool)> {
    let mut out = Vec::with_capacity(text.len());
    let mut before: Option<char> = None;
    for ch in text.chars() {
        let start = match before {
            None => true,
            Some(b) => {
                (!b.is_alphanumeric() && ch.is_alphanumeric())
                    || (b.is_lowercase() && ch.is_uppercase())
            }
        };
        out.push((fold(ch), start));
        before = Some(ch);
    }
    out
}

/// How well one word of a query (folded) matches a text: its letters in
/// order, scored by how many start words or follow one another, less the
/// gaps between them. The text's characters it matched, too.
fn word_score(word: &[char], text: &[(char, bool)]) -> Option<(i32, Vec<usize>)> {
    let first = *word.first()?;
    let mut best: Option<(i32, Vec<usize>)> = None;
    for start in (0..text.len()).filter(|&i| text[i].0 == first) {
        let mut hits = vec![start];
        let mut at = start + 1;
        for &ch in &word[1..] {
            // The next of it at the start of a word, if the next letter
            // along isn't it: "mtf" is Move To Folder's initials.
            let next = (at..text.len()).find(|&i| text[i].0 == ch);
            let Some(mut next) = next else {
                hits.clear();
                break;
            };
            if next != at
                && let Some(word_start) = (next..text.len()).find(|&i| text[i].0 == ch && text[i].1)
            {
                next = word_start;
            }
            hits.push(next);
            at = next + 1;
        }
        if hits.is_empty() {
            // The rest of the word isn't after this start, nor after any later one.
            break;
        }
        let mut score = 0;
        for (i, &h) in hits.iter().enumerate() {
            score += 1;
            if text[h].1 {
                score += 8;
            }
            if i > 0 {
                let gap = h - hits[i - 1] - 1;
                if gap == 0 {
                    score += 6;
                } else {
                    score -= gap.min(6) as i32;
                }
            }
        }
        score -= (start as i32).min(8) / 2;
        // The whole text, or how it starts.
        if start == 0 && hits.windows(2).all(|w| w[1] == w[0] + 1) {
            score += if hits.len() == text.len() { 20 } else { 10 };
        }
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, hits));
        }
    }
    best
}

/// How well `query` matches an item: every word of it in the title, the
/// detail or the keywords (the title counting most), or None.
pub(super) fn fuzzy(
    query: &str,
    title: &str,
    detail: &str,
    keywords: &str,
) -> Option<(i32, Vec<usize>, Vec<usize>)> {
    let title_text = prepared(title);
    let detail_text = prepared(detail);
    let keyword_text = prepared(keywords);
    let mut total = 0;
    let mut title_hits = Vec::new();
    let mut detail_hits = Vec::new();
    for word in query.split_whitespace() {
        let word: Vec<char> = word.chars().map(fold).collect();
        let in_title = word_score(&word, &title_text).map(|(s, h)| (s + 6, h));
        let in_detail = word_score(&word, &detail_text);
        let in_keywords = word_score(&word, &keyword_text).map(|(s, _)| s - 2);
        let title_score = in_title.as_ref().map(|(s, _)| *s);
        let detail_score = in_detail.as_ref().map(|(s, _)| *s);
        let best = [title_score, detail_score, in_keywords]
            .into_iter()
            .flatten()
            .max()?;
        total += best;
        if title_score == Some(best) {
            title_hits.extend(in_title.map(|(_, h)| h).unwrap_or_default());
        } else if detail_score == Some(best) {
            detail_hits.extend(in_detail.map(|(_, h)| h).unwrap_or_default());
        }
    }
    // Shorter titles first among equals: "Inbox" before "Inbox Rules".
    total -= (title_text.len() as i32) / 12;
    title_hits.sort_unstable();
    title_hits.dedup();
    detail_hits.sort_unstable();
    detail_hits.dedup();
    Some((total, title_hits, detail_hits))
}

/// The items matching `query`, best first; before anything is typed,
/// everything but contacts (there may be thousands), in its own order.
pub(super) fn rank(items: &[Item], query: &str) -> Vec<Found> {
    if query.trim().is_empty() {
        return items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.kind != Kind::Contact)
            .map(|(item, _)| Found {
                item,
                score: 0,
                title_hits: vec![],
                detail_hits: vec![],
            })
            .collect();
    }
    let mut found: Vec<Found> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let (score, title_hits, detail_hits) =
                fuzzy(query, &item.title, &item.detail, &item.keywords)?;
            Some(Found {
                item: i,
                score,
                title_hits,
                detail_hits,
            })
        })
        .collect();
    // Among equals, the order they're listed in: quick actions and
    // commands before folders, folders before contacts.
    found.sort_by(|a, b| b.score.cmp(&a.score).then(a.item.cmp(&b.item)));
    found
}

/// The byte ranges of `text`'s characters at `hits` (character indices),
/// joined where they follow one another.
fn hit_ranges(text: &str, hits: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let mut wanted = hits.iter().peekable();
    for (i, (at, ch)) in text.char_indices().enumerate() {
        while wanted.peek().is_some_and(|&&h| h < i) {
            wanted.next();
        }
        if wanted.peek() == Some(&&i) {
            let end = at + ch.len_utf8();
            match out.last_mut() {
                Some(last) if last.end == at => last.end = end,
                _ => out.push(at..end),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    fn item(kind: Kind, title: &str, detail: &str) -> Item {
        Item::new(kind, title.to_owned(), "", Run::Search(String::new())).detail(detail.to_owned())
    }
    fn titles(items: &[Item], query: &str) -> Vec<String> {
        rank(items, query)
            .iter()
            .map(|f| items[f.item].title.to_string())
            .collect()
    }

    #[test]
    fn letters_in_order_match_and_word_starts_count_most() {
        assert!(fuzzy("inb", "Inbox", "", "").is_some());
        assert!(fuzzy("ibx", "Inbox", "", "").is_some());
        assert!(fuzzy("xbi", "Inbox", "", "").is_none());
        // Initials beat letters inside a word.
        let initials = fuzzy("mtf", "Move to Folder", "", "").unwrap().0;
        let inside = fuzzy("mtf", "Smtp Info", "", "")
            .map(|f| f.0)
            .unwrap_or(i32::MIN);
        assert!(initials > inside);
        // "arch" is Archive's start, not the middle of Search.
        let (_, hits, _) = fuzzy("arch", "Search Archive", "", "").unwrap();
        assert_eq!(hits, vec![7, 8, 9, 10]);
    }

    #[test]
    fn matching_ignores_case_and_accents() {
        assert!(fuzzy("jose", "José Álvarez", "", "").is_some());
        assert!(fuzzy("ALV", "José Álvarez", "", "").is_some());
        assert!(fuzzy("bjork", "Björk", "", "").is_some());
    }

    #[test]
    fn every_word_must_match_somewhere() {
        // "sent work": the folder in the title, its account in the detail.
        let (_, title, detail) = fuzzy("sent work", "Sent", "work@example.com", "").unwrap();
        assert_eq!(title, vec![0, 1, 2, 3]);
        assert_eq!(detail, vec![0, 1, 2, 3]);
        assert!(fuzzy("sent play", "Sent", "work@example.com", "").is_none());
        // Keywords find it without being shown.
        let (_, title, detail) = fuzzy("delete", "Move to Trash", "", "delete").unwrap();
        assert!(title.is_empty() && detail.is_empty());
    }

    #[test]
    fn the_best_match_comes_first() {
        let items = vec![
            item(Kind::Command, "Reply All", ""),
            item(Kind::Command, "Reply", ""),
            item(Kind::Folder, "Receipts", ""),
            item(Kind::Contact, "Rey Palmer", "rey@example.com"),
        ];
        assert_eq!(titles(&items, "reply"), vec!["Reply", "Reply All"]);
        assert_eq!(titles(&items, "rec")[0], "Receipts");
        // Contacts only once something is typed.
        assert!(!titles(&items, "").contains(&"Rey Palmer".to_owned()));
        assert_eq!(
            titles(&items, "")[0],
            "Reply All",
            "untyped, in their own order"
        );
        assert!(titles(&items, "rey").contains(&"Rey Palmer".to_owned()));
        // Equals keep their order: quick actions and commands first.
        let items = vec![
            item(Kind::Command, "Archive", ""),
            item(Kind::Folder, "Archive", ""),
        ];
        let found = rank(&items, "archive");
        assert_eq!(found.iter().map(|f| f.item).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn matched_characters_become_byte_ranges() {
        assert_eq!(hit_ranges("Inbox", &[0, 1, 2]), vec![0..3]);
        assert_eq!(
            hit_ranges("Move to Folder", &[0, 5, 8]),
            vec![0..1, 5..6, 8..9]
        );
        // "é" is two bytes.
        assert_eq!(hit_ranges("José", &[2, 3]), vec![2..5]);
        assert!(hit_ranges("", &[0]).is_empty());
    }

    #[test]
    fn labels_with_spaces_are_quoted() {
        assert_eq!(label_query("Urgent"), "label:Urgent");
        assert_eq!(label_query("To do"), "label:\"To do\"");
    }
}
