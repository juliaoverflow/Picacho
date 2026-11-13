//! Alias forwarding services: SimpleLogin, addy.io and Firefox Relay.
//!
//! Each one forwards mail sent to an alias on to a real mailbox, and has a
//! JSON API, authenticated with a key made in its web app, for listing,
//! creating, turning off and deleting aliases. Every service is open source
//! and can be self-hosted, so the server is configurable.
//!
//! These helpers only talk to the service; Manage keeps the result.
use crate::manage::forwarding::{
    Alias, AliasDomain, AliasFormat, AliasOptions, DomainKind, ForwardingService, NewAlias,
    random_prefix,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::time::Duration;

/// Who the key belongs to, as the service names them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// The real address aliases forward to, when the service says.
    pub mailbox: String,
}

/// A connection to one service account.
pub struct Client {
    service: ForwardingService,
    server: String,
    key: String,
    http: reqwest::blocking::Client,
}

/// The server's address without a trailing slash; the default when blank.
/// Only HTTPS is accepted, except for a server on this computer.
pub fn server_url(service: ForwardingService, server: &str) -> Result<String> {
    let server = server.trim().trim_end_matches('/');
    let server = if server.is_empty() {
        service.default_server()
    } else {
        server
    };
    let url = url::Url::parse(server).context("Enter the server's address, like https://…")?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && local),
        "The server must use https://."
    );
    Ok(server.to_owned())
}

impl Client {
    pub fn new(service: ForwardingService, server: &str, key: &str) -> Result<Self> {
        let key = key.trim();
        ensure!(!key.is_empty(), "Enter an API key.");
        ensure!(
            !key.contains(['\r', '\n']),
            "The API key has a line break in it."
        );
        Ok(Self {
            service,
            server: server_url(service, server)?,
            key: key.to_owned(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        let r = self
            .http
            .request(method, format!("{}{path}", self.server))
            .header("Accept", "application/json");
        match self.service {
            ForwardingService::SimpleLogin => r.header("Authentication", &self.key),
            ForwardingService::Addy => r
                .bearer_auth(&self.key)
                .header("X-Requested-With", "XMLHttpRequest"),
            ForwardingService::FirefoxRelay => {
                r.header("Authorization", format!("Token {}", self.key))
            }
        }
    }

    fn send(&self, request: reqwest::blocking::RequestBuilder) -> Result<Value> {
        let response = request
            .send()
            .with_context(|| format!("Could not reach {}", self.service.label()))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!(
                "{} did not accept the API key. Check it and try again.",
                self.service.label()
            );
        }
        if !status.is_success() {
            bail!(
                "{} answered {status}{}",
                self.service.label(),
                error_message(&text)
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default()
            );
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .with_context(|| format!("{} sent a reply Picacho can't read", self.service.label()))
    }

    /// Checks the key and says whose it is.
    pub fn profile(&self) -> Result<Profile> {
        use reqwest::Method;
        Ok(match self.service {
            ForwardingService::SimpleLogin => {
                let v = self.send(self.request(Method::GET, "/api/user_info"))?;
                Profile {
                    name: str_of(&v["name"]),
                    mailbox: str_of(&v["email"]),
                }
            }
            ForwardingService::Addy => {
                let v = self.send(self.request(Method::GET, "/api/v1/account-details"))?;
                Profile {
                    name: str_of(&v["data"]["username"]),
                    mailbox: String::new(),
                }
            }
            ForwardingService::FirefoxRelay => {
                let v = self.send(self.request(Method::GET, "/api/v1/profiles/"))?;
                ensure!(v.is_array(), "Firefox Relay sent an unexpected reply");
                Profile::default()
            }
        })
    }

    /// Every alias on the account.
    pub fn aliases(&self) -> Result<Vec<Alias>> {
        use reqwest::Method;
        let mut out = Vec::new();
        match self.service {
            ForwardingService::SimpleLogin => {
                for page in 0..500 {
                    let v = self.send(
                        self.request(Method::GET, &format!("/api/v2/aliases?page_id={page}")),
                    )?;
                    let batch = parse_simplelogin(&v);
                    if batch.is_empty() {
                        break;
                    }
                    out.extend(batch);
                }
            }
            ForwardingService::Addy => {
                for page in 1..500 {
                    let v = self.send(self.request(
                        Method::GET,
                        &format!("/api/v1/aliases?page[size]=100&page[number]={page}"),
                    ))?;
                    out.extend(parse_addy(&v["data"]));
                    let last = v["meta"]["last_page"].as_u64().unwrap_or(1);
                    if page as u64 >= last {
                        break;
                    }
                }
            }
            ForwardingService::FirefoxRelay => {
                let v = self.send(self.request(Method::GET, "/api/v1/relayaddresses/"))?;
                out.extend(parse_relay(&v));
            }
        }
        Ok(out)
    }

    /// The domains new aliases can use, and whether a prefix can be chosen.
    pub fn options(&self) -> Result<AliasOptions> {
        use reqwest::Method;
        Ok(match self.service {
            ForwardingService::SimpleLogin => {
                parse_simplelogin_options(&self.send(self.request(Method::GET, "/api/v5/alias/options"))?)
            }
            ForwardingService::Addy => {
                let v = self.send(self.request(Method::GET, "/api/v1/domain-options"))?;
                // Which are the person's own; neither is needed to make an alias.
                let custom = self
                    .send(self.request(Method::GET, "/api/v1/domains"))
                    .unwrap_or_default();
                let username = self
                    .send(self.request(Method::GET, "/api/v1/account-details"))
                    .map(|d| str_of(&d["data"]["username"]))
                    .unwrap_or_default();
                parse_addy_options(&v, &custom, &username)
            }
            ForwardingService::FirefoxRelay => AliasOptions {
                can_create: true,
                ..AliasOptions::default()
            },
        })
    }

    /// A new alias: at a chosen domain and with a chosen prefix where the
    /// service allows, otherwise random.
    pub fn create(&self, new: &NewAlias) -> Result<Alias> {
        use reqwest::Method;
        let note = new.note.as_str();
        let alias = match self.service {
            ForwardingService::SimpleLogin if new.domain.is_none() && new.prefix.is_empty() => {
                let mode = match new.format {
                    Some(AliasFormat::RandomWords) => "?mode=word",
                    Some(_) => "?mode=uuid",
                    None => "",
                };
                let v = self.send(
                    self.request(Method::POST, &format!("/api/alias/random/new{mode}"))
                        .json(&json!({ "note": note })),
                )?;
                parse_simplelogin_alias(&v)
            }
            ForwardingService::SimpleLogin => {
                // Suffixes are signed for a short while, so sign one now.
                let options = self.send(self.request(Method::GET, "/api/v5/alias/options"))?;
                ensure!(
                    options["can_create"].as_bool() != Some(false),
                    "SimpleLogin won't make more aliases on this plan."
                );
                let signed = simplelogin_signed_suffix(&options, new.domain.as_deref())
                    .context("SimpleLogin no longer offers that domain. Choose another.")?;
                let prefix = if new.prefix.is_empty() {
                    random_prefix(new.format.unwrap_or(AliasFormat::RandomWords))
                } else {
                    new.prefix.clone()
                };
                let mailboxes = self.send(self.request(Method::GET, "/api/v2/mailboxes"))?;
                let mailbox = simplelogin_mailbox(&mailboxes, new.mailbox.as_deref())
                    .context("SimpleLogin has no mailbox to forward the alias to")?;
                let v = self.send(
                    self.request(Method::POST, "/api/v3/alias/custom/new")
                        .json(&json!({
                            "alias_prefix": prefix,
                            "signed_suffix": signed,
                            "mailbox_ids": [mailbox],
                            "note": note,
                        })),
                )?;
                parse_simplelogin_alias(&v)
            }
            ForwardingService::Addy => {
                // Unless chosen, the account's own default domain and format.
                let details = self.send(self.request(Method::GET, "/api/v1/account-details"))?;
                let data = &details["data"];
                let domain = new
                    .domain
                    .as_deref()
                    .map(|d| d.trim_start_matches('@').to_owned())
                    .or_else(|| Some(str_of(&data["default_alias_domain"])))
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "anonaddy.me".into());
                let mut body = json!({ "domain": domain, "description": note });
                if !new.prefix.is_empty() {
                    body["format"] = json!("custom");
                    body["local_part"] = json!(new.prefix);
                } else if let Some(format) = new.format {
                    body["format"] = json!(format.addy_key());
                } else {
                    let format = str_of(&data["default_alias_format"]);
                    if !format.is_empty() {
                        body["format"] = json!(format);
                    }
                }
                let v = self.send(self.request(Method::POST, "/api/v1/aliases").json(&body))?;
                parse_addy_alias(&v["data"])
            }
            ForwardingService::FirefoxRelay => {
                let v = self.send(
                    self.request(Method::POST, "/api/v1/relayaddresses/")
                        .json(&json!({ "enabled": true, "description": note })),
                )?;
                parse_relay_alias(&v)
            }
        };
        alias.context("The new alias was made, but its address was missing from the reply")
    }

    /// Whether the alias still exists at the service; it can be deleted
    /// from the service's own app or another client.
    pub fn alias_exists(&self, alias: &Alias) -> Result<bool> {
        use reqwest::{Method, StatusCode};
        let id = &alias.id;
        let path = match self.service {
            ForwardingService::SimpleLogin => format!("/api/aliases/{id}"),
            ForwardingService::Addy => format!("/api/v1/aliases/{id}"),
            ForwardingService::FirefoxRelay => format!("/api/v1/relayaddresses/{id}/"),
        };
        let response = self
            .request(Method::GET, &path)
            .send()
            .with_context(|| format!("Could not reach {}", self.service.label()))?;
        let status = response.status();
        // SimpleLogin answers Forbidden for an alias that isn't the key's,
        // which a deleted one no longer is.
        if status == StatusCode::NOT_FOUND
            || (self.service == ForwardingService::SimpleLogin && status == StatusCode::FORBIDDEN)
        {
            return Ok(false);
        }
        ensure!(
            status.is_success(),
            "{} answered {status}",
            self.service.label()
        );
        let v: Value = serde_json::from_str(&response.text().unwrap_or_default()).unwrap_or_default();
        Ok(alias_listed(self.service, &v))
    }

    /// Changes an alias's description.
    pub fn set_note(&self, alias: &Alias, note: &str) -> Result<()> {
        use reqwest::Method;
        let id = &alias.id;
        let (path, body) = match self.service {
            ForwardingService::SimpleLogin => (format!("/api/aliases/{id}"), json!({ "note": note })),
            ForwardingService::Addy => (
                format!("/api/v1/aliases/{id}"),
                json!({ "description": note }),
            ),
            ForwardingService::FirefoxRelay => (
                format!("/api/v1/relayaddresses/{id}/"),
                json!({ "description": note }),
            ),
        };
        self.send(self.request(Method::PATCH, &path).json(&body))?;
        Ok(())
    }

    /// Turns forwarding for an alias on or off.
    pub fn set_enabled(&self, alias: &Alias, enabled: bool) -> Result<()> {
        use reqwest::Method;
        let id = &alias.id;
        match self.service {
            ForwardingService::SimpleLogin => {
                // SimpleLogin only toggles, so ask only when it differs.
                if alias.enabled != enabled {
                    let v = self
                        .send(self.request(Method::POST, &format!("/api/aliases/{id}/toggle")))?;
                    ensure!(
                        v["enabled"].as_bool() == Some(enabled),
                        "SimpleLogin did not change the alias"
                    );
                }
            }
            ForwardingService::Addy => {
                if enabled {
                    self.send(
                        self.request(Method::POST, "/api/v1/active-aliases")
                            .json(&json!({ "id": id })),
                    )?;
                } else {
                    self.send(
                        self.request(Method::DELETE, &format!("/api/v1/active-aliases/{id}")),
                    )?;
                }
            }
            ForwardingService::FirefoxRelay => {
                self.send(
                    self.request(Method::PATCH, &format!("/api/v1/relayaddresses/{id}/"))
                        .json(&json!({ "enabled": enabled })),
                )?;
            }
        }
        Ok(())
    }

    /// SimpleLogin: the reverse aliases of an alias's contacts. Mail sent
    /// to one of these from your mailbox goes out as the alias.
    pub fn reverse_aliases(&self, alias: &Alias) -> Result<std::collections::HashSet<String>> {
        use reqwest::Method;
        ensure!(
            self.service == ForwardingService::SimpleLogin,
            "Only SimpleLogin has reverse aliases"
        );
        let mut out = std::collections::HashSet::new();
        for page in 0..100 {
            let v = self.send(self.request(
                Method::GET,
                &format!("/api/aliases/{}/contacts?page_id={page}", alias.id),
            ))?;
            let contacts = v["contacts"].as_array().cloned().unwrap_or_default();
            if contacts.is_empty() {
                break;
            }
            out.extend(
                contacts
                    .iter()
                    .map(|c| str_of(&c["reverse_alias_address"]).to_lowercase())
                    .filter(|a| !a.is_empty()),
            );
        }
        Ok(out)
    }

    /// SimpleLogin: the reverse alias for writing to `contact` as `alias`,
    /// made if it does not exist yet.
    pub fn reverse_alias(&self, alias: &Alias, contact: &str) -> Result<String> {
        use reqwest::Method;
        ensure!(
            self.service == ForwardingService::SimpleLogin,
            "Only SimpleLogin has reverse aliases"
        );
        let v = self.send(
            self.request(Method::POST, &format!("/api/aliases/{}/contacts", alias.id))
                .json(&json!({ "contact": contact })),
        )?;
        let address = str_of(&v["reverse_alias_address"]);
        ensure!(
            !address.is_empty(),
            "SimpleLogin did not give an address for writing to {contact}"
        );
        Ok(address)
    }

    /// Deletes an alias. Mail sent to it afterwards is refused.
    pub fn delete(&self, alias: &Alias) -> Result<()> {
        use reqwest::Method;
        let id = &alias.id;
        let path = match self.service {
            ForwardingService::SimpleLogin => format!("/api/aliases/{id}"),
            ForwardingService::Addy => format!("/api/v1/aliases/{id}"),
            ForwardingService::FirefoxRelay => format!("/api/v1/relayaddresses/{id}/"),
        };
        self.send(self.request(Method::DELETE, &path))?;
        Ok(())
    }
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
/// The message a JSON error reply carries, if any.
fn error_message(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    ["error", "message", "detail"]
        .iter()
        .map(|k| str_of(&v[k]))
        .find(|m| !m.is_empty())
        .map(|m| m.chars().take(200).collect())
}
fn alias(id: &Value, email: &Value, enabled: Option<bool>, note: &Value) -> Option<Alias> {
    let email = str_of(email).trim().to_lowercase();
    let id = str_of(id);
    (!email.is_empty() && !id.is_empty()).then(|| Alias {
        id,
        email,
        enabled: enabled.unwrap_or(true),
        note: str_of(note),
        display: Default::default(),
    })
}
fn parse_simplelogin_alias(v: &Value) -> Option<Alias> {
    alias(&v["id"], &v["email"], v["enabled"].as_bool(), &v["note"])
}
fn parse_simplelogin(v: &Value) -> Vec<Alias> {
    v["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_simplelogin_alias)
        .collect()
}
/// Whether a service's reply about one alias shows it still in use.
/// addy.io keeps deleted aliases for a while, marked as deleted.
fn alias_listed(service: ForwardingService, v: &Value) -> bool {
    match service {
        ForwardingService::Addy => v["data"]["deleted_at"].is_null(),
        _ => true,
    }
}
fn parse_simplelogin_options(v: &Value) -> AliasOptions {
    let domains = v["suffixes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let suffix = str_of(&s["suffix"]);
            (!suffix.is_empty() && !str_of(&s["signed_suffix"]).is_empty()).then(|| AliasDomain {
                suffix,
                kind: if s["is_custom"].as_bool() == Some(true) {
                    DomainKind::Custom
                } else if s["is_premium"].as_bool() == Some(true) {
                    DomainKind::Premium
                } else {
                    DomainKind::Shared
                },
                custom_prefix: true,
            })
        })
        .collect();
    AliasOptions {
        domains,
        prefix_suggestion: str_of(&v["prefix_suggestion"]),
        can_create: v["can_create"].as_bool().unwrap_or(true),
    }
}
/// The signed form of `suffix`. SimpleLogin's shared domains add a random
/// word that changes between requests, so the same domain will do.
fn simplelogin_signed_suffix(v: &Value, suffix: Option<&str>) -> Option<String> {
    let suffixes = v["suffixes"].as_array()?;
    let domain = |s: &str| s.rsplit_once('@').map(|(_, d)| d.to_lowercase());
    let found = match suffix {
        None => suffixes.first(),
        Some(want) => suffixes
            .iter()
            .find(|s| str_of(&s["suffix"]).eq_ignore_ascii_case(want))
            .or_else(|| {
                suffixes
                    .iter()
                    .find(|s| domain(&str_of(&s["suffix"])) == domain(want))
            }),
    }?;
    Some(str_of(&found["signed_suffix"])).filter(|s| !s.is_empty())
}
/// The SimpleLogin mailbox for `address`, else the default one.
fn simplelogin_mailbox(v: &Value, address: Option<&str>) -> Option<Value> {
    let mailboxes = v["mailboxes"].as_array()?;
    address
        .and_then(|a| {
            mailboxes
                .iter()
                .find(|m| str_of(&m["email"]).eq_ignore_ascii_case(a))
        })
        .or_else(|| mailboxes.iter().find(|m| m["default"].as_bool() == Some(true)))
        .or_else(|| mailboxes.first())
        .map(|m| m["id"].clone())
}
fn parse_addy_options(v: &Value, custom: &Value, username: &str) -> AliasOptions {
    let custom: std::collections::HashSet<String> = custom["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| str_of(&d["domain"]).to_lowercase())
        .collect();
    let default = str_of(&v["defaultAliasDomain"]).to_lowercase();
    let username = username.to_lowercase();
    let mut domains: Vec<AliasDomain> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| str_of(d).to_lowercase())
        .filter(|d| !d.is_empty())
        .map(|d| {
            let kind = if custom.contains(&d) {
                DomainKind::Custom
            } else if !username.is_empty() && d.starts_with(&format!("{username}.")) {
                DomainKind::Username
            } else {
                DomainKind::Shared
            };
            AliasDomain {
                suffix: format!("@{d}"),
                kind,
                // Shared domains only take random aliases.
                custom_prefix: kind != DomainKind::Shared,
            }
        })
        .collect();
    if let Some(i) = domains.iter().position(|d| d.suffix[1..] == default) {
        let d = domains.remove(i);
        domains.insert(0, d);
    }
    AliasOptions {
        domains,
        prefix_suggestion: String::new(),
        can_create: true,
    }
}
fn parse_addy_alias(v: &Value) -> Option<Alias> {
    alias(
        &v["id"],
        &v["email"],
        v["active"].as_bool(),
        &v["description"],
    )
}
fn parse_addy(v: &Value) -> Vec<Alias> {
    v.as_array()
        .into_iter()
        .flatten()
        // Deleted aliases are kept a while, marked; they no longer forward.
        .filter(|a| a["deleted_at"].is_null())
        .filter_map(parse_addy_alias)
        .collect()
}
fn parse_relay_alias(v: &Value) -> Option<Alias> {
    alias(
        &v["id"],
        &v["full_address"],
        v["enabled"].as_bool(),
        &v["description"],
    )
}
fn parse_relay(v: &Value) -> Vec<Alias> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_relay_alias)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_services_aliases() {
        let sl = json!({"aliases": [
            {"id": 12, "email": "Shop.Cat@simplelogin.com", "enabled": false, "note": "store"},
            {"id": 13, "email": "", "enabled": true}
        ]});
        let got = parse_simplelogin(&sl);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "12");
        assert_eq!(got[0].email, "shop.cat@simplelogin.com");
        assert!(!got[0].enabled);
        assert_eq!(got[0].note, "store");

        let addy = json!([
            {"id": "50c9e585-e7f5", "email": "x1@anonaddy.me", "active": true, "description": null},
            {"id": "gone", "email": "x2@anonaddy.me", "active": false, "deleted_at": "2026-01-01 10:00:00"}
        ]);
        let got = parse_addy(&addy);
        assert_eq!(got.len(), 1, "deleted aliases are left out");
        assert!(!alias_listed(ForwardingService::Addy, &json!({"data": addy[1]})));
        assert!(alias_listed(ForwardingService::Addy, &json!({"data": addy[0]})));
        assert_eq!(got[0].id, "50c9e585-e7f5");
        assert!(got[0].enabled);
        assert_eq!(got[0].note, "");

        let relay = json!([{"id": 7, "full_address": "abc@mozmail.com", "enabled": true, "description": "news"}]);
        let got = parse_relay(&relay);
        assert_eq!(got[0].email, "abc@mozmail.com");
        assert_eq!(got[0].note, "news");
    }

    #[test]
    fn reads_the_domains_aliases_can_use() {
        let sl = json!({"can_create": true, "prefix_suggestion": "shop", "suffixes": [
            {"suffix": "@me.org", "signed_suffix": "@me.org.X", "is_custom": true, "is_premium": false},
            {"suffix": ".cat@simplelogin.com", "signed_suffix": ".cat@simplelogin.com.Y", "is_custom": false, "is_premium": false},
            {"suffix": ".dog@slmail.me", "signed_suffix": ".dog@slmail.me.Z", "is_custom": false, "is_premium": true}
        ]});
        let got = parse_simplelogin_options(&sl);
        assert_eq!(got.prefix_suggestion, "shop");
        let kinds: Vec<_> = got.domains.iter().map(|d| d.kind).collect();
        assert_eq!(kinds, [DomainKind::Custom, DomainKind::Shared, DomainKind::Premium]);
        // A shared suffix's word changes between requests; its domain matches.
        assert_eq!(
            simplelogin_signed_suffix(&sl, Some(".bird@slmail.me")).as_deref(),
            Some(".dog@slmail.me.Z")
        );
        assert_eq!(
            simplelogin_signed_suffix(&sl, None).as_deref(),
            Some("@me.org.X")
        );
        let mailboxes = json!({"mailboxes": [
            {"id": 1, "email": "a@x.org", "default": false},
            {"id": 2, "email": "b@x.org", "default": true}
        ]});
        assert_eq!(simplelogin_mailbox(&mailboxes, Some("A@x.org")), Some(json!(1)));
        assert_eq!(simplelogin_mailbox(&mailboxes, None), Some(json!(2)));

        let addy = json!({"data": ["anonaddy.me", "jo.anonaddy.com", "me.org"], "defaultAliasDomain": "me.org"});
        let custom = json!({"data": [{"domain": "me.org"}]});
        let got = parse_addy_options(&addy, &custom, "Jo");
        let listed: Vec<_> = got
            .domains
            .iter()
            .map(|d| (d.suffix.as_str(), d.kind, d.custom_prefix))
            .collect();
        assert_eq!(
            listed,
            [
                ("@me.org", DomainKind::Custom, true),
                ("@anonaddy.me", DomainKind::Shared, false),
                ("@jo.anonaddy.com", DomainKind::Username, true),
            ]
        );
    }

    #[test]
    fn servers_must_be_secure_unless_local() {
        assert_eq!(
            server_url(ForwardingService::SimpleLogin, "").unwrap(),
            "https://app.simplelogin.io"
        );
        assert_eq!(
            server_url(ForwardingService::Addy, "https://mail.example.org/").unwrap(),
            "https://mail.example.org"
        );
        assert!(server_url(ForwardingService::Addy, "http://mail.example.org").is_err());
        assert!(server_url(ForwardingService::Addy, "http://localhost:8000").is_ok());
        assert!(server_url(ForwardingService::Addy, "not a url").is_err());
    }

    #[test]
    fn error_replies_name_the_problem() {
        assert_eq!(
            error_message(r#"{"error":"Alias limit reached"}"#).as_deref(),
            Some("Alias limit reached")
        );
        assert_eq!(error_message("<html>"), None);
    }
}
