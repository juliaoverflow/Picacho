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
#[derive(Debug)]
struct Rejected;
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("POP3 server rejected the request")
    }
}
impl std::error::Error for Rejected {}
pub struct PopSession<T: Read + Write> {
    reader: BufReader<T>,
}
impl<T: Read + Write> PopSession<T> {
    pub(crate) fn new(stream: T) -> Result<Self> {
        let mut s = Self {
            reader: BufReader::new(stream),
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
            return Err(Rejected.into());
        }
        ensure!(
            status == "+OK" || status.starts_with("+OK "),
            "Invalid POP3 status"
        );
        Ok(line)
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
        Ok(String::from_utf8_lossy(&list)
            .lines()
            .map(|l| l.trim().to_ascii_uppercase())
            .filter(|l| !l.is_empty())
            .collect())
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
                let error = anyhow::Error::new(Rejected);
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
pub(crate) fn pop_login(
    a: &Account,
    password: &str,
    tls: &native_tls::TlsConnector,
) -> Result<PopSession<native_tls::TlsStream<Wire>>> {
    let tcp = tcp(&a.incoming_host, a.incoming_port)?;
    let stream = tls.connect(&a.incoming_host, tcp)?;
    let mut s = PopSession::new(stream)?;
    let rejected = |error: anyhow::Error| {
        if error.is::<Rejected>() || error.chain().any(|c| c.is::<Rejected>()) {
            error.context(crate::manage::status::AccessFailure::Authentication)
        } else {
            error
        }
    };
    let caps = s.capabilities()?;
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
    };
    // SCRAM where offered; else USER/PASS (unless the server lists
    // capabilities without USER), else another SASL mechanism.
    let user_pass = caps.is_empty() || caps.iter().any(|c| c == "USER");
    let mechanism = Mechanism::choose(
        &[Mechanism::ScramSha256, Mechanism::ScramSha1],
        offered.iter().copied(),
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
            for command in [format!("USER {}", a.login()), format!("PASS {password}")] {
                s.command(&command).map_err(rejected)?;
            }
        }
    }
    Ok(s)
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
