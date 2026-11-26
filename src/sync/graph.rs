//! Microsoft Graph (`graph.microsoft.com`), for Outlook and Microsoft 365
//! accounts that use it instead of IMAP and SMTP (`Account::api`): mail,
//! its folders and inbox rules, automatic replies, and (`graph/dav.rs`)
//! calendars, contacts and the organization's directory.
//!
//! Every request asks for immutable ids (`Prefer: IdType="ImmutableId"`),
//! so a message keeps its id when it moves between folders. Requests for
//! many messages go together in JSON batches of 20 (`$batch`). Messages
//! are read and sent as MIME (`$value`, `sendMail` with a MIME body); one
//! too large for a single request is sent as a draft whose attachments
//! upload in pieces (`send_large`).
//!
//! These helpers only talk to Microsoft; Manage keeps the result
//! (`manage/graph.rs`).
use super::rest::{Rest, escape};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

pub mod dav;

pub const BASE: &str = "https://graph.microsoft.com";
const V: &str = "/v1.0";
/// The most requests in one batch.
const BATCH: usize = 20;
/// What the message list shows of a message.
const SELECT: &str = "id,conversationId,parentFolderId,subject,from,toRecipients,ccRecipients,replyTo,receivedDateTime,isRead,flag,categories,hasAttachments,bodyPreview,internetMessageId,isDraft";
/// Its size (PR_MESSAGE_SIZE) and what was last done with it
/// (PR_LAST_VERB_EXECUTED: replied, replied to all, forwarded).
const EXPAND: &str = "singleValueExtendedProperties($filter=id eq 'Integer 0x0E08' or id eq 'Integer 0x1081')";
/// The largest MIME message one request may carry, base64 and all: 4 MB.
pub const MIME_MOST: usize = 4 * 1024 * 1024 * 3 / 4 - 64 * 1024;

/// The standard folders' well-known names, and those Picacho leaves out
/// of the folder list.
pub mod well_known {
    pub const INBOX: &str = "inbox";
    pub const SENT: &str = "sentitems";
    pub const DRAFTS: &str = "drafts";
    pub const TRASH: &str = "deleteditems";
    pub const JUNK: &str = "junkemail";
    pub const ARCHIVE: &str = "archive";
    /// Not shown as folders of their own.
    pub const HIDDEN: &[&str] = &["outbox", "conversationhistory", "syncissues", "scheduled"];
}

/// A mail folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailFolder {
    pub id: String,
    pub name: String,
    pub parent_id: String,
    /// Its path from the top, with `/` between names.
    pub path: String,
    pub depth: usize,
    /// Its well-known name (`inbox`, `sentitems`…), for standard ones.
    pub well_known: Option<String>,
    pub total: u64,
}

/// A person a message names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
}
impl Person {
    /// As a mail header writes it.
    pub fn shown(&self) -> String {
        if self.name.trim().is_empty() || self.name == self.email {
            self.email.clone()
        } else {
            let name = self.name.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{name}\" <{}>", self.email)
        }
    }
    fn from_json(v: &Value) -> Option<Self> {
        let address = &v["emailAddress"];
        let email = address["address"].as_str().unwrap_or_default().to_owned();
        let name = address["name"].as_str().unwrap_or_default().to_owned();
        (!email.is_empty() || !name.is_empty()).then_some(Self { name, email })
    }
    fn list(v: &Value) -> Vec<Self> {
        v.as_array().map(|l| l.iter().filter_map(Self::from_json).collect()).unwrap_or_default()
    }
}
/// People as a header writes them.
pub fn shown(people: &[Person]) -> String {
    people.iter().map(Person::shown).collect::<Vec<_>>().join(", ")
}

/// What the message list shows of a message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub folder_id: String,
    pub subject: String,
    pub from: Option<Person>,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub reply_to: Vec<Person>,
    /// When it arrived (Unix seconds).
    pub received: i64,
    pub read: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub has_attachments: bool,
    pub preview: String,
    pub message_id: String,
    pub draft: bool,
    pub size: u64,
    /// PR_LAST_VERB_EXECUTED: 102 replied, 103 replied to all, 104 forwarded.
    pub last_verb: Option<i64>,
}
impl Message {
    pub fn from_json(v: &Value) -> Self {
        let mut size = 0;
        let mut last_verb = None;
        for p in v["singleValueExtendedProperties"].as_array().into_iter().flatten() {
            let value = p["value"].as_str().and_then(|s| s.parse::<i64>().ok());
            match p["id"].as_str().unwrap_or_default().to_ascii_lowercase().as_str() {
                "integer 0xe08" | "integer 0x0e08" => size = value.unwrap_or_default().max(0) as u64,
                "integer 0x1081" => last_verb = value,
                _ => {}
            }
        }
        Self {
            id: text(&v["id"]),
            conversation_id: text(&v["conversationId"]),
            folder_id: text(&v["parentFolderId"]),
            subject: text(&v["subject"]),
            from: Person::from_json(&v["from"]),
            to: Person::list(&v["toRecipients"]),
            cc: Person::list(&v["ccRecipients"]),
            reply_to: Person::list(&v["replyTo"]),
            received: time(&v["receivedDateTime"]),
            read: v["isRead"].as_bool().unwrap_or(false),
            flagged: v["flag"]["flagStatus"].as_str() == Some("flagged"),
            categories: v["categories"]
                .as_array()
                .map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            has_attachments: v["hasAttachments"].as_bool().unwrap_or(false),
            preview: text(&v["bodyPreview"]),
            message_id: text(&v["internetMessageId"]),
            draft: v["isDraft"].as_bool().unwrap_or(false),
            size,
            last_verb,
        }
    }
}

/// An inbox rule, as Graph describes it (`messageRule`): kept as JSON, so
/// what Picacho doesn't show of it is kept as it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub id: String,
    pub json: Value,
}

/// The automatic reply while away (`automaticRepliesSetting`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutomaticReplies {
    pub enabled: bool,
    /// The reply to people outside the organization, where it differs.
    pub external: String,
    pub internal: String,
    /// Reply to people outside the organization only if they are contacts.
    pub contacts_only: bool,
    /// Unix seconds; both 0 when not scheduled.
    pub start: i64,
    pub end: i64,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
/// A Graph time (`2026-11-24T10:00:00Z`) in Unix seconds.
pub fn time(v: &Value) -> i64 {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(0, |d| d.timestamp())
}
/// Unix seconds as Graph writes a UTC time.
pub fn graph_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A signed-in Graph connection.
pub struct Session {
    pub(crate) rest: Rest,
    folders: Mutex<Vec<MailFolder>>,
}

impl Session {
    pub fn open(base: &str, token: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        let rest = Rest::new(base, token, "Microsoft 365", tls)?.with_header("Prefer", "IdType=\"ImmutableId\"");
        Ok(Self { rest, folders: Mutex::new(Vec::new()) })
    }
    /// A path under the API's version (`/me/messages` → `/v1.0/me/messages`).
    fn p(path: &str) -> String {
        format!("{V}{path}")
    }

    /// The signed-in mailbox's address and name.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn me(&self) -> Result<(String, String)> {
        let v = self.rest.get(&Self::p("/me?$select=mail,userPrincipalName,displayName"))?;
        let email = v["mail"].as_str().filter(|m| !m.is_empty()).or(v["userPrincipalName"].as_str()).unwrap_or_default();
        Ok((email.to_owned(), text(&v["displayName"])))
    }

    /// Every page of a listing, up to `most` items.
    pub fn all_pages(&self, first: &str, most: usize) -> Result<Vec<Value>> {
        /// The most pages asked for: a server answering with empty pages,
        /// each linking to another (or the same), would be asked for ever.
        const MAX_PAGES: usize = 1000;
        let mut out = Vec::new();
        let mut next = Some(Self::p(first));
        let mut seen = std::collections::HashSet::new();
        let mut pages = 0;
        while let Some(path) = next.take() {
            pages += 1;
            if pages > MAX_PAGES || !seen.insert(path.clone()) {
                break;
            }
            let v = self.rest.get(&path)?;
            let page = v["value"].as_array().cloned().unwrap_or_default();
            let empty = page.is_empty();
            out.extend(page);
            if out.len() >= most {
                out.truncate(most);
                break;
            }
            if empty {
                break;
            }
            next = v["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(out)
    }

    // Folders.

    /// Reads the folders again: every one, nested ones too, with the
    /// standard ones' well-known names.
    pub fn reload_folders(&self) -> Result<Vec<MailFolder>> {
        let mut out: Vec<(Value, usize, String)> = Vec::new();
        for f in self.all_pages("/me/mailFolders?$top=250&$select=id,displayName,parentFolderId,childFolderCount,totalItemCount", 5000)? {
            out.push((f, 0, String::new()));
        }
        // Folders inside folders, as deep as they go (bounded).
        let mut i = 0;
        while i < out.len() && out.len() < 5000 {
            let (f, depth, parent_path) = out[i].clone();
            let path = if parent_path.is_empty() { text(&f["displayName"]) } else { format!("{parent_path}/{}", text(&f["displayName"])) };
            if f["childFolderCount"].as_u64().unwrap_or(0) > 0 && depth < 12 {
                let id = text(&f["id"]);
                for child in self.all_pages(
                    &format!("/me/mailFolders/{}/childFolders?$top=250&$select=id,displayName,parentFolderId,childFolderCount,totalItemCount", escape(&id)),
                    5000,
                )? {
                    out.push((child, depth + 1, path.clone()));
                }
            }
            i += 1;
        }
        // The standard folders' ids, by well-known name, in one batch.
        let names: Vec<&str> = [
            well_known::INBOX,
            well_known::SENT,
            well_known::DRAFTS,
            well_known::TRASH,
            well_known::JUNK,
            well_known::ARCHIVE,
        ]
        .into_iter()
        .chain(well_known::HIDDEN.iter().copied())
        .collect();
        let requests: Vec<Value> = names
            .iter()
            .map(|n| json!({"method": "GET", "url": format!("/me/mailFolders/{n}?$select=id")}))
            .collect();
        let mut known: HashMap<String, String> = HashMap::new();
        for (n, (status, body)) in names.iter().zip(self.batch(requests)?) {
            if status == 200 {
                known.insert(text(&body["id"]), (*n).to_owned());
            }
        }
        let folders: Vec<MailFolder> = out
            .into_iter()
            .map(|(f, depth, parent_path)| {
                let name = text(&f["displayName"]);
                let id = text(&f["id"]);
                MailFolder {
                    well_known: known.get(&id).cloned(),
                    path: if parent_path.is_empty() { name.clone() } else { format!("{parent_path}/{name}") },
                    id,
                    name,
                    parent_id: text(&f["parentFolderId"]),
                    depth,
                    total: f["totalItemCount"].as_u64().unwrap_or_default(),
                }
            })
            .collect();
        *self.folders.lock().unwrap() = folders.clone();
        Ok(folders)
    }
    /// The folders as last read (read now if they never were).
    pub fn folders(&self) -> Result<Vec<MailFolder>> {
        let known = self.folders.lock().unwrap().clone();
        if known.is_empty() { self.reload_folders() } else { Ok(known) }
    }
    /// The space the mailbox's mail takes, in bytes: the sum of its mail
    /// folders' sizes (PR_MESSAGE_SIZE_EXTENDED, an extended property),
    /// where Microsoft says them. Graph doesn't tell a user their quota.
    pub fn mailbox_size(&self) -> Result<Option<u64>> {
        let folders = self.folders()?;
        let requests: Vec<Value> = folders
            .iter()
            .map(|f| {
                json!({"method": "GET", "url": format!(
                    "/me/mailFolders/{}?$select=id&$expand=singleValueExtendedProperties($filter=id%20eq%20'Long%200x0E08')",
                    escape(&f.id)
                )})
            })
            .collect();
        let mut sizes = Vec::new();
        for (status, body) in self.batch(requests)? {
            if status != 200 {
                continue;
            }
            for p in body["singleValueExtendedProperties"].as_array().into_iter().flatten() {
                // Named `Long 0x0E08` or `Long 0xe08`.
                let id = text(&p["id"]);
                let tag = id.split_whitespace().nth(1).and_then(|t| t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")));
                if tag.and_then(|t| u32::from_str_radix(t, 16).ok()) == Some(0x0E08)
                    && let Ok(size) = text(&p["value"]).trim().parse::<u64>()
                {
                    sizes.push(size);
                }
            }
        }
        Ok((!sizes.is_empty()).then(|| sizes.iter().sum()))
    }
    /// Something that changes whenever the inbox does: its counts and its
    /// newest message's id and last change.
    pub fn inbox_signature(&self) -> Result<String> {
        let folder = self.rest.get(&Self::p("/me/mailFolders/inbox?$select=totalItemCount,unreadItemCount"))?;
        let newest = self.rest.get(&Self::p(
            "/me/mailFolders/inbox/messages?$top=1&$select=id,lastModifiedDateTime&$orderby=lastModifiedDateTime%20desc",
        ))?;
        Ok(format!(
            "{}:{}:{}:{}",
            folder["totalItemCount"],
            folder["unreadItemCount"],
            newest["value"][0]["id"],
            newest["value"][0]["lastModifiedDateTime"]
        ))
    }
    pub fn folder_total(&self, id: &str) -> Result<u64> {
        let v = self.rest.get(&Self::p(&format!("/me/mailFolders/{}?$select=totalItemCount", escape(id))))?;
        Ok(v["totalItemCount"].as_u64().unwrap_or_default())
    }
    pub fn create_folder(&self, parent: Option<&str>, name: &str) -> Result<String> {
        let path = match parent {
            Some(p) => format!("/me/mailFolders/{}/childFolders", escape(p)),
            None => "/me/mailFolders".to_owned(),
        };
        let v = self.rest.post(&Self::p(&path), &json!({"displayName": name}))?;
        self.reload_folders()?;
        Ok(text(&v["id"]))
    }
    pub fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        self.rest.patch(&Self::p(&format!("/me/mailFolders/{}", escape(id))), &json!({"displayName": name}))?;
        self.reload_folders()?;
        Ok(())
    }
    pub fn delete_folder(&self, id: &str) -> Result<()> {
        self.rest.delete(&Self::p(&format!("/me/mailFolders/{}", escape(id))))?;
        self.reload_folders()?;
        Ok(())
    }

    // Messages.

    /// One page of a folder's messages, newest first (since `since`, where
    /// given), and the next page's address.
    pub fn list(
        &self,
        folder: &str,
        since: Option<i64>,
        next: Option<&str>,
        top: usize,
    ) -> Result<(Vec<Message>, Option<String>)> {
        let path = match next {
            Some(next) => next.to_owned(),
            None => {
                let mut path = format!(
                    "/me/mailFolders/{}/messages?$top={}&$select={SELECT}&$expand={}&$orderby=receivedDateTime%20desc",
                    escape(folder),
                    top.clamp(1, 1000),
                    escape(EXPAND)
                );
                if let Some(since) = since {
                    path.push_str(&format!("&$filter={}", escape(&format!("receivedDateTime ge {}", graph_time(since)))));
                }
                Self::p(&path)
            }
        };
        let v = self.rest.get(&path)?;
        let messages = v["value"].as_array().map(|l| l.iter().map(Message::from_json).collect()).unwrap_or_default();
        Ok((messages, v["@odata.nextLink"].as_str().map(str::to_owned)))
    }
    /// What the message list shows of each of `ids`; those gone are left out.
    pub fn messages(&self, ids: &[String]) -> Result<Vec<Message>> {
        let requests: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({"method": "GET", "url": format!("/me/messages/{}?$select={SELECT}&$expand={}", escape(id), escape(EXPAND))})
            })
            .collect();
        let mut out = Vec::new();
        for (status, body) in self.batch(requests)? {
            match status {
                200 => out.push(Message::from_json(&body)),
                404 => {}
                _ => return Err(self.batch_problem(status, &body)),
            }
        }
        Ok(out)
    }
    /// A message's source.
    pub fn raw(&self, id: &str) -> Result<Vec<u8>> {
        self.rest.get_bytes(&Self::p(&format!("/me/messages/{}/$value", escape(id))))
    }
    /// Changes the same properties of each message.
    pub fn update(&self, ids: &[String], patch: &Value) -> Result<()> {
        let requests: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({"method": "PATCH", "url": format!("/me/messages/{}", escape(id)),
                       "headers": {"Content-Type": "application/json"}, "body": patch})
            })
            .collect();
        self.each_done(requests)
    }
    /// Changes each message's own properties: (id, patch).
    pub fn update_each(&self, patches: &[(String, Value)]) -> Result<()> {
        let requests: Vec<Value> = patches
            .iter()
            .map(|(id, patch)| {
                json!({"method": "PATCH", "url": format!("/me/messages/{}", escape(id)),
                       "headers": {"Content-Type": "application/json"}, "body": patch})
            })
            .collect();
        self.each_done(requests)
    }
    /// Moves messages into a folder (by id or well-known name).
    pub fn move_to(&self, ids: &[String], folder: &str) -> Result<()> {
        let requests: Vec<Value> = ids
            .iter()
            .map(|id| {
                json!({"method": "POST", "url": format!("/me/messages/{}/move", escape(id)),
                       "headers": {"Content-Type": "application/json"}, "body": {"destinationId": folder}})
            })
            .collect();
        self.each_done(requests)
    }
    /// Deletes messages for good (not to Deleted Items).
    pub fn delete(&self, ids: &[String]) -> Result<()> {
        let requests: Vec<Value> = ids
            .iter()
            .map(|id| json!({"method": "POST", "url": format!("/me/messages/{}/permanentDelete", escape(id))}))
            .collect();
        self.each_done(requests)
    }
    /// The ids of a folder's messages matching an OData `$filter`, up to `most`.
    pub fn ids_where(&self, folder: &str, filter: &str, most: usize) -> Result<Vec<String>> {
        let mut path = format!("/me/mailFolders/{}/messages?$top=1000&$select=id", escape(folder));
        if !filter.is_empty() {
            path.push_str(&format!("&$filter={}", escape(filter)));
        }
        Ok(self.all_pages(&path, most)?.iter().map(|m| text(&m["id"])).collect())
    }
    /// Microsoft's own search (KQL) within a folder, up to `most` results.
    pub fn search(&self, folder: &str, query: &str, most: usize) -> Result<Vec<Message>> {
        let quoted = format!("\"{}\"", query.replace('"', "'"));
        let path = format!(
            "/me/mailFolders/{}/messages?$top=250&$select={SELECT}&$expand={}&$search={}",
            escape(folder),
            escape(EXPAND),
            escape(&quoted)
        );
        Ok(self.all_pages(&path, most)?.iter().map(Message::from_json).collect())
    }

    /// Sends a finished message: Microsoft sends it to those its To, Cc and
    /// Bcc name and keeps it in Sent Items. One too large for a request
    /// goes as a draft whose files upload in pieces.
    pub fn send(&self, raw: &[u8]) -> Result<()> {
        if raw.len() <= MIME_MOST {
            let body = base64::engine::general_purpose::STANDARD.encode(raw);
            self.rest.upload(reqwest::Method::POST, &Self::p("/me/sendMail"), "text/plain", body.into_bytes())?;
            return Ok(());
        }
        self.send_draft(raw, None)
    }
    /// Has Exchange send a message at `at` (Unix seconds), even with
    /// Picacho closed: a draft with a deferred delivery time
    /// (PR_DEFERRED_SEND_TIME), sent now, which Exchange holds until then.
    pub fn send_later(&self, raw: &[u8], at: i64) -> Result<()> {
        self.send_draft(raw, Some(at))
    }
    /// Whether a message must be taken apart to be sent (too large for
    /// one request), and can't be: signed or encrypted.
    pub fn too_large_to_send(raw: &[u8]) -> bool {
        raw.len() > MIME_MOST
            && mailparse::parse_mail(raw).is_ok_and(|p| {
                let kind = p.ctype.mimetype.to_ascii_lowercase();
                kind == "multipart/signed" || kind == "multipart/encrypted" || kind.contains("pkcs7")
            })
    }
    /// Sends a message as a draft: from MIME where it fits one request;
    /// else its text as a draft, each file uploaded in pieces (an upload
    /// session). With `at`, Exchange holds it until then. A signed or
    /// encrypted message too large for one request can't be taken apart.
    fn send_draft(&self, raw: &[u8], at: Option<i64>) -> Result<()> {
        if Self::too_large_to_send(raw) {
            bail!(t!("sync-graph-signed-or-encrypted-mail-this-large"));
        }
        let (text, files) = if raw.len() <= MIME_MOST {
            (raw.to_vec(), Vec::new())
        } else {
            split_attachments(raw, &mailparse::parse_mail(raw)?)?
        };
        let draft = self.create_mime(&text, None)?;
        let result = (|| {
            for (name, kind, inline_id, bytes) in &files {
                self.upload_attachment(&draft, name, kind, inline_id.as_deref(), bytes)?;
            }
            if let Some(at) = at {
                self.rest.patch(
                    &Self::p(&format!("/me/messages/{}", escape(&draft))),
                    &json!({"singleValueExtendedProperties": [{"id": "SystemTime 0x3FEF", "value": graph_time(at)}]}),
                )?;
            }
            self.rest.post(&Self::p(&format!("/me/messages/{}/send", escape(&draft))), &Value::Null)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.delete_message(&draft);
        }
        result
    }
    /// Adds a file to a draft in 4 MB pieces (an upload session).
    fn upload_attachment(&self, draft: &str, name: &str, kind: &str, inline_id: Option<&str>, bytes: &[u8]) -> Result<()> {
        let mut item = json!({"attachmentType": "file", "name": name, "size": bytes.len(), "contentType": kind});
        if let Some(id) = inline_id {
            item["isInline"] = true.into();
            item["contentId"] = id.into();
        }
        let session = self.rest.post(
            &Self::p(&format!("/me/messages/{}/attachments/createUploadSession", escape(draft))),
            &json!({"AttachmentItem": item}),
        )?;
        let url = session["uploadUrl"].as_str().with_context(|| t!("sync-graph-microsoft-gave-nowhere-to-upload"))?;
        // The upload address is Outlook's own, signed; it takes no token.
        // Over https (in tests, the loopback server's http), never to this
        // computer's network, and not redirected: the file goes where
        // Microsoft said, with the account's TLS settings.
        let parsed = url::Url::parse(url).with_context(|| t!("sync-graph-microsoft-gave-nowhere-to-upload"))?;
        ensure!(
            (parsed.scheme() == "https" || cfg!(test)) && !crate::sync::public_net::reaches_inside(&parsed),
            t!("sync-graph-microsoft-gave-nowhere-to-upload")
        );
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(self.rest.tls.clone())
            .timeout(std::time::Duration::from_secs(600))
            .connect_timeout(std::time::Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .https_only(!cfg!(test))
            .build()?;
        const PIECE: usize = 4 * 1024 * 1024 - 64 * 1024;
        let total = bytes.len();
        for (i, piece) in bytes.chunks(PIECE).enumerate() {
            let start = i * PIECE;
            let end = start + piece.len() - 1;
            let response = http
                .put(parsed.clone())
                .header("Content-Range", format!("bytes {start}-{end}/{total}"))
                .header("Content-Type", "application/octet-stream")
                .body(piece.to_vec())
                .send()?;
            if !response.status().is_success() {
                let status = response.status();
                let body = crate::sync::public_net::bounded_text(response, 64 * 1024).unwrap_or_default();
                return Err(self.rest.problem(status, &body, None));
            }
        }
        Ok(())
    }
    /// Makes a message from MIME, in Drafts (or `folder`); its id. Graph
    /// makes it a draft wherever it goes.
    pub fn create_mime(&self, raw: &[u8], folder: Option<&str>) -> Result<String> {
        if raw.len() > MIME_MOST {
            bail!(t!("sync-graph-the-message-is-too-large-to"));
        }
        let path = match folder {
            Some(f) => format!("/me/mailFolders/{}/messages", escape(f)),
            None => "/me/messages".to_owned(),
        };
        let body = base64::engine::general_purpose::STANDARD.encode(raw);
        let v = self.rest.upload(reqwest::Method::POST, &Self::p(&path), "text/plain", body.into_bytes())?;
        Ok(text(&v["id"]))
    }
    /// Puts a received or sent message in `folder` as a message, not a
    /// draft: Microsoft keeps anything given as MIME as a draft, so it is
    /// made from its parts (people, subject, text, files, dates, its
    /// Message-ID), marked as delivered (PR_MESSAGE_FLAGS without
    /// MSGFLAG_UNSENT). Signed or encrypted mail can only keep its form as
    /// MIME, and goes so (`create_mime`). Its id.
    pub fn import(&self, raw: &[u8], folder: &str, read: bool, flagged: bool) -> Result<String> {
        let parsed = mailparse::parse_mail(raw)?;
        let kind = parsed.ctype.mimetype.to_ascii_lowercase();
        if kind == "multipart/signed" || kind == "multipart/encrypted" || kind.contains("pkcs7") {
            return self.create_mime(raw, Some(folder));
        }
        use mailparse::MailHeaderMap as _;
        let header = |name: &str| parsed.headers.get_first_value(name).unwrap_or_default();
        let people = |name: &str| -> Vec<Value> {
            mailparse::addrparse(&header(name))
                .map(|list| {
                    list.iter()
                        .flat_map(|a| match a {
                            mailparse::MailAddr::Single(s) => vec![s.clone()],
                            mailparse::MailAddr::Group(g) => g.addrs.clone(),
                        })
                        .map(|s| json!({"emailAddress": {"address": s.addr, "name": s.display_name.unwrap_or_default()}}))
                        .collect()
                })
                .unwrap_or_default()
        };
        let (mut plain, mut html, mut files): (Option<String>, Option<String>, Files) = (None, None, Vec::new());
        let mut leaves = Vec::new();
        super::mime::collect_leaves(&parsed, &mut leaves);
        for leaf in leaves {
            let disposition = leaf.get_content_disposition();
            let name = disposition.params.get("filename").or_else(|| leaf.ctype.params.get("name")).cloned();
            let content_id = leaf.headers.get_first_value("Content-ID").map(|v| v.trim().trim_matches(['<', '>']).to_owned());
            let mime = leaf.ctype.mimetype.to_ascii_lowercase();
            let attached = name.is_some() || disposition.disposition == mailparse::DispositionType::Attachment;
            if !attached && mime == "text/html" && html.is_none() {
                html = leaf.get_body().ok();
            } else if !attached && mime == "text/plain" && plain.is_none() {
                plain = leaf.get_body().ok();
            } else if attached || content_id.is_some() {
                files.push((name.unwrap_or_else(|| "attachment".into()), mime, content_id, leaf.get_body_raw()?));
            }
        }
        let date = mailparse::dateparse(&header("Date")).unwrap_or_else(|_| chrono::Utc::now().timestamp());
        let mut item = json!({
            "subject": header("Subject"),
            "body": match &html {
                Some(h) => json!({"contentType": "html", "content": h}),
                None => json!({"contentType": "text", "content": plain.unwrap_or_default()}),
            },
            "toRecipients": people("To"),
            "ccRecipients": people("Cc"),
            "bccRecipients": people("Bcc"),
            "replyTo": people("Reply-To"),
            "isRead": read,
            "flag": {"flagStatus": if flagged { "flagged" } else { "notFlagged" }},
            "singleValueExtendedProperties": [
                // Delivered (no MSGFLAG_UNSENT), read where it was.
                {"id": "Integer 0x0E07", "value": if read { "1" } else { "0" }},
                {"id": "SystemTime 0x0039", "value": graph_time(date)},
                {"id": "SystemTime 0x0E06", "value": graph_time(date)},
            ],
        });
        if let Some(from) = people("From").into_iter().next() {
            item["from"] = from.clone();
            item["sender"] = from;
        }
        let id = header("Message-ID");
        if !id.trim().is_empty() {
            item["internetMessageId"] = id.trim().into();
        }
        // Small files go with it; the rest one at a time after.
        const WITH_IT: usize = 2 * 1024 * 1024;
        let (small, large): (Files, Files) = {
            let mut total = 0;
            files.into_iter().partition(|f| {
                total += f.3.len();
                total <= WITH_IT
            })
        };
        let attachment = |(name, kind, cid, bytes): &(String, String, Option<String>, Vec<u8>)| {
            let mut a = json!({
                "@odata.type": "#microsoft.graph.fileAttachment",
                "name": name, "contentType": kind,
                "contentBytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            });
            if let Some(cid) = cid {
                a["isInline"] = true.into();
                a["contentId"] = cid.clone().into();
            }
            a
        };
        item["attachments"] = small.iter().map(attachment).collect::<Vec<_>>().into();
        let made = self.rest.post(&Self::p(&format!("/me/mailFolders/{}/messages", escape(folder))), &item)?;
        let id = text(&made["id"]);
        for f in &large {
            if f.3.len() < 3 * 1024 * 1024 {
                self.rest.post(&Self::p(&format!("/me/messages/{}/attachments", escape(&id))), &attachment(f))?;
            } else {
                self.upload_attachment(&id, &f.0, &f.1, f.2.as_deref(), &f.3)?;
            }
        }
        Ok(id)
    }
    pub fn delete_message(&self, id: &str) -> Result<()> {
        self.rest.delete(&Self::p(&format!("/me/messages/{}", escape(id))))
    }
    /// The drafts' ids.
    pub fn draft_ids(&self, most: usize) -> Result<Vec<String>> {
        Ok(self
            .all_pages("/me/mailFolders/drafts/messages?$top=250&$select=id", most)?
            .iter()
            .map(|m| text(&m["id"]))
            .collect())
    }

    // Settings.

    pub fn rules(&self) -> Result<Vec<Rule>> {
        Ok(self
            .all_pages("/me/mailFolders/inbox/messageRules", 1000)?
            .into_iter()
            .map(|json| Rule { id: text(&json["id"]), json })
            .collect())
    }
    pub fn create_rule(&self, rule: &Value) -> Result<String> {
        let v = self.rest.post(&Self::p("/me/mailFolders/inbox/messageRules"), rule)?;
        Ok(text(&v["id"]))
    }
    pub fn update_rule(&self, id: &str, rule: &Value) -> Result<()> {
        self.rest.patch(&Self::p(&format!("/me/mailFolders/inbox/messageRules/{}", escape(id))), rule)?;
        Ok(())
    }
    pub fn delete_rule(&self, id: &str) -> Result<()> {
        self.rest.delete(&Self::p(&format!("/me/mailFolders/inbox/messageRules/{}", escape(id))))
    }
    pub fn automatic_replies(&self) -> Result<AutomaticReplies> {
        let v = self.rest.get(&Self::p("/me/mailboxSettings/automaticRepliesSetting"))?;
        let at = |v: &Value| {
            let date = v["dateTime"].as_str().unwrap_or_default();
            chrono::NaiveDateTime::parse_from_str(date.split('.').next().unwrap_or(date), "%Y-%m-%dT%H:%M:%S")
                .map_or(0, |d| d.and_utc().timestamp())
        };
        let status = v["status"].as_str().unwrap_or("disabled");
        let scheduled = status == "scheduled";
        Ok(AutomaticReplies {
            enabled: status != "disabled",
            external: text(&v["externalReplyMessage"]),
            internal: text(&v["internalReplyMessage"]),
            contacts_only: v["externalAudience"].as_str() == Some("contactsOnly"),
            start: if scheduled { at(&v["scheduledStartDateTime"]) } else { 0 },
            end: if scheduled { at(&v["scheduledEndDateTime"]) } else { 0 },
        })
    }
    pub fn set_automatic_replies(&self, r: &AutomaticReplies) -> Result<()> {
        let scheduled = r.enabled && r.start > 0 && r.end > r.start;
        let utc = |at: i64| json!({"dateTime": graph_time(at).trim_end_matches('Z'), "timeZone": "UTC"});
        let mut setting = json!({
            "status": if !r.enabled { "disabled" } else if scheduled { "scheduled" } else { "alwaysEnabled" },
            "externalAudience": if r.contacts_only { "contactsOnly" } else { "all" },
            "internalReplyMessage": r.internal,
            "externalReplyMessage": if r.external.is_empty() { &r.internal } else { &r.external },
        });
        if scheduled {
            setting["scheduledStartDateTime"] = utc(r.start);
            setting["scheduledEndDateTime"] = utc(r.end);
        }
        self.rest.patch(&Self::p("/me/mailboxSettings"), &json!({"automaticRepliesSetting": setting}))?;
        Ok(())
    }

    // Batches.

    /// Sends `requests` (each a JSON batch request without its id) in
    /// batches of 20: each one's status and body, in order. Requests
    /// Microsoft turned away for going too fast are sent again, twice at
    /// most.
    pub fn batch(&self, requests: Vec<Value>) -> Result<Vec<(u16, Value)>> {
        let mut answers: Vec<Option<(u16, Value)>> = vec![None; requests.len()];
        for attempt in 0..3u64 {
            let waiting: Vec<usize> = (0..requests.len())
                .filter(|&i| answers[i].as_ref().is_none_or(|(s, _)| *s == 429 && attempt > 0))
                .collect();
            if waiting.is_empty() {
                break;
            }
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(800 * attempt));
            }
            for chunk in waiting.chunks(BATCH) {
                let batch: Vec<Value> = chunk
                    .iter()
                    .map(|&i| {
                        let mut r = requests[i].clone();
                        r["id"] = i.to_string().into();
                        r
                    })
                    .collect();
                let v = self.rest.post(&Self::p("/$batch"), &json!({"requests": batch}))?;
                for r in v["responses"].as_array().into_iter().flatten() {
                    let Some(i) = r["id"].as_str().and_then(|i| i.parse::<usize>().ok()) else { continue };
                    if i < answers.len() {
                        answers[i] = Some((r["status"].as_u64().unwrap_or(502) as u16, r["body"].clone()));
                    }
                }
            }
        }
        answers
            .into_iter()
            .map(|a| match a {
                Some((429, _)) => bail!(
                    "[THROTTLED] {}",
                    t!("sync-rest-is-limiting-requests", service = "Microsoft 365", status = 429u16)
                ),
                Some(a) => Ok(a),
                None => bail!(t!("sync-gmail-gmail-left-a-request-unanswered")),
            })
            .collect()
    }
    /// A batch's requests, each of which must succeed (those already gone
    /// count as done).
    fn each_done(&self, requests: Vec<Value>) -> Result<()> {
        for (status, body) in self.batch(requests)? {
            if !(200..300).contains(&status) && status != 404 {
                return Err(self.batch_problem(status, &body));
            }
        }
        Ok(())
    }
    fn batch_problem(&self, status: u16, body: &Value) -> anyhow::Error {
        let code = reqwest::StatusCode::from_u16(status).unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
        self.rest.problem(code, &body.to_string(), None)
    }
}

/// A message without its files (as MIME, each file's part left out), and
/// the files: (name, type, Content-ID for one shown in the text, bytes).
type Files = Vec<(String, String, Option<String>, Vec<u8>)>;
fn split_attachments(raw: &[u8], parsed: &mailparse::ParsedMail) -> Result<(Vec<u8>, Files)> {
    let mut files = Vec::new();
    let mut leaves = Vec::new();
    super::mime::collect_leaves(parsed, &mut leaves);
    let mut cut: Vec<&[u8]> = Vec::new();
    for leaf in leaves {
        let disposition = leaf.get_content_disposition();
        let name = disposition.params.get("filename").or_else(|| leaf.ctype.params.get("name")).cloned();
        let inline_id = leaf
            .headers
            .iter()
            .find(|h| h.get_key().eq_ignore_ascii_case("Content-ID"))
            .map(|h| h.get_value().trim().trim_matches(['<', '>']).to_owned());
        let is_file = name.is_some() || disposition.disposition == mailparse::DispositionType::Attachment;
        if is_file && !leaf.ctype.mimetype.starts_with("multipart/") {
            files.push((
                name.unwrap_or_else(|| "attachment".into()),
                leaf.ctype.mimetype.clone(),
                inline_id,
                leaf.get_body_raw()?,
            ));
            cut.push(leaf.raw_bytes);
        }
    }
    // Each file's part (its headers and body) left out of the source.
    let mut text = raw.to_vec();
    for part in cut {
        if let Some(at) = memchr::memmem::find(&text, part) {
            text.splice(at..at + part.len(), b"Content-Type: text/plain\r\n\r\n".iter().copied());
        }
    }
    Ok((text, files))
}

#[cfg(test)]
pub(crate) mod tests;
