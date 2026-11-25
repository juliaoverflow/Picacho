use super::super::{Keyring, Signature, open, test_key};
use crate::manage::model::{Account, Draft, Provider};
use crate::sync::smtp;
use pgp::composed::{ArmorOptions, Deserializable, DetachedSignature, SignedPublicKey};

/// Ana, who sends, with her key and Bo's public key; Bo with his key and
/// Ana's public key.
struct People {
    anas: Keyring,
    bos: Keyring,
    ana_public: SignedPublicKey,
}

fn people() -> People {
    let ana = test_key("Ana <ana@example.org>", "");
    let bo = test_key("Bo <bo@example.se>", "");
    let public = |k: &pgp::composed::SignedSecretKey| {
        SignedPublicKey::from(k.clone()).to_armored_string(ArmorOptions::default()).unwrap()
    };
    let mut anas = Keyring::default();
    anas.import(&ana.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
    anas.import(&public(&bo), "").unwrap();
    let mut bos = Keyring::default();
    bos.import(&bo.to_armored_string(ArmorOptions::default()).unwrap(), "").unwrap();
    bos.import(&public(&ana), "").unwrap();
    People { anas, bos, ana_public: SignedPublicKey::from(ana) }
}

/// A long line, which goes out format=flowed in several.
const TEXT: &str = "Meet at noon by the old mill on the river, where the path from the station meets the road. Bring the maps.\n\n- Ana, über alles\nFrom the hill.\n-- \nAna";

/// The message as Picacho sends it from Ana to Bo: built, then signed
/// and/or encrypted as inline PGP.
fn send(p: &People, d: Draft) -> anyhow::Result<Vec<u8>> {
    let mut a = Account::new(Provider::Imap);
    a.email = "ana@example.org".into();
    a.name = "Ana".into();
    let d = Draft { to: "bo@example.se".into(), subject: "Plans".into(), pgp_inline: true, ..d };
    let message = smtp::build(&a, &d, false)?;
    let certs = crate::manage::smime::Certificates::default();
    smtp::protected(&message, &d, &smtp::Keys { ring: &p.anas, certs: &certs })
}

fn files(files: &[(&str, &[u8])]) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
    let dir = std::env::temp_dir().join(format!("picacho-inline-pgp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let paths = files
        .iter()
        .map(|(name, bytes)| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        })
        .collect();
    (dir, paths)
}

fn text_of(readable: &[u8]) -> String {
    let parsed = mailparse::parse_mail(readable).unwrap();
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&parsed, &mut leaves);
    let text = leaves.iter().find(|l| l.ctype.mimetype == "text/plain").unwrap();
    text.get_body().unwrap().replace("\r\n", "\n")
}

/// Every file of a message: its name and bytes.
fn attached(raw: &[u8]) -> Vec<(String, Vec<u8>)> {
    let parsed = mailparse::parse_mail(raw).unwrap();
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&parsed, &mut leaves);
    leaves.into_iter().filter_map(|l| Some((super::file_name(l)?, l.get_body_raw().unwrap()))).collect()
}

#[test]
fn encrypted_inline_pgp_is_an_armored_text_body() {
    let p = people();
    let sent = send(&p, Draft { body: TEXT.into(), sign: true, encrypt: true, ..Draft::default() }).unwrap();
    let parsed = mailparse::parse_mail(&sent).unwrap();
    // One plain-text part, its text the armored message; no PGP/MIME.
    assert_eq!(parsed.ctype.mimetype, "text/plain");
    assert!(!parsed.ctype.params.contains_key("format"), "{:?}", parsed.ctype);
    let body = parsed.get_body().unwrap();
    assert!(body.trim_start().starts_with("-----BEGIN PGP MESSAGE-----"), "{body}");
    let raw = String::from_utf8_lossy(&sent);
    assert!(!raw.contains("multipart/encrypted") && !raw.contains("old mill"), "{raw}");
    // No protected headers: the subject stays as it was.
    assert!(raw.contains("Subject: Plans"));
    assert!(crate::manage::crypto::is_encrypted(&sent));

    // Bo reads it signed by Ana, the text as written (unflowed).
    let (readable, verdict) = open(&sent, &p.bos).unwrap();
    assert!(verdict.decrypted);
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
    assert_eq!(text_of(&readable).trim_end(), TEXT);
    // Ana's copy in Sent opens for her too; for nobody else.
    assert!(open(&sent, &p.anas).is_some());
    assert!(open(&sent, &Keyring::default()).is_none());
}

#[test]
fn signed_inline_pgp_is_cleartext_signed() {
    let p = people();
    let sent = send(&p, Draft { body: TEXT.into(), sign: true, ..Draft::default() }).unwrap();
    let parsed = mailparse::parse_mail(&sent).unwrap();
    assert_eq!(parsed.ctype.mimetype, "text/plain");
    let body = parsed.get_body().unwrap();
    assert!(body.starts_with("-----BEGIN PGP SIGNED MESSAGE-----"), "{body}");
    assert!(body.contains("über alles") && body.contains("-----BEGIN PGP SIGNATURE-----"), "{body}");
    assert!(!String::from_utf8_lossy(&sent).contains("multipart/signed"));

    let (readable, verdict) = open(&sent, &p.bos).unwrap();
    assert!(!verdict.decrypted);
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
    // Dash-escaped lines read as written; the signature line's space is
    // trailing whitespace, which a cleartext signature leaves out.
    assert_eq!(text_of(&readable).trim_end(), TEXT.replace("-- \n", "--\n"));

    // Changed on the way, the signature no longer matches.
    let forged = String::from_utf8_lossy(&sent).replace("noon", "dusk");
    let (_, verdict) = open(forged.as_bytes(), &p.bos).unwrap();
    assert!(!matches!(verdict.signature, Signature::Good(_)), "{verdict:?}");
}

#[test]
fn inline_pgp_attachments_are_encrypted_each_and_opened_when_read() {
    let p = people();
    let (dir, attachments) = files(&[("notes.txt", b"the secret plan"), ("map.png", b"\x89PNG-1")]);
    let d = Draft { body: TEXT.into(), sign: true, encrypt: true, attachments, ..Draft::default() };
    let sent = send(&p, d).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let raw = String::from_utf8_lossy(&sent);
    assert!(!raw.contains("old mill") && !raw.contains("the secret plan"), "{raw}");
    let names: Vec<String> = attached(&sent).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["notes.txt.pgp", "map.png.pgp"]);

    // Bo sees the text and the files as they were sent.
    let (readable, verdict) = open(&sent, &p.bos).unwrap();
    assert!(verdict.decrypted);
    assert_eq!(text_of(&readable).trim_end(), TEXT);
    let files = attached(&readable);
    assert_eq!(files, [
        ("notes.txt".to_owned(), b"the secret plan".to_vec()),
        ("map.png".to_owned(), b"\x89PNG-1".to_vec()),
    ]);
    let parsed = mailparse::parse_mail(&readable).unwrap();
    assert_eq!(parsed.subparts[2].ctype.mimetype, "image/png");
}

#[test]
fn signed_inline_pgp_attachments_carry_their_own_signatures() {
    let p = people();
    let (dir, attachments) = files(&[("notes.txt", b"the plan")]);
    let d = Draft { body: TEXT.into(), sign: true, attachments, ..Draft::default() };
    let sent = send(&p, d).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let files = attached(&sent);
    assert_eq!(files[0], ("notes.txt".to_owned(), b"the plan".to_vec()));
    assert_eq!(files[1].0, "notes.txt.sig");
    let signature = DetachedSignature::from_bytes(&files[1].1[..]).unwrap();
    let verifies = |data: &[u8]| p.ana_public.public_subkeys.iter().any(|s| signature.verify(&s.key, data).is_ok());
    assert!(verifies(b"the plan"));
    assert!(!verifies(b"the plan!"));

    // Read, the text is checked and the files are both still there.
    let (readable, verdict) = open(&sent, &p.bos).unwrap();
    assert_eq!(verdict.signature, Signature::Good("Ana <ana@example.org>".into()));
    let names: Vec<String> = attached(&readable).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["notes.txt", "notes.txt.sig"]);
}

#[test]
fn inline_pgp_refuses_formatted_mail_and_missing_keys() {
    let p = people();
    let formatted = Draft { body: "**Noon**".into(), formatted: true, encrypt: true, ..Draft::default() };
    let error = send(&p, formatted).unwrap_err().to_string();
    assert!(error.contains("plain text"), "{error}");
    let mut stranger = Draft { body: "Hi".into(), encrypt: true, ..Draft::default() };
    stranger.cc = "cy@example.net".into();
    let error = send(&p, stranger).unwrap_err().to_string();
    assert!(error.contains("cy@example.net"), "{error}");
}

