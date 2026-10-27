//! Bounded POP3 command and multiline response handling.
use super::{imap::tcp, wire::Wire};
use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
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
        ensure!(
            status == "+OK" || status.starts_with("+OK "),
            "POP3 server rejected the request."
        );
        Ok(line)
    }
    pub(crate) fn command(&mut self, command: &str) -> Result<String> {
        ensure!(!command.contains(['\r', '\n']), "Invalid POP3 command");
        write!(self.reader.get_mut(), "{command}\r\n")?;
        self.reader.get_mut().flush()?;
        self.status()
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
    s.command(&format!("USER {}", a.email))?;
    s.command(&format!("PASS {password}"))?;
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
