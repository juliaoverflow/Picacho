//! Bounded POP3 command and multiline response handling.
use super::{
    imap::tcp,
    sasl::{self, Mechanism},
    wire::Wire,
};
use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
/// The server answered -ERR. `code` is its extended response code (RFC
/// 2449 §8, RFC 3206), uppercase, where it gave one: `IN-USE`,
/// `LOGIN-DELAY`, `SYS/TEMP`, `SYS/PERM`, `AUTH`, `UTF8`.
#[derive(Debug)]
pub(crate) struct Rejected {
    pub(crate) code: Option<String>,
    pub(crate) text: String,
}
impl Rejected {
    fn parse(status: &str) -> Self {
        let text = status.strip_prefix("-ERR").unwrap_or(status).trim();
        let (code, text) = match text.strip_prefix('[').and_then(|t| t.split_once(']')) {
            Some((code, rest)) => (Some(code.trim().to_ascii_uppercase()), rest.trim()),
            None => (None, text),
        };
        Self {
            code,
            text: text.to_owned(),
        }
    }
    /// The server is busy or limiting this account, for now: another
    /// session holds the mailbox (`IN-USE`), it allows only so many
    /// sign-ins (`LOGIN-DELAY`), or a temporary fault (`SYS/TEMP`). The
    /// credentials aren't at fault.
    pub(crate) fn temporary(&self) -> bool {
        self.is("IN-USE") || self.is("LOGIN-DELAY") || self.is("SYS/TEMP")
    }
    /// Whether the code is `code` or one below it in the hierarchy
    /// (`SYS/TEMP/DISK` is SYS/TEMP; RFC 3206 §4 and RFC 2449 §8: clients
    /// must treat unknown sub-codes as their parent).
    fn is(&self, code: &str) -> bool {
        self.code.as_deref().is_some_and(|c| {
            c == code || c.strip_prefix(code).is_some_and(|rest| rest.starts_with('/'))
        })
    }
    /// A sign-in refused for the credentials themselves: `AUTH`, or no
    /// code at all from a server without RESP-CODES/AUTH-RESP-CODE.
    fn credentials(&self) -> bool {
        !self.temporary() && !self.is("SYS/PERM") && !self.is("UTF8")
    }
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = if self.is("IN-USE") {
            "The POP3 mailbox is in use by another session"
        } else if self.is("LOGIN-DELAY") {
            // Words in this one let the account pause as for a server limit.
            "The POP3 server limits how often this account signs in [THROTTLED]"
        } else if self.is("SYS/TEMP") {
            "The POP3 server has a temporary problem [UNAVAILABLE]"
        } else if self.is("SYS/PERM") {
            "The POP3 server has a lasting problem"
        } else if self.is("UTF8") {
            "The POP3 server can't handle UTF-8 here"
        } else {
            "POP3 server rejected the request"
        };
        if self.text.is_empty() {
            f.write_str(what)
        } else {
            write!(f, "{what}: {}", self.text)
        }
    }
}
impl std::error::Error for Rejected {}
/// The refusal behind an error, if POP3 gave one.
pub(crate) fn rejection(error: &anyhow::Error) -> Option<&Rejected> {
    error.chain().find_map(|c| c.downcast_ref::<Rejected>())
}
pub struct PopSession<T: Read + Write> {
    reader: BufReader<T>,
    /// What CAPA listed (uppercase), once asked.
    caps: Vec<String>,
    /// A SASL initial response too long for the AUTH line, sent after the
    /// server's first (empty) challenge.
    pending_initial: Option<String>,
    /// UTF8 was turned on (RFC 6856).
    pub(crate) utf8_on: bool,
    /// The greeting's `<…@…>` timestamp, where it has one: APOP's
    /// challenge (RFC 1939 §7).
    apop: Option<String>,
}
impl<T: Read + Write> PopSession<T> {
    pub(crate) fn new(stream: T) -> Result<Self> {
        let mut s = Self {
            reader: BufReader::new(stream),
            caps: Vec::new(),
            pending_initial: None,
            utf8_on: false,
            apop: None,
        };
        let greeting = s.status()?;
        // `+OK POP3 ready <1896.697170952@dbc.mtview.ca.us>`
        s.apop = greeting
            .rfind('<')
            .zip(greeting.rfind('>'))
            .filter(|(a, b)| a < b)
            .map(|(a, b)| greeting[a..=b].to_owned())
            .filter(|t| t.contains('@') && !t.contains(char::is_whitespace));
        Ok(s)
    }
    fn status(&mut self) -> Result<String> {
        let mut line = String::new();
        self.reader.by_ref().take(8193).read_line(&mut line)?;
        ensure!(
            line.ends_with('\n') && line.len() <= 8192,
            "Incomplete or oversized POP3 status"
        );
        let status = line.trim_end_matches(['\r', '\n']);
        if status.starts_with("-ERR") {
            return Err(Rejected::parse(status).into());
        }
        ensure!(
            status == "+OK" || status.starts_with("+OK "),
            "Invalid POP3 status"
        );
        Ok(line)
    }
    /// Whether CAPA listed `capability` (its first word).
    pub(crate) fn has(&self, capability: &str) -> bool {
        self.caps.iter().any(|c| {
            c.split_ascii_whitespace()
                .next()
                .is_some_and(|w| w.eq_ignore_ascii_case(capability))
        })
    }
    /// The headers of message `number`, without its body (TOP n 0, RFC 1939
    /// §7), where the server lists TOP; `None` where it doesn't.
    pub(crate) fn top(&mut self, number: u32, max: usize) -> Result<Option<Vec<u8>>> {
        ensure!(number > 0, "Invalid POP3 message number");
        if !self.caps.is_empty() && !self.has("TOP") {
            return Ok(None);
        }
        match self.command(&format!("TOP {number} 0")) {
            Ok(_) => Ok(Some(self.multiline(max)?)),
            // A server without CAPA may not know TOP.
            Err(e) if self.caps.is_empty() && rejection(&e).is_some() => Ok(None),
            Err(e) => Err(e),
        }
    }
    /// Each message's size in octets (LIST), by message number.
    pub(crate) fn list(&mut self) -> Result<std::collections::HashMap<u32, u64>> {
        self.command("LIST")?;
        let bytes = self.multiline(4 * 1024 * 1024)?;
        let mut out = std::collections::HashMap::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let mut parts = line.split_ascii_whitespace();
            if let (Some(Ok(n)), Some(Ok(size))) = (
                parts.next().map(str::parse::<u32>),
                parts.next().map(str::parse::<u64>),
            ) {
                out.insert(n, size);
            }
        }
        Ok(out)
    }
    pub(crate) fn command(&mut self, command: &str) -> Result<String> {
        ensure!(!command.contains(['\r', '\n']), "Invalid POP3 command");
        write!(self.reader.get_mut(), "{command}\r\n")?;
        self.reader.get_mut().flush()?;
        self.status()
    }
    /// What the server says it can do (CAPA, RFC 2449), uppercase; none
    /// when it doesn't know CAPA.
    pub(crate) fn capabilities(&mut self) -> Result<Vec<String>> {
        match self.command("CAPA") {
            Ok(_) => {}
            Err(e) if e.is::<Rejected>() => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
        let list = self.multiline(64 * 1024)?;
        self.caps = String::from_utf8_lossy(&list)
            .lines()
            .map(|l| l.trim().to_ascii_uppercase())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(self.caps.clone())
    }
    /// Signs in with AUTH and a SASL mechanism (RFC 5034).
    pub(crate) fn authenticate(&mut self, exchange: &mut sasl::Exchange) -> Result<()> {
        // The client's first message goes with the command (RFC 5034 §4),
        // where the whole line stays within POP3's 255 octets; else it
        // follows the server's empty challenge.
        let mut command = format!("AUTH {}", exchange.mechanism().name());
        if let Some(first) = exchange.initial() {
            let encoded = if first.is_empty() { "=".to_owned() } else { sasl::encode(&first) };
            if command.len() + 1 + encoded.len() + 2 <= 255 {
                command = format!("{command} {encoded}");
            } else {
                // Too long: sent after the challenge instead.
                self.pending_initial = Some(encoded);
            }
        }
        write!(self.reader.get_mut(), "{command}\r\n")?;
        self.reader.get_mut().flush()?;
        loop {
            let mut line = String::new();
            self.reader.by_ref().take(8193).read_line(&mut line)?;
            ensure!(
                line.ends_with('\n') && line.len() <= 8192,
                "Incomplete or oversized POP3 status"
            );
            let line = line.trim_end_matches(['\r', '\n']);
            if line == "+OK" || line.starts_with("+OK ") {
                return exchange.finish(None);
            }
            if line.starts_with("-ERR") {
                let reason = exchange.refusal().map(str::to_owned);
                let error = anyhow::Error::new(Rejected::parse(line));
                return Err(match reason {
                    Some(reason) => {
                        error.context(format!("The server refused the token ({reason})"))
                    }
                    None => error,
                });
            }
            let challenge = line
                .strip_prefix('+')
                .context("Invalid POP3 sign-in reply")?;
            if let Some(first) = self.pending_initial.take() {
                write!(self.reader.get_mut(), "{first}\r\n")?;
                self.reader.get_mut().flush()?;
                continue;
            }
            let answer = match exchange.step(&sasl::decode(challenge)?) {
                Ok(answer) => answer,
                Err(e) => {
                    // "*" cancels the exchange; the server answers -ERR.
                    self.reader.get_mut().write_all(b"*\r\n")?;
                    self.reader.get_mut().flush()?;
                    let _ = self.status();
                    return Err(e);
                }
            };
            write!(self.reader.get_mut(), "{}\r\n", sasl::encode(&answer))?;
            self.reader.get_mut().flush()?;
        }
    }
    pub(crate) fn multiline(&mut self, max: usize) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        loop {
            let mut line = Vec::new();
            let n = self
                .reader
                .by_ref()
                .take((max.saturating_sub(result.len()).saturating_add(3)) as u64)
                .read_until(b'\n', &mut line)?;
            ensure!(n > 0, "POP3 connection closed unexpectedly");
            ensure!(line.ends_with(b"\n"), "Incomplete POP3 response line");
            if line == b".\r\n" || line == b".\n" {
                break;
            }
            if line.starts_with(b"..") {
                line.remove(0);
            }
            ensure!(
                result.len() + line.len() <= max,
                "POP3 response exceeds the download limit"
            );
            result.extend(line);
        }
        Ok(result)
    }
    /// Marks a message for deletion (DELE). The server removes marked
    /// messages only when the session ends with QUIT (RFC 1939's UPDATE
    /// state); a dropped connection deletes nothing.
    /// Whether the server marked it; a refusal (-ERR, say the message was
    /// already marked) leaves the session usable.
    pub(crate) fn dele(&mut self, number: u32) -> Result<bool> {
        ensure!(number > 0, "Invalid POP3 message number");
        match self.command(&format!("DELE {number}")) {
            Ok(_) => Ok(true),
            Err(e) if e.is::<Rejected>() => Ok(false),
            Err(e) => Err(e),
        }
    }
    /// Ends the session (QUIT). Its +OK confirms that messages marked with
    /// DELE were removed.
    pub(crate) fn quit(&mut self) -> Result<()> {
        self.command("QUIT")?;
        Ok(())
    }
    /// How many messages the mailbox has and their total size in octets
    /// (STAT, RFC 1939 §5: `+OK nn mm`).
    pub(crate) fn stat(&mut self) -> Result<(u32, u64)> {
        let line = self.command("STAT")?;
        let mut words = line.split_ascii_whitespace().skip(1);
        let count = words.next().and_then(|n| n.parse().ok());
        let size = words.next().and_then(|n| n.parse().ok());
        count.zip(size).context("Invalid POP3 STAT reply")
    }
    /// Unmarks every message marked for deletion in this session (RSET,
    /// RFC 1939 §5), so QUIT removes nothing.
    pub(crate) fn rset(&mut self) -> Result<()> {
        self.command("RSET")?;
        Ok(())
    }
    /// Each message's lasting identity, by message number, oldest first:
    /// UIDL's (RFC 1939 §7) where the server has it; otherwise one made
    /// from a hash of the message's headers (`header_identity`), read with
    /// TOP (or RETR, where there is no TOP). Messages with the same headers
    /// are told apart by how many came before them.
    pub(crate) fn identities(&mut self, max_header: usize) -> Result<Vec<(u32, String)>> {
        // A server listing capabilities without UIDL lacks it; one without
        // CAPA may still have it.
        if self.caps.is_empty() || self.has("UIDL") {
            match self.uidl() {
                Ok(ids) => return Ok(ids),
                Err(e) if self.caps.is_empty() && rejection(&e).is_some() => {}
                Err(e) => return Err(e),
            }
        }
        let (count, _) = self.stat()?;
        let mut out = Vec::with_capacity(count as usize);
        let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for number in 1..=count {
            let header = match self.top(number, max_header)? {
                Some(header) => header,
                // Without TOP the whole message is read for it; only its
                // headers are hashed.
                None => {
                    self.command(&format!("RETR {number}"))?;
                    self.multiline(crate::manage::crypto::MAX_MESSAGE)?
                }
            };
            let base = header_identity(&header);
            let n = seen.entry(base.clone()).or_default();
            *n += 1;
            let id = if *n == 1 { base } else { format!("{base}-{n}") };
            out.push((number, id));
        }
        Ok(out)
    }
    pub(crate) fn uidl(&mut self) -> Result<Vec<(u32, String)>> {
        self.command("UIDL")?;
        let bytes = self.multiline(4 * 1024 * 1024)?;
        let mut numbers = HashSet::new();
        let mut identities = HashSet::new();
        let mut result = Vec::new();
        for line in std::str::from_utf8(&bytes)
            .context("Invalid POP3 UIDL encoding")?
            .lines()
        {
            let mut parts = line.split_ascii_whitespace();
            let number: u32 = parts.next().context("Invalid POP3 UIDL")?.parse()?;
            let uid = parts.next().context("Invalid POP3 UIDL")?;
            ensure!(
                number > 0 && parts.next().is_none(),
                "Invalid POP3 UIDL entry"
            );
            ensure!(
                (1..=70).contains(&uid.len()) && uid.bytes().all(|b| (0x21..=0x7e).contains(&b)),
                "Invalid POP3 UIDL identity"
            );
            ensure!(
                numbers.insert(number) && identities.insert(uid.to_owned()),
                "Duplicate POP3 UIDL identity or message number"
            );
            result.push((number, uid.to_owned()));
        }
        // Servers may return UIDL rows out of order. Message numbers, not row
        // positions, determine which messages are newest in this session.
        result.sort_unstable_by_key(|(number, _)| *number);
        Ok(result)
    }
}
/// A lasting identity for a message on a server without UIDL: a hash of
/// the headers that don't change while it sits in the mailbox (its
/// Message-ID, Date, From, To, Subject, and the first Received). Prefixed
/// so it can't be taken for a UIDL identity; 70 printable characters at
/// most, as UIDL's (RFC 1939 §7).
pub(crate) fn header_identity(header: &[u8]) -> String {
    use sha2::Digest as _;
    let (headers, _) = mailparse::parse_headers(header).unwrap_or_default();
    let mut hash = sha2::Sha256::new();
    for name in ["Message-ID", "Date", "From", "To", "Subject", "Received"] {
        let value = headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case(name))
            .map(|h| h.get_value_raw().to_vec())
            .unwrap_or_default();
        hash.update(name.as_bytes());
        hash.update(b":");
        hash.update(&value);
        hash.update(b"\n");
    }
    // Without any of those, the header block as a whole.
    if headers.is_empty() {
        hash.update(header);
    }
    let digest = hash.finalize();
    let hex: String = digest.iter().take(20).map(|b| format!("{b:02x}")).collect();
    format!("hdr-{hex}")
}
/// Connects with TLS from the start, or (`Account::incoming_starttls`)
/// in the clear and upgrades with STLS (RFC 2595) before anything else: a
/// server that doesn't offer or refuses it is left without signing in.
fn pop_connect(
    a: &Account,
    tls: &native_tls::TlsConnector,
) -> Result<native_tls::TlsStream<Wire>> {
    let tcp = tcp(&a.incoming_host, a.incoming_port)?;
    if !a.incoming_starttls {
        return Ok(tls.connect(&a.incoming_host, tcp)?);
    }
    let mut plain = PopSession::new(tcp)?;
    let caps = plain.capabilities()?;
    ensure!(
        caps.is_empty() || plain.has("STLS"),
        "{} doesn't offer encryption (STLS) on port {}; Picacho won't sign in to it. Use port 995 with TLS from the start.",
        a.incoming_host,
        a.incoming_port
    );
    plain.command("STLS").map_err(|e| {
        e.context("The POP3 server refused to start encryption (STLS); credentials were not sent")
    })?;
    // A server must not send anything before the handshake; bytes already
    // read could be an attacker's injected commands' replies.
    ensure!(
        plain.reader.buffer().is_empty(),
        "Unexpected data from the POP3 server before encryption started"
    );
    let socket = plain.reader.into_inner();
    tls.connect(&a.incoming_host, socket)
        .map_err(|e| anyhow::anyhow!("POP3 TLS handshake failed: {e}"))
}
/// The session after STLS has no greeting; this starts one already past it.
impl<T: Read + Write> PopSession<T> {
    fn upgraded(stream: T) -> Self {
        Self {
            reader: BufReader::new(stream),
            caps: Vec::new(),
            pending_initial: None,
            utf8_on: false,
            apop: None,
        }
    }
}
pub(crate) fn pop_login(
    a: &Account,
    password: &str,
    tls: &native_tls::TlsConnector,
) -> Result<PopSession<native_tls::TlsStream<Wire>>> {
    let stream = pop_connect(a, tls)?;
    let binding = super::sasl::binding(&stream);
    let mut s = if a.incoming_starttls {
        PopSession::upgraded(stream)
    } else {
        PopSession::new(stream)?
    };
    let rejected = |error: anyhow::Error| match rejection(&error) {
        Some(r) if r.credentials() => {
            error.context(crate::manage::status::AccessFailure::Authentication)
        }
        _ => error,
    };
    let caps = s.capabilities()?;
    s.utf8();
    let offered: Vec<&str> = caps
        .iter()
        .filter_map(|c| c.strip_prefix("SASL "))
        .flat_map(str::split_ascii_whitespace)
        .collect();
    let credentials = sasl::Credentials {
        user: a.login().to_owned(),
        secret: password.to_owned(),
        host: a.incoming_host.clone(),
        port: a.incoming_port,
        binding: binding.clone(),
        service: "pop",
    };
    if a.oauth {
        // An access token (single sign-on): OAUTHBEARER, else XOAUTH2;
        // never the token as a password.
        let mechanism = Mechanism::choose(&sasl::TOKEN_ORDER, offered.iter().copied())
            .with_context(|| {
                format!(
                    "{} doesn't take sign-in tokens (SASL OAUTHBEARER or XOAUTH2). Ask your organization whether its POP3 server supports single sign-on.",
                    a.incoming_host
                )
            })?;
        s.authenticate(&mut sasl::Exchange::new(
            mechanism,
            sasl::Credentials {
                user: if a.sso.is_some() { a.login().to_owned() } else { a.email.clone() },
                ..credentials
            },
        ))
        .map_err(rejected)?;
        s.capabilities_after_login();
        return Ok(s);
    }
    if a.kerberos {
        // Kerberos only: never a password, and never another mechanism.
        let mechanism = Mechanism::choose(&sasl::KERBEROS_ORDER, offered.iter().copied())
            .with_context(|| {
                format!(
                    "{} doesn't offer Kerberos sign-in (SASL GSSAPI). Turn off Kerberos for this account to use a password.",
                    a.incoming_host
                )
            })?;
        s.authenticate(&mut sasl::Exchange::new(
            mechanism,
            sasl::Credentials {
                secret: String::new(),
                ..credentials
            },
        ))
        .map_err(rejected)?;
        s.capabilities_after_login();
        return Ok(s);
    }
    // SCRAM where offered; else USER/PASS (unless the server lists
    // capabilities without USER), else another SASL mechanism.
    let user_pass = caps.is_empty() || caps.iter().any(|c| c == "USER");
    let mechanism = Mechanism::choose_bound(
        &sasl::SCRAM_ORDER,
        offered.iter().copied(),
        !binding.is_empty(),
    )
    .or_else(|| {
        (!user_pass)
            .then(|| {
                Mechanism::choose(
                    &[Mechanism::Plain, Mechanism::Login, Mechanism::CramMd5],
                    offered.iter().copied(),
                )
            })
            .flatten()
    });
    match mechanism {
        Some(mechanism) => s
            .authenticate(&mut sasl::Exchange::new(mechanism, credentials))
            .map_err(rejected)?,
        None => {
            ensure!(
                user_pass || s.apop.is_some(),
                "{} offers no sign-in method Picacho supports.",
                a.incoming_host
            );
            // USER/PASS are ASCII unless UTF8 is turned on first (RFC
            // 6856 §2: allowed before signing in where listed as
            // `UTF8 USER`).
            if !(a.login().is_ascii() && password.is_ascii()) {
                ensure!(
                    s.utf8_on && s.caps.iter().any(|c| c == "UTF8 USER"),
                    "{} can't take a sign-in name or password with non-ASCII characters (POP3 UTF8 USER).",
                    a.incoming_host
                );
            }
            // APOP where the server lists no USER but greeted with a
            // timestamp: the password goes as a digest (RFC 1939 §7).
            if !s.caps.is_empty() && !s.has("USER") && let Some(stamp) = s.apop.clone() {
                use md5::Digest as _;
                let digest = md5::Md5::digest(format!("{stamp}{password}").as_bytes());
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                s.command(&format!("APOP {} {hex}", a.login())).map_err(rejected)?;
            } else {
                for command in [format!("USER {}", a.login()), format!("PASS {password}")] {
                    s.command(&command).map_err(rejected)?;
                }
            }
        }
    }
    s.capabilities_after_login();
    Ok(s)
}
impl<T: Read + Write> PopSession<T> {
    /// Turns on UTF-8 (RFC 6856) where listed, so headers aren't mangled by
    /// servers that would otherwise downgrade them. Only allowed before
    /// signing in (AUTHORIZATION state). Best effort.
    fn utf8(&mut self) {
        if self.has("UTF8") {
            // A refusal leaves the session in ASCII; it is noted so a
            // non-ASCII sign-in isn't attempted as if UTF-8 were on.
            self.utf8_on = self.command("UTF8").is_ok();
        }
    }
    /// Asks CAPA again once signed in (RFC 2449 §5: the TRANSACTION
    /// state's capabilities may differ: TOP, UIDL, EXPIRE, LOGIN-DELAY).
    /// What was listed before stays where the server doesn't answer.
    fn capabilities_after_login(&mut self) {
        let before = std::mem::take(&mut self.caps);
        match self.capabilities() {
            Ok(caps) if !caps.is_empty() => {}
            _ => self.caps = before,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn response_codes_match_their_hierarchy() {
        let r = Rejected::parse("-ERR [SYS/TEMP/DISK] full");
        assert!(r.temporary() && !r.credentials());
        let r = Rejected::parse("-ERR [SYS/PERM/GONE] closed");
        assert!(!r.temporary() && !r.credentials());
        let r = Rejected::parse("-ERR [UTF8/USER] no");
        assert!(!r.credentials());
        let r = Rejected::parse("-ERR [SYS/TEMPORARY] odd");
        assert!(!r.temporary());
        assert!(Rejected::parse("-ERR [AUTH] wrong").credentials());
    }
    use super::*;
    use std::io::Cursor;
    #[test]
    fn rejects_truncated_oversized_and_ambiguous_status_lines() {
        for status in [
            b"+OK".to_vec(),
            b"+OKAY\r\n".to_vec(),
            b"-ERR\r\n".to_vec(),
            [b"+OK ".as_slice(), &vec![b'x'; 8192], b"\r\n"].concat(),
        ] {
            assert!(PopSession::new(Cursor::new(status)).is_err());
        }
    }
    #[test]
    fn kerberos_accounts_never_fall_back_to_a_password() -> Result<()> {
        // A server offering only passwords: sign-in stops before any is sent.
        let (cert, acceptor) = crate::manage::backend::protocol_tests_tls()?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = std::thread::spawn(move || {
            let stream = acceptor.accept(listener.accept().unwrap().0).unwrap();
            let mut r = BufReader::new(stream);
            r.get_mut().write_all(b"+OK ready\r\n").unwrap();
            let mut seen = Vec::new();
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 0 {
                let command = line.trim_end().to_owned();
                line.clear();
                let reply: &[u8] = if command == "CAPA" {
                    b"+OK\r\nUSER\r\nSASL PLAIN SCRAM-SHA-256\r\n.\r\n"
                } else {
                    b"+OK\r\n"
                };
                seen.push(command);
                if r.get_mut().write_all(reply).is_err() {
                    break;
                }
            }
            seen
        });
        let mut a = Account::new(Provider::Pop3);
        a.email = "ana@example.com".into();
        a.incoming_host = "localhost".into();
        a.incoming_port = port;
        a.kerberos = true;
        let mut tls = native_tls::TlsConnector::builder();
        tls.add_root_certificate(native_tls::Certificate::from_pem(&cert)?);
        let error = pop_login(&a, "a-password", &tls.build()?).err().unwrap();
        assert!(
            format!("{error:#}").contains("doesn't offer Kerberos"),
            "{error:#}"
        );
        let seen = server.join().unwrap();
        assert_eq!(seen, ["CAPA"], "no USER, PASS or AUTH was sent");
        Ok(())
    }
    /// RFC 1939 §5: STAT's counts and RSET; without UIDL, each
    /// message's identity comes from a hash of its headers (TOP), the same
    /// in every session and different for different mail.
    #[test]
    fn stat_rset_and_identities_without_uidl() -> Result<()> {
        struct Script {
            replies: Cursor<Vec<u8>>,
            sent: Vec<u8>,
        }
        impl Read for Script {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.replies.read(buf)
            }
        }
        impl Write for Script {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.sent.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let a = "Message-ID: <a@x>\r\nSubject: One\r\n\r\n";
        let b = "Message-ID: <b@x>\r\nSubject: Two\r\n\r\n";
        let replies = format!(
            "+OK ready\r\n+OK\r\nUSER\r\nTOP\r\n.\r\n+OK 3 920\r\n+OK\r\n+OK 3 920\r\n+OK\r\n{a}.\r\n+OK\r\n{b}.\r\n+OK\r\n{a}.\r\n"
        );
        let mut pop = PopSession::new(Script {
            replies: Cursor::new(replies.into_bytes()),
            sent: Vec::new(),
        })?;
        pop.capabilities()?;
        assert_eq!(pop.stat()?, (3, 920));
        pop.rset()?;
        let ids = pop.identities(4096)?;
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|(_, id)| id.starts_with("hdr-") && id.len() <= 70));
        assert_ne!(ids[0].1, ids[1].1);
        // The same headers again: told apart, the same way every session.
        assert_eq!(ids[2].1, format!("{}-2", ids[0].1));
        assert_eq!(header_identity(a.as_bytes()), ids[0].1);
        let sent = String::from_utf8(pop.reader.get_ref().sent.clone()).unwrap();
        assert_eq!(sent, "CAPA\r\nSTAT\r\nRSET\r\nSTAT\r\nTOP 1 0\r\nTOP 2 0\r\nTOP 3 0\r\n");
        Ok(())
    }
    #[test]
    fn multiline_limits_include_dot_unstuffing_and_terminators() -> Result<()> {
        let mut pop = PopSession::new(Cursor::new(b"+OK\r\n..abc\r\n.\r\n".to_vec()))?;
        assert_eq!(pop.multiline(6)?, b".abc\r\n");
        let mut pop = PopSession::new(Cursor::new(b"+OK\r\n..abc\r\n.\r\n".to_vec()))?;
        assert!(pop.multiline(5).is_err());
        let mut pop = PopSession::new(Cursor::new(b"+OK\r\nPartial".to_vec()))?;
        assert!(pop.multiline(100).is_err());
        Ok(())
    }
}
