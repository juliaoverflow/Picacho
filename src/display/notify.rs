//! System notifications for new mail. Failures are ignored: a missing or
//! disabled notification service must never interrupt mail. Android's go
//! through the app's Java side (`android::notify`); elsewhere notify-rust's.
//!
//! Tapping one opens what it tells of (its `Target`): the message in its
//! mailbox, or the mailbox. The tap reaches `clicked`, and the window takes
//! it with `take_clicked` (see `MailApp::open_notified`).
//!
//! New mail's notifications have buttons (Linux, Windows, macOS, Android):
//! Mark as Read, Archive, Delete and Reply for one message; Mark All as
//! Read, Archive All and Delete All for several. A button reaches `quick`;
//! the window acts on it (`MailApp::quick_action`), or, while there is no
//! window, the background process or Android's service does (`Background`).

use crate::manage::{
    backend::{Backend, Change},
    model::{Folder, Mailbox},
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Where tapping a notification goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The account's id.
    pub account: String,
    pub mailbox: Mailbox,
    /// The message to open: its id, or its Message-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Every message announced (several new messages): what the buttons
    /// act on. Empty when it's `message` alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    /// A button pressed, where the press travels with the target: Reply,
    /// which opens Picacho (Android's activity, or the window the
    /// background process starts) with a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Target {
    fn new(account: &str, folder: Folder, message: Option<&str>) -> Self {
        Self {
            account: account.to_owned(),
            mailbox: folder.into(),
            message: message.filter(|m| !m.is_empty()).map(str::to_owned),
            messages: Vec::new(),
            action: None,
        }
    }
    /// The messages a button acts on: every one announced, or the one.
    pub fn message_ids(&self) -> Vec<String> {
        if self.messages.is_empty() {
            self.message.iter().cloned().collect()
        } else {
            self.messages.clone()
        }
    }
}

/// The most messages a notification's buttons act on: more are left as
/// they are (and the notification says how many arrived).
const MOST_MESSAGES: usize = 50;

/// What a button other than Reply changes.
pub fn quick_change(action: &str) -> Option<Change> {
    Some(match action {
        "read" => Change::Read(true),
        "archive" => Change::Move(Folder::Archive),
        "trash" => Change::Move(Folder::Trash),
        _ => return None,
    })
}

/// Acting on taps and buttons while no window is there to: the background
/// process's, or Android's service's. `open` starts the window with a
/// target to open, or a reply to write (`Target::action`); without it
/// (Android, whose taps open the activity themselves) they're kept for
/// the window.
pub struct Background {
    pub backend: Arc<Backend>,
    pub open: Option<Box<dyn Fn(Target) + Send + Sync>>,
}

/// Whether taps go to the background's `open` rather than wait for a
/// window.
fn opens_in_background() -> bool {
    !window_listening()
        && BACKGROUND
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|b| b.open.is_some())
}
static BACKGROUND: Mutex<Option<Background>> = Mutex::new(None);

/// Taps and buttons go to `background` while no window takes them. Ones
/// that came before it (a button that started Android's service) are
/// acted on now.
pub fn set_background(background: Background) {
    // Under BACKGROUND's lock, as `in_background` keeps them: none is
    // kept after this has looked.
    let pending = {
        let mut b = BACKGROUND.lock().unwrap_or_else(|e| e.into_inner());
        *b = Some(background);
        if window_listening() { Vec::new() } else { take_quick() }
    };
    for (target, action) in pending {
        in_background(target, &action);
    }
}

/// Whether a window takes taps and buttons now.
fn window_listening() -> bool {
    WAKE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|w| !w.is_closed())
}

/// A tap or button with no window to take it: Reply and taps open one;
/// the others change the messages here and on the server.
fn in_background(target: Target, action: &str) {
    let background = BACKGROUND.lock().unwrap_or_else(|e| e.into_inner());
    let Some(b) = background.as_ref() else {
        // Nothing yet to act with: kept for the window, or for
        // `set_background`.
        QUICK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((target, action.to_owned()));
        return;
    };
    let Some(change) = quick_change(action) else {
        let mut target = target;
        if action != "default" {
            target.action = Some(action.to_owned());
        }
        match &b.open {
            Some(open) => open(target),
            // Kept for the window: Reply as its target says.
            None => {
                drop(background);
                *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(target);
            }
        }
        return;
    };
    let backend = b.backend.clone();
    drop(background);
    std::thread::spawn(move || {
        if let Err(e) = act(&backend, &target, change) {
            eprintln!("Picacho: a notification's button couldn't act: {e:#}");
        }
    });
}

/// Makes `change` to the messages `target` announced, as the window does.
pub fn act(backend: &Backend, target: &Target, change: Change) -> anyhow::Result<()> {
    let store = &backend.store;
    let a = store
        .accounts()?
        .into_iter()
        .find(|a| a.id == target.account)
        .ok_or_else(|| anyhow::anyhow!("The account is gone"))?;
    let mails: Vec<_> = target
        .message_ids()
        .iter()
        .filter_map(|id| {
            store
                .message(id)
                .ok()
                .flatten()
                .or_else(|| store.by_message_id(id).ok().flatten())
        })
        .filter(|m| m.account == a.id)
        .collect();
    if mails.is_empty() {
        return Ok(());
    }
    let result = backend.change_many(&a, &mails, change);
    if result.is_ok() {
        crate::manage::widgets::publish_soon(backend.store.clone());
    }
    result
}

/// The notification tapped last, until the window takes it.
static CLICKED: Mutex<Option<Target>> = Mutex::new(None);
/// Tells the window a notification was tapped.
static WAKE: Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>> = Mutex::new(None);

/// A notification was tapped: `target` is what it showed (as `Target`'s
/// JSON, from the platform's side). Kept until the window takes it, so a
/// tap that opens the app is followed once its window is up.
pub fn clicked(target: &str) {
    let Ok(target) = serde_json::from_str::<Target>(target) else {
        return;
    };
    // No window: the background process opens one with it.
    if opens_in_background() {
        in_background(target, "default");
        return;
    }
    *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(target);
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = wake.unbounded_send(());
    }
}

/// Buttons a new message's notification offers: their action keys and
/// labels, for one message and for several.
const QUICK_ONE: [(&str, &str); 4] = [
    ("read", "Mark as Read"),
    ("archive", "Archive"),
    ("trash", "Delete"),
    ("reply", "Reply"),
];
const QUICK_SEVERAL: [(&str, &str); 3] = [
    ("read", "Mark All as Read"),
    ("archive", "Archive All"),
    ("trash", "Delete All"),
];
/// Whether `action` is one of the buttons.
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos")), allow(dead_code))]
fn is_quick(action: &str) -> bool {
    QUICK_ONE.iter().any(|(k, _)| *k == action)
}
/// Notification buttons pressed, until the window takes them: the
/// notification's target and the button's key.
static QUICK: Mutex<Vec<(Target, String)>> = Mutex::new(Vec::new());

/// A button on a notification was pressed (`target` as `Target`'s JSON):
/// the window acts on it, or the background process or service does.
pub fn quick(target: &str, action: &str) {
    let Ok(target) = serde_json::from_str::<Target>(target) else {
        return;
    };
    if !window_listening() {
        in_background(target, action);
        return;
    }
    QUICK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((target, action.to_owned()));
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = wake.unbounded_send(());
    }
}

/// Notification buttons pressed that the window hasn't acted on.
pub fn take_quick() -> Vec<(Target, String)> {
    std::mem::take(&mut *QUICK.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The notification tapped last, if the window hasn't taken it.
pub fn take_clicked() -> Option<Target> {
    CLICKED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Taps from now on, for the window to `take_clicked`. A window opened
/// later takes them over; the earlier one's stream then ends.
pub fn clicks() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
    receiver
}

/// Announces new messages in one account. `latest` is (sender, subject),
/// and `message` the latest's id, which tapping opens; `messages` are all
/// of them, which its buttons act on.
pub fn new_mail(
    account: &str,
    account_id: &str,
    message: &str,
    messages: &[&str],
    latest: (&str, &str),
    count: usize,
) {
    // Settings → Privacy → Hide mail outside Picacho: how many, not who or what.
    let (title, body) = if crate::manage::applock::hiding() {
        if count == 1 {
            ("New message".to_owned(), String::new())
        } else {
            (format!("{count} new messages"), String::new())
        }
    } else if count == 1 {
        (latest.0.to_owned(), latest.1.to_owned())
    } else {
        (
            format!("{count} new messages"),
            format!("{} — {}", latest.0, latest.1),
        )
    };
    // A newer announcement for the account replaces the last (Android).
    let mut target = Target::new(account_id, Folder::Inbox, Some(message));
    if messages.len() > 1 {
        target.messages = messages
            .iter()
            .take(MOST_MESSAGES)
            .map(|m| (*m).to_owned())
            .collect();
    }
    let buttons: &[(&str, &str)] = if messages.len() > 1 { &QUICK_SEVERAL } else { &QUICK_ONE };
    show_with(
        format!("mail:{account}"),
        title,
        account.to_owned(),
        body,
        Some(target),
        buttons,
    );
}

/// Shows what the background monitor has to tell (see
/// `manage::background`).
pub fn announce(announcement: crate::manage::background::Announcement<'_>) {
    use crate::manage::background::Announcement::*;
    match announcement {
        NewMail {
            account,
            account_id,
            message,
            messages,
            sender,
            subject,
            count,
        } => new_mail(account, account_id, message, messages, (sender, subject), count),
        BackFromSnooze {
            account,
            account_id,
            message_id,
            sender,
            subject,
            count,
        } => {
            let hiding = crate::manage::applock::hiding();
            let body = if count == 1 && hiding {
                "A snoozed message is back in the inbox".to_owned()
            } else if count == 1 {
                format!("{sender} — {}", or_untitled(subject))
            } else {
                format!("{count} snoozed messages are back in the inbox")
            };
            show(
                format!("snooze:{account}"),
                "Back from snooze".to_owned(),
                account.to_owned(),
                body,
                // One message opens; several, the inbox they're back in.
                Some(Target::new(
                    account_id,
                    Folder::Inbox,
                    (count == 1).then_some(message_id),
                )),
            );
        }
        ScheduledSent {
            account,
            account_id,
            to,
            subject,
        } => show(
            format!("sent:{account}"),
            "Scheduled message sent".to_owned(),
            account.to_owned(),
            if crate::manage::applock::hiding() {
                String::new()
            } else {
                format!("{} — to {to}", or_untitled(subject))
            },
            Some(Target::new(account_id, Folder::Sent, None)),
        ),
        ScheduledFailed {
            account,
            account_id,
            draft,
            subject,
            error,
        } => show(
            format!("unsent:{draft}"),
            "A scheduled message wasn't sent".to_owned(),
            account.to_owned(),
            if crate::manage::applock::hiding() {
                "It's in Drafts.".to_owned()
            } else {
                format!("{}: {error} It's in Drafts.", or_untitled(subject))
            },
            account_id.map(|id| Target::new(id, Folder::Drafts, None)),
        ),
    }
}

fn or_untitled(subject: &str) -> &str {
    if subject.trim().is_empty() {
        "(No subject)"
    } else {
        subject
    }
}

/// Shows a notice from the mail provider itself (the messages Proton
/// sends its apps' users), titled as the provider sent it. Tapping it
/// opens the account's inbox.
pub fn account_notice(account: &str, account_id: &str, title: &str, body: &str) {
    show(
        format!("notice:{account}:{title}"),
        title.to_owned(),
        account.to_owned(),
        body.to_owned(),
        Some(Target::new(account_id, Folder::Inbox, None)),
    );
}

/// A calendar reminder: the event's title, and when and where.
pub fn reminder(uid: &str, title: &str, body: &str) {
    let hiding = crate::manage::applock::hiding();
    show(
        format!("reminder:{uid}"),
        if hiding { "Upcoming event".to_owned() } else { title.to_owned() },
        "Calendar".to_owned(),
        if hiding { String::new() } else { body.to_owned() },
        None,
    );
}

/// `key` names the notification where the system can replace one;
/// `target` is where tapping it goes.
fn show(key: String, title: String, account: String, body: String, target: Option<Target>) {
    show_with(key, title, account, body, target, &[]);
}

/// As `show`, with `buttons` (action keys and labels) for acting on the
/// messages it tells of.
fn show_with(
    key: String,
    title: String,
    account: String,
    body: String,
    target: Option<Target>,
    buttons: &[(&'static str, &'static str)],
) {
    let buttons: Vec<(&'static str, &'static str)> = if target.is_some() { buttons.to_vec() } else { Vec::new() };
    // Android's buttons each carry their own target: Reply opens the app
    // with it, the others reach the service (`Native.quick`).
    #[cfg(target_os = "android")]
    let actions: String = target
        .as_ref()
        .map(|t| {
            buttons
                .iter()
                .filter_map(|(key, label)| {
                    let mut t = t.clone();
                    t.action = Some((*key).to_owned());
                    Some(format!("{key}\t{label}\t{}", serde_json::to_string(&t).ok()?))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let target = target
        .and_then(|t| serde_json::to_string(&t).ok())
        .unwrap_or_default();
    #[cfg(target_os = "android")]
    {
        let _ = buttons;
        std::thread::spawn(move || {
            crate::android::notify(&key, &title, &body, &account, &target, &actions)
        });
    }
    #[cfg(not(target_os = "android"))]
    std::thread::spawn(move || {
        let _ = key;
        // Each notification waiting for a click holds this thread (and, on
        // Linux, a D-Bus connection) until it is clicked or dismissed, which
        // a notification center can put off for days. At most `WAITING` wait;
        // past them a notification still shows, without opening on a click.
        #[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
        let waiting = Waiting::take();
        #[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
        let target = if waiting.is_some() { target } else { String::new() };
        #[cfg(target_os = "macos")]
        {
            static APP: std::sync::Once = std::sync::Once::new();
            APP.call_once(|| {
                // Outside the app bundle macOS has no identity to show; keep the default.
                if std::env::current_exe()
                    .is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS"))
                {
                    let _ = notify_rust::set_application("tech.pinal.picacho");
                }
            });
        }
        let mut note = notify_rust::Notification::new();
        // The icon installed with the desktop entry; see `desktop`.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        note.icon(super::desktop::APP_ID);
        // Shown as Picacho's (its name and icon), not PowerShell's, the
        // default for apps without one; see `windows::set_app_identity`.
        #[cfg(windows)]
        note.app_id(super::windows::APP_ID);
        // Clicking the notification is its default action.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if !target.is_empty() {
            note.action("default", "Open");
        }
        // Buttons only where a press can be heard (`Waiting`).
        #[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
        if !target.is_empty() {
            for (key, label) in &buttons {
                note.action(key, label);
            }
        }
        // macOS shows several as a menu on one button.
        #[cfg(target_os = "macos")]
        if buttons.len() > 1 {
            use notify_rust::NsNotificationExt;
            note.actions_label("Actions");
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos")))]
        let _ = buttons;
        let shown = note
            .appname("Picacho")
            .summary(&title)
            .subtitle(&account)
            .body(&body)
            .show();
        // Waits (on this thread) until it is clicked or closed. On macOS the
        // notification is delivered by this wait (or, without a target, as
        // its handle is dropped); a click on it is "default".
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
        if let Ok(handle) = shown
            && !target.is_empty()
        {
            handle.wait_for_action(|action| {
                if action == "default" {
                    clicked(&target);
                } else if is_quick(action) {
                    quick(&target, action);
                }
            });
        }
        // A click on the toast's body is its default response; the toast
        // calls back while it is shown, and while this process runs.
        #[cfg(windows)]
        if let Ok(handle) = shown
            && !target.is_empty()
        {
            let _ = handle.wait_for_response(|response: &notify_rust::NotificationResponse| {
                match response {
                    notify_rust::NotificationResponse::Default => clicked(&target),
                    notify_rust::NotificationResponse::Action(action) if is_quick(action) => {
                        quick(&target, action)
                    }
                    _ => {}
                }
            });
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos")))]
        let _ = (shown, target);
    });
}

/// A place among the notifications waiting for a click (see `show`).
#[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
struct Waiting;
#[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
static WAITING_NOW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
impl Waiting {
    const MOST: usize = 32;
    fn take() -> Option<Self> {
        use std::sync::atomic::Ordering;
        WAITING_NOW
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < Self::MOST).then_some(n + 1))
            .ok()
            .map(|_| Waiting)
    }
}
#[cfg(any(target_os = "linux", target_os = "freebsd", windows, target_os = "macos"))]
impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING_NOW.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_name_every_message_and_older_ones_still_read() {
        let mut t = Target::new("a", Folder::Inbox, Some("m3"));
        assert_eq!(t.message_ids(), vec!["m3".to_string()]);
        t.messages = vec!["m1".into(), "m2".into(), "m3".into()];
        assert_eq!(t.message_ids().len(), 3);
        t.action = Some("reply".into());
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Target>(&json).unwrap(), t);
        // As a notification shown by an earlier version carries it.
        // Without them it's written as before, and read either way.
        let plain = serde_json::to_string(&Target::new("a", Folder::Inbox, Some("m"))).unwrap();
        assert!(!plain.contains("messages") && !plain.contains("action"), "{plain}");
        let old: Target = serde_json::from_str(&plain).unwrap();
        assert!(old.messages.is_empty() && old.action.is_none());
        assert_eq!(quick_change("archive"), Some(Change::Move(Folder::Archive)));
        assert_eq!(quick_change("reply"), None);
        assert!(is_quick("trash") && !is_quick("default"));
    }

}
