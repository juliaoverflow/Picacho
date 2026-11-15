//! Build a MIME message and deliver it once. Acceptance is the send boundary.
use crate::manage::model::{Account, Draft};
use crate::sync::{sasl, wire::Wire};
use anyhow::{Context, Result, bail, ensure};
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart, header};
use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

/// Without a SIZE limit from the server, messages larger than this aren't
/// sent: most servers refuse them, often only after the upload.
pub(crate) const DEFAULT_LIMIT: usize = crate::sync::transport::MAX_MESSAGE;
/// Identifies a draft's copy on the server, so later versions replace it
/// and other devices running Picacho find the same draft.
pub(crate) const DRAFT_HEADER: &str = "X-Picacho-Draft";
/// Marks a draft written in Markdown, whose text part is its source.
pub(crate) const FORMAT_HEADER: &str = "X-Picacho-Format";

fn raw_header(name: &'static str, value: String) -> header::HeaderValue {
    header::HeaderValue::new(header::HeaderName::new_from_ascii_str(name), value)
}
/// The message a draft makes, as sent (`draft: false`) or as saved in the
/// server's Drafts folder: then incomplete drafts are allowed, Bcc is kept,
/// and Picacho's own headers let it be found again.
pub(crate) fn build(a: &Account, d: &Draft, draft: bool) -> Result<Message> {
    // One of the mailbox's other addresses, unless sent as an alias.
    let identity = match d.identity.as_str() {
        "" => None,
        _ if !d.send_as.trim().is_empty() => None,
        id => Some(a.identity(id).with_context(|| {
            format!(
                "The address this message is from was removed from {} (Settings → Sending). Choose another in From.",
                a.email
            )
        })?),
    };
    let from = match identity {
        Some(i) => Mailbox::new(
            Some(if i.name.trim().is_empty() {
                a.name.clone()
            } else {
                i.name.trim().to_owned()
            }),
            i.email
                .trim()
                .parse()
                .context("The address in From isn't valid.")?,
        ),
        None => Mailbox::new(Some(a.name.clone()), a.email.parse()?),
    };
    let mut builder = Message::builder().from(from.clone()).subject(&d.subject);
    let mut count = 0;
    for (field, list) in [(0, &d.to), (1, &d.cc), (2, &d.bcc)] {
        for (name, address) in recipients(list)? {
            let mailbox = Mailbox::new(name, address.parse()?);
            builder = match field {
                0 => builder.to(mailbox),
                1 => builder.cc(mailbox),
                // Bcc recipients are on the envelope only; lettre leaves the
                // header out of the sent message.
                _ => builder.bcc(mailbox),
            };
            count += 1;
        }
    }
    if draft {
        // Drafts may have no recipients yet; nothing is sent.
        builder = builder
            .keep_bcc()
            .envelope(lettre::address::Envelope::new(
                Some(from.email.clone()),
                vec![from.email.clone()],
            )?)
            .raw_header(raw_header(DRAFT_HEADER, d.id.clone()));
        if d.formatted {
            builder = builder.raw_header(raw_header(FORMAT_HEADER, "markdown".into()));
        }
    } else {
        ensure!(count > 0, "Add at least one recipient.");
    }
    // Sent as an alias, the forwarding service puts the alias in From; the
    // mailbox's own address must not show up elsewhere in the message.
    let alias = d.send_as.trim();
    // Replies go where the message, or else the account, says. The
    // account's Reply-To is its own and isn't used for alias mail.
    let reply_to = if !d.reply_to.trim().is_empty() {
        &d.reply_to
    } else if let Some(i) = identity {
        &i.reply_to
    } else if alias.is_empty() {
        &a.reply_to
    } else {
        ""
    };
    for (name, address) in recipients(reply_to).context("Enter a valid Reply-To address.")? {
        builder = builder.reply_to(Mailbox::new(name, address.parse()?));
    }
    if a.read_receipts && !draft {
        // RFC 8098: the recipient's app may offer to say it was read.
        builder = builder.raw_header(raw_header(
            "Disposition-Notification-To",
            format!(
                "<{}>",
                match identity {
                    _ if !alias.is_empty() => alias,
                    Some(i) => i.email.trim(),
                    None => &a.email,
                }
            ),
        ));
    }
    if !d.reply_id.is_empty() {
        builder = builder.in_reply_to(d.reply_id.clone());
        let references = if d.references.trim().is_empty() {
            d.reply_id.clone()
        } else {
            d.references.clone()
        };
        builder = builder.references(references);
    }
    let text = if d.formatted {
        MultiPart::alternative_plain_html(d.body.clone(), html_body(&d.body))
    } else {
        MultiPart::alternative().singlepart(SinglePart::plain(d.body.clone()))
    };
    let mut multipart = MultiPart::mixed().multipart(text);
    let mut total = d.body.len();
    for path in &d.attachments {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("The attachment {} can't be read.", path.display()))?;
        total += meta.len() as usize;
        // The server's own limit is checked once connected; this only
        // keeps absurd files out of memory.
        ensure!(
            total < 4 * DEFAULT_LIMIT,
            "Attachments are too large to send by email. Share them by link instead."
        );
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        multipart = multipart.singlepart(attachment_part(name, bytes, content_type(path)));
    }
    Ok(builder.multipart(multipart)?)
}
/// One attached file. A forwarded message (message/rfc822) goes as it is
/// (7bit or 8bit, as RFC 2046 asks of messages), so recipients' apps open it
/// as a message; when its lines are too long for that, it is encoded like any
/// other file.
fn attachment_part(name: String, bytes: Vec<u8>, kind: header::ContentType) -> SinglePart {
    use lettre::message::{Body, header::ContentTransferEncoding};
    if kind == header::ContentType::parse("message/rfc822").unwrap() {
        let encoding = if bytes.is_ascii() {
            ContentTransferEncoding::SevenBit
        } else {
            ContentTransferEncoding::EightBit
        };
        if let Ok(body) = Body::new_with_encoding(bytes.clone(), encoding) {
            return SinglePart::builder()
                .header(kind)
                .header(header::ContentDisposition::attachment(&name))
                .body(body);
        }
    }
    Attachment::new(name).body(bytes, kind)
}
/// An attachment's type from its name (image/png, application/pdf…), so
/// recipients' apps show it; unknown kinds are sent as plain bytes.
pub(crate) fn content_type(path: &std::path::Path) -> header::ContentType {
    let guess = mime_guess::from_path(path).first_or_octet_stream();
    header::ContentType::parse(guess.essence_str())
        .unwrap_or(header::ContentType::parse("application/octet-stream").unwrap())
}
/// What an outgoing server says it can do, after EHLO.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerLimits {
    /// SIZE (RFC 1870): the largest message it accepts, if it says.
    pub(crate) size: Option<usize>,
    /// DSN (RFC 3461): it can report delivery.
    pub(crate) dsn: bool,
    pub(crate) eight_bit: bool,
    pub(crate) utf8: bool,
    /// PIPELINING (RFC 2920): commands may be sent without waiting for each
    /// reply.
    pub(crate) pipelining: bool,
    /// CHUNKING (RFC 3030): the message may go in BDAT chunks, as is.
    pub(crate) chunking: bool,
    /// REQUIRETLS (RFC 8689): it can insist on TLS all the way.
    pub(crate) require_tls: bool,
    pub(crate) starttls: bool,
    /// SASL mechanisms it takes (AUTH), uppercase.
    pub(crate) auth: Vec<String>,
}
pub(crate) fn server_limits<'a>(lines: impl Iterator<Item = &'a str>) -> ServerLimits {
    let mut limits = ServerLimits::default();
    for line in lines {
        let mut words = line.split_whitespace();
        match words.next().map(str::to_ascii_uppercase).as_deref() {
            // "SIZE 0" (or no number) means no fixed limit.
            Some("SIZE") => {
                limits.size = words.next().and_then(|n| n.parse().ok()).filter(|n| *n > 0)
            }
            Some("DSN") => limits.dsn = true,
            Some("8BITMIME") => limits.eight_bit = true,
            Some("SMTPUTF8") => limits.utf8 = true,
            Some("PIPELINING") => limits.pipelining = true,
            Some("CHUNKING") => limits.chunking = true,
            Some("REQUIRETLS") => limits.require_tls = true,
            Some("STARTTLS") => limits.starttls = true,
            // Some servers still write "AUTH=PLAIN LOGIN".
            Some(word) if word == "AUTH" || word.starts_with("AUTH=") => {
                let first = word.strip_prefix("AUTH=").filter(|w| !w.is_empty());
                for m in first
                    .into_iter()
                    .map(str::to_owned)
                    .chain(words.map(str::to_ascii_uppercase))
                {
                    if !limits.auth.contains(&m) {
                        limits.auth.push(m);
                    }
                }
            }
            _ => {}
        }
    }
    limits
}
/// What happened to a message the server accepted.
pub(crate) struct Delivered {
    pub(crate) message: Message,
    /// Delivery receipts were asked for, but this server can't send them.
    pub(crate) receipts_unsupported: bool,
}
fn megabytes(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// An outgoing server and who signs in to it: the account's own, or one
/// of its sending addresses'.
#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: String,
    /// Signs in with an access token rather than a password.
    pub(crate) oauth: bool,
}
impl Endpoint {
    /// The account's own outgoing server.
    pub(crate) fn of(a: &Account) -> Self {
        Self {
            host: a.smtp_host.clone(),
            port: a.smtp_port,
            user: if a.oauth {
                a.email.clone()
            } else {
                a.login().to_owned()
            },
            oauth: a.oauth,
        }
    }
    /// A sending address's own outgoing server.
    pub(crate) fn of_identity(i: &crate::manage::model::Identity) -> Option<Self> {
        let s = i.smtp.as_ref()?;
        Some(Self {
            host: s.host.clone(),
            port: s.port,
            user: i.login().to_owned(),
            oauth: false,
        })
    }
}

/// One server reply: its code and text lines.
#[derive(Debug)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}
impl Reply {
    fn positive(&self) -> bool {
        (200..400).contains(&self.code)
    }
    fn text(&self) -> String {
        self.lines.join(" ")
    }
}
/// A server's refusal, with what it said.
#[derive(Debug)]
pub(crate) struct Refused(pub(crate) String);
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Refused {}

/// A stream whose reads may be told to wait longer than usual.
trait Patience {
    fn wait_for(&mut self, wait: Option<Duration>);
}
impl Patience for Wire {
    fn wait_for(&mut self, wait: Option<Duration>) {
        Wire::wait_for(self, wait)
    }
}
impl Patience for native_tls::TlsStream<Wire> {
    fn wait_for(&mut self, wait: Option<Duration>) {
        self.get_mut().wait_for(wait)
    }
}

/// An SMTP session: commands out, replies in.
struct Session<S: Read + Write> {
    reader: BufReader<S>,
}
impl<S: Read + Write + Patience> Session<S> {
    fn new(stream: S) -> Self {
        Self {
            reader: BufReader::new(stream),
        }
    }
    fn reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let read = match self.reader.by_ref().take(4097).read_line(&mut line) {
                Err(e) if crate::sync::stream::closed_without_goodbye(&e) => 0,
                other => other?,
            };
            ensure!(read > 0, "The outgoing server closed the connection.");
            ensure!(
                line.ends_with('\n') && line.len() <= 4096 && lines.len() < 512,
                "The outgoing server sent an invalid reply."
            );
            let line = line.trim_end_matches(['\r', '\n']);
            let code: u16 = line
                .get(..3)
                .and_then(|c| c.parse().ok())
                .context("The outgoing server sent an invalid reply.")?;
            let rest = line.get(3..).unwrap_or("");
            lines.push(rest.get(1..).unwrap_or("").to_owned());
            if !rest.starts_with('-') {
                return Ok(Reply { code, lines });
            }
        }
    }
    /// Writes a command without waiting for its reply.
    fn send(&mut self, line: &str) -> Result<()> {
        ensure!(!line.contains(['\r', '\n']), "Invalid SMTP command");
        let stream = self.reader.get_mut();
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\r\n")?;
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        self.reader.get_mut().flush()?;
        Ok(())
    }
    fn command(&mut self, line: &str) -> Result<Reply> {
        self.send(line)?;
        self.flush()?;
        self.reply()
    }
    /// A command that must succeed; its refusal names `what`.
    fn expect(&mut self, line: &str, what: &str) -> Result<Reply> {
        let reply = self.command(line)?;
        if !reply.positive() {
            return Err(Refused(format!("{what}: {} {}", reply.code, reply.text())).into());
        }
        Ok(reply)
    }
    fn ehlo(&mut self) -> Result<ServerLimits> {
        // An address literal, as Thunderbird says hello: the computer's own
        // name would be written into every message's Received header.
        let reply = self.expect("EHLO [127.0.0.1]", "The outgoing server refused EHLO")?;
        Ok(server_limits(
            reply.lines.iter().skip(1).map(String::as_str),
        ))
    }
    /// Signs in with the best mechanism the server offers.
    fn authenticate(
        &mut self,
        endpoint: &Endpoint,
        limits: &ServerLimits,
        secret: &str,
    ) -> Result<()> {
        let order: &[sasl::Mechanism] = if endpoint.oauth {
            &sasl::TOKEN_ORDER
        } else {
            &sasl::PASSWORD_ORDER
        };
        let mechanism = sasl::Mechanism::choose(order, limits.auth.iter().map(String::as_str))
            .with_context(|| {
                format!(
                    "{} offers no sign-in method Picacho supports{}.",
                    endpoint.host,
                    if limits.auth.is_empty() {
                        String::new()
                    } else {
                        format!(" (it offers {})", limits.auth.join(", "))
                    }
                )
            })?;
        let mut exchange = sasl::Exchange::new(
            mechanism,
            sasl::Credentials {
                user: endpoint.user.clone(),
                secret: secret.to_owned(),
                host: endpoint.host.clone(),
                port: endpoint.port,
            },
        );
        // RFC 4954: the first message may go with the command.
        let mut reply = match exchange.initial() {
            Some(first) => self.command(&format!(
                "AUTH {} {}",
                mechanism.name(),
                if first.is_empty() {
                    "=".into()
                } else {
                    sasl::encode(&first)
                }
            ))?,
            None => self.command(&format!("AUTH {}", mechanism.name()))?,
        };
        loop {
            match reply.code {
                235 => {
                    // A server may send SCRAM's signature with its success.
                    let extra = reply
                        .lines
                        .first()
                        .and_then(|t| sasl::decode(t).ok())
                        .filter(|d| d.starts_with(b"v="));
                    return exchange.finish(extra.as_deref());
                }
                334 => {
                    let challenge = sasl::decode(reply.lines.first().map_or("", String::as_str))?;
                    match exchange.step(&challenge) {
                        Ok(answer) => reply = self.command(&sasl::encode(&answer))?,
                        Err(e) => {
                            // "*" cancels; the server answers 501.
                            let _ = self.command("*");
                            return Err(e);
                        }
                    }
                }
                _ => {
                    let why = match exchange.refusal() {
                        Some(reason) => format!("{} ({reason})", reply.text()),
                        None => reply.text(),
                    };
                    return Err(Refused(format!(
                        "{} refused the sign-in with {}: {} {why}",
                        endpoint.host,
                        mechanism.name(),
                        reply.code
                    ))
                    .into());
                }
            }
        }
    }
    /// The message with dots stuffed and the end-of-data line (DATA).
    fn data(&mut self, bytes: &[u8]) -> Result<Reply> {
        let reply = self.command("DATA")?;
        if reply.code != 354 {
            return Err(Refused(format!(
                "The server refused the message: {} {}",
                reply.code,
                reply.text()
            ))
            .into());
        }
        let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 64 + 5);
        let mut line_start = true;
        for &b in bytes {
            if line_start && b == b'.' {
                out.push(b'.');
            }
            out.push(b);
            line_start = b == b'\n';
        }
        if !out.ends_with(b"\r\n") {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b".\r\n");
        self.reader.get_mut().write_all(&out)?;
        self.flush()?;
        self.final_reply()
    }
    /// The message as is, in BDAT chunks (CHUNKING); with PIPELINING all
    /// chunks go before their replies are read.
    fn bdat(&mut self, bytes: &[u8], pipelining: bool) -> Result<Reply> {
        const CHUNK: usize = 1024 * 1024;
        let chunks: Vec<&[u8]> = bytes.chunks(CHUNK).collect();
        let last = chunks.len() - 1;
        let mut pending = 0;
        let mut reply = None;
        for (i, chunk) in chunks.iter().enumerate() {
            let command = if i == last {
                format!("BDAT {} LAST", chunk.len())
            } else {
                format!("BDAT {}", chunk.len())
            };
            self.send(&command)?;
            self.reader.get_mut().write_all(chunk)?;
            pending += 1;
            if !pipelining || i == last {
                self.flush()?;
                while pending > 0 {
                    let r = if pending == 1 && i == last {
                        self.final_reply()?
                    } else {
                        self.reply()?
                    };
                    pending -= 1;
                    if !r.positive() {
                        return Err(Refused(format!(
                            "The server refused the message: {} {}",
                            r.code,
                            r.text()
                        ))
                        .into());
                    }
                    reply = Some(r);
                }
            }
        }
        reply.context("No reply to the message")
    }
    /// The reply to the end of a message: servers may check it first, so
    /// it is waited for longer (RFC 5321 suggests ten minutes).
    fn final_reply(&mut self) -> Result<Reply> {
        self.reader
            .get_mut()
            .wait_for(Some(Duration::from_secs(600)));
        let reply = self.reply();
        self.reader.get_mut().wait_for(None);
        reply
    }
}

/// Connects, secures and signs in to an outgoing server: implicit TLS on
/// port 465, else STARTTLS, never in the clear.
fn open(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<(Session<native_tls::TlsStream<Wire>>, ServerLimits)> {
    let socket = crate::sync::transport::tcp(&endpoint.host, endpoint.port)?;
    let mut session = if endpoint.port == 465 {
        let stream = tls
            .connect(&endpoint.host, socket)
            .map_err(|e| anyhow::anyhow!("TLS with {} failed: {e}", endpoint.host))?;
        let mut session = Session::new(stream);
        let greeting = session.reply()?;
        ensure!(
            greeting.code == 220,
            "{} refused the connection: {}",
            endpoint.host,
            greeting.text()
        );
        session
    } else {
        let mut plain = Session::new(socket);
        let greeting = plain.reply()?;
        ensure!(
            greeting.code == 220,
            "{} refused the connection: {}",
            endpoint.host,
            greeting.text()
        );
        let limits = plain.ehlo()?;
        // Never send credentials or mail unencrypted.
        ensure!(
            limits.starttls,
            "{} doesn't offer STARTTLS on port {}; Picacho only sends over encrypted connections.",
            endpoint.host,
            endpoint.port
        );
        let reply = plain.command("STARTTLS")?;
        ensure!(
            reply.code == 220,
            "{} refused STARTTLS: {}",
            endpoint.host,
            reply.text()
        );
        // Anything sent before the handshake could be an attacker's.
        ensure!(
            plain.reader.buffer().is_empty(),
            "Unexpected data from {} before encryption started; nothing was sent.",
            endpoint.host
        );
        let stream = tls
            .connect(&endpoint.host, plain.reader.into_inner())
            .map_err(|e| anyhow::anyhow!("TLS with {} failed: {e}", endpoint.host))?;
        Session::new(stream)
    };
    let limits = session.ehlo()?;
    session.authenticate(endpoint, &limits, secret)?;
    Ok((session, limits))
}

/// Signs in to an outgoing server and leaves: checks a sending address's
/// own server before it is saved.
pub(crate) fn check_server(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<()> {
    let (mut session, _) = open(endpoint, secret, tls)?;
    let _ = session.command("QUIT");
    Ok(())
}

pub(crate) fn deliver(
    a: &Account,
    d: &Draft,
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<Delivered> {
    let message = build(a, d, false)?;
    let bytes = message.formatted();
    let failed =
        "SMTP delivery failed. Your draft has been kept; check the server and account settings.";
    let (mut session, limits) = open(endpoint, secret, tls).context(failed)?;
    let limit = limits.size.unwrap_or(DEFAULT_LIMIT);
    if bytes.len() > limit {
        bail!(
            "This message is {} with its attachments; {} accepts up to {}. Remove an attachment or share it by link.",
            megabytes(bytes.len()),
            endpoint.host,
            megabytes(limit)
        );
    }
    if a.require_tls {
        ensure!(
            limits.require_tls,
            "{} can't promise this message stays encrypted all the way (it doesn't offer REQUIRETLS), so it wasn't sent. Turn off Require encryption in transit (Settings → {} → Sending) to send it anyway.",
            endpoint.host,
            a.email
        );
    }
    let envelope = message.envelope();
    let from = envelope.from().map(|f| f.to_string()).unwrap_or_default();
    let mut mail = format!("MAIL FROM:<{from}>");
    if limits.size.is_some() {
        mail.push_str(&format!(" SIZE={}", bytes.len()));
    }
    let non_ascii = envelope
        .from()
        .into_iter()
        .chain(envelope.to())
        .any(|address| !AsRef::<str>::as_ref(address).is_ascii());
    if non_ascii {
        ensure!(
            limits.utf8,
            "{} can't send to or from addresses with accents or non-Latin letters (no SMTPUTF8).",
            endpoint.host
        );
        mail.push_str(" SMTPUTF8");
    }
    if !bytes.is_ascii() && limits.eight_bit {
        mail.push_str(" BODY=8BITMIME");
    }
    let receipts = a.delivery_receipts && limits.dsn;
    if receipts {
        // RFC 3461: return headers only, identified by the draft.
        mail.push_str(&format!(" RET=HDRS ENVID={}", xtext(&d.id)));
    }
    if a.require_tls {
        mail.push_str(" REQUIRETLS");
    }
    let rcpt = |to: &lettre::Address| {
        let mut line = format!("RCPT TO:<{to}>");
        if receipts {
            line.push_str(" NOTIFY=SUCCESS,FAILURE,DELAY");
        }
        line
    };
    // With PIPELINING the sender and every recipient go at once, and their
    // replies are read after; the message itself waits for them.
    let recipients = envelope.to();
    let replies: Vec<Reply> = if limits.pipelining {
        session.send(&mail)?;
        for to in recipients {
            session.send(&rcpt(to))?;
        }
        session.flush()?;
        (0..=recipients.len())
            .map(|_| session.reply())
            .collect::<Result<_>>()?
    } else {
        let mut replies = vec![session.command(&mail)?];
        if replies[0].positive() {
            for to in recipients {
                replies.push(session.command(&rcpt(to))?);
            }
        }
        replies
    };
    let sender = &replies[0];
    if !sender.positive() {
        let _ = session.command("QUIT");
        return Err(Refused(format!(
            "The server refused to send from {from}: {} {}",
            sender.code,
            sender.text()
        )))
        .context(failed);
    }
    if let Some((to, reply)) = recipients
        .iter()
        .zip(&replies[1..])
        .find(|(_, r)| !r.positive())
    {
        let _ = session.command("QUIT");
        return Err(Refused(format!("{} {}", reply.code, reply.text())))
            .with_context(|| format!("The server refused the recipient {to}."));
    }
    let accepted = if limits.chunking {
        session.bdat(&bytes, limits.pipelining)
    } else {
        session.data(&bytes)
    };
    accepted.context(failed)?;
    // Accepted; a failed goodbye doesn't matter.
    let _ = session.command("QUIT");
    Ok(Delivered {
        message,
        receipts_unsupported: a.delivery_receipts && !limits.dsn,
    })
}
/// Sends a message made elsewhere (a read receipt) from `from` to `to`,
/// as it is. No delivery receipts are asked for.
pub(crate) fn deliver_raw(
    a: &Account,
    from: &str,
    to: &[String],
    bytes: &[u8],
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<()> {
    ensure!(!to.is_empty(), "Add at least one recipient.");
    for address in std::iter::once(from).chain(to.iter().map(String::as_str)) {
        ensure!(
            !address.contains(['<', '>', '\r', '\n', ' ']) && address.contains('@'),
            "“{address}” isn't a valid address."
        );
    }
    let (mut session, limits) = open(endpoint, secret, tls)?;
    if a.require_tls {
        ensure!(
            limits.require_tls,
            "{} can't promise this message stays encrypted all the way (no REQUIRETLS), so it wasn't sent.",
            endpoint.host
        );
    }
    let mut mail = format!("MAIL FROM:<{from}>");
    if !from.is_ascii() || to.iter().any(|t| !t.is_ascii()) {
        ensure!(limits.utf8, "{} can't send to this address (no SMTPUTF8).", endpoint.host);
        mail.push_str(" SMTPUTF8");
    }
    if a.require_tls {
        mail.push_str(" REQUIRETLS");
    }
    let reply = session.command(&mail)?;
    ensure!(reply.positive(), "The server refused to send from {from}: {}", reply.text());
    for address in to {
        let reply = session.command(&format!("RCPT TO:<{address}>"))?;
        ensure!(reply.positive(), "The server refused the recipient {address}: {}", reply.text());
    }
    session.data(bytes)?;
    let _ = session.command("QUIT");
    Ok(())
}
/// A read receipt (MDN, RFC 8098) saying `m` was shown to `a`'s owner:
/// a multipart/report with a human-readable part and the machine-readable
/// disposition.
pub(crate) fn receipt(a: &Account, m: &crate::manage::model::Mail, to: &str) -> Vec<u8> {
    use base64::Engine as _;
    let encoded = |text: &str| {
        if text.is_ascii() && !text.contains(['\r', '\n']) {
            text.to_owned()
        } else {
            format!(
                "=?UTF-8?B?{}?=",
                base64::engine::general_purpose::STANDARD.encode(text.replace(['\r', '\n'], " "))
            )
        }
    };
    let boundary = format!("mdn-{}", uuid::Uuid::new_v4().simple());
    let domain = a.email.rsplit('@').next().unwrap_or("localhost");
    let now = chrono::Local::now();
    let name = encoded(&a.name);
    let subject = encoded(&format!("Read: {}", m.subject));
    let original = m.message_id.trim();
    let mut out = String::new();
    out.push_str(&format!("From: {name} <{}>\r\n", a.email));
    out.push_str(&format!("To: <{to}>\r\n"));
    out.push_str(&format!("Subject: {subject}\r\n"));
    out.push_str(&format!("Date: {}\r\n", now.to_rfc2822()));
    out.push_str(&format!("Message-ID: <{}@{domain}>\r\n", uuid::Uuid::new_v4()));
    if !original.is_empty() {
        out.push_str(&format!("In-Reply-To: {original}\r\nReferences: {original}\r\n"));
    }
    out.push_str("Auto-Submitted: auto-replied\r\nMIME-Version: 1.0\r\n");
    out.push_str(&format!(
        "Content-Type: multipart/report; report-type=disposition-notification; boundary=\"{boundary}\"\r\n\r\n"
    ));
    out.push_str(&format!(
        "--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
    ));
    out.push_str(&format!(
        "Your message to {} was displayed on {}.\r\nSubject: {}\r\n\r\nThis only says the message was shown; it doesn't guarantee that it was read or understood.\r\n",
        a.email,
        now.format("%a, %b %-d %Y at %-H:%M"),
        m.subject.replace(['\r', '\n'], " ")
    ));
    out.push_str(&format!(
        "--{boundary}\r\nContent-Type: message/disposition-notification\r\n\r\n"
    ));
    out.push_str(&format!(
        "Reporting-UA: Picacho; Picacho {}\r\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str(&format!("Final-Recipient: rfc822;{}\r\n", a.email));
    if !original.is_empty() {
        out.push_str(&format!("Original-Message-ID: {original}\r\n"));
    }
    out.push_str("Disposition: manual-action/MDN-sent-manually; displayed\r\n");
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}
/// A value as RFC 3461's xtext: printable ASCII but + and =, else +XX.
fn xtext(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'!'..=b'~' if b != b'+' && b != b'=' => (b as char).to_string(),
            b => format!("+{b:02X}"),
        })
        .collect()
}

/// Addresses in a comma-separated recipient field, with display names.
pub(crate) fn recipients(list: &str) -> Result<Vec<(Option<String>, String)>> {
    if list.trim().is_empty() {
        return Ok(vec![]);
    }
    let parsed = mailparse::addrparse(list)
        .context("Enter valid recipient email addresses, separated by commas")?;
    let mut out = vec![];
    for address in parsed.iter() {
        match address {
            mailparse::MailAddr::Single(s) => out.push((s.display_name.clone(), s.addr.clone())),
            mailparse::MailAddr::Group(g) => out.extend(
                g.addrs
                    .iter()
                    .map(|s| (s.display_name.clone(), s.addr.clone())),
            ),
        }
    }
    Ok(out)
}

/// The HTML version of a formatted (Markdown) body. Raw HTML in the text is
/// escaped, not passed through.
pub(crate) fn html_body(markdown: &str) -> String {
    let options = markdown::Options {
        parse: markdown::ParseOptions::gfm(),
        compile: markdown::CompileOptions {
            allow_dangerous_html: false,
            allow_dangerous_protocol: false,
            gfm_tagfilter: true,
            ..markdown::CompileOptions::gfm()
        },
    };
    // Single line breaks are kept, as people expect from an email editor.
    let content = markdown::to_html_with_options(&hard_breaks(markdown), &options)
        .unwrap_or_else(|_| markdown::to_html(markdown));
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head>\
<body><div style=\"font-family:-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;\
font-size:14px;line-height:1.5\">{content}</div></body></html>"
    )
}

/// Ends each line inside a paragraph with Markdown's hard break, so a single
/// newline stays a new line. Lists, quotes, headings and code are left alone.
fn hard_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() + lines.len() * 2);
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        }
        out.push_str(line);
        let next = lines.get(i + 1).map(|l| l.trim_start()).unwrap_or("");
        let block = |l: &str| {
            l.is_empty()
                || l.starts_with('>')
                || l.starts_with('#')
                || l.starts_with("- ")
                || l.starts_with("* ")
                || l.starts_with("```")
                || l.split_once(". ")
                    .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
        };
        if !fenced && i + 1 < lines.len() && !block(trimmed) && !block(next) {
            out.push('\\');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatted_bodies_become_safe_html_with_line_breaks() {
        let html = html_body("**Hi** _there_\nsecond line\n\n- one\n- two\n\n<script>x</script>");
        assert!(html.contains("<strong>Hi</strong>"));
        assert!(html.contains("<em>there</em><br />"));
        assert!(html.contains("<li>one</li>"));
        assert!(!html.contains("<script>"));
    }

    /// Mail sent as an alias asks for read receipts at the alias and
    /// doesn't take the account's Reply-To: neither shows the mailbox's own
    /// address, which the alias exists to keep private.
    #[test]
    fn alias_mail_never_names_the_mailbox() -> Result<()> {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        a.name = "Me".into();
        a.read_receipts = true;
        a.reply_to = "me@example.com".into();
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Hi".into(),
            body: "Hello".into(),
            send_as: "shop@alias.example".into(),
            ..Draft::default()
        };
        let headers = |m: Message| {
            let text = String::from_utf8_lossy(&m.formatted()).into_owned();
            text.split("\r\n\r\n").next().unwrap_or_default().to_owned()
        };
        let sent = headers(build(&a, &d, false)?);
        assert!(
            sent.contains("Disposition-Notification-To: <shop@alias.example>"),
            "{sent}"
        );
        assert!(!sent.contains("Reply-To"), "{sent}");
        // As the account itself, both are its own.
        let own = Draft {
            send_as: String::new(),
            ..d
        };
        let sent = headers(build(&a, &own, false)?);
        assert!(sent.contains("Disposition-Notification-To: <me@example.com>"));
        assert!(sent.contains("Reply-To: me@example.com"), "{sent}");
        Ok(())
    }

    /// Sent as one of the account's other addresses: it is in From, its
    /// own Reply-To is used (not the account's), and read receipts come
    /// back to it.
    #[test]
    fn other_addresses_send_as_themselves() -> Result<()> {
        use crate::manage::model::Identity;
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        a.name = "Me".into();
        a.read_receipts = true;
        a.reply_to = "me@example.com".into();
        a.identities = vec![Identity {
            id: "desk".into(),
            email: "desk@shop.example".into(),
            name: "Shop Desk".into(),
            reply_to: "help@shop.example".into(),
            smtp: None,
        }];
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Hi".into(),
            body: "Hello".into(),
            identity: "desk".into(),
            ..Draft::default()
        };
        let m = build(&a, &d, false)?;
        assert_eq!(
            m.envelope().from().map(|f| f.to_string()).as_deref(),
            Some("desk@shop.example")
        );
        let text = String::from_utf8_lossy(&m.formatted()).into_owned();
        let headers = text.split("\r\n\r\n").next().unwrap_or_default();
        assert!(
            headers.contains("From: \"Shop Desk\" <desk@shop.example>")
                || headers.contains("From: Shop Desk <desk@shop.example>"),
            "{headers}"
        );
        assert!(headers.contains("Reply-To: help@shop.example"), "{headers}");
        assert!(headers.contains("Disposition-Notification-To: <desk@shop.example>"));
        assert!(!headers.contains("me@example.com"), "{headers}");
        // An address since removed: not sent as the account by mistake.
        a.identities.clear();
        assert!(build(&a, &d, false).is_err());
        Ok(())
    }

    #[test]
    fn ehlo_limits_and_attachment_types() {
        let limits = server_limits(
            [
                "PIPELINING",
                "SIZE 35882577",
                "DSN",
                "8BITMIME",
                "AUTH PLAIN",
            ]
            .into_iter(),
        );
        assert_eq!(limits.size, Some(35882577));
        assert!(limits.dsn && limits.eight_bit && !limits.utf8);
        assert_eq!(server_limits(["SIZE 0"].into_iter()).size, None);
        assert!(limits.pipelining && !limits.chunking && !limits.require_tls);
        assert_eq!(limits.auth, ["PLAIN"]);
        let modern = server_limits(
            [
                "CHUNKING",
                "REQUIRETLS",
                "AUTH=LOGIN",
                "AUTH SCRAM-SHA-256 plain",
                "STARTTLS",
            ]
            .into_iter(),
        );
        assert!(modern.chunking && modern.require_tls && modern.starttls);
        assert_eq!(modern.auth, ["LOGIN", "SCRAM-SHA-256", "PLAIN"]);
        let ty = |name: &str| format!("{:?}", content_type(std::path::Path::new(name)));
        assert!(
            ty("photo.JPG").contains("image/jpeg"),
            "{}",
            ty("photo.JPG")
        );
        assert!(ty("report.pdf").contains("application/pdf"));
        assert!(ty("notes").contains("application/octet-stream"));
        assert_eq!(xtext("a+b=c d"), "a+2Bb+3Dc+20d");
    }

    #[test]
    fn recipient_fields_parse_names_and_allow_empty() {
        assert!(recipients("  ").unwrap().is_empty());
        let r = recipients("Ana <ana@example.com>, bo@example.com").unwrap();
        assert_eq!(r[0], (Some("Ana".into()), "ana@example.com".into()));
        assert_eq!(r[1].1, "bo@example.com");
    }
}
