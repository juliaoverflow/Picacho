//! The message list's search: words and `field:value` terms, all of which a
//! message must match.
//!
//! - `from:ana` — the sender's name or address.
//! - `reply-to:list@example.com` — where replies go (`replyto:` too).
//! - `to:` (To or Cc), `cc:`, `subject:`.
//! - `is:unread`, `is:read`, `is:starred`, `is:unstarred`.
//! - `has:attachment`.
//! - `has:otp` — a sign-in or verification code (`has:code`, `has:2fa`).
//! - `has:package-update` — where a package is (`has:package`,
//!   `has:shipping`).
//! - `has:receipt` — a receipt or a refund's (`has:receipts`,
//!   `has:invoice`, `has:purchase`). Which mail is which kind is in
//!   `mail_kinds`.
//! - `newer:7d`, `older:1y` — received within, or longer ago than, so many
//!   days (`d`), weeks (`w`), months (`m`) or years (`y`) (`newer_than:`,
//!   `older_than:` too).
//! - A leading `-` excludes (`-from:noreply`); quotes keep spaces in a value
//!   or phrase (`from:"Ana López"`, `"weekly report"`).
//!
//! Matching ignores case. Anything not understood as a term (`http://x`,
//! `from:` with nothing after it) is searched for as written.
use super::mail_kinds::Kinds;
use super::model::Mail;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    /// Sender, address, subject or preview.
    Any,
    From,
    ReplyTo,
    To,
    Cc,
    Subject,
    /// A label (IMAP keyword or Gmail label) on it.
    Label,
    Unread,
    Read,
    Starred,
    Unstarred,
    Attachment,
    /// A sign-in or verification code.
    Code,
    /// News of where a package is.
    Package,
    /// A receipt, or a refund's.
    Receipt,
    /// Received within the last `value` seconds.
    Newer,
    /// Received longer ago than `value` seconds.
    Older,
    /// In the server's conversation `value` (as `Mail::thread_id` keeps
    /// it: "oid:…" or "gm:…"), case kept.
    Thread,
}

impl Field {
    /// Whether it is a kind of mail worked out from the cache
    /// (`Store::mail_kinds`), which a server's search can't tell.
    fn kind(&self) -> bool {
        matches!(self, Self::Code | Self::Package | Self::Receipt)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    field: Field,
    /// Lowercase; empty for flag terms (`is:`, `has:`).
    value: String,
    negated: bool,
}

/// A parsed search. The empty search matches everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    terms: Vec<Term>,
    /// Which mail is code mail, a package update or a receipt, as worked
    /// out from the cache; without it (or for mail not looked at), its
    /// subject, preview and sender decide.
    kinds: Option<Arc<Kinds>>,
}

/// The fields the search understands, for hints: (operator, what it finds).
pub const OPERATORS: &[(&str, &str)] = &[
    ("from:", "Sender name or address"),
    ("reply-to:", "Where replies go"),
    ("to:", "Recipients, copied too"),
    ("subject:", "Subject"),
    ("label:", "Mail with a label"),
    ("is:unread", "Unread mail"),
    ("is:starred", "Starred mail"),
    ("has:attachment", "Mail with attachments"),
    ("has:otp", "Sign-in and verification codes"),
    ("has:package-update", "News of where a package is"),
    ("has:receipt", "Receipts and refunds"),
    ("newer:", "Received within a time (7d, 2w, 3m, 1y)"),
    ("older:", "Received longer ago (7d, 2w, 3m, 1y)"),
];
/// What a server's search can do beyond IMAP4rev1's, for `imap_criteria`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImapSearch {
    /// WITHIN (RFC 5032): YOUNGER and OLDER, in seconds, by the server's
    /// clock; otherwise SINCE and BEFORE by date.
    pub within: bool,
    /// SEARCH=FUZZY (RFC 6203): words match approximately (typos, word
    /// forms), as people expect of a search box.
    pub fuzzy: bool,
    /// OBJECTID (RFC 8474 §6): THREADID is a search key.
    pub objectid: bool,
    /// Gmail's X-GM-EXT-1: X-GM-THRID is a search key.
    pub gmail: bool,
}
/// A length of time such as `7d`, `2w`, `3m`, `1y` (or `7`, days), in
/// seconds.
fn age(text: &str) -> Option<i64> {
    let text = text.trim();
    let (number, unit) = match text.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&text[..i], &text[i..]),
        None => (text, "d"),
    };
    let n: i64 = number.parse().ok().filter(|n| *n > 0 && *n < 100_000)?;
    let day = 86_400;
    Some(n * match unit {
        "h" => 3600,
        "d" => day,
        "w" => 7 * day,
        "m" => 30 * day,
        "y" => 365 * day,
        _ => return None,
    })
}
/// IMAP's date (`7-Nov-2026`) of `seconds` ago.
fn imap_date(seconds_ago: i64) -> String {
    let t = chrono::Utc::now() - chrono::Duration::seconds(seconds_ago);
    t.format("%-d-%b-%Y").to_string()
}

impl Search {
    pub fn parse(query: &str) -> Self {
        let terms = tokens(query)
            .into_iter()
            .filter_map(|(raw, quoted)| term(&raw, quoted))
            .collect();
        Self { terms, kinds: None }
    }
    /// The search, telling code mail, package updates and receipts by
    /// `kinds`.
    pub fn with_kinds(mut self, kinds: Option<Arc<Kinds>>) -> Self {
        self.kinds = kinds;
        self
    }
    /// Whether it asks for code mail, package updates or receipts, which
    /// are worked out from the cache (`Store::mail_kinds`).
    pub fn wants_kinds(&self) -> bool {
        self.terms.iter().any(|t| t.field.kind())
    }
    pub fn matches(&self, m: &Mail) -> bool {
        self.terms
            .iter()
            .all(|t| t.matches(m, self.kinds.as_deref()) != t.negated)
    }
    /// The words and phrases searched for anywhere (not `field:` terms),
    /// for the local full-text index: those a message must hold, and those
    /// it mustn't (`-word`).
    pub fn text_terms(&self) -> (Vec<String>, Vec<String>) {
        let mut with = Vec::new();
        let mut without = Vec::new();
        for t in self.terms.iter().filter(|t| t.field == Field::Any) {
            if t.negated {
                without.push(t.value.clone());
            } else {
                with.push(t.value.clone());
            }
        }
        (with, without)
    }
    /// Whether `m` matches every term but the words and phrases searched
    /// for anywhere: for mail whose text the local full-text index found
    /// them in (`text_terms`), which its preview may not show.
    pub fn matches_besides_text(&self, m: &Mail) -> bool {
        self.terms
            .iter()
            .filter(|t| t.field != Field::Any)
            .all(|t| t.matches(m, self.kinds.as_deref()) != t.negated)
    }
    /// The search's words and phrases alone (`-word` too), written as a
    /// query: what a server is asked for, for mail whose text isn't here to
    /// search. Empty when it has none.
    pub fn text_query(&self) -> String {
        self.terms
            .iter()
            .filter(|t| t.field == Field::Any && !t.value.is_empty())
            .map(|t| {
                let sign = if t.negated { "-" } else { "" };
                if t.value.contains(char::is_whitespace) {
                    format!("{sign}\"{}\"", t.value.replace('"', ""))
                } else {
                    format!("{sign}{}", t.value)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    /// Whether a message is the kinds of mail asked for (`has:otp`,
    /// `has:package-update`, `has:receipt`): what a server's search can't
    /// tell, checked on what it found.
    pub fn matches_kinds(&self, m: &Mail) -> bool {
        self.terms
            .iter()
            .filter(|t| t.field.kind())
            .all(|t| t.matches(m, self.kinds.as_deref()) != t.negated)
    }
    /// The search as IMAP SEARCH criteria, for asking the server to search
    /// the whole mailbox. `string` makes a search string (quoted, or a
    /// literal for text the session can't quote). Words search the whole
    /// message (TEXT), as server-side search does.
    #[cfg(test)]
    pub fn imap_criteria(
        &self,
        string: &dyn Fn(&str) -> anyhow::Result<String>,
    ) -> anyhow::Result<String> {
        self.imap_criteria_with(string, ImapSearch::default())
    }
    /// As `imap_criteria`, with what the server can do beyond IMAP4rev1.
    pub fn imap_criteria_with(
        &self,
        string: &dyn Fn(&str) -> anyhow::Result<String>,
        can: ImapSearch,
    ) -> anyhow::Result<String> {
        let mut parts = Vec::new();
        for t in &self.terms {
            let v = || string(&t.value);
            if t.field == Field::Thread {
                // A server's own conversation: THREADID (RFC 8474 §6) or
                // Gmail's X-GM-THRID, where the id is that kind; elsewhere
                // there is no such key, and the term narrows nothing there.
                if let Some(part) = thread_criterion(&t.value, can) {
                    parts.push(if t.negated { format!("NOT ({part})") } else { part });
                }
                continue;
            }
            // WITHIN's interval is a positive nz-number (RFC 5032 §3).
            let seconds = || t.value.parse::<i64>().unwrap_or(0).clamp(1, i64::from(u32::MAX));
            let part = match t.field {
                Field::Any if can.fuzzy => format!("FUZZY (TEXT {})", v()?),
                Field::Any => format!("TEXT {}", v()?),
                Field::Newer if can.within => format!("YOUNGER {}", seconds()),
                Field::Older if can.within => format!("OLDER {}", seconds()),
                // By date: SINCE takes the whole day, BEFORE excludes it.
                Field::Newer => format!("SINCE {}", imap_date(seconds())),
                Field::Older => format!("BEFORE {}", imap_date(seconds())),
                Field::From => format!("FROM {}", v()?),
                Field::ReplyTo => format!("HEADER Reply-To {}", v()?),
                Field::To => format!("OR TO {} CC {}", v()?, v()?),
                Field::Cc => format!("CC {}", v()?),
                Field::Subject => format!("SUBJECT {}", v()?),
                Field::Label => match crate::sync::transport::keyword(&t.value) {
                    Ok(keyword) => format!("KEYWORD {keyword}"),
                    // Not a keyword this server could have.
                    Err(_) => format!("TEXT {}", v()?),
                },
                Field::Unread => "UNSEEN".into(),
                Field::Read => "SEEN".into(),
                Field::Starred => "FLAGGED".into(),
                Field::Unstarred => "UNFLAGGED".into(),
                Field::Attachment => "HEADER Content-Type \"multipart/mixed\"".into(),
                // Servers know no such thing: mail whose subject may be
                // it, narrowed down locally (`matches_kinds`).
                Field::Code => any_subject(CODE_SUBJECT_WORDS, string)?,
                Field::Package => any_subject(PACKAGE_SUBJECT_WORDS, string)?,
                Field::Receipt => any_subject(RECEIPT_SUBJECT_WORDS, string)?,
                Field::Thread => unreachable!("handled above"),
            };
            parts.push(if t.negated { format!("NOT ({part})") } else { part });
        }
        Ok(if parts.is_empty() { "ALL".into() } else { parts.join(" ") })
    }
    /// The search as JMAP filter conditions (RFC 8621 §4.4.1), all of which
    /// a message must match. Words search the whole message (`text`).
    pub fn jmap_conditions(&self) -> Vec<serde_json::Value> {
        use serde_json::json;
        self.terms
            .iter()
            // JMAP's filter has no conversation (RFC 8621 §4.4.1); the
            // term is matched here, on what is found.
            .filter(|t| t.field != Field::Thread)
            .map(|t| {
                let v = &t.value;
                let c = match t.field {
                    Field::Any => json!({ "text": v }),
                    Field::From => json!({ "from": v }),
                    Field::ReplyTo => json!({ "header": ["Reply-To", v] }),
                    Field::To => {
                        json!({ "operator": "OR", "conditions": [{ "to": v }, { "cc": v }] })
                    }
                    Field::Cc => json!({ "cc": v }),
                    Field::Subject => json!({ "subject": v }),
                    Field::Label => json!({ "hasKeyword": v }),
                    Field::Unread => json!({ "notKeyword": "$seen" }),
                    Field::Read => json!({ "hasKeyword": "$seen" }),
                    Field::Starred => json!({ "hasKeyword": "$flagged" }),
                    Field::Unstarred => json!({ "notKeyword": "$flagged" }),
                    Field::Attachment => json!({ "hasAttachment": true }),
                    Field::Newer | Field::Older => {
                        let at = chrono::Utc::now()
                            - chrono::Duration::seconds(v.parse::<i64>().unwrap_or(0));
                        let at = at.format("%Y-%m-%dT%H:%M:%SZ").to_string();
                        if t.field == Field::Newer {
                            json!({ "after": at })
                        } else {
                            json!({ "before": at })
                        }
                    }
                    Field::Thread => unreachable!("filtered out above"),
                    Field::Code | Field::Package | Field::Receipt => {
                        let words = match t.field {
                            Field::Code => CODE_SUBJECT_WORDS,
                            Field::Package => PACKAGE_SUBJECT_WORDS,
                            _ => RECEIPT_SUBJECT_WORDS,
                        };
                        let conditions: Vec<_> =
                            words.iter().map(|w| json!({ "subject": w })).collect();
                        json!({ "operator": "OR", "conditions": conditions })
                    }
                };
                if t.negated {
                    json!({ "operator": "NOT", "conditions": [c] })
                } else {
                    c
                }
            })
            .collect()
    }
}

/// Words in the subject of code mail, of package updates and of receipts,
/// for a server's search: what it finds is narrowed down locally.
const CODE_SUBJECT_WORDS: &[&str] = &[
    "code", "verif", "passcode", "one-time", "sign in", "sign-in", "login", "log in", "2fa", "otp",
];
const PACKAGE_SUBJECT_WORDS: &[&str] = &[
    "shipped", "delivered", "delivery", "tracking", "package", "parcel", "shipment", "in transit",
    "on its way", "on the way", "arriving",
];
const RECEIPT_SUBJECT_WORDS: &[&str] = &[
    "receipt", "invoice", "order", "purchase", "payment", "paid", "refund", "rechnung", "quittung",
    "factura", "recibo", "facture", "reçu", "fattura", "ricevuta",
];

/// The IMAP search key for the server conversation `thread` (as
/// `Mail::thread_id` writes it), where the server has one for that kind.
fn thread_criterion(thread: &str, can: ImapSearch) -> Option<String> {
    if let Some(id) = thread.strip_prefix("oid:") {
        // RFC 8474 §4: objectid is 1–255 of A-Z a-z 0-9 _ -.
        let valid = (1..=255).contains(&id.len())
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        return (can.objectid && valid).then(|| format!("THREADID {id}"));
    }
    if let Some(id) = thread.strip_prefix("gm:") {
        let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
        return (can.gmail && valid).then(|| format!("X-GM-THRID {id}"));
    }
    None
}

/// IMAP criteria for a subject with any of `words`: ORs, nested, as IMAP
/// takes two at a time.
fn any_subject(
    words: &[&str],
    string: &dyn Fn(&str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let mut out = format!("SUBJECT {}", string(words[0])?);
    for w in &words[1..] {
        out = format!("OR ({out}) SUBJECT {}", string(w)?);
    }
    Ok(out)
}

impl Term {
    fn matches(&self, m: &Mail, kinds: Option<&Kinds>) -> bool {
        // Most text is ASCII: compared without a lowercase copy of each
        // field, for each term, of every message searched.
        let has = |s: &str| {
            if s.is_ascii() && self.value.is_ascii() {
                contains_ignoring_ascii_case(s, &self.value)
            } else {
                s.to_lowercase().contains(&self.value)
            }
        };
        match self.field {
            Field::Any => [&m.sender, &m.from, &m.subject, &m.snippet]
                .iter()
                .any(|s| has(s)),
            Field::From => has(&m.from) || has(&m.sender),
            Field::ReplyTo => has(&m.reply_to),
            Field::To => has(&m.to) || has(m.cc()),
            Field::Cc => has(m.cc()),
            Field::Subject => has(&m.subject),
            Field::Label => m
                .labels()
                .iter()
                .any(|l| has(l) || has(&crate::sync::transport::decode_name(l))),
            Field::Unread => m.unread,
            Field::Read => !m.unread,
            Field::Starred => m.starred,
            Field::Unstarred => !m.starred,
            Field::Attachment => m.has_attachments(),
            Field::Code => Kinds::code(kinds, m),
            Field::Package => Kinds::package(kinds, m),
            Field::Receipt => Kinds::receipt(kinds, m),
            Field::Thread => m.thread_id() == self.value,
            Field::Newer | Field::Older => {
                let cutoff = chrono::Utc::now().timestamp() - self.value.parse::<i64>().unwrap_or(0);
                if self.field == Field::Newer {
                    m.date >= cutoff
                } else {
                    m.date < cutoff
                }
            }
        }
    }
}

/// Whether `text` has `lower` (lowercase) in it, ignoring ASCII case.
fn contains_ignoring_ascii_case(text: &str, lower: &str) -> bool {
    let (text, lower) = (text.as_bytes(), lower.as_bytes());
    lower.is_empty()
        || text
            .windows(lower.len())
            .any(|w| w.iter().zip(lower).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

/// Splits on whitespace outside double quotes. Each token comes with
/// whether it began with a quote, which keeps `"from:x"` a plain phrase.
fn tokens(query: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    for ch in query.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                quoted |= current.is_empty();
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() || quoted {
                    out.push((std::mem::take(&mut current), quoted));
                }
                quoted = false;
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() || quoted {
        out.push((current, quoted));
    }
    out
}

fn term(raw: &str, phrase: bool) -> Option<Term> {
    let lower = raw.to_lowercase();
    if phrase {
        return (!lower.is_empty()).then_some(Term {
            field: Field::Any,
            value: lower,
            negated: false,
        });
    }
    // A lone "-" is a word, not an exclusion.
    let (negated, body) = match lower.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest.to_owned()),
        _ => (false, lower.clone()),
    };
    let any = |value: String| {
        (!value.is_empty()).then_some(Term {
            field: Field::Any,
            value,
            negated: false,
        })
    };
    let Some((key, value)) = body.split_once(':') else {
        return if negated {
            Some(Term {
                field: Field::Any,
                value: body,
                negated,
            })
        } else {
            any(lower)
        };
    };
    let flag = |field| {
        Some(Term {
            field,
            value: String::new(),
            negated,
        })
    };
    let field = match key {
        "from" | "sender" => Field::From,
        "reply-to" | "replyto" | "reply_to" => Field::ReplyTo,
        "to" => Field::To,
        "cc" => Field::Cc,
        "subject" => Field::Subject,
        "label" | "tag" | "keyword" => Field::Label,
        // Backend-only until the UI calls it: a conversation's messages on
        // the server. Ids are case-sensitive, so the typed one is kept.
        "thread" => {
            let raw_value = raw.split_once(':').map_or("", |(_, v)| v).trim();
            if raw_value.is_empty() {
                return None;
            }
            return Some(Term {
                field: Field::Thread,
                value: raw_value.to_owned(),
                negated,
            });
        }
        "newer" | "newer_than" | "older" | "older_than" => {
            let Some(seconds) = age(value) else {
                return any(lower);
            };
            return Some(Term {
                field: if key.starts_with("newer") { Field::Newer } else { Field::Older },
                value: seconds.to_string(),
                negated,
            });
        }
        "is" => {
            return match value {
                "unread" => flag(Field::Unread),
                "read" => flag(Field::Read),
                "starred" | "flagged" => flag(Field::Starred),
                "unstarred" => flag(Field::Unstarred),
                _ => any(lower),
            };
        }
        "has" => {
            return match value {
                "attachment" | "attachments" => flag(Field::Attachment),
                "otp" | "code" | "codes" | "2fa" | "verification-code" => flag(Field::Code),
                "package-update" | "package-updates" | "package" | "packages" | "parcel"
                | "shipping" => flag(Field::Package),
                "receipt" | "receipts" | "invoice" | "invoices" | "purchase" | "purchases"
                | "refund" | "refunds" => flag(Field::Receipt),
                _ => any(lower),
            };
        }
        _ => return any(lower),
    };
    // `from:` alone (still being typed, or a quoted empty value) narrows
    // nothing yet.
    if value.is_empty() {
        return None;
    }
    // Addresses are often pasted in angle brackets.
    let value = value.trim_matches(|c| c == '<' || c == '>').to_owned();
    (!value.is_empty()).then_some(Term {
        field,
        value,
        negated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8474 §6: `thread:` searches the server's conversation by
    /// THREADID where OBJECTID is offered, X-GM-THRID on Gmail, and by
    /// nothing elsewhere; locally it matches the cached thread id.
    #[test]
    fn thread_terms_search_the_servers_conversation() {
        let quote = |s: &str| Ok(format!("\"{s}\""));
        let objectid = ImapSearch { objectid: true, ..ImapSearch::default() };
        let gmail = ImapSearch { gmail: true, ..ImapSearch::default() };
        let s = Search::parse("thread:oid:T64b478a75b7ea9 report");
        assert_eq!(s.imap_criteria_with(&quote, objectid).unwrap(), "THREADID T64b478a75b7ea9 TEXT \"report\"");
        assert_eq!(s.imap_criteria_with(&quote, gmail).unwrap(), "TEXT \"report\"");
        assert_eq!(s.imap_criteria(&quote).unwrap(), "TEXT \"report\"");
        let g = Search::parse("-thread:gm:1278455344230334865");
        assert_eq!(g.imap_criteria_with(&quote, gmail).unwrap(), "NOT (X-GM-THRID 1278455344230334865)");
        // Not an id a server could have: nothing is sent for it.
        assert_eq!(Search::parse("thread:oid:a.b(c)").imap_criteria_with(&quote, objectid).unwrap(), "ALL");
        assert!(Search::parse("thread:").terms.is_empty());
        assert!(Search::parse("thread:oid:X").jmap_conditions().is_empty());
        let mut m = Mail::default();
        m.set_thread_id("oid:T64b478a75b7ea9");
        assert!(Search::parse("thread:oid:T64b478a75b7ea9").matches(&m));
        assert!(!Search::parse("thread:oid:t64b478a75b7ea9").matches(&m), "case kept");
    }

    #[test]
    fn ages_search_by_the_servers_clock_where_it_can() {
        let quote = |s: &str| Ok(format!("\"{s}\""));
        let s = Search::parse("newer:2d report");
        assert_eq!(
            s.imap_criteria_with(&quote, ImapSearch { within: true, fuzzy: true, ..ImapSearch::default() }).unwrap(),
            "YOUNGER 172800 FUZZY (TEXT \"report\")"
        );
        assert!(Search::parse("older:1y").imap_criteria(&quote).unwrap().starts_with("BEFORE "));
        assert!(Search::parse("newer:1w").imap_criteria(&quote).unwrap().starts_with("SINCE "));
        assert_eq!(Search::parse("newer:soon").terms[0].field, Field::Any);
        let mut m = mail();
        m.date = chrono::Utc::now().timestamp() - 3 * 86_400;
        assert!(Search::parse("newer:1w").matches(&m) && !Search::parse("older:1w").matches(&m));
    }

    fn mail() -> Mail {
        let mut m = Mail {
            id: "1".into(),
            from: "news@shop.example".into(),
            sender: "Shop News".into(),
            reply_to: "Support <help@shop.example>".into(),
            to: "ana@example.com".into(),
            subject: "Weekly report".into(),
            snippet: "Your order shipped".into(),
            unread: true,
            ..Mail::default()
        };
        m.set_cc("team@example.com");
        m
    }

    fn hit(q: &str) -> bool {
        Search::parse(q).matches(&mail())
    }

    #[test]
    fn plain_words_must_all_appear_somewhere() {
        assert!(hit(""));
        assert!(hit("weekly"));
        assert!(hit("WEEKLY shop"));
        assert!(hit("\"weekly report\""));
        assert!(!hit("\"report weekly\""));
        assert!(!hit("weekly invoice"));
        assert!(!hit("\"from:shop\""), "a quoted operator is a phrase");
    }

    #[test]
    fn from_matches_name_or_address() {
        assert!(hit("from:news@shop.example"));
        assert!(hit("from:shop.example"));
        assert!(hit("from:\"shop news\""));
        assert!(hit("FROM:<news@shop.example>"));
        assert!(!hit("from:help@shop.example"));
        assert!(!hit("from:news@shop.example -from:shop"));
    }

    #[test]
    fn reply_to_matches_where_replies_go() {
        assert!(hit("reply-to:help@shop.example"));
        assert!(hit("replyto:support"));
        assert!(!hit("reply-to:news@shop.example"));
        assert!(hit("-reply-to:news@shop.example"));
        let plain = Mail {
            reply_to: String::new(),
            ..mail()
        };
        assert!(!Search::parse("reply-to:shop").matches(&plain));
    }

    #[test]
    fn fields_and_flags() {
        assert!(hit("to:team@example.com"));
        assert!(!hit("cc:ana@"));
        assert!(hit("subject:report is:unread"));
        assert!(!hit("is:read"));
        assert!(!hit("is:starred"));
        assert!(hit("-is:starred"));
        assert!(!hit("has:attachment"));
    }

    #[test]
    fn server_criteria() {
        let quote = |s: &str| -> anyhow::Result<String> { Ok(format!("\"{s}\"")) };
        assert_eq!(Search::parse("").imap_criteria(&quote).unwrap(), "ALL");
        assert_eq!(
            Search::parse("from:ana -is:read report").imap_criteria(&quote).unwrap(),
            "FROM \"ana\" NOT (SEEN) TEXT \"report\""
        );
        assert_eq!(
            Search::parse("to:team has:attachment").imap_criteria(&quote).unwrap(),
            "OR TO \"team\" CC \"team\" HEADER Content-Type \"multipart/mixed\""
        );
    }

    #[test]
    fn code_mail_and_package_updates() {
        let code = Mail {
            id: "code".into(),
            subject: "Your verification code".into(),
            snippet: "Your code is 482913. It expires in 10 minutes.".into(),
            ..Mail::default()
        };
        let parcel = Mail {
            id: "parcel".into(),
            from: "shop@example.com".into(),
            subject: "Your package has been delivered".into(),
            ..Mail::default()
        };
        let both = [code.clone(), parcel.clone(), mail()];
        let found = |q: &str, kinds: Option<Arc<Kinds>>| -> Vec<String> {
            let s = Search::parse(q).with_kinds(kinds);
            both.iter().filter(|m| s.matches(m)).map(|m| m.id.clone()).collect()
        };
        // Before the mail is looked at: by subject and preview.
        assert_eq!(found("has:otp", None), ["code"]);
        assert_eq!(found("has:2fa", None), ["code"]);
        assert_eq!(found("has:package-update", None), ["parcel"]);
        assert_eq!(found("has:package", None), ["parcel"]);
        assert_eq!(found("-has:otp -has:package-update", None), ["1"]);
        assert!(Search::parse("weekly has:otp").wants_kinds());
        assert!(!Search::parse("has:attachment").wants_kinds());
        // Looked at: what was worked out decides, as when the body says
        // what the preview didn't.
        let mut kinds = Kinds::default();
        kinds.checked.extend(["code".into(), "parcel".into(), "1".into()]);
        kinds.codes.insert("parcel".into());
        kinds.packages.insert("1".into());
        let kinds = Some(Arc::new(kinds));
        assert_eq!(found("has:otp", kinds.clone()), ["parcel"]);
        assert_eq!(found("has:package-update", kinds.clone()), ["1"]);
        assert_eq!(found("has:receipt", kinds.clone()), Vec::<String>::new());
        // A server's results are narrowed down to the kinds alone.
        let s = Search::parse("weekly has:package-update").with_kinds(kinds);
        assert!(s.matches_kinds(&mail()) && !s.matches_kinds(&code));
    }

    #[test]
    fn receipts() {
        let receipt = Mail {
            id: "receipt".into(),
            from: "receipts@store.example".into(),
            sender: "Example Store".into(),
            subject: "Your receipt from Example Store.".into(),
            ..Mail::default()
        };
        let mut sent = receipt.clone();
        sent.id = "sent".into();
        sent.folder = crate::manage::model::Folder::Sent;
        let mail = [receipt.clone(), sent, mail()];
        let found = |q: &str, kinds: Option<Arc<Kinds>>| -> Vec<String> {
            let s = Search::parse(q).with_kinds(kinds);
            mail.iter().filter(|m| s.matches(m)).map(|m| m.id.clone()).collect()
        };
        // By subject and sender before the mail is looked at; never sent mail.
        assert_eq!(found("has:receipt", None), ["receipt"]);
        assert_eq!(found("has:invoice", None), ["receipt"]);
        assert_eq!(found("-has:receipt", None), ["sent", "1"]);
        assert!(Search::parse("has:receipts").wants_kinds());
        // Looked at: what the Summary found decides.
        let mut kinds = Kinds::default();
        kinds.checked.extend(["receipt".into(), "1".into()]);
        kinds.receipts.insert("1".into());
        let kinds = Some(Arc::new(kinds));
        assert_eq!(found("has:receipt", kinds.clone()), ["1"]);
        let s = Search::parse("weekly has:receipt").with_kinds(kinds);
        assert!(s.matches_kinds(&mail[2]) && !s.matches_kinds(&receipt));
    }

    #[test]
    fn code_mail_and_package_updates_at_servers() {
        let quote = |s: &str| -> anyhow::Result<String> { Ok(format!("\"{s}\"")) };
        let imap = Search::parse("has:otp").imap_criteria(&quote).unwrap();
        assert!(imap.starts_with("OR (OR (") && imap.contains("SUBJECT \"verif\""), "{imap}");
        let negated = Search::parse("-has:package").imap_criteria(&quote).unwrap();
        assert!(negated.starts_with("NOT (OR (") && negated.contains("SUBJECT \"delivered\""));
        let receipts = Search::parse("has:receipt").imap_criteria(&quote).unwrap();
        assert!(receipts.contains("SUBJECT \"receipt\"") && receipts.contains("SUBJECT \"invoice\""));
        let jmap = Search::parse("has:package-update").jmap_conditions();
        assert_eq!(jmap[0]["operator"], "OR");
        assert!(jmap[0]["conditions"].as_array().unwrap().contains(&serde_json::json!({ "subject": "shipped" })));
    }

    #[test]
    fn unfinished_or_unknown_terms() {
        // Still typing: no narrowing yet.
        assert!(hit("from:"));
        assert!(hit("weekly from:"));
        // Not an operator: searched as written.
        assert!(!hit("https://shop.example"));
        assert!(!hit("is:pinned"));
        assert!(!hit("-"), "a lone dash is a word");
    }

    #[test]
    fn words_apart_from_operators() {
        let s = Search::parse("from:shop invoice \"due date\" -draft is:unread");
        assert_eq!(
            s.text_terms(),
            (vec!["invoice".to_string(), "due date".to_string()], vec!["draft".to_string()])
        );
        assert_eq!(s.text_query(), "invoice \"due date\" -draft");
        // The preview doesn't say "invoice", so the search alone misses it;
        // found in its text, the operators still decide.
        assert!(!s.matches(&mail()));
        assert!(s.matches_besides_text(&mail()));
        let mut read = mail();
        read.unread = false;
        assert!(!s.matches_besides_text(&read));
        assert!(!Search::parse("from:someone-else invoice").matches_besides_text(&mail()));
        assert_eq!(Search::parse("from:shop").text_terms(), (vec![], vec![]));
    }
}
