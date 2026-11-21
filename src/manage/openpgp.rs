//! OpenPGP for every account: your own keys and the keys of people you
//! write to, kept in the system keychain; PGP/MIME messages (RFC 3156)
//! decrypted and their signatures checked when read; and messages signed
//! and encrypted when sent.
//!
//! Pinal Mail's zero-access encryption (`crypto.rs`) is separate: its keys
//! come with the account. Here the person imports keys exported from GnuPG,
//! Thunderbird, Proton or another app, as armored text. Secret keys must
//! have no passphrase, or have it given once at import: they are kept
//! unprotected in the keychain, as the keychain itself protects them.
use crate::manage::store;
use anyhow::{Context, Result, bail, ensure};
use pgp::composed::{
    ArmorOptions, Deserializable, DetachedSignature, Message, MessageBuilder, SignedPublicKey,
    SignedSecretKey,
};
use pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
use pgp::types::{KeyDetails, Password};
use serde::{Deserialize, Serialize};

/// The keychain entry holding the keyring.
#[cfg_attr(test, allow(dead_code))]
const KEYRING: &str = "openpgp:keyring";

/// The keys Picacho knows, armored.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Keyring {
    /// Your own keys (transferable secret keys), each with no passphrase.
    #[serde(default)]
    pub secret: Vec<String>,
    /// Other people's public keys.
    #[serde(default)]
    pub public: Vec<String>,
    /// Fingerprints of public keys Picacho found itself (WKD,
    /// keys.openpgp.org, Autocrypt) rather than ones imported by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub found: Vec<String>,
    /// Fingerprints of found keys you compared with their owner and marked
    /// as checked: they no longer count as unverified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified: Vec<String>,
}

/// One key, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    pub fingerprint: String,
    /// The key's user ids ("Ana <ana@example.org>").
    pub users: Vec<String>,
    pub secret: bool,
    /// Found automatically rather than imported.
    pub found: bool,
    /// A found key you compared and marked as checked.
    pub verified: bool,
}

/// What a message's OpenPGP protection turned out to be when read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// The message was encrypted, and decrypted with one of your keys.
    pub decrypted: bool,
    /// The signature, if any, and what it says.
    pub signature: Signature,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Signature {
    #[default]
    None,
    /// Made by this key (a user id of it), and the message is unchanged.
    Good(String),
    /// Signed by a key that isn't in your keyring.
    UnknownKey,
    /// The signature doesn't match the message: it was changed, or forged.
    Bad,
}

impl Signature {
    pub fn label(&self) -> String {
        match self {
            Self::None => "Not signed".into(),
            Self::Good(who) => format!("Signed by {who}"),
            Self::UnknownKey => "Signed, by a key you haven't imported".into(),
            Self::Bad => "The signature doesn't match: the message may have been changed".into(),
        }
    }
}

fn users(key: &SignedPublicKey) -> Vec<String> {
    key.details
        .users
        .iter()
        .map(|u| String::from_utf8_lossy(u.id.id()).into_owned())
        .collect()
}

fn fingerprint(key: &impl KeyDetails) -> String {
    format!("{:X}", key.fingerprint())
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect()
}

/// Whether one of the key's user ids is `email`.
fn has_address(key: &SignedPublicKey, email: &str) -> bool {
    let email = email.trim().to_lowercase();
    users(key).iter().any(|u| {
        let u = u.to_lowercase();
        let address = match (u.rfind('<'), u.rfind('>')) {
            (Some(a), Some(b)) if a < b => &u[a + 1..b],
            _ => u.trim(),
        };
        address == email
    })
}

impl Keyring {
    /// The keyring kept in the keychain, or an empty one.
    #[cfg_attr(test, allow(dead_code))]
    pub fn load() -> Result<Self> {
        Ok(match store::read_secret_text(KEYRING)? {
            Some(json) => {
                serde_json::from_str(&json).context("The OpenPGP keyring couldn't be read.")?
            }
            None => Self::default(),
        })
    }
    #[cfg_attr(test, allow(dead_code))]
    pub fn save(&self) -> Result<()> {
        store::write_secret_text(KEYRING, &serde_json::to_string(self)?)
    }
    pub fn secret_keys(&self) -> Vec<SignedSecretKey> {
        self.secret
            .iter()
            .filter_map(|k| SignedSecretKey::from_string(k).ok().map(|(k, _)| k))
            .collect()
    }
    /// Everyone's public keys, your own included.
    pub fn public_keys(&self) -> Vec<SignedPublicKey> {
        let mut keys: Vec<SignedPublicKey> = self
            .public
            .iter()
            .filter_map(|k| SignedPublicKey::from_string(k).ok().map(|(k, _)| k))
            .collect();
        keys.extend(self.secret_keys().into_iter().map(SignedPublicKey::from));
        keys
    }
    /// Every key, for Settings.
    pub fn list(&self) -> Vec<KeyInfo> {
        let mut out: Vec<KeyInfo> = self
            .secret_keys()
            .into_iter()
            .map(|k| {
                let public = SignedPublicKey::from(k);
                KeyInfo {
                    fingerprint: fingerprint(&public),
                    users: users(&public),
                    secret: true,
                    found: false,
                    verified: true,
                }
            })
            .collect();
        for k in self
            .public
            .iter()
            .filter_map(|k| SignedPublicKey::from_string(k).ok().map(|(k, _)| k))
        {
            let fp = fingerprint(&k);
            out.push(KeyInfo {
                found: self.found.contains(&fp),
                verified: !self.found.contains(&fp) || self.verified.contains(&fp),
                fingerprint: fp,
                users: users(&k),
                secret: false,
            });
        }
        out
    }
    /// Adds armored keys: secret keys (opened with `passphrase` if they
    /// have one) and public keys. A key already there is replaced. Returns
    /// how many were added.
    pub fn import(&mut self, armored: &str, passphrase: &str) -> Result<usize> {
        let mut added = 0;
        let text = armored.trim();
        ensure!(!text.is_empty(), "Paste or choose an armored OpenPGP key.");
        if text.contains("PRIVATE KEY BLOCK") {
            let (keys, _) = SignedSecretKey::from_armor_many(text.as_bytes())
                .context("That secret key couldn't be read.")?;
            for key in keys {
                let key = key.context("That secret key couldn't be read.")?;
                key.verify_bindings()
                    .context("That key's signatures don't check out.")?;
                let key = unprotect(key, passphrase)?;
                let fp = fingerprint(&SignedPublicKey::from(key.clone()));
                self.secret.retain(|k| {
                    SignedSecretKey::from_string(k)
                        .map(|(k, _)| fingerprint(&SignedPublicKey::from(k)) != fp)
                        .unwrap_or(true)
                });
                self.secret
                    .push(key.to_armored_string(ArmorOptions::default())?);
                added += 1;
            }
        }
        if text.contains("PUBLIC KEY BLOCK") {
            let (keys, _) = SignedPublicKey::from_armor_many(text.as_bytes())
                .context("That public key couldn't be read.")?;
            for key in keys {
                let key = key.context("That public key couldn't be read.")?;
                key.verify_bindings()
                    .context("That key's signatures don't check out.")?;
                let fp = fingerprint(&key);
                self.public.retain(|k| {
                    SignedPublicKey::from_string(k)
                        .map(|(k, _)| fingerprint(&k) != fp)
                        .unwrap_or(true)
                });
                self.public
                    .push(key.to_armored_string(ArmorOptions::default())?);
                added += 1;
            }
        }
        ensure!(
            added > 0,
            "No OpenPGP keys were found. Keys start with -----BEGIN PGP PUBLIC KEY BLOCK----- or -----BEGIN PGP PRIVATE KEY BLOCK-----."
        );
        Ok(added)
    }
    /// Adds a public key Picacho found for `email` (binary or armored, as
    /// WKD, keyservers and Autocrypt give them). Only the key's user ids for
    /// `email` matter: a key that doesn't name the address isn't taken.
    /// A key imported by hand for the address is never replaced. Returns
    /// whether a key was added or refreshed.
    pub fn add_found(&mut self, email: &str, data: &[u8]) -> Result<bool> {
        let keys: Vec<SignedPublicKey> = match SignedPublicKey::from_reader_many(data) {
            Ok((keys, _)) => keys.filter_map(|k| k.ok()).collect(),
            Err(_) => Vec::new(),
        };
        let key = keys
            .into_iter()
            .find(|k| has_address(k, email) && k.verify_bindings().is_ok())
            .with_context(|| format!("No usable key for {email} was found."))?;
        let fp = fingerprint(&key);
        let by_hand = self.public.iter().any(|k| {
            SignedPublicKey::from_string(k).is_ok_and(|(k, _)| {
                has_address(&k, email) && !self.found.contains(&fingerprint(&k))
            })
        });
        if by_hand {
            return Ok(false);
        }
        // An earlier found key for the address gives way to this one.
        let found = self.found.clone();
        self.public.retain(|k| {
            SignedPublicKey::from_string(k).map_or(true, |(k, _)| {
                let f = fingerprint(&k);
                f != fp && !(found.contains(&f) && has_address(&k, email))
            })
        });
        self.public
            .push(key.to_armored_string(ArmorOptions::default())?);
        if !self.found.contains(&fp) {
            self.found.push(fp);
        }
        // Checks were of keys that may be gone now.
        let kept: Vec<String> = self
            .public
            .iter()
            .filter_map(|k| {
                SignedPublicKey::from_string(k)
                    .ok()
                    .map(|(k, _)| fingerprint(&k))
            })
            .collect();
        self.verified.retain(|f| kept.contains(f));
        Ok(true)
    }
    /// Your public key for `email`, binary, as Autocrypt sends it.
    pub fn own_public_bytes(&self, email: &str) -> Option<Vec<u8>> {
        use pgp::ser::Serialize as _;
        SignedPublicKey::from(self.own_key(email)?).to_bytes().ok()
    }
    /// Checks a key against a fingerprint its owner gave you (read out,
    /// pasted from their mail or chat, or from their QR code): spaces,
    /// colons and case don't matter, an `openpgp4fpr:` link prefix is
    /// accepted, and the whole fingerprint must match. A key that matches
    /// is marked as checked.
    pub fn check(&mut self, fp: &str, given: &str) -> Result<()> {
        let given: String = given
            .trim()
            .trim_start_matches("openpgp4fpr:")
            .trim_start_matches("OPENPGP4FPR:")
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect::<String>()
            .to_ascii_uppercase();
        ensure!(
            given.len() >= 32,
            "Paste the whole fingerprint (40 characters); a short key id can be faked."
        );
        ensure!(
            given == fp.to_ascii_uppercase(),
            "That fingerprint doesn't match this key. Don't use the key until you know why: it may not be theirs."
        );
        self.verify(fp);
        Ok(())
    }
    /// Marks a found key as compared with its owner.
    pub fn verify(&mut self, fp: &str) {
        if !self.verified.iter().any(|f| f == fp) {
            self.verified.push(fp.to_owned());
        }
    }
    /// Of `emails`, those whose key was found automatically and not
    /// compared with its owner yet.
    pub fn unverified(&self, emails: &[String]) -> Vec<String> {
        emails
            .iter()
            .filter(|e| {
                self.key_for(e).is_some_and(|k| {
                    let fp = fingerprint(&k);
                    self.found.contains(&fp) && !self.verified.contains(&fp)
                })
            })
            .cloned()
            .collect()
    }
    /// Forgets the key with this fingerprint.
    pub fn remove(&mut self, fp: &str) {
        self.found.retain(|f| f != fp);
        self.verified.retain(|f| f != fp);
        self.secret.retain(|k| {
            SignedSecretKey::from_string(k)
                .map(|(k, _)| fingerprint(&SignedPublicKey::from(k)) != fp)
                .unwrap_or(true)
        });
        self.public.retain(|k| {
            SignedPublicKey::from_string(k)
                .map(|(k, _)| fingerprint(&k) != fp)
                .unwrap_or(true)
        });
    }
    /// Your secret key for `email`, for signing.
    pub fn own_key(&self, email: &str) -> Option<SignedSecretKey> {
        self.secret_keys()
            .into_iter()
            .find(|k| has_address(&SignedPublicKey::from(k.clone()), email))
    }
    /// A public key for `email`, for encrypting to them.
    pub fn key_for(&self, email: &str) -> Option<SignedPublicKey> {
        self.public_keys()
            .into_iter()
            .find(|k| has_address(k, email))
    }
}

/// A secret key with its passphrase removed, so it can be kept in the
/// keychain and used without asking.
fn unprotect(key: SignedSecretKey, passphrase: &str) -> Result<SignedSecretKey> {
    let password = Password::from(passphrase);
    // The passphrase must open the key: try unlocking a copy.
    let mut test = key.primary_key.clone();
    if test.remove_password(&password).is_err() || test.secret_params().is_encrypted() {
        if passphrase.is_empty() {
            bail!("This key has a passphrase. Enter it to import the key.");
        }
        bail!("The passphrase is wrong.");
    }
    if passphrase.is_empty() {
        return Ok(key);
    }
    let mut key = key;
    key.primary_key
        .remove_password(&password)
        .context("The key couldn't be unlocked.")?;
    for sub in &mut key.secret_subkeys {
        sub.key
            .remove_password(&password)
            .context("A subkey couldn't be unlocked.")?;
    }
    Ok(key)
}

// --- Reading -----------------------------------------------------------------

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}
fn from_latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u8).collect()
}

/// A multipart/signed message's signed part (exactly as sent, CRLF) and its
/// armored signature, and the outer headers.
fn signed_parts(raw: &[u8]) -> Option<(Vec<u8>, String)> {
    let parsed = mailparse::parse_mail(raw).ok()?;
    if !parsed
        .ctype
        .mimetype
        .eq_ignore_ascii_case("multipart/signed")
        || !parsed
            .ctype
            .params
            .get("protocol")
            .is_some_and(|p| p.eq_ignore_ascii_case("application/pgp-signature"))
    {
        return None;
    }
    let boundary = parsed.ctype.params.get("boundary")?;
    let text = latin1(raw);
    let delimiter = format!("--{boundary}");
    // The first part, byte for byte between its delimiters (RFC 3156 §5).
    let start = text.find(&delimiter)? + delimiter.len();
    let start = start + text[start..].find('\n')? + 1;
    let end = start + text[start..].find(&format!("\n{delimiter}"))?;
    let end = if text[..end].ends_with('\r') {
        end - 1
    } else {
        end
    };
    let signed = text[start..end].replace("\r\n", "\n").replace('\n', "\r\n");
    let signature = parsed
        .subparts
        .get(1)
        .filter(|p| {
            p.ctype
                .mimetype
                .eq_ignore_ascii_case("application/pgp-signature")
        })?
        .get_body()
        .ok()?;
    Some((from_latin1(&signed), signature))
}

/// The headers of `raw` other than Content-* ones, ending in CRLF.
fn outer_headers(raw: &[u8]) -> Vec<u8> {
    let text = latin1(raw);
    let head = text
        .find("\r\n\r\n")
        .or_else(|| text.find("\n\n"))
        .map_or(text.as_str(), |i| &text[..i]);
    let mut out = String::new();
    let mut keep = false;
    for line in head.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if !line.starts_with([' ', '\t']) {
            let name = line.split(':').next().unwrap_or("").to_ascii_lowercase();
            keep = !name.starts_with("content-") && name != "mime-version";
        }
        if keep {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("MIME-Version: 1.0\r\n");
    from_latin1(&out)
}

/// The Subject a protected-headers part (draft-autocrypt-lamps-protected-
/// headers, RFC 9788's "v1") carries in its own header, if it has one.
fn protected_subject(part: &[u8]) -> Option<Vec<(String, String)>> {
    let parsed = mailparse::parse_headers(part).ok()?.0;
    use mailparse::MailHeaderMap;
    let content_type = parsed.get_first_value("Content-Type")?;
    if !content_type
        .to_ascii_lowercase()
        .contains("protected-headers")
    {
        return None;
    }
    let found: Vec<(String, String)> = PROTECTED
        .iter()
        .filter_map(|name| parsed.get_first_value(name).map(|v| (name.to_string(), v)))
        .collect();
    (!found.is_empty()).then_some(found)
}

/// `outer` (headers as text, CRLF) with the protected headers put back:
/// each replaces the outer one of its name, or is added.
fn with_headers(outer: &str, headers: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in outer.split_inclusive('\n') {
        if !line.starts_with([' ', '\t']) {
            let name = line
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            skipping = headers.iter().any(|(n, _)| *n == name);
        }
        if !skipping {
            out.push_str(line);
        }
    }
    for (name, value) in headers {
        let title: String = name
            .split('-')
            .map(|w| {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("-");
        out.push_str(&format!("{title}: {}\r\n", encode_header(value)));
    }
    out
}

/// A header value, encoded as RFC 2047 UTF-8 when it isn't plain ASCII.
fn encode_header(text: &str) -> String {
    use base64::Engine as _;
    let text = text.replace(['\r', '\n'], " ");
    if text.is_ascii() {
        text
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(text)
        )
    }
}

/// Headers moved inside the encryption with the subject: what says which
/// conversation the mail is part of. Routing headers stay outside.
const PROTECTED: [&str; 5] = [
    "subject",
    "in-reply-to",
    "references",
    "reply-to",
    "keywords",
];

/// What a protected message's Subject says outside the encryption.
pub const HIDDEN_SUBJECT: &str = "...";

/// Every key of `public` that can check signatures: primaries and subkeys,
/// with the key they belong to.
fn verifiers(public: &[SignedPublicKey]) -> Vec<(&SignedPublicKey, &dyn pgp::types::VerifyingKey)> {
    let mut out: Vec<(&SignedPublicKey, &dyn pgp::types::VerifyingKey)> = Vec::new();
    for key in public {
        out.push((key, &key.primary_key));
        for sub in &key.public_subkeys {
            out.push((key, &sub.key));
        }
    }
    out
}

fn signer_name(key: &SignedPublicKey) -> String {
    users(key)
        .into_iter()
        .next()
        .unwrap_or_else(|| fingerprint(key))
}

/// Checks a detached signature over `content` against your keyring.
fn check(signature: &str, content: &[u8], keys: &[SignedPublicKey]) -> Signature {
    let Ok((signature, _)) = DetachedSignature::from_string(signature) else {
        return Signature::Bad;
    };
    let issuers: Vec<pgp::types::KeyId> = signature
        .signature
        .issuer_key_id()
        .into_iter()
        .cloned()
        .collect();
    let mut known = false;
    for key in keys {
        let primary = &key.primary_key;
        if signature.signature.verify(primary, content).is_ok() {
            return Signature::Good(signer_name(key));
        }
        known |= issuers.contains(&primary.legacy_key_id());
        for sub in &key.public_subkeys {
            if signature.signature.verify(&sub.key, content).is_ok() {
                return Signature::Good(signer_name(key));
            }
            known |= issuers.contains(&sub.key.legacy_key_id());
        }
    }
    if known {
        Signature::Bad
    } else {
        Signature::UnknownKey
    }
}

/// Opens a PGP/MIME message with your keys: decrypts it (and checks a
/// signature inside), or checks a multipart/signed one. Returns the
/// readable message, with its outer headers, and what was found; `None`
/// when the message has no OpenPGP protection, or can't be decrypted.
pub fn open(raw: &[u8], ring: &Keyring) -> Option<(Vec<u8>, Verdict)> {
    if let Some((armored, outer)) = crate::manage::crypto::encrypted_part(raw) {
        let secret = ring.secret_keys();
        if secret.is_empty() {
            return None;
        }
        let (message, _) = Message::from_string(&armored).ok()?;
        let passwords: Vec<Password> = secret.iter().map(|_| Password::empty()).collect();
        let mut opened = message
            .decrypt_with_keys(passwords.iter().collect(), secret.iter().collect())
            .ok()?;
        if opened.is_compressed() {
            opened = opened.decompress().ok()?;
        }
        let public = ring.public_keys();
        let signed_inline = opened.is_signed();
        // The signature is checked once the data has been read.
        let inner = opened.as_data_vec().ok()?;
        let mut signature = Signature::None;
        if signed_inline {
            signature = Signature::UnknownKey;
            for (key, verifier) in verifiers(&public) {
                if opened.verify(verifier).is_ok() {
                    signature = Signature::Good(signer_name(key));
                    break;
                }
            }
        }
        let protected = protected_subject(&inner);
        let outer = match &protected {
            Some(headers) => with_headers(&outer, headers),
            None => outer,
        };
        let mut full = from_latin1(&outer);
        full.extend_from_slice(&inner);
        // A multipart/signed message inside the encryption (sign, then encrypt).
        if let Some((content, sig)) = signed_parts(&full) {
            signature = check(&sig, &content, &public);
            let mut readable = outer_headers(&full);
            readable.extend_from_slice(&content);
            return Some((
                readable,
                Verdict {
                    decrypted: true,
                    signature,
                },
            ));
        }
        return Some((
            full,
            Verdict {
                decrypted: true,
                signature,
            },
        ));
    }
    let (content, sig) = signed_parts(raw)?;
    let signature = check(&sig, &content, &ring.public_keys());
    let mut readable = outer_headers(raw);
    readable.extend_from_slice(&content);
    Some((
        readable,
        Verdict {
            decrypted: false,
            signature,
        },
    ))
}

// --- Sending -----------------------------------------------------------------

/// How a message is protected when sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protect {
    pub sign: bool,
    pub encrypt: bool,
    /// When encrypting, hide the subject too: the real one goes inside
    /// the encryption (protected headers), and "..." outside.
    #[serde(default)]
    pub subject: bool,
}

/// The message's body part (its Content-* headers and body, CRLF) and the
/// rest of its headers.
fn split_body(raw: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let text = latin1(raw);
    let split = text.find("\r\n\r\n").map_or(text.len(), |i| i + 2);
    let (head, body) = text.split_at(split);
    let mut outer = String::new();
    let mut part = String::new();
    let mut into_part = false;
    for line in head.split("\r\n").filter(|l| !l.is_empty()) {
        if !line.starts_with([' ', '\t']) {
            let name = line.split(':').next().unwrap_or("").to_ascii_lowercase();
            into_part = name.starts_with("content-");
            if name == "mime-version" {
                continue;
            }
        }
        let target = if into_part { &mut part } else { &mut outer };
        target.push_str(line);
        target.push_str("\r\n");
    }
    part.push_str(body);
    (from_latin1(&part), from_latin1(&outer))
}

/// Moves the message's Subject inside the part to be encrypted (marking
/// its Content-Type `protected-headers="v1"`), and leaves "..." outside.
fn hide_subject(part: Vec<u8>, outer: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let outer_text = latin1(&outer);
    let mut subject_lines = String::new();
    let mut rest = String::new();
    let mut inside = false;
    let mut has_subject = false;
    for line in outer_text.split_inclusive('\n') {
        if !line.starts_with([' ', '\t']) {
            let name = line
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            inside = PROTECTED.contains(&name.as_str());
            has_subject |= name == "subject";
        }
        if inside {
            subject_lines.push_str(line);
        } else {
            rest.push_str(line);
        }
    }
    if !has_subject {
        return (part, outer);
    }
    rest.push_str(&format!("Subject: {HIDDEN_SUBJECT}\r\n"));
    // The part's own header: its Content-Type marked, then the Subject.
    let part_text = latin1(&part);
    let split = part_text
        .find("\r\n\r\n")
        .map_or(part_text.len(), |i| i + 2);
    let (head, body) = part_text.split_at(split);
    // The Content-Type header, with its folded lines, gets the parameter
    // at its end.
    let mut new_head = String::new();
    let mut in_type = false;
    let mut marked = false;
    let mark = |out: &mut String| {
        while out.ends_with(['\r', '\n']) {
            out.pop();
        }
        out.push_str("; protected-headers=\"v1\"\r\n");
    };
    for line in head.split_inclusive('\n') {
        let continuation = line.starts_with([' ', '\t']);
        if in_type && !continuation {
            mark(&mut new_head);
            in_type = false;
            marked = true;
        }
        if !continuation && !marked && line.to_ascii_lowercase().starts_with("content-type:") {
            in_type = true;
        }
        new_head.push_str(line);
    }
    if in_type {
        mark(&mut new_head);
    }
    new_head.push_str(&subject_lines);
    let mut new_part = from_latin1(&new_head);
    new_part.extend_from_slice(&from_latin1(body));
    (new_part, from_latin1(&rest))
}

fn boundary() -> String {
    format!("=-picacho-{}", uuid::Uuid::new_v4().simple())
}

/// Signs and/or encrypts a finished message as PGP/MIME. `from` is the
/// sender's address (whose key signs), `to` every recipient (each must have
/// a key to encrypt). The sender's own key is always an encryption
/// recipient too, so the copy in Sent stays readable.
pub fn protect(
    raw: &[u8],
    from: &str,
    to: &[String],
    how: Protect,
    ring: &Keyring,
) -> Result<Vec<u8>> {
    if !how.sign && !how.encrypt {
        return Ok(raw.to_vec());
    }
    let (mut part, outer) = split_body(raw);
    // The line break before the next boundary belongs to the boundary
    // (RFC 3156 §5): what is signed ends without it.
    while part.ends_with(b"\r\n") {
        part.truncate(part.len() - 2);
    }
    let (part, outer) = if how.encrypt && how.subject {
        hide_subject(part, outer)
    } else {
        (part, outer)
    };
    let own = ring.own_key(from);
    let mut content = part.clone();
    if how.sign {
        let key = own.as_ref().with_context(|| {
            format!("There's no OpenPGP key of yours for {from}. Import one in Settings → Encryption, or turn off Sign.")
        })?;
        // A subkey made for signing, as most keys have; else the primary key.
        let signing_sub = key
            .secret_subkeys
            .iter()
            .find(|s| s.signatures.iter().any(|sig| sig.key_flags().sign()));
        let signature = match signing_sub {
            Some(sub) => DetachedSignature::sign_binary_data(
                rand08::thread_rng(),
                &sub.key,
                &Password::empty(),
                HashAlgorithm::Sha256,
                &part[..],
            )?,
            None => DetachedSignature::sign_binary_data(
                rand08::thread_rng(),
                &key.primary_key,
                &Password::empty(),
                HashAlgorithm::Sha256,
                &part[..],
            )?,
        }
        .to_armored_string(ArmorOptions::default())?;
        let b = boundary();
        let mut signed = format!(
            "Content-Type: multipart/signed; micalg=pgp-sha256; protocol=\"application/pgp-signature\"; boundary=\"{b}\"\r\n\r\n--{b}\r\n"
        )
        .into_bytes();
        signed.extend_from_slice(&part);
        signed.extend_from_slice(format!(
            "\r\n--{b}\r\nContent-Type: application/pgp-signature; name=\"signature.asc\"\r\nContent-Disposition: attachment; filename=\"signature.asc\"\r\n\r\n{}\r\n--{b}--\r\n",
            signature.replace("\r\n", "\n").replace('\n', "\r\n")
        ).as_bytes());
        content = signed;
    }
    let mut out = outer;
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");
    if how.encrypt {
        let mut keys = Vec::new();
        let mut missing = Vec::new();
        for address in to {
            match ring.key_for(address) {
                Some(k) => keys.push(k),
                None => missing.push(address.clone()),
            }
        }
        ensure!(
            missing.is_empty(),
            "There's no OpenPGP key for {}. Import theirs in Settings → Encryption, or turn off Encrypt.",
            missing.join(", ")
        );
        if let Some(own) = &own {
            keys.push(SignedPublicKey::from(own.clone()));
        }
        let mut builder = MessageBuilder::from_bytes("", content)
            .seipd_v1(rand08::thread_rng(), SymmetricKeyAlgorithm::AES256);
        for key in &keys {
            let for_encryption = key.public_subkeys.iter().find(|s| {
                s.key.algorithm().can_encrypt()
                    && s.signatures.iter().any(|sig| {
                        let flags = sig.key_flags();
                        flags.encrypt_comms() || flags.encrypt_storage()
                    })
            });
            match for_encryption {
                Some(sub) => builder.encrypt_to_key(rand08::thread_rng(), &sub.key)?,
                None => builder.encrypt_to_key(rand08::thread_rng(), &key.primary_key)?,
            };
        }
        let armored = builder.to_armored_string(rand08::thread_rng(), ArmorOptions::default())?;
        let b = boundary();
        out.extend_from_slice(format!(
            "Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"{b}\"\r\n\r\nThis is an OpenPGP/MIME encrypted message (RFC 3156).\r\n--{b}\r\nContent-Type: application/pgp-encrypted\r\nContent-Description: PGP/MIME version identification\r\n\r\nVersion: 1\r\n\r\n--{b}\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\nContent-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n{}\r\n--{b}--\r\n",
            armored.replace("\r\n", "\n").replace('\n', "\r\n")
        ).as_bytes());
    } else {
        out.extend_from_slice(&content);
    }
    Ok(out)
}

/// A key as GnuPG makes them: a certifying primary key, a signing subkey
/// and an encryption subkey. For tests.
#[cfg(test)]
pub fn test_key(uid: &str, passphrase: &str) -> SignedSecretKey {
    use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use pgp::crypto::ecc_curve::ECCCurve;
    {
        let mut sign = SubkeyParamsBuilder::default();
        sign.key_type(KeyType::Ed25519Legacy)
            .can_sign(true)
            .can_encrypt(EncryptionCaps::None)
            .passphrase((!passphrase.is_empty()).then(|| passphrase.to_owned()));
        let mut encrypt = SubkeyParamsBuilder::default();
        encrypt
            .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
            .can_sign(false)
            .can_encrypt(EncryptionCaps::All)
            .passphrase((!passphrase.is_empty()).then(|| passphrase.to_owned()));
        let mut params = SecretKeyParamsBuilder::default();
        params
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(false)
            .can_encrypt(EncryptionCaps::None)
            .primary_user_id(uid.into())
            .passphrase((!passphrase.is_empty()).then(|| passphrase.to_owned()))
            .subkeys(vec![sign.build().unwrap(), encrypt.build().unwrap()]);
        params
            .build()
            .unwrap()
            .generate(rand08::thread_rng())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(uid: &str, passphrase: &str) -> SignedSecretKey {
        test_key(uid, passphrase)
    }

    fn armored_public(k: &SignedSecretKey) -> String {
        SignedPublicKey::from(k.clone())
            .to_armored_string(ArmorOptions::default())
            .unwrap()
    }

    const MULTI: &[u8] = b"From: Ana <ana@example.org>\r\nSubject: Plans\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed;\r\n boundary=\"xx\"\r\n\r\n--xx\r\nContent-Type: text/plain\r\n\r\nMeet at noon.\r\n--xx--\r\n";
    const MESSAGE: &[u8] = b"From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nMeet at noon.\r\n";

    #[test]
    fn signed_and_encrypted_mail_opens_for_its_recipient() {
        let ana = key("Ana <ana@example.org>", "");
        let bo = key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        anas.import(&armored_public(&bo), "").unwrap();
        let mut bos = Keyring::default();
        bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        bos.import(&armored_public(&ana), "").unwrap();
        assert_eq!(bos.list().len(), 2);

        let sent = protect(
            MESSAGE,
            "ana@example.org",
            &["bo@example.se".into()],
            Protect {
                sign: true,
                encrypt: true,
                ..Default::default()
            },
            &anas,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&sent);
        assert!(text.contains("Subject: Plans") && !text.contains("Meet at noon"));
        assert!(text.contains("multipart/encrypted"));

        let (readable, verdict) = open(&sent, &bos).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(
            verdict.signature,
            Signature::Good("Ana <ana@example.org>".into())
        );
        let parsed = mailparse::parse_mail(&readable).unwrap();
        assert_eq!(parsed.get_body().unwrap().trim(), "Meet at noon.");
        // The sender's copy in Sent opens for the sender too.
        assert!(open(&sent, &anas).unwrap().0.ends_with(b"Meet at noon."));
        // Without the right key, nothing opens.
        let stranger = Keyring::default();
        assert!(open(&sent, &stranger).is_none());
    }

    #[test]
    fn signatures_are_checked_and_tampering_is_caught() {
        let ana = key("Ana <ana@example.org>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let signed = protect(
            MESSAGE,
            "ana@example.org",
            &[],
            Protect {
                sign: true,
                encrypt: false,
                ..Default::default()
            },
            &anas,
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&signed).contains("multipart/signed"));

        let mut bos = Keyring::default();
        assert_eq!(
            open(&signed, &bos).unwrap().1.signature,
            Signature::UnknownKey
        );
        bos.import(&armored_public(&ana), "").unwrap();
        let (readable, verdict) = open(&signed, &bos).unwrap();
        assert!(!verdict.decrypted);
        assert_eq!(
            verdict.signature,
            Signature::Good("Ana <ana@example.org>".into())
        );
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));

        let forged = String::from_utf8_lossy(&signed).replace("Meet at noon.", "Meet at nine.");
        assert_eq!(
            open(forged.as_bytes(), &bos).unwrap().1.signature,
            Signature::Bad
        );
        // Unsigned, unencrypted mail isn't OpenPGP's to open.
        assert!(open(MESSAGE, &bos).is_none());
    }

    #[test]
    fn hidden_subjects_come_back_for_the_recipient() {
        let ana = key("Ana <ana@example.org>", "");
        let bo = key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        anas.import(&armored_public(&bo), "").unwrap();
        let mut bos = Keyring::default();
        bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let how = Protect {
            sign: false,
            encrypt: true,
            subject: true,
        };
        let threaded = String::from_utf8_lossy(MESSAGE).replace(
            "Subject: Plans\r\n",
            "Subject: Plans\r\nIn-Reply-To: <thread@example.org>\r\n",
        );
        let sent = protect(
            threaded.as_bytes(),
            "ana@example.org",
            &["bo@example.se".into()],
            how,
            &anas,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&sent);
        assert!(text.contains("Subject: ...\r\n") && !text.contains("Plans"));
        assert!(
            !text.contains("thread@example.org"),
            "the thread is hidden too"
        );
        let (readable, _) = open(&sent, &bos).unwrap();
        let parsed = mailparse::parse_mail(&readable).unwrap();
        use mailparse::MailHeaderMap;
        assert_eq!(parsed.headers.get_first_value("Subject").unwrap(), "Plans");
        assert_eq!(
            parsed.headers.get_first_value("In-Reply-To").unwrap(),
            "<thread@example.org>"
        );
        assert_eq!(parsed.get_body().unwrap().trim(), "Meet at noon.");
    }

    #[test]
    fn hidden_subjects_keep_multipart_bodies() {
        let ana = key("Ana <ana@example.org>", "");
        let mut ring = Keyring::default();
        ring.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let how = Protect {
            sign: false,
            encrypt: true,
            subject: true,
        };
        let sent = protect(MULTI, "ana@example.org", &[], how, &ring).unwrap();
        let (readable, _) = open(&sent, &ring).unwrap();
        let parsed = mailparse::parse_mail(&readable).unwrap();
        use mailparse::MailHeaderMap;
        assert_eq!(parsed.headers.get_first_value("Subject").unwrap(), "Plans");
        assert_eq!(parsed.subparts.len(), 1);
        assert_eq!(
            parsed.subparts[0].get_body().unwrap().trim(),
            "Meet at noon."
        );
    }

    #[test]
    fn found_keys_never_replace_imported_ones() {
        use pgp::ser::Serialize as _;
        let bo = key("Bo <bo@example.se>", "");
        let other = key("Bo <bo@example.se>", "");
        let mut ring = Keyring::default();
        let binary = SignedPublicKey::from(bo.clone()).to_bytes().unwrap();
        assert!(ring.add_found("bo@example.se", &binary).unwrap());
        assert!(ring.list()[0].found);
        assert!(
            ring.add_found("cy@example.net", &binary).is_err(),
            "a key must name the address"
        );
        // A newer found key replaces the older found one.
        let newer = SignedPublicKey::from(other.clone()).to_bytes().unwrap();
        assert!(ring.add_found("bo@example.se", &newer).unwrap());
        assert_eq!(ring.list().len(), 1);
        // One imported by hand stays.
        let mut hand = Keyring::default();
        hand.import(&armored_public(&bo), "").unwrap();
        assert!(!hand.add_found("bo@example.se", &newer).unwrap());
        assert_eq!(hand.list().len(), 1);
        assert!(!hand.list()[0].found);
        // Found keys aren't verified until compared.
        assert_eq!(
            ring.unverified(&["bo@example.se".into()]),
            ["bo@example.se"]
        );
        let fp = ring.list()[0].fingerprint.clone();
        ring.verify(&fp);
        assert!(ring.unverified(&["bo@example.se".into()]).is_empty());
        assert!(ring.list()[0].verified);
        // Checking against a fingerprint they gave.
        let mut other_ring = Keyring::default();
        other_ring.add_found("bo@example.se", &binary).unwrap();
        let real = other_ring.list()[0].fingerprint.clone();
        assert!(
            other_ring.check(&real, &real[..16]).is_err(),
            "short ids aren't enough"
        );
        let wrong = real.replace(&real[..1], if &real[..1] == "A" { "B" } else { "A" });
        assert!(other_ring.check(&real, &wrong).is_err());
        assert!(!other_ring.list()[0].verified);
        let spaced = real
            .chars()
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|c| c.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        other_ring
            .check(&real, &format!("openpgp4fpr:{spaced}"))
            .unwrap();
        assert!(other_ring.list()[0].verified);
        assert!(hand.own_public_bytes("bo@example.se").is_none());
    }

    #[test]
    fn sending_needs_keys_for_everyone() {
        let ana = key("Ana <ana@example.org>", "");
        let mut ring = Keyring::default();
        ring.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let error = protect(
            MESSAGE,
            "ana@example.org",
            &["bo@example.se".into()],
            Protect {
                sign: false,
                encrypt: true,
                ..Default::default()
            },
            &ring,
        )
        .unwrap_err();
        assert!(error.to_string().contains("bo@example.se"));
        let error = protect(
            MESSAGE,
            "other@example.org",
            &[],
            Protect {
                sign: true,
                encrypt: false,
                ..Default::default()
            },
            &ring,
        )
        .unwrap_err();
        assert!(error.to_string().contains("other@example.org"));
    }

    #[test]
    fn protected_secret_keys_import_with_their_passphrase() {
        let locked = key("Cy <cy@example.net>", "hunter2")
            .to_armored_string(ArmorOptions::default())
            .unwrap();
        let mut ring = Keyring::default();
        assert!(
            ring.import(&locked, "")
                .unwrap_err()
                .to_string()
                .contains("passphrase")
        );
        assert!(ring.import(&locked, "wrong").is_err());
        assert_eq!(ring.import(&locked, "hunter2").unwrap(), 1);
        // Kept unlocked, so it signs without asking.
        assert!(
            protect(
                MESSAGE,
                "cy@example.net",
                &[],
                Protect {
                    sign: true,
                    ..Default::default()
                },
                &ring
            )
            .is_ok()
        );
        let fp = ring.list()[0].fingerprint.clone();
        ring.remove(&fp);
        assert!(ring.list().is_empty());
    }
}
