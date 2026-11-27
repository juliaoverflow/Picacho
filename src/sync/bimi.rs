//! BIMI (Brand Indicators for Message Identification): the logo a sender's
//! domain publishes for its mail, shown only once it is vouched for.
//!
//! A logo counts here only when all of these hold:
//!
//! - the message passed DMARC for its From domain, and that domain asks
//!   for failing mail to be quarantined or rejected (`eligible`), so mail
//!   that merely claims the domain never shows its logo;
//! - the domain (or its organizational domain) publishes a BIMI record,
//!   `default._bimi.<domain>` (`record`);
//! - the record names a Verified Mark Certificate or a Common Mark
//!   Certificate (`a=`), which chains to one of the Mark Verifying
//!   Authorities' roots kept in `assets/bimi/roots.pem`, is current, is for
//!   BIMI (its extended key usage), and names the domain (`verify`);
//! - and the certificate carries the logo itself (RFC 3709's logotype
//!   extension), an SVG Tiny PS image with nothing outside it
//!   (`rasterize`). The image at the record's `l=` is never fetched: the
//!   certificate's is the one an authority checked.
//!
//! Self-asserted records (with no certificate) show nothing. Only the
//! `default` selector is looked up: a BIMI-Selector header chosen per
//! message would let a sender tell when each recipient's mail was checked.
//! Revocation is checked by Manage (`manage/bimi.rs`), which keeps the
//! results.
use anyhow::{Context, Result, bail, ensure};
use openssl::x509::X509;

/// RFC 3709: id-pe-logotype.
const LOGOTYPE: &str = "1.3.6.1.5.5.7.1.12";
/// id-kp-BrandIndicatorforMessageIdentification.
const EKU_BIMI: &str = "1.3.6.1.5.5.7.3.31";
/// The BIMI Group's mark type, a subject attribute: Registered Mark,
/// Government Mark, Prior Use Mark, Modified Registered Mark…
const MARK_TYPE: &str = "1.3.6.1.4.1.53087.1.13";
/// The largest certificate chain read.
const MAX_CERTIFICATE: u64 = 256 * 1024;
/// The largest logo accepted, uncompressed. BIMI asks for 32 KB; some
/// published logos are a little over.
const MAX_SVG: usize = 64 * 1024;
/// The side of the square the logo is drawn in, in pixels.
pub const LOGO_SIZE: u32 = 128;

/// The roots of the Mark Verifying Authorities.
const ROOTS: &str = include_str!("../../assets/bimi/roots.pem");

/// What kind of mark certificate vouched for a logo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MarkKind {
    /// A Verified Mark Certificate: a registered (or government) trademark.
    Verified,
    /// A Common Mark Certificate: a mark in use, not registered.
    Common,
}

/// A domain's published record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// `l=`: where the logo is published (not fetched; see above).
    pub location: String,
    /// `a=`: where its mark certificate (PEM) is.
    pub authority: String,
}

/// A certificate that vouched for a logo, as read from it.
#[derive(Clone, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    /// The organization it was issued to.
    pub holder: String,
    /// The logo, SVG.
    pub svg: Vec<u8>,
    /// When it expires, Unix seconds.
    pub until: i64,
    /// The certificate and its issuer, for checking revocation.
    pub leaf: X509,
    pub issuer: X509,
}

/// Why a message's sender has no logo shown, when it doesn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// DMARC didn't pass, or the domain doesn't ask for failing mail to be
    /// quarantined or rejected.
    NotEligible,
    /// The domain publishes no BIMI record.
    NoRecord,
    /// Its record names no certificate (a self-asserted logo).
    SelfAsserted,
    /// A certificate was named but didn't check out, or couldn't be had.
    Invalid(String),
}

/// Whether a message's sender may have its logo shown: DMARC passed, and
/// its policy (`p=`, or `sp=` for a subdomain) is quarantine or reject.
pub fn eligible(dmarc: Option<&str>, policy: Option<&str>) -> bool {
    dmarc == Some("pass") && matches!(policy, Some("quarantine" | "reject"))
}

/// A BIMI record's tags (`v=BIMI1; l=…; a=…`). `None` unless it is one.
/// Strings a TXT record was split into may come joined with spaces, so
/// spaces are dropped from the addresses.
pub fn parse_record(text: &str) -> Option<Record> {
    let mut tags = text.split(';').map(str::trim).filter(|t| !t.is_empty());
    let version = tags.next()?;
    let (k, v) = version.split_once('=')?;
    if !k.trim().eq_ignore_ascii_case("v") || !v.trim().eq_ignore_ascii_case("BIMI1") {
        return None;
    }
    let mut record = Record::default();
    for tag in tags {
        let Some((k, v)) = tag.split_once('=') else { continue };
        let value: String = v.chars().filter(|c| !c.is_whitespace()).collect();
        match k.trim().to_ascii_lowercase().as_str() {
            "l" => record.location = value,
            "a" => record.authority = value,
            _ => {}
        }
    }
    Some(record)
}

/// The domain's BIMI record, and the domain it was found at: the domain
/// itself, else its organizational domain. `None` where neither has one.
pub fn record(domain: &str) -> Result<Option<(Record, String)>> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut candidates = vec![domain.clone()];
    if let Some(org) = psl::domain_str(&domain)
        && org != domain
    {
        candidates.push(org.to_owned());
    }
    for at in candidates {
        let records: Vec<Record> = crate::sync::auth::txt(&format!("default._bimi.{at}"))?
            .iter()
            .filter_map(|t| parse_record(t))
            .collect();
        // RFC-to-be §4.4: more than one record is as none.
        match records.as_slice() {
            [one] => return Ok(Some((one.clone(), at))),
            [] => continue,
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Fetches a mark certificate chain (PEM) from `url`: HTTPS, on the
/// internet only, and small.
pub fn fetch_certificate(url: &str) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(url).with_context(|| t!("bimi-the-certificate-address-isnt-valid"))?;
    ensure!(parsed.scheme() == "https", t!("bimi-the-certificate-isnt-served-over-https"));
    ensure!(!crate::sync::public_net::names_local_address(&parsed), t!("bimi-the-certificate-address-isnt-valid"));
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .dns_resolver(std::sync::Arc::new(crate::sync::public_net::PublicOnly))
        .redirect(crate::sync::public_net::public_redirects(3))
        .build()?;
    let response = client
        .get(parsed)
        .send()
        .with_context(|| t!("bimi-couldnt-fetch-the-certificate"))?;
    ensure!(response.status().is_success(), t!("bimi-couldnt-fetch-the-certificate"));
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response.take(MAX_CERTIFICATE + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_CERTIFICATE, t!("bimi-the-certificate-is-too-large"));
    Ok(bytes)
}

/// The Mark Verifying Authorities' roots.
fn roots() -> Result<Vec<X509>> {
    Ok(X509::stack_from_pem(ROOTS.as_bytes())?)
}

/// A store of `roots` that verifies as at `now` (Unix seconds).
fn store_at(roots: &[X509], now: i64) -> Result<openssl::x509::store::X509Store> {
    let mut builder = openssl::x509::store::X509StoreBuilder::new()?;
    for root in roots {
        builder.add_cert(root.clone())?;
    }
    let mut param = openssl::x509::verify::X509VerifyParam::new()?;
    param.set_time(now);
    builder.set_param(&param)?;
    Ok(builder.build())
}

/// Checks a mark certificate chain (PEM, the certificate first) for
/// `domains` (the author domain and where the record was found) at `now`
/// (Unix seconds): it chains to an authority's root, is current, is for
/// BIMI, names one of the domains, and carries an SVG logo.
pub fn verify(pem: &[u8], domains: &[&str], now: i64) -> Result<Mark> {
    verify_with(pem, domains, now, &roots()?)
}

fn verify_with(pem: &[u8], domains: &[&str], now: i64, roots: &[X509]) -> Result<Mark> {
    let roots = store_at(roots, now)?;
    let chain = X509::stack_from_pem(pem).map_err(|_| anyhow::anyhow!(t!("bimi-the-certificate-cant-be-read")))?;
    let mut chain = chain.into_iter();
    let leaf = chain.next().with_context(|| t!("bimi-the-certificate-cant-be-read"))?;
    let rest: Vec<X509> = chain.collect();
    let mut untrusted = openssl::stack::Stack::new()?;
    for c in &rest {
        untrusted.push(c.clone())?;
    }
    // The chain, at `now`.
    let mut context = openssl::x509::X509StoreContext::new()?;
    let checked = context.init(&roots, &leaf, &untrusted, |c| {
        let ok = c.verify_cert()?;
        let issuer = c.chain().and_then(|chain| chain.get(1).map(|i| i.to_owned()));
        Ok((ok, c.error(), issuer))
    })?;
    let (ok, error, issuer) = checked;
    if !ok {
        bail!(t!("bimi-the-certificate-isnt-vouched-for", error = error.error_string()));
    }
    let issuer = issuer.with_context(|| t!("bimi-the-certificate-cant-be-read"))?;
    let der = leaf.to_der()?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&der).map_err(|_| anyhow::anyhow!(t!("bimi-the-certificate-cant-be-read")))?;
    // For BIMI: its extended key usage names it.
    let for_bimi = parsed
        .extended_key_usage()
        .ok()
        .flatten()
        .is_some_and(|eku| eku.value.other.iter().any(|o| o.to_id_string() == EKU_BIMI));
    ensure!(for_bimi, t!("bimi-the-certificate-isnt-for-bimi"));
    // For the domain.
    let names: Vec<String> = parsed
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some(d.trim_end_matches('.').to_ascii_lowercase()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let names_domain = domains.iter().any(|d| {
        let d = d.to_ascii_lowercase();
        names.iter().any(|n| *n == d || *n == format!("default._bimi.{d}"))
    });
    ensure!(names_domain, t!("bimi-the-certificate-is-for-another-domain"));
    // The logo it carries.
    let extension = parsed
        .extensions()
        .iter()
        .find(|e| e.oid.to_id_string() == LOGOTYPE)
        .with_context(|| t!("bimi-the-certificate-carries-no-logo"))?;
    let svg = logo_of(extension.value).with_context(|| t!("bimi-the-certificate-carries-no-logo"))?;
    // Who holds it, and what kind of mark.
    let mut holder = String::new();
    let mut mark_type = String::new();
    for attribute in parsed.subject().iter_attributes() {
        let oid = attribute.attr_type().to_id_string();
        let value = attribute.as_str().unwrap_or_default().to_owned();
        if oid == MARK_TYPE {
            mark_type = value;
        } else if oid == "2.5.4.10" && holder.is_empty() {
            holder = value;
        }
    }
    let kind = if matches!(mark_type.trim(), "Prior Use Mark" | "Modified Registered Mark") {
        MarkKind::Common
    } else {
        MarkKind::Verified
    };
    Ok(Mark {
        kind,
        holder,
        svg,
        until: parsed.validity().not_after.timestamp(),
        leaf,
        issuer,
    })
}

/// The SVG logo in a logotype extension's value (RFC 3709 §4.1): the
/// first `data:image/svg+xml` URI, decoded, and inflated when gzipped.
pub fn logo_of(value: &[u8]) -> Option<Vec<u8>> {
    let mut found = Vec::new();
    ia5_strings(value, 0, &mut found);
    let uri = found.into_iter().find(|s| s.to_ascii_lowercase().starts_with("data:image/svg+xml"))?;
    let url = data_url::DataUrl::process(&uri).ok()?;
    let (bytes, _) = url.decode_to_vec().ok()?;
    if bytes.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read as _;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .take(MAX_SVG as u64 + 1)
            .read_to_end(&mut out)
            .ok()?;
        (out.len() <= MAX_SVG).then_some(out)
    } else {
        (bytes.len() <= MAX_SVG).then_some(bytes)
    }
}

/// Every IA5String in DER `input`, at any depth (up to 16 levels).
fn ia5_strings(mut input: &[u8], depth: usize, out: &mut Vec<String>) {
    if depth > 16 {
        return;
    }
    while input.len() >= 2 {
        let tag = input[0];
        let (len, header) = match input[1] {
            n if n < 0x80 => (n as usize, 2),
            0x81 if input.len() >= 3 => (input[2] as usize, 3),
            0x82 if input.len() >= 4 => (((input[2] as usize) << 8) | input[3] as usize, 4),
            0x83 if input.len() >= 5 => (((input[2] as usize) << 16) | ((input[3] as usize) << 8) | input[4] as usize, 5),
            _ => return,
        };
        let Some(content) = input.get(header..header + len) else { return };
        if tag == 0x16 {
            if let Ok(s) = std::str::from_utf8(content) {
                out.push(s.to_owned());
            }
        } else if tag & 0x20 != 0 {
            // Constructed: SEQUENCE, SET, or a context tag around them.
            ia5_strings(content, depth + 1, out);
        } else if tag == 0x04 {
            // The extension's value is itself DER inside an OCTET STRING.
            ia5_strings(content, depth + 1, out);
        }
        input = &input[header + len..];
    }
}

/// Whether an SVG keeps to what BIMI's SVG Tiny PS allows that matters
/// here: nothing that runs, nothing from outside it.
fn tiny_ps(svg: &str) -> bool {
    let lower = svg.to_ascii_lowercase();
    let forbidden = ["<script", "<foreignobject", "<image", "<iframe", "javascript:", "@import", "url(http", "url(//"];
    if forbidden.iter().any(|f| lower.contains(f)) {
        return false;
    }
    // Links may name only something inside the image (`#id`).
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("href") {
        let after = rest[at + 4..].trim_start();
        let Some(after) = after.strip_prefix('=') else {
            rest = &rest[at + 4..];
            continue;
        };
        let value = after.trim_start().trim_start_matches(['"', '\'']);
        if !value.starts_with('#') {
            return false;
        }
        rest = &rest[at + 4..];
    }
    lower.contains("<svg")
}

/// The logo drawn as a `LOGO_SIZE` square PNG, centered, with nothing it
/// names loaded.
pub fn rasterize(svg: &[u8]) -> Result<Vec<u8>> {
    ensure!(svg.len() <= MAX_SVG, t!("bimi-the-logo-is-too-large"));
    let text = std::str::from_utf8(svg).map_err(|_| anyhow::anyhow!(t!("bimi-the-logo-cant-be-drawn")))?;
    ensure!(tiny_ps(text), t!("bimi-the-logo-isnt-svg-tiny-ps"));
    let options = resvg::usvg::Options {
        resources_dir: None,
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(svg, &options).map_err(|_| anyhow::anyhow!(t!("bimi-the-logo-cant-be-drawn")))?;
    let size = tree.size();
    let (w, h) = (size.width().max(1.), size.height().max(1.));
    let side = LOGO_SIZE as f32;
    let scale = side / w.max(h);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(LOGO_SIZE, LOGO_SIZE).with_context(|| t!("bimi-the-logo-cant-be-drawn"))?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale)
        .post_translate((side - w * scale) / 2., (side - h * scale) / 2.);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|_| anyhow::anyhow!(t!("bimi-the-logo-cant-be-drawn")))
}

/// The author domain of a From header's address.
pub fn author_domain(from: &str) -> Option<String> {
    let address = crate::sync::smtp::recipients(from).ok()?.into_iter().next()?.1;
    let (_, domain) = address.rsplit_once('@')?;
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    (!domain.is_empty()).then_some(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYPAL: &[u8] = include_bytes!("../../tests/fixtures/bimi/paypal-vmc.pem");
    const SSLCOM: &[u8] = include_bytes!("../../tests/fixtures/bimi/sslcom-cmc.pem");
    /// 2026-12-01 and 2026-01-01, in Unix seconds.
    const DECEMBER_2026: i64 = 1_796_083_200;
    const JANUARY_2026: i64 = 1_767_225_600;

    #[test]
    fn records_are_read_with_their_tags() {
        let r = parse_record("v=BIMI1; l=https://a.example/logo.svg; a=https://a.example/vmc.pem;").unwrap();
        assert_eq!(r.location, "https://a.example/logo.svg");
        assert_eq!(r.authority, "https://a.example/vmc.pem");
        // Split TXT strings, joined with a space.
        let r = parse_record("v=BIMI1;l=https://www.apple.com/bimi/v2/apple.svg; a=https://www.apple.com/bimi/v2/ap ple.pem;").unwrap();
        assert_eq!(r.authority, "https://www.apple.com/bimi/v2/apple.pem");
        let r = parse_record("v=BIMI1; l=https://bimigroup.org/bimi-sq.svg; a=").unwrap();
        assert!(r.authority.is_empty());
        assert!(parse_record("v=spf1 -all").is_none());
        assert!(parse_record("l=https://x/; v=BIMI1").is_none());
    }

    #[test]
    fn only_enforced_dmarc_passes_are_eligible() {
        assert!(eligible(Some("pass"), Some("reject")));
        assert!(eligible(Some("pass"), Some("quarantine")));
        assert!(!eligible(Some("pass"), Some("none")));
        assert!(!eligible(Some("fail"), Some("reject")));
        assert!(!eligible(None, None));
    }

    #[test]
    fn a_verified_mark_certificate_vouches_for_its_domains_logo() {
        let mark = verify(PAYPAL, &["paypal.com"], DECEMBER_2026).unwrap();
        assert_eq!(mark.kind, MarkKind::Verified);
        assert_eq!(mark.holder, "PayPal, Inc.");
        assert!(mark.until > DECEMBER_2026);
        let png = rasterize(&mark.svg).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        // Another domain's mail can't use it.
        assert!(verify(PAYPAL, &["paypal.example"], DECEMBER_2026).is_err());
        // Nor before it was issued.
        assert!(verify(PAYPAL, &["paypal.com"], JANUARY_2026).is_err());
    }

    #[test]
    fn common_mark_certificates_are_told_apart_and_expire() {
        let mark = verify(SSLCOM, &["ssl.com"], JANUARY_2026).unwrap();
        assert_eq!(mark.kind, MarkKind::Common);
        assert!(verify(SSLCOM, &["ssl.com"], DECEMBER_2026).is_err(), "expired in June 2026");
    }

    #[test]
    fn a_chain_to_another_root_is_refused() {
        // Only the certificate, with its issuers left out, and no root
        // of the authorities to reach: not vouched for.
        let only = X509::stack_from_pem(PAYPAL).unwrap().remove(0).to_pem().unwrap();
        assert!(verify_with(PAYPAL, &["paypal.com"], DECEMBER_2026, &[]).is_err());
        assert!(verify(&only, &["paypal.com"], DECEMBER_2026).is_err());
    }

    #[test]
    fn logos_may_not_reach_outside_themselves() {
        assert!(tiny_ps(r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#a"/></svg>"##));
        assert!(!tiny_ps(r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="other.svg#a"/></svg>"##));
        assert!(tiny_ps(r##"<svg xmlns="http://www.w3.org/2000/svg"><linearGradient xlink:href="#g"/></svg>"##));
        assert!(!tiny_ps(r#"<svg><a href="https://evil.example/">x</a></svg>"#));
        assert!(!tiny_ps(r#"<svg><script>alert(1)</script></svg>"#));
        assert!(!tiny_ps(r#"<svg><image href="file:///etc/passwd"/></svg>"#));
        assert!(!tiny_ps(r#"<svg><style>@import url(http://x/a.css);</style></svg>"#));
        assert!(rasterize(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>"#).is_ok());
    }

    #[test]
    fn author_domains_come_from_the_from_address() {
        assert_eq!(author_domain("PayPal <service@PayPal.com>").as_deref(), Some("paypal.com"));
        assert_eq!(author_domain("nobody").as_deref(), None);
    }
}
