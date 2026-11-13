//! Mail subscriptions: senders whose mail carries a List-Unsubscribe
//! header, how many of their messages a mailbox holds, and asking them to
//! stop (RFC 2369, with RFC 8058 one-click where offered).
use crate::manage::model::{Draft, Mail};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// The preference holding the subscriptions already unsubscribed from.
pub const UNSUBSCRIBED_PREFERENCE: &str = "unsubscribed";

/// One sender's subscription in one mailbox.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub account: String,
    /// The sender's address, lowercase.
    pub address: String,
    /// The sender's name as its latest message gives it.
    pub name: String,
    /// Messages detected from it, each counted once across folders.
    pub count: usize,
    /// Its latest message's List-Unsubscribe header and one-click support.
    pub unsubscribe: String,
    pub one_click: bool,
    pub latest: i64,
}
impl Subscription {
    /// What identifies it among every mailbox's subscriptions.
    pub fn key(&self) -> String {
        key(&self.account, &self.address)
    }
    /// The subscription a message belongs to, if it is from one.
    pub fn of(m: &Mail) -> Option<Self> {
        if m.list_unsubscribe.trim().is_empty() || method(&m.list_unsubscribe, m.one_click).is_none() {
            return None;
        }
        Some(Self {
            account: m.account.clone(),
            address: m.from.trim().to_lowercase(),
            name: m.sender.clone(),
            count: 1,
            unsubscribe: m.list_unsubscribe.clone(),
            one_click: m.one_click,
            latest: m.date,
        })
    }
}
pub fn key(account: &str, address: &str) -> String {
    format!("{account}\u{1f}{}", address.trim().to_lowercase())
}

/// Groups messages into subscriptions, most messages first. Copies of a
/// message in several folders (or Gmail labels) count once.
pub fn group(mails: impl IntoIterator<Item = Mail>) -> Vec<Subscription> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut by: HashMap<String, Subscription> = HashMap::new();
    for m in mails {
        let Some(sub) = Subscription::of(&m) else {
            continue;
        };
        let identity = if m.message_id.is_empty() {
            m.id.clone()
        } else {
            m.message_id.clone()
        };
        if !seen.insert((m.account.clone(), identity)) {
            continue;
        }
        match by.get_mut(&sub.key()) {
            Some(existing) => {
                existing.count += 1;
                if sub.latest > existing.latest {
                    existing.latest = sub.latest;
                    existing.name = sub.name;
                    existing.unsubscribe = sub.unsubscribe;
                    existing.one_click = sub.one_click;
                }
            }
            None => {
                by.insert(sub.key(), sub);
            }
        }
    }
    let mut list: Vec<Subscription> = by.into_values().collect();
    list.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| b.latest.cmp(&a.latest))
            .then_with(|| a.address.cmp(&b.address))
    });
    list
}

/// How to ask a sender to stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    /// POST `List-Unsubscribe=One-Click` to the address; done at once.
    OneClick(String),
    /// Send a message (address, subject, body).
    Mail { to: String, subject: String, body: String },
    /// A web page the sender wants opened.
    Web(String),
}

/// The addresses in a List-Unsubscribe header, in its order.
pub fn targets(header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = header;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let uri: String = rest[start + 1..start + end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if !uri.is_empty() {
            out.push(uri);
        }
        rest = &rest[start + end + 1..];
    }
    out
}

/// The best way to unsubscribe: one click when offered, else by mail
/// (nothing to open), else the sender's page.
pub fn method(header: &str, one_click: bool) -> Option<Method> {
    let all = targets(header);
    let https = all
        .iter()
        .find(|u| u.to_ascii_lowercase().starts_with("https://"))
        .cloned();
    if one_click && let Some(url) = &https {
        return Some(Method::OneClick(url.clone()));
    }
    if let Some(mailto) = all.iter().find(|u| u.to_ascii_lowercase().starts_with("mailto:")) {
        let rest = &mailto["mailto:".len()..];
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut subject = String::new();
        let mut body = String::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k.to_ascii_lowercase().as_str() {
                "subject" => subject = decode(v),
                "body" => body = decode(v),
                _ => {}
            }
        }
        let to = decode(to);
        if to.contains('@') {
            return Some(Method::Mail {
                to,
                subject: if subject.is_empty() { "unsubscribe".into() } else { subject },
                body: if body.is_empty() { "unsubscribe".into() } else { body },
            });
        }
    }
    https
        .or_else(|| {
            all.into_iter()
                .find(|u| u.to_ascii_lowercase().starts_with("http://"))
        })
        .map(Method::Web)
}

/// Percent-decoding for mailto: parts.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What asking to unsubscribe did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The sender was asked (one click, or by mail).
    Done,
    /// The sender's page has to be opened to finish.
    Open(String),
}

/// Asks the sender of `sub` to stop. Mail goes out through `send`.
pub fn unsubscribe(
    sub: &Subscription,
    send: impl FnOnce(&Draft) -> Result<()>,
) -> Result<Outcome> {
    match method(&sub.unsubscribe, sub.one_click).context("This sender offers no way to unsubscribe.")? {
        Method::OneClick(url) => {
            let response = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?
                .post(&url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("List-Unsubscribe=One-Click")
                .send()?;
            if !response.status().is_success() {
                bail!("The sender refused the request ({}).", response.status());
            }
            Ok(Outcome::Done)
        }
        Method::Mail { to, subject, body } => {
            let draft = Draft {
                id: uuid::Uuid::new_v4().to_string(),
                account: sub.account.clone(),
                to,
                subject,
                body,
                ..Draft::default()
            };
            send(&draft)?;
            Ok(Outcome::Done)
        }
        Method::Web(url) => Ok(Outcome::Open(url)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_header() {
        let h = "<mailto:leave-123@lists.example.com?subject=Unsubscribe%20me>, <https://example.com/u/123>";
        assert_eq!(
            targets(h),
            ["mailto:leave-123@lists.example.com?subject=Unsubscribe%20me", "https://example.com/u/123"]
        );
        assert_eq!(method(h, true), Some(Method::OneClick("https://example.com/u/123".into())));
        assert_eq!(
            method(h, false),
            Some(Method::Mail {
                to: "leave-123@lists.example.com".into(),
                subject: "Unsubscribe me".into(),
                body: "unsubscribe".into(),
            })
        );
        assert_eq!(
            method("<https://example.com/u>", false),
            Some(Method::Web("https://example.com/u".into()))
        );
        assert_eq!(method("nonsense", true), None);
    }

    #[test]
    fn groups_by_sender_and_counts_copies_once() {
        let mail = |id: &str, from: &str, mid: &str, date: i64| Mail {
            id: id.into(),
            account: "a".into(),
            from: from.into(),
            sender: from.into(),
            message_id: mid.into(),
            date,
            list_unsubscribe: "<https://example.com/u>".into(),
            ..Mail::default()
        };
        let list = group([
            mail("1", "News@Example.com", "<1>", 1),
            mail("2", "news@example.com", "<2>", 3),
            // The same message under another label.
            mail("3", "news@example.com", "<2>", 3),
            mail("4", "deals@shop.com", "<4>", 2),
            Mail { list_unsubscribe: String::new(), ..mail("5", "friend@x.com", "<5>", 5) },
        ]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].address, "news@example.com");
        assert_eq!(list[0].count, 2);
        assert_eq!(list[1].count, 1);
    }

    #[test]
    fn detected_when_downloaded_and_backfilled_for_older_mail() -> Result<()> {
        use crate::manage::model::{Account, Folder, Provider};
        use crate::manage::store::Store;
        let path = std::env::temp_dir().join(format!("picacho-subs-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        let raw = b"From: News <news@example.com>\r\nList-Unsubscribe: <https://example.com/u/1>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nMessage-ID: <1@example.com>\r\nSubject: Hi\r\n\r\nBody";
        let id = format!("{}:INBOX:1:1", a.id);
        let m = crate::sync::mime::parse_message(raw, &a, &id, Folder::Inbox, "INBOX", 1)?;
        assert_eq!(m.list_unsubscribe, "<https://example.com/u/1>");
        assert!(m.one_click);
        // As cached before subscriptions were detected.
        let old = Mail { list_unsubscribe: String::new(), one_click: false, ..m };
        store.save_download(&old, raw)?;
        assert!(store.subscriptions(std::slice::from_ref(&a.id))?.is_empty());
        assert_eq!(store.backfill_subscriptions()?, 1);
        let list = store.subscriptions(std::slice::from_ref(&a.id))?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].address, "news@example.com");
        assert!(list[0].one_click);
        // Nothing is left to do the second time.
        assert_eq!(store.backfill_subscriptions()?, 0);
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }

    #[test]
    fn cached_previews_are_cleaned_from_their_source() -> Result<()> {
        use crate::manage::model::{Account, Folder, Provider};
        use crate::manage::store::Store;
        let path = std::env::temp_dir().join(format!("picacho-previews-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&path)?;
        let a = Account::new(Provider::Imap);
        let raw = b"Content-Type: text/html\r\nSubject: Hi\r\n\r\n<head><!--[if mso]><xml><o:PixelsPerInch>96</o:PixelsPerInch></xml><![endif]--></head><p>Hello&nbsp;&zwnj;there</p>";
        let id = format!("{}:INBOX:1:1", a.id);
        let m = crate::sync::mime::parse_message(raw, &a, &id, Folder::Inbox, "INBOX", 1)?;
        assert_eq!(m.snippet, "Hello there");
        // As cached by the old reading of HTML.
        let old = Mail { body: Some("96\nHello\u{a0}&zwnj;there".into()), snippet: "96 Hello &zwnj;there".into(), ..m };
        store.save_download(&old, raw)?;
        assert_eq!(store.refresh_previews()?, 1);
        let fixed = store.message(&id)?.unwrap();
        assert_eq!(fixed.snippet, "Hello there");
        assert_eq!(fixed.body.as_deref(), Some("Hello there"));
        assert_eq!(store.refresh_previews()?, 0);
        // Downloaded in the background: only a preview is kept, no text.
        let raw = b"Content-Type: text/html\r\nSubject: Sale\r\n\r\n<div style='display:none'>Big sale&#847; &#847; &zwnj;&nbsp;&#847</div><p>Shop now</p>";
        let id = format!("{}:INBOX:1:2", a.id);
        let m = crate::sync::mime::parse_message(raw, &a, &id, Folder::Inbox, "INBOX", 2)?;
        assert_eq!(m.snippet, "Big sale Shop now");
        let old = Mail { body: None, html: None, snippet: "Big sale&#847; &#847; &zwnj;&nbsp;&#847 Shop now".into(), ..m };
        store.save_download(&old, raw)?;
        assert_eq!(store.refresh_previews()?, 1);
        let fixed = store.message(&id)?.unwrap();
        assert_eq!(fixed.snippet, "Big sale Shop now");
        assert!(fixed.body.is_none(), "text stays out of the summary");
        // Without a source, the cached preview itself is cleaned.
        let id = format!("{}:INBOX:1:3", a.id);
        store.save_messages(&[Mail { id: id.clone(), account: a.id.clone(), snippet: "Hi&#847; &zwnj;there".into(), ..Mail::default() }])?;
        assert_eq!(store.refresh_previews()?, 1);
        assert_eq!(store.message(&id)?.unwrap().snippet, "Hi there");
        let _ = std::fs::remove_dir_all(path);
        Ok(())
    }
}
