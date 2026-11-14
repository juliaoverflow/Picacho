//! The byte stream an IMAP session runs over: verified TLS on a bounded
//! [`Wire`], optionally compressed (COMPRESS=DEFLATE, RFC 4978), and able to
//! wait longer for the server while a session is in IDLE.
use super::wire::Wire;
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
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
        }
    }
}

pub(crate) struct ImapStream {
    tls: native_tls::TlsStream<Wire>,
    deflate: Option<Box<Deflate>>,
    /// Set once the server accepted COMPRESS; compression starts with the
    /// next byte in either direction.
    compressing: Arc<AtomicBool>,
}
impl ImapStream {
    pub(crate) fn new(tls: native_tls::TlsStream<Wire>) -> Self {
        Self {
            tls,
            deflate: None,
            compressing: Arc::new(AtomicBool::new(false)),
        }
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
impl Read for ImapStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
        let Some(d) = self.deflate() else {
            return self.tls.write(buf);
        };
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
    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
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
