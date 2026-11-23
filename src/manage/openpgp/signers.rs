//! Finding the key that made a signature: by the Issuer Fingerprint
//! subpacket (RFC 9580 §5.2.3.35), else by the Issuer Key ID subpacket
//! (§5.2.3.12), else, for a signature that names no issuer, every key.
use super::{Signature, good_from, unrevoked, usable_subkeys};
use pgp::composed::SignedPublicKey;
use pgp::packet::{PublicKey, PublicSubkey};

/// A key that can check signatures: a primary key or one of its subkeys.
#[derive(Clone, Copy)]
pub(super) enum Verifier<'k> {
    Primary(&'k PublicKey),
    Subkey(&'k PublicSubkey),
}

impl<'k> Verifier<'k> {
    pub(super) fn as_dyn(self) -> &'k dyn pgp::types::VerifyingKey {
        match self {
            Self::Primary(k) => k,
            Self::Subkey(k) => k,
        }
    }
    /// Whether `signature` over `data` was made by this key.
    pub(super) fn verifies(self, signature: &pgp::packet::Signature, data: &[u8]) -> bool {
        match self {
            Self::Primary(k) => signature.verify(k, data).is_ok(),
            Self::Subkey(k) => signature.verify(k, data).is_ok(),
        }
    }
}

/// Who signed: the keys `signature` names as its issuer, else every key.
pub(super) struct Signers<'k> {
    /// Each candidate key, with the key it belongs to.
    pub(super) keys: Vec<(&'k SignedPublicKey, Verifier<'k>)>,
    /// Whether `keys` are the ones the signature names (by fingerprint or
    /// key ID), rather than every key, as none it names is here.
    pub(super) named: bool,
}

/// The keys `signature` may have been made by: those its Issuer
/// Fingerprint names; for a signature without one (as older apps make),
/// those its Issuer Key ID names; else, when none of them is here, all of
/// `public` (whichever made it, it is still theirs). Only keys still good count
/// (see `unrevoked` and `usable_subkeys`): one that was revoked or has
/// expired is unknown, not a failure, as what it signed may well predate
/// that.
pub(super) fn signers<'k>(signature: &pgp::packet::Signature, public: &'k [SignedPublicKey]) -> Signers<'k> {
    let mut all = Vec::new();
    for key in unrevoked(public) {
        all.push((key, Verifier::Primary(&key.primary_key)));
        for sub in usable_subkeys(key) {
            all.push((key, Verifier::Subkey(&sub.key)));
        }
    }
    // RFC 9580 §5.2.3.35: the fingerprint names the key exactly; the key
    // ID (§5.2.3.12), 64 bits of it, only hints, and many signatures by
    // older apps carry nothing else.
    // A key ID that matches where the fingerprint doesn't is another key
    // with the same 64 bits, not the signer.
    let fingerprints = signature.issuer_fingerprint();
    let named: Vec<_> = if fingerprints.is_empty() {
        let ids = signature.issuer_key_id();
        all.iter()
            .copied()
            .filter(|(_, v)| ids.iter().any(|id| **id == v.as_dyn().legacy_key_id()))
            .collect()
    } else {
        all.iter()
            .copied()
            .filter(|(_, v)| fingerprints.iter().any(|f| **f == v.as_dyn().fingerprint()))
            .collect()
    };
    if named.is_empty() {
        Signers { keys: all, named: false }
    } else {
        Signers { keys: named, named: true }
    }
}

/// The verdict on `signature`, made from `public` (see `signers`), where
/// `verifies` says whether a key made it. Good (or OtherSender, see
/// `good_from`) when one of its signers' keys did; Bad when a key it names
/// is here but didn't make it; UnknownKey when no key it names is here and
/// no other key made it.
pub(super) fn verdict(
    signature: &pgp::packet::Signature,
    public: &[SignedPublicKey],
    from: Option<&str>,
    verifies: impl Fn(Verifier<'_>) -> bool,
) -> Signature {
    let signers = signers(signature, public);
    for (key, verifier) in &signers.keys {
        if verifies(*verifier) {
            return good_from(key, from);
        }
    }
    if signers.named {
        Signature::Bad
    } else {
        Signature::UnknownKey
    }
}

#[cfg(test)]
mod tests {
    use super::super::{check, test_key};
    use super::*;
    use pgp::composed::{ArmorOptions, Deserializable as _, DetachedSignature, SignedSecretKey, SubpacketConfig};
    use pgp::crypto::hash::HashAlgorithm;
    use pgp::packet::{Subpacket, SubpacketData};
    use pgp::types::{KeyDetails, Password, Timestamp};

    const CONTENT: &[u8] = b"Content-Type: text/plain\r\n\r\nMeet at noon.";

    fn signing_key(k: &SignedSecretKey) -> &pgp::packet::SecretSubkey {
        &k.secret_subkeys
            .iter()
            .find(|s| s.signatures.iter().any(|g| g.key_flags().sign()))
            .unwrap()
            .key
    }

    /// A detached signature by `k`'s signing subkey with only `issuer` as
    /// its issuer subpackets (hashed, as RFC 9580 §5.2.3.35 asks).
    fn signed_with(k: &SignedSecretKey, issuer: Vec<SubpacketData>, content: &[u8]) -> String {
        let mut hashed = vec![Subpacket::regular(SubpacketData::SignatureCreationTime(Timestamp::now())).unwrap()];
        hashed.extend(issuer.into_iter().map(|d| Subpacket::regular(d).unwrap()));
        DetachedSignature::sign_binary_data_with_subpackets(
            rand08::thread_rng(),
            signing_key(k),
            &Password::empty(),
            HashAlgorithm::Sha256,
            content,
            SubpacketConfig::UserDefined { hashed, unhashed: vec![] },
        )
        .unwrap()
        .to_armored_string(ArmorOptions::default())
        .unwrap()
    }

    fn public(keys: &[&SignedSecretKey]) -> Vec<SignedPublicKey> {
        keys.iter().map(|k| SignedPublicKey::from((*k).clone())).collect()
    }

    /// RFC 9580 §5.2.3.35 then §5.2.3.12: the signer is found by the Issuer
    /// Fingerprint, or by the Issuer Key ID where that is all there is.
    #[test]
    fn signers_are_found_by_fingerprint_then_key_id() {
        let ana = test_key("Ana <ana@example.org>", "");
        let cy = test_key("Cy <cy@example.net>", "");
        let ring = public(&[&cy, &ana]);
        let sub = signing_key(&ana);
        let good = Signature::Good("Ana <ana@example.org>".into());
        let by_fingerprint = signed_with(&ana, vec![SubpacketData::IssuerFingerprint(sub.fingerprint())], CONTENT);
        let by_id = signed_with(&ana, vec![SubpacketData::IssuerKeyId(sub.legacy_key_id())], CONTENT);
        let anonymous = signed_with(&ana, vec![], CONTENT);
        for armored in [&by_fingerprint, &by_id, &anonymous] {
            assert_eq!(check(armored, CONTENT, &ring, Some("ana@example.org")), good);
        }
        // Only the named key is tried.
        let (parsed, _) = DetachedSignature::from_string(&by_fingerprint).unwrap();
        let found = signers(&parsed.signature, &ring);
        assert!(found.named);
        assert_eq!(found.keys.len(), 1);
        assert_eq!(found.keys[0].1.as_dyn().fingerprint(), sub.fingerprint());
        let (parsed, _) = DetachedSignature::from_string(&by_id).unwrap();
        assert_eq!(signers(&parsed.signature, &ring).keys.len(), 1);
        // Changed content: Bad when the named key is here, whichever way
        // it is named; unknown when it names nobody or someone not here.
        let changed = b"Content-Type: text/plain\r\n\r\nMeet at nine.";
        assert_eq!(check(&by_fingerprint, changed, &ring, None), Signature::Bad);
        assert_eq!(check(&by_id, changed, &ring, None), Signature::Bad);
        assert_eq!(check(&anonymous, changed, &ring, None), Signature::UnknownKey);
        assert_eq!(check(&by_fingerprint, CONTENT, &public(&[&cy]), None), Signature::UnknownKey);
    }

    /// A key ID that matches where the fingerprint names another key is a
    /// collision, not the signer: the fingerprint decides.
    #[test]
    fn issuer_fingerprints_win_over_key_ids() {
        let ana = test_key("Ana <ana@example.org>", "");
        let cy = test_key("Cy <cy@example.net>", "");
        let ring = public(&[&ana, &cy]);
        let ana_sub = signing_key(&ana);
        let cy_sub = signing_key(&cy);
        // Signed by Ana, naming Cy's fingerprint and Ana's key ID: Cy is
        // tried, and didn't sign it.
        let misnamed = signed_with(
            &ana,
            vec![
                SubpacketData::IssuerFingerprint(cy_sub.fingerprint()),
                SubpacketData::IssuerKeyId(ana_sub.legacy_key_id()),
            ],
            CONTENT,
        );
        let (parsed, _) = DetachedSignature::from_string(&misnamed).unwrap();
        let found = signers(&parsed.signature, &ring);
        assert_eq!(found.keys.len(), 1);
        assert_eq!(found.keys[0].1.as_dyn().fingerprint(), cy_sub.fingerprint());
        assert_eq!(check(&misnamed, CONTENT, &ring, None), Signature::Bad);
    }
}
