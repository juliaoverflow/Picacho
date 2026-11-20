// A JMAP server over loopback TLS, small but by the RFCs, exercises the
// production backend without a real account.
use super::protocol_tests::{backend, tls_fixture};
use super::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MESSAGE: &[u8] = b"From: Sophie Chen <sophie@example.com>\r\nTo: qa@example.com\r\nSubject: Over JMAP\r\nDate: Sat, 24 Oct 2026 10:00:00 +0000\r\nMessage-ID: <jmap-1@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello from the local JMAP fixture.\r\n";

#[derive(Clone)]
struct Message {
    id: String,
    blob: String,
    mailboxes: Vec<String>,
    keywords: Vec<String>,
    subject: String,
    received: String,
}

#[derive(Default)]
struct Server {
    port: u16,
    emails: Vec<Message>,
    blobs: HashMap<String, Vec<u8>>,
    state: u64,
    /// (state, id, destroyed) for each change.
    log: Vec<(u64, String, bool)>,
    /// Method names called, in order.
    calls: Vec<String>,
    /// Envelopes of submitted messages: (identity, from, recipients).
    sent: Vec<(String, String, Vec<String>)>,
    next: u64,
    /// Sieve scripts: (id, name, blob id, active).
    scripts: Vec<(String, String, String, bool)>,
    /// A method refused as Cyrus does, naming an argument.
    refuse: Option<&'static str>,
}

const AUTH: &str = "Basic cWFAZXhhbXBsZS5jb206Zml4dHVyZS1wYXNzd29yZA=="; // qa@example.com:fixture-password
/// An OAuth access token the server accepts too.
const TOKEN: &str = "Bearer fixture-token";

impl Server {
    fn touch(&mut self, id: &str, destroyed: bool) {
        self.state += 1;
        self.log.push((self.state, id.to_owned(), destroyed));
    }
    fn mailboxes() -> Value {
        json!([
            { "id": "i", "name": "Inbox", "role": "inbox", "parentId": null },
            { "id": "s", "name": "Sent", "role": "sent", "parentId": null },
            { "id": "d", "name": "Drafts", "role": "drafts", "parentId": null },
            { "id": "t", "name": "Trash", "role": "trash", "parentId": null },
            { "id": "w", "name": "Work", "role": null, "parentId": null },
        ])
    }
    fn email_json(m: &Message) -> Value {
        let keywords: serde_json::Map<String, Value> = m
            .keywords
            .iter()
            .map(|k| (k.clone(), json!(true)))
            .collect();
        let mailboxes: serde_json::Map<String, Value> = m
            .mailboxes
            .iter()
            .map(|k| (k.clone(), json!(true)))
            .collect();
        json!({
            "id": m.id, "blobId": m.blob, "threadId": format!("T{}", m.id),
            "mailboxIds": mailboxes, "keywords": keywords, "size": 300,
            "receivedAt": m.received, "subject": m.subject,
            "from": [{ "name": "Sophie Chen", "email": "sophie@example.com" }],
            "to": [{ "name": null, "email": "qa@example.com" }],
            "messageId": [format!("{}@example.com", m.id)],
            "preview": "Hello from the local JMAP fixture.",
            "hasAttachment": false,
            "header:List-Unsubscribe:asURLs": ["https://example.com/unsubscribe"],
            "header:List-Unsubscribe-Post:asText": "List-Unsubscribe=One-Click",
        })
    }
    fn matches(m: &Message, f: &Value) -> bool {
        if f.is_null() {
            return true;
        }
        if let Some(op) = f["operator"].as_str() {
            let mut each = f["conditions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| Self::matches(m, c));
            return match op {
                "AND" => each.all(|x| x),
                "OR" => each.any(|x| x),
                _ => !each.any(|x| x),
            };
        }
        let mut ok = true;
        if let Some(id) = f["inMailbox"].as_str() {
            ok &= m.mailboxes.iter().any(|b| b == id);
        }
        if let Some(ids) = f["inMailboxOtherThan"].as_array() {
            ok &= m.mailboxes.iter().any(|b| !ids.iter().any(|i| i == b));
        }
        if let Some(k) = f["hasKeyword"].as_str() {
            ok &= m.keywords.iter().any(|x| x == k);
        }
        if let Some(k) = f["notKeyword"].as_str() {
            ok &= !m.keywords.iter().any(|x| x == k);
        }
        if let Some(t) = f["text"].as_str() {
            ok &= m.subject.to_lowercase().contains(t);
        }
        ok
    }
    fn method(&mut self, name: &str, args: &Value, earlier: &[Value]) -> Vec<(String, Value)> {
        self.calls.push(name.to_owned());
        if self.refuse == Some(name) {
            return vec![(
                "error".into(),
                json!({ "type": "invalidArguments", "arguments": ["ids"] }),
            )];
        }
        let ids_of = |args: &Value| -> Option<Vec<String>> {
            if let Some(r) = args.get("#ids") {
                let i: usize = r["resultOf"].as_str().unwrap()[1..].parse().unwrap();
                return Some(
                    earlier[i]["ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect(),
                );
            }
            args["ids"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        };
        let state = self.state.to_string();
        match name {
            "Mailbox/get" => vec![(
                name.into(),
                json!({ "list": Self::mailboxes(), "state": "m1" }),
            )],
            "Email/query" => {
                let mut found: Vec<&Message> = self
                    .emails
                    .iter()
                    .filter(|m| Self::matches(m, &args["filter"]))
                    .collect();
                found.sort_by(|a, b| b.received.cmp(&a.received));
                let total = found.len();
                let position = args["position"].as_u64().unwrap_or(0) as usize;
                let limit = args["limit"].as_u64().unwrap_or(100) as usize;
                let ids: Vec<&str> = found
                    .iter()
                    .skip(position)
                    .take(limit)
                    .map(|m| m.id.as_str())
                    .collect();
                vec![(
                    name.into(),
                    json!({ "ids": ids, "total": total, "position": position }),
                )]
            }
            "Email/get" => {
                // Header forms a header may be asked for, as strict servers
                // (Fastmail's Cyrus) check them: RFC 8621 §4.1.2.
                for (i, p) in args["properties"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let p = p.as_str().unwrap();
                    if let Some(rest) = p.strip_prefix("header:")
                        && let Some((header, form)) = rest.rsplit_once(':')
                        && !header_form_allowed(header, form)
                    {
                        return vec![(
                            "error".into(),
                            json!({ "type": "invalidArguments", "arguments": [format!("properties[{i}]")] }),
                        )];
                    }
                }
                let wanted = ids_of(args);
                let list: Vec<Value> = self
                    .emails
                    .iter()
                    .filter(|m| wanted.as_ref().is_none_or(|w| w.contains(&m.id)))
                    .map(Self::email_json)
                    .collect();
                vec![(
                    name.into(),
                    json!({ "list": list, "state": state, "notFound": [] }),
                )]
            }
            "Email/changes" => {
                let since: u64 = args["sinceState"].as_str().unwrap().parse().unwrap();
                let mut updated = Vec::new();
                let mut destroyed = Vec::new();
                for (s, id, gone) in &self.log {
                    if *s > since {
                        if *gone {
                            destroyed.push(id.clone());
                        } else {
                            updated.push(id.clone());
                        }
                    }
                }
                vec![(
                    name.into(),
                    json!({ "oldState": since.to_string(), "newState": state, "hasMoreChanges": false,
                            "created": [], "updated": updated, "destroyed": destroyed }),
                )]
            }
            "Email/set" => {
                let mut updated = serde_json::Map::new();
                for (id, patch) in args["update"].as_object().into_iter().flatten() {
                    let m = self.emails.iter_mut().find(|m| &m.id == id).unwrap();
                    for (path, v) in patch.as_object().unwrap() {
                        if let Some(k) = path.strip_prefix("keywords/") {
                            m.keywords.retain(|x| x != k);
                            if v == &json!(true) {
                                m.keywords.push(k.to_owned());
                            }
                        } else if let Some(b) = path.strip_prefix("mailboxIds/") {
                            m.mailboxes.retain(|x| x != b);
                            if v == &json!(true) {
                                m.mailboxes.push(b.to_owned());
                            }
                        } else if path == "mailboxIds" {
                            m.mailboxes = v.as_object().unwrap().keys().cloned().collect();
                        }
                    }
                    updated.insert(id.clone(), Value::Null);
                    self.touch(id, false);
                }
                let mut destroyed = Vec::new();
                for id in args["destroy"].as_array().into_iter().flatten() {
                    let id = id.as_str().unwrap().to_owned();
                    self.emails.retain(|m| m.id != id);
                    self.touch(&id, true);
                    destroyed.push(id);
                }
                vec![(
                    name.into(),
                    json!({ "updated": updated, "destroyed": destroyed, "newState": self.state.to_string() }),
                )]
            }
            "Email/import" => {
                let mut created = serde_json::Map::new();
                for (key, e) in args["emails"].as_object().unwrap() {
                    self.next += 1;
                    let id = format!("N{}", self.next);
                    self.emails.push(Message {
                        id: id.clone(),
                        blob: e["blobId"].as_str().unwrap().to_owned(),
                        mailboxes: e["mailboxIds"]
                            .as_object()
                            .unwrap()
                            .keys()
                            .cloned()
                            .collect(),
                        keywords: e["keywords"].as_object().unwrap().keys().cloned().collect(),
                        subject: "Imported".into(),
                        received: "2026-10-25T10:00:00Z".into(),
                    });
                    self.touch(&id, false);
                    created.insert(key.clone(), json!({ "id": id, "blobId": e["blobId"] }));
                }
                vec![(name.into(), json!({ "created": created }))]
            }
            "Identity/get" => vec![(
                name.into(),
                json!({ "list": [{ "id": "id1", "name": "QA", "email": "qa@example.com" }] }),
            )],
            "EmailSubmission/set" => {
                let s = &args["create"]["s"];
                let email_id = s["emailId"].as_str().unwrap();
                // A back-reference to the import before it.
                let email_id = if let Some(key) = email_id.strip_prefix('#') {
                    earlier
                        .iter()
                        .find_map(|v| v["created"][key]["id"].as_str())
                        .unwrap()
                        .to_owned()
                } else {
                    email_id.to_owned()
                };
                self.sent.push((
                    s["identityId"].as_str().unwrap().to_owned(),
                    s["envelope"]["mailFrom"]["email"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    s["envelope"]["rcptTo"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| r["email"].as_str().unwrap().to_owned())
                        .collect(),
                ));
                let mut out = vec![(
                    name.to_owned(),
                    json!({ "created": { "s": { "id": "sub1" } } }),
                )];
                // onSuccessUpdateEmail runs as an implicit Email/set.
                if let Some(patch) = args["onSuccessUpdateEmail"]["#s"].as_object() {
                    let update = json!({ "update": { email_id: patch } });
                    out.extend(self.method("Email/set", &update, earlier));
                }
                out
            }
            "SieveScript/get" => {
                let list: Vec<Value> = self
                    .scripts
                    .iter()
                    .map(|(id, name, blob, active)| {
                        json!({ "id": id, "name": name, "blobId": blob, "isActive": active })
                    })
                    .collect();
                vec![(name.into(), json!({ "list": list, "state": "x" }))]
            }
            "SieveScript/validate" => {
                let text = String::from_utf8_lossy(&self.blobs[args["blobId"].as_str().unwrap()])
                    .into_owned();
                let error = if text.contains("require") {
                    Value::Null
                } else {
                    json!({ "type": "invalidSieve", "description": "no require" })
                };
                vec![(name.into(), json!({ "error": error }))]
            }
            "SieveScript/set" => {
                let mut created = serde_json::Map::new();
                for (key, s) in args["create"].as_object().into_iter().flatten() {
                    self.next += 1;
                    let id = format!("S{}", self.next);
                    self.scripts.push((
                        id.clone(),
                        s["name"].as_str().unwrap().to_owned(),
                        s["blobId"].as_str().unwrap().to_owned(),
                        false,
                    ));
                    created.insert(key.clone(), json!({ "id": id }));
                }
                for (id, s) in args["update"].as_object().into_iter().flatten() {
                    let script = self.scripts.iter_mut().find(|x| &x.0 == id).unwrap();
                    script.2 = s["blobId"].as_str().unwrap().to_owned();
                }
                if let Some(target) = args["onSuccessActivateScript"].as_str() {
                    let target = match target.strip_prefix('#') {
                        Some(key) => created[key]["id"].as_str().unwrap().to_owned(),
                        None => target.to_owned(),
                    };
                    for x in &mut self.scripts {
                        x.3 = x.0 == target;
                    }
                }
                if args["onSuccessDeactivateScript"] == json!(true) {
                    for x in &mut self.scripts {
                        x.3 = false;
                    }
                }
                vec![(name.into(), json!({ "created": created, "updated": {} }))]
            }
            "Quota/get" => vec![(
                name.into(),
                json!({ "list": [{ "id": "q", "resourceType": "octets", "scope": "account",
                                   "used": 2048, "hardLimit": 1048576 }] }),
            )],
            _ => vec![("error".into(), json!({ "type": "unknownMethod" }))],
        }
    }
    fn respond(&mut self, method: &str, path: &str, auth: &str, body: &[u8]) -> (u16, Vec<u8>) {
        if auth != AUTH && auth != TOKEN {
            return (401, b"{}".to_vec());
        }
        let base = format!("https://localhost:{}", self.port);
        if method == "GET" && path == "/.well-known/jmap" {
            return (
                200,
                json!({
                    "capabilities": {
                        "urn:ietf:params:jmap:core": { "maxObjectsInGet": 100, "maxObjectsInSet": 100 },
                        "urn:ietf:params:jmap:mail": {},
                        "urn:ietf:params:jmap:submission": {},
                        "urn:ietf:params:jmap:quota": {},
                        "urn:ietf:params:jmap:sieve": {},
                    },
                    "accounts": { "u1": { "name": "qa@example.com", "accountCapabilities": {
                        "urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:submission": {},
                        "urn:ietf:params:jmap:sieve": {
                            "sieveExtensions": ["fileinto", "vacation", "imap4flags", "include"] } } } },
                    "primaryAccounts": { "urn:ietf:params:jmap:mail": "u1" },
                    "username": "qa@example.com",
                    "apiUrl": format!("{base}/api"),
                    "downloadUrl": format!("{base}/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
                    "uploadUrl": format!("{base}/upload/{{accountId}}/"),
                    "state": "s1",
                })
                .to_string()
                .into_bytes(),
            );
        }
        if method == "POST" && path == "/api" {
            let request: Value = serde_json::from_slice(body).unwrap();
            let mut responses = Vec::new();
            let mut earlier: Vec<Value> = Vec::new();
            for call in request["methodCalls"].as_array().unwrap() {
                let (name, args, id) = (call[0].as_str().unwrap(), &call[1], &call[2]);
                assert_eq!(args["accountId"], "u1", "{name} names the account");
                for (n, v) in self.method(name, args, &earlier) {
                    responses.push(json!([n, v, id]));
                    if n == name {
                        earlier.push(v);
                    }
                }
            }
            return (
                200,
                json!({ "methodResponses": responses, "sessionState": "s1" })
                    .to_string()
                    .into_bytes(),
            );
        }
        if method == "POST" && path.starts_with("/upload/u1/") {
            self.next += 1;
            let blob = format!("B{}", self.next);
            self.blobs.insert(blob.clone(), body.to_vec());
            return (
                201,
                json!({ "accountId": "u1", "blobId": blob, "type": "message/rfc822", "size": body.len() })
                    .to_string()
                    .into_bytes(),
            );
        }
        if method == "GET"
            && let Some(rest) = path.strip_prefix("/download/u1/")
        {
            let blob = rest.split('/').next().unwrap();
            return match self.blobs.get(blob) {
                Some(b) => (200, b.clone()),
                None => (404, Vec::new()),
            };
        }
        (404, Vec::new())
    }
}

/// Whether RFC 8621 (§4.1.2) lets a header be asked for in a form. Headers
/// RFC 5322 and RFC 2369 define take only their own forms; other headers
/// take any form.
fn header_form_allowed(header: &str, form: &str) -> bool {
    let h = header.to_ascii_lowercase();
    let form = form.trim_end_matches(":all");
    let addresses = [
        "from",
        "sender",
        "reply-to",
        "to",
        "cc",
        "bcc",
        "resent-from",
        "resent-sender",
        "resent-reply-to",
        "resent-to",
        "resent-cc",
        "resent-bcc",
    ];
    let urls = [
        "list-help",
        "list-unsubscribe",
        "list-subscribe",
        "list-post",
        "list-owner",
        "list-archive",
    ];
    let ids = [
        "message-id",
        "in-reply-to",
        "references",
        "resent-message-id",
    ];
    let dates = ["date", "resent-date"];
    let text = ["subject", "comments", "keywords", "list-id"];
    let known = |list: &[&str]| list.contains(&h.as_str());
    let defined = known(&addresses) || known(&urls) || known(&ids) || known(&dates) || known(&text);
    match form {
        "asRaw" => true,
        "asText" => !defined || known(&text),
        "asAddresses" | "asGroupedAddresses" => !defined || known(&addresses),
        "asMessageIds" => !defined || known(&ids),
        "asDate" => !defined || known(&dates),
        "asURLs" => !defined || known(&urls),
        // A header name with a colon in it, or no form: the raw value.
        _ => true,
    }
}

fn serve(
    listener: std::net::TcpListener,
    tls: native_tls::TlsAcceptor,
    server: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let socket = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(e) => panic!("{e}"),
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let Ok(stream) = tls.accept(socket) else {
            continue;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or_default().to_owned(),
            parts.next().unwrap_or_default().to_owned(),
        );
        let (mut length, mut auth) = (0usize, String::new());
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (key, value) = header.split_once(':').unwrap();
            match key.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap(),
                "authorization" => auth = value.trim().to_owned(),
                _ => {}
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let (status, reply) = server.lock().unwrap().respond(&method, &path, &auth, &body);
        let stream = reader.get_mut();
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        );
        let _ = stream.write_all(&reply);
        let _ = stream.flush();
        let _ = stream.shutdown();
    }
}

#[test]
fn jmap_sync_read_change_send_and_folders() -> Result<()> {
    let (cert, tls) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let server = Arc::new(Mutex::new(Server {
        port,
        state: 1,
        ..Default::default()
    }));
    {
        let mut s = server.lock().unwrap();
        s.blobs.insert("B-M1".into(), MESSAGE.to_vec());
        s.emails.push(Message {
            id: "M1".into(),
            blob: "B-M1".into(),
            mailboxes: vec!["i".into()],
            keywords: vec![],
            subject: "Over JMAP".into(),
            received: "2026-10-24T10:00:00Z".into(),
        });
        s.emails.push(Message {
            id: "M2".into(),
            blob: "B-M1".into(),
            mailboxes: vec!["w".into()],
            keywords: vec!["$seen".into(), "$flagged".into(), "later".into()],
            subject: "Filed away".into(),
            received: "2026-10-23T10:00:00Z".into(),
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (server, stop) = (server.clone(), stop.clone());
        std::thread::spawn(move || serve(listener, tls, server, stop))
    };
    let result = (|| -> Result<()> {
        let (b, mut a, path) = backend(&cert)?;
        let _cleanup = Cleanup(path);
        a.provider = Provider::Jmap;
        a.incoming_port = port;
        b.check_jmap(&a, &b.test_secret.clone().unwrap())?;

        // The inbox lists, with what the listing says.
        let inbox = b.sync(&a, Folder::Inbox, 50)?;
        assert_eq!(inbox.len(), 1);
        let m = &inbox[0];
        assert_eq!(m.id, format!("{}:jmap:M1", a.id));
        assert_eq!(
            (m.sender.as_str(), m.subject.as_str()),
            ("Sophie Chen", "Over JMAP")
        );
        assert!(m.unread && !m.starred);
        assert_eq!(m.message_id, "<M1@example.com>");
        // Subscription details, asked for in the forms RFC 8621 allows.
        assert_eq!(m.list_unsubscribe, "<https://example.com/unsubscribe>");
        assert!(m.one_click);
        // The account's own folders are kept.
        assert_eq!(b.store.folders(&a.id)[0].name, "Work");
        // Starred is a query: M2, with its label.
        let starred = b.sync(&a, Folder::Starred, 50)?;
        assert_eq!(starred.len(), 1);
        assert_eq!(starred[0].folder, Folder::Other);
        assert_eq!(starred[0].remote_folder, "Work");
        assert_eq!(starred[0].labels(), ["later"]);

        // Opening downloads the source.
        let read = b.read(&a, m)?;
        assert!(read.body.as_deref().unwrap().contains("local JMAP fixture"));
        assert_eq!(b.store.raw(&m.id)?.as_deref(), Some(MESSAGE));

        // Changes reach the server.
        b.change_many(&a, std::slice::from_ref(m), Change::Read(true))?;
        b.change_many(
            &a,
            std::slice::from_ref(m),
            Change::Label("Later".into(), true),
        )?;
        {
            let s = server.lock().unwrap();
            let kw = &s.emails.iter().find(|e| e.id == "M1").unwrap().keywords;
            assert!(kw.contains(&"$seen".to_owned()) && kw.contains(&"later".to_owned()));
        }
        // Nothing changed since: one Email/changes, no listing.
        let before = server.lock().unwrap().calls.len();
        b.sync(&a, Folder::Starred, 50)?;
        let calls = server.lock().unwrap().calls[before..].to_vec();
        assert!(
            calls.contains(&"Email/query".to_owned()),
            "M1 changed: {calls:?}"
        );
        let before = server.lock().unwrap().calls.len();
        b.sync(&a, Folder::Starred, 50)?;
        assert_eq!(server.lock().unwrap().calls[before..], ["Email/changes"]);

        // Moving keeps the row, in its new place.
        b.change_many(&a, std::slice::from_ref(m), Change::Move(Folder::Trash))?;
        assert_eq!(
            server
                .lock()
                .unwrap()
                .emails
                .iter()
                .find(|e| e.id == "M1")
                .unwrap()
                .mailboxes,
            ["t"]
        );
        assert_eq!(b.store.message(&m.id)?.unwrap().folder, Folder::Trash);
        assert!(b.sync(&a, Folder::Inbox, 50)?.is_empty());

        // Server search.
        let found = b.search_server(&a, &Folder::Archive.into(), "filed", 10);
        assert!(found.is_err(), "no Archive folder here");
        let found = b.search_server(&a, &Mailbox::Remote("Work".into()), "filed", 10)?;
        assert_eq!(found.total, 1);

        // Sending: added to Drafts, submitted, then filed in Sent.
        let draft = Draft {
            id: "d1".into(),
            account: a.id.clone(),
            to: "ana@example.org".into(),
            subject: "Hi".into(),
            body: "Hello".into(),
            ..Default::default()
        };
        b.send(&a, &draft)?;
        {
            let s = server.lock().unwrap();
            assert_eq!(
                s.sent,
                [(
                    "id1".to_owned(),
                    "qa@example.com".to_owned(),
                    vec!["ana@example.org".to_owned()]
                )]
            );
            let copy = s.emails.iter().find(|e| e.subject == "Imported").unwrap();
            assert_eq!(copy.mailboxes, ["s"]);
            assert!(!copy.keywords.contains(&"$draft".to_owned()));
        }

        // Quota, and deleting for good.
        let q = b.quota(&a)?.unwrap();
        assert_eq!((q.used_kib, q.limit_kib), (2, 1024));
        let trashed = b.store.message(&m.id)?.unwrap();
        b.delete_forever(&a, &[trashed])?;
        assert!(server.lock().unwrap().emails.iter().all(|e| e.id != "M1"));
        assert!(b.store.message(&m.id)?.is_none());
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
    result
}

#[test]
fn jmap_refused_sign_in_is_an_access_failure() -> Result<()> {
    let (cert, tls) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let server = Arc::new(Mutex::new(Server {
        port,
        ..Default::default()
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (server, stop) = (server.clone(), stop.clone());
        std::thread::spawn(move || serve(listener, tls, server, stop))
    };
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let wrong = Secret {
        password: "wrong".into(),
        ..Default::default()
    };
    let error = b.check_jmap(&a, &wrong).unwrap_err();
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
    assert!(error.downcast_ref::<AccessFailure>().is_some(), "{error:#}");
    Ok(())
}

/// Starts the fixture server; returns its port and a handle that stops it.
/// The fixture's certificate, its port, its state and its running thread.
type Started = (Vec<u8>, u16, Arc<Mutex<Server>>, Running);
fn start(server: Server) -> Result<Started> {
    let (cert, tls) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let server = Arc::new(Mutex::new(Server { port, ..server }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (server, stop) = (server.clone(), stop.clone());
        std::thread::spawn(move || serve(listener, tls, server, stop))
    };
    Ok((cert, port, server, Running(stop, Some(thread))))
}
struct Running(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        if let Some(t) = self.1.take() {
            let _ = t.join();
        }
    }
}

#[test]
fn jmap_sieve_filters_and_vacation() -> Result<()> {
    let mut fixture = Server::default();
    fixture
        .blobs
        .insert("B-vendor".into(), b"require \"fileinto\";\r\n".to_vec());
    fixture
        .scripts
        .push(("V".into(), "vendor".into(), "B-vendor".into(), true));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;

    // Another app's script runs; Picacho's would include it.
    let state = b.filters(&a)?;
    assert_eq!(state.other_active.as_deref(), Some("vendor"));
    assert_eq!(state.filters.include.as_deref(), Some("vendor"));
    assert!(state.has("vacation") && !state.active);

    let mut filters = state.filters.clone();
    filters.rules.push(crate::sync::sieve::Rule {
        field: crate::sync::sieve::Field::From,
        value: "news@shop.example".into(),
        act: crate::sync::sieve::Act::Folder("Work".into()),
        stop: true,
    });
    let saved = b.save_filters(&a, &filters)?;
    assert!(saved.active);
    {
        let s = server.lock().unwrap();
        let ours = s.scripts.iter().find(|x| x.1 == "picacho").unwrap();
        assert!(ours.3, "Picacho's script is the active one");
        let text = String::from_utf8_lossy(&s.blobs[&ours.2]).into_owned();
        assert!(text.contains("fileinto \"Work\";"), "{text}");
        assert!(text.contains("include :personal \"vendor\";"));
    }
    // Read back as saved.
    let again = b.filters(&a)?;
    assert!(again.active);
    assert_eq!(again.filters, filters);

    // Saved again: the same script is replaced, not a second one made.
    b.save_filters(&a, &filters)?;
    assert_eq!(server.lock().unwrap().scripts.len(), 2);

    // No rules: back to the script it included.
    let mut none = filters.clone();
    none.rules.clear();
    let off = b.save_filters(&a, &none)?;
    assert!(!off.active);
    let s = server.lock().unwrap();
    assert!(s.scripts.iter().find(|x| x.1 == "vendor").unwrap().3);
    Ok(())
}

#[test]
fn jmap_refusals_say_which_method() -> Result<()> {
    let (cert, port, _server, _running) = start(Server {
        refuse: Some("Quota/get"),
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let error = format!("{:#}", b.quota(&a).unwrap_err());
    assert!(
        error.contains("refused Quota/get (invalidArguments: ids)"),
        "{error}"
    );
    Ok(())
}

#[test]
fn jmap_browser_sign_in_uses_the_access_token() -> Result<()> {
    let (cert, port, _server, _running) = start(Server::default())?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    a.oauth = true;
    let token = Secret {
        access_token: "fixture-token".into(),
        expires_at: chrono::Utc::now().timestamp() + 3600,
        ..Default::default()
    };
    b.check_jmap(&a, &token)?;
    assert_eq!(b.jmap_username(&a, &token)?, "qa@example.com");
    // The password isn't tried in its place.
    let expired = Secret {
        password: "fixture-password".into(),
        access_token: "old".into(),
        ..Default::default()
    };
    let error = b.check_jmap(&a, &expired).unwrap_err();
    assert!(error.downcast_ref::<AccessFailure>().is_some(), "{error:#}");
    Ok(())
}

struct Cleanup(std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
