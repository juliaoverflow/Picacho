//! Messages protected more than once: an encrypted message inside another
//! (a forwarder that re-encrypts, a signed message encrypted twice), at the
//! MIME level (RFC 3156 parts nested in each other) and at the OpenPGP
//! packet level (RFC 9580 §10.3.1: decrypting or decompressing yields a new
//! OpenPGP message, which may itself be encrypted, compressed or signed).
use super::{Keyring, Signature, Verdict};
use pgp::composed::{Message, SignedPublicKey, SignedSecretKey};
use pgp::types::Password;

/// How many layers a message may have: each PGP/MIME part opened counts
/// one, and so does each decryption and each decompression inside it. RFC
/// 9580 §10.3.1 sets no limit; a message over this one is treated as one
/// that can't be opened, so that a crafted message can't make reading it
/// go on without end. A message re-encrypted by a forwarder, compressed at
/// each step, takes six.
pub(super) const MAX_NESTING: usize = 16;

/// Counts one more layer. False once there are more than `MAX_NESTING`.
pub(super) fn descend(depth: &mut usize) -> bool {
    *depth += 1;
    *depth <= MAX_NESTING
}

/// Opens `raw` (see `super::open_layer`) and then, while what came out is
/// itself PGP/MIME (multipart/encrypted, RFC 3156 §4, or multipart/signed,
/// §5), that too. `depth` counts the layers opened so far.
pub(super) fn open_layers(raw: &[u8], ring: &Keyring, depth: &mut usize) -> Option<(Vec<u8>, Verdict)> {
    let (mut readable, mut verdict) = super::open_layer(raw, ring, depth)?;
    while is_pgp_mime(&readable) {
        if *depth >= MAX_NESTING {
            return None;
        }
        // A layer that can't be opened (encrypted to a key you don't have)
        // is shown as it is, as a single layer always was.
        let Some((inner, inner_verdict)) = super::open_layer(&readable, ring, depth) else {
            break;
        };
        verdict = combine(verdict, inner_verdict);
        readable = inner;
    }
    Some((readable, verdict))
}

/// Whether `raw` is a PGP/MIME message: multipart/encrypted (RFC 3156 §4)
/// or multipart/signed (RFC 3156 §5).
fn is_pgp_mime(raw: &[u8]) -> bool {
    crate::manage::crypto::encrypted_part(raw).is_some() || super::signed_parts(raw).is_some()
}

/// What two layers of one message say together. Decrypted if either was.
/// Of the signatures, a bad one wins (something was changed), then a good
/// one, then one by an unknown key; between two of the same rank the inner
/// one, nearer the text, wins.
pub(super) fn combine(outer: Verdict, inner: Verdict) -> Verdict {
    Verdict {
        decrypted: outer.decrypted || inner.decrypted,
        signature: stronger(outer.signature, inner.signature),
    }
}

fn stronger(outer: Signature, inner: Signature) -> Signature {
    fn rank(s: &Signature) -> u8 {
        match s {
            Signature::None => 0,
            Signature::UnknownKey => 1,
            Signature::Good(_) | Signature::OtherSender(_) => 2,
            Signature::Bad => 3,
        }
    }
    if rank(&outer) > rank(&inner) { outer } else { inner }
}

/// What the first layer of a message that isn't a signature over another
/// is (a signed layer passes decryption and decompression on to the
/// message it signs: pgp's `Message::Signed` does so itself).
enum Layer {
    Encrypted,
    Compressed,
    Readable,
}

fn innermost(message: &Message<'_>) -> Layer {
    match message {
        Message::Signed { reader, .. } => innermost(reader.get_ref()),
        Message::Encrypted { .. } => Layer::Encrypted,
        Message::Compressed { .. } => Layer::Compressed,
        Message::Literal { .. } => Layer::Readable,
    }
}

/// RFC 9580 §10.3.1: decrypts and decompresses `message` until what is
/// left is signed or literal data, each step counted in `depth`. `None`
/// when a layer can't be decrypted with `secret`, there are too many, or
/// nothing was encrypted (it is opened as encrypted, and was claimed to be).
pub(super) fn unwrap_message<'a>(
    mut message: Message<'a>,
    secret: &[SignedSecretKey],
    depth: &mut usize,
) -> Option<Message<'a>> {
    let passwords: Vec<Password> = secret.iter().map(|_| Password::empty()).collect();
    let mut decrypted = false;
    loop {
        match innermost(&message) {
            Layer::Readable => return decrypted.then_some(message),
            Layer::Encrypted => {
                if !descend(depth) {
                    return None;
                }
                message = message
                    .decrypt_with_keys(passwords.iter().collect(), secret.iter().collect())
                    .ok()?;
                decrypted = true;
            }
            Layer::Compressed => {
                if !descend(depth) {
                    return None;
                }
                message = message.decompress().ok()?;
            }
        }
    }
}

/// The signature on a message read to its end (see `unwrap_message`):
/// every signature of every signed layer is checked against `public`, a
/// layer is good when one of its signatures is, and the layers' verdicts
/// are combined (see `combine`). `Signature::None` when nothing was signed.
pub(super) fn message_signature(message: &Message<'_>, public: &[SignedPublicKey], from: Option<&str>) -> Signature {
    let mut signature = Signature::None;
    let mut layer = message;
    while let Message::Signed { reader, .. } = layer {
        let mut verdict = Signature::UnknownKey;
        'signatures: for index in 0..reader.num_signatures() {
            for (key, verifier) in super::verifiers(public) {
                if layer.verify_nested_explicit(index, verifier).is_ok() {
                    verdict = super::good_from(key, from);
                    break 'signatures;
                }
            }
        }
        signature = stronger(signature, verdict);
        layer = reader.get_ref();
    }
    signature
}

#[cfg(test)]
mod tests {
    use super::super::{Protect, encryption_subkey, open, protect, test_key};
    use super::*;
    use pgp::composed::{ArmorOptions, MessageBuilder, SignedPublicKey};
    use pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
    use pgp::packet::{Packet, PublicKeyEncryptedSessionKey, SymEncryptedProtectedData};

    const MESSAGE: &[u8] = b"From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nMessage-ID: <1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.\r\n";
    const BODY: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.";

    struct People {
        ana: SignedSecretKey,
        bo: SignedSecretKey,
        anas: Keyring,
        bos: Keyring,
    }

    fn armored_public(k: &SignedSecretKey) -> String {
        SignedPublicKey::from(k.clone()).to_armored_string(ArmorOptions::default()).unwrap()
    }

    fn people() -> People {
        let ana = test_key("Ana <ana@example.org>", "");
        let bo = test_key("Bo <bo@example.se>", "");
        let mut anas = Keyring::default();
        anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        anas.import(&armored_public(&bo), "").unwrap();
        let mut bos = Keyring::default();
        bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        bos.import(&armored_public(&ana), "").unwrap();
        People { ana, bo, anas, bos }
    }

    /// `plain` as the packets of an encrypted message to `to` (RFC 9580
    /// §10.3: PKESK, then SEIPD v1), its plaintext taken as it is: an
    /// OpenPGP message, which RFC 9580 §10.3.1 lets be encrypted again.
    fn encrypt_packets(plain: &[u8], to: &SignedSecretKey) -> Vec<u8> {
        use pgp::ser::Serialize as _;
        let public = SignedPublicKey::from(to.clone());
        let sub = encryption_subkey(&public).unwrap();
        let alg = SymmetricKeyAlgorithm::AES256;
        let session_key = alg.new_session_key(rand08::thread_rng());
        let esk = PublicKeyEncryptedSessionKey::from_session_key_v3(rand08::thread_rng(), &session_key, alg, &sub.key)
            .unwrap();
        let data = SymEncryptedProtectedData::encrypt_seipdv1(rand08::thread_rng(), alg, session_key.as_ref(), plain)
            .unwrap();
        vec![Packet::from(esk), Packet::from(data)].to_bytes().unwrap()
    }

    /// The packets of `body` signed by `signer` and encrypted to `to`, as
    /// one OpenPGP message (one-pass signed, RFC 9580 §10.3).
    fn signed_and_encrypted(body: &[u8], signer: &SignedSecretKey, to: &SignedSecretKey) -> Vec<u8> {
        let public = SignedPublicKey::from(to.clone());
        let signing = signer
            .secret_subkeys
            .iter()
            .find(|s| s.signatures.iter().any(|g| g.key_flags().sign()))
            .unwrap();
        let mut builder = MessageBuilder::from_bytes("", body.to_vec())
            .seipd_v1(rand08::thread_rng(), SymmetricKeyAlgorithm::AES256);
        builder
            .sign(&signing.key, Password::empty(), HashAlgorithm::Sha256)
            .encrypt_to_key(rand08::thread_rng(), &encryption_subkey(&public).unwrap().key)
            .unwrap();
        builder.to_vec(rand08::thread_rng()).unwrap()
    }

    /// OpenPGP message packets in a PGP/MIME envelope (RFC 3156 §4).
    fn pgp_mime(packets: &[u8]) -> Vec<u8> {
        struct Raw<'a>(&'a [u8]);
        impl pgp::ser::Serialize for Raw<'_> {
            fn to_writer<W: std::io::Write>(&self, w: &mut W) -> pgp::errors::Result<()> {
                w.write_all(self.0)?;
                Ok(())
            }
            fn write_len(&self) -> usize {
                self.0.len()
            }
        }
        let mut armored = Vec::new();
        pgp::armor::write(&Raw(packets), pgp::armor::BlockType::Message, &mut armored, None, true).unwrap();
        let armored = String::from_utf8(armored).unwrap().replace("\r\n", "\n").replace('\n', "\r\n");
        format!(
            "From: Ana <ana@example.org>\r\nTo: Bo <bo@example.se>\r\nSubject: Plans\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n\r\n--b\r\nContent-Type: application/octet-stream\r\n\r\n{armored}\r\n--b--\r\n"
        )
        .into_bytes()
    }

    fn body_of(readable: &[u8]) -> String {
        mailparse::parse_mail(readable).unwrap().get_body().unwrap().trim().to_owned()
    }

    /// RFC 9580 §10.3.1: a signed message inside an encrypted one inside
    /// another encrypted one opens, and its signature is checked.
    #[test]
    fn signed_message_encrypted_twice_opens_with_its_signature() {
        let p = people();
        let inner = signed_and_encrypted(BODY, &p.ana, &p.bo);
        let sent = pgp_mime(&encrypt_packets(&inner, &p.bo));
        let (readable, verdict) = open(&sent, &p.bos).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        assert_eq!(body_of(&readable), "Meet at noon.");
        // Three encryption layers.
        let sent = pgp_mime(&encrypt_packets(&encrypt_packets(&inner, &p.bo), &p.bo));
        let (_, verdict) = open(&sent, &p.bos).unwrap();
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        // Without Ana's key the signature is by an unknown key, not lost.
        let mut alone = Keyring::default();
        alone.import(&p.bo.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
        assert_eq!(open(&sent, &alone).unwrap().1.signature, Signature::UnknownKey);
        // A layer for a key that isn't yours: nothing opens.
        let for_ana = pgp_mime(&encrypt_packets(&inner, &p.ana));
        assert!(open(&for_ana, &p.bos).is_none());
    }

    /// A PGP/MIME message encrypted again as a whole (a forwarder that
    /// re-encrypts what it got): the multipart/encrypted part inside is
    /// opened too, and the signed part inside that checked (RFC 3156 §6.1).
    #[test]
    fn pgp_mime_inside_pgp_mime_opens() {
        let p = people();
        let both = Protect { sign: true, encrypt: true, ..Default::default() };
        let once = protect(MESSAGE, "ana@example.org", &["bo@example.se".into()], both, &p.anas).unwrap();
        let only_encrypt = Protect { encrypt: true, ..Default::default() };
        let twice = protect(&once, "ana@example.org", &["bo@example.se".into()], only_encrypt, &p.anas).unwrap();
        let (readable, verdict) = open(&twice, &p.bos).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        assert_eq!(body_of(&readable), "Meet at noon.");
        // A signed-only message encrypted: its signature comes through.
        let sign = Protect { sign: true, ..Default::default() };
        let signed = protect(MESSAGE, "ana@example.org", &[], sign, &p.anas).unwrap();
        let sealed = protect(&signed, "ana@example.org", &["bo@example.se".into()], only_encrypt, &p.anas).unwrap();
        let (readable, verdict) = open(&sealed, &p.bos).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
        assert_eq!(body_of(&readable), "Meet at noon.");
        // The inner signature broken: Bad, even under a layer that opens.
        let forged = String::from_utf8_lossy(&signed).replace("Meet at noon.", "Meet at nine.");
        let sealed = protect(forged.as_bytes(), "ana@example.org", &["bo@example.se".into()], only_encrypt, &p.anas).unwrap();
        assert_eq!(open(&sealed, &p.bos).unwrap().1.signature, Signature::Bad);
    }

    /// Past `MAX_NESTING` layers a message isn't opened, at the packet level
    /// and at the MIME level.
    #[test]
    fn nesting_is_limited() {
        let p = people();
        let wrap = |layers: usize| {
            let mut packets = signed_and_encrypted(BODY, &p.ana, &p.bo);
            for _ in 1..layers {
                packets = encrypt_packets(&packets, &p.bo);
            }
            pgp_mime(&packets)
        };
        // The PGP/MIME part is one layer, each decryption another.
        assert!(open(&wrap(MAX_NESTING - 1), &p.bos).is_some());
        assert!(open(&wrap(MAX_NESTING), &p.bos).is_none());
        let only_encrypt = Protect { encrypt: true, ..Default::default() };
        let mut mime = MESSAGE.to_vec();
        // Each PGP/MIME layer counts twice: its part and its decryption.
        for _ in 0..MAX_NESTING / 2 {
            mime = protect(&mime, "ana@example.org", &["bo@example.se".into()], only_encrypt, &p.anas).unwrap();
        }
        assert_eq!(body_of(&open(&mime, &p.bos).unwrap().0), "Meet at noon.");
        mime = protect(&mime, "ana@example.org", &["bo@example.se".into()], only_encrypt, &p.anas).unwrap();
        assert!(open(&mime, &p.bos).is_none());
    }
}
