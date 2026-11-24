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
    let parsed = url::Url::parse(url).with_context(|| format!("{url:?} isn't a valid address."))?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        "Revocation is fetched over HTTP only, not {}.",
        parsed.scheme()
    );
    ensure!(parsed.host().is_some(), "{url:?} names no server.");
    if crate::sync::public_net::names_local_address(&parsed) {
        bail!("{url:?} is on a local network.");
    }
    Ok(parsed)
}

/// Reads `reader` to its end, refusing more than `limit` bytes.
fn bounded(reader: impl std::io::Read, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "The revocation server's answer is too large.");
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
        ensure!(status.is_success(), "{url} answered {status}.");
        if response.content_length().is_some_and(|n| n > MAX_RESPONSE) {
            bail!("The revocation server's answer is too large.");
        }
        bounded(response, MAX_RESPONSE)
    }
}

impl RevocationFetcher for HttpFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let at = checked_url(url)?;
        let response = self.client.get(at).send().with_context(|| format!("Couldn't fetch {url}."))?;
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
            .with_context(|| format!("Couldn't ask {url}."))?;
        self.body(url, response)
    }
}

/// What revocation checking found for one certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Its issuer's current list doesn't name it (RFC 5280 §5), or its
    /// OCSP responder says good (RFC 6960 §2.2).
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

/// RFC 5280 §6.3.3: whether `crl` is `issuer`'s and still current at
/// `now`: issued by it (same name, signature by its key), and not past
/// its nextUpdate. A list without nextUpdate is taken as current.
pub fn current_from(
    crl: &openssl::x509::X509CrlRef,
    issuer: &openssl::x509::X509Ref,
    now: i64,
) -> bool {
    let same_name = crl.issuer_name().to_der().ok() == issuer.subject_name().to_der().ok();
    let signed = issuer.public_key().is_ok_and(|key| crl.verify(&key).unwrap_or(false));
    let fresh = crl.next_update().and_then(unix).is_none_or(|next| now < next);
    same_name && signed && fresh
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
        .context("The revocation server's answer isn't a revocation list.")
}

/// RFC 5280 §6.3: `cert`'s status by its issuer's CRL. A current one kept
/// in `cache` is used; else each distribution point is fetched (only with
/// `fetcher`, which is how the user asking is told apart from reading)
/// until one gives a list that is the issuer's and current, which is then
/// kept. `urls` overrides the certificate's own points (tests).
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
        return Status::Unknown("Its certificate names no revocation list.".into());
    }
    let status = |crl: &openssl::x509::X509CrlRef| match crl.get_by_cert(&cert.to_owned()) {
        openssl::x509::CrlStatus::Revoked(_) => Status::Revoked,
        _ => Status::Good,
    };
    for url in &urls {
        if let Some(crl) = cache.fresh(url, issuer, now) {
            return status(&crl);
        }
    }
    let Some(fetcher) = fetcher else {
        return Status::Unknown("Its revocation list hasn't been fetched.".into());
    };
    let mut why = String::from("No revocation list could be fetched.");
    for url in &urls {
        match fetcher.get(url).and_then(|bytes| parse_crl(&bytes)) {
            Ok(crl) if current_from(&crl, issuer, now) => {
                let _ = cache.keep(url, &crl);
                return status(&crl);
            }
            Ok(_) => why = format!("{url} gave a list its issuer didn't sign, or an outdated one."),
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
        return unknown("the request couldn't be made");
    };
    let answer = match fetcher.post_ocsp(url, &request) {
        Ok(answer) => answer,
        Err(e) => return Status::Unknown(format!("{e:#}")),
    };
    let Ok(response) = OcspResponse::from_der(&answer) else {
        return unknown("the answer isn't an OCSP response");
    };
    if response.status() != OcspResponseStatus::SUCCESSFUL {
        return unknown("the responder couldn't answer");
    }
    let Ok(basic) = response.basic() else {
        return unknown("the answer is incomplete");
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
        return unknown("the answer isn't signed by the certificate's issuer");
    }
    let Ok(id) = id() else {
        return unknown("the request couldn't be made");
    };
    let Some(status) = basic.find_status(&id) else {
        return unknown("the answer isn't about this certificate");
    };
    if status.check_validity(OCSP_SLACK, Some(OCSP_MAX_AGE)).is_err() {
        return unknown("the answer is out of date");
    }
    match status.status {
        OcspCertStatus::GOOD => Status::Good,
        OcspCertStatus::REVOKED => Status::Revoked,
        _ => unknown("the responder doesn't know this certificate"),
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
