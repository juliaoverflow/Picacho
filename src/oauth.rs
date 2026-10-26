use crate::model::{Account, Provider, Secret, Service};
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
        .timeout(Duration::from_secs(30))
        .build()?)
}

/// `picacho://` URLs macOS handed the app, waiting for a sign-in to claim them.
static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub fn opened(urls: Vec<String>) {
    OPENED
        .lock()
        .unwrap()
        .extend(urls.into_iter().filter(|u| u.starts_with("picacho:")));
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
    fn code(&self, url: &url::Url, cancel: &AtomicBool) -> Result<String> {
        open::that(url.as_str()).context("Could not open the system browser")?;
        self.wait(cancel)
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
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                    let mut request = [0u8; 8192];
                    let n = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..n]);
                    let target = request
                        .lines()
                        .next()
                        .and_then(|s| s.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let callback = url::Url::parse(&format!("http://localhost{target}"))?;
                    let answer = self.answer(&callback);
                    let body = if matches!(answer, Some(Ok(_))) {
                        "Sign-in complete. You can return to Picacho."
                    } else {
                        "Sign-in could not be completed. Return to Picacho and try again."
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

/// Signs in to Gmail or Outlook in the browser. The account's email may be
/// empty: the provider's ID token says which account was chosen.
pub fn authorize(account: &Account, cancel: Arc<AtomicBool>) -> Result<(Secret, Identity)> {
    let microsoft = account.provider == Provider::Outlook;
    let browser = Browser::new(if microsoft { "localhost" } else { "127.0.0.1" }, "/")?;
    let (auth, token, scope) = endpoints(account.provider);
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
    let mut fields = vec![
        ("client_id", account.client_id.as_str()),
        ("code", &code),
        ("redirect_uri", &browser.redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &browser.verifier),
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

/// A Pinal mailbox with the app password issued to this device.
#[derive(Debug, serde::Deserialize)]
pub struct PinalMailbox {
    pub address: String,
    #[serde(default)]
    pub name: String,
    pub password: String,
    pub imap: PinalServer,
    pub smtp: PinalServer,
}
#[derive(Debug, serde::Deserialize)]
pub struct PinalServer {
    pub host: String,
    pub port: u16,
}
impl PinalMailbox {
    pub fn account(&self) -> Account {
        let mut a = Service::Pinal.account(&self.address).unwrap();
        a.name = if self.name.is_empty() {
            self.address.clone()
        } else {
            self.name.clone()
        };
        a.incoming_host = self.imap.host.clone();
        a.incoming_port = self.imap.port;
        a.smtp_host = self.smtp.host.clone();
        a.smtp_port = self.smtp.port;
        a
    }
}
/// Signs in to Pinal Accounts with OpenID Connect, then exchanges the
/// access token for the primary mailbox's settings and an app password.
pub fn pinal(cancel: Arc<AtomicBool>) -> Result<PinalMailbox> {
    let origin = pinal_accounts();
    let client_id = pinal_client_id();
    let http = http()?;
    let config: serde_json::Value = http
        .get(format!(
            "{origin}/api/auth/.well-known/openid-configuration"
        ))
        .send()
        .and_then(|r| r.error_for_status())
        .context("Could not reach Pinal Accounts. Check your connection.")?
        .json()?;
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
    let (auth, token) = (
        endpoint("authorization_endpoint")?,
        endpoint("token_endpoint")?,
    );
    let browser = if bundled() {
        Browser::hosted(format!("{origin}/native/callback"))
    } else {
        Browser::new("127.0.0.1", "/callback")?
    };
    let mut url = auth;
    let nonce = random_url_token();
    url.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", &browser.redirect),
        ("response_type", "code"),
        ("scope", "openid profile email"),
        ("state", &browser.state),
        ("nonce", &nonce),
        ("code_challenge", &browser.challenge),
        ("code_challenge_method", "S256"),
    ]);
    let code = browser.code(&url, &cancel)?;
    let (secret, _) = exchange(
        token.as_str(),
        &[
            ("client_id", client_id.as_str()),
            ("code", &code),
            ("redirect_uri", &browser.redirect),
            ("grant_type", "authorization_code"),
            ("code_verifier", &browser.verifier),
        ],
    )?;
    ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
    let response = http
        .post(format!("{origin}/api/native/mail"))
        .bearer_auth(&secret.access_token)
        .send()
        .context("Could not reach Pinal Accounts. Check your connection.")?;
    let status = response.status();
    if !status.is_success() {
        let message = response
            .json::<serde_json::Value>()
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("Pinal could not set up your mailbox ({status})."));
        bail!(message);
    }
    Ok(response.json()?)
}

fn exchange(endpoint: &str, fields: &[(&str, &str)]) -> Result<(Secret, serde_json::Value)> {
    let response = http()?.post(endpoint).form(fields).send()?;
    let status = response.status();
    let value: serde_json::Value = response.json()?;
    // Do not include the provider response: it can contain credentials or codes.
    ensure!(
        status.is_success(),
        "Sign-in failed at the token step ({status}). Please try again."
    );
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
pub fn refresh(account: &Account, secret: &mut Secret) -> Result<()> {
    if !account.oauth || secret.expires_at > chrono::Utc::now().timestamp() + 120 {
        return Ok(());
    }
    ensure!(
        !secret.refresh_token.is_empty(),
        "Your sign-in expired. Reconnect the account."
    );
    let (_, token, scope) = endpoints(account.provider);
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
    crate::store::write_secret(&account.id, secret)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
