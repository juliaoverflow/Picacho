//! The Gmail API (`gmail.googleapis.com`), for Gmail accounts that use it
//! instead of IMAP and SMTP (`Account::api`).
//!
//! Gmail keeps one copy of each message and labels it: the system labels
//! (INBOX, SENT, DRAFT, SPAM, TRASH, STARRED, UNREAD…) and the person's
//! own, whose names nest with `/`. Listing gives only ids, so what the
//! message list shows is read in batches (`metadata`, Gmail's batch
//! endpoint: up to 50 requests in one); what changed since is read from
//! the mailbox's history (`history`). Messages are read and sent as MIME
//! (`format=raw`, the upload endpoints).
//!
//! These helpers only talk to Gmail; Manage keeps the result
//! (`manage/gmail.rs`).
use super::rest::{NotFound, Rest, escape};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::sync::Mutex;

pub const BASE: &str = "https://gmail.googleapis.com";
const USER: &str = "/gmail/v1/users/me";
/// The most requests Google suggests in one batch.
const BATCH: usize = 50;
/// The headers read for the message list.
const LIST_HEADERS: &[&str] = &[
    "From",
    "To",
    "Cc",
    "Reply-To",
    "Subject",
    "Date",
    "Message-ID",
    "References",
    "In-Reply-To",
    "List-Unsubscribe",
    "List-Unsubscribe-Post",
    "List-Id",
    "Disposition-Notification-To",
    "Auto-Submitted",
    "X-Autoreply",
    "X-Autorespond",
    "Precedence",
    "Content-Type",
];

/// Gmail's system labels Picacho uses.
pub mod label {
    pub const INBOX: &str = "INBOX";
    pub const SENT: &str = "SENT";
    pub const DRAFT: &str = "DRAFT";
    pub const SPAM: &str = "SPAM";
    pub const TRASH: &str = "TRASH";
    pub const STARRED: &str = "STARRED";
    pub const UNREAD: &str = "UNREAD";
}

/// A label: a system one, or one of the person's (which act as folders).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub id: String,
    pub name: String,
    /// One of Gmail's own (INBOX, CATEGORY_SOCIAL…), not the person's.
    pub system: bool,
}

/// What the message list shows of a message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    pub snippet: String,
    /// When Gmail received it (Unix seconds).
    pub received: i64,
    pub size: u64,
    pub mime_type: String,
    /// The headers asked for (`LIST_HEADERS`), as they came.
    pub headers: Vec<(String, String)>,
}
impl Metadata {
    pub fn has(&self, label: &str) -> bool {
        self.label_ids.iter().any(|l| l == label)
    }
    /// Its headers as a header block, for the MIME parser.
    pub fn header_block(&self) -> Vec<u8> {
        let mut out = String::new();
        for (name, value) in &self.headers {
            // Gmail gives them unfolded; a stray line break can't start a
            // header of its own.
            let value = value.replace(['\r', '\n'], " ");
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        out.into_bytes()
    }
    fn from_json(v: &Value) -> Self {
        Self {
            id: text(&v["id"]),
            thread_id: text(&v["threadId"]),
            label_ids: v["labelIds"]
                .as_array()
                .map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            snippet: html_unescape(&text(&v["snippet"])),
            received: v["internalDate"]
                .as_str()
                .and_then(|d| d.parse::<i64>().ok())
                .map_or(0, |ms| ms / 1000),
            size: v["sizeEstimate"].as_u64().unwrap_or_default(),
            mime_type: text(&v["payload"]["mimeType"]),
            headers: v["payload"]["headers"]
                .as_array()
                .map(|h| {
                    h.iter()
                        .map(|h| (text(&h["name"]), text(&h["value"])))
                        .filter(|(n, _)| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// What changed in the mailbox since a history id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    /// Messages added or relabeled.
    pub changed: Vec<String>,
    /// Messages deleted for good.
    pub deleted: Vec<String>,
    /// Where the next look starts.
    pub history_id: String,
}

/// Who the mailbox is, and where its history is now.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub email: String,
    pub history_id: String,
}

/// A filter: what it matches and what it does (Gmail's own fields).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    /// Gmail search words the message must match.
    pub query: String,
    pub has_attachment: bool,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub forward: String,
}

/// The automatic reply while away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vacation {
    pub enabled: bool,
    pub subject: String,
    pub body: String,
    pub contacts_only: bool,
    /// Unix seconds; 0 when open-ended.
    pub start: i64,
    pub end: i64,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// Gmail's snippets come with HTML entities.
fn html_unescape(s: &str) -> String {
    s.replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .context("Gmail sent a message that isn't base64url")
}
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A signed-in Gmail API connection.
pub struct Session {
    rest: Rest,
    labels: Mutex<Vec<Label>>,
}

impl Session {
    pub fn open(base: &str, token: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        Ok(Self {
            rest: Rest::new(base, token, "Gmail", tls)?,
            labels: Mutex::new(Vec::new()),
        })
    }

    pub fn profile(&self) -> Result<Profile> {
        let v = self.rest.get(&format!("{USER}/profile"))?;
        Ok(Profile {
            email: text(&v["emailAddress"]),
            history_id: v["historyId"].as_str().map(str::to_owned).unwrap_or_else(|| v["historyId"].to_string()),
        })
    }

    // Labels.

    /// Reads the labels again.
    pub fn reload_labels(&self) -> Result<Vec<Label>> {
        let v = self.rest.get(&format!("{USER}/labels"))?;
        let mut labels: Vec<Label> = v["labels"]
            .as_array()
            .map(|l| {
                l.iter()
                    .map(|l| Label {
                        id: text(&l["id"]),
                        name: text(&l["name"]),
                        system: l["type"].as_str() == Some("system"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        labels.sort_by_key(|l| l.name.to_lowercase());
        *self.labels.lock().unwrap() = labels.clone();
        Ok(labels)
    }
    /// The labels as last read (read now if they never were).
    pub fn labels(&self) -> Result<Vec<Label>> {
        let known = self.labels.lock().unwrap().clone();
        if known.is_empty() { self.reload_labels() } else { Ok(known) }
    }
    /// How many messages a label has.
    pub fn label_total(&self, id: &str) -> Result<u64> {
        let v = self.rest.get(&format!("{USER}/labels/{}", escape(id)))?;
        Ok(v["messagesTotal"].as_u64().unwrap_or_default())
    }
    pub fn create_label(&self, name: &str) -> Result<Label> {
        let v = self.rest.post(
            &format!("{USER}/labels"),
            &json!({"name": name, "labelListVisibility": "labelShow", "messageListVisibility": "show"}),
        )?;
        self.reload_labels()?;
        Ok(Label { id: text(&v["id"]), name: text(&v["name"]), system: false })
    }
    pub fn rename_label(&self, id: &str, name: &str) -> Result<()> {
        self.rest.patch(&format!("{USER}/labels/{}", escape(id)), &json!({"name": name}))?;
        self.reload_labels()?;
        Ok(())
    }
    pub fn delete_label(&self, id: &str) -> Result<()> {
        self.rest.delete(&format!("{USER}/labels/{}", escape(id)))?;
        self.reload_labels()?;
        Ok(())
    }

    // Messages.

    /// One page of message ids, newest first: those with `label` (any,
    /// without Spam and Trash, when `None`) matching the Gmail search
    /// `query`; and the next page's token.
    pub fn list(
        &self,
        label: Option<&str>,
        query: &str,
        page: Option<&str>,
        max: usize,
    ) -> Result<(Vec<String>, Option<String>)> {
        let mut path = format!("{USER}/messages?maxResults={}", max.clamp(1, 500));
        if let Some(label) = label {
            path.push_str(&format!("&labelIds={}", escape(label)));
        }
        if !query.is_empty() {
            path.push_str(&format!("&q={}", escape(query)));
        }
        if let Some(page) = page {
            path.push_str(&format!("&pageToken={}", escape(page)));
        }
        let v = self.rest.get(&path)?;
        let ids = v["messages"]
            .as_array()
            .map(|m| m.iter().map(|m| text(&m["id"])).filter(|i| !i.is_empty()).collect())
            .unwrap_or_default();
        Ok((ids, v["nextPageToken"].as_str().map(str::to_owned)))
    }
    /// Up to `most` ids, page after page.
    pub fn list_all(&self, label: Option<&str>, query: &str, most: usize) -> Result<Vec<String>> {
        /// The most pages asked for: a server answering with empty pages,
        /// each naming another, would otherwise be asked for ever.
        const MAX_PAGES: usize = 1000;
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        let mut tokens = std::collections::HashSet::new();
        for _ in 0..MAX_PAGES {
            let (ids, next) = self.list(label, query, page.as_deref(), (most - out.len()).min(500))?;
            let empty = ids.is_empty();
            out.extend(ids);
            match next {
                // A page token seen before, or a page of nothing: no further.
                Some(n) if out.len() < most && !empty && tokens.insert(n.clone()) => page = Some(n),
                _ => break,
            }
        }
        out.truncate(most);
        Ok(out)
    }

    /// What the message list shows of each of `ids`, in batches. Those
    /// gone meanwhile are left out.
    pub fn metadata(&self, ids: &[String]) -> Result<Vec<Metadata>> {
        let headers: String = LIST_HEADERS.iter().map(|h| format!("&metadataHeaders={h}")).collect();
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH) {
            let paths: Vec<String> = chunk
                .iter()
                .map(|id| format!("{USER}/messages/{}?format=metadata{headers}", escape(id)))
                .collect();
            for (status, body) in self.batch(&paths)? {
                match status {
                    200 => out.push(Metadata::from_json(&serde_json::from_slice(&body).unwrap_or(Value::Null))),
                    404 => {}
                    _ => {
                        let text = String::from_utf8_lossy(&body);
                        let code = reqwest::StatusCode::from_u16(status).unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
                        return Err(self.rest.problem(code, &text, None));
                    }
                }
            }
        }
        Ok(out)
    }

    /// GET requests for `paths`, in one batch (Gmail's batch endpoint): each
    /// one's status and body, in order. Requests Gmail turned away for
    /// going too fast are sent again, twice at most.
    fn batch(&self, paths: &[String]) -> Result<Vec<(u16, Vec<u8>)>> {
        let mut answers: Vec<Option<(u16, Vec<u8>)>> = vec![None; paths.len()];
        for attempt in 0..3 {
            let waiting: Vec<usize> = (0..paths.len())
                .filter(|&i| answers[i].as_ref().is_none_or(|(s, _)| *s == 429 && attempt > 0))
                .collect();
            if waiting.is_empty() {
                break;
            }
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(800 * attempt as u64));
            }
            let boundary = format!("batch_{}", uuid::Uuid::new_v4().simple());
            let mut body = String::new();
            for &i in &waiting {
                body.push_str(&format!(
                    "--{boundary}\r\nContent-Type: application/http\r\nContent-ID: <item{i}>\r\n\r\nGET {}\r\n\r\n",
                    paths[i]
                ));
            }
            body.push_str(&format!("--{boundary}--\r\n"));
            let (kind, reply) = self.rest.exchange(
                reqwest::Method::POST,
                "/batch/gmail/v1",
                &format!("multipart/mixed; boundary={boundary}"),
                body.into_bytes(),
            )?;
            for (id, status, body) in batch_parts(&kind, &reply)? {
                if let Some(i) = id.strip_prefix("response-item").and_then(|i| i.parse::<usize>().ok())
                    && i < answers.len()
                {
                    answers[i] = Some((status, body));
                }
            }
        }
        answers
            .into_iter()
            .map(|a| match a {
                Some((429, _)) => bail!(
                    "[THROTTLED] {}",
                    t!("sync-rest-is-limiting-requests", service = "Gmail", status = 429u16)
                ),
                Some(a) => Ok(a),
                None => bail!(t!("sync-gmail-gmail-left-a-request-unanswered")),
            })
            .collect()
    }

    /// The ids of a conversation's messages (its thread), wherever they are.
    pub fn thread(&self, id: &str) -> Result<Vec<String>> {
        let v = self.rest.get(&format!("{USER}/threads/{}?format=minimal", escape(id)))?;
        Ok(v["messages"]
            .as_array()
            .map(|l| l.iter().filter_map(|m| m["id"].as_str()).map(str::to_owned).collect())
            .unwrap_or_default())
    }

    /// A message's source.
    pub fn raw(&self, id: &str) -> Result<Vec<u8>> {
        let v = self.rest.get(&format!("{USER}/messages/{}?format=raw", escape(id)))?;
        b64url_decode(v["raw"].as_str().with_context(|| t!("sync-gmail-gmail-sent-no-source"))?)
    }

    /// Adds and removes labels on messages (a thousand at a time).
    pub fn modify(&self, ids: &[String], add: &[&str], remove: &[&str]) -> Result<()> {
        for chunk in ids.chunks(1000) {
            self.rest.post(
                &format!("{USER}/messages/batchModify"),
                &json!({"ids": chunk, "addLabelIds": add, "removeLabelIds": remove}),
            )?;
        }
        Ok(())
    }
    /// Deletes messages for good.
    pub fn delete(&self, ids: &[String]) -> Result<()> {
        for chunk in ids.chunks(1000) {
            self.rest.post(&format!("{USER}/messages/batchDelete"), &json!({"ids": chunk}))?;
        }
        Ok(())
    }
    /// Sends a finished message: Gmail sends it to those its To, Cc and
    /// Bcc name (removing Bcc), and keeps it in Sent. Its id.
    pub fn send(&self, raw: &[u8]) -> Result<String> {
        let v = self.rest.upload(
            reqwest::Method::POST,
            &format!("/upload{USER}/messages/send?uploadType=media"),
            "message/rfc822",
            raw.to_vec(),
        )?;
        Ok(text(&v["id"]))
    }
    /// Puts a message in the mailbox with `labels`, as it is (a copy of
    /// mail sent elsewhere). Its id.
    pub fn insert(&self, raw: &[u8], labels: &[&str]) -> Result<String> {
        let v = self.rest.post(
            &format!("{USER}/messages?internalDateSource=dateHeader"),
            &json!({"raw": b64url(raw), "labelIds": labels}),
        )?;
        Ok(text(&v["id"]))
    }

    /// What changed since `since` (a history id): `None` when that is too
    /// old for Gmail to say, and the mailbox must be read again.
    pub fn history(&self, since: &str) -> Result<Option<History>> {
        let mut out = History::default();
        let mut page: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        // Bounded: a mailbox changing faster than it is read is read again.
        for _ in 0..20 {
            let mut path = format!("{USER}/history?startHistoryId={}&maxResults=500", escape(since));
            if let Some(p) = &page {
                path.push_str(&format!("&pageToken={}", escape(p)));
            }
            let v = match self.rest.get(&path) {
                Ok(v) => v,
                Err(e) if e.downcast_ref::<NotFound>().is_some() => return Ok(None),
                Err(e) => return Err(e),
            };
            for h in v["history"].as_array().into_iter().flatten() {
                for kind in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
                    for m in h[kind].as_array().into_iter().flatten() {
                        let id = text(&m["message"]["id"]);
                        if !id.is_empty() && seen.insert(id.clone()) {
                            out.changed.push(id);
                        }
                    }
                }
                for m in h["messagesDeleted"].as_array().into_iter().flatten() {
                    let id = text(&m["message"]["id"]);
                    if !id.is_empty() {
                        out.deleted.push(id);
                    }
                }
            }
            if let Some(id) = v["historyId"].as_str() {
                out.history_id = id.to_owned();
            }
            match v["nextPageToken"].as_str() {
                Some(next) => page = Some(next.to_owned()),
                None => {
                    // Deleted ones need not be read again.
                    out.changed.retain(|id| !out.deleted.contains(id));
                    if out.history_id.is_empty() {
                        out.history_id = since.to_owned();
                    }
                    return Ok(Some(out));
                }
            }
        }
        Ok(None)
    }

    // Drafts.

    /// The drafts: (draft id, message id).
    pub fn drafts(&self, most: usize) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut path = format!("{USER}/drafts?maxResults=500");
            if let Some(p) = &page {
                path.push_str(&format!("&pageToken={}", escape(p)));
            }
            let v = self.rest.get(&path)?;
            for d in v["drafts"].as_array().into_iter().flatten() {
                out.push((text(&d["id"]), text(&d["message"]["id"])));
            }
            match v["nextPageToken"].as_str() {
                Some(next) if out.len() < most => page = Some(next.to_owned()),
                _ => break,
            }
        }
        Ok(out)
    }
    pub fn draft_raw(&self, id: &str) -> Result<Vec<u8>> {
        let v = self.rest.get(&format!("{USER}/drafts/{}?format=raw", escape(id)))?;
        b64url_decode(v["message"]["raw"].as_str().with_context(|| t!("sync-gmail-gmail-sent-no-source"))?)
    }
    /// Saves a new draft; its draft id.
    pub fn create_draft(&self, raw: &[u8]) -> Result<String> {
        let v = self.rest.post(&format!("{USER}/drafts"), &json!({"message": {"raw": b64url(raw)}}))?;
        Ok(text(&v["id"]))
    }
    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.rest.delete(&format!("{USER}/drafts/{}", escape(id)))
    }

    // Settings.

    pub fn filters(&self) -> Result<Vec<Filter>> {
        let v = self.rest.get(&format!("{USER}/settings/filters"))?;
        let list = |v: &Value| -> Vec<String> {
            v.as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
        };
        Ok(v["filter"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| Filter {
                id: text(&f["id"]),
                from: text(&f["criteria"]["from"]),
                to: text(&f["criteria"]["to"]),
                subject: text(&f["criteria"]["subject"]),
                query: text(&f["criteria"]["query"]),
                has_attachment: f["criteria"]["hasAttachment"].as_bool().unwrap_or(false),
                add_labels: list(&f["action"]["addLabelIds"]),
                remove_labels: list(&f["action"]["removeLabelIds"]),
                forward: text(&f["action"]["forward"]),
            })
            .collect())
    }
    /// Adds a filter; its id. Gmail has no changing a filter: it is
    /// deleted and added again.
    pub fn create_filter(&self, f: &Filter) -> Result<String> {
        let mut criteria = serde_json::Map::new();
        for (key, value) in [("from", &f.from), ("to", &f.to), ("subject", &f.subject), ("query", &f.query)] {
            if !value.is_empty() {
                criteria.insert(key.into(), value.clone().into());
            }
        }
        if f.has_attachment {
            criteria.insert("hasAttachment".into(), true.into());
        }
        let mut action = serde_json::Map::new();
        if !f.add_labels.is_empty() {
            action.insert("addLabelIds".into(), json!(f.add_labels));
        }
        if !f.remove_labels.is_empty() {
            action.insert("removeLabelIds".into(), json!(f.remove_labels));
        }
        if !f.forward.is_empty() {
            action.insert("forward".into(), f.forward.clone().into());
        }
        let v = self
            .rest
            .post(&format!("{USER}/settings/filters"), &json!({"criteria": criteria, "action": action}))?;
        Ok(text(&v["id"]))
    }
    pub fn delete_filter(&self, id: &str) -> Result<()> {
        self.rest.delete(&format!("{USER}/settings/filters/{}", escape(id)))
    }
    pub fn vacation(&self) -> Result<Vacation> {
        let v = self.rest.get(&format!("{USER}/settings/vacation"))?;
        let ms = |v: &Value| {
            v.as_str().and_then(|s| s.parse::<i64>().ok()).or(v.as_i64()).map_or(0, |ms| ms / 1000)
        };
        Ok(Vacation {
            enabled: v["enableAutoReply"].as_bool().unwrap_or(false),
            subject: text(&v["responseSubject"]),
            body: text(&v["responseBodyPlainText"]),
            contacts_only: v["restrictToContacts"].as_bool().unwrap_or(false),
            start: ms(&v["startTime"]),
            end: ms(&v["endTime"]),
        })
    }
    pub fn set_vacation(&self, v: &Vacation) -> Result<()> {
        let mut body = json!({
            "enableAutoReply": v.enabled,
            "responseSubject": v.subject,
            "responseBodyPlainText": v.body,
            "restrictToContacts": v.contacts_only,
        });
        if v.start > 0 {
            body["startTime"] = (v.start * 1000).to_string().into();
        }
        if v.end > 0 {
            body["endTime"] = (v.end * 1000).to_string().into();
        }
        self.rest.put(&format!("{USER}/settings/vacation"), &body)?;
        Ok(())
    }
}

/// The parts of a batch's answer (`multipart/mixed` of `application/http`
/// answers): each one's Content-ID, status and body.
fn batch_parts(content_type: &str, body: &[u8]) -> Result<Vec<(String, u16, Vec<u8>)>> {
    let boundary = content_type
        .split(';')
        .filter_map(|p| p.trim().strip_prefix("boundary="))
        .next()
        .map(|b| b.trim_matches('"').to_owned())
        .with_context(|| t!("sync-gmail-gmail-answered-a-batch-in-a"))?;
    let delimiter = format!("--{boundary}");
    let text = String::from_utf8_lossy(body);
    let mut out = Vec::new();
    for part in text.split(delimiter.as_str()).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let part = part.trim_start_matches(['\r', '\n']);
        let Some((head, rest)) = split_head(part) else { continue };
        let id = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case("content-id").then(|| v.trim().trim_matches(['<', '>']).to_owned())
            })
            .unwrap_or_default();
        let Some((status_head, inner)) = split_head(rest.trim_start_matches(['\r', '\n'])) else { continue };
        let status = status_head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(502);
        let inner = inner.trim_end_matches(['\r', '\n']);
        out.push((id, status, inner.as_bytes().to_vec()));
    }
    Ok(out)
}
/// A header block and what follows it.
fn split_head(text: &str) -> Option<(&str, &str)> {
    let crlf = text.find("\r\n\r\n").map(|i| (i, 4));
    let lf = text.find("\n\n").map(|i| (i, 2));
    let (at, len) = match (crlf, lf) {
        (Some(a), Some(b)) => if a.0 <= b.0 { a } else { b },
        (a, b) => a.or(b)?,
    };
    Some((&text[..at], &text[at + len..]))
}

#[cfg(test)]
pub(crate) mod tests;
