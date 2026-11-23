//! Your own keys after they are made: revoking them.
use super::{Keyring, fingerprint};
use anyhow::{Context, Result};
use pgp::composed::{ArmorOptions, SignedPublicKey, SignedSecretKey};
use pgp::packet::{RevocationCode, SignatureConfig, SignatureType, Subpacket, SubpacketData};
use pgp::types::{KeyDetails, KeyVersion, Password, Timestamp};

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

impl Keyring {
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
}
