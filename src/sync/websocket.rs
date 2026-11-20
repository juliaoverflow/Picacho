//! A small WebSocket client (RFC 6455) over TLS, for JMAP's pushed changes
//! (RFC 8887): the opening handshake, text frames out (masked, as clients
//! must send them), and text, ping and close frames in. Nothing else JMAP
//! needs here: no extensions, no binary messages.
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
    /// Nothing within the read timeout.
    Idle,
    Closed,
}

/// An open WebSocket.
pub(crate) struct Socket<S: Read + Write> {
    stream: BufReader<S>,
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
    let key = STANDARD.encode(rand::random::<[u8; 16]>());
    let mut stream = BufReader::new(stream);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: {protocol}\r\nAuthorization: {authorization}\r\nUser-Agent: Picacho/{}\r\n\r\n",
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
    for _ in 0..100 {
        let mut line = String::new();
        stream.by_ref().take(8192).read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_owned();
            match name.trim().to_ascii_lowercase().as_str() {
                "sec-websocket-accept" => accept = Some(value),
                "sec-websocket-protocol" => chosen = Some(value),
                _ => {}
            }
        }
    }
    ensure!(
        accept.as_deref() == Some(accept_for(&key).as_str()),
        "The JMAP server's WebSocket answer doesn't match the request."
    );
    ensure!(
        chosen.as_deref() == Some(protocol),
        "The JMAP server didn't agree to speak JMAP over the WebSocket."
    );
    Ok(Socket { stream })
}

impl<S: Read + Write> Socket<S> {
    /// Sends a ping, keeping the connection open through quiet times.
    pub(crate) fn ping(&mut self) -> Result<()> {
        self.send_frame(0x9, b"")
    }

    /// Sends a text message, masked as RFC 6455 asks of clients.
    pub(crate) fn send_text(&mut self, text: &str) -> Result<()> {
        self.send_frame(0x1, text.as_bytes())
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mut frame = vec![0x80 | opcode];
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
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload)?;
            match opcode {
                // A ping is answered with its payload.
                0x9 => self.send_frame(0xA, &payload)?,
                0xA => {}
                0x8 => {
                    let _ = self.send_frame(0x8, &payload);
                    return Ok(Incoming::Closed);
                }
                0x0 | 0x1 | 0x2 => {
                    message.extend_from_slice(&payload);
                    if fin {
                        if opcode == 0x2 {
                            bail!("The JMAP server sent a binary WebSocket message.");
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

    #[test]
    fn frames_are_masked_out_and_read_in_with_pings_answered() {
        let mut socket = Socket {
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
