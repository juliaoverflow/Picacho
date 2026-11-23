//! Finding people's OpenPGP keys: the Web Key Directory their domain
//! publishes (WKD, draft-koch-openpgp-webkey-service), the
//! keys.openpgp.org keyserver (which only lists addresses their owners
//! confirmed), and Autocrypt headers in mail they sent (Autocrypt Level 1).
use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};
use std::time::Duration;

/// z-base-32 (used for WKD's hashed local parts).
fn zbase32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The two WKD addresses for `email`: advanced (openpgpkey.<domain>) first.
pub fn wkd_urls(email: &str) -> Option<[String; 2]> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.to_ascii_lowercase();
    // Only a plain DNS name: no ports, user info, IP addresses or path
    // characters, so an address can't point the lookup anywhere else.
    if local.is_empty() || !super::mta_sts::is_domain(&domain) {
        return None;
    }
    // ASCII letters only are folded (draft-koch-openpgp-webkey-service §3.1).
    let hash = zbase32(&Sha1::digest(local.to_ascii_lowercase().as_bytes()));
    // Percent-encoded (`%20`, not form encoding's `+`).
    let local: String = local
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Some([
        format!("https://openpgpkey.{domain}/.well-known/openpgpkey/{domain}/hu/{hash}?l={local}"),
        format!("https://{domain}/.well-known/openpgpkey/hu/{hash}?l={local}"),
    ])
}

fn http(tls: native_tls::TlsConnector) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(6))
        .https_only(true)
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// The most a key lookup reads: keys are a few kilobytes, and a server
/// could otherwise send without end.
const MAX_KEY_BYTES: u64 = 1 << 20;

/// A response's body, refused past `MAX_KEY_BYTES`.
fn limited(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response.take(MAX_KEY_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_KEY_BYTES, "The key found is too large.");
    Ok(bytes)
}

/// Where a key was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Wkd,
    /// DNS (OPENPGPKEY, RFC 7929). `dnssec_validated`: the answer was
    /// signed and validated to the root, so the domain itself published
    /// the key; otherwise it is unverified, as a key server's is.
    Dane { dnssec_validated: bool },
    KeysOpenpgpOrg,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wkd => "the address's own domain (WKD)",
            Self::Dane { dnssec_validated: true } => "the address's domain's DNSSEC-signed DNS (OPENPGPKEY)",
            Self::Dane { dnssec_validated: false } => "the address's domain's DNS (OPENPGPKEY, unverified)",
            Self::KeysOpenpgpOrg => "keys.openpgp.org",
        }
    }
    /// Whether the address's domain itself vouches for a key from here:
    /// only a DNS answer DNSSEC validated to the root (RFC 7929 §7, RFC
    /// 8162 §9). Every other source is unverified until compared with the
    /// key's owner.
    pub fn domain_vouched(self) -> bool {
        matches!(self, Self::Dane { dnssec_validated: true })
    }
}

/// The key a DNS lookup gives (OPENPGPKEY, RFC 7929), and its source:
/// validated or not as DNSSEC said. A bogus answer has no keys (see
/// `auth::dns_keys`), so it gives none here, not even unverified.
fn dane_found(found: super::auth::DnsKeys) -> Option<(Vec<u8>, Source)> {
    let key = found.keys.into_iter().find(|k| !k.is_empty())?;
    Some((
        key,
        Source::Dane {
            dnssec_validated: found.dnssec_validated,
        },
    ))
}

/// Looks for `email`'s key on the network: its domain's WKD, then
/// keys.openpgp.org. Returns the key (binary or armored, as served).
pub fn lookup(email: &str, tls: native_tls::TlsConnector) -> Result<Option<(Vec<u8>, Source)>> {
    let client = http(tls)?;
    if let Some(urls) = wkd_urls(email) {
        let (_, domain) = email.trim().rsplit_once('@').unwrap_or_default();
        let domain = domain.to_ascii_lowercase();
        // The direct method only where the advanced one isn't set up: a
        // domain with the openpgpkey subdomain (its policy file there)
        // answers for its keys there alone (§3.1).
        let advanced = format!("https://openpgpkey.{domain}/.well-known/openpgpkey/{domain}/policy");
        let advanced_set_up = client
            .get(&advanced)
            .send()
            .is_ok_and(|r| r.status().is_success());
        for (i, url) in urls.into_iter().enumerate() {
            if i == 1 && advanced_set_up {
                break;
            }
            if let Ok(response) = client.get(&url).send()
                && response.status().is_success()
            {
                let bytes = limited(response)?;
                if !bytes.is_empty() {
                    return Ok(Some((bytes, Source::Wkd)));
                }
            }
        }
    }
    // The domain's DNS (RFC 7929), where it publishes keys there; none
    // from an answer DNSSEC found bogus.
    if let Ok(found) = super::auth::openpgpkey(email)
        && let Some(found) = dane_found(found)
    {
        return Ok(Some(found));
    }
    let url = format!(
        "https://keys.openpgp.org/vks/v1/by-email/{}",
        url::form_urlencoded::byte_serialize(email.trim().as_bytes()).collect::<String>()
    );
    let response = client
        .get(&url)
        .send()
        .context("keys.openpgp.org couldn't be reached.")?;
    match response.status().as_u16() {
        200 => Ok(Some((limited(response)?, Source::KeysOpenpgpOrg))),
        404 => Ok(None),
        status => bail!("keys.openpgp.org answered {status}."),
    }
}

/// An Autocrypt header's address and key (RFC-like Level 1: `addr=`,
/// optional `prefer-encrypt=`, and `keydata=` base64), from a message's
/// headers. The header must be for the message's From address.
#[cfg(test)]
pub fn autocrypt(raw: &[u8]) -> Option<(String, Vec<u8>)> {
    autocrypt_full(raw).map(|a| (a.addr, a.key))
}

/// An Autocrypt header's whole content, and when the message was sent.
#[derive(Debug, PartialEq, Eq)]
pub struct Autocrypt {
    pub addr: String,
    pub key: Vec<u8>,
    /// `prefer-encrypt=mutual`: the sender wants encrypted replies.
    pub mutual: bool,
    /// The message's Date, Unix seconds (0 where it has none).
    pub date: i64,
}

/// The Autocrypt header of a message for its sender (Autocrypt 1.1 §2.3):
/// none from reports (multipart/report) or mail without a single From.
pub fn autocrypt_full(raw: &[u8]) -> Option<Autocrypt> {
    use base64::Engine as _;
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    if headers
        .get_first_value("Content-Type")
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("multipart/report"))
    {
        return None;
    }
    let date = headers
        .get_first_value("Date")
        .and_then(|d| mailparse::dateparse(&d).ok())
        .unwrap_or(0);
    let from = mailparse::addrparse(&headers.get_first_value("From")?)
        .ok()?
        .extract_single_info()?
        .addr
        .to_lowercase();
    for value in headers.get_all_values("Autocrypt") {
        let mut addr = None;
        let mut keydata = None;
        let mut mutual = false;
        let mut unknown_critical = false;
        for attr in value.split(';') {
            let Some((k, v)) = attr.split_once('=') else {
                continue;
            };
            match k.trim() {
                "addr" => addr = Some(v.trim().to_lowercase()),
                "keydata" => keydata = Some(v.split_whitespace().collect::<String>()),
                "prefer-encrypt" => mutual = v.trim().eq_ignore_ascii_case("mutual"),
                // Attributes not starting with `_` must be understood.
                other if !other.starts_with('_') => unknown_critical = true,
                _ => {}
            }
        }
        if unknown_critical || addr.as_deref() != Some(from.as_str()) {
            continue;
        }
        let key = base64::engine::general_purpose::STANDARD
            .decode(keydata?)
            .ok()?;
        return Some(Autocrypt {
            addr: from,
            key,
            mutual,
            date,
        });
    }
    None
}

/// The keys an encrypted message's sender passed along for its other
/// recipients (Autocrypt-Gossip, Autocrypt 1.1 §3.6), from the decrypted
/// headers: (address, key), for addresses among `recipients` only.
pub fn autocrypt_gossip(decrypted: &[u8], recipients: &[String]) -> Vec<(String, Vec<u8>)> {
    use base64::Engine as _;
    use mailparse::MailHeaderMap;
    let Ok((headers, _)) = mailparse::parse_headers(decrypted) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for value in headers.get_all_values("Autocrypt-Gossip") {
        let mut addr = None;
        let mut keydata = None;
        for attr in value.split(';') {
            if let Some((k, v)) = attr.split_once('=') {
                match k.trim() {
                    "addr" => addr = Some(v.trim().to_lowercase()),
                    "keydata" => keydata = Some(v.split_whitespace().collect::<String>()),
                    _ => {}
                }
            }
        }
        if let (Some(addr), Some(data)) = (addr, keydata)
            && recipients.iter().any(|r| r.eq_ignore_ascii_case(&addr))
            && let Ok(key) = base64::engine::general_purpose::STANDARD.decode(data)
        {
            out.push((addr, key));
        }
    }
    out
}

/// A key by its fingerprint (HKP, draft-gallagher-openpgp-hkp §3.1.1), from
/// keys.openpgp.org's HKP interface: for a signature's issuer that isn't
/// known here yet.
pub fn lookup_fingerprint(fingerprint: &str, tls: native_tls::TlsConnector) -> Result<Option<Vec<u8>>> {
    let fp: String = fingerprint.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    anyhow::ensure!(
        fp.len() == 40 || fp.len() == 64,
        "A whole fingerprint is needed to look up a key."
    );
    let client = http(tls)?;
    let url = format!("https://keys.openpgp.org/pks/lookup?op=get&options=mr&search=0x{fp}");
    let response = client.get(&url).send().context("keys.openpgp.org couldn't be reached.")?;
    match response.status().as_u16() {
        200 => Ok(Some(limited(response)?)),
        404 => Ok(None),
        status => bail!("keys.openpgp.org answered {status}."),
    }
}

/// The Autocrypt header advertising `armored_public` for `email`.
pub fn autocrypt_header(email: &str, public: &[u8]) -> String {
    autocrypt_header_with(email, public, false)
}
/// As `autocrypt_header`, saying `prefer-encrypt=mutual` when `mutual`.
pub fn autocrypt_header_with(email: &str, public: &[u8], mutual: bool) -> String {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(public);
    // Folded at 76, as header lines should be.
    let folded = data
        .as_bytes()
        .chunks(72)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n ");
    let prefer = if mutual { " prefer-encrypt=mutual;" } else { "" };
    format!("addr={email};{prefer} keydata=\r\n {folded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkd_hashes_the_local_part() {
        // The example from the WKD draft: Joe.Doe@Example.ORG.
        let [advanced, direct] = wkd_urls("Joe.Doe@Example.ORG").unwrap();
        assert_eq!(
            direct,
            "https://example.org/.well-known/openpgpkey/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q?l=Joe.Doe"
        );
        assert_eq!(
            advanced,
            "https://openpgpkey.example.org/.well-known/openpgpkey/example.org/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q?l=Joe.Doe"
        );
        assert!(wkd_urls("nobody").is_none());
    }

    #[test]
    fn autocrypt_headers_are_read_for_their_sender_only() {
        let raw = b"From: Ana <ana@example.org>\r\nAutocrypt: addr=ana@example.org; prefer-encrypt=mutual; keydata=\r\n AAEC\r\n Aw==\r\nSubject: x\r\n\r\nbody";
        assert_eq!(
            autocrypt(raw),
            Some(("ana@example.org".into(), vec![0, 1, 2, 3]))
        );
        let other = b"From: Mallory <m@evil.example>\r\nAutocrypt: addr=ana@example.org; keydata=AAEC\r\n\r\n";
        assert_eq!(autocrypt(other), None);
        let critical = b"From: ana@example.org\r\nAutocrypt: addr=ana@example.org; future=1; keydata=AAEC\r\n\r\n";
        assert_eq!(autocrypt(critical), None);
        let header = autocrypt_header("ana@example.org", &[0, 1, 2, 3]);
        let back = format!("From: ana@example.org\r\nAutocrypt: {header}\r\n\r\n");
        assert_eq!(autocrypt(back.as_bytes()).unwrap().1, vec![0, 1, 2, 3]);
    }

    /// RFC 7929 §7: a DNS-published key is the domain's word only when
    /// DNSSEC validated the answer; an insecure one is unverified, and a
    /// bogus one gives no key at all (the lookup goes on to the keyserver).
    #[test]
    fn dns_keys_are_trusted_only_when_dnssec_validated() {
        use super::super::auth::{Validation, dns_keys};
        let key = vec![vec![1u8, 2, 3]];
        let secure = dane_found(dns_keys(key.clone(), Validation::Secure)).unwrap();
        assert_eq!(secure, (key[0].clone(), Source::Dane { dnssec_validated: true }));
        assert!(secure.1.domain_vouched());
        let insecure = dane_found(dns_keys(key.clone(), Validation::Insecure)).unwrap();
        assert_eq!(insecure.1, Source::Dane { dnssec_validated: false });
        assert!(!insecure.1.domain_vouched());
        assert_eq!(dane_found(dns_keys(key, Validation::Bogus)), None);
        assert_eq!(dane_found(dns_keys(vec![Vec::new()], Validation::Secure)), None);
        assert!(!Source::Wkd.domain_vouched());
        assert!(!Source::KeysOpenpgpOrg.domain_vouched());
    }
}
