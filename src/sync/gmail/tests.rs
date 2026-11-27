//! A small Gmail API over loopback HTTP, by Google's documentation, and
//! tests of the client against it. Manage's tests (`manage/gmail_tests.rs`)
//! use it too.
use super::*;
use crate::sync::rest::test_server::{self, Answer, Request, json as answer};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct FakeMessage {
    pub id: String,
    pub thread: String,
    pub labels: Vec<String>,
    pub raw: Vec<u8>,
    /// Milliseconds.
    pub internal: i64,
}

#[derive(Default)]
pub(crate) struct Fake {
    /// (id, name, system)
    pub labels: Vec<(String, String, bool)>,
    pub messages: Vec<FakeMessage>,
    /// (draft id, message id)
    pub drafts: Vec<(String, String)>,
    /// Each change: (history id, message id, deleted).
    pub history: Vec<(u64, String, bool)>,
    pub history_id: u64,
    /// History before this is no longer kept (404 when asked from before).
    pub history_floor: u64,
    pub sent: Vec<Vec<u8>>,
    pub filters: Vec<Value>,
    pub vacation: Value,
    pub next: u64,
    /// Requests made to the batch endpoint.
    pub batches: usize,
}

impl Fake {
    pub fn new() -> Self {
        let mut f = Fake { history_id: 100, history_floor: 0, next: 1, ..Default::default() };
        for id in ["INBOX", "SENT", "DRAFT", "SPAM", "TRASH", "STARRED", "UNREAD", "IMPORTANT", "CATEGORY_SOCIAL"] {
            f.labels.push((id.into(), id.into(), true));
        }
        f.vacation = json!({"enableAutoReply": false});
        f
    }
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{:x}", self.next + 0x18c0)
    }
    /// Notes a change to a message in the history.
    pub fn changed(&mut self, id: &str, deleted: bool) {
        self.history_id += 1;
        self.history.push((self.history_id, id.to_owned(), deleted));
    }
    pub fn add(&mut self, subject: &str, labels: &[&str], minutes_ago: i64) -> String {
        let id = self.id("m");
        let now = chrono::Utc::now().timestamp();
        let date = chrono::DateTime::from_timestamp(now - minutes_ago * 60, 0).unwrap().to_rfc2822();
        let raw = format!(
            "From: Ana Lima <ana@example.org>\r\nTo: qa@example.com\r\nSubject: {subject}\r\nDate: {date}\r\nMessage-ID: <{id}@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nAbout {subject}.\r\n"
        );
        self.messages.push(FakeMessage {
            id: id.clone(),
            thread: format!("t{id}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            raw: raw.into_bytes(),
            internal: (now - minutes_ago * 60) * 1000,
        });
        self.changed(&id, false);
        id
    }
    pub fn message(&self, id: &str) -> Option<&FakeMessage> {
        self.messages.iter().find(|m| m.id == id)
    }
    fn user_label(&self, id: &str) -> bool {
        self.labels.iter().any(|(l, _, system)| l == id && !system)
    }
    fn matches(&self, m: &FakeMessage, label: Option<&str>, q: &str) -> bool {
        if let Some(label) = label
            && !m.labels.iter().any(|l| l == label)
        {
            return false;
        }
        if label.is_none()
            && !q.contains("in:trash")
            && !q.contains("in:spam")
            && m.labels.iter().any(|l| l == "TRASH" || l == "SPAM")
        {
            return false;
        }
        fn place(name: &str) -> &str {
            match name {
                "inbox" => "INBOX",
                "trash" => "TRASH",
                "spam" => "SPAM",
                "drafts" => "DRAFT",
                "sent" => "SENT",
                other => other,
            }
        }
        q.split_whitespace().all(|word| {
            if word == "is:unread" {
                m.labels.iter().any(|l| l == "UNREAD")
            } else if let Some(name) = word.strip_prefix("-in:") {
                !m.labels.iter().any(|l| l == place(name))
            } else if word == "-has:userlabels" {
                !m.labels.iter().any(|l| self.user_label(l))
            } else if word.starts_with("newer_than:") {
                true
            } else {
                String::from_utf8_lossy(&m.raw).to_lowercase().contains(&word.to_lowercase())
            }
        })
    }
    fn metadata(&self, m: &FakeMessage) -> Value {
        let parsed = mailparse::parse_mail(&m.raw).unwrap();
        let headers: Vec<Value> = parsed
            .headers
            .iter()
            .filter(|h| LIST_HEADERS.iter().any(|w| w.eq_ignore_ascii_case(&h.get_key())))
            .map(|h| json!({"name": h.get_key(), "value": h.get_value()}))
            .collect();
        json!({
            "id": m.id, "threadId": m.thread, "labelIds": m.labels,
            "snippet": "About &amp; more", "internalDate": m.internal.to_string(),
            "sizeEstimate": m.raw.len(), "historyId": self.history_id.to_string(),
            "payload": {"mimeType": parsed.ctype.mimetype, "headers": headers},
        })
    }

    pub fn respond(&mut self, r: &Request) -> Answer {
        let (path, query) = r.path.split_once('?').unwrap_or((&r.path, ""));
        let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
        let param = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        let base = "/gmail/v1/users/me";
        let ok = |v: Value| answer(200, v);
        let missing = || answer(404, json!({"error": {"code": 404, "message": "Requested entity was not found."}}));
        match (r.method.as_str(), path) {
            ("GET", p) if p == format!("{base}/profile") => ok(json!({
                "emailAddress": "qa@example.com", "messagesTotal": self.messages.len(),
                "historyId": self.history_id.to_string(),
            })),
            ("GET", p) if p == format!("{base}/labels") => ok(json!({
                "labels": self.labels.iter().map(|(id, name, system)| json!({
                    "id": id, "name": name, "type": if *system { "system" } else { "user" }
                })).collect::<Vec<_>>()
            })),
            ("POST", p) if p == format!("{base}/labels") => {
                let name = r.json()["name"].as_str().unwrap().to_owned();
                if self.labels.iter().any(|(_, n, _)| *n == name) {
                    return answer(409, json!({"error": {"message": "Label name exists or conflicts"}}));
                }
                let id = format!("Label_{}", self.next);
                self.next += 1;
                self.labels.push((id.clone(), name.clone(), false));
                ok(json!({"id": id, "name": name}))
            }
            (method, p) if p.starts_with(&format!("{base}/labels/")) => {
                let id = &p[base.len() + 8..];
                let Some(i) = self.labels.iter().position(|(l, _, _)| l == id) else { return missing() };
                match method {
                    "GET" => ok(json!({
                        "id": id,
                        "messagesTotal": self.messages.iter().filter(|m| m.labels.iter().any(|l| l == id)).count()
                    })),
                    "PATCH" => {
                        self.labels[i].1 = r.json()["name"].as_str().unwrap().to_owned();
                        ok(json!({"id": id}))
                    }
                    "DELETE" => {
                        self.labels.remove(i);
                        for m in &mut self.messages {
                            m.labels.retain(|l| l != id);
                        }
                        answer(204, Value::Null)
                    }
                    _ => missing(),
                }
            }
            ("GET", p) if p == format!("{base}/messages") => {
                let label = param("labelIds");
                let q = param("q").unwrap_or_default();
                let max: usize = param("maxResults").and_then(|m| m.parse().ok()).unwrap_or(100);
                let from: usize = param("pageToken").and_then(|m| m.parse().ok()).unwrap_or(0);
                let mut found: Vec<&FakeMessage> =
                    self.messages.iter().filter(|m| self.matches(m, label.as_deref(), &q)).collect();
                found.sort_by_key(|m| std::cmp::Reverse(m.internal));
                let page: Vec<Value> = found
                    .iter()
                    .skip(from)
                    .take(max)
                    .map(|m| json!({"id": m.id, "threadId": m.thread}))
                    .collect();
                let mut v = json!({"resultSizeEstimate": found.len()});
                if !page.is_empty() {
                    v["messages"] = page.into();
                }
                if from + max < found.len() {
                    v["nextPageToken"] = (from + max).to_string().into();
                }
                ok(v)
            }
            ("POST", "/batch/gmail/v1") => {
                self.batches += 1;
                let body = String::from_utf8_lossy(&r.body).into_owned();
                assert!(r.content_type.starts_with("multipart/mixed; boundary="), "{}", r.content_type);
                let mut out = String::new();
                let mut id = String::new();
                for line in body.lines() {
                    if let Some(v) = line.strip_prefix("Content-ID: <") {
                        id = v.trim_end_matches('>').to_owned();
                    }
                    if let Some(rest) = line.strip_prefix("GET ") {
                        let path = rest.split_once('?').map_or(rest, |(p, _)| p);
                        let mid = path.rsplit('/').next().unwrap();
                        let (status, json) = match self.message(mid) {
                            Some(m) => ("200 OK", self.metadata(m).to_string()),
                            None => ("404 Not Found", json!({"error": {"code": 404}}).to_string()),
                        };
                        out.push_str(&format!(
                            "--batch_resp\r\nContent-Type: application/http\r\nContent-ID: <response-{id}>\r\n\r\nHTTP/1.1 {status}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{json}\r\n"
                        ));
                    }
                }
                out.push_str("--batch_resp--\r\n");
                (200, "multipart/mixed; boundary=batch_resp".into(), out.into_bytes())
            }
            ("POST", p) if p == format!("{base}/messages/batchModify") => {
                let v = r.json();
                let list = |k: &str| -> Vec<String> {
                    v[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
                };
                let (add, remove) = (list("addLabelIds"), list("removeLabelIds"));
                for id in list("ids") {
                    if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
                        m.labels.retain(|l| !remove.contains(l));
                        for l in &add {
                            if !m.labels.contains(l) {
                                m.labels.push(l.clone());
                            }
                        }
                        self.history_id += 1;
                        self.history.push((self.history_id, id, false));
                    }
                }
                answer(204, Value::Null)
            }
            ("POST", p) if p == format!("{base}/messages/batchDelete") => {
                for id in r.json()["ids"].as_array().unwrap() {
                    let id = id.as_str().unwrap();
                    self.messages.retain(|m| m.id != id);
                    self.changed(id, true);
                }
                answer(204, Value::Null)
            }
            ("POST", p) if p == format!("/upload{base}/messages/send") => {
                assert_eq!(r.content_type, "message/rfc822");
                self.sent.push(r.body.clone());
                let id = self.id("s");
                self.messages.push(FakeMessage {
                    id: id.clone(),
                    thread: id.clone(),
                    labels: vec!["SENT".into()],
                    raw: r.body.clone(),
                    internal: chrono::Utc::now().timestamp() * 1000,
                });
                self.changed(&id, false);
                ok(json!({"id": id, "labelIds": ["SENT"]}))
            }
            ("POST", p) if p == format!("{base}/messages") => {
                let v = r.json();
                let raw = b64url_decode(v["raw"].as_str().unwrap()).unwrap();
                let id = self.id("i");
                self.messages.push(FakeMessage {
                    id: id.clone(),
                    thread: id.clone(),
                    labels: v["labelIds"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
                    raw,
                    internal: chrono::Utc::now().timestamp() * 1000,
                });
                self.changed(&id, false);
                ok(json!({"id": id}))
            }
            ("GET", p) if p.starts_with(&format!("{base}/threads/")) => {
                let id = &p[base.len() + 9..];
                let messages: Vec<Value> = self.messages.iter().filter(|m| m.thread == id).map(|m| json!({"id": m.id})).collect();
                if messages.is_empty() { missing() } else { ok(json!({"id": id, "messages": messages})) }
            }
            ("GET", p) if p.starts_with(&format!("{base}/messages/")) => {
                let id = &p[base.len() + 10..];
                match self.message(id) {
                    Some(m) if param("format").as_deref() == Some("raw") => ok(json!({"id": id, "raw": b64url(&m.raw)})),
                    Some(m) => ok(self.metadata(m)),
                    None => missing(),
                }
            }
            ("GET", p) if p == format!("{base}/history") => {
                let start: u64 = param("startHistoryId").unwrap().parse().unwrap();
                if start < self.history_floor {
                    return missing();
                }
                let history: Vec<Value> = self
                    .history
                    .iter()
                    .filter(|(h, _, _)| *h > start)
                    .map(|(h, id, deleted)| {
                        let entry = json!({"message": {"id": id}});
                        if *deleted {
                            json!({"id": h.to_string(), "messagesDeleted": [entry]})
                        } else {
                            json!({"id": h.to_string(), "labelsAdded": [entry]})
                        }
                    })
                    .collect();
                ok(json!({"history": history, "historyId": self.history_id.to_string()}))
            }
            ("GET", p) if p == format!("{base}/drafts") => ok(json!({
                "drafts": self.drafts.iter().map(|(d, m)| json!({"id": d, "message": {"id": m}})).collect::<Vec<_>>()
            })),
            ("POST", p) if p == format!("{base}/drafts") => {
                let raw = b64url_decode(r.json()["message"]["raw"].as_str().unwrap()).unwrap();
                let (draft, id) = (self.id("r"), self.id("m"));
                self.messages.push(FakeMessage {
                    id: id.clone(),
                    thread: id.clone(),
                    labels: vec!["DRAFT".into()],
                    raw,
                    internal: chrono::Utc::now().timestamp() * 1000,
                });
                self.drafts.push((draft.clone(), id.clone()));
                ok(json!({"id": draft, "message": {"id": id}}))
            }
            (method, p) if p.starts_with(&format!("{base}/drafts/")) => {
                let id = p[base.len() + 8..].to_owned();
                let Some(i) = self.drafts.iter().position(|(d, _)| *d == id) else { return missing() };
                let mid = self.drafts[i].1.clone();
                match method {
                    "GET" => ok(json!({"id": id, "message": {"id": mid, "raw": b64url(&self.message(&mid).unwrap().raw)}})),
                    "DELETE" => {
                        self.drafts.remove(i);
                        self.messages.retain(|m| m.id != mid);
                        answer(204, Value::Null)
                    }
                    _ => missing(),
                }
            }
            ("GET", p) if p == format!("{base}/settings/filters") => ok(json!({"filter": self.filters})),
            ("POST", p) if p == format!("{base}/settings/filters") => {
                let mut f = r.json();
                let id = self.id("f");
                f["id"] = id.clone().into();
                self.filters.push(f.clone());
                ok(f)
            }
            ("DELETE", p) if p.starts_with(&format!("{base}/settings/filters/")) => {
                let id = &p[base.len() + 18..];
                self.filters.retain(|f| f["id"] != id);
                answer(204, Value::Null)
            }
            ("GET", p) if p == format!("{base}/settings/vacation") => ok(self.vacation.clone()),
            ("PUT", p) if p == format!("{base}/settings/vacation") => {
                self.vacation = r.json();
                ok(self.vacation.clone())
            }
            _ => answer(400, json!({"error": {"code": 400, "message": format!("unknown {} {}", r.method, r.path)}})),
        }
    }
}

/// The fake, serving.
pub(crate) fn serve(fake: Fake) -> (test_server::Server, Arc<Mutex<Fake>>) {
    let fake = Arc::new(Mutex::new(fake));
    let shared = fake.clone();
    let server = test_server::start(move |r| shared.lock().unwrap().respond(r));
    (server, fake)
}

fn tls() -> native_tls::TlsConnector {
    native_tls::TlsConnector::new().unwrap()
}

#[test]
fn messages_are_listed_read_in_batches_and_relabeled() -> Result<()> {
    let mut fake = Fake::new();
    let first = fake.add("Lunch", &["INBOX", "UNREAD"], 5);
    let second = fake.add("Plans", &["INBOX"], 10);
    let (server, fake) = serve(fake);
    let s = Session::open(&server.base, "token", tls())?;
    assert_eq!(s.profile()?.email, "qa@example.com");
    let (ids, next) = s.list(Some(label::INBOX), "", None, 1)?;
    assert_eq!(ids, std::slice::from_ref(&first));
    let (ids, next) = s.list(Some(label::INBOX), "", next.as_deref(), 1)?;
    assert_eq!((ids, next), (vec![second.clone()], None));
    // Read together; one gone meanwhile is left out.
    let read = s.metadata(&[first.clone(), "gone".into(), second.clone()])?;
    assert_eq!(read.len(), 2);
    assert_eq!(fake.lock().unwrap().batches, 1);
    let lunch = &read[0];
    assert!(lunch.has(label::UNREAD));
    assert_eq!(lunch.snippet, "About & more");
    assert!(lunch.received > 0);
    let block = String::from_utf8(lunch.header_block()).unwrap();
    assert!(block.contains("Subject: Lunch\r\n") && block.ends_with("\r\n\r\n"), "{block}");
    // Relabeled, and the history says so.
    let since = s.profile()?.history_id;
    s.modify(std::slice::from_ref(&first), &[], &[label::UNREAD])?;
    let h = s.history(&since)?.unwrap();
    assert_eq!(h.changed, std::slice::from_ref(&first));
    assert!(!fake.lock().unwrap().message(&first).unwrap().labels.contains(&"UNREAD".into()));
    // Too old: the mailbox must be read again.
    fake.lock().unwrap().history_floor = 1_000;
    assert!(s.history(&since)?.is_none());
    // The source, from base64url.
    assert!(String::from_utf8(s.raw(&second)?).unwrap().contains("Subject: Plans"));
    Ok(())
}

#[test]
fn batch_answers_are_taken_apart() -> Result<()> {
    let body = b"--b1\r\nContent-Type: application/http\r\nContent-ID: <response-item1>\r\n\r\nHTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{\"error\":{}}\r\n--b1\r\nContent-Type: application/http\r\nContent-ID: <response-item0>\r\n\r\nHTTP/1.1 200 OK\r\n\r\n{\"id\":\"a\"}\r\n--b1--\r\n";
    let parts = batch_parts("multipart/mixed; boundary=b1", body)?;
    assert_eq!(parts[0].0, "response-item1");
    assert_eq!(parts[0].1, 404);
    assert_eq!((parts[1].1, parts[1].2.as_slice()), (200, &b"{\"id\":\"a\"}"[..]));
    assert!(batch_parts("multipart/mixed", body).is_err());
    Ok(())
}
