//! Reading conversations: which messages belong together, where the quoted
//! history in a reply begins, and short recipient lists.

/// A subject without reply and forward prefixes ("Re:", "Fwd:", "AW:"…),
/// any number of them, compared case-insensitively.
pub(super) fn conversation_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let prefix = [
            "re:", "fwd:", "fw:", "aw:", "wg:", "sv:", "vs:", "tr:", "rv:", "réf:",
        ]
        .iter()
        .find(|p| lower.starts_with(*p));
        // "Re[2]:" and "Re (2):" as some clients write them.
        let numbered = lower.starts_with("re[") || lower.starts_with("re (");
        match (prefix, numbered) {
            (Some(p), _) => s = s[p.len()..].trim_start(),
            (None, true) => match s.find(':') {
                Some(i) if i < 8 => s = s[i + 1..].trim_start(),
                _ => break,
            },
            _ => break,
        }
    }
    s.to_lowercase()
}

/// Whether `subject` can have `key` as its `conversation_subject`, decided
/// without allocating: that is a suffix of the trimmed subject, lowercased,
/// so the subject must end in `key` (in any case). Run over every listed
/// message when a conversation is gathered, this rules most out at once.
pub(super) fn may_be_conversation(subject: &str, key: &str) -> bool {
    let subject = subject.trim();
    if subject.is_ascii() && key.is_ascii() {
        return subject.len() >= key.len()
            && subject.as_bytes()[subject.len() - key.len()..].eq_ignore_ascii_case(key.as_bytes());
    }
    // Lowercased a character at a time, from the end. Lowercasing a whole
    // string turns a final Σ into ς, one character alone into σ: they count
    // as the same, so that a subject that matches is never ruled out.
    let fold = |c: char| if c == 'ς' { 'σ' } else { c };
    let mut tail = subject
        .chars()
        .rev()
        .flat_map(|c| c.to_lowercase().rev())
        .map(fold);
    key.chars().rev().map(fold).all(|k| tail.next() == Some(k))
}

/// `m`'s conversation among `messages`, as indexes, oldest first,
/// including `m` itself. The server's thread ids decide where both
/// messages have one (Gmail's X-GM-THRID, OBJECTID's THREADID, or IMAP
/// THREAD within a mailbox); otherwise messages join by References, or by
/// subject when at least one is a reply (so newsletters that reuse a
/// subject stay apart).
pub(super) fn conversation(messages: &[Mail], m: &Mail) -> Vec<usize> {
    let key = conversation_subject(&m.subject);
    let mut rows = members(messages, m, &key, 0..messages.len());
    rows.sort_by_key(|i| messages[*i].date);
    rows
}

use crate::manage::model::Mail;
use std::collections::{HashMap, HashSet};

/// Of `candidates` (indexes into `messages`, in order), those in `m`'s
/// conversation (`key` being its `conversation_subject`), each message
/// once however many labels or folders it's under: `conversation`'s rule,
/// which grouping the list (`group`) shares.
fn members(messages: &[Mail], m: &Mail, key: &str, candidates: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut seen = HashSet::new();
    candidates
        .filter(|&i| belongs(m, key, &messages[i]))
        // The same message under several labels or folders shows once.
        .filter(|&i| {
            let x = &messages[i];
            x.message_id.is_empty() || seen.insert(x.message_id.as_str())
        })
        .collect()
}

/// Whether `x` is in `m`'s conversation (`key` being `m`'s
/// `conversation_subject`).
fn belongs(m: &Mail, key: &str, x: &Mail) -> bool {
    if x.account != m.account || x.pending {
        return false;
    }
    let replyish = |x: &Mail| !x.references().is_empty() || x.subject.trim().to_lowercase() != key;
    let linked = |a: &Mail, b: &Mail| !b.message_id.is_empty() && a.references().contains(b.message_id.as_str());
    // Permanent server thread ids (not IMAP THREAD's, which only cover
    // one mailbox) settle it either way.
    let permanent = |x: &Mail| !x.thread_id().is_empty() && !x.thread_id().starts_with("t:");
    if !x.thread_id().is_empty() && !m.thread_id().is_empty() {
        if x.thread_id() == m.thread_id() {
            return true;
        }
        if permanent(x) && permanent(m) {
            return x.id == m.id;
        }
    }
    x.id == m.id
        || linked(x, m)
        || linked(m, x)
        || (key.chars().count() > 2
            && may_be_conversation(&x.subject, key)
            && conversation_subject(&x.subject) == key
            && (replyish(x) || replyish(m)))
}

/// The Message-IDs a References (or In-Reply-To) header names, without
/// their angle brackets.
fn referenced(references: &str) -> impl Iterator<Item = &str> {
    references
        .split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == ',')
        .filter(|r| !r.is_empty())
}

/// Messages with one conversation subject: all of them, and those that
/// are replies.
type SubjectRows = (Vec<usize>, Vec<usize>);

/// The list's rows grouped by conversation, as the reader gathers them:
/// of `rows` (indexes into `messages`, in the order shown), the first of
/// each conversation, and how many messages each kept row's conversation
/// holds (among all of `messages`, which is what opening it shows).
///
/// Each kept row's conversation is `conversation`'s for it, found through
/// indexes of thread ids, Message-IDs, references and subjects rather
/// than by going through every message for every row.
pub(super) fn group(messages: &[Mail], rows: &[usize]) -> (Vec<usize>, HashMap<usize, usize>) {
    let trim = |id: &str| id.trim().trim_matches(|c| c == '<' || c == '>').to_owned();
    let mut by_thread: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    let mut by_id: HashMap<(&str, String), Vec<usize>> = HashMap::new();
    let mut referring: HashMap<(&str, String), Vec<usize>> = HashMap::new();
    // By subject: every message with it, and those that are replies.
    let mut by_subject: HashMap<(&str, String), SubjectRows> = HashMap::new();
    let mut subjects: Vec<String> = Vec::with_capacity(messages.len());
    for (i, x) in messages.iter().enumerate() {
        let key = conversation_subject(&x.subject);
        if x.pending {
            subjects.push(key);
            continue;
        }
        let account = x.account.as_str();
        if !x.thread_id().is_empty() {
            by_thread.entry((account, x.thread_id())).or_default().push(i);
        }
        if !x.message_id.is_empty() {
            by_id.entry((account, trim(&x.message_id))).or_default().push(i);
        }
        for r in referenced(x.references()) {
            referring.entry((account, r.to_owned())).or_default().push(i);
        }
        if key.chars().count() > 2 {
            let entry = by_subject.entry((account, key.clone())).or_default();
            entry.0.push(i);
            if !x.references().is_empty() || x.subject.trim().to_lowercase() != key {
                entry.1.push(i);
            }
        }
        subjects.push(key);
    }
    let mut taken: HashSet<usize> = HashSet::new();
    let mut kept = Vec::new();
    let mut counts = HashMap::new();
    let mut candidates: Vec<usize> = Vec::new();
    for &row in rows {
        if taken.contains(&row) {
            continue;
        }
        kept.push(row);
        let m = &messages[row];
        if m.pending {
            continue;
        }
        let account = m.account.as_str();
        let key = &subjects[row];
        candidates.clear();
        candidates.push(row);
        if !m.thread_id().is_empty()
            && let Some(v) = by_thread.get(&(account, m.thread_id()))
        {
            candidates.extend(v);
        }
        if !m.message_id.is_empty()
            && let Some(v) = referring.get(&(account, trim(&m.message_id)))
        {
            candidates.extend(v);
        }
        for r in referenced(m.references()) {
            if let Some(v) = by_id.get(&(account, r.to_owned())) {
                candidates.extend(v);
            }
        }
        if let Some((all, replies)) = by_subject.get(&(account, key.clone())) {
            let replyish = !m.references().is_empty() || m.subject.trim().to_lowercase() != *key;
            candidates.extend(if replyish { all } else { replies });
        }
        // In the list's order, as `conversation` goes through them.
        candidates.sort_unstable();
        candidates.dedup();
        let found = members(messages, m, key, candidates.iter().copied());
        // Other copies of the members (labels, folders) go with them.
        for &i in &candidates {
            if belongs(m, key, &messages[i]) {
                taken.insert(i);
            }
        }
        if found.len() > 1 {
            counts.insert(row, found.len());
        }
    }
    (kept, counts)
}

/// Splits a plain-text reply into what was written now and the quoted
/// history below it, when there is one worth hiding.
pub(super) fn split_quoted(body: &str) -> (&str, Option<&str>) {
    let mut offset = 0;
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let attribution = (t.starts_with("On ") && t.ends_with("wrote:"))
            || (t.starts_with("Le ") && t.ends_with("a écrit :"))
            || (t.starts_with("Am ") && t.ends_with("schrieb:"))
            || t.starts_with("-----Original Message-----")
            || t.starts_with("---------- Forwarded message")
            || t.starts_with("________________________________")
            // Outlook: "From: …" followed by "Sent:" within two lines.
            || (t.starts_with("From:")
                && lines[i + 1..]
                    .iter()
                    .take(2)
                    .any(|l| l.trim_start().starts_with("Sent:")));
        // A run of quoted lines that continues to the end (ignoring a
        // trailing signature) also counts.
        let quoted_tail = t.starts_with('>')
            && lines[i..]
                .iter()
                .filter(|l| !l.trim().is_empty())
                .take_while(|l| !l.trim_start().starts_with("-- "))
                .all(|l| l.trim_start().starts_with('>'));
        if (attribution || quoted_tail) && i > 0 {
            let written = body[..offset].trim_end();
            if written.is_empty() {
                return (body, None);
            }
            return (written, Some(&body[offset..]));
        }
        offset += line.len();
    }
    (body, None)
}

/// CSS that hides the quoted history of an HTML reply, for the common ways
/// mail programs mark it. `None` when the email has none.
pub(super) fn hide_html_quotes(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let markers = [
        "gmail_quote",
        "type=\"cite\"",
        "type=cite",
        "moz-cite-prefix",
        "divrplyfwdmsg",
        "appendonsend",
        "yahoo_quoted",
        "protonmail_quote",
    ];
    if !markers.iter().any(|m| lower.contains(m)) {
        return None;
    }
    let style = "<style>\
        .gmail_quote,.gmail_extra,blockquote[type=cite],.moz-cite-prefix,\
        #divRplyFwdMsg,#divRplyFwdMsg~*,#appendonsend~*,.yahoo_quoted,\
        .protonmail_quote{display:none!important}\
        </style>";
    // Before </head> if there is one, else first: either way it applies.
    Some(match lower.find("</head>") {
        Some(i) => format!("{}{style}{}", &html[..i], &html[i..]),
        None => format!("{style}{html}"),
    })
}

/// Short names for a recipient list: display names (first names when
/// several), "me" for the reader's own address.
pub(super) fn short_names(list: &str, own: &[String]) -> Vec<String> {
    let parsed = crate::sync::smtp::recipients(list).unwrap_or_default();
    let several = parsed.len() > 1;
    parsed
        .into_iter()
        .map(|(name, email)| {
            if own.iter().any(|o| o.eq_ignore_ascii_case(&email)) {
                return "me".to_string();
            }
            match name.filter(|n| !n.trim().is_empty()) {
                Some(n) if several => n.split_whitespace().next().unwrap_or(&n).to_string(),
                Some(n) => n,
                None => email.split('@').next().unwrap_or(&email).to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn conversation_subjects_ignore_prefixes() {
        assert_eq!(conversation_subject("Re: RE: Fwd: Plans"), "plans");
        assert_eq!(conversation_subject("AW: Plans"), "plans");
        assert_eq!(conversation_subject("Re[2]: Plans"), "plans");
        assert_eq!(conversation_subject("Reading list"), "reading list");
    }

    /// The quick check never rules out a subject that matches.
    #[test]
    fn the_quick_subject_check_keeps_every_match() {
        let subjects = [
            "Plans", "Re: Plans", "  RE:  fwd: PLANS  ", "Re[2]: Plans", "Re (3): plans",
            "AW: Plans", "Réf: Plans", "Re: Café plans", "Re: CAFÉ PLANS", "Plans B",
            "Other plans", "Re: ", "", "Re: İstanbul", "Re: KK plans",
            "Re: ΣΟΦΙΑΣ", "σοφιας", "🎁 Sale!", "Re: 🎁 SALE!",
            "Fwd: Café", "cafÉ", "Straße", "STRASSE",
        ];
        for subject in subjects {
            for other in subjects {
                let key = conversation_subject(other);
                if conversation_subject(subject) == key {
                    assert!(may_be_conversation(subject, &key), "{subject:?} / {key:?}");
                }
            }
        }
        assert!(!may_be_conversation("Re: Lunch", "plans"));
        assert!(!may_be_conversation("Plans B", "plans"));
        // Text that isn't ASCII is ruled out too, without lowercasing it whole.
        assert!(!may_be_conversation("🎁 Sale ends", "🎁 sale!"));
        assert!(may_be_conversation("RE: 🎁 SALE!", "🎁 sale!"));
    }

    #[test]
    fn quoted_history_is_split_from_the_reply() {
        let (written, quoted) =
            split_quoted("Sounds good!\n\nOn Mon, Ana wrote:\n> Lunch?\n> Bo\n");
        assert_eq!(written, "Sounds good!");
        assert!(quoted.unwrap().starts_with("On Mon"));
        let (written, quoted) = split_quoted("Yes\n> Lunch?\n>\n> Ana\n-- \nBo");
        assert_eq!(written, "Yes");
        assert!(quoted.is_some());
        // Inline answers between quotes are not hidden.
        let (_, quoted) = split_quoted("> Lunch?\nYes\n> Where?\nThe usual\n");
        assert!(quoted.is_none());
        let (_, quoted) = split_quoted("Hi\nFrom: Ana\nSent: Monday\nTo: Bo\n\nOld");
        assert!(quoted.is_some());
    }

    #[test]
    fn html_quotes_are_hidden_only_when_present() {
        assert!(hide_html_quotes("<p>Hi</p>").is_none());
        let html = hide_html_quotes("<html><head></head><body><div class=\"gmail_quote\">x</div>")
            .unwrap();
        assert!(html.contains("<style>") && html.find("<style>") < html.find("</head>"));
    }

    #[test]
    fn recipient_names_are_short() {
        let own = vec!["me@example.com".to_string()];
        assert_eq!(
            short_names("Ana Diaz <ana@x.com>, me@example.com, bo@x.com", &own),
            ["Ana", "me", "bo"]
        );
        assert_eq!(short_names("Ana Diaz <ana@x.com>", &own), ["Ana Diaz"]);
    }

    fn mail(id: &str, subject: &str, refs: &str, date: i64) -> Mail {
        let mut m = Mail {
            id: id.into(),
            account: "a".into(),
            subject: subject.into(),
            message_id: format!("<{id}@x>"),
            date,
            ..Mail::default()
        };
        m.set_references(refs);
        m
    }

    /// The list groups as the reader gathers: each kept row stands for its
    /// own conversation, counted as opening it shows.
    #[test]
    fn the_list_groups_conversations_as_the_reader_does() {
        let mut gmail = mail("g2", "Trip", "", 7);
        gmail.set_thread_id("gm:1");
        let mut gmail_start = mail("g1", "Plans", "", 6);
        gmail_start.set_thread_id("gm:1");
        let mut other_thread = mail("g3", "Re: Trip", "", 5);
        other_thread.set_thread_id("gm:2");
        let messages = vec![
            mail("3", "Re: Lunch", "<1@x> <2@x>", 9),
            mail("2", "Re: Lunch", "<1@x>", 8),
            gmail,
            gmail_start,
            other_thread,
            mail("4", "Weekly digest", "", 4),
            mail("5", "Weekly digest", "", 3),
            mail("6", "Re: Lunch", "", 2),
            mail("1", "Lunch", "", 1),
        ];
        let rows: Vec<usize> = (0..messages.len()).collect();
        let (kept, counts) = group(&messages, &rows);
        assert_eq!(kept, vec![0, 2, 4, 5, 6]);
        for &row in &kept {
            let whole = conversation(&messages, &messages[row]).len();
            assert_eq!(counts.get(&row).copied().unwrap_or(1), whole, "row {row}");
        }
        assert_eq!(counts.get(&0), Some(&4), "by References and by a reply's subject");
        assert_eq!(counts.get(&2), Some(&2), "by Gmail's thread, whatever the subjects");
        assert_eq!(counts.get(&5), None, "newsletters reusing a subject stay apart");
        // The same message under two labels counts once, and is one row.
        let mut copy = mail("2", "Re: Lunch", "<1@x>", 8);
        copy.id = "2-label".into();
        let mut with_copy = messages.clone();
        with_copy.insert(2, copy);
        let rows: Vec<usize> = (0..with_copy.len()).collect();
        let (kept, counts) = group(&with_copy, &rows);
        assert!(!kept.contains(&2));
        assert_eq!(counts.get(&0), Some(&4));
    }
}
