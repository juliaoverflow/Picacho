//! An organization's single sign-on, end to end: a Keycloak realm that
//! signs people in through another realm over SAML 2.0, Picacho signing in
//! to it in the "browser" (a script that fills in the SAML identity
//! provider's form and posts its assertion back), and loopback IMAP, POP3
//! and SMTP servers that take the token with OAUTHBEARER and check it with
//! Keycloak's userinfo endpoint, as real servers do. Opt-in:
//! `PICACHO_KEYCLOAK=http://127.0.0.1:18080 cargo test -- --ignored sso`,
//! with Keycloak set up as `scripts/keycloak-sso.sh` does.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use protocol_tests::{backend, tls_fixture};
use std::io::{BufRead, BufReader, Write};

fn keycloak() -> String {
    std::env::var("PICACHO_KEYCLOAK").unwrap_or_else(|_| "http://127.0.0.1:18080".into())
}

/// The person: signs in at the SAML identity provider (realm `corp`) as
/// ana, through `scripts/sso-browser.py`.
fn browse(url: url::Url) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/sso-browser.py");
    let out = std::process::Command::new("python3")
        .args([script, url.as_str(), "ana", "ana-password"])
        .output()
        .expect("python3");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("saml steps 2"),
        "the sign-in went through SAML (request and response): {text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Who a token belongs to, as the identity provider says (its userinfo).
fn token_owner(token: &str) -> Option<String> {
    let v: serde_json::Value = reqwest::blocking::Client::new()
        .get(format!(
            "{}/realms/mail/protocol/openid-connect/userinfo",
            keycloak()
        ))
        .bearer_auth(token)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    v["email"].as_str().map(str::to_owned)
}

/// OAUTHBEARER's or XOAUTH2's token, from the client's first message.
fn bearer(message: &[u8]) -> String {
    let text = String::from_utf8_lossy(message);
    text.split('\x01')
        .find_map(|f| f.strip_prefix("auth=Bearer "))
        .unwrap_or_default()
        .to_owned()
}

/// The end of a reply after which the server hangs up.
const BYE: &str = " bye\r\n";
/// A loopback TLS server: each connection gets `greeting`, then lines are
/// answered by `answer` (None hangs up), on its own thread.
fn serve(
    acceptor: Arc<native_tls::TlsAcceptor>,
    greeting: &'static str,
    answer: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    // A reply that ends with this closes the connection after it is sent.
) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let answer = Arc::new(answer);
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let (acceptor, answer) = (acceptor.clone(), answer.clone());
            std::thread::spawn(move || {
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(20)))
                    .unwrap();
                let Ok(stream) = acceptor.accept(socket) else {
                    return;
                };
                let mut r = BufReader::new(stream);
                let _ = r.get_mut().write_all(greeting.as_bytes());
                let mut line = String::new();
                while r.read_line(&mut line).unwrap_or(0) > 0 {
                    let Some(reply) = answer(line.trim_end()) else {
                        return;
                    };
                    if r.get_mut().write_all(reply.as_bytes()).is_err() || reply.ends_with(BYE) {
                        return;
                    }
                    line.clear();
                }
            });
        }
    });
    port
}

#[test]
#[ignore = "needs Keycloak with the SAML-federated realms of scripts/keycloak-sso.sh"]
fn sso_through_saml_signs_in_to_imap_pop3_and_smtp_and_renews() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let acceptor = Arc::new(acceptor);
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();

    // IMAP: OAUTHBEARER (SASL-IR), checked with the identity provider.
    let log = seen.clone();
    let imap = serve(acceptor.clone(), "* OK ready\r\n", move |line| {
        let (tag, command) = line.split_once(' ')?;
        let upper = command.to_ascii_uppercase();
        Some(if upper == "CAPABILITY" {
            format!(
                "* CAPABILITY IMAP4rev1 SASL-IR AUTH=OAUTHBEARER AUTH=XOAUTH2\r\n{tag} OK done\r\n"
            )
        } else if let Some(rest) = upper.strip_prefix("AUTHENTICATE OAUTHBEARER ") {
            let message = STANDARD
                .decode(&command[command.len() - rest.len()..])
                .unwrap();
            match token_owner(&bearer(&message)) {
                Some(owner) => {
                    log.lock().unwrap().push(format!("imap:{owner}"));
                    format!("{tag} OK signed in\r\n")
                }
                None => format!("{tag} NO token refused\r\n"),
            }
        } else if upper == "LOGOUT" {
            format!("* BYE\r\n{tag} OK bye\r\n")
        } else {
            format!("{tag} OK done\r\n")
        })
    });
    // POP3: AUTH OAUTHBEARER, its message after the empty challenge.
    let log = seen.clone();
    let pending = Arc::new(Mutex::new(false));
    let pop = serve(acceptor.clone(), "+OK ready\r\n", move |line| {
        let mut waiting = pending.lock().unwrap();
        Some(if *waiting {
            *waiting = false;
            match token_owner(&bearer(&STANDARD.decode(line).unwrap_or_default())) {
                Some(owner) => {
                    log.lock().unwrap().push(format!("pop:{owner}"));
                    "+OK signed in\r\n".into()
                }
                None => "-ERR token refused\r\n".into(),
            }
        } else if line == "CAPA" {
            "+OK\r\nSASL OAUTHBEARER\r\nUIDL\r\n.\r\n".into()
        } else if line == "AUTH OAUTHBEARER" {
            *waiting = true;
            "+ \r\n".into()
        } else if line == "UIDL" {
            "+OK\r\n.\r\n".into()
        } else if line == "QUIT" {
            "+OK bye\r\n".into()
        } else {
            panic!("POP3 got {line}")
        })
    });
    // SMTP (TLS from the start): AUTH OAUTHBEARER with its first message.
    let log = seen.clone();
    let smtp = serve(acceptor.clone(), "220 ready\r\n", move |line| {
        let upper = line.to_ascii_uppercase();
        Some(if upper.starts_with("EHLO") {
            "250-mail.example.edu\r\n250 AUTH OAUTHBEARER PLAIN\r\n".into()
        } else if let Some(rest) = upper.strip_prefix("AUTH OAUTHBEARER ") {
            let message = STANDARD.decode(&line[line.len() - rest.len()..]).unwrap();
            match token_owner(&bearer(&message)) {
                Some(owner) => {
                    log.lock().unwrap().push(format!("smtp:{owner}"));
                    "235 2.7.0 signed in\r\n".into()
                }
                None => "535 5.7.8 token refused\r\n".into(),
            }
        } else if upper == "QUIT" {
            "221 bye\r\n".into()
        } else {
            "250 ok\r\n".into()
        })
    });

    *crate::sync::oauth::TEST_BROWSER.lock().unwrap() = Some(browse);
    let (mut b, mut a, path) = backend(&cert)?;
    b.test_secret = None;
    a.oauth = true;
    a.email = "ana@example.edu".into();
    a.client_id = "picacho".into();
    a.sso = Some(crate::manage::model::Sso {
        issuer: format!("{}/realms/mail", keycloak()),
        scope: String::new(),
    });
    a.incoming_port = imap;
    a.smtp_port = smtp;
    a.smtp_tls = Some(true);
    // The sign-in, as Add Account runs it: browser, tokens, IMAP check.
    let grant = crate::sync::oauth::authorize_with(&a, &[], Arc::new(AtomicBool::new(false)))?;
    let (mut secret, who) = crate::sync::oauth::redeem(&a, &grant)?;
    assert_eq!(who.email, "ana@example.edu", "the SAML assertion's address");
    assert!(!secret.refresh_token.is_empty(), "offline access");
    let session = b.imap_login(&a, &secret);
    assert!(session.is_ok(), "{:#}", session.err().unwrap());
    drop(session);
    let mut p = a.clone();
    p.provider = Provider::Pop3;
    p.incoming_port = pop;
    let mut session = b.pop_login(&p, &secret.access_token)?;
    session.quit()?;
    crate::sync::smtp::check_server(
        &crate::sync::smtp::Endpoint::of(&a),
        &secret.access_token,
        &b.tls_connector()?,
    )?;
    // An expired token is renewed at the issuer's token endpoint, and the
    // new one is taken too.
    let old = secret.access_token.clone();
    secret.expires_at = 0;
    assert!(crate::sync::oauth::refresh(&a, &mut secret)?);
    assert_ne!(secret.access_token, old);
    assert!(b.imap_login(&a, &secret).is_ok());
    // A forged token is refused.
    let mut forged = secret.clone();
    forged.access_token = "not-a-token".into();
    assert!(b.imap_login(&a, &forged).is_err());
    *crate::sync::oauth::TEST_BROWSER.lock().unwrap() = None;
    assert_eq!(
        seen.lock().unwrap().clone(),
        [
            "imap:ana@example.edu",
            "pop:ana@example.edu",
            "smtp:ana@example.edu",
            "imap:ana@example.edu"
        ]
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
