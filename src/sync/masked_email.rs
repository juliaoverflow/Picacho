//! Fastmail's Masked Email: JMAP's `MaskedEmail` objects
//! (<https://www.fastmail.com/dev/maskedemail>), each an address at
//! Fastmail that delivers to the account until it is disabled or deleted.
//!
//! Two ways in: a Fastmail API token with the Masked Email scope (`Token`,
//! which reads the JMAP session itself, so a token without mail access
//! works), or the JMAP session of a Fastmail account already added to
//! Picacho (`sync/jmap.rs`'s `Session`, when it has the capability). Both
//! answer the same method calls; what is asked is the same either way.
//!
//! These helpers only talk to Fastmail; Manage keeps the result.
use crate::manage::forwarding::{Alias, AliasOptions, NewAlias};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::time::Duration;

/// The capability (and the `using` entry) of Masked Email.
pub const CAPABILITY: &str = "https://www.fastmail.com/dev/maskedemail";
/// Where Fastmail's JMAP session is, for API tokens.
pub const FASTMAIL_SESSION: &str = "https://api.fastmail.com/jmap/session";
/// The most read of a session document or a reply.
const LIMIT: u64 = 32 * 1024 * 1024;

/// Something that can make Masked Email method calls: one method, its
/// arguments (without `accountId`, added here), and its response.
pub trait Caller {
    /// The account the masked addresses are kept in.
    fn masked_account(&self) -> Result<String>;
    fn masked_call(&self, method: &str, args: Value) -> Result<Value>;
}

/// A connection made with an API token.
pub struct Token {
    http: reqwest::blocking::Client,
    token: String,
    api_url: String,
    account: String,
    /// The session's `username`: the Fastmail address it belongs to.
    pub username: String,
}

impl Token {
    /// Reads the session at `session_url` (Fastmail's when blank) with
    /// `token`, and checks that it can reach Masked Email.
    pub fn open(session_url: &str, token: &str) -> Result<Self> {
        let token = token.trim();
        ensure!(!token.is_empty(), t!("masked-email-enter-an-api-token"));
        ensure!(!token.contains(char::is_whitespace), t!("masked-email-the-token-has-a-space"));
        let session_url = session_url_of(session_url)?;
        let http = reqwest::blocking::Client::builder()
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let response = http
            .get(&session_url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .with_context(|| t!("masked-email-couldnt-reach-fastmail"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!(t!("masked-email-fastmail-didnt-accept-the-token"));
        }
        let text = crate::sync::public_net::bounded_text(response, LIMIT).unwrap_or_default();
        ensure!(status.is_success(), t!("masked-email-fastmail-answered", status = status.to_string()));
        let session: Value = serde_json::from_str(&text).with_context(|| t!("masked-email-fastmail-sent-a-reply-picacho-cant-read"))?;
        let (api_url, account) = read_session(&session)?;
        Ok(Self {
            http,
            token: token.to_owned(),
            api_url,
            account,
            username: session["username"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// The session resource's address: Fastmail's when blank, else the one
/// given (HTTPS only, but for a server on this computer).
pub fn session_url_of(server: &str) -> Result<String> {
    let server = server.trim().trim_end_matches('/');
    if server.is_empty() {
        return Ok(FASTMAIL_SESSION.to_owned());
    }
    let url = url::Url::parse(server).with_context(|| t!("sync-forwarding-enter-the-servers-address-like-https"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && local),
        t!("sync-forwarding-the-server-must-use-https")
    );
    // A server's root names its session resource the usual way.
    Ok(if url.path().trim_matches('/').is_empty() {
        format!("{server}/jmap/session")
    } else {
        server.to_owned()
    })
}

/// A session document's API address and the account Masked Email uses.
fn read_session(v: &Value) -> Result<(String, String)> {
    ensure!(
        v["capabilities"].get(CAPABILITY).is_some(),
        t!("masked-email-this-token-cant-use-masked-email")
    );
    let account = v["primaryAccounts"][CAPABILITY]
        .as_str()
        .with_context(|| t!("masked-email-this-token-cant-use-masked-email"))?
        .to_owned();
    let api_url = v["apiUrl"].as_str().with_context(|| t!("masked-email-fastmail-sent-a-reply-picacho-cant-read"))?;
    let parsed = url::Url::parse(api_url)?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"));
    ensure!(parsed.scheme() == "https" || local, t!("sync-forwarding-the-server-must-use-https"));
    Ok((api_url.to_owned(), account))
}

impl Caller for Token {
    fn masked_account(&self) -> Result<String> {
        Ok(self.account.clone())
    }
    fn masked_call(&self, method: &str, args: Value) -> Result<Value> {
        let body = json!({
            "using": [crate::sync::jmap::CORE, CAPABILITY],
            "methodCalls": [[method, args, "m"]],
        });
        let response = self
            .http
            .post(&self.api_url)
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .with_context(|| t!("masked-email-couldnt-reach-fastmail"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!(t!("masked-email-fastmail-didnt-accept-the-token"));
        }
        let text = crate::sync::public_net::bounded_text(response, LIMIT).unwrap_or_default();
        ensure!(status.is_success(), t!("masked-email-fastmail-answered", status = status.to_string()));
        let v: Value = serde_json::from_str(&text).with_context(|| t!("masked-email-fastmail-sent-a-reply-picacho-cant-read"))?;
        response_of(&v, method)
    }
}

impl Caller for crate::sync::jmap::Session {
    fn masked_account(&self) -> Result<String> {
        self.masked_email_account()
            .map(str::to_owned)
            .with_context(|| t!("masked-email-this-account-has-no-masked-email"))
    }
    fn masked_call(&self, method: &str, args: Value) -> Result<Value> {
        self.call_one(method, args)
    }
}

/// The response arguments of the call named `method` in a JMAP reply, or
/// the method's error.
fn response_of(v: &Value, method: &str) -> Result<Value> {
    let responses = v["methodResponses"]
        .as_array()
        .with_context(|| t!("masked-email-fastmail-sent-a-reply-picacho-cant-read"))?;
    for r in responses {
        let (name, args) = (r[0].as_str().unwrap_or_default(), &r[1]);
        if name == "error" {
            let kind = args["type"].as_str().unwrap_or("serverFail");
            let detail = args["description"].as_str().map(|d| format!(": {d}")).unwrap_or_default();
            bail!(t!("masked-email-fastmail-refused", kind = kind, detail = detail));
        }
        if name == method {
            return Ok(args.clone());
        }
    }
    bail!(t!("masked-email-fastmail-sent-a-reply-picacho-cant-read"))
}

/// A /set's problem for one object (`notCreated`, `notUpdated`), if any.
fn set_problem(v: &Value, key: &str, id: &str) -> Result<()> {
    if let Some(e) = v[key].get(id) {
        let kind = e["type"].as_str().unwrap_or("invalidProperties");
        let detail = e["description"].as_str().map(|d| format!(": {d}")).unwrap_or_default();
        bail!(t!("masked-email-fastmail-refused", kind = kind, detail = detail));
    }
    Ok(())
}

/// One MaskedEmail object as an alias: its address, whether it delivers
/// (`enabled`; a `pending` one, made but not used yet, does too), and its
/// description, or the site it was made for. Deleted ones are none.
pub fn parse_masked(v: &Value) -> Option<Alias> {
    let id = v["id"].as_str().filter(|s| !s.is_empty())?;
    let email = v["email"].as_str().filter(|s| s.contains('@'))?;
    let state = v["state"].as_str().unwrap_or("enabled");
    if state == "deleted" {
        return None;
    }
    let description = v["description"].as_str().unwrap_or_default().trim();
    let site = v["forDomain"].as_str().unwrap_or_default().trim();
    let note = match (description.is_empty(), site.is_empty()) {
        (false, false) if !description.contains(site) => format!("{description} ({site})"),
        (false, _) => description.to_owned(),
        (true, _) => site.to_owned(),
    };
    Some(Alias {
        id: id.to_owned(),
        email: email.trim().to_lowercase(),
        enabled: matches!(state, "enabled" | "pending"),
        note,
        ..Alias::default()
    })
}

/// Every masked address but deleted ones, most recently made first.
pub fn aliases(c: &dyn Caller) -> Result<Vec<Alias>> {
    let v = c.masked_call("MaskedEmail/get", json!({ "accountId": c.masked_account()?, "ids": null }))?;
    let mut list: Vec<(String, Alias)> = v["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m["createdAt"].as_str().unwrap_or_default().to_owned(), parse_masked(m)?)))
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(list.into_iter().map(|(_, a)| a).collect())
}

/// What a new masked address can be: Fastmail picks the domain and the
/// address; a prefix may begin it.
pub fn options() -> AliasOptions {
    AliasOptions {
        can_create: true,
        ..AliasOptions::default()
    }
}

/// The characters Fastmail takes in a prefix (`emailPrefix`): up to 64 of
/// a–z, 0–9 and `_`.
pub fn clean_prefix(prefix: &str) -> String {
    prefix
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == '.' || c == '-' { '_' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .take(64)
        .collect()
}

/// Makes a masked address, enabled at once, with the note as its
/// description and the prefix (if any) at its start.
pub fn create(c: &dyn Caller, new: &NewAlias) -> Result<Alias> {
    let mut object = json!({ "state": "enabled", "description": new.note.trim() });
    let prefix = clean_prefix(&new.prefix);
    if !prefix.is_empty() {
        object["emailPrefix"] = json!(prefix);
    }
    let v = c.masked_call(
        "MaskedEmail/set",
        json!({ "accountId": c.masked_account()?, "create": { "new": object } }),
    )?;
    set_problem(&v, "notCreated", "new")?;
    let created = &v["created"]["new"];
    // The reply holds the server-set properties; the rest are as sent.
    let mut whole = object.clone();
    if let (Some(into), Some(from)) = (whole.as_object_mut(), created.as_object()) {
        into.extend(from.clone());
    }
    parse_masked(&whole).with_context(|| t!("sync-forwarding-the-new-alias-was-made-but"))
}

/// Changes one masked address's properties.
fn update(c: &dyn Caller, alias: &Alias, patch: Value) -> Result<()> {
    let v = c.masked_call(
        "MaskedEmail/set",
        json!({ "accountId": c.masked_account()?, "update": { alias.id.clone(): patch } }),
    )?;
    set_problem(&v, "notUpdated", &alias.id)
}

/// Turns delivery on or off: mail to a disabled address goes to Trash.
pub fn set_enabled(c: &dyn Caller, alias: &Alias, enabled: bool) -> Result<()> {
    update(c, alias, json!({ "state": if enabled { "enabled" } else { "disabled" } }))
}

pub fn set_note(c: &dyn Caller, alias: &Alias, note: &str) -> Result<()> {
    update(c, alias, json!({ "description": note.trim() }))
}

/// Deletes the address: Fastmail refuses its mail from then on. (Masked
/// Email has no /set destroy; deletion is a state.)
pub fn delete(c: &dyn Caller, alias: &Alias) -> Result<()> {
    update(c, alias, json!({ "state": "deleted" }))
}

/// Whether the address is still there, and not deleted.
pub fn exists(c: &dyn Caller, alias: &Alias) -> Result<bool> {
    let v = c.masked_call(
        "MaskedEmail/get",
        json!({ "accountId": c.masked_account()?, "ids": [alias.id] }),
    )?;
    Ok(v["list"].as_array().into_iter().flatten().any(|m| parse_masked(m).is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake Masked Email account, answering from memory.
    struct Fake(std::sync::Mutex<Vec<Value>>);
    impl Caller for Fake {
        fn masked_account(&self) -> Result<String> {
            Ok("u1".into())
        }
        fn masked_call(&self, method: &str, args: Value) -> Result<Value> {
            assert_eq!(args["accountId"], "u1");
            let mut all = self.0.lock().unwrap();
            match method {
                "MaskedEmail/get" => {
                    let list: Vec<Value> = match args["ids"].as_array() {
                        Some(ids) => all.iter().filter(|m| ids.contains(&m["id"])).cloned().collect(),
                        None => all.clone(),
                    };
                    Ok(json!({ "list": list }))
                }
                "MaskedEmail/set" => {
                    if let Some(new) = args["create"]["new"].as_object() {
                        let prefix = new.get("emailPrefix").and_then(Value::as_str).unwrap_or("x");
                        let made = json!({ "id": "m9", "email": format!("{prefix}.abc12@fastmail.com"), "createdAt": "2026-11-01T00:00:00Z" });
                        let mut whole = Value::Object(new.clone());
                        whole.as_object_mut().unwrap().extend(made.as_object().unwrap().clone());
                        all.push(whole);
                        return Ok(json!({ "created": { "new": made } }));
                    }
                    let mut out = json!({ "updated": {} });
                    for (id, patch) in args["update"].as_object().unwrap() {
                        match all.iter_mut().find(|m| m["id"] == *id) {
                            Some(m) => m.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone()),
                            None => out["notUpdated"][id] = json!({ "type": "notFound" }),
                        }
                    }
                    Ok(out)
                }
                _ => bail!("unexpected {method}"),
            }
        }
    }

    fn fake() -> Fake {
        Fake(std::sync::Mutex::new(vec![
            json!({ "id": "m1", "email": "Old.Fox12@fastmail.com", "state": "enabled", "forDomain": "shop.example", "description": "", "createdAt": "2026-01-01T00:00:00Z" }),
            json!({ "id": "m2", "email": "new.owl34@fastmail.com", "state": "pending", "forDomain": "", "description": "Newsletter", "createdAt": "2026-06-01T00:00:00Z" }),
            json!({ "id": "m3", "email": "gone@fastmail.com", "state": "deleted", "createdAt": "2026-07-01T00:00:00Z" }),
            json!({ "id": "m4", "email": "off@fastmail.com", "state": "disabled", "description": "Bank", "forDomain": "bank.example", "createdAt": "2025-01-01T00:00:00Z" }),
        ]))
    }

    #[test]
    fn lists_masked_addresses_newest_first_without_deleted_ones() {
        let listed = aliases(&fake()).unwrap();
        let emails: Vec<&str> = listed.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(emails, ["new.owl34@fastmail.com", "old.fox12@fastmail.com", "off@fastmail.com"]);
        assert!(listed[0].enabled, "a pending address delivers");
        assert_eq!(listed[1].note, "shop.example");
        assert_eq!(listed[2].note, "Bank (bank.example)");
        assert!(!listed[2].enabled);
    }

    #[test]
    fn makes_changes_and_deletes_masked_addresses() {
        let f = fake();
        let made = create(&f, &NewAlias { prefix: "My-Shop".into(), note: "Shoes".into(), ..NewAlias::default() }).unwrap();
        assert_eq!(made.email, "my_shop.abc12@fastmail.com");
        assert_eq!(made.note, "Shoes");
        assert!(made.enabled);
        set_enabled(&f, &made, false).unwrap();
        assert!(!aliases(&f).unwrap().iter().find(|a| a.id == "m9").unwrap().enabled);
        set_note(&f, &made, "Boots").unwrap();
        assert_eq!(aliases(&f).unwrap().iter().find(|a| a.id == "m9").unwrap().note, "Boots");
        assert!(exists(&f, &made).unwrap());
        delete(&f, &made).unwrap();
        assert!(!exists(&f, &made).unwrap());
        let missing = Alias { id: "nope".into(), ..Alias::default() };
        assert!(set_note(&f, &missing, "x").is_err());
    }

    #[test]
    fn sessions_need_the_capability_and_https() {
        let good = json!({
            "capabilities": { CAPABILITY: {} },
            "primaryAccounts": { CAPABILITY: "u1" },
            "apiUrl": "https://api.fastmail.com/jmap/api/",
        });
        assert_eq!(read_session(&good).unwrap(), ("https://api.fastmail.com/jmap/api/".into(), "u1".into()));
        let mut plain = good.clone();
        plain["apiUrl"] = json!("http://api.fastmail.com/jmap/api/");
        assert!(read_session(&plain).is_err());
        let without = json!({ "capabilities": {}, "apiUrl": "https://api.fastmail.com/jmap/api/" });
        assert!(read_session(&without).is_err());
        assert_eq!(session_url_of("").unwrap(), FASTMAIL_SESSION);
        assert_eq!(session_url_of("https://jmap.example.org/").unwrap(), "https://jmap.example.org/jmap/session");
        assert!(session_url_of("http://jmap.example.org").is_err());
    }

    #[test]
    fn method_errors_are_reported() {
        let reply = json!({ "methodResponses": [["error", { "type": "forbidden", "description": "no" }, "m"]] });
        assert!(response_of(&reply, "MaskedEmail/get").is_err());
        let reply = json!({ "methodResponses": [["MaskedEmail/get", { "list": [] }, "m"]] });
        assert_eq!(response_of(&reply, "MaskedEmail/get").unwrap(), json!({ "list": [] }));
    }

    #[test]
    fn prefixes_keep_only_what_fastmail_takes() {
        assert_eq!(clean_prefix(" My.Shop-2 €"), "my_shop_2");
        assert_eq!(clean_prefix(&"a".repeat(80)).len(), 64);
    }
}
