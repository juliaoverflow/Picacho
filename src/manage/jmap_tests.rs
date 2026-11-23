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
    /// Newer features: submission extensions (DSN, REQUIRETLS,
    /// FUTURERELEASE), an away reply in place of Sieve, a shared account
    /// (`u2`), search snippets.
    modern: bool,
    /// Each submission's whole envelope.
    envelopes: Vec<Value>,
    /// The away reply (VacationResponse's singleton).
    vacation: Value,
    /// The session state responses name; changing it makes the client
    /// read the session again.
    session_state: String,
    /// How many times the session resource was read.
    session_reads: usize,
    /// Email/query states and Email/queryChanges: each query's results by
    /// the state it was answered in.
    query_changes: bool,
    queries: HashMap<String, Vec<String>>,
    /// Calendar events and contact cards (JSON objects by id), and
    /// submissions' undo states.
    events: Vec<Value>,
    cards: Vec<Value>,
    undo: HashMap<String, String>,
    /// Calendars' and contacts' states (`CalendarEvent/changes`,
    /// `ContactCard/changes`): each change with its state, (kind, id,
    /// destroyed), kind `e` or `k`. Off: those methods are unknown.
    dav_changes: bool,
    dav_state: u64,
    dav_log: Vec<(u64, char, String, bool)>,
    /// The most results one /query returns, as a server's own limit.
    query_limit: usize,
    /// Blob ids downloaded, in order.
    downloads: Vec<String>,
}

const AUTH: &str = "Basic cWFAZXhhbXBsZS5jb206Zml4dHVyZS1wYXNzd29yZA=="; // qa@example.com:fixture-password
/// An OAuth access token the server accepts too.
const TOKEN: &str = "Bearer fixture-token";

impl Server {
    fn state_name(&self) -> String {
        if self.session_state.is_empty() {
            "s1".into()
        } else {
            self.session_state.clone()
        }
    }
    /// The session resource with the newer features (`modern`).
    fn modern_session(&self, base: &str) -> Value {
        let account = json!({
            "urn:ietf:params:jmap:mail": {},
            "urn:ietf:params:jmap:submission": {
                "maxDelayedSend": 86400,
                "submissionExtensions": { "DSN": [], "REQUIRETLS": [], "FUTURERELEASE": ["86400"] },
            },
            "urn:ietf:params:jmap:vacationresponse": {},
        });
        json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": {
                    "maxObjectsInGet": 100, "maxObjectsInSet": 100,
                    "maxCallsInRequest": 2, "maxSizeRequest": 1_000_000,
                },
                "urn:ietf:params:jmap:mail": {},
                "urn:ietf:params:jmap:submission": {},
                "urn:ietf:params:jmap:vacationresponse": {},
                "urn:ietf:params:jmap:calendars": {},
                "urn:ietf:params:jmap:contacts": {},
            },
            "accounts": {
                "u1": { "name": "qa@example.com", "isPersonal": true, "accountCapabilities": account },
                "u2": { "name": "Team", "isPersonal": false, "accountCapabilities": account },
            },
            "primaryAccounts": {
                "urn:ietf:params:jmap:mail": "u1",
                "urn:ietf:params:jmap:calendars": "u1",
                "urn:ietf:params:jmap:contacts": "u1",
            },
            "username": "qa@example.com",
            "apiUrl": format!("{base}/api"),
            "downloadUrl": format!("{base}/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
            "uploadUrl": format!("{base}/upload/{{accountId}}/"),
            "state": self.state_name(),
        })
    }
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
            "Mailbox/get" => {
                let all = Self::mailboxes();
                let list = match ids_of(args) {
                    Some(ids) => all
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|m| ids.iter().any(|i| m["id"] == i.as_str()))
                        .cloned()
                        .collect(),
                    None => all,
                };
                vec![(name.into(), json!({ "list": list, "state": "m1" }))]
            }
            // The mailboxes don't change here.
            "Mailbox/changes" if args["sinceState"] == "m1" => vec![(
                name.into(),
                json!({ "oldState": "m1", "newState": "m1", "hasMoreChanges": false,
                        "created": [], "updated": [], "destroyed": [] }),
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
                let mut answer = json!({ "ids": ids, "total": total, "position": position });
                if self.query_changes {
                    let all: Vec<String> = found.iter().map(|m| m.id.clone()).collect();
                    let q = format!("q{}", self.state);
                    self.queries.insert(q.clone(), all);
                    answer["queryState"] = json!(q);
                }
                vec![(name.into(), answer)]
            }
            "Email/queryChanges" if self.query_changes => {
                let since = args["sinceQueryState"].as_str().unwrap_or_default();
                let Some(before) = self.queries.get(since).cloned() else {
                    return vec![("error".into(), json!({ "type": "cannotCalculateChanges" }))];
                };
                let mut now: Vec<&Message> = self
                    .emails
                    .iter()
                    .filter(|m| Self::matches(m, &args["filter"]))
                    .collect();
                now.sort_by(|a, b| b.received.cmp(&a.received));
                let now: Vec<String> = now.iter().map(|m| m.id.clone()).collect();
                let removed: Vec<&String> = before.iter().filter(|id| !now.contains(id)).collect();
                let added: Vec<Value> = now
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| !before.contains(id))
                    .map(|(i, id)| json!({ "id": id, "index": i }))
                    .collect();
                let q = format!("q{}", self.state);
                self.queries.insert(q.clone(), now.clone());
                vec![(
                    name.into(),
                    json!({ "oldQueryState": since, "newQueryState": q, "total": now.len(),
                            "removed": removed, "added": added }),
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
            "Calendar/get" => vec![(
                name.into(),
                json!({ "list": [
                    { "id": "c1", "name": "Personal", "color": "#3366ff", "myRights": { "mayWriteAll": true } },
                ], "state": "c" }),
            )],
            "CalendarEvent/query" => {
                let ids: Vec<&str> = self.events.iter().filter_map(|e| e["id"].as_str()).collect();
                let total = ids.len();
                // Paged as asked, never more than the server's own limit.
                let from = args["position"].as_u64().unwrap_or(0) as usize;
                let mut limit = args["limit"].as_u64().map_or(usize::MAX, |l| l as usize);
                if self.query_limit > 0 {
                    limit = limit.min(self.query_limit);
                }
                let page: Vec<&str> = ids.into_iter().skip(from).take(limit).collect();
                vec![(name.into(), json!({ "ids": page, "total": total }))]
            }
            "CalendarEvent/changes" | "ContactCard/changes" if self.dav_changes => {
                let kind = if name.starts_with("Calendar") { 'e' } else { 'k' };
                let since: u64 = args["sinceState"].as_str().unwrap().trim_start_matches(kind).parse().unwrap_or(0);
                let mut updated = Vec::new();
                let mut destroyed = Vec::new();
                for (state, k, id, gone) in &self.dav_log {
                    if *k == kind && *state > since {
                        if *gone { destroyed.push(id.clone()) } else { updated.push(id.clone()) }
                    }
                }
                vec![(name.into(), json!({ "created": [], "updated": updated, "destroyed": destroyed,
                    "newState": format!("{kind}{}", self.dav_state), "hasMoreChanges": false }))]
            }
            "CalendarEvent/get" => {
                let wanted = ids_of(args).unwrap_or_default();
                let list: Vec<&Value> = self
                    .events
                    .iter()
                    .filter(|e| wanted.iter().any(|w| e["id"] == w.as_str()))
                    .collect();
                let state = if self.dav_changes { format!("e{}", self.dav_state) } else { "e".into() };
                vec![(name.into(), json!({ "list": list, "state": state }))]
            }
            "CalendarEvent/set" => {
                assert_eq!(args["sendSchedulingMessages"], false, "Picacho invites by mail");
                let mut out = json!({});
                if let Some(create) = args["create"].as_object() {
                    for (key, props) in create {
                        let id = format!("E{}", self.events.len() + 1);
                        let mut e = props.clone();
                        e["id"] = json!(id);
                        self.events.push(e);
                        out["created"][key] = json!({ "id": id });
                    }
                }
                if let Some(update) = args["update"].as_object() {
                    for (id, patch) in update {
                        let e = self.events.iter_mut().find(|e| e["id"] == id.as_str()).unwrap();
                        for (k, v) in patch.as_object().unwrap() {
                            e[k] = v.clone();
                        }
                    }
                }
                for id in args["destroy"].as_array().into_iter().flatten() {
                    self.events.retain(|e| e["id"] != *id);
                }
                vec![(name.into(), out)]
            }
            "AddressBook/get" => vec![(
                name.into(),
                json!({ "list": [{ "id": "b1", "name": "Contacts", "myRights": { "mayWrite": true } }], "state": "b" }),
            )],
            "ContactCard/get" => {
                let wanted = ids_of(args);
                let list: Vec<&Value> = self
                    .cards
                    .iter()
                    .filter(|c| wanted.as_ref().is_none_or(|w| w.iter().any(|i| c["id"] == i.as_str())))
                    .collect();
                let state = if self.dav_changes { format!("k{}", self.dav_state) } else { "k".into() };
                vec![(name.into(), json!({ "list": list, "state": state }))]
            }
            "ContactCard/set" => {
                let mut out = json!({});
                if let Some(create) = args["create"].as_object() {
                    for (key, props) in create {
                        let id = format!("K{}", self.cards.len() + 1);
                        let mut c = props.clone();
                        c["id"] = json!(id);
                        self.cards.push(c);
                        out["created"][key] = json!({ "id": id });
                    }
                }
                if let Some(update) = args["update"].as_object() {
                    for (id, patch) in update {
                        let c = self.cards.iter_mut().find(|c| c["id"] == id.as_str()).unwrap();
                        for (k, v) in patch.as_object().unwrap() {
                            c[k] = v.clone();
                        }
                    }
                }
                for id in args["destroy"].as_array().into_iter().flatten() {
                    self.cards.retain(|c| c["id"] != *id);
                }
                vec![(name.into(), out)]
            }
            "EmailSubmission/query" => {
                let ids: Vec<&String> = self.undo.iter().filter(|(_, u)| *u == "pending").map(|(k, _)| k).collect();
                vec![(name.into(), json!({ "ids": ids }))]
            }
            "EmailSubmission/get" => {
                let wanted = ids_of(args).unwrap_or_default();
                let list: Vec<Value> = wanted
                    .iter()
                    .filter_map(|id| {
                        let undo = self.undo.get(id)?;
                        Some(json!({ "id": id, "emailId": "M1", "undoStatus": undo,
                                     "sendAt": "2026-12-01T09:00:00Z",
                                     "deliveryStatus": { "bo@example.se": { "delivered": "queued", "smtpReply": "250 OK" } } }))
                    })
                    .collect();
                vec![(name.into(), json!({ "list": list, "state": "sub" }))]
            }
            "EmailSubmission/set" if args.get("update").is_some() => {
                let mut out = json!({ "updated": {} });
                for (id, patch) in args["update"].as_object().unwrap() {
                    match self.undo.get_mut(id) {
                        Some(u) if u == "pending" => {
                            *u = patch["undoStatus"].as_str().unwrap().to_owned();
                            out["updated"][id] = Value::Null;
                        }
                        _ => out["notUpdated"][id] = json!({ "type": "cannotUnsend" }),
                    }
                }
                vec![(name.into(), out)]
            }
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
                self.envelopes.push(s["envelope"].clone());
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
            "VacationResponse/get" => vec![(
                name.into(),
                json!({ "list": [ { "id": "singleton", "isEnabled": false } ] }),
            )]
            .into_iter()
            .map(|(n, mut v)| {
                if !self.vacation.is_null() {
                    v["list"][0] = self.vacation.clone();
                }
                (n, v)
            })
            .collect(),
            "VacationResponse/set" => {
                let mut reply = args["update"]["singleton"].clone();
                reply["id"] = json!("singleton");
                self.vacation = reply;
                vec![(name.into(), json!({ "updated": { "singleton": null } }))]
            }
            "SearchSnippet/get" => {
                let list: Vec<Value> = args["emailIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| json!({ "emailId": id, "subject": null,
                                      "preview": "…the <mark>fixture</mark> &amp; more" }))
                    .collect();
                vec![(name.into(), json!({ "list": list }))]
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
            self.session_reads += 1;
            if self.modern {
                return (200, self.modern_session(&base).to_string().into_bytes());
            }
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
                    "state": self.state_name(),
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
                assert!(
                    args["accountId"] == "u1" || (self.modern && args["accountId"] == "u2"),
                    "{name} names the account"
                );
                for (n, v) in self.method(name, args, &earlier) {
                    responses.push(json!([n, v, id]));
                    if n == name {
                        earlier.push(v);
                    }
                }
            }
            return (
                200,
                json!({ "methodResponses": responses, "sessionState": self.state_name() })
                    .to_string()
                    .into_bytes(),
            );
        }
        if method == "POST" && (path.starts_with("/upload/u1/") || path.starts_with("/upload/u2/")) {
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
            self.downloads.push(blob.to_owned());
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
        // Reading the folders again asks only what changed.
        let before = server.lock().unwrap().calls.len();
        b.with_jmap(&a, |s| s.reload_mailboxes())?;
        assert_eq!(server.lock().unwrap().calls[before..], ["Mailbox/changes"]);
        assert_eq!(b.with_jmap(&a, |s| s.mailboxes())?.len(), 5);

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

/// The fixture's certificate, its port, its state and its running thread.
type Started = (Vec<u8>, u16, Arc<Mutex<Server>>, Running);
/// Starts the fixture server.
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
        ..Default::default()
    });
    let saved = b.save_filters(&a, &filters)?;
    assert!(saved.active);
    {
        let s = server.lock().unwrap();
        let ours = s.scripts.iter().find(|x| x.1 == "picacho").unwrap();
        assert!(ours.3, "Picacho's script is the active one");
        let text = String::from_utf8_lossy(&s.blobs[&ours.2]).into_owned();
        assert!(text.contains("fileinto \"Work\";"), "{text}");
        assert!(text.contains("include :personal :optional \"vendor\";"));
    }
    // Read back as saved.
    let again = b.filters(&a)?;
    assert!(again.active);
    assert_eq!(again.filters, filters);

    // Saved again: the same script is replaced, not a second one made,
    // and its text uploaded once for both validating and saving (RFC 9661).
    let blobs_before = server.lock().unwrap().blobs.len();
    b.save_filters(&a, &filters)?;
    assert_eq!(server.lock().unwrap().scripts.len(), 2);
    assert_eq!(server.lock().unwrap().blobs.len(), blobs_before + 1);

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

/// Sending with encryption required, delivery reports asked for and a
/// message held for later: the envelope carries REQUIRETLS, RET/ENVID and
/// NOTIFY, and HOLDUNTIL, as the server offers them.
#[test]
fn jmap_submission_extensions_reach_the_envelope() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    a.require_tls = true;
    a.delivery_receipts = true;
    let d = Draft {
        id: "dsn-draft".into(),
        account: a.id.clone(),
        to: "recipient@example.com".into(),
        subject: "Encrypted all the way".into(),
        body: "Hello.".into(),
        ..Default::default()
    };
    b.store.save_draft(&d)?;
    let notes = b.send_noting(&a, &d)?;
    assert!(
        !notes.iter().any(|n| n.contains("delivery receipts")),
        "{notes:?}"
    );
    let envelope = server.lock().unwrap().envelopes[0].clone();
    let from = &envelope["mailFrom"]["parameters"];
    assert!(from.as_object().unwrap().contains_key("REQUIRETLS"), "{envelope}");
    assert_eq!(from["RET"], "HDRS");
    assert_eq!(from["ENVID"], "dsn-draft");
    assert_eq!(
        envelope["rcptTo"][0]["parameters"]["NOTIFY"],
        "SUCCESS,FAILURE,DELAY"
    );

    // Held at the server for later, within what it allows.
    let options = crate::sync::jmap::SendOptions {
        hold_until: Some(chrono::Utc::now().timestamp() + 3600),
        ..Default::default()
    };
    b.send_jmap(&a, "qa@example.com", &["recipient@example.com".into()], MESSAGE, &options)?;
    let envelope = server.lock().unwrap().envelopes[1].clone();
    let until = envelope["mailFrom"]["parameters"]["HOLDUNTIL"].as_str().unwrap().to_owned();
    assert!(until.ends_with('Z') && until.len() == 20, "{until}");
    // Too far ahead for it: refused before anything is sent.
    let too_far = crate::sync::jmap::SendOptions {
        hold_until: Some(chrono::Utc::now().timestamp() + 3 * 86400),
        ..Default::default()
    };
    assert!(b.send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &too_far).is_err());

    // A scheduled draft handed to the server, which holds it.
    a.require_tls = false;
    a.delivery_receipts = false;
    a.server_send_later = true;
    let later = Draft {
        id: "later".into(),
        account: a.id.clone(),
        to: "recipient@example.com".into(),
        subject: "Later".into(),
        body: "Hello.".into(),
        send_at: Some(chrono::Utc::now().timestamp() + 7200),
        ..Default::default()
    };
    b.store.save_draft(&later)?;
    assert!(b.hand_to_server(&a, "later")?);
    assert!(b.store.draft("later")?.is_none());
    let envelope = server.lock().unwrap().envelopes.last().unwrap().clone();
    assert!(envelope["mailFrom"]["parameters"]["HOLDUNTIL"].is_string(), "{envelope}");
    // Too far ahead for it: stays scheduled here.
    let far = Draft {
        id: "far".into(),
        send_at: Some(chrono::Utc::now().timestamp() + 5 * 86400),
        ..later
    };
    b.store.save_draft(&far)?;
    assert!(!b.hand_to_server(&a, "far")?);
    assert!(b.store.draft("far")?.is_some());
    Ok(())
}

/// Encryption required where the JMAP server can't ask for it: refused,
/// as SMTP refuses it, rather than sent without.
#[test]
fn jmap_without_requiretls_refuses_to_send_when_it_is_required() -> Result<()> {
    let (cert, port, server, _running) = start(Server::default())?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    a.require_tls = true;
    let d = Draft {
        id: "plain".into(),
        account: a.id.clone(),
        to: "recipient@example.com".into(),
        subject: "Must be encrypted".into(),
        body: "Hello.".into(),
        ..Default::default()
    };
    b.store.save_draft(&d)?;
    let error = format!("{:#}", b.send(&a, &d).unwrap_err());
    assert!(error.contains("REQUIRETLS"), "{error}");
    assert!(server.lock().unwrap().sent.is_empty());
    Ok(())
}

/// A server with only JMAP's away reply: the filter settings offer it
/// alone, and saving writes VacationResponse.
#[test]
fn jmap_away_reply_without_sieve() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let state = b.filters(&a)?;
    assert!(state.vacation_only && state.has("vacation") && !state.active);
    let mut filters = state.filters.clone();
    filters.vacation = crate::sync::sieve::Vacation {
        on: true,
        subject: "Away".into(),
        body: "Back Monday.".into(),
        days: 7,
        from: Some("2026-12-01".into()),
        until: Some("2026-12-05".into()),
    };
    let saved = b.save_filters(&a, &filters)?;
    assert!(saved.active && saved.vacation_only);
    let reply = server.lock().unwrap().vacation.clone();
    assert_eq!(reply["isEnabled"], true);
    assert_eq!(reply["fromDate"], "2026-12-01T00:00:00Z");
    assert_eq!(reply["toDate"], "2026-12-05T23:59:59Z");
    assert_eq!(reply["textBody"], "Back Monday.");
    // Read back as saved.
    assert_eq!(b.filters(&a)?.filters.vacation, filters.vacation);
    // Rules can't be kept here.
    filters.rules.push(crate::sync::sieve::Rule {
        field: crate::sync::sieve::Field::From,
        value: "x".into(),
        act: crate::sync::sieve::Act::Star,
        stop: false,
        ..Default::default()
    });
    assert!(b.save_filters(&a, &filters).is_err());
    Ok(())
}

/// A mail account shared with the sign-in is found, added as an account of
/// its own that signs in as its owner, and read; the owner can't be
/// removed while it's there.
#[test]
fn jmap_shared_accounts_are_added_and_sign_in_as_their_owner() -> Result<()> {
    let (cert, port, _server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    let found = b.jmap_shared(&a)?;
    assert_eq!(found, vec![("u2".to_owned(), "Team".to_owned())]);
    let team = b.add_jmap_shared(&a, "u2", "Team")?;
    assert_eq!(team.jmap_account, "u2");
    assert_eq!(team.credentials_of, a.id);
    assert_eq!(team.label(), "Team");
    assert!(b.jmap_shared(&a)?.is_empty(), "already added");
    b.sync(&team, Folder::Inbox, 50)?;
    let error = format!("{:#}", b.remove_account(&a.id).unwrap_err());
    assert!(error.contains("Team"), "{error}");
    b.remove_account(&team.id)?;
    b.remove_account(&a.id)?;
    Ok(())
}

/// A response naming another session state: the session is read again
/// before the next request. More calls than the server takes at once go
/// in several requests.
#[test]
fn jmap_session_changes_and_request_limits() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let ask = || b.with_jmap(&a, |s| s.identities().map(drop));
    ask()?;
    assert_eq!(server.lock().unwrap().session_reads, 1);
    server.lock().unwrap().session_state = "s2".into();
    ask()?;
    ask()?;
    assert_eq!(server.lock().unwrap().session_reads, 2, "read again once");
    // Three independent calls, two at a time.
    b.with_jmap(&a, |s| {
        let calls: Vec<(&str, Value)> = (0..3)
            .map(|_| ("Identity/get", json!({ "accountId": "u1" })))
            .collect();
        assert_eq!(s.call(&calls)?.len(), 3);
        Ok(())
    })?;
    Ok(())
}

/// Search results show the part of each message that matched.
#[test]
fn jmap_search_results_show_what_matched() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        ..Default::default()
    };
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    fixture.emails.push(Message {
        id: "e1".into(),
        blob: "B0".into(),
        mailboxes: vec!["i".into()],
        keywords: vec![],
        subject: "Over JMAP".into(),
        received: "2026-10-24T10:00:00Z".into(),
    });
    let (cert, port, _server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let results = b.search_server(&a, &Folder::Inbox.into(), "jmap", 10)?;
    assert_eq!(results.mails.len(), 1);
    assert_eq!(results.mails[0].snippet, "…the fixture & more");
    Ok(())
}

#[test]
fn jmap_shipping_mail_downloads_in_batches_for_the_summary() -> Result<()> {
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
        for n in 1..=3 {
            let blob = format!("B-S{n}");
            let source = String::from_utf8_lossy(MESSAGE).replace("local JMAP fixture", &format!("parcel {n}"));
            s.blobs.insert(blob.clone(), source.into_bytes());
            s.emails.push(Message {
                id: format!("S{n}"),
                blob,
                mailboxes: vec!["i".into()],
                keywords: vec![],
                subject: format!("Shipped {n}"),
                received: format!("2026-10-2{n}T10:00:00Z"),
            });
        }
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
        // Listed with the account's provider, as a signed-in account is.
        b.store.save_account(&a)?;
        let inbox = b.sync(&a, Folder::Inbox, 50)?;
        assert_eq!(inbox.len(), 3);
        let wanted: Vec<(String, String)> = inbox.iter().map(|m| (a.id.clone(), m.id.clone())).collect();
        let before = server.lock().unwrap().calls.len();
        let got = b.download_for_parcels(&wanted, &AtomicBool::new(false))?;
        assert_eq!(got, 3);
        // One request finds every source.
        let calls = server.lock().unwrap().calls[before..].to_vec();
        assert_eq!(calls, ["Email/get"]);
        for m in &inbox {
            let n = m.subject.trim_start_matches("Shipped ");
            let raw = b.store.raw(&m.id)?.expect("downloaded");
            assert!(String::from_utf8_lossy(&raw).contains(&format!("parcel {n}")));
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
    result
}

/// A mailbox's listing that changed is brought up to date from what
/// changed in it (Email/queryChanges): new mail read, removed mail asked
/// about, without listing the mailbox again.
#[test]
fn jmap_listing_changes_come_from_query_changes() -> Result<()> {
    let mut fixture = Server {
        query_changes: true,
        state: 1,
        ..Default::default()
    };
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    let message = |id: &str, mailbox: &str, received: &str| Message {
        id: id.into(),
        blob: "B0".into(),
        mailboxes: vec![mailbox.into()],
        keywords: vec![],
        subject: format!("Message {id}"),
        received: received.into(),
    };
    fixture.emails.push(message("e1", "i", "2026-10-24T10:00:00Z"));
    fixture.emails.push(message("e2", "i", "2026-10-25T10:00:00Z"));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    assert_eq!(b.sync(&a, Folder::Inbox, 50)?.len(), 2);
    // A new message arrives; another moves to Trash.
    {
        let mut s = server.lock().unwrap();
        s.emails.push(message("e3", "i", "2026-10-26T10:00:00Z"));
        s.touch("e3", false);
        s.emails.iter_mut().find(|m| m.id == "e1").unwrap().mailboxes = vec!["t".into()];
        s.touch("e1", false);
    }
    let before = server.lock().unwrap().calls.len();
    let inbox = b.sync(&a, Folder::Inbox, 50)?;
    let calls = server.lock().unwrap().calls[before..].to_vec();
    assert!(calls.contains(&"Email/queryChanges".to_owned()), "{calls:?}");
    assert!(!calls.contains(&"Email/query".to_owned()), "not listed again: {calls:?}");
    let ids: Vec<String> = inbox.iter().map(|m| m.id.rsplit(':').next().unwrap().to_owned()).collect();
    assert_eq!(ids, ["e3", "e2"]);
    // The moved message's row is in Trash now.
    let moved = b.store.message(&format!("{}:jmap:e1", a.id))?.unwrap();
    assert_eq!(moved.folder, Folder::Trash);
    Ok(())
}

/// Calendars and contacts kept by a JMAP server: read, added to, changed
/// and removed there, with the server told not to send invitations; and a
/// message held to send later is taken back.
#[test]
fn jmap_calendars_contacts_and_held_messages() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        ..Default::default()
    };
    fixture.events.push(json!({
        "@type": "Event", "id": "E0", "uid": "dentist@x", "title": "Dentist",
        "start": "2099-11-24T10:00:00", "timeZone": "Europe/Berlin", "duration": "PT1H",
        "calendarIds": { "c1": true },
    }));
    fixture.cards.push(json!({
        "@type": "Card", "id": "K0", "uid": "bo", "name": { "full": "Bo Lindqvist" },
        "emails": { "work": { "@type": "EmailAddress", "address": "bo@example.se", "contexts": { "work": true } } },
    }));
    fixture.undo.insert("held1".into(), "pending".into());
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    fixture.emails.push(Message {
        id: "M1".into(),
        blob: "B0".into(),
        mailboxes: vec!["s".into()],
        keywords: vec!["$seen".into()],
        subject: "Held".into(),
        received: "2026-11-20T10:00:00Z".into(),
    });
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    // Calendars.
    b.connect_jmap_calendar(&a)?;
    let cals = b.calendars();
    assert_eq!(cals.len(), 1);
    assert_eq!((cals[0].href.as_str(), cals[0].color.as_str()), ("jmap:c1", "#3366ff"));
    let events = b.calendar_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.summary, "Dentist");
    assert_eq!(events[0].href, "jmap:E0");
    let mut lunch = crate::sync::ical::Event {
        summary: "Lunch".into(),
        start: 4_100_000_000,
        end: 4_100_003_600,
        utc: true,
        alarms: vec![600],
        ..Default::default()
    };
    lunch.attendees.push(crate::sync::ical::Attendee {
        email: "bo@example.se".into(),
        ..Default::default()
    });
    let saved = b.save_event(lunch, None)?;
    assert!(saved.href.starts_with("jmap:E"));
    {
        let s = server.lock().unwrap();
        let made = s.events.iter().find(|e| e["title"] == "Lunch").unwrap();
        assert_eq!(made["calendarIds"]["c1"], true);
        assert_eq!(made["participants"]["p0"]["scheduleAgent"], "client");
    }
    b.delete_event(&saved.event.uid)?;
    assert!(!server.lock().unwrap().events.iter().any(|e| e["title"] == "Lunch"));
    // Contacts.
    assert_eq!(b.connect_jmap_contacts(&a)?, 1);
    let mut bo = b.contacts().into_iter().find(|c| c.name == "Bo Lindqvist").unwrap();
    bo.phones.push("+46 8 123".into());
    b.save_contact(bo)?;
    {
        let s = server.lock().unwrap();
        let card = s.cards.iter().find(|c| c["id"] == "K0").unwrap();
        assert_eq!(card["emails"]["work"]["contexts"]["work"], true, "the label stays");
        assert_eq!(card["phones"]["phones1"]["number"], "+46 8 123");
    }
    let cy = b.save_contact(crate::sync::carddav::Card {
        name: "Cy".into(),
        emails: vec!["cy@example.net".into()],
        ..Default::default()
    })?;
    assert_eq!(cy.href, "jmap:K2");
    b.delete_contact(&cy.uid)?;
    assert_eq!(server.lock().unwrap().cards.len(), 1);
    // Held to send later: listed, and taken back once.
    let held = b.held_submissions(&a)?;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, "held1");
    assert!(b.cancel_held(&a, "held1")?);
    {
        // Back in Drafts, as a draft.
        let s = server.lock().unwrap();
        let m = s.emails.iter().find(|m| m.id == "M1").unwrap();
        assert_eq!(m.mailboxes, ["d"]);
        assert!(m.keywords.contains(&"$draft".to_owned()));
    }
    assert!(!b.cancel_held(&a, "held1")?, "already taken back");
    let status = b.submission_status(&a, "held1")?.unwrap();
    assert_eq!(status.undo, "canceled");
    assert_eq!(status.delivery[0].1, "queued");
    Ok(())
}

/// Calendars and contacts over JMAP read only what changed once read
/// (CalendarEvent/changes, ContactCard/changes), page through a query the
/// server cuts short, and don't overwrite a card changed elsewhere.
#[test]
fn jmap_calendars_and_contacts_sync_by_changes() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        dav_changes: true,
        query_limit: 2,
        ..Default::default()
    };
    for i in 0..5 {
        fixture.events.push(json!({
            "@type": "Event", "id": format!("E{i}"), "uid": format!("u{i}@x"), "title": format!("Event {i}"),
            "start": "2099-11-24T10:00:00", "timeZone": "Europe/Berlin", "duration": "PT1H",
            "calendarIds": { "c1": true },
        }));
    }
    fixture.cards.push(json!({
        "@type": "Card", "id": "K0", "uid": "bo", "name": { "full": "Bo" }, "updated": "2026-11-01T00:00:00Z",
    }));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_calendar(&a)?;
    // Every page of five, two at a time.
    assert_eq!(b.calendar_events().len(), 5);
    {
        let mut s = server.lock().unwrap();
        s.dav_state += 1;
        let state = s.dav_state;
        s.events.retain(|e| e["id"] != "E0");
        s.dav_log.push((state, 'e', "E0".into(), true));
        s.events[0]["title"] = json!("Renamed");
        s.dav_log.push((state, 'e', "E1".into(), false));
        s.calls.clear();
    }
    b.sync_calendar()?;
    let events = b.calendar_events();
    assert_eq!(events.len(), 4);
    assert!(events.iter().any(|e| e.event.summary == "Renamed"));
    {
        let s = server.lock().unwrap();
        assert!(s.calls.contains(&"CalendarEvent/changes".to_owned()), "{:?}", s.calls);
        assert!(!s.calls.contains(&"CalendarEvent/query".to_owned()), "only changes: {:?}", s.calls);
    }
    // Contacts: changes only, then a card changed elsewhere isn't overwritten.
    b.connect_jmap_contacts(&a)?;
    {
        let mut s = server.lock().unwrap();
        s.dav_state += 1;
        let state = s.dav_state;
        s.cards.push(json!({ "@type": "Card", "id": "K1", "uid": "cy", "name": { "full": "Cy" } }));
        s.dav_log.push((state, 'k', "K1".into(), false));
        s.calls.clear();
    }
    b.sync_contacts()?;
    assert!(b.contacts().iter().any(|c| c.name == "Cy"));
    assert!(server.lock().unwrap().calls.contains(&"ContactCard/changes".to_owned()));
    let mut bo = b.contacts().into_iter().find(|c| c.name == "Bo").unwrap();
    server.lock().unwrap().cards[0]["updated"] = json!("2026-11-02T00:00:00Z");
    bo.note = "Mine".into();
    let error = b.save_contact(bo).unwrap_err();
    assert!(error.downcast_ref::<crate::sync::jscal::Conflict>().is_some(), "{error:#}");
    {
        let s = server.lock().unwrap();
        let sets = s.calls.iter().filter(|c| *c == "ContactCard/set").count();
        assert_eq!(sets, 0, "not overwritten: {:?}", s.calls);
    }
    Ok(())
}

/// Contact photos the server keeps as blobs are downloaded once, several
/// at a time, and reused on later reads while the blob is the same.
#[test]
fn jmap_contact_photos_download_once_and_are_reused() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        dav_changes: true,
        ..Default::default()
    };
    for i in 0..6 {
        let blob = format!("P{i}");
        fixture.blobs.insert(blob.clone(), format!("photo {i}").into_bytes());
        fixture.cards.push(json!({
            "@type": "Card", "id": format!("K{i}"), "uid": format!("u{i}"), "name": { "full": format!("Person {i}") },
            "media": { "p": { "@type": "Media", "kind": "photo", "blobId": blob, "mediaType": "image/png" } },
        }));
    }
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    assert_eq!(b.connect_jmap_contacts(&a)?, 6);
    {
        let mut downloads = server.lock().unwrap().downloads.clone();
        downloads.sort();
        assert_eq!(downloads, ["P0", "P1", "P2", "P3", "P4", "P5"]);
    }
    let three = b.contacts().into_iter().find(|c| c.name == "Person 3").unwrap();
    assert_eq!(three.photo_bytes().as_deref(), Some(&b"photo 3"[..]));
    assert_eq!(three.photo_blob, "P3");
    // Read again: the photos are already here.
    {
        let mut s = server.lock().unwrap();
        s.downloads.clear();
        // One photo is replaced by a new blob, which is downloaded.
        s.blobs.insert("P9".into(), b"photo 9".to_vec());
        s.cards[1]["media"]["p"]["blobId"] = json!("P9");
        s.cards[3]["name"]["full"] = json!("Person 3b");
        s.dav_state += 1;
        let state = s.dav_state;
        s.dav_log.push((state, 'k', "K1".into(), false));
        s.dav_log.push((state, 'k', "K3".into(), false));
    }
    b.sync_contacts()?;
    assert_eq!(server.lock().unwrap().downloads, ["P9"]);
    let one = b.contacts().into_iter().find(|c| c.name == "Person 1").unwrap();
    assert_eq!(one.photo_bytes().as_deref(), Some(&b"photo 9"[..]));
    // Changed, but its photo is the same blob: not downloaded again.
    let three = b.contacts().into_iter().find(|c| c.name == "Person 3b").unwrap();
    assert_eq!(three.photo_bytes().as_deref(), Some(&b"photo 3"[..]));
    // A photo kept from before isn't an edit: saving leaves `media` alone.
    b.save_contact(three)?;
    assert_eq!(server.lock().unwrap().cards[3]["media"]["p"]["blobId"], "P3");
    Ok(())
}

/// A submission the server refuses as a whole (a method error, RFC 8620
/// §3.6.2) still lets the import before it be seen, so its copy is removed
/// rather than left behind in Drafts.
#[test]
fn jmap_refused_submission_leaves_no_orphaned_import() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        refuse: Some("EmailSubmission/set"),
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let options = crate::sync::jmap::SendOptions::default();
    let error = b
        .send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &options)
        .unwrap_err();
    assert!(format!("{error:#}").contains("EmailSubmission/set"), "{error:#}");
    let s = server.lock().unwrap();
    assert!(s.calls.contains(&"Email/import".to_owned()), "{:?}", s.calls);
    assert!(s.emails.is_empty(), "the imported copy is removed");
    Ok(())
}

/// A server without ContactCard/changes (`unknownMethod`) has its cards
/// read whole again, rather than failing the sync.
#[test]
fn jmap_contacts_without_changes_are_read_whole() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        ..Default::default()
    };
    fixture.cards.push(json!({ "@type": "Card", "id": "K0", "uid": "bo", "name": { "full": "Bo" } }));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    assert_eq!(b.connect_jmap_contacts(&a)?, 1);
    server
        .lock()
        .unwrap()
        .cards
        .push(json!({ "@type": "Card", "id": "K1", "uid": "cy", "name": { "full": "Cy" } }));
    assert_eq!(b.sync_contacts()?, 2);
    Ok(())
}
