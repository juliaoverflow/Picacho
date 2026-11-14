use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Gmail,
    Outlook,
    Imap,
    Pop3,
}
impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::Outlook => "Outlook",
            Self::Imap => "IMAP",
            Self::Pop3 => "POP3",
        }
    }
}
/// The mail services offered when adding an account. `Provider` is the
/// protocol behaviour; a service picks one along with its servers and the
/// simplest way to sign in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Service {
    Pinal,
    Gmail,
    Outlook,
    ICloud,
    Fastmail,
    Zoho,
    ProtonBridge,
    /// Any other server, found by autoconfiguration or entered by hand.
    #[default]
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Pinal Accounts in the browser; the server issues an app password.
    Pinal,
    /// The provider's OAuth consent in the browser.
    Browser,
    /// A password made for mail apps, as named by `password_label`.
    Password,
}
impl Service {
    /// The services offered when adding an account, in order.
    pub const PICKER: &'static [Self] = &[
        #[cfg(feature = "pinal")]
        Self::Pinal,
        Self::Gmail,
        Self::Outlook,
        Self::ICloud,
        Self::Fastmail,
        Self::Zoho,
        Self::ProtonBridge,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinal => "Pinal Mail",
            Self::Gmail => "Gmail",
            Self::Outlook => "Outlook",
            Self::ICloud => "iCloud Mail",
            Self::Fastmail => "Fastmail",
            Self::Zoho => "Zoho Mail",
            Self::ProtonBridge => "Proton Mail",
            Self::Other => "Other",
        }
    }
    pub fn provider(self) -> Provider {
        match self {
            Self::Gmail => Provider::Gmail,
            Self::Outlook => Provider::Outlook,
            _ => Provider::Imap,
        }
    }
    pub fn sign_in(self) -> SignIn {
        match self {
            Self::Pinal => SignIn::Pinal,
            Self::Gmail | Self::Outlook if crate::sync::oauth::configured(self.provider()) => {
                SignIn::Browser
            }
            _ => SignIn::Password,
        }
    }
    pub fn password_label(self) -> &'static str {
        match self {
            Self::Gmail | Self::Outlook | Self::Fastmail | Self::Zoho => "App password",
            Self::ICloud => "App-specific password",
            Self::ProtonBridge => "Bridge password",
            _ => "Password",
        }
    }
    /// Why this build can't offer browser sign-in for a service that
    /// usually has it: the app was compiled without that provider's OAuth
    /// registration, so an app password is the only way in.
    pub fn browser_unavailable(self) -> Option<&'static str> {
        if crate::sync::oauth::configured(self.provider()) {
            return None;
        }
        match self {
            Self::Gmail => Some(
                "This copy of Picacho was built without Google sign-in, so an app password is the only way to connect Gmail.",
            ),
            Self::Outlook => Some(
                "This copy of Picacho was built without Microsoft sign-in, so an app password is the only way to connect Outlook.",
            ),
            _ => None,
        }
    }
    /// Where the password comes from, with a page that makes one.
    pub fn password_help(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Gmail => Some((
                "Turn on 2-Step Verification in your Google Account, then make an app password and paste it here. Spaces are ignored.",
                "https://myaccount.google.com/apppasswords",
            )),
            Self::ICloud => Some((
                "iCloud needs an app-specific password, made in your Apple Account.",
                "https://account.apple.com/account/manage",
            )),
            Self::Fastmail => Some((
                "Fastmail needs an app password with IMAP and SMTP access.",
                "https://www.fastmail.help/hc/en-us/search?query=app+passwords",
            )),
            Self::Zoho => Some((
                "Turn on IMAP in Zoho Mail settings, then make an app password.",
                "https://accounts.zoho.com/home#security/app_password",
            )),
            Self::ProtonBridge => Some((
                "Open Proton Mail Bridge, choose your account and copy its mailbox password. Bridge must be running, with its certificate trusted.",
                "https://proton.me/support/apple-mail-certificate",
            )),
            Self::Outlook => Some((
                "Turn on two-step verification in your Microsoft account, then make an app password and paste it here. Work and school accounts can use one only if their admin allows it.",
                "https://account.live.com/proofs/AppPassword",
            )),
            _ => None,
        }
    }
    /// Fixed server settings. `None` means look them up from the address.
    pub fn account(self, email: &str) -> Option<Account> {
        let mut a = Account::new(self.provider());
        a.service = self;
        a.email = email.into();
        let (host, port, smtp, smtp_port) = match self {
            Self::Gmail | Self::Outlook => return Some(a),
            Self::Pinal => ("mail.sonoran.exchange", 993, "mail.sonoran.exchange", 465),
            Self::ICloud => ("imap.mail.me.com", 993, "smtp.mail.me.com", 587),
            Self::Fastmail => ("imap.fastmail.com", 993, "smtp.fastmail.com", 465),
            Self::Zoho => {
                // Zoho keeps each region's mail on its own servers.
                let domain = crate::sync::discover::domain(email).unwrap_or_default();
                let region = ["eu", "in", "com.au", "jp", "ca", "sa", "com.cn"]
                    .into_iter()
                    .find(|r| domain == format!("zoho.{r}") || domain == format!("zohomail.{r}"))
                    .unwrap_or("com");
                a.incoming_host = format!("imap.zoho.{region}");
                a.smtp_host = format!("smtp.zoho.{region}");
                a.smtp_port = 465;
                return Some(a);
            }
            Self::ProtonBridge => {
                a.incoming_starttls = true;
                ("127.0.0.1", 1143, "127.0.0.1", 1025)
            }
            Self::Other => return None,
        };
        a.incoming_host = host.into();
        a.incoming_port = port;
        a.smtp_host = smtp.into();
        a.smtp_port = smtp_port;
        Some(a)
    }
}
/// Per-mailbox sync preferences, edited in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSettings {
    /// Only mail received in the last this many days is synced; 0 syncs all.
    pub days: u32,
    /// Minutes between checks for new mail; 0 checks only on request.
    pub refresh_minutes: u32,
    pub notify: bool,
    /// Message contents from the last this many days download in the
    /// background; 0 downloads all of them, `OFFLINE_WHEN_OPENED` none.
    /// Anything else downloads when it is opened.
    #[serde(default = "default_offline_days")]
    pub offline_days: u32,
}
/// `SyncSettings::offline_days`: download message contents only when opened.
pub const OFFLINE_WHEN_OPENED: u32 = u32::MAX;
fn default_offline_days() -> u32 {
    30
}
impl SyncSettings {
    /// The oldest date whose contents download in the background, if any do.
    pub fn offline_since(&self, now: i64) -> Option<i64> {
        match self.offline_days {
            OFFLINE_WHEN_OPENED => None,
            0 => Some(i64::MIN),
            days => Some(now - i64::from(days) * 86_400),
        }
    }
}
impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            days: 0,
            refresh_minutes: 5,
            notify: true,
            offline_days: default_offline_days(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub name: String,
    /// A name the person chose, shown instead of `name` in the sidebar and Settings.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nickname: String,
    pub provider: Provider,
    pub incoming_host: String,
    pub incoming_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub oauth: bool,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub service: Service,
    /// Upgrade a plain IMAP connection with STARTTLS instead of connecting
    /// with TLS. Only Proton Mail Bridge, on this computer, needs it.
    #[serde(default)]
    pub incoming_starttls: bool,
    #[serde(default)]
    pub sync: SyncSettings,
}
impl Account {
    pub fn new(provider: Provider) -> Self {
        let (host, smtp) = match provider {
            Provider::Gmail => ("imap.gmail.com", "smtp.gmail.com"),
            Provider::Outlook => ("outlook.office365.com", "smtp.office365.com"),
            _ => ("", ""),
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            email: String::new(),
            name: String::new(),
            nickname: String::new(),
            provider,
            incoming_host: host.into(),
            incoming_port: if provider == Provider::Pop3 { 995 } else { 993 },
            smtp_host: smtp.into(),
            smtp_port: 587,
            oauth: provider == Provider::Outlook,
            client_id: String::new(),
            client_secret: String::new(),
            service: match provider {
                Provider::Gmail => Service::Gmail,
                Provider::Outlook => Service::Outlook,
                _ => Service::Other,
            },
            incoming_starttls: false,
            sync: SyncSettings::default(),
        }
    }
    /// The name shown for the account: its nickname, if it has one.
    pub fn label(&self) -> &str {
        if self.nickname.is_empty() {
            &self.name
        } else {
            &self.nickname
        }
    }
    /// How the account appears in Settings, e.g. "iCloud Mail · IMAP".
    pub fn service_label(&self) -> &'static str {
        match self.service {
            Service::Other => self.provider.label(),
            s => s.label(),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.email.parse::<lettre::Address>().is_ok(),
            "Enter a valid email address."
        );
        for host in [&self.incoming_host, &self.smtp_host] {
            anyhow::ensure!(
                !host.is_empty() && !host.contains(['\r', '\n', ' ', '/', ':']),
                "Enter valid incoming and SMTP server hostnames."
            );
        }
        anyhow::ensure!(
            self.incoming_port > 0 && self.smtp_port > 0,
            "Ports must be between 1 and 65535."
        );
        anyhow::ensure!(
            !self.oauth || !self.client_id.is_empty(),
            "Browser sign-in isn't set up for {} in this build.",
            self.provider.label()
        );
        anyhow::ensure!(
            !self.oauth || matches!(self.provider, Provider::Gmail | Provider::Outlook),
            "OAuth is available for Gmail and Outlook."
        );
        // STARTTLS is only accepted from Bridge on this computer, where the
        // first, unencrypted exchange never leaves the machine.
        anyhow::ensure!(
            !self.incoming_starttls
                || matches!(self.incoming_host.as_str(), "127.0.0.1" | "localhost"),
            "STARTTLS is only used with Proton Mail Bridge on this computer."
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Folder {
    #[default]
    Inbox,
    Starred,
    Sent,
    Drafts,
    Archive,
    Junk,
    Trash,
    /// A server folder outside the standard set. `Mail::remote_folder` names it.
    Other,
}
impl Folder {
    pub const ALL: [Self; 7] = [
        Self::Inbox,
        Self::Starred,
        Self::Sent,
        Self::Drafts,
        Self::Archive,
        Self::Junk,
        Self::Trash,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "Inbox",
            Self::Starred => "Starred",
            Self::Sent => "Sent",
            Self::Drafts => "Drafts",
            Self::Archive => "Archive",
            Self::Junk => "Spam",
            Self::Trash => "Trash",
            Self::Other => "Folder",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Starred => "starred",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::Archive => "archive",
            Self::Junk => "junk",
            Self::Trash => "trash",
            Self::Other => "other",
        }
    }
}

/// A mailbox view: one of the standard folders, or a server folder by name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mailbox {
    Standard(Folder),
    Remote(String),
}
impl Mailbox {
    pub fn key(&self) -> String {
        match self {
            Self::Standard(f) => f.key().into(),
            Self::Remote(name) => format!("remote:{name}"),
        }
    }
    pub fn folder(&self) -> Folder {
        match self {
            Self::Standard(f) => *f,
            Self::Remote(_) => Folder::Other,
        }
    }
}
impl From<Folder> for Mailbox {
    fn from(f: Folder) -> Self {
        Self::Standard(f)
    }
}
impl From<&Mailbox> for Mailbox {
    fn from(m: &Mailbox) -> Self {
        m.clone()
    }
}

/// A server folder that is not one of the standard mailboxes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFolder {
    pub name: String,
    pub label: String,
    pub depth: usize,
    /// Whether the server allows folders inside this one.
    #[serde(default)]
    pub nestable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub mime: String,
    pub part: usize,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub account: String,
    pub folder: Folder,
    pub remote_folder: String,
    pub uid: u32,
    pub from: String,
    pub sender: String,
    pub to: String,
    /// Copied recipients; empty in cache entries from before it was kept.
    #[serde(default)]
    pub cc: String,
    /// Where replies go, when the sender asked for somewhere else (alias
    /// services and mailing lists do). Empty in older cache entries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to: String,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub unread: bool,
    pub starred: bool,
    pub size: u32,
    pub body: Option<String>,
    /// Original decoded HTML; absent in header-only and older cache entries.
    #[serde(default)]
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub message_id: String,
    /// The thread's earlier message ids (References, or In-Reply-To).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub references: String,
    #[serde(default)]
    pub has_attachments: bool,
    /// Gmail's permanent id, shared by the copies under each label; 0 elsewhere.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gmail_id: u64,
    /// The List-Unsubscribe header of mail from a subscription: where to
    /// ask to stop it (mailto: and https: addresses in angle brackets).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub list_unsubscribe: String,
    /// The sender accepts a one-click unsubscribe (RFC 8058).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub one_click: bool,
    /// A row the list knows exists on the server but has not listed yet.
    /// Never stored; the list draws it as a placeholder until it loads.
    #[serde(skip)]
    pub pending: bool,
    /// The parts its HTML shows in place (`cid:` images), found when the
    /// message is read. Never stored: the downloaded source holds them.
    #[serde(skip)]
    pub inline: std::sync::Arc<InlineParts>,
}
fn is_zero(v: &u64) -> bool {
    *v == 0
}
/// A message's parts that its HTML refers to by Content-ID (`cid:`), such
/// as the pictures in a newsletter sent with its images attached.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct InlineParts {
    /// By Content-ID, lowercase, without its angle brackets.
    parts: std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>,
}
impl InlineParts {
    /// Whether `html` shows parts of its message (`cid:` images).
    pub fn wanted_by(html: &str) -> bool {
        html.as_bytes()
            .windows(4)
            .any(|w| w.eq_ignore_ascii_case(b"cid:"))
    }
    /// At most this much is kept per message.
    pub const MAX_BYTES: usize = 32 * 1024 * 1024;
    /// Adds a part under its Content-ID header (`<id>`). Returns false,
    /// adding nothing, once the message's parts would grow too large.
    pub fn insert(&mut self, content_id: &str, bytes: Vec<u8>) -> bool {
        let id = content_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim()
            .to_lowercase();
        let total: usize = self.parts.values().map(|b| b.len()).sum();
        if id.is_empty() || total + bytes.len() > Self::MAX_BYTES {
            return false;
        }
        self.parts.insert(id, std::sync::Arc::new(bytes));
        true
    }
    /// The part a `cid:` URL names, if the message has it. The URL's id
    /// may be percent-encoded (RFC 2392).
    pub fn get(&self, url: &str) -> Option<std::sync::Arc<Vec<u8>>> {
        let id = url.get(..4)?.eq_ignore_ascii_case("cid:").then(|| &url[4..])?;
        self.parts.get(&percent_decoded(id).trim().to_lowercase()).cloned()
    }
}
/// `%XX` escapes decoded; anything else, including a malformed escape, kept.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
impl std::fmt::Debug for InlineParts {
    // The ids, not megabytes of image data.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.parts.keys()).finish()
    }
}
impl Mail {
    /// Whether the HTML shows parts of the message that haven't been read
    /// from its source yet; the cache keeps the HTML without them.
    pub fn lacks_inline_parts(&self) -> bool {
        self.inline.parts.is_empty() && self.html.as_deref().is_some_and(InlineParts::wanted_by)
    }
    /// Newest first, ties broken by id, so every list of the same messages
    /// comes out in the same order (as SQLite's `date DESC, id DESC`).
    pub fn newest_first(a: &Mail, b: &Mail) -> std::cmp::Ordering {
        b.date.cmp(&a.date).then_with(|| b.id.cmp(&a.id))
    }
    /// A row for a message the server has that is not listed here yet.
    pub fn placeholder(
        id: String,
        account: &str,
        mailbox: &Mailbox,
        remote: &str,
        uid: u32,
    ) -> Self {
        Self {
            id,
            account: account.into(),
            folder: mailbox.folder(),
            remote_folder: remote.into(),
            uid,
            from: String::new(),
            sender: String::new(),
            to: String::new(),
            pending: true,
            ..Self::default()
        }
    }
    pub fn mailbox(&self) -> Mailbox {
        if self.folder == Folder::Other {
            Mailbox::Remote(self.remote_folder.clone())
        } else {
            Mailbox::Standard(self.folder)
        }
    }
    pub fn has_attachments(&self) -> bool {
        self.has_attachments || !self.attachments.is_empty()
    }
    pub fn initials(&self) -> String {
        initials(&self.sender)
    }
}
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|s| s.chars().next())
        .collect::<String>()
        .to_uppercase()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Secret {
    pub password: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default)]
    pub client_secret: String,
    /// Pinal Mail's key bundle (JSON). Every secret in it is ciphertext.
    #[serde(default)]
    pub mail_bundle: String,
    /// The unwrapped Pinal mail key once this device was unlocked; it can
    /// always be rebuilt from the password, so the keychain is only a cache.
    #[serde(default)]
    pub mail_key: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    /// The account it is sent from.
    pub account: String,
    /// An alias (at a forwarding service that can send) the message is sent
    /// as, through `account`; empty to send as the account itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub send_as: String,
    pub to: String,
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub bcc: String,
    pub subject: String,
    /// Plain text, or Markdown when `formatted`.
    pub body: String,
    /// The body is Markdown and is sent with an HTML version.
    #[serde(default)]
    pub formatted: bool,
    pub reply_id: String,
    /// The thread's message ids, oldest first, for the References header.
    #[serde(default)]
    pub references: String,
    pub attachments: Vec<std::path::PathBuf>,
}
impl Draft {
    /// Whether anything has been written: an untouched draft is not kept.
    pub fn is_empty(&self) -> bool {
        self.to.trim().is_empty()
            && self.cc.trim().is_empty()
            && self.bcc.trim().is_empty()
            && self.subject.trim().is_empty()
            && self.body.trim().is_empty()
            && self.attachments.is_empty()
    }
}

#[cfg(any(test, feature = "sample-data"))]
pub fn demo_mail() -> Vec<Mail> {
    let examples = [
        (
            "Alex Morgan",
            "alex@studio.design",
            "A little room for something great",
            "Hey there,\n\nI've been thinking about our conversation yesterday. There's something exciting about starting with a blank canvas — a little space to slow down, explore, and build something that feels right.\n\nI've put together a few ideas for the next chapter. The direction is simple: less noise, more intention. Thoughtful details, a clearer story, and room for the work to speak for itself.\n\nWould love to hear what you think. How does a quick catch-up on Monday sound?\n\nTalk soon,\nAlex",
            true,
        ),
        (
            "Linear",
            "updates@linear.app",
            "Your team's week, at a glance",
            "Your weekly workspace digest is here. See what shipped and what's coming next.",
            true,
        ),
        (
            "Sophie Chen",
            "sophie@example.com",
            "Re: Weekend plans",
            "That sounds perfect! Let's meet at the usual place around 10. Coffee is on me this time.",
            false,
        ),
        (
            "Figma",
            "news@figma.com",
            "Made for your next big idea",
            "A fresh set of tools to help you bring your next idea to life. Here's what's new.",
            true,
        ),
        (
            "Daniel Kim",
            "daniel@example.com",
            "Re: Weekend plans",
            "Finally got around to sorting through the photos from the trip. Such a good weekend — we need to do it again.\n\nOn Fri, Sophie Chen wrote:\n> That sounds perfect! Let's meet at the usual place around 10.\n>\n> On Thu, Daniel Kim wrote:\n>> Are we still on for Saturday?",
            false,
        ),
        (
            "Notion",
            "team@makenotion.com",
            "A calmer place for your work",
            "Your notes, projects, and ideas. All together in one beautifully simple workspace.",
            false,
        ),
        (
            "Emma Wilson",
            "emma@example.com",
            "The first draft is ready",
            "Hi! Sharing the first pass of the proposal. I've included your feedback from last week.",
            false,
        ),
        (
            "GitHub",
            "notifications@github.com",
            "All checks have passed",
            "Your pull request is ready for review. All automated checks completed successfully.",
            false,
        ),
        (
            "Oliver James",
            "oliver@example.com",
            "Something I thought you'd like",
            "Came across this little studio and immediately thought of you. Their attention to detail is incredible.",
            false,
        ),
        (
            "The Browser",
            "hello@thebrowser.com",
            "Five things worth your time",
            "A small collection of good reads for a quiet afternoon. Enjoy the weekend.",
            false,
        ),
    ];
    examples
        .iter()
        .enumerate()
        .map(|(i, (name, from, subject, body, unread))| Mail {
            id: format!("demo-{i}"),
            account: "demo".into(),
            folder: Folder::Inbox,
            remote_folder: "INBOX".into(),
            uid: i as u32,
            from: (*from).into(),
            sender: (*name).into(),
            to: "you@example.com".into(),
            subject: (*subject).into(),
            snippet: body
                .lines()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            date: chrono::Utc::now().timestamp() - i as i64 * 3900,
            unread: *unread,
            starred: i == 0 || i == 4,
            size: body.len() as u32,
            body: Some((*body).into()),
            html: None,
            attachments: vec![],
            // One sample message copies others, to show Cc and Reply All.
            cc: if i == 2 {
                "Daniel Kim <daniel@example.com>, Emma Wilson <emma@example.com>".into()
            } else {
                String::new()
            },
            references: String::new(),
            message_id: format!("<demo-{i}@picacho.example>"),
            has_attachments: i == 2,
            gmail_id: 0,
            list_unsubscribe: if *from == "hello@thebrowser.com" {
                "<mailto:unsubscribe@thebrowser.com?subject=unsubscribe>".into()
            } else {
                String::new()
            },
            one_click: false,
            pending: false,
            reply_to: String::new(),
            inline: Default::default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_parts_match_their_cid_addresses() {
        let mut parts = InlineParts::default();
        assert!(parts.insert(" <Scan 1@USPS.com> ", vec![1, 2, 3]));
        // Brackets, case and percent-encoding don't matter (RFC 2392).
        assert_eq!(parts.get("cid:scan%201@usps.com").as_deref(), Some(&vec![1, 2, 3]));
        assert_eq!(parts.get("CID:Scan 1@USPS.com").as_deref(), Some(&vec![1, 2, 3]));
        assert!(parts.get("cid:other@usps.com").is_none());
        assert!(parts.get("https://usps.com/scan 1").is_none());
        assert!(!parts.insert("<>", vec![1]), "an empty id names nothing");
        // A message can't hold more than the limit in memory.
        assert!(!parts.insert("<big>", vec![0; InlineParts::MAX_BYTES]));
        assert!(InlineParts::wanted_by("<img src=\"CID:x\">"));
        assert!(!InlineParts::wanted_by("<img src=\"https://x\">"));
    }
}
