//! The languages Picacho has, and their messages, built into the app.

/// The language messages are written in first, and shown where a
/// translation lacks one.
pub const SOURCE: &str = "en";

/// Each language's code (BCP 47) and its files: name and text, as in
/// `locales/<code>/`.
pub const LANGUAGES: &[(&str, &[(&str, &str)])] = &[("en", EN)];

/// Each language's name, in that language, for Settings.
pub const NAMES: [(&str, &str); 1] = [("en", "English")];

/// English, the source (`locales/en`).
const EN: &[(&str, &str)] = &[
    // @generated-en-files (the list below is kept sorted)
    ("accounts.ftl", include_str!("../../locales/en/accounts.ftl")),
    ("app.ftl", include_str!("../../locales/en/app.ftl")),
    ("applock.ftl", include_str!("../../locales/en/applock.ftl")),
    ("bimi.ftl", include_str!("../../locales/en/bimi.ftl")),
    ("biometric.ftl", include_str!("../../locales/en/biometric.ftl")),
    ("calendar-views.ftl", include_str!("../../locales/en/calendar-views.ftl")),
    ("calendar.ftl", include_str!("../../locales/en/calendar.ftl")),
    ("categories.ftl", include_str!("../../locales/en/categories.ftl")),
    ("cloud-settings.ftl", include_str!("../../locales/en/cloud-settings.ftl")),
    ("compose.ftl", include_str!("../../locales/en/compose.ftl")),
    ("composer.ftl", include_str!("../../locales/en/composer.ftl")),
    ("contacts.ftl", include_str!("../../locales/en/contacts.ftl")),
    ("dates.ftl", include_str!("../../locales/en/dates.ftl")),
    ("datetime.ftl", include_str!("../../locales/en/datetime.ftl")),
    ("desktop.ftl", include_str!("../../locales/en/desktop.ftl")),
    ("encryption.ftl", include_str!("../../locales/en/encryption.ftl")),
    ("filters.ftl", include_str!("../../locales/en/filters.ftl")),
    ("follow-up.ftl", include_str!("../../locales/en/follow-up.ftl")),
    ("forwarding.ftl", include_str!("../../locales/en/forwarding.ftl")),
    ("html.ftl", include_str!("../../locales/en/html.ftl")),
    ("inbox-control.ftl", include_str!("../../locales/en/inbox-control.ftl")),
    ("jmap-extras.ftl", include_str!("../../locales/en/jmap-extras.ftl")),
    ("later.ftl", include_str!("../../locales/en/later.ftl")),
    ("links.ftl", include_str!("../../locales/en/links.ftl")),
    ("local-ai.ftl", include_str!("../../locales/en/local-ai.ftl")),
    ("manage.ftl", include_str!("../../locales/en/manage.ftl")),
    ("masked-email.ftl", include_str!("../../locales/en/masked-email.ftl")),
    ("message-window.ftl", include_str!("../../locales/en/message-window.ftl")),
    ("mobile.ftl", include_str!("../../locales/en/mobile.ftl")),
    ("motion.ftl", include_str!("../../locales/en/motion.ftl")),
    ("notify.ftl", include_str!("../../locales/en/notify.ftl")),
    ("open-with.ftl", include_str!("../../locales/en/open-with.ftl")),
    ("organize.ftl", include_str!("../../locales/en/organize.ftl")),
    ("outgoing.ftl", include_str!("../../locales/en/outgoing.ftl")),
    ("panes.ftl", include_str!("../../locales/en/panes.ftl")),
    ("platform.ftl", include_str!("../../locales/en/platform.ftl")),
    ("proton-login.ftl", include_str!("../../locales/en/proton-login.ftl")),
    ("reader.ftl", include_str!("../../locales/en/reader.ftl")),
    ("receipts.ftl", include_str!("../../locales/en/receipts.ftl")),
    ("recovery.ftl", include_str!("../../locales/en/recovery.ftl")),
    ("security-keys.ftl", include_str!("../../locales/en/security-keys.ftl")),
    ("settings.ftl", include_str!("../../locales/en/settings.ftl")),
    ("shortcuts.ftl", include_str!("../../locales/en/shortcuts.ftl")),
    ("subscriptions.ftl", include_str!("../../locales/en/subscriptions.ftl")),
    ("summary.ftl", include_str!("../../locales/en/summary.ftl")),
    ("sync.ftl", include_str!("../../locales/en/sync.ftl")),
    ("tabs.ftl", include_str!("../../locales/en/tabs.ftl")),
    ("templates.ftl", include_str!("../../locales/en/templates.ftl")),
    ("tools.ftl", include_str!("../../locales/en/tools.ftl")),
    ("tray.ftl", include_str!("../../locales/en/tray.ftl")),
    ("tree.ftl", include_str!("../../locales/en/tree.ftl")),
    ("ui.ftl", include_str!("../../locales/en/ui.ftl")),
    ("verify.ftl", include_str!("../../locales/en/verify.ftl")),
    ("views.ftl", include_str!("../../locales/en/views.ftl")),
    ("widget-window.ftl", include_str!("../../locales/en/widget-window.ftl")),
    ("widgets.ftl", include_str!("../../locales/en/widgets.ftl")),
    ("windows.ftl", include_str!("../../locales/en/windows.ftl")),
    ("window.ftl", include_str!("../../locales/en/window.ftl")),
    ("workspaces.ftl", include_str!("../../locales/en/workspaces.ftl")),
];

/// The text of `language`'s files.
pub fn files(language: &str) -> impl Iterator<Item = &'static str> {
    LANGUAGES
        .iter()
        .find(|(code, _)| *code == language)
        .into_iter()
        .flat_map(|(_, files)| files.iter().map(|(_, text)| *text))
}
