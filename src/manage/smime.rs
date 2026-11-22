//! S/MIME (RFC 8551): your certificate and key, imported from a PKCS#12
//! file (`.p12`/`.pfx`) as certificate authorities and companies issue
//! them, and the certificates of people who sent you signed mail; mail
//! signed or encrypted with them read; and mail signed and encrypted when
//! sent. Kept in the system keychain, as the OpenPGP keyring is.
//!
//! Signatures are checked against the system's trusted certificate
//! authorities; one that checks out, but by a certificate they don't vouch
//! for, is shown as such rather than as good.
use crate::manage::store;
use anyhow::{Context, Result, bail, ensure};
use openssl::pkcs7::{Pkcs7, Pkcs7Flags};
use openssl::pkey::{PKey, Private};
use openssl::stack::Stack;
use openssl::symm::Cipher;
use openssl::x509::{X509, store::X509StoreBuilder};
use serde::{Deserialize, Serialize};

#[cfg_attr(test, allow(dead_code))]
const ENTRY: &str = "smime:certificates";

/// Your identities and others' certificates, as PEM.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Certificates {
    /// Your own: (certificate chain, private key), each PEM.
    #[serde(default)]
    pub own: Vec<(String, String)>,
    /// Other people's certificates, from signed mail they sent or imported.
    #[serde(default)]
    pub others: Vec<String>,
}

/// One certificate, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    pub fingerprint: String,
    pub emails: Vec<String>,
    pub subject: String,
    pub issuer: String,
    /// Unix seconds.
    pub expires: i64,
    pub own: bool,
}

/// What S/MIME found when a message was read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub decrypted: bool,
    pub signature: Signature,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Signature {
    #[default]
    None,
    /// Signed by this address, with a certificate a trusted authority issued.
    Good(String),
    /// The signature matches, but no trusted authority vouches for the certificate.
    Untrusted(String),
    /// The signature doesn't match: the message was changed, or forged.
    Bad,
}

impl Signature {
    pub fn label(&self) -> String {
        match self {
            Self::None => "Not signed".into(),
            Self::Good(who) => format!("Signed by {who} (S/MIME)"),
            Self::Untrusted(who) => {
                format!(
                    "Signed by {who} (S/MIME), with a certificate no trusted authority vouches for"
                )
            }
            Self::Bad => {
                "The S/MIME signature doesn't match: the message may have been changed".into()
            }
        }
    }
    pub fn good(&self) -> bool {
        matches!(self, Self::Good(_))
    }
}

/// A certificate's email addresses: its subject's emailAddress and its
/// subjectAltName rfc822Name entries.
fn emails(cert: &X509) -> Vec<String> {
    let mut out: Vec<String> = cert
        .subject_alt_names()
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.email().map(str::to_lowercase))
                .collect()
        })
        .unwrap_or_default();
    for entry in cert
        .subject_name()
        .entries_by_nid(openssl::nid::Nid::PKCS9_EMAILADDRESS)
    {
        if let Ok(e) = entry.data().to_string() {
            let e = e.to_lowercase();
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

fn name(n: &openssl::x509::X509NameRef) -> String {
    n.entries_by_nid(openssl::nid::Nid::COMMONNAME)
        .next()
        .and_then(|e| e.data().to_string().ok())
        .unwrap_or_default()
}

fn fingerprint(cert: &X509) -> String {
    cert.digest(openssl::hash::MessageDigest::sha256())
        .map(|d| d.iter().map(|b| format!("{b:02X}")).collect())
        .unwrap_or_default()
}

fn expires(cert: &X509) -> i64 {
    let epoch = openssl::asn1::Asn1Time::from_unix(0).ok();
    epoch
        .and_then(|e| e.diff(cert.not_after()).ok())
        .map(|d| d.days as i64 * 86_400 + d.secs as i64)
        .unwrap_or(0)
}

fn expired(cert: &X509) -> bool {
    openssl::asn1::Asn1Time::days_from_now(0)
        .ok()
        .is_some_and(|now| cert.not_after() < now)
}

fn info(cert: &X509, own: bool) -> CertInfo {
    CertInfo {
        fingerprint: fingerprint(cert),
        emails: emails(cert),
        subject: name(cert.subject_name()),
        issuer: name(cert.issuer_name()),
        expires: expires(cert),
        own,
    }
}

impl Certificates {
    #[cfg_attr(test, allow(dead_code))]
    pub fn load() -> Result<Self> {
        Ok(match store::read_secret_text(ENTRY)? {
            Some(json) => {
                serde_json::from_str(&json).context("The S/MIME certificates couldn't be read.")?
            }
            None => Self::default(),
        })
    }
    #[cfg_attr(test, allow(dead_code))]
    pub fn save(&self) -> Result<()> {
        store::write_secret_text(ENTRY, &serde_json::to_string(self)?)
    }
    /// Your identities: certificate (first of the chain), the chain's
    /// others, and key.
    fn identities(&self) -> Vec<(X509, Vec<X509>, PKey<Private>)> {
        self.own
            .iter()
            .filter_map(|(chain, key)| {
                let mut certs = X509::stack_from_pem(chain.as_bytes()).ok()?.into_iter();
                let cert = certs.next()?;
                let key = PKey::private_key_from_pem(key.as_bytes()).ok()?;
                Some((cert, certs.collect(), key))
            })
            .collect()
    }
    fn others(&self) -> Vec<X509> {
        self.others
            .iter()
            .filter_map(|pem| X509::from_pem(pem.as_bytes()).ok())
            .collect()
    }
    pub fn list(&self) -> Vec<CertInfo> {
        let mut out: Vec<CertInfo> = self
            .identities()
            .iter()
            .map(|(c, _, _)| info(c, true))
            .collect();
        out.extend(self.others().iter().map(|c| info(c, false)));
        out
    }
    /// Imports a PKCS#12 file (your certificate and key) with its password.
    pub fn import_identity(&mut self, der: &[u8], password: &str) -> Result<CertInfo> {
        let parsed = openssl::pkcs12::Pkcs12::from_der(der)
            .context("That isn't a PKCS#12 file (.p12 or .pfx).")?
            .parse2(password)
            .map_err(|_| anyhow::anyhow!("The password is wrong, or the file is damaged."))?;
        let cert = parsed.cert.context("The file has no certificate.")?;
        let key = parsed.pkey.context("The file has no private key.")?;
        ensure!(
            cert.public_key()?.public_eq(&key),
            "The file's key doesn't belong to its certificate."
        );
        ensure!(
            !emails(&cert).is_empty(),
            "The certificate names no email address."
        );
        let mut chain = String::from_utf8(cert.to_pem()?)?;
        for extra in parsed.ca.into_iter().flatten() {
            chain.push_str(&String::from_utf8(extra.to_pem()?)?);
        }
        let fp = fingerprint(&cert);
        self.own
            .retain(|(c, _)| X509::from_pem(c.as_bytes()).map_or(true, |c| fingerprint(&c) != fp));
        self.own
            .push((chain, String::from_utf8(key.private_key_to_pem_pkcs8()?)?));
        Ok(info(&cert, true))
    }
    /// Imports someone's certificate (PEM or DER).
    pub fn import_certificate(&mut self, data: &[u8]) -> Result<CertInfo> {
        let cert = X509::from_pem(data)
            .or_else(|_| X509::from_der(data))
            .context("That isn't a certificate (.pem, .crt or .cer).")?;
        ensure!(
            !emails(&cert).is_empty(),
            "The certificate names no email address."
        );
        let info = info(&cert, false);
        self.add_other(&cert)?;
        Ok(info)
    }
    fn add_other(&mut self, cert: &X509) -> Result<bool> {
        let fp = fingerprint(cert);
        if self.others().iter().any(|c| fingerprint(c) == fp) {
            return Ok(false);
        }
        // A newer certificate for the same addresses replaces the older one.
        let addresses = emails(cert);
        self.others.retain(|pem| {
            X509::from_pem(pem.as_bytes()).map_or(true, |c| {
                !(emails(&c) == addresses && c.not_after() < cert.not_after())
            })
        });
        self.others.push(String::from_utf8(cert.to_pem()?)?);
        Ok(true)
    }
    pub fn remove(&mut self, fp: &str) {
        self.own
            .retain(|(c, _)| X509::from_pem(c.as_bytes()).map_or(true, |c| fingerprint(&c) != fp));
        self.others
            .retain(|c| X509::from_pem(c.as_bytes()).map_or(true, |c| fingerprint(&c) != fp));
    }
    fn own_for(&self, email: &str) -> Option<(X509, Vec<X509>, PKey<Private>)> {
        let email = email.trim().to_lowercase();
        self.identities()
            .into_iter()
            .find(|(c, _, _)| emails(c).contains(&email) && !expired(c))
    }
    fn cert_for(&self, email: &str) -> Option<X509> {
        let email = email.trim().to_lowercase();
        self.identities()
            .into_iter()
            .map(|(c, _, _)| c)
            .chain(self.others())
            .filter(|c| emails(c).contains(&email) && !expired(c))
            .max_by(|a, b| {
                a.not_after()
                    .partial_cmp(b.not_after())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
    pub fn has_own(&self, email: &str) -> bool {
        self.own_for(email).is_some()
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_cert(&self, email: &str) -> bool {
        self.cert_for(email).is_some()
    }
}

/// The message's body part (Content-* headers and body) and the other headers.
fn split(raw: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let text: String = raw.iter().map(|&b| b as char).collect();
    let at = text.find("\r\n\r\n").map_or(text.len(), |i| i + 2);
    let (head, body) = text.split_at(at);
    let mut outer = String::new();
    let mut part = String::new();
    let mut into_part = false;
    for line in head.split("\r\n").filter(|l| !l.is_empty()) {
        if !line.starts_with([' ', '\t']) {
            let name = line.split(':').next().unwrap_or("").to_ascii_lowercase();
            into_part = name.starts_with("content-");
            if name == "mime-version" {
                continue;
            }
        }
        let target = if into_part { &mut part } else { &mut outer };
        target.push_str(line);
        target.push_str("\r\n");
    }
    part.push_str(body);
    let bytes = |s: &str| s.chars().map(|c| c as u8).collect::<Vec<u8>>();
    (bytes(&part), bytes(&outer))
}

/// `to_smime`'s output with CRLF line ends, without its MIME-Version line
/// (the message keeps its own).
fn as_part(smime: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8_lossy(&smime)
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    text.lines()
        .skip_while(|l| l.starts_with("MIME-Version"))
        .collect::<Vec<_>>()
        .join("\r\n")
        .into_bytes()
}

/// Signs and/or encrypts a finished message with S/MIME. Encrypted mail is
/// encrypted to each recipient's certificate and the sender's own.
pub fn protect_with(
    certs: &Certificates,
    raw: &[u8],
    from: &str,
    to: &[String],
    sign: bool,
    encrypt: bool,
) -> Result<Vec<u8>> {
    let (mut part, outer) = split(raw);
    while part.ends_with(b"\r\n") {
        part.truncate(part.len() - 2);
    }
    let own = certs.own_for(from);
    let mut content = part;
    if sign {
        let (cert, chain, key) = own.as_ref().with_context(|| {
            format!("There's no S/MIME certificate of yours for {from}. Import one in Settings → Encryption, or turn off Sign.")
        })?;
        let mut extra = Stack::new()?;
        for c in chain {
            extra.push(c.clone())?;
        }
        let flags = Pkcs7Flags::DETACHED | Pkcs7Flags::BINARY;
        let signed = Pkcs7::sign(cert, key, &extra, &content, flags)?;
        content = as_part(signed.to_smime(&content, flags)?);
    }
    if encrypt {
        let mut recipients = Stack::new()?;
        let mut missing = Vec::new();
        for address in to {
            match certs.cert_for(address) {
                Some(c) => recipients.push(c)?,
                None => missing.push(address.clone()),
            }
        }
        ensure!(
            missing.is_empty(),
            "There's no S/MIME certificate for {}. They need to send you signed mail first, or you can import theirs in Settings → Encryption.",
            missing.join(", ")
        );
        if let Some((cert, _, _)) = &own {
            recipients.push(cert.clone())?;
        }
        let flags = Pkcs7Flags::BINARY;
        let enveloped = Pkcs7::encrypt(&recipients, &content, Cipher::aes_256_cbc(), flags)?;
        content = as_part(enveloped.to_smime(&content, flags)?);
    }
    let mut out = outer;
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");
    out.extend_from_slice(&content);
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    Ok(out)
}

/// The type of the message's top part, lowercased, and its smime-type.
fn kind(raw: &[u8]) -> Option<(String, String)> {
    let parsed = mailparse::parse_headers(raw).ok()?.0;
    use mailparse::MailHeaderMap;
    let ct = mailparse::parse_content_type(&parsed.get_first_value("Content-Type")?);
    let smime_type = ct
        .params
        .get("smime-type")
        .cloned()
        .unwrap_or_default()
        .to_lowercase();
    let protocol = ct
        .params
        .get("protocol")
        .cloned()
        .unwrap_or_default()
        .to_lowercase();
    let mime = ct.mimetype.to_lowercase();
    Some((
        if mime == "multipart/signed" {
            format!("{mime};{protocol}")
        } else {
            mime
        },
        smime_type,
    ))
}

fn trusted_store() -> Result<openssl::x509::store::X509Store> {
    let mut builder = X509StoreBuilder::new()?;
    // The system's trusted authorities; on systems where OpenSSL has none
    // configured, nothing is trusted and signatures show as unvouched for.
    let _ = builder.set_default_paths();
    Ok(builder.build())
}

/// Opens an S/MIME message: decrypts it with your key and checks its
/// signature. Returns the readable message (with its outer headers), what
/// was found, and the signer's certificate (to keep, so you can write to
/// them encrypted). `None` when it has no S/MIME protection, or can't be
/// opened.
pub fn open(raw: &[u8], certs: &Certificates) -> Option<(Vec<u8>, Verdict, Option<X509>)> {
    let (outer_kind, _) = kind(raw)?;
    let is_enveloped = matches!(
        outer_kind.as_str(),
        "application/pkcs7-mime" | "application/x-pkcs7-mime"
    );
    let is_signed = outer_kind.starts_with("multipart/signed;application/")
        && outer_kind.contains("pkcs7-signature");
    if !is_enveloped && !is_signed {
        return None;
    }
    let (_, outer) = split(raw);
    let mut current = raw.to_vec();
    let mut verdict = Verdict::default();
    let mut signer = None;
    // Up to two layers: encrypted, then signed (or signed opaquely).
    for _ in 0..3 {
        let (k, smime_type) = kind(&current)?;
        if matches!(
            k.as_str(),
            "application/pkcs7-mime" | "application/x-pkcs7-mime"
        ) {
            let (p7, _) = Pkcs7::from_smime(&current).ok()?;
            if smime_type == "signed-data" {
                // Opaque signing: the content is inside the signature.
                let (out, verified, who) = verify(&p7, None)?;
                verdict.signature = verified;
                signer = who;
                current = out;
                continue;
            }
            let opened = certs
                .identities()
                .into_iter()
                .find_map(|(cert, _, key)| p7.decrypt(&key, &cert, Pkcs7Flags::BINARY).ok())?;
            verdict.decrypted = true;
            current = opened;
        } else if k.starts_with("multipart/signed;") {
            let (p7, content) = Pkcs7::from_smime(&current).ok()?;
            let content = content?;
            let (_, verified, who) = verify(&p7, Some(&content))?;
            verdict.signature = verified;
            signer = who;
            current = content;
        } else {
            break;
        }
    }
    let mut readable = outer;
    readable.extend_from_slice(b"MIME-Version: 1.0\r\n");
    let current = String::from_utf8_lossy(&current)
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    readable.extend_from_slice(current.as_bytes());
    Some((readable, verdict, signer))
}

/// Checks a signature: first against trusted authorities, then for the
/// signature alone. Returns the signed content, the verdict and the signer.
fn verify(p7: &Pkcs7, content: Option<&[u8]>) -> Option<(Vec<u8>, Signature, Option<X509>)> {
    let empty = Stack::new().ok()?;
    let signer = p7
        .signers(&empty, Pkcs7Flags::empty())
        .ok()
        .and_then(|s| s.iter().next().map(|c| c.to_owned()));
    let who = signer
        .as_ref()
        .and_then(|c| emails(c).into_iter().next())
        .unwrap_or_else(|| "an unknown sender".into());
    let store = trusted_store().ok()?;
    let mut out = Vec::new();
    if p7
        .verify(&empty, &store, content, Some(&mut out), Pkcs7Flags::BINARY)
        .is_ok()
    {
        return Some((out, Signature::Good(who), signer));
    }
    out.clear();
    let unchecked = Pkcs7Flags::BINARY | Pkcs7Flags::NOVERIFY;
    if p7
        .verify(&empty, &store, content, Some(&mut out), unchecked)
        .is_ok()
    {
        return Some((out, Signature::Untrusted(who), signer));
    }
    let content = content.map(<[u8]>::to_vec).unwrap_or_default();
    Some((content, Signature::Bad, signer))
}

/// Keeps the signer's certificate from a signed message, so you can write
/// back to them encrypted. Returns whether it was new.
///
/// Only a certificate for the address the message is from (`from`) is
/// kept: anyone can sign with a certificate naming someone else's address,
/// and mail encrypted to that person would then be readable only by them.
/// One no trusted authority vouches for is kept only while there is no
/// certificate yet for its addresses, so a stranger's can never replace
/// one already kept; one an authority vouches for replaces what was kept.
pub fn learn(certs: &mut Certificates, signer: &X509, verdict: &Signature, from: &str) -> Result<bool> {
    if matches!(verdict, Signature::Bad | Signature::None) {
        return Ok(false);
    }
    let addresses = emails(signer);
    if addresses.is_empty() {
        bail!("The signer's certificate names no address.");
    }
    let from = from.trim().to_lowercase();
    if from.is_empty() || !addresses.contains(&from) {
        return Ok(false);
    }
    match verdict {
        Signature::Good(_) => {
            let fp = fingerprint(signer);
            if certs.others().iter().any(|c| fingerprint(c) == fp) {
                return Ok(false);
            }
            // Vouched for: it takes the place of any kept for its addresses.
            certs.others.retain(|pem| {
                X509::from_pem(pem.as_bytes())
                    .map_or(true, |c| !emails(&c).iter().any(|e| addresses.contains(e)))
            });
            certs.others.push(String::from_utf8(signer.to_pem()?)?);
            Ok(true)
        }
        _ => {
            let known = certs
                .identities()
                .into_iter()
                .map(|(c, _, _)| c)
                .chain(certs.others())
                .any(|c| emails(&c).iter().any(|e| addresses.contains(e)));
            if known {
                return Ok(false);
            }
            certs.add_other(signer)
        }
    }
}

/// A self-signed certificate and key for `email`, as a PKCS#12 file. For tests.
#[cfg(test)]
pub fn test_identity(email: &str, password: &str) -> Vec<u8> {
    use openssl::{
        asn1::Asn1Time,
        bn::BigNum,
        hash::MessageDigest,
        rsa::Rsa,
        x509::{X509NameBuilder, extension::SubjectAlternativeName},
    };
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", email).unwrap();
    let name = name.build();
    let mut b = X509::builder().unwrap();
    b.set_version(2).unwrap();
    b.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    b.set_subject_name(&name).unwrap();
    b.set_issuer_name(&name).unwrap();
    b.set_pubkey(&key).unwrap();
    b.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    b.set_not_after(&Asn1Time::days_from_now(30).unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .email(email)
        .build(&b.x509v3_context(None, None))
        .unwrap();
    b.append_extension(san).unwrap();
    b.sign(&key, MessageDigest::sha256()).unwrap();
    let cert = b.build();
    openssl::pkcs12::Pkcs12::builder()
        .name(email)
        .pkey(&key)
        .cert(&cert)
        .build2(password)
        .unwrap()
        .to_der()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGE: &[u8] = b"From: Ana <ana@example.org>\r\nTo: bo@example.se\r\nSubject: Plans\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.\r\n";

    fn people() -> (Certificates, Certificates) {
        let mut ana = Certificates::default();
        assert!(
            ana.import_identity(&test_identity("ana@example.org", "pw"), "wrong")
                .is_err()
        );
        ana.import_identity(&test_identity("ana@example.org", "pw"), "pw")
            .unwrap();
        let mut bo = Certificates::default();
        bo.import_identity(&test_identity("bo@example.se", ""), "")
            .unwrap();
        (ana, bo)
    }

    #[test]
    fn signed_mail_is_checked_and_its_certificate_kept() {
        let (ana, mut bo) = people();
        let signed = protect_with(&ana, MESSAGE, "ana@example.org", &[], true, false).unwrap();
        let text = String::from_utf8_lossy(&signed);
        assert!(text.contains("Subject: Plans") && text.contains("multipart/signed"));
        let (readable, verdict, signer) = open(&signed, &bo).unwrap();
        // Self-signed: the signature matches, but no authority vouches.
        assert_eq!(
            verdict.signature,
            Signature::Untrusted("ana@example.org".into())
        );
        assert!(!verdict.decrypted);
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        let signer = signer.unwrap();
        // Not for an address the message isn't from.
        assert!(!learn(&mut bo, &signer, &verdict.signature, "eve@example.net").unwrap());
        assert!(learn(&mut bo, &signer, &verdict.signature, "ana@example.org").unwrap());
        assert!(bo.has_cert("ana@example.org"));
        // A stranger's self-signed certificate for the same address doesn't
        // take the place of the one kept.
        let mut eve = Certificates::default();
        eve.import_identity(&test_identity("ana@example.org", ""), "").unwrap();
        let spoofed = protect_with(&eve, MESSAGE, "ana@example.org", &[], true, false).unwrap();
        let (_, spoofed_verdict, spoofed_signer) = open(&spoofed, &bo).unwrap();
        assert!(!learn(&mut bo, &spoofed_signer.unwrap(), &spoofed_verdict.signature, "ana@example.org").unwrap());
        assert_eq!(bo.cert_for("ana@example.org").map(|c| fingerprint(&c)), Some(fingerprint(&signer)));
        let forged = text.replace("Meet at noon.", "Meet at nine.");
        assert_eq!(
            open(forged.as_bytes(), &bo).unwrap().1.signature,
            Signature::Bad
        );
        assert!(open(MESSAGE, &bo).is_none());
    }

    #[test]
    fn encrypted_mail_opens_for_its_recipients_only() {
        let (mut ana, bo) = people();
        assert!(
            protect_with(
                &ana,
                MESSAGE,
                "ana@example.org",
                &["bo@example.se".into()],
                false,
                true
            )
            .is_err()
        );
        let bo_cert = bo.identities()[0].0.to_pem().unwrap();
        ana.import_certificate(&bo_cert).unwrap();
        let sent = protect_with(
            &ana,
            MESSAGE,
            "ana@example.org",
            &["bo@example.se".into()],
            true,
            true,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&sent);
        assert!(
            text.contains("pkcs7-mime; smime-type=enveloped-data")
                && !text.contains("Meet at noon")
        );
        let (readable, verdict, _) = open(&sent, &bo).unwrap();
        assert!(verdict.decrypted);
        assert_eq!(
            verdict.signature,
            Signature::Untrusted("ana@example.org".into())
        );
        let parsed = mailparse::parse_mail(&readable).unwrap();
        assert_eq!(parsed.get_body().unwrap().trim(), "Meet at noon.");
        // The sender's copy opens for the sender.
        assert!(open(&sent, &ana).unwrap().1.decrypted);
        assert!(open(&sent, &Certificates::default()).is_none());
        assert_eq!(ana.list().len(), 2);
        let fp = ana.list()[1].fingerprint.clone();
        ana.remove(&fp);
        assert_eq!(ana.list().len(), 1);
    }
}
