//! Server filters for accounts that use their provider's API: Picacho's
//! rules (`sieve::Rule`) as Gmail's filters and as Microsoft 365's inbox
//! rules, and back, with the away reply as each keeps it.
//!
//! A filter or rule set elsewhere that Picacho's rules can say (one test,
//! one action) is shown and edited here like any other; one that can't
//! (several tests, several actions, what Picacho has no words for) is left
//! as it is, and counted (`State::kept_elsewhere`). What a provider can't
//! do with a rule is refused when it is saved, saying what.
use super::*;
use crate::sync::gmail::{self, Label};
use crate::sync::graph::{self, MailFolder};
use crate::sync::sieve::{Act, Field, Match, Rule, State, Vacation};
use serde_json::{Value, json};

/// What the filter editor offers for Gmail (as Sieve extensions).
const GMAIL_FEATURES: &[&str] = &["fileinto", "imap4flags", "vacation", "date", "relational", "body", "envelope", "copy"];
/// And for Microsoft 365.
const GRAPH_FEATURES: &[&str] = &["fileinto", "imap4flags", "vacation", "date", "relational", "body", "copy"];

fn unsupported(service: &str, what: &str) -> anyhow::Error {
    anyhow::anyhow!(t!("manage-api-filters-cant-do-this", service = service, what = what))
}

/// A date (YYYY-MM-DD) as Unix seconds at its start (`end`: its end), UTC.
fn day(date: &str, end: bool) -> i64 {
    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .ok()
        .and_then(|d| if end { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) })
        .map_or(0, |t| t.and_utc().timestamp())
}
fn date_of(at: i64) -> Option<String> {
    (at > 0).then(|| chrono::DateTime::from_timestamp(at, 0).unwrap_or_default().format("%Y-%m-%d").to_string())
}

// --- Gmail ---------------------------------------------------------------

/// A Gmail search word for a rule's test.
fn gmail_query(r: &Rule) -> Result<String> {
    let v = r.value.trim();
    let quoted = if v.contains(' ') { format!("\"{v}\"") } else { v.to_owned() };
    Ok(match r.field {
        Field::From => format!("from:{quoted}"),
        Field::To => format!("to:{quoted}"),
        Field::Recipients => format!("{{to:{quoted} cc:{quoted}}}"),
        Field::Subject => format!("subject:{quoted}"),
        Field::List => format!("list:{quoted}"),
        Field::Body => quoted,
        Field::LargerThan => format!("larger:{}K", v.parse::<u64>().map_err(|_| unsupported("Gmail", v))?),
        Field::DeliveredTo => format!("deliveredto:{quoted}"),
        Field::Header => return Err(unsupported("Gmail", &t!("filters-header", header = r.header))),
    })
}
/// A rule's test, read back from a Gmail search word.
fn gmail_test(query: &str) -> (Field, String) {
    let unquote = |s: &str| s.trim().trim_matches('"').to_owned();
    for (prefix, field) in [
        ("from:", Field::From),
        ("to:", Field::To),
        ("subject:", Field::Subject),
        ("list:", Field::List),
        ("deliveredto:", Field::DeliveredTo),
    ] {
        if let Some(v) = query.strip_prefix(prefix) {
            return (field, unquote(v));
        }
    }
    if let Some(v) = query.strip_prefix("{to:") {
        let v = v.split(" cc:").next().unwrap_or(v);
        return (Field::Recipients, unquote(v));
    }
    if let Some(v) = query.strip_prefix("larger:") {
        return (Field::LargerThan, v.trim_end_matches(['K', 'k']).to_owned());
    }
    (Field::Body, unquote(query))
}

/// Picacho's rules as Gmail filters. Labels a rule names that Gmail
/// doesn't have yet are `missing` (to make first); their ids are their
/// names until then.
pub(super) fn gmail_filters(rules: &[Rule], labels: &[Label], missing: &mut Vec<String>) -> Result<Vec<gmail::Filter>> {
    let mut label_id = |name: &str| -> String {
        match labels.iter().find(|l| !l.system && l.name.eq_ignore_ascii_case(name)) {
            Some(l) => l.id.clone(),
            None => {
                if !missing.iter().any(|m| m.eq_ignore_ascii_case(name)) {
                    missing.push(name.to_owned());
                }
                name.to_owned()
            }
        }
    };
    let mut out = Vec::new();
    for r in rules {
        if matches!(r.matching, Match::Matches | Match::Regex) {
            return Err(unsupported("Gmail", r.matching.label()));
        }
        let mut f = gmail::Filter::default();
        let query = gmail_query(r)?;
        match r.field {
            // Mail that doesn't match, in Gmail's search words.
            _ if r.negate => f.query = format!("-({query})"),
            Field::From => f.from = r.value.trim().to_owned(),
            Field::To => f.to = r.value.trim().to_owned(),
            Field::Subject => f.subject = r.value.trim().to_owned(),
            _ => f.query = query,
        }
        out.push(act_gmail(r, &mut label_id, f)?);
    }
    Ok(out)
}
fn act_gmail(r: &Rule, label_id: &mut impl FnMut(&str) -> String, mut f: gmail::Filter) -> Result<gmail::Filter> {
    use gmail::label;
    match &r.act {
        // Kept in the inbox: never to Spam.
        Act::Inbox => f.remove_labels.push(label::SPAM.into()),
        Act::Folder(name) => {
            f.add_labels.push(label_id(name));
            f.remove_labels.push(label::INBOX.into());
        }
        Act::MarkRead => f.remove_labels.push(label::UNREAD.into()),
        Act::Star => f.add_labels.push(label::STARRED.into()),
        Act::Discard => f.add_labels.push(label::TRASH.into()),
        Act::Redirect(to) => {
            f.forward = to.trim().to_owned();
            if !r.keep_copy {
                f.add_labels.push(label::TRASH.into());
            }
        }
        Act::Reject(_) => return Err(unsupported("Gmail", t!("filters-act-refuse"))),
        Act::Label(name) => f.add_labels.push(label_id(name)),
    }
    Ok(f)
}

/// A Gmail filter as one of Picacho's rules, where one can say it.
fn gmail_rule(f: &gmail::Filter, labels: &[Label]) -> Option<Rule> {
    use gmail::label;
    let tests = [!f.from.is_empty(), !f.to.is_empty(), !f.subject.is_empty(), !f.query.is_empty(), f.has_attachment];
    if tests.iter().filter(|t| **t).count() != 1 {
        return None;
    }
    let (field, value, negate) = if !f.from.is_empty() {
        (Field::From, f.from.clone(), false)
    } else if !f.to.is_empty() {
        (Field::To, f.to.clone(), false)
    } else if !f.subject.is_empty() {
        (Field::Subject, f.subject.clone(), false)
    } else if let Some(inner) = f.query.strip_prefix("-(").and_then(|q| q.strip_suffix(')')) {
        let (field, value) = gmail_test(inner);
        (field, value, true)
    } else if !f.query.is_empty() {
        let (field, value) = gmail_test(&f.query);
        (field, value, false)
    } else {
        return None;
    };
    let name = |id: &str| labels.iter().find(|l| l.id == id && !l.system).map(|l| l.name.clone());
    let trash = f.add_labels.iter().any(|l| l == label::TRASH);
    let (act, keep_copy) = match (f.add_labels.as_slice(), f.remove_labels.as_slice(), f.forward.is_empty()) {
        (_, _, false) if f.add_labels.iter().all(|l| l == label::TRASH) && f.remove_labels.is_empty() => {
            (Act::Redirect(f.forward.clone()), !trash)
        }
        ([], [r], true) if r == label::SPAM => (Act::Inbox, false),
        ([], [r], true) if r == label::UNREAD => (Act::MarkRead, false),
        ([a], [], true) if a == label::STARRED => (Act::Star, false),
        ([a], [], true) if a == label::TRASH => (Act::Discard, false),
        ([a], [], true) => (Act::Label(name(a)?), false),
        ([a], [r], true) if r == label::INBOX => (Act::Folder(name(a)?), false),
        _ => return None,
    };
    Some(Rule { field, value, act, negate, keep_copy, ..Default::default() })
}

/// What Gmail has, as the filter editor shows it.
pub(super) fn gmail_state(filters: &[gmail::Filter], labels: &[Label], v: &gmail::Vacation) -> State {
    let rules: Vec<Rule> = filters.iter().filter_map(|f| gmail_rule(f, labels)).collect();
    let kept = filters.len() - rules.len();
    State {
        filters: crate::sync::sieve::Filters {
            rules,
            vacation: Vacation {
                on: v.enabled,
                subject: v.subject.clone(),
                body: v.body.clone(),
                days: 1,
                from: date_of(v.start),
                until: date_of(v.end),
                ..Default::default()
            },
            include: None,
        },
        extensions: GMAIL_FEATURES.iter().map(|s| s.to_string()).collect(),
        active: true,
        kept_elsewhere: kept,
        ..Default::default()
    }
}
pub(super) fn gmail_vacation(v: &Vacation) -> gmail::Vacation {
    gmail::Vacation {
        enabled: v.on,
        subject: v.subject.clone(),
        body: v.body.clone(),
        contacts_only: false,
        start: v.from.as_deref().map_or(0, |d| day(d, false)),
        end: v.until.as_deref().map_or(0, |d| day(d, true)),
    }
}

// --- Microsoft 365 ---------------------------------------------------------

/// The name Picacho gives the inbox rules it writes.
const GRAPH_NAME: &str = "Picacho";

/// Picacho's rules as inbox rules, in order.
pub(super) fn graph_rules(rules: &[Rule], folders: &[MailFolder]) -> Result<Vec<Value>> {
    const SERVICE: &str = "Microsoft 365";
    let mut out = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        if matches!(r.matching, Match::Matches | Match::Regex) {
            return Err(unsupported(SERVICE, r.matching.label()));
        }
        let v = r.value.trim();
        let person = || json!([{"emailAddress": {"address": v}}]);
        let test = match (r.field, r.matching) {
            (Field::From, Match::Is) => json!({"fromAddresses": person()}),
            (Field::From, _) => json!({"senderContains": [v]}),
            (Field::To, Match::Is) => json!({"sentToAddresses": person()}),
            (Field::To | Field::Recipients | Field::DeliveredTo, _) => json!({"recipientContains": [v]}),
            (Field::Subject, _) => json!({"subjectContains": [v]}),
            (Field::Body, _) => json!({"bodyContains": [v]}),
            (Field::List, _) => json!({"headerContains": [format!("List-Id: {v}")]}),
            (Field::Header, _) => json!({"headerContains": [format!("{}: {v}", r.header.trim())]}),
            (Field::LargerThan, _) => json!({"withinSizeRange": {
                "minimumSize": v.parse::<i64>().map_err(|_| unsupported(SERVICE, v))?,
                "maximumSize": i32::MAX,
            }}),
        };
        let folder = |name: &str| -> Result<String> {
            folders
                .iter()
                .find(|f| f.path == name || (name == "INBOX" && f.well_known.as_deref() == Some("inbox")))
                .map(|f| f.id.clone())
                .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = name))
        };
        let mut actions = match &r.act {
            Act::Inbox => json!({"moveToFolder": folder("INBOX")?}),
            Act::Folder(name) => json!({"moveToFolder": folder(name)?}),
            Act::MarkRead => json!({"markAsRead": true}),
            Act::Star => return Err(unsupported(SERVICE, t!("filters-star"))),
            Act::Discard => json!({"delete": true}),
            Act::Redirect(to) => {
                let mut a = json!({"redirectTo": [{"emailAddress": {"address": to.trim()}}]});
                if !r.keep_copy {
                    a["delete"] = true.into();
                }
                a
            }
            Act::Reject(_) => return Err(unsupported(SERVICE, t!("filters-act-refuse"))),
            Act::Label(name) => json!({"assignCategories": [name]}),
        };
        if r.stop {
            actions["stopProcessingRules"] = true.into();
        }
        let mut rule = json!({
            "displayName": format!("{GRAPH_NAME} {}", i + 1),
            "sequence": i + 1,
            "isEnabled": true,
            "actions": actions,
        });
        rule[if r.negate { "exceptions" } else { "conditions" }] = test;
        out.push(rule);
    }
    Ok(out)
}

/// An inbox rule as one of Picacho's rules, where one can say it.
fn graph_rule(rule: &Value, folders: &[MailFolder]) -> Option<Rule> {
    let tests = |v: &Value| v.as_object().map(|o| o.iter().filter(|(_, x)| !x.is_null()).count()).unwrap_or(0);
    let (test, negate) = match (tests(&rule["conditions"]), tests(&rule["exceptions"])) {
        (1, 0) => (&rule["conditions"], false),
        (0, 1) => (&rule["exceptions"], true),
        _ => return None,
    };
    let (key, value) = test.as_object()?.iter().find(|(_, v)| !v.is_null())?;
    let first = |v: &Value| -> Option<String> {
        let list = v.as_array()?;
        (list.len() == 1).then(|| list[0].as_str().map(str::to_owned).or_else(|| list[0]["emailAddress"]["address"].as_str().map(str::to_owned)))?
    };
    let (field, matching, value, header) = match key.as_str() {
        "fromAddresses" => (Field::From, Match::Is, first(value)?, String::new()),
        "senderContains" => (Field::From, Match::Contains, first(value)?, String::new()),
        "sentToAddresses" => (Field::To, Match::Is, first(value)?, String::new()),
        "recipientContains" => (Field::Recipients, Match::Contains, first(value)?, String::new()),
        "subjectContains" => (Field::Subject, Match::Contains, first(value)?, String::new()),
        "bodyContains" => (Field::Body, Match::Contains, first(value)?, String::new()),
        "headerContains" => {
            let h = first(value)?;
            match h.strip_prefix("List-Id: ") {
                Some(list) => (Field::List, Match::Contains, list.to_owned(), String::new()),
                None => {
                    let (name, v) = h.split_once(": ")?;
                    (Field::Header, Match::Contains, v.to_owned(), name.to_owned())
                }
            }
        }
        "withinSizeRange" => (Field::LargerThan, Match::Contains, value["minimumSize"].as_i64()?.to_string(), String::new()),
        _ => return None,
    };
    let actions = rule["actions"].as_object()?;
    let stop = actions.get("stopProcessingRules").and_then(Value::as_bool).unwrap_or(false);
    let set: Vec<(&String, &Value)> = actions
        .iter()
        .filter(|(k, v)| k.as_str() != "stopProcessingRules" && !v.is_null() && **v != json!(false))
        .collect();
    let folder_path = |id: &Value| -> Option<String> {
        let f = folders.iter().find(|f| Some(f.id.as_str()) == id.as_str())?;
        Some(if f.well_known.as_deref() == Some("inbox") { String::new() } else { f.path.clone() })
    };
    let (act, keep_copy) = match set.as_slice() {
        [(k, v)] if k.as_str() == "moveToFolder" => match folder_path(v)? {
            inbox if inbox.is_empty() => (Act::Inbox, false),
            path => (Act::Folder(path), false),
        },
        [(k, _)] if k.as_str() == "markAsRead" => (Act::MarkRead, false),
        [(k, _)] if k.as_str() == "delete" => (Act::Discard, false),
        [(k, v)] if k.as_str() == "redirectTo" => (Act::Redirect(first(v)?), true),
        [(k, v)] if k.as_str() == "assignCategories" => (Act::Label(first(v)?), false),
        // Sent on and deleted here: without a copy kept.
        [(a, x), (b, y)] if a.as_str() == "delete" && b.as_str() == "redirectTo" => {
            let _ = x;
            (Act::Redirect(first(y)?), false)
        }
        [(a, x), (b, y)] if a.as_str() == "redirectTo" && b.as_str() == "delete" => {
            let _ = y;
            (Act::Redirect(first(x)?), false)
        }
        _ => return None,
    };
    Some(Rule { field, value, act, matching, negate, header, keep_copy, stop })
}

/// What Microsoft 365 has, as the filter editor shows it. Rules are in
/// their order (`sequence`); turned-off ones are kept as they are.
pub(super) fn graph_state(rules: &[graph::Rule], folders: &[MailFolder], r: &graph::AutomaticReplies) -> (State, Vec<String>) {
    let mut ordered: Vec<&graph::Rule> = rules.iter().collect();
    ordered.sort_by_key(|x| x.json["sequence"].as_i64().unwrap_or(i64::MAX));
    let mut shown = Vec::new();
    let mut ids = Vec::new();
    for x in ordered {
        if x.json["isEnabled"].as_bool() == Some(false) {
            continue;
        }
        if let Some(rule) = graph_rule(&x.json, folders) {
            shown.push(rule);
            ids.push(x.id.clone());
        }
    }
    let kept = rules.len() - shown.len();
    let state = State {
        filters: crate::sync::sieve::Filters {
            rules: shown,
            vacation: Vacation {
                on: r.enabled,
                subject: String::new(),
                body: if r.external.is_empty() { r.internal.clone() } else { r.external.clone() },
                days: 1,
                from: date_of(r.start),
                until: date_of(r.end),
                ..Default::default()
            },
            include: None,
        },
        extensions: GRAPH_FEATURES.iter().map(|s| s.to_string()).collect(),
        active: true,
        kept_elsewhere: kept,
        ..Default::default()
    };
    (state, ids)
}
pub(super) fn graph_replies(v: &Vacation) -> graph::AutomaticReplies {
    graph::AutomaticReplies {
        enabled: v.on,
        // Microsoft's replies have no subject of their own: it leads the text.
        internal: if v.subject.trim().is_empty() { v.body.clone() } else { format!("{}\n\n{}", v.subject.trim(), v.body) },
        external: String::new(),
        contacts_only: false,
        start: v.from.as_deref().map_or(0, |d| day(d, false)),
        end: v.until.as_deref().map_or(0, |d| day(d, true)),
    }
}

impl Backend {
    pub(super) fn filters_gmail(&self, a: &Account) -> Result<State> {
        self.with_gmail(a, |s| {
            let labels = s.reload_labels()?;
            Ok(gmail_state(&s.filters()?, &labels, &s.vacation()?))
        })
    }
    /// Replaces the Gmail filters Picacho can show with `filters`' rules
    /// (making labels they name), and sets the away reply.
    pub(super) fn save_filters_gmail(&self, a: &Account, filters: &crate::sync::sieve::Filters) -> Result<State> {
        self.with_gmail(a, |s| {
            let mut labels = s.reload_labels()?;
            let mut missing = Vec::new();
            gmail_filters(&filters.rules, &labels, &mut missing)?;
            for name in &missing {
                s.create_label(name)?;
            }
            if !missing.is_empty() {
                labels = s.labels()?;
            }
            let wanted = gmail_filters(&filters.rules, &labels, &mut Vec::new())?;
            // Gmail has no changing a filter: those Picacho shows go, and
            // the rules are made again, in order.
            for f in s.filters()? {
                if gmail_rule(&f, &labels).is_some() {
                    s.delete_filter(&f.id)?;
                }
            }
            for f in &wanted {
                s.create_filter(f)?;
            }
            s.set_vacation(&gmail_vacation(&filters.vacation))?;
            Ok(gmail_state(&s.filters()?, &labels, &s.vacation()?))
        })
    }
    pub(super) fn filters_graph(&self, a: &Account) -> Result<State> {
        self.with_graph(a, |s| {
            let folders = s.folders()?;
            Ok(graph_state(&s.rules()?, &folders, &s.automatic_replies()?).0)
        })
    }
    /// Replaces the inbox rules Picacho can show with `filters`' rules, in
    /// order (each changed in place where there was one), and sets the
    /// automatic replies.
    pub(super) fn save_filters_graph(&self, a: &Account, filters: &crate::sync::sieve::Filters) -> Result<State> {
        self.with_graph(a, |s| {
            let folders = s.reload_folders()?;
            let wanted = graph_rules(&filters.rules, &folders)?;
            let (_, ids) = graph_state(&s.rules()?, &folders, &Default::default());
            for (i, rule) in wanted.iter().enumerate() {
                match ids.get(i) {
                    Some(id) => s.update_rule(id, rule)?,
                    None => {
                        s.create_rule(rule)?;
                    }
                }
            }
            for id in ids.iter().skip(wanted.len()) {
                s.delete_rule(id)?;
            }
            s.set_automatic_replies(&graph_replies(&filters.vacation))?;
            Ok(graph_state(&s.rules()?, &folders, &s.automatic_replies()?).0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(field: Field, value: &str, act: Act) -> Rule {
        Rule { field, value: value.into(), act, ..Default::default() }
    }

    #[test]
    fn rules_become_gmail_filters_and_come_back() -> Result<()> {
        let labels = vec![Label { id: "Label_1".into(), name: "Receipts".into(), system: false }];
        let rules = vec![
            rule(Field::From, "shop@example.com", Act::Folder("Receipts".into())),
            rule(Field::Subject, "invoice", Act::Star),
            rule(Field::List, "dev.example.org", Act::MarkRead),
            Rule { negate: true, ..rule(Field::To, "qa@example.com", Act::Label("Other".into())) },
            Rule { keep_copy: false, ..rule(Field::Recipients, "team@example.com", Act::Redirect("me@example.net".into())) },
            rule(Field::LargerThan, "5000", Act::Discard),
        ];
        let mut missing = Vec::new();
        let filters = gmail_filters(&rules, &labels, &mut missing)?;
        assert_eq!(missing, ["Other"]);
        assert_eq!(filters[0].from, "shop@example.com");
        assert_eq!((filters[0].add_labels.as_slice(), filters[0].remove_labels.as_slice()), (&["Label_1".to_owned()][..], &["INBOX".to_owned()][..]));
        assert_eq!(filters[2].query, "list:dev.example.org");
        assert_eq!(filters[3].query, "-(to:qa@example.com)");
        assert_eq!(filters[4].query, "{to:team@example.com cc:team@example.com}");
        assert!(filters[4].add_labels.contains(&"TRASH".into()), "without a copy kept");
        assert_eq!(filters[5].query, "larger:5000K");
        // Read back: the same rules, the label made meanwhile named.
        let mut labels = labels;
        labels.push(Label { id: "Other".into(), name: "Other".into(), system: false });
        let state = gmail_state(&filters, &labels, &gmail::Vacation::default());
        assert_eq!(state.filters.rules, rules);
        assert_eq!(state.kept_elsewhere, 0);
        // Several tests at once: left as it is.
        let both = gmail::Filter { from: "a@example.com".into(), subject: "x".into(), add_labels: vec!["STARRED".into()], ..Default::default() };
        assert_eq!(gmail_state(&[both], &labels, &Default::default()).kept_elsewhere, 1);
        // What Gmail can't do.
        assert!(gmail_filters(&[rule(Field::From, "a", Act::Reject("no".into()))], &labels, &mut missing).is_err());
        assert!(gmail_filters(&[Rule { matching: Match::Regex, ..rule(Field::From, "a.*", Act::Star) }], &labels, &mut missing).is_err());
        Ok(())
    }

    #[test]
    fn rules_become_inbox_rules_and_come_back() -> Result<()> {
        let folders = vec![
            MailFolder { id: "f-in".into(), name: "Inbox".into(), path: "Inbox".into(), well_known: Some("inbox".into()), ..Default::default() },
            MailFolder { id: "f-r".into(), name: "Receipts".into(), path: "Inbox/Receipts".into(), depth: 1, ..Default::default() },
        ];
        let rules = vec![
            Rule { matching: Match::Is, stop: true, ..rule(Field::From, "shop@example.com", Act::Folder("Inbox/Receipts".into())) },
            rule(Field::Subject, "invoice", Act::Label("Finance".into())),
            Rule { negate: true, ..rule(Field::Body, "unsubscribe", Act::MarkRead) },
            Rule { header: "X-Spam".into(), ..rule(Field::Header, "yes", Act::Discard) },
            Rule { keep_copy: true, ..rule(Field::Recipients, "team", Act::Redirect("me@example.net".into())) },
            rule(Field::From, "boss", Act::Inbox),
        ];
        let json = graph_rules(&rules, &folders)?;
        assert_eq!(json[0]["conditions"]["fromAddresses"][0]["emailAddress"]["address"], "shop@example.com");
        assert_eq!(json[0]["actions"]["moveToFolder"], "f-r");
        assert_eq!(json[0]["actions"]["stopProcessingRules"], true);
        assert_eq!(json[2]["exceptions"]["bodyContains"][0], "unsubscribe");
        assert_eq!(json[3]["conditions"]["headerContains"][0], "X-Spam: yes");
        assert_eq!(json[1]["sequence"], 2);
        let read: Vec<graph::Rule> =
            json.iter().enumerate().map(|(i, j)| graph::Rule { id: format!("r{i}"), json: j.clone() }).collect();
        let (state, ids) = graph_state(&read, &folders, &Default::default());
        assert_eq!(state.filters.rules, rules);
        assert_eq!(ids.len(), 6);
        // Microsoft 365 can't flag or refuse mail by rule.
        assert!(graph_rules(&[rule(Field::From, "a", Act::Star)], &folders).is_err());
        // Away replies: dates as days, the subject leading the text.
        let v = Vacation { on: true, subject: "Away".into(), body: "Back Monday".into(), from: Some("2026-12-20".into()), until: Some("2026-12-27".into()), ..Default::default() };
        let r = graph_replies(&v);
        assert_eq!(r.internal, "Away\n\nBack Monday");
        assert_eq!(date_of(r.start).as_deref(), Some("2026-12-20"));
        assert_eq!(date_of(r.end).as_deref(), Some("2026-12-27"));
        Ok(())
    }
}
