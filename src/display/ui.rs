#[path = "accounts.rs"]
mod accounts;
#[path = "catalog.rs"]
mod catalog;
#[cfg(all(debug_assertions, target_os = "macos"))]
#[path = "devdriver.rs"]
mod devdriver;
#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
#[path = "panes.rs"]
mod panes;
use panes::{LIST_MIN, Pane, Resize};
#[path = "reader.rs"]
mod reader;
#[cfg(test)]
#[path = "reading_tests.rs"]
mod reading_tests;
#[cfg(test)]
#[path = "responsive_tests.rs"]
mod responsive_tests;
#[path = "settings.rs"]
mod settings;
#[path = "window.rs"]
mod window;
use super::html::HtmlView;
use crate::manage::reading::{ReadingOptions, message_key};
use crate::manage::status::{AccountIssue, DownloadStatus as Download, IssueSource};
use crate::{
    manage::{
        backend::{Backend, Change},
        model::*,
        store::Store,
    },
    sync::discover,
};
use gpui::{prelude::*, *};
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputState},
    scroll::{Scrollbar, ScrollbarShow},
    tooltip::Tooltip,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
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
        ToggleSidebar,
        SelectAllMail,
        SendMail,
        Quit,
        CloseWindow,
        ShowHelp,
        ShowAccounts,
        ShowAbout,
        AddAccount,
        ReplyAll,
        ForwardMail,
        ToggleRead,
        MoveToSpam,
        MarkAllRead,
        GoInbox,
        GoStarred,
        GoSent,
        GoDrafts,
        GoArchive,
        GoSpam,
        GoTrash,
        ShowAllMail,
        ShowUnread,
        ShowStarred,
        ShowAttachments,
        Minimize,
        Zoom,
        Hide,
        HideOthers,
        ShowAll
    ]
);

const TITLEBAR_HEIGHT: f32 = 38.;
const SIDEBAR_WIDTH: f32 = 228.;
const SIDEBAR_RAIL: f32 = 60.;
const SIDEBAR_MIN: f32 = 190.;
const SIDEBAR_MAX: f32 = 420.;
/// Sidebar and reader panes animate collapsing and expanding for this long.
const PANE_ANIMATION: Duration = Duration::from_millis(240);
const STAR: u32 = 0xe0a526;
const ROW_HEIGHT: f32 = 86.;
/// The newest messages a sync keeps current; the catalog lists the rest.
const SYNC_WINDOW: usize = 300;
/// Setup form inputs.
const EMAIL: usize = 0;
const PASSWORD: usize = 1;
const IMAP_HOST: usize = 2;
const IMAP_PORT: usize = 3;
const SMTP_HOST: usize = 4;
const SMTP_PORT: usize = 5;

#[derive(Clone, Copy)]
struct Colors {
    bg: u32,
    surface: u32,
    raised: u32,
    text: u32,
    muted: u32,
    border: u32,
    accent: u32,
    accent_hover: u32,
    /// Text and icons drawn on the accent color.
    on_accent: u32,
    soft: u32,
    danger: u32,
}
impl Colors {
    fn new(dark: bool, accent: Accent) -> Self {
        let (accent, soft, accent_hover, on_accent) = accent.colors(dark);
        if dark {
            Self {
                bg: 0x1c1f26,
                surface: 0x242831,
                raised: 0x2a2f39,
                text: 0xedf0f5,
                muted: 0xa9b3c4,
                border: 0x3c4452,
                accent,
                accent_hover,
                on_accent,
                soft,
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
                accent,
                accent_hover,
                on_accent,
                soft,
                danger: 0xa8313e,
            }
        }
    }
}
/// The theme color chosen in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accent {
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
    Orange,
    Graphite,
}
impl Accent {
    const ALL: [Self; 7] = [
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Pink,
        Self::Orange,
        Self::Graphite,
    ];
    fn key(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
            Self::Orange => "orange",
            Self::Graphite => "graphite",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Green => "Green",
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Pink => "Pink",
            Self::Orange => "Orange",
            Self::Graphite => "Graphite",
        }
    }
    fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|a| a.key() == key)
            .unwrap_or(Self::Green)
    }
    /// (accent, soft, hover, on accent). Light accents keep white text above
    /// 4.5:1 contrast; dark accents are light enough for dark text.
    fn colors(self, dark: bool) -> (u32, u32, u32, u32) {
        match (self, dark) {
            (Self::Green, false) => (0x246846, 0xdcebe2, 0x2a7650, 0xffffff),
            (Self::Green, true) => (0x86cfaa, 0x30463b, 0x98ddba, 0x13291e),
            (Self::Teal, false) => (0x0b6b70, 0xd6ecec, 0x0f7a80, 0xffffff),
            (Self::Teal, true) => (0x7fd0d0, 0x2b4647, 0x93dcdc, 0x0f2a2b),
            (Self::Blue, false) => (0x2459a8, 0xdde7f6, 0x2c66bd, 0xffffff),
            (Self::Blue, true) => (0x93b8f0, 0x2e3a52, 0xa7c6f4, 0x132440),
            (Self::Purple, false) => (0x6741a8, 0xe8e0f5, 0x7550b8, 0xffffff),
            (Self::Purple, true) => (0xbea5ef, 0x3e3557, 0xcbb6f3, 0x261a40),
            (Self::Pink, false) => (0xa8336b, 0xf5dfe9, 0xb83f79, 0xffffff),
            (Self::Pink, true) => (0xf0a3c6, 0x53334a, 0xf4b5d2, 0x3d1428),
            (Self::Orange, false) => (0xa64a12, 0xf6e3d6, 0xb85716, 0xffffff),
            (Self::Orange, true) => (0xf2b07f, 0x553c2c, 0xf5bf95, 0x3d2210),
            (Self::Graphite, false) => (0x444c59, 0xe3e6eb, 0x525b69, 0xffffff),
            (Self::Graphite, true) => (0xc2c9d4, 0x3a404b, 0xd1d7e0, 0x1d2229),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Appearance {
    System,
    Light,
    Dark,
}
impl Appearance {
    fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}
fn system_dark(w: &Window) -> bool {
    matches!(
        w.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}
/// Shortcut text for this platform: macOS symbols, or Ctrl/Alt/Shift words.
fn keys(text: &str) -> String {
    if cfg!(target_os = "macos") {
        return text.into();
    }
    text.replace("⌃ ⌘", "Ctrl Alt")
        .replace("⌃⌘", "Ctrl+Alt+")
        .replace("⌘ ", "Ctrl ")
        .replace('⌘', "Ctrl+")
        .replace("⌥ ", "Alt ")
        .replace('⌥', "Alt+")
        .replace("⇧ ", "Shift ")
        .replace('⇧', "Shift+")
        .replace('⌫', "Backspace")
}
#[derive(Clone, PartialEq)]
enum Modal {
    Connect,
    Settings,
    About,
    Compose,
    Help,
    RemoveAccount(String),
    Reconnect(String),
}
#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Unread,
    Starred,
    Attachments,
}
impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All Messages",
            Self::Unread => "Unread",
            Self::Starred => "Starred",
            Self::Attachments => "Has Attachments",
        }
    }
}
#[derive(Clone)]
enum Entry {
    Item {
        label: SharedString,
        icon: &'static str,
        action: Action,
        checked: bool,
        danger: bool,
    },
    Separator,
}
fn entry(label: impl Into<SharedString>, icon: &'static str, action: Action) -> Entry {
    Entry::Item {
        label: label.into(),
        icon,
        action,
        checked: false,
        danger: false,
    }
}
fn checked(label: impl Into<SharedString>, on: bool, action: Action) -> Entry {
    Entry::Item {
        label: label.into(),
        icon: "",
        action,
        checked: on,
        danger: false,
    }
}
fn danger(label: impl Into<SharedString>, icon: &'static str, action: Action) -> Entry {
    Entry::Item {
        label: label.into(),
        icon,
        action,
        checked: false,
        danger: true,
    }
}
struct PopupMenu {
    position: Point<Pixels>,
    entries: Vec<Entry>,
    key: u64,
}
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Provider,
    Email,
    Credentials,
    Done,
}
/// A Settings section.
#[derive(Clone, Debug, PartialEq)]
enum Page {
    General,
    Reading,
    Account(String),
}
#[derive(Clone)]
enum Action {
    Compose,
    Reply(bool),
    Forward,
    Refresh,
    OpenMailbox(String, Mailbox),
    Select(String),
    Star(String),
    Read(String),
    Move(Folder),
    Apply(Vec<String>, Change),
    ApplySelection(Change),
    ReplyTo(String, u8),
    DeleteDraft(String),
    MarkAllRead(String, Mailbox),
    RefreshAccount(String),
    ToggleAccount(String),
    ToggleSubfolders(String),
    ConfirmRemove(String),
    ToggleSidebar,
    ToggleReader,
    ChooseAvatar(String),
    RemoveAvatar(String),
    StartSelecting(String),
    ToggleSelected(String),
    SelectAll,
    StopSelecting,
    OpenSearch,
    CloseSearch,
    SetFilter(Filter),
    OldestFirst(bool),
    FilterMenu,
    Download,
    Connect,
    Settings,
    AccountSettings(String),
    RetryAccount(String),
    Reconnect(String),
    SubmitReconnect(String),
    SettingsPage(Page),
    SetAppearance(Appearance),
    SetAccent(Accent),
    SetSync(String, SyncSettings),
    SetReadingDefaults(ReadingOptions),
    ToggleHtml,
    ToggleRemote,
    About,
    ChooseService(Service),
    OpenLink(&'static str),
    Help,
    Close,
    Theme,
    Protocol(Provider),
    Start,
    Discover,
    SetupBack,
    Manual,
    Finish,
    #[cfg(feature = "sample-data")]
    Sample,
    SubmitAccount,
    Remove(String),
    Send,
    Attach,
    RemoveAttachment(usize),
    SaveAttachment(usize),
    UnlockMail,
    BackToList,
    LockMail(String),
    OpenDraft(String),
    DiscardDraft,
    Dismiss,
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
    remote: String,
    folders: HashMap<String, Vec<RemoteFolder>>,
    unread: HashMap<(String, String), usize>,
    collapsed: HashSet<String>,
    sidebar_collapsed: bool,
    sidebar_anim: u64,
    /// The sidebar's width when its current animation started.
    sidebar_from: f32,
    sidebar_since: Option<std::time::Instant>,
    /// The width the sidebar was last laid out at.
    sidebar_shown: Cell<f32>,
    /// The expanded sidebar's width, which the person can drag.
    sidebar_width: f32,
    /// Whether the window is too narrow for the expanded sidebar.
    compact: Cell<bool>,
    window_width: Cell<f32>,
    /// The pane edge being dragged.
    resizing: Option<Resize>,
    /// The reader's dragged width; until then it fills the space left of the list.
    reader_width: Option<f32>,
    reader_collapsed: bool,
    reader_anim: u64,
    reader_from: f32,
    reader_since: Option<std::time::Instant>,
    reader_shown: Cell<f32>,
    /// The collapsed reader's colors, for the message they were taken from.
    palette: RefCell<(String, Vec<u32>)>,
    /// Account pictures saved on this computer.
    avatars: HashMap<String, std::path::PathBuf>,
    menu: Option<PopupMenu>,
    filter_anchor: Rc<Cell<Bounds<Pixels>>>,
    selecting: bool,
    selection: HashSet<String>,
    select_anchor: Option<String>,
    select_anim: u64,
    search_open: bool,
    search_anim: u64,
    filter_by: Filter,
    oldest_first: bool,
    download: Download,
    /// Catalog progress per (account, mailbox), and catalogs running now.
    catalogs: HashMap<(String, String), crate::manage::backend::Catalog>,
    cataloging: HashSet<(String, String)>,
    search: Entity<InputState>,
    focus: FocusHandle,
    modal: Option<Modal>,
    dark: bool,
    appearance: Appearance,
    accent: Accent,
    page: Page,
    /// Whether the window is frontmost; new mail is announced only when not.
    active: bool,
    last_check: HashMap<String, std::time::Instant>,
    account_issues: HashMap<String, HashMap<IssueSource, AccountIssue>>,
    sync_requests: HashSet<(String, String)>,
    downloading: HashSet<String>,
    retrying: HashSet<String>,
    /// Unread inbox messages already seen per account, to spot new mail.
    known: HashMap<String, HashSet<String>>,
    busy: usize,
    syncing: bool,
    status: String,
    error: bool,
    form: Vec<Entity<InputState>>,
    service: Service,
    provider: Provider,
    oauth: bool,
    connecting: bool,
    cancel: Arc<AtomicBool>,
    onboarding: bool,
    step: Step,
    discovering: bool,
    manual: bool,
    setup_generation: u64,
    notice_generation: u64,
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
    reading_defaults: ReadingOptions,
    html_view: Entity<HtmlView>,
    settings_scroll: ScrollHandle,
    sidebar_scroll: ScrollHandle,
    rail_scroll: ScrollHandle,
    onboarding_scroll: ScrollHandle,
    modal_scroll: ScrollHandle,
    /// Pinal Mail's password (or recovery key) prompt for encrypted mail.
    unlock_input: Entity<InputState>,
    /// Pinal accounts whose mail is encrypted, and those not unlocked here yet.
    encrypted: HashSet<String>,
    locked: HashSet<String>,
    unlocking: HashSet<String>,
    unlock_errors: HashMap<String, String>,
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
        let appearance = match store.preference("theme").as_deref() {
            Some("dark") => Appearance::Dark,
            Some("light") => Appearance::Light,
            _ => Appearance::System,
        };
        let dark = match appearance {
            Appearance::System => system_dark(window),
            a => a == Appearance::Dark,
        };
        let accent = Accent::from_key(&store.preference("accent").unwrap_or_default());
        let backend = Arc::new(Backend::new(store));
        let search = input("Search", false, false, window, cx);
        let compose_to = input("", false, false, window, cx);
        let compose_subject = input("", false, false, window, cx);
        let compose_body = input("", true, false, window, cx);
        let placeholders = [
            "you@example.com",
            "Password",
            "imap.example.com",
            "993",
            "smtp.example.com",
            "587",
        ];
        let form: Vec<_> = placeholders
            .iter()
            .enumerate()
            .map(|(i, p)| input(p, false, i == PASSWORD, window, cx))
            .collect();
        let mut subscriptions = vec![];
        for state in &form {
            subscriptions.push(cx.subscribe_in(state, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let action = if let Some(Modal::Reconnect(id)) = &this.modal {
                        Action::SubmitReconnect(id.clone())
                    } else if this.step == Step::Email {
                        Action::Discover
                    } else {
                        Action::SubmitAccount
                    };
                    this.handle(action, w, cx);
                }
            }));
        }
        subscriptions.extend([cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter(cx);
                cx.notify();
            }
        })]);
        let unlock_input = input("Pinal password or recovery key", false, true, window, cx);
        subscriptions.push(
            cx.subscribe_in(&unlock_input, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.handle(Action::UnlockMail, w, cx);
                }
            }),
        );
        let encrypted = HashSet::new();
        for state in [&compose_to, &compose_subject, &compose_body] {
            subscriptions.push(cx.subscribe(state, |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_draft_save(cx);
                }
            }));
        }
        subscriptions.push(cx.observe_window_appearance(window, |this, w, cx| {
            if this.appearance == Appearance::System {
                this.dark = system_dark(w);
                this.apply_theme(w, cx);
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_window_activation(window, |this, w, _| {
            this.active = w.is_window_active();
        }));
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.persist_draft(cx);
            std::future::ready(())
        }));
        #[cfg(feature = "sample-data")]
        let demo = demo_mail();
        #[cfg(not(feature = "sample-data"))]
        let demo = Vec::new();
        let messages = if let Some(id) = &account {
            backend
                .store
                .summaries(id, Folder::Inbox)
                .unwrap_or_default()
        } else {
            vec![]
        };
        let onboarding = accounts.is_empty();
        let folders = accounts
            .iter()
            .map(|a| (a.id.clone(), backend.store.folders(&a.id)))
            .collect();
        let collapsed = backend
            .store
            .preference("collapsed_accounts")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let sidebar_collapsed = backend.store.preference("sidebar").as_deref() == Some("collapsed");
        let sidebar_width = backend
            .store
            .preference("sidebar_width")
            .and_then(|w| w.parse::<f32>().ok())
            .map_or(SIDEBAR_WIDTH, |w| w.clamp(SIDEBAR_MIN, SIDEBAR_MAX));
        let reader_width = backend
            .store
            .preference("reader_width")
            .and_then(|w| w.parse::<f32>().ok())
            .map(|w| w.max(panes::READER_MIN));
        let reader_collapsed = backend.store.preference("reader").as_deref() == Some("collapsed");
        let reading_defaults = backend
            .store
            .preference("reading_defaults")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let html_view = cx.new(HtmlView::new);
        let mut this = Self {
            backend,
            accounts,
            account,
            messages,
            demo,
            visible: vec![],
            selected: None,
            folder: Folder::Inbox,
            remote: String::new(),
            folders,
            unread: HashMap::new(),
            collapsed,
            sidebar_collapsed,
            sidebar_anim: 0,
            sidebar_from: 0.,
            sidebar_since: None,
            sidebar_shown: Cell::new(0.),
            sidebar_width,
            compact: Cell::new(false),
            window_width: Cell::new(0.),
            resizing: None,
            reader_width,
            reader_collapsed,
            reader_anim: 0,
            reader_from: 0.,
            reader_since: None,
            reader_shown: Cell::new(0.),
            palette: RefCell::new((String::new(), Vec::new())),
            avatars: HashMap::new(),
            menu: None,
            filter_anchor: Rc::new(Cell::new(Bounds::default())),
            selecting: false,
            selection: HashSet::new(),
            select_anchor: None,
            select_anim: 0,
            search_open: false,
            search_anim: 0,
            filter_by: Filter::All,
            oldest_first: false,
            download: Download::default(),
            catalogs: HashMap::new(),
            cataloging: HashSet::new(),
            search,
            focus: cx.focus_handle(),
            modal: None,
            dark,
            appearance,
            accent,
            page: Page::General,
            active: true,
            last_check: HashMap::new(),
            account_issues: HashMap::new(),
            sync_requests: HashSet::new(),
            downloading: HashSet::new(),
            retrying: HashSet::new(),
            known: HashMap::new(),
            busy: 0,
            syncing: false,
            status: String::new(),
            error: false,
            form,
            service: Service::Other,
            provider: Provider::Gmail,
            oauth: false,
            connecting: false,
            cancel: Arc::new(AtomicBool::new(false)),
            onboarding,
            step: Step::Welcome,
            discovering: false,
            manual: false,
            setup_generation: 0,
            notice_generation: 0,
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
            reading_defaults,
            html_view,
            settings_scroll: ScrollHandle::new(),
            sidebar_scroll: ScrollHandle::new(),
            rail_scroll: ScrollHandle::new(),
            onboarding_scroll: ScrollHandle::new(),
            modal_scroll: ScrollHandle::new(),
            unlock_input,
            encrypted,
            locked: HashSet::new(),
            unlocking: HashSet::new(),
            unlock_errors: HashMap::new(),
        };
        this.filter(cx);
        this.apply_theme(window, cx);
        #[cfg(all(debug_assertions, target_os = "macos"))]
        devdriver::start(window, cx);
        if this.account.is_some() {
            cx.defer_in(window, |this, _, cx| {
                this.refresh_counts(cx);
                this.refresh(cx);
            });
        }
        for a in this.accounts.clone() {
            this.load_avatar(a, cx);
        }
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(20)).await;
                if this.update(cx, |this, cx| this.check_due(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn colors(&self) -> Colors {
        Colors::new(self.dark, self.accent)
    }
    fn account_of(&self, m: &Mail) -> Option<Account> {
        self.accounts.iter().find(|a| a.id == m.account).cloned()
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
    fn mailbox(&self) -> Mailbox {
        if self.folder == Folder::Other {
            Mailbox::Remote(self.remote.clone())
        } else {
            Mailbox::Standard(self.folder)
        }
    }
    fn mailbox_label(&self) -> String {
        if self.folder != Folder::Other {
            return self.folder.label().into();
        }
        self.account
            .as_ref()
            .and_then(|id| self.folders.get(id))
            .and_then(|fs| fs.iter().find(|f| f.name == self.remote))
            .map(|f| f.label.clone())
            .unwrap_or_else(|| self.remote.clone())
    }
    /// Recomputes the visible list, keeping the scroll on the same message.
    fn filter(&mut self, cx: &App) {
        let anchor = self.scroll_anchor();
        self.apply_filter(cx);
        if let Some(anchor) = anchor {
            self.restore_scroll(anchor);
        }
    }
    fn apply_filter(&mut self, cx: &App) {
        let query = Self::value(&self.search, cx).trim().to_lowercase();
        let filter = self.filter_by;
        let open = self.selected.clone();
        self.visible = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| match filter {
                Filter::All => true,
                // Opening a message reads it; it stays listed while it is open.
                Filter::Unread => m.unread || open.as_ref() == Some(&m.id),
                Filter::Starred => m.starred,
                Filter::Attachments => m.has_attachments(),
            })
            .filter(|(_, m)| m.matches(&query))
            .map(|(i, _)| i)
            .collect();
        if self.oldest_first {
            self.visible.reverse();
        }
        let present: HashSet<&String> = self.messages.iter().map(|m| &m.id).collect();
        self.selection.retain(|id| present.contains(id));
        self.sync_selecting();
    }
    /// Reloads sidebar unread counts and download progress from the cache.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        let store = self.backend.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Ok::<_, anyhow::Error>((
                        store.unread_counts()?,
                        store.download_counts(crate::sync::transport::MAX_MESSAGE)?,
                    ))
                })
                .await;
            if let Ok((unread, (total, missing))) = result {
                let _ = this.update(cx, |this, cx| {
                    this.unread = unread;
                    this.download.total = total;
                    this.download.missing = missing;
                    this.download.paused = missing > 0 && !this.download.active;
                    cx.notify();
                });
            }
        })
        .detach();
    }
    /// Each account and mailbox has its own in-flight request. UI work and
    /// another account's slow server never suppress a due inbox check.
    fn check_due(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        for a in self.accounts.clone() {
            let every = Duration::from_secs(u64::from(a.sync.refresh_minutes) * 60);
            if a.sync.refresh_minutes == 0
                || self.account_blocked(&a.id)
                || self
                    .last_check
                    .get(&a.id)
                    .is_some_and(|t| now.duration_since(*t) < every)
            {
                continue;
            }
            if self.account.as_ref() == Some(&a.id) && self.folder != Folder::Drafts {
                self.refresh(cx);
                if self.folder == Folder::Inbox {
                    continue;
                }
            }
            self.check_inbox(a, cx);
        }
    }
    fn check_inbox(&mut self, a: Account, cx: &mut Context<Self>) {
        self.sync_mailbox(a, Folder::Inbox.into(), 100, cx);
    }
    /// Remembers unread inbox mail and announces any that is new since the
    /// last check. The first check after launch only records what is there.
    fn note_inbox(&mut self, id: &str, messages: &[Mail]) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id) else {
            return;
        };
        let first = !self.known.contains_key(id);
        let known = self.known.entry(id.to_owned()).or_default();
        let recent = chrono::Utc::now().timestamp() - 2 * 86400;
        let fresh: Vec<&Mail> = messages
            .iter()
            .filter(|m| m.unread && m.date > recent && !known.contains(&m.id))
            .collect();
        let present: HashSet<&String> = messages.iter().map(|m| &m.id).collect();
        known.retain(|m| present.contains(m));
        known.extend(messages.iter().filter(|m| m.unread).map(|m| m.id.clone()));
        if first || fresh.is_empty() || !a.sync.notify || self.active {
            return;
        }
        if let Some(latest) = fresh.iter().max_by_key(|m| m.date) {
            crate::display::notify::new_mail(
                &a.email,
                (&latest.sender, &latest.subject),
                fresh.len(),
            );
        }
    }
    /// One worker per account avoids starving healthy accounts behind a slow
    /// server. Batches release the account lock between network operations.
    fn download(&mut self, cx: &mut Context<Self>) {
        for a in self.accounts.clone() {
            self.download_account(a, cx);
        }
    }
    fn download_account(&mut self, a: Account, cx: &mut Context<Self>) {
        if self.account_blocked(&a.id) || !self.downloading.insert(a.id.clone()) {
            return;
        }
        self.download.active = true;
        self.download.paused = false;
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let (b, account) = (backend.clone(), a.clone());
                let result = cx
                    .background_executor()
                    .spawn(async move { b.download_batch(&account, 25) })
                    .await;
                let more = this
                    .update(cx, |this, cx| {
                        this.account_result(&a.id, IssueSource::Download, &result);
                        let more = matches!(result, Ok(n) if n > 0)
                            && this.accounts.iter().any(|account| account.id == a.id)
                            && !this.account_blocked(&a.id);
                        if !more {
                            this.downloading.remove(&a.id);
                            this.download.active = !this.downloading.is_empty();
                        }
                        this.refresh_counts(cx);
                        cx.notify();
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
                // Yield to interactive operations between batches.
                Timer::after(Duration::from_millis(25)).await;
            }
        })
        .detach();
        cx.notify();
    }
    fn save_collapsed(&self) {
        if let Ok(json) = serde_json::to_string(&self.collapsed) {
            let _ = self
                .backend
                .store
                .set_preference("collapsed_accounts", &json);
        }
    }
    fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<Entry>, cx: &mut Context<Self>) {
        self.anim += 1;
        self.menu = Some(PopupMenu {
            position,
            entries,
            key: self.anim,
        });
        cx.notify();
    }
    fn account_entries(&self, id: &str) -> Vec<Entry> {
        let id = id.to_owned();
        vec![
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
            entry(
                "Account Settings…",
                "settings",
                Action::AccountSettings(id.clone()),
            ),
            Entry::Separator,
            danger("Remove Account…", "trash", Action::ConfirmRemove(id)),
        ]
    }
    fn mailbox_entries(&self, id: &str, mailbox: Mailbox) -> Vec<Entry> {
        let mut entries = vec![
            entry(
                "Open",
                "inbox",
                Action::OpenMailbox(id.into(), mailbox.clone()),
            ),
            entry(
                "Refresh",
                "refresh",
                Action::OpenMailbox(id.into(), mailbox.clone()),
            ),
        ];
        if !matches!(mailbox, Mailbox::Standard(Folder::Drafts | Folder::Starred)) {
            entries.push(entry(
                "Mark All as Read",
                "mail-open",
                Action::MarkAllRead(id.into(), mailbox),
            ));
        }
        entries
    }
    fn mail_entries(&self, id: &str) -> Vec<Entry> {
        if self.folder == Folder::Drafts {
            return vec![
                entry("Open Draft", "compose", Action::Select(id.into())),
                danger("Delete Draft", "trash", Action::DeleteDraft(id.into())),
            ];
        }
        if self.selection.len() > 1 && self.selection.contains(id) {
            let n = self.selection.len();
            return vec![
                entry(
                    format!("Mark {n} as Read"),
                    "mail-open",
                    Action::ApplySelection(Change::Read(true)),
                ),
                entry(
                    format!("Mark {n} as Unread"),
                    "mail",
                    Action::ApplySelection(Change::Read(false)),
                ),
                entry(
                    format!("Star {n}"),
                    "star",
                    Action::ApplySelection(Change::Star(true)),
                ),
                Entry::Separator,
                entry(
                    format!("Archive {n}"),
                    "archive",
                    Action::ApplySelection(Change::Move(Folder::Archive)),
                ),
                entry(
                    format!("Move {n} to Spam"),
                    "spam",
                    Action::ApplySelection(Change::Move(Folder::Junk)),
                ),
                danger(
                    format!("Move {n} to Trash"),
                    "trash",
                    Action::ApplySelection(Change::Move(Folder::Trash)),
                ),
            ];
        }
        let Some(m) = self.messages.iter().find(|m| m.id == id) else {
            return vec![];
        };
        let ids = vec![m.id.clone()];
        let mut entries = vec![
            entry("Reply", "reply", Action::ReplyTo(m.id.clone(), 1)),
            entry("Reply All", "reply-all", Action::ReplyTo(m.id.clone(), 2)),
            entry("Forward", "forward", Action::ReplyTo(m.id.clone(), 3)),
            Entry::Separator,
            if m.unread {
                entry(
                    "Mark as Read",
                    "mail-open",
                    Action::Apply(ids.clone(), Change::Read(true)),
                )
            } else {
                entry(
                    "Mark as Unread",
                    "mail",
                    Action::Apply(ids.clone(), Change::Read(false)),
                )
            },
            if m.starred {
                entry(
                    "Unstar",
                    "star",
                    Action::Apply(ids.clone(), Change::Star(false)),
                )
            } else {
                entry(
                    "Star",
                    "star",
                    Action::Apply(ids.clone(), Change::Star(true)),
                )
            },
            Entry::Separator,
        ];
        for (folder, label, icon) in [
            (Folder::Inbox, "Move to Inbox", "inbox"),
            (Folder::Archive, "Archive", "archive"),
            (Folder::Junk, "Move to Spam", "spam"),
        ] {
            if m.folder != folder {
                entries.push(entry(
                    label,
                    icon,
                    Action::Apply(ids.clone(), Change::Move(folder)),
                ));
            }
        }
        if m.folder != Folder::Trash {
            entries.push(danger(
                "Move to Trash",
                "trash",
                Action::Apply(ids, Change::Move(Folder::Trash)),
            ));
        }
        entries.push(Entry::Separator);
        entries.push(entry(
            "Select",
            "select",
            Action::StartSelecting(m.id.clone()),
        ));
        entries
    }
    fn filter_entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = [
            Filter::All,
            Filter::Unread,
            Filter::Starred,
            Filter::Attachments,
        ]
        .into_iter()
        .map(|f| checked(f.label(), self.filter_by == f, Action::SetFilter(f)))
        .collect();
        entries.push(Entry::Separator);
        entries.push(checked(
            "Newest First",
            !self.oldest_first,
            Action::OldestFirst(false),
        ));
        entries.push(checked(
            "Oldest First",
            self.oldest_first,
            Action::OldestFirst(true),
        ));
        entries
    }
    fn notice(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.error = error;
        self.notice_generation += 1;
        if !error {
            let generation = self.notice_generation;
            cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_secs(4)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.notice_generation == generation && !this.connecting {
                        this.status.clear();
                        cx.notify();
                    }
                });
            })
            .detach();
        }
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
        // The app root draws its own rounded window outline and background
        // (`window::window_frame`); `Root`'s square fill would show past its
        // corners. Text fields set their background through `field`.
        theme.colors.window_border = transparent_black();
        // Scrollable areas always show a slim, quiet scrollbar.
        theme.scrollbar_show = ScrollbarShow::Always;
        theme.colors.scrollbar = transparent_black();
        theme.colors.scrollbar_thumb = rgba((c.muted << 8) | 0x55).into();
        theme.colors.scrollbar_thumb_hover = rgba((c.muted << 8) | 0xa0).into();
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            theme.colors.background = transparent_black();
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.current_account() else {
            return;
        };
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
            return;
        }
        if self.account_blocked(&a.id) {
            return;
        }
        self.sync_mailbox(a, self.mailbox(), SYNC_WINDOW, cx);
    }
    fn sync_mailbox(&mut self, a: Account, mailbox: Mailbox, limit: usize, cx: &mut Context<Self>) {
        let id = a.id.clone();
        let key = mailbox.key();
        if !self.sync_requests.insert((id.clone(), key.clone())) {
            return;
        }
        let inbox = mailbox == Mailbox::Standard(Folder::Inbox);
        if inbox {
            self.last_check
                .insert(id.clone(), std::time::Instant::now());
        }
        let visible = self.account.as_ref() == Some(&id) && self.mailbox() == mailbox;
        if visible {
            self.syncing = true;
        }
        let catalog = mailbox.clone();
        self.task(
            cx,
            move |b| {
                let messages = b.sync(&a, mailbox, limit)?;
                let encrypted = a.service == Service::Pinal && b.encrypted(&a);
                Ok((messages, b.store.folders(&a.id), encrypted))
            },
            move |this, result, cx| {
                this.sync_requests.remove(&(id.clone(), key.clone()));
                this.account_result(&id, IssueSource::Sync(key.clone()), &result);
                let visible = this.account.as_ref() == Some(&id) && this.mailbox().key() == key;
                if visible {
                    this.syncing = false;
                }
                if let Ok((messages, folders, encrypted)) = result {
                    if !this.accounts.iter().any(|a| a.id == id) {
                        return;
                    }
                    this.folders.insert(id.clone(), folders);
                    if encrypted {
                        this.encrypted.insert(id.clone());
                    }
                    if inbox {
                        this.note_inbox(&id, &messages);
                    }
                    if visible {
                        this.reload_cached(cx);
                        if let Some(a) = this.accounts.iter().find(|a| a.id == id).cloned() {
                            this.catalog_mailbox(a, catalog, cx);
                        }
                    }
                    if let Some(a) = this.accounts.iter().find(|a| a.id == id).cloned() {
                        this.download_account(a, cx);
                    }
                }
                this.refresh_counts(cx);
            },
        );
    }
    fn open_mailbox(
        &mut self,
        account: Option<String>,
        mailbox: Mailbox,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.html_view.update(cx, |view, cx| view.clear(cx));
        Self::set(&self.unlock_input, "", w, cx);
        self.account = account;
        self.folder = mailbox.folder();
        self.remote = match &mailbox {
            Mailbox::Remote(name) => name.clone(),
            Mailbox::Standard(_) => String::new(),
        };
        self.selected = None;
        self.selecting = false;
        self.selection.clear();
        self.load_generation += 1;
        self.read_generation += 1;
        self.syncing = false;
        self.messages.clear();
        self.filter_by = Filter::All;
        Self::set(&self.search, "", w, cx);
        self.search_open = false;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
        } else if self.account.is_none() {
            let folder = self.folder;
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
                move |b| b.store.summaries(&id, mailbox),
                move |this, result, cx| {
                    if gen_ != this.load_generation {
                        return;
                    }
                    if let Ok(ms) = result {
                        this.set_messages(ms, cx);
                    }
                    this.refresh(cx);
                },
            );
        }
        self.anim += 1;
        cx.notify();
    }
    /// Reload confirmed cache state after a failed or partly applied operation.
    /// A generation check prevents an old request from replacing a newer view.
    fn reload_cached(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.account.clone() else {
            return;
        };
        let mailbox = self.mailbox();
        let generation = self.load_generation;
        self.task(
            cx,
            move |b| b.store.summaries(&id, mailbox),
            move |this, result, cx| {
                if generation == this.load_generation
                    && let Ok(messages) = result
                {
                    this.set_messages(messages, cx);
                }
            },
        );
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
                                html: None,
                                attachments: vec![],
                                message_id: String::new(),
                                has_attachments: !d.attachments.is_empty(),
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
        self.html_view.update(cx, |view, cx| view.clear(cx));
        self.selected = Some(id.clone());
        self.anim += 1;
        self.read_generation += 1;
        self.reader_scroll.set_offset(point(px(0.), px(0.)));
        // The list holds messages without bodies; show the cached body at
        // once (also offline) while the message is read below.
        if let Some(full) = self
            .messages
            .iter()
            .any(|m| m.id == id && m.body.is_none())
            .then(|| self.backend.store.message(&id).ok().flatten())
            .flatten()
            .filter(|m| m.body.is_some())
            && let Some(m) = self.messages.iter_mut().find(|m| m.id == id)
        {
            *m = self
                .backend
                .with_readable(vec![Mail {
                    unread: m.unread,
                    starred: m.starred,
                    folder: m.folder,
                    ..full
                }])
                .remove(0);
        }
        let Some(m) = self.selected_mail() else {
            return;
        };
        // Opening marks the message read immediately; the server catches up.
        for m in self
            .messages
            .iter_mut()
            .chain(self.demo.iter_mut())
            .filter(|m| m.id == id)
        {
            m.unread = false;
        }
        if let Some(a) = self.current_account() {
            let account_id = a.id.clone();
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
                move |this, result, cx| {
                    this.account_result(&account_id, IssueSource::Read, &result);
                    match result {
                        Ok(mail) => {
                            if let Some(old) = this.messages.iter_mut().find(|m| m.id == id2) {
                                *old = mail;
                            }
                            this.filter(cx);
                            this.refresh_counts(cx);
                        }
                        Err(e) => {
                            if e.downcast_ref::<crate::manage::crypto::Locked>().is_some() {
                                // The reader asks for the password instead.
                                if let Some(m) = this.messages.iter().find(|m| m.id == id2) {
                                    this.locked.insert(m.account.clone());
                                    this.encrypted.insert(m.account.clone());
                                }
                            } else {
                                this.reload_cached(cx);
                            }
                        }
                    }
                },
            );
        }
        cx.notify();
    }
    /// Applies a change to messages immediately and reverts it if the server
    /// rejects it.
    fn apply(&mut self, ids: Vec<String>, change: Change, cx: &mut Context<Self>) {
        let ids: HashSet<String> = ids.into_iter().collect();
        let before: Vec<Mail> = self
            .messages
            .iter()
            .filter(|m| ids.contains(&m.id))
            .cloned()
            .collect();
        if before.is_empty() {
            return;
        }
        for m in self
            .messages
            .iter_mut()
            .chain(self.demo.iter_mut())
            .filter(|m| ids.contains(&m.id))
        {
            match change {
                Change::Read(read) => m.unread = !read,
                Change::Star(star) => m.starred = star,
                Change::Move(folder) => m.folder = folder,
            }
        }
        let leaves = matches!(change, Change::Move(_))
            || (self.folder == Folder::Starred && change == Change::Star(false));
        if leaves {
            self.messages.retain(|m| !ids.contains(&m.id));
            if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
                self.selected = None;
            }
        }
        self.filter(cx);
        if self.selection.is_empty() {
            self.selecting = false;
        }
        cx.notify();
        let Some(a) = self.current_account() else {
            return;
        };
        let generation = self.load_generation;
        let count = before.len();
        let sent = before.clone();
        let account_id = a.id.clone();
        self.task(
            cx,
            move |b| b.change_many(&a, &sent, change),
            move |this, result, cx| {
                this.refresh_counts(cx);
                this.account_result(&account_id, IssueSource::Change, &result);
                match result {
                    Ok(()) => {
                        if let Change::Move(folder) = change {
                            this.notice(
                                if count == 1 {
                                    format!("Moved to {}", folder.label())
                                } else {
                                    format!("Moved {count} messages to {}", folder.label())
                                },
                                false,
                                cx,
                            );
                        }
                    }
                    Err(_) => {
                        if generation == this.load_generation {
                            for old in before {
                                match this.messages.iter_mut().find(|m| m.id == old.id) {
                                    Some(m) => *m = old,
                                    None => this.messages.push(old),
                                }
                            }
                            this.messages.sort_by_key(|m| std::cmp::Reverse(m.date));
                            this.filter(cx);
                        }
                        this.reload_cached(cx);
                    }
                }
            },
        );
    }
    fn mark_all_read(&mut self, id: String, mailbox: Mailbox, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if self.account.as_ref() == Some(&id) && self.mailbox() == mailbox {
            for m in &mut self.messages {
                m.unread = false;
            }
            self.filter(cx);
        }
        self.unread.remove(&(id.clone(), mailbox.key()));
        self.task(
            cx,
            move |b| b.mark_all_read(&a, &mailbox),
            move |this, result, cx| {
                this.refresh_counts(cx);
                this.account_result(&id, IssueSource::Change, &result);
                if result.is_err() {
                    this.reload_cached(cx);
                }
            },
        );
    }
    fn new_compose(&mut self, mode: u8, w: &mut Window, cx: &mut Context<Self>) {
        if self.modal == Some(Modal::Compose) || self.onboarding {
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
        if self.menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.modal.is_none() {
            if !self.selection.is_empty() {
                self.selection.clear();
                self.sync_selecting();
            } else if self.search_open {
                self.close_search(w, cx);
            } else if w.viewport_size().width < px(900.) {
                self.selected = None;
                self.read_generation += 1;
            }
            cx.notify();
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.connecting = false;
        self.discovering = false;
        self.setup_generation += 1;
        self.persist_draft(cx);
        self.modal = None;
        self.status.clear();
        self.error = false;
        Self::set(&self.form[PASSWORD], "", w, cx);
        self.focus.focus(w);
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
        }
        cx.notify();
    }
    fn begin_setup(&mut self, step: Step, w: &mut Window, cx: &mut Context<Self>) {
        for i in [EMAIL, PASSWORD] {
            Self::set(&self.form[i], "", w, cx);
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.setup_generation += 1;
        self.step = step;
        self.discovering = false;
        self.manual = false;
        self.connecting = false;
        self.service = Service::Other;
        self.provider = Provider::Imap;
        self.oauth = false;
        self.status.clear();
        self.anim += 1;
        if step == Step::Email {
            self.form[EMAIL].update(cx, |s, cx| s.focus(w, cx));
        }
    }
    /// Picks a service from the provider list. Browser sign-in starts at
    /// once; password services ask for the address and password.
    fn choose_service(&mut self, service: Service, w: &mut Window, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        self.status.clear();
        self.manual = false;
        self.anim += 1;
        if service == Service::Other {
            self.service = Service::Other;
            self.step = Step::Email;
            self.form[EMAIL].update(cx, |s, cx| s.focus(w, cx));
            return;
        }
        self.service = service;
        self.provider = service.provider();
        self.oauth = service.sign_in() == SignIn::Browser;
        self.step = Step::Credentials;
        if let Some(a) = service.account("") {
            self.apply_settings(
                &discover::Settings {
                    provider: a.provider,
                    incoming_host: a.incoming_host,
                    incoming_port: a.incoming_port,
                    smtp_host: a.smtp_host,
                    smtp_port: a.smtp_port,
                },
                w,
                cx,
            );
        }
        if service.sign_in() == SignIn::Password {
            let field = if Self::value(&self.form[EMAIL], cx).is_empty() {
                EMAIL
            } else {
                PASSWORD
            };
            self.form[field].update(cx, |s, cx| s.focus(w, cx));
        } else {
            self.connect(cx);
        }
    }
    fn apply_settings(&mut self, settings: &discover::Settings, w: &mut Window, cx: &mut App) {
        self.provider = settings.provider;
        Self::set(&self.form[IMAP_HOST], settings.incoming_host.clone(), w, cx);
        Self::set(
            &self.form[IMAP_PORT],
            settings.incoming_port.to_string(),
            w,
            cx,
        );
        Self::set(&self.form[SMTP_HOST], settings.smtp_host.clone(), w, cx);
        Self::set(&self.form[SMTP_PORT], settings.smtp_port.to_string(), w, cx);
    }
    fn discover(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.discovering {
            return;
        }
        let email = Self::value(&self.form[EMAIL], cx).trim().to_owned();
        if email.parse::<lettre::Address>().is_err() {
            self.notice("Enter a valid email address.", true, cx);
            return;
        }
        if self
            .accounts
            .iter()
            .any(|old| old.email.eq_ignore_ascii_case(&email))
        {
            self.notice("This account is already added.", true, cx);
            return;
        }
        self.status.clear();
        self.discovering = true;
        self.setup_generation += 1;
        let generation = self.setup_generation;
        cx.spawn_in(w, async move |this, cx| {
            let lookup = email.clone();
            let found = cx
                .background_executor()
                .spawn(async move { discover::discover(&lookup) })
                .await;
            let _ = this.update_in(cx, |this, w, cx| {
                if generation != this.setup_generation {
                    return;
                }
                this.discovering = false;
                // A known service, such as a Google Workspace domain, gets
                // that service's sign-in.
                let service = match found.as_ref().map(|f| f.provider) {
                    Some(Provider::Gmail) => Service::Gmail,
                    Some(Provider::Outlook) => Service::Outlook,
                    _ => Service::Other,
                };
                if service != Service::Other {
                    this.choose_service(service, w, cx);
                    return;
                }
                let settings = found.clone().unwrap_or_else(|| {
                    let domain = discover::domain(&email).unwrap_or_default();
                    discover::Settings {
                        provider: Provider::Imap,
                        incoming_host: format!("imap.{domain}"),
                        incoming_port: 993,
                        smtp_host: format!("smtp.{domain}"),
                        smtp_port: 587,
                    }
                });
                this.apply_settings(&settings, w, cx);
                this.manual = found.is_none();
                this.step = Step::Credentials;
                this.anim += 1;
                this.form[PASSWORD].update(cx, |s, cx| s.focus(w, cx));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn set_protocol(&mut self, p: Provider, w: &mut Window, cx: &mut Context<Self>) {
        let (old, new) = if p == Provider::Pop3 {
            ("993", "995")
        } else {
            ("995", "993")
        };
        if Self::value(&self.form[IMAP_PORT], cx).trim() == old {
            Self::set(&self.form[IMAP_PORT], new, w, cx);
        }
        self.provider = p;
        self.oauth = false;
    }
    /// The account the setup form describes. Browser and Pinal sign-in learn
    /// the address from the provider.
    fn setup_account(&self, cx: &App) -> Account {
        let value = |i: usize| Self::value(&self.form[i], cx).trim().to_owned();
        let email = value(EMAIL);
        let mut a = match self.service.sign_in() {
            SignIn::Pinal => Service::Pinal.account("").unwrap(),
            SignIn::Browser => {
                let mut a = Account::new(self.service.provider());
                let client = crate::sync::oauth::client(a.provider);
                a.oauth = true;
                a.client_id = client.id;
                a.client_secret = client.secret;
                a.email = email.clone();
                a
            }
            SignIn::Password => {
                let mut a = self
                    .service
                    .account(&email)
                    .unwrap_or_else(|| Account::new(self.provider));
                a.oauth = false;
                if self.manual || self.service == Service::Other {
                    a.provider = self.provider;
                    a.incoming_host = value(IMAP_HOST);
                    a.incoming_port = value(IMAP_PORT).parse().unwrap_or(0);
                    a.smtp_host = value(SMTP_HOST);
                    a.smtp_port = value(SMTP_PORT).parse().unwrap_or(0);
                }
                a
            }
        };
        a.service = self.service;
        a.email = if a.service == Service::Pinal {
            String::new()
        } else {
            email
        };
        a.name = a.email.clone();
        a
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        if self.connecting {
            return;
        }
        let a = self.setup_account(cx);
        let password = Self::value(&self.form[PASSWORD], cx);
        let password = if a.service == Service::Gmail {
            password.replace(' ', "")
        } else {
            password
        };
        if self.service.sign_in() == SignIn::Password {
            if let Err(e) = a.validate() {
                self.notice(e.to_string(), true, cx);
                return;
            }
            if password.is_empty() {
                self.notice(
                    format!(
                        "Enter your {}.",
                        self.service.password_label().to_lowercase()
                    ),
                    true,
                    cx,
                );
                return;
            }
        }
        let taken: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        if taken
            .iter()
            .any(|e| !a.email.is_empty() && e.eq_ignore_ascii_case(&a.email))
        {
            self.notice("This account is already added.", true, cx);
            return;
        }
        self.connecting = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let cancelled = cancel.clone();
        self.status.clear();
        self.error = false;
        self.task(
            cx,
            move |b| {
                let saved = b.connect(&a, password, &taken, cancel)?;
                let encrypted = saved.service == Service::Pinal && b.encrypted(&saved);
                Ok((saved, encrypted))
            },
            move |this, result, cx| {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                this.connecting = false;
                match result {
                    Ok((saved, encrypted)) => {
                        if encrypted {
                            this.encrypted.insert(saved.id.clone());
                        }
                        this.accounts.push(saved.clone());
                        this.load_avatar(saved.clone(), cx);
                        this.account = Some(saved.id.clone());
                        if this.onboarding {
                            this.step = Step::Done;
                            this.anim += 1;
                        } else {
                            this.notice(format!("Added {}", saved.email), false, cx);
                        }
                        this.modal = None;
                        this.folder = Folder::Inbox;
                        this.messages.clear();
                        this.selected = None;
                        this.load_generation += 1;
                        this.filter(cx);
                        if this.onboarding {
                            this.status.clear();
                        }
                        this.refresh(cx);
                    }
                    Err(e) => this.notice(e.to_string(), true, cx),
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
        let id = a.id.clone();
        self.task(
            cx,
            move |b| b.send(&a, &d),
            move |this, result, cx| {
                this.sending = false;
                this.account_result(&id, IssueSource::Send, &result);
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
        let Some(account) = self.account_of(&m) else {
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
                            let bytes = b.attachment(&account, &m, a.part)?;
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
        self.menu = None;
        if self.onboarding
            && matches!(
                action,
                Action::Compose | Action::Reply(_) | Action::Forward | Action::Settings
            )
        {
            return;
        }
        if self.modal == Some(Modal::Compose)
            && matches!(
                action,
                Action::Connect | Action::Settings | Action::Help | Action::OpenMailbox(..)
            )
        {
            self.persist_draft(cx);
        }
        match action {
            Action::Compose => self.new_compose(0, w, cx),
            Action::Reply(all) => self.new_compose(if all { 2 } else { 1 }, w, cx),
            Action::Forward => self.new_compose(3, w, cx),
            Action::Refresh => {
                if let Some(id) = self.account.clone() {
                    self.retry_account(id, cx);
                }
            }
            Action::Select(id) => {
                if self.folder == Folder::Drafts {
                    self.handle(Action::OpenDraft(id), w, cx);
                } else {
                    self.select(id, cx);
                }
            }
            Action::Star(id) => {
                if let Some(m) = self.messages.iter().find(|m| m.id == id) {
                    let star = !m.starred;
                    self.apply(vec![id], Change::Star(star), cx);
                }
            }
            Action::Read(id) => {
                if let Some(m) = self.messages.iter().find(|m| m.id == id) {
                    let read = m.unread;
                    self.apply(vec![id], Change::Read(read), cx);
                }
            }
            Action::Move(f) => {
                if self.selecting && !self.selection.is_empty() {
                    self.handle(Action::ApplySelection(Change::Move(f)), w, cx);
                } else if let Some(id) = self.selected.clone() {
                    self.apply(vec![id], Change::Move(f), cx);
                }
            }
            Action::Apply(ids, change) => self.apply(ids, change, cx),
            Action::ApplySelection(change) => {
                let ids: Vec<String> = self.selection.iter().cloned().collect();
                if matches!(change, Change::Move(_)) {
                    self.selection.clear();
                }
                self.apply(ids, change, cx);
            }
            Action::OpenMailbox(id, mailbox) => {
                let account = (!id.is_empty()).then_some(id);
                if self.account == account && self.mailbox() == mailbox {
                    self.refresh(cx);
                } else {
                    self.open_mailbox(account, mailbox, w, cx);
                }
            }
            Action::ReplyTo(id, mode) => {
                if self.selected.as_ref() != Some(&id) {
                    self.select(id, cx);
                }
                self.new_compose(mode, w, cx);
            }
            Action::DeleteDraft(id) => {
                let _ = self.backend.store.delete_draft(&id);
                if self.draft.id == id {
                    self.draft = Draft::default();
                }
                self.load_drafts(cx);
            }
            Action::MarkAllRead(id, mailbox) => self.mark_all_read(id, mailbox, cx),
            Action::RefreshAccount(id) => {
                if self.account.as_ref() == Some(&id) {
                    self.retry_account(id, cx);
                } else {
                    self.open_mailbox(Some(id.clone()), Folder::Inbox.into(), w, cx);
                    if self.account_blocked(&id) {
                        self.retry_account(id, cx);
                    }
                }
            }
            Action::ToggleAccount(id) => {
                if !self.collapsed.remove(&id) {
                    self.collapsed.insert(id);
                }
                self.save_collapsed();
            }
            Action::ToggleSubfolders(id) => {
                let key = format!("folders:{id}");
                if !self.collapsed.remove(&key) {
                    self.collapsed.insert(key);
                }
                self.save_collapsed();
            }
            Action::ConfirmRemove(id) => {
                self.modal = Some(Modal::RemoveAccount(id));
                self.anim += 1;
            }
            Action::ToggleSidebar => self.set_sidebar_collapsed(!self.sidebar_collapsed, cx),
            Action::ToggleReader => self.set_reader_collapsed(!self.reader_collapsed, cx),
            Action::ChooseAvatar(id) => self.choose_avatar(id, cx),
            Action::RemoveAvatar(id) => match self.backend.remove_avatar(&id) {
                Ok(()) => {
                    self.avatars.remove(&id);
                }
                Err(e) => self.notice(format!("{e:#}"), true, cx),
            },
            Action::StartSelecting(id) => {
                self.selection.insert(id.clone());
                self.select_anchor = Some(id);
            }
            Action::ToggleSelected(id) => {
                if !self.selection.remove(&id) {
                    self.selection.insert(id.clone());
                }
                self.select_anchor = Some(id);
            }
            Action::SelectAll => {
                if self.folder != Folder::Drafts {
                    let all: HashSet<String> = self
                        .visible
                        .iter()
                        .map(|i| self.messages[*i].id.clone())
                        .collect();
                    self.selection = if self.selection == all {
                        HashSet::new()
                    } else {
                        all
                    };
                }
            }
            Action::StopSelecting => self.selection.clear(),
            Action::OpenSearch => {
                if !self.search_open {
                    self.search_open = true;
                    self.search_anim += 1;
                }
                self.search.update(cx, |s, cx| s.focus(w, cx));
            }
            Action::CloseSearch => self.close_search(w, cx),
            Action::SetFilter(f) => {
                self.filter_by = f;
                self.filter(cx);
            }
            Action::OldestFirst(oldest) => {
                self.oldest_first = oldest;
                self.filter(cx);
                self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            Action::FilterMenu => {
                let b = self.filter_anchor.get();
                let entries = self.filter_entries();
                self.open_menu(point(b.left(), b.bottom() + px(4.)), entries, cx);
                return;
            }
            Action::Download => {
                self.download.paused = false;
                self.download(cx);
            }
            Action::Connect => {
                if self.onboarding {
                    self.begin_setup(Step::Provider, w, cx);
                } else {
                    self.modal = Some(Modal::Connect);
                    self.begin_setup(Step::Provider, w, cx);
                }
            }
            Action::Start => self.begin_setup(Step::Provider, w, cx),
            Action::ChooseService(service) => self.choose_service(service, w, cx),
            Action::OpenLink(url) => cx.open_url(url),
            Action::Discover => {
                if self.step == Step::Email
                    && (self.onboarding || self.modal == Some(Modal::Connect))
                {
                    self.discover(w, cx);
                }
            }
            Action::SetupBack => {
                if self.connecting {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.connecting = false;
                }
                self.setup_generation += 1;
                self.discovering = false;
                self.status.clear();
                self.manual = false;
                self.step = match self.step {
                    Step::Credentials if self.service == Service::Other => Step::Email,
                    Step::Credentials | Step::Email => Step::Provider,
                    _ => Step::Welcome,
                };
                self.anim += 1;
                if self.step == Step::Email {
                    self.form[EMAIL].update(cx, |s, cx| s.focus(w, cx));
                }
            }
            Action::Manual => {
                if !self.connecting {
                    self.manual = true;
                    if matches!(self.provider, Provider::Gmail | Provider::Outlook) {
                        self.provider = Provider::Imap;
                    }
                }
            }
            Action::Finish => {
                self.onboarding = false;
                self.step = Step::Welcome;
                self.focus.focus(w);
            }
            #[cfg(feature = "sample-data")]
            Action::Sample => {
                self.onboarding = false;
                self.open_mailbox(self.account.clone(), Folder::Inbox.into(), w, cx);
            }

            Action::BackToList => {
                self.html_view.update(cx, |view, cx| view.clear(cx));
                self.selected = None;
                self.read_generation += 1;
                Self::set(&self.unlock_input, "", w, cx);
                self.focus.focus(w);
            }
            Action::UnlockMail => {
                let Some(m) = self.selected_mail() else {
                    return;
                };
                let Some(a) = self.account_of(&m) else {
                    return;
                };
                if self.unlocking.contains(&a.id) {
                    return;
                }
                let secret = Self::value(&self.unlock_input, cx);
                if secret.is_empty() {
                    self.unlock_errors
                        .insert(a.id, "Enter your password or recovery key.".into());
                    self.unlock_input.update(cx, |s, cx| s.focus(w, cx));
                } else {
                    Self::set(&self.unlock_input, "", w, cx);
                    self.unlocking.insert(a.id.clone());
                    self.unlock_errors.remove(&a.id);
                    let id = m.id.clone();
                    self.task(
                        cx,
                        move |b| {
                            let secret = zeroize::Zeroizing::new(secret);
                            b.unlock(&a, &secret)
                        },
                        move |this, result, cx| {
                            this.unlocking.remove(&m.account);
                            match result {
                                Ok(()) => {
                                    this.locked.remove(&m.account);
                                    this.unlock_errors.remove(&m.account);
                                    this.notice(
                                        "Encrypted mail unlocked on this computer",
                                        false,
                                        cx,
                                    );
                                    if this.selected.as_deref() == Some(id.as_str()) {
                                        this.select(id, cx);
                                    }
                                }
                                Err(e) => {
                                    this.unlock_errors.insert(m.account, e.to_string());
                                }
                            }
                        },
                    );
                }
            }
            Action::LockMail(account) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else {
                    return;
                };
                self.html_view.update(cx, |view, cx| view.clear(cx));
                self.locked.insert(account);
                for m in self.messages.iter_mut().filter(|m| m.account == a.id) {
                    if self.encrypted.contains(&m.account) {
                        m.body = None;
                        m.html = None;
                    }
                }
                self.task(
                    cx,
                    move |b| b.lock_mail(&a),
                    |this, result, cx| match result {
                        Ok(()) => this.notice("This computer forgot your mail key", false, cx),
                        Err(e) => this.notice(e.to_string(), true, cx),
                    },
                );
            }
            Action::Settings => {
                if self.modal != Some(Modal::Settings) {
                    self.settings_scroll.set_offset(point(px(0.), px(0.)));
                    self.page = Page::General;
                    self.modal = Some(Modal::Settings);
                    self.anim += 1;
                }
            }
            Action::RetryAccount(id) => self.retry_account(id, cx),
            Action::Reconnect(id) => {
                if self.retrying.contains(&id) {
                    return;
                }
                self.cancel.store(true, Ordering::Relaxed);
                self.connecting = false;
                self.status.clear();
                Self::set(&self.form[PASSWORD], "", w, cx);
                self.modal = Some(Modal::Reconnect(id));
                self.anim += 1;
            }
            Action::SubmitReconnect(id) => self.reconnect(id, w, cx),
            Action::AccountSettings(id) => {
                self.settings_scroll.set_offset(point(px(0.), px(0.)));
                self.page = Page::Account(id);
                if self.modal != Some(Modal::Settings) {
                    self.modal = Some(Modal::Settings);
                    self.anim += 1;
                }
            }
            Action::SettingsPage(page) => {
                self.settings_scroll.set_offset(point(px(0.), px(0.)));
                self.page = page;
            }
            Action::About => {
                self.modal = Some(Modal::About);
                self.anim += 1;
            }
            Action::SetAppearance(appearance) => {
                self.appearance = appearance;
                self.dark = match appearance {
                    Appearance::System => system_dark(w),
                    a => a == Appearance::Dark,
                };
                self.apply_theme(w, cx);
                let _ = self.backend.store.set_preference("theme", appearance.key());
            }
            Action::SetAccent(accent) => {
                self.accent = accent;
                self.apply_theme(w, cx);
                let _ = self.backend.store.set_preference("accent", accent.key());
            }
            Action::SetSync(id, sync) => self.set_sync(id, sync, w, cx),
            Action::SetReadingDefaults(options) => {
                match self.backend.store.set_preference(
                    "reading_defaults",
                    &serde_json::to_string(&options).unwrap(),
                ) {
                    Ok(()) => self.reading_defaults = options,
                    Err(e) => self.notice(e.to_string(), true, cx),
                }
            }
            Action::ToggleHtml | Action::ToggleRemote => {
                if let Some(m) = self.selected_mail() {
                    let mut options = self.reading_options(&m);
                    match action {
                        Action::ToggleHtml => options.html = !options.html,
                        _ => options.block_remote = !options.block_remote,
                    }
                    if let Err(e) = self.backend.store.set_preference(
                        &message_key(&m.account, &m.id),
                        &serde_json::to_string(&options).unwrap(),
                    ) {
                        self.notice(e.to_string(), true, cx);
                    }
                    self.html_view.update(cx, |view, cx| view.clear(cx));
                }
            }

            Action::Help => {
                self.modal = Some(Modal::Help);
                self.anim += 1;
            }
            Action::Close => self.close(w, cx),
            Action::Theme => {
                let next = if self.dark {
                    Appearance::Light
                } else {
                    Appearance::Dark
                };
                self.handle(Action::SetAppearance(next), w, cx);
            }
            Action::Protocol(p) => {
                if !self.connecting {
                    self.set_protocol(p, w, cx)
                }
            }
            Action::SubmitAccount => {
                if self.step == Step::Credentials
                    && (self.onboarding || self.modal == Some(Modal::Connect))
                {
                    if self.service.sign_in() == SignIn::Password {
                        self.connect(cx)
                    } else {
                        // Try the browser again after an error or a closed tab.
                        self.cancel.store(true, Ordering::Relaxed);
                        self.connecting = false;
                        self.connect(cx)
                    }
                }
            }
            Action::Remove(id) => {
                if self.busy > 0 || self.downloading.contains(&id) {
                    self.notice("Wait for the current mail operation to finish before removing this account.",true,cx);
                    return;
                }
                self.retrying.insert(id.clone());
                let removed = id.clone();
                self.task(
                    cx,
                    move |b| b.remove_account(&removed),
                    move |this, result, cx| {
                        this.retrying.remove(&id);
                        match result {
                            Ok(()) => {
                                this.accounts.retain(|a| a.id != id);
                                this.account_issues.remove(&id);
                                this.folders.remove(&id);
                                this.known.remove(&id);
                                this.last_check.remove(&id);
                                this.modal = None;
                                if this.account.as_ref() == Some(&id) {
                                    this.account = this.accounts.first().map(|a| a.id.clone());
                                    this.messages.clear();
                                    this.selected = None;
                                    this.load_generation += 1;
                                    this.filter(cx);
                                    this.reload_cached(cx);
                                    this.refresh(cx);
                                }
                                if this.accounts.is_empty() {
                                    this.onboarding = true;
                                    this.step = Step::Welcome;
                                }
                                this.refresh_counts(cx);
                            }
                            Err(e) => this.notice(e.to_string(), true, cx),
                        }
                    },
                );
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
        }
        self.sync_selecting();
        cx.notify();
    }
    /// Selection mode is simply "something is selected"; the bulk toolbar
    /// animates in when the first message is picked.
    fn sync_selecting(&mut self) {
        let now = !self.selection.is_empty();
        if now && !self.selecting {
            self.select_anim += 1;
        }
        self.selecting = now;
    }
    /// Opens a standard mailbox of the current account from the menu bar.
    fn go(&mut self, folder: Folder, w: &mut Window, cx: &mut Context<Self>) {
        if self.onboarding || self.modal.is_some() {
            return;
        }
        let id = self.account.clone().unwrap_or_default();
        self.handle(Action::OpenMailbox(id, folder.into()), w, cx);
    }
    fn set_sync(&mut self, id: String, sync: SyncSettings, w: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter_mut().find(|a| a.id == id) else {
            return;
        };
        let window = a.sync.days != sync.days;
        a.sync = sync;
        if let Err(e) = self.backend.store.save_account(a) {
            self.notice(e.to_string(), true, cx);
            return;
        }
        // A new sync window applies at the next sync; run it now for the open account.
        if window && self.account.as_ref() == Some(&id) {
            self.open_mailbox(Some(id), self.mailbox(), w, cx);
        }
    }
    fn close_search(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        if !Self::value(&self.search, cx).is_empty() {
            Self::set(&self.search, "", w, cx);
            self.filter(cx);
        }
        self.focus.focus(w);
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
            c.on_accent
        } else if matches!(&action, Action::Star(id) if self.messages.iter().any(|m| &m.id==id && m.starred))
        {
            c.accent
        } else {
            c.muted
        };
        let tooltip = keys(label.trim_start_matches('@'));
        let label = label.to_string();
        let id = id.into();
        let selector = id.to_string();
        let el = div()
            .id(id)
            .debug_selector(move || selector)
            .flex_shrink_0()
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
            .text_color(rgb(if primary { c.on_accent } else { c.muted }))
            .hover(move |s| s.bg(rgb(if primary { c.accent_hover } else { c.raised })))
            .active(|s| s.opacity(0.75))
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)));
        let el = if icon.is_empty() {
            el
        } else {
            el.child(icon_el(icon, 16., icon_color))
        };
        if label.starts_with('@') {
            el.w(px(34.))
                .px_0()
                .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
        } else {
            el.child(label)
        }
    }
    fn topbar(&self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let small = |el: Stateful<Div>| window::no_drag(el.h(px(28.)).w(px(28.)).rounded(px(7.)));
        let bar = div()
            .id("topbar")
            .flex()
            .items_center()
            .h(px(TITLEBAR_HEIGHT))
            .flex_shrink_0()
            // Room for the macOS window buttons; on Linux the window controls
            // are appended by `client_titlebar`.
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 12. }))
            .pr_3()
            .border_b_1()
            .border_color(rgb(c.border))
            .gap_1()
            .on_click(window::on_titlebar_double_click)
            .child(img("brand/picacho.svg").w(px(20.)).h(px(17.)))
            .child(
                div()
                    .ml_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Picacho"),
            )
            .child(div().flex_1())
            .child(small(self.button(
                "theme",
                "@Switch appearance",
                if self.dark { "sun" } else { "moon" },
                Action::Theme,
                false,
                cx,
            )))
            .child(small(self.button(
                "help",
                "@Keyboard shortcuts",
                "info",
                Action::Help,
                false,
                cx,
            )))
            .child(small(self.button(
                "accounts",
                "@Settings · ⌘,",
                "settings",
                Action::Settings,
                false,
                cx,
            )))
            .when(window::is_client_decorated(w), |bar| {
                bar.child(div().w(px(1.)).h(px(18.)).mx_1().bg(rgb(c.border)))
            });
        let close = self.close_listener(cx);
        window::client_titlebar(bar, c, true, close, w, cx)
    }
    /// The close control of a client-decorated title bar: the same rules as
    /// the compositor's close request, so a message that is sending is never
    /// interrupted and the draft is kept.
    fn close_listener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(|this, _: &ClickEvent, w, cx| {
            if this.sending {
                this.notice("Wait for the current message to finish sending.", true, cx);
            } else {
                this.persist_draft(cx);
                w.remove_window();
            }
        })
    }
    fn unread_for(&self, account: &str, mailbox: &Mailbox) -> usize {
        if mailbox == &Mailbox::Standard(Folder::Starred) {
            return 0;
        }
        self.unread
            .get(&(account.to_owned(), mailbox.key()))
            .copied()
            .unwrap_or_default()
    }
    fn nav_row(
        &self,
        account: &str,
        mailbox: Mailbox,
        label: String,
        icon: &'static str,
        indent: f32,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        let active = self.account.as_deref() == Some(account) && self.mailbox() == mailbox;
        let count = self.unread_for(account, &mailbox);
        let entries = self.mailbox_entries(account, mailbox.clone());
        let action = Action::OpenMailbox(account.to_owned(), mailbox.clone());
        let selector = format!("nav-{account}-{}", mailbox.key());
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(|| selector)
            .h(px(30.))
            .pl(px(if self.sidebar_collapsed { 10. } else { 30. } + indent))
            .pr_2()
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(if active { c.soft } else { c.surface })))
            .text_color(rgb(if active { c.text } else { c.muted }))
            .child(icon_el(icon, 16., if active { c.accent } else { c.muted }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .child(label),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active { c.accent } else { c.muted }))
                        .child(fmt_count(count)),
                )
            })
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            )
    }
    fn sidebar_tree(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let mut list = div()
            .id("sidebar-accounts")
            .track_scroll(&self.sidebar_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px_2()
            .pb_2();
        let mut row_index = 0;
        let mut next_row = || {
            row_index += 1;
            row_index - 1
        };
        for (i, a) in self.accounts.iter().enumerate() {
            let id = a.id.clone();
            let open = !self.collapsed.contains(&a.id);
            let entries = self.account_entries(&a.id);
            let inbox_unread = self.unread_for(&a.id, &Folder::Inbox.into());
            let current = self.account.as_deref() == Some(a.id.as_str());
            let row = next_row();
            list = list.child(
                self.stagger(
                    div()
                        .id(SharedString::from(format!("account-{}", a.id)))
                        .debug_selector({
                            let id = a.id.clone();
                            move || format!("account-{id}")
                        })
                        .relative()
                        .mt(px(if i == 0 { 2. } else { 8. }))
                        .h(px(34.))
                        .px_2()
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        // The current account stays highlighted, open or closed.
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
                        .child(
                            div()
                                .size(px(12.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon_el(
                                    if open {
                                        "chevron-down"
                                    } else {
                                        "chevron-right"
                                    },
                                    12.,
                                    c.muted,
                                )),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(if current { c.accent } else { c.bg }))
                                .child(self.account_avatar(a, i, 20.)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if current { c.text } else { c.muted }))
                                .child(a.name.clone()),
                        )
                        .when(
                            self.account_issues
                                .get(&a.id)
                                .is_some_and(|issues| !issues.is_empty()),
                            |d| d.child(self.account_warning(&a.id, cx)),
                        )
                        .when(!open && inbox_unread > 0, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .child(fmt_count(inbox_unread)),
                            )
                        })
                        .tooltip({
                            let email = a.email.clone();
                            move |w, cx| Tooltip::new(email.clone()).build(w, cx)
                        })
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, w, cx| {
                                this.handle(Action::ToggleAccount(id.clone()), w, cx)
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.open_menu(e.position, entries.clone(), cx)
                            }),
                        ),
                    row,
                ),
            );
            if !open {
                continue;
            }
            let folders = self.folders.get(&a.id).cloned().unwrap_or_default();
            let sub_open = !self.collapsed.contains(&format!("folders:{}", a.id));
            for folder in Folder::ALL {
                let mut row = self.nav_row(
                    &a.id,
                    folder.into(),
                    folder.label().into(),
                    folder_icon(folder),
                    0.,
                    cx,
                );
                if folder == Folder::Inbox && !folders.is_empty() {
                    let id = a.id.clone();
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("subfolders-{}", a.id)))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(move |s| s.bg(rgb(c.border)))
                            .child(icon_el(
                                if sub_open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                12.,
                                c.muted,
                            ))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.handle(Action::ToggleSubfolders(id.clone()), w, cx)
                            })),
                    );
                }
                list = list.child(self.stagger(row, next_row()));
                if folder == Folder::Inbox && sub_open {
                    for f in &folders {
                        let row = self.nav_row(
                            &a.id,
                            Mailbox::Remote(f.name.clone()),
                            f.label.clone(),
                            "folder",
                            14. + f.depth as f32 * 14.,
                            cx,
                        );
                        list = list.child(self.stagger(row, next_row()));
                    }
                }
            }
        }
        #[cfg(feature = "sample-data")]
        if self.accounts.is_empty() {
            for folder in Folder::ALL {
                list = list.child(self.nav_row(
                    "",
                    folder.into(),
                    folder.label().into(),
                    folder_icon(folder),
                    0.,
                    cx,
                ));
            }
        }
        list = list.child(
            div()
                .id("sidebar-add-account")
                .mt_2()
                .h(px(30.))
                .px_2()
                .rounded(px(7.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(rgb(c.muted))
                .hover(move |s| s.bg(rgb(c.surface)).text_color(rgb(c.text)))
                .child(div().size(px(12.)))
                .child(
                    div()
                        .size(px(22.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(c.border))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon_el("plus", 12., c.muted)),
                )
                .child("Add Account")
                .on_click(cx.listener(|this, _, w, cx| this.handle(Action::Connect, w, cx))),
        );
        div()
            .flex_1()
            .min_h_0()
            .w(px(self.sidebar_width))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        self.button("compose", "Compose", "compose", Action::Compose, true, cx)
                            .flex_1()
                            .min_w_0()
                            .h(px(38.))
                            .rounded(px(10.)),
                    )
                    .child(
                        self.sidebar_toggle(false, cx)
                            .size(px(38.))
                            .rounded(px(10.)),
                    ),
            )
            .child(panes::scroll_frame(list, &self.sidebar_scroll))
    }
    fn rail_button(
        &self,
        id: String,
        tooltip: String,
        child: AnyElement,
        active: bool,
        action: Action,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id))
            .size(px(36.))
            .flex_shrink_0()
            .rounded(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(c.soft)))
            .hover(move |s| s.bg(rgb(if active { c.soft } else { c.surface })))
            .child(child)
            .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    fn sidebar_rail(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let mut rail = div()
            .id("sidebar-rail")
            .track_scroll(&self.rail_scroll)
            .flex_1()
            .min_h_0()
            .w(px(SIDEBAR_RAIL))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_3()
            .when(!self.compact.get(), |d| {
                d.child(self.sidebar_toggle(true, cx).size(px(36.)).rounded(px(10.)))
            })
            .child(
                self.button(
                    "compose",
                    "@New message · ⌘N",
                    "compose",
                    Action::Compose,
                    true,
                    cx,
                )
                .size(px(36.))
                .rounded(px(10.)),
            )
            .child(div().h(px(1.)).w(px(28.)).my_1().bg(rgb(c.border)));
        let mut row = 0;
        for (i, a) in self.accounts.iter().enumerate() {
            let current = self.account.as_ref() == Some(&a.id);
            let entries = self.account_entries(&a.id);
            row += 1;
            rail = rail.child(
                self.stagger(
                    self.rail_button(
                        format!("rail-account-{}", a.id),
                        a.email.clone(),
                        div()
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(if current { c.accent } else { c.bg }))
                            .relative()
                            .child(self.account_avatar(a, i, 24.))
                            .when(
                                self.account_issues
                                    .get(&a.id)
                                    .is_some_and(|issues| !issues.is_empty()),
                                |d| {
                                    d.child(
                                        self.account_warning(&a.id, cx)
                                            .absolute()
                                            .top(px(-7.))
                                            .right(px(-8.)),
                                    )
                                },
                            )
                            .into_any_element(),
                        current,
                        Action::OpenMailbox(a.id.clone(), Folder::Inbox.into()),
                        cx,
                    )
                    .debug_selector({
                        let id = a.id.clone();
                        move || format!("rail-account-{id}")
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            this.open_menu(e.position, entries.clone(), cx)
                        }),
                    ),
                    row,
                ),
            );
            if !current {
                continue;
            }
            for folder in Folder::ALL {
                let mailbox: Mailbox = folder.into();
                let active = self.mailbox() == mailbox;
                let count = self.unread_for(&a.id, &mailbox);
                let entries = self.mailbox_entries(&a.id, mailbox.clone());
                row += 1;
                rail = rail.child(
                    self.stagger(
                        self.rail_button(
                            format!("rail-{}-{}", a.id, folder.key()),
                            if count > 0 {
                                format!("{} · {} unread", folder.label(), fmt_count(count))
                            } else {
                                folder.label().into()
                            },
                            div()
                                .relative()
                                .child(icon_el(
                                    folder_icon(folder),
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
                            Action::OpenMailbox(a.id.clone(), mailbox),
                            cx,
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                this.open_menu(e.position, entries.clone(), cx)
                            }),
                        ),
                        row,
                    ),
                );
            }
        }
        panes::scroll_frame(
            rail.child(self.rail_button(
                "rail-add-account".into(),
                "Add Account".into(),
                icon_el("plus", 16., c.muted).into_any_element(),
                false,
                Action::Connect,
                cx,
            )),
            &self.rail_scroll,
        )
        .w(px(SIDEBAR_RAIL))
    }
    fn download_status(&self, collapsed: bool, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let d = &self.download;
        if d.total == 0 {
            return div().into_any_element();
        }
        let done = d.total - d.missing;
        let progress = done as f32 / d.total as f32;
        if collapsed {
            let tooltip = if d.missing == 0 {
                format!("All {} messages downloaded", fmt_count(d.total))
            } else {
                format!(
                    "{} of {} downloaded · {} left",
                    fmt_count(done),
                    fmt_count(d.total),
                    fmt_count(d.missing)
                )
            };
            let inner = if d.missing == 0 {
                icon_el("check", 12., c.accent).into_any_element()
            } else {
                div()
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.muted))
                    .child(format!("{}", (progress * 100.).floor() as u32))
                    .into_any_element()
            };
            return div()
                .id("download-ring")
                .debug_selector(|| "download-ring".into())
                .flex_shrink_0()
                .w(px(SIDEBAR_RAIL))
                .h(px(56.))
                .flex()
                .items_center()
                .justify_center()
                .border_t_1()
                .border_color(rgb(c.border))
                .child(
                    div()
                        .relative()
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ring(progress, c.border, c.accent).absolute().inset_0())
                        .child(inner),
                )
                .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
                .when(d.paused, |el| {
                    el.cursor_pointer().on_click(
                        cx.listener(|this, _, w, cx| this.handle(Action::Download, w, cx)),
                    )
                })
                .into_any_element();
        }
        let mut panel = div()
            .id("download-status")
            .flex_shrink_0()
            .w(px(self.sidebar_width))
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap(px(6.))
            .border_t_1()
            .border_color(rgb(c.border))
            .text_size(px(11.))
            .text_color(rgb(c.muted));
        if d.missing == 0 {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_el("check", 12., c.accent))
                    .child(format!("{} messages downloaded", fmt_count(d.total))),
            );
            return panel.into_any_element();
        }
        panel = panel
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(c.text))
                            .child(if d.active {
                                "Downloading"
                            } else if d.paused {
                                "Download paused"
                            } else {
                                "Waiting to download"
                            }),
                    )
                    .child(format!("{} / {}", fmt_count(done), fmt_count(d.total))),
            )
            .child(
                div()
                    .h(px(4.))
                    .w_full()
                    .rounded_full()
                    .bg(rgb(c.border))
                    .child(
                        div()
                            .h_full()
                            .w(relative(progress.max(0.02)))
                            .rounded_full()
                            .bg(rgb(c.accent)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{} not downloaded", fmt_count(d.missing))),
                    )
                    .when(d.paused, |row| {
                        row.child(self.link("download-resume", "Resume", Action::Download, cx))
                    }),
            );
        panel.into_any_element()
    }
    fn message_list(
        &self,
        single: bool,
        corners: Corners<Pixels>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let c = self.colors();
        let entity = cx.entity();
        let count = self.visible.len();
        div()
            .flex()
            .flex_col()
            .id("message-list")
            .debug_selector(|| "message-list".into())
            .flex_1()
            .min_w(px(if single { 0. } else { LIST_MIN }))
            .when(single, |d| d.w_full())
            .min_h_0()
            .h_full()
            .bg(rgb(c.surface))
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .border_r_1()
            .border_color(rgb(c.border))
            .child(self.list_header(count, cx))
            .child(if count == 0 {
                let searching = !Self::value(&self.search, cx).trim().is_empty();
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_8()
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .child(if self.syncing && !searching {
                        spinner(20., c.muted).into_any_element()
                    } else {
                        icon_el(
                            if self.folder == Folder::Drafts {
                                "compose"
                            } else if searching {
                                "search"
                            } else {
                                folder_icon(self.folder)
                            },
                            28.,
                            c.border,
                        )
                        .into_any_element()
                    })
                    .child(if searching {
                        "No results"
                    } else if self.syncing {
                        "Loading…"
                    } else if self.filter_by != Filter::All {
                        "No matching mail"
                    } else if self.folder == Folder::Drafts {
                        "No drafts"
                    } else {
                        "No mail yet"
                    })
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
                .map(|list| panes::scroll_frame(list, &self.list_scroll))
                .into_any_element()
            })
            .when_some(self.catalog_progress(), |d, progress| {
                d.child(
                    div()
                        .id("catalog-progress")
                        .debug_selector(|| "catalog-progress".into())
                        .h(px(34.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .border_t_1()
                        .border_color(rgb(c.border))
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .child(spinner(12., c.muted))
                        .child(if progress.total == 0 {
                            "Listing mail…".to_owned()
                        } else {
                            format!(
                                "Listing mail · {} of {}",
                                fmt_count(progress.cataloged),
                                fmt_count(progress.total)
                            )
                        }),
                )
            })
    }
    /// Unread messages in the open mailbox: exact from the loaded list when it
    /// holds the whole mailbox, otherwise from the cache's counts.
    fn mailbox_unread(&self) -> usize {
        if self.folder == Folder::Drafts {
            return 0;
        }
        let loaded = self.messages.iter().filter(|m| m.unread).count();
        match &self.account {
            Some(id) if self.folder != Folder::Starred => {
                self.unread_for(id, &self.mailbox()).max(loaded)
            }
            _ => loaded,
        }
    }
    fn list_header(&self, _count: usize, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let small = |el: Stateful<Div>| el.h(px(28.)).w(px(28.)).rounded(px(7.));
        let anchor = self.filter_anchor.clone();
        let filtered = self.filter_by != Filter::All || self.oldest_first;
        let unread = self.mailbox_unread();
        let mut header = div()
            .px_3()
            .py(px(8.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .gap_1()
                    .pl_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.mailbox_label()),
                    )
                    .when(unread > 0, |d| {
                        d.child(
                            div()
                                .id("unread-count")
                                .ml_1()
                                .px(px(7.))
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .bg(rgb(c.soft))
                                .text_color(rgb(c.accent))
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(fmt_count(unread))
                                .tooltip(move |w, cx| {
                                    Tooltip::new(format!("{} unread", fmt_count(unread)))
                                        .build(w, cx)
                                }),
                        )
                    })
                    .when(self.filter_by != Filter::All, |d| {
                        d.child(
                            div()
                                .id("filter-chip")
                                .ml_1()
                                .px_2()
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(c.border))
                                .text_color(rgb(c.muted))
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .cursor_pointer()
                                .child(self.filter_by.label())
                                .child(icon_el("close", 10., c.muted))
                                .tooltip(|w, cx| Tooltip::new("Clear filter").build(w, cx))
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.handle(Action::SetFilter(Filter::All), w, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        small(self.button(
                            "search-toggle",
                            "@Search · ⌘F",
                            "search",
                            if self.search_open {
                                Action::CloseSearch
                            } else {
                                Action::OpenSearch
                            },
                            false,
                            cx,
                        ))
                        .when(self.search_open, |d| d.bg(rgb(c.soft))),
                    )
                    .child(
                        small(self.button(
                            "filter",
                            "@Filter and sort",
                            "filter",
                            Action::FilterMenu,
                            false,
                            cx,
                        ))
                        .relative()
                        .when(filtered, |d| d.bg(rgb(c.soft)))
                        .child(
                            canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .size_full(),
                        ),
                    )
                    .child(if self.syncing {
                        div()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(spinner(15., c.accent))
                            .into_any_element()
                    } else {
                        small(self.button(
                            "refresh",
                            "@Refresh · ⌘R",
                            "refresh",
                            Action::Refresh,
                            false,
                            cx,
                        ))
                        .into_any_element()
                    }),
            );
        if self.search_open {
            header = header.child(
                div()
                    .overflow_hidden()
                    .child(
                        div().pt_2().child(
                            field(Input::new(&self.search), c, false)
                                .prefix(icon_el("search", 14., c.muted))
                                .suffix(
                                    div()
                                        .id("search-close")
                                        .size(px(20.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.))
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(rgb(c.border)))
                                        .child(icon_el("close", 12., c.muted))
                                        .tooltip(|w, cx| {
                                            Tooltip::new("Close search · Esc").build(w, cx)
                                        })
                                        .on_click(cx.listener(|this, _, w, cx| {
                                            this.handle(Action::CloseSearch, w, cx)
                                        })),
                                )
                                .h(px(32.))
                                .text_size(px(13.))
                                .bg(rgb(c.bg)),
                        ),
                    )
                    .with_animation(
                        ("search-bar", self.search_anim),
                        Animation::new(Duration::from_millis(200))
                            .with_easing(|t| 1. - (1. - t).powi(3)),
                        |d, t| d.h(px(40. * t)).opacity(t),
                    ),
            );
        }
        if self.selecting {
            header = header.child(
                div()
                    .overflow_hidden()
                    .child(div().pt_2().child(self.selection_bar(cx)))
                    .with_animation(
                        ("selection-bar", self.select_anim),
                        Animation::new(Duration::from_millis(220))
                            .with_easing(|t| 1. - (1. - t).powi(3)),
                        |d, t| d.h(px(46. * t)).opacity(t),
                    ),
            );
        }
        header
    }
    fn selection_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let n = self.selection.len();
        let all = n == self.visible.len();
        let tool = |id: &str, label: &str, icon: &str, change: Change| {
            self.button(
                id.to_owned(),
                label,
                icon,
                Action::ApplySelection(change),
                false,
                cx,
            )
            .h(px(28.))
            .w(px(28.))
            .rounded(px(7.))
            .bg(rgb(c.soft))
        };
        let any_unread = self
            .messages
            .iter()
            .any(|m| m.unread && self.selection.contains(&m.id));
        let all_starred = self
            .messages
            .iter()
            .filter(|m| self.selection.contains(&m.id))
            .all(|m| m.starred);
        div()
            .min_h(px(38.))
            .py_1()
            .rounded(px(10.))
            .bg(rgb(c.soft))
            .pl_3()
            .pr_1()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.accent))
                    .child(format!("{} selected", fmt_count(n))),
            )
            .child(
                div()
                    .id("select-all")
                    .ml_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.accent))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.7))
                    .child(if all { "Deselect All" } else { "Select All" })
                    .on_click(cx.listener(|this, _, w, cx| this.handle(Action::SelectAll, w, cx))),
            )
            .child(div().flex_1())
            .child(if any_unread {
                tool(
                    "bulk-read",
                    "@Mark as read",
                    "mail-open",
                    Change::Read(true),
                )
            } else {
                tool(
                    "bulk-unread",
                    "@Mark as unread",
                    "mail",
                    Change::Read(false),
                )
            })
            .child(tool(
                "bulk-star",
                if all_starred { "@Unstar" } else { "@Star" },
                if all_starred { "star-filled" } else { "star" },
                Change::Star(!all_starred),
            ))
            .child(
                tool(
                    "bulk-archive",
                    "@Archive",
                    "archive",
                    Change::Move(Folder::Archive),
                )
                .debug_selector(|| "bulk-archive".into()),
            )
            .child(tool(
                "bulk-spam",
                "@Move to spam",
                "spam",
                Change::Move(Folder::Junk),
            ))
            .child(tool(
                "bulk-trash",
                "@Move to trash",
                "trash",
                Change::Move(Folder::Trash),
            ))
            .child(
                self.button(
                    "select-done",
                    "@Clear selection · Esc",
                    "close",
                    Action::StopSelecting,
                    false,
                    cx,
                )
                .h(px(28.))
                .w(px(28.))
                .rounded(px(7.))
                .bg(rgb(c.soft)),
            )
    }
    fn message_row(&self, index: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let c = self.colors();
        let m = &self.messages[index];
        let id = m.id.clone();
        let selected = self.selected.as_ref() == Some(&m.id);
        let checked = self.selection.contains(&m.id);
        let highlighted = selected || checked;
        let star = m.starred;
        let drafts = self.folder == Folder::Drafts;
        let row_group = SharedString::from(format!("row-group-{id}"));
        let lead_group = SharedString::from(format!("lead-group-{id}"));
        let entries = self.mail_entries(&m.id);
        let check_fg = c.on_accent;
        let checkbox = div()
            .size(px(20.))
            .rounded_full()
            .border_2()
            .border_color(rgb(if checked { c.accent } else { c.muted }))
            .bg(rgb(if checked { c.accent } else { c.surface }))
            .flex()
            .items_center()
            .justify_center()
            .when(checked, |d| d.child(icon_el("check", 12., check_fg)));
        let check_id = m.id.clone();
        let lead = div()
            .id(SharedString::from(format!("lead-{id}")))
            .debug_selector(|| format!("lead-{id}"))
            .group(lead_group.clone())
            .relative()
            .size(px(36.))
            .flex_shrink_0()
            .child(avatar(&m.initials(), 36., index, c).when(self.selecting, |d| d.opacity(0.)))
            .when(!drafts, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgb(if highlighted { c.soft } else { c.surface }))
                        .when(!self.selecting, |d| {
                            d.opacity(0.)
                                .group_hover(lead_group.clone(), |s| s.opacity(1.))
                        })
                        .child(checkbox),
                )
                .cursor_pointer()
                .tooltip(move |w, cx| {
                    Tooltip::new(if checked { "Deselect" } else { "Select" }).build(w, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                    cx.stop_propagation();
                    if e.modifiers().shift
                        && let Some(anchor) = this.select_anchor.clone()
                    {
                        this.select_range(&anchor, &check_id, cx);
                    } else {
                        this.handle(Action::ToggleSelected(check_id.clone()), w, cx)
                    }
                }))
            });
        let star_id = m.id.clone();
        let address =
            (!m.from.is_empty() && !m.from.eq_ignore_ascii_case(&m.sender)).then(|| m.from.clone());
        div()
            .id(SharedString::from(format!("row-{id}")))
            .debug_selector(|| format!("row-{id}"))
            .group(row_group.clone())
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .pl(px(14.))
            // Room for the list's scrollbar.
            .pr(px(18.))
            .py(px(11.))
            .border_b_1()
            .border_color(rgb(c.border))
            .flex()
            .items_start()
            .gap(px(12.))
            .bg(rgb(if highlighted { c.soft } else { c.surface }))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(if highlighted { c.soft } else { c.bg })))
            .when(m.unread, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .top(px(26.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(rgb(c.accent)),
                )
            })
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(relative(0.62))
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(if m.unread {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .child(m.sender.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .children(address),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(if m.unread { c.accent } else { c.muted }))
                                    .font_weight(if m.unread {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(date_short(m.date)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(18.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(if m.unread {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(m.subject.clone()),
                            )
                            .when(m.has_attachments(), |d| {
                                d.child(icon_el("paperclip", 13., c.muted))
                            })
                            .when(!drafts, |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("star-{}", m.id)))
                                        .size(px(20.))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.))
                                        .when(!star, |d| {
                                            d.opacity(0.)
                                                .group_hover(row_group.clone(), |s| s.opacity(1.))
                                        })
                                        .hover(move |s| s.bg(rgb(c.border)))
                                        .child(if star {
                                            icon_el("star-filled", 14., STAR)
                                        } else {
                                            icon_el("star", 14., c.muted)
                                        })
                                        .tooltip(move |w, cx| {
                                            Tooltip::new(if star { "Unstar" } else { "Star" })
                                                .build(w, cx)
                                        })
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, w, cx| {
                                            cx.stop_propagation();
                                            this.handle(Action::Star(star_id.clone()), w, cx)
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(16.))
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(m.snippet.clone()),
                    ),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                let modifiers = e.modifiers();
                if this.folder == Folder::Drafts {
                    this.handle(Action::Select(id.clone()), w, cx)
                } else if modifiers.shift
                    && let Some(anchor) = this.select_anchor.clone().or(this.selected.clone())
                {
                    this.select_range(&anchor, &id, cx);
                } else if modifiers.platform || this.selecting {
                    if !this.selecting
                        && let Some(open) = this.selected.clone()
                    {
                        this.selection.insert(open);
                    }
                    this.handle(Action::ToggleSelected(id.clone()), w, cx)
                } else {
                    this.handle(Action::Select(id.clone()), w, cx)
                }
            }))
    }
    fn select_range(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let position = |id: &str| self.visible.iter().position(|i| self.messages[*i].id == id);
        let (Some(a), Some(b)) = (position(from), position(to)) else {
            return;
        };
        let (a, b) = (a.min(b), a.max(b));
        let ids: Vec<String> = self.visible[a..=b]
            .iter()
            .map(|i| self.messages[*i].id.clone())
            .collect();
        self.selection.extend(ids);
        self.sync_selecting();
        cx.notify();
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
                field(Input::new(&self.form[index]), c, self.connecting)
                    .disabled(self.connecting)
                    .h(px(39.))
                    .text_size(px(13.)),
            )
    }
    fn link(&self, id: &str, label: &str, action: Action, cx: &Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        div()
            .id(SharedString::from(id.to_owned()))
            .text_size(px(12.))
            .text_color(rgb(c.accent))
            .cursor_pointer()
            .hover(|s| s.opacity(0.75))
            .child(label.to_owned())
            .on_click(cx.listener(move |this, _, w, cx| this.handle(action.clone(), w, cx)))
    }
    /// A service's mark: the Pinal logo for Pinal, otherwise a color tile.
    fn service_tile(&self, service: Service, size: f32) -> Div {
        let c = self.colors();
        if service == Service::Pinal {
            return div()
                .size(px(size))
                .flex_shrink_0()
                .rounded(px(size * 0.25))
                .bg(rgb(if self.dark { 0xf4f5f7 } else { 0xffffff }))
                .border_1()
                .border_color(rgb(c.border))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img("brand/pinal.svg")
                        .w(px(size * 0.7))
                        .h(px(size * 0.7 * 468. / 568.)),
                );
        }
        let key = match service {
            Service::Gmail => "gmail",
            Service::Outlook => "outlook",
            Service::ICloud => "icloud",
            Service::Fastmail => "fastmail",
            Service::Zoho => "zoho",
            Service::ProtonBridge => "proton",
            _ => "other",
        };
        div()
            .size(px(size))
            .flex_shrink_0()
            .child(img(SharedString::from(format!("brand/{key}.svg"))).size_full())
    }
    /// The first setup step: Pinal Mail, then the other supported services.
    fn provider_picker(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let pinal = div()
            .id("service-Pinal")
            .debug_selector(|| "service-Pinal".into())
            .w_full()
            .p_4()
            .flex()
            .items_center()
            .gap_4()
            .rounded(px(14.))
            .border_1()
            .border_color(rgb(c.accent))
            .bg(rgb(c.soft))
            .cursor_pointer()
            .hover(|s| s.opacity(0.88))
            .active(|s| s.opacity(0.75))
            .child(self.service_tile(Service::Pinal, 60.))
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
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Pinal Mail"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .rounded_full()
                                    .bg(rgb(c.accent))
                                    .text_color(rgb(c.on_accent))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Recommended"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("Private email designed for Picacho. Sign in once, with no passwords to copy."),
                    ),
            )
            .child(icon_el("chevron-right", 16., c.accent))
            .on_click(cx.listener(|this, _, w, cx| {
                this.handle(Action::ChooseService(Service::Pinal), w, cx)
            }));
        let card = |service: Service| {
            let id = format!("service-{service:?}");
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id)
                .flex_1()
                .h(px(96.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .cursor_pointer()
                .hover(move |s| s.border_color(rgb(c.accent)))
                .active(|s| s.opacity(0.75))
                .child(self.service_tile(service, 36.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(service.label()),
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.handle(Action::ChooseService(service), w, cx)
                }))
        };
        let others: Vec<Service> = Service::PICKER[1..].to_vec();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(pinal)
            .children(
                others
                    .chunks(3)
                    .map(|row| div().flex().gap_3().children(row.iter().map(|s| card(*s)))),
            )
            .child(div().pt_1().flex().justify_center().child(self.link(
                "service-Other",
                "Use another provider",
                Action::ChooseService(Service::Other),
                cx,
            )))
            .into_any_element()
    }
    /// The address and password form for services without browser sign-in.
    fn setup_form(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        if self.step == Step::Email {
            return div()
                .flex()
                .flex_col()
                .gap_4()
                .child(self.field("Email address", EMAIL))
                .into_any_element();
        }
        let mut content = div().flex().flex_col().gap_4();
        if self.service == Service::Other {
            let email = Self::value(&self.form[EMAIL], cx);
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .child(avatar(&initials(&email), 24., 0, c))
                    .child(div().flex_1().min_w_0().truncate().child(email))
                    .child(self.link("setup-change", "Change", Action::SetupBack, cx)),
            );
        } else {
            content = content.child(self.field("Email address", EMAIL));
        }
        if self.manual {
            content = content
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .children([Provider::Imap, Provider::Pop3].map(|p| {
                            self.button(
                                format!("protocol-{}", p.label()),
                                p.label(),
                                "",
                                Action::Protocol(p),
                                false,
                                cx,
                            )
                            .h(px(30.))
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
                        })),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(self.field("Incoming server", IMAP_HOST))
                        .child(div().w(px(80.)).child(self.field("Port", IMAP_PORT))),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(self.field("SMTP server", SMTP_HOST))
                        .child(div().w(px(80.)).child(self.field("Port", SMTP_PORT))),
                );
        }
        content = content.child(self.field(self.service.password_label(), PASSWORD));
        if let Some((help, url)) = self.service.password_help() {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(help)
                    .child(
                        div().flex().child(
                            self.link("password-help", "Make one now", Action::OpenLink(url), cx)
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(icon_el("external-link", 12., c.accent)),
                        ),
                    ),
            );
        }
        if !self.manual {
            content = content.child(div().flex().justify_end().child(self.link(
                "setup-manual",
                "Server settings",
                Action::Manual,
                cx,
            )));
        }
        content.into_any_element()
    }
    /// A row of mutually exclusive choices.
    fn about_modal(&self) -> AnyElement {
        let c = self.colors();
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_4()
            .child(img("brand/picacho.svg").w(px(128.)).h(px(108.)))
            .child(
                div()
                    .mt_3()
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Picacho"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(13.))
                    .text_color(rgb(c.muted))
                    .text_center()
                    .child("Fast, native email for every inbox you have. Made by Pinal."),
            )
            .into_any_element()
    }
    fn compose_modal(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let a = self.current_account();
        let mut d = div()
            .flex()
            .flex_col()
            .gap_3()
            .when_some(a, |d, a| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child(format!("From: {}", a.email)),
                )
            })
            .child(
                field(Input::new(&self.compose_to), c, false)
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
                field(Input::new(&self.compose_subject), c, false)
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
                field(Input::new(&self.compose_body), c, self.sending)
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
                .child(div().flex_1()),
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
                    ("Reply all", "⌘ ⌥ R"),
                    ("Forward", "⌘ ⇧ F"),
                    ("Mark read / unread", "⌘ ⇧ U"),
                    ("Go to Inbox … Trash", "⌘ 1 … ⌘ 7"),
                    ("Settings", "⌘ ,"),
                    ("Star message", "⌘ ⇧ S"),
                    ("Archive message", "⌘ ⇧ A"),
                    ("Select all messages", "⌘ A"),
                    ("Select range / toggle", "⇧ Click / ⌘ Click"),
                    ("Toggle sidebar", "⌃ ⌘ S"),
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
                                .child(keys(key)),
                        )
                }),
            )
            .into_any_element()
    }
    /// The steps for adding an account, shared by onboarding and the Add
    /// Account dialog.
    fn setup_view(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let in_modal = self.modal == Some(Modal::Connect);
        let browser = self.service.sign_in() != SignIn::Password;
        let label = self.service.label();
        let (title, subtitle): (String, String) = match self.step {
            Step::Welcome => (
                "Welcome to Picacho".into(),
                "Fast, native email for every inbox you have.".into(),
            ),
            Step::Provider => (
                "Add your email".into(),
                "Choose where your mail lives.".into(),
            ),
            Step::Email => (
                "Another provider".into(),
                "Enter your address and we'll find its settings.".into(),
            ),
            Step::Credentials if browser => (
                format!("Sign in to {label}"),
                if self.connecting {
                    "Continue in the browser window that just opened.".into()
                } else {
                    "Sign in with your browser. Picacho never sees your password.".into()
                },
            ),
            Step::Credentials => (
                if self.service == Service::Other {
                    "Sign in".into()
                } else {
                    format!("Sign in to {label}")
                },
                if self.manual {
                    "We couldn't find your settings. Enter them below.".into()
                } else {
                    String::new()
                },
            ),
            Step::Done => ("You're all set".into(), "Your mail is syncing.".into()),
        };
        let hero = match self.step {
            Step::Welcome => img("brand/picacho.svg")
                .w(px(112.))
                .h(px(94.))
                .with_animation(
                    "welcome-logo",
                    Animation::new(Duration::from_millis(700)),
                    |d, t| d.opacity(t).mt(px((1. - back_out(t)) * 24.)),
                )
                .into_any_element(),
            Step::Done => div()
                .size(px(72.))
                .rounded_full()
                .bg(rgb(c.accent))
                .flex()
                .items_center()
                .justify_center()
                .child(icon_el("check", 34., c.on_accent))
                .with_animation(
                    ("done-check", self.anim),
                    Animation::new(Duration::from_millis(520)),
                    |d, t| d.size(px(72. * back_out(t).max(0.01))).opacity(t),
                )
                .into_any_element(),
            Step::Credentials => self.service_tile(self.service, 56.).into_any_element(),
            Step::Provider => img("brand/picacho.svg")
                .w(px(64.))
                .h(px(54.))
                .into_any_element(),
            Step::Email => self.service_tile(Service::Other, 56.).into_any_element(),
        };
        let width = if in_modal {
            560.
        } else if self.step == Step::Provider {
            540.
        } else {
            400.
        };
        let mut card = div()
            .w_full()
            .max_w(px(width))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_5()
            .child(div().min_h(px(56.)).flex().items_center().child(hero))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(if in_modal { 22. } else { 26. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_center()
                            .child(title),
                    )
                    .when(!subtitle.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(14.))
                                .text_color(rgb(c.muted))
                                .text_center()
                                .child(subtitle),
                        )
                    }),
            );
        match self.step {
            Step::Provider => {
                card = card.child(div().w_full().mt_1().child(self.provider_picker(cx)))
            }
            Step::Email => card = card.child(div().w_full().mt_2().child(self.setup_form(cx))),
            Step::Credentials if !browser => {
                card = card.child(div().w_full().mt_2().child(self.setup_form(cx)))
            }
            Step::Credentials if self.connecting => {
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(13.))
                        .text_color(rgb(c.muted))
                        .child(spinner(16., c.accent))
                        .child("Waiting for your browser…"),
                )
            }
            _ => {}
        }
        if matches!(self.step, Step::Email | Step::Credentials) && !self.status.is_empty() {
            card = card.child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_center()
                    .text_color(rgb(if self.error { c.danger } else { c.muted }))
                    .child(self.status.clone()),
            );
        }
        let primary = match self.step {
            Step::Welcome => Some(
                self.button("onboard-start", "Get Started", "", Action::Start, true, cx)
                    .into_any_element(),
            ),
            Step::Done => Some(
                self.button("onboard-finish", "Open Inbox", "", Action::Finish, true, cx)
                    .into_any_element(),
            ),
            Step::Provider => None,
            Step::Credentials if browser && self.connecting => None,
            _ => Some(self.setup_submit(cx)),
        };
        let mut actions = div()
            .w_full()
            .mt_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .children(primary.map(|p| div().w_full().max_w(px(400.)).flex().flex_col().child(p)));
        let back = match self.step {
            Step::Provider if !in_modal => Some("Back"),
            Step::Email | Step::Credentials => Some(if self.connecting {
                "Cancel"
            } else {
                "Choose another provider"
            }),
            _ => None,
        };
        if let Some(back) = back {
            actions = actions.child(self.link("onboard-back", back, Action::SetupBack, cx));
        }
        #[cfg(feature = "sample-data")]
        if self.step == Step::Welcome {
            actions = actions.child(self.link(
                "onboard-sample",
                "Explore a sample inbox",
                Action::Sample,
                cx,
            ));
        }
        if self.step == Step::Done {
            actions = actions.child(self.link(
                "onboard-another",
                "Add another account",
                Action::Connect,
                cx,
            ));
        }
        card.child(actions)
            .with_animation(
                ("onboard-step", self.anim),
                Animation::new(Duration::from_millis(320)).with_easing(|t| 1. - (1. - t).powi(3)),
                |d, t| d.opacity(t).ml(px((1. - t) * 28.)),
            )
            .into_any_element()
    }
    fn onboarding(&self, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = self.colors();
        let close = self.close_listener(cx);
        let current = match self.step {
            Step::Welcome => 0,
            Step::Provider => 1,
            Step::Email | Step::Credentials => 2,
            Step::Done => 3,
        };
        let dots = div().flex().gap_2().children((0..4).map(|i| {
            div()
                .h(px(6.))
                .w(px(if i == current { 20. } else { 6. }))
                .rounded_full()
                .bg(rgb(if i <= current { c.accent } else { c.border }))
        }));
        div()
            .id("onboarding")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .bg(rgb(c.bg))
            .map(|d| {
                let r = window::window_corners(w);
                d.rounded_tl(r.top_left)
                    .rounded_tr(r.top_right)
                    .rounded_bl(r.bottom_left)
                    .rounded_br(r.bottom_right)
            })
            .child(window::client_titlebar(
                div()
                    .id("onboarding-titlebar")
                    .h(px(TITLEBAR_HEIGHT))
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr_3()
                    .gap_1()
                    .on_click(window::on_titlebar_double_click),
                c,
                true,
                close,
                w,
                cx,
            ))
            .child(
                div()
                    .id("onboarding-body")
                    .track_scroll(&self.onboarding_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px_4()
                    .py_8()
                    .child(
                        div()
                            .my_auto()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .child(self.setup_view(cx)),
                    ),
            )
            .map(|d| {
                // The scrollbar sits over the body only, above the page dots.
                d.child(
                    div()
                        .absolute()
                        .top(px(TITLEBAR_HEIGHT))
                        .left_0()
                        .right_0()
                        .bottom(px(56.))
                        .child(Scrollbar::vertical(&self.onboarding_scroll)),
                )
            })
            .child(div().h(px(56.)).flex().items_center().child(dots))
            .into_any_element()
    }
    fn popup_menu(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        let mut panel = div()
            .min_w(px(200.))
            .p_1()
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .text_size(px(13.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let any_checks = menu
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Item { icon: "", .. }));
        for (i, e) in menu.entries.iter().enumerate() {
            match e {
                Entry::Separator => {
                    panel = panel.child(div().h(px(1.)).mx_2().my_1().bg(rgb(c.border)));
                }
                Entry::Item {
                    label,
                    icon,
                    action,
                    checked,
                    danger,
                } => {
                    let action = action.clone();
                    let color = if *danger { c.danger } else { c.text };
                    panel = panel.child(
                        div()
                            .id(("menu-item", i))
                            .debug_selector(|| format!("menu-item-{i}"))
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_color(rgb(color))
                            .hover(move |s| s.bg(rgb(c.soft)))
                            .child(
                                div()
                                    .size(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(if icon.is_empty() {
                                        if *checked && any_checks {
                                            icon_el("check", 14., c.accent).into_any_element()
                                        } else {
                                            div().into_any_element()
                                        }
                                    } else {
                                        icon_el(icon, 15., if *danger { c.danger } else { c.muted })
                                            .into_any_element()
                                    }),
                            )
                            .child(div().flex_1().pr_4().child(label.clone()))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.menu = None;
                                this.handle(action.clone(), w, cx)
                            })),
                    );
                }
            }
        }
        let close =
            |this: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.menu = None;
                cx.notify();
            };
        div()
            .id("menu-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(close))
            .on_mouse_down(MouseButton::Right, cx.listener(close))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        panel.with_animation(
                            ("popup-menu", menu.key),
                            Animation::new(Duration::from_millis(120))
                                .with_easing(|t| 1. - (1. - t).powi(3)),
                            |d, t| d.opacity(t).mt(px((1. - t) * -4.)),
                        ),
                    ),
            )
            .into_any_element()
    }
    fn remove_modal(&self, id: &str) -> AnyElement {
        let c = self.colors();
        let email = self
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.email.clone())
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("Remove {email}?")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Its local mail and drafts are deleted. Mail on the server is kept."),
            )
            .into_any_element()
    }
    fn setup_submit(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.discovering || self.connecting;
        let (label, action) = if self.step == Step::Credentials {
            (
                if self.connecting {
                    "Connecting…"
                } else if self.service.sign_in() != SignIn::Password {
                    "Open Browser Again"
                } else {
                    "Connect"
                },
                Action::SubmitAccount,
            )
        } else {
            (
                if self.discovering {
                    "Looking up…"
                } else {
                    "Continue"
                },
                Action::Discover,
            )
        };
        let fg = c.on_accent;
        self.button("setup-submit", label, "", action, true, cx)
            .min_w(px(120.))
            .when(busy, |d| d.opacity(0.7).child(spinner(14., fg)))
            .when(!busy, |d| d.child(icon_el("arrow-right", 14., fg)))
            .when(busy, |d| d.bg(rgb(c.accent)))
            .into_any_element()
    }
    fn modal(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let modal = self.modal.as_ref().unwrap();
        let title = match modal {
            Modal::Connect => "Add Account",
            Modal::Reconnect(_) => "Sign In Again",
            Modal::Settings => "Settings",
            Modal::About => "About Picacho",
            Modal::Compose => "New Message",
            Modal::Help => "Keyboard Shortcuts",
            Modal::RemoveAccount(_) => "Remove Account",
        };
        let content = match modal {
            Modal::Connect => self.setup_view(cx),
            Modal::Reconnect(id) => self.reconnect_form(id),
            Modal::Settings => self.settings_modal(window, cx),
            Modal::About => self.about_modal(),
            Modal::Compose => self.compose_modal(cx),
            Modal::Help => self.help_modal(),
            Modal::RemoveAccount(id) => self.remove_modal(id),
        };
        let available_height = (window.viewport_size().height - px(32.)).max(px(0.));
        let available_width = (window.viewport_size().width - px(32.)).max(px(0.));
        let mut panel = div()
            .id("modal-panel")
            .debug_selector(|| "modal-panel".into())
            .flex()
            .flex_col()
            .w(px(match modal {
                Modal::Compose => 720.,
                Modal::Connect => 680.,
                Modal::Help => 460.,
                Modal::RemoveAccount(_) | Modal::Reconnect(_) | Modal::About => 400.,
                Modal::Settings => 820.,
            }))
            .max_w(available_width)
            .when(*modal == Modal::Settings, |d| {
                d.h(px(600.).min(available_height))
            })
            .max_h(available_height)
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
                    .px_5()
                    .py_3()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(15.))
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
            .child(match modal {
                // Settings scrolls its own sections beside a fixed sidebar.
                Modal::Settings => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(content)
                    .into_any_element(),
                Modal::Connect => panes::scroll_frame(
                    div()
                        .id("modal-content")
                        .track_scroll(&self.modal_scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(content),
                    &self.modal_scroll,
                )
                .into_any_element(),
                _ => panes::scroll_frame(
                    div()
                        .id("modal-content")
                        .track_scroll(&self.modal_scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_5()
                        .child(content),
                    &self.modal_scroll,
                )
                .into_any_element(),
            });
        if !self.status.is_empty() && *modal != Modal::Connect {
            panel = panel.child(
                div()
                    .id("modal-status")
                    .max_h(px(48.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pb_3()
                    .text_size(px(12.))
                    .text_color(rgb(if self.error { c.danger } else { c.accent }))
                    .child(self.status.clone()),
            );
        }
        let mut footer = div()
            .flex_shrink_0()
            .flex_wrap()
            .border_t_1()
            .border_color(rgb(c.border))
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_2();
        if matches!(modal, Modal::Connect | Modal::Settings) {
            // These carry their own buttons.
            return self.modal_frame(panel, window);
        } else if let Modal::RemoveAccount(id) = modal {
            footer = footer
                .child(div().flex_1())
                .child(self.button("remove-cancel", "Cancel", "", Action::Close, false, cx))
                .child(
                    self.button(
                        "remove-confirm",
                        "Remove",
                        "",
                        Action::Remove(id.clone()),
                        true,
                        cx,
                    )
                    .bg(rgb(c.danger)),
                );
        } else if let Modal::Reconnect(id) = modal {
            footer = footer.child(div().flex_1()).child(self.button(
                "reconnect-submit",
                if self.connecting {
                    "Signing in…"
                } else {
                    "Sign In"
                },
                "",
                Action::SubmitReconnect(id.clone()),
                true,
                cx,
            ));
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
                .child(self.button("save-close", "Save Draft", "", Action::Close, false, cx))
                .child(
                    self.button(
                        "send",
                        if self.sending { "Sending…" } else { "Send" },
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
                "",
                Action::Close,
                true,
                cx,
            ));
        }
        panel = panel.child(footer);
        self.modal_frame(panel, window)
    }
    fn modal_frame(&self, panel: Stateful<Div>, window: &Window) -> AnyElement {
        let corners = window::window_corners(window);
        div()
            .occlude()
            .absolute()
            .inset_0()
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .bg(rgba(0x101c2c66))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
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
            .map(|d| window::window_frame(d, c, window))
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
            .on_action(cx.listener(|this, _: &Refresh, w, cx| this.handle(Action::Refresh, w, cx)))
            .on_action(cx.listener(|this, _: &Search, w, cx| {
                this.handle(Action::OpenSearch, w, cx);
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
            .on_action(cx.listener(|this, _: &ToggleSidebar, w, cx| {
                if !this.onboarding {
                    this.handle(Action::ToggleSidebar, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAllMail, w, cx| {
                if this.modal.is_none() && !this.onboarding {
                    this.handle(Action::SelectAll, w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SendMail, _, cx| this.send(cx)))
            .on_action(cx.listener(|this, _: &ShowAbout, w, cx| this.handle(Action::About, w, cx)))
            .on_action(
                cx.listener(|this, _: &AddAccount, w, cx| this.handle(Action::Connect, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ReplyAll, w, cx| this.new_compose(2, w, cx)))
            .on_action(cx.listener(|this, _: &ForwardMail, w, cx| this.new_compose(3, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRead, w, cx| {
                if this.modal.is_none()
                    && let Some(id) = this.selected.clone()
                {
                    this.handle(Action::Read(id), w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveToSpam, w, cx| {
                if this.modal.is_none() {
                    this.handle(Action::Move(Folder::Junk), w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MarkAllRead, w, cx| {
                if let Some(id) = this.account.clone() {
                    let mailbox = this.mailbox();
                    this.handle(Action::MarkAllRead(id, mailbox), w, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &GoInbox, w, cx| this.go(Folder::Inbox, w, cx)))
            .on_action(cx.listener(|this, _: &GoStarred, w, cx| this.go(Folder::Starred, w, cx)))
            .on_action(cx.listener(|this, _: &GoSent, w, cx| this.go(Folder::Sent, w, cx)))
            .on_action(cx.listener(|this, _: &GoDrafts, w, cx| this.go(Folder::Drafts, w, cx)))
            .on_action(cx.listener(|this, _: &GoArchive, w, cx| this.go(Folder::Archive, w, cx)))
            .on_action(cx.listener(|this, _: &GoSpam, w, cx| this.go(Folder::Junk, w, cx)))
            .on_action(cx.listener(|this, _: &GoTrash, w, cx| this.go(Folder::Trash, w, cx)))
            .on_action(cx.listener(|this, _: &ShowAllMail, w, cx| {
                this.handle(Action::SetFilter(Filter::All), w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowUnread, w, cx| {
                this.handle(Action::SetFilter(Filter::Unread), w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowStarred, w, cx| {
                this.handle(Action::SetFilter(Filter::Starred), w, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAttachments, w, cx| {
                this.handle(Action::SetFilter(Filter::Attachments), w, cx)
            }))
            .on_action(cx.listener(|_, _: &Minimize, w, _| w.minimize_window()))
            .on_action(cx.listener(|_, _: &Zoom, w, _| w.zoom_window()))
            .when(self.onboarding, |d| d.child(self.onboarding(window, cx)))
            .when(!self.onboarding, |d| self.main(d, window, cx))
            .map(|d| self.track_resize(d, cx))
            .when(self.modal.is_some(), |d| d.child(self.modal(window, cx)))
            .when(self.menu.is_some(), |d| d.child(self.popup_menu(cx)))
    }
}
impl MailApp {
    fn main(&self, d: Stateful<Div>, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        let width = window.viewport_size().width;
        let single = width < px(900.);
        let compact = width < px(1180.);
        self.compact.set(compact);
        // Panes that paint their own background round the window's lower
        // corners they touch; GPUI clips children only to rectangles.
        let corners = window::window_corners(window);
        let show_sidebar = !(single && self.selected.is_some() && width < px(480.));
        let show_list = !single || self.selected.is_none();
        let show_reader = !single || self.selected.is_some();
        let list_corners = Corners {
            bottom_left: if show_sidebar {
                px(0.)
            } else {
                corners.bottom_left
            },
            bottom_right: if show_reader {
                px(0.)
            } else {
                corners.bottom_right
            },
            ..Corners::default()
        };
        let reader_corners = Corners {
            bottom_left: if show_sidebar || show_list {
                px(0.)
            } else {
                corners.bottom_left
            },
            bottom_right: corners.bottom_right,
            ..Corners::default()
        };
        let layout = self.layout(f32::from(width), compact);
        self.window_width.set(f32::from(width));
        let resizable = !single;
        d.child(self.topbar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(show_sidebar, |d| {
                        d.child(self.sidebar(layout.sidebar, layout.sidebar_collapsed, cx))
                    })
                    .when(show_sidebar && resizable && !compact, |d| {
                        d.child(self.resize_handle(Pane::Sidebar, cx))
                    })
                    .when(show_list, |d| {
                        d.child(self.message_list(single, list_corners, cx))
                    })
                    .when(show_reader && resizable, |d| {
                        d.child(self.resize_handle(Pane::Reader, cx))
                    })
                    .when(show_reader, |d| {
                        d.child(self.reader_pane(
                            resizable.then_some(layout.reader),
                            reader_corners,
                            window,
                            cx,
                        ))
                    }),
            )
            .when(self.modal.is_none() && !self.status.is_empty(), |d| {
                d.child(
                    div()
                        .id("notice-toast")
                        .debug_selector(|| "notice-toast".into())
                        .absolute()
                        .bottom_5()
                        .right_5()
                        .max_w(px(500.).min((width - px(40.)).max(px(0.))))
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
    }
}
/// Overshooting ease-out. GPUI requires easing output within 0..=1, so apply
/// this inside an animator rather than with `with_easing`.
fn fmt_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
/// A circular progress track starting at twelve o'clock.
fn ring(progress: f32, track: u32, fill: u32) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, _, w, _| {
            let center = bounds.center();
            let radius = bounds.size.width / px(1.) / 2. - 1.5;
            let arc = |from: f32, to: f32| {
                let mut path = PathBuilder::stroke(px(3.));
                let steps = ((to - from) * 64.).ceil().max(2.) as usize;
                for i in 0..=steps {
                    let a = std::f32::consts::TAU * (from + (to - from) * i as f32 / steps as f32)
                        - std::f32::consts::FRAC_PI_2;
                    let p = point(
                        center.x + px(radius * a.cos()),
                        center.y + px(radius * a.sin()),
                    );
                    if i == 0 {
                        path.move_to(p)
                    } else {
                        path.line_to(p)
                    }
                }
                path.build()
            };
            if let Ok(path) = arc(0., 1.) {
                w.paint_path(path, rgb(track));
            }
            if progress > 0.
                && let Ok(path) = arc(0., progress.min(1.))
            {
                w.paint_path(path, rgb(fill));
            }
        },
    )
}
fn back_out(t: f32) -> f32 {
    let (c1, t) = (1.70158, t - 1.);
    1. + (c1 + 1.) * t.powi(3) + c1 * t.powi(2)
}
fn spinner(size: f32, color: u32) -> impl IntoElement {
    svg()
        .path("icons/loader-circle.svg")
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
        .with_animation(
            "spinner",
            Animation::new(Duration::from_millis(900)).repeat(),
            |s, t| s.with_transformation(Transformation::rotate(percentage(t))),
        )
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
        Folder::Other => "folder",
    }
}
/// A text field on the app's background. Inputs otherwise take the theme's
/// background, which is transparent on Linux so `Root` does not paint square
/// corners under the rounded window frame. Disabled fields keep the theme's
/// muted fill.
fn field(input: Input, c: Colors, disabled: bool) -> Input {
    input.when(!disabled, |i| i.bg(rgb(c.bg)))
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
        "star-filled",
        "mail-open",
        "select",
        "monitor",
        "lock",
        "warning",
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
    let app = Application::new().with_assets(crate::display::assets::Assets);
    app.on_open_urls(crate::sync::oauth::opened);
    app.run(move |cx| {
        gpui_component::init(cx);
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.set_menus(menus());
        // `secondary` is Command on macOS and Control elsewhere.
        cx.bind_keys([
            KeyBinding::new("secondary-q", Quit, Some("Mail")),
            KeyBinding::new("secondary-w", CloseWindow, Some("Mail")),
            KeyBinding::new("secondary-,", ShowAccounts, Some("Mail")),
            KeyBinding::new("secondary-n", Compose, Some("Mail")),
            KeyBinding::new("secondary-f", Search, Some("Mail")),
            KeyBinding::new("secondary-r", Refresh, Some("Mail")),
            KeyBinding::new("escape", Close, Some("Mail")),
            KeyBinding::new("alt-down", Next, Some("Mail")),
            KeyBinding::new("alt-up", Previous, Some("Mail")),
            KeyBinding::new("secondary-shift-a", Archive, Some("Mail")),
            KeyBinding::new("secondary-backspace", DeleteMail, Some("Mail")),
            KeyBinding::new("secondary-shift-r", Reply, Some("Mail")),
            KeyBinding::new("secondary-alt-r", ReplyAll, Some("Mail")),
            KeyBinding::new("secondary-shift-f", ForwardMail, Some("Mail")),
            KeyBinding::new("secondary-shift-u", ToggleRead, Some("Mail")),
            KeyBinding::new("secondary-shift-j", MoveToSpam, Some("Mail")),
            KeyBinding::new("secondary-shift-s", ToggleStar, Some("Mail")),
            KeyBinding::new("secondary-shift-l", ToggleTheme, Some("Mail")),
            KeyBinding::new("secondary-enter", SendMail, Some("Mail")),
            KeyBinding::new(
                if cfg!(target_os = "macos") {
                    "ctrl-cmd-s"
                } else {
                    "ctrl-alt-s"
                },
                ToggleSidebar,
                Some("Mail"),
            ),
            KeyBinding::new("secondary-a", SelectAllMail, Some("Mail")),
            KeyBinding::new("secondary-1", GoInbox, Some("Mail")),
            KeyBinding::new("secondary-2", GoStarred, Some("Mail")),
            KeyBinding::new("secondary-3", GoSent, Some("Mail")),
            KeyBinding::new("secondary-4", GoDrafts, Some("Mail")),
            KeyBinding::new("secondary-5", GoArchive, Some("Mail")),
            KeyBinding::new("secondary-6", GoSpam, Some("Mail")),
            KeyBinding::new("secondary-7", GoTrash, Some("Mail")),
            KeyBinding::new("secondary-m", Minimize, Some("Mail")),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(360.), px(320.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Picacho".into()),
                    // macOS draws the window buttons over the app's own bar.
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: Some(point(px(13.), px(12.))),
                }),
                // Draw the title bar ourselves on Linux so the topbar is the
                // window's title bar (GNOME never offers server decorations).
                window_decorations: cfg!(any(target_os = "linux", target_os = "freebsd"))
                    .then_some(WindowDecorations::Client),
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

/// The menu bar. On macOS the first menu is the application menu.
fn menus() -> Vec<Menu> {
    use gpui_component::input;
    let menu = |name: &str, items: Vec<MenuItem>| Menu {
        name: name.to_owned().into(),
        items,
    };
    vec![
        menu(
            "Picacho",
            vec![
                MenuItem::action("About Picacho", ShowAbout),
                MenuItem::separator(),
                MenuItem::action("Settings…", ShowAccounts),
                MenuItem::action("Add Account…", AddAccount),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Picacho", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Picacho", Quit),
            ],
        ),
        menu(
            "File",
            vec![
                MenuItem::action("New Message", Compose),
                MenuItem::action("Add Account…", AddAccount),
                MenuItem::separator(),
                MenuItem::action("Check for New Mail", Refresh),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        ),
        menu(
            "Edit",
            vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Find", Search),
            ],
        ),
        menu(
            "View",
            vec![
                MenuItem::action("All Messages", ShowAllMail),
                MenuItem::action("Unread", ShowUnread),
                MenuItem::action("Starred", ShowStarred),
                MenuItem::action("Has Attachments", ShowAttachments),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Dark Mode", ToggleTheme),
            ],
        ),
        menu(
            "Mailbox",
            vec![
                MenuItem::action("Inbox", GoInbox),
                MenuItem::action("Starred", GoStarred),
                MenuItem::action("Sent", GoSent),
                MenuItem::action("Drafts", GoDrafts),
                MenuItem::action("Archive", GoArchive),
                MenuItem::action("Spam", GoSpam),
                MenuItem::action("Trash", GoTrash),
                MenuItem::separator(),
                MenuItem::action("Mark All as Read", MarkAllRead),
                MenuItem::action("Refresh", Refresh),
            ],
        ),
        menu(
            "Message",
            vec![
                MenuItem::action("Reply", Reply),
                MenuItem::action("Reply All", ReplyAll),
                MenuItem::action("Forward", ForwardMail),
                MenuItem::separator(),
                MenuItem::action("Mark as Read / Unread", ToggleRead),
                MenuItem::action("Star / Unstar", ToggleStar),
                MenuItem::separator(),
                MenuItem::action("Archive", Archive),
                MenuItem::action("Move to Spam", MoveToSpam),
                MenuItem::action("Move to Trash", DeleteMail),
                MenuItem::separator(),
                MenuItem::action("Next Message", Next),
                MenuItem::action("Previous Message", Previous),
                MenuItem::separator(),
                MenuItem::action("Send", SendMail),
            ],
        ),
        menu(
            "Window",
            vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        ),
        menu(
            "Help",
            vec![MenuItem::action("Keyboard Shortcuts", ShowHelp)],
        ),
    ]
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

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` exports a `test` macro that would shadow the built-in one.
    use std::prelude::v1::test;

    pub(super) fn open(
        store: Arc<Store>,
        cx: &mut TestAppContext,
    ) -> (Entity<MailApp>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let mut app = None;
        let window = cx.add_window(|w, cx| {
            let view = cx.new(|cx| MailApp::new(store, w, cx));
            app = Some(view.clone());
            Root::new(view, w, cx)
        });
        let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
        cx.run_until_parked();
        (app.unwrap(), cx)
    }
    pub(super) fn store() -> Arc<Store> {
        let dir = std::env::temp_dir().join(format!("mail-ui-{}", uuid::Uuid::new_v4()));
        Arc::new(Store::open(&dir).unwrap())
    }
    pub(super) fn act(view: &Entity<MailApp>, action: Action, cx: &mut VisualTestContext) {
        view.update_in(cx, |this, w, cx| this.handle(action, w, cx));
        cx.run_until_parked();
    }
    fn type_email(view: &Entity<MailApp>, email: &str, cx: &mut VisualTestContext) {
        view.update_in(cx, |this, w, cx| {
            MailApp::set(&this.form[EMAIL], email.to_owned(), w, cx)
        });
    }

    #[gpui::test]
    fn onboarding_offers_services_then_discovers_others(cx: &mut TestAppContext) {
        let (view, cx) = open(store(), cx);
        view.read_with(cx, |this, _| {
            assert!(this.onboarding);
            assert!(this.step == Step::Welcome);
        });
        act(&view, Action::Start, cx);
        view.read_with(cx, |this, _| assert!(this.step == Step::Provider));
        frame(cx);
        assert!(
            cx.debug_bounds("service-Pinal").is_some(),
            "Pinal Mail is offered"
        );
        assert!(cx.debug_bounds("service-ProtonBridge").is_some());
        // Password services use fixed servers and never ask for them.
        act(&view, Action::ChooseService(Service::ICloud), cx);
        view.read_with(cx, |this, cx| {
            assert!(this.step == Step::Credentials && !this.manual);
            assert_eq!(
                MailApp::value(&this.form[IMAP_HOST], cx),
                "imap.mail.me.com"
            );
            assert_eq!(this.setup_account(cx).smtp_port, 587);
        });
        // Validation stops before any network connection is attempted.
        act(&view, Action::SubmitAccount, cx);
        view.read_with(cx, |this, _| assert!(!this.connecting && this.error));
        act(&view, Action::SetupBack, cx);
        act(&view, Action::ChooseService(Service::ProtonBridge), cx);
        type_email(&view, "me@proton.me", cx);
        view.read_with(cx, |this, cx| {
            let a = this.setup_account(cx);
            assert!(a.incoming_starttls && a.incoming_port == 1143 && a.smtp_port == 1025);
            assert!(a.validate().is_ok());
        });
        // Another provider: look the address up.
        act(&view, Action::SetupBack, cx);
        act(&view, Action::ChooseService(Service::Other), cx);
        view.read_with(cx, |this, _| assert!(this.step == Step::Email));
        type_email(&view, "not an address", cx);
        act(&view, Action::Discover, cx);
        view.read_with(cx, |this, _| {
            assert!(this.step == Step::Email, "invalid email must not advance");
            assert!(this.error);
        });
        // A Gmail address gets Gmail's sign-in; without a built-in Google
        // client that is an app password.
        type_email(&view, "someone@gmail.com", cx);
        act(&view, Action::Discover, cx);
        view.read_with(cx, |this, cx| {
            assert!(this.step == Step::Credentials);
            assert!(this.service == Service::Gmail && !this.manual);
            assert_eq!(MailApp::value(&this.form[IMAP_HOST], cx), "imap.gmail.com");
        });
        act(&view, Action::SetupBack, cx);
        view.read_with(cx, |this, _| assert!(this.step == Step::Provider));
        act(&view, Action::ChooseService(Service::Other), cx);
        type_email(&view, "someone@no-such-domain.invalid", cx);
        act(&view, Action::Discover, cx);
        view.read_with(cx, |this, cx| {
            assert!(this.step == Step::Credentials);
            assert!(this.manual, "failed discovery shows server settings");
            assert!(this.provider == Provider::Imap);
            assert_eq!(
                MailApp::value(&this.form[IMAP_HOST], cx),
                "imap.no-such-domain.invalid"
            );
        });
        act(&view, Action::Protocol(Provider::Pop3), cx);
        view.read_with(cx, |this, cx| {
            assert_eq!(MailApp::value(&this.form[IMAP_PORT], cx), "995")
        });
        act(&view, Action::SubmitAccount, cx);
        view.read_with(cx, |this, _| {
            assert!(!this.connecting);
            assert!(this.error && this.onboarding);
        });
    }

    #[gpui::test]
    fn settings_change_theme_and_mailbox_sync(cx: &mut TestAppContext) {
        let (view, cx, store) = two_accounts(cx);
        let first = view.read_with(cx, |this, _| this.accounts[0].id.clone());
        act(&view, Action::Settings, cx);
        frame(cx);
        assert!(cx.debug_bounds("settings-general").is_some());
        act(&view, Action::SetAccent(Accent::Purple), cx);
        act(&view, Action::SetAppearance(Appearance::Dark), cx);
        view.read_with(cx, |this, _| {
            assert!(this.dark);
            assert_eq!(this.colors().accent, Accent::Purple.colors(true).0);
        });
        assert_eq!(store.preference("accent").as_deref(), Some("purple"));
        assert_eq!(store.preference("theme").as_deref(), Some("dark"));
        act(&view, Action::AccountSettings(first.clone()), cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.page, Page::Account(first.clone()))
        });
        let sync = SyncSettings {
            days: 30,
            refresh_minutes: 15,
            notify: false,
        };
        act(&view, Action::SetSync(first.clone(), sync), cx);
        let saved = store.accounts().unwrap();
        assert_eq!(saved.iter().find(|a| a.id == first).unwrap().sync, sync);
        // Adding an account opens the same provider list in a dialog.
        act(&view, Action::Connect, cx);
        view.read_with(cx, |this, _| {
            assert!(this.modal == Some(Modal::Connect));
            assert!(this.step == Step::Provider);
        });
        frame(cx);
        assert!(cx.debug_bounds("service-Pinal").is_some());
    }

    pub(super) fn two_accounts(
        cx: &mut TestAppContext,
    ) -> (Entity<MailApp>, &mut VisualTestContext, Arc<Store>) {
        let store = store();
        for (name, email) in [("Work", "work@example.com"), ("Home", "home@example.com")] {
            let mut a = Account::new(Provider::Imap);
            a.name = name.into();
            a.email = email.into();
            store.save_account(&a).unwrap();
            let mut mails = demo_mail();
            for m in &mut mails {
                m.account = a.id.clone();
                m.id = format!("{}:{}", a.id, m.id);
            }
            store.save_messages(&mails).unwrap();
            store
                .save_folders(
                    &a.id,
                    &[RemoteFolder {
                        name: "INBOX/Receipts".into(),
                        label: "Receipts".into(),
                        depth: 0,
                    }],
                )
                .unwrap();
        }
        let (view, cx) = open(store.clone(), cx);
        (view, cx, store)
    }
    /// Test windows draw before dispatching input, so a no-op mouse move
    /// brings the rendered frame up to date.
    pub(super) fn frame(cx: &mut VisualTestContext) {
        cx.simulate_event(MouseMoveEvent {
            position: point(px(1.), px(1.)),
            pressed_button: None,
            modifiers: Modifiers::none(),
        });
    }
    fn center(cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
        frame(cx);
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not rendered"))
            .center()
    }

    #[gpui::test]
    fn sidebar_lists_accounts_with_folders(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let (first, second) = view.read_with(cx, |this, _| {
            assert!(!this.onboarding);
            (this.accounts[0].id.clone(), this.accounts[1].id.clone())
        });
        view.read_with(cx, |this, _| {
            assert_eq!(this.account.as_ref(), Some(&first));
            assert_eq!(this.messages.len(), 10, "cached inbox shows before syncing");
            assert_eq!(this.unread_for(&first, &Folder::Inbox.into()), 3);
        });
        // Every account's folders are listed; clicking another account's
        // server folder opens it.
        let receipts = Mailbox::Remote("INBOX/Receipts".into());
        let target = view.read_with(cx, |_, _| format!("nav-{second}-{}", receipts.key()));
        let at = center(cx, Box::leak(target.into_boxed_str()));
        cx.simulate_click(at, Modifiers::none());
        view.read_with(cx, |this, _| {
            assert_eq!(this.account.as_ref(), Some(&second));
            assert_eq!(this.mailbox(), receipts);
            assert_eq!(this.mailbox_label(), "Receipts");
        });
        // Collapsing an account hides its folders.
        let other_inbox: &'static str = Box::leak(format!("nav-{second}-inbox").into_boxed_str());
        let before = center(cx, other_inbox);
        act(&view, Action::ToggleAccount(first.clone()), cx);
        let after = center(cx, other_inbox);
        assert!(after.y < before.y - px(150.), "{before:?} -> {after:?}");
        // The collapsed sidebar keeps icons and draws the download ring.
        act(&view, Action::ToggleSidebar, cx);
        cx.run_until_parked();
        frame(cx);
        view.read_with(cx, |this, _| {
            assert!(this.sidebar_collapsed);
            assert_eq!(this.download.total, 20);
            assert_eq!(this.download.missing, 20);
        });
        assert!(cx.debug_bounds("download-ring").is_some());
    }

    #[gpui::test]
    fn context_menus_and_bulk_selection(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let row = view.read_with(cx, |this, _| this.messages[1].id.clone());
        let selector = Box::leak(format!("row-{row}").into_boxed_str());
        let at = center(cx, selector);
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            let menu = this.menu.as_ref().expect("right-click opens a menu");
            assert!(
                menu.entries
                    .iter()
                    .any(|e| matches!(e, Entry::Item { label, .. } if label.as_ref() == "Archive"))
            );
        });
        // Choosing "Select" from the menu starts selection mode.
        let select = view.read_with(cx, |this, _| {
            this.menu
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .position(|e| matches!(e, Entry::Item { label, .. } if label.as_ref() == "Select"))
                .unwrap()
        });
        let item = center(
            cx,
            Box::leak(format!("menu-item-{select}").into_boxed_str()),
        );
        cx.simulate_click(item, Modifiers::none());
        view.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            assert!(this.selecting);
            assert!(this.selection.contains(&row));
        });
        frame(cx);
        assert!(
            cx.debug_bounds("bulk-archive").is_some(),
            "selection bar is shown"
        );
        // Shift-click extends the selection.
        let last = view.read_with(cx, |this, _| this.messages[3].id.clone());
        let at = center(cx, Box::leak(format!("row-{last}").into_boxed_str()));
        cx.simulate_click(at, Modifiers::shift());
        view.read_with(cx, |this, _| assert_eq!(this.selection.len(), 3));
        // The avatar doubles as a checkbox and never opens the message.
        let fourth = view.read_with(cx, |this, _| this.messages[5].id.clone());
        let lead = center(cx, Box::leak(format!("lead-{fourth}").into_boxed_str()));
        cx.simulate_click(lead, Modifiers::none());
        view.read_with(cx, |this, _| {
            assert_eq!(this.selection.len(), 4);
            assert!(this.selected.is_none());
        });
        cx.simulate_click(lead, Modifiers::none());
        view.read_with(cx, |this, _| assert_eq!(this.selection.len(), 3));
        // Bulk star is applied optimistically before the server responds.
        view.update_in(cx, |this, w, cx| {
            this.handle(Action::ApplySelection(Change::Star(true)), w, cx)
        });
        view.read_with(cx, |this, _| {
            let starred = this
                .messages
                .iter()
                .filter(|m| this.selection.contains(&m.id) && m.starred)
                .count();
            assert_eq!(starred, 3);
        });
        // There are no credentials in tests, so the server rejects the change
        // and it is rolled back.
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert!(!this.error && this.status.is_empty());
            assert!(this.account_issues[&this.accounts[0].id].contains_key(&IssueSource::Change));
            assert!(!this.messages[1].starred);
        });
        // Clearing the selection leaves selection mode; then a checkbox click
        // alone starts it again.
        act(&view, Action::StopSelecting, cx);
        view.read_with(cx, |this, _| assert!(!this.selecting));
        let lead = center(cx, Box::leak(format!("lead-{fourth}").into_boxed_str()));
        cx.simulate_click(lead, Modifiers::none());
        view.read_with(cx, |this, _| {
            assert!(this.selecting);
            assert_eq!(this.selection.len(), 1);
        });
        act(&view, Action::Close, cx);
        view.read_with(cx, |this, _| {
            assert!(!this.selecting, "Escape clears the selection")
        });
        // Search is hidden behind a button, focused on open, cleared on close.
        view.read_with(cx, |this, _| {
            assert!(!this.search_open);
            assert_eq!(this.mailbox_unread(), 3);
        });
        act(&view, Action::OpenSearch, cx);
        view.update_in(cx, |this, w, cx| {
            assert!(this.search_open);
            assert!(this.search.read(cx).focus_handle(cx).is_focused(w));
            MailApp::set(&this.search, "linear", w, cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |this, _| assert_eq!(this.visible.len(), 1));
        act(&view, Action::Close, cx);
        view.read_with(cx, |this, cx| {
            assert!(!this.search_open);
            assert!(MailApp::value(&this.search, cx).is_empty());
            assert_eq!(this.visible.len(), 10);
        });
        // Filter dropdown.
        act(&view, Action::FilterMenu, cx);
        view.read_with(cx, |this, _| assert!(this.menu.is_some()));
        act(&view, Action::SetFilter(Filter::Starred), cx);
        view.read_with(cx, |this, _| {
            assert!(this.menu.is_none());
            assert_eq!(this.visible.len(), 2);
        });
        act(&view, Action::SetFilter(Filter::Attachments), cx);
        view.read_with(cx, |this, _| assert_eq!(this.visible.len(), 1));
        act(&view, Action::SetFilter(Filter::All), cx);
        act(&view, Action::OldestFirst(true), cx);
        view.read_with(cx, |this, _| {
            let first = &this.messages[this.visible[0]];
            assert!(this.messages.iter().all(|m| m.date >= first.date));
        });
    }
}
