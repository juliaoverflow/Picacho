//! SASL sign-in (RFC 4422) for IMAP, POP3 and SMTP: the mechanisms Picacho
//! speaks, which one to use of those a server offers, and each one's side of
//! the exchange. The protocols carry the messages (base64 in IMAP's `+`,
//! POP3's `+` and SMTP's `334` lines); this module only makes them.
//!
//! - SCRAM-SHA-256 and SCRAM-SHA-1 (RFC 5802, 7677): the password never
//!   crosses the wire, and the server proves it knows it too. Their -PLUS
//!   forms also bind the exchange to the TLS connection (RFC 5929's
//!   tls-server-end-point: a hash of the server's certificate), so a
//!   machine in the middle with a certificate of its own can't relay it.
//! - PLAIN (RFC 4616) and LOGIN: the password, inside TLS.
//! - CRAM-MD5 (RFC 2195): a keyed hash of a challenge; only where nothing
//!   better is offered, since it needs the server to keep the password.
//! - OAUTHBEARER (RFC 7628) and Google/Microsoft's XOAUTH2: an access token.
//! - GSSAPI (RFC 4752): Kerberos, with the ticket the system already has
//!   (`sync::gss`); no password is used or sent. Only for accounts set to
//!   sign in with Kerberos (`Account::kerberos`), never chosen otherwise.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};

/// SCRAM iteration counts above this are refused: a hostile server could
/// otherwise make signing in take minutes.
const MAX_ITERATIONS: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mechanism {
    ScramSha256Plus,
    ScramSha1Plus,
    ScramSha256,
    ScramSha1,
    Plain,
    Login,
    CramMd5,
    OAuthBearer,
    XOAuth2,
    Gssapi,
}

/// For a password, best first: SCRAM proves both sides without sending the
/// password; PLAIN and LOGIN send it inside TLS; CRAM-MD5 last.
pub(crate) const PASSWORD_ORDER: [Mechanism; 7] = [
    Mechanism::ScramSha256Plus,
    Mechanism::ScramSha1Plus,
    Mechanism::ScramSha256,
    Mechanism::ScramSha1,
    Mechanism::Plain,
    Mechanism::Login,
    Mechanism::CramMd5,
];
/// SCRAM alone, best first (IMAP and POP3 prefer it to plain LOGIN/USER).
pub(crate) const SCRAM_ORDER: [Mechanism; 4] = [
    Mechanism::ScramSha256Plus,
    Mechanism::ScramSha1Plus,
    Mechanism::ScramSha256,
    Mechanism::ScramSha1,
];
/// For an access token: the standard mechanism, then the older one.
pub(crate) const TOKEN_ORDER: [Mechanism; 2] = [Mechanism::OAuthBearer, Mechanism::XOAuth2];
/// For a Kerberos ticket: GSSAPI alone. An account set to Kerberos never
/// falls back to a password.
pub(crate) const KERBEROS_ORDER: [Mechanism; 1] = [Mechanism::Gssapi];

impl Mechanism {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ScramSha256Plus => "SCRAM-SHA-256-PLUS",
            Self::ScramSha1Plus => "SCRAM-SHA-1-PLUS",
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::ScramSha1 => "SCRAM-SHA-1",
            Self::Plain => "PLAIN",
            Self::Login => "LOGIN",
            Self::CramMd5 => "CRAM-MD5",
            Self::OAuthBearer => "OAUTHBEARER",
            Self::XOAuth2 => "XOAUTH2",
            Self::Gssapi => "GSSAPI",
        }
    }
    /// The best of `order` among the mechanisms a server offers (names in
    /// any case), leaving out the -PLUS ones (no channel binding to give).
    pub(crate) fn choose<'a>(
        order: &[Mechanism],
        offered: impl IntoIterator<Item = &'a str>,
    ) -> Option<Mechanism> {
        Self::choose_bound(order, offered, false)
    }
    /// As `choose`, with the -PLUS ones where `bound` (the connection's
    /// channel binding is known).
    pub(crate) fn choose_bound<'a>(
        order: &[Mechanism],
        offered: impl IntoIterator<Item = &'a str>,
        bound: bool,
    ) -> Option<Mechanism> {
        let offered: Vec<String> = offered.into_iter().map(str::to_ascii_uppercase).collect();
        order
            .iter()
            .copied()
            .filter(|m| bound || !m.plus())
            .find(|m| offered.iter().any(|o| o == m.name()))
    }
    /// SCRAM with channel binding.
    pub(crate) fn plus(self) -> bool {
        matches!(self, Self::ScramSha256Plus | Self::ScramSha1Plus)
    }
    fn scram(self) -> Option<Hash> {
        match self {
            Self::ScramSha256 | Self::ScramSha256Plus => Some(Hash::Sha256),
            Self::ScramSha1 | Self::ScramSha1Plus => Some(Hash::Sha1),
            _ => None,
        }
    }
    /// Whether the client speaks first (and so can send its first message
    /// with the command, where the protocol allows).
    fn client_first(self) -> bool {
        !matches!(self, Self::Login | Self::CramMd5)
    }
}

/// What signs in: a user name and a password or access token, and the
/// server (OAUTHBEARER names it).
#[derive(Clone)]
pub(crate) struct Credentials {
    pub(crate) user: String,
    pub(crate) secret: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    /// The TLS connection's tls-server-end-point binding (RFC 5929), for
    /// SCRAM-PLUS; empty where unknown.
    pub(crate) binding: Vec<u8>,
    /// The service's GSS-API name (`imap`, `pop`, `smtp`, `sieve`), for
    /// GSSAPI's `service@host`.
    pub(crate) service: &'static str,
}

/// The tls-server-end-point binding of a TLS stream, or empty.
pub(crate) fn binding<S: std::io::Read + std::io::Write>(
    tls: &native_tls::TlsStream<S>,
) -> Vec<u8> {
    tls.tls_server_end_point().ok().flatten().unwrap_or_default()
}

enum State {
    Start,
    /// LOGIN: the user name was sent; the password is next.
    LoginUser,
    /// SCRAM: the client's first message was sent.
    ScramFirst {
        bare: String,
        nonce: String,
        /// The GS2 header sent ("n,," or "p=tls-server-end-point,,").
        header: String,
    },
    /// SCRAM: the proof was sent; the server's signature is expected.
    ScramFinal {
        signature: Vec<u8>,
    },
    /// A token mechanism answered an error challenge; the server will
    /// refuse the sign-in next, with this reason.
    Refused(String),
    /// GSSAPI: the security context is being established.
    GssContext(Box<dyn crate::sync::gss::Context>),
    /// GSSAPI: established; the server's security-layer offer is next.
    GssLayer(Box<dyn crate::sync::gss::Context>),
    Done,
}

/// One sign-in exchange, from the client's side.
pub(crate) struct Exchange {
    mechanism: Mechanism,
    credentials: Credentials,
    state: State,
    /// SCRAM's client nonce, fixed in tests.
    nonce: Option<String>,
    /// GSSAPI's context, given in tests instead of the system's Kerberos.
    gss: Option<Box<dyn crate::sync::gss::Context>>,
}

impl Exchange {
    pub(crate) fn new(mechanism: Mechanism, credentials: Credentials) -> Self {
        Self {
            mechanism,
            credentials,
            state: State::Start,
            nonce: None,
            gss: None,
        }
    }
    pub(crate) fn mechanism(&self) -> Mechanism {
        self.mechanism
    }
    /// The first message, for mechanisms where the client speaks first.
    pub(crate) fn initial(&mut self) -> Option<Vec<u8>> {
        if !self.mechanism.client_first() || !matches!(self.state, State::Start) {
            return None;
        }
        self.step(&[]).ok()
    }
    /// The answer to a server challenge (already base64-decoded).
    pub(crate) fn step(&mut self, challenge: &[u8]) -> Result<Vec<u8>> {
        let Credentials {
            user,
            secret,
            host,
            port,
            binding: _,
            service,
        } = &self.credentials;
        let state = std::mem::replace(&mut self.state, State::Done);
        let (answer, next) = match (self.mechanism, state) {
            (Mechanism::Plain, State::Start) => {
                (format!("\0{user}\0{secret}").into_bytes(), State::Done)
            }
            (Mechanism::Login, State::Start) => (user.clone().into_bytes(), State::LoginUser),
            (Mechanism::Login, State::LoginUser) => (secret.clone().into_bytes(), State::Done),
            (Mechanism::CramMd5, State::Start) => {
                let digest = hmac_md5(secret.as_bytes(), challenge);
                (format!("{user} {}", hex(&digest)).into_bytes(), State::Done)
            }
            (Mechanism::XOAuth2, State::Start) => (
                format!("user={user}\x01auth=Bearer {secret}\x01\x01").into_bytes(),
                State::Done,
            ),
            (Mechanism::OAuthBearer, State::Start) => (
                format!(
                    "n,a={},\x01host={host}\x01port={port}\x01auth=Bearer {secret}\x01\x01",
                    gs2_name(user)
                )
                .into_bytes(),
                State::Done,
            ),
            // A challenge after the token is the server's error (JSON);
            // RFC 7628 has the client answer it with ^A, XOAUTH2 with
            // nothing, and the server then refuses.
            (Mechanism::OAuthBearer | Mechanism::XOAuth2, State::Done) => {
                let reason = token_error(challenge);
                let answer = if self.mechanism == Mechanism::OAuthBearer {
                    b"\x01".to_vec()
                } else {
                    Vec::new()
                };
                (answer, State::Refused(reason))
            }
            (Mechanism::Gssapi, State::Start) => {
                let mut context = match self.gss.take() {
                    Some(context) => context,
                    None => crate::sync::gss::start(&format!("{service}@{host}"))?,
                };
                let (token, done) = context.step(&[])?;
                (
                    token,
                    if done {
                        State::GssLayer(context)
                    } else {
                        State::GssContext(context)
                    },
                )
            }
            (Mechanism::Gssapi, State::GssContext(mut context)) => {
                let (token, done) = context.step(challenge)?;
                (
                    token,
                    if done {
                        State::GssLayer(context)
                    } else {
                        State::GssContext(context)
                    },
                )
            }
            // RFC 4752 \u{a7}3.1: the server offers security layers and its
            // largest message, wrapped; Picacho takes none (TLS protects the
            // connection) and answers with the user to act as.
            (Mechanism::Gssapi, State::GssLayer(mut context)) => {
                if challenge.is_empty() {
                    // The context's last token was ours; the server sends
                    // nothing, and is answered with nothing.
                    (Vec::new(), State::GssLayer(context))
                } else {
                    let offer = context.unwrap(challenge)?;
                    ensure!(
                        offer.len() == 4,
                        "The server's Kerberos security-layer offer is malformed."
                    );
                    ensure!(
                        offer[0] & 0x01 != 0,
                        "The server requires a Kerberos security layer Picacho doesn't use (it offers none without one)."
                    );
                    let mut answer = vec![0x01, 0, 0, 0];
                    // The user to sign in as: the ticket's own unless a
                    // different sign-in name is set (authorization identity).
                    answer.extend_from_slice(user.as_bytes());
                    (context.wrap(&answer)?, State::Done)
                }
            }
            (m, State::Start) if m.scram().is_some() => {
                let nonce = self
                    .nonce
                    .clone()
                    .unwrap_or_else(|| STANDARD.encode(rand::random::<[u8; 18]>()));
                let bare = format!("n={},r={nonce}", saslname(user));
                let header = if m.plus() {
                    ensure!(
                        !self.credentials.binding.is_empty(),
                        "The connection's channel binding isn't known."
                    );
                    "p=tls-server-end-point,,".to_owned()
                } else {
                    // Not "y,,": servers behind a TLS-ending proxy offer no
                    // -PLUS and would refuse a client that says it could.
                    "n,,".to_owned()
                };
                (
                    format!("{header}{bare}").into_bytes(),
                    State::ScramFirst { bare, nonce, header },
                )
            }
            (m, State::ScramFirst { bare, nonce, header }) if m.scram().is_some() => {
                let hash = m.scram().expect("a SCRAM mechanism");
                let mut binding = header.into_bytes();
                if m.plus() {
                    binding.extend_from_slice(&self.credentials.binding);
                }
                let (answer, signature) =
                    scram_proof(hash, secret, &bare, &nonce, challenge, &binding)?;
                (answer.into_bytes(), State::ScramFinal { signature })
            }
            (m, State::ScramFinal { signature }) if m.scram().is_some() => {
                let text = std::str::from_utf8(challenge)
                    .context("The server's SCRAM reply isn't text.")?;
                let fields = scram_fields(text);
                if let Some(error) = fields.iter().find(|(k, _)| *k == 'e') {
                    bail!("The server refused the sign-in ({}).", error.1);
                }
                let v = fields
                    .iter()
                    .find(|(k, _)| *k == 'v')
                    .context("The server's SCRAM reply has no signature.")?;
                let theirs = STANDARD
                    .decode(v.1)
                    .context("The server's SCRAM signature isn't base64.")?;
                ensure!(
                    theirs == signature,
                    "The server couldn't prove it knows your password (SCRAM signature mismatch); not signed in."
                );
                (Vec::new(), State::Done)
            }
            (_, state) => {
                self.state = state;
                bail!(
                    "The server sent an unexpected {} challenge.",
                    self.mechanism.name()
                );
            }
        };
        self.state = next;
        Ok(answer)
    }
    /// Whether the exchange is somewhere the server can rightly accept it.
    /// GSSAPI must have finished its security-layer exchange, which is
    /// what proves the server holds the service's key.
    fn unfinished_gss(&self) -> bool {
        matches!(self.state, State::GssContext(_) | State::GssLayer(_))
    }
    /// After the server accepted: for SCRAM, that it also proved itself.
    /// Servers may send their signature with the success reply instead of
    /// as a challenge; `success` is that reply's data, if any.
    pub(crate) fn finish(&mut self, success: Option<&[u8]>) -> Result<()> {
        if let (State::ScramFinal { .. }, Some(data)) = (&self.state, success) {
            self.step(data)?;
        }
        match &self.state {
            State::ScramFinal { .. } => bail!(
                "The server didn't prove it knows your password (no SCRAM signature); not signed in."
            ),
            _ if self.unfinished_gss() => bail!(
                "The server accepted the Kerberos sign-in before proving who it is; not signed in."
            ),
            _ => Ok(()),
        }
    }
    /// Why a token was refused, when the server said (OAUTHBEARER/XOAUTH2).
    pub(crate) fn refusal(&self) -> Option<&str> {
        match &self.state {
            State::Refused(reason) if !reason.is_empty() => Some(reason),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Hash {
    Sha1,
    Sha256,
}
impl Hash {
    fn hmac(self, key: &[u8], data: &[u8]) -> Vec<u8> {
        match self {
            Hash::Sha1 => {
                let mut m =
                    <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(key).expect("any key length");
                m.update(data);
                m.finalize().into_bytes().to_vec()
            }
            Hash::Sha256 => {
                let mut m =
                    <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(key).expect("any key length");
                m.update(data);
                m.finalize().into_bytes().to_vec()
            }
        }
    }
    fn digest(self, data: &[u8]) -> Vec<u8> {
        use sha2::Digest as _;
        match self {
            Hash::Sha1 => sha1::Sha1::digest(data).to_vec(),
            Hash::Sha256 => sha2::Sha256::digest(data).to_vec(),
        }
    }
    /// PBKDF2 with one output block, as SCRAM's Hi() is.
    fn hi(self, password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
        let mut block = salt.to_vec();
        block.extend_from_slice(&1u32.to_be_bytes());
        let mut u = self.hmac(password, &block);
        let mut out = u.clone();
        for _ in 1..iterations {
            u = self.hmac(password, &u);
            out.iter_mut().zip(&u).for_each(|(o, x)| *o ^= x);
        }
        out
    }
}

fn hmac_md5(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = <Hmac<md5::Md5> as KeyInit>::new_from_slice(key).expect("any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A name as SCRAM writes it: "=" and "," escaped.
fn saslname(name: &str) -> String {
    name.replace('=', "=3D").replace(',', "=2C")
}
/// A name in a GS2 header (OAUTHBEARER's "a="), escaped the same way.
fn gs2_name(name: &str) -> String {
    saslname(name)
}

/// SCRAM's attribute=value fields.
fn scram_fields(text: &str) -> Vec<(char, &str)> {
    text.split(',')
        .filter_map(|field| {
            let mut chars = field.chars();
            let key = chars.next()?;
            let value = field.get(key.len_utf8()..)?.strip_prefix('=')?;
            Some((key, value))
        })
        .collect()
}

/// The client's final message for the server's first one, and the server
/// signature to expect back.
fn scram_proof(
    hash: Hash,
    password: &str,
    client_first_bare: &str,
    nonce: &str,
    server_first: &[u8],
    binding: &[u8],
) -> Result<(String, Vec<u8>)> {
    let server_first =
        std::str::from_utf8(server_first).context("The server's SCRAM challenge isn't text.")?;
    let fields = scram_fields(server_first);
    let get = |key: char| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    if let Some(error) = get('e') {
        bail!("The server refused the sign-in ({error}).");
    }
    ensure!(
        get('m').is_none(),
        "The server asked for a SCRAM extension Picacho doesn't know."
    );
    let combined = get('r').context("The server's SCRAM challenge has no nonce.")?;
    ensure!(
        combined.starts_with(nonce) && combined.len() > nonce.len(),
        "The server's SCRAM nonce doesn't extend Picacho's; not signed in."
    );
    let salt = STANDARD
        .decode(get('s').context("The server's SCRAM challenge has no salt.")?)
        .context("The server's SCRAM salt isn't base64.")?;
    let iterations: u32 = get('i')
        .context("The server's SCRAM challenge has no iteration count.")?
        .parse()
        .context("The server's SCRAM iteration count isn't a number.")?;
    ensure!(
        (1..=MAX_ITERATIONS).contains(&iterations),
        "The server asked for {iterations} SCRAM iterations; Picacho allows up to {MAX_ITERATIONS}."
    );
    use unicode_normalization::UnicodeNormalization as _;
    // SASLprep, as far as it matters for passwords people type: NFKC.
    let password: String = password.nfkc().collect();
    let salted = hash.hi(password.as_bytes(), &salt, iterations);
    let client_key = hash.hmac(&salted, b"Client Key");
    let stored_key = hash.digest(&client_key);
    // The GS2 header and, for -PLUS, the binding data ("biws" is "n,,").
    let without_proof = format!("c={},r={combined}", STANDARD.encode(binding));
    let auth_message = format!("{client_first_bare},{server_first},{without_proof}");
    let signature = hash.hmac(&stored_key, auth_message.as_bytes());
    let proof: Vec<u8> = client_key
        .iter()
        .zip(&signature)
        .map(|(k, s)| k ^ s)
        .collect();
    let server_key = hash.hmac(&salted, b"Server Key");
    let server_signature = hash.hmac(&server_key, auth_message.as_bytes());
    Ok((
        format!("{without_proof},p={}", STANDARD.encode(proof)),
        server_signature,
    ))
}

/// The reason in an OAUTHBEARER/XOAUTH2 error challenge, which is JSON
/// like `{"status":"invalid_token"}`.
fn token_error(challenge: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(challenge)
        .ok()
        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// Base64 for the wire.
pub(crate) fn encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}
/// A base64 challenge from the wire; an empty one (or "=") is no data.
pub(crate) fn decode(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || text == "=" {
        return Ok(Vec::new());
    }
    STANDARD
        .decode(text)
        .context("The server's sign-in challenge isn't base64.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(user: &str, secret: &str) -> Credentials {
        Credentials {
            user: user.into(),
            secret: secret.into(),
            host: "mail.example.com".into(),
            port: 993,
            binding: Vec::new(),
            service: "imap",
        }
    }

    #[test]
    fn the_best_offered_mechanism_is_chosen() {
        let offered = ["plain", "LOGIN", "CRAM-MD5", "SCRAM-SHA-1"];
        assert_eq!(
            Mechanism::choose(&PASSWORD_ORDER, offered),
            Some(Mechanism::ScramSha1)
        );
        assert_eq!(
            Mechanism::choose(&PASSWORD_ORDER, ["CRAM-MD5", "LOGIN"]),
            Some(Mechanism::Login)
        );
        assert_eq!(
            Mechanism::choose(&PASSWORD_ORDER, ["CRAM-MD5", "GSSAPI"]),
            Some(Mechanism::CramMd5)
        );
        assert_eq!(
            Mechanism::choose(&TOKEN_ORDER, ["XOAUTH2", "OAUTHBEARER"]),
            Some(Mechanism::OAuthBearer)
        );
        assert_eq!(Mechanism::choose(&TOKEN_ORDER, ["PLAIN"]), None);
    }

    /// RFC 5802, section 5.
    #[test]
    fn scram_sha_1_matches_rfc_5802() -> Result<()> {
        let mut x = Exchange::new(Mechanism::ScramSha1, credentials("user", "pencil"));
        x.nonce = Some("fyko+d2lbbFgONRv9qkxdawL".into());
        assert_eq!(
            x.initial().unwrap(),
            b"n,,n=user,r=fyko+d2lbbFgONRv9qkxdawL"
        );
        let answer =
            x.step(b"r=fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j,s=QSXCR+Q6sek8bf92,i=4096")?;
        assert_eq!(
            answer,
            b"c=biws,r=fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j,p=v0X8v3Bz2T0CJGbJQyF0X+HI4Ts="
        );
        assert!(x.step(b"v=rmF9pqV8S7suAoZWja4dJRkFsKQ=")?.is_empty());
        x.finish(None)
    }

    /// RFC 7677, section 3.
    #[test]
    fn scram_sha_256_matches_rfc_7677_and_checks_the_server() -> Result<()> {
        let start = || {
            let mut x = Exchange::new(Mechanism::ScramSha256, credentials("user", "pencil"));
            x.nonce = Some("rOprNGfwEbeRWgbNEkqO".into());
            assert_eq!(x.initial().unwrap(), b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
            let answer = x
                .step(b"r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")
                .unwrap();
            assert_eq!(
                answer,
                b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=".to_vec()
            );
            x
        };
        let mut x = start();
        x.finish(Some(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="))?;
        // A server that doesn't know the password can't sign.
        let mut x = start();
        assert!(
            x.step(b"v=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
                .is_err()
        );
        // Nor can one that never answers with a signature pass.
        let mut x = start();
        assert!(x.finish(None).is_err());
        // A nonce that doesn't extend ours is refused.
        let mut x = Exchange::new(Mechanism::ScramSha256, credentials("user", "pencil"));
        x.nonce = Some("abc".into());
        x.initial();
        assert!(x.step(b"r=xyz123,s=QSXCR+Q6sek8bf92,i=4096").is_err());
        Ok(())
    }

    #[test]
    fn channel_binding_is_used_only_when_known() -> Result<()> {
        let offered = ["SCRAM-SHA-256", "SCRAM-SHA-256-PLUS", "PLAIN"];
        assert_eq!(
            Mechanism::choose(&PASSWORD_ORDER, offered),
            Some(Mechanism::ScramSha256)
        );
        assert_eq!(
            Mechanism::choose_bound(&PASSWORD_ORDER, offered, true),
            Some(Mechanism::ScramSha256Plus)
        );
        // The same exchange as RFC 7677's, bound to a certificate hash: the
        // header names the binding, and c= carries header and hash.
        let mut c = credentials("user", "pencil");
        c.binding = vec![0xAB; 32];
        let mut x = Exchange::new(Mechanism::ScramSha256Plus, c);
        x.nonce = Some("rOprNGfwEbeRWgbNEkqO".into());
        assert_eq!(
            x.initial().unwrap(),
            b"p=tls-server-end-point,,n=user,r=rOprNGfwEbeRWgbNEkqO"
        );
        let answer = String::from_utf8(x.step(
            b"r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
        )?)?;
        let mut expected = b"p=tls-server-end-point,,".to_vec();
        expected.extend([0xAB; 32]);
        assert!(answer.starts_with(&format!("c={},r=", STANDARD.encode(&expected))));
        // Without a binding, -PLUS can't start.
        let mut x = Exchange::new(Mechanism::ScramSha1Plus, credentials("user", "pencil"));
        assert!(x.initial().is_none());
        Ok(())
    }

    /// RFC 2195's example.
    #[test]
    fn cram_md5_matches_rfc_2195() -> Result<()> {
        let mut x = Exchange::new(Mechanism::CramMd5, credentials("tim", "tanstaaftanstaaf"));
        assert!(x.initial().is_none());
        let answer = x.step(b"<1896.697170952@postoffice.reston.mci.net>")?;
        assert_eq!(answer, b"tim b913a602c7eda7a495b4e6e7334d3890");
        Ok(())
    }

    #[test]
    fn plain_login_and_token_messages() -> Result<()> {
        let mut x = Exchange::new(Mechanism::Plain, credentials("ana", "secret"));
        assert_eq!(x.initial().unwrap(), b"\0ana\0secret");
        let mut x = Exchange::new(Mechanism::Login, credentials("ana", "secret"));
        assert!(x.initial().is_none());
        assert_eq!(x.step(b"Username:")?, b"ana");
        assert_eq!(x.step(b"Password:")?, b"secret");
        let mut x = Exchange::new(
            Mechanism::OAuthBearer,
            credentials("ana@example.com", "tok"),
        );
        assert_eq!(
            x.initial().unwrap(),
            b"n,a=ana@example.com,\x01host=mail.example.com\x01port=993\x01auth=Bearer tok\x01\x01"
        );
        // An error challenge is answered with ^A, and its reason kept.
        assert_eq!(x.step(br#"{"status":"invalid_token"}"#)?, b"\x01");
        assert_eq!(x.refusal(), Some("invalid_token"));
        let mut x = Exchange::new(Mechanism::XOAuth2, credentials("ana@example.com", "tok"));
        assert_eq!(
            x.initial().unwrap(),
            b"user=ana@example.com\x01auth=Bearer tok\x01\x01"
        );
        assert_eq!(saslname("a=b,c"), "a=3Db=2Cc");
        Ok(())
    }

    /// A GSS-API context standing in for Kerberos: two round trips, then
    /// "wrapping" is a marker byte in front.
    pub(crate) struct FakeGss {
        pub(crate) steps: usize,
        pub(crate) mutual: bool,
    }
    impl crate::sync::gss::Context for FakeGss {
        fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)> {
            self.steps += 1;
            match self.steps {
                1 => {
                    assert!(input.is_empty());
                    Ok((b"ap-req".to_vec(), false))
                }
                _ => {
                    assert_eq!(input, b"ap-rep");
                    ensure!(self.mutual, "no mutual authentication");
                    Ok((Vec::new(), true))
                }
            }
        }
        fn unwrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            Ok(message.strip_prefix(b"W").context("not wrapped")?.to_vec())
        }
        fn wrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            Ok([b"W".as_slice(), message].concat())
        }
    }
    pub(crate) fn kerberos_exchange(user: &str, mutual: bool) -> Exchange {
        let mut x = Exchange::new(Mechanism::Gssapi, credentials(user, ""));
        x.gss = Some(Box::new(FakeGss { steps: 0, mutual }));
        x
    }

    #[test]
    fn gssapi_follows_rfc_4752() -> Result<()> {
        // Chosen only when asked for, never among passwords' mechanisms.
        assert_eq!(
            Mechanism::choose(&PASSWORD_ORDER, ["GSSAPI", "PLAIN"]),
            Some(Mechanism::Plain)
        );
        assert_eq!(
            Mechanism::choose(&KERBEROS_ORDER, ["PLAIN", "gssapi"]),
            Some(Mechanism::Gssapi)
        );
        assert_eq!(
            Mechanism::choose(&KERBEROS_ORDER, ["PLAIN", "SCRAM-SHA-256"]),
            None
        );
        let mut x = kerberos_exchange("ana", true);
        // The context's first token goes with the command.
        assert_eq!(x.initial().unwrap(), b"ap-req");
        // The server's reply completes the context; the client sends nothing.
        assert!(x.step(b"ap-rep")?.is_empty());
        // Accepting now would be too early: the layer exchange is unfinished.
        assert!(x.unfinished_gss());
        // The server offers "no security layer" (bit 1) and a 64 KiB maximum.
        let answer = x.step(b"W\x01\x00\x10\x00")?;
        assert_eq!(answer, b"W\x01\x00\x00\x00ana");
        x.finish(None)?;
        // A server that only offers integrity or privacy layers is refused.
        let mut x = kerberos_exchange("ana", true);
        x.initial();
        x.step(b"ap-rep")?;
        assert!(x.step(b"W\x06\x00\x10\x00").is_err());
        // A server that can't prove itself (no mutual authentication) fails.
        let mut x = kerberos_exchange("ana", false);
        x.initial();
        assert!(x.step(b"ap-rep").is_err());
        // A server accepting before the layer exchange isn't trusted.
        let mut x = kerberos_exchange("ana", true);
        x.initial();
        x.step(b"ap-rep")?;
        assert!(x.finish(None).is_err());
        Ok(())
    }
}
