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
}
/// Reads a server's metadata document, keeping only https endpoints.
pub fn parse_metadata(v: &serde_json::Value) -> Result<Metadata> {
    let https = |key: &str| -> Option<String> {
        let u = v[key].as_str()?;
        let parsed = url::Url::parse(u).ok()?;
        (parsed.scheme() == "https" || cfg!(test)).then(|| u.to_owned())
    };
    Ok(Metadata {
        authorization: https("authorization_endpoint")
            .context("The server's sign-in settings have no https authorization endpoint.")?,
        token: https("token_endpoint")
            .context("The server's sign-in settings have no https token endpoint.")?,
        registration: https("registration_endpoint"),
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
            .and_then(|r| r.json::<serde_json::Value>());
        // A document that can't be used is passed over for the next one.
        match found
            .map_err(anyhow::Error::from)
            .and_then(|v| issued_by(&v, &origin).and_then(|()| parse_metadata(&v)))
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
/// Checks that a metadata document fetched from `origin` is that server's
/// own (RFC 8414 §3.3): its `issuer`, where it names one, is `origin`.
fn issued_by(v: &serde_json::Value, origin: &str) -> Result<()> {
    match v["issuer"].as_str() {
        None => Ok(()),
        Some(issuer) if issuer.trim_end_matches('/').eq_ignore_ascii_case(origin) => Ok(()),
        Some(issuer) => Err(anyhow::anyhow!(
            "its sign-in settings are for {issuer}, not {origin}"
        )),
    }
}
/// Registers Picacho with a server that allows apps to (RFC 7591), for
/// `redirect`: a public client, without a secret (RFC 8252).
fn register(endpoint: &str, redirect: &str) -> Result<String> {
    let v: serde_json::Value = http()?
        .post(endpoint)
        .json(&serde_json::json!({
            "client_name": "Picacho",
            "application_type": "native",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .context("Couldn't reach the server to register Picacho for sign-in.")?
        .error_for_status()
        .context("The server didn't register Picacho for browser sign-in.")?
        .json()?;
    Ok(v["client_id"]
        .as_str()
        .context("The server registered Picacho without a client id.")?
        .to_owned())
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
fn random_url_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(30))
        .build()?)
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
        .json()?;
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
}
impl Browser {
    /// Microsoft registers `http://localhost`; Google and Pinal accept the
    /// loopback address with any port.
    fn new(host: &str, path: &str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
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
        }
    }
    /// Opens `url` in the system browser and waits for the authorization code.
    #[cfg(not(target_os = "android"))]
    fn code(&self, url: &url::Url, cancel: &AtomicBool) -> Result<String> {
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
fn identity(id_token: &str) -> Identity {
    let claims = id_token
        .split('.')
        .nth(1)
        .and_then(|p| {
            URL_SAFE_NO_PAD
                .decode(p.trim_end_matches('='))
                .or_else(|_| URL_SAFE.decode(p))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default();
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
    })
}

/// Signs in to a JMAP server in the browser, with the endpoints its
/// metadata names: as the app's own registration where there is one
/// (Fastmail), else registered for this sign-in.
fn authorize_jmap(account: &Account, cancel: Arc<AtomicBool>) -> Result<Grant> {
    let metadata = jmap_metadata(&account.incoming_host, account.incoming_port)?;
    let browser = Browser::new("127.0.0.1", "/callback")?;
    let own = jmap_client(account);
    let client_id = if !own.id.is_empty() {
        own.id
    } else if let Some(endpoint) = &metadata.registration {
        register(endpoint, &browser.redirect)?
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
    })
}

/// Exchanges the browser's code for tokens and says who signed in. A
/// provider rejection (`AccessFailure::Authentication`) means the code is
/// spent; any other failure may be retried with the same grant.
pub fn redeem(account: &Account, grant: &Grant) -> Result<(Secret, Identity)> {
    let jmap = account.provider == Provider::Jmap;
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
    let mut fields = vec![
        ("client_id", client_id),
        ("code", &grant.code),
        ("redirect_uri", &grant.redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &grant.verifier),
    ];
    if !account.client_secret.is_empty() {
        fields.push(("client_secret", &account.client_secret));
    }
    let (mut secret, value) = exchange(token, &fields)?;
    secret.client_secret = account.client_secret.clone();
    Ok((
        secret,
        identity(value["id_token"].as_str().unwrap_or_default()),
    ))
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
            && let Ok(value) = response.json::<serde_json::Value>()
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
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("scope", PINAL_SCOPES),
        ("state", &browser.state),
        ("nonce", &random_url_token()),
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
    })
}
/// Turns Pinal's code into tokens and says whose mailbox they open.
fn pinal_redeem(request: &PinalRequest, code: &str) -> Result<(Secret, Identity)> {
    let client_id = pinal_client_id();
    let (secret, value) = exchange(
        request.endpoints.token.as_str(),
        &[
            ("client_id", client_id.as_str()),
            ("code", code),
            ("redirect_uri", &request.browser.redirect),
            ("grant_type", "authorization_code"),
            ("code_verifier", &request.browser.verifier),
        ],
    )?;
    let mut who = identity(value["id_token"].as_str().unwrap_or_default());
    if who.email.is_empty() {
        // Pinal keeps the ID token small; the address is in its userinfo.
        let info: serde_json::Value = http()?
            .get(request.endpoints.userinfo.clone())
            .bearer_auth(&secret.access_token)
            .send()
            .and_then(|r| r.error_for_status())
            .context("Could not reach Pinal Accounts. Check your connection.")?
            .json()?;
        let text = |key: &str| info[key].as_str().unwrap_or_default().to_owned();
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

fn exchange(endpoint: &str, fields: &[(&str, &str)]) -> Result<(Secret, serde_json::Value)> {
    let response = http()?.post(endpoint).form(fields).send()?;
    let status = response.status();
    let value: serde_json::Value = response.json()?;
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
    let (token, scope) = if account.service == Service::Pinal {
        found_token = pinal_endpoints(&http()?)?.token.to_string();
        (found_token.as_str(), "")
    } else if account.provider == Provider::Jmap {
        found_token = jmap_metadata(&account.incoming_host, account.incoming_port)?
            .token
            .to_string();
        (found_token.as_str(), "")
    } else {
        let (_, token, scope) = endpoints(account.provider);
        (token, scope)
    };
    let mut fields = vec![
        ("client_id", account.client_id.as_str()),
        ("refresh_token", &secret.refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if account.provider == Provider::Outlook {
        fields.push(("scope", scope));
    }
    if !secret.client_secret.is_empty() {
        fields.push(("client_secret", &secret.client_secret));
    }
    let (next, _) = exchange(token, &fields)?;
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
        assert!(issued_by(&serde_json::json!({}), origin).is_ok());
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
