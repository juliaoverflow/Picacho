use crate::manage::model::{Account, Provider, Secret, Service};
use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// A value set when the app was built, which the same environment variable
/// overrides at run time (for development against other registrations).
fn setting(name: &str, built: Option<&'static str>) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .or(built.map(Into::into))
        .unwrap_or_default()
}
/// The app's own registration with a provider. Desktop secrets are not
/// confidential (RFC 8252 §8.5); Google still requires one for installed apps.
pub struct Client {
    pub id: String,
    pub secret: String,
}
pub fn client(provider: Provider) -> Client {
    match provider {
        Provider::Gmail => Client {
            id: setting(
                "PICACHO_GOOGLE_CLIENT_ID",
                option_env!("PICACHO_GOOGLE_CLIENT_ID"),
            ),
            secret: setting(
                "PICACHO_GOOGLE_CLIENT_SECRET",
                option_env!("PICACHO_GOOGLE_CLIENT_SECRET"),
            ),
        },
        Provider::Outlook => Client {
            id: setting(
                "PICACHO_MICROSOFT_CLIENT_ID",
                option_env!("PICACHO_MICROSOFT_CLIENT_ID"),
            ),
            secret: String::new(),
        },
        _ => Client {
            id: String::new(),
            secret: String::new(),
        },
    }
}
pub fn configured(provider: Provider) -> bool {
    !client(provider).id.is_empty()
}
/// Fastmail's JMAP server, whose browser sign-in needs the app's own
/// registration with Fastmail.
pub const FASTMAIL_JMAP: &str = "api.fastmail.com";
/// The app's registration with Fastmail, if this build has one.
pub fn fastmail() -> Client {
    Client {
        id: setting(
            "PICACHO_FASTMAIL_CLIENT_ID",
            option_env!("PICACHO_FASTMAIL_CLIENT_ID"),
        ),
        secret: String::new(),
    }
}
/// The app's own registration for a JMAP account's server, if there is
/// one: Fastmail's. Other servers register Picacho when it signs in
/// (RFC 7591), where they allow that.
pub fn jmap_client(account: &Account) -> Client {
    if account.incoming_host.eq_ignore_ascii_case(FASTMAIL_JMAP) {
        fastmail()
    } else {
        Client {
            id: String::new(),
            secret: String::new(),
        }
    }
}

/// A JMAP server's OAuth endpoints (RFC 8414 metadata).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub authorization: String,
    pub token: String,
    pub registration: Option<String>,
    pub scopes: Vec<String>,
    /// Where tokens are given up (RFC 7009), where the server has it.
    pub revocation: Option<String>,
    /// The server's issuer identifier, for the callback's `iss` (RFC 9207).
    pub issuer: Option<String>,
    /// How the token endpoint wants a client with a secret to say so.
    pub client_auth: ClientAuth,
}
/// How a client with a secret authenticates at the token endpoint
/// (RFC 6749 §2.3.1, RFC 8414 §2 `token_endpoint_auth_methods_supported`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClientAuth {
    /// HTTP Basic with the form-encoded id and secret: what RFC 6749
    /// §2.3.1 requires servers to support, and RFC 8414 §2's default.
    #[default]
    Basic,
    /// `client_id` and `client_secret` in the request body, for servers
    /// that list only that (and Google, whose documentation uses it).
    Post,
}
/// Reads a server's metadata document, keeping only https endpoints.
/// Refused: a document whose `jwks_uri` isn't https (RFC 8414 §2), or
/// one listing PKCE methods without S256 (RFC 9700 §2.1.1: "plain"
/// protects nothing against a code seen in transit). One that lists none
/// is used, with S256 sent anyway: many servers that check PKCE omit it.
pub fn parse_metadata(v: &serde_json::Value) -> Result<Metadata> {
    let https = |key: &str| -> Option<String> {
        let u = v[key].as_str()?;
        let parsed = url::Url::parse(u).ok()?;
        (parsed.scheme() == "https" || cfg!(test)).then(|| u.to_owned())
    };
    ensure!(
        v["jwks_uri"].is_null() || https("jwks_uri").is_some(),
        "The server's sign-in keys (jwks_uri) aren't served over https."
    );
    if let Some(methods) = v["code_challenge_methods_supported"].as_array() {
        ensure!(
            methods.iter().any(|m| m.as_str() == Some("S256")),
            "The server's sign-in doesn't support PKCE with S256, which Picacho requires."
        );
    }
    let listed = |key: &str, value: &str| {
        v[key]
            .as_array()
            .is_some_and(|a| a.iter().any(|m| m.as_str() == Some(value)))
    };
    let client_auth = if v["token_endpoint_auth_methods_supported"].is_array()
        && !listed("token_endpoint_auth_methods_supported", "client_secret_basic")
        && listed("token_endpoint_auth_methods_supported", "client_secret_post")
    {
        ClientAuth::Post
    } else {
        ClientAuth::Basic
    };
    Ok(Metadata {
        client_auth,
        authorization: https("authorization_endpoint")
            .context("The server's sign-in settings have no https authorization endpoint.")?,
        token: https("token_endpoint")
            .context("The server's sign-in settings have no https token endpoint.")?,
        registration: https("registration_endpoint"),
        revocation: https("revocation_endpoint"),
        issuer: v["issuer"].as_str().map(str::to_owned),
        scopes: v["scopes_supported"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect(),
    })
}
/// The scopes to ask a JMAP server for: JMAP's own (mail, sending,
/// filters) and a refresh token, among those it lists, or JMAP's when it
/// lists none.
pub fn jmap_scopes(supported: &[String]) -> String {
    use crate::sync::jmap::{CORE, MAIL, SIEVE, SUBMISSION};
    if supported.is_empty() {
        return [CORE, MAIL, SUBMISSION].join(" ");
    }
    [
        CORE,
        MAIL,
        SUBMISSION,
        SIEVE,
        "offline_access",
        "openid",
        "email",
    ]
    .into_iter()
    .filter(|s| supported.iter().any(|x| x == s))
    .collect::<Vec<_>>()
    .join(" ")
}
/// A JMAP server's OAuth metadata, from the server itself
/// (`/.well-known/oauth-authorization-server`, else OpenID Connect's
/// configuration), kept for the session.
pub fn jmap_metadata(host: &str, port: u16) -> Result<Metadata> {
    static KNOWN: Mutex<Vec<(String, Metadata)>> = Mutex::new(Vec::new());
    let origin = if port == 443 || port == 0 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    };
    if let Some((_, m)) = KNOWN.lock().unwrap().iter().find(|(o, _)| *o == origin) {
        return Ok(m.clone());
    }
    let http = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut last = None;
    for path in [
        "/.well-known/oauth-authorization-server",
        "/.well-known/openid-configuration",
    ] {
        let found = http
            .get(format!("{origin}{path}"))
            .header("Accept", "application/json")
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(anyhow::Error::from)
            .and_then(bounded_json);
        // A document that can't be used is passed over for the next one.
        match found.and_then(|v| issued_by(&v, &origin).and_then(|()| parse_metadata(&v)))
        {
            Ok(m) => {
                KNOWN.lock().unwrap().push((origin, m.clone()));
                return Ok(m);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::anyhow!(
        "{host} doesn't offer browser sign-in for mail apps{}. Use a password or an API token instead.",
        last.map(|e| format!(" ({e:#})")).unwrap_or_default()
    ))
}
/// An organization's identity provider's metadata (OpenID Connect
/// Discovery, else RFC 8414), from its issuer: `issuer` +
/// `/.well-known/openid-configuration`, or the RFC 8414 form with the
/// issuer's path after the well-known part. The document must name that
/// same issuer, so a redirect or a lookalike can't send the sign-in, and
/// the tokens, elsewhere. Kept for the session.
pub fn sso_metadata(issuer: &str) -> Result<Metadata> {
    static KNOWN: Mutex<Vec<(String, Metadata)>> = Mutex::new(Vec::new());
    let issuer = issuer.trim().trim_end_matches('/').to_owned();
    if let Some((_, m)) = KNOWN.lock().unwrap().iter().find(|(i, _)| *i == issuer) {
        return Ok(m.clone());
    }
    let parsed = url::Url::parse(&issuer).context("The sign-in service's address isn't valid.")?;
    ensure!(
        parsed.scheme() == "https" || cfg!(test),
        "The sign-in service's address must start with https://."
    );
    let origin = parsed.origin().ascii_serialization();
    let path = parsed.path().trim_end_matches('/');
    let http = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut last = None;
    for url in [
        format!("{issuer}/.well-known/openid-configuration"),
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
    ] {
        let found = http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(anyhow::Error::from)
            .and_then(bounded_json);
        match found.and_then(|v| {
            // OpenID Connect Discovery §4.3: exactly the issuer asked for.
            let named = v["issuer"].as_str().unwrap_or_default().trim_end_matches('/');
            ensure!(
                named == issuer,
                "its sign-in settings are for {}, not {issuer}",
                if named.is_empty() { "no issuer" } else { named }
            );
            parse_metadata(&v)
        }) {
            Ok(m) => {
                KNOWN.lock().unwrap().push((issuer, m.clone()));
                return Ok(m);
            }
            // A document that answered says more than a missing one.
            Err(e) if last.is_none() || e.downcast_ref::<reqwest::Error>().is_none() => {
                if last.as_ref().is_none_or(|l: &anyhow::Error| l.downcast_ref::<reqwest::Error>().is_some()) {
                    last = Some(e);
                }
            }
            Err(_) => {}
        }
    }
    Err(anyhow::anyhow!(
        "{issuer} isn't a sign-in service Picacho can use{}. Check the address with your organization.",
        last.map(|e| format!(" ({e:#})")).unwrap_or_default()
    ))
}
/// What to ask an organization's sign-in for: who you are and a refresh
/// token, plus `extra` (the scopes its mail servers want, from the mail
/// domain's configuration or the account), each only where the provider
/// lists it, when it lists any.
pub fn sso_scopes(supported: &[String], extra: &str) -> String {
    let mut wanted: Vec<&str> = vec!["openid", "email", "profile", "offline_access"];
    wanted.extend(extra.split_whitespace());
    let mut out: Vec<&str> = Vec::new();
    for s in wanted {
        let listed = supported.is_empty() || supported.iter().any(|x| x == s);
        // Scopes the account names were asked for on purpose: always sent.
        let asked = extra.split_whitespace().any(|x| x == s);
        if (listed || asked) && !out.contains(&s) {
            out.push(s);
        }
    }
    out.join(" ")
}

/// Checks that a metadata document fetched from `origin` is that server's
/// own (RFC 8414 §3.3): its `issuer`, which §2 requires, is `origin`. A
/// trailing `/` is allowed, as servers write their origin both ways.
fn issued_by(v: &serde_json::Value, origin: &str) -> Result<()> {
    match v["issuer"].as_str() {
        None => Err(anyhow::anyhow!("its sign-in settings name no issuer")),
        Some(issuer) if issuer.trim_end_matches('/').eq_ignore_ascii_case(origin) => Ok(()),
        Some(issuer) => Err(anyhow::anyhow!(
            "its sign-in settings are for {issuer}, not {origin}"
        )),
    }
}
/// Picacho's registration with a server (RFC 7591 §3.2.1), kept in the
/// keychain for the next sign-in there, so each one doesn't add another
/// client to the server: the client id, the loopback address it was
/// registered for, and where and with what it can be read and changed
/// (RFC 7592 §3), where the server allows that.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Registration {
    client_id: String,
    redirect: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    registration_access_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    registration_client_uri: String,
}
impl Registration {
    /// The keychain entry for registrations at `endpoint`.
    fn key(endpoint: &str) -> String {
        format!("oauth-registration:{endpoint}")
    }
    fn load(endpoint: &str) -> Option<Self> {
        #[cfg(test)]
        let text = TEST_REGISTRATIONS.lock().unwrap().get(endpoint).cloned();
        #[cfg(not(test))]
        let text = crate::manage::store::read_secret_text(&Self::key(endpoint)).ok().flatten();
        serde_json::from_str(&text?).ok()
    }
    /// Kept, best effort: one that can't be is registered again next time.
    fn save(&self, endpoint: &str) {
        let Ok(text) = serde_json::to_string(self) else { return };
        #[cfg(test)]
        {
            let _ = Self::key(endpoint);
            TEST_REGISTRATIONS.lock().unwrap().insert(endpoint.to_owned(), text);
        }
        #[cfg(not(test))]
        let _ = crate::manage::store::write_secret_text(&Self::key(endpoint), &text);
    }
    /// The loopback port it was registered with, to listen on again.
    fn port(&self) -> Option<u16> {
        url::Url::parse(&self.redirect).ok()?.port()
    }
    fn from_response(v: &serde_json::Value, redirect: &str) -> Result<Self> {
        let text = |key: &str| v[key].as_str().unwrap_or_default().to_owned();
        let client_id = text("client_id");
        ensure!(!client_id.is_empty(), "The server registered Picacho without a client id.");
        // RFC 7592 §3: the management URI must be https, as the access
        // token is sent to it.
        let uri = text("registration_client_uri");
        let managed = uri.starts_with("https://") || (cfg!(test) && !uri.is_empty());
        Ok(Self {
            client_id,
            redirect: redirect.to_owned(),
            registration_access_token: if managed { text("registration_access_token") } else { String::new() },
            registration_client_uri: if managed { uri } else { String::new() },
        })
    }
}
#[cfg(test)]
static TEST_REGISTRATIONS: Mutex<std::collections::BTreeMap<String, String>> =
    Mutex::new(std::collections::BTreeMap::new());

/// What Picacho registers as: a native public client, without a secret
/// (RFC 8252 §8.4), for `redirect`.
fn client_metadata(redirect: &str) -> serde_json::Value {
    serde_json::json!({
        "client_name": "Picacho",
        "application_type": "native",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}
/// Registers Picacho with a server that allows apps to (RFC 7591), for
/// `redirect`, and keeps the registration.
fn register(endpoint: &str, redirect: &str) -> Result<String> {
    let v: serde_json::Value = http()?
        .post(endpoint)
        .json(&client_metadata(redirect))
        .send()
        .context("Couldn't reach the server to register Picacho for sign-in.")?
        .error_for_status()
        .context("The server didn't register Picacho for browser sign-in.")?
        .and_then_bounded_json()?;
    let registration = Registration::from_response(&v, redirect)?;
    registration.save(endpoint);
    Ok(registration.client_id)
}
/// The client id to sign in with at a server that registers apps: the one
/// kept from an earlier sign-in where it is still good for `redirect`, else
/// a new registration. A kept one registered for another port (the one it
/// had was taken) is changed to this one where the server lets Picacho
/// (RFC 7592 §2.2); one the server no longer knows (401, 404: §2.1) is
/// registered again.
fn registered(endpoint: &str, redirect: &str) -> Result<String> {
    let Some(kept) = Registration::load(endpoint) else {
        return register(endpoint, redirect);
    };
    if kept.redirect == redirect {
        return Ok(kept.client_id);
    }
    if kept.registration_client_uri.is_empty() {
        return register(endpoint, redirect);
    }
    // §2.2: the whole metadata again, with the client id, and the new
    // redirect in place of the old.
    let mut metadata = client_metadata(redirect);
    metadata["client_id"] = kept.client_id.clone().into();
    let updated = http()?
        .put(&kept.registration_client_uri)
        .bearer_auth(&kept.registration_access_token)
        .json(&metadata)
        .send()
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.error_for_status()?))
        .and_then(bounded_json)
        .and_then(|v| Registration::from_response(&v, redirect));
    match updated {
        Ok(mut registration) if registration.client_id == kept.client_id => {
            // §2.2: the server may issue a new access token; else the old
            // one stays good.
            if registration.registration_access_token.is_empty() {
                registration.registration_access_token = kept.registration_access_token;
                registration.registration_client_uri = kept.registration_client_uri;
            }
            registration.save(endpoint);
            Ok(registration.client_id)
        }
        _ => register(endpoint, redirect),
    }
}
fn pinal_accounts() -> String {
    let origin = setting(
        "PICACHO_PINAL_ACCOUNTS",
        option_env!("PICACHO_PINAL_ACCOUNTS"),
    );
    if origin.is_empty() {
        "https://accounts.pinal.tech".into()
    } else {
        origin.trim_end_matches('/').into()
    }
}
fn pinal_client_id() -> String {
    let id = setting(
        "PICACHO_PINAL_CLIENT_ID",
        option_env!("PICACHO_PINAL_CLIENT_ID"),
    );
    if id.is_empty() {
        "pinal-picacho-production".into()
    } else {
        id
    }
}

fn endpoints(provider: Provider) -> (&'static str, &'static str, &'static str) {
    match provider {
        Provider::Gmail => (
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "openid email profile https://mail.google.com/",
        ),
        _ => (
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "openid email profile offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
        ),
    }
}
/// Gives up a token (RFC 7009 §2.1) at `endpoint`, so it can't be used
/// once the account is removed here: the refresh token where there is one
/// (which also ends its access tokens, §2.1), else the access token. A
/// client with a secret authenticates with it (§2.1: confidential clients
/// must). Best effort: a server that can't is left alone; the token
/// expires there in time.
pub fn revoke(endpoint: &str, client_id: &str, secret: &Secret) -> Result<()> {
    ensure!(
        endpoint.starts_with("https://") || cfg!(test),
        "Tokens are only given up over https."
    );
    let (token, hint) = if !secret.refresh_token.is_empty() {
        (secret.refresh_token.as_str(), "refresh_token")
    } else {
        (secret.access_token.as_str(), "access_token")
    };
    if token.is_empty() {
        return Ok(());
    }
    let mut fields = vec![("token", token), ("token_type_hint", hint)];
    if !client_id.is_empty() {
        fields.push(("client_id", client_id));
    }
    if !secret.client_secret.is_empty() {
        fields.push(("client_secret", &secret.client_secret));
    }
    // §2.2: 200 whether or not the token was still good; §2.2.1's
    // `unsupported_token_type` and the like are errors.
    http()?
        .post(endpoint)
        .form(&fields)
        .timeout(Duration::from_secs(10))
        .send()?
        .error_for_status()?;
    Ok(())
}
/// Where an account's tokens are given up (RFC 7009), if its issuer has
/// such an endpoint: Google's own; a JMAP server's, an organization's
/// identity provider's or Pinal's, from its metadata's
/// `revocation_endpoint` (RFC 8414 §2). Microsoft's identity platform has
/// none (tokens are revoked from the account's own page).
pub fn revocation_endpoint(account: &Account) -> Option<String> {
    if !account.oauth {
        return None;
    }
    if account.service == Service::Pinal {
        return pinal_endpoints(&http().ok()?)
            .ok()?
            .revocation
            .map(|u| u.to_string());
    }
    if let Some(sso) = &account.sso {
        return sso_metadata(&sso.issuer).ok()?.revocation;
    }
    match account.provider {
        Provider::Gmail => Some("https://oauth2.googleapis.com/revoke".into()),
        Provider::Jmap => {
            jmap_metadata(&account.incoming_host, account.incoming_port)
                .ok()?
                .revocation
        }
        _ => None,
    }
}
fn random_url_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
/// A sign-in server's JSON answer, read up to a megabyte (they are a few
/// kilobytes): one answering without end isn't held in memory whole.
fn bounded_json(response: reqwest::blocking::Response) -> Result<serde_json::Value> {
    let text = crate::sync::public_net::bounded_text(response, 1024 * 1024)?;
    Ok(serde_json::from_str(&text)?)
}
/// `bounded_json`, as a step at the end of a request's chain.
trait BoundedJson {
    fn and_then_bounded_json(self) -> Result<serde_json::Value>;
}
impl BoundedJson for reqwest::blocking::Response {
    fn and_then_bounded_json(self) -> Result<serde_json::Value> {
        bounded_json(self)
    }
}
fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(30))
        .redirect(same_origin_redirects())
        .build()?)
}
/// Redirects followed only within the origin first asked (scheme, host and
/// port), a few at most. A 307 or 308 sends a POST's body again: a token
/// request's code, verifier, refresh token or client secret, which another
/// host, or the same one over plain HTTP, must never be sent.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if same && attempt.previous().len() <= 5 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// The Gravatar picture for an address, or `None` when it has none. Only
/// the SHA-256 hash of the address leaves this computer.
pub fn gravatar(email: &str) -> Result<Option<Vec<u8>>> {
    use sha2::Digest;
    use std::io::Read;
    let hash = sha2::Sha256::digest(email.trim().to_lowercase().as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    // d=404 asks for an error rather than a generated placeholder.
    let response = client
        .get(format!("https://gravatar.com/avatar/{hex}?s=256&d=404"))
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(5 << 20)
        .read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// The Google account's profile picture, if it has one. The sign-in's
/// `profile` scope covers the OpenID userinfo endpoint; the image itself is
/// only fetched from Google's own content host.
pub fn google_picture(access_token: &str) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let info: serde_json::Value = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token)
        .send()?
        .error_for_status()?
        .and_then_bounded_json()?;
    let Some(picture) = info["picture"].as_str().filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let mut url = url::Url::parse(picture)?;
    ensure!(
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|h| h.ends_with(".googleusercontent.com")),
        "Unexpected profile picture address"
    );
    // Google sizes pictures with a path suffix such as `=s96-c`.
    if let Some((path, _)) = url.path().rsplit_once('=') {
        let path = format!("{path}=s256-c");
        url.set_path(&path);
    }
    let mut bytes = Vec::new();
    client
        .get(url)
        .send()?
        .error_for_status()?
        .take(5 << 20)
        .read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Tests: what plays the person in the browser, instead of opening one.
#[cfg(test)]
pub(crate) static TEST_BROWSER: Mutex<Option<fn(url::Url)>> = Mutex::new(None);
/// `picacho://` URLs macOS handed the app, waiting for a sign-in to claim them.
static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub fn opened(urls: Vec<String>) {
    // Any web page can ask for `picacho://` addresses: only the latest few,
    // of a callback's size, are kept for a sign-in to claim.
    const KEPT: usize = 16;
    let mut opened = OPENED.lock().unwrap();
    opened.extend(
        urls.into_iter()
            .filter(|u| u.starts_with("picacho:") && u.len() <= 8192),
    );
    let extra = opened.len().saturating_sub(KEPT);
    opened.drain(..extra);
}
/// Running from Picacho.app, where macOS routes `picacho://` to this process.
fn bundled() -> bool {
    cfg!(target_os = "macos")
        && std::env::current_exe()
            .is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
}

/// A pending browser sign-in: the PKCE and state values that tie its
/// callback to this request, and a loopback listener (RFC 8252 §7.3) unless
/// a web page passes the callback on as a `picacho://` URL (§7.1).
struct Browser {
    listener: Option<TcpListener>,
    redirect: String,
    verifier: String,
    challenge: String,
    state: String,
    /// The issuer the callback must name where it names one (RFC 9207
    /// §2.4: a mix-up of servers is refused).
    issuer: Option<String>,
}
impl Browser {
    /// Microsoft registers `http://localhost`; Google and Pinal accept the
    /// loopback address with any port.
    fn new(host: &str, path: &str) -> Result<Self> {
        Self::on_port(host, path, None)
    }
    /// As `new`, listening on `port` where it is free (the one a kept
    /// registration was made for), else on any.
    fn on_port(host: &str, path: &str, port: Option<u16>) -> Result<Self> {
        let listener = match port.and_then(|p| TcpListener::bind(("127.0.0.1", p)).ok()) {
            Some(listener) => listener,
            None => TcpListener::bind("127.0.0.1:0")?,
        };
        listener.set_nonblocking(true)?;
        let redirect = format!("http://{host}:{}{path}", listener.local_addr()?.port());
        Ok(Self::with(Some(listener), redirect))
    }
    /// Pinal's callback page opens `picacho://` with the code.
    fn hosted(redirect: String) -> Self {
        OPENED.lock().unwrap().clear();
        Self::with(None, redirect)
    }
    fn with(listener: Option<TcpListener>, redirect: String) -> Self {
        let verifier = random_url_token();
        Self {
            listener,
            redirect,
            challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            verifier,
            state: random_url_token(),
            issuer: None,
        }
    }
    /// Opens `url` in the system browser and waits for the authorization code.
    #[cfg(not(target_os = "android"))]
    fn code(&self, url: &url::Url, cancel: &AtomicBool) -> Result<String> {
        #[cfg(test)]
        if let Some(visit) = *TEST_BROWSER.lock().unwrap() {
            let url = url.clone();
            let visitor = std::thread::spawn(move || visit(url));
            let code = self.wait(cancel);
            let _ = visitor.join();
            return code;
        }
        open::that(url.as_str()).context("Could not open the system browser")?;
        self.wait(cancel)
    }
    /// On Android, in a Custom Tab over Picacho (or the browser), with the
    /// process kept from being frozen until the browser comes back.
    #[cfg(target_os = "android")]
    fn code(&self, url: &url::Url, cancel: &AtomicBool) -> Result<String> {
        crate::android::open_sign_in(url.as_str())?;
        let code = self.wait(cancel);
        crate::android::sign_in_ended();
        code
    }
    fn wait(&self, cancel: &AtomicBool) -> Result<String> {
        let start = Instant::now();
        loop {
            ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
            ensure!(
                start.elapsed() < Duration::from_secs(300),
                "Sign-in timed out. Please try again."
            );
            {
                // Claim only this request's callback; it answers once.
                let mut opened = OPENED.lock().unwrap();
                let found = opened.iter().enumerate().find_map(|(i, url)| {
                    let answer = url::Url::parse(url).ok().and_then(|u| self.answer(&u));
                    answer.map(|a| (i, a))
                });
                if let Some((i, answer)) = found {
                    opened.remove(i);
                    return answer;
                }
            }
            let Some(listener) = &self.listener else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // A browser that hung up at once: wait for the next one.
                    if stream.set_nonblocking(false).is_err()
                        || stream.set_read_timeout(Some(Duration::from_secs(2))).is_err()
                    {
                        continue;
                    }
                    let mut request = [0u8; 8192];
                    let n = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..n]);
                    let target = request
                        .lines()
                        .next()
                        .and_then(|s| s.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let Ok(callback) = url::Url::parse(&format!("http://localhost{target}")) else {
                        continue;
                    };
                    let answer = self.answer(&callback);
                    let (kind, body) = callback_page(matches!(answer, Some(Ok(_))));
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    if let Some(answer) = answer {
                        return answer;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                // A connection that went away before it was taken, or a
                // signal: neither ends the sign-in (both happen on busy
                // systems, and to tests run side by side).
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    /// The code or error a callback carries, if it answers this request.
    fn answer(&self, callback: &url::Url) -> Option<Result<String>> {
        let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
        if params.get("state") != Some(&self.state) {
            return None;
        }
        // RFC 9207: an `iss` other than the server's own is an answer from
        // another server, mixed in.
        if let (Some(expected), Some(given)) = (&self.issuer, params.get("iss"))
            && given != expected
        {
            return Some(Err(anyhow::anyhow!(
                "The sign-in answered for another server ({given}); nothing was signed in."
            )));
        }
        if let Some(error) = params.get("error") {
            return Some(Err(anyhow::anyhow!(if error == "access_denied" {
                "Sign-in was cancelled.".to_owned()
            } else {
                format!("Sign-in was declined: {error}")
            })));
        }
        params.get("code").cloned().map(Ok)
    }
}

/// The page the browser shows once it has handed back the sign-in: its
/// content type and body.
fn callback_page(ok: bool) -> (&'static str, String) {
    let text = if ok {
        "Sign-in complete. You can return to Picacho."
    } else {
        "Sign-in could not be completed. Return to Picacho and try again."
    };
    #[cfg(target_os = "android")]
    if let Some(back) = crate::android::return_url() {
        // The browser is over Picacho (a Custom Tab) or in front of it:
        // the page goes back to it, and has a button for when the browser
        // won't open apps on its own.
        return (
            "text/html",
            format!(
                "<!doctype html><html><head><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Picacho</title><style>body{{font:16px system-ui,sans-serif;margin:48px 24px;text-align:center}}a{{display:inline-block;margin-top:16px;padding:12px 24px;border-radius:24px;background:#18b965;color:#fff;text-decoration:none}}</style></head><body><p>{text}</p><a href=\"{back}\">Return to Picacho</a><script>location.replace(\"{back}\")</script></body></html>"
            ),
        );
    }
    ("text/plain", text.to_owned())
}

/// Who signed in, from the ID token.
#[derive(Debug, Default, PartialEq)]
pub struct Identity {
    pub email: String,
    pub name: String,
}
/// Reads the ID token's claims. It came straight from the token endpoint
/// over verified TLS, which OpenID Connect Core §3.1.3.7 accepts in place
/// of checking its signature.
#[cfg(test)]
fn identity(id_token: &str) -> Identity {
    identity_of(&claims_of(id_token))
}
/// An ID token's claims (its middle part), or `null`.
fn claims_of(id_token: &str) -> serde_json::Value {
    id_token
        .split('.')
        .nth(1)
        .and_then(|p| {
            URL_SAFE_NO_PAD
                .decode(p.trim_end_matches('='))
                .or_else(|_| URL_SAFE.decode(p))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default()
}
fn identity_of(claims: &serde_json::Value) -> Identity {
    let text = |key: &str| claims[key].as_str().unwrap_or_default().to_owned();
    let email = [text("email"), text("preferred_username"), text("upn")]
        .into_iter()
        .find(|e| e.contains('@'))
        .unwrap_or_default();
    Identity {
        email,
        name: text("name"),
    }
}

/// Who may have issued a sign-in's ID token (its `iss`).
#[derive(Clone, Copy, Debug)]
enum Issuer<'a> {
    /// Exactly this issuer identifier (OpenID Connect Core §3.1.3.7 step
    /// 2): a JMAP server's or an organization's, from its own metadata.
    Exact(&'a str),
    /// Google, which writes its issuer both with and without the scheme
    /// (its OpenID Connect documentation, "Validating an ID token").
    Google,
    /// Microsoft's `common` endpoint, whose metadata names
    /// `https://login.microsoftonline.com/{tenantid}/v2.0`: the token's
    /// own tenant (its `tid` claim) stands for `{tenantid}`.
    Microsoft,
    /// None known (metadata that names no issuer): not checked.
    Unknown,
}
impl Issuer<'_> {
    fn accepts(self, claims: &serde_json::Value) -> bool {
        let Some(iss) = claims["iss"].as_str() else {
            return matches!(self, Self::Unknown);
        };
        match self {
            Self::Exact(expected) => iss == expected,
            Self::Google => matches!(iss, "https://accounts.google.com" | "accounts.google.com"),
            Self::Microsoft => {
                // A tenant is a GUID: one with a `/` would make another
                // issuer's address of the template.
                let tid = claims["tid"].as_str().unwrap_or_default();
                !tid.is_empty()
                    && tid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                    && iss == format!("https://login.microsoftonline.com/{tid}/v2.0")
            }
            Self::Unknown => true,
        }
    }
}
/// How far another clock may be off: OpenID Connect Core §3.1.3.7 step 9
/// allows "some small leeway, usually no more than a few minutes".
const CLOCK_SKEW: i64 = 300;

/// The ID token's claims checked as OpenID Connect Core §3.1.3.7 asks of
/// a token straight from the token endpoint: from the expected issuer
/// (`iss`), issued to this client (`aud`, and `azp` with several
/// audiences or wherever it is given), not expired, with the `nonce` this
/// sign-in sent (where one was), and naming a subject. Returns who signed
/// in and the subject. No ID token at all is no identity, not an error.
fn checked_identity(
    id_token: &str,
    client_id: &str,
    issuer: Issuer<'_>,
    nonce: Option<&str>,
) -> Result<(Identity, String)> {
    checked_identity_at(id_token, client_id, issuer, nonce, chrono::Utc::now().timestamp())
}
fn checked_identity_at(
    id_token: &str,
    client_id: &str,
    issuer: Issuer<'_>,
    nonce: Option<&str>,
    now: i64,
) -> Result<(Identity, String)> {
    if id_token.is_empty() {
        return Ok((Identity::default(), String::new()));
    }
    let claims = claims_of(id_token);
    ensure!(claims.is_object(), "The sign-in's ID token couldn't be read.");
    // Step 2: the issuer must be the one signed in with, so a token from
    // another server (a mix-up) isn't taken for this one's.
    ensure!(
        issuer.accepts(&claims),
        "The sign-in's ID token came from another server ({}).",
        claims["iss"].as_str().unwrap_or("none named")
    );
    let audiences: Vec<&str> = match &claims["aud"] {
        serde_json::Value::String(a) => vec![a.as_str()],
        serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    };
    if !client_id.is_empty() {
        // Step 3: this client among the audiences.
        ensure!(
            audiences.contains(&client_id),
            "The sign-in's ID token was issued to another app."
        );
        // Steps 4 and 5: with several audiences, the one it was given to
        // (`azp`) must be said, and wherever it is said, be this client.
        let azp = claims["azp"].as_str();
        ensure!(
            audiences.len() == 1 || azp.is_some(),
            "The sign-in's ID token doesn't say which of its apps it was issued to."
        );
        ensure!(
            azp.is_none_or(|azp| azp == client_id),
            "The sign-in's ID token was issued to another app."
        );
    }
    // Step 9: `exp` is required, and must not have passed (give or take
    // `CLOCK_SKEW`). `iat` isn't checked: step 10 leaves that to the client,
    // and a clock that runs behind would only refuse good tokens.
    let exp = claims["exp"]
        .as_i64()
        .or_else(|| claims["exp"].as_f64().map(|f| f as i64))
        .context("The sign-in's ID token has no expiry.")?;
    ensure!(
        exp + CLOCK_SKEW >= now,
        "The sign-in's ID token has expired. Check this device's clock."
    );
    if let Some(nonce) = nonce {
        ensure!(
            claims["nonce"].as_str() == Some(nonce),
            "The sign-in's ID token doesn't belong to this sign-in."
        );
    }
    let subject = claims["sub"].as_str().unwrap_or_default().to_owned();
    ensure!(!subject.is_empty(), "The sign-in's ID token names no account.");
    Ok((identity_of(&claims), subject))
}

/// What the browser handed back: an authorization code and the values that
/// redeem it. Kept so a failed token request can be sent again without
/// another trip through the browser while the code is still unused.
pub struct Grant {
    code: String,
    redirect: String,
    verifier: String,
    /// JMAP: the client the server knows Picacho by (registered for this
    /// sign-in, or the app's own), and where the code is redeemed.
    pub client_id: String,
    token: String,
    /// The issuer the ID token must name: the server's own (from its
    /// metadata), or Google's or Microsoft's.
    issuer: Option<String>,
    /// How the server's token endpoint takes a client secret.
    client_auth: ClientAuth,
    /// The scopes asked for, to compare with those granted.
    scope: String,
}
impl Grant {
    fn issuer(&self, provider: Provider, own_endpoints: bool) -> Issuer<'_> {
        match (&self.issuer, provider) {
            (Some(issuer), _) => Issuer::Exact(issuer),
            (None, _) if own_endpoints => Issuer::Unknown,
            (None, Provider::Gmail) => Issuer::Google,
            (None, Provider::Outlook) => Issuer::Microsoft,
            (None, _) => Issuer::Unknown,
        }
    }
}
impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Grant { .. }")
    }
}

/// Signs in to Gmail or Outlook in the browser. The account's email may be
/// empty: the provider's ID token says which account was chosen.
pub fn authorize(account: &Account, cancel: Arc<AtomicBool>) -> Result<Grant> {
    if account.provider == Provider::Jmap {
        return authorize_jmap(account, cancel);
    }
    if let Some(sso) = &account.sso {
        return authorize_sso(account, sso, cancel);
    }
    let microsoft = account.provider == Provider::Outlook;
    let browser = Browser::new(if microsoft { "localhost" } else { "127.0.0.1" }, "/")?;
    let (auth, _, scope) = endpoints(account.provider);
    let mut url = url::Url::parse(auth)?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", account.client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("scope", scope),
        ("state", &browser.state),
        ("code_challenge", &browser.challenge),
        ("code_challenge_method", "S256"),
    ]);
    if !account.email.is_empty() {
        url.query_pairs_mut()
            .append_pair("login_hint", &account.email);
    }
    if account.provider == Provider::Gmail {
        url.query_pairs_mut()
            .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
    } else {
        url.query_pairs_mut()
            .append_pair("prompt", "select_account");
    }
    let code = browser.code(&url, &cancel)?;
    Ok(Grant {
        code,
        redirect: browser.redirect.clone(),
        verifier: browser.verifier.clone(),
        client_id: account.client_id.clone(),
        token: String::new(),
        issuer: None,
        client_auth: ClientAuth::Post,
        scope: scope.to_owned(),
    })
}

/// Signs in to a JMAP server in the browser, with the endpoints its
/// metadata names: as the app's own registration where there is one
/// (Fastmail), else registered for this sign-in.
fn authorize_jmap(account: &Account, cancel: Arc<AtomicBool>) -> Result<Grant> {
    let metadata = jmap_metadata(&account.incoming_host, account.incoming_port)?;
    let own = jmap_client(account);
    let kept = metadata
        .registration
        .as_deref()
        .filter(|_| own.id.is_empty())
        .and_then(Registration::load);
    let mut browser = Browser::on_port("127.0.0.1", "/callback", kept.and_then(|k| k.port()))?;
    browser.issuer = metadata.issuer.clone();
    let client_id = if !own.id.is_empty() {
        own.id
    } else if let Some(endpoint) = &metadata.registration {
        registered(endpoint, &browser.redirect)?
    } else if !account.client_id.is_empty() {
        account.client_id.clone()
    } else {
        bail!(
            "{} doesn't let mail apps sign in in the browser without registering them first. Use a password or an API token instead.",
            account.incoming_host
        );
    };
    let mut url = url::Url::parse(&metadata.authorization)?;
    let scope = jmap_scopes(&metadata.scopes);
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("state", &browser.state),
        ("code_challenge", &browser.challenge),
        ("code_challenge_method", "S256"),
    ]);
    if !scope.is_empty() {
        url.query_pairs_mut().append_pair("scope", &scope);
    }
    if !account.email.is_empty() {
        url.query_pairs_mut()
            .append_pair("login_hint", &account.email);
    }
    let code = browser.code(&url, &cancel)?;
    Ok(Grant {
        code,
        redirect: browser.redirect.clone(),
        verifier: browser.verifier.clone(),
        client_id,
        token: metadata.token,
        issuer: metadata.issuer,
        client_auth: metadata.client_auth,
        scope,
    })
}

/// Signs in with an organization's identity provider in the browser: the
/// client id given (an administrator's registration), else one the
/// provider registers for this sign-in (RFC 7591), where it allows that.
fn authorize_sso(
    account: &Account,
    sso: &crate::manage::model::Sso,
    cancel: Arc<AtomicBool>,
) -> Result<Grant> {
    let metadata = sso_metadata(&sso.issuer)?;
    let scope = sso_scopes(&metadata.scopes, &sso.scope);
    let kept = metadata.registration.as_deref().and_then(Registration::load);
    // A client id that isn't the one Picacho registered is an
    // administrator's registration, used as given.
    let given = !account.client_id.is_empty()
        && kept.as_ref().is_none_or(|k| k.client_id != account.client_id);
    let port = kept.filter(|_| !given).and_then(|k| k.port());
    let mut browser = Browser::on_port("127.0.0.1", "/callback", port)?;
    browser.issuer = metadata.issuer.clone();
    let client_id = if given {
        account.client_id.clone()
    } else if let Some(endpoint) = &metadata.registration {
        registered(endpoint, &browser.redirect)?
    } else {
        bail!(
            "Your organization's sign-in doesn't let mail apps register themselves. Ask your administrator for a client id for Picacho (a public client with a http://127.0.0.1 redirect) and enter it under Server settings."
        );
    };
    let mut url = url::Url::parse(&metadata.authorization)?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("scope", &scope),
        ("state", &browser.state),
        ("code_challenge", &browser.challenge),
        ("code_challenge_method", "S256"),
    ]);
    if !account.email.is_empty() {
        url.query_pairs_mut()
            .append_pair("login_hint", &account.email);
    }
    let code = browser.code(&url, &cancel)?;
    Ok(Grant {
        code,
        redirect: browser.redirect.clone(),
        verifier: browser.verifier.clone(),
        client_id,
        token: metadata.token,
        issuer: metadata.issuer,
        client_auth: metadata.client_auth,
        scope,
    })
}

/// The scopes without which the account's mail can't be reached, among
/// those `requested`: Gmail's mail scope, Outlook's IMAP and SMTP ones,
/// and JMAP's mail capability. Where a token response names its scope
/// (RFC 6749 §3.3: it must when it differs from what was asked), the
/// person may have unticked one (Google's granular consent).
fn needed_scopes<'a>(account: &Account, requested: &'a str) -> Vec<&'a str> {
    let wanted: &[&str] = match account.provider {
        Provider::Gmail if account.sso.is_none() => &["https://mail.google.com/"],
        Provider::Outlook if account.sso.is_none() => &[
            "https://outlook.office.com/IMAP.AccessAsUser.All",
            "https://outlook.office.com/SMTP.Send",
        ],
        Provider::Jmap => &[crate::sync::jmap::MAIL],
        _ => &[],
    };
    requested
        .split_whitespace()
        .filter(|s| wanted.contains(s))
        .collect()
}
/// Whether `granted` (a token response's `scope`) has `scope`. Scope
/// tokens are case-sensitive (RFC 6749 §3.3), but Microsoft returns its
/// resource's scopes in its own spelling of the name, so those compare
/// without case.
fn granted_scope(granted: &str, scope: &str) -> bool {
    granted.split_whitespace().any(|g| {
        g == scope || (scope.starts_with("https://outlook.office.com/") && g.eq_ignore_ascii_case(scope))
    })
}

/// Exchanges the browser's code for tokens and says who signed in. A
/// provider rejection (`AccessFailure::Authentication`) means the code is
/// spent; any other failure may be retried with the same grant.
pub fn redeem(account: &Account, grant: &Grant) -> Result<(Secret, Identity)> {
    // JMAP servers and organizations' sign-ins name their own endpoints,
    // and may have registered Picacho for this sign-in.
    let jmap = account.provider == Provider::Jmap || account.sso.is_some();
    let token = if jmap {
        grant.token.as_str()
    } else {
        endpoints(account.provider).1
    };
    let client_id = if jmap {
        grant.client_id.as_str()
    } else {
        account.client_id.as_str()
    };
    let fields = [
        ("code", grant.code.as_str()),
        ("redirect_uri", &grant.redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &grant.verifier),
    ];
    let client = TokenClient {
        id: client_id,
        secret: &account.client_secret,
        // Google's documentation sends the secret in the body.
        auth: if jmap { grant.client_auth } else { ClientAuth::Post },
    };
    let (mut secret, value) = exchange(token, &fields, &client)?;
    secret.client_secret = account.client_secret.clone();
    if let Some(granted) = value["scope"].as_str() {
        let needed = needed_scopes(account, &grant.scope);
        let missing: Vec<&str> = needed
            .into_iter()
            .filter(|s| !granted_scope(granted, s))
            .collect();
        ensure!(
            missing.is_empty(),
            "The sign-in didn't allow access to mail ({} wasn't granted). Sign in again and allow Picacho access to your mail.",
            missing.join(", ")
        );
    }
    let (who, _) = checked_identity(
        value["id_token"].as_str().unwrap_or_default(),
        client_id,
        grant.issuer(account.provider, jmap),
        None,
    )?;
    Ok((secret, who))
}

/// What Picacho asks Pinal for: who you are, and reading, organizing,
/// writing and sending mail and changing its filters. Pinal issues a token
/// addressed to its mail server, which accepts it over IMAP, SMTP and
/// ManageSieve; `offline_access` keeps the account signed in.
const PINAL_SCOPES: &str = "openid profile email offline_access mail.read mail.labels mail.compose mail.send mail.settings";

/// Pinal's sign-in and token endpoints, from its OpenID discovery document.
/// Cached for the session: every token refresh needs the token endpoint.
#[derive(Clone)]
struct PinalEndpoints {
    authorize: url::Url,
    token: url::Url,
    userinfo: url::Url,
    /// Where its tokens are given up (RFC 7009), where it says.
    revocation: Option<url::Url>,
    /// The issuer its ID tokens name (OpenID Connect Discovery §3).
    issuer: Option<String>,
}
fn pinal_endpoints(http: &reqwest::blocking::Client) -> Result<PinalEndpoints> {
    static FOUND: Mutex<Option<(String, PinalEndpoints)>> = Mutex::new(None);
    let origin = pinal_accounts();
    if let Some((o, found)) = FOUND.lock().unwrap().clone()
        && o == origin
    {
        return Ok(found);
    }
    let mut config = None;
    // The issuer root first, then where Better Auth also publishes it.
    for path in [
        "/.well-known/openid-configuration",
        "/api/auth/.well-known/openid-configuration",
    ] {
        let response = http
            .get(format!("{origin}{path}"))
            .send()
            .context("Could not reach Pinal Accounts. Check your connection.")?;
        if response.status().is_success()
            && let Ok(value) = bounded_json(response)
            && value["authorization_endpoint"].is_string()
        {
            config = Some(value);
            break;
        }
    }
    let config = config.context("Pinal Accounts is not answering sign-in requests right now.")?;
    let endpoint = |key: &str| -> Result<url::Url> {
        let url = url::Url::parse(
            config[key]
                .as_str()
                .context("Pinal Accounts is misconfigured")?,
        )?;
        // Discovery must not send the code or token anywhere but Pinal.
        ensure!(
            url.origin().ascii_serialization() == origin,
            "Pinal Accounts returned an unexpected sign-in address."
        );
        Ok(url)
    };
    let found = PinalEndpoints {
        authorize: endpoint("authorization_endpoint")?,
        token: endpoint("token_endpoint")?,
        userinfo: endpoint("userinfo_endpoint")?,
        // Optional: one elsewhere is left unused rather than refusing the
        // sign-in.
        revocation: endpoint("revocation_endpoint").ok(),
        issuer: config["issuer"].as_str().map(str::to_owned),
    };
    *FOUND.lock().unwrap() = Some((origin, found.clone()));
    Ok(found)
}

/// The domains Pinal Mail gives addresses on.
fn pinal_domains() -> Vec<String> {
    let domains = setting(
        "PICACHO_PINAL_MAIL_DOMAINS",
        option_env!("PICACHO_PINAL_MAIL_DOMAINS"),
    );
    let domains = if domains.is_empty() {
        "sonoran.exchange".to_owned()
    } else {
        domains
    };
    domains
        .split(',')
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}
/// A `host:port` override for one of Pinal Mail's servers (development).
fn pinal_server(name: &str, built: Option<&'static str>) -> Option<(String, u16)> {
    let value = setting(name, built);
    let (host, port) = value.rsplit_once(':')?;
    Some((host.to_owned(), port.parse().ok()?))
}
/// The Pinal Mail account for an address: Pinal's mail servers, signed in
/// to with Pinal tokens.
pub fn pinal_account(email: &str, name: &str) -> Account {
    let mut a = Service::Pinal.account(email).unwrap();
    a.name = if name.is_empty() {
        email.to_owned()
    } else {
        name.to_owned()
    };
    if let Some((host, port)) =
        pinal_server("PICACHO_PINAL_IMAP", option_env!("PICACHO_PINAL_IMAP"))
    {
        a.incoming_host = host;
        a.incoming_port = port;
    }
    if let Some((host, port)) =
        pinal_server("PICACHO_PINAL_SMTP", option_env!("PICACHO_PINAL_SMTP"))
    {
        a.smtp_host = host;
        a.smtp_port = port;
    }
    a.oauth = true;
    a.client_id = pinal_client_id();
    a
}

/// A pending Pinal sign-in: the address to open in the browser, and the
/// callback that answers it.
struct PinalRequest {
    url: url::Url,
    endpoints: PinalEndpoints,
    browser: Browser,
    /// The nonce the ID token must carry back.
    nonce: String,
}
fn pinal_request(hint: &str, hosted: bool) -> Result<PinalRequest> {
    let endpoints = pinal_endpoints(&http()?)?;
    let browser = if hosted {
        Browser::hosted(format!("{}/native/callback", pinal_accounts()))
    } else {
        Browser::new("127.0.0.1", "/callback")?
    };
    let mut url = endpoints.authorize.clone();
    let client_id = pinal_client_id();
    let nonce = random_url_token();
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("scope", PINAL_SCOPES),
        ("state", &browser.state),
        ("nonce", &nonce),
        ("code_challenge", &browser.challenge),
        ("code_challenge_method", "S256"),
    ]);
    if !hint.is_empty() {
        url.query_pairs_mut().append_pair("login_hint", hint);
    }
    Ok(PinalRequest {
        url,
        endpoints,
        browser,
        nonce,
    })
}
/// Turns Pinal's code into tokens and says whose mailbox they open.
fn pinal_redeem(request: &PinalRequest, code: &str) -> Result<(Secret, Identity)> {
    let client_id = pinal_client_id();
    let (secret, value) = exchange(
        request.endpoints.token.as_str(),
        &[
            ("code", code),
            ("redirect_uri", &request.browser.redirect),
            ("grant_type", "authorization_code"),
            ("code_verifier", &request.browser.verifier),
        ],
        &TokenClient {
            id: &client_id,
            secret: "",
            auth: ClientAuth::Basic,
        },
    )?;
    let (mut who, subject) = checked_identity(
        value["id_token"].as_str().unwrap_or_default(),
        &client_id,
        request
            .endpoints
            .issuer
            .as_deref()
            .map_or(Issuer::Unknown, Issuer::Exact),
        Some(&request.nonce),
    )?;
    if who.email.is_empty() {
        // Pinal keeps the ID token small; the address is in its userinfo.
        let info: serde_json::Value = http()?
            .get(request.endpoints.userinfo.clone())
            .bearer_auth(&secret.access_token)
            .send()
            .and_then(|r| r.error_for_status())
            .context("Could not reach Pinal Accounts. Check your connection.")?
            .and_then_bounded_json()?;
        let text = |key: &str| info[key].as_str().unwrap_or_default().to_owned();
        // The same account as the ID token's (OpenID Connect Core §5.3.4).
        ensure!(
            subject.is_empty() || text("sub") == subject,
            "Pinal Accounts answered for another account."
        );
        if info["email_verified"].as_bool() != Some(false) {
            who.email = text("email");
        }
        if who.name.is_empty() {
            who.name = text("name");
        }
    }
    let domain = who
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.to_ascii_lowercase())
        .unwrap_or_default();
    ensure!(
        pinal_domains().contains(&domain),
        "{} has no Pinal Mail address. Pinal Mail is for accounts made with a @{} address.",
        if who.email.is_empty() {
            "This Pinal account"
        } else {
            who.email.as_str()
        },
        pinal_domains()
            .first()
            .map(String::as_str)
            .unwrap_or("sonoran.exchange")
    );
    ensure!(
        !secret.refresh_token.is_empty(),
        "Pinal did not let Picacho stay signed in. Please try again."
    );
    Ok((secret, who))
}
/// Signs in to Pinal Accounts in the browser. The tokens it issues are
/// accepted by Pinal's mail server directly, so no password is kept.
/// `hint` is the address being reconnected, if any; `answered` runs once
/// the browser hands back its code.
pub fn pinal(
    hint: &str,
    cancel: Arc<AtomicBool>,
    answered: &dyn Fn(),
) -> Result<(Secret, Identity)> {
    let request = pinal_request(hint, bundled())?;
    let code = request.browser.code(&request.url, &cancel)?;
    answered();
    let signed_in = pinal_redeem(&request, &code)?;
    ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
    Ok(signed_in)
}

/// The Pinal sign-in with `visit` standing in for the browser: it gets the
/// address that would be opened, on a thread of its own (live tests).
#[cfg(test)]
pub(crate) fn pinal_driven(
    visit: impl FnOnce(url::Url) + Send + 'static,
) -> Result<(Secret, Identity)> {
    let request = pinal_request("", false)?;
    let url = request.url.clone();
    let visitor = std::thread::spawn(move || visit(url));
    let code = request.browser.wait(&AtomicBool::new(false));
    let _ = visitor.join();
    pinal_redeem(&request, &code?)
}

/// Who asks the token endpoint: the client's id, its secret (empty for
/// a public client) and how a secret is given.
struct TokenClient<'a> {
    id: &'a str,
    secret: &'a str,
    auth: ClientAuth,
}
/// A token request: `fields` and the client, which a public client names
/// in the body (RFC 6749 §3.2.1) and a confidential one authenticates as
/// its server asks (§2.3.1): with HTTP Basic, the id and secret each
/// form-encoded first, or both in the body.
fn token_request(
    http: &reqwest::blocking::Client,
    endpoint: &str,
    fields: &[(&str, &str)],
    client: &TokenClient<'_>,
) -> reqwest::blocking::RequestBuilder {
    let mut fields = fields.to_vec();
    let request = http.post(endpoint);
    let request = if client.secret.is_empty() {
        if !client.id.is_empty() {
            fields.push(("client_id", client.id));
        }
        request
    } else if client.auth == ClientAuth::Basic {
        let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        request.basic_auth(encode(client.id), Some(encode(client.secret)))
    } else {
        fields.push(("client_id", client.id));
        fields.push(("client_secret", client.secret));
        request
    };
    request.form(&fields)
}
fn exchange(
    endpoint: &str,
    fields: &[(&str, &str)],
    client: &TokenClient<'_>,
) -> Result<(Secret, serde_json::Value)> {
    let response = token_request(&http()?, endpoint, fields, client).send()?;
    let status = response.status();
    let value: serde_json::Value = bounded_json(response)?;
    // Do not include the whole provider response: it can contain credentials
    // or codes. The error code and Microsoft's AADSTS number are safe to show.
    if !status.is_success() {
        let code = value["error"].as_str().unwrap_or("unknown");
        let aadsts = value["error_codes"][0]
            .as_i64()
            .map(|n| format!(", AADSTS{n}"))
            .unwrap_or_default();
        let error = anyhow::anyhow!(
            "Sign-in failed at the token step ({status}: {code}{aadsts}). Please try again."
        );
        return Err(
            if matches!(
                value["error"].as_str(),
                Some("invalid_grant" | "invalid_client" | "unauthorized_client")
            ) {
                error.context(crate::manage::status::AccessFailure::Authentication)
            } else {
                error
            },
        );
    }
    let access_token = value["access_token"]
        .as_str()
        .context("Provider did not return an access token")?
        .to_string();
    // RFC 6749 §5.1 and §7.1: the token's type is required, and only a
    // bearer token (RFC 6750 §4, compared without case) is what the mail
    // protocols take; another kind (DPoP, MAC) would be sent wrongly.
    let kind = value["token_type"].as_str().unwrap_or_default();
    ensure!(
        kind.eq_ignore_ascii_case("bearer"),
        "The sign-in issued a {} token, which Picacho can't use; it needs a bearer token.",
        if kind.is_empty() { "untyped" } else { kind }
    );
    let secret = Secret {
        access_token,
        refresh_token: value["refresh_token"].as_str().unwrap_or_default().into(),
        expires_at: chrono::Utc::now().timestamp() + value["expires_in"].as_i64().unwrap_or(3600),
        ..Default::default()
    };
    Ok((secret, value))
}
pub fn refresh(account: &Account, secret: &mut Secret) -> Result<bool> {
    if !account.oauth || secret.expires_at > chrono::Utc::now().timestamp() + 120 {
        return Ok(false);
    }
    if secret.refresh_token.is_empty() {
        return Err(
            anyhow::anyhow!("Your sign-in expired. Reconnect the account.")
                .context(crate::manage::status::AccessFailure::Authentication),
        );
    }
    let found_token;
    let (token, scope, auth) = if account.service == Service::Pinal {
        found_token = pinal_endpoints(&http()?)?.token.to_string();
        (found_token.as_str(), "", ClientAuth::Basic)
    } else if account.provider == Provider::Jmap {
        let metadata = jmap_metadata(&account.incoming_host, account.incoming_port)?;
        found_token = metadata.token;
        (found_token.as_str(), "", metadata.client_auth)
    } else if let Some(sso) = &account.sso {
        let metadata = sso_metadata(&sso.issuer)?;
        found_token = metadata.token;
        (found_token.as_str(), "", metadata.client_auth)
    } else {
        let (_, token, scope) = endpoints(account.provider);
        (token, scope, ClientAuth::Post)
    };
    let mut fields = vec![
        ("refresh_token", secret.refresh_token.as_str()),
        ("grant_type", "refresh_token"),
    ];
    if account.provider == Provider::Outlook {
        fields.push(("scope", scope));
    }
    let client = TokenClient {
        id: &account.client_id,
        secret: &secret.client_secret,
        auth,
    };
    let (next, _) = exchange(token, &fields, &client)?;
    secret.access_token = next.access_token;
    secret.expires_at = next.expires_at;
    if !next.refresh_token.is_empty() {
        secret.refresh_token = next.refresh_token;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jmap_metadata_must_be_the_servers_own() {
        let origin = "https://api.fastmail.com";
        let own = serde_json::json!({"issuer": "https://api.fastmail.com/"});
        assert!(issued_by(&own, origin).is_ok());
        // RFC 8414 §3.3: an issuer is required, and must be the server's.
        assert!(issued_by(&serde_json::json!({}), origin).is_err());
        let other = serde_json::json!({"issuer": "https://login.example.net"});
        assert!(issued_by(&other, origin).is_err());
    }
    #[test]
    fn jmap_servers_name_their_endpoints_and_scopes() {
        let m = parse_metadata(&serde_json::json!({
            "issuer": "https://mail.example.com",
            "authorization_endpoint": "https://mail.example.com/authorize",
            "token_endpoint": "https://mail.example.com/token",
            "registration_endpoint": "https://mail.example.com/register",
            "scopes_supported": ["offline_access", "urn:ietf:params:jmap:mail", "other"],
        }))
        .unwrap();
        assert_eq!(m.token, "https://mail.example.com/token");
        assert_eq!(
            m.registration.as_deref(),
            Some("https://mail.example.com/register")
        );
        assert_eq!(
            jmap_scopes(&m.scopes),
            "urn:ietf:params:jmap:mail offline_access"
        );
        // None listed: JMAP's own.
        assert_eq!(
            jmap_scopes(&[]),
            "urn:ietf:params:jmap:core urn:ietf:params:jmap:mail urn:ietf:params:jmap:submission"
        );
        assert!(parse_metadata(&serde_json::json!({ "token_endpoint": "x" })).is_err());
        // Fastmail's server uses the app's own registration.
        let mut a = Account::new(Provider::Jmap);
        a.incoming_host = "API.fastmail.com".into();
        assert_eq!(jmap_client(&a).id, fastmail().id);
        a.incoming_host = "mail.example.com".into();
        assert!(jmap_client(&a).id.is_empty());
    }
    /// A small HTTP server answering `respond(method, path, body)` for each
    /// of `count` requests. It gives up after 30 seconds without one, so a
    /// test expecting more requests than are made fails instead of hanging.
    fn http_server(
        count: usize,
        respond: impl Fn(&str, &str, &str) -> (u16, String) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut deadline = Instant::now() + Duration::from_secs(30);
            let incoming = std::iter::from_fn(|| loop {
                match listener.accept() {
                    Ok((s, _)) => {
                        s.set_nonblocking(false).unwrap();
                        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                        deadline = Instant::now() + Duration::from_secs(30);
                        return Some(s);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "the test made fewer requests than expected");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            });
            for mut s in incoming.take(count) {
                let mut buf = vec![0; 16384];
                let n = s.read(&mut buf).unwrap();
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                let mut first = text.lines().next().unwrap_or("").split(' ');
                let (method, path) = (first.next().unwrap_or("").to_owned(), first.next().unwrap_or("").to_owned());
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
                let (status, reply) = respond(&method, &path, &body);
                write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
                seen.push(format!("{method} {path} {body}"));
            }
            seen
        });
        (origin, handle)
    }

    #[test]
    fn sso_metadata_comes_from_the_issuer_itself() {
        // A document naming another issuer is passed over at every
        // discovery location, and the sign-in refused.
        let (origin, server) = http_server(3, |_, path, _| {
            if path == "/realms/mail/.well-known/openid-configuration" {
                (200, r#"{"issuer":"https://evil.example","authorization_endpoint":"https://evil.example/a","token_endpoint":"https://evil.example/t"}"#.into())
            } else {
                (404, "{}".into())
            }
        });
        let error = sso_metadata(&format!("{origin}/realms/mail")).unwrap_err();
        assert!(format!("{error:#}").contains("evil.example"), "{error:#}");
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 3, "every discovery location was tried: {seen:?}");
        assert!(seen[1].starts_with("GET /.well-known/oauth-authorization-server/realms/mail"));
        // RFC 8414's form (the path after the well-known part) is found,
        // and its endpoints and registration kept.
        let doc: Arc<Mutex<String>> = Arc::default();
        let served = doc.clone();
        let (origin, server) = http_server(2, move |_, path, _| {
            if path == "/.well-known/oauth-authorization-server/tenant" {
                (200, served.lock().unwrap().clone())
            } else {
                (404, "{}".to_owned())
            }
        });
        *doc.lock().unwrap() = format!(
            r#"{{"issuer":"{origin}/tenant","authorization_endpoint":"{origin}/auth","token_endpoint":"{origin}/token","registration_endpoint":"{origin}/register","scopes_supported":["openid","offline_access"]}}"#
        );
        let m = sso_metadata(&format!("{origin}/tenant/")).unwrap();
        assert_eq!(m.token, format!("{origin}/token"));
        assert_eq!(m.registration, Some(format!("{origin}/register")));
        server.join().unwrap();
        // Kept for the session: no second fetch.
        assert_eq!(sso_metadata(&format!("{origin}/tenant")).unwrap(), m);
    }

    #[test]
    fn sso_registers_picacho_where_allowed_and_asks_for_what_mail_needs() {
        assert_eq!(
            sso_scopes(&[], "mail.imap"),
            "openid email profile offline_access mail.imap"
        );
        // Only listed ones, but always those the configuration names.
        let listed: Vec<String> = ["openid", "email", "offline_access"].map(String::from).into();
        assert_eq!(sso_scopes(&listed, "https://mail.example.edu/imap"), "openid email offline_access https://mail.example.edu/imap");
        // Registration: a public native client for the loopback redirect.
        let (origin, server) = http_server(1, |method, path, body| {
            assert_eq!((method, path), ("POST", "/register"));
            let v: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(v["token_endpoint_auth_method"], "none");
            assert_eq!(v["application_type"], "native");
            assert_eq!(v["redirect_uris"][0], "http://127.0.0.1:5555/callback");
            (201, r#"{"client_id":"registered-1"}"#.into())
        });
        let id = register(&format!("{origin}/register"), "http://127.0.0.1:5555/callback").unwrap();
        assert_eq!(id, "registered-1");
        server.join().unwrap();
    }

    /// RFC 7009: removed accounts' tokens are given up wherever the issuer
    /// says it can (Google; JMAP and organizations' issuers by metadata),
    /// and not where it has no endpoint (Microsoft, metadata without one).
    #[test]
    fn tokens_are_revoked_where_the_issuer_advertises_it() {
        let mut gmail = Account::new(Provider::Gmail);
        gmail.oauth = true;
        assert_eq!(revocation_endpoint(&gmail).as_deref(), Some("https://oauth2.googleapis.com/revoke"));
        let mut outlook = Account::new(Provider::Outlook);
        outlook.oauth = true;
        assert_eq!(revocation_endpoint(&outlook), None);
        let mut password = Account::new(Provider::Gmail);
        password.oauth = false;
        assert_eq!(revocation_endpoint(&password), None);
        // An organization's issuer whose metadata names an endpoint, and
        // one whose doesn't.
        // The documents name the server's own origin, known once it listens.
        let documents: Arc<Mutex<String>> = Arc::default();
        let named = documents.clone();
        let (origin, metadata) = http_server(2, move |_, path, _| {
            let documents = named.lock().unwrap().clone();
            let issuer = path.strip_suffix("/.well-known/openid-configuration").unwrap_or_default();
            let revocation = if issuer == "/with" {
                format!(r#","revocation_endpoint":"{documents}/with/revoke""#)
            } else {
                String::new()
            };
            (200, format!(r#"{{"issuer":"{documents}{issuer}","authorization_endpoint":"{documents}/a","token_endpoint":"{documents}/t"{revocation}}}"#))
        });
        *documents.lock().unwrap() = origin.clone();
        let mut sso = Account::new(Provider::Imap);
        sso.oauth = true;
        sso.sso = Some(crate::manage::model::Sso { issuer: format!("{origin}/with"), scope: String::new() });
        assert_eq!(revocation_endpoint(&sso), Some(format!("{origin}/with/revoke")));
        sso.sso = Some(crate::manage::model::Sso { issuer: format!("{origin}/without"), scope: String::new() });
        assert_eq!(revocation_endpoint(&sso), None);
        metadata.join().unwrap();
        // The request (RFC 7009 §2.1): the refresh token, hinted, with the
        // client's id and, for a confidential client, its secret.
        let (origin, server) = http_server(2, |method, path, _| {
            assert_eq!((method, path), ("POST", "/revoke"));
            (200, String::new())
        });
        let secret = Secret {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            client_secret: "cs".into(),
            ..Default::default()
        };
        revoke(&format!("{origin}/revoke"), "picacho", &secret).unwrap();
        let access_only = Secret {
            access_token: "at".into(),
            ..Default::default()
        };
        revoke(&format!("{origin}/revoke"), "picacho", &access_only).unwrap();
        let seen = server.join().unwrap();
        assert!(seen[0].ends_with("token=rt&token_type_hint=refresh_token&client_id=picacho&client_secret=cs"), "{seen:?}");
        assert!(seen[1].ends_with("token=at&token_type_hint=access_token&client_id=picacho"), "{seen:?}");
        // A refusal (§2.2.1) is an error for the caller to ignore or show.
        let (origin, server) = http_server(1, |_, _, _| (400, r#"{"error":"unsupported_token_type"}"#.into()));
        assert!(revoke(&format!("{origin}/revoke"), "picacho", &secret).is_err());
        server.join().unwrap();
    }

    /// RFC 6749 §2.3.1: a secret goes in HTTP Basic, its parts form-encoded,
    /// unless the server lists only `client_secret_post`; a public client
    /// names itself in the body.
    #[test]
    fn token_requests_authenticate_the_client_as_the_server_asks() {
        let http = reqwest::blocking::Client::new();
        let fields = [("grant_type", "refresh_token"), ("refresh_token", "rt")];
        let build = |client: TokenClient<'_>| {
            let r = token_request(&http, "https://id.example/token", &fields, &client).build().unwrap();
            let auth = r.headers().get("authorization").map(|v| v.to_str().unwrap().to_owned());
            let body = String::from_utf8(r.body().unwrap().as_bytes().unwrap().to_vec()).unwrap();
            (auth, body)
        };
        let (auth, body) = build(TokenClient { id: "app:1", secret: "sécret/+", auth: ClientAuth::Basic });
        let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("app%3A1:s%C3%A9cret%2F%2B"));
        assert_eq!(auth.as_deref(), Some(expected.as_str()));
        assert_eq!(body, "grant_type=refresh_token&refresh_token=rt");
        let (auth, body) = build(TokenClient { id: "app", secret: "s", auth: ClientAuth::Post });
        assert_eq!(auth, None);
        assert_eq!(body, "grant_type=refresh_token&refresh_token=rt&client_id=app&client_secret=s");
        let (auth, body) = build(TokenClient { id: "app", secret: "", auth: ClientAuth::Basic });
        assert_eq!(auth, None);
        assert_eq!(body, "grant_type=refresh_token&refresh_token=rt&client_id=app");
        // The method comes from the server's metadata.
        let doc = |methods: serde_json::Value| {
            parse_metadata(&serde_json::json!({
                "issuer": "https://id.example",
                "authorization_endpoint": "https://id.example/a",
                "token_endpoint": "https://id.example/t",
                "token_endpoint_auth_methods_supported": methods,
            }))
            .unwrap()
            .client_auth
        };
        assert_eq!(doc(serde_json::json!(["client_secret_post"])), ClientAuth::Post);
        assert_eq!(doc(serde_json::json!(["client_secret_post", "client_secret_basic"])), ClientAuth::Basic);
        assert_eq!(doc(serde_json::Value::Null), ClientAuth::Basic);
    }

    /// RFC 6749 §5.1: only bearer tokens are taken; §3.3: a granted scope
    /// without mail access is refused at sign-in.
    #[test]
    fn token_type_and_granted_scope_are_checked() {
        let (origin, server) = http_server(1, |_, _, _| {
            (200, r#"{"access_token":"a","token_type":"Bearer","expires_in":60}"#.into())
        });
        let client = TokenClient { id: "app", secret: "", auth: ClientAuth::Basic };
        assert!(exchange(&format!("{origin}/t"), &[], &client).is_ok());
        server.join().unwrap();
        let (origin, server) = http_server(1, |_, _, _| {
            (200, r#"{"access_token":"a","token_type":"DPoP"}"#.into())
        });
        let error = exchange(&format!("{origin}/t"), &[], &client).unwrap_err();
        assert!(format!("{error:#}").contains("DPoP"), "{error:#}");
        server.join().unwrap();
        let (origin, server) = http_server(1, |_, _, _| (200, r#"{"access_token":"a"}"#.into()));
        assert!(exchange(&format!("{origin}/t"), &[], &client).is_err());
        server.join().unwrap();
        // Scopes: those mail needs, among those asked for.
        let gmail = Account::new(Provider::Gmail);
        let (_, _, asked) = endpoints(Provider::Gmail);
        assert_eq!(needed_scopes(&gmail, asked), ["https://mail.google.com/"]);
        assert!(granted_scope("openid https://mail.google.com/ email", "https://mail.google.com/"));
        assert!(!granted_scope("openid email profile", "https://mail.google.com/"));
        let outlook = Account::new(Provider::Outlook);
        let (_, _, asked) = endpoints(Provider::Outlook);
        assert_eq!(needed_scopes(&outlook, asked).len(), 2);
        // Microsoft writes its scopes in its own case.
        assert!(granted_scope(
            "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
            "https://outlook.office.com/imap.accessasuser.all"
        ));
        let jmap = Account::new(Provider::Jmap);
        assert_eq!(needed_scopes(&jmap, "urn:ietf:params:jmap:mail offline_access"), [crate::sync::jmap::MAIL]);
        // Not asked for (a server listing other scopes): not required.
        assert!(needed_scopes(&jmap, "offline_access").is_empty());
        assert!(needed_scopes(&Account::new(Provider::Imap), "openid mail").is_empty());
    }

    /// RFC 8414 §2 and RFC 9700 §2.1.1: PKCE must allow S256, and the
    /// signing keys must be served over https.
    #[test]
    fn metadata_must_allow_s256_and_serve_keys_over_https() {
        let doc = |extra: serde_json::Value| {
            let mut v = serde_json::json!({
                "issuer": "https://id.example",
                "authorization_endpoint": "https://id.example/a",
                "token_endpoint": "https://id.example/t",
            });
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            parse_metadata(&v)
        };
        assert!(doc(serde_json::json!({})).is_ok());
        assert!(doc(serde_json::json!({"code_challenge_methods_supported": ["plain", "S256"]})).is_ok());
        assert!(doc(serde_json::json!({"code_challenge_methods_supported": ["plain"]})).is_err());
        assert!(doc(serde_json::json!({"code_challenge_methods_supported": []})).is_err());
        assert!(doc(serde_json::json!({"jwks_uri": "https://id.example/keys"})).is_ok());
        // Tests accept loopback http endpoints; a jwks_uri that isn't a URL
        // at all is refused everywhere.
        assert!(doc(serde_json::json!({"jwks_uri": "not a url"})).is_err());
    }

    /// RFC 7591 and RFC 7592: a registration is kept and used again; one for
    /// another loopback port is updated in place; one the server forgot is
    /// made again.
    #[test]
    fn dynamic_registrations_are_kept_and_reused() {
        let origin_cell: Arc<Mutex<String>> = Arc::default();
        let named = origin_cell.clone();
        let forget = Arc::new(AtomicBool::new(false));
        let forgotten = forget.clone();
        let (origin, server) = http_server(4, move |method, path, body| {
            let origin = named.lock().unwrap().clone();
            let v: serde_json::Value = serde_json::from_str(body).unwrap();
            match (method, path) {
                ("POST", "/register") => (
                    201,
                    serde_json::json!({
                        "client_id": format!("c-{}", v["redirect_uris"][0].as_str().unwrap().len()),
                        "registration_access_token": "rat-1",
                        "registration_client_uri": format!("{origin}/register/c"),
                    })
                    .to_string(),
                ),
                ("PUT", "/register/c") if forgotten.load(Ordering::Relaxed) => (401, "{}".into()),
                ("PUT", "/register/c") => {
                    assert_eq!(v["redirect_uris"][0], "http://127.0.0.1:6000/callback");
                    (200, serde_json::json!({"client_id": v["client_id"]}).to_string())
                }
                other => panic!("{other:?}"),
            }
        });
        *origin_cell.lock().unwrap() = origin.clone();
        let endpoint = format!("{origin}/register");
        // First sign-in: registered, and kept.
        let first = registered(&endpoint, "http://127.0.0.1:5000/callback").unwrap();
        let kept = Registration::load(&endpoint).unwrap();
        assert_eq!((kept.client_id.as_str(), kept.port()), (first.as_str(), Some(5000)));
        assert_eq!(kept.registration_access_token, "rat-1");
        // Again on the same port: no request at all.
        assert_eq!(registered(&endpoint, "http://127.0.0.1:5000/callback").unwrap(), first);
        // On another port: updated, the same client, its token kept.
        assert_eq!(registered(&endpoint, "http://127.0.0.1:6000/callback").unwrap(), first);
        let kept = Registration::load(&endpoint).unwrap();
        assert_eq!((kept.port(), kept.registration_access_token.as_str()), (Some(6000), "rat-1"));
        // The server forgot it: registered again.
        forget.store(true, Ordering::Relaxed);
        let again = registered(&endpoint, "http://127.0.0.1:7000000/callback").unwrap();
        assert_ne!(again, first);
        let seen = server.join().unwrap();
        let methods: Vec<&str> = seen.iter().map(|s| s.split(' ').next().unwrap()).collect();
        assert_eq!(methods, ["POST", "PUT", "PUT", "POST"]);
        // A management URI over plain http (outside tests) isn't kept.
        let r = Registration::from_response(&serde_json::json!({"client_id": "x"}), "r").unwrap();
        assert!(r.registration_client_uri.is_empty());
    }

    #[test]
    fn pkce_is_url_safe_and_correct_length() {
        let v = random_url_token();
        assert_eq!(v.len(), 43);
        assert!(!v.contains(['+', '/', '=']));
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(v)).len(), 43);
    }
    #[test]
    fn id_token_names_the_account() {
        let token = |claims: &str| format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims));
        assert_eq!(
            identity(&token(r#"{"email":"a@gmail.com","name":"A B"}"#)),
            Identity {
                email: "a@gmail.com".into(),
                name: "A B".into()
            }
        );
        // Microsoft personal accounts may name the address only here.
        assert_eq!(
            identity(&token(r#"{"preferred_username":"c@outlook.com"}"#)).email,
            "c@outlook.com"
        );
        assert_eq!(identity("garbage"), Identity::default());
    }
    /// An unsigned ID token carrying `claims` (only its claims are read).
    fn id_token(claims: serde_json::Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }
    const NOW: i64 = 1_800_000_000;

    /// OpenID Connect Core §3.1.3.7, with tokens as Google issues them.
    #[test]
    fn id_tokens_from_google_are_checked() {
        let google = |iss: &str| {
            id_token(serde_json::json!({
                "iss": iss, "aud": "picacho.apps.googleusercontent.com",
                "azp": "picacho.apps.googleusercontent.com", "sub": "1093",
                "email": "ana@gmail.com", "exp": NOW + 3600, "iat": NOW,
            }))
        };
        let check = |token: &str| {
            checked_identity_at(token, "picacho.apps.googleusercontent.com", Issuer::Google, None, NOW)
        };
        // Both of Google's issuer forms.
        for iss in ["https://accounts.google.com", "accounts.google.com"] {
            let (who, sub) = check(&google(iss)).unwrap();
            assert_eq!((who.email.as_str(), sub.as_str()), ("ana@gmail.com", "1093"));
        }
        // Another issuer, or none, is refused.
        assert!(check(&google("https://accounts.google.com.evil.example")).is_err());
        let mut none: serde_json::Value = claims_of(&google("x"));
        none.as_object_mut().unwrap().remove("iss");
        assert!(check(&id_token(none)).is_err());
    }

    /// Microsoft's `common` endpoint: the issuer has the token's own tenant.
    #[test]
    fn id_tokens_from_microsoft_name_their_tenant() {
        let microsoft = |iss: &str, tid: &str| {
            id_token(serde_json::json!({
                "iss": iss, "tid": tid, "aud": "client-guid", "sub": "AAAA",
                "preferred_username": "ana@outlook.com", "exp": NOW + 3600,
            }))
        };
        let check = |token: &str| checked_identity_at(token, "client-guid", Issuer::Microsoft, None, NOW);
        // A personal account's tenant, and an organization's.
        for tid in ["9188040d-6c67-4c5b-b112-36a304b66dad", "72f988bf-86f1-41af-91ab-2d7cd011db47"] {
            let iss = format!("https://login.microsoftonline.com/{tid}/v2.0");
            assert_eq!(check(&microsoft(&iss, tid)).unwrap().0.email, "ana@outlook.com");
        }
        let tid = "72f988bf-86f1-41af-91ab-2d7cd011db47";
        // The issuer of another tenant than the token's.
        assert!(check(&microsoft("https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0", tid)).is_err());
        // The template itself, or a tenant that isn't a tenant id.
        assert!(check(&microsoft("https://login.microsoftonline.com/{tenantid}/v2.0", "{tenantid}")).is_err());
        assert!(check(&microsoft("https://login.microsoftonline.com/x/y/v2.0", "x/y")).is_err());
        // The v1 issuer (sts.windows.net) isn't the v2 endpoint's.
        assert!(check(&microsoft(&format!("https://sts.windows.net/{tid}/"), tid)).is_err());
    }

    /// A JMAP server's (Fastmail) and an organization's (Keycloak) tokens:
    /// the issuer their metadata names, exactly.
    #[test]
    fn id_tokens_from_jmap_servers_and_organizations_name_their_issuer() {
        for (issuer, client) in [
            ("https://api.fastmail.com", "35c941ae"),
            ("https://login.example.edu/realms/staff", "picacho"),
        ] {
            let token = |iss: &str| {
                id_token(serde_json::json!({
                    "iss": iss, "aud": client, "sub": "u1",
                    "email": "ana@example.edu", "exp": NOW + 60,
                }))
            };
            assert!(checked_identity_at(&token(issuer), client, Issuer::Exact(issuer), None, NOW).is_ok());
            assert!(checked_identity_at(&token("https://other.example"), client, Issuer::Exact(issuer), None, NOW).is_err());
            // A grant from such a server checks against its metadata's
            // issuer; metadata without one can't be checked against.
            let grant = Grant {
                code: String::new(),
                redirect: String::new(),
                verifier: String::new(),
                client_id: client.into(),
                token: String::new(),
                issuer: Some(issuer.into()),
                client_auth: ClientAuth::Basic,
                scope: String::new(),
            };
            assert!(matches!(grant.issuer(Provider::Jmap, true), Issuer::Exact(i) if i == issuer));
            let grant = Grant { issuer: None, ..grant };
            assert!(matches!(grant.issuer(Provider::Imap, true), Issuer::Unknown));
            assert!(matches!(grant.issuer(Provider::Gmail, false), Issuer::Google));
            assert!(matches!(grant.issuer(Provider::Outlook, false), Issuer::Microsoft));
        }
    }

    /// Audiences, `azp`, expiry with leeway, the nonce and the subject.
    #[test]
    fn id_token_audience_expiry_nonce_and_subject_are_enforced() {
        let base = serde_json::json!({
            "iss": "https://id.example", "aud": "me", "sub": "u1", "exp": NOW, "nonce": "n1",
        });
        let with = |changes: serde_json::Value| {
            let mut claims = base.clone();
            for (k, v) in changes.as_object().unwrap() {
                if v.is_null() {
                    claims.as_object_mut().unwrap().remove(k);
                } else {
                    claims[k] = v.clone();
                }
            }
            id_token(claims)
        };
        let check = |token: &str, now: i64| {
            checked_identity_at(token, "me", Issuer::Exact("https://id.example"), Some("n1"), now)
        };
        assert!(check(&with(serde_json::json!({})), NOW).is_ok());
        // Clock skew: up to CLOCK_SKEW past `exp`, not beyond.
        assert!(check(&with(serde_json::json!({})), NOW + CLOCK_SKEW).is_ok());
        assert!(check(&with(serde_json::json!({})), NOW + CLOCK_SKEW + 1).is_err());
        // No `exp` at all.
        assert!(check(&with(serde_json::json!({"exp": null})), NOW).is_err());
        // Another app's token.
        assert!(check(&with(serde_json::json!({"aud": "other"})), NOW).is_err());
        // Several audiences: `azp` must say this app.
        assert!(check(&with(serde_json::json!({"aud": ["other", "me"]})), NOW).is_err());
        assert!(check(&with(serde_json::json!({"aud": ["other", "me"], "azp": "other"})), NOW).is_err());
        assert!(check(&with(serde_json::json!({"aud": ["other", "me"], "azp": "me"})), NOW).is_ok());
        // One audience, with an `azp` for someone else.
        assert!(check(&with(serde_json::json!({"azp": "other"})), NOW).is_err());
        // The nonce sent, and a subject.
        assert!(check(&with(serde_json::json!({"nonce": "n2"})), NOW).is_err());
        assert!(check(&with(serde_json::json!({"nonce": null})), NOW).is_err());
        assert!(check(&with(serde_json::json!({"sub": ""})), NOW).is_err());
        // No ID token: no identity, and no error.
        assert_eq!(check("", NOW).unwrap(), (Identity::default(), String::new()));
        assert!(check("garbage", NOW).is_err());
    }

    #[test]
    fn loopback_callback_ignores_forged_state() {
        let browser = Browser::new("127.0.0.1", "/callback").unwrap();
        assert!(browser.redirect.starts_with("http://127.0.0.1:"));
        assert!(browser.redirect.ends_with("/callback"));
        let port = browser
            .listener
            .as_ref()
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let state = browser.state.clone();
        let visit = std::thread::spawn(move || {
            for query in [
                "state=wrong&code=forged".to_owned(),
                format!("state={state}&code=real"),
            ] {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET /callback?{query} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let _ = s.read(&mut [0; 512]);
            }
        });
        assert_eq!(browser.wait(&AtomicBool::new(false)).unwrap(), "real");
        visit.join().unwrap();
        assert!(
            browser.wait(&AtomicBool::new(true)).is_err(),
            "cancel stops waiting"
        );
    }
    #[test]
    fn pinal_accounts_use_tokens_on_pinal_servers() {
        let a = pinal_account("sam@sonoran.exchange", "");
        assert!(a.oauth && a.service == Service::Pinal);
        assert_eq!(a.name, "sam@sonoran.exchange");
        assert_eq!(a.client_id, pinal_client_id());
        a.validate().unwrap();
        assert!(PINAL_SCOPES.split(' ').any(|s| s == "offline_access"));
        assert!(PINAL_SCOPES.split(' ').any(|s| s == "mail.send"));
        assert_eq!(pinal_domains(), vec!["sonoran.exchange".to_owned()]);
    }
    #[test]
    fn opened_url_answers_hosted_sign_in() {
        let browser = Browser::hosted("https://accounts.pinal.tech/native/callback".into());
        opened(vec![
            "https://elsewhere.example/?state=x&code=no".into(),
            "picacho://callback?state=forged&code=no".into(),
            format!("picacho://callback?state={}&code=real&iss=x", browser.state),
        ]);
        assert_eq!(browser.wait(&AtomicBool::new(false)).unwrap(), "real");
        opened(vec![format!(
            "picacho://callback?state={}&error=access_denied",
            browser.state
        )]);
        assert!(browser.wait(&AtomicBool::new(false)).is_err());
    }
}
