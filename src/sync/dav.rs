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
    /// CalDAV scheduling's Schedule-Tag (RFC 6638 §3.2.10), where asked
    /// for and given.
    pub schedule_tag: String,
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
    /// The largest resource it takes, in octets (`max-resource-size`, RFC
    /// 4791 §5.2.5, RFC 6352 §6.2.3); 0 where it doesn't say.
    pub max_size: u64,
    /// An address book's vCard versions (`supported-address-data`, RFC
    /// 6352 §6.2.2): `3.0`, `4.0`; empty where it doesn't say (3.0).
    pub vcard_versions: Vec<String>,
    /// A calendar's limits (RFC 4791 §5.2.4–§5.2.9), where it says.
    pub limits: CalendarLimits,
}

/// What a calendar collection takes (RFC 4791 §5.2.4–§5.2.9); each
/// field empty or `None` where the server doesn't say.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CalendarLimits {
    /// `supported-calendar-data`: media types and versions, as
    /// `text/calendar 2.0`; empty: iCalendar 2.0 (§5.2.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<String>,
    /// `min-date-time` and `max-date-time` (§5.2.6–§5.2.7), Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_time: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_time: Option<i64>,
    /// `max-instances` (§5.2.8): most occurrences of one repeating event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_instances: Option<u64>,
    /// `max-attendees-per-instance` (§5.2.9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attendees: Option<u64>,
}
impl CalendarLimits {
    /// Whether the calendar takes iCalendar 2.0 (RFC 4791 §5.2.4: the
    /// default where it lists nothing).
    pub fn takes_icalendar(&self) -> bool {
        self.data.is_empty()
            || self.data.iter().any(|d| {
                let mut w = d.split_whitespace();
                w.next().is_some_and(|t| t.eq_ignore_ascii_case("text/calendar")) && w.next().is_none_or(|v| v == "2.0")
            })
    }
    /// `from`..`to` within the calendar's dates (§5.2.6–§5.2.7), so a
    /// query doesn't ask for times it refuses.
    pub fn clamp(&self, from: i64, to: i64) -> (i64, i64) {
        let from = self.min_time.map_or(from, |m| from.max(m));
        let to = self.max_time.map_or(to, |m| to.min(m));
        (from, to.max(from))
    }
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
        f.write_str(&if self.seconds < 120 {
            t!("sync-dav-the-server-is-busy-seconds", seconds = self.seconds)
        } else {
            t!("sync-dav-the-server-is-busy-minutes", minutes = self.seconds.div_ceil(60))
        })
    }
}
impl std::error::Error for Busy {}

/// A sync token the server no longer knows (RFC 6578 §3.2
/// `valid-sync-token`): read the collection whole again.
#[derive(Debug)]
pub struct TokenExpired;
impl std::fmt::Display for TokenExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("sync-dav-the-server-no-longer-knows-what"))
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

/// Whether `href`, which a server named, is on the same site as `base`,
/// the address it was asked at: the same scheme, and the same host or
/// registrable domain (`p42-caldav.icloud.com` for `caldav.icloud.com`).
/// The account's password goes with every request, and hrefs can name any
/// address; redirects already stay on the host.
pub fn same_site(href: &url::Url, base: &url::Url) -> bool {
    if href.scheme() != base.scheme() {
        return false;
    }
    let (Some(url::Host::Domain(a)), Some(url::Host::Domain(b))) = (href.host(), base.host()) else {
        return href.host() == base.host() && href.port_or_known_default() == base.port_or_known_default();
    };
    let (a, b) = (a.trim_end_matches('.').to_ascii_lowercase(), b.trim_end_matches('.').to_ascii_lowercase());
    a == b || psl::domain_str(&a).is_some_and(|d| Some(d) == psl::domain_str(&b))
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
        // The sync token decides where there is one: a ctag the server
        // forgot to bump mustn't hide a change the token shows.
        let same = if !state.sync_token.is_empty() && !previous.sync_token.is_empty() {
            state.sync_token == previous.sync_token
        } else {
            !state.ctag.is_empty() && state.ctag == previous.ctag
        };
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

/// The most changes a sync-collection answer is asked to hold (RFC 6578
/// §3.7), so a collection with many doesn't come in one huge answer.
const SYNC_LIMIT: u32 = 1000;

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
    /// How `password` signs requests.
    scheme: Scheme,
    /// The server's Digest challenge (RFC 7616), once it has asked for
    /// Digest rather than Basic, and how many requests it has signed.
    digest: std::sync::Mutex<Option<(Digest, u32)>>,
}

/// What a `Client`'s secret is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    /// A password: Basic (RFC 7617), or Digest when asked (RFC 7616).
    Password,
    /// An OAuth access token (RFC 6750 Bearer).
    Bearer,
}

/// A WWW-Authenticate Digest challenge (RFC 7616 §3.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Digest {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    /// SHA-256 or MD5 (RFC 7616 §3.3 `algorithm`), and its `-sess` form.
    sha256: bool,
    session: bool,
    /// `qop=auth` offered; else the RFC 2069 form without it.
    qop: bool,
    /// The user name hashed (RFC 7616 §3.4.4 `userhash`).
    userhash: bool,
}
impl Digest {
    /// The strongest Digest challenge among a 401's WWW-Authenticate
    /// headers: SHA-256 before MD5 (RFC 7616 §3.7). `None` without one.
    fn choose<'a>(headers: impl Iterator<Item = &'a str>) -> Option<Self> {
        let mut found: Vec<Self> = headers.filter_map(Self::parse).collect();
        found.sort_by_key(|d| !d.sha256);
        found.into_iter().next()
    }
    fn parse(header: &str) -> Option<Self> {
        let rest = header.trim();
        let (scheme, params) = rest.split_once(char::is_whitespace)?;
        if !scheme.eq_ignore_ascii_case("Digest") {
            return None;
        }
        let mut d = Digest::default();
        let mut algorithm = "MD5".to_owned();
        for (k, v) in auth_params(params) {
            match k.to_ascii_lowercase().as_str() {
                "realm" => d.realm = v,
                "nonce" => d.nonce = v,
                "opaque" => d.opaque = Some(v),
                "algorithm" => algorithm = v.to_ascii_uppercase(),
                "qop" => d.qop = v.split(',').any(|q| q.trim().eq_ignore_ascii_case("auth")),
                "userhash" => d.userhash = v.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
        d.session = algorithm.ends_with("-SESS");
        d.sha256 = match algorithm.trim_end_matches("-SESS") {
            "SHA-256" => true,
            "MD5" => false,
            // SHA-512-256 and others: not ones Picacho has.
            _ => return None,
        };
        (!d.nonce.is_empty()).then_some(d)
    }
    fn hash(&self, text: &str) -> String {
        use sha2::Digest as _;
        if self.sha256 {
            hex(&sha2::Sha256::digest(text.as_bytes()))
        } else {
            hex(&md5::Md5::digest(text.as_bytes()))
        }
    }
    /// The Authorization header for `method` on `uri` (RFC 7616 §3.4), the
    /// `count`th request on this nonce, with client nonce `cnonce`.
    fn authorization(&self, user: &str, password: &str, method: &str, uri: &str, count: u32, cnonce: &str) -> String {
        let nc = format!("{count:08x}");
        let mut a1 = self.hash(&format!("{user}:{}:{password}", self.realm));
        if self.session {
            a1 = self.hash(&format!("{a1}:{}:{cnonce}", self.nonce));
        }
        let a2 = self.hash(&format!("{method}:{uri}"));
        let response = if self.qop {
            self.hash(&format!("{a1}:{}:{nc}:{cnonce}:auth:{a2}", self.nonce))
        } else {
            self.hash(&format!("{a1}:{}:{a2}", self.nonce))
        };
        let quoted = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
        let username = if self.userhash { self.hash(&format!("{user}:{}", self.realm)) } else { quoted(user) };
        let algorithm = format!("{}{}", if self.sha256 { "SHA-256" } else { "MD5" }, if self.session { "-sess" } else { "" });
        let mut out = format!(
            "Digest username=\"{username}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={algorithm}, response=\"{response}\"",
            quoted(&self.realm),
            quoted(&self.nonce),
            quoted(uri)
        );
        if self.qop {
            out.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
        }
        if let Some(o) = &self.opaque {
            out.push_str(&format!(", opaque=\"{}\"", quoted(o)));
        }
        if self.userhash {
            out.push_str(", userhash=true");
        }
        out
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
/// The `name=value` pairs of an authentication challenge (RFC 9110
/// §11.2), values unquoted.
fn auth_params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let name: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ',' && !c.is_whitespace())).collect();
        if name.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next_if_eq(&'=').is_none() {
            continue;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
        } else {
            value = std::iter::from_fn(|| chars.next_if(|c| *c != ',')).collect::<String>().trim().to_owned();
        }
        out.push((name, value));
    }
    out
}

/// The namespace an element Picacho reads belongs in (RFC 4918 §14,
/// RFC 4791 §9, RFC 6352 §10, RFC 6578 §6, RFC 6638 §9, RFC 4331 §3,
/// RFC 5397 §3): a `calendar-data` or `href` in some other namespace is
/// another vocabulary's and isn't read as this one's. `None` for names
/// from vendor namespaces (Apple's colour, CalendarServer's ctag), which
/// servers write in more than one.
fn namespace_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "multistatus" | "response" | "href" | "propstat" | "prop" | "status" | "resourcetype"
        | "collection" | "displayname" | "getetag" | "sync-token" | "current-user-principal"
        | "current-user-privilege-set" | "privilege" | "quota-used-bytes"
        | "quota-available-bytes" | "error" | "valid-sync-token" | "mkcol-response"
        | "owner" | "lockdiscovery" | "activelock" | "locktoken" | "supportedlock" => DAV,
        "calendar" | "calendar-data" | "calendar-home-set" | "schedule-inbox-URL"
        | "schedule-outbox-URL" | "schedule-tag" | "supported-calendar-component-set"
        | "supported-calendar-data" | "min-date-time" | "max-date-time" | "max-instances"
        | "max-attendees-per-instance" | "max-attachment-size" | "max-attachments-per-resource" | "calendar-description" | "calendar-timezone"
        | "mkcalendar-response" | "schedule-response" | "recipient" | "request-status"
        | "calendar-user-address-set" | "schedule-default-calendar-URL" => CALDAV,
        "addressbook" | "address-data" | "addressbook-home-set" | "supported-address-data"
        | "address-data-type" | "addressbook-description" => CARDDAV,
        _ => return None,
    })
}
/// Whether `node` is the element `name` in the namespace it belongs in.
/// `comp` and `max-resource-size` are CalDAV's and CardDAV's alike.
fn is(node: &roxmltree::Node, name: &str) -> bool {
    let tag = node.tag_name();
    if tag.name() != name {
        return false;
    }
    match (name, tag.namespace()) {
        ("max-resource-size", Some(ns)) => ns == CALDAV || ns == CARDDAV,
        ("comp", Some(ns)) => ns == CALDAV,
        (_, ns) => namespace_of(name).is_none_or(|want| ns == Some(want)),
    }
}

/// The `href`s of the responses in a multistatus answer, with each
/// response's node.
fn responses<'a>(
    doc: &'a roxmltree::Document<'a>,
) -> impl Iterator<Item = (String, roxmltree::Node<'a, 'a>)> + 'a {
    doc.descendants()
        .filter(|n| is(n, "response"))
        .filter_map(|response| {
            let href = response
                .children()
                .find(|c| is(c, "href"))?
                .text()?
                .trim()
                .to_owned();
            Some((href, response))
        })
}

/// Whether a property element is in a `propstat` whose status isn't a
/// success (RFC 4918 §14.22: a 404 propstat lists the properties the
/// resource doesn't have, often as empty elements).
fn failed_propstat(node: roxmltree::Node) -> bool {
    node.ancestors()
        .find(|a| is(a, "propstat"))
        .and_then(|p| p.children().find(|c| is(c, "status")))
        .and_then(|s| s.text())
        .is_some_and(|s| !s.split_whitespace().nth(1).is_some_and(|code| code.starts_with('2')))
}
/// The first `name` element under `node` that a propstat says is there.
fn property<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<roxmltree::Node<'a, 'a>> {
    node.descendants()
        .find(|n| is(n, name) && !failed_propstat(*n))
}

/// Every href inside the first `name` element under `node` (a home set
/// may list several, RFC 4791 §6.2.1).
fn hrefs_in(node: roxmltree::Node, name: &str) -> Vec<String> {
    property(node, name)
        .map(|p| {
            p.descendants()
                .filter(|n| is(n, "href"))
                .filter_map(|n| n.text().map(|t| t.trim().to_owned()))
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn text_of(node: roxmltree::Node, name: &str) -> Option<String> {
    property(node, name).map(|n| n.text().unwrap_or_default().trim().to_owned())
}

fn parse(text: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse(text).with_context(|| t!("sync-dav-the-servers-answer-couldnt-be-read"))
}

impl Client {
    pub fn new(user: &str, password: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .use_preconfigured_tls(tls)
                .timeout(Duration::from_secs(60))
                .connect_timeout(Duration::from_secs(12))
                // Same-host redirects only, so the password stays with this
                // server; one elsewhere comes back as `Moved`, for discovery
                // to decide on (RFC 6764 §5).
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    let same = attempt.previous().first().is_some_and(|first| {
                        first.host_str() == attempt.url().host_str()
                            && first.port_or_known_default() == attempt.url().port_or_known_default()
                    });
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
            scheme: Scheme::Password,
            digest: Default::default(),
        })
    }
    // Backend-only until the UI calls it: no CalDAV/CardDAV sign-in yields
    // an access token yet (needs calendar/contacts OAuth scopes).
    #[allow(dead_code)]
    /// A client signing in with an OAuth access token (RFC 6750 §2.1).
    pub fn with_token(user: &str, token: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self { scheme: Scheme::Bearer, ..Self::new(user, token, tls)? })
    }
    /// `request` signed: Digest once the server asked for it (RFC 7616);
    /// else Basic (RFC 7617).
    fn signed(&self, request: reqwest::blocking::RequestBuilder, method: &str, url: &url::Url) -> reqwest::blocking::RequestBuilder {
        if self.scheme == Scheme::Bearer {
            return request.bearer_auth(&self.password);
        }
        let mut digest = self.digest.lock().unwrap_or_else(|e| e.into_inner());
        match digest.as_mut() {
            Some((challenge, count)) => {
                *count += 1;
                let uri = match url.query() {
                    Some(q) => format!("{}?{q}", url.path()),
                    None => url.path().to_owned(),
                };
                let cnonce = uuid::Uuid::new_v4().simple().to_string();
                request.header(
                    reqwest::header::AUTHORIZATION,
                    challenge.authorization(&self.user, &self.password, method, &uri, *count, &cnonce),
                )
            }
            None => request.basic_auth(&self.user, Some(&self.password)),
        }
    }
    /// After a 401: whether the server wants Digest with a challenge not
    /// tried yet (a first one, or a stale nonce, RFC 7616 §3.3), which is
    /// then used. False where trying again wouldn't help.
    fn take_challenge(&self, response: &reqwest::blocking::Response) -> bool {
        if self.scheme == Scheme::Bearer {
            return false;
        }
        let Some(challenge) = Digest::choose(
            response
                .headers()
                .get_all(reqwest::header::WWW_AUTHENTICATE)
                .iter()
                .filter_map(|v| v.to_str().ok()),
        ) else {
            return false;
        };
        let mut digest = self.digest.lock().unwrap_or_else(|e| e.into_inner());
        let stale = response
            .headers()
            .get_all(reqwest::header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|h| auth_params(h.split_once(' ').map_or("", |(_, p)| p)).iter().any(|(k, v)| k.eq_ignore_ascii_case("stale") && v.eq_ignore_ascii_case("true")));
        let new = digest.as_ref().is_none_or(|(d, _)| d.nonce != challenge.nonce);
        if new && (digest.is_none() || stale) {
            *digest = Some((challenge, 0));
            return true;
        }
        false
    }

    fn send(
        &self,
        method: &str,
        url: &url::Url,
        headers: &[(&str, &str)],
        body: String,
    ) -> Result<reqwest::blocking::Response> {
        let name = method;
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let attempt = |body: String| {
            let mut request = self.signed(self.http.request(method.clone(), url.clone()), name, url);
            for (k, v) in headers {
                request = request.header(*k, *v);
            }
            request.body(body).send().with_context(|| {
                t!("sync-dav-couldnt-reach", host = url.host_str().unwrap_or(t!("sync-dav-the-server")))
            })
        };
        let mut response = attempt(body.clone())?;
        // RFC 7616 §3.3: asked for Digest (or a fresh nonce), once more.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED && self.take_challenge(&response) {
            response = attempt(body)?;
        }
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
            bail!(t!("sync-dav-the-server-is-out-of-storage"));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!(t!("sync-dav-the-server-didnt-accept-the-user"));
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            bail!(t!("sync-dav-this-was-changed-on-the-server"));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            // Only the start: the reason (a precondition element) is there.
            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(response, 64 * 1024), &mut body);
            let body = String::from_utf8_lossy(&body);
            bail!(Forbidden(body.chars().take(4096).collect()));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(NotFound);
        }
        if status.is_redirection()
            && let Some(to) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|l| response.url().join(l).ok())
        {
            bail!(Moved(to));
        }
        ensure!(status.is_success() || status.as_u16() == 207, Answered(status));
        Ok(response)
    }

    /// The user's principal (RFC 5397 `current-user-principal`), from the
    /// address discovery starts at (`start_url`). A redirect to another
    /// host is followed only within the same domain (RFC 6764 §5, §8: the
    /// password goes along), at most a few times; a `.well-known` address
    /// the server doesn't have is tried as `/` instead (RFC 6764 §6).
    /// Without the property, the address itself (an older server's own
    /// principal URL). Returns the address that answered and the principal.
    pub fn principal(&self, start: &url::Url) -> Result<(url::Url, url::Url)> {
        let mut at = start.clone();
        let mut tried_root = false;
        for _ in 0..5 {
            match self.find_href(&at, "current-user-principal", DAV) {
                Ok(found) => {
                    let principal = found.unwrap_or_else(|| at.clone());
                    return Ok((at, principal));
                }
                Err(e) => {
                    if let Some(Moved(to)) = e.downcast_ref::<Moved>() {
                        ensure!(
                            to.scheme() == "https" || cfg!(test),
                            t!("sync-dav-the-server-sent-picacho-to-an")
                        );
                        ensure!(
                            same_domain(start.host_str().unwrap_or(""), to.host_str().unwrap_or("")),
                            t!("sync-dav-the-server-sent-picacho-to-outside", host = to.host_str().unwrap_or(t!("sync-dav-another-server")))
                        );
                        at = to.clone();
                        continue;
                    }
                    let well_known = at.path().starts_with("/.well-known/");
                    if well_known && !tried_root && (e.is::<NotFound>() || e.is::<Answered>()) {
                        tried_root = true;
                        at.set_path("/");
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        bail!(t!("sync-dav-the-server-redirected-too-many-times"))
    }

    fn xml(&self, method: &str, url: &url::Url, depth: &str, body: &str) -> Result<String> {
        body_text(self
            .send(
                method,
                url,
                &[
                    ("Depth", depth),
                    ("Content-Type", "application/xml; charset=utf-8"),
                ],
                body.to_owned(),
            )?)
    }

    /// Every href of a property at `url` (Depth 0) as written: for those
    /// that aren't addresses on the server (RFC 6638 §2.4.1's `mailto:`).
    pub fn hrefs(&self, url: &url::Url, property: &str, namespace: &str) -> Result<Vec<String>> {
        let body = format!(
            "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{namespace}\"><d:prop><c:{property}/></d:prop></d:propfind>"
        );
        let text = self.xml("PROPFIND", url, "0", &body)?;
        Ok(hrefs_in(parse(&text)?.root(), property))
    }
    /// A property's href at `url` (Depth 0).
    pub fn find_href(
        &self,
        url: &url::Url,
        property: &str,
        namespace: &str,
    ) -> Result<Option<url::Url>> {
        Ok(self.find_hrefs(url, property, namespace)?.into_iter().next())
    }
    /// Every href of a property at `url` (Depth 0), in the server's order.
    fn find_hrefs(&self, url: &url::Url, property: &str, namespace: &str) -> Result<Vec<url::Url>> {
        let body = format!(
            "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{namespace}\"><d:prop><c:{property}/></d:prop></d:propfind>"
        );
        let text = self.xml("PROPFIND", url, "0", &body)?;
        let doc = parse(&text)?;
        let mut out: Vec<url::Url> = Vec::new();
        for h in hrefs_in(doc.root(), property) {
            if let Ok(u) = url.join(&h)
                && same_site(&u, url)
                && !out.contains(&u)
            {
                out.push(u);
            }
        }
        Ok(out)
    }

    /// The collections of kind `kind` (`addressbook` or `calendar`) at
    /// `home` (Depth 1), or `home` itself if it is one.
    fn collections(&self, home: &url::Url, kind: &str) -> Result<Vec<Collection>> {
        let body = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:r=\"urn:ietf:params:xml:ns:carddav\" xmlns:a=\"http://apple.com/ns/ical/\" xmlns:cs=\"http://calendarserver.org/ns/\"><d:prop><d:resourcetype/><d:displayname/><d:current-user-privilege-set/><c:supported-calendar-component-set/><a:calendar-color/><d:sync-token/><cs:getctag/><c:max-resource-size/><r:max-resource-size/><r:supported-address-data/><c:supported-calendar-data/><c:min-date-time/><c:max-date-time/><c:max-instances/><c:max-attendees-per-instance/></d:prop></d:propfind>";
        let text = self.xml("PROPFIND", home, "1", body)?;
        let doc = parse(&text)?;
        let mut out = Vec::new();
        for (href, response) in responses(&doc) {
            let is_kind = response.descendants().any(|n| {
                is(&n, kind)
                    && n.parent()
                        .is_some_and(|p| is(&p, "resourcetype"))
            });
            if !is_kind {
                continue;
            }
            let Ok(url) = home.join(&href) else { continue };
            if !same_site(&url, home) {
                continue;
            }
            // Without a privilege set, assume it can be written.
            let privileges: Vec<&str> = response
                .descendants()
                .filter(|n| is(n, "privilege"))
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
                    .filter(|p| is(p, "propstat"))
                    .filter(|p| {
                        text_of(*p, "status").is_none_or(|s| s.contains(" 200 "))
                    })
                    .find_map(|p| text_of(p, name))
                    .unwrap_or_default()
            };
            let components = response
                .descendants()
                .filter(|n| is(n, "supported-calendar-component-set"))
                .flat_map(|set| set.children().filter(|c| is(c, "comp")))
                .filter_map(|c| c.attribute("name"))
                .map(str::to_ascii_uppercase)
                .collect();
            let vcard_versions = response
                .descendants()
                .filter(|n| is(n, "supported-address-data"))
                .flat_map(|set| set.children().filter(|c| is(c, "address-data-type")))
                .filter_map(|c| c.attribute("version"))
                .map(str::to_owned)
                .collect();
            let time = |name: &str| {
                chrono::NaiveDateTime::parse_from_str(found(name).trim_end_matches('Z'), "%Y%m%dT%H%M%S")
                    .ok()
                    .map(|t| t.and_utc().timestamp())
            };
            let limits = CalendarLimits {
                data: response
                    .descendants()
                    .filter(|n| is(n, "supported-calendar-data") && !failed_propstat(*n))
                    .flat_map(|set| set.children().filter(|c| is(c, "calendar-data")))
                    .map(|c| {
                        let kind = c.attribute("content-type").unwrap_or("text/calendar");
                        match c.attribute("version") {
                            Some(v) => format!("{kind} {v}"),
                            None => kind.to_owned(),
                        }
                    })
                    .collect(),
                min_time: time("min-date-time"),
                max_time: time("max-date-time"),
                max_instances: found("max-instances").parse().ok(),
                max_attendees: found("max-attendees-per-instance").parse().ok(),
            };
            out.push(Collection {
                href: url,
                name,
                writable,
                components,
                color: found("calendar-color"),
                sync_token: found("sync-token"),
                ctag: found("getctag"),
                max_size: found("max-resource-size").parse().unwrap_or(0),
                vcard_versions,
                limits,
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
                .find(|n| is(n, data))
                .and_then(|n| n.text())
            else {
                continue;
            };
            let Ok(href) = collection.join(&href) else {
                continue;
            };
            if !same_site(&href, collection) {
                continue;
            }
            out.push(Resource {
                href,
                etag: text_of(response, "getetag").unwrap_or_default(),
                data: content.to_owned(),
                schedule_tag: text_of(response, "schedule-tag").unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// A REPORT whose answer is the body as it is, not a multistatus (RFC
    /// 4791 §7.10 free-busy-query answers text/calendar), Depth 1.
    pub fn report_text(&self, collection: &url::Url, query: &str) -> Result<String> {
        self.xml("REPORT", collection, "1", query)
    }

    /// What changed in a collection since `token` (RFC 6578 sync-collection),
    /// as hrefs and ETags only. An empty token lists everything.
    pub fn changes(&self, collection: &url::Url, token: &str) -> Result<Changes> {
        // A server may answer in parts (RFC 6578 §3.6: 507 on the
        // collection itself): ask again from its new token until whole.
        let mut all = Changes::default();
        let mut token = token.to_owned();
        let mut limit = Some(SYNC_LIMIT);
        for _ in 0..100 {
            let (part, truncated) = match self.changes_once(collection, &token, limit) {
                // RFC 6578 §3.7: a server that won't limit its answer
                // refuses the limit; asked again without it.
                Err(e) if limit.is_some() && e.downcast_ref::<Forbidden>().is_some_and(|f| f.0.contains("number-of-matches-within-limits")) => {
                    limit = None;
                    self.changes_once(collection, &token, None)?
                }
                other => other?,
            };
            all.removed.retain(|r| !part.changed.iter().any(|(h, _)| h == r));
            all.changed.retain(|(h, _)| !part.removed.contains(h));
            all.changed.extend(part.changed);
            all.removed.extend(part.removed);
            let moved = part.token != token;
            token = part.token;
            all.token = token.clone();
            if !truncated || !moved {
                break;
            }
        }
        Ok(all)
    }
    fn changes_once(&self, collection: &url::Url, token: &str, limit: Option<u32>) -> Result<(Changes, bool)> {
        // RFC 6578 §3.7: at most `limit` changes at a time; the server
        // marks a cut-short answer with a 507 on the collection, and the
        // rest are asked for from its new token.
        let limit = limit.map_or(String::new(), |n| format!("<d:limit><d:nresults>{n}</d:nresults></d:limit>"));
        let body = format!(
            "<?xml version=\"1.0\"?><d:sync-collection xmlns:d=\"DAV:\"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level>{limit}<d:prop><d:getetag/></d:prop></d:sync-collection>",
            xml_escape(token)
        );
        let response = self.send(
            "REPORT",
            collection,
            &[("Content-Type", "application/xml; charset=utf-8")],
            body,
        );
        let text = match response {
            Ok(r) => body_text(r)?,
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
            // CalDAV's Schedule-Tag with each (RFC 6638 §3.2.10), so an
            // edit can be conditional on it.
            let tag = if namespace == CALDAV { "<c:schedule-tag/>" } else { "" };
            let body = format!(
                "<?xml version=\"1.0\"?><c:{report} xmlns:d=\"DAV:\" xmlns:c=\"{namespace}\"><d:prop><d:getetag/>{tag}<c:{data}/></d:prop>{refs}</c:{report}>"
            );
            out.extend(self.list(collection, &body, data)?);
        }
        Ok(out)
    }

    /// The user's home for collections of `kind` (`calendar-home-set` or
    /// `addressbook-home-set`), from the server's address as `discover`
    /// finds it.
    pub fn home(&self, given: &str, service: &str, home_set: &str, namespace: &str) -> Result<url::Url> {
        let (_, principal) = self.principal(&start_url(given, service)?)?;
        self.find_href(&principal, home_set, namespace)?
            .with_context(|| t!("sync-dav-this-server-doesnt-say-where-your"))
    }
    /// The compliance classes in a resource's `DAV:` header (OPTIONS),
    /// lowercase: `calendar-auto-schedule` says the server schedules
    /// (RFC 6638 §2).
    pub fn compliance(&self, url: &url::Url) -> Result<Vec<String>> {
        let response = self.send("OPTIONS", url, &[], String::new())?;
        Ok(response
            .headers()
            .get_all("DAV")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|c| c.trim().to_ascii_lowercase())
            .filter(|c| !c.is_empty())
            .collect())
    }
    /// The account's scheduling outbox (`schedule-outbox-URL`, RFC 6638).
    pub fn outbox(&self, given: &str) -> Result<url::Url> {
        let (_, principal) = self.principal(&start_url(given, "caldav")?)?;
        self.find_href(&principal, "schedule-outbox-URL", CALDAV)?
            .with_context(|| t!("sync-dav-this-server-cant-say-when-people"))
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
        body_text(self
            .send(
                "POST",
                outbox,
                &[
                    ("Content-Type", content_type),
                    ("Originator", originator),
                    ("Recipient", &recipients),
                ],
                body,
            )?)
    }
    /// Makes an address book at `href` in `home`: by extended MKCOL (RFC
    /// 5689) where the home's DAV header says `extended-mkcol` (§3.1),
    /// else, as RFC 6352 §6.3.1 lets an older server do, by plain MKCOL
    /// with no body, then PROPPATCH of its resource type and name.
    pub fn make_address_book(&self, home: &url::Url, href: &url::Url, name: &str, setup: &CollectionSetup) -> Result<()> {
        let extended = self.compliance(home).map(|c| c.iter().any(|k| k == "extended-mkcol")).unwrap_or(false);
        if extended {
            return self.make_collection_with(href, false, name, "", setup);
        }
        self.send("MKCOL", href, &[], String::new())?;
        let description = if setup.description.trim().is_empty() {
            String::new()
        } else {
            format!("<c:addressbook-description>{}</c:addressbook-description>", xml_escape(setup.description.trim()))
        };
        let body = format!(
            "<?xml version=\"1.0\"?><d:propertyupdate xmlns:d=\"DAV:\" xmlns:c=\"{CARDDAV}\"><d:set><d:prop><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype><d:displayname>{}</d:displayname>{description}</d:prop></d:set></d:propertyupdate>",
            xml_escape(name)
        );
        let text = self.xml("PROPPATCH", href, "0", &body)?;
        let refused = refused_properties(&text);
        if !refused.is_empty() {
            // A plain collection was made, not an address book: taken back.
            let _ = self.delete(href, "");
            bail!(t!("sync-dav-the-server-refused", properties = refused.join(", ")));
        }
        Ok(())
    }
    /// Makes a collection at `href`: a calendar (MKCALENDAR, RFC 4791
    /// §5.3.1) for events, or an address book (extended MKCOL, RFC 5689),
    /// named `name`, with a calendar's colour where given, and a
    /// description and calendar time zone (RFC 4791 §5.2.1–§5.2.2). The server's answer is
    /// read (RFC 4791 §5.3.1.2, RFC 5689 §3.2: a `mkcol-response` or
    /// `mkcalendar-response` saying which properties it refused); one it
    /// refuses means the collection wasn't made.
    pub fn make_collection_with(
        &self,
        href: &url::Url,
        calendar: bool,
        name: &str,
        color: &str,
        setup: &CollectionSetup,
    ) -> Result<()> {
        let name = xml_escape(name);
        let description = if setup.description.trim().is_empty() {
            String::new()
        } else if calendar {
            format!("<c:calendar-description>{}</c:calendar-description>", xml_escape(setup.description.trim()))
        } else {
            format!("<c:addressbook-description>{}</c:addressbook-description>", xml_escape(setup.description.trim()))
        };
        let (method, body) = if calendar {
            let color = if color.is_empty() {
                String::new()
            } else {
                format!("<a:calendar-color>{}</a:calendar-color>", xml_escape(color))
            };
            // RFC 4791 §5.2.2: a VCALENDAR holding one VTIMEZONE.
            let zone = match setup.timezone.as_deref().and_then(|z| crate::sync::ical::vtimezone(z, 2020..=2040)) {
                Some(tz) => format!(
                    "<c:calendar-timezone>{}</c:calendar-timezone>",
                    xml_escape(&format!(
                        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Pinal Technologies//Picacho//EN\r\n{tz}END:VCALENDAR\r\n"
                    ))
                ),
                None => String::new(),
            };
            (
                "MKCALENDAR",
                format!(
                    "<?xml version=\"1.0\"?><c:mkcalendar xmlns:d=\"DAV:\" xmlns:c=\"{CALDAV}\" xmlns:a=\"http://apple.com/ns/ical/\"><d:set><d:prop><d:displayname>{name}</d:displayname>{description}{color}{zone}<c:supported-calendar-component-set><c:comp name=\"VEVENT\"/><c:comp name=\"VTODO\"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"
                ),
            )
        } else {
            (
                "MKCOL",
                format!(
                    "<?xml version=\"1.0\"?><d:mkcol xmlns:d=\"DAV:\" xmlns:c=\"{CARDDAV}\"><d:set><d:prop><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype><d:displayname>{name}</d:displayname>{description}</d:prop></d:set></d:mkcol>"
                ),
            )
        };
        let response = self.send(
            method,
            href,
            &[("Content-Type", "application/xml; charset=utf-8")],
            body,
        );
        let text = match response {
            Ok(r) => body_text(r)?,
            // RFC 5689 §3.2, RFC 4791 §5.3.1.2: refused with a 403 whose
            // body names the properties it wouldn't set.
            Err(e) => {
                let refused = e.downcast_ref::<Forbidden>().map(|f| refused_properties(&f.0)).unwrap_or_default();
                if refused.is_empty() {
                    return Err(e);
                }
                bail!(t!("sync-dav-the-server-refused", properties = refused.join(", ")));
            }
        };
        let refused = refused_properties(&text);
        ensure!(refused.is_empty(), t!("sync-dav-the-server-refused", properties = refused.join(", ")));
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
        Ok(self.put_scheduled(href, content_type, etag, "", data)?.0)
    }
    /// As `put`; with `schedule_tag` (RFC 6638 §3.2), only over that
    /// scheduling version (If-Schedule-Tag-Match), which a guest's answer
    /// arriving on the server doesn't change, so an attendee's own change
    /// isn't refused because the organizer's copy merely took a reply.
    /// Returns the new ETag and Schedule-Tag, where the server says.
    pub fn put_scheduled(
        &self,
        href: &url::Url,
        content_type: &str,
        etag: &str,
        schedule_tag: &str,
        data: String,
    ) -> Result<(String, String)> {
        let mut headers = vec![("Content-Type", content_type)];
        if !schedule_tag.is_empty() {
            headers.push(("If-Schedule-Tag-Match", schedule_tag));
        } else if etag.is_empty() {
            headers.push(("If-None-Match", "*"));
        } else if !etag.starts_with("W/") {
            // A weak ETag never matches If-Match's strong comparison (RFC
            // 9110 §13.1.1): with one, the write goes without a condition.
            headers.push(("If-Match", etag));
        }
        let response = self.send("PUT", href, &headers, data)?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let (mut etag, mut tag) = (header("ETag"), header("Schedule-Tag"));
        // A strong ETag only (RFC 4918 §8.6 lets a server change the data
        // as it stores it, then sending none or a weak one): the stored
        // version's own is asked for, so the next edit isn't taken for a
        // new resource (If-None-Match: *) and refused.
        if (etag.is_empty() || etag.starts_with("W/"))
            && let Ok((fresh, fresh_tag)) = self.etag_of(href)
        {
            etag = fresh;
            if tag.is_empty() {
                tag = fresh_tag;
            }
        }
        Ok((etag, tag))
    }
    /// A resource's current ETag and Schedule-Tag (PROPFIND, depth 0).
    fn etag_of(&self, href: &url::Url) -> Result<(String, String)> {
        let body = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:schedule-tag/></d:prop></d:propfind>";
        let text = self.xml("PROPFIND", href, "0", body)?;
        let doc = parse(&text)?;
        let found = responses(&doc).into_iter().next().map(|(_, r)| {
            (
                text_of(r, "getetag").unwrap_or_default(),
                text_of(r, "schedule-tag").unwrap_or_default(),
            )
        });
        found.with_context(|| t!("sync-dav-the-server-didnt-say-the-saved"))
    }
    /// Changes a collection's name and (a calendar's) colour (PROPPATCH,
    /// RFC 4918 §9.2). An empty `color` leaves it as it is.
    pub fn rename_collection(&self, href: &url::Url, name: &str, color: &str) -> Result<()> {
        let color = if color.is_empty() {
            String::new()
        } else {
            format!("<a:calendar-color>{}</a:calendar-color>", xml_escape(color))
        };
        let body = format!(
            "<?xml version=\"1.0\"?><d:propertyupdate xmlns:d=\"DAV:\" xmlns:a=\"http://apple.com/ns/ical/\"><d:set><d:prop><d:displayname>{}</d:displayname>{color}</d:prop></d:set></d:propertyupdate>",
            xml_escape(name)
        );
        let text = self.xml("PROPPATCH", href, "0", &body)?;
        // 200 or 204 without a body: done.
        if text.trim().is_empty() {
            return Ok(());
        }
        // A 207 whose propstat refuses a property: say so.
        let doc = parse(&text)?;
        let refused = doc
            .descendants()
            .filter(|n| is(n, "propstat"))
            .filter_map(|p| p.children().find(|c| is(c, "status")).and_then(|s| s.text()))
            .any(|s| !s.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2')));
        ensure!(!refused, t!("sync-dav-the-server-didnt-accept-the-new"));
        Ok(())
    }
    /// Removes a whole collection (a calendar or address book) and what is
    /// in it (DELETE, RFC 4918 §9.6.1).
    pub fn delete_collection(&self, href: &url::Url) -> Result<()> {
        self.delete(href, "")
    }
    /// The account's storage used and left (RFC 4331), in octets, at
    /// `url`; `None` where the server doesn't say.
    pub fn quota(&self, url: &url::Url) -> Result<Option<(u64, u64)>> {
        let body = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:quota-used-bytes/><d:quota-available-bytes/></d:prop></d:propfind>";
        let text = self.xml("PROPFIND", url, "0", body)?;
        let doc = parse(&text)?;
        let number = |name: &str| text_of(doc.root(), name).and_then(|t| t.parse::<u64>().ok());
        // quota-used-bytes is a SHOULD (RFC 4331 §4): without it, what's
        // left is known and none counted as used.
        Ok(match (number("quota-used-bytes"), number("quota-available-bytes")) {
            (Some(used), Some(left)) => Some((used, left)),
            (None, Some(left)) => Some((0, left)),
            _ => None,
        })
    }
    /// The account's scheduling inbox (`schedule-inbox-URL`, RFC 6638
    /// §2.2), where a scheduling server delivers invitations and answers.
    pub fn inbox(&self, given: &str) -> Result<url::Url> {
        let (_, principal) = self.principal(&start_url(given, "caldav")?)?;
        self.find_href(&principal, "schedule-inbox-URL", CALDAV)?
            .with_context(|| t!("sync-dav-this-server-has-no-scheduling-inbox"))
    }
    /// Adds, replaces or removes a managed attachment (RFC 8607 §3.4–§3.6),
    /// for the whole event or only the occurrences `rids` names (§3.4:
    /// `rid=` RECURRENCE-ID values, `M` for the series itself).
    pub fn attachment(
        &self,
        href: &url::Url,
        etag: &str,
        change: &AttachmentChange,
        rids: &[String],
    ) -> Result<AttachmentResult> {
        let mut url = href.clone();
        {
            let mut q = url.query_pairs_mut();
            match change {
                AttachmentChange::Add { .. } => q.append_pair("action", "attachment-add"),
                AttachmentChange::Update { managed_id, .. } => {
                    q.append_pair("action", "attachment-update").append_pair("managed-id", managed_id)
                }
                AttachmentChange::Remove { managed_id } => {
                    q.append_pair("action", "attachment-remove").append_pair("managed-id", managed_id)
                }
            };
            if !rids.is_empty() {
                q.append_pair("rid", &rids.join(","));
            }
        }
        let mut request = self.signed(self.http.post(url.clone()), "POST", &url);
        match change {
            AttachmentChange::Add { name, content_type, data } | AttachmentChange::Update { name, content_type, data, .. } => {
                request = request
                    .header("Content-Type", *content_type)
                    .header("Content-Disposition", content_disposition(name))
                    .body(data.clone());
            }
            AttachmentChange::Remove { .. } => {}
        }
        if !etag.is_empty() && !etag.starts_with("W/") {
            request = request.header("If-Match", etag);
        }
        let response = request.send().with_context(|| {
            t!("sync-dav-couldnt-reach", host = url.host_str().unwrap_or(t!("sync-dav-the-server")))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            bail!(t!("sync-dav-this-was-changed-on-the-server"));
        }
        if status == reqwest::StatusCode::INSUFFICIENT_STORAGE
            || status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        {
            bail!(t!("sync-dav-the-server-has-no-room-for"));
        }
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::CONFLICT {
            // RFC 8607 §3.8: a precondition says which limit it was.
            let body = body_text(response).unwrap_or_default();
            bail!(
                "{}",
                if body.contains("max-attachment-size") {
                    t!("sync-dav-this-file-is-larger-than-the")
                } else if body.contains("max-attachments-per-resource") {
                    t!("sync-dav-this-event-has-as-many-attachments")
                } else if body.contains("valid-managed-id") {
                    t!("sync-dav-that-attachment-is-no-longer-on")
                } else if body.contains("valid-rid") {
                    t!("sync-dav-that-occurrence-isnt-part-of-the")
                } else {
                    t!("sync-dav-the-calendar-server-refused-the-attachment")
                }
            );
        }
        ensure!(status.is_success(), t!("sync-dav-the-server-didnt-change-the-attachment", status = status));
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        // RFC 8607 §3.4: the new attachment's id, for updating or removing it.
        Ok(AttachmentResult { etag: header("ETag"), managed_id: header("Cal-Managed-ID") })
    }
    /// A calendar home's limits on managed attachments (RFC 8607 §6.1–§6.2:
    /// `max-attachment-size`, `max-attachments-per-resource`), where it says.
    pub fn attachment_limits(&self, url: &url::Url) -> Result<(Option<u64>, Option<u64>)> {
        let body = format!(
            "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"{CALDAV}\"><d:prop><c:max-attachment-size/><c:max-attachments-per-resource/></d:prop></d:propfind>"
        );
        let text = self.xml("PROPFIND", url, "0", &body)?;
        let doc = parse(&text)?;
        let number = |name: &str| text_of(doc.root(), name).and_then(|t| t.parse::<u64>().ok());
        Ok((number("max-attachment-size"), number("max-attachments-per-resource")))
    }
    /// Whether a calendar collection keeps attachments itself (RFC 8607
    /// `calendar-managed-attachments` in its DAV header).
    pub fn manages_attachments(&self, url: &url::Url) -> Result<bool> {
        Ok(self
            .compliance(url)?
            .iter()
            .any(|c| c == "calendar-managed-attachments"))
    }

    /// Copies (COPY, RFC 4918 §9.8) or moves (MOVE, §9.9) a resource to
    /// `to` on the same server, never over one already there
    /// (`Overwrite: F`, §10.6), and only if it is still the version with
    /// `etag`. Returns the ETag at `to`, where the server says.
    pub fn transfer(&self, from: &url::Url, to: &url::Url, etag: &str, keep_original: bool) -> Result<String> {
        ensure!(from.origin() == to.origin(), t!("sync-dav-a-copy-or-move-stays-on"));
        let mut headers = vec![("Destination", to.as_str()), ("Overwrite", "F")];
        if !etag.is_empty() && !etag.starts_with("W/") {
            headers.push(("If-Match", etag));
        }
        let response = self.send(if keep_original { "COPY" } else { "MOVE" }, from, &headers, String::new())?;
        // 201 made, 204 replaced; a 207 lists what failed (§9.8.8, §9.9.4).
        if response.status().as_u16() == 207 {
            let text = body_text(response)?;
            let doc = parse(&text)?;
            let failed = responses(&doc).any(|(_, r)| {
                r.children().find(|c| is(c, "status")).and_then(|s| s.text()).is_some_and(|s| !s.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2')))
            });
            ensure!(!failed, t!("sync-dav-the-server-didnt-all-of-it", action = if keep_original { "copy" } else { "move" }));
            return Ok(String::new());
        }
        Ok(response.headers().get("ETag").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned())
    }
    // Backend-only until the UI calls it: CalDAV and CardDAV writes are
    // guarded by If-Match/If-Schedule-Tag-Match already; a lock is for a
    // longer edit (a WebDAV file) that nothing here makes yet.
    #[allow(dead_code)]
    /// Locks a resource for writing (LOCK, RFC 4918 §9.10): an exclusive
    /// write lock for `seconds`, so another client's change can't land
    /// between reading and writing it. Returns the lock token (§10.5).
    pub fn lock(&self, href: &url::Url, owner: &str, seconds: u32) -> Result<String> {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:lockinfo xmlns:d=\"DAV:\"><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype><d:owner>{}</d:owner></d:lockinfo>",
            xml_escape(owner)
        );
        let timeout = format!("Second-{seconds}");
        let response = self.send(
            "LOCK",
            href,
            &[("Content-Type", "application/xml; charset=utf-8"), ("Depth", "0"), ("Timeout", &timeout)],
            body,
        )?;
        // The Lock-Token header (§10.5), else the body's locktoken.
        let header = response
            .headers()
            .get("Lock-Token")
            .and_then(|v| v.to_str().ok())
            .map(|t| t.trim().trim_start_matches('<').trim_end_matches('>').to_owned());
        let text = body_text(response)?;
        let token = header.filter(|t| !t.is_empty()).or_else(|| {
            let doc = roxmltree::Document::parse(&text).ok()?;
            let token = doc.descendants().find(|n| is(n, "locktoken"))?;
            token.descendants().find(|n| is(n, "href"))?.text().map(|t| t.trim().to_owned())
        });
        token.with_context(|| t!("sync-dav-the-server-didnt-say-the-locks"))
    }
    // Backend-only until the UI calls it (see `lock`).
    #[allow(dead_code)]
    /// Releases a lock (UNLOCK, RFC 4918 §9.11).
    pub fn unlock(&self, href: &url::Url, token: &str) -> Result<()> {
        self.send("UNLOCK", href, &[("Lock-Token", &format!("<{token}>"))], String::new())?;
        Ok(())
    }

    /// Removes a resource, if it is still the version with `etag`.
    pub fn delete(&self, href: &url::Url, etag: &str) -> Result<()> {
        let headers: Vec<(&str, &str)> = if etag.is_empty() || etag.starts_with("W/") {
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
/// The most a server's answer may be: a whole calendar or address book
/// read at once is large, but past this it is a fault or an attack, and
/// would be parsed (and kept) in memory whole.
const MOST_BODY: u64 = 256 * 1024 * 1024;
/// A response's body as text, refusing one larger than `MOST_BODY`.
fn body_text(response: reqwest::blocking::Response) -> Result<String> {
    use std::io::Read;
    if response.content_length().is_some_and(|n| n > MOST_BODY) {
        bail!(t!("sync-dav-the-servers-answer-is-too-large"));
    }
    let mut data = Vec::new();
    response.take(MOST_BODY + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= MOST_BODY,
        t!("sync-dav-the-servers-answer-is-too-large")
    );
    Ok(match String::from_utf8(data) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A sync-collection answer: changed resources (with an ETag), removed ones
/// (a 404 status), and the new token.
fn parse_changes(collection: &url::Url, text: &str) -> Result<(Changes, bool)> {
    let doc = parse(text)?;
    let mut out = Changes::default();
    let mut truncated = false;
    for (href, response) in responses(&doc) {
        let Ok(url) = collection.join(&href) else {
            continue;
        };
        if !same_site(&url, collection) {
            continue;
        }
        // The collection itself may be listed; it isn't one of its items,
        // but its 507 says the answer was cut short.
        if url.path().trim_end_matches('/') == collection.path().trim_end_matches('/') {
            truncated |= response
                .children()
                .find(|c| is(c, "status"))
                .and_then(|s| s.text())
                .is_some_and(|s| s.contains(" 507 "));
            continue;
        }
        let status = response
            .children()
            .find(|c| is(c, "status"))
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
        .find(|n| is(n, "sync-token"))
        .and_then(|n| n.text())
        .unwrap_or_default()
        .trim()
        .to_owned();
    ensure!(!out.token.is_empty(), t!("sync-dav-the-servers-changes-came-without-a"));
    Ok((out, truncated))
}

/// The properties a `mkcol-response` or `mkcalendar-response` (RFC 5689
/// §3.2, RFC 4791 §5.3.1.2), or any propstat answer, says weren't set: a
/// non-2xx propstat's property names. Empty for an empty or plain body.
fn refused_properties(text: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(text) else { return Vec::new() };
    doc.descendants()
        .filter(|n| is(n, "propstat"))
        .filter(|p| {
            p.children()
                .find(|c| is(c, "status"))
                .and_then(|s| s.text())
                .is_some_and(|s| !s.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2')))
        })
        .flat_map(|p| p.children().filter(|c| is(c, "prop")))
        .flat_map(|prop| prop.children().filter(|c| c.is_element()).map(|c| c.tag_name().name().to_owned()))
        .collect()
}

/// What a new collection is made with besides its name and colour.
#[derive(Clone, Debug, Default)]
pub struct CollectionSetup {
    /// `calendar-description` or `addressbook-description`.
    pub description: String,
    /// A calendar's time zone (an IANA name), for its floating times and
    /// time-range queries (RFC 4791 §5.2.2).
    pub timezone: Option<String>,
}

/// The server refused a request (403), with its body: a precondition
/// element such as `valid-sync-token` says why.
#[derive(Debug)]
pub struct Forbidden(pub String);
impl std::fmt::Display for Forbidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("sync-dav-the-server-doesnt-allow-this-change"))
    }
}
impl std::error::Error for Forbidden {}

/// A change to a managed attachment (RFC 8607 §3.4–§3.6).
pub enum AttachmentChange<'a> {
    Add { name: &'a str, content_type: &'a str, data: Vec<u8> },
    /// Replaces the file with MANAGED-ID `managed_id`.
    Update { managed_id: &'a str, name: &'a str, content_type: &'a str, data: Vec<u8> },
    Remove { managed_id: &'a str },
}
/// What the server answered to an attachment change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachmentResult {
    /// The event's new ETag, where it said.
    pub etag: String,
    /// The attachment's MANAGED-ID (`Cal-Managed-ID`), for an add or update.
    pub managed_id: String,
}

/// A Content-Disposition for a file named `name` (RFC 8607 §3.4, RFC 6266
/// §4.3): a plain ASCII `filename`, and RFC 8187's `filename*` with the
/// name as it is where it isn't plain ASCII.
pub(crate) fn content_disposition(name: &str) -> String {
    let plain: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '%') || c == ' ' { c } else { '_' })
        .collect();
    if plain == name {
        return format!("attachment;filename=\"{plain}\"");
    }
    // RFC 8187 §3.2.1: attr-char stays, everything else is %-encoded UTF-8.
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment;filename=\"{plain}\";filename*=UTF-8''{encoded}")
}

/// The server sent the request elsewhere: a redirect to another host,
/// which isn't followed with the password unasked.
#[derive(Debug)]
pub struct Moved(pub url::Url);
impl std::fmt::Display for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("sync-dav-the-server-moved-this-to", host = self.0.host_str().unwrap_or(t!("sync-dav-another-address"))))
    }
}
impl std::error::Error for Moved {}

/// Whether `to` is in the same domain as `from`: the same host, or one in
/// the same registered domain by the Public Suffix List (`p12-caldav.icloud.com`
/// for `caldav.icloud.com`; not `evil.co.uk` for `example.co.uk`, nor another
/// customer's `b.synology.me` for `a.synology.me`, whose suffixes are
/// shared by many owners), as `same_site` and JMAP's redirects judge.
fn same_domain(from: &str, to: &str) -> bool {
    let (from, to) = (from.trim_end_matches('.').to_ascii_lowercase(), to.trim_end_matches('.').to_ascii_lowercase());
    if from.is_empty() || to.is_empty() {
        return false;
    }
    if from == to {
        return true;
    }
    // Addresses have no domain: only the same one (above).
    let ip = |h: &str| h.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().is_ok();
    !ip(&from) && !ip(&to) && psl::domain_str(&from).is_some_and(|d| Some(d) == psl::domain_str(&to))
}

/// Where discovery starts for the address `given` (RFC 6764): the address
/// itself where it has a path; else the domain's SRV record (and TXT
/// `path=`, §3–§4) where it names a host in the domain; else
/// `/.well-known/<service>` (§5).
pub fn start_url(given: &str, service: &str) -> Result<url::Url> {
    let given = given.trim();
    ensure!(!given.is_empty(), t!("sync-dav-enter-the-servers-address"));
    let given = if given.contains("://") {
        given.to_owned()
    } else {
        format!("https://{given}")
    };
    let mut start = url::Url::parse(&given).with_context(|| t!("sync-dav-the-servers-address-isnt-valid"))?;
    ensure!(
        start.scheme() == "https" || cfg!(test),
        t!("sync-dav-the-servers-address-must-start-with")
    );
    if start.path() == "/" || start.path().is_empty() {
        if !cfg!(test)
            && let Some(found) = start.host_str().and_then(|d| srv_start(service, d))
        {
            start = found;
        } else {
            start.set_path(&format!("/.well-known/{service}"));
        }
    }
    Ok(start)
}

/// The server has no such resource.
#[derive(Debug)]
pub struct NotFound;
impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(t!("sync-dav-the-server-has-no-such-item"))
    }
}
impl std::error::Error for NotFound {}

/// The server answered with a status other than success.
#[derive(Debug)]
pub struct Answered(reqwest::StatusCode);
impl std::fmt::Display for Answered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("sync-dav-the-server-answered", status = self.0))
    }
}
impl std::error::Error for Answered {}

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
    let start = start_url(given, service)?;
    let mut found = client.collections(&start, kind).unwrap_or_default();
    if found.is_empty() {
        let (_, principal) = client.principal(&start)?;
        // RFC 4791 §6.2.1, RFC 6352 §7.1.1: a home set may list several
        // homes; the collections in each are the account's.
        let homes = client.find_hrefs(&principal, home_set, namespace)?;
        ensure!(!homes.is_empty(), t!("sync-dav-this-server-doesnt-list-any-for"));
        let mut failed = None;
        for home in &homes {
            match client.collections(home, kind) {
                Ok(more) => {
                    for c in more {
                        if !found.iter().any(|f| f.href == c.href) {
                            found.push(c);
                        }
                    }
                }
                Err(e) => failed = Some(e),
            }
        }
        // One home failing is that home's trouble; all of them, the server's.
        if found.is_empty()
            && let Some(e) = failed
        {
            return Err(e);
        }
    }
    ensure!(
        !found.is_empty(),
        t!("sync-dav-nothing-was-found-on-the-server")
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
    // A deadline for each request: a test expecting more requests than the
    // client makes fails, naming what came, instead of waiting forever.
    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        let mut seen: Seen = Vec::new();
        for _ in 0..count {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        let requests: Vec<String> = seen.iter().map(|(m, p, _)| format!("{m} {p}")).collect();
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the test server waited for request {} of {count}; it had {requests:?}",
                            seen.len() + 1
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            let mut auth = false;
            let mut conditions = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                // Conditional headers, for tests to check: kept before the body.
                if ["if-schedule-tag-match:", "destination:", "overwrite:", "timeout:", "lock-token:"].iter().any(|h| line.to_ascii_lowercase().starts_with(h)) {
                    conditions.push_str(&line);
                }
                // "ana:secret", base64; or a Digest answer, which the test
                // checks itself (it's passed on with the body).
                auth |= line.contains("Basic YW5hOnNlY3JldA==") || line.contains("Bearer secret");
                if line.to_ascii_lowercase().starts_with("authorization: digest ") {
                    auth = true;
                    conditions.push_str(&line);
                }
                if line == "\r\n" {
                    break;
                }
            }
            assert!(auth, "every request signs in");
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = conditions + &String::from_utf8_lossy(&body);
            let mut parts = first.split(' ');
            let method = parts.next().unwrap().to_owned();
            let path = parts.next().unwrap().to_owned();
            let (status, text) = respond(&method, &path, &body);
            // A redirect: its body is where to.
            let (extra, text) = if status.starts_with("401") {
                // A challenge: its body is the WWW-Authenticate value.
                (format!("WWW-Authenticate: {text}\r\n"), String::new())
            } else if status.starts_with('3') {
                (format!("Location: {text}\r\n"), String::new())
            } else if method == "OPTIONS" {
                // A scheduling server's compliance classes.
                ("DAV: 1, 3, calendar-access, calendar-auto-schedule, calendar-managed-attachments\r\n".to_owned(), text)
            } else if method == "PUT" {
                ("Schedule-Tag: \"s-1\"\r\n".to_owned(), text)
            } else {
                (String::new(), text)
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nETag: \"e-{}\"\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
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
            max_size: 0,
            vcard_versions: Vec::new(),
            limits: Default::default(),
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

    /// RFC 5689 §3.1: an address book is made by extended MKCOL where the
    /// home says `extended-mkcol`; else by MKCOL and PROPPATCH, and a
    /// PROPPATCH refused takes the plain collection back.
    #[test]
    fn address_books_without_extended_mkcol() -> Result<()> {
        let (port, server) = test_server(4, |method, path, body| match (method, path) {
            // The test server's OPTIONS lists no extended-mkcol.
            ("OPTIONS", _) => ("200 OK", String::new()),
            ("MKCOL", _) => {
                assert!(body.is_empty(), "{body}");
                ("201 Created", String::new())
            }
            ("PROPPATCH", _) => {
                assert!(body.contains("<c:addressbook/>") && body.contains("<d:displayname>Friends</d:displayname>"), "{body}");
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/ab/new/</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 403 Forbidden</d:status></d:propstat></d:response></d:multistatus>".into())
            }
            ("DELETE", _) => ("204 No Content", String::new()),
            _ => ("404 Not Found", String::new()),
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let at = |p: &str| url::Url::parse(&format!("http://127.0.0.1:{port}{p}")).unwrap();
        let err = client.make_address_book(&at("/ab/"), &at("/ab/new/"), "Friends", &CollectionSetup::default()).unwrap_err();
        assert!(err.to_string().contains("resourcetype"), "{err}");
        let seen: Vec<String> = server.join().unwrap().iter().map(|(m, ..)| m.clone()).collect();
        assert_eq!(seen, ["OPTIONS", "MKCOL", "PROPPATCH", "DELETE"]);
        Ok(())
    }

    /// RFC 4918 §9.8–§9.11: COPY and MOVE never overwrite and go only
    /// within the server; LOCK's token is read from its header or body.
    #[test]
    fn copy_move_and_lock() -> Result<()> {
        let (port, server) = test_server(4, |method, path, body| match (method, path) {
            ("MOVE", "/a/e.ics") => {
                let lower = body.to_ascii_lowercase();
                assert!(lower.contains("destination: http://127.0.0.1:") && lower.contains("/b/e.ics") && lower.contains("overwrite: f"), "{body}");
                ("201 Created", String::new())
            }
            ("COPY", "/a/f.ics") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/b/f.ics</d:href><d:status>HTTP/1.1 423 Locked</d:status></d:response></d:multistatus>".into()),
            ("LOCK", "/doc") => {
                assert!(body.contains("<d:exclusive/>") && body.contains("<d:owner>Picacho</d:owner>"), "{body}");
                ("200 OK", "<d:prop xmlns:d=\"DAV:\"><d:lockdiscovery><d:activelock><d:locktoken><d:href>urn:uuid:e71d4fae-5dec</d:href></d:locktoken></d:activelock></d:lockdiscovery></d:prop>".into())
            }
            ("UNLOCK", "/doc") => {
                assert!(body.to_ascii_lowercase().contains("lock-token: <urn:uuid:e71d4fae-5dec>"), "{body}");
                ("204 No Content", String::new())
            }
            _ => ("404 Not Found", String::new()),
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let at = |p: &str| url::Url::parse(&format!("http://127.0.0.1:{port}{p}")).unwrap();
        assert_eq!(client.transfer(&at("/a/e.ics"), &at("/b/e.ics"), "\"1\"", false)?, "\"e-0\"");
        assert!(client.transfer(&at("/a/f.ics"), &at("/b/f.ics"), "", true).unwrap_err().to_string().contains("copy"));
        assert!(client.transfer(&at("/a/e.ics"), &url::Url::parse("http://other.example/b/e.ics")?, "", false).is_err());
        let token = client.lock(&at("/doc"), "Picacho", 60)?;
        assert_eq!(token, "urn:uuid:e71d4fae-5dec");
        client.unlock(&at("/doc"), &token)?;
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 4);
        Ok(())
    }

    /// RFC 7616 §3.9.1's own example, SHA-256 and MD5: the response the RFC
    /// gives; and a server asking for Digest is answered with it.
    #[test]
    fn digest_authentication() -> Result<()> {
        let challenges = [
            "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=MD5, nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\"",
            "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=SHA-256, nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\"",
        ];
        let chosen = Digest::choose(challenges.into_iter()).unwrap();
        assert!(chosen.sha256 && chosen.qop && !chosen.session);
        let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
        let sha = chosen.authorization("Mufasa", "Circle of Life", "GET", "/dir/index.html", 1, cnonce);
        assert!(sha.contains("response=\"753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1\""), "{sha}");
        assert!(sha.contains("nc=00000001") && sha.contains("opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""));
        let md5 = Digest::parse(challenges[0]).unwrap().authorization("Mufasa", "Circle of Life", "GET", "/dir/index.html", 1, cnonce);
        assert!(md5.contains("response=\"8ca523f5e9506fed4657c9700eebdbec\""), "{md5}");
        assert!(Digest::parse("Basic realm=\"x\"").is_none());
        assert!(Digest::parse("Digest realm=\"x\", nonce=\"n\", algorithm=SHA-512-256").is_none());
        // A server that wants Digest: asked again with it, and later
        // requests go with it (the count rising) from the start.
        let (port, server) = test_server(3, |_, _, body| {
            if body.to_ascii_lowercase().contains("authorization: digest ") {
                assert!(body.contains("username=\"ana\"") && body.contains("algorithm=SHA-256"), "{body}");
                ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/</d:href><d:propstat><d:prop><d:quota-available-bytes>5</d:quota-available-bytes></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into())
            } else {
                ("401 Unauthorized", "Digest realm=\"dav\", qop=\"auth\", algorithm=SHA-256, nonce=\"abc\"".into())
            }
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let url = url::Url::parse(&format!("http://127.0.0.1:{port}/"))?;
        assert_eq!(client.quota(&url)?, Some((0, 5)));
        assert_eq!(client.quota(&url)?, Some((0, 5)));
        let seen = server.join().unwrap();
        assert!(seen[2].2.contains("nc=00000002"), "{}", seen[2].2);
        // A wrong password stays a sign-in error, not a loop.
        let (port, server) = test_server(2, |_, _, _| ("401 Unauthorized", "Digest realm=\"dav\", qop=\"auth\", nonce=\"abc\"".into()));
        let url = url::Url::parse(&format!("http://127.0.0.1:{port}/"))?;
        let err = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?.quota(&url).unwrap_err();
        assert!(err.to_string().contains("user name and password"), "{err}");
        assert_eq!(server.join().unwrap().len(), 2);
        // A token goes as Bearer (RFC 6750 §2.1), and a Digest challenge
        // doesn't make it go as a password.
        let (port, server) = test_server(1, |_, _, _| ("401 Unauthorized", "Digest realm=\"dav\", nonce=\"abc\"".into()));
        let url = url::Url::parse(&format!("http://127.0.0.1:{port}/"))?;
        assert!(Client::with_token("ana", "secret", native_tls::TlsConnector::new()?)?.quota(&url).is_err());
        assert_eq!(server.join().unwrap().len(), 1);
        Ok(())
    }

    /// RFC 6578 §3.6–§3.7: changes are asked for with a limit; an answer
    /// cut short (507 on the collection) continues from its token; a server
    /// refusing the limit is asked without one.
    #[test]
    fn sync_collection_asks_with_a_limit_and_continues() -> Result<()> {
        let item = |h: &str, e: &str| format!("<d:response><d:href>/c/{h}</d:href><d:propstat><d:prop><d:getetag>\"{e}\"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let (port, server) = test_server(3, move |_, path, body| {
            assert_eq!(path, "/c/");
            let limited = body.contains("<d:limit><d:nresults>1000</d:nresults></d:limit>");
            if body.contains("<d:sync-token>t0</d:sync-token>") {
                assert!(limited, "{body}");
                ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\">{}<d:response><d:href>/c/</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response><d:sync-token>t1</d:sync-token></d:multistatus>", item("a", "1")))
            } else if body.contains("<d:sync-token>t1</d:sync-token>") && limited {
                ("403 Forbidden", "<d:error xmlns:d=\"DAV:\"><d:number-of-matches-within-limits/></d:error>".into())
            } else {
                assert!(!limited);
                ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\">{}<d:sync-token>t2</d:sync-token></d:multistatus>", item("b", "2")))
            }
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let href = url::Url::parse(&format!("http://127.0.0.1:{port}/c/"))?;
        let changes = client.changes(&href, "t0")?;
        assert_eq!(changes.token, "t2");
        assert_eq!(changes.changed.iter().map(|(h, _)| h.path()).collect::<Vec<_>>(), ["/c/a", "/c/b"]);
        assert_eq!(server.join().unwrap().len(), 3);
        Ok(())
    }

    /// Elements are read in the namespace they belong in: a `response` or
    /// `getetag` in another vocabulary isn't WebDAV's.
    #[test]
    fn elements_are_read_in_their_namespace() {
        let text = "<d:multistatus xmlns:d=\"DAV:\" xmlns:x=\"urn:example\"><x:response><x:href>/fake</x:href></x:response><d:response><d:href>/real</d:href><d:propstat><d:prop><x:getetag>\"no\"</x:getetag><d:getetag>\"yes\"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        let doc = parse(text).unwrap();
        let found: Vec<(String, Option<String>)> = responses(&doc).map(|(h, r)| (h, text_of(r, "getetag"))).collect();
        assert_eq!(found, [("/real".to_owned(), Some("\"yes\"".to_owned()))]);
        // A default namespace counts as well as a prefix.
        let plain = parse("<multistatus xmlns=\"DAV:\"><response><href>/a</href></response></multistatus>").unwrap();
        assert_eq!(responses(&plain).count(), 1);
        // Elements with no namespace at all aren't WebDAV's.
        let none = parse("<multistatus><response><href>/a</href></response></multistatus>").unwrap();
        assert_eq!(responses(&none).count(), 0);
    }

    /// RFC 6764 §5: `.well-known` redirecting to another host in the same
    /// domain is followed; one outside it isn't; a `.well-known` the server
    /// lacks is tried as `/` (§6).
    #[test]
    fn well_known_redirects_and_falls_back_to_root() -> Result<()> {
        assert!(same_domain("caldav.icloud.com", "p12-caldav.icloud.com"));
        assert!(same_domain("example.com", "dav.example.com"));
        assert!(!same_domain("dav.example.com", "evil.com"));
        // The parent of `example.co.uk` is `co.uk`: only hosts under the
        // address itself count there.
        assert!(!same_domain("example.co.uk", "evil.co.uk"));
        assert!(same_domain("example.co.uk", "dav.example.co.uk"));
        assert!(!same_domain("localhost", "127.0.0.1"));
        // Suffixes many owners share are no one's domain.
        assert!(!same_domain("alice.synology.me", "mallory.synology.me"));
        assert!(!same_domain("a.herokuapp.com", "b.herokuapp.com"));
        assert!(!same_domain("site.github.io", "other.github.io"));
        assert!(!same_domain("mairie.gouv.fr", "evil.gouv.fr"));
        assert!(!same_domain("x.govt.nz", "y.govt.nz"));
        assert!(same_domain("alice.synology.me", "dav.alice.synology.me"));
        assert!(!same_domain("127.0.0.1", "10.0.0.1"));
        let principal = "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/ana/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        // Another port, the same host: redirected there and followed.
        let (target, target_server) = test_server(1, move |_, path, _| {
            assert_eq!(path, "/dav/");
            ("207 Multi-Status", principal.into())
        });
        let (port, server) = test_server(3, move |_, path, _| match path {
            "/.well-known/caldav" => ("301 Moved Permanently", format!("http://127.0.0.1:{target}/dav/")),
            "/.well-known/carddav" => ("404 Not Found", String::new()),
            "/" => ("207 Multi-Status", principal.into()),
            _ => ("404 Not Found", String::new()),
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let (at, p) = client.principal(&start_url(&format!("http://127.0.0.1:{port}"), "caldav")?)?;
        assert_eq!((at.port(), p.path()), (Some(target), "/p/ana/"));
        target_server.join().unwrap();
        let (at, p) = client.principal(&start_url(&format!("http://127.0.0.1:{port}"), "carddav")?)?;
        assert_eq!((at.path(), p.path()), ("/", "/p/ana/"));
        assert_eq!(server.join().unwrap().len(), 3);
        // Another host altogether: not followed with the password.
        let (port, server) = test_server(1, move |_, _, _| ("302 Found", "http://localhost:1/dav/".into()));
        let err = client.principal(&start_url(&format!("http://127.0.0.1:{port}"), "caldav")?).unwrap_err();
        assert!(err.to_string().contains("outside its domain"), "{err}");
        server.join().unwrap();
        Ok(())
    }

    /// A home set listing two homes (RFC 4791 §6.2.1): the calendars of
    /// both are found, a home that fails doesn't hide the other's.
    #[test]
    fn every_home_in_a_home_set_is_read() -> Result<()> {
        let calendar = |href: &str| format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        let (port, server) = test_server(6, move |method, path, _| match (method, path) {
            // Not a collection itself: the principal is asked.
            ("PROPFIND", "/.well-known/caldav") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/.well-known/caldav</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/p/ana/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/p/ana/") => ("207 Multi-Status", "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/p/ana/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/home1/</d:href><d:href>/home2/</d:href><d:href>/gone/</d:href></c:calendar-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>".into()),
            ("PROPFIND", "/home1/") => ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">{}</d:multistatus>", calendar("/home1/work/"))),
            ("PROPFIND", "/home2/") => ("207 Multi-Status", format!("<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">{}</d:multistatus>", calendar("/home2/shared/"))),
            _ => ("404 Not Found", String::new()),
        });
        let client = Client::new("ana", "secret", native_tls::TlsConnector::new()?)?;
        let found = discover(&client, &format!("http://127.0.0.1:{port}"), "caldav", "calendar", "calendar-home-set", CALDAV)?;
        let paths: Vec<&str> = found.iter().map(|c| c.href.path()).collect();
        assert_eq!(paths, ["/home1/work/", "/home2/shared/"]);
        assert_eq!(server.join().unwrap().len(), 6);
        Ok(())
    }

    /// Hrefs a server names are followed (with the password) only on its
    /// own site: the same host or registrable domain, and scheme.
    #[test]
    fn hrefs_stay_on_the_servers_site() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        let base = u("https://caldav.icloud.com/123/principal/");
        assert!(same_site(&u("https://p42-caldav.icloud.com/123/calendars/"), &base));
        assert!(same_site(&u("https://CALDAV.icloud.com./x"), &base));
        assert!(!same_site(&u("https://evil.example/123/"), &base));
        assert!(!same_site(&u("http://caldav.icloud.com/123/"), &base));
        assert!(!same_site(&u("https://icloud.com.evil.example/"), &base));
        // A public suffix shared is no site shared.
        assert!(!same_site(&u("https://alice.github.io/"), &u("https://bob.github.io/")));
        // Addresses by number: the same one, and port.
        assert!(same_site(&u("http://127.0.0.1:8080/a"), &u("http://127.0.0.1:8080/b")));
        assert!(!same_site(&u("http://127.0.0.1:8081/a"), &u("http://127.0.0.1:8080/b")));
        assert!(!same_site(&u("http://10.0.0.1/a"), &u("http://127.0.0.1/b")));
    }
}
