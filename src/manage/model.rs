use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Gmail,
    Outlook,
    Imap,
    Pop3,
    /// Proton's own API, without Proton Mail Bridge (`sync/proton.rs`).
    Proton,
    /// JMAP (RFC 8620/8621) over HTTPS, for reading and sending alike
    /// (`sync/jmap.rs`).
    Jmap,
}
impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::Outlook => "Outlook",
            Self::Imap => "IMAP",
            Self::Pop3 => "POP3",
            Self::Proton => "Proton API",
            Self::Jmap => "JMAP",
        }
    }
    /// Whether its mail comes over IMAP (anything but POP3, Proton's API
    /// and JMAP).
    pub fn imap(self) -> bool {
        !matches!(self, Self::Pop3 | Self::Proton | Self::Jmap)
    }
    /// Whether it sends through its own API rather than SMTP.
    pub fn api_sending(self) -> bool {
        matches!(self, Self::Proton | Self::Jmap)
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
    /// Proton Mail through Proton Mail Bridge on this computer (IMAP/SMTP).
    ProtonBridge,
    /// Proton Mail directly through Proton's API, without Bridge.
    Proton,
    /// Any other server, found by autoconfiguration or entered by hand.
    #[default]
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Pinal Accounts in the browser; Pinal's mail server takes its tokens.
    Pinal,
    /// Proton's own sign-in (SRP), with a two-factor code and mailbox
    /// password where the account has them.
    Proton,
    /// The provider's OAuth consent in the browser.
    Browser,
    /// A password made for mail apps, as named by `password_label`.
    Password,
}
impl SignIn {
    /// Whether the sign-in is typed into the form (a password, and for
    /// Proton maybe a two-factor code), rather than done in the browser.
    pub fn typed(self) -> bool {
        matches!(self, Self::Password | Self::Proton)
    }
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
        // Its Bridge sign-in is a tab of its own.
        Self::Proton,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinal => "Pinal Mail",
            Self::Gmail => "Gmail",
            Self::Outlook => "Outlook",
            Self::ICloud => "iCloud Mail",
            Self::Fastmail => "Fastmail",
            Self::Zoho => "Zoho Mail",
            Self::ProtonBridge => "Proton Mail Bridge",
            Self::Proton => "Proton Mail",
            Self::Other => "Other",
        }
    }
    pub fn provider(self) -> Provider {
        match self {
            Self::Gmail => Provider::Gmail,
            Self::Outlook => Provider::Outlook,
            Self::Proton => Provider::Proton,
            // Fastmail made JMAP, and offers it to mail apps.
            Self::Fastmail => Provider::Jmap,
            _ => Provider::Imap,
        }
    }
    pub fn sign_in(self) -> SignIn {
        match self {
            Self::Pinal => SignIn::Pinal,
            Self::Proton => SignIn::Proton,
            Self::Gmail | Self::Outlook if crate::sync::oauth::configured(self.provider()) => {
                SignIn::Browser
            }
            Self::Fastmail if !crate::sync::oauth::fastmail().id.is_empty() => SignIn::Browser,
            _ => SignIn::Password,
        }
    }
    /// What can be typed instead of signing in with the browser, named as
    /// the service names it: Fastmail's API token (JMAP Bearer), Google's
    /// app password (IMAP and SMTP). `None` where the browser is the only
    /// way in: Pinal, and Microsoft, which is retiring app passwords for
    /// most accounts and whose work accounts usually refuse them.
    pub fn token_instead(self) -> Option<&'static str> {
        self.token_offer(self.sign_in() == SignIn::Browser)
    }
    /// `token_instead`, given whether this build signs in to the service
    /// with the browser.
    fn token_offer(self, browser: bool) -> Option<&'static str> {
        if !browser {
            return None;
        }
        match self {
            Self::Fastmail => Some("API token"),
            Self::Gmail => Some("app password"),
            _ => None,
        }
    }
    pub fn password_label(self) -> &'static str {
        match self {
            Self::Gmail | Self::Outlook | Self::Zoho => "App password",
            Self::Fastmail => "API token",
            Self::ICloud => "App-specific password",
            Self::ProtonBridge => "Bridge password",
            Self::Proton => "Proton password",
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
                "Picacho talks to Fastmail over JMAP. Make an API token with access to mail (and to sending and filters, to use them here), then paste it here.",
                "https://app.fastmail.com/settings/security/tokens",
            )),
            Self::Zoho => Some((
                "Turn on IMAP in Zoho Mail settings, then make an app password.",
                "https://accounts.zoho.com/home#security/app_password",
            )),
            Self::ProtonBridge => Some((
                "Open Proton Mail Bridge, choose your account and copy its mailbox password. Bridge must be running, with its certificate trusted.",
                "https://proton.me/support/apple-mail-certificate",
            )),
            Self::Proton => Some((
                "Picacho signs in to Proton directly; Proton Mail Bridge isn't needed. Mail is decrypted on this computer with your keys, which never leave it.",
                "https://account.proton.me/",
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
            Self::Fastmail => (
                crate::sync::oauth::FASTMAIL_JMAP,
                443,
                crate::sync::oauth::FASTMAIL_JMAP,
                443,
            ),
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
            // No mail servers: everything goes through Proton's API.
            Self::Proton => ("mail-api.proton.me", 443, "mail-api.proton.me", 443),
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
    /// Proton Direct: seconds between asking Proton for changes (new mail,
    /// moves, notices); 0 doesn't ask, leaving new mail to the regular
    /// check (`refresh_minutes`).
    #[serde(default = "default_proton_events_seconds")]
    pub proton_events_seconds: u32,
    /// Proton Direct: receive Proton's push notifications as its Android
    /// app does (`sync::proton_push`), where this copy has a Firebase
    /// project for them. Needs `notify`; with both on, pushes announce new
    /// mail instead of the regular checks (`Account::pushes_new_mail`). Off
    /// unless turned on.
    #[serde(default)]
    pub proton_push: bool,
    /// IMAP: only the folders subscribed to on the server are shown (as
    /// other mail apps show them), unless none of them is.
    #[serde(default)]
    pub subscribed_only: bool,
    /// POP3: delete a message from the server this many days after it was
    /// downloaded here (0: at the next check after downloading). None keeps
    /// everything on the server, the default. Only messages whose whole
    /// source is saved on this computer are ever deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pop_delete_after_days: Option<u32>,
    /// POP3: deleting a message for good here also deletes it from the
    /// server. Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pop_delete_with_local: bool,
}
/// `SyncSettings::offline_days`: download message contents only when opened.
pub const OFFLINE_WHEN_OPENED: u32 = u32::MAX;
fn default_offline_days() -> u32 {
    30
}
/// About as often as Proton's own apps ask.
fn default_proton_events_seconds() -> u32 {
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
            proton_events_seconds: default_proton_events_seconds(),
            proton_push: false,
            subscribed_only: false,
            pop_delete_after_days: None,
            pop_delete_with_local: false,
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
    /// Upgrade a plain incoming connection with STARTTLS (IMAP, RFC 3501
    /// §6.2.1) or STLS (POP3, RFC 2595) instead of connecting with TLS:
    /// servers that only offer port 143 or 110, and Proton Mail Bridge on
    /// this computer. The upgrade is required; nothing is sent in the clear.
    #[serde(default)]
    pub incoming_starttls: bool,
    /// How the outgoing server is secured: TLS from the first byte (true)
    /// or a plain connection upgraded with STARTTLS (false). Unset: TLS on
    /// port 465, STARTTLS elsewhere (`smtp_implicit_tls`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_tls: Option<bool>,
    #[serde(default)]
    pub sync: SyncSettings,
    /// The name the servers know this sign-in by, when it isn't the email
    /// address (some hosts use a plain user name). Empty: the address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// Where replies go unless a message names somewhere else; empty for
    /// the sending address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to: String,
    /// Ask recipients' mail apps to confirm when a message is read
    /// (Disposition-Notification-To, RFC 8098).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_receipts: bool,
    /// Ask the receiving servers to report delivery (SMTP DSN, RFC 3461),
    /// where the sending server supports it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delivery_receipts: bool,
    /// With `delivery_receipts`: what reports are asked for and how much
    /// of the message a failure report returns (RFC 3461 NOTIFY and RET).
    #[serde(default, skip_serializing_if = "DsnOptions::is_default")]
    pub dsn: DsnOptions,
    /// Only send where every server on the way must use TLS (REQUIRETLS,
    /// RFC 8689): the outgoing server has to offer it, and mail that can't
    /// go encrypted the whole way bounces rather than going in the clear.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_tls: bool,
    /// Give up on delivering a message after this many hours rather than
    /// the servers' usual days (SMTP DELIVERBY, RFC 2852, with return); 0:
    /// as long as the servers try. Only where the outgoing server has it.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub deliver_within_hours: u32,
    /// With `deliver_within_hours`: a message not delivered in time is
    /// delivered anyway and you're told (DELIVERBY's N mode, RFC 2852 §4),
    /// rather than coming back to you (R mode).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deliver_late_notify: bool,
    /// With `deliver_within_hours`: each server on the way reports
    /// relaying it (DELIVERBY's trace, RFC 2852 §4, `T`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deliver_by_trace: bool,
    /// Other addresses this mailbox sends as, each optionally through its
    /// own outgoing server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<Identity>,
    /// JMAP: the mail account this one reads, when it isn't the sign-in's
    /// own (one shared with it); its id at the server.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jmap_account: String,
    /// Signs in with another account's credentials (by its id): a JMAP
    /// account shared with that one's sign-in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credentials_of: String,
    /// Scheduled messages are handed to the server, which sends them at
    /// their time even with Picacho closed (SMTP FUTURERELEASE, JMAP
    /// HOLDUNTIL), where it can.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub server_send_later: bool,
    /// Signs in to the incoming, outgoing and filter servers with Kerberos
    /// (SASL GSSAPI, RFC 4752) using the ticket this computer already has,
    /// instead of a password. For IMAP and POP3 accounts at organizations
    /// whose servers use Kerberos. Off by default; never chosen on its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kerberos: bool,
    /// An organization's own single sign-on (OpenID Connect / OAuth 2.0)
    /// for an IMAP or POP3 account: the identity provider (Keycloak, Okta,
    /// Entra ID, ADFS, Authentik…) that issues tokens the mail servers take
    /// with OAUTHBEARER or XOAUTH2. The provider may itself sign people in
    /// with SAML; Picacho only sees its browser pages. With `oauth` on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso: Option<Sso>,
}
/// Where an account's single sign-on happens and how Picacho is known there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sso {
    /// The identity provider's issuer (`https://login.example.com/realms/mail`):
    /// its endpoints come from the issuer's metadata.
    pub issuer: String,
    /// What to ask for; empty: `openid email offline_access`, plus what the
    /// mail domain's configuration names.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
}
/// An address a mailbox sends as besides its own (a sending identity): the
/// name and address recipients see, where replies go, and the outgoing
/// server, when it isn't the mailbox's. Its password is in the keychain
/// under `secret_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp: Option<OutgoingServer>,
}
/// An outgoing (SMTP) server and the name to sign in to it with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutgoingServer {
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// TLS from the first byte (true) or STARTTLS (false); unset, by port
    /// as `Account::smtp_tls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<bool>,
}
/// Whether an outgoing server on `port` takes TLS from its first byte:
/// `setting` when chosen, else by the port (465 is submission over TLS,
/// RFC 8314; the others start plain and upgrade with STARTTLS).
pub fn smtp_implicit_tls(setting: Option<bool>, port: u16) -> bool {
    setting.unwrap_or(port == 465)
}
impl Identity {
    /// Where its outgoing server's password is kept in the keychain.
    pub fn secret_id(&self) -> String {
        format!("identity:{}", self.id)
    }
    /// The name to sign in to its own outgoing server with.
    pub fn login(&self) -> &str {
        match &self.smtp {
            Some(s) if !s.username.trim().is_empty() => s.username.trim(),
            _ => &self.email,
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.email.trim().parse::<lettre::Address>().is_ok(),
            "Enter a valid email address."
        );
        anyhow::ensure!(
            !self.name.contains(['\r', '\n']),
            "Enter a name without line breaks."
        );
        crate::sync::smtp::recipients(&self.reply_to)
            .map_err(|_| anyhow::anyhow!("Enter a valid Reply-To address."))?;
        if let Some(s) = &self.smtp {
            anyhow::ensure!(
                !s.host.is_empty() && !s.host.contains(['\r', '\n', ' ', '/', ':']),
                "Enter a valid outgoing server hostname."
            );
            anyhow::ensure!(s.port > 0, "Ports must be between 1 and 65535.");
            anyhow::ensure!(
                !s.username.contains(['\r', '\n']),
                "Enter a sign-in name without line breaks."
            );
        }
        Ok(())
    }
}
impl Account {
    /// Whether Proton's pushes announce this account's new mail
    /// (`SyncSettings::proton_push`, which needs `notify`), in place of the
    /// announcements from checking for mail.
    pub fn pushes_new_mail(&self) -> bool {
        self.provider == Provider::Proton
            && self.sync.notify
            && self.sync.proton_push
            && crate::sync::proton_push::available()
    }
    /// One of the addresses it sends as, by id.
    pub fn identity(&self, id: &str) -> Option<&Identity> {
        self.identities.iter().find(|i| i.id == id)
    }
    /// The name to sign in to the mail servers with.
    pub fn login(&self) -> &str {
        if self.username.trim().is_empty() {
            &self.email
        } else {
            self.username.trim()
        }
    }
    pub fn new(provider: Provider) -> Self {
        let (host, smtp) = match provider {
            Provider::Gmail => ("imap.gmail.com", "smtp.gmail.com"),
            Provider::Outlook => ("outlook.office365.com", "smtp.office365.com"),
            Provider::Proton => ("mail-api.proton.me", "mail-api.proton.me"),
            _ => ("", ""),
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            email: String::new(),
            name: String::new(),
            nickname: String::new(),
            provider,
            incoming_host: host.into(),
            incoming_port: match provider {
                Provider::Pop3 => 995,
                Provider::Proton | Provider::Jmap => 443,
                _ => 993,
            },
            smtp_host: smtp.into(),
            smtp_port: if provider.api_sending() { 443 } else { 587 },
            oauth: provider == Provider::Outlook,
            client_id: String::new(),
            client_secret: String::new(),
            service: match provider {
                Provider::Gmail => Service::Gmail,
                Provider::Outlook => Service::Outlook,
                Provider::Proton => Service::Proton,
                _ => Service::Other,
            },
            incoming_starttls: false,
            smtp_tls: None,
            sync: SyncSettings::default(),
            username: String::new(),
            reply_to: String::new(),
            read_receipts: false,
            delivery_receipts: false,
            dsn: DsnOptions::default(),
            require_tls: false,
            deliver_within_hours: 0,
            deliver_late_notify: false,
            deliver_by_trace: false,
            identities: Vec::new(),
            jmap_account: String::new(),
            credentials_of: String::new(),
            server_send_later: false,
            kerberos: false,
            sso: None,
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
        // JMAP sends through the same server it reads from.
        let smtp = (self.provider != Provider::Jmap).then_some(&self.smtp_host);
        for host in std::iter::once(&self.incoming_host).chain(smtp) {
            anyhow::ensure!(
                !host.is_empty() && !host.contains(['\r', '\n', ' ', '/', ':']),
                "Enter valid incoming and SMTP server hostnames."
            );
        }
        anyhow::ensure!(
            self.incoming_port > 0 && self.smtp_port > 0,
            "Ports must be between 1 and 65535."
        );
        // A JMAP server or an organization's sign-in may register Picacho
        // as it signs in.
        anyhow::ensure!(
            !self.oauth
                || !self.client_id.is_empty()
                || self.provider == Provider::Jmap
                || self.sso.is_some(),
            "Browser sign-in isn't set up for {} in this build.",
            self.provider.label()
        );
        anyhow::ensure!(
            !self.oauth
                || matches!(
                    self.provider,
                    Provider::Gmail | Provider::Outlook | Provider::Jmap
                )
                || self.service == Service::Pinal
                || (self.sso.is_some() && matches!(self.provider, Provider::Imap | Provider::Pop3)),
            "OAuth is available for Pinal Mail, Gmail, Outlook, JMAP servers, and IMAP and POP3 servers with an organization's sign-in."
        );
        if let Some(sso) = &self.sso {
            anyhow::ensure!(self.oauth, "Single sign-on needs browser sign-in.");
            anyhow::ensure!(
                matches!(self.provider, Provider::Imap | Provider::Pop3),
                "Single sign-on is for IMAP and POP3 accounts."
            );
            anyhow::ensure!(!self.kerberos, "Choose Kerberos or single sign-on, not both.");
            let issuer = url::Url::parse(sso.issuer.trim())
                .map_err(|_| anyhow::anyhow!("Enter the sign-in service's address (its issuer URL)."))?;
            anyhow::ensure!(
                issuer.scheme() == "https" && issuer.host_str().is_some(),
                "The sign-in service's address must start with https://."
            );
            anyhow::ensure!(
                !sso.scope.chars().any(char::is_control),
                "The sign-in scopes can't contain line breaks."
            );
        }
        // STARTTLS is an upgrade of IMAP or POP3; other protocols start
        // with TLS (JMAP is HTTPS).
        anyhow::ensure!(
            !self.incoming_starttls || !matches!(self.provider, Provider::Proton | Provider::Jmap),
            "STARTTLS is only used for IMAP and POP3."
        );
        anyhow::ensure!(
            !self.username.chars().any(char::is_control),
            "The sign-in name can't contain line breaks."
        );
        anyhow::ensure!(
            !self.kerberos
                || (matches!(self.provider, Provider::Imap | Provider::Pop3) && !self.oauth),
            "Kerberos sign-in is for IMAP and POP3 accounts."
        );
        anyhow::ensure!(
            !self.kerberos || !cfg!(any(target_os = "android", target_os = "ios")),
            "Kerberos sign-in isn't available on phones and tablets."
        );
        anyhow::ensure!(
            self.reply_to.trim().is_empty()
                || crate::sync::smtp::recipients(&self.reply_to).is_ok_and(|r| !r.is_empty()),
            "Enter a valid Reply-To address."
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
    /// Every message of the account, whatever folder it is in (drafts
    /// aside). Like Starred, it is assembled from the other mailboxes.
    All,
}
impl Folder {
    pub const ALL: [Self; 8] = [
        Self::Inbox,
        Self::Starred,
        Self::Sent,
        Self::Drafts,
        Self::Archive,
        Self::Junk,
        Self::Trash,
        Self::All,
    ];
    /// Mailboxes assembled from the others rather than kept on a server.
    pub fn is_virtual(self) -> bool {
        matches!(self, Self::Starred | Self::All)
    }
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
            Self::All => "All Mail",
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
            Self::All => "all",
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Its IMAP section ("1.2"), for messages too large to download whole,
    /// whose parts are fetched one at a time; empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
    /// Its transfer encoding, with `section`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub encoding: String,
}
/// A message: what the cache keeps of it, and what lists and the reader show.
///
/// Fields few messages have (copied recipients, a thread's references,
/// labels, attachments, a server thread id, the save date, a receipt
/// address) are kept apart in `extra`, absent when all are empty: a list
/// holds tens of thousands of messages, and inline they made every one
/// half again as large. Read them through their accessors (`cc()`…) and
/// change them with the setters (`set_cc`…), which keep `extra` absent
/// while there is nothing in it. The cache's format is unchanged (see
/// `MailRecord`).
#[derive(Clone, Debug, Default)]
pub struct Mail {
    pub id: String,
    pub account: String,
    pub folder: Folder,
    pub remote_folder: String,
    pub uid: u32,
    pub from: String,
    pub sender: String,
    pub to: String,
    /// Where replies go, when the sender asked for somewhere else (alias
    /// services and mailing lists do). Empty in older cache entries.
    pub reply_to: String,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub unread: bool,
    pub starred: bool,
    pub size: u32,
    pub body: Option<String>,
    /// Original decoded HTML; absent in header-only and older cache entries.
    pub html: Option<String>,
    pub message_id: String,
    pub has_attachments: bool,
    /// Gmail's permanent id, shared by the copies under each label; 0 elsewhere.
    pub gmail_id: u64,
    /// The List-Unsubscribe header of mail from a subscription: where to
    /// ask to stop it (mailto: and https: addresses in angle brackets).
    pub list_unsubscribe: String,
    /// The sender accepts a one-click unsubscribe (RFC 8058).
    pub one_click: bool,
    /// Replied to (\\Answered), here or in another mail app.
    pub answered: bool,
    /// Forwarded ($Forwarded).
    pub forwarded: bool,
    /// A read receipt was sent or declined ($MDNSent).
    pub mdn_sent: bool,
    /// Too large to download whole: its text was read from its parts, and
    /// attachments download one at a time (`Attachment::section`).
    pub partial: bool,
    /// A row the list knows exists on the server but has not listed yet.
    /// Never stored; the list draws it as a placeholder until it loads.
    pub pending: bool,
    /// The parts its HTML shows in place (`cid:` images), found when the
    /// message is read. Never stored: the downloaded source holds them.
    pub inline: std::sync::Arc<InlineParts>,
    /// Visible so that `Mail { .., ..other }` can be written; its fields
    /// are private, set through `Mail`'s setters.
    pub extra: Option<Box<MailExtra>>,
}

/// What few messages have; see `Mail`.
#[derive(Clone, Debug, Default)]
pub struct MailExtra {
    /// Copied recipients; empty in cache entries from before it was kept.
    cc: String,
    attachments: Vec<Attachment>,
    /// The thread's earlier message ids (References, or In-Reply-To).
    references: String,
    /// Its labels: IMAP keywords set on it, or Gmail's labels (`X-GM-LABELS`),
    /// as the server names them.
    labels: Vec<String>,
    /// The server's conversation id: Gmail's X-GM-THRID ("gm:…"), OBJECTID's
    /// THREADID ("oid:…") or IMAP THREAD's ("t:…"); empty where unknown.
    thread_id: String,
    /// When it was put in its mailbox (SAVEDATE), where the server says.
    saved: i64,
    /// Where the sender asked for a read receipt (Disposition-Notification-To).
    receipt_to: String,
    /// The mailing list it came through (List-Id, RFC 2919), lowercase.
    list_id: String,
    /// The list's other addresses (RFC 2369: List-Help, -Subscribe,
    /// -Post, -Owner, -Archive), stored only, never opened by themselves.
    list_links: Vec<crate::manage::list_headers::ListLink>,
    /// The server's id for the message itself (OBJECTID's EMAILID, RFC
    /// 8474 §5.1): the same in every mailbox of the account.
    email_id: String,
}
impl MailExtra {
    fn is_empty(&self) -> bool {
        self.cc.is_empty()
            && self.attachments.is_empty()
            && self.references.is_empty()
            && self.labels.is_empty()
            && self.thread_id.is_empty()
            && self.saved == 0
            && self.receipt_to.is_empty()
            && self.list_id.is_empty()
            && self.list_links.is_empty()
            && self.email_id.is_empty()
    }
}

impl Mail {
    /// Copied recipients.
    pub fn cc(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.cc)
    }
    pub fn attachments(&self) -> &[Attachment] {
        self.extra.as_ref().map_or(&[], |e| &e.attachments)
    }
    /// The thread's earlier message ids (References, or In-Reply-To).
    pub fn references(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.references)
    }
    /// IMAP keywords set on it, or Gmail's labels, as the server names them.
    pub fn labels(&self) -> &[String] {
        self.extra.as_ref().map_or(&[], |e| &e.labels)
    }
    /// The server's conversation id; empty where unknown.
    pub fn thread_id(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.thread_id)
    }
    /// When it was put in its mailbox (SAVEDATE); 0 where unknown.
    pub fn saved(&self) -> i64 {
        self.extra.as_ref().map_or(0, |e| e.saved)
    }
    /// Where the sender asked for a read receipt.
    pub fn receipt_to(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.receipt_to)
    }
    pub fn set_cc(&mut self, cc: impl Into<String>) {
        let cc = cc.into();
        self.change_extra(cc.is_empty(), |e| e.cc = cc);
    }
    pub fn set_attachments(&mut self, attachments: Vec<Attachment>) {
        self.change_extra(attachments.is_empty(), |e| e.attachments = attachments);
    }
    pub fn set_references(&mut self, references: impl Into<String>) {
        let references = references.into();
        self.change_extra(references.is_empty(), |e| e.references = references);
    }
    pub fn set_labels(&mut self, labels: Vec<String>) {
        self.change_extra(labels.is_empty(), |e| e.labels = labels);
    }
    pub fn set_thread_id(&mut self, thread_id: impl Into<String>) {
        let thread_id = thread_id.into();
        self.change_extra(thread_id.is_empty(), |e| e.thread_id = thread_id);
    }
    pub fn set_saved(&mut self, saved: i64) {
        self.change_extra(saved == 0, |e| e.saved = saved);
    }
    pub fn set_receipt_to(&mut self, receipt_to: impl Into<String>) {
        let receipt_to = receipt_to.into();
        self.change_extra(receipt_to.is_empty(), |e| e.receipt_to = receipt_to);
    }
    /// The mailing list it came through (List-Id), lowercase; empty if none.
    pub fn list_id(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.list_id)
    }
    pub fn set_list_id(&mut self, list_id: impl Into<String>) {
        let list_id = list_id.into();
        self.change_extra(list_id.is_empty(), |e| e.list_id = list_id);
    }
    /// The list's help, subscribe, post, owner and archive addresses.
    pub fn list_links(&self) -> &[crate::manage::list_headers::ListLink] {
        self.extra.as_ref().map_or(&[], |e| &e.list_links)
    }
    pub fn set_list_links(&mut self, links: Vec<crate::manage::list_headers::ListLink>) {
        self.change_extra(links.is_empty(), |e| e.list_links = links);
    }
    /// The server's EMAILID for it (OBJECTID); empty where unknown.
    pub fn email_id(&self) -> &str {
        self.extra.as_ref().map_or("", |e| &e.email_id)
    }
    pub fn set_email_id(&mut self, email_id: impl Into<String>) {
        let email_id = email_id.into();
        self.change_extra(email_id.is_empty(), |e| e.email_id = email_id);
    }
    /// Takes the attachments out, leaving none.
    pub fn take_attachments(&mut self) -> Vec<Attachment> {
        let taken = self
            .extra
            .as_mut()
            .map(|e| std::mem::take(&mut e.attachments))
            .unwrap_or_default();
        self.compact_extra();
        taken
    }
    /// Applies `change` to the extra fields; an empty value set where there
    /// are none allocates nothing, and none left frees them.
    fn change_extra(&mut self, empty: bool, change: impl FnOnce(&mut MailExtra)) {
        if empty && self.extra.is_none() {
            return;
        }
        change(self.extra.get_or_insert_with(Default::default));
        self.compact_extra();
    }
    fn compact_extra(&mut self) {
        if self.extra.as_ref().is_some_and(|e| e.is_empty()) {
            self.extra = None;
        }
    }
}

/// `Mail` as the cache stores it, field for field as it always has been,
/// so stored messages read back unchanged and unchanged ones save the same.
/// Read owned (strings are moved into the `Mail`, not copied); written from
/// borrows (`MailRecordRef`), so saving copies nothing.
#[derive(Deserialize)]
struct MailRecord {
    id: String,
    account: String,
    folder: Folder,
    remote_folder: String,
    uid: u32,
    from: String,
    sender: String,
    to: String,
    #[serde(default)]
    cc: String,
    #[serde(default)]
    reply_to: String,
    subject: String,
    snippet: String,
    date: i64,
    unread: bool,
    starred: bool,
    size: u32,
    body: Option<String>,
    #[serde(default)]
    html: Option<String>,
    attachments: Vec<Attachment>,
    message_id: String,
    #[serde(default)]
    references: String,
    #[serde(default)]
    has_attachments: bool,
    #[serde(default)]
    gmail_id: u64,
    #[serde(default)]
    list_unsubscribe: String,
    #[serde(default)]
    one_click: bool,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    answered: bool,
    #[serde(default)]
    forwarded: bool,
    #[serde(default)]
    thread_id: String,
    #[serde(default)]
    saved: i64,
    #[serde(default)]
    receipt_to: String,
    #[serde(default)]
    list_id: String,
    #[serde(default)]
    list_links: Vec<crate::manage::list_headers::ListLink>,
    #[serde(default)]
    email_id: String,
    #[serde(default)]
    mdn_sent: bool,
    #[serde(default)]
    partial: bool,
}
#[derive(Serialize)]
struct MailRecordRef<'a> {
    id: &'a str,
    account: &'a str,
    folder: Folder,
    remote_folder: &'a str,
    uid: u32,
    from: &'a str,
    sender: &'a str,
    to: &'a str,
    cc: &'a str,
    #[serde(skip_serializing_if = "no_text")]
    reply_to: &'a str,
    subject: &'a str,
    snippet: &'a str,
    date: i64,
    unread: bool,
    starred: bool,
    size: u32,
    body: Option<&'a str>,
    html: Option<&'a str>,
    attachments: &'a [Attachment],
    message_id: &'a str,
    #[serde(skip_serializing_if = "no_text")]
    references: &'a str,
    has_attachments: bool,
    #[serde(skip_serializing_if = "is_zero")]
    gmail_id: u64,
    #[serde(skip_serializing_if = "no_text")]
    list_unsubscribe: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    one_click: bool,
    #[serde(skip_serializing_if = "no_items")]
    labels: &'a [String],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    answered: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    forwarded: bool,
    #[serde(skip_serializing_if = "no_text")]
    thread_id: &'a str,
    #[serde(skip_serializing_if = "is_zero_i64")]
    saved: i64,
    #[serde(skip_serializing_if = "no_text")]
    receipt_to: &'a str,
    #[serde(skip_serializing_if = "no_text")]
    list_id: &'a str,
    #[serde(skip_serializing_if = "no_items")]
    list_links: &'a [crate::manage::list_headers::ListLink],
    #[serde(skip_serializing_if = "no_text")]
    email_id: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    mdn_sent: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    partial: bool,
}
fn no_text(s: &&str) -> bool {
    s.is_empty()
}
fn no_items<T>(items: &&[T]) -> bool {
    items.is_empty()
}
impl From<MailRecord> for Mail {
    fn from(r: MailRecord) -> Self {
        let extra = MailExtra {
            cc: r.cc,
            attachments: r.attachments,
            references: r.references,
            labels: r.labels,
            thread_id: r.thread_id,
            saved: r.saved,
            receipt_to: r.receipt_to,
            list_id: r.list_id,
            list_links: r.list_links,
            email_id: r.email_id,
        };
        Mail {
            id: r.id,
            account: r.account,
            folder: r.folder,
            remote_folder: r.remote_folder,
            uid: r.uid,
            from: r.from,
            sender: r.sender,
            to: r.to,
            reply_to: r.reply_to,
            subject: r.subject,
            snippet: r.snippet,
            date: r.date,
            unread: r.unread,
            starred: r.starred,
            size: r.size,
            body: r.body,
            html: r.html,
            message_id: r.message_id,
            has_attachments: r.has_attachments,
            gmail_id: r.gmail_id,
            list_unsubscribe: r.list_unsubscribe,
            one_click: r.one_click,
            answered: r.answered,
            forwarded: r.forwarded,
            mdn_sent: r.mdn_sent,
            partial: r.partial,
            pending: false,
            inline: InlineParts::none(),
            extra: (!extra.is_empty()).then(|| Box::new(extra)),
        }
    }
}
impl Serialize for Mail {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        MailRecordRef {
            id: &self.id,
            account: &self.account,
            folder: self.folder,
            remote_folder: &self.remote_folder,
            uid: self.uid,
            from: &self.from,
            sender: &self.sender,
            to: &self.to,
            cc: self.cc(),
            reply_to: &self.reply_to,
            subject: &self.subject,
            snippet: &self.snippet,
            date: self.date,
            unread: self.unread,
            starred: self.starred,
            size: self.size,
            body: self.body.as_deref(),
            html: self.html.as_deref(),
            attachments: self.attachments(),
            message_id: &self.message_id,
            references: self.references(),
            has_attachments: self.has_attachments,
            gmail_id: self.gmail_id,
            list_unsubscribe: &self.list_unsubscribe,
            one_click: self.one_click,
            labels: self.labels(),
            answered: self.answered,
            forwarded: self.forwarded,
            thread_id: self.thread_id(),
            saved: self.saved(),
            receipt_to: self.receipt_to(),
            list_id: self.list_id(),
            list_links: self.list_links(),
            email_id: self.email_id(),
            mdn_sent: self.mdn_sent,
            partial: self.partial,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Mail {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MailRecord::deserialize(deserializer).map(Mail::from)
    }
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero(v: &u64) -> bool {
    *v == 0
}
fn is_zero_i64(v: &i64) -> bool {
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
    /// No parts, shared: every message read from the cache starts with
    /// these, and a list holds tens of thousands of messages.
    pub fn none() -> std::sync::Arc<Self> {
        static NONE: std::sync::LazyLock<std::sync::Arc<InlineParts>> =
            std::sync::LazyLock::new(Default::default);
        NONE.clone()
    }
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
    /// Adds a part under its Content-Location (RFC 2557): an absolute
    /// URL the HTML refers to it by, kept apart from Content-IDs.
    pub fn insert_location(&mut self, location: &str, bytes: Vec<u8>) -> bool {
        let location = location.trim();
        if !location.contains(':') || location.len() > 2048 {
            return false;
        }
        self.insert(&format!("loc:{location}"), bytes)
    }
    /// The part at a Content-Location the message gave, if any: the
    /// message's own copy, served without fetching anything.
    pub fn at_location(&self, url: &str) -> Option<std::sync::Arc<Vec<u8>>> {
        if self.parts.is_empty() {
            return None;
        }
        self.parts.get(&format!("loc:{}", url.trim()).to_lowercase()).cloned()
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
        self.has_attachments || !self.attachments().is_empty()
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

#[derive(Clone, Default, Serialize, Deserialize)]
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
    /// Proton (without Bridge): the session's UID. Its tokens are in
    /// `access_token` and `refresh_token`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proton_uid: String,
    /// Proton: the salted key passphrase (base64) that unlocks the
    /// account's keys. The password itself isn't kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proton_key: String,
}
/// Says which secrets are there, never what they are: one printed for
/// debugging (`{:?}`, an error's details) would otherwise put passwords
/// and tokens in a log.
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kept = |s: &str| if s.is_empty() { "" } else { "<redacted>" };
        f.debug_struct("Secret")
            .field("password", &kept(&self.password))
            .field("access_token", &kept(&self.access_token))
            .field("refresh_token", &kept(&self.refresh_token))
            .field("expires_at", &self.expires_at)
            .field("client_secret", &kept(&self.client_secret))
            .field("mail_bundle", &kept(&self.mail_bundle))
            .field("mail_key", &kept(&self.mail_key))
            .field("proton_uid", &self.proton_uid)
            .field("proton_key", &kept(&self.proton_key))
            .finish()
    }
}
impl Secret {
    pub(crate) fn proton(&self) -> crate::sync::proton::Saved {
        crate::sync::proton::Saved {
            uid: self.proton_uid.clone(),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            key: self.proton_key.clone(),
        }
    }
    pub(crate) fn set_proton(&mut self, saved: &crate::sync::proton::Saved) {
        self.proton_uid = saved.uid.clone();
        self.access_token = saved.access_token.clone();
        self.refresh_token = saved.refresh_token.clone();
        self.proton_key = saved.key.clone();
    }
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
    /// One of the account's other addresses (`Account::identities`) it is
    /// sent as; empty to send as the account (or `send_as`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub identity: String,
    /// When to send it (seconds since the epoch), for a message scheduled
    /// with Send Later; it waits in Drafts until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<i64>,
    /// Why it couldn't be sent when it was scheduled to be, if it couldn't.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub send_error: String,
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
    /// Where replies go, for this message; empty for the account's own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_to: String,
    /// Its copy in the server's Drafts folder, once uploaded or found there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteDraft>,
    /// A fingerprint of what was last uploaded, so unchanged drafts aren't
    /// uploaded again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uploaded: String,
    /// The message this replies to or forwards (its id here), marked
    /// answered or forwarded once this is sent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original: String,
    /// It forwards `original`, rather than replying to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forwarding: bool,
    /// Sign it with your OpenPGP key (PGP/MIME).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sign: bool,
    /// Encrypt it to every recipient's OpenPGP key (PGP/MIME).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypt: bool,
    /// When encrypting, hide the subject too (protected headers).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypt_subject: bool,
    /// Sign and encrypt it with S/MIME instead of OpenPGP.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub smime: bool,
    /// How urgent it is: shown to recipients (Importance, X-Priority) and,
    /// where the outgoing server has MT-PRIORITY (RFC 6710), given to it.
    #[serde(default, skip_serializing_if = "Priority::is_normal")]
    pub priority: Priority,
}
/// Delivery report options (RFC 3461 §4.1, §4.3). The defaults ask for
/// every report and only the headers back, as Picacho always did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DsnOptions {
    /// Report delivery (NOTIFY=SUCCESS).
    pub success: bool,
    /// Report failure (NOTIFY=FAILURE).
    pub failure: bool,
    /// Report a delay (NOTIFY=DELAY).
    pub delay: bool,
    /// A failure report returns the whole message (RET=FULL) rather than
    /// its headers (RET=HDRS).
    pub return_full: bool,
}
impl Default for DsnOptions {
    fn default() -> Self {
        Self {
            success: true,
            failure: true,
            delay: true,
            return_full: false,
        }
    }
}
impl DsnOptions {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// The NOTIFY value (RFC 3461 §4.1): the reports asked for, or NEVER.
    pub fn notify(&self) -> String {
        let asked: Vec<&str> = [
            (self.success, "SUCCESS"),
            (self.failure, "FAILURE"),
            (self.delay, "DELAY"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if asked.is_empty() {
            "NEVER".into()
        } else {
            asked.join(",")
        }
    }
    /// The RET value (RFC 3461 §4.3).
    pub fn ret(&self) -> &'static str {
        if self.return_full { "FULL" } else { "HDRS" }
    }
}
/// A message's urgency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}
impl Priority {
    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }
    /// The next one, for a button that cycles them.
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::High,
            Self::High => Self::Low,
            Self::Low => Self::Normal,
        }
    }
    /// MT-PRIORITY's level (RFC 6710 §3: -9..9; the MIXER profile's -4, 0
    /// and 4 are used).
    pub fn mt_priority(self) -> i8 {
        match self {
            Self::Low => -4,
            Self::Normal => 0,
            Self::High => 4,
        }
    }
}
/// Where a draft is in the server's Drafts folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDraft {
    pub mailbox: String,
    pub validity: u32,
    pub uid: u32,
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
    /// What the message says, to tell whether the server's copy is current.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for part in [
            &self.send_as,
            &self.to,
            &self.cc,
            &self.bcc,
            &self.subject,
            &self.body,
            &self.reply_id,
            &self.references,
            &self.reply_to,
        ] {
            h.update(part.as_bytes());
            h.update([0]);
        }
        h.update([u8::from(self.formatted)]);
        // Added later: drafts without one keep their earlier fingerprints.
        if !self.identity.is_empty() {
            h.update(b"identity\0");
            h.update(self.identity.as_bytes());
        }
        // Added later, likewise.
        if !self.priority.is_normal() {
            h.update(b"priority\0");
            h.update([self.priority.mt_priority() as u8]);
        }
        // Added later, likewise: how it is protected changes what's uploaded.
        if self.encrypt || self.sign || self.smime || self.encrypt_subject {
            h.update(b"protect\0");
            h.update([
                u8::from(self.sign),
                u8::from(self.encrypt),
                u8::from(self.smime),
                u8::from(self.encrypt_subject),
            ]);
        }
        for path in &self.attachments {
            h.update(path.to_string_lossy().as_bytes());
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            h.update(size.to_le_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
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
        .map(|(i, (name, from, subject, body, unread))| {
            let mut m = Mail {
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
            answered: i == 2,
            forwarded: false,
            mdn_sent: false,
            partial: false,
            extra: None,
            };
            // One sample message copies others, to show Cc and Reply All.
            if i == 2 {
                m.set_cc("Daniel Kim <daniel@example.com>, Emma Wilson <emma@example.com>");
            }
            m
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// Where the browser is how a service signs in, Fastmail offers an API
    /// token and Google an app password instead; Microsoft and services
    /// without one, or typed already, offer nothing.
    #[test]
    fn tokens_are_offered_instead_of_the_browser_where_taken() {
        assert_eq!(Service::Fastmail.token_offer(true), Some("API token"));
        assert_eq!(Service::Gmail.token_offer(true), Some("app password"));
        for none in [Service::Outlook, Service::Pinal, Service::ICloud, Service::Zoho, Service::Proton, Service::Other] {
            assert_eq!(none.token_offer(true), None, "{none:?}");
        }
        // Without browser sign-in in this build, the token is the only way,
        // not an alternative.
        assert_eq!(Service::Gmail.token_offer(false), None);
    }

    #[test]
    fn single_sign_on_is_for_imap_and_pop3_over_https() {
        let mut a = Account::new(Provider::Imap);
        a.email = "ana@example.edu".into();
        a.incoming_host = "imap.example.edu".into();
        a.smtp_host = "smtp.example.edu".into();
        a.oauth = true;
        // No client id: the identity provider may register Picacho.
        a.sso = Some(Sso {
            issuer: "https://login.example.edu/realms/mail".into(),
            scope: String::new(),
        });
        assert!(a.validate().is_ok());
        a.provider = Provider::Pop3;
        assert!(a.validate().is_ok());
        let mut plain = a.clone();
        plain.sso.as_mut().unwrap().issuer = "http://login.example.edu".into();
        assert!(plain.validate().is_err());
        let mut both = a.clone();
        both.kerberos = true;
        assert!(both.validate().is_err());
        let mut no_browser = a.clone();
        no_browser.oauth = false;
        assert!(no_browser.validate().is_err());
        // Without single sign-on, IMAP still can't use browser sign-in.
        let mut none = a.clone();
        none.sso = None;
        assert!(none.validate().is_err());
    }
    #[test]
    fn kerberos_is_only_for_imap_and_pop3_password_accounts() {
        let mut a = Account::new(Provider::Imap);
        a.email = "ana@example.com".into();
        a.incoming_host = "mail.example.com".into();
        a.smtp_host = "mail.example.com".into();
        a.kerberos = true;
        assert!(a.validate().is_ok());
        a.provider = Provider::Pop3;
        assert!(a.validate().is_ok());
        a.provider = Provider::Jmap;
        assert!(a.validate().is_err());
        let mut g = Account::new(Provider::Gmail);
        g.email = "ana@gmail.com".into();
        g.kerberos = true;
        assert!(g.validate().is_err());
        // Saved accounts from before read without it.
        let old: Account =
            serde_json::from_str(&serde_json::to_string(&Account::new(Provider::Imap)).unwrap())
                .unwrap();
        assert!(!old.kerberos);
        assert!(!serde_json::to_string(&old).unwrap().contains("kerberos"));
    }

    use super::*;

    /// A message with every field set, as the cache stores it.
    fn full_mail_json() -> &'static str {
        r#"{"id":"a:INBOX:1:7","account":"a","folder":"Inbox","remote_folder":"INBOX","uid":7,"from":"ana@example.com","sender":"Ana","to":"bo@example.com","cc":"cy@example.com","reply_to":"list@example.com","subject":"Plans","snippet":"Lunch?","date":1700000000,"unread":true,"starred":true,"size":1234,"body":"Lunch? Bo","html":"<p>Lunch?</p>","attachments":[{"name":"a.pdf","size":10,"mime":"application/pdf","part":2,"section":"2","encoding":"base64"}],"message_id":"<m@x>","references":"<r@x>","has_attachments":true,"gmail_id":42,"list_unsubscribe":"<https://x/u>","one_click":true,"labels":["Work"],"answered":true,"forwarded":true,"thread_id":"gm:9","saved":1700000001,"receipt_to":"ana@example.com","mdn_sent":true,"partial":true}"#
    }
    /// Old and new keep the cache's format exactly: a stored message reads
    /// back and writes out the same, and an empty one stays minimal.
    #[test]
    fn mail_keeps_the_cache_format() {
        let full: Mail = serde_json::from_str(full_mail_json()).unwrap();
        assert_eq!(serde_json::to_string(&full).unwrap(), full_mail_json());
        assert_eq!(
            serde_json::to_string(&Mail::default()).unwrap(),
            r#"{"id":"","account":"","folder":"Inbox","remote_folder":"","uid":0,"from":"","sender":"","to":"","cc":"","subject":"","snippet":"","date":0,"unread":false,"starred":false,"size":0,"body":null,"html":null,"attachments":[],"message_id":"","has_attachments":false}"#
        );
    }

    /// A mailing list's identity and links are kept (by name, the kind as
    /// a word), and mail cached before them reads and writes as it did.
    #[test]
    fn list_headers_are_kept_in_the_cache() {
        use crate::manage::list_headers::{ListLink, ListLinkKind};
        let old: Mail = serde_json::from_str(full_mail_json()).unwrap();
        assert_eq!(old.list_id(), "");
        assert!(old.list_links().is_empty());
        let mut m = old.clone();
        m.set_list_id("users.example.org");
        m.set_list_links(vec![ListLink {
            kind: ListLinkKind::Archive,
            uri: "https://example.org/archive".into(),
        }]);
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains(r#""list_id":"users.example.org","list_links":[{"kind":"Archive","uri":"https://example.org/archive"}]"#),
            "{json}"
        );
        let back: Mail = serde_json::from_str(&json).unwrap();
        assert_eq!(back.list_id(), "users.example.org");
        assert_eq!(back.list_links(), m.list_links());
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
    /// Accounts saved before Proton's event pace was a setting keep the
    /// pace they had: about every 30 seconds.
    #[test]
    fn sync_settings_saved_before_the_proton_pace_ask_every_30_seconds() {
        let old: SyncSettings =
            serde_json::from_str(r#"{"days":0,"refresh_minutes":5,"notify":true,"offline_days":30}"#)
                .unwrap();
        assert_eq!(old.proton_events_seconds, 30);
        assert_eq!(old, SyncSettings::default());
    }

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
