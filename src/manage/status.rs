//! Presentation-neutral synchronization status shared with Display.
use crate::i18n::dates::Local as _;
/// A snapshot of background body downloads. Counts come from the local cache;
/// the coordinator owns task lifetime, while Display chooses how to draw it.
#[derive(Default)]
pub struct DownloadStatus {
    pub total: usize,
    pub missing: usize,
    pub active: bool,
    pub paused: bool,
}

/// How far a sign-in has come, so the waiting text can follow it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignInStep {
    /// Waiting for the person to finish in the browser.
    #[default]
    Browser,
    /// The browser answered; exchanging its code with the provider.
    Finishing,
    /// Signing in to the incoming mail server.
    Mailbox,
    /// Waiting for the person to pass Proton's human verification (its
    /// CAPTCHA) in the window it opened.
    Verifying,
}
impl SignInStep {
    pub fn label(self) -> &'static str {
        match self {
            Self::Browser => t!("manage-status-waiting-for-your-browser"),
            Self::Finishing => t!("manage-status-signed-in-finishing-up"),
            Self::Mailbox => t!("manage-status-connecting-to-your-mailbox"),
            Self::Verifying => t!("manage-status-finish-protons-check-in-the-window"),
        }
    }
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
            Self::Keychain => t!("manage-status-cannot-access-saved-credentials"),
            Self::Authentication => t!("manage-status-sign-in-needs-attention"),
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
                    .local(t!("dates-time"))
            })
            .unwrap_or_default();
        if self.budget {
            f.write_str(&t!("manage-status-todays-background-download-allowance", local = local))
        } else {
            f.write_str(&t!("manage-status-the-server-is-limiting-this-account", local = local))
        }
    }
}
impl std::error::Error for Throttled {}

/// Proton refused to register this device for an account's push
/// notifications, or push couldn't start. Critical, like a refused sign-in:
/// with push on, nothing else announces the account's new mail
/// (`Account::pushes_new_mail`). Unlike one, it doesn't pause mail checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushRegistration;
impl std::fmt::Display for PushRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("manage-status-proton-didnt-register-this-device-for"))
    }
}
impl std::error::Error for PushRegistration {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IssueSource {
    Sync(String),
    Download,
    Read,
    Change,
    Send,
    Push,
}
impl IssueSource {
    pub fn label(&self) -> &str {
        match self {
            Self::Sync(_) => t!("manage-status-checking-mail"),
            Self::Download => t!("manage-status-downloading-messages"),
            Self::Read => t!("manage-status-opening-a-message"),
            Self::Change => t!("manage-status-updating-messages"),
            Self::Send => t!("manage-status-sending-mail"),
            Self::Push => t!("manage-status-push-notifications"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountIssue {
    pub access: Option<AccessFailure>,
    pub throttled: bool,
    /// A `PushRegistration` failure.
    pub push: bool,
    pub details: String,
    pub occurred_at: i64,
}
impl AccountIssue {
    pub fn new(error: &anyhow::Error) -> Self {
        Self {
            access: error.downcast_ref::<AccessFailure>().copied(),
            throttled: error.downcast_ref::<Throttled>().is_some(),
            push: error.downcast_ref::<PushRegistration>().is_some(),
            details: format!("{error:#}"),
            occurred_at: chrono::Utc::now().timestamp(),
        }
    }
    /// Whether it needs the person before the account works again: its
    /// sign-in was refused, its saved credentials couldn't be read, or its
    /// push notifications couldn't be registered. Anything else (a dropped
    /// connection, a refused command, a message that wouldn't send) is a
    /// warning, and can be dismissed.
    pub fn critical(&self) -> bool {
        self.access.is_some() || self.push
    }
    pub fn guidance(&self, source: &IssueSource) -> &'static str {
        if self.push {
            return t!("manage-status-this-mailbox-gets-no-new-mail");
        }
        if *source == IssueSource::Send {
            return t!("manage-status-your-draft-has-been-kept-check");
        }
        if self.throttled && self.access.is_none() {
            return t!("manage-status-the-server-is-limiting-how-much");
        }
        match self.access {
            Some(AccessFailure::Keychain) => {
                t!("manage-status-allow-picacho-to-access-your-system")
            }
            Some(AccessFailure::Authentication) => {
                t!("manage-status-the-server-could-not-accept-your")
            }
            None => {
                t!("manage-status-check-your-connection-and-server-settings")
            }
        }
    }
}
