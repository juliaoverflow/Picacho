//! Organizing mail on this computer (see `manage::organize`): the list
//! grouped by conversation, pinned messages and muted conversations, saved
//! searches in the sidebar, blocked senders and rules applied here, list
//! density and where the reader goes; and Settings → Rules & Layout.
use super::*;
use crate::manage::categories::Category;
use crate::manage::organize::{
    Condition, Density, ReaderLayout, Rule, RuleAction, RuleField, RuleOp, SavedSearch,
    conversation_key, mail_key,
};

/// Messages dragged from the list to a sidebar folder.
#[derive(Clone)]
pub(super) struct MailDrag {
    pub(super) ids: Vec<String>,
    pub(super) label: String,
}
/// What follows the pointer while messages are dragged.
pub(super) struct MailDragPreview {
    pub(super) label: SharedString,
    pub(super) c: Colors,
    pub(super) offset: Point<Pixels>,
}
impl Render for MailDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = self.c;
        div()
            .pl(px((f32::from(self.offset.x) - 12.).max(0.)))
            .pt(px((f32::from(self.offset.y) - 10.).max(0.)))
            .child(
                div()
                    .h(px(28.))
                    .max_w(px(260.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(8.))
                    .bg(rgb(c.raised))
                    .border_1()
                    .border_color(rgb(c.border))
                    .shadow_md()
                    .text_size(px(12.))
                    .text_color(rgb(c.text))
                    .child(icon_el("mail", 14., c.accent))
                    .child(div().min_w_0().truncate().child(self.label.clone())),
            )
    }
}

/// Search All Mail on This Computer's results: every account's mail on
/// this computer that matches, listed on their own, and where they were
/// opened from, which Done goes back to.
pub(super) struct LocalResults {
    pub(super) back: Option<(String, Mailbox)>,
}

/// The rule action the form adds, before its folder or label is typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RuleKind {
    #[default]
    MarkRead,
    Star,
    Archive,
    Spam,
    Trash,
    MoveTo,
    Label,
    Silence,
}
impl RuleKind {
    const ALL: [Self; 8] = [
        Self::MarkRead,
        Self::Star,
        Self::Archive,
        Self::Spam,
        Self::Trash,
        Self::MoveTo,
        Self::Label,
        Self::Silence,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::MarkRead => t!("organize-mark-read"),
            Self::Star => t!("organize-star"),
            Self::Archive => t!("organize-archive"),
            Self::Spam => t!("organize-spam"),
            Self::Trash => t!("organize-trash"),
            Self::MoveTo => t!("organize-move-to-folder"),
            Self::Label => t!("organize-label"),
            Self::Silence => t!("organize-dont-notify"),
        }
    }
    fn named(self) -> bool {
        matches!(self, Self::MoveTo | Self::Label)
    }
    fn action(self, name: &str) -> Option<RuleAction> {
        Some(match self {
            Self::MarkRead => RuleAction::MarkRead,
            Self::Star => RuleAction::Star,
            Self::Archive => RuleAction::Archive,
            Self::Spam => RuleAction::Spam,
            Self::Trash => RuleAction::Trash,
            Self::MoveTo if !name.is_empty() => RuleAction::MoveTo(name.to_owned()),
            Self::Label if !name.is_empty() => RuleAction::Label(name.to_owned()),
            Self::Silence => RuleAction::Silence,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OrgAction {
    ToggleThreaded,
    /// Pins or unpins a message (its id in the list).
    TogglePin(String),
    /// Mutes or unmutes a message's conversation (its id in the list).
    ToggleMute(String),
    /// Blocks or unblocks a message's sender (its id in the list).
    BlockSender(String),
    Unblock(String),
    /// Blocks the address or @domain typed in Settings.
    AddBlocked,
    /// Keeps what is typed in the search as a saved search.
    SaveSearch,
    /// Searches every account's mail on this computer for what is typed.
    SearchEverywhere,
    /// Leaves those results for where they were opened from.
    CloseLocalResults,
    OpenSavedSearch(usize),
    RemoveSavedSearch(usize),
    SetDensity(Density),
    SetLayout(ReaderLayout),
    RuleField(RuleField),
    RuleOp(RuleOp),
    /// The condition being added holds when its test doesn't.
    RuleNegate,
    /// The category a Category condition names.
    RuleCategory(Category),
    RuleKind(RuleKind),
    /// Adds the condition typed to the rule being put together.
    AddCondition,
    RemoveCondition(usize),
    /// Adds the action chosen to the rule being put together.
    AddAction,
    RemoveAction(usize),
    /// Any condition is enough (OR), or all must hold (AND).
    RuleAny(bool),
    /// No later rule applies to mail the rule matched.
    RuleStop,
    /// Keeps the rule put together (a new one, or the one edited).
    SaveRule,
    /// Puts a rule back in the form, to change it.
    EditRule(usize),
    /// Leaves the rule put together.
    CancelRule,
    /// Moves a rule one place earlier.
    RaiseRule(usize),
    ToggleRule(usize),
    RemoveRule(usize),
    /// Applies blocked senders and rules to mail in the inboxes now.
    ApplyRulesNow,
}

/// Settings' fields for rules and blocked senders.
pub(super) struct OrgUi {
    pub(super) rule_text: Entity<InputState>,
    pub(super) rule_header: Entity<InputState>,
    pub(super) rule_name: Entity<InputState>,
    pub(super) block: Entity<InputState>,
    pub(super) field: RuleField,
    pub(super) op: RuleOp,
    pub(super) negate: bool,
    pub(super) category: Category,
    pub(super) kind: RuleKind,
    /// The rule being put together: its conditions and actions so far.
    pub(super) draft: Rule,
    /// Where the rule being changed was, when one is.
    pub(super) editing: Option<usize>,
}
impl OrgUi {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            rule_text: input(t!("organize-text-to-look-for-as-newsletter"), false, window, cx),
            rule_header: input(t!("organize-header-name"), false, window, cx),
            rule_name: input(t!("organize-folder-or-label-name"), false, window, cx),
            block: input(t!("organize-address-or-domain-to-block"), false, window, cx),
            field: RuleField::default(),
            op: RuleOp::default(),
            negate: false,
            category: Category::Promotions,
            kind: RuleKind::default(),
            draft: Rule { enabled: true, ..Rule::default() },
            editing: None,
        }
    }
}

impl MailApp {
    fn save_org(&mut self, cx: &mut Context<Self>) {
        self.org.save(&self.backend.store);
        cx.notify();
    }

    /// The list's rows once filtered: grouped by conversation when asked
    /// (outside Drafts), and pinned messages first.
    pub(super) fn organize_rows(&mut self) {
        self.thread_counts.clear();
        if self.org.threaded && self.folder != Folder::Drafts && self.visible.len() > 1 {
            let (kept, counts) = thread::group(&self.messages, &self.visible);
            self.visible = kept;
            self.thread_counts = counts;
        }
        if !self.org.pinned.is_empty() {
            let messages = &self.messages;
            let org = &self.org;
            // Stable: each part keeps the list's order.
            let (pinned, rest): (Vec<usize>, Vec<usize>) = self
                .visible
                .iter()
                .partition(|&&i| org.is_pinned(&messages[i]));
            if !pinned.is_empty() {
                self.visible = pinned.into_iter().chain(rest).collect();
            }
        }
    }

    /// Acts on messages dropped on a sidebar mailbox. Into one of their
    /// own account's mailboxes they move (onto Starred, they're starred);
    /// onto All Mail, those in the inbox, Spam or Trash are archived, the
    /// rest being there already; onto Drafts, each starts a new draft and
    /// stays. Onto another account's mailbox they move to that account
    /// (`Backend::move_to_account`). The unified mailboxes take each
    /// message into its own account's.
    pub(super) fn drop_mail(&mut self, account: &str, mailbox: &Mailbox, d: &MailDrag, cx: &mut Context<Self>) {
        // A set: a drag of thousands, checked against every listed message.
        let ids: HashSet<&String> = d.ids.iter().collect();
        let dragged: Vec<Mail> = self
            .messages
            .iter()
            .filter(|m| ids.contains(&m.id))
            .cloned()
            .collect();
        if dragged.is_empty() {
            return;
        }
        if *mailbox == Mailbox::Standard(Folder::Drafts) {
            self.drop_as_drafts(account, dragged, cx);
            return;
        }
        let (own, others): (Vec<Mail>, Vec<Mail>) = dragged
            .into_iter()
            .partition(|m| m.account == account || account == ALL_MAIL);
        if !others.is_empty() {
            self.move_to_account(account, mailbox, others, cx);
        }
        if own.is_empty() {
            return;
        }
        let (ids, change): (Vec<String>, Change) = match mailbox {
            Mailbox::Remote(name) => (own.iter().map(|m| m.id.clone()).collect(), Change::MoveTo(name.clone())),
            Mailbox::Standard(Folder::Starred) => (own.iter().map(|m| m.id.clone()).collect(), Change::Star(true)),
            // All Mail holds everything but Spam, Trash (and the inbox, on
            // Gmail, whose All Mail is its archive): what's elsewhere is
            // archived.
            Mailbox::Standard(Folder::All) => {
                let ids: Vec<String> = own
                    .iter()
                    .filter(|m| matches!(m.folder, Folder::Inbox | Folder::Junk | Folder::Trash))
                    .map(|m| m.id.clone())
                    .collect();
                if ids.is_empty() {
                    self.notice(t!("organize-already-in-all-mail"), false, cx);
                    return;
                }
                (ids, Change::Move(Folder::Archive))
            }
            Mailbox::Standard(Folder::Drafts | Folder::Other) => return,
            Mailbox::Standard(folder) => (own.iter().map(|m| m.id.clone()).collect(), Change::Move(*folder)),
        };
        // Already there: nothing to do.
        if let Change::Move(f) = &change
            && account != ALL_MAIL
            && self.account.as_deref() == Some(account)
            && self.folder == *f
        {
            return;
        }
        self.selection.retain(|s| !ids.contains(s));
        self.apply(ids, change, cx);
    }

    /// Messages dropped on Drafts: each starts a new draft (in the
    /// account dropped on, or its own on the unified Drafts), to send again
    /// or change; the messages stay where they are.
    fn drop_as_drafts(&mut self, account: &str, dragged: Vec<Mail>, cx: &mut Context<Self>) {
        let mut groups: Vec<(Account, Account, Vec<Mail>)> = Vec::new();
        for m in dragged {
            let Some(from) = self.account_of(&m) else { continue };
            let to_id = if account == ALL_MAIL { m.account.clone() } else { account.to_owned() };
            let Some(to) = self.accounts.iter().find(|a| a.id == to_id).cloned() else {
                continue;
            };
            match groups.iter_mut().find(|(f, t, _)| f.id == from.id && t.id == to.id) {
                Some((_, _, mails)) => mails.push(m),
                None => groups.push((from, to, vec![m])),
            }
        }
        if groups.is_empty() {
            return;
        }
        self.task(
            cx,
            move |b| {
                let mut made = 0;
                for (from, to, mails) in &groups {
                    made += b.copy_as_drafts(from, mails, to)?.len();
                }
                Ok(made)
            },
            |this, result: anyhow::Result<usize>, cx| match result {
                Ok(n) => {
                    this.notice(t!("organize-started-in-drafts", n = n), false, cx);
                    if this.folder == Folder::Drafts {
                        this.refresh(cx);
                    }
                    this.refresh_counts(cx);
                }
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// Messages dropped on another account's mailbox: moved there, each
    /// account's in one go. They leave the list once moved.
    fn move_to_account(&mut self, account: &str, mailbox: &Mailbox, mails: Vec<Mail>, cx: &mut Context<Self>) {
        let Some(to) = self.accounts.iter().find(|a| a.id == account).cloned() else {
            return;
        };
        let mut groups: Vec<(Account, Vec<Mail>)> = Vec::new();
        for m in mails {
            let Some(from) = self.account_of(&m) else { continue };
            match groups.iter_mut().find(|(f, _)| f.id == from.id) {
                Some((_, list)) => list.push(m),
                None => groups.push((from, vec![m])),
            }
        }
        if groups.is_empty() {
            return;
        }
        let name = if to.name.is_empty() { to.email.clone() } else { to.name.clone() };
        let n: usize = groups.iter().map(|(_, l)| l.len()).sum();
        self.notice(
            t!("organize-moving-to", n = n, name = name),
            false,
            cx,
        );
        let mailbox = mailbox.clone();
        self.task(
            cx,
            move |b| {
                let mut total = crate::manage::backend::archive::Moved::default();
                let mut gone = Vec::new();
                for (from, mails) in &groups {
                    let moved = b.move_to_account(from, mails, &to, &mailbox)?;
                    total.done += moved.done;
                    total.skipped += moved.skipped;
                    if moved.skipped == 0 {
                        gone.extend(mails.iter().map(|m| m.id.clone()));
                    }
                }
                Ok((total, gone))
            },
            move |this, result: anyhow::Result<(crate::manage::backend::archive::Moved, Vec<String>)>, cx| match result {
                Ok((moved, gone)) => {
                    this.notice(moved.notice(crate::manage::backend::archive::Done::MovedTo(&name)), false, cx);
                    this.selection.retain(|s| !gone.contains(s));
                    this.messages.retain(|m| !gone.contains(&m.id));
                    if this.selected.as_ref().is_some_and(|s| gone.contains(s)) {
                        this.selected = None;
                    }
                    this.filter(cx);
                    this.reload_cached(cx);
                    this.refresh_counts(cx);
                }
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// The height of a message list row.
    pub(super) fn row_height(&self) -> f32 {
        match self.org.density {
            Density::Comfortable => ROW_HEIGHT,
            Density::Compact => COMPACT_ROW_HEIGHT,
        }
    }

    /// Small marks a row shows after its subject: pinned, muted, and how
    /// many messages its conversation holds.
    pub(super) fn row_marks(&self, index: usize, m: &Mail) -> Div {
        let c = self.colors();
        let count = self.thread_counts.get(&index).copied().unwrap_or(0);
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .flex_shrink_0()
            .when(self.org.is_pinned(m), |d| d.child(icon_el("pin", 12., c.accent)))
            .when(self.org.is_muted(m), |d| d.child(icon_el("bell-off", 12., c.muted)))
            .when(count > 1, |d| {
                d.child(
                    div()
                        .px(px(5.))
                        .h(px(16.))
                        .rounded(px(8.))
                        .bg(rgb(c.soft))
                        .flex()
                        .items_center()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(c.muted))
                        .child(count.to_string()),
                )
            })
    }

    /// The context menu's entries for organizing one message.
    pub(super) fn org_entries(&self, m: &Mail) -> Vec<Entry> {
        let mut entries = vec![
            Entry::Separator,
            entry(
                if self.org.is_pinned(m) { t!("organize-unpin") } else { t!("organize-pin-to-top") },
                "pin",
                Action::Organize(OrgAction::TogglePin(m.id.clone())),
            ),
            entry(
                if self.org.is_muted(m) {
                    t!("organize-unmute-conversation")
                } else {
                    t!("organize-mute-conversation")
                },
                "bell-off",
                Action::Organize(OrgAction::ToggleMute(m.id.clone())),
            ),
            entry(t!("organize-print"), "print", Action::Tools(super::tools::ToolAction::Print(m.id.clone()))),
        ];
        if !m.from.is_empty() {
            entries.push(if self.org.is_blocked(&m.from) {
                entry(
                    t!("organize-unblock", from = m.from),
                    "ban",
                    Action::Organize(OrgAction::BlockSender(m.id.clone())),
                )
            } else {
                danger(
                    t!("organize-block", from = m.from),
                    "ban",
                    Action::Organize(OrgAction::BlockSender(m.id.clone())),
                )
            });
        }
        entries
    }

    /// The filter menu's organizing entries.
    pub(super) fn org_filter_entries(&self) -> Vec<Entry> {
        vec![
            Entry::Separator,
            checked(
                t!("organize-group-by-conversation"),
                self.org.threaded,
                Action::Organize(OrgAction::ToggleThreaded),
            ),
            checked(
                t!("organize-comfortable-rows"),
                self.org.density == Density::Comfortable,
                Action::Organize(OrgAction::SetDensity(Density::Comfortable)),
            ),
            checked(
                t!("organize-compact-rows"),
                self.org.density == Density::Compact,
                Action::Organize(OrgAction::SetDensity(Density::Compact)),
            ),
            checked(
                t!("organize-reader-beside-list"),
                self.org.layout == ReaderLayout::Beside,
                Action::Organize(OrgAction::SetLayout(ReaderLayout::Beside)),
            ),
            checked(
                t!("organize-reader-below-list"),
                self.org.layout == ReaderLayout::Below,
                Action::Organize(OrgAction::SetLayout(ReaderLayout::Below)),
            ),
        ]
    }

    pub(super) fn org_action(&mut self, action: OrgAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            OrgAction::ToggleThreaded => {
                self.org.threaded = !self.org.threaded;
                self.save_org(cx);
                self.filter(cx);
            }
            OrgAction::TogglePin(id) => {
                let Some(m) = self.messages.iter().find(|m| m.id == id) else {
                    return;
                };
                let key = mail_key(m);
                if !self.org.pinned.remove(&key) {
                    self.org.pinned.insert(key);
                }
                self.save_org(cx);
                self.filter(cx);
            }
            OrgAction::ToggleMute(id) => {
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                if self.org.is_muted(&m) {
                    let key = conversation_key(&m);
                    self.org.muted.remove(&key);
                    // Muted from another message of it: forget that too.
                    for r in m.references().split_whitespace() {
                        let r = format!("m:{}", r.trim_matches(|c| c == '<' || c == '>'));
                        self.org.muted.remove(&r);
                    }
                    self.notice(t!("organize-conversation-unmuted"), false, cx);
                } else {
                    self.org.muted.insert(conversation_key(&m));
                    // What's there now is read too.
                    let ids: Vec<String> = thread::conversation(&self.messages, &m)
                        .into_iter()
                        .filter(|&i| self.messages[i].unread)
                        .map(|i| self.messages[i].id.clone())
                        .collect();
                    if !ids.is_empty() {
                        self.apply(ids, Change::Read(true), cx);
                    }
                    self.notice(
                        t!("organize-conversation-muted-new-messages-in-it"),
                        false,
                        cx,
                    );
                }
                self.save_org(cx);
                self.filter(cx);
            }
            OrgAction::BlockSender(id) => {
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                if self.org.toggle_block(&m.from) {
                    // Everything from them listed here goes too.
                    let from = m.from.to_lowercase();
                    let ids: Vec<String> = self
                        .messages
                        .iter()
                        .filter(|x| x.from.to_lowercase() == from && x.folder != Folder::Trash)
                        .map(|x| x.id.clone())
                        .collect();
                    self.save_org(cx);
                    self.notice(
                        t!("organize-blocked-their-mail-goes-to-trash", from = m.from),
                        false,
                        cx,
                    );
                    if !ids.is_empty() {
                        self.apply(ids, Change::Move(Folder::Trash), cx);
                    }
                } else {
                    self.save_org(cx);
                    self.notice(t!("organize-unblocked", from = m.from), false, cx);
                }
            }
            OrgAction::Unblock(address) => {
                self.org.blocked.retain(|b| b != &address);
                self.save_org(cx);
            }
            OrgAction::AddBlocked => {
                let text = Self::value(&self.org_ui.block, cx).trim().to_lowercase();
                if text.is_empty() || (!text.contains('@')) {
                    self.notice(t!("organize-type-an-address-or-and-a"), true, cx);
                    return;
                }
                if !self.org.is_blocked(&text) || text.starts_with('@') {
                    if !self.org.blocked.contains(&text) {
                        self.org.blocked.push(text);
                    }
                }
                Self::set(&self.org_ui.block, "", w, cx);
                self.save_org(cx);
            }
            OrgAction::SaveSearch => {
                let query = Self::value(&self.search, cx).trim().to_owned();
                if query.is_empty() {
                    return;
                }
                if !self.org.saved_searches.iter().any(|s| s.query == query) {
                    self.org.saved_searches.push(SavedSearch {
                        name: query.clone(),
                        query,
                    });
                    self.save_org(cx);
                    self.notice(t!("organize-saved-its-in-the-sidebar-under"), false, cx);
                }
            }
            OrgAction::SearchEverywhere => {
                let query = Self::value(&self.search, cx).trim().to_owned();
                if query.is_empty() {
                    return;
                }
                // Every account's mail, whether or not the sidebar shows
                // All Accounts (which stays as it is).
                let account = if self.accounts.len() > 1 {
                    ALL_MAIL.to_owned()
                } else {
                    match self.accounts.first() {
                        Some(a) => a.id.clone(),
                        None => return,
                    }
                };
                let back = match self.local_results.take() {
                    Some(r) => r.back,
                    None => self.account.clone().map(|a| (a, self.mailbox())),
                };
                self.open_mailbox(Some(account), Folder::All.into(), w, cx);
                self.local_results = Some(LocalResults { back });
                self.handle(Action::SearchFor(query), w, cx);
            }
            OrgAction::CloseLocalResults => {
                let Some(r) = self.local_results.take() else {
                    return;
                };
                match r.back {
                    Some((account, mailbox)) if self.place_exists(&account, &mailbox) => {
                        self.open_mailbox(Some(account), mailbox, w, cx)
                    }
                    _ => {
                        let first = self.first_account();
                        self.open_mailbox(first, Folder::Inbox.into(), w, cx);
                    }
                }
            }
            OrgAction::OpenSavedSearch(i) => {
                let Some(saved) = self.org.saved_searches.get(i).cloned() else {
                    return;
                };
                // Every account's mail on this computer, as Search All Mail
                // on This Computer lists it.
                if !self.search_open {
                    self.search_open = true;
                    self.search_anim += 1;
                }
                Self::set(&self.search, &saved.query, w, cx);
                self.org_action(OrgAction::SearchEverywhere, w, cx);
            }
            OrgAction::RemoveSavedSearch(i) => {
                if i < self.org.saved_searches.len() {
                    self.org.saved_searches.remove(i);
                    self.save_org(cx);
                }
            }
            OrgAction::SetDensity(d) => {
                self.org.density = d;
                self.save_org(cx);
                self.list_revision += 1;
            }
            OrgAction::SetLayout(l) => {
                self.org.layout = l;
                self.save_org(cx);
            }
            OrgAction::RuleField(f) => {
                self.org_ui.field = f;
                if !f.ops().contains(&self.org_ui.op) {
                    self.org_ui.op = f.ops().first().copied().unwrap_or_default();
                }
                cx.notify();
            }
            OrgAction::RuleOp(op) => {
                self.org_ui.op = op;
                cx.notify();
            }
            OrgAction::RuleNegate => {
                self.org_ui.negate = !self.org_ui.negate;
                cx.notify();
            }
            OrgAction::RuleCategory(c) => {
                self.org_ui.category = c;
                cx.notify();
            }
            OrgAction::RuleKind(k) => {
                self.org_ui.kind = k;
                cx.notify();
            }
            OrgAction::AddCondition => {
                if let Err(problem) = self.take_condition(w, cx) {
                    self.notice(problem, true, cx);
                }
            }
            OrgAction::RemoveCondition(i) => {
                if i < self.org_ui.draft.conditions.len() {
                    self.org_ui.draft.conditions.remove(i);
                }
            }
            OrgAction::AddAction => {
                if let Err(problem) = self.take_action(w, cx) {
                    self.notice(problem, true, cx);
                }
            }
            OrgAction::RemoveAction(i) => {
                if i < self.org_ui.draft.actions.len() {
                    self.org_ui.draft.actions.remove(i);
                }
            }
            OrgAction::RuleAny(any) => self.org_ui.draft.any = any,
            OrgAction::RuleStop => self.org_ui.draft.stop = !self.org_ui.draft.stop,
            OrgAction::SaveRule => {
                // What's typed and not yet added is taken as the last
                // condition and action.
                let typed = !Self::value(&self.org_ui.rule_text, cx).trim().is_empty()
                    || (self.org_ui.field == RuleField::Attachment && self.org_ui.draft.conditions.is_empty());
                if typed && let Err(problem) = self.take_condition(w, cx) {
                    self.notice(problem, true, cx);
                    return;
                }
                let chosen = self.org_ui.draft.actions.is_empty() || !Self::value(&self.org_ui.rule_name, cx).trim().is_empty();
                if chosen && let Err(problem) = self.take_action(w, cx) {
                    self.notice(problem, true, cx);
                    return;
                }
                let rule = Rule { enabled: true, ..self.org_ui.draft.clone() };
                if let Some(problem) = rule.problem() {
                    self.notice(problem, true, cx);
                    return;
                }
                match self.org_ui.editing.take() {
                    Some(i) if i < self.org.rules.len() => self.org.rules[i] = Rule { enabled: self.org.rules[i].enabled, ..rule },
                    _ => self.org.rules.push(rule),
                }
                self.org_ui.draft = Rule { enabled: true, ..Rule::default() };
                self.save_org(cx);
            }
            OrgAction::EditRule(i) => {
                if let Some(r) = self.org.rules.get(i) {
                    self.org_ui.draft = r.clone();
                    self.org_ui.editing = Some(i);
                }
            }
            OrgAction::CancelRule => {
                self.org_ui.draft = Rule { enabled: true, ..Rule::default() };
                self.org_ui.editing = None;
                Self::set(&self.org_ui.rule_text, "", w, cx);
                Self::set(&self.org_ui.rule_name, "", w, cx);
            }
            OrgAction::RaiseRule(i) => {
                if i > 0 && i < self.org.rules.len() {
                    self.org.rules.swap(i - 1, i);
                    match self.org_ui.editing {
                        Some(e) if e == i => self.org_ui.editing = Some(i - 1),
                        Some(e) if e == i - 1 => self.org_ui.editing = Some(i),
                        _ => {}
                    }
                    self.save_org(cx);
                }
            }
            OrgAction::ToggleRule(i) => {
                if let Some(r) = self.org.rules.get_mut(i) {
                    r.enabled = !r.enabled;
                    self.save_org(cx);
                }
            }
            OrgAction::ApplyRulesNow => self.apply_rules_now(cx),
            OrgAction::RemoveRule(i) => {
                if i < self.org.rules.len() {
                    self.org.rules.remove(i);
                    match self.org_ui.editing {
                        Some(e) if e == i => self.org_ui.editing = None,
                        Some(e) if e > i => self.org_ui.editing = Some(e - 1),
                        _ => {}
                    }
                    self.save_org(cx);
                }
            }
        }
        cx.notify();
    }

    /// Adds the condition typed in Settings to the rule being put
    /// together, or says what's missing.
    fn take_condition(&mut self, w: &mut Window, cx: &mut Context<Self>) -> Result<(), &'static str> {
        let ui = &self.org_ui;
        let value = match ui.field {
            RuleField::Category => ui.category.key().to_owned(),
            RuleField::Attachment => String::new(),
            _ => Self::value(&ui.rule_text, cx).trim().to_owned(),
        };
        let condition = Condition {
            field: ui.field,
            op: ui.field.ops().contains(&ui.op).then_some(ui.op).or(ui.field.ops().first().copied()).unwrap_or_default(),
            value,
            header: if ui.field == RuleField::Header { Self::value(&ui.rule_header, cx).trim().to_owned() } else { String::new() },
            negate: ui.negate,
        };
        if let Some(problem) = condition.problem() {
            return Err(problem);
        }
        if !self.org_ui.draft.conditions.contains(&condition) {
            self.org_ui.draft.conditions.push(condition);
        }
        self.org_ui.negate = false;
        Self::set(&self.org_ui.rule_text, "", w, cx);
        cx.notify();
        Ok(())
    }

    /// Adds the action chosen in Settings to the rule being put together,
    /// or says what's missing.
    fn take_action(&mut self, w: &mut Window, cx: &mut Context<Self>) -> Result<(), &'static str> {
        let name = Self::value(&self.org_ui.rule_name, cx).trim().to_owned();
        let Some(action) = self.org_ui.kind.action(&name) else {
            return Err(t!("organize-type-the-folder-or-labels-name"));
        };
        let draft = &mut self.org_ui.draft;
        if action.moves() && draft.actions.iter().any(RuleAction::moves) {
            return Err(t!("manage-organize-a-rule-moves-mail-once"));
        }
        if !draft.actions.contains(&action) {
            draft.actions.push(action);
        }
        Self::set(&self.org_ui.rule_name, "", w, cx);
        cx.notify();
        Ok(())
    }

    /// Applies blocked senders, muted conversations and rules to mail that
    /// just arrived in an account's inbox (`organize::apply_to_arrivals`).
    pub(super) fn apply_arrival_rules(&mut self, account: &str, fresh: Vec<Mail>, cx: &mut Context<Self>) {
        if fresh.is_empty() || !self.org.applies_to_arrivals() {
            return;
        }
        let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else {
            return;
        };
        self.task(
            cx,
            move |b| crate::manage::organize::apply_to_arrivals(&b, &a, &fresh),
            move |this, result: anyhow::Result<()>, cx| {
                if let Err(e) = result {
                    this.notice(t!("organize-a-rule-couldnt-be-applied", e = format!("{:#}", e)), true, cx);
                }
                this.reload_cached(cx);
                this.refresh_counts(cx);
            },
        );
    }

    /// Applies blocked senders and rules to the mail in every inbox now.
    fn apply_rules_now(&mut self, cx: &mut Context<Self>) {
        if self.applying_rules {
            return;
        }
        if self.org.blocked.is_empty() && !self.org.rules.iter().any(|r| r.enabled) {
            self.notice(t!("organize-add-a-rule-or-block-a"), true, cx);
            return;
        }
        self.applying_rules = true;
        let accounts = self.accounts.clone();
        // Read by the task as saved: saving queues the preference, which
        // reads give back at once.
        self.task(
            cx,
            move |b| crate::manage::organize::apply_to_inboxes(&b, &accounts),
            |this, result: anyhow::Result<usize>, cx| {
                this.applying_rules = false;
                match result {
                    Ok(0) => this.notice(t!("organize-no-mail-in-your-inboxes-matches"), false, cx),
                    Ok(1) => this.notice(t!("organize-rules-applied-to-1-message"), false, cx),
                    Ok(n) => this.notice(t!("organize-rules-applied-to-messages", shown = fmt_count(n), count = n), false, cx),
                    Err(e) => this.notice(t!("organize-rules-couldnt-be-applied-everywhere", e = format!("{:#}", e)), true, cx),
                }
                this.reload_cached(cx);
                this.refresh_counts(cx);
            },
        );
        cx.notify();
    }

    /// Links under the search field: keep the search, or search every
    /// account's mail on this computer (subjects, people, text and
    /// attachments, from the local index); and what is still being looked
    /// up.
    pub(super) fn search_extras(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let query = Self::value(&self.search, cx).trim().to_owned();
        if query.is_empty() {
            return None;
        }
        let c = self.colors();
        let saved = self.org.saved_searches.iter().any(|s| s.query == query);
        let everywhere = self.local_results.is_some();
        let searching_text = matches!(&self.body_matches, Some((q, None)) if *q == query);
        let asking_servers = matches!(&self.undownloaded_search, Some((q, true)) if *q == query);
        Some(
            div()
                .id("search-extras")
                .pt_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_3()
                .gap_y_1()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .when(!saved, |d| {
                    d.child(self.link(
                        "save-search",
                        t!("organize-save-search"),
                        Action::Organize(OrgAction::SaveSearch),
                        cx,
                    ))
                })
                .when(!everywhere, |d| {
                    d.child(self.link(
                        "search-everywhere",
                        t!("organize-search-all-mail-on-this-computer"),
                        Action::Organize(OrgAction::SearchEverywhere),
                        cx,
                    ))
                })
                // What is still being looked up goes after the links, in
                // whatever room is left (basis 0, so it never wraps the row
                // or moves the links while typing), cut short if need be.
                .when(searching_text || asking_servers, |d| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().flex_none().child(spinner(11., c.muted)))
                            .child(div().min_w_0().truncate().child(if searching_text {
                                t!("organize-searching-message-text")
                            } else {
                                t!("organize-asking-the-server-about-mail-not")
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// The list's title while Search All Mail on This Computer's results
    /// show: what they are, how many, and Done to go back.
    pub(super) fn local_results_title(&self, cx: &Context<Self>) -> Option<AnyElement> {
        self.local_results.as_ref()?;
        let c = self.colors();
        let count = self.visible.len();
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    self.button(
                        "local-results-done",
                        t!("organize-back-to-where-you-were"),
                        "chevron-left",
                        Action::Organize(OrgAction::CloseLocalResults),
                        false,
                        cx,
                    )
                    .h(px(28.))
                    .w(px(28.))
                    .rounded(px(7.)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("organize-results-on-this-computer")),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(c.muted))
                                .child(match count {
                                    0 => t!("organize-no-messages-found-in-every-accounts").to_owned(),
                                    1 => t!("organize-1-message-from-every-accounts-mail").to_owned(),
                                    n => t!("organize-messages-from-every-accounts-mail", shown = fmt_count(n), count = n),
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Looks up the words typed in the search (beside any `from:`-style
    /// operators, which the list itself matches) in the local full-text
    /// index of subjects, people, text and attachments, for the list to
    /// show those too. Then asks the servers about mail whose text isn't
    /// here yet (`search_undownloaded_soon`).
    pub(super) fn search_texts_soon(&mut self, cx: &mut Context<Self>) {
        self.look_up_texts(true, cx);
    }

    /// `search_texts_soon`; without `ask_servers`, the index is looked in
    /// again (more of it was filled in) and what servers found stays.
    fn look_up_texts(&mut self, ask_servers: bool, cx: &mut Context<Self>) {
        let typed = Self::value(&self.search, cx).trim().to_owned();
        let search = crate::manage::search::Search::parse(&typed.to_lowercase());
        let (with, without) = search.text_terms();
        let letters: usize = with.iter().map(|w| w.chars().count()).sum();
        if with.is_empty() || letters < 2 {
            self.body_matches = None;
            self.undownloaded_search = None;
            return;
        }
        let earlier = match &self.body_matches {
            Some((q, ids)) if *q == typed => {
                if ask_servers {
                    return;
                }
                ids.clone()
            }
            _ => None,
        };
        if ask_servers {
            self.body_matches = Some((typed.clone(), None));
            self.undownloaded_search = None;
        }
        self.task(
            cx,
            move |b| b.store.search_texts(&with, &without, 20_000),
            move |this, result: anyhow::Result<Vec<String>>, cx| {
                let current = this.body_matches.as_ref().is_some_and(|(q, _)| *q == typed);
                if !current {
                    return;
                }
                let mut ids: HashSet<String> = result.unwrap_or_default().into_iter().collect();
                if let Some(earlier) = earlier {
                    ids.extend(earlier.iter().cloned());
                }
                this.body_matches = Some((typed.clone(), Some(Arc::new(ids))));
                this.filter(cx);
                if ask_servers {
                    this.search_undownloaded_soon(typed, search.text_query(), cx);
                }
                cx.notify();
            },
        );
    }

    /// Mail downloaded only in part (its headers and preview) has no text
    /// in the local index: once typing pauses, the servers of the listed
    /// accounts that have such mail in this mailbox are asked for the
    /// search's words, and what they find is listed with the rest.
    fn search_undownloaded_soon(&mut self, typed: String, words: String, cx: &mut Context<Self>) {
        if words.is_empty() || self.subs_open || self.summary_open || self.folder == Folder::Drafts {
            return;
        }
        let accounts: Vec<Account> = self
            .accounts
            .iter()
            .filter(|a| self.shows(&a.id) && a.provider != Provider::Pop3 && !self.account_blocked(&a.id))
            .cloned()
            .collect();
        if accounts.is_empty() {
            return;
        }
        let mailbox = self.mailbox();
        let generation = self.load_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDOWNLOADED_PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                let current = generation == this.load_generation
                    && this.body_matches.as_ref().is_some_and(|(q, _)| *q == typed)
                    && Self::value(&this.search, cx).trim() == typed;
                if !current {
                    return;
                }
                this.undownloaded_search = Some((typed.clone(), true));
                cx.notify();
                let asked = mailbox.clone();
                this.task(
                    cx,
                    move |b| {
                        let mut found = Vec::new();
                        for a in &accounts {
                            if !b.store.has_undownloaded(&a.id, &asked)? {
                                continue;
                            }
                            // One server that can't be reached leaves the
                            // others' matches.
                            if let Ok(r) = b.search_server(a, &asked, &words, UNDOWNLOADED_RESULTS) {
                                found.extend(r.mails);
                            }
                        }
                        Ok(found)
                    },
                    move |this, result: anyhow::Result<Vec<Mail>>, cx| {
                        let current = generation == this.load_generation
                            && this.body_matches.as_ref().is_some_and(|(q, _)| *q == typed);
                        if !current {
                            return;
                        }
                        this.undownloaded_search = Some((typed.clone(), false));
                        let Ok(found) = result else {
                            return;
                        };
                        if found.is_empty() {
                            return;
                        }
                        let mut ids: HashSet<String> = this
                            .body_matches
                            .as_ref()
                            .and_then(|(_, ids)| ids.as_deref().cloned())
                            .unwrap_or_default();
                        ids.extend(found.iter().map(|m| m.id.clone()));
                        let listed: HashSet<String> = this.messages.iter().map(|m| m.id.clone()).collect();
                        let new: Vec<Mail> = found.into_iter().filter(|m| !listed.contains(&m.id)).collect();
                        if !new.is_empty() {
                            let new = this.backend.with_readable_previews(new);
                            this.messages.extend(new);
                            this.messages.sort_by(Mail::newest_first);
                        }
                        this.body_matches = Some((typed, Some(Arc::new(ids))));
                        this.filter(cx);
                    },
                );
            });
        })
        .detach();
    }

    /// Adds mail cached before the full-text index to it, then reads the
    /// text of downloaded attachments into it, a batch at a time, off the
    /// interface's thread. Called again as mail downloads; one pass runs
    /// at a time.
    pub(super) fn index_texts(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            self.index_again = true;
            return;
        }
        self.indexing = true;
        let store = self.backend.store.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let mut attachments = false;
                loop {
                    let store = store.clone();
                    let added = cx
                        .background_executor()
                        .spawn(async move {
                            match store.index_texts(500)? {
                                0 => store.index_attachments(20).map(|n| (n, true)),
                                n => Ok((n, false)),
                            }
                        })
                        .await;
                    let gone = this.update(cx, |this, _| this.retired).unwrap_or(true);
                    if gone {
                        return;
                    }
                    match added {
                        Ok((0, _)) => break,
                        Ok((_, a)) => attachments |= a,
                        Err(e) => {
                            eprintln!("Picacho: the search index couldn't be completed: {e:#}");
                            break;
                        }
                    }
                    // Other work gets the cache in between.
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                }
                let again = this
                    .update(cx, |this, cx| {
                        // A search typed meanwhile finds what was added.
                        if attachments && this.body_matches.is_some() {
                            this.look_up_texts(false, cx);
                        }
                        let again = std::mem::take(&mut this.index_again);
                        if !again {
                            this.indexing = false;
                        }
                        again
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
        })
        .detach();
    }

    /// Saved searches under the sidebar's mailboxes.
    pub(super) fn saved_searches_sidebar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.org.saved_searches.is_empty() {
            return None;
        }
        let c = self.colors();
        let mut list = div().flex().flex_col().gap(px(1.)).mt_3().child(
            div()
                .px_2()
                .pb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(c.muted))
                .child(t!("organize-saved-searches")),
        );
        for (i, s) in self.org.saved_searches.iter().enumerate() {
            let name = s.name.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("saved-search-{i}")))
                    .debug_selector(move || format!("saved-search-{i}"))
                    .h(px(30.))
                    .px_2()
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .hover(move |s| s.bg(rgb(c.surface)))
                    .child(div().size(px(12.)))
                    .child(icon_el("search", 14., c.muted))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::Organize(OrgAction::OpenSavedSearch(i)), w, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            this.open_menu(
                                e.position,
                                vec![
                                    entry(
                                        t!("organize-open"),
                                        "search",
                                        Action::Organize(OrgAction::OpenSavedSearch(i)),
                                    ),
                                    danger(
                                        t!("organize-remove-saved-search"),
                                        "trash",
                                        Action::Organize(OrgAction::RemoveSavedSearch(i)),
                                    ),
                                ],
                                cx,
                            )
                        }),
                    ),
            );
        }
        Some(list.into_any_element())
    }

    /// Settings → Rules & Layout.
    pub(super) fn organizing_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let org = &self.org;
        let act = |a: OrgAction| Action::Organize(a);
        let density = self
            .segmented(
                "density",
                vec![
                    (
                        t!("organize-comfortable").into(),
                        act(OrgAction::SetDensity(Density::Comfortable)),
                        org.density == Density::Comfortable,
                    ),
                    (
                        t!("organize-compact").into(),
                        act(OrgAction::SetDensity(Density::Compact)),
                        org.density == Density::Compact,
                    ),
                ],
                cx,
            )
            .max_w(px(300.));
        let layout = self
            .segmented(
                "reader-layout",
                vec![
                    (
                        t!("organize-beside-the-list").into(),
                        act(OrgAction::SetLayout(ReaderLayout::Beside)),
                        org.layout == ReaderLayout::Beside,
                    ),
                    (
                        t!("organize-below-the-list").into(),
                        act(OrgAction::SetLayout(ReaderLayout::Below)),
                        org.layout == ReaderLayout::Below,
                    ),
                ],
                cx,
            )
            .max_w(px(300.));
        let small = |t: String| {
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(t)
        };
        let row = |id: String, text: String, trailing: AnyElement| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap_3()
                .child(div().flex_1().min_w_0().text_size(px(13.)).child(text))
                .child(trailing)
        };
        let layout_rows = Rows::new()
            .row(self.setting_row(t!("organize-rows"), "", density))
            .row(self.setting_row(t!("organize-reader"), "", layout))
            .row(self.switch_setting(
                t!("organize-group-by-conversation-2"),
                "",
                "threaded".into(),
                org.threaded,
                act(OrgAction::ToggleThreaded),
                cx,
            ));
        let mut saved = Rows::new().when(org.saved_searches.is_empty(), |rows| {
            rows.row(small(t!("organize-search-then-choose-save-search-beside").into()))
        });
        for (i, s) in org.saved_searches.iter().enumerate() {
            saved = saved.row(row(
                format!("saved-row-{i}"),
                s.query.clone(),
                self.button(
                    format!("remove-saved-{i}"),
                    "@Remove",
                    "trash",
                    act(OrgAction::RemoveSavedSearch(i)),
                    false,
                    cx,
                )
                .into_any_element(),
            ));
        }
        let mut blocked = Rows::new();
        for (i, b) in org.blocked.iter().enumerate() {
            blocked = blocked.row(row(
                format!("blocked-row-{i}"),
                b.clone(),
                self.button(
                    format!("unblock-{i}"),
                    t!("organize-unblock-2"),
                    "",
                    act(OrgAction::Unblock(b.clone())),
                    false,
                    cx,
                )
                .into_any_element(),
            ));
        }
        blocked = blocked.row(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().child(field(Input::new(&self.org_ui.block), c, false)))
                .child(self.button("add-blocked", t!("organize-block-2"), "", act(OrgAction::AddBlocked), true, cx)),
        );
        let mut rules = Rows::new();
        for (i, r) in org.rules.iter().enumerate() {
            let editing = self.org_ui.editing == Some(i);
            rules = rules.row(
                div()
                    .id(SharedString::from(format!("rule-row-{i}")))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_size(px(13.))
                            .when(!r.name.is_empty(), |d| d.child(div().font_weight(FontWeight::SEMIBOLD).child(r.name.clone())))
                            .child(r.describe())
                            .when(r.stop, |d| d.child(small(t!("organize-then-stop").into())))
                            .when(editing, |d| d.child(small(t!("organize-being-changed-below").into()))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.switch(format!("rule-on-{i}"), r.enabled, act(OrgAction::ToggleRule(i)), cx))
                            .when(i > 0, |d| {
                                d.child(self.button(format!("raise-rule-{i}"), t!("organize-move-up"), "chevron-up", act(OrgAction::RaiseRule(i)), false, cx))
                            })
                            .child(self.button(format!("edit-rule-{i}"), t!("organize-edit-rule"), "", act(OrgAction::EditRule(i)), false, cx))
                            .child(self.button(
                                format!("remove-rule-{i}"),
                                t!("organize-remove-rule"),
                                "trash",
                                act(OrgAction::RemoveRule(i)),
                                false,
                                cx,
                            )),
                    ),
            );
        }
        rules = rules.row(self.rule_form(cx));
        self.settings_column()
            .child(self.settings_group(t!("organize-layout"), layout_rows))
            .child(self.categories_settings(cx))
            .child(self.settings_group(t!("organize-saved-searches-2"), saved))
            .child(self.settings_group(t!("organize-blocked-senders"), blocked))
            .child(self.settings_group(t!("organize-rules"), rules))
            .into_any_element()
    }
}

impl MailApp {
    /// Putting a rule together: its conditions (each added in turn), all
    /// or any of them, its actions in order, and whether later rules still
    /// apply.
    fn rule_form(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let ui = &self.org_ui;
        let draft = &ui.draft;
        let act = |a: OrgAction| Action::Organize(a);
        let small = |t: String| div().text_size(px(12.)).text_color(rgb(c.muted)).child(t);
        let chip = |id: String, text: String, remove: OrgAction| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .max_w_full()
                .h(px(26.))
                .pl_2()
                .pr_1()
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(13.))
                .bg(rgb(c.soft))
                .text_size(px(12.))
                .text_color(rgb(c.text))
                .child(div().min_w_0().truncate().child(text.clone()))
                .child(
                    div()
                        .id("remove")
                        .role(Role::Button)
                        .aria_label(t!("organize-remove-part", part = text))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.border)))
                        .child(icon_el("close", 10., c.muted))
                        .on_click(cx.listener(move |this, _, w, cx| this.handle(Action::Organize(remove.clone()), w, cx))),
                )
        };
        let conditions = div().flex().flex_wrap().gap_1().children(
            draft
                .conditions
                .iter()
                .enumerate()
                .map(|(i, cond)| chip(format!("rule-condition-{i}"), cond.describe(), OrgAction::RemoveCondition(i))),
        );
        let actions = div().flex().flex_wrap().gap_1().children(
            draft
                .actions
                .iter()
                .enumerate()
                .map(|(i, a)| chip(format!("rule-action-{i}"), a.label(), OrgAction::RemoveAction(i))),
        );
        let fields = self.segmented(
            "rule-field",
            RuleField::ALL
                .iter()
                .map(|f| (f.label().to_owned(), act(OrgAction::RuleField(*f)), ui.field == *f))
                .collect(),
            cx,
        );
        let ops = ui.field.ops();
        let op = if ops.contains(&ui.op) { ui.op } else { ops.first().copied().unwrap_or_default() };
        let comparisons = (ops.len() > 1).then(|| {
            self.segmented(
                "rule-op",
                ops.iter().map(|o| (o.label().to_owned(), act(OrgAction::RuleOp(*o)), op == *o)).collect(),
                cx,
            )
        });
        let value: AnyElement = match ui.field {
            RuleField::Attachment => small(t!("organize-attachment-condition").into()).into_any_element(),
            RuleField::Category => self
                .segmented(
                    "rule-category",
                    Category::ALL
                        .iter()
                        .map(|cat| (cat.label().to_owned(), act(OrgAction::RuleCategory(*cat)), ui.category == *cat))
                        .collect(),
                    cx,
                )
                .into_any_element(),
            RuleField::Header => div()
                .flex()
                .gap_2()
                .child(div().w(px(160.)).child(field(Input::new(&ui.rule_header), c, false)))
                .child(div().flex_1().child(field(Input::new(&ui.rule_text), c, false)))
                .into_any_element(),
            _ => field(Input::new(&ui.rule_text), c, false).into_any_element(),
        };
        let kinds = self.segmented(
            "rule-kind",
            RuleKind::ALL
                .iter()
                .map(|k| (k.label().to_owned(), act(OrgAction::RuleKind(*k)), ui.kind == *k))
                .collect(),
            cx,
        );
        let several = draft.conditions.len() > 1;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(if ui.editing.is_some() {
                t!("organize-change-rule")
            } else {
                t!("organize-new-rule")
            }))
            .child(small(t!("organize-when").into()))
            .when(!draft.conditions.is_empty(), |d| d.child(conditions))
            .when(several, |d| {
                d.child(
                    self.segmented(
                        "rule-any",
                        vec![
                            (t!("organize-all-conditions").into(), act(OrgAction::RuleAny(false)), !draft.any),
                            (t!("organize-any-condition").into(), act(OrgAction::RuleAny(true)), draft.any),
                        ],
                        cx,
                    )
                    .max_w(px(360.)),
                )
            })
            .child(fields)
            .children(comparisons)
            .child(value)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.switch_setting(t!("organize-doesnt-match"), "", "rule-negate".into(), ui.negate, act(OrgAction::RuleNegate), cx).flex_1())
                    .child(self.button("add-condition", t!("organize-add-condition"), "plus", act(OrgAction::AddCondition), false, cx)),
            )
            .child(small(t!("organize-then").into()))
            .when(!draft.actions.is_empty(), |d| d.child(actions))
            .child(kinds)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().when(ui.kind.named(), |d| d.child(field(Input::new(&ui.rule_name), c, false))))
                    .child(self.button("add-action", t!("organize-add-action"), "plus", act(OrgAction::AddAction), false, cx)),
            )
            .child(self.switch_setting(
                t!("organize-stop-other-rules"),
                t!("organize-stop-other-rules-caption"),
                "rule-stop".into(),
                draft.stop,
                act(OrgAction::RuleStop),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(if self.applying_rules {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(spinner(12., c.muted))
                            .child(t!("organize-applying"))
                            .into_any_element()
                    } else {
                        self.button("apply-rules", t!("organize-apply-rules-now"), "", act(OrgAction::ApplyRulesNow), false, cx)
                            .into_any_element()
                    })
                    .when(ui.editing.is_some() || !draft.conditions.is_empty() || !draft.actions.is_empty(), |d| {
                        d.child(self.button("cancel-rule", t!("organize-cancel"), "", act(OrgAction::CancelRule), false, cx))
                    })
                    .child(self.button(
                        "add-rule",
                        if ui.editing.is_some() { t!("organize-save-rule") } else { t!("organize-add-rule") },
                        "plus",
                        act(OrgAction::SaveRule),
                        true,
                        cx,
                    )),
            )
    }
}

/// A compact row: sender and subject, no preview.
pub(super) const COMPACT_ROW_HEIGHT: f32 = 58.;
/// How long typing pauses before servers are asked about mail whose text
/// isn't here (`search_undownloaded_soon`).
const UNDOWNLOADED_PAUSE: Duration = Duration::from_millis(800);
/// The most matches each server lists for it.
const UNDOWNLOADED_RESULTS: usize = 200;
