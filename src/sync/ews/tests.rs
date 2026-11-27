//! A small Exchange Server over loopback HTTP, answering EWS SOAP as the
//! documentation describes and signing in with NTLMv2 (checked against
//! its password), and tests of the client against it. Manage's tests
//! (`manage/ews_tests.rs`) use it too.
use super::*;
use crate::sync::rest::test_server::{self, FullAnswer, Request};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct FakeFolder {
    pub id: String,
    pub name: String,
    pub parent: String,
    pub distinguished: Option<String>,
    pub class: String,
}
#[derive(Clone, Debug)]
pub(crate) struct FakeItem {
    pub id: String,
    pub folder: String,
    /// MIME (a message's source, an event's iCalendar, a contact's vCard).
    pub mime: Vec<u8>,
    pub subject: String,
    pub read: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub verb: Option<i64>,
    pub received: i64,
    /// PR_MESSAGE_FLAGS as made (8: unsent, a draft).
    pub flags: i64,
    pub kind: &'static str,
    #[allow(dead_code)]
    pub deferred: Option<String>,
}

pub(crate) struct Fake {
    pub password: String,
    pub folders: Vec<FakeFolder>,
    pub items: Vec<FakeItem>,
    pub sent: Vec<(Vec<u8>, Option<String>)>,
    pub rules: Vec<String>,
    pub oof: String,
    pub next: u64,
    pub challenge: [u8; 8],
    /// Operations called, in order.
    pub calls: Vec<String>,
    /// Signed-in requests, and refused ones.
    pub signed_in: usize,
    pub refused: usize,
    /// Accepted, tentatively accepted or declined meetings, by id.
    pub answered: Vec<(String, String)>,
    /// People the directory knows: (name, address).
    pub people: Vec<(String, String)>,
    /// What each streaming request answers, in turn: "event" (a new
    /// message), "forget" (the subscription is gone); then only keep-alives.
    pub stream: Vec<&'static str>,
    /// An address Autodiscover sends on to another (from, to).
    pub redirect_addr: Option<(String, String)>,
    /// Refuses a calendar item whose UID one in its folder already has.
    pub unique_uids: bool,
    /// Item classes other than IPM.Note, by item id.
    pub classes: Vec<(String, String)>,
}

impl Fake {
    pub fn new(password: &str) -> Self {
        let mut f = Fake {
            password: password.into(),
            folders: Vec::new(),
            items: Vec::new(),
            sent: Vec::new(),
            rules: Vec::new(),
            oof: "<t:OofState>Disabled</t:OofState>".into(),
            next: 1,
            challenge: [7; 8],
            calls: Vec::new(),
            signed_in: 0,
            refused: 0,
            answered: Vec::new(),
            people: Vec::new(),
            stream: Vec::new(),
            redirect_addr: None,
            unique_uids: false,
            classes: Vec::new(),
        };
        f.folders.push(FakeFolder { id: "root".into(), name: "Top of Information Store".into(), parent: "mailbox".into(), distinguished: Some("msgfolderroot".into()), class: String::new() });
        for (id, name, d, class) in [
            ("f-inbox", "Inbox", Some("inbox"), "IPF.Note"),
            ("f-sent", "Sent Items", Some("sentitems"), "IPF.Note"),
            ("f-drafts", "Drafts", Some("drafts"), "IPF.Note"),
            ("f-trash", "Deleted Items", Some("deleteditems"), "IPF.Note"),
            ("f-junk", "Junk Email", Some("junkemail"), "IPF.Note"),
            ("f-outbox", "Outbox", Some("outbox"), "IPF.Note"),
            ("f-archive", "Archive", None, "IPF.Note"),
            ("f-cal", "Calendar", Some("calendar"), "IPF.Appointment"),
            ("f-contacts", "Contacts", Some("contacts"), "IPF.Contact"),
        ] {
            f.folders.push(FakeFolder { id: id.into(), name: name.into(), parent: "root".into(), distinguished: d.map(str::to_owned), class: class.into() });
        }
        f
    }
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}AAMk{}=", self.next)
    }
    fn resolve(&self, n: Node) -> Option<String> {
        match n.tag_name().name() {
            "DistinguishedFolderId" => {
                let d = n.attribute("Id")?;
                self.folders.iter().find(|f| f.distinguished.as_deref() == Some(d)).map(|f| f.id.clone())
            }
            _ => n.attribute("Id").map(str::to_owned),
        }
    }
    pub fn folder(&mut self, name: &str, parent: &str) -> String {
        let id = self.id("F");
        self.folders.push(FakeFolder { id: id.clone(), name: name.into(), parent: parent.into(), distinguished: None, class: "IPF.Note".into() });
        id
    }
    pub fn add(&mut self, subject: &str, folder: &str, minutes_ago: i64) -> String {
        let id = self.id("I");
        let received = chrono::Utc::now().timestamp() - minutes_ago * 60;
        let date = chrono::DateTime::from_timestamp(received, 0).unwrap().to_rfc2822();
        let mime = format!("From: Bo Berg <bo@example.se>\r\nTo: qa@corp.example\r\nSubject: {subject}\r\nDate: {date}\r\nMessage-ID: <{id}@corp.example>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nAbout {subject}.\r\n");
        self.items.push(FakeItem {
            id: id.clone(), folder: folder.into(), mime: mime.into_bytes(), subject: subject.into(), read: false,
            flagged: false, categories: Vec::new(), verb: None, received, flags: 1, kind: "Message", deferred: None,
        });
        id
    }
    pub fn item(&self, id: &str) -> Option<&FakeItem> {
        self.items.iter().find(|i| i.id == id)
    }
    fn item_xml(&self, i: &FakeItem, mime: bool) -> String {
        let mut x = format!(r#"<t:{} xmlns:t="{TYPES}"><t:ItemId Id="{}" ChangeKey="ck"/>"#, i.kind, esc(&i.id));
        if mime {
            x.push_str(&format!("<t:MimeContent>{}</t:MimeContent>", b64(&i.mime)));
        }
        x.push_str(&format!(
            r#"<t:ParentFolderId Id="{}"/><t:Subject>{}</t:Subject><t:DateTimeReceived>{}</t:DateTimeReceived><t:Size>{}</t:Size><t:HasAttachments>false</t:HasAttachments><t:DisplayTo>qa</t:DisplayTo>"#,
            esc(&i.folder), esc(&i.subject), ews_time(i.received), i.mime.len()
        ));
        if !i.categories.is_empty() {
            x.push_str("<t:Categories>");
            for c in &i.categories {
                x.push_str(&format!("<t:String>{}</t:String>", esc(c)));
            }
            x.push_str("</t:Categories>");
        }
        x.push_str(&format!(r#"<t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="0x1090" PropertyType="Integer"/><t:Value>{}</t:Value></t:ExtendedProperty>"#, if i.flagged { 2 } else { 0 }));
        if let Some(v) = i.verb {
            x.push_str(&format!(r#"<t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="0x1081" PropertyType="Integer"/><t:Value>{v}</t:Value></t:ExtendedProperty>"#));
        }
        // A conversation is its subject here.
        let class = self.classes.iter().find(|(id, _)| *id == i.id).map_or("IPM.Note", |(_, c)| c.as_str());
        x.push_str(&format!(r#"<t:ItemClass>{}</t:ItemClass><t:ConversationId Id="c-{}"/>"#, esc(class), esc(&i.subject)));
        x.push_str(&format!(r#"<t:From><t:Mailbox><t:Name>Bo Berg</t:Name><t:EmailAddress>bo@example.se</t:EmailAddress></t:Mailbox></t:From><t:IsRead>{}</t:IsRead><t:InternetMessageId>&lt;{}@corp.example&gt;</t:InternetMessageId>"#, i.read, esc(&i.id)));
        x.push_str(&format!("</t:{}>", i.kind));
        x
    }

    /// Answers a request, checking its NTLM sign-in.
    pub fn respond(&mut self, r: &Request) -> FullAnswer {
        let xml = |status: u16, body: String| -> FullAnswer { (status, "text/xml; charset=utf-8".into(), body.into_bytes(), Vec::new()) };
        // An access token from the organization's sign-in.
        let bearer = r.authorization == "Bearer entra-token";
        // NTLM: NEGOTIATE gets the challenge; AUTHENTICATE is checked.
        let Some(token) = r.authorization.strip_prefix("NTLM ").or(bearer.then_some("")) else {
            self.refused += 1;
            return (401, "text/plain".into(), Vec::new(), vec![("WWW-Authenticate".into(), "NTLM".into())]);
        };
        let message = unb64(token).unwrap_or_default();
        if !bearer && message.get(8) == Some(&1) {
            let mut c = b"NTLMSSP\0".to_vec();
            c.extend_from_slice(&2u32.to_le_bytes());
            c.extend_from_slice(&[0; 8]);
            c.extend_from_slice(&0xa288_8205u32.to_le_bytes());
            c.extend_from_slice(&self.challenge);
            c.extend_from_slice(&[0; 8]);
            c.extend_from_slice(&[0; 8]); // no target info
            return (401, "text/plain".into(), Vec::new(), vec![("WWW-Authenticate".into(), format!("NTLM {}", b64(&c)))]);
        }
        if !bearer && !self.ntlm_ok(&message) {
            self.refused += 1;
            return (401, "text/plain".into(), Vec::new(), vec![("WWW-Authenticate".into(), "NTLM".into())]);
        }
        self.signed_in += 1;
        let text = String::from_utf8_lossy(&r.body).into_owned();
        let url = format!("http://127.0.0.1:{}/EWS/Exchange.asmx", self.port());
        // A server with POX turned off: SOAP Autodiscover only.
        if r.path.starts_with("/nopox/") && r.path.ends_with("/autodiscover.xml") {
            return (404, "text/plain".into(), Vec::new(), Vec::new());
        }
        if r.path.ends_with("/autodiscover/autodiscover.svc") {
            return xml(200, format!(r#"<?xml version="1.0"?><s:Envelope xmlns:s="{SOAP}"><s:Body><GetUserSettingsResponseMessage xmlns="http://schemas.microsoft.com/exchange/2010/Autodiscover"><Response><ErrorCode>NoError</ErrorCode><UserResponses><UserResponse><ErrorCode>NoError</ErrorCode><UserSettings><UserSetting><Name>ExternalEwsUrl</Name><Value>https://outside.example/EWS/Exchange.asmx</Value></UserSetting><UserSetting><Name>InternalEwsUrl</Name><Value>{url}</Value></UserSetting></UserSettings></UserResponse></UserResponses></Response></GetUserSettingsResponseMessage></s:Body></s:Envelope>"#));
        }
        if r.path.ends_with("/autodiscover/autodiscover.xml") {
            let asked = String::from_utf8_lossy(&r.body).split("<EMailAddress>").nth(1).and_then(|t| t.split('<').next()).unwrap_or_default().to_owned();
            if let Some((from, to)) = &self.redirect_addr
                && *from == asked
            {
                return xml(200, format!(r#"<?xml version="1.0"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006"><Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a"><Account><Action>redirectAddr</Action><RedirectAddr>{to}</RedirectAddr></Account></Response></Autodiscover>"#));
            }
            return xml(200, format!(r#"<?xml version="1.0"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006"><Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a"><Account><Protocol><Type>EXCH</Type><EwsUrl>{url}</EwsUrl></Protocol></Account></Response></Autodiscover>"#));
        }
        let doc = roxmltree::Document::parse(&text).expect("SOAP");
        let body = find(doc.root(), "Body").expect("a body");
        let op = body.children().find(|c| c.is_element()).expect("an operation");
        let name = op.tag_name().name().to_owned();
        self.calls.push(name.clone());
        // Streaming: envelopes one after another, the last closing it.
        if name == "GetStreamingEvents" {
            let envelope = |inner: &str| format!(r#"<s:Envelope xmlns:s="{SOAP}"><s:Body><m:GetStreamingEventsResponse xmlns:m="{MESSAGES}" xmlns:t="{TYPES}"><m:ResponseMessages>{inner}</m:ResponseMessages></m:GetStreamingEventsResponse></s:Body></s:Envelope>"#);
            let keep_alive = Self::ok("GetStreamingEvents", "<m:ConnectionStatus>OK</m:ConnectionStatus>");
            let closed = Self::ok("GetStreamingEvents", "<m:ConnectionStatus>Closed</m:ConnectionStatus>");
            let next = if self.stream.is_empty() { "quiet" } else { self.stream.remove(0) };
            let body = match next {
                "event" => format!(
                    "{}{}{}",
                    envelope(&keep_alive),
                    envelope(&Self::ok("GetStreamingEvents", "<m:Notifications><m:Notification><t:SubscriptionId>sub</t:SubscriptionId><t:NewMailEvent><t:TimeStamp>2026-11-25T10:00:00Z</t:TimeStamp><t:ItemId Id=\"x\"/><t:ParentFolderId Id=\"f-inbox\"/></t:NewMailEvent></m:Notification></m:Notifications>")),
                    envelope(&closed)
                ),
                "forget" => envelope(&Self::error("GetStreamingEvents", "ErrorSubscriptionNotFound")),
                _ => format!("{}{}", envelope(&keep_alive), envelope(&closed)),
            };
            return xml(200, body);
        }
        let inner = self.operation(&name, op);
        xml(200, format!(r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="{SOAP}"><s:Body><m:{name}Response xmlns:m="{MESSAGES}" xmlns:t="{TYPES}"><m:ResponseMessages>{inner}</m:ResponseMessages></m:{name}Response></s:Body></s:Envelope>"#))
    }
    fn port(&self) -> u16 {
        PORT.with(|p| *p.borrow())
    }
    fn ntlm_ok(&self, m: &[u8]) -> bool {
        let field = |at: usize| -> Option<Vec<u8>> {
            let len = u16::from_le_bytes(m.get(at..at + 2)?.try_into().ok()?) as usize;
            let off = u32::from_le_bytes(m.get(at + 4..at + 8)?.try_into().ok()?) as usize;
            m.get(off..off + len).map(<[u8]>::to_vec)
        };
        let (Some(nt), Some(domain), Some(user)) = (field(20), field(28), field(36)) else { return false };
        if nt.len() < 16 {
            return false;
        }
        let from16 = |b: &[u8]| String::from_utf16_lossy(&b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
        let id = ntlm::Identity { user: from16(&user), domain: from16(&domain), password: self.password.clone() };
        let blob = &nt[16..];
        // Recomputed from the client's own blob: the same proof only with the password.
        let challenge = ntlm::Challenge { flags: 0, server_challenge: self.challenge, target_info: blob[28..blob.len() - 4].to_vec() };
        let time: [u8; 8] = blob[8..16].try_into().unwrap();
        let client: [u8; 8] = blob[16..24].try_into().unwrap();
        ntlm::ntlmv2_response(&id, &challenge, time, client) == nt
    }

    fn ok(name: &str, inner: &str) -> String {
        format!(r#"<m:{name}ResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode>{inner}</m:{name}ResponseMessage>"#)
    }
    fn error(name: &str, code: &str) -> String {
        format!(r#"<m:{name}ResponseMessage ResponseClass="Error"><m:MessageText>{code}</m:MessageText><m:ResponseCode>{code}</m:ResponseCode></m:{name}ResponseMessage>"#)
    }
    fn folder_xml(&self, f: &FakeFolder) -> String {
        let total = self.items.iter().filter(|i| i.folder == f.id).count();
        let tag = if f.class == "IPF.Appointment" { "CalendarFolder" } else if f.class == "IPF.Contact" { "ContactsFolder" } else { "Folder" };
        let size: usize = self.items.iter().filter(|i| i.folder == f.id).map(|i| i.mime.len()).sum();
        format!(
            r#"<t:{tag}><t:FolderId Id="{}"/><t:ParentFolderId Id="{}"/><t:FolderClass>{}</t:FolderClass><t:DisplayName>{}</t:DisplayName><t:TotalCount>{total}</t:TotalCount><t:UnreadCount>{}</t:UnreadCount><t:ExtendedProperty><t:ExtendedFieldURI PropertyTag="0xe08" PropertyType="Long"/><t:Value>{size}</t:Value></t:ExtendedProperty></t:{tag}>"#,
            esc(&f.id), esc(&f.parent), f.class, esc(&f.name),
            self.items.iter().filter(|i| i.folder == f.id && !i.read).count()
        )
    }
    fn operation(&mut self, name: &str, op: Node) -> String {
        match name {
            "GetFolder" => child(op, "FolderIds")
                .unwrap()
                .children()
                .filter(|c| c.is_element())
                .map(|f| match self.resolve(f).and_then(|id| self.folders.iter().find(|x| x.id == id).cloned()) {
                    Some(folder) => Self::ok("GetFolder", &format!("<m:Folders>{}</m:Folders>", self.folder_xml(&folder))),
                    None => Self::error("GetFolder", "ErrorFolderNotFound"),
                })
                .collect(),
            "FindFolder" => {
                let parent = self.resolve(child(child(op, "ParentFolderIds").unwrap(), "DistinguishedFolderId").or_else(|| child(child(op, "ParentFolderIds").unwrap(), "FolderId")).unwrap()).unwrap();
                // Deep: everything under the parent.
                let mut under = vec![parent];
                let mut found = Vec::new();
                while let Some(p) = under.pop() {
                    for f in self.folders.iter().filter(|f| f.parent == p) {
                        found.push(f.clone());
                        under.push(f.id.clone());
                    }
                }
                let folders: String = found.iter().map(|f| self.folder_xml(f)).collect();
                Self::ok("FindFolder", &format!(r#"<m:RootFolder IncludesLastItemInRange="true" TotalItemsInView="{}"><t:Folders>{folders}</t:Folders></m:RootFolder>"#, found.len()))
            }
            "CreateFolder" => {
                let parent = self.resolve(child(op, "ParentFolderId").unwrap().children().find(|c| c.is_element()).unwrap()).unwrap();
                let made = find(op, "DisplayName").unwrap().text().unwrap().to_owned();
                let calendar = find(op, "CalendarFolder").is_some();
                let id = self.folder(&made, &parent);
                if calendar {
                    self.folders.last_mut().unwrap().class = "IPF.Appointment".into();
                }
                Self::ok("CreateFolder", &format!(r#"<m:Folders><t:Folder><t:FolderId Id="{id}"/></t:Folder></m:Folders>"#))
            }
            "UpdateFolder" => {
                let id = self.resolve(find(op, "FolderId").unwrap()).unwrap();
                let new = find(child(op, "FolderChanges").unwrap(), "DisplayName").and_then(|n| n.text()).unwrap().to_owned();
                self.folders.iter_mut().find(|f| f.id == id).unwrap().name = new;
                Self::ok("UpdateFolder", "")
            }
            "DeleteFolder" => {
                let id = self.resolve(find(op, "FolderId").unwrap()).unwrap();
                self.folders.retain(|f| f.id != id);
                for i in self.items.iter_mut().filter(|i| i.folder == id) {
                    i.folder = "f-trash".into();
                }
                Self::ok("DeleteFolder", "")
            }
            "FindItem" => {
                let parents = child(op, "ParentFolderIds").unwrap();
                let folder = self.resolve(parents.children().find(|c| c.is_element()).unwrap()).unwrap();
                let view = child(op, "IndexedPageItemView");
                let max: usize = view.and_then(|v| v.attribute("MaxEntriesReturned")).and_then(|m| m.parse().ok()).unwrap_or(1000);
                let offset: usize = view.and_then(|v| v.attribute("Offset")).and_then(|m| m.parse().ok()).unwrap_or(0);
                let unread_only = find(op, "IsEqualTo").and_then(|e| find(e, "FieldURI")).and_then(|f| f.attribute("FieldURI")) == Some("message:IsRead");
                let query = child(op, "QueryString").and_then(|q| q.text()).map(|q| q.to_lowercase());
                let mut found: Vec<&FakeItem> = self
                    .items
                    .iter()
                    .filter(|i| i.folder == folder || folder == "root")
                    .filter(|i| !unread_only || !i.read)
                    .filter(|i| query.as_ref().is_none_or(|q| i.subject.to_lowercase().contains(q)))
                    .collect();
                found.sort_by_key(|i| std::cmp::Reverse(i.received));
                let page: String = found.iter().skip(offset).take(max).map(|i| self.item_xml(i, false)).collect();
                let last = offset + max >= found.len();
                Self::ok("FindItem", &format!(r#"<m:RootFolder IncludesLastItemInRange="{last}" TotalItemsInView="{}"><t:Items>{page}</t:Items></m:RootFolder>"#, found.len()))
            }
            "GetConversationItems" => {
                let id = find(op, "ConversationId").and_then(|c| c.attribute("Id")).unwrap_or_default().to_owned();
                let ignored = ["f-trash", "f-drafts"];
                let items: String = self
                    .items
                    .iter()
                    .filter(|i| format!("c-{}", i.subject) == id && !ignored.contains(&i.folder.as_str()))
                    .map(|i| self.item_xml(i, false))
                    .collect();
                Self::ok(
                    "GetConversationItems",
                    &format!("<m:Conversation><t:ConversationNodes><t:ConversationNode><t:Items>{items}</t:Items></t:ConversationNode></t:ConversationNodes></m:Conversation>"),
                )
            }
            "GetItem" => {
                let mime = find(op, "IncludeMimeContent").and_then(|n| n.text()) == Some("true");
                children(child(op, "ItemIds").unwrap(), "ItemId")
                    .map(|i| match self.item(i.attribute("Id").unwrap()) {
                        Some(item) => Self::ok("GetItem", &format!("<m:Items>{}</m:Items>", self.item_xml(item, mime))),
                        None => Self::error("GetItem", "ErrorItemNotFound"),
                    })
                    .collect()
            }
            "UpdateItem" => {
                let mut out = String::new();
                for change in children(child(op, "ItemChanges").unwrap(), "ItemChange") {
                    let id = find(change, "ItemId").unwrap().attribute("Id").unwrap().to_owned();
                    let Some(item) = self.items.iter_mut().find(|i| i.id == id) else {
                        out.push_str(&Self::error("UpdateItem", "ErrorItemNotFound"));
                        continue;
                    };
                    // Exchange wants each change written for the item's own kind.
                    let wrong_kind = find(change, "Updates").unwrap().children().filter(|c| c.is_element()).any(|set| {
                        set.children().filter(|c| c.is_element()).nth(1).is_some_and(|k| k.tag_name().name() != item.kind)
                    });
                    if wrong_kind {
                        out.push_str(&Self::error("UpdateItem", "ErrorObjectTypeChanged"));
                        continue;
                    }
                    for set in find(change, "Updates").unwrap().children().filter(|c| c.is_element()) {
                        let delete = set.tag_name().name() == "DeleteItemField";
                        if let Some(read) = find(set, "IsRead").and_then(|n| n.text()) {
                            item.read = read == "true";
                        }
                        if let Some(uri) = child(set, "FieldURI").and_then(|f| f.attribute("FieldURI"))
                            && uri == "item:Categories"
                        {
                            item.categories = if delete {
                                Vec::new()
                            } else {
                                find(set, "Categories").map(|c| children(c, "String").filter_map(|s| s.text()).map(str::to_owned).collect()).unwrap_or_default()
                            };
                        }
                        if let Some(tag) = child(set, "ExtendedFieldURI").and_then(|f| f.attribute("PropertyTag")) {
                            let value: i64 = find(set, "Value").and_then(|v| v.text()).and_then(|v| v.parse().ok()).unwrap_or(0);
                            match tag {
                                "0x1090" => item.flagged = value == 2,
                                "0x1081" => item.verb = Some(value),
                                _ => {}
                            }
                        }
                    }
                    out.push_str(&Self::ok("UpdateItem", ""));
                }
                out
            }
            "MoveItem" => {
                let target = self.resolve(child(op, "ToFolderId").unwrap().children().find(|c| c.is_element()).unwrap()).unwrap();
                let ids: Vec<String> = children(child(op, "ItemIds").unwrap(), "ItemId").map(|i| i.attribute("Id").unwrap().to_owned()).collect();
                let mut out = String::new();
                for old in ids {
                    let new = self.id("I");
                    match self.items.iter_mut().find(|i| i.id == old) {
                        Some(item) => {
                            // A moved item gets a new id, as Exchange's do.
                            item.id = new.clone();
                            item.folder = target.clone();
                            out.push_str(&Self::ok("MoveItem", &format!(r#"<m:Items><t:Message><t:ItemId Id="{new}"/></t:Message></m:Items>"#)));
                        }
                        None => out.push_str(&Self::error("MoveItem", "ErrorItemNotFound")),
                    }
                }
                out
            }
            "DeleteItem" => children(child(op, "ItemIds").unwrap(), "ItemId")
                .map(|i| {
                    let id = i.attribute("Id").unwrap();
                    let before = self.items.len();
                    self.items.retain(|x| x.id != id);
                    if before == self.items.len() { Self::error("DeleteItem", "ErrorItemNotFound") } else { Self::ok("DeleteItem", "") }
                })
                .collect(),
            "EmptyFolder" => {
                let id = self.resolve(child(op, "FolderIds").unwrap().children().find(|c| c.is_element()).unwrap()).unwrap();
                self.items.retain(|i| i.folder != id);
                Self::ok("EmptyFolder", "")
            }
            "CreateItem" => {
                let disposition = op.attribute("MessageDisposition").unwrap_or("SaveOnly").to_owned();
                let saved = child(op, "SavedItemFolderId").and_then(|s| s.children().find(|c| c.is_element())).and_then(|n| self.resolve(n));
                let items = child(op, "Items").unwrap();
                let mut out = String::new();
                for item in items.children().filter(|c| c.is_element()) {
                    let kind = item.tag_name().name();
                    if matches!(kind, "AcceptItem" | "TentativelyAcceptItem" | "DeclineItem") {
                        let reference = child(item, "ReferenceItemId").unwrap().attribute("Id").unwrap().to_owned();
                        self.answered.push((reference, kind.to_owned()));
                        out.push_str(&Self::ok("CreateItem", ""));
                        continue;
                    }
                    let mime = child(item, "MimeContent").and_then(|m| m.text()).map(|t| unb64(t).unwrap()).unwrap_or_default();
                    let mut flags = 8; // unsent, unless said otherwise
                    let mut deferred = None;
                    for p in children(item, "ExtendedProperty") {
                        let tag = child(p, "ExtendedFieldURI").and_then(|u| u.attribute("PropertyTag")).unwrap_or_default();
                        let value = text(p, "Value");
                        match tag {
                            "0x0E07" => flags = value.parse().unwrap_or(8),
                            "0x3FEF" => deferred = Some(value),
                            _ => {}
                        }
                    }
                    if kind == "Message" && disposition == "SendAndSaveCopy" {
                        self.sent.push((mime.clone(), deferred.clone()));
                    }
                    let subject = mailparse::parse_mail(&mime)
                        .ok()
                        .and_then(|p| { use mailparse::MailHeaderMap as _; p.headers.get_first_value("Subject") })
                        .or_else(|| {
                            let text = String::from_utf8_lossy(&mime);
                            text.lines().find_map(|l| l.strip_prefix("SUMMARY:").map(str::to_owned))
                        })
                        .or_else(|| find(item, "DisplayName").and_then(|n| n.text()).map(str::to_owned))
                        .unwrap_or_default();
                    let folder = saved.clone().unwrap_or_else(|| "f-drafts".into());
                    let uid = |mime: &[u8]| String::from_utf8_lossy(mime).lines().find_map(|l| l.strip_prefix("UID:").map(str::to_owned));
                    if kind == "CalendarItem"
                        && self.unique_uids
                        && let Some(new) = uid(&mime)
                        && self.items.iter().any(|i| i.folder == folder && i.kind == "CalendarItem" && uid(&i.mime).as_ref() == Some(&new))
                    {
                        out.push_str(&Self::error("CreateItem", "ErrorItemSave"));
                        continue;
                    }
                    let id = self.id("I");
                    let (stored_kind, mime) = match kind {
                        "CalendarItem" => ("CalendarItem", mime),
                        "Contact" => {
                            // As a vCard, as Exchange gives contacts back.
                            let email = find(item, "Entry").and_then(|e| e.text()).unwrap_or_default();
                            ("Contact", format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:{subject}\r\nEMAIL:{email}\r\nEND:VCARD\r\n").into_bytes())
                        }
                        _ => ("Message", mime),
                    };
                    self.items.push(FakeItem {
                        id: id.clone(), folder, mime, subject, read: flags & 1 == 1, flagged: false, categories: Vec::new(),
                        verb: None, received: chrono::Utc::now().timestamp(), flags, kind: stored_kind, deferred,
                    });
                    out.push_str(&Self::ok("CreateItem", &format!(r#"<m:Items><t:Message><t:ItemId Id="{id}"/></t:Message></m:Items>"#)));
                }
                out
            }
            "GetUserOofSettingsRequest" => format!(
                r#"<m:ResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode></m:ResponseMessage><t:OofSettings>{}<t:ExternalAudience>All</t:ExternalAudience></t:OofSettings>"#,
                self.oof
            ),
            "SetUserOofSettingsRequest" => {
                let settings = child(op, "UserOofSettings").unwrap();
                let state = text(settings, "OofState");
                let reply = child(settings, "InternalReply").map(|r| text(r, "Message")).unwrap_or_default();
                self.oof = format!("<t:OofState>{state}</t:OofState><t:InternalReply><t:Message>{}</t:Message></t:InternalReply>", esc(&reply));
                r#"<m:ResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode></m:ResponseMessage>"#.into()
            }
            "GetInboxRules" => format!(r#"<m:ResponseCode>NoError</m:ResponseCode><m:InboxRules>{}</m:InboxRules>"#, self.rules.join("")),
            "UpdateInboxRules" => {
                for operation in child(op, "Operations").unwrap().children().filter(|c| c.is_element()) {
                    match operation.tag_name().name() {
                        "CreateRuleOperation" => {
                            let id = self.id("R");
                            let rule = child(operation, "Rule").unwrap();
                            let inner: String = rule.children().filter(|c| c.is_element()).map(|c| serialize(c)).collect();
                            self.rules.push(format!(r#"<t:Rule xmlns:t="{TYPES}"><t:RuleId>{id}</t:RuleId>{inner}</t:Rule>"#));
                        }
                        "SetRuleOperation" => {
                            let rule = child(operation, "Rule").unwrap();
                            let id = text(rule, "RuleId");
                            let inner: String = rule.children().filter(|c| c.is_element()).map(|c| serialize(c)).collect();
                            if let Some(r) = self.rules.iter_mut().find(|r| r.contains(&format!("<t:RuleId>{id}</t:RuleId>"))) {
                                *r = format!(r#"<t:Rule xmlns:t="{TYPES}">{inner}</t:Rule>"#);
                            }
                        }
                        "DeleteRuleOperation" => {
                            let id = text(operation, "RuleId");
                            self.rules.retain(|r| !r.contains(&format!("<t:RuleId>{id}</t:RuleId>")));
                        }
                        _ => {}
                    }
                }
                r#"<m:ResponseCode>NoError</m:ResponseCode>"#.into()
            }
            "ResolveNames" => {
                let wanted = text(op, "UnresolvedEntry").to_lowercase();
                let found: String = self
                    .people
                    .iter()
                    .filter(|(n, e)| n.to_lowercase().contains(&wanted) || e.contains(&wanted))
                    .map(|(n, e)| format!("<t:Resolution><t:Mailbox><t:Name>{}</t:Name><t:EmailAddress>{}</t:EmailAddress></t:Mailbox><t:Contact><t:JobTitle>Engineer</t:JobTitle></t:Contact></t:Resolution>", esc(n), esc(e)))
                    .collect();
                if found.is_empty() {
                    Self::error("ResolveNames", "ErrorNameResolutionNoResults")
                } else {
                    Self::ok("ResolveNames", &format!("<m:ResolutionSet>{found}</m:ResolutionSet>"))
                }
            }
            "GetUserAvailabilityRequest" => {
                let people = find(op, "MailboxDataArray").unwrap().children().filter(|c| c.is_element()).count();
                (0..people)
                    .map(|_| "<m:FreeBusyResponse><m:ResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode></m:ResponseMessage><m:FreeBusyView><t:CalendarEventArray><t:CalendarEvent><t:StartTime>2026-12-01T10:00:00</t:StartTime><t:EndTime>2026-12-01T11:00:00</t:EndTime><t:BusyType>Busy</t:BusyType></t:CalendarEvent></t:CalendarEventArray></m:FreeBusyView></m:FreeBusyResponse>".to_owned())
                    .collect()
            }
            "Subscribe" => {
                assert!(find(op, "StreamingSubscriptionRequest").is_some());
                let id = self.id("S");
                Self::ok("Subscribe", &format!("<m:SubscriptionId>{id}</m:SubscriptionId><m:Watermark>w</m:Watermark>"))
            }
            other => Self::error(other, "ErrorInvalidOperation"),
        }
    }
}

/// An element as XML again (for the fake's kept rules), with `t:` names.
fn serialize(n: Node) -> String {
    if n.is_text() {
        return esc(n.text().unwrap_or_default());
    }
    let attrs: String = n.attributes().map(|a| format!(r#" {}="{}""#, a.name(), esc(a.value()))).collect();
    let inner: String = n.children().filter(|c| c.is_element() || c.is_text()).map(serialize).collect();
    format!("<t:{0}{attrs}>{inner}</t:{0}>", n.tag_name().name())
}

thread_local! {
    static PORT: std::cell::RefCell<u16> = const { std::cell::RefCell::new(0) };
}

/// The fake, serving.
pub(crate) fn serve(fake: Fake) -> (test_server::Server, Arc<Mutex<Fake>>) {
    let fake = Arc::new(Mutex::new(fake));
    let shared = fake.clone();
    let port = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let seen = port.clone();
    let server = test_server::start_raw(move |r| {
        let port = seen.load(std::sync::atomic::Ordering::Relaxed);
        PORT.with(|p| *p.borrow_mut() = port);
        shared.lock().unwrap().respond(r)
    });
    port.store(server.port, std::sync::atomic::Ordering::Relaxed);
    (server, fake)
}

fn tls() -> native_tls::TlsConnector {
    native_tls::TlsConnector::new().unwrap()
}
fn session(server: &test_server::Server, password: &str) -> Session {
    let auth = Auth::Ntlm(ntlm::Identity::new("CORP\\qa", password));
    Session::open(&format!("{}/EWS/Exchange.asmx", server.base), "qa@corp.example", auth, tls()).unwrap()
}

#[test]
fn signs_in_with_ntlm_and_reads_folders_and_mail() -> Result<()> {
    let mut fake = Fake::new("secret");
    let work = fake.folder("Work", "f-inbox");
    let first = fake.add("Budget", "f-inbox", 5);
    fake.add("Agenda", "f-inbox", 10);
    fake.add("Plan", &work, 20);
    let (server, fake) = serve(fake);
    // A wrong password is refused, as a sign-in that needs attention.
    let wrong = session(&server, "nope").reload_folders().unwrap_err();
    assert!(wrong.downcast_ref::<AccessFailure>().is_some(), "{wrong:#}");
    let s = session(&server, "secret");
    let folders = s.reload_folders()?;
    let inbox = folders.iter().find(|f| f.name == "Inbox").unwrap();
    assert_eq!(inbox.distinguished.as_deref(), Some("inbox"));
    let nested = folders.iter().find(|f| f.name == "Work").unwrap();
    assert_eq!((nested.path.as_str(), nested.depth), ("Inbox/Work", 1));
    assert_eq!(folders.iter().find(|f| f.name == "Archive").unwrap().distinguished.as_deref(), Some("archive"));
    let (page, last) = s.list("inbox", 0, 1, None, None)?;
    assert!(!last);
    assert_eq!((page[0].id.as_str(), page[0].subject.as_str(), page[0].from_name.as_str()), (first.as_str(), "Budget", "Bo Berg"));
    s.set_read(std::slice::from_ref(&first), true)?;
    s.set_flagged(std::slice::from_ref(&first), true)?;
    s.set_categories(&[(first.clone(), vec!["Finance".into()])])?;
    let now = &s.messages(std::slice::from_ref(&first))?[0];
    assert!(now.read && now.flagged);
    assert_eq!(now.categories, ["Finance"]);
    // Moved: a new id.
    let moved = s.move_to(std::slice::from_ref(&first), &work)?;
    assert_ne!(moved[0], first);
    assert!(s.messages(std::slice::from_ref(&first))?.is_empty());
    assert!(String::from_utf8_lossy(&s.mime(&moved[0])?).contains("Subject: Budget"));
    // Sent as MIME, held where asked.
    s.send(b"To: a@example.org\r\nSubject: Later\r\n\r\nHi\r\n", Some(1_800_000_000))?;
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.sent[0].1.as_deref(), Some("2027-01-15T08:00:00Z"));
        assert!(f.signed_in > 0 && f.refused > 0);
    }
    // Imported as delivered mail, not a draft.
    let id = s.import(b"Subject: Old\r\n\r\nx\r\n", "inbox", true)?;
    assert_eq!(fake.lock().unwrap().item(&id).unwrap().flags, 1);
    Ok(())
}

#[test]
fn autodiscover_finds_the_ews_address() -> Result<()> {
    let xml = r#"<Autodiscover xmlns="a"><Response xmlns="b"><Account><Protocol><Type>EXPR</Type><EwsUrl>https://outside.example/EWS/Exchange.asmx</EwsUrl></Protocol><Protocol><Type>EXCH</Type><EwsUrl>https://mail.corp.example/EWS/Exchange.asmx</EwsUrl></Protocol></Account></Response></Autodiscover>"#;
    assert_eq!(ews_url_in(xml).as_deref(), Some("https://mail.corp.example/EWS/Exchange.asmx"));
    assert_eq!(ews_url_in("<x/>"), None);
    Ok(())
}

#[test]
fn rules_go_both_ways() {
    let rule = serde_json::json!({
        "displayName": "Picacho 1", "sequence": 1, "isEnabled": true,
        "conditions": {"senderContains": ["shop"]},
        "exceptions": {},
        "actions": {"moveToFolder": "f-r", "stopProcessingRules": true},
    });
    let xml = rule_xml(&rule, Some("r1"));
    assert!(xml.contains("<t:ContainsSenderStrings><t:String>shop</t:String></t:ContainsSenderStrings>"), "{xml}");
    assert!(xml.find("<t:MoveToFolder").unwrap() < xml.find("<t:StopProcessingRules").unwrap(), "in the schema's order");
    let wrapped = format!(r#"<r xmlns:t="{TYPES}">{xml}</r>"#);
    let doc = roxmltree::Document::parse(&wrapped).unwrap();
    let back = rule_json(find(doc.root(), "Rule").unwrap());
    assert_eq!(back["conditions"], rule["conditions"]);
    assert_eq!(back["actions"]["moveToFolder"], "f-r");
    assert_eq!(back["actions"]["stopProcessingRules"], true);
    // Several tests and actions, attachments among them, in the schema's order.
    let rule = serde_json::json!({
        "displayName": "Picacho 2", "sequence": 2, "isEnabled": true,
        "conditions": {"senderContains": ["shop"], "hasAttachments": true, "withinSizeRange": {"minimumSize": 10, "maximumSize": 900}},
        "exceptions": {"bodyContains": ["draft"]},
        "actions": {"markAsRead": true, "assignCategories": ["Bills"]},
    });
    let xml = rule_xml(&rule, None);
    assert!(xml.find("<t:ContainsSenderStrings").unwrap() < xml.find("<t:HasAttachments>true</t:HasAttachments>").unwrap(), "{xml}");
    assert!(xml.find("<t:HasAttachments").unwrap() < xml.find("<t:WithinSizeRange").unwrap(), "{xml}");
    let wrapped = format!(r#"<r xmlns:t="{TYPES}">{xml}</r>"#);
    let doc = roxmltree::Document::parse(&wrapped).unwrap();
    let back = rule_json(find(doc.root(), "Rule").unwrap());
    assert_eq!(back["conditions"], rule["conditions"]);
    assert_eq!(back["exceptions"], rule["exceptions"]);
    assert_eq!(back["actions"]["assignCategories"], rule["actions"]["assignCategories"]);
}

#[test]
fn changes_name_each_items_own_kind() -> Result<()> {
    let mut fake = Fake::new("secret");
    let invite = fake.add("Planning", "f-inbox", 5);
    let event = fake.add("Standup", "f-inbox", 6);
    for i in fake.items.iter_mut() {
        i.kind = if i.id == invite { "MeetingRequest" } else { "CalendarItem" };
    }
    let (server, fake) = serve(fake);
    // Not listed first: the kinds are asked for.
    let s = session(&server, "secret");
    s.set_flagged(std::slice::from_ref(&invite), true)?;
    s.set_read(&[invite.clone(), event.clone()], true)?;
    s.set_categories(&[(event.clone(), vec!["Team".into()])])?;
    let f = fake.lock().unwrap();
    let (i, e) = (f.item(&invite).unwrap(), f.item(&event).unwrap());
    assert!(i.flagged && i.read);
    // A calendar item can't be read or not; it was left alone.
    assert!(!e.read);
    assert_eq!(e.categories, ["Team"]);
    // Each item's kind asked once, when first changed.
    assert_eq!(f.calls.iter().filter(|c| *c == "GetItem").count(), 2, "{:?}", f.calls);
    Ok(())
}

#[test]
fn mailbox_size_adds_up_the_folders() -> Result<()> {
    let mut fake = Fake::new("secret");
    let work = fake.folder("Work", "f-inbox");
    fake.add("A", "f-inbox", 1);
    fake.add("B", &work, 2);
    let total: usize = fake.items.iter().map(|i| i.mime.len()).sum();
    let (server, _fake) = serve(fake);
    assert_eq!(session(&server, "secret").mailbox_size()?, Some(total as u64));
    Ok(())
}

#[test]
fn streams_inbox_changes_and_subscribes_again() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.stream = vec!["event", "forget", "event"];
    let (server, fake) = serve(fake);
    let s = session(&server, "secret");
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut heard = 0;
    s.stream_inbox(&stop, 1, false, || {
        heard += 1;
        if heard == 3 {
            stop.store(true, std::sync::atomic::Ordering::Release);
        }
    })?;
    // The event, the forgotten subscription (events may be lost), the event.
    assert_eq!(heard, 3);
    let f = fake.lock().unwrap();
    assert_eq!(f.calls.iter().filter(|c| *c == "Subscribe").count(), 2, "{:?}", f.calls);
    assert_eq!(f.calls.iter().filter(|c| *c == "GetStreamingEvents").count(), 3, "{:?}", f.calls);
    Ok(())
}

#[test]
fn streaming_answers_are_read_one_envelope_at_a_time() -> Result<()> {
    let a = br#"<s:Envelope xmlns:s="x"><s:Body/></s:Envelope><soap11:Envelope xmlns:soap11="x">"#;
    assert_eq!(envelope_end(a), Some(46));
    assert_eq!(envelope_end(br#"<Envelope><Body>Envelope></Body>"#), None);
    assert_eq!(envelope_end(b"<Envelope></Envelope>"), Some(21));
    let wrap = |inner: &str| format!(r#"<s:Envelope xmlns:s="{SOAP}"><s:Body><m:GetStreamingEventsResponse xmlns:m="{MESSAGES}" xmlns:t="{TYPES}"><m:ResponseMessages>{inner}</m:ResponseMessages></m:GetStreamingEventsResponse></s:Body></s:Envelope>"#);
    let status = wrap(r#"<m:GetStreamingEventsResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Notifications><m:Notification><t:SubscriptionId>s</t:SubscriptionId><t:StatusEvent><t:Watermark>w</t:Watermark></t:StatusEvent></m:Notification></m:Notifications></m:GetStreamingEventsResponseMessage>"#);
    assert_eq!(stream_answer(status)?, Streamed::Quiet);
    let moved = wrap(r#"<m:GetStreamingEventsResponseMessage ResponseClass="Success"><m:ResponseCode>NoError</m:ResponseCode><m:Notifications><m:Notification><t:SubscriptionId>s</t:SubscriptionId><t:MovedEvent/></m:Notification></m:Notifications></m:GetStreamingEventsResponseMessage>"#);
    assert_eq!(stream_answer(moved)?, Streamed::Events);
    let expired = wrap(r#"<m:GetStreamingEventsResponseMessage ResponseClass="Error"><m:ResponseCode>ErrorExpiredSubscription</m:ResponseCode></m:GetStreamingEventsResponseMessage>"#);
    assert_eq!(stream_answer(expired)?, Streamed::Forgotten);
    let denied = wrap(r#"<m:GetStreamingEventsResponseMessage ResponseClass="Error"><m:ResponseCode>ErrorAccessDenied</m:ResponseCode></m:GetStreamingEventsResponseMessage>"#);
    assert!(stream_answer(denied).is_err());
    Ok(())
}

#[test]
fn autodiscover_follows_redirects_and_asks_soap_where_pox_is_off() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.redirect_addr = Some(("ana@old.example".into(), "qa@corp.old.example".into()));
    let (server, fake) = serve(fake);
    let base = server.base.clone();
    let well_known = move |domain: &str| match domain {
        "old.example" => vec![format!("{base}/autodiscover/autodiscover.xml")],
        _ => vec![format!("{base}/nopox/autodiscover/autodiscover.xml")],
    };
    let auth = Auth::Ntlm(ntlm::Identity::new("CORP\\qa", "secret"));
    let found = autodiscover_with("ana@old.example", &auth, tls(), &Places { well_known: &well_known, fallbacks: &|_, _| Vec::new() })?;
    assert_eq!(found, Some(format!("http://127.0.0.1:{}/EWS/Exchange.asmx", server.port)));
    let seen: Vec<String> = server.seen.lock().unwrap().iter().filter(|r| !r.body.is_empty()).map(|r| r.path.clone()).collect();
    assert_eq!(seen, ["/autodiscover/autodiscover.xml", "/nopox/autodiscover/autodiscover.xml", "/nopox/autodiscover/autodiscover.svc"]);
    // A wrong password is said, not passed over.
    let wrong = Auth::Ntlm(ntlm::Identity::new("CORP\\qa", "nope"));
    let refused = autodiscover_with("ana@old.example", &wrong, tls(), &Places { well_known: &well_known, fallbacks: &|_, _| Vec::new() }).unwrap_err();
    assert!(refused.downcast_ref::<AccessFailure>().is_some());
    drop(fake);
    // Answers send the question, and the password, on only within the
    // address's domain or to Microsoft's cloud.
    assert!(autodiscover_vouched("corp.old.example", "old.example"));
    assert!(autodiscover_vouched("mail.contoso.com", "sales.contoso.com"));
    assert!(autodiscover_vouched("contoso.mail.onmicrosoft.com", "contoso.com"));
    assert!(autodiscover_vouched("autodiscover-s.outlook.com", "contoso.com"));
    assert!(!autodiscover_vouched("attacker.example", "contoso.com"));
    assert!(!autodiscover_vouched("contoso.com.evil.net", "contoso.com"));
    assert!(!autodiscover_vouched("other.co.uk", "contoso.co.uk"));
    assert!(!autodiscover_vouched("10.0.0.5", "contoso.com"));
    // Unauthenticated redirects stay within the domain, over https.
    assert_eq!(redirect_target("https://mail.corp.example/autodiscover/autodiscover.xml", "corp.example").as_deref(), Some("https://mail.corp.example/autodiscover/autodiscover.xml"));
    assert_eq!(redirect_target("http://mail.corp.example/autodiscover/autodiscover.xml", "corp.example"), None);
    assert_eq!(redirect_target("https://corp.example.evil.net/autodiscover/autodiscover.xml", "corp.example"), None);
    // The answers' redirects.
    let pox = r#"<Autodiscover xmlns="a"><Response xmlns="b"><Account><Action>redirectUrl</Action><RedirectUrl>https://ad.corp.example/autodiscover/autodiscover.xml</RedirectUrl></Account></Response></Autodiscover>"#;
    assert_eq!(pox_answer(pox), Found::Server("https://ad.corp.example/autodiscover/autodiscover.xml".into()));
    let soap = r#"<s:Envelope xmlns:s="x"><s:Body><GetUserSettingsResponseMessage xmlns="a"><Response><UserResponses><UserResponse><ErrorCode>RedirectAddress</ErrorCode><RedirectTarget>bo@corp.example</RedirectTarget></UserResponse></UserResponses></Response></GetUserSettingsResponseMessage></s:Body></s:Envelope>"#;
    assert_eq!(soap_answer(soap), Found::Address("bo@corp.example".into()));
    Ok(())
}

/// An answer sending the question to an address in another domain isn't
/// followed: the password isn't sent to that domain's servers.
#[test]
fn autodiscover_keeps_the_password_in_the_domain() -> Result<()> {
    let mut fake = Fake::new("secret");
    fake.redirect_addr = Some(("ana@old.example".into(), "ana@attacker.example".into()));
    let (server, fake) = serve(fake);
    let base = server.base.clone();
    let well_known = move |domain: &str| match domain {
        "old.example" => vec![format!("{base}/autodiscover/autodiscover.xml")],
        _ => vec![format!("{base}/elsewhere/autodiscover/autodiscover.xml")],
    };
    let auth = Auth::Ntlm(ntlm::Identity::new("CORP\\qa", "secret"));
    let found = autodiscover_with("ana@old.example", &auth, tls(), &Places { well_known: &well_known, fallbacks: &|_, _| Vec::new() })?;
    assert_eq!(found, None);
    let elsewhere = server.seen.lock().unwrap().iter().any(|r| r.path.starts_with("/elsewhere/"));
    assert!(!elsewhere, "the password went to another domain's server");
    drop(fake);
    Ok(())
}

/// A certificate's DER with only what the hash choice reads: an empty
/// to-be-signed part and the signature algorithm.
fn certificate_signed_with(oid: &[u8], params: &[u8]) -> Vec<u8> {
    let tlv = |tag: u8, content: &[u8]| [vec![tag, content.len() as u8], content.to_vec()].concat();
    let algorithm = tlv(0x30, &[tlv(0x06, oid), params.to_vec()].concat());
    tlv(0x30, &[tlv(0x30, &[]), algorithm, tlv(0x03, &[0, 1, 2])].concat())
}

#[test]
fn channel_binding_hashes_as_the_certificate_signs() {
    use sha2::Digest as _;
    let sha1_rsa = certificate_signed_with(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05], &[0x05, 0x00]);
    assert_eq!(signature_hash(&sha1_rsa), 256);
    assert_eq!(tls_server_end_point(&sha1_rsa), sha2::Sha256::digest(&sha1_rsa).to_vec());
    let sha384_ecdsa = certificate_signed_with(&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03], &[]);
    assert_eq!(tls_server_end_point(&sha384_ecdsa), sha2::Sha384::digest(&sha384_ecdsa).to_vec());
    let sha512_rsa = certificate_signed_with(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d], &[0x05, 0x00]);
    assert_eq!(signature_hash(&sha512_rsa), 512);
    let pss_sha384 = certificate_signed_with(
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a],
        &[0x30, 0x0d, 0xa0, 0x0b, 0x30, 0x09, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02],
    );
    assert_eq!(signature_hash(&pss_sha384), 384);
    assert_eq!(signature_hash(b"not a certificate"), 256);
}

#[test]
fn signs_in_with_an_organizations_token() -> Result<()> {
    let (server, _fake) = serve(Fake::new("secret"));
    let url = format!("{}/EWS/Exchange.asmx", server.base);
    let s = Session::open(&url, "qa@corp.example", Auth::Bearer("entra-token".into()), tls())?;
    assert!(s.reload_folders()?.iter().any(|f| f.name == "Inbox"));
    let wrong = Session::open(&url, "qa@corp.example", Auth::Bearer("old".into()), tls())?.reload_folders().unwrap_err();
    assert!(wrong.downcast_ref::<AccessFailure>().is_some());
    Ok(())
}

#[test]
fn basic_sign_in_is_never_a_silent_fallback() -> Result<()> {
    // A server that offers only Basic, counting the requests that carry it.
    let basic = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = basic.clone();
    let server = test_server::start_raw(move |r: &Request| -> FullAnswer {
        if r.authorization.starts_with("Basic ") {
            seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        (401, "text/plain".into(), Vec::new(), vec![("WWW-Authenticate".into(), "Basic realm=\"corp\"".into())])
    });
    let url = format!("{}/EWS/Exchange.asmx", server.base);
    // NTLM is refused there, saying the server takes only Basic; the
    // password isn't sent.
    let ntlm = session(&server, "secret").reload_folders().unwrap_err();
    assert!(ntlm.downcast_ref::<BasicOnly>().is_some(), "{ntlm:#}");
    // Chosen Basic goes over HTTPS only: this server is plain HTTP.
    let chosen = Auth::Basic { user: "CORP\\qa".into(), password: "secret".into() };
    let plain = Session::open(&url, "qa@corp.example", chosen.clone(), tls())?.reload_folders().unwrap_err();
    assert!(plain.downcast_ref::<BasicOnly>().is_none(), "{plain:#}");
    // Autodiscover never sends Basic, whatever the account chose.
    let binding = Binding::default();
    assert!(post(&reqwest::blocking::Client::new(), &url, &chosen, "text/xml", b"", &binding, Reach::Autodiscover).is_err());
    assert_eq!(basic.load(std::sync::atomic::Ordering::Relaxed), 0);
    Ok(())
}
