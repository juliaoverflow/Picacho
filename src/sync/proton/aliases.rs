//! Proton's own email aliases ("hide-my-email" aliases), for a Proton
//! account signed in without Bridge: the same session and keys as its mail.
//!
//! Proton keeps them apart from a SimpleLogin account of one's own, though
//! SimpleLogin forwards their mail: each alias is an item in one of the
//! account's Proton Pass vaults, and is managed through Pass's API. The
//! library's Vault Client (`protected_email_bridge::pass`) makes the calls
//! and does Pass's encryption; this only puts what it returns into
//! Picacho's forwarding types.
//!
//! - Listing needs no decryption: an alias item says its address and
//!   whether it is turned off in the clear, and the alias's details
//!   (description, display name, mailboxes, 14 days of counts) are asked
//!   for in bulk.
//! - Making one makes its end-to-end encrypted Pass item too, so the vault
//!   it goes in is opened (its key unwrapped with, and verified against,
//!   the account's user key). An account that has never used Pass gets a
//!   vault for them.
//! - Contacts and their reverse aliases work as SimpleLogin's do.
//! - The account's own addresses, all but its original one, are listed
//!   with them (from its organisation membership, `members`, where Proton
//!   lists each member's addresses): turned on and off, named and deleted
//!   as Proton's address settings do. Proton sends from them itself, so
//!   they have no reverse aliases, and neither descriptions nor pins.
//!
//! An address's id here is `addr.{address}`, an alias's `{share}.{item}`,
//! and a contact's is
//! `{share}.{item}.{contact}`, since every call names the vault and item.
//! (Proton's ids are base64url, without dots; colons would clash with
//! Picacho's view ids.)
use super::{Session, block_on};
use crate::manage::forwarding::{
    Alias, AliasDomain, AliasFormat, AliasOptions, AliasStats, ContactStats, DomainKind, NewAlias,
    PassSignInNeeded, ProtonAddressKind, ReverseAlias, ServiceMailbox, random_prefix,
};

/// Proton Pass must be signed in to (again) before Pass can be asked.
fn sign_in_needed() -> anyhow::Error {
    anyhow::Error::new(PassSignInNeeded(t!("sync-proton-aliases-sign-in-to-proton-pass").into()))
}
use anyhow::{Context, Result, anyhow, bail, ensure};
use futures::future::BoxFuture;
use protected_email_bridge::{
    config::PassConfig,
    crypto::KeyRing,
    api::ApiClient,
    gateway::AccountContext,
    pass::{
        self, KeySource, VaultClient, access,
        aliases::{self as palias, AliasContact},
        content::VaultContent,
        items::{self as pitems, ItemRevision, item_flags},
        shares::{self as pshares, Share},
    },
};
use serde_json::Value;
use std::sync::Arc;

/// The Vault Client's errors, with Pass's own refusals said plainly:
/// a sign-in Proton gave no Pass access, Pass's extra password, or its
/// lock. Others as for mail.
trait Passed<T> {
    fn passed(self) -> Result<T>;
}
impl<T> Passed<T> for protected_email_bridge::Result<T> {
    fn passed(self) -> Result<T> {
        self.map_err(|e| {
            let code = protected_email_bridge::mobile::api_code(&e);
            let status = e.api().map(|a| a.status);
            if status == Some(403) && code.is_some_and(|c| (9100..=9106).contains(&c)) {
                let missing = protected_email_bridge::manage::scopes::missing_scopes(&e)
                    .filter(|m| !m.is_empty())
                    .map(|m| m.join(", "))
                    .unwrap_or_else(|| "?".into());
                anyhow!(t!("sync-proton-aliases-this-sign-in-has-no-pass-access", missing = missing))
            } else if matches!(e, protected_email_bridge::Error::Deauthenticated)
                || (status == Some(401) && super::pass_sign_in())
            {
                anyhow::Error::new(PassSignInNeeded(t!("sync-proton-aliases-proton-ended-the-pass-session").into()))
            } else if pass::needs_extra_password(&e) {
                anyhow!(t!("sync-proton-aliases-pass-has-an-extra-password"))
            } else if pass::is_locked(&e) {
                anyhow!(t!("sync-proton-aliases-pass-is-locked"))
            } else {
                super::explain(e)
            }
        })
    }
}

/// The account's keys as the Vault Client asks for them.
struct Keys(Arc<AccountContext>);

impl KeySource for Keys {
    fn user_ring(&self) -> protected_email_bridge::Result<Arc<KeyRing>> {
        Ok(self.0.user_ring())
    }
    /// The user keys held unlocked, by Proton's id: those whose key is in
    /// the unlocked ring.
    fn user_key_ids(&self) -> Vec<String> {
        let held: Vec<String> = self
            .0
            .user_ring()
            .public_keys()
            .iter()
            .map(protected_email_bridge::keyops::fingerprint_lower)
            .collect();
        self.0
            .user()
            .keys
            .iter()
            .filter(|k| {
                protected_email_bridge::crypto::parse_secret_key(k.private_key.as_bytes())
                    .is_ok_and(|s| {
                        held.contains(&protected_email_bridge::keyops::fingerprint_lower(&s.to_public_key()))
                    })
            })
            .map(|k| k.id.clone())
            .collect()
    }
    fn address_ring(&self, address_id: &str) -> protected_email_bridge::Result<Arc<KeyRing>> {
        self.0.keyring(address_id)
    }
    /// The addresses and their keys read again: a vault shared through a
    /// group may need a key just given.
    fn refresh(&self) -> BoxFuture<'_, protected_email_bridge::Result<()>> {
        Box::pin(async move {
            let addresses = self.0.client.get_addresses().await?;
            self.0.set_addresses(addresses);
            self.0.unlock_address_rings();
            Ok(())
        })
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
/// Whether an id Proton gave is safe in a request's path: base64url and
/// numbers. One with `/`, `..` or `?` would make the request another.
fn proton_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='))
}
/// An alias's (share, item) from its id here. One of the account's own
/// addresses has none: what is asked of it is Pass's alone.
fn alias_ids(alias: &Alias) -> Result<(&str, &str)> {
    ensure!(!alias.is_address(), t!("sync-proton-aliases-addresses-dont-have-that", email = alias.email));
    alias
        .id
        .split_once('.')
        .filter(|(s, i)| proton_id(s) && proton_id(i))
        .with_context(|| t!("sync-proton-aliases-not-a-proton-alias", email = alias.email))
}
/// One of the account's own addresses' Proton id, from its id here.
fn address_id(alias: &Alias) -> Result<&str> {
    alias
        .id
        .strip_prefix("addr.")
        .filter(|id| alias.is_address() && proton_id(id))
        .with_context(|| t!("sync-proton-aliases-not-a-proton-alias", email = alias.email))
}
/// One of the account's own addresses as an alias: all but its original
/// address and those being deleted.
fn address_alias(id: &str, email: &str, status: i64, kind: i64, name: &str) -> Option<Alias> {
    use protected_email_bridge::api::types::address_status;
    let kind = ProtonAddressKind::of(kind)?;
    let email = email.trim().to_lowercase();
    (status != address_status::DELETING && !email.is_empty() && proton_id(id)).then(|| Alias {
        id: format!("addr.{id}"),
        email,
        enabled: status == address_status::ENABLED,
        name: name.trim().to_owned(),
        proton_address: Some(kind),
        ..Alias::default()
    })
}
/// A contact's (share, item, contact) from its id here.
fn contact_ids(contact: &ReverseAlias) -> Result<(&str, &str, i64)> {
    let mut parts = contact.id.splitn(3, '.');
    match (parts.next(), parts.next(), parts.next().and_then(|c| c.parse().ok())) {
        (Some(s), Some(i), Some(c)) if proton_id(s) && proton_id(i) => Ok((s, i, c)),
        _ => bail!(t!("sync-proton-aliases-not-a-proton-contact")),
    }
}

// ---- Reading the Vault Client's answers ---------------------------------

/// An alias item, as Pass lists it; `None` for other items, trashed ones
/// and aliases that aren't the account's.
fn alias_of(share: &str, item: &ItemRevision) -> Option<Alias> {
    let email = item.alias_email.as_deref().unwrap_or_default().trim().to_lowercase();
    let owned = item.alias_owner != Some(false);
    (!email.is_empty() && proton_id(&item.item_id) && !item.is_trashed() && owned).then(|| Alias {
        id: format!("{share}.{}", item.item_id),
        email,
        enabled: item.flags & item_flags::ALIAS_DISABLED == 0,
        pinned: item.pinned,
        ..Alias::default()
    })
}
/// Adds what an alias's details say (`AliasDetailsResponse`).
fn add_details(alias: &mut Alias, details: &Value) {
    alias.note = text(&details["Note"]);
    alias.name = text(&details["Name"]);
    alias.mailboxes = details["Mailboxes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| text(&m["ID"]))
        .filter(|id| !id.is_empty())
        .collect();
    let stats = &details["Stats"];
    alias.stats = AliasStats {
        forwarded: stats["ForwardedEmails"].as_u64().unwrap_or(0),
        blocked: stats["BlockedEmails"].as_u64().unwrap_or(0),
        replied: stats["RepliedEmails"].as_u64().unwrap_or(0),
        sent: None,
    };
}
/// A contact, whose counts are of the last 14 days only, so its totals are
/// at least those.
fn contact_of(share: &str, item: &str, c: &AliasContact) -> Option<ReverseAlias> {
    let address = c.reverse_alias.trim().to_lowercase();
    let address = address
        .rsplit_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(inner, _)| inner.trim().to_owned())
        .unwrap_or(address);
    let count = |n: i64| u64::try_from(n).unwrap_or(0);
    address.contains('@').then(|| ReverseAlias {
        id: format!("{share}.{item}.{}", c.id),
        contact: c.email.trim().to_owned(),
        address,
        blocked: c.blocked,
        last_sent: None,
        stats: Some(ContactStats {
            received_recent: count(c.forwarded_emails),
            received: count(c.forwarded_emails),
            sent_recent: count(c.replied_emails),
            sent: count(c.replied_emails),
            partial: true,
        }),
    })
}
/// The domains new aliases can use.
fn options_of(o: &palias::AliasOptions) -> AliasOptions {
    AliasOptions {
        domains: o
            .suffixes
            .iter()
            .filter(|s| !s.suffix.is_empty() && !s.signed_suffix.is_empty())
            .map(|s| AliasDomain {
                suffix: s.suffix.clone(),
                kind: if s.is_custom {
                    DomainKind::Custom
                } else if s.is_premium {
                    DomainKind::Premium
                } else {
                    DomainKind::Shared
                },
                custom_prefix: true,
            })
            .collect(),
        prefix_suggestion: String::new(),
        can_create: o.can_create_alias,
    }
}
/// The suffix offered for `suffix`, with its signature. Shared domains add
/// a random word that changes between requests, so the same domain will do.
fn suffix_for<'a>(o: &'a palias::AliasOptions, suffix: Option<&str>) -> Option<&'a palias::Suffix> {
    let domain = |s: &str| s.rsplit_once('@').map(|(_, d)| d.to_lowercase());
    match suffix {
        None => o.suffixes.first(),
        Some(want) => o
            .suffixes
            .iter()
            .find(|s| s.suffix.eq_ignore_ascii_case(want))
            .or_else(|| o.suffixes.iter().find(|s| domain(&s.suffix) == domain(want))),
    }
    .filter(|s| !s.signed_suffix.is_empty())
}
/// The mailbox for `address` among the options, else the first.
fn mailbox_id(o: &palias::AliasOptions, address: Option<&str>) -> Option<i64> {
    address
        .and_then(|a| o.mailboxes.iter().find(|m| m.email.eq_ignore_ascii_case(a)))
        .or_else(|| o.mailboxes.first())
        .map(|m| m.id)
}
/// The vault aliases are made in: the primary one, else the first the
/// account owns.
fn pick_vault(shares: Vec<Share>) -> Option<Share> {
    let mut owned: Vec<Share> = shares
        .into_iter()
        .filter(|s| s.is_vault() && s.owner && proton_id(&s.share_id))
        .collect();
    let primary = owned.iter().position(|s| s.primary).unwrap_or(0);
    (!owned.is_empty()).then(|| owned.swap_remove(primary))
}

impl Session {
    /// The account's Vault Client, made on first use and kept for the
    /// session: it keeps the vaults it opened. In a build with a Pass
    /// registration, its session is the one signed in for Pass
    /// (`use_pass_session`), which must come first; otherwise the account's
    /// own.
    fn vault_client(&self) -> Result<VaultClient> {
        let mut pass = self.pass.lock().unwrap();
        if let Some(vc) = pass.as_ref() {
            return Ok(vc.clone());
        }
        if super::pass_sign_in() {
            return Err(sign_in_needed());
        }
        let api = self.ctx.client.clone();
        self.check_pass_scope(&api)?;
        let vc = VaultClient::new(api, Arc::new(PassConfig::default()), Arc::new(Keys(self.ctx.clone())));
        *pass = Some(vc.clone());
        Ok(vc)
    }
    /// Uses `saved`, a session signed in as this build's Pass registration
    /// (`Login::begin_for_pass`), for Pass's requests, unless one is in use
    /// already. Proton renews its tokens as it goes; `pass_renewed` hands
    /// the new ones over to be saved.
    pub fn use_pass_session(&self, saved: &super::Saved) -> Result<()> {
        let mut pass = self.pass.lock().unwrap();
        if pass.is_some() {
            return Ok(());
        }
        let config = super::pass_config().with_context(|| t!("sync-proton-this-build-has-no-pass-registration"))?;
        if saved.uid.is_empty() || saved.refresh_token.is_empty() {
            return Err(sign_in_needed());
        }
        let api = ApiClient::new(config).passed()?;
        let slot = self.pass_tokens.clone();
        block_on(async {
            api.set_tokens(saved.tokens()).await;
            api.set_token_listener(Arc::new(move |t: &protected_email_bridge::api::SessionTokens| {
                *slot.lock().unwrap() = Some(t.clone());
            }))
            .await;
        });
        self.check_pass_scope(&api)?;
        *pass = Some(VaultClient::new(api, Arc::new(PassConfig::default()), Arc::new(Keys(self.ctx.clone()))));
        Ok(())
    }
    /// Forgets the Pass session in use and what it opened, before another
    /// is used (signing in to Pass again).
    pub fn reset_pass_session(&self) {
        *self.pass.lock().unwrap() = None;
        *self.pass_vault.lock().unwrap() = None;
        *self.pass_tokens.lock().unwrap() = None;
        self.pass_unlocked.store(false, std::sync::atomic::Ordering::Release);
    }
    /// New tokens of the Pass session, to save, if Proton issued any since
    /// the last call.
    pub fn pass_renewed(&self, saved: &super::Saved) -> Option<super::Saved> {
        let tokens = self.pass_tokens.lock().unwrap().take()?;
        Some(saved.with_tokens(&tokens))
    }
    /// Whether the session Pass's requests use has Pass access, as Proton's
    /// own Pass app checks (`Scopes` includes `pass`). Without it, the
    /// account may protect Pass with an extra password, which grants it
    /// (`unlock_proton_pass`); otherwise Proton gave this registration no
    /// Pass access, which no password changes, so its scopes are named.
    fn check_pass_scope(&self, api: &ApiClient) -> Result<()> {
        use protected_email_bridge::manage::scopes;
        let Ok(have) = block_on(scopes::scopes(api)) else {
            // Unknown: Pass's own answers will say.
            return Ok(());
        };
        if have.iter().any(|s| s == "pass")
            || block_on(access::has_extra_password(api)).unwrap_or(false)
        {
            return Ok(());
        }
        let mut have = have;
        have.sort();
        bail!(t!(
            "sync-proton-aliases-this-registration-has-no-pass-access",
            app = api.config().app_version_header(),
            scopes = if have.is_empty() { "—".to_owned() } else { have.join(", ") }
        ))
    }
    /// Verifies Pass's extra password, which an account can protect Pass
    /// with (§18.3.3): until then Proton refuses Pass's requests (code
    /// 9108). Once per session.
    pub fn unlock_proton_pass(&self, extra_password: &str) -> Result<()> {
        use std::sync::atomic::Ordering;
        if extra_password.is_empty() || self.pass_unlocked.load(Ordering::Acquire) {
            return Ok(());
        }
        let api = self.pass_api()?;
        // A password given for an account without one is left unused, so
        // a field filled in by mistake doesn't stop the aliases.
        if !block_on(access::has_extra_password(&api)).unwrap_or(true) {
            self.pass_unlocked.store(true, Ordering::Release);
            return Ok(());
        }
        match block_on(access::verify_extra_password(&api, extra_password.as_bytes())) {
            Ok(()) => {
                self.pass_unlocked.store(true, Ordering::Release);
                Ok(())
            }
            Err(protected_email_bridge::Error::Auth(_)) => {
                bail!(t!("sync-proton-aliases-the-pass-extra-password-is-wrong"))
            }
            Err(e) => Err(e).passed(),
        }
    }
    /// The API client Pass's requests go through.
    fn pass_api(&self) -> Result<ApiClient> {
        Ok(self.vault_client()?.api)
    }
    /// Registers the account with Pass unless it is already: Pass lists no
    /// vaults for an account it doesn't know.
    fn pass_register(&self) -> Result<()> {
        let api = self.pass_api()?;
        block_on(async {
            if matches!(access::check(&api).await, Ok(None)) {
                let _ = access::register(&api).await;
            }
        });
        Ok(())
    }
    /// The ids of every vault the account can see aliases in.
    fn vault_shares(&self) -> Result<Vec<String>> {
        let shares = block_on(pshares::list(&self.pass_api()?)).passed()?;
        Ok(shares
            .into_iter()
            .filter(|s| s.is_vault() && proton_id(&s.share_id))
            .map(|s| s.share_id)
            .collect())
    }
    /// The vault new aliases go in, if the account has one.
    fn existing_vault(&self) -> Result<Option<Share>> {
        Ok(pick_vault(block_on(pshares::list(&self.pass_api()?)).passed()?))
    }
    /// The vault new aliases go in, opened; made, as Proton's apps do, when
    /// the account has none. Remembered for the session.
    fn alias_vault(&self, vc: &VaultClient) -> Result<String> {
        if let Some(share) = self.pass_vault.lock().unwrap().clone()
            && vc.open_share(&share).is_ok()
        {
            return Ok(share);
        }
        self.pass_register()?;
        let share = match self.existing_vault()? {
            Some(share) => {
                let id = share.share_id.clone();
                block_on(vc.load_share(share))
                    .passed()?
                    .with_context(|| t!("sync-proton-aliases-a-pass-key-wouldnt-open"))?;
                id
            }
            None => {
                let address = self
                    .ctx
                    .primary_address()
                    .with_context(|| t!("sync-proton-this-proton-account-has-no-address"))?;
                let open = block_on(vc.create_vault(&address.id, &VaultContent::default_vault()))
                    .passed()?;
                open.share.share_id
            }
        };
        *self.pass_vault.lock().unwrap() = Some(share.clone());
        Ok(share)
    }

    // ---- Aliases --------------------------------------------------------

    /// Every alias of the account's, in every vault it can see, with its
    /// details where Pass gives them.
    pub fn proton_aliases(&self) -> Result<Vec<Alias>> {
        let api = &self.pass_api()?;
        let mut out = Vec::new();
        for share in self.vault_shares()? {
            let items = block_on(pitems::list(api, &share, true)).passed()?;
            let mut aliases: Vec<Alias> = items.iter().filter_map(|i| alias_of(&share, i)).collect();
            // Details in bulk; an alias is still listed without them.
            let ids: Vec<String> = aliases
                .iter()
                .filter_map(|a| a.id.split_once('.').map(|(_, item)| item.to_owned()))
                .collect();
            if let Ok(details) = block_on(palias::details_bulk(api, &share, &ids)) {
                for d in &details {
                    let email = text(&d["Email"]).trim().to_lowercase();
                    if let Some(a) = aliases.iter_mut().find(|a| a.email == email) {
                        add_details(a, d);
                    }
                }
            }
            out.extend(aliases);
        }
        // Pinned first, as Pass shows them.
        out.sort_by_key(|a| !a.pinned);
        Ok(out)
    }
    /// The real addresses aliases can forward to; ones not yet verified
    /// can't receive aliases, so they are left out.
    pub fn proton_alias_mailboxes(&self) -> Result<Vec<ServiceMailbox>> {
        let mailboxes = block_on(palias::mailboxes(&self.pass_api()?)).passed()?;
        Ok(mailboxes
            .into_iter()
            .filter(|m| m.verified && !m.email.trim().is_empty())
            .map(|m| ServiceMailbox {
                id: m.mailbox_id.to_string(),
                email: m.email.trim().to_lowercase(),
            })
            .collect())
    }
    /// The account's name, and the mailbox aliases forward to by default,
    /// for naming the service here.
    pub fn proton_alias_profile(&self) -> (String, String) {
        let _ = self.pass_register();
        let user = self.ctx.user();
        let name = if user.display_name.is_empty() { user.email.clone() } else { user.display_name.clone() };
        let default = self
            .pass_api()
            .ok()
            .and_then(|api| block_on(palias::mailboxes(&api)).ok())
            .and_then(|list| list.into_iter().find(|m| m.is_default))
            .map(|m| m.email.trim().to_lowercase())
            .unwrap_or_else(|| user.email.to_lowercase());
        (name, default)
    }
    /// The domains new aliases can use. With no vault yet (one is made
    /// with the first alias), only random aliases at the default domain.
    pub fn proton_alias_options(&self) -> Result<AliasOptions> {
        let Some(share) = self.existing_vault()? else {
            return Ok(AliasOptions {
                can_create: true,
                ..AliasOptions::default()
            });
        };
        let o = block_on(palias::options(&self.pass_api()?, &share.share_id)).passed()?;
        Ok(options_of(&o))
    }
    /// A new alias and its Pass item: at a chosen domain and with a chosen
    /// prefix, otherwise random, as the account's settings say.
    pub fn create_proton_alias(&self, new: &NewAlias) -> Result<Alias> {
        let vc = self.vault_client()?;
        let share = self.alias_vault(&vc)?;
        let revision = if new.domain.is_none() && new.prefix.is_empty() && new.format.is_none() {
            let name = if new.note.is_empty() { "Alias" } else { new.note.as_str() };
            block_on(vc.create_random_alias(&share, name, &new.note)).passed()?
        } else {
            // Suffixes are signed for ten minutes, so sign one now.
            let o = block_on(palias::options(&self.pass_api()?, &share)).passed()?;
            ensure!(o.can_create_alias, t!("sync-proton-aliases-proton-wont-make-more-aliases"));
            let suffix = suffix_for(&o, new.domain.as_deref())
                .with_context(|| t!("sync-proton-aliases-proton-no-longer-offers-that-domain"))?;
            let mailbox = mailbox_id(&o, new.mailbox.as_deref())
                .with_context(|| t!("sync-proton-aliases-proton-has-no-mailbox-to-forward"))?;
            let prefix = if new.prefix.is_empty() {
                random_prefix(new.format.unwrap_or(AliasFormat::RandomWords))
            } else {
                new.prefix.clone()
            };
            let alias = palias::NewAlias {
                name: format!("{prefix}{}", suffix.suffix),
                prefix,
                signed_suffix: suffix.signed_suffix.clone(),
                mailbox_ids: vec![mailbox],
                alias_name: None,
                note: new.note.clone(),
            };
            block_on(vc.create_alias(&share, &alias)).passed()?.revision
        };
        let mut alias = alias_of(&share, &revision)
            .with_context(|| t!("sync-proton-aliases-the-new-alias-was-made-but"))?;
        if !new.note.is_empty() {
            // Its description where the alias's details keep it, too.
            if self.set_proton_alias_note(&alias, &new.note).is_ok() {
                alias.note = new.note.clone();
            }
        }
        Ok(alias)
    }
    // ---- The account's own addresses -----------------------------------

    /// The account's own addresses but its original one, as aliases: from
    /// its membership (`GET /core/v4/members?IncludeAddresses=1`, the
    /// account's own entry), else, for an account Proton lists no members
    /// for, its address list. Names come from the address list.
    pub fn proton_addresses(&self) -> Result<Vec<Alias>> {
        use protected_email_bridge::manage::org::members;
        let api = &self.ctx.client;
        let listed = block_on(api.get_addresses()).unwrap_or_else(|_| self.ctx.addresses());
        let name = |id: &str| {
            listed
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.display_name.clone())
                .unwrap_or_default()
        };
        let own = block_on(members::list(api))
            .ok()
            .and_then(|all| all.into_iter().find(|m| m.is_self))
            .filter(|m| !m.addresses.is_empty());
        let mut out: Vec<Alias> = match own {
            Some(me) => me
                .addresses
                .iter()
                .filter_map(|a| address_alias(&a.id, &a.email, a.status, a.kind, &name(&a.id)))
                .collect(),
            None => listed
                .iter()
                .filter_map(|a| address_alias(&a.id, &a.email, a.status, a.kind, &a.display_name))
                .collect(),
        };
        out.sort_by(|a, b| a.email.cmp(&b.email));
        Ok(out)
    }
    /// The address list read again, so sending (which needs an address
    /// turned on) knows of a change made here.
    fn reload_addresses(&self) {
        if let Ok(addresses) = block_on(self.ctx.client.get_addresses()) {
            self.ctx.set_addresses(addresses);
            self.ctx.unlock_address_rings();
        }
    }

    /// Whether the alias still exists, and isn't in Pass's trash; for an
    /// address, whether the account still has it.
    pub fn proton_alias_exists(&self, alias: &Alias) -> Result<bool> {
        use protected_email_bridge::manage::addresses;
        if alias.is_address() {
            let id = address_id(alias)?;
            return match block_on(addresses::get(&self.ctx.client, id)) {
                Ok(a) => Ok(a.status != protected_email_bridge::api::types::address_status::DELETING),
                Err(e) if pass::is_gone(&e) => Ok(false),
                Err(e) => Err(e).passed(),
            };
        }
        let (share, item) = alias_ids(alias)?;
        match block_on(pitems::get(&self.pass_api()?, share, item)) {
            Ok(revision) => Ok(alias_of(share, &revision).is_some()),
            Err(e) if pass::is_gone(&e) => Ok(false),
            Err(e) => Err(e).passed(),
        }
    }
    /// Turns forwarding for an alias on or off; an address is turned on
    /// or off at Proton (off, it neither receives nor sends).
    pub fn set_proton_alias_enabled(&self, alias: &Alias, enabled: bool) -> Result<()> {
        if alias.is_address() {
            let id = address_id(alias)?;
            block_on(protected_email_bridge::manage::addresses::set_enabled(&self.ctx.client, id, enabled)).passed()?;
            self.reload_addresses();
            return Ok(());
        }
        let (share, item) = alias_ids(alias)?;
        block_on(palias::set_enabled(&self.pass_api()?, share, item, enabled)).passed()?;
        Ok(())
    }
    pub fn set_proton_alias_note(&self, alias: &Alias, note: &str) -> Result<()> {
        let (share, item) = alias_ids(alias)?;
        block_on(palias::set_note(&self.pass_api()?, share, item, note)).passed()
    }
    /// The name recipients see on mail from the alias; empty for none.
    pub fn set_proton_alias_name(&self, alias: &Alias, name: &str) -> Result<()> {
        if alias.is_address() {
            let id = address_id(alias)?;
            block_on(protected_email_bridge::mobile::core::update_address(&self.ctx.client, id, Some(name.trim()), None))
                .passed()?;
            self.reload_addresses();
            return Ok(());
        }
        let (share, item) = alias_ids(alias)?;
        block_on(palias::set_name(&self.pass_api()?, share, item, name.trim())).passed()
    }
    /// Pins the alias's item, which Pass lists first, or unpins it.
    pub fn set_proton_alias_pinned(&self, alias: &Alias, pinned: bool) -> Result<()> {
        let (share, item) = alias_ids(alias)?;
        block_on(self.vault_client()?.set_pinned(share, item, pinned)).passed()?;
        Ok(())
    }
    /// Chooses the mailboxes the alias forwards to.
    pub fn set_proton_alias_mailboxes(&self, alias: &Alias, ids: &[String]) -> Result<()> {
        let (share, item) = alias_ids(alias)?;
        ensure!(!ids.is_empty(), t!("sync-forwarding-an-alias-needs-somewhere-to-forward"));
        let ids = ids
            .iter()
            .map(|id| id.parse::<i64>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| anyhow!(t!("sync-proton-aliases-not-a-proton-mailbox")))?;
        block_on(palias::set_mailboxes(&self.pass_api()?, share, item, &ids)).passed()?;
        Ok(())
    }
    /// Deletes the alias and its Pass item for good. Mail sent to it
    /// afterwards is refused.
    pub fn delete_proton_alias(&self, alias: &Alias) -> Result<()> {
        if alias.is_address() {
            // Proton deletes only addresses turned off, and only where the
            // account may delete them.
            use protected_email_bridge::manage::addresses;
            let id = address_id(alias)?;
            let api = &self.ctx.client;
            ensure!(
                block_on(addresses::deletion_allowed(api)).unwrap_or(true),
                t!("sync-proton-aliases-proton-doesnt-let-this-account-delete-addresses")
            );
            if alias.enabled {
                block_on(addresses::set_enabled(api, id, false)).passed()?;
            }
            let deleted = block_on(addresses::delete(api, id)).passed();
            self.reload_addresses();
            return deleted;
        }
        let (share, item) = alias_ids(alias)?;
        let vc = self.vault_client()?;
        block_on(async {
            let revision = pitems::get(&vc.api, share, item).await?.revision;
            vc.delete_items(share, &[(item.to_owned(), revision)], true).await
        })
        .passed()
    }

    // ---- Contacts -------------------------------------------------------

    /// An alias's contacts, each with its reverse alias and how much mail
    /// it had in the last 14 days.
    pub fn proton_alias_contacts(&self, alias: &Alias) -> Result<Vec<ReverseAlias>> {
        let (share, item) = alias_ids(alias)?;
        let contacts = block_on(palias::contacts(&self.pass_api()?, share, item)).passed()?;
        let mut out: Vec<ReverseAlias> = Vec::new();
        for r in contacts.iter().filter_map(|c| contact_of(share, item, c)) {
            if !out.iter().any(|o| o.address == r.address) {
                out.push(r);
            }
        }
        Ok(out)
    }
    /// The contact for writing to `address` as the alias, made unless it
    /// exists.
    pub fn add_proton_alias_contact(&self, alias: &Alias, address: &str) -> Result<ReverseAlias> {
        let (share, item) = alias_ids(alias)?;
        let address = address.trim();
        match block_on(palias::create_contact(&self.pass_api()?, share, item, address, None)) {
            Ok(c) => contact_of(share, item, &c)
                .with_context(|| t!("sync-proton-aliases-proton-didnt-give-a-reverse-alias", contact = address)),
            // Already a contact: Pass refuses to make it again.
            Err(e) => {
                let e = anyhow::Error::from(e);
                self.proton_alias_contacts(alias)?
                    .into_iter()
                    .find(|c| c.contact.eq_ignore_ascii_case(address))
                    .ok_or(e)
            }
        }
    }
    /// Blocks or unblocks mail from a contact; returns whether it is
    /// blocked now.
    pub fn toggle_proton_alias_contact(&self, contact: &ReverseAlias) -> Result<bool> {
        let (share, item, id) = contact_ids(contact)?;
        let c = block_on(palias::set_contact_blocked(&self.pass_api()?, share, item, id, !contact.blocked))
            .passed()?;
        Ok(c.blocked)
    }
    /// Deletes a contact and its reverse alias.
    pub fn delete_proton_alias_contact(&self, contact: &ReverseAlias) -> Result<()> {
        let (share, item, id) = contact_ids(contact)?;
        block_on(palias::delete_contact(&self.pass_api()?, share, item, id)).passed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn revision(v: Value) -> ItemRevision {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn reads_alias_items_and_their_details() {
        let item = revision(json!({
            "ItemID": "abc-_=", "AliasEmail": "Shop.Cat@passinbox.com", "Flags": 4,
            "State": 1, "Pinned": true, "Content": "…"
        }));
        let mut a = alias_of("S1", &item).unwrap();
        assert_eq!(a.id, "S1.abc-_=");
        assert_eq!(a.email, "shop.cat@passinbox.com");
        assert!(!a.enabled, "flag 4 turns it off");
        assert!(a.pinned);
        add_details(&mut a, &json!({
            "Email": "shop.cat@passinbox.com", "Note": "store", "Name": "Jo",
            "Mailboxes": [{"ID": 3, "Email": "me@proton.me"}],
            "Stats": {"ForwardedEmails": 2, "RepliedEmails": 1, "BlockedEmails": 0}
        }));
        assert_eq!((a.note.as_str(), a.name.as_str()), ("store", "Jo"));
        assert_eq!(a.mailboxes, ["3"]);
        assert_eq!(a.stats.summary(), "2 forwarded · 1 replied");
        assert_eq!(alias_ids(&a).unwrap(), ("S1", "abc-_="));

        // Not aliases, trashed, someone else's, or unsafe in a path.
        assert!(alias_of("S1", &revision(json!({"ItemID": "x", "State": 1}))).is_none());
        assert!(alias_of("S1", &revision(json!({"ItemID": "x", "AliasEmail": "a@b.c", "State": 2}))).is_none());
        assert!(alias_of("S1", &revision(json!({"ItemID": "x", "AliasEmail": "a@b.c", "AliasOwner": false}))).is_none());
        assert!(alias_of("S1", &revision(json!({"ItemID": "../x", "AliasEmail": "a@b.c"}))).is_none());
        let bad = Alias { id: "S1.x/y".into(), ..Alias::default() };
        assert!(alias_ids(&bad).is_err());
    }

    #[test]
    fn reads_contacts_and_options() {
        let contact: AliasContact = serde_json::from_value(json!({
            "ID": 9, "Email": "ana@example.org", "Blocked": true,
            "ReverseAlias": "\"ana at example.org\" <RA+Ana@PassInbox.com>",
            "ForwardedEmails": 3, "RepliedEmails": 1
        }))
        .unwrap();
        let c = contact_of("S", "I", &contact).unwrap();
        assert_eq!(c.id, "S.I.9");
        assert_eq!(c.address, "ra+ana@passinbox.com");
        assert!(c.blocked);
        let stats = c.stats.unwrap();
        assert_eq!((stats.received_recent, stats.sent_recent, stats.partial), (3, 1, true));
        assert_eq!(contact_ids(&c).unwrap(), ("S", "I", 9));
        let bad = ReverseAlias { id: "S.I.x".into(), ..ReverseAlias::default() };
        assert!(contact_ids(&bad).is_err());

        let options: palias::AliasOptions = serde_json::from_value(json!({"CanCreateAlias": false, "Suffixes": [
            {"Suffix": "@me.org", "SignedSuffix": "@me.org.X", "IsCustom": true, "IsPremium": false, "Domain": "me.org"},
            {"Suffix": ".cat@passinbox.com", "SignedSuffix": ".cat@passinbox.com.Y", "IsCustom": false, "IsPremium": false, "Domain": "passinbox.com"}
        ], "Mailboxes": [{"ID": 1, "Email": "a@proton.me"}, {"ID": 2, "Email": "b@proton.me"}]}))
        .unwrap();
        let got = options_of(&options);
        assert!(!got.can_create);
        assert_eq!(got.domains[0].kind, DomainKind::Custom);
        assert_eq!(got.domains[1].kind, DomainKind::Shared);
        let signed = |want| suffix_for(&options, want).map(|s| s.signed_suffix.as_str());
        assert_eq!(signed(Some(".dog@passinbox.com")), Some(".cat@passinbox.com.Y"));
        assert_eq!(signed(None), Some("@me.org.X"));
        assert_eq!(mailbox_id(&options, Some("B@proton.me")), Some(2));
        assert_eq!(mailbox_id(&options, None), Some(1));
    }

    #[test]
    fn addresses_but_the_original_are_listed_as_aliases() {
        let custom = address_alias("a1", "Me@Example.org", 1, 3, " Jo ").unwrap();
        assert_eq!(custom.id, "addr.a1");
        assert_eq!(custom.email, "me@example.org");
        assert!(custom.enabled && custom.is_address());
        assert_eq!(custom.proton_address, Some(ProtonAddressKind::Custom));
        assert_eq!(custom.name, "Jo");
        assert_eq!(address_id(&custom).unwrap(), "a1");
        assert!(alias_ids(&custom).is_err(), "an address isn't a Pass item");
        assert!(!address_alias("a2", "x@pm.me", 0, 4, "").unwrap().enabled, "off");
        assert!(address_alias("a3", "me@proton.me", 1, 1, "").is_none(), "the original is left out");
        assert!(address_alias("a4", "old@proton.me", 2, 2, "").is_none(), "being deleted");
        assert!(address_alias("../a5", "x@proton.me", 1, 2, "").is_none(), "unsafe in a path");
        let pass_alias = Alias { id: "addr.a1".into(), ..Alias::default() };
        assert!(address_id(&pass_alias).is_err(), "only addresses have address ids");
    }

    #[test]
    fn aliases_go_in_the_primary_owned_vault() {
        let share = |id: &str, target, owner, primary| Share {
            share_id: id.into(),
            target_type: target,
            owner,
            primary,
            ..Share::default()
        };
        let picked = pick_vault(vec![
            share("item", 2, true, true),
            share("theirs", 1, false, false),
            share("mine", 1, true, false),
            share("main", 1, true, true),
        ]);
        assert_eq!(picked.unwrap().share_id, "main");
        let picked = pick_vault(vec![share("theirs", 1, false, false), share("mine", 1, true, false)]);
        assert_eq!(picked.unwrap().share_id, "mine");
        assert!(pick_vault(vec![]).is_none());
    }
}
