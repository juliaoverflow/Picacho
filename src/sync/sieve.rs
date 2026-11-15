//! Server-side filters and vacation replies: a ManageSieve client (RFC
//! 5804) and the Sieve script (RFC 5228, vacation RFC 5230, imap4flags RFC
//! 5232) that Picacho writes for its rules.
//!
//! Picacho keeps its rules in a script of its own, `picacho`, whose first
//! line carries them as JSON so they can be read back exactly; the Sieve
//! below it is generated from them. A script someone else made stays as it
//! is: where the server has `include`, Picacho's script runs it first;
//! otherwise turning Picacho's on means turning that one off, which the
//! settings say before saving.
use crate::sync::{sasl, wire::Wire};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};

/// ManageSieve's registered port.
pub const PORT: u16 = 4190;
/// The script Picacho writes.
pub const SCRIPT: &str = "picacho";
const MARKER: &str = "# Picacho filters: ";

/// Which part of a message a rule looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    #[default]
    From,
    To,
    Subject,
    /// To or Cc.
    Recipients,
    /// The List-Id header: mail from a mailing list.
    List,
}
impl Field {
    pub const ALL: [Self; 5] = [
        Self::From,
        Self::To,
        Self::Recipients,
        Self::Subject,
        Self::List,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::From => "From",
            Self::To => "To",
            Self::Recipients => "To or Cc",
            Self::Subject => "Subject",
            Self::List => "Mailing list",
        }
    }
}
/// What a rule does with matching mail.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    /// Into one of the account's folders, by its server name.
    #[default]
    Inbox,
    Folder(String),
    MarkRead,
    Star,
    /// Thrown away; it never arrives.
    Discard,
}
/// One filter: mail whose `field` contains `value` gets `act`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub value: String,
    pub act: Act,
    /// No later rule applies to mail this one matched.
    #[serde(default)]
    pub stop: bool,
}
/// An automatic reply while away.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vacation {
    pub on: bool,
    pub subject: String,
    pub body: String,
    /// Each sender gets the reply at most once in this many days.
    pub days: u32,
    /// First and last day it answers (YYYY-MM-DD), where the server can
    /// tell dates (Sieve `date`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}
/// Everything Picacho's script says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filters {
    pub rules: Vec<Rule>,
    pub vacation: Vacation,
    /// Another script run first (`include`), so turning Picacho's on
    /// doesn't turn it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
}
/// What the server has, as read when Settings opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub filters: Filters,
    /// Sieve extensions the server has (fileinto, vacation…).
    pub extensions: Vec<String>,
    /// The active script, if it isn't Picacho's.
    pub other_active: Option<String>,
    /// Picacho's script is the active one.
    pub active: bool,
}
impl State {
    pub fn has(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(extension))
    }
}

/// A Sieve string, quoted.
fn string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
    )
}

/// The Sieve script for `filters`, given the server's extensions and the
/// addresses the vacation reply answers for.
pub fn script(filters: &Filters, extensions: &[String], addresses: &[String]) -> Result<String> {
    let has = |e: &str| extensions.iter().any(|x| x.eq_ignore_ascii_case(e));
    let mut require: Vec<&str> = Vec::new();
    let mut body = String::new();
    if let Some(other) = &filters.include {
        ensure!(
            has("include"),
            "This server can't run another script from Picacho's."
        );
        require.push("include");
        body.push_str(&format!("include :personal {};\r\n", string(other)));
    }
    let vacation = &filters.vacation;
    if vacation.on {
        ensure!(has("vacation"), "This server can't send vacation replies.");
        ensure!(
            !vacation.body.trim().is_empty(),
            "Write the vacation reply's message."
        );
        require.push("vacation");
        let mut tests = Vec::new();
        for (date, op) in [(&vacation.from, "ge"), (&vacation.until, "le")] {
            if let Some(date) = date.as_deref().filter(|d| !d.trim().is_empty()) {
                ensure!(
                    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").is_ok(),
                    "Enter dates like 2026-12-24."
                );
                ensure!(
                    has("date") && has("relational"),
                    "This server can't limit vacation replies to dates."
                );
                tests.push(format!(
                    "currentdate :value {} \"date\" {}",
                    string(op),
                    string(date.trim())
                ));
            }
        }
        if !tests.is_empty() {
            require.extend(["date", "relational"]);
        }
        let mut reply = format!(":days {}", vacation.days.clamp(1, 30));
        if !vacation.subject.trim().is_empty() {
            reply.push_str(&format!(" :subject {}", string(vacation.subject.trim())));
        }
        if !addresses.is_empty() {
            let list: Vec<String> = addresses.iter().map(|a| string(a)).collect();
            reply.push_str(&format!(" :addresses [{}]", list.join(", ")));
        }
        let action = format!("vacation {reply} {};\r\n", string(&vacation.body));
        if tests.is_empty() {
            body.push_str(&action);
        } else {
            body.push_str(&format!(
                "if allof ({}) {{\r\n  {action}}}\r\n",
                tests.join(", ")
            ));
        }
    }
    for rule in &filters.rules {
        let value = rule.value.trim();
        ensure!(
            !value.is_empty(),
            "Each filter needs something to look for."
        );
        let test = match rule.field {
            Field::From => format!("address :contains \"from\" {}", string(value)),
            Field::To => format!("address :contains \"to\" {}", string(value)),
            Field::Recipients => format!("address :contains [\"to\", \"cc\"] {}", string(value)),
            Field::Subject => format!("header :contains \"subject\" {}", string(value)),
            Field::List => format!("header :contains \"list-id\" {}", string(value)),
        };
        let action = match &rule.act {
            Act::Inbox => "keep;".to_owned(),
            Act::Folder(name) => {
                ensure!(has("fileinto"), "This server can't file mail into folders.");
                require.push("fileinto");
                // Sieve names folders in UTF-8.
                format!("fileinto {};", string(&super::transport::decode_name(name)))
            }
            Act::MarkRead => {
                ensure!(
                    has("imap4flags"),
                    "This server can't mark mail read from a filter."
                );
                require.push("imap4flags");
                "addflag \"\\\\Seen\";".to_owned()
            }
            Act::Star => {
                ensure!(
                    has("imap4flags"),
                    "This server can't star mail from a filter."
                );
                require.push("imap4flags");
                "addflag \"\\\\Flagged\";".to_owned()
            }
            Act::Discard => "discard;".to_owned(),
        };
        body.push_str(&format!(
            "if {test} {{\r\n  {action}{}\r\n}}\r\n",
            if rule.stop { "\r\n  stop;" } else { "" }
        ));
    }
    require.sort_unstable();
    require.dedup();
    let mut out = format!("{MARKER}{}\r\n", serde_json::to_string(filters)?);
    out.push_str("# Made by Picacho. Changes made here are replaced when it saves.\r\n");
    if !require.is_empty() {
        let list: Vec<String> = require.iter().map(|r| string(r)).collect();
        out.push_str(&format!("require [{}];\r\n", list.join(", ")));
    }
    out.push_str(&body);
    Ok(out)
}
/// The rules a script of Picacho's carries, if it is one.
pub fn read_script(text: &str) -> Option<Filters> {
    let line = text.lines().next()?.strip_prefix(MARKER)?;
    serde_json::from_str(line.trim()).ok()
}

/// Where a ManageSieve server is: the incoming server's host, normally.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// An access token rather than a password.
    pub oauth: bool,
}

/// One ManageSieve reply: OK, NO or BYE, with its text.
struct Reply {
    ok: bool,
    text: String,
}

struct Session<S: Read + Write> {
    reader: BufReader<S>,
}
impl<S: Read + Write> Session<S> {
    fn line(&mut self) -> Result<String> {
        let mut line = Vec::new();
        (&mut self.reader)
            .take(65_537)
            .read_until(b'\n', &mut line)?;
        ensure!(!line.is_empty(), "The filter server closed the connection.");
        ensure!(
            line.len() <= 65_536,
            "The filter server's reply is too long."
        );
        Ok(String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    }
    /// A literal's bytes after its `{n}` line, then the rest of that line.
    fn literal(&mut self, len: usize) -> Result<String> {
        ensure!(len <= 1024 * 1024, "The filter server sent too much.");
        let mut data = vec![0; len];
        self.reader.read_exact(&mut data)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    }
    fn send(&mut self, data: &str) -> Result<()> {
        let stream = self.reader.get_mut();
        stream.write_all(data.as_bytes())?;
        stream.flush()?;
        Ok(())
    }
    /// Lines until OK/NO/BYE; the data lines before it (literals read in).
    fn response(&mut self) -> Result<(Vec<String>, Reply)> {
        let mut data = Vec::new();
        loop {
            let line = self.line()?;
            let upper = line.to_ascii_uppercase();
            for (word, ok) in [("OK", true), ("NO", false), ("BYE", false)] {
                if upper == word || upper.starts_with(&format!("{word} ")) {
                    let mut text = line[word.len()..].trim().to_owned();
                    if let Some(len) = literal_len(&text) {
                        text = self.literal(len)?;
                        self.line()?;
                    }
                    return Ok((data, Reply { ok, text }));
                }
            }
            if let Some(len) = literal_len(&line) {
                data.push(self.literal(len)?);
                let rest = self.line()?;
                if !rest.trim().is_empty() {
                    data.push(rest);
                }
            } else {
                data.push(line);
            }
        }
    }
    fn command(&mut self, line: &str) -> Result<(Vec<String>, Reply)> {
        self.send(&format!("{line}\r\n"))?;
        self.response()
    }
    fn expect(&mut self, line: &str, what: &str) -> Result<Vec<String>> {
        let (data, reply) = self.command(line)?;
        ensure!(
            reply.ok,
            "The filter server refused to {what}: {}",
            reply.text
        );
        Ok(data)
    }
}
/// `{12}` or `{12+}` at the end of a line: a literal of that many bytes.
fn literal_len(text: &str) -> Option<usize> {
    let inner = text.trim().strip_suffix('}')?;
    let start = inner.rfind('{')?;
    inner[start + 1..].trim_end_matches('+').parse().ok()
}
/// The words of a capability or LISTSCRIPTS line: quoted strings and atoms.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut text = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => text.extend(chars.next()),
                    '"' => break,
                    c => text.push(c),
                }
            }
            out.push(text);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            out.push(word);
        }
    }
    out
}
#[derive(Default)]
struct Capabilities {
    sasl: Vec<String>,
    sieve: Vec<String>,
    starttls: bool,
}
fn capabilities(lines: &[String]) -> Capabilities {
    let mut caps = Capabilities::default();
    for line in lines {
        let w = words(line);
        match (w.first().map(|s| s.to_ascii_uppercase()), w.get(1)) {
            (Some(k), Some(v)) if k == "SASL" => {
                caps.sasl = v.split_whitespace().map(str::to_ascii_uppercase).collect()
            }
            (Some(k), Some(v)) if k == "SIEVE" => {
                caps.sieve = v.split_whitespace().map(str::to_owned).collect()
            }
            (Some(k), _) if k == "STARTTLS" => caps.starttls = true,
            _ => {}
        }
    }
    caps
}

type Secured = Session<native_tls::TlsStream<Wire>>;
/// Connects, starts TLS (never in the clear) and signs in.
fn open(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<(Secured, Vec<String>)> {
    let socket = crate::sync::transport::tcp(&endpoint.host, endpoint.port).with_context(|| {
        format!(
            "{} offers no filter server (ManageSieve) on port {}",
            endpoint.host, endpoint.port
        )
    })?;
    let mut plain = Session {
        reader: BufReader::new(socket),
    };
    let (greeting, reply) = plain.response()?;
    ensure!(
        reply.ok,
        "The filter server refused the connection: {}",
        reply.text
    );
    ensure!(
        capabilities(&greeting).starttls,
        "{}'s filter server doesn't offer encryption (STARTTLS); Picacho won't sign in to it.",
        endpoint.host
    );
    plain.expect("STARTTLS", "start encryption")?;
    ensure!(
        plain.reader.buffer().is_empty(),
        "Unexpected data from the filter server before encryption started."
    );
    let stream = tls
        .connect(&endpoint.host, plain.reader.into_inner())
        .map_err(|e| anyhow::anyhow!("TLS with {}'s filter server failed: {e}", endpoint.host))?;
    let mut session = Session {
        reader: BufReader::new(stream),
    };
    // The capabilities again, now that they can be trusted (RFC 5804 2.2).
    let (lines, reply) = session.response()?;
    ensure!(
        reply.ok,
        "The filter server failed after STARTTLS: {}",
        reply.text
    );
    let caps = capabilities(&lines);
    let order: &[sasl::Mechanism] = if endpoint.oauth {
        &sasl::TOKEN_ORDER
    } else {
        &sasl::PASSWORD_ORDER
    };
    let mechanism = sasl::Mechanism::choose(order, caps.sasl.iter().map(String::as_str))
        .context("The filter server offers no sign-in method Picacho supports.")?;
    let mut exchange = sasl::Exchange::new(
        mechanism,
        sasl::Credentials {
            user: endpoint.user.clone(),
            secret: secret.to_owned(),
            host: endpoint.host.clone(),
            port: endpoint.port,
        },
    );
    let mut line = format!("AUTHENTICATE \"{}\"", mechanism.name());
    if let Some(first) = exchange.initial() {
        line.push_str(&format!(" \"{}\"", sasl::encode(&first)));
    }
    session.send(&format!("{line}\r\n"))?;
    loop {
        let reply = session.line()?;
        let upper = reply.to_ascii_uppercase();
        if upper.starts_with("OK") {
            // SCRAM's signature may come with the OK: `OK (SASL "…")`.
            let data = reply
                .split_once("SASL \"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(b64, _)| sasl::decode(b64))
                .transpose()?;
            exchange
                .finish(data.as_deref())
                .context(crate::manage::status::AccessFailure::Authentication)?;
            break;
        }
        if upper.starts_with("NO") || upper.starts_with("BYE") {
            return Err(
                anyhow::anyhow!("The filter server refused the sign-in: {reply}")
                    .context(crate::manage::status::AccessFailure::Authentication),
            );
        }
        let challenge = match literal_len(&reply) {
            Some(len) => {
                let text = session.literal(len)?;
                session.line()?;
                text
            }
            None => words(&reply).into_iter().next().unwrap_or_default(),
        };
        let answer = match exchange.step(&sasl::decode(&challenge)?) {
            Ok(answer) => answer,
            Err(e) => {
                // Cancels the exchange; the server answers NO.
                session.send("\"*\"\r\n")?;
                let _ = session.response();
                return Err(e.context(crate::manage::status::AccessFailure::Authentication));
            }
        };
        session.send(&format!("\"{}\"\r\n", sasl::encode(&answer)))?;
    }
    Ok((session, caps.sieve))
}

fn scripts(session: &mut Secured) -> Result<Vec<(String, bool)>> {
    let lines = session.expect("LISTSCRIPTS", "list filter scripts")?;
    Ok(lines
        .iter()
        .filter_map(|l| {
            let w = words(l);
            let name = w.first()?.clone();
            let active = w.get(1).is_some_and(|a| a.eq_ignore_ascii_case("ACTIVE"));
            Some((name, active))
        })
        .collect())
}

/// What the account's filter server has: Picacho's rules, and the script
/// that runs now if it isn't Picacho's.
pub fn load(endpoint: &Endpoint, secret: &str, tls: &native_tls::TlsConnector) -> Result<State> {
    let (mut session, extensions) = open(endpoint, secret, tls)?;
    let listed = scripts(&mut session)?;
    let mut state = State {
        extensions,
        ..State::default()
    };
    for (name, active) in &listed {
        if name == SCRIPT {
            state.active = *active;
            let data = session.expect(
                &format!("GETSCRIPT {}", string(SCRIPT)),
                "read Picacho's filters",
            )?;
            if let Some(filters) = data.first().and_then(|t| read_script(t)) {
                state.filters = filters;
            }
        } else if *active {
            state.other_active = Some(name.clone());
        }
    }
    let _ = session.command("LOGOUT");
    Ok(state)
}

/// Saves Picacho's rules as its script and makes it the active one.
/// Without rules or a vacation reply, the script is turned off instead
/// (and another script that was running before is left alone).
pub fn save(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
    filters: &Filters,
    addresses: &[String],
) -> Result<State> {
    let (mut session, extensions) = open(endpoint, secret, tls)?;
    let text = script(filters, &extensions, addresses)?;
    let listed = scripts(&mut session)?;
    let empty = filters.rules.is_empty() && !filters.vacation.on;
    if empty {
        if listed.iter().any(|(n, a)| n == SCRIPT && *a) {
            // Back to whichever script it included, else none.
            let next = filters.include.clone().unwrap_or_default();
            session.expect(
                &format!("SETACTIVE {}", string(&next)),
                "turn the filters off",
            )?;
        }
    } else {
        let literal = format!("{{{}+}}\r\n{text}", text.len());
        if extensions.is_empty() {
            bail!("The filter server didn't say what its filters can do.");
        }
        let (_, check) = session.command(&format!("CHECKSCRIPT {literal}"))?;
        if !check.ok && !check.text.to_ascii_lowercase().contains("unknown command") {
            bail!("The filter server rejected the filters: {}", check.text);
        }
        session.expect(
            &format!("PUTSCRIPT {} {literal}", string(SCRIPT)),
            "save the filters",
        )?;
        session.expect(
            &format!("SETACTIVE {}", string(SCRIPT)),
            "turn the filters on",
        )?;
    }
    let _ = session.command("LOGOUT");
    let other = listed
        .iter()
        .find(|(n, a)| *a && n != SCRIPT)
        .map(|(n, _)| n.clone())
        .filter(|_| empty);
    Ok(State {
        filters: filters.clone(),
        extensions,
        other_active: other,
        active: !empty,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_carry_their_rules_and_escape_text() {
        let filters = Filters {
            rules: vec![
                Rule {
                    field: Field::From,
                    value: "news@shop.example".into(),
                    act: Act::Folder("Caf&AOk-".into()),
                    stop: true,
                },
                Rule {
                    field: Field::Subject,
                    value: "say \"hi\"".into(),
                    act: Act::MarkRead,
                    stop: false,
                },
            ],
            vacation: Vacation {
                on: true,
                subject: "Away".into(),
                body: "Back on Monday.\nThanks!".into(),
                days: 7,
                from: None,
                until: Some("2026-12-31".into()),
            },
            include: None,
        };
        let ext: Vec<String> = ["fileinto", "vacation", "imap4flags", "date", "relational"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let text = script(&filters, &ext, &["me@example.com".into()]).unwrap();
        assert!(text.contains(
            "require [\"date\", \"fileinto\", \"imap4flags\", \"relational\", \"vacation\"];"
        ));
        assert!(text.contains("fileinto \"Café\";"));
        assert!(text.contains("header :contains \"subject\" \"say \\\"hi\\\"\""));
        assert!(text.contains("currentdate :value \"le\" \"date\" \"2026-12-31\""));
        assert!(text.contains(":addresses [\"me@example.com\"]"));
        assert!(text.contains("\"Back on Monday.\r\nThanks!\""));
        assert_eq!(read_script(&text), Some(filters.clone()));
        // A server without vacation can't take the reply.
        assert!(script(&filters, &ext[..1], &[]).is_err());
        assert_eq!(literal_len("{12+}"), Some(12));
        assert_eq!(literal_len("OK"), None);
        assert_eq!(words("\"picacho\" ACTIVE"), vec!["picacho", "ACTIVE"]);
    }
}
