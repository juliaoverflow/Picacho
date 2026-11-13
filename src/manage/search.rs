//! The message list's search: words and `field:value` terms, all of which a
//! message must match.
//!
//! - `from:ana` — the sender's name or address.
//! - `reply-to:list@example.com` — where replies go (`replyto:` too).
//! - `to:` (To or Cc), `cc:`, `subject:`.
//! - `is:unread`, `is:read`, `is:starred`, `is:unstarred`.
//! - `has:attachment`.
//! - A leading `-` excludes (`-from:noreply`); quotes keep spaces in a value
//!   or phrase (`from:"Ana López"`, `"weekly report"`).
//!
//! Matching ignores case. Anything not understood as a term (`http://x`,
//! `from:` with nothing after it) is searched for as written.
use super::model::Mail;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    /// Sender, address, subject or preview.
    Any,
    From,
    ReplyTo,
    To,
    Cc,
    Subject,
    Unread,
    Read,
    Starred,
    Unstarred,
    Attachment,
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
}

/// The fields the search understands, for hints: (operator, what it finds).
pub const OPERATORS: &[(&str, &str)] = &[
    ("from:", "Sender name or address"),
    ("reply-to:", "Where replies go"),
    ("to:", "Recipients, copied too"),
    ("subject:", "Subject"),
    ("is:unread", "Unread mail"),
    ("is:starred", "Starred mail"),
    ("has:attachment", "Mail with attachments"),
];

impl Search {
    pub fn parse(query: &str) -> Self {
        let terms = tokens(query)
            .into_iter()
            .filter_map(|(raw, quoted)| term(&raw, quoted))
            .collect();
        Self { terms }
    }
    pub fn matches(&self, m: &Mail) -> bool {
        self.terms.iter().all(|t| t.matches(m) != t.negated)
    }
}

impl Term {
    fn matches(&self, m: &Mail) -> bool {
        let has = |s: &str| s.to_lowercase().contains(&self.value);
        match self.field {
            Field::Any => [&m.sender, &m.from, &m.subject, &m.snippet]
                .iter()
                .any(|s| has(s)),
            Field::From => has(&m.from) || has(&m.sender),
            Field::ReplyTo => has(&m.reply_to),
            Field::To => has(&m.to) || has(&m.cc),
            Field::Cc => has(&m.cc),
            Field::Subject => has(&m.subject),
            Field::Unread => m.unread,
            Field::Read => !m.unread,
            Field::Starred => m.starred,
            Field::Unstarred => !m.starred,
            Field::Attachment => m.has_attachments(),
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
        Mail {
            id: "1".into(),
            from: "news@shop.example".into(),
            sender: "Shop News".into(),
            reply_to: "Support <help@shop.example>".into(),
            to: "ana@example.com".into(),
            cc: "team@example.com".into(),
            subject: "Weekly report".into(),
            snippet: "Your order shipped".into(),
            unread: true,
            ..Mail::default()
        }
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
