//! WebDAV as CardDAV (RFC 6352) and CalDAV (RFC 4791) use it: finding a
//! user's collections from a server address (RFC 6764), listing what is in
//! them with their ETags, and writing and removing single resources with
//! If-Match, so a change made elsewhere meanwhile isn't overwritten.
use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;

pub const DAV: &str = "DAV:";
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";

/// One resource on the server: where it is, its ETag, and its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub href: url::Url,
    pub etag: String,
    pub data: String,
}

/// A collection (address book or calendar) and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub href: url::Url,
    pub name: String,
    /// Whether this account may change what's in it.
    pub writable: bool,
}

pub struct Client {
    http: reqwest::blocking::Client,
    user: String,
    password: String,
}

/// The `href`s of the responses in a multistatus answer, with each
/// response's node.
fn responses<'a>(
    doc: &'a roxmltree::Document<'a>,
) -> impl Iterator<Item = (String, roxmltree::Node<'a, 'a>)> + 'a {
    doc.descendants()
        .filter(|n| n.tag_name().name() == "response")
        .filter_map(|response| {
            let href = response
                .children()
                .find(|c| c.tag_name().name() == "href")?
                .text()?
                .trim()
                .to_owned();
            Some((href, response))
        })
}

/// The href inside the first `name` element under `node`.
fn href_in(node: roxmltree::Node, name: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.tag_name().name() == name)?
        .descendants()
        .find(|n| n.tag_name().name() == "href")?
        .text()
        .map(|t| t.trim().to_owned())
}

fn text_of(node: roxmltree::Node, name: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.tag_name().name() == name)
        .map(|n| n.text().unwrap_or_default().trim().to_owned())
}

fn parse(text: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse(text).context("The server's answer couldn't be read.")
}

impl Client {
    pub fn new(user: &str, password: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .use_preconfigured_tls(tls)
                .timeout(Duration::from_secs(60))
                .connect_timeout(Duration::from_secs(12))
                // Same-host redirects only, so the password stays with this server.
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    let same = attempt
                        .previous()
                        .first()
                        .is_some_and(|first| first.host_str() == attempt.url().host_str());
                    if same && attempt.previous().len() < 5 {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
                .build()?,
            user: user.to_owned(),
            password: password.to_owned(),
        })
    }

    fn send(
        &self,
        method: &str,
        url: &url::Url,
        headers: &[(&str, &str)],
        body: String,
    ) -> Result<reqwest::blocking::Response> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut request = self
            .http
            .request(method, url.clone())
            .basic_auth(&self.user, Some(&self.password));
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let response = request.body(body).send().with_context(|| {
            format!("Couldn't reach {}", url.host_str().unwrap_or("the server"))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("The server didn't accept the user name and password.");
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            bail!("This was changed on the server meanwhile. Read it again and retry.");
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            bail!("The server doesn't allow this change.");
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(NotFound);
        }
        ensure!(
            status.is_success() || status.as_u16() == 207,
            "The server answered {status}."
        );
        Ok(response)
    }

    fn xml(&self, method: &str, url: &url::Url, depth: &str, body: &str) -> Result<String> {
        Ok(self
            .send(
                method,
                url,
                &[
                    ("Depth", depth),
                    ("Content-Type", "application/xml; charset=utf-8"),
                ],
                body.to_owned(),
            )?
            .text()?)
    }

    /// A property's href at `url` (Depth 0).
    fn find_href(
        &self,
        url: &url::Url,
        property: &str,
        namespace: &str,
    ) -> Result<Option<url::Url>> {
        let body = format!(
            "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{namespace}\"><d:prop><c:{property}/></d:prop></d:propfind>"
        );
        let text = self.xml("PROPFIND", url, "0", &body)?;
        let doc = parse(&text)?;
        Ok(href_in(doc.root(), property).and_then(|h| url.join(&h).ok()))
    }

    /// The collections of kind `kind` (`addressbook` or `calendar`) at
    /// `home` (Depth 1), or `home` itself if it is one.
    fn collections(&self, home: &url::Url, kind: &str) -> Result<Vec<Collection>> {
        let body = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:resourcetype/><d:displayname/><d:current-user-privilege-set/></d:prop></d:propfind>";
        let text = self.xml("PROPFIND", home, "1", body)?;
        let doc = parse(&text)?;
        let mut out = Vec::new();
        for (href, response) in responses(&doc) {
            let is_kind = response.descendants().any(|n| {
                n.tag_name().name() == kind
                    && n.parent()
                        .is_some_and(|p| p.tag_name().name() == "resourcetype")
            });
            if !is_kind {
                continue;
            }
            let Ok(url) = home.join(&href) else { continue };
            // Without a privilege set, assume it can be written.
            let privileges: Vec<&str> = response
                .descendants()
                .filter(|n| n.tag_name().name() == "privilege")
                .flat_map(|p| {
                    p.children()
                        .filter(|c| c.is_element())
                        .map(|c| c.tag_name().name())
                })
                .collect();
            let writable = privileges.is_empty()
                || privileges
                    .iter()
                    .any(|p| matches!(*p, "write" | "write-content" | "all" | "bind"));
            let name = text_of(response, "displayname")
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    url.path_segments()
                        .and_then(|mut s| s.rfind(|p| !p.is_empty()))
                        .unwrap_or("Default")
                        .to_owned()
                });
            out.push(Collection {
                href: url,
                name,
                writable,
            });
        }
        Ok(out)
    }

    /// Every resource in a collection, by a REPORT query (`addressbook-query`
    /// or `calendar-query`) with its data element (`address-data` or
    /// `calendar-data`).
    pub fn list(&self, collection: &url::Url, query: &str, data: &str) -> Result<Vec<Resource>> {
        let text = self.xml("REPORT", collection, "1", query)?;
        let doc = parse(&text)?;
        let mut out = Vec::new();
        for (href, response) in responses(&doc) {
            let Some(content) = response
                .descendants()
                .find(|n| n.tag_name().name() == data)
                .and_then(|n| n.text())
            else {
                continue;
            };
            let Ok(href) = collection.join(&href) else {
                continue;
            };
            out.push(Resource {
                href,
                etag: text_of(response, "getetag").unwrap_or_default(),
                data: content.to_owned(),
            });
        }
        Ok(out)
    }

    /// Writes a resource: a new one (`etag` empty, which must not exist
    /// yet), or a change to the version with `etag`. Returns its new ETag,
    /// when the server says.
    pub fn put(
        &self,
        href: &url::Url,
        content_type: &str,
        etag: &str,
        data: String,
    ) -> Result<String> {
        let mut headers = vec![("Content-Type", content_type)];
        if etag.is_empty() {
            headers.push(("If-None-Match", "*"));
        } else {
            headers.push(("If-Match", etag));
        }
        let response = self.send("PUT", href, &headers, data)?;
        Ok(response
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned())
    }

    /// Removes a resource, if it is still the version with `etag`.
    pub fn delete(&self, href: &url::Url, etag: &str) -> Result<()> {
        let headers: Vec<(&str, &str)> = if etag.is_empty() {
            Vec::new()
        } else {
            vec![("If-Match", etag)]
        };
        match self.send("DELETE", href, &headers, String::new()) {
            Ok(_) => Ok(()),
            // Already gone.
            Err(e) if e.is::<NotFound>() => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The server has no such resource.
#[derive(Debug)]
pub struct NotFound;
impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The server has no such item (404).")
    }
}
impl std::error::Error for NotFound {}

/// Finds the user's collections of `kind` (`addressbook` in CardDAV's
/// `addressbook-home-set`, or `calendar` in CalDAV's `calendar-home-set`)
/// from the server's address: that address itself, its
/// `/.well-known/<service>`, the principal and its home.
pub fn discover(
    client: &Client,
    given: &str,
    service: &str,
    kind: &str,
    home_set: &str,
    namespace: &str,
) -> Result<Vec<Collection>> {
    let given = given.trim();
    ensure!(!given.is_empty(), "Enter the server's address.");
    let given = if given.contains("://") {
        given.to_owned()
    } else {
        format!("https://{given}")
    };
    let mut start = url::Url::parse(&given).context("The server's address isn't valid.")?;
    ensure!(
        start.scheme() == "https" || cfg!(test),
        "The server's address must start with https://."
    );
    if start.path() == "/" || start.path().is_empty() {
        start.set_path(&format!("/.well-known/{service}"));
    }
    let mut found = client.collections(&start, kind).unwrap_or_default();
    if found.is_empty() {
        let principal = client
            .find_href(&start, "current-user-principal", DAV)?
            .unwrap_or_else(|| start.clone());
        let home = client
            .find_href(&principal, home_set, namespace)?
            .context("This server doesn't list any for your account.")?;
        found = client.collections(&home, kind)?;
    }
    ensure!(
        !found.is_empty(),
        "Nothing was found on the server for your account."
    );
    Ok(found)
}

/// A new resource's address in a collection.
pub fn new_href(collection: &url::Url, uid: &str, extension: &str) -> Result<url::Url> {
    let safe: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut base = collection.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    Ok(base.join(&format!("{safe}.{extension}"))?)
}

/// Requests a test DAV server saw: (method, path, body).
#[cfg(test)]
pub type Seen = Vec<(String, String, String)>;

/// A small DAV server for tests: answers each request with `respond`
/// (method, path, body → status line and XML), until it has served `count`.
#[cfg(test)]
pub fn test_server(
    count: usize,
    respond: impl Fn(&str, &str, &str) -> (&'static str, String) + Send + 'static,
) -> (u16, std::thread::JoinHandle<Seen>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for stream in listener.incoming().take(count) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            let mut auth = false;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                // "ana:secret", base64.
                auth |= line.contains("Basic YW5hOnNlY3JldA==");
                if line == "\r\n" {
                    break;
                }
            }
            assert!(auth, "every request signs in");
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            let mut parts = first.split(' ');
            let method = parts.next().unwrap().to_owned();
            let path = parts.next().unwrap().to_owned();
            let (status, text) = respond(&method, &path, &body);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nETag: \"e-{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                seen.len(),
                text.len()
            )
            .unwrap();
            seen.push((method, path, body));
        }
        seen
    });
    (port, handle)
}
