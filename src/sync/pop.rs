//! Bounded POP3 command and multiline response handling.
use super::imap::tcp;
use crate::manage::model::*;
use anyhow::{Context, Result, ensure};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
};
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
        self.reader.by_ref().take(8192).read_line(&mut line)?;
        ensure!(line.starts_with("+OK"), "POP3 server rejected the request.");
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
                .take((max + 1) as u64)
                .read_until(b'\n', &mut line)?;
            ensure!(n > 0, "POP3 connection closed unexpectedly");
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
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|l| {
                let mut parts = l.split_whitespace();
                let n = parts.next().context("Invalid POP3 UIDL")?.parse()?;
                let uid = parts.next().context("Invalid POP3 UIDL")?.to_string();
                Ok((n, uid))
            })
            .collect()
    }
}
pub(crate) fn pop_login(
    a: &Account,
    password: &str,
    tls: &native_tls::TlsConnector,
) -> Result<PopSession<native_tls::TlsStream<TcpStream>>> {
    let tcp = tcp(&a.incoming_host, a.incoming_port)?;
    let stream = tls.connect(&a.incoming_host, tcp)?;
    let mut s = PopSession::new(stream)?;
    s.command(&format!("USER {}", a.email))?;
    s.command(&format!("PASS {password}"))?;
    Ok(s)
}
