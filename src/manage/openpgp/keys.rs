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
        .context("The revocation couldn't be made.")
}

/// A revocation certificate for `key`: its public key with a Key
/// Revocation signature added (RFC 9580 §10.1, "Primary Key [Revocation
/// Signature] ..."), armored. A whole key rather than the bare signature
/// GnuPG files away: RFC 9580 §10.1.2 requires the primary key packet for
/// v6 keys, and every OpenPGP app (Picacho's `import` too) imports a key
/// and merges the revocation into the copy it has. Publishing it, or
/// sending it to the people who have the key, marks the key revoked for
/// them; made ahead of time, it can be kept to use if the key is lost.
#[cfg_attr(not(test), allow(dead_code))]
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
    let mut config = old.config().context("A self-signature on the key can't be read.")?.clone();
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

/// `key` re-signed to expire `lasts` after it was made (RFC 9580
/// §5.2.3.13), or never with `None`. The newest self-signature of each
/// user id, and the newest direct-key signature where there is one (v6
/// keys keep their preferences there, §5.2.3.10), gets a newer copy saying
/// so (see `resigned_subpackets`); the old ones stay, as they are
/// superseded by date. A user id whose newest self-signature revokes it
/// stays revoked.
///
/// Subkey binding signatures are left as they are: §5.2.3.13 puts the
/// key's expiry on the primary key's self-signatures, and a subkey stops
/// being usable when the primary key does (Picacho's `key_for` and
/// `unrevoked` check the primary first, as other apps do). A subkey's own
/// expiry, in its binding, is a separate, shorter limit, which this doesn't
/// touch. The test checks that a peer encrypting to the key sees it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_expiry(key: &SignedSecretKey, lasts: Option<std::time::Duration>) -> Result<SignedSecretKey> {
    let lasts = match lasts {
        None => None,
        Some(d) => {
            let secs = u32::try_from(d.as_secs()).ok().filter(|s| *s > 0);
            ensure!(secs.is_some(), "Choose a time between a second and 136 years.");
            secs.map(Duration::from_secs)
        }
    };
    let primary = &key.primary_key;
    let public = primary.public_key();
    let mut out = key.clone();
    let mut old: Vec<&pgp::packet::Signature> = key.details.direct_signatures.iter().collect();
    old.extend(key.details.users.iter().flat_map(|u| u.signatures.iter()));
    let created = after(&old);
    let mut resigned = 0;
    if let Some(direct) = newest(&key.details.direct_signatures) {
        let config = resigned_subpackets(direct, created, lasts)?;
        let signature = config
            .sign_key(primary, &Password::empty(), public)
            .context("The key couldn't be re-signed.")?;
        out.details.direct_signatures.push(signature);
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
            .context("The key couldn't be re-signed.")?;
        user.signatures.push(signature);
        resigned += 1;
    }
    ensure!(resigned > 0, "This key has no self-signature to set its expiry on.");
    Ok(out)
}

impl Keyring {
    // Backend-only until the UI calls it:
    /// Sets when your key with fingerprint `fp` expires (see `set_expiry`),
    /// keeping it re-signed in place of the old copy. Others see the new
    /// date once they have the key again (Autocrypt, WKD, or sent by hand).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_expiry(&mut self, fp: &str, lasts: Option<std::time::Duration>) -> Result<()> {
        let at = self
            .secret
            .iter()
            .position(|k| {
                use pgp::composed::Deserializable as _;
                SignedSecretKey::from_string(k)
                    .is_ok_and(|(k, _)| fingerprint(&SignedPublicKey::from(k)) == fp)
            })
            .context("That isn't one of your keys.")?;
        let key = self.secret_keys().into_iter().find(|k| fingerprint(&SignedPublicKey::from(k.clone())) == fp);
        let key = key.context("That isn't one of your keys.")?;
        self.secret[at] = set_expiry(&key, lasts)?.to_armored_string(ArmorOptions::default())?;
        Ok(())
    }
    // Backend-only until the UI calls it:
    /// A revocation certificate (see `revocation_certificate`) for your key
    /// with fingerprint `fp`. The keyring itself is unchanged: the key is
    /// revoked once the certificate is imported.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn revocation_certificate(&self, fp: &str, reason: RevocationReason, text: &str) -> Result<String> {
        let key = self
            .secret_keys()
            .into_iter()
            .find(|k| fingerprint(&SignedPublicKey::from(k.clone())) == fp)
            .context("That isn't one of your keys.")?;
        revocation_certificate(&key, reason, text)
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
        // Subkey bindings are left as they were (see `set_expiry`).
        for (new, old) in twice.secret_subkeys.iter().zip(&ana.secret_subkeys) {
            assert_eq!(new.signatures, old.signatures);
        }
        // None: never expires again.
        let cleared = set_expiry(&twice, None).unwrap();
        for user in &cleared.details.users {
            assert_eq!(newest_of(user).key_expiration_time(), None);
        }
        assert!(!super::super::primary_expired(&SignedPublicKey::from(cleared)));
        assert!(set_expiry(&ana, Some(std::time::Duration::ZERO)).is_err());
    }

    /// An expiry in the past: once others have the re-signed key (exported
    /// and imported over the copy they had), they no longer encrypt to it
    /// nor trust new signatures by it; cleared, they do again.
    #[test]
    fn expired_keys_are_neither_encrypted_to_nor_trusted() {
        let ana = key_made_days_ago("Ana <ana@example.org>", 10);
        let bo = super::super::test_key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
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

        // Ten days old, set to last a day: expired.
        anas.set_expiry(&fp, Some(std::time::Duration::from_secs(86_400))).unwrap();
        assert!(anas.set_expiry("00", None).is_err());
        assert_eq!(anas.list().len(), 1, "re-signed in place");
        let resigned = anas.own_key("ana@example.org").unwrap();
        bos.import(&SignedPublicKey::from(resigned.clone()).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        assert_eq!(bos.list().len(), 2, "merged into the copy Bo had");
        assert!(bos.key_for("ana@example.org").is_none());
        let error = protect(message, "bo@example.se", &["ana@example.org".into()], encrypt, &bos).unwrap_err();
        assert!(error.to_string().contains("ana@example.org"), "{error}");
        let signed = protect(from_ana, "ana@example.org", &[], sign, &anas).unwrap();
        assert_eq!(open(&signed, &bos).unwrap().1.signature, Signature::UnknownKey);

        // Cleared, and the newer copy imported again: the newest wins.
        anas.set_expiry(&fp, None).unwrap();
        let renewed = anas.own_key("ana@example.org").unwrap();
        bos.import(&SignedPublicKey::from(renewed).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        assert!(bos.key_for("ana@example.org").is_some());
        assert!(protect(message, "bo@example.se", &["ana@example.org".into()], encrypt, &bos).is_ok());
        assert!(matches!(open(&signed, &bos).unwrap().1.signature, Signature::Good(_)));
        // An older copy imported after doesn't undo it.
        bos.import(&SignedPublicKey::from(resigned).to_armored_string(ArmorOptions::default()).unwrap(), "")
            .unwrap();
        assert!(bos.key_for("ana@example.org").is_some());
    }
}
