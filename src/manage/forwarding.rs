//! Alias forwarding services (SimpleLogin, addy.io, Firefox Relay) and the
//! mailbox views they add: an alias shown as an account of its own, and All
//! Mail across every account.
//!
//! Forwarded mail arrives in an ordinary mailbox, still addressed to the
//! alias. An alias shown on its own lists the mail addressed to it, and the
//! mailbox it arrives in no longer does; one included with the email changes
//! nothing. The service accounts are kept in preferences and their API keys
//! in the system keychain, like mailbox passwords.
use crate::manage::{
    model::{Account, Draft, Folder, Mail, Secret},
    store::{self, Store},
};
use crate::sync::forwarding::Client;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Preference holding the service accounts.
const PREFERENCE: &str = "forwarding";
/// Preference: list All Mail when there are several accounts ("true"/"false").
pub const ALL_MAIL_PREFERENCE: &str = "all_mail";
/// The All Mail view's id, used where an account id goes.
pub const ALL_MAIL: &str = "all-mail";
const ALIAS_PREFIX: &str = "alias:";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForwardingService {
    #[default]
    SimpleLogin,
    Addy,
    FirefoxRelay,
}
impl ForwardingService {
    pub const ALL: [Self; 3] = [Self::SimpleLogin, Self::Addy, Self::FirefoxRelay];
    pub fn label(self) -> &'static str {
        match self {
            Self::SimpleLogin => "SimpleLogin",
            Self::Addy => "addy.io",
            Self::FirefoxRelay => "Firefox Relay",
        }
    }
    pub fn default_server(self) -> &'static str {
        match self {
            Self::SimpleLogin => "https://app.simplelogin.io",
            Self::Addy => "https://app.addy.io",
            Self::FirefoxRelay => "https://relay.firefox.com",
        }
    }
    /// Whether mail can be written as an alias, not only replied to.
    /// SimpleLogin does it with reverse aliases and addy.io with
    /// `alias+name=domain` addresses; Firefox Relay only relays replies.
    pub fn can_send(self) -> bool {
        !matches!(self, Self::FirefoxRelay)
    }
    /// Where the API key is made, in one sentence, and the page.
    pub fn key_help(self) -> (&'static str, &'static str) {
        match self {
            Self::SimpleLogin => (
                "Make an API key in SimpleLogin under API Keys.",
                "https://app.simplelogin.io/dashboard/api_key",
            ),
            Self::Addy => (
                "Make an API access token in addy.io under Settings, API.",
                "https://app.addy.io/settings/api",
            ),
            Self::FirefoxRelay => (
                "Copy the API key from the bottom of Firefox Relay's settings.",
                "https://relay.firefox.com/accounts/settings/",
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}
impl ForwardingAccount {
    pub fn title(&self) -> String {
        if self.name.is_empty() {
            self.service.label().into()
        } else {
            format!("{} · {}", self.service.label(), self.name)
        }
    }
    fn secret_id(&self) -> String {
        secret_id(&self.id)
    }
    fn client(&self) -> Result<Client> {
        let key = store::read_secret(&self.secret_id())?.password;
        Client::new(self.service, &self.server, &key)
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
    let client = Client::new(service, server, key)?;
    let profile = client.profile()?;
    let aliases = client.aliases()?;
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
/// The account's aliases as the service lists them now.
pub fn refresh(account: &ForwardingAccount) -> Result<Vec<Alias>> {
    Ok(account.merge(account.client()?.aliases()?))
}
pub fn create(account: &ForwardingAccount, note: &str) -> Result<Alias> {
    account.client()?.create(note)
}
pub fn set_enabled(account: &ForwardingAccount, alias: &Alias, enabled: bool) -> Result<()> {
    account.client()?.set_enabled(alias, enabled)
}
pub fn delete(account: &ForwardingAccount, alias: &Alias) -> Result<()> {
    account.client()?.delete(alias)
}
/// Forgets the account's key. Its aliases keep working at the service.
pub fn forget(account: &ForwardingAccount) {
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

/// The draft with its recipients rewritten so the service sends it as
/// `draft.send_as`: SimpleLogin's reverse aliases, or addy.io's
/// `alias+name=domain@alias-domain`. Addresses that already go through the
/// alias (as in a reply to mail it forwarded) are kept. Unchanged when the
/// draft is sent as its account.
pub fn route(store: &Store, draft: &Draft) -> Result<Draft> {
    if draft.send_as.is_empty() {
        return Ok(draft.clone());
    }
    let forwarding = load(store);
    let Some((account, alias)) = find_sendable(&forwarding, &draft.send_as) else {
        anyhow::bail!(
            "{} is no longer an alias Picacho can send as. Choose another address in From.",
            draft.send_as
        );
    };
    anyhow::ensure!(
        alias.enabled,
        "{} is turned off at {}. Turn it on in Forwarding settings, or choose another address in From.",
        alias.email,
        account.service.label()
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
        ForwardingService::SimpleLogin => {
            let client = account.client()?;
            let reverse = client.reverse_aliases(alias)?;
            let mut made: HashMap<String, String> = HashMap::new();
            let mut out = Vec::new();
            for list in fields {
                let mut field = Vec::new();
                for (name, address) in list {
                    let key = address.to_lowercase();
                    let routed = if reverse.contains(&key) {
                        address
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
            out
        }
        ForwardingService::FirefoxRelay => unreachable!("Firefox Relay cannot send"),
    };
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

/// addy.io's address for writing to `recipient` as `alias`:
/// `alias-local+recipient-local=recipient-domain@alias-domain`. An address
/// already in that form for this alias is kept.
fn addy_route(alias: &str, recipient: &str) -> Result<String> {
    let (alias_local, alias_domain) = alias
        .rsplit_once('@')
        .with_context(|| format!("{alias} is not an email address"))?;
    let (local, domain) = recipient
        .rsplit_once('@')
        .with_context(|| format!("{recipient} is not an email address"))?;
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
/// Whether `id` is All Mail or an alias rather than a real mailbox.
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
        self.admits_recipients(&m.to, &m.cc)
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
/// What the mailbox `folder` of `view` (an account, All Mail or an alias) lists.
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
/// shown on their own, those aliases, and All Mail.
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
    fn drafts_sent_as_an_alias_are_routed_and_others_unchanged() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        let alias = |email: &str, enabled| Alias {
            id: email.into(),
            email: email.into(),
            enabled,
            note: String::new(),
            display: AliasDisplay::Included,
        };
        save(
            &store,
            &[
                ForwardingAccount {
                    id: "addy".into(),
                    service: ForwardingService::Addy,
                    server: String::new(),
                    name: String::new(),
                    deliver_to: None,
                    aliases: vec![
                        alias("shop@me.anonaddy.com", true),
                        alias("old@me.anonaddy.com", false),
                    ],
                },
                ForwardingAccount {
                    id: "relay".into(),
                    service: ForwardingService::FirefoxRelay,
                    server: String::new(),
                    name: String::new(),
                    deliver_to: None,
                    aliases: vec![alias("x@mozmail.com", true)],
                },
            ],
        )?;
        let draft = Draft {
            to: "Ana <ana@example.org>, bo@example.net".into(),
            cc: "cy@example.com".into(),
            ..Draft::default()
        };
        assert_eq!(route(&store, &draft)?.to, draft.to, "sent as the account");
        let routed = route(
            &store,
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
            assert!(route(&store, &draft).is_err(), "{send_as}");
        }
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
            note: String::new(),
            display,
        }
    }
    fn service(deliver_to: Option<&str>) -> ForwardingAccount {
        ForwardingAccount {
            id: "sl".into(),
            service: ForwardingService::SimpleLogin,
            server: String::new(),
            name: String::new(),
            deliver_to: deliver_to.map(Into::into),
            aliases: vec![
                alias("1", "shop@sl.io", AliasDisplay::Separate),
                alias("2", "news@sl.io", AliasDisplay::Included),
            ],
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
    fn refreshed_aliases_keep_how_they_are_shown() {
        let f = service(None);
        let merged = f.merge(vec![
            alias("1", "shop@sl.io", AliasDisplay::Included),
            alias("3", "new@sl.io", AliasDisplay::Included),
        ]);
        assert_eq!(merged[0].display, AliasDisplay::Separate);
        assert_eq!(merged[1].display, AliasDisplay::Included);
    }
}
