use super::*;

/// Made by the Pinal app's TypeScript (`@pinal/mail-crypto`): a key bundle for
/// a test-only password, a message encrypted the way Stalwart does, and one
/// encrypted to someone else's key.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    password: String,
    recovery_key: String,
    bundle: KeyBundle,
    original: String,
    stored: String,
    foreign: String,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../../tests/fixtures/pinal-encryption.json")).unwrap()
}
fn bytes(b64: &str) -> Vec<u8> {
    B64.decode(b64).unwrap()
}

#[test]
fn derivations_match_the_web_app_known_answers() {
    // The same vector is pinned in tests/server/mail-crypto.test.ts.
    let salt: Vec<u8> = (0..16).collect();
    let kdf = KdfParams {
        alg: "argon2id".into(),
        memory_kib: 65536,
        iterations: 3,
        parallelism: 1,
        salt: String::new(),
    };
    let stretched = argon2("correct horse battery staple ✓", &salt, &kdf).unwrap();
    assert_eq!(
        hex(stretched.as_ref()),
        "e19fcc47005273599bfd0e1f9743abb56e263eb165b6fceace742996e9b6993e"
    );
    let kek = hkdf(stretched.as_ref(), "pinal-mail/v1/password-kek");
    assert_eq!(
        hex(kek.as_ref()),
        "2277b4a8d720ef9922fe77b97e6b3abeecb0f958bbefd367f5654b5b3a4097cf"
    );
    let cipher = Aes256Gcm::new_from_slice(kek.as_ref()).unwrap();
    let mk: Vec<u8> = (0..32).map(|i| 255 - i as u8).collect();
    let aad = context(&["pinal-mail/v1/slot", "user-1", &"0".repeat(32), "password"]);
    let wrapped = cipher
        .encrypt(
            &Nonce::from([0; 12]),
            Payload {
                msg: &mk,
                aad: &aad,
            },
        )
        .unwrap();
    assert_eq!(
        hex(&wrapped),
        "813ea979f38099ff804dc6860b011bb3e162016f1215168e552cbe7ee395d595ac8dfdca828ab8b50234ee13741cf127"
    );
    // Passwords typed in either Unicode form derive the same key.
    assert_eq!(
        argon2("Café", &salt, &kdf).unwrap().as_ref(),
        argon2("Cafe\u{301}", &salt, &kdf).unwrap().as_ref()
    );
}

#[test]
fn opens_a_bundle_made_by_the_web_app_and_reads_stalwart_mail() {
    let f = fixture();
    let mk = unlock_with_password(&f.bundle, &f.password).unwrap();
    let by_recovery = unlock_with_recovery_key(&f.bundle, &f.recovery_key.to_lowercase()).unwrap();
    assert_eq!(mk.as_ref(), by_recovery.as_ref());
    let keys = open_keys(&f.bundle, &mk).unwrap();
    let (protection, plain) = open_message(&bytes(&f.stored), &keys).unwrap();
    assert_eq!(protection, Protection::Pinal);
    let parsed = mailparse::parse_mail(&plain).unwrap();
    let original_bytes = bytes(&f.original);
    let original = mailparse::parse_mail(&original_bytes).unwrap();
    assert_eq!(
        parsed.subparts[0].get_body().unwrap(),
        original.subparts[0].get_body().unwrap()
    );
    assert_eq!(parsed.subparts[1].get_body_raw().unwrap(), b"%PDF secret");
    use mailparse::MailHeaderMap;
    assert_eq!(
        parsed.headers.get_first_value("Subject").unwrap(),
        "Fixture"
    );
    // Nothing readable was in the stored copy.
    let stored = String::from_utf8_lossy(&bytes(&f.stored)).into_owned();
    assert!(!stored.contains("Picacho reads this"));
    assert!(!stored.contains(&B64.encode("%PDF secret")));
}

#[test]
fn wrong_secrets_and_moved_slots_do_not_open() {
    let f = fixture();
    assert!(
        unlock_with_password(&f.bundle, "not the password")
            .unwrap_err()
            .is::<WrongSecret>()
    );
    assert!(unlock_with_password(&f.bundle, "").is_err());
    let mut moved = f.bundle.clone();
    moved.user_id = "someone-else".into();
    assert!(unlock_with_password(&moved, &f.password).is_err());
    let mut renamed = f.bundle.clone();
    renamed.slots[0].id = "f".repeat(32);
    assert!(unlock_with_password(&renamed, &f.password).is_err());
    let mut typo = f.recovery_key.clone().into_bytes();
    let i = typo.iter().position(|c| *c != b'-').unwrap();
    typo[i] = if typo[i] == b'A' { b'B' } else { b'A' };
    assert!(parse_recovery_key(std::str::from_utf8(&typo).unwrap()).is_err());
    assert!(looks_like_recovery_key(&f.recovery_key));
    assert!(!looks_like_recovery_key(&f.password));
}

#[test]
fn a_server_cannot_weaken_the_password_settings() {
    let f = fixture();
    for change in [
        |k: &mut KdfParams| k.memory_kib = 1024,
        |k: &mut KdfParams| k.iterations = 1,
        |k: &mut KdfParams| k.memory_kib = 4 * 1024 * 1024,
        |k: &mut KdfParams| k.alg = "argon2i".into(),
    ] {
        let mut b = f.bundle.clone();
        change(b.slots[0].kdf.as_mut().unwrap());
        let error = unlock_with_password(&b, &f.password).unwrap_err();
        assert!(error.to_string().contains("not allowed"), "{error}");
    }
}

#[test]
fn private_keys_must_match_their_listed_public_keys() {
    let f = fixture();
    let mk = unlock_with_password(&f.bundle, &f.password).unwrap();
    let mut swapped = f.bundle.clone();
    // Another key's armored public key under this fingerprint.
    swapped.keys[0].public_key = include_str!("../../tests/fixtures/other-public-key.asc").into();
    assert!(open_keys(&swapped, &mk).is_err());
    let mut tampered = f.bundle.clone();
    let mut sealed = bytes(&tampered.keys[0].sealed_private_key);
    sealed[10] ^= 1;
    tampered.keys[0].sealed_private_key = B64.encode(sealed);
    assert!(open_keys(&tampered, &mk).is_err());
}

#[test]
fn altered_foreign_and_disguised_mail_is_never_called_protected() {
    let f = fixture();
    let mk = unlock_with_password(&f.bundle, &f.password).unwrap();
    let keys = open_keys(&f.bundle, &mk).unwrap();
    assert_eq!(
        open_message(&bytes(&f.foreign), &keys).unwrap().0,
        Protection::Foreign
    );
    // Flip one character deep in the encrypted packet.
    let text = String::from_utf8(bytes(&f.stored)).unwrap();
    let mut lines: Vec<String> = text.split("\r\n").map(str::to_owned).collect();
    let i = lines
        .iter()
        .rposition(|l| {
            l.len() >= 60
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        })
        .unwrap()
        - 1;
    let c = if lines[i].as_bytes()[20] == b'A' {
        "B"
    } else {
        "A"
    };
    lines[i].replace_range(20..21, c);
    let altered = lines.join("\r\n");
    assert_eq!(
        open_message(altered.as_bytes(), &keys).unwrap().0,
        Protection::Foreign
    );
    let disguised = b"From: a@example.org\r\nContent-Type: multipart/encrypted; boundary=x\r\n\r\n--x\r\n\r\nplain words\r\n--x--\r\n";
    assert_eq!(
        open_message(disguised, &keys).unwrap().0,
        Protection::Foreign
    );
    assert_eq!(
        open_message(&bytes(&f.original), &keys).unwrap().0,
        Protection::None
    );
    assert!(open_message(&bytes(&f.stored), &[]).unwrap().0 == Protection::Foreign);
}

#[test]
fn master_keys_round_trip_through_the_keychain_encoding() {
    let mk = [7u8; 32];
    assert_eq!(
        decode_master_key(&encode_master_key(&mk)).unwrap().as_ref(),
        &mk
    );
    assert!(decode_master_key("short").is_none());
}
