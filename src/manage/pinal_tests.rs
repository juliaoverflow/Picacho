//! Pinal Mail against live local servers: Pinal Accounts signs in, and
//! Pinal's mail server (Picacho Server) takes the tokens over IMAP and SMTP.
//! Ignored by default; run with
//!
//! ```sh
//! PICACHO_PINAL_ACCOUNTS=http://localhost:3010 PICACHO_PINAL_CLIENT_ID=… \
//! PICACHO_PINAL_IMAP=127.0.0.1:2993 PICACHO_PINAL_SMTP=127.0.0.1:2598 \
//! PINAL_E2E_EMAIL=… PINAL_E2E_PASSWORD=… PINAL_E2E_CERT=cert.pem \
//! cargo test pinal_live -- --ignored --nocapture
//! ```
use super::*;

/// Signs in to Pinal as a browser would and returns the session cookie.
fn sign_in(origin: &str, email: &str, password: &str) -> Result<String> {
    let http = reqwest::blocking::Client::new();
    let response = http
        .post(format!("{origin}/api/auth/sign-in/email"))
        .header("origin", origin)
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()?;
    ensure!(
        response.status().is_success(),
        "sign-in: {}",
        response.status()
    );
    let cookies: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect();
    Ok(cookies.join("; "))
}

/// Follows Pinal's authorize step, accepting consent, to the app's callback.
fn approve(origin: &str, cookie: &str, url: url::Url) -> Result<()> {
    let http = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = http
        .get(url)
        .header("cookie", cookie)
        .header("origin", origin)
        .send()?;
    let mut next = match response.headers().get("location") {
        Some(l) => l.to_str()?.to_owned(),
        None => response.json::<serde_json::Value>()?["url"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    };
    if next.contains("/consent") {
        let query = url::Url::parse(origin)?
            .join(&next)?
            .query()
            .unwrap_or_default()
            .to_owned();
        let answer: serde_json::Value = http
            .post(format!("{origin}/api/auth/oauth2/consent"))
            .header("cookie", cookie)
            .header("origin", origin)
            .json(&serde_json::json!({ "accept": true, "oauth_query": query }))
            .send()?
            .json()?;
        next = answer["url"]
            .as_str()
            .or(answer["redirect_uri"].as_str())
            .unwrap_or_default()
            .to_owned();
    }
    ensure!(next.starts_with("http://127.0.0.1:"), "no callback: {next}");
    http.get(next).send()?;
    Ok(())
}

#[test]
#[ignore = "needs Pinal Accounts and Picacho Server running locally"]
fn pinal_live() -> Result<()> {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let origin = env("PICACHO_PINAL_ACCOUNTS");
    let email = env("PINAL_E2E_EMAIL");
    let cookie = sign_in(&origin, &email, &env("PINAL_E2E_PASSWORD"))?;

    // The app's own sign-in, with this test as the browser.
    let (mut secret, who) = {
        let origin = origin.clone();
        crate::sync::oauth::pinal_driven(move |url| approve(&origin, &cookie, url).unwrap())?
    };
    assert_eq!(who.email, email, "the ID token names the mailbox");
    assert!(!secret.refresh_token.is_empty(), "stays signed in");
    println!("signed in as {} ({})", who.email, who.name);

    let account = crate::sync::oauth::pinal_account(&who.email, &who.name);
    let path = std::env::temp_dir().join(format!("pinal-live-{}", uuid::Uuid::new_v4()));
    let mut backend = Backend::new(Arc::new(Store::open(&path)?));
    backend.test_certificate = Some(std::fs::read(env("PINAL_E2E_CERT"))?);
    let tls = backend.tls_connector()?;

    // IMAP and SMTP take the Pinal token (XOAUTH2/OAUTHBEARER).
    backend.check_login(&account, &mut secret)?;
    println!("IMAP accepted the Pinal token");
    let smtp = crate::sync::smtp::Endpoint::of(&account);
    crate::sync::smtp::check_server(&smtp, &secret.access_token, &tls)?;
    println!("SMTP accepted the Pinal token");

    // An expiring token is renewed at Pinal and still opens the mailbox.
    let before = secret.access_token.clone();
    secret.expires_at = 0;
    assert!(crate::sync::oauth::refresh(&account, &mut secret)?);
    assert_ne!(secret.access_token, before);
    backend.check_login(&account, &mut secret)?;
    println!("refreshed token accepted");

    // A forged token is refused as an authentication failure.
    let mut forged = secret.clone();
    forged.access_token = format!("{}x", &secret.access_token[..secret.access_token.len() - 1]);
    forged.expires_at = i64::MAX;
    let refused = backend.check_login(&account, &mut forged).unwrap_err();
    assert!(
        refused.downcast_ref::<AccessFailure>().is_some(),
        "{refused:#}"
    );
    println!("forged token refused");
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}
