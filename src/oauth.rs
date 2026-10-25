use crate::model::{Account, Provider, Secret};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn endpoints(provider: Provider) -> (&'static str, &'static str, &'static str) {
    match provider {
        Provider::Gmail => (
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://mail.google.com/",
        ),
        _ => (
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send",
        ),
    }
}
fn random_url_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
pub fn authorize(account: &Account, cancel: Arc<AtomicBool>) -> Result<Secret> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect = format!("http://localhost:{}/", listener.local_addr()?.port());
    let verifier = random_url_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_token();
    let (auth, token, scope) = endpoints(account.provider);
    let mut url = url::Url::parse(auth)?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", account.client_id.as_str()),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", scope),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("login_hint", &account.email),
    ]);
    if account.provider == Provider::Gmail {
        url.query_pairs_mut()
            .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
    }
    open::that(url.as_str()).context("Could not open the system browser")?;
    let start = Instant::now();
    let code = loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Sign-in cancelled.");
        ensure!(
            start.elapsed() < Duration::from_secs(180),
            "Sign-in timed out. Please try again."
        );
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let mut request = [0u8; 8192];
                let n = stream.read(&mut request)?;
                let request = String::from_utf8_lossy(&request[..n]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|s| s.split_whitespace().nth(1))
                    .unwrap_or("/");
                let callback = url::Url::parse(&format!("http://localhost{target}"))?;
                let params: std::collections::HashMap<_, _> =
                    callback.query_pairs().into_owned().collect();
                let valid = params.get("state") == Some(&state);
                let body = if valid && params.contains_key("code") {
                    "Sign-in complete. You can return to Pinal Mail."
                } else {
                    "Sign-in could not be completed. Return to Pinal Mail and try again."
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                if !valid {
                    continue;
                }
                if params.contains_key("error") {
                    bail!(
                        "Provider declined sign-in: {}",
                        params.get("error").unwrap()
                    );
                }
                if let Some(code) = params.get("code") {
                    break code.clone();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    };
    let mut fields = vec![
        ("client_id", account.client_id.as_str()),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &verifier),
    ];
    if !account.client_secret.is_empty() {
        fields.push(("client_secret", &account.client_secret));
    }
    let mut secret = exchange(token, &fields)?;
    secret.client_secret = account.client_secret.clone();
    Ok(secret)
}
fn exchange(endpoint: &str, fields: &[(&str, &str)]) -> Result<Secret> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .post(endpoint)
        .form(fields)
        .send()?;
    let status = response.status();
    let value: serde_json::Value = response.json()?;
    // Do not include the provider response: it can contain credentials or codes.
    ensure!(
        status.is_success(),
        "OAuth token request failed ({status}). Check your application registration and consent settings."
    );
    let access_token = value["access_token"]
        .as_str()
        .context("Provider did not return an access token")?
        .to_string();
    Ok(Secret {
        access_token,
        refresh_token: value["refresh_token"].as_str().unwrap_or_default().into(),
        expires_at: chrono::Utc::now().timestamp() + value["expires_in"].as_i64().unwrap_or(3600),
        ..Default::default()
    })
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
    let next = exchange(token, &fields)?;
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
}
