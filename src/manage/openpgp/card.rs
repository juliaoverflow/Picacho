//! OpenPGP cards (OpenPGP Card 3.x over PC/SC, `sync/pcsc.rs`): your key's
//! secret parts stay on the card, which signs and decrypts when given its
//! PIN (`manage/hardware.rs`). The keyring keeps only the key's
//! certificate and which card holds it (`CardKey`).
use super::{fingerprint, has_address, read_public, users};
use anyhow::{Context, Result, bail, ensure};
use openpgp_card::Card;
use openpgp_card::ocard::KeyType;
use openpgp_card::state::Open;
use openpgp_card_rpgp::CardSlot;
use pgp::composed::{Deserializable, Message, PlainSessionKey, SignedPublicKey};
use pgp::crypto::hash::HashAlgorithm;
use pgp::packet::PublicKey;
use pgp::types::{
    DecryptionKey, EskType, Fingerprint, KeyDetails, KeyId, KeyVersion, Password, PublicParams, SignatureBytes,
    SigningKey, Timestamp,
};
use serde::{Deserialize, Serialize};

/// One of your keys on an OpenPGP card.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardKey {
    /// The card's identity (`MMMM:SSSSSSSS`, its maker and serial number).
    pub ident: String,
    /// The key's certificate, armored: public only.
    pub public: String,
    /// The fingerprints (hex, upper case) of the card's signing and
    /// decryption keys, which are the certificate's (sub)keys.
    #[serde(default)]
    pub signing: Option<String>,
    #[serde(default)]
    pub decryption: Option<String>,
    /// The card's name for people: its holder, else its maker and serial.
    #[serde(default)]
    pub name: String,
}
impl CardKey {
    pub fn certificate(&self) -> Option<SignedPublicKey> {
        read_public(&self.public)
    }
    /// The certificate's fingerprint.
    pub fn fingerprint(&self) -> String {
        self.certificate().map(|k| fingerprint(&k)).unwrap_or_default()
    }
    pub fn addresses(&self) -> Vec<String> {
        self.certificate()
            .map(|k| {
                users(&k)
                    .iter()
                    .filter_map(|u| crate::sync::smtp::recipients(u).ok()?.into_iter().next().map(|(_, a)| a.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn is_for(&self, email: &str) -> bool {
        self.certificate().is_some_and(|k| has_address(&k, email))
    }
    /// The legacy key id of its decryption key, as PKESKs name it.
    fn decryption_id(&self) -> Option<String> {
        self.decryption.as_ref().map(|fp| fp[fp.len().saturating_sub(16)..].to_owned())
    }
}

/// The PIN's id of a card (`manage/hardware.rs`).
pub fn pin_id(ident: &str) -> String {
    format!("openpgp-card:{}", ident.to_ascii_uppercase())
}

/// A card connected now, as Settings offers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardInfo {
    pub ident: String,
    pub name: String,
    pub signing: Option<String>,
    pub decryption: Option<String>,
    /// The card's URL of its public key, if it has one.
    pub url: String,
    /// PIN tries left before the card blocks its PIN.
    pub tries: Option<u8>,
}

/// The OpenPGP cards connected now.
pub fn connected() -> Result<Vec<CardInfo>> {
    let backends = crate::sync::pcsc::cards().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut out = Vec::new();
    for backend in backends {
        // Cards without the OpenPGP application (a bank card, a PIV-only
        // token) are left out.
        let Ok(mut card) = Card::new(backend) else { continue };
        let Ok(mut tx) = card.transaction() else { continue };
        let Ok(aid) = tx.application_identifier() else { continue };
        let ident = aid.ident();
        let fps = tx.fingerprints().ok();
        let holder = tx.cardholder_name().unwrap_or_default();
        let name = if holder.trim().is_empty() {
            format!("{} {}", aid.manufacturer_name(), ident)
        } else {
            holder.replace("<<", " ").replace('<', " ").trim().to_owned()
        };
        out.push(CardInfo {
            name,
            signing: fps.as_ref().and_then(|f| f.signature()).map(|f| f.to_hex().to_ascii_uppercase()),
            decryption: fps.as_ref().and_then(|f| f.decryption()).map(|f| f.to_hex().to_ascii_uppercase()),
            url: tx.url().unwrap_or_default(),
            tries: tx.pw_status_bytes().ok().map(|s| s.err_count_pw1()),
            ident,
        });
    }
    Ok(out)
}

/// The fingerprints of a certificate's primary key and subkeys, hex.
fn key_fingerprints(key: &SignedPublicKey) -> Vec<String> {
    std::iter::once(hex(&key.primary_key.fingerprint()))
        .chain(key.public_subkeys.iter().map(|s| hex(&s.key.fingerprint())))
        .collect()
}
fn hex(fp: &Fingerprint) -> String {
    fp.as_bytes().iter().map(|b| format!("{b:02X}")).collect()
}

/// Of `candidates` (armored or binary certificates), the one whose keys
/// are the card's: its signing or decryption key is one of them.
pub fn certificate_for(card: &CardInfo, candidates: &[Vec<u8>]) -> Option<SignedPublicKey> {
    let wanted: Vec<&String> = card.signing.iter().chain(card.decryption.iter()).collect();
    ensure_some(!wanted.is_empty())?;
    candidates
        .iter()
        .flat_map(|data| {
            let parsed: Vec<SignedPublicKey> = match SignedPublicKey::from_reader_many(&data[..]) {
                Ok((keys, _)) => keys.filter_map(|k| k.ok()).collect(),
                Err(_) => SignedPublicKey::from_armor_many(&data[..])
                    .map(|(keys, _)| keys.filter_map(|k| k.ok()).collect())
                    .unwrap_or_default(),
            };
            parsed
        })
        .find(|k| k.verify_bindings().is_ok() && key_fingerprints(k).iter().any(|f| wanted.contains(&f)))
}
fn ensure_some(ok: bool) -> Option<()> {
    ok.then_some(())
}

/// The card as a key of yours, with its certificate.
pub fn card_key(card: &CardInfo, certificate: &SignedPublicKey) -> Result<CardKey> {
    let fps = key_fingerprints(certificate);
    let signing = card.signing.clone().filter(|f| fps.contains(f));
    let decryption = card.decryption.clone().filter(|f| fps.contains(f));
    ensure!(signing.is_some() || decryption.is_some(), t!("security-keys-the-certificate-isnt-the-cards"));
    Ok(CardKey {
        ident: card.ident.clone(),
        public: certificate.to_armored_string(pgp::composed::ArmorOptions::default())?,
        signing,
        decryption,
        name: card.name.clone(),
    })
}

/// Opens the card `ident`, among those connected.
fn open(ident: &str) -> Result<Card<Open>> {
    let backends = crate::sync::pcsc::cards().map_err(|e| anyhow::anyhow!("{e}"))?;
    Card::open_by_ident(backends.into_iter().map(Ok), ident)
        .map_err(|_| anyhow::anyhow!(t!("security-keys-insert-the-card", ident = ident.to_owned())))
}

/// Checks a card's PIN (PW1, as for decrypting) and keeps it. A wrong PIN
/// costs one of the card's tries, which the error says.
pub fn unlock(ident: &str, pin: &str) -> Result<()> {
    ensure!(!pin.trim().is_empty(), t!("security-keys-enter-the-pin"));
    let mut card = open(ident)?;
    let mut tx = card.transaction().map_err(|e| anyhow::anyhow!("{e}"))?;
    if tx.verify_user_pin(pin.to_owned().into()).is_err() {
        crate::manage::hardware::forget_pin(&pin_id(ident));
        let tries = tx.pw_status_bytes().map(|s| s.err_count_pw1()).unwrap_or(0);
        bail!(t!("security-keys-wrong-pin", tries = tries));
    }
    crate::manage::hardware::remember_pin(&pin_id(ident), pin);
    Ok(())
}

/// The PIN kept for a card, or the error the reader turns into Unlock.
fn kept_pin(key: &CardKey) -> Result<zeroize::Zeroizing<String>> {
    crate::manage::hardware::pin(&pin_id(&key.ident)).ok_or_else(|| {
        crate::manage::hardware::PinNeeded { key: pin_id(&key.ident), name: key.name.clone() }.into()
    })
}

/// The card's slot's public key, as rPGP models it (a primary key packet).
fn slot_key(card: &mut Card<Open>, kind: KeyType, expected: &str) -> Result<PublicKey> {
    let mut tx = card.transaction().map_err(|e| anyhow::anyhow!("{e}"))?;
    let slot = CardSlot::init_from_card(&mut tx, kind, &|| {}).map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = slot.public_key().clone();
    ensure!(hex(&key.fingerprint()) == expected, t!("security-keys-the-card-holds-another-key"));
    Ok(key)
}

/// A card's signing key as rPGP signs with it: each signature verifies
/// the PIN again first, as cards may allow one signature per PIN.
pub struct CardSigner {
    card: std::sync::Mutex<Card<Open>>,
    public: PublicKey,
    pin: zeroize::Zeroizing<String>,
}
impl std::fmt::Debug for CardSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CardSigner({:?})", self.public.fingerprint())
    }
}
impl CardSigner {
    /// The signer of `key`'s card, with its kept PIN.
    pub fn open(key: &CardKey) -> Result<Self> {
        let expected = key.signing.as_deref().with_context(|| t!("security-keys-the-card-doesnt-sign"))?;
        let pin = kept_pin(key)?;
        let mut card = open(&key.ident)?;
        let public = slot_key(&mut card, KeyType::Signing, expected)?;
        Ok(Self { card: std::sync::Mutex::new(card), public, pin })
    }
}
impl KeyDetails for CardSigner {
    fn version(&self) -> KeyVersion {
        self.public.version()
    }
    fn fingerprint(&self) -> Fingerprint {
        self.public.fingerprint()
    }
    fn legacy_key_id(&self) -> KeyId {
        self.public.legacy_key_id()
    }
    fn algorithm(&self) -> pgp::crypto::public_key::PublicKeyAlgorithm {
        self.public.algorithm()
    }
    fn created_at(&self) -> Timestamp {
        self.public.created_at()
    }
    fn legacy_v3_expiration_days(&self) -> Option<u16> {
        None
    }
    fn public_params(&self) -> &PublicParams {
        self.public.public_params()
    }
}
impl SigningKey for CardSigner {
    fn sign(&self, key_pw: &Password, hash: HashAlgorithm, data: &[u8]) -> pgp::errors::Result<SignatureBytes> {
        let message = |e: String| pgp::errors::Error::Message { message: e, backtrace: None };
        let mut card = self.card.lock().unwrap_or_else(|e| e.into_inner());
        let mut tx = card.transaction().map_err(|e| message(e.to_string()))?;
        tx.verify_user_signing_pin(self.pin.to_string().into())
            .map_err(|_| message(t!("security-keys-the-card-refused-the-pin").into()))?;
        let slot = CardSlot::with_public_key(&mut tx, KeyType::Signing, self.public.clone(), &|| {})
            .map_err(|e| message(e.to_string()))?;
        slot.sign(key_pw, hash, data)
    }
    fn hash_alg(&self) -> HashAlgorithm {
        self.public.public_params().hash_alg()
    }
}

/// The session keys the cards among `keys` can decrypt from `message`'s
/// PKESKs addressed to them (an encrypted layer, as `nested` meets it).
/// A card that isn't connected or unlocked gives none: the reader offers
/// to unlock it (see `manage/hardware.rs`).
pub(super) fn session_keys(message: &Message<'_>, keys: &[CardKey]) -> Vec<PlainSessionKey> {
    let esk = match message {
        Message::Encrypted { esk, .. } => esk,
        Message::Signed { reader, .. } => return session_keys(reader.get_ref(), keys),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for key in keys {
        let Some(id) = key.decryption_id() else { continue };
        let addressed: Vec<_> = esk
            .iter()
            .filter_map(|e| match e {
                pgp::composed::Esk::PublicKeyEncryptedSessionKey(p) => Some(p),
                _ => None,
            })
            .filter(|p| {
                p.id().is_ok_and(|k| {
                    let hex: String = k.as_ref().iter().map(|b| format!("{b:02X}")).collect();
                    hex.eq_ignore_ascii_case(&id) || k.is_wildcard()
                })
            })
            .collect();
        if addressed.is_empty() {
            continue;
        }
        let Ok(pin) = kept_pin(key) else { continue };
        let Ok(found) = decrypt_with_card(key, &pin, &addressed) else { continue };
        out.extend(found);
    }
    out
}

fn decrypt_with_card(
    key: &CardKey,
    pin: &str,
    addressed: &[&pgp::packet::PublicKeyEncryptedSessionKey],
) -> Result<Option<PlainSessionKey>> {
    let expected = key.decryption.as_deref().context("no decryption key")?;
    let mut card = open(&key.ident)?;
    let public = slot_key(&mut card, KeyType::Decryption, expected)?;
    let mut tx = card.transaction().map_err(|e| anyhow::anyhow!("{e}"))?;
    if tx.verify_user_pin(pin.to_owned().into()).is_err() {
        // Changed since: asked for again.
        crate::manage::hardware::forget_pin(&pin_id(&key.ident));
        bail!(t!("security-keys-the-card-refused-the-pin"));
    }
    let slot = CardSlot::with_public_key(&mut tx, KeyType::Decryption, public, &|| {}).map_err(|e| anyhow::anyhow!("{e}"))?;
    for pkesk in addressed {
        let Ok(values) = pkesk.values() else { continue };
        if let Ok(Ok(session)) = slot.decrypt(&Password::empty(), values, EskType::V3_4) {
            return Ok(Some(session));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cards_certificate_is_the_one_holding_its_keys() {
        let secret = super::super::test_key("Ana <ana@example.org>", "");
        let public = SignedPublicKey::from(secret.clone());
        let fps = key_fingerprints(&public);
        let card = CardInfo {
            ident: "0006:12345678".into(),
            name: "Ana".into(),
            signing: Some(fps[0].clone()),
            decryption: fps.get(1).cloned(),
            ..CardInfo::default()
        };
        let other = SignedPublicKey::from(super::super::test_key("Bo <bo@example.org>", ""));
        let candidates = vec![
            other.to_armored_string(Default::default()).unwrap().into_bytes(),
            public.to_armored_string(Default::default()).unwrap().into_bytes(),
        ];
        let found = certificate_for(&card, &candidates).unwrap();
        assert_eq!(fingerprint(&found), fingerprint(&public));
        let key = card_key(&card, &found).unwrap();
        assert!(key.is_for("ana@example.org"));
        assert_eq!(key.addresses(), ["ana@example.org"]);
        assert_eq!(key.decryption_id().map(|id| id.len()), key.decryption.as_ref().map(|_| 16));
        // Another person's certificate isn't the card's.
        assert!(card_key(&card, &other).is_err());
        assert_eq!(pin_id("0006:1234abcd"), "openpgp-card:0006:1234ABCD");
    }

    #[test]
    fn a_locked_card_is_passed_over_rather_than_asked() {
        use pgp::composed::MessageBuilder;
        let secret = super::super::test_key("Ana <ana@example.org>", "");
        let public = SignedPublicKey::from(secret.clone());
        let fps = key_fingerprints(&public);
        let key = CardKey {
            ident: format!("FFFE:{:08X}", rand::random::<u32>()),
            public: public.to_armored_string(Default::default()).unwrap(),
            decryption: fps.get(1).cloned(),
            name: "Test".into(),
            ..CardKey::default()
        };
        let sub = super::super::encryption_subkey(&public).unwrap();
        let mut builder = MessageBuilder::from_bytes("", b"hello".to_vec()).seipd_v1(rand08::thread_rng(), pgp::crypto::sym::SymmetricKeyAlgorithm::AES128);
        builder.encrypt_to_key(rand08::thread_rng(), &sub.key).unwrap();
        let armored = builder.to_armored_string(rand08::thread_rng(), Default::default()).unwrap();
        let (message, _) = Message::from_string(&armored).unwrap();
        // Locked: nothing, and no card is looked for.
        assert!(session_keys(&message, std::slice::from_ref(&key)).is_empty());
    }
}
