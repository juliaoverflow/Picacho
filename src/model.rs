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
            "OAuth requires your provider's desktop application client ID."
        );
        anyhow::ensure!(
            !self.oauth || matches!(self.provider, Provider::Gmail | Provider::Outlook),
            "OAuth is available for Gmail and Outlook."
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Folder {
    Inbox,
    Starred,
    Sent,
    Drafts,
    Archive,
    Junk,
    Trash,
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
        }
    }
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
}
impl Mail {
    pub fn initials(&self) -> String {
        initials(&self.sender)
    }
    pub fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        [&self.sender, &self.from, &self.subject, &self.snippet]
            .iter()
            .any(|s| s.to_lowercase().contains(&query))
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
        })
        .collect()
}
