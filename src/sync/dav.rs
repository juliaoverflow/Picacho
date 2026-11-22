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
    /// A calendar's kinds of component (`VEVENT`, `VTODO`…), from
    /// `supported-calendar-component-set`; empty where it doesn't say, which
    /// means any (RFC 4791 §5.2.3).
    pub components: Vec<String>,
    /// Apple's `calendar-color` (`#RRGGBB` or `#RRGGBBAA`), if set.
    pub color: String,
    /// RFC 6578's `sync-token`, when the collection offers sync-collection.
    pub sync_token: String,
    /// The older `getctag` (CalendarServer), which changes with any change.
    pub ctag: String,
}

/// What changed in a collection since a sync token (RFC 6578).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Changed or new resources, with their ETags.
    pub changed: Vec<(url::Url, String)>,
    /// Removed resources.
    pub removed: Vec<url::Url>,
    /// The token to ask with next time.
    pub token: String,
}

/// The server is busy or limiting requests (429, 503): try again after
/// `seconds`, from its Retry-After when it says.
#[derive(Debug)]
pub struct Busy {
    pub seconds: u64,
}
impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The server is busy; try again in {}.",
            if self.seconds < 120 {
                format!("{} seconds", self.seconds)
            } else {
                format!("{} minutes", self.seconds.div_ceil(60))
            }
        )
    }
}
impl std::error::Error for Busy {}

/// A sync token the server no longer knows (RFC 6578 §3.2
/// `valid-sync-token`): read the collection whole again.
#[derive(Debug)]
pub struct TokenExpired;
impl std::fmt::Display for TokenExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The server no longer knows what changed since the last sync.")
    }
}
impl std::error::Error for TokenExpired {}

/// A collection's state when it was last read, to ask what changed since.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ctag: String,
}

/// What reading a collection found: resources downloaded anew, and the
/// hrefs of those kept as they were.
pub struct Read {
    pub fetched: Vec<Resource>,
    pub kept: std::collections::HashSet<String>,
    pub state: State,
    /// The collection was read whole (`whole`), rather than by its changes.
    pub whole: bool,
}

/// Whether `href` is a resource in `collection`.
pub fn in_collection(href: &str, collection: &url::Url) -> bool {
    let Ok(href) = url::Url::parse(href) else {
        return false;
    };
    let base = collection.path().trim_end_matches('/');
    href.origin() == collection.origin()
        && href
            .path()
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
}

/// Reads a collection, downloading as little as the server allows:
///
/// - With `previous` and a ctag (or sync token) that hasn't changed: nothing.
/// - With a sync token: sync-collection's changes, then the changed
///   resources by multiget. An expired token reads it whole.
/// - Otherwise the whole collection (`whole`).
///
/// `known` is what was read before, as (href, ETag).
pub fn read_collection(
    client: &Client,
    collection: &Collection,
    previous: Option<&State>,
    known: &[(String, String)],
    whole: impl Fn(&Client) -> Result<Vec<Resource>>,
    multiget: impl Fn(&Client, &[url::Url]) -> Result<Vec<Resource>>,
) -> Result<Read> {
    let state = State {
        sync_token: collection.sync_token.clone(),
        ctag: collection.ctag.clone(),
    };
    let all_kept = || known.iter().map(|(h, _)| h.clone()).collect();
    if let Some(previous) = previous {
        let same = (!state.sync_token.is_empty() && state.sync_token == previous.sync_token)
            || (!state.ctag.is_empty() && state.ctag == previous.ctag);
        if same {
            return Ok(Read {
                fetched: Vec::new(),
                kept: all_kept(),
                state,
                whole: false,
            });
        }
        if !previous.sync_token.is_empty() && !state.sync_token.is_empty() {
            match client.changes(&collection.href, &previous.sync_token) {
                Ok(changes) => {
                    let mut kept: std::collections::HashSet<String> = all_kept();
                    for gone in &changes.removed {
                        kept.remove(gone.as_str());
                    }
                    // Changed ones whose ETag is what's kept were changed here.
                    let wanted: Vec<url::Url> = changes
                        .changed
                        .iter()
                        .filter(|(href, etag)| {
                            !known.iter().any(|(h, e)| h == href.as_str() && e == etag)
                        })
                        .map(|(href, _)| href.clone())
                        .collect();
                    for href in &wanted {
                        kept.remove(href.as_str());
                    }
                    let fetched = if wanted.is_empty() {
                        Vec::new()
                    } else {
                        multiget(client, &wanted)?
                    };
                    return Ok(Read {
                        fetched,
                        kept,
                        state: State {
                            sync_token: changes.token,
                            ctag: state.ctag,
                        },
                        whole: false,
                    });
                }
                Err(e) if e.is::<TokenExpired>() => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(Read {
        fetched: whole(client)?,
        kept: Default::default(),
        state,
        whole: true,
    })
}

/// Seconds from a Retry-After header (seconds, or an HTTP date), at most a
/// day; a minute where it is missing or unreadable.
pub fn retry_after(value: Option<&str>, now: i64) -> u64 {
    const DEFAULT: u64 = 60;
    let Some(value) = value.map(str::trim) else {
        return DEFAULT;
    };
    let seconds = value.parse::<u64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|t| (t.timestamp() - now).max(1) as u64)
    });
    seconds.unwrap_or(DEFAULT).clamp(1, 86_400)
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
                // Hrefs and redirects from the server can name any address:
                // the password is never sent over plain http.
                .https_only(!cfg!(test))
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
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
        {
            let seconds = retry_after(
                response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
                chrono::Utc::now().timestamp(),
            );
            return Err(Busy { seconds }.into());
        }
        if status == reqwest::StatusCode::INSUFFICIENT_STORAGE {
            bail!("The server is out of storage for your account. Free some space there and retry.");
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("The server didn't accept the user name and password.");
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            bail!("This was changed on the server meanwhile. Read it again and retry.");
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            let body = response.text().unwrap_or_default();
            bail!(Forbidden(body.chars().take(4096).collect()));
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
        let body = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:a=\"http://apple.com/ns/ical/\" xmlns:cs=\"http://calendarserver.org/ns/\"><d:prop><d:resourcetype/><d:displayname/><d:current-user-privilege-set/><c:supported-calendar-component-set/><a:calendar-color/><d:sync-token/><cs:getctag/></d:prop></d:propfind>";
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
            // Only properties the server found (a 200 propstat) count.
            let found = |name: &str| {
                response
                    .children()
                    .filter(|p| p.tag_name().name() == "propstat")
                    .filter(|p| {
                        text_of(*p, "status").is_none_or(|s| s.contains(" 200 "))
                    })
                    .find_map(|p| text_of(p, name))
                    .unwrap_or_default()
            };
            let components = response
                .descendants()
                .filter(|n| n.tag_name().name() == "supported-calendar-component-set")
                .flat_map(|set| set.children().filter(|c| c.tag_name().name() == "comp"))
                .filter_map(|c| c.attribute("name"))
                .map(str::to_ascii_uppercase)
                .collect();
            out.push(Collection {
                href: url,
                name,
                writable,
                components,
                color: found("calendar-color"),
                sync_token: found("sync-token"),
                ctag: found("getctag"),
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

    /// What changed in a collection since `token` (RFC 6578 sync-collection),
    /// as hrefs and ETags only. An empty token lists everything.
    pub fn changes(&self, collection: &url::Url, token: &str) -> Result<Changes> {
        let body = format!(
            "<?xml version=\"1.0\"?><d:sync-collection xmlns:d=\"DAV:\"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>",
            xml_escape(token)
        );
        let response = self.send(
            "REPORT",
            collection,
            &[("Content-Type", "application/xml; charset=utf-8")],
            body,
        );
        let text = match response {
            Ok(r) => r.text()?,
            Err(e) => {
                if let Some(Forbidden(body)) = e.downcast_ref::<Forbidden>()
                    && body.contains("valid-sync-token")
                {
                    return Err(TokenExpired.into());
                }
                return Err(e);
            }
        };
        parse_changes(collection, &text)
    }

    /// Several resources at once by href (`calendar-multiget` or
    /// `addressbook-multiget`), in batches.
    pub fn multiget(
        &self,
        collection: &url::Url,
        report: &str,
        namespace: &str,
        data: &str,
        hrefs: &[url::Url],
    ) -> Result<Vec<Resource>> {
        let mut out = Vec::new();
        for chunk in hrefs.chunks(100) {
            let refs: String = chunk
                .iter()
                .map(|h| format!("<d:href>{}</d:href>", xml_escape(h.path())))
                .collect();
            let body = format!(
                "<?xml version=\"1.0\"?><c:{report} xmlns:d=\"DAV:\" xmlns:c=\"{namespace}\"><d:prop><d:getetag/><c:{data}/></d:prop>{refs}</c:{report}>"
            );
            out.extend(self.list(collection, &body, data)?);
        }
        Ok(out)
    }

    /// The user's home for collections of `kind` (`calendar-home-set` or
    /// `addressbook-home-set`), from the server's address as `discover`
    /// finds it.
    pub fn home(&self, given: &str, service: &str, home_set: &str, namespace: &str) -> Result<url::Url> {
        let given = given.trim();
        let given = if given.contains("://") {
            given.to_owned()
        } else {
            format!("https://{given}")
        };
        let mut start = url::Url::parse(&given).context("The server's address isn't valid.")?;
        if start.path() == "/" || start.path().is_empty() {
            start.set_path(&format!("/.well-known/{service}"));
        }
        let principal = self
            .find_href(&start, "current-user-principal", DAV)?
            .unwrap_or_else(|| start.clone());
        self.find_href(&principal, home_set, namespace)?
            .context("This server doesn't say where your collections go.")
    }
    /// The account's scheduling outbox (`schedule-outbox-URL`, RFC 6638).
    pub fn outbox(&self, given: &str) -> Result<url::Url> {
        let given = given.trim();
        let given = if given.contains("://") {
            given.to_owned()
        } else {
            format!("https://{given}")
        };
        let mut start = url::Url::parse(&given).context("The server's address isn't valid.")?;
        if start.path() == "/" || start.path().is_empty() {
            start.set_path("/.well-known/caldav");
        }
        let principal = self
            .find_href(&start, "current-user-principal", DAV)?
            .unwrap_or_else(|| start.clone());
        self.find_href(&principal, "schedule-outbox-URL", CALDAV)?
            .context("This server can't say when people are free (no scheduling outbox).")
    }
    /// POSTs a scheduling message to an outbox (RFC 6638 §5), returning
    /// the server's answer.
    pub fn post(
        &self,
        outbox: &url::Url,
        content_type: &str,
        body: String,
        originator: &str,
        recipients: &[String],
    ) -> Result<String> {
        let recipients = recipients
            .iter()
            .map(|r| format!("mailto:{r}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(self
            .send(
                "POST",
                outbox,
                &[
                    ("Content-Type", content_type),
                    ("Originator", originator),
                    ("Recipient", &recipients),
                ],
                body,
            )?
            .text()?)
    }
    /// Makes a collection at `href`: a calendar (MKCALENDAR, RFC 4791
    /// §5.3.1) for events, or an address book (extended MKCOL, RFC 5689),
    /// named `name`, with a calendar's colour where given.
    pub fn make_collection(&self, href: &url::Url, calendar: bool, name: &str, color: &str) -> Result<()> {
        let name = xml_escape(name);
        let (method, body) = if calendar {
            let color = if color.is_empty() {
                String::new()
            } else {
                format!("<a:calendar-color>{}</a:calendar-color>", xml_escape(color))
            };
            (
                "MKCALENDAR",
                format!(
                    "<?xml version=\"1.0\"?><c:mkcalendar xmlns:d=\"DAV:\" xmlns:c=\"{CALDAV}\" xmlns:a=\"http://apple.com/ns/ical/\"><d:set><d:prop><d:displayname>{name}</d:displayname>{color}<c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"
                ),
            )
        } else {
            (
                "MKCOL",
                format!(
                    "<?xml version=\"1.0\"?><d:mkcol xmlns:d=\"DAV:\" xmlns:c=\"{CARDDAV}\"><d:set><d:prop><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype><d:displayname>{name}</d:displayname></d:prop></d:set></d:mkcol>"
                ),
            )
        };
        self.send(
            method,
            href,
            &[("Content-Type", "application/xml; charset=utf-8")],
            body,
        )?;
        Ok(())
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

/// Text escaped for an XML element.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A sync-collection answer: changed resources (with an ETag), removed ones
/// (a 404 status), and the new token.
fn parse_changes(collection: &url::Url, text: &str) -> Result<Changes> {
    let doc = parse(text)?;
    let mut out = Changes::default();
    for (href, response) in responses(&doc) {
        let Ok(url) = collection.join(&href) else {
            continue;
        };
        // The collection itself may be listed; it isn't one of its items.
        if url.path().trim_end_matches('/') == collection.path().trim_end_matches('/') {
            continue;
        }
        let status = response
            .children()
            .find(|c| c.tag_name().name() == "status")
            .and_then(|s| s.text())
            .unwrap_or_default();
        if status.contains(" 404 ") {
            out.removed.push(url);
        } else if let Some(etag) = text_of(response, "getetag").filter(|e| !e.is_empty()) {
            out.changed.push((url, etag));
        }
    }
    out.token = doc
        .root_element()
        .children()
        .find(|n| n.tag_name().name() == "sync-token")
        .and_then(|n| n.text())
        .unwrap_or_default()
        .trim()
        .to_owned();
    ensure!(!out.token.is_empty(), "The server's changes came without a sync token.");
    Ok(out)
}

/// The server refused a request (403), with its body: a precondition
/// element such as `valid-sync-token` says why.
#[derive(Debug)]
pub struct Forbidden(pub String);
impl std::fmt::Display for Forbidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The server doesn't allow this change.")
    }
}
impl std::error::Error for Forbidden {}

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
        // RFC 6764: the domain's `_caldavs._tcp` / `_carddavs._tcp` SRV
        // record names its server, and a TXT `path=` its context path.
        if !cfg!(test)
            && let Some(found) = start.host_str().and_then(|d| srv_start(service, d))
        {
            start = found;
        } else {
            start.set_path(&format!("/.well-known/{service}"));
        }
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

/// Where a domain's SRV and TXT records (RFC 6764 §3–4) say its CalDAV or
/// CardDAV service starts. As with JMAP's records, DNS isn't authenticated,
/// so only a target in the domain itself is used: the password then goes
/// only to a host with the domain's certificate.
fn srv_start(service: &str, domain: &str) -> Option<url::Url> {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    let name = format!("_{service}s._tcp.{domain}");
    let (host, port) = crate::sync::auth::srv(&name)
        .ok()?
        .into_iter()
        .find(|(host, _)| host == &domain || host.ends_with(&format!(".{domain}")))?;
    let path = crate::sync::auth::txt(&name)
        .ok()
        .and_then(|records| srv_path(&records))
        .unwrap_or_else(|| format!("/.well-known/{service}"));
    let port = if port == 443 {
        String::new()
    } else {
        format!(":{port}")
    };
    url::Url::parse(&format!("https://{host}{port}{path}")).ok()
}

/// The `path=` of a service's TXT records, if one starts with `/`.
fn srv_path(records: &[String]) -> Option<String> {
    records
        .iter()
        .flat_map(|r| r.split_whitespace())
        .find_map(|kv| kv.strip_prefix("path="))
        .filter(|p| p.starts_with('/'))
        .map(str::to_owned)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_seconds_and_dates() {
        assert_eq!(retry_after(Some("120"), 0), 120);
        assert_eq!(retry_after(None, 0), 60);
        assert_eq!(retry_after(Some("soon"), 0), 60);
        assert_eq!(retry_after(Some("999999"), 0), 86_400);
        let now = chrono::DateTime::parse_from_rfc2822("Sun, 22 Nov 2026 10:00:00 GMT")
            .unwrap()
            .timestamp();
        assert_eq!(retry_after(Some("Sun, 22 Nov 2026 10:05:00 GMT"), now), 300);
    }

    #[test]
    fn service_records_name_a_path() {
        assert_eq!(srv_path(&["path=/dav/".into()]).as_deref(), Some("/dav/"));
        assert_eq!(srv_path(&["v=1 path=/caldav".into()]).as_deref(), Some("/caldav"));
        assert_eq!(srv_path(&["path=elsewhere".into()]), None);
        assert_eq!(srv_path(&[]), None);
    }

    #[test]
    fn hrefs_belong_to_their_collection() {
        let book = url::Url::parse("https://dav.example/books/ana/default/").unwrap();
        assert!(in_collection("https://dav.example/books/ana/default/1.vcf", &book));
        assert!(!in_collection("https://dav.example/books/ana/default/", &book));
        assert!(!in_collection("https://dav.example/books/ana/defaults/1.vcf", &book));
        assert!(!in_collection("https://other.example/books/ana/default/1.vcf", &book));
    }

    #[test]
    fn only_changes_are_downloaded() -> Result<()> {
        let (port, server) = test_server(3, |method, path, body| match (method, path) {
            ("REPORT", "/cal/home/") if body.contains("sync-collection") => {
                if body.contains("<d:sync-token>old-gone</d:sync-token>") {
                    return ("403 Forbidden", "<d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>".into());
                }
                assert!(body.contains("<d:sync-token>t1</d:sync-token>"));
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/cal/home/a.ics</d:href><d:propstat><d:prop><d:getetag>\"a1\"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/cal/home/b.ics</d:href><d:propstat><d:prop><d:getetag>\"b2\"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/cal/home/c.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response><d:sync-token>t2</d:sync-token></d:multistatus>".into())
            }
            ("REPORT", "/cal/home/") => {
                // Only b changed; a's ETag is the one kept.
                assert!(body.contains("calendar-multiget") && body.contains("/cal/home/b.ics"));
                assert!(!body.contains("/cal/home/a.ics"));
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/cal/home/b.ics</d:href><d:propstat><d:prop><d:getetag>\"b2\"</d:getetag><c:calendar-data>B</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>".into())
            }
            _ => ("404 Not Found", String::new()),
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let href = url::Url::parse(&format!("http://127.0.0.1:{port}/cal/home/"))?;
        let at = |p: &str| href.join(p).unwrap().to_string();
        let collection = Collection {
            href: href.clone(),
            name: "Home".into(),
            writable: true,
            components: vec![],
            color: String::new(),
            sync_token: "t2".into(),
            ctag: String::new(),
        };
        let known = vec![
            (at("a.ics"), "\"a1\"".to_owned()),
            (at("b.ics"), "\"b1\"".to_owned()),
            (at("c.ics"), "\"c1\"".to_owned()),
        ];
        let multiget = |c: &Client, hrefs: &[url::Url]| {
            c.multiget(&href, "calendar-multiget", CALDAV, "calendar-data", hrefs)
        };
        // Unchanged: no request at all.
        let same = State {
            sync_token: "t2".into(),
            ctag: String::new(),
        };
        let read = read_collection(&client, &collection, Some(&same), &known, |_| unreachable!(), multiget)?;
        assert!(!read.whole && read.fetched.is_empty() && read.kept.len() == 3);
        // Changed: the changes, then only b.
        let before = State {
            sync_token: "t1".into(),
            ctag: String::new(),
        };
        let read = read_collection(&client, &collection, Some(&before), &known, |_| unreachable!(), multiget)?;
        assert_eq!(read.state.sync_token, "t2");
        assert_eq!(read.fetched.len(), 1);
        assert_eq!(read.fetched[0].data, "B");
        assert!(read.kept.contains(&at("a.ics")));
        assert!(!read.kept.contains(&at("b.ics")) && !read.kept.contains(&at("c.ics")));
        // A token the server forgot: read whole.
        let gone = State {
            sync_token: "old-gone".into(),
            ctag: String::new(),
        };
        let read = read_collection(&client, &collection, Some(&gone), &known, |_| Ok(vec![]), multiget)?;
        assert!(read.whole);
        assert_eq!(server.join().unwrap().len(), 3);
        Ok(())
    }
}
