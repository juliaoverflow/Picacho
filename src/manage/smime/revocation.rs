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
