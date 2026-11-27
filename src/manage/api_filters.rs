//! Server filters for accounts that use their provider's API: Picacho's
//! rules (`sieve::Rule`) as Gmail's filters and as Microsoft 365's inbox
//! rules, and back, with the away reply as each keeps it.
//!
//! A filter or rule set elsewhere that Picacho's rules can say (its tests,
//! all or any of them, and its actions) is shown and edited here like any
//! other; one that can't (tests Picacho has no words for, or combined in a
//! way it can't say) is left as it is, and counted (`State::kept_elsewhere`). What a provider can't
//! do with a rule is refused when it is saved, saying what.
use super::*;
use crate::sync::gmail::{self, Label};
use crate::sync::graph::{self, MailFolder};
use crate::sync::sieve::{Act, Field, Match, Rule, State, Test, Vacation};
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

/// A Gmail search word for one test (without its negation).
fn gmail_query(r: &Test) -> Result<String> {
    let v = r.value.trim();
    let quoted = if v.contains(' ') { format!("\"{v}\"") } else { v.to_owned() };
    let kb = || v.trim_end_matches(['K', 'k']).trim().parse::<u64>().map_err(|_| unsupported("Gmail", v));
    Ok(match r.field {
        Field::From => format!("from:{quoted}"),
        Field::To => format!("to:{quoted}"),
        Field::Recipients => format!("{{to:{quoted} cc:{quoted}}}"),
        Field::Subject => format!("subject:{quoted}"),
        Field::List => format!("list:{quoted}"),
        Field::Body => quoted,
        Field::LargerThan => format!("larger:{}K", kb()?),
        Field::SmallerThan => format!("smaller:{}K", kb()?),
        Field::Attachment => "has:attachment".to_owned(),
        Field::DeliveredTo => format!("deliveredto:{quoted}"),
        Field::Header => return Err(unsupported("Gmail", &t!("filters-header", header = r.header))),
    })
}
/// A test's search word with its negation: `-(…)`.
fn gmail_term(t: &Test) -> Result<String> {
    let q = gmail_query(t)?;
    Ok(if t.negate { format!("-({q})") } else { q })
}
/// A test, read back from a Gmail search word.
fn gmail_test(query: &str) -> Test {
    let unquote = |s: &str| s.trim().trim_matches('"').to_owned();
    let test = |field, value| Test { field, value, ..Test::default() };
    for (prefix, field) in [
        ("from:", Field::From),
        ("to:", Field::To),
        ("subject:", Field::Subject),
        ("list:", Field::List),
        ("deliveredto:", Field::DeliveredTo),
    ] {
        if let Some(v) = query.strip_prefix(prefix) {
            return test(field, unquote(v));
        }
    }
    if let Some(v) = query.strip_prefix("{to:") {
        let v = v.split(" cc:").next().unwrap_or(v);
        return test(Field::Recipients, unquote(v));
    }
    if let Some(v) = query.strip_prefix("larger:") {
        return test(Field::LargerThan, v.trim_end_matches(['K', 'k']).to_owned());
    }
    if let Some(v) = query.strip_prefix("smaller:") {
        return test(Field::SmallerThan, v.trim_end_matches(['K', 'k']).to_owned());
    }
    if query.eq_ignore_ascii_case("has:attachment") {
        return test(Field::Attachment, String::new());
    }
    test(Field::Body, unquote(query))
}
/// A Gmail search's words at its top level: split at spaces outside
/// quotes, braces and parentheses.
fn query_terms(query: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quoted, mut start) = (0i32, false, None);
    for (i, c) in query.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' | '{' if !quoted => depth += 1,
            ')' | '}' if !quoted => depth -= 1,
            c if c.is_whitespace() && !quoted && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&query[s..i]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(s) = start {
        out.push(&query[s..]);
    }
    out
}
/// Whether `term` is a Recipients test, as Picacho writes it:
/// `{to:x cc:x}`.
fn recipients_term(term: &str) -> bool {
    let Some(inner) = term.strip_prefix("{to:").and_then(|t| t.strip_suffix('}')) else { return false };
    inner.split_once(" cc:").is_some_and(|(a, b)| a == b)
}
/// One word of a search as a test: negated (`-(…)`, `-word`) or not.
fn gmail_term_test(term: &str) -> Option<Test> {
    if let Some(inner) = term.strip_prefix("-(").and_then(|t| t.strip_suffix(')')) {
        let mut t = gmail_term_test(inner)?;
        if t.negate {
            return None;
        }
        t.negate = true;
        return Some(t);
    }
    if let Some(inner) = term.strip_prefix('-').filter(|t| !t.is_empty()) {
        let mut t = gmail_term_test(inner)?;
        t.negate = !t.negate;
        return Some(t);
    }
    if term.starts_with('{') && !recipients_term(term) {
        return None;
    }
    Some(gmail_test(term))
}
/// A Gmail search as tests: all of its words, or, for one `{…}` group,
/// any of them (`true`).
fn gmail_query_tests(query: &str) -> Option<(Vec<Test>, bool)> {
    let terms = query_terms(query.trim());
    if let [one] = terms.as_slice()
        && one.starts_with('{')
        && !recipients_term(one)
    {
        let inner = one.strip_prefix('{')?.strip_suffix('}')?;
        let tests = query_terms(inner).into_iter().map(gmail_term_test).collect::<Option<Vec<_>>>()?;
        return Some((tests, true));
    }
    let tests = terms.into_iter().map(gmail_term_test).collect::<Option<Vec<_>>>()?;
    Some((tests, false))
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
        let tests = r.tests();
        if let Some(t) = tests.iter().find(|t| matches!(t.matching, Match::Matches | Match::Regex) && t.field.has_value()) {
            return Err(unsupported("Gmail", t.matching.label()));
        }
        let mut f = gmail::Filter::default();
        if r.any && tests.len() > 1 {
            // Any of them: one `{…}` group of Gmail's search words.
            let terms = tests.iter().map(gmail_term).collect::<Result<Vec<_>>>()?;
            f.query = format!("{{{}}}", terms.join(" "));
        } else {
            // All of them: Gmail's own criteria where free, the rest as
            // search words.
            let mut terms = Vec::new();
            for t in &tests {
                let v = t.value.trim().to_owned();
                match t.field {
                    _ if t.negate => terms.push(gmail_term(t)?),
                    Field::From if f.from.is_empty() => f.from = v,
                    Field::To if f.to.is_empty() => f.to = v,
                    Field::Subject if f.subject.is_empty() => f.subject = v,
                    Field::Attachment => f.has_attachment = true,
                    _ => terms.push(gmail_term(t)?),
                }
            }
            f.query = terms.join(" ");
        }
        for act in r.acts() {
            act_gmail(act, r.keep_copy, &mut label_id, &mut f)?;
        }
        out.push(f);
    }
    Ok(out)
}
fn act_gmail(act: &Act, keep_copy: bool, label_id: &mut impl FnMut(&str) -> String, f: &mut gmail::Filter) -> Result<()> {
    use gmail::label;
    let add = |labels: &mut Vec<String>, l: String| {
        if !labels.contains(&l) {
            labels.push(l);
        }
    };
    match act {
        // Kept in the inbox: never to Spam.
        Act::Inbox => add(&mut f.remove_labels, label::SPAM.into()),
        Act::Folder(name) => {
            if f.remove_labels.iter().any(|l| l == label::INBOX) {
                return Err(unsupported("Gmail", t!("filters-two-folders")));
            }
            add(&mut f.add_labels, label_id(name));
            add(&mut f.remove_labels, label::INBOX.into());
        }
        Act::MarkRead => add(&mut f.remove_labels, label::UNREAD.into()),
        Act::Star => add(&mut f.add_labels, label::STARRED.into()),
        Act::Discard => add(&mut f.add_labels, label::TRASH.into()),
        Act::Redirect(to) => {
            if !f.forward.is_empty() {
                return Err(unsupported("Gmail", t!("filters-two-redirects")));
            }
            f.forward = to.trim().to_owned();
            if !keep_copy {
                add(&mut f.add_labels, label::TRASH.into());
            }
        }
        Act::Reject(_) => return Err(unsupported("Gmail", t!("filters-act-refuse"))),
        Act::Label(name) => add(&mut f.add_labels, label_id(name)),
    }
    Ok(())
}

/// A Gmail filter as one of Picacho's rules, where one can say it.
fn gmail_rule(f: &gmail::Filter, labels: &[Label]) -> Option<Rule> {
    use gmail::label;
    let mut tests = Vec::new();
    let test = |field, value: &str| Test { field, value: value.to_owned(), ..Test::default() };
    if !f.from.is_empty() {
        tests.push(test(Field::From, &f.from));
    }
    if !f.to.is_empty() {
        tests.push(test(Field::To, &f.to));
    }
    if !f.subject.is_empty() {
        tests.push(test(Field::Subject, &f.subject));
    }
    if f.has_attachment {
        tests.push(test(Field::Attachment, ""));
    }
    let mut any = false;
    if !f.query.trim().is_empty() {
        let (found, either) = gmail_query_tests(&f.query)?;
        if either && !tests.is_empty() {
            return None;
        }
        any = either && found.len() > 1;
        tests.extend(found);
    }
    if tests.is_empty() {
        return None;
    }
    let name = |id: &str| labels.iter().find(|l| l.id == id && !l.system).map(|l| l.name.clone());
    let trash = f.add_labels.iter().any(|l| l == label::TRASH);
    let archived = f.remove_labels.iter().any(|l| l == label::INBOX);
    let redirect = !f.forward.is_empty();
    // In the order Picacho writes them.
    let mut acts = Vec::new();
    let mut custom = f.add_labels.iter().filter(|l| !matches!(l.as_str(), label::STARRED | label::TRASH));
    if archived {
        acts.push(Act::Folder(name(custom.next()?)?));
    }
    for l in custom {
        acts.push(Act::Label(name(l)?));
    }
    for r in &f.remove_labels {
        match r.as_str() {
            label::UNREAD => acts.push(Act::MarkRead),
            label::SPAM => acts.push(Act::Inbox),
            label::INBOX => {}
            _ => return None,
        }
    }
    if f.add_labels.iter().any(|l| l == label::STARRED) {
        acts.push(Act::Star);
    }
    // Sent on without a copy kept: what Trash means beside a forward.
    if trash && !redirect {
        acts.push(Act::Discard);
    }
    if redirect {
        acts.push(Act::Redirect(f.forward.clone()));
    }
    if acts.is_empty() {
        return None;
    }
    let mut rule = Rule { any, keep_copy: redirect && !trash, ..Rule::default() };
    rule.set_tests(tests);
    rule.any = any;
    rule.set_acts(acts);
    Some(rule)
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

/// One test as an inbox rule's predicate: its name and value.
fn graph_test(t: &Test) -> Result<(&'static str, Value)> {
    const SERVICE: &str = "Microsoft 365";
    if matches!(t.matching, Match::Matches | Match::Regex) && t.field.has_value() && !t.field.is_size() {
        return Err(unsupported(SERVICE, t.matching.label()));
    }
    let v = t.value.trim();
    let person = || json!([{"emailAddress": {"address": v}}]);
    let size = || v.trim_end_matches(['K', 'k']).trim().parse::<i64>().map_err(|_| unsupported(SERVICE, v));
    Ok(match (t.field, t.matching) {
        (Field::From, Match::Is) => ("fromAddresses", person()),
        (Field::From, _) => ("senderContains", json!([v])),
        (Field::To, Match::Is) => ("sentToAddresses", person()),
        (Field::To | Field::Recipients | Field::DeliveredTo, _) => ("recipientContains", json!([v])),
        (Field::Subject, _) => ("subjectContains", json!([v])),
        (Field::Body, _) => ("bodyContains", json!([v])),
        (Field::List, _) => ("headerContains", json!([format!("List-Id: {v}")])),
        (Field::Header, _) => ("headerContains", json!([format!("{}: {v}", t.header.trim())])),
        (Field::LargerThan, _) => ("withinSizeRange", json!({"minimumSize": size()?, "maximumSize": i32::MAX})),
        (Field::SmallerThan, _) => ("withinSizeRange", json!({"minimumSize": 0, "maximumSize": size()?})),
        (Field::Attachment, _) => ("hasAttachments", json!(true)),
    })
}

/// Adds predicate `key` to `into`: a list's values join the list there
/// (any of them), where `join` allows; a size range narrows the one
/// there. Anything else twice can't be said.
fn add_predicate(into: &mut serde_json::Map<String, Value>, key: &str, value: Value, join: bool) -> Result<()> {
    const SERVICE: &str = "Microsoft 365";
    let Some(there) = into.get_mut(key) else {
        into.insert(key.to_owned(), value);
        return Ok(());
    };
    match (there, value) {
        (Value::Array(list), Value::Array(more)) if join => list.extend(more),
        (range @ Value::Object(_), Value::Object(more)) if key == "withinSizeRange" && !join => {
            let min = range["minimumSize"].as_i64().unwrap_or(0).max(more["minimumSize"].as_i64().unwrap_or(0));
            let max = range["maximumSize"]
                .as_i64()
                .unwrap_or(i32::MAX.into())
                .min(more["maximumSize"].as_i64().unwrap_or(i32::MAX.into()));
            *range = json!({"minimumSize": min, "maximumSize": max});
        }
        (Value::Bool(_), Value::Bool(_)) => {}
        _ => return Err(unsupported(SERVICE, t!("filters-these-tests-together"))),
    }
    Ok(())
}

/// Picacho's rules as inbox rules, in order.
pub(super) fn graph_rules(rules: &[Rule], folders: &[MailFolder]) -> Result<Vec<Value>> {
    const SERVICE: &str = "Microsoft 365";
    let mut out = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let tests = r.tests();
        // Conditions all hold (a list's values: any of them); any one
        // exception stops the rule (so each is a test that mustn't hold).
        let mut conditions = serde_json::Map::new();
        let mut exceptions = serde_json::Map::new();
        let either = r.any && tests.len() > 1;
        for t in &tests {
            let (key, value) = graph_test(t)?;
            match (either, t.negate) {
                (false, false) => add_predicate(&mut conditions, key, value, false)?,
                (false, true) => add_predicate(&mut exceptions, key, value, true)?,
                // Any of several: only values of one list.
                (true, false) if conditions.is_empty() || conditions.contains_key(key) => {
                    if !value.is_array() {
                        return Err(unsupported(SERVICE, t!("filters-any-of-these-tests")));
                    }
                    add_predicate(&mut conditions, key, value, true)?
                }
                (true, _) => return Err(unsupported(SERVICE, t!("filters-any-of-these-tests"))),
            }
        }
        let folder = |name: &str| -> Result<String> {
            folders
                .iter()
                .find(|f| f.path == name || (name == "INBOX" && f.well_known.as_deref() == Some("inbox")))
                .map(|f| f.id.clone())
                .with_context(|| t!("manage-proton-the-folder-no-longer-exists-at", name = name))
        };
        let mut actions = serde_json::Map::new();
        let mut set = |key: &str, value: Value| -> Result<()> {
            if actions.contains_key(key) && !value.is_array() && actions[key] != value {
                return Err(unsupported(SERVICE, t!("filters-two-folders")));
            }
            add_predicate(&mut actions, key, value, true)
        };
        for act in r.acts() {
            match act {
                Act::Inbox => set("moveToFolder", folder("INBOX")?.into())?,
                Act::Folder(name) => set("moveToFolder", folder(name)?.into())?,
                Act::MarkRead => set("markAsRead", true.into())?,
                Act::Star => return Err(unsupported(SERVICE, t!("filters-star"))),
                Act::Discard => set("delete", true.into())?,
                Act::Redirect(to) => {
                    set("redirectTo", json!([{"emailAddress": {"address": to.trim()}}]))?;
                    if !r.keep_copy {
                        set("delete", true.into())?;
                    }
                }
                Act::Reject(_) => return Err(unsupported(SERVICE, t!("filters-act-refuse"))),
                Act::Label(name) => set("assignCategories", json!([name]))?,
            }
        }
        if r.stop {
            actions.insert("stopProcessingRules".into(), true.into());
        }
        let mut rule = json!({
            "displayName": format!("{GRAPH_NAME} {}", i + 1),
            "sequence": i + 1,
            "isEnabled": true,
            "actions": actions,
        });
        if !conditions.is_empty() {
            rule["conditions"] = conditions.into();
        }
        if !exceptions.is_empty() {
            rule["exceptions"] = exceptions.into();
        }
        out.push(rule);
    }
    Ok(out)
}

/// An inbox rule's predicates (`conditions` or `exceptions`) as tests:
/// each value of a list its own test. `None` for one Picacho can't say.
fn graph_tests(predicates: &Value) -> Option<Vec<Test>> {
    let mut out = Vec::new();
    let Some(map) = predicates.as_object() else {
        return predicates.is_null().then_some(out);
    };
    for (key, value) in map.iter().filter(|(_, v)| !v.is_null()) {
        let values = || -> Option<Vec<String>> {
            value
                .as_array()?
                .iter()
                .map(|x| x.as_str().map(str::to_owned).or_else(|| x["emailAddress"]["address"].as_str().map(str::to_owned)))
                .collect()
        };
        let test = |field, matching, value: String| Test { field, matching, value, ..Test::default() };
        match key.as_str() {
            "fromAddresses" => out.extend(values()?.into_iter().map(|v| test(Field::From, Match::Is, v))),
            "senderContains" => out.extend(values()?.into_iter().map(|v| test(Field::From, Match::Contains, v))),
            "sentToAddresses" => out.extend(values()?.into_iter().map(|v| test(Field::To, Match::Is, v))),
            "recipientContains" => out.extend(values()?.into_iter().map(|v| test(Field::Recipients, Match::Contains, v))),
            "subjectContains" => out.extend(values()?.into_iter().map(|v| test(Field::Subject, Match::Contains, v))),
            "bodyContains" => out.extend(values()?.into_iter().map(|v| test(Field::Body, Match::Contains, v))),
            "headerContains" => {
                for h in values()? {
                    out.push(match h.strip_prefix("List-Id: ") {
                        Some(list) => test(Field::List, Match::Contains, list.to_owned()),
                        None => {
                            let (name, v) = h.split_once(": ")?;
                            Test { header: name.to_owned(), ..test(Field::Header, Match::Contains, v.to_owned()) }
                        }
                    });
                }
            }
            "withinSizeRange" => {
                let min = value["minimumSize"].as_i64().unwrap_or(0);
                let max = value["maximumSize"].as_i64().unwrap_or(i32::MAX.into());
                if min > 0 {
                    out.push(test(Field::LargerThan, Match::Contains, min.to_string()));
                }
                if max < i64::from(i32::MAX) {
                    out.push(test(Field::SmallerThan, Match::Contains, max.to_string()));
                }
            }
            "hasAttachments" if value.as_bool() == Some(true) => out.push(test(Field::Attachment, Match::Contains, String::new())),
            "hasAttachments" => {}
            _ => return None,
        }
    }
    Some(out)
}

/// An inbox rule as one of Picacho's rules, where one can say it.
fn graph_rule(rule: &Value, folders: &[MailFolder]) -> Option<Rule> {
    let conditions = graph_tests(&rule["conditions"])?;
    let mut exceptions = graph_tests(&rule["exceptions"])?;
    // Several values of one list in the conditions are any of them: a
    // rule of that list alone, or one Picacho can't say.
    let keys: Vec<&Value> = rule["conditions"].as_object().map(|o| o.values().filter(|v| !v.is_null()).collect()).unwrap_or_default();
    let several = keys.iter().any(|v| v.as_array().is_some_and(|a| a.len() > 1));
    let any = several && keys.len() == 1;
    if (several && !any) || (any && !exceptions.is_empty()) {
        return None;
    }
    for e in &mut exceptions {
        e.negate = true;
    }
    let tests: Vec<Test> = conditions.into_iter().chain(exceptions).collect();
    if tests.is_empty() {
        return None;
    }
    let actions = rule["actions"].as_object()?;
    let stop = actions.get("stopProcessingRules").and_then(Value::as_bool).unwrap_or(false);
    let on = |key: &str| actions.get(key).filter(|v| !v.is_null() && **v != json!(false));
    if actions.iter().any(|(k, v)| {
        !v.is_null()
            && *v != json!(false)
            && !matches!(k.as_str(), "stopProcessingRules" | "moveToFolder" | "markAsRead" | "delete" | "redirectTo" | "assignCategories")
    }) {
        return None;
    }
    let strings = |v: &Value| -> Option<Vec<String>> {
        v.as_array()?
            .iter()
            .map(|x| x.as_str().map(str::to_owned).or_else(|| x["emailAddress"]["address"].as_str().map(str::to_owned)))
            .collect()
    };
    // In the order Picacho writes them.
    let mut acts = Vec::new();
    if let Some(id) = on("moveToFolder") {
        let f = folders.iter().find(|f| Some(f.id.as_str()) == id.as_str())?;
        acts.push(if f.well_known.as_deref() == Some("inbox") { Act::Inbox } else { Act::Folder(f.path.clone()) });
    }
    if on("markAsRead").is_some() {
        acts.push(Act::MarkRead);
    }
    for label in on("assignCategories").map(strings).unwrap_or(Some(Vec::new()))? {
        acts.push(Act::Label(label));
    }
    let redirects = on("redirectTo").map(strings).unwrap_or(Some(Vec::new()))?;
    // Deleted beside a redirect: sent on without a copy kept.
    let deleted = on("delete").is_some();
    if deleted && redirects.is_empty() {
        acts.push(Act::Discard);
    }
    acts.extend(redirects.iter().cloned().map(Act::Redirect));
    if acts.is_empty() {
        return None;
    }
    let mut out = Rule { stop, keep_copy: !redirects.is_empty() && !deleted, ..Rule::default() };
    out.set_tests(tests);
    out.any = any;
    out.set_acts(acts);
    Some(out)
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
        // Several tests at once: all of them.
        let both = gmail::Filter { from: "a@example.com".into(), subject: "x".into(), add_labels: vec!["STARRED".into()], ..Default::default() };
        let state = gmail_state(&[both], &labels, &Default::default());
        assert_eq!(state.kept_elsewhere, 0);
        assert_eq!(state.filters.rules[0].tests().len(), 2);
        // What Picacho has no words for (a label that isn't there): left as it is.
        let unknown = gmail::Filter { from: "a@example.com".into(), add_labels: vec!["Label_gone".into()], ..Default::default() };
        assert_eq!(gmail_state(&[unknown], &labels, &Default::default()).kept_elsewhere, 1);
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

    fn test(field: Field, value: &str) -> Test {
        Test { field, value: value.into(), ..Default::default() }
    }

    #[test]
    fn several_tests_and_actions_as_gmail_filters() -> Result<()> {
        let labels = vec![Label { id: "Label_1".into(), name: "Receipts".into(), system: false }];
        // All of: Gmail's own criteria where they're free, the rest as words.
        let mut all = rule(Field::From, "shop@example.com", Act::Folder("Receipts".into()));
        all.more = vec![
            test(Field::Subject, "invoice"),
            Test { negate: true, ..test(Field::Body, "draft") },
            test(Field::Attachment, ""),
            test(Field::SmallerThan, "2000"),
        ];
        all.also = vec![Act::MarkRead, Act::Star];
        // Any of: one `{…}` group.
        let mut any = rule(Field::From, "a@example.com", Act::Label("Receipts".into()));
        any.more = vec![test(Field::Subject, "receipt")];
        any.any = true;
        let filters = gmail_filters(&[all.clone(), any.clone()], &labels, &mut Vec::new())?;
        assert_eq!((filters[0].from.as_str(), filters[0].subject.as_str()), ("shop@example.com", "invoice"));
        assert!(filters[0].has_attachment);
        assert_eq!(filters[0].query, "-(draft) smaller:2000K");
        assert!(filters[0].remove_labels.contains(&"UNREAD".into()) && filters[0].add_labels.contains(&"STARRED".into()));
        assert_eq!(filters[1].query, "{from:a@example.com subject:receipt}");
        // Read back: the same tests (in Gmail's order) and actions.
        let state = gmail_state(&filters, &labels, &gmail::Vacation::default());
        assert_eq!(state.kept_elsewhere, 0);
        let back = &state.filters.rules[0];
        assert_eq!(back.tests().len(), 5);
        assert!(back.tests().contains(&Test { negate: true, ..test(Field::Body, "draft") }));
        assert_eq!(back.acts().cloned().collect::<Vec<_>>(), [Act::Folder("Receipts".into()), Act::MarkRead, Act::Star]);
        assert_eq!(state.filters.rules[1], any);
        // Two folders can't be.
        let mut two = rule(Field::From, "x", Act::Folder("Receipts".into()));
        two.also = vec![Act::Folder("Other".into())];
        assert!(gmail_filters(&[two], &labels, &mut Vec::new()).is_err());
        // Recipients stays one test, not an "any".
        assert_eq!(gmail_query_tests("{to:t@x.example cc:t@x.example}"), Some((vec![test(Field::Recipients, "t@x.example")], false)));
        Ok(())
    }

    #[test]
    fn several_tests_and_actions_as_inbox_rules() -> Result<()> {
        let folders = vec![
            MailFolder { id: "f-in".into(), name: "Inbox".into(), path: "Inbox".into(), well_known: Some("inbox".into()), ..Default::default() },
            MailFolder { id: "f-r".into(), name: "Receipts".into(), path: "Inbox/Receipts".into(), depth: 1, ..Default::default() },
        ];
        let mut all = rule(Field::From, "shop", Act::Folder("Inbox/Receipts".into()));
        all.more = vec![
            test(Field::Subject, "invoice"),
            test(Field::Attachment, ""),
            test(Field::LargerThan, "10"),
            test(Field::SmallerThan, "900"),
            Test { negate: true, ..test(Field::Body, "draft") },
        ];
        all.also = vec![Act::MarkRead, Act::Label("Finance".into())];
        let json = graph_rules(std::slice::from_ref(&all), &folders)?;
        let c = &json[0]["conditions"];
        assert_eq!(c["senderContains"][0], "shop");
        assert_eq!(c["hasAttachments"], true);
        assert_eq!((c["withinSizeRange"]["minimumSize"].as_i64(), c["withinSizeRange"]["maximumSize"].as_i64()), (Some(10), Some(900)));
        assert_eq!(json[0]["exceptions"]["bodyContains"][0], "draft");
        assert_eq!(json[0]["actions"]["markAsRead"], true);
        assert_eq!(json[0]["actions"]["assignCategories"][0], "Finance");
        let read = vec![graph::Rule { id: "r".into(), json: json[0].clone() }];
        let (state, _) = graph_state(&read, &folders, &Default::default());
        let back = &state.filters.rules[0];
        assert_eq!(back.tests().len(), 6);
        assert_eq!(back.acts().cloned().collect::<Vec<_>>(), [Act::Folder("Inbox/Receipts".into()), Act::MarkRead, Act::Label("Finance".into())]);
        // Any of one kind's values: one list.
        let mut any = rule(Field::Subject, "invoice", Act::MarkRead);
        any.more = vec![test(Field::Subject, "receipt")];
        any.any = true;
        let json = graph_rules(std::slice::from_ref(&any), &folders)?;
        assert_eq!(json[0]["conditions"]["subjectContains"], json!(["invoice", "receipt"]));
        let (state, _) = graph_state(&[graph::Rule { id: "r".into(), json: json[0].clone() }], &folders, &Default::default());
        assert_eq!(state.filters.rules[0], any);
        // Any of different kinds can't be said; nor two of one kind, all.
        any.more = vec![test(Field::Body, "receipt")];
        assert!(graph_rules(std::slice::from_ref(&any), &folders).is_err());
        any.any = false;
        any.more = vec![test(Field::Subject, "receipt")];
        assert!(graph_rules(&[any], &folders).is_err());
        Ok(())
    }
}
