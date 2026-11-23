//! JMAP (RFC 8620, the core; RFC 8621, mail): mail over HTTPS and JSON,
//! the protocol Fastmail, Stalwart, Cyrus and Apache James speak.
//!
//! A session starts from `https://<server>/.well-known/jmap`, which says
//! where to send method calls (`apiUrl`), where blobs download and upload,
//! and where pushed changes arrive (`eventSourceUrl`). Every request after
//! that is a batch of method calls, POSTed together; later calls can use
//! earlier ones' results (back-references), so a listing is one round trip.
//!
//! Signing in is HTTP Basic with the address (or user name) and password,
//! or a bearer token: Fastmail's API tokens, which some servers require in
//! place of a password. Picacho tries Basic, then the password as a token.
//!
//! These helpers only talk to the server; Manage keeps the result
//! (`manage/jmap.rs`).
use crate::manage::status::AccessFailure;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    sync::Mutex,
    time::Duration,
};

pub const CORE: &str = "urn:ietf:params:jmap:core";
pub const MAIL: &str = "urn:ietf:params:jmap:mail";
pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
pub const QUOTA: &str = "urn:ietf:params:jmap:quota";
/// JMAP for Sieve Scripts (RFC 9661).
pub const SIEVE: &str = "urn:ietf:params:jmap:sieve";
/// The automatic reply while away (RFC 8621 §8).
pub const VACATION: &str = "urn:ietf:params:jmap:vacationresponse";
/// Pushed changes over a WebSocket (RFC 8887).
pub const WEBSOCKET: &str = "urn:ietf:params:jmap:websocket";
/// Read receipts (RFC 9007).
pub const MDN: &str = "urn:ietf:params:jmap:mdn";
/// Blob management: copying and reading parts of blobs (RFC 9404).
pub const BLOB: &str = "urn:ietf:params:jmap:blob";
/// Address books and contact cards (RFC 9610), as JSContact (RFC 9553).
pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
/// Calendars and events (JMAP for Calendars), as JSCalendar (RFC 8984).
pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";
/// Who an account belongs to and shares with (JMAP Sharing, RFC 9670).
pub const PRINCIPALS: &str = "urn:ietf:params:jmap:principals";
/// RFC 9670 §2.2: a principal's busy times (Principal/getAvailability).
pub const PRINCIPALS_AVAILABILITY: &str = "urn:ietf:params:jmap:principals:availability";
/// Whether a method belongs to JMAP Mail (RFC 8621).
fn mail_method(name: &str) -> bool {
    ["Mailbox/", "Thread/", "Email/", "SearchSnippet/", "Identity/", "EmailSubmission/", "VacationResponse/"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// What a listing asks of each message: what the message list shows, and
/// the headers Picacho keeps from a source it hasn't downloaded.
pub const LIST_PROPERTIES: &[&str] = &[
    "id",
    "threadId",
    "mailboxIds",
    "keywords",
    "size",
    "receivedAt",
    "messageId",
    "references",
    "inReplyTo",
    "from",
    "to",
    "cc",
    "replyTo",
    "subject",
    "sentAt",
    "hasAttachment",
    "preview",
    // RFC 8621 §4.1.2: List-Unsubscribe is an RFC 2369 header, which may
    // only be asked for as URLs; servers such as Fastmail refuse `asText`.
    // The other two are from later RFCs, so any form will do.
    "header:List-Unsubscribe:asURLs",
    "header:List-Unsubscribe-Post:asText",
    "header:Disposition-Notification-To:asText",
];

/// How long an ordinary request may take, and a download.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// How often the event stream is asked to send a ping, so a stopped watch
/// notices within this long and a dead connection is found.
const PING_SECONDS: u32 = 30;

/// A JMAP mailbox (a folder), as the server describes it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// `inbox`, `sent`, `drafts`, `archive`, `junk`, `trash`… (RFC 8457).
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub sort_order: u32,
    #[serde(default)]
    pub total_emails: u64,
    #[serde(default)]
    pub unread_emails: u64,
}

/// A name and address.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Address {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}
impl Address {
    pub fn email(&self) -> &str {
        self.email.as_deref().unwrap_or("")
    }
    pub fn name(&self) -> &str {
        self.name.as_deref().map(str::trim).unwrap_or("")
    }
    /// `Name <address>`, or the address alone.
    pub fn display(&self) -> String {
        if self.name().is_empty() {
            self.email().to_owned()
        } else {
            format!("{} <{}>", self.name(), self.email())
        }
    }
}
/// A list of addresses as one header value.
pub fn addresses(list: &Option<Vec<Address>>) -> String {
    list.iter()
        .flatten()
        .map(Address::display)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A message as a listing describes it (`LIST_PROPERTIES`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub mailbox_ids: HashMap<String, bool>,
    #[serde(default)]
    pub keywords: HashMap<String, bool>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub received_at: Option<String>,
    #[serde(default)]
    pub sent_at: Option<String>,
    #[serde(default)]
    pub message_id: Option<Vec<String>>,
    #[serde(default)]
    pub in_reply_to: Option<Vec<String>>,
    #[serde(default)]
    pub references: Option<Vec<String>>,
    #[serde(default)]
    pub from: Option<Vec<Address>>,
    #[serde(default)]
    pub to: Option<Vec<Address>>,
    #[serde(default)]
    pub cc: Option<Vec<Address>>,
    #[serde(default)]
    pub reply_to: Option<Vec<Address>>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub has_attachment: bool,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default, rename = "header:List-Unsubscribe:asURLs")]
    pub list_unsubscribe: Option<Vec<String>>,
    #[serde(default, rename = "header:List-Unsubscribe-Post:asText")]
    pub list_unsubscribe_post: Option<String>,
    #[serde(default, rename = "header:Disposition-Notification-To:asText")]
    pub receipt_to: Option<String>,
}
impl Email {
    /// The List-Unsubscribe header as mail carries it: each address in
    /// angle brackets.
    pub fn unsubscribe(&self) -> String {
        self.list_unsubscribe
            .iter()
            .flatten()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .map(|u| format!("<{u}>"))
            .collect::<Vec<_>>()
            .join(", ")
    }
    pub fn has_keyword(&self, k: &str) -> bool {
        self.keywords
            .iter()
            .any(|(key, on)| *on && key.eq_ignore_ascii_case(k))
    }
    pub fn in_mailbox(&self, id: &str) -> bool {
        self.mailbox_ids.get(id).copied().unwrap_or(false)
    }
    /// When it arrived, in seconds since the epoch; 0 when unknown.
    pub fn time(&self) -> i64 {
        self.received_at
            .as_deref()
            .or(self.sent_at.as_deref())
            .and_then(timestamp)
            .unwrap_or(0)
    }
}
/// An RFC 3339 date in seconds since the epoch.
pub fn timestamp(date: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date)
        .ok()
        .map(|d| d.timestamp())
}
/// Seconds since the epoch as JMAP's UTCDate.
pub fn utc_date(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// One page of a query, with the messages it found.
#[derive(Debug, Default)]
pub struct Page {
    pub emails: Vec<Email>,
    /// How many messages match, where asked for and the server says.
    pub total: Option<usize>,
    /// The Email state these were read in.
    pub state: String,
    /// The query's state, to ask later what changed in its results.
    pub query_state: String,
}

/// What changed since an Email state (`Email/changes`).
#[derive(Debug, Default)]
pub struct Changes {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub destroyed: Vec<String>,
    pub new_state: String,
    pub has_more: bool,
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty() && self.destroyed.is_empty()
    }
}

/// An address the account may send as.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Identity {
    pub id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// Whether this account may remove it (`mayDelete`).
    #[serde(default, rename = "mayDelete")]
    pub may_delete: bool,
}
impl Identity {
    /// Whether it sends as `address`: the same address, or a whole domain
    /// (`*@example.com`).
    pub fn covers(&self, address: &str) -> bool {
        if self.email.eq_ignore_ascii_case(address) {
            return true;
        }
        match (self.email.strip_prefix("*@"), address.rsplit_once('@')) {
            (Some(domain), Some((_, theirs))) => domain.eq_ignore_ascii_case(theirs),
            _ => false,
        }
    }
}

/// The body of a message too large to download whole.
#[derive(Debug, Default)]
pub struct Parts {
    pub text: String,
    pub html: String,
    /// (blob id, name, size, type).
    pub attachments: Vec<(String, String, usize, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Auth {
    Basic,
    Bearer,
}

/// A signed-in JMAP session.
pub struct Session {
    http: reqwest::blocking::Client,
    /// Verifies the server, for connections outside `http` (the WebSocket).
    tls: native_tls::TlsConnector,
    auth: Auth,
    login: String,
    password: String,
    api_url: String,
    download_url: String,
    upload_url: String,
    event_source_url: String,
    /// The last event's id from the event source, sent back when it is
    /// opened again.
    last_event_id: std::sync::Mutex<String>,
    account_id: String,
    /// Capabilities the server and the mail account have.
    submission: bool,
    /// The SMTP extensions submissions may use, with their arguments
    /// (`submissionExtensions`, RFC 8621 §1.3.2): DSN, REQUIRETLS,
    /// FUTURERELEASE…, by upper-case name.
    submission_extensions: HashMap<String, Vec<String>>,
    /// How far ahead a submission may be held for sending later, in
    /// seconds (`maxDelayedSend`); 0 where it can't.
    max_delayed_send: u64,
    quota: bool,
    /// The account has the automatic reply while away.
    vacation: bool,
    max_get: usize,
    max_set: usize,
    max_upload: usize,
    /// The most method calls one request may hold, and its largest size in
    /// bytes (`maxCallsInRequest`, `maxSizeRequest`).
    max_calls: usize,
    max_request: usize,
    /// RFC 8620 §2: the most requests the server takes at once from one
    /// client (`maxConcurrentRequests`).
    max_concurrent_requests: usize,
    /// The session resource's state when it was read. A response naming
    /// another one means the session changed (new addresses, limits or
    /// accounts): `stale` is set, and the session is read again.
    session_state: String,
    stale: std::sync::atomic::AtomicBool,
    /// Where pushed changes arrive over a WebSocket, where the server has
    /// one (RFC 8887).
    websocket_url: String,
    /// Other accounts with mail that this sign-in reaches (shared with
    /// it): (id, name). The one used is `account_id`.
    other_accounts: Vec<(String, String)>,
    mailboxes: Mutex<Vec<Mailbox>>,
    /// The capabilities the server and the mail account have (session and
    /// account capability URIs), for the extensions used when there.
    capabilities: std::collections::HashSet<String>,
    /// The sign-in's own accounts for contacts and calendars
    /// (`primaryAccounts`), where the server has them.
    contacts_account: Option<String>,
    primary_mail: String,
    calendars_account: Option<String>,
    /// The sign-in's principals account (JMAP Principals, RFC 9670).
    principals_account: Option<String>,
    /// The Mailbox state `mailboxes` was read in, for Mailbox/changes.
    mailbox_state: Mutex<String>,
    /// The account's name for itself (the session's `username`).
    username: String,
    /// The Sieve extensions of the account's filters, where it has JMAP
    /// for Sieve Scripts.
    sieve: Option<Vec<String>>,
}

/// A Sieve script on the server (RFC 9661).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SieveScript {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub blob_id: String,
    #[serde(default)]
    pub is_active: bool,
}

/// The session resource's address for a server.
pub fn session_url(host: &str, port: u16) -> String {
    if port == 443 || port == 0 {
        format!("https://{host}/.well-known/jmap")
    } else {
        format!("https://{host}:{port}/.well-known/jmap")
    }
}

/// A transport failure in the terms Picacho's retries understand: nothing
/// reached (try later), or the connection broke (a read may be repeated).
/// The most read of a JMAP session document or upload answer: a few
/// kilobytes are usual.
const SESSION_LIMIT: u64 = 16 * 1024 * 1024;
/// The most read of a method call's reply: a page of messages' properties,
/// which is far less however many it lists.
const REPLY_LIMIT: u64 = 256 * 1024 * 1024;
/// The most read of an error's text.
const ERROR_LIMIT: u64 = 64 * 1024;

fn transport(e: reqwest::Error, what: &str) -> anyhow::Error {
    let kind = if e.is_timeout() {
        Some(std::io::ErrorKind::TimedOut)
    } else if e.is_connect() {
        return anyhow::Error::new(crate::sync::retry::Unreachable(format!(
            "Couldn't reach the JMAP server: {e}"
        )));
    } else if e.is_request() || e.is_body() {
        Some(std::io::ErrorKind::ConnectionAborted)
    } else {
        None
    };
    match kind {
        Some(kind) => anyhow::Error::new(std::io::Error::new(kind, e.to_string()))
            .context(format!("JMAP {what} failed")),
        None => anyhow::Error::new(e).context(format!("JMAP {what} failed")),
    }
}

/// A server's refusal of a whole request (RFC 7807 problem details). A
/// server that is limiting requests (429) or busy (503) is reported as
/// `[THROTTLED]`, so the account's background work pauses as it does for
/// IMAP servers that say so (`imap::throttled`).
fn problem(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        return anyhow::anyhow!(
            "[THROTTLED] The JMAP server is limiting requests ({status}); background sync waits a while."
        );
    }
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = v["detail"]
        .as_str()
        .or(v["title"].as_str())
        .or(v["type"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| body.chars().take(200).collect());
    anyhow::anyhow!("The JMAP server answered {status}: {detail}")
}

fn usize_of(v: &Value, default: usize) -> usize {
    v.as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

impl Session {
    /// Signs in: reads the session resource at `host` and checks that the
    /// account has mail. `tls` verifies the server (the system's roots).
    pub fn open(
        host: &str,
        port: u16,
        login: &str,
        password: &str,
        account: &str,
        tls: native_tls::TlsConnector,
    ) -> Result<Self> {
        ensure!(!password.is_empty(), "Enter your password or API token.");
        // Fastmail's API tokens are bearer tokens: no need to try Basic first.
        let order = if password.starts_with("fmu") && !password.contains(char::is_whitespace) {
            [Auth::Bearer, Auth::Basic]
        } else {
            [Auth::Basic, Auth::Bearer]
        };
        Self::open_with(host, port, login, password, account, &order, tls)
    }
    /// Signs in with an OAuth access token.
    pub fn open_token(
        host: &str,
        port: u16,
        token: &str,
        account: &str,
        tls: native_tls::TlsConnector,
    ) -> Result<Self> {
        ensure!(
            !token.is_empty(),
            "Your sign-in expired. Reconnect the account."
        );
        Self::open_with(host, port, "", token, account, &[Auth::Bearer], tls)
    }
    fn open_with(
        host: &str,
        port: u16,
        login: &str,
        password: &str,
        account: &str,
        order: &[Auth],
        tls: native_tls::TlsConnector,
    ) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls.clone())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(12))
            // Redirects are followed here, keeping credentials on one host.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .https_only(!cfg!(test))
            .build()?;
        let start = session_url(host, port);
        let mut tried = Vec::new();
        for &auth in order {
            if auth == Auth::Bearer && password.chars().any(char::is_whitespace) {
                break;
            }
            match Self::fetch_session(&http, &start, auth, login, password) {
                Ok(resource) => {
                    return Self::from_resource(
                        http, tls, auth, login, password, account, &resource,
                    );
                }
                Err(e) if e.downcast_ref::<AccessFailure>().is_some() => tried.push(e),
                Err(e) => return Err(e),
            }
        }
        Err(tried
            .pop()
            .unwrap_or_else(|| anyhow::Error::new(AccessFailure::Authentication)))
    }

    fn fetch_session(
        http: &reqwest::blocking::Client,
        start: &str,
        auth: Auth,
        login: &str,
        password: &str,
    ) -> Result<Value> {
        let mut url = url::Url::parse(start)?;
        let host = url.host_str().unwrap_or_default().to_owned();
        for _ in 0..5 {
            let request = http.get(url.clone()).header("Accept", "application/json");
            let response = Self::authorize(request, auth, login, password)
                .send()
                .map_err(|e| transport(e, "sign-in"))?;
            let status = response.status();
            if status.is_redirection() {
                let next = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .context("The JMAP server redirected without saying where.")?;
                let next = url.join(next)?;
                // Credentials go only to the server that was entered.
                ensure!(
                    next.host_str() == Some(host.as_str()),
                    "The JMAP server sent Picacho on to {}; enter that as the server instead.",
                    next.host_str().unwrap_or("another server")
                );
                url = next;
                continue;
            }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(anyhow::Error::new(AccessFailure::Authentication).context(
                    "The JMAP server didn't accept the sign-in. Check the password, or use an API token if the server asks for one.",
                ));
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                bail!("{} doesn't offer JMAP (no /.well-known/jmap).", host);
            }
            let body = crate::sync::public_net::bounded_text(response, SESSION_LIMIT)
                .context("Couldn't read the JMAP server's session.")?;
            if !status.is_success() {
                return Err(problem(status, &body));
            }
            return serde_json::from_str(&body)
                .context("The JMAP server's session isn't JSON Picacho can read.");
        }
        bail!("The JMAP server redirected too many times.")
    }

    /// The session for the mail account `chosen` (empty: the sign-in's
    /// own, its primary mail account).
    fn from_resource(
        http: reqwest::blocking::Client,
        tls: native_tls::TlsConnector,
        auth: Auth,
        login: &str,
        password: &str,
        chosen: &str,
        v: &Value,
    ) -> Result<Self> {
        let caps = &v["capabilities"];
        ensure!(
            caps.get(MAIL).is_some(),
            "This JMAP server doesn't offer mail."
        );
        let account_id = if chosen.is_empty() {
            v["primaryAccounts"][MAIL]
                .as_str()
                .context("This JMAP account has no mail.")?
                .to_owned()
        } else {
            ensure!(
                v["accounts"][chosen]["accountCapabilities"].get(MAIL).is_some(),
                "This shared mail account is no longer shared with you."
            );
            chosen.to_owned()
        };
        let account = &v["accounts"][&account_id]["accountCapabilities"];
        let other_accounts = v["accounts"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, a)| {
                **id != account_id
                    && a["accountCapabilities"].get(MAIL).is_some()
                    && Some(id.as_str()) != v["primaryAccounts"][MAIL].as_str()
            })
            .map(|(id, a)| {
                let name = a["name"].as_str().unwrap_or(id).to_owned();
                (id.clone(), name)
            })
            .collect();
        let submission_caps = &account[SUBMISSION];
        let submission_extensions = submission_caps["submissionExtensions"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, args)| {
                let args = args
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a.as_str().map(str::to_owned))
                    .collect();
                (name.to_ascii_uppercase(), args)
            })
            .collect();
        let core = &caps[CORE];
        let url = |key: &str| -> Result<String> {
            let u = v[key]
                .as_str()
                .with_context(|| format!("The JMAP session has no {key}."))?;
            let parsed = url::Url::parse(u)
                .with_context(|| format!("The JMAP session's {key} isn't an address."))?;
            ensure!(
                parsed.scheme() == "https" || cfg!(test),
                "The JMAP server's {key} doesn't use https://."
            );
            Ok(u.to_owned())
        };
        Ok(Self {
            http,
            tls,
            auth,
            login: login.to_owned(),
            password: password.to_owned(),
            api_url: url("apiUrl")?,
            download_url: url("downloadUrl")?,
            upload_url: url("uploadUrl")?,
            event_source_url: url("eventSourceUrl").unwrap_or_default(),
            last_event_id: Default::default(),
            submission: caps.get(SUBMISSION).is_some()
                && (account.get(SUBMISSION).is_some() || account.is_null()),
            submission_extensions,
            max_delayed_send: submission_caps["maxDelayedSend"].as_u64().unwrap_or(0),
            quota: caps.get(QUOTA).is_some(),
            vacation: caps.get(VACATION).is_some()
                && (account.get(VACATION).is_some() || account.is_null()),
            max_get: usize_of(&core["maxObjectsInGet"], 500).min(500),
            max_set: usize_of(&core["maxObjectsInSet"], 500).min(500),
            max_upload: usize_of(&core["maxSizeUpload"], 50_000_000),
            // RFC 8620 asks servers for at least 16 calls and 10 MB.
            max_calls: usize_of(&core["maxCallsInRequest"], 16),
            max_request: usize_of(&core["maxSizeRequest"], 10_000_000),
            // RFC 8620 §2: servers SHOULD take at least 4; more than 8 at
            // once is never needed here.
            max_concurrent_requests: usize_of(&core["maxConcurrentRequests"], 4).min(8),
            session_state: v["state"].as_str().unwrap_or_default().to_owned(),
            stale: std::sync::atomic::AtomicBool::new(false),
            websocket_url: caps[WEBSOCKET]["url"]
                .as_str()
                .filter(|u| u.starts_with("wss://") || cfg!(test))
                .unwrap_or_default()
                .to_owned(),
            other_accounts,
            username: v["username"].as_str().unwrap_or_default().to_owned(),
            sieve: caps.get(SIEVE).map(|_| {
                let ext = &account[SIEVE]["sieveExtensions"];
                let ext = if ext.is_array() {
                    ext
                } else {
                    &caps[SIEVE]["sieveExtensions"]
                };
                ext.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            }),
            account_id,
            mailboxes: Mutex::new(Vec::new()),
            capabilities: caps
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k.clone())
                .filter(|k| {
                    // Mail-account extensions the account itself must have.
                    ![MDN, BLOB].contains(&k.as_str()) || account.get(k).is_some() || account.is_null()
                })
                .collect(),
            contacts_account: v["primaryAccounts"][CONTACTS].as_str().map(str::to_owned),
            primary_mail: v["primaryAccounts"][MAIL].as_str().unwrap_or_default().to_owned(),
            calendars_account: v["primaryAccounts"][CALENDARS].as_str().map(str::to_owned),
            principals_account: v["primaryAccounts"][PRINCIPALS].as_str().map(str::to_owned),
            mailbox_state: Mutex::new(String::new()),
        })
    }

    fn authorize(
        r: reqwest::blocking::RequestBuilder,
        auth: Auth,
        login: &str,
        password: &str,
    ) -> reqwest::blocking::RequestBuilder {
        match auth {
            Auth::Basic => r.basic_auth(login, Some(password)),
            Auth::Bearer => r.bearer_auth(password),
        }
    }
    fn signed(&self, r: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        Self::authorize(r, self.auth, &self.login, &self.password)
    }

    /// The account's name for itself: its address, on most servers.
    pub fn username(&self) -> &str {
        &self.username
    }
    /// Other accounts with mail this sign-in reaches: (id, name).
    pub fn other_accounts(&self) -> &[(String, String)] {
        &self.other_accounts
    }
    /// The session changed at the server since it was read (a response
    /// named another session state): it should be read again.
    pub fn stale(&self) -> bool {
        self.stale.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Whether submissions may use this SMTP extension (DSN, REQUIRETLS,
    /// FUTURERELEASE…).
    pub fn submits_with(&self, extension: &str) -> bool {
        self.submission_extensions
            .contains_key(&extension.to_ascii_uppercase())
    }
    /// How far ahead the server holds a submission for sending later, in
    /// seconds; 0 where it doesn't (FUTURERELEASE).
    pub fn max_delayed_send(&self) -> u64 {
        if self.submits_with("FUTURERELEASE") {
            self.max_delayed_send.max(
                self.submission_extensions["FUTURERELEASE"]
                    .first()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0),
            )
        } else {
            0
        }
    }
    /// Whether the account has the automatic reply while away.
    pub fn has_vacation(&self) -> bool {
        self.vacation
    }
    /// Whether the server pushes changes (an event source or a WebSocket).
    pub fn pushes(&self) -> bool {
        !self.event_source_url.is_empty() || !self.websocket_url.is_empty()
    }

    /// Sends method calls in one request, returning what the server
    /// answered each, in the calls' order. One method's error is that
    /// call's answer alone: the others' results still come back (RFC 8620
    /// §3.6.2), so what an earlier call made isn't lost.
    pub fn call(&self, calls: &[(&str, Value)]) -> Result<Vec<Answer>> {
        // More calls than the server takes at once go in several requests,
        // where none uses another's result.
        if calls.len() > self.max_calls {
            ensure!(
                !calls.iter().any(|(_, args)| uses_results(args)),
                "This JMAP server takes only {} method calls at a time.",
                self.max_calls
            );
            let mut out = Vec::with_capacity(calls.len());
            for chunk in calls.chunks(self.max_calls) {
                out.extend(self.call(chunk)?);
            }
            return Ok(out);
        }
        let mut using = vec![CORE, MAIL];
        if calls
            .iter()
            .any(|(n, _)| n.starts_with("EmailSubmission/") || n.starts_with("Identity/"))
        {
            using.push(SUBMISSION);
        }
        if calls.iter().any(|(n, _)| n.starts_with("Quota/")) {
            using.push(QUOTA);
        }
        if calls.iter().any(|(n, _)| n.starts_with("SieveScript/")) {
            using.push(SIEVE);
        }
        if calls.iter().any(|(n, _)| n.starts_with("VacationResponse/")) {
            using.push(VACATION);
        }
        if calls.iter().any(|(n, _)| n.starts_with("MDN/")) {
            using.push(MDN);
        }
        if calls
            .iter()
            .any(|(n, _)| n.starts_with("Blob/") && *n != "Blob/copy")
        {
            using.push(BLOB);
        }
        if calls
            .iter()
            .any(|(n, _)| n.starts_with("AddressBook/") || n.starts_with("ContactCard/"))
        {
            using.push(CONTACTS);
        }
        if calls.iter().any(|(n, _)| {
            n.starts_with("Calendar/")
                || n.starts_with("CalendarEvent/")
                || n.starts_with("ParticipantIdentity/")
        }) {
            using.push(CALENDARS);
        }
        if calls.iter().any(|(n, _)| n.starts_with("CalendarEventNotification/")) {
            using.push(CALENDARS);
        }
        if calls
            .iter()
            .any(|(n, _)| n.starts_with("Principal/") || n.starts_with("ShareNotification/"))
        {
            using.push(PRINCIPALS);
        }
        // Principal/getAvailability is its own capability (RFC 9670 §2.2).
        if calls.iter().any(|(n, _)| *n == "Principal/getAvailability") {
            using.push(PRINCIPALS_AVAILABILITY);
        }
        // Mail only where the server has it: a contacts- or calendars-
        // only server would refuse a request naming it.
        if !self.has(MAIL) && !calls.iter().any(|(n, _)| mail_method(n)) {
            using.retain(|c| *c != MAIL);
        }
        let method_calls: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(i, (name, args))| json!([name, args, format!("c{i}")]))
            .collect();
        let body = serde_json::to_vec(&json!({ "using": using, "methodCalls": method_calls }))?;
        ensure!(
            body.len() <= self.max_request,
            "This request is larger than the JMAP server takes ({} bytes at most).",
            self.max_request
        );
        let response = self
            .signed(self.http.post(&self.api_url))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .map_err(|e| transport(e, "request"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(AccessFailure::Authentication)
                .context("The JMAP server no longer accepts the sign-in."));
        }
        let text = crate::sync::public_net::bounded_text(response, REPLY_LIMIT)
            .context("Couldn't read the JMAP server's reply.")?;
        if !status.is_success() {
            return Err(problem(status, &text));
        }
        let v: Value =
            serde_json::from_str(&text).context("The JMAP server's reply isn't JSON.")?;
        if let Some(state) = v["sessionState"].as_str()
            && !self.session_state.is_empty()
            && state != self.session_state
        {
            self.stale.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let responses = v["methodResponses"]
            .as_array()
            .context("The JMAP server's reply has no method responses.")?;
        Ok(answers(calls, responses))
    }
    /// One method call's response arguments.
    fn one(&self, name: &str, args: Value) -> Result<Value> {
        self.call(&[(name, args)])?
            .into_iter()
            .next()
            .with_context(|| format!("The JMAP server didn't answer {name}."))?
            .into_value()
    }

    // Mailboxes.

    /// Reads the account's mailboxes again: once read, only those changed
    /// since (Mailbox/changes, then Mailbox/get of those in one request),
    /// else all of them.
    pub fn reload_mailboxes(&self) -> Result<Vec<Mailbox>> {
        const PROPERTIES: [&str; 7] =
            ["id", "name", "parentId", "role", "sortOrder", "totalEmails", "unreadEmails"];
        let since = self.mailbox_state.lock().unwrap().clone();
        let known = self.mailboxes.lock().unwrap().clone();
        if !since.is_empty() && !known.is_empty() {
            match self.mailbox_changes(&since, &PROPERTIES, known) {
                Ok(Some(list)) => return Ok(list),
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        let v = self.one(
            "Mailbox/get",
            json!({ "accountId": self.account_id, "ids": null, "properties": PROPERTIES }),
        )?;
        let list: Vec<Mailbox> = serde_json::from_value(v["list"].clone())
            .context("The JMAP server's mailboxes aren't in a form Picacho can read.")?;
        *self.mailboxes.lock().unwrap() = list.clone();
        *self.mailbox_state.lock().unwrap() = v["state"].as_str().unwrap_or_default().to_owned();
        Ok(list)
    }
    /// `known` brought up to date by Mailbox/changes since `since`, or
    /// `None` when the server can't say (or there are too many changes).
    fn mailbox_changes(
        &self,
        since: &str,
        properties: &[&str],
        known: Vec<Mailbox>,
    ) -> Result<Option<Vec<Mailbox>>> {
        // Two calls, so servers that take few at a time still answer in one
        // request (back-references can't cross requests).
        let changes = match self.one(
            "Mailbox/changes",
            json!({ "accountId": self.account_id, "sinceState": since, "maxChanges": 500 }),
        ) {
            Ok(v) => v,
            // Too old a state, or a server without it: read them all.
            Err(e) if must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let ids: Vec<&str> = ["created", "updated"]
            .iter()
            .flat_map(|k| changes[*k].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        let mut responses = vec![("Mailbox/changes".to_owned(), changes.clone())];
        for chunk in ids.chunks(self.max_get) {
            responses.push((
                "Mailbox/get".to_owned(),
                self.one(
                    "Mailbox/get",
                    json!({ "accountId": self.account_id, "ids": chunk, "properties": properties }),
                )?,
            ));
        }
        let Some((_, changes)) = responses.iter().find(|(n, _)| n == "Mailbox/changes") else {
            return Ok(None);
        };
        if changes["hasMoreChanges"].as_bool().unwrap_or(false) {
            return Ok(None);
        }
        let destroyed: Vec<&str> = changes["destroyed"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fresh: Vec<Mailbox> = Vec::new();
        for (name, v) in &responses {
            if name == "Mailbox/get" {
                fresh.extend(
                    serde_json::from_value::<Vec<Mailbox>>(v["list"].clone()).context(
                        "The JMAP server's mailboxes aren't in a form Picacho can read.",
                    )?,
                );
            }
        }
        let mut list: Vec<Mailbox> = known
            .into_iter()
            .filter(|m| !destroyed.contains(&m.id.as_str()))
            .filter(|m| !fresh.iter().any(|f| f.id == m.id))
            .collect();
        list.extend(fresh);
        *self.mailboxes.lock().unwrap() = list.clone();
        *self.mailbox_state.lock().unwrap() =
            changes["newState"].as_str().unwrap_or_default().to_owned();
        Ok(Some(list))
    }
    /// The mailboxes, as last read (reading them the first time).
    pub fn mailboxes(&self) -> Result<Vec<Mailbox>> {
        let known = self.mailboxes.lock().unwrap().clone();
        if known.is_empty() {
            self.reload_mailboxes()
        } else {
            Ok(known)
        }
    }
    pub fn create_mailbox(&self, name: &str, parent: Option<&str>) -> Result<String> {
        let v = self.one(
            "Mailbox/set",
            json!({
                "accountId": self.account_id,
                "create": { "new": { "name": name, "parentId": parent } },
            }),
        )?;
        if let Some(e) = v["notCreated"].get("new") {
            bail!("The server didn't make the folder: {}", set_error(e));
        }
        let id = v["created"]["new"]["id"]
            .as_str()
            .context("The server didn't say which folder it made.")?
            .to_owned();
        self.reload_mailboxes()?;
        Ok(id)
    }
    pub fn rename_mailbox(&self, id: &str, name: &str) -> Result<()> {
        let v = self.one(
            "Mailbox/set",
            json!({ "accountId": self.account_id, "update": { id: { "name": name } } }),
        )?;
        if let Some(e) = v["notUpdated"].get(id) {
            bail!("The server didn't rename the folder: {}", set_error(e));
        }
        self.reload_mailboxes()?;
        Ok(())
    }
    /// Deletes an empty mailbox; a mailbox with mail in it is refused.
    pub fn destroy_mailbox(&self, id: &str) -> Result<()> {
        let v = self.one(
            "Mailbox/set",
            json!({
                "accountId": self.account_id,
                "destroy": [id],
                "onDestroyRemoveEmails": false,
            }),
        )?;
        if let Some(e) = v["notDestroyed"].get(id) {
            if e["type"] == "mailboxHasEmail" {
                bail!("The folder still has mail in it. Move or delete its mail first.");
            }
            if e["type"] == "mailboxHasChild" {
                bail!("The folder has folders inside it. Delete those first.");
            }
            bail!("The server didn't delete the folder: {}", set_error(e));
        }
        self.reload_mailboxes()?;
        Ok(())
    }

    // Messages.

    /// One page of the messages matching `filter`, newest first, with
    /// their listing properties: a query and a get in one request.
    pub fn query(
        &self,
        filter: &Value,
        position: usize,
        limit: usize,
        total: bool,
    ) -> Result<Page> {
        let limit = limit.clamp(1, self.max_get);
        let responses = self.call(&[
            (
                "Email/query",
                json!({
                    "accountId": self.account_id,
                    "filter": filter,
                    "sort": [{ "property": "receivedAt", "isAscending": false }],
                    "position": position,
                    "limit": limit,
                    "calculateTotal": total,
                }),
            ),
            (
                "Email/get",
                json!({
                    "accountId": self.account_id,
                    "#ids": { "resultOf": "c0", "name": "Email/query", "path": "/ids" },
                    "properties": LIST_PROPERTIES,
                }),
            ),
        ])?;
        let query = responses
            .first()
            .context("The JMAP server didn't answer the query.")?
            .value()?;
        let get = responses
            .get(1)
            .context("The JMAP server didn't answer the query.")?
            .value()?;
        let order: Vec<&str> = query["ids"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut emails: Vec<Email> = serde_json::from_value(get["list"].clone())
            .context("The JMAP server's messages aren't in a form Picacho can read.")?;
        // In the query's order, which Email/get needn't keep.
        emails.sort_by_key(|e| {
            order
                .iter()
                .position(|id| *id == e.id)
                .unwrap_or(usize::MAX)
        });
        Ok(Page {
            emails,
            total: query["total"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()),
            state: get["state"].as_str().unwrap_or_default().to_owned(),
            query_state: query["queryState"].as_str().unwrap_or_default().to_owned(),
        })
    }
    /// Just the ids of the messages matching `filter`, from `position`.
    pub fn query_ids(&self, filter: &Value, position: usize, limit: usize) -> Result<Vec<String>> {
        let v = self.one(
            "Email/query",
            json!({
                "accountId": self.account_id,
                "filter": filter,
                "sort": [{ "property": "receivedAt", "isAscending": false }],
                "position": position,
                "limit": limit.max(1),
            }),
        )?;
        Ok(v["ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|i| i.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }
    /// The listing properties of messages by id; those gone are left out.
    pub fn emails(&self, ids: &[String]) -> Result<Vec<Email>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(self.max_get) {
            let v = self.one(
                "Email/get",
                json!({ "accountId": self.account_id, "ids": chunk, "properties": LIST_PROPERTIES }),
            )?;
            let list: Vec<Email> = serde_json::from_value(v["list"].clone())
                .context("The JMAP server's messages aren't in a form Picacho can read.")?;
            out.extend(list);
        }
        Ok(out)
    }
    /// The account's current Email state.
    pub fn state(&self) -> Result<String> {
        let v = self.one(
            "Email/get",
            json!({ "accountId": self.account_id, "ids": [], "properties": ["id"] }),
        )?;
        Ok(v["state"].as_str().unwrap_or_default().to_owned())
    }
    /// What changed since `since`, or `None` when the server can no longer
    /// say (the state is too old): then everything is read again.
    pub fn changes(&self, since: &str) -> Result<Option<Changes>> {
        let v = match self.one(
            "Email/changes",
            json!({ "accountId": self.account_id, "sinceState": since, "maxChanges": 500 }),
        ) {
            Ok(v) => v,
            Err(e) if must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let ids = |key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|i| i.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(Some(Changes {
            created: ids("created"),
            updated: ids("updated"),
            destroyed: ids("destroyed"),
            new_state: v["newState"].as_str().unwrap_or_default().to_owned(),
            has_more: v["hasMoreChanges"].as_bool().unwrap_or(false),
        }))
    }
    /// Changes messages: each id with its patch (`keywords/$seen`: true…).
    pub fn update(&self, patches: Map<String, Value>) -> Result<()> {
        let all: Vec<(String, Value)> = patches.into_iter().collect();
        for chunk in all.chunks(self.max_set) {
            let update: Map<String, Value> = chunk.iter().cloned().collect();
            let v = self.one(
                "Email/set",
                json!({ "accountId": self.account_id, "update": update }),
            )?;
            // Every refusal counts, not the first alone; a message that's
            // gone has nothing left to change.
            let failed: Vec<String> = v["notUpdated"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, e)| e["type"] != "notFound")
                .map(|(id, e)| format!("{id}: {}", set_error(e)))
                .collect();
            if !failed.is_empty() {
                bail!(
                    "The server didn't change {} {}: {}",
                    failed.len(),
                    if failed.len() == 1 { "message" } else { "messages" },
                    failed.join("; ")
                );
            }
        }
        Ok(())
    }
    /// Deletes messages for good.
    pub fn destroy(&self, ids: &[String]) -> Result<()> {
        for chunk in ids.chunks(self.max_set) {
            let v = self.one(
                "Email/set",
                json!({ "accountId": self.account_id, "destroy": chunk }),
            )?;
            let failed: Vec<String> = v["notDestroyed"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, e)| e["type"] != "notFound")
                .map(|(id, e)| format!("{id}: {}", set_error(e)))
                .collect();
            if !failed.is_empty() {
                bail!(
                    "The server didn't delete {} {}: {}",
                    failed.len(),
                    if failed.len() == 1 { "message" } else { "messages" },
                    failed.join("; ")
                );
            }
        }
        Ok(())
    }
    /// The blob ids of messages' sources, by message id.
    pub fn blob_ids(&self, ids: &[String]) -> Result<HashMap<String, String>> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(self.max_get) {
            let v = self.one(
                "Email/get",
                json!({ "accountId": self.account_id, "ids": chunk, "properties": ["id", "blobId"] }),
            )?;
            for e in v["list"].as_array().into_iter().flatten() {
                if let (Some(id), Some(blob)) = (e["id"].as_str(), e["blobId"].as_str()) {
                    out.insert(id.to_owned(), blob.to_owned());
                }
            }
        }
        Ok(out)
    }
    /// A message's source (its blob).
    pub fn source(&self, id: &str, limit: usize) -> Result<Vec<u8>> {
        let blob = self
            .blob_ids(&[id.to_owned()])?
            .remove(id)
            .context("Message no longer exists on the server")?;
        self.download(&blob, "message.eml", "message/rfc822", limit)
    }
    /// The header block of a message's source, without downloading the
    /// rest, where the server has the Blob extension (RFC 9404): what the
    /// sender check and details read from a message too large to keep.
    pub fn header_block(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let blob = self
            .blob_ids(&[id.to_owned()])?
            .remove(id)
            .context("Message no longer exists on the server")?;
        // Headers rarely run past 64 KiB, even with long Received chains.
        let Some(start) = self.blob_range(&blob, 0, 64 * 1024)? else {
            return Ok(None);
        };
        let end = start
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(start.len());
        Ok(Some(start[..end].to_vec()))
    }
    /// Downloads a blob, refusing one larger than `limit`.
    pub fn download(&self, blob: &str, name: &str, kind: &str, limit: usize) -> Result<Vec<u8>> {
        self.download_in(&self.account_id.clone(), blob, name, kind, limit)
    }
    /// As `download`, from another of the sign-in's accounts (contacts'
    /// or calendars', which may not be the mail account).
    pub fn download_in(&self, account: &str, blob: &str, name: &str, kind: &str, limit: usize) -> Result<Vec<u8>> {
        let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
        let url = self
            .download_url
            .replace("{accountId}", &enc(account))
            .replace("{blobId}", &enc(blob))
            .replace("{name}", &enc(name))
            .replace("{type}", &enc(kind));
        let response = self
            .signed(self.http.get(&url))
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .map_err(|e| transport(e, "download"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(AccessFailure::Authentication)
                .context("The JMAP server no longer accepts the sign-in."));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!("Message no longer exists on the server");
        }
        if !status.is_success() {
            let body = crate::sync::public_net::bounded_text(response, ERROR_LIMIT).unwrap_or_default();
            return Err(problem(status, &body));
        }
        if response
            .content_length()
            .is_some_and(|n| n as usize > limit)
        {
            bail!("Message exceeds the download limit.");
        }
        let mut bytes = Vec::new();
        use std::io::Read;
        response
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow::Error::new(e).context("JMAP download failed"))?;
        ensure!(bytes.len() <= limit, "Message exceeds the download limit.");
        Ok(bytes)
    }
    /// Uploads a blob, returning its id.
    pub fn upload(&self, bytes: &[u8], kind: &str) -> Result<String> {
        ensure!(
            bytes.len() <= self.max_upload,
            "The message is larger than the server accepts."
        );
        let url = self.upload_url.replace(
            "{accountId}",
            &url::form_urlencoded::byte_serialize(self.account_id.as_bytes()).collect::<String>(),
        );
        let response = self
            .signed(self.http.post(&url))
            .timeout(DOWNLOAD_TIMEOUT)
            .header("Content-Type", kind)
            .body(bytes.to_vec())
            .send()
            .map_err(|e| transport(e, "upload"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(AccessFailure::Authentication)
                .context("The JMAP server no longer accepts the sign-in."));
        }
        let text = crate::sync::public_net::bounded_text(response, SESSION_LIMIT)
            .context("Couldn't read the JMAP server's answer to the upload.")?;
        if !status.is_success() {
            return Err(problem(status, &text));
        }
        let v: Value =
            serde_json::from_str(&text).context("The JMAP upload's reply isn't JSON.")?;
        Ok(v["blobId"]
            .as_str()
            .context("The JMAP server didn't say where the upload went.")?
            .to_owned())
    }
    /// Adds a message to mailboxes from its source, with keywords
    /// (`$seen`, `$draft`). Returns its id.
    /// Adds a message already uploaded (a blob) to `mailboxes`.
    pub fn import_blob(&self, blob: &str, mailboxes: &[&str], keywords: &[&str]) -> Result<String> {
        let v = self.one(
            "Email/import",
            json!({ "accountId": self.account_id, "emails": { "m": import_args(blob, mailboxes, keywords) } }),
        )?;
        imported(&v, "m")
    }
    pub fn import(&self, raw: &[u8], mailboxes: &[&str], keywords: &[&str]) -> Result<String> {
        let blob = self.upload(raw, "message/rfc822")?;
        let v = self.one(
            "Email/import",
            json!({
                "accountId": self.account_id,
                "emails": { "m": import_args(&blob, mailboxes, keywords) },
            }),
        )?;
        imported(&v, "m")
    }
    /// The bodies of a message too large to download whole: its text and
    /// HTML parts' values, and its attachments' blobs.
    pub fn parts(&self, id: &str) -> Result<Parts> {
        let v = self.one(
            "Email/get",
            json!({
                "accountId": self.account_id,
                "ids": [id],
                "properties": ["id", "textBody", "htmlBody", "attachments", "bodyValues"],
                "fetchTextBodyValues": true,
                "fetchHTMLBodyValues": true,
                "maxBodyValueBytes": 4 * 1024 * 1024,
            }),
        )?;
        let e = v["list"]
            .get(0)
            .context("Message no longer exists on the server")?;
        let values = &e["bodyValues"];
        let joined = |key: &str, kind: &str| -> String {
            e[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| {
                    p["type"]
                        .as_str()
                        .is_some_and(|t| t.eq_ignore_ascii_case(kind))
                })
                .filter_map(|p| p["partId"].as_str())
                .filter_map(|part| values[part]["value"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(Parts {
            text: joined("textBody", "text/plain"),
            html: joined("htmlBody", "text/html"),
            attachments: e["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((
                        p["blobId"].as_str()?.to_owned(),
                        p["name"].as_str().unwrap_or("attachment").to_owned(),
                        usize_of(&p["size"], 0),
                        p["type"]
                            .as_str()
                            .unwrap_or("application/octet-stream")
                            .to_owned(),
                    ))
                })
                .collect(),
        })
    }

    // Sending.

    pub fn identities(&self) -> Result<Vec<Identity>> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        let v = self.one(
            "Identity/get",
            json!({ "accountId": self.account_id, "ids": null }),
        )?;
        serde_json::from_value(v["list"].clone())
            .context("The JMAP server's identities aren't in a form Picacho can read.")
    }
    /// Sends a finished message: it is added to `drafts` (or `sent`, where
    /// there's no Drafts) and submitted, and once the server accepts it,
    /// moved to `sent`. The server delivers it to `to` from `from`.
    pub fn send(
        &self,
        identity: &str,
        from: &str,
        to: &[String],
        raw: &[u8],
        drafts: Option<&str>,
        sent: Option<&str>,
        options: &SendOptions,
    ) -> Result<Submitted> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        // The envelope's SMTP parameters (RFC 8621 §7: `parameters`).
        let mut from_params = Map::new();
        let mut rcpt_params = Map::new();
        if options.require_tls {
            ensure!(
                self.submits_with("REQUIRETLS"),
                "This JMAP server can't promise this message stays encrypted all the way (it doesn't offer REQUIRETLS), so it wasn't sent. Turn off Require encryption in transit (Settings → Sending) to send it anyway."
            );
            from_params.insert("REQUIRETLS".into(), Value::Null);
        }
        let receipts = options.receipts.is_some() && self.submits_with("DSN");
        if let (true, Some(envid)) = (receipts, &options.receipts) {
            from_params.insert("RET".into(), json!("HDRS"));
            from_params.insert("ENVID".into(), json!(envid));
            rcpt_params.insert("NOTIFY".into(), json!("SUCCESS,FAILURE,DELAY"));
        }
        if let Some(at) = options.hold_until {
            let ahead = (at - chrono::Utc::now().timestamp()).max(0) as u64;
            ensure!(
                self.max_delayed_send() >= ahead,
                "This JMAP server can't hold a message that long before sending it."
            );
            let when = chrono::DateTime::from_timestamp(at, 0)
                .context("The time to send is out of range.")?;
            from_params.insert(
                "HOLDUNTIL".into(),
                json!(when.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            );
        }
        let mail_from = if from_params.is_empty() {
            json!({ "email": from })
        } else {
            json!({ "email": from, "parameters": from_params })
        };
        let rcpt_to: Vec<Value> = to
            .iter()
            .map(|t| {
                if rcpt_params.is_empty() {
                    json!({ "email": t })
                } else {
                    json!({ "email": t, "parameters": rcpt_params })
                }
            })
            .collect();
        let blob = self.upload(raw, "message/rfc822")?;
        let first: Vec<&str> = drafts.or(sent).into_iter().collect();
        ensure!(
            !first.is_empty(),
            "The account has neither a Drafts nor a Sent folder to send from."
        );
        let mut after = Map::new();
        after.insert("keywords/$draft".into(), Value::Null);
        if let (Some(d), Some(s)) = (drafts, sent) {
            after.insert(format!("mailboxIds/{d}"), Value::Null);
            after.insert(format!("mailboxIds/{s}"), Value::Bool(true));
        }
        let responses = self.call(&[
            (
                "Email/import",
                json!({
                    "accountId": self.account_id,
                    "emails": { "m": import_args(&blob, &first, &["$seen", "$draft"]) },
                }),
            ),
            (
                "EmailSubmission/set",
                json!({
                    "accountId": self.account_id,
                    "create": { "s": {
                        "identityId": identity,
                        "emailId": "#m",
                        "envelope": { "mailFrom": mail_from, "rcptTo": rcpt_to },
                    } },
                    "onSuccessUpdateEmail": { "#s": after },
                }),
            ),
        ])?;
        let [import, submission] = <[Answer; 2]>::try_from(responses)
            .map_err(|_| anyhow::anyhow!("The JMAP server didn't answer the send."))?;
        let id = imported(import.value()?, "m")?;
        // The submission refused as a whole (RFC 8620 §3.6.2): the copy
        // the import added goes again, as for a refused creation.
        let submission = match submission.into_value() {
            Ok(v) => v,
            Err(e) => {
                let _ = self.destroy(&[id]);
                return Err(e.context("The server didn't send the message."));
            }
        };
        if let Some(e) = submission["notCreated"].get("s") {
            // Not sent: the copy added for sending goes again.
            let _ = self.destroy(&[id]);
            bail!("The server didn't send the message: {}", set_error(e));
        }
        let created = submission["created"]
            .get("s")
            .context("The server didn't say whether it sent the message.")?;
        Ok(Submitted {
            receipts,
            id: created["id"].as_str().unwrap_or_default().to_owned(),
        })
    }
    /// One method call's response arguments, for callers outside this
    /// module (calendars and contacts: `jscal`).
    pub fn call_one(&self, name: &str, args: Value) -> Result<Value> {
        self.one(name, args)
    }
    /// The most ids one /get may name (`maxObjectsInGet`).
    pub fn max_get(&self) -> usize {
        self.max_get.max(1)
    }
    /// Runs `work` on each item, as many at once as the server takes
    /// requests (RFC 8620 §2: `maxConcurrentRequests`), returning the
    /// results in the items' order.
    pub fn in_parallel<T: Sync, R: Send>(&self, items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
        bounded_parallel(items, self.max_concurrent_requests, work)
    }
    /// The sign-in's calendar account, where the server has calendars.
    pub fn calendars_account(&self) -> Option<&str> {
        self.calendars_account
            .as_deref()
            .filter(|_| self.has(CALENDARS))
    }
    /// The sign-in's principals account, where the server has a directory
    /// of people (JMAP Principals).
    pub fn principals_account(&self) -> Option<&str> {
        self.principals_account.as_deref().filter(|_| self.has(PRINCIPALS))
    }
    /// The sign-in's contacts account, where the server has contacts.
    pub fn contacts_account(&self) -> Option<&str> {
        self.contacts_account.as_deref().filter(|_| self.has(CONTACTS))
    }
    /// The mail account this session reads.
    pub fn account(&self) -> &str {
        &self.account_id
    }
    /// The sign-in's own mail account (`primaryAccounts`).
    pub fn primary_mail_account(&self) -> &str {
        &self.primary_mail
    }
    /// Whether the server and account have the capability `uri`.
    pub fn has(&self, uri: &str) -> bool {
        self.capabilities.contains(uri)
    }
    /// What became of a submission, and whether it can still be cancelled
    /// (`undoStatus`). `None` once the server no longer has it.
    pub fn submission_status(&self, id: &str) -> Result<Option<SubmissionStatus>> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        let v = self.one(
            "EmailSubmission/get",
            json!({
                "accountId": self.account_id,
                "ids": [id],
                "properties": ["id", "undoStatus", "deliveryStatus"],
            }),
        )?;
        let Some(s) = v["list"].as_array().and_then(|l| l.first()) else {
            return Ok(None);
        };
        let mut delivery: Vec<(String, String, String)> = s["deliveryStatus"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(to, d)| {
                (
                    to.clone(),
                    d["delivered"].as_str().unwrap_or("unknown").to_owned(),
                    d["smtpReply"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        delivery.sort();
        Ok(Some(SubmissionStatus {
            undo: s["undoStatus"].as_str().unwrap_or("final").to_owned(),
            delivery,
        }))
    }
    /// The submissions changed since `since` (ids), and the new state, for
    /// following what was sent; `None` where the server can't say.
    pub fn submission_changes(&self, since: &str) -> Result<Option<(Vec<String>, String)>> {
        let v = match self.one(
            "EmailSubmission/changes",
            json!({ "accountId": self.account_id, "sinceState": since, "maxChanges": 200 }),
        ) {
            Ok(v) => v,
            Err(e) if must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let ids = ["created", "updated"]
            .iter()
            .flat_map(|k| v[*k].as_array().into_iter().flatten())
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect();
        Ok(Some((ids, v["newState"].as_str().unwrap_or_default().to_owned())))
    }
    /// The submissions not yet final (held for later, or still being
    /// tried), newest first: EmailSubmission/query on `undoStatus`.
    pub fn pending_submissions(&self) -> Result<Vec<String>> {
        let v = self.one(
            "EmailSubmission/query",
            json!({
                "accountId": self.account_id,
                "filter": { "undoStatus": "pending" },
                "sort": [{ "property": "sendAt", "isAscending": false }],
                "limit": 100,
            }),
        )?;
        Ok(v["ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect())
    }
    /// Takes back a submission the server still holds (`undoStatus`
    /// `pending`: held for later with HOLDUNTIL, or in the server's own
    /// undo window), and moves its message back to `drafts` as a draft.
    /// False if it had already gone.
    pub fn cancel_submission(&self, id: &str, drafts: Option<&str>) -> Result<bool> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        let v = self.one(
            "EmailSubmission/set",
            json!({
                "accountId": self.account_id,
                "update": { id: { "undoStatus": "canceled" } },
            }),
        )?;
        if let Some(e) = v["notUpdated"].get(id) {
            if e["type"] == "cannotUnsend" || e["type"] == "notFound" {
                return Ok(false);
            }
            bail!("The server couldn't take the message back: {}", set_error(e));
        }
        // Its message, back as a draft where it was moved to Sent.
        let got = self.one(
            "EmailSubmission/get",
            json!({ "accountId": self.account_id, "ids": [id], "properties": ["emailId"] }),
        )?;
        if let (Some(email), Some(drafts)) = (
            got["list"][0]["emailId"].as_str(),
            drafts,
        ) {
            let mut patch = Map::new();
            patch.insert("keywords/$draft".into(), Value::Bool(true));
            patch.insert("mailboxIds".into(), json!({ drafts: true }));
            let mut update = Map::new();
            update.insert(email.to_owned(), Value::Object(patch));
            let _ = self.update(update);
        }
        Ok(true)
    }
    /// Changes a sending identity: its name, Reply-To and signatures.
    pub fn update_identity(&self, id: &str, change: &IdentityChange) -> Result<()> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        let mut patch = Map::new();
        if let Some(name) = &change.name {
            patch.insert("name".into(), json!(name));
        }
        if let Some(reply_to) = &change.reply_to {
            let list: Vec<Value> = reply_to
                .iter()
                .map(|(name, email)| json!({ "name": name, "email": email }))
                .collect();
            patch.insert(
                "replyTo".into(),
                if list.is_empty() { Value::Null } else { Value::Array(list) },
            );
        }
        if let Some(text) = &change.text_signature {
            patch.insert("textSignature".into(), json!(text));
        }
        if let Some(html) = &change.html_signature {
            patch.insert("htmlSignature".into(), json!(html));
        }
        let v = self.one(
            "Identity/set",
            json!({ "accountId": self.account_id, "update": { id: patch } }),
        )?;
        if let Some(e) = v["notUpdated"].get(id) {
            bail!("The server didn't change the sending address: {}", set_error(e));
        }
        Ok(())
    }
    /// Adds a sending identity for `email` (which the server must allow
    /// this account to send as), returning its id.
    pub fn create_identity(&self, email: &str, name: &str) -> Result<String> {
        ensure!(self.submission, "This JMAP server doesn't send mail.");
        let v = self.one(
            "Identity/set",
            json!({
                "accountId": self.account_id,
                "create": { "i": { "email": email, "name": name } },
            }),
        )?;
        if let Some(e) = v["notCreated"].get("i") {
            if e["type"] == "forbiddenFrom" {
                bail!("The server doesn't let this account send as {email}.");
            }
            bail!("The server didn't add the sending address: {}", set_error(e));
        }
        Ok(v["created"]["i"]["id"]
            .as_str()
            .context("The server didn't say which sending address it added.")?
            .to_owned())
    }
    /// Removes a sending identity.
    pub fn destroy_identity(&self, id: &str) -> Result<()> {
        let v = self.one(
            "Identity/set",
            json!({ "accountId": self.account_id, "destroy": [id] }),
        )?;
        if let Some(e) = v["notDestroyed"].get(id)
            && e["type"] != "notFound"
        {
            bail!("The server didn't remove the sending address: {}", set_error(e));
        }
        Ok(())
    }
    /// The messages of conversations, by thread id: each thread's message
    /// ids, oldest first (Thread/get).
    pub fn threads(&self, ids: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(self.max_get) {
            let v = self.one(
                "Thread/get",
                json!({ "accountId": self.account_id, "ids": chunk }),
            )?;
            for t in v["list"].as_array().into_iter().flatten() {
                let Some(id) = t["id"].as_str() else { continue };
                let emails = t["emailIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect();
                out.insert(id.to_owned(), emails);
            }
        }
        Ok(out)
    }
    /// Which conversations changed since `since` (Thread/changes): their
    /// ids, and the new state; `None` where the server can't say.
    pub fn thread_changes(&self, since: &str) -> Result<Option<ThreadChanges>> {
        let v = match self.one(
            "Thread/changes",
            json!({ "accountId": self.account_id, "sinceState": since, "maxChanges": 500 }),
        ) {
            Ok(v) => v,
            Err(e) if must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        if v["hasMoreChanges"].as_bool().unwrap_or(false) {
            return Ok(None);
        }
        let ids = |key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect()
        };
        let mut changed = ids("created");
        changed.extend(ids("updated"));
        Ok(Some((changed, ids("destroyed"), v["newState"].as_str().unwrap_or_default().to_owned())))
    }
    /// What changed in a query's results since `since_query_state`
    /// (Email/queryChanges): ids removed, and ids added with where. `None`
    /// where the server can't say; the query is then read again.
    pub fn query_changes(
        &self,
        filter: &Value,
        since_query_state: &str,
        up_to: Option<&str>,
    ) -> Result<Option<QueryChanges>> {
        let mut args = json!({
            "accountId": self.account_id,
            "filter": filter,
            "sort": [{ "property": "receivedAt", "isAscending": false }],
            "sinceQueryState": since_query_state,
            "maxChanges": 500,
            "calculateTotal": true,
        });
        if let Some(id) = up_to {
            args["upToId"] = json!(id);
        }
        let v = match self.one("Email/queryChanges", args) {
            Ok(v) => v,
            Err(e) if must_resync(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        let removed = v["removed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect();
        let added = v["added"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some((a["id"].as_str()?.to_owned(), a["index"].as_u64()? as usize))
            })
            .collect();
        Ok(Some(QueryChanges {
            removed,
            added,
            total: v["total"].as_u64().map(|n| n as usize),
            new_state: v["newQueryState"].as_str().unwrap_or_default().to_owned(),
        }))
    }
    /// Copies messages from another account this sign-in reaches (shared
    /// with it) into this one, in `mailbox` (Email/copy, RFC 8621 §4.7).
    /// The originals stay. Returns the new ids, by original id.
    pub fn copy_from(
        &self,
        from_account: &str,
        ids: &[String],
        mailbox: &str,
        keywords: &[&str],
    ) -> Result<HashMap<String, String>> {
        let kw: Map<String, Value> = keywords.iter().map(|k| ((*k).to_owned(), json!(true))).collect();
        let create: Map<String, Value> = ids
            .iter()
            .map(|id| {
                // Without keywords, a copy keeps the original's (RFC 8621
                // §4.7: read, starred and labels stay as they were).
                let mut o = json!({ "id": id, "mailboxIds": { mailbox: true } });
                if !kw.is_empty() {
                    o["keywords"] = Value::Object(kw.clone());
                }
                (id.clone(), o)
            })
            .collect();
        let v = self.one(
            "Email/copy",
            json!({
                "fromAccountId": from_account,
                "accountId": self.account_id,
                "create": create,
            }),
        )?;
        if let Some((id, e)) = v["notCreated"].as_object().and_then(|m| m.iter().next()) {
            bail!("The server didn't copy message {id}: {}", set_error(e));
        }
        Ok(v["created"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, c)| Some((k.clone(), c["id"].as_str()?.to_owned())))
            .collect())
    }
    /// Reads a message kept as a blob (an attached `.eml`) without adding
    /// it to the mailbox (Email/parse, RFC 8621 §4.9): its headers, text
    /// and attachments as for any message.
    pub fn parse_blob(&self, blob: &str) -> Result<Option<Email>> {
        let v = self.one(
            "Email/parse",
            json!({
                "accountId": self.account_id,
                "blobIds": [blob],
                "properties": LIST_PROPERTIES.iter().filter(|p| !["id", "blobId", "threadId", "mailboxIds", "keywords", "receivedAt", "size"].contains(p)).collect::<Vec<_>>(),
            }),
        )?;
        let Some(parsed) = v["parsed"].get(blob) else {
            return Ok(None);
        };
        let mut parsed = parsed.clone();
        // What Email/get always gives, which a parsed blob has not.
        for (k, d) in [
            ("id", json!(blob)),
            ("blobId", json!(blob)),
            ("threadId", json!("")),
            ("mailboxIds", json!({})),
            ("keywords", json!({})),
        ] {
            if parsed.get(k).is_none_or(Value::is_null) {
                parsed[k] = d;
            }
        }
        Ok(Some(serde_json::from_value(parsed).context(
            "The JMAP server's reading of the message isn't in a form Picacho can read.",
        )?))
    }
    /// Copies blobs from another account this sign-in reaches (Blob/copy,
    /// RFC 8620 §6.3), returning the new ids by old id.
    pub fn copy_blobs(&self, from_account: &str, blobs: &[String]) -> Result<HashMap<String, String>> {
        let v = self.one(
            "Blob/copy",
            json!({ "fromAccountId": from_account, "accountId": self.account_id, "blobIds": blobs }),
        )?;
        Ok(v["copied"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, n)| Some((k.clone(), n.as_str()?.to_owned())))
            .collect())
    }
    /// Uploads small data in the API request itself, where the server has
    /// the Blob extension (Blob/upload, RFC 9404), rather than a separate
    /// upload; otherwise as `upload` does. Returns its blob id.
    pub fn upload_inline(&self, bytes: &[u8], kind: &str) -> Result<String> {
        use base64::Engine as _;
        if !self.has(BLOB) || bytes.len() > 256 * 1024 {
            return self.upload(bytes, kind);
        }
        let v = self.one(
            "Blob/upload",
            json!({
                "accountId": self.account_id,
                "create": { "b": {
                    "data": [{ "data:asBase64": base64::engine::general_purpose::STANDARD.encode(bytes) }],
                    "type": kind,
                } },
            }),
        )?;
        if let Some(e) = v["notCreated"].get("b") {
            bail!("The server didn't take the data: {}", set_error(e));
        }
        Ok(v["created"]["b"]["id"]
            .as_str()
            .context("The server didn't say where it kept the data.")?
            .to_owned())
    }
    /// Part of a blob, where the server has the Blob extension (Blob/get
    /// with `offset` and `length`, RFC 9404): the start of a large message
    /// without downloading it all.
    pub fn blob_range(&self, blob: &str, offset: usize, length: usize) -> Result<Option<Vec<u8>>> {
        use base64::Engine as _;
        if !self.has(BLOB) {
            return Ok(None);
        }
        let v = self.one(
            "Blob/get",
            json!({
                "accountId": self.account_id,
                "ids": [blob],
                "properties": ["data:asBase64"],
                "offset": offset,
                "length": length,
            }),
        )?;
        let Some(data) = v["list"][0]["data:asBase64"].as_str() else {
            return Ok(None);
        };
        Ok(Some(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .context("The server's data isn't base64.")?,
        ))
    }
    /// Sends a read receipt for message `id` (MDN/send, RFC 9007), from
    /// `identity`; the server marks the message `$mdnsent`.
    pub fn send_receipt(&self, id: &str, identity: &str, subject: &str, text: &str) -> Result<()> {
        ensure!(self.has(MDN), "This JMAP server can't send read receipts.");
        let v = self.one(
            "MDN/send",
            json!({
                "accountId": self.account_id,
                "identityId": identity,
                "send": { "r": {
                    "forEmailId": id,
                    "subject": subject,
                    "textBody": text,
                    "includeOriginalMessage": false,
                    "disposition": {
                        "actionMode": "manual-action",
                        "sendingMode": "mdn-sent-manually",
                        "type": "displayed",
                    },
                } },
                "onSuccessUpdateEmail": { "#r": { "keywords/$mdnsent": true } },
            }),
        )?;
        if let Some(e) = v["notSent"].get("r") {
            bail!("The server didn't send the read receipt: {}", set_error(e));
        }
        Ok(())
    }
    /// The automatic reply while away (RFC 8621 §8), where the account has
    /// it.
    pub fn vacation(&self) -> Result<Option<VacationResponse>> {
        if !self.vacation {
            return Ok(None);
        }
        let v = self.one(
            "VacationResponse/get",
            json!({ "accountId": self.account_id, "ids": ["singleton"] }),
        )?;
        v["list"]
            .get(0)
            .map(|r| serde_json::from_value(r.clone()))
            .transpose()
            .context("The JMAP server's away reply isn't in a form Picacho can read.")
    }
    /// Sets the automatic reply while away.
    pub fn set_vacation(&self, reply: &VacationResponse) -> Result<()> {
        ensure!(
            self.vacation,
            "This JMAP server doesn't offer an automatic reply while away."
        );
        let v = self.one(
            "VacationResponse/set",
            json!({
                "accountId": self.account_id,
                "update": { "singleton": {
                    "isEnabled": reply.is_enabled,
                    "fromDate": reply.from_date,
                    "toDate": reply.to_date,
                    "subject": reply.subject,
                    "textBody": reply.text_body,
                    // The text is the reply: an HTML one set elsewhere
                    // would be sent instead of it (RFC 8621 §8.1).
                    "htmlBody": null,
                } },
            }),
        )?;
        if let Some(e) = v["notUpdated"].get("singleton") {
            bail!("The server didn't save the away reply: {}", set_error(e));
        }
        Ok(())
    }

    /// The parts of each message that matched a search, with the matches
    /// marked (RFC 8621 §5): message id → its preview, as plain text.
    pub fn search_snippets(&self, filter: &Value, ids: &[String]) -> Result<HashMap<String, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        for chunk in ids.chunks(self.max_get) {
            let v = self.one(
                "SearchSnippet/get",
                json!({ "accountId": self.account_id, "filter": filter, "emailIds": chunk }),
            )?;
            for snippet in v["list"].as_array().into_iter().flatten() {
                if let (Some(id), Some(preview)) =
                    (snippet["emailId"].as_str(), snippet["preview"].as_str())
                {
                    out.insert(id.to_owned(), unmark(preview));
                }
            }
        }
        Ok(out)
    }

    /// Storage used and allowed, in bytes (RFC 9425), where the server
    /// has quotas.
    pub fn quota(&self) -> Result<Option<(u64, u64)>> {
        if !self.quota {
            return Ok(None);
        }
        let v = self.one(
            "Quota/get",
            json!({ "accountId": self.account_id, "ids": null }),
        )?;
        Ok(v["list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|q| q["resourceType"] == "octets")
            .filter(|q| q["scope"] != "global")
            // Mail's quota (RFC 9425 §4.1: `types` names what it counts),
            // not a calendar's or contacts' alone.
            .filter(|q| {
                q["types"].as_array().is_none_or(|t| {
                    t.is_empty() || t.iter().any(|v| v == "Mail" || v == "Email")
                })
            })
            .find_map(|q| Some((q["used"].as_u64()?, q["hardLimit"].as_u64()?))))
    }

    /// Follows the server's pushed changes (an event source) until `stop`
    /// or the connection ends, calling `heard` whenever mail changed.
    pub fn listen(
        &self,
        stop: &std::sync::atomic::AtomicBool,
        heard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        ensure!(self.pushes(), "This JMAP server doesn't push changes.");
        if self.event_source_url.is_empty() {
            return self.listen_websocket(stop, heard);
        }
        let url = self
            .event_source_url
            .replace("{types}", "Email,Mailbox")
            .replace("{closeafter}", "no")
            .replace("{ping}", &PING_SECONDS.to_string());
        let last_id = self
            .last_event_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut request = self
            .signed(self.http.get(&url))
            .header("Accept", "text/event-stream");
        // Where the last stream left off (HTML's event source, which RFC
        // 8620 §7.3 follows), so the server can send what was missed.
        if !last_id.is_empty() {
            request = request.header("Last-Event-ID", last_id);
        }
        let response = request
            // Reconnected now and then, as servers and proxies end long
            // streams anyway.
            .timeout(Duration::from_secs(10 * 60))
            .send()
            .map_err(|e| transport(e, "event stream"))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::Error::new(AccessFailure::Authentication)
                .context("The JMAP server no longer accepts the sign-in."));
        }
        ensure!(
            status.is_success(),
            "The JMAP event stream answered {status}."
        );
        read_events(BufReader::new(response), stop, &self.account_id, &self.last_event_id, heard)
    }
}

/// Reads an event stream (`text/event-stream`) until `stop` or its end,
/// calling `heard` for each state change of `account`.
fn read_events(
    mut reader: impl BufRead,
    stop: &std::sync::atomic::AtomicBool,
    account: &str,
    last_id: &std::sync::Mutex<String>,
    heard: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    use std::io::Read as _;
    use std::sync::atomic::Ordering;
    let mut event = String::new();
    let mut data = String::new();
    // A state change is a few hundred bytes: a stream that sends more
    // than this without ending an event (or a line) is cut off rather
    // than held in memory as it grows.
    const MOST: u64 = 1 << 20;
    let mut line = String::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        line.clear();
        match (&mut reader).take(MOST).read_line(&mut line) {
            // The stream ended (a timeout, or the server closed it).
            Ok(0) | Err(_) => return Ok(()),
            Ok(_) => {}
        }
        if !line.ends_with('\n') || data.len() as u64 > MOST {
            bail!("The JMAP event stream sent more than an event can be.");
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if (event.is_empty() || event == "state") && state_change_for(&data, account) {
                heard()?;
            }
            event.clear();
            data.clear();
        } else if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("id:") {
            // Kept for the next stream's Last-Event-ID.
            let v = v.strip_prefix(' ').unwrap_or(v);
            if !v.contains('\0') && v.len() < 1024 {
                *last_id.lock().unwrap_or_else(|e| e.into_inner()) = v.to_owned();
            }
        } else if let Some(v) = line.strip_prefix("data:") {
            // A field's lines are one value, joined by line feeds.
            if !data.is_empty() {
                data.push('\n');
            }
            // One space after the colon is the field's, not the value's.
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
}

impl Session {
    /// The Sieve extensions the account's filters can use, or `None` when
    /// the server has no JMAP for Sieve Scripts.
    pub fn sieve_extensions(&self) -> Option<&[String]> {
        self.sieve.as_deref()
    }
    fn require_sieve(&self) -> Result<()> {
        ensure!(
            self.sieve.is_some(),
            "This JMAP server doesn't offer server filters (JMAP for Sieve Scripts)."
        );
        Ok(())
    }
    /// The account's Sieve scripts.
    pub fn sieve_scripts(&self) -> Result<Vec<SieveScript>> {
        self.require_sieve()?;
        let v = self.one(
            "SieveScript/get",
            json!({ "accountId": self.account_id, "ids": null }),
        )?;
        serde_json::from_value(v["list"].clone())
            .context("The JMAP server's filter scripts aren't in a form Picacho can read.")
    }
    /// A script's text.
    pub fn sieve_text(&self, script: &SieveScript) -> Result<String> {
        let name = script.name.as_deref().unwrap_or("script");
        let bytes = self.download(&script.blob_id, name, "application/sieve", 1 << 20)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    /// Uploads a script's text, returning its blob id, which validating
    /// and saving then both name (RFC 9661 §2.2, §2.3): it goes up once.
    pub fn sieve_upload(&self, text: &str) -> Result<String> {
        self.require_sieve()?;
        self.upload_inline(text.as_bytes(), "application/sieve")
    }
    /// Checks a script the server would run (uploaded as `blob`), without
    /// saving it.
    pub fn sieve_validate(&self, blob: &str) -> Result<()> {
        self.require_sieve()?;
        let v = self.one(
            "SieveScript/validate",
            json!({ "accountId": self.account_id, "blobId": blob }),
        )?;
        if !v["error"].is_null() {
            bail!(
                "The filter server rejected the filters: {}",
                set_error(&v["error"])
            );
        }
        Ok(())
    }
    /// Saves the script uploaded as `blob` under `name` (replacing
    /// `existing`, its id if it is there) and makes it the active one.
    pub fn sieve_put(&self, name: &str, blob: &str, existing: Option<&str>) -> Result<()> {
        self.require_sieve()?;
        let (args, key, activate) = match existing {
            Some(id) => (
                json!({ "update": { id: { "blobId": blob } } }),
                id.to_owned(),
                id.to_owned(),
            ),
            None => (
                json!({ "create": { "s": { "name": name, "blobId": blob } } }),
                "s".to_owned(),
                "#s".to_owned(),
            ),
        };
        let mut args = args;
        args["accountId"] = json!(self.account_id);
        args["onSuccessActivateScript"] = json!(activate);
        let v = self.one("SieveScript/set", args)?;
        for failed in ["notCreated", "notUpdated"] {
            if let Some(e) = v[failed].get(&key) {
                bail!("The server didn't save the filters: {}", set_error(e));
            }
        }
        Ok(())
    }
    /// Makes the script `id` the active one, or (`None`) none.
    pub fn sieve_activate(&self, id: Option<&str>) -> Result<()> {
        self.require_sieve()?;
        let mut args = json!({ "accountId": self.account_id });
        match id {
            Some(id) => args["onSuccessActivateScript"] = json!(id),
            None => args["onSuccessDeactivateScript"] = json!(true),
        }
        self.one("SieveScript/set", args).map(|_| ())
    }
}

/// Whether an event stream's StateChange says mail of `account` changed.
impl Session {
    /// Follows pushed changes over a WebSocket (RFC 8887), where the server
    /// has no event source: as `listen`.
    fn listen_websocket(
        &self,
        stop: &std::sync::atomic::AtomicBool,
        heard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        use base64::Engine as _;
        use std::sync::atomic::Ordering;
        let authorization = match self.auth {
            Auth::Basic => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", self.login, self.password))
            ),
            Auth::Bearer => format!("Bearer {}", self.password),
        };
        let mut socket = super::websocket::connect(
            &self.websocket_url,
            "jmap",
            &authorization,
            &self.tls,
            Duration::from_secs(u64::from(PING_SECONDS) * 2),
        )?;
        socket.send_text(
            &json!({ "@type": "WebSocketPushEnable", "dataTypes": ["Email", "Mailbox"] })
                .to_string(),
        )?;
        // Reconnected now and then, as servers and proxies end long
        // connections anyway.
        let until = std::time::Instant::now() + Duration::from_secs(10 * 60);
        // Pings unanswered in a row: past a few, the link is taken for dead.
        let mut unanswered = 0;
        while std::time::Instant::now() < until {
            if stop.load(Ordering::Acquire) {
                let _ = socket.close();
                return Ok(());
            }
            match socket.receive()? {
                super::websocket::Incoming::Text(text) => {
                    unanswered = 0;
                    if state_change_for(&text, &self.account_id) {
                        heard()?;
                    }
                }
                super::websocket::Incoming::Pong => unanswered = 0,
                super::websocket::Incoming::Idle => {
                    unanswered += 1;
                    if unanswered > 2 {
                        let _ = socket.close();
                        return Ok(());
                    }
                    socket.ping()?
                }
                super::websocket::Incoming::Closed => return Ok(()),
            }
        }
        let _ = socket.close();
        Ok(())
    }
}

fn state_change_for(data: &str, account: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return false;
    };
    v["changed"][account]
        .as_object()
        .is_some_and(|types| types.contains_key("Email") || types.contains_key("Mailbox"))
}

/// Runs `work` on each item on at most `workers` threads at once, returning
/// the results in the items' order.
fn bounded_parallel<T: Sync, R: Send>(items: &[T], workers: usize, work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let workers = workers.clamp(1, items.len().max(1));
    if workers == 1 {
        return items.iter().map(work).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = work(item);
                    results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .map(|r| r.expect("every item was worked on"))
        .collect()
}

/// Whether a call uses another's result: a back-reference (`#ids`), or a
/// creation id another call made (`"#m"` as a value, RFC 8620 §5.3).
fn uses_results(args: &Value) -> bool {
    fn refers(v: &Value) -> bool {
        match v {
            Value::String(s) => s.starts_with('#') && s.len() > 1,
            Value::Array(a) => a.iter().any(refers),
            Value::Object(m) => m.iter().any(|(k, v)| k.starts_with('#') || refers(v)),
            _ => false,
        }
    }
    refers(args)
}

/// A search snippet as plain text: its `<mark>` tags gone and its
/// entities read (RFC 8621 §5.1 escapes the rest).
fn unmark(html: &str) -> String {
    html.replace("<mark>", "")
        .replace("</mark>", "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// How a message is submitted, beyond its envelope.
#[derive(Clone, Debug, Default)]
pub struct SendOptions {
    /// Every server on the way must use TLS (REQUIRETLS): refused where
    /// the server can't ask for it.
    pub require_tls: bool,
    /// Ask for delivery reports (DSN), identified by this envelope id,
    /// where the server can.
    pub receipts: Option<String>,
    /// Held at the server and sent then (FUTURERELEASE `HOLDUNTIL`), a
    /// Unix time.
    pub hold_until: Option<i64>,
}

/// A submission the server took.
#[derive(Clone, Debug, Default)]
pub struct Submitted {
    /// Delivery reports were asked for.
    pub receipts: bool,
    /// The submission's id, to follow it (`submission_status`) or, while
    /// the server still holds it, cancel it (`cancel_submission`).
    pub id: String,
}

/// Where a submission is (RFC 8621 §7): whether it can still be taken
/// back, and what became of each recipient.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmissionStatus {
    /// `pending` (held, and can be cancelled), `final` or `canceled`.
    pub undo: String,
    /// Each recipient's delivery: (address, `queued`/`yes`/`no`/`unknown`,
    /// the server's last SMTP reply).
    pub delivery: Vec<(String, String, String)>,
}

/// A sending identity (RFC 8621 §6) as it can be changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityChange {
    pub name: Option<String>,
    pub reply_to: Option<Vec<(String, String)>>,
    pub text_signature: Option<String>,
    pub html_signature: Option<String>,
}

/// Conversations changed and gone since a state, and the new state.
pub type ThreadChanges = (Vec<String>, Vec<String>, String);

/// What changed in a query's results (Email/queryChanges).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryChanges {
    pub removed: Vec<String>,
    /// Added (or moved) ids, with their place in the results.
    pub added: Vec<(String, usize)>,
    pub total: Option<usize>,
    pub new_state: String,
}

/// The automatic reply while away (RFC 8621 §8.1).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VacationResponse {
    #[serde(default)]
    pub is_enabled: bool,
    /// UTC date-times (`2026-11-20T00:00:00Z`) it answers from and until.
    #[serde(default)]
    pub from_date: Option<String>,
    #[serde(default)]
    pub to_date: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub text_body: Option<String>,
}

fn import_args(blob: &str, mailboxes: &[&str], keywords: &[&str]) -> Value {
    let mailbox_ids: Map<String, Value> = mailboxes
        .iter()
        .map(|m| ((*m).to_owned(), Value::Bool(true)))
        .collect();
    let keywords: Map<String, Value> = keywords
        .iter()
        .map(|k| ((*k).to_owned(), Value::Bool(true)))
        .collect();
    json!({ "blobId": blob, "mailboxIds": mailbox_ids, "keywords": keywords })
}
/// The id of an imported message; one the server had already counts.
fn imported(v: &Value, key: &str) -> Result<String> {
    if let Some(e) = v["notCreated"].get(key) {
        if e["type"] == "alreadyExists"
            && let Some(id) = e["existingId"].as_str()
        {
            return Ok(id.to_owned());
        }
        bail!("The server didn't take the message: {}", set_error(e));
    }
    Ok(v["created"][key]["id"]
        .as_str()
        .context("The server didn't say where the message went.")?
        .to_owned())
}
/// A SetError's type and description.
fn set_error(e: &Value) -> String {
    let kind = e["type"].as_str().unwrap_or("refused");
    match e["description"].as_str() {
        Some(d) if !d.is_empty() => format!("{d} ({kind})"),
        _ => kind.to_owned(),
    }
}

/// What the server answered one method call (RFC 8620 §3.4): the
/// responses carrying its call id, or the method's error.
#[derive(Debug)]
pub struct Answer {
    method: String,
    /// Its own response, and any it implied with the same call id (the
    /// Email/set of an `onSuccessUpdateEmail`, RFC 8621 §7.5).
    responses: Vec<(String, Value)>,
    error: Option<MethodError>,
}
impl Answer {
    /// The call's own response arguments, or its error.
    pub fn value(&self) -> Result<&Value> {
        if let Some(e) = &self.error {
            return Err(e.to_error());
        }
        self.responses
            .iter()
            .find(|(n, _)| *n == self.method)
            .or(self.responses.first())
            .map(|(_, v)| v)
            .with_context(|| format!("The JMAP server didn't answer {}.", self.method))
    }
    pub fn into_value(self) -> Result<Value> {
        self.value().cloned()
    }
}
/// Each call's answer, in the calls' order, from a reply's method responses:
/// matched by call id (RFC 8620 §3.4), as a server may answer in another
/// order or add implied responses with the same id.
fn answers(calls: &[(&str, Value)], responses: &[Value]) -> Vec<Answer> {
    let mut out: Vec<Answer> = calls
        .iter()
        .map(|(name, _)| Answer {
            method: (*name).to_owned(),
            responses: Vec::new(),
            error: None,
        })
        .collect();
    for r in responses {
        let Some(i) = r[2]
            .as_str()
            .and_then(|id| id.strip_prefix('c'))
            .and_then(|i| i.parse::<usize>().ok())
            .filter(|i| r[2].as_str() == Some(format!("c{i}").as_str()))
        else {
            continue;
        };
        let Some(answer) = out.get_mut(i) else { continue };
        let name = r[0].as_str().unwrap_or_default();
        if name == "error" {
            if answer.error.is_none() {
                answer.error = Some(MethodError::from_response(answer.method.clone(), &r[1]));
            }
        } else {
            answer.responses.push((name.to_owned(), r[1].clone()));
        }
    }
    out
}

/// RFC 8620 §5.2: the server can't say what changed since a state (it is
/// too old, or the server keeps no such history). Everything is read
/// again rather than reported as a failure.
#[derive(Debug)]
pub struct CannotCalculateChanges {
    /// The method that couldn't (`Email/changes`…).
    pub method: String,
}
impl std::fmt::Display for CannotCalculateChanges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The JMAP server can't say what changed ({}), so everything is read again.", self.method)
    }
}
impl std::error::Error for CannotCalculateChanges {}

/// Whether a /changes or /queryChanges call failed in a way a full read
/// recovers from: `cannotCalculateChanges` (RFC 8620 §5.2, §5.6), more
/// changes than asked for (`tooManyChanges`, §5.6), or a server without
/// the method at all (`unknownMethod`, §3.6.2). Other refusals are errors.
pub fn must_resync(e: &anyhow::Error) -> bool {
    e.downcast_ref::<CannotCalculateChanges>().is_some()
        || e.downcast_ref::<MethodError>()
            .is_some_and(|m| m.is("tooManyChanges") || m.is("unknownMethod"))
}

/// A method the server refused (RFC 8620 §3.6.2).
#[derive(Clone, Debug)]
pub struct MethodError {
    /// The method refused (`Email/query`…); empty when unknown.
    pub method: String,
    pub kind: String,
    pub description: String,
    /// The arguments it objected to, where the server says (Cyrus, and so
    /// Fastmail, lists them: `properties[3]`).
    pub arguments: Vec<String>,
}
impl MethodError {
    fn from_response(method: String, args: &Value) -> Self {
        Self {
            method,
            kind: args["type"].as_str().unwrap_or("serverFail").to_owned(),
            description: args["description"].as_str().unwrap_or_default().to_owned(),
            arguments: args["arguments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect(),
        }
    }
    /// Whether it is the error `kind`; servers differ in its case.
    pub fn is(&self, kind: &str) -> bool {
        self.kind.eq_ignore_ascii_case(kind)
    }
    /// As an error: `cannotCalculateChanges` is its own kind of error
    /// (`CannotCalculateChanges`), which callers answer with a full read.
    fn to_error(&self) -> anyhow::Error {
        if self.is("cannotCalculateChanges") {
            anyhow::Error::new(CannotCalculateChanges { method: self.method.clone() })
        } else {
            anyhow::Error::new(self.clone())
        }
    }
}
impl std::fmt::Display for MethodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The JMAP server refused ")?;
        if self.method.is_empty() {
            f.write_str("the request")?;
        } else {
            write!(f, "{}", self.method)?;
        }
        if !self.description.is_empty() {
            write!(f, ": {}", self.description)?;
        }
        write!(f, " ({}", self.kind)?;
        if !self.arguments.is_empty() {
            write!(f, ": {}", self.arguments.join(", "))?;
        }
        f.write_str(")")
    }
}
impl std::error::Error for MethodError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Events are read as they come; a stream that never ends one is cut
    /// off instead of growing in memory.
    #[test]
    fn event_streams_are_read_and_bounded() {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let change = r#"{"@type":"StateChange","changed":{"A1":{"Email":"s2"}}}"#;
        let stream = format!(": ping\r\n\r\nevent: state\r\ndata: {change}\r\n\r\ndata: {change}\n\n");
        let mut heard = 0;
        read_events(stream.as_bytes(), &stop, "A1", &Default::default(), &mut || {
            heard += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(heard, 2);
        let endless = "data: x\n".repeat(600_000);
        assert!(read_events(endless.as_bytes(), &stop, "A1", &Default::default(), &mut || Ok(())).is_err());
        let long_line = format!("data: {}", "x".repeat(2 << 20));
        assert!(read_events(long_line.as_bytes(), &stop, "A1", &Default::default(), &mut || Ok(())).is_err());
    }

    /// Work runs on no more threads than asked, and results keep the
    /// items' order.
    #[test]
    fn bounded_parallel_keeps_order_and_bound() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let items: Vec<usize> = (0..40).collect();
        let out = bounded_parallel(&items, 3, |n| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(2));
            running.fetch_sub(1, Ordering::SeqCst);
            n * 2
        });
        assert_eq!(out, items.iter().map(|n| n * 2).collect::<Vec<_>>());
        assert!(most.load(Ordering::SeqCst) <= 3);
        assert!(bounded_parallel(&[] as &[usize], 4, |n| *n).is_empty());
    }

    /// Responses are matched to calls by call id, not by order or name; an
    /// error answers its own call alone, and cannotCalculateChanges is its
    /// own error, which asks for a full read (RFC 8620 §3.4, §5.2).
    #[test]
    fn answers_match_calls_by_id_and_keep_each_result() {
        let calls = [
            ("Email/import", json!({})),
            ("EmailSubmission/set", json!({})),
            ("Email/changes", json!({})),
        ];
        let responses = [
            json!(["error", { "type": "cannotCalculateChanges" }, "c2"]),
            json!(["EmailSubmission/set", { "created": { "s": { "id": "S1" } } }, "c1"]),
            json!(["Email/set", { "updated": { "M1": null } }, "c1"]),
            json!(["Email/import", { "created": { "m": { "id": "M1" } } }, "c0"]),
            json!(["Email/get", { "list": [] }, "c9"]),
            json!(["Email/get", { "list": [] }, "c01"]),
        ];
        let out = answers(&calls, &responses);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].value().unwrap()["created"]["m"]["id"], "M1");
        assert_eq!(out[1].value().unwrap()["created"]["s"]["id"], "S1");
        let e = out[2].value().unwrap_err();
        assert!(e.downcast_ref::<CannotCalculateChanges>().is_some(), "{e:#}");
        assert!(must_resync(&e));
        // Other refusals stay MethodErrors, naming their method.
        let out = answers(
            &calls[..1],
            &[json!(["error", { "type": "invalidArguments" }, "c0"])],
        );
        let e = out[0].value().unwrap_err();
        assert!(!must_resync(&e));
        assert!(e.to_string().contains("refused Email/import"), "{e}");
        // A call the server didn't answer.
        let out = answers(&calls[..1], &[]);
        assert!(out[0].value().is_err());
    }

    #[test]
    fn session_urls() {
        assert_eq!(
            session_url("api.fastmail.com", 443),
            "https://api.fastmail.com/.well-known/jmap"
        );
        assert_eq!(
            session_url("mail.example.com", 8443),
            "https://mail.example.com:8443/.well-known/jmap"
        );
    }

    #[test]
    fn identities_cover_their_addresses() {
        let one = Identity {
            id: "1".into(),
            email: "Ana@Example.com".into(),
            ..Default::default()
        };
        assert!(one.covers("ana@example.com"));
        assert!(!one.covers("bo@example.com"));
        let domain = Identity {
            email: "*@example.com".into(),
            ..one
        };
        assert!(domain.covers("bo@EXAMPLE.com"));
        assert!(!domain.covers("bo@example.org"));
    }

    #[test]
    fn refusals_name_the_method_and_arguments() {
        let e = MethodError::from_response(
            "Email/get".into(),
            &json!({ "type": "invalidArguments", "arguments": ["properties[19]"] }),
        );
        assert_eq!(
            e.to_string(),
            "The JMAP server refused Email/get (invalidArguments: properties[19])"
        );
        assert!(e.is("InvalidArguments"));
        let e = MethodError::from_response(
            String::new(),
            &json!({ "type": "forbidden", "description": "No" }),
        );
        assert_eq!(
            e.to_string(),
            "The JMAP server refused the request: No (forbidden)"
        );
    }

    #[test]
    fn state_changes_name_the_account() {
        let data = r#"{"@type":"StateChange","changed":{"u1":{"Email":"s2"}}}"#;
        assert!(state_change_for(data, "u1"));
        assert!(!state_change_for(data, "u2"));
        assert!(!state_change_for(
            r#"{"changed":{"u1":{"Calendar":"x"}}}"#,
            "u1"
        ));
        assert!(!state_change_for("not json", "u1"));
    }

    #[test]
    fn emails_read_their_listing() {
        let e: Email = serde_json::from_value(json!({
            "id": "M1",
            "mailboxIds": { "inbox": true },
            "keywords": { "$Seen": true, "work": true },
            "receivedAt": "2026-10-24T10:00:00Z",
            "from": [{ "name": "Ana", "email": "ana@example.com" }],
            "header:List-Unsubscribe:asURLs": ["mailto:u@x", "https://x/u"],
        }))
        .unwrap();
        assert!(e.has_keyword("$seen"));
        assert!(e.in_mailbox("inbox"));
        assert_eq!(e.time(), 1_792_836_000);
        assert_eq!(addresses(&e.from), "Ana <ana@example.com>");
        assert_eq!(e.unsubscribe(), "<mailto:u@x>, <https://x/u>");
        assert_eq!(utc_date(1_792_836_000), "2026-10-24T10:00:00Z");
    }
}
