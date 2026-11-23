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
    /// The end of the last read, so a `[INPROGRESS` split between two
    /// reads is still seen.
    tail: Vec<u8>,
    /// Literals in extension replies, made readable; see [`Framer`].
    framer: Framer,
}

/// Untagged replies whose literals are rewritten as quoted strings.
///
/// The vendored imap-proto keeps these replies (`resp_other`) as text up to
/// the line's end, so a literal in one (`{n}` CRLF and n bytes; RFC 9051
/// §4.3) cuts it short, and the imap crate then fails the whole command on
/// the literal's bytes. METADATA has a parser of its own, which reads
/// literals as entry names and values but not as the mailbox name.
///
/// A stopgap: once imap-proto reads literals there itself
/// (docs/imap-proto-resp_other-literals.patch, docs/imap-proto-patches.md), `Framer` goes.
const REWRITTEN: [&str; 13] = [
    "NAMESPACE",
    "QUOTA",
    "QUOTAROOT",
    "ACL",
    "MYRIGHTS",
    "LISTRIGHTS",
    "METADATA",
    "ID",
    "ENABLED",
    "ESEARCH",
    "SORT",
    "THREAD",
    "GENURLAUTH",
];
/// The response code of the line that replaces a `REWRITTEN` reply whose
/// literal can't be quoted: `* NO [PICACHO-UNREADABLE NAME] …`. The
/// command's own tagged reply still follows, so the session stays in step;
/// `other_lines` turns the line into an error for that one command.
pub(crate) const UNREADABLE: &str = "PICACHO-UNREADABLE";

/// A piece of an untagged reply kept whole until its last line.
enum Piece {
    Text(Vec<u8>),
    /// A literal: its announcement (`{n}` or `~{n}` with CRLF) and bytes.
    Literal(Vec<u8>, Vec<u8>),
}

/// Finds where the server's responses and their literals begin and end,
/// counting each literal's bytes (`{n}` and literal8's `~{n}`, RFC 3516),
/// so a literal's contents are never read as lines. Responses named in
/// `REWRITTEN` are held until complete and their literals written as
/// quoted strings; everything else passes through unchanged.
#[derive(Default)]
pub(crate) struct Framer {
    /// Bytes ready for the reader, from `taken` on.
    out: Vec<u8>,
    taken: usize,
    /// The line being read, up to its line feed.
    line: Vec<u8>,
    /// Bytes of the current literal still to come.
    literal_left: usize,
    /// The current line continues an untagged response after a literal.
    continuing: bool,
    /// The `REWRITTEN` response being held, with its name.
    held: Option<(String, Vec<Piece>)>,
}
impl Framer {
    /// Takes bytes as the server sent them.
    pub(crate) fn feed(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            if self.literal_left > 0 {
                let n = self.literal_left.min(input.len());
                match &mut self.held {
                    // A held literal always follows its announcement.
                    Some((_, pieces)) => {
                        if let Some(Piece::Literal(_, data)) = pieces.last_mut() {
                            data.extend_from_slice(&input[..n]);
                        }
                    }
                    None => self.out.extend_from_slice(&input[..n]),
                }
                self.literal_left -= n;
                input = &input[n..];
                continue;
            }
            match memchr::memchr(b'\n', input) {
                Some(at) => {
                    self.line.extend_from_slice(&input[..=at]);
                    input = &input[at + 1..];
                    self.end_line();
                }
                None => {
                    self.line.extend_from_slice(input);
                    input = &[];
                }
            }
        }
    }
    /// What the server sent last before the connection ended, as it is.
    pub(crate) fn finish(&mut self) {
        if let Some((_, pieces)) = self.held.take() {
            for piece in pieces {
                match piece {
                    Piece::Text(t) => self.out.extend(t),
                    Piece::Literal(head, data) => {
                        self.out.extend(head);
                        self.out.extend(data);
                    }
                }
            }
        }
        let line = std::mem::take(&mut self.line);
        self.out.extend(line);
        self.literal_left = 0;
        self.continuing = false;
    }
    /// Hands out up to `buf.len()` ready bytes.
    pub(crate) fn take(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.out.len() - self.taken);
        buf[..n].copy_from_slice(&self.out[self.taken..self.taken + n]);
        self.taken += n;
        if self.taken == self.out.len() {
            self.out.clear();
            self.taken = 0;
        }
        n
    }
    fn end_line(&mut self) {
        let line = std::mem::take(&mut self.line);
        if !self.continuing {
            self.held = rewritten_name(&line).map(|name| (name, Vec::new()));
        }
        // Only untagged responses have literals (RFC 9051 §7); a tagged
        // reply's or a continuation's text is text.
        let literal = if self.continuing || (line.starts_with(b"* ") && !status_line(&line)) {
            announced_literal(&line)
        } else {
            None
        };
        self.continuing = literal.is_some();
        match (&mut self.held, literal) {
            (Some((_, pieces)), Some((at, n))) => {
                pieces.push(Piece::Text(line[..at].to_vec()));
                pieces.push(Piece::Literal(line[at..].to_vec(), Vec::with_capacity(n.min(4096))));
                self.literal_left = n;
            }
            (Some(_), None) => {
                let Some((name, mut pieces)) = self.held.take() else {
                    return;
                };
                pieces.push(Piece::Text(line));
                let rewritten = rewrite(&name, pieces);
                self.out.extend(rewritten);
            }
            (None, literal) => {
                self.out.extend(line);
                self.literal_left = literal.map_or(0, |(_, n)| n);
            }
        }
    }
}
/// The name of a `REWRITTEN` untagged response that `line` begins.
fn rewritten_name(line: &[u8]) -> Option<String> {
    let rest = line.strip_prefix(b"* ")?;
    let end = rest
        .iter()
        .position(|b| matches!(b, b' ' | b'\r' | b'\n'))
        .unwrap_or(rest.len());
    let word = std::str::from_utf8(&rest[..end]).ok()?;
    REWRITTEN
        .iter()
        .find(|n| n.eq_ignore_ascii_case(word))
        .map(|n| (*n).to_owned())
}
/// Whether `line` is an untagged status or capability response, whose
/// human-readable text (RFC 9051 §7.1) has no literals even if it ends
/// with what looks like one.
fn status_line(line: &[u8]) -> bool {
    let rest = &line[2..];
    ["OK", "NO", "BAD", "BYE", "PREAUTH", "CAPABILITY"].iter().any(|w| {
        rest.len() >= w.len()
            && rest[..w.len()].eq_ignore_ascii_case(w.as_bytes())
            && matches!(rest.get(w.len()), None | Some(b' ' | b'\r' | b'\n'))
    })
}
/// Where the literal announced at the end of `line` begins (its `{` or
/// `~{`) and how many bytes it has.
fn announced_literal(line: &[u8]) -> Option<(usize, usize)> {
    let body = line.strip_suffix(b"\r\n").or_else(|| line.strip_suffix(b"\n"))?;
    let body = body.strip_suffix(b"}")?;
    let open = body.iter().rposition(|b| *b == b'{')?;
    let digits = &body[open + 1..];
    let digits = digits.strip_suffix(b"+").unwrap_or(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = std::str::from_utf8(digits).ok()?.parse().ok()?;
    let start = if open > 0 && body[open - 1] == b'~' { open - 1 } else { open };
    Some((start, n))
}
/// Whether a literal's bytes can be a quoted string: no line breaks or
/// NUL (RFC 9051 §4.3's QUOTED-CHAR), and text the parser can read.
fn quotable(data: &[u8]) -> bool {
    !data.iter().any(|b| matches!(b, b'\r' | b'\n' | 0)) && std::str::from_utf8(data).is_ok()
}
/// A held response, with its literals as quoted strings; or, where one
/// can't be, a line saying so (`UNREADABLE`).
fn rewrite(name: &str, pieces: Vec<Piece>) -> Vec<u8> {
    let mut out = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(t) => out.extend(t),
            // RFC 9051 §4.3: quoted-specials are escaped.
            Piece::Literal(_, data)
                if quotable(&data) && !(name == "METADATA" && needs_escape(&data)) =>
            {
                out.push(b'"');
                for b in data {
                    if matches!(b, b'"' | b'\\') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
                out.push(b'"');
            }
            // METADATA's parser reads literal values (with line breaks)
            // itself, and keeps quoted values' escapes as they are: what
            // it reads, or needs escaping, stays a literal.
            // Its parser can't take bytes that aren't text, though.
            Piece::Literal(head, data)
                if name == "METADATA" && std::str::from_utf8(&data).is_ok() =>
            {
                out.extend(head);
                out.extend(data);
            }
            Piece::Literal(..) => {
                return format!(
                    "* NO [{UNREADABLE} {name}] The server's {name} reply has a string with a line break or bytes that aren't text, which Picacho can't read.\r\n"
                )
                .into_bytes();
            }
        }
    }
    out
}
fn needs_escape(data: &[u8]) -> bool {
    data.iter().any(|b| matches!(b, b'"' | b'\\'))
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
            tail: Vec::new(),
            framer: Framer::default(),
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
        // Untagged lines may come before the go-ahead (RFC 3501 §5.5:
        // EXISTS, FETCH…); they are kept for the imap crate, in order.
        let mut held = Vec::new();
        let result = (|| {
            for piece in queued {
                loop {
                    let line = self.line_below()?;
                    if line.starts_with(b"+") {
                        break;
                    }
                    let untagged = line.starts_with(b"* ");
                    held.extend_from_slice(&line);
                    if untagged {
                        // Its own literal, if it ends with one, is read
                        // whole before the next line.
                        if let Some(n) = trailing_literal(&line) {
                            for _ in 0..n {
                                let mut byte = [0u8];
                                if self.read(&mut byte)? == 0 {
                                    return Err(io::Error::new(
                                        io::ErrorKind::UnexpectedEof,
                                        "The server closed the connection during a command.",
                                    ));
                                }
                                held.push(byte[0]);
                            }
                        }
                        continue;
                    }
                    // A refusal (tagged NO or BAD): nothing more is sent.
                    return Ok(());
                }
                self.write_plain(&piece)?;
                self.tls.flush()?;
            }
            Ok(())
        })();
        // Give what was read back to the imap crate's reader.
        held.append(&mut self.unread);
        self.unread = held;
        result
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
/// `{n}` (or `{n+}`) ending a line: a literal of `n` bytes follows.
fn trailing_literal(line: &[u8]) -> Option<usize> {
    let line = line.strip_suffix(b"\r\n").or_else(|| line.strip_suffix(b"\n"))?;
    let line = line.strip_suffix(b"}")?;
    let open = line.iter().rposition(|b| *b == b'{')?;
    let digits = &line[open + 1..];
    let digits = digits.strip_suffix(b"+").unwrap_or(digits);
    std::str::from_utf8(digits).ok()?.parse().ok()
}
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
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
        if buf.is_empty() {
            return Ok(0);
        }
        // What the server sent, framed (see `Framer`), until some is ready.
        let read = loop {
            let ready = self.framer.take(buf);
            if ready > 0 {
                break ready;
            }
            let mut chunk = [0u8; 16 * 1024];
            let got = match self.read_plain(&mut chunk) {
                Err(e) if closed_without_goodbye(&e) => 0,
                other => other?,
            };
            if got == 0 {
                self.framer.finish();
                break self.framer.take(buf);
            }
            self.framer.feed(&chunk[..got]);
        };
        self.tap.record(&buf[..read]);
        // `* OK [INPROGRESS ("tag" n total)]`: still working (RFC 9585;
        // servers listing INPROGRESS send it unasked). A mention elsewhere
        // (inside a message) only extends the time by one more budget,
        // still bounded by the byte limit.
        if read > 0 {
            let mut seen = std::mem::take(&mut self.tail);
            seen.extend_from_slice(&buf[..read]);
            if contains(&seen, b"[INPROGRESS") {
                self.tls.get_mut().progressed();
            }
            let keep = seen.len().min(b"[INPROGRESS".len() - 1);
            self.tail = seen[seen.len() - keep..].to_vec();
        }
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
                if status == Status::StreamEnd && made == 0 {
                    return Err(invalid("the server ended compression"));
                }
                // An end of the stream with its last bytes: those are
                // handed out first; the end is reported on the next read.
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
    #[test]
    fn literals_ending_lines_are_found() {
        assert_eq!(trailing_literal(b"* 1 FETCH (BODY[] {12}\r\n"), Some(12));
        assert_eq!(trailing_literal(b"* LIST () \"/\" {4+}\r\n"), Some(4));
        assert_eq!(trailing_literal(b"* 1 EXISTS\r\n"), None);
    }
    use super::*;
    /// What `input` reads as through a `Framer`, fed in pieces of `step`.
    fn framed(input: &[u8], step: usize) -> Vec<u8> {
        let mut f = Framer::default();
        let mut out = Vec::new();
        for piece in input.chunks(step) {
            f.feed(piece);
            let mut buf = [0u8; 7];
            loop {
                let n = f.take(&mut buf);
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
        }
        f.finish();
        let mut buf = vec![0u8; 1 << 16];
        let n = f.take(&mut buf);
        out.extend_from_slice(&buf[..n]);
        out
    }
    /// RFC 9051 §4.3: an extension reply's literal is read as a string,
    /// however the bytes arrive.
    #[test]
    fn extension_reply_literals_become_quoted_strings() {
        let sent = b"* NAMESPACE (({5}\r\nINBOX \"/\")) NIL NIL\r\na1 OK done\r\n";
        let want = b"* NAMESPACE ((\"INBOX\" \"/\")) NIL NIL\r\na1 OK done\r\n".to_vec();
        for step in [1, 2, 3, 5, 11, sent.len()] {
            assert_eq!(framed(sent, step), want, "step {step}");
        }
        // Quoted-specials are escaped; literal8 and LITERAL+ forms count too.
        assert_eq!(
            framed(b"* MYRIGHTS {6}\r\na\"b\\cd ~{2}\r\nlr\r\n", 4),
            b"* MYRIGHTS \"a\\\"b\\\\cd\" \"lr\"\r\n"
        );
        assert_eq!(
            framed(b"* QUOTA {2+}\r\nab (STORAGE 1 2)\r\n", 3),
            b"* QUOTA \"ab\" (STORAGE 1 2)\r\n"
        );
        // A literal that can't be a quoted string stands for the reply as
        // a refusal, before the command's own tagged reply.
        let broken = framed(b"* NAMESPACE (({7}\r\nIN\r\nBOX \"/\")) NIL NIL\r\na1 OK done\r\n", 3);
        let text = String::from_utf8(broken).unwrap();
        assert!(text.starts_with("* NO [PICACHO-UNREADABLE NAMESPACE] "), "{text}");
        assert!(text.ends_with("\r\na1 OK done\r\n"), "{text}");
        assert_eq!(text.matches("\r\n").count(), 2, "{text}");
        let bytes = framed(b"* ID ({2}\r\n\xff\xfe NIL)\r\n", 1);
        assert!(bytes.starts_with(b"* NO [PICACHO-UNREADABLE ID]"));
    }
    /// Everything else passes byte for byte, and literals' contents are
    /// never taken for responses (RFC 9051 §4.3).
    #[test]
    fn other_responses_and_message_literals_pass_unchanged() {
        let body = b"* NAMESPACE ((\"x\" \"/\")) {5}\r\n\r\n* OK [ALERT] fake\r\n";
        let mut sent = format!("* 1 FETCH (UID 4 BODY[] {{{}}}\r\n", body.len()).into_bytes();
        sent.extend_from_slice(body);
        sent.extend_from_slice(b" FLAGS (\\Seen))\r\n* 2 FETCH (BINARY[1] ~{3}\r\n{1}\r\n)\r\n");
        sent.extend_from_slice(b"* OK [CAPABILITY IMAP4rev1] text {3}\r\n");
        sent.extend_from_slice(b"* LIST () \"/\" {4}\r\nWork\r\n");
        sent.extend_from_slice(b"+ go ahead {9}\r\na2 OK [READ-WRITE] {4}\r\n");
        for step in [1, 3, 8, sent.len()] {
            assert_eq!(framed(&sent, step), sent, "step {step}");
        }
        // A METADATA value with a line break stays a literal its own
        // parser reads; its quotable ones become quoted strings.
        let metadata = b"* METADATA \"\" (/shared/comment {7}\r\nA\r\nnote /shared/admin {4}\r\nroot)\r\n";
        let once = framed(metadata, 2);
        assert_eq!(
            once,
            b"* METADATA \"\" (/shared/comment {7}\r\nA\r\nnote /shared/admin \"root\")\r\n"
        );
        assert!(matches!(
            imap_proto::parse_response(&once),
            Ok((rest, imap_proto::Response::MailboxData(_))) if rest.is_empty()
        ));
    }
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
