//! Version 6 keys (RFC 9580 §5.5.2.3), tested with the RFC's own samples
//! (Appendix A, copied into `rfc9580/`) and keys pgp 0.21 makes.
#![cfg(test)]

use super::keys::{RevocationReason, expires, revocation_certificate, set_expiry};
use super::{Keyring, Protect, Signature, encryption_subkey, fingerprint, open, protect, reads_seipd_v2};
use pgp::composed::{
    ArmorOptions, CleartextSignedMessage, Deserializable as _, Message, SignedPublicKey, SignedSecretKey,
};
use pgp::types::{KeyDetails as _, KeyVersion, Password};

/// RFC 9580 Appendix A.3: the sample v6 certificate (Ed25519 primary
/// with a direct-key signature, X25519 subkey; no user id).
const A3_CERTIFICATE: &str = include_str!("rfc9580/a3-v6-certificate.asc");
/// Appendix A.4: its secret key, unlocked.
const A4_SECRET_KEY: &str = include_str!("rfc9580/a4-v6-secret-key.asc");
/// Appendix A.5: the same secret key locked with Argon2 and AEAD.
const A5_LOCKED_SECRET_KEY: &str = include_str!("rfc9580/a5-v6-locked-secret-key.asc");
const A5_PASSPHRASE: &str = "correct horse battery staple";
/// Appendix A.6: a cleartext-signed message by the sample key.
const A6_CLEARTEXT_SIGNED: &str = include_str!("rfc9580/a6-cleartext-signed.asc");
/// Appendix A.7: the same message and signature, inline-signed.
const A7_INLINE_SIGNED: &str = include_str!("rfc9580/a7-inline-signed.asc");
/// Appendix A.8.5: "Hello, world!" encrypted to the sample key's subkey
/// with X25519, SEIPD v2, AES-128 and OCB.
const A8_ENCRYPTED: &str = include_str!("rfc9580/a8-x25519-aead-ocb.asc");

const PRIMARY_FINGERPRINT: &str = "CB186C4F0609A697E4D52DFA6C722B0C1F1E27C18A56708F6525EC27BAD9ACC9";
const SUBKEY_FINGERPRINT: &str = "12C83F1E706F6308FE151A417743A1F033790E93E9978488D1DB378DA9930885";
const GROCERIES: &str = "What we need from the grocery store:\n\n- tofu\n- vegetables\n- noodles\n";

/// RFC 9580 Appendix A.3: the sample v6 certificate is read and imported,
/// its self-signatures check out, and its 32-byte fingerprints are those
/// the RFC gives (§5.5.4.3).
#[test]
fn rfc9580_sample_v6_certificate_is_imported() {
    let (key, _) = SignedPublicKey::from_string(A3_CERTIFICATE).unwrap();
    assert_eq!(key.primary_key.version(), KeyVersion::V6);
    key.verify_bindings().unwrap();
    assert_eq!(fingerprint(&key), PRIMARY_FINGERPRINT);
    assert_eq!(key.public_subkeys.len(), 1);
    assert_eq!(fingerprint(&key.public_subkeys[0].key), SUBKEY_FINGERPRINT);
    assert!(key.details.users.is_empty(), "the sample has no user id");
    assert_eq!(expires(&key), None);
    // Its X25519 subkey is for encryption, and the key reads SEIPD v2.
    assert_eq!(encryption_subkey(&key).map(|s| fingerprint(&s.key)).as_deref(), Some(SUBKEY_FINGERPRINT));
    assert!(reads_seipd_v2(&key));

    let mut ring = Keyring::default();
    assert_eq!(ring.import(A3_CERTIFICATE, "").unwrap(), 1);
    let listed = ring.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].fingerprint, PRIMARY_FINGERPRINT);
    assert!(!listed[0].secret);
}

/// RFC 9580 Appendix A.4: the sample v6 secret key imports, and decrypts
/// what was encrypted to it (Appendix A.8).
#[test]
fn rfc9580_sample_v6_secret_key_is_imported() {
    let mut ring = Keyring::default();
    assert_eq!(ring.import(A4_SECRET_KEY, "").unwrap(), 1);
    assert_eq!(ring.list()[0].fingerprint, PRIMARY_FINGERPRINT);
    assert!(ring.list()[0].secret);
    let (message, _) = Message::from_string(A8_ENCRYPTED).unwrap();
    let mut opened = message.decrypt(&Password::empty(), &ring.secret_keys()[0]).unwrap();
    assert_eq!(opened.as_data_string().unwrap(), "Hello, world!");
}

/// RFC 9580 Appendix A.5: the locked copy of the sample key opens with
/// its passphrase and not without it. Ignored by default: its Argon2
/// parameters (t = 1, p = 4, m = 2^21 KiB, §3.7.1.4) take 2 GiB of memory
/// for each try, and a minute and a half in a debug build. Run with
/// `cargo test --bin picacho rfc9580_sample_locked -- --ignored`.
#[test]
#[ignore = "Argon2 with 2 GiB of memory per try"]
fn rfc9580_sample_locked_v6_secret_key_opens_with_its_passphrase() {
    let mut locked = Keyring::default();
    assert!(locked.import(A5_LOCKED_SECRET_KEY, "").unwrap_err().to_string().contains("passphrase"));
    assert!(locked.import(A5_LOCKED_SECRET_KEY, "wrong").is_err());
    assert_eq!(locked.import(A5_LOCKED_SECRET_KEY, A5_PASSPHRASE).unwrap(), 1);
    assert_eq!(locked.list()[0].fingerprint, PRIMARY_FINGERPRINT);
    // Kept unlocked: it decrypts without the passphrase (Appendix A.8).
    let (message, _) = Message::from_string(A8_ENCRYPTED).unwrap();
    let mut opened = message.decrypt(&Password::empty(), &locked.secret_keys()[0]).unwrap();
    assert_eq!(opened.as_data_string().unwrap(), "Hello, world!");
}

/// RFC 9580 Appendix A.6 and A.7: the sample v6 signatures verify, as
/// inline PGP mail (cleartext-signed, and as an OpenPGP message), with the
/// sample key imported; changed text doesn't.
#[test]
fn rfc9580_sample_v6_signatures_verify_in_mail() {
    let (message, _) = CleartextSignedMessage::from_string(A6_CLEARTEXT_SIGNED).unwrap();
    let (key, _) = SignedPublicKey::from_string(A3_CERTIFICATE).unwrap();
    message.verify(&key.primary_key).unwrap();

    let mut ring = Keyring::default();
    ring.import(A3_CERTIFICATE, "").unwrap();
    // The key has no user id: named by its fingerprint, and not the
    // sender's word, as nothing ties it to the From address.
    let mail = |body: &str| {
        format!("From: Grocer <grocer@example.org>\r\nSubject: List\r\nContent-Type: text/plain\r\n\r\n{body}\r\n")
    };
    let (readable, verdict) = open(mail(A6_CLEARTEXT_SIGNED).as_bytes(), &ring).unwrap();
    assert_eq!(verdict.signature, Signature::OtherSender(PRIMARY_FINGERPRINT.into()));
    let text = String::from_utf8_lossy(&readable).replace("\r\n", "\n");
    assert!(text.ends_with(GROCERIES), "{text}");
    let changed = A6_CLEARTEXT_SIGNED.replace("- - tofu", "- - beef");
    assert!(!matches!(
        open(mail(&changed).as_bytes(), &ring).unwrap().1.signature,
        Signature::Good(_) | Signature::OtherSender(_)
    ));
    // Without the key: signed, by a key you haven't imported.
    let (_, verdict) = open(mail(A6_CLEARTEXT_SIGNED).as_bytes(), &Keyring::default()).unwrap();
    assert_eq!(verdict.signature, Signature::UnknownKey);

    // A.7, the same signature one-pass signed in an OpenPGP message.
    let (mut inline, _) = Message::from_string(A7_INLINE_SIGNED).unwrap();
    assert_eq!(inline.as_data_string().unwrap(), GROCERIES);
    let verdict = super::nested::message_signature(&inline, &ring.public_keys(), None);
    assert_eq!(verdict, Signature::Good(PRIMARY_FINGERPRINT.into()));
}

/// RFC 9580 Appendix A.8: mail encrypted to the sample v6 key (X25519,
/// PKESK v6, SEIPD v2 with OCB) opens with the sample secret key, as
/// PGP/MIME.
#[test]
fn rfc9580_sample_v6_encrypted_message_opens() {
    let mut ring = Keyring::default();
    ring.import(A4_SECRET_KEY, "").unwrap();
    let armored = A8_ENCRYPTED.replace('\n', "\r\n");
    let mail = format!(
        "From: Grocer <grocer@example.org>\r\nSubject: Hi\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n\r\n--b\r\nContent-Type: application/octet-stream\r\n\r\n{armored}\r\n--b--\r\n"
    );
    let (readable, verdict) = open(mail.as_bytes(), &ring).unwrap();
    assert!(verdict.decrypted);
    assert_eq!(verdict.signature, Signature::None);
    assert!(readable.ends_with(b"Hello, world!"));
}

/// A v6 key pgp 0.21 makes (Ed25519 and X25519, RFC 9580 §5.5.5): mail
/// to it is encrypted with SEIPD v2 and opens; mail it signs verifies;
/// its expiry is set (on the direct-key signature too, where a v6 key
/// keeps its preferences) and it is revoked like a v4 key.
#[test]
fn v6_keys_sign_encrypt_expire_and_revoke() {
    let ana = v6_key("Ana <ana@example.org>");
    let bo = v6_key("Bo <bo@example.se>");
    assert_eq!(ana.primary_key.version(), KeyVersion::V6);
    let armored_secret = |k: &SignedSecretKey| k.to_armored_string(ArmorOptions::default()).unwrap();
    let armored_public =
        |k: &SignedSecretKey| SignedPublicKey::from(k.clone()).to_armored_string(ArmorOptions::default()).unwrap();
    let mut anas = Keyring::default();
    anas.import(&armored_secret(&ana), "").unwrap();
    anas.import(&armored_public(&bo), "").unwrap();
    let mut bos = Keyring::default();
    bos.import(&armored_secret(&bo), "").unwrap();
    bos.import(&armored_public(&ana), "").unwrap();

    let message = b"From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nContent-Type: text/plain\r\n\r\nMeet at noon.\r\n";
    let both = Protect { sign: true, encrypt: true, ..Default::default() };
    let sent = protect(message, "ana@example.org", &["bo@example.se".into()], both, &anas).unwrap();
    let (armored, _) = crate::manage::crypto::encrypted_part(&sent).unwrap();
    let (encrypted, _) = Message::from_string(&armored).unwrap();
    let Message::Encrypted { esk, .. } = &encrypted else { panic!("encrypted: {encrypted:?}") };
    assert!(esk.iter().all(|e| matches!(
        e,
        pgp::composed::Esk::PublicKeyEncryptedSessionKey(p) if p.version() == pgp::types::PkeskVersion::V6
    )));
    let (readable, verdict) = open(&sent, &bos).unwrap();
    assert!(verdict.decrypted);
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
    assert!(readable.ends_with(b"Meet at noon."));

    // Expiry: on the direct-key signature and every self-signature.
    let year = std::time::Duration::from_secs(365 * 86_400);
    let lasting = set_expiry(&ana, Some(year)).unwrap();
    let public = SignedPublicKey::from(lasting.clone());
    public.verify_bindings().unwrap();
    let made = i64::from(public.primary_key.created_at().as_secs());
    assert_eq!(expires(&public), Some(made + year.as_secs() as i64));
    let direct = public.details.direct_signatures.last().unwrap();
    assert_eq!(direct.key_expiration_time().map(|d| d.as_secs()), Some(year.as_secs() as u32));
    assert!(public.details.direct_signatures.len() == 2, "the old one kept, superseded");
    assert_eq!(direct.preferred_aead_algs(), ana.details.direct_signatures[0].preferred_aead_algs());

    // Revoked: Bo no longer encrypts to it.
    let certificate = revocation_certificate(&ana, RevocationReason::Retired, "").unwrap();
    let (revoked, _) = SignedPublicKey::from_string(&certificate).unwrap();
    revoked.verify_bindings().unwrap();
    bos.import(&certificate, "").unwrap();
    assert!(bos.key_for("ana@example.org").is_none());
}

/// The SEIPD version (RFC 9580 §5.13) of the PGP/MIME message `sent`,
/// and the version of each of its PKESK packets (§5.1).
fn encryption_versions(sent: &[u8]) -> (usize, Vec<pgp::types::PkeskVersion>) {
    use pgp::composed::{Edata, Esk};
    use pgp::packet::{ProtectedDataConfig, SymEncryptedProtectedDataConfig};
    let (armored, _) = crate::manage::crypto::encrypted_part(sent).unwrap();
    let (encrypted, _) = Message::from_string(&armored).unwrap();
    let Message::Encrypted { esk, edata, .. } = &encrypted else { panic!("encrypted: {encrypted:?}") };
    let Edata::SymEncryptedProtectedData { reader } = edata else { panic!("SEIPD: {edata:?}") };
    let seipd = match reader.config() {
        ProtectedDataConfig::Seipd(SymEncryptedProtectedDataConfig::V1) => 1,
        ProtectedDataConfig::Seipd(SymEncryptedProtectedDataConfig::V2 { .. }) => 2,
        other => panic!("not SEIPD: {other:?}"),
    };
    let pkesks = esk
        .iter()
        .map(|e| match e {
            Esk::PublicKeyEncryptedSessionKey(p) => p.version(),
            other => panic!("not a PKESK: {other:?}"),
        })
        .collect();
    (seipd, pkesks)
}

/// Two keyrings, Ana's and Bo's, each with its own secret key and the
/// other's public key.
fn exchanged(ana: &Keyring, bo: &Keyring) -> (Keyring, Keyring) {
    let public = |ring: &Keyring| {
        SignedPublicKey::from(ring.secret_keys()[0].clone()).to_armored_string(ArmorOptions::default()).unwrap()
    };
    let (mut anas, mut bos) = (ana.clone(), bo.clone());
    anas.import(&public(bo), "").unwrap();
    bos.import(&public(ana), "").unwrap();
    (anas, bos)
}

const PLANS: &[u8] =
    b"From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nContent-Type: text/plain\r\n\r\nMeet at noon.\r\n";

/// A v6 key Picacho makes (RFC 9580 §5.5.2.3) is Ed25519 and X25519, not
/// the legacy EdDSA and ECDH of v4 keys, with its preferences on its
/// direct-key signature: AEAD ciphersuites (§5.2.3.15) and Features
/// saying it reads SEIPD v1 and v2 (§5.2.3.32). Mail it signs and
/// encrypts to another such key opens, with SEIPD v2 and v6 PKESKs
/// (§10.3.2.1), and its signature is good.
#[test]
fn v6_keys_picacho_makes_sign_and_encrypt_mail_picacho_opens() {
    use pgp::crypto::{aead::AeadAlgorithm, public_key::PublicKeyAlgorithm, sym::SymmetricKeyAlgorithm};
    let mut ana = Keyring::default();
    let fp = ana.create("Ana", "ana@example.org", None, super::KeyVersion::V6).unwrap();
    let mut bo = Keyring::default();
    bo.create("Bo", "bo@example.se", None, super::KeyVersion::V6).unwrap();

    let key = SignedPublicKey::from(ana.own_key("ana@example.org").unwrap());
    key.verify_bindings().unwrap();
    assert_eq!(fp.len(), 64, "a v6 fingerprint is 32 bytes (§5.5.4.3)");
    assert_eq!(key.primary_key.version(), KeyVersion::V6);
    assert_eq!(key.primary_key.algorithm(), PublicKeyAlgorithm::Ed25519);
    let algorithms: Vec<_> = key.public_subkeys.iter().map(|s| (s.key.version(), s.key.algorithm())).collect();
    assert_eq!(
        algorithms,
        [(KeyVersion::V6, PublicKeyAlgorithm::Ed25519), (KeyVersion::V6, PublicKeyAlgorithm::X25519)]
    );
    let [direct] = key.details.direct_signatures.as_slice() else { panic!("one direct-key signature") };
    let features = direct.features().unwrap();
    assert!(features.seipd_v1() && features.seipd_v2());
    assert_eq!(
        direct.preferred_aead_algs(),
        [(SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb), (SymmetricKeyAlgorithm::AES128, AeadAlgorithm::Ocb)]
    );
    assert_eq!(direct.preferred_symmetric_algs(), [SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES128]);
    assert!(reads_seipd_v2(&key));
    assert_eq!(super::keys::expires(&key), None, "never, unless asked");

    let (anas, bos) = exchanged(&ana, &bo);
    let both = Protect { sign: true, encrypt: true, ..Default::default() };
    let sent = protect(PLANS, "ana@example.org", &["bo@example.se".into()], both, &anas).unwrap();
    let (seipd, pkesks) = encryption_versions(&sent);
    assert_eq!(seipd, 2);
    assert_eq!(pkesks, [pgp::types::PkeskVersion::V6; 2], "to Bo, and to Ana for Sent");
    let (readable, verdict) = open(&sent, &bos).unwrap();
    assert!(verdict.decrypted);
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
    assert!(readable.ends_with(b"Meet at noon."));
    assert!(open(&sent, &anas).unwrap().0.ends_with(b"Meet at noon."), "Ana's copy in Sent");

    // Signed only: a v6 signature (§5.2: a v6 key makes v6 signatures).
    let signed = Protect { sign: true, ..Default::default() };
    let sent = protect(PLANS, "ana@example.org", &["bo@example.se".into()], signed, &anas).unwrap();
    assert_eq!(open(&sent, &bos).unwrap().1.signature, Signature::Good("Ana <ana@example.org>".into()));
}

/// A v6 key Picacho makes to expire does (RFC 9580 §5.2.3.13), with the
/// expiry on its direct-key signature, where a v6 key keeps it
/// (§5.2.3.10), and on its subkeys' bindings; renewed, it lasts longer,
/// or for ever, and still signs and encrypts.
#[test]
fn v6_keys_picacho_makes_expire_and_renew() {
    let two_years = std::time::Duration::from_secs(2 * 365 * 86_400);
    let mut ana = Keyring::default();
    let fp = ana.create("Ana", "ana@example.org", Some(two_years), super::KeyVersion::V6).unwrap();
    let wanted = chrono::Utc::now().timestamp() + two_years.as_secs() as i64;
    assert!((ana.list()[0].expires.unwrap() - wanted).abs() <= 2);
    let key = SignedPublicKey::from(ana.own_key("ana@example.org").unwrap());
    key.verify_bindings().unwrap();
    assert_eq!(key.primary_key.version(), KeyVersion::V6);
    let [direct] = key.details.direct_signatures.as_slice() else { panic!("replaced, not superseded") };
    assert!(direct.key_expiration_time().is_some());
    assert!(direct.features().unwrap().seipd_v2(), "its preferences kept");
    for sub in &key.public_subkeys {
        let [binding] = sub.signatures.as_slice() else { panic!("one binding") };
        let lasts = binding.key_expiration_time().unwrap().as_secs();
        assert!((i64::from(sub.key.created_at().as_secs()) + i64::from(lasts) - wanted).abs() <= 2);
    }

    // Renewed for five years from now, then for ever.
    let five_years = std::time::Duration::from_secs(5 * 365 * 86_400);
    ana.renew(&fp, Some(five_years)).unwrap();
    let wanted = chrono::Utc::now().timestamp() + five_years.as_secs() as i64;
    assert!((ana.list()[0].expires.unwrap() - wanted).abs() <= 2);
    let key = SignedPublicKey::from(ana.own_key("ana@example.org").unwrap());
    key.verify_bindings().unwrap();
    assert!(reads_seipd_v2(&key), "the renewed direct-key signature keeps the features");
    ana.renew(&fp, None).unwrap();
    assert_eq!(ana.list()[0].expires, None);

    let mut bo = Keyring::default();
    bo.create("Bo", "bo@example.se", None, super::KeyVersion::V6).unwrap();
    let (anas, bos) = exchanged(&ana, &bo);
    assert!(bos.key_for("ana@example.org").is_some());
    let both = Protect { sign: true, encrypt: true, ..Default::default() };
    let sent = protect(PLANS, "ana@example.org", &["bo@example.se".into()], both, &anas).unwrap();
    let (_, verdict) = open(&sent, &bos).unwrap();
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
}

/// Keys are v4 unless v6 is asked for, made as before: legacy EdDSA and
/// ECDH over Curve25519, which every OpenPGP app reads, with no direct-key
/// signature, and mail between two of them uses SEIPD v1 (they don't say
/// they read v2, §5.2.3.32).
#[test]
fn new_keys_stay_v4_by_default() {
    use pgp::crypto::public_key::PublicKeyAlgorithm;
    assert_eq!(super::KeyVersion::default(), super::KeyVersion::V4);
    let mut ana = Keyring::default();
    let fp = ana.create("Ana", "ana@example.org", None, super::KeyVersion::default()).unwrap();
    assert_eq!(fp.len(), 40, "a v4 fingerprint is 20 bytes (§5.5.4.2)");
    let key = SignedPublicKey::from(ana.own_key("ana@example.org").unwrap());
    assert_eq!(key.primary_key.version(), KeyVersion::V4);
    assert_eq!(key.primary_key.algorithm(), PublicKeyAlgorithm::EdDSALegacy);
    let algorithms: Vec<_> = key.public_subkeys.iter().map(|s| (s.key.version(), s.key.algorithm())).collect();
    assert_eq!(
        algorithms,
        [(KeyVersion::V4, PublicKeyAlgorithm::EdDSALegacy), (KeyVersion::V4, PublicKeyAlgorithm::ECDH)]
    );
    assert!(key.details.direct_signatures.is_empty());
    assert!(!reads_seipd_v2(&key));

    let mut bo = Keyring::default();
    bo.create("Bo", "bo@example.se", None, super::KeyVersion::default()).unwrap();
    let (anas, bos) = exchanged(&ana, &bo);
    let both = Protect { sign: true, encrypt: true, ..Default::default() };
    let sent = protect(PLANS, "ana@example.org", &["bo@example.se".into()], both, &anas).unwrap();
    assert_eq!(encryption_versions(&sent), (1, vec![pgp::types::PkeskVersion::V3; 2]));
    assert_eq!(open(&sent, &bos).unwrap().1.signature, Signature::Good("Ana <ana@example.org>".into()));
}

/// Mail to a v6 recipient, from a v4 sender whose own key says it reads
/// SEIPD v2 too, is SEIPD v2 with v6 PKESKs (RFC 9580 §10.3.2.1, §13.7).
/// Mail to a v6 and a v4 recipient whose key doesn't say so is one
/// SEIPD v1 message with v3 PKESKs, which both open: §10.3.2.1 doesn't
/// let v3 and v6 PKESKs share a payload, and a v6 key reads v1 too.
#[test]
fn mixed_v4_and_v6_recipients_get_seipd_v1_and_v6_alone_gets_v2() {
    let mut ana = Keyring::default();
    ana.create("Ana", "ana@example.org", None, super::KeyVersion::V6).unwrap();
    let mut bo = Keyring::default();
    bo.create("Bo", "bo@example.se", None, super::KeyVersion::V6).unwrap();
    let mut cy = Keyring::default();
    cy.create("Cy", "cy@example.net", None, super::KeyVersion::V4).unwrap();
    let public = |ring: &Keyring| {
        SignedPublicKey::from(ring.secret_keys()[0].clone()).to_armored_string(ArmorOptions::default()).unwrap()
    };
    let mut anas = ana.clone();
    anas.import(&public(&bo), "").unwrap();
    anas.import(&public(&cy), "").unwrap();
    for ring in [&mut bo, &mut cy] {
        ring.import(&public(&ana), "").unwrap();
    }
    let both = Protect { sign: true, encrypt: true, ..Default::default() };
    let message = |to: &str| {
        format!("From: Ana <ana@example.org>\r\nTo: {to}\r\nSubject: Plans\r\nContent-Type: text/plain\r\n\r\nMeet at noon.\r\n")
    };

    // To Bo alone: both keys say they read SEIPD v2.
    let sent = protect(message("bo@example.se").as_bytes(), "ana@example.org", &["bo@example.se".into()], both, &anas)
        .unwrap();
    assert_eq!(encryption_versions(&sent), (2, vec![pgp::types::PkeskVersion::V6; 2]));
    assert!(open(&sent, &bo).unwrap().0.ends_with(b"Meet at noon."));

    // To Bo and Cy, whose v4 key doesn't: SEIPD v1, read by all three.
    let to = ["bo@example.se".to_owned(), "cy@example.net".to_owned()];
    let sent = protect(message("bo@example.se, cy@example.net").as_bytes(), "ana@example.org", &to, both, &anas)
        .unwrap();
    assert_eq!(encryption_versions(&sent), (1, vec![pgp::types::PkeskVersion::V3; 3]));
    for (ring, who) in [(&bo, "Bo"), (&cy, "Cy"), (&anas, "Ana, in Sent")] {
        let (readable, verdict) = open(&sent, ring).unwrap_or_else(|| panic!("{who} opens it"));
        assert!(verdict.decrypted, "{who}");
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()), "{who}");
        assert!(readable.ends_with(b"Meet at noon."), "{who}");
    }
}

/// RFC 9580 §13.7: a v6 key with no Features subpacket reads SEIPD v2;
/// §5.2.3.15: a key that lists no AEAD ciphersuite still takes AES-128
/// with OCB, which mail to it then uses. The RFC's sample certificate
/// (Appendix A.3) lists AES-256 and AES-128 with OCB, and gets AES-256.
#[test]
fn v6_keys_without_features_read_seipd_v2_with_the_implied_suite() {
    use pgp::crypto::{aead::AeadAlgorithm, sym::SymmetricKeyAlgorithm};
    use pgp::packet::{SignatureConfig, SignatureType, Subpacket, SubpacketData};
    let ana = v6_key("Ana <ana@example.org>");
    // Its direct-key signature made again without features or AEAD
    // preferences.
    let primary = &ana.primary_key;
    let mut config = SignatureConfig::from_key(rand08::thread_rng(), primary, SignatureType::Key).unwrap();
    config.hashed_subpackets = vec![
        Subpacket::regular(SubpacketData::SignatureCreationTime(pgp::types::Timestamp::now())).unwrap(),
        Subpacket::regular(SubpacketData::IssuerFingerprint(primary.fingerprint())).unwrap(),
    ];
    let bare = config.sign_key(primary, &Password::empty(), primary.public_key()).unwrap();
    let mut public = SignedPublicKey::from(ana.clone());
    public.details.direct_signatures = vec![bare];
    assert!(
        public.details.users.iter().flat_map(|u| u.signatures.iter()).all(|s| s.features().is_none()),
        "a v6 key's user id binding has none"
    );
    public.verify_bindings().unwrap();
    assert!(super::reads_seipd_v2(&public));
    assert_eq!(super::seipd_v2_suite(&[public.clone()]), (SymmetricKeyAlgorithm::AES128, AeadAlgorithm::Ocb));

    let (sample, _) = SignedPublicKey::from_string(A3_CERTIFICATE).unwrap();
    assert_eq!(super::seipd_v2_suite(&[sample.clone()]), (SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb));
    assert_eq!(super::seipd_v2_suite(&[sample, public]), (SymmetricKeyAlgorithm::AES128, AeadAlgorithm::Ocb));
}

/// A v6 key with a signing and an encryption subkey, for `uid`.
fn v6_key(uid: &str) -> SignedSecretKey {
    use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use pgp::crypto::{aead::AeadAlgorithm, sym::SymmetricKeyAlgorithm};
    let mut sign = SubkeyParamsBuilder::default();
    sign.version(KeyVersion::V6).key_type(KeyType::Ed25519).can_sign(true);
    let mut encrypt = SubkeyParamsBuilder::default();
    encrypt.version(KeyVersion::V6).key_type(KeyType::X25519).can_encrypt(EncryptionCaps::All);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .version(KeyVersion::V6)
        .key_type(KeyType::Ed25519)
        .can_certify(true)
        .primary_user_id(uid.into())
        .preferred_symmetric_algorithms(vec![SymmetricKeyAlgorithm::AES256].into())
        .preferred_aead_algorithms(vec![(SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb)].into())
        .feature_seipd_v2(true)
        .subkeys(vec![sign.build().unwrap(), encrypt.build().unwrap()]);
    params.build().unwrap().generate(rand08::thread_rng()).unwrap()
}
