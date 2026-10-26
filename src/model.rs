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
    pub const PICKER: [Self; 7] = [
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
            Self::Gmail | Self::Outlook if crate::oauth::configured(self.provider()) => {
                SignIn::Browser
            }
            _ => SignIn::Password,
        }
    }
    pub fn password_label(self) -> &'static str {
        match self {
            Self::Gmail | Self::Fastmail | Self::Zoho => "App password",
            Self::ICloud => "App-specific password",
            Self::ProtonBridge => "Bridge password",
            _ => "Password",
        }
    }
    /// One sentence on where the password comes from, with a page that makes one.
    pub fn password_help(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Gmail => Some((
                "Gmail needs an app password, made in your Google Account.",
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
                "Microsoft sign-in isn't set up in this build. Use an app password if your account allows one.",
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
                let domain = crate::discover::domain(email).unwrap_or_default();
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
}
impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            days: 0,
            refresh_minutes: 5,
            notify: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub name: String,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Folder {
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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub size: usize,
    pub mime: String,
    pub part: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub account: String,
    pub folder: Folder,
    pub remote_folder: String,
    pub uid: u32,
    pub from: String,
    pub sender: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub unread: bool,
    pub starred: bool,
    pub size: u32,
    pub body: Option<String>,
    pub attachments: Vec<Attachment>,
    pub message_id: String,
    #[serde(default)]
    pub has_attachments: bool,
}
impl Mail {
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
    /// `query` must already be lowercase.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [&self.sender, &self.from, &self.subject, &self.snippet]
                .iter()
                .any(|s| s.to_lowercase().contains(query))
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
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub account: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub reply_id: String,
    pub attachments: Vec<std::path::PathBuf>,
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
            "A few photos from the trip",
            "Finally got around to sorting through these. Such a good weekend — we need to do it again.",
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
            attachments: vec![],
            message_id: String::new(),
            has_attachments: i == 2,
        })
        .collect()
}
