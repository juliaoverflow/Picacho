//! The byte stream an IMAP session runs over: verified TLS on a bounded
//! [`Wire`], optionally compressed (COMPRESS=DEFLATE, RFC 4978), and able to
//! wait longer for the server while a session is in IDLE.
use super::wire::{RESPONSE_LIMIT, Wire};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Raw deflate in each direction, once both sides have agreed to it.
struct Deflate {
    compress: Compress,
    decompress: Decompress,
    /// Compressed bytes read from TLS and not yet inflated.
    input: Vec<u8>,
    /// Inflated bytes not yet handed to the reader.
    output: Vec<u8>,
    taken: usize,
    /// Inflated bytes the current response may still grow by. The wire
    /// below counts compressed bytes, which deflate can expand about a
    /// thousandfold, so the same limit is kept here on what they inflate to.
    remaining: usize,
}
impl Deflate {
    fn new() -> Self {
        Self {
            // RFC 4978: raw deflate, without zlib headers.
            compress: Compress::new(Compression::default(), false),
            decompress: Decompress::new(false),
            input: Vec::new(),
            output: Vec::new(),
            taken: 0,
            remaining: RESPONSE_LIMIT,
        }
    }
    /// Counts `made` freshly inflated bytes against the response's budget.
    fn spend(&mut self, made: usize) -> io::Result<()> {
        self.remaining = self.remaining.checked_sub(made).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Mail server response exceeds the 32 MB session-command limit",
            )
        })?;
        Ok(())
    }
}

/// Keeps a copy of what the server sends while it is on, so a session can
/// see the lines the imap crate reads and drops (those IDLE wakes on).
#[derive(Clone, Default)]
pub(crate) struct Tap(Arc<Mutex<Option<Vec<u8>>>>, Literals);
impl Tap {
    /// The session's commands with literals; see [`Literals`].
    pub(crate) fn literals(&self) -> Literals {
        self.1.clone()
    }
    /// More than this is never kept: the copy is only read for what kind
    /// of lines it holds, and a long one is taken to report a change.
    pub(crate) const LIMIT: usize = 64 * 1024;
    /// Starts keeping a copy, from the next byte read.
    pub(crate) fn start(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(Vec::new());
    }
    /// Stops keeping a copy, returning what was read since `start`.
    pub(crate) fn take(&self) -> Option<Vec<u8>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
    fn record(&self, bytes: &[u8]) {
        if let Some(copy) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let room = Self::LIMIT.saturating_sub(copy.len());
            copy.extend_from_slice(&bytes[..bytes.len().min(room)]);
        }
    }
}

pub(crate) struct ImapStream {
    tls: native_tls::TlsStream<Wire>,
    deflate: Option<Box<Deflate>>,
    tap: Tap,
    /// Set once the server accepted COMPRESS; compression starts with the
    /// next byte in either direction.
    compressing: Arc<AtomicBool>,
    /// Literals for the next command (`literals`): each is sent once the
    /// server has answered the line before it with `+`.
    literals: Arc<Mutex<Vec<Vec<u8>>>>,
    /// A line read while sending literals that wasn't a go-ahead: the
    /// imap crate reads it next.
    unread: Vec<u8>,
}

/// Commands with literals of any bytes, through the imap crate's text
/// commands: the command's line ends where its first literal is announced
/// (`{n}`); the rest waits here and is sent, piece by piece, as the server
/// asks for it (RFC 3501 §7.5). Used for APPEND with several messages
/// (MULTIAPPEND), REPLACE and CATENATE.
#[derive(Clone, Default)]
pub(crate) struct Literals(Arc<Mutex<Vec<Vec<u8>>>>);
impl Literals {
    /// The command `parts` make: text and literals alternating, text first.
    /// Returns the first line, to run; the rest is sent from here.
    pub(crate) fn command(&self, parts: &[Part<'_>]) -> String {
        let mut line = String::new();
        let mut queued: Vec<Vec<u8>> = Vec::new();
        let mut current: Vec<u8> = Vec::new();
        let mut first = true;
        for part in parts {
            match part {
                Part::Text(t) => {
                    if first {
                        line.push_str(t)
                    } else {
                        current.extend_from_slice(t.as_bytes())
                    }
                }
                Part::Literal(bytes) => {
                    let size = format!("{{{}}}", bytes.len());
                    if first {
                        line.push_str(&size);
                        first = false;
                    } else {
                        current.extend_from_slice(size.as_bytes());
                        current.extend_from_slice(b"\r\n");
                        queued.push(std::mem::take(&mut current));
                    }
                    current.extend_from_slice(bytes);
                }
            }
        }
        // What follows the last literal, ended as the imap crate ends lines.
        current.extend_from_slice(b"\r\n");
        queued.push(current);
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = queued;
        line
    }
    fn take(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A piece of a command with literals.
pub(crate) enum Part<'a> {
    Text(&'a str),
    Literal(&'a [u8]),
}
impl ImapStream {
    pub(crate) fn new(tls: native_tls::TlsStream<Wire>) -> Self {
        let literals: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        Self {
            tls,
            deflate: None,
            tap: Tap(Arc::default(), Literals(literals.clone())),
            compressing: Arc::new(AtomicBool::new(false)),
            literals,
            unread: Vec::new(),
        }
    }
    /// Reads one line from below the imap crate's buffer, which is empty
    /// between a command and its reply.
    fn line_below(&mut self) -> io::Result<Vec<u8>> {
        let mut line = Vec::new();
        let mut byte = [0u8];
        while !line.ends_with(b"\n") {
            if line.len() > 64 * 1024 || self.read(&mut byte)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "The server closed the connection during a command.",
                ));
            }
            line.push(byte[0]);
        }
        Ok(line)
    }
    /// Sends the queued literals once the command's line has gone: each
    /// after the server's go-ahead (`+`). A refusal (a tagged NO or BAD
    /// instead) is left for the imap crate to read; nothing more is sent.
    fn send_literals(&mut self) -> io::Result<()> {
        let queued = Literals(self.literals.clone()).take();
        for piece in queued {
            let line = self.line_below()?;
            if !line.starts_with(b"+") {
                // Give the answer back to the imap crate's reader.
                self.unread = line;
                return Ok(());
            }
            self.write_plain(&piece)?;
            self.tls.flush()?;
        }
        Ok(())
    }
    /// The TLS channel binding, for SCRAM-PLUS.
    pub(crate) fn binding(&self) -> Vec<u8> {
        super::sasl::binding(&self.tls)
    }
    /// A handle on the copy of what the server sends; see [`Tap`].
    pub(crate) fn tap(&self) -> Tap {
        self.tap.clone()
    }
    /// Turns compression on, from the session that negotiated it.
    pub(crate) fn compression_switch(&self) -> Arc<AtomicBool> {
        self.compressing.clone()
    }
    /// Closes the connection from another thread, ending a waiting read.
    pub(crate) fn interrupter(&self) -> io::Result<std::net::TcpStream> {
        self.tls.get_ref().interrupter()
    }
    fn deflate(&mut self) -> Option<&mut Deflate> {
        if self.deflate.is_none() && self.compressing.load(Ordering::Acquire) {
            self.deflate = Some(Box::new(Deflate::new()));
        }
        self.deflate.as_deref_mut()
    }
}
fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Compressed IMAP stream: {e}"),
    )
}
/// OpenSSL 3 reports a connection closed without TLS's goodbye as an error;
/// IMAP's replies are self-delimiting, so it is the end of the stream, as
/// earlier versions reported it (and the imap crate then sees the
/// connection lost, which a read-only sync may retry).
pub(crate) fn closed_without_goodbye(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::Other && e.to_string().contains("unexpected eof")
}
impl Read for ImapStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.unread.is_empty() {
            let n = buf.len().min(self.unread.len());
            buf[..n].copy_from_slice(&self.unread[..n]);
            self.unread.drain(..n);
            return Ok(n);
        }
        let read = match self.read_plain(buf) {
            Err(e) if closed_without_goodbye(&e) => 0,
            other => other?,
        };
        self.tap.record(&buf[..read]);
        Ok(read)
    }
}
impl ImapStream {
    /// Reads what the server sent, inflated where compressed.
    fn read_plain(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.deflate().is_none() {
            return self.tls.read(buf);
        }
        loop {
            let d = self.deflate.as_deref_mut().unwrap();
            if d.taken < d.output.len() {
                let n = buf.len().min(d.output.len() - d.taken);
                buf[..n].copy_from_slice(&d.output[d.taken..d.taken + n]);
                d.taken += n;
                return Ok(n);
            }
            d.output.clear();
            d.taken = 0;
            if !d.input.is_empty() {
                // Inflate what is buffered; the server flushes after each
                // response, so what arrived usually inflates completely.
                let mut out = vec![0; 32 * 1024];
                let (before_in, before_out) = (d.decompress.total_in(), d.decompress.total_out());
                let status = d
                    .decompress
                    .decompress(&d.input, &mut out, FlushDecompress::Sync)
                    .map_err(invalid)?;
                let used = (d.decompress.total_in() - before_in) as usize;
                let made = (d.decompress.total_out() - before_out) as usize;
                d.input.drain(..used);
                d.spend(made)?;
                out.truncate(made);
                d.output = out;
                if status == Status::StreamEnd {
                    return Err(invalid("the server ended compression"));
                }
                if made > 0 {
                    continue;
                }
                if used > 0 {
                    // Consumed without output yet (block headers): read on.
                    continue;
                }
            }
            let mut chunk = [0; 16 * 1024];
            let read = self.tls.read(&mut chunk)?;
            if read == 0 {
                return Ok(0);
            }
            self.deflate
                .as_deref_mut()
                .unwrap()
                .input
                .extend_from_slice(&chunk[..read]);
        }
    }
}
impl Write for ImapStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_plain(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()?;
        // A command's line went: its literals follow as the server asks.
        if !self.literals.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            self.send_literals()?;
        }
        Ok(())
    }
}
impl ImapStream {
    /// Writes to the server, compressed where COMPRESS is on.
    fn write_plain(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(d) = self.deflate() else {
            return self.tls.write(buf);
        };
        // A new command: its response gets a fresh budget, as on the wire.
        if !buf.is_empty() {
            d.remaining = RESPONSE_LIMIT;
        }
        // Each write is a whole command line or literal; a sync flush sends
        // it at once, as the server must see it to answer.
        let mut out = Vec::with_capacity(buf.len() / 2 + 64);
        let mut consumed = 0;
        loop {
            out.reserve(buf.len() + 64);
            let before = d.compress.total_in();
            d.compress
                .compress_vec(&buf[consumed..], &mut out, FlushCompress::Sync)
                .map_err(invalid)?;
            consumed += (d.compress.total_in() - before) as usize;
            // Done once all input is in and the flush left spare room.
            if consumed == buf.len() && out.len() < out.capacity() {
                break;
            }
        }
        self.tls.write_all(&out)?;
        Ok(buf.len())
    }
}
impl imap::extensions::idle::SetReadTimeout for ImapStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> imap::error::Result<()> {
        self.tls.get_mut().wait_for(timeout);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// What a response inflates to is bounded, not just what it costs on
    /// the wire: a few compressed megabytes can inflate to gigabytes.
    #[test]
    fn inflated_responses_are_bounded() {
        let mut d = Deflate::new();
        d.spend(RESPONSE_LIMIT - 1).unwrap();
        d.spend(1).unwrap();
        assert_eq!(d.spend(1).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
    /// What one side compresses, the other inflates, across many small
    /// writes (as IMAP commands are) and reads of any size.
    #[test]
    fn deflate_round_trips_in_pieces() {
        let mut d = Deflate::new();
        let mut wire = Vec::new();
        let lines: Vec<String> = (0..200)
            .map(|i| format!("a{i} UID FETCH {i}:* (FLAGS) {}\r\n", "x".repeat(i % 37)))
            .collect();
        for line in &lines {
            let mut out = Vec::with_capacity(line.len() + 64);
            d.compress
                .compress_vec(line.as_bytes(), &mut out, FlushCompress::Sync)
                .unwrap();
            wire.extend(out);
        }
        let mut inflate = Decompress::new(false);
        let mut text = vec![0; 1 << 20];
        inflate
            .decompress(&wire, &mut text, FlushDecompress::Sync)
            .unwrap();
        text.truncate(inflate.total_out() as usize);
        assert_eq!(String::from_utf8(text).unwrap(), lines.concat());
    }
}
