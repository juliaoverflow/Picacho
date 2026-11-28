//! The attachment browser: every attachment of a mailbox, an account or
//! All Accounts in one list, in place of the message list and reader (an
//! `AppView`, from the sidebar, a mailbox's menu or the contact panel).
//! Attachments are filtered by kind, by words found in their names,
//! their messages' subjects and senders or in what they say (the
//! full-text index of `attachment_text`), and by a person; sorted;
//! previewed beside the list (pictures, and the text of documents, PDFs
//! and attached emails); and opened, saved, or picked and saved together.
//! The work is `manage::attachments`'s.
use super::*;
use crate::manage::attachments::{self as lib, Item, Kind, Sort};
use std::collections::HashSet;

/// A row's height; the list draws only the rows in view.
const ROW: f32 = 54.;
/// The preview's width beside the list.
const PREVIEW_WIDTH: f32 = 380.;
/// Below this width the preview takes the list's place.
const SIDE_BY_SIDE: f32 = 820.;
/// How long typing pauses before what attachments say is searched.
const CONTENT_PAUSE: Duration = Duration::from_millis(300);
/// The most of a document's text the preview shows.
const PREVIEW_TEXT: usize = 20_000;

/// What the preview shows of the focused attachment (its key first).
enum Preview {
    None,
    Loading(String),
    Image(String, Arc<gpui::Image>),
    Text(String, String),
    /// Nothing to show for this kind of file.
    Nothing(String),
    Failed(String, String),
}

impl Preview {
    fn key(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Loading(k) | Self::Image(k, _) | Self::Text(k, _) | Self::Nothing(k) | Self::Failed(k, _) => Some(k),
        }
    }
}

/// The browser's state.
pub(super) struct Library {
    /// Whose attachments: an account's id, or All Accounts'; empty before
    /// it was first opened.
    account: String,
    /// Only this mailbox's.
    mailbox: Option<Mailbox>,
    /// Only those from or to this address (lowercase).
    person: Option<String>,
    /// Every attachment loaded, in the `sort` order.
    items: Vec<Item>,
    /// How many of each kind there are.
    kinds: Vec<(Kind, usize)>,
    /// What the filters keep: indexes into `items`, and their total size.
    shown: Vec<usize>,
    shown_size: u64,
    /// Messages known to have attachments not listed yet (only headers
    /// downloaded).
    unlisted: usize,
    loading: bool,
    generation: u64,
    kind: Option<Kind>,
    sort: Sort,
    /// The messages whose attachments' text holds the words typed, and
    /// the text they were found for.
    content: Option<(String, Arc<HashSet<String>>)>,
    content_generation: u64,
    searching: bool,
    /// The attachments picked for saving together, by key.
    picked: HashSet<String>,
    /// The attachment previewed, by key, and what the preview shows.
    focused: Option<Item>,
    preview: Preview,
    saving: bool,
    pub(super) search: Entity<InputState>,
    scroll: UniformListScrollHandle,
}

impl Library {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            account: String::new(),
            mailbox: None,
            person: None,
            items: Vec::new(),
            kinds: Vec::new(),
            shown: Vec::new(),
            shown_size: 0,
            unlisted: 0,
            loading: false,
            generation: 0,
            kind: None,
            sort: Sort::default(),
            content: None,
            content_generation: 0,
            searching: false,
            picked: HashSet::new(),
            focused: None,
            preview: Preview::None,
            saving: false,
            search: input(t!("attachments-search"), false, window, cx),
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// What the browser does.
#[derive(Clone)]
pub(super) enum LibAction {
    /// Opens the browser over an account's attachments (or All Accounts'),
    /// one mailbox's where given, and one person's (an address) where given.
    Open {
        account: String,
        mailbox: Option<Mailbox>,
        person: Option<String>,
    },
    /// The menu of whose attachments to browse.
    ScopeMenu,
    /// Shows only one kind, or every kind.
    Kind(Option<Kind>),
    SortMenu,
    Sort(Sort),
    /// Previews an attachment.
    Focus(Item),
    /// Closes the preview.
    Unfocus,
    /// Picks an attachment for saving together, or unpicks it (its key).
    Pick(String),
    /// Picks every attachment shown, or none.
    PickAll,
    PickNone,
    /// Saves the picked attachments into a folder chosen.
    SavePicked,
    /// One attachment: saved where chosen, opened with its app, shown in
    /// its folder, or its message opened.
    Save(Item),
    OpenFile(Item),
    Reveal(Item),
    ShowMessage(Item),
    /// Every person's attachments again, or every mailbox's.
    ClearPerson,
    ClearMailbox,
}

impl MailApp {
    pub(super) fn library_action(&mut self, action: LibAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            LibAction::Open { account, mailbox, person } => {
                let changed = self.library.account != account
                    || self.library.mailbox != mailbox
                    || self.library.person != person;
                self.library.account = account;
                self.library.mailbox = mailbox;
                self.library.person = person.map(|p| p.trim().to_lowercase());
                if changed {
                    self.library.kind = None;
                    self.library.picked.clear();
                    self.library_unfocus();
                }
                self.cal_action(calendar_views::CalAction::Open(calendar_views::AppView::Attachments), w, cx);
            }
            LibAction::ScopeMenu => {
                let entries = self.library_scope_entries();
                self.open_menu_here(entries, w, cx);
            }
            LibAction::Kind(kind) => {
                self.library.kind = kind;
                self.library_refilter(cx);
                self.library.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            LibAction::SortMenu => {
                let entries = Sort::ALL
                    .into_iter()
                    .map(|s| checked(sort_label(s), self.library.sort == s, Action::Attachments(LibAction::Sort(s))))
                    .collect();
                self.open_menu_here(entries, w, cx);
            }
            LibAction::Sort(sort) => {
                self.library.sort = sort;
                lib::sort(&mut self.library.items, sort);
                self.library_refilter(cx);
                self.library.scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            LibAction::Focus(item) => self.library_focus(item, cx),
            LibAction::Unfocus => self.library_unfocus(),
            LibAction::Pick(key) => {
                if !self.library.picked.remove(&key) {
                    self.library.picked.insert(key);
                }
            }
            LibAction::PickAll => {
                let keys: Vec<String> = self.library.shown.iter().map(|&i| self.library.items[i].key()).collect();
                self.library.picked.extend(keys);
            }
            LibAction::PickNone => self.library.picked.clear(),
            LibAction::SavePicked => self.library_save_picked(cx),
            LibAction::Save(item) => self.library_save(item, cx),
            LibAction::OpenFile(item) => self.library_open_file(item, false, cx),
            LibAction::Reveal(item) => self.library_open_file(item, true, cx),
            LibAction::ShowMessage(item) => {
                // Its mailbox may be the one open under the browser, which
                // opening it again leaves as it is.
                self.app_view = None;
                let m = &item.mail;
                self.handle(Action::OpenSummaryMail(m.account.clone(), m.mailbox(), m.id.clone()), w, cx);
            }
            LibAction::ClearPerson => {
                self.library.person = None;
                self.library_refilter(cx);
            }
            LibAction::ClearMailbox => {
                self.library.mailbox = None;
                self.library.picked.clear();
                self.library_unfocus();
                self.load_library(cx);
            }
        }
        cx.notify();
    }

    /// The accounts whose attachments are browsed: the workspace's for All
    /// Accounts.
    fn library_accounts(&self) -> Vec<String> {
        if self.library.account == ALL_MAIL {
            self.workspace_accounts().into_iter().map(|a| a.id).collect()
        } else {
            self.accounts
                .iter()
                .filter(|a| a.id == self.library.account)
                .map(|a| a.id.clone())
                .collect()
        }
    }

    /// Loads the attachments of the browser's mail, as the browser opens.
    pub(super) fn load_library(&mut self, cx: &mut Context<Self>) {
        // First opened, or its account since removed: the account open,
        // else All Accounts.
        let known = self.library.account == ALL_MAIL || self.accounts.iter().any(|a| a.id == self.library.account);
        if !known {
            self.library.account = match self.account.as_deref() {
                Some(id) if self.accounts.iter().any(|a| a.id == id) => id.to_owned(),
                _ => ALL_MAIL.to_owned(),
            };
            self.library.mailbox = None;
        }
        self.library.generation += 1;
        let generation = self.library.generation;
        if self.accounts.is_empty() {
            // The sample mailbox, before any account is added.
            let mails = self.demo.iter().filter(|m| !m.attachments().is_empty()).cloned().collect();
            self.library_loaded(mails, 0, cx);
            return;
        }
        let accounts = self.library_accounts();
        let mailbox = self.library.mailbox.clone();
        self.library.loading = true;
        self.task(
            cx,
            move |b| b.store.attachment_mail(&accounts, mailbox.as_ref(), lib::LIBRARY_LIMIT),
            move |this, result, cx| {
                if generation != this.library.generation {
                    return;
                }
                this.library.loading = false;
                match result {
                    Ok((mails, unlisted)) => {
                        this.library_loaded(mails, unlisted, cx);
                        // Words typed before: what attachments say, again.
                        this.library_search_changed(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
                cx.notify();
            },
        );
    }

    fn library_loaded(&mut self, mails: Vec<Mail>, unlisted: usize, cx: &App) {
        let mut items = lib::items(mails);
        lib::sort(&mut items, self.library.sort);
        let mut counts: HashMap<Kind, usize> = HashMap::new();
        for item in &items {
            *counts.entry(item.kind()).or_default() += 1;
        }
        self.library.kinds = Kind::ALL.into_iter().filter_map(|k| Some((k, *counts.get(&k)?))).collect();
        if self.library.kind.is_some_and(|k| !counts.contains_key(&k)) {
            self.library.kind = None;
        }
        self.library.items = items;
        self.library.unlisted = unlisted;
        // A focused attachment no longer listed (its message moved away)
        // is let go.
        if let Some(focused) = &self.library.focused {
            let key = focused.key();
            if !self.library.items.iter().any(|i| i.key() == key) {
                self.library_unfocus();
            }
        }
        self.library_refilter(cx);
    }

    /// Works out what the filters keep, once something they go by changed.
    pub(super) fn library_refilter(&mut self, cx: &App) {
        let text = self.library.search.read(cx).value().to_string();
        self.library_refilter_with(Some(text));
    }

    fn library_refilter_with(&mut self, text: Option<String>) {
        let text = text.unwrap_or_default();
        let lib_state = &mut self.library;
        let content = lib_state
            .content
            .as_ref()
            .filter(|(q, _)| q == text.trim())
            .map(|(_, ids)| ids.clone());
        let filter = lib::Filter {
            kind: lib_state.kind,
            words: lib::words(&text),
            content,
            person: lib_state.person.clone(),
        };
        lib_state.shown = (0..lib_state.items.len()).filter(|&i| filter.keeps(&lib_state.items[i])).collect();
        lib_state.shown_size = lib_state.shown.iter().map(|&i| lib_state.items[i].attachment().size as u64).sum();
        // Only what is shown stays picked: a save never takes what can't
        // be seen.
        let shown: HashSet<String> = lib_state.shown.iter().map(|&i| lib_state.items[i].key()).collect();
        lib_state.picked.retain(|k| shown.contains(k));
    }

    /// The search field changed: names and senders are matched at once,
    /// what attachments say once typing pauses.
    pub(super) fn library_search_changed(&mut self, cx: &mut Context<Self>) {
        self.library_refilter(cx);
        let text = self.library.search.read(cx).value().trim().to_owned();
        let words = lib::words(&text);
        self.library.content_generation += 1;
        let generation = self.library.content_generation;
        if words.is_empty() || self.accounts.is_empty() {
            self.library.content = None;
            self.library.searching = false;
            cx.notify();
            return;
        }
        if self.library.content.as_ref().is_some_and(|(q, _)| *q == text) {
            return;
        }
        self.library.searching = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONTENT_PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.library.content_generation {
                    return;
                }
                this.task(
                    cx,
                    move |b| b.store.attachment_text_matches(&words, lib::CONTENT_LIMIT),
                    move |this, result, cx| {
                        if generation != this.library.content_generation {
                            return;
                        }
                        this.library.searching = false;
                        match result {
                            Ok(ids) => {
                                this.library.content = Some((text, Arc::new(ids)));
                                this.library_refilter(cx);
                            }
                            Err(e) => this.notice(format!("{e:#}"), true, cx),
                        }
                    },
                );
            });
        })
        .detach();
        cx.notify();
    }

    fn library_unfocus(&mut self) {
        self.library.focused = None;
        self.library_set_preview(Preview::None);
    }

    /// Shows `preview`, giving back the picture shown before.
    fn library_set_preview(&mut self, preview: Preview) {
        if let Preview::Image(_, image) = std::mem::replace(&mut self.library.preview, preview) {
            crate::display::textures::retire_image(image);
        }
    }

    /// Previews an attachment: a picture shown, a document's text, or
    /// nothing for other kinds of file.
    fn library_focus(&mut self, item: Item, cx: &mut Context<Self>) {
        let key = item.key();
        if self.library.focused.as_ref().is_some_and(|f| f.key() == key) {
            return;
        }
        self.library.focused = Some(item.clone());
        let a = item.attachment().clone();
        let Some(account) = self.accounts.iter().find(|x| x.id == item.mail.account).cloned() else {
            self.library_set_preview(Preview::Nothing(key));
            return;
        };
        if super::tools::is_image(&a) {
            // Told before anything downloads.
            if a.size > super::tools::MAX_PREVIEW_FILE {
                self.library_set_preview(Preview::Failed(key, t!("tools-this-picture-is-too-large-to-preview").into()));
                return;
            }
            self.library_set_preview(Preview::Loading(key.clone()));
            self.task(
                cx,
                move |b| {
                    let bytes = b.library_attachment(&account, &item)?;
                    // Its listed size may be an estimate.
                    anyhow::ensure!(
                        bytes.len() <= super::tools::MAX_PREVIEW_FILE,
                        t!("tools-this-picture-is-too-large-to-preview")
                    );
                    super::tools::decode_image_preview(bytes, &a.mime, &a.name)
                },
                move |this, result, cx| {
                    if this.library.preview.key() != Some(key.as_str()) {
                        return;
                    }
                    let preview = match result {
                        Ok((format, bytes)) => Preview::Image(key, Arc::new(gpui::Image::from_bytes(format, bytes))),
                        Err(e) => Preview::Failed(key, format!("{e:#}")),
                    };
                    this.library_set_preview(preview);
                    cx.notify();
                },
            );
        } else if item.readable() {
            self.library_set_preview(Preview::Loading(key.clone()));
            self.task(
                cx,
                move |b| b.attachment_preview_text(&account, &item),
                move |this, result, cx| {
                    if this.library.preview.key() != Some(key.as_str()) {
                        return;
                    }
                    let preview = match result {
                        Ok(text) if text.trim().is_empty() => Preview::Nothing(key),
                        Ok(mut text) => {
                            if text.len() > PREVIEW_TEXT {
                                let mut at = PREVIEW_TEXT;
                                while !text.is_char_boundary(at) {
                                    at -= 1;
                                }
                                text.truncate(at);
                                text.push('…');
                            }
                            Preview::Text(key, text)
                        }
                        Err(e) => Preview::Failed(key, format!("{e:#}")),
                    };
                    this.library_set_preview(preview);
                    cx.notify();
                },
            );
        } else {
            self.library_set_preview(Preview::Nothing(key));
        }
    }

    /// Opens an attachment with the system's app for it, or shows it in
    /// its folder (`reveal`), as the reader's Open and Show in Folder do.
    pub(super) fn library_open_file(&mut self, item: Item, reveal: bool, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.iter().find(|a| a.id == item.mail.account).cloned() else {
            return;
        };
        self.task(
            cx,
            move |b| b.library_attachment_file(&account, &item),
            move |this, result: anyhow::Result<std::path::PathBuf>, cx| match result {
                Ok(path) if reveal => cx.reveal_path(&path),
                // A program or script from an email isn't run with one
                // click: it's shown in its folder instead.
                Ok(path)
                    if path
                        .file_name()
                        .is_some_and(|n| crate::manage::backend::opened::runs_code(&n.to_string_lossy())) =>
                {
                    cx.reveal_path(&path);
                    this.notice(t!("tools-programs-arent-opened"), true, cx);
                }
                Ok(path) => cx.open_with_system(&path),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    fn library_save(&mut self, item: Item, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.iter().find(|a| a.id == item.mail.account).cloned() else {
            return;
        };
        let name = safe_file_name(&item.attachment().name);
        let picker = cx.prompt_for_new_path(&self.save_start(), Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    this.task(
                        cx,
                        move |b| b.save_library_attachment(&account, &item, &path),
                        |this, result, cx| match result {
                            Ok(()) => this.notice(t!("ui-attachment-saved"), false, cx),
                            Err(e) => this.notice(format!("{e:#}"), true, cx),
                        },
                    )
                });
            }
        })
        .detach();
    }

    /// Saves the picked attachments into a folder chosen, each under its
    /// own name, numbered where names repeat.
    fn library_save_picked(&mut self, cx: &mut Context<Self>) {
        if self.library.saving {
            return;
        }
        let todo: Vec<(Account, Item)> = self
            .library
            .shown
            .iter()
            .map(|&i| &self.library.items[i])
            .filter(|i| self.library.picked.contains(&i.key()))
            .filter_map(|i| Some((self.accounts.iter().find(|a| a.id == i.mail.account)?.clone(), i.clone())))
            .collect();
        if todo.is_empty() {
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("attachments-save-here").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut dirs))) = picker.await else { return };
            let Some(dir) = dirs.pop() else { return };
            let _ = this.update(cx, |this, cx| {
                this.library.saving = true;
                let count = todo.len();
                this.notice(t!("attachments-saving", count = count, shown = fmt_count(count)), false, cx);
                this.task(
                    cx,
                    move |b| Ok(b.save_attachments(&todo, &dir)),
                    move |this, result: anyhow::Result<lib::Saved>, cx| {
                        this.library.saving = false;
                        match result {
                            Ok(saved) if saved.failed.is_empty() => {
                                this.library.picked.clear();
                                this.notice(
                                    t!("attachments-saved", count = saved.saved, shown = fmt_count(saved.saved)),
                                    false,
                                    cx,
                                );
                            }
                            Ok(saved) => {
                                let (name, why) = &saved.failed[0];
                                this.notice(
                                    t!(
                                        "attachments-saved-some",
                                        saved = fmt_count(saved.saved),
                                        count = count,
                                        shown = fmt_count(count),
                                        name = name.clone(),
                                        e = why.clone()
                                    ),
                                    true,
                                    cx,
                                );
                            }
                            Err(e) => this.notice(format!("{e:#}"), true, cx),
                        }
                    },
                );
            });
        })
        .detach();
    }

    /// Whose attachments can be browsed: All Accounts (the workspace's),
    /// each account, and the whole account rather than one mailbox.
    fn library_scope_entries(&self) -> Vec<Entry> {
        let person = self.library.person.clone();
        let open = |account: &str| {
            Action::Attachments(LibAction::Open {
                account: account.to_owned(),
                mailbox: None,
                person: person.clone(),
            })
        };
        let whole = self.library.mailbox.is_none();
        let mut entries = Vec::new();
        let accounts = self.workspace_accounts();
        if accounts.len() > 1 {
            entries.push(checked(
                t!("attachments-all-accounts"),
                whole && self.library.account == ALL_MAIL,
                open(ALL_MAIL),
            ));
            entries.push(Entry::Separator);
        }
        for a in accounts {
            entries.push(checked(a.label(), whole && self.library.account == a.id, open(&a.id)));
        }
        entries
    }

    /// An attachment's menu.
    fn library_item_entries(&self, item: &Item) -> Vec<Entry> {
        let act = |a: LibAction| Action::Attachments(a);
        let picked = self.library.picked.contains(&item.key());
        let mut entries = vec![
            entry(t!("tools-open"), "external", act(LibAction::OpenFile(item.clone()))),
            entry(t!("tools-save"), "download", act(LibAction::Save(item.clone()))),
        ];
        if !self.mobile.on {
            entries.push(entry(
                if cfg!(target_os = "macos") { t!("tools-show-in-finder") } else { t!("tools-show-in-folder") },
                "folder",
                act(LibAction::Reveal(item.clone())),
            ));
        }
        entries.push(entry(t!("attachments-show-message"), "mail", act(LibAction::ShowMessage(item.clone()))));
        entries.push(Entry::Separator);
        entries.push(checked(t!("attachments-pick"), picked, act(LibAction::Pick(item.key()))));
        entries
    }

    /// What the scope button says: an account's name or All Accounts, and
    /// the mailbox.
    fn library_scope_label(&self) -> String {
        let who = if self.library.account == ALL_MAIL || self.accounts.is_empty() {
            t!("attachments-all-accounts").to_owned()
        } else {
            self.accounts
                .iter()
                .find(|a| a.id == self.library.account)
                .map(|a| a.label().to_owned())
                .unwrap_or_default()
        };
        match &self.library.mailbox {
            None => who,
            Some(Mailbox::Standard(f)) => format!("{who} · {}", f.label()),
            Some(Mailbox::Remote(name)) => format!("{who} · {}", self.folder_label(&self.library.account, name)),
        }
    }

    /// The browser, in place of the message list and the reader; `mobile`
    /// on a phone's screen of its own, under its bar.
    pub(super) fn attachments_page(&self, corners: Corners<Pixels>, mobile: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let room = if mobile {
            0.
        } else {
            let window = self.window_width.get();
            window - self.layout(window, self.compact.get()).sidebar
        };
        let side_by_side = !mobile && room >= SIDE_BY_SIDE;
        let previewing = self.library.focused.is_some();
        let list = self.library_list(cx);
        let body = div().flex().flex_1().min_h_0().min_w_0().map(|d| {
            if side_by_side {
                d.child(list).when(previewing, |d| {
                    d.child(
                        div()
                            .w(px(PREVIEW_WIDTH))
                            .flex_shrink_0()
                            .h_full()
                            .border_l_1()
                            .border_color(rgb(c.border))
                            .child(self.library_preview(cx)),
                    )
                })
            } else if previewing {
                d.child(self.library_preview(cx))
            } else {
                d.child(list)
            }
        });
        div()
            .id("attachments-page")
            .debug_selector(|| "attachments-page".into())
            .role(Role::Main)
            .aria_label(t!("attachments-attachments"))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .when(!mobile, |d| d.child(self.library_title(cx)))
            .child(self.library_controls(cx))
            .child(body)
            .into_any_element()
    }

    fn library_title(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        div()
            .h(px(57.))
            .flex_shrink_0()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(icon_el("paperclip", 18., c.accent))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("attachments-attachments")),
            )
            .when(self.library.loading, |d| d.child(spinner(13., c.muted)))
            .child(div().flex_1())
            .child(self.button(
                "attachments-close",
                t!("calendar-views-back-to-mail"),
                "close",
                Action::Calendar(calendar_views::CalAction::Close),
                false,
                cx,
            ))
    }

    /// Whose attachments, the search, the sort, the kinds, the person and
    /// what is picked.
    fn library_controls(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let lib_state = &self.library;
        let chip = |id: String, label: String, icon: &'static str, on: bool, action: Action| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .role(Role::Button)
                .aria_label(label.clone())
                .h(px(26.))
                .px_3()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .bg(rgb(if on { c.soft } else { c.surface }))
                .text_color(rgb(if on { c.text } else { c.muted }))
                .border_1()
                .border_color(rgb(if on { c.soft } else { c.border }))
                .hover(move |s| s.bg(rgb(if on { c.soft } else { c.raised })))
                .when(!icon.is_empty(), |d| d.child(icon_el(icon, 13., if on { c.accent } else { c.muted })))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
        };
        let menu_button = |id: &'static str, label: String, action: Action| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .role(Role::Button)
                .aria_label(label.clone())
                .h(px(32.))
                .px_3()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(c.border))
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(rgb(c.text))
                .hover(move |s| s.bg(rgb(c.raised)))
                .child(div().max_w(px(260.)).truncate().child(label))
                .child(icon_el("chevrons-up-down", 13., c.muted))
                .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
        };
        let shown = lib_state.shown.len();
        let picked = lib_state.picked.len();
        let summary = if lib_state.loading && lib_state.items.is_empty() {
            t!("attachments-loading").to_owned()
        } else {
            t!("attachments-count", count = shown, shown = fmt_count(shown), size = human_size(lib_state.shown_size))
        };
        div()
            .flex_shrink_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(menu_button(
                        "attachments-scope",
                        self.library_scope_label(),
                        Action::Attachments(LibAction::ScopeMenu),
                    ))
                    .child(
                        div()
                            .id("attachments-search")
                            .flex_1()
                            .min_w(px(180.))
                            .max_w(px(420.))
                            .child(field(Input::new(&lib_state.search), c, false)),
                    )
                    .when(lib_state.searching, |d| d.child(spinner(13., c.muted)))
                    .child(menu_button(
                        "attachments-sort",
                        sort_label(lib_state.sort).to_owned(),
                        Action::Attachments(LibAction::SortMenu),
                    )),
            )
            .when(!lib_state.kinds.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .child(chip(
                            "attachments-kind-all".into(),
                            t!("attachments-all-kinds").into(),
                            "",
                            lib_state.kind.is_none(),
                            Action::Attachments(LibAction::Kind(None)),
                        ))
                        .children(lib_state.kinds.iter().map(|&(kind, n)| {
                            let on = lib_state.kind == Some(kind);
                            chip(
                                format!("attachments-kind-{}", kind_key(kind)),
                                format!("{} {}", kind_label(kind), fmt_count(n)),
                                kind.icon(),
                                on,
                                Action::Attachments(LibAction::Kind(if on { None } else { Some(kind) })),
                            )
                        })),
                )
            })
            .when(lib_state.person.is_some() || lib_state.mailbox.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(lib_state.person.as_ref().map(|p| {
                            chip(
                                "attachments-person".into(),
                                t!("attachments-with-person", person = p.clone()),
                                "close",
                                true,
                                Action::Attachments(LibAction::ClearPerson),
                            )
                        }))
                        .children(lib_state.mailbox.as_ref().map(|_| {
                            chip(
                                "attachments-every-mailbox".into(),
                                t!("attachments-every-mailbox").into(),
                                "close",
                                true,
                                Action::Attachments(LibAction::ClearMailbox),
                            )
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(28.))
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .map(|d| {
                        if picked > 0 {
                            d.child(
                                div()
                                    .text_color(rgb(c.text))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t!("attachments-picked", count = picked, shown = fmt_count(picked))),
                            )
                            .child(
                                self.button(
                                    "attachments-save-picked",
                                    if lib_state.saving { t!("attachments-saving-short") } else { t!("attachments-save-picked") },
                                    "save-all",
                                    Action::Attachments(LibAction::SavePicked),
                                    true,
                                    cx,
                                )
                                .h(px(28.))
                                .text_size(px(12.)),
                            )
                            .child(self.link("attachments-pick-none", t!("attachments-pick-none"), Action::Attachments(LibAction::PickNone), cx))
                        } else {
                            d.child(div().child(summary)).when(shown > 0, |d| {
                                d.child(self.link(
                                    "attachments-pick-all",
                                    t!("attachments-pick-all"),
                                    Action::Attachments(LibAction::PickAll),
                                    cx,
                                ))
                            })
                        }
                    }),
            )
            .when(lib_state.unlisted > 0, |d| {
                d.child(
                    div()
                        .id("attachments-unlisted")
                        .debug_selector(|| "attachments-unlisted".into())
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(11.5))
                        .text_color(rgb(c.muted))
                        .child(icon_el("info", 13., c.muted))
                        .child(t!(
                            "attachments-unlisted",
                            count = lib_state.unlisted,
                            shown = fmt_count(lib_state.unlisted)
                        )),
                )
            })
    }

    fn library_list(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let count = self.library.shown.len();
        if count == 0 {
            let filtered = !self.library.items.is_empty();
            return div()
                .id("attachments-empty")
                .debug_selector(|| "attachments-empty".into())
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child(if self.library.loading {
                    spinner(20., c.muted).into_any_element()
                } else {
                    icon_el("paperclip", 28., c.border).into_any_element()
                })
                .child(if self.library.loading {
                    t!("attachments-loading")
                } else if filtered {
                    t!("attachments-none-match")
                } else {
                    t!("attachments-none")
                })
                .into_any_element();
        }
        let entity = cx.entity();
        uniform_list("attachment-list", count, move |range, _, cx| {
            entity.update(cx, |this, cx| {
                let cx: &Context<Self> = cx;
                range
                    .filter_map(|i| {
                        let item = this.library.items.get(*this.library.shown.get(i)?)?;
                        Some(this.library_row(i, item, cx))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(&self.library.scroll)
        .flex_1()
        .min_h_0()
        .map(|list| panes::scroll_frame(list, &self.library.scroll))
        .into_any_element()
    }

    fn library_row(&self, index: usize, item: &Item, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let a = item.attachment();
        let m = &item.mail;
        let key = item.key();
        let picked = self.library.picked.contains(&key);
        let focused = self.library.focused.as_ref().is_some_and(|f| f.key() == key);
        let who = if m.sender.trim().is_empty() { m.from.clone() } else { m.sender.clone() };
        let mut detail = if m.subject.trim().is_empty() { who } else { format!("{who} · {}", m.subject) };
        if self.library.account == ALL_MAIL
            && self.accounts.len() > 1
            && let Some(account) = self.accounts.iter().find(|x| x.id == m.account)
        {
            detail = format!("{detail} · {}", account.label());
        }
        let (focus, menu, pick) = (item.clone(), item.clone(), key.clone());
        let name = a.name.clone();
        div()
            .id(SharedString::from(format!("attachment-row-{index}")))
            .debug_selector(move || format!("attachment-row-{index}"))
            .role(Role::Button)
            .aria_label(name.clone())
            .h(px(ROW))
            .w_full()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .bg(rgb(if focused { c.soft } else { c.surface }))
            .hover(move |s| s.bg(rgb(if focused { c.soft } else { c.bg })))
            .when(index > 0, |d| d.border_t_1().border_color(rgb(c.border)))
            .on_click(cx.listener(move |this, _, w, cx| {
                this.handle(Action::Attachments(LibAction::Focus(focus.clone())), w, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let entries = this.library_item_entries(&menu);
                    this.open_menu(e.position, entries, cx)
                }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("attachment-pick-{index}")))
                    .debug_selector(move || format!("attachment-pick-{index}"))
                    .role(Role::CheckBox)
                    .aria_label(t!("attachments-pick"))
                    .size(px(28.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .hover(move |s| s.bg(rgb(c.raised)))
                    .child(icon_el(if picked { "square-check" } else { "square" }, 16., if picked { c.accent } else { c.muted }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.handle(Action::Attachments(LibAction::Pick(pick.clone())), w, cx)
                    })),
            )
            .child(icon_el(item.kind().icon(), 20., c.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).truncate().child(name))
                    .child(div().text_size(px(11.5)).text_color(rgb(c.muted)).truncate().child(detail)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .items_end()
                    .gap(px(2.))
                    .text_size(px(11.5))
                    .text_color(rgb(c.muted))
                    .child(date_short(m.date))
                    .child(human_size(a.size as u64)),
            )
            .into_any_element()
    }

    /// The focused attachment: what it is, what can be done with it, and
    /// its picture or text.
    fn library_preview(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(item) = &self.library.focused else {
            return div().into_any_element();
        };
        let a = item.attachment();
        let m = &item.mail;
        let act = |a: LibAction| Action::Attachments(a);
        let who = if m.sender.trim().is_empty() { m.from.clone() } else { format!("{} <{}>", m.sender, m.from) };
        let body: AnyElement = match &self.library.preview {
            Preview::Loading(_) | Preview::None => div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .py_8()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(spinner(18., c.muted))
                .child(t!("attachments-loading-preview"))
                .into_any_element(),
            Preview::Image(_, image) => div()
                .flex()
                .justify_center()
                .child(img(image.clone()).max_w_full().max_h(px(520.)).object_fit(ObjectFit::Contain))
                .into_any_element(),
            Preview::Text(_, text) => div()
                .id("attachment-preview-text")
                .debug_selector(|| "attachment-preview-text".into())
                .p_3()
                .rounded_lg()
                .bg(rgb(c.raised))
                .text_size(px(12.))
                .line_height(relative(1.5))
                .whitespace_normal()
                .children(text.lines().take(600).map(|l| div().min_h(px(8.)).child(l.to_owned())).collect::<Vec<_>>())
                .into_any_element(),
            Preview::Nothing(_) => div()
                .py_6()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(icon_el(item.kind().icon(), 32., c.border))
                .child(t!("attachments-no-preview"))
                .into_any_element(),
            Preview::Failed(_, why) => div()
                .py_4()
                .text_size(px(12.))
                .text_color(rgb(c.danger))
                .child(why.clone())
                .into_any_element(),
        };
        div()
            .id("attachment-preview")
            .debug_selector(|| "attachment-preview".into())
            .role(Role::Region)
            .aria_label(t!("attachments-preview"))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(icon_el(item.kind().icon(), 20., c.accent))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .whitespace_normal()
                                    .child(a.name.clone()),
                            )
                            .child(
                                self.button(
                                    "attachment-preview-close",
                                    t!("attachments-close-preview"),
                                    "close",
                                    act(LibAction::Unfocus),
                                    false,
                                    cx,
                                )
                                .w(px(30.))
                                .h(px(30.))
                                .px_0(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .child(format!("{} · {}", human_size(a.size as u64), kind_label(item.kind()))),
                    )
                    .child(div().text_size(px(12.)).truncate().child(who))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(format!("{} · {}", date_full(m.date), m.subject)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(self.button("attachment-preview-open", t!("tools-open"), "external-link", act(LibAction::OpenFile(item.clone())), false, cx).border_1().border_color(rgb(c.border)))
                            .child(self.button("attachment-preview-save", t!("tools-save"), "download", act(LibAction::Save(item.clone())), false, cx).border_1().border_color(rgb(c.border)))
                            .child(self.button("attachment-preview-message", t!("attachments-show-message"), "mail", act(LibAction::ShowMessage(item.clone())), false, cx).border_1().border_color(rgb(c.border))),
                    ),
            )
            .child(
                div()
                    .id("attachment-preview-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
impl MailApp {
    /// The names of the attachments shown, in order.
    pub(super) fn library_shown_names(&self) -> Vec<String> {
        self.library.shown.iter().map(|&i| self.library.items[i].attachment().name.clone()).collect()
    }
    pub(super) fn library_picked(&self) -> usize {
        self.library.picked.len()
    }
    /// The attachment loaded with this name.
    pub(super) fn library_item(&self, name: &str) -> Item {
        self.library.items.iter().find(|i| i.attachment().name == name).cloned().expect(name)
    }
}

fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::Newest => t!("attachments-newest-first"),
        Sort::Oldest => t!("attachments-oldest-first"),
        Sort::Largest => t!("attachments-largest-first"),
        Sort::Name => t!("attachments-by-name"),
    }
}

fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "image",
        Kind::Pdf => "pdf",
        Kind::Document => "document",
        Kind::Spreadsheet => "spreadsheet",
        Kind::Presentation => "presentation",
        Kind::Archive => "archive",
        Kind::Audio => "audio",
        Kind::Video => "video",
        Kind::Calendar => "calendar",
        Kind::Email => "email",
        Kind::Other => "other",
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => t!("attachments-kind-images"),
        Kind::Pdf => t!("attachments-kind-pdfs"),
        Kind::Document => t!("attachments-kind-documents"),
        Kind::Spreadsheet => t!("attachments-kind-spreadsheets"),
        Kind::Presentation => t!("attachments-kind-presentations"),
        Kind::Archive => t!("attachments-kind-archives"),
        Kind::Audio => t!("attachments-kind-audio"),
        Kind::Video => t!("attachments-kind-video"),
        Kind::Calendar => t!("attachments-kind-calendar"),
        Kind::Email => t!("attachments-kind-emails"),
        Kind::Other => t!("attachments-kind-other"),
    }
}
