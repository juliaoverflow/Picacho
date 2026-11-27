//! Security keys that hold private keys themselves: OpenPGP cards
//! (YubiKey, Nitrokey, Gnuk…, `openpgp/card.rs`) and PKCS#11 tokens for
//! S/MIME (`smime/token.rs`). Signing and decrypting happen on the key; the
//! private key is never read out, nor kept in the keychain. Only the public
//! part (a certificate) and where to find the key are kept, with your other
//! keys.
//!
//! A key's PIN is asked for in Settings → Encryption, or by the reader when
//! a message needs the key, and kept in memory only (`remember_pin`) until
//! it hasn't been used for `PIN_IDLE`, the app locks, or Picacho quits.
//! Messages are read, and mail is signed, with it meanwhile; without it, a
//! message encrypted to the key stays unreadable and the reader offers to
//! unlock the key. Nothing here asks for a PIN itself.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// How long an unused PIN is kept.
pub const PIN_IDLE: Duration = Duration::from_secs(30 * 60);

/// Each key's PIN, and when it was last used.
type Pins = HashMap<String, (Zeroizing<String>, Instant)>;
static PINS: LazyLock<Mutex<Pins>> = LazyLock::new(Default::default);

fn pins() -> std::sync::MutexGuard<'static, Pins> {
    PINS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps a security key's PIN (checked already) for `PIN_IDLE`. `key` is
/// the key's id (`openpgp/card.rs`'s `pin_id`, `smime/token.rs`'s).
pub fn remember_pin(key: &str, pin: &str) {
    pins().insert(key.to_owned(), (Zeroizing::new(pin.to_owned()), Instant::now()));
}

/// The PIN kept for a key, if it is still fresh; using it keeps it longer.
pub fn pin(key: &str) -> Option<Zeroizing<String>> {
    let mut pins = pins();
    let now = Instant::now();
    pins.retain(|_, (_, at)| now.duration_since(*at) < PIN_IDLE);
    let (pin, at) = pins.get_mut(key)?;
    *at = now;
    Some(pin.clone())
}

/// Whether a key's PIN is kept now.
pub fn unlocked(key: &str) -> bool {
    let pins = pins();
    pins.get(key).is_some_and(|(_, at)| at.elapsed() < PIN_IDLE)
}

/// Forgets one key's PIN (Lock in Settings, or a PIN the key refused).
pub fn forget_pin(key: &str) {
    pins().remove(key);
}

/// Forgets every PIN: the app locked.
pub fn forget_pins() {
    pins().clear();
}

/// A security key's PIN is needed: what the reader and Settings offer to
/// unlock. Carries the key's id and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinNeeded {
    pub key: String,
    pub name: String,
}
impl std::fmt::Display for PinNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("security-keys-unlock-to-use", name = self.name.clone()))
    }
}
impl std::error::Error for PinNeeded {}

/// One security key, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityKey {
    /// The PIN's id.
    pub id: String,
    pub kind: Kind,
    /// What it is called: the card's holder or maker, the token's label.
    pub name: String,
    /// The addresses its keys are for.
    pub addresses: Vec<String>,
    pub unlocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An OpenPGP card, for OpenPGP.
    OpenPgpCard,
    /// A PKCS#11 token, for S/MIME.
    Pkcs11,
}

/// Every security key known: the OpenPGP keyring's cards, then the S/MIME
/// tokens, each once (one token can hold several identities).
pub fn listed(ring: &crate::manage::openpgp::Keyring, certs: &crate::manage::smime::Certificates) -> Vec<SecurityKey> {
    let mut out: Vec<SecurityKey> = Vec::new();
    for card in &ring.cards {
        let id = crate::manage::openpgp::card::pin_id(&card.ident);
        if let Some(known) = out.iter_mut().find(|k| k.id == id) {
            known.addresses.extend(card.addresses());
            continue;
        }
        out.push(SecurityKey {
            unlocked: unlocked(&id),
            id,
            kind: Kind::OpenPgpCard,
            name: card.name.clone(),
            addresses: card.addresses(),
        });
    }
    for token in &certs.tokens {
        let id = token.pin_id();
        let addresses = token.addresses();
        if let Some(known) = out.iter_mut().find(|k| k.id == id) {
            known.addresses.extend(addresses);
            continue;
        }
        out.push(SecurityKey {
            unlocked: unlocked(&id),
            id,
            kind: Kind::Pkcs11,
            name: token.label.clone(),
            addresses,
        });
    }
    for key in &mut out {
        key.addresses.sort();
        key.addresses.dedup();
    }
    out
}

impl crate::manage::backend::Backend {
    /// Your security keys, for Settings.
    pub fn security_keys(&self) -> Vec<SecurityKey> {
        listed(&self.keyring(), &self.certificates())
    }
    /// The OpenPGP cards connected now, not added yet.
    pub fn connected_cards(&self) -> anyhow::Result<Vec<crate::manage::openpgp::card::CardInfo>> {
        let ring = self.keyring();
        Ok(crate::manage::openpgp::card::connected()?
            .into_iter()
            .filter(|c| !ring.cards.iter().any(|k| k.ident == c.ident))
            .collect())
    }
    /// Adds a connected card as your OpenPGP key: its certificate is found
    /// among the keys here, at the card's URL, or on keys.openpgp.org by
    /// the card's fingerprints, else in `pasted` (armored). With `pin`,
    /// the card is unlocked too.
    pub fn add_card(
        &self,
        card: &crate::manage::openpgp::card::CardInfo,
        pasted: &str,
        pin: &str,
    ) -> anyhow::Result<crate::manage::openpgp::card::CardKey> {
        use crate::manage::openpgp::card;
        let ring = self.keyring();
        let mut candidates: Vec<Vec<u8>> = ring.public.iter().map(|k| k.clone().into_bytes()).collect();
        if !pasted.trim().is_empty() {
            candidates.insert(0, pasted.trim().as_bytes().to_vec());
        }
        let mut certificate = card::certificate_for(card, &candidates);
        if certificate.is_none() && !cfg!(test) {
            let mut fetched = Vec::new();
            if !card.url.trim().is_empty()
                && let Ok(tls) = self.tls_connector_pub()
                && let Ok(Some(data)) = crate::sync::keys::fetch_url(&card.url, tls)
            {
                fetched.push(data);
            }
            for fp in card.signing.iter().chain(card.decryption.iter()) {
                if let Ok(tls) = self.tls_connector_pub()
                    && let Ok(Some(data)) = crate::sync::keys::lookup_fingerprint(fp, tls)
                {
                    fetched.push(data);
                }
            }
            certificate = card::certificate_for(card, &fetched);
        }
        let certificate = certificate.ok_or_else(|| anyhow::anyhow!(t!("security-keys-paste-the-certificate")))?;
        let key = card::card_key(card, &certificate)?;
        if !pin.is_empty() {
            card::unlock(&card.ident, pin)?;
        }
        let mut next = crate::manage::openpgp::Keyring::clone(&ring);
        next.add_card(key.clone());
        self.set_keyring(next)?;
        Ok(key)
    }
    /// Adds the S/MIME identities on the tokens a PKCS#11 module reaches,
    /// checking (and keeping) their PIN.
    pub fn add_token_identities(&self, module: &str, pin: &str) -> anyhow::Result<Vec<crate::manage::smime::CertInfo>> {
        let found = crate::manage::smime::token::discover(module.trim(), pin)?;
        let mut certs = crate::manage::smime::Certificates::clone(&self.certificates());
        let added = certs.add_tokens(found);
        self.set_certificates(certs)?;
        Ok(added)
    }
    /// Checks a security key's PIN and keeps it; messages that couldn't be
    /// read without it are read again.
    pub fn unlock_security_key(&self, id: &str, pin: &str) -> anyhow::Result<()> {
        use crate::manage::openpgp::card;
        let ring = self.keyring();
        if let Some(c) = ring.cards.iter().find(|c| card::pin_id(&c.ident) == id) {
            card::unlock(&c.ident, pin)?;
        } else {
            let certs = self.certificates();
            let token = certs
                .tokens
                .iter()
                .find(|t| t.pin_id() == id)
                .ok_or_else(|| anyhow::anyhow!(t!("security-keys-no-longer-added")))?;
            crate::manage::smime::token::unlock(token, pin)?;
        }
        self.forget_verdicts();
        Ok(())
    }
    /// Forgets a security key's PIN.
    pub fn lock_security_key(&self, id: &str) {
        forget_pin(id);
    }
    /// Removes a security key: the card's keys from the keyring, the
    /// token's identities from your certificates. The keys stay on it.
    pub fn remove_security_key(&self, id: &str) -> anyhow::Result<()> {
        use crate::manage::openpgp::card;
        forget_pin(id);
        let ring = self.keyring();
        if ring.cards.iter().any(|c| card::pin_id(&c.ident) == id) {
            let mut next = crate::manage::openpgp::Keyring::clone(&ring);
            next.cards.retain(|c| card::pin_id(&c.ident) != id);
            self.set_keyring(next)?;
        }
        let certs = self.certificates();
        if certs.tokens.iter().any(|t| t.pin_id() == id) {
            let mut next = crate::manage::smime::Certificates::clone(&certs);
            next.tokens.retain(|t| t.pin_id() != id);
            self.set_certificates(next)?;
        }
        Ok(())
    }
    /// Whether a message stays encrypted for want of a locked security key:
    /// it is encrypted, wasn't decrypted, and a security key is locked.
    pub fn needs_security_key(&self, m: &crate::manage::model::Mail) -> bool {
        if self.locked_security_keys().is_empty() {
            return false;
        }
        if self.verdict(&m.id).is_some_and(|v| v.decrypted) || self.smime_verdict(&m.id).is_some_and(|v| v.decrypted) {
            return false;
        }
        self.store
            .raw(&m.id)
            .ok()
            .flatten()
            .is_some_and(|raw| crate::manage::crypto::is_encrypted(&raw) && !crate::manage::crypto::sender_encrypted(&raw))
    }
    /// The locked security keys that hold a key a message may be
    /// encrypted to: the reader offers to unlock them for an unreadable one.
    pub fn locked_security_keys(&self) -> Vec<SecurityKey> {
        self.security_keys().into_iter().filter(|k| !k.unlocked).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_kept_until_forgotten() {
        let key = format!("test:{}", uuid::Uuid::new_v4());
        assert!(pin(&key).is_none() && !unlocked(&key));
        remember_pin(&key, "123456");
        assert_eq!(pin(&key).as_deref().map(String::as_str), Some("123456"));
        assert!(unlocked(&key));
        forget_pin(&key);
        assert!(pin(&key).is_none());
        remember_pin(&key, "654321");
        forget_pins();
        assert!(!unlocked(&key));
    }

    #[test]
    fn security_keys_are_listed_once_per_key_and_removed() {
        use crate::manage::backend::Backend;
        use crate::manage::openpgp::{card::CardKey, test_key};
        let dir = std::env::temp_dir().join(format!("picacho-hardware-{}", uuid::Uuid::new_v4().simple()));
        let keys: std::sync::Arc<crate::manage::vault::MemoryKeys> = std::sync::Arc::default();
        let store = crate::manage::store::Store::open_with_keys(&dir, keys).unwrap();
        let b = Backend::new(std::sync::Arc::new(store));
        let public = pgp::composed::SignedPublicKey::from(test_key("Ana <ana@example.org>", ""));
        let card = CardKey {
            ident: "0006:00000001".into(),
            public: public.to_armored_string(Default::default()).unwrap(),
            name: "Ana's key".into(),
            ..CardKey::default()
        };
        let mut ring = crate::manage::openpgp::Keyring::default();
        ring.add_card(card.clone());
        b.set_keyring(ring).unwrap();
        let listed = b.security_keys();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, Kind::OpenPgpCard);
        assert_eq!(listed[0].addresses, ["ana@example.org"]);
        assert!(!listed[0].unlocked);
        // Its certificate counts as your key: mail is encrypted to it, and
        // Autocrypt offers it.
        assert!(b.keyring().own_public("ana@example.org").is_some());
        assert!(b.keyring().key_for("ana@example.org").is_some());
        assert!(b.keyring().own_key("ana@example.org").is_none(), "nothing secret is kept");
        b.remove_security_key(&listed[0].id).unwrap();
        assert!(b.security_keys().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
