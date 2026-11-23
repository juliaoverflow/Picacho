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
mod der;
use anyhow::{Context, Result, bail, ensure};
use openssl::cms::{CMSOptions, CmsContentInfo};
use openssl::pkcs7::{Pkcs7, Pkcs7Flags};
use openssl::pkey::{PKey, Private};
use openssl::stack::Stack;
use openssl::symm::Cipher;
use openssl::x509::{X509, store::{X509StoreBuilder, X509StoreRef}};
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
    /// The message as a whole: see `overall` for how its signers add up.
    pub signature: Signature,
    /// Each signature's own result: outer layers first, and within one in
    /// the order the message lists them.
    pub signers: Vec<Signer>,
    /// A certs-only message's certificates (RFC 8551 §3.6), as PEM, for
    /// importing when asked. They vouch for nothing by being sent.
    pub certificates: Vec<String>,
    /// RFC 9788 §4.4.2: the protected From is for another address than the
    /// outer one, and no valid signer's certificate is for it. As for
    /// OpenPGP (see `openpgp::from_mismatch`); the outer From stays the one
    /// shown.
    pub from_mismatch: Option<crate::manage::openpgp::FromMismatch>,
}

impl Verdict {
    /// The message details' signature line and whether it is all good:
    /// the signature's own label, with RFC 9788 §4.4.2's From warning
    /// when there is one (as `openpgp::Verdict::signature_line`).
    pub fn signature_line(&self) -> Option<(String, bool)> {
        let signed = (self.signature != Signature::None).then(|| self.signature.label());
        let good = self.signature.good() && self.from_mismatch.is_none();
        match (signed, &self.from_mismatch) {
            (Some(s), Some(w)) => Some((format!("{s}. {}", w.label()), good)),
            (Some(s), None) => Some((s, good)),
            (None, Some(w)) => Some((w.label(), good)),
            (None, None) => None,
        }
    }
}

/// One SignerInfo's result (RFC 5652 §5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signer {
    /// The address in the signer's certificate, or "an unknown signer"
    /// when the message doesn't carry it.
    pub address: String,
    pub result: SignerResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerResult {
    /// The signature matches, by a certificate a trusted authority issued.
    Good,
    /// The signature matches; no trusted authority vouches for it.
    Untrusted,
    /// The message doesn't carry the signer's certificate, so the
    /// signature couldn't be checked.
    Unverified,
    /// The signature matches, but a supplied revocation list (RFC 5280
    /// §5.3.1) lists the certificate.
    Revoked,
    /// The signature doesn't match the content, or its certificate isn't
    /// one for signing mail.
    Bad,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Signature {
    #[default]
    None,
    /// Signed by this address, with a certificate a trusted authority issued.
    Good(String),
    /// The signature matches, but no trusted authority vouches for the certificate.
    Untrusted(String),
    /// The signature matches, but the certificate isn't for the address the
    /// message says it is from: someone else signed it.
    OtherSender(String),
    /// Signed, but the message doesn't carry the certificate of anyone who
    /// signed it, so no signature could be checked.
    Unverified,
    /// The signature matches, but a revocation list its authority issued
    /// (RFC 5280 §5) says the certificate was withdrawn.
    Revoked(String),
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
            Self::OtherSender(who) => {
                format!("Signed by {who} (S/MIME), who isn't the sender shown")
            }
            Self::Revoked(who) => {
                format!("Signed by {who} (S/MIME), whose certificate has been revoked")
            }
            Self::Unverified => {
                "Signed with S/MIME, by a certificate the message doesn't include: it couldn't be checked".into()
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

/// Whether `cert` may sign mail now: within its dates, and, where it lists
/// what its key is for, for signing (digitalSignature or nonRepudiation)
/// and for email (emailProtection). One that lists nothing may sign.
fn usable_for_mail(cert: &X509) -> bool {
    let Ok(now) = openssl::asn1::Asn1Time::days_from_now(0) else {
        return false;
    };
    if cert.not_before() > now || cert.not_after() < now {
        return false;
    }
    // X509v3 Key Usage and Extended Key Usage, as OpenSSL prints them.
    let Ok(text) = cert.to_text() else {
        return false;
    };
    let text = String::from_utf8_lossy(&text);
    let listed = |name: &str| {
        let mut lines = text.lines();
        lines.find(|l| l.trim_start().starts_with(name)).map(|_| lines.next().unwrap_or_default().to_owned())
    };
    let signs = listed("X509v3 Key Usage")
        .is_none_or(|v| v.contains("Digital Signature") || v.contains("Non Repudiation"));
    let mail = listed("X509v3 Extended Key Usage")
        .is_none_or(|v| v.contains("E-mail Protection") || v.contains("Any Extended Key Usage"));
    signs && mail
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
        let pkcs12 = openssl::pkcs12::Pkcs12::from_der(der)
            .context("That isn't a PKCS#12 file (.p12 or .pfx).")?;
        let parsed = match pkcs12.parse2(password) {
            Ok(parsed) => parsed,
            // Older files use RC2 or triple DES, which OpenSSL 3 keeps in
            // its legacy provider: loaded, and tried again.
            Err(_) if legacy_provider() => pkcs12
                .parse2(password)
                .map_err(|_| anyhow::anyhow!("The password is wrong, or the file is damaged."))?,
            Err(_) => bail!("The password is wrong, or the file is damaged."),
        };
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
    /// Every certificate kept, yours and others': a signature whose
    /// certificate the message doesn't carry is checked with these.
    fn known(&self) -> Vec<X509> {
        let mut out: Vec<X509> = self.identities().into_iter().map(|(c, _, _)| c).collect();
        out.extend(self.others());
        out
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
    // The blank line ending the headers, CRLF or (mail saved with bare
    // line feeds) LF: with only CRLF looked for, such mail was all header.
    let at = match (text.find("\r\n\r\n"), text.find("\n\n")) {
        (Some(c), l) if l.is_none_or(|l| c <= l) => c + 2,
        (_, Some(l)) => l + 1,
        _ => text.len(),
    };
    let (head, body) = text.split_at(at);
    let mut outer = String::new();
    let mut part = String::new();
    let mut into_part = false;
    for line in head
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.is_empty())
    {
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
    // One char per byte (Latin-1), so every byte comes back as it was: as
    // UTF-8, signed text in another charset was changed after signing, and
    // its signature then failed for everyone.
    let text: String = smime.iter().map(|&b| b as char).collect();
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    text.lines()
        .skip_while(|l| l.starts_with("MIME-Version"))
        .collect::<Vec<_>>()
        .join("\r\n")
        .chars()
        .map(|c| c as u8)
        .collect()
}

/// OIDs' content octets for the signing writer.
mod oids {
    /// RFC 5652 §5.1, §11.1, §11.2, §11.3; RFC 8551 §2.5.2.
    pub const DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
    pub const CONTENT_TYPE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03];
    pub const MESSAGE_DIGEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];
    pub const SIGNING_TIME: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x05];
    pub const SMIME_CAPABILITIES: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x0f];
    /// RFC 5754 §2.2: id-sha256.
    pub const SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    /// RFC 3370 §3.2: rsaEncryption, as today's PKCS#7 signatures name
    /// PKCS#1 v1.5.
    pub const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    /// RFC 5084 §3.2 and RFC 3565 §4: AES-GCM and AES-CBC.
    pub const AES256_GCM: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2e];
    pub const AES128_GCM: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x06];
    pub const AES256_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
    pub const AES128_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x02];
}

/// RFC 8551 §2.5.2: the ciphers we can read, most preferred first. AES-GCM
/// leads (RFC 8551 §2.7: MUST support AES-128 GCM and AES-256 GCM), as
/// decryption of AuthEnvelopedData is proven by
/// `aes_gcm_auth_enveloped_mail_decrypts_and_is_authenticated`. Each has no
/// parameters, which §2.5.2 then requires be omitted. RC2 and DES are
/// never listed, so no reply is encrypted with them.
fn smime_capabilities() -> Vec<u8> {
    let capabilities: Vec<Vec<u8>> = [oids::AES256_GCM, oids::AES128_GCM, oids::AES256_CBC, oids::AES128_CBC]
        .iter()
        .map(|cipher| der::encode(der::SEQUENCE, &[&der::oid(cipher)]))
        .collect();
    let refs: Vec<&[u8]> = capabilities.iter().map(Vec::as_slice).collect();
    der::encode(der::SEQUENCE, &refs)
}

/// RFC 5652 §5.3 Attribute: its type and one value.
fn attribute(kind: &[u8], value: &[u8]) -> Vec<u8> {
    der::encode(der::SEQUENCE, &[&der::oid(kind), &der::encode(der::SET, &[value])])
}

/// RFC 5652 §11.3: UTCTime through 2049, GeneralizedTime after (RFC 5280
/// §4.1.2.5's rule), in UTC to the second.
fn signing_time(now: chrono::DateTime<chrono::Utc>) -> Vec<u8> {
    use chrono::Datelike as _;
    if now.year() < 2050 {
        der::encode(der::UTC_TIME, &[now.format("%y%m%d%H%M%SZ").to_string().as_bytes()])
    } else {
        der::encode(der::GENERALIZED_TIME, &[now.format("%Y%m%d%H%M%SZ").to_string().as_bytes()])
    }
}

/// A detached SignedData (RFC 5652 §5) over `content`, by an RSA `key`:
/// SHA-256 and PKCS#1 v1.5, as `Pkcs7::sign` wrote it before. Written here
/// rather than by OpenSSL so its SMIMECapabilities lists only what we read
/// (OpenSSL's own lists triple DES, DES and RC2).
fn sign_rsa(
    cert: &X509,
    chain: &[X509],
    key: &PKey<Private>,
    content: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<u8>> {
    let sha256 = der::encode(der::SEQUENCE, &[&der::oid(oids::SHA256), &der::encode(der::NULL, &[])]);
    let digest = openssl::hash::hash(openssl::hash::MessageDigest::sha256(), content)?;
    // RFC 5652 §5.3, §11: contentType, signingTime and messageDigest,
    // and RFC 8551 §2.5.2's SMIMECapabilities.
    let attributes = [
        attribute(oids::CONTENT_TYPE, &der::oid(oids::DATA)),
        attribute(oids::SIGNING_TIME, &signing_time(now)),
        attribute(oids::MESSAGE_DIGEST, &der::encode(der::OCTET_STRING, &[&digest])),
        attribute(oids::SMIME_CAPABILITIES, &smime_capabilities()),
    ];
    // RFC 5652 §5.4: the signature is over the attributes' DER as a SET
    // OF (sorted, X.690 §11.6), which goes in the message as [0] IMPLICIT.
    let signed_attributes = der::set_of(&attributes);
    let mut signer = openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), key)?;
    signer.set_rsa_padding(openssl::rsa::Padding::PKCS1)?;
    signer.update(&signed_attributes)?;
    let signature = signer.sign_to_vec()?;
    let mut implicit = signed_attributes;
    if let Some(tag) = implicit.first_mut() {
        *tag = der::CONTEXT_0;
    }
    // RFC 5652 §5.3: version 1, with issuerAndSerialNumber.
    let issuer = cert.issuer_name().to_der()?;
    let serial = der::unsigned_integer(&cert.serial_number().to_bn()?.to_vec());
    let rsa = der::encode(der::SEQUENCE, &[&der::oid(oids::RSA_ENCRYPTION), &der::encode(der::NULL, &[])]);
    let version = der::unsigned_integer(&[1]);
    let signer_info = der::encode(
        der::SEQUENCE,
        &[
            &version,
            &der::encode(der::SEQUENCE, &[&issuer, &serial]),
            &sha256,
            &implicit,
            &rsa,
            &der::encode(der::OCTET_STRING, &[&signature]),
        ],
    );
    let certificates: Vec<Vec<u8>> = std::iter::once(cert)
        .chain(chain)
        .map(|c| c.to_der())
        .collect::<Result<_, _>>()?;
    let certificates: Vec<&[u8]> = certificates.iter().map(Vec::as_slice).collect();
    // RFC 5652 §5.1: version 1, detached (no eContent).
    let signed_data = der::encode(
        der::SEQUENCE,
        &[
            &version,
            &der::encode(der::SET, &[&sha256]),
            &der::encode(der::SEQUENCE, &[&der::oid(oids::DATA)]),
            &der::encode(der::CONTEXT_0, &certificates),
            &der::encode(der::SET, &[&signer_info]),
        ],
    );
    Ok(der::encode(
        der::SEQUENCE,
        &[&der::oid(der::SIGNED_DATA), &der::encode(der::CONTEXT_0, &[&signed_data])],
    ))
}

/// RFC 8551 §3.5.3: `content` and its detached signature as
/// multipart/signed, the signature base64 in lines of 64.
fn multipart_signed(content: &[u8], signature: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let boundary = format!("----{}", uuid::Uuid::new_v4().simple().to_string().to_uppercase());
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature);
    let mut out = format!(
        "Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"{boundary}\"\r\n\r\n\
         This is an S/MIME signed message\r\n\r\n--{boundary}\r\n"
    )
    .into_bytes();
    out.extend_from_slice(content);
    out.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
             Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\n"
        )
        .as_bytes(),
    );
    for line in encoded.as_bytes().chunks(64) {
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// Signs `content` (a MIME part, CRLF line ends) as multipart/signed: RSA
/// keys with `sign_rsa`, others (EC) with OpenSSL's CMS_sign.
fn sign_part(cert: &X509, chain: &[X509], key: &PKey<Private>, content: &[u8]) -> Result<Vec<u8>> {
    if key.rsa().is_ok() {
        let signature = sign_rsa(cert, chain, key, content, chrono::Utc::now())?;
        return Ok(multipart_signed(content, &signature));
    }
    let mut extra = Stack::new()?;
    for c in chain {
        extra.push(c.clone())?;
    }
    // NOSMIMECAP: OpenSSL's own SMIMECapabilities lists triple DES, DES
    // and RC2, which we never advertise; with none, a reply falls back to
    // what the recipient knows of us (RFC 8551 §2.7.1).
    let flags = CMSOptions::DETACHED | CMSOptions::BINARY | CMSOptions::NOSMIMECAP;
    let signed = CmsContentInfo::sign(Some(cert), Some(key), Some(&extra), Some(content), flags)?;
    Ok(multipart_signed(content, &signed.to_der()?))
}

/// RFC 9788 §1.1.2 (RFC 9787's user-facing fields): the header fields
/// copied into the Cryptographic Payload. Bcc never is (RFC 9788 §11.4):
/// each Bcc recipient gets a copy of their own, and inside it would show
/// them to everyone who can decrypt. To be merged with the hp sub-lane's
/// OpenPGP helper, which uses the same set.
const INNER_HEADERS: [&str; 9] = [
    "from",
    "to",
    "cc",
    "reply-to",
    "subject",
    "date",
    "message-id",
    "in-reply-to",
    "references",
];

/// A header section's fields, each with its folded lines, and the name
/// lowercased.
fn header_fields(head: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in head.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).is_empty() {
            continue;
        }
        let mut line = line.to_owned();
        if !line.ends_with('\n') {
            line.push_str("\r\n");
        }
        match out.last_mut() {
            Some((_, field)) if line.starts_with([' ', '\t']) => field.push_str(&line),
            _ => {
                let name = line.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
                out.push((name, line));
            }
        }
    }
    out
}

/// RFC 9788 §5.2.1: the MIME part about to be signed or encrypted, with
/// the message's user-facing header fields copied into it and its
/// Content-Type marked hp="clear" (signed only) or hp="cipher"
/// (encrypted). Encrypted, every non-structural field left outside is
/// recorded in an HP-Outer field (§2.2), so a reader knows none were made
/// confidential. The outer header section stays as it is.
fn protected_part(part: &[u8], outer: &[u8], encrypt: bool) -> Vec<u8> {
    let latin1 = |b: &[u8]| -> String { b.iter().map(|&c| c as char).collect() };
    let part = latin1(part);
    let outer = latin1(outer);
    let (head, body) = if part.starts_with("\r\n") {
        ("", part.as_str())
    } else {
        match part.find("\r\n\r\n") {
            Some(at) => (&part[..at + 2], &part[at + 2..]),
            None => (part.as_str(), "\r\n"),
        }
    };
    let hp = if encrypt { "cipher" } else { "clear" };
    let mut new_head = String::new();
    let mut marked = false;
    for (name, field) in header_fields(head) {
        if name == "content-type" && !marked {
            let field = field.trim_end_matches(['\r', '\n']);
            new_head.push_str(&format!("{field}; hp=\"{hp}\"\r\n"));
            marked = true;
        } else {
            new_head.push_str(&field);
        }
    }
    if !marked {
        // RFC 2045 §5.2: without one, the part is text/plain in US-ASCII.
        new_head.insert_str(0, &format!("Content-Type: text/plain; charset=us-ascii; hp=\"{hp}\"\r\n"));
    }
    let fields = header_fields(&outer);
    for (name, field) in &fields {
        if INNER_HEADERS.contains(&name.as_str()) {
            new_head.push_str(field);
        }
    }
    if encrypt {
        for (name, field) in &fields {
            // RFC 9788 §1.1.1 (RFC 9787): MIME-Version and Content-* are
            // structural, and Bcc isn't sent at all.
            if name == "bcc" || name == "mime-version" || name.starts_with("content-") {
                continue;
            }
            new_head.push_str(&format!("HP-Outer: {field}"));
        }
    }
    let out = new_head + body;
    out.chars().map(|c| c as u8).collect()
}

/// RFC 9788 §4.2: where the opened payload's Content-Type has an hp
/// parameter, its header fields are the message's, so outer fields of the
/// same names give way to them, except From: the outer From is the one
/// each signature's certificate was compared with (`overall`), so an inner
/// one would show an address no check covered. RFC 9788 §4.4's From
/// comparison is left to the merge with the hp sub-lane's reading helper.
/// HP-Outer fields are dropped: they describe the outside and are not the
/// message's own.
fn take_protected_headers(outer: Vec<u8>, payload: &mut Vec<u8>) -> Vec<u8> {
    let Some((ctype, _)) = kind(payload) else {
        return outer;
    };
    let Ok((headers, _)) = mailparse::parse_headers(payload) else {
        return outer;
    };
    use mailparse::MailHeaderMap as _;
    let protected = headers
        .get_first_value("Content-Type")
        .map(|v| mailparse::parse_content_type(&v).params.contains_key("hp"))
        .unwrap_or(false);
    if !protected || ctype.is_empty() {
        return outer;
    }
    let latin1 = |b: &[u8]| -> String { b.iter().map(|&c| c as char).collect() };
    let text = latin1(payload);
    let at = text.find("\r\n\r\n").map_or(text.len(), |i| i + 2);
    let (head, body) = text.split_at(at);
    let inner = header_fields(head);
    let shown = |n: &str| INNER_HEADERS.contains(&n) && n != "from";
    let inside: Vec<&str> = inner.iter().map(|(n, _)| n.as_str()).filter(|n| shown(n)).collect();
    let mut kept: String = header_fields(&latin1(&outer))
        .into_iter()
        .filter(|(n, _)| !inside.contains(&n.as_str()))
        .map(|(_, f)| f)
        .collect();
    let mut rest = String::new();
    for (name, field) in inner {
        if shown(&name) {
            kept.push_str(&field);
        } else if name != "hp-outer" && !INNER_HEADERS.contains(&name.as_str()) {
            rest.push_str(&field);
        }
    }
    *payload = (rest + body).chars().map(|c| c as u8).collect();
    kept.chars().map(|c| c as u8).collect()
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
    // RFC 9788 §5.2: the user-facing header fields are protected too,
    // signed (and encrypted) with the body.
    let mut content = protected_part(&part, &outer, encrypt);
    if sign {
        let (cert, chain, key) = own.as_ref().with_context(|| {
            format!("There's no S/MIME certificate of yours for {from}. Import one in Settings → Encryption, or turn off Sign.")
        })?;
        content = sign_part(cert, chain, key, &content)?;
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

/// Loads OpenSSL 3's legacy provider (RC2, triple DES… for old .p12
/// files), with the default one kept. Whether it is loaded now.
fn legacy_provider() -> bool {
    static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOADED.get_or_init(|| {
        // Loading `legacy` explicitly would otherwise drop the implicit
        // default provider: both are loaded, and kept for the process.
        let default = openssl::provider::Provider::try_load(None, "default", true);
        let legacy = openssl::provider::Provider::try_load(None, "legacy", true);
        match (default, legacy) {
            (Ok(d), Ok(l)) => {
                std::mem::forget(d);
                std::mem::forget(l);
                true
            }
            _ => false,
        }
    })
}

fn trusted_store() -> Result<openssl::x509::store::X509Store> {
    Ok(trusted_builder()?.build())
}

/// The system's trusted authorities, in a store still to be built.
fn trusted_builder() -> Result<X509StoreBuilder> {
    static ROOTS: std::sync::OnceLock<Vec<X509>> = std::sync::OnceLock::new();
    let mut builder = X509StoreBuilder::new()?;
    // OpenSSL's own configured authorities, where it has some.
    let _ = builder.set_default_paths();
    // And the system's (Windows' certificate store, macOS' keychain,
    // Linux' bundle): a vendored OpenSSL knows none of these, and every
    // signature showed as unvouched for. Read once.
    let roots = ROOTS.get_or_init(|| {
        rustls_native_certs::load_native_certs()
            .certs
            .iter()
            .filter_map(|der| X509::from_der(der.as_ref()).ok())
            .collect()
    });
    for root in roots {
        // Duplicates of a default path's are refused; that's fine.
        let _ = builder.add_cert(root.clone());
    }
    Ok(builder)
}

/// Certificate revocation lists the caller supplies (RFC 5280 §5), as PEM
/// files, each checked for the certificate of every signature that matches.
/// Fetching them (from a certificate's CRL distribution points, RFC 5280
/// §4.2.1.13) and asking OCSP responders (RFC 6960) is not done here.
#[derive(Clone, Debug, Default)]
pub struct CrlCheck {
    pub files: Vec<std::path::PathBuf>,
}

impl CrlCheck {
    /// A store with `builder`'s authorities and these lists, checking the
    /// signer's certificate against them (X509_V_FLAG_CRL_CHECK).
    fn store(&self, mut builder: X509StoreBuilder) -> Result<openssl::x509::store::X509Store> {
        let lookup = builder.add_lookup(openssl::x509::store::X509Lookup::file())?;
        for file in &self.files {
            // X509Lookup::load_crl_file panics on a path that isn't UTF-8
            // or holds a NUL: refused here instead.
            ensure!(
                file.to_str().is_some_and(|f| !f.contains('\0')),
                "A revocation list's path can't be read: {}",
                file.display()
            );
            lookup
                .load_crl_file(file, openssl::ssl::SslFiletype::PEM)
                .with_context(|| format!("{} isn't a revocation list (PEM).", file.display()))?;
        }
        builder.set_flags(openssl::x509::verify::X509VerifyFlags::CRL_CHECK)?;
        Ok(builder.build())
    }
}

/// x509_vfy.h's X509_V_ERR_CERT_REVOKED, which the openssl crate doesn't name.
const CERT_REVOKED: i32 = 23;

/// Whether `crls` (a store from `CrlCheck::store`) says `cert` was revoked.
/// Only that answer counts: a chain that can't be built, or an issuer with
/// no list supplied, leaves the result as the signature check found it.
fn revoked(cert: &X509, untrusted: &[X509], crls: &X509StoreRef) -> bool {
    let Ok(mut chain) = Stack::new() else {
        return false;
    };
    for c in untrusted {
        if chain.push(c.clone()).is_err() {
            return false;
        }
    }
    let Ok(mut context) = openssl::x509::X509StoreContext::new() else {
        return false;
    };
    context
        .init(crls, cert, &chain, |c| {
            c.verify_cert()?;
            Ok(c.error())
        })
        .is_ok_and(|error| error.as_raw() == CERT_REVOKED)
}

/// Opens an S/MIME message: decrypts it with your key and checks its
/// signatures. Returns the readable message (with its outer headers), what
/// was found, and the signer's certificate (to keep, so you can write to
/// them encrypted). `None` when it has no S/MIME protection, or can't be
/// opened.
pub fn open(raw: &[u8], certs: &Certificates) -> Option<(Vec<u8>, Verdict, Option<X509>)> {
    let store = trusted_store().ok()?;
    open_with(raw, certs, &store)
}

/// `open`, with each signer's certificate also checked against the
/// revocation lists `crls` supplies.
#[cfg_attr(not(test), allow(dead_code))]
pub fn open_checking_revocation(
    raw: &[u8],
    certs: &Certificates,
    crls: &CrlCheck,
) -> Result<Option<(Vec<u8>, Verdict, Option<X509>)>> {
    let store = trusted_store()?;
    let crls = crls.store(trusted_builder()?)?;
    Ok(open_checked(raw, certs, &store, Some(&crls)))
}

/// The most layers read: encryption, a signature, a second signature (RFC
/// 2634 §1.1's triple wrapping) and one compression.
const MAX_LAYERS: usize = 4;

/// `open`, with signatures checked against `store`.
fn open_with(
    raw: &[u8],
    certs: &Certificates,
    store: &X509StoreRef,
) -> Option<(Vec<u8>, Verdict, Option<X509>)> {
    open_checked(raw, certs, store, None)
}

/// `open_with`, and where `crls` is given, each signer's certificate
/// checked against its revocation lists.
fn open_checked(
    raw: &[u8],
    certs: &Certificates,
    store: &X509StoreRef,
    crls: Option<&X509StoreRef>,
) -> Option<(Vec<u8>, Verdict, Option<X509>)> {
    // As OpenPGP's: past this, the copies made opening it (decoded,
    // decrypted, each layer's) would be several times its size.
    if raw.len() > crate::manage::crypto::MAX_MESSAGE {
        return None;
    }
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
    let mut compressed = false;
    let known = certs.known();
    for _ in 0..MAX_LAYERS {
        let (k, _) = kind(&current)?;
        let (content, found) = if matches!(
            k.as_str(),
            "application/pkcs7-mime" | "application/x-pkcs7-mime"
        ) {
            // The structure itself says what it is (RFC 8551 §3.2.2:
            // smime-type is optional, and a sender can get it wrong).
            // Read through OpenSSL and written back as DER, which also
            // turns BER's indefinite lengths into definite ones.
            let der = CmsContentInfo::smime_read_cms(&current).ok()?.to_der().ok()?;
            let (oid, _) = der::content_info(&der).ok()?;
            if oid == der::SIGNED_DATA {
                let signed = der::SignedData::parse(&der).ok()?;
                if signed.signer_infos.is_empty() && signed.content().ok()?.is_none() {
                    // RFC 8551 §3.6: certs-only, a way of handing over
                    // certificates. Shown as it came, for importing.
                    verdict.certificates = signed
                        .certificates()
                        .ok()?
                        .into_iter()
                        .filter_map(|c| X509::from_der(c).ok()?.to_pem().ok())
                        .filter_map(|pem| String::from_utf8(pem).ok())
                        .collect();
                    if verdict.certificates.is_empty() {
                        return None;
                    }
                    break;
                }
                let (content, signers) = check_signers(&der, None, store, crls, &known)?;
                (content, Some(signers))
            } else if oid == der::COMPRESSED_DATA {
                // RFC 3274: one level, at most, so a small message can't
                // unpack into a large one again and again.
                if compressed {
                    return None;
                }
                compressed = true;
                (inflate(der::compressed_content(&der).ok()?, crate::manage::crypto::MAX_MESSAGE)?, None)
            } else {
                let opened = decrypt(&der, certs)?;
                verdict.decrypted = true;
                (opened, None)
            }
        } else if k.starts_with("multipart/signed;") {
            let (content, signature) = signed_parts(&current)?;
            let der = CmsContentInfo::from_der(&signature).ok()?.to_der().ok()?;
            let (_, signers) = check_signers(&der, Some(&content), store, crls, &known)?;
            (content, Some(signers))
        } else {
            break;
        };
        if let Some(found) = found {
            let (signature, cert) = overall(&found, raw);
            // A layer that failed stays failed: an inner signature that
            // checks out doesn't hide that an outer one didn't.
            if verdict.signature != Signature::Bad {
                verdict.signature = signature;
                signer = cert;
            }
            verdict.signers.extend(found.into_iter().map(|(s, _)| s));
        }
        current = content;
    }
    if !verdict.certificates.is_empty() {
        return Some((raw.to_vec(), verdict, None));
    }
    // RFC 9788 §4.4: the protected From is vouched for by a signer whose
    // certificate (one that checked out) names that address.
    verdict.from_mismatch = crate::manage::openpgp::from_mismatch(&outer, &current, |address| {
        let address = address.to_lowercase();
        verdict.signers.iter().any(|s| {
            matches!(s.result, SignerResult::Good | SignerResult::Untrusted)
                && s.address.to_lowercase() == address
        })
    });
    let mut readable = take_protected_headers(outer, &mut current);
    readable.extend_from_slice(b"MIME-Version: 1.0\r\n");
    // Line ends made CRLF byte by byte: through a UTF-8 string, text in
    // another charset (and binary parts) came out as replacement marks.
    // Only where the content has bare LF line ends throughout (written
    // on a system that uses them): content already using CRLF is kept
    // byte for byte, so a binary part's LF bytes aren't changed.
    let bare = current.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r')
        || current.first() == Some(&b'\n');
    let crlf = current.windows(2).any(|w| w == b"\r\n");
    if bare && !crlf {
        let mut previous = 0u8;
        for &b in &current {
            if b == b'\n' && previous != b'\r' {
                readable.push(b'\r');
            }
            readable.push(b);
            previous = b;
        }
    } else {
        readable.extend_from_slice(&current);
    }
    Some((readable, verdict, signer))
}

/// Decrypts a DER EnvelopedData (RFC 5652 §6) or AuthEnvelopedData (RFC
/// 5083) with whichever of your keys it was encrypted to. CMS_decrypt
/// handles RSA (PKCS#1 v1.5 and OAEP, RFC 3560) and ECDH (RFC 5753)
/// recipients, and AES-GCM (RFC 5084), whose tag it checks before giving
/// back any content. With the certificate given, only the recipient info
/// naming it is tried (OpenSSL's guard against Bleichenbacher's attack).
fn decrypt(der: &[u8], certs: &Certificates) -> Option<Vec<u8>> {
    let cms = CmsContentInfo::from_der(der).ok()?;
    certs
        .identities()
        .into_iter()
        .find_map(|(cert, _, key)| cms.decrypt(&key, &cert).ok())
}

/// RFC 1847 §2.1: a multipart/signed's first part, byte for byte as it was
/// signed, and its second, the signature, decoded.
fn signed_parts(raw: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let parsed = mailparse::parse_mail(raw).ok()?;
    let [content, signature] = parsed.subparts.as_slice() else {
        return None;
    };
    Some((content.raw_bytes.to_vec(), signature.get_body_raw().ok()?))
}

/// RFC 3274 §1.1: zlib-compressed content, inflated up to `limit` (the
/// largest message opened); past that, it isn't.
fn inflate(packed: &[u8], limit: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(packed)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    (out.len() <= limit).then_some(out)
}

/// The message as a whole, from its signers' results (RFC 5652 §5.1 lets
/// one message carry several): Bad if any signature is cryptographically
/// invalid; Revoked if any certificate is; otherwise the best of the rest, so a good signature beside one
/// that couldn't be checked shows as good. The certificate is that best
/// signer's, preferring one for the address the message is from.
fn overall(signers: &[(Signer, Option<X509>)], raw: &[u8]) -> (Signature, Option<X509>) {
    if signers.iter().any(|(s, _)| s.result == SignerResult::Bad) {
        let cert = signers
            .iter()
            .find(|(s, _)| s.result == SignerResult::Bad)
            .and_then(|(_, c)| c.clone());
        return (Signature::Bad, cert);
    }
    // RFC 5280 §3.3: a revoked certificate speaks for no one, so a
    // signature by one isn't outweighed by a good one beside it.
    if let Some((signer, cert)) = signers.iter().find(|(s, _)| s.result == SignerResult::Revoked) {
        return (Signature::Revoked(signer.address.clone()), cert.clone());
    }
    let from = {
        use mailparse::MailHeaderMap as _;
        mailparse::parse_headers(raw)
            .ok()
            .and_then(|(h, _)| h.get_first_value("From"))
            .map(|v| crate::sync::mime::receipt_address(&v).to_lowercase())
            .unwrap_or_default()
    };
    for best in [SignerResult::Good, SignerResult::Untrusted] {
        let mut candidates = signers.iter().filter(|(s, _)| s.result == best);
        let first = candidates.clone().next();
        let chosen = candidates
            .find(|(_, c)| c.as_ref().is_some_and(|c| emails(c).contains(&from)))
            .or(first);
        let Some((signer, cert)) = chosen else {
            continue;
        };
        let who = signer.address.clone();
        // A signature is the sender's only with a certificate for the
        // address the message is from: a message signed by one person can
        // be sent on, re-encrypted, as from another.
        let other = !from.is_empty() && cert.as_ref().is_some_and(|c| !emails(c).contains(&from));
        let signature = match (other, best) {
            (true, _) => Signature::OtherSender(who),
            (false, SignerResult::Good) => Signature::Good(who),
            (false, _) => Signature::Untrusted(who),
        };
        return (signature, cert.clone());
    }
    (Signature::Unverified, None)
}

/// The certificate a SignerInfo names (RFC 5652 §5.3 SignerIdentifier),
/// from the message's own or those already known.
fn signer_certificate(signer_info: &der::Tlv<'_>, message: &[X509], known: &[X509]) -> Option<X509> {
    let id = der::signer_id(signer_info).ok()?;
    message.iter().chain(known).find_map(|cert| {
        let matches = match id {
            der::SignerId::SubjectKeyId(key_id) => {
                cert.subject_key_id().is_some_and(|k| k.as_slice() == key_id)
            }
            der::SignerId::IssuerAndSerial { issuer, serial } => {
                let encoded = cert.to_der().ok()?;
                der::issuer_and_serial(&encoded).ok() == Some((issuer, serial))
            }
        };
        matches.then(|| cert.clone())
    })
}

/// Checks each signature of a DER SignedData on its own, the content
/// (`detached`, or the one inside) and certificates kept, and with `crls`
/// each matching signature's certificate against revocation lists. Returns
/// the content and each signer's result with its certificate.
fn check_signers(
    der: &[u8],
    detached: Option<&[u8]>,
    store: &X509StoreRef,
    crls: Option<&X509StoreRef>,
    known: &[X509],
) -> Option<(Vec<u8>, Vec<(Signer, Option<X509>)>)> {
    let signed = der::SignedData::parse(der).ok()?;
    let content = match detached {
        Some(content) => content.to_vec(),
        None => signed.content().ok()??.to_vec(),
    };
    let message: Vec<X509> = signed
        .certificates()
        .ok()?
        .into_iter()
        .filter_map(|c| X509::from_der(c).ok())
        .collect();
    let mut out = Vec::new();
    for signer_info in &signed.signer_infos {
        let cert = signer_certificate(signer_info, &message, known);
        let address = cert
            .as_ref()
            .and_then(|c| emails(c).into_iter().next())
            .unwrap_or_else(|| "an unknown signer".into());
        let result = match &cert {
            None => SignerResult::Unverified,
            Some(cert) => match check_one(&signed.with_signers(&[signer_info.raw]), detached, cert, store) {
                SignerResult::Good | SignerResult::Untrusted
                    if crls.is_some_and(|crls| revoked(cert, &message, crls)) =>
                {
                    SignerResult::Revoked
                }
                result => result,
            },
        };
        out.push((Signer { address, result }, cert));
    }
    Some((content, out))
}

/// One signer's SignedData, checked first against trusted authorities, then
/// for the signature alone. CMS_verify compares the messageDigest attribute
/// with the content's digest (RFC 5652 §5.4), so changed content fails both.
fn check_one(single: &[u8], detached: Option<&[u8]>, cert: &X509, store: &X509StoreRef) -> SignerResult {
    let attempt = |flags: CMSOptions| -> bool {
        let Ok(mut cms) = CmsContentInfo::from_der(single) else {
            return false;
        };
        let Ok(mut extra) = Stack::new() else {
            return false;
        };
        if extra.push(cert.clone()).is_err() {
            return false;
        }
        cms.verify(Some(&extra), Some(store), detached, None, flags).is_ok()
    };
    if attempt(CMSOptions::BINARY) {
        return SignerResult::Good;
    }
    // Checked without its chain (NOVERIFY), so the certificate itself is
    // checked here: in its dates (OpenSSL checks them with the chain, for
    // Good), and meant for signing mail where it says what it's for.
    if attempt(CMSOptions::BINARY | CMSOptions::NOVERIFY) && usable_for_mail(cert) {
        return SignerResult::Untrusted;
    }
    SignerResult::Bad
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
    if matches!(verdict, Signature::Bad | Signature::None | Signature::Unverified | Signature::Revoked(_)) {
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
            // Unvouched, it speaks only for the address that sent it: one
            // also naming another address (victim@ beside attacker@) would
            // have mail to that other person encrypted to the sender.
            if addresses.iter().any(|a| *a != from) {
                return Ok(false);
            }
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
        // Ana's signature on a message that says it is from someone else.
        let resent = text.replacen("From: Ana <ana@example.org>", "From: Bank <alerts@bank.example>", 1);
        assert_eq!(
            open(resent.as_bytes(), &bo).unwrap().1.signature,
            Signature::OtherSender("ana@example.org".into())
        );
        let forged = text.replace("Meet at noon.", "Meet at nine.");
        assert_eq!(
            open(forged.as_bytes(), &bo).unwrap().1.signature,
            Signature::Bad
        );
        assert!(open(MESSAGE, &bo).is_none());
    }

    /// A certificate for something other than signing mail (a web server's)
    /// or not valid now doesn't vouch for a message, even unvouched.
    #[test]
    fn only_certificates_for_mail_sign_it() {
        use openssl::{asn1::Asn1Time, hash::MessageDigest, rsa::Rsa, x509::extension::{ExtendedKeyUsage, KeyUsage, SubjectAlternativeName}};
        let make = |extended: Option<ExtendedKeyUsage>, from: i32, to: i32| {
            let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
            let mut b = X509::builder().unwrap();
            b.set_version(2).unwrap();
            b.set_pubkey(&key).unwrap();
            let at = |d: i32| if d >= 0 { Asn1Time::days_from_now(d as u32).unwrap() } else {
                Asn1Time::from_unix(chrono::Utc::now().timestamp() + i64::from(d) * 86_400).unwrap()
            };
            b.set_not_before(&at(from)).unwrap();
            b.set_not_after(&at(to)).unwrap();
            let san = SubjectAlternativeName::new().email("ana@example.org").build(&b.x509v3_context(None, None)).unwrap();
            b.append_extension(san).unwrap();
            b.append_extension(KeyUsage::new().digital_signature().build().unwrap()).unwrap();
            if let Some(e) = extended {
                b.append_extension(e.build().unwrap()).unwrap();
            }
            b.sign(&key, MessageDigest::sha256()).unwrap();
            b.build()
        };
        let mut mail = ExtendedKeyUsage::new();
        mail.email_protection();
        let mut web = ExtendedKeyUsage::new();
        web.server_auth();
        assert!(usable_for_mail(&make(Some(mail), 0, 30)));
        assert!(usable_for_mail(&make(None, 0, 30)));
        assert!(!usable_for_mail(&make(Some(web), 0, 30)));
        assert!(!usable_for_mail(&make(None, -60, -30)), "expired");
        assert!(!usable_for_mail(&make(None, 10, 30)), "not yet valid");
    }

    /// Text in a charset other than UTF-8 comes out byte for byte.
    #[test]
    fn signed_latin1_text_keeps_its_bytes() {
        let (ana, bo) = people();
        let latin1 = b"From: ana@example.org\r\nTo: bo@example.se\r\nSubject: Caf\xe9\r\nContent-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: 8bit\r\n\r\nUn caf\xe9.\r\n";
        let signed = protect_with(&ana, latin1, "ana@example.org", &[], true, false).unwrap();
        let (readable, verdict, _) = open(&signed, &bo).unwrap();
        // Unchanged after signing, so the signature still matches.
        assert_eq!(verdict.signature, Signature::Untrusted("ana@example.org".into()));
        assert!(readable.windows(8).any(|w| w == b"Un caf\xe9."), "{}", String::from_utf8_lossy(&readable));
    }

    /// A self-signed certificate naming the sender's address and another's
    /// isn't kept: mail to the other would be encrypted to the sender.
    #[test]
    fn unvouched_certificates_speak_only_for_their_sender() {
        use openssl::{asn1::Asn1Time, hash::MessageDigest, rsa::Rsa, x509::extension::SubjectAlternativeName};
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_pubkey(&key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        b.set_not_after(&Asn1Time::days_from_now(30).unwrap()).unwrap();
        let san = SubjectAlternativeName::new()
            .email("eve@example.net")
            .email("ana@example.org")
            .build(&b.x509v3_context(None, None))
            .unwrap();
        b.append_extension(san).unwrap();
        b.sign(&key, MessageDigest::sha256()).unwrap();
        let both = b.build();
        let mut bo = Certificates::default();
        let untrusted = Signature::Untrusted("eve@example.net".into());
        assert!(!learn(&mut bo, &both, &untrusted, "eve@example.net").unwrap());
        assert!(!bo.has_cert("ana@example.org"));
        assert!(!bo.has_cert("eve@example.net"));
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

    /// The fixtures tests/fixtures/smime/make.sh makes with the openssl CLI.
    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/smime/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// A fixture identity: its certificate and key, as `import_identity`
    /// keeps them.
    fn fixture_identity(name: &str) -> Certificates {
        let key = PKey::private_key_from_pem(&fixture(&format!("{name}.key"))).unwrap();
        let pem = String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        Certificates {
            own: vec![(String::from_utf8(fixture(&format!("{name}.pem"))).unwrap(), pem)],
            others: Vec::new(),
        }
    }

    /// The fixtures' test authority, as the only one trusted.
    fn test_authority() -> openssl::x509::store::X509Store {
        let mut builder = X509StoreBuilder::new().unwrap();
        builder.add_cert(X509::from_pem(&fixture("ca.pem")).unwrap()).unwrap();
        builder.build()
    }

    fn untrusting() -> openssl::x509::store::X509Store {
        X509StoreBuilder::new().unwrap().build()
    }

    /// The signers' results, sorted: RFC 5652 §5.1's signerInfos is a SET,
    /// whose DER order isn't the order they signed in.
    fn results(verdict: &Verdict) -> Vec<(&str, SignerResult)> {
        let mut out: Vec<_> = verdict.signers.iter().map(|s| (s.address.as_str(), s.result)).collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    #[test]
    fn each_of_two_signers_is_checked_on_its_own() {
        let (readable, verdict, signer) =
            open_with(&fixture("two-signers.eml"), &Certificates::default(), &test_authority()).unwrap();
        assert_eq!(
            results(&verdict),
            [("ana@example.org", SignerResult::Good), ("bo@example.se", SignerResult::Good)]
        );
        // The message is from Ana: hers is the signature shown and kept.
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        assert_eq!(signer.map(|c| emails(&c)), Some(vec!["ana@example.org".to_owned()]));
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
    }

    /// RFC 5652 §5.4: each signature covers the content through its
    /// messageDigest attribute, so changed content fails every signer.
    #[test]
    fn changed_content_fails_every_signer() {
        let raw = String::from_utf8(fixture("two-signers.eml")).unwrap();
        let changed = raw.replace("Meet at noon.", "Meet at nine.");
        let (_, verdict, _) =
            open_with(changed.as_bytes(), &Certificates::default(), &test_authority()).unwrap();
        assert_eq!(
            results(&verdict),
            [("ana@example.org", SignerResult::Bad), ("bo@example.se", SignerResult::Bad)]
        );
        assert_eq!(verdict.signature, Signature::Bad);
    }

    /// A signer whose certificate the message lacks is unverified, not bad:
    /// beside a good one the message shows as good.
    #[test]
    fn a_signer_without_a_certificate_is_unverified_not_bad() {
        let (_, verdict, _) = open_with(
            &fixture("one-certificate-missing.eml"),
            &Certificates::default(),
            &test_authority(),
        )
        .unwrap();
        assert_eq!(
            results(&verdict),
            [("an unknown signer", SignerResult::Unverified), ("ana@example.org", SignerResult::Good)]
        );
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        // Untrusted beside unverified is untrusted.
        let (_, verdict, _) =
            open_with(&fixture("one-certificate-missing.eml"), &Certificates::default(), &untrusting())
                .unwrap();
        assert_eq!(verdict.signature, Signature::Untrusted("ana@example.org".into()));
        // Only unverified signers: unverified.
        let (readable, verdict, signer) =
            open_with(&fixture("certificate-missing.eml"), &Certificates::default(), &test_authority())
                .unwrap();
        assert_eq!(results(&verdict), [("an unknown signer", SignerResult::Unverified)]);
        assert_eq!(verdict.signature, Signature::Unverified);
        assert!(signer.is_none());
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        // With Bo's certificate already kept, his signature is checked.
        let mut known = Certificates::default();
        known.others.push(String::from_utf8(fixture("bo.pem")).unwrap());
        let (_, verdict, _) =
            open_with(&fixture("certificate-missing.eml"), &known, &test_authority()).unwrap();
        assert_eq!(results(&verdict), [("bo@example.se", SignerResult::Good)]);
        // Bo's signature on mail from Ana: not Ana's.
        assert_eq!(verdict.signature, Signature::OtherSender("bo@example.se".into()));
    }

    /// One cryptographically invalid signature makes the message bad, beside
    /// a good one or one that couldn't be checked.
    #[test]
    fn one_invalid_signature_makes_the_message_bad() {
        let (content, signature) = signed_parts(&fixture("two-signers.eml")).unwrap();
        let der = CmsContentInfo::from_der(&signature).unwrap().to_der().unwrap();
        let signed = der::SignedData::parse(&der).unwrap();
        let [ana, bo] = [signed.signer_infos[0].raw, signed.signer_infos[1].raw];
        // SignerInfo ends with its signature value (RFC 5652 §5.3, no
        // unsigned attributes here): one bit of it changed.
        let mut forged = bo.to_vec();
        if let Some(last) = forged.last_mut() {
            *last ^= 1;
        }
        let store = test_authority();
        let check = |infos: &[&[u8]]| {
            let mixed = signed.with_signers(infos);
            let (_, found) = check_signers(&mixed, Some(&content), &store, None, &[]).unwrap();
            let mut results: Vec<SignerResult> = found.iter().map(|(s, _)| s.result).collect();
            results.sort_by_key(|r| *r as u8);
            (results, overall(&found, b"From: ana@example.org\r\n\r\n").0)
        };
        assert_eq!(
            check(&[ana, &forged]),
            (vec![SignerResult::Good, SignerResult::Bad], Signature::Bad)
        );
        // The forged signature alone is bad too.
        let (_, found) =
            check_signers(&signed.with_signers(&[&forged]), Some(&content), &store, None, &[]).unwrap();
        assert_eq!(found[0].0.result, SignerResult::Bad);
        assert_eq!(
            check(&[ana, bo]),
            (vec![SignerResult::Good, SignerResult::Good], Signature::Good("ana@example.org".into()))
        );
    }

    /// How signers add up, for every mix of results.
    #[test]
    fn overall_result_is_bad_if_any_is_and_otherwise_the_best() {
        let ana = X509::from_pem(&fixture("ana.pem")).unwrap();
        let signer = |result, cert: Option<&X509>| {
            let address = cert.map_or("an unknown signer".into(), |c| emails(c)[0].clone());
            (Signer { address, result }, cert.cloned())
        };
        let from = b"From: Ana <ana@example.org>\r\n\r\n";
        let good = || Signature::Good("ana@example.org".into());
        let untrusted = || Signature::Untrusted("ana@example.org".into());
        use SignerResult::*;
        for (mix, expected) in [
            (vec![signer(Good, Some(&ana)), signer(Unverified, None)], good()),
            (vec![signer(Unverified, None), signer(Untrusted, Some(&ana))], untrusted()),
            (vec![signer(Untrusted, Some(&ana)), signer(Good, Some(&ana))], good()),
            (vec![signer(Good, Some(&ana)), signer(Bad, Some(&ana))], Signature::Bad),
            (vec![signer(Unverified, None), signer(Bad, Some(&ana))], Signature::Bad),
            (vec![signer(Unverified, None), signer(Unverified, None)], Signature::Unverified),
            (vec![], Signature::Unverified),
        ] {
            assert_eq!(overall(&mix, from).0, expected, "{mix:?}");
        }
    }

    /// RFC 8551 §3.6: certs-only mail hands over certificates, signs nothing.
    #[test]
    fn certs_only_mail_lists_its_certificates() {
        let (readable, verdict, signer) =
            open_with(&fixture("certs-only.eml"), &Certificates::default(), &test_authority()).unwrap();
        assert_eq!(verdict.signature, Signature::None);
        assert!(verdict.signers.is_empty() && signer.is_none() && !verdict.decrypted);
        let names: Vec<String> = verdict
            .certificates
            .iter()
            .map(|pem| name(X509::from_pem(pem.as_bytes()).unwrap().subject_name()))
            .collect();
        let mut names = names;
        names.sort();
        assert_eq!(names, ["Ana", "Picacho Test CA"]);
        assert_eq!(readable, fixture("certs-only.eml"));
    }

    /// RFC 3274: compressed content, alone or inside a signature, opens;
    /// compressed twice, it doesn't.
    #[test]
    fn compressed_mail_opens_once_compressed() {
        let none = Certificates::default();
        let (readable, verdict, _) = open_with(&fixture("compressed.eml"), &none, &test_authority()).unwrap();
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        assert_eq!(verdict.signature, Signature::None);
        let (readable, verdict, _) =
            open_with(&fixture("signed-compressed.eml"), &none, &test_authority()).unwrap();
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        assert!(open_with(&fixture("compressed-twice.eml"), &none, &test_authority()).is_none());
    }

    #[test]
    fn compressed_content_is_inflated_only_up_to_the_limit() {
        use std::io::Write as _;
        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        zlib.write_all(&vec![0u8; 100_000]).unwrap();
        let packed = zlib.finish().unwrap();
        assert!(packed.len() < 1_000);
        assert_eq!(inflate(&packed, 100_000).map(|v| v.len()), Some(100_000));
        assert_eq!(inflate(&packed, 99_999), None);
        assert_eq!(inflate(b"not zlib", 100), None);
    }

    /// Opaque signing as Outlook writes it: BER, with indefinite lengths.
    #[test]
    fn opaque_ber_signed_mail_is_checked() {
        let (readable, verdict, _) =
            open_with(&fixture("opaque-signed-ber.eml"), &Certificates::default(), &test_authority())
                .unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
    }

    /// Old PKCS#7 mail (application/x-pkcs7-signature, as `openssl smime`
    /// and older clients write it) still reads.
    #[test]
    fn old_pkcs7_signed_mail_still_reads() {
        let (readable, verdict, _) =
            open_with(&fixture("pkcs7-signed.eml"), &Certificates::default(), &test_authority()).unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
    }

    /// RSA-OAEP recipients (RFC 3560), as recent Outlook writes them.
    #[test]
    fn rsa_oaep_encrypted_mail_decrypts() {
        let (readable, verdict, _) =
            open_with(&fixture("encrypted-rsa-oaep.eml"), &fixture_identity("ana"), &test_authority()).unwrap();
        assert!(verdict.decrypted);
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
        assert!(open_with(&fixture("encrypted-rsa-oaep.eml"), &fixture_identity("bo"), &test_authority()).is_none());
    }

    /// ECDH recipients (RFC 5753), for EC certificates.
    #[test]
    fn ecdh_encrypted_mail_decrypts() {
        let (readable, verdict, _) =
            open_with(&fixture("encrypted-ecdh.eml"), &fixture_identity("bo"), &test_authority()).unwrap();
        assert!(verdict.decrypted);
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
    }

    /// AES-GCM as AuthEnvelopedData (RFC 5083, RFC 5084), for an RSA and
    /// an ECDH recipient; a changed ciphertext fails its tag.
    #[test]
    fn aes_gcm_auth_enveloped_mail_decrypts_and_is_authenticated() {
        let raw = fixture("encrypted-aes-gcm.eml");
        for who in ["ana", "bo"] {
            let (readable, verdict, _) = open_with(&raw, &fixture_identity(who), &test_authority()).unwrap();
            assert!(verdict.decrypted, "{who}");
            assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."), "{who}");
        }
        // One byte of the encrypted content changed: the DER's last
        // value is the GCM tag (RFC 5083 §2.1 mac), and before it the
        // ciphertext.
        let der = CmsContentInfo::smime_read_cms(&raw).unwrap().to_der().unwrap();
        let mut changed = der.clone();
        let at = changed.len() - 40;
        changed[at] ^= 1;
        assert!(decrypt(&der, &fixture_identity("ana")).is_some());
        assert!(decrypt(&changed, &fixture_identity("ana")).is_none());
        let mut tag = der.clone();
        if let Some(last) = tag.last_mut() {
            *last ^= 1;
        }
        assert!(decrypt(&tag, &fixture_identity("ana")).is_none());
    }

    /// Old PKCS#7 encrypted mail (application/x-pkcs7-mime, RSA PKCS#1
    /// v1.5) still reads.
    #[test]
    fn old_pkcs7_encrypted_mail_still_reads() {
        let (readable, verdict, _) =
            open_with(&fixture("pkcs7-encrypted.eml"), &fixture_identity("ana"), &test_authority()).unwrap();
        assert!(verdict.decrypted);
        assert!(String::from_utf8_lossy(&readable).contains("Meet at noon."));
    }

    /// Runs the `openssl` CLI (`smime` or `cms`) to verify `signed`
    /// against the fixtures' authority, under a deadline.
    fn openssl_cli_verifies(command: &str, signed: &[u8]) -> bool {
        let dir = std::env::temp_dir().join(format!("picacho-smime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let message = dir.join("signed.eml");
        std::fs::write(&message, signed).unwrap();
        let ca = format!("{}/tests/fixtures/smime/ca.pem", env!("CARGO_MANIFEST_DIR"));
        let status = std::process::Command::new("timeout")
            .args(["30", "openssl", command, "-verify", "-CAfile", &ca, "-purpose", "smimesign", "-in"])
            .arg(&message)
            .args(["-out", "/dev/null"])
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&dir);
        status.is_ok_and(|s| s.success())
    }

    /// Our RSA signature verifies with all four: OpenSSL's CMS and PKCS#7
    /// APIs, and `openssl cms -verify` and `openssl smime -verify`.
    #[test]
    fn signed_mail_verifies_with_cms_pkcs7_and_both_openssl_commands() {
        let ana = fixture_identity("ana");
        let signed = protect_with(&ana, MESSAGE, "ana@example.org", &[], true, false).unwrap();
        let (content, signature) = signed_parts(&signed).unwrap();
        let store = test_authority();
        let mut cms = CmsContentInfo::from_der(&signature).unwrap();
        cms.verify(None, Some(&store), Some(&content), None, CMSOptions::BINARY).unwrap();
        let p7 = Pkcs7::from_der(&signature).unwrap();
        let none = Stack::new().unwrap();
        p7.verify(&none, &store, Some(&content), None, Pkcs7Flags::BINARY).unwrap();
        // The CLIs are part of the test: missing, the test fails.
        assert!(openssl_cli_verifies("cms", &signed), "openssl cms -verify");
        assert!(openssl_cli_verifies("smime", &signed), "openssl smime -verify");
        // Changed after signing, it fails for all four.
        let changed = String::from_utf8_lossy(&signed).replace("Meet at noon.", "Meet at nine.");
        let (changed_content, _) = signed_parts(changed.as_bytes()).unwrap();
        let mut cms = CmsContentInfo::from_der(&signature).unwrap();
        assert!(cms.verify(None, Some(&store), Some(&changed_content), None, CMSOptions::BINARY).is_err());
        assert!(p7.verify(&none, &store, Some(&changed_content), None, Pkcs7Flags::BINARY).is_err());
        assert!(!openssl_cli_verifies("cms", changed.as_bytes()));
        assert!(!openssl_cli_verifies("smime", changed.as_bytes()));
        // And we read it back as good.
        let (_, verdict, _) = open_with(&signed, &Certificates::default(), &store).unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
    }

    /// RFC 5652 §5.3, §5.4 and RFC 8551 §2.5.2: the signed attributes are
    /// DER-sorted and include contentType, signingTime, messageDigest and
    /// SMIMECapabilities, which lists AES and never RC2 or DES; the
    /// signature is sha256 with rsaEncryption (PKCS#1 v1.5), as before.
    #[test]
    fn signed_attributes_are_sorted_and_advertise_only_aes() {
        let ana = fixture_identity("ana");
        let (cert, chain, key) = ana.identities().remove(0);
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-23T10:00:00Z").unwrap().to_utc();
        let signature = sign_rsa(&cert, &chain, &key, b"content", at).unwrap();
        let signed = der::SignedData::parse(&signature).unwrap();
        let [signer_info] = signed.signer_infos.as_slice() else { panic!("one signer") };
        let mut fields = signer_info.children().unwrap();
        for _ in 0..3 {
            fields.read().unwrap();
        }
        let attributes = fields.read_tag(der::CONTEXT_0).unwrap();
        let algorithm = fields.read_tag(der::SEQUENCE).unwrap();
        assert_eq!(
            algorithm.raw,
            der::encode(der::SEQUENCE, &[&der::oid(oids::RSA_ENCRYPTION), &der::encode(der::NULL, &[])])
        );
        let mut each = attributes.children().unwrap();
        let mut raws = Vec::new();
        let mut kinds = Vec::new();
        while !each.is_empty() {
            let attribute = each.read().unwrap();
            raws.push(attribute.raw.to_vec());
            kinds.push(attribute.children().unwrap().read().unwrap().value.to_vec());
        }
        let mut sorted = raws.clone();
        sorted.sort();
        assert_eq!(raws, sorted, "X.690 §11.6 order");
        let mut kinds = kinds;
        kinds.sort();
        let mut expected = vec![
            oids::CONTENT_TYPE.to_vec(),
            oids::MESSAGE_DIGEST.to_vec(),
            oids::SIGNING_TIME.to_vec(),
            oids::SMIME_CAPABILITIES.to_vec(),
        ];
        expected.sort();
        assert_eq!(kinds, expected);
        let text = String::from_utf8(
            std::process::Command::new("timeout")
                .args(["30", "openssl", "cms", "-cmsout", "-print", "-inform", "DER"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write as _;
                    child.stdin.take().unwrap().write_all(&signature)?;
                    child.wait_with_output()
                })
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(text.contains("aes-256-gcm") && text.contains("aes-128-cbc"), "{text}");
        assert!(text.contains("UTCTIME:Nov 23 10:00:00 2026 GMT"), "signingTime: {text}");
        for weak in ["rc2", "des"] {
            assert!(!text.to_lowercase().contains(weak), "{weak}: {text}");
        }
        assert_eq!(signing_time(at), [&[0x17, 13][..], b"261123100000Z"].concat());
        let later = chrono::DateTime::parse_from_rfc3339("2050-01-01T00:00:00Z").unwrap().to_utc();
        assert_eq!(signing_time(later), [&[0x18, 15][..], b"20500101000000Z"].concat());
    }

    /// EC keys sign through OpenSSL's CMS_sign, and verify.
    #[test]
    fn ec_keys_sign_through_cms() {
        let bo = fixture_identity("bo");
        let message = b"From: bo@example.se\r\nTo: ana@example.org\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHi.\r\n";
        let signed = protect_with(&bo, message, "bo@example.se", &[], true, false).unwrap();
        assert!(openssl_cli_verifies("cms", &signed));
        let (_, signature) = signed_parts(&signed).unwrap();
        let smime_capabilities = der::oid(oids::SMIME_CAPABILITIES);
        assert!(!signature.windows(smime_capabilities.len()).any(|w| w == smime_capabilities));
        let (_, verdict, _) = open_with(&signed, &Certificates::default(), &test_authority()).unwrap();
        assert_eq!(verdict.signature, Signature::Good("bo@example.se".into()));
    }

    fn crl_check(files: &[&str]) -> CrlCheck {
        let dir = format!("{}/tests/fixtures/smime", env!("CARGO_MANIFEST_DIR"));
        CrlCheck { files: files.iter().map(|f| std::path::Path::new(&dir).join(f)).collect() }
    }

    /// RFC 5280 §5: with a supplied list revoking Bo's certificate, his
    /// signature is revoked, and the message with it; Ana's beside it, on
    /// a list that revokes nothing, stays good.
    #[test]
    fn a_revoked_signer_is_found_with_supplied_crls() {
        let none = Certificates::default();
        let mut builder = X509StoreBuilder::new().unwrap();
        builder.add_cert(X509::from_pem(&fixture("ca.pem")).unwrap()).unwrap();
        let revoking = crl_check(&["crl-bo-revoked.pem"]).store(builder).unwrap();
        let (_, verdict, _) =
            open_checked(&fixture("two-signers.eml"), &none, &test_authority(), Some(&revoking)).unwrap();
        assert_eq!(
            results(&verdict),
            [("ana@example.org", SignerResult::Good), ("bo@example.se", SignerResult::Revoked)]
        );
        assert_eq!(verdict.signature, Signature::Revoked("bo@example.se".into()));
        assert!(!verdict.signature.good());
        let mut builder = X509StoreBuilder::new().unwrap();
        builder.add_cert(X509::from_pem(&fixture("ca.pem")).unwrap()).unwrap();
        let clean = crl_check(&["crl-empty.pem"]).store(builder).unwrap();
        let (_, verdict, _) =
            open_checked(&fixture("two-signers.eml"), &none, &test_authority(), Some(&clean)).unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        // Changed content is still bad, revoked or not.
        let changed = String::from_utf8(fixture("two-signers.eml")).unwrap().replace("noon", "nine");
        let (_, verdict, _) =
            open_checked(changed.as_bytes(), &none, &test_authority(), Some(&revoking)).unwrap();
        assert_eq!(verdict.signature, Signature::Bad);
        // A revoked signer's certificate isn't kept.
        let bo = X509::from_pem(&fixture("bo.pem")).unwrap();
        let mut kept = Certificates::default();
        assert!(!learn(&mut kept, &bo, &Signature::Revoked("bo@example.se".into()), "bo@example.se").unwrap());
    }

    /// With no list for its issuer, a certificate isn't called revoked,
    /// and a list that isn't one is refused.
    #[test]
    fn crl_checks_need_a_list_for_the_issuer() {
        let mut builder = X509StoreBuilder::new().unwrap();
        builder.add_cert(X509::from_pem(&fixture("ca.pem")).unwrap()).unwrap();
        let no_lists = CrlCheck::default().store(builder).unwrap();
        let (_, verdict, _) = open_checked(
            &fixture("two-signers.eml"),
            &Certificates::default(),
            &test_authority(),
            Some(&no_lists),
        )
        .unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        assert!(crl_check(&["ana.pem"]).store(X509StoreBuilder::new().unwrap()).is_err());
        assert!(crl_check(&["missing.pem"]).store(X509StoreBuilder::new().unwrap()).is_err());
    }

    /// The public hook, with the system's authorities: the fixtures' test
    /// authority isn't one, so Ana's signature is unvouched, not revoked.
    #[test]
    fn open_checking_revocation_reads_mail_with_the_system_authorities() {
        let (_, verdict, _) = open_checking_revocation(
            &fixture("two-signers.eml"),
            &Certificates::default(),
            &crl_check(&["crl-bo-revoked.pem"]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(verdict.signature, Signature::Untrusted("ana@example.org".into()));
    }

    const WITH_BCC: &[u8] = b"From: Ana <ana@example.org>\r\nTo: bo@example.se\r\nCc: cy@example.com\r\nBcc: dee@example.net\r\nReply-To: ana@home.example\r\nSubject: Plans\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <plans@example.org>\r\nIn-Reply-To: <earlier@example.org>\r\nReferences: <earlier@example.org>\r\nX-Mailer: Picacho\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.\r\n";

    fn names(head: &[u8]) -> Vec<String> {
        header_fields(&String::from_utf8_lossy(head)).into_iter().map(|(n, _)| n).collect()
    }

    /// RFC 9788 §5.2.1: signed only, the inner part carries the nine
    /// user-facing fields, never Bcc, and hp="clear", with no HP-Outer.
    #[test]
    fn signed_part_carries_the_rfc_9788_headers_without_bcc() {
        let (part, outer) = split(WITH_BCC);
        let inner = protected_part(&part, &outer, false);
        let text = String::from_utf8_lossy(&inner);
        let head = &text[..text.find("\r\n\r\n").unwrap() + 2];
        assert!(head.starts_with("Content-Type: text/plain; charset=utf-8; hp=\"clear\"\r\n"), "{head}");
        assert_eq!(
            names(head.as_bytes()),
            ["content-type", "from", "to", "cc", "reply-to", "subject", "date", "message-id", "in-reply-to", "references"]
        );
        assert!(!head.to_lowercase().contains("bcc") && !head.contains("HP-Outer") && !head.contains("X-Mailer"));
        assert!(text.ends_with("\r\n\r\nMeet at noon.\r\n"));
    }

    /// RFC 9788 §5.2.1 step 5 and §2.2: encrypted, hp="cipher", and each
    /// non-structural field left outside is recorded as HP-Outer; Bcc in
    /// neither.
    #[test]
    fn encrypted_part_records_outer_fields_as_hp_outer() {
        let (part, outer) = split(WITH_BCC);
        let inner = protected_part(&part, &outer, true);
        let text = String::from_utf8_lossy(&inner);
        let head = &text[..text.find("\r\n\r\n").unwrap() + 2];
        assert!(head.contains("; hp=\"cipher\"\r\n"));
        assert!(head.contains("HP-Outer: Subject: Plans\r\n") && head.contains("HP-Outer: X-Mailer: Picacho\r\n"));
        assert!(!head.to_lowercase().contains("bcc"), "{head}");
        assert!(!head.contains("HP-Outer: MIME-Version") && !head.contains("HP-Outer: Content-Type"));
        // A part without a Content-Type gets the default one, marked.
        let bare = protected_part(b"\r\nHi.", b"Subject: Hi\r\n", true);
        assert!(String::from_utf8_lossy(&bare).starts_with("Content-Type: text/plain; charset=us-ascii; hp=\"cipher\"\r\n"));
    }

    /// Signed-only S/MIME with protected headers verifies, as the inner
    /// headers are under the signature; on read, the inner fields are the
    /// message's (once each) and HP-Outer isn't shown.
    #[test]
    fn protected_headers_round_trip_signed_and_encrypted() {
        let ana = fixture_identity("ana");
        let mut bo = fixture_identity("bo");
        bo.others.push(String::from_utf8(fixture("ana.pem")).unwrap());
        let signed = protect_with(&ana, WITH_BCC, "ana@example.org", &[], true, false).unwrap();
        assert!(openssl_cli_verifies("cms", &signed));
        let (readable, verdict, _) = open_with(&signed, &bo, &test_authority()).unwrap();
        assert_eq!(verdict.signature, Signature::Good("ana@example.org".into()));
        let head = &readable[..readable.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 2];
        let mut seen = names(head);
        assert_eq!(seen.iter().filter(|n| *n == "subject").count(), 1, "{}", String::from_utf8_lossy(head));
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), names(head).len(), "no field twice");
        // Encrypted to Ana's own (RSA) certificate: Pkcs7::encrypt, which
        // sending uses, can't encrypt to Bo's EC one.
        let encrypted = protect_with(&ana, WITH_BCC, "ana@example.org", &[], true, true).unwrap();
        let (readable, verdict, _) = open_with(&encrypted, &ana, &test_authority()).unwrap();
        assert!(verdict.decrypted);
        let text = String::from_utf8_lossy(&readable);
        assert!(!text.contains("HP-Outer") && !text.contains("hp=\"cipher\"; hp"), "{text}");
        let parsed = mailparse::parse_mail(&readable).unwrap();
        assert_eq!(parsed.get_body().unwrap().trim(), "Meet at noon.");
    }

    /// RFC 9788 §4.4.2, as for OpenPGP: a protected From for another
    /// address than the outer one warns, unless a signer whose signature
    /// checks out has a certificate for it (§4.4.1.2). The outer From stays
    /// the one shown.
    #[test]
    fn protected_from_mismatch_warns_without_a_signer_for_it() {
        let ana = fixture_identity("ana");
        let mut bo = fixture_identity("bo");
        bo.others.push(String::from_utf8(fixture("ana.pem")).unwrap());
        let rewrite = |raw: &[u8]| -> Vec<u8> {
            // Only the outer header section: the signed part stays as it was.
            let text = String::from_utf8_lossy(raw).into_owned();
            let at = text.find("\r\n\r\n").unwrap();
            let (head, body) = text.split_at(at);
            (head.replacen("From: Ana <ana@example.org>", "From: Eve <eve@example.net>", 1) + body).into_bytes()
        };
        // Signed by Ana, protected From Ana, outer From rewritten to Eve:
        // Ana's certificate vouches for the protected From, so no warning.
        let signed = protect_with(&ana, WITH_BCC, "ana@example.org", &[], true, false).unwrap();
        let (_, verdict, _) = open_with(&rewrite(&signed), &bo, &test_authority()).unwrap();
        assert!(verdict.from_mismatch.is_none(), "{verdict:?}");
        // A protected From for someone no signer is: a warning, naming
        // both, and the line isn't good.
        let forged = WITH_BCC.iter().copied().collect::<Vec<u8>>();
        let forged = String::from_utf8(forged).unwrap().replacen(
            "From: Ana <ana@example.org>",
            "From: Bank <alerts@bank.example>",
            1,
        );
        let signed = protect_with(&ana, forged.as_bytes(), "ana@example.org", &[], true, false).unwrap();
        let outer_fixed = String::from_utf8_lossy(&signed).replacen(
            "From: Bank <alerts@bank.example>",
            "From: Ana <ana@example.org>",
            1,
        );
        let (readable, verdict, _) = open_with(outer_fixed.as_bytes(), &bo, &test_authority()).unwrap();
        let warning = verdict.from_mismatch.clone().expect("warned");
        assert!(warning.protected.contains("alerts@bank.example"), "{warning:?}");
        assert!(warning.outer.contains("ana@example.org"), "{warning:?}");
        let (line, good) = verdict.signature_line().unwrap();
        assert!(!good && line.contains("(S/MIME)"), "{line}");
        let head = String::from_utf8_lossy(&readable);
        assert!(head.contains("From: Ana <ana@example.org>") && !head.contains("bank.example>\r\nTo"), "{head}");
    }

    /// RFC 9788 §4.2 on read: a protected Subject replaces a hidden outer
    /// one, but an inner From never replaces the outer From the signature
    /// was checked against; an unprotected payload changes nothing.
    #[test]
    fn inner_headers_replace_outer_ones_except_from() {
        let outer = b"From: eve@example.net\r\nSubject: ...\r\nX-Mailer: Other\r\n".to_vec();
        let mut payload = b"Content-Type: text/plain; hp=\"cipher\"\r\nFrom: Bank <alerts@bank.example>\r\nSubject: Real subject\r\nHP-Outer: Subject: ...\r\n\r\nBody\r\n".to_vec();
        let kept = take_protected_headers(outer.clone(), &mut payload);
        let kept = String::from_utf8(kept).unwrap();
        assert!(kept.contains("From: eve@example.net\r\n") && !kept.contains("bank"), "{kept}");
        assert!(kept.contains("Subject: Real subject\r\n") && !kept.contains("Subject: ...\r\n"), "{kept}");
        assert!(kept.contains("X-Mailer: Other\r\n"));
        assert_eq!(payload, b"Content-Type: text/plain; hp=\"cipher\"\r\n\r\nBody\r\n");
        let mut plain = b"Content-Type: text/plain\r\nSubject: Inner\r\n\r\nBody\r\n".to_vec();
        let before = plain.clone();
        assert_eq!(take_protected_headers(outer.clone(), &mut plain), outer);
        assert_eq!(plain, before);
    }
}
