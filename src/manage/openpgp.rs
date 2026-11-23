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
    ArmorOptions, CleartextSignedMessage, Deserializable, DetachedSignature, Message,
    MessageBuilder, SignedPublicKey,
    SignedSecretKey,
};
use pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
use pgp::types::{KeyDetails, Password};
use serde::{Deserialize, Serialize};

mod autocrypt;
pub use autocrypt::{Peer, Recommendation, key_fingerprint, peer_preference_key};

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
    /// Fingerprints of found keys the address's domain published in DNS
    /// that DNSSEC validated (RFC 7929 §7): the domain vouches for them,
    /// so they don't count as unverified either (see `checked`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dnssec: Vec<String>,
    /// Fingerprints of found keys known only from Autocrypt headers or
    /// Autocrypt-Gossip (Autocrypt 1.1 §2.3, §2.4), which `reset_peer`
    /// may forget; a lookup that finds the same key takes it off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub autocrypt: Vec<String>,
    /// Your addresses whose Autocrypt header says prefer-encrypt=mutual
    /// (Autocrypt 1.1 §2.2, `accounts[addr].prefer_encrypt`), lowercased.
    /// Off (nopreference) unless turned on in Settings → Encryption.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_mutual: Vec<String>,
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
    /// Made by this key, unchanged, but the key isn't for the address the
    /// message says it is from: someone else signed it (a message signed by
    /// one person can be sent on as from another).
    OtherSender(String),
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
            Self::OtherSender(who) => {
                format!("Signed by {who}, who isn't the sender shown")
            }
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
                verified: !self.found.contains(&fp) || self.checked(&fp),
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
                // The same key again: merged with what was kept, so a copy
                // from elsewhere never drops a revocation or a user id.
                let earlier = self
                    .public
                    .iter()
                    .filter_map(|k| SignedPublicKey::from_string(k).ok().map(|(k, _)| k))
                    .find(|k| fingerprint(k) == fp);
                let key = match earlier {
                    Some(old) => merge_public(old, key),
                    None => key,
                };
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
    /// A key imported by hand for the address is never replaced, nor is a
    /// found one you checked with its owner by another (only refreshed,
    /// as the same key): anyone can put a key in an Autocrypt header.
    /// Returns whether a key was added or refreshed.
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
        let kept = self.public.iter().any(|k| {
            SignedPublicKey::from_string(k).is_ok_and(|(k, _)| {
                let f = fingerprint(&k);
                has_address(&k, email)
                    && (!self.found.contains(&f) || (self.checked(&f) && f != fp))
            })
        });
        if kept {
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
        self.dnssec.retain(|f| kept.contains(f));
        self.autocrypt.retain(|f| kept.contains(f));
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
        let given = given.trim();
        // `openpgp4fpr:` in any case, and without its query (`?a=…`),
        // whose letters would otherwise count as hex digits.
        let given = match given.get(..12) {
            Some(p) if p.eq_ignore_ascii_case("openpgp4fpr:") => {
                given[12..].split(['?', '#']).next().unwrap_or_default()
            }
            _ => given,
        };
        let given: String = given
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
                    self.found.contains(&fp) && !self.checked(&fp)
                })
            })
            .cloned()
            .collect()
    }
    /// Forgets the key with this fingerprint.
    pub fn remove(&mut self, fp: &str) {
        self.found.retain(|f| f != fp);
        self.verified.retain(|f| f != fp);
        self.dnssec.retain(|f| f != fp);
        self.autocrypt.retain(|f| f != fp);
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
    /// The key to encrypt to for `email`: one its owner hasn't revoked
    /// and that hasn't expired.
    pub fn key_for(&self, email: &str) -> Option<SignedPublicKey> {
        self.public_keys()
            .into_iter()
            .find(|k| {
                has_address(k, email)
                    && k.details.revocation_signatures.is_empty()
                    && !primary_expired(k)
            })
    }
}

/// Two copies of one key as one: every revocation, direct signature,
/// user id (with all its signatures) and subkey (with all its
/// signatures) either has, each once.
fn merge_public(mut into: SignedPublicKey, from: SignedPublicKey) -> SignedPublicKey {
    fn add<T: PartialEq>(list: &mut Vec<T>, more: Vec<T>) {
        for item in more {
            if !list.contains(&item) {
                list.push(item);
            }
        }
    }
    add(&mut into.details.revocation_signatures, from.details.revocation_signatures);
    add(&mut into.details.direct_signatures, from.details.direct_signatures);
    for user in from.details.users {
        match into.details.users.iter_mut().find(|u| u.id == user.id) {
            Some(kept) => add(&mut kept.signatures, user.signatures),
            None => into.details.users.push(user),
        }
    }
    for sub in from.public_subkeys {
        match into
            .public_subkeys
            .iter_mut()
            .find(|k| fingerprint(&k.key) == fingerprint(&sub.key))
        {
            Some(kept) => add(&mut kept.signatures, sub.signatures),
            None => into.public_subkeys.push(sub),
        }
    }
    into
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
    // RFC 3156 §5: micalg names the hash, as `pgp-<name>`.
    if parsed
        .ctype
        .params
        .get("micalg")
        .is_some_and(|m| !m.to_ascii_lowercase().starts_with("pgp-"))
    {
        return None;
    }
    let boundary = parsed.ctype.params.get("boundary").filter(|b| !b.is_empty())?;
    let text = latin1(raw);
    let delimiter = format!("--{boundary}");
    // The first part, byte for byte between its delimiters (RFC 3156 §5).
    // Delimiters are whole lines of the body: not text in a header, nor a
    // longer boundary (a nested part's) that begins with this one.
    let body = match (text.find("\r\n\r\n"), text.find("\n\n")) {
        (Some(c), l) if l.is_none_or(|l| c <= l) => c + 2,
        (_, Some(l)) => l + 1,
        _ => return None,
    };
    let line_at = |from: usize| -> Option<usize> {
        let mut at = from;
        loop {
            let found = at + text[at..].find(&delimiter)?;
            let after = &text[found + delimiter.len()..];
            let line_start = found == body || text[..found].ends_with('\n');
            let ends = |rest: &str| rest.is_empty() || rest.starts_with(['\r', '\n', ' ', '\t']);
            // The delimiter, or the closing one ("--boundary--").
            let line_end = ends(after) || after.strip_prefix("--").is_some_and(ends);
            if line_start && line_end {
                return Some(found);
            }
            at = found + 1;
        }
    };
    let start = line_at(body)? + delimiter.len();
    let start = start + text[start..].find('\n')? + 1;
    let end = line_at(start)?.checked_sub(1)?;
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
    // RFC 9788 §4.10: a legacy display part repeats the headers in the
    // body for old readers; the real headers are those on the part.
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
    crate::sync::mime::encoded_words(text)
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

/// Headers copied inside the encryption as well as left outside (RFC
/// 9788 §2.1), so a reader can tell they weren't changed on the way.
const COPIED: [&str; 5] = ["from", "to", "cc", "date", "message-id"];

/// What a protected message's Subject says outside the encryption.
pub const HIDDEN_SUBJECT: &str = "...";

/// Every key of `public` that can check signatures: primaries and subkeys,
/// with the key they belong to.
/// Keys their owner hasn't revoked (a stolen key, given up): only those
/// vouch for a signature. Every key counted before, revoked ones included.
fn unrevoked(public: &[SignedPublicKey]) -> impl Iterator<Item = &SignedPublicKey> {
    public
        .iter()
        .filter(|k| k.details.revocation_signatures.is_empty() && !primary_expired(k))
}
/// A key's subkeys bound to it, not revoked, not expired, and (where their
/// binding says what they're for) for signing.
fn usable_subkeys(key: &SignedPublicKey) -> impl Iterator<Item = &pgp::composed::SignedPublicSubKey> {
    use pgp::packet::SignatureType;
    key.public_subkeys.iter().filter(|sub| {
        let binding = sub
            .signatures
            .iter()
            .filter(|s| s.typ() == Some(SignatureType::SubkeyBinding))
            .max_by_key(|s| s.created().map(|t| t.as_secs()));
        !sub.signatures.iter().any(|s| s.typ() == Some(SignatureType::SubkeyRevocation))
            && sub.verify_bindings(&key.primary_key).is_ok()
            && binding.is_some_and(|b| {
                !past(sub.key.created_at().as_secs(), b.key_expiration_time())
                    && b.key_flags().sign()
            })
    })
}
/// Whether a key made at `created` that lasts `lasts` (none, or zero:
/// forever) has expired.
fn past(created: u32, lasts: Option<pgp::types::Duration>) -> bool {
    let Some(lasts) = lasts.map(|d| d.as_secs()).filter(|d| *d > 0) else {
        return false;
    };
    let now = chrono::Utc::now().timestamp();
    i64::from(created) + i64::from(lasts) < now
}
/// Whether the primary key has expired: its newest self-signature (on a
/// user id, or directly on the key) says how long it lasts.
fn primary_expired(key: &SignedPublicKey) -> bool {
    let newest = key
        .details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter())
        .chain(key.details.direct_signatures.iter())
        .max_by_key(|s| s.created().map(|t| t.as_secs()));
    newest.is_some_and(|s| past(key.primary_key.created_at().as_secs(), s.key_expiration_time()))
}

/// The keys a signature may be checked against (see `unrevoked`).
fn verifiers(public: &[SignedPublicKey]) -> Vec<(&SignedPublicKey, &dyn pgp::types::VerifyingKey)> {
    let mut out: Vec<(&SignedPublicKey, &dyn pgp::types::VerifyingKey)> = Vec::new();
    for key in unrevoked(public) {
        out.push((key, &key.primary_key));
        for sub in usable_subkeys(key) {
            out.push((key, &sub.key));
        }
    }
    out
}

/// The verdict for a good signature by `key` on a message from `from`
/// (its From address): Good when the key is for that address, else
/// OtherSender. Without a From to compare (none given), Good.
fn good_from(key: &SignedPublicKey, from: Option<&str>) -> Signature {
    match from {
        Some(from) if !has_address(key, from) => Signature::OtherSender(signer_name(key)),
        _ => Signature::Good(signer_name(key)),
    }
}

/// The address a message's From header gives, lowercase, if it has one.
fn from_address(raw: &[u8]) -> Option<String> {
    use mailparse::MailHeaderMap as _;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    let from = headers.get_first_value("From")?;
    let address = crate::sync::mime::receipt_address(&from);
    (!address.is_empty()).then(|| address.to_lowercase())
}

fn signer_name(key: &SignedPublicKey) -> String {
    users(key)
        .into_iter()
        .next()
        .unwrap_or_else(|| fingerprint(key))
}

/// Checks a detached signature over `content` against your keyring.
fn check(signature: &str, content: &[u8], keys: &[SignedPublicKey], from: Option<&str>) -> Signature {
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
    // Only keys still good (see `unrevoked`): one revoked is unknown, not a
    // failure, as what it signed may well predate the revocation.
    for key in unrevoked(keys) {
        let primary = &key.primary_key;
        if signature.signature.verify(primary, content).is_ok() {
            return good_from(key, from);
        }
        known |= issuers.contains(&primary.legacy_key_id());
        for sub in usable_subkeys(key) {
            if signature.signature.verify(&sub.key, content).is_ok() {
                return good_from(key, from);
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
    // A signature counts as the sender's only from a key for the address
    // the message is from (see `good_from`).
    let from = from_address(raw);
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
        // The signature is checked once the data has been read. Read up to
        // what a message may be: compressed data a few kilobytes long can
        // inflate to gigabytes, and was read whole.
        let inner = {
            use std::io::Read as _;
            let limit = crate::manage::store::MAX_SOURCE;
            let mut inner = Vec::new();
            (&mut opened).take(limit + 1).read_to_end(&mut inner).ok()?;
            if inner.len() as u64 > limit {
                return None;
            }
            inner
        };
        let mut signature = Signature::None;
        if signed_inline {
            signature = Signature::UnknownKey;
            for (key, verifier) in verifiers(&public) {
                if opened.verify(verifier).is_ok() {
                    signature = good_from(key, from.as_deref());
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
            signature = check(&sig, &content, &public, from.as_deref());
            let mut readable = outer_headers(&full);
            // Hidden headers signed with the text sit on the signed part,
            // not the encrypted one: the subject read "..." whenever the
            // message was signed as well.
            if let Some(headers) = protected_subject(&content) {
                readable = from_latin1(&with_headers(&latin1(&readable), &headers));
            }
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
    match signed_parts(raw) {
        Some((content, sig)) => {
            let signature = check(&sig, &content, &ring.public_keys(), from.as_deref());
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
        // Not PGP/MIME: inline PGP, or PGP/MIME a list wrapped in a part.
        None => open_inline(raw, ring, from.as_deref()).or_else(|| open_nested(raw, ring)),
    }
}

/// Inline PGP (RFC 4880 §7, the old way): a plain-text body that is a
/// cleartext-signed message or an armored encrypted one. The readable
/// message is its headers with the text as a plain UTF-8 body.
fn open_inline(raw: &[u8], ring: &Keyring, from: Option<&str>) -> Option<(Vec<u8>, Verdict)> {
    let parsed = mailparse::parse_mail(raw).ok()?;
    let text_part = if parsed.subparts.is_empty() {
        &parsed
    } else {
        // A list footer may follow in its own part: the first plain text.
        let mut leaves = Vec::new();
        crate::sync::mime::collect_leaves(&parsed, &mut leaves);
        leaves.into_iter().find(|p| p.ctype.mimetype.eq_ignore_ascii_case("text/plain"))?
    };
    if !text_part.ctype.mimetype.eq_ignore_ascii_case("text/plain") {
        return None;
    }
    let body = text_part.get_body().ok()?;
    let readable = |text: &str| {
        let mut out = outer_headers(raw);
        out.extend_from_slice(b"Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n");
        out.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
        out
    };
    if let Some(start) = body.find("-----BEGIN PGP SIGNED MESSAGE-----") {
        let end = body[start..].find("-----END PGP SIGNATURE-----")? + start + 27;
        let (message, _) = CleartextSignedMessage::from_string(&body[start..end]).ok()?;
        let public = ring.public_keys();
        let signed = message.signed_text();
        let mut signature = Signature::UnknownKey;
        'keys: for key in unrevoked(&public) {
            for sig in message.signatures() {
                let by_sub = usable_subkeys(key).any(|sub| sig.verify(&sub.key, signed.as_bytes()).is_ok());
                if by_sub || sig.verify(&key.primary_key, signed.as_bytes()).is_ok() {
                    signature = good_from(key, from);
                    break 'keys;
                }
            }
        }
        // Text around the signed block isn't covered by it: left out.
        return Some((
            readable(&signed),
            Verdict {
                decrypted: false,
                signature,
            },
        ));
    }
    if let Some(start) = body.find("-----BEGIN PGP MESSAGE-----") {
        let end = body[start..].find("-----END PGP MESSAGE-----")? + start + 25;
        let secret = ring.secret_keys();
        if secret.is_empty() {
            return None;
        }
        let (message, _) = Message::from_string(&body[start..end]).ok()?;
        let passwords: Vec<Password> = secret.iter().map(|_| Password::empty()).collect();
        let mut opened = message
            .decrypt_with_keys(passwords.iter().collect(), secret.iter().collect())
            .ok()?;
        if opened.is_compressed() {
            opened = opened.decompress().ok()?;
        }
        let signed = opened.is_signed();
        let inner = {
            use std::io::Read as _;
            let limit = crate::manage::store::MAX_SOURCE;
            let mut inner = Vec::new();
            (&mut opened).take(limit + 1).read_to_end(&mut inner).ok()?;
            if inner.len() as u64 > limit {
                return None;
            }
            inner
        };
        let mut signature = Signature::None;
        if signed {
            signature = Signature::UnknownKey;
            let public = ring.public_keys();
            for (key, verifier) in verifiers(&public) {
                if opened.verify(verifier).is_ok() {
                    signature = good_from(key, from);
                    break;
                }
            }
        }
        return Some((
            readable(&String::from_utf8_lossy(&inner)),
            Verdict {
                decrypted: true,
                signature,
            },
        ));
    }
    None
}

/// PGP/MIME one level down: a mailing list's multipart/mixed wrapping the
/// signed or encrypted message with its footer. The message is opened as
/// its own; the footer, not covered by the signature, is left out.
fn open_nested(raw: &[u8], ring: &Keyring) -> Option<(Vec<u8>, Verdict)> {
    let parsed = mailparse::parse_mail(raw).ok()?;
    if !parsed.ctype.mimetype.eq_ignore_ascii_case("multipart/mixed") {
        return None;
    }
    let first = parsed.subparts.first()?;
    let kind = first.ctype.mimetype.to_ascii_lowercase();
    if kind != "multipart/signed" && kind != "multipart/encrypted" {
        return None;
    }
    // The part as its own message: the outer headers, then the part.
    let part = first.raw_bytes;
    let mut inner = outer_headers(raw);
    inner.truncate(inner.len().saturating_sub(b"MIME-Version: 1.0\r\n".len()));
    inner.extend_from_slice(part);
    open(&inner, ring)
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
    let mut copied = false;
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
            // RFC 9788 §2.1: the routing headers are protected too, by a
            // copy inside; they stay outside, as delivery needs them.
            copied = COPIED.contains(&name.as_str());
            has_subject |= name == "subject";
        }
        if inside {
            subject_lines.push_str(line);
        } else {
            if copied {
                subject_lines.push_str(line);
            }
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
        // The older draft's marker and RFC 9788's (§2.1.1), for readers
        // of either.
        out.push_str("; protected-headers=\"v1\"; hp=\"cipher\"\r\n");
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
    // (RFC 3156 §5): what is signed ends without that one; the body's own
    // blank lines are its own.
    if part.ends_with(b"\r\n") {
        part.truncate(part.len() - 2);
    }
    let (part, outer) = if how.encrypt && how.subject {
        hide_subject(part, outer)
    } else {
        (part, outer)
    };
    // Encrypted to several people: each one's key goes inside, so they can
    // write to the others encrypted too (Autocrypt-Gossip, Autocrypt 1.1
    // §3.6).
    let part = if how.encrypt && to.len() > 1 {
        let gossip: String = to
            .iter()
            .filter_map(|addr| {
                use pgp::ser::Serialize as _;
                let key = ring.key_for(addr)?;
                let bytes = key.to_bytes().ok()?;
                let header = crate::sync::keys::autocrypt_header(&addr.to_lowercase(), &bytes);
                Some(format!("Autocrypt-Gossip: {header}\r\n"))
            })
            .collect();
        let mut with = gossip.into_bytes();
        with.extend_from_slice(&part);
        with
    } else {
        part
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
        // Each recipient's key to encrypt to: an encryption subkey bound
        // to it, neither revoked nor expired, else the key itself.
        macro_rules! add_recipients {
            ($builder:expr) => {
                for key in &keys {
                    match encryption_subkey(key) {
                        Some(sub) => $builder.encrypt_to_key(rand08::thread_rng(), &sub.key)?,
                        None => $builder.encrypt_to_key(rand08::thread_rng(), &key.primary_key)?,
                    };
                }
            };
        }
        // RFC 9580's AEAD encryption (SEIPD v2) where every recipient's key
        // says it reads it; else SEIPD v1, which every OpenPGP app reads.
        let armored = if keys.iter().all(reads_seipd_v2) {
            let mut builder = MessageBuilder::from_bytes("", content).seipd_v2(
                rand08::thread_rng(),
                SymmetricKeyAlgorithm::AES256,
                pgp::crypto::aead::AeadAlgorithm::Ocb,
                pgp::crypto::aead::ChunkSize::default(),
            );
            add_recipients!(builder);
            builder.to_armored_string(rand08::thread_rng(), ArmorOptions::default())?
        } else {
            let mut builder = MessageBuilder::from_bytes("", content)
                .seipd_v1(rand08::thread_rng(), preferred_cipher(&keys));
            add_recipients!(builder);
            builder.to_armored_string(rand08::thread_rng(), ArmorOptions::default())?
        };
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

/// The newest self-signature on a key (on a user id, or on the key).
fn newest_self_signature(key: &SignedPublicKey) -> Option<&pgp::packet::Signature> {
    key.details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter())
        .chain(key.details.direct_signatures.iter())
        .max_by_key(|s| s.created().map(|t| t.as_secs()))
}
/// Whether a key says it reads AEAD-encrypted data (SEIPD v2, RFC 9580
/// §5.13.2) with AES-256 in OCB mode.
fn reads_seipd_v2(key: &SignedPublicKey) -> bool {
    newest_self_signature(key).is_some_and(|s| {
        s.features().is_some_and(|f| f.seipd_v2())
            && s.preferred_aead_algs().iter().any(|(sym, aead)| {
                *sym == SymmetricKeyAlgorithm::AES256 && *aead == pgp::crypto::aead::AeadAlgorithm::Ocb
            })
    })
}
/// The strongest cipher every recipient lists (RFC 9580 §13.2), AES-256
/// where all take it or none says; AES-128 is every key's fallback.
fn preferred_cipher(keys: &[SignedPublicKey]) -> SymmetricKeyAlgorithm {
    let lists: Vec<&[SymmetricKeyAlgorithm]> = keys
        .iter()
        .filter_map(newest_self_signature)
        .map(|s| s.preferred_symmetric_algs())
        .filter(|l| !l.is_empty())
        .collect();
    for candidate in [
        SymmetricKeyAlgorithm::AES256,
        SymmetricKeyAlgorithm::AES192,
        SymmetricKeyAlgorithm::AES128,
    ] {
        if lists.iter().all(|l| l.contains(&candidate)) {
            return candidate;
        }
    }
    SymmetricKeyAlgorithm::AES128
}
/// A key's subkey for encryption: bound to it, not revoked or expired.
fn encryption_subkey(key: &SignedPublicKey) -> Option<&pgp::composed::SignedPublicSubKey> {
    use pgp::packet::SignatureType;
    key.public_subkeys.iter().find(|s| {
        let binding = s
            .signatures
            .iter()
            .filter(|sig| sig.typ() == Some(SignatureType::SubkeyBinding))
            .max_by_key(|sig| sig.created().map(|t| t.as_secs()));
        s.key.algorithm().can_encrypt()
            && !s.signatures.iter().any(|sig| sig.typ() == Some(SignatureType::SubkeyRevocation))
            && s.verify_bindings(&key.primary_key).is_ok()
            && binding.is_some_and(|b| {
                let flags = b.key_flags();
                (flags.encrypt_comms() || flags.encrypt_storage())
                    && !past(s.key.created_at().as_secs(), b.key_expiration_time())
            })
    })
}

/// A new key of yours for `name <email>`, as GnuPG makes them (Ed25519
/// certifying primary, Ed25519 signing and Curve25519 encryption subkeys;
/// version 4, which every OpenPGP app reads). It doesn't expire: the
/// library makes keys without an end date.
pub fn generate_key(name: &str, email: &str) -> Result<SignedSecretKey> {
    use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use pgp::crypto::ecc_curve::ECCCurve;
    ensure!(email.contains('@'), "Enter the address the key is for.");
    let name = name.replace(['<', '>', '\r', '\n'], "");
    let uid = if name.trim().is_empty() {
        format!("<{}>", email.trim())
    } else {
        format!("{} <{}>", name.trim(), email.trim())
    };
    let mut sign = SubkeyParamsBuilder::default();
    sign.key_type(KeyType::Ed25519Legacy)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None);
    let mut encrypt = SubkeyParamsBuilder::default();
    encrypt
        .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(false)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id(uid)
        .preferred_symmetric_algorithms(
            vec![SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128].into(),
        )
        .subkeys(vec![
            sign.build().context("The key couldn't be made.")?,
            encrypt.build().context("The key couldn't be made.")?,
        ]);
    params
        .build()
        .context("The key couldn't be made.")?
        .generate(rand08::thread_rng())
        .context("The key couldn't be made.")
}
/// The armored, password-encrypted key an Autocrypt Setup Message
/// carries (Autocrypt 1.1 §4.4: a message with `Autocrypt-Setup-Message:
/// v1` and an `application/autocrypt-setup` attachment), if `raw` is one.
pub fn setup_message_payload(raw: &[u8]) -> Option<String> {
    use mailparse::MailHeaderMap as _;
    let parsed = mailparse::parse_mail(raw).ok()?;
    if !parsed
        .headers
        .get_first_value("Autocrypt-Setup-Message")
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("v1"))
    {
        return None;
    }
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&parsed, &mut leaves);
    let part = leaves
        .into_iter()
        .find(|p| p.ctype.mimetype.eq_ignore_ascii_case("application/autocrypt-setup"))?;
    let text = part.get_body().ok()?;
    let start = text.find("-----BEGIN PGP MESSAGE-----")?;
    let end = text[start..].find("-----END PGP MESSAGE-----")? + start + 25;
    Some(text[start..end].to_owned())
}
impl Keyring {
    /// Takes in the key an Autocrypt Setup Message carries, opened with
    /// its setup code (the 36 digits shown where it was made; dashes and
    /// spaces don't matter). Returns how many keys were added.
    pub fn import_setup_message(&mut self, raw: &[u8], code: &str) -> Result<usize> {
        let armored = setup_message_payload(raw).context("This isn't an Autocrypt Setup Message.")?;
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        ensure!(digits.len() == 36, "Enter the setup code's 36 digits.");
        // The code as it was made: nine groups of four, dashes between.
        let code = digits
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-");
        let (message, _) = Message::from_string(&armored).context("The setup message can't be read.")?;
        let mut opened = message
            .decrypt_with_password(&Password::from(code.as_str()))
            .map_err(|_| anyhow::anyhow!("That setup code doesn't open it."))?;
        let mut text = String::new();
        {
            use std::io::Read as _;
            (&mut opened)
                .take(1 << 20)
                .read_to_string(&mut text)
                .context("The setup message can't be read.")?;
        }
        self.import(&text, "")
    }
    /// Makes a new key of yours for `email` and keeps it (see
    /// `generate_key`). Returns its fingerprint.
    pub fn create(&mut self, name: &str, email: &str) -> Result<String> {
        let key = generate_key(name, email)?;
        let fp = fingerprint(&SignedPublicKey::from(key.clone()));
        self.secret.push(key.to_armored_string(ArmorOptions::default())?);
        Ok(fp)
    }
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
    fn inline_pgp_signatures_are_checked() {
        let ana = key("Ana <ana@example.org>", "");
        let mut bos = Keyring::default();
        bos.import(&armored_public(&ana), "").unwrap();
        let signing = ana.secret_subkeys.iter().find(|s| s.signatures.iter().any(|g| g.key_flags().sign())).unwrap();
        let signed = CleartextSignedMessage::sign(rand08::thread_rng(), "Meet at noon.\n- Ana", &signing.key, &Password::empty())
            .unwrap()
            .to_armored_string(ArmorOptions::default())
            .unwrap();
        let mail = format!("From: Ana <ana@example.org>\r\nSubject: Plans\r\nContent-Type: text/plain\r\n\r\nHi\r\n{signed}\r\n");
        let (readable, verdict) = open(mail.as_bytes(), &bos).unwrap();
        assert!(matches!(verdict.signature, Signature::Good(_)), "{verdict:?}");
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        let forged = mail.replace("Meet at noon.", "Meet at midnight.");
        let (_, verdict) = open(forged.as_bytes(), &bos).unwrap();
        assert!(!matches!(verdict.signature, Signature::Good(_)), "{verdict:?}");
    }

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

    /// A key lasts from its creation for its stated time; none, or zero, is
    /// for ever.
    #[test]
    fn keys_expire_when_they_say() {
        use pgp::types::Duration;
        let now = chrono::Utc::now().timestamp() as u32;
        let day = 86_400;
        assert!(!past(now - 10 * day, None));
        assert!(!past(now - 10 * day, Some(Duration::from_secs(0))));
        assert!(!past(now - 10 * day, Some(Duration::from_secs(30 * day))));
        assert!(past(now - 10 * day, Some(Duration::from_secs(5 * day))));
        // The test keys don't expire, and their signing subkey is usable.
        let ana = SignedPublicKey::from(key("Ana <ana@example.org>", ""));
        assert!(!primary_expired(&ana));
        assert_eq!(usable_subkeys(&ana).count(), 1, "the signing subkey, not the encryption one");
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

        // Ana's signature, but the message says it is from someone else (its
        // From isn't signed): not called Ana's word for the sender shown.
        let resent = String::from_utf8_lossy(&signed).replacen("From: Ana <ana@example.org>", "From: Bank <alerts@bank.example>", 1);
        assert_eq!(
            open(resent.as_bytes(), &bos).unwrap().1.signature,
            Signature::OtherSender("Ana <ana@example.org>".into())
        );
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

    /// The signed part is found by whole delimiter lines in the body, not
    /// by the boundary written in a header.
    #[test]
    fn signed_parts_are_found_by_their_delimiter_lines() {
        let raw = b"Subject: about --b and more\r\nContent-Type: multipart/signed; protocol=\"application/pgp-signature\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: multipart/mixed; boundary=\"inner\"\r\n\r\n--inner\r\nContent-Type: text/plain\r\n\r\nHi --b- there\r\n--inner--\r\n\r\n--b\r\nContent-Type: application/pgp-signature\r\n\r\nSIG\r\n--b--\r\n";
        let (signed, signature) = signed_parts(raw).unwrap();
        let signed = String::from_utf8(signed).unwrap();
        assert!(signed.starts_with("Content-Type: multipart/mixed"), "{signed}");
        assert!(signed.ends_with("--inner--\r\n"), "{signed}");
        assert_eq!(signature.trim(), "SIG");
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
        // A key you checked isn't replaced by another found one (a forged
        // Autocrypt header), only refreshed as itself.
        assert!(!ring.add_found("bo@example.se", &binary).unwrap());
        assert_eq!(ring.list().len(), 1);
        assert_eq!(ring.list()[0].fingerprint, fp);
        assert!(ring.list()[0].verified);
        assert!(ring.add_found("bo@example.se", &newer).unwrap());
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

    /// The decrypted payload of a PGP/MIME message, exactly as encrypted:
    /// the root of the Cryptographic Payload (RFC 9788 §1.7).
    fn payload(sent: &[u8], key: &SignedSecretKey) -> Vec<u8> {
        let (armored, _) = crate::manage::crypto::encrypted_part(sent).unwrap();
        let (message, _) = Message::from_string(&armored).unwrap();
        let mut opened = message.decrypt(&Password::empty(), key).unwrap();
        if opened.is_compressed() {
            opened = opened.decompress().unwrap();
        }
        opened.as_data_vec().unwrap()
    }

    /// The header block of `raw` (up to the first blank line).
    fn head(raw: &[u8]) -> String {
        let text = String::from_utf8_lossy(raw).into_owned();
        text.split("\r\n\r\n").next().unwrap_or_default().to_owned()
    }

    /// RFC 9788 §2.1.1 and §3.2.1: a message encrypted with its subject
    /// hidden carries hp="cipher" on the root of the Cryptographic Payload,
    /// the real Subject and the routing headers inside, "..." outside, and
    /// never a Bcc inside.
    #[test]
    fn encrypted_payload_carries_protected_headers() {
        let ana = key("Ana <ana@example.org>", "");
        let bo = key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        anas.import(&armored_public(&bo), "").unwrap();
        let raw = String::from_utf8_lossy(MESSAGE).replace(
            "Subject: Plans\r\n",
            "Cc: Cy <cy@example.net>\r\nBcc: secret@example.com\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nSubject: Plans\r\n",
        );
        let how = Protect { sign: false, encrypt: true, subject: true, ..Default::default() };
        let sent = protect(raw.as_bytes(), "ana@example.org", &["bo@example.se".into()], how, &anas).unwrap();
        let outer = head(&sent);
        assert!(outer.contains("\r\nSubject: ...") && !outer.contains("Plans"), "{outer}");
        assert!(outer.contains("Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\""), "{outer}");
        let inner = payload(&sent, &bo);
        let inner_head = head(&inner);
        let parsed = mailparse::parse_mail(&inner).unwrap();
        assert_eq!(parsed.ctype.mimetype, "text/plain");
        assert_eq!(parsed.ctype.params.get("hp").map(String::as_str), Some("cipher"));
        assert_eq!(parsed.ctype.params.get("protected-headers").map(String::as_str), Some("v1"));
        use mailparse::MailHeaderMap;
        for (name, value) in [
            ("Subject", "Plans"),
            ("From", "Ana <ana@example.org>"),
            ("To", "Bo <bo@example.se>"),
            ("Cc", "Cy <cy@example.net>"),
            ("Date", "Mon, 23 Nov 2026 10:00:00 +0000"),
            ("Message-ID", "<1@example.org>"),
        ] {
            assert_eq!(parsed.headers.get_first_value(name).as_deref(), Some(value), "{name} in {inner_head}");
        }
        assert!(parsed.headers.get_first_value("Bcc").is_none(), "{inner_head}");
        assert_eq!(parsed.get_body().unwrap().trim(), "Meet at noon.");
    }

    /// Autocrypt 1.1 §2.4 (Key Gossip): encrypted to several people, the
    /// root part of the encrypted payload has one Autocrypt-Gossip header
    /// per recipient with the key the mail is encrypted to; none outside.
    #[test]
    fn gossip_headers_go_inside_the_encryption() {
        use pgp::ser::Serialize as _;
        let ana = key("Ana <ana@example.org>", "");
        let bo = key("Bo <bo@example.se>", "");
        let cy = key("Cy <cy@example.net>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        anas.import(&armored_public(&bo), "").unwrap();
        anas.import(&armored_public(&cy), "").unwrap();
        let to = ["bo@example.se".to_owned(), "cy@example.net".to_owned()];
        let how = Protect { sign: false, encrypt: true, ..Default::default() };
        let sent = protect(MESSAGE, "ana@example.org", &to, how, &anas).unwrap();
        assert!(!String::from_utf8_lossy(&sent).contains("Autocrypt-Gossip"));
        let inner = payload(&sent, &cy);
        let gossip = crate::sync::keys::autocrypt_gossip(&inner, &to);
        let cy_bytes = SignedPublicKey::from(cy.clone()).to_bytes().unwrap();
        let bo_bytes = SignedPublicKey::from(bo.clone()).to_bytes().unwrap();
        assert_eq!(gossip, vec![("bo@example.se".into(), bo_bytes), ("cy@example.net".into(), cy_bytes)]);
        // One recipient: nothing to gossip.
        let alone = protect(MESSAGE, "ana@example.org", &to[..1], how, &anas).unwrap();
        assert!(!String::from_utf8_lossy(&payload(&alone, &bo)).contains("Autocrypt-Gossip"));
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
