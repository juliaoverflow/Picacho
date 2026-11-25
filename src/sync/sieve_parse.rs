//! A reader for Sieve scripts (RFC 5228) that other apps wrote: their
//! grammar (commands, blocks, tests, string lists, numbers with K/M/G,
//! `text:` multi-line strings, comments), and the rules Picacho can show
//! and edit taken from them. What doesn't fit a rule is counted, so the
//! person knows the script does more than the list shows.
use super::sieve::{Act, Field, Filters, Match, Rule};
use anyhow::{Result, bail, ensure};

/// A token of the script.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Identifier(String),
    Tag(String),
    Str(String),
    Number(u64),
    Open(char),
    Close(char),
    Comma,
    Semicolon,
}

/// The script's tokens.
fn tokenize(text: &str) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                // RFC 5228 §2.3: a bracketed comment ends with `*/`.
                ensure!(i + 1 < chars.len(), t!("sync-sieve-parse-a-comment-in-the-script-isnt"));
                i += 2;
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                ensure!(i < chars.len(), t!("sync-sieve-parse-a-string-in-the-script-isnt"));
                i += 1;
                out.push(Token::Str(s.replace("\r\n", "\n")));
            }
            '(' | '[' | '{' => {
                out.push(Token::Open(c));
                i += 1;
            }
            ')' | ']' | '}' => {
                out.push(Token::Close(c));
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            ';' => {
                out.push(Token::Semicolon);
                i += 1;
            }
            ':' => {
                let start = i + 1;
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Tag(chars[start..i].iter().collect::<String>().to_ascii_lowercase()));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let n: u64 = chars[start..i].iter().collect::<String>().parse()?;
                let scale = match chars.get(i).map(|c| c.to_ascii_uppercase()) {
                    Some('K') => 1 << 10,
                    Some('M') => 1 << 20,
                    Some('G') => 1 << 30,
                    _ => 1,
                };
                if scale > 1 {
                    i += 1;
                }
                out.push(Token::Number(n.saturating_mul(scale)));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
                // `text:` starts a multi-line string, to a line holding a
                // lone dot (RFC 5228 §2.4.2), dots doubled at line starts.
                if word == "text" && chars.get(i) == Some(&':') {
                    i += 1;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    i += 1;
                    let mut s = String::new();
                    loop {
                        let start = i;
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                        // RFC 5228 §2.4.2: it ends at a line of a lone dot.
                        ensure!(i < chars.len(), t!("sync-sieve-parse-a-text-string-in-the-script"));
                        let line: String = chars[start..i].iter().collect();
                        let line = line.trim_end_matches('\r');
                        i += 1;
                        if line == "." {
                            break;
                        }
                        s.push_str(line.strip_prefix('.').filter(|_| line.starts_with("..")).unwrap_or(line));
                        s.push('\n');
                    }
                    out.push(Token::Str(s));
                } else {
                    out.push(Token::Identifier(word));
                }
            }
            other => bail!(t!("sync-sieve-parse-the-script-has-a-character-sieve", character = format!("{:?}", other))),
        }
    }
    Ok(out)
}

/// An argument of a command or test.
#[derive(Clone, Debug, PartialEq)]
enum Arg {
    Tag(String),
    Strings(Vec<String>),
    Number(u64),
}

/// A test (RFC 5228 §5).
#[derive(Clone, Debug, PartialEq)]
struct Test {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Test>,
}

/// A command and its block.
#[derive(Clone, Debug, PartialEq)]
struct Command {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Test>,
    block: Option<Vec<Command>>,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}
impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }
    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }
    fn commands(&mut self, depth: usize) -> Result<Vec<Command>> {
        ensure!(depth < 32, t!("sync-sieve-parse-the-script-nests-too-deeply"));
        let mut out = Vec::new();
        while let Some(t) = self.peek() {
            if *t == Token::Close('}') {
                break;
            }
            out.push(self.command(depth)?);
        }
        Ok(out)
    }
    fn command(&mut self, depth: usize) -> Result<Command> {
        let Some(Token::Identifier(name)) = self.next() else {
            bail!(t!("sync-sieve-parse-the-script-has-something-where-a"));
        };
        let args = self.arguments()?;
        let mut tests = Vec::new();
        if matches!(self.peek(), Some(Token::Open('('))) {
            tests = self.test_list(depth)?;
        } else if matches!(self.peek(), Some(Token::Identifier(_))) {
            tests.push(self.test(depth)?);
        }
        let block = match self.next() {
            Some(Token::Semicolon) => None,
            Some(Token::Open('{')) => {
                let block = self.commands(depth + 1)?;
                ensure!(self.next() == Some(Token::Close('}')), t!("sync-sieve-parse-a-block-in-the-script-isnt"));
                Some(block)
            }
            _ => bail!(t!("sync-sieve-parse-a-command-in-the-script-doesnt")),
        };
        Ok(Command { name, args, tests, block })
    }
    fn arguments(&mut self) -> Result<Vec<Arg>> {
        let mut out = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Tag(t)) => {
                    out.push(Arg::Tag(t.clone()));
                    self.at += 1;
                }
                Some(Token::Number(n)) => {
                    out.push(Arg::Number(*n));
                    self.at += 1;
                }
                Some(Token::Str(s)) => {
                    out.push(Arg::Strings(vec![s.clone()]));
                    self.at += 1;
                }
                Some(Token::Open('[')) => {
                    self.at += 1;
                    let mut list = Vec::new();
                    loop {
                        match self.next() {
                            Some(Token::Str(s)) => list.push(s),
                            Some(Token::Comma) => {}
                            Some(Token::Close(']')) => break,
                            _ => bail!(t!("sync-sieve-parse-a-list-in-the-script-isnt")),
                        }
                    }
                    out.push(Arg::Strings(list));
                }
                _ => break,
            }
        }
        Ok(out)
    }
    fn test_list(&mut self, depth: usize) -> Result<Vec<Test>> {
        ensure!(self.next() == Some(Token::Open('(')), t!("sync-sieve-parse-a-test-list-in-the-script"));
        let mut out = Vec::new();
        loop {
            out.push(self.test(depth)?);
            match self.next() {
                Some(Token::Comma) => {}
                Some(Token::Close(')')) => break,
                _ => bail!(t!("sync-sieve-parse-a-test-list-in-the-script")),
            }
        }
        Ok(out)
    }
    fn test(&mut self, depth: usize) -> Result<Test> {
        ensure!(depth < 32, t!("sync-sieve-parse-the-script-nests-too-deeply"));
        let Some(Token::Identifier(name)) = self.next() else {
            bail!(t!("sync-sieve-parse-the-script-has-something-where-a-2"));
        };
        let args = self.arguments()?;
        let tests = match self.peek() {
            Some(Token::Open('(')) => self.test_list(depth + 1)?,
            Some(Token::Identifier(_)) => vec![self.test(depth + 1)?],
            _ => Vec::new(),
        };
        Ok(Test { name, args, tests })
    }
}

/// What reading another app's script found: the rules Picacho can show,
/// and how many of its parts it couldn't.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub filters: Filters,
    /// Parts of the script that are more than a rule says (several tests,
    /// variables, notifications…).
    pub left_out: usize,
}

/// Reads a Sieve script, taking what Picacho's rules can say from it.
pub fn read(text: &str) -> Result<Read> {
    let tokens = tokenize(text)?;
    let mut parser = Parser { tokens, at: 0 };
    let commands = parser.commands(0)?;
    ensure!(parser.peek().is_none(), t!("sync-sieve-parse-the-script-has-an-unmatched-closing"));
    let mut out = Read::default();
    for c in &commands {
        match c.name.as_str() {
            "require" => {}
            "if" | "elsif" => match rule_of(c) {
                Some(rule) => out.filters.rules.push(rule),
                None if dated_vacation_of(c, &mut out.filters) => {}
                None => out.left_out += 1,
            },
            "include" => {
                if let Some(Arg::Strings(names)) = c.args.iter().find(|a| matches!(a, Arg::Strings(_))) {
                    out.filters.include = names.first().cloned();
                }
            }
            "vacation" => vacation_of(c, &mut out.filters),
            "keep" | "stop" => {}
            _ => out.left_out += 1,
        }
    }
    Ok(out)
}

/// One `if test { action; [stop;] }` as a rule.
fn rule_of(c: &Command) -> Option<Rule> {
    let [test] = c.tests.as_slice() else { return None };
    let (test, negate) = match test.name.as_str() {
        "not" => (test.tests.first()?, true),
        _ => (test, false),
    };
    let mut rule = rule_test(test)?;
    rule.negate = negate;
    let block = c.block.as_ref()?;
    let mut actions = block.iter().filter(|a| a.name != "stop");
    let action = actions.next()?;
    if actions.next().is_some() {
        return None;
    }
    rule.stop = block.iter().any(|a| a.name == "stop");
    let first = |args: &[Arg]| {
        args.iter().rev().find_map(|a| match a {
            Arg::Strings(s) => s.first().cloned(),
            _ => None,
        })
    };
    // RFC 5229: an action naming a variable (`fileinto "${1}"`) isn't a
    // fixed one; RFC 5232's `:flags`, RFC 3894's `:copy` on fileinto and
    // RFC 8580's `:fcc` do more than a rule says.
    if action.args.iter().any(|a| match a {
        Arg::Strings(s) => s.iter().any(|s| s.contains("${")),
        Arg::Tag(t) => matches!(t.as_str(), "flags" | "fcc") || (t == "copy" && action.name == "fileinto"),
        _ => false,
    }) {
        return None;
    }
    rule.act = match action.name.as_str() {
        "fileinto" => {
            let name = first(&action.args)?;
            // A standard folder by its role: kept as the role.
            let role = action.args.windows(2).find_map(|w| match w {
                [Arg::Tag(t), Arg::Strings(s)] if t == "specialuse" => s.first().cloned(),
                _ => None,
            });
            Act::Folder(role.unwrap_or_else(|| super::transport::encode_name(&name)))
        }
        "discard" => Act::Discard,
        "keep" => Act::Inbox,
        "redirect" => {
            rule.keep_copy = action.args.contains(&Arg::Tag("copy".into()));
            Act::Redirect(first(&action.args)?)
        }
        "reject" | "ereject" => Act::Reject(first(&action.args)?),
        "addflag" | "setflag" => {
            let flag = first(&action.args)?;
            match flag.to_ascii_lowercase().as_str() {
                "\\seen" => Act::MarkRead,
                "\\flagged" => Act::Star,
                f if f.starts_with('\\') => return None,
                _ => Act::Label(flag),
            }
        }
        _ => return None,
    };
    Some(rule)
}

/// A single test as a rule's field, comparison and value.
fn rule_test(t: &Test) -> Option<Rule> {
    let matching = t.args.iter().find_map(|a| match a {
        Arg::Tag(tag) => match tag.as_str() {
            "contains" => Some(Match::Contains),
            "is" => Some(Match::Is),
            "matches" => Some(Match::Matches),
            "regex" => Some(Match::Regex),
            _ => None,
        },
        _ => None,
    });
    let lists: Vec<&Vec<String>> = t
        .args
        .iter()
        .filter_map(|a| match a {
            Arg::Strings(s) => Some(s),
            _ => None,
        })
        .collect();
    let mut rule = Rule {
        matching: matching.unwrap_or(Match::Is),
        ..Rule::default()
    };
    // Part selectors and comparators other than the defaults change what
    // matches; such tests aren't rules. So do RFC 5703's `:mime` header
    // tests (a body part's headers), RFC 5260's `:index`, and RFC 5229's
    // variables in a value (`${1}`), which Picacho's rules can't say.
    if t.args.iter().any(|a| matches!(a, Arg::Tag(t) if matches!(t.as_str(), "localpart" | "domain" | "user" | "detail" | "comparator" | "count" | "value" | "mime" | "anychild" | "index" | "last" | "type" | "subtype" | "contenttype" | "param" | "raw" | "content")))
        || lists.iter().flat_map(|l| l.iter()).any(|s| s.contains("${"))
    {
        return None;
    }
    match t.name.as_str() {
        "address" | "header" | "envelope" => {
            let [names, values] = lists.as_slice() else { return None };
            let [value] = values.as_slice() else { return None };
            rule.value = value.clone();
            let mut names: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
            names.sort();
            rule.field = match (t.name.as_str(), names.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
                ("envelope", ["to"]) => Field::DeliveredTo,
                ("envelope", _) => return None,
                (_, ["from"]) => Field::From,
                (_, ["to"]) => Field::To,
                (_, ["cc", "to"]) => Field::Recipients,
                ("header", ["subject"]) => Field::Subject,
                ("header", ["list-id"]) => Field::List,
                ("header", [name]) => {
                    rule.header = (*name).to_owned();
                    Field::Header
                }
                _ => return None,
            };
        }
        "body" => {
            let [values] = lists.as_slice() else { return None };
            let [value] = values.as_slice() else { return None };
            rule.field = Field::Body;
            rule.value = value.clone();
        }
        "size" if t.args.contains(&Arg::Tag("over".into())) => {
            let n = t.args.iter().find_map(|a| match a {
                Arg::Number(n) => Some(*n),
                _ => None,
            })?;
            rule.field = Field::LargerThan;
            rule.value = (n / 1024).max(1).to_string();
        }
        _ => return None,
    }
    Some(rule)
}

/// `if allof (currentdate …) { vacation …; }`, a reply only between two
/// dates (RFC 5260 §5), as Roundcube and Picacho write it: read as the
/// vacation reply with its first and last day. False where it isn't one.
fn dated_vacation_of(c: &Command, filters: &mut Filters) -> bool {
    let Some([action]) = c.block.as_deref() else { return false };
    if action.name != "vacation" || c.name != "if" {
        return false;
    }
    let tests: &[Test] = match c.tests.as_slice() {
        [t] if t.name == "allof" => &t.tests,
        tests => tests,
    };
    let mut bounds = (None, None);
    let mut zone = String::new();
    for t in tests {
        // RFC 5260 §4.3: `:zone "+0100"` says whose day it is.
        let mut args = t.args.clone();
        if let Some(at) = args.iter().position(|a| *a == Arg::Tag("zone".into())) {
            match args.get(at + 1) {
                Some(Arg::Strings(z)) if z.len() == 1 && (zone.is_empty() || zone == z[0]) => zone = z[0].clone(),
                _ => return false,
            }
            args.drain(at..at + 2);
        }
        let t = &Test { args, ..t.clone() };
        // currentdate :value "ge"|"le" "date" "YYYY-MM-DD"
        let strings: Vec<&str> = t
            .args
            .iter()
            .filter_map(|a| match a {
                Arg::Strings(s) if s.len() == 1 => Some(s[0].as_str()),
                _ => None,
            })
            .collect();
        let valued = t.args.contains(&Arg::Tag("value".into()));
        let date = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok();
        match (t.name.as_str(), valued, strings.as_slice()) {
            ("currentdate", true, ["ge", "date", d]) if date(d) && bounds.0.is_none() => bounds.0 = Some((*d).to_owned()),
            ("currentdate", true, ["le", "date", d]) if date(d) && bounds.1.is_none() => bounds.1 = Some((*d).to_owned()),
            _ => return false,
        }
    }
    if bounds == (None, None) {
        return false;
    }
    vacation_of(action, filters);
    (filters.vacation.from, filters.vacation.until) = bounds;
    filters.vacation.zone = zone;
    true
}

/// A top-level `vacation`'s reply, where it is one Picacho can show.
fn vacation_of(c: &Command, filters: &mut Filters) {
    let mut v = std::mem::take(&mut filters.vacation);
    v.on = true;
    v.days = 7;
    let mut i = 0;
    while i < c.args.len() {
        match (&c.args[i], c.args.get(i + 1)) {
            (Arg::Tag(t), Some(Arg::Number(n))) if t == "days" => {
                v.days = (*n).clamp(1, 30) as u32;
                i += 1;
            }
            // RFC 6131 §2: the period in seconds; `days` the nearest whole
            // days above it, for where only days are shown.
            (Arg::Tag(t), Some(Arg::Number(n))) if t == "seconds" => {
                v.seconds = Some((*n).min(u64::from(u32::MAX)) as u32);
                v.days = n.div_ceil(86_400).clamp(1, 30) as u32;
                i += 1;
            }
            (Arg::Tag(t), Some(Arg::Strings(s))) if t == "subject" => {
                v.subject = s.first().cloned().unwrap_or_default();
                i += 1;
            }
            // RFC 5230 §4.4.
            (Arg::Tag(t), Some(Arg::Strings(s))) if t == "from" => {
                v.sender = s.first().cloned().unwrap_or_default();
                i += 1;
            }
            // RFC 8580 §4: the folder a copy of the reply is kept in.
            (Arg::Tag(t), Some(Arg::Strings(s))) if t == "fcc" => {
                v.fcc = super::transport::encode_name(s.first().map_or("", String::as_str));
                i += 1;
            }
            (Arg::Tag(t), _) if t == "mime" => v.mime = true,
            (Arg::Tag(_), Some(Arg::Strings(_))) => i += 1,
            (Arg::Strings(s), _) => v.body = s.first().cloned().unwrap_or_default(),
            _ => {}
        }
        i += 1;
    }
    filters.vacation = v;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripts as Roundcube's managesieve plugin, Fastmail and
    /// Dovecot/Pigeonhole users write them (RFC 5228 and its extensions).
    const ROUNDCUBE: &str = include_str!("../../tests/fixtures/sieve/roundcube.sieve");
    const FASTMAIL: &str = include_str!("../../tests/fixtures/sieve/fastmail.sieve");
    const DOVECOT: &str = include_str!("../../tests/fixtures/sieve/dovecot.sieve");

    #[test]
    fn roundcube_scripts_are_read() {
        let read = read(ROUNDCUBE).unwrap();
        let rules = &read.filters.rules;
        // The two single-test rules; `anyof` and `allof` with two tests
        // are more than a rule.
        assert_eq!(rules.len(), 2, "{rules:#?}");
        assert_eq!(rules[0].act, Act::Redirect("me@mobile.example".into()));
        assert!(rules[0].keep_copy);
        assert_eq!((rules[1].field, rules[1].value.as_str()), (Field::LargerThan, "10240"));
        assert_eq!(read.left_out, 2);
        // The vacation reply within its dates (RFC 5260 currentdate).
        let v = &read.filters.vacation;
        assert!(v.on);
        assert_eq!((v.days, v.subject.as_str()), (1, "Out of office"));
        assert_eq!(v.body, "I am away until January 5th.\n\n.For urgent matters, call the office.\n");
        assert_eq!((v.from.as_deref(), v.until.as_deref()), (Some("2026-12-20"), Some("2027-01-05")));
    }

    #[test]
    fn fastmail_scripts_are_read() {
        let read = read(FASTMAIL).unwrap();
        // Nested blocks, `set`, `duplicate` and `exists` are parsed and
        // counted, not refused.
        let rules = &read.filters.rules;
        assert_eq!(rules.len(), 0, "{rules:#?}");
        assert!(read.left_out >= 5, "{}", read.left_out);
        assert!(!read.filters.vacation.on, "the reply is inside an `if`");
    }

    #[test]
    fn dovecot_scripts_are_read() {
        let read = read(DOVECOT).unwrap();
        assert_eq!(read.filters.rules.len(), 0);
        assert_eq!(read.filters.include.as_deref(), Some("after"));
        let v = &read.filters.vacation;
        assert!(v.on);
        assert_eq!((v.subject.as_str(), v.body.as_str()), ("Away", "Back Monday."));
        // :seconds 3600 is under a day: the closest a day count says.
        assert_eq!(v.days, 1);
        assert!(read.left_out >= 4, "{}", read.left_out);
    }

    #[test]
    fn grammar_corners_parse() {
        // A comment right after a tag, a bracketed comment across lines,
        // a `text:` with a comment after it, CRLF line ends, and a
        // string with escaped quotes and backslashes.
        let script = "require [\"fileinto\"]; # hash\r\n/* a\r\n * block */\r\nif header :is \"subject\" \"say \\\"hi\\\" \\\\\" { fileinto \"A\"; }\r\nvacation text: # why\r\nline\r\n.\r\n;\r\n";
        let read = read(script).unwrap();
        assert_eq!(read.filters.rules[0].value, "say \"hi\" \\");
        assert_eq!(read.filters.vacation.body, "line\n");
        // A `text:` not closed with a lone dot is an error, not a body.
        assert!(super::read("vacation text:\r\nno end\r\n").is_err());
        // An unclosed bracketed comment too.
        assert!(super::read("keep; /* no end").is_err());
    }

    /// Each extension's commands and tests in their RFC's own forms parse
    /// without error, and those that do more than a rule aren't taken for
    /// one (so taking another app's rules over changes nothing they do).
    #[test]
    fn extensions_parse_and_are_not_mistaken_for_rules() {
        let cases: &[(&str, &str)] = &[
            // RFC 5435 enotify.
            ("enotify", "require \"enotify\"; if header :contains \"from\" \"boss\" { notify :importance \"1\" :options [\"a\"] :message \"Boss\" \"mailto:me@x.example\"; }"),
            ("enotify test", "if valid_notify_method \"xmpp:me@x.example\" { keep; }"),
            ("notify_method_capability", "if notify_method_capability \"xmpp:me@x.example\" \"Online\" [\"yes\"] { keep; }"),
            // RFC 5293 editheader.
            ("addheader", "if header :is \"x\" \"y\" { addheader :last \"X-Seen\" \"yes\"; }"),
            ("deleteheader", "deleteheader :index 1 :last :contains \"X-Spam\" \"yes\";"),
            // RFC 7352 duplicate.
            ("duplicate", "if duplicate :header \"message-id\" :handle \"h\" :seconds 600 :last { discard; }"),
            ("duplicate uniqueid", "if duplicate :uniqueid \"${id}\" { discard; }"),
            // RFC 8580 fcc.
            ("fcc", "if header :is \"x\" \"y\" { notify :fcc \"Sent\" :create \"mailto:a@x.example\"; }"),
            // RFC 5229 variables.
            ("set", "set :lower :upperfirst \"name\" \"${1}\";"),
            ("string", "if string :matches \"${name}\" \"a*\" { keep; }"),
            ("variable folder", "if header :matches \"list-id\" \"*<*.x>\" { fileinto \"Lists/${2}\"; }"),
            ("variable value", "if header :is \"subject\" \"${x}\" { discard; }"),
            // RFC 5235 spamtest and virustest.
            ("spamtest", "if spamtest :value \"ge\" :comparator \"i;ascii-numeric\" \"5\" { discard; }"),
            ("spamtest percent", "if spamtest :percent :count \"eq\" \"0\" { keep; }"),
            ("virustest", "if virustest :value \"eq\" \"5\" { discard; }"),
            // RFC 5703 mime, foreverypart, replace, enclose, extracttext.
            ("mime header", "if header :mime :type \"Content-Type\" \"image\" { discard; }"),
            ("mime anychild", "if header :mime :anychild :param [\"filename\"] :matches \"Content-Disposition\" \"*.exe\" { discard; }"),
            ("foreverypart", "foreverypart :name \"outer\" { if exists :mime \"Content-ID\" { break :name \"outer\"; } }"),
            ("replace", "replace :mime :subject \"Removed\" \"text\";"),
            ("enclose", "enclose :subject \"Warning\" :headers [\"From\"] \"This was flagged\";"),
            ("extracttext", "foreverypart { extracttext :first 100 \"body\"; }"),
            // RFC 5232 :flags, RFC 3894 :copy on fileinto.
            ("fileinto flags", "if header :is \"x\" \"y\" { fileinto :flags \"\\\\Seen\" \"A\"; }"),
            ("fileinto copy", "if header :is \"x\" \"y\" { fileinto :copy \"A\"; }"),
        ];
        for (name, script) in cases {
            let read = read(script).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(read.filters.rules.is_empty(), "{name}: taken for a rule: {:?}", read.filters.rules);
            assert!(read.left_out >= 1, "{name}: not counted");
        }
    }

    /// Picacho's own script, read as another app's would be, says the same.
    #[test]
    fn picachos_own_scripts_read_back() {
        use crate::sync::sieve::{Vacation, script};
        let filters = Filters {
            rules: vec![Rule { field: Field::Subject, value: "x".into(), act: Act::Discard, stop: true, ..Default::default() }],
            vacation: Vacation {
                on: true,
                subject: "Away".into(),
                body: "Back soon.".into(),
                days: 3,
                from: Some("2026-12-20".into()),
                until: Some("2027-01-05".into()),
                ..Default::default()
            },
            include: None,
        };
        let ext: Vec<String> = ["vacation", "date", "relational", "vacation-seconds", "fcc"].iter().map(|s| s.to_string()).collect();
        let read = read(&script(&filters, &ext, &[]).unwrap()).unwrap();
        assert_eq!(read.filters, filters);
        assert_eq!(read.left_out, 0);
        // The reply's From, its zone, a period in seconds, MIME and a copy
        // kept (RFC 5230 §4.4, §5.8; RFC 5260 §4.3; RFC 6131; RFC 8580).
        let mut more = filters.clone();
        more.vacation.sender = "Ana <ana@example.com>".into();
        more.vacation.zone = "+0100".into();
        more.vacation.seconds = Some(3600);
        more.vacation.days = 1;
        more.vacation.mime = true;
        more.vacation.body = "Content-Type: text/plain\n\nBack soon.\n".into();
        more.vacation.fcc = "Sent".into();
        let text = script(&more, &ext, &[]).unwrap();
        assert!(text.contains("currentdate :zone \"+0100\" :value \"ge\""), "{text}");
        assert!(text.contains("vacation :seconds 3600 :handle"), "{text}");
        assert!(text.contains(":from \"Ana <ana@example.com>\" :fcc \"Sent\" :mime "), "{text}");
        assert!(text.contains("\"fcc\"") && text.contains("\"vacation-seconds\""));
        assert_eq!(super::read(&text).unwrap().filters, more);
        // Without the extensions, refused rather than written wrong.
        assert!(script(&more, &ext[..3], &[]).is_err());
        more.vacation.zone = "CET".into();
        assert!(script(&more, &ext, &[]).is_err());
    }

    #[test]
    fn other_apps_scripts_are_read_into_rules() {
        let script = r#"require ["fileinto", "imap4flags", "copy", "body"];
# Roundcube's way
if header :contains "subject" "invoice" { fileinto "Bills"; stop; }
if address :is "from" "boss@example.com" { addflag "\\Flagged"; }
if not header :matches "list-id" "*announce*" { redirect :copy "me@example.org"; }
if size :over 5M { discard; }
if anyof (header :contains "x-spam" "yes", body :contains "viagra") { fileinto "Junk"; }
vacation :days 3 :subject "Away" text:
I'm away.
..Really.
.
;
"#;
        let read = read(script).unwrap();
        let rules = &read.filters.rules;
        assert_eq!(rules.len(), 4, "{rules:#?}");
        assert_eq!((rules[0].field, rules[0].matching, rules[0].value.as_str()), (Field::Subject, Match::Contains, "invoice"));
        assert_eq!(rules[0].act, Act::Folder("Bills".into()));
        assert!(rules[0].stop);
        assert_eq!(rules[1].act, Act::Star);
        assert!(rules[2].negate && rules[2].keep_copy);
        assert_eq!(rules[2].act, Act::Redirect("me@example.org".into()));
        assert_eq!((rules[3].field, rules[3].value.as_str()), (Field::LargerThan, "5120"));
        assert_eq!(read.left_out, 1);
        assert!(read.filters.vacation.on);
        assert_eq!(read.filters.vacation.days, 3);
        assert_eq!(read.filters.vacation.body, "I'm away.\n.Really.\n");
        assert!(super::read("if header { ").is_err());
    }
}
