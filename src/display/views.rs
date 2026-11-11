//! Views that list more than one mailbox, or part of one: All Mail across
//! every account, and aliases from forwarding services shown as accounts of
//! their own. Also the Forwarding settings page that manages those aliases.
use super::tree::Dragged;
use super::*;
use crate::manage::{backend::Layout, forwarding::Alias};

/// An alias's view has no Sent or Drafts: nothing is written from it.
const ALIAS_FOLDERS: [Folder; 5] = [
    Folder::Inbox,
    Folder::Starred,
    Folder::Archive,
    Folder::Junk,
    Folder::Trash,
];
/// Aliases listed per service before "Show all".
const ALIASES_SHOWN: usize = 25;

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
    fn view_mark(&self, icon: &'static str, size: f32, current: bool) -> Div {
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
    pub(super) fn refresh_aliases(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(account) = self.forwarding.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        if !self.fwd_busy.insert(id.clone()) {
            return;
        }
        self.task(
            cx,
            move |_| fwd::refresh(&account),
            move |this, result, cx| {
                this.fwd_busy.remove(&id);
                match result {
                    Ok(aliases) => {
                        if let Some(f) = this.forwarding.iter_mut().find(|f| f.id == id) {
                            f.aliases = aliases;
                        }
                        this.views_changed(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
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
        if !self.fwd_busy.insert(id.clone()) {
            return;
        }
        let note = Self::value(&self.fwd_note, cx).trim().to_owned();
        self.task(
            cx,
            move |_| fwd::create(&account, &note),
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
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
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
    /// A small bordered button for settings rows.
    fn small_button(
        &self,
        id: String,
        label: &str,
        icon: &str,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        self.button(id, label, icon, action, false, cx)
            .h(px(30.))
            .border_1()
            .border_color(rgb(c.border))
    }
    /// Forwarding services as a list: each row has its icon and details on
    /// the left and its actions on the right, and opens to manage its
    /// aliases. "Add Service" ends the list.
    pub(super) fn forwarding_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let adding = self.fwd_adding || self.forwarding.is_empty();
        let mut list = div()
            .id("forwarding-list")
            .debug_selector(|| "forwarding-list".into())
            // Stretched by the page: a percentage width resolves to the
            // content's width inside the scrolling page.
            .min_w_0()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .flex()
            .flex_col();
        for (i, f) in self.forwarding.iter().enumerate() {
            list = list.child(
                div()
                    .when(i > 0, |d| d.border_t_1().border_color(rgb(c.border)))
                    .child(self.forwarding_row(f, cx)),
            );
        }
        let add = div()
            .id("forwarding-add")
            .debug_selector(|| "forwarding-add".into())
            .h(px(52.))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(c.accent))
            .hover(move |s| s.bg(rgb(c.surface)))
            .when(!self.forwarding.is_empty(), |d| {
                d.border_t_1().border_color(rgb(c.border))
            })
            .child(
                div()
                    .size(px(32.))
                    .flex_shrink_0()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(c.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon_el(
                        if adding { "close" } else { "plus" },
                        14.,
                        c.accent,
                    )),
            )
            .child(div().flex_1().child(if self.forwarding.is_empty() {
                "Add a Service"
            } else if adding {
                "Cancel"
            } else {
                "Add Service"
            }))
            .when(!self.forwarding.is_empty(), |d| {
                d.on_click(cx.listener(|this, _, w, cx| this.handle(Action::AddForwarding, w, cx)))
            });
        list = list.child(add).when(adding, |d| {
            d.child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.surface))
                    .child(self.forwarding_form(cx)),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Forwarding"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Aliases from SimpleLogin, addy.io and Firefox Relay forward mail to one of your mailboxes. Choose whether each alias's mail stays in that mailbox or appears as an account of its own, without Sent or Drafts."),
            )
            .child(list)
            .into_any_element()
    }
    fn forwarding_form(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let service = self.fwd_service;
        let busy = self.fwd_busy.contains("connect");
        let (help, url) = service.key_help();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                self.segmented(
                    "forwarding-service",
                    ForwardingService::ALL
                        .into_iter()
                        .map(|s| {
                            (
                                s.label().to_owned(),
                                Action::ForwardingService(s),
                                s == service,
                            )
                        })
                        .collect(),
                    cx,
                )
                .max_w(px(420.)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(help)
                    .child(self.link(
                        "forwarding-key-help",
                        "Open",
                        Action::OpenUrl(url.into()),
                        cx,
                    )),
            )
            .child(
                div()
                    .id("forwarding-key")
                    .debug_selector(|| "forwarding-key".into())
                    .max_w(px(420.))
                    .child(field(Input::new(&self.fwd_key), c, busy)),
            )
            .child(div().id("forwarding-server").max_w(px(420.)).child(field(
                Input::new(&self.fwd_server),
                c,
                busy,
            )))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "Leave the server blank for {}. The key is kept in the system keychain.",
                        service.default_server()
                    )),
            )
            .when_some(self.fwd_error.clone(), |d, e| {
                d.child(
                    div()
                        .id("forwarding-error")
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .child(e),
                )
            })
            .child(
                div().flex().child(
                    self.button(
                        "forwarding-connect",
                        if busy { "Connecting…" } else { "Connect" },
                        "plus",
                        Action::ConnectForwarding,
                        true,
                        cx,
                    )
                    .when(busy, |d| d.opacity(0.6)),
                ),
            )
    }
    /// A square icon-only button with a tooltip.
    fn fwd_icon_button(
        &self,
        id: String,
        icon: &'static str,
        tip: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .size(px(30.))
            .flex_shrink_0()
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(c.surface)))
            .active(|s| s.opacity(0.75))
            .child(icon_el(icon, 15., c.muted))
            .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    /// One forwarding service in the list, and its aliases when open.
    fn forwarding_row(&self, f: &ForwardingAccount, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.fwd_busy.contains(&f.id);
        let removing = self.fwd_confirm.as_deref() == Some(f.id.as_str());
        let open = self.fwd_open.contains(&f.id);
        let server = fwd_server_label(f);
        let target = match &f.deliver_to {
            Some(id) => self
                .accounts
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.email.clone())
                .unwrap_or_else(|| "a removed mailbox".into()),
            None => "any mailbox".into(),
        };
        let toggle = Action::ToggleForwarding(f.id.clone());
        let info = div()
            .id(SharedString::from(format!("forwarding-info-{}", f.id)))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .child(self.view_mark("alias", 36., false))
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
                            .child(f.title()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(format!(
                                "{server} · {} alias{} · to {target}",
                                f.aliases.len(),
                                if f.aliases.len() == 1 { "" } else { "es" }
                            )),
                    ),
            )
            .on_click({
                let toggle = toggle.clone();
                cx.listener(move |this, _, w, cx| this.handle(toggle.clone(), w, cx))
            });
        let actions = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .child(if busy {
                div()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner(14., c.muted))
                    .into_any_element()
            } else {
                self.fwd_icon_button(
                    format!("aliases-refresh-{}", f.id),
                    "refresh",
                    "Refresh aliases",
                    Action::RefreshAliases(f.id.clone()),
                    cx,
                )
                .into_any_element()
            })
            .child(if removing {
                self.small_button(
                    format!("forwarding-remove-{}", f.id),
                    "Remove?",
                    "",
                    Action::RemoveForwarding(f.id.clone()),
                    cx,
                )
                .text_color(rgb(c.danger))
                .border_color(rgb(c.danger))
                .into_any_element()
            } else {
                self.fwd_icon_button(
                    format!("forwarding-remove-{}", f.id),
                    "trash",
                    "Remove service",
                    Action::RemoveForwarding(f.id.clone()),
                    cx,
                )
                .into_any_element()
            })
            .child(self.fwd_icon_button(
                format!("forwarding-toggle-{}", f.id),
                if open { "chevron-up" } else { "chevron-down" },
                if open {
                    "Hide aliases"
                } else {
                    "Manage aliases"
                },
                toggle,
                cx,
            ));
        let row = div()
            .min_h(px(60.))
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .child(info)
            .child(actions);
        div()
            .id(SharedString::from(format!("forwarding-{}", f.id)))
            .debug_selector({
                let id = f.id.clone();
                move || format!("forwarding-{id}")
            })
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(row)
            .when(open, |d| d.child(self.forwarding_details(f, cx)))
            .into_any_element()
    }
    /// Where a service delivers, and its aliases.
    fn forwarding_details(&self, f: &ForwardingAccount, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let busy = self.fwd_busy.contains(&f.id);
        let mut deliver: Vec<(String, Action, bool)> = vec![(
            "Any mailbox".into(),
            Action::SetDeliverTo(f.id.clone(), None),
            f.deliver_to.is_none(),
        )];
        deliver.extend(self.accounts.iter().map(|a| {
            (
                a.email.clone(),
                Action::SetDeliverTo(f.id.clone(), Some(a.id.clone())),
                f.deliver_to.as_deref() == Some(a.id.as_str()),
            )
        }));
        let creating = self.fwd_creating.as_deref() == Some(f.id.as_str());
        let mut aliases = div().flex().flex_col().gap_1();
        let expanded = self.fwd_expanded.contains(&f.id);
        let shown = if expanded {
            f.aliases.len()
        } else {
            f.aliases.len().min(ALIASES_SHOWN)
        };
        for a in &f.aliases[..shown] {
            aliases = aliases.child(self.alias_row(f, a, cx));
        }
        if f.aliases.is_empty() {
            aliases = aliases.child(
                div()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("No aliases yet."),
            );
        }
        if shown < f.aliases.len() {
            aliases = aliases.child(div().pt_1().child(self.link(
                &format!("aliases-all-{}", f.id),
                &format!("Show all {} aliases", f.aliases.len()),
                Action::ShowAllAliases(f.id.clone()),
                cx,
            )));
        }
        div()
            .px_4()
            .pb_4()
            .pt_1()
            .ml(px(48.))
            .flex()
            .flex_col()
            .gap_4()
            .child(self.setting(
                "Forwards to",
                "The mailbox these aliases deliver to. Any mailbox looks for their mail in every account.",
                self.segmented(&format!("deliver-{}", f.id), deliver, cx),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Aliases"),
                    )
                    .child(self.small_button(
                        format!("alias-new-{}", f.id),
                        if creating { "Cancel" } else { "New Alias" },
                        if creating { "" } else { "plus" },
                        Action::NewAlias(f.id.clone()),
                        cx,
                    )),
            )
            .when(creating, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("alias-note")
                                .flex_1()
                                .min_w(px(180.))
                                .child(field(Input::new(&self.fwd_note), c, busy)),
                        )
                        .child(
                            self.button(
                                format!("alias-create-{}", f.id),
                                if busy { "Creating…" } else { "Create" },
                                "",
                                Action::CreateAlias(f.id.clone()),
                                true,
                                cx,
                            )
                            .h(px(32.))
                            .when(busy, |d| d.opacity(0.6)),
                        ),
                )
            })
            .child(aliases)
    }
    fn alias_row(&self, f: &ForwardingAccount, a: &Alias, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let key = format!("{}:{}", f.id, a.id);
        let deleting = self.fwd_confirm.as_deref() == Some(key.as_str());
        let display = self
            .segmented(
                &format!("alias-display-{key}"),
                [
                    (AliasDisplay::Included, "In mailbox"),
                    (AliasDisplay::Separate, "Own account"),
                ]
                .into_iter()
                .map(|(d, label)| {
                    (
                        label.to_owned(),
                        Action::SetAliasDisplay(f.id.clone(), a.id.clone(), d),
                        a.display == d,
                    )
                })
                .collect(),
                cx,
            )
            .w(px(220.))
            .flex_shrink_0();
        let switch = self
            .switch(
                format!("alias-enabled-{key}"),
                a.enabled,
                Action::SetAliasEnabled(f.id.clone(), a.id.clone(), !a.enabled),
                cx,
            )
            .tooltip(|w, cx| Tooltip::new("Forward mail sent to this alias").build(w, cx));
        let icon_button = |id: String, icon: &'static str, tip: &'static str, action: Action| {
            div()
                .id(SharedString::from(id))
                .size(px(28.))
                .flex_shrink_0()
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(c.surface)))
                .child(icon_el(icon, 14., c.muted))
                .tooltip(move |w, cx| Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
        };
        div()
            .id(SharedString::from(format!("alias-{key}")))
            .w_full()
            .min_w_0()
            .py_2()
            .border_t_1()
            .border_color(rgb(c.border))
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
                    .gap(px(1.))
                    .when(!a.enabled, |d| d.opacity(0.6))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(a.email.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(match (a.enabled, a.note.is_empty()) {
                                (true, true) => "Forwarding".to_owned(),
                                (true, false) => a.note.clone(),
                                (false, true) => "Off: mail to it is refused".to_owned(),
                                (false, false) => format!("Off · {}", a.note),
                            }),
                    ),
            )
            .child(display)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(switch)
                    .child(icon_button(
                        format!("alias-copy-{key}"),
                        "copy",
                        "Copy address",
                        Action::CopyText(a.email.clone()),
                    ))
                    .child(if deleting {
                        div()
                            .id(SharedString::from(format!("alias-delete-{key}")))
                            .h(px(28.))
                            .px_2()
                            .rounded(px(7.))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(c.danger))
                            .border_1()
                            .border_color(rgb(c.danger))
                            .child("Delete?")
                            .on_click(cx.listener({
                                let action = Action::DeleteAlias(f.id.clone(), a.id.clone());
                                move |this, _, w, cx| this.handle(action.clone(), w, cx)
                            }))
                    } else {
                        icon_button(
                            format!("alias-delete-{key}"),
                            "trash",
                            "Delete alias",
                            Action::DeleteAlias(f.id.clone(), a.id.clone()),
                        )
                    }),
            )
            .into_any_element()
    }
}
fn fwd_server_label(f: &ForwardingAccount) -> String {
    let server = if f.server.is_empty() {
        f.service.default_server()
    } else {
        &f.server
    };
    server
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{act, frame, open, store};
    use super::*;
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
                server: String::new(),
                name: String::new(),
                deliver_to: Some(work.clone()),
                aliases: vec![Alias {
                    id: "1".into(),
                    email: "shop@sl.io".into(),
                    enabled: true,
                    note: String::new(),
                    display: AliasDisplay::Separate,
                }],
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
}
