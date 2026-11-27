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

/// The client for key lookups. Their addresses come from correspondents'
/// domains, anyone's to choose: only servers on the internet
/// (`public_net::PublicOnly`), redirected only there, over https.
fn http(tls: native_tls::TlsConnector) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(6))
        .https_only(true)
        .dns_resolver(std::sync::Arc::new(super::public_net::PublicOnly))
        .redirect(super::public_net::public_redirects(5))
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// GETs `url` unless it would reach this computer or its network: a host
/// given as such an address, or a name a proxy would lead there
/// (`public_net::reaches_inside`), which the resolver doesn't see.
fn get(client: &reqwest::blocking::Client, url: &str) -> Option<reqwest::blocking::Response> {
    let parsed = url::Url::parse(url).ok()?;
    if super::public_net::reaches_inside(&parsed) {
        return None;
    }
    client.get(parsed).send().ok()
}

/// The most a key lookup reads: keys are a few kilobytes, and a server
/// could otherwise send without end.
const MAX_KEY_BYTES: u64 = 1 << 20;

/// A response's body, refused past `MAX_KEY_BYTES`.
fn limited(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response.take(MAX_KEY_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_KEY_BYTES, t!("sync-keys-the-key-found-is-too-large"));
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
            Self::Wkd => t!("sync-keys-the-addresss-own-domain-wkd"),
            Self::Dane { dnssec_validated: true } => t!("sync-keys-the-addresss-domains-dnssec-signed-dns"),
            Self::Dane { dnssec_validated: false } => t!("sync-keys-the-addresss-domains-dns-openpgpkey-unverified"),
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
        let advanced_set_up = get(&client, &advanced).is_some_and(|r| r.status().is_success());
        for (i, url) in urls.into_iter().enumerate() {
            if i == 1 && advanced_set_up {
                break;
            }
            if let Some(response) = get(&client, &url)
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
        .with_context(|| t!("sync-keys-keys-openpgp-org-couldnt-be-reached"))?;
    match response.status().as_u16() {
        200 => Ok(Some((limited(response)?, Source::KeysOpenpgpOrg))),
        // Not on keys.openpgp.org. HKP keyservers (keyserver.ubuntu.com and
        // the like) aren't asked: they list a key under any address its
        // uploader claims, without checking it, so a key found there could
        // be anyone's, and encrypting to it would give them the mail.
        404 => Ok(None),
        status => bail!(t!("sync-keys-keys-openpgp-org-answered", status = status)),
    }
}

/// Normalizes an HKP keyserver address (such as `keyserver.ubuntu.com`,
/// `hkps://keys.mailvelope.com`, or `https://pgp.example.org`) to its
/// HTTPS `/pks/add` endpoint. Plain HTTP is refused.
pub fn hkp_add_url(server: &str) -> Result<String> {
    let raw = server.trim();
    anyhow::ensure!(!raw.is_empty(), t!("sync-keys-enter-a-keyserver-address"));
    let with_scheme = if raw.contains("://") {
        if let Some(rest) = raw
            .strip_prefix("hkps://")
            .or_else(|| raw.strip_prefix("HKPS://"))
        {
            format!("https://{rest}")
        } else {
            raw.to_owned()
        }
    } else {
        format!("https://{raw}")
    };
    let mut url = url::Url::parse(&with_scheme).with_context(|| t!("sync-keys-that-isnt-a-valid-keyserver-address"))?;
    anyhow::ensure!(
        url.scheme() == "https",
        t!("sync-keys-keyservers-must-use-https-or-hkps")
    );
    anyhow::ensure!(url.host_str().is_some(), t!("sync-keys-keyserver-address-has-no-host"));
    url.set_path("/pks/add");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

/// Publishes an armored public key to an HKP keyserver (`POST /pks/add`,
/// draft-gallagher-openpgp-hkp §4), such as `keyserver.ubuntu.com` or a
/// custom server. Unlike keys.openpgp.org, standard HKP keyservers list the
/// key and its email addresses immediately without a confirmation email.
pub fn publish_hkp(server: &str, armored: &str, tls: native_tls::TlsConnector) -> Result<String> {
    let url = hkp_add_url(server)?;
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| server.trim().to_owned());
    let client = http(tls)?;
    let response = client
        .post(&url)
        .form(&[("keytext", armored)])
        .send()
        .with_context(|| t!("sync-keys-couldnt-be-reached", host = host))?;
    let status = response.status();
    if !status.is_success() {
        bail!(t!("sync-keys-answered", host = host, status = status.as_u16()));
    }
    Ok(host)
}

/// Exports public keys into Web Key Directory (WKD) trees under `dir`, one
/// per domain, laid out for the direct method (draft-koch-openpgp-webkey-service
/// §3.1): `<domain>/.well-known/openpgpkey/hu/<hash>` and an empty `policy`
/// beside `hu`, so each `<domain>` folder's contents go in that domain's web
/// root as they are (served at `https://<domain>/.well-known/openpgpkey/…`).
/// Each key is the one for its address alone (`keys` pairs them). Returns the
/// domain folders written.
pub fn export_wkd(
    keys: &[(String, Vec<u8>)],
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>> {
    let mut roots = Vec::new();
    for (email, binary_key) in keys {
        let Some((local, domain)) = email.trim().rsplit_once('@') else {
            continue;
        };
        let domain = domain.to_ascii_lowercase();
        if local.is_empty() || !super::mta_sts::is_domain(&domain) || binary_key.is_empty() {
            continue;
        }
        let hash = zbase32(&Sha1::digest(local.to_ascii_lowercase().as_bytes()));
        let root = dir.join(&domain);
        let base = root.join(".well-known").join("openpgpkey");
        let hu = base.join("hu");
        std::fs::create_dir_all(&hu)?;
        let policy = base.join("policy");
        if !policy.exists() {
            std::fs::write(&policy, b"")?;
        }
        std::fs::write(hu.join(&hash), binary_key)?;
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    anyhow::ensure!(
        !roots.is_empty(),
        t!("sync-keys-no-email-addresses-on-your-keys")
    );
    Ok(roots)
}

/// What publishing a key on keys.openpgp.org did, by address: those it
/// lists already, and those it sent a confirmation email to (it lists an
/// address only once its owner follows the link in that email).
#[derive(Debug, Default)]
pub struct Published {
    pub listed: Vec<String>,
    pub confirming: Vec<String>,
}

/// Uploads a public key (armored) to keys.openpgp.org, and asks it to
/// confirm `addresses` of it that it doesn't list yet (VKS API v1).
pub fn publish(armored: &str, addresses: &[String], tls: native_tls::TlsConnector) -> Result<Published> {
    let client = http(tls)?;
    let response = client
        .post("https://keys.openpgp.org/vks/v1/upload")
        .json(&serde_json::json!({ "keytext": armored }))
        .send()
        .with_context(|| t!("sync-keys-keys-openpgp-org-couldnt-be-reached"))?;
    let status = response.status();
    let answer: serde_json::Value = serde_json::from_slice(&limited(response)?).unwrap_or_default();
    if !status.is_success() {
        let why = answer["error"].as_str().unwrap_or(t!("sync-keys-it-refused-the-key"));
        bail!(t!("sync-keys-keys-openpgp-org-answered-2", status = status.as_u16(), why = why));
    }
    let token = answer["token"].as_str().with_context(|| t!("sync-keys-keys-openpgp-org-didnt-answer-as"))?;
    let mut out = Published::default();
    let mut ask = Vec::new();
    if let Some(states) = answer["status"].as_object() {
        for (address, state) in states {
            let wanted = addresses.iter().any(|a| a.eq_ignore_ascii_case(address));
            match state.as_str() {
                Some("published") => out.listed.push(address.clone()),
                Some("unpublished") if wanted => ask.push(address.clone()),
                Some("pending") if wanted => out.confirming.push(address.clone()),
                _ => {}
            }
        }
    }
    if !ask.is_empty() {
        let response = client
            .post("https://keys.openpgp.org/vks/v1/request-verify")
            .json(&serde_json::json!({ "token": token, "addresses": ask, "locale": ["en"] }))
            .send()
            .with_context(|| t!("sync-keys-keys-openpgp-org-couldnt-be-reached"))?;
        if !response.status().is_success() {
            bail!(t!("sync-keys-keys-openpgp-org-couldnt-send-the", status = response.status().as_u16()));
        }
        out.confirming.extend(ask);
    }
    Ok(out)
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
    /// The message's effective date, Unix seconds (see `AutocryptMessage`).
    pub date: i64,
}

/// The Autocrypt header of a message for its sender (Autocrypt 1.1 §2.3),
/// where the message counts for Autocrypt and has exactly one valid one.
#[cfg(test)]
pub fn autocrypt_full(raw: &[u8]) -> Option<Autocrypt> {
    autocrypt_message(raw, i64::MAX)?.header
}

/// A received message as Autocrypt peer state sees it (Autocrypt 1.1
/// §2.3, "Updating Autocrypt Peer State").
#[derive(Debug, PartialEq, Eq)]
pub struct AutocryptMessage {
    /// The single From address, lowercased.
    pub from: String,
    /// The effective date: the Date, or `received` where the Date is later.
    pub date: i64,
    /// Its one valid Autocrypt header; `None` where it has none, or more
    /// than one (which counts as none).
    pub header: Option<Autocrypt>,
}

/// What a message received at `received` (Unix seconds) tells Autocrypt
/// about its sender. `None` for messages peer state must ignore
/// (Autocrypt 1.1 §2.3): reports (multipart/report), mail without exactly
/// one From address, mailing-list mail (List-Id, RFC 2919; List-Post,
/// RFC 2369), which a list's own software may have changed, and mail
/// without a Date, which would count as new each time it is read (the
/// §2.3 note allows ignoring it).
pub fn autocrypt_message(raw: &[u8], received: i64) -> Option<AutocryptMessage> {
    use mailparse::MailHeaderMap;
    let (headers, _) = mailparse::parse_headers(raw).ok()?;
    if headers
        .get_first_value("Content-Type")
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("multipart/report"))
    {
        return None;
    }
    if headers.get_first_header("List-Id").is_some() || headers.get_first_header("List-Post").is_some() {
        return None;
    }
    let date = headers
        .get_first_value("Date")
        .and_then(|d| mailparse::dateparse(&d).ok())?
        .min(received);
    let from = mailparse::addrparse(&headers.get_first_value("From")?)
        .ok()?
        .extract_single_info()?
        .addr
        .to_lowercase();
    let mut valid = headers
        .get_all_values("Autocrypt")
        .into_iter()
        .filter_map(|value| autocrypt_value(&value, &from, date));
    // §2.3: more than one valid header is an error, and all are discarded.
    let header = match (valid.next(), valid.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    };
    Some(AutocryptMessage { from, date, header })
}

/// One Autocrypt header's value, if valid for `from` (Autocrypt 1.1
/// §2.1.1): `addr` is the From address, every critical attribute is
/// understood, and `keydata` is base64.
fn autocrypt_value(value: &str, from: &str, date: i64) -> Option<Autocrypt> {
    use base64::Engine as _;
    let mut addr = None;
    let mut keydata = None;
    let mut mutual = false;
    for attr in value.split(';') {
        let Some((k, v)) = attr.split_once('=') else {
            continue;
        };
        match k.trim() {
            "addr" => addr = Some(v.trim().to_lowercase()),
            "keydata" => keydata = Some(v.split_whitespace().collect::<String>()),
            "prefer-encrypt" => mutual = v.trim().eq_ignore_ascii_case("mutual"),
            // Attributes not starting with `_` must be understood.
            other if !other.starts_with('_') => return None,
            _ => {}
        }
    }
    if addr.as_deref() != Some(from) {
        return None;
    }
    let key = base64::engine::general_purpose::STANDARD
        .decode(keydata?)
        .ok()?;
    Some(Autocrypt {
        addr: from.to_owned(),
        key,
        mutual,
        date,
    })
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
        t!("sync-keys-a-whole-fingerprint-is-needed-to")
    );
    let client = http(tls)?;
    let url = format!("https://keys.openpgp.org/pks/lookup?op=get&options=mr&search=0x{fp}");
    let response = client.get(&url).send().with_context(|| t!("sync-keys-keys-openpgp-org-couldnt-be-reached"))?;
    match response.status().as_u16() {
        200 => Ok(Some(limited(response)?)),
        404 => Ok(None),
        status => bail!(t!("sync-keys-keys-openpgp-org-answered", status = status)),
    }
}

/// The public key at the URL an OpenPGP card names for it (its "URL of
/// public key" data object): https on the internet only. `None` when the
/// address can't be fetched.
pub fn fetch_url(url: &str, tls: native_tls::TlsConnector) -> Result<Option<Vec<u8>>> {
    let client = http(tls)?;
    match get(&client, url.trim()) {
        Some(response) if response.status().is_success() => Ok(Some(limited(response)?)),
        _ => Ok(None),
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
        let raw = b"Date: Tue, 1 Oct 2024 10:00:00 +0000\r\nFrom: Ana <ana@example.org>\r\nAutocrypt: addr=ana@example.org; prefer-encrypt=mutual; keydata=\r\n AAEC\r\n Aw==\r\nSubject: x\r\n\r\nbody";
        assert_eq!(
            autocrypt(raw),
            Some(("ana@example.org".into(), vec![0, 1, 2, 3]))
        );
        let other = b"Date: Tue, 1 Oct 2024 10:00:00 +0000\r\nFrom: Mallory <m@evil.example>\r\nAutocrypt: addr=ana@example.org; keydata=AAEC\r\n\r\n";
        assert_eq!(autocrypt(other), None);
        let critical = b"Date: Tue, 1 Oct 2024 10:00:00 +0000\r\nFrom: ana@example.org\r\nAutocrypt: addr=ana@example.org; future=1; keydata=AAEC\r\n\r\n";
        assert_eq!(autocrypt(critical), None);
        let header = autocrypt_header("ana@example.org", &[0, 1, 2, 3]);
        let back = format!("Date: Tue, 1 Oct 2024 10:00:00 +0000\r\nFrom: ana@example.org\r\nAutocrypt: {header}\r\n\r\n");
        assert_eq!(autocrypt(back.as_bytes()).unwrap().1, vec![0, 1, 2, 3]);
    }

    /// Autocrypt 1.1 §2.3: which messages count for peer state, their
    /// effective date, and how many headers make a valid one.
    #[test]
    fn autocrypt_messages_follow_the_peer_state_rules() {
        let sent = 1_727_776_800; // Tue, 1 Oct 2024 10:00:00 +0000
        let head = "From: Ana <ana@example.org>\r\nDate: Tue, 1 Oct 2024 10:00:00 +0000\r\n";
        let one = "Autocrypt: addr=ana@example.org; _note=x; keydata=AAEC\r\n";
        let message = |extra: &str| format!("{head}{extra}Subject: x\r\n\r\nbody");
        // The Date is the effective date; one received earlier than it was
        // sent has the receipt time instead.
        let m = autocrypt_message(message(one).as_bytes(), sent + 60).unwrap();
        assert_eq!((m.from.as_str(), m.date), ("ana@example.org", sent));
        assert_eq!(m.header.unwrap().key, vec![0, 1, 2]);
        assert_eq!(autocrypt_message(message(one).as_bytes(), sent - 60).unwrap().date, sent - 60);
        // Without a header it still counts (for last_seen).
        let none = autocrypt_message(message("").as_bytes(), sent).unwrap();
        assert!(none.header.is_none());
        // Two valid headers: neither counts; an invalid second is skipped.
        let two = format!("{one}Autocrypt: addr=ana@example.org; keydata=AQ==\r\n");
        assert!(autocrypt_message(message(&two).as_bytes(), sent).unwrap().header.is_none());
        let bad = format!("{one}Autocrypt: addr=ana@example.org; future=1; keydata=AQ==\r\n");
        assert!(autocrypt_message(message(&bad).as_bytes(), sent).unwrap().header.is_some());
        // Ignored altogether: mailing lists, reports, several From
        // addresses, and mail without a Date.
        for ignored in [
            message(&format!("List-Id: <talk.example.org>\r\n{one}")),
            message(&format!("List-Post: <mailto:talk@example.org>\r\n{one}")),
            message(&format!("Content-Type: multipart/report; report-type=delivery-status\r\n{one}")),
            format!("From: ana@example.org, bo@example.org\r\nDate: Tue, 1 Oct 2024 10:00:00 +0000\r\n{one}\r\n"),
            format!("From: ana@example.org\r\n{one}\r\n"),
        ] {
            assert_eq!(autocrypt_message(ignored.as_bytes(), sent), None, "{ignored}");
        }
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

    #[test]
    fn hkp_add_url_normalizes_and_requires_tls() {
        assert_eq!(
            hkp_add_url("keyserver.ubuntu.com").unwrap(),
            "https://keyserver.ubuntu.com/pks/add"
        );
        assert_eq!(
            hkp_add_url("hkps://keys.example.org:443/").unwrap(),
            "https://keys.example.org/pks/add"
        );
        assert_eq!(
            hkp_add_url("https://keys.example.org/pks/add").unwrap(),
            "https://keys.example.org/pks/add"
        );
        assert!(hkp_add_url("http://insecure.example.org").is_err());
        assert!(hkp_add_url("hkp://insecure.example.org").is_err());
        assert!(hkp_add_url("   ").is_err());
    }

    #[test]
    fn export_wkd_writes_hashed_keys_and_policy_per_domain() {
        let dir = std::env::temp_dir().join(format!("picacho-wkd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let written = export_wkd(
            &[
                ("Joe.Doe@Example.ORG".into(), vec![0x99, 0x01, 0x02]),
                ("invalid-address".into(), vec![0x99]),
            ],
            &dir,
        )
        .unwrap();
        // Direct method: the domain's folder holds its web root's
        // `.well-known/openpgpkey/hu/<hash>` and `policy`.
        assert_eq!(written, vec![dir.join("example.org")]);
        let base = dir.join("example.org").join(".well-known").join("openpgpkey");
        let expected_key = base.join("hu").join("iy9q119eutrkn8s1mk4r39qejnbu3n5q");
        assert_eq!(std::fs::read(&expected_key).unwrap(), vec![0x99, 0x01, 0x02]);
        assert!(base.join("policy").is_file());
        assert!(export_wkd(&[("bad".into(), vec![1])], &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
