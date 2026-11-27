//! Pinal Mail zero-access encryption on this device.
//!
//! Stalwart encrypts each Pinal mailbox to its owner's OpenPGP key (RFC 3156
//! PGP/MIME, keeping the non-MIME headers outside). The private key reaches
//! this device only inside the owner's key bundle, wrapped with keys derived
//! from their Pinal password or recovery key. This module mirrors
//! `@pinal/mail-crypto` in the Pinal app byte for byte; the known-answer tests
//! below pin the two implementations to each other. See Pinal's
//! `docs/mail-encryption.md` for the design.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use pgp::composed::{Deserializable, Message, SignedPublicKey, SignedSecretKey};
use pgp::types::{KeyDetails, Password};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct KdfParams {
    pub alg: String,
    #[serde(rename = "memoryKiB")]
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<KdfParams>,
    pub nonce: String,
    pub wrapped: String,
    pub created_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MailKey {
    pub fingerprint: String,
    pub public_key: String,
    pub nonce: String,
    pub sealed_private_key: String,
    pub created_at: String,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyBundle {
    pub version: u8,
    pub user_id: String,
    pub slots: Vec<KeySlot>,
    pub keys: Vec<MailKey>,
}

/// Mail is encrypted and this device has not been unlocked with the password.
#[derive(Debug)]
pub struct Locked;
impl std::fmt::Display for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("manage-crypto-this-mail-is-encrypted-enter-your"))
    }
}
impl std::error::Error for Locked {}

/// The password or recovery key does not open the bundle.
#[derive(Debug)]
pub struct WrongSecret;
impl std::fmt::Display for WrongSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            t!("manage-crypto-that-does-not-unlock-your-mail"),
        )
    }
}
impl std::error::Error for WrongSecret {}

// Floor: RFC 9106's second recommendation; ceiling: a server cannot make this
// device hang. The same limits as the web app.
const FLOOR: (u32, u32, u32) = (64 * 1024, 3, 1);
const CEILING: (u32, u32, u32) = (1024 * 1024, 16, 8);

fn context(parts: &[&str]) -> Vec<u8> {
    parts.join("\0").into_bytes()
}

pub fn argon2(password: &str, salt: &[u8], kdf: &KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(
        kdf.alg == "argon2id"
            && (FLOOR.0..=CEILING.0).contains(&kdf.memory_kib)
            && (FLOOR.1..=CEILING.1).contains(&kdf.iterations)
            && (FLOOR.2..=CEILING.2).contains(&kdf.parallelism),
        t!("manage-crypto-these-key-settings-are-not-allowed")
    );
    let params = argon2::Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let hasher = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let normalized = Zeroizing::new(password.nfc().collect::<String>());
    let mut out = Zeroizing::new([0u8; 32]);
    hasher
        .hash_password_into(normalized.as_bytes(), salt, out.as_mut())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(out)
}

/// HKDF-SHA256 with an empty salt and a labelled info, as in the web app.
pub fn hkdf(ikm: &[u8], info: &str) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(None, ikm)
        .expand(info.as_bytes(), out.as_mut())
        .expect("32 bytes is a valid HKDF length");
    out
}

fn open(key: &[u8; 32], nonce: &str, ciphertext: &str, aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let nonce = B64.decode(nonce).map_err(|_| WrongSecret)?;
    let ciphertext = B64.decode(ciphertext).map_err(|_| WrongSecret)?;
    let nonce = Nonce::try_from(nonce.as_slice()).map_err(|_| WrongSecret)?;
    ensure!(ciphertext.len() >= 16, WrongSecret);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    Ok(Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
            .map_err(|_| WrongSecret)?,
    ))
}

fn master_key(bytes: Zeroizing<Vec<u8>>) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(bytes.len() == 32, WrongSecret);
    let mut mk = Zeroizing::new([0u8; 32]);
    mk.copy_from_slice(&bytes);
    Ok(mk)
}

fn slot<'a>(bundle: &'a KeyBundle, kind: &str) -> Result<&'a KeySlot> {
    bundle
        .slots
        .iter()
        .find(|s| s.kind == kind)
        .ok_or_else(|| WrongSecret.into())
}

pub fn unlock_with_password(bundle: &KeyBundle, password: &str) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(!password.is_empty(), WrongSecret);
    let slot = slot(bundle, "password")?;
    let kdf = slot
        .kdf
        .as_ref()
        .with_context(|| t!("manage-crypto-the-password-slot-has-no-key"))?;
    let salt = B64.decode(&kdf.salt).map_err(|_| WrongSecret)?;
    ensure!(salt.len() == 16, t!("manage-crypto-bad-salt"));
    let stretched = argon2(password, &salt, kdf)?;
    let kek = hkdf(stretched.as_ref(), "pinal-mail/v1/password-kek");
    let aad = context(&["pinal-mail/v1/slot", &bundle.user_id, &slot.id, "password"]);
    master_key(open(&kek, &slot.nonce, &slot.wrapped, &aad)?)
}

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 56 Crockford base32 characters: 32 random bytes and a 3-byte checksum.
pub fn parse_recovery_key(text: &str) -> Result<Zeroizing<[u8; 32]>> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    ensure!(
        clean.len() == 56,
        t!("manage-crypto-check-the-recovery-key-it-has")
    );
    let mut bytes = Zeroizing::new(Vec::with_capacity(35));
    let (mut bits, mut value) = (0u32, 0u32);
    for c in clean.bytes() {
        let digit = ALPHABET
            .iter()
            .position(|a| *a == c)
            .with_context(|| t!("manage-crypto-check-the-recovery-key-it-has"))?;
        value = (value << 5) | digit as u32;
        bits += 5;
        if bits >= 8 {
            bytes.push((value >> (bits - 8)) as u8);
            bits -= 8;
            value &= (1 << bits) - 1;
        }
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&bytes[..32]);
    let sum = Sha256::new()
        .chain_update(b"pinal-mail/v1/recovery-checksum")
        .chain_update(&key[..])
        .finalize();
    ensure!(
        sum[..3] == bytes[32..35],
        t!("manage-crypto-check-the-recovery-key-it-has")
    );
    Ok(key)
}

pub fn looks_like_recovery_key(text: &str) -> bool {
    let n = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .count();
    n == 56
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c == '-')
}

pub fn unlock_with_recovery_key(bundle: &KeyBundle, text: &str) -> Result<Zeroizing<[u8; 32]>> {
    let slot = slot(bundle, "recovery")?;
    let key = parse_recovery_key(text)?;
    let kek = hkdf(key.as_ref(), "pinal-mail/v1/recovery-kek");
    let aad = context(&["pinal-mail/v1/slot", &bundle.user_id, &slot.id, "recovery"]);
    master_key(open(&kek, &slot.nonce, &slot.wrapped, &aad)?)
}

/// Decrypts every private key and checks it against the public key the
/// bundle lists, so a server cannot pair a private key with another public key.
pub fn open_keys(bundle: &KeyBundle, mk: &[u8; 32]) -> Result<Vec<SignedSecretKey>> {
    let k = hkdf(mk, "pinal-mail/v1/private-key");
    let mut keys = Vec::new();
    for record in &bundle.keys {
        let aad = context(&[
            "pinal-mail/v1/private-key",
            &bundle.user_id,
            &record.fingerprint,
        ]);
        let binary = open(&k, &record.nonce, &record.sealed_private_key, &aad)?;
        let key = SignedSecretKey::from_bytes(Cursor::new(binary.as_slice()))
            .with_context(|| t!("manage-crypto-a-mail-key-could-not-be"))?;
        let (listed, _) = SignedPublicKey::from_string(&record.public_key)
            .with_context(|| t!("manage-crypto-a-mail-keys-public-half-could"))?;
        let hex = |f: pgp::types::Fingerprint| hex(f.as_bytes());
        let subkeys = |k: &[String]| {
            let mut v = k.to_vec();
            v.sort();
            v
        };
        let secret_subs: Vec<String> = key
            .secret_subkeys
            .iter()
            .map(|s| hex(s.fingerprint()))
            .collect();
        let public_subs: Vec<String> = listed
            .public_subkeys
            .iter()
            .map(|s| hex(s.fingerprint()))
            .collect();
        ensure!(
            hex(key.fingerprint()) == record.fingerprint
                && hex(listed.fingerprint()) == record.fingerprint
                && subkeys(&secret_subs) == subkeys(&public_subs),
            t!("manage-crypto-a-mail-key-does-not-match")
        );
        keys.push(key);
    }
    Ok(keys)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// Encrypted at rest to the owner's key; returned decrypted.
    Pinal,
    /// Encrypted, but not openable here (sender-encrypted, another key, or damaged).
    Foreign,
    /// Stored readable.
    None,
}

struct Header {
    name: String,
    value: String,
    raw: String,
}
// Bytes as Latin-1 characters, so slicing never breaks and bytes round-trip.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}
fn from_latin1(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u32 as u8).collect()
}
fn split_message(text: &str) -> (&str, &str) {
    let crlf = text.find("\r\n\r\n");
    let lf = text.find("\n\n");
    match (crlf, lf) {
        (Some(c), l) if l.is_none_or(|l| c <= l) => (&text[..c + 2], &text[c + 4..]),
        (_, Some(l)) => (&text[..l + 1], &text[l + 2..]),
        _ => (text, ""),
    }
}
fn parse_headers(head: &str) -> Vec<Header> {
    let mut out: Vec<Header> = Vec::new();
    for line in head.split_inclusive('\n') {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.raw.push_str(line);
                last.value.push(' ');
                last.value.push_str(line.trim());
            }
        } else if let Some(colon) = line.find(':').filter(|c| *c > 0) {
            out.push(Header {
                name: line[..colon].trim().to_ascii_lowercase(),
                value: line[colon + 1..].trim().to_owned(),
                raw: line.to_owned(),
            });
        }
    }
    out
}
fn content_type(headers: &[Header]) -> (String, Vec<(String, String)>) {
    let value = headers
        .iter()
        .find(|h| h.name == "content-type")
        .map(|h| h.value.as_str())
        .unwrap_or("");
    let mut parts = value.splitn(2, ';');
    let kind = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut params = Vec::new();
    let rest = parts.next().unwrap_or("");
    let mut chars = rest.char_indices().peekable();
    // name=value or name="quoted value", separated by semicolons.
    while let Some((start, c)) = chars.next() {
        if c == ';' || c.is_whitespace() {
            continue;
        }
        let mut name_end = start;
        let mut found = false;
        for (i, c) in chars.by_ref() {
            if c == '=' {
                name_end = i;
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
        let name = rest[start..name_end].trim().to_ascii_lowercase();
        while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek().is_some_and(|(_, c)| *c == '"') {
            chars.next();
            while let Some((_, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            value.push(n)
                        }
                    }
                    '"' => break,
                    c => value.push(c),
                }
            }
        } else {
            while let Some((_, c)) = chars.peek() {
                if *c == ';' || c.is_whitespace() {
                    break;
                }
                value.push(*c);
                chars.next();
            }
        }
        params.push((name, value));
    }
    (kind, params)
}
fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The armored payload of a PGP/MIME message, and the headers kept outside it.
pub fn encrypted_part(raw: &[u8]) -> Option<(String, String)> {
    // Its type is read from the headers alone first: nearly all mail isn't
    // PGP/MIME, and turning a whole source (attachments and all) into text
    // only to find that cost a copy of it, on every message read.
    let end = memchr::memmem::find(raw, b"\n\n")
        .into_iter()
        .chain(memchr::memmem::find(raw, b"\r\n\r\n"))
        .min()
        .map_or(raw.len(), |at| (at + 4).min(raw.len()));
    let (kind, _) = content_type(&parse_headers(split_message(&latin1(&raw[..end])).0));
    if kind != "multipart/encrypted" {
        return None;
    }
    let text = latin1(raw);
    let (head, body) = split_message(&text);
    let headers = parse_headers(head);
    let (kind, params) = content_type(&headers);
    let boundary = param(&params, "boundary")?;
    if kind != "multipart/encrypted"
        || !param(&params, "protocol")
            .is_some_and(|p| p.eq_ignore_ascii_case("application/pgp-encrypted"))
        || boundary.is_empty()
    {
        return None;
    }
    let delimiter = format!("--{boundary}");
    let closing = format!("{delimiter}--");
    let mut parts: Vec<Vec<&str>> = Vec::new();
    for line in body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if line.trim_end() == delimiter {
            parts.push(Vec::new());
        } else if line.trim_end() == closing {
            break;
        } else if let Some(part) = parts.last_mut() {
            part.push(line);
        }
    }
    if parts.len() != 2 {
        return None;
    }
    let mut decoded = parts.iter().map(|p| {
        let joined = p.join("\n");
        let (head, body) = split_message(&joined);
        (content_type(&parse_headers(head)).0, body.to_owned())
    });
    let (control, version) = decoded.next()?;
    let (payload, armored) = decoded.next()?;
    if control != "application/pgp-encrypted" || payload != "application/octet-stream" {
        return None;
    }
    // RFC 3156 §4: the control part says `Version: 1`.
    if !version
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("version: 1") || l.trim().eq_ignore_ascii_case("version:1"))
    {
        return None;
    }
    let armored = armored.trim().to_owned();
    if !armored.starts_with("-----BEGIN PGP MESSAGE-----") {
        return None;
    }
    let outer = headers
        .iter()
        // All Content-* headers stay with the encrypted part: the outer
        // Content-Transfer-Encoding came first in the decrypted message and
        // was used to decode its inner part's body.
        .filter(|h| !h.name.starts_with("content-"))
        .map(|h| {
            if h.raw.ends_with('\n') {
                h.raw.clone()
            } else {
                format!("{}\r\n", h.raw)
            }
        })
        .collect::<String>();
    Some((armored, outer))
}

/// Whether `raw` is encrypted in any form Picacho knows (PGP/MIME, S/MIME,
/// inline PGP): its text is ciphertext, read only when opened.
pub fn is_encrypted(raw: &[u8]) -> bool {
    sender_encrypted(raw) || encrypted_part(raw).is_some() || inline_pgp_in_parts(raw)
}

/// Whether `raw` is inline PGP with files: a multipart message whose first
/// plain-text part is an armored encrypted message (its files `.pgp` beside
/// it). `sender_encrypted` reads the top-level body alone, which here is
/// the multipart's preamble: a reply to such mail went out unencrypted.
pub fn inline_pgp_in_parts(raw: &[u8]) -> bool {
    let raw = &raw[..raw.len().min(MAX_MESSAGE)];
    let Ok(parsed) = mailparse::parse_mail(raw) else {
        return false;
    };
    if !parsed.ctype.mimetype.to_ascii_lowercase().starts_with("multipart/") {
        return false;
    }
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(&parsed, &mut leaves);
    leaves
        .into_iter()
        .find(|p| p.ctype.mimetype.eq_ignore_ascii_case("text/plain"))
        .and_then(|p| p.get_body().ok())
        .is_some_and(|body| body.trim_start().starts_with("-----BEGIN PGP MESSAGE-----"))
}

/// Mirrors Stalwart's test for mail its sender already encrypted.
pub fn sender_encrypted(raw: &[u8]) -> bool {
    let text = latin1(&raw[..raw.len().min(256 * 1024)]);
    let (head, body) = split_message(&text);
    let (kind, params) = content_type(&parse_headers(head));
    let p7 = param(&params, "name").is_some_and(|n| {
        let n = n.to_ascii_lowercase();
        [".p7m", ".p7s", ".p7c", ".p7z"]
            .iter()
            .any(|e| n.ends_with(e))
    });
    matches!(
        kind.as_str(),
        "multipart/encrypted"
            | "application/pkcs7-mime"
            | "application/x-pkcs7-mime"
            | "application/pkcs7-signature"
    ) || (kind == "application/octet-stream" && p7)
        || body.trim_start().starts_with("-----BEGIN PGP MESSAGE-----")
}

pub const MAX_MESSAGE: usize = 40 * 1024 * 1024;

/// Decrypts a stored message. Integrity is enforced: rPGP rejects data
/// without a modification detection code, and compressed data (which
/// Stalwart never writes) is refused rather than inflated.
pub fn open_message(raw: &[u8], keys: &[SignedSecretKey]) -> Result<(Protection, Vec<u8>)> {
    let Some((armored, outer)) = encrypted_part(raw) else {
        // Nothing to decrypt: as large as a download may be, which the
        // cache read it back at (a limit of its own here refused mail
        // that downloaded and saved).
        let p = if sender_encrypted(raw) {
            Protection::Foreign
        } else {
            Protection::None
        };
        return Ok((p, raw.to_vec()));
    };
    ensure!(
        raw.len() <= MAX_MESSAGE,
        t!("manage-crypto-this-message-is-too-large-to")
    );
    let decrypt = || -> Result<Vec<u8>> {
        let (message, _) = Message::from_string(&armored)?;
        let passwords: Vec<Password> = keys.iter().map(|_| Password::empty()).collect();
        let mut opened =
            message.decrypt_with_keys(passwords.iter().collect(), keys.iter().collect())?;
        if opened.is_compressed() {
            bail!("unexpected compressed data");
        }
        Ok(opened.as_data_vec()?)
    };
    match decrypt() {
        Ok(inner) => {
            let mut full = from_latin1(&outer);
            full.extend_from_slice(&inner);
            Ok((Protection::Pinal, full))
        }
        Err(_) => Ok((Protection::Foreign, raw.to_vec())),
    }
}

/// The mail key, kept in the keychain between launches. It can always be
/// rebuilt from the password, so nothing is lost if the keychain is cleared.
pub fn encode_master_key(mk: &[u8; 32]) -> String {
    B64.encode(mk)
}
pub fn decode_master_key(text: &str) -> Option<Zeroizing<[u8; 32]>> {
    let mut bytes = B64.decode(text).ok()?;
    if bytes.len() != 32 {
        bytes.zeroize();
        return None;
    }
    let mut mk = Zeroizing::new([0u8; 32]);
    mk.copy_from_slice(&bytes);
    bytes.zeroize();
    Some(mk)
}

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
