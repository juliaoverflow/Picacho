//! A small WebSocket client (RFC 6455) over TLS, for JMAP over WebSocket
//! (RFC 8887): the opening handshake, text frames out (masked, as clients
//! must send them), and text, ping and close frames in. The one extension
//! is permessage-deflate (RFC 7692), offered without context takeover in
//! either direction, so each message is compressed on its own. No binary
//! messages.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha1::Digest;
use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

/// RFC 6455's key suffix, proving the server understood the handshake.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// The largest message read; JMAP's state changes are small.
const MOST: u64 = 1 << 20;

/// What arrived.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Text(String),
    /// The answer to a ping: the link is alive.
    Pong,
    /// Nothing within the read timeout.
    Idle,
    Closed,
}

/// An open WebSocket.
pub(crate) struct Socket<S: Read + Write> {
    stream: BufReader<S>,
    /// permessage-deflate was agreed (RFC 7692): messages may come
    /// compressed (RSV1), and are sent so.
    deflate: bool,
}

/// What the handshake offers (RFC 7692 §7): permessage-deflate, with each
/// message compressed on its own both ways, so neither side keeps a window
/// between messages.
const OFFER: &str = "permessage-deflate; client_no_context_takeover; server_no_context_takeover";

/// Whether the server's `Sec-WebSocket-Extensions` answer is one this
/// client can take: permessage-deflate with no context takeover by the
/// server (which was asked for), and no window sizes other than the
/// largest (RFC 7692 §7.1). `Err` for anything else, which fails the
/// connection (RFC 6455 §9.1).
fn agreed_deflate(answer: &str) -> Result<bool> {
    if answer.trim().is_empty() {
        return Ok(false);
    }
    ensure!(!answer.contains(','), "The JMAP server agreed to more than one WebSocket extension.");
    let mut parts = answer.split(';').map(str::trim);
    ensure!(
        parts.next().is_some_and(|n| n.eq_ignore_ascii_case("permessage-deflate")),
        "The JMAP server turned on a WebSocket extension that wasn't asked for."
    );
    let mut server_no_takeover = false;
    for p in parts {
        let (name, value) = p.split_once('=').map_or((p, ""), |(n, v)| (n.trim(), v.trim().trim_matches('"')));
        match name.to_ascii_lowercase().as_str() {
            "server_no_context_takeover" => server_no_takeover = true,
            "client_no_context_takeover" => {}
            // 15 is the largest window, which the inflater assumes.
            "server_max_window_bits" | "client_max_window_bits" => {
                ensure!(value.is_empty() || value == "15", "The JMAP server asked for a smaller compression window than Picacho uses.")
            }
            _ => bail!("The JMAP server agreed to WebSocket compression with a setting Picacho doesn't know."),
        }
    }
    ensure!(server_no_takeover, "The JMAP server agreed to WebSocket compression without what was asked.");
    Ok(true)
}

/// RFC 7692 §7.2.1: a message compressed with its final 4 bytes (an empty
/// stored block's `00 00 ff ff`) taken off.
fn compress(text: &[u8]) -> Result<Vec<u8>> {
    let mut d = flate2::Compress::new(flate2::Compression::default(), false);
    let mut out = Vec::with_capacity(text.len() + 64);
    // A sync flush ends with the empty stored block once all is in; more
    // room is given until it does.
    loop {
        d.compress_vec(&text[d.total_in() as usize..], &mut out, flate2::FlushCompress::Sync)?;
        if d.total_in() as usize == text.len() && out.ends_with(&[0, 0, 0xff, 0xff]) {
            break;
        }
        out.reserve(out.capacity().max(64));
    }
    out.truncate(out.len() - 4);
    Ok(out)
}
/// RFC 7692 §7.2.2: a compressed message, its final 4 bytes put back,
/// inflated; no more than `MOST` bytes.
fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut d = flate2::Decompress::new(false);
    let input: Vec<u8> = data.iter().copied().chain([0, 0, 0xff, 0xff]).collect();
    let mut out = Vec::with_capacity(data.len() * 3);
    loop {
        out.reserve(16 * 1024);
        let before = (d.total_in(), d.total_out());
        d.decompress_vec(&input[d.total_in() as usize..], &mut out, flate2::FlushDecompress::Sync)
            .context("The JMAP server sent a WebSocket message that doesn't inflate.")?;
        ensure!(out.len() as u64 <= MOST, "The JMAP server sent too large a WebSocket message.");
        if d.total_in() as usize >= input.len() || (d.total_in(), d.total_out()) == before {
            break;
        }
    }
    Ok(out)
}

/// The `Sec-WebSocket-Accept` a server must answer `key` with.
fn accept_for(key: &str) -> String {
    let mut sha = sha1::Sha1::new();
    sha.update(key.as_bytes());
    sha.update(GUID.as_bytes());
    STANDARD.encode(sha.finalize())
}

/// Connects to a `wss://` address, verifying the server with `tls`, and
/// opens the WebSocket with `protocol`, sending `authorization`.
pub(crate) fn connect(
    url: &str,
    protocol: &str,
    authorization: &str,
    tls: &native_tls::TlsConnector,
    timeout: Duration,
) -> Result<Socket<native_tls::TlsStream<std::net::TcpStream>>> {
    let parsed = url::Url::parse(url).context("The WebSocket address isn't valid.")?;
    ensure!(
        parsed.scheme() == "wss",
        "The WebSocket address doesn't use wss://."
    );
    let host = parsed
        .host_str()
        .context("The WebSocket address has no host.")?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let tcp = open(host, port)?;
    // Reads wait this long for the server before coming back `Idle`.
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))?;
    let stream = tls
        .connect(host, tcp)
        .map_err(|e| anyhow::anyhow!("TLS with {host} failed: {e}"))?;
    let mut path = parsed.path().to_owned();
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }
    let authority = match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    };
    handshake(stream, &authority, &path, protocol, authorization)
}

/// A TCP connection to the first of `host`'s addresses that answers.
fn open(host: &str, port: u16) -> Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = None;
    for addr in (host, port)
        .to_socket_addrs()
        .context(crate::sync::retry::Unreachable(format!(
            "{host} could not be resolved"
        )))?
    {
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(crate::sync::retry::Unreachable(format!(
        "Could not connect to {host}:{port}: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
    .into())
}

/// The opening handshake over `stream`.
pub(crate) fn handshake<S: Read + Write>(
    stream: S,
    authority: &str,
    path: &str,
    protocol: &str,
    authorization: &str,
) -> Result<Socket<S>> {
    // Each is put on a line of its own: a line break in one (the session
    // names the address) would add headers of its own.
    ensure!(
        ![authority, path, protocol, authorization].iter().any(|v| v.contains(['\r', '\n'])),
        "The JMAP server's WebSocket address isn't valid."
    );
    let key = STANDARD.encode(rand::random::<[u8; 16]>());
    let mut stream = BufReader::new(stream);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: {protocol}\r\nSec-WebSocket-Extensions: {OFFER}\r\nAuthorization: {authorization}\r\nUser-Agent: Picacho/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    stream.get_mut().write_all(request.as_bytes())?;
    stream.get_mut().flush()?;
    let mut status = String::new();
    stream.by_ref().take(4096).read_line(&mut status)?;
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code == "401" || code == "403" {
        return Err(
            anyhow::Error::new(crate::manage::status::AccessFailure::Authentication)
                .context("The JMAP server no longer accepts the sign-in."),
        );
    }
    ensure!(
        code == "101",
        "The JMAP server didn't open the WebSocket: {}",
        status.trim()
    );
    let mut accept = None;
    let mut chosen = None;
    let mut upgrade = false;
    let mut connection = false;
    let mut extensions = String::new();
    let mut ended = false;
    for _ in 0..100 {
        let mut line = String::new();
        stream.by_ref().take(8192).read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            ended = true;
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_owned();
            match name.trim().to_ascii_lowercase().as_str() {
                "sec-websocket-accept" => accept = Some(value),
                "sec-websocket-protocol" => chosen = Some(value),
                "upgrade" => upgrade |= value.eq_ignore_ascii_case("websocket"),
                "connection" => {
                    connection |= value
                        .split(',')
                        .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
                }
                // RFC 6455 §4.1: only what was offered (permessage-deflate).
                "sec-websocket-extensions" => {
                    if !extensions.is_empty() && !value.is_empty() {
                        extensions.push_str(", ");
                    }
                    extensions.push_str(&value);
                }
                _ => {}
            }
        }
    }
    // More headers than that, or the connection's end: the rest would have
    // been read as frames.
    ensure!(ended, "The JMAP server's WebSocket answer is too long.");
    // RFC 6455 §4.1: the answer must upgrade to WebSocket.
    ensure!(
        upgrade && connection,
        "The JMAP server's answer didn't upgrade to a WebSocket."
    );
    let deflate = agreed_deflate(&extensions)?;
    ensure!(
        accept.as_deref() == Some(accept_for(&key).as_str()),
        "The JMAP server's WebSocket answer doesn't match the request."
    );
    ensure!(
        chosen.as_deref() == Some(protocol),
        "The JMAP server didn't agree to speak JMAP over the WebSocket."
    );
    Ok(Socket { stream, deflate })
}

impl<S: Read + Write> Socket<S> {
    /// Sends a ping, keeping the connection open through quiet times.
    pub(crate) fn ping(&mut self) -> Result<()> {
        self.send_frame(0x9, b"")
    }

    /// Closes the WebSocket from this side (RFC 6455 §5.5.1): a normal
    /// closure (1000).
    pub(crate) fn close(&mut self) -> Result<()> {
        self.send_frame(0x8, &1000u16.to_be_bytes())
    }

    /// Sends a text message, masked as RFC 6455 asks of clients, and
    /// compressed where that was agreed and makes it smaller.
    pub(crate) fn send_text(&mut self, text: &str) -> Result<()> {
        if self.deflate && text.len() > 64 {
            let packed = compress(text.as_bytes())?;
            if packed.len() < text.len() {
                // RFC 7692 §6: RSV1 on the first frame marks it compressed.
                return self.send_frame_with(0x1 | 0x40, &packed);
            }
        }
        self.send_frame(0x1, text.as_bytes())
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        self.send_frame_with(opcode, payload)
    }
    /// A frame whose first byte, past FIN, is `bits` (the opcode, and RSV1
    /// for a compressed message).
    fn send_frame_with(&mut self, bits: u8, payload: &[u8]) -> Result<()> {
        let mut frame = vec![0x80 | bits];
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask: [u8; 4] = rand::random();
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.get_mut().write_all(&frame)?;
        self.stream.get_mut().flush()?;
        Ok(())
    }

    /// The next message. Pings are answered on the way; a read timeout
    /// before a frame starts is `Idle`.
    pub(crate) fn receive(&mut self) -> Result<Incoming> {
        let mut message = Vec::new();
        // The opcode of the message being put together from fragments.
        let mut started: Option<u8> = None;
        // Its first frame had RSV1: it is compressed (RFC 7692 §6).
        let mut compressed = false;
        loop {
            let mut head = [0u8; 2];
            match self.stream.read_exact(&mut head[..1]) {
                Ok(()) => {}
                Err(e)
                    if message.is_empty()
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                {
                    return Ok(Incoming::Idle);
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(Incoming::Closed);
                }
                Err(e) => return Err(e.into()),
            }
            self.stream.read_exact(&mut head[1..])?;
            let fin = head[0] & 0x80 != 0;
            let opcode = head[0] & 0x0f;
            // RSV2 and RSV3 are never in use; RSV1 only with permessage-
            // deflate, on a message's first frame (RFC 6455 §5.2, RFC 7692
            // §6).
            let rsv1 = head[0] & 0x40 != 0;
            ensure!(
                head[0] & 0x30 == 0 && (!rsv1 || (self.deflate && matches!(opcode, 0x1 | 0x2))),
                "The JMAP server sent a WebSocket frame with reserved bits set."
            );
            ensure!(
                head[1] & 0x80 == 0,
                "The JMAP server sent a masked WebSocket frame."
            );
            let len = match head[1] & 0x7f {
                126 => {
                    let mut n = [0u8; 2];
                    self.stream.read_exact(&mut n)?;
                    u16::from_be_bytes(n) as u64
                }
                127 => {
                    let mut n = [0u8; 8];
                    self.stream.read_exact(&mut n)?;
                    u64::from_be_bytes(n)
                }
                n => n as u64,
            };
            ensure!(
                len <= MOST && message.len() as u64 + len <= MOST,
                "The JMAP server sent too large a WebSocket message."
            );
            // Control frames (close, ping, pong) are short and whole (RFC 6455
            // §5.5): one claiming a megabyte is held, and echoed, otherwise.
            ensure!(
                opcode < 0x8 || (len <= 125 && fin),
                "The JMAP server sent an invalid WebSocket control frame."
            );
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload)?;
            match opcode {
                // A ping is answered with its payload.
                0x9 => self.send_frame(0xA, &payload)?,
                0xA if message.is_empty() => return Ok(Incoming::Pong),
                0xA => {}
                0x8 => {
                    // A close's body is a status code and UTF-8 reason
                    // (RFC 6455 §5.5.1); one of a single byte is invalid.
                    let valid = payload.is_empty()
                        || (payload.len() >= 2 && std::str::from_utf8(&payload[2..]).is_ok());
                    let answer: &[u8] = if valid { &payload[..payload.len().min(2)] } else { &[0x03, 0xEA] };
                    let _ = self.send_frame(0x8, answer);
                    return Ok(Incoming::Closed);
                }
                0x0..=0x2 => {
                    // A continuation only within a message; a new message
                    // only after the last ended (RFC 6455 §5.4).
                    let kind = match (opcode, started) {
                        (0x0, Some(kind)) => kind,
                        (0x0, None) => bail!("The JMAP server sent a WebSocket continuation without a message."),
                        (_, Some(_)) => bail!("The JMAP server started a WebSocket message inside another."),
                        (kind, None) => kind,
                    };
                    if kind == 0x2 {
                        bail!("The JMAP server sent a binary WebSocket message.");
                    }
                    if started.is_none() {
                        compressed = rsv1;
                    }
                    started = Some(kind);
                    message.extend_from_slice(&payload);
                    if fin {
                        if compressed {
                            message = decompress(&message)?;
                        }
                        return Ok(Incoming::Text(
                            String::from_utf8(message)
                                .context("The JMAP server sent text that isn't UTF-8.")?,
                        ));
                    }
                }
                _ => bail!("The JMAP server sent an unknown WebSocket frame."),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A stream that hands out `input` and keeps what's written.
    struct Fake {
        input: VecDeque<u8>,
        output: Vec<u8>,
    }
    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.input.len());
            for b in buf.iter_mut().take(n) {
                *b = self.input.pop_front().unwrap();
            }
            Ok(n)
        }
    }
    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The handshake against a server that answers it, then JMAP's push
    /// request out and a state change in.
    #[test]
    fn handshake_checks_the_servers_answer() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let mut r = BufReader::new(socket);
            let mut key = String::new();
            let mut auth = String::new();
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.strip_prefix("Sec-WebSocket-Key: ") {
                    key = v.trim().to_owned();
                }
                if let Some(v) = line.strip_prefix("Authorization: ") {
                    auth = v.trim().to_owned();
                }
            }
            assert_eq!(auth, "Bearer t");
            write!(
                r.get_mut(),
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\nSec-WebSocket-Protocol: jmap\r\n\r\n",
                accept_for(&key)
            )
            .unwrap();
            // The client's (masked) push request.
            let mut head = [0u8; 2];
            r.read_exact(&mut head).unwrap();
            assert_eq!(head[0], 0x81);
            let len = (head[1] & 0x7f) as usize;
            let mut rest = vec![0u8; 4 + len];
            r.read_exact(&mut rest).unwrap();
            let text: Vec<u8> = rest[4..]
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ rest[i % 4])
                .collect();
            assert!(
                String::from_utf8(text)
                    .unwrap()
                    .contains("WebSocketPushEnable")
            );
            let change = br#"{"@type":"StateChange","changed":{"u1":{"Email":"2"}}}"#;
            r.get_mut().write_all(&[0x81, change.len() as u8]).unwrap();
            r.get_mut().write_all(change).unwrap();
        });
        let socket = std::net::TcpStream::connect(addr).unwrap();
        let mut ws = handshake(socket, "localhost", "/jmap/ws", "jmap", "Bearer t").unwrap();
        ws.send_text(r#"{"@type":"WebSocketPushEnable","dataTypes":["Email"]}"#)
            .unwrap();
        match ws.receive().unwrap() {
            Incoming::Text(t) => assert!(t.contains("StateChange")),
            other => panic!("{other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn accept_key_is_the_rfcs_example() {
        assert_eq!(
            accept_for("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// permessage-deflate (RFC 7692): only the offered answer is taken;
    /// messages compressed by the server are read, and sent compressed.
    #[test]
    fn permessage_deflate_both_ways() {
        assert!(!agreed_deflate("").unwrap());
        assert!(agreed_deflate("permessage-deflate; server_no_context_takeover").unwrap());
        assert!(agreed_deflate("permessage-deflate; server_no_context_takeover; client_no_context_takeover; server_max_window_bits=15").unwrap());
        assert!(agreed_deflate("permessage-deflate").is_err(), "server takeover wasn't agreed");
        assert!(agreed_deflate("permessage-deflate; server_no_context_takeover; server_max_window_bits=10").is_err());
        assert!(agreed_deflate("x-webkit-deflate-frame").is_err());
        // RFC 7692 §7.2.3.1's example: "Hello" compressed.
        assert_eq!(decompress(&[0xf2, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]).unwrap(), b"Hello");
        let text = r#"{"@type":"StateChange","changed":{"u1":{"Email":"s2","Mailbox":"m2","Thread":"t2"}}}"#.repeat(3);
        let packed = compress(text.as_bytes()).unwrap();
        assert!(packed.len() < text.len());
        assert_eq!(decompress(&packed).unwrap(), text.as_bytes());
        // A compressed message in, in two frames (RSV1 on the first).
        let (a, b) = packed.split_at(packed.len() / 2);
        let mut input = vec![0x41, a.len() as u8];
        input.extend_from_slice(a);
        input.extend_from_slice(&[0x80, b.len() as u8]);
        input.extend_from_slice(b);
        let mut socket = Socket {
            stream: BufReader::new(Fake { input: input.into(), output: Vec::new() }),
            deflate: true,
        };
        assert_eq!(socket.receive().unwrap(), Incoming::Text(text.clone()));
        // Out: compressed, RSV1 set.
        socket.send_text(&text).unwrap();
        assert_eq!(socket.stream.get_ref().output[0], 0xC1);
        // RSV1 without the extension is refused.
        let mut plain = Socket {
            stream: BufReader::new(Fake { input: vec![0xC1, 1, b'x'].into(), output: Vec::new() }),
            deflate: false,
        };
        assert!(plain.receive().is_err());
    }

    #[test]
    fn frames_are_masked_out_and_read_in_with_pings_answered() {
        let mut socket = Socket {
            deflate: false,
            stream: BufReader::new(Fake {
                // A ping "hi", then "{}" in two fragments, then a close.
                input: [
                    &[0x89, 2, b'h', b'i'][..],
                    &[0x01, 1, b'{'],
                    &[0x80, 1, b'}'],
                    &[0x88, 0],
                ]
                .concat()
                .into(),
                output: Vec::new(),
            }),
        };
        socket.send_text("ok").unwrap();
        let out = socket.stream.get_ref().output.clone();
        assert_eq!(out[0], 0x81);
        assert_eq!(out[1], 0x80 | 2);
        let mask = &out[2..6];
        assert_eq!([out[6] ^ mask[0], out[7] ^ mask[1]], *b"ok");
        assert_eq!(socket.receive().unwrap(), Incoming::Text("{}".into()));
        // The ping was answered with a pong carrying "hi".
        let pong = &socket.stream.get_ref().output[8..];
        assert_eq!(pong[0], 0x8A);
        assert_eq!([pong[6] ^ pong[2], pong[7] ^ pong[3]], *b"hi");
        assert_eq!(socket.receive().unwrap(), Incoming::Closed);
    }
}
