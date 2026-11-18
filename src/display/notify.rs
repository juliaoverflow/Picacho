//! System notifications for new mail. Failures are ignored: a missing or
//! disabled notification service must never interrupt mail. Android's go
//! through the app's Java side (`android::notify`); elsewhere notify-rust's.

/// Announces new messages in one account. `latest` is (sender, subject).
pub fn new_mail(account: &str, latest: (&str, &str), count: usize) {
    let (title, body) = if count == 1 {
        (latest.0.to_owned(), latest.1.to_owned())
    } else {
        (
            format!("{count} new messages"),
            format!("{} — {}", latest.0, latest.1),
        )
    };
    // A newer announcement for the account replaces the last (Android).
    show(format!("mail:{account}"), title, account.to_owned(), body);
}

/// Shows what the background monitor has to tell (see
/// `manage::background`).
pub fn announce(announcement: crate::manage::background::Announcement<'_>) {
    use crate::manage::background::Announcement::*;
    match announcement {
        NewMail {
            account,
            sender,
            subject,
            count,
        } => new_mail(account, (sender, subject), count),
        BackFromSnooze {
            account,
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
            );
        }
        ScheduledSent {
            account,
            to,
            subject,
        } => show(
            format!("sent:{account}"),
            "Scheduled message sent".to_owned(),
            account.to_owned(),
            format!("{} — to {to}", or_untitled(subject)),
        ),
        ScheduledFailed {
            account,
            draft,
            subject,
            error,
        } => show(
            format!("unsent:{draft}"),
            "A scheduled message wasn't sent".to_owned(),
            account.to_owned(),
            format!("{}: {error} It's in Drafts.", or_untitled(subject)),
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
/// sends its apps' users), titled as the provider sent it.
pub fn account_notice(account: &str, title: &str, body: &str) {
    show(
        format!("notice:{account}:{title}"),
        title.to_owned(),
        account.to_owned(),
        body.to_owned(),
    );
}

/// `key` names the notification where the system can replace one.
fn show(key: String, title: String, account: String, body: String) {
    #[cfg(target_os = "android")]
    std::thread::spawn(move || crate::android::notify(&key, &title, &body, &account));
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
        let _ = note
            .appname("Picacho")
            .summary(&title)
            .subtitle(&account)
            .body(&body)
            .show();
    });
}
