//! S/MIME identities on PKCS#11 tokens (a YubiKey's PIV applet through
//! ykcs11 or OpenSC, a smart card, an HSM): the certificate is read from the
//! token and kept; the private key stays on it, and signs and decrypts
//! there with the token's PIN (`manage/hardware.rs`).
//!
//! Signing builds the SignedData here (as `sign_rsa` does) and asks the
//! token only for the signature over the signed attributes. Decrypting
//! takes the EnvelopedData or AuthEnvelopedData apart here: the token
//! unwraps the content-encryption key (RSA, RFC 5652 §6.2.1 and RFC 3560's
//! OAEP; or the ECDH secret of RFC 5753, then X9.63's KDF and AES key wrap
//! here), and the content is decrypted with it (AES-CBC, AES-GCM, or triple
//! DES for old mail).
use super::der;
use anyhow::{Context, Result, bail, ensure};
use openssl::x509::X509;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock, Mutex};

/// An identity whose key is on a token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenIdentity {
    /// The PKCS#11 module that reaches the token (a library's path).
    pub module: String,
    /// The token's serial number and label, as it gives them.
    pub serial: String,
    pub label: String,
    /// The private key's CKA_ID, hex.
    pub key_id: String,
    /// The certificate (first) and any others of its chain, PEM.
    pub chain: String,
}

impl TokenIdentity {
    /// The PIN's id (`manage/hardware.rs`): one per token.
    pub fn pin_id(&self) -> String {
        format!("pkcs11:{}#{}", self.module, self.serial)
    }
    pub fn certificate(&self) -> Option<X509> {
        X509::stack_from_pem(self.chain.as_bytes()).ok()?.into_iter().next()
    }
    pub fn others(&self) -> Vec<X509> {
        X509::stack_from_pem(self.chain.as_bytes()).map(|c| c.into_iter().skip(1).collect()).unwrap_or_default()
    }
    pub fn addresses(&self) -> Vec<String> {
        self.certificate().map(|c| super::emails(&c)).unwrap_or_default()
    }
}

/// Where PKCS#11 modules usually are, of those that exist here: OpenSC's
/// and Yubico's ykcs11.
pub fn suggested_modules() -> Vec<String> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Library/OpenSC/lib/opensc-pkcs11.so",
            "/usr/local/lib/opensc-pkcs11.so",
            "/opt/homebrew/lib/opensc-pkcs11.so",
            "/usr/local/lib/libykcs11.dylib",
            "/opt/homebrew/lib/libykcs11.dylib",
        ]
    } else if cfg!(windows) {
        &[
            "C:\\Windows\\System32\\opensc-pkcs11.dll",
            "C:\\Program Files\\OpenSC Project\\OpenSC\\pkcs11\\opensc-pkcs11.dll",
            "C:\\Program Files\\Yubico\\Yubico PIV Tool\\bin\\libykcs11.dll",
        ]
    } else {
        &[
            "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
            "/usr/lib/aarch64-linux-gnu/opensc-pkcs11.so",
            "/usr/lib64/opensc-pkcs11.so",
            "/usr/lib/opensc-pkcs11.so",
            "/usr/lib/pkcs11/opensc-pkcs11.so",
            "/usr/lib/x86_64-linux-gnu/libykcs11.so.2",
            "/usr/lib/x86_64-linux-gnu/libykcs11.so",
            "/usr/lib64/libykcs11.so.2",
            "/usr/lib/libykcs11.so",
            "/usr/local/lib/libykcs11.so",
        ]
    };
    candidates
        .iter()
        .filter(|p| std::path::Path::new(p).is_file())
        .map(|p| (*p).to_owned())
        .collect()
}

/// What a token's private key does for S/MIME. Implemented for tokens
/// (`TokenKey`) and, in tests, for software keys.
pub(super) trait PrivateOps {
    /// The CMS signature value over `data` (SHA-256): PKCS#1 v1.5 for RSA,
    /// a DER ECDSA-Sig-Value for EC.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;
    /// RSA: the key `encrypted` holds, PKCS#1 v1.5, or OAEP with this hash.
    fn rsa_decrypt(&self, encrypted: &[u8], oaep: Option<Hash>) -> Result<Vec<u8>>;
    /// EC: the shared secret with the other party's public point.
    fn ecdh(&self, point: &[u8]) -> Result<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}
impl Hash {
    fn digest(self) -> openssl::hash::MessageDigest {
        use openssl::hash::MessageDigest;
        match self {
            Hash::Sha1 => MessageDigest::sha1(),
            Hash::Sha256 => MessageDigest::sha256(),
            Hash::Sha384 => MessageDigest::sha384(),
            Hash::Sha512 => MessageDigest::sha512(),
        }
    }
}

// --- PKCS#11 ------------------------------------------------------------------

/// Modules loaded and initialized, by path: PKCS#11 lets a process
/// initialize a module once.
static MODULES: LazyLock<Mutex<std::collections::HashMap<String, Arc<cryptoki::context::Pkcs11>>>> =
    LazyLock::new(Default::default);

fn module(path: &str) -> Result<Arc<cryptoki::context::Pkcs11>> {
    use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
    let mut modules = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = modules.get(path) {
        return Ok(m.clone());
    }
    ensure!(std::path::Path::new(path).is_file(), t!("security-keys-no-module-there", path = path.to_owned()));
    let pkcs11 = Pkcs11::new(path).map_err(|e| anyhow::anyhow!(t!("security-keys-module-wont-load", error = e.to_string())))?;
    match pkcs11.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => bail!(t!("security-keys-module-wont-load", error = e.to_string())),
    }
    let pkcs11 = Arc::new(pkcs11);
    modules.insert(path.to_owned(), pkcs11.clone());
    Ok(pkcs11)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).filter_map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()).collect()
}

/// A session with the token `serial` behind `module`, signed in with `pin`.
fn session(module_path: &str, serial: &str, pin: &str) -> Result<cryptoki::session::Session> {
    use cryptoki::session::UserType;
    let pkcs11 = module(module_path)?;
    let slot = pkcs11
        .get_slots_with_token()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .find(|s| pkcs11.get_token_info(*s).is_ok_and(|i| i.serial_number().trim() == serial))
        .with_context(|| t!("security-keys-insert-the-token"))?;
    let session = pkcs11.open_ro_session(slot).map_err(|e| anyhow::anyhow!("{e}"))?;
    match session.login(UserType::User, Some(&cryptoki::types::AuthPin::new(pin.to_owned().into()))) {
        Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::UserAlreadyLoggedIn, _)) => Ok(session),
        Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::PinIncorrect, _)) => {
            bail!(t!("security-keys-the-token-refused-the-pin"))
        }
        Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::PinLocked, _)) => {
            bail!(t!("security-keys-the-pin-is-blocked"))
        }
        Err(e) => bail!("{e}"),
    }
}

/// The identities on the tokens behind `module_path`: each certificate
/// for an email address with a private key beside it (the same CKA_ID).
/// Signing in with `pin` checks it, and it is kept for each token.
pub fn discover(module_path: &str, pin: &str) -> Result<Vec<TokenIdentity>> {
    use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass};
    ensure!(!pin.trim().is_empty(), t!("security-keys-enter-the-pin"));
    let pkcs11 = module(module_path)?;
    let slots = pkcs11.get_slots_with_token().map_err(|e| anyhow::anyhow!("{e}"))?;
    ensure!(!slots.is_empty(), t!("security-keys-insert-the-token"));
    let mut out = Vec::new();
    for slot in slots {
        let Ok(info) = pkcs11.get_token_info(slot) else { continue };
        let serial = info.serial_number().trim().to_owned();
        let label = info.label().trim().to_owned();
        let session = session(module_path, &serial, pin)?;
        let certificates = session
            .find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut all: Vec<(Vec<u8>, X509)> = Vec::new();
        for handle in certificates {
            let Ok(attributes) = session.get_attributes(handle, &[AttributeType::Id, AttributeType::Value]) else { continue };
            let (mut id, mut value) = (Vec::new(), Vec::new());
            for a in attributes {
                match a {
                    Attribute::Id(v) => id = v,
                    Attribute::Value(v) => value = v,
                    _ => {}
                }
            }
            if let Ok(cert) = X509::from_der(&value) {
                all.push((id, cert));
            }
        }
        for (id, cert) in &all {
            if super::emails(cert).is_empty() || id.is_empty() {
                continue;
            }
            let keys = session
                .find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(id.clone())])
                .unwrap_or_default();
            if keys.is_empty() {
                continue;
            }
            // The token's other certificates that issued this one.
            let mut chain = String::from_utf8(cert.to_pem()?)?;
            for (_, other) in &all {
                if other.subject_name().to_der().ok() == cert.issuer_name().to_der().ok() && other.to_der().ok() != cert.to_der().ok() {
                    chain.push_str(&String::from_utf8(other.to_pem()?)?);
                }
            }
            out.push(TokenIdentity {
                module: module_path.to_owned(),
                serial: serial.clone(),
                label: if label.is_empty() { serial.clone() } else { label.clone() },
                key_id: hex(id),
                chain,
            });
        }
    }
    ensure!(!out.is_empty(), t!("security-keys-no-identity-on-the-token"));
    for identity in &out {
        crate::manage::hardware::remember_pin(&identity.pin_id(), pin);
    }
    Ok(out)
}

/// Checks the token's PIN and keeps it.
pub fn unlock(identity: &TokenIdentity, pin: &str) -> Result<()> {
    ensure!(!pin.trim().is_empty(), t!("security-keys-enter-the-pin"));
    if let Err(e) = session(&identity.module, &identity.serial, pin) {
        crate::manage::hardware::forget_pin(&identity.pin_id());
        return Err(e);
    }
    crate::manage::hardware::remember_pin(&identity.pin_id(), pin);
    Ok(())
}

/// A token's key, with its kept PIN.
pub(super) struct TokenKey<'a> {
    identity: &'a TokenIdentity,
    pin: zeroize::Zeroizing<String>,
    rsa: bool,
}
impl<'a> TokenKey<'a> {
    /// The key of `identity`, or `PinNeeded` when its PIN isn't kept.
    pub fn open(identity: &'a TokenIdentity) -> Result<Self> {
        let pin = crate::manage::hardware::pin(&identity.pin_id()).ok_or_else(|| crate::manage::hardware::PinNeeded {
            key: identity.pin_id(),
            name: identity.label.clone(),
        })?;
        let rsa = identity
            .certificate()
            .and_then(|c| c.public_key().ok())
            .is_some_and(|k| k.rsa().is_ok());
        Ok(Self { identity, pin, rsa })
    }
    fn with_key<T>(&self, work: impl FnOnce(&cryptoki::session::Session, cryptoki::object::ObjectHandle) -> Result<T>) -> Result<T> {
        use cryptoki::object::{Attribute, ObjectClass};
        let session = session(&self.identity.module, &self.identity.serial, &self.pin)?;
        let key = session
            .find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(unhex(&self.identity.key_id))])
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .into_iter()
            .next()
            .with_context(|| t!("security-keys-the-key-isnt-on-the-token"))?;
        work(&session, key)
    }
}
impl PrivateOps for TokenKey<'_> {
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        use cryptoki::mechanism::Mechanism;
        if self.rsa {
            return self.with_key(|s, key| s.sign(&Mechanism::Sha256RsaPkcs, key, data).map_err(|e| anyhow::anyhow!("{e}")));
        }
        // CKM_ECDSA signs a digest, and answers r and s side by side.
        let digest = openssl::hash::hash(openssl::hash::MessageDigest::sha256(), data)?;
        let raw = self.with_key(|s, key| s.sign(&Mechanism::Ecdsa, key, &digest).map_err(|e| anyhow::anyhow!("{e}")))?;
        ecdsa_der(&raw)
    }
    fn rsa_decrypt(&self, encrypted: &[u8], oaep: Option<Hash>) -> Result<Vec<u8>> {
        use cryptoki::mechanism::{Mechanism, MechanismType, rsa::{PkcsMgfType, PkcsOaepParams, PkcsOaepSource}};
        self.with_key(|s, key| {
            let mechanism = match oaep {
                None => Mechanism::RsaPkcs,
                Some(hash) => {
                    let (alg, mgf) = match hash {
                        Hash::Sha1 => (MechanismType::SHA1, PkcsMgfType::MGF1_SHA1),
                        Hash::Sha256 => (MechanismType::SHA256, PkcsMgfType::MGF1_SHA256),
                        Hash::Sha384 => (MechanismType::SHA384, PkcsMgfType::MGF1_SHA384),
                        Hash::Sha512 => (MechanismType::SHA512, PkcsMgfType::MGF1_SHA512),
                    };
                    Mechanism::RsaPkcsOaep(PkcsOaepParams::new(alg, mgf, PkcsOaepSource::empty()))
                }
            };
            s.decrypt(&mechanism, key, encrypted).map_err(|e| anyhow::anyhow!("{e}"))
        })
    }
    fn ecdh(&self, point: &[u8]) -> Result<Vec<u8>> {
        use cryptoki::mechanism::{Mechanism, elliptic_curve::{EcKdf, Ecdh1DeriveParams}};
        use cryptoki::object::{Attribute, AttributeType, KeyType, ObjectClass};
        let size = self
            .identity
            .certificate()
            .and_then(|c| c.public_key().ok())
            .and_then(|k| k.ec_key().ok())
            .map(|k| k.group().degree().div_ceil(8) as usize)
            .context("not an EC key")?;
        let size = cryptoki::types::Ulong::try_from(size).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.with_key(|s, key| {
            let params = Ecdh1DeriveParams::new(EcKdf::null(), point);
            let secret = s
                .derive_key(
                    &Mechanism::Ecdh1Derive(params),
                    key,
                    &[
                        Attribute::Class(ObjectClass::SECRET_KEY),
                        Attribute::KeyType(KeyType::GENERIC_SECRET),
                        Attribute::ValueLen(size),
                        Attribute::Token(false),
                        Attribute::Sensitive(false),
                        Attribute::Extractable(true),
                    ],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let value = s.get_attributes(secret, &[AttributeType::Value]).map_err(|e| anyhow::anyhow!("{e}"))?;
            let _ = s.destroy_object(secret);
            value
                .into_iter()
                .find_map(|a| match a {
                    Attribute::Value(v) => Some(v),
                    _ => None,
                })
                .context("no shared secret")
        })
    }
}

/// An ECDSA signature as r‖s (PKCS#11, the same length each) written as
/// RFC 3279's ECDSA-Sig-Value.
fn ecdsa_der(raw: &[u8]) -> Result<Vec<u8>> {
    ensure!(!raw.is_empty() && raw.len().is_multiple_of(2), "an ECDSA signature of odd length");
    let (r, s) = raw.split_at(raw.len() / 2);
    Ok(der::encode(der::SEQUENCE, &[&der::unsigned_integer(r), &der::unsigned_integer(s)]))
}

// --- CMS: decrypting -----------------------------------------------------------

/// An OBJECT IDENTIFIER's content octets as dotted text.
fn dotted(content: &[u8]) -> String {
    let mut parts: Vec<u64> = Vec::new();
    let mut value = 0u64;
    for (i, &b) in content.iter().enumerate() {
        value = (value << 7) | u64::from(b & 0x7f);
        if b & 0x80 == 0 {
            if parts.is_empty() && i < 9 {
                let first = (value / 40).min(2);
                parts.push(first);
                parts.push(value - first * 40);
            } else {
                parts.push(value);
            }
            value = 0;
        }
    }
    parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

const ENVELOPED_DATA: &str = "1.2.840.113549.1.7.3";
const AUTH_ENVELOPED_DATA: &str = "1.2.840.113549.1.9.16.1.23";

/// Decrypts a DER EnvelopedData or AuthEnvelopedData encrypted to `cert`
/// with the private key `ops` uses.
pub(super) fn decrypt(der_input: &[u8], cert: &X509, ops: &dyn PrivateOps) -> Result<Vec<u8>> {
    let bad = |e: der::Error| anyhow::anyhow!("{e}");
    let (oid, content) = der::content_info(der_input).map_err(bad)?;
    let kind = dotted(oid);
    let auth = match kind.as_str() {
        ENVELOPED_DATA => false,
        AUTH_ENVELOPED_DATA => true,
        _ => bail!("not encrypted CMS content"),
    };
    let mut fields = content.children().map_err(bad)?;
    fields.read_tag(der::INTEGER).map_err(bad)?;
    fields.optional(der::CONTEXT_0).map_err(bad)?; // originatorInfo
    let recipients = fields.read_tag(der::SET).map_err(bad)?;
    let encrypted = fields.read_tag(der::SEQUENCE).map_err(bad)?;
    let cert_der = cert.to_der()?;
    let key = content_key(&recipients, &cert_der, cert, ops)?;
    // EncryptedContentInfo: contentType, contentEncryptionAlgorithm and
    // the [0] IMPLICIT encryptedContent.
    let mut info = encrypted.children().map_err(bad)?;
    info.read_tag(der::OID).map_err(bad)?;
    let algorithm = info.read_tag(der::SEQUENCE).map_err(bad)?;
    let ciphertext = match info.read() {
        Ok(t) if t.tag == 0x80 => t.value.to_vec(),
        Ok(t) if t.tag == 0xa0 => {
            let mut chunks = t.children().map_err(bad)?;
            let mut all = Vec::new();
            while !chunks.is_empty() {
                all.extend_from_slice(chunks.read_tag(der::OCTET_STRING).map_err(bad)?.value);
            }
            all
        }
        _ => bail!("no encrypted content"),
    };
    let mut algorithm = algorithm.children().map_err(bad)?;
    let cipher_oid = dotted(algorithm.read_tag(der::OID).map_err(bad)?.value);
    let params = algorithm.read().ok();
    if auth {
        // authAttrs [1] (their DER as a SET is the additional data), then the tag.
        let attributes = fields.optional(0xa1).map_err(bad)?;
        let mac = fields.read_tag(der::OCTET_STRING).map_err(bad)?.value;
        let aad = attributes.map(|a| der::encode(der::SET, &[a.value])).unwrap_or_default();
        let params = params.context("GCM without its parameters")?;
        let mut gcm = params.children().map_err(bad)?;
        let nonce = gcm.read_tag(der::OCTET_STRING).map_err(bad)?.value;
        let cipher = match cipher_oid.as_str() {
            "2.16.840.1.101.3.4.1.6" => openssl::symm::Cipher::aes_128_gcm(),
            "2.16.840.1.101.3.4.1.26" => openssl::symm::Cipher::aes_192_gcm(),
            "2.16.840.1.101.3.4.1.46" => openssl::symm::Cipher::aes_256_gcm(),
            other => bail!("unknown authenticated cipher {other}"),
        };
        return openssl::symm::decrypt_aead(cipher, &key, Some(nonce), &aad, &ciphertext, mac)
            .map_err(|_| anyhow::anyhow!("the message was changed, or isn't for this key"));
    }
    let iv = params.filter(|p| p.tag == der::OCTET_STRING).map(|p| p.value.to_vec());
    let cipher = match cipher_oid.as_str() {
        "2.16.840.1.101.3.4.1.2" => openssl::symm::Cipher::aes_128_cbc(),
        "2.16.840.1.101.3.4.1.22" => openssl::symm::Cipher::aes_192_cbc(),
        "2.16.840.1.101.3.4.1.42" => openssl::symm::Cipher::aes_256_cbc(),
        "1.2.840.113549.3.7" => openssl::symm::Cipher::des_ede3_cbc(),
        other => bail!("unknown cipher {other}"),
    };
    openssl::symm::decrypt(cipher, &key, iv.as_deref(), &ciphertext).map_err(|_| anyhow::anyhow!("the content couldn't be decrypted"))
}

/// Whether a recipient identifier names `cert`: its issuer and serial, or
/// its subject key identifier.
fn names(rid: &der::Tlv<'_>, cert_der: &[u8], cert: &X509) -> bool {
    match rid.tag {
        der::SEQUENCE => {
            let Ok((issuer, serial)) = der::issuer_and_serial(cert_der) else { return false };
            let Ok(mut fields) = rid.children() else { return false };
            let given_issuer = fields.read_tag(der::SEQUENCE).map(|t| t.raw);
            let given_serial = fields.read_tag(der::INTEGER).map(|t| t.value);
            given_issuer == Ok(issuer) && given_serial == Ok(serial)
        }
        0x80 => cert.subject_key_id().is_some_and(|id| id.as_slice() == rid.value),
        _ => false,
    }
}

/// The content-encryption key, from the recipient info for `cert`.
fn content_key(recipients: &der::Tlv<'_>, cert_der: &[u8], cert: &X509, ops: &dyn PrivateOps) -> Result<Vec<u8>> {
    let bad = |e: der::Error| anyhow::anyhow!("{e}");
    let mut all = recipients.children().map_err(bad)?;
    while !all.is_empty() {
        let info = all.read().map_err(bad)?;
        match info.tag {
            // KeyTransRecipientInfo (RFC 5652 §6.2.1).
            der::SEQUENCE => {
                let mut f = info.children().map_err(bad)?;
                f.read_tag(der::INTEGER).map_err(bad)?;
                let rid = f.read().map_err(bad)?;
                if !names(&rid, cert_der, cert) {
                    continue;
                }
                let mut algorithm = f.read_tag(der::SEQUENCE).map_err(bad)?.children().map_err(bad)?;
                let oid = dotted(algorithm.read_tag(der::OID).map_err(bad)?.value);
                let oaep = match oid.as_str() {
                    "1.2.840.113549.1.1.1" => None,
                    "1.2.840.113549.1.1.7" => Some(oaep_hash(algorithm.read().ok())),
                    other => bail!("unknown key transport {other}"),
                };
                let encrypted = f.read_tag(der::OCTET_STRING).map_err(bad)?.value;
                return ops.rsa_decrypt(encrypted, oaep);
            }
            // KeyAgreeRecipientInfo [1] (RFC 5652 §6.2.2, RFC 5753).
            0xa1 => {
                if let Some(key) = agreed_key(&info, cert_der, cert, ops)? {
                    return Ok(key);
                }
            }
            _ => {}
        }
    }
    bail!("the message isn't encrypted to this certificate")
}

/// RSAES-OAEP-params' hash (RFC 4055 §4.1: SHA-1 when left out). The MGF
/// is taken to use the same hash, as everyone writes it.
fn oaep_hash(params: Option<der::Tlv<'_>>) -> Hash {
    let Some(params) = params.filter(|p| p.tag == der::SEQUENCE) else { return Hash::Sha1 };
    let Ok(mut fields) = params.children() else { return Hash::Sha1 };
    let Ok(Some(explicit)) = fields.optional(der::CONTEXT_0) else { return Hash::Sha1 };
    let oid = explicit
        .children()
        .and_then(|mut c| c.read_tag(der::SEQUENCE))
        .and_then(|a| a.children())
        .and_then(|mut a| a.read_tag(der::OID))
        .map(|o| dotted(o.value))
        .unwrap_or_default();
    hash_of(&oid).unwrap_or(Hash::Sha1)
}

fn hash_of(oid: &str) -> Option<Hash> {
    Some(match oid {
        "1.3.14.3.2.26" => Hash::Sha1,
        "2.16.840.1.101.3.4.2.1" => Hash::Sha256,
        "2.16.840.1.101.3.4.2.2" => Hash::Sha384,
        "2.16.840.1.101.3.4.2.3" => Hash::Sha512,
        _ => return None,
    })
}

/// RFC 5753 §3.1.2: the content key of a KeyAgreeRecipientInfo for `cert`,
/// if it has one for it.
fn agreed_key(info: &der::Tlv<'_>, cert_der: &[u8], cert: &X509, ops: &dyn PrivateOps) -> Result<Option<Vec<u8>>> {
    let bad = |e: der::Error| anyhow::anyhow!("{e}");
    let mut f = info.children().map_err(bad)?;
    f.read_tag(der::INTEGER).map_err(bad)?;
    // originator [0] EXPLICIT: the ephemeral key, [1] OriginatorPublicKey.
    let originator = f.read_tag(der::CONTEXT_0).map_err(bad)?;
    let mut o = originator.children().map_err(bad)?;
    let public = o.read_tag(0xa1).map_err(bad)?;
    let mut p = public.children().map_err(bad)?;
    p.read_tag(der::SEQUENCE).map_err(bad)?;
    let bits = p.read_tag(0x03).map_err(bad)?.value;
    let point = bits.get(1..).context("an empty public key")?;
    let ukm = f.optional(der::CONTEXT_0 + 1).map_err(bad)?.and_then(|u| u.children().ok()?.read_tag(der::OCTET_STRING).ok().map(|o| o.value));
    let mut algorithm = f.read_tag(der::SEQUENCE).map_err(bad)?.children().map_err(bad)?;
    let scheme = dotted(algorithm.read_tag(der::OID).map_err(bad)?.value);
    let hash = match scheme.as_str() {
        "1.3.133.16.840.63.0.2" => Hash::Sha1,
        "1.3.132.1.11.1" => Hash::Sha256,
        "1.3.132.1.11.2" => Hash::Sha384,
        "1.3.132.1.11.3" => Hash::Sha512,
        other => bail!("unknown key agreement {other}"),
    };
    let wrap = algorithm.read_tag(der::SEQUENCE).map_err(bad)?;
    let wrap_oid = wrap.children().map_err(bad)?.read_tag(der::OID).map_err(bad)?;
    let key_bits: u32 = match dotted(wrap_oid.value).as_str() {
        "2.16.840.1.101.3.4.1.5" => 128,
        "2.16.840.1.101.3.4.1.25" => 192,
        "2.16.840.1.101.3.4.1.45" => 256,
        other => bail!("unknown key wrap {other}"),
    };
    let mut keys = f.read_tag(der::SEQUENCE).map_err(bad)?.children().map_err(bad)?;
    while !keys.is_empty() {
        let mut key = keys.read_tag(der::SEQUENCE).map_err(bad)?.children().map_err(bad)?;
        let rid = key.read().map_err(bad)?;
        // RecipientKeyIdentifier [0] holds the subject key identifier.
        let named = match rid.tag {
            0xa0 => rid
                .children()
                .and_then(|mut c| c.read_tag(der::OCTET_STRING))
                .is_ok_and(|id| cert.subject_key_id().is_some_and(|k| k.as_slice() == id.value)),
            _ => names(&rid, cert_der, cert),
        };
        if !named {
            continue;
        }
        let wrapped = key.read_tag(der::OCTET_STRING).map_err(bad)?.value;
        let z = zeroize::Zeroizing::new(ops.ecdh(point)?);
        // ECC-CMS-SharedInfo: the wrap algorithm (parameters absent for
        // AES), the UKM, and the key's length in bits.
        let key_info = der::encode(der::SEQUENCE, &[&der::oid(wrap_oid.value)]);
        let mut shared = vec![key_info];
        if let Some(ukm) = ukm {
            shared.push(der::encode(der::CONTEXT_0, &[&der::encode(der::OCTET_STRING, &[ukm])]));
        }
        shared.push(der::encode(0xa2, &[&der::encode(der::OCTET_STRING, &[&key_bits.to_be_bytes()])]));
        let parts: Vec<&[u8]> = shared.iter().map(Vec::as_slice).collect();
        let shared_info = der::encode(der::SEQUENCE, &parts);
        let kek = zeroize::Zeroizing::new(x963_kdf(hash, &z, &shared_info, key_bits as usize / 8)?);
        let aes = openssl::aes::AesKey::new_decrypt(&kek).map_err(|_| anyhow::anyhow!("a bad key-encryption key"))?;
        ensure!(wrapped.len() >= 24 && wrapped.len() % 8 == 0, "a wrapped key of the wrong length");
        let mut out = vec![0u8; wrapped.len() - 8];
        openssl::aes::unwrap_key(&aes, None, &mut out, wrapped).map_err(|_| anyhow::anyhow!("the key couldn't be unwrapped"))?;
        return Ok(Some(out));
    }
    Ok(None)
}

/// ANSI X9.63's KDF: hashes of Z, a counter from 1 and the shared info,
/// to `len` bytes.
fn x963_kdf(hash: Hash, z: &[u8], shared_info: &[u8], len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len + 64);
    let mut counter: u32 = 1;
    while out.len() < len {
        let mut h = openssl::hash::Hasher::new(hash.digest())?;
        h.update(z)?;
        h.update(&counter.to_be_bytes())?;
        h.update(shared_info)?;
        out.extend_from_slice(&h.finish()?);
        counter += 1;
    }
    out.truncate(len);
    Ok(out)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use openssl::pkey::{PKey, Private};

    /// A software key standing in for a token.
    pub struct Software(pub PKey<Private>);
    impl PrivateOps for Software {
        fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
            let mut signer = openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &self.0)?;
            Ok(signer.sign_oneshot_to_vec(data)?)
        }
        fn rsa_decrypt(&self, encrypted: &[u8], oaep: Option<Hash>) -> Result<Vec<u8>> {
            let mut d = openssl::encrypt::Decrypter::new(&self.0)?;
            match oaep {
                None => d.set_rsa_padding(openssl::rsa::Padding::PKCS1)?,
                Some(hash) => {
                    d.set_rsa_padding(openssl::rsa::Padding::PKCS1_OAEP)?;
                    d.set_rsa_oaep_md(hash.digest())?;
                    d.set_rsa_mgf1_md(hash.digest())?;
                }
            }
            let mut out = vec![0; d.decrypt_len(encrypted)?];
            let n = d.decrypt(encrypted, &mut out)?;
            out.truncate(n);
            Ok(out)
        }
        fn ecdh(&self, point: &[u8]) -> Result<Vec<u8>> {
            let ec = self.0.ec_key()?;
            let mut ctx = openssl::bn::BigNumContext::new()?;
            let peer = openssl::ec::EcPoint::from_bytes(ec.group(), point, &mut ctx)?;
            let peer = PKey::from_ec_key(openssl::ec::EcKey::from_public_key(ec.group(), &peer)?)?;
            let mut deriver = openssl::derive::Deriver::new(&self.0)?;
            deriver.set_peer(&peer)?;
            Ok(deriver.derive_to_vec()?)
        }
    }

    /// A self-signed certificate for `email` with `key`.
    pub fn certificate(email: &str, key: &PKey<Private>) -> X509 {
        use openssl::{asn1::Asn1Time, bn::BigNum, hash::MessageDigest, x509::{X509NameBuilder, extension::{SubjectAlternativeName, SubjectKeyIdentifier}}};
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", email).unwrap();
        let name = name.build();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_serial_number(&BigNum::from_u32(7).unwrap().to_asn1_integer().unwrap()).unwrap();
        b.set_subject_name(&name).unwrap();
        b.set_issuer_name(&name).unwrap();
        b.set_pubkey(key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        b.set_not_after(&Asn1Time::days_from_now(30).unwrap()).unwrap();
        let san = SubjectAlternativeName::new().email(email).build(&b.x509v3_context(None, None)).unwrap();
        b.append_extension(san).unwrap();
        let ski = SubjectKeyIdentifier::new().build(&b.x509v3_context(None, None)).unwrap();
        b.append_extension(ski).unwrap();
        b.sign(key, MessageDigest::sha256()).unwrap();
        b.build()
    }

    pub fn ec_key() -> PKey<Private> {
        let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap();
        PKey::from_ec_key(openssl::ec::EcKey::generate(&group).unwrap()).unwrap()
    }
    pub fn rsa_key() -> PKey<Private> {
        PKey::from_rsa(openssl::rsa::Rsa::generate(2048).unwrap()).unwrap()
    }

    fn encrypted_to(cert: &X509, cipher: openssl::symm::Cipher, content: &[u8]) -> Vec<u8> {
        let mut stack = openssl::stack::Stack::new().unwrap();
        stack.push(cert.clone()).unwrap();
        openssl::cms::CmsContentInfo::encrypt(&stack, content, cipher, openssl::cms::CMSOptions::BINARY)
            .unwrap()
            .to_der()
            .unwrap()
    }

    #[test]
    fn rsa_and_ec_recipients_decrypt_with_the_token_alone() {
        let content = b"Content-Type: text/plain\r\n\r\nHello from the token.\r\n";
        for key in [rsa_key(), ec_key()] {
            let cert = certificate("ana@example.org", &key);
            for cipher in [openssl::symm::Cipher::aes_256_cbc(), openssl::symm::Cipher::aes_128_cbc(), openssl::symm::Cipher::aes_256_gcm()] {
                let der = encrypted_to(&cert, cipher, content);
                let opened = decrypt(&der, &cert, &Software(key.clone())).unwrap();
                assert_eq!(opened, content);
            }
            // Not for another certificate.
            let other_key = rsa_key();
            let other = certificate("bo@example.org", &other_key);
            let der = encrypted_to(&other, openssl::symm::Cipher::aes_256_cbc(), content);
            assert!(decrypt(&der, &cert, &Software(key.clone())).is_err());
        }
    }

    #[test]
    fn mail_signed_on_a_token_checks_out() {
        for key in [rsa_key(), ec_key()] {
            let cert = certificate("ana@example.org", &key);
            let content = b"Content-Type: text/plain; charset=utf-8\r\n\r\nSigned on the key.\r\n";
            let signature = super::super::signed_data(&cert, &[], content, chrono::Utc::now(), |attributes| {
                Software(key.clone()).sign(attributes)
            })
            .unwrap();
            let mut raw = b"From: ana@example.org\r\nTo: bo@example.org\r\nSubject: Hi\r\nMIME-Version: 1.0\r\n".to_vec();
            raw.extend_from_slice(&super::super::multipart_signed(content, &signature));
            let (_, verdict, _) = super::super::open(&raw, &super::super::Certificates::default()).unwrap();
            // Self-signed: matches, though no authority vouches for it.
            assert_eq!(verdict.signature, super::super::Signature::Untrusted("ana@example.org".into()), "{verdict:?}");
        }
    }

    #[test]
    fn oids_read_as_dotted_text() {
        assert_eq!(dotted(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x03]), ENVELOPED_DATA);
        assert_eq!(dotted(&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2e]), "2.16.840.1.101.3.4.1.46");
        assert_eq!(dotted(&[0x2b, 0x81, 0x04, 0x01, 0x0b, 0x01]), "1.3.132.1.11.1");
    }

    #[test]
    fn ecdsa_signatures_are_written_as_der() {
        let raw = [[0x80u8; 32], [0x01u8; 32]].concat();
        let der = ecdsa_der(&raw).unwrap();
        assert_eq!(der[0], der::SEQUENCE);
        // r gains a leading zero: its high bit is set.
        assert_eq!(&der[2..5], &[der::INTEGER, 33, 0]);
        assert!(ecdsa_der(&[1, 2, 3]).is_err());
    }

    #[test]
    fn identities_name_their_token_and_addresses() {
        let key = rsa_key();
        let cert = certificate("ana@example.org", &key);
        let identity = TokenIdentity {
            module: "/usr/lib/opensc-pkcs11.so".into(),
            serial: "1234".into(),
            label: "PIV".into(),
            key_id: "01".into(),
            chain: String::from_utf8(cert.to_pem().unwrap()).unwrap(),
        };
        assert_eq!(identity.addresses(), ["ana@example.org"]);
        assert_eq!(identity.pin_id(), "pkcs11:/usr/lib/opensc-pkcs11.so#1234");
        assert!(matches!(TokenKey::open(&identity), Err(e) if e.downcast_ref::<crate::manage::hardware::PinNeeded>().is_some()));
        assert_eq!(unhex("0aff"), [0x0a, 0xff]);
        assert!(discover("/nonexistent/pkcs11.so", "1234").is_err());
    }
}
