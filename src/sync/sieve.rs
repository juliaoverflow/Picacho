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
    /// The message's text (Sieve `body`, RFC 5173).
    Body,
    /// The message's size in kilobytes, over the value (Sieve `size`).
    LargerThan,
    /// A header named in the rule's `header` (RFC 5228 §5.7).
    Header,
    /// The address mail was delivered to (Sieve `envelope "to"`), which
    /// for mail sent as Bcc or by a list differs from To.
    DeliveredTo,
}
impl Field {
    pub const ALL: [Self; 9] = [
        Self::From,
        Self::To,
        Self::Recipients,
        Self::Subject,
        Self::List,
        Self::Body,
        Self::LargerThan,
        Self::Header,
        Self::DeliveredTo,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::From => t!("sync-sieve-from"),
            Self::To => t!("sync-sieve-to"),
            Self::Recipients => t!("sync-sieve-to-or-cc"),
            Self::Subject => t!("sync-sieve-subject"),
            Self::List => t!("sync-sieve-mailing-list"),
            Self::Body => t!("sync-sieve-text"),
            Self::LargerThan => t!("sync-sieve-larger-than-kb"),
            Self::Header => t!("sync-sieve-header"),
            Self::DeliveredTo => t!("sync-sieve-delivered-to"),
        }
    }
    /// The Sieve extension it needs, if any.
    pub fn needs(self) -> Option<&'static str> {
        match self {
            Self::Body => Some("body"),
            Self::DeliveredTo => Some("envelope"),
            _ => None,
        }
    }
}
/// How a rule's value is compared (RFC 5228 §2.7.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    #[default]
    Contains,
    Is,
    /// With `*` and `?` wildcards.
    Matches,
    /// A regular expression (the `regex` extension).
    Regex,
}
impl Match {
    pub const ALL: [Self; 4] = [Self::Contains, Self::Is, Self::Matches, Self::Regex];
    pub fn label(self) -> &'static str {
        match self {
            Self::Contains => t!("sync-sieve-contains"),
            Self::Is => t!("sync-sieve-is"),
            Self::Matches => t!("sync-sieve-matches"),
            Self::Regex => t!("sync-sieve-matches-the-pattern"),
        }
    }
    /// Its name in messages that choose words by it (`filters-rule`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Is => "is",
            Self::Matches => "matches",
            Self::Regex => "pattern",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Self::Contains => ":contains",
            Self::Is => ":is",
            Self::Matches => ":matches",
            Self::Regex => ":regex",
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
    /// Sent on to another address (`redirect`; with `copy`, a copy goes
    /// and the message is kept too).
    Redirect(String),
    /// Refused, with this reason sent back (`reject`, RFC 5429).
    Reject(String),
    /// Given a label (an IMAP keyword, imap4flags).
    Label(String),
}
/// One filter: mail whose `field` contains `value` gets `act`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub value: String,
    pub act: Act,
    /// How `value` is compared.
    #[serde(default)]
    pub matching: Match,
    /// The rule applies when the test doesn't match (`not`).
    #[serde(default)]
    pub negate: bool,
    /// For `Field::Header`: the header's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header: String,
    /// With `Act::Redirect`, the message is also kept (`:copy`, RFC 3894).
    #[serde(default)]
    pub keep_copy: bool,
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
    /// The time zone `from` and `until` are days in (RFC 5260 §4.3
    /// `:zone`, as `+0100`); the server's own where empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
    /// The reply's From (RFC 5230 §4.4 `:from`); the server picks where
    /// empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender: String,
    /// Each sender gets the reply at most once in this many seconds, where
    /// set (RFC 6131 `:seconds`, the `vacation-seconds` extension) rather
    /// than in `days`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
    /// `body` is a whole MIME entity, headers and all (RFC 5230 §5.8
    /// `:mime`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mime: bool,
    /// A copy of each reply goes to this folder (RFC 8580 `:fcc`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fcc: String,
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
    /// The server has no Sieve, only an automatic reply while away (JMAP's
    /// VacationResponse): no rules, and no choice of how often it answers.
    pub vacation_only: bool,
    /// The other app's active script as read (see `sieve_parse`): its rules
    /// Picacho can take over, and how many parts it couldn't read as
    /// rules. `None` where there's no such script or it can't be read.
    pub other_rules: Option<super::sieve_parse::Read>,
    /// Filters the server has that Picacho's rules can't say (Gmail's
    /// filters, Microsoft 365's inbox rules made elsewhere with several
    /// tests or actions): left as they are when Picacho's are saved.
    pub kept_elsewhere: usize,
}
impl State {
    pub fn has(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(extension))
    }
}

/// A string in a ManageSieve command (a script's name): quoted, which
/// RFC 5804 allows no line break or NUL in. One with them (a name a server
/// listed) is refused, so it can't end the command and start another.
fn protocol_string(text: &str) -> Result<String> {
    anyhow::ensure!(
        !text.contains(['\r', '\n', '\0']),
        t!("sync-sieve-the-server-named-a-filter-script")
    );
    Ok(format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
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
            t!("sync-sieve-this-server-cant-run-another-script")
        );
        require.push("include");
        // :optional (RFC 6609 §3.2): the other script being deleted must
        // not make every message fail delivery.
        body.push_str(&format!("include :personal :optional {};\r\n", string(other)));
    }
    let vacation = &filters.vacation;
    if vacation.on {
        ensure!(has("vacation"), t!("sync-sieve-this-server-cant-send-vacation-replies"));
        ensure!(
            !vacation.body.trim().is_empty(),
            t!("sync-sieve-write-the-vacation-replys-message")
        );
        require.push("vacation");
        let mut tests = Vec::new();
        for (date, op) in [(&vacation.from, "ge"), (&vacation.until, "le")] {
            if let Some(date) = date.as_deref().filter(|d| !d.trim().is_empty()) {
                ensure!(
                    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").is_ok(),
                    t!("sync-sieve-enter-dates-like-2026-12-24")
                );
                ensure!(
                    has("date") && has("relational"),
                    t!("sync-sieve-this-server-cant-limit-vacation-replies")
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
            // RFC 5260 §4.3: :zone "+hhmm" says whose day it is.
            let zone = vacation.zone.trim();
            if !zone.is_empty() {
                let valid = zone.len() == 5
                    && zone.starts_with(['+', '-'])
                    && zone[1..].bytes().all(|b| b.is_ascii_digit());
                ensure!(valid, t!("sync-sieve-enter-the-time-zone-like-0100"));
                for t in &mut tests {
                    *t = t.replacen("currentdate ", &format!("currentdate :zone {} ", string(zone)), 1);
                }
            }
        }
        // A fixed :handle (RFC 5230 §4.2): editing the reply's text
        // doesn't make it go again to everyone already answered.
        let period = match vacation.seconds {
            // RFC 6131 §2: :seconds in place of :days.
            Some(s) => {
                ensure!(has("vacation-seconds"), t!("sync-sieve-this-server-cant-send-vacation-replies-2"));
                require.push("vacation-seconds");
                format!(":seconds {s}")
            }
            None => format!(":days {}", vacation.days.clamp(1, 30)),
        };
        let mut reply = format!("{period} :handle \"picacho-vacation\"");
        if !vacation.subject.trim().is_empty() {
            reply.push_str(&format!(" :subject {}", string(vacation.subject.trim())));
        }
        let sender = vacation.sender.trim();
        if !sender.is_empty() {
            ensure!(
                sender.contains('@') && !sender.contains(['\r', '\n']),
                t!("sync-sieve-enter-the-address-the-vacation-reply")
            );
            reply.push_str(&format!(" :from {}", string(sender)));
        }
        if !vacation.fcc.trim().is_empty() {
            ensure!(has("fcc"), t!("sync-sieve-this-server-cant-keep-copies-of"));
            require.push("fcc");
            reply.push_str(&format!(" :fcc {}", string(&super::transport::decode_name(vacation.fcc.trim()))));
        }
        if vacation.mime {
            reply.push_str(" :mime");
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
            t!("sync-sieve-each-filter-needs-something-to-look")
        );
        let m = rule.matching.tag();
        if rule.matching == Match::Regex {
            ensure!(has("regex"), t!("sync-sieve-this-server-cant-match-patterns-in"));
            require.push("regex");
        }
        if let Some(ext) = rule.field.needs() {
            ensure!(has(ext), t!("sync-sieve-this-server-cant-filter-on", label = rule.field.label(), ext = ext));
            require.push(ext);
        }
        let test = match rule.field {
            Field::From => format!("address {m} \"from\" {}", string(value)),
            Field::To => format!("address {m} \"to\" {}", string(value)),
            Field::Recipients => format!("address {m} [\"to\", \"cc\"] {}", string(value)),
            Field::Subject => format!("header {m} \"subject\" {}", string(value)),
            Field::List => format!("header {m} \"list-id\" {}", string(value)),
            Field::Body => format!("body :text {m} {}", string(value)),
            Field::LargerThan => {
                let kb: u64 = value
                    .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .with_context(|| t!("sync-sieve-enter-a-size-in-kilobytes-such"))?;
                format!("size :over {kb}K")
            }
            Field::Header => {
                let name = rule.header.trim();
                ensure!(
                    !name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic() && b != b':'),
                    t!("sync-sieve-name-the-header-the-filter-looks")
                );
                format!("header {m} {} {}", string(name), string(value))
            }
            Field::DeliveredTo => format!("envelope {m} \"to\" {}", string(value)),
        };
        let test = if rule.negate { format!("not {test}") } else { test };
        let action = match &rule.act {
            Act::Inbox => "keep;".to_owned(),
            Act::Folder(name) => {
                ensure!(has("fileinto"), t!("sync-sieve-this-server-cant-file-mail-into"));
                require.push("fileinto");
                // A standard folder by its role (`\\Trash`): the server
                // finds it by its special use (RFC 8579), whatever it's
                // called; its usual name if the server can't.
                if let Some(role) = name.strip_prefix('\\') {
                    let plain = match role {
                        "Junk" => "Junk",
                        "Trash" => "Trash",
                        "Archive" => "Archive",
                        "Sent" => "Sent",
                        other => bail!(t!("sync-sieve-isnt-a-folder-filters-can-use", folder = other)),
                    };
                    if has("special-use") {
                        require.push("special-use");
                        format!("fileinto :specialuse {} {};", string(name), string(plain))
                    } else {
                        format!("fileinto {};", string(plain))
                    }
                } else {
                    // `mailbox`'s :create (RFC 5490): a folder deleted
                    // since isn't a reason to lose the mail.
                    let create = if has("mailbox") {
                        require.push("mailbox");
                        ":create "
                    } else {
                        ""
                    };
                    // Sieve names folders in UTF-8.
                    format!("fileinto {create}{};", string(&super::transport::decode_name(name)))
                }
            }
            Act::MarkRead => {
                ensure!(
                    has("imap4flags"),
                    t!("sync-sieve-this-server-cant-mark-mail-read")
                );
                require.push("imap4flags");
                "addflag \"\\\\Seen\";".to_owned()
            }
            Act::Star => {
                ensure!(
                    has("imap4flags"),
                    t!("sync-sieve-this-server-cant-star-mail-from")
                );
                require.push("imap4flags");
                "addflag \"\\\\Flagged\";".to_owned()
            }
            Act::Discard => "discard;".to_owned(),
            Act::Redirect(to) => {
                let to = to.trim();
                ensure!(
                    to.contains('@') && !to.contains(|c: char| c.is_whitespace() || c == ','),
                    t!("sync-sieve-enter-the-one-address-mail-is")
                );
                if rule.keep_copy {
                    ensure!(has("copy"), t!("sync-sieve-this-server-cant-send-a-copy"));
                    require.push("copy");
                    format!("redirect :copy {};", string(to))
                } else {
                    format!("redirect {};", string(to))
                }
            }
            Act::Reject(reason) => {
                ensure!(has("reject"), t!("sync-sieve-this-server-cant-refuse-mail-from"));
                require.push("reject");
                format!("reject {};", string(reason.trim()))
            }
            Act::Label(label) => {
                ensure!(has("imap4flags"), t!("sync-sieve-this-server-cant-label-mail-from"));
                require.push("imap4flags");
                let keyword = super::transport::keyword(label)?;
                format!("addflag {};", string(&keyword))
            }
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
    /// Kerberos (SASL GSSAPI) rather than a password.
    pub kerberos: bool,
}

/// One ManageSieve reply: OK, NO or BYE, with its text and its response
/// code (RFC 5804 §1.3), uppercase: `QUOTA`, `QUOTA/MAXSIZE`, `TRYLATER`,
/// `REFERRAL`, `WARNINGS`, `NONEXISTENT`, `ALREADYEXISTS`, `ACTIVE`…
struct Reply {
    ok: bool,
    text: String,
    code: Option<String>,
    /// REFERRAL's `sieve://host[:port]` URL.
    referral: Option<String>,
}
impl Reply {
    fn new(ok: bool, rest: String) -> Self {
        let (mut code, mut referral) = (None, None);
        let mut text = rest.trim().to_owned();
        if let Some(inner) = text.strip_prefix('(')
            && let Some(end) = inner.find(')')
        {
            let w = words(&inner[..end]);
            code = w.first().map(|c| c.to_ascii_uppercase());
            if code.as_deref() == Some("REFERRAL") {
                referral = w.get(1).cloned();
            }
            text = inner[end + 1..].trim().to_owned();
        }
        // The human-readable text is a quoted string (or a literal, read
        // in by `response`).
        if text.starts_with('"') {
            text = words(&text).into_iter().next().unwrap_or_default();
        }
        Self { ok, text, code, referral }
    }
    fn is(&self, code: &str) -> bool {
        self.code
            .as_deref()
            .is_some_and(|c| c == code || c.starts_with(&format!("{code}/")))
    }
    /// What went wrong, in words, for refusals with a known code.
    fn explain(&self) -> String {
        let why = if self.is("QUOTA") {
            t!("sync-sieve-the-account-has-no-room-for")
        } else if self.is("TRYLATER") {
            t!("sync-sieve-the-server-is-busy-try-again")
        } else if self.is("NONEXISTENT") {
            t!("sync-sieve-the-script-doesnt-exist")
        } else if self.is("ALREADYEXISTS") {
            t!("sync-sieve-a-script-by-that-name-already")
        } else if self.is("ACTIVE") {
            t!("sync-sieve-the-script-is-in-use")
        } else if self.is("REFERRAL") {
            return match self.referral.as_deref() {
                Some(server) => t!("sync-sieve-the-accounts-filters-are-kept-on", server = server),
                None => t!("sync-sieve-the-accounts-filters-are-kept-on-unnamed").to_owned(),
            };
        } else {
            return self.text.clone();
        };
        if self.text.is_empty() {
            why.to_owned()
        } else {
            format!("{why} ({})", self.text)
        }
    }
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
        ensure!(!line.is_empty(), t!("sync-sieve-the-filter-server-closed-the-connection"));
        ensure!(
            line.len() <= 65_536,
            t!("sync-sieve-the-filter-servers-reply-is-too")
        );
        Ok(String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    }
    /// A literal's bytes after its `{n}` line, then the rest of that line.
    fn literal(&mut self, len: usize) -> Result<String> {
        ensure!(len <= 1024 * 1024, t!("sync-sieve-the-filter-server-sent-too-much"));
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
    /// Lines until OK/NO/BYE; the data lines before it, each as its words
    /// (quoted strings, atoms and literals alike, so a literal stays on
    /// its own line: `"SIEVE" {n}` is one line of two words).
    fn response(&mut self) -> Result<(Vec<Vec<String>>, Reply)> {
        let mut data = Vec::new();
        loop {
            let line = self.line()?;
            let upper = line.to_ascii_uppercase();
            for (word, ok) in [("OK", true), ("NO", false), ("BYE", false)] {
                if upper == word || upper.starts_with(&format!("{word} ")) {
                    let mut text = line[word.len()..].trim().to_owned();
                    if let Some(len) = literal_len(&text) {
                        // The code stays; the literal is its text.
                        let head = text[..text.rfind('{').unwrap_or(0)].trim().to_owned();
                        let body = self.literal(len)?;
                        self.line()?;
                        text = format!("{head} \"{}\"", body.replace('\\', "\\\\").replace('"', "\\\""));
                    }
                    return Ok((data, Reply::new(ok, text)));
                }
            }
            // A line may hold several literals: each `{n}` ends a piece.
            let mut words_of_line = Vec::new();
            let mut piece = line;
            while let Some(len) = literal_len(&piece) {
                let head = piece[..piece.rfind('{').unwrap_or(0)].to_owned();
                words_of_line.extend(words(&head));
                words_of_line.push(self.literal(len)?);
                piece = self.line()?;
            }
            words_of_line.extend(words(&piece));
            if !words_of_line.is_empty() {
                data.push(words_of_line);
            }
        }
    }
    fn command(&mut self, line: &str) -> Result<(Vec<Vec<String>>, Reply)> {
        self.send(&format!("{line}\r\n"))?;
        self.response()
    }
    /// `what`: the step, one of the keys `sync-sieve-the-filter-server-refused-to`
    /// selects on.
    fn expect(&mut self, line: &str, what: &'static str) -> Result<Vec<Vec<String>>> {
        let (data, reply) = self.command(line)?;
        ensure!(
            reply.ok,
            t!("sync-sieve-the-filter-server-refused-to", what = what, explain = reply.explain())
        );
        Ok(data)
    }
}
/// A base64 SASL payload as a ManageSieve string: quoted where short, a
/// literal past the 1024 octets RFC 5804 §4 allows a quoted string.
fn sasl_string(b64: &str) -> String {
    if b64.len() <= 1000 {
        format!("\"{b64}\"")
    } else {
        format!("{{{}+}}\r\n{b64}", b64.len())
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
    /// `VERSION "1.0"`: RFC 5804 itself, so CHECKSCRIPT, RENAMESCRIPT and
    /// NOOP are there (servers from before it lack them).
    rfc5804: bool,
}
fn capabilities(lines: &[Vec<String>]) -> Capabilities {
    let mut caps = Capabilities::default();
    for w in lines {
        match (w.first().map(|s| s.to_ascii_uppercase()), w.get(1)) {
            (Some(k), Some(v)) if k == "SASL" => {
                caps.sasl = v.split_whitespace().map(str::to_ascii_uppercase).collect()
            }
            (Some(k), Some(v)) if k == "SIEVE" => {
                caps.sieve = v.split_whitespace().map(str::to_owned).collect()
            }
            (Some(k), _) if k == "STARTTLS" => caps.starttls = true,
            (Some(k), Some(_)) if k == "VERSION" => caps.rfc5804 = true,
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
) -> Result<(Secured, Opened)> {
    let socket = crate::sync::transport::tcp(&endpoint.host, endpoint.port).with_context(|| {
        t!("sync-sieve-offers-no-filter-server-managesieve-on", host = endpoint.host, port = endpoint.port)
    })?;
    let mut plain = Session {
        reader: BufReader::new(socket),
    };
    let (greeting, reply) = plain.response()?;
    ensure!(
        reply.ok,
        t!("sync-sieve-the-filter-server-refused-the-connection", text = reply.text)
    );
    ensure!(
        capabilities(&greeting).starttls,
        t!("sync-sieve-s-filter-server-doesnt-offer-encryption", host = endpoint.host)
    );
    plain.expect("STARTTLS", "start-encryption")?;
    ensure!(
        plain.reader.buffer().is_empty(),
        t!("sync-sieve-unexpected-data-from-the-filter-server")
    );
    let stream = tls
        .connect(&endpoint.host, plain.reader.into_inner())
        .map_err(|e| anyhow::anyhow!(t!("sync-sieve-tls-with-s-filter-server-failed", host = endpoint.host, error = e.to_string())))?;
    let binding = sasl::binding(&stream);
    let mut session = Session {
        reader: BufReader::new(stream),
    };
    // The capabilities again, now that they can be trusted (RFC 5804 2.2).
    let (lines, reply) = session.response()?;
    ensure!(
        reply.ok,
        t!("sync-sieve-the-filter-server-failed-after-starttls", text = reply.text)
    );
    let caps = capabilities(&lines);
    let order: &[sasl::Mechanism] = if endpoint.kerberos {
        &sasl::KERBEROS_ORDER
    } else if endpoint.oauth {
        &sasl::TOKEN_ORDER
    } else {
        &sasl::PASSWORD_ORDER
    };
    let mechanism =
        sasl::Mechanism::choose_bound(order, caps.sasl.iter().map(String::as_str), !binding.is_empty())
        .with_context(|| t!("sync-sieve-the-filter-server-offers-no-sign"))?;
    let mut exchange = sasl::Exchange::new(
        mechanism,
        sasl::Credentials {
            user: endpoint.user.clone(),
            secret: secret.to_owned(),
            host: endpoint.host.clone(),
            port: endpoint.port,
            binding,
            service: "sieve",
        },
    );
    let mut line = format!("AUTHENTICATE \"{}\"", mechanism.name());
    if let Some(first) = exchange.initial() {
        line.push(' ');
        line.push_str(&sasl_string(&sasl::encode(&first)));
    }
    session.send(&format!("{line}\r\n"))?;
    loop {
        let reply = session.line()?;
        let upper = reply.to_ascii_uppercase();
        if upper.starts_with("OK") {
            // SCRAM's signature may come with the OK: `OK (SASL "…")`,
            // the string perhaps a literal (`OK (SASL {n}`).
            let data = match upper.find("(SASL ") {
                Some(at) => {
                    let rest = &reply[at + 6..];
                    let b64 = match literal_len(rest) {
                        Some(len) => {
                            let text = session.literal(len)?;
                            session.line()?;
                            text
                        }
                        None => words(rest)
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                            .trim_end_matches(')')
                            .to_owned(),
                    };
                    Some(sasl::decode(&b64)?)
                }
                None => None,
            };
            exchange
                .finish(data.as_deref())
                .context(crate::manage::status::AccessFailure::Authentication)?;
            break;
        }
        if upper.starts_with("NO") || upper.starts_with("BYE") {
            let parsed = Reply::new(false, reply[if upper.starts_with("NO") { 2 } else { 3 }..].to_owned());
            if let Some(e) = referral(&parsed, endpoint) {
                return Err(e);
            }
            if parsed.is("TRYLATER") {
                bail!(t!("sync-sieve-the-filter-server-is-busy-try"));
            }
            return Err(
                anyhow::anyhow!(t!("sync-sieve-the-filter-server-refused-the-sign", reply = reply))
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
        session.send(&format!("{}\r\n", sasl_string(&sasl::encode(&answer))))?;
    }
    // The capabilities may differ once signed in (RFC 5804 §2.1: a
    // user's own SIEVE extensions); asked again where the server has
    // CAPABILITY as a command (RFC 5804 servers). This also confirms the
    // session works before scripts are touched.
    let mut caps = caps;
    if caps.rfc5804 {
        let (lines, reply) = session.command("CAPABILITY")?;
        ensure!(reply.ok, t!("sync-sieve-the-filter-server-isnt-answering", explain = reply.explain()));
        let after = capabilities(&lines);
        if !after.sieve.is_empty() {
            caps.sieve = after.sieve;
        }
    }
    Ok((
        session,
        Opened {
            extensions: caps.sieve,
        },
    ))
}
/// What `open` learned of the server.
struct Opened {
    extensions: Vec<String>,
}
/// A refusal that is a REFERRAL: where the account's filters are.
fn referral(reply: &Reply, endpoint: &Endpoint) -> Option<anyhow::Error> {
    let url = reply.referral.as_deref().filter(|_| reply.is("REFERRAL"))?;
    let rest = url.strip_prefix("sieve://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // Only within the same domain: following another host would send the
    // password to whoever answered.
    // "Same domain" is the endpoint's parent domain (`example.co.uk` for
    // `imap.example.co.uk`), not the last two labels, which would let
    // anyone under `co.uk` pass.
    let host = authority
        .split(':')
        .next()
        .unwrap_or(authority)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let own = endpoint.host.trim_end_matches('.').to_ascii_lowercase();
    let parent = own.split_once('.').map(|(_, p)| p).filter(|p| p.contains('.'));
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    let same = valid
        && (host == own || parent.is_some_and(|p| host == p || host.ends_with(&format!(".{p}"))));
    if !same {
        let host: String = host.chars().filter(|c| !c.is_control()).take(253).collect();
        return Some(anyhow::anyhow!(
            t!("sync-sieve-the-filter-server-sends-this-account", host = host)
        ));
    }
    let authority: String = authority.chars().filter(|c| !c.is_control()).take(260).collect();
    Some(anyhow::anyhow!(
        t!("sync-sieve-this-accounts-filters-are-kept-on", authority = authority)
    ))
}

fn scripts(session: &mut Secured) -> Result<Vec<(String, bool)>> {
    let lines = session.expect("LISTSCRIPTS", "list-scripts")?;
    Ok(lines
        .iter()
        .filter_map(|w| {
            let name = w.first()?.clone();
            let active = w.get(1).is_some_and(|a| a.eq_ignore_ascii_case("ACTIVE"));
            Some((name, active))
        })
        .collect())
}

/// What the account's filter server has: Picacho's rules, and the script
/// that runs now if it isn't Picacho's.
pub fn load(endpoint: &Endpoint, secret: &str, tls: &native_tls::TlsConnector) -> Result<State> {
    let (mut session, opened) = open(endpoint, secret, tls)?;
    let extensions = opened.extensions;
    let listed = scripts(&mut session)?;
    let own = if listed.iter().any(|(n, _)| n == SCRIPT) {
        let data = session.expect(
            &format!("GETSCRIPT {}", string(SCRIPT)),
            "read-filters",
        )?;
        data.into_iter().next().and_then(|w| w.into_iter().next())
    } else {
        None
    };
    // The other app's active script, to offer its rules.
    let other = listed
        .iter()
        .find(|(n, a)| *a && n != SCRIPT)
        .and_then(|(name, _)| {
            let (data, reply) = session.command(&format!("GETSCRIPT {}", protocol_string(name).ok()?)).ok()?;
            reply.ok.then(|| data.into_iter().next().and_then(|w| w.into_iter().next())).flatten()
        });
    let _ = session.command("LOGOUT");
    let mut state = loaded(extensions, &listed, own.as_deref());
    state.other_rules = other.and_then(|text| super::sieve_parse::read(&text).ok());
    Ok(state)
}

/// What a filter server has, from its scripts (name, whether active) and
/// the text of Picacho's own, if it has one. ManageSieve and JMAP alike.
pub fn loaded(extensions: Vec<String>, listed: &[(String, bool)], own: Option<&str>) -> State {
    let mut state = State {
        extensions,
        ..State::default()
    };
    for (name, active) in listed {
        if name == SCRIPT {
            state.active = *active;
            if let Some(filters) = own.and_then(read_script) {
                state.filters = filters;
            }
        } else if *active {
            state.other_active = Some(name.clone());
        }
    }
    // Another app's filters are on and Picacho's aren't: by default
    // Picacho's run them too, so saving doesn't silently turn them off.
    if !state.active
        && state.filters.include.is_none()
        && state.has("include")
        && let Some(other) = &state.other_active
    {
        state.filters.include = Some(other.clone());
    }
    state
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
    let (mut session, opened) = open(endpoint, secret, tls)?;
    let extensions = opened.extensions;
    let text = script(filters, &extensions, addresses)?;
    let listed = scripts(&mut session)?;
    let empty = filters.rules.is_empty() && !filters.vacation.on;
    if empty {
        if listed.iter().any(|(n, a)| n == SCRIPT && *a) {
            // Back to whichever script it included, else none.
            let next = filters.include.clone().unwrap_or_default();
            session.expect(
                &format!("SETACTIVE {}", protocol_string(&next)?),
                "turn-off",
            )?;
        }
        // Then removed, so the rules don't come back on the next load.
        if listed.iter().any(|(n, _)| n == SCRIPT) {
            session.expect(
                &format!("DELETESCRIPT {}", string(SCRIPT)),
                "remove",
            )?;
        }
    } else {
        let literal = format!("{{{}+}}\r\n{text}", text.len());
        if extensions.is_empty() {
            bail!(t!("sync-sieve-the-filter-server-didnt-say-what"));
        }
        // Room for it first (HAVESPACE), so a full account says so before
        // anything is uploaded.
        let (_, space) = session.command(&format!(
            "HAVESPACE {} {}",
            string(SCRIPT),
            text.len()
        ))?;
        if !space.ok && (space.is("QUOTA") || !space.text.to_ascii_lowercase().contains("unknown")) {
            bail!(t!("sync-sieve-the-filter-server-cant-take-the", explain = space.explain()));
        }
        let (_, check) = session.command(&format!("CHECKSCRIPT {literal}"))?;
        if !check.ok && !check.text.to_ascii_lowercase().contains("unknown command") {
            bail!(t!("sync-sieve-the-filter-server-rejected-the-filters", explain = check.explain()));
        }
        // PUTSCRIPT replaces a script in one step (RFC 5804 §2.6), active
        // or not: no second copy that a quota (QUOTA/MAXSCRIPTS) could
        // refuse.
        {
            let (_, put) = session.command(&format!("PUTSCRIPT {} {literal}", string(SCRIPT)))?;
            ensure!(put.ok, t!("sync-sieve-the-filter-server-refused-to-save", explain = put.explain()));
            // The script was saved, with warnings (RFC 5804 §1.3): worth
            // knowing, not a failure.
            if put.is("WARNINGS") {
                eprintln!("ManageSieve warnings: {}", put.text);
            }
        }
        session.expect(
            &format!("SETACTIVE {}", string(SCRIPT)),
            "turn-on",
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
        vacation_only: false,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script name with a line break can't end a command and start another.
    #[test]
    fn protocol_strings_refuse_line_breaks() {
        assert_eq!(protocol_string("a\"b").unwrap(), "\"a\\\"b\"");
        for bad in ["x\r\nLOGOUT", "x\nLOGOUT", "x\rLOGOUT", "x\0"] {
            assert!(protocol_string(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn replies_carry_their_codes() {
        let r = Reply::new(false, "(QUOTA/MAXSIZE) \"Too big\"".into());
        assert!(r.is("QUOTA") && !r.ok);
        assert_eq!(r.text, "Too big");
        assert!(r.explain().contains("no room"));
        let r = Reply::new(false, "(REFERRAL \"sieve://sieve2.example.com\") \"Elsewhere\"".into());
        assert_eq!(r.referral.as_deref(), Some("sieve://sieve2.example.com"));
        let ep = Endpoint {
            host: "imap.example.com".into(),
            port: PORT,
            user: "a".into(),
            oauth: false,
            kerberos: false,
        };
        assert!(referral(&r, &ep).unwrap().to_string().contains("sieve2.example.com"));
        let away = Reply::new(false, "(REFERRAL \"sieve://evil.net\")".into());
        assert!(referral(&away, &ep).unwrap().to_string().contains("outside"));
        let plain = Reply::new(true, "".into());
        assert!(plain.ok && plain.code.is_none());
    }

    #[test]
    fn scripts_carry_their_rules_and_escape_text() {
        let filters = Filters {
            rules: vec![
                Rule {
                    field: Field::From,
                    value: "news@shop.example".into(),
                    act: Act::Folder("Caf&AOk-".into()),
                    stop: true,
                    ..Default::default()
                },
                Rule {
                    field: Field::Subject,
                    value: "say \"hi\"".into(),
                    act: Act::MarkRead,
                    stop: false,
                    ..Default::default()
                },
            ],
            vacation: Vacation {
                on: true,
                subject: "Away".into(),
                body: "Back on Monday.\nThanks!".into(),
                days: 7,
                from: None,
                until: Some("2026-12-31".into()),
                ..Default::default()
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
