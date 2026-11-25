//! Exchange Web Services (EWS), for Exchange Server in an organization's
//! own data center (2010 SP2 and later): SOAP over HTTPS at the server's
//! `/EWS/Exchange.asmx`, found by Autodiscover from the address.
//!
//! Signing in is what IIS offers Exchange: NTLM (NTLMv2 only, `ntlm.rs`),
//! Kerberos (HTTP Negotiate, with the system's ticket, `gss.rs`), Basic
//! over TLS, or an access token from Microsoft Entra (hybrid modern
//! authentication). NTLM and Negotiate authenticate a connection, so each
//! request goes through its handshake on one kept connection, one request
//! at a time; both are bound to the TLS channel and the service (Extended
//! Protection for Authentication).
//!
//! Messages are read and sent as MIME (`MimeContent`); calendar items as
//! iCalendar and contacts as vCard, which Exchange gives as their MIME
//! content, so they are read as CalDAV's and CardDAV's are. Flags are the
//! MAPI properties Outlook uses (PR_FLAG_STATUS, PR_LAST_VERB_EXECUTED),
//! which Exchange 2010 already has.
//!
//! These helpers only talk to the server; Manage keeps the result
//! (`manage/ews.rs`).
use crate::manage::status::AccessFailure;
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use roxmltree::Node;
use std::time::Duration;

pub mod ntlm;

const SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const TYPES: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
const MESSAGES: &str = "http://schemas.microsoft.com/exchange/services/2006/messages";
/// The oldest Exchange whose answers Picacho reads.
const VERSION: &str = "Exchange2010_SP2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);
/// The most of an answer read into memory.
const MOST: u64 = 160 * 1024 * 1024;
/// Items asked about in one request.
const BATCH: usize = 100;
/// What the message list shows of a message.
const ITEM_PROPERTIES: &str = r#"<t:FieldURI FieldURI="item:Subject"/><t:FieldURI FieldURI="message:From"/><t:FieldURI FieldURI="item:DateTimeReceived"/><t:FieldURI FieldURI="message:IsRead"/><t:FieldURI FieldURI="item:Size"/><t:FieldURI FieldURI="item:HasAttachments"/><t:FieldURI FieldURI="message:InternetMessageId"/><t:FieldURI FieldURI="item:Categories"/><t:FieldURI FieldURI="item:DisplayTo"/><t:FieldURI FieldURI="item:DisplayCc"/><t:FieldURI FieldURI="item:ParentFolderId"/><t:FieldURI FieldURI="item:ItemClass"/><t:FieldURI FieldURI="item:ConversationId"/><t:ExtendedFieldURI PropertyTag="0x1090" PropertyType="Integer"/><t:ExtendedFieldURI PropertyTag="0x1081" PropertyType="Integer"/>"#;

/// How the server is signed in to.
#[derive(Clone)]
pub enum Auth {
    /// NTLMv2 (most Exchange servers' default).
    Ntlm(ntlm::Identity),
    /// Kerberos, with the ticket this computer already has.
    Kerberos,
    /// The user name and password, over TLS.
    Basic { user: String, password: String },
    /// An access token from the organization's sign-in (Microsoft Entra,
    /// for Exchange with hybrid modern authentication).
    Bearer(String),
}
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ntlm(_) => "Ntlm",
            Self::Kerberos => "Kerberos",
            Self::Basic { .. } => "Basic",
            Self::Bearer(_) => "Bearer",
        })
    }
}

/// Characters escaped for XML text and attributes.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
fn unb64(text: &str) -> Result<Vec<u8>> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    Ok(base64::engine::general_purpose::STANDARD.decode(clean)?)
}

// --- XML reading ------------------------------------------------------------

fn named<'a>(n: Node<'a, 'a>, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}
/// The first child element called `name`.
pub fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| named(*c, name))
}
/// Every child element called `name`.
pub fn children<'a>(n: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> + 'a {
    n.children().filter(move |c| named(*c, name))
}
/// The first element called `name` anywhere under `n`.
pub fn find<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.descendants().find(|c| named(*c, name))
}
/// The text of `n`'s child `name`, or empty.
pub fn text(n: Node, name: &str) -> String {
    n.children().find(|c| named(*c, name)).and_then(|c| c.text()).unwrap_or_default().to_owned()
}
fn time(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s.trim()).map_or(0, |d| d.timestamp())
}
/// Unix seconds as EWS writes a time (UTC).
pub fn ews_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// An item's id and its change key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemId {
    pub id: String,
    pub change_key: String,
}
impl ItemId {
    fn of(n: Node) -> Self {
        child(n, "ItemId")
            .map(|i| ItemId {
                id: i.attribute("Id").unwrap_or_default().to_owned(),
                change_key: i.attribute("ChangeKey").unwrap_or_default().to_owned(),
            })
            .unwrap_or_default()
    }
}
fn item_ids(ids: &[String]) -> String {
    ids.iter().map(|id| format!(r#"<t:ItemId Id="{}"/>"#, esc(id))).collect()
}
/// A folder: by id, or by its distinguished name (`inbox`, `sentitems`…).
pub fn folder_id(folder: &str) -> String {
    if DISTINGUISHED.contains(&folder) {
        format!(r#"<t:DistinguishedFolderId Id="{folder}"/>"#)
    } else {
        format!(r#"<t:FolderId Id="{}"/>"#, esc(folder))
    }
}
/// The distinguished folders Picacho names.
pub const DISTINGUISHED: &[&str] = &[
    "msgfolderroot",
    "inbox",
    "sentitems",
    "drafts",
    "deleteditems",
    "junkemail",
    "outbox",
    "calendar",
    "contacts",
    "tasks",
    "notes",
    "journal",
];

/// A message, as the message list shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub id: String,
    pub folder_id: String,
    pub subject: String,
    pub from_name: String,
    pub from_email: String,
    pub display_to: String,
    pub display_cc: String,
    pub received: i64,
    pub read: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub has_attachments: bool,
    pub message_id: String,
    pub conversation_id: String,
    pub size: u64,
    /// PR_LAST_VERB_EXECUTED: 102 replied, 103 replied to all, 104 forwarded.
    pub last_verb: Option<i64>,
}
impl Message {
    fn of(n: Node) -> Self {
        let mut m = Message {
            id: ItemId::of(n).id,
            folder_id: child(n, "ParentFolderId").and_then(|f| f.attribute("Id")).unwrap_or_default().to_owned(),
            subject: text(n, "Subject"),
            display_to: text(n, "DisplayTo"),
            display_cc: text(n, "DisplayCc"),
            received: time(&text(n, "DateTimeReceived")),
            read: text(n, "IsRead") == "true",
            has_attachments: text(n, "HasAttachments") == "true",
            message_id: text(n, "InternetMessageId"),
            conversation_id: child(n, "ConversationId").and_then(|c| c.attribute("Id")).unwrap_or_default().to_owned(),
            size: text(n, "Size").parse().unwrap_or_default(),
            categories: child(n, "Categories")
                .map(|c| children(c, "String").filter_map(|s| s.text()).map(str::to_owned).collect())
                .unwrap_or_default(),
            ..Default::default()
        };
        if let Some(mailbox) = child(n, "From").and_then(|f| child(f, "Mailbox")) {
            m.from_name = text(mailbox, "Name");
            m.from_email = text(mailbox, "EmailAddress");
        }
        for p in children(n, "ExtendedProperty") {
            let tag = child(p, "ExtendedFieldURI").and_then(|u| u.attribute("PropertyTag")).and_then(property_tag);
            let value: Option<i64> = text(p, "Value").parse().ok();
            match tag {
                Some(0x1090) => m.flagged = value == Some(2),
                Some(0x1081) => m.last_verb = value,
                _ => {}
            }
        }
        m
    }
}

/// A mail folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailFolder {
    pub id: String,
    pub name: String,
    pub parent_id: String,
    /// Its path from the top of the mailbox, with `/` between names.
    pub path: String,
    pub depth: usize,
    /// Its distinguished name (`inbox`, `sentitems`…; `archive` for the
    /// folder Outlook archives to), for standard ones.
    pub distinguished: Option<String>,
    pub total: u64,
    /// IPF.Note and the like; calendars and contacts aren't mail.
    pub class: String,
}

/// An inbox rule, in the shape Microsoft Graph gives its `messageRule`
/// (`conditions`, `exceptions`, `actions`), which EWS's rules mirror: so
/// Picacho's filters are read and written as for Microsoft 365
/// (`manage/api_filters.rs`). Tests and actions Graph has no name for are
/// kept under their EWS names, so such a rule shows as one Picacho can't say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub id: String,
    pub json: serde_json::Value,
}

/// EWS rule tests by their Graph names; values are strings (`Str`),
/// addresses (`Addr`), or the size range.
const RULE_TESTS: &[(&str, &str)] = &[
    ("ContainsBodyStrings", "bodyContains"),
    ("ContainsHeaderStrings", "headerContains"),
    ("ContainsRecipientStrings", "recipientContains"),
    ("ContainsSenderStrings", "senderContains"),
    ("ContainsSubjectStrings", "subjectContains"),
    ("FromAddresses", "fromAddresses"),
    ("SentToAddresses", "sentToAddresses"),
    ("WithinSizeRange", "withinSizeRange"),
];
/// EWS rule actions by their Graph names, in the schema's order.
const RULE_ACTIONS: &[(&str, &str)] = &[
    ("AssignCategories", "assignCategories"),
    ("Delete", "delete"),
    ("MarkAsRead", "markAsRead"),
    ("MoveToFolder", "moveToFolder"),
    ("RedirectToRecipients", "redirectTo"),
    ("StopProcessingRules", "stopProcessingRules"),
];
fn strings_of(n: Node) -> serde_json::Value {
    serde_json::Value::Array(children(n, "String").filter_map(|s| s.text()).map(|s| s.into()).collect())
}
fn addresses_of(n: Node) -> serde_json::Value {
    serde_json::Value::Array(
        children(n, "Address")
            .map(|a| serde_json::json!({"emailAddress": {"address": text(a, "EmailAddress"), "name": text(a, "Name")}}))
            .collect(),
    )
}
/// Tests (`Conditions` or `Exceptions`) as Graph names them.
fn tests_json(n: Option<Node>) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for test in n.into_iter().flat_map(|n| n.children().filter(|c| c.is_element())) {
        let ews = test.tag_name().name();
        let value = match ews {
            "FromAddresses" | "SentToAddresses" => addresses_of(test),
            "WithinSizeRange" => serde_json::json!({
                "minimumSize": text(test, "MinimumSize").parse::<i64>().unwrap_or(0),
                "maximumSize": text(test, "MaximumSize").parse::<i64>().unwrap_or(i32::MAX as i64),
            }),
            _ => strings_of(test),
        };
        let name = RULE_TESTS.iter().find(|(e, _)| *e == ews).map_or(ews, |(_, g)| g);
        out.insert(name.to_owned(), value);
    }
    serde_json::Value::Object(out)
}
fn rule_json(r: Node) -> serde_json::Value {
    let mut actions = serde_json::Map::new();
    for a in child(r, "Actions").into_iter().flat_map(|n| n.children().filter(|c| c.is_element())) {
        let ews = a.tag_name().name();
        let value = match ews {
            "MoveToFolder" | "CopyToFolder" => find(a, "FolderId").and_then(|f| f.attribute("Id")).unwrap_or_default().into(),
            "RedirectToRecipients" | "ForwardToRecipients" | "ForwardAsAttachmentToRecipients" => addresses_of(a),
            "AssignCategories" => strings_of(a),
            _ => (a.text().unwrap_or_default() == "true").into(),
        };
        let name = RULE_ACTIONS.iter().find(|(e, _)| *e == ews).map_or(ews, |(_, g)| g);
        actions.insert(name.to_owned(), value);
    }
    serde_json::json!({
        "displayName": text(r, "DisplayName"),
        "sequence": text(r, "Priority").parse::<i64>().unwrap_or(i64::MAX),
        "isEnabled": text(r, "IsEnabled") != "false",
        "conditions": tests_json(child(r, "Conditions")),
        "exceptions": tests_json(child(r, "Exceptions")),
        "actions": actions,
    })
}
/// A rule in Graph's shape as EWS's `t:Rule` (with `id` to change one).
pub fn rule_xml(rule: &serde_json::Value, id: Option<&str>) -> String {
    let strings = |v: &serde_json::Value| -> String {
        v.as_array().into_iter().flatten().filter_map(|s| s.as_str()).map(|s| format!("<t:String>{}</t:String>", esc(s))).collect()
    };
    let addresses = |v: &serde_json::Value| -> String {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|a| format!("<t:Address><t:EmailAddress>{}</t:EmailAddress></t:Address>", esc(a["emailAddress"]["address"].as_str().unwrap_or_default())))
            .collect()
    };
    let tests = |v: &serde_json::Value| -> String {
        RULE_TESTS
            .iter()
            .filter_map(|(ews, graph)| {
                let value = v.get(*graph)?;
                Some(match *ews {
                    "FromAddresses" | "SentToAddresses" => format!("<t:{ews}>{}</t:{ews}>", addresses(value)),
                    "WithinSizeRange" => format!(
                        "<t:WithinSizeRange><t:MinimumSize>{}</t:MinimumSize><t:MaximumSize>{}</t:MaximumSize></t:WithinSizeRange>",
                        value["minimumSize"].as_i64().unwrap_or(0),
                        value["maximumSize"].as_i64().unwrap_or(i32::MAX as i64)
                    ),
                    _ => format!("<t:{ews}>{}</t:{ews}>", strings(value)),
                })
            })
            .collect()
    };
    let actions: String = RULE_ACTIONS
        .iter()
        .filter_map(|(ews, graph)| {
            let value = rule["actions"].get(*graph)?;
            Some(match *ews {
                "MoveToFolder" => format!(r#"<t:MoveToFolder><t:FolderId Id="{}"/></t:MoveToFolder>"#, esc(value.as_str().unwrap_or_default())),
                "RedirectToRecipients" => format!("<t:RedirectToRecipients>{}</t:RedirectToRecipients>", addresses(value)),
                "AssignCategories" => format!("<t:AssignCategories>{}</t:AssignCategories>", strings(value)),
                _ => format!("<t:{ews}>{}</t:{ews}>", value.as_bool().unwrap_or(false)),
            })
        })
        .collect();
    let id = id.map(|id| format!("<t:RuleId>{}</t:RuleId>", esc(id))).unwrap_or_default();
    let exceptions = tests(&rule["exceptions"]);
    format!(
        "<t:Rule>{id}<t:DisplayName>{}</t:DisplayName><t:Priority>{}</t:Priority><t:IsEnabled>true</t:IsEnabled><t:Conditions>{}</t:Conditions>{}<t:Actions>{actions}</t:Actions></t:Rule>",
        esc(rule["displayName"].as_str().unwrap_or("Picacho")),
        rule["sequence"].as_i64().unwrap_or(1),
        tests(&rule["conditions"]),
        if exceptions.is_empty() { String::new() } else { format!("<t:Exceptions>{exceptions}</t:Exceptions>") },
    )
}

/// A change to an item's properties (`UpdateItem`).
#[derive(Clone, Debug)]
enum Update {
    Read(bool),
    /// A MAPI integer property (tag, value).
    Integer(&'static str, i64),
    /// The categories; none removes them.
    Categories(Vec<String>),
}
/// The kinds of item that are messages (and so can be read or not).
const MESSAGE_KINDS: &[&str] = &["Message", "MeetingMessage", "MeetingRequest", "MeetingResponse", "MeetingCancellation"];
impl Update {
    /// The `t:SetItemField`/`t:DeleteItemField` for an item of `kind`;
    /// `None` where that kind hasn't the property.
    fn xml(&self, kind: &str) -> Option<String> {
        // Item kinds are names from Exchange's schema: plain words.
        if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        Some(match self {
            Self::Read(read) => {
                if !MESSAGE_KINDS.contains(&kind) {
                    return None;
                }
                format!(r#"<t:SetItemField><t:FieldURI FieldURI="message:IsRead"/><t:{kind}><t:IsRead>{read}</t:IsRead></t:{kind}></t:SetItemField>"#)
            }
            Self::Integer(tag, value) => format!(
                r#"<t:SetItemField><t:ExtendedFieldURI PropertyTag="{tag}" PropertyType="Integer"/><t:{kind}><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="{tag}" PropertyType="Integer"/><t:Value>{value}</t:Value></t:ExtendedProperty></t:{kind}></t:SetItemField>"#
            ),
            Self::Categories(cats) if cats.is_empty() => r#"<t:DeleteItemField><t:FieldURI FieldURI="item:Categories"/></t:DeleteItemField>"#.to_owned(),
            Self::Categories(cats) => {
                let strings: String = cats.iter().map(|c| format!("<t:String>{}</t:String>", esc(c))).collect();
                format!(r#"<t:SetItemField><t:FieldURI FieldURI="item:Categories"/><t:{kind}><t:Categories>{strings}</t:Categories></t:{kind}></t:SetItemField>"#)
            }
        })
    }
}

/// Someone the directory knows: name, address, title, company, phones.
pub type Person = (String, String, String, String, Vec<String>);

/// The automatic reply while away (Out of Office).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Oof {
    pub enabled: bool,
    pub internal: String,
    pub external: String,
    /// Unix seconds; both 0 when not scheduled.
    pub start: i64,
    pub end: i64,
}

/// A signed-in EWS connection.
pub struct Session {
    http: reqwest::blocking::Client,
    /// `https://mail.example.com/EWS/Exchange.asmx`.
    pub url: String,
    auth: Auth,
    /// The mailbox's address (for Out of Office).
    pub email: String,
    folders: std::sync::Mutex<Vec<MailFolder>>,
    /// The TLS channel's binding (RFC 5929 tls-server-end-point), once
    /// the server's certificate was seen; sign-ins are bound to it.
    binding: Binding,
    /// One request at a time: NTLM's two legs must go over the one kept
    /// connection, which a request from another thread could take between
    /// them.
    one_at_a_time: std::sync::Mutex<()>,
    /// The kind of item (`Message`, `MeetingRequest`, `CalendarItem`…) each
    /// id was last seen as: changes name it, as Exchange's schema wants.
    kinds: std::sync::Mutex<std::collections::HashMap<String, String>>,
    tls: native_tls::TlsConnector,
}

/// The binding data of the server's TLS channel, where it is known.
type Binding = std::sync::Mutex<Option<Vec<u8>>>;

/// The HTTP client for a server: one kept connection (NTLM and Negotiate
/// authenticate a connection), no redirects, the server's certificate
/// kept with each answer (for channel binding).
fn client(tls: native_tls::TlsConnector) -> Result<reqwest::blocking::Client> {
    client_timing(tls, REQUEST_TIMEOUT)
}
fn client_timing(tls: native_tls::TlsConnector, timeout: Duration) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(12))
        .pool_max_idle_per_host(1)
        .redirect(reqwest::redirect::Policy::none())
        .tls_info(true)
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .https_only(!cfg!(test))
        .build()?)
}

/// RFC 5929's tls-server-end-point: the server certificate's hash, by
/// the hash its signature uses (SHA-256 for MD5 and SHA-1).
pub fn tls_server_end_point(der: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    match signature_hash(der) {
        384 => sha2::Sha384::digest(der).to_vec(),
        512 => sha2::Sha512::digest(der).to_vec(),
        _ => sha2::Sha256::digest(der).to_vec(),
    }
}
/// One DER element: its tag, its content, and what follows it.
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *b.first()?;
    let first = *b.get(1)? as usize;
    let (len, head) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | *b.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let content = b.get(head..head + len)?;
    Some((tag, content, &b[head + len..]))
}
/// The bits of the hash a certificate's signature algorithm uses: 384 or
/// 512 where it says so, else 256 (RFC 5929 §4.1 takes SHA-256 for
/// anything weaker).
fn signature_hash(cert: &[u8]) -> u32 {
    const SHA384_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
    const SHA512_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
    const SHA384_ECDSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
    const SHA512_ECDSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
    const RSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
    const SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
    const SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
    let algorithm = || -> Option<(&[u8], &[u8])> {
        let (_, certificate, _) = der(cert)?;
        let (_, _tbs, rest) = der(certificate)?;
        let (_, algorithm, _) = der(rest)?;
        let (tag, oid, params) = der(algorithm)?;
        (tag == 0x06).then_some((oid, params))
    };
    let Some((oid, params)) = algorithm() else { return 256 };
    let contains = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).any(|w| w == needle);
    match oid {
        o if o == SHA384_RSA || o == SHA384_ECDSA => 384,
        o if o == SHA512_RSA || o == SHA512_ECDSA => 512,
        o if o == RSA_PSS && contains(params, SHA384) => 384,
        o if o == RSA_PSS && contains(params, SHA512) => 512,
        _ => 256,
    }
}
/// The channel binding application data (`tls-server-end-point:` and
/// the certificate's hash) of the connection an answer came over.
fn binding_of(response: &reqwest::blocking::Response) -> Option<Vec<u8>> {
    let cert = response.extensions().get::<reqwest::tls::TlsInfo>()?.peer_certificate()?;
    Some([b"tls-server-end-point:".as_slice(), &tls_server_end_point(cert)].concat())
}

/// Reads an answer: its status and bytes.
fn read(r: reqwest::blocking::Response) -> Result<(u16, Vec<u8>)> {
    use std::io::Read as _;
    let status = r.status().as_u16();
    let mut out = Vec::new();
    r.take(MOST + 1).read_to_end(&mut out)?;
    ensure!(out.len() as u64 <= MOST, t!("sync-rest-the-answer-was-too-large"));
    Ok((status, out))
}

/// POSTs `body` to `url`, signed in as `auth`: the answer's status and bytes.
fn post(http: &reqwest::blocking::Client, url: &str, auth: &Auth, content_type: &str, body: &[u8], binding: &Binding) -> Result<(u16, Vec<u8>)> {
    read(send(http, url, auth, content_type, body, binding)?)
}

/// POSTs `body` to `url`, signed in as `auth`: the answer, not yet read
/// (refused sign-ins are errors). NTLM and Kerberos answers are bound to
/// the TLS channel (`binding`, learned from the server's certificate) and
/// to the service, so Exchange's Extended Protection takes them.
fn send(
    http: &reqwest::blocking::Client,
    url: &str,
    auth: &Auth,
    content_type: &str,
    body: &[u8],
    binding: &Binding,
) -> Result<reqwest::blocking::Response> {
    let request = |authorization: Option<String>, with_body: bool| -> Result<reqwest::blocking::Response> {
        let mut b = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .timeout(TRANSFER_TIMEOUT);
        if let Some(a) = authorization {
            b = b.header(reqwest::header::AUTHORIZATION, a);
        }
        b = if with_body { b.body(body.to_vec()) } else { b.body(Vec::new()) };
        let response = b.send().with_context(|| t!("sync-ews-couldnt-reach-the-exchange-server", url = url))?;
        if let Some(seen) = binding_of(&response) {
            *binding.lock().unwrap() = Some(seen);
        }
        Ok(response)
    };
    let refused = || -> anyhow::Error {
        anyhow::Error::new(AccessFailure::Authentication).context(t!("sync-ews-exchange-didnt-accept-the-sign-in"))
    };
    let checked = |r: reqwest::blocking::Response| -> Result<reqwest::blocking::Response> {
        if r.status().as_u16() == 401 {
            let _ = read(r);
            return Err(refused());
        }
        Ok(r)
    };
    let host = url::Url::parse(url)?.host_str().unwrap_or_default().to_owned();
    match auth {
        Auth::Basic { user, password } => {
            let token = b64(format!("{user}:{password}").as_bytes());
            checked(request(Some(format!("Basic {token}")), true)?)
        }
        Auth::Bearer(token) => checked(request(Some(format!("Bearer {token}")), true)?),
        Auth::Ntlm(id) => {
            // The handshake's first leg carries no body; the same kept
            // connection carries the second.
            let first = request(Some(format!("NTLM {}", b64(&ntlm::negotiate()))), false)?;
            let schemes: Vec<String> = first
                .headers()
                .get_all(reqwest::header::WWW_AUTHENTICATE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_owned))
                .collect();
            let _ = read(first)?;
            let offered = schemes.iter().find_map(|v| v.strip_prefix("NTLM ").map(str::to_owned));
            // A server that takes only Basic (over TLS): the same name and
            // password, as it asks.
            if offered.is_none() && schemes.iter().any(|v| v.to_ascii_lowercase().starts_with("basic")) {
                let user = if id.domain.is_empty() { id.user.clone() } else { format!("{}\\{}", id.domain, id.user) };
                return send(http, url, &Auth::Basic { user, password: id.password.clone() }, content_type, body, binding);
            }
            let challenge = ntlm::parse_challenge(&unb64(&offered.ok_or_else(refused)?)?)?;
            let bound = binding.lock().unwrap().clone();
            let answer = ntlm::authenticate(id, &challenge, bound.as_deref(), Some(&format!("HTTP/{host}")));
            checked(request(Some(format!("NTLM {}", b64(&answer))), true)?)
        }
        Auth::Kerberos => {
            // Over TLS, the ticket is bound to the channel: where the
            // certificate wasn't seen yet, a request without a sign-in
            // (refused) shows it.
            let tls = url.starts_with("https://");
            if tls && binding.lock().unwrap().is_none() {
                let _ = read(request(None, false)?);
            }
            let bound = binding.lock().unwrap().clone();
            let target = format!("HTTP@{host}");
            let mut context = match bound.filter(|_| tls) {
                Some(data) => crate::sync::gss::start_bound(&target, data)?,
                None => crate::sync::gss::start(&target)?,
            };
            let (token, _) = context.step(&[])?;
            let response = request(Some(format!("Negotiate {}", b64(&token))), true)?;
            // The server proves it is the service named (mutual).
            if let Some(back) = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Negotiate "))
                && response.status().is_success()
            {
                context.step(&unb64(back)?)?;
            }
            checked(response)
        }
    }
}

impl Session {
    pub fn open(url: &str, email: &str, auth: Auth, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self {
            http: client(tls.clone())?,
            url: url.to_owned(),
            auth,
            email: email.to_owned(),
            folders: std::sync::Mutex::new(Vec::new()),
            binding: Binding::default(),
            one_at_a_time: std::sync::Mutex::new(()),
            kinds: Default::default(),
            tls,
        })
    }

    /// The SOAP envelope around an operation.
    fn envelope(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="{SOAP}" xmlns:t="{TYPES}" xmlns:m="{MESSAGES}"><soap:Header><t:RequestServerVersion Version="{VERSION}"/></soap:Header><soap:Body>{body}</soap:Body></soap:Envelope>"#
        )
    }

    /// Sends one EWS operation (`body`, the SOAP body's content) and
    /// returns the answer's body, its response messages checked: one in
    /// error fails the call unless `allow` lists its code.
    fn call(&self, body: &str, allow: &[&str]) -> Result<String> {
        let envelope = Self::envelope(body);
        let (status, bytes) = {
            let _one = self.one_at_a_time.lock().unwrap_or_else(|e| e.into_inner());
            post(&self.http, &self.url, &self.auth, "text/xml; charset=utf-8", envelope.as_bytes(), &self.binding)?
        };
        let answer = String::from_utf8_lossy(&bytes).into_owned();
        checked(status, answer, allow)
    }

    // Folders.

    /// Reads the mail folders again, nested ones too, with the standard
    /// ones' distinguished names.
    pub fn reload_folders(&self) -> Result<Vec<MailFolder>> {
        let known: Vec<&str> = ["msgfolderroot", "inbox", "sentitems", "drafts", "deleteditems", "junkemail", "outbox"].to_vec();
        let ids: String = known.iter().map(|d| folder_id(d)).collect();
        let answer = self.call(
            &format!(r#"<m:GetFolder><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape></m:FolderShape><m:FolderIds>{ids}</m:FolderIds></m:GetFolder>"#),
            &["ErrorFolderNotFound"],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let mut distinguished: Vec<(String, String)> = Vec::new();
        for (name, m) in known.iter().zip(doc.descendants().filter(|n| named(*n, "GetFolderResponseMessage"))) {
            if let Some(id) = find(m, "FolderId").and_then(|f| f.attribute("Id")) {
                distinguished.push((id.to_owned(), (*name).to_owned()));
            }
        }
        let root = distinguished.iter().find(|(_, n)| n == "msgfolderroot").map(|(i, _)| i.clone()).unwrap_or_default();
        let mut found: Vec<(String, String, String, u64, String)> = Vec::new();
        let mut offset = 0;
        loop {
            let answer = self.call(
                &format!(
                    r#"<m:FindFolder Traversal="Deep"><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties><t:FieldURI FieldURI="folder:DisplayName"/><t:FieldURI FieldURI="folder:ParentFolderId"/><t:FieldURI FieldURI="folder:TotalCount"/><t:FieldURI FieldURI="folder:FolderClass"/></t:AdditionalProperties></m:FolderShape><m:IndexedPageFolderView MaxEntriesReturned="1000" Offset="{offset}" BasePoint="Beginning"/><m:ParentFolderIds>{}</m:ParentFolderIds></m:FindFolder>"#,
                    folder_id("msgfolderroot")
                ),
                &[],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            let mut page = 0;
            for f in doc.descendants().filter(|n| n.is_element() && n.tag_name().name().ends_with("Folder") && child(*n, "FolderId").is_some()) {
                page += 1;
                found.push((
                    child(f, "FolderId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned(),
                    text(f, "DisplayName"),
                    child(f, "ParentFolderId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned(),
                    text(f, "TotalCount").parse().unwrap_or_default(),
                    text(f, "FolderClass"),
                ));
            }
            let last = find(doc.root(), "RootFolder").and_then(|r| r.attribute("IncludesLastItemInRange")) != Some("false");
            offset += page;
            if last || page == 0 || found.len() >= 5000 {
                break;
            }
        }
        // Paths from the parents, as deep as they go.
        let path_of = |id: &str| -> (String, usize) {
            let mut names = Vec::new();
            let mut at = id.to_owned();
            for _ in 0..32 {
                match found.iter().find(|f| f.0 == at) {
                    Some(f) => {
                        names.push(f.1.clone());
                        if f.2 == root {
                            break;
                        }
                        at = f.2.clone();
                    }
                    None => break,
                }
            }
            names.reverse();
            let depth = names.len().saturating_sub(1);
            (names.join("/"), depth)
        };
        let mut folders: Vec<MailFolder> = found
            .iter()
            .map(|(id, name, parent, total, class)| {
                let (path, depth) = path_of(id);
                MailFolder {
                    id: id.clone(),
                    name: name.clone(),
                    parent_id: parent.clone(),
                    path,
                    depth,
                    distinguished: distinguished.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()),
                    total: *total,
                    class: class.clone(),
                }
            })
            .collect();
        // The Archive folder Outlook files to: at the top, so named.
        if let Some(f) = folders
            .iter_mut()
            .find(|f| f.parent_id == root && f.distinguished.is_none() && f.name.eq_ignore_ascii_case("Archive"))
        {
            f.distinguished = Some("archive".into());
        }
        *self.folders.lock().unwrap() = folders.clone();
        Ok(folders)
    }
    pub fn folders(&self) -> Result<Vec<MailFolder>> {
        let known = self.folders.lock().unwrap().clone();
        if known.is_empty() { self.reload_folders() } else { Ok(known) }
    }
    pub fn create_folder(&self, parent: &str, name: &str) -> Result<String> {
        let answer = self.call(
            &format!(r#"<m:CreateFolder><m:ParentFolderId>{}</m:ParentFolderId><m:Folders><t:Folder><t:FolderClass>IPF.Note</t:FolderClass><t:DisplayName>{}</t:DisplayName></t:Folder></m:Folders></m:CreateFolder>"#, folder_id(parent), esc(name)),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let id = find(doc.root(), "FolderId").and_then(|f| f.attribute("Id")).unwrap_or_default().to_owned();
        self.reload_folders()?;
        Ok(id)
    }
    pub fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        self.call(
            &format!(r#"<m:UpdateFolder><m:FolderChanges><t:FolderChange>{}<t:Updates><t:SetFolderField><t:FieldURI FieldURI="folder:DisplayName"/><t:Folder><t:DisplayName>{}</t:DisplayName></t:Folder></t:SetFolderField></t:Updates></t:FolderChange></m:FolderChanges></m:UpdateFolder>"#, folder_id(id), esc(name)),
            &[],
        )?;
        self.reload_folders()?;
        Ok(())
    }
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        // Into Deleted Items, as Outlook does.
        self.call(&format!(r#"<m:DeleteFolder DeleteType="MoveToDeletedItems"><m:FolderIds>{}</m:FolderIds></m:DeleteFolder>"#, folder_id(id)), &[])?;
        self.reload_folders()?;
        Ok(())
    }

    // Messages.

    /// One page of a folder's messages, newest first, from `offset` (since
    /// `since`, where given; matching AQS `query`, where given), and
    /// whether that was the last.
    pub fn list(&self, folder: &str, offset: usize, max: usize, since: Option<i64>, query: Option<&str>) -> Result<(Vec<Message>, bool)> {
        let restriction = since
            .map(|s| format!(r#"<m:Restriction><t:IsGreaterThanOrEqualTo><t:FieldURI FieldURI="item:DateTimeReceived"/><t:FieldURIOrConstant><t:Constant Value="{}"/></t:FieldURIOrConstant></t:IsGreaterThanOrEqualTo></m:Restriction>"#, ews_time(s)))
            .unwrap_or_default();
        // AQS can't be sorted; it comes newest first anyway.
        let (sort, query) = match query {
            Some(q) => (String::new(), format!("<m:QueryString>{}</m:QueryString>", esc(q))),
            None => (r#"<m:SortOrder><t:FieldOrder Order="Descending"><t:FieldURI FieldURI="item:DateTimeReceived"/></t:FieldOrder></m:SortOrder>"#.to_owned(), String::new()),
        };
        let answer = self.call(
            &format!(
                r#"<m:FindItem Traversal="Shallow"><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>{ITEM_PROPERTIES}</t:AdditionalProperties></m:ItemShape><m:IndexedPageItemView MaxEntriesReturned="{}" Offset="{offset}" BasePoint="Beginning"/>{restriction}{sort}<m:ParentFolderIds>{}</m:ParentFolderIds>{query}</m:FindItem>"#,
                max.clamp(1, 1000),
                folder_id(folder)
            ),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let root = find(doc.root(), "RootFolder");
        let last = root.and_then(|r| r.attribute("IncludesLastItemInRange")) != Some("false");
        let found = root.and_then(|r| child(r, "Items"));
        if let Some(items) = found {
            self.note_kinds(items.children().filter(|n| n.is_element()));
        }
        let items = found.map(|i| i.children().filter(|n| n.is_element()).map(Message::of).collect()).unwrap_or_default();
        Ok((items, last))
    }
    /// The ids of a folder's messages, up to `most` (unread only, with
    /// `unread`).
    pub fn ids_in(&self, folder: &str, unread: bool, most: usize) -> Result<Vec<String>> {
        let restriction = if unread {
            r#"<m:Restriction><t:IsEqualTo><t:FieldURI FieldURI="message:IsRead"/><t:FieldURIOrConstant><t:Constant Value="false"/></t:FieldURIOrConstant></t:IsEqualTo></m:Restriction>"#
        } else {
            ""
        };
        let mut out = Vec::new();
        loop {
            let answer = self.call(
                &format!(r#"<m:FindItem Traversal="Shallow"><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape></m:ItemShape><m:IndexedPageItemView MaxEntriesReturned="1000" Offset="{}" BasePoint="Beginning"/>{restriction}<m:ParentFolderIds>{}</m:ParentFolderIds></m:FindItem>"#, out.len(), folder_id(folder)),
                &[],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            let root = find(doc.root(), "RootFolder");
            let before = out.len();
            if let Some(items) = root.and_then(|r| child(r, "Items")) {
                out.extend(items.children().filter(|n| n.is_element()).map(|n| ItemId::of(n).id));
            }
            let last = root.and_then(|r| r.attribute("IncludesLastItemInRange")) != Some("false");
            if last || out.len() == before || out.len() >= most {
                break;
            }
        }
        out.truncate(most);
        Ok(out)
    }
    /// What the message list shows of each of `ids`; those gone are left out.
    pub fn messages(&self, ids: &[String]) -> Result<Vec<Message>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let answer = self.call(
                &format!(r#"<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>{ITEM_PROPERTIES}</t:AdditionalProperties></m:ItemShape><m:ItemIds>{}</m:ItemIds></m:GetItem>"#, item_ids(chunk)),
                &["ErrorItemNotFound", "ErrorInvalidIdMalformed", "ErrorInvalidIdNotAnItemAttachmentId"],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            for m in doc.descendants().filter(|n| named(*n, "GetItemResponseMessage")) {
                if let Some(items) = child(m, "Items") {
                    self.note_kinds(items.children().filter(|n| n.is_element()));
                    out.extend(items.children().filter(|n| n.is_element()).map(Message::of));
                }
            }
        }
        Ok(out)
    }
    /// An item's MIME content: a message's source; a calendar item's
    /// iCalendar; a contact's vCard.
    pub fn mime(&self, id: &str) -> Result<Vec<u8>> {
        Ok(self.mimes(&[id.to_owned()])?.into_iter().next().map(|(_, m)| m).ok_or(crate::sync::rest::NotFound)?)
    }
    /// The MIME content of each of `ids` that is still there: (id, bytes).
    pub fn mimes(&self, ids: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(20) {
            let answer = self.call(
                &format!(r#"<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:IncludeMimeContent>true</t:IncludeMimeContent></m:ItemShape><m:ItemIds>{}</m:ItemIds></m:GetItem>"#, item_ids(chunk)),
                &["ErrorItemNotFound"],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            for item in doc.descendants().filter(|n| n.is_element() && child(*n, "MimeContent").is_some()) {
                let id = ItemId::of(item).id;
                let content = child(item, "MimeContent").and_then(|c| c.text()).unwrap_or_default();
                out.push((id, unb64(content)?));
            }
        }
        Ok(out)
    }
    /// Notes the kind each item in `items` (an answer's `t:Items`) is.
    fn note_kinds<'a>(&self, items: impl Iterator<Item = Node<'a, 'a>>) {
        let mut kinds = self.kinds.lock().unwrap();
        // Bounded: ids seen long ago are looked up again if changed.
        if kinds.len() > 50_000 {
            kinds.clear();
        }
        for n in items {
            let id = ItemId::of(n).id;
            if !id.is_empty() {
                kinds.insert(id, n.tag_name().name().to_owned());
            }
        }
    }
    /// The kind of each of `ids` (asked of Exchange where not seen yet);
    /// those gone are left out.
    fn kinds_of(&self, ids: &[String]) -> Result<std::collections::HashMap<String, String>> {
        let unknown: Vec<String> = {
            let kinds = self.kinds.lock().unwrap();
            ids.iter().filter(|id| !kinds.contains_key(*id)).cloned().collect()
        };
        for chunk in unknown.chunks(BATCH) {
            let answer = self.call(
                &format!(r#"<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape></m:ItemShape><m:ItemIds>{}</m:ItemIds></m:GetItem>"#, item_ids(chunk)),
                &["ErrorItemNotFound", "ErrorInvalidIdMalformed"],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            self.note_kinds(doc.descendants().filter(|n| named(*n, "Items")).flat_map(|i| i.children().filter(|c| c.is_element())));
        }
        let kinds = self.kinds.lock().unwrap();
        Ok(ids.iter().filter_map(|id| Some((id.clone(), kinds.get(id)?.clone()))).collect())
    }
    /// Changes items' properties, each update written for the item's own
    /// kind (`t:Message`, `t:MeetingRequest`, `t:CalendarItem`…), as
    /// Exchange's schema wants; an update that kind hasn't is skipped.
    fn update(&self, changes: &[(String, Update)]) -> Result<()> {
        let ids: Vec<String> = changes.iter().map(|(id, _)| id.clone()).collect();
        let kinds = self.kinds_of(&ids)?;
        let rendered: Vec<String> = changes
            .iter()
            .filter_map(|(id, update)| {
                let xml = update.xml(kinds.get(id)?)?;
                Some(format!(r#"<t:ItemChange><t:ItemId Id="{}"/><t:Updates>{xml}</t:Updates></t:ItemChange>"#, esc(id)))
            })
            .collect();
        for chunk in rendered.chunks(BATCH) {
            let items: String = chunk.concat();
            self.call(
                &format!(r#"<m:UpdateItem MessageDisposition="SaveOnly" ConflictResolution="AlwaysOverwrite" SuppressReadReceipts="true"><m:ItemChanges>{items}</m:ItemChanges></m:UpdateItem>"#),
                &["ErrorItemNotFound"],
            )?;
        }
        Ok(())
    }
    pub fn set_read(&self, ids: &[String], read: bool) -> Result<()> {
        self.update(&ids.iter().map(|id| (id.clone(), Update::Read(read))).collect::<Vec<_>>())
    }
    /// Flags (PR_FLAG_STATUS 2) or clears the flag, as Outlook does.
    pub fn set_flagged(&self, ids: &[String], flagged: bool) -> Result<()> {
        let value = if flagged { 2 } else { 0 };
        self.update(&ids.iter().map(|id| (id.clone(), Update::Integer("0x1090", value))).collect::<Vec<_>>())
    }
    /// Sets PR_LAST_VERB_EXECUTED (102 replied, 104 forwarded).
    pub fn set_last_verb(&self, ids: &[String], verb: i64) -> Result<()> {
        self.update(&ids.iter().map(|id| (id.clone(), Update::Integer("0x1081", verb))).collect::<Vec<_>>())
    }
    /// Sets each item's categories: (id, categories).
    pub fn set_categories(&self, changes: &[(String, Vec<String>)]) -> Result<()> {
        self.update(&changes.iter().map(|(id, cats)| (id.clone(), Update::Categories(cats.clone()))).collect::<Vec<_>>())
    }
    /// Moves items into a folder; their new ids, in order (an item's id
    /// changes when it moves), empty for those that were gone.
    pub fn move_to(&self, ids: &[String], folder: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let answer = self.call(
                &format!(r#"<m:MoveItem ReturnNewItemIds="true"><m:ToFolderId>{}</m:ToFolderId><m:ItemIds>{}</m:ItemIds></m:MoveItem>"#, folder_id(folder), item_ids(chunk)),
                &["ErrorItemNotFound"],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            for m in doc.descendants().filter(|n| named(*n, "MoveItemResponseMessage")) {
                out.push(find(m, "ItemId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned());
            }
            // The moved items keep their kind under their new ids.
            let mut kinds = self.kinds.lock().unwrap();
            for (old, new) in chunk.iter().zip(&out[out.len().saturating_sub(chunk.len())..]) {
                if let Some(kind) = kinds.remove(old).filter(|_| !new.is_empty()) {
                    kinds.insert(new.clone(), kind);
                }
            }
        }
        Ok(out)
    }
    /// Deletes items for good.
    pub fn delete(&self, ids: &[String]) -> Result<()> {
        for chunk in ids.chunks(BATCH) {
            self.call(
                &format!(r#"<m:DeleteItem DeleteType="HardDelete" SendMeetingCancellations="SendToNone" AffectedTaskOccurrences="AllOccurrences"><m:ItemIds>{}</m:ItemIds></m:DeleteItem>"#, item_ids(chunk)),
                &["ErrorItemNotFound"],
            )?;
        }
        Ok(())
    }
    /// Deletes everything in a folder for good.
    pub fn empty(&self, folder: &str) -> Result<()> {
        self.call(&format!(r#"<m:EmptyFolder DeleteType="HardDelete" DeleteSubFolders="false"><m:FolderIds>{}</m:FolderIds></m:EmptyFolder>"#, folder_id(folder)), &[])?;
        Ok(())
    }

    /// Sends a finished message (its To, Cc and Bcc headers name who to)
    /// and keeps it in Sent Items; with `at`, Exchange holds it until
    /// then (PR_DEFERRED_SEND_TIME).
    pub fn send(&self, raw: &[u8], at: Option<i64>) -> Result<()> {
        let deferred = at
            .map(|at| format!(r#"<t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="0x3FEF" PropertyType="SystemTime"/><t:Value>{}</t:Value></t:ExtendedProperty>"#, ews_time(at)))
            .unwrap_or_default();
        self.call(
            &format!(r#"<m:CreateItem MessageDisposition="SendAndSaveCopy"><m:SavedItemFolderId>{}</m:SavedItemFolderId><m:Items><t:Message><t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent>{deferred}</t:Message></m:Items></m:CreateItem>"#, folder_id("sentitems"), b64(raw)),
            &[],
        )?;
        Ok(())
    }
    /// Puts a message in `folder` as it was: delivered, not a draft
    /// (PR_MESSAGE_FLAGS set as it is made), read or not. Its id.
    pub fn import(&self, raw: &[u8], folder: &str, read: bool) -> Result<String> {
        let answer = self.call(
            &format!(r#"<m:CreateItem MessageDisposition="SaveOnly"><m:SavedItemFolderId>{}</m:SavedItemFolderId><m:Items><t:Message><t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="0x0E07" PropertyType="Integer"/><t:Value>{}</t:Value></t:ExtendedProperty></t:Message></m:Items></m:CreateItem>"#, folder_id(folder), b64(raw), if read { 1 } else { 0 }),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(find(doc.root(), "ItemId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned())
    }
    /// Saves a draft in Drafts; its id.
    pub fn create_draft(&self, raw: &[u8]) -> Result<String> {
        let answer = self.call(
            &format!(r#"<m:CreateItem MessageDisposition="SaveOnly"><m:SavedItemFolderId>{}</m:SavedItemFolderId><m:Items><t:Message><t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent></t:Message></m:Items></m:CreateItem>"#, folder_id("drafts"), b64(raw)),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(find(doc.root(), "ItemId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned())
    }

    // Settings.

    pub fn oof(&self) -> Result<Oof> {
        let answer = self.call(
            &format!(r#"<m:GetUserOofSettingsRequest><t:Mailbox><t:Address>{}</t:Address></t:Mailbox></m:GetUserOofSettingsRequest>"#, esc(&self.email)),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let settings = find(doc.root(), "OofSettings").context("Exchange didn't say its Out of Office settings")?;
        let state = text(settings, "OofState");
        let scheduled = state == "Scheduled";
        let duration = child(settings, "Duration");
        let reply = |name: &str| child(settings, name).map(|r| text(r, "Message")).unwrap_or_default();
        Ok(Oof {
            enabled: state != "Disabled",
            internal: html_to_text(&reply("InternalReply")),
            external: html_to_text(&reply("ExternalReply")),
            start: if scheduled { duration.map_or(0, |d| time(&text(d, "StartTime"))) } else { 0 },
            end: if scheduled { duration.map_or(0, |d| time(&text(d, "EndTime"))) } else { 0 },
        })
    }
    pub fn set_oof(&self, o: &Oof) -> Result<()> {
        let scheduled = o.enabled && o.start > 0 && o.end > o.start;
        let state = if !o.enabled { "Disabled" } else if scheduled { "Scheduled" } else { "Enabled" };
        let duration = if scheduled {
            format!("<t:Duration><t:StartTime>{}</t:StartTime><t:EndTime>{}</t:EndTime></t:Duration>", ews_time(o.start), ews_time(o.end))
        } else {
            String::new()
        };
        let external = if o.external.is_empty() { &o.internal } else { &o.external };
        self.call(
            &format!(r#"<m:SetUserOofSettingsRequest><t:Mailbox><t:Address>{}</t:Address></t:Mailbox><t:UserOofSettings><t:OofState>{state}</t:OofState><t:ExternalAudience>All</t:ExternalAudience>{duration}<t:InternalReply><t:Message>{}</t:Message></t:InternalReply><t:ExternalReply><t:Message>{}</t:Message></t:ExternalReply></t:UserOofSettings></m:SetUserOofSettingsRequest>"#, esc(&self.email), esc(&o.internal), esc(external)),
            &[],
        )?;
        Ok(())
    }
    /// The inbox rules, in order.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        let answer = self.call(&format!(r#"<m:GetInboxRules><m:MailboxSmtpAddress>{}</m:MailboxSmtpAddress></m:GetInboxRules>"#, esc(&self.email)), &[])?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(doc
            .descendants()
            .filter(|n| named(*n, "Rule"))
            .map(|r| Rule { id: text(r, "RuleId"), json: rule_json(r) })
            .collect())
    }
    /// Applies rule operations (`t:CreateRuleOperation`… XML), in order.
    pub fn update_rules(&self, operations: &str) -> Result<()> {
        if operations.is_empty() {
            return Ok(());
        }
        self.call(
            &format!(r#"<m:UpdateInboxRules><m:MailboxSmtpAddress>{}</m:MailboxSmtpAddress><m:RemoveOutlookRuleBlob>true</m:RemoveOutlookRuleBlob><m:Operations>{operations}</m:Operations></m:UpdateInboxRules>"#, esc(&self.email)),
            &[],
        )?;
        Ok(())
    }

    // Calendars and contacts (as iCalendar and vCard).

    /// The calendar folders: the default one, and those inside it.
    pub fn calendars(&self) -> Result<Vec<(String, String)>> {
        self.special_folders("calendar")
    }
    /// A distinguished folder (as its id, then its name) and the folders
    /// inside it: (id, name).
    fn special_folders(&self, which: &str) -> Result<Vec<(String, String)>> {
        let answer = self.call(&format!(r#"<m:GetFolder><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape><m:FolderIds>{}</m:FolderIds></m:GetFolder>"#, folder_id(which)), &[])?;
        let doc = roxmltree::Document::parse(&answer)?;
        let top = find(doc.root(), "FolderId").and_then(|f| f.attribute("Id")).unwrap_or_default().to_owned();
        let name = find(doc.root(), "DisplayName").and_then(|n| n.text()).unwrap_or(which).to_owned();
        let mut out = vec![(top, name)];
        let answer = self.call(&format!(r#"<m:FindFolder Traversal="Deep"><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape><m:ParentFolderIds>{}</m:ParentFolderIds></m:FindFolder>"#, folder_id(which)), &[])?;
        let doc = roxmltree::Document::parse(&answer)?;
        for f in doc.descendants().filter(|n| n.is_element() && n.tag_name().name().ends_with("Folder") && child(*n, "FolderId").is_some()) {
            out.push((child(f, "FolderId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned(), text(f, "DisplayName")));
        }
        Ok(out)
    }
    /// A folder's items' ids (calendar items: those ending after `since`;
    /// series by their own item, not each occurrence).
    pub fn item_ids(&self, folder: &str, ends_after: Option<i64>, most: usize) -> Result<Vec<String>> {
        let restriction = ends_after
            .map(|t| format!(r#"<m:Restriction><t:Or><t:IsGreaterThanOrEqualTo><t:FieldURI FieldURI="calendar:End"/><t:FieldURIOrConstant><t:Constant Value="{}"/></t:FieldURIOrConstant></t:IsGreaterThanOrEqualTo><t:IsEqualTo><t:FieldURI FieldURI="calendar:IsRecurring"/><t:FieldURIOrConstant><t:Constant Value="true"/></t:FieldURIOrConstant></t:IsEqualTo></t:Or></m:Restriction>"#, ews_time(t)))
            .unwrap_or_default();
        let mut out = Vec::new();
        loop {
            let answer = self.call(
                &format!(r#"<m:FindItem Traversal="Shallow"><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape></m:ItemShape><m:IndexedPageItemView MaxEntriesReturned="500" Offset="{}" BasePoint="Beginning"/>{restriction}<m:ParentFolderIds>{}</m:ParentFolderIds></m:FindItem>"#, out.len(), folder_id(folder)),
                &[],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            let root = find(doc.root(), "RootFolder");
            let before = out.len();
            if let Some(items) = root.and_then(|r| child(r, "Items")) {
                out.extend(items.children().filter(|n| n.is_element()).map(|n| ItemId::of(n).id));
            }
            let last = root.and_then(|r| r.attribute("IncludesLastItemInRange")) != Some("false");
            if last || out.len() == before || out.len() >= most {
                break;
            }
        }
        Ok(out)
    }
    /// Makes a calendar item from iCalendar in `folder`, inviting its
    /// guests (Exchange sends the invitations); its id.
    pub fn create_event(&self, folder: &str, ics: &str, invite: bool) -> Result<String> {
        let send = if invite { "SendToAllAndSaveCopy" } else { "SendToNone" };
        let answer = self.call(
            &format!(r#"<m:CreateItem SendMeetingInvitations="{send}"><m:SavedItemFolderId>{}</m:SavedItemFolderId><m:Items><t:CalendarItem><t:MimeContent CharacterSet="UTF-8">{}</t:MimeContent></t:CalendarItem></m:Items></m:CreateItem>"#, folder_id(folder), b64(ics.as_bytes())),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(find(doc.root(), "ItemId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned())
    }
    /// Makes a calendar inside the default one; its id.
    pub fn create_calendar(&self, name: &str) -> Result<String> {
        let answer = self.call(
            &format!(r#"<m:CreateFolder><m:ParentFolderId>{}</m:ParentFolderId><m:Folders><t:CalendarFolder><t:DisplayName>{}</t:DisplayName></t:CalendarFolder></m:Folders></m:CreateFolder>"#, folder_id("calendar"), esc(name)),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(find(doc.root(), "FolderId").and_then(|f| f.attribute("Id")).unwrap_or_default().to_owned())
    }
    /// Renames or deletes any folder by id (calendars too), without
    /// reading the mail folders again.
    pub fn rename_any_folder(&self, id: &str, name: &str) -> Result<()> {
        self.call(
            &format!(r#"<m:UpdateFolder><m:FolderChanges><t:FolderChange>{}<t:Updates><t:SetFolderField><t:FieldURI FieldURI="folder:DisplayName"/><t:Folder><t:DisplayName>{}</t:DisplayName></t:Folder></t:SetFolderField></t:Updates></t:FolderChange></m:FolderChanges></m:UpdateFolder>"#, folder_id(id), esc(name)),
            &[],
        )?;
        Ok(())
    }
    pub fn delete_any_folder(&self, id: &str) -> Result<()> {
        self.call(&format!(r#"<m:DeleteFolder DeleteType="MoveToDeletedItems"><m:FolderIds>{}</m:FolderIds></m:DeleteFolder>"#, folder_id(id)), &[])?;
        Ok(())
    }
    /// Deletes a calendar item, telling guests (`tell`) or not.
    pub fn delete_event(&self, id: &str, tell: bool) -> Result<()> {
        let send = if tell { "SendToAllAndSaveCopy" } else { "SendToNone" };
        match self.call(&format!(r#"<m:DeleteItem DeleteType="MoveToDeletedItems" SendMeetingCancellations="{send}"><m:ItemIds>{}</m:ItemIds></m:DeleteItem>"#, item_ids(&[id.to_owned()])), &[]) {
            Err(e) if crate::sync::rest::not_found(&e) => Ok(()),
            other => other.map(drop),
        }
    }
    /// Answers a meeting in the calendar (accepted, maybe, declined),
    /// without a reply of Exchange's own (Picacho sent one by mail).
    pub fn respond_event(&self, id: &str, response: &str) -> Result<()> {
        self.call(
            &format!(r#"<m:CreateItem MessageDisposition="SaveOnly"><m:Items><t:{response}><t:ReferenceItemId Id="{}"/></t:{response}></m:Items></m:CreateItem>"#, esc(id)),
            &[],
        )?;
        Ok(())
    }
    /// When people are busy (GetUserAvailability), in UTC: each address
    /// with its busy times.
    pub fn free_busy(&self, people: &[String], from: i64, to: i64) -> Result<Vec<crate::sync::ical::FreeBusy>> {
        let utc_rule = "<t:Bias>0</t:Bias><t:Time>00:00:00</t:Time><t:DayOrder>1</t:DayOrder><t:Month>1</t:Month><t:DayOfWeek>Sunday</t:DayOfWeek>";
        let local = |at: i64| ews_time(at).trim_end_matches('Z').to_owned();
        let mailboxes: String = people
            .iter()
            .map(|p| format!("<t:MailboxData><t:Email><t:Address>{}</t:Address></t:Email><t:AttendeeType>Required</t:AttendeeType></t:MailboxData>", esc(p)))
            .collect();
        let answer = self.call(
            &format!(
                "<m:GetUserAvailabilityRequest><t:TimeZone><t:Bias>0</t:Bias><t:StandardTime>{utc_rule}</t:StandardTime><t:DaylightTime>{utc_rule}</t:DaylightTime></t:TimeZone><m:MailboxDataArray>{mailboxes}</m:MailboxDataArray><t:FreeBusyViewOptions><t:TimeWindow><t:StartTime>{}</t:StartTime><t:EndTime>{}</t:EndTime></t:TimeWindow><t:MergedFreeBusyIntervalInMinutes>30</t:MergedFreeBusyIntervalInMinutes><t:RequestedView>FreeBusy</t:RequestedView></t:FreeBusyViewOptions></m:GetUserAvailabilityRequest>",
                local(from),
                local(to)
            ),
            &["ErrorMailRecipientNotFound", "ErrorNoFreeBusyAccess"],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let at = |s: String| chrono::NaiveDateTime::parse_from_str(s.trim().trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S").map_or(0, |d| d.and_utc().timestamp());
        Ok(people
            .iter()
            .zip(doc.descendants().filter(|n| named(*n, "FreeBusyResponse")))
            .map(|(person, r)| crate::sync::ical::FreeBusy {
                attendee: person.clone(),
                busy: r
                    .descendants()
                    .filter(|n| named(*n, "CalendarEvent"))
                    .filter(|e| text(*e, "BusyType") != "Free")
                    .map(|e| (at(text(e, "StartTime")), at(text(e, "EndTime"))))
                    .collect(),
            })
            .collect())
    }
    /// Makes a contact; its id.
    pub fn create_contact(&self, folder: &str, contact_xml: &str) -> Result<String> {
        let answer = self.call(&format!(r#"<m:CreateItem><m:SavedItemFolderId>{}</m:SavedItemFolderId><m:Items>{contact_xml}</m:Items></m:CreateItem>"#, folder_id(folder)), &[])?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(find(doc.root(), "ItemId").and_then(|i| i.attribute("Id")).unwrap_or_default().to_owned())
    }
    /// People the directory (and your contacts) know by `text`: (name,
    /// address, title, company, phones).
    pub fn resolve_names(&self, text_: &str) -> Result<Vec<Person>> {
        let answer = self.call(
            &format!(r#"<m:ResolveNames ReturnFullContactData="true" SearchScope="ActiveDirectoryContacts"><m:UnresolvedEntry>{}</m:UnresolvedEntry></m:ResolveNames>"#, esc(text_)),
            &["ErrorNameResolutionNoResults", "ErrorNameResolutionMultipleResults"],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        Ok(doc
            .descendants()
            .filter(|n| named(*n, "Resolution"))
            .filter_map(|r| {
                let mailbox = child(r, "Mailbox")?;
                let email = text(mailbox, "EmailAddress");
                if email.is_empty() {
                    return None;
                }
                let contact = child(r, "Contact");
                let phones = contact
                    .and_then(|c| child(c, "PhoneNumbers"))
                    .map(|p| children(p, "Entry").filter_map(|e| e.text()).filter(|t| !t.trim().is_empty()).map(str::to_owned).collect())
                    .unwrap_or_default();
                Some((
                    text(mailbox, "Name"),
                    email,
                    contact.map(|c| text(c, "JobTitle")).unwrap_or_default(),
                    contact.map(|c| text(c, "CompanyName")).unwrap_or_default(),
                    phones,
                ))
            })
            .collect())
    }
    /// Something that changes whenever the inbox does.
    pub fn inbox_signature(&self) -> Result<String> {
        let answer = self.call(&format!(r#"<m:GetFolder><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape><m:FolderIds>{}</m:FolderIds></m:GetFolder>"#, folder_id("inbox")), &[])?;
        let doc = roxmltree::Document::parse(&answer)?;
        let total = find(doc.root(), "TotalCount").and_then(|n| n.text()).unwrap_or_default().to_owned();
        let unread = find(doc.root(), "UnreadCount").and_then(|n| n.text()).unwrap_or_default().to_owned();
        let (newest, _) = self.list("inbox", 0, 1, None, None)?;
        Ok(format!("{total}:{unread}:{}", newest.first().map(|m| m.id.as_str()).unwrap_or_default()))
    }

    /// The space the mailbox's mail takes, in bytes: the sum of its
    /// folders' sizes (PR_MESSAGE_SIZE_EXTENDED), where Exchange says them.
    /// EWS doesn't tell a user their quota.
    pub fn mailbox_size(&self) -> Result<Option<u64>> {
        const SIZE: &str = r#"<t:ExtendedFieldURI PropertyTag="0x0E08" PropertyType="Long"/>"#;
        let sizes = |doc: &roxmltree::Document| -> Vec<u64> {
            doc.descendants()
                .filter(|n| named(*n, "ExtendedProperty"))
                .filter(|p| child(*p, "ExtendedFieldURI").and_then(|u| u.attribute("PropertyTag")).is_some_and(|t| property_tag(t) == Some(0x0E08)))
                .filter_map(|p| text(p, "Value").trim().parse().ok())
                .collect()
        };
        let answer = self.call(
            &format!(r#"<m:GetFolder><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>{SIZE}</t:AdditionalProperties></m:FolderShape><m:FolderIds>{}</m:FolderIds></m:GetFolder>"#, folder_id("msgfolderroot")),
            &[],
        )?;
        let mut found = sizes(&roxmltree::Document::parse(&answer)?);
        let mut offset = 0;
        loop {
            let answer = self.call(
                &format!(r#"<m:FindFolder Traversal="Deep"><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>{SIZE}</t:AdditionalProperties></m:FolderShape><m:IndexedPageFolderView MaxEntriesReturned="1000" Offset="{offset}" BasePoint="Beginning"/><m:ParentFolderIds>{}</m:ParentFolderIds></m:FindFolder>"#, folder_id("msgfolderroot")),
                &[],
            )?;
            let doc = roxmltree::Document::parse(&answer)?;
            let page = doc.descendants().filter(|n| n.is_element() && n.tag_name().name().ends_with("Folder") && child(*n, "FolderId").is_some()).count();
            found.extend(sizes(&doc));
            let last = find(doc.root(), "RootFolder").and_then(|r| r.attribute("IncludesLastItemInRange")) != Some("false");
            offset += page;
            if last || page == 0 || offset >= 10_000 {
                break;
            }
        }
        Ok((!found.is_empty()).then(|| found.iter().sum()))
    }

    /// Waits for changes in the inbox with a streaming subscription
    /// (Exchange 2010 SP1 and later), calling `changed` as they come, until
    /// `stop` is set (looked at between answers) or something fails. Each
    /// request is held open for `minutes` and then asked again; a
    /// subscription Exchange forgot is made again, `changed` being called
    /// as events may have been missed meanwhile; so it is once subscribed
    /// where `after_gap` (an earlier stream broke).
    pub fn stream_inbox(&self, stop: &std::sync::atomic::AtomicBool, minutes: u32, after_gap: bool, mut changed: impl FnMut()) -> Result<()> {
        use std::io::Read as _;
        use std::sync::atomic::Ordering;
        let minutes = minutes.clamp(1, 30);
        let http = client_timing(self.tls.clone(), Duration::from_secs(u64::from(minutes) * 60 + 90))?;
        #[cfg(not(test))]
        const QUICK_PAUSE: Duration = Duration::from_secs(1);
        #[cfg(test)]
        const QUICK_PAUSE: Duration = Duration::from_millis(20);
        let mut subscription = self.subscribe_inbox()?;
        if after_gap && !stop.load(Ordering::Acquire) {
            changed();
        }
        // Requests that end at once (a proxy that won't hold them open):
        // a pause after each, and after a few in a row, a failure.
        let mut quick_ends = 0;
        while !stop.load(Ordering::Acquire) {
            let started = std::time::Instant::now();
            let request = Self::envelope(&format!(
                "<m:GetStreamingEvents><m:SubscriptionIds><t:SubscriptionId>{}</t:SubscriptionId></m:SubscriptionIds><m:ConnectionTimeout>{minutes}</m:ConnectionTimeout></m:GetStreamingEvents>",
                esc(&subscription)
            ));
            let mut response = send(&http, &self.url, &self.auth, "text/xml; charset=utf-8", request.as_bytes(), &self.binding)?;
            if response.status().as_u16() != 200 {
                let (status, body) = read(response)?;
                checked(status, String::from_utf8_lossy(&body).into_owned(), &[])?;
                bail!(t!("sync-ews-exchange-answered", status = status, message = ""));
            }
            let mut pending: Vec<u8> = Vec::new();
            let mut chunk = vec![0u8; 16 * 1024];
            let mut forgotten = false;
            loop {
                let n = response.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..n]);
                ensure!(pending.len() <= 16 * 1024 * 1024, t!("sync-rest-the-answer-was-too-large"));
                while let Some(end) = envelope_end(&pending) {
                    let one: Vec<u8> = pending.drain(..end).collect();
                    match stream_answer(String::from_utf8_lossy(&one).into_owned())? {
                        Streamed::Events if !stop.load(Ordering::Acquire) => changed(),
                        Streamed::Forgotten => forgotten = true,
                        _ => {}
                    }
                }
                if stop.load(Ordering::Acquire) || forgotten {
                    break;
                }
            }
            if forgotten && !stop.load(Ordering::Acquire) {
                subscription = self.subscribe_inbox()?;
                changed();
            }
            if started.elapsed() < Duration::from_secs(2) {
                quick_ends += 1;
                ensure!(quick_ends < 5, t!("sync-ews-exchange-wont-hold-the-stream-open"));
                std::thread::sleep(QUICK_PAUSE);
            } else {
                quick_ends = 0;
            }
        }
        Ok(())
    }
    /// A streaming subscription to the inbox's changes: its id.
    fn subscribe_inbox(&self) -> Result<String> {
        let events: String = ["NewMailEvent", "CreatedEvent", "DeletedEvent", "ModifiedEvent", "MovedEvent", "CopiedEvent"]
            .iter()
            .map(|e| format!("<t:EventType>{e}</t:EventType>"))
            .collect();
        let answer = self.call(
            &format!("<m:Subscribe><m:StreamingSubscriptionRequest><t:FolderIds>{}</t:FolderIds><t:EventTypes>{events}</t:EventTypes></m:StreamingSubscriptionRequest></m:Subscribe>", folder_id("inbox")),
            &[],
        )?;
        let doc = roxmltree::Document::parse(&answer)?;
        let id = find(doc.root(), "SubscriptionId").and_then(|n| n.text()).unwrap_or_default().trim().to_owned();
        ensure!(!id.is_empty(), t!("sync-ews-exchange-answered", status = 200, message = "no subscription"));
        Ok(id)
    }
}

/// A property tag as Exchange writes it (`0xe08`, `0x0E08`).
fn property_tag(tag: &str) -> Option<u32> {
    let hex = tag.trim().strip_prefix("0x").or_else(|| tag.trim().strip_prefix("0X"))?;
    u32::from_str_radix(hex, 16).ok()
}

/// What one answer of a streaming request said.
#[derive(Debug, PartialEq, Eq)]
enum Streamed {
    /// Something changed.
    Events,
    /// Nothing did (a keep-alive, or the request ending).
    Quiet,
    /// The subscription is gone (expired, or events were lost): make it again.
    Forgotten,
}
/// Reads one SOAP envelope of a streaming answer.
fn stream_answer(envelope: String) -> Result<Streamed> {
    const FORGOTTEN: &[&str] = &["ErrorInvalidSubscription", "ErrorSubscriptionNotFound", "ErrorExpiredSubscription", "ErrorMissedNotificationEvents"];
    let answer = checked(200, envelope, FORGOTTEN)?;
    let doc = roxmltree::Document::parse(&answer)?;
    let mut out = Streamed::Quiet;
    for m in doc.descendants().filter(|n| named(*n, "GetStreamingEventsResponseMessage")) {
        if m.attribute("ResponseClass") == Some("Error") && FORGOTTEN.contains(&text(m, "ResponseCode").as_str()) {
            return Ok(Streamed::Forgotten);
        }
        let events = m
            .descendants()
            .filter(|n| named(*n, "Notification"))
            .flat_map(|n| n.children().filter(|c| c.is_element()))
            .any(|e| !matches!(e.tag_name().name(), "SubscriptionId" | "PreviousWatermark" | "MoreEvents" | "StatusEvent"));
        if events {
            out = Streamed::Events;
        }
    }
    Ok(out)
}
/// Where the first complete SOAP envelope in `bytes` ends (after its
/// closing tag, whatever its prefix), if one is complete.
fn envelope_end(bytes: &[u8]) -> Option<usize> {
    const CLOSE: &[u8] = b"Envelope>";
    let mut from = 0;
    while let Some(at) = bytes[from..].windows(CLOSE.len()).position(|w| w == CLOSE).map(|p| p + from) {
        if let Some(open) = bytes[..at].iter().rposition(|&c| c == b'<') {
            let tag = &bytes[open + 1..at];
            if tag.first() == Some(&b'/') && tag[1..].iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b':' | b'_' | b'-' | b'.')) {
                return Some(at + CLOSE.len());
            }
        }
        from = at + CLOSE.len();
    }
    None
}

/// An EWS answer, its response messages checked: one in error fails
/// unless `allow` lists its code.
fn checked(status: u16, answer: String, allow: &[&str]) -> Result<String> {
    if status == 503 || status == 429 {
        bail!("[THROTTLED] {}", t!("sync-rest-is-limiting-requests", service = "Exchange", status = status));
    }
    let doc = roxmltree::Document::parse(&answer)
        .map_err(|_| anyhow::anyhow!(t!("sync-ews-exchange-answered", status = status, message = answer.chars().take(200).collect::<String>())))?;
    if let Some(fault) = find(doc.root(), "Fault") {
        let message = text_of_any(fault, &["faultstring", "Message"]);
        if message.contains("ErrorServerBusy") || answer.contains("ErrorServerBusy") {
            bail!("[THROTTLED] {}", t!("sync-rest-is-limiting-requests", service = "Exchange", status = status));
        }
        bail!(t!("sync-ews-exchange-answered", status = status, message = message));
    }
    for m in doc.descendants().filter(|n| n.is_element() && n.tag_name().name().ends_with("ResponseMessage")) {
        if m.attribute("ResponseClass") == Some("Error") {
            let code = text(m, "ResponseCode");
            if allow.contains(&code.as_str()) {
                continue;
            }
            if code == "ErrorServerBusy" {
                bail!("[THROTTLED] {}", t!("sync-rest-is-limiting-requests", service = "Exchange", status = status));
            }
            if code == "ErrorItemNotFound" || code == "ErrorFolderNotFound" {
                return Err(anyhow::Error::new(crate::sync::rest::NotFound));
            }
            bail!(t!("sync-ews-exchange-answered", status = code, message = text(m, "MessageText")));
        }
    }
    ensure!((200..300).contains(&status), t!("sync-ews-exchange-answered", status = status, message = ""));
    Ok(answer)
}

fn text_of_any(n: Node, names: &[&str]) -> String {
    names.iter().find_map(|name| find(n, name).and_then(|c| c.text())).unwrap_or_default().to_owned()
}
/// Exchange's Out of Office text comes as HTML.
fn html_to_text(html: &str) -> String {
    if !html.contains('<') {
        return html.trim().to_owned();
    }
    crate::sync::mime::html_to_text(html).trim().to_owned()
}

/// Finds a mailbox's EWS address by Autodiscover, as Outlook does
/// ([MS-OXDSCLI] §3.1.5): POX at the address's domain and at
/// `autodiscover.` it, then where `http://autodiscover.` it redirects,
/// then where the domain's `_autodiscover._tcp` SRV record points, signed
/// in as the account is. Where a server has POX turned off, its SOAP
/// Autodiscover ([MS-OXWSADISC]) is asked instead. An answer may send the
/// question on to another address (`redirectAddr`, up to ten times) or
/// server (`redirectUrl`, https only). The password only goes to servers
/// within the address's domain or ones such a server named: a redirect or
/// an SRV record pointing elsewhere is passed over, as anyone on the
/// network could have sent it. `None` where none answers.
pub fn autodiscover(email: &str, auth: &Auth, tls: native_tls::TlsConnector) -> Result<Option<String>> {
    autodiscover_with(email, auth, tls, &Places { well_known: &well_known, fallbacks: &fallbacks })
}

/// Where Autodiscover asks for a domain: the well-known places, then (only
/// where those don't answer) the redirect and SRV ones.
struct Places<'a> {
    well_known: &'a dyn Fn(&str) -> Vec<String>,
    fallbacks: &'a dyn Fn(&reqwest::blocking::Client, &str) -> Vec<String>,
}
fn well_known(domain: &str) -> Vec<String> {
    vec![
        format!("https://{domain}/autodiscover/autodiscover.xml"),
        format!("https://autodiscover.{domain}/autodiscover/autodiscover.xml"),
    ]
}
fn fallbacks(http: &reqwest::blocking::Client, domain: &str) -> Vec<String> {
    let mut out = Vec::new();
    // The HTTP redirect method: an unauthenticated GET, its redirect
    // followed only to https within the domain.
    if let Ok(r) = http.get(format!("http://autodiscover.{domain}/autodiscover/autodiscover.xml")).send()
        && r.status().is_redirection()
        && let Some(to) = r.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok())
        && let Some(url) = redirect_target(to, domain)
    {
        out.push(url);
    }
    // The SRV record method.
    for (host, port) in crate::sync::auth::srv(&format!("_autodiscover._tcp.{domain}")).unwrap_or_default() {
        if crate::sync::discover::in_domain(&host, domain) {
            out.push(format!("https://{}:{port}/autodiscover/autodiscover.xml", host.trim_end_matches('.')));
        }
    }
    out
}
/// Where an unauthenticated redirect may send Autodiscover: https, to a
/// host within `domain`.
fn redirect_target(location: &str, domain: &str) -> Option<String> {
    let url = url::Url::parse(location).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && crate::sync::discover::in_domain(host, domain)).then(|| url.to_string())
}

/// What an Autodiscover answer says.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Ews(String),
    /// Ask again for this address.
    Address(String),
    /// Ask this server.
    Server(String),
    Nothing,
}

fn autodiscover_with(email: &str, auth: &Auth, tls: native_tls::TlsConnector, places: &Places) -> Result<Option<String>> {
    let http = client(tls)?;
    let pox_request = |address: &str| {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006"><Request><EMailAddress>{}</EMailAddress><AcceptableResponseSchema>http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a</AcceptableResponseSchema></Request></Autodiscover>"#,
            esc(address)
        )
    };
    let mut refused = None;
    let mut address = email.to_owned();
    let mut asked: Vec<String> = Vec::new();
    'addresses: for _ in 0..10 {
        let domain = address.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()).context("not an address")?;
        let mut queue: std::collections::VecDeque<String> = (places.well_known)(&domain).into();
        let mut fell_back = false;
        while let Some(url) = queue.pop_front().or_else(|| {
            if fell_back {
                return None;
            }
            fell_back = true;
            queue.extend((places.fallbacks)(&http, &domain));
            queue.pop_front()
        }) {
            if asked.contains(&format!("{address} {url}")) || asked.len() > 40 {
                continue;
            }
            asked.push(format!("{address} {url}"));
            // A connection's binding is its own server's.
            let binding = Binding::default();
            let found = match post(&http, &url, auth, "text/xml; charset=utf-8", pox_request(&address).as_bytes(), &binding) {
                Ok((200, body)) => match pox_answer(&String::from_utf8_lossy(&body)) {
                    Found::Nothing => soap_autodiscover(&http, &url, auth, &address, &binding),
                    found => found,
                },
                Ok(_) => soap_autodiscover(&http, &url, auth, &address, &binding),
                Err(e) if e.downcast_ref::<AccessFailure>().is_some() => {
                    refused = Some(e);
                    Found::Nothing
                }
                Err(_) => Found::Nothing,
            };
            match found {
                Found::Ews(url) => return Ok(Some(url)),
                Found::Address(next) if next.contains('@') && !next.eq_ignore_ascii_case(&address) => {
                    address = next;
                    continue 'addresses;
                }
                Found::Server(next) if next.starts_with("https://") || (cfg!(test) && next.starts_with("http://")) => queue.push_front(next),
                _ => {}
            }
        }
        break;
    }
    match refused {
        Some(e) => Err(e),
        None => Ok(None),
    }
}
/// A POX Autodiscover answer: the EWS address, or where to ask instead.
fn pox_answer(xml: &str) -> Found {
    if let Some(url) = ews_url_in(xml) {
        return Found::Ews(url);
    }
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Found::Nothing };
    let Some(account) = find(doc.root(), "Account") else { return Found::Nothing };
    match text(account, "Action").as_str() {
        "redirectAddr" => Found::Address(text(account, "RedirectAddr").trim().to_owned()),
        "redirectUrl" => Found::Server(text(account, "RedirectUrl").trim().to_owned()),
        _ => Found::Nothing,
    }
}
/// SOAP Autodiscover (GetUserSettings) at the server POX was asked of.
fn soap_autodiscover(http: &reqwest::blocking::Client, pox_url: &str, auth: &Auth, address: &str, binding: &Binding) -> Found {
    let Some(base) = pox_url.strip_suffix("autodiscover.xml") else { return Found::Nothing };
    let url = format!("{base}autodiscover.svc");
    let request = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="{SOAP}" xmlns:a="http://schemas.microsoft.com/exchange/2010/Autodiscover" xmlns:wsa="http://www.w3.org/2005/08/addressing"><soap:Header><a:RequestedServerVersion>Exchange2010</a:RequestedServerVersion><wsa:Action>http://schemas.microsoft.com/exchange/2010/Autodiscover/Autodiscover/GetUserSettings</wsa:Action><wsa:To>{}</wsa:To></soap:Header><soap:Body><a:GetUserSettingsRequestMessage><a:Request><a:Users><a:User><a:Mailbox>{}</a:Mailbox></a:User></a:Users><a:RequestedSettings><a:Setting>InternalEwsUrl</a:Setting><a:Setting>ExternalEwsUrl</a:Setting></a:RequestedSettings></a:Request></a:GetUserSettingsRequestMessage></soap:Body></soap:Envelope>"#,
        esc(&url),
        esc(address)
    );
    match post(http, &url, auth, "text/xml; charset=utf-8", request.as_bytes(), binding) {
        Ok((200, body)) => soap_answer(&String::from_utf8_lossy(&body)),
        _ => Found::Nothing,
    }
}
/// A SOAP Autodiscover answer: the internal EWS address, else the
/// external one, or where to ask instead.
fn soap_answer(xml: &str) -> Found {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Found::Nothing };
    let Some(user) = find(doc.root(), "UserResponse") else { return Found::Nothing };
    let usable = |u: &str| u.starts_with("https://") || (cfg!(test) && u.starts_with("http://"));
    match text(user, "ErrorCode").as_str() {
        "RedirectAddress" => return Found::Address(text(user, "RedirectTarget").trim().to_owned()),
        "RedirectUrl" => return Found::Server(text(user, "RedirectTarget").trim().to_owned()),
        _ => {}
    }
    let setting = |name: &str| {
        user.descendants()
            .filter(|n| named(*n, "UserSetting"))
            .find(|s| text(*s, "Name") == name)
            .map(|s| text(s, "Value").trim().to_owned())
            .filter(|v| usable(v))
    };
    setting("InternalEwsUrl").or_else(|| setting("ExternalEwsUrl")).map_or(Found::Nothing, Found::Ews)
}
/// The EWS address in an Autodiscover answer: the internal Exchange
/// protocol's, else the external one's (Outlook Anywhere).
pub fn ews_url_in(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let mut found: Option<String> = None;
    for p in doc.descendants().filter(|n| named(*n, "Protocol")) {
        let kind = text(p, "Type");
        let url = text(p, "EwsUrl");
        if url.starts_with("https://") || (cfg!(test) && url.starts_with("http://")) {
            if kind == "EXCH" {
                return Some(url);
            }
            found.get_or_insert(url);
        }
    }
    found
}

#[cfg(test)]
pub(crate) mod tests;
