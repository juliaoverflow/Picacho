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
];

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
    pub fn imap_criteria(
        &self,
        string: &dyn Fn(&str) -> anyhow::Result<String>,
    ) -> anyhow::Result<String> {
        let mut parts = Vec::new();
        for t in &self.terms {
            let v = || string(&t.value);
            let part = match t.field {
                Field::Any => format!("TEXT {}", v()?),
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
        let has = |s: &str| s.to_lowercase().contains(&self.value);
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
        }
    }
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
}
