//! RFC 8551 §2.5.2 and §2.7.1: the SMIMECapabilities a correspondent's
//! signed mail announces, remembered for their certificate, and the
//! content encryption chosen from them when writing to them.
use super::{Certificates, ContentCipher, Signer, SignerResult, Verdict, der, fingerprint, oids};
use openssl::x509::X509;
use serde::{Deserialize, Serialize};

/// What a certificate's holder last announced they can decrypt.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    /// The signer's certificate, by its SHA-256 fingerprint.
    pub fingerprint: String,
    /// The message's signingTime (RFC 5652 §11.3), in Unix seconds:
    /// RFC 8551 §2.7.1 keeps it so only newer mail replaces the list.
    pub signing_time: i64,
    /// The capabilities' OIDs, dotted, in the signer's order of
    /// preference (RFC 8551 §2.5.2).
    pub algorithms: Vec<String>,
}

/// How far ahead of now a signing time may be: clock skew, no more.
/// RFC 8551 §2.7.1: one further in the future MUST NOT be accepted.
const MAX_CLOCK_SKEW: i64 = 24 * 60 * 60;

/// An OID's dotted form, from its content octets (X.690 §8.19).
fn dotted(content: &[u8]) -> Option<String> {
    if content.last()? & 0x80 != 0 {
        return None;
    }
    let mut arcs: Vec<u64> = Vec::new();
    let mut value: u64 = 0;
    for &octet in content {
        if value > u64::MAX >> 7 {
            return None;
        }
        value = value << 7 | u64::from(octet & 0x7f);
        if octet & 0x80 == 0 {
            arcs.push(value);
            value = 0;
        }
    }
    // X.690 §8.19.4: the first subidentifier holds the first two arcs.
    let first = *arcs.first()?;
    let (a, b) = match first {
        0..40 => (0, first),
        40..80 => (1, first - 40),
        _ => (2, first - 80),
    };
    let mut out = format!("{a}.{b}");
    for arc in &arcs[1..] {
        out.push_str(&format!(".{arc}"));
    }
    Some(out)
}

/// RFC 5652 §11.3: a signingTime value, UTCTime (years 1950 to 2049, as
/// RFC 5280 §4.1.2.5.1 reads them) or GeneralizedTime, as Unix seconds.
fn signing_time(value: &der::Tlv<'_>) -> Option<i64> {
    let text = std::str::from_utf8(value.value).ok()?;
    let full = match value.tag {
        der::UTC_TIME => {
            let year: u32 = text.get(..2)?.parse().ok()?;
            format!("{}{text}", if year >= 50 { "19" } else { "20" })
        }
        der::GENERALIZED_TIME => text.to_owned(),
        _ => return None,
    };
    chrono::NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%SZ")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// A SignerInfo's signingTime and SMIMECapabilities (RFC 5652 §5.3
/// signedAttrs), where it has both.
fn announced_by(signer_info: &der::Tlv<'_>) -> Option<(i64, Vec<String>)> {
    let mut fields = signer_info.children().ok()?;
    fields.read_tag(der::INTEGER).ok()?;
    fields.read().ok()?;
    fields.read_tag(der::SEQUENCE).ok()?;
    let mut attributes = fields.optional(der::CONTEXT_0).ok()??.children().ok()?;
    let (mut time, mut algorithms) = (None, None);
    while !attributes.is_empty() {
        let mut attribute = attributes.read_tag(der::SEQUENCE).ok()?.children().ok()?;
        let kind = attribute.read_tag(der::OID).ok()?.value;
        let mut values = attribute.read_tag(der::SET).ok()?.children().ok()?;
        if kind == oids::SIGNING_TIME {
            time = signing_time(&values.read().ok()?);
        } else if kind == oids::SMIME_CAPABILITIES {
            // SMIMECapabilities ::= SEQUENCE OF SMIMECapability, each a
            // SEQUENCE of its OID and optional parameters.
            let mut each = values.read_tag(der::SEQUENCE).ok()?.children().ok()?;
            let mut found = Vec::new();
            while !each.is_empty() {
                let mut capability = each.read_tag(der::SEQUENCE).ok()?.children().ok()?;
                found.push(dotted(capability.read_tag(der::OID).ok()?.value)?);
            }
            algorithms = Some(found);
        }
    }
    Some((time?, algorithms?))
}

/// The capabilities each signer of a DER SignedData announced, for those
/// whose signature matches (RFC 8551 §2.7.1: a list in a message whose
/// signature couldn't be verified MUST NOT be accepted). `signers` is
/// `check_signers`'s result for it, in the same order as its SignerInfos.
pub(super) fn of_signers(signed_data: &[u8], signers: &[(Signer, Option<X509>)]) -> Vec<Capabilities> {
    let Ok(signed) = der::SignedData::parse(signed_data) else {
        return Vec::new();
    };
    signed
        .signer_infos
        .iter()
        .zip(signers)
        .filter_map(|(signer_info, (signer, cert))| {
            if !matches!(signer.result, SignerResult::Good | SignerResult::Untrusted) {
                return None;
            }
            let (signing_time, algorithms) = announced_by(signer_info)?;
            Some(Capabilities { fingerprint: fingerprint(cert.as_ref()?), signing_time, algorithms })
        })
        .collect()
}

/// RFC 8551 §2.7.1: keeps the capabilities a message's signers announced,
/// for certificates kept to write to (`learn`'s), where there is no list
/// yet or the message is newer than the one it came from. Returns whether
/// anything changed.
pub fn learn_capabilities(certs: &mut Certificates, verdict: &Verdict) -> bool {
    record(certs, &verdict.capabilities, chrono::Utc::now().timestamp())
}

fn record(certs: &mut Certificates, announced: &[Capabilities], now: i64) -> bool {
    let kept: Vec<String> = certs.others().iter().map(fingerprint).collect();
    let mut changed = false;
    for found in announced {
        if !kept.contains(&found.fingerprint) || found.signing_time > now + MAX_CLOCK_SKEW {
            continue;
        }
        match certs.capabilities.iter_mut().find(|c| c.fingerprint == found.fingerprint) {
            Some(known) if found.signing_time > known.signing_time => *known = found.clone(),
            Some(_) => continue,
            None => certs.capabilities.push(found.clone()),
        }
        changed = true;
    }
    // Those of certificates no longer kept go with them.
    let before = certs.capabilities.len();
    certs.capabilities.retain(|c| kept.contains(&c.fingerprint));
    changed || certs.capabilities.len() != before
}

/// RFC 8551 §2.7.1: AES-256 GCM, as AuthEnvelopedData, where every
/// recipient's kept capabilities list it ahead of AES-256 CBC (rule 1, in
/// their order of preference); otherwise AES-256 CBC, which S/MIME 3.2
/// clients that don't know AuthEnvelopedData read too. RFC 8551 §2.7.1.2
/// would have GCM with nothing known, but a recipient with an older client
/// then couldn't read it. Your own certificate, also encrypted to, doesn't
/// count: Picacho reads both. With no recipients (a draft), CBC.
pub(super) fn content_cipher(certs: &Certificates, recipients: &[X509]) -> ContentCipher {
    let gcm = dotted(oids::AES256_GCM);
    let cbc = dotted(oids::AES256_CBC);
    let prefers_gcm = |cert: &X509| {
        let fp = fingerprint(cert);
        certs.capabilities.iter().find(|c| c.fingerprint == fp).is_some_and(|known| {
            let first = known.algorithms.iter().find(|a| Some(*a) == gcm.as_ref() || Some(*a) == cbc.as_ref());
            first.is_some() && first == gcm.as_ref()
        })
    };
    if !recipients.is_empty() && recipients.iter().all(prefers_gcm) {
        ContentCipher::Aes256Gcm
    } else {
        ContentCipher::Aes256Cbc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oids_are_written_dotted() {
        assert_eq!(dotted(oids::AES256_GCM).as_deref(), Some("2.16.840.1.101.3.4.1.46"));
        assert_eq!(dotted(oids::SMIME_CAPABILITIES).as_deref(), Some("1.2.840.113549.1.9.15"));
        assert_eq!(dotted(&[]), None);
        assert_eq!(dotted(&[0x86]), None, "unfinished arc");
        assert_eq!(dotted(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]), None, "past 64 bits");
    }

    #[test]
    fn signing_times_read_as_rfc_5280_says() {
        let tlv = |tag, text: &str| {
            let encoded = der::encode(tag, &[text.as_bytes()]);
            signing_time(&der::single(&encoded).unwrap())
        };
        assert_eq!(tlv(der::UTC_TIME, "261123100000Z"), Some(1_795_428_000));
        assert_eq!(tlv(der::UTC_TIME, "500101000000Z"), Some(-631_152_000));
        assert_eq!(tlv(der::GENERALIZED_TIME, "20261123100000Z"), Some(1_795_428_000));
        assert_eq!(tlv(der::UTC_TIME, "nonsense"), None);
        assert_eq!(tlv(der::OCTET_STRING, "261123100000Z"), None);
    }
}
