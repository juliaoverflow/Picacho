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
        matches!(
            self.code.as_deref(),
            Some("IN-USE" | "LOGIN-DELAY" | "SYS/TEMP")
        )
    }
    /// A sign-in refused for the credentials themselves: `AUTH`, or no
    /// code at all from a server without RESP-CODES/AUTH-RESP-CODE.
    fn credentials(&self) -> bool {
        !self.temporary() && !matches!(self.code.as_deref(), Some("SYS/PERM"))
    }
}
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.code.as_deref() {
            Some("IN-USE") => "The POP3 mailbox is in use by another session",
            // Words in this one let the account pause as for a server limit.
            Some("LOGIN-DELAY") => "The POP3 server limits how often this account signs in [THROTTLED]",
            Some("SYS/TEMP") => "The POP3 server has a temporary problem [UNAVAILABLE]",
            Some("SYS/PERM") => "The POP3 server has a lasting problem",
            Some("UTF8") => "The POP3 server can't handle UTF-8 here",
            _ => "POP3 server rejected the request",
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
}
impl<T: Read + Write> PopSession<T> {
    pub(crate) fn new(stream: T) -> Result<Self> {
        let mut s = Self {
            reader: BufReader::new(stream),
            caps: Vec::new(),
        };
        s.status()?;
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
        let command = format!("AUTH {}", exchange.mechanism().name());
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
                user_pass,
                "{} offers no sign-in method Picacho supports.",
                a.incoming_host
            );
            // USER/PASS are ASCII unless UTF8 is turned on first (RFC
            // 6856 §2: allowed before signing in where listed as
            // `UTF8 USER`).
            if !(a.login().is_ascii() && password.is_ascii()) {
                ensure!(
                    s.caps.iter().any(|c| c == "UTF8 USER"),
                    "{} can't take a sign-in name or password with non-ASCII characters (POP3 UTF8 USER).",
                    a.incoming_host
                );
            }
            for command in [format!("USER {}", a.login()), format!("PASS {password}")] {
                s.command(&command).map_err(rejected)?;
            }
        }
    }
    Ok(s)
}
impl<T: Read + Write> PopSession<T> {
    /// Turns on UTF-8 (RFC 6856) where listed, so headers aren't mangled by
    /// servers that would otherwise downgrade them. Only allowed before
    /// signing in (AUTHORIZATION state). Best effort.
    fn utf8(&mut self) {
        if self.has("UTF8") {
            let _ = self.command("UTF8");
        }
    }
}

#[cfg(test)]
mod tests {
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
