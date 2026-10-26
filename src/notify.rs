//! System notifications for new mail. Failures are ignored: a missing or
//! disabled notification service must never interrupt mail.

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
    let account = account.to_owned();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            static APP: std::sync::Once = std::sync::Once::new();
            APP.call_once(|| {
                // Outside the app bundle macOS has no identity to show; keep the default.
                if std::env::current_exe()
                    .is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS"))
                {
                    let _ = notify_rust::set_application("com.pinal.Picacho");
                }
            });
        }
        let _ = notify_rust::Notification::new()
            .appname("Picacho")
            .summary(&title)
            .subtitle(&account)
            .body(&body)
            .show();
    });
}
