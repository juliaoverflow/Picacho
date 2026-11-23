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
    ScramSha512Plus,
    ScramSha512,
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
    /// GS2-KRB5 (RFC 5801): Kerberos through the GS2 family, with the TLS
    /// channel binding in its -PLUS form.
    Gs2Krb5Plus,
    Gs2Krb5,
}

/// For a password, best first: SCRAM proves both sides without sending the
/// password; PLAIN and LOGIN send it inside TLS; CRAM-MD5 last.
pub(crate) const PASSWORD_ORDER: [Mechanism; 9] = [
    Mechanism::ScramSha512Plus,
    Mechanism::ScramSha256Plus,
    Mechanism::ScramSha1Plus,
    Mechanism::ScramSha512,
    Mechanism::ScramSha256,
    Mechanism::ScramSha1,
    Mechanism::Plain,
    Mechanism::Login,
    Mechanism::CramMd5,
];
/// SCRAM alone, best first (IMAP and POP3 prefer it to plain LOGIN/USER).
pub(crate) const SCRAM_ORDER: [Mechanism; 6] = [
    Mechanism::ScramSha512Plus,
    Mechanism::ScramSha256Plus,
    Mechanism::ScramSha1Plus,
    Mechanism::ScramSha512,
    Mechanism::ScramSha256,
    Mechanism::ScramSha1,
];
/// For an access token: the standard mechanism, then the older one.
pub(crate) const TOKEN_ORDER: [Mechanism; 2] = [Mechanism::OAuthBearer, Mechanism::XOAuth2];
/// For a Kerberos ticket: GS2-KRB5-PLUS where the channel binding is
/// known (the ticket can't be relayed onto another connection), then
/// GSSAPI, then GS2-KRB5. An account set to Kerberos never falls back to a
/// password.
pub(crate) const KERBEROS_ORDER: [Mechanism; 3] =
    [Mechanism::Gs2Krb5Plus, Mechanism::Gssapi, Mechanism::Gs2Krb5];

impl Mechanism {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ScramSha512Plus => "SCRAM-SHA-512-PLUS",
            Self::ScramSha512 => "SCRAM-SHA-512",
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
            Self::Gs2Krb5Plus => "GS2-KRB5-PLUS",
            Self::Gs2Krb5 => "GS2-KRB5",
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
        matches!(
            self,
            Self::ScramSha512Plus | Self::ScramSha256Plus | Self::ScramSha1Plus | Self::Gs2Krb5Plus
        )
    }
    fn gs2(self) -> bool {
        matches!(self, Self::Gs2Krb5 | Self::Gs2Krb5Plus)
    }
    fn scram(self) -> Option<Hash> {
        match self {
            Self::ScramSha512 | Self::ScramSha512Plus => Some(Hash::Sha512),
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
    /// GSSAPI: an empty challenge was answered; only the offer may follow.
    GssOffer(Box<dyn crate::sync::gss::Context>),
    /// GSSAPI: established; the server's security-layer offer is next.
    GssLayer(Box<dyn crate::sync::gss::Context>),
    /// GS2-KRB5: the context is being established.
    Gs2Context(Box<dyn crate::sync::gss::Context>),
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
    /// Challenges answered so far (see `MAX_STEPS`).
    steps: u32,
    /// The identity to act as, when not the signed-in user's own (a shared
    /// mailbox): SCRAM's `a=` (RFC 5802 §5.1) and PLAIN's authzid (RFC
    /// 4616 §2). Empty: the user's own, and the messages are as without.
    authzid: String,
}

/// The most challenges one sign-in answers. Every mechanism here finishes
/// in a few (Kerberos in the most, a handful of context tokens); a server
/// that kept sending them (empty ones, which GSSAPI answers with nothing)
/// held the sign-in, each reply restarting the connection's time and size
/// budgets, for good.
const MAX_STEPS: u32 = 16;

impl Exchange {
    pub(crate) fn new(mechanism: Mechanism, credentials: Credentials) -> Self {
        Self {
            mechanism,
            credentials,
            state: State::Start,
            nonce: None,
            gss: None,
            steps: 0,
            authzid: String::new(),
        }
    }
    /// The exchange, acting as `authzid` (see `Exchange::authzid`). No
    /// account sets one yet: that needs a setting of its own.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn acting_as(mut self, authzid: &str) -> Self {
        self.authzid = authzid.trim().to_owned();
        self
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
        self.steps += 1;
        ensure!(
            self.steps <= MAX_STEPS,
            "The server kept asking for more during sign-in."
        );
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
            (Mechanism::Plain, State::Start) => (
                format!("{}\0{user}\0{secret}", self.authzid).into_bytes(),
                State::Done,
            ),
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
            // RFC 5801 §4: the GS2 header, then the context's first
            // token without its RFC 2743 §3.1 framing; the header and the
            // TLS binding go into the context's channel bindings (§5).
            (m, State::Start) if m.gs2() => {
                let authzid = if self.authzid.is_empty() {
                    String::new()
                } else {
                    format!("a={}", saslname(&self.authzid))
                };
                let header = if m.plus() {
                    ensure!(
                        !self.credentials.binding.is_empty(),
                        "The connection's channel binding isn't known."
                    );
                    format!("p=tls-server-end-point,{authzid},")
                } else {
                    format!("n,{authzid},")
                };
                let mut bindings = header.clone().into_bytes();
                if m.plus() {
                    bindings.extend_from_slice(&self.credentials.binding);
                }
                let mut context = match self.gss.take() {
                    Some(context) => context,
                    None => crate::sync::gss::start_bound(&format!("{service}@{host}"), bindings)?,
                };
                let (token, done) = context.step(&[])?;
                let mut message = header.into_bytes();
                message.extend_from_slice(unframed(&token)?);
                (message, if done { State::Done } else { State::Gs2Context(context) })
            }
            (m, State::Gs2Context(mut context)) if m.gs2() => {
                let (token, done) = context.step(challenge)?;
                (token, if done { State::Done } else { State::Gs2Context(context) })
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
            (Mechanism::Gssapi, State::GssOffer(_)) if challenge.is_empty() => {
                bail!("The server's Kerberos sign-in didn't offer its security layers.")
            }
            (Mechanism::Gssapi, State::GssLayer(mut context) | State::GssOffer(mut context)) => {
                if challenge.is_empty() {
                    // The context's last token was ours; the server sends
                    // nothing, and is answered with nothing (once: the
                    // offer comes next).
                    (Vec::new(), State::GssOffer(context))
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
                    // different sign-in name is set (authorization identity):
                    // empty asks for the ticket's own (RFC 4752 §3.1).
                    // An authorization identity set for the exchange
                    // takes the place of the sign-in name.
                    let acting = if self.authzid.is_empty() { user.trim() } else { &self.authzid };
                    answer.extend_from_slice(acting.as_bytes());
                    (context.wrap(&answer)?, State::Done)
                }
            }
            (m, State::Start) if m.scram().is_some() => {
                let nonce = self
                    .nonce
                    .clone()
                    .unwrap_or_else(|| STANDARD.encode(rand::random::<[u8; 18]>()));
                let bare = format!("n={},r={nonce}", saslname(user));
                // RFC 5802 §7: gs2-header = gs2-cbind-flag "," [authzid] ",",
                // with authzid = "a=" saslname.
                let authzid = if self.authzid.is_empty() {
                    String::new()
                } else {
                    format!("a={}", saslname(&self.authzid))
                };
                let header = if m.plus() {
                    ensure!(
                        !self.credentials.binding.is_empty(),
                        "The connection's channel binding isn't known."
                    );
                    format!("p=tls-server-end-point,{authzid},")
                } else {
                    // Not "y,,": servers behind a TLS-ending proxy offer no
                    // -PLUS and would refuse a client that says it could.
                    format!("n,{authzid},")
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
                // Compared in constant time: how long it takes says nothing
                // about how much of a forged signature matched.
                let same = theirs.len() == signature.len()
                    && theirs.iter().zip(&signature).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
                ensure!(
                    same,
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
        matches!(
            self.state,
            State::GssContext(_) | State::GssLayer(_) | State::GssOffer(_) | State::Gs2Context(_)
        )
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
    Sha512,
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
            Hash::Sha512 => {
                let mut m =
                    <Hmac<sha2::Sha512> as KeyInit>::new_from_slice(key).expect("any key length");
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
            Hash::Sha512 => sha2::Sha512::digest(data).to_vec(),
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

/// A GSS-API initial context token without its framing (RFC 2743 §3.1:
/// 0x60, a DER length, then the mechanism's OID, then the inner token),
/// as GS2 sends it (RFC 5801 §4). Kerberos' must name Kerberos 5's OID.
fn unframed(token: &[u8]) -> Result<&[u8]> {
    const KRB5_OID: [u8; 11] = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02];
    let malformed = || anyhow::anyhow!("Kerberos made a malformed first token.");
    let rest = token.strip_prefix(&[0x60]).ok_or_else(malformed)?;
    let (&first, rest) = rest.split_first().ok_or_else(malformed)?;
    // DER: a short length, or 0x8n and n big-endian bytes of it.
    let (length, rest) = if first < 0x80 {
        (first as usize, rest)
    } else {
        let n = (first & 0x7f) as usize;
        ensure!((1..=4).contains(&n) && rest.len() >= n, malformed());
        let length = rest[..n].iter().fold(0usize, |acc, b| (acc << 8) | *b as usize);
        (length, &rest[n..])
    };
    ensure!(length == rest.len(), malformed());
    rest.strip_prefix(&KRB5_OID).ok_or_else(malformed)
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
    // SASLprep (RFC 4013): non-ASCII spaces as a space, characters mapped
    // to nothing (soft hyphens, joiners) left out, then NFKC; prohibited
    // characters (controls) refused.
    let password: String = saslprep(password)?;
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

/// SASLprep (RFC 4013 §2) of a password: mapping, NFKC, and the
/// prohibited characters refused.
fn saslprep(text: &str) -> Result<String> {
    use unicode_normalization::UnicodeNormalization as _;
    let mapped: String = text
        .chars()
        // B.1: mapped to nothing.
        .filter(|c| !matches!(*c, '\u{00AD}' | '\u{034F}' | '\u{1806}' | '\u{180B}'..='\u{180D}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'))
        // C.1.2: non-ASCII spaces as a space.
        .map(|c| if c != ' ' && c.is_whitespace() && !c.is_ascii() { ' ' } else { c })
        .collect();
    let normalized: String = mapped.nfkc().collect();
    ensure!(
        !normalized.chars().any(|c| c.is_control() || ('\u{E000}'..='\u{F8FF}').contains(&c) || ('\u{FFF9}'..='\u{FFFD}').contains(&c)),
        "The password has a character SCRAM sign-in can't use."
    );
    Ok(normalized)
}

/// The reason in an OAUTHBEARER/XOAUTH2 error challenge, which is JSON
/// like `{"status":"invalid_token"}`.
fn token_error(challenge: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(challenge) else {
        return String::new();
    };
    let status = v["status"].as_str().unwrap_or_default().to_owned();
    // RFC 7628 §3.2.2: the scope the token needs, said with the status
    // (`insufficient_scope`), is worth telling.
    match v["scope"].as_str().filter(|s| !s.is_empty()) {
        Some(scope) => format!("{status}; it needs: {scope}"),
        None => status,
    }
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

/// The server's side of SCRAM-SHA-256 (RFC 5802 §5), for mock servers in
/// tests: it checks the client's proof and signs as a real server would.
#[cfg(test)]
pub(crate) struct ScramServer {
    pub(crate) password: String,
    /// The server's signature is made with this password instead, where
    /// set: a server that doesn't know the password.
    pub(crate) forged: Option<String>,
    first_bare: String,
    server_first: String,
}
#[cfg(test)]
impl ScramServer {
    const SALT: &'static [u8] = b"mock-salt";
    const ITERATIONS: u32 = 4096;
    pub(crate) fn new(password: &str) -> Self {
        Self {
            password: password.into(),
            forged: None,
            first_bare: String::new(),
            server_first: String::new(),
        }
    }
    /// The server-first-message for the client's first message.
    pub(crate) fn first(&mut self, client_first: &[u8]) -> Vec<u8> {
        let text = String::from_utf8(client_first.to_vec()).unwrap();
        let bare = text.strip_prefix("n,,").expect("a GS2 header without binding");
        let nonce = scram_fields(bare).into_iter().find(|(k, _)| *k == 'r').unwrap().1;
        self.first_bare = bare.to_owned();
        self.server_first = format!(
            "r={nonce}server,s={},i={}",
            STANDARD.encode(Self::SALT),
            Self::ITERATIONS
        );
        self.server_first.clone().into_bytes()
    }
    /// The server-final-message (`v=`) for the client's final one, or
    /// `None` where its proof is wrong.
    pub(crate) fn last(&self, client_final: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(client_final).ok()?;
        let (without_proof, proof) = text.rsplit_once(",p=")?;
        let auth = format!("{},{},{without_proof}", self.first_bare, self.server_first);
        let hash = Hash::Sha256;
        let salted = |password: &str| hash.hi(password.as_bytes(), Self::SALT, Self::ITERATIONS);
        let client_key = hash.hmac(&salted(&self.password), b"Client Key");
        let signature = hash.hmac(&hash.digest(&client_key), auth.as_bytes());
        let expected: Vec<u8> = client_key.iter().zip(&signature).map(|(k, s)| k ^ s).collect();
        if STANDARD.decode(proof).ok()? != expected {
            return None;
        }
        let signer = self.forged.as_deref().unwrap_or(&self.password);
        let server_key = hash.hmac(&salted(signer), b"Server Key");
        Some(format!("v={}", STANDARD.encode(hash.hmac(&server_key, auth.as_bytes()))).into_bytes())
    }
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

    /// RFC 5802 §5.1 and RFC 4616 §2: an authorization identity, escaped,
    /// in SCRAM's GS2 header (and so in its binding data, `c=`) and as
    /// PLAIN's first field; none leaves the messages as before.
    #[test]
    fn authzid_is_sent_only_when_set() -> Result<()> {
        // None: the RFC 7677 exchange byte for byte (`n,,`, `c=biws`).
        let mut x = Exchange::new(Mechanism::ScramSha256, credentials("user", "pencil")).acting_as("");
        x.nonce = Some("rOprNGfwEbeRWgbNEkqO".into());
        assert_eq!(x.initial().unwrap(), b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
        let answer = x.step(b"r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096")?;
        assert_eq!(answer, b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=".to_vec());
        x.finish(Some(b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="))?;
        // Set: `a=` with `=` and `,` escaped, and the header in `c=`.
        let mut x = Exchange::new(Mechanism::ScramSha256, credentials("user", "pencil")).acting_as("shared=box,1");
        x.nonce = Some("abc".into());
        assert_eq!(x.initial().unwrap(), b"n,a=shared=3Dbox=2C1,n=user,r=abc");
        let answer = String::from_utf8(x.step(b"r=abcdef,s=QSXCR+Q6sek8bf92,i=4096")?)?;
        assert!(answer.starts_with(&format!("c={},", STANDARD.encode("n,a=shared=3Dbox=2C1,"))), "{answer}");
        // With channel binding the header is the binding's, with `a=`.
        let mut c = credentials("user", "pencil");
        c.binding = vec![7; 32];
        let mut x = Exchange::new(Mechanism::ScramSha256Plus, c).acting_as("shared");
        x.nonce = Some("abc".into());
        assert_eq!(x.initial().unwrap(), b"p=tls-server-end-point,a=shared,n=user,r=abc");
        // PLAIN: authzid NUL authcid NUL password.
        let mut x = Exchange::new(Mechanism::Plain, credentials("ana", "secret")).acting_as("shared");
        assert_eq!(x.initial().unwrap(), b"shared\0ana\0secret");
        let mut x = Exchange::new(Mechanism::Plain, credentials("ana", "secret"));
        assert_eq!(x.initial().unwrap(), b"\0ana\0secret");
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

    /// RFC 5801 §4: GS2-KRB5's first message is the GS2 header and the
    /// token without its RFC 2743 framing; the context then completes with
    /// no security-layer exchange.
    #[test]
    fn gs2_krb5_follows_rfc_5801() -> Result<()> {
        // Chosen over GSSAPI only with a channel binding to give.
        assert_eq!(Mechanism::choose(&KERBEROS_ORDER, ["GSSAPI", "GS2-KRB5-PLUS"]), Some(Mechanism::Gssapi));
        assert_eq!(
            Mechanism::choose_bound(&KERBEROS_ORDER, ["GSSAPI", "GS2-KRB5-PLUS"], true),
            Some(Mechanism::Gs2Krb5Plus)
        );
        assert_eq!(Mechanism::choose(&KERBEROS_ORDER, ["gs2-krb5"]), Some(Mechanism::Gs2Krb5));
        // Framing: 0x60, length, Kerberos 5's OID, inner token.
        let inner = b"\x01\x00ap-req";
        let mut framed = vec![0x60, (11 + inner.len()) as u8];
        framed.extend([0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02]);
        framed.extend(inner);
        assert_eq!(unframed(&framed)?, inner);
        let mut long = vec![0x60, 0x81, (11 + inner.len()) as u8];
        long.extend(&framed[2..]);
        assert_eq!(unframed(&long)?, inner);
        assert!(unframed(b"ap-req").is_err());
        assert!(unframed(&framed[..framed.len() - 1]).is_err());
        let mut other_mech = framed.clone();
        other_mech[12] = 0x03;
        assert!(unframed(&other_mech).is_err());
        /// A context whose first token is `framed` and which completes on
        /// the server's reply.
        struct Framed(Vec<u8>, bool);
        impl crate::sync::gss::Context for Framed {
            fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)> {
                if input.is_empty() {
                    return Ok((self.0.clone(), false));
                }
                assert_eq!(input, b"ap-rep");
                ensure!(self.1, "no mutual authentication");
                Ok((Vec::new(), true))
            }
            fn unwrap(&mut self, _: &[u8]) -> Result<Vec<u8>> {
                bail!("GS2 has no security layer")
            }
            fn wrap(&mut self, _: &[u8]) -> Result<Vec<u8>> {
                bail!("GS2 has no security layer")
            }
        }
        let mut x = Exchange::new(Mechanism::Gs2Krb5, credentials("ana", ""));
        x.gss = Some(Box::new(Framed(framed.clone(), true)));
        assert_eq!(x.initial().unwrap(), [b"n,,".as_slice(), inner].concat());
        assert!(x.unfinished_gss());
        assert!(x.step(b"ap-rep")?.is_empty());
        x.finish(None)?;
        // With a binding and an authorization identity: their header.
        let mut c = credentials("ana", "");
        c.binding = vec![9; 32];
        let mut x = Exchange::new(Mechanism::Gs2Krb5Plus, c).acting_as("shared");
        x.gss = Some(Box::new(Framed(framed.clone(), true)));
        assert_eq!(x.initial().unwrap(), [b"p=tls-server-end-point,a=shared,".as_slice(), inner].concat());
        // A server that can't prove itself, or accepts too early, fails.
        let mut x = Exchange::new(Mechanism::Gs2Krb5, credentials("ana", ""));
        x.gss = Some(Box::new(Framed(framed.clone(), false)));
        x.initial();
        assert!(x.step(b"ap-rep").is_err());
        let mut x = Exchange::new(Mechanism::Gs2Krb5, credentials("ana", ""));
        x.gss = Some(Box::new(Framed(framed, true)));
        x.initial();
        assert!(x.finish(None).is_err());
        Ok(())
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
        // One empty challenge is answered; the offer must come next, not
        // empty challenges without end.
        let mut x = kerberos_exchange("ana", true);
        x.initial();
        x.step(b"ap-rep")?;
        assert!(x.step(b"")?.is_empty());
        assert!(x.step(b"").is_err());
        let mut x = kerberos_exchange("ana", true);
        x.initial();
        x.step(b"ap-rep")?;
        x.step(b"")?;
        assert_eq!(x.step(b"W\x01\x00\x10\x00")?, b"W\x01\x00\x00\x00ana");
        // An authorization identity is the one asked for instead.
        let mut x = kerberos_exchange("ana", true).acting_as("shared");
        x.initial();
        x.step(b"ap-rep")?;
        assert_eq!(x.step(b"W\x01\x00\x10\x00")?, b"W\x01\x00\x00\x00shared");
        Ok(())
    }
}
