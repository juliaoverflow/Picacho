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
    /// The session's maxObjectsInGet and maxObjectsInSet, where set (0:
    /// 100); calls naming more are refused with `requestTooLarge`.
    max_objects: usize,
    /// Where /.well-known/jmap redirects (a 307), if anywhere; the session
    /// is then at /jmap/session.
    session_redirect: Option<String>,
    /// Where downloads redirect, by blob id: the address a 302 names.
    blob_redirects: HashMap<String, String>,
    /// The Authorization header each /cdn/ request came with.
    cdn_auth: Vec<String>,
    /// Paths requested, in order.
    paths: Vec<String>,
    /// Mailboxes as `Mailbox/set` left them, where it was used (else the
    /// fixed `mailboxes()`).
    boxes: Option<Vec<Value>>,
    /// Identities added after `id1`; the Identity state is `i<count>`.
    extra_identities: Vec<Value>,
    /// ParticipantIdentity/get names one address (else unknownMethod).
    participant_identities: bool,
    /// Each CalendarEvent/set: (sendSchedulingMessages, the participants'
    /// scheduleAgents).
    scheduling: Vec<(bool, Vec<String>)>,
    /// The mail account's capability (RFC 8621 §1.3.1), where set.
    mail_caps: Option<Value>,
    /// Read receipts the server sent (MDN/send's MDN objects).
    mdns: Vec<Value>,
    /// The Sieve capability's maxSizeScript, where set (else 64 KiB).
    sieve_max_size: Option<usize>,
    /// Quota: the bytes used (else 2048) and the Quota state.
    quota_used: Option<u64>,
    quota_state: u64,
    /// Blob/get answers with a digest that doesn't match.
    bad_digest: bool,
    /// Calendar/set shareWith and Principal/set patches, as sent.
    shared_with: Vec<Value>,
    /// JMAP for Tasks, in the modern session: its task lists and tasks.
    tasks_on: bool,
    task_lists: Vec<Value>,
    tasks: Vec<Value>,
    /// The Task state and each change since: (state, id, destroyed).
    task_state: u64,
    task_log: Vec<(u64, String, bool)>,
    /// The account's mayCreateTaskList, where said.
    may_create_task_list: Option<bool>,
    /// TaskNotifications not yet destroyed.
    task_notifications: Vec<Value>,
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
                "submissionExtensions": { "DSN": [], "REQUIRETLS": [], "FUTURERELEASE": ["86400"], "DELIVERBY": ["600"] },
            },
            "urn:ietf:params:jmap:vacationresponse": {},
            "urn:ietf:params:jmap:mdn": {},
            "urn:ietf:params:jmap:blob": { "maxSizeBlobSet": 1000, "supportedDigestAlgorithms": ["sha", "sha-256"] },
        });
        json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": {
                    "maxObjectsInGet": self.object_limit(), "maxObjectsInSet": self.object_limit(),
                    "maxCallsInRequest": 2, "maxSizeRequest": 1_000_000,
                },
                "urn:ietf:params:jmap:mail": {},
                "urn:ietf:params:jmap:submission": {},
                "urn:ietf:params:jmap:vacationresponse": {},
                "urn:ietf:params:jmap:calendars": {},
                "urn:ietf:params:jmap:contacts": {},
                "urn:ietf:params:jmap:mdn": {},
                "urn:ietf:params:jmap:blob": {},
                "urn:ietf:params:jmap:principals": {},
            },
            "accounts": {
                "u1": { "name": "qa@example.com", "isPersonal": true, "accountCapabilities": account },
                "u2": { "name": "Team", "isPersonal": false, "accountCapabilities": account },
                "u3": { "name": "Archive", "isPersonal": false, "isReadOnly": true, "accountCapabilities": account },
            },
            "primaryAccounts": {
                "urn:ietf:params:jmap:mail": "u1",
                "urn:ietf:params:jmap:calendars": "u1",
                "urn:ietf:params:jmap:contacts": "u1",
                "urn:ietf:params:jmap:principals": "u1",
            },
            "username": "qa@example.com",
            "apiUrl": format!("{base}/api"),
            "downloadUrl": format!("{base}/download/{{accountId}}/{{blobId}}/{{name}}?type={{type}}"),
            "uploadUrl": format!("{base}/upload/{{accountId}}/"),
            "state": self.state_name(),
        })
    }
    fn object_limit(&self) -> usize {
        if self.max_objects == 0 { 100 } else { self.max_objects }
    }
    /// RFC 8620 §5.1, §5.3: a call naming more objects than the session
    /// allows is refused.
    fn too_many_objects(&self, args: &Value) -> bool {
        let count = |v: &Value| match v {
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            _ => 0,
        };
        ["ids", "create", "update", "destroy", "blobIds", "emailIds"]
            .iter()
            .any(|k| count(&args[*k]) > self.object_limit())
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
        if self.too_many_objects(args) {
            return vec![("error".into(), json!({ "type": "requestTooLarge" }))];
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
            "Mailbox/set" => {
                let boxes = self.boxes.get_or_insert_with(|| Self::mailboxes().as_array().unwrap().clone());
                let mut out = json!({ "updated": {}, "created": {}, "destroyed": [] });
                for (key, props) in args["create"].as_object().into_iter().flatten() {
                    let id = format!("box{}", boxes.len());
                    let mut m = props.clone();
                    m["id"] = json!(id);
                    boxes.push(m);
                    out["created"][key] = json!({ "id": id });
                }
                for (id, patch) in args["update"].as_object().into_iter().flatten() {
                    let rights = boxes.iter().find(|m| m["id"] == id.as_str()).map(|m| m["myRights"].clone());
                    if rights.as_ref().is_some_and(|r| r["mayRename"] == false) {
                        out["notUpdated"][id] = json!({ "type": "forbidden" });
                        continue;
                    }
                    let m = boxes.iter_mut().find(|m| m["id"] == id.as_str()).unwrap();
                    for (k, v) in patch.as_object().unwrap() {
                        m[k] = v.clone();
                    }
                    out["updated"][id] = Value::Null;
                }
                for id in args["destroy"].as_array().into_iter().flatten() {
                    let id = id.as_str().unwrap().to_owned();
                    let has = self.emails.iter().any(|e| e.mailboxes.contains(&id));
                    if has && args["onDestroyRemoveEmails"] != true {
                        out["notDestroyed"][&id] = json!({ "type": "mailboxHasEmail" });
                        continue;
                    }
                    // Its messages go where they are in no other mailbox.
                    self.emails.retain(|e| !(e.mailboxes == [id.clone()]));
                    for e in &mut self.emails {
                        e.mailboxes.retain(|m| *m != id);
                    }
                    self.boxes.as_mut().unwrap().retain(|m| m["id"] != id.as_str());
                    out["destroyed"].as_array_mut().unwrap().push(json!(id));
                }
                vec![(name.into(), out)]
            }
            "Mailbox/query" => {
                let all = self.boxes.clone().unwrap_or_else(|| Self::mailboxes().as_array().unwrap().clone());
                let ids: Vec<&Value> = all
                    .iter()
                    .filter(|m| args["filter"]["isSubscribed"].as_bool().is_none_or(|on| m["isSubscribed"].as_bool().unwrap_or(true) == on))
                    .map(|m| &m["id"])
                    .collect();
                vec![(name.into(), json!({ "ids": ids, "queryState": "mq1" }))]
            }
            "Mailbox/get" => {
                let all = match &self.boxes {
                    Some(b) => Value::Array(b.clone()),
                    None => Self::mailboxes(),
                };
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
                let state = if self.boxes.is_some() { "m2" } else { "m1" };
                vec![(name.into(), json!({ "list": list, "state": state }))]
            }
            // The mailboxes don't change here, unless Mailbox/set changed
            // them (then the state is too old: read them all).
            "Mailbox/changes" if args["sinceState"] == "m1" && self.boxes.is_none() => vec![(
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
            "Identity/get" => {
                let all = vec![json!({ "id": "id1", "name": "QA", "email": "qa@example.com",
                    "replyTo": [{ "name": "QA desk", "email": "desk@example.com" }],
                    "bcc": null, "textSignature": "-- \nQA", "htmlSignature": "<p>QA</p>" })];
                let mut all = all;
                all.extend(self.extra_identities.iter().cloned());
                let list: Vec<Value> = match ids_of(args) {
                    Some(ids) => all.into_iter().filter(|i| ids.iter().any(|w| i["id"] == w.as_str())).collect(),
                    None => all,
                };
                vec![(name.into(), json!({ "list": list, "state": format!("i{}", self.extra_identities.len()) }))]
            }
            "Identity/changes" => {
                let since: usize = args["sinceState"].as_str().unwrap().trim_start_matches('i').parse().unwrap_or(0);
                let created: Vec<&Value> = self.extra_identities[since.min(self.extra_identities.len())..]
                    .iter()
                    .map(|i| &i["id"])
                    .collect();
                vec![(name.into(), json!({ "oldState": format!("i{since}"), "newState": format!("i{}", self.extra_identities.len()),
                    "hasMoreChanges": false, "created": created, "updated": [], "destroyed": [] }))]
            }
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
            "ContactCard/set" | "CalendarEvent/set"
                if args["ifInState"].is_string() && self.dav_changes =>
            {
                // RFC 8620 §5.3: refused unless the state is still that.
                let kind = if name.starts_with("Calendar") { 'e' } else { 'k' };
                if args["ifInState"] != format!("{kind}{}", self.dav_state).as_str() {
                    return vec![("error".into(), json!({ "type": "stateMismatch" }))];
                }
                let mut plain = args.clone();
                plain.as_object_mut().unwrap().remove("ifInState");
                self.dav_state += 1;
                self.method(name, &plain, earlier)
            }
            "ParticipantIdentity/get" if self.participant_identities => vec![(
                name.into(),
                json!({ "list": [{ "id": "pi1", "name": "QA", "sendTo": { "imip": "mailto:qa@example.com" } }] }),
            )],
            "CalendarEvent/set" => {
                // Who tells the participants: each /set's sendSchedulingMessages
                // and the participants' scheduleAgent, in order.
                let agents: Vec<String> = ["create", "update"]
                    .iter()
                    .flat_map(|k| args[*k].as_object().into_iter().flatten())
                    .flat_map(|(_, e)| e["participants"].as_object().into_iter().flatten())
                    .filter_map(|(_, p)| p["scheduleAgent"].as_str().map(str::to_owned))
                    .collect();
                self.scheduling.push((args["sendSchedulingMessages"] == true, agents));
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
            "Calendar/changes" | "AddressBook/changes" | "ShareNotification/changes" => vec![(
                name.into(),
                json!({ "oldState": args["sinceState"], "newState": "c2", "hasMoreChanges": false,
                        "created": ["c9"], "updated": [], "destroyed": ["c1"] }),
            )],
            "CalendarEvent/queryChanges" | "ContactCard/queryChanges" => vec![(
                name.into(),
                json!({ "oldQueryState": args["sinceQueryState"], "newQueryState": "q2", "total": 1,
                        "removed": ["E0"], "added": [{ "id": "E9", "index": 0 }] }),
            )],
            "CalendarEvent/copy" | "ContactCard/copy" => {
                assert_eq!(args["fromAccountId"], "u2");
                let created: serde_json::Map<String, Value> = args["create"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|k| (k.clone(), json!({ "id": format!("copy-{k}") })))
                    .collect();
                vec![(name.into(), json!({ "created": created }))]
            }
            "CalendarEvent/parse" => {
                let parsed: serde_json::Map<String, Value> = args["blobIds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| (b.as_str().unwrap().to_owned(), json!([{ "@type": "Event", "uid": "p@x", "title": "Parsed",
                        "start": "2026-12-01T09:00:00", "timeZone": "Etc/UTC", "duration": "PT30M" }])))
                    .collect();
                vec![(name.into(), json!({ "parsed": parsed, "notParsable": [], "notFound": [] }))]
            }
            "Calendar/set" if args["update"].as_object().is_some_and(|u| u.values().any(|p| p.get("shareWith").is_some())) => {
                self.shared_with.push(args["update"].clone());
                let updated: serde_json::Map<String, Value> =
                    args["update"].as_object().unwrap().keys().map(|k| (k.clone(), Value::Null)).collect();
                vec![(name.into(), json!({ "updated": updated }))]
            }
            "Principal/get" => vec![(
                name.into(),
                json!({ "list": [{ "id": "P1", "type": "individual", "name": "Bo Lindqvist", "email": "bo@example.se",
                                   "accounts": { "u2": {} } }], "state": "p1" }),
            )],
            "Principal/set" => {
                self.shared_with.push(args["update"].clone());
                vec![(name.into(), json!({ "updated": { "P1": null } }))]
            }
            "TaskList/get" => vec![(name.into(), json!({ "list": self.task_lists, "state": "tl" }))],
            "TaskList/set" => {
                let mut out = json!({});
                for (key, props) in args["create"].as_object().into_iter().flatten() {
                    let id = format!("L{}", self.task_lists.len() + 1);
                    let mut l = props.clone();
                    l["id"] = json!(id);
                    self.task_lists.push(l);
                    out["created"][key] = json!({ "id": id });
                }
                vec![(name.into(), out)]
            }
            "Task/query" => {
                // draft-ietf-jmap-tasks-07 §4.8.1: `inTaskList` and `progress`.
                let f = &args["filter"];
                let ids: Vec<&Value> = self
                    .tasks
                    .iter()
                    .filter(|t| f["inTaskList"].as_str().is_none_or(|l| t["taskListIds"][l] == true))
                    .filter(|t| {
                        let progress = t["progress"].as_str().unwrap_or("needs-action");
                        f["progress"].as_array().is_none_or(|p| p.iter().any(|x| x == progress))
                    })
                    .map(|t| &t["id"])
                    .collect();
                vec![(name.into(), json!({ "ids": ids, "total": ids.len() }))]
            }
            "Task/get" => {
                let wanted = ids_of(args).unwrap_or_default();
                let list: Vec<&Value> = self.tasks.iter().filter(|t| wanted.iter().any(|w| t["id"] == w.as_str())).collect();
                vec![(name.into(), json!({ "list": list, "state": format!("t{}", self.task_state) }))]
            }
            "Task/changes" => {
                let since: u64 = args["sinceState"].as_str().unwrap().trim_start_matches('t').parse().unwrap();
                let (mut updated, mut destroyed) = (Vec::new(), Vec::new());
                for (state, id, gone) in &self.task_log {
                    if *state > since {
                        if *gone { destroyed.push(id.clone()) } else { updated.push(id.clone()) }
                    }
                }
                vec![(name.into(), json!({ "oldState": args["sinceState"], "newState": format!("t{}", self.task_state),
                    "hasMoreChanges": false, "created": [], "updated": updated, "destroyed": destroyed }))]
            }
            "TaskNotification/get" => vec![(name.into(), json!({ "list": self.task_notifications, "state": "n" }))],
            "TaskNotification/set" => {
                let gone = args["destroy"].as_array().cloned().unwrap_or_default();
                self.task_notifications.retain(|n| !gone.contains(&n["id"]));
                vec![(name.into(), json!({ "destroyed": gone }))]
            }
            "Task/set" if args["ifInState"].is_string() && args["ifInState"] != format!("t{}", self.task_state).as_str() => {
                vec![("error".into(), json!({ "type": "stateMismatch" }))]
            }
            "Task/set" => {
                let mut out = json!({});
                for (key, props) in args["create"].as_object().into_iter().flatten() {
                    let id = format!("T{}", self.tasks.len() + 10);
                    let mut t = props.clone();
                    t["id"] = json!(id);
                    self.tasks.push(t);
                    self.task_state += 1;
                    self.task_log.push((self.task_state, id.clone(), false));
                    out["created"][key] = json!({ "id": id });
                }
                for (id, patch) in args["update"].as_object().into_iter().flatten() {
                    let at = self.tasks.iter().position(|t| t["id"] == id.as_str()).unwrap();
                    let mut t = self.tasks[at].as_object().unwrap().clone();
                    let mut invalid_patch = false;
                    for (path, v) in patch.as_object().unwrap() {
                        // A patch path (`alerts/a1`) or a property; RFC 8620
                        // §5.3: every part before the last must be there.
                        let mut keys: Vec<String> = path.split('/').map(|k| k.replace("~1", "/").replace("~0", "~")).collect();
                        let last = keys.pop().unwrap();
                        let mut obj = Some(&mut t);
                        for k in keys {
                            obj = obj.and_then(|o| o.get_mut(&k)).and_then(Value::as_object_mut);
                        }
                        match obj {
                            Some(obj) if v.is_null() => {
                                obj.remove(&last);
                            }
                            Some(obj) => {
                                obj.insert(last, v.clone());
                            }
                            None => invalid_patch = true,
                        }
                    }
                    if invalid_patch {
                        out["notUpdated"][id] = json!({ "type": "invalidPatch" });
                        continue;
                    }
                    // §4.6: a zone or a time-less task needs a due or start
                    // time; §5: an alert, the time it's relative to.
                    let has = |k: &str| t.get(k).is_some_and(Value::is_string);
                    let zoned = t.get("timeZone").is_some_and(Value::is_string) || t.get("showWithoutTime") == Some(&json!(true));
                    let orphan_alert = t.get("alerts").and_then(Value::as_object).into_iter().flatten().any(|(_, a)| {
                        a["trigger"]["@type"] == "OffsetTrigger"
                            && !has(if a["trigger"]["relativeTo"] == "end" { "due" } else { "start" })
                    });
                    if (zoned && !has("due") && !has("start")) || orphan_alert {
                        out["notUpdated"][id] = json!({ "type": "invalidProperties" });
                        continue;
                    }
                    self.tasks[at] = Value::Object(t);
                    self.task_state += 1;
                    self.task_log.push((self.task_state, id.clone(), false));
                    out["updated"][id] = Value::Null;
                }
                for id in args["destroy"].as_array().into_iter().flatten() {
                    self.tasks.retain(|t| t["id"] != *id);
                    self.task_state += 1;
                    self.task_log.push((self.task_state, id.as_str().unwrap().to_owned(), true));
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
                                     "deliveryStatus": {
                                         "bo@example.se": { "delivered": "queued", "smtpReply": "250 OK", "displayed": "unknown" },
                                         "cy@example.net": { "delivered": "yes", "smtpReply": "250 OK", "displayed": "yes" },
                                     } }))
                    })
                    .collect();
                vec![(name.into(), json!({ "list": list, "state": "sub" }))]
            }
            "EmailSubmission/set" if args.get("destroy").is_some() => {
                let mut out = json!({ "destroyed": [] });
                for id in args["destroy"].as_array().unwrap() {
                    let id = id.as_str().unwrap();
                    match self.undo.get(id).map(String::as_str) {
                        Some("pending") => out["notDestroyed"][id] = json!({ "type": "forbidden" }),
                        Some(_) => {
                            self.undo.remove(id);
                            out["destroyed"].as_array_mut().unwrap().push(json!(id));
                        }
                        None => out["notDestroyed"][id] = json!({ "type": "notFound" }),
                    }
                }
                vec![(name.into(), out)]
            }
            "EmailSubmission/queryChanges" => {
                vec![(name.into(), json!({ "oldQueryState": args["sinceQueryState"], "newQueryState": "sq2",
                    "removed": ["held0"], "added": [{ "id": "held9", "index": 0 }] }))]
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
                // onSuccessUpdateEmail and onSuccessDestroyEmail run as an
                // implicit Email/set.
                if let Some(patch) = args["onSuccessUpdateEmail"]["#s"].as_object() {
                    let update = json!({ "update": { email_id.clone(): patch } });
                    out.extend(self.method("Email/set", &update, earlier));
                }
                if args["onSuccessDestroyEmail"] == json!(["#s"]) {
                    let destroy = json!({ "destroy": [email_id] });
                    out.extend(self.method("Email/set", &destroy, earlier));
                }
                out
            }
            "Blob/get" => {
                let blob = ids_of(args).unwrap()[0].clone();
                let data = self.blobs.get(&blob).cloned().unwrap_or_default();
                let from = args["offset"].as_u64().unwrap_or(0) as usize;
                let len = args["length"].as_u64().map_or(usize::MAX, |l| l as usize);
                let part = &data[from.min(data.len())..from.saturating_add(len).min(data.len())];
                use base64::Engine as _;
                let mut item = json!({ "id": blob, "data:asBase64": base64::engine::general_purpose::STANDARD.encode(part) });
                if args["properties"].as_array().unwrap().iter().any(|p| p == "digest:sha-256") {
                    use sha2::Digest as _;
                    let mut hash = sha2::Sha256::digest(part).to_vec();
                    if self.bad_digest {
                        hash[0] ^= 1;
                    }
                    item["digest:sha-256"] = json!(base64::engine::general_purpose::STANDARD.encode(hash));
                }
                vec![(name.into(), json!({ "list": [item], "notFound": [] }))]
            }
            "Blob/lookup" => {
                let list: Vec<Value> = args["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| {
                        let users: Vec<&String> = self.emails.iter().filter(|m| m.blob == b.as_str().unwrap()).map(|m| &m.id).collect();
                        json!({ "id": b, "matchedIds": { "Email": users } })
                    })
                    .collect();
                vec![(name.into(), json!({ "list": list, "notFound": [] }))]
            }
            "MDN/send" => {
                let mut sent = serde_json::Map::new();
                for (key, mdn) in args["send"].as_object().into_iter().flatten() {
                    self.mdns.push(mdn.clone());
                    sent.insert(key.clone(), json!({ "finalRecipient": "rfc822; qa@example.com" }));
                    // onSuccessUpdateEmail runs as an implicit Email/set.
                    if let Some(patch) = args["onSuccessUpdateEmail"][format!("#{key}")].as_object()
                        && let Some(id) = mdn["forEmailId"].as_str()
                    {
                        let update = json!({ "update": { id: patch } });
                        let _ = self.method("Email/set", &update, earlier);
                    }
                }
                vec![(name.into(), json!({ "sent": sent }))]
            }
            "MDN/parse" => {
                let mut parsed = serde_json::Map::new();
                for blob in args["blobIds"].as_array().into_iter().flatten() {
                    parsed.insert(blob.as_str().unwrap().to_owned(), json!({
                        "forEmailId": "M1", "originalMessageId": "<m1@example.com>",
                        "finalRecipient": "rfc822; bo@example.se", "reportingUA": "Other/2",
                        "disposition": { "actionMode": "automatic-action", "sendingMode": "mdn-sent-automatically", "type": "displayed" },
                    }));
                }
                vec![(name.into(), json!({ "parsed": parsed, "notParsable": [], "notFound": [] }))]
            }
            "Email/parse" => {
                let mut parsed = serde_json::Map::new();
                for blob in args["blobIds"].as_array().into_iter().flatten() {
                    let blob = blob.as_str().unwrap();
                    if !self.blobs.contains_key(blob) {
                        continue;
                    }
                    let mut e = json!({ "subject": "Attached", "from": [{ "email": "ana@example.org" }], "preview": "Short preview" });
                    if args["fetchTextBodyValues"] == true {
                        let most = args["maxBodyValueBytes"].as_u64().unwrap_or(u64::MAX) as usize;
                        let text = "The whole first paragraph of the attached message.";
                        e["textBody"] = json!([{ "partId": "1", "type": "text/plain" }]);
                        e["bodyValues"] = json!({ "1": { "value": &text[..text.len().min(most)], "isTruncated": text.len() > most } });
                    }
                    parsed.insert(blob.to_owned(), e);
                }
                vec![(name.into(), json!({ "parsed": parsed, "notParsable": [], "notFound": [] }))]
            }
            "Email/copy" => {
                let mut created = serde_json::Map::new();
                for (key, c) in args["create"].as_object().into_iter().flatten() {
                    let Some(original) = self.emails.iter().find(|m| m.id == c["id"]).cloned() else {
                        continue;
                    };
                    self.next += 1;
                    let id = format!("C{}", self.next);
                    self.emails.push(Message {
                        id: id.clone(),
                        mailboxes: c["mailboxIds"].as_object().unwrap().keys().cloned().collect(),
                        ..original
                    });
                    created.insert(key.clone(), json!({ "id": id }));
                }
                vec![(name.into(), json!({ "created": created }))]
            }
            "Blob/copy" => {
                let copied: serde_json::Map<String, Value> = args["blobIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b.as_str())
                    .map(|b| (b.to_owned(), json!(format!("copy-{b}"))))
                    .collect();
                vec![(name.into(), json!({ "copied": copied }))]
            }
            "CalendarEventNotification/set" => {
                let destroyed = args["destroy"].clone();
                vec![(name.into(), json!({ "destroyed": destroyed }))]
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
            "SieveScript/query" => {
                let ids: Vec<&String> = self
                    .scripts
                    .iter()
                    .filter(|x| args["filter"]["isActive"].as_bool().is_none_or(|on| x.3 == on))
                    .filter(|x| args["filter"]["name"].as_str().is_none_or(|n| x.1 == n))
                    .map(|x| &x.0)
                    .collect();
                vec![(name.into(), json!({ "ids": ids, "queryState": "sq" }))]
            }
            "SieveScript/set" if args.get("destroy").is_some() || args["update"].as_object().is_some_and(|u| u.values().any(|p| p.get("name").is_some())) => {
                let mut out = json!({ "updated": {}, "destroyed": [] });
                for (id, p) in args["update"].as_object().into_iter().flatten() {
                    let new = p["name"].as_str().unwrap().to_owned();
                    if self.scripts.iter().any(|x| x.1 == new && x.0 != *id) {
                        out["notUpdated"][id] = json!({ "type": "alreadyExists" });
                        continue;
                    }
                    self.scripts.iter_mut().find(|x| x.0 == *id).unwrap().1 = new;
                    out["updated"][id] = Value::Null;
                }
                for id in args["destroy"].as_array().into_iter().flatten() {
                    let id = id.as_str().unwrap();
                    match self.scripts.iter().find(|x| x.0 == id) {
                        Some(x) if x.3 => out["notDestroyed"][id] = json!({ "type": "sieveIsActive" }),
                        Some(_) => {
                            self.scripts.retain(|x| x.0 != id);
                            out["destroyed"].as_array_mut().unwrap().push(json!(id));
                        }
                        None => out["notDestroyed"][id] = json!({ "type": "notFound" }),
                    }
                }
                vec![(name.into(), out)]
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
                                   "used": self.quota_used.unwrap_or(2048), "hardLimit": 1048576 }],
                        "state": format!("qs{}", self.quota_state) }),
            )],
            "Quota/changes" => {
                let since: u64 = args["sinceState"].as_str().unwrap().trim_start_matches("qs").parse().unwrap();
                let updated: Vec<&str> = if since < self.quota_state { vec!["q"] } else { vec![] };
                vec![(name.into(), json!({ "oldState": args["sinceState"], "newState": format!("qs{}", self.quota_state),
                    "hasMoreChanges": false, "created": [], "updated": updated, "destroyed": [] }))]
            }
            "Quota/query" => vec![(name.into(), json!({ "ids": ["q"], "queryState": "qq" }))],
            _ => vec![("error".into(), json!({ "type": "unknownMethod" }))],
        }
    }
    fn respond(&mut self, method: &str, path: &str, auth: &str, body: &[u8]) -> (u16, Vec<u8>) {
        self.paths.push(path.to_owned());
        // A CDN's presigned address: no sign-in wanted, nor kept.
        if let Some(blob) = path.strip_prefix("/cdn/") {
            self.cdn_auth.push(auth.to_owned());
            return match self.blobs.get(blob) {
                Some(b) => (200, b.clone()),
                None => (404, Vec::new()),
            };
        }
        if auth != AUTH && auth != TOKEN {
            return (401, b"{}".to_vec());
        }
        let base = format!("https://localhost:{}", self.port);
        if method == "GET"
            && path == "/.well-known/jmap"
            && let Some(to) = &self.session_redirect
        {
            // A 3xx's body is the Location it names (see `serve`).
            return (307, to.clone().into_bytes());
        }
        if method == "GET" && (path == "/.well-known/jmap" || path == "/jmap/session") {
            self.session_reads += 1;
            if self.modern {
                let mut session = self.modern_session(&base);
                if self.tasks_on {
                    session["capabilities"]["urn:ietf:params:jmap:tasks"] = json!({});
                    session["primaryAccounts"]["urn:ietf:params:jmap:tasks"] = json!("u1");
                    if let Some(may) = self.may_create_task_list {
                        session["accounts"]["u1"]["accountCapabilities"]["urn:ietf:params:jmap:tasks"] = json!({ "mayCreateTaskList": may });
                    }
                }
                return (200, session.to_string().into_bytes());
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
                        "urn:ietf:params:jmap:mail": self.mail_caps.clone().unwrap_or(json!({})), "urn:ietf:params:jmap:submission": {},
                        "urn:ietf:params:jmap:sieve": {
                            "sieveExtensions": ["fileinto", "vacation", "imap4flags", "include"],
                            "maxSizeScript": self.sieve_max_size.unwrap_or(65536), "maxNumberScripts": 5 } } } },
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
            // RFC 8620 §3.3: creation ids from earlier requests resolve as
            // if made in this one.
            let mut earlier: Vec<Value> = request["createdIds"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, id)| json!({ "created": { k.clone(): { "id": id } } }))
                .collect();
            let carried = earlier.len();
            for call in request["methodCalls"].as_array().unwrap() {
                let (name, args, id) = (call[0].as_str().unwrap(), &call[1], &call[2]);
                if name.starts_with("Task/") || name.starts_with("TaskList/") {
                    assert!(
                        request["using"].as_array().unwrap().iter().any(|u| u == "urn:ietf:params:jmap:tasks"),
                        "{name} is asked using JMAP for Tasks"
                    );
                }
                assert!(
                    args["accountId"] == "u1" || (self.modern && (args["accountId"] == "u2" || args["accountId"] == "u3")),
                    "{name} names the account"
                );
                for (n, v) in self.method(name, args, &earlier) {
                    responses.push(json!([n, v, id]));
                    if n == name {
                        earlier.push(v);
                    }
                }
            }
            let mut reply = json!({ "methodResponses": responses, "sessionState": self.state_name() });
            if request.get("createdIds").is_some() {
                let mut ids = request["createdIds"].clone();
                for v in &earlier[carried..] {
                    for (k, c) in v["created"].as_object().into_iter().flatten() {
                        ids[k] = c["id"].clone();
                    }
                }
                reply["createdIds"] = ids;
            }
            return (200, reply.to_string().into_bytes());
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
            if let Some(to) = self.blob_redirects.get(blob) {
                return (302, to.clone().into_bytes());
            }
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
        // A redirect's reply is where it goes.
        let (location, reply) = if (300..400).contains(&status) {
            (format!("Location: {}\r\n", String::from_utf8_lossy(&reply)), Vec::new())
        } else {
            (String::new(), reply)
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
        ..Default::default()
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
    let mut found = b.jmap_shared(&a)?;
    found.sort();
    // RFC 8620 §2: isPersonal and isReadOnly show in the names.
    assert_eq!(
        found,
        vec![
            ("u2".to_owned(), "Team (shared)".to_owned()),
            ("u3".to_owned(), "Archive (shared, read-only)".to_owned()),
        ]
    );
    let team = b.add_jmap_shared(&a, "u2", "Team")?;
    assert_eq!(team.jmap_account, "u2");
    assert_eq!(team.credentials_of, a.id);
    assert_eq!(team.label(), "Team");
    assert_eq!(b.jmap_shared(&a)?.len(), 1, "Team already added");
    // A read-only account's changes are refused before they're sent.
    let archive = b.add_jmap_shared(&a, "u3", "Archive")?;
    let error = b
        .with_jmap(&archive, |s| s.destroy(&["M1".to_owned()]))
        .unwrap_err();
    assert!(error.downcast_ref::<crate::sync::jmap::ReadOnly>().is_some(), "{error:#}");
    b.remove_account(&archive.id)?;
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
        // Picacho invites by mail: the server is told to send nothing.
        assert!(s.scheduling.iter().all(|(sends, _)| !sends), "{:?}", s.scheduling);
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
    // RFC 8621 §7: a read receipt's `displayed`.
    assert_eq!(status.displayed, ["cy@example.net"]);
    // Its record goes once no longer needed; a pending one stays.
    b.with_jmap(&a, |s| s.forget_submissions(&["held1".to_owned()]))?;
    assert!(!server.lock().unwrap().undo.contains_key("held1"));
    server.lock().unwrap().undo.insert("held2".into(), "pending".into());
    assert!(b.with_jmap(&a, |s| s.forget_submissions(&["held2".to_owned()])).is_err());
    let changes = b.with_jmap(&a, |s| s.pending_submission_changes("sq1"))?.unwrap();
    assert_eq!((changes.removed, changes.added), (vec!["held0".to_owned()], vec![("held9".to_owned(), 0)]));
    // Sent without keeping a copy: the message is gone once sent.
    let before = server.lock().unwrap().emails.len();
    let options = crate::sync::jmap::SendOptions { keep_no_copy: true, ..Default::default() };
    b.send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &options)?;
    assert_eq!(server.lock().unwrap().emails.len(), before);
    Ok(())
}

/// Tasks over JMAP (JMAP for Tasks): the open ones in the task lists are
/// read, and added, edited, done and deleted on the server; an account
/// without a task list gets one for a new task.
#[test]
fn jmap_tasks_read_and_written() -> Result<()> {
    let fixture = Server {
        modern: true,
        tasks_on: true,
        task_lists: vec![
            json!({ "id": "l0", "name": "Groceries", "isDefault": false, "isSubscribed": true, "myRights": { "mayWrite": true } }),
            json!({ "id": "l1", "name": "Work", "isDefault": true, "isSubscribed": true, "myRights": { "mayWrite": true } }),
            json!({ "id": "hid", "name": "Hidden", "isSubscribed": true, "isVisible": false, "myRights": { "mayWrite": true } }),
            json!({ "id": "un", "name": "Team", "isSubscribed": false, "myRights": { "mayWrite": false } }),
        ],
        tasks: vec![
            json!({ "@type": "Task", "id": "T1", "uid": "taxes@x", "taskListIds": { "l1": true }, "title": "Taxes",
                    "due": "2099-11-30T17:00:00", "timeZone": "Europe/Berlin", "priority": 1,
                    "alerts": { "a1": { "@type": "Alert", "trigger": { "@type": "OffsetTrigger", "offset": "-PT1H", "relativeTo": "end" } } } }),
            json!({ "@type": "Task", "id": "T2", "uid": "old@x", "taskListIds": { "l1": true }, "title": "Old", "progress": "completed" }),
            json!({ "@type": "Task", "id": "T3", "uid": "hid@x", "taskListIds": { "hid": true }, "title": "Hidden" }),
            json!({ "@type": "Task", "id": "T4", "uid": "team@x", "taskListIds": { "un": true }, "title": "Team" }),
        ],
        ..Default::default()
    };
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    // Connecting reads the tasks too: only the open one in a list shown.
    b.connect_jmap_calendar(&a)?;
    let tasks = b.calendar_tasks();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!((tasks[0].event.summary.as_str(), tasks[0].href.as_str(), tasks[0].calendar.as_str()), ("Taxes", "jmap:T1", "jmap:l1"));
    assert!(tasks[0].event.task);
    // The default list first; hidden and unsubscribed lists aren't kept.
    let lists: Vec<String> = b.task_lists().into_iter().map(|l| l.href).collect();
    assert_eq!(lists, ["jmap:l1", "jmap:l0"]);
    assert_eq!(b.task_calendar().unwrap().href, "jmap:l1");
    assert_eq!(b.sync_tasks()?, 1);
    // Edited: only what Picacho edits changes.
    let due = tasks[0].event.start + 86_400;
    b.edit_task("taxes@x", "Taxes 2026", "Forms", due, false)?;
    {
        let s = server.lock().unwrap();
        let t = s.tasks.iter().find(|t| t["id"] == "T1").unwrap();
        assert_eq!((&t["title"], &t["description"], &t["due"]), (&json!("Taxes 2026"), &json!("Forms"), &json!("2099-12-01T17:00:00")));
        assert_eq!((&t["timeZone"], &t["priority"]), (&json!("Europe/Berlin"), &json!(1)));
    }
    // Its due date removed: the server takes it (§4.6, §5), without the
    // zone and the alert before the due time.
    b.edit_task("taxes@x", "Taxes 2026", "Forms", 0, false)?;
    {
        let s = server.lock().unwrap();
        let t = s.tasks.iter().find(|t| t["id"] == "T1").unwrap();
        assert!(t.get("due").is_none() && t.get("timeZone").is_none(), "{t}");
        assert_eq!(t["alerts"], json!({}));
    }
    // Added in the default list, then done, then deleted.
    let added = b.add_task("Call Bo", 0)?;
    assert_eq!(added.calendar, "jmap:l1");
    let id = added.href.strip_prefix("jmap:").unwrap().to_owned();
    {
        let s = server.lock().unwrap();
        let t = s.tasks.iter().find(|t| t["id"] == id.as_str()).unwrap();
        assert_eq!((&t["@type"], &t["taskListIds"], &t["title"]), (&json!("Task"), &json!({ "l1": true }), &json!("Call Bo")));
        assert!(t.get("due").is_none());
    }
    assert_eq!(b.calendar_tasks().len(), 2);
    b.complete_task(&added.event.uid, true)?;
    assert_eq!(server.lock().unwrap().tasks.iter().find(|t| t["id"] == id.as_str()).unwrap()["progress"], "completed");
    assert_eq!(b.calendar_tasks().len(), 1);
    b.delete_task("taxes@x")?;
    assert!(!server.lock().unwrap().tasks.iter().any(|t| t["id"] == "T1"));
    assert!(b.calendar_tasks().is_empty());
    assert_eq!(b.sync_tasks()?, 0);
    // Only a hidden list to write to: not used, and none made beside it.
    server.lock().unwrap().task_lists.retain(|l| l["id"] == "hid" || l["id"] == "un");
    b.sync_tasks()?;
    assert!(b.add_task("Water plants", 0).is_err());
    assert_eq!(server.lock().unwrap().task_lists.len(), 2);
    // No list to put it in: one is made.
    server.lock().unwrap().task_lists.clear();
    b.sync_tasks()?;
    assert!(b.task_calendar().is_none());
    let made = b.add_task("Water plants", 4_100_000_000)?;
    assert_eq!(made.calendar, "jmap:L1");
    assert_eq!(server.lock().unwrap().task_lists[0]["name"], "Tasks");
    Ok(())
}

/// Tasks over JMAP once read follow changes (Task/changes); a repeating
/// task is done one occurrence at a time; task alerts are reminders; and
/// tasks others changed (TaskNotification) are told like calendars'.
#[test]
fn jmap_tasks_follow_changes_repeat_remind_and_notify() -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let utc = |t: i64| chrono::DateTime::from_timestamp(t, 0).unwrap().format("%Y-%m-%dT%H:%M:%S").to_string();
    // A weekly task that began three weeks ago, at 09:00 UTC.
    let first = (now - 21 * 86_400) / 86_400 * 86_400 + 9 * 3600;
    let fixture = Server {
        modern: true,
        tasks_on: true,
        task_lists: vec![json!({ "id": "l1", "name": "Home", "isDefault": true, "myRights": { "mayWrite": true } })],
        tasks: vec![
            json!({ "@type": "Task", "id": "T1", "uid": "taxes@x", "taskListIds": { "l1": true }, "title": "Taxes",
                    "due": utc(now + 1800), "timeZone": "Etc/UTC",
                    "alerts": { "a1": { "@type": "Alert", "trigger": { "@type": "OffsetTrigger", "offset": "-PT1H", "relativeTo": "end" } } } }),
            json!({ "@type": "Task", "id": "T5", "uid": "bins@x", "taskListIds": { "l1": true }, "title": "Bins",
                    "start": utc(first), "due": utc(first + 3600), "timeZone": "Etc/UTC",
                    "recurrenceRule": { "@type": "RecurrenceRule", "frequency": "weekly" } }),
        ],
        task_notifications: vec![json!({ "id": "n1", "type": "updated", "taskId": "T1",
            "changedBy": { "name": "Bo", "email": "bo@example.se", "principalId": null },
            "task": { "@type": "Task", "title": "Taxes" }, "taskPatch": { "description": "Forms" } })],
        ..Default::default()
    };
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_calendar(&a)?;
    // Told once, then gone from the server.
    assert!(b.calendar_notices().iter().any(|n| n == "Bo changed Taxes."), "{:?}", b.calendar_notices());
    assert!(server.lock().unwrap().task_notifications.is_empty());
    let bins = |b: &Backend| b.calendar_tasks().into_iter().find(|t| t.event.uid == "bins@x").unwrap();
    // The latest occurrence already due: three weeks on, or two.
    let shown = bins(&b).event.recurrence_id.unwrap();
    let latest_due = first + (now - first - 3600) / (7 * 86_400) * 7 * 86_400;
    assert_eq!(shown, latest_due);
    // The task alert an hour before it's due, half an hour ago: a reminder.
    let reminders = b.due_reminders(now);
    assert!(reminders.iter().any(|(e, _)| e.summary == "Taxes"), "{reminders:?}");
    // Changed on the server: read by Task/changes, not all again.
    {
        let mut s = server.lock().unwrap();
        let t = s.tasks.iter_mut().find(|t| t["id"] == "T1").unwrap();
        t["title"] = json!("Taxes 2026");
        s.task_state += 1;
        let state = s.task_state;
        s.task_log.push((state, "T1".into(), false));
        s.calls.clear();
    }
    assert_eq!(b.sync_tasks()?, 2);
    {
        let s = server.lock().unwrap();
        assert!(s.calls.contains(&"Task/changes".to_owned()), "{:?}", s.calls);
        assert!(!s.calls.contains(&"Task/query".to_owned()), "only changes: {:?}", s.calls);
    }
    assert!(b.calendar_tasks().iter().any(|t| t.event.summary == "Taxes 2026"));
    // The repeating one done: that occurrence alone, and the next listed.
    b.complete_task("bins@x", true)?;
    let key = utc(shown);
    {
        let s = server.lock().unwrap();
        let t = s.tasks.iter().find(|t| t["id"] == "T5").unwrap();
        assert_eq!(t["recurrenceOverrides"][&key]["progress"], "completed", "{t}");
        assert!(t.get("progress").is_none(), "the series isn't done");
    }
    let next = bins(&b).event.recurrence_id.unwrap();
    assert_eq!(next, shown + 7 * 86_400);
    // Again: patched into the overrides now there.
    b.complete_task("bins@x", true)?;
    assert_eq!(bins(&b).event.recurrence_id, Some(next + 7 * 86_400));
    assert_eq!(server.lock().unwrap().tasks.iter().find(|t| t["id"] == "T5").unwrap()["recurrenceOverrides"].as_object().unwrap().len(), 2);
    Ok(())
}

/// An account that may not make task lists (`mayCreateTaskList`) isn't
/// asked to.
#[test]
fn jmap_task_lists_are_made_only_where_allowed() -> Result<()> {
    let (cert, port, server, _running) =
        start(Server { modern: true, tasks_on: true, may_create_task_list: Some(false), ..Default::default() })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_calendar(&a)?;
    let error = b.add_task("Taxes", 0).unwrap_err().to_string();
    assert!(error.contains("make task lists"), "{error}");
    assert!(!server.lock().unwrap().calls.contains(&"TaskList/set".to_owned()));
    Ok(())
}

/// A JMAP server without JMAP for Tasks has no tasks, and says so when
/// one is added.
#[test]
fn jmap_without_tasks_has_none() -> Result<()> {
    let (cert, port, _server, _running) = start(Server { modern: true, ..Default::default() })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_calendar(&a)?;
    assert_eq!(b.sync_tasks()?, 0);
    assert!(b.task_calendar().is_none());
    assert!(b.add_task("Taxes", 0).is_err());
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

/// Copies, blob copies, held submissions and dismissed notices go in
/// pieces the server takes (RFC 8620 §5.1, §5.3: `maxObjectsInGet` and
/// `maxObjectsInSet`), however many there are.
#[test]
fn jmap_batches_respect_the_object_limits() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        max_objects: 2,
        ..Default::default()
    };
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    for i in 0..5 {
        fixture.emails.push(Message {
            id: format!("M{i}"),
            blob: "B0".into(),
            mailboxes: vec!["i".into()],
            keywords: vec![],
            subject: format!("Message {i}"),
            received: "2026-10-24T10:00:00Z".into(),
        });
        fixture.undo.insert(format!("held{i}"), "pending".into());
    }
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    let ids: Vec<String> = (0..5).map(|i| format!("M{i}")).collect();
    let copied = b.with_jmap(&a, |s| s.copy_from("u2", &ids, "w", &[]))?;
    assert_eq!(copied.len(), 5);
    let blobs: Vec<String> = (0..5).map(|i| format!("B{i}")).collect();
    let copied = b.with_jmap(&a, |s| s.copy_blobs("u2", &blobs))?;
    assert_eq!(copied.len(), 5);
    assert_eq!(b.held_submissions(&a)?.len(), 5);
    let notices: Vec<crate::sync::jscal::Notice> = (0..5)
        .map(|i| crate::sync::jscal::Notice {
            id: format!("n{i}"),
            kind: "CalendarEventNotification".into(),
            text: String::new(),
        })
        .collect();
    b.with_jmap(&a, |s| crate::sync::jscal::dismiss_notices(s, &notices))?;
    let s = server.lock().unwrap();
    let count = |m: &str| s.calls.iter().filter(|c| *c == m).count();
    assert_eq!(count("Email/copy"), 3);
    assert_eq!(count("Blob/copy"), 3);
    assert_eq!(count("CalendarEventNotification/set"), 3);
    assert!(count("EmailSubmission/get") >= 3, "{:?}", s.calls);
    Ok(())
}

/// RFC 8620 §2.2: the well-known address's redirect within the server is
/// followed and remembered, so the next sign-in starts where it ended; one
/// to another host is refused, naming it.
#[test]
fn jmap_session_redirects_are_followed_and_kept() -> Result<()> {
    let (cert, port, server, _running) = start(Server::default())?;
    server.lock().unwrap().session_redirect = Some(format!("https://localhost:{port}/jmap/session"));
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.with_jmap(&a, |s| s.identities().map(drop))?;
    {
        let mut s = server.lock().unwrap();
        assert_eq!(s.paths[..2], ["/.well-known/jmap", "/jmap/session"]);
        s.paths.clear();
    }
    // Signed in again: straight to where the redirect went.
    b.jmap.lock().unwrap().clear();
    b.with_jmap(&a, |s| s.identities().map(drop))?;
    assert_eq!(server.lock().unwrap().paths[0], "/jmap/session");
    // Another host (an IP address isn't the name entered): refused.
    server.lock().unwrap().session_redirect = Some(format!("https://127.0.0.1:{port}/jmap/session"));
    let (b2, mut other, path2) = backend(&cert)?;
    let _cleanup2 = Cleanup(path2);
    other.provider = Provider::Jmap;
    other.incoming_port = port;
    let error = format!("{:#}", b2.check_jmap(&other, &b2.test_secret.clone().unwrap()).unwrap_err());
    assert!(error.contains("127.0.0.1") && error.contains("enter that as the server"), "{error}");
    Ok(())
}

/// A download redirected to another host (a CDN's presigned address) is
/// followed without the account's credentials.
#[test]
fn jmap_blob_redirects_drop_credentials_off_the_origin() -> Result<()> {
    let mut fixture = Server::default();
    fixture.blobs.insert("B-cdn".into(), MESSAGE.to_vec());
    let (cert, port, server, _running) = start(fixture)?;
    server
        .lock()
        .unwrap()
        .blob_redirects
        .insert("B-cdn".into(), format!("https://127.0.0.1:{port}/cdn/B-cdn"));
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let bytes = b.with_jmap(&a, |s| s.download("B-cdn", "m.eml", "message/rfc822", 1 << 20))?;
    assert_eq!(bytes, MESSAGE);
    assert_eq!(server.lock().unwrap().cdn_auth, [""], "no Authorization off the origin");
    // Back on the origin, the credentials go again.
    server
        .lock()
        .unwrap()
        .blob_redirects
        .insert("B-cdn".into(), format!("https://localhost:{port}/download/u1/B-1/m.eml"));
    server.lock().unwrap().blobs.insert("B-1".into(), b"one".to_vec());
    let bytes = b.with_jmap(&a, |s| s.download("B-cdn", "m.eml", "message/rfc822", 1 << 20))?;
    assert_eq!(bytes, b"one");
    Ok(())
}

/// More calls than the server takes in one request: a creation id made in
/// one request is named in the next, carried in `createdIds` (RFC 8620
/// §3.3).
#[test]
fn jmap_creation_ids_cross_requests() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let blob = b.with_jmap(&a, |s| s.upload(MESSAGE, "message/rfc822"))?;
    // The server takes two calls a time: the submission goes in a second
    // request, naming the import's `#m`.
    let answers = b.with_jmap(&a, |s| {
        s.call(&[
            ("Email/import", json!({ "accountId": "u1", "emails": { "m": {
                "blobId": blob, "mailboxIds": { "d": true }, "keywords": {} } } })),
            ("Identity/get", json!({ "accountId": "u1" })),
            ("EmailSubmission/set", json!({ "accountId": "u1", "create": { "s": {
                "identityId": "id1", "emailId": "#m",
                "envelope": { "mailFrom": { "email": "qa@example.com" }, "rcptTo": [{ "email": "r@example.com" }] } } } })),
        ])
    })?;
    assert_eq!(answers.len(), 3);
    assert_eq!(answers[2].value()?["created"]["s"]["id"], "sub1");
    assert_eq!(server.lock().unwrap().sent.len(), 1);
    Ok(())
}

/// A card is changed with `ifInState` (RFC 8620 §5.3): another card's change
/// in between moves the state, and the change is tried again since this card
/// is as it was; this card's own change in between is a conflict.
#[test]
fn jmap_card_changes_use_if_in_state() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        dav_changes: true,
        ..Default::default()
    };
    fixture.cards.push(json!({ "@type": "Card", "id": "K0", "uid": "bo", "name": { "full": "Bo" }, "updated": "1" }));
    fixture.cards.push(json!({ "@type": "Card", "id": "K1", "uid": "cy", "name": { "full": "Cy" }, "updated": "1" }));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_contacts(&a)?;
    // Saved as read: one ContactCard/set, with the state it was read in.
    let mut bo = b.contacts().into_iter().find(|c| c.name == "Bo").unwrap();
    bo.note = "One".into();
    let bo = b.save_contact(bo)?;
    assert_eq!(server.lock().unwrap().cards[0]["notes"]["n1"]["note"], "One");
    // Another card changes on the server: the state moves, this card is
    // still as read, so the change goes through on the second try.
    {
        let mut s = server.lock().unwrap();
        s.dav_state += 1;
        s.calls.clear();
    }
    let mut again = bo.clone();
    again.note = "Two".into();
    b.save_contact(again)?;
    {
        let s = server.lock().unwrap();
        assert_eq!(s.cards[0]["notes"]["n1"]["note"], "Two");
        assert_eq!(s.calls.iter().filter(|c| *c == "ContactCard/set").count(), 2, "{:?}", s.calls);
    }
    Ok(())
}

/// Mailboxes (RFC 8621 §2): rights, subscription and thread counts are
/// read; a folder is moved under another and subscribed to; a folder the
/// sign-in may not rename is refused before anything is sent; and a folder
/// with mail is deleted with it only when asked (`onDestroyRemoveEmails`).
#[test]
fn jmap_mailbox_rights_moves_and_removal() -> Result<()> {
    let mut fixture = Server {
        boxes: Some(vec![
        json!({ "id": "i", "name": "Inbox", "role": "inbox", "parentId": null, "totalThreads": 3, "unreadThreads": 1,
                "myRights": { "mayRename": false, "mayDelete": false, "mayCreateChild": true, "mayAddItems": true } }),
        json!({ "id": "w", "name": "Work", "parentId": null, "isSubscribed": true }),
        json!({ "id": "r", "name": "Reports", "parentId": null, "myRights": { "mayAddItems": false } }),
        json!({ "id": "p", "name": "Projects", "parentId": null, "isSubscribed": false }),
        ]),
        ..Default::default()
    };
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    fixture.emails.push(Message {
        id: "M1".into(),
        blob: "B0".into(),
        mailboxes: vec!["p".into()],
        keywords: vec![],
        subject: "Plans".into(),
        received: "2026-10-24T10:00:00Z".into(),
    });
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.with_jmap(&a, |s| {
        let boxes = s.reload_mailboxes()?;
        let inbox = boxes.iter().find(|m| m.id == "i").unwrap();
        assert_eq!((inbox.total_threads, inbox.unread_threads), (3, 1));
        assert!(!inbox.may_rename() && !inbox.may_delete() && inbox.may_create_child());
        assert_eq!(boxes.iter().find(|m| m.id == "p").unwrap().is_subscribed, Some(false));
        // Refused here: the server isn't asked.
        let before = server.lock().unwrap().calls.len();
        assert!(s.rename_mailbox("i", "Post").is_err());
        assert!(s.move_mailbox("i", Some("w")).is_err());
        assert!(s.destroy_mailbox_with("i", true).is_err());
        assert_eq!(server.lock().unwrap().calls.len(), before);
        // A folder that takes no messages: refused here.
        let mut patch = serde_json::Map::new();
        patch.insert("M1".into(), json!({ "mailboxIds/r": true }));
        let e = s.update(patch).unwrap_err();
        assert!(e.to_string().contains("Reports"), "{e}");
        // With mail in it, unless its mail goes too: refused.
        assert!(s.destroy_mailbox_with("p", false).is_err());
        Ok(())
    })?;
    // Moved under Work, its path changing with it, and subscribed to.
    let change = b.move_folder_jmap(&a, "Projects", Some("Work"))?;
    assert_eq!(change.moved, [("Projects".to_owned(), "Work/Projects".to_owned())]);
    assert!(b.move_folder_jmap(&a, "Work", Some("Work/Projects")).is_err(), "not inside itself");
    assert!(!b.subscribed_folders_jmap(&a)?.contains(&"Work/Projects".to_owned()));
    b.subscribe_folder_jmap(&a, "Work/Projects", true)?;
    // This fixture has no Mailbox/queryChanges: listed again.
    assert!(b.subscribed_folders_jmap(&a)?.contains(&"Work/Projects".to_owned()));
    assert!(server.lock().unwrap().calls.contains(&"Mailbox/queryChanges".to_owned()));
    // Deleted with the mail only it held.
    let left = b.delete_remote_folder(&a, "Work/Projects")?;
    assert!(left.iter().all(|f| f.name != "Work/Projects"));
    assert!(server.lock().unwrap().emails.is_empty());
    Ok(())
}

/// Identities are read with their Reply-To, Bcc and signatures (RFC 8621
/// §6), and once read, only what changed is asked (Identity/changes).
#[test]
fn jmap_identities_read_their_defaults_and_changes() -> Result<()> {
    let (cert, port, server, _running) = start(Server::default())?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let list = b.jmap_identities(&a)?;
    assert_eq!(list.len(), 1);
    assert_eq!(crate::sync::jmap::addresses(&list[0].reply_to), "QA desk <desk@example.com>");
    assert_eq!(list[0].text_signature, "-- \nQA");
    assert_eq!(list[0].html_signature, "<p>QA</p>");
    server.lock().unwrap().extra_identities.push(json!({ "id": "id2", "name": "Ops", "email": "ops@example.com" }));
    let before = server.lock().unwrap().calls.len();
    let list = b.jmap_identities(&a)?;
    assert_eq!(list.len(), 2);
    let calls = server.lock().unwrap().calls[before..].to_vec();
    assert_eq!(calls, ["Identity/changes", "Identity/get"]);
    // Nothing changed: no Identity/get at all.
    let before = server.lock().unwrap().calls.len();
    assert_eq!(b.jmap_identities(&a)?.len(), 2);
    assert_eq!(server.lock().unwrap().calls[before..], ["Identity/changes"]);
    Ok(())
}

/// Server scheduling for JMAP calendars is opt-in. Off (the default), the
/// server is told to send nothing and Picacho emails the guests, as before.
/// On, the server sends for events organized here and Picacho emails
/// nothing; never both for one change. Invitations from others are still
/// answered by mail, the server sending nothing.
#[test]
fn jmap_calendar_scheduling_uses_one_channel() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    b.connect_jmap_calendar(&a)?;
    let lunch = |uid: &str, organizer: &str| {
        let mut e = crate::sync::ical::Event {
            uid: uid.into(),
            summary: "Lunch".into(),
            start: 4_100_000_000,
            end: 4_100_003_600,
            utc: true,
            ..Default::default()
        };
        e.organizer = Some(crate::sync::ical::Attendee { email: organizer.into(), ..Default::default() });
        e.attendees.push(crate::sync::ical::Attendee { email: "bo@example.se".into(), ..Default::default() });
        e
    };
    let mails = |server: &Arc<Mutex<Server>>| server.lock().unwrap().sent.len();
    let last = |server: &Arc<Mutex<Server>>| server.lock().unwrap().scheduling.last().cloned().unwrap();

    // Off: byte-for-byte as before. The server sends nothing, participants
    // are client-scheduled, and Picacho emails the guest.
    b.save_event(lunch("off@x", "qa@example.com"), None)?;
    assert_eq!(last(&server), (false, vec!["client".to_owned(), "client".to_owned()]));
    let before = mails(&server);
    assert_eq!(b.send_invitations(&a, "off@x")?, 1);
    assert_eq!(mails(&server), before + 1, "invited by mail");
    b.delete_event("off@x")?;
    assert!(!last(&server).0);

    // A server that doesn't name the addresses it sends as: refused.
    let error = format!("{:#}", b.set_server_scheduling(true).unwrap_err());
    assert!(error.contains("keeps emailing"), "{error}");
    server.lock().unwrap().participant_identities = true;
    b.set_server_scheduling(true)?;

    // On, organized here: the server sends; Picacho emails nothing.
    let before = mails(&server);
    b.save_event(lunch("on@x", "qa@example.com"), None)?;
    assert_eq!(last(&server), (true, vec!["server".to_owned(), "server".to_owned()]));
    assert_eq!(b.send_invitations(&a, "on@x")?, 1);
    // Changed: the server again.
    let mut changed = b.calendar_events().into_iter().find(|s| s.event.uid == "on@x").unwrap().event;
    changed.summary = "Long lunch".into();
    b.save_event(changed, None)?;
    assert!(last(&server).0);
    // Cancelled: the server tells them.
    assert_eq!(b.cancel_invitations(&a, "on@x")?, 1);
    b.delete_event("on@x")?;
    assert!(last(&server).0);
    assert_eq!(mails(&server), before, "no mail while the server schedules");

    // Someone else's event (an invitation answered here): the server
    // sends nothing for it; the answer goes by mail.
    b.save_event(lunch("theirs@x", "ana@example.org"), None)?;
    assert_eq!(last(&server), (false, vec!["client".to_owned(), "client".to_owned()]));
    Ok(())
}

/// The mail account's limits (RFC 8621 §1.3.1) are said before anything is
/// sent: folder depth, top-level folders, name length, attachments' size
/// and how many folders a message may be in.
#[test]
fn jmap_mail_limits_are_honoured() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        mail_caps: Some(json!({
            "maxMailboxesPerEmail": 1, "maxMailboxDepth": 2, "maxSizeMailboxName": 100,
            "maxSizeAttachmentsPerEmail": 1000, "mayCreateTopLevelMailbox": false,
        })),
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let big = format!(
        "From: qa@example.com\r\nTo: r@example.com\r\nSubject: Big\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nHi\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=x.bin\r\n\r\n{}\r\n--b--\r\n",
        "x".repeat(4000)
    );
    b.with_jmap(&a, |s| {
        s.reload_mailboxes()?;
        let before = server.lock().unwrap().calls.len();
        assert!(s.create_mailbox("Top", None).is_err(), "not at the top");
        assert!(s.create_mailbox(&"n".repeat(101), Some("w")).is_err(), "name too long");
        assert!(s.import(MESSAGE, &["i", "w"], &[]).is_err(), "one folder per message");
        let mut patch = serde_json::Map::new();
        patch.insert("M1".into(), json!({ "mailboxIds": { "i": true, "w": true } }));
        assert!(s.update(patch).is_err());
        let options = crate::sync::jmap::SendOptions::default();
        let e = s.send("id1", "qa@example.com", &["r@example.com".into()], big.as_bytes(), Some("d"), Some("s"), &options).unwrap_err();
        assert!(e.to_string().contains("attachments"), "{e}");
        assert_eq!(server.lock().unwrap().calls.len(), before, "nothing was asked of the server");
        // Within the limits: made.
        let id = s.create_mailbox("Inner", Some("w"))?;
        // Inner is 2 deep; one inside it would be 3.
        assert!(s.create_mailbox("Deeper", Some(&id)).is_err());
        Ok(())
    })?;
    Ok(())
}

/// Email/parse with bodyValues (RFC 8621 §4.9): the start of an attached
/// message's text, not only its preview.
#[test]
fn jmap_parsed_messages_read_their_text() -> Result<()> {
    let mut fixture = Server::default();
    fixture.blobs.insert("B-att".into(), MESSAGE.to_vec());
    let (cert, port, _server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let (e, text) = b.with_jmap(&a, |s| s.parse_blob_with_text("B-att", 4096))?.unwrap();
    assert_eq!(e.subject.as_deref(), Some("Attached"));
    assert_eq!(text, "The whole first paragraph of the attached message.");
    let (_, cut) = b.with_jmap(&a, |s| s.parse_blob_with_text("B-att", 9))?.unwrap();
    assert_eq!(cut, "The whole");
    let (plain, none) = b.with_jmap(&a, |s| s.parse_blob_with_text("B-att", 0))?.unwrap();
    assert_eq!(plain.preview.as_deref(), Some("Short preview"));
    assert!(none.is_empty());
    assert!(b.with_jmap(&a, |s| s.parse_blob_with_text("B-none", 0))?.is_none());
    Ok(())
}

/// Read receipts over JMAP (RFC 9007): MDN/send carries the whole report
/// (reporting program, the Original-Recipient noted, the disposition), and
/// MDN/parse reads one received.
#[test]
fn jmap_read_receipts_carry_every_field() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        ..Default::default()
    };
    let raw = b"From: Ana <ana@example.org>\r\nTo: qa@example.com\r\nOriginal-Recipient: rfc822;qa@example.com\r\nDisposition-Notification-To: ana@example.org\r\nSubject: Please confirm\r\nMessage-ID: <m1@example.com>\r\n\r\nHello\r\n";
    fixture.blobs.insert("B0".into(), raw.to_vec());
    fixture.emails.push(Message {
        id: "M1".into(),
        blob: "B0".into(),
        mailboxes: vec!["i".into()],
        keywords: vec![],
        subject: "Please confirm".into(),
        received: "2026-10-24T10:00:00Z".into(),
    });
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.store.save_account(&a)?;
    let mut m = b.sync(&a, Folder::Inbox, 50)?.remove(0);
    m.set_receipt_to("ana@example.org");
    b.send_receipt(&a, &m)?;
    {
        let s = server.lock().unwrap();
        let mdn = &s.mdns[0];
        assert_eq!(mdn["forEmailId"], "M1");
        assert_eq!(mdn["disposition"]["type"], "displayed");
        assert_eq!(mdn["disposition"]["actionMode"], "manual-action");
        assert_eq!(mdn["originalRecipient"], "rfc822;qa@example.com");
        assert!(mdn["reportingUA"].as_str().unwrap().starts_with("Picacho/"));
        assert!(s.emails[0].keywords.contains(&"$mdnsent".to_owned()));
        assert!(s.sent.is_empty(), "the server wrote it, not a submission");
    }
    let parsed = b.with_jmap(&a, |s| s.parse_receipts(&["R1".to_owned()]))?;
    let r = &parsed["R1"];
    assert_eq!(r.for_email_id.as_deref(), Some("M1"));
    assert_eq!(r.disposition, "displayed");
    assert!(r.automatic);
    assert_eq!(r.reporting_ua.as_deref(), Some("Other/2"));
    Ok(())
}

/// Sieve scripts over JMAP (RFC 9661): found by query, renamed, removed
/// (not the active one), and a script larger than the server keeps is
/// refused before it is uploaded.
#[test]
fn jmap_sieve_query_rename_destroy_and_limits() -> Result<()> {
    let mut fixture = Server {
        sieve_max_size: Some(40),
        ..Default::default()
    };
    fixture.blobs.insert("B-v".into(), b"require \"fileinto\";".to_vec());
    fixture.scripts.push(("V".into(), "vendor".into(), "B-v".into(), true));
    fixture.scripts.push(("O".into(), "old".into(), "B-v".into(), false));
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.with_jmap(&a, |s| {
        assert_eq!(s.query_sieve(json!({ "isActive": true }))?, ["V"]);
        s.sieve_rename("O", "older")?;
        assert!(s.sieve_rename("O", "vendor").is_err(), "the name is taken");
        assert!(s.sieve_destroy("V").is_err(), "the active one stays");
        s.sieve_destroy("O")?;
        let before = server.lock().unwrap().blobs.len();
        let e = s.sieve_upload(&"x".repeat(41)).unwrap_err();
        assert!(e.to_string().contains("larger"), "{e}");
        assert_eq!(server.lock().unwrap().blobs.len(), before, "not uploaded");
        Ok(())
    })?;
    let s = server.lock().unwrap();
    assert_eq!(s.scripts.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["vendor"]);
    Ok(())
}

/// Storage is read once, then followed by Quota/changes (RFC 9425 §4.2):
/// unchanged, nothing more is asked; changed, only that quota.
#[test]
fn jmap_quota_follows_changes() -> Result<()> {
    let (cert, port, server, _running) = start(Server::default())?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    assert_eq!(b.quota(&a)?.unwrap().used_kib, 2);
    let before = server.lock().unwrap().calls.len();
    assert_eq!(b.quota(&a)?.unwrap().used_kib, 2);
    assert_eq!(server.lock().unwrap().calls[before..], ["Quota/changes"]);
    {
        let mut s = server.lock().unwrap();
        s.quota_used = Some(4096);
        s.quota_state += 1;
    }
    assert_eq!(b.quota(&a)?.unwrap().used_kib, 4);
    assert_eq!(b.with_jmap(&a, |s| s.query_quotas(&["Mail"]))?, ["q"]);
    Ok(())
}

/// The Blob extension (RFC 9404): a range read is checked against the
/// digest the server gives; Blob/lookup says what uses a blob; data larger
/// than Blob/upload takes goes by the ordinary upload.
#[test]
fn jmap_blob_digests_lookup_and_limits() -> Result<()> {
    let mut fixture = Server {
        modern: true,
        ..Default::default()
    };
    fixture.blobs.insert("B0".into(), MESSAGE.to_vec());
    fixture.emails.push(Message {
        id: "M1".into(),
        blob: "B0".into(),
        mailboxes: vec!["i".into()],
        keywords: vec![],
        subject: "Over JMAP".into(),
        received: "2026-10-24T10:00:00Z".into(),
    });
    let (cert, port, server, _running) = start(fixture)?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    b.with_jmap(&a, |s| {
        let head = s.header_block("M1")?.unwrap();
        assert!(head.starts_with(b"From: Sophie Chen"));
        server.lock().unwrap().bad_digest = true;
        let e = s.header_block("M1").unwrap_err();
        assert!(e.to_string().contains("digest"), "{e}");
        let used = s.lookup_blobs(&["B0".to_owned()], &["Email"])?;
        assert_eq!(used["B0"]["Email"], ["M1"]);
        // Over maxSizeBlobSet (1000 bytes here): uploaded, not inline.
        let before = server.lock().unwrap().calls.len();
        s.upload_inline(&[b'x'; 2000], "application/octet-stream")?;
        assert!(!server.lock().unwrap().calls[before..].contains(&"Blob/upload".to_owned()));
        Ok(())
    })?;
    Ok(())
}

/// Calendars and contacts over JMAP beyond events and cards: the lists'
/// own /changes, /queryChanges, copies from a shared account,
/// CalendarEvent/parse, account limits, sharing, and the directory of
/// people (RFC 9670).
#[test]
fn jmap_calendars_and_contacts_changes_copy_parse_and_sharing() -> Result<()> {
    let (cert, port, server, _running) = start(Server {
        modern: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    use crate::sync::jscal;
    b.with_jmap(&a, |s| {
        let c = jscal::collection_changes(s, "Calendar", "c1")?.unwrap();
        assert_eq!((c.changed, c.destroyed, c.state), (vec!["c9".to_owned()], vec!["c1".to_owned()], "c2".to_owned()));
        assert!(jscal::collection_changes(s, "AddressBook", "b1")?.is_some());
        assert!(jscal::collection_changes(s, "ShareNotification", "n1")?.is_some());
        // A server without ParticipantIdentity/changes: read again.
        assert!(jscal::collection_changes(s, "ParticipantIdentity", "x")?.is_none());
        let q = jscal::query_changes(s, "CalendarEvent", json!({}), "q1")?.unwrap();
        assert_eq!((q.removed, q.added), (vec!["E0".to_owned()], vec![("E9".to_owned(), 0)]));
        let copied = jscal::copy_from(s, "ContactCard", "u2", &["K5".to_owned()], "b1")?;
        assert_eq!(copied["K5"], "copy-K5");
        let parsed = jscal::parse_events(s, &["ics1".to_owned()])?;
        assert_eq!(parsed["ics1"][0].summary, "Parsed");
        let mut with = serde_json::Map::new();
        with.insert("P1".into(), json!({ "mayReadItems": true, "mayWriteAll": false }));
        jscal::share_with(s, "Calendar", "c1", &with)?;
        let people = jscal::principals(s, None)?;
        assert_eq!((people[0].name.as_str(), people[0].email.as_str()), ("Bo Lindqvist", "bo@example.se"));
        assert_eq!(people[0].accounts, ["u2"]);
        let mut patch = serde_json::Map::new();
        patch.insert("description".into(), json!("On leave in December"));
        jscal::update_principal(s, "P1", &patch)?;
        Ok(())
    })?;
    let shared = server.lock().unwrap().shared_with.clone();
    assert_eq!(shared[0]["c1"]["shareWith"]["P1"]["mayReadItems"], true);
    assert_eq!(shared[1]["P1"]["description"], "On leave in December");
    let limits = jscal::collection_limits(&json!({ "maxCalendarsPerEvent": 1, "mayCreateCalendar": false }), "Calendar");
    assert_eq!(limits, jscal::CollectionLimits { max_per_item: Some(1), may_create: false });
    assert_eq!(jscal::collection_limits(&json!({}), "AddressBook"), jscal::CollectionLimits { max_per_item: None, may_create: true });
    Ok(())
}

/// Encrypted with Bcc over JMAP (RFC 5322 §3.6.3, RFC 8621 §7.5): as over
/// SMTP, To gets a copy encrypted to them, and each Bcc recipient a copy of
/// their own, encrypted to them alone; each submission's copy is destroyed
/// once sent, none has a Bcc field, and Sent gets one copy, with its Bcc.
#[test]
fn encrypted_bcc_over_jmap_goes_to_each_alone() -> Result<()> {
    use super::smtp_tests::{keyring_for, opens};
    let (cert, port, server, _running) = start(Server::default())?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let (ring, keys) = keyring_for(&["Ana <ana@example.com>", "Bo <bo@example.com>", "Di <di@example.com>"])?;
    b.set_keyring(ring)?;
    let d = Draft {
        id: "jmap-bcc".into(),
        account: a.id.clone(),
        to: "Ana <ana@example.com>".into(),
        bcc: "bo@example.com, di@example.com".into(),
        subject: "Sealed".into(),
        body: "Only for some.".into(),
        encrypt: true,
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.send(&a, &d)?;
    let s = server.lock().unwrap();
    let rcpts: Vec<Vec<String>> = s.sent.iter().map(|(_, _, to)| to.clone()).collect();
    assert_eq!(rcpts, [vec!["ana@example.com".to_owned()], vec!["bo@example.com".into()], vec!["di@example.com".into()]]);
    // The submitted copies are gone; one copy is left, in Sent.
    assert_eq!(s.emails.len(), 1, "{:?}", s.emails.iter().map(|e| &e.mailboxes).collect::<Vec<_>>());
    assert_eq!(s.emails[0].mailboxes, ["s"]);
    let filed = &s.blobs[&s.emails[0].blob];
    assert!(filed.starts_with(b"Bcc: "), "{}", String::from_utf8_lossy(&filed[..80]));
    // Each copy uploaded: only its recipient's key opens it, and it shows
    // no Bcc. Uploads are kept by blob id, in the order they were sent.
    let [ana, bo, di] = [&keys[0], &keys[1], &keys[2]];
    let copies: Vec<&Vec<u8>> = s.blobs.values().filter(|raw| !raw.starts_with(b"Bcc: ")).collect();
    assert_eq!(copies.len(), 3);
    let opened = |key| copies.iter().filter(|raw| opens(raw, key)).count();
    assert_eq!((opened(ana), opened(bo), opened(di)), (1, 1, 1));
    for raw in &copies {
        let text = String::from_utf8_lossy(raw);
        // Encrypted copies show only their outer headers: To, no Bcc.
        let headers = text.split("\r\n\r\n").next().unwrap_or_default().to_ascii_lowercase();
        assert!(!headers.starts_with("bcc:") && !headers.contains("\r\nbcc:"), "{headers}");
        assert!(text.contains("To: Ana <ana@example.com>"), "{text}");
    }
    Ok(())
}

/// A delivery time limit reaches a JMAP submission's envelope as BY (RFC
/// 2852 §4 through RFC 8621 §7), at least the server's least by-time for
/// R mode, and not with a message held for later.
#[test]
fn jmap_submissions_carry_the_delivery_time_limit() -> Result<()> {
    let (cert, port, server, _running) = start(Server { modern: true, ..Default::default() })?;
    let (b, mut a, path) = backend(&cert)?;
    let _cleanup = Cleanup(path);
    a.provider = Provider::Jmap;
    a.incoming_port = port;
    let by = |a: &Account| crate::sync::jmap::SendOptions {
        deliver_by: crate::sync::smtp::DeliverBy::of(a),
        ..Default::default()
    };
    a.deliver_within_hours = 4;
    a.deliver_late_notify = true;
    b.send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &by(&a))?;
    a.deliver_within_hours = 0;
    b.send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &by(&a))?;
    a.deliver_within_hours = 4;
    let held = crate::sync::jmap::SendOptions {
        hold_until: Some(chrono::Utc::now().timestamp() + 3600),
        ..by(&a)
    };
    b.send_jmap(&a, "qa@example.com", &["r@example.com".into()], MESSAGE, &held)?;
    let s = server.lock().unwrap();
    let params = |i: usize| s.envelopes[i]["mailFrom"]["parameters"].clone();
    assert_eq!(params(0)["BY"], json!("14400;N"));
    assert!(params(1).get("BY").is_none(), "{}", params(1));
    assert!(params(2).get("BY").is_none() && params(2).get("HOLDUNTIL").is_some(), "{}", params(2));
    Ok(())
}
