//! What the Gmail API (`gmail.rs`) and Microsoft Graph (`graph.rs`) share:
//! JSON over HTTPS with the account's OAuth access token, and the same
//! shape of errors (`{"error": {"code", "message"}}`).
//!
//! A refused token (401) is `AccessFailure::Authentication`, so the
//! account asks to sign in again; a server limiting requests (429) or busy
//! (503) is reported as `[THROTTLED]`, so the account's background work
//! pauses as it does for IMAP servers that say so (`imap::throttled`).
use crate::manage::status::AccessFailure;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::time::Duration;

/// How long an ordinary request may take, and a download or upload.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);
/// The most of an answer read into memory: a message's source at most.
const MOST: u64 = 64 * 1024 * 1024;

/// The server has no such thing (404): a message or folder gone meanwhile.
#[derive(Debug)]
pub struct NotFound;
impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("sync-rest-it-no-longer-exists-at-the"))
    }
}
impl std::error::Error for NotFound {}

/// Whether `e` says the thing asked about is gone (`NotFound`).
pub fn not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotFound>().is_some()
}

/// One API's address and the token that signs in to it.
pub struct Rest {
    http: reqwest::blocking::Client,
    /// `https://gmail.googleapis.com`, `https://graph.microsoft.com`; a
    /// loopback server in tests.
    pub base: String,
    token: String,
    /// The service's name, for messages: "Gmail", "Microsoft 365".
    service: &'static str,
    /// Sent with every request (Graph's `Prefer: IdType="ImmutableId"`).
    headers: Vec<(&'static str, String)>,
    /// The TLS settings the client was made with (the system's roots and
    /// any the account trusts), for clients of other hosts the API names
    /// (Graph's upload sessions).
    pub tls: native_tls::TlsConnector,
}

impl Rest {
    pub fn new(base: &str, token: &str, service: &'static str, tls: native_tls::TlsConnector) -> Result<Self> {
        if token.is_empty() {
            return Err(anyhow::Error::new(AccessFailure::Authentication)
                .context(t!("sync-rest-your-sign-in-expired-reconnect-the")));
        }
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls.clone())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(12))
            // The token goes to the API's own host only.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .https_only(!cfg!(test))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            service,
            headers: Vec::new(),
            tls,
        })
    }
    /// Sends `name: value` with every request.
    pub fn with_header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    /// The address of `path` (which starts with `/`), or `path` itself
    /// where it is a whole address the API gave (a next page's link) on the
    /// same host.
    pub fn url(&self, path: &str) -> Result<String> {
        if path.starts_with('/') {
            return Ok(format!("{}{path}", self.base));
        }
        let given = url::Url::parse(path)?;
        let base = url::Url::parse(&self.base)?;
        anyhow::ensure!(
            given.origin() == base.origin(),
            t!("sync-rest-the-server-pointed-elsewhere", service = self.service)
        );
        Ok(path.to_owned())
    }

    pub fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::blocking::RequestBuilder> {
        let mut builder = self.http.request(method, self.url(path)?).bearer_auth(&self.token);
        for (name, value) in &self.headers {
            builder = builder.header(*name, value);
        }
        Ok(builder)
    }

    /// Sends a request and checks its status, returning the response.
    pub fn send(&self, builder: reqwest::blocking::RequestBuilder) -> Result<reqwest::blocking::Response> {
        let response = builder
            .send()
            .with_context(|| t!("sync-rest-couldnt-reach", service = self.service))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = read_text(response).unwrap_or_default();
        Err(self.problem(status, &body, retry.as_deref()))
    }

    /// What a refusal means, as an error.
    pub fn problem(&self, status: reqwest::StatusCode, body: &str, retry: Option<&str>) -> anyhow::Error {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let message = v["error"]["message"]
            .as_str()
            .or(v["error_description"].as_str())
            .or(v["error"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| body.chars().take(200).collect());
        match status.as_u16() {
            401 => anyhow::Error::new(AccessFailure::Authentication)
                .context(t!("sync-rest-didnt-accept-the-sign-in", service = self.service, message = message)),
            404 => anyhow::Error::new(NotFound),
            429 | 503 => {
                let wait = retry.map(|r| format!(" (Retry-After: {r})")).unwrap_or_default();
                // The code stays outside the words: `imap::throttled` looks for it.
                anyhow::anyhow!(
                    "[THROTTLED] {}{wait}",
                    t!("sync-rest-is-limiting-requests", service = self.service, status = status.as_u16())
                )
            }
            _ => anyhow::anyhow!(t!(
                "sync-rest-answered",
                service = self.service,
                status = status,
                message = message
            )),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        read_json(self.send(self.request(reqwest::Method::GET, path)?)?)
    }
    /// The bytes at `path` (a message's source, an attachment).
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let response = self.send(self.request(reqwest::Method::GET, path)?.timeout(TRANSFER_TIMEOUT))?;
        read_bytes(response)
    }
    /// Sends `body` as JSON with `method`; the answer's JSON (null when it
    /// has none, as 204 No Content).
    pub fn json(&self, method: reqwest::Method, path: &str, body: &Value) -> Result<Value> {
        read_json(self.send(self.request(method, path)?.json(body).timeout(TRANSFER_TIMEOUT))?)
    }
    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.json(reqwest::Method::POST, path, body)
    }
    pub fn patch(&self, path: &str, body: &Value) -> Result<Value> {
        self.json(reqwest::Method::PATCH, path, body)
    }
    pub fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.json(reqwest::Method::PUT, path, body)
    }
    pub fn delete(&self, path: &str) -> Result<()> {
        self.send(self.request(reqwest::Method::DELETE, path)?)?;
        Ok(())
    }
    /// Sends `bytes` of `content_type` with `method`; the answer's JSON.
    pub fn upload(&self, method: reqwest::Method, path: &str, content_type: &str, bytes: Vec<u8>) -> Result<Value> {
        let builder = self
            .request(method, path)?
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .timeout(TRANSFER_TIMEOUT);
        read_json(self.send(builder)?)
    }
    /// The answer to a request of the caller's making (Gmail's batches):
    /// its Content-Type and bytes.
    pub fn exchange(
        &self,
        method: reqwest::Method,
        path: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<(String, Vec<u8>)> {
        let builder = self
            .request(method, path)?
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes);
        let response = self.send(builder)?;
        let kind = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        Ok((kind, read_bytes(response)?))
    }
}

fn read_bytes(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    response.take(MOST + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MOST {
        bail!(t!("sync-rest-the-answer-was-too-large"));
    }
    Ok(out)
}
fn read_text(response: reqwest::blocking::Response) -> Result<String> {
    Ok(String::from_utf8_lossy(&read_bytes(response)?).into_owned())
}
/// A response's JSON, read up to the size any answer may be.
pub fn bounded_json(response: reqwest::blocking::Response) -> Result<Value> {
    read_json(response)
}
fn read_json(response: reqwest::blocking::Response) -> Result<Value> {
    let bytes = read_bytes(response)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// `text` for a URL's path or query (RFC 3986 unreserved characters kept).
pub fn escape(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// A small HTTPS-less server for tests of the APIs: each request (method,
/// path, body) answered by `respond` with a status and JSON or bytes,
/// until `stop` is dropped. Returns its address.
#[cfg(test)]
pub mod test_server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    /// A request the server saw.
    #[derive(Clone, Debug)]
    pub struct Request {
        pub method: String,
        pub path: String,
        pub content_type: String,
        pub body: Vec<u8>,
        /// The Authorization header, as sent.
        pub authorization: String,
    }
    impl Request {
        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }
    }
    /// An answer: status, Content-Type, body.
    pub type Answer = (u16, String, Vec<u8>);
    pub fn json(status: u16, v: serde_json::Value) -> Answer {
        (status, "application/json".into(), v.to_string().into_bytes())
    }

    pub struct Server {
        pub base: String,
        pub port: u16,
        /// Every request, in order, for tests to look at.
        #[allow(dead_code)]
        pub seen: Arc<Mutex<Vec<Request>>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// An answer with headers of its own.
    pub type FullAnswer = (u16, String, Vec<u8>, Vec<(String, String)>);

    /// Serves `respond`, every request signed in with `Bearer token`.
    pub fn start(respond: impl Fn(&Request) -> Answer + Send + Sync + 'static) -> Server {
        start_raw(move |r| {
            let (status, kind, body) = if r.authorization == "Bearer token" || r.path.starts_with("/upload-session/") {
                respond(r)
            } else {
                json(401, serde_json::json!({"error": {"code": 401, "message": "no token"}}))
            };
            (status, kind, body, Vec::new())
        })
    }
    /// Serves `respond`, which checks sign-ins itself.
    pub fn start_raw(respond: impl Fn(&Request) -> FullAnswer + Send + Sync + 'static) -> Server {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let seen: Arc<Mutex<Vec<Request>>> = Arc::default();
        let stop: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let respond = Arc::new(respond);
        {
            let seen = seen.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let socket = match listener.accept() {
                        Ok((s, _)) => s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                            continue;
                        }
                        Err(_) => break,
                    };
                    let seen = seen.clone();
                    let respond = respond.clone();
                    std::thread::spawn(move || {
                        socket.set_nonblocking(false).unwrap();
                        socket.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
                        let mut reader = BufReader::new(socket);
                        loop {
                            let mut first = String::new();
                            if reader.read_line(&mut first).unwrap_or(0) == 0 {
                                return;
                            }
                            let mut parts = first.split_whitespace();
                            let method = parts.next().unwrap_or_default().to_owned();
                            let path = parts.next().unwrap_or_default().to_owned();
                            let (mut length, mut content_type, mut authorization) = (0usize, String::new(), String::new());
                            loop {
                                let mut line = String::new();
                                reader.read_line(&mut line).unwrap();
                                let line = line.trim_end();
                                if line.is_empty() {
                                    break;
                                }
                                let (k, v) = line.split_once(':').unwrap();
                                match k.to_ascii_lowercase().as_str() {
                                    "content-length" => length = v.trim().parse().unwrap(),
                                    "content-type" => content_type = v.trim().to_owned(),
                                    "authorization" => authorization = v.trim().to_owned(),
                                    _ => {}
                                }
                            }
                            let mut body = vec![0; length];
                            reader.read_exact(&mut body).unwrap();
                            let request = Request { method, path, content_type, body, authorization };
                            seen.lock().unwrap().push(request.clone());
                            let (status, kind, mut reply, headers) = respond(&request);
                            // No Content has none.
                            if status == 204 {
                                reply.clear();
                            }
                            let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                            let head = format!(
                                "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\n\r\n",
                                reply.len()
                            );
                            let stream = reader.get_mut();
                            if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&reply)).is_err() {
                                return;
                            }
                        }
                    });
                }
            });
        }
        Server { base: format!("http://127.0.0.1:{port}"), port, seen, stop }
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{json, start};
    use super::*;

    fn tls() -> native_tls::TlsConnector {
        native_tls::TlsConnector::new().unwrap()
    }

    #[test]
    fn refusals_say_what_they_mean() {
        let server = start(|r| match r.path.as_str() {
            "/gone" => json(404, serde_json::json!({"error": {"code": 404, "message": "Not Found"}})),
            "/busy" => json(429, serde_json::json!({"error": {"message": "Too many"}})),
            "/bad" => json(400, serde_json::json!({"error": {"code": "BadRequest", "message": "No such label"}})),
            _ => json(200, serde_json::json!({"ok": true})),
        });
        let rest = Rest::new(&server.base, "token", "Gmail", tls()).unwrap();
        assert_eq!(rest.get("/fine").unwrap()["ok"], true);
        assert!(not_found(&rest.get("/gone").unwrap_err()));
        assert!(rest.get("/busy").unwrap_err().to_string().starts_with("[THROTTLED]"));
        let bad = format!("{:#}", rest.get("/bad").unwrap_err());
        assert!(bad.contains("No such label"), "{bad}");
        let other = Rest::new(&server.base, "other", "Gmail", tls()).unwrap();
        let refused = other.get("/fine").unwrap_err();
        assert!(refused.downcast_ref::<AccessFailure>().is_some(), "{refused:#}");
        // A next page elsewhere isn't followed with the token.
        assert!(rest.url("https://example.com/next").is_err());
        assert!(rest.url(&format!("{}/next?x=1", server.base)).is_ok());
        assert_eq!(escape("a b/c"), "a%20b%2Fc");
    }
}
