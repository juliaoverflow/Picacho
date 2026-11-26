//! Revocation fetched from the network, when the user asks: certificate
//! revocation lists from a certificate's CRL distribution points (RFC 5280
//! §4.2.1.13) and answers from its OCSP responder (RFC 6960).
//!
//! Asking either tells a third party that a message by that certificate
//! was opened, so nothing here runs while reading: only from the message's
//! details, as "Check sender" does. Every request goes through a
//! `RevocationFetcher`, which tests replace with a fake.
use anyhow::{Context, Result, bail, ensure};

/// The most a CRL or OCSP answer may be. A large authority's CRL runs to
/// a megabyte or two; past this, the server is sending without end.
pub const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

/// Fetches revocation data: a CRL by GET (RFC 5280 §4.2.1.13), and an OCSP
/// answer by POST (RFC 6960 Appendix A.1).
pub trait RevocationFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>>;
    fn post_ocsp(&self, url: &str, der: &[u8]) -> Result<Vec<u8>>;
}

/// A URL revocation data may be fetched from: http: or https: only. CRLs
/// and OCSP answers are signed (RFC 5280 §5, RFC 6960 §4.2.1), so they are
/// normally served over plain HTTP; the signature, not the transport,
/// vouches for them. A host given as an address on this computer or its
/// network is refused, as for any address a stranger's certificate chose.
pub fn checked_url(url: &str) -> Result<url::Url> {
    let parsed = url::Url::parse(url).with_context(|| t!("manage-smime-revocation-isnt-a-valid-address", url = format!("{:?}", url)))?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        t!("manage-smime-revocation-revocation-is-fetched-over-http-only", scheme = parsed.scheme())
    );
    ensure!(parsed.host().is_some(), t!("manage-smime-revocation-names-no-server", url = format!("{:?}", url)));
    if crate::sync::public_net::reaches_inside(&parsed) {
        bail!(t!("manage-smime-revocation-is-on-a-local-network", url = format!("{:?}", url)));
    }
    Ok(parsed)
}

/// Reads `reader` to its end, refusing more than `limit` bytes.
fn bounded(reader: impl std::io::Read, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, t!("manage-smime-revocation-the-revocation-servers-answer-is-too"));
    Ok(bytes)
}

/// The fetcher the app uses: reqwest with short timeouts, only to servers
/// on the internet (`public_net::PublicOnly`), and a few redirects that
/// stay on http: or https: and off local networks.
pub struct HttpFetcher {
    client: reqwest::blocking::Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .dns_resolver(std::sync::Arc::new(crate::sync::public_net::PublicOnly))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 || checked_url(attempt.url().as_str()).is_err() {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self { client })
    }
    fn body(&self, url: &str, response: reqwest::blocking::Response) -> Result<Vec<u8>> {
        let status = response.status();
        ensure!(status.is_success(), t!("manage-smime-revocation-answered", url = url, status = status));
        if response.content_length().is_some_and(|n| n > MAX_RESPONSE) {
            bail!(t!("manage-smime-revocation-the-revocation-servers-answer-is-too"));
        }
        bounded(response, MAX_RESPONSE)
    }
}

impl RevocationFetcher for HttpFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let at = checked_url(url)?;
        let response = self.client.get(at).send().with_context(|| t!("manage-smime-revocation-couldnt-fetch", url = url))?;
        self.body(url, response)
    }
    fn post_ocsp(&self, url: &str, der: &[u8]) -> Result<Vec<u8>> {
        let at = checked_url(url)?;
        let response = self
            .client
            .post(at)
            .header(reqwest::header::CONTENT_TYPE, "application/ocsp-request")
            .header(reqwest::header::ACCEPT, "application/ocsp-response")
            .body(der.to_vec())
            .send()
            .with_context(|| t!("manage-smime-revocation-couldnt-ask", url = url))?;
        self.body(url, response)
    }
}

/// What revocation checking found for one certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A current, complete list of its issuer's that covers it doesn't
    /// name it (RFC 5280 §5, §6.3.3), or its OCSP responder says good
    /// (RFC 6960 §2.2).
    Good,
    /// Its issuer revoked it.
    Revoked,
    /// Nothing current could be had: no list or responder named, none
    /// reachable, or one not signed by its issuer. Never taken as revoked.
    Unknown(String),
}

/// The CRL distribution points' URLs (RFC 5280 §4.2.1.13) of `cert`: the
/// fullName URIs over HTTP. LDAP and others are left out.
pub fn crl_urls(cert: &openssl::x509::X509Ref) -> Vec<String> {
    let Some(points) = cert.crl_distribution_points() else {
        return Vec::new();
    };
    points
        .iter()
        .filter_map(|p| p.distpoint()?.fullname())
        .flat_map(|names| names.iter().filter_map(|n| n.uri().map(str::to_owned)))
        .filter(|u| checked_url(u).is_ok())
        .collect()
}

/// Seconds since 1970 of an ASN.1 time.
fn unix(time: &openssl::asn1::Asn1TimeRef) -> Option<i64> {
    let epoch = openssl::asn1::Asn1Time::from_unix(0).ok()?;
    let d = epoch.diff(time).ok()?;
    Some(i64::from(d.days) * 86_400 + i64::from(d.secs))
}

/// Clock difference allowed for a list's thisUpdate.
const CRL_SLACK: i64 = 5 * 60;

/// RFC 5280 §6.3.3: whether `crl` is `issuer`'s and current at `now`:
/// issued by it (same name, signature by its key, a key it may sign lists
/// with), already issued (thisUpdate, give or take `CRL_SLACK`), and not
/// past its nextUpdate. A list without nextUpdate, which RFC 5280 §5.1.2.5
/// requires, can't be told to be current. Whether it speaks for a given
/// certificate is `scope`'s to say.
pub fn current_from(
    crl: &openssl::x509::X509CrlRef,
    issuer: &openssl::x509::X509Ref,
    now: i64,
) -> bool {
    let same_name = crl.issuer_name().to_der().ok() == issuer.subject_name().to_der().ok();
    let signed = issuer.public_key().is_ok_and(|key| crl.verify(&key).unwrap_or(false));
    let issued = unix(crl.last_update()).is_some_and(|this| this <= now + CRL_SLACK);
    let fresh = crl.next_update().and_then(unix).is_some_and(|next| now < next);
    same_name && signed && issued && fresh && signs_crls(issuer)
}

/// RFC 5280 §4.2.1.3: an issuer whose key usage is given signs lists only
/// with cRLSign among it.
fn signs_crls(issuer: &openssl::x509::X509Ref) -> bool {
    let Ok(der) = issuer.to_der() else { return false };
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(&der) else {
        return false;
    };
    match cert.key_usage() {
        Ok(Some(usage)) => usage.value.crl_sign(),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// How far a list speaks for a certificate (RFC 5280 §5.2.4, §5.2.5,
/// §6.3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// It covers the certificate for every reason: one it doesn't name
    /// isn't revoked.
    Complete,
    /// It covers the certificate for some reasons only (`onlySomeReasons`,
    /// or a distribution point's `reasons`): one it names is revoked, but
    /// one it doesn't may be revoked for another.
    Partial,
    /// It isn't for the certificate, or can't be told to be: another
    /// partition of the issuer's lists, another kind of certificate, a
    /// delta list, an indirect one, or one with a critical extension not
    /// understood here. Nothing it says counts.
    Unrelated(String),
}

/// `crl`'s scope for `cert`, fetched from `url`: whether the list covers
/// the certificate's distribution point (the one `url` is in, else any of
/// the certificate's) and its kind, and for which reasons. A list without
/// an issuing distribution point covers all its issuer's certificates for
/// every reason, but a distribution point with reasons of its own limits
/// what it may say. Only a complete list (not a delta, §5.2.4) applies;
/// indirect lists (§5.2.5's indirectCRL, a point's cRLIssuer) aren't
/// supported.
pub fn scope(cert: &openssl::x509::X509Ref, crl: &openssl::x509::X509CrlRef, url: &str) -> Scope {
    use x509_parser::extensions::{DistributionPointName, GeneralName, ParsedExtension};
    use x509_parser::oid_registry::{
        OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER, OID_X509_EXT_CRL_NUMBER, OID_X509_EXT_DELTA_CRL_INDICATOR,
        OID_X509_EXT_ISSUER_DISTRIBUTION_POINT,
    };
    let unrelated = |why: &str| Scope::Unrelated(why.to_owned());
    let (Ok(cert_der), Ok(crl_der)) = (cert.to_der(), crl.to_der()) else {
        return unrelated(t!("manage-smime-revocation-list-unreadable"));
    };
    let (Ok((_, cert)), Ok((_, crl))) =
        (x509_parser::parse_x509_certificate(&cert_der), x509_parser::parse_x509_crl(&crl_der))
    else {
        return unrelated(t!("manage-smime-revocation-list-unreadable"));
    };
    // The certificate's distribution points, and the one `url` is in.
    let points: Vec<_> = cert
        .extensions()
        .iter()
        .find_map(|e| match e.parsed_extension() {
            ParsedExtension::CRLDistributionPoints(points) => Some(points.points.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let names_url = |names: &[GeneralName]| names.iter().any(|n| matches!(n, GeneralName::URI(u) if *u == url));
    let point = points.iter().find(|p| {
        matches!(&p.distribution_point, Some(DistributionPointName::FullName(names)) if names_url(names))
    });
    if point.is_some_and(|p| p.crl_issuer.is_some()) {
        return unrelated(t!("manage-smime-revocation-list-indirect"));
    }
    let mut partial = point.is_some_and(|p| p.reasons.is_some());
    let mut idp = None;
    for ext in crl.extensions() {
        if ext.oid == OID_X509_EXT_DELTA_CRL_INDICATOR {
            return unrelated(t!("manage-smime-revocation-list-delta"));
        }
        if ext.oid == OID_X509_EXT_ISSUER_DISTRIBUTION_POINT {
            match ext.parsed_extension() {
                ParsedExtension::IssuingDistributionPoint(found) => idp = Some(found),
                _ => return unrelated(t!("manage-smime-revocation-list-unreadable")),
            }
            continue;
        }
        let known = ext.oid == OID_X509_EXT_CRL_NUMBER || ext.oid == OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER;
        // §5.2: a list with a critical extension not understood isn't used.
        if ext.critical && !known {
            return unrelated(t!("manage-smime-revocation-list-unsupported"));
        }
    }
    if let Some(idp) = idp {
        if idp.indirect_crl {
            return unrelated(t!("manage-smime-revocation-list-indirect"));
        }
        let ca = cert.is_ca();
        if idp.only_contains_attribute_certs || (idp.only_contains_ca_certs && !ca) || (idp.only_contains_user_certs && ca) {
            return unrelated(t!("manage-smime-revocation-list-for-other-certificates"));
        }
        match &idp.distribution_point {
            None => {}
            // §6.3.3 (b)(2)(i): one of its names must be one of the
            // certificate's point's (the one fetched from, if it is one).
            Some(DistributionPointName::FullName(names)) => {
                let shares = |p: &&x509_parser::extensions::CRLDistributionPoint| {
                    matches!(&p.distribution_point, Some(DistributionPointName::FullName(theirs))
                        if theirs.iter().any(|n| names.contains(n)))
                };
                let covered = match point {
                    Some(p) => shares(&p),
                    None => points.iter().any(|p| shares(&p)),
                };
                if !covered {
                    return unrelated(t!("manage-smime-revocation-list-for-other-certificates"));
                }
            }
            Some(DistributionPointName::NameRelativeToCRLIssuer(_)) => {
                return unrelated(t!("manage-smime-revocation-list-unsupported"));
            }
        }
        partial |= idp.only_some_reasons.is_some();
    }
    if partial { Scope::Partial } else { Scope::Complete }
}

/// What `crl`, current and its issuer's, says of `cert` (fetched from
/// `url`): revoked if it names it and covers it; good only if it covers it
/// completely; otherwise unknown, with why.
fn judged(cert: &openssl::x509::X509Ref, crl: &openssl::x509::X509CrlRef, url: &str) -> Status {
    let scope = scope(cert, crl, url);
    if let Scope::Unrelated(why) = scope {
        return Status::Unknown(t!("manage-smime-revocation-list-doesnt-apply", url = url, why = why));
    }
    match crl.get_by_cert(&cert.to_owned()) {
        openssl::x509::CrlStatus::Revoked(_) => Status::Revoked,
        _ if scope == Scope::Complete => Status::Good,
        _ => Status::Unknown(t!("manage-smime-revocation-list-only-some-reasons", url = url)),
    }
}

/// Where fetched lists are kept: one file per URL, as PEM, named by the
/// URL's SHA-256. Reused until their nextUpdate (RFC 5280 §5.1.2.5).
pub struct CrlCache {
    pub folder: std::path::PathBuf,
}

impl CrlCache {
    fn path(&self, url: &str) -> std::path::PathBuf {
        let digest = openssl::sha::sha256(url.as_bytes());
        let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        self.folder.join(format!("{name}.pem"))
    }
    /// The list kept for `url`, if it is `issuer`'s and still current.
    pub fn fresh(&self, url: &str, issuer: &openssl::x509::X509Ref, now: i64) -> Option<openssl::x509::X509Crl> {
        let pem = std::fs::read(self.path(url)).ok()?;
        let crl = openssl::x509::X509Crl::from_pem(&pem).ok()?;
        current_from(&crl, issuer, now).then_some(crl)
    }
    fn keep(&self, url: &str, crl: &openssl::x509::X509CrlRef) -> Result<()> {
        std::fs::create_dir_all(&self.folder)?;
        std::fs::write(self.path(url), crl.to_pem()?)?;
        Ok(())
    }
}

/// A list as served: DER, as RFC 5280 §4.2.1.13 says it is over HTTP, or
/// PEM, as some servers send it anyway.
fn parse_crl(bytes: &[u8]) -> Result<openssl::x509::X509Crl> {
    openssl::x509::X509Crl::from_der(bytes)
        .or_else(|_| openssl::x509::X509Crl::from_pem(bytes))
        .with_context(|| t!("manage-smime-revocation-the-revocation-servers-answer-isnt-a"))
}

/// RFC 5280 §6.3: `cert`'s status by its issuer's CRL. A current one kept
/// in `cache` is used; else each distribution point is fetched (only with
/// `fetcher`, which is how the user asking is told apart from reading)
/// until one gives a list that is the issuer's, current and covers the
/// certificate (`scope`), which is then kept. Only a list that covers it
/// completely can say it isn't revoked. `urls` overrides the certificate's
/// own points (tests).
pub fn crl_status(
    cert: &openssl::x509::X509Ref,
    issuer: &openssl::x509::X509Ref,
    urls: &[String],
    cache: &CrlCache,
    fetcher: Option<&dyn RevocationFetcher>,
    now: i64,
) -> Status {
    let urls = if urls.is_empty() { crl_urls(cert) } else { urls.to_vec() };
    if urls.is_empty() {
        return Status::Unknown(t!("manage-smime-revocation-its-certificate-names-no-revocation-list").into());
    }
    let mut why = None;
    for url in &urls {
        if let Some(crl) = cache.fresh(url, issuer, now) {
            match judged(cert, &crl, url) {
                Status::Unknown(w) => why = Some(w),
                known => return known,
            }
        }
    }
    let Some(fetcher) = fetcher else {
        return Status::Unknown(
            why.unwrap_or_else(|| t!("manage-smime-revocation-its-revocation-list-hasnt-been-fetched").into()),
        );
    };
    let mut why = String::from(t!("manage-smime-revocation-no-revocation-list-could-be-fetched"));
    for url in &urls {
        match fetcher.get(url).and_then(|bytes| parse_crl(&bytes)) {
            Ok(crl) if current_from(&crl, issuer, now) => {
                let status = judged(cert, &crl, url);
                // Kept only when it speaks for this certificate: one for
                // another partition, or a delta, isn't this URL's list.
                if !matches!(scope(cert, &crl, url), Scope::Unrelated(_)) {
                    let _ = cache.keep(url, &crl);
                }
                match status {
                    Status::Unknown(w) => why = w,
                    known => return known,
                }
            }
            Ok(_) => why = t!("manage-smime-revocation-gave-a-list-its-issuer-didnt", url = url),
            Err(e) => why = format!("{e:#}"),
        }
    }
    Status::Unknown(why)
}

/// The OCSP responders' URLs of `cert` (its Authority Information Access,
/// RFC 5280 §4.2.2.1, id-ad-ocsp), over HTTP.
pub fn ocsp_urls(cert: &openssl::x509::X509Ref) -> Vec<String> {
    cert.ocsp_responders()
        .map(|list| list.iter().map(|u| u.to_string()).filter(|u| checked_url(u).is_ok()).collect())
        .unwrap_or_default()
}

/// How old an OCSP answer may be and still count (RFC 6960 §4.2.2.1:
/// thisUpdate). openssl's crate has no way to put a nonce in the request
/// (RFC 8954), so freshness is what keeps an old "good" from being
/// replayed after a revocation.
const OCSP_MAX_AGE: u32 = 7 * 86_400;
/// Clock difference allowed either way.
const OCSP_SLACK: u32 = 5 * 60;

/// RFC 6960: `cert`'s status from its issuer's OCSP responder at `url`.
/// The answer must be signed by the issuer or a responder it delegated to
/// (§4.2.2.2, which OCSP_basic_verify checks against `issuer`), be about
/// this very certificate, and be recent; anything else is Unknown.
pub fn ocsp_status(
    cert: &openssl::x509::X509Ref,
    issuer: &openssl::x509::X509Ref,
    url: &str,
    fetcher: &dyn RevocationFetcher,
) -> Status {
    use openssl::ocsp::{OcspCertId, OcspCertStatus, OcspFlag, OcspRequest, OcspResponse, OcspResponseStatus};
    let unknown = |why: &str| Status::Unknown(format!("{url}: {why}"));
    let id = || OcspCertId::from_cert(openssl::hash::MessageDigest::sha1(), cert, issuer);
    let request = (|| -> Result<Vec<u8>> {
        let mut request = OcspRequest::new()?;
        request.add_id(id()?)?;
        Ok(request.to_der()?)
    })();
    let Ok(request) = request else {
        return unknown(t!("manage-smime-revocation-the-request-couldnt-be-made"));
    };
    let answer = match fetcher.post_ocsp(url, &request) {
        Ok(answer) => answer,
        Err(e) => return Status::Unknown(format!("{e:#}")),
    };
    let Ok(response) = OcspResponse::from_der(&answer) else {
        return unknown(t!("manage-smime-revocation-the-answer-isnt-an-ocsp-response"));
    };
    if response.status() != OcspResponseStatus::SUCCESSFUL {
        return unknown(t!("manage-smime-revocation-the-responder-couldnt-answer"));
    }
    let Ok(basic) = response.basic() else {
        return unknown(t!("manage-smime-revocation-the-answer-is-incomplete"));
    };
    // §4.2.2.2: signed by the issuer itself, or by a certificate it
    // issued for answering OCSP (id-kp-OCSPSigning).
    let verified = (|| -> Result<()> {
        let mut store = openssl::x509::store::X509StoreBuilder::new()?;
        store.add_cert(issuer.to_owned())?;
        let mut certs = openssl::stack::Stack::new()?;
        certs.push(issuer.to_owned())?;
        basic.verify(&certs, &store.build(), OcspFlag::TRUST_OTHER)?;
        Ok(())
    })();
    if verified.is_err() {
        return unknown(t!("manage-smime-revocation-the-answer-isnt-signed-by-the"));
    }
    let Ok(id) = id() else {
        return unknown(t!("manage-smime-revocation-the-request-couldnt-be-made"));
    };
    let Some(status) = basic.find_status(&id) else {
        return unknown(t!("manage-smime-revocation-the-answer-isnt-about-this-certificate"));
    };
    if status.check_validity(OCSP_SLACK, Some(OCSP_MAX_AGE)).is_err() {
        return unknown(t!("manage-smime-revocation-the-answer-is-out-of-date"));
    }
    match status.status {
        OcspCertStatus::GOOD => Status::Good,
        OcspCertStatus::REVOKED => Status::Revoked,
        _ => unknown(t!("manage-smime-revocation-the-responder-doesnt-know-this-certificate")),
    }
}

/// RFC 5280 §6.3 and RFC 6960, together: OCSP first where the certificate
/// names a responder (an answer about it alone, and current), else, or
/// when OCSP can't say, its issuer's CRL. Both need `fetcher`; without one
/// only a kept, current CRL is used.
pub fn status(
    cert: &openssl::x509::X509Ref,
    issuer: &openssl::x509::X509Ref,
    cache: &CrlCache,
    fetcher: Option<&dyn RevocationFetcher>,
    now: i64,
) -> Status {
    let mut ocsp = None;
    if let Some(fetcher) = fetcher {
        for url in ocsp_urls(cert) {
            match ocsp_status(cert, issuer, &url, fetcher) {
                Status::Unknown(why) => ocsp = Some(why),
                known => return known,
            }
        }
    }
    match crl_status(cert, issuer, &[], cache, fetcher, now) {
        Status::Unknown(why) => Status::Unknown(ocsp.unwrap_or(why)),
        known => known,
    }
}

/// A fetcher for tests: canned answers by URL, and a record of what was
/// asked. A URL with no answer fails, as an unreachable server does.
#[cfg(test)]
#[derive(Default)]
pub struct FakeFetcher {
    pub answers: std::collections::HashMap<String, Vec<u8>>,
    pub asked: std::sync::Mutex<Vec<String>>,
    pub posted: std::sync::Mutex<Vec<Vec<u8>>>,
}

#[cfg(test)]
impl FakeFetcher {
    pub fn with(answers: &[(&str, Vec<u8>)]) -> Self {
        Self {
            answers: answers.iter().map(|(u, b)| ((*u).to_owned(), b.clone())).collect(),
            ..Self::default()
        }
    }
    fn answer(&self, url: &str) -> Result<Vec<u8>> {
        checked_url(url)?;
        self.asked.lock().unwrap().push(url.to_owned());
        self.answers.get(url).cloned().with_context(|| format!("Couldn't fetch {url}."))
    }
}

#[cfg(test)]
impl RevocationFetcher for FakeFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        self.answer(url)
    }
    fn post_ocsp(&self, url: &str, der: &[u8]) -> Result<Vec<u8>> {
        self.posted.lock().unwrap().push(der.to_vec());
        self.answer(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocation_urls_are_http_or_https_only() {
        assert!(checked_url("http://crl.example.com/ca.crl").is_ok());
        assert!(checked_url("https://ocsp.example.com/").is_ok());
        for refused in ["ldap://ldap.example.com/cn=CA?certificateRevocationList", "file:///etc/passwd", "ftp://example.com/ca.crl", "not a url"] {
            assert!(checked_url(refused).is_err(), "{refused}");
        }
        let fake = FakeFetcher::with(&[("http://crl.example.com/ca.crl", b"crl".to_vec())]);
        assert!(fake.get("file:///etc/passwd").is_err());
        assert!(fake.asked.lock().unwrap().is_empty());
        assert_eq!(fake.get("http://crl.example.com/ca.crl").unwrap(), b"crl");
        assert!(fake.get("http://crl.example.com/other.crl").is_err());
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/smime/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }
    fn cert(name: &str) -> openssl::x509::X509 {
        openssl::x509::X509::from_pem(&fixture(name)).unwrap()
    }
    fn der_of(pem: &str) -> Vec<u8> {
        openssl::x509::X509Crl::from_pem(&fixture(pem)).unwrap().to_der().unwrap()
    }
    fn cache() -> CrlCache {
        CrlCache {
            folder: std::env::temp_dir().join(format!("picacho-crl-cache-{}", uuid::Uuid::new_v4())),
        }
    }
    const URL: &str = "http://crl.example.org/ca.crl";

    /// RFC 5280 §6.3: a list from the issuer names the revoked; an
    /// empty one is good; kept, it is used without fetching again.
    #[test]
    fn crls_are_fetched_checked_against_their_issuer_and_kept() {
        let (ca, bo, ana) = (cert("ca.pem"), cert("bo.pem"), cert("ana.pem"));
        let now = chrono::Utc::now().timestamp();
        let urls = [URL.to_owned()];
        let revoking = FakeFetcher::with(&[(URL, der_of("crl-bo-revoked.pem"))]);
        let kept = cache();
        assert_eq!(crl_status(&bo, &ca, &urls, &kept, Some(&revoking), now), Status::Revoked);
        assert_eq!(revoking.asked.lock().unwrap().len(), 1);
        // Kept: Ana is checked against the same list without a fetch.
        assert_eq!(crl_status(&ana, &ca, &urls, &kept, None, now), Status::Good);
        assert_eq!(revoking.asked.lock().unwrap().len(), 1);
        // An empty list: good.
        let empty = FakeFetcher::with(&[(URL, der_of("crl-empty.pem"))]);
        assert_eq!(crl_status(&bo, &ca, &urls, &cache(), Some(&empty), now), Status::Good);
        // PEM, as some servers send it, is read too.
        let pem = FakeFetcher::with(&[(URL, fixture("crl-bo-revoked.pem"))]);
        assert_eq!(crl_status(&bo, &ca, &urls, &cache(), Some(&pem), now), Status::Revoked);
        let _ = std::fs::remove_dir_all(&kept.folder);
    }

    /// A list not signed by the certificate's issuer, an outdated one, an
    /// unreachable server or none named: unknown, never revoked.
    #[test]
    fn crls_that_cant_be_trusted_or_had_are_unknown() {
        let (ca, bo, ana) = (cert("ca.pem"), cert("bo.pem"), cert("ana.pem"));
        let now = chrono::Utc::now().timestamp();
        let urls = [URL.to_owned()];
        let revoking = FakeFetcher::with(&[(URL, der_of("crl-bo-revoked.pem"))]);
        // Ana isn't the list's issuer: its signature doesn't check out.
        assert!(matches!(crl_status(&bo, &ana, &urls, &cache(), Some(&revoking), now), Status::Unknown(_)));
        // Far in the future, the list is past its nextUpdate.
        let later = now + 200 * 365 * 86_400;
        assert!(matches!(crl_status(&bo, &ca, &urls, &cache(), Some(&revoking), later), Status::Unknown(_)));
        // Nothing answers.
        let silent = FakeFetcher::default();
        assert!(matches!(crl_status(&bo, &ca, &urls, &cache(), Some(&silent), now), Status::Unknown(_)));
        // Not asked to fetch, and nothing kept.
        assert!(matches!(crl_status(&bo, &ca, &urls, &cache(), None, now), Status::Unknown(_)));
        // The fixtures name no distribution point of their own.
        assert!(crl_urls(&bo).is_empty());
        assert!(matches!(crl_status(&bo, &ca, &[], &cache(), Some(&revoking), now), Status::Unknown(_)));
    }

    /// RFC 5280 §5.2.4, §5.2.5, §6.3.3: a list signed by the issuer speaks
    /// only for the certificates and reasons it covers. Another partition
    /// of the issuer's lists (served at the certificate's point, by
    /// mistake or by someone in the way), a delta list, or one for CA
    /// certificates never says a certificate isn't revoked, and isn't kept;
    /// one limited to some reasons can say it is revoked, but not that it
    /// isn't.
    #[test]
    fn crls_speak_only_for_the_certificates_and_reasons_they_cover() {
        let (ca, cara, bo) = (cert("ca.pem"), cert("cara.pem"), cert("bo.pem"));
        let now = chrono::Utc::now().timestamp();
        let p1 = "http://crl.example.org/partition-1.crl";
        assert_eq!(crl_urls(&cara), vec![p1.to_owned()]);
        let unknown = |fixture: &str| {
            let fetcher = FakeFetcher::with(&[(p1, der_of(fixture))]);
            let kept = cache();
            let status = crl_status(&cara, &ca, &[], &kept, Some(&fetcher), now);
            let unkept = kept.fresh(p1, &ca, now).is_none();
            let _ = std::fs::remove_dir_all(&kept.folder);
            matches!(status, Status::Unknown(_)) && unkept
        };
        // Partition 2, signed by the same authority: not "good" for Cara,
        // whose point is partition 1, and not kept as partition 1's list.
        assert!(unknown("crl-partition-2-empty.pem"));
        assert!(unknown("crl-delta-empty.pem"));
        assert!(unknown("crl-ca-only-empty.pem"));
        // Its own partition: good when empty, revoked when it names her.
        let kept = cache();
        let own = FakeFetcher::with(&[(p1, der_of("crl-partition-1-empty.pem"))]);
        assert_eq!(crl_status(&cara, &ca, &[], &kept, Some(&own), now), Status::Good);
        assert!(kept.fresh(p1, &ca, now).is_some(), "kept");
        let revoking = FakeFetcher::with(&[(p1, der_of("crl-partition-1-cara-revoked.pem"))]);
        assert_eq!(crl_status(&cara, &ca, &[], &cache(), Some(&revoking), now), Status::Revoked);
        // Kept for partition 1, it doesn't speak for Bo, who names no point.
        assert!(matches!(crl_status(&bo, &ca, &[p1.to_owned()], &kept, None, now), Status::Unknown(_)));
        let _ = std::fs::remove_dir_all(&kept.folder);
        // Key compromise only: revoked when it names her, else unknown.
        let reasons = FakeFetcher::with(&[(p1, der_of("crl-reasons-empty.pem"))]);
        assert!(matches!(crl_status(&cara, &ca, &[], &cache(), Some(&reasons), now), Status::Unknown(_)));
        let reasons = FakeFetcher::with(&[(p1, der_of("crl-reasons-cara-revoked.pem"))]);
        assert_eq!(crl_status(&cara, &ca, &[], &cache(), Some(&reasons), now), Status::Revoked);
        // The scopes themselves.
        let crl = |f: &str| openssl::x509::X509Crl::from_pem(&fixture(f)).unwrap();
        assert_eq!(scope(&cara, &crl("crl-partition-1-empty.pem"), p1), Scope::Complete);
        assert_eq!(scope(&cara, &crl("crl-empty.pem"), p1), Scope::Complete);
        assert_eq!(scope(&cara, &crl("crl-reasons-empty.pem"), p1), Scope::Partial);
        assert!(matches!(scope(&cara, &crl("crl-partition-2-empty.pem"), p1), Scope::Unrelated(_)));
        // A list not yet issued isn't current.
        assert!(!current_from(&crl("crl-empty.pem"), &ca, now - 400 * 86_400));
    }

    /// An OCSP responder for tests: answers each request with `openssl
    /// ocsp`, signed by `signer`, from an index naming `revoked` (serials)
    /// as revoked and `known` as valid; others are unknown.
    struct Responder {
        signer: &'static str,
        revoked: Vec<String>,
        known: Vec<String>,
        /// Answers for this request instead (a stale or mismatched one).
        canned: Option<Vec<u8>>,
    }
    impl RevocationFetcher for Responder {
        fn get(&self, url: &str) -> Result<Vec<u8>> {
            bail!("no CRL at {url}")
        }
        fn post_ocsp(&self, url: &str, der: &[u8]) -> Result<Vec<u8>> {
            checked_url(url)?;
            if let Some(answer) = &self.canned {
                return Ok(answer.clone());
            }
            let dir = std::env::temp_dir().join(format!("picacho-ocsp-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir)?;
            let f = |n: &str| format!("{}/tests/fixtures/smime/{n}", env!("CARGO_MANIFEST_DIR"));
            let mut index = String::new();
            for serial in &self.revoked {
                index.push_str(&format!("R\t21261030230737Z\t261101000000Z\t{serial}\tunknown\t/CN=x\n"));
            }
            for serial in &self.known {
                index.push_str(&format!("V\t21261030230737Z\t\t{serial}\tunknown\t/CN=x\n"));
            }
            std::fs::write(dir.join("index.txt"), index)?;
            std::fs::write(dir.join("req.der"), der)?;
            let status = std::process::Command::new("timeout")
                .args(["30", "openssl", "ocsp", "-index"])
                .arg(dir.join("index.txt"))
                .args(["-rsigner", &f(&format!("{}.pem", self.signer)), "-rkey", &f(&format!("{}.key", self.signer))])
                .args(["-CA", &f("ca.pem"), "-ndays", "1", "-reqin"])
                .arg(dir.join("req.der"))
                .arg("-respout")
                .arg(dir.join("resp.der"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            ensure!(status.success(), "openssl ocsp failed");
            let answer = std::fs::read(dir.join("resp.der"))?;
            let _ = std::fs::remove_dir_all(&dir);
            Ok(answer)
        }
    }
    fn serial(c: &openssl::x509::X509) -> String {
        c.serial_number().to_bn().unwrap().to_hex_str().unwrap().to_string()
    }
    const OCSP: &str = "http://ocsp.example.org/";

    /// RFC 6960: an answer signed by the issuer about this certificate
    /// says good or revoked; one the responder doesn't know is unknown.
    #[test]
    fn ocsp_answers_signed_by_the_issuer_say_good_or_revoked() {
        let (ca, bo, ana) = (cert("ca.pem"), cert("bo.pem"), cert("ana.pem"));
        let responder = Responder { signer: "ca", revoked: vec![serial(&bo)], known: vec![serial(&ana)], canned: None };
        assert_eq!(ocsp_status(&bo, &ca, OCSP, &responder), Status::Revoked);
        assert_eq!(ocsp_status(&ana, &ca, OCSP, &responder), Status::Good);
        let none = Responder { signer: "ca", revoked: vec![], known: vec![], canned: None };
        assert!(matches!(ocsp_status(&ana, &ca, OCSP, &none), Status::Unknown(_)));
    }

    /// §4.2.2.2: an answer signed by anyone else, one about another
    /// certificate, or one that isn't OCSP at all: unknown, never revoked.
    #[test]
    fn ocsp_answers_that_cant_be_trusted_are_unknown() {
        let (ca, bo, ana) = (cert("ca.pem"), cert("bo.pem"), cert("ana.pem"));
        // Signed by Ana's key, not the authority's.
        let forged = Responder { signer: "ana", revoked: vec![serial(&bo)], known: vec![], canned: None };
        assert!(matches!(ocsp_status(&bo, &ca, OCSP, &forged), Status::Unknown(_)));
        // A genuine answer, but about Bo, given for Ana.
        let honest = Responder { signer: "ca", revoked: vec![serial(&bo)], known: vec![], canned: None };
        let request = {
            use openssl::ocsp::{OcspCertId, OcspRequest};
            let mut r = OcspRequest::new().unwrap();
            r.add_id(OcspCertId::from_cert(openssl::hash::MessageDigest::sha1(), &bo, &ca).unwrap()).unwrap();
            r.to_der().unwrap()
        };
        let about_bo = honest.post_ocsp(OCSP, &request).unwrap();
        let swapped = Responder { canned: Some(about_bo), ..honest };
        assert!(matches!(ocsp_status(&ana, &ca, OCSP, &swapped), Status::Unknown(_)));
        let junk = Responder { signer: "ca", revoked: vec![], known: vec![], canned: Some(b"<html>".to_vec()) };
        assert!(matches!(ocsp_status(&bo, &ca, OCSP, &junk), Status::Unknown(_)));
        // Unreachable.
        assert!(matches!(ocsp_status(&bo, &ca, OCSP, &FakeFetcher::default()), Status::Unknown(_)));
    }

    #[test]
    fn revocation_answers_are_read_up_to_the_cap() {
        let at_cap = vec![0u8; MAX_RESPONSE as usize];
        assert_eq!(bounded(&at_cap[..], MAX_RESPONSE).unwrap().len(), at_cap.len());
        let over = vec![0u8; MAX_RESPONSE as usize + 1];
        assert!(bounded(&over[..], MAX_RESPONSE).is_err());
    }

    /// The real fetcher against a server on this computer (allowed in
    /// tests only), which answers once and gives up after a deadline: a
    /// small answer is read, one past the cap refused, and a POST carries
    /// the OCSP media type (RFC 6960 Appendix A.1).
    #[test]
    fn http_fetcher_reads_small_answers_and_refuses_large_ones() {
        use std::io::{Read as _, Write as _};
        fn serve(body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/ca.crl", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let handle = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(_) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(10))
                        }
                        Err(e) => panic!("no request: {e}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..n]);
                }
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                String::from_utf8_lossy(&request).into_owned()
            });
            (url, handle)
        }
        let fetcher = HttpFetcher::new().unwrap();
        let (url, server) = serve(b"crl".to_vec());
        assert_eq!(fetcher.get(&url).unwrap(), b"crl");
        assert!(server.join().unwrap().starts_with("GET /ca.crl"));
        let (url, server) = serve(vec![0u8; MAX_RESPONSE as usize + 1]);
        assert!(fetcher.get(&url).is_err());
        server.join().unwrap();
        let (url, server) = serve(b"answer".to_vec());
        assert_eq!(fetcher.post_ocsp(&url, b"request").unwrap(), b"answer");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post") && request.contains("content-type: application/ocsp-request"));
    }
}
