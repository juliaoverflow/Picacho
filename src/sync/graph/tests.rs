//! A small Microsoft Graph over loopback HTTP, by Microsoft's
//! documentation, and tests of the client against it. Manage's tests
//! (`manage/graph_tests.rs`) use it too.
use super::*;
use crate::sync::rest::test_server::{self, Answer, Request, json as answer};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct FakeFolder {
    pub id: String,
    pub name: String,
    pub parent: String,
    pub well_known: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FakeMessage {
    pub id: String,
    pub folder: String,
    pub raw: Vec<u8>,
    pub subject: String,
    pub read: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub received: i64,
    pub draft: bool,
    pub verb: Option<i64>,
}

#[derive(Default)]
pub(crate) struct Fake {
    pub folders: Vec<FakeFolder>,
    pub messages: Vec<FakeMessage>,
    pub sent: Vec<Vec<u8>>,
    pub rules: Vec<Value>,
    pub replies: Value,
    pub next: u64,
    pub batches: usize,
    /// Calendars, events, contacts (for `graph/dav.rs`'s tests).
    pub calendars: Vec<Value>,
    pub events: Vec<Value>,
    pub contacts: Vec<Value>,
    pub people: Vec<Value>,
    /// Files given with each message made from its parts: (id, how many).
    pub attachments: Vec<(String, usize)>,
    /// Drafts sent (`/send`): their source and deferred delivery time.
    pub sent_drafts: Vec<(Vec<u8>, Option<String>)>,
    /// Deferred delivery times set on drafts, by id.
    pub deferred: Vec<(String, String)>,
    /// Files uploaded in pieces: (message id, name, bytes so far, size).
    pub uploads: Vec<(String, String, Vec<u8>, usize)>,
}

const ROOT: &str = "root";

impl Fake {
    pub fn new() -> Self {
        let mut f = Fake { next: 1, ..Default::default() };
        for (id, name, wk) in [
            ("f-inbox", "Inbox", "inbox"),
            ("f-sent", "Sent Items", "sentitems"),
            ("f-drafts", "Drafts", "drafts"),
            ("f-trash", "Deleted Items", "deleteditems"),
            ("f-junk", "Junk Email", "junkemail"),
            ("f-archive", "Archive", "archive"),
            ("f-outbox", "Outbox", "outbox"),
        ] {
            f.folders.push(FakeFolder { id: id.into(), name: name.into(), parent: ROOT.into(), well_known: Some(wk.into()) });
        }
        f.replies = json!({"status": "disabled", "externalAudience": "all", "internalReplyMessage": "", "externalReplyMessage": ""});
        f
    }
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}=", self.next)
    }
    pub fn folder(&mut self, name: &str, parent: &str) -> String {
        let id = self.id("f-");
        self.folders.push(FakeFolder { id: id.clone(), name: name.into(), parent: parent.into(), well_known: None });
        id
    }
    /// A folder by id or well-known name.
    fn resolve(&self, key: &str) -> Option<String> {
        if key == "msgfolderroot" {
            return Some(ROOT.into());
        }
        self.folders
            .iter()
            .find(|f| f.id == key || f.well_known.as_deref() == Some(key))
            .map(|f| f.id.clone())
    }
    pub fn add(&mut self, subject: &str, folder: &str, minutes_ago: i64) -> String {
        let id = self.id("AAMk");
        let folder = self.resolve(folder).unwrap();
        let received = chrono::Utc::now().timestamp() - minutes_ago * 60;
        let date = chrono::DateTime::from_timestamp(received, 0).unwrap().to_rfc2822();
        let raw = format!(
            "From: Bo Berg <bo@example.se>\r\nTo: qa@example.com\r\nSubject: {subject}\r\nDate: {date}\r\nMessage-ID: <{id}@example.se>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nAbout {subject}.\r\n"
        );
        self.messages.push(FakeMessage {
            id: id.clone(),
            folder,
            raw: raw.into_bytes(),
            subject: subject.into(),
            read: false,
            flagged: false,
            categories: Vec::new(),
            received,
            draft: false,
            verb: None,
        });
        id
    }
    pub fn message(&self, id: &str) -> Option<&FakeMessage> {
        self.messages.iter().find(|m| m.id == id)
    }
    fn message_json(&self, m: &FakeMessage) -> Value {
        let mut props = vec![json!({"id": "Integer 0xe08", "value": m.raw.len().to_string()})];
        if let Some(v) = m.verb {
            props.push(json!({"id": "Integer 0x1081", "value": v.to_string()}));
        }
        json!({
            "id": m.id, "conversationId": format!("c-{}", m.subject), "parentFolderId": m.folder,
            "subject": m.subject,
            "from": {"emailAddress": {"name": "Bo Berg", "address": "bo@example.se"}},
            "toRecipients": [{"emailAddress": {"name": "", "address": "qa@example.com"}}],
            "ccRecipients": [], "replyTo": [],
            "receivedDateTime": graph_time(m.received), "isRead": m.read,
            "flag": {"flagStatus": if m.flagged { "flagged" } else { "notFlagged" }},
            "categories": m.categories, "hasAttachments": false, "bodyPreview": format!("About {}.", m.subject),
            "internetMessageId": format!("<{}@example.se>", m.id), "isDraft": m.draft,
            "singleValueExtendedProperties": props,
        })
    }
    fn folder_json(&self, f: &FakeFolder) -> Value {
        json!({
            "id": f.id, "displayName": f.name, "parentFolderId": f.parent,
            "childFolderCount": self.folders.iter().filter(|c| c.parent == f.id).count(),
            "totalItemCount": self.messages.iter().filter(|m| m.folder == f.id).count(),
        })
    }
    fn page(&self, items: Vec<Value>, params: &[(String, String)], path: &str) -> Value {
        let param = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let top: usize = param("$top").and_then(|t| t.parse().ok()).unwrap_or(10);
        let skip: usize = param("$skip").and_then(|t| t.parse().ok()).unwrap_or(0);
        let mut v = json!({"value": items.iter().skip(skip).take(top).cloned().collect::<Vec<_>>()});
        if skip + top < items.len() {
            let mut next = url::Url::parse("http://x").unwrap().join(path).unwrap();
            {
                let mut q = next.query_pairs_mut();
                for (k, value) in params.iter().filter(|(k, _)| k != "$skip") {
                    q.append_pair(k, value);
                }
                q.append_pair("$skip", &(skip + top).to_string());
            }
            v["@odata.nextLink"] = format!("{{base}}{}?{}", next.path(), next.query().unwrap()).into();
        }
        v
    }

    pub fn respond(&mut self, r: &Request, base: &str) -> Answer {
        let (path, query) = r.path.split_once('?').unwrap_or((&r.path, ""));
        let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        let param = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let ok = |v: Value| answer(200, v);
        let missing = || answer(404, json!({"error": {"code": "ErrorItemNotFound", "message": "Not found"}}));
        if let Some(n) = path.strip_prefix("/upload-session/") {
            let upload = &mut self.uploads[n.parse::<usize>().unwrap()];
            upload.2.extend_from_slice(&r.body);
            return answer(if upload.2.len() >= upload.3 { 201 } else { 200 }, json!({}));
        }
        let Some(path) = path.strip_prefix("/v1.0") else { return missing() };
        // Each segment as the server reads it, percent-decoded.
        let decoded: Vec<String> = path
            .trim_start_matches('/')
            .split('/')
            .map(|p| url::form_urlencoded::parse(format!("x={}", p.replace('+', "%2B")).as_bytes()).next().unwrap().1.into_owned())
            .collect();
        let parts: Vec<&str> = decoded.iter().map(String::as_str).collect();
        let fix = |v: Value| -> Answer {
            // Next links name the server's own address.
            let text = v.to_string().replace("{base}", base);
            (200, "application/json".into(), text.into_bytes())
        };
        match (r.method.as_str(), parts.as_slice()) {
            ("POST", ["$batch"]) => {
                self.batches += 1;
                let mut responses = Vec::new();
                for q in r.json()["requests"].as_array().unwrap() {
                    let inner = Request {
                        method: q["method"].as_str().unwrap().into(),
                        path: format!("/v1.0{}", q["url"].as_str().unwrap()),
                        content_type: "application/json".into(),
                        body: if q["body"].is_null() { Vec::new() } else { q["body"].to_string().into_bytes() },
                        authorization: String::new(),
                    };
                    let (status, _, body) = self.respond(&inner, base);
                    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    responses.push(json!({"id": q["id"], "status": status, "body": body}));
                }
                ok(json!({"responses": responses}))
            }
            ("GET", ["me"]) => ok(json!({"mail": "qa@example.com", "displayName": "QA"})),
            ("GET", ["me", "mailFolders"]) => {
                let items = self.folders.iter().filter(|f| f.parent == ROOT).map(|f| self.folder_json(f)).collect();
                fix(self.page(items, &params, &r.path))
            }
            ("POST", ["me", "mailFolders"]) => {
                let id = self.folder(r.json()["displayName"].as_str().unwrap(), ROOT);
                ok(json!({"id": id}))
            }
            ("GET", ["me", "mailFolders", key, "childFolders"]) => {
                let Some(id) = self.resolve(key) else { return missing() };
                let items = self.folders.iter().filter(|f| f.parent == id).map(|f| self.folder_json(f)).collect();
                fix(self.page(items, &params, &r.path))
            }
            ("POST", ["me", "mailFolders", key, "childFolders"]) => {
                let Some(parent) = self.resolve(key) else { return missing() };
                let id = self.folder(r.json()["displayName"].as_str().unwrap(), &parent);
                ok(json!({"id": id}))
            }
            ("GET", ["me", "mailFolders", key]) => match self.resolve(key) {
                Some(id) if id != ROOT => {
                    let f = self.folders.iter().find(|f| f.id == id).unwrap().clone();
                    let mut v = self.folder_json(&f);
                    // The folder's size, as an extended property asked for.
                    if param("$expand").is_some_and(|e| e.contains("singleValueExtendedProperties") && e.contains("0x0E08")) {
                        let size: usize = self.messages.iter().filter(|m| m.folder == f.id).map(|m| m.raw.len()).sum();
                        v["singleValueExtendedProperties"] = json!([{"id": "Long 0xe08", "value": size.to_string()}]);
                    }
                    ok(v)
                }
                _ => missing(),
            },
            ("PATCH", ["me", "mailFolders", key]) => {
                let Some(id) = self.resolve(key) else { return missing() };
                let name = r.json()["displayName"].as_str().unwrap().to_owned();
                self.folders.iter_mut().find(|f| f.id == id).unwrap().name = name;
                ok(json!({"id": id}))
            }
            ("DELETE", ["me", "mailFolders", key]) => {
                let Some(id) = self.resolve(key) else { return missing() };
                self.folders.retain(|f| f.id != id);
                for m in self.messages.iter_mut().filter(|m| m.folder == id) {
                    m.folder = "f-trash".into();
                }
                answer(204, Value::Null)
            }
            ("GET", ["me", "mailFolders", _, "messages"]) | ("GET", ["me", "messages"]) => {
                let folder = match parts.as_slice() {
                    [_, _, key, _] => match self.resolve(key) {
                        Some(f) => Some(f),
                        None => return missing(),
                    },
                    _ => None,
                };
                let filter = param("$filter").unwrap_or_default();
                let search = param("$search").map(|s| s.trim_matches('"').to_lowercase());
                let mut found: Vec<&FakeMessage> = self
                    .messages
                    .iter()
                    .filter(|m| folder.as_ref().is_none_or(|f| *f == m.folder || f == ROOT))
                    .filter(|m| !filter.contains("isRead eq false") || !m.read)
                    .filter(|m| !filter.contains("flagStatus eq 'flagged'") || m.flagged)
                    .filter(|m| search.as_ref().is_none_or(|s| m.subject.to_lowercase().contains(s)))
                    .collect();
                found.sort_by_key(|m| std::cmp::Reverse(m.received));
                let items = found.iter().map(|m| self.message_json(m)).collect();
                fix(self.page(items, &params, &r.path))
            }
            ("POST", ["me", "mailFolders", _, "messages"]) | ("POST", ["me", "messages"]) => {
                let folder = match parts.as_slice() {
                    [_, _, key, _] => self.resolve(key).unwrap(),
                    _ => "f-drafts".into(),
                };
                // Made from its parts (JSON): a draft unless its flags say
                // it was delivered.
                if r.content_type.starts_with("application/json") {
                    let v = r.json();
                    let delivered = v["singleValueExtendedProperties"].as_array().into_iter().flatten().any(|p| {
                        p["id"] == "Integer 0x0E07" && p["value"].as_str().and_then(|x| x.parse::<i64>().ok()).is_some_and(|f| f & 8 == 0)
                    });
                    let subject = v["subject"].as_str().unwrap_or_default().to_owned();
                    let raw = format!(
                        "From: {}\r\nSubject: {subject}\r\nMessage-ID: {}\r\n\r\n{}\r\n",
                        v["from"]["emailAddress"]["address"].as_str().unwrap_or_default(),
                        v["internetMessageId"].as_str().unwrap_or_default(),
                        v["body"]["content"].as_str().unwrap_or_default()
                    );
                    let id = self.id("AAMk");
                    self.messages.push(FakeMessage {
                        id: id.clone(), folder, raw: raw.into_bytes(), subject,
                        read: v["isRead"].as_bool().unwrap_or(false),
                        flagged: v["flag"]["flagStatus"] == "flagged", categories: Vec::new(),
                        received: chrono::Utc::now().timestamp(), draft: !delivered, verb: None,
                    });
                    self.attachments.push((id.clone(), v["attachments"].as_array().map_or(0, |a| a.len())));
                    return ok(json!({"id": id}));
                }
                assert_eq!(r.content_type, "text/plain");
                let raw = base64::engine::general_purpose::STANDARD.decode(&r.body).unwrap();
                let subject = mailparse::parse_mail(&raw).unwrap().headers.iter().find(|h| h.get_key() == "Subject").map(|h| h.get_value()).unwrap_or_default();
                let id = self.id("AAMk");
                self.messages.push(FakeMessage {
                    id: id.clone(), folder, raw, subject, read: true, flagged: false, categories: Vec::new(),
                    received: chrono::Utc::now().timestamp(), draft: true, verb: None,
                });
                ok(json!({"id": id}))
            }
            ("GET", ["me", "messages", id]) => match self.message(id) {
                Some(m) => ok(self.message_json(m)),
                None => missing(),
            },
            ("GET", ["me", "messages", id, "$value"]) => match self.message(id) {
                Some(m) => (200, "message/rfc822".into(), m.raw.clone()),
                None => missing(),
            },
            ("PATCH", ["me", "messages", id]) => {
                let v = r.json();
                let Some(m) = self.messages.iter_mut().find(|m| m.id == *id) else { return missing() };
                if let Some(read) = v["isRead"].as_bool() {
                    m.read = read;
                }
                if let Some(flag) = v["flag"]["flagStatus"].as_str() {
                    m.flagged = flag == "flagged";
                }
                if let Some(c) = v["categories"].as_array() {
                    m.categories = c.iter().filter_map(Value::as_str).map(str::to_owned).collect();
                }
                let mut deferred = None;
                for p in v["singleValueExtendedProperties"].as_array().into_iter().flatten() {
                    if p["id"] == "SystemTime 0x3FEF" {
                        deferred = p["value"].as_str().map(str::to_owned);
                    } else {
                        m.verb = p["value"].as_str().and_then(|v| v.parse().ok());
                    }
                }
                if let Some(at) = deferred {
                    self.deferred.push((id.to_string(), at));
                }
                ok(json!({"id": id}))
            }
            ("DELETE", ["me", "messages", id]) | ("POST", ["me", "messages", id, "permanentDelete"]) => {
                let before = self.messages.len();
                self.messages.retain(|m| m.id != *id);
                if before == self.messages.len() { missing() } else { answer(204, Value::Null) }
            }
            ("POST", ["me", "messages", id, "move"]) => {
                let Some(target) = self.resolve(r.json()["destinationId"].as_str().unwrap()) else { return missing() };
                let Some(m) = self.messages.iter_mut().find(|m| m.id == *id) else { return missing() };
                // Immutable ids: the same after the move.
                m.folder = target;
                ok(json!({"id": id}))
            }
            ("POST", ["me", "messages", id, "send"]) => {
                let Some(i) = self.messages.iter().position(|m| m.id == *id && m.draft) else { return missing() };
                let m = self.messages.remove(i);
                let at = self.deferred.iter().find(|(d, _)| d == id).map(|(_, t)| t.clone());
                let files: Vec<String> = self.uploads.iter().filter(|u| u.0 == *id).map(|u| u.1.clone()).collect();
                let mut raw = m.raw;
                for name in files {
                    raw.extend_from_slice(format!("\r\n[attached {name}]").as_bytes());
                }
                self.sent_drafts.push((raw, at));
                answer(202, Value::Null)
            }
            ("POST", ["me", "messages", id, "attachments", "createUploadSession"]) => {
                let item = r.json()["AttachmentItem"].clone();
                let n = self.uploads.len();
                self.uploads.push((id.to_string(), item["name"].as_str().unwrap().into(), Vec::new(), item["size"].as_u64().unwrap() as usize));
                ok(json!({"uploadUrl": format!("{base}/upload-session/{n}")}))
            }
            ("POST", ["me", "sendMail"]) => {
                assert_eq!(r.content_type, "text/plain");
                let raw = base64::engine::general_purpose::STANDARD.decode(&r.body).unwrap();
                self.sent.push(raw);
                answer(202, Value::Null)
            }
            ("GET", ["me", "mailFolders", "inbox", "messageRules"]) => ok(json!({"value": self.rules})),
            ("POST", ["me", "mailFolders", "inbox", "messageRules"]) => {
                let mut rule = r.json();
                rule["id"] = self.id("rule").into();
                self.rules.push(rule.clone());
                ok(rule)
            }
            ("PATCH", ["me", "mailFolders", "inbox", "messageRules", id]) => {
                let v = r.json();
                let Some(rule) = self.rules.iter_mut().find(|x| x["id"] == *id) else { return missing() };
                for (k, value) in v.as_object().unwrap() {
                    rule[k] = value.clone();
                }
                ok(rule.clone())
            }
            ("DELETE", ["me", "mailFolders", "inbox", "messageRules", id]) => {
                self.rules.retain(|x| x["id"] != *id);
                answer(204, Value::Null)
            }
            ("GET", ["me", "mailboxSettings", "automaticRepliesSetting"]) => ok(self.replies.clone()),
            ("PATCH", ["me", "mailboxSettings"]) => {
                self.replies = r.json()["automaticRepliesSetting"].clone();
                ok(Value::Null)
            }
            _ => self.respond_dav(r, &parts, &params, base),
        }
    }
    /// Calendars, contacts and people: see `graph/dav.rs`.
    fn respond_dav(&mut self, r: &Request, parts: &[&str], params: &[(String, String)], base: &str) -> Answer {
        super::dav::tests::respond(self, r, parts, params, base)
    }
}

/// The fake, serving.
pub(crate) fn serve(fake: Fake) -> (test_server::Server, Arc<Mutex<Fake>>) {
    let fake = Arc::new(Mutex::new(fake));
    let shared = fake.clone();
    let base = Arc::new(Mutex::new(String::new()));
    let seen_base = base.clone();
    let server = test_server::start(move |r| {
        let base = seen_base.lock().unwrap().clone();
        shared.lock().unwrap().respond(r, &base)
    });
    *base.lock().unwrap() = server.base.clone();
    (server, fake)
}

fn tls() -> native_tls::TlsConnector {
    native_tls::TlsConnector::new().unwrap()
}

#[test]
fn folders_messages_and_batches() -> Result<()> {
    let mut fake = Fake::new();
    let projects = fake.folder("Projects", "f-inbox");
    fake.folder("2026", &projects);
    let first = fake.add("Budget", "inbox", 5);
    let second = fake.add("Agenda", "inbox", 10);
    let (server, fake) = serve(fake);
    let s = Session::open(&server.base, "token", tls())?;
    assert_eq!(s.me()?.0, "qa@example.com");
    let folders = s.reload_folders()?;
    let nested = folders.iter().find(|f| f.name == "2026").unwrap();
    assert_eq!((nested.path.as_str(), nested.depth), ("Inbox/Projects/2026", 2));
    assert_eq!(folders.iter().find(|f| f.name == "Inbox").unwrap().well_known.as_deref(), Some("inbox"));
    assert_eq!(folders.iter().find(|f| f.name == "Outbox").unwrap().well_known.as_deref(), Some("outbox"));
    // Newest first, a page at a time, the next page at the server's address.
    let (page, next) = s.list("inbox", None, None, 1)?;
    assert_eq!(page[0].id, first);
    assert!(next.as_deref().is_some_and(|n| n.starts_with(&server.base)));
    let (page, next) = s.list("inbox", None, next.as_deref(), 1)?;
    assert_eq!((page[0].id.as_str(), next), (second.as_str(), None));
    let m = &page[0];
    assert_eq!(m.from.as_ref().unwrap().shown(), "\"Bo Berg\" <bo@example.se>");
    assert!(m.size > 0 && !m.read && m.received > 0);
    // Changed together; one gone meanwhile counts as done.
    s.update(&[first.clone(), "gone".into()], &json!({"isRead": true}))?;
    assert!(fake.lock().unwrap().message(&first).unwrap().read);
    s.move_to(std::slice::from_ref(&first), "archive")?;
    assert_eq!(fake.lock().unwrap().message(&first).unwrap().folder, "f-archive");
    assert_eq!(s.messages(&[first.clone(), "gone".into()])?.len(), 1);
    // The mail's size: its folders' sizes added up.
    let size: usize = fake.lock().unwrap().messages.iter().map(|m| m.raw.len()).sum();
    assert_eq!(s.mailbox_size()?, Some(size as u64));
    // Sent as MIME.
    s.send(b"To: a@example.org\r\nSubject: Hi\r\n\r\nHello\r\n")?;
    assert!(String::from_utf8_lossy(&fake.lock().unwrap().sent[0]).contains("Subject: Hi"));
    Ok(())
}

#[test]
fn large_messages_lose_their_files_to_upload_them_apart() -> Result<()> {
    let raw = b"From: a@example.org\r\nTo: b@example.org\r\nSubject: Files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"xx\"\r\n\r\n--xx\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--xx\r\nContent-Type: application/pdf; name=\"a.pdf\"\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERg==\r\n--xx--\r\n";
    let parsed = mailparse::parse_mail(raw)?;
    let (text, files) = split_attachments(raw, &parsed)?;
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("See attached.") && !text.contains("JVBERg=="), "{text}");
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].0.as_str(), files[0].1.as_str(), files[0].3.as_slice()), ("a.pdf", "application/pdf", &b"%PDF"[..]));
    Ok(())
}
