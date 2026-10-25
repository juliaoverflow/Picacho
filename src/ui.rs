use crate::{
    mail::{Backend, Change},
    model::*,
    store::Store,
};
use gpui::{prelude::*, *};
use gpui_component::{
    Icon, IconName, Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState},
    tooltip::Tooltip,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

actions!(
    mail,
    [
        Compose,
        Refresh,
        Search,
        Close,
        Next,
        Previous,
        Archive,
        DeleteMail,
        Reply,
        ToggleStar,
        ToggleTheme,
        SendMail,
        Quit,
        CloseWindow,
        ShowHelp,
        ShowAccounts
    ]
);

#[derive(Clone, Copy)]
struct Colors {
    bg: u32,
    surface: u32,
    raised: u32,
    text: u32,
    muted: u32,
    border: u32,
    accent: u32,
    soft: u32,
    danger: u32,
}
impl Colors {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: 0x1c1f26,
                surface: 0x242831,
                raised: 0x2a2f39,
                text: 0xedf0f5,
                muted: 0xa9b3c4,
                border: 0x3c4452,
                accent: 0x86cfaa,
                soft: 0x30463b,
                danger: 0xff9aa7,
            }
        } else {
            Self {
                bg: 0xeef0f3,
                surface: 0xf4f5f7,
                raised: 0xf8f9fb,
                text: 0x242b36,
                muted: 0x586474,
                border: 0xd8dde5,
                accent: 0x246846,
                soft: 0xdcebe2,
                danger: 0xa8313e,
            }
        }
    }
}
#[derive(Clone, PartialEq)]
enum Modal {
    Connect,
    Settings,
    Compose,
    Help,
}
#[derive(Clone)]
enum Action {
    Compose,
    Reply(bool),
    Forward,
    Refresh,
    Folder(Folder),
    Select(String),
    Star(String),
    Read(String),
    Move(Folder),
    Connect,
    Settings,
    Help,
    Close,
    Theme,
    Provider(Provider),
    Auth(bool),
    SubmitAccount,
    Remove(String),
    ChooseAccount(String),
    Send,
    Attach,
    RemoveAttachment(usize),
    SaveAttachment(usize),
    UnreadOnly,
    LoadMore,
    OpenDraft(String),
    DiscardDraft,
    Dismiss,
    CancelConnect,
}

pub struct MailApp {
    backend: Arc<Backend>,
    accounts: Vec<Account>,
    account: Option<String>,
    messages: Vec<Mail>,
    demo: Vec<Mail>,
    visible: Vec<usize>,
    selected: Option<String>,
    folder: Folder,
    limit: usize,
    search: Entity<InputState>,
    focus: FocusHandle,
    modal: Option<Modal>,
    dark: bool,
    unread_only: bool,
    busy: usize,
    syncing: bool,
    status: String,
    error: bool,
    form: Vec<Entity<InputState>>,
    provider: Provider,
    oauth: bool,
    connecting: bool,
    cancel: Arc<AtomicBool>,
    compose_to: Entity<InputState>,
    compose_subject: Entity<InputState>,
    compose_body: Entity<InputState>,
    draft: Draft,
    draft_revision: u64,
    drafts: Vec<Draft>,
    sending: bool,
    _subscriptions: Vec<Subscription>,
    load_generation: u64,
    read_generation: u64,
    anim: u64,
    list_scroll: UniformListScrollHandle,
    reader_scroll: ScrollHandle,
}
fn input(
    placeholder: &str,
    multi: bool,
    masked: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        let state = InputState::new(window, cx)
            .placeholder(placeholder.to_owned())
            .multi_line(multi);
        if multi { state } else { state.masked(masked) }
    })
}
impl MailApp {
    pub fn new(store: Arc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let accounts = store.accounts().unwrap_or_default();
        let account = accounts.first().map(|a| a.id.clone());
        let dark = store.preference("theme").as_deref() == Some("dark");
        let backend = Arc::new(Backend::new(store));
        let search = input("Search mail…", false, false, window, cx);
        let compose_to = input("Recipients, separated by commas", false, false, window, cx);
        let compose_subject = input("Give your message a subject", false, false, window, cx);
        let compose_body = input("Write something thoughtful…", true, false, window, cx);
        let placeholders = [
            "you@example.com",
            "Your name",
            "App password",
            "imap.example.com",
            "993",
            "smtp.example.com",
            "587",
            "OAuth client ID",
            "Client secret (Google desktop apps)",
        ];
        let form = placeholders
            .iter()
            .enumerate()
            .map(|(i, p)| input(p, false, i == 2 || i == 8, window, cx))
            .collect();
        let mut subscriptions = vec![cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter(cx);
                cx.notify();
            }
        })];
        for state in [&compose_to, &compose_subject, &compose_body] {
            subscriptions.push(cx.subscribe(state, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_draft_save(cx);
                }
            }));
        }
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.persist_draft(cx);
            std::future::ready(())
        }));
        let demo = demo_mail();
        let messages = if let Some(id) = &account {
            backend
                .store
                .messages(id, Folder::Inbox, 300)
                .unwrap_or_default()
        } else {
            demo.clone()
        };
        let selected = if account.is_none() {
            messages.first().map(|m| m.id.clone())
        } else {
            None
        };
        let mut this = Self {
            backend,
            accounts,
            account,
            messages,
            demo,
            visible: vec![],
            selected,
            folder: Folder::Inbox,
            limit: 300,
            search,
            focus: cx.focus_handle(),
            modal: None,
            dark,
            unread_only: false,
            busy: 0,
            syncing: false,
            status: String::new(),
            error: false,
            form,
            provider: Provider::Gmail,
            oauth: false,
            connecting: false,
            cancel: Arc::new(AtomicBool::new(false)),
            compose_to,
            compose_subject,
            compose_body,
            draft: Draft::default(),
            draft_revision: 0,
            drafts: vec![],
            sending: false,
            _subscriptions: subscriptions,
            load_generation: 0,
            read_generation: 0,
            anim: 0,
            list_scroll: UniformListScrollHandle::new(),
            reader_scroll: ScrollHandle::new(),
        };
        this.filter(cx);
        this.apply_theme(window, cx);
        if this.account.is_some() {
            cx.defer_in(window, |this, _, cx| this.refresh(cx));
        }
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(90)).await;
                if this
                    .update(cx, |this, cx| {
                        if this.account.is_some() && this.busy == 0 && this.folder != Folder::Drafts
                        {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn colors(&self) -> Colors {
        Colors::new(self.dark)
    }
    fn current_account(&self) -> Option<Account> {
        self.accounts
            .iter()
            .find(|a| Some(&a.id) == self.account.as_ref())
            .cloned()
    }
    fn selected_mail(&self) -> Option<Mail> {
        self.messages
            .iter()
            .find(|m| Some(&m.id) == self.selected.as_ref())
            .cloned()
    }
    fn value(state: &Entity<InputState>, cx: &App) -> String {
        state.read(cx).value().to_string()
    }
    fn set(
        state: &Entity<InputState>,
        value: impl Into<SharedString>,
        w: &mut Window,
        cx: &mut App,
    ) {
        state.update(cx, |s, cx| s.set_value(value, w, cx));
    }
    fn filter(&mut self, cx: &App) {
        let query = Self::value(&self.search, cx);
        self.visible = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| (!self.unread_only || m.unread) && m.matches(&query))
            .map(|(i, _)| i)
            .collect();
    }
    fn notice(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.error = error;
        cx.notify();
    }
    fn task<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<Backend>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<T>, &mut Context<Self>) + 'static,
    ) {
        self.busy += 1;
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job(backend) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn apply_theme(&self, w: &mut Window, cx: &mut App) {
        Theme::change(
            if self.dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            Some(w),
            cx,
        );
        let c = self.colors();
        let theme = Theme::global_mut(cx);
        theme.colors.background = rgb(c.bg).into();
        theme.colors.foreground = rgb(c.text).into();
        theme.colors.input = rgb(c.border).into();
        theme.colors.border = rgb(c.border).into();
        theme.colors.primary = rgb(c.accent).into();
        theme.colors.ring = rgb(c.accent).into();
        theme.colors.muted_foreground = rgb(c.muted).into();
        theme.colors.accent = rgb(c.soft).into();
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        let Some(a) = self.current_account() else {
            self.notice(
                "You're exploring a sample inbox. Connect an account to receive your mail.",
                false,
                cx,
            );
            return;
        };
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
            return;
        }
        let folder = self.folder;
        let limit = self.limit;
        let generation = self.load_generation;
        self.syncing = true;
        self.task(
            cx,
            move |b| b.sync(&a, folder, limit),
            move |this, result, cx| {
                if generation != this.load_generation {
                    return;
                }
                this.syncing = false;
                match result {
                    Ok(messages) => {
                        this.messages = messages;
                        this.filter(cx);
                        this.notice("Mailbox is up to date", false, cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }
    fn load_folder(&mut self, folder: Folder, w: &mut Window, cx: &mut Context<Self>) {
        self.folder = folder;
        self.limit = 300;
        self.selected = None;
        self.load_generation += 1;
        self.read_generation += 1;
        self.syncing = false;
        self.messages.clear();
        self.unread_only = false;
        Self::set(&self.search, "", w, cx);
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if folder == Folder::Drafts {
            self.load_drafts(cx);
        } else if self.account.is_none() {
            self.messages = self
                .demo
                .iter()
                .filter(|m| {
                    if folder == Folder::Starred {
                        m.starred
                    } else {
                        m.folder == folder
                    }
                })
                .cloned()
                .collect();
            self.filter(cx);
        } else {
            let id = self.account.clone().unwrap();
            let gen_ = self.load_generation;
            self.task(
                cx,
                move |b| b.store.messages(&id, folder, 300),
                move |this, result, cx| {
                    if gen_ != this.load_generation {
                        return;
                    }
                    if let Ok(ms) = result {
                        this.messages = ms;
                        this.filter(cx);
                    }
                    this.refresh(cx);
                },
            );
        }
        self.anim += 1;
        cx.notify();
    }
    fn load_drafts(&mut self, cx: &mut Context<Self>) {
        let id = self.account.clone().unwrap_or_else(|| "demo".into());
        let generation = self.load_generation;
        self.task(
            cx,
            move |b| b.store.drafts(&id),
            move |this, result, cx| {
                if generation != this.load_generation || this.folder != Folder::Drafts {
                    return;
                }
                match result {
                    Ok(ds) => {
                        this.drafts = ds;
                        this.messages = this
                            .drafts
                            .iter()
                            .map(|d| Mail {
                                id: d.id.clone(),
                                account: d.account.clone(),
                                folder: Folder::Drafts,
                                remote_folder: String::new(),
                                uid: 0,
                                from: d.to.clone(),
                                sender: if d.to.is_empty() {
                                    "New message".into()
                                } else {
                                    d.to.clone()
                                },
                                to: d.to.clone(),
                                subject: if d.subject.is_empty() {
                                    "(No subject)".into()
                                } else {
                                    d.subject.clone()
                                },
                                snippet: d.body.clone(),
                                date: chrono::Utc::now().timestamp(),
                                unread: false,
                                starred: false,
                                size: 0,
                                body: Some(d.body.clone()),
                                attachments: vec![],
                                message_id: String::new(),
                            })
                            .collect();
                        this.filter(cx);
                    }
                    Err(e) => this.notice(e.to_string(), true, cx),
                }
            },
        );
    }
    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        self.anim += 1;
        self.read_generation += 1;
        self.reader_scroll.set_offset(point(px(0.), px(0.)));
        let Some(m) = self.selected_mail() else {
            return;
        };
        if let Some(a) = self.current_account() {
            let generation = self.read_generation;
            let id2 = id.clone();
            self.task(
                cx,
                move |b| {
                    let mut m = b.read(&a, &m)?;
                    if m.unread {
                        m = b.change(&a, &m, Change::Read(true))?;
                    }
                    Ok(m)
                },
                move |this, result, cx| match result {
                    Ok(mail) => {
                        if let Some(old) = this.messages.iter_mut().find(|m| m.id == id2) {
                            *old = mail;
                        }
                        this.filter(cx);
                    }
                    Err(e) => {
                        if this.read_generation == generation {
                            this.notice(format!("{e:#}"), true, cx);
                        }
                    }
                },
            );
        } else {
            if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
                m.unread = false;
            }
            if let Some(m) = self.demo.iter_mut().find(|m| m.id == id) {
                m.unread = false;
            }
        }
        cx.notify();
    }
    fn change(&mut self, id: String, change: Change, cx: &mut Context<Self>) {
        let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        if let Some(a) = self.current_account() {
            self.task(
                cx,
                move |b| b.change(&a, &m, change),
                move |this, result, cx| match result {
                    Ok(next) => {
                        if matches!(change, Change::Move(_)) {
                            this.messages.retain(|m| m.id != id);
                            this.selected = None;
                            this.notice("Message moved", false, cx);
                        } else if let Some(m) = this.messages.iter_mut().find(|m| m.id == id) {
                            *m = next;
                        }
                        if this.folder == Folder::Starred {
                            this.messages.retain(|m| m.starred);
                        }
                        this.filter(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                },
            );
        } else {
            for m in self
                .demo
                .iter_mut()
                .chain(self.messages.iter_mut())
                .filter(|m| m.id == id)
            {
                match change {
                    Change::Read(read) => m.unread = !read,
                    Change::Star(star) => m.starred = star,
                    Change::Move(folder) => m.folder = folder,
                }
            }
            if matches!(change, Change::Move(_)) {
                self.messages.retain(|m| m.id != id);
                self.selected = None;
            }
            if self.folder == Folder::Starred {
                self.messages.retain(|m| m.starred);
            }
            self.filter(cx);
            cx.notify();
        }
    }
    fn new_compose(&mut self, mode: u8, w: &mut Window, cx: &mut Context<Self>) {
        if self.modal == Some(Modal::Compose) {
            return;
        }
        let selected = self.selected_mail();
        let own = self.current_account().map(|a| a.email).unwrap_or_default();
        let (to, subject, body, reply_id) = if let Some(m) = selected.filter(|_| mode > 0) {
            let subject = if mode == 3 {
                format!("Fwd: {}", m.subject)
            } else if m.subject.to_lowercase().starts_with("re:") {
                m.subject.clone()
            } else {
                format!("Re: {}", m.subject)
            };
            let to = if mode == 3 {
                String::new()
            } else if mode == 2 {
                let mut addresses = vec![m.from.clone()];
                if let Ok(parsed) = mailparse::addrparse(&m.to) {
                    for a in parsed.iter() {
                        if let mailparse::MailAddr::Single(a) = a
                            && !a.addr.eq_ignore_ascii_case(&own)
                            && !addresses.contains(&a.addr)
                        {
                            addresses.push(a.addr.clone());
                        }
                    }
                }
                addresses.join(", ")
            } else {
                m.from.clone()
            };
            let body = format!(
                "\n\nOn {}, {} wrote:\n{}",
                date_full(m.date),
                m.sender,
                m.body
                    .unwrap_or_default()
                    .lines()
                    .map(|l| format!("> {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            (to, subject, body, m.message_id)
        } else {
            (String::new(), String::new(), String::new(), String::new())
        };
        self.draft = Draft {
            id: uuid::Uuid::new_v4().to_string(),
            account: self.account.clone().unwrap_or_else(|| "demo".into()),
            to: to.clone(),
            subject: subject.clone(),
            body: body.clone(),
            reply_id,
            attachments: vec![],
        };
        Self::set(&self.compose_to, to, w, cx);
        Self::set(&self.compose_subject, subject, w, cx);
        Self::set(&self.compose_body, body, w, cx);
        self.modal = Some(Modal::Compose);
        self.anim += 1;
        self.compose_to.update(cx, |s, cx| s.focus(w, cx));
        cx.notify();
    }
    fn snapshot_draft(&mut self, cx: &App) {
        self.draft.to = Self::value(&self.compose_to, cx);
        self.draft.subject = Self::value(&self.compose_subject, cx);
        self.draft.body = Self::value(&self.compose_body, cx);
    }
    fn persist_draft(&mut self, cx: &App) {
        if self.modal == Some(Modal::Compose) && !self.sending {
            self.snapshot_draft(cx);
            if !self.draft.to.is_empty()
                || !self.draft.subject.is_empty()
                || !self.draft.body.is_empty()
            {
                let _ = self.backend.store.save_draft(&self.draft);
            }
        }
    }
    fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        self.draft_revision += 1;
        let rev = self.draft_revision;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(700)).await;
            let _ = this.update(cx, |this, cx| {
                if this.draft_revision == rev {
                    this.persist_draft(cx);
                }
            });
        })
        .detach();
    }
    fn close(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.persist_draft(cx);
        self.modal = None;
        self.focus.focus(w);
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
        }
        cx.notify();
    }
    fn provider(&mut self, p: Provider, w: &mut Window, cx: &mut Context<Self>) {
        self.provider = p;
        self.oauth = p == Provider::Outlook;
        let a = Account::new(p);
        Self::set(&self.form[3], a.incoming_host, w, cx);
        Self::set(&self.form[4], a.incoming_port.to_string(), w, cx);
        Self::set(&self.form[5], a.smtp_host, w, cx);
        Self::set(&self.form[6], a.smtp_port.to_string(), w, cx);
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let v: Vec<_> = self
            .form
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if i == 2 || i == 8 {
                    Self::value(s, cx)
                } else {
                    Self::value(s, cx).trim().to_owned()
                }
            })
            .collect();
        let mut a = Account::new(self.provider);
        a.email = v[0].clone();
        a.name = if v[1].is_empty() {
            v[0].clone()
        } else {
            v[1].clone()
        };
        a.incoming_host = v[3].clone();
        a.incoming_port = v[4].parse().unwrap_or(0);
        a.smtp_host = v[5].clone();
        a.smtp_port = v[6].parse().unwrap_or(0);
        a.client_id = v[7].clone();
        a.client_secret = v[8].clone();
        a.oauth = self.oauth;
        if let Err(e) = a.validate() {
            self.notice(e.to_string(), true, cx);
            return;
        }
        if self
            .accounts
            .iter()
            .any(|old| old.email.eq_ignore_ascii_case(&a.email))
        {
            self.notice(
                "This address is already connected. Remove it in Accounts before reconnecting.",
                true,
                cx,
            );
            return;
        }
        self.connecting = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let password = if self.provider == Provider::Gmail {
            v[2].replace(' ', "")
        } else {
            v[2].clone()
        };
        let saved = a.clone();
        self.notice(
            if self.oauth {
                "Complete sign-in in your browser…"
            } else {
                "Connecting securely to your mail server…"
            },
            false,
            cx,
        );
        self.task(
            cx,
            move |b| b.connect(&a, password, cancel),
            move |this, result, cx| {
                this.connecting = false;
                match result {
                    Ok(()) => {
                        this.accounts.push(saved.clone());
                        this.account = Some(saved.id);
                        this.modal = None;
                        this.folder = Folder::Inbox;
                        this.messages.clear();
                        this.selected = None;
                        this.load_generation += 1;
                        this.filter(cx);
                        this.notice("Account connected", false, cx);
                        this.refresh(cx);
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.modal != Some(Modal::Compose) {
            return;
        }
        let Some(a) = self.current_account() else {
            self.notice(
                "Connect an account before sending. Your draft is saved locally.",
                true,
                cx,
            );
            return;
        };
        self.snapshot_draft(cx);
        let d = self.draft.clone();
        if d.to.trim().is_empty() {
            self.notice("Add a recipient before sending.", true, cx);
            return;
        }
        if let Err(e) = self.backend.store.save_draft(&d) {
            self.notice(e.to_string(), true, cx);
            return;
        }
        self.sending = true;
        self.task(
            cx,
            move |b| b.send(&a, &d),
            |this, result, cx| {
                this.sending = false;
                match result {
                    Ok(()) => {
                        this.modal = None;
                        this.draft = Draft::default();
                        this.notice("Message sent", false, cx);
                        if this.folder == Folder::Drafts {
                            this.load_drafts(cx);
                        }
                    }
                    Err(e) => this.notice(format!("{e:#}"), true, cx),
                }
            },
        );
    }
    fn attach(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    if this.modal == Some(Modal::Compose) {
                        this.draft.attachments.extend(paths);
                        this.persist_draft(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    fn save_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else {
            return;
        };
        let Some(a) = m.attachments.get(index).cloned() else {
            return;
        };
        let name = std::path::Path::new(&a.name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let picker = cx.prompt_for_new_path(&self.backend.store.path, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    this.task(
                        cx,
                        move |b| {
                            let bytes = b.attachment(&m, a.part)?;
                            std::fs::write(&path, bytes)?;
                            Ok(())
                        },
                        |this, result, cx| match result {
                            Ok(()) => this.notice("Attachment saved", false, cx),
                            Err(e) => this.notice(e.to_string(), true, cx),
                        },
                    )
                });
            }
        })
        .detach();
    }
    fn next(&mut self, offset: isize, cx: &mut Context<Self>) {
        let pos = self
            .visible
            .iter()
            .position(|i| Some(&self.messages[*i].id) == self.selected.as_ref())
            .map(|i| i as isize)
            .unwrap_or(-1);
        let next = (pos + offset).max(0) as usize;
        if let Some(index) = self.visible.get(next) {
            self.list_scroll
                .scroll_to_item(next, ScrollStrategy::Center);
            self.select(self.messages[*index].id.clone(), cx);
        }
    }
    fn handle(&mut self, action: Action, w: &mut Window, cx: &mut Context<Self>) {
        if self.sending && !matches!(action, Action::Theme | Action::Dismiss) {
            return;
        }
        if self.modal == Some(Modal::Compose)
            && matches!(
                action,
                Action::Connect | Action::Settings | Action::Help | Action::ChooseAccount(_)
            )
        {
            self.persist_draft(cx);
        }
        match action {
            Action::Compose => self.new_compose(0, w, cx),
            Action::Reply(all) => self.new_compose(if all { 2 } else { 1 }, w, cx),
            Action::Forward => self.new_compose(3, w, cx),
            Action::Refresh => self.refresh(cx),
            Action::Folder(f) => self.load_folder(f, w, cx),
            Action::Select(id) => {
                if self.folder == Folder::Drafts {
                    self.handle(Action::OpenDraft(id), w, cx);
                } else {
                    self.select(id, cx);
                }
            }
            Action::Star(id) => {
                if let Some(m) = self.messages.iter().find(|m| m.id == id) {
                    self.change(id, Change::Star(!m.starred), cx);
                }
            }
            Action::Read(id) => {
                if let Some(m) = self.messages.iter().find(|m| m.id == id) {
                    self.change(id, Change::Read(m.unread), cx);
                }
            }
            Action::Move(f) => {
                if let Some(id) = self.selected.clone() {
                    self.change(id, Change::Move(f), cx);
                }
            }
            Action::Connect => {
                Self::set(&self.form[2], "", w, cx);
                Self::set(&self.form[8], "", w, cx);
                self.modal = Some(Modal::Connect);
                self.status.clear();
                self.provider(Provider::Gmail, w, cx);
                self.anim += 1;
            }
            Action::Settings => {
                self.modal = Some(Modal::Settings);
                self.anim += 1;
            }
            Action::Help => {
                self.modal = Some(Modal::Help);
                self.anim += 1;
            }
            Action::Close => self.close(w, cx),
            Action::Theme => {
                self.dark = !self.dark;
                self.apply_theme(w, cx);
                let _ = self
                    .backend
                    .store
                    .set_preference("theme", if self.dark { "dark" } else { "light" });
            }
            Action::Provider(p) => {
                if !self.connecting {
                    self.provider(p, w, cx)
                }
            }
            Action::Auth(o) => {
                if !self.connecting {
                    self.oauth = o
                }
            }
            Action::SubmitAccount => self.connect(cx),
            Action::Remove(id) => {
                if self.busy > 0 {
                    self.notice("Wait for the current mail operation to finish before removing this account.",true,cx);
                    return;
                }
                let _ = crate::store::secret_entry(&id)
                    .and_then(|e| e.delete_credential().map_err(Into::into));
                match self.backend.store.remove_account(&id) {
                    Ok(()) => {
                        self.accounts.retain(|a| a.id != id);
                        if self.account.as_ref() == Some(&id) {
                            self.account = self.accounts.first().map(|a| a.id.clone());
                            self.load_folder(Folder::Inbox, w, cx);
                        }
                    }
                    Err(e) => self.notice(e.to_string(), true, cx),
                }
            }
            Action::ChooseAccount(id) => {
                self.account = Some(id);
                self.load_folder(Folder::Inbox, w, cx);
            }
            Action::Send => self.send(cx),
            Action::Attach => self.attach(cx),
            Action::RemoveAttachment(i) => {
                if i < self.draft.attachments.len() {
                    self.draft.attachments.remove(i);
                    self.persist_draft(cx);
                }
            }
            Action::SaveAttachment(i) => self.save_attachment(i, cx),
            Action::UnreadOnly => {
                self.unread_only = !self.unread_only;
                self.filter(cx);
            }
            Action::LoadMore => {
                self.limit = (self.limit + 300).min(5000);
                self.refresh(cx);
            }
            Action::OpenDraft(id) => {
                if let Some(d) = self.drafts.iter().find(|d| d.id == id).cloned() {
                    self.draft = d.clone();
                    Self::set(&self.compose_to, d.to, w, cx);
                    Self::set(&self.compose_subject, d.subject, w, cx);
                    Self::set(&self.compose_body, d.body, w, cx);
                    self.modal = Some(Modal::Compose);
                    self.anim += 1;
                }
            }
            Action::DiscardDraft => {
                let _ = self.backend.store.delete_draft(&self.draft.id);
                self.draft = Draft::default();
                self.modal = None;
                if self.folder == Folder::Drafts {
                    self.load_drafts(cx);
                }
            }
            Action::Dismiss => self.status.clear(),
            Action::CancelConnect => {
                self.cancel.store(true, Ordering::Relaxed);
                self.close(w, cx);
            }
        }
        cx.notify();
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        label: &str,
        icon: &str,
        action: Action,
        primary: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let icon_color = if primary {
            if self.dark { 0x13291e } else { 0xffffff }
        } else if matches!(&action, Action::Star(id) if self.messages.iter().any(|m| &m.id==id && m.starred))
        {
            c.accent
        } else {
            c.muted
        };
        let tooltip = label.trim_start_matches('@').to_string();
        let label = label.to_string();
        let el = div()
            .id(id.into())
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h(px(36.))
            .px_3()
            .rounded(px(9.))
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .bg(rgb(if primary { c.accent } else { c.surface }))
            .text_color(rgb(if primary {
                if self.dark { 0x13291e } else { 0xffffff }
            } else {
                c.muted
            }))
            .hover(move |s| {
                s.bg(rgb(if primary {
                    if c.accent == 0x246846 {
                        0x2a7650
                    } else {
                        0x98ddba
                    }
                } else {
                    c.raised
                }))
            })
            .active(|s| s.opacity(0.75))
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
            .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx));
        let el = if icon.is_empty() {
            el
        } else {
            el.child(icon_el(icon, 16., icon_color))
        };
        if label.starts_with('@') {
            el.w(px(34.)).px_0()
        } else {
            el.child(label)
        }
    }
    fn topbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        div()
            .flex()
            .items_center()
            .h(px(76.))
            .flex_shrink_0()
            .pl(px(86.))
            .pr_6()
            .border_b_1()
            .border_color(rgb(c.border))
            .gap_3()
            .child(icon_el("logo", 30., c.accent))
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Email"),
            )
            .child(
                div()
                    .ml_3()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_size(px(10.))
                    .text_color(rgb(c.muted))
                    .border_1()
                    .border_color(rgb(c.border))
                    .child("NATIVE"),
            )
            .child(div().flex_1())
            .when(self.account.is_none(), |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .mr_3()
                        .child("Sample inbox"),
                )
            })
            .child(self.button(
                "theme",
                "@Switch appearance",
                if self.dark { "sun" } else { "moon" },
                Action::Theme,
                false,
                cx,
            ))
            .child(self.button(
                "help",
                "@Keyboard shortcuts",
                "info",
                Action::Help,
                false,
                cx,
            ))
            .child(self.button(
                "accounts",
                "@Manage accounts",
                "settings",
                Action::Settings,
                false,
                cx,
            ))
            .child(div().ml_2().child(self.button(
                "add-account",
                "Connect account",
                "plus",
                Action::Connect,
                true,
                cx,
            )))
    }
    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let a = self.current_account();
        let unread = self.messages.iter().filter(|m| m.unread).count();
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(216.))
            .flex_shrink_0()
            .h_full()
            .px_3()
            .pt_4()
            .pb_3()
            .gap_3()
            .child(
                self.button("compose", "Compose", "compose", Action::Compose, true, cx)
                    .h(px(44.))
                    .justify_start()
                    .px_4()
                    .rounded(px(12.)),
            );
        let name = a.as_ref().map(|a| a.name.as_str()).unwrap_or("Personal");
        sidebar = sidebar.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .pt_2()
                .pb_1()
                .child(avatar(&initials(name), 25., 0, c))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(name.to_owned()),
                )
                .child(icon_el("chevron-down", 14., c.muted)),
        );
        let mut navigation = div().flex().flex_col().gap(px(2.));
        for folder in Folder::ALL {
            let active = self.folder == folder;
            let action = Action::Folder(folder);
            navigation = navigation.child(
                div()
                    .id(SharedString::from(format!("nav-{}", folder.key())))
                    .h(px(38.))
                    .px_3()
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .bg(rgb(if active { c.soft } else { c.bg }))
                    .text_color(rgb(if active { c.text } else { c.muted }))
                    .hover(move |s| s.bg(rgb(if active { c.soft } else { c.surface })))
                    .child(icon_el(
                        folder_icon(folder),
                        18.,
                        if active { c.accent } else { c.muted },
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(if active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child(folder.label()),
                    )
                    .child(div().flex_1())
                    .when(active && unread > 0, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(c.accent))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(unread.to_string()),
                        )
                    })
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)),
                    ),
            );
        }
        sidebar = sidebar.child(navigation);
        if self.accounts.len() > 1 {
            sidebar = sidebar.child(
                div()
                    .mt_4()
                    .px_2()
                    .text_size(px(10.))
                    .text_color(rgb(c.muted))
                    .child("OTHER ACCOUNTS"),
            );
            for a in self
                .accounts
                .iter()
                .filter(|a| Some(&a.id) != self.account.as_ref())
            {
                let id = a.id.clone();
                sidebar = sidebar.child(
                    div()
                        .id(SharedString::from(format!("switch-{id}")))
                        .p_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(c.surface)))
                        .text_size(px(12.))
                        .truncate()
                        .child(a.email.clone())
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.handle(Action::ChooseAccount(id.clone()), w, cx)
                        })),
                );
            }
        }
        sidebar.child(div().flex_1()).child(
            div()
                .px_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().h(px(1.)).bg(rgb(c.border)).mb_1())
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .truncate()
                        .child(
                            a.as_ref()
                                .map(|a| a.email.clone())
                                .unwrap_or("you@example.com".into()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(10.))
                        .text_color(rgb(c.muted))
                        .child(div().size(px(5.)).rounded_full().bg(rgb(c.accent)))
                        .child(if self.account.is_some() {
                            "Encrypted connection · Local cache"
                        } else {
                            "Demo · Connect your own mailbox"
                        }),
                ),
        )
    }
    fn message_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let entity = cx.entity();
        let count = self.visible.len();
        div()
            .flex()
            .flex_col()
            .w(px(380.))
            .flex_shrink_0()
            .min_h_0()
            .h_full()
            .bg(rgb(c.surface))
            .border_l_1()
            .border_r_1()
            .border_color(rgb(c.border))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(36.))
                            .gap_2()
                            .pl_1()
                            .mb_2()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.folder.label()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .child(count.to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                self.button(
                                    "unread",
                                    "@Filter unread",
                                    "filter",
                                    Action::UnreadOnly,
                                    false,
                                    cx,
                                )
                                .when(self.unread_only, |d| d.bg(rgb(c.soft))),
                            )
                            .child(if self.syncing {
                                div()
                                    .w(px(34.))
                                    .flex()
                                    .justify_center()
                                    .child(icon_el("refresh", 15., c.accent).with_animation(
                                        "sync-pulse",
                                        Animation::new(Duration::from_millis(850)).repeat(),
                                        |d, t| {
                                            d.opacity(
                                                0.35 + 0.65 * (t * std::f32::consts::PI).sin(),
                                            )
                                        },
                                    ))
                                    .into_any_element()
                            } else {
                                self.button(
                                    "refresh",
                                    "@Refresh mailbox · ⌘R",
                                    "refresh",
                                    Action::Refresh,
                                    false,
                                    cx,
                                )
                                .into_any_element()
                            }),
                    )
                    .child(
                        Input::new(&self.search)
                            .prefix(Icon::new(IconName::Search).size(px(15.)))
                            .cleanable(true)
                            .h(px(36.))
                            .text_size(px(13.))
                            .bg(rgb(c.bg)),
                    ),
            )
            .child(if count == 0 {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .p_8()
                    .child(icon_el(
                        if self.folder == Folder::Drafts {
                            "compose"
                        } else {
                            "inbox"
                        },
                        32.,
                        c.muted,
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.syncing {
                                "Fetching your mail…"
                            } else {
                                "A little breathing room"
                            }),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(if !Self::value(&self.search, cx).is_empty() {
                                "No messages match your search."
                            } else if self.folder == Folder::Drafts {
                                "Your saved drafts will appear here."
                            } else {
                                "There are no messages in this view."
                            }),
                    )
                    .into_any_element()
            } else {
                uniform_list("message-list", count, move |range, _, cx| {
                    entity.update(cx, |this, cx| {
                        range
                            .map(|i| this.message_row(this.visible[i], cx))
                            .collect::<Vec<_>>()
                    })
                })
                .track_scroll(self.list_scroll.clone())
                .flex_1()
                .min_h_0()
                .into_any_element()
            })
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .text_size(px(10.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "{} conversation{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ))
                    .child(div().flex_1())
                    .when(
                        self.account.is_some()
                            && self.messages.len() >= self.limit
                            && self.limit < 5000,
                        |d| {
                            d.child(
                                self.button(
                                    "load-more",
                                    "Load more",
                                    "",
                                    Action::LoadMore,
                                    false,
                                    cx,
                                )
                                .h(px(26.))
                                .text_size(px(10.)),
                            )
                        },
                    ),
            )
    }
    fn message_row(&self, index: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let c = self.colors();
        let m = &self.messages[index];
        let id = m.id.clone();
        let selected = self.selected.as_ref() == Some(&m.id);
        let star = m.starred;
        div()
            .id(SharedString::from(format!("row-{id}")))
            .relative()
            .h(px(104.))
            .w_full()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .gap_3()
            .bg(rgb(if selected { c.soft } else { c.surface }))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(if selected { c.soft } else { c.bg })))
            .when(m.unread || selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .w(px(3.))
                        .rounded_r_md()
                        .bg(rgb(c.accent))
                        .opacity(if selected { 1. } else { 0.55 }),
                )
            })
            .child(avatar(&m.initials(), 34., index, c))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(13.))
                                    .font_weight(if m.unread {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .truncate()
                                    .child(m.sender.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(c.muted))
                                    .child(date_short(m.date)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(if m.unread {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .truncate()
                            .child(m.subject.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(c.muted))
                            .truncate()
                            .child(if m.snippet.is_empty() {
                                "Open to read this message…".into()
                            } else {
                                m.snippet.clone()
                            }),
                    ),
            )
            .child(
                div().absolute().bottom(px(8.)).right(px(7.)).child(
                    self.button(
                        format!("star-{}", m.id),
                        "@Star message",
                        "star",
                        Action::Star(m.id.clone()),
                        false,
                        cx,
                    )
                    .h(px(22.))
                    .w(px(22.))
                    .bg(rgb(if selected { c.soft } else { c.surface }))
                    .text_color(rgb(if star { 0xb58a38 } else { c.muted }))
                    .opacity(if star { 1. } else { 0.25 })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                ),
            )
            .on_click(
                cx.listener(move |this, _, w, cx| this.handle(Action::Select(id.clone()), w, cx)),
            )
    }
    fn reader(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let Some(m) = self.selected_mail() else {
            return div()
                .flex_1()
                .min_w_0()
                .bg(rgb(c.surface))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .size(px(68.))
                        .rounded(px(22.))
                        .bg(rgb(c.soft))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon_el("mail", 30., c.accent)),
                )
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("A quieter kind of inbox"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(c.muted))
                        .child("Choose a message and make yourself at home."),
                )
                .child(
                    div()
                        .mt_4()
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child("⌘ N  New message     ⌘ F  Search mail"),
                )
                .into_any_element();
        };
        let toolbar = div()
            .h(px(57.))
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(c.border))
            .child(self.button("reply", "Reply", "reply", Action::Reply(false), false, cx))
            .child(self.button(
                "reply-all",
                "Reply all",
                "reply-all",
                Action::Reply(true),
                false,
                cx,
            ))
            .child(self.button("forward", "Forward", "forward", Action::Forward, false, cx))
            .child(div().flex_1())
            .child(self.button(
                "archive",
                "@Archive message",
                "archive",
                Action::Move(Folder::Archive),
                false,
                cx,
            ))
            .child(self.button(
                "read",
                "@Mark read / unread",
                "mail",
                Action::Read(m.id.clone()),
                false,
                cx,
            ))
            .child(self.button(
                "trash",
                "@Move to trash",
                "trash",
                Action::Move(Folder::Trash),
                false,
                cx,
            ));
        let mut content = div()
            .id(SharedString::from(format!("reader-{}", m.id)))
            .track_scroll(&self.reader_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px(px(32.))
            .pt_6()
            .pb_10()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .mb_5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(23.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(relative(1.35))
                            .child(m.subject.clone()),
                    )
                    .child(
                        self.button(
                            "reader-star",
                            "@Star message",
                            "star",
                            Action::Star(m.id.clone()),
                            false,
                            cx,
                        )
                        .when(m.starred, |d| d.bg(rgb(c.soft)).text_color(rgb(c.accent))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .mb_6()
                    .child(avatar(&m.initials(), 40., 0, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(m.sender.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(c.muted))
                                            .truncate()
                                            .child(format!("<{}>", m.from)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(format!("to {}", m.to)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(c.muted))
                            .child(date_full(m.date)),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(c.border)).mb_6());
        let body = m.body.unwrap_or_else(|| {
            if self.busy > 0 {
                "Loading message…".into()
            } else {
                "This message has no text body.".into()
            }
        });
        // Native selectable text preserves paragraphs without running untrusted HTML.
        content = content.child(
            gpui_component::text::TextView::markdown(
                SharedString::from(format!("body-{}", m.id)),
                plain_markdown(&body),
                window,
                cx,
            )
            .selectable(true)
            .text_size(px(14.))
            .line_height(relative(1.8)),
        );
        for (i, a) in m.attachments.iter().enumerate() {
            content = content.child(
                div()
                    .mt_3()
                    .border_1()
                    .border_color(rgb(c.border))
                    .rounded_lg()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(icon_el("paperclip", 20., c.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .truncate()
                            .child(format!("{}  ·  {} KB", a.name, a.size / 1024)),
                    )
                    .child(self.button(
                        format!("attachment-{i}"),
                        "Save",
                        "download",
                        Action::SaveAttachment(i),
                        false,
                        cx,
                    )),
            );
        }
        content = content.child(
            div()
                .mt_10()
                .pt_5()
                .border_t_1()
                .border_color(rgb(c.border))
                .flex()
                .gap_2()
                .child(
                    self.button(
                        "reply-bottom",
                        "Reply",
                        "reply",
                        Action::Reply(false),
                        false,
                        cx,
                    )
                    .border_1()
                    .border_color(rgb(c.border)),
                )
                .child(
                    self.button(
                        "forward-bottom",
                        "Forward",
                        "forward",
                        Action::Forward,
                        false,
                        cx,
                    )
                    .border_1()
                    .border_color(rgb(c.border)),
                ),
        );
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(c.surface))
            .flex()
            .flex_col()
            .child(toolbar)
            .child(content.with_animation(
                ("reader-enter", self.anim),
                Animation::new(Duration::from_millis(220)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t).top(px((1. - t) * 5.)),
            ))
            .into_any_element()
    }

    fn field(&self, label: &str, index: usize) -> impl IntoElement {
        let c = self.colors();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(c.muted))
                    .child(label.to_string()),
            )
            .child(
                Input::new(&self.form[index])
                    .disabled(self.connecting)
                    .h(px(39.))
                    .text_size(px(13.)),
            )
    }
    fn connect_modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .child("Your mail, all in one calm place. Credentials stay in macOS Keychain."),
            )
            .child(
                div().flex().gap_2().children(
                    [
                        Provider::Gmail,
                        Provider::Outlook,
                        Provider::Imap,
                        Provider::Pop3,
                    ]
                    .into_iter()
                    .map(|p| {
                        self.button(
                            format!("provider-{}", p.label()),
                            p.label(),
                            if p == Provider::Gmail || p == Provider::Outlook {
                                "mail"
                            } else {
                                "globe"
                            },
                            Action::Provider(p),
                            false,
                            cx,
                        )
                        .flex_1()
                        .border_1()
                        .border_color(rgb(if p == self.provider {
                            c.accent
                        } else {
                            c.border
                        }))
                        .when(p == self.provider, |d| {
                            d.bg(rgb(c.soft)).text_color(rgb(c.accent))
                        })
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(self.field("Email address", 0))
                    .child(self.field("Display name", 1)),
            );
        if matches!(self.provider, Provider::Gmail | Provider::Outlook) {
            content = content.child(
                div()
                    .flex()
                    .gap_2()
                    .child(self.button(
                        "auth-oauth",
                        "Browser sign-in (OAuth)",
                        "shield",
                        Action::Auth(true),
                        self.oauth,
                        cx,
                    ))
                    .when(self.provider == Provider::Gmail, |d| {
                        d.child(self.button(
                            "auth-password",
                            "App password",
                            "",
                            Action::Auth(false),
                            !self.oauth,
                            cx,
                        ))
                    }),
            );
        }
        if self.oauth {
            content=content.child(self.field("Application client ID",7)).when(self.provider==Provider::Gmail,|d|d.child(self.field("Desktop application client secret",8))).child(div().p_3().rounded_lg().bg(rgb(c.soft)).text_size(px(11.)).line_height(relative(1.6)).text_color(rgb(c.muted)).child(if self.provider==Provider::Gmail{"Use a Google Cloud Desktop app client. Add your email as a test user if the consent screen is in testing. Scope: https://mail.google.com/"}else{"Register an Entra public client for personal and organizational accounts. Add a Mobile / desktop redirect: http://localhost. Grant delegated IMAP.AccessAsUser.All and SMTP.Send permissions."}));
        } else {
            content = content.child(self.field(
                if self.provider == Provider::Gmail {
                    "Google app password (requires 2-Step Verification)"
                } else {
                    "Password / app password"
                },
                2,
            ));
        }
        if matches!(self.provider, Provider::Imap | Provider::Pop3) {
            content = content.child(
                div()
                    .flex()
                    .gap_4()
                    .child(self.field(
                        if self.provider == Provider::Pop3 {
                            "POP3 server (implicit TLS)"
                        } else {
                            "IMAP server (implicit TLS)"
                        },
                        3,
                    ))
                    .child(div().w(px(90.)).child(self.field("Port", 4))),
            );
        }
        content=content.child(div().flex().gap_4().child(self.field("SMTP server",5)).child(div().w(px(90.)).child(self.field("Port",6))))
            .child(div().text_size(px(11.)).line_height(relative(1.6)).text_color(rgb(c.muted)).child(if self.provider==Provider::Pop3{"POP3 keeps downloaded messages locally. Read, star, archive, and trash changes stay on this device; originals remain on your server. SMTP uses STARTTLS (587) or TLS (465)."}else{"Incoming mail uses verified TLS. SMTP uses STARTTLS on port 587 or implicit TLS on port 465. Your provider may need IMAP and SMTP AUTH enabled."}));
        content.into_any_element()
    }
    fn settings_modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let mut d = div().flex().flex_col().gap_4().child(
            div()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .child("Connected accounts"),
        );
        if self.accounts.is_empty() {
            d = d.child(
                div()
                    .p_5()
                    .rounded_lg()
                    .bg(rgb(c.bg))
                    .text_size(px(13.))
                    .child("No accounts connected yet. You're viewing sample messages."),
            );
        }
        for a in &self.accounts {
            d = d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(c.border))
                    .child(avatar(&initials(&a.name), 36., 0, c))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(a.email.clone()),
                            )
                            .child(div().text_size(px(11.)).text_color(rgb(c.muted)).child(
                                format!(
                                    "{} · {}",
                                    a.provider.label(),
                                    if a.oauth { "OAuth 2.0" } else { "App password" }
                                ),
                            )),
                    )
                    .child(
                        self.button(
                            format!("remove-{}", a.id),
                            "Remove",
                            "trash",
                            Action::Remove(a.id.clone()),
                            false,
                            cx,
                        )
                        .text_color(rgb(c.danger)),
                    ),
            );
        }
        d.child(self.button("add-another","Connect an account","plus",Action::Connect,true,cx)).child(div().text_size(px(11.)).text_color(rgb(c.muted)).child("Removing an account clears its local mail cache, drafts, and Keychain credentials. Mail on the server is preserved."))
        .child(div().mt_3().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child("Appearance"))
        .child(self.button("settings-theme",if self.dark{"Switch to light appearance"}else{"Switch to dark appearance"},if self.dark{"sun"}else{"moon"},Action::Theme,false,cx).border_1().border_color(rgb(c.border)))
        .child(div().mt_3().text_size(px(11.)).text_color(rgb(c.muted)).child("Pinal Mail 0.1.0 · Native Rust + GPUI\nLocal SQLite cache · Virtualized message list\nBody and attachments download only when you open a message."))
        .into_any_element()
    }
    fn compose_modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let a = self.current_account();
        let mut d = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "From: {}",
                        a.map(|a| a.email)
                            .unwrap_or_else(|| "Connect an account to send".into())
                    )),
            )
            .child(
                Input::new(&self.compose_to)
                    .prefix(
                        div()
                            .w(px(44.))
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("To"),
                    )
                    .disabled(self.sending)
                    .h(px(40.)),
            )
            .child(
                Input::new(&self.compose_subject)
                    .prefix(
                        div()
                            .w(px(44.))
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("Subject"),
                    )
                    .disabled(self.sending)
                    .h(px(40.)),
            )
            .child(
                Input::new(&self.compose_body)
                    .disabled(self.sending)
                    .h(px(260.))
                    .text_size(px(14.)),
            );
        for (i, path) in self.draft.attachments.iter().enumerate() {
            d = d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("paperclip", 14., c.muted))
                    .child(
                        div().text_size(px(12.)).flex_1().truncate().child(
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        ),
                    )
                    .child(self.button(
                        format!("remove-attachment-{i}"),
                        "@Remove attachment",
                        "close",
                        Action::RemoveAttachment(i),
                        false,
                        cx,
                    )),
            );
        }
        d.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(self.button(
                    "attach",
                    "Attach files",
                    "paperclip",
                    Action::Attach,
                    false,
                    cx,
                ))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(c.muted))
                        .child("Draft saved automatically"),
                ),
        )
        .into_any_element()
    }
    fn help_modal(&self) -> AnyElement {
        let c = self.colors();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(
                [
                    ("New message", "⌘ N"),
                    ("Search mail", "⌘ F"),
                    ("Refresh mailbox", "⌘ R"),
                    ("Next / previous message", "⌥ ↓ / ⌥ ↑"),
                    ("Reply", "⌘ ⇧ R"),
                    ("Star message", "⌘ ⇧ S"),
                    ("Archive message", "⌘ ⇧ A"),
                    ("Move to trash", "⌘ ⌫"),
                    ("Send message", "⌘ Enter"),
                    ("Appearance", "⌘ ⇧ L"),
                    ("Close / save draft", "Escape"),
                ]
                .into_iter()
                .map(|(name, key)| {
                    div()
                        .flex()
                        .items_center()
                        .child(div().text_size(px(13.)).flex_1().child(name))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(c.bg))
                                .text_size(px(12.))
                                .text_color(rgb(c.muted))
                                .child(key),
                        )
                }),
            )
            .into_any_element()
    }
    fn modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let modal = self.modal.as_ref().unwrap();
        let title = match modal {
            Modal::Connect => "Connect your email",
            Modal::Settings => "Accounts & preferences",
            Modal::Compose => "New message",
            Modal::Help => "A few handy shortcuts",
        };
        let content = match modal {
            Modal::Connect => self.connect_modal(cx),
            Modal::Settings => self.settings_modal(cx),
            Modal::Compose => self.compose_modal(cx),
            Modal::Help => self.help_modal(),
        };
        let mut panel = div()
            .flex()
            .flex_col()
            .w(px(if *modal == Modal::Compose { 720. } else { 600. }))
            .max_h(relative(0.9))
            .rounded(px(18.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.surface))
            .shadow_2xl()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(self.button(
                        "modal-close",
                        "@Close dialog",
                        "close",
                        Action::Close,
                        false,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("modal-content")
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_6()
                    .child(content),
            );
        if !self.status.is_empty() {
            panel = panel.child(
                div()
                    .px_6()
                    .pb_3()
                    .text_size(px(12.))
                    .text_color(rgb(if self.error { c.danger } else { c.accent }))
                    .child(self.status.clone()),
            );
        }
        let mut footer = div()
            .border_t_1()
            .border_color(rgb(c.border))
            .px_6()
            .py_4()
            .flex()
            .items_center()
            .gap_2();
        if *modal == Modal::Connect {
            footer = footer
                .child(self.button(
                    "cancel-connect",
                    "Cancel",
                    "",
                    Action::CancelConnect,
                    false,
                    cx,
                ))
                .child(div().flex_1())
                .child(
                    self.button(
                        "connect-submit",
                        if self.connecting {
                            "Connecting…"
                        } else if self.oauth {
                            "Continue in browser"
                        } else {
                            "Connect securely"
                        },
                        "shield",
                        Action::SubmitAccount,
                        true,
                        cx,
                    )
                    .when(self.connecting, |d| d.opacity(0.5)),
                );
        } else if *modal == Modal::Compose {
            footer = footer
                .child(self.button(
                    "discard",
                    "Discard",
                    "trash",
                    Action::DiscardDraft,
                    false,
                    cx,
                ))
                .child(div().flex_1())
                .child(self.button("save-close", "Save & close", "", Action::Close, false, cx))
                .child(
                    self.button(
                        "send",
                        if self.sending {
                            "Sending…"
                        } else {
                            "Send message"
                        },
                        "send",
                        Action::Send,
                        true,
                        cx,
                    )
                    .when(self.sending, |d| d.opacity(0.5)),
                );
        } else {
            footer = footer.child(div().flex_1()).child(self.button(
                "done",
                "Done",
                "check",
                Action::Close,
                true,
                cx,
            ));
        }
        panel = panel.child(footer);
        div()
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x101c2c66))
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(panel.with_animation(
                ("modal-enter", self.anim),
                Animation::new(Duration::from_millis(180)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t).mt(px((1. - t) * 16.)),
            ))
            .into_any_element()
    }
}
impl Render for MailApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors();
        div()
            .id("mail-app")
            .key_context("Mail")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(c.bg))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .on_action(cx.listener(|this, _: &Compose, w, cx| this.new_compose(0, w, cx)))
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                if this.sending {
                    this.notice("Wait for the current message to finish sending.", true, cx);
                } else {
                    this.persist_draft(cx);
                    cx.quit();
                }
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, w, cx| {
                if this.modal.is_some() {
                    this.close(w, cx);
                } else if !this.sending {
                    this.persist_draft(cx);
                    w.remove_window();
                }
            }))
            .on_action(cx.listener(|this, _: &ShowHelp, w, cx| this.handle(Action::Help, w, cx)))
            .on_action(
                cx.listener(|this, _: &ShowAccounts, w, cx| this.handle(Action::Settings, w, cx)),
            )
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &Search, w, cx| {
                this.search.update(cx, |s, cx| s.focus(w, cx));
            }))
            .on_action(cx.listener(|this, _: &Close, w, cx| this.close(w, cx)))
            .on_action(cx.listener(|this, _: &Next, _, cx| {
                if this.modal.is_none() {
                    this.next(1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Previous, _, cx| {
                if this.modal.is_none() {
                    this.next(-1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Archive, w, cx| {
                if this.modal.is_none() {
                    this.handle(Action::Move(Folder::Archive), w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteMail, w, cx| {
                if this.modal.is_none() {
                    this.handle(Action::Move(Folder::Trash), w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Reply, w, cx| this.new_compose(1, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleStar, w, cx| {
                if let Some(id) = this.selected.clone() {
                    this.handle(Action::Star(id), w, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &ToggleTheme, w, cx| this.handle(Action::Theme, w, cx)),
            )
            .on_action(cx.listener(|this, _: &SendMail, _, cx| this.send(cx)))
            .child(self.topbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(self.message_list(cx))
                    .child(self.reader(window, cx)),
            )
            .when(self.modal.is_none() && !self.status.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_5()
                        .right_5()
                        .max_w(px(500.))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .rounded_lg()
                        .bg(rgb(c.raised))
                        .border_1()
                        .border_color(rgb(c.border))
                        .shadow_lg()
                        .text_size(px(12.))
                        .text_color(rgb(if self.error { c.danger } else { c.text }))
                        .child(self.status.clone())
                        .child(self.button(
                            "dismiss",
                            "@Dismiss notification",
                            "close",
                            Action::Dismiss,
                            false,
                            cx,
                        )),
                )
            })
            .when(self.modal.is_some(), |d| d.child(self.modal(cx)))
    }
}
fn folder_icon(f: Folder) -> &'static str {
    match f {
        Folder::Inbox => "inbox",
        Folder::Starred => "star",
        Folder::Sent => "send",
        Folder::Drafts => "compose",
        Folder::Archive => "archive",
        Folder::Junk => "spam",
        Folder::Trash => "trash",
    }
}
fn icon_el(name: &str, size: f32, color: u32) -> Div {
    let path = if [
        "logo",
        "mail",
        "compose",
        "send",
        "archive",
        "trash",
        "reply",
        "reply-all",
        "forward",
        "refresh",
        "paperclip",
        "shield",
        "spam",
        "download",
        "filter",
    ]
    .contains(&name)
    {
        format!("mail/{name}.svg")
    } else {
        format!("icons/{name}.svg")
    };
    div().size(px(size)).flex_shrink_0().child(
        svg()
            .path(SharedString::from(path))
            .size_full()
            .text_color(rgb(color)),
    )
}
fn avatar(initials: &str, size: f32, index: usize, c: Colors) -> Div {
    let light = [
        (0xdbe9e1, 0x3b6b51),
        (0xe1e5ef, 0x52658b),
        (0xeae1d9, 0x8c674e),
        (0xe7deed, 0x7f6391),
        (0xdae7ea, 0x4d7884),
    ];
    let (bg, fg) = if c.bg == 0x1c1f26 {
        (c.raised, c.accent)
    } else {
        light[index % light.len()]
    };
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .text_size(px(size * 0.31))
        .font_weight(FontWeight::SEMIBOLD)
        .child(initials.to_owned())
}
fn date_short(ts: i64) -> String {
    let Some(date) = chrono::DateTime::from_timestamp(ts, 0) else {
        return String::new();
    };
    let now = chrono::Local::now();
    let local = date.with_timezone(&chrono::Local);
    if local.date_naive() == now.date_naive() {
        local.format("%-I:%M %p").to_string()
    } else {
        local.format("%b %-d").to_string()
    }
}
fn date_full(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%b %-d, %Y · %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn run(store: Arc<Store>) {
    Application::new()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            cx.set_menus(vec![
                Menu {
                    name: "Pinal Mail".into(),
                    items: vec![
                        MenuItem::action("Accounts & Preferences…", ShowAccounts),
                        MenuItem::separator(),
                        MenuItem::action("Quit Pinal Mail", Quit),
                    ],
                },
                Menu {
                    name: "File".into(),
                    items: vec![
                        MenuItem::action("New Message", Compose),
                        MenuItem::action("Refresh Mailbox", Refresh),
                        MenuItem::separator(),
                        MenuItem::action("Close Window", CloseWindow),
                    ],
                },
                Menu {
                    name: "View".into(),
                    items: vec![
                        MenuItem::action("Search Mail", Search),
                        MenuItem::action("Toggle Appearance", ToggleTheme),
                    ],
                },
                Menu {
                    name: "Help".into(),
                    items: vec![MenuItem::action("Keyboard Shortcuts", ShowHelp)],
                },
            ]);
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, Some("Mail")),
                KeyBinding::new("cmd-w", CloseWindow, Some("Mail")),
                KeyBinding::new("cmd-,", ShowAccounts, Some("Mail")),
                KeyBinding::new("cmd-n", Compose, Some("Mail")),
                KeyBinding::new("cmd-f", Search, Some("Mail")),
                KeyBinding::new("cmd-r", Refresh, Some("Mail")),
                KeyBinding::new("escape", Close, Some("Mail")),
                KeyBinding::new("alt-down", Next, Some("Mail")),
                KeyBinding::new("alt-up", Previous, Some("Mail")),
                KeyBinding::new("cmd-shift-a", Archive, Some("Mail")),
                KeyBinding::new("cmd-backspace", DeleteMail, Some("Mail")),
                KeyBinding::new("cmd-shift-r", Reply, Some("Mail")),
                KeyBinding::new("cmd-shift-s", ToggleStar, Some("Mail")),
                KeyBinding::new("cmd-shift-l", ToggleTheme, Some("Mail")),
                KeyBinding::new("cmd-enter", SendMail, Some("Mail")),
            ]);
            let bounds = Bounds::centered(None, size(px(1440.), px(920.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1120.), px(680.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Pinal Mail".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(20.), px(28.))),
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| MailApp::new(store, window, cx));
                    view.read(cx).focus.focus(window);
                    let close_view = view.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        close_view
                            .update(cx, |this, cx| {
                                if this.sending {
                                    this.notice(
                                        "Wait for the current message to finish sending.",
                                        true,
                                        cx,
                                    );
                                    false
                                } else {
                                    this.persist_draft(cx);
                                    true
                                }
                            })
                            .unwrap_or(true)
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("Could not open the mail window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}

fn plain_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 100);
    // GPUI Component 0.5's native renderer drops markdown hard breaks and
    // paragraph margins in fit-content mode. Keep literal soft line breaks,
    // with a non-breaking space on empty lines to preserve plain email spacing.
    for ch in text
        .replace("\r\n", "\n")
        .replace("\n\n", "\n\u{00a0}\n")
        .chars()
    {
        if "\\`*_{}[]<>()#+-.!|>".contains(ch) {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}
