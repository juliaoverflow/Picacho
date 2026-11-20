//! System notifications for new mail. Failures are ignored: a missing or
//! disabled notification service must never interrupt mail. Android's go
//! through the app's Java side (`android::notify`); elsewhere notify-rust's.
//!
//! Tapping one opens what it tells of (its `Target`): the message in its
//! mailbox, or the mailbox. The tap reaches `clicked`, and the window takes
//! it with `take_clicked` (see `MailApp::open_notified`).

use crate::manage::model::{Folder, Mailbox};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Where tapping a notification goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The account's id.
    pub account: String,
    pub mailbox: Mailbox,
    /// The message to open: its id, or its Message-ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Target {
    fn new(account: &str, folder: Folder, message: Option<&str>) -> Self {
        Self {
            account: account.to_owned(),
            mailbox: folder.into(),
            message: message.filter(|m| !m.is_empty()).map(str::to_owned),
        }
    }
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
    *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(target);
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = wake.unbounded_send(());
    }
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
/// and `message` the latest's id, which tapping opens.
pub fn new_mail(
    account: &str,
    account_id: &str,
    message: &str,
    latest: (&str, &str),
    count: usize,
) {
    let (title, body) = if count == 1 {
        (latest.0.to_owned(), latest.1.to_owned())
    } else {
        (
            format!("{count} new messages"),
            format!("{} — {}", latest.0, latest.1),
        )
    };
    // A newer announcement for the account replaces the last (Android).
    show(
        format!("mail:{account}"),
        title,
        account.to_owned(),
        body,
        Some(Target::new(account_id, Folder::Inbox, Some(message))),
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
            sender,
            subject,
            count,
        } => new_mail(account, account_id, message, (sender, subject), count),
        BackFromSnooze {
            account,
            account_id,
            message_id,
            sender,
            subject,
            count,
        } => {
            let body = if count == 1 {
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
            format!("{} — to {to}", or_untitled(subject)),
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
            format!("{}: {error} It's in Drafts.", or_untitled(subject)),
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

/// `key` names the notification where the system can replace one;
/// `target` is where tapping it goes.
fn show(key: String, title: String, account: String, body: String, target: Option<Target>) {
    let target = target
        .and_then(|t| serde_json::to_string(&t).ok())
        .unwrap_or_default();
    #[cfg(target_os = "android")]
    std::thread::spawn(move || crate::android::notify(&key, &title, &body, &account, &target));
    #[cfg(not(target_os = "android"))]
    std::thread::spawn(move || {
        let _ = key;
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
        // Clicking the notification is its default action.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if !target.is_empty() {
            note.action("default", "Open");
        }
        let shown = note
            .appname("Picacho")
            .summary(&title)
            .subtitle(&account)
            .body(&body)
            .show();
        // Waits (on this thread) until it is clicked or closed.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Ok(handle) = shown
            && !target.is_empty()
        {
            handle.wait_for_action(|action| {
                if action == "default" {
                    clicked(&target);
                }
            });
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let _ = (shown, target);
    });
}
