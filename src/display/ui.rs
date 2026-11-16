#[path = "accounts.rs"]
mod accounts;
#[path = "proton_login.rs"]
mod proton_login;
use crate::manage::backend::{KeyMethod, KeyPrompt};
use proton_login::ProtonStage;
#[path = "catalog.rs"]
mod catalog;
#[path = "compose.rs"]
mod compose;
#[cfg(test)]
#[path = "compose_tests.rs"]
mod compose_tests;
#[path = "composer.rs"]
mod composer;
use composer::{Composer, ComposerDrag, Placement};
#[cfg(all(debug_assertions, target_os = "macos"))]
#[path = "devdriver.rs"]
mod devdriver;
#[path = "datetime.rs"]
mod datetime;
#[path = "later.rs"]
mod later;
#[path = "filters.rs"]
mod filters;
use datetime::{DateTimePicker, PickerEvent};
#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;
#[path = "forwarding.rs"]
mod forwarding;
#[path = "local_ai.rs"]
mod ai;
#[cfg(all(test, not(target_os = "macos")))]
#[path = "local_ai_tests.rs"]
mod local_ai_tests;
#[cfg(test)]
#[path = "when_tests.rs"]
mod when_tests;
#[path = "message_window.rs"]
mod message_window;
#[path = "panes.rs"]
mod panes;
#[path = "prompt.rs"]
mod prompt;
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
#[path = "subscriptions.rs"]
mod subscriptions;
#[path = "thread.rs"]
mod thread;
#[path = "tree.rs"]
mod tree;
#[path = "views.rs"]
mod views;
#[path = "window.rs"]
mod window;
use super::html::{HtmlContextMenu, HtmlView};
use crate::manage::composing::{ComposeOpen, ComposeOptions, ReplyPlacement};
use crate::manage::reading::{EmailTheme, ReadingOptions, message_key};
use crate::manage::subscriptions::Subscription as MailSubscription;
use crate::manage::status::{AccountIssue, DownloadStatus as Download, IssueSource, SignInStep};
use crate::{
    manage::{
        backend::{Backend, Change},
        forwarding::{self as fwd, ALL_MAIL, AliasDisplay, ForwardingAccount, ForwardingService},
        model::*,
        store::Store,
    },
    sync::discover,
};
use gpui::{prelude::*, *};
use gpui_component::{
    Root, Theme, ThemeMode,
    input::{Input, InputEvent, InputModeKind, InputState, Textarea, TextareaState},
    scroll::{Scrollbar, ScrollbarMode},
    tooltip::Tooltip,
};
use std::{
    cell::Cell,
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
        GoAllMail,
        ShowAllMail,
        ShowUnread,
        ShowStarred,
        ShowAttachments,
        Minimize,
        Zoom,
        Hide,
        HideOthers,
        ShowAll,
        OpenInWindow
    ]
);

const TITLEBAR_HEIGHT: f32 = 38.;
/// Space around full-size Settings, the same on every side.
const MAXIMIZED_MARGIN: f32 = 40.;
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
/// The servers' user name, when it isn't the email address.
const LOGIN: usize = 6;
/// Proton: the authenticator app's code, when two-factor is on.
const TOTP: usize = 7;
/// Proton: the mailbox password, in two-password mode.
const MAILBOX_PASSWORD: usize = 8;
/// Proton: a security key's PIN, once the key asks for it.
const KEY_PIN: usize = 9;

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
    /// Problems that don't stop the account (yellow).
    warning: u32,
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
                warning: 0xf2c14e,
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
                warning: 0xb7791f,
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
/// A search for `value` in `field`, quoted when it has spaces.
fn search_term(field: &str, value: &str) -> String {
    let value = value.replace('"', "");
    if value.chars().any(char::is_whitespace) {
        format!("{field}:\"{value}\"")
    } else {
        format!("{field}:{value}")
    }
}
/// "Name <address>" as a header writes it, the name quoted when it holds
/// characters that would split it; the bare address when there is no name.
fn name_and_address(name: Option<&str>, email: &str) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => email.to_owned(),
        Some(name) => {
            let plain = name
                .chars()
                .all(|ch| ch.is_alphanumeric() || " !#$%&'*+-/=?^_`{|}~".contains(ch));
            if plain {
                format!("{name} <{email}>")
            } else {
                let quoted = name.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{quoted}\" <{email}>")
            }
        }
    }
}
/// The menu of an address in the open message's headers: copying it, and,
/// for a sender (`from`: the address to look its mail up by), writing to it
/// and finding its mail.
fn address_entries(name: Option<&str>, email: &str, from: Option<&str>) -> Vec<Entry> {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    let mut entries = Vec::new();
    if let Some(name) = name {
        entries.push(entry("Copy Name", "copy", Action::CopyText(name.to_owned())));
    }
    entries.push(entry("Copy Address", "copy", Action::CopyText(email.to_owned())));
    if name.is_some() {
        entries.push(entry(
            "Copy Name and Address",
            "copy",
            Action::CopyText(name_and_address(name, email)),
        ));
    }
    if let Some(from) = from {
        entries.push(Entry::Separator);
        entries.push(entry(
            "Send Email…",
            "compose",
            Action::ComposeTo(name_and_address(name, email)),
        ));
        entries.push(entry(
            "Show Mail From This Address",
            "search",
            Action::SearchFor(search_term("from", from)),
        ));
    }
    entries
}
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
    Help,
    RemoveAccount(String),
    /// Confirms deleting a folder on an account's server: (account, folder).
    DeleteFolder(String, String),
    /// Confirms deleting mail for good.
    Erase(Erase),
    /// Names a new label for messages.
    NewLabel(MoveTarget),
    Reconnect(String),
    /// The raw source of the open message.
    Original,
    /// Preset pictures for an account.
    AvatarPresets(String),
    /// A message wasn't sent because the alias in From no longer exists:
    /// (why, the address From was reset to).
    AliasGone(String, String),
    /// When to bring these messages back.
    Snooze(Vec<String>),
}
/// What is deleted for good, once confirmed.
#[derive(Clone, Debug, PartialEq)]
enum Erase {
    /// These messages.
    Mail(Vec<String>),
    /// Everything in an account's Trash or Spam.
    Mailbox(String, Folder),
}
/// A server folder being named in the sidebar, as top folders are.
#[derive(Clone, Debug, PartialEq)]
enum FolderEdit {
    /// Makes a folder inside this one, or at the top level.
    New(Option<String>),
    /// Renames this folder.
    Rename(String),
}
/// What the Original Message window shows of the open message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceView {
    Headers,
    Html,
    Original,
}
impl SourceView {
    pub(super) const ALL: [Self; 3] = [Self::Headers, Self::Html, Self::Original];
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Headers => "Headers",
            Self::Html => "HTML",
            Self::Original => "Original",
        }
    }
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
    /// When the menu opened. A trackpad's two-finger click also sends small
    /// scroll events; those must not close the menu it just opened.
    opened: std::time::Instant,
    /// The window it opened in, when not the main window (popped-out
    /// Settings draws its own menus).
    window: Option<WindowId>,
}
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Provider,
    Email,
    Credentials,
    Done,
}
/// Where Settings is shown: a dialog, filling the main window, or in a
/// window of its own (like a composer).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum SettingsPlace {
    #[default]
    Dialog,
    Maximized,
    Window,
}
/// A Settings section.
#[derive(Clone, Debug, PartialEq)]
enum Page {
    General,
    Reading,
    Composing,
    Forwarding,
    LocalAi,
    Account(String),
}
/// What a folder picker moves into the chosen folder.
#[derive(Clone, Debug, PartialEq)]
enum MoveTarget {
    /// These messages.
    Mail(Vec<String>),
    /// The selected messages.
    Selection,
    /// Every message from a subscription.
    Subscription(Box<MailSubscription>),
}
#[derive(Clone)]
enum Action {
    /// Proton sign-in: a security key or passkey, this way.
    ProtonKey(KeyMethod),
    /// Proton sign-in: Direct (its API) or Bridge.
    ProtonMode(Service),
    /// Proton sign-in: back to the password.
    ProtonBack,
    Compose,
    /// A new message to these addresses.
    ComposeTo(String),
    Reply(bool),
    Forward,
    Refresh,
    OpenMailbox(String, Mailbox),
    Select(String),
    /// Opens a message in a window of its own.
    OpenWindow(String),
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
    /// Opens or closes a sidebar folder.
    ToggleGroup(String),
    /// Offers a new account, folder or separator.
    AddMenu,
    NewFolder(Option<String>),
    NewSeparator(Option<String>),
    RenameFolder(String),
    DeleteFolder(String),
    /// Asks to delete an account's folder on its server: (account, folder).
    DeleteRemoteFolder(String, String),
    /// Deletes it, as confirmed in the dialog.
    ConfirmDeleteFolder(String, String),
    /// Asks for a new folder's name on an account's server: (account, parent).
    NewRemoteFolder(String, Option<String>),
    /// Asks for a new name for an account's folder: (account, folder).
    RenameRemoteFolder(String, String),
    RemoveSeparator(String),
    ConfirmRemove(String),
    ToggleSidebar,
    ToggleReader,
    ChooseAvatar(String),
    RemoveAvatar(String),
    /// The account picture menu: provider, presets, upload, remove.
    AvatarMenu(String),
    ProviderAvatar(String),
    AvatarPresets(String),
    SetPresetAvatar(String, &'static str),
    StartSelecting(String),
    ToggleSelected(String),
    SelectAll,
    StopSelecting,
    /// Shows the list's checkboxes, or leaves selection mode.
    ToggleSelectMode,
    /// Selects every listed message from the same sender as this one.
    SelectFromSender(String),
    OpenSearch,
    /// Opens the search with this query, as `from:ana@example.com`.
    SearchFor(String),
    CloseSearch,
    SetFilter(Filter),
    OldestFirst(bool),
    FilterMenu,
    Download,
    Connect,
    Settings,
    AccountSettings(String),
    /// Edits an account's nickname: in the sidebar (true) or on its Settings page.
    EditNickname(String, bool),
    RetryAccount(String),
    /// Hides a warning (not a sign-in problem) of an account's.
    DismissIssue(String, IssueSource),
    Reconnect(String),
    SubmitReconnect(String),
    SettingsPage(Page),
    SetAppearance(Appearance),
    SetAccent(Accent),
    SetSync(String, SyncSettings),
    SetReadingDefaults(ReadingOptions),
    /// Removes tracking from links in mail, or stops.
    SetCleanLinks(bool),
    /// Turns Local AI on, or off (deleting its models).
    SetLocalAi(bool),
    /// Hides sign-in codes (downloading the model), or stops (deleting it).
    SetHideCodes(bool),
    /// Reads dates and times typed where Picacho asks when (downloading the
    /// model), or stops (deleting it).
    SetUnderstandDates(bool),
    /// Shows a message's hidden sign-in codes, or hides them again.
    ShowCodes(String, bool),
    /// Copies a message's sign-in code without showing it.
    CopyCode(String),
    /// Asks before deleting a Local AI model; deletes it once confirmed.
    AskDeleteModel(String),
    DeleteModel(String),
    CloseModelSheet,
    ToggleHtml,
    ToggleRemote,
    /// Shows the open email's body in light or dark colors.
    ToggleEmailTheme,
    /// Opens the collapsed reader at a fraction of the message's height.
    Reveal(f32),
    /// Shows the HTML part (true) or the plain text part (false).
    ShowHtml(bool),
    ToggleDetails,
    /// Checks the open message's sender (SPF, DKIM, DMARC) in DNS.
    CheckSender,
    /// Shows or hides the quoted history of the open reply.
    ToggleQuotes,
    /// Lists every message of a long conversation.
    ExpandThread,
    ShowOriginal,
    /// Lists the subscriptions detected in an account, or in every account
    /// (All Accounts' id).
    OpenSubscriptions(String),
    /// Asks a subscription's sender to stop, from the list or an email.
    Unsubscribe(MailSubscription),
    /// Lists a subscription's mail in place of the subscriptions.
    OpenSubscription(MailSubscription),
    /// Back from a subscription's mail to the subscriptions.
    CloseSubscription,
    /// Shows subscribed (false) or unsubscribed (true) senders.
    SubscriptionsTab(bool),
    /// Marks or moves all of a subscription's mail.
    ChangeSubscription(Box<MailSubscription>, Change),
    /// Lists the account's server folders to move mail into.
    MoveMenu(MoveTarget),
    /// Turns asking for read receipts on or off for an account.
    ToggleReadReceipts(String),
    /// Turns asking for delivery receipts on or off for an account.
    ToggleDeliveryReceipts(String),
    /// Turns requiring encryption all the way (REQUIRETLS) on or off.
    ToggleRequireTls(String),
    /// Starts adding a sending address to an account.
    AddIdentity(String),
    /// Edits one of an account's sending addresses: (account, identity).
    EditIdentity(String, String),
    /// Removes one of an account's sending addresses: (account, identity).
    RemoveIdentity(String, String),
    /// Switches the address being edited between the mailbox's outgoing
    /// server and one of its own.
    ToggleIdentityServer,
    SaveIdentity,
    CancelIdentity,
    /// Asks before deleting messages for good.
    DeleteForever(Vec<String>),
    /// Asks before emptying an account's Trash or Spam.
    EmptyMailbox(String, Folder),
    /// Deletes, as confirmed in the dialog.
    ConfirmErase(Erase),
    /// The labels to add to or remove from messages.
    LabelMenu(MoveTarget),
    /// Asks for a new label's name.
    NewLabel(MoveTarget),
    /// Adds the label named in the dialog.
    SaveLabel,
    /// What can be done with all of a subscription's mail.
    SubscriptionMenu(MailSubscription),
    /// Opens the open message's headers, HTML or whole source.
    ShowSource(SourceView),
    CopyOriginal,
    SaveOriginal,
    CopyText(String),
    OpenUrl(String),
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
    RetrySignIn,
    Manual,
    Finish,
    #[cfg(feature = "sample-data")]
    Sample,
    SubmitAccount,
    Remove(String),
    SaveAttachment(usize),
    UnlockMail,
    BackToList,
    LockMail(String),
    OpenDraft(String),
    /// Puts back a message whose sending was delayed for undo.
    UndoSend(u64),
    SetComposeOptions(ComposeOptions),
    /// Lists All Accounts when there are several accounts.
    SetAllMail(bool),
    /// The service picked in Forwarding settings' add form.
    ForwardingService(ForwardingService),
    ConnectForwarding,
    RefreshAliases(String),
    RemoveForwarding(String),
    /// Where a service's aliases forward to; `None` is any mailbox.
    SetDeliverTo(String, Option<String>),
    /// Opens the new alias form under a service, or closes it.
    NewAlias(String),
    /// Opens Forwarding settings at a service's new alias form, for an
    /// alias that forwards to a mailbox: (service, mailbox).
    NewAliasFor(String, String),
    CreateAlias(String),
    /// The new alias form's domain menu, for a service.
    DomainMenu(String),
    /// A domain chosen in the new alias form: its suffix.
    SetAliasDomain(String),
    /// The new alias form's random prefix menu, for a service.
    PrefixMenu(String),
    /// A random prefix: made here, or left to the service to make.
    RandomPrefix(fwd::AliasFormat),
    SetAliasPrefix(String),
    /// Edits an alias's description, or the name recipients see on mail
    /// from it, below its row: (service, alias, which).
    EditAlias(String, String, forwarding::AliasField),
    SaveAliasEdit,
    CancelAliasEdit,
    /// (service, alias, pinned)
    SetAliasPinned(String, String, bool),
    /// The menu of a service's mailboxes (or recipients) for an alias,
    /// and one of them turned on or off for it: (service, alias[, mailbox]).
    AliasMailboxMenu(String, String),
    ToggleAliasMailbox(String, String, String),
    /// SimpleLogin: PGP-encrypt an alias's forwarded mail: (service, alias, on).
    SetAliasPgp(String, String, bool),
    /// Shows or hides a SimpleLogin alias's reverse aliases, and fetches
    /// them again: (service, alias).
    ToggleReverseAliases(String, String),
    LoadReverseAliases(String, String),
    /// A reverse alias's menu, blocking or unblocking its contact, and
    /// deleting it (asked first): (service, alias, contact).
    ContactMenu(String, String, String),
    ToggleContactBlock(String, String, String),
    DeleteContact(String, String, String),
    /// Makes a reverse alias for the address typed in the panel: (service, alias).
    AddContact(String, String),
    /// Shows a page of a service's aliases: (service, page).
    AliasPage(String, usize),
    /// Shows a page of the open reverse aliases.
    ContactPage(usize),
    /// (service account, alias, on)
    SetAliasEnabled(String, String, bool),
    SetAliasDisplay(String, String, AliasDisplay),
    /// The first click asks again; the second deletes.
    DeleteAlias(String, String),
    /// Shows or hides a forwarding service's details in the list.
    ToggleForwarding(String),
    /// Shows or hides the form that connects a forwarding service.
    AddForwarding,
    /// Menus in Forwarding settings: a service's, where it delivers, an alias's.
    ServiceMenu(String),
    DeliverMenu(String),
    /// Keeps the mailboxes a service forwards to from sending as
    /// themselves (service, on).
    SetAliasesOnly(String, bool),
    AliasMenu(String, String),
    /// Leaves a pending Remove or Delete in Forwarding settings.
    CancelForwardingConfirm,
    PlaceSettings(SettingsPlace),
    ClosePresets,
    Dismiss,
    /// Asks when to bring messages back: these, or the selection (empty).
    SnoozeMenu(Vec<String>),
    /// Brings a snoozed message back now: (account, Message-ID).
    Unsnooze(String, String),
    /// Searches the open mailbox on the server for what is typed.
    SearchServer,
    /// Back to searching the mail on this computer.
    ClearServerSearch,
    /// Answers a read receipt request: (message, send it).
    Receipt(String, bool),
    /// Server-side filters and vacation replies in an account's Settings.
    Filters(filters::FilterAction),
}

/// A conversation's rows for one message at one list revision.
type ThreadRows = (String, u64, Rc<Vec<usize>>);

/// How long typing in the sign-in name field pauses before the name applies.
const LOGIN_PAUSE: Duration = Duration::from_millis(1500);

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
    /// The order server folders are shown in, by account, once one was made
    /// or renamed here: like top folders, they stay where they were put.
    /// Folders not in it (made elsewhere) follow, by name.
    folder_order: HashMap<String, Vec<String>>,
    unread: HashMap<(String, String), usize>,
    collapsed: HashSet<String>,
    /// The sidebar's accounts, folders and separators, in order.
    tree: Vec<tree::Node>,
    tree_ui: tree::TreeUi,
    rename_input: Entity<InputState>,
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
    /// Where the collapsed reader's preview was last laid out.
    preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Account pictures saved on this computer.
    avatars: HashMap<String, std::path::PathBuf>,
    menu: Option<PopupMenu>,
    filter_anchor: Rc<Cell<Bounds<Pixels>>>,
    selecting: bool,
    /// Selection mode was turned on from the list's Select button, so the
    /// checkboxes stay while nothing is picked yet.
    select_mode: bool,
    selection: HashSet<String>,
    select_anchor: Option<String>,
    select_anim: u64,
    search_open: bool,
    search_anim: u64,
    filter_by: Filter,
    oldest_first: bool,
    download: Download,
    /// Catalog progress per (account, mailbox), and catalogs running now.
    catalogs: HashMap<(String, String), catalog::Progress>,
    cataloging: HashSet<(String, String)>,
    /// The open mailbox's layout: every message the server has, listed or not.
    layout: Option<crate::manage::backend::Layout>,
    /// Placeholder rows on screen waiting to be listed, and whether a
    /// request for them is running.
    wanted_rows: Vec<u32>,
    filling: bool,
    /// Placeholder rows a fill asked for that the server did not list.
    unfillable: HashSet<u32>,
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
    /// Warnings dismissed in Settings, by account and source, with what
    /// they said: the same failure again stays hidden until that check
    /// succeeds or fails some other way.
    dismissed_issues: HashMap<String, HashMap<IssueSource, String>>,
    sync_requests: HashSet<(String, String)>,
    downloading: HashSet<String>,
    retrying: HashSet<String>,
    /// Unread inbox messages already seen per account, to spot new mail.
    known: HashMap<String, HashSet<String>>,
    busy: usize,
    syncing: bool,
    /// The open mailbox's cached mail is being read, before any sync.
    cache_loading: bool,
    status: String,
    error: bool,
    form: Vec<Entity<InputState>>,
    service: Service,
    provider: Provider,
    oauth: bool,
    connecting: bool,
    /// The last sign-in failed after the browser and can be retried as is.
    sign_in_retry: bool,
    /// How far the running sign-in has come, followed from the backend.
    sign_in_step: SignInStep,
    /// Whether this build offers Proton without Bridge (Direct): it needs
    /// Proton's app token or version. Without it, Proton Mail is Bridge.
    proton_direct: bool,
    /// The Proton sign-in's step after the password, while it has one.
    proton_stage: Option<ProtonStage>,
    /// The way a security key or passkey is being used, while it is.
    key_method: Option<KeyMethod>,
    /// What that key or phone waits for, followed from the backend.
    key_prompt: KeyPrompt,
    cancel: Arc<AtomicBool>,
    onboarding: bool,
    step: Step,
    discovering: bool,
    manual: bool,
    setup_generation: u64,
    notice_generation: u64,
    drafts: Vec<Draft>,
    /// Open message editors: floating, inline replies and windows.
    composers: Vec<Entity<Composer>>,
    compose_options: ComposeOptions,
    /// Messages waiting out their undo delay, or being sent.
    outbox: Vec<compose::Outgoing>,
    outbox_generation: u64,
    /// Inline replies by the message they answer.
    inline_replies: HashMap<String, EntityId>,
    composer_subscriptions: HashMap<EntityId, Subscription>,
    /// The signature field in Composing settings.
    signature_input: Entity<TextareaState>,
    signature_revision: u64,
    /// The nickname field on an account's Settings page, and the account it
    /// shows.
    nickname_input: Entity<InputState>,
    /// The account page's sign-in name and Reply-To fields.
    login_input: Entity<InputState>,
    reply_to_input: Entity<InputState>,
    /// The sending address being added or changed in Settings, if any.
    identity_edit: Option<settings::IdentityEdit>,
    /// Its fields: address, name, Reply-To, server, port, sign-in name and
    /// password.
    identity_inputs: [Entity<InputState>; 7],
    /// A sign-in name being typed and not yet applied: (account, name).
    /// Each change signs the account out, so it applies once typing pauses
    /// or ends, not on every key; see `apply_login`.
    login_pending: Option<(String, String)>,
    /// Counts edits of the sign-in name, so only the latest pause applies.
    login_revision: u64,
    /// Storage use by account, once asked (None: no quota).
    quotas: HashMap<String, Option<crate::sync::transport::Quota>>,
    nickname_for: Option<String>,
    /// The preset picker was opened from Settings, which it returns to.
    /// The account whose preset pictures are shown over Settings.
    presets_for: Option<String>,
    settings_place: SettingsPlace,
    /// Settings' own window while it is popped out.
    settings_window: Option<AnyWindowHandle>,
    /// The main window, to return to from popped-out Settings.
    main_window: Option<AnyWindowHandle>,
    /// A move asked for by a composer whose subscription had no window.
    pending_place: Option<(EntityId, Placement)>,
    /// Any message is being delivered now; quitting waits for it.
    sending: bool,
    /// A spinner tick is pending (see `spinner`).
    spin_scheduled: bool,
    menu_anim: u64,
    /// A count refresh is running, and another was asked for meanwhile.
    counts_running: bool,
    counts_again: bool,
    /// A reload of the listed messages is running, and another was asked
    /// for meanwhile (see `reload_cached`).
    reload_running: bool,
    reload_again: bool,
    /// A check of the listed messages (`reload_if_changed`) was asked for
    /// while a reload ran.
    recheck_again: bool,
    /// Composers' address suggestions, with the list revision and your
    /// addresses they were gathered for; see `contacts`.
    contacts_cache: std::cell::RefCell<Option<(u64, Vec<String>, Rc<Vec<(String, String)>>)>>,
    /// What the listed messages were built from; see `list_freshness`.
    listed: Option<views::ListKey>,
    /// Unread messages among those loaded, counted when the list changes.
    loaded_unread: usize,
    /// Bumped whenever the listed messages change; keys `thread_cache`.
    list_revision: u64,
    /// The open message's conversation: (message, list revision, rows).
    thread_cache: std::cell::RefCell<Option<ThreadRows>>,
    /// The open reply's quoted history is shown, and every message of its
    /// conversation is listed.
    quotes_open: bool,
    thread_open: bool,
    /// The open HTML email without its quoted history, worked out once.
    quote_cache: std::cell::RefCell<Option<(String, Option<Rc<String>>)>>,
    /// The open plain email as Markdown: (message and quote state, text,
    /// whether it has quoted history).
    plain_cache: std::cell::RefCell<Option<(String, SharedString, bool)>>,
    /// The open message's reading options, read from preferences once.
    options_cache: std::cell::RefCell<Option<(String, ReadingOptions)>>,
    _subscriptions: Vec<Subscription>,
    load_generation: u64,
    read_generation: u64,
    anim: u64,
    /// The message whose body is being fetched or decoded now.
    reading: Option<String>,
    /// Bumped only when a different message opens (its entrance animation),
    /// when details open, and when the remote-content switch flips.
    open_anim: u64,
    details_anim: u64,
    /// When the open message and its details appeared. Their entrance
    /// animations run only just after, never again when the reader is
    /// rebuilt (as at the end of a pane animation).
    open_at: Option<std::time::Instant>,
    details_at: Option<std::time::Instant>,
    /// A place in the message to show once the reader opens, as a fraction
    /// of its height (from the collapsed reader's preview).
    reveal: Option<f32>,
    switch_anim: u64,
    switch_at: Option<std::time::Instant>,
    /// Whether the open message's details are expanded, and what was read
    /// from its original source.
    details_open: bool,
    /// The message whose sender is being checked.
    sender_checking: Option<String>,
    /// What the Original Message window shows.
    source_view: SourceView,
    /// The open subscriptions list (in place of the message list), for an
    /// account or All Accounts' id, and what it lists.
    subs_open: bool,
    subs: Vec<MailSubscription>,
    subs_loading: bool,
    subs_generation: u64,
    subs_scroll: UniformListScrollHandle,
    /// Subscribed or Unsubscribed senders.
    subs_tab: subscriptions::SubsTab,
    /// The subscription whose mail is listed, in place of the subscriptions.
    subs_sender: Option<MailSubscription>,
    subs_sender_loading: bool,
    /// Subscriptions whose mail is being marked or moved, by key.
    subs_busy: HashSet<String>,
    /// Server folders being deleted: (account, folder).
    deleting_folders: HashSet<(String, String)>,
    /// Accounts whose drafts are being synced with the server now.
    draft_syncs: HashSet<String>,
    /// Scheduled drafts being sent now (Send Later).
    scheduled_sending: HashSet<String>,
    /// Scheduled drafts that couldn't reach their server, and when to try
    /// them again.
    scheduled_retry: HashMap<String, std::time::Instant>,
    /// The date and time picker of the open dialog (Snooze).
    picker: Option<Entity<DateTimePicker>>,
    picker_subscription: Option<Subscription>,
    /// Each account's snoozed messages.
    snoozed: HashMap<String, Vec<crate::manage::backend::Snoozed>>,
    /// Accounts whose snoozed messages are being brought back now.
    waking: HashSet<String>,
    /// A search of the whole mailbox on the server.
    server_search: Option<later::ServerSearch>,
    /// Server-side filters by account, as loaded in Settings.
    filters: HashMap<String, filters::FiltersUi>,
    filter_inputs: filters::FilterInputs,
    /// What each account's server says about itself (METADATA).
    server_notes: HashMap<String, crate::manage::backend::ServerNotes>,
    /// Watchers (IDLE, NOTIFY) send their account's id and the mailbox
    /// here when the server reports a change.
    pushed: futures::channel::mpsc::UnboundedSender<(String, Mailbox)>,
    /// When each account's drafts were last synced.
    draft_synced: HashMap<String, std::time::Instant>,
    /// The name in the new label dialog.
    label_input: Entity<InputState>,
    /// The server folder being named in the sidebar: (account, edit).
    folder_edit: Option<(String, FolderEdit)>,
    /// A server folder change being made: (account, edit, its new name).
    folder_saving: Option<(String, FolderEdit, String)>,
    /// Where the sender's mail list was scrolled when a message opened over
    /// it, put back when the message closes.
    subs_return: Cell<Option<Point<Pixels>>>,
    /// Subscriptions being, or already, unsubscribed from.
    unsubscribing: HashSet<String>,
    unsubscribed: HashSet<String>,
    details: Option<MessageInfo>,
    original_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    reader_scroll: ScrollHandle,
    /// Renders left that scroll the reader to its end (see `scroll_reader_to_end`).
    reader_to_end: std::cell::Cell<u8>,
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
    /// Alias forwarding services and their aliases.
    forwarding: Vec<ForwardingAccount>,
    /// All Accounts is listed when there are several accounts.
    all_mail: bool,
    /// Links in mail lose their tracking parameters and redirect wrappers.
    clean_links: bool,
    /// Local AI and its sign-in code hiding (Settings → Local AI).
    local_ai: bool,
    hide_codes: bool,
    /// The sign-in code model, loaded while codes are hidden.
    otp: Option<Rc<crate::manage::local_ai::otp::Detector>>,
    /// Codes found per message, with the lengths of the subject and text
    /// they were found in.
    otp_found: std::cell::RefCell<HashMap<String, ai::Found>>,
    /// Messages whose codes were shown on request.
    codes_shown: HashSet<String>,
    /// A model downloading, and why the last download or load failed.
    model_busy: bool,
    model_error: Option<String>,
    /// Bytes of the model downloaded so far.
    model_progress: Arc<std::sync::atomic::AtomicU64>,
    /// The model whose deletion is being confirmed over Settings.
    model_confirm: Option<String>,
    /// Typed dates and times are read (Settings → Local AI), with the date
    /// and time model once loaded; its download, as the sign-in code
    /// model's above.
    understand_dates: bool,
    when: Option<Rc<crate::manage::local_ai::when::Parser>>,
    when_busy: bool,
    when_error: Option<String>,
    when_progress: Arc<std::sync::atomic::AtomicU64>,
    /// Reverse addresses that write as an alias, to show which alias sent
    /// mail went out as (see `fwd::routes`).
    alias_routes: HashMap<String, String>,
    /// The contacts SimpleLogin reverse aliases stand for, shown in their
    /// place on mail of a mailbox kept to its aliases (see
    /// `fwd::reverse_contacts`).
    reverse_contacts: Arc<HashMap<String, String>>,
    /// Aliases whose contacts were fetched this session to name the
    /// reverse aliases on mail being read.
    contacts_fetched: HashSet<String>,
    /// What the open view lists: its mailboxes and address filters.
    scope: fwd::Scope,
    /// Forwarding settings' add form, and the work running there.
    fwd_service: ForwardingService,
    fwd_key: Entity<InputState>,
    fwd_server: Entity<InputState>,
    fwd_note: Entity<InputState>,
    /// Narrows a service's aliases.
    fwd_search: Entity<InputState>,
    fwd_busy: HashSet<String>,
    fwd_error: Option<String>,
    /// An alias waiting for its delete to be confirmed.
    fwd_confirm: Option<String>,
    /// The service whose new alias form is open.
    fwd_creating: Option<String>,
    /// The new alias form: its prefix, the domains each service offers (or
    /// why they could not be listed), the chosen domain's suffix, how a
    /// random prefix is made, and the mailbox the alias forwards to.
    fwd_prefix: Entity<InputState>,
    fwd_options: HashMap<String, Result<fwd::AliasOptions, String>>,
    fwd_domain: Option<String>,
    fwd_format: Option<fwd::AliasFormat>,
    fwd_mailbox: Option<String>,
    /// The alias whose description or display name is being edited:
    /// (service, alias, which), and the field it is edited in.
    fwd_editing: Option<(String, String, forwarding::AliasField)>,
    fwd_description: Entity<InputState>,
    /// A new alias form asked for by a composer whose subscription had no window.
    pending_new_alias: Option<(String, String)>,
    /// When each service's aliases were last fetched.
    fwd_refreshed: HashMap<String, std::time::Instant>,
    /// The alias ("service:alias") whose reverse aliases are shown, and
    /// each alias's as last fetched (or why they could not be), ready to
    /// list. The panel's search and new-contact fields.
    fwd_reverse: Option<String>,
    fwd_contacts: HashMap<String, Result<forwarding::Contacts, String>>,
    fwd_contact_search: Entity<InputState>,
    fwd_contact_new: Entity<InputState>,
    /// The reverse aliases subpage's own scroll, so Forwarding settings'
    /// stays where it was underneath.
    fwd_reverse_scroll: ScrollHandle,
    /// The page of each service's aliases shown, and of the open reverse aliases.
    fwd_pages: HashMap<String, usize>,
    fwd_contact_page: usize,
    /// Forwarding services whose details are open in the list.
    fwd_open: HashSet<String>,
    fwd_adding: bool,
}
fn input(
    placeholder: &str,
    masked: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder.to_owned())
            .masked(masked)
    })
}
/// Sets a field's text from code and reports it as a change, as a person's
/// edit would be. gpui-base's `set_value` stays silent; Picacho's handlers
/// (search filters, account fields) expect to hear about every new value.
pub(crate) trait SetText: Sized {
    fn set_text(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
}
impl<M: InputModeKind> SetText for gpui_base::input::InputBaseState<M> {
    fn set_text(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value: SharedString = value.into();
        self.set_value(value, window, cx);
        cx.emit(InputEvent::Change);
    }
}
/// A multi-line text field.
fn textarea(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<TextareaState> {
    cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder.to_owned()))
}
impl MailApp {
    pub fn new(store: Arc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let accounts = store.accounts().unwrap_or_default();
        let forwarding = fwd::load(&store);
        let all_mail = fwd::all_mail_enabled(&store);
        let alias_routes = fwd::routes(&store);
        let reverse_contacts = Arc::new(fwd::reverse_contacts(&store));
        let clean_links = store.preference("clean_links").as_deref() != Some("false");
        let local_ai = crate::manage::local_ai::enabled(&store);
        let hide_codes = crate::manage::local_ai::otp_enabled(&store);
        let understand_dates = crate::manage::local_ai::when_enabled(&store);
        if clean_links {
            crate::manage::tracking::warm_up();
        }
        // Fonts for the first email opened, found while the window opens.
        #[cfg(not(test))]
        super::html::warm_up();
        let tree = Self::load_tree(&store, &Self::tree_ids(&accounts, &forwarding));
        let account = tree::account_order(&tree)
            .into_iter()
            .find(|id| !fwd::is_view(id));
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
        let search = input("Search, or from: reply-to: is:unread", false, window, cx);
        let placeholders = [
            "you@example.com",
            "Password",
            "imap.example.com",
            "993",
            "smtp.example.com",
            "587",
            "Same as the email address",
            "123456",
            "Mailbox password",
            "PIN",
        ];
        let form: Vec<_> = placeholders
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let masked = matches!(i, PASSWORD | MAILBOX_PASSWORD | KEY_PIN);
                input(p, masked, window, cx)
            })
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
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.search_server(cx);
            }
            if matches!(event, InputEvent::Change) {
                // A new search lists other mail: start from its top rather
                // than keeping the place in the old list.
                this.apply_filter(cx);
                this.scroll_list_to_top();
                cx.notify();
            }
        })]);
        let filter_inputs = filters::FilterInputs::new(window, cx);
        let rename_input = input("Folder name", false, window, cx);
        subscriptions.push(
            cx.subscribe_in(&rename_input, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_rename(w, cx);
                }
            }),
        );
        let label_input = input("Label name", false, window, cx);
        subscriptions.push(
            cx.subscribe_in(&label_input, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.handle(Action::SaveLabel, w, cx);
                }
            }),
        );
        let unlock_input = input("Pinal password or recovery key", true, window, cx);
        subscriptions.push(
            cx.subscribe_in(&unlock_input, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.handle(Action::UnlockMail, w, cx);
                }
            }),
        );
        let encrypted = HashSet::new();
        let fwd_key = input("API key", true, window, cx);
        let fwd_server = input("Server (optional, for self-hosted)", false,
            window,
            cx,
        );
        let fwd_note = input("Description (optional)", false, window, cx);
        let fwd_prefix = input("Random prefix", false, window, cx);
        subscriptions.push(cx.subscribe_in(&fwd_prefix, window, |this, _, event, w, cx| {
            match event {
                InputEvent::PressEnter { .. } => {
                    if let Some(id) = this.fwd_creating.clone() {
                        this.handle(Action::CreateAlias(id), w, cx);
                    }
                }
                InputEvent::Change => cx.notify(),
                _ => {}
            }
        }));
        let fwd_description = input("Description", false, window, cx);
        subscriptions.push(
            cx.subscribe_in(&fwd_description, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.handle(Action::SaveAliasEdit, w, cx);
                }
            }),
        );
        let fwd_contact_search = input("Find a contact", false, window, cx);
        subscriptions.push(cx.subscribe(&fwd_contact_search, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().to_string();
                this.filter_contacts(&query);
                this.fwd_contact_page = 0;
                cx.notify();
            }
        }));
        let fwd_contact_new = input("Write to a new address", false, window, cx);
        subscriptions.push(
            cx.subscribe_in(&fwd_contact_new, window, |this, _, event, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some((id, alias)) = this.fwd_reverse.as_ref().and_then(|k| k.split_once(':'))
                {
                    let action = Action::AddContact(id.to_owned(), alias.to_owned());
                    this.handle(action, w, cx);
                }
            }),
        );
        let fwd_search = input("Find an alias", false, window, cx);
        subscriptions.push(cx.subscribe(&fwd_search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.fwd_pages.clear();
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe_in(&fwd_key, window, |this, _, event, w, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.handle(Action::ConnectForwarding, w, cx);
            }
        }));
        subscriptions.push(cx.subscribe_in(&fwd_note, window, |this, _, event, w, cx| {
            if matches!(event, InputEvent::PressEnter { .. })
                && let Some(id) = this.fwd_creating.clone()
            {
                this.handle(Action::CreateAlias(id), w, cx);
            }
        }));
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
            // Preferences changed just before quitting are still written.
            this.backend.store.flush();
            std::future::ready(())
        }));
        #[cfg(feature = "sample-data")]
        let demo = demo_mail();
        #[cfg(not(feature = "sample-data"))]
        let demo = Vec::new();
        // A saved place is opened once the app is up; the first mailbox's
        // mail isn't read for it meanwhile.
        let restoring = backend.store.preference("last_place").is_some();
        let messages = if restoring {
            vec![]
        } else if let Some(id) = &account {
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
        let folder_order = accounts
            .iter()
            .filter_map(|a| {
                let order = backend.store.preference(&format!("folder_order:{}", a.id))?;
                Some((a.id.clone(), serde_json::from_str(&order).ok()?))
            })
            .collect();
        let collapsed = backend
            .store
            .preference("collapsed_accounts")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let sidebar_collapsed = backend.store.preference("sidebar").as_deref() == Some("collapsed");
        let oldest_first = backend.store.preference("sort").as_deref() == Some("oldest");
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
        let compose_options: ComposeOptions = backend
            .store
            .preference(crate::manage::composing::PREFERENCE)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let signature_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Your name, title, phone…")
                .default_value(compose_options.signature.clone())
        });
        subscriptions.push(cx.subscribe(&signature_input, |this, input, e, cx| {
            if matches!(e, InputEvent::Change) {
                let signature = input.read(cx).value().to_string();
                this.signature_revision += 1;
                let revision = this.signature_revision;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.signature_revision == revision {
                            let options = ComposeOptions {
                                signature,
                                ..this.compose_options.clone()
                            };
                            this.save_compose_options(options, cx);
                        }
                    });
                })
                .detach();
            }
        }));
        let nickname_input = input("Nickname", false, window, cx);
        subscriptions.push(cx.subscribe(&nickname_input, |this, input, e, cx| {
            if matches!(e, InputEvent::Change) {
                if let Some(id) = this.nickname_for.clone() {
                    let nickname = input.read(cx).value().to_string();
                    this.set_nickname(&id, &nickname, cx);
                }
            }
        }));
        let login_input = input("Same as the email address", false, window, cx);
        subscriptions.push(cx.subscribe(&login_input, |this, input, e, cx| {
            // Applied once typed (a pause, Enter or leaving the field), not
            // on every key: each change signs the account out, and its
            // connections would sign in again with half a name, failing
            // until the server throttles or locks the account.
            match e {
                InputEvent::Change => {
                    let Some(id) = this.nickname_for.clone() else {
                        return;
                    };
                    let value = input.read(cx).value().trim().to_owned();
                    let saved = this.accounts.iter().find(|a| a.id == id);
                    if saved.is_none_or(|a| a.username == value) {
                        this.login_pending = None;
                        return;
                    }
                    this.login_pending = Some((id, value));
                    this.login_revision += 1;
                    let revision = this.login_revision;
                    cx.spawn(async move |this, cx| {
                        // The executor's timer, which tests can move on.
                        cx.background_executor().timer(LOGIN_PAUSE).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.login_revision == revision {
                                this.apply_login(cx);
                            }
                        });
                    })
                    .detach();
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.apply_login(cx),
                _ => {}
            }
        }));
        let reply_to_input = input("Replies come to the sending address", false, window, cx);
        subscriptions.push(cx.subscribe(&reply_to_input, |this, input, e, cx| {
            if matches!(e, InputEvent::Change)
                && let Some(id) = this.nickname_for.clone()
            {
                let value = input.read(cx).value().trim().to_owned();
                if this.accounts.iter().any(|a| a.id == id && a.reply_to != value) {
                    this.edit_account(&id, |a| a.reply_to = value, cx);
                }
            }
        }));
        let identity_inputs = [
            input("name@example.com", false, window, cx),
            input("Your name, as recipients see it", false, window, cx),
            input("Replies come to this address", false, window, cx),
            input("smtp.example.com", false, window, cx),
            input("587", false, window, cx),
            input("Same as the address", false, window, cx),
            input("Password or app password", true, window, cx),
        ];
        let html_view = cx.new(HtmlView::new);
        subscriptions.push(
            cx.subscribe(&html_view, |this, _, menu: &HtmlContextMenu, cx| {
                this.html_menu(menu, cx)
            }),
        );
        let (pushed, mut pushes) = futures::channel::mpsc::unbounded::<(String, Mailbox)>();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some((id, mailbox)) = pushes.next().await {
                if this.update(cx, |this, cx| this.on_push(id, mailbox, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
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
            folder_order,
            unread: HashMap::new(),
            collapsed,
            tree,
            tree_ui: tree::TreeUi::default(),
            rename_input,
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
            avatars: HashMap::new(),
            menu: None,
            filter_anchor: Rc::new(Cell::new(Bounds::default())),
            selecting: false,
            select_mode: false,
            selection: HashSet::new(),
            select_anchor: None,
            select_anim: 0,
            search_open: false,
            search_anim: 0,
            filter_by: Filter::All,
            oldest_first,
            download: Download::default(),
            catalogs: HashMap::new(),
            cataloging: HashSet::new(),
            layout: None,
            wanted_rows: Vec::new(),
            filling: false,
            unfillable: HashSet::new(),
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
            dismissed_issues: HashMap::new(),
            sync_requests: HashSet::new(),
            downloading: HashSet::new(),
            retrying: HashSet::new(),
            known: HashMap::new(),
            busy: 0,
            syncing: false,
            cache_loading: false,
            status: String::new(),
            error: false,
            form,
            service: Service::Other,
            provider: Provider::Gmail,
            oauth: false,
            connecting: false,
            sign_in_retry: false,
            sign_in_step: SignInStep::Browser,
            proton_direct: crate::sync::proton::available(),
            proton_stage: None,
            key_method: None,
            key_prompt: KeyPrompt::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            onboarding,
            step: Step::Welcome,
            discovering: false,
            manual: false,
            setup_generation: 0,
            notice_generation: 0,
            drafts: vec![],
            composers: vec![],
            compose_options,
            outbox: vec![],
            outbox_generation: 0,
            inline_replies: HashMap::new(),
            composer_subscriptions: HashMap::new(),
            signature_input,
            signature_revision: 0,
            nickname_input,
            login_input,
            reply_to_input,
            identity_edit: None,
            identity_inputs,
            login_pending: None,
            login_revision: 0,
            quotas: HashMap::new(),
            nickname_for: None,
            presets_for: None,
            settings_place: SettingsPlace::Dialog,
            settings_window: None,
            main_window: Some(window.window_handle()),
            pending_place: None,
            sending: false,
            spin_scheduled: false,
            menu_anim: 0,
            counts_running: false,
            counts_again: false,
            reload_running: false,
            reload_again: false,
            recheck_again: false,
            contacts_cache: Default::default(),
            listed: None,
            loaded_unread: 0,
            options_cache: Default::default(),
            list_revision: 0,
            thread_cache: Default::default(),
            quotes_open: false,
            thread_open: false,
            quote_cache: Default::default(),
            plain_cache: Default::default(),
            _subscriptions: subscriptions,
            load_generation: 0,
            read_generation: 0,
            anim: 0,
            reading: None,
            open_anim: 0,
            details_anim: 0,
            open_at: None,
            details_at: None,
            reveal: None,
            preview_bounds: Rc::new(Cell::new(Bounds::default())),
            switch_anim: 0,
            switch_at: None,
            details_open: false,
            sender_checking: None,
            source_view: SourceView::Original,
            subs_open: false,
            subs: Vec::new(),
            subs_loading: false,
            subs_generation: 0,
            subs_scroll: UniformListScrollHandle::new(),
            subs_tab: subscriptions::SubsTab::Subscribed,
            subs_sender: None,
            subs_sender_loading: false,
            subs_busy: HashSet::new(),
            deleting_folders: HashSet::new(),
            draft_syncs: HashSet::new(),
            scheduled_sending: HashSet::new(),
            scheduled_retry: HashMap::new(),
            pushed,
            draft_synced: HashMap::new(),
            label_input,
            folder_edit: None,
            folder_saving: None,
            subs_return: Cell::new(None),
            unsubscribing: HashSet::new(),
            unsubscribed: Default::default(),
            details: None,
            original_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            reader_scroll: ScrollHandle::new(),
            reader_to_end: Default::default(),
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
            forwarding,
            all_mail,
            alias_routes,
            reverse_contacts,
            contacts_fetched: HashSet::new(),
            clean_links,
            local_ai,
            hide_codes,
            otp: None,
            otp_found: Default::default(),
            codes_shown: HashSet::new(),
            model_busy: false,
            model_error: None,
            model_progress: Arc::default(),
            model_confirm: None,
            understand_dates,
            when: None,
            when_busy: false,
            when_error: None,
            when_progress: Arc::default(),
            scope: fwd::Scope::default(),
            fwd_service: ForwardingService::default(),
            fwd_key,
            fwd_server,
            fwd_note,
            fwd_search,
            fwd_busy: HashSet::new(),
            fwd_error: None,
            fwd_confirm: None,
            fwd_creating: None,
            fwd_prefix,
            fwd_options: HashMap::new(),
            fwd_domain: None,
            fwd_format: None,
            fwd_mailbox: None,
            fwd_editing: None,
            fwd_description,
            pending_new_alias: None,
            fwd_refreshed: HashMap::new(),
            fwd_reverse: None,
            fwd_contacts: HashMap::new(),
            fwd_contact_search,
            fwd_contact_new,
            fwd_reverse_scroll: ScrollHandle::new(),
            fwd_pages: HashMap::new(),
            fwd_contact_page: 0,
            fwd_open: HashSet::new(),
            fwd_adding: false,
            picker: None,
            picker_subscription: None,
            snoozed: HashMap::new(),
            waking: HashSet::new(),
            server_search: None,
            filters: HashMap::new(),
            filter_inputs,
            server_notes: HashMap::new(),
        };
        this.update_scope();
        this.load_snoozed();
        this.load_unsubscribed();
        this.backfill_subscriptions(cx);
        this.refresh_previews(cx);
        this.pack_saved_sources(cx);
        this.filter(cx);
        this.apply_theme(window, cx);
        #[cfg(all(debug_assertions, target_os = "macos"))]
        devdriver::start(window, cx);
        this.watch_inboxes();
        if this.account.is_some() {
            cx.defer_in(window, |this, w, cx| {
                this.refresh_counts(cx);
                // Back where the person was, else the first mailbox.
                if this.restore_place(w, cx) {
                } else if this.messages.is_empty() {
                    let first = this.account.clone();
                    this.open_mailbox(first, Folder::Inbox.into(), w, cx);
                } else {
                    this.refresh(cx);
                }
            });
        }
        for a in this.accounts.clone() {
            this.load_avatar(a, cx);
        }
        this.start_local_ai(cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(20)).await;
                let checked = this.update(cx, |this, cx| {
                    this.watch_inboxes();
                    this.check_due(cx);
                    this.send_due(cx);
                    this.wake_snoozed(cx);
                    this.refresh_aliases_due(views::ALIAS_REFRESH, cx);
                });
                if checked.is_err() {
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
    /// The open mailbox's account. In All Accounts or an alias's view, the
    /// first mailbox it draws from, which new messages are sent from.
    fn current_account(&self) -> Option<Account> {
        let id = self.account.as_ref()?;
        let id = if fwd::is_view(id) {
            self.scope.accounts.first()?
        } else {
            id
        };
        self.accounts.iter().find(|a| &a.id == id).cloned()
    }
    fn selected_mail(&self) -> Option<Mail> {
        self.selected_ref().cloned()
    }
    /// The open message, without copying its body.
    fn selected_ref(&self) -> Option<&Mail> {
        let id = self.selected.as_ref()?;
        self.messages.iter().find(|m| &m.id == id)
    }
    fn value<M: InputModeKind>(
        state: &Entity<gpui_base::input::InputBaseState<M>>,
        cx: &App,
    ) -> String {
        state.read(cx).value().to_string()
    }
    fn set<M: InputModeKind>(
        state: &Entity<gpui_base::input::InputBaseState<M>>,
        value: impl Into<SharedString>,
        w: &mut Window,
        cx: &mut App,
    ) {
        let value: SharedString = value.into();
        state.update(cx, |s, cx| s.set_text(value, w, cx));
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
    /// Scrolls the message list to its first row now.
    fn scroll_list_to_top(&self) {
        self.list_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(0.)));
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }
    fn apply_filter(&mut self, cx: &App) {
        let query = Self::value(&self.search, cx).trim().to_lowercase();
        let search = crate::manage::search::Search::parse(&query);
        let server = self
            .current_server_search(Self::value(&self.search, cx).trim())
            .filter(|s| !s.running && s.error.is_none())
            .map(|s| s.ids.clone());
        let filter = self.filter_by;
        let open = self.selected.clone();
        let (subs_open, scope) = (self.subs_open, &self.scope);
        let keep = |m: &Mail| {
            // Placeholders have nothing to match yet; they show only in the
            // whole, unsearched list, where they hold their messages' places.
            (!m.pending || (filter == Filter::All && query.is_empty()))
                && match filter {
                    Filter::All => true,
                    // Opening a message reads it; it stays listed while it is open.
                    Filter::Unread => m.unread || open.as_ref() == Some(&m.id),
                    Filter::Starred => m.starred,
                    Filter::Attachments => m.has_attachments(),
                }
                && match &server {
                    // Found by the server's search, in its own syntax.
                    Some(ids) => ids.contains(&m.id),
                    None => search.matches(m),
                }
                // Views leave out mail of aliases shown on their own, or list
                // only an alias's mail.
                // A subscription's mail is listed whole, as its counts are.
                && (m.pending || subs_open || scope.admits(m))
        };
        // Matching text is what costs: spread it over the cores.
        self.visible = matching_rows(&self.messages, !query.is_empty(), keep);
        if self.oldest_first {
            self.visible.reverse();
        }
        if !self.selection.is_empty() {
            let present: HashSet<&String> = self.messages.iter().map(|m| &m.id).collect();
            self.selection.retain(|id| present.contains(id));
        }
        self.loaded_unread = self
            .messages
            .iter()
            .filter(|m| m.unread && self.scope.admits(m))
            .count();
        self.list_revision += 1;
        self.sync_selecting();
    }
    /// Reloads sidebar unread counts and download progress from the cache.
    /// Requests made while one runs are coalesced into a single follow-up,
    /// spaced out so a running download does not query on every batch.
    fn refresh_counts(&mut self, cx: &mut Context<Self>) {
        if self.counts_running {
            self.counts_again = true;
            return;
        }
        self.counts_running = true;
        let store = self.backend.store.clone();
        let accounts = self.accounts.clone();
        let forwarding = self.forwarding.clone();
        let all_mail = self.shows_all_mail();
        let now = chrono::Utc::now().timestamp();
        let offline: Vec<(String, i64)> = self
            .accounts
            .iter()
            .filter(|a| a.provider != Provider::Pop3)
            .filter_map(|a| Some((a.id.clone(), a.sync.offline_since(now)?)))
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let raw = store.unread_counts()?;
                    // Recipients are only read when an alias has a view of its own.
                    let rows = if fwd::separate_views(&forwarding).is_empty() {
                        Vec::new()
                    } else {
                        store.unread_recipients()?
                    };
                    Ok::<_, anyhow::Error>((
                        fwd::view_counts(raw, &rows, &accounts, &forwarding, all_mail),
                        store.download_counts(crate::sync::transport::MAX_MESSAGE, &offline)?,
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok((unread, (total, missing))) = result {
                    let paused = missing > 0 && !this.download.active;
                    if this.unread != unread
                        || this.download.total != total
                        || this.download.missing != missing
                        || this.download.paused != paused
                    {
                        this.unread = unread;
                        this.download.total = total;
                        this.download.missing = missing;
                        this.download.paused = paused;
                        cx.notify();
                    }
                }
            });
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let _ = this.update(cx, |this, cx| {
                this.counts_running = false;
                if std::mem::take(&mut this.counts_again) {
                    this.refresh_counts(cx);
                }
            });
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
                // The server is limiting this account; wait it out.
                || self.backend.throttled_until(&a.id).is_some()
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
    /// Keeps an IDLE watch on each IMAP account's inbox, so new mail shows
    /// at once rather than at the next check; ends watches of accounts that
    /// are gone or can't sign in. Polling carries on regardless.
    fn watch_inboxes(&mut self) {
        let watching = self.backend.watching();
        for id in &watching {
            let keep = self.accounts.iter().any(|a| &a.id == id) && !self.account_blocked(id);
            if !keep {
                self.backend.stop_watching(id);
            }
        }
        for a in &self.accounts {
            if a.provider == Provider::Pop3
                || a.incoming_host.is_empty()
                || watching.contains(&a.id)
                || self.account_blocked(&a.id)
            {
                continue;
            }
            let (pushed, id) = (self.pushed.clone(), a.id.clone());
            self.backend.watch_inbox(a, move |mailbox| {
                drop(pushed.unbounded_send((id.clone(), mailbox)))
            });
        }
        self.watch_open_folder();
    }
    /// Pushes the mailbox open in the window too (IDLE on a connection of
    /// its own), unless its account's inbox watcher already hears about
    /// every mailbox (NOTIFY). Other accounts' folder watches end.
    fn watch_open_folder(&mut self) {
        let open = self
            .account
            .clone()
            .filter(|id| !fwd::is_view(id))
            .filter(|_| !self.folder.is_virtual() && self.folder != Folder::Drafts);
        for a in self.accounts.iter().filter(|a| !a.incoming_host.is_empty()) {
            let mailbox = (open.as_ref() == Some(&a.id) && !self.account_blocked(&a.id))
                .then(|| self.mailbox());
            let (pushed, id) = (self.pushed.clone(), a.id.clone());
            self.backend.watch_folder(a, mailbox, move |mailbox| {
                drop(pushed.unbounded_send((id.clone(), mailbox)))
            });
        }
    }
    /// The server reported a change in one of an account's mailboxes (IDLE,
    /// NOTIFY): check it now, as the regular check would.
    fn on_push(&mut self, id: String, mailbox: Mailbox, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if self.account_blocked(&id) {
            return;
        }
        let open = self.shows(&id) && self.mailbox() == mailbox;
        if open && self.account.as_ref() == Some(&id) {
            self.refresh(cx);
        } else if mailbox == Mailbox::Standard(Folder::Inbox) {
            self.check_inbox(a, cx);
        } else if mailbox.folder() != Folder::Drafts {
            // Other mailboxes keep their counts current.
            self.sync_mailbox(a, mailbox, 100, cx);
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
            // A sign-in code in the subject isn't shown on the lock screen.
            let subject = self.hide_codes(latest, &latest.subject);
            crate::display::notify::new_mail(
                &a.email,
                (&latest.sender, &subject),
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
        if self.account_blocked(&a.id)
            || self.backend.throttled_until(&a.id).is_some()
            || !self.downloading.insert(a.id.clone())
        {
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
                        // Proton's subscriptions are found as its mail
                        // downloads (its list doesn't say): show them as
                        // they are, when its subscriptions are open.
                        if a.provider == Provider::Proton
                            && matches!(result, Ok(n) if n > 0)
                            && this.subs_open
                            && this.subs_sender.is_none()
                            && this.account.as_ref().is_some_and(|id| id == &a.id || id == ALL_MAIL)
                        {
                            this.load_subscriptions(cx);
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
                cx.background_executor().timer(Duration::from_millis(25)).await;
            }
        })
        .detach();
        cx.notify();
    }
    /// Saves a preference without waiting for the cache. If it can't be
    /// saved, says why and calls `undo` to put back what it changed.
    pub(super) fn save_preference(
        &mut self,
        key: &str,
        value: &str,
        cx: &mut Context<Self>,
        undo: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let store = self.backend.store.clone();
        let ticket = store.set_preference_soon(key, value);
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { store.preference_saved(&key, ticket) })
                .await;
            if let Err(e) = saved {
                let _ = this.update(cx, |this, cx| {
                    this.notice(e.to_string(), true, cx);
                    undo(this, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn save_collapsed(&self) {
        if let Ok(json) = serde_json::to_string(&self.collapsed) {
            self
                .backend
                .store.set_preference_soon("collapsed_accounts", &json);
        }
    }
    fn open_menu(&mut self, position: Point<Pixels>, entries: Vec<Entry>, cx: &mut Context<Self>) {
        // Its own counter: bumping `anim` would replay an open dialog's
        // entrance, as if Settings reloaded under the menu.
        self.menu_anim += 1;
        self.menu = Some(PopupMenu {
            position,
            entries,
            key: self.menu_anim,
            opened: std::time::Instant::now(),
            window: None,
        });
        cx.notify();
    }
    /// Opens a menu at the pointer in the window it was asked from: the
    /// main window, or popped-out Settings.
    fn open_menu_here(&mut self, entries: Vec<Entry>, w: &Window, cx: &mut Context<Self>) {
        self.open_menu(w.mouse_position(), entries, cx);
        let here = w.window_handle().window_id();
        if self.settings_window.is_some_and(|h| h.window_id() == here)
            && let Some(menu) = &mut self.menu
        {
            menu.window = Some(here);
        }
    }
    /// An account's menu. `inline` when it was opened from the expanded
    /// sidebar, which can edit the nickname in place.
    fn account_entries(&self, id: &str, inline: bool) -> Vec<Entry> {
        let id = id.to_owned();
        let has_nickname = self
            .accounts
            .iter()
            .any(|a| a.id == id && !a.nickname.is_empty());
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
            entry(
                if has_nickname {
                    "Edit Nickname…"
                } else {
                    "Add Nickname…"
                },
                "compose",
                Action::EditNickname(id.clone(), inline),
            ),
            entry("Change Picture…", "user", Action::AvatarMenu(id.clone())),
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
                Action::MarkAllRead(id.into(), mailbox.clone()),
            ));
        }
        if let Mailbox::Standard(folder @ (Folder::Trash | Folder::Junk)) = mailbox
            && self.accounts.iter().any(|a| a.id == id)
        {
            entries.push(Entry::Separator);
            entries.push(danger(
                if folder == Folder::Trash {
                    "Empty Trash…"
                } else {
                    "Empty Spam…"
                },
                "trash",
                Action::EmptyMailbox(id.into(), folder),
            ));
        }
        // The same entries, in the same order, as top folders have.
        if mailbox == Mailbox::Standard(Folder::Inbox) && self.manages_folders(id) {
            entries.push(Entry::Separator);
            entries.push(entry(
                "New Folder",
                "folder-closed",
                Action::NewRemoteFolder(id.into(), None),
            ));
        }
        if let Mailbox::Remote(name) = mailbox
            && self.folder_deletable(id, &name)
        {
            entries.push(Entry::Separator);
            entries.push(entry(
                "Rename…",
                "compose",
                Action::RenameRemoteFolder(id.into(), name.clone()),
            ));
            if self.folder_nestable(id, &name) {
                entries.push(Entry::Separator);
                entries.push(entry(
                    "New Folder Inside",
                    "folder-closed",
                    Action::NewRemoteFolder(id.into(), Some(name.clone())),
                ));
            }
            entries.push(Entry::Separator);
            entries.push(danger(
                "Delete Folder…",
                "trash",
                Action::DeleteRemoteFolder(id.into(), name),
            ));
        }
        entries
    }
    /// Whether the account keeps folders on a server that can make,
    /// rename and delete them.
    fn manages_folders(&self, id: &str) -> bool {
        self.accounts
            .iter()
            .any(|a| a.id == id && Backend::can_manage_folders(a))
    }
    /// Whether the server allows folders inside one of the account's own.
    fn folder_nestable(&self, id: &str, name: &str) -> bool {
        self.folder_deletable(id, name)
            && self
                .folders
                .get(id)
                .is_some_and(|fs| fs.iter().any(|f| f.name == name && f.nestable))
    }
    /// Whether a server folder can be offered for renaming or deleting:
    /// one of the account's own, on a server that keeps folders. The
    /// server has the last word (subfolders, access rights).
    fn folder_deletable(&self, id: &str, name: &str) -> bool {
        let Some(a) = self.accounts.iter().find(|a| a.id == id) else {
            return false;
        };
        Backend::can_manage_folders(a)
            && !(a.provider == Provider::Gmail && name.starts_with("[Gmail]"))
            && self
                .folders
                .get(id)
                .is_some_and(|fs| fs.iter().any(|f| f.name == name))
    }
    /// A folder's name as the sidebar shows it.
    fn folder_label(&self, id: &str, name: &str) -> String {
        self.folders
            .get(id)
            .and_then(|fs| fs.iter().find(|f| f.name == name))
            .map_or_else(|| name.to_owned(), |f| f.label.clone())
    }
    /// The Delete Folder dialog: what deleting does on this account's server.
    fn delete_folder_modal(&self, id: &str, name: &str) -> AnyElement {
        let c = self.colors();
        let label = self.folder_label(id, name);
        let (email, gmail) = self
            .accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| (a.email.clone(), a.provider == Provider::Gmail))
            .unwrap_or_default();
        div()
            .id("delete-folder")
            .debug_selector(|| "delete-folder".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("Delete “{label}”?")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if gmail {
                        format!("The label is removed from {email}. Its messages stay in All Mail.")
                    } else {
                        format!(
                            "The folder and the messages in it are deleted from {email}'s server. This can't be undone."
                        )
                    }),
            )
            .into_any_element()
    }
    /// Deletes a folder on the account's server, as confirmed in the dialog,
    /// and forgets its cached mail. Its view closes if it was open.
    fn delete_remote_folder(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        if !self.folder_deletable(&id, &name) || !self.deleting_folders.insert((id.clone(), name.clone())) {
            return;
        }
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            self.deleting_folders.remove(&(id, name));
            return;
        };
        let label = self.folder_label(&id, &name);
        let deleting = name.clone();
        self.task(
            cx,
            move |b| b.delete_remote_folder(&a, &deleting),
            move |this, result, cx| {
                this.deleting_folders.remove(&(id.clone(), name.clone()));
                // The dialog stays up, showing progress, until it is done.
                if this.modal == Some(Modal::DeleteFolder(id.clone(), name.clone())) {
                    this.modal = None;
                }
                match result {
                    Ok(folders) => {
                        this.folders.insert(id.clone(), folders);
                        this.notice(format!("Deleted “{label}”"), false, cx);
                        // Its view is gone; show the account's inbox.
                        if this.account.as_ref() == Some(&id)
                            && this.mailbox() == Mailbox::Remote(name.clone())
                        {
                            this.show_mailbox_instead(Folder::Inbox.into(), cx);
                        }
                        this.refresh_counts(cx);
                    }
                    Err(e) => this.notice(format!("Couldn't delete “{label}”: {e:#}"), true, cx),
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    /// Shows another of the open account's mailboxes in place of one that
    /// was deleted or renamed on the server.
    fn show_mailbox_instead(&mut self, mailbox: Mailbox, cx: &mut Context<Self>) {
        self.load_generation += 1;
        self.subs_open = false;
        self.folder = mailbox.folder();
        self.remote = match mailbox {
            Mailbox::Remote(name) => name,
            Mailbox::Standard(_) => String::new(),
        };
        self.update_scope();
        self.selected = None;
        self.messages.clear();
        self.visible.clear();
        self.layout = None;
        self.filter(cx);
        self.reload_cached(cx);
        self.refresh(cx);
        self.save_place();
    }
    /// The sidebar row key of a server folder being named.
    fn folder_edit_key(account: &str, edit: &FolderEdit) -> String {
        match edit {
            FolderEdit::New(parent) => {
                format!("new-folder\u{1f}{account}\u{1f}{}", parent.as_deref().unwrap_or(""))
            }
            FolderEdit::Rename(name) => format!("rename-folder\u{1f}{account}\u{1f}{name}"),
        }
    }
    /// Whether this row of the sidebar is being named now.
    fn editing_folder(&self, account: &str, edit: &FolderEdit) -> bool {
        self.tree_ui.renaming.as_deref() == Some(Self::folder_edit_key(account, edit).as_str())
    }
    /// A name for a new folder that its neighbours don't have yet: "New
    /// Folder", else "New Folder 2"…
    fn new_folder_name(&self, account: &str, parent: Option<&str>) -> String {
        let depth = parent.map_or(0, |p| {
            self.folders
                .get(account)
                .and_then(|fs| fs.iter().find(|f| f.name == p))
                .map_or(0, |f| f.depth + 1)
        });
        let taken = |label: &str| {
            self.folders.get(account).is_some_and(|fs| {
                fs.iter()
                    .any(|f| f.depth == depth && f.label.eq_ignore_ascii_case(label))
            })
        };
        (1..)
            .map(|n| if n == 1 { "New Folder".to_owned() } else { format!("New Folder {n}") })
            .find(|name| !taken(name))
            .unwrap_or_default()
    }
    /// Names a new server folder, or renames one, in place in the sidebar,
    /// just as top folders are: a new one starts as "New Folder" with its
    /// name selected; Enter or clicking away saves, Escape keeps the name.
    fn edit_remote_folder(
        &mut self,
        id: String,
        edit: FolderEdit,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let allowed = match &edit {
            FolderEdit::New(None) => self.manages_folders(&id),
            FolderEdit::New(Some(parent)) => self.folder_nestable(&id, parent),
            FolderEdit::Rename(name) => self.folder_deletable(&id, name),
        };
        if !allowed || self.folder_saving.is_some() {
            return;
        }
        // Finish any other name being edited first.
        if self.tree_ui.renaming.is_some() {
            self.commit_rename(w, cx);
        }
        // The row is in the full sidebar, inside the account's open folders.
        if self.sidebar_collapsed {
            self.set_sidebar_collapsed(false, cx);
        }
        for key in [id.clone(), format!("folders:{id}")] {
            if self.collapsed.remove(&key) {
                self.save_collapsed();
            }
        }
        let value = match &edit {
            FolderEdit::Rename(name) => self.folder_label(&id, name),
            FolderEdit::New(parent) => self.new_folder_name(&id, parent.as_deref()),
        };
        self.tree_ui.renaming = Some(Self::folder_edit_key(&id, &edit));
        self.folder_edit = Some((id, edit));
        self.rename_input.update(cx, |s, cx| {
            s.set_placeholder("Folder name", w, cx);
            s.set_text(value, w, cx);
            s.focus(w, cx);
        });
        // Select the whole name once the field has focus, so typing replaces it.
        w.on_next_frame(|w, cx| w.dispatch_action(Box::new(gpui_component::input::SelectAll), cx));
        cx.notify();
    }
    /// Ends naming a server folder: saves the name typed, or (`cancel`, as
    /// Escape does for top folders) keeps the old name; a new folder is
    /// still made, with its starting name.
    pub(super) fn finish_remote_folder(&mut self, cancel: bool, cx: &mut Context<Self>) -> bool {
        let Some((id, edit)) = self.folder_edit.take() else {
            return false;
        };
        let typed = Self::value(&self.rename_input, cx).trim().to_owned();
        let leaf = match &edit {
            FolderEdit::New(parent) if cancel || typed.is_empty() => {
                self.new_folder_name(&id, parent.as_deref())
            }
            FolderEdit::Rename(_) if cancel || typed.is_empty() => return true,
            FolderEdit::Rename(name) if typed == self.folder_label(&id, name) => return true,
            _ => typed,
        };
        self.save_remote_folder(id, edit, leaf, cx);
        true
    }
    /// Makes or renames a server folder. Until the server answers, the
    /// sidebar shows it by its new name; a refusal is said in a notice.
    fn save_remote_folder(&mut self, id: String, edit: FolderEdit, leaf: String, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        self.folder_saving = Some((id.clone(), edit.clone(), leaf.clone()));
        let job = edit.clone();
        self.task(
            cx,
            move |b| match &job {
                FolderEdit::New(parent) => b.create_remote_folder(&a, parent.as_deref(), &leaf),
                FolderEdit::Rename(name) => b.rename_remote_folder(&a, name, &leaf),
            },
            move |this, result, cx| {
                if this
                    .folder_saving
                    .as_ref()
                    .is_some_and(|(a, e, _)| *a == id && *e == edit)
                {
                    this.folder_saving = None;
                }
                match result {
                    Ok(change) => {
                        // The folder stays where it was shown while it was
                        // named, rather than moving to where its name sorts.
                        this.place_folders(&id, &edit, &change);
                        this.folders.insert(id.clone(), change.folders.clone());
                        if let FolderEdit::Rename(_) = &edit {
                            // An open folder that moved is shown by its new name.
                            let showing = this.mailbox();
                            let moved = change
                                .moved
                                .iter()
                                .find(|(old, _)| showing == Mailbox::Remote(old.clone()));
                            if this.account.as_ref() == Some(&id)
                                && let Some((_, new)) = moved
                            {
                                this.show_mailbox_instead(Mailbox::Remote(new.clone()), cx);
                            }
                        }
                        this.refresh_counts(cx);
                    }
                    Err(e) => {
                        let what = match &edit {
                            FolderEdit::New(_) => "create the folder",
                            FolderEdit::Rename(_) => "rename the folder",
                        };
                        this.notice(format!("Couldn't {what}: {e:#}"), true, cx);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    fn mail_entries(&self, id: &str) -> Vec<Entry> {
        if self.folder == Folder::Drafts {
            return vec![
                entry("Open Draft", "compose", Action::Select(id.into())),
                danger("Delete Draft", "trash", Action::DeleteDraft(id.into())),
            ];
        }
        if self.selection.len() > 1 && self.selection.contains(id) {
            let n = fmt_count(self.selection.len());
            let all_starred = self.selection_all(|m| m.starred);
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
                if all_starred {
                    entry(
                        format!("Unstar {n}"),
                        "star",
                        Action::ApplySelection(Change::Star(false)),
                    )
                } else {
                    entry(
                        format!("Star {n}"),
                        "star",
                        Action::ApplySelection(Change::Star(true)),
                    )
                },
                Entry::Separator,
                entry(
                    format!("Move {n} to Inbox"),
                    "inbox",
                    Action::ApplySelection(Change::Move(Folder::Inbox)),
                ),
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
                entry(format!("Snooze {n}…"), "snooze", Action::SnoozeMenu(vec![])),
            ]
            .into_iter()
            .chain(self.move_folder_entry(format!("Move {n} to Folder…"), MoveTarget::Selection))
            .chain(self.label_entry(format!("Label {n}…"), MoveTarget::Selection))
            .chain(
                self.selection_all(|m| matches!(m.folder, Folder::Trash | Folder::Junk))
                    .then(|| {
                        danger(
                            format!("Delete {n} Permanently…"),
                            "trash",
                            Action::DeleteForever(self.selection.iter().cloned().collect()),
                        )
                    }),
            )
            .chain([
                Entry::Separator,
                entry("Select All", "select", Action::SelectAll),
                entry("Clear Selection", "close", Action::StopSelecting),
            ])
            .collect();
        }
        let Some(m) = self.messages.iter().find(|m| m.id == id) else {
            return vec![];
        };
        let ids = vec![m.id.clone()];
        let mut entries = vec![
            entry(
                "Open in New Window",
                "popout",
                Action::OpenWindow(m.id.clone()),
            ),
            Entry::Separator,
            entry("Reply", "reply", Action::ReplyTo(m.id.clone(), 1)),
            entry("Reply All", "reply-all", Action::ReplyTo(m.id.clone(), 2)),
            entry("Forward", "forward", Action::ReplyTo(m.id.clone(), 3)),
            entry(
                "Forward as Attachment",
                "paperclip",
                Action::ReplyTo(m.id.clone(), 4),
            ),
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
                Action::Apply(ids.clone(), Change::Move(Folder::Trash)),
            ));
        }
        if self.account_of(m).is_some_and(|a| a.provider != Provider::Pop3) {
            match self.snoozed_until(m) {
                Some(_) => entries.push(entry(
                    "Unsnooze",
                    "inbox",
                    Action::Unsnooze(m.account.clone(), m.message_id.clone()),
                )),
                None if !matches!(m.folder, Folder::Drafts | Folder::Sent) => {
                    entries.push(entry("Snooze…", "snooze", Action::SnoozeMenu(ids.clone())))
                }
                None => {}
            }
        }
        entries.extend(self.move_folder_entry("Move to Folder…", MoveTarget::Mail(ids.clone())));
        entries.extend(self.label_entry("Labels…", MoveTarget::Mail(ids.clone())));
        if matches!(m.folder, Folder::Trash | Folder::Junk) {
            entries.push(danger(
                "Delete Permanently…",
                "trash",
                Action::DeleteForever(ids),
            ));
        }
        entries.push(Entry::Separator);
        entries.push(entry(
            "Select",
            "select",
            Action::StartSelecting(m.id.clone()),
        ));
        if !m.from.is_empty() {
            entries.push(entry(
                "Select All From Sender",
                "select",
                Action::SelectFromSender(m.id.clone()),
            ));
            entries.push(Entry::Separator);
            entries.push(entry(
                "Show Mail From Sender",
                "search",
                Action::SearchFor(search_term("from", &m.from)),
            ));
        }
        // Lists and alias services send from one address and take replies
        // at another: find everything that replies there.
        let reply_to = crate::sync::smtp::recipients(&m.reply_to)
            .ok()
            .and_then(|list| list.into_iter().next())
            .map(|(_, email)| email)
            .filter(|email| !email.eq_ignore_ascii_case(&m.from));
        if let Some(email) = reply_to {
            if m.from.is_empty() {
                entries.push(Entry::Separator);
            }
            entries.push(entry(
                "Show Mail With This Reply-To",
                "search",
                Action::SearchFor(search_term("reply-to", &email)),
            ));
        }
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
                cx.background_executor().timer(Duration::from_secs(4)).await;
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
            // A panic in a job is reported as its error; left alone, it would
            // end the background thread and then the app awaiting it.
            let result = cx
                .background_executor()
                .spawn(async move {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(backend)))
                        .unwrap_or_else(|_| {
                            Err(anyhow::anyhow!(
                                "Something went wrong while doing this. Please try again."
                            ))
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }
    /// Like `task`, for jobs that build a message list: they run one at a
    /// time on a thread of their own (see `views::on_list_thread`).
    fn list_task<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<Backend>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<T>, &mut Context<Self>) + 'static,
    ) {
        // Tests run jobs on GPUI's executor, which they wait for when parked.
        #[cfg(test)]
        return self.task(cx, job, done);
        #[cfg(not(test))]
        self.list_task_on_thread(cx, job, done);
    }
    #[cfg(not(test))]
    fn list_task_on_thread<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<Backend>) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, anyhow::Result<T>, &mut Context<Self>) + 'static,
    ) {
        self.busy += 1;
        let backend = self.backend.clone();
        let result = views::on_list_thread(move || job(backend));
        cx.spawn(async move |this, cx| {
            let result = result.await.unwrap_or_else(|_| {
                Err(anyhow::anyhow!(
                    "Something went wrong while doing this. Please try again."
                ))
            });
            let _ = this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn apply_theme(&self, w: &mut Window, cx: &mut App) {
        prompt::set_colors(self.colors());
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
        for composer in &self.composers {
            composer.update(cx, |composer, cx| {
                composer.colors = c;
                cx.notify();
            });
        }
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
        theme.scrollbar_mode = ScrollbarMode::Always;
        theme.colors.scrollbar = transparent_black();
        theme.colors.scrollbar_thumb = rgba((c.muted << 8) | 0x55).into();
        theme.colors.scrollbar_thumb_hover = rgba((c.muted << 8) | 0xa0).into();
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            theme.colors.background = transparent_black();
        }
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.account.as_deref().is_some_and(fwd::is_view) {
            self.refresh_view(cx);
            return;
        }
        let Some(a) = self.current_account() else {
            return;
        };
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
            self.sync_drafts_soon(&a.id, cx);
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
        let visible = self.shows(&id) && self.mailbox() == mailbox;
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
                let visible = this.shows(&id) && this.mailbox().key() == key;
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
                        // Drafts written on other devices appear, and ones
                        // written here reach them, with the inbox's checks.
                        let due = this
                            .draft_synced
                            .get(&id)
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(120));
                        if due {
                            this.sync_drafts_soon(&id, cx);
                        }
                    }
                    if visible {
                        this.reload_if_changed(cx);
                        if !this.subs_open
                            && let Some(a) = this.accounts.iter().find(|a| a.id == id).cloned()
                        {
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
        self.subs_open = false;
        self.subs_sender = None;
        self.subs_return.set(None);
        self.account = account;
        self.folder = mailbox.folder();
        self.remote = match &mailbox {
            Mailbox::Remote(name) => name.clone(),
            Mailbox::Standard(_) => String::new(),
        };
        self.update_scope();
        self.selected = None;
        self.selecting = false;
        self.select_mode = false;
        self.selection.clear();
        self.load_generation += 1;
        self.read_generation += 1;
        self.syncing = false;
        self.cache_loading = false;
        self.messages.clear();
        self.visible.clear();
        self.layout = None;
        self.wanted_rows.clear();
        self.unfillable.clear();
        self.filter_by = Filter::All;
        Self::set(&self.search, "", w, cx);
        self.search_open = false;
        self.server_search = None;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
            for id in self.scope.accounts.clone() {
                self.sync_drafts_soon(&id, cx);
            }
        } else if self.account.is_none() {
            let folder = self.folder;
            self.messages = self
                .demo
                .iter()
                .filter(|m| {
                    if folder == Folder::Starred {
                        m.starred
                    } else if folder == Folder::All {
                        m.folder != Folder::Drafts
                    } else {
                        m.folder == folder
                    }
                })
                .cloned()
                .collect();
            self.filter(cx);
        } else {
            let gen_ = self.load_generation;
            self.cache_loading = true;
            self.list_task(cx, self.list_job(mailbox), move |this, result, cx| {
                if gen_ != this.load_generation {
                    return;
                }
                this.cache_loading = false;
                if let Ok((ms, layout, key)) = result {
                    this.layout = layout;
                    this.set_messages(ms, cx);
                    this.listed = Some(key);
                }
                this.refresh(cx);
            });
        }
        self.save_place();
        self.watch_open_folder();
        self.anim += 1;
        cx.notify();
    }
    /// Reload confirmed cache state after a failed or partly applied operation.
    /// A generation check prevents an old request from replacing a newer view.
    ///
    /// Requests made while one runs are coalesced into a single follow-up:
    /// in All Accounts, every account's sync finishing asks for a reload,
    /// and each one lists every account's mailbox (tens of thousands of
    /// messages, tens of megabytes). Run side by side, they held several
    /// such lists at once, and the allocator kept that memory for good.
    fn reload_cached(&mut self, cx: &mut Context<Self>) {
        if self.subs_open {
            // Subscriptions list their own mail, not the mailbox under them.
            self.reload_subscriptions(cx);
            return;
        }
        if self.account.is_none() {
            return;
        }
        if self.reload_running {
            self.reload_again = true;
            return;
        }
        self.reload_running = true;
        let mailbox = self.mailbox();
        let generation = self.load_generation;
        self.list_task(cx, self.list_job(mailbox), move |this, result, cx| {
            this.reload_running = false;
            if generation == this.load_generation
                && let Ok((messages, layout, key)) = result
            {
                this.layout = layout;
                this.set_messages(messages, cx);
                this.listed = Some(key);
            }
            this.reload_follow_up(cx);
        });
    }
    /// What was asked for while a reload ran, in whichever view is open now:
    /// a whole reload, or a check for changes.
    fn reload_follow_up(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.reload_again) {
            self.recheck_again = false;
            self.reload_cached(cx);
        } else if std::mem::take(&mut self.recheck_again) {
            self.reload_if_changed(cx);
        }
    }
    /// After a check of a mailbox the list shows: rebuilds what changed.
    /// Most checks find nothing new, and the list stays as it is. In a view
    /// of several accounts, only the accounts whose mail changed are listed
    /// again and put in place of theirs; the others' mail stays.
    fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        if self.subs_open || self.account.is_none() {
            self.reload_cached(cx);
            return;
        }
        if self.reload_running {
            self.recheck_again = true;
            return;
        }
        let changed = match self.list_freshness() {
            views::Freshness::Current => return,
            views::Freshness::Stale(changed) if changed.len() < self.scope.accounts.len() => changed,
            views::Freshness::Stale(_) | views::Freshness::Unknown => {
                self.reload_cached(cx);
                return;
            }
        };
        let accounts: Vec<String> = changed
            .into_iter()
            .map(|i| self.scope.accounts[i].clone())
            .collect();
        self.reload_running = true;
        let generation = self.load_generation;
        let job = Self::account_lists_job(accounts, self.mailbox());
        self.list_task(cx, job, move |this, result, cx| {
            this.reload_running = false;
            match result {
                // Another view meanwhile lists itself; these parts are dropped.
                Ok(parts) => {
                    this.replace_account_lists(generation, parts, cx);
                }
                Err(_) => this.reload_again = true,
            }
            this.reload_follow_up(cx);
        });
    }
    fn load_drafts(&mut self, cx: &mut Context<Self>) {
        let ids = match &self.account {
            Some(id) if fwd::is_view(id) => self.scope.accounts.clone(),
            Some(id) => vec![id.clone()],
            None => vec!["demo".into()],
        };
        let generation = self.load_generation;
        self.task(
            cx,
            move |b| {
                let mut drafts = Vec::new();
                for id in &ids {
                    drafts.extend(b.store.drafts(id)?);
                }
                Ok(drafts)
            },
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
                            .map(|d| {
                                let mut m = Mail {
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
                                // Scheduled ones say when they go, or why
                                // they didn't.
                                snippet: match d.send_at {
                                    Some(at) => {
                                        let when = crate::manage::composing::describe_later(
                                            at,
                                            chrono::Local::now(),
                                        );
                                        if d.send_error.is_empty() {
                                            format!("Scheduled to send {when} · {}", d.body)
                                        } else {
                                            format!(
                                                "Not sent {when}: {} · {}",
                                                d.send_error, d.body
                                            )
                                        }
                                    }
                                    None => d.body.clone(),
                                },
                                date: d.send_at.unwrap_or_else(|| chrono::Utc::now().timestamp()),
                                unread: false,
                                starred: false,
                                size: 0,
                                body: Some(d.body.clone()),
                                html: None,
                                message_id: String::new(),
                                has_attachments: !d.attachments.is_empty(),
                                gmail_id: 0,
                                list_unsubscribe: String::new(),
                                one_click: false,
                                pending: false,
                                reply_to: String::new(),
                                inline: Default::default(),
                                ..Mail::default()
                                };
                                m.set_cc(d.cc.clone());
                                m
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
        // A placeholder has nothing to open until it is listed.
        if self.messages.iter().any(|m| m.id == id && m.pending) {
            return;
        }
        // Clicking the open message again leaves the reader as it is.
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        self.html_view.update(cx, |view, cx| view.clear(cx));
        // A subscription's mail list gives way to the message; keep its place.
        if self.subs_open && self.selected.is_none() {
            self.subs_return
                .set(Some(self.list_scroll.0.borrow().base_handle.offset()));
        }
        self.selected = Some(id.clone());
        self.details_open = false;
        self.quotes_open = false;
        self.thread_open = false;
        self.open_anim += 1;
        self.open_at = Some(std::time::Instant::now());
        self.reveal = None;
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
        // Its reverse aliases show as their contacts once those are known.
        self.learn_contacts(&m, cx);
        // Opening marks the message read immediately; the server catches up.
        for m in self
            .messages
            .iter_mut()
            .chain(self.demo.iter_mut())
            .filter(|m| m.id == id)
        {
            m.unread = false;
        }
        if let Some(a) = self.account_of(&m) {
            let account_id = a.id.clone();
            let id2 = id.clone();
            self.reading = Some(id.clone());
            let mark = a.clone();
            // Showing the message doesn't wait for the server to mark it
            // read: that needs the account's connection, which a sync or a
            // download may be using.
            self.task(
                cx,
                move |b| b.read(&a, &m),
                move |this, result, cx| {
                    if this.reading.as_ref() == Some(&id2) {
                        this.reading = None;
                    }
                    this.account_result(&account_id, IssueSource::Read, &result);
                    match result {
                        Ok(mut mail) => {
                            let unread = mail.unread;
                            // Shown read already; the server catches up below.
                            mail.unread = false;
                            if let Some(old) = this.messages.iter_mut().find(|m| m.id == id2) {
                                *old = mail.clone();
                            }
                            this.filter(cx);
                            if unread {
                                this.mark_read(mark, mail, cx);
                            } else {
                                this.refresh_counts(cx);
                            }
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
    /// Marks an opened message read at the server and in the cache. It is
    /// shown read already; a refusal shows the cache as it is.
    fn mark_read(&mut self, a: Account, mail: Mail, cx: &mut Context<Self>) {
        let account_id = a.id.clone();
        let id = mail.id.clone();
        self.task(
            cx,
            move |b| b.change(&a, &Mail { unread: true, ..mail }, Change::Read(true)),
            move |this, result, cx| {
                this.account_result(&account_id, IssueSource::Read, &result);
                match result {
                    Ok(changed) => {
                        if let Some(m) = this.messages.iter_mut().find(|m| m.id == id) {
                            m.unread = changed.unread;
                            m.starred = changed.starred;
                        }
                        this.filter(cx);
                        this.refresh_counts(cx);
                    }
                    Err(_) => this.reload_cached(cx),
                }
            },
        );
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
            change.apply(m);
        }
        // A subscription lists its mail in every folder but Spam and Trash.
        let leaves = match change {
            Change::Move(folder) if self.subs_open => matches!(folder, Folder::Junk | Folder::Trash),
            Change::MoveTo(_) if self.subs_open => false,
            // All Mail lists every mailbox: moved mail stays where it is.
            Change::Move(_) | Change::MoveTo(_) if self.folder == Folder::All => false,
            Change::Move(_) | Change::MoveTo(_) => true,
            _ => self.folder == Folder::Starred && change == Change::Star(false),
        };
        if leaves {
            self.messages.retain(|m| !ids.contains(&m.id));
            if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
                self.selected = None;
            }
        }
        self.filter(cx);
        if self.selection.is_empty() {
            self.selecting = false;
            self.select_mode = false;
        }
        cx.notify();
        // A view can list several mailboxes; each one's server gets its own.
        let mut groups: Vec<(Account, Vec<Mail>)> = vec![];
        for m in before {
            let Some(a) = self.account_of(&m) else {
                continue;
            };
            match groups.iter_mut().find(|(g, _)| g.id == a.id) {
                Some((_, ms)) => ms.push(m),
                None => groups.push((a, vec![m])),
            }
        }
        for (a, before) in groups {
            let generation = self.load_generation;
            let count = before.len();
            let sent = before.clone();
            let account_id = a.id.clone();
            let change = change.clone();
            let sending = change.clone();
            self.task(
                cx,
                move |b| b.change_many(&a, &sent, sending),
                move |this, result, cx| {
                    this.refresh_counts(cx);
                    this.account_result(&account_id, IssueSource::Change, &result);
                    match result {
                        Ok(()) => {
                            // Moved mail has new identities in its new mailbox.
                            if change.moves() && this.folder == Folder::All {
                                this.reload_cached(cx);
                            }
                            if let Some(to) = this.destination_label(&account_id, &change) {
                                this.notice(
                                    if count == 1 {
                                        format!("Moved to {to}")
                                    } else {
                                        format!("Moved {count} messages to {to}")
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
                                this.messages.sort_by(Mail::newest_first);
                                this.filter(cx);
                            }
                            this.reload_cached(cx);
                        }
                    }
                },
            );
        }
    }
    /// Where a move puts an account's mail, as the sidebar names it.
    pub(super) fn destination_label(&self, account: &str, change: &Change) -> Option<String> {
        match change {
            Change::Move(folder) => Some(folder.label().into()),
            Change::MoveTo(name) => Some(self.folder_label(account, name)),
            _ => None,
        }
    }
    /// The account whose server folders `target` can move into: the one
    /// all of its mail is in, when that account keeps folders on its
    /// server and has some of its own. Also the server folder all of it is
    /// in already, if any, which isn't offered.
    fn move_account(&self, target: &MoveTarget) -> Option<(String, Option<String>)> {
        let (account, mails): (String, Vec<&Mail>) = match target {
            MoveTarget::Subscription(sub) => (sub.account.clone(), vec![]),
            MoveTarget::Mail(ids) => {
                let mails: Vec<&Mail> =
                    self.messages.iter().filter(|m| ids.contains(&m.id)).collect();
                (mails.first()?.account.clone(), mails)
            }
            MoveTarget::Selection => {
                let mails: Vec<&Mail> = self
                    .messages
                    .iter()
                    .filter(|m| self.selection.contains(&m.id))
                    .collect();
                (mails.first()?.account.clone(), mails)
            }
        };
        if mails.iter().any(|m| m.account != account)
            || !self.manages_folders(&account)
            || self.folders.get(&account).is_none_or(|fs| fs.is_empty())
        {
            return None;
        }
        let first = mails.first();
        let within = first
            .filter(|f| {
                f.folder == Folder::Other
                    && mails
                        .iter()
                        .all(|m| m.folder == Folder::Other && m.remote_folder == f.remote_folder)
            })
            .map(|f| f.remote_folder.clone());
        Some((account, within))
    }
    /// The "Move to Folder…" entry for a menu, when there are folders to offer.
    fn move_folder_entry(&self, label: impl Into<SharedString>, target: MoveTarget) -> Option<Entry> {
        let (account, within) = self.move_account(&target)?;
        let others = self.folders.get(&account)?.iter().any(|f| Some(&f.name) != within.as_ref());
        others.then(|| entry(label, "folder", Action::MoveMenu(target)))
    }
    /// The messages a menu's target names (none for a subscription).
    fn target_ids(&self, target: &MoveTarget) -> Vec<String> {
        match target {
            MoveTarget::Mail(ids) => ids.clone(),
            MoveTarget::Selection => self.selection.iter().cloned().collect(),
            MoveTarget::Subscription(_) => vec![],
        }
    }
    /// The IMAP account whose labels `target`'s messages can have: the one
    /// they are all from.
    fn label_account(&self, target: &MoveTarget) -> Option<&Account> {
        let ids = self.target_ids(target);
        let mut mails = self.messages.iter().filter(|m| ids.contains(&m.id));
        let first = mails.next()?;
        if mails.any(|m| m.account != first.account) {
            return None;
        }
        self.accounts
            .iter()
            .find(|a| a.id == first.account && a.provider != Provider::Pop3)
    }
    /// The "Labels…" entry for a message menu.
    fn label_entry(&self, label: impl Into<SharedString>, target: MoveTarget) -> Option<Entry> {
        self.label_account(&target)?;
        Some(entry(label, "tag", Action::LabelMenu(target)))
    }
    /// The label picker: labels in use on the account (and Gmail's own
    /// labels, its folders), checked where every message has one; choosing
    /// one adds or removes it.
    fn label_entries(&self, target: &MoveTarget) -> Vec<Entry> {
        let Some(a) = self.label_account(target) else {
            return vec![];
        };
        let ids = self.target_ids(target);
        let mails: Vec<&Mail> = self.messages.iter().filter(|m| ids.contains(&m.id)).collect();
        let gmail = a.provider == Provider::Gmail;
        let mut known = self.backend.store.labels(&a.id).unwrap_or_default();
        if gmail {
            known.extend(
                self.folders
                    .get(&a.id)
                    .into_iter()
                    .flatten()
                    .filter(|f| !f.name.starts_with("[Gmail]"))
                    .map(|f| f.name.clone()),
            );
        }
        known.extend(mails.iter().flat_map(|m| m.labels().iter().cloned()));
        known.sort_by_key(|l| l.to_lowercase());
        known.dedup();
        let mut entries: Vec<Entry> = known
            .into_iter()
            .map(|label| {
                let all = mails.iter().all(|m| m.labels().contains(&label));
                let shown = if gmail {
                    crate::sync::transport::decode_name(&label)
                } else {
                    label.clone()
                };
                let change = Change::Label(label, !all);
                let action = match target {
                    MoveTarget::Selection => Action::ApplySelection(change),
                    _ => Action::Apply(ids.clone(), change),
                };
                checked(shown, all, action)
            })
            .collect();
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        entries.push(entry("New Label…", "plus", Action::NewLabel(target.clone())));
        entries
    }
    /// Applies a change to a menu's target.
    fn apply_target(&mut self, target: MoveTarget, ids: Vec<String>, change: Change, cx: &mut Context<Self>) {
        match target {
            MoveTarget::Subscription(sub) => self.change_subscription(*sub, change, cx),
            _ => self.apply(ids, change, cx),
        }
    }
    /// Deletes mail for good, as confirmed: it leaves the list at once and
    /// comes back if the server refuses.
    fn erase(&mut self, erase: Erase, cx: &mut Context<Self>) {
        self.modal = None;
        match erase {
            Erase::Mail(ids) => {
                let gone: Vec<Mail> = self
                    .messages
                    .iter()
                    .filter(|m| ids.contains(&m.id))
                    .cloned()
                    .collect();
                self.messages.retain(|m| !ids.contains(&m.id));
                self.selection.retain(|id| !ids.contains(id));
                if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
                    self.selected = None;
                }
                self.filter(cx);
                let mut groups: Vec<(Account, Vec<Mail>)> = vec![];
                for m in gone {
                    let Some(a) = self.account_of(&m) else {
                        continue;
                    };
                    match groups.iter_mut().find(|(g, _)| g.id == a.id) {
                        Some((_, ms)) => ms.push(m),
                        None => groups.push((a, vec![m])),
                    }
                }
                for (a, mails) in groups {
                    let count = mails.len();
                    let id = a.id.clone();
                    self.task(
                        cx,
                        move |b| b.delete_forever(&a, &mails),
                        move |this, result, cx| {
                            this.account_result(&id, IssueSource::Change, &result);
                            this.refresh_counts(cx);
                            match result {
                                Ok(()) => this.notice(
                                    if count == 1 {
                                        "Deleted permanently".to_owned()
                                    } else {
                                        format!("Deleted {count} messages permanently")
                                    },
                                    false,
                                    cx,
                                ),
                                Err(e) => {
                                    this.notice(format!("Couldn't delete: {e:#}"), true, cx);
                                    this.reload_cached(cx);
                                }
                            }
                        },
                    );
                }
            }
            Erase::Mailbox(id, folder) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                if self.shows(&id) && self.folder == folder {
                    self.messages.retain(|m| m.account != id);
                    self.selected = None;
                    self.selection.clear();
                    self.filter(cx);
                }
                self.task(
                    cx,
                    move |b| b.empty_mailbox(&a, folder),
                    move |this, result, cx| {
                        this.account_result(&id, IssueSource::Change, &result);
                        this.refresh_counts(cx);
                        match result {
                            Ok(n) => this.notice(
                                format!(
                                    "Emptied {} ({} message{})",
                                    folder.label(),
                                    fmt_count(n),
                                    if n == 1 { "" } else { "s" }
                                ),
                                false,
                                cx,
                            ),
                            Err(e) => {
                                this.notice(format!("Couldn't empty {}: {e:#}", folder.label()), true, cx);
                                this.reload_cached(cx);
                            }
                        }
                    },
                );
            }
        }
        cx.notify();
    }
    /// The dialog asking before mail is deleted for good.
    fn erase_modal(&self, erase: &Erase) -> AnyElement {
        let c = self.colors();
        let (question, pop) = match erase {
            Erase::Mail(ids) => {
                let pop = self
                    .messages
                    .iter()
                    .filter(|m| ids.contains(&m.id))
                    .any(|m| self.account_of(m).is_some_and(|a| a.provider == Provider::Pop3));
                (
                    if ids.len() == 1 {
                        "Delete this message permanently?".to_owned()
                    } else {
                        format!("Delete {} messages permanently?", fmt_count(ids.len()))
                    },
                    pop,
                )
            }
            Erase::Mailbox(id, folder) => {
                let a = self.accounts.iter().find(|a| &a.id == id);
                (
                    format!(
                        "Empty {} in {}?",
                        folder.label(),
                        a.map(|a| a.email.as_str()).unwrap_or_default()
                    ),
                    a.is_some_and(|a| a.provider == Provider::Pop3),
                )
            }
        };
        div()
            .id("erase")
            .debug_selector(|| "erase".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(question),
            )
            .child(div().text_size(px(12.)).text_color(rgb(c.muted)).child(if pop {
                "They're deleted from this computer. The server's copies are kept (Picacho never deletes POP mail there) but won't download again."
            } else {
                "They're deleted from the server and this computer. This can't be undone."
            }))
            .into_any_element()
    }
    /// The dialog naming a new label.
    fn new_label_modal(&self, target: &MoveTarget) -> AnyElement {
        let c = self.colors();
        let n = self.target_ids(target).len();
        div()
            .id("new-label")
            .debug_selector(|| "new-label".into())
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if n == 1 {
                        "A label for this message, kept on the server.".to_owned()
                    } else {
                        format!("A label for these {} messages, kept on the server.", fmt_count(n))
                    }),
            )
            .child(
                div()
                    .id("new-label-name")
                    .debug_selector(|| "new-label-name".into())
                    .child(
                        field(Input::new(&self.label_input), c, false)
                            .h(px(39.))
                            .text_size(px(13.)),
                    ),
            )
            .into_any_element()
    }
    /// The folder picker: the account's own server folders, nested as in
    /// the sidebar, each moving `target` there.
    fn move_folder_entries(&self, target: &MoveTarget) -> Vec<Entry> {
        let Some((account, within)) = self.move_account(target) else {
            return vec![];
        };
        // In the sidebar's order.
        self.ordered_folders(&account)
            .iter()
            .filter(|f| Some(&f.name) != within.as_ref())
            .map(|f| {
                let change = Change::MoveTo(f.name.clone());
                let action = match target {
                    MoveTarget::Mail(ids) => Action::Apply(ids.clone(), change),
                    MoveTarget::Selection => Action::ApplySelection(change),
                    MoveTarget::Subscription(sub) => {
                        Action::ChangeSubscription(sub.clone(), change)
                    }
                };
                // Em spaces indent subfolders beneath their parents.
                let label = format!("{}{}", "\u{2003}".repeat(f.depth), f.label);
                entry(label, "folder", action)
            })
            .collect()
    }
    fn mark_all_read(&mut self, id: String, mailbox: Mailbox, cx: &mut Context<Self>) {
        if fwd::is_view(&id) {
            self.mark_view_read(id, mailbox, cx);
            return;
        }
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
    fn close(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some()
            || self.presets_for.take().is_some()
            || self.model_confirm.take().is_some()
        {
            cx.notify();
            return;
        }
        if self.cancel_rename(w, cx) {
            return;
        }
        if self.modal.is_none() {
            if self.selecting {
                self.selection.clear();
                self.select_mode = false;
                self.sync_selecting();
            } else if self.search_open {
                self.close_search(w, cx);
            } else if self.subs_open && self.selected.is_some() {
                self.handle(Action::BackToList, w, cx);
            } else if self.subs_open && self.subs_sender.is_some() {
                self.close_subscription(w, cx);
            } else if window::content_size(w).width < px(900.) {
                self.selected = None;
                self.read_generation += 1;
            }
            cx.notify();
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.connecting = false;
        self.sign_in_retry = false;
        self.backend.forget_sign_in();
        self.discovering = false;
        self.setup_generation += 1;
        self.modal = None;
        self.status.clear();
        self.error = false;
        self.drop_proton_login(cx);
        for i in [PASSWORD, TOTP, MAILBOX_PASSWORD, KEY_PIN] {
            Self::set(&self.form[i], "", w, cx);
        }
        self.focus.focus(w, cx);
        if self.folder == Folder::Drafts {
            self.load_drafts(cx);
        }
        cx.notify();
    }
    fn begin_setup(&mut self, step: Step, w: &mut Window, cx: &mut Context<Self>) {
        self.drop_proton_login(cx);
        for i in [EMAIL, PASSWORD, LOGIN, TOTP, MAILBOX_PASSWORD, KEY_PIN] {
            Self::set(&self.form[i], "", w, cx);
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.sign_in_retry = false;
        self.backend.forget_sign_in();
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
        // Without Direct in this build, Proton Mail is its Bridge sign-in.
        let service = if service == Service::Proton && !self.proton_direct {
            Service::ProtonBridge
        } else {
            service
        };
        self.drop_proton_login(cx);
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
        if service.sign_in().typed() {
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
            SignIn::Password | SignIn::Proton => {
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
                    a.username = value(LOGIN);
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
        if self.service == Service::Proton {
            return self.proton_continue(None, cx);
        }
        let a = self.setup_account(cx);
        let password = Self::value(&self.form[PASSWORD], cx);
        let password = if a.service == Service::Gmail {
            password.replace(' ', "")
        } else {
            password
        };
        if self.service.sign_in().typed() {
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
        self.sign_in_step = SignInStep::Browser;
        self.start_connect(cx, move |b, cancel| b.connect(&a, password, &taken, cancel));
    }
    /// Follows the backend's progress on this sign-in until it ends or is
    /// replaced, so the waiting text moves on once the browser answers.
    pub(super) fn follow_sign_in(&mut self, cx: &mut Context<Self>) {
        let cancel = self.cancel.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let running = this.update(cx, |this, cx| {
                    if !this.connecting || !Arc::ptr_eq(&this.cancel, &cancel) {
                        return false;
                    }
                    let step = this.backend.sign_in_step();
                    if step != this.sign_in_step {
                        this.sign_in_step = step;
                        cx.notify();
                    }
                    if this.key_method.is_some() {
                        let prompt = this.backend.proton_prompt();
                        if prompt != this.key_prompt {
                            this.key_prompt = prompt;
                            cx.notify();
                        }
                    }
                    true
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
    /// Runs a new-account sign-in (or its retry) and applies the result.
    fn start_connect(
        &mut self,
        cx: &mut Context<Self>,
        sign_in: impl FnOnce(&Backend, Arc<AtomicBool>) -> anyhow::Result<Account> + Send + 'static,
    ) {
        self.connecting = true;
        self.sign_in_retry = false;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let cancelled = cancel.clone();
        self.status.clear();
        self.error = false;
        self.follow_sign_in(cx);
        self.task(
            cx,
            move |b| {
                let saved = sign_in(&b, cancel)?;
                let encrypted = saved.service == Service::Pinal && b.encrypted(&saved);
                Ok((saved, encrypted))
            },
            move |this, result, cx| {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                this.connecting = false;
                match result {
                    Ok((saved, encrypted)) => this.connected(saved, encrypted, cx),
                    // The whole chain: a bare "Sign-in needs attention" hides
                    // whether the token step or the mail server refused.
                    Err(e) => {
                        this.sign_in_retry = this.backend.can_retry_sign_in();
                        this.notice(format!("{e:#}"), true, cx)
                    }
                }
            },
        );
    }
    /// Shows a newly added account and starts syncing it.
    pub(super) fn connected(&mut self, saved: Account, encrypted: bool, cx: &mut Context<Self>) {
        if encrypted {
            self.encrypted.insert(saved.id.clone());
        }
        self.accounts.push(saved.clone());
        self.sync_tree();
        self.load_avatar(saved.clone(), cx);
        self.account = Some(saved.id.clone());
        self.update_scope();
        if self.onboarding {
            self.step = Step::Done;
            self.anim += 1;
        } else {
            self.notice(format!("Added {}", saved.email), false, cx);
        }
        self.modal = None;
        self.folder = Folder::Inbox;
        self.messages.clear();
        self.visible.clear();
        self.selected = None;
        self.load_generation += 1;
        self.filter(cx);
        if self.onboarding {
            self.status.clear();
        }
        self.refresh(cx);
    }
    /// Repeats the step after the browser that failed, in the open setup or
    /// sign-in-again form.
    fn retry_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.connecting || !self.sign_in_retry {
            return;
        }
        match self.modal.clone() {
            Some(Modal::Reconnect(id)) => self.retry_reconnect(id, cx),
            _ if self.step == Step::Credentials => {
                self.sign_in_step = SignInStep::Finishing;
                self.start_connect(cx, |b, cancel| b.retry_sign_in(cancel))
            }
            _ => {}
        }
    }
    /// Reads the open message's original source and its details once.
    fn load_details(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else {
            return;
        };
        if self.details.as_ref().is_some_and(|d| d.id == m.id) {
            return;
        }
        self.details = Some(MessageInfo {
            id: m.id.clone(),
            result: None,
        });
        let Some(account) = self.account_of(&m) else {
            self.details = Some(MessageInfo {
                id: m.id.clone(),
                result: Some(Err("This message's account is not available.".into())),
            });
            return;
        };
        let id = m.id.clone();
        self.task(
            cx,
            move |b| {
                let raw = b.original(&account, &m)?;
                // The sender is checked only when asked (Check sender): the
                // lookups reach the sender's DNS and can tell it when the
                // message was opened.
                let details = crate::sync::mime::details(&raw, b.decrypts(&account, &raw));
                Ok(Original::new(details, &raw))
            },
            move |this, result, cx| {
                if this.details.as_ref().is_some_and(|d| d.id == id) {
                    this.details = Some(MessageInfo {
                        id,
                        result: Some(result.map(Rc::new).map_err(|e| e.to_string())),
                    });
                }
                cx.notify();
            },
        );
    }
    /// Checks the open message's sender, once, when asked.
    fn check_sender(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.original_source() else {
            return;
        };
        let Some(id) = self.selected.clone() else {
            return;
        };
        if original.details.checked || self.sender_checking.as_ref() == Some(&id) {
            return;
        }
        self.sender_checking = Some(id.clone());
        let (raw, mut details) = (original.raw.clone(), original.details.clone());
        self.task(
            cx,
            move |_| {
                crate::sync::auth::check(&raw, &mut details);
                Ok(Original::new(details, &raw))
            },
            move |this, result, cx| {
                if this.sender_checking.as_ref() == Some(&id) {
                    this.sender_checking = None;
                }
                if let (Ok(original), Some(info)) = (result, this.details.as_mut())
                    && info.id == id
                {
                    info.result = Some(Ok(Rc::new(original)));
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    fn original_source(&self) -> Option<Rc<Original>> {
        let info = self.details.as_ref()?;
        if Some(&info.id) != self.selected.as_ref() {
            return None;
        }
        info.result.as_ref()?.as_ref().ok().cloned()
    }
    fn save_original(&mut self, cx: &mut Context<Self>) {
        let (Some(original), Some(m)) = (self.original_source(), self.selected_mail()) else {
            return;
        };
        let stem: String = m
            .subject
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(60)
            .collect();
        let name = format!("{}.eml", stem.trim().trim_matches('_').if_empty("message"));
        let picker = cx.prompt_for_new_path(&self.backend.store.path, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picker.await {
                let result = std::fs::write(&path, &original.raw);
                let _ = this.update(cx, |this, cx| match result {
                    Ok(()) => this.notice("Original message saved", false, cx),
                    Err(e) => this.notice(e.to_string(), true, cx),
                });
            }
        })
        .detach();
    }
    /// The email's own context menu: copy, links and images, and the source.
    fn html_menu(&mut self, menu: &HtmlContextMenu, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        if let Some(text) = &menu.selection {
            entries.push(entry("Copy", "copy", Action::CopyText(text.clone())));
        }
        if let Some(link) = &menu.link {
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            let mailto = link.starts_with("mailto:");
            entries.push(entry(
                if mailto {
                    "Open Email Link"
                } else {
                    "Open Link in Browser"
                },
                "external-link",
                Action::OpenUrl(link.clone()),
            ));
            entries.push(entry(
                if mailto {
                    "Copy Email Address"
                } else {
                    "Copy Link Address"
                },
                "copy",
                Action::CopyText(
                    link.strip_prefix("mailto:")
                        .map(|a| a.split('?').next().unwrap_or(a).to_string())
                        .unwrap_or_else(|| link.clone()),
                ),
            ));
        }
        if let Some(image) = &menu.image {
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            entries.push(entry(
                "Open Image in Browser",
                "external-link",
                Action::OpenUrl(image.clone()),
            ));
            entries.push(entry(
                "Copy Image Address",
                "copy",
                Action::CopyText(image.clone()),
            ));
        }
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        if let Some(body) = self.selected_mail().and_then(|m| m.body) {
            entries.push(entry("Copy Message Text", "copy", Action::CopyText(body)));
        }
        entries.push(entry("Show Original", "code", Action::ShowOriginal));
        self.open_menu(menu.position, entries, cx);
    }
    fn save_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else {
            return;
        };
        let Some(a) = m.attachments().get(index).cloned() else {
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
        self.menu = None;
        if self.onboarding
            && matches!(
                action,
                Action::Compose
                    | Action::ComposeTo(_)
                    | Action::Reply(_)
                    | Action::Forward
                    | Action::Settings
            )
        {
            return;
        }
        match action {
            Action::Compose => self.new_compose(0, w, cx),
            Action::ComposeTo(to) => self.compose_to(to, w, cx),
            Action::Reply(all) => {
                let all = all || self.compose_options.default_reply_all;
                self.new_compose(if all { 2 } else { 1 }, w, cx)
            }
            Action::Forward => self.new_compose(3, w, cx),
            Action::Refresh => {
                if let Some(id) = self.account.clone() {
                    if fwd::is_view(&id) {
                        self.refresh(cx);
                    } else {
                        self.retry_account(id, cx);
                    }
                }
            }
            Action::Select(id) => {
                if self.folder == Folder::Drafts {
                    self.handle(Action::OpenDraft(id), w, cx);
                } else {
                    self.select(id, cx);
                }
            }
            Action::OpenWindow(id) => self.open_message_window(id, w, cx),
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
                if change.moves() {
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
                let account = self.drafts.iter().find(|d| d.id == id).map(|d| d.account.clone());
                let _ = self.backend.discard_draft(&id);
                self.drop_composer_for_draft(&id, cx);
                self.load_drafts(cx);
                if let Some(account) = account {
                    self.sync_drafts_soon(&account, cx);
                }
            }
            Action::MarkAllRead(id, mailbox) => self.mark_all_read(id, mailbox, cx),
            Action::RefreshAccount(id) if fwd::is_view(&id) => {
                if self.account.as_ref() == Some(&id) {
                    self.refresh(cx);
                } else {
                    self.open_mailbox(Some(id), Folder::Inbox.into(), w, cx);
                }
            }
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
            Action::ToggleAccount(id) | Action::ToggleGroup(id) => self.toggle_node(id),
            Action::AddMenu => {
                let entries = self.create_entries(None);
                self.open_menu(w.mouse_position(), entries, cx);
            }
            Action::NewFolder(parent) => self.new_folder(parent, w, cx),
            Action::NewSeparator(parent) => self.new_separator(parent, w, cx),
            Action::RenameFolder(id) => self.start_rename(id, w, cx),
            Action::DeleteFolder(id) => self.delete_folder(&id),
            Action::DeleteRemoteFolder(id, name) => {
                if self.folder_deletable(&id, &name) {
                    self.modal = Some(Modal::DeleteFolder(id, name));
                    self.anim += 1;
                }
            }
            Action::ConfirmDeleteFolder(id, name) => self.delete_remote_folder(id, name, cx),
            Action::NewRemoteFolder(id, parent) => {
                self.edit_remote_folder(id, FolderEdit::New(parent), w, cx)
            }
            Action::RenameRemoteFolder(id, name) => {
                self.edit_remote_folder(id, FolderEdit::Rename(name), w, cx)
            }
            Action::RemoveSeparator(id) => self.remove_separator(&id),
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
            Action::AvatarMenu(id) => {
                let entries = self.avatar_entries(&id);
                self.open_menu_here(entries, w, cx);
                return;
            }
            Action::ProviderAvatar(id) => self.provider_avatar(id, cx),
            Action::AvatarPresets(id) => {
                if self.modal == Some(Modal::Settings) || self.settings_window.is_some() {
                    // Over Settings, which stays open where it was.
                    self.menu_anim += 1;
                    self.presets_for = Some(id);
                } else {
                    self.modal = Some(Modal::AvatarPresets(id));
                    self.anim += 1;
                }
            }
            Action::SetPresetAvatar(id, name) => self.preset_avatar(id, name, cx),
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
                        .filter(|i| !self.messages[**i].pending)
                        .map(|i| self.messages[*i].id.clone())
                        .collect();
                    self.selection = if self.selection == all {
                        HashSet::new()
                    } else {
                        all
                    };
                }
            }
            Action::StopSelecting => {
                self.selection.clear();
                self.select_mode = false;
            }
            Action::ToggleSelectMode => {
                if self.selecting {
                    self.selection.clear();
                    self.select_mode = false;
                } else if self.folder != Folder::Drafts {
                    // Nothing is picked until a message is clicked; the open
                    // one isn't taken for a pick.
                    self.select_mode = true;
                }
            }
            Action::SelectFromSender(id) => {
                if let Some(from) = self
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.from.to_lowercase())
                    .filter(|f| !f.is_empty())
                {
                    let ids: Vec<String> = self
                        .visible
                        .iter()
                        .map(|i| &self.messages[*i])
                        .filter(|m| !m.pending && m.from.to_lowercase() == from)
                        .map(|m| m.id.clone())
                        .collect();
                    self.selection.extend(ids);
                    self.select_anchor = Some(id);
                }
            }
            Action::SearchFor(query) => {
                if !self.search_open {
                    self.search_open = true;
                    self.search_anim += 1;
                }
                Self::set(&self.search, &query, w, cx);
                self.search.update(cx, |s, cx| s.focus(w, cx));
                self.apply_filter(cx);
                self.scroll_list_to_top();
            }
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
                self
                    .backend
                    .store.set_preference_soon("sort", if oldest { "oldest" } else { "newest" });
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
                // Adding an account happens in the main window, also when
                // asked from popped-out Settings.
                if let Some(main) = self.main_window
                    && main.window_id() != w.window_handle().window_id()
                {
                    let _ = main.update(cx, |_, w, _| w.activate_window());
                }
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
            Action::RetrySignIn => self.retry_sign_in(cx),
            Action::OpenSubscription(sub) => self.open_subscription(sub, cx),
            Action::CloseSubscription => self.close_subscription(w, cx),
            Action::SubscriptionsTab(unsubscribed) => {
                self.subs_tab = if unsubscribed {
                    subscriptions::SubsTab::Unsubscribed
                } else {
                    subscriptions::SubsTab::Subscribed
                };
                self.subs_scroll.scroll_to_item(0, ScrollStrategy::Top);
            }
            Action::ChangeSubscription(sub, change) => self.change_subscription(*sub, change, cx),
            Action::ToggleReadReceipts(id) => {
                self.edit_account(&id, |a| a.read_receipts = !a.read_receipts, cx)
            }
            Action::ToggleDeliveryReceipts(id) => {
                self.edit_account(&id, |a| a.delivery_receipts = !a.delivery_receipts, cx)
            }
            Action::ToggleRequireTls(id) => {
                self.edit_account(&id, |a| a.require_tls = !a.require_tls, cx)
            }
            Action::AddIdentity(account) => self.start_identity(account, None, w, cx),
            Action::EditIdentity(account, id) => self.start_identity(account, Some(id), w, cx),
            Action::RemoveIdentity(account, id) => self.remove_identity(account, id, cx),
            Action::ToggleIdentityServer => {
                if let Some(edit) = &mut self.identity_edit {
                    edit.own_server = !edit.own_server;
                }
            }
            Action::SaveIdentity => self.save_identity(cx),
            Action::CancelIdentity => self.identity_edit = None,
            Action::DeleteForever(ids) => {
                if !ids.is_empty() {
                    self.modal = Some(Modal::Erase(Erase::Mail(ids)));
                    self.anim += 1;
                }
            }
            Action::EmptyMailbox(id, folder) => {
                if self.accounts.iter().any(|a| a.id == id) {
                    self.modal = Some(Modal::Erase(Erase::Mailbox(id, folder)));
                    self.anim += 1;
                }
            }
            Action::ConfirmErase(erase) => self.erase(erase, cx),
            Action::LabelMenu(target) => {
                let entries = self.label_entries(&target);
                if !entries.is_empty() {
                    self.open_menu_here(entries, w, cx);
                }
                return;
            }
            Action::NewLabel(target) => {
                self.status.clear();
                self.error = false;
                self.modal = Some(Modal::NewLabel(target));
                self.anim += 1;
                self.label_input.update(cx, |s, cx| {
                    s.set_text("", w, cx);
                    s.focus(w, cx);
                });
            }
            Action::SaveLabel => {
                if let Some(Modal::NewLabel(target)) = self.modal.clone() {
                    let label = Self::value(&self.label_input, cx).trim().to_owned();
                    let ids = self.target_ids(&target);
                    let gmail = self
                        .move_account(&target)
                        .and_then(|(id, _)| self.accounts.iter().find(|a| a.id == id))
                        .is_some_and(|a| a.provider == Provider::Gmail);
                    let checked = if gmail {
                        // Gmail names labels like folders (modified UTF-7).
                        let label = crate::sync::transport::encode_name(&label);
                        crate::sync::transport::gmail_label(&label).map(|_| label)
                    } else {
                        crate::sync::transport::keyword(&label)
                    };
                    match checked {
                        Err(e) => self.notice(e.to_string(), true, cx),
                        Ok(label) => {
                            self.modal = None;
                            self.focus.focus(w, cx);
                            self.apply_target(target, ids, Change::Label(label, true), cx);
                        }
                    }
                }
            }
            Action::MoveMenu(target) => {
                let entries = self.move_folder_entries(&target);
                if !entries.is_empty() {
                    self.open_menu_here(entries, w, cx);
                }
                return;
            }
            Action::SubscriptionMenu(sub) => {
                let entries = self.subscription_entries(&sub);
                self.open_menu(w.mouse_position(), entries, cx);
            }
            Action::SetupBack if self.proton_stage.is_some() => self.proton_back(w, cx),
            Action::SetupBack => {
                self.sign_in_retry = false;
                self.backend.forget_sign_in();
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
                self.focus.focus(w, cx);
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
                self.focus.focus(w, cx);
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
                if !self.reveal_settings_window(cx) && self.modal != Some(Modal::Settings) {
                    self.settings_scroll.set_offset(point(px(0.), px(0.)));
                    self.fwd_reverse = None;
                    self.page = Page::General;
                    self.modal = Some(Modal::Settings);
                    self.anim += 1;
                }
            }
            Action::RetryAccount(id) => self.retry_account(id, cx),
            Action::DismissIssue(id, source) => self.dismiss_issue(&id, source),
            Action::Reconnect(id) => {
                if self.retrying.contains(&id) {
                    return;
                }
                self.cancel.store(true, Ordering::Relaxed);
                self.connecting = false;
                self.sign_in_retry = false;
                self.backend.forget_sign_in();
                self.status.clear();
                self.drop_proton_login(cx);
                for i in [PASSWORD, TOTP, MAILBOX_PASSWORD, KEY_PIN] {
                    Self::set(&self.form[i], "", w, cx);
                }
                self.modal = Some(Modal::Reconnect(id));
                self.anim += 1;
            }
            Action::SubmitReconnect(id) => self.reconnect(id, w, cx),
            Action::ProtonKey(method) => {
                let reconnect = self.proton_reconnecting();
                self.proton_use_key(method, reconnect, cx)
            }
            Action::ProtonMode(service) => {
                if !self.connecting
                    && self.service != service
                    && (service != Service::Proton || self.proton_direct)
                {
                    self.choose_service(service, w, cx)
                }
            }
            Action::ProtonBack => self.proton_back(w, cx),
            Action::AccountSettings(id) => {
                self.settings_scroll.set_offset(point(px(0.), px(0.)));
                self.fwd_reverse = None;
                self.page = Page::Account(id);
                if !self.reveal_settings_window(cx) && self.modal != Some(Modal::Settings) {
                    self.modal = Some(Modal::Settings);
                    self.anim += 1;
                }
            }
            Action::EditNickname(id, true) => self.start_rename(id, w, cx),
            Action::EditNickname(id, false) => {
                self.handle(Action::AccountSettings(id), w, cx);
                self.sync_nickname_input(w, cx);
                let input = self.nickname_input.clone();
                match self.settings_window {
                    Some(handle) if handle.window_id() != w.window_handle().window_id() => {
                        let _ = handle.update(cx, |_, w, cx| input.update(cx, |s, cx| s.focus(w, cx)));
                    }
                    _ => input.update(cx, |s, cx| s.focus(w, cx)),
                }
            }
            Action::SettingsPage(page) => {
                self.settings_scroll.set_offset(point(px(0.), px(0.)));
                self.fwd_reverse = None;
                if page == Page::Forwarding {
                    self.refresh_aliases_due(views::ALIAS_REFRESH_SOON, cx);
                }
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
                self.backend.store.set_preference_soon("theme", appearance.key());
            }
            Action::SetAccent(accent) => {
                self.accent = accent;
                self.apply_theme(w, cx);
                self.backend.store.set_preference_soon("accent", accent.key());
            }
            Action::SetSync(id, sync) => self.set_sync(id, sync, w, cx),
            Action::SetLocalAi(on) => self.set_local_ai(on, cx),
            Action::SetHideCodes(on) => self.set_hide_codes(on, cx),
            Action::SetUnderstandDates(on) => self.set_understand_dates(on, cx),
            Action::ShowCodes(id, show) => self.show_codes(id, show, cx),
            Action::CopyCode(id) => self.copy_code(id, cx),
            Action::AskDeleteModel(id) => self.ask_delete_model(id, cx),
            Action::DeleteModel(id) => self.delete_model(id, cx),
            Action::CloseModelSheet => {
                self.model_confirm = None;
            }
            Action::SetCleanLinks(on) => {
                self.clean_links = on;
                if on {
                    crate::manage::tracking::warm_up();
                }
                self.backend
                    .store
                    .set_preference_soon("clean_links", if on { "true" } else { "false" });
            }
            Action::SetReadingDefaults(options) => {
                let before = self.reading_defaults;
                self.save_preference(
                    "reading_defaults",
                    &serde_json::to_string(&options).unwrap(),
                    cx,
                    move |this, cx| {
                        this.reading_defaults = before;
                        this.options_cache.borrow_mut().take();
                        this.html_view.update(cx, |view, cx| view.clear(cx));
                    },
                );
                self.reading_defaults = options;
                self.options_cache.borrow_mut().take();
            }
            Action::ShowHtml(on) => {
                if let Some(m) = self.selected_mail()
                    && self.reading_options(&m).html != on
                {
                    self.handle(Action::ToggleHtml, w, cx);
                }
            }
            Action::ToggleQuotes => {
                self.quotes_open = !self.quotes_open;
                self.html_view.update(cx, |view, cx| view.clear(cx));
            }
            Action::ExpandThread => self.thread_open = true,
            Action::ToggleDetails => {
                self.details_open = !self.details_open;
                if self.details_open {
                    self.details_anim += 1;
                    self.details_at = Some(std::time::Instant::now());
                    self.load_details(cx);
                }
            }
            Action::CheckSender => self.check_sender(cx),
            Action::OpenSubscriptions(id) => self.open_subscriptions(id, w, cx),
            Action::Unsubscribe(sub) => self.unsubscribe(sub, w, cx),
            Action::ShowOriginal => self.handle(Action::ShowSource(SourceView::Original), w, cx),
            Action::ShowSource(view) => {
                self.source_view = view;
                self.load_details(cx);
                self.original_scroll.scroll_to_item(0, ScrollStrategy::Top);
                self.modal = Some(Modal::Original);
                self.menu = None;
                self.anim += 1;
            }
            Action::CopyOriginal => {
                if let Some(raw) = self.original_source() {
                    let view = self.source_view;
                    let Some((text, _)) = raw.view(view) else {
                        return;
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                    let what = match view {
                        SourceView::Headers => "Headers",
                        SourceView::Html => "HTML",
                        SourceView::Original => "Original message",
                    };
                    self.notice(format!("{what} copied"), false, cx);
                }
            }
            Action::SaveOriginal => self.save_original(cx),
            Action::CopyText(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.menu = None;
            }
            Action::OpenUrl(url) => {
                self.menu = None;
                // Addresses can come from an email; only web and email links
                // are handed to the system.
                let url = if self.clean_links {
                    crate::manage::tracking::clean_url(&url).unwrap_or(url)
                } else {
                    url
                };
                match super::html::openable_url(&url) {
                    Some(url) => cx.open_url(url.as_str()),
                    None => self.notice("Picacho only opens web and email links.", true, cx),
                }
            }
            Action::Reveal(fraction) => {
                self.reveal = Some(fraction.clamp(0., 1.));
                self.set_reader_collapsed(false, cx);
            }
            Action::ToggleHtml | Action::ToggleRemote | Action::ToggleEmailTheme => {
                if let Some(m) = self.selected_mail() {
                    if matches!(action, Action::ToggleRemote) {
                        self.switch_anim += 1;
                        self.switch_at = Some(std::time::Instant::now());
                    }
                    let mut options = self.reading_options(&m);
                    match action {
                        Action::ToggleHtml => options.html = !options.html,
                        Action::ToggleEmailTheme => {
                            options.theme = if options.theme.is_dark(self.dark) {
                                EmailTheme::Light
                            } else {
                                EmailTheme::Dark
                            }
                        }
                        _ => options.block_remote = !options.block_remote,
                    }
                    self.options_cache.borrow_mut().take();
                    // Unsaved, the message reads its options as saved before.
                    self.save_preference(
                        &message_key(&m.account, &m.id),
                        &serde_json::to_string(&options).unwrap(),
                        cx,
                        |this, cx| {
                            this.options_cache.borrow_mut().take();
                            this.html_view.update(cx, |view, cx| view.clear(cx));
                        },
                    );
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
                    if self.service.sign_in().typed() {
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
                                this.forget_mailbox(&id);
                                this.sync_tree();
                                this.account_issues.remove(&id);
                                this.dismissed_issues.remove(&id);
                                this.folders.remove(&id);
                                this.known.remove(&id);
                                this.last_check.remove(&id);
                                this.modal = None;
                                if this.account.as_ref() == Some(&id) {
                                    this.account = this.first_account();
                                    this.update_scope();
                                    this.messages.clear();
                                    this.visible.clear();
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
            Action::SaveAttachment(i) => self.save_attachment(i, cx),

            Action::OpenDraft(id) => {
                if let Some(d) = self.drafts.iter().find(|d| d.id == id).cloned() {
                    self.open_draft(d, w, cx);
                }
            }
            Action::UndoSend(id) => self.undo_send(id, w, cx),
            Action::SetComposeOptions(options) => self.save_compose_options(options, cx),
            Action::SetAllMail(on) => self.set_all_mail(on, cx),
            Action::ForwardingService(service) => {
                self.fwd_service = service;
                self.fwd_error = None;
            }
            Action::ConnectForwarding => self.connect_forwarding(w, cx),
            Action::RefreshAliases(id) => self.refresh_aliases(id, false, cx),
            Action::RemoveForwarding(id) => self.remove_forwarding(id, cx),
            Action::SetDeliverTo(id, to) => {
                self.edit_forwarding(&id, |f| f.deliver_to = to, cx);
            }
            Action::NewAlias(id) => {
                if self.fwd_creating.take() != Some(id.clone()) {
                    let mailbox = self
                        .forwarding
                        .iter()
                        .find(|f| f.id == id)
                        .and_then(|f| f.deliver_to.clone());
                    self.open_alias_form(id, mailbox, w, cx);
                }
            }
            Action::NewAliasFor(id, mailbox) => self.new_alias_for(id, mailbox, w, cx),
            Action::CreateAlias(id) => self.create_alias(id, cx),
            Action::DomainMenu(id) => {
                if !matches!(self.fwd_options.get(&id), Some(Ok(_))) {
                    self.load_alias_options(id, cx);
                } else {
                    let entries = self.domain_entries(&id);
                    self.open_menu_here(entries, w, cx);
                    return;
                }
            }
            Action::SetAliasDomain(suffix) => self.fwd_domain = Some(suffix),
            Action::PrefixMenu(id) => {
                let entries = self.prefix_entries(&id);
                self.open_menu_here(entries, w, cx);
                return;
            }
            Action::RandomPrefix(format) => self.random_prefix(format, w, cx),
            Action::SetAliasPrefix(prefix) => {
                self.fwd_format = None;
                Self::set(&self.fwd_prefix, prefix, w, cx);
            }
            Action::EditAlias(id, alias, which) => self.edit_alias(id, alias, which, w, cx),
            Action::SaveAliasEdit => self.save_alias_edit(cx),
            Action::CancelAliasEdit => self.fwd_editing = None,
            Action::SetAliasPinned(id, alias, pinned) => self.set_alias_pinned(id, alias, pinned, cx),
            Action::AliasMailboxMenu(id, alias) => {
                let entries = self.alias_mailbox_entries(&id, &alias);
                self.open_menu_here(entries, w, cx);
                return;
            }
            Action::ToggleAliasMailbox(id, alias, mailbox) => {
                self.toggle_alias_mailbox(id, alias, mailbox, cx)
            }
            Action::SetAliasPgp(id, alias, on) => self.set_alias_pgp(id, alias, on, cx),
            Action::ToggleReverseAliases(id, alias) => {
                self.toggle_reverse_aliases(id, alias, w, cx)
            }
            Action::LoadReverseAliases(id, alias) => self.load_reverse_aliases(id, alias, cx),
            Action::ContactMenu(id, alias, contact) => {
                let entries = self.contact_entries(&id, &alias, &contact);
                self.open_menu_here(entries, w, cx);
                return;
            }
            Action::ToggleContactBlock(id, alias, contact) => {
                self.toggle_contact_block(id, alias, contact, cx)
            }
            Action::DeleteContact(id, alias, contact) => self.delete_contact(id, alias, contact, cx),
            Action::AddContact(id, alias) => self.add_contact(id, alias, w, cx),
            Action::AliasPage(id, page) => {
                self.fwd_pages.insert(id, page);
            }
            Action::ContactPage(page) => {
                self.fwd_contact_page = page;
                self.fwd_reverse_scroll.set_offset(point(px(0.), px(0.)));
            }
            Action::SetAliasEnabled(id, alias, on) => self.set_alias_enabled(id, alias, on, cx),
            Action::SetAliasDisplay(id, alias, display) => {
                self.edit_forwarding(
                    &id,
                    |f| {
                        if let Some(a) = f.aliases.iter_mut().find(|a| a.id == alias) {
                            a.display = display;
                        }
                    },
                    cx,
                );
            }
            Action::DeleteAlias(id, alias) => self.delete_alias(id, alias, cx),
            Action::ToggleForwarding(id) => {
                if !self.fwd_open.remove(&id) {
                    self.fwd_open.insert(id);
                }
            }
            Action::ServiceMenu(id) => {
                let entries = self.service_entries(&id);
                self.open_menu_here(entries, w, cx);
            }
            Action::SetAliasesOnly(id, on) => {
                self.edit_forwarding(&id, |f| f.aliases_only = on, cx);
            }
            Action::DeliverMenu(id) => {
                let entries = self.deliver_entries(&id);
                self.open_menu_here(entries, w, cx);
            }
            Action::AliasMenu(id, alias) => {
                let entries = self.alias_entries(&id, &alias);
                self.open_menu_here(entries, w, cx);
            }
            Action::CancelForwardingConfirm => self.fwd_confirm = None,
            Action::AddForwarding => {
                self.fwd_adding = !self.fwd_adding;
                self.fwd_error = None;
                if self.fwd_adding {
                    self.fwd_key.update(cx, |s, cx| s.focus(w, cx));
                }
            }
            Action::PlaceSettings(place) => self.place_settings(place, w, cx),
            Action::ClosePresets => self.presets_for = None,
            Action::Dismiss => self.status.clear(),
            Action::SnoozeMenu(ids) => self.snooze_menu(ids, w, cx),
            Action::Unsnooze(account, message_id) => self.unsnooze(account, message_id, cx),
            Action::SearchServer => self.search_server(cx),
            Action::ClearServerSearch => {
                self.server_search = None;
                self.apply_filter(cx);
            }
            Action::Receipt(id, send) => self.answer_receipt(id, send, cx),
            Action::Filters(action) => self.filter_action(action, w, cx),
        }
        self.sync_selecting();
        cx.notify();
    }
    /// Selection mode is "something is selected", or turned on from the
    /// list's Select button; the bulk toolbar animates in as it starts.
    fn sync_selecting(&mut self) {
        let now = !self.selection.is_empty() || self.select_mode;
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
        let offline = a.sync.offline_days;
        let account = a.clone();
        // A new sync window applies at the next sync; run it now for the open account.
        if window && self.account.as_ref() == Some(&id) {
            self.open_mailbox(Some(id), self.mailbox(), w, cx);
        }
        // A wider offline range starts downloading now; any change recounts.
        if offline != OFFLINE_WHEN_OPENED {
            self.download_account(account, cx);
        }
        self.refresh_counts(cx);
    }
    /// Opens the form for a new sending address of `account`, or for
    /// changing one (`id`).
    fn start_identity(
        &mut self,
        account: String,
        id: Option<String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = id.as_ref().and_then(|id| {
            self.accounts
                .iter()
                .find(|a| a.id == account)?
                .identity(id)
                .cloned()
        });
        let i = existing.clone().unwrap_or_default();
        let server = i.smtp.clone().unwrap_or_default();
        let values = [
            i.email.clone(),
            i.name.clone(),
            i.reply_to.clone(),
            server.host.clone(),
            if server.port == 0 {
                "587".into()
            } else {
                server.port.to_string()
            },
            server.username.clone(),
            String::new(),
        ];
        for (input, value) in self.identity_inputs.clone().iter().zip(values) {
            Self::set(input, value, w, cx);
        }
        self.identity_edit = Some(settings::IdentityEdit {
            account,
            id: existing.map(|i| i.id),
            own_server: i.smtp.is_some(),
            had_password: i.smtp.is_some(),
            saving: false,
            error: None,
        });
        let first = self.identity_inputs[0].clone();
        first.update(cx, |s, cx| s.focus(w, cx));
    }
    /// Checks and saves the sending address being edited, signing in to
    /// its own server first if it has one.
    fn save_identity(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.identity_edit.clone() else {
            return;
        };
        if edit.saving {
            return;
        }
        let value = |i: usize| Self::value(&self.identity_inputs[i], cx).trim().to_owned();
        let smtp = if edit.own_server {
            let port = match value(4).parse::<u16>() {
                Ok(p) if p > 0 => p,
                _ => {
                    self.identity_failed("Enter a port between 1 and 65535.".into(), cx);
                    return;
                }
            };
            Some(crate::manage::model::OutgoingServer {
                host: value(3),
                port,
                username: value(5),
            })
        } else {
            None
        };
        let identity = crate::manage::model::Identity {
            id: edit
                .id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            email: value(0),
            name: value(1),
            reply_to: value(2),
            smtp,
        };
        if let Err(e) = identity.validate() {
            self.identity_failed(e.to_string(), cx);
            return;
        }
        let password = Self::value(&self.identity_inputs[6], cx).to_string();
        if edit.own_server && password.is_empty() && !edit.had_password {
            self.identity_failed("Enter the password for its outgoing server.".into(), cx);
            return;
        }
        if let Some(e) = &mut self.identity_edit {
            e.saving = true;
            e.error = None;
        }
        let account = edit.account.clone();
        let password = (!password.is_empty()).then_some(password);
        self.task(
            cx,
            move |b| b.save_identity(&account, identity, password),
            move |this, result, cx| match result {
                Ok(a) => {
                    this.identity_edit = None;
                    this.account_saved(a, cx);
                    this.notice("Address saved.", false, cx);
                }
                Err(e) => this.identity_failed(format!("{e:#}"), cx),
            },
        );
    }
    fn identity_failed(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.identity_edit {
            e.saving = false;
            e.error = Some(error);
        }
        cx.notify();
    }
    fn remove_identity(&mut self, account: String, id: String, cx: &mut Context<Self>) {
        if self
            .identity_edit
            .as_ref()
            .is_some_and(|e| e.id.as_deref() == Some(id.as_str()))
        {
            self.identity_edit = None;
        }
        self.task(
            cx,
            move |b| b.remove_identity(&account, &id),
            |this, result, cx| match result {
                Ok(a) => this.account_saved(a, cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }
    /// Takes an account as the backend saved it, here and in open composers.
    fn account_saved(&mut self, a: Account, cx: &mut Context<Self>) {
        if let Some(existing) = self.accounts.iter_mut().find(|x| x.id == a.id) {
            *existing = a;
        }
        let accounts = self.accounts.clone();
        for composer in self.composers.clone() {
            composer.update(cx, |c, cx| {
                c.set_accounts(accounts.clone());
                cx.notify();
            });
        }
        cx.notify();
    }
    /// Saves an account's nickname; an empty one goes back to its name.
    /// Changes an account's settings and saves them.
    pub(super) fn edit_account(&mut self, id: &str, edit: impl FnOnce(&mut Account), cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter_mut().find(|a| a.id == id) else {
            return;
        };
        edit(a);
        if let Err(e) = self.backend.store.save_account(a) {
            self.notice(e.to_string(), true, cx);
        }
        cx.notify();
    }
    /// Asks the account's server how much of its storage is used.
    fn load_quota(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if a.provider == Provider::Pop3 || a.incoming_host.is_empty() || self.account_blocked(id) {
            return;
        }
        let id = id.to_owned();
        self.task(
            cx,
            move |b| b.quota(&a),
            move |this, result, cx| {
                if let Ok(quota) = result {
                    this.quotas.insert(id, quota);
                    cx.notify();
                }
            },
        );
    }
    pub(super) fn set_nickname(&mut self, id: &str, nickname: &str, cx: &mut Context<Self>) {
        let nickname = nickname.trim();
        let Some(a) = self.accounts.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if a.nickname == nickname {
            return;
        }
        a.nickname = nickname.to_owned();
        if let Err(e) = self.backend.store.save_account(a) {
            self.notice(e.to_string(), true, cx);
        }
        cx.notify();
    }
    /// Shows the open account page's nickname in its field, also after it
    /// was changed elsewhere, unless the field is being typed in.
    /// Applies a sign-in name being typed (see `login_pending`): saves it
    /// and signs the account's connections out, so the next sign in with it.
    fn apply_login(&mut self, cx: &mut Context<Self>) {
        let Some((id, value)) = self.login_pending.take() else {
            return;
        };
        if self.accounts.iter().any(|a| a.id == id && a.username != value) {
            self.edit_account(&id, |a| a.username = value, cx);
            self.backend.reconnect_later(&id);
        }
    }
    pub(super) fn sync_nickname_input(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let shown = match &self.page {
            Page::Account(id) => self.accounts.iter().find(|a| &a.id == id),
            _ => None,
        };
        let shown = shown.map(|a| {
            (
                a.id.clone(),
                [a.nickname.clone(), a.username.clone(), a.reply_to.clone()],
            )
        });
        // Leaving the page, or another account's: a name being typed for
        // the one shown before applies now.
        if self.login_pending.as_ref().map(|(id, _)| id) != shown.as_ref().map(|(id, _)| id) {
            self.apply_login(cx);
        }
        let Some((id, mut values)) = shown else {
            self.nickname_for = None;
            return;
        };
        let same = self.nickname_for.as_ref() == Some(&id);
        // What is being typed stays in its field until it applies.
        if same && let Some((_, typed)) = &self.login_pending {
            values[1] = typed.clone();
        }
        if !same {
            self.nickname_for = Some(id.clone());
            self.load_quota(&id, cx);
            self.load_account_extras(&id, cx);
        }
        self.populate_filter_inputs(&id, w, cx);
        let inputs = [
            self.nickname_input.clone(),
            self.login_input.clone(),
            self.reply_to_input.clone(),
        ];
        for (input, value) in inputs.iter().zip(values) {
            let current = Self::value(input, cx);
            let typing = input.read(cx).focus_handle(cx).is_focused(w);
            if !same || (current.trim() != value && !typing) {
                Self::set(input, value, w, cx);
            }
        }
    }
    fn close_search(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        if !Self::value(&self.search, cx).is_empty() {
            Self::set(&self.search, "", w, cx);
            self.filter(cx);
        }
        self.focus.focus(w, cx);
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
        // Starred and All Mail show no count: their mail is counted where it is.
        if mailbox.folder().is_virtual() {
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
        let active = self.account.as_deref() == Some(account)
            && self.mailbox() == mailbox
            && !self.subs_open;
        let count = self.unread_for(account, &mailbox);
        let entries = self.mailbox_entries(account, mailbox.clone());
        let action = Action::OpenMailbox(account.to_owned(), mailbox.clone());
        let selector = format!("nav-{account}-{}", mailbox.key());
        // A server folder is renamed in place, as top folders are.
        let rename = match &mailbox {
            Mailbox::Remote(name) => Some(FolderEdit::Rename(name.clone())),
            Mailbox::Standard(_) => None,
        };
        let editing = rename
            .as_ref()
            .is_some_and(|edit| self.editing_folder(account, edit));
        let saving = rename.as_ref().and_then(|edit| {
            self.folder_saving
                .as_ref()
                .filter(|(a, e, _)| a == account && e == edit)
                .map(|(_, _, name)| name.clone())
        });
        let label_el = if editing {
            self.rename_field()
        } else {
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
                .when(saving.is_some(), |d| d.opacity(0.6))
                .child(saving.unwrap_or(label))
                .into_any_element()
        };
        let renamable = matches!(&mailbox, Mailbox::Remote(name) if self.folder_deletable(account, name));
        let double = match &mailbox {
            Mailbox::Remote(name) if renamable => {
                Some(Action::RenameRemoteFolder(account.to_owned(), name.clone()))
            }
            _ => None,
        };
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
            .child(label_el)
            .when(count > 0, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active { c.accent } else { c.muted }))
                        .child(fmt_count(count)),
                )
            })
            .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                if editing {
                    return;
                }
                // Double-clicking a server folder renames it, as with top folders.
                match &double {
                    Some(rename) if e.click_count() >= 2 => this.handle(rename.clone(), w, cx),
                    _ => this.handle(action.clone(), w, cx),
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.open_menu(e.position, entries.clone(), cx)
                }),
            )
    }
    fn sidebar_tree(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let list = div()
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
        let mut list = self.tree_list(list, &self.sidebar_scroll, cx);
        let row = Cell::new(0);
        if self.shows_all_mail()
            && let Some(el) = self.view_block(ALL_MAIL, 0, &row, false, cx)
        {
            list = list.child(el);
        }
        for el in self.sidebar_nodes(&self.tree, 0, &row, cx) {
            list = list.child(el);
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
                .child("Add…")
                .tooltip(|w, cx| Tooltip::new("Add an account, folder or separator").build(w, cx))
                .on_click(cx.listener(|this, _, w, cx| this.handle(Action::AddMenu, w, cx))),
        );
        list = list.child(self.tree_end("sidebar-end", cx));
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
        let rail = div()
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
        let mut rail = self.tree_list(rail, &self.rail_scroll, cx);
        let row = Cell::new(0);
        if self.shows_all_mail()
            && let Some(el) = self.rail_view(ALL_MAIL, &row, false, cx)
        {
            rail = rail.child(el);
        }
        for el in self.rail_nodes(&self.tree, &row, cx) {
            rail = rail.child(el);
        }
        panes::scroll_frame(
            rail.child(
                self.rail_button(
                    "rail-add-account".into(),
                    "Add an account, folder or separator".into(),
                    icon_el("plus", 16., c.muted).into_any_element(),
                    false,
                    Action::AddMenu,
                    cx,
                )
                .mt(px(4.)),
            )
            .child(self.tree_end("rail-end", cx)),
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
                self.empty_list(cx)
            } else {
                uniform_list("message-list", count, move |range, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.want_rows(range.clone(), cx);
                        range
                            .map(|i| this.message_row(this.visible[i], cx))
                            .collect::<Vec<_>>()
                    })
                })
                .track_scroll(&self.list_scroll)
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
    /// Accounts the open mailbox is still being fetched from: a sync, a
    /// sign-in retry or a listing in flight. A view (All Accounts, an alias)
    /// lists several accounts, each fetched on its own.
    fn fetching_accounts(&self) -> Vec<&Account> {
        if self.account.is_none() || self.folder == Folder::Drafts {
            return vec![];
        }
        let key = self.mailbox().key();
        self.accounts
            .iter()
            .filter(|a| self.shows(&a.id))
            .filter(|a| {
                self.retrying.contains(&a.id)
                    || self.sync_requests.contains(&(a.id.clone(), key.clone()))
                    || self.cataloging.contains(&(a.id.clone(), key.clone()))
            })
            .collect()
    }
    /// Whether the open mailbox may still get mail: its cache is being read
    /// or one of its accounts is being fetched.
    fn mailbox_loading(&self) -> bool {
        self.syncing || self.cache_loading || !self.fetching_accounts().is_empty()
    }
    /// Whether the open mailbox is being synced with a server right now.
    /// Listing a large mailbox in the background has its own progress bar
    /// and leaves Refresh available.
    fn checking_server(&self) -> bool {
        if self.syncing {
            return true;
        }
        if self.account.is_none() || self.folder == Folder::Drafts {
            return false;
        }
        let key = self.mailbox().key();
        self.sync_requests
            .iter()
            .any(|(id, k)| k == &key && self.shows(id))
    }
    /// Shown accounts whose last fetch of the open mailbox failed, or that
    /// can't sign in.
    fn unreachable_accounts(&self) -> Vec<&Account> {
        if self.account.is_none() || self.folder == Folder::Drafts {
            return vec![];
        }
        let source = IssueSource::Sync(self.mailbox().key());
        self.accounts
            .iter()
            .filter(|a| self.shows(&a.id))
            .filter(|a| {
                self.account_blocked(&a.id)
                    || self
                        .account_issues
                        .get(&a.id)
                        .is_some_and(|issues| issues.contains_key(&source))
            })
            .collect()
    }
    /// What an empty list says: an icon (or a spinner), a title, and a
    /// line explaining it.
    fn empty_list(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let searching = !Self::value(&self.search, cx).trim().is_empty();
        let loading = self.mailbox_loading();
        let fetching = self.fetching_accounts();
        let shown = self
            .accounts
            .iter()
            .filter(|a| self.shows(&a.id))
            .count();
        let names = |list: &[&Account]| match list {
            [] => String::new(),
            [one] => one.label().to_owned(),
            [first, second] => format!("{} and {}", first.label(), second.label()),
            [first, rest @ ..] => format!("{} and {} more", first.label(), rest.len()),
        };
        let unreachable = if loading { vec![] } else { self.unreachable_accounts() };
        let (icon, title, detail): (Option<&str>, String, Option<String>) = if searching {
            if loading {
                (
                    Some("search"),
                    "No results yet".into(),
                    Some("Still fetching mail; more may match.".into()),
                )
            } else {
                (Some("search"), "No results".into(), None)
            }
        } else if loading {
            let detail = if self.cache_loading && fetching.is_empty() {
                "Reading mail saved on this computer…".to_owned()
            } else if let Some(p) = self.catalog_progress().filter(|p| p.total > 0) {
                format!(
                    "Listed {} of {} messages",
                    fmt_count(p.cataloged),
                    fmt_count(p.total)
                )
            } else if shown > 1 && !fetching.is_empty() {
                format!(
                    "Checking {} of {} accounts: {}",
                    fetching.len(),
                    shown,
                    names(&fetching)
                )
            } else if !fetching.is_empty() {
                format!("Checking {} for mail", names(&fetching))
            } else {
                "Checking the server for mail".to_owned()
            };
            (None, "Fetching mail…".into(), Some(detail))
        } else if !unreachable.is_empty() && self.filter_by == Filter::All {
            (
                Some("warning"),
                "Couldn't fetch mail".into(),
                Some(format!(
                    "{} couldn't be reached. Refresh to try again.",
                    names(&unreachable)
                )),
            )
        } else if self.filter_by != Filter::All {
            (Some("filter"), "No matching mail".into(), None)
        } else if self.folder == Folder::Drafts {
            (Some("compose"), "No drafts".into(), None)
        } else {
            (Some(folder_icon(self.folder)), "No mail here".into(), None)
        };
        let failed = icon == Some("warning");
        div()
            .id("empty-list")
            .debug_selector(|| "empty-list".into())
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .text_size(px(13.))
            .text_color(rgb(c.muted))
            .child(match icon {
                None => spinner(20., c.accent).into_any_element(),
                Some(icon) => icon_el(icon, 28., c.border).into_any_element(),
            })
            .child(
                div()
                    .id("empty-list-title")
                    .debug_selector(|| "empty-list-title".into())
                    .text_color(rgb(if loading { c.text } else { c.muted }))
                    .font_weight(if loading {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(title),
            )
            .when_some(detail, |d, detail| {
                d.child(
                    div()
                        .id("empty-list-detail")
                        .debug_selector(|| "empty-list-detail".into())
                        .max_w(px(280.))
                        .text_center()
                        .text_size(px(12.))
                        .child(detail),
                )
            })
            .when(failed, |d| {
                d.child(
                    self.button("empty-list-retry", "Refresh", "refresh", Action::Refresh, false, cx)
                        .h(px(30.))
                        .mt_1(),
                )
            })
            .into_any_element()
    }
    /// Unread messages in the open mailbox: exact from the loaded list when it
    /// holds the whole mailbox, otherwise from the cache's counts.
    fn mailbox_unread(&self) -> usize {
        if self.folder == Folder::Drafts {
            return 0;
        }
        let loaded = self.loaded_unread;
        match &self.account {
            Some(id) if !self.folder.is_virtual() => {
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
                    .when(self.folder != Folder::Drafts, |d| {
                        d.child(
                            small(self.button(
                                "select-toggle",
                                if self.selecting {
                                    "@Done selecting · Esc"
                                } else {
                                    "@Select messages · ⌘A selects all"
                                },
                                "select",
                                Action::ToggleSelectMode,
                                false,
                                cx,
                            ))
                            .when(self.selecting, |d| d.bg(rgb(c.soft))),
                        )
                    })
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
                    .child(if self.checking_server() {
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
            // Until something is typed, the fields it can search by.
            if Self::value(&self.search, cx).trim().is_empty() {
                header = header.child(self.search_hints(cx));
            }
            header = header.children(self.server_search_bar(cx));
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
    /// Chips that start a search by sender, Reply-To, state and so on.
    fn search_hints(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        div()
            .id("search-hints")
            .debug_selector(|| "search-hints".into())
            .pt_2()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(crate::manage::search::OPERATORS.iter().map(|(op, what)| {
                let op = *op;
                let what = *what;
                div()
                    .id(SharedString::from(format!("search-hint-{op}")))
                    .debug_selector(move || format!("search-hint-{op}"))
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(c.border))
                    .text_size(px(11.))
                    .text_color(rgb(c.muted))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.bg)).text_color(rgb(c.accent)))
                    .child(op)
                    .tooltip(move |w, cx| Tooltip::new(what).build(w, cx))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        // Operators that take a value wait for it; flags
                        // search straight away and leave room for more.
                        let query = if op.ends_with(':') {
                            op.to_owned()
                        } else {
                            format!("{op} ")
                        };
                        this.handle(Action::SearchFor(query), w, cx)
                    }))
            }))
    }
    fn selection_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = self.colors();
        let n = self.selection.len();
        let all = n > 0 && n >= self.selectable_count();
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
                    .id("selection-count")
                    .debug_selector(|| "selection-count".into())
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(c.accent))
                    .child(if n == 0 {
                        "Pick messages".to_owned()
                    } else {
                        format!("{} selected", fmt_count(n))
                    })
                    .when(n == 0, |d| {
                        d.tooltip(|w, cx| {
                            Tooltip::new("Click messages to select them; Shift-click selects a range")
                                .build(w, cx)
                        })
                    }),
            )
            .child(
                div()
                    .id("select-all")
                    .debug_selector(|| "select-all".into())
                    .ml_2()
                    .text_size(px(12.))
                    .text_color(rgb(c.accent))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.7))
                    .child(if all { "Deselect All" } else { "Select All" })
                    .on_click(cx.listener(|this, _, w, cx| this.handle(Action::SelectAll, w, cx))),
            )
            .child(div().flex_1())
            .when(n > 0, |d| {
                d.child(if any_unread {
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
                        "bulk-snooze",
                        "@Snooze until…",
                        "snooze",
                        Action::SnoozeMenu(vec![]),
                        false,
                        cx,
                    )
                    .h(px(28.))
                    .w(px(28.))
                    .rounded(px(7.))
                    .bg(rgb(c.soft)),
                )
            })
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
    fn message_row(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let m = &self.messages[index];
        if m.pending {
            return placeholder_row(&m.id, c).into_any_element();
        }
        let id = m.id.clone();
        let selected = self.selected.as_ref() == Some(&m.id);
        let checked = self.selection.contains(&m.id);
        let highlighted = selected || checked;
        let star = m.starred;
        let drafts = self.folder == Folder::Drafts;
        let row_group = SharedString::from(format!("row-group-{id}"));
        let lead_group = SharedString::from(format!("lead-group-{id}"));
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
        // Sent mail names the alias it went out as, in place of the address.
        let sent_alias = self.sent_alias(m);
        // A mailbox kept to its aliases sent it as the alias.
        let sender = match self.mask_for(m) {
            Some(k) if k.sent => k.sender(),
            _ => m.sender.clone(),
        };
        let address = (sent_alias.is_none()
            && !m.from.is_empty()
            && !m.from.eq_ignore_ascii_case(&m.sender))
        .then(|| m.from.clone());
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
                                    .child(sender),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .children(address)
                                    .when_some(sent_alias, |d, alias| {
                                        d.child(alias_tag(&format!("sent-alias-{id}"), alias, c))
                                    }),
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
                                    .child(self.hide_codes(m, &m.subject).into_owned()),
                            )
                            // Replied to, forwarded (here or in other apps)
                            // and snoozed.
                            .when(m.answered, |d| d.child(icon_el("answered", 13., c.muted)))
                            .when(m.forwarded, |d| d.child(icon_el("forwarded", 13., c.muted)))
                            .when(self.snoozed_until(m).is_some(), |d| {
                                d.child(icon_el("snooze", 13., c.accent))
                            })
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
                            .child(preview(&self.hide_codes(m, &m.snippet))),
                    ),
            )
            .on_mouse_down(MouseButton::Right, {
                // Built on demand: rows render often, menus open rarely.
                let id = id.clone();
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let entries = this.mail_entries(&id);
                    this.open_menu(e.position, entries, cx)
                })
            })
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
                    this.handle(Action::Select(id.clone()), w, cx);
                    // Double-clicking opens a collapsed reader.
                    if e.click_count() >= 2 && this.reader_collapsed {
                        this.set_reader_collapsed(false, cx);
                    }
                }
            }))
            .into_any_element()
    }
    /// Whether every selected message is `test`; false when none is selected.
    fn selection_all(&self, test: impl Fn(&Mail) -> bool) -> bool {
        let mut any = false;
        for m in self.messages.iter().filter(|m| self.selection.contains(&m.id)) {
            if !test(m) {
                return false;
            }
            any = true;
        }
        any
    }
    /// Listed messages that can be selected (not placeholders).
    fn selectable_count(&self) -> usize {
        self.visible
            .iter()
            .filter(|i| !self.messages[**i].pending)
            .count()
    }
    fn select_range(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        let position = |id: &str| self.visible.iter().position(|i| self.messages[*i].id == id);
        let (Some(a), Some(b)) = (position(from), position(to)) else {
            return;
        };
        let (a, b) = (a.min(b), a.max(b));
        let ids: Vec<String> = self.visible[a..=b]
            .iter()
            .filter(|i| !self.messages[**i].pending)
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
            Service::ProtonBridge | Service::Proton => "proton",
            _ => "other",
        };
        div()
            .size(px(size))
            .flex_shrink_0()
            .child(img(SharedString::from(format!("brand/{key}.svg"))).size_full())
    }
    /// The first setup step: Pinal Mail (with the `pinal` feature), then the
    /// other supported services.
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
        let others: Vec<Service> = Service::PICKER
            .iter()
            .copied()
            .filter(|s| *s != Service::Pinal)
            .collect();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .children(cfg!(feature = "pinal").then_some(pinal))
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
        // Proton's steps after the password replace the form.
        if self.service == Service::Proton
            && let Some(step) = self.proton_step_view(false, cx)
        {
            return step;
        }
        let mut content = div().flex().flex_col().gap_4();
        if matches!(self.service, Service::Proton | Service::ProtonBridge) {
            content = content.child(if self.proton_direct {
                self.proton_tabs(cx)
            } else {
                notice_box(crate::sync::proton::UNAVAILABLE, c)
            });
        }
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
                )
                .child(self.field("Sign-in name, if not the email address", LOGIN));
        }
        if let Some(why) = self.service.browser_unavailable() {
            content = content.child(notice_box(why, c));
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
                            self.link(
                                "password-help",
                                if self.service == Service::Proton {
                                    "Open your Proton account"
                                } else {
                                    "Make one now"
                                },
                                Action::OpenLink(url),
                                cx,
                            )
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icon_el(
                                "external-link",
                                12.,
                                c.accent,
                            )),
                        ),
                    ),
            );
        }
        // Proton's API has no server settings to change.
        if !self.manual && self.service != Service::Proton {
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
                    ("Open message in new window", "⌘ O"),
                    ("Reply", "⌘ ⇧ R"),
                    ("Reply all", "⌘ ⌥ R"),
                    ("Forward", "⌘ ⇧ F"),
                    ("Mark read / unread", "⌘ ⇧ U"),
                    ("Go to Inbox … All Mail", "⌘ 1 … ⌘ 8"),
                    ("Settings", "⌘ ,"),
                    ("Star message", "⌘ ⇧ S"),
                    ("Archive message", "⌘ ⇧ A"),
                    ("Select all messages", "⌘ A"),
                    ("Select range / toggle", "⇧ Click / ⌘ Click"),
                    ("Toggle sidebar", "⌃ ⌘ S"),
                    ("Move to trash", "⌘ ⌫"),
                    ("Send message", "⌘ Enter"),
                    ("Save draft", "⌘ S"),
                    ("Bold / italic / link", "⌘ B / ⌘ I / ⌘ K"),
                    ("Bulleted / numbered list", "⌘ ⇧ 8 / ⌘ ⇧ 7"),
                    ("Preview formatting", "⌘ ⇧ P"),
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
        let browser = !self.service.sign_in().typed();
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
                if self.connecting && self.sign_in_step != SignInStep::Browser {
                    "Picacho has your sign-in and is setting up the account.".into()
                } else if self.connecting {
                    "Continue in the browser window that just opened.".into()
                } else {
                    "Sign in with your browser. Picacho never sees your password.".into()
                },
            ),
            // Proton's steps after the password name themselves.
            Step::Credentials
                if self.service == Service::Proton && self.proton_heading().is_some() =>
            {
                let (title, about) = self.proton_heading().unwrap_or_default();
                (title.into(), about.into())
            }
            Step::Credentials => (
                if self.service == Service::Other {
                    "Sign in".into()
                } else {
                    format!("Sign in to {label}")
                },
                if self.manual {
                    "We couldn't find your settings. Enter them below.".into()
                } else if self.service.browser_unavailable().is_some() {
                    "Connect with an app password from your account's security settings.".into()
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
        // A fixed width (narrower windows shrink it), not a share of the
        // parent: measured as a share, the wrapped text in the form came
        // out taller than it is, leaving empty space to scroll into.
        let mut card = div()
            .w(px(width))
            .max_w_full()
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
                        .child(self.sign_in_step.label()),
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
            // A security key alone: its own buttons.
            Step::Credentials
                if self.service == Service::Proton && self.proton_submit_label().is_none() =>
            {
                None
            }
            Step::Credentials if self.sign_in_retry => Some(
                self.button("setup-retry", "Retry", "", Action::RetrySignIn, true, cx)
                    .min_w(px(120.))
                    .into_any_element(),
            ),
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
            } else if self.proton_stage.is_some() {
                // Back to the password (SetupBack leaves Proton's step).
                "Back"
            } else {
                "Choose another provider"
            }),
            _ => None,
        };
        if self.step == Step::Credentials && self.sign_in_retry && !self.connecting {
            // Retry reuses what the browser returned; this starts over.
            actions = actions.child(self.link(
                "setup-start-over",
                if browser {
                    "Open Browser Again"
                } else {
                    "Connect Again"
                },
                Action::SubmitAccount,
                cx,
            ));
        }
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
        card.child(actions.debug_selector(|| "setup-actions".into()))
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
                            .flex_col()
                            .items_center()
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
    fn popup_menu(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        // The whole menu fits in the window where it can: rows get shorter
        // when there are many, and only a menu that still doesn't fit (a
        // long folder list) scrolls.
        // The menu is placed here rather than by `anchored`'s snapping,
        // which only moves what crosses the window's edge: on a window with
        // its own decorations, a menu ending in the transparent shadow band
        // around the frame stayed put and was cut off by the frame. Its size
        // is known up front, so it is kept inside the visible frame.
        let viewport = window.viewport_size();
        let insets = window::frame_insets(window);
        let frame = viewport.height - insets.top - insets.bottom;
        let (height, row) = menu_size(&menu.entries, f32::from(frame));
        let width = menu_width(&menu.entries, window);
        let origin = menu_origin(menu.position, size(px(width), px(height)), viewport, insets);
        let mut panel = div()
            .id("menu-panel")
            .debug_selector(|| "menu-panel".into())
            // Long addresses end in an ellipsis rather than widen the menu.
            .w(px(width))
            .max_h(px(height))
            .overflow_y_scroll()
            // Scrolling the menu scrolls it, rather than closing it as a
            // scroll elsewhere does.
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .p_1()
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .shadow_lg()
            .text_size(px(13.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let any_checks = menu
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Item { icon: "", .. }));
        for (i, e) in menu.entries.iter().enumerate() {
            match e {
                Entry::Separator => {
                    panel = panel.child(
                        div()
                            .flex_shrink_0()
                            .h(px(1.))
                            .mx_2()
                            .my_1()
                            .bg(rgb(c.border)),
                    );
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
                            .flex_shrink_0()
                            .h(px(row))
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
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .pr_4()
                                    .child(label.clone()),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.menu = None;
                                this.handle(action.clone(), w, cx)
                            })),
                    );
                }
            }
        }
        // A press outside the panel closes it. A left press is consumed, like
        // a native menu; a right press goes on, so right-clicking another row
        // or folder opens its menu at once.
        let panel = panel.on_mouse_down_out(cx.listener(|this, e: &MouseDownEvent, _, cx| {
            this.menu = None;
            if e.button == MouseButton::Left {
                cx.stop_propagation();
            }
            cx.notify();
        }));
        div()
            .id("menu-backdrop")
            .absolute()
            .inset_0()
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| {
                let moved = match e.delta {
                    ScrollDelta::Pixels(d) => d.x.abs().max(d.y.abs()) > px(3.),
                    ScrollDelta::Lines(d) => d.x.abs().max(d.y.abs()) > 0.2,
                };
                let settled = this
                    .menu
                    .as_ref()
                    .is_some_and(|m| m.opened.elapsed() > Duration::from_millis(250));
                if moved && settled && !matches!(e.touch_phase, TouchPhase::Ended) {
                    this.menu = None;
                    cx.notify();
                }
            }))
            .child(
                anchored()
                    .position(origin)
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
    fn alias_gone_modal(&self, why: &str, from: &str) -> AnyElement {
        let c = self.colors();
        div()
            .id("alias-gone")
            .debug_selector(|| "alias-gone".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon_el("warning", 16., c.danger))
                    .child(why.to_owned()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "Your message wasn't sent and is back in its composer. From was reset to {from}; send it from there, or choose another address in From."
                    )),
            )
            .into_any_element()
    }
    fn setup_submit(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let busy = self.discovering || self.connecting;
        let (label, action) = if self.step == Step::Credentials {
            (
                if let Some(label) = self
                    .proton_submit_label()
                    .filter(|_| self.service == Service::Proton)
                {
                    label
                } else if self.connecting && self.sign_in_step == SignInStep::Verifying {
                    "Waiting for Proton's check…"
                } else if self.connecting {
                    "Connecting…"
                } else if !self.service.sign_in().typed() {
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
            Modal::Help => "Keyboard Shortcuts",
            Modal::RemoveAccount(_) => "Remove Account",
            Modal::DeleteFolder(..) => "Delete Folder",
            Modal::Erase(Erase::Mail(_)) => "Delete Permanently",
            Modal::Erase(Erase::Mailbox(_, Folder::Junk)) => "Empty Spam",
            Modal::Erase(Erase::Mailbox(..)) => "Empty Trash",
            Modal::NewLabel(_) => "New Label",
            Modal::Original => match self.source_view {
                SourceView::Headers => "Message Headers",
                SourceView::Html => "Message HTML",
                SourceView::Original => "Original Message",
            },
            Modal::AvatarPresets(_) => "Choose a Picture",
            Modal::AliasGone(..) => "Alias No Longer Exists",
            Modal::Snooze(ids) if ids.len() > 1 => "Snooze Messages",
            Modal::Snooze(_) => "Snooze",
        };
        let content = match modal {
            Modal::Connect => self.setup_view(cx),
            Modal::Reconnect(id) => self.reconnect_form(id, cx),
            Modal::Settings => self.settings_modal(window, cx),
            Modal::About => self.about_modal(),
            Modal::Help => self.help_modal(),
            Modal::RemoveAccount(id) => self.remove_modal(id),
            Modal::DeleteFolder(id, name) => self.delete_folder_modal(id, name),
            Modal::Erase(erase) => self.erase_modal(erase),
            Modal::NewLabel(target) => self.new_label_modal(target),
            Modal::Original => self.original_modal(cx),
            Modal::AvatarPresets(id) => self.presets_modal(id, cx),
            Modal::AliasGone(why, from) => self.alias_gone_modal(why, from),
            Modal::Snooze(_) => self.snooze_modal(),
        };
        // Dialogs are laid out inside the window's frame, which is smaller
        // than the window while Picacho draws its own decorations.
        let frame = window::content_size(window);
        let available_height = (frame.height - px(32.)).max(px(0.));
        let available_width = (frame.width - px(32.)).max(px(0.));
        let mut panel = div()
            .id("modal-panel")
            .debug_selector(|| "modal-panel".into())
            .flex()
            .flex_col()
            .w(px(match modal {
                Modal::Connect => 680.,
                Modal::Help => 460.,
                Modal::AvatarPresets(_) => 560.,
                Modal::Snooze(_) => 600.,
                Modal::RemoveAccount(_)
                | Modal::DeleteFolder(..)
                | Modal::Erase(_)
                | Modal::NewLabel(_)
                | Modal::Reconnect(_)
                | Modal::About
                | Modal::AliasGone(..) => 400.,
                Modal::Settings | Modal::Original => 820.,
            }))
            .max_w(available_width)
            .when(*modal == Modal::Settings, |d| {
                if self.settings_place == SettingsPlace::Maximized {
                    // Like a full-size composer: below the title bar, which
                    // stays visible and usable, with an even margin all round.
                    let edge = px(2. * MAXIMIZED_MARGIN);
                    d.w((frame.width - edge).max(px(0.)))
                        .h((frame.height - px(TITLEBAR_HEIGHT) - edge).max(px(0.)))
                } else {
                    d.h(px(600.).min(available_height))
                }
            })
            .when(*modal == Modal::Original, |d| {
                d.h(px(760.).min(available_height))
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
                    .when(*modal == Modal::Settings, |d| {
                        d.child(self.settings_place_controls(cx))
                    })
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
                // Settings scrolls its own sections beside a fixed sidebar;
                // the original source scrolls its own lines.
                Modal::Settings | Modal::Original => div()
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
        if matches!(modal, Modal::Connect | Modal::Settings | Modal::Snooze(_)) {
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
        } else if let Modal::DeleteFolder(id, name) = modal {
            let deleting = self.deleting_folders.contains(&(id.clone(), name.clone()));
            footer = footer
                .child(div().flex_1())
                .child(self.button("delete-folder-cancel", "Cancel", "", Action::Close, false, cx))
                .child(
                    self.button(
                        "delete-folder-confirm",
                        if deleting { "Deleting…" } else { "Delete" },
                        "",
                        Action::ConfirmDeleteFolder(id.clone(), name.clone()),
                        true,
                        cx,
                    )
                    .bg(rgb(c.danger))
                    .when(deleting, |d| d.opacity(0.7)),
                );
        } else if let Modal::Erase(erase) = modal {
            footer = footer
                .child(div().flex_1())
                .child(self.button("erase-cancel", "Cancel", "", Action::Close, false, cx))
                .child(
                    self.button(
                        "erase-confirm",
                        match erase {
                            Erase::Mail(_) => "Delete",
                            Erase::Mailbox(_, Folder::Junk) => "Empty Spam",
                            Erase::Mailbox(..) => "Empty Trash",
                        },
                        "",
                        Action::ConfirmErase(erase.clone()),
                        true,
                        cx,
                    )
                    .bg(rgb(c.danger)),
                );
        } else if matches!(modal, Modal::NewLabel(_)) {
            footer = footer
                .child(div().flex_1())
                .child(self.button("new-label-cancel", "Cancel", "", Action::Close, false, cx))
                .child(self.button("new-label-save", "Add Label", "", Action::SaveLabel, true, cx));
        } else if let Modal::Reconnect(id) = modal {
            footer = footer.child(div().flex_1());
            if self.sign_in_retry && !self.connecting {
                footer = footer.child(self.button(
                    "reconnect-retry",
                    "Retry",
                    "",
                    Action::RetrySignIn,
                    false,
                    cx,
                ));
            }
            if self.proton_reconnecting().is_some() && self.proton_stage.is_some() {
                footer = footer.child(self.button(
                    "proton-back",
                    if self.connecting { "Cancel" } else { "Back" },
                    "",
                    Action::ProtonBack,
                    false,
                    cx,
                ));
            }
            let proton = self
                .proton_reconnecting()
                .and_then(|_| self.proton_submit_label().map(|l| (l, true)))
                .or_else(|| self.proton_reconnecting().map(|_| ("", false)));
            if proton == Some(("", false)) {
                // A security key alone: its own buttons.
            } else if let Some((label, _)) = proton {
                footer = footer.child(self.button(
                    "reconnect-submit",
                    label,
                    "",
                    Action::SubmitReconnect(id.clone()),
                    true,
                    cx,
                ));
            } else {
            footer = footer.child(self.button(
                "reconnect-submit",
                if !self.connecting {
                    "Sign In"
                } else if self.sign_in_step == SignInStep::Browser {
                    "Signing in…"
                } else {
                    self.sign_in_step.label()
                },
                "",
                Action::SubmitReconnect(id.clone()),
                true,
                cx,
            ));
            }
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
        let below_titlebar =
            self.modal == Some(Modal::Settings) && self.settings_place == SettingsPlace::Maximized;
        div()
            .occlude()
            .on_mouse_down(MouseButton::Left, swallow_press)
            .absolute()
            .inset_0()
            .when(below_titlebar, |d| d.top(px(TITLEBAR_HEIGHT)))
            .rounded_tl(corners.top_left)
            .rounded_tr(corners.top_right)
            .rounded_bl(corners.bottom_left)
            .rounded_br(corners.bottom_right)
            .bg(rgba(0x101c2c66))
            .flex()
            .items_center()
            .justify_center()
            .p(px(if below_titlebar {
                MAXIMIZED_MARGIN
            } else {
                16.
            }))
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
        self.apply_reveal(window, cx);
        self.apply_pending_place(window, cx);
        self.apply_pending_new_alias(window, cx);
        self.sync_nickname_input(window, cx);
        SPINNING.with(|s| s.set(false));
        let root = self.root(window, cx);
        // The email's images, once the reader no longer paints it.
        self.html_view.update(cx, |view, _| view.release(window));
        if SPINNING.with(|s| s.get()) && !self.spin_scheduled {
            self.spin_scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SPIN_STEP).await;
                let _ = this.update(cx, |this, cx| {
                    this.spin_scheduled = false;
                    cx.notify();
                });
            })
            .detach();
        }
        root
    }
}
impl MailApp {
    fn root(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        // A drag only lasts while the button is held: a move without it
        // means the release was missed, and the drag would otherwise stay,
        // swallowing clicks, until the next one.
        let this = cx.weak_entity();
        let stray_drags = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && cx.has_active_drag() && !e.dragging() {
                        let _ = this.update(cx, |this, cx| this.cancel_stray_drag(window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        div()
            .id("mail-app")
            .child(stray_drags)
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
                if this.modal.is_some() {
                } else if !this.selection.is_empty() {
                    let star = !this.selection_all(|m| m.starred);
                    this.handle(Action::ApplySelection(Change::Star(star)), w, cx)
                } else if let Some(id) = this.selected.clone() {
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
            .on_action(cx.listener(|this, _: &ShowAbout, w, cx| this.handle(Action::About, w, cx)))
            .on_action(
                cx.listener(|this, _: &AddAccount, w, cx| this.handle(Action::Connect, w, cx)),
            )
            .on_action(cx.listener(|this, _: &ReplyAll, w, cx| this.new_compose(2, w, cx)))
            .on_action(cx.listener(|this, _: &ForwardMail, w, cx| this.new_compose(3, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleRead, w, cx| {
                if this.modal.is_some() {
                } else if !this.selection.is_empty() {
                    let read = !this.selection_all(|m| !m.unread);
                    this.handle(Action::ApplySelection(Change::Read(read)), w, cx)
                } else if let Some(id) = this.selected.clone() {
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
            .on_action(cx.listener(|this, _: &GoAllMail, w, cx| this.go(Folder::All, w, cx)))
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
            .on_action(cx.listener(|this, _: &OpenInWindow, w, cx| {
                if this.modal.is_none()
                    && let Some(id) = this.selected.clone()
                {
                    this.handle(Action::OpenWindow(id), w, cx)
                }
            }))
            .on_action(cx.listener(|_, _: &Minimize, w, _| w.minimize_window()))
            .on_action(cx.listener(|_, _: &Zoom, w, _| w.zoom_window()))
            .when(self.onboarding, |d| d.child(self.onboarding(window, cx)))
            .when(!self.onboarding, |d| self.main(d, window, cx))
            .map(|d| self.track_resize(d, cx))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ComposerDrag>, w, cx| {
                this.drag_composer(e, w, cx)
            }))
            .when(!self.onboarding, |d| {
                d.children(self.composer_layer(window, cx))
            })
            .when(self.modal.is_some(), |d| d.child(self.modal(window, cx)))
            .when(
                self.menu.as_ref().is_some_and(|m| m.window.is_none()),
                |d| d.child(self.popup_menu(window, cx)),
            )
    }
}
impl MailApp {
    fn main(&self, d: Stateful<Div>, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = self.colors();
        // The frame's width: in a window drawing its own decorations, the
        // window's is wider by its shadow band and borders, and panes sized
        // to it ran past the frame, cutting off the reader's right edge.
        let width = window::content_size(window).width;
        let single = width < px(900.);
        let compact = width < px(1180.);
        self.compact.set(compact);
        // Panes that paint their own background round the window's lower
        // corners they touch; GPUI clips children only to rectangles.
        let corners = window::window_corners(window);
        let show_sidebar = !(single && self.selected.is_some() && width < px(480.));
        // Subscriptions list senders in the middle and one sender's mail
        // (or a message of it) on the right.
        let open = if self.subs_open {
            self.subs_sender.is_some()
        } else {
            self.selected.is_some()
        };
        let show_list = !single || !open;
        let show_reader = !single || open;
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
                        d.child(if self.subs_open {
                            self.subscription_list(single, list_corners, cx)
                                .into_any_element()
                        } else {
                            self.message_list(single, list_corners, cx).into_any_element()
                        })
                    })
                    .when(show_reader && resizable, |d| {
                        d.child(self.resize_handle(Pane::Reader, cx))
                    })
                    .when(show_reader, |d| {
                        d.child(if self.subs_open {
                            // A collapsed reader still gives a sender's mail room.
                            let room = if self.reader_collapsed {
                                (f32::from(width) - layout.sidebar - LIST_MIN).max(panes::READER_MIN)
                            } else {
                                layout.reader
                            };
                            self.subscription_mail_pane(
                                resizable.then_some(room),
                                reader_corners,
                                single,
                                window,
                                cx,
                            )
                        } else {
                            self.reader_pane(
                                resizable.then_some(layout.reader),
                                reader_corners,
                                window,
                                cx,
                            )
                        })
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
/// Ends a left press on a layer drawn over the app (dialogs, floating
/// composers) once its own controls have had it. Selectable text views
/// check only their bounds, not what covers them, so without this a press
/// on the layer started a text selection in the email underneath.
fn swallow_press(_: &MouseDownEvent, _: &mut Window, cx: &mut App) {
    cx.stop_propagation()
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
thread_local! {
    /// Set while a render draws a spinner, so the app keeps ticking it.
    static SPINNING: Cell<bool> = const { Cell::new(false) };
    static SPIN_EPOCH: std::time::Instant = std::time::Instant::now();
}
/// Spinners turn in steps on a slow clock instead of on every display frame:
/// each frame redraws the whole window, so a vsync spinner kept the CPU busy
/// for as long as a sync or download ran.
const SPIN_STEP: Duration = Duration::from_millis(70);
const SPIN_STEPS: u128 = 14;
/// The indexes of `messages` that `keep` keeps, in order. With `parallel`
/// and many messages, they are checked on several threads at once, each a
/// part of the list in turn. Searching text in every listed message is
/// bound by reading their strings from memory (about 10 ms for 68,000 on
/// one core, on each keystroke); eight cores do it in about 2.
fn matching_rows(messages: &[Mail], parallel: bool, keep: impl Fn(&Mail) -> bool + Sync) -> Vec<usize> {
    const PART: usize = 4096;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    if !parallel || threads < 2 || messages.len() < 2 * PART {
        return (0..messages.len()).filter(|&i| keep(&messages[i])).collect();
    }
    let size = messages.len().div_ceil(threads).max(PART);
    let serial = || (0..messages.len()).filter(|&i| keep(&messages[i])).collect();
    let keep = &keep;
    let found: Option<Vec<usize>> = std::thread::scope(|scope| {
        let parts: Vec<_> = messages
            .chunks(size)
            .enumerate()
            .map(|(n, part)| {
                scope.spawn(move || {
                    let start = n * size;
                    (0..part.len()).filter(|&i| keep(&part[i])).map(|i| start + i).collect::<Vec<_>>()
                })
            })
            .collect();
        let mut rows = Vec::new();
        for part in parts {
            rows.extend(part.join().ok()?);
        }
        Some(rows)
    });
    // A part that failed: filter the list here, as without threads.
    found.unwrap_or_else(serial)
}
fn spinner(size: f32, color: u32) -> impl IntoElement {
    SPINNING.with(|s| s.set(true));
    let step = SPIN_EPOCH.with(|e| e.elapsed().as_millis() / SPIN_STEP.as_millis()) % SPIN_STEPS;
    svg()
        .path("icons/loader-circle.svg")
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
        .with_transformation(Transformation::rotate(percentage(
            step as f32 / SPIN_STEPS as f32,
        )))
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
        Folder::All => "all-mail",
    }
}
/// A text field on the app's background. Inputs otherwise take the theme's
/// background, which is transparent on Linux so `Root` does not paint square
/// corners under the rounded window frame. Disabled fields keep the theme's
/// muted fill.
fn field(input: Input, c: Colors, disabled: bool) -> Input {
    input.when(!disabled, |i| i.bg(rgb(c.bg)))
}
/// A multi-line [`field`].
fn textarea_field(input: Textarea, c: Colors, disabled: bool) -> Textarea {
    input.when(!disabled, |i| i.bg(rgb(c.bg)))
}
/// A message's preview as the list shows it. Previews are cleaned when
/// made; any that still carry a character reference or invisible padding
/// (cached by an older version, say) are cleaned here.
fn preview(snippet: &str) -> String {
    let clean = snippet.is_ascii() && !snippet.contains('&');
    if clean {
        snippet.to_owned()
    } else {
        crate::sync::mime::snippet(snippet)
    }
}
fn icon_el(name: &str, size: f32, color: u32) -> Div {
    let path = if [
        "logo",
        "mail",
        "compose",
        "send",
        "clock",
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
        "image",
        "image-off",
        "code",
        "text",
        "formatted",
        "bold",
        "italic",
        "underline",
        "strike",
        "list",
        "list-ordered",
        "quote",
        "link",
        "heading",
        "save",
        "popout",
        "expand",
        "shrink",
        "dock",
        "undock",
        "preview",
        "format",
        "contact",
        "thread",
        "upload",
        "cloud",
        "grid",
        "alias",
        "inboxes",
        "all-mail",
        "sparkle",
        "pin",
        "ban",
        "subscriptions",
        "tag",
        "snooze",
        "answered",
        "forwarded",
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
/// The space a popup menu keeps from the window frame, each side.
const MENU_MARGIN: f32 = 8.;
/// A menu's narrowest and widest.
const MENU_WIDTH: (f32, f32) = (200., 380.);
/// A menu's width: its longest label's, between `MENU_WIDTH`'s bounds.
fn menu_width(entries: &[Entry], window: &Window) -> f32 {
    // Around the label: the border and padding (1 + 4 each side), a row's
    // padding (8 each side), its icon and gap (16 + 8), and the space after
    // the label (16); a pixel more so rounding never cuts a label short.
    const CHROME: f32 = 2. * (1. + 4. + 8.) + 16. + 8. + 16. + 1.;
    let font = font(".SystemUIFont");
    let widest = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item { label, .. } => Some(label),
            Entry::Separator => None,
        })
        .map(|label| {
            let run = TextRun {
                len: label.len(),
                font: font.clone(),
                color: black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(window.text_system().shape_line(label.clone(), px(13.), &[run], None).width)
        })
        .fold(0., f32::max);
    (widest + CHROME).clamp(MENU_WIDTH.0, MENU_WIDTH.1)
}
/// Where a menu of `size` opened at `at` goes: there, moved only as far as
/// it takes to keep `MENU_MARGIN` inside the window's visible frame
/// (`insets` inside the `viewport`). A menu too big for it keeps its top
/// left corner in view.
fn menu_origin(
    at: Point<Pixels>,
    size: Size<Pixels>,
    viewport: Size<Pixels>,
    insets: Edges<Pixels>,
) -> Point<Pixels> {
    let margin = px(MENU_MARGIN);
    let (left, top) = (insets.left + margin, insets.top + margin);
    let right = viewport.width - insets.right - margin;
    let bottom = viewport.height - insets.bottom - margin;
    point(
        at.x.min(right - size.width).max(left),
        at.y.min(bottom - size.height).max(top),
    )
}
/// A popup menu's largest height and its rows' height, for a window frame
/// `viewport` tall: the menu fits when it can, with rows from 28 down to 22
/// points high; beyond that it scrolls.
fn menu_size(entries: &[Entry], viewport: f32) -> (f32, f32) {
    // The frame margin it keeps (each side), and its padding and border.
    const CHROME: f32 = 2. * MENU_MARGIN + 10.;
    const SEPARATOR: f32 = 9.;
    let items = entries.iter().filter(|e| matches!(e, Entry::Item { .. })).count() as f32;
    let separators = entries.len() as f32 - items;
    let available = (viewport - CHROME).max(120.);
    let row = if items == 0. {
        28.
    } else {
        ((available - separators * SEPARATOR) / items).clamp(22., 28.)
    };
    let needed = items * row + separators * SEPARATOR + 10.;
    (needed.min(available + 10.), row)
}
/// The alias a sent message went out as: its icon and address.
fn alias_tag(id: &str, alias: String, c: Colors) -> Stateful<Div> {
    let tip = format!("Sent as {alias}");
    let selector = id.to_owned();
    div()
        .id(SharedString::from(id.to_owned()))
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .text_color(rgb(c.accent))
        .child(icon_el("alias", 11., c.accent))
        .child(div().min_w_0().truncate().child(alias))
        .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
}
/// A message the server has that is not listed yet: the row's shape, so the
/// list keeps its length and scrollbar while headers load.
fn placeholder_row(id: &str, c: Colors) -> Stateful<Div> {
    let bar = |width: f32, height: f32| {
        div()
            .w(relative(width))
            .h(px(height))
            .rounded(px(height / 2.))
            .bg(rgb(c.soft))
    };
    div()
        .id(SharedString::from(format!("row-{id}")))
        .debug_selector(|| "placeholder-row".into())
        .h(px(ROW_HEIGHT))
        .w_full()
        .pl(px(14.))
        .pr(px(18.))
        .py(px(11.))
        .border_b_1()
        .border_color(rgb(c.border))
        .flex()
        .items_start()
        .gap(px(12.))
        .bg(rgb(c.surface))
        .child(
            div()
                .size(px(36.))
                .flex_shrink_0()
                .rounded_full()
                .bg(rgb(c.soft)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(9.))
                .pt(px(4.))
                .child(bar(0.38, 10.))
                .child(bar(0.72, 9.))
                .child(bar(0.55, 9.)),
        )
}
/// A quiet, tinted note that explains a limitation before the form asks
/// for something, e.g. why only an app password is offered.
fn notice_box(text: &'static str, c: Colors) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .p_3()
        .rounded(px(8.))
        .bg(rgb(c.soft))
        .text_size(px(12.))
        .child(div().pt(px(1.)).child(icon_el("info", 14., c.accent)))
        .child(div().flex_1().min_w_0().child(text))
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
    use chrono::Datelike;
    let Some(date) = chrono::DateTime::from_timestamp(ts, 0) else {
        return String::new();
    };
    let now = chrono::Local::now();
    let local = date.with_timezone(&chrono::Local);
    if local.date_naive() == now.date_naive() {
        local.format("%-I:%M %p").to_string()
    } else if local.year() == now.year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%b %-d, %Y").to_string()
    }
}
#[cfg(test)]
#[::core::prelude::v1::test]
fn short_dates_name_the_year_only_when_it_differs() {
    use chrono::{Datelike, TimeZone};
    let now = chrono::Local::now();
    let at = |y: i32, m: u32| {
        chrono::Local
            .with_ymd_and_hms(y, m, 15, 12, 0, 0)
            .unwrap()
            .timestamp()
    };
    let earlier_month = if now.month() == 1 { 1 } else { now.month() - 1 };
    let this_year = date_short(at(now.year(), earlier_month));
    assert!(!this_year.contains(&now.year().to_string()), "{this_year}");
    let last_year = date_short(at(now.year() - 1, 6));
    assert!(
        last_year.ends_with(&format!(", {}", now.year() - 1)),
        "{last_year}"
    );
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

/// A key binding whose `secondary-` modifier is Command on macOS and
/// Control elsewhere, as the running platform reports. (`KeyBinding::new`
/// no longer resolves `secondary-`, and panics on it.)
pub(super) fn key<A: gpui::Action>(
    cx: &App,
    keystrokes: &str,
    action: A,
    context: Option<&str>,
) -> KeyBinding {
    let context = context.map(|c| {
        std::rc::Rc::new(KeyBindingContextPredicate::parse(c).expect("a valid key context"))
    });
    KeyBinding::load(keystrokes, Box::new(action), context, false, None, cx)
        .expect("a valid keystroke")
}

/// The main and detached windows' key bindings.
fn app_bindings(cx: &App) -> Vec<KeyBinding> {
    vec![
        key(cx, "secondary-q", Quit, Some("Mail")),
        key(cx, "secondary-w", CloseWindow, Some("Mail")),
        key(cx, "secondary-,", ShowAccounts, Some("Mail")),
        key(cx, "secondary-n", Compose, Some("Mail")),
        key(cx, "secondary-f", Search, Some("Mail")),
        key(cx, "secondary-r", Refresh, Some("Mail")),
        key(cx, "escape", Close, Some("Mail")),
        key(cx, "alt-down", Next, Some("Mail")),
        key(cx, "alt-up", Previous, Some("Mail")),
        key(cx, "secondary-shift-a", Archive, Some("Mail")),
        key(cx, "secondary-backspace", DeleteMail, Some("Mail")),
        key(cx, "secondary-shift-r", Reply, Some("Mail")),
        key(cx, "secondary-alt-r", ReplyAll, Some("Mail")),
        key(cx, "secondary-shift-f", ForwardMail, Some("Mail")),
        key(cx, "secondary-shift-u", ToggleRead, Some("Mail")),
        key(cx, "secondary-shift-j", MoveToSpam, Some("Mail")),
        key(cx, "secondary-shift-s", ToggleStar, Some("Mail")),
        key(cx, "secondary-shift-l", ToggleTheme, Some("Mail")),
        key(cx, "secondary-enter", SendMail, Some("Mail")),
        key(cx, 
            if cfg!(target_os = "macos") {
                "ctrl-cmd-s"
            } else {
                "ctrl-alt-s"
            },
            ToggleSidebar,
            Some("Mail"),
        ),
        key(cx, "secondary-a", SelectAllMail, Some("Mail")),
        key(cx, "secondary-1", GoInbox, Some("Mail")),
        key(cx, "secondary-2", GoStarred, Some("Mail")),
        key(cx, "secondary-3", GoSent, Some("Mail")),
        key(cx, "secondary-4", GoDrafts, Some("Mail")),
        key(cx, "secondary-5", GoArchive, Some("Mail")),
        key(cx, "secondary-6", GoSpam, Some("Mail")),
        key(cx, "secondary-7", GoTrash, Some("Mail")),
        key(cx, "secondary-8", GoAllMail, Some("Mail")),
        key(cx, "secondary-m", Minimize, Some("Mail")),
        key(cx, "secondary-o", OpenInWindow, Some("Mail")),
        key(cx, "secondary-w", CloseWindow, Some("Detached")),
        key(cx, "escape", Close, Some("Detached")),
        key(cx, "cmd-h", Hide, None),
        key(cx, "alt-cmd-h", HideOthers, None),
    ]
}

/// Runs the app on this system's own GPUI platform.
#[cfg(not(target_os = "android"))]
pub fn run(store: Arc<Store>) {
    run_on(gpui_platform::application(), store);
}

/// Runs the app on `app`'s platform (Android's comes from its activity).
pub fn run_on(app: Application, store: Arc<Store>) {
    let app = app.with_assets(crate::display::assets::Assets);
    app.on_open_urls(crate::sync::oauth::opened);
    super::desktop::register();
    app.run(move |cx| {
        gpui_component::init(cx);
        prompt::install(cx);
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.set_menus(menus());
        // `secondary` is Command on macOS and Control elsewhere.
        let bindings = composer::bindings(cx);
        cx.bind_keys(bindings);
        let bindings = app_bindings(cx);
        cx.bind_keys(bindings);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                // Android's window is the whole screen.
                window_bounds: (!cfg!(target_os = "android"))
                    .then_some(WindowBounds::Windowed(bounds)),
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
                app_id: Some(super::desktop::APP_ID.into()),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| MailApp::new(store, window, cx));
                { let focus = view.read(cx).focus.clone(); focus.focus(window, cx); }
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
        cx.on_window_closed(|cx, _| {
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
        disabled: false,
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
                MenuItem::action("All Mail", GoAllMail),
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

/// The open message's details, read from its original source.
struct MessageInfo {
    id: String,
    /// None while loading.
    result: Option<Result<Rc<Original>, String>>,
}
/// A message's original source, prepared for display.
pub(super) struct Original {
    details: crate::sync::mime::Details,
    raw: Vec<u8>,
    /// The whole source, its header block, and its decoded HTML part (if
    /// it has one), each with its display lines, wrapped so every row has
    /// the same height.
    source: (SharedString, Vec<SharedString>),
    headers: (SharedString, Vec<SharedString>),
    html: Option<(SharedString, Vec<SharedString>)>,
}
impl Original {
    const WRAP: usize = 110;
    fn new(details: crate::sync::mime::Details, raw: &[u8]) -> Self {
        let text = String::from_utf8_lossy(raw).into_owned();
        // Headers end at the first blank line.
        let end = [text.find("\r\n\r\n"), text.find("\n\n")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(text.len());
        let headers = text[..end].to_owned();
        let html = mailparse::parse_mail(raw).ok().and_then(|p| {
            let mut leaves = Vec::new();
            crate::sync::mime::collect_leaves(&p, &mut leaves);
            leaves
                .iter()
                .find(|l| l.ctype.mimetype.eq_ignore_ascii_case("text/html"))
                .and_then(|l| l.get_body().ok())
        });
        Self {
            details,
            raw: raw.to_vec(),
            headers: Self::prepare(headers),
            html: html.map(Self::prepare),
            source: Self::prepare(text),
        }
    }
    fn prepare(text: String) -> (SharedString, Vec<SharedString>) {
        let mut lines = Vec::new();
        for line in text.lines() {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                lines.push(SharedString::default());
            }
            for chunk in chars.chunks(Self::WRAP) {
                lines.push(chunk.iter().collect::<String>().into());
            }
        }
        (text.into(), lines)
    }
    /// A view's text and lines; `None` for HTML when there is none.
    pub(super) fn view(&self, view: SourceView) -> Option<(&SharedString, &Vec<SharedString>)> {
        let (text, lines) = match view {
            SourceView::Headers => &self.headers,
            SourceView::Html => self.html.as_ref()?,
            SourceView::Original => &self.source,
        };
        Some((text, lines))
    }
}
trait IfEmpty {
    fn if_empty<'a>(&'a self, other: &'a str) -> &'a str;
}
impl IfEmpty for str {
    fn if_empty<'a>(&'a self, other: &'a str) -> &'a str {
        if self.is_empty() { other } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` exports a `test` macro that would shadow the built-in one.
    use std::prelude::v1::test;

    #[test]
    fn original_splits_headers_and_html() {
        let raw = b"From: a@example.com\r\nSubject: Hi\r\nContent-Type: multipart/alternative; boundary=X\r\n\r\n--X\r\nContent-Type: text/plain\r\n\r\nHello\r\n--X\r\nContent-Type: text/html\r\n\r\n<p>Hello</p>\r\n--X--\r\n";
        let o = Original::new(Default::default(), raw);
        let (headers, _) = o.view(SourceView::Headers).unwrap();
        assert!(headers.starts_with("From: a@example.com"));
        assert!(headers.contains("boundary=X"));
        assert!(!headers.contains("Hello"));
        let (html, _) = o.view(SourceView::Html).unwrap();
        assert_eq!(html.trim(), "<p>Hello</p>");
        let (source, _) = o.view(SourceView::Original).unwrap();
        assert_eq!(source.as_bytes(), raw);
        let plain = Original::new(Default::default(), b"Subject: Hi\r\n\r\nJust text\r\n");
        assert!(plain.view(SourceView::Html).is_none());
    }

    #[gpui::test]
    fn every_key_binding_parses(cx: &mut TestAppContext) {
        // Launch binds these; a keystroke gpui can't parse panics there.
        cx.update(|cx| {
            assert!(!app_bindings(cx).is_empty());
            assert!(!composer::bindings(cx).is_empty());
        });
    }

    pub(super) fn open(
        store: Arc<Store>,
        cx: &mut TestAppContext,
    ) -> (Entity<MailApp>, &mut VisualTestContext) {
        // Emails lay out on their own threads (html/worker.rs), whose
        // updates wake the window from outside the test scheduler.
        cx.executor().allow_parking();
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
    /// Filtering on several threads finds exactly what one thread does, in
    /// the list's order, at sizes around where it starts.
    #[test]
    fn threaded_filtering_finds_the_same_rows_in_order() {
        for n in [0, 10, 8191, 8192, 8193, 30_001] {
            let messages: Vec<Mail> = (0..n)
                .map(|i| Mail { id: i.to_string(), subject: format!("s{}", i % 97), unread: i % 3 == 0, ..Mail::default() })
                .collect();
            let keep = |m: &Mail| m.unread && m.subject.ends_with('7');
            let expected: Vec<usize> = (0..n).filter(|&i| keep(&messages[i])).collect();
            assert_eq!(matching_rows(&messages, true, keep), expected, "{n} messages");
            assert_eq!(matching_rows(&messages, false, keep), expected, "{n} messages");
        }
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
    fn proton_is_bridge_alone_without_directs_registration(cx: &mut TestAppContext) {
        let (view, cx) = open(store(), cx);
        act(&view, Action::Start, cx);
        view.update(cx, |this, _| this.proton_direct = false);
        act(&view, Action::ChooseService(Service::Proton), cx);
        view.read_with(cx, |this, _| {
            assert!(this.step == Step::Credentials);
            assert_eq!(this.service, Service::ProtonBridge);
        });
        // The Direct tab can't be chosen either.
        act(&view, Action::ProtonMode(Service::Proton), cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.service, Service::ProtonBridge)
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
        assert_eq!(
            cx.debug_bounds("service-Pinal").is_some(),
            cfg!(feature = "pinal"),
            "Pinal Mail is offered only with the pinal feature"
        );
        // Proton Mail is one item; Bridge is a tab inside it.
        assert!(cx.debug_bounds("service-Proton").is_some());
        assert!(cx.debug_bounds("service-ProtonBridge").is_none());
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
        // Proton Mail: Direct (Proton's API) first, with Bridge a tab away.
        act(&view, Action::SetupBack, cx);
        act(&view, Action::ChooseService(Service::Proton), cx);
        type_email(&view, "me@proton.me", cx);
        frame(cx);
        assert!(cx.debug_bounds("proton-mode-0").is_some());
        assert!(cx.debug_bounds("proton-mode-1").is_some());
        view.read_with(cx, |this, cx| {
            assert!(this.step == Step::Credentials && !this.manual);
            let a = this.setup_account(cx);
            assert_eq!(a.provider, Provider::Proton);
            assert_eq!(a.service, Service::Proton);
            assert!(!a.incoming_starttls && !a.oauth);
            assert!(a.validate().is_ok());
        });
        // No password: stopped here, before Proton is asked anything.
        act(&view, Action::SubmitAccount, cx);
        view.read_with(cx, |this, _| assert!(!this.connecting && this.error));
        act(&view, Action::ProtonMode(Service::ProtonBridge), cx);
        view.read_with(cx, |this, cx| {
            assert_eq!(this.service, Service::ProtonBridge);
            assert_eq!(MailApp::value(&this.form[EMAIL], cx), "me@proton.me");
            let a = this.setup_account(cx);
            assert!(a.incoming_starttls && a.incoming_port == 1143 && a.smtp_port == 1025);
            assert!(a.validate().is_ok());
        });
        act(&view, Action::ProtonMode(Service::Proton), cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.service, Service::Proton);
            assert!(this.step == Step::Credentials);
        });
        // The steps after the password replace the form, and Back leaves them.
        view.update(cx, |this, _| {
            this.proton_stage = Some(ProtonStage::SecondFactor {
                code: true,
                key: true,
                pin: false,
            })
        });
        frame(cx);
        // The step names itself in the card's title, and the setup's own
        // link goes back to the password.
        view.read_with(cx, |this, _| {
            assert_eq!(
                this.proton_heading().map(|h| h.0),
                Some("Two-factor authentication")
            )
        });
        let first = KeyMethod::offered()[0];
        assert!(cx.debug_bounds(format!("proton-key-{first:?}").leak()).is_some());
        view.read_with(cx, |this, _| assert_eq!(this.proton_submit_label(), Some("Verify")));
        // With a key alone, the key's buttons are the way on.
        view.update(cx, |this, _| {
            this.proton_stage = Some(ProtonStage::SecondFactor {
                code: false,
                key: true,
                pin: false,
            })
        });
        view.read_with(cx, |this, _| assert_eq!(this.proton_submit_label(), None));
        act(&view, Action::SetupBack, cx);
        view.read_with(cx, |this, _| {
            assert!(this.proton_stage.is_none());
            assert!(this.step == Step::Credentials, "Back from a step returns to the password");
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
            offline_days: 365,
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
        assert_eq!(
            cx.debug_bounds("service-Pinal").is_some(),
            cfg!(feature = "pinal")
        );
        assert!(cx.debug_bounds("service-Gmail").is_some());
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
                        nestable: true,
                    }],
                )
                .unwrap();
        }
        let (view, cx) = open(store.clone(), cx);
        (view, cx, store)
    }
    /// Test windows draw before dispatching input, so a no-op mouse move
    /// brings the rendered frame up to date.
    /// Draws a fresh frame. The app no longer redraws on its own while idle,
    /// so ask for one before moving the pointer.
    pub(super) fn frame(cx: &mut VisualTestContext) {
        cx.update(|w, _| w.refresh());
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
    fn only_server_folders_offer_deleting(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
        let labels = |mailbox: Mailbox, cx: &mut VisualTestContext| {
            view.read_with(cx, |this, _| {
                this.mailbox_entries(&id, mailbox)
                    .into_iter()
                    .filter_map(|e| match e {
                        Entry::Item { label, .. } => Some(label.to_string()),
                        Entry::Separator => None,
                    })
                    .collect::<Vec<_>>()
            })
        };
        let receipts = labels(Mailbox::Remote("INBOX/Receipts".into()), cx);
        assert_eq!(receipts.last().map(String::as_str), Some("Delete Folder…"));
        for standard in [Folder::Inbox, Folder::Sent, Folder::Trash] {
            assert!(!labels(standard.into(), cx).contains(&"Delete Folder…".to_owned()));
        }
        // Choosing it asks in the app's own dialog; Cancel changes nothing.
        act(&view, Action::DeleteRemoteFolder(id.clone(), "INBOX/Receipts".into()), cx);
        frame(cx);
        view.read_with(cx, |this, _| {
            assert!(
                this.modal == Some(Modal::DeleteFolder(id.clone(), "INBOX/Receipts".into()))
            )
        });
        for part in ["modal-panel", "delete-folder", "delete-folder-confirm", "delete-folder-cancel"] {
            assert!(cx.debug_bounds(part).is_some(), "{part}");
        }
        act(&view, Action::Close, cx);
        view.read_with(cx, |this, _| {
            assert!(this.modal.is_none());
            assert!(this.deleting_folders.is_empty());
        });
        // Standard folders don't open it.
        act(&view, Action::DeleteRemoteFolder(id.clone(), "INBOX".into()), cx);
        view.read_with(cx, |this, _| assert!(this.modal.is_none()));
        // POP mailboxes have no folders on the server to delete.
        view.update(cx, |this, _| this.accounts[0].provider = Provider::Pop3);
        assert!(!labels(Mailbox::Remote("INBOX/Receipts".into()), cx)
            .contains(&"Delete Folder…".to_owned()));
    }

    #[gpui::test]
    fn server_folders_can_be_made_and_renamed(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let id = view.read_with(cx, |this, _| this.accounts[0].id.clone());
        let labels = |mailbox: Mailbox, cx: &mut VisualTestContext| {
            view.read_with(cx, |this, _| {
                this.mailbox_entries(&id, mailbox)
                    .into_iter()
                    .filter_map(|e| match e {
                        Entry::Item { label, .. } => Some(label.to_string()),
                        Entry::Separator => None,
                    })
                    .collect::<Vec<_>>()
            })
        };
        let has = |labels: &[String], label: &str| labels.iter().any(|l| l == label);
        // The same entries as top folders: New Folder, Rename…, New Folder Inside.
        assert!(has(&labels(Folder::Inbox.into(), cx), "New Folder"));
        assert!(!has(&labels(Folder::Sent.into(), cx), "New Folder"));
        let receipts = labels(Mailbox::Remote("INBOX/Receipts".into()), cx);
        assert!(has(&receipts, "Rename…") && has(&receipts, "New Folder Inside"), "{receipts:?}");
        // Renaming edits the name in place in the sidebar, with no dialog.
        act(&view, Action::RenameRemoteFolder(id.clone(), "INBOX/Receipts".into()), cx);
        frame(cx);
        let rename = FolderEdit::Rename("INBOX/Receipts".into());
        view.read_with(cx, |this, cx| {
            assert!(this.modal.is_none());
            assert!(this.editing_folder(&id, &rename));
            assert_eq!(MailApp::value(&this.rename_input, cx), "Receipts");
        });
        assert!(cx.debug_bounds(Box::leak(format!("nav-{id}-remote:INBOX/Receipts").into_boxed_str())).is_some());
        // Escape keeps the name; nothing goes to the server.
        act(&view, Action::Close, cx);
        view.read_with(cx, |this, _| {
            assert!(this.folder_edit.is_none() && this.folder_saving.is_none());
            assert!(this.tree_ui.renaming.is_none());
        });
        // A new folder appears in place as "New Folder", its name selected.
        view.update(cx, |this, _| this.collapsed.insert(format!("folders:{id}")));
        act(&view, Action::NewRemoteFolder(id.clone(), None), cx);
        frame(cx);
        view.read_with(cx, |this, cx| {
            assert!(this.modal.is_none());
            assert!(this.editing_folder(&id, &FolderEdit::New(None)));
            assert_eq!(MailApp::value(&this.rename_input, cx), "New Folder");
            // Its folders opened to show it.
            assert!(!this.collapsed.contains(&format!("folders:{id}")));
        });
        assert!(cx.debug_bounds(Box::leak(format!("new-folder-{id}").into_boxed_str())).is_some());
        // Enter makes it on the server; the row shows the name meanwhile.
        view.update_in(cx, |this, w, cx| {
            MailApp::set(&this.rename_input, "Travel", w, cx);
            this.commit_rename(w, cx);
            assert_eq!(
                this.folder_saving,
                Some((id.clone(), FolderEdit::New(None), "Travel".into()))
            );
            assert!(this.tree_ui.renaming.is_none());
        });
        // Draw without running tasks: the attempt would finish first.
        cx.update(|w, _| w.refresh());
        assert!(cx.debug_bounds(Box::leak(format!("new-folder-{id}").into_boxed_str())).is_some());
        // This account has no server here: the attempt ends, and says why.
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert!(this.folder_saving.is_none());
            assert!(this.error && this.status.contains("Couldn't create the folder"));
        });
        // Escape on a new folder still makes it, with its starting name, as
        // a top folder made with New Folder stays.
        view.update_in(cx, |this, w, cx| {
            this.handle(Action::NewRemoteFolder(id.clone(), Some("INBOX/Receipts".into())), w, cx);
            assert!(this.editing_folder(&id, &FolderEdit::New(Some("INBOX/Receipts".into()))));
            this.handle(Action::Close, w, cx);
            assert_eq!(
                this.folder_saving,
                Some((id.clone(), FolderEdit::New(Some("INBOX/Receipts".into())), "New Folder".into()))
            );
        });
        cx.run_until_parked();
        // A new folder's row is where it stays: at the end of its level,
        // like a top folder's, not where its name sorts.
        let names = |this: &MailApp| -> Vec<String> {
            this.ordered_folders(&id).into_iter().map(|f| f.name).collect()
        };
        let folder = |name: &str| RemoteFolder {
            name: name.into(),
            label: name.rsplit('/').next().unwrap().into(),
            depth: 0,
            nestable: true,
        };
        view.update_in(cx, |this, w, cx| {
            this.folders.get_mut(&id).unwrap().push(folder("INBOX/Zebra"));
            this.handle(Action::NewRemoteFolder(id.clone(), None), w, cx);
            let rows = this.folder_rows(&id);
            assert!(matches!(rows.last(), Some(tree::FolderRow::New { parent: None, depth: 0 })));
            this.cancel_rename(w, cx);
            // The server made "Aardvark", which sorts first; it stays last.
            let change = crate::manage::backend::FolderChange {
                folders: vec![folder("INBOX/Receipts"), folder("INBOX/Zebra"), folder("INBOX/Aardvark")],
                moved: vec![],
            };
            this.place_folders(&id, &FolderEdit::New(None), &change);
            this.folders.insert(id.clone(), change.folders);
            this.folder_saving = None;
            assert_eq!(names(this), ["INBOX/Receipts", "INBOX/Zebra", "INBOX/Aardvark"]);
            // Renamed, a folder keeps its place.
            let change = crate::manage::backend::FolderChange {
                folders: vec![folder("INBOX/Zzz"), folder("INBOX/Zebra"), folder("INBOX/Aardvark")],
                moved: vec![("INBOX/Receipts".into(), "INBOX/Zzz".into())],
            };
            this.place_folders(&id, &FolderEdit::Rename("INBOX/Receipts".into()), &change);
            this.folders.insert(id.clone(), change.folders);
            assert_eq!(names(this), ["INBOX/Zzz", "INBOX/Zebra", "INBOX/Aardvark"]);
            // And it's remembered.
            let saved = this.backend.store.preference(&format!("folder_order:{id}")).unwrap();
            assert_eq!(saved, r#"["INBOX/Zzz","INBOX/Zebra","INBOX/Aardvark"]"#);
        });
        // Standard folders can't be renamed.
        act(&view, Action::RenameRemoteFolder(id.clone(), "INBOX".into()), cx);
        view.read_with(cx, |this, _| assert!(this.folder_edit.is_none()));
        // POP mailboxes have no folders on the server.
        view.update(cx, |this, _| this.accounts[0].provider = Provider::Pop3);
        assert!(!has(&labels(Folder::Inbox.into(), cx), "New Folder"));
        assert!(!has(&labels(Mailbox::Remote("INBOX/Receipts".into()), cx), "Rename…"));
        act(&view, Action::NewRemoteFolder(id.clone(), None), cx);
        view.read_with(cx, |this, _| assert!(this.folder_edit.is_none()));
    }

    #[gpui::test]
    fn mail_moves_into_server_folders_from_menus(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let (work, home) =
            view.read_with(cx, |this, _| (this.accounts[0].id.clone(), this.accounts[1].id.clone()));
        let mail = |id: &str, account: &str, mailbox: Mailbox| {
            let mut m = Mail {
                id: id.into(),
                account: account.into(),
                from: "news@example.com".into(),
                ..Mail::default()
            };
            m.folder = mailbox.folder();
            if let Mailbox::Remote(name) = mailbox {
                m.remote_folder = name;
            }
            m
        };
        view.update(cx, |this, _| {
            this.folders.get_mut(&work).unwrap().push(RemoteFolder {
                name: "INBOX/Receipts/2026".into(),
                label: "2026".into(),
                depth: 1,
                nestable: true,
            });
            this.messages = vec![
                mail("w1", &work, Folder::Inbox.into()),
                mail("w2", &work, Folder::Inbox.into()),
                mail("w3", &work, Mailbox::Remote("INBOX/Receipts".into())),
                mail("h1", &home, Folder::Inbox.into()),
            ];
        });
        let labels = |entries: Vec<Entry>| -> Vec<String> {
            entries
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    Entry::Separator => None,
                })
                .collect()
        };
        let has = |labels: &[String], label: &str| labels.iter().any(|l| l == label);
        view.read_with(cx, |this, _| {
            // One message: its menu offers the folders, nested as in the sidebar.
            assert!(has(&labels(this.mail_entries("w1")), "Move to Folder…"));
            let target = MoveTarget::Mail(vec!["w1".into()]);
            assert_eq!(
                labels(this.move_folder_entries(&target)),
                ["Receipts", "\u{2003}2026"]
            );
            match &this.move_folder_entries(&target)[0] {
                Entry::Item {
                    action: Action::Apply(ids, change),
                    ..
                } => {
                    assert_eq!(ids, &["w1".to_owned()]);
                    assert_eq!(change, &Change::MoveTo("INBOX/Receipts".into()));
                }
                _ => panic!("not a move"),
            }
            // Not the folder it is already in.
            let inside = MoveTarget::Mail(vec!["w3".into()]);
            assert_eq!(labels(this.move_folder_entries(&inside)), ["\u{2003}2026"]);
            // The other account has its own folders.
            assert!(has(&labels(this.mail_entries("h1")), "Move to Folder…"));
            // A subscription's mail moves all at once.
            let sub = MailSubscription {
                account: work.clone(),
                address: "news@example.com".into(),
                ..MailSubscription::default()
            };
            let entries = labels(this.subscription_entries(&sub));
            assert_eq!(entries.last().map(String::as_str), Some("Move All to Folder…"));
            match &this.move_folder_entries(&MoveTarget::Subscription(Box::new(sub.clone())))[0] {
                Entry::Item {
                    action: Action::ChangeSubscription(s, change),
                    ..
                } => {
                    assert_eq!(s.as_ref(), &sub);
                    assert_eq!(change, &Change::MoveTo("INBOX/Receipts".into()));
                }
                _ => panic!("not a move"),
            }
        });
        // Several selected messages from one account move together.
        view.update(cx, |this, _| {
            this.selection = ["w1".to_owned(), "w2".to_owned()].into_iter().collect();
        });
        view.read_with(cx, |this, _| {
            assert!(has(&labels(this.mail_entries("w1")), "Move 2 to Folder…"));
            assert!(matches!(
                &this.move_folder_entries(&MoveTarget::Selection)[0],
                Entry::Item { action: Action::ApplySelection(Change::MoveTo(_)), .. }
            ));
        });
        // Folders belong to one account: a selection across two has none.
        view.update(cx, |this, _| {
            this.selection = ["w1".to_owned(), "h1".to_owned()].into_iter().collect();
        });
        view.read_with(cx, |this, _| {
            assert!(!has(&labels(this.mail_entries("w1")), "Move 2 to Folder…"));
            assert!(this.move_folder_entries(&MoveTarget::Selection).is_empty());
        });
        // Choosing a folder moves the message out of the list at once.
        view.update(cx, |this, _| this.selection.clear());
        act(
            &view,
            Action::Apply(vec!["w1".into()], Change::MoveTo("INBOX/Receipts".into())),
            cx,
        );
        view.read_with(cx, |this, _| assert!(!this.messages.iter().any(|m| m.id == "w1")));
        // POP mailboxes have no server folders.
        view.update(cx, |this, _| this.accounts[0].provider = Provider::Pop3);
        view.read_with(cx, |this, _| {
            assert!(!has(&labels(this.mail_entries("w2")), "Move to Folder…"));
        });
    }

    #[gpui::test]
    fn trash_labels_and_account_sending_settings(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let (work, home) =
            view.read_with(cx, |this, _| (this.accounts[0].id.clone(), this.accounts[1].id.clone()));
        let mail = |id: &str, account: &str, folder: Folder, labels: &[&str]| {
            let mut m = Mail {
                id: id.into(),
                account: account.into(),
                folder,
                ..Mail::default()
            };
            m.set_labels(labels.iter().map(|l| (*l).into()).collect());
            m
        };
        view.update(cx, |this, cx| {
            // Not this view's mail: nothing reloads it from the cache.
            this.load_generation += 1;
            this.messages = vec![
                mail("t1", &work, Folder::Trash, &["Work"]),
                mail("t2", &work, Folder::Trash, &[]),
                mail("i1", &work, Folder::Inbox, &["Work", "Later"]),
                mail("h1", &home, Folder::Trash, &[]),
            ];
            this.filter(cx);
        });
        let labels = |entries: Vec<Entry>| -> Vec<(String, bool)> {
            entries
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Item { label, checked, .. } => Some((label.to_string(), checked)),
                    Entry::Separator => None,
                })
                .collect()
        };
        let names = |entries: Vec<Entry>| -> Vec<String> {
            labels(entries).into_iter().map(|(l, _)| l).collect()
        };
        view.read_with(cx, |this, _| {
            // Mail in Trash can be deleted for good; mail elsewhere can't.
            assert!(names(this.mail_entries("t1")).contains(&"Delete Permanently…".to_owned()));
            assert!(!names(this.mail_entries("i1")).contains(&"Delete Permanently…".to_owned()));
            // Trash and Spam can be emptied.
            assert!(names(this.mailbox_entries(&work, Folder::Trash.into())).contains(&"Empty Trash…".to_owned()));
            assert!(names(this.mailbox_entries(&work, Folder::Junk.into())).contains(&"Empty Spam…".to_owned()));
            assert!(!names(this.mailbox_entries(&work, Folder::Inbox.into())).iter().any(|l| l.starts_with("Empty")));
            // Labels in use are offered, checked where the message has them.
            assert!(names(this.mail_entries("i1")).contains(&"Labels…".to_owned()));
            let picker = labels(this.label_entries(&MoveTarget::Mail(vec!["t1".into()])));
            assert!(picker.contains(&("Work".to_owned(), true)), "{picker:?}");
            assert!(picker.contains(&("New Label…".to_owned(), false)));
        });
        // Several selected messages in Trash.
        view.update(cx, |this, _| {
            this.selection = ["t1".to_owned(), "t2".to_owned()].into_iter().collect();
        });
        view.read_with(cx, |this, _| {
            let menu = names(this.mail_entries("t1"));
            assert!(menu.contains(&"Delete 2 Permanently…".to_owned()), "{menu:?}");
            assert!(menu.contains(&"Label 2…".to_owned()));
        });
        // Deleting asks first, in the app's own dialog.
        act(&view, Action::DeleteForever(vec!["t1".into()]), cx);
        frame(cx);
        view.read_with(cx, |this, _| {
            assert!(this.modal == Some(Modal::Erase(Erase::Mail(vec!["t1".into()]))));
        });
        for part in ["erase", "erase-confirm", "erase-cancel"] {
            assert!(cx.debug_bounds(part).is_some(), "{part}");
        }
        act(&view, Action::Close, cx);
        act(&view, Action::EmptyMailbox(work.clone(), Folder::Trash), cx);
        view.read_with(cx, |this, _| {
            assert!(this.modal == Some(Modal::Erase(Erase::Mailbox(work.clone(), Folder::Trash))));
        });
        act(&view, Action::Close, cx);
        // A new label is named in a dialog; names keywords can't hold are refused there.
        view.update(cx, |this, _| this.selection.clear());
        act(&view, Action::NewLabel(MoveTarget::Mail(vec!["i1".into()])), cx);
        frame(cx);
        assert!(cx.debug_bounds("new-label-name").is_some());
        view.update_in(cx, |this, w, cx| {
            MailApp::set(&this.label_input, "two words", w, cx);
            this.handle(Action::SaveLabel, w, cx);
            assert!(this.error);
            assert!(matches!(this.modal, Some(Modal::NewLabel(_))));
            MailApp::set(&this.label_input, "Urgent", w, cx);
            this.handle(Action::SaveLabel, w, cx);
            assert!(this.modal.is_none());
            let m = this.messages.iter().find(|m| m.id == "i1").unwrap();
            assert!(m.labels().contains(&"Urgent".to_owned()));
        });
        // The account page has the sign-in name, Reply-To and receipts.
        act(&view, Action::AccountSettings(work.clone()), cx);
        frame(cx);
        for part in ["account-login", "account-reply-to", "account-storage"] {
            assert!(cx.debug_bounds(part).is_some(), "{part}");
        }
        // The receipts switches sit inside the page, their captions wrapping.
        let page = cx.debug_bounds("settings-page").unwrap();
        for kind in ["read", "delivery"] {
            let selector: &'static str = Box::leak(format!("{kind}-receipts-{work}").into_boxed_str());
            let switch = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector}"));
            assert!(
                switch.left() >= page.left() && switch.right() <= page.right(),
                "{kind} receipts switch {switch:?} outside the page {page:?}"
            );
            assert!(switch.size.width >= px(38.), "{switch:?}");
        }
        view.update_in(cx, |this, w, cx| {
            // Typed, so the field has focus.
            { let focus = this.login_input.read(cx).focus_handle(cx); focus.focus(w, cx); }
            MailApp::set(&this.login_input, "work-user", w, cx);
        });
        frame(cx);
        // Typing alone changes nothing: each change would sign out.
        view.read_with(cx, |this, _| {
            let a = this.accounts.iter().find(|a| a.id == work).unwrap();
            assert_ne!(a.login(), "work-user");
        });
        // Enter applies it.
        view.update(cx, |this, cx| {
            this.login_input
                .update(cx, |_, cx| {
                    cx.emit(InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    })
                });
        });
        view.update_in(cx, |this, w, cx| {
            MailApp::set(&this.reply_to_input, "desk@example.com", w, cx);
            this.handle(Action::ToggleReadReceipts(work.clone()), w, cx);
        });
        view.read_with(cx, |this, _| {
            let a = this.accounts.iter().find(|a| a.id == work).unwrap();
            assert_eq!(a.login(), "work-user");
            assert_eq!(a.reply_to, "desk@example.com");
            assert!(a.read_receipts && !a.delivery_receipts);
            let saved = this.backend.store.accounts().unwrap();
            let saved = saved.iter().find(|s| s.id == work).unwrap();
            assert_eq!(saved.username, "work-user");
            assert!(saved.read_receipts);
        });
        // Left unfinished, what was typed stays in the field (the page
        // refreshes fields from the account it shows), and applies once
        // typing has paused.
        view.update_in(cx, |this, w, cx| {
            { let focus = this.login_input.read(cx).focus_handle(cx); focus.focus(w, cx); }
            MailApp::set(&this.login_input, "work-user2", w, cx);
        });
        frame(cx);
        view.update_in(cx, |this, w, cx| this.focus.focus(w, cx));
        frame(cx);
        view.read_with(cx, |this, cx| {
            assert_eq!(MailApp::value(&this.login_input, cx), "work-user2");
            let a = this.accounts.iter().find(|a| a.id == work).unwrap();
            assert_eq!(a.login(), "work-user");
        });
        cx.executor().advance_clock(LOGIN_PAUSE);
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            let a = this.accounts.iter().find(|a| a.id == work).unwrap();
            assert_eq!(a.login(), "work-user2");
        });
        // POP mailboxes: deleting for good, but no labels.
        view.update(cx, |this, cx| {
            this.accounts[0].provider = Provider::Pop3;
            this.load_generation += 1;
            this.messages = vec![mail("t2", &work, Folder::Trash, &[])];
            this.filter(cx);
        });
        view.read_with(cx, |this, _| {
            let menu = names(this.mail_entries("t2"));
            assert!(menu.contains(&"Delete Permanently…".to_owned()));
            assert!(!menu.contains(&"Labels…".to_owned()));
        });
    }

    #[gpui::test]
    fn unlisted_mail_holds_its_rows_as_placeholders(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let a = view.read_with(cx, |this, _| this.accounts[0].clone());
        // The server has 500 more messages than are listed here.
        view.update_in(cx, |this, w, cx| {
            this.backend.set_test_layout(
                &a,
                &Folder::Inbox.into(),
                "INBOX",
                7,
                (1..=500).collect(),
            );
            this.open_mailbox(Some(a.id.clone()), Folder::Inbox.into(), w, cx);
        });
        cx.run_until_parked();
        frame(cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.messages.len(), 510);
            assert_eq!(
                this.visible.len(),
                510,
                "the scrollbar covers the whole mailbox"
            );
            // Newest first, by arrival: placeholders keep their places.
            assert!(this.messages[this.visible[0]].pending);
            assert_eq!(this.messages[this.visible[0]].uid, 500);
        });
        assert!(cx.debug_bounds("placeholder-row").is_some());
        // Placeholders cannot be opened or selected, and filters skip them.
        let pending = view.read_with(cx, |this, _| this.messages[this.visible[0]].id.clone());
        act(&view, Action::Select(pending), cx);
        act(&view, Action::SelectAll, cx);
        view.update(cx, |this, cx| {
            assert!(this.selected.is_none());
            assert_eq!(this.selection.len(), 10);
            this.filter_by = Filter::Unread;
            this.apply_filter(cx);
            assert!(this.visible.iter().all(|i| !this.messages[*i].pending));
        });
        // Listing a placeholder puts the message in its row.
        view.update(cx, |this, cx| {
            this.filter_by = Filter::All;
            this.apply_filter(cx);
            let id = this.messages[this.visible[0]].id.clone();
            let mut listed = demo_mail().remove(0);
            listed.id = id;
            listed.uid = 500;
            listed.subject = "Listed now".into();
            assert!(this.place(vec![listed], cx));
            let row = &this.messages[this.visible[0]];
            assert!(!row.pending);
            assert_eq!(row.subject, "Listed now");
        });
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
    fn a_setting_that_cannot_be_saved_says_so_and_is_put_back(cx: &mut TestAppContext) {
        let (view, cx, store) = two_accounts(cx);
        store.fail_preference_writes_for_test();
        let before = view.read_with(cx, |this, _| this.reading_defaults);
        let changed = ReadingOptions {
            html: !before.html,
            ..before
        };
        act(&view, Action::SetReadingDefaults(changed), cx);
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            assert!(this.error, "an error is shown: {}", this.status);
            assert!(this.status.contains("Couldn't save"), "{}", this.status);
            assert_eq!(this.reading_defaults, before, "the old setting is back");
        });
    }

    #[gpui::test]
    fn a_drag_whose_release_was_missed_ends_on_the_next_move(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let first = view.read_with(cx, |this, _| this.accounts[0].id.clone());
        let from = center(cx, Box::leak(format!("account-{first}").into_boxed_str()));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        for i in 1..=4 {
            let at = point(from.x, from.y + px(10. * i as f32));
            cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        }
        cx.update(|_, cx| assert!(cx.has_active_drag(), "dragging while held"));
        // The release never arrives; the pointer moves on with no button.
        cx.simulate_event(MouseMoveEvent {
            position: point(from.x + px(40.), from.y + px(60.)),
            pressed_button: None,
            modifiers: Modifiers::none(),
        });
        cx.run_until_parked();
        cx.update(|_, cx| assert!(!cx.has_active_drag(), "the drag ended"));
        view.read_with(cx, |this, _| {
            assert!(this.tree_ui.dragging.is_none() && this.tree_ui.drop_at.is_none());
        });
    }

    #[gpui::test]
    fn sidebar_folders_hold_dragged_accounts(cx: &mut TestAppContext) {
        let (view, cx, store) = two_accounts(cx);
        let (first, second) = view.read_with(cx, |this, _| {
            (this.accounts[0].id.clone(), this.accounts[1].id.clone())
        });
        // A new folder goes at the end and starts with its name selected.
        act(&view, Action::NewFolder(None), cx);
        let folder = view.read_with(cx, |this, _| {
            let key = this.tree.last().unwrap().key().to_owned();
            assert_eq!(this.tree_ui.renaming.as_ref(), Some(&key));
            key
        });
        view.update_in(cx, |this, w, cx| {
            MailApp::set(&this.rename_input, "Personal", w, cx);
            this.commit_rename(w, cx);
        });
        // Collapse the second account so the folder sits right below it.
        act(&view, Action::ToggleAccount(second.clone()), cx);
        let from = center(cx, Box::leak(format!("account-{first}").into_boxed_str()));
        let to = center(cx, Box::leak(format!("folder-{folder}").into_boxed_str()));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        for i in 1..=8 {
            let t = i as f32 / 8.;
            let at = point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            cx.simulate_mouse_move(at, MouseButton::Left, Modifiers::none());
        }
        view.read_with(cx, |this, _| {
            assert_eq!(
                this.tree_ui.drop_at,
                Some(tree::DropAt::Into(folder.clone())),
                "the folder's middle takes the account"
            );
        });
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        view.read_with(cx, |this, _| {
            let Some(tree::Node::Folder { name, children, .. }) = tree::find(&this.tree, &folder)
            else {
                panic!("the folder is kept");
            };
            assert_eq!(name, "Personal");
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].key(), first);
            assert_eq!(
                tree::account_order(&this.tree),
                [second.clone(), first.clone()]
            );
        });
        // The arrangement is saved.
        let saved = store.preference("sidebar_tree").unwrap();
        assert!(saved.contains("Personal") && saved.contains(&first));

        // The rail shows each account's folders by its own open state, not
        // by which account is current.
        act(&view, Action::ToggleSidebar, cx);
        cx.run_until_parked();
        frame(cx);
        assert!(
            cx.debug_bounds(Box::leak(format!("rail-{second}-inbox").into_boxed_str()))
                .is_none()
        );
        assert!(
            cx.debug_bounds(Box::leak(format!("rail-{first}-inbox").into_boxed_str()))
                .is_some()
        );
        act(&view, Action::ToggleAccount(second.clone()), cx);
        frame(cx);
        assert!(
            cx.debug_bounds(Box::leak(format!("rail-{second}-inbox").into_boxed_str()))
                .is_some()
        );
        // Deleting the folder keeps its account.
        act(&view, Action::DeleteFolder(folder.clone()), cx);
        view.read_with(cx, |this, _| {
            assert!(tree::find(&this.tree, &folder).is_none());
            assert_eq!(tree::account_order(&this.tree), [second.clone(), first.clone()]);
        });
        // Settings lists the mailboxes in the sidebar's order, though they
        // were added the other way round.
        act(&view, Action::Settings, cx);
        frame(cx);
        frame(cx);
        let top = |cx: &mut VisualTestContext, id: &str| {
            cx.debug_bounds(Box::leak(format!("settings-account-{id}").into_boxed_str()))
                .expect("listed in Settings")
                .top()
        };
        assert!(top(cx, &second) < top(cx, &first));
    }

    #[gpui::test]
    fn links_lose_their_tracking_until_turned_off(cx: &mut TestAppContext) {
        let (view, cx, store) = two_accounts(cx);
        let id = view.update(cx, |this, _| {
            assert!(this.clean_links, "on by default");
            let m = &mut this.messages[0];
            m.html = None;
            m.body = Some("Order: https://shop.example/o?id=7&utm_source=mail&fbclid=x".into());
            m.id.clone()
        });
        act(&view, Action::Select(id.clone()), cx);
        let body = |view: &Entity<MailApp>, cx: &mut VisualTestContext| {
            view.read_with(cx, |this, _| {
                let m = this.messages.iter().find(|m| m.id == id).unwrap();
                this.plain_body(m).0.to_string()
            })
        };
        let shown = body(&view, cx);
        assert!(shown.contains("/o?id=7"), "{shown}");
        assert!(!shown.contains("utm_source") && !shown.contains("fbclid"), "{shown}");
        // Settings → Reading turns it off, and it stays off.
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Reading), cx);
        frame(cx);
        frame(cx);
        assert!(cx.debug_bounds("clean-links").is_some());
        act(&view, Action::SetCleanLinks(false), cx);
        assert!(body(&view, cx).contains("utm"));
        cx.run_until_parked();
        store.flush();
        assert_eq!(store.preference("clean_links").as_deref(), Some("false"));
    }

    #[gpui::test]
    fn an_empty_all_mail_says_it_is_still_fetching(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        // A test window alternates between two frames; draw both.
        let redraw = |cx: &mut VisualTestContext| {
            frame(cx);
            frame(cx);
        };
        // All Accounts' spam is empty in the cache; both accounts are asked.
        view.update_in(cx, |this, w, cx| {
            this.open_mailbox(Some(ALL_MAIL.into()), Folder::Junk.into(), w, cx)
        });
        // Let the real syncs it starts (on other threads) finish first, so
        // none lands in the middle of the states set below.
        for _ in 0..200 {
            cx.run_until_parked();
            let idle = view.read_with(cx, |this, _| {
                !this.cache_loading
                    && this.sync_requests.is_empty()
                    && this.retrying.is_empty()
                    && this.cataloging.is_empty()
                    && this.busy == 0
            });
            if idle {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (ids, key) = view.read_with(cx, |this, _| {
            assert!(this.visible.is_empty());
            let ids: Vec<String> = this.accounts.iter().map(|a| a.id.clone()).collect();
            (ids, this.mailbox().key())
        });
        // One account still being fetched: the list says so, not "No mail".
        view.update(cx, |this, cx| {
            this.account_issues.clear();
            this.sync_requests.insert((ids[1].clone(), key.clone()));
            cx.notify();
        });
        redraw(cx);
        view.read_with(cx, |this, _| {
            assert!(this.mailbox_loading());
            assert_eq!(this.fetching_accounts().len(), 1);
            assert!(this.checking_server());
        });
        assert!(cx.debug_bounds("empty-list-detail").is_some());
        // (Test frames keep the bounds of elements drawn before, as the
        // Refresh button may have been while the real syncs failed.)
        // Done, but that account failed: it says which, with a way to retry.
        view.update(cx, |this, cx| {
            this.sync_requests.clear();
            this.account_result::<()>(
                &ids[1],
                IssueSource::Sync(key.clone()),
                &Err(anyhow::anyhow!("connection reset")),
            );
            cx.notify();
        });
        redraw(cx);
        view.read_with(cx, |this, _| {
            assert!(!this.mailbox_loading());
            assert_eq!(this.unreachable_accounts().len(), 1);
        });
        assert!(cx.debug_bounds("empty-list-retry").is_some());
        // Every account answered with nothing: it is simply empty.
        view.update(cx, |this, cx| {
            this.account_issues.clear();
            cx.notify();
        });
        // (Test frames keep the bounds of elements drawn before, so this
        // checks the state the list is drawn from.)
        view.read_with(cx, |this, _| {
            assert!(!this.mailbox_loading());
            assert!(this.unreachable_accounts().is_empty());
        });
    }

    #[gpui::test]
    fn selecting_doesnt_pick_the_open_message(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let open = view.read_with(cx, |this, _| this.messages[0].id.clone());
        act(&view, Action::Select(open.clone()), cx);
        view.read_with(cx, |this, _| {
            assert_eq!(this.selected.as_ref(), Some(&open))
        });
        act(&view, Action::ToggleSelectMode, cx);
        view.read_with(cx, |this, _| {
            assert!(this.selecting);
            assert!(this.selection.is_empty(), "nothing is picked until clicked");
        });
    }

    #[gpui::test]
    fn select_button_summary_and_sender_searches(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        // The list's Select button shows the checkboxes before anything is picked.
        frame(cx);
        assert!(cx.debug_bounds("select-toggle").is_some());
        act(&view, Action::ToggleSelectMode, cx);
        view.read_with(cx, |this, _| {
            assert!(this.selecting);
            assert!(this.selection.is_empty());
        });
        frame(cx);
        assert!(cx.debug_bounds("select-all").is_some());
        assert!(cx.debug_bounds("bulk-archive").is_none(), "no tools for nothing");
        // A plain click picks rather than opens.
        let (a, b) = view.read_with(cx, |this, _| {
            (this.messages[1].id.clone(), this.messages[2].id.clone())
        });
        for id in [&a, &b] {
            let at = center(cx, Box::leak(format!("row-{id}").into_boxed_str()));
            cx.simulate_click(at, Modifiers::none());
        }
        view.read_with(cx, |this, _| {
            assert_eq!(this.selection.len(), 2);
            assert!(this.selected.is_none());
        });
        // The reader sums the selection up and acts on all of it.
        frame(cx);
        assert!(cx.debug_bounds("selection-summary").is_some());
        assert!(cx.debug_bounds("summary-star").is_some());
        act(&view, Action::ToggleSelectMode, cx);
        view.read_with(cx, |this, _| {
            assert!(!this.selecting);
            assert!(this.selection.is_empty());
        });
        // Every listed message from one sender, at once.
        let (row, from, total) = view.read_with(cx, |this, _| {
            let m = &this.messages[0];
            let total = this.messages.iter().filter(|o| o.from == m.from).count();
            (m.id.clone(), m.from.clone(), total)
        });
        act(&view, Action::SelectFromSender(row.clone()), cx);
        view.read_with(cx, |this, _| assert_eq!(this.selection.len(), total));
        act(&view, Action::StopSelecting, cx);
        // The menu searches by sender, and by Reply-To where it differs.
        view.update(cx, |this, cx| {
            this.messages[0].reply_to = "List <replies@lists.example>".into();
            this.apply_filter(cx);
        });
        let labels = view.read_with(cx, |this, _| {
            this.mail_entries(&row)
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Item { label, .. } => Some(label.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        assert!(labels.iter().any(|l| l == "Show Mail From Sender"));
        assert!(labels.iter().any(|l| l == "Show Mail With This Reply-To"));
        act(&view, Action::SearchFor("reply-to:replies@lists.example".into()), cx);
        view.read_with(cx, |this, cx| {
            assert!(this.search_open);
            assert_eq!(MailApp::value(&this.search, cx), "reply-to:replies@lists.example");
            assert_eq!(this.visible.len(), 1);
            assert_eq!(this.messages[this.visible[0]].id, row);
        });
        act(&view, Action::SearchFor(format!("from:{from}")), cx);
        view.read_with(cx, |this, _| assert_eq!(this.visible.len(), total));
        // Hints show while the search is empty.
        act(&view, Action::SearchFor(String::new()), cx);
        frame(cx);
        assert!(cx.debug_bounds("search-hint-reply-to:").is_some());
    }

    #[test]
    fn menus_keep_clear_of_a_client_decorated_windows_shadow() {
        // A window drawing its own decorations: a 20px transparent shadow
        // band on every side, as gpui-component draws it.
        let viewport = size(px(1000.), px(700.));
        let band = Edges::all(px(20.));
        let menu = size(px(220.), px(160.));
        // The Add button sits near the bottom left. Opened 10px above the
        // frame's bottom, the menu used to run 150px past it.
        let at = point(px(40.), px(670.));
        assert_eq!(menu_origin(at, menu, viewport, band), point(px(40.), px(512.)));
        // Near the right edge, it moves left to clear the band.
        let at = point(px(900.), px(100.));
        assert_eq!(menu_origin(at, menu, viewport, band), point(px(752.), px(100.)));
        // Room where it opens: it stays at the pointer.
        let at = point(px(300.), px(200.));
        assert_eq!(menu_origin(at, menu, viewport, band), at);
        // Maximized or tiled: no band, just the margin.
        let at = point(px(40.), px(695.));
        let none = Edges::all(px(0.));
        assert_eq!(menu_origin(at, menu, viewport, none), point(px(40.), px(532.)));
        // Too big for the frame: its top left stays in view.
        let huge = size(px(1200.), px(900.));
        assert_eq!(menu_origin(at, huge, viewport, band), point(px(28.), px(28.)));
    }

    #[gpui::test]
    fn menus_opened_near_an_edge_stay_inside_the_window_frame(cx: &mut TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        let size = cx.update(|w, _| w.viewport_size());
        let insets = cx.update(|w, _| window::frame_insets(w));
        let margins = insets.map(|i| *i + px(MENU_MARGIN));
        // The Add menu opens at the pointer; the button sits low in the
        // sidebar, so try every corner of the window.
        for at in [
            point(px(4.), size.height - px(4.)),
            point(size.width - px(4.), size.height - px(4.)),
            point(size.width - px(4.), px(4.)),
            point(px(4.), px(4.)),
        ] {
            view.update(cx, |this, cx| {
                let entries = this.create_entries(None);
                this.open_menu(at, entries, cx)
            });
            // Past the opening animation, which nudges the panel.
            cx.executor().advance_clock(Duration::from_millis(200));
            frame(cx);
            let panel = cx.debug_bounds("menu-panel").expect("the menu is shown");
            assert!(panel.top() >= margins.top, "{at:?}: {panel:?}");
            assert!(panel.left() >= margins.left, "{at:?}: {panel:?}");
            assert!(panel.bottom() <= size.height - margins.bottom, "{at:?}: {panel:?}");
            assert!(panel.right() <= size.width - margins.right, "{at:?}: {panel:?}");
        }
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
        // A trackpad's two-finger click sends a tiny scroll with it; the menu stays.
        cx.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(1.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Started,
            source: Default::default(),
        });
        view.read_with(cx, |this, _| assert!(this.menu.is_some()));
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
