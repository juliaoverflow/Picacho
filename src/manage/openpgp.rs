//! OpenPGP for every account: your own keys, kept in the system keychain,
//! and the keys of people you write to, kept in the database; PGP/MIME messages (RFC 3156)
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
use pgp::types::{KeyDetails, Password, SigningKey};
use serde::{Deserialize, Serialize};

pub mod keys;
/// Your keys on OpenPGP cards (security keys).
pub mod card;
mod autocrypt;
mod inline;
mod legacy_display;
mod nested;
mod protected;
mod signers;
mod v6;
pub use autocrypt::{Peer, Recommendation, key_fingerprint, peer_preference_key};
/// RFC 9788 §4.4.2's From comparison, shared with S/MIME.
pub(crate) use protected::from_mismatch;

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
    /// Your keys on OpenPGP cards: each one's certificate and the card that
    /// holds its secret parts (`card.rs`). Nothing secret is kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<card::CardKey>,
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
    /// When the key expires (or expired), as seconds since 1970; `None`
    /// when it never does (see `keys::expires`).
    pub expires: Option<i64>,
    /// The key carries a Key Revocation signature (RFC 9580 §5.2.1.11):
    /// it is no longer encrypted to nor trusted (see `key_for`).
    pub revoked: bool,
    /// Its secret parts are on this OpenPGP card (its name), not here.
    pub card: Option<String>,
}

/// What a message's OpenPGP protection turned out to be when read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// The message was encrypted, and decrypted with one of your keys.
    pub decrypted: bool,
    /// The signature, if any, and what it says.
    pub signature: Signature,
    /// RFC 9788 §4.4.2: the protected From is for another address than the
    /// outer one, and no signature by a key for it vouches for it.
    pub from_mismatch: Option<FromMismatch>,
    /// Inline PGP: the names of files shown with the message that neither
    /// its signature nor its encryption covers (anyone could have added
    /// them on the way, to a genuine signed or encrypted text).
    pub uncovered: Vec<String>,
}

/// The two From fields of a message whose protected From doesn't match
/// its outer one (RFC 9788 §4.4.1.1), as written (decoded).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FromMismatch {
    /// The From outside the protection: what the sender's server saw.
    pub outer: String,
    /// The From inside the protection.
    pub protected: String,
}

impl FromMismatch {
    /// RFC 9788 §4.4.2: a warning that shows both From fields.
    pub fn label(&self) -> String {
        t!("manage-openpgp-warning-the-sender-inside-the-message", protected = self.protected, outer = self.outer)
    }
}

impl Verdict {
    /// The signature line of the message's details and whether it is good:
    /// what the signature says, with the From warning (RFC 9788 §4.4.2)
    /// after it. `None` with neither.
    pub fn signature_line(&self) -> Option<(String, bool)> {
        let signed = (self.signature != Signature::None).then(|| self.signature.label());
        let good = matches!(self.signature, Signature::Good(_))
            && self.from_mismatch.is_none()
            && self.uncovered.is_empty();
        let mut warnings: Vec<String> = self.from_mismatch.iter().map(FromMismatch::label).collect();
        if !self.uncovered.is_empty() {
            warnings.push(if self.decrypted {
                t!("manage-openpgp-files-not-encrypted", files = self.uncovered.join(", "))
            } else {
                t!("manage-openpgp-files-not-signed", files = self.uncovered.join(", "))
            });
        }
        let line = signed.into_iter().chain(warnings).collect::<Vec<_>>().join(". ");
        (!line.is_empty()).then_some((line, good))
    }
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
            Self::None => t!("manage-openpgp-not-signed").into(),
            Self::Good(who) => t!("manage-openpgp-signed-by", who = who),
            Self::OtherSender(who) => {
                t!("manage-openpgp-signed-by-who-isnt-the-sender", who = who)
            }
            Self::UnknownKey => t!("manage-openpgp-signed-by-a-key-you-havent").into(),
            Self::Bad => t!("manage-openpgp-the-signature-doesnt-match-the-message").into(),
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

/// Whether `signature` is the key's own (made by its primary key): it names
/// that key as its issuer, or names none (as some older signatures).
fn by_primary(signature: &pgp::packet::Signature, key: &SignedPublicKey) -> bool {
    let fingerprints = signature.issuer_fingerprint();
    let ids = signature.issuer_key_id();
    if fingerprints.is_empty() && ids.is_empty() {
        return true;
    }
    fingerprints.iter().any(|f| **f == key.primary_key.fingerprint())
        || ids.iter().any(|id| **id == key.primary_key.legacy_key_id())
}

/// Whether a self-signature certifies a user id (rather than revoking it).
fn certifies(signature: &pgp::packet::Signature) -> bool {
    use pgp::packet::SignatureType as T;
    matches!(
        signature.typ(),
        Some(T::CertGeneric | T::CertPersona | T::CertCasual | T::CertPositive)
    )
}

/// The user ids the key still has: those whose newest self-signature
/// certifies them. One whose newest is a revocation (RFC 9580 §5.2.1.14,
/// "this user id is no longer valid") isn't the key's any more: a message
/// from that address isn't the key holder's to sign.
fn live_users(key: &SignedPublicKey) -> Vec<String> {
    use pgp::packet::SignatureType as T;
    key.details
        .users
        .iter()
        .filter(|u| {
            let newest = u
                .signatures
                .iter()
                .filter(|s| by_primary(s, key) && (certifies(s) || s.typ() == Some(T::CertRevocation)))
                .max_by_key(|s| s.created().map(|t| t.as_secs()));
            newest.is_none_or(certifies)
        })
        .map(|u| String::from_utf8_lossy(u.id.id()).into_owned())
        .collect()
}

/// Whether a signature over a message may count at all: made with a hash
/// still resistant to collisions (RFC 9580 §9.5: not MD5, SHA-1 or
/// RIPEMD-160, with which one message's signature can be made to cover
/// another), and not past its own expiration time (§5.2.3.18).
fn acceptable_signature(signature: &pgp::packet::Signature) -> bool {
    use pgp::crypto::hash::HashAlgorithm as H;
    if matches!(signature.hash_alg(), None | Some(H::None | H::Md5 | H::Sha1 | H::Ripemd160)) {
        return false;
    }
    let created = signature.created().map_or(0, |t| t.as_secs());
    !past(created, signature.signature_expiration_time())
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
    live_users(key).iter().any(|u| {
        let u = u.to_lowercase();
        let address = match (u.rfind('<'), u.rfind('>')) {
            (Some(a), Some(b)) if a < b => &u[a + 1..b],
            _ => u.trim(),
        };
        address == email
    })
}

/// Keys as parsed, by the SHA-256 of their armored text: lookups (one or
/// more for every message read or downloaded) parsed every key in the
/// keyring each time. Content-addressed, so a keyring changed in any way
/// never reads a stale key; dropped whole when it grows past
/// `PARSED_KEYS`, as keys a keyring no longer has accumulate.
struct Parsed<K> {
    keys: std::sync::LazyLock<std::sync::Mutex<ParsedKeys<K>>>,
}
/// Keys by the SHA-256 of their armored text; `None` for text that isn't one.
type ParsedKeys<K> = std::collections::HashMap<[u8; 32], Option<std::sync::Arc<K>>>;
const PARSED_KEYS: usize = 8192;
static PARSED_PUBLIC: Parsed<SignedPublicKey> = Parsed { keys: std::sync::LazyLock::new(Default::default) };
static PARSED_SECRET: Parsed<SignedSecretKey> = Parsed { keys: std::sync::LazyLock::new(Default::default) };
impl<K> Parsed<K> {
    fn get(&self, armored: &str, parse: impl FnOnce(&str) -> Option<K>) -> Option<std::sync::Arc<K>> {
        use sha2::Digest as _;
        let id: [u8; 32] = sha2::Sha256::digest(armored.as_bytes()).into();
        if let Some(kept) = self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            return kept.clone();
        }
        let key = parse(armored).map(std::sync::Arc::new);
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        if keys.len() >= PARSED_KEYS {
            keys.clear();
        }
        keys.insert(id, key.clone());
        key
    }
}
/// An armored public key as parsed (see `Parsed`).
fn parsed_public(armored: &str) -> Option<std::sync::Arc<SignedPublicKey>> {
    PARSED_PUBLIC.get(armored, |k| SignedPublicKey::from_string(k).ok().map(|(k, _)| k))
}
/// An armored secret key as parsed (see `Parsed`).
fn parsed_secret(armored: &str) -> Option<std::sync::Arc<SignedSecretKey>> {
    PARSED_SECRET.get(armored, |k| SignedSecretKey::from_string(k).ok().map(|(k, _)| k))
}
fn read_public(armored: &str) -> Option<SignedPublicKey> {
    parsed_public(armored).map(|k| SignedPublicKey::clone(&k))
}
fn read_secret(armored: &str) -> Option<SignedSecretKey> {
    parsed_secret(armored).map(|k| SignedSecretKey::clone(&k))
}

/// The most key data an Autocrypt or Autocrypt-Gossip header may carry:
/// a key with a few user ids and subkeys is a few kilobytes, and anyone
/// can send one, to be kept and saved.
pub const AUTOCRYPT_KEY_LIMIT: usize = 64 * 1024;
/// The most keys kept that only Autocrypt gave: each sender's header adds
/// one, so forged senders could grow the keyring without end. The oldest
/// go first.
pub const AUTOCRYPT_KEYS: usize = 1000;

impl Keyring {
    /// The keyring kept in the keychain, or an empty one.
    #[cfg_attr(test, allow(dead_code))]
    pub fn load() -> Result<Self> {
        Ok(match store::read_secret_text(KEYRING)? {
            Some(json) => {
                serde_json::from_str(&json).with_context(|| t!("manage-openpgp-the-openpgp-keyring-couldnt-be-read"))?
            }
            None => Self::default(),
        })
    }
    /// Saves your own part (see `own_part`) to the keychain. Other
    /// people's keys are kept in the database instead (`others_part`).
    #[cfg_attr(test, allow(dead_code))]
    pub fn save(&self) -> Result<()> {
        store::write_secret_text(KEYRING, &serde_json::to_string(&self.own_part())?)
    }
    /// What the keychain keeps: your own keys and choices. Rewritten only
    /// when they change: other people's keys, which every sender's
    /// Autocrypt header can add to, were rewritten into it whole, in parts
    /// of `store::PART` bytes, for each one.
    pub fn own_part(&self) -> Keyring {
        Keyring {
            secret: self.secret.clone(),
            prefer_mutual: self.prefer_mutual.clone(),
            cards: self.cards.clone(),
            ..Keyring::default()
        }
    }
    /// What the database keeps: other people's public keys, and what is
    /// known of where they came from. Nothing in it is secret.
    pub fn others_part(&self) -> Keyring {
        Keyring {
            public: self.public.clone(),
            found: self.found.clone(),
            verified: self.verified.clone(),
            dnssec: self.dnssec.clone(),
            autocrypt: self.autocrypt.clone(),
            ..Keyring::default()
        }
    }
    /// Your own part with `others` (see `others_part`) in place of whatever
    /// of theirs it held.
    pub fn with_others(self, others: Keyring) -> Keyring {
        Keyring {
            public: others.public,
            found: others.found,
            verified: others.verified,
            dnssec: others.dnssec,
            autocrypt: others.autocrypt,
            ..self
        }
    }
    /// Whether any of other people's part is here (a keychain entry an
    /// older Picacho wrote, before it moved to the database).
    pub fn has_others(&self) -> bool {
        *self != self.own_part()
    }
    pub fn secret_keys(&self) -> Vec<SignedSecretKey> {
        self.secret
            .iter()
            .filter_map(|k| read_secret(k))
            .collect()
    }
    /// Everyone's public keys, your own included.
    pub fn public_keys(&self) -> Vec<SignedPublicKey> {
        let mut keys: Vec<SignedPublicKey> = self
            .public
            .iter()
            .filter_map(|k| read_public(k))
            .collect();
        keys.extend(self.secret_keys().into_iter().map(SignedPublicKey::from));
        keys.extend(self.cards.iter().filter_map(card::CardKey::certificate));
        keys
    }
    /// Whether you have a key of your own: here, or on a card.
    pub fn has_own(&self) -> bool {
        !self.secret.is_empty() || !self.cards.is_empty()
    }
    /// Your own public keys: of your keys here, then of your cards.
    pub fn own_public_keys(&self) -> Vec<SignedPublicKey> {
        self.secret_keys()
            .into_iter()
            .map(SignedPublicKey::from)
            .chain(self.cards.iter().filter_map(card::CardKey::certificate))
            .collect()
    }
    /// What can decrypt: your keys here and your cards.
    pub(crate) fn secrets(&self) -> Secrets {
        Secrets { keys: self.secret_keys(), cards: self.cards.clone() }
    }
    /// Your public key for `email`: of a key here, else of a card.
    pub fn own_public(&self, email: &str) -> Option<SignedPublicKey> {
        self.own_key(email)
            .map(SignedPublicKey::from)
            .or_else(|| self.own_card(email).and_then(card::CardKey::certificate))
    }
    /// Your card whose key is for `email`.
    pub fn own_card(&self, email: &str) -> Option<&card::CardKey> {
        self.cards.iter().find(|c| c.is_for(email))
    }
    /// What signs as `email`: your key here, else your card.
    pub(crate) fn signer(&self, email: &str) -> Option<Signer> {
        self.own_key(email)
            .map(|k| Signer::Key(Box::new(k)))
            .or_else(|| self.own_card(email).cloned().map(Signer::Card))
    }
    /// Adds (or replaces) a card's key. Its certificate leaves the public
    /// keys, where it may have been imported before.
    pub fn add_card(&mut self, key: card::CardKey) {
        let fp = key.fingerprint();
        self.cards.retain(|c| c.ident != key.ident && c.fingerprint() != fp);
        self.public.retain(|k| parsed_public(k).is_none_or(|k| fingerprint(&*k) != fp));
        self.found.retain(|f| *f != fp);
        self.cards.push(key);
    }
    /// Other people's public keys as parsed, without copying them.
    fn each_public(&self) -> impl Iterator<Item = std::sync::Arc<SignedPublicKey>> + '_ {
        self.public.iter().filter_map(|k| parsed_public(k))
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
                    expires: keys::expires(&public),
                    revoked: !public.details.revocation_signatures.is_empty(),
                    card: None,
                }
            })
            .collect();
        for c in &self.cards {
            let Some(public) = c.certificate() else { continue };
            out.push(KeyInfo {
                fingerprint: fingerprint(&public),
                users: users(&public),
                secret: true,
                found: false,
                verified: true,
                expires: keys::expires(&public),
                revoked: !public.details.revocation_signatures.is_empty(),
                card: Some(c.name.clone()),
            });
        }
        for k in self
            .public
            .iter()
            .filter_map(|k| read_public(k))
        {
            let fp = fingerprint(&k);
            out.push(KeyInfo {
                expires: keys::expires(&k),
                revoked: !k.details.revocation_signatures.is_empty(),
                found: self.found.contains(&fp),
                verified: !self.found.contains(&fp) || self.checked(&fp),
                fingerprint: fp,
                users: users(&k),
                secret: false,
                card: None,
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
        ensure!(!text.is_empty(), t!("manage-openpgp-paste-or-choose-an-armored-openpgp"));
        if text.contains("PRIVATE KEY BLOCK") {
            let (keys, _) = SignedSecretKey::from_armor_many(text.as_bytes())
                .with_context(|| t!("manage-openpgp-that-secret-key-couldnt-be-read"))?;
            for key in keys {
                let key = key.with_context(|| t!("manage-openpgp-that-secret-key-couldnt-be-read"))?;
                key.verify_bindings()
                    .with_context(|| t!("manage-openpgp-that-keys-signatures-dont-check-out"))?;
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
                .with_context(|| t!("manage-openpgp-that-public-key-couldnt-be-read"))?;
            for key in keys {
                let key = key.with_context(|| t!("manage-openpgp-that-public-key-couldnt-be-read"))?;
                key.verify_bindings()
                    .with_context(|| t!("manage-openpgp-that-keys-signatures-dont-check-out"))?;
                // A revocation certificate for one of your own keys goes
                // onto that key, rather than beside it as someone's.
                if self.revoke_own(&key)? {
                    added += 1;
                    continue;
                }
                let fp = fingerprint(&key);
                // The same key again: merged with what was kept, so a copy
                // from elsewhere never drops a revocation or a user id.
                let earlier = self
                    .public
                    .iter()
                    .filter_map(|k| read_public(k))
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
            t!("manage-openpgp-no-openpgp-keys-were-found-keys")
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
            .with_context(|| t!("manage-openpgp-no-usable-key-for-was-found", email = email))?;
        let fp = fingerprint(&key);
        let kept = self.each_public().any(|k| {
            let f = fingerprint(&*k);
            has_address(&k, email)
                && (!self.found.contains(&f) || (self.checked(&f) && f != fp))
        });
        if kept {
            return Ok(false);
        }
        // The same key kept already: merged with this copy, not replaced
        // by it. A copy is anyone's to send (an Autocrypt header can carry
        // an older one), and replacing would drop the revocations,
        // expiry changes and subkeys only the kept copy has.
        let key = match self.each_public().find(|k| fingerprint(&**k) == fp) {
            Some(old) => merge_public((*old).clone(), key),
            None => key,
        };
        // An earlier found key for the address gives way to this one.
        let found = self.found.clone();
        self.public.retain(|k| {
            parsed_public(k).is_none_or(|k| {
                let f = fingerprint(&*k);
                f != fp && !(found.contains(&f) && has_address(&k, email))
            })
        });
        self.public
            .push(key.to_armored_string(ArmorOptions::default())?);
        if !self.found.contains(&fp) {
            self.found.push(fp);
        }
        // Checks were of keys that may be gone now.
        let kept: std::collections::HashSet<String> =
            self.each_public().map(|k| fingerprint(&*k)).collect();
        self.verified.retain(|f| kept.contains(f));
        self.dnssec.retain(|f| kept.contains(f));
        self.autocrypt.retain(|f| kept.contains(f));
        Ok(true)
    }
    /// Your public key for `email`, binary, as Autocrypt sends it.
    pub fn own_public_bytes(&self, email: &str) -> Option<Vec<u8>> {
        use pgp::ser::Serialize as _;
        self.own_public(email)?.to_bytes().ok()
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
            t!("manage-openpgp-paste-the-whole-fingerprint-40-characters")
        );
        ensure!(
            given == fp.to_ascii_uppercase(),
            t!("manage-openpgp-that-fingerprint-doesnt-match-this-key")
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
            parsed_secret(k).is_none_or(|k| fingerprint(&SignedPublicKey::from(SignedSecretKey::clone(&k))) != fp)
        });
        self.public
            .retain(|k| parsed_public(k).is_none_or(|k| fingerprint(&*k) != fp));
        self.cards.retain(|c| c.fingerprint() != fp);
    }
    /// Your secret key for `email`, for signing.
    pub fn own_key(&self, email: &str) -> Option<SignedSecretKey> {
        self.secret_keys()
            .into_iter()
            .find(|k| has_address(&SignedPublicKey::from(k.clone()), email))
    }
    /// The key to encrypt to for `email`: one its owner hasn't revoked
    /// and that hasn't expired. Other people's keys first, then your own.
    pub fn key_for(&self, email: &str) -> Option<SignedPublicKey> {
        let usable = |k: &SignedPublicKey| {
            has_address(k, email) && k.details.revocation_signatures.is_empty() && !primary_expired(k)
        };
        self.each_public()
            .find(|k| usable(k))
            .map(|k| SignedPublicKey::clone(&k))
            .or_else(|| {
                self.secret_keys()
                    .into_iter()
                    .map(SignedPublicKey::from)
                    .chain(self.cards.iter().filter_map(card::CardKey::certificate))
                    .find(|k| usable(k))
            })
    }
}

/// What decrypts messages to you: your secret keys here, and your cards
/// (used when unlocked; see `card::session_keys`).
pub(crate) struct Secrets {
    pub keys: Vec<SignedSecretKey>,
    pub cards: Vec<card::CardKey>,
}
impl Secrets {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.cards.iter().all(|c| c.decryption.is_none())
    }
}

/// What signs your mail: a key of yours here, or a card.
pub(crate) enum Signer {
    Key(Box<SignedSecretKey>),
    Card(card::CardKey),
}
impl Signer {
    /// Runs `work` with the signing key: the key's signing subkey, or the
    /// card (its PIN kept, `manage/hardware.rs`; else `PinNeeded`).
    pub fn with<T>(&self, work: impl FnOnce(&dyn SigningKey) -> Result<T>) -> Result<T> {
        match self {
            Signer::Key(key) => work(signing_key(key)),
            Signer::Card(key) => {
                let signer = card::CardSigner::open(key)?;
                work(&signer)
            }
        }
    }
    /// Its certificate, which mail encrypted to you also goes to.
    pub fn public(&self) -> Option<SignedPublicKey> {
        match self {
            Signer::Key(key) => Some(SignedPublicKey::from(SignedSecretKey::clone(key))),
            Signer::Card(key) => key.certificate(),
        }
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
            bail!(t!("manage-openpgp-this-key-has-a-passphrase-enter"));
        }
        bail!(t!("manage-openpgp-the-passphrase-is-wrong"));
    }
    if passphrase.is_empty() {
        return Ok(key);
    }
    let mut key = key;
    key.primary_key
        .remove_password(&password)
        .with_context(|| t!("manage-openpgp-the-key-couldnt-be-unlocked"))?;
    for sub in &mut key.secret_subkeys {
        sub.key
            .remove_password(&password)
            .with_context(|| t!("manage-openpgp-a-subkey-couldnt-be-unlocked"))?;
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
            && bound(key, sub)
            && binding.is_some_and(|b| {
                !past(sub.key.created_at().as_secs(), b.key_expiration_time())
                    && b.key_flags().sign()
            })
    })
}
/// Whether `sub` is bound to `key` (its binding signatures check out),
/// remembered by the SHA-256 of the primary key and the subkey with its
/// signatures: each check is a signature verification, and every signature
/// read checked its signer's bindings again. The answer depends on those
/// bytes alone, so a changed key is checked anew; the memory is dropped
/// whole past `PARSED_KEYS` entries.
fn bound(key: &SignedPublicKey, sub: &pgp::composed::SignedPublicSubKey) -> bool {
    use pgp::ser::Serialize as _;
    use sha2::Digest as _;
    static BOUND: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<[u8; 32], bool>>> =
        std::sync::LazyLock::new(Default::default);
    let (Ok(primary), Ok(subkey)) = (key.primary_key.to_bytes(), sub.to_bytes()) else {
        return sub.verify_bindings(&key.primary_key).is_ok();
    };
    let mut hash = sha2::Sha256::new();
    hash.update((primary.len() as u64).to_be_bytes());
    hash.update(&primary);
    hash.update(&subkey);
    let id: [u8; 32] = hash.finalize().into();
    if let Some(known) = BOUND.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
        return *known;
    }
    let ok = sub.verify_bindings(&key.primary_key).is_ok();
    let mut kept = BOUND.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= PARSED_KEYS {
        kept.clear();
    }
    kept.insert(id, ok);
    ok
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
    // The newest self-signature that says how long the key lasts: a
    // certification of a user id, or a direct-key signature. Not a
    // revocation of a user id, which says nothing about it (and, newest,
    // would otherwise make an expired key look current).
    let newest = key
        .details
        .users
        .iter()
        .flat_map(|u| u.signatures.iter())
        .filter(|s| certifies(s))
        .chain(
            key.details
                .direct_signatures
                .iter()
                .filter(|s| s.typ() == Some(pgp::packet::SignatureType::Key)),
        )
        .filter(|s| by_primary(s, key))
        .max_by_key(|s| s.created().map(|t| t.as_secs()));
    newest.is_some_and(|s| past(key.primary_key.created_at().as_secs(), s.key_expiration_time()))
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
    live_users(key)
        .into_iter()
        .next()
        .unwrap_or_else(|| fingerprint(key))
}

/// Checks a detached signature over `content` against your keyring, with
/// the key it names (see `signers`).
fn check(signature: &str, content: &[u8], keys: &[SignedPublicKey], from: Option<&str>) -> Signature {
    let Ok((signature, _)) = DetachedSignature::from_string(signature) else {
        return Signature::Bad;
    };
    let signature = signature.signature;
    signers::verdict(&signature, keys, from, |v| v.verifies(&signature, content))
}

/// Opens a PGP/MIME message with your keys: decrypts it (and checks a
/// signature inside), or checks a multipart/signed one, layer by layer
/// where one is inside another (see `nested`). Returns the readable
/// message, with its outer headers, and what was found; `None` when the
/// message has no OpenPGP protection, can't be decrypted, or has more
/// layers than `nested::MAX_NESTING`.
pub fn open(raw: &[u8], ring: &Keyring) -> Option<(Vec<u8>, Verdict)> {
    nested::open_layers(raw, ring, &mut 0)
}

/// One PGP/MIME layer of `open`: the OpenPGP message inside it unwrapped
/// whole (RFC 9580 §10.3.1), `depth` counting the layers on the way.
fn open_layer(raw: &[u8], ring: &Keyring, depth: &mut usize) -> Option<(Vec<u8>, Verdict)> {
    if !nested::descend(depth) {
        return None;
    }
    // A signature counts as the sender's only from a key for the address
    // the message is from (see `good_from`).
    let from = from_address(raw);
    if let Some((armored, outer)) = crate::manage::crypto::encrypted_part(raw) {
        let secret = ring.secrets();
        if secret.is_empty() {
            return None;
        }
        let (message, _) = Message::from_string(&armored).ok()?;
        let mut opened = nested::unwrap_message(message, &secret, depth)?;
        let public = ring.public_keys();
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
        // Every signed layer's signatures, each checked against the key it
        // names (see `nested::message_signature`); and, for RFC 9788
        // §4.4.1.2, the keys whose signature checked out.
        let (mut signature, signers) = nested::message_signature_and_signers(&opened, &public, from.as_deref());
        let outer = from_latin1(&outer);
        let mut full = outer.clone();
        full.extend_from_slice(&inner);
        // A multipart/signed message inside the encryption (sign, then encrypt).
        if let Some((content, sig)) = signed_parts(&full) {
            signature = nested::combine(
                Verdict { decrypted: true, signature, ..Default::default() },
                Verdict { decrypted: true, signature: check(&sig, &content, &public, from.as_deref()), ..Default::default() },
            )
            .signature;
            let from_mismatch =
                protected::from_mismatch(&outer, &content, |a| signed_by_key_for(&sig, &content, &public, a));
            // RFC 9788 §4.5: the Cryptographic Payload is the signed part,
            // whose headers are the protected ones.
            return Some((
                shown(&outer_headers(&full), &content),
                Verdict {
                    decrypted: true,
                    signature,
                    from_mismatch,
                    uncovered: Vec::new(),
                },
            ));
        }
        let from_mismatch =
            protected::from_mismatch(&outer, &inner, |a| signers.iter().any(|k| has_address(k, a)));
        return Some((
            shown(&outer, &inner),
            Verdict {
                decrypted: true,
                signature,
                from_mismatch,
                uncovered: Vec::new(),
            },
        ));
    }
    match signed_parts(raw) {
        Some((content, sig)) => {
            let public = ring.public_keys();
            let signature = check(&sig, &content, &public, from.as_deref());
            let from_mismatch =
                protected::from_mismatch(raw, &content, |a| signed_by_key_for(&sig, &content, &public, a));
            Some((
                shown(&outer_headers(raw), &content),
                Verdict {
                    decrypted: false,
                    signature,
                    from_mismatch,
                    uncovered: Vec::new(),
                },
            ))
        }
        // Not PGP/MIME: inline PGP, or PGP/MIME a list wrapped in a part.
        None => open_inline(raw, ring, from.as_deref(), depth).or_else(|| open_nested(raw, ring, depth)),
    }
}

/// RFC 9788 §4.4.1.2: whether the detached `signature` over `content` is
/// valid and made by a key whose user id is `address`.
fn signed_by_key_for(signature: &str, content: &[u8], public: &[SignedPublicKey], address: &str) -> bool {
    let keys: Vec<SignedPublicKey> = public.iter().filter(|k| has_address(k, address)).cloned().collect();
    !keys.is_empty() && matches!(check(signature, content, &keys, None), Signature::Good(_))
}

/// The message shown for `outer` (its outer header fields) and `payload`
/// (the root of its Cryptographic Payload). RFC 9788 §4.10: a payload
/// that wraps the message whole (message/rfc822, the older RFC 8551 form)
/// is read as that message, its header fields the protected ones.
fn shown(outer: &[u8], payload: &[u8]) -> Vec<u8> {
    match protected::unwrapped(payload) {
        Some(message) => protected::readable(outer, &message, true),
        None => protected::readable(outer, &shown_payload(payload), false),
    }
}

/// A decrypted Cryptographic Payload as it is shown. RFC 9788 §4.5.3:
/// with Header Protection (an hp parameter on its root), the Legacy
/// Display Elements of parts marked hp-legacy-display="1" are left out;
/// a payload without one is never changed.
fn shown_payload(payload: &[u8]) -> Vec<u8> {
    if protected::has_hp_parameter(payload) {
        legacy_display::without_elements(payload)
    } else {
        payload.to_vec()
    }
}

/// Inline PGP (RFC 4880 §7, the old way): a plain-text body that is a
/// cleartext-signed message or an armored encrypted one. The readable
/// message is its headers with the text as a plain UTF-8 body, and its
/// files after it (see `inline::attachments`).
fn open_inline(raw: &[u8], ring: &Keyring, from: Option<&str>, depth: &mut usize) -> Option<(Vec<u8>, Verdict)> {
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
    // The message shown, and the names of its files that its protection
    // doesn't cover (see `inline::attachments`).
    let readable = |text: &str,
                    depth: usize,
                    signed: &dyn Fn(&[u8], &[u8]) -> bool,
                    sealed: &dyn Fn(&Message<'_>) -> bool| {
        let mut out = outer_headers(raw);
        let mut part = b"Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n".to_vec();
        part.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
        let files = if parsed.subparts.is_empty() {
            inline::Files { parts: Vec::new(), uncovered: Vec::new() }
        } else {
            inline::attachments(&parsed, text_part, &ring.secrets(), depth, signed, sealed)
        };
        if files.parts.is_empty() {
            out.extend_from_slice(&part);
        } else {
            out.extend_from_slice(&inline::mixed(std::iter::once(part).chain(files.parts).collect()));
        }
        (out, files.uncovered)
    };
    if let Some(start) = body.find("-----BEGIN PGP SIGNED MESSAGE-----") {
        let end = body[start..].find("-----END PGP SIGNATURE-----")? + start + 27;
        let (message, _) = CleartextSignedMessage::from_string(&body[start..end]).ok()?;
        let public = ring.public_keys();
        let signed = message.signed_text();
        // Good when one of its signatures is good, else what the first says.
        let verdicts: Vec<Signature> = message
            .signatures()
            .iter()
            .map(|sig| signers::verdict(sig, &public, from, |v| v.verifies(sig, signed.as_bytes())))
            .collect();
        let signature = verdicts
            .iter()
            .find(|s| matches!(s, Signature::Good(_) | Signature::OtherSender(_)))
            .or(verdicts.first())
            .cloned()
            .unwrap_or(Signature::UnknownKey);
        // A file is covered by a signature beside it that says what the
        // text's does: made by the same key, which counted.
        let covers = |file: &[u8], sig: &[u8]| {
            matches!(signature, Signature::Good(_) | Signature::OtherSender(_))
                && inline::detached(sig).is_some_and(|s| {
                    signers::verdict(&s, &public, from, |v| v.verifies(&s, file)) == signature
                })
        };
        // An encrypted file is covered only if signed as the text is: this
        // message isn't encrypted, so anyone could have added it.
        let sealed = |m: &Message<'_>| {
            matches!(signature, Signature::Good(_) | Signature::OtherSender(_))
                && nested::message_signature(m, &public, from) == signature
        };
        // Text around the signed block isn't covered by it: left out.
        let (shown, uncovered) = readable(&signed, *depth, &covers, &sealed);
        return Some((
            shown,
            Verdict {
                decrypted: false,
                signature,
                uncovered,
                ..Default::default()
            },
        ));
    }
    if let Some(start) = body.find("-----BEGIN PGP MESSAGE-----") {
        let end = body[start..].find("-----END PGP MESSAGE-----")? + start + 25;
        let secret = ring.secrets();
        if secret.is_empty() {
            return None;
        }
        let (message, _) = Message::from_string(&body[start..end]).ok()?;
        let mut opened = nested::unwrap_message(message, &secret, depth)?;
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
        let public = ring.public_keys();
        let signature = nested::message_signature(&opened, &public, from);
        // Only files that were encrypted too are covered; under a signature,
        // only those signed as the text is (anyone can encrypt a file to
        // you and add it on the way).
        let sealed = |m: &Message<'_>| match &signature {
            Signature::Good(_) | Signature::OtherSender(_) => nested::message_signature(m, &public, from) == signature,
            _ => true,
        };
        let (shown, uncovered) = readable(&String::from_utf8_lossy(&inner), *depth, &|_, _| false, &sealed);
        return Some((
            shown,
            Verdict {
                decrypted: true,
                signature,
                uncovered,
                ..Default::default()
            },
        ));
    }
    None
}

/// PGP/MIME one level down: a mailing list's multipart/mixed wrapping the
/// signed or encrypted message with its footer. The message is opened as
/// its own; the footer, not covered by the signature, is left out.
fn open_nested(raw: &[u8], ring: &Keyring, depth: &mut usize) -> Option<(Vec<u8>, Verdict)> {
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
    nested::open_layers(&inner, ring, depth)
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
    /// With the subject hidden, repeat it (and Reply-To) at the top of the
    /// text for apps that decrypt but don't read protected headers: a
    /// Legacy Display Element (RFC 9788 §5.2.2). Off, with no setting.
    #[serde(default)]
    pub legacy_display: bool,
    /// Inline PGP (RFC 4880 §7) rather than PGP/MIME: the text itself
    /// cleartext-signed or armored, each attachment encrypted or signed
    /// on its own, for apps that read only that. Plain text only; no
    /// protected headers, so `subject` and `legacy_display` don't apply.
    #[serde(default)]
    pub inline: bool,
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

fn boundary() -> String {
    format!("=-picacho-{}", uuid::Uuid::new_v4().simple())
}

/// Signs and/or encrypts a finished message as PGP/MIME (or inline PGP,
/// with `how.inline`). `from` is the
/// sender's address (whose key signs), `to` every recipient (each must have
/// a key to encrypt). The sender's own key is always an encryption
/// recipient too, so the copy in Sent stays readable.
/// The addresses a message's To and Cc headers show, lowercase: not its
/// Bcc recipients.
fn shown_recipients(raw: &[u8]) -> std::collections::HashSet<String> {
    use mailparse::MailHeaderMap as _;
    let Ok((headers, _)) = mailparse::parse_headers(raw) else {
        return Default::default();
    };
    ["To", "Cc"]
        .iter()
        .flat_map(|name| headers.get_all_values(name))
        .flat_map(|list| crate::sync::smtp::recipients(&list).unwrap_or_default())
        .map(|(_, address)| address.trim().to_lowercase())
        .collect()
}

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
    if how.inline {
        return inline::protect(raw, from, to, how, ring);
    }
    let (mut part, outer) = split_body(raw);
    // The line break before the next boundary belongs to the boundary
    // (RFC 3156 §5): what is signed ends without that one; the body's own
    // blank lines are its own.
    if part.ends_with(b"\r\n") {
        part.truncate(part.len() - 2);
    }
    // RFC 9788 §5.2.1: the inner headers on the Cryptographic Payload,
    // hp="cipher" when encrypted and hp="clear" when signed only. The
    // outside changes only when the subject is hidden.
    let part = if how.legacy_display {
        let fields = protected::legacy_display_fields(&outer, how.encrypt, how.subject);
        legacy_display::with_elements(&part, &fields)
    } else {
        part
    };
    let (part, outer) = protected::compose(&part, &outer, how.encrypt, how.subject);
    // Encrypted to several people: each one's key goes inside, so they can
    // write to the others encrypted too (Autocrypt-Gossip, Autocrypt 1.1
    // §3.6). Only those the message shows (To and Cc): `to` is everyone it
    // goes to, Bcc too, whose addresses and keys would otherwise be
    // handed to every other recipient.
    let visible = shown_recipients(raw);
    let gossip_to: Vec<&String> = to
        .iter()
        .filter(|addr| visible.contains(&addr.trim().to_lowercase()))
        .collect();
    let part = if how.encrypt && gossip_to.len() > 1 {
        let gossip: String = gossip_to
            .into_iter()
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
    let own = ring.signer(from);
    let mut content = part.clone();
    if how.sign {
        let key = own.as_ref().with_context(|| {
            t!("manage-openpgp-theres-no-openpgp-key-of-yours", from = from)
        })?;
        let signature = key
            .with(|signer| {
                Ok(DetachedSignature::sign_binary_data(
                    rand08::thread_rng(),
                    &Box::new(signer),
                    &Password::empty(),
                    HashAlgorithm::Sha256,
                    &part[..],
                )?)
            })?
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
        let keys = recipient_keys(to, own.as_ref().and_then(Signer::public), ring)?;
        let armored = encrypt(content, &keys, None, true)?;
        let armored = String::from_utf8(armored)?;
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

/// The key of yours that signs: a subkey made for signing, as most keys
/// have; else the primary key.
fn signing_key(key: &SignedSecretKey) -> &dyn SigningKey {
    match key
        .secret_subkeys
        .iter()
        .find(|s| s.signatures.iter().any(|sig| sig.key_flags().sign()))
    {
        Some(sub) => &sub.key,
        None => &key.primary_key,
    }
}

/// The keys a message is encrypted to: every recipient's (each must have
/// one), and your own, so the copy in Sent stays readable.
fn recipient_keys(to: &[String], own: Option<SignedPublicKey>, ring: &Keyring) -> Result<Vec<SignedPublicKey>> {
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
        t!("manage-openpgp-theres-no-openpgp-key-for-import", addresses = missing.join(", "))
    );
    if let Some(own) = own {
        keys.push(own);
    }
    Ok(keys)
}

/// `content` encrypted to `keys` as one OpenPGP message, signed inside it
/// by `signer` when given (one-pass signed, RFC 9580 §10.3): armored text,
/// or binary packets.
fn encrypt(
    content: Vec<u8>,
    keys: &[SignedPublicKey],
    signer: Option<&dyn SigningKey>,
    armored: bool,
) -> Result<Vec<u8>> {
    // Each recipient's key to encrypt to: an encryption subkey bound to
    // it, neither revoked nor expired, else the key itself.
    macro_rules! finish {
        ($builder:expr) => {{
            let mut builder = $builder;
            if let Some(signer) = signer {
                builder.sign(signer, Password::empty(), HashAlgorithm::Sha256);
            }
            for key in keys {
                match encryption_subkey(key) {
                    Some(sub) => builder.encrypt_to_key(rand08::thread_rng(), &sub.key)?,
                    None => builder.encrypt_to_key(rand08::thread_rng(), &key.primary_key)?,
                };
            }
            if armored {
                builder.to_armored_string(rand08::thread_rng(), ArmorOptions::default())?.into_bytes()
            } else {
                builder.to_vec(rand08::thread_rng())?
            }
        }};
    }
    // RFC 9580's AEAD encryption (SEIPD v2) where every recipient's key
    // says it reads it; else SEIPD v1, which every OpenPGP app reads
    // (§13.7). One message for all: §10.3.2.1 doesn't let v3 and v6
    // PKESKs share a payload, so one v4-only recipient means v1.
    Ok(if keys.iter().all(reads_seipd_v2) {
        let (sym, aead) = seipd_v2_suite(keys);
        finish!(MessageBuilder::from_bytes("", content).seipd_v2(
            rand08::thread_rng(),
            sym,
            aead,
            pgp::crypto::aead::ChunkSize::default(),
        ))
    } else {
        finish!(MessageBuilder::from_bytes("", content).seipd_v1(rand08::thread_rng(), preferred_cipher(keys)))
    })
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
/// The self-signature whose preferences count for a key: for a v6 key its
/// newest direct-key signature, where RFC 9580 §5.2.3.10 and §10.1.1 keep
/// a v6 key's features and preferences (its user ids' bindings may be
/// newer and say none); else the newest self-signature.
fn preference_signature(key: &SignedPublicKey) -> Option<&pgp::packet::Signature> {
    let direct = || {
        key.details
            .direct_signatures
            .iter()
            .max_by_key(|s| s.created().map(|t| t.as_secs()))
    };
    match key.primary_key.version() {
        pgp::types::KeyVersion::V6 => direct().or_else(|| newest_self_signature(key)),
        _ => newest_self_signature(key),
    }
}
/// Whether a key reads AEAD-encrypted data (SEIPD v2, RFC 9580 §5.13.2):
/// its Features say so (§5.2.3.32), or it is a v6 key with no Features
/// subpacket (§13.7).
fn reads_seipd_v2(key: &SignedPublicKey) -> bool {
    let signature = preference_signature(key);
    match signature.and_then(|s| s.features()) {
        Some(features) => features.seipd_v2(),
        None => key.primary_key.version() == pgp::types::KeyVersion::V6,
    }
}
/// The SEIPD v2 ciphersuite for `keys`, which all read SEIPD v2: AES-256
/// with OCB where every key lists it, else AES-128 with OCB, which every
/// key takes even unlisted (RFC 9580 §5.2.3.15).
fn seipd_v2_suite(keys: &[SignedPublicKey]) -> (SymmetricKeyAlgorithm, pgp::crypto::aead::AeadAlgorithm) {
    use pgp::crypto::aead::AeadAlgorithm;
    let strong = (SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb);
    let all_list_strong = keys
        .iter()
        .all(|k| preference_signature(k).is_some_and(|s| s.preferred_aead_algs().contains(&strong)));
    if all_list_strong {
        strong
    } else {
        (SymmetricKeyAlgorithm::AES128, AeadAlgorithm::Ocb)
    }
}
/// The strongest cipher every recipient lists (RFC 9580 §13.2), AES-256
/// where all take it or none says; AES-128 is every key's fallback.
fn preferred_cipher(keys: &[SignedPublicKey]) -> SymmetricKeyAlgorithm {
    let lists: Vec<&[SymmetricKeyAlgorithm]> = keys
        .iter()
        .filter_map(preference_signature)
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

/// The OpenPGP version of a key Picacho makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyVersion {
    /// Version 4 (RFC 9580 §5.5.2.2), which every OpenPGP app reads.
    #[default]
    V4,
    /// Version 6 (RFC 9580 §5.5.2.3), which GnuPG 2.4 and older and most
    /// mail apps can't use yet.
    V6,
}

/// A new key of yours for `name <email>`. Version 4 is made as GnuPG makes
/// them (Ed25519 certifying primary, Ed25519 signing and Curve25519
/// encryption subkeys); version 6 as `v6_key_params` says. It expires
/// `lasts` from now (RFC 9580 §5.2.3.13, see `keys::set_expiry`), or never
/// with `None`.
pub fn generate_key(
    name: &str,
    email: &str,
    lasts: Option<std::time::Duration>,
    version: KeyVersion,
) -> Result<SignedSecretKey> {
    use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use pgp::crypto::ecc_curve::ECCCurve;
    ensure!(email.contains('@'), t!("manage-openpgp-enter-the-address-the-key-is"));
    let name = name.replace(['<', '>', '\r', '\n'], "");
    let uid = if name.trim().is_empty() {
        format!("<{}>", email.trim())
    } else {
        format!("{} <{}>", name.trim(), email.trim())
    };
    if version == KeyVersion::V6 {
        let key = v6_key_params(uid)?
            .generate(rand08::thread_rng())
            .with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?;
        return keys::expiring(key, lasts);
    }
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
            sign.build().with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?,
            encrypt.build().with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?,
        ]);
    let key = params
        .build()
        .with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?
        .generate(rand08::thread_rng())
        .with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?;
    keys::expiring(key, lasts)
}
/// A version 6 key for `uid` (RFC 9580 §5.5.2.3): an Ed25519 certifying
/// primary key, an Ed25519 signing subkey and an X25519 encryption subkey
/// (the v6 algorithms of §5.5.5.9 and §5.5.5.7, not the legacy EdDSA and
/// ECDH ones). Its preferences go on its direct-key signature (§5.2.3.10,
/// as pgp does for v6 keys): AES-256 then AES-128 for SEIPD v1
/// (§5.2.3.14), AES-256 then AES-128 with OCB for SEIPD v2 (§5.2.3.15),
/// and the Features subpacket saying it reads both (§5.2.3.32), so
/// senders whose keys all say so use AEAD.
fn v6_key_params(uid: String) -> Result<pgp::composed::SecretKeyParams> {
    use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use pgp::crypto::aead::AeadAlgorithm;
    use pgp::types::KeyVersion::V6;
    let mut sign = SubkeyParamsBuilder::default();
    sign.version(V6)
        .key_type(KeyType::Ed25519)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None);
    let mut encrypt = SubkeyParamsBuilder::default();
    encrypt
        .version(V6)
        .key_type(KeyType::X25519)
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .version(V6)
        .key_type(KeyType::Ed25519)
        .can_certify(true)
        .can_sign(false)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id(uid)
        .preferred_symmetric_algorithms(
            vec![SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128].into(),
        )
        .preferred_aead_algorithms(
            vec![
                (SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb),
                (SymmetricKeyAlgorithm::AES128, AeadAlgorithm::Ocb),
            ]
            .into(),
        )
        .feature_seipd_v1(true)
        .feature_seipd_v2(true)
        .subkeys(vec![
            sign.build().with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?,
            encrypt.build().with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))?,
        ]);
    params.build().with_context(|| t!("manage-openpgp-the-key-couldnt-be-made"))
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
        let armored = setup_message_payload(raw).with_context(|| t!("manage-openpgp-this-isnt-an-autocrypt-setup-message"))?;
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        ensure!(digits.len() == 36, t!("manage-openpgp-enter-the-setup-codes-36-digits"));
        // The code as it was made: nine groups of four, dashes between.
        let code = digits
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-");
        let (message, _) = Message::from_string(&armored).with_context(|| t!("manage-openpgp-the-setup-message-cant-be-read"))?;
        let mut opened = message
            .decrypt_with_password(&Password::from(code.as_str()))
            .map_err(|_| anyhow::anyhow!(t!("manage-openpgp-that-setup-code-doesnt-open-it")))?;
        let mut text = String::new();
        {
            use std::io::Read as _;
            (&mut opened)
                .take(1 << 20)
                .read_to_string(&mut text)
                .with_context(|| t!("manage-openpgp-the-setup-message-cant-be-read"))?;
        }
        self.import(&text, "")
    }
    /// Makes a new key of yours for `email`, of `version` and expiring
    /// `lasts` from now or never, and keeps it (see `generate_key`).
    /// Returns its fingerprint.
    pub fn create(
        &mut self,
        name: &str,
        email: &str,
        lasts: Option<std::time::Duration>,
        version: KeyVersion,
    ) -> Result<String> {
        let key = generate_key(name, email, lasts, version)?;
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
            ..Default::default()
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
            ..Default::default()
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
        let both: &[u8] = b"From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nCc: cy@example.net\r\nSubject: Plans\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nMeet at noon.\r\n";
        let sent = protect(both, "ana@example.org", &to, how, &anas).unwrap();
        assert!(!String::from_utf8_lossy(&sent).contains("Autocrypt-Gossip"));
        let inner = payload(&sent, &cy);
        let gossip = crate::sync::keys::autocrypt_gossip(&inner, &to);
        let cy_bytes = SignedPublicKey::from(cy.clone()).to_bytes().unwrap();
        let bo_bytes = SignedPublicKey::from(bo.clone()).to_bytes().unwrap();
        assert_eq!(gossip, vec![("bo@example.se".into(), bo_bytes), ("cy@example.net".into(), cy_bytes)]);
        // One recipient: nothing to gossip.
        let alone = protect(MESSAGE, "ana@example.org", &to[..1], how, &anas).unwrap();
        assert!(!String::from_utf8_lossy(&payload(&alone, &bo)).contains("Autocrypt-Gossip"));
        // Cy in Bcc (in `to`, not the headers): neither Cy's address nor key
        // goes to Bo, nor Bo's to Cy.
        let hidden = protect(MESSAGE, "ana@example.org", &to, how, &anas).unwrap();
        for reader in [&bo, &cy] {
            let inner = String::from_utf8_lossy(&payload(&hidden, reader)).into_owned();
            assert!(!inner.contains("Autocrypt-Gossip") && !inner.contains("cy@example.net"), "{inner}");
        }
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
