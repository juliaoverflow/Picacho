//! Your own keys after they are made: revoking them, and setting when
//! they expire.
use super::{Keyring, fingerprint};
use anyhow::{Context, Result, ensure};
use pgp::composed::{ArmorOptions, SignedPublicKey, SignedSecretKey};
use pgp::packet::{PacketTrait as _, RevocationCode, SignatureConfig, SignatureType, Subpacket, SubpacketData};
use pgp::types::{Duration, KeyDetails, KeyVersion, Password, Timestamp};

/// Why a key is revoked (RFC 9580 §5.2.3.31, codes for Key Revocation
/// signatures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum RevocationReason {
    /// No reason given: what a certificate made ahead of time, kept in
    /// case the key is lost, says.
    Unspecified,
    /// A new key replaces this one.
    Superseded,
    /// Someone else may have the secret key.
    Compromised,
    /// The key is no longer used.
    Retired,
}

impl From<RevocationReason> for RevocationCode {
    fn from(reason: RevocationReason) -> Self {
        match reason {
            RevocationReason::Unspecified => RevocationCode::NoReason,
            RevocationReason::Superseded => RevocationCode::KeySuperseded,
            RevocationReason::Compromised => RevocationCode::KeyCompromised,
            RevocationReason::Retired => RevocationCode::KeyRetired,
        }
    }
}

/// A Key Revocation signature (RFC 9580 §5.2.1.11, type 0x20) by `key`'s
/// primary key over itself, saying `reason` and, for people, `text`.
fn revocation_signature(key: &SignedSecretKey, reason: RevocationReason, text: &str) -> Result<pgp::packet::Signature> {
    let primary = &key.primary_key;
    let mut config = SignatureConfig::from_key(rand08::thread_rng(), primary, SignatureType::KeyRevocation)?;
    config.hashed_subpackets = vec![
        Subpacket::regular(SubpacketData::SignatureCreationTime(Timestamp::now()))?,
        Subpacket::regular(SubpacketData::IssuerFingerprint(primary.fingerprint()))?,
        Subpacket::regular(SubpacketData::RevocationReason(
            reason.into(),
            text.as_bytes().to_vec().into(),
        ))?,
    ];
    // RFC 9580 §5.2.3.12: v4 signatures also name the issuer by key ID,
    // for older readers; v6 ones don't.
    if primary.version() == KeyVersion::V4 {
        config.unhashed_subpackets = vec![Subpacket::regular(SubpacketData::IssuerKeyId(primary.legacy_key_id()))?];
    }
    config
        .sign_key(primary, &Password::empty(), primary.public_key())
        .with_context(|| t!("manage-openpgp-keys-the-revocation-couldnt-be-made"))
}

/// A revocation certificate for `key`: its public key with a Key
/// Revocation signature added (RFC 9580 §10.1, "Primary Key [Revocation
/// Signature] ..."), armored. A whole key rather than the bare signature
/// GnuPG files away: RFC 9580 §10.1.2 requires the primary key packet for
/// v6 keys, and every OpenPGP app (Picacho's `import` too) imports a key
/// and merges the revocation into the copy it has. Publishing it, or
/// sending it to the people who have the key, marks the key revoked for
/// them; made ahead of time, it can be kept to use if the key is lost.
pub fn revocation_certificate(key: &SignedSecretKey, reason: RevocationReason, text: &str) -> Result<String> {
    let mut public = SignedPublicKey::from(key.clone());
    public.details.revocation_signatures.push(revocation_signature(key, reason, text)?);
    Ok(public.to_armored_string(ArmorOptions::default())?)
}

/// The newest of `signatures` (by their creation time).
fn newest(signatures: &[pgp::packet::Signature]) -> Option<&pgp::packet::Signature> {
    signatures.iter().max_by_key(|s| s.created().map(|t| t.as_secs()))
}

/// A creation time for a signature that replaces `old`: now, or a second
/// after the newest of them where that isn't earlier (made within the same
/// second, or with a clock behind). RFC 9580 §5.2.3.10: the newest
/// self-signature is the one that counts, and two from the same second
/// can't be told apart.
fn after(old: &[&pgp::packet::Signature]) -> Timestamp {
    let now = Timestamp::now().as_secs();
    let newest = old.iter().filter_map(|s| s.created()).map(|t| t.as_secs()).max();
    match newest {
        Some(t) if t >= now => Timestamp::from_secs(t.saturating_add(1)),
        _ => Timestamp::from_secs(now),
    }
}

/// The hashed subpackets of `old` for its replacement made at `created`:
/// every one kept as it was (key flags, preferences, features, primary
/// user id...), but the creation time and the key's expiry, which are
/// `created` and `lasts`.
fn resigned_subpackets(
    old: &pgp::packet::Signature,
    created: Timestamp,
    lasts: Option<Duration>,
) -> Result<SignatureConfig> {
    let mut config = old.config().with_context(|| t!("manage-openpgp-keys-a-self-signature-on-the-key"))?.clone();
    config.hashed_subpackets.retain(|p| {
        !matches!(p.data, SubpacketData::SignatureCreationTime(_) | SubpacketData::KeyExpirationTime(_))
    });
    config
        .hashed_subpackets
        .insert(0, Subpacket::regular(SubpacketData::SignatureCreationTime(created))?);
    if let Some(lasts) = lasts {
        // RFC 9580 §5.2.3.13: generated, it SHOULD be marked critical.
        config.hashed_subpackets.push(Subpacket::critical(SubpacketData::KeyExpirationTime(lasts))?);
    }
    // RFC 9580 §5.2.3.10: a v6 signature's salt is fresh each time.
    if let pgp::packet::SignatureVersionSpecific::V6 { .. } = config.version_specific {
        let fresh = SignatureConfig::v6(rand08::thread_rng(), config.typ, config.pub_alg, config.hash_alg)?;
        config.version_specific = fresh.version_specific;
    }
    Ok(config)
}

/// How long a key lasts, as RFC 9580 §5.2.3.13 writes it: seconds after
/// its creation, at least one, and no more than 32 bits hold.
fn validity(lasts: std::time::Duration) -> Result<Duration> {
    let secs = u32::try_from(lasts.as_secs()).ok().filter(|s| *s > 0);
    secs.map(Duration::from_secs)
        .with_context(|| t!("manage-openpgp-keys-choose-a-time-between-a-second"))
}

/// `key` re-signed to expire `lasts` after it was made (RFC 9580
/// §5.2.3.13), or never with `None`.
///
/// The newest self-signature of each user id, the newest direct-key
/// signature where there is one (v6 keys keep their preferences there,
/// §5.2.3.10), and the newest binding signature of each subkey get a newer
/// copy saying so (see `resigned_subpackets`); the old ones stay, as they
/// are superseded by date. A user id or subkey whose newest signature
/// revokes it stays revoked.
///
/// Subkeys are re-signed as well: §5.2.3.13 puts a subkey's expiry in its
/// binding signature, counted from the subkey's own creation, and apps
/// differ on whether a subkey outlives an expired primary key's renewal
/// or a binding's older, earlier end. Each subkey is set to end when the
/// primary key does (never, with `None`). A signing subkey's binding keeps
/// its embedded back-signature (§5.2.1.9), which covers the two keys, not
/// the binding, and so stays valid.
pub fn set_expiry(key: &SignedSecretKey, lasts: Option<std::time::Duration>) -> Result<SignedSecretKey> {
    resign(key, lasts.map(validity).transpose()?, Supersede::Newer)
}

/// Whether `resign` adds newer signatures after the old ones or replaces
/// them (for a key just made, which no one else has yet).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Supersede {
    Newer,
    Replace,
}

fn resign(key: &SignedSecretKey, lasts: Option<Duration>, how: Supersede) -> Result<SignedSecretKey> {
    let primary = &key.primary_key;
    let public = primary.public_key();
    let made = primary.created_at().as_secs();
    let mut out = key.clone();
    let mut old: Vec<&pgp::packet::Signature> = key.details.direct_signatures.iter().collect();
    old.extend(key.details.users.iter().flat_map(|u| u.signatures.iter()));
    old.extend(key.secret_subkeys.iter().flat_map(|s| s.signatures.iter()));
    let created = match how {
        Supersede::Newer => after(&old),
        Supersede::Replace => Timestamp::now(),
    };
    let add = |list: &mut Vec<pgp::packet::Signature>, signature| match how {
        Supersede::Newer => list.push(signature),
        Supersede::Replace => *list = vec![signature],
    };
    let mut resigned = 0;
    if let Some(direct) = newest(&key.details.direct_signatures) {
        let config = resigned_subpackets(direct, created, lasts)?;
        let signature = config
            .sign_key(primary, &Password::empty(), public)
            .with_context(|| t!("manage-openpgp-keys-the-key-couldnt-be-re-signed"))?;
        add(&mut out.details.direct_signatures, signature);
        resigned += 1;
    }
    for user in &mut out.details.users {
        let Some(newest) = newest(&user.signatures) else { continue };
        if newest.typ() == Some(SignatureType::CertRevocation) || !newest.is_certification() {
            continue;
        }
        let config = resigned_subpackets(newest, created, lasts)?;
        let signature = config
            .sign_certification(primary, public, &Password::empty(), user.id.tag(), &user.id)
            .with_context(|| t!("manage-openpgp-keys-the-key-couldnt-be-re-signed"))?;
        add(&mut user.signatures, signature);
        resigned += 1;
    }
    ensure!(resigned > 0, t!("manage-openpgp-keys-this-key-has-no-self-signature"));
    for sub in &mut out.secret_subkeys {
        let Some(binding) = newest(&sub.signatures) else { continue };
        if binding.typ() != Some(SignatureType::SubkeyBinding) {
            continue;
        }
        // The primary key's end, counted from the subkey's creation; a
        // subkey made after it ends a second after it was made.
        let sub_lasts = lasts.map(|l| {
            let end = i64::from(made) + i64::from(l.as_secs());
            let from_sub = end - i64::from(sub.key.created_at().as_secs());
            Duration::from_secs(u32::try_from(from_sub.max(1)).unwrap_or(u32::MAX))
        });
        let config = resigned_subpackets(binding, created, sub_lasts)?;
        let signature = config
            .sign_subkey_binding(primary, public, &Password::empty(), sub.key.public_key())
            .with_context(|| t!("manage-openpgp-keys-the-key-couldnt-be-re-signed"))?;
        add(&mut sub.signatures, signature);
    }
    Ok(out)
}

/// When `key` expires (RFC 9580 §5.2.3.13), as seconds since 1970, by its
/// newest self-signature; `None` when it never does.
pub fn expires(key: &SignedPublicKey) -> Option<i64> {
    let lasts = super::newest_self_signature(key)?.key_expiration_time()?.as_secs();
    (lasts > 0).then(|| i64::from(key.primary_key.created_at().as_secs()) + i64::from(lasts))
}

/// `key`, just made, made to expire `lasts` after now: its self-signatures
/// and bindings replaced by ones saying so (see `set_expiry`).
pub(super) fn expiring(key: SignedSecretKey, lasts: Option<std::time::Duration>) -> Result<SignedSecretKey> {
    let Some(lasts) = lasts else { return Ok(key) };
    let age = Timestamp::now().as_secs().saturating_sub(key.primary_key.created_at().as_secs());
    let lasts = validity(lasts + std::time::Duration::from_secs(age.into()))?;
    resign(&key, Some(lasts), Supersede::Replace)
}

impl Keyring {
    /// Renews your key with fingerprint `fp`: it expires `from_now` after
    /// now, or never with `None` (see `set_expiry`). An expired key is
    /// renewed too, the main reason to (RFC 9580 §5.2.3.13: its expiry is
    /// only what its newest self-signature says). The re-signed key takes
    /// the old one's place; others see the new date once they have the
    /// key again (sent by hand, or by Autocrypt or WKD).
    pub fn renew(&mut self, fp: &str, from_now: Option<std::time::Duration>) -> Result<()> {
        use pgp::composed::Deserializable as _;
        let (at, key) = self
            .secret
            .iter()
            .enumerate()
            .find_map(|(at, k)| {
                let (k, _) = SignedSecretKey::from_string(k).ok()?;
                (fingerprint(&SignedPublicKey::from(k.clone())) == fp).then_some((at, k))
            })
            .with_context(|| t!("manage-openpgp-keys-that-isnt-one-of-your-keys"))?;
        let lasts = from_now.map(|d| {
            let age = Timestamp::now().as_secs().saturating_sub(key.primary_key.created_at().as_secs());
            d + std::time::Duration::from_secs(age.into())
        });
        self.secret[at] = set_expiry(&key, lasts)?.to_armored_string(ArmorOptions::default())?;
        Ok(())
    }
    /// A revocation certificate (see `revocation_certificate`) for your key
    /// with fingerprint `fp`. The keyring itself is unchanged: the key is
    /// revoked once the certificate is imported.
    pub fn revocation_certificate(&self, fp: &str, reason: RevocationReason, text: &str) -> Result<String> {
        let key = self
            .secret_keys()
            .into_iter()
            .find(|k| fingerprint(&SignedPublicKey::from(k.clone())) == fp)
            .with_context(|| t!("manage-openpgp-keys-that-isnt-one-of-your-keys"))?;
        revocation_certificate(&key, reason, text)
    }
    /// Where `public` (its signatures already verified) is one of your own
    /// keys carrying Key Revocation signatures (RFC 9580 §5.2.1.11), as a
    /// revocation certificate does: adds them to your secret key, which
    /// then lists as revoked and is no longer encrypted to. Returns whether
    /// it was one of yours; a copy of your key without revocations isn't
    /// taken here.
    pub(super) fn revoke_own(&mut self, public: &SignedPublicKey) -> Result<bool> {
        use pgp::composed::Deserializable as _;
        if public.details.revocation_signatures.is_empty() {
            return Ok(false);
        }
        let fp = fingerprint(public);
        let Some((at, mut key)) = self.secret.iter().enumerate().find_map(|(at, k)| {
            let (k, _) = SignedSecretKey::from_string(k).ok()?;
            (fingerprint(&SignedPublicKey::from(k.clone())) == fp).then_some((at, k))
        }) else {
            return Ok(false);
        };
        for signature in &public.details.revocation_signatures {
            if !key.details.revocation_signatures.contains(signature) {
                key.details.revocation_signatures.push(signature.clone());
            }
        }
        self.secret[at] = key.to_armored_string(ArmorOptions::default())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Protect, Signature, open, protect, test_key};
    use super::*;
    use pgp::composed::Deserializable as _;

    /// RFC 9580 §5.2.1.11: the revocation is a Key Revocation signature by
    /// the key over itself, saying why; it verifies, and once imported the
    /// key is no longer encrypted to nor trusted for signatures.
    #[test]
    fn revocation_certificates_verify_and_revoke() {
        let ana = test_key("Ana <ana@example.org>", "");
        let bo = test_key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        anas.import(&SignedPublicKey::from(bo.clone()).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let fp = fingerprint(&SignedPublicKey::from(ana.clone()));
        let certificate = anas
            .revocation_certificate(&fp, RevocationReason::Compromised, "Laptop stolen")
            .unwrap();
        assert!(anas.revocation_certificate("00", RevocationReason::Retired, "").is_err());

        let (revoked, _) = SignedPublicKey::from_string(&certificate).unwrap();
        assert_eq!(fingerprint(&revoked), fp);
        revoked.verify_bindings().unwrap();
        let [signature] = revoked.details.revocation_signatures.as_slice() else {
            panic!("one revocation: {:?}", revoked.details.revocation_signatures);
        };
        assert_eq!(signature.typ(), Some(SignatureType::KeyRevocation));
        signature.verify_key(&revoked.primary_key).unwrap();
        assert_eq!(signature.revocation_reason_code(), Some(&RevocationCode::KeyCompromised));
        assert_eq!(signature.revocation_reason_string().map(|b| b.as_ref()), Some(&b"Laptop stolen"[..]));
        assert_eq!(signature.issuer_fingerprint(), vec![&revoked.primary_key.fingerprint()]);
        // Bo's own key doesn't make Ana's revocation verify.
        assert!(signature.verify_key(&SignedPublicKey::from(bo.clone()).primary_key).is_err());

        // Bo has Ana's key, and a message she signed.
        let mut bos = Keyring::default();
        bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        bos.import(&SignedPublicKey::from(ana.clone()).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let message = b"From: Ana <ana@example.org>\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHi\r\n";
        let sign = Protect { sign: true, ..Default::default() };
        let signed = protect(message, "ana@example.org", &[], sign, &anas).unwrap();
        assert!(matches!(open(&signed, &bos).unwrap().1.signature, Signature::Good(_)));
        assert!(bos.key_for("ana@example.org").is_some());
        // The certificate imported: merged into the key Bo had, which he no
        // longer encrypts to nor trusts.
        bos.import(&certificate, "").unwrap();
        assert_eq!(bos.list().len(), 2);
        assert!(bos.key_for("ana@example.org").is_none());
        assert_eq!(open(&signed, &bos).unwrap().1.signature, Signature::UnknownKey);
    }

    /// RFC 9580 §5.2.1.11: a revocation certificate imported where the
    /// secret key is goes onto it, and the key lists as revoked, in the
    /// keyring that made it as in one that has the public key alone.
    #[test]
    fn imported_revocation_certificates_revoke_your_own_key_and_list_it() {
        let mut ring = Keyring::default();
        let fp = ring.create("Ana", "ana@example.org", None, Default::default()).unwrap();
        assert!(!ring.list()[0].revoked);
        let certificate = ring.revocation_certificate(&fp, RevocationReason::Unspecified, "").unwrap();
        let mut bos = Keyring::default();
        let public = SignedPublicKey::from(ring.own_key("ana@example.org").unwrap());
        bos.import(&public.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        assert!(!bos.list()[0].revoked);

        assert_eq!(ring.import(&certificate, "").unwrap(), 1);
        let [listed]: [_; 1] = ring.list().try_into().unwrap_or_else(|l| panic!("one key: {l:?}"));
        assert!(listed.secret && listed.revoked, "{listed:?}");
        assert!(ring.public.is_empty(), "not kept again as someone else's");
        assert!(ring.key_for("ana@example.org").is_none());
        // Exported, your public key now carries the revocation.
        let exported = SignedPublicKey::from(ring.secret_keys().remove(0));
        assert_eq!(exported.details.revocation_signatures.len(), 1);
        exported.verify_bindings().unwrap();
        // Imported twice, it is there once.
        ring.import(&certificate, "").unwrap();
        assert_eq!(SignedPublicKey::from(ring.secret_keys().remove(0)).details.revocation_signatures.len(), 1);

        bos.import(&certificate, "").unwrap();
        assert!(bos.list()[0].revoked && !bos.list()[0].secret);
    }

    /// A key as `test_key` makes them, but made `days` ago.
    fn key_made_days_ago(uid: &str, days: u32) -> SignedSecretKey {
        use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
        use pgp::crypto::{ecc_curve::ECCCurve, sym::SymmetricKeyAlgorithm};
        let made = Timestamp::from_secs(Timestamp::now().as_secs() - days * 86_400);
        let mut sign = SubkeyParamsBuilder::default();
        sign.key_type(KeyType::Ed25519Legacy).can_sign(true).created_at(made);
        let mut encrypt = SubkeyParamsBuilder::default();
        encrypt
            .key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
            .can_encrypt(EncryptionCaps::All)
            .created_at(made);
        let mut params = SecretKeyParamsBuilder::default();
        params
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .created_at(made)
            .primary_user_id(uid.into())
            .user_id("Ana (work) <ana@work.example>")
            .preferred_symmetric_algorithms(vec![SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128].into())
            .subkeys(vec![sign.build().unwrap(), encrypt.build().unwrap()]);
        params.build().unwrap().generate(rand08::thread_rng()).unwrap()
    }

    fn newest_of(user: &pgp::types::SignedUser) -> &pgp::packet::Signature {
        newest(&user.signatures).unwrap()
    }

    /// RFC 9580 §5.2.3.13: the expiry goes on a new self-signature of
    /// every user id, newer than the old one even within the same second,
    /// keeping every other subpacket; `None` clears it.
    #[test]
    fn expiry_is_set_on_every_user_id_keeping_preferences() {
        let ana = key_made_days_ago("Ana <ana@example.org>", 10);
        let before: Vec<_> = ana.details.users.iter().map(|u| newest_of(u).clone()).collect();
        assert_eq!(before.len(), 2);
        let year = std::time::Duration::from_secs(365 * 86_400);
        let once = set_expiry(&ana, Some(year)).unwrap();
        // Again at once: the same second, and still the newer.
        let twice = set_expiry(&once, Some(year * 2)).unwrap();
        for (k, lasts) in [(&once, year), (&twice, year * 2)] {
            SignedPublicKey::from(k.clone()).verify_bindings().unwrap();
            for (user, old) in k.details.users.iter().zip(&before) {
                let new = newest_of(user);
                assert_eq!(new.key_expiration_time(), Some(Duration::from_secs(lasts.as_secs() as u32)));
                assert!(new.created().unwrap().as_secs() > old.created().unwrap().as_secs());
                new.verify_certification(&k.primary_key.public_key(), user.id.tag(), &user.id).unwrap();
                // Everything else is as it was.
                assert_eq!(new.key_flags(), old.key_flags());
                assert_eq!(new.preferred_symmetric_algs(), old.preferred_symmetric_algs());
                assert_eq!(new.preferred_hash_algs(), old.preferred_hash_algs());
                assert_eq!(new.preferred_aead_algs(), old.preferred_aead_algs());
                assert_eq!(new.features(), old.features());
                assert_eq!(new.is_primary(), old.is_primary());
                assert_eq!(new.typ(), old.typ());
            }
        }
        let ids = |k: &SignedSecretKey| k.details.users.iter().map(|u| newest_of(u).created().unwrap().as_secs()).collect::<Vec<_>>();
        assert!(ids(&twice).iter().zip(ids(&once)).all(|(t, o)| *t > o));
        // Each subkey's binding is re-signed to end with the primary key,
        // counted from its own creation, keeping its flags and, for the
        // signing subkey, its back-signature.
        let made = twice.primary_key.created_at().as_secs();
        for (new, old) in twice.secret_subkeys.iter().zip(&ana.secret_subkeys) {
            let binding = newest(&new.signatures).unwrap();
            let before = newest(&old.signatures).unwrap();
            assert_eq!(binding.typ(), Some(SignatureType::SubkeyBinding));
            assert!(binding.created().unwrap().as_secs() > before.created().unwrap().as_secs());
            let end = made + 2 * year.as_secs() as u32;
            assert_eq!(binding.key_expiration_time(), Some(Duration::from_secs(end - new.key.created_at().as_secs())));
            assert_eq!(binding.key_flags(), before.key_flags());
            assert_eq!(binding.embedded_signature().is_some(), before.embedded_signature().is_some());
            binding.verify_subkey_binding(&twice.primary_key.public_key(), new.key.public_key()).unwrap();
        }
        assert_eq!(expires(&SignedPublicKey::from(twice.clone())), Some(i64::from(made) + 2 * year.as_secs() as i64));
        // None: never expires again, nor do the subkeys.
        let cleared = set_expiry(&twice, None).unwrap();
        for user in &cleared.details.users {
            assert_eq!(newest_of(user).key_expiration_time(), None);
        }
        for sub in &cleared.secret_subkeys {
            assert_eq!(newest(&sub.signatures).unwrap().key_expiration_time(), None);
        }
        assert_eq!(expires(&SignedPublicKey::from(cleared.clone())), None);
        assert!(!super::super::primary_expired(&SignedPublicKey::from(cleared)));
        assert!(set_expiry(&ana, Some(std::time::Duration::ZERO)).is_err());
    }

    /// The key Bo has for Ana: the one she has now, imported over his copy.
    fn send_to_bo(anas: &Keyring, bos: &mut Keyring) -> SignedPublicKey {
        let key = SignedPublicKey::from(anas.own_key("ana@example.org").unwrap());
        bos.import(&key.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        key
    }

    /// An expiry in the past: once others have the re-signed key (exported
    /// and imported over the copy they had, the newest self-signature
    /// winning), they no longer encrypt to it, its encryption subkey
    /// included, nor trust signatures by it. Renewed, even though expired,
    /// it encrypts and verifies again; renewing with `None` clears it.
    #[test]
    fn expired_keys_are_neither_encrypted_to_nor_trusted_until_renewed() {
        let ana = key_made_days_ago("Ana <ana@example.org>", 10);
        let bo = super::super::test_key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        // Ten days old, set to have lasted a day: expired.
        let lapsed = set_expiry(&ana, Some(std::time::Duration::from_secs(86_400))).unwrap();
        anas.import(&lapsed.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        let mut bos = Keyring::default();
        bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        bos.import(&SignedPublicKey::from(ana.clone()).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        let fp = fingerprint(&SignedPublicKey::from(ana.clone()));
        let encrypt = Protect { encrypt: true, ..Default::default() };
        let sign = Protect { sign: true, ..Default::default() };
        let message = b"From: Bo <bo@example.se>\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHi\r\n";
        let from_ana = b"From: Ana <ana@example.org>\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHi\r\n";
        assert!(protect(message, "bo@example.se", &["ana@example.org".into()], encrypt, &bos).is_ok());
        let signed = protect(from_ana, "ana@example.org", &[], sign, &anas).unwrap();
        assert!(matches!(open(&signed, &bos).unwrap().1.signature, Signature::Good(_)));

        let had = send_to_bo(&anas, &mut bos);
        assert_eq!(bos.list().len(), 2, "merged into the copy Bo had");
        let day_after = bos.list().iter().find(|k| k.fingerprint == fp).unwrap().expires;
        assert_eq!(day_after, Some(i64::from(ana.primary_key.created_at().as_secs()) + 86_400));
        assert!(bos.key_for("ana@example.org").is_none());
        assert!(super::super::encryption_subkey(&had).is_none(), "the subkey has expired too");
        let error = protect(message, "bo@example.se", &["ana@example.org".into()], encrypt, &bos).unwrap_err();
        assert!(error.to_string().contains("ana@example.org"), "{error}");
        assert_eq!(open(&signed, &bos).unwrap().1.signature, Signature::UnknownKey);

        // Renewed for a year from now, though expired.
        let year = std::time::Duration::from_secs(365 * 86_400);
        anas.renew(&fp, Some(year)).unwrap();
        assert!(anas.renew("00", None).is_err());
        assert_eq!(anas.list().len(), 1, "re-signed in place");
        let expires = anas.list()[0].expires.unwrap();
        let wanted = chrono::Utc::now().timestamp() + year.as_secs() as i64;
        assert!((expires - wanted).abs() <= 2, "{expires} vs {wanted}");
        let renewed = send_to_bo(&anas, &mut bos);
        let sub = super::super::encryption_subkey(&renewed).expect("the encryption subkey is usable again");
        let binding = newest(&sub.signatures).unwrap();
        assert_eq!(
            i64::from(sub.key.created_at().as_secs()) + i64::from(binding.key_expiration_time().unwrap().as_secs()),
            expires
        );
        assert!(bos.key_for("ana@example.org").is_some());
        let sent = protect(message, "bo@example.se", &["ana@example.org".into()], encrypt, &bos).unwrap();
        assert!(open(&sent, &anas).unwrap().1.decrypted);
        assert!(matches!(open(&signed, &bos).unwrap().1.signature, Signature::Good(_)));
        // The lapsed copy imported after doesn't undo it: the newest wins.
        bos.import(&had.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        assert!(bos.key_for("ana@example.org").is_some());

        // Renewed with None: never expires.
        anas.renew(&fp, None).unwrap();
        assert_eq!(anas.list()[0].expires, None);
        send_to_bo(&anas, &mut bos);
        assert_eq!(bos.list().iter().find(|k| k.fingerprint == fp).unwrap().expires, None);
        assert!(bos.key_for("ana@example.org").is_some());
    }

    /// New keys don't expire unless asked; asked, they and their subkeys
    /// end that long from now.
    #[test]
    fn new_keys_expire_only_when_asked() {
        let mut ring = Keyring::default();
        let fp = ring.create("Ana", "ana@example.org", None, super::super::KeyVersion::V4).unwrap();
        assert_eq!(ring.list()[0].expires, None);
        let key = ring.own_key("ana@example.org").unwrap();
        assert!(key.secret_subkeys.iter().all(|s| newest(&s.signatures).unwrap().key_expiration_time().is_none()));
        ring.remove(&fp);

        let two_years = std::time::Duration::from_secs(2 * 365 * 86_400);
        ring.create("Ana", "ana@example.org", Some(two_years), super::super::KeyVersion::V4).unwrap();
        let info = &ring.list()[0];
        let wanted = chrono::Utc::now().timestamp() + two_years.as_secs() as i64;
        assert!((info.expires.unwrap() - wanted).abs() <= 2);
        let key = ring.own_key("ana@example.org").unwrap();
        let public = SignedPublicKey::from(key.clone());
        public.verify_bindings().unwrap();
        // Signed once, not superseded: a new key has one of each.
        assert!(key.details.users.iter().all(|u| u.signatures.len() == 1));
        assert!(key.secret_subkeys.iter().all(|s| s.signatures.len() == 1));
        for sub in &key.secret_subkeys {
            let lasts = sub.signatures[0].key_expiration_time().unwrap().as_secs();
            assert!((i64::from(sub.key.created_at().as_secs()) + i64::from(lasts) - wanted).abs() <= 2);
        }
        assert!(newest(&key.details.users[0].signatures).unwrap().preferred_symmetric_algs().contains(
            &pgp::crypto::sym::SymmetricKeyAlgorithm::AES256
        ));
        assert!(ring.key_for("ana@example.org").is_some());
    }
}
