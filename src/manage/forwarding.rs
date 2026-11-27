//! Alias forwarding services (SimpleLogin, addy.io, Firefox Relay, Fastmail's
//! Masked Email, DuckDuckGo Email Protection, and Proton's own aliases for a
//! Proton account signed in without Bridge) and
//! the mailbox views they add: an alias shown as an account of its own, and All
//! Mail across every account.
//!
//! Forwarded mail arrives in an ordinary mailbox, still addressed to the
//! alias. An alias shown on its own lists the mail addressed to it, and the
//! mailbox it arrives in no longer does; one included with the email changes
//! nothing. The service accounts are kept in preferences and their API keys
//! in the system keychain, like mailbox passwords. Proton's aliases need no
//! key: they use the Proton account's own session (`sync/proton/aliases.rs`).
use crate::manage::{
    backend::Backend,
    model::{Account, Draft, Folder, Mail, Provider, Secret},
    store::{self, Store},
};
use crate::sync::forwarding::Client;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Preference holding the service accounts.
const PREFERENCE: &str = "forwarding";
/// Preference: list All Accounts when there are several accounts ("true"/"false").
pub const ALL_MAIL_PREFERENCE: &str = "all_mail";
/// The All Accounts view's id, used where an account id goes. (It kept
/// its old name, "all-mail", so saved places and preferences still work.)
pub const ALL_MAIL: &str = "all-mail";
const ALIAS_PREFIX: &str = "alias:";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForwardingService {
    #[default]
    SimpleLogin,
    Addy,
    FirefoxRelay,
    /// Proton's own aliases (hide-my-email aliases, kept in Proton Pass),
    /// for a Proton account signed in without Bridge. SimpleLogin forwards
    /// their mail, but they are Proton's, not a SimpleLogin account's.
    Proton,
    /// Fastmail's Masked Email, over JMAP (`sync/masked_email.rs`): with an
    /// API token, or the session of a Fastmail account added here.
    Fastmail,
    /// DuckDuckGo Email Protection's private Duck Addresses. Its API only
    /// makes new ones, so the ones made here are kept here.
    DuckDuckGo,
}
impl ForwardingService {
    pub const ALL: [Self; 6] = [
        Self::SimpleLogin,
        Self::Addy,
        Self::FirefoxRelay,
        Self::Fastmail,
        Self::DuckDuckGo,
        Self::Proton,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::SimpleLogin => "SimpleLogin",
            Self::Addy => "addy.io",
            Self::FirefoxRelay => "Firefox Relay",
            Self::Proton => "Proton",
            Self::Fastmail => "Fastmail",
            Self::DuckDuckGo => "DuckDuckGo",
        }
    }
    pub fn default_server(self) -> &'static str {
        match self {
            Self::SimpleLogin => "https://app.simplelogin.io",
            Self::Addy => "https://app.addy.io",
            Self::FirefoxRelay => "https://relay.firefox.com",
            Self::Proton => "https://pass.proton.me",
            Self::Fastmail => crate::sync::masked_email::FASTMAIL_SESSION,
            Self::DuckDuckGo => "https://quack.duckduckgo.com",
        }
    }
    /// Whether it uses Masked Email over JMAP rather than a REST API.
    pub fn uses_jmap(self) -> bool {
        matches!(self, Self::Fastmail)
    }
    /// Whether the service can list its aliases. DuckDuckGo's API only
    /// makes new ones: those made here are the ones known.
    pub fn lists_aliases(self) -> bool {
        !matches!(self, Self::DuckDuckGo)
    }
    /// Whether an alias can be turned off, deleted, and its description
    /// changed at the service. A Duck Address can only be deactivated on
    /// DuckDuckGo's own site; here it can only be forgotten.
    pub fn manages_aliases(self) -> bool {
        !matches!(self, Self::DuckDuckGo)
    }
    /// Whether a new alias's prefix and domain are chosen in the form:
    /// Firefox Relay, Fastmail and DuckDuckGo make the whole address.
    pub fn picks_address(self) -> bool {
        !matches!(self, Self::FirefoxRelay | Self::Fastmail | Self::DuckDuckGo)
    }
    /// Whether it signs in with a Proton account here rather than an API
    /// key and server.
    pub fn uses_proton_account(self) -> bool {
        matches!(self, Self::Proton)
    }
    /// Whether it takes a new alias's prefix as given, so a random one is
    /// made here (addy.io makes its own).
    pub fn takes_prefix(self) -> bool {
        matches!(self, Self::SimpleLogin | Self::Proton)
    }
    /// Whether mail can be written as an alias, not only replied to.
    /// SimpleLogin does it with reverse aliases and addy.io with
    /// `alias+name=domain` addresses; Firefox Relay only relays replies.
    /// Firefox Relay, Fastmail's masked addresses and Duck Addresses are
    /// left to replies, which each service routes itself.
    pub fn can_send(self) -> bool {
        !matches!(self, Self::FirefoxRelay | Self::Fastmail | Self::DuckDuckGo)
    }
    /// Whether an alias can have a display name, the name recipients see
    /// on mail sent from it: SimpleLogin's `name`, addy.io's `from_name`.
    pub fn has_display_name(self) -> bool {
        !matches!(self, Self::FirefoxRelay | Self::Fastmail | Self::DuckDuckGo)
    }
    /// Whether aliases can be pinned to the top of the list.
    pub fn can_pin(self) -> bool {
        !matches!(self, Self::FirefoxRelay | Self::Fastmail | Self::DuckDuckGo)
    }
    /// What the service calls the real addresses an alias forwards to
    /// (SimpleLogin's and Proton's mailboxes, addy.io's recipients), if it
    /// lets them be chosen per alias.
    pub fn mailboxes_label(self) -> Option<&'static str> {
        match self {
            Self::SimpleLogin | Self::Proton => Some(t!("manage-forwarding-mailboxes")),
            Self::Addy => Some(t!("manage-forwarding-recipients")),
            Self::FirefoxRelay | Self::Fastmail | Self::DuckDuckGo => None,
        }
    }
    /// Whether aliases list their reverse aliases (contacts), and write
    /// to people through them.
    pub fn has_contacts(self) -> bool {
        matches!(self, Self::SimpleLogin | Self::Proton)
    }
    /// Whether an alias always forwards to at least one of its mailboxes
    /// (addy.io's falls back to the default recipient).
    pub fn needs_mailbox(self) -> bool {
        matches!(self, Self::SimpleLogin | Self::Proton)
    }
    /// Where the API key is made, in one sentence, and the page.
    pub fn key_help(self) -> (&'static str, &'static str) {
        match self {
            Self::SimpleLogin => (
                t!("manage-forwarding-make-an-api-key-in-simplelogin"),
                "https://app.simplelogin.io/dashboard/api_key",
            ),
            Self::Addy => (
                t!("manage-forwarding-make-an-api-access-token-in"),
                "https://app.addy.io/settings/api",
            ),
            Self::FirefoxRelay => (
                t!("manage-forwarding-copy-the-api-key-from-the"),
                "https://relay.firefox.com/accounts/settings/",
            ),
            Self::Proton => (
                t!("manage-forwarding-proton-aliases-use-the-accounts-sign-in"),
                "https://pass.proton.me/",
            ),
            Self::Fastmail => (
                t!("masked-email-make-an-api-token-in-fastmail"),
                "https://app.fastmail.com/settings/security/tokens",
            ),
            Self::DuckDuckGo => (
                t!("masked-email-copy-the-token-duckduckgo"),
                "https://duckduckgo.com/email/settings/autofill",
            ),
        }
    }
}

/// How an alias's mail is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AliasDisplay {
    /// In the mailbox it is forwarded to, with the rest of its mail.
    #[default]
    Included,
    /// As an account of its own in the sidebar, without Sent or Drafts.
    Separate,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alias {
    /// The service's id for it.
    pub id: String,
    /// Lowercase.
    pub email: String,
    /// Whether the service forwards its mail.
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub display: AliasDisplay,
    /// The name recipients see on mail sent from it; empty for none.
    #[serde(default)]
    pub name: String,
    /// Listed first.
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub stats: AliasStats,
    /// Ids of the service's mailboxes (or recipients) it forwards to.
    #[serde(default)]
    pub mailboxes: Vec<String>,
    /// SimpleLogin: whether forwarded mail is PGP-encrypted, when one of
    /// its mailboxes has a key; `None` when that isn't possible.
    #[serde(default)]
    pub pgp: Option<bool>,
    /// Proton: an address of the account's own (not a Pass alias), and its
    /// kind. Proton sends from it itself; it has no reverse aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proton_address: Option<ProtonAddressKind>,
}
impl Alias {
    /// Whether it is one of a Proton account's own addresses.
    pub fn is_address(&self) -> bool {
        self.proton_address.is_some()
    }
}

/// The kinds of Proton address listed as aliases (all but the account's
/// original address).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtonAddressKind {
    /// Another address at Proton's domains.
    Alias,
    /// At a domain of the account's own.
    Custom,
    /// A short premium address, such as `@pm.me`.
    Premium,
    /// An address elsewhere that signs in to Proton.
    External,
}
impl ProtonAddressKind {
    /// Proton's `Address.Type`; `None` for the original address (left out)
    /// and kinds not known here.
    pub fn of(kind: i64) -> Option<Self> {
        match kind {
            2 => Some(Self::Alias),
            3 => Some(Self::Custom),
            4 => Some(Self::Premium),
            5 => Some(Self::External),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Alias => t!("manage-forwarding-proton-address"),
            Self::Custom => t!("manage-forwarding-your-domain"),
            Self::Premium => t!("manage-forwarding-premium-address"),
            Self::External => t!("manage-forwarding-external-address"),
        }
    }
}

/// How much mail an alias has handled, as its service counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasStats {
    pub forwarded: u64,
    pub blocked: u64,
    pub replied: u64,
    /// addy.io also counts mail sent from the alias.
    pub sent: Option<u64>,
}
impl AliasStats {
    /// "12 forwarded · 1 blocked · 3 replied", leaving out zeros; empty
    /// when nothing was counted.
    pub fn summary(&self) -> String {
        [
            (self.forwarded, "forwarded"),
            (self.blocked, "blocked"),
            (self.replied, "replied"),
            (self.sent.unwrap_or(0), "sent"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

/// One of a service's real addresses that aliases forward to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceMailbox {
    pub id: String,
    pub email: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardingAccount {
    pub id: String,
    pub service: ForwardingService,
    /// Blank for the service's own server.
    #[serde(default)]
    pub server: String,
    /// The service's name for the account, if it gave one.
    #[serde(default)]
    pub name: String,
    /// The Picacho mailbox its aliases forward to; `None` looks in all.
    #[serde(default)]
    pub deliver_to: Option<String>,
    #[serde(default)]
    pub aliases: Vec<Alias>,
    /// The service's mailboxes or recipients, where it has them.
    #[serde(default)]
    pub mailboxes: Vec<ServiceMailbox>,
    /// The mailboxes it forwards to send only as its aliases: they can't
    /// write as themselves, and their address is shown as the alias used.
    #[serde(default)]
    pub aliases_only: bool,
    /// Proton: the Picacho account (signed in to Proton without Bridge)
    /// whose aliases these are.
    #[serde(default)]
    pub proton_account: String,
    /// Fastmail: the Picacho account (JMAP, at Fastmail) whose session
    /// reaches its Masked Email, when no API token was given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub jmap_account: String,
}
impl ForwardingAccount {
    fn secret_id(&self) -> String {
        secret_id(&self.id)
    }
    fn client(&self) -> Result<Client> {
        let key = store::read_secret(&self.secret_id())?.password;
        Client::new(self.service, &self.server, &key)
    }
    /// What talks to the service: its API with the key in the keychain, or
    /// for Proton's aliases, the Proton account's session.
    fn api<'a>(&self, backend: &'a Backend) -> Result<Api<'a>> {
        if self.service.uses_proton_account() {
            // The Pass session and Pass's extra password, kept as other
            // services' keys are.
            let secret = store::read_secret(&self.secret_id()).unwrap_or_default();
            return Ok(Api::Proton(ProtonApi {
                backend,
                account: self.proton_account.clone(),
                secret_id: self.secret_id(),
                extra: Some(zeroize::Zeroizing::new(secret.password.clone())).filter(|p| !p.is_empty()),
                pass: Some(secret.proton()).filter(|p| !p.uid.is_empty()),
            }));
        }
        if self.service.uses_jmap() {
            if !self.jmap_account.is_empty() {
                return Ok(Api::Masked(MaskedApi::Account(backend, self.jmap_account.clone())));
            }
            let key = store::read_secret(&self.secret_id())?.password;
            return crate::sync::masked_email::Token::open(&self.server, &key)
                .map(|t| Api::Masked(MaskedApi::Token(Box::new(t))));
        }
        self.client().map(Api::Http)
    }
    /// Aliases from the service, keeping how each one is shown here.
    fn merge(&self, fresh: Vec<Alias>) -> Vec<Alias> {
        let shown: HashMap<&str, AliasDisplay> = self
            .aliases
            .iter()
            .map(|a| (a.id.as_str(), a.display))
            .collect();
        fresh
            .into_iter()
            .map(|a| Alias {
                display: shown.get(a.id.as_str()).copied().unwrap_or_default(),
                ..a
            })
            .collect()
    }
}
fn secret_id(id: &str) -> String {
    format!("forwarding:{id}")
}

/// A service to ask: one with its own API (`sync/forwarding.rs`), or
/// Proton's aliases through a Proton account's session.
enum Api<'a> {
    Http(Client),
    Proton(ProtonApi<'a>),
    Masked(MaskedApi<'a>),
}
/// Fastmail's Masked Email: through an API token, or a Fastmail account's
/// own JMAP session (the backend keeps it, by the account's id).
enum MaskedApi<'a> {
    Token(Box<crate::sync::masked_email::Token>),
    Account(&'a Backend, String),
}
impl MaskedApi<'_> {
    fn run<T>(&self, work: impl FnOnce(&dyn crate::sync::masked_email::Caller) -> Result<T>) -> Result<T> {
        match self {
            MaskedApi::Token(t) => work(t.as_ref()),
            MaskedApi::Account(backend, id) => {
                let account = backend
                    .store
                    .accounts()?
                    .into_iter()
                    .find(|a| &a.id == id)
                    .with_context(|| t!("masked-email-the-fastmail-account-is-gone"))?;
                backend.with_jmap(&account, |s| work(s))
            }
        }
    }
}
/// Proton's aliases of one account.
struct ProtonApi<'a> {
    /// The backend that keeps the account's session.
    backend: &'a Backend,
    /// The account's id.
    account: String,
    /// Where the Pass session and extra password are kept.
    secret_id: String,
    /// Pass's extra password, if the account protects Pass with one.
    extra: Option<zeroize::Zeroizing<String>>,
    /// The session signed in for Pass, in a build with a Pass registration.
    pass: Option<crate::sync::proton::Saved>,
}
impl ProtonApi<'_> {
    /// `work` with the account's session, set up for Pass: its Pass
    /// session in use and Pass unlocked. Tokens Proton renewed for the Pass
    /// session meanwhile are saved, since each refresh replaces the last.
    fn run<T>(&self, work: impl FnOnce(&crate::sync::proton::Session) -> Result<T>) -> Result<T> {
        self.backend.with_proton_account(&self.account, |s| {
            if let Some(pass) = &self.pass {
                s.use_pass_session(pass)?;
            }
            let result = (|| {
                if let Some(extra) = &self.extra {
                    s.unlock_proton_pass(extra)?;
                }
                work(s)
            })();
            if let Some(pass) = &self.pass
                && let Some(renewed) = s.pass_renewed(pass)
            {
                let mut secret = store::read_secret(&self.secret_id).unwrap_or_default();
                secret.set_proton(&renewed);
                if store::write_secret(&self.secret_id, &secret).is_err() {
                    eprintln!("Picacho: the renewed Proton Pass session couldn't be saved");
                }
            }
            result
        })
    }
}
/// `$http` with the HTTP client as `$c`, or `$proton` with the Proton
/// session as `$s`.
macro_rules! ask {
    ($api:expr, $c:ident => $http:expr, $s:ident => $proton:expr) => {
        ask!($api, $c => $http, $s => $proton, _m => anyhow::bail!(t!("masked-email-fastmail-cant-do-this")))
    };
    ($api:expr, $c:ident => $http:expr, $s:ident => $proton:expr, $m:ident => $masked:expr) => {
        match $api {
            Api::Http($c) => $http,
            Api::Proton(p) => p.run(|$s| $proton),
            Api::Masked(m) => m.run(|$m| $masked),
        }
    };
}
use crate::sync::masked_email as masked;
impl Api<'_> {
    /// Every alias; for Proton, its Pass aliases and then the account's
    /// own addresses but its original one and the one it signs in with.
    fn aliases(&self) -> Result<Vec<Alias>> {
        match self {
            Api::Http(c) => c.aliases(),
            Api::Masked(m) => m.run(masked::aliases),
            Api::Proton(p) => {
                let own = p.backend.store.accounts()?.into_iter().find(|a| a.id == p.account).map(|a| a.email);
                p.run(|s| proton_aliases_and_addresses(s, own.as_deref()))
            }
        }
    }
    fn mailboxes(&self) -> Result<Vec<ServiceMailbox>> {
        ask!(self, c => c.mailboxes(), s => s.proton_alias_mailboxes(), _m => Ok(Vec::new()))
    }
    fn options(&self) -> Result<AliasOptions> {
        ask!(self, c => c.options(), s => s.proton_alias_options(), _m => Ok(masked::options()))
    }
    fn create(&self, new: &NewAlias) -> Result<Alias> {
        ask!(self, c => c.create(new), s => s.create_proton_alias(new), m => masked::create(m, new))
    }
    fn alias_exists(&self, alias: &Alias) -> Result<bool> {
        ask!(self, c => c.alias_exists(alias), s => s.proton_alias_exists(alias), m => masked::exists(m, alias))
    }
    fn set_note(&self, alias: &Alias, note: &str) -> Result<()> {
        ask!(self, c => c.set_note(alias, note), s => s.set_proton_alias_note(alias, note), m => masked::set_note(m, alias, note))
    }
    fn set_enabled(&self, alias: &Alias, enabled: bool) -> Result<()> {
        ask!(
            self,
            c => c.set_enabled(alias, enabled),
            s => s.set_proton_alias_enabled(alias, enabled),
            m => masked::set_enabled(m, alias, enabled)
        )
    }
    fn set_name(&self, alias: &Alias, name: &str) -> Result<()> {
        ask!(self, c => c.set_name(alias, name), s => s.set_proton_alias_name(alias, name))
    }
    fn set_pinned(&self, alias: &Alias, pinned: bool) -> Result<()> {
        ask!(self, c => c.set_pinned(alias, pinned), s => s.set_proton_alias_pinned(alias, pinned))
    }
    fn set_mailboxes(&self, alias: &Alias, ids: &[String]) -> Result<()> {
        ask!(self, c => c.set_mailboxes(alias, ids), s => s.set_proton_alias_mailboxes(alias, ids))
    }
    fn set_pgp(&self, alias: &Alias, enabled: bool) -> Result<()> {
        match self {
            Api::Http(c) => c.set_pgp(alias, enabled),
            Api::Proton(..) | Api::Masked(..) => anyhow::bail!(t!("sync-forwarding-only-simplelogin-encrypts-forwarded-mail-per")),
        }
    }
    /// An alias's contacts. Proton's come with their counts.
    fn contacts(&self, alias: &Alias) -> Result<Vec<ReverseAlias>> {
        ask!(self, c => c.contacts(alias), s => s.proton_alias_contacts(alias))
    }
    fn toggle_contact(&self, contact: &ReverseAlias) -> Result<bool> {
        ask!(self, c => c.toggle_contact(contact), s => s.toggle_proton_alias_contact(contact))
    }
    fn delete_contact(&self, contact: &ReverseAlias) -> Result<()> {
        ask!(self, c => c.delete_contact(contact), s => s.delete_proton_alias_contact(contact))
    }
    fn add_contact(&self, alias: &Alias, address: &str) -> Result<ReverseAlias> {
        ask!(self, c => c.add_contact(alias, address), s => s.add_proton_alias_contact(alias, address))
    }
    /// The reverse alias for writing to `contact` as `alias`, made if it
    /// does not exist yet.
    fn reverse_alias(&self, alias: &Alias, contact: &str) -> Result<String> {
        ask!(
            self,
            c => c.reverse_alias(alias, contact),
            s => s.add_proton_alias_contact(alias, contact).map(|r| r.address)
        )
    }
    fn delete(&self, alias: &Alias) -> Result<()> {
        ask!(self, c => c.delete(alias), s => s.delete_proton_alias(alias), m => masked::delete(m, alias))
    }
}

pub fn load(store: &Store) -> Vec<ForwardingAccount> {
    store
        .preference(PREFERENCE)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
pub fn save(store: &Store, accounts: &[ForwardingAccount]) -> Result<()> {
    store.set_preference(PREFERENCE, &serde_json::to_string(accounts)?)
}
pub fn all_mail_enabled(store: &Store) -> bool {
    store.preference(ALL_MAIL_PREFERENCE).as_deref() != Some("false")
}

/// Checks the key, lists the aliases and keeps the key in the keychain.
/// `accounts` picks where the aliases forward to, when the service says.
pub fn connect(
    service: ForwardingService,
    server: &str,
    key: &str,
    accounts: &[Account],
) -> Result<ForwardingAccount> {
    if service.uses_jmap() {
        return connect_masked(server, key, accounts);
    }
    let client = Client::new(service, server, key)?;
    let profile = client.profile()?;
    let aliases = client.aliases()?;
    // Only needed to choose them per alias; the account works without.
    let mailboxes = client.mailboxes().unwrap_or_default();
    let account = ForwardingAccount {
        id: uuid::Uuid::new_v4().to_string(),
        service,
        server: if crate::sync::forwarding::server_url(service, "")?
            == crate::sync::forwarding::server_url(service, server)?
        {
            String::new()
        } else {
            crate::sync::forwarding::server_url(service, server)?
        },
        name: profile.name.clone(),
        deliver_to: accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(&profile.mailbox))
            .map(|a| a.id.clone()),
        aliases,
        mailboxes,
        aliases_only: false,
        proton_account: String::new(),
        jmap_account: String::new(),
    };
    store::write_secret(
        &account.secret_id(),
        &Secret {
            password: key.trim().to_owned(),
            ..Secret::default()
        },
    )?;
    Ok(account)
}
/// Fastmail's Masked Email with an API token: checks it can reach Masked
/// Email, lists the addresses and keeps the token in the keychain.
fn connect_masked(server: &str, key: &str, accounts: &[Account]) -> Result<ForwardingAccount> {
    let token = crate::sync::masked_email::Token::open(server, key)?;
    let aliases = masked::aliases(&token)?;
    let session = crate::sync::masked_email::session_url_of(server)?;
    let account = ForwardingAccount {
        id: uuid::Uuid::new_v4().to_string(),
        service: ForwardingService::Fastmail,
        server: if session == crate::sync::masked_email::FASTMAIL_SESSION { String::new() } else { session },
        name: token.username.clone(),
        deliver_to: accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(&token.username))
            .map(|a| a.id.clone()),
        aliases,
        ..ForwardingAccount::default()
    };
    store::write_secret(
        &account.secret_id(),
        &Secret {
            password: key.trim().to_owned(),
            ..Secret::default()
        },
    )?;
    Ok(account)
}

/// Fastmail's Masked Email through `account`'s own JMAP session (a
/// Fastmail account added here): no token is kept, the account's sign-in
/// is used.
pub fn connect_jmap_account(backend: &Backend, account: &Account) -> Result<ForwardingAccount> {
    anyhow::ensure!(
        account.provider == Provider::Jmap,
        t!("masked-email-this-account-has-no-masked-email")
    );
    let aliases = backend.with_jmap(account, |s| masked::aliases(s))?;
    Ok(ForwardingAccount {
        id: uuid::Uuid::new_v4().to_string(),
        service: ForwardingService::Fastmail,
        name: account.email.clone(),
        deliver_to: Some(account.id.clone()),
        aliases,
        jmap_account: account.id.clone(),
        ..ForwardingAccount::default()
    })
}
/// The JMAP accounts whose Masked Email isn't listed here yet: those that
/// might be at Fastmail. Whether one has Masked Email shows on connecting.
pub fn jmap_accounts<'a>(forwarding: &[ForwardingAccount], accounts: &'a [Account]) -> Vec<&'a Account> {
    accounts
        .iter()
        .filter(|a| a.provider == Provider::Jmap && a.credentials_of.is_empty())
        .filter(|a| !forwarding.iter().any(|f| f.service.uses_jmap() && f.jmap_account == a.id))
        .collect()
}

/// A Proton account's Pass aliases, then its own addresses but `own` (the
/// one it is signed in with here), each address after the aliases.
fn proton_aliases_and_addresses(s: &crate::sync::proton::Session, own: Option<&str>) -> Result<Vec<Alias>> {
    let mut all = s.proton_aliases()?;
    let addresses: Vec<Alias> = s
        .proton_addresses()?
        .into_iter()
        .filter(|a| own.is_none_or(|own| !a.email.eq_ignore_ascii_case(own)))
        .filter(|a| !all.iter().any(|b| b.email == a.email))
        .collect();
    all.extend(addresses);
    Ok(all)
}

/// Proton's aliases need Proton Pass signed in to again: Proton ended its
/// session, or there is none (Forwarding settings offer to sign in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassSignInNeeded(pub String);
impl std::fmt::Display for PassSignInNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PassSignInNeeded {}

/// Signs Proton's aliases in to Proton Pass again with `pass`, a session
/// just signed in (`Backend::proton_pass_begin`): checked by listing the
/// aliases, then kept in place of the old one, which is signed out. Pass's
/// extra password is replaced when one is given. Returns what is listed.
pub fn sign_in_to_pass_again(
    backend: &Backend,
    account: &ForwardingAccount,
    pass: crate::sync::proton::Saved,
    extra_password: &str,
) -> Result<Refreshed> {
    anyhow::ensure!(account.service.uses_proton_account(), t!("sync-proton-aliases-sign-in-to-proton-pass"));
    let mut secret = store::read_secret(&account.secret_id()).unwrap_or_default();
    let old = Some(secret.proton()).filter(|p| !p.uid.is_empty() && p.uid != pass.uid);
    if !extra_password.is_empty() {
        secret.password = extra_password.to_owned();
    }
    let extra = secret.password.clone();
    let checked = backend.with_proton_account(&account.proton_account, |s| {
        s.reset_pass_session();
        s.use_pass_session(&pass)?;
        s.unlock_proton_pass(&extra)?;
        s.proton_aliases()?;
        Ok(s.pass_renewed(&pass).unwrap_or(pass.clone()))
    });
    let kept = match checked {
        Ok(kept) => kept,
        Err(e) => {
            crate::sync::proton::sign_out(&pass);
            return Err(e);
        }
    };
    secret.set_proton(&kept);
    store::write_secret(&account.secret_id(), &secret)?;
    if let Some(old) = old {
        crate::sync::proton::sign_out(&old);
    }
    refresh(backend, account)
}

/// How Proton's aliases are signed in to: in a build with a Pass
/// registration, the session signed in to Proton Pass for the account
/// (`Backend::proton_pass_begin`; Proton gives Pass access only to a session
/// signed in as an app with Pass); and Pass's extra password, if the
/// account protects Pass with one.
#[derive(Clone, Default)]
pub struct ProtonSignIn {
    pub pass: Option<crate::sync::proton::Saved>,
    pub extra_password: zeroize::Zeroizing<String>,
}

/// Proton's own aliases of a Proton account signed in without Bridge:
/// lists them with the account's session (or, in a build with a Pass
/// registration, a Pass session signed in with `with`), keeping only that
/// Pass session and Pass's extra password, if any, in the keychain.
pub fn connect_proton(
    backend: &Backend,
    account: &Account,
    accounts: &[Account],
    with: &ProtonSignIn,
) -> Result<ForwardingAccount> {
    anyhow::ensure!(
        account.provider == Provider::Proton,
        t!("manage-forwarding-proton-aliases-need-a-proton-account", email = account.email)
    );
    let pass = with.pass.clone();
    if pass.is_none() && crate::sync::proton::pass_sign_in() {
        return Err(PassSignInNeeded(t!("sync-proton-aliases-sign-in-to-proton-pass").into()).into());
    }
    let listed = backend.with_proton_account(&account.id, |s| {
        if let Some(pass) = &pass {
            s.use_pass_session(pass)?;
        }
        s.unlock_proton_pass(&with.extra_password)?;
        let (name, mailbox) = s.proton_alias_profile();
        let aliases = proton_aliases_and_addresses(s, Some(&account.email))?;
        let renewed = pass.as_ref().and_then(|p| s.pass_renewed(p));
        Ok((name, mailbox, aliases, s.proton_alias_mailboxes().unwrap_or_default(), renewed))
    });
    let (name, mailbox, aliases, mailboxes, renewed) = match listed {
        Ok(listed) => listed,
        Err(e) => {
            // Not kept, so not left signed in at Proton either.
            if let Some(pass) = &pass {
                crate::sync::proton::sign_out(pass);
            }
            return Err(e);
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    if pass.is_some() || !with.extra_password.is_empty() {
        let mut secret = Secret {
            password: with.extra_password.to_string(),
            ..Secret::default()
        };
        if let Some(pass) = renewed.as_ref().or(pass.as_ref()) {
            secret.set_proton(pass);
        }
        store::write_secret(&secret_id(&id), &secret)?;
    }
    Ok(ForwardingAccount {
        id,
        service: ForwardingService::Proton,
        name: if name.is_empty() { account.email.clone() } else { name },
        deliver_to: accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(&mailbox))
            .map(|a| a.id.clone())
            .or_else(|| Some(account.id.clone())),
        aliases,
        mailboxes,
        proton_account: account.id.clone(),
        ..ForwardingAccount::default()
    })
}
/// The Proton accounts signed in without Bridge whose aliases aren't
/// listed here yet.
pub fn proton_accounts<'a>(forwarding: &[ForwardingAccount], accounts: &'a [Account]) -> Vec<&'a Account> {
    accounts
        .iter()
        .filter(|a| a.provider == Provider::Proton)
        .filter(|a| !forwarding.iter().any(|f| f.service.uses_proton_account() && f.proton_account == a.id))
        .collect()
}
/// What a service lists now: its aliases, and its mailboxes (None when
/// they couldn't be listed, so the ones known are kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refreshed {
    pub aliases: Vec<Alias>,
    pub mailboxes: Option<Vec<ServiceMailbox>>,
}
/// The account's aliases and mailboxes as the service lists them now.
pub fn refresh(backend: &Backend, account: &ForwardingAccount) -> Result<Refreshed> {
    if !account.service.lists_aliases() {
        // Nothing to ask: the ones made here are all that is known.
        return Ok(Refreshed { aliases: account.aliases.clone(), mailboxes: None });
    }
    let api = account.api(backend)?;
    Ok(Refreshed {
        aliases: account.merge(api.aliases()?),
        mailboxes: api.mailboxes().ok(),
    })
}
/// Changes the name recipients see on mail from an alias; empty for none.
pub fn set_name(backend: &Backend, account: &ForwardingAccount, alias: &Alias, name: &str) -> Result<()> {
    account.api(backend)?.set_name(alias, name)
}
pub fn set_pinned(backend: &Backend, account: &ForwardingAccount, alias: &Alias, pinned: bool) -> Result<()> {
    account.api(backend)?.set_pinned(alias, pinned)
}
/// Chooses which of the service's mailboxes an alias forwards to.
pub fn set_mailboxes(backend: &Backend, account: &ForwardingAccount, alias: &Alias, ids: &[String]) -> Result<()> {
    account.api(backend)?.set_mailboxes(alias, ids)
}
/// SimpleLogin: turns PGP encryption of an alias's forwarded mail on or off.
pub fn set_pgp(backend: &Backend, account: &ForwardingAccount, alias: &Alias, enabled: bool) -> Result<()> {
    account.api(backend)?.set_pgp(alias, enabled)
}
/// SimpleLogin: blocks or unblocks mail from a contact; returns whether it
/// is blocked now.
pub fn toggle_contact(backend: &Backend, account: &ForwardingAccount, contact: &ReverseAlias) -> Result<bool> {
    account.api(backend)?.toggle_contact(contact)
}
pub fn delete_contact(backend: &Backend, account: &ForwardingAccount, contact: &ReverseAlias) -> Result<()> {
    account.api(backend)?.delete_contact(contact)
}
/// SimpleLogin: the reverse alias for writing to `address` as the alias,
/// made unless it exists.
pub fn add_contact(backend: &Backend, account: &ForwardingAccount, alias: &Alias, address: &str) -> Result<ReverseAlias> {
    account.api(backend)?.add_contact(alias, address)
}
pub fn create(backend: &Backend, account: &ForwardingAccount, new: &NewAlias) -> Result<Alias> {
    account.api(backend)?.create(new)
}
/// The domains and prefixes the account can make aliases with.
pub fn options(backend: &Backend, account: &ForwardingAccount) -> Result<AliasOptions> {
    account.api(backend)?.options()
}
/// Changes an alias's description at the service.
/// For DuckDuckGo, whose API keeps no descriptions, the note stays here.
pub fn set_note(backend: &Backend, account: &ForwardingAccount, alias: &Alias, note: &str) -> Result<()> {
    if !account.service.manages_aliases() {
        return Ok(());
    }
    account.api(backend)?.set_note(alias, note)
}

/// One of a SimpleLogin alias's contacts: someone it has mail with, and
/// the reverse alias that writes to them as the alias.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReverseAlias {
    /// The service's id for the contact.
    pub id: String,
    /// The contact's own address.
    pub contact: String,
    /// The reverse alias, lowercase. Never shown; only `obscure`d.
    pub address: String,
    /// Whether mail from the contact to the alias is blocked.
    pub blocked: bool,
    /// When the alias last wrote to the contact, in Unix seconds.
    pub last_sent: Option<i64>,
    /// How much mail went between the alias and the contact; `None` when
    /// the alias's activity couldn't be read.
    pub stats: Option<ContactStats>,
}

/// Days counted as recent in a contact's stats.
pub const CONTACT_RECENT_DAYS: i64 = 14;

/// How much mail an alias has had with one contact, counted from the
/// alias's activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContactStats {
    /// Mail from the contact forwarded to you, in the last
    /// `CONTACT_RECENT_DAYS` days and all time.
    pub received_recent: u64,
    pub received: u64,
    /// Mail you sent the contact through the reverse alias.
    pub sent_recent: u64,
    pub sent: u64,
    /// Whether the activity was too long to read in full, so the all-time
    /// counts are at least these.
    pub partial: bool,
}

/// What happened in one entry of an alias's activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    /// Mail from a contact forwarded to your mailbox.
    Forward,
    /// Mail you sent a contact through its reverse alias.
    Reply,
    /// Anything else: blocked or bounced mail.
    Other,
}

/// One entry of a SimpleLogin alias's activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub kind: ActivityKind,
    /// The reverse alias involved, lowercase; may be empty.
    pub reverse_alias: String,
    /// The contact's address, lowercase: the sender of forwarded mail, the
    /// recipient of a reply.
    pub contact: String,
    /// Unix seconds.
    pub timestamp: i64,
}

/// Counts each contact's mail from an alias's activity. `partial` says the
/// activity wasn't read to its start.
pub fn tally(contacts: &mut [ReverseAlias], activity: &[Activity], partial: bool, now: i64) {
    let since = now - CONTACT_RECENT_DAYS * 24 * 60 * 60;
    let mut by_address: HashMap<String, usize> = HashMap::new();
    let mut by_contact: HashMap<String, usize> = HashMap::new();
    for (i, c) in contacts.iter_mut().enumerate() {
        c.stats = Some(ContactStats {
            partial,
            ..ContactStats::default()
        });
        by_address.insert(c.address.clone(), i);
        by_contact.entry(c.contact.to_lowercase()).or_insert(i);
    }
    for a in activity {
        let Some(&i) = (!a.reverse_alias.is_empty())
            .then(|| by_address.get(&a.reverse_alias))
            .flatten()
            .or_else(|| by_contact.get(&a.contact))
        else {
            continue;
        };
        let Some(stats) = contacts[i].stats.as_mut() else {
            continue;
        };
        let recent = a.timestamp >= since;
        match a.kind {
            ActivityKind::Forward => {
                stats.received += 1;
                stats.received_recent += recent as u64;
            }
            ActivityKind::Reply => {
                stats.sent += 1;
                stats.sent_recent += recent as u64;
            }
            ActivityKind::Other => {}
        }
    }
}

/// A SimpleLogin alias's reverse aliases, as the service lists them now,
/// with how much mail each has had. Stats are left out, rather than the
/// list failing, when the activity can't be read.
pub fn reverse_aliases(backend: &Backend, account: &ForwardingAccount, alias: &Alias) -> Result<Vec<ReverseAlias>> {
    let client = match account.api(backend)? {
        // Proton's contacts come with their counts.
        api @ Api::Proton(..) => return api.contacts(alias),
        Api::Masked(..) => anyhow::bail!(t!("sync-forwarding-only-simplelogin-has-reverse-aliases")),
        Api::Http(client) => client,
    };
    let (contacts, activity) = std::thread::scope(|scope| {
        let activity = scope.spawn(|| client.activity(alias));
        let contacts = client.contacts(alias);
        let activity = activity
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("manage-forwarding-reading-the-aliass-activity-failed"))));
        (contacts, activity)
    });
    let mut contacts = contacts?;
    if let Ok((activity, partial)) = activity {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        tally(&mut contacts, &activity, partial, now);
    }
    Ok(contacts)
}
/// What a reverse alias shows in place of its address: nothing of it, not
/// even its length, so it can't be read off the screen. It can be copied.
pub fn obscure(_address: &str) -> String {
    "••••••••••••".to_owned()
}

/// What kind of domain an alias can be made at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainKind {
    /// A domain the person owns and added to the service.
    Custom,
    /// addy.io's subdomain of the person's username.
    Username,
    /// A shared domain only paying subscribers can use.
    Premium,
    /// A shared domain anyone can use.
    Shared,
}
impl DomainKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => t!("manage-forwarding-your-domain"),
            Self::Username => t!("manage-forwarding-your-subdomain"),
            Self::Premium => t!("manage-forwarding-premium"),
            Self::Shared => t!("manage-forwarding-free"),
        }
    }
}
/// A domain (or, at SimpleLogin, a suffix) a new alias can end in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasDomain {
    /// What follows the prefix: `@example.com`, or SimpleLogin's
    /// `.word@simplelogin.com`.
    pub suffix: String,
    pub kind: DomainKind,
    /// Whether the prefix can be chosen here; otherwise it is random.
    pub custom_prefix: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AliasOptions {
    /// The domains, the default first.
    pub domains: Vec<AliasDomain>,
    /// A prefix the service suggests, if any.
    pub prefix_suggestion: String,
    /// False when the account has reached its alias limit.
    pub can_create: bool,
}
/// How a random prefix is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasFormat {
    RandomWords,
    RandomCharacters,
    Uuid,
}
impl AliasFormat {
    pub const ALL: [Self; 3] = [Self::RandomWords, Self::RandomCharacters, Self::Uuid];
    pub fn label(self) -> &'static str {
        match self {
            Self::RandomWords => t!("manage-forwarding-random-words"),
            Self::RandomCharacters => t!("manage-forwarding-random-characters"),
            Self::Uuid => "UUID",
        }
    }
    /// addy.io's name for it.
    pub fn addy_key(self) -> &'static str {
        match self {
            Self::RandomWords => "random_words",
            Self::RandomCharacters => "random_characters",
            Self::Uuid => "uuid",
        }
    }
}
/// A new alias to make.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewAlias {
    /// Empty for a random one.
    pub prefix: String,
    /// An `AliasDomain::suffix`; `None` for the account's default.
    pub domain: Option<String>,
    /// How the service makes a random prefix; `None` for its default.
    pub format: Option<AliasFormat>,
    pub note: String,
    /// The real address its mail should go to, where the service asks.
    pub mailbox: Option<String>,
}

const WORDS: [&str; 64] = [
    "amber", "apple", "arrow", "aspen", "basil", "berry", "birch", "breeze", "brook", "cactus",
    "canyon", "cedar", "cherry", "cinder", "clover", "comet", "coral", "cotton", "daisy", "delta",
    "dune", "ember", "fable", "fern", "field", "flint", "frost", "garnet", "glade", "harbor",
    "hazel", "heron", "indigo", "ivy", "jasper", "juniper", "lagoon", "lark", "lemon", "lilac",
    "maple", "marble", "meadow", "mesa", "mint", "moss", "nectar", "oak", "olive", "orbit",
    "pebble", "pine", "plum", "quartz", "raven", "ridge", "sage", "slate", "spruce", "summit",
    "thistle", "tulip", "willow", "zephyr",
];
/// A random prefix made here, for services that take the prefix as given.
pub fn random_prefix(format: AliasFormat) -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    match format {
        AliasFormat::RandomWords => {
            let word = |rng: &mut rand::rngs::ThreadRng| WORDS[rng.random_range(0..WORDS.len())];
            format!(
                "{}.{}{}",
                word(&mut rng),
                word(&mut rng),
                rng.random_range(10..100)
            )
        }
        AliasFormat::RandomCharacters => {
            const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
            (0..10)
                .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
                .collect()
        }
        AliasFormat::Uuid => uuid::Uuid::new_v4().to_string(),
    }
}
/// A typed prefix as services accept it: lowercase, without spaces or `@`.
pub fn clean_prefix(prefix: &str) -> String {
    prefix
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect()
}
pub fn set_enabled(backend: &Backend, account: &ForwardingAccount, alias: &Alias, enabled: bool) -> Result<()> {
    anyhow::ensure!(
        account.service.manages_aliases(),
        t!("masked-email-turn-duck-addresses-off-on-duckduckgo")
    );
    account.api(backend)?.set_enabled(alias, enabled)
}
/// Deletes an alias at its service. A Duck Address, which DuckDuckGo's API
/// can't delete, is only forgotten here: it keeps forwarding until it is
/// deactivated on DuckDuckGo's site.
pub fn delete(backend: &Backend, account: &ForwardingAccount, alias: &Alias) -> Result<()> {
    if !account.service.manages_aliases() {
        return Ok(());
    }
    account.api(backend)?.delete(alias)
}
/// Forgets the account's key (for Proton's aliases, Pass's extra password,
/// if any; the Proton account keeps its sign-in). Its aliases keep working
/// at the service.
pub fn forget(account: &ForwardingAccount) {
    // Proton's aliases: their Pass session is signed out as well.
    if account.service.uses_proton_account()
        && let Ok(secret) = store::read_secret(&account.secret_id())
        && !secret.proton_uid.is_empty()
    {
        crate::sync::proton::sign_out(&secret.proton());
    }
    let _ = store::secret_entry(&account.secret_id())
        .and_then(|e| e.delete_credential().map_err(Into::into));
}

/// An alias offered in a composer's From menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendAlias {
    /// Lowercase.
    pub email: String,
    pub note: String,
    pub service: ForwardingService,
    /// The Picacho mailbox it sends through, when the service has one set.
    pub deliver_to: Option<String>,
}
/// Turned-on aliases at services that can send, in the services' order.
pub fn sendable(forwarding: &[ForwardingAccount]) -> Vec<SendAlias> {
    forwarding
        .iter()
        .filter(|f| f.service.can_send())
        .flat_map(|f| {
            f.aliases.iter().filter(|a| a.enabled).map(|a| SendAlias {
                email: a.email.clone(),
                note: a.note.clone(),
                service: f.service,
                deliver_to: f.deliver_to.clone(),
            })
        })
        .collect()
}
/// The Picacho mailboxes a service forwards to: the one it delivers to,
/// and those that are among its mailboxes.
pub fn forwards_to(f: &ForwardingAccount, accounts: &[Account]) -> Vec<String> {
    accounts
        .iter()
        .filter(|a| {
            f.deliver_to.as_deref() == Some(a.id.as_str())
                || f.mailboxes
                    .iter()
                    .any(|m| m.email.eq_ignore_ascii_case(&a.email))
        })
        .map(|a| a.id.clone())
        .collect()
}

/// Whether `account` may send only as an alias: a service that can send
/// forwards to it, with "Send only as aliases" on.
pub fn requires_alias(forwarding: &[ForwardingAccount], accounts: &[Account], account: &str) -> bool {
    forwarding.iter().any(|f| {
        f.aliases_only
            && f.service.can_send()
            && forwards_to(f, accounts).iter().any(|id| id == account)
    })
}

/// The alias `account` writes as when none was chosen: the first turned-on
/// alias (pinned ones first) of a service that keeps it to its aliases.
pub fn default_alias(
    forwarding: &[ForwardingAccount],
    accounts: &[Account],
    account: &str,
) -> Option<String> {
    forwarding
        .iter()
        .filter(|f| {
            f.aliases_only
                && f.service.can_send()
                && forwards_to(f, accounts).iter().any(|id| id == account)
        })
        .flat_map(|f| {
            let mut aliases: Vec<&Alias> = f.aliases.iter().filter(|a| a.enabled).collect();
            aliases.sort_by_key(|a| !a.pinned);
            aliases
        })
        .map(|a| a.email.clone())
        .next()
}

/// The name recipients see for `alias`, when it has one.
pub fn alias_name(forwarding: &[ForwardingAccount], alias: &str) -> Option<String> {
    forwarding
        .iter()
        .flat_map(|f| &f.aliases)
        .find(|a| a.email.eq_ignore_ascii_case(alias))
        .map(|a| a.name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// `text` with every mention of `address` (in any case) replaced by `alias`.
pub fn mask_address<'a>(text: &'a str, address: &str, alias: &str) -> std::borrow::Cow<'a, str> {
    let address = address.trim();
    if address.is_empty() || !address.is_ascii() {
        return text.into();
    }
    let lower = text.to_ascii_lowercase();
    let needle = address.to_ascii_lowercase();
    if !lower.contains(&needle) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle) {
        let at = from + i;
        out.push_str(&text[last..at]);
        out.push_str(alias);
        last = at + needle.len();
        from = last;
    }
    out.push_str(&text[last..]);
    out.into()
}

/// The sendable alias with this address, and its service account.
fn find_sendable<'a>(
    forwarding: &'a [ForwardingAccount],
    email: &str,
) -> Option<(&'a ForwardingAccount, &'a Alias)> {
    forwarding
        .iter()
        .filter(|f| f.service.can_send())
        .find_map(|f| {
            let alias = f
                .aliases
                .iter()
                .find(|a| a.email.eq_ignore_ascii_case(email.trim()))?;
            Some((f, alias))
        })
}

/// Sending failed because the alias in From no longer exists: it was
/// deleted at its service, or is no longer listed here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasGone {
    /// The alias's address.
    pub email: String,
    /// The service it was at, when known.
    pub service: Option<ForwardingService>,
}
impl std::fmt::Display for AliasGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.service {
            Some(s) => f.write_str(&t!("manage-forwarding-no-longer-exists-at", email = self.email, service = s.label())),
            None => f.write_str(&t!("manage-forwarding-is-no-longer-an-alias-picacho", email = self.email)),
        }
    }
}
impl std::error::Error for AliasGone {}

/// Before sending as an alias, asks its service whether the alias still
/// exists, since it can be deleted elsewhere. When the service can't be
/// asked, sending goes ahead and reports any problem itself.
pub fn verify_alias(backend: &Backend, draft: &Draft) -> Result<()> {
    if draft.send_as.is_empty() {
        return Ok(());
    }
    let forwarding = load(&backend.store);
    let Some((account, alias)) = find_sendable(&forwarding, &draft.send_as) else {
        return Err(AliasGone {
            email: draft.send_as.trim().to_lowercase(),
            service: None,
        }
        .into());
    };
    if let Ok(api) = account.api(backend)
        && let Ok(false) = api.alias_exists(alias)
    {
        return Err(AliasGone {
            email: alias.email.clone(),
            service: Some(account.service),
        }
        .into());
    }
    Ok(())
}

/// The draft with its recipients rewritten so the service sends it as
/// `draft.send_as`: SimpleLogin's (and Proton's) reverse aliases, or addy.io's
/// `alias+name=domain@alias-domain`. Addresses that already go through the
/// alias (as in a reply to mail it forwarded) are kept. Unchanged when the
/// draft is sent as its account.
pub fn route(backend: &Backend, draft: &Draft) -> Result<Draft> {
    if draft.send_as.is_empty() {
        return Ok(draft.clone());
    }
    let store = &backend.store;
    let forwarding = load(store);
    let Some((account, alias)) = find_sendable(&forwarding, &draft.send_as) else {
        return Err(AliasGone {
            email: draft.send_as.trim().to_lowercase(),
            service: None,
        }
        .into());
    };
    anyhow::ensure!(
        alias.enabled,
        t!("manage-forwarding-is-turned-off-at-turn-it", email = alias.email, label = account.service.label())
    );
    let fields = [&draft.to, &draft.cc, &draft.bcc]
        .map(|list| crate::sync::smtp::recipients(list))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let routed: Vec<Vec<(Option<String>, String)>> = match account.service {
        ForwardingService::Addy => fields
            .into_iter()
            .map(|list| {
                list.into_iter()
                    .map(|(name, address)| Ok((name, addy_route(&alias.email, &address)?)))
                    .collect::<Result<_>>()
            })
            .collect::<Result<_>>()?,
        // One of the account's own Proton addresses: Proton sends from it
        // itself, so the recipients stay and From names it.
        ForwardingService::Proton if alias.is_address() => {
            return Ok(Draft {
                from_address: alias.email.clone(),
                from_name: alias.name.trim().to_owned(),
                ..draft.clone()
            });
        }
        ForwardingService::SimpleLogin | ForwardingService::Proton => {
            let client = account.api(backend)?;
            let contacts = client.contacts(alias)?;
            let reverse: std::collections::HashSet<String> =
                contacts.iter().map(|c| c.address.clone()).collect();
            // A contact's existing reverse alias, by the contact's address.
            let known: HashMap<String, String> = contacts
                .iter()
                .map(|c| (c.contact.trim().to_lowercase(), c.address.clone()))
                .collect();
            let mut made: HashMap<String, String> = HashMap::new();
            let mut out = Vec::new();
            for list in fields {
                let mut field = Vec::new();
                for (name, address) in list {
                    let key = address.to_lowercase();
                    let routed = if reverse.contains(&key) {
                        address
                    } else if let Some(r) = known.get(&key) {
                        r.clone()
                    } else if let Some(r) = made.get(&key) {
                        r.clone()
                    } else {
                        let r = client.reverse_alias(alias, &address)?;
                        made.insert(key, r.clone());
                        r
                    };
                    field.push((name, routed));
                }
                out.push(field);
            }
            // Sent copies are addressed to these: note who each stands
            // for, so a mailbox kept to its aliases shows the contact.
            let _ = remember_contacts(
                store,
                contacts
                    .iter()
                    .map(|c| (c.address.clone(), c.contact.clone()))
                    .chain(made.iter().map(|(contact, r)| (r.clone(), contact.clone()))),
            );
            // And note them, so Sent can say which alias each went out as.
            let _ = remember_routes(
                store,
                &alias.email,
                reverse.into_iter().chain(made.into_values()),
            );
            out
        }
        ForwardingService::FirefoxRelay | ForwardingService::Fastmail | ForwardingService::DuckDuckGo => {
            unreachable!("{} cannot send", account.service.label())
        }
    };
    let _ = remember_routes(
        store,
        &alias.email,
        routed.iter().flatten().map(|(_, address)| address.clone()),
    );
    let join = |list: &[(Option<String>, String)]| {
        list.iter()
            .map(|(name, address)| match name {
                Some(n) if !n.is_empty() && n != address => {
                    format!("\"{}\" <{address}>", n.replace(['"', '\\'], ""))
                }
                _ => address.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(Draft {
        to: join(&routed[0]),
        cc: join(&routed[1]),
        bcc: join(&routed[2]),
        ..draft.clone()
    })
}

/// Where the reverse addresses that write as an alias are kept:
/// a JSON object of address (lowercase) to the alias's address.
const ROUTES_PREFERENCE: &str = "alias_routes";

/// The reverse addresses known to write as an alias, to tell which alias a
/// sent message went out as: its recipients are those addresses.
pub fn routes(store: &Store) -> HashMap<String, String> {
    store
        .preference(ROUTES_PREFERENCE)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Notes that `addresses` write as `alias`. Saving is serialized, since
/// sends and the Forwarding settings can both note some at once.
pub fn remember_routes<I: IntoIterator<Item = String>>(
    store: &Store,
    alias: &str,
    addresses: I,
) -> Result<()> {
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = routes(store);
    let alias = alias.trim().to_lowercase();
    let mut changed = false;
    for address in addresses {
        let address = address.trim().to_lowercase();
        if !address.is_empty() && all.get(&address) != Some(&alias) {
            all.insert(address, alias.clone());
            changed = true;
        }
    }
    if changed {
        store.set_preference(ROUTES_PREFERENCE, &serde_json::to_string(&all)?)?;
    }
    Ok(())
}

/// Where the contacts SimpleLogin's reverse aliases stand for are kept:
/// a JSON object of reverse alias (lowercase) to the contact's address.
const CONTACTS_PREFERENCE: &str = "reverse_contacts";

/// The contacts known behind reverse aliases: reverse alias (lowercase)
/// to the contact's own address.
pub fn reverse_contacts(store: &Store) -> HashMap<String, String> {
    store
        .preference(CONTACTS_PREFERENCE)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Notes the contacts reverse aliases stand for: (reverse alias, contact).
pub fn remember_contacts<I: IntoIterator<Item = (String, String)>>(
    store: &Store,
    pairs: I,
) -> Result<()> {
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _saving = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = reverse_contacts(store);
    let mut changed = false;
    for (reverse, contact) in pairs {
        let (reverse, contact) = (reverse.trim().to_lowercase(), contact.trim().to_owned());
        if !reverse.is_empty() && !contact.is_empty() && all.get(&reverse) != Some(&contact) {
            all.insert(reverse, contact);
            changed = true;
        }
    }
    if changed {
        store.set_preference(CONTACTS_PREFERENCE, &serde_json::to_string(&all)?)?;
    }
    Ok(())
}

/// Fetches a SimpleLogin (or Proton) alias's contacts and notes who each
/// of its reverse aliases stands for.
pub fn learn_contacts(backend: &Backend, account: &ForwardingAccount, alias: &Alias) -> Result<()> {
    let contacts = account.api(backend)?.contacts(alias)?;
    remember_contacts(&backend.store, contacts.into_iter().map(|c| (c.address, c.contact)))
}

/// A recipient list with each known reverse alias in it replaced by the
/// contact it forwards to.
pub fn unreverse<'a>(
    list: &'a str,
    contacts: &HashMap<String, String>,
) -> std::borrow::Cow<'a, str> {
    if contacts.is_empty() {
        return list.into();
    }
    let mut out: std::borrow::Cow<'a, str> = list.into();
    for (_, email) in crate::sync::smtp::recipients(list).unwrap_or_default() {
        if let Some(contact) = contacts.get(&email.trim().to_lowercase()) {
            out = mask_address(&out, &email, contact).into_owned().into();
        }
    }
    out
}

/// The alias a message you sent went out as, if any: a recipient is a
/// reverse address noted for it, an addy.io routed address
/// (`alias+name=domain@alias-domain`) of a known alias, or the message is
/// from the alias itself.
pub fn sent_as(
    from: &str,
    to: &str,
    cc: &str,
    forwarding: &[ForwardingAccount],
    routes: &HashMap<String, String>,
) -> Option<String> {
    let known = |email: &str| {
        forwarding
            .iter()
            .flat_map(|f| &f.aliases)
            .any(|a| a.email.eq_ignore_ascii_case(email))
    };
    if !from.is_empty() && known(from) {
        return Some(from.trim().to_lowercase());
    }
    let recipients = [to, cc]
        .into_iter()
        .flat_map(|list| crate::sync::smtp::recipients(list).unwrap_or_default())
        .map(|(_, email)| email.to_lowercase());
    for email in recipients {
        if let Some(alias) = routes.get(&email) {
            return Some(alias.clone());
        }
        if let Some(alias) = addy_alias(&email, forwarding) {
            return Some(alias);
        }
    }
    None
}

/// The addy.io alias a routed address (`alias+name=domain@alias-domain`)
/// writes as, when it is one of the person's aliases.
fn addy_alias(address: &str, forwarding: &[ForwardingAccount]) -> Option<String> {
    let (local, domain) = address.rsplit_once('@')?;
    let (alias_local, rest) = local.split_once('+')?;
    if !rest.contains('=') {
        return None;
    }
    let alias = format!("{alias_local}@{domain}");
    forwarding
        .iter()
        .filter(|f| f.service == ForwardingService::Addy)
        .flat_map(|f| &f.aliases)
        .any(|a| a.email.eq_ignore_ascii_case(&alias))
        .then_some(alias)
}

/// addy.io's address for writing to `recipient` as `alias`:
/// `alias-local+recipient-local=recipient-domain@alias-domain`. An address
/// already in that form for this alias is kept.
fn addy_route(alias: &str, recipient: &str) -> Result<String> {
    let (alias_local, alias_domain) = alias
        .rsplit_once('@')
        .with_context(|| t!("manage-forwarding-is-not-an-email-address", alias = alias))?;
    let (local, domain) = recipient
        .rsplit_once('@')
        .with_context(|| t!("manage-forwarding-is-not-an-email-address-2", recipient = recipient))?;
    if domain.eq_ignore_ascii_case(alias_domain)
        && local
            .to_lowercase()
            .strip_prefix(&format!("{}+", alias_local.to_lowercase()))
            .is_some_and(|rest| rest.contains('='))
    {
        return Ok(recipient.to_owned());
    }
    Ok(format!("{alias_local}+{local}={domain}@{alias_domain}"))
}

/// The view id of an alias shown on its own.
pub fn alias_view(account: &str, alias: &str) -> String {
    format!("{ALIAS_PREFIX}{account}:{alias}")
}
/// Whether `id` is All Accounts or an alias rather than a real mailbox.
pub fn is_view(id: &str) -> bool {
    id == ALL_MAIL || id.starts_with(ALIAS_PREFIX)
}
/// The alias a view id names, with its service account.
pub fn find_alias<'a>(
    forwarding: &'a [ForwardingAccount],
    view: &str,
) -> Option<(&'a ForwardingAccount, &'a Alias)> {
    let rest = view.strip_prefix(ALIAS_PREFIX)?;
    forwarding.iter().find_map(|f| {
        let alias_id = rest.strip_prefix(f.id.as_str())?.strip_prefix(':')?;
        Some((f, f.aliases.iter().find(|a| a.id == alias_id)?))
    })
}
/// View ids of the aliases shown as accounts, in order.
pub fn separate_views(forwarding: &[ForwardingAccount]) -> Vec<String> {
    forwarding
        .iter()
        .flat_map(|f| {
            f.aliases
                .iter()
                .filter(|a| a.display == AliasDisplay::Separate)
                .map(|a| alias_view(&f.id, &a.id))
        })
        .collect()
}

/// The addresses a message was sent to, lowercase.
fn recipients<'a>(to: &'a str, cc: &'a str) -> impl Iterator<Item = String> + 'a {
    to.split([',', ';', '<', '>', ' ', '"', '\t', '\n'])
        .chain(cc.split([',', ';', '<', '>', ' ', '"', '\t', '\n']))
        .filter(|s| s.contains('@'))
        .map(|s| s.trim().trim_matches(['\'', '(', ')']).to_lowercase())
}

/// Which mail a view lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// The mailboxes it draws from.
    pub accounts: Vec<String>,
    /// Lists only mail to one of these addresses, when any are given.
    pub only: HashSet<String>,
    /// Leaves out mail to these (aliases shown on their own).
    pub hide: HashSet<String>,
}
impl Scope {
    pub fn filters(&self) -> bool {
        !self.only.is_empty() || !self.hide.is_empty()
    }
    pub fn admits(&self, m: &Mail) -> bool {
        self.admits_recipients(&m.to, m.cc())
    }
    fn admits_recipients(&self, to: &str, cc: &str) -> bool {
        if !self.filters() {
            return true;
        }
        let mut only = self.only.is_empty();
        for r in recipients(to, cc) {
            if self.hide.contains(&r) {
                return false;
            }
            only |= self.only.contains(&r);
        }
        only
    }
}
/// Separate aliases whose mail leaves `account`'s own lists.
fn hidden_from(forwarding: &[ForwardingAccount], account: &str) -> HashSet<String> {
    forwarding
        .iter()
        .filter(|f| f.deliver_to.as_deref().is_none_or(|d| d == account))
        .flat_map(|f| &f.aliases)
        .filter(|a| a.display == AliasDisplay::Separate)
        .map(|a| a.email.clone())
        .collect()
}
/// What the mailbox `folder` of `view` (an account, All Accounts or an alias) lists.
pub fn scope(
    view: &str,
    folder: Folder,
    accounts: &[Account],
    forwarding: &[ForwardingAccount],
) -> Scope {
    let all: Vec<String> = accounts.iter().map(|a| a.id.clone()).collect();
    if view == ALL_MAIL {
        return Scope {
            accounts: all,
            ..Scope::default()
        };
    }
    if let Some((f, alias)) = find_alias(forwarding, view) {
        return Scope {
            accounts: match &f.deliver_to {
                Some(id) if all.contains(id) => vec![id.clone()],
                _ => all,
            },
            only: HashSet::from([alias.email.clone()]),
            hide: HashSet::new(),
        };
    }
    Scope {
        accounts: vec![view.to_owned()],
        only: HashSet::new(),
        // Messages written here stay with the mailbox they were written in.
        hide: if matches!(folder, Folder::Sent | Folder::Drafts) {
            HashSet::new()
        } else {
            hidden_from(forwarding, view)
        },
    }
}

/// An unread message's place and recipients, for counting views.
pub type UnreadRow = (String, String, String, String);

/// Sidebar unread counts for every view: mailboxes less the mail of aliases
/// shown on their own, those aliases, and All Accounts.
pub fn view_counts(
    raw: HashMap<(String, String), usize>,
    rows: &[UnreadRow],
    accounts: &[Account],
    forwarding: &[ForwardingAccount],
    all_mail: bool,
) -> HashMap<(String, String), usize> {
    let mut out = raw.clone();
    if all_mail {
        for ((account, folder), n) in &raw {
            if accounts.iter().any(|a| &a.id == account) && !folder.starts_with("remote:") {
                *out.entry((ALL_MAIL.into(), folder.clone())).or_default() += n;
            }
        }
    }
    let views = separate_views(forwarding);
    if views.is_empty() {
        return out;
    }
    let sent = [Folder::Sent.key(), Folder::Drafts.key()];
    for (account, folder, to, cc) in rows {
        if sent.contains(&folder.as_str()) {
            continue;
        }
        let shown = scope(account, Folder::Inbox, accounts, forwarding);
        if !shown.admits_recipients(to, cc)
            && let Some(n) = out.get_mut(&(account.clone(), folder.clone()))
        {
            *n = n.saturating_sub(1);
        }
        for view in &views {
            let s = scope(view, Folder::Inbox, accounts, forwarding);
            if s.accounts.contains(account) && s.admits_recipients(to, cc) {
                *out.entry((view.clone(), folder.clone())).or_default() += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn addy_routes_each_recipient_through_the_alias() {
        assert_eq!(
            addy_route("shop@me.anonaddy.com", "ana@example.org").unwrap(),
            "shop+ana=example.org@me.anonaddy.com"
        );
        // A reply address addy.io already gave is kept.
        assert_eq!(
            addy_route(
                "shop@me.anonaddy.com",
                "shop+ana=example.org@me.anonaddy.com"
            )
            .unwrap(),
            "shop+ana=example.org@me.anonaddy.com"
        );
        assert!(addy_route("shop@me.anonaddy.com", "nobody").is_err());
    }

    #[test]
    fn mailboxes_kept_to_their_aliases() {
        let account = |id: &str, email: &str| Account {
            id: id.into(),
            email: email.into(),
            ..Account::new(Provider::Imap)
        };
        let accounts = [account("w", "work@example.com"), account("h", "home@example.com")];
        let alias = |email: &str, enabled, pinned| Alias {
            id: email.into(),
            email: email.into(),
            enabled,
            pinned,
            ..Alias::default()
        };
        let mut services = vec![ForwardingAccount {
            id: "sl".into(),
            service: ForwardingService::SimpleLogin,
            deliver_to: Some("w".into()),
            mailboxes: vec![ServiceMailbox {
                id: "1".into(),
                email: "HOME@example.com".into(),
            }],
            aliases: vec![
                alias("off@sl.io", false, true),
                alias("a@sl.io", true, false),
                alias("b@sl.io", true, true),
            ],
            ..ForwardingAccount::default()
        }];
        assert_eq!(forwards_to(&services[0], &accounts), ["w", "h"]);
        assert!(!requires_alias(&services, &accounts, "w"), "off by default");
        services[0].aliases_only = true;
        assert!(requires_alias(&services, &accounts, "w"));
        assert!(requires_alias(&services, &accounts, "h"));
        // A pinned, turned-on alias first.
        assert_eq!(default_alias(&services, &accounts, "w").as_deref(), Some("b@sl.io"));
        // Firefox Relay can't send, so it never keeps a mailbox from sending.
        services[0].service = ForwardingService::FirefoxRelay;
        assert!(!requires_alias(&services, &accounts, "w"));

        assert_eq!(
            mask_address("To: Work <WORK@example.com>, work@example.com", "work@example.com", "a@sl.io"),
            "To: Work <a@sl.io>, a@sl.io"
        );
        assert!(matches!(
            mask_address("nothing here", "work@example.com", "a@sl.io"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn reverse_aliases_show_as_their_contacts() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        remember_contacts(
            &store,
            [
                ("RA+abc@SimpleLogin.co".to_owned(), "ana@example.org".to_owned()),
                ("ra+def@simplelogin.co".to_owned(), "bo@example.net".to_owned()),
            ],
        )?;
        let contacts = reverse_contacts(&store);
        assert_eq!(contacts.len(), 2);
        assert_eq!(
            unreverse("Ana <RA+abc@simplelogin.co>, ra+def@simplelogin.co, cy@example.com", &contacts),
            "Ana <ana@example.org>, bo@example.net, cy@example.com"
        );
        assert!(matches!(
            unreverse("cy@example.com", &contacts),
            std::borrow::Cow::Borrowed(_)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Ok(())
    }

    #[test]
    fn sent_mail_names_the_alias_it_went_out_as() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        let alias = |email: &str| Alias {
            id: email.into(),
            email: email.into(),
            enabled: true,
            ..Alias::default()
        };
        let forwarding = [
            ForwardingAccount {
                id: "addy".into(),
                service: ForwardingService::Addy,
                aliases: vec![alias("shop@me.anonaddy.com")],
                ..ForwardingAccount::default()
            },
            ForwardingAccount {
                id: "sl".into(),
                service: ForwardingService::SimpleLogin,
                aliases: vec![alias("news@sl.io")],
                ..ForwardingAccount::default()
            },
        ];
        // SimpleLogin's reverse aliases are opaque: they are noted when used.
        remember_routes(&store, "News@sl.io", ["RA+abc@SimpleLogin.co".to_owned()])?;
        remember_routes(&store, "news@sl.io", ["ra+def@simplelogin.co".to_owned()])?;
        let routes = routes(&store);
        assert_eq!(routes.len(), 2);
        let me = "me@example.com";
        let sent = |to: &str, cc: &str| sent_as(me, to, cc, &forwarding, &routes);
        assert_eq!(
            sent("\"ana at example.org\" <ra+abc@simplelogin.co>", "").as_deref(),
            Some("news@sl.io")
        );
        assert_eq!(sent("bo@example.net", "ra+def@simplelogin.co").as_deref(), Some("news@sl.io"));
        // addy.io's routed addresses name their alias.
        assert_eq!(
            sent("shop+ana=example.org@me.anonaddy.com", "").as_deref(),
            Some("shop@me.anonaddy.com")
        );
        // Only the person's own aliases, and not plain plus addressing.
        assert_eq!(sent("other+ana=example.org@me.anonaddy.com", ""), None);
        assert_eq!(sent("shop+news@me.anonaddy.com", ""), None);
        assert_eq!(sent("ana@example.org", ""), None);
        // Sent from the alias's own address.
        assert_eq!(
            sent_as("news@sl.io", "ana@example.org", "", &forwarding, &routes).as_deref(),
            Some("news@sl.io")
        );
        Ok(())
    }

    #[test]
    fn drafts_sent_as_an_alias_are_routed_and_others_unchanged() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(Store::open(&p)?);
        let backend = Backend::new(store.clone());
        let alias = |email: &str, enabled| Alias {
            id: email.into(),
            email: email.into(),
            enabled,
            ..Alias::default()
        };
        save(
            &store,
            &[
                ForwardingAccount {
                    id: "addy".into(),
                    service: ForwardingService::Addy,
                    aliases: vec![
                        alias("shop@me.anonaddy.com", true),
                        alias("old@me.anonaddy.com", false),
                    ],
                    ..ForwardingAccount::default()
                },
                ForwardingAccount {
                    id: "relay".into(),
                    service: ForwardingService::FirefoxRelay,
                    aliases: vec![alias("x@mozmail.com", true)],
                    ..ForwardingAccount::default()
                },
            ],
        )?;
        let draft = Draft {
            to: "Ana <ana@example.org>, bo@example.net".into(),
            cc: "cy@example.com".into(),
            ..Draft::default()
        };
        assert_eq!(route(&backend, &draft)?.to, draft.to, "sent as the account");
        let routed = route(
            &backend,
            &Draft {
                send_as: "Shop@me.anonaddy.com".into(),
                ..draft.clone()
            },
        )?;
        assert_eq!(
            routed.to,
            "\"Ana\" <shop+ana=example.org@me.anonaddy.com>, shop+bo=example.net@me.anonaddy.com"
        );
        assert_eq!(routed.cc, "shop+cy=example.com@me.anonaddy.com");
        // Turned-off aliases and services that cannot send are refused.
        for send_as in ["old@me.anonaddy.com", "x@mozmail.com"] {
            let draft = Draft {
                send_as: send_as.into(),
                ..draft.clone()
            };
            assert!(route(&backend, &draft).is_err(), "{send_as}");
        }
        // An alias no longer listed is reported as gone, so From can be reset.
        let gone = route(
            &backend,
            &Draft {
                send_as: "Deleted@me.anonaddy.com".into(),
                ..draft.clone()
            },
        )
        .unwrap_err();
        assert_eq!(
            gone.downcast_ref::<AliasGone>(),
            Some(&AliasGone {
                email: "deleted@me.anonaddy.com".into(),
                service: None
            })
        );
        let offered: Vec<String> = sendable(&load(&store))
            .into_iter()
            .map(|a| a.email)
            .collect();
        assert_eq!(offered, ["shop@me.anonaddy.com"]);
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }

    use super::*;
    use crate::manage::model::Provider;

    fn account(id: &str) -> Account {
        Account {
            id: id.into(),
            email: format!("{id}@example.com"),
            ..Account::new(Provider::Imap)
        }
    }
    fn alias(id: &str, email: &str, display: AliasDisplay) -> Alias {
        Alias {
            id: id.into(),
            email: email.into(),
            enabled: true,
            display,
            ..Alias::default()
        }
    }
    fn service(deliver_to: Option<&str>) -> ForwardingAccount {
        ForwardingAccount {
            id: "sl".into(),
            service: ForwardingService::SimpleLogin,
            deliver_to: deliver_to.map(Into::into),
            aliases: vec![
                alias("1", "shop@sl.io", AliasDisplay::Separate),
                alias("2", "news@sl.io", AliasDisplay::Included),
            ],
            ..ForwardingAccount::default()
        }
    }
    fn mail(account: &str, to: &str) -> Mail {
        Mail {
            account: account.into(),
            to: to.into(),
            ..Mail::default()
        }
    }

    #[test]
    fn separate_aliases_move_their_mail_out_of_the_mailbox() {
        let accounts = [account("a"), account("b")];
        let fwd = [service(Some("a"))];
        let inbox = scope("a", Folder::Inbox, &accounts, &fwd);
        assert!(!inbox.admits(&mail("a", "Shop <SHOP@sl.io>")));
        assert!(inbox.admits(&mail("a", "news@sl.io")));
        assert!(inbox.admits(&mail("a", "a@example.com")));
        // Not a substring match.
        assert!(inbox.admits(&mail("a", "workshop@sl.io")));
        // Another mailbox and Sent are untouched.
        assert!(!scope("b", Folder::Inbox, &accounts, &fwd).filters());
        assert!(!scope("a", Folder::Sent, &accounts, &fwd).filters());

        let view = alias_view("sl", "1");
        let shop = scope(&view, Folder::Inbox, &accounts, &fwd);
        assert_eq!(shop.accounts, vec!["a".to_string()]);
        assert!(shop.admits(&mail("a", "x@y.z, shop@sl.io")));
        assert!(!shop.admits(&mail("a", "news@sl.io")));
        assert_eq!(separate_views(&fwd), vec![view.clone()]);
        assert_eq!(find_alias(&fwd, &view).unwrap().1.email, "shop@sl.io");
        assert!(is_view(&view) && is_view(ALL_MAIL) && !is_view("a"));
    }

    #[test]
    fn an_alias_without_a_mailbox_looks_in_all_of_them() {
        let accounts = [account("a"), account("b")];
        let fwd = [service(None)];
        assert_eq!(
            scope(&alias_view("sl", "1"), Folder::Inbox, &accounts, &fwd).accounts,
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(scope("b", Folder::Inbox, &accounts, &fwd).filters());
        let all = scope(ALL_MAIL, Folder::Junk, &accounts, &fwd);
        assert_eq!(all.accounts.len(), 2);
        assert!(all.admits(&mail("a", "shop@sl.io")));
    }

    #[test]
    fn counts_follow_the_views() {
        let accounts = [account("a"), account("b")];
        let fwd = [service(Some("a"))];
        let raw = HashMap::from([
            (("a".to_string(), "inbox".to_string()), 3),
            (("b".to_string(), "inbox".to_string()), 2),
            (("b".to_string(), "junk".to_string()), 1),
        ]);
        let rows: Vec<UnreadRow> = vec![
            (
                "a".into(),
                "inbox".into(),
                "shop@sl.io".into(),
                String::new(),
            ),
            (
                "a".into(),
                "inbox".into(),
                "a@example.com".into(),
                String::new(),
            ),
            (
                "a".into(),
                "inbox".into(),
                "news@sl.io".into(),
                String::new(),
            ),
        ];
        let counts = view_counts(raw, &rows, &accounts, &fwd, true);
        let get = |a: &str, f: &str| counts.get(&(a.into(), f.into())).copied().unwrap_or(0);
        assert_eq!(get("a", "inbox"), 2);
        assert_eq!(get(&alias_view("sl", "1"), "inbox"), 1);
        assert_eq!(get(ALL_MAIL, "inbox"), 5);
        assert_eq!(get(ALL_MAIL, "junk"), 1);
    }

    #[test]
    fn services_saved_before_names_and_pins_still_load() {
        let saved = r#"[{"id":"sl","service":"SimpleLogin","deliver_to":null,
            "aliases":[{"id":"1","email":"a@b.c","enabled":true,"note":"x","display":"Separate"}]}]"#;
        let loaded: Vec<ForwardingAccount> = serde_json::from_str(saved).unwrap();
        let a = &loaded[0].aliases[0];
        assert_eq!((a.name.as_str(), a.pinned, a.pgp), ("", false, None));
        assert_eq!(a.display, AliasDisplay::Separate);
        assert!(loaded[0].mailboxes.is_empty());
    }

    #[test]
    fn stats_read_briefly_without_zeros() {
        let stats = AliasStats {
            forwarded: 12,
            blocked: 0,
            replied: 3,
            sent: Some(1),
        };
        assert_eq!(stats.summary(), "12 forwarded · 3 replied · 1 sent");
        assert_eq!(AliasStats::default().summary(), "");
    }

    #[test]
    fn reverse_aliases_are_hidden_entirely() {
        let a = obscure("ra+x7k2abcd@simplelogin.co");
        let b = obscure("ab@x.cc");
        assert_eq!(a, b, "not even the length shows");
        assert!(!a.contains('@') && !a.chars().any(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn contacts_count_their_mail_recently_and_all_time() {
        let day = 24 * 60 * 60;
        let now = 100 * day;
        let contact = |id: &str, who: &str, ra: &str| ReverseAlias {
            id: id.into(),
            contact: who.into(),
            address: ra.into(),
            ..ReverseAlias::default()
        };
        let mut list = vec![
            contact("1", "ana@example.org", "ra+ana@sl.co"),
            contact("2", "Bo@Example.org", "ra+bo@sl.co"),
        ];
        let act = |kind, ra: &str, who: &str, days_ago: i64| Activity {
            kind,
            reverse_alias: ra.into(),
            contact: who.into(),
            timestamp: now - days_ago * day,
        };
        let activity = vec![
            act(ActivityKind::Forward, "ra+ana@sl.co", "ana@example.org", 1),
            act(ActivityKind::Forward, "ra+ana@sl.co", "ana@example.org", 30),
            act(ActivityKind::Reply, "ra+ana@sl.co", "ana@example.org", 2),
            act(ActivityKind::Other, "ra+ana@sl.co", "ana@example.org", 2),
            // No reverse alias given: matched by the contact's address.
            act(ActivityKind::Reply, "", "bo@example.org", 20),
            act(ActivityKind::Forward, "ra+someone@sl.co", "x@example.org", 1),
        ];
        tally(&mut list, &activity, false, now);
        assert_eq!(
            list[0].stats,
            Some(ContactStats {
                received_recent: 1,
                received: 2,
                sent_recent: 1,
                sent: 1,
                partial: false,
            })
        );
        assert_eq!(
            list[1].stats,
            Some(ContactStats {
                sent: 1,
                ..ContactStats::default()
            })
        );
    }

    #[test]
    fn refreshed_aliases_keep_how_they_are_shown() {
        let f = service(None);
        let merged = f.merge(vec![
            alias("1", "shop@sl.io", AliasDisplay::Included),
            alias("3", "new@sl.io", AliasDisplay::Included),
        ]);
        assert_eq!(merged[0].display, AliasDisplay::Separate);
        assert_eq!(merged[1].display, AliasDisplay::Included);
    }

    #[test]
    fn proton_addresses_send_as_themselves() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(Store::open(&p)?);
        let backend = Backend::new(store.clone());
        save(
            &store,
            &[ForwardingAccount {
                id: "proton".into(),
                service: ForwardingService::Proton,
                aliases: vec![Alias {
                    id: "addr.a1".into(),
                    email: "me@example.org".into(),
                    enabled: true,
                    name: "Jo".into(),
                    proton_address: Some(ProtonAddressKind::Custom),
                    ..Alias::default()
                }],
                ..ForwardingAccount::default()
            }],
        )?;
        let draft = Draft {
            to: "Ana <ana@example.org>".into(),
            send_as: "Me@Example.org".into(),
            ..Draft::default()
        };
        // No reverse aliases: the recipients stay, and From names the address.
        let routed = route(&backend, &draft)?;
        assert_eq!(routed.to, draft.to);
        assert_eq!((routed.from_address.as_str(), routed.from_name.as_str()), ("me@example.org", "Jo"));
        assert_eq!(sendable(&load(&store))[0].email, "me@example.org");
        Ok(())
    }

    #[test]
    fn proton_aliases_come_from_proton_accounts_signed_in_directly() {
        let proton = |id: &str| Account {
            id: id.into(),
            email: format!("{id}@proton.me"),
            ..Account::new(Provider::Proton)
        };
        let accounts = [account("imap"), proton("p1"), proton("p2")];
        let listed = [ForwardingAccount {
            id: "f".into(),
            service: ForwardingService::Proton,
            proton_account: "p1".into(),
            ..ForwardingAccount::default()
        }];
        // Each Proton account's aliases are listed once; others aren't Proton.
        let open: Vec<&str> = proton_accounts(&listed, &accounts).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(open, ["p2"]);
        assert_eq!(proton_accounts(&[], &accounts).len(), 2);
        // Proton's aliases never go through an API key.
        assert!(Client::new(ForwardingService::Proton, "", "key").is_err());
        let p = ForwardingService::Proton;
        assert!(p.uses_proton_account() && p.can_send() && p.has_contacts() && p.needs_mailbox() && p.takes_prefix());
        assert!(!ForwardingService::SimpleLogin.uses_proton_account());
    }
}
