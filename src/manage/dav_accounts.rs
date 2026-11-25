//! Calendars and contacts are kept apart from mail: each has its own
//! server, set up in Settings → Calendar and Settings → Contacts, and
//! removing or changing a mail account doesn't change them. As an option,
//! either can use one of your mail accounts as its provider, signed in as
//! that account already is:
//!
//! - a JMAP account: its server's calendars and contacts (JMAP for
//!   Calendars and Contacts; see `connect_jmap_calendar`);
//! - a Google account: Google's CalDAV and CardDAV, with the account's
//!   OAuth token (RFC 6750 Bearer). Google asks once for calendar or
//!   contacts access on top of mail (incremental authorization);
//! - any other mail account with a password: its domain's CalDAV and
//!   CardDAV, found as for any server (SRV and `.well-known`, RFC 6764),
//!   with the account's password. iCloud, Fastmail, mailbox.org,
//!   Nextcloud and others serve both.
//!
//! Microsoft has neither CalDAV nor CardDAV, and Proton offers no calendar
//! or contacts protocol to other apps, so they aren't offered.
use super::*;
use crate::sync::{caldav, carddav};

/// Google's CalDAV and CardDAV endpoints. Discovery starts from these.
const GOOGLE_CALDAV: &str = "https://apidata.googleusercontent.com/caldav/v2/";
const GOOGLE_CARDDAV: &str = "https://www.googleapis.com/carddav/v1/principals/";

/// What a mail account can be for calendars and contacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailProvider {
    /// JMAP for Calendars and Contacts.
    Jmap,
    /// Exchange's calendars and contacts through Microsoft Graph.
    Graph,
    /// Exchange Server's, through Exchange Web Services.
    Ews,
    /// Google's CalDAV and CardDAV, with the OAuth token.
    Google,
    /// The domain's CalDAV and CardDAV, with the account's password.
    Dav,
}

impl MailProvider {
    /// What `a` can provide, if anything (see the module comment).
    pub fn of(a: &Account) -> Option<Self> {
        match a.provider {
            Provider::Jmap => Some(Self::Jmap),
            Provider::Outlook if a.graph() => Some(Self::Graph),
            Provider::Ews => Some(Self::Ews),
            Provider::Gmail if a.sso.is_none() => Some(Self::Google),
            Provider::Outlook | Provider::Proton => None,
            Provider::Imap | Provider::Pop3 if !a.oauth && !a.kerberos => Some(Self::Dav),
            _ => None,
        }
    }
}

impl Backend {
    /// The secret a DAV server shared with a mail account signs in with:
    /// the account's fresh OAuth token, or its password.
    pub(super) fn mail_account_secret(&self, id: &str) -> Result<String> {
        let a = self
            .store
            .accounts()?
            .into_iter()
            .find(|a| a.id == id)
            .with_context(|| t!("manage-dav-accounts-the-mail-account-your-calendars-or"))?;
        let s = self.credentials(&a)?;
        Ok(if a.oauth { s.access_token } else { s.password })
    }

    /// The server calendars would use with `a` as their provider: where it
    /// is and how to sign in.
    fn mail_caldav(&self, a: &Account) -> Result<caldav::Server> {
        let kind = MailProvider::of(a).with_context(|| t!("manage-dav-accounts-this-accounts-provider-doesnt-offer-calendars"))?;
        Ok(match kind {
            MailProvider::Jmap => bail!("JMAP calendars are set up with connect_jmap_calendar."),
            MailProvider::Graph => bail!("Exchange calendars are set up with connect_graph_calendar."),
            MailProvider::Ews => bail!("Exchange calendars are set up with connect_ews_calendar."),
            MailProvider::Google => caldav::Server {
                url: format!("{GOOGLE_CALDAV}{}/", a.email),
                user: a.email.clone(),
                mail_account: a.id.clone(),
                bearer: true,
                ..Default::default()
            },
            MailProvider::Dav => caldav::Server {
                url: mail_domain(a)?,
                user: a.login().to_owned(),
                mail_account: a.id.clone(),
                ..Default::default()
            },
        })
    }

    /// As `mail_caldav`, for contacts.
    fn mail_carddav(&self, a: &Account) -> Result<carddav::Server> {
        let kind = MailProvider::of(a).with_context(|| t!("manage-dav-accounts-this-accounts-provider-doesnt-offer-contacts"))?;
        Ok(match kind {
            MailProvider::Jmap => bail!("JMAP contacts are set up with connect_jmap_contacts."),
            MailProvider::Graph => bail!("Exchange contacts are set up with connect_graph_contacts."),
            MailProvider::Ews => bail!("Exchange contacts are set up with connect_ews_contacts."),
            MailProvider::Google => carddav::Server {
                url: format!("{GOOGLE_CARDDAV}{}/", a.email),
                user: a.email.clone(),
                mail_account: a.id.clone(),
                bearer: true,
                ..Default::default()
            },
            MailProvider::Dav => carddav::Server {
                url: mail_domain(a)?,
                user: a.login().to_owned(),
                mail_account: a.id.clone(),
                ..Default::default()
            },
        })
    }

    /// Google: asks, in the browser, for calendar or contacts access on top
    /// of mail, and keeps the token that covers both. Where it was already
    /// given, nothing is asked.
    fn grant_google(&self, a: &Account, scope: &str, cancel: Arc<AtomicBool>) -> Result<()> {
        if self.granted(a, scope) {
            return Ok(());
        }
        let mut granted = self.granted_scopes(a);
        granted.push(scope.to_owned());
        let asked: Vec<&str> = granted.iter().map(String::as_str).collect();
        let grant = crate::sync::oauth::authorize_with(a, &asked, cancel)?;
        let (secret, _) = crate::sync::oauth::redeem(a, &grant)?;
        self.save_credentials(&a.id, &secret)?;
        self.store
            .set_preference(&granted_key(&a.id), &serde_json::to_string(&granted)?)?;
        Ok(())
    }
    fn granted_scopes(&self, a: &Account) -> Vec<String> {
        self.store
            .preference(&granted_key(&a.id))
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }
    fn granted(&self, a: &Account, scope: &str) -> bool {
        self.granted_scopes(a).iter().any(|s| s == scope)
    }

    /// Uses mail account `a` as the calendar provider (see the module
    /// comment), signed in as it is. `cancel` stops Google's browser step.
    pub fn use_mail_account_for_calendar(&self, a: &Account, cancel: Arc<AtomicBool>) -> Result<usize> {
        if MailProvider::of(a) == Some(MailProvider::Jmap) {
            return self.connect_jmap_calendar(a);
        }
        if MailProvider::of(a) == Some(MailProvider::Graph) {
            return self.connect_graph_calendar(a);
        }
        if MailProvider::of(a) == Some(MailProvider::Ews) {
            return self.connect_ews_calendar(a);
        }
        let server = self.mail_caldav(a)?;
        if server.bearer {
            self.grant_google(a, crate::sync::oauth::GOOGLE_CALENDAR, cancel)?;
        }
        let secret = self.mail_account_secret(&a.id)?;
        self.connect_calendar(server, &secret)
    }

    /// As `use_mail_account_for_calendar`, for contacts.
    pub fn use_mail_account_for_contacts(&self, a: &Account, cancel: Arc<AtomicBool>) -> Result<usize> {
        if MailProvider::of(a) == Some(MailProvider::Jmap) {
            return self.connect_jmap_contacts(a);
        }
        if MailProvider::of(a) == Some(MailProvider::Graph) {
            return self.connect_graph_contacts(a);
        }
        if MailProvider::of(a) == Some(MailProvider::Ews) {
            return self.connect_ews_contacts(a);
        }
        let server = self.mail_carddav(a)?;
        if server.bearer {
            self.grant_google(a, crate::sync::oauth::GOOGLE_CONTACTS, cancel)?;
        }
        let secret = self.mail_account_secret(&a.id)?;
        self.connect_contacts(server, &secret)
    }

    /// The mail account calendars sign in with, if they share one's.
    pub fn calendar_mail_account(&self) -> Option<String> {
        self.calendar_server()
            .map(|s| [s.mail_account, s.jmap_account, s.graph_account, s.ews_account].into_iter().find(|id| !id.is_empty()).unwrap_or_default())
            .filter(|id| !id.is_empty())
    }
    /// The mail account contacts sign in with, if they share one's.
    pub fn contacts_mail_account(&self) -> Option<String> {
        self.contacts_server()
            .map(|s| [s.mail_account, s.jmap_account, s.graph_account, s.ews_account].into_iter().find(|id| !id.is_empty()).unwrap_or_default())
            .filter(|id| !id.is_empty())
    }
}

/// Where Google's extra grants for an account are kept.
fn granted_key(account: &str) -> String {
    format!("google_granted:{account}")
}

/// The domain of `a`'s address, as discovery starts from it (RFC 6764).
fn mail_domain(a: &Account) -> Result<String> {
    let domain = a
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.trim())
        .filter(|d| !d.is_empty())
        .with_context(|| t!("manage-dav-accounts-this-accounts-address-has-no-domain"))?;
    Ok(domain.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: Provider) -> Account {
        Account {
            email: "ana@example.org".into(),
            ..Account::new(provider)
        }
    }

    /// What each kind of mail account can provide.
    #[test]
    fn mail_accounts_offer_what_their_provider_serves() {
        assert_eq!(MailProvider::of(&account(Provider::Jmap)), Some(MailProvider::Jmap));
        let gmail = Account { oauth: true, ..account(Provider::Gmail) };
        assert_eq!(MailProvider::of(&gmail), Some(MailProvider::Google));
        assert_eq!(MailProvider::of(&account(Provider::Imap)), Some(MailProvider::Dav));
        // Microsoft and Proton offer neither to other apps.
        assert_eq!(MailProvider::of(&account(Provider::Outlook)), None);
        assert_eq!(MailProvider::of(&account(Provider::Proton)), None);
        // An organization's sign-in or a Kerberos ticket has no password
        // a DAV server would take.
        let kerberos = Account { kerberos: true, ..account(Provider::Imap) };
        assert_eq!(MailProvider::of(&kerberos), None);
    }

    /// Google: its endpoints, signed in with the account's token; others:
    /// the address's domain, discovered as any server is, with its
    /// password. Either way the server names the mail account.
    #[test]
    fn servers_from_mail_accounts_name_them_and_how_to_sign_in() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-dav-accounts-{}", uuid::Uuid::new_v4()));
        let b = Backend::new(Arc::new(Store::open(&path)?));
        let gmail = Account { oauth: true, id: "g".into(), ..account(Provider::Gmail) };
        let cal = b.mail_caldav(&gmail)?;
        assert!(cal.bearer && cal.mail_account == "g", "{cal:?}");
        assert!(cal.url.starts_with("https://apidata.googleusercontent.com/caldav/v2/ana@example.org"));
        let cards = b.mail_carddav(&gmail)?;
        assert!(cards.bearer && cards.url.starts_with("https://www.googleapis.com/carddav/v1/"));
        let imap = Account { id: "i".into(), username: "ana".into(), ..account(Provider::Imap) };
        let cal = b.mail_caldav(&imap)?;
        assert_eq!((cal.url.as_str(), cal.user.as_str(), cal.bearer), ("example.org", "ana", false));
        assert!(b.mail_caldav(&account(Provider::Outlook)).is_err());
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    /// Calendars sharing a mail account's sign-in read it from that
    /// account each time (a token renewed there is the one used); removed,
    /// they say to choose another rather than signing in as no one. Their
    /// own server setting stays apart from the account's mail.
    #[test]
    fn shared_sign_ins_follow_the_mail_account() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-dav-accounts-{}", uuid::Uuid::new_v4()));
        let mut b = Backend::new(Arc::new(Store::open(&path)?));
        let imap = Account { id: "i".into(), ..account(Provider::Imap) };
        b.store.save_account(&imap)?;
        b.test_secret = Some(Secret { password: "mail-password".into(), ..Default::default() });
        assert_eq!(b.mail_account_secret("i")?, "mail-password");
        let server = caldav::Server { mail_account: "i".into(), ..b.mail_caldav(&imap)? };
        b.store.set_preference("calendar_server", &serde_json::to_string(&server)?)?;
        assert_eq!(b.calendar_mail_account().as_deref(), Some("i"));
        // A token is what an OAuth account's calendars sign in with.
        let gmail = Account { id: "g".into(), oauth: true, ..account(Provider::Gmail) };
        b.store.save_account(&gmail)?;
        b.test_secret = Some(Secret { access_token: "token".into(), password: "x".into(), ..Default::default() });
        assert_eq!(b.mail_account_secret("g")?, "token");
        let err = b.mail_account_secret("gone").unwrap_err();
        assert!(format!("{err:#}").contains("Choose another"), "{err:#}");
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
