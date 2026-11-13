//! Views that list more than one mailbox, or part of one: All Mail across
//! every account, and aliases from forwarding services shown as accounts of
//! their own, and the work behind the Forwarding settings page (drawn in
//! `forwarding.rs`) that manages those aliases.
use super::tree::Dragged;
use super::*;
use crate::manage::backend::Layout;

/// An alias's view has no Sent or Drafts: nothing is written from it.
const ALIAS_FOLDERS: [Folder; 5] = [
    Folder::Inbox,
    Folder::Starred,
    Folder::Archive,
    Folder::Junk,
    Folder::Trash,
];
/// How often aliases are fetched again in the background.
pub(super) const ALIAS_REFRESH: Duration = Duration::from_secs(5 * 60);
/// Opening Forwarding settings or a composer fetches them unless they
/// were fetched this recently.
pub(super) const ALIAS_REFRESH_SOON: Duration = Duration::from_secs(30);

impl MailApp {
    /// The sidebar arrangement's entries: accounts, then aliases shown as accounts.
    pub(super) fn tree_ids(accounts: &[Account], forwarding: &[ForwardingAccount]) -> Vec<String> {
        accounts
            .iter()
            .map(|a| a.id.clone())
            .chain(fwd::separate_views(forwarding))
            .collect()
    }
    /// Works out what the open view lists; call when it or its aliases change.
    pub(super) fn update_scope(&mut self) {
        self.scope = match &self.account {
            Some(id) => fwd::scope(id, self.folder, &self.accounts, &self.forwarding),
            None => fwd::Scope::default(),
        };
    }
    /// Whether the open view lists mail from `account`.
    pub(super) fn shows(&self, account: &str) -> bool {
        self.account.is_some() && self.scope.accounts.iter().any(|a| a == account)
    }
    /// Whether the open view lists exactly one mailbox, `account`, so its
    /// layout and placeholders apply.
    pub(super) fn shows_only(&self, account: &str) -> bool {
        self.account.is_some() && self.scope.accounts == [account]
    }
    pub(super) fn shows_all_mail(&self) -> bool {
        self.all_mail && self.accounts.len() > 1
    }
    /// Lists the open view from the cache: one mailbox as it is, with its
    /// placeholders, or several merged newest first.
    pub(super) fn list_job(
        &self,
        mailbox: Mailbox,
    ) -> impl FnOnce(Arc<Backend>) -> anyhow::Result<(Vec<Mail>, Option<Layout>)> + Send + 'static
    {
        let accounts = self.scope.accounts.clone();
        move |b| {
            if let [id] = accounts.as_slice() {
                return b.listed(id, &mailbox);
            }
            let mut all = Vec::new();
            for id in &accounts {
                let (messages, _) = b.listed(id, &mailbox)?;
                all.extend(messages.into_iter().filter(|m| !m.pending));
            }
            all.sort_by(Mail::newest_first);
            Ok((all, None))
        }
    }
    /// Checks every mailbox the open view lists.
    pub(super) fn refresh_view(&mut self, cx: &mut Context<Self>) {
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
            return;
        }
        let mailbox = self.mailbox();
        for id in self.scope.accounts.clone() {
            if self.account_blocked(&id) {
                continue;
            }
            if let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() {
                self.sync_mailbox(a, mailbox.clone(), SYNC_WINDOW, cx);
            }
        }
    }
    /// Marks a view's mailbox read: All Mail in each account, an alias's
    /// view only the messages sent to it.
    pub(super) fn mark_view_read(
        &mut self,
        view: String,
        mailbox: Mailbox,
        cx: &mut Context<Self>,
    ) {
        let scope = fwd::scope(&view, mailbox.folder(), &self.accounts, &self.forwarding);
        if !scope.filters() {
            for id in scope.accounts {
                self.mark_all_read(id, mailbox.clone(), cx);
            }
            return;
        }
        if self.account.as_ref() == Some(&view) && self.mailbox() == mailbox {
            for m in self.messages.iter_mut().filter(|m| scope.admits(m)) {
                m.unread = false;
            }
            self.filter(cx);
        }
        let accounts: Vec<Account> = self
            .accounts
            .iter()
            .filter(|a| scope.accounts.contains(&a.id))
            .cloned()
            .collect();
        for a in accounts {
            let (scope, mailbox, id) = (scope.clone(), mailbox.clone(), a.id.clone());
            self.task(
                cx,
                move |b| {
                    let unread: Vec<Mail> = b
                        .store
                        .summaries(&a.id, mailbox)?
                        .into_iter()
                        .filter(|m| m.unread && scope.admits(m))
                        .collect();
                    if unread.is_empty() {
                        return Ok(());
                    }
                    b.change_many(&a, &unread, Change::Read(true))
                },
                move |this, result, cx| {
                    this.account_result(&id, IssueSource::Change, &result);
                    this.refresh_counts(cx);
                    if result.is_err() {
                        this.reload_cached(cx);
                    }
                },
            );
        }
    }

    /// A view's name, what its tooltip says, and its icon.
    fn view_title(&self, id: &str) -> Option<(String, String, &'static str)> {
        if id == ALL_MAIL {
            return Some((
                "All Mail".into(),
                "Mail from every account".into(),
                "inboxes",
            ));
        }
        let (f, alias) = fwd::find_alias(&self.forwarding, id)?;
        let into = f
            .deliver_to
            .as_ref()
            .and_then(|d| self.accounts.iter().find(|a| &a.id == d))
            .map(|a| format!(" · forwards to {}", a.email))
            .unwrap_or_default();
        Some((
            alias.email.clone(),
            format!("{} alias{into}", f.service.label()),
            "alias",
        ))
    }
    fn view_folders(id: &str) -> &'static [Folder] {
        if id == ALL_MAIL {
            &Folder::ALL
        } else {
            &ALIAS_FOLDERS
        }
    }
    fn view_entries(&self, id: &str) -> Vec<Entry> {
        let id = id.to_owned();
        let mut entries = vec![
            entry("Refresh", "refresh", Action::RefreshAccount(id.clone())),
            entry(
                "Mark Inbox as Read",
                "mail-open",
                Action::MarkAllRead(id.clone(), Folder::Inbox.into()),
            ),
            Entry::Separator,
            entry(
                if self.collapsed.contains(&id) {
                    "Expand"
                } else {
                    "Collapse"
                },
                "chevron-down",
                Action::ToggleAccount(id.clone()),
            ),
        ];
        if id == ALL_MAIL {
            entries.push(entry("Hide All Mail", "eye-off", Action::SetAllMail(false)));
        } else if let Some((f, alias)) = fwd::find_alias(&self.forwarding, &id) {
            entries.push(entry(
                "Copy Address",
                "copy",
                Action::CopyText(alias.email.clone()),
            ));
            entries.push(entry(
                "Forwarding Settings…",
                "settings",
                Action::SettingsPage(Page::Forwarding),
            ));
            entries.push(Entry::Separator);
            entries.push(entry(
                "Show in Mailbox",
                "inbox",
                Action::SetAliasDisplay(f.id.clone(), alias.id.clone(), AliasDisplay::Included),
            ));
        }
        entries
    }
    /// A view's mark: its icon on a soft circle.
    pub(super) fn view_mark(&self, icon: &'static str, size: f32, current: bool) -> Div {
        let c = self.colors();
        div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .bg(rgb(if current { c.accent } else { c.soft }))
            .flex()
            .items_center()
            .justify_center()
            .child(icon_el(
                icon,
                size * 0.62,
                if current { c.on_accent } else { c.accent },
            ))
    }
    /// All Mail or an alias in the expanded sidebar, like an account.
    pub(super) fn view_block(
        &self,
        id: &str,
        depth: usize,
        row: &Cell<usize>,
        movable: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.colors();
        let (name, tip, icon) = self.view_title(id)?;
        let open = self.open(id);
        let entries = self.view_entries(id);
        let inbox_unread = self.unread_for(id, &Folder::Inbox.into());
        let current = self.account.as_deref() == Some(id);
        let header = div()
            .id(SharedString::from(format!("account-{id}")))
            .debug_selector({
                let id = id.to_owned();
                move || format!("account-{id}")
            })
            .relative()
            .h(px(34.))
            .px_2()
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(current, |d| {
                d.bg(rgb(if open { c.surface } else { c.soft })).child(
                    div()
                        .absolute()
                        .left(px(-6.))
                        .top(px(9.))
                        .w(px(3.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(rgb(c.accent)),
                )
            })
            .hover(move |s| s.bg(rgb(if current { c.soft } else { c.surface })))
            .child(self.chevron(id, open))
            .child(
                div()
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(if current { c.accent } else { c.bg }))
                    .child(self.view_mark(icon, 20., false)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(if current { c.text } else { c.muted }))
                    .child(name),
            )
            .when(!open && inbox_unread > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child(fmt_count(inbox_unread)),
                )
            })
            .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            .on_click(cx.listener({
                let id = id.to_owned();
                move |this, _, w, cx| this.handle(Action::ToggleAccount(id.clone()), w, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            );
        let mut block = if movable {
            let header = self.draggable(header, id, cx);
            self.block(id, 0., cx)
                .id(SharedString::from(format!("block-{id}")))
                .on_drag_move(cx.listener({
                    let id = id.to_owned();
                    move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                        this.on_hover_row(&id, e, depth > 0, cx)
                    }
                }))
                .child(self.stagger(header, Self::next_row(row)))
        } else {
            div()
                .mt(px(4.))
                .flex()
                .flex_col()
                .id(SharedString::from(format!("block-{id}")))
                .child(self.stagger(header, Self::next_row(row)))
        };
        if open {
            for (index, folder) in Self::view_folders(id).iter().enumerate() {
                let nav = self.nav_row(
                    id,
                    (*folder).into(),
                    folder.label().into(),
                    folder_icon(*folder),
                    0.,
                    cx,
                );
                block = block.child(self.reveal(
                    id,
                    div().child(self.stagger(nav, Self::next_row(row))),
                    index + 1,
                ));
            }
            if id == ALL_MAIL {
                let nav = self.subscriptions_nav(id, cx);
                block = block.child(self.reveal(
                    id,
                    div().child(self.stagger(nav, Self::next_row(row))),
                    Self::view_folders(id).len() + 1,
                ));
            }
        }
        // All Mail stands apart from the accounts below it.
        if !movable {
            block = block.child(div().mt(px(6.)).mx_2().h(px(1.)).bg(rgb(c.border)));
        }
        Some(block.into_any_element())
    }
    /// All Mail or an alias in the collapsed sidebar.
    pub(super) fn rail_view(
        &self,
        id: &str,
        row: &Cell<usize>,
        movable: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let c = self.colors();
        let (name, _, icon) = self.view_title(id)?;
        let current = self.account.as_deref() == Some(id);
        let open = self.open(id);
        let entries = self.view_entries(id);
        let unread = self.unread_for(id, &Folder::Inbox.into());
        let button = self
            .rail_button(
                format!("rail-account-{id}"),
                name,
                div()
                    .relative()
                    .rounded_full()
                    .border_2()
                    .border_color(rgb(if current { c.accent } else { c.bg }))
                    .child(self.view_mark(icon, 24., false))
                    .when(!open && unread > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-2.))
                                .size(px(8.))
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(c.bg))
                                .bg(rgb(c.accent)),
                        )
                    })
                    .into_any_element(),
                current && !open,
                Action::ToggleAccount(id.to_owned()),
                cx,
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            );
        let mut block = if movable {
            let button = self.draggable(button, id, cx);
            self.block(id, -4., cx)
                .id(SharedString::from(format!("rail-block-{id}")))
                .on_drag_move(cx.listener({
                    let id = id.to_owned();
                    move |this, e: &DragMoveEvent<Dragged>, _, cx| {
                        this.on_hover_row(&id, e, false, cx)
                    }
                }))
                .items_center()
                .gap_1()
                .child(self.stagger(button, Self::next_row(row)))
        } else {
            div()
                .id(SharedString::from(format!("rail-block-{id}")))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(self.stagger(button, Self::next_row(row)))
        };
        if open {
            for (index, folder) in Self::view_folders(id).iter().enumerate() {
                let mailbox: Mailbox = (*folder).into();
                let active = current && self.mailbox() == mailbox;
                let count = self.unread_for(id, &mailbox);
                let button = self.rail_button(
                    format!("rail-{id}-{}", folder.key()),
                    if count > 0 {
                        format!("{} · {} unread", folder.label(), fmt_count(count))
                    } else {
                        folder.label().into()
                    },
                    div()
                        .relative()
                        .child(icon_el(
                            folder_icon(*folder),
                            17.,
                            if active { c.accent } else { c.muted },
                        ))
                        .when(count > 0, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(-2.))
                                    .right(px(-3.))
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(rgb(c.accent)),
                            )
                        })
                        .into_any_element(),
                    active,
                    Action::OpenMailbox(id.to_owned(), mailbox),
                    cx,
                );
                block = block.child(self.reveal(
                    id,
                    div().child(self.stagger(button, Self::next_row(row))),
                    index,
                ));
            }
            if id == ALL_MAIL {
                let button = self.subscriptions_rail(id, cx);
                block = block.child(self.reveal(
                    id,
                    div().child(self.stagger(button, Self::next_row(row))),
                    Self::view_folders(id).len(),
                ));
            }
        }
        if !movable {
            block = block.child(div().h(px(1.)).w(px(28.)).my_1().bg(rgb(c.border)));
        }
        Some(block.into_any_element())
    }

    pub(super) fn set_all_mail(&mut self, on: bool, cx: &mut Context<Self>) {
        self.all_mail = on;
        let _ = self
            .backend
            .store
            .set_preference(fwd::ALL_MAIL_PREFERENCE, if on { "true" } else { "false" });
        self.views_changed(cx);
    }
    /// After aliases, services or All Mail change: saves them, updates the
    /// sidebar, and leaves a view that is no longer listed.
    pub(super) fn views_changed(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = fwd::save(&self.backend.store, &self.forwarding) {
            self.notice(e.to_string(), true, cx);
        }
        self.sync_tree();
        let gone = match self.account.as_deref() {
            Some(ALL_MAIL) => !self.shows_all_mail(),
            Some(id) if fwd::is_view(id) => !fwd::separate_views(&self.forwarding)
                .iter()
                .any(|v| v == id),
            _ => false,
        };
        if gone {
            self.html_view.update(cx, |view, cx| view.clear(cx));
            self.account = self.first_account();
            self.folder = Folder::Inbox;
            self.remote.clear();
            self.selected = None;
            self.selection.clear();
            self.messages.clear();
            self.layout = None;
            self.load_generation += 1;
            self.update_scope();
            self.filter(cx);
            self.reload_cached(cx);
            self.refresh(cx);
        } else {
            let before = self.scope.clone();
            self.update_scope();
            if self.scope.accounts != before.accounts {
                self.reload_cached(cx);
                self.refresh(cx);
            }
            self.filter(cx);
        }
        self.refresh_counts(cx);
        self.update_composer_aliases(cx);
        cx.notify();
    }
    /// Changes a service account here (not at the service) and saves it.
    pub(super) fn edit_forwarding(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ForwardingAccount),
        cx: &mut Context<Self>,
    ) {
        if let Some(f) = self.forwarding.iter_mut().find(|f| f.id == id) {
            change(f);
            self.views_changed(cx);
        }
    }
    /// A removed mailbox no longer receives any service's aliases.
    pub(super) fn forget_mailbox(&mut self, id: &str) {
        let mut changed = false;
        for f in &mut self.forwarding {
            if f.deliver_to.as_deref() == Some(id) {
                f.deliver_to = None;
                changed = true;
            }
        }
        if changed {
            let _ = fwd::save(&self.backend.store, &self.forwarding);
        }
    }
    pub(super) fn connect_forwarding(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.fwd_busy.insert("connect".into()) {
            return;
        }
        let service = self.fwd_service;
        let key = Self::value(&self.fwd_key, cx);
        let server = Self::value(&self.fwd_server, cx);
        let accounts = self.accounts.clone();
        self.fwd_error = None;
        self.task(
            cx,
            move |_| fwd::connect(service, &server, &key, &accounts),
            move |this, result, cx| {
                this.fwd_busy.remove("connect");
                match result {
                    Ok(account) => {
                        let count = account.aliases.len();
                        this.notice(
                            format!(
                                "Connected {} · {count} alias{}",
                                account.service.label(),
                                if count == 1 { "" } else { "es" }
                            ),
                            false,
                            cx,
                        );
                        this.fwd_adding = false;
                        this.fwd_open.insert(account.id.clone());
                        this.forwarding.push(account);
                        this.views_changed(cx);
                    }
                    Err(e) => this.fwd_error = Some(format!("{e:#}")),
                }
            },
        );
        // The key is not kept in the field once sent.
        Self::set(&self.fwd_key, "", w, cx);
    }
    /// Fetches a service's aliases again. `quiet` ones (made by Picacho on
    /// its own) don't report failures.
    pub(super) fn refresh_aliases(&mut self, id: String, quiet: bool, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        if !self.fwd_busy.insert(id.clone()) {
            return;
        }
        self.fwd_refreshed
            .insert(id.clone(), std::time::Instant::now());
        self.task(
            cx,
            move |_| fwd::refresh(&account),
            move |this, result, cx| {
                this.fwd_busy.remove(&id);
                match result {
                    Ok(fresh) => {
                        let Some(f) = this.forwarding.iter_mut().find(|f| f.id == id) else {
                            return;
                        };
                        let mut changed = false;
                        if let Some(mailboxes) = fresh.mailboxes
                            && f.mailboxes != mailboxes
                        {
                            f.mailboxes = mailboxes;
                            changed = true;
                        }
                        if f.aliases != fresh.aliases {
                            f.aliases = fresh.aliases;
                            changed = true;
                        }
                        if changed {
                            this.views_changed(cx);
                        }
                    }
                    Err(_) if quiet => {}
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }
    /// Fetches every service's aliases again unless that was done within
    /// `within`: they can be made, changed or deleted outside Picacho.
    pub(super) fn refresh_aliases_due(&mut self, within: Duration, cx: &mut Context<Self>) {
        // Tests have no services or keychain to ask.
        if cfg!(test) {
            return;
        }
        let now = std::time::Instant::now();
        let due: Vec<String> = self
            .forwarding
            .iter()
            .map(|f| f.id.clone())
            .filter(|id| {
                self.fwd_refreshed
                    .get(id)
                    .is_none_or(|t| now.duration_since(*t) >= within)
            })
            .collect();
        for id in due {
            self.refresh_aliases(id, true, cx);
        }
    }
    pub(super) fn remove_forwarding(&mut self, id: String, cx: &mut Context<Self>) {
        if self.fwd_confirm.as_deref() != Some(&id) {
            self.fwd_confirm = Some(id);
            return;
        }
        self.fwd_confirm = None;
        if let Some(i) = self.forwarding.iter().position(|f| f.id == id) {
            let f = self.forwarding.remove(i);
            fwd::forget(&f);
            self.views_changed(cx);
        }
    }
    pub(super) fn create_alias(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        if self.fwd_busy.contains(&id) {
            return;
        }
        let domain = self.alias_domain(&id).cloned();
        let prefix = fwd::clean_prefix(&Self::value(&self.fwd_prefix, cx));
        let typed = !Self::value(&self.fwd_prefix, cx).trim().is_empty();
        if typed && prefix.is_empty() {
            self.notice(
                "Use letters, numbers, dots, dashes or underscores in the prefix.",
                true,
                cx,
            );
            return;
        }
        let new = fwd::NewAlias {
            prefix: if domain.as_ref().is_none_or(|d| d.custom_prefix) {
                prefix
            } else {
                String::new()
            },
            domain: domain.map(|d| d.suffix),
            format: self.fwd_format,
            note: Self::value(&self.fwd_note, cx).trim().to_owned(),
            mailbox: self
                .fwd_mailbox
                .as_ref()
                .and_then(|m| self.accounts.iter().find(|a| &a.id == m))
                .map(|a| a.email.clone()),
        };
        self.fwd_busy.insert(id.clone());
        self.task(
            cx,
            move |_| fwd::create(&account, &new),
            move |this, result, cx| {
                this.fwd_busy.remove(&id);
                match result {
                    Ok(alias) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(alias.email.clone()));
                        this.notice(format!("Created {} · copied", alias.email), false, cx);
                        if let Some(f) = this.forwarding.iter_mut().find(|f| f.id == id) {
                            f.aliases.insert(0, alias);
                        }
                        this.fwd_creating = None;
                        let _ = fwd::save(&this.backend.store, &this.forwarding);
                        this.update_composer_aliases(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }
    /// Opens a service's new alias form, empty, for an alias forwarding to
    /// `mailbox`, and lists the domains it can use.
    pub(super) fn open_alias_form(
        &mut self,
        id: String,
        mailbox: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.forwarding.iter().find(|f| f.id == id).map(|f| f.service) else {
            return;
        };
        self.fwd_creating = Some(id.clone());
        self.fwd_domain = None;
        self.fwd_format = None;
        self.fwd_mailbox = mailbox;
        Self::set(&self.fwd_prefix, "", w, cx);
        Self::set(&self.fwd_note, "", w, cx);
        let first = if service == ForwardingService::FirefoxRelay {
            self.fwd_note.clone()
        } else {
            self.fwd_prefix.clone()
        };
        match self.settings_window {
            Some(handle) if handle.window_id() != w.window_handle().window_id() => {
                let _ = handle.update(cx, |_, w, cx| first.update(cx, |s, cx| s.focus(w, cx)));
            }
            _ => first.update(cx, |s, cx| s.focus(w, cx)),
        }
        if !matches!(self.fwd_options.get(&id), Some(Ok(_))) {
            self.load_alias_options(id, cx);
        }
    }
    pub(super) fn load_alias_options(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let busy = format!("options:{id}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        self.task(
            cx,
            move |_| fwd::options(&account),
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                this.fwd_options
                    .insert(id, result.map_err(|e| format!("{e:#}")));
                cx.notify();
            },
        );
    }
    /// From a composer's From menu: Forwarding settings, open at the
    /// service's new alias form for `mailbox`.
    pub(super) fn new_alias_for(
        &mut self,
        id: String,
        mailbox: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_scroll.set_offset(point(px(0.), px(0.)));
        self.page = Page::Forwarding;
        if !self.reveal_settings_window(cx) {
            if self.modal != Some(Modal::Settings) {
                self.modal = Some(Modal::Settings);
                self.anim += 1;
            }
            w.activate_window();
        }
        self.fwd_adding = false;
        self.fwd_open.insert(id.clone());
        self.open_alias_form(id, Some(mailbox), w, cx);
    }
    /// Applies a new alias form asked for by a composer that was restored
    /// after Undo (its subscription has no window).
    pub(super) fn apply_pending_new_alias(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, mailbox)) = self.pending_new_alias.take() {
            self.new_alias_for(id, mailbox, w, cx);
        }
    }
    /// Gives open composers the aliases they can send as now.
    pub(super) fn update_composer_aliases(&mut self, cx: &mut Context<Self>) {
        let aliases = fwd::sendable(&self.forwarding);
        let services = Self::alias_services(&self.forwarding);
        for composer in &self.composers {
            composer.update(cx, |c, cx| {
                c.set_forwarding(aliases.clone(), services.clone());
                cx.notify();
            });
        }
    }
    /// Services that can make aliases to send as: (service, the mailbox it
    /// forwards to, if one).
    pub(super) fn alias_services(
        forwarding: &[ForwardingAccount],
    ) -> Vec<(String, fwd::ForwardingService, Option<String>)> {
        forwarding
            .iter()
            .filter(|f| f.service.can_send())
            .map(|f| (f.id.clone(), f.service, f.deliver_to.clone()))
            .collect()
    }
    /// The domain the new alias form will use: the chosen one, else the
    /// service's default.
    pub(super) fn alias_domain(&self, id: &str) -> Option<&fwd::AliasDomain> {
        let Some(Ok(options)) = self.fwd_options.get(id) else {
            return None;
        };
        self.fwd_domain
            .as_ref()
            .and_then(|s| options.domains.iter().find(|d| &d.suffix == s))
            .or(options.domains.first())
    }
    /// The domains a service offers, grouped: the person's own, then
    /// premium, then free.
    pub(super) fn domain_entries(&self, id: &str) -> Vec<Entry> {
        let Some(Ok(options)) = self.fwd_options.get(id) else {
            return vec![];
        };
        let chosen = self.alias_domain(id).map(|d| d.suffix.clone());
        let mut entries = vec![];
        for kinds in [
            &[fwd::DomainKind::Custom, fwd::DomainKind::Username][..],
            &[fwd::DomainKind::Premium],
            &[fwd::DomainKind::Shared],
        ] {
            let group: Vec<&fwd::AliasDomain> = options
                .domains
                .iter()
                .filter(|d| kinds.contains(&d.kind))
                .collect();
            if group.is_empty() {
                continue;
            }
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            entries.extend(group.into_iter().map(|d| {
                checked(
                    format!("{} · {}", d.suffix, d.kind.label()),
                    chosen.as_ref() == Some(&d.suffix),
                    Action::SetAliasDomain(d.suffix.clone()),
                )
            }));
        }
        entries
    }
    /// Ways to fill the prefix: the service's suggestion, or random.
    pub(super) fn prefix_entries(&self, id: &str) -> Vec<Entry> {
        let mut entries = vec![];
        if let Some(Ok(options)) = self.fwd_options.get(id)
            && !options.prefix_suggestion.is_empty()
        {
            entries.push(entry(
                format!("Suggested: {}", options.prefix_suggestion),
                "star",
                Action::SetAliasPrefix(options.prefix_suggestion.clone()),
            ));
            entries.push(Entry::Separator);
        }
        entries.extend(fwd::AliasFormat::ALL.into_iter().map(|format| {
            entry(
                format.label(),
                "refresh",
                Action::RandomPrefix(format),
            )
        }));
        entries
    }
    /// SimpleLogin takes the prefix as given, so a random one is made here
    /// and can be edited; addy.io makes its own, so the field is cleared.
    pub(super) fn random_prefix(
        &mut self,
        format: fwd::AliasFormat,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self
            .fwd_creating
            .as_ref()
            .and_then(|id| self.forwarding.iter().find(|f| &f.id == id))
            .map(|f| f.service);
        if service == Some(ForwardingService::SimpleLogin) {
            self.fwd_format = None;
            Self::set(&self.fwd_prefix, fwd::random_prefix(format), w, cx);
        } else {
            self.fwd_format = Some(format);
            Self::set(&self.fwd_prefix, "", w, cx);
        }
    }
    pub(super) fn set_alias_enabled(
        &mut self,
        id: String,
        alias_id: String,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(alias) = account.aliases.iter().find(|a| a.id == alias_id).cloned() else {
            return;
        };
        let busy = format!("{id}:{alias_id}");
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        // Shown at once; put back if the service refuses.
        self.set_alias_flag(&id, &alias_id, on);
        self.task(
            cx,
            move |_| fwd::set_enabled(&account, &alias, on),
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                if let Err(e) = result {
                    this.set_alias_flag(&id, &alias_id, !on);
                    this.notice(format!("{e:#}"), true, cx);
                }
                let _ = fwd::save(&this.backend.store, &this.forwarding);
            },
        );
    }
    fn set_alias_flag(&mut self, id: &str, alias: &str, on: bool) {
        if let Some(a) = self
            .forwarding
            .iter_mut()
            .find(|f| f.id == id)
            .and_then(|f| f.aliases.iter_mut().find(|a| a.id == alias))
        {
            a.enabled = on;
        }
    }
    pub(super) fn delete_alias(&mut self, id: String, alias_id: String, cx: &mut Context<Self>) {
        let busy = format!("{id}:{alias_id}");
        if self.fwd_confirm.as_deref() != Some(&busy) {
            self.fwd_confirm = Some(busy);
            return;
        }
        self.fwd_confirm = None;
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let Some(alias) = account.aliases.iter().find(|a| a.id == alias_id).cloned() else {
            return;
        };
        if !self.fwd_busy.insert(busy.clone()) {
            return;
        }
        self.task(
            cx,
            move |_| fwd::delete(&account, &alias),
            move |this, result, cx| {
                this.fwd_busy.remove(&busy);
                match result {
                    Ok(()) => {
                        if let Some(f) = this.forwarding.iter_mut().find(|f| f.id == id) {
                            f.aliases.retain(|a| a.id != alias_id);
                        }
                        this.views_changed(cx);
                        this.notice("Alias deleted", false, cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }

    /// A row with a title, a caption and a control at the end.
    pub(super) fn toggle_row(&self, title: &str, caption: &str, control: impl IntoElement) -> Div {
        let c = self.colors();
        div()
            .flex()
            .items_center()
            .gap_4()
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
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_owned()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(caption.to_owned()),
                    ),
            )
            .child(control)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{act, frame, open, store};
    use super::*;
    use crate::manage::forwarding::Alias;
    use std::prelude::v1::test;

    /// Two mailboxes; two of Work's messages were sent to a SimpleLogin alias
    /// shown as an account of its own.
    fn setup(cx: &mut TestAppContext) -> (Entity<MailApp>, &mut VisualTestContext, String) {
        let store = store();
        let mut work = String::new();
        for (name, email) in [("Work", "work@example.com"), ("Home", "home@example.com")] {
            let mut a = Account::new(Provider::Imap);
            a.name = name.into();
            a.email = email.into();
            store.save_account(&a).unwrap();
            let mut mails = demo_mail();
            for (i, m) in mails.iter_mut().enumerate() {
                m.account = a.id.clone();
                m.id = format!("{}:{}", a.id, m.id);
                if name == "Work" && (i == 0 || i == 2) {
                    m.to = "Shop <shop@sl.io>".into();
                }
            }
            store.save_messages(&mails).unwrap();
            if name == "Work" {
                work = a.id.clone();
            }
        }
        fwd::save(
            &store,
            &[ForwardingAccount {
                id: "sl".into(),
                service: ForwardingService::SimpleLogin,
                deliver_to: Some(work.clone()),
                aliases: vec![Alias {
                    id: "1".into(),
                    email: "shop@sl.io".into(),
                    enabled: true,
                    display: AliasDisplay::Separate,
                    mailboxes: vec!["7".into()],
                    ..Alias::default()
                }],
                mailboxes: vec![
                    fwd::ServiceMailbox {
                        id: "7".into(),
                        email: "work@example.com".into(),
                    },
                    fwd::ServiceMailbox {
                        id: "8".into(),
                        email: "home@example.com".into(),
                    },
                ],
                ..ForwardingAccount::default()
            }],
        )
        .unwrap();
        let (view, cx) = open(store, cx);
        (view, cx, work)
    }

    #[gpui::test]
    fn aliases_and_all_mail_are_listed_as_accounts(cx: &mut TestAppContext) {
        let (view, cx, work) = setup(cx);
        let alias = fwd::alias_view("sl", "1");
        view.read_with(cx, |this, _| {
            assert_eq!(this.account.as_ref(), Some(&work));
            assert_eq!(this.visible.len(), 8, "the alias's mail left Work's inbox");
            assert_eq!(this.unread_for(&work, &Folder::Inbox.into()), 2);
            assert_eq!(this.unread_for(&alias, &Folder::Inbox.into()), 1);
            assert_eq!(this.unread_for(ALL_MAIL, &Folder::Inbox.into()), 6);
            assert!(tree::account_order(&this.tree).contains(&alias));
        });
        frame(cx);
        assert!(cx.debug_bounds("account-all-mail").is_some());
        let selector: &'static str = Box::leak(format!("account-{alias}").into_boxed_str());
        assert!(cx.debug_bounds(selector).is_some());
        // Neither offers Sent or Drafts.
        let sent: &'static str = Box::leak(format!("nav-{alias}-sent").into_boxed_str());
        assert!(cx.debug_bounds(sent).is_none());
        assert!(cx.debug_bounds("nav-all-mail-drafts").is_some());

        act(
            &view,
            Action::OpenMailbox(alias.clone(), Folder::Inbox.into()),
            cx,
        );
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert_eq!(this.visible.len(), 2);
            assert!(
                this.visible
                    .iter()
                    .all(|i| this.messages[*i].to.contains("shop@sl.io"))
            );
            // New messages are written from the mailbox it forwards to.
            assert_eq!(this.current_account().map(|a| a.id), Some(work.clone()));
        });

        act(
            &view,
            Action::OpenMailbox(ALL_MAIL.into(), Folder::Inbox.into()),
            cx,
        );
        cx.run_until_parked();
        view.read_with(cx, |this, _| assert_eq!(this.visible.len(), 20));

        // Showing the alias in its mailbox again puts its mail back there.
        act(
            &view,
            Action::SetAliasDisplay("sl".into(), "1".into(), AliasDisplay::Included),
            cx,
        );
        act(
            &view,
            Action::OpenMailbox(work.clone(), Folder::Inbox.into()),
            cx,
        );
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert_eq!(this.visible.len(), 10);
            assert!(!tree::account_order(&this.tree).contains(&alias));
        });

        // Turning All Mail off while it is open goes back to an account,
        // and the accounts move up into its place.
        act(
            &view,
            Action::OpenMailbox(ALL_MAIL.into(), Folder::Inbox.into()),
            cx,
        );
        let header: &'static str = Box::leak(format!("account-{work}").into_boxed_str());
        frame(cx);
        let before = cx.debug_bounds(header).unwrap().origin.y;
        act(&view, Action::SetAllMail(false), cx);
        cx.run_until_parked();
        frame(cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.account.as_ref(), Some(&work));
            assert!(!this.all_mail && !this.shows_all_mail());
        });
        let after = cx.debug_bounds(header).unwrap().origin.y;
        assert!(after < before - px(100.), "{before:?} -> {after:?}");
    }

    #[gpui::test]
    fn replies_to_alias_mail_are_sent_as_the_alias(cx: &mut TestAppContext) {
        let (view, cx, work) = setup(cx);
        let alias = fwd::alias_view("sl", "1");
        act(&view, Action::OpenMailbox(alias, Folder::Inbox.into()), cx);
        let id = view.read_with(cx, |this, _| this.messages[this.visible[0]].id.clone());
        act(&view, Action::Select(id), cx);
        act(&view, Action::Reply(false), cx);
        view.read_with(cx, |this, cx| {
            let draft = &this.composers.last().unwrap().read(cx).draft;
            assert_eq!(draft.send_as, "shop@sl.io");
            assert_eq!(draft.account, work);
            assert!(!draft.to.contains("shop@sl.io"), "never to yourself");
        });
        // A new message is written as the account.
        act(&view, Action::Compose, cx);
        view.read_with(cx, |this, cx| {
            assert!(
                this.composers
                    .last()
                    .unwrap()
                    .read(cx)
                    .draft
                    .send_as
                    .is_empty()
            );
        });
        frame(cx);
        assert!(cx.debug_bounds("compose-from").is_some());
    }

    #[gpui::test]
    fn sending_as_a_deleted_alias_explains_and_resets_from(cx: &mut TestAppContext) {
        let (view, cx, work) = setup(cx);
        // Deleted elsewhere: the service no longer lists it, though this
        // window has not fetched the aliases again yet.
        view.update(cx, |this, _| {
            let mut gone = this.forwarding.clone();
            gone[0].aliases.clear();
            fwd::save(&this.backend.store, &gone).unwrap();
        });
        view.update_in(cx, |this, w, cx| this.new_compose(0, w, cx));
        let composer = view.read_with(cx, |this, _| this.composers[0].clone());
        composer.update_in(cx, |c, w, cx| {
            c.fill("ana@example.com", "Plans", w, cx);
            c.draft.send_as = "shop@sl.io".into();
        });
        cx.run_until_parked();
        composer.update_in(cx, |c, w, cx| c.request_send(w, cx));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(6));
        cx.run_until_parked();
        view.read_with(cx, |this, cx| {
            assert!(
                matches!(&this.modal, Some(Modal::AliasGone(why, from))
                    if why.contains("shop@sl.io") && from == "work@example.com"),
                "a dialog says the alias is gone"
            );
            assert!(this.forwarding[0].aliases.is_empty(), "and it is forgotten");
            assert!(this.outbox.is_empty());
            let c = this.composers[0].read(cx);
            assert!(c.draft.send_as.is_empty(), "From is the mailbox again");
            assert_eq!(c.draft.account, work);
            assert!(c.error.is_some());
            assert!(
                this.account_issues
                    .get(&work)
                    .is_none_or(|issues| !issues.contains_key(&IssueSource::Send)),
                "a deleted alias is not a mailbox problem"
            );
        });
        frame(cx);
        assert!(cx.debug_bounds("alias-gone").is_some());
    }

    #[gpui::test]
    fn long_alias_addresses_stay_inside_the_forwarding_page(cx: &mut TestAppContext) {
        let (view, cx, _) = setup(cx);
        let long = format!("{}@{}.example.com", "a".repeat(90), "b".repeat(60));
        view.update(cx, |this, _| {
            this.forwarding[0].aliases[0].email = long.clone();
            this.forwarding[0].aliases[0].note = "n".repeat(200);
        });
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Forwarding), cx);
        act(&view, Action::ToggleForwarding("sl".into()), cx);
        frame(cx);
        let list = cx.debug_bounds("forwarding-list").unwrap();
        let service = cx.debug_bounds("forwarding-sl").unwrap();
        let deliver = cx.debug_bounds("deliver-sl").unwrap();
        assert!(
            service.right() <= list.right() + px(1.),
            "{service:?} in {list:?}"
        );
        assert!(deliver.right() <= list.right(), "{deliver:?} in {list:?}");
        // The alias menu offers what the row no longer shows inline.
        view.update_in(cx, |this, _, _| {
            let labels: Vec<String> = this
                .alias_entries("sl", "1")
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    Entry::Separator => None,
                })
                .collect();
            assert_eq!(
                labels,
                [
                    "Copy Address",
                    "Edit Description…",
                    "Set Display Name…",
                    "Pin",
                    "Mailboxes…",
                    "Show Reverse Aliases",
                    "Show in Its Mailbox",
                    "Show as Its Own Account",
                    "Delete Alias…"
                ]
            );
            // Its mailboxes, checked where it forwards.
            let mailboxes: Vec<(String, bool)> = this
                .alias_mailbox_entries("sl", "1")
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { label, checked, .. } => Some((label.to_string(), *checked)),
                    Entry::Separator => None,
                })
                .collect();
            assert_eq!(
                mailboxes,
                [
                    ("work@example.com".to_string(), true),
                    ("home@example.com".to_string(), false)
                ]
            );
        });
        // Deleting asks first, in the row.
        act(&view, Action::DeleteAlias("sl".into(), "1".into()), cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.fwd_confirm.as_deref(), Some("sl:1"))
        });
        act(&view, Action::CancelForwardingConfirm, cx);
        view.read_with(cx, |this, _| assert!(this.fwd_confirm.is_none()));
    }

    #[gpui::test]
    fn reverse_aliases_open_as_a_subpage(cx: &mut TestAppContext) {
        let (view, cx, _) = setup(cx);
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Forwarding), cx);
        act(&view, Action::ToggleForwarding("sl".into()), cx);
        // As fetched from SimpleLogin: hundreds of them.
        let contacts: Vec<fwd::ReverseAlias> = (0..450)
            .map(|i| fwd::ReverseAlias {
                id: i.to_string(),
                contact: format!("person{i}@example.org"),
                address: format!("ra+secret{i}@simplelogin.co"),
                blocked: i == 3,
                last_sent: (i % 2 == 0).then_some(1_700_000_000 + i as i64),
            })
            .collect();
        view.update(cx, |this, _| {
            this.fwd_reverse = Some("sl:1".into());
            this.fwd_contacts.insert(
                "sl:1".into(),
                Ok(super::super::forwarding::Contacts::new(contacts, "")),
            );
        });
        frame(cx);
        let page = cx.debug_bounds("reverse-page").unwrap();
        let panel = cx.debug_bounds("reverse-aliases").expect("the panel shows");
        assert!(panel.right() <= page.right() + px(1.), "{panel:?} in {page:?}");
        assert!(cx.debug_bounds("reverse-back").is_some(), "a way back");
        // The alias is named in full, not squeezed to an ellipsis.
        let email = cx.debug_bounds("reverse-alias-email").expect("the alias is named");
        assert!(email.size.width > px(50.), "{email:?}");
        // Contacts sit on the page, as wide as it.
        assert!(panel.size.width > page.size.width - px(100.), "{panel:?} in {page:?}");
        // Only a page of contacts is laid out, not all 450 rows.
        assert!(
            panel.size.height < px(46. * 60.),
            "one page of contacts: {panel:?}"
        );
        view.read_with(cx, |this, _| {
            let Some(Ok(list)) = this.fwd_contacts.get("sl:1") else {
                panic!("listed");
            };
            // Most recently written first.
            assert_eq!(list.rows[0].contact.contact, "person448@example.org");
            assert_eq!(list.shown.len(), 450);
        });
        // The search narrows by the contact's address only.
        view.update(cx, |this, _| this.filter_contacts("person12"));
        view.read_with(cx, |this, _| {
            let Some(Ok(list)) = this.fwd_contacts.get("sl:1") else {
                panic!("listed");
            };
            assert!(list.shown.len() >= 11 && list.shown.len() < 450);
            assert!(
                list.shown
                    .iter()
                    .all(|&i| list.rows[i].contact.contact.contains("person12"))
            );
        });
        view.update(cx, |this, _| this.filter_contacts("secret12"));
        view.read_with(cx, |this, _| {
            let Some(Ok(list)) = this.fwd_contacts.get("sl:1") else {
                panic!("listed");
            };
            assert!(list.shown.is_empty(), "reverse aliases aren't searchable");
        });
        // A contact's menu offers what SimpleLogin allows.
        view.update_in(cx, |this, _, _| {
            let labels: Vec<String> = this
                .contact_entries("sl", "1", "3")
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    Entry::Separator => None,
                })
                .collect();
            assert_eq!(
                labels,
                ["Copy Reverse Alias", "Unblock Contact", "Delete Contact…"]
            );
        });
        // Deleting a contact asks first.
        act(
            &view,
            Action::DeleteContact("sl".into(), "1".into(), "3".into()),
            cx,
        );
        view.read_with(cx, |this, _| {
            assert_eq!(this.fwd_confirm.as_deref(), Some("contact:sl:1:3"))
        });
        act(&view, Action::CancelForwardingConfirm, cx);
        view.update_in(cx, |this, _, _| {
            let labels: Vec<String> = this
                .alias_entries("sl", "1")
                .iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    Entry::Separator => None,
                })
                .collect();
            assert!(labels.contains(&"Hide Reverse Aliases".to_string()));
        });
        // Closing hides it. (gpui keeps debug bounds of elements no longer
        // drawn, so the state is checked rather than the frame.)
        act(
            &view,
            Action::ToggleReverseAliases("sl".into(), "1".into()),
            cx,
        );
        view.read_with(cx, |this, _| assert_eq!(this.fwd_reverse, None));
    }

    #[gpui::test]
    fn leaving_reverse_aliases_returns_to_the_same_place(cx: &mut TestAppContext) {
        let (view, cx, _) = setup(cx);
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Forwarding), cx);
        act(&view, Action::ToggleForwarding("sl".into()), cx);
        // Enough aliases that the page scrolls.
        view.update(cx, |this, _| {
            let f = &mut this.forwarding[0];
            let first = f.aliases[0].clone();
            f.aliases = (0..60)
                .map(|i| Alias {
                    id: i.to_string(),
                    email: format!("a{i}@sl.io"),
                    ..first.clone()
                })
                .collect();
        });
        frame(cx);
        let target = view.update(cx, |this, _| {
            let max = this.settings_scroll.max_offset().height;
            assert!(max > px(100.), "the page scrolls: {max:?}");
            let y = -(max / 2.).round();
            this.settings_scroll.set_offset(point(px(0.), y));
            y
        });
        frame(cx);
        act(&view, Action::ToggleReverseAliases("sl".into(), "5".into()), cx);
        frame(cx);
        assert!(cx.debug_bounds("reverse-page").is_some(), "the subpage shows");
        // Scrolling the subpage leaves Forwarding's place alone.
        view.update(cx, |this, _| {
            this.fwd_reverse_scroll.set_offset(point(px(0.), px(-40.)))
        });
        frame(cx);
        act(&view, Action::ToggleReverseAliases("sl".into(), "5".into()), cx);
        frame(cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.fwd_reverse, None);
            assert_eq!(this.settings_scroll.offset().y, target);
        });
    }

    #[gpui::test]
    fn subscriptions_list_per_mailbox_and_in_all(cx: &mut TestAppContext) {
        let (view, cx, work) = setup(cx);
        act(&view, Action::OpenSubscriptions(work.clone()), cx);
        frame(cx);
        view.read_with(cx, |this, _| {
            assert!(this.subs_open);
            assert_eq!(this.subs.len(), 1, "{:?} {} {}", this.subs, this.status, this.subs_loading);
            assert_eq!(this.subs[0].address, "hello@thebrowser.com");
            assert_eq!(this.subs[0].count, 1);
            assert_eq!(this.subs[0].account, work);
        });
        assert!(cx.debug_bounds("subscription-list").is_some());
        assert!(cx.debug_bounds(Box::leak(format!("nav-{work}-subscriptions").into_boxed_str())).is_some());
        // Every mailbox's, in All Subscriptions.
        act(&view, Action::OpenSubscriptions(ALL_MAIL.into()), cx);
        view.read_with(cx, |this, _| assert_eq!(this.subs.len(), 2));
        // Opening a mailbox shows its mail again.
        act(&view, Action::OpenMailbox(work.clone(), Folder::Inbox.into()), cx);
        view.read_with(cx, |this, _| assert!(!this.subs_open));
        // An email from the subscription offers to unsubscribe; others don't.
        let id = format!("{work}:demo-9");
        act(&view, Action::Select(id), cx);
        frame(cx);
        assert!(cx.debug_bounds("unsubscribe-banner").is_some());
        // Once unsubscribed, it says so instead of asking again.
        view.update(cx, |this, _| {
            this.unsubscribed
                .insert(crate::manage::subscriptions::key(&work, "hello@thebrowser.com"));
        });
        let subs = view.read_with(cx, |this, _| this.subs.clone());
        let (done, other): (Vec<_>, Vec<_>) = subs
            .into_iter()
            .partition(|s| s.account == work && s.address == "hello@thebrowser.com");
        act(&view, Action::Unsubscribe(done[0].clone()), cx);
        view.read_with(cx, |this, _| assert!(this.unsubscribing.is_empty()));
        assert!(!cx.has_pending_prompt());
        // Its header was never signed (the sample mail has no signatures),
        // so nothing is sent or opened until the person agrees.
        let other = other[0].clone();
        act(&view, Action::Unsubscribe(other.clone()), cx);
        cx.run_until_parked();
        assert!(cx.has_pending_prompt(), "asks before unsubscribing");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert!(this.unsubscribing.is_empty());
            assert!(!this.unsubscribed.contains(&other.key()));
        });
    }
}
