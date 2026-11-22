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
    let hash = zbase32(&Sha1::digest(local.to_lowercase().as_bytes()));
    let local: String = url::form_urlencoded::byte_serialize(local.as_bytes()).collect();
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
    KeysOpenpgpOrg,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wkd => "the address's own domain (WKD)",
            Self::KeysOpenpgpOrg => "keys.openpgp.org",
        }
    }
}

/// Looks for `email`'s key on the network: its domain's WKD, then
/// keys.openpgp.org. Returns the key (binary or armored, as served).
pub fn lookup(email: &str, tls: native_tls::TlsConnector) -> Result<Option<(Vec<u8>, Source)>> {
    let client = http(tls)?;
    if let Some(urls) = wkd_urls(email) {
        for url in urls {
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
pub fn autocrypt(raw: &[u8]) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    let from = mailparse::addrparse(&headers.get_first_value("From")?)
        .ok()?
        .extract_single_info()?
        .addr
        .to_lowercase();
    for value in headers.get_all_values("Autocrypt") {
        let mut addr = None;
        let mut keydata = None;
        let mut unknown_critical = false;
        for attr in value.split(';') {
            let Some((k, v)) = attr.split_once('=') else {
                continue;
            };
            match k.trim() {
                "addr" => addr = Some(v.trim().to_lowercase()),
                "keydata" => keydata = Some(v.split_whitespace().collect::<String>()),
                "prefer-encrypt" => {}
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
        return Some((from, key));
    }
    None
}

/// The Autocrypt header advertising `armored_public` for `email`.
pub fn autocrypt_header(email: &str, public: &[u8]) -> String {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(public);
    // Folded at 76, as header lines should be.
    let folded = data
        .as_bytes()
        .chunks(72)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n ");
    format!("addr={email}; keydata=\r\n {folded}")
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
}
