//! Presentation-neutral synchronization status shared with Display.
/// A snapshot of background body downloads. Counts come from the local cache;
/// the coordinator owns task lifetime, while Display chooses how to draw it.
#[derive(Default)]
pub struct DownloadStatus {
    pub total: usize,
    pub missing: usize,
    pub active: bool,
    pub paused: bool,
}

/// Typed boundaries keep transport failures distinct from rejected credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessFailure {
    Keychain,
    Authentication,
}
impl std::fmt::Display for AccessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keychain => "Cannot access saved credentials",
            Self::Authentication => "Sign-in needs attention",
        })
    }
}
impl std::error::Error for AccessFailure {}

/// The server is limiting this account (Gmail: bandwidth or connections).
/// Background work for the account waits until `until` (Unix seconds);
/// explicit refreshes and opening messages still try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throttled {
    pub until: i64,
    /// Picacho's own daily allowance, rather than the server, stopped it.
    pub budget: bool,
}
impl std::fmt::Display for Throttled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let local = chrono::DateTime::from_timestamp(self.until, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%-I:%M %p")
                    .to_string()
            })
            .unwrap_or_default();
        if self.budget {
            write!(
                f,
                "Today's background download allowance for this account is used up; it resumes at {local}"
            )
        } else {
            write!(
                f,
                "The server is limiting this account; background sync resumes at {local}"
            )
        }
    }
}
impl std::error::Error for Throttled {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IssueSource {
    Sync(String),
    Download,
    Read,
    Change,
    Send,
}
impl IssueSource {
    pub fn label(&self) -> &str {
        match self {
            Self::Sync(_) => "Checking mail",
            Self::Download => "Downloading messages",
            Self::Read => "Opening a message",
            Self::Change => "Updating messages",
            Self::Send => "Sending mail",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountIssue {
    pub access: Option<AccessFailure>,
    pub throttled: bool,
    pub details: String,
    pub occurred_at: i64,
}
impl AccountIssue {
    pub fn new(error: &anyhow::Error) -> Self {
        Self {
            access: error.downcast_ref::<AccessFailure>().copied(),
            throttled: error.downcast_ref::<Throttled>().is_some(),
            details: format!("{error:#}"),
            occurred_at: chrono::Utc::now().timestamp(),
        }
    }
    pub fn guidance(&self, source: &IssueSource) -> &'static str {
        if *source == IssueSource::Send {
            return "Your draft has been kept. Check your sign-in, recipient, and outgoing server settings before sending again.";
        }
        if self.throttled && self.access.is_none() {
            return "The server is limiting how much this account can download, which Gmail does after heavy use. Background checks and downloads pause and resume on their own; cached mail stays available, and opening a message still works.";
        }
        match self.access {
            Some(AccessFailure::Keychain) => {
                "Allow Picacho to access your system keychain, then choose Try Again. If the saved credentials are missing, sign in again. Automatic checks are paused for this mailbox."
            }
            Some(AccessFailure::Authentication) => {
                "The server could not accept your sign-in. Sign in again with your provider or update your app password. Automatic checks are paused for this mailbox."
            }
            None => {
                "Check your connection and server settings, then try again. Your cached mail remains available. Automatic checks will retry on this mailbox's schedule."
            }
        }
    }
}
