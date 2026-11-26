//! Inline PGP (RFC 4880 §7, the old way), for people whose apps read only
//! that: sending, where the text is cleartext-signed or replaced by an
//! armored encrypted message and each attachment is encrypted (`name.pgp`)
//! or given a detached signature (`name.sig`) on its own; and, when one is
//! read, its attachments kept and opened beside its text.
//!
//! Inline PGP protects no header fields, so the subject always stays
//! readable, and only plain text: a formatted message has HTML, which
//! nothing would sign or decrypt as part of the text.
use super::{
    Keyring, Protect, encrypt, nested, recipient_keys, signing_key, split_body,
};
use anyhow::{Context, Result, ensure};
use lettre::message::{Attachment, SinglePart, header::ContentType};
use mailparse::{DispositionType, ParsedMail};
use pgp::composed::{ArmorOptions, CleartextSignedMessage, DetachedSignature, Message, SignedSecretKey};
use pgp::crypto::hash::HashAlgorithm;
use pgp::types::Password;

/// Signs and/or encrypts a finished plain-text message as inline PGP:
/// `protect` with `how.inline`. Encrypted, the signature (with
/// `how.sign`) goes inside the encryption, as `gpg --sign --encrypt` does.
pub(super) fn protect(raw: &[u8], from: &str, to: &[String], how: Protect, ring: &Keyring) -> Result<Vec<u8>> {
    let (part, mut out) = split_body(raw);
    let parsed = mailparse::parse_mail(&part)?;
    let (text, attachments) = contents(&parsed)?;
    let own = ring.own_key(from);
    let signer = if how.sign {
        Some(own.as_ref().with_context(|| t!("manage-openpgp-theres-no-openpgp-key-of-yours", from = from))?)
    } else {
        None
    };
    let keys = if how.encrypt { recipient_keys(to, own.as_ref(), ring)? } else { Vec::new() };
    let armored = match signer {
        _ if how.encrypt => String::from_utf8(encrypt(text.into_bytes(), &keys, signer, true)?)?,
        Some(key) => CleartextSignedMessage::sign(
            rand08::thread_rng(),
            // RFC 9580 §7.2: trailing spaces aren't signed, and are left
            // out when checked; rPGP signs the text as given.
            &text.split('\n').map(|l| l.trim_end_matches([' ', '\t'])).collect::<Vec<_>>().join("\n"),
            &Box::new(signing_key(key)),
            &Password::empty(),
        )?
        .to_armored_string(ArmorOptions::default())?,
        None => unreachable!("neither signed nor encrypted"),
    };
    let armored = armored.replace("\r\n", "\n").replace('\n', "\r\n");
    // No format=flowed: reflowing would change what was signed.
    let text_part = SinglePart::builder()
        .header(ContentType::parse("text/plain; charset=utf-8").expect("a valid content type"))
        .body(armored);
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");
    if attachments.is_empty() {
        out.extend_from_slice(&text_part.formatted());
        return Ok(out);
    }
    let mut parts = vec![text_part.formatted()];
    let octets = ContentType::parse("application/octet-stream").expect("a valid content type");
    for attachment in attachments {
        let name = file_name(attachment).unwrap_or_else(|| "attachment".into());
        let bytes = attachment.get_body_raw()?;
        if how.encrypt {
            let sealed = encrypt(bytes, &keys, signer, false)?;
            parts.push(Attachment::new(format!("{name}.pgp")).body(sealed, octets.clone()).formatted());
        } else if let Some(key) = signer {
            use pgp::ser::Serialize as _;
            // The file as it was, and its signature beside it.
            parts.push(attachment.raw_bytes.to_vec());
            let signature = DetachedSignature::sign_binary_data(
                rand08::thread_rng(),
                &Box::new(signing_key(key)),
                &Password::empty(),
                HashAlgorithm::Sha256,
                &bytes[..],
            )?
            .to_bytes()?;
            let kind = ContentType::parse("application/pgp-signature").expect("a valid content type");
            parts.push(Attachment::new(format!("{name}.sig")).body(signature, kind).formatted());
        }
    }
    out.extend_from_slice(&mixed(parts));
    Ok(out)
}

/// A multipart/mixed entity of `parts` (each its header fields and body).
pub(super) fn mixed(parts: Vec<Vec<u8>>) -> Vec<u8> {
    let b = super::boundary();
    let mut out = format!("Content-Type: multipart/mixed; boundary=\"{b}\"\r\n\r\n").into_bytes();
    for part in parts {
        out.extend_from_slice(format!("--{b}\r\n").as_bytes());
        out.extend_from_slice(&part);
        if !part.ends_with(b"\r\n") {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(format!("--{b}--\r\n").as_bytes());
    out
}

/// A plain-text message's text (unflowed) and its attachments: the text
/// alone, or a multipart/mixed of the text then files. Anything with HTML
/// (a formatted message) is refused.
fn contents<'a, 'm>(part: &'a ParsedMail<'m>) -> Result<(String, Vec<&'a ParsedMail<'m>>)> {
    let plain_only = || t!("manage-openpgp-inline-pgp-sends-plain-text-only");
    let (text, attachments) = match only_text(part) {
        Some(text) => (text, Vec::new()),
        None => {
            ensure!(part.ctype.mimetype.eq_ignore_ascii_case("multipart/mixed"), plain_only());
            let (first, rest) = part.subparts.split_first().with_context(plain_only)?;
            let text = only_text(first).with_context(plain_only)?;
            ensure!(rest.iter().all(|p| p.subparts.is_empty()), plain_only());
            (text, rest.iter().collect())
        }
    };
    let mut body = text.get_body()?;
    let param = |name: &str, value: &str| text.ctype.params.get(name).is_some_and(|v| v.eq_ignore_ascii_case(value));
    if param("format", "flowed") {
        body = crate::sync::mime::unflow(&body, param("delsp", "yes"));
    }
    Ok((body.replace("\r\n", "\n"), attachments))
}

/// The text/plain part `part` is or holds alone (a multipart/alternative
/// of plain text only, as a message with attachments has).
fn only_text<'a, 'm>(part: &'a ParsedMail<'m>) -> Option<&'a ParsedMail<'m>> {
    let plain = |p: &ParsedMail| p.ctype.mimetype.eq_ignore_ascii_case("text/plain");
    if plain(part) {
        return Some(part);
    }
    let alternative = part.ctype.mimetype.eq_ignore_ascii_case("multipart/alternative");
    (alternative && !part.subparts.is_empty() && part.subparts.iter().all(plain)).then(|| &part.subparts[0])
}

fn file_name(part: &ParsedMail) -> Option<String> {
    part.get_content_disposition()
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .filter(|n| !n.trim().is_empty())
        .cloned()
}

// --- Reading -----------------------------------------------------------------

/// The files of an inline PGP message, as parts of the message shown,
/// and the names of those its protection doesn't cover.
pub(super) struct Files {
    pub parts: Vec<Vec<u8>>,
    pub uncovered: Vec<String>,
}

/// The attachments of an inline PGP message `parsed` whose text is
/// `text`, as parts of the message shown: each encrypted file (`.pgp`,
/// `.gpg`, or armored) decrypted with `secret` where it can be, named
/// without that extension; the rest as they came. Parts that aren't files
/// (a list's footer) are left out, as they were before.
///
/// Inline PGP protects each file on its own, if at all, so each is
/// checked: one that decrypted is covered, as is one with a detached
/// signature beside it (`name.sig`, or `name.asc`) that `signed` accepts
/// (given the file and the signature), and that signature itself. Any
/// other is listed in `uncovered`: it may have been added on the way.
pub(super) fn attachments(
    parsed: &ParsedMail,
    text: &ParsedMail,
    secret: &[SignedSecretKey],
    depth: usize,
    signed: &dyn Fn(&[u8], &[u8]) -> bool,
    sealed: &dyn Fn(&Message<'_>) -> bool,
) -> Files {
    let mut leaves = Vec::new();
    crate::sync::mime::collect_leaves(parsed, &mut leaves);
    let files: Vec<&ParsedMail> = leaves
        .into_iter()
        .filter(|leaf| !std::ptr::eq(*leaf, text))
        .filter(|leaf| {
            leaf.get_content_disposition().disposition == DispositionType::Attachment || file_name(leaf).is_some()
        })
        .collect();
    let mut covered = vec![false; files.len()];
    let mut parts = Vec::with_capacity(files.len());
    for (i, leaf) in files.iter().enumerate() {
        if let Some((part, covers)) = opened(leaf, secret, depth, sealed) {
            covered[i] = covers;
            parts.push(part);
            continue;
        }
        parts.push(leaf.raw_bytes.to_vec());
        let Some(name) = file_name(leaf) else { continue };
        let Ok(body) = leaf.get_body_raw() else { continue };
        for ext in [".sig", ".asc"] {
            let sig_name = format!("{name}{ext}");
            let found = files.iter().position(|f| file_name(f).is_some_and(|n| n.eq_ignore_ascii_case(&sig_name)));
            if let Some(j) = found
                && j != i
                && files[j].get_body_raw().is_ok_and(|sig| signed(&body, &sig))
            {
                covered[i] = true;
                covered[j] = true;
                break;
            }
        }
    }
    let uncovered = files
        .iter()
        .zip(&covered)
        .filter(|(_, c)| !**c)
        .map(|(f, _)| file_name(f).unwrap_or_else(|| t!("manage-openpgp-an-unnamed-file").into()))
        .collect();
    Files { parts, uncovered }
}

/// The detached signature in `bytes`, binary or armored.
pub(super) fn detached(bytes: &[u8]) -> Option<pgp::packet::Signature> {
    use pgp::composed::Deserializable as _;
    let signature = if bytes.trim_ascii_start().starts_with(b"-----BEGIN PGP SIGNATURE-----") {
        DetachedSignature::from_string(std::str::from_utf8(bytes).ok()?).ok()?.0
    } else {
        DetachedSignature::from_bytes(bytes).ok()?
    };
    Some(signature.signature)
}

/// An encrypted file as the part it decrypts to, if it does, and whether
/// the message's protection covers it (`sealed`, given the file's
/// decrypted message, read to its end: signed as the text was). Anyone can
/// encrypt a file to you: under a good signature, one the signer didn't
/// sign may have been added on the way.
fn opened(
    leaf: &ParsedMail,
    secret: &[SignedSecretKey],
    mut depth: usize,
    sealed: &dyn Fn(&Message<'_>) -> bool,
) -> Option<(Vec<u8>, bool)> {
    if secret.is_empty() {
        return None;
    }
    let name = file_name(leaf)?;
    let lower = name.to_ascii_lowercase();
    let stem = [".pgp", ".gpg", ".asc"]
        .iter()
        .find_map(|e| lower.ends_with(e).then(|| name[..name.len() - e.len()].to_owned()))
        .filter(|s| !s.is_empty())?;
    let bytes = leaf.get_body_raw().ok()?;
    let message = if bytes.trim_ascii_start().starts_with(b"-----BEGIN PGP MESSAGE-----") {
        Message::from_armor(&bytes[..]).ok()?.0
    } else if lower.ends_with(".asc") {
        // An armored key or signature, not an encrypted file.
        return None;
    } else {
        Message::from_bytes(&bytes[..]).ok()?
    };
    let mut message = nested::unwrap_message(message, secret, &mut depth)?;
    let inner = {
        use std::io::Read as _;
        let limit = crate::manage::store::MAX_SOURCE;
        let mut inner = Vec::new();
        (&mut message).take(limit + 1).read_to_end(&mut inner).ok()?;
        if inner.len() as u64 > limit {
            return None;
        }
        inner
    };
    let covers = sealed(&message);
    let kind = crate::sync::smtp::content_type(std::path::Path::new(&stem));
    Some((Attachment::new(stem).body(inner, kind).formatted(), covers))
}

#[cfg(test)]
mod tests;
