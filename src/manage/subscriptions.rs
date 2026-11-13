//! Mail subscriptions: senders whose mail carries a List-Unsubscribe
//! header, how many of their messages a mailbox holds, and asking them to
//! stop (RFC 2369, with RFC 8058 one-click where offered).
//!
//! The header is only trusted as far as its sender's DKIM signature vouches
//! for it: anyone can send mail "from" a newsletter's address. One click is
//! used only when the signature covers it, and anything else (a message
//! sent from the person's account, a page to open) is shown to them first.
use crate::manage::model::{Draft, Folder, Mail};
use crate::sync::auth::SignedUnsubscribe;
use anyhow::{Result, bail};
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
    /// Mail in Junk or Trash is not offered: unsubscribing from spam tells
    /// its sender the address is read.
    pub fn of(m: &Mail) -> Option<Self> {
        if matches!(m.folder, Folder::Junk | Folder::Trash)
            || m.list_unsubscribe.trim().is_empty()
            || method(&m.list_unsubscribe, m.one_click).is_none()
        {
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
    /// Send a message to one address with this subject; the body is
    /// always just "unsubscribe".
    Mail { to: String, subject: String },
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
    // Of a mailto: only one recipient, and its subject, are used: other
    // recipients (in the address or `to=`, `cc=`, `bcc=`) and the body
    // would let a sender have the person's account mail anyone anything.
    if let Some(mailto) = all.iter().find(|u| u.to_ascii_lowercase().starts_with("mailto:")) {
        let rest = &mailto["mailto:".len()..];
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut subject = String::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k.eq_ignore_ascii_case("subject") {
                subject = decode(v);
            }
        }
        let subject: String = subject
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect::<String>()
            .trim()
            .to_owned();
        let to = decode(to).trim().to_owned();
        if single_address(&to) {
            return Some(Method::Mail {
                to,
                subject: if subject.is_empty() { "unsubscribe".into() } else { subject },
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

/// Whether `to` is exactly one plain address (`local@domain`).
fn single_address(to: &str) -> bool {
    let Some((local, domain)) = to.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !to.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')' | ':' | '\\' | '[' | ']')
        })
        && !domain.contains('@')
}

/// What unsubscribing will do, worked out before anything is done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub method: Method,
    /// Whether the header it came from was signed by the sender (DKIM, for
    /// the From address's domain).
    pub verified: bool,
}
impl Plan {
    /// From the newest message whose header its sender signed; failing
    /// that, from the `latest` header, which anyone could have written, and
    /// then never by one click.
    pub fn new(signed: Option<&SignedUnsubscribe>, latest: &str) -> Option<Self> {
        match signed {
            Some(s) => method(&s.header, s.one_click).map(|method| Self { method, verified: true }),
            None => method(latest, false).map(|method| Self { method, verified: false }),
        }
    }
    /// Whether the person is asked first: everything but a one-click
    /// request, which is only planned when the sender signed it.
    pub fn needs_confirmation(&self) -> bool {
        !matches!(self.method, Method::OneClick(_))
    }
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

/// The redirects a one-click request follows: a few, and only to HTTPS,
/// so the request is never sent on in the clear.
fn https_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if attempt.url().scheme() != "https" {
            attempt.error("redirected to an address that isn't HTTPS")
        } else {
            attempt.follow()
        }
    })
}

/// Carries out `plan` for `account`. Mail goes out through `send`.
pub fn unsubscribe(
    plan: &Plan,
    account: &str,
    send: impl FnOnce(&Draft) -> Result<()>,
) -> Result<Outcome> {
    match &plan.method {
        Method::OneClick(url) => {
            if !plan.verified {
                bail!("The sender's request to unsubscribe couldn't be verified.");
            }
            let response = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(https_redirects())
                .build()?
                .post(url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("List-Unsubscribe=One-Click")
                .send()?;
            if !response.status().is_success() {
                bail!("The sender refused the request ({}).", response.status());
            }
            Ok(Outcome::Done)
        }
        Method::Mail { to, subject } => {
            let draft = Draft {
                id: uuid::Uuid::new_v4().to_string(),
                account: account.to_owned(),
                to: to.clone(),
                subject: subject.clone(),
                body: "unsubscribe".into(),
                ..Draft::default()
            };
            send(&draft)?;
            Ok(Outcome::Done)
        }
        Method::Web(url) => Ok(Outcome::Open(url.clone())),
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
            })
        );
        assert_eq!(
            method("<https://example.com/u>", false),
            Some(Method::Web("https://example.com/u".into()))
        );
        assert_eq!(method("nonsense", true), None);
    }

    #[test]
    fn mail_goes_to_one_address_with_a_fixed_body() {
        // Two recipients in one address: not mailed, the page is offered.
        let h = "<mailto:a@evil.example,b@victim.example?subject=SUBSCRIBE&body=confirm%20yes>, <https://example.com/u>";
        assert_eq!(method(h, false), Some(Method::Web("https://example.com/u".into())));
        for to in [
            "a@evil.example%2Cb@victim.example",
            "a@evil.example;b@victim.example",
            "Name <a@evil.example>",
            "a@b@evil.example",
            "a@localhost",
            "",
        ] {
            assert_eq!(method(&format!("<mailto:{to}>"), false), None, "{to}");
        }
        // Extra recipients and the body in the query are ignored.
        let Some(Method::Mail { to, subject }) = method(
            "<mailto:leave@lists.example.com?cc=b@victim.example&bcc=c@victim.example&subject=Stop%0D%0ABcc:%20d@x.example&body=confirm>",
            false,
        ) else {
            panic!("a mailto");
        };
        assert_eq!(to, "leave@lists.example.com");
        assert_eq!(subject, "StopBcc: d@x.example");
        let mut sent = None;
        let plan = Plan::new(None, "<mailto:leave@lists.example.com?body=SUBSCRIBE>").unwrap();
        unsubscribe(&plan, "acct", |d| {
            sent = Some(d.clone());
            Ok(())
        })
        .unwrap();
        let sent = sent.unwrap();
        assert_eq!((sent.to.as_str(), sent.body.as_str()), ("leave@lists.example.com", "unsubscribe"));
    }

    #[test]
    fn one_click_only_for_a_signed_header() {
        let h = "<https://example.com/u/1>, <mailto:leave@example.com>";
        // Unsigned (or forged): never one click, and asked first.
        let plan = Plan::new(None, h).unwrap();
        assert!(!plan.verified);
        assert!(matches!(plan.method, Method::Mail { .. }));
        assert!(plan.needs_confirmation());
        // Signed, with one-click signed too.
        let signed = SignedUnsubscribe { header: h.into(), one_click: true };
        let plan = Plan::new(Some(&signed), "<https://forged.example/u>").unwrap();
        assert_eq!(plan, Plan { method: Method::OneClick("https://example.com/u/1".into()), verified: true });
        assert!(!plan.needs_confirmation());
        // A one-click plan that wasn't verified is refused outright.
        let forged = Plan { method: Method::OneClick("https://forged.example/u".into()), verified: false };
        assert!(unsubscribe(&forged, "acct", |_| Ok(())).is_err());
    }

    #[test]
    fn junk_and_trash_are_not_offered() {
        let m = |folder| Mail {
            account: "a".into(),
            from: "news@example.com".into(),
            folder,
            list_unsubscribe: "<https://example.com/u>".into(),
            ..Mail::default()
        };
        assert!(Subscription::of(&m(Folder::Inbox)).is_some());
        assert!(Subscription::of(&m(Folder::Junk)).is_none());
        assert!(Subscription::of(&m(Folder::Trash)).is_none());
        assert_eq!(group([m(Folder::Junk), m(Folder::Trash)]).len(), 0);
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
