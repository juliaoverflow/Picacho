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
/// Whom a draft is from: one of the mailbox's other addresses (unless
/// sent as an alias), else the account, with the name to show.
fn sender<'a>(
    a: &'a Account,
    d: &Draft,
) -> Result<(Option<&'a crate::manage::model::Identity>, Mailbox)> {
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
    Ok((identity, from))
}
/// The message a draft makes, as sent (`draft: false`) or as saved in the
/// server's Drafts folder: then incomplete drafts are allowed, Bcc is kept,
/// and Picacho's own headers let it be found again.
pub(crate) fn build(a: &Account, d: &Draft, draft: bool) -> Result<Message> {
    build_to(a, d, draft, None, None)
}
/// As `build`, sent only to `only_to` when given: the To and Cc headers
/// are the draft's, but the envelope (and so whom it is encrypted to)
/// names that one address. RFC 5322 §3.6.3: a Bcc recipient's own copy
/// of a message whose other copy has no Bcc field. `message_id`, when
/// given, is the one all copies of the message share.
pub(crate) fn build_to(
    a: &Account,
    d: &Draft,
    draft: bool,
    only_to: Option<&str>,
    message_id: Option<&str>,
) -> Result<Message> {
    let (identity, from) = sender(a, d)?;
    let mut builder = Message::builder().from(from.clone()).subject(&d.subject);
    if !draft {
        // RFC 5322 §3.6.4: a Message-ID unique to this message. Without
        // one, lettre would name the computer's host; this uses the
        // sender's domain (the alias's, sent as an alias) and a random
        // part, so it says nothing about the computer.
        let id = match message_id {
            Some(id) => id.to_owned(),
            None => new_message_id(&from_address(a, d)?),
        };
        builder = builder.message_id(Some(id));
    }
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
        ensure!(count > 0 || only_to.is_some(), "Add at least one recipient.");
        if let Some(to) = only_to {
            builder = builder.envelope(lettre::address::Envelope::new(
                Some(from.email.clone()),
                vec![to.parse().with_context(|| format!("“{to}” isn't a valid address."))?],
            )?);
        }
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
    // Urgency for the recipients' apps: Importance (RFC 2156) and the
    // X-Priority most apps read. Normal says nothing.
    let (importance, x_priority) = match d.priority {
        crate::manage::model::Priority::High => (Some("high"), Some("1 (Highest)")),
        crate::manage::model::Priority::Low => (Some("low"), Some("5 (Lowest)")),
        crate::manage::model::Priority::Normal => (None, None),
    };
    if let (Some(i), Some(x)) = (importance, x_priority) {
        builder = builder
            .raw_header(raw_header("Importance", i.into()))
            .raw_header(raw_header("X-Priority", x.into()));
    }
    if !d.reply_id.is_empty() {
        // Both come from the message replied to, written by its sender:
        // kept to one line so they can't add headers of their own.
        let one_line = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).collect() };
        builder = builder.in_reply_to(one_line(&d.reply_id));
        let references = if d.references.trim().is_empty() {
            one_line(&d.reply_id)
        } else {
            one_line(&d.references)
        };
        builder = builder.references(references);
    }
    // Plain text as format=flowed (RFC 3676): long paragraphs reflow on
    // narrow screens instead of breaking mid-sentence.
    // A formatted body's `file:` pictures read as their alt text there:
    // no local file name goes out (`html_body_with`). A draft's copy in
    // Drafts keeps its source, which is how it's read back (FORMAT_HEADER).
    let text_body = if !draft && d.formatted && !file_images(&d.body).is_empty() {
        rewrite_file_images(&d.body, false, |_| None)
    } else {
        d.body.clone()
    };
    let plain = || {
        SinglePart::builder()
            .header(
                header::ContentType::parse("text/plain; charset=utf-8; format=flowed; delsp=no")
                    .expect("a valid content type"),
            )
            .body(flowed(&text_body))
    };
    // Pictures placed in a formatted body (`![…](file:<attachment name>)`)
    // go beside its HTML, named by Content-ID (RFC 2387, RFC 2392).
    let inline = if d.formatted {
        inline_images(&d.body, &d.attachments, &from_address(a, d)?)
    } else {
        InlineImages::default()
    };
    let html = || SinglePart::html(html_body_with(&d.body, &inline));
    // Without attachments, the text alone: a multipart/mixed wrapper made
    // every message look like it had some.
    if d.attachments.is_empty() {
        return Ok(if d.formatted {
            builder.multipart(MultiPart::alternative().singlepart(plain()).singlepart(html()))?
        } else {
            builder.singlepart(plain())?
        });
    }
    let mut total = d.body.len();
    let text = if d.formatted && !inline.images.is_empty() {
        // RFC 2387 §3.1: the HTML first, its pictures after; the required
        // `type` parameter names the HTML's type.
        let kind = header::ContentType::parse(&format!(
            "multipart/related; type=\"text/html\"; boundary=\"{}\"",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("a valid content type");
        let mut related = MultiPart::builder().header(kind).singlepart(html());
        for image in &inline.images {
            let meta = std::fs::metadata(&image.path)
                .with_context(|| format!("The picture {} can't be read.", image.path.display()))?;
            total += meta.len() as usize;
            ensure!(
                total < 4 * DEFAULT_LIMIT,
                "Attachments are too large to send by email. Share them by link instead."
            );
            let bytes = std::fs::read(&image.path)?;
            let name = image.name.clone();
            related = related.singlepart(
                Attachment::new_inline_with_name(image.content_id.clone(), name)
                    .body(bytes, content_type(&image.path)),
            );
        }
        MultiPart::alternative().singlepart(plain()).multipart(related)
    } else if d.formatted {
        MultiPart::alternative().singlepart(plain()).singlepart(html())
    } else {
        MultiPart::alternative().singlepart(plain())
    };
    let mut multipart = MultiPart::mixed().multipart(text);
    for path in d.attachments.iter().filter(|p| !inline.images.iter().any(|i| &i.path == *p)) {
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
/// Text as format=flowed (RFC 3676 §4.2, without DelSp): lines longer
/// than 78 characters wrapped at spaces, each wrapped line ending in a
/// space (a soft break); lines that ended in spaces lose them (they'd read
/// as soft); `>` quotes and lines starting with a space or "From " are
/// space-stuffed. The signature separator "-- " stays as it is.
pub(crate) fn flowed(text: &str) -> String {
    const WIDTH: usize = 78;
    let mut out = String::with_capacity(text.len() + text.len() / 40);
    for line in text.replace("\r\n", "\n").split('\n') {
        if line == "-- " {
            out.push_str("-- \r\n");
            continue;
        }
        let quotes = line.chars().take_while(|c| *c == '>').count();
        let (prefix, rest) = line.split_at(quotes);
        let rest = rest.trim_end_matches(' ');
        // Stuffed: a line starting with a space or "From " (§4.4); a
        // quote's text follows its `>`s after one space.
        let stuff = rest.starts_with(' ') || (quotes == 0 && rest.starts_with("From "));
        let lead = if (quotes > 0 && !rest.is_empty()) || stuff {
            format!("{prefix} ")
        } else {
            prefix.to_owned()
        };
        let rest = if quotes > 0 { rest.strip_prefix(' ').unwrap_or(rest) } else { rest };
        let mut current = String::new();
        let words: Vec<&str> = rest.split(' ').collect();
        let room = WIDTH.saturating_sub(lead.chars().count()).max(20);
        for (i, word) in words.iter().enumerate() {
            let last = i + 1 == words.len();
            let piece = if last { (*word).to_owned() } else { format!("{word} ") };
            if !current.is_empty() && current.chars().count() + word.chars().count() > room {
                // A soft break: the line ends in its space.
                out.push_str(&lead);
                out.push_str(&current);
                out.push_str("\r\n");
                current.clear();
            }
            current.push_str(&piece);
        }
        out.push_str(&lead);
        out.push_str(current.trim_end_matches(' '));
        out.push_str("\r\n");
    }
    // The text's own last line break isn't doubled.
    if !text.ends_with('\n') {
        out.truncate(out.len() - 2);
    }
    out
}

/// One attached file. A forwarded message (message/rfc822) goes as it is
/// (7bit or 8bit, as RFC 2046 asks of messages), so recipients' apps open it
/// as a message; when its lines are too long for that, it is encoded like any
/// other file. One whose headers are in UTF-8 (an internationalized message)
/// goes as message/global (RFC 6532 §3.7), which may be encoded like any
/// other part where message/rfc822 may not.
fn attachment_part(name: String, bytes: Vec<u8>, kind: header::ContentType) -> SinglePart {
    use lettre::message::{Body, header::ContentTransferEncoding};
    if kind == header::ContentType::parse("message/rfc822").unwrap() {
        let head_end = memchr::memmem::find(&bytes, b"\r\n\r\n")
            .or_else(|| memchr::memmem::find(&bytes, b"\n\n"))
            .unwrap_or(bytes.len());
        if !bytes[..head_end].is_ascii() && std::str::from_utf8(&bytes[..head_end]).is_ok() {
            let global = header::ContentType::parse("message/global").unwrap();
            return Attachment::new(name).body(bytes, global);
        }
        let encoding = if bytes.is_ascii() {
            ContentTransferEncoding::SevenBit
        } else {
            ContentTransferEncoding::EightBit
        };
        // UTF-8 text is given as text: as bytes, lettre takes any non-ASCII
        // as binary and refuses 8bit for it.
        let text: lettre::message::MaybeString = match String::from_utf8(bytes) {
            Ok(text) => text.into(),
            Err(e) => e.into_bytes().into(),
        };
        // Refused, it hands the bytes back: no copy is kept for trying.
        return match Body::new_with_encoding(text, encoding) {
            Ok(body) => SinglePart::builder()
                .header(kind)
                .header(header::ContentDisposition::attachment(&name))
                .body(body),
            Err(bytes) => Attachment::new(name).body(bytes, kind),
        };
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
    /// SIZE was listed at all (with a limit or none): MAIL FROM may then
    /// declare the message's size.
    pub(crate) size_offered: bool,
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
    /// ENHANCEDSTATUSCODES (RFC 2034): replies start with a class.subject.detail
    /// code such as 5.1.1.
    pub(crate) enhanced: bool,
    /// LIMITS RCPTMAX (RFC 9422): how many recipients one message may have.
    pub(crate) rcpt_max: Option<usize>,
    /// LIMITS MAILMAX: how many messages one connection may send.
    pub(crate) mail_max: Option<usize>,
    /// LIMITS RCPTDOMAINMAX: how many recipient domains one message may
    /// have.
    pub(crate) rcpt_domain_max: Option<usize>,
    /// BURL (RFC 4468): the message may be named by an IMAP URL rather than
    /// sent again; `imap` when the server fetches from IMAP servers.
    pub(crate) burl_imap: bool,
    /// BINARYMIME (RFC 3030): 8-bit lines of any length and NULs, with BDAT.
    pub(crate) binary_mime: bool,
    /// FUTURERELEASE (RFC 4865): how far ahead it holds a message for
    /// release, in seconds (0 where it doesn't).
    pub(crate) future_release: u64,
    /// FUTURERELEASE's max-future-release-date-time (RFC 4865 §3): the
    /// latest moment it holds a message until, a Unix time.
    pub(crate) future_release_until: Option<i64>,
    /// MT-PRIORITY (RFC 6710): it takes a message's priority.
    pub(crate) mt_priority: bool,
    /// The priority profile it named (MIXER, STANAG4406, NSEP), if any.
    pub(crate) mt_profile: String,
    /// DELIVERBY (RFC 2852): the shortest time it accepts for BY=, in
    /// seconds (0: any); `None` where it doesn't.
    pub(crate) deliver_by: Option<u64>,
}

/// The outgoing server can't hold a message for sending later (no
/// FUTURERELEASE, or not that far ahead): nothing was sent.
#[derive(Debug)]
pub(crate) struct NoServerHold;
impl std::fmt::Display for NoServerHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The outgoing server can't hold this message until it's due.")
    }
}
impl std::error::Error for NoServerHold {}

/// Encrypted mail with Bcc through a service that takes one message for
/// all its recipients (Proton, JMAP): its encryption would name every Bcc
/// recipient's key to everyone, and the service can't be sent a separate
/// copy for each without delivering to To and Cc again or filing each copy
/// in Sent. Refused before anything is sent; the recipients can be written
/// to separately.
#[derive(Debug)]
pub(crate) struct EncryptedBccUnsupported {
    /// The service, as named to the person sending.
    pub(crate) service: String,
}
impl std::fmt::Display for EncryptedBccUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Encrypted mail with Bcc can't be sent through {} without showing who got a Bcc. Send to them separately, or turn off Encrypt.",
            self.service
        )
    }
}
impl std::error::Error for EncryptedBccUnsupported {}
pub(crate) fn server_limits<'a>(lines: impl Iterator<Item = &'a str>) -> ServerLimits {
    let mut limits = ServerLimits::default();
    for line in lines {
        let mut words = line.split_whitespace();
        match words.next().map(str::to_ascii_uppercase).as_deref() {
            // "SIZE 0" (or no number) means no fixed limit.
            Some("SIZE") => {
                limits.size_offered = true;
                limits.size = words.next().and_then(|n| n.parse().ok()).filter(|n| *n > 0)
            }
            Some("DSN") => limits.dsn = true,
            Some("8BITMIME") => limits.eight_bit = true,
            Some("SMTPUTF8") => limits.utf8 = true,
            Some("PIPELINING") => limits.pipelining = true,
            Some("CHUNKING") => limits.chunking = true,
            Some("REQUIRETLS") => limits.require_tls = true,
            Some("STARTTLS") => limits.starttls = true,
            // RFC 4865 §3: "FUTURERELEASE <max interval> <max date-time>".
            Some("FUTURERELEASE") => {
                limits.future_release = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                limits.future_release_until = words
                    .next()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp());
            }
            Some("ENHANCEDSTATUSCODES") => limits.enhanced = true,
            // "MT-PRIORITY MIXER" (a profile, or none).
            Some("MT-PRIORITY") => {
                limits.mt_priority = true;
                limits.mt_profile = words.next().map(str::to_ascii_uppercase).unwrap_or_default();
            }
            // "DELIVERBY 240" (or no minimum).
            Some("DELIVERBY") => {
                limits.deliver_by = Some(words.next().and_then(|n| n.parse().ok()).unwrap_or(0))
            }
            Some("BINARYMIME") => limits.binary_mime = true,
            // "BURL imap" (or a bare BURL, which fetches from URLs it is given).
            Some("BURL") => {
                // Each word is a scheme (`imap`) or a trusted authority
                // URL (`imap://imap.example.com`, RFC 4468 §3).
                let schemes: Vec<String> = words.map(str::to_ascii_lowercase).collect();
                limits.burl_imap = schemes.is_empty()
                    || schemes.iter().any(|s| s == "imap" || s.starts_with("imap://"));
            }
            // "LIMITS RCPTMAX=100 MAILMAX=1000 RCPTDOMAINMAX=10"
            Some("LIMITS") => {
                for word in words {
                    let Some((name, value)) = word.split_once('=') else {
                        continue;
                    };
                    let value = value.parse::<usize>().ok().filter(|n| *n > 0);
                    match name.to_ascii_uppercase().as_str() {
                        "RCPTMAX" => limits.rcpt_max = value,
                        "MAILMAX" => limits.mail_max = value,
                        "RCPTDOMAINMAX" => limits.rcpt_domain_max = value,
                        _ => {}
                    }
                }
            }
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
/// A delivery time limit (DELIVERBY, RFC 2852) as the account asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliverBy {
    /// How long, in seconds.
    pub(crate) seconds: u64,
    /// Late, it is delivered and the sender told (N) rather than returned
    /// (R).
    pub(crate) notify: bool,
    /// Each server on the way reports relaying it (T).
    pub(crate) trace: bool,
}
impl DeliverBy {
    /// The account's limit, if it sets one; N and T mean nothing alone.
    pub(crate) fn of(a: &Account) -> Option<Self> {
        (a.deliver_within_hours > 0).then(|| Self {
            seconds: u64::from(a.deliver_within_hours) * 3600,
            notify: a.deliver_late_notify,
            trace: a.deliver_by_trace,
        })
    }
    /// The BY= value (RFC 2852 §4: `by-time;by-mode[trace]`) for a server
    /// whose least by-time is `minimum` seconds. That minimum is for R mode
    /// only (§4: a server refuses an R by-time below it); an N by-time is
    /// as asked. At most nine digits.
    pub(crate) fn parameter(self, minimum: u64) -> String {
        let seconds = if self.notify {
            self.seconds
        } else {
            self.seconds.max(minimum)
        }
        .min(999_999_999);
        format!(
            "{seconds};{}{}",
            if self.notify { "N" } else { "R" },
            if self.trace { "T" } else { "" }
        )
    }
}

/// HOLDFOR's seconds (RFC 4865 §4) for a message due at `at` when it is
/// `now`, where the server can hold it that long: within its
/// max-future-release-interval and, if it named one, not after its
/// max-future-release-date-time. HOLDFOR rather than HOLDUNTIL, since a
/// time from now doesn't depend on this computer's clock agreeing with
/// the server's. `None` where it can't (no FUTURERELEASE gives 0).
fn hold_seconds(limits: &ServerLimits, at: i64, now: i64) -> Option<u64> {
    let ahead = (at - now).max(1) as u64;
    let within = ahead <= limits.future_release;
    let before_end = limits.future_release_until.is_none_or(|end| at <= end);
    (within && before_end).then_some(ahead)
}
/// What happened to a message the server accepted.
pub(crate) struct Delivered {
    /// The message as sent: signed or encrypted with OpenPGP if asked.
    pub(crate) raw: Vec<u8>,
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
    /// TLS from the first byte, rather than STARTTLS.
    pub(crate) implicit_tls: bool,
    /// Signs in with Kerberos (SASL GSSAPI) rather than a password.
    pub(crate) kerberos: bool,
}
impl Endpoint {
    /// The account's own outgoing server.
    pub(crate) fn of(a: &Account) -> Self {
        Self {
            host: a.smtp_host.clone(),
            port: a.smtp_port,
            user: if a.oauth && a.sso.is_none() {
                a.email.clone()
            } else {
                a.login().to_owned()
            },
            oauth: a.oauth,
            implicit_tls: crate::manage::model::smtp_implicit_tls(a.smtp_tls, a.smtp_port),
            kerberos: a.kerberos,
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
            implicit_tls: crate::manage::model::smtp_implicit_tls(s.tls, s.port),
            // A sending address's own server signs in with its password.
            kerberos: false,
        })
    }
}

/// How Picacho greets an outgoing server: an address literal, as
/// Thunderbird says hello. The computer's own name would be written into
/// every message's Received header.
const HELLO: &str = "[127.0.0.1]";

/// One server reply: its code and text lines.
#[derive(Debug)]
struct Reply {
    code: u16,
    lines: Vec<String>,
    /// The server offered ENHANCEDSTATUSCODES: its replies start with one
    /// (RFC 2034 §3). Elsewhere text like "5.1.1" is only text.
    enhanced: bool,
}
impl Reply {
    /// A completion (2xx). Intermediate replies (3xx: 334, 354) are
    /// expected by their own code where they belong, never as success.
    fn positive(&self) -> bool {
        (200..300).contains(&self.code)
    }
    /// The reply's text, its lines joined. RFC 2034 §4: each line of a
    /// multi-line reply starts with the same enhanced code; it is kept on
    /// the first line only.
    fn text(&self) -> String {
        let code = self.enhanced().map(|_| {
            self.lines[0].split_whitespace().next().unwrap_or("").to_owned()
        });
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            let line = match &code {
                Some(code) if i > 0 => line
                    .strip_prefix(code.as_str())
                    .map_or(line.as_str(), str::trim_start),
                _ => line.as_str(),
            };
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(line);
        }
        out
    }
    /// The enhanced status code (RFC 3463) the reply starts with, if any.
    /// Only one whose class agrees with the reply code's (RFC 2034 §4).
    fn enhanced(&self) -> Option<(u8, u16, u16)> {
        if !self.enhanced {
            return None;
        }
        enhanced_code(self.lines.first()?).filter(|(class, ..)| u16::from(*class) == self.code / 100)
    }
    /// "Too many recipients" (RFC 5321 §4.5.3.1.10): 452, or 4.5.3. The
    /// recipients refused for it go in another transaction.
    fn too_many_recipients(&self) -> bool {
        self.code == 452 && self.enhanced().is_none_or(|e| e == (4, 5, 3))
            || self.enhanced() == Some((4, 5, 3))
    }
    /// The server's text, with what its enhanced code means in front when
    /// the text alone is unclear.
    fn explained(&self) -> String {
        match self.enhanced().and_then(|(c, s, d)| meaning(c, s, d)) {
            Some(meaning) => format!("{meaning} ({} {})", self.code, self.text()),
            None => format!("{} {}", self.code, self.text()),
        }
    }
}

/// An enhanced status code at the start of a reply line: `5.1.1 …`.
pub(crate) fn enhanced_code(line: &str) -> Option<(u8, u16, u16)> {
    let code = line.split_whitespace().next()?;
    let mut parts = code.split('.');
    fn digits(p: &str, most: usize) -> Option<&str> {
        (!p.is_empty() && p.len() <= most && p.bytes().all(|b| b.is_ascii_digit())).then_some(p)
    }
    // RFC 3463 §2: subject and detail are 1–3 digits each.
    let class: u8 = digits(parts.next()?, 1)?.parse().ok()?;
    let subject: u16 = digits(parts.next()?, 3)?.parse().ok()?;
    let detail: u16 = digits(parts.next()?, 3)?.parse().ok()?;
    (parts.next().is_none() && matches!(class, 2 | 4 | 5)).then_some((class, subject, detail))
}

/// What an enhanced status code (RFC 3463, and the IANA registry) means,
/// for the codes people most often meet.
pub(crate) fn meaning(class: u8, subject: u16, detail: u16) -> Option<&'static str> {
    if class == 2 {
        return None;
    }
    Some(match (subject, detail) {
        (1, 1) => "That mailbox doesn't exist",
        (1, 2) => "That domain doesn't exist or takes no mail",
        (1, 3) => "That address isn't valid",
        (1, 6) => "That person has moved and no longer gets mail there",
        (1, 7) | (1, 8) => "The server won't send from this address",
        (1, 10) => "That domain takes no mail (null MX)",
        (2, 1) => "That mailbox isn't taking mail now",
        (2, 2) => "That mailbox is full",
        (2, 3) | (3, 4) => "The message is too large",
        (3, 1) => "The server's storage is full",
        (4, 1) | (4, 2) | (4, 7) => "The recipient's server couldn't be reached",
        (4, 4) => "The recipient's domain couldn't be routed to",
        (5, 3) => "Too many recipients for one message",
        (6, _) => "The server can't take this message's format",
        (7, 0) | (7, 1) => "The server refused this by its policy",
        (7, 8) | (7, 9) => "The sign-in wasn't accepted",
        (7, 13) => "This account is disabled",
        (7, 23) => "SPF says this server may not send for this address",
        (7, 25) | (7, 26) | (7, 27) => "The message failed the receiving server's sender checks",
        (7, 30) => "The server can't keep the message encrypted all the way",
        (_, _) if class == 4 => "The server can't take this now; it may later",
        _ => return None,
    })
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

/// The outgoing server refused to fetch a message by its IMAP URL (BURL,
/// RFC 4468 §3), as the first transaction: nobody was sent it, so it can
/// be sent again as itself.
#[derive(Debug)]
pub(crate) struct BurlRefused(pub(crate) Refused);
impl std::fmt::Display for BurlRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for BurlRefused {}

/// A stream whose reads may be told to wait longer than usual.
trait Patience {
    fn wait_for(&mut self, wait: Option<Duration>);
    /// The TLS channel binding (SCRAM-PLUS), where the stream is TLS.
    fn binding(&self) -> Vec<u8> {
        Vec::new()
    }
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
    fn binding(&self) -> Vec<u8> {
        sasl::binding(self)
    }
}

/// An SMTP session: commands out, replies in.
struct Session<S: Read + Write> {
    reader: BufReader<S>,
    /// The last EHLO listed ENHANCEDSTATUSCODES (RFC 2034).
    enhanced: bool,
}
impl<S: Read + Write + Patience> Session<S> {
    fn new(stream: S) -> Self {
        Self {
            reader: BufReader::new(stream),
            enhanced: false,
        }
    }
    fn reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        let mut first_code = None;
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
            // Every line of a reply carries the same code (RFC 5321 §4.2.1).
            if let Some(first) = first_code {
                ensure!(first == code, "The outgoing server sent an invalid reply.");
            }
            first_code = Some(code);
            ensure!(
                rest.is_empty() || rest.starts_with(['-', ' ']),
                "The outgoing server sent an invalid reply."
            );
            lines.push(rest.get(1..).unwrap_or("").to_owned());
            if !rest.starts_with('-') {
                return Ok(Reply {
                    code,
                    lines,
                    enhanced: self.enhanced,
                });
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
            return Err(Refused(format!("{what}: {}", reply.explained())).into());
        }
        Ok(reply)
    }
    fn ehlo(&mut self) -> Result<ServerLimits> {
        // An address literal, as Thunderbird says hello: the computer's own
        // name would be written into every message's Received header.
        let reply = self.command(&format!("EHLO {HELLO}"))?;
        if !reply.positive() {
            // A server from before ESMTP (RFC 5321 §4.1.4): HELO, and no
            // extensions. Without STARTTLS it is only used over TLS.
            if matches!(reply.code, 500 | 502) {
                // HELO takes a domain, not an address literal (RFC 5321
                // §4.1.1.1).
                self.expect("HELO localhost.localdomain", "The outgoing server refused HELO")?;
                self.enhanced = false;
                return Ok(ServerLimits::default());
            }
            return Err(Refused(format!(
                "The outgoing server refused EHLO: {}",
                reply.explained()
            ))
            .into());
        }
        let limits = server_limits(reply.lines.iter().skip(1).map(String::as_str));
        self.enhanced = limits.enhanced;
        Ok(limits)
    }
    /// Signs in with the best mechanism the server offers.
    fn authenticate(
        &mut self,
        endpoint: &Endpoint,
        limits: &ServerLimits,
        secret: &str,
    ) -> Result<()> {
        let order: &[sasl::Mechanism] = if endpoint.kerberos {
            &sasl::KERBEROS_ORDER
        } else if endpoint.oauth {
            &sasl::TOKEN_ORDER
        } else {
            &sasl::PASSWORD_ORDER
        };
        let bound = !self.reader.get_ref().binding().is_empty();
        let mechanism = sasl::Mechanism::choose_bound(order, limits.auth.iter().map(String::as_str), bound)
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
                binding: self.reader.get_ref().binding(),
                service: "smtp",
            },
        );
        // RFC 4954 §4: the first message may go with the command, where the
        // line stays within 512 octets; otherwise it answers the server's
        // empty challenge.
        let first = exchange.initial().map(|first| {
            if first.is_empty() {
                "=".to_owned()
            } else {
                sasl::encode(&first)
            }
        });
        let inline = format!("AUTH {} ", mechanism.name());
        let mut reply = match first {
            Some(first) if inline.len() + first.len() + 2 <= 512 => {
                self.command(&format!("{inline}{first}"))?
            }
            Some(first) => {
                let reply = self.command(&format!("AUTH {}", mechanism.name()))?;
                if reply.code == 334 {
                    self.command(if first == "=" { "" } else { &first })?
                } else {
                    reply
                }
            }
            None => self.command(&format!("AUTH {}", mechanism.name()))?,
        };
        loop {
            match reply.code {
                235 => {
                    // A server may send SCRAM's signature with its success.
                    // With ENHANCEDSTATUSCODES the text starts `2.7.0 `.
                    let extra = reply
                        .lines
                        .first()
                        .map(|t| match reply.enhanced() {
                            Some(_) => t.split_once(' ').map_or("", |(_, rest)| rest.trim()),
                            None => t.trim(),
                        })
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
                    // RFC 4954 §6 and RFC 3463: 535 5.7.8 and the like say
                    // why; the mechanism's own reason (SCRAM's e=) follows.
                    let why = match exchange.refusal() {
                        Some(reason) => format!("{} ({reason})", reply.explained()),
                        None => reply.explained(),
                    };
                    return Err(Refused(format!(
                        "{} refused the sign-in with {}: {why}",
                        endpoint.host,
                        mechanism.name(),
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
                "The server refused the message: {}",
                reply.explained()
            ))
            .into());
        }
        // Stuffed a piece at a time as it goes out: a whole stuffed copy was
        // a second message's worth of memory (with attachments, a hundred
        // megabytes and more).
        dot_stuff_into(bytes, self.reader.get_mut())?;
        self.flush()?;
        // RFC 5321 §4.1.1.4: only a 2xx after the final dot accepts it.
        let reply = self.final_reply()?;
        if !reply.positive() {
            return Err(Refused(format!(
                "The server refused the message: {}",
                reply.explained()
            ))
            .into());
        }
        Ok(reply)
    }
    /// The message as is, in BDAT chunks (CHUNKING); with PIPELINING all
    /// chunks go before their replies are read.
    fn bdat(&mut self, bytes: &[u8], pipelining: bool) -> Result<Reply> {
        const CHUNK: usize = 1024 * 1024;
        // Replies are read at least every few chunks, so neither side's
        // buffers fill while the other waits (RFC 2920 §3.1).
        const WINDOW: usize = 4;
        let mut chunks: Vec<&[u8]> = bytes.chunks(CHUNK).collect();
        if chunks.is_empty() {
            // An empty message is `BDAT 0 LAST`.
            chunks.push(&[]);
        }
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
            if !pipelining || i == last || pending >= WINDOW {
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
                            "The server refused the message: {}",
                            r.explained()
                        ))
                        .into());
                    }
                    reply = Some(r);
                }
            }
        }
        reply.context("No reply to the message")
    }
    /// The message named by an IMAP URL, which the server fetches (BURL,
    /// RFC 4468), as the whole of the message.
    fn burl(&mut self, url: &str) -> Result<Reply> {
        self.send(&format!("BURL {url} LAST"))?;
        self.flush()?;
        let reply = self.final_reply()?;
        if !reply.positive() {
            // RFC 4468 §3: a refused BURL ends the transaction with nothing
            // delivered, so the message can still be sent another way.
            return Err(BurlRefused(Refused(format!(
                "The server couldn't fetch the message to send: {}",
                reply.explained()
            )))
            .into());
        }
        Ok(reply)
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

/// Connects, secures and signs in to an outgoing server: TLS from the
/// first byte or STARTTLS (`Endpoint::implicit_tls`), never in the clear.
/// A server that offers no sign-in at all (a relay that knows its senders
/// by their network) is sent to without one.
fn open(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<(Session<native_tls::TlsStream<Wire>>, ServerLimits)> {
    let socket = crate::sync::transport::tcp(&endpoint.host, endpoint.port)?;
    let mut session = if endpoint.implicit_tls {
        let stream = tls
            .connect(&endpoint.host, socket)
            .map_err(|e| anyhow::anyhow!("TLS with {} failed: {e}", endpoint.host))?;
        let mut session = Session::new(stream);
        let greeting = session.reply()?;
        ensure!(
            greeting.code == 220,
            "{} refused the connection: {}",
            endpoint.host,
            greeting.explained()
        );
        session
    } else {
        let mut plain = Session::new(socket);
        let greeting = plain.reply()?;
        ensure!(
            greeting.code == 220,
            "{} refused the connection: {}",
            endpoint.host,
            greeting.explained()
        );
        let limits = plain.ehlo()?;
        // Never send credentials or mail unencrypted.
        ensure!(
            limits.starttls,
            "{} doesn't offer STARTTLS on port {}; Picacho only sends over encrypted connections. If this server uses TLS from the start, turn on \"TLS from the start\" for it.",
            endpoint.host,
            endpoint.port
        );
        let reply = plain.command("STARTTLS")?;
        ensure!(
            reply.code == 220,
            "{} refused STARTTLS: {}",
            endpoint.host,
            reply.explained()
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
    // A relay that takes mail by its senders' network offers no AUTH.
    // An access token, or Kerberos, is only for servers that sign in: one
    // offering no sign-in then is refused, not sent mail unauthenticated.
    if !limits.auth.is_empty() || endpoint.oauth || endpoint.kerberos {
        session.authenticate(endpoint, &limits, secret)?;
    }
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

/// Delivers `d`. With `hold_until` (a Unix time) the server holds it and
/// sends it then (FUTURERELEASE's HOLDFOR); a server that can't, or not
/// that far ahead, is left before anything is sent, with `NoServerHold`.
pub(crate) fn deliver(
    a: &Account,
    d: &Draft,
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
    hold_until: Option<i64>,
    keys: &Keys,
) -> Result<Delivered> {
    let mut delivered = if d.encrypt && !d.bcc.trim().is_empty() {
        deliver_bcc_apart(a, d, endpoint, secret, tls, hold_until, keys)?
    } else {
        let message = build(a, d, false)?;
        let bytes = protected(&message, d, keys)?;
        deliver_bytes(a, d, &message, bytes, endpoint, secret, tls, hold_until, None)?
    };
    // RFC 5322 §3.6.3: what went out has no Bcc field; the sender's own
    // copy (in Sent) keeps it, so it shows whom the message also went to.
    delivered.raw = with_bcc(delivered.raw, d)?;
    Ok(delivered)
}

/// Encrypted with Bcc: the Bcc recipients' keys would be listed in the
/// copy everyone gets (each encrypted session key names its key), telling
/// To and Cc who else got it. Each Bcc recipient gets a copy of their own
/// instead, encrypted to them alone and the sender; its To and Cc are the
/// message's, with no Bcc field (RFC 5322 §3.6.3), and its envelope names
/// only them. The result is the To/Cc copy (or, with no To or Cc, the first
/// Bcc copy), for Sent.
fn deliver_bcc_apart(
    a: &Account,
    d: &Draft,
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
    hold_until: Option<i64>,
    keys: &Keys,
) -> Result<Delivered> {
    let visible = Draft {
        bcc: String::new(),
        ..d.clone()
    };
    let mut delivered = None;
    // One message, however many copies: the same Message-ID in each, so
    // replies thread together.
    let message_id = new_message_id(&from_address(a, d)?);
    if !recipients(&d.to)?.is_empty() || !recipients(&d.cc)?.is_empty() {
        let message = build_to(a, &visible, false, None, Some(&message_id))?;
        let bytes = protected(&message, &visible, keys)?;
        delivered = Some(deliver_bytes(a, &visible, &message, bytes, endpoint, secret, tls, hold_until, None)?);
    }
    for (_, address) in recipients(&d.bcc)? {
        let message = build_to(a, &visible, false, Some(&address), Some(&message_id))?;
        let bytes = protected(&message, &visible, keys)?;
        let alone = deliver_bytes(a, &visible, &message, bytes, endpoint, secret, tls, hold_until, None);
        let alone = match delivered {
            Some(_) => alone.with_context(|| {
                format!("The message went to its other recipients, but not to {address}.")
            })?,
            None => alone?,
        };
        delivered.get_or_insert(alone);
    }
    delivered.context("Add at least one recipient.")
}

/// A new Message-ID at `address`'s domain (RFC 5322 §3.6.4): random
/// (a version 4 UUID), so unique, and naming nothing about the computer.
/// An internationalized domain is written as ASCII (IDNA).
pub(crate) fn new_message_id(address: &str) -> String {
    let domain = address
        .rsplit_once('@')
        .and_then(|(_, domain)| idna::domain_to_ascii(domain).ok())
        .filter(|d| !d.is_empty() && !d.contains(['<', '>', ' ', '[', ']']))
        .unwrap_or_else(|| "localhost".into());
    format!("<{}@{domain}>", uuid::Uuid::new_v4().simple())
}
/// The address a draft is sent from, as Message-IDs name its domain: the
/// alias's, else the sending address's or the account's.
pub(crate) fn from_address(a: &Account, d: &Draft) -> Result<String> {
    if !d.send_as.trim().is_empty() {
        return Ok(d.send_as.trim().to_owned());
    }
    Ok(sender(a, d)?.1.email.to_string())
}
/// The Message-ID a built message has.
#[cfg(test)]
fn message_id_of(message: &Message) -> String {
    message
        .headers()
        .get_raw("Message-ID")
        .unwrap_or_default()
        .to_owned()
}

/// `raw` with the draft's Bcc field added in front, for the sender's own
/// copy; as it is without Bcc recipients. Outside any signature or
/// encryption, which cover the body part only (RFC 3156 §5, RFC 8551).
pub(crate) fn with_bcc(raw: Vec<u8>, d: &Draft) -> Result<Vec<u8>> {
    let mut list = lettre::message::Mailboxes::new();
    for (name, address) in recipients(&d.bcc)? {
        list.push(Mailbox::new(name, address.parse()?));
    }
    if list.iter().next().is_none() {
        return Ok(raw);
    }
    let mut headers = header::Headers::new();
    headers.set(header::Bcc::from(list));
    let mut out = headers.to_string().into_bytes();
    out.extend_from_slice(&raw);
    Ok(out)
}

/// What the server can be told to fetch instead of being sent the
/// message: an URLAUTH-authorized IMAP URL (RFC 4467) for BURL (RFC 4468).
pub(crate) struct Burl<'a> {
    pub(crate) url: &'a str,
}

/// Whether a server can take the message as a BURL reference.
pub(crate) fn takes_burl(
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
) -> Result<bool> {
    let (mut session, limits) = open(endpoint, secret, tls)?;
    let _ = session.command("QUIT");
    Ok(limits.burl_imap && limits.chunking)
}

/// Delivers `bytes`, the message built from `d` (`message`, for its
/// envelope). With `burl`, a server offering BURL fetches the message from
/// there rather than being sent it; others are sent `bytes`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn deliver_bytes(
    a: &Account,
    d: &Draft,
    message: &Message,
    bytes: Vec<u8>,
    endpoint: &Endpoint,
    secret: &str,
    tls: &native_tls::TlsConnector,
    hold_until: Option<i64>,
    burl: Option<Burl<'_>>,
) -> Result<Delivered> {
    let failed =
        "SMTP delivery failed. Your draft has been kept; check the server and account settings.";
    let (mut session, limits) = open(endpoint, secret, tls).context(failed)?;
    let limit = limits.size.unwrap_or(DEFAULT_LIMIT);
    // As the server will count it (RFC 1870 §4): with DATA, every line
    // ending a CRLF; with BDAT, the bytes as they are.
    let size = if limits.chunking {
        bytes.len()
    } else {
        data_size(&bytes)
    };
    if size > limit {
        bail!(
            "This message is {} with its attachments; {} accepts up to {}. Remove an attachment or share it by link.",
            megabytes(size),
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
    // Held for later: for how long, from the server's own clock.
    let hold_for = match hold_until {
        None => None,
        Some(at) => match hold_seconds(&limits, at, chrono::Utc::now().timestamp()) {
            Some(ahead) => Some(ahead),
            None => {
                let _ = session.command("QUIT");
                return Err(NoServerHold.into());
            }
        },
    };
    let envelope = message.envelope();
    let from = envelope.from().map(|f| f.to_string()).unwrap_or_default();
    // An internationalized domain with a plain local part goes as its
    // ASCII form (punycode), so a server without SMTPUTF8 takes it.
    let from = ascii_domain(&from);
    let mut mail = format!("MAIL FROM:<{from}>");
    if limits.size_offered {
        mail.push_str(&format!(" SIZE={size}"));
    }
    let non_ascii = envelope
        .from()
        .into_iter()
        .chain(envelope.to())
        .any(|address| !ascii_domain(AsRef::<str>::as_ref(address)).is_ascii());
    // A header in UTF-8 itself (not encoded words) needs SMTPUTF8 too
    // (RFC 6532 §3: such a message is an internationalized one).
    let head_end = memchr::memmem::find(&bytes, b"\r\n\r\n").unwrap_or(bytes.len());
    let utf8_headers = !bytes[..head_end].is_ascii();
    if utf8_headers && !non_ascii && limits.utf8 {
        mail.push_str(" SMTPUTF8");
    }
    if non_ascii {
        ensure!(
            limits.utf8,
            "{} can't send to or from addresses with accents or non-Latin letters (no SMTPUTF8).",
            endpoint.host
        );
        mail.push_str(" SMTPUTF8");
    }
    // BURL's chunk is a BDAT-style transfer (RFC 4468 §3 needs CHUNKING).
    let burl = burl.filter(|_| limits.burl_imap && limits.chunking);
    // Binary content (NULs, bare CR, very long lines) goes as BINARYMIME,
    // which only BDAT can carry; otherwise 8BITMIME for 8-bit text.
    let binary = needs_binary(&bytes) && limits.binary_mime && limits.chunking && burl.is_none();
    if binary {
        mail.push_str(" BODY=BINARYMIME");
    } else if !bytes.is_ascii() && limits.eight_bit {
        mail.push_str(" BODY=8BITMIME");
    }
    let receipts = a.delivery_receipts && limits.dsn;
    if receipts {
        // RFC 3461 §4.3, §4.4: what a failure returns (the headers unless
        // asked otherwise), identified by the draft.
        mail.push_str(&format!(" RET={} ENVID={}", a.dsn.ret(), xtext(&d.id)));
    }
    if a.require_tls {
        mail.push_str(" REQUIRETLS");
    }
    if let Some(seconds) = hold_for {
        mail.push_str(&format!(" HOLDFOR={seconds}"));
    }
    // Priority (RFC 6710): only where it isn't the default, since a server
    // may lower it for senders it doesn't trust; that's the server's to do.
    if limits.mt_priority && !d.priority.is_normal() {
        // In the profile's own range (RFC 6710 §5, Appendices): MIXER's
        // levels are -4 and 4; STANAG 4406's six, NSEP's five, start at 0.
        let level = match (limits.mt_profile.as_str(), d.priority.mt_priority()) {
            ("STANAG4406", p) if p < 0 => 0,
            ("STANAG4406", _) => 5,
            ("NSEP", p) if p < 0 => 0,
            ("NSEP", _) => 3,
            (_, p) => p,
        };
        mail.push_str(&format!(" MT-PRIORITY={level}"));
    }
    // A time limit (RFC 2852). Not with a held message, whose release
    // would count against it.
    if hold_for.is_none()
        && let Some(minimum) = limits.deliver_by
        && let Some(by) = DeliverBy::of(a)
    {
        mail.push_str(&format!(" BY={}", by.parameter(minimum)));
    }
    let rcpt = |to: &lettre::Address| {
        let mut line = format!("RCPT TO:<{}>", ascii_domain(to.as_ref()));
        if receipts {
            // RFC 3461 §4.1: the reports the account asks for.
            line.push_str(&format!(" NOTIFY={}", a.dsn.notify()));
            // The address as written, so a report names it (RFC 3461 §4.2).
            // Non-ASCII ones would need RFC 6533's utf-8-addr form; left out.
            let to: &str = to.as_ref();
            if to.is_ascii() {
                line.push_str(&format!(" ORCPT=rfc822;{}", xtext(to)));
            } else {
                // RFC 6533 §3: utf-8-addr-xtext, `\x{…}` for what isn't
                // printable ASCII (and `+`, `=`, `\`).
                let encoded: String = to
                    .chars()
                    .map(|c| match c {
                        '!'..='~' if !matches!(c, '+' | '=' | '\\') => c.to_string(),
                        c => format!("\\x{{{:X}}}", c as u32),
                    })
                    .collect();
                line.push_str(&format!(" ORCPT=utf-8;{encoded}"));
            }
        }
        line
    };
    // Recipients go in as many transactions as the server needs: at most
    // RCPTMAX each (RFC 9422), and those refused with "too many recipients"
    // (452, 4.5.3) in the next one (RFC 5321 §4.5.3.1.10). A server that
    // took no recipient in a transaction ends the send.
    let mut pending: Vec<&lettre::Address> = envelope.to().iter().collect();
    let batch = limits.rcpt_max.unwrap_or(usize::MAX).max(1);
    let mut sent = 0;
    while !pending.is_empty() {
        if limits.mail_max.is_some_and(|max| sent > 0 && sent % max == 0) {
            // The server's limit per connection (RFC 9422 MAILMAX): the
            // rest go over a new one.
            let _ = session.command("QUIT");
            session = open(endpoint, secret, tls)
                .map(|(s, _)| s)
                .context("The message went to some recipients only.")?;
        } else if sent > 0 {
            let reset = session.command("RSET")?;
            ensure!(
                reset.positive(),
                "{} refused to start another message: {}",
                endpoint.host,
                reset.explained()
            );
        }
        let these = take_batch(&mut pending, batch, limits.rcpt_domain_max);
        let replies = transaction(&mut session, &mail, &these, &rcpt, limits.pipelining)?;
        let sender = &replies[0];
        if !sender.positive() {
            let _ = session.command("QUIT");
            let error = Err(Refused(format!(
                "The server refused to send from {from}: {}",
                sender.explained()
            )));
            return if sent == 0 {
                error.context(failed)
            } else {
                error.context("The message went to some recipients only.")
            };
        }
        let mut later = Vec::new();
        for (to, reply) in these.iter().zip(&replies[1..]) {
            if reply.positive() {
                continue;
            }
            if reply.too_many_recipients() {
                later.push(*to);
                continue;
            }
            let _ = session.command("QUIT");
            let error = Err(Refused(reply.explained()));
            return if sent == 0 {
                error.with_context(|| format!("The server refused the recipient {to}."))
            } else {
                error.with_context(|| {
                    format!("The message went to some recipients, but the server refused {to}.")
                })
            };
        }
        if later.len() == these.len() {
            let _ = session.command("QUIT");
            bail!("{} took none of this message's recipients (too many).", endpoint.host);
        }
        // Those refused for their number go first next time.
        later.append(&mut pending);
        pending = later;
        let accepted = if let Some(burl) = &burl {
            session.burl(burl.url)
        } else if limits.chunking {
            session.bdat(&bytes, limits.pipelining)
        } else {
            session.data(&bytes)
        };
        if sent == 0 {
            if let Err(e) = &accepted
                && e.downcast_ref::<BurlRefused>().is_some()
            {
                // Nothing was delivered: the caller may send it as itself.
                let _ = session.command("QUIT");
            }
            accepted.context(failed)?;
        } else {
            // Some recipients already have it: a refused BURL now is a
            // partial send, never one to repeat as a whole.
            accepted
                .map_err(|e| match e.downcast::<BurlRefused>() {
                    Ok(BurlRefused(refused)) => anyhow::Error::new(refused),
                    Err(e) => e,
                })
                .context("The message went to some recipients only.")?;
        }
        sent += 1;
    }
    // Accepted; a failed goodbye doesn't matter.
    let _ = session.command("QUIT");
    Ok(Delivered {
        raw: bytes,
        receipts_unsupported: a.delivery_receipts && !limits.dsn,
    })
}

/// The next transaction's recipients from `pending`: at most `batch`, and
/// with RCPTDOMAINMAX (RFC 9422) at most that many different domains.
fn take_batch<'a>(
    pending: &mut Vec<&'a lettre::Address>,
    batch: usize,
    domains: Option<usize>,
) -> Vec<&'a lettre::Address> {
    let Some(most) = domains else {
        return pending.drain(..batch.min(pending.len())).collect();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut these = Vec::new();
    let mut rest = Vec::new();
    for to in pending.drain(..) {
        let domain = to.domain().to_ascii_lowercase();
        let known = seen.contains(&domain);
        if these.len() < batch && (known || seen.len() < most.max(1)) {
            if !known {
                seen.push(domain);
            }
            these.push(to);
        } else {
            rest.push(to);
        }
    }
    *pending = rest;
    these
}
/// MAIL FROM and the RCPT TO of `to`: with PIPELINING all at once and their
/// replies read after; otherwise one by one, stopping if the sender is
/// refused. The first reply is MAIL FROM's.
fn transaction<S: Read + Write + Patience>(
    session: &mut Session<S>,
    mail: &str,
    to: &[&lettre::Address],
    rcpt: &dyn Fn(&lettre::Address) -> String,
    pipelining: bool,
) -> Result<Vec<Reply>> {
    if pipelining {
        session.send(mail)?;
        for t in to {
            session.send(&rcpt(t))?;
        }
        session.flush()?;
        (0..=to.len()).map(|_| session.reply()).collect()
    } else {
        let mut replies = vec![session.command(mail)?];
        if replies[0].positive() {
            for t in to {
                replies.push(session.command(&rcpt(t))?);
            }
        }
        Ok(replies)
    }
}
/// The message's bytes as they go: signed and encrypted with OpenPGP where
/// the draft asks. Bcc recipients are among those encrypted to; the
/// envelope's recipients are exactly who receives it.
/// Your OpenPGP keys and S/MIME certificates, for signing and encrypting.
pub(crate) struct Keys<'a> {
    pub ring: &'a crate::manage::openpgp::Keyring,
    pub certs: &'a crate::manage::smime::Certificates,
}

pub(crate) fn protected(message: &Message, d: &Draft, keys: &Keys) -> Result<Vec<u8>> {
    let ring = keys.ring;
    let mut raw = message.formatted();
    // With only an S/MIME certificate, S/MIME is how it's protected.
    let smime = d.smime || (ring.secret.is_empty() && (d.sign || d.encrypt));
    if smime && (d.sign || d.encrypt) {
        let envelope = message.envelope();
        let from = envelope.from().map(|f| f.to_string()).unwrap_or_default();
        let to: Vec<String> = envelope.to().iter().map(|t| t.to_string()).collect();
        return crate::manage::smime::protect_with(keys.certs, &raw, &from, &to, d.sign, d.encrypt);
    }
    // Mail from an address with a key tells recipients' apps that key
    // (Autocrypt 1.1 §2.1: in every message, signed or not).
    {
        let envelope = message.envelope();
        let from = envelope.from().map(|f| f.to_string()).unwrap_or_default();
        if let Some(public) = ring.own_public_bytes(&from) {
            let header = format!(
                "Autocrypt: {}\r\n",
                crate::sync::keys::autocrypt_header(&from, &public)
            );
            raw = [header.as_bytes(), &raw].concat();
        }
    }
    if !d.sign && !d.encrypt {
        return Ok(raw);
    }
    let envelope = message.envelope();
    let from = envelope.from().map(|f| f.to_string()).unwrap_or_default();
    let to: Vec<String> = envelope.to().iter().map(|t| t.to_string()).collect();
    crate::manage::openpgp::protect(
        &raw,
        &from,
        &to,
        crate::manage::openpgp::Protect {
            sign: d.sign,
            encrypt: d.encrypt,
            subject: d.encrypt_subject,
            ..Default::default()
        },
        ring,
    )
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
    let named = (!from.is_empty()).then_some(from);
    for address in named.into_iter().chain(to.iter().map(String::as_str)) {
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
    let limit = limits.size.unwrap_or(DEFAULT_LIMIT);
    // RFC 1870 §4: as the server counts it (see `deliver_bytes`).
    let size = if limits.chunking {
        bytes.len()
    } else {
        data_size(bytes)
    };
    ensure!(
        size <= limit,
        "This message is {}; {} accepts up to {}.",
        megabytes(size),
        endpoint.host,
        megabytes(limit)
    );
    // `from` empty: the null reverse-path, for reports such as read
    // receipts, which must never be answered (RFC 8098 §3).
    let mut mail = format!("MAIL FROM:<{from}>");
    if limits.size_offered {
        mail.push_str(&format!(" SIZE={size}"));
    }
    // RFC 6531 §3.4 and RFC 6532 §3: UTF-8 addresses, or headers in UTF-8
    // (such as a message/global-disposition-notification, RFC 6533 §6),
    // need SMTPUTF8.
    let head_end = memchr::memmem::find(bytes, b"\r\n\r\n").unwrap_or(bytes.len());
    let utf8_report = has_global_part(bytes);
    if !from.is_ascii() || to.iter().any(|t| !t.is_ascii()) || utf8_report {
        ensure!(limits.utf8, "{} can't send to this address (no SMTPUTF8).", endpoint.host);
        mail.push_str(" SMTPUTF8");
    } else if !bytes[..head_end].is_ascii() && limits.utf8 {
        mail.push_str(" SMTPUTF8");
    }
    // 8-bit content only where the server takes it (RFC 6152 §3).
    if !bytes.is_ascii() {
        ensure!(
            limits.eight_bit,
            "{} can't take this message's 8-bit content (no 8BITMIME).",
            endpoint.host
        );
        mail.push_str(" BODY=8BITMIME");
    }
    if a.require_tls {
        mail.push_str(" REQUIRETLS");
    }
    let rcpt = |t: &lettre::Address| format!("RCPT TO:<{t}>");
    let addresses: Vec<lettre::Address> = to
        .iter()
        .map(|t| t.parse().with_context(|| format!("“{t}” isn't a valid address.")))
        .collect::<Result<_>>()?;
    let refs: Vec<&lettre::Address> = addresses.iter().collect();
    let replies = transaction(&mut session, &mail, &refs, &rcpt, limits.pipelining)?;
    let shown = if from.is_empty() { "<>" } else { from };
    ensure!(
        replies[0].positive(),
        "The server refused to send from {shown}: {}",
        replies[0].explained()
    );
    for (address, reply) in to.iter().zip(&replies[1..]) {
        ensure!(
            reply.positive(),
            "The server refused the recipient {address}: {}",
            reply.explained()
        );
    }
    if limits.chunking {
        session.bdat(bytes, limits.pipelining)?;
    } else {
        session.data(bytes)?;
    }
    let _ = session.command("QUIT");
    Ok(())
}
/// A message as DATA sends it (RFC 5321 §4.5.2): every line ended by CRLF,
/// a dot doubled at the start of a line, and the ending `.` line. A bare CR
/// or LF (from a forwarded original, or a file) becomes CRLF, so no
/// sequence in the message can be taken by a server for the end of the data
/// and the rest sent on as a message of its own (SMTP smuggling).
#[cfg(test)]
fn dot_stuffed(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    dot_stuff_into(bytes, &mut out).expect("writing to memory");
    out
}
/// The size of `bytes` as DATA sends it, for SIZE (RFC 1870 §4): each
/// line ending (CRLF, or a bare CR or LF, which `dot_stuff_into` makes
/// CRLF) counts two octets, and an unended last line gains its CRLF. The
/// dots stuffed in and the final `.` line aren't counted: RFC 1870 §4
/// counts the message's own octets, which the server gets back after it
/// removes them.
fn data_size(bytes: &[u8]) -> usize {
    let mut size = 0;
    let mut i = 0;
    let mut line_start = true;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                size += 2;
                line_start = true;
            }
            _ => {
                size += 1;
                line_start = false;
            }
        }
        i += 1;
    }
    if !line_start {
        size += 2;
    }
    size
}
/// `bytes` dot-stuffed (RFC 5321 §4.5.2) with CRLF line ends and the final
/// `.` line, written to `out` in pieces of at most a few hundred kilobytes.
fn dot_stuff_into(bytes: &[u8], out: &mut impl Write) -> std::io::Result<()> {
    const PIECE: usize = 256 * 1024;
    let mut buffer = Vec::with_capacity(PIECE + 8);
    let mut line_start = true;
    let mut i = 0;
    while i < bytes.len() {
        if buffer.len() >= PIECE {
            out.write_all(&buffer)?;
            buffer.clear();
        }
        let out = &mut buffer;
        let b = bytes[i];
        if b == b'\r' || b == b'\n' {
            // CRLF, a bare CR or a bare LF: one line ending.
            if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            out.extend_from_slice(b"\r\n");
            line_start = true;
        } else {
            if line_start && b == b'.' {
                out.push(b'.');
            }
            out.push(b);
            line_start = false;
        }
        i += 1;
    }
    if !line_start {
        buffer.extend_from_slice(b"\r\n");
    }
    buffer.extend_from_slice(b".\r\n");
    out.write_all(&buffer)
}
/// A read receipt (MDN, RFC 8098) saying `m` was shown to `a`'s owner:
/// a multipart/report with a human-readable part and the machine-readable
/// disposition.
pub(crate) fn receipt(
    a: &Account,
    m: &crate::manage::model::Mail,
    to: &[String],
    original_recipient: &str,
) -> Vec<u8> {
    let encoded = crate::sync::mime::encoded_words;
    let boundary = format!("mdn-{}", uuid::Uuid::new_v4().simple());
    let domain = a.email.rsplit('@').next().unwrap_or("localhost");
    let now = chrono::Local::now();
    let name = crate::sync::mime::display_name(&a.name);
    let subject = encoded(&format!("Read: {}", m.subject));
    // The sender's Message-ID is theirs to write (and decoded, so CR/LF can
    // hide in an encoded word): kept to one line so it can't add headers.
    let original: String = m
        .message_id
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let original = original.trim();
    // The same for the addresses, which come from the sender's
    // Disposition-Notification-To; all of them are written (RFC 8098 §2.1).
    let to: Vec<String> = to
        .iter()
        .map(|t| t.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ',')).collect())
        .collect();
    let mut out = String::new();
    out.push_str(&format!("From: {name} <{}>\r\n", a.email));
    out.push_str(&format!(
        "To: {}\r\n",
        to.iter().map(|t| format!("<{t}>")).collect::<Vec<_>>().join(", ")
    ));
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
    // RFC 6533 §6: a report naming an address or message id that isn't
    // ASCII is message/global-disposition-notification, its addresses of
    // type utf-8 (which reads UTF-8 as is, as message/global does).
    let international = !a.email.is_ascii()
        || !original.is_ascii()
        || !original_recipient.is_ascii();
    out.push_str(&format!(
        "--{boundary}\r\nContent-Type: message/{}disposition-notification\r\n{}\r\n",
        if international { "global-" } else { "" },
        if international { "Content-Transfer-Encoding: 8bit\r\n" } else { "" },
    ));
    out.push_str(&format!(
        "Reporting-UA: Picacho; Picacho {}\r\n",
        env!("CARGO_PKG_VERSION")
    ));
    // As the message had it (`rfc822;address`), on one line.
    let original_recipient: String = original_recipient
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if original_recipient.contains(';') {
        out.push_str(&format!("Original-Recipient: {}\r\n", original_recipient.trim()));
    }
    let address_type = if a.email.is_ascii() { "rfc822" } else { "utf-8" };
    out.push_str(&format!("Final-Recipient: {address_type};{}\r\n", a.email));
    if !original.is_empty() {
        out.push_str(&format!("Original-Message-ID: {original}\r\n"));
    }
    out.push_str("Disposition: manual-action/MDN-sent-manually; displayed\r\n");
    out.push_str(&format!("--{boundary}--\r\n"));
    out.into_bytes()
}
/// Whether a message has a part of a message/global type (RFC 6532 §3.7,
/// RFC 6533 §6) among its own parts, whose UTF-8 needs SMTPUTF8. Only the
/// top-level parts' own headers are looked at, not what they contain.
fn has_global_part(bytes: &[u8]) -> bool {
    let Ok(mail) = mailparse::parse_mail(bytes) else {
        return false;
    };
    let global = |m: &mailparse::ParsedMail| m.ctype.mimetype.to_ascii_lowercase().starts_with("message/global");
    global(&mail) || mail.subparts.iter().any(global)
}
/// Whether a message can't go as 7- or 8-bit text (RFC 6152): it has a
/// NUL, a bare CR or LF, or a line over 998 octets.
fn needs_binary(bytes: &[u8]) -> bool {
    let mut line = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            0 => return true,
            b'\r' if bytes.get(i + 1) != Some(&b'\n') => return true,
            b'\n' if i == 0 || bytes[i - 1] != b'\r' => return true,
            b'\n' => line = 0,
            b'\r' => {}
            _ => {
                line += 1;
                if line > 998 {
                    return true;
                }
            }
        }
    }
    false
}
/// A value as RFC 3461's xtext: printable ASCII but + and =, else +XX.
/// An address with its domain as ASCII (IDNA's punycode) where the local
/// part is ASCII already; as it is otherwise.
fn ascii_domain(address: &str) -> String {
    match address.rsplit_once('@') {
        Some((local, domain)) if local.is_ascii() && !domain.is_ascii() => {
            match idna::domain_to_ascii(domain) {
                Ok(ascii) => format!("{local}@{ascii}"),
                Err(_) => address.to_owned(),
            }
        }
        _ => address.to_owned(),
    }
}

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

/// Pictures a formatted body places, with the Content-IDs its HTML names
/// them by.
#[derive(Debug, Default)]
struct InlineImages {
    images: Vec<InlineImage>,
}
#[derive(Debug)]
struct InlineImage {
    /// The attachment's file name, as `file:` named it.
    name: String,
    path: std::path::PathBuf,
    /// Without its angle brackets.
    content_id: String,
}
/// The pictures `markdown` places with `![alt](file:<name>)` where `name`
/// is exactly one of `attachments`' file names; each is given a Content-ID
/// at `from`'s domain (RFC 2392, RFC 5322 §3.6.4), random so unique.
fn inline_images(markdown: &str, attachments: &[std::path::PathBuf], from: &str) -> InlineImages {
    let mut found = InlineImages::default();
    let domain = new_message_id(from);
    let domain = domain.trim_end_matches('>').rsplit_once('@').map_or("localhost", |(_, d)| d).to_owned();
    for name in file_images(markdown) {
        if found.images.iter().any(|i| i.name == name) {
            continue;
        }
        let matching: Vec<&std::path::PathBuf> = attachments
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name.as_str()))
            .collect();
        // Two attachments of one name: which is meant can't be told.
        if let [path] = matching[..] {
            found.images.push(InlineImage {
                name,
                path: path.clone(),
                content_id: format!("{}@{domain}", uuid::Uuid::new_v4().simple()),
            });
        }
    }
    found
}
/// The names `![…](file:<name>)` images in `markdown` refer to, in order.
fn file_images(markdown: &str) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(tree) = markdown::to_mdast(markdown, &markdown::ParseOptions::gfm()) else {
        return names;
    };
    fn walk(node: &markdown::mdast::Node, names: &mut Vec<String>) {
        if let markdown::mdast::Node::Image(image) = node
            && let Some(name) = image.url.strip_prefix("file:")
        {
            names.push(name.to_owned());
        }
        for child in node.children().into_iter().flatten() {
            walk(child, names);
        }
    }
    walk(&tree, &mut names);
    names
}

/// The HTML version of a formatted (Markdown) body. Raw HTML in the text is
/// escaped, not passed through.
#[cfg(test)]
pub(crate) fn html_body(markdown: &str) -> String {
    html_body_with(markdown, &InlineImages::default())
}
/// As `html_body`, with `![…](file:<name>)` pictures shown from `inline`'s
/// parts by `cid:` (RFC 2392). One not among them (its attachment removed,
/// or never there) shows its alt text: a `file:` address never goes out,
/// so no local file name or path does either.
fn html_body_with(markdown: &str, inline: &InlineImages) -> String {
    // Each placed picture's address becomes a placeholder the compiler
    // lets through (it drops `cid:` as an unknown protocol); after, the
    // placeholder becomes the `cid:` address.
    let placeholder = |i: usize| format!("https://inline.invalid/{i}");
    let markdown = rewrite_file_images(markdown, true, |name| {
        inline
            .images
            .iter()
            .position(|image| image.name == name)
            .map(placeholder)
    });
    let mut html = html_compiled(&markdown);
    for (i, image) in inline.images.iter().enumerate() {
        html = html.replace(
            &format!("src=\"{}\"", placeholder(i)),
            &format!("src=\"cid:{}\"", image.content_id),
        );
    }
    html
}
/// `markdown` with each `file:` image either pointed at what `resolve`
/// gives for its name, or replaced by its alt text where it gives nothing:
/// escaped to stay text where the result is compiled (`escape`), as it is
/// for the plain text.
fn rewrite_file_images(
    markdown: &str,
    escape: bool,
    resolve: impl Fn(&str) -> Option<String>,
) -> String {
    let Ok(tree) = markdown::to_mdast(markdown, &markdown::ParseOptions::gfm()) else {
        return markdown.to_owned();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    fn walk(
        node: &markdown::mdast::Node,
        escape: bool,
        resolve: &dyn Fn(&str) -> Option<String>,
        edits: &mut Vec<(usize, usize, String)>,
    ) {
        if let markdown::mdast::Node::Image(image) = node
            && let Some(name) = image.url.strip_prefix("file:")
            && let Some(at) = &image.position
        {
            // The alt text, escaped so it stays text.
            let alt: String = image
                .alt
                .chars()
                .flat_map(|c| {
                    let special = matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '!' | '<' | '>' | '#' | '&');
                    (escape && special).then_some('\\').into_iter().chain(std::iter::once(c))
                })
                .collect();
            let replacement = match resolve(name) {
                Some(url) => format!("![{alt}](<{url}>)"),
                None => alt,
            };
            edits.push((at.start.offset, at.end.offset, replacement));
        }
        for child in node.children().into_iter().flatten() {
            walk(child, escape, resolve, edits);
        }
    }
    walk(&tree, escape, &resolve, &mut edits);
    let mut out = markdown.to_owned();
    for (start, end, replacement) in edits.into_iter().rev() {
        out.replace_range(start..end, &replacement);
    }
    out
}
/// The compiled HTML document of a Markdown body.
fn html_compiled(markdown: &str) -> String {
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
    #[test]
    fn enhanced_codes_and_completions_are_strict() {
        assert_eq!(enhanced_code("5.1.1 no such user"), Some((5, 1, 1)));
        assert_eq!(enhanced_code("5.1234.1 odd"), None);
        let r = Reply { code: 354, lines: vec!["go ahead".into()], enhanced: true };
        assert!(!r.positive());
        let r = Reply { code: 250, lines: vec!["5.0.0 mismatched".into()], enhanced: true };
        assert_eq!(r.enhanced(), None);
        let limits = server_limits(["SIZE", "BURL imap://imap.example.com", "LIMITS RCPTDOMAINMAX=2"].into_iter());
        assert!(limits.size_offered && limits.size.is_none() && limits.burl_imap);
        assert_eq!(limits.rcpt_domain_max, Some(2));
    }
    use super::*;

    #[test]
    fn formatted_bodies_become_safe_html_with_line_breaks() {
        let html = html_body("**Hi** _there_\nsecond line\n\n- one\n- two\n\n<script>x</script>");
        assert!(html.contains("<strong>Hi</strong>"));
        assert!(html.contains("<em>there</em><br />"));
        assert!(html.contains("<li>one</li>"));
        assert!(!html.contains("<script>"));
    }

    /// RFC 1870 §4: the size declared for DATA counts bare CR and LF as
    /// the CRLF they're sent as, and the unended last line's CRLF; it
    /// matches what the server gets once it undoes the dot-stuffing.
    /// RFC 2852 §4: BY= in each mode, with trace; the server's minimum
    /// raises R mode's time only; nothing without a time limit.
    #[test]
    fn deliver_by_modes_and_trace() {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        assert_eq!(DeliverBy::of(&a), None);
        a.deliver_late_notify = true;
        a.deliver_by_trace = true;
        assert_eq!(DeliverBy::of(&a), None, "N and T alone ask for nothing");
        a.deliver_within_hours = 1;
        let by = |notify, trace| DeliverBy {
            notify,
            trace,
            ..DeliverBy::of(&a).unwrap()
        };
        assert_eq!(by(false, false).parameter(0), "3600;R");
        assert_eq!(by(true, false).parameter(0), "3600;N");
        assert_eq!(by(false, true).parameter(0), "3600;RT");
        assert_eq!(by(true, true).parameter(0), "3600;NT");
        assert_eq!(by(false, false).parameter(7200), "7200;R");
        assert_eq!(by(true, true).parameter(7200), "3600;NT");
        let long = DeliverBy { seconds: 10_000_000_000, notify: false, trace: false };
        assert_eq!(long.parameter(0), "999999999;R");
    }

    /// RFC 4865 §3: both limits FUTURERELEASE names are kept to.
    #[test]
    fn future_release_keeps_to_the_servers_interval_and_date() {
        let limits = server_limits(["FUTURERELEASE 604800 2030-01-01T00:00:00Z"].into_iter());
        assert_eq!(limits.future_release, 604_800);
        let end = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z").unwrap().timestamp();
        assert_eq!(limits.future_release_until, Some(end));
        let now = end - 10 * 86_400;
        assert_eq!(hold_seconds(&limits, now + 3600, now), Some(3600));
        // Past the interval.
        assert_eq!(hold_seconds(&limits, now + 8 * 86_400, now), None);
        // Within the interval, but after the date-time.
        let late = end - 3600;
        assert_eq!(hold_seconds(&limits, end + 60, late), None);
        assert_eq!(hold_seconds(&limits, end, late), Some(3600));
        // Only an interval (the date-time is optional in practice).
        let open = server_limits(["FUTURERELEASE 600"].into_iter());
        assert_eq!(open.future_release_until, None);
        assert_eq!(hold_seconds(&open, now + 600, now), Some(600));
        // No FUTURERELEASE: nothing is held.
        assert_eq!(hold_seconds(&ServerLimits::default(), now + 60, now), None);
    }

    #[test]
    fn data_size_counts_line_endings_as_sent() {
        for message in [
            &b"a\r\nb\r\n"[..],
            b"a\nb\rc\r\n",
            b"a\n.\n..b\r",
            b"no ending",
            b"",
            b"\r\r\n\n",
        ] {
            let stuffed = dot_stuffed(message);
            // What the server keeps: the stuffed text without the final
            // line, its doubled dots undone.
            let body = &stuffed[..stuffed.len() - 3];
            let kept: usize = body
                .split_inclusive(|b| *b == b'\n')
                .map(|line| line.len() - usize::from(line.starts_with(b"..")))
                .sum();
            assert_eq!(data_size(message), kept, "{:?}", String::from_utf8_lossy(message));
        }
        assert_eq!(data_size(b"a\nb"), 6);
    }

    #[test]
    fn data_ends_only_where_it_should() {
        assert_eq!(dot_stuffed(b"a\r\n.b\r\n"), b"a\r\n..b\r\n.\r\n");
        assert_eq!(dot_stuffed(b"a"), b"a\r\n.\r\n");
        assert_eq!(dot_stuffed(b""), b".\r\n");
        // Bare line endings, as smuggling uses them, become CRLF.
        assert_eq!(dot_stuffed(b"a\n.\nb\r.\r\nc"), b"a\r\n..\r\nb\r\n..\r\nc\r\n.\r\n");
        // Across the pieces it is written in, as in one.
        let long: Vec<u8> = b".x\r\n".repeat(200_000);
        let stuffed = dot_stuffed(&long);
        assert_eq!(stuffed.len(), long.len() + 200_000 + 3);
        assert!(stuffed.starts_with(b"..x\r\n..x") && stuffed.ends_with(b"..x\r\n.\r\n"));
    }

    /// A sender's Message-ID or receipt address with CR/LF in it (an
    /// encoded word decodes to anything) can't add headers to a receipt
    /// or reply.
    #[test]
    fn sender_values_cannot_inject_headers() -> Result<()> {
        // (See also `international_receipts_are_global_reports`.)
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        let m = crate::manage::model::Mail {
            message_id: "<x@y>\r\nBcc: victim@example.com".into(),
            subject: "Hi".into(),
            ..Default::default()
        };
        let raw = String::from_utf8(receipt(
            &a,
            &m,
            &["ana@example.com>\r\nX-Evil: 1".into(), "bo@example.com".into()],
            "rfc822;qa@example.com\r\nX-Evil: 2",
        ))?;
        let headers = raw.split("\r\n\r\n").next().unwrap_or_default();
        assert!(!headers.contains("\r\nBcc:"), "{headers}");
        assert!(!headers.contains("\r\nX-Evil:"), "{headers}");
        // Every address that asked is told (RFC 8098 §2.1).
        assert!(headers.contains("\r\nTo: <ana@example.comX-Evil: 1>, <bo@example.com>\r\n"), "{headers}");
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Re: Hi".into(),
            reply_id: "<x@y>\r\nBcc: victim@example.com".into(),
            ..Draft::default()
        };
        let sent = String::from_utf8_lossy(&build(&a, &d, false)?.formatted()).into_owned();
        assert!(!sent.contains("\r\nBcc:"), "{sent}");
        Ok(())
    }

    /// A formatted draft with its attachments in a fresh folder.
    fn with_files(files: &[(&str, &[u8])]) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let dir = std::env::temp_dir().join(format!("picacho-inline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = files
            .iter()
            .map(|(name, bytes)| {
                let path = dir.join(name);
                std::fs::write(&path, bytes).unwrap();
                path
            })
            .collect();
        (dir, paths)
    }

    /// RFC 2387: a picture a formatted body places by `file:` goes in
    /// multipart/related (type="text/html") beside the HTML, which names
    /// it by `cid:`, and leaves the attachments; others stay attached.
    #[test]
    fn pictures_in_the_body_go_inline_by_content_id() -> Result<()> {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        let (dir, attachments) = with_files(&[("Pasted image.png", b"\x89PNG-1"), ("notes.pdf", b"%PDF")]);
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Look".into(),
            body: "Here:\n\n![Pasted image](<file:Pasted image.png>)\n\nAnd `![x](file:notes.pdf)` in code.".into(),
            formatted: true,
            attachments,
            ..Draft::default()
        };
        let raw = String::from_utf8_lossy(&build(&a, &d, false)?.formatted()).into_owned();
        let parsed = mailparse::parse_mail(raw.as_bytes())?;
        assert_eq!(parsed.ctype.mimetype, "multipart/mixed");
        let alternative = &parsed.subparts[0];
        assert_eq!(alternative.ctype.mimetype, "multipart/alternative");
        assert_eq!(alternative.subparts[0].ctype.mimetype, "text/plain");
        let related = &alternative.subparts[1];
        assert_eq!(related.ctype.mimetype, "multipart/related");
        assert_eq!(related.ctype.params.get("type").map(String::as_str), Some("text/html"));
        let html = related.subparts[0].get_body()?;
        let image = &related.subparts[1];
        assert_eq!(image.ctype.mimetype, "image/png");
        use mailparse::MailHeaderMap as _;
        let cid = image.headers.get_first_value("Content-ID").unwrap();
        let cid = cid.trim().trim_start_matches('<').trim_end_matches('>');
        assert!(cid.ends_with("@example.com"), "{cid}");
        assert!(html.contains(&format!("src=\"cid:{cid}\"")), "{html}");
        assert_eq!(image.get_content_disposition().disposition, mailparse::DispositionType::Inline);
        assert_eq!(image.get_body_raw()?, b"\x89PNG-1");
        // The PDF stays an attachment, and only it; the code span is text.
        assert_eq!(parsed.subparts.len(), 2);
        assert!(parsed.subparts[1].ctype.mimetype == "application/pdf");
        assert!(html.contains("<code>![x](file:notes.pdf)</code>"), "{html}");
        // No local name or `file:` address goes out as a picture's source;
        // the plain text reads the alt text.
        assert!(!html.contains("src=\"file:") && !html.contains("Pasted image.png"), "{html}");
        let plain = alternative.subparts[0].get_body()?;
        assert!(plain.contains("Pasted image") && !plain.contains("<file:"), "{plain}");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// A `file:` picture with no attachment of its name (removed, or never
    /// there), or two of that name, shows its alt text; other images
    /// (https, data:) are as before.
    #[test]
    fn unmatched_file_pictures_are_their_alt_text() -> Result<()> {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        let (dir, attachments) = with_files(&[("other.txt", b"x")]);
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Look".into(),
            body: "![My *photo*](file:/home/me/secret/photo.png) and ![web](https://example.com/a.png)".into(),
            formatted: true,
            attachments,
            ..Draft::default()
        };
        let raw = String::from_utf8_lossy(&build(&a, &d, false)?.formatted()).into_owned();
        assert!(!raw.contains("file:") && !raw.contains("/home/me"), "{raw}");
        assert!(!raw.contains("multipart/related"));
        let parsed = mailparse::parse_mail(raw.as_bytes())?;
        let html = parsed.subparts[0].subparts[1].get_body()?;
        // Its alt text as text (Markdown's alt is plain: emphasis dropped).
        assert!(html.contains("<p>My photo and "), "{html}");
        assert!(html.contains("<img src=\"https://example.com/a.png\" alt=\"web\" />"), "{html}");
        // Unchanged where there are no `file:` pictures.
        assert_eq!(
            html_body_with("![web](https://example.com/a.png)", &InlineImages::default()),
            html_body("![web](https://example.com/a.png)")
        );
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// Without `file:` pictures, the MIME structure is as before: the
    /// alternative straight in mixed, and the HTML as `html_body` made it.
    #[test]
    fn messages_without_inline_pictures_are_unchanged() -> Result<()> {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        let (dir, attachments) = with_files(&[("photo.png", b"\x89PNG")]);
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Look".into(),
            body: "**Bold** and ![web](https://example.com/a.png)".into(),
            formatted: true,
            attachments,
            ..Draft::default()
        };
        let raw = build(&a, &d, false)?.formatted();
        let parsed = mailparse::parse_mail(&raw)?;
        assert_eq!(parsed.subparts[0].ctype.mimetype, "multipart/alternative");
        assert_eq!(parsed.subparts[0].subparts[1].ctype.mimetype, "text/html");
        assert_eq!(
            parsed.subparts[0].subparts[1].get_body()?.replace("\r\n", "\n"),
            html_body(&d.body)
        );
        assert_eq!(parsed.subparts[0].subparts[0].get_body()?, flowed(&d.body));
        assert_eq!(parsed.subparts[1].get_content_disposition().disposition, mailparse::DispositionType::Attachment);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    /// RFC 6532 §3.7: a forwarded message whose headers are in UTF-8 is
    /// attached as message/global; an ASCII one stays message/rfc822, as
    /// it is.
    #[test]
    fn internationalized_forwards_are_message_global() {
        let kind = header::ContentType::parse("message/rfc822").unwrap();
        let formatted = |part: SinglePart| String::from_utf8_lossy(&part.formatted()).into_owned();
        let global = formatted(attachment_part(
            "fwd.eml".into(),
            "From: Jörg <jörg@bücher.example>\r\nSubject: Grüße\r\n\r\nHallo\r\n".as_bytes().to_vec(),
            kind.clone(),
        ));
        assert!(global.contains("Content-Type: message/global"), "{global}");
        let plain = formatted(attachment_part(
            "fwd.eml".into(),
            b"From: ana@example.com\r\nSubject: Hi\r\n\r\nHall\xc3\xb6\r\n".to_vec(),
            kind,
        ));
        assert!(plain.contains("Content-Type: message/rfc822"), "{plain}");
        assert!(plain.contains("Content-Transfer-Encoding: 8bit"), "{plain}");
    }

    /// RFC 6533 §6: a read receipt naming non-ASCII addresses is a
    /// message/global-disposition-notification with utf-8 addresses; an
    /// ASCII one stays message/disposition-notification.
    #[test]
    fn international_receipts_are_global_reports() {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "jörg@bücher.example".into();
        let m = crate::manage::model::Mail {
            message_id: "<id@example.com>".into(),
            subject: "Grüße".into(),
            ..Default::default()
        };
        let to = ["ana@example.com".to_owned()];
        let raw = String::from_utf8(receipt(&a, &m, &to, "")).unwrap();
        assert!(raw.contains("Content-Type: message/global-disposition-notification\r\n"), "{raw}");
        assert!(raw.contains("Final-Recipient: utf-8;jörg@bücher.example\r\n"), "{raw}");
        assert!(has_global_part(raw.as_bytes()));
        a.email = "me@example.com".into();
        let raw = String::from_utf8(receipt(&a, &m, &to, "rfc822;me@example.com")).unwrap();
        assert!(raw.contains("Content-Type: message/disposition-notification\r\n"), "{raw}");
        assert!(raw.contains("Final-Recipient: rfc822;me@example.com\r\n"), "{raw}");
        assert!(!has_global_part(raw.as_bytes()));
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
        let limited = server_limits(
            ["ENHANCEDSTATUSCODES", "LIMITS RCPTMAX=50 MAILMAX=0 RCPTDOMAINMAX=5"].into_iter(),
        );
        assert!(limited.enhanced && !modern.enhanced);
        assert_eq!((limited.rcpt_max, limited.mail_max), (Some(50), None));
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

    /// A stream whose replies are scripted and whose writes are kept.
    struct Scripted {
        replies: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }
    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(buf)
        }
    }
    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Patience for Scripted {
        fn wait_for(&mut self, _: Option<Duration>) {}
    }
    fn scripted(replies: &str) -> Session<Scripted> {
        Session::new(Scripted {
            replies: std::io::Cursor::new(replies.as_bytes().to_vec()),
            written: Vec::new(),
        })
    }

    /// RFC 4468 §3: a server refusing to fetch the URL is told apart from
    /// other failures (nothing was delivered, so it can be sent as itself),
    /// and an accepted BURL is an ordinary reply.
    #[test]
    fn refused_burl_is_its_own_error() {
        let mut s = scripted("554 5.7.0 URL not authorized\r\n");
        let e = s.burl("imap://ana@imap.example/Sent;UIDVALIDITY=1/;UID=2;urlauth=submit+ana:internal:00").unwrap_err();
        assert!(e.downcast_ref::<BurlRefused>().is_some(), "{e:#}");
        assert!(format!("{e:#}").contains("couldn't fetch the message"), "{e:#}");
        let mut s = scripted("250 2.0.0 Queued\r\n");
        assert!(s.burl("imap://x").is_ok());
        // Any other refusal isn't one.
        let mut s = scripted("554 5.3.4 Too big\r\n");
        assert!(s.data(b"x").unwrap_err().downcast_ref::<BurlRefused>().is_none());
    }

    /// RFC 2034 and RFC 3463: once EHLO offers ENHANCEDSTATUSCODES, every
    /// refusal (a command, the sign-in, DATA and its end, BDAT) says what
    /// its code means; a code repeated on each line shows once; and from a
    /// server that didn't offer them, a code-like text is only text.
    #[test]
    fn enhanced_codes_are_read_everywhere_a_server_refuses() {
        let ehlo = "250-fixture\r\n250 ENHANCEDSTATUSCODES\r\n";
        let text = |e: anyhow::Error| format!("{e:#}");
        // A plain command.
        let mut s = scripted(&format!("{ehlo}550-5.7.1 Relaying\r\n550 5.7.1 denied\r\n"));
        s.ehlo().unwrap();
        let e = text(s.expect("RSET", "Refused").unwrap_err());
        assert_eq!(e, "Refused: The server refused this by its policy (550 5.7.1 Relaying denied)");
        // DATA refused before the message.
        let mut s = scripted(&format!("{ehlo}554 5.3.4 Too big\r\n"));
        s.ehlo().unwrap();
        let e = text(s.data(b"x").unwrap_err());
        assert!(e.contains("The message is too large (554 5.3.4 Too big)"), "{e}");
        // DATA refused after it: no longer taken as accepted.
        let mut s = scripted(&format!("{ehlo}354 go\r\n550 5.7.26 DMARC\r\n"));
        s.ehlo().unwrap();
        let e = text(s.data(b"x").unwrap_err());
        assert!(e.contains("sender checks (550 5.7.26 DMARC)"), "{e}");
        // BDAT.
        let mut s = scripted(&format!("{ehlo}552 5.2.2 Mailbox full\r\n"));
        s.ehlo().unwrap();
        let e = text(s.bdat(b"x", false).unwrap_err());
        assert!(e.contains("That mailbox is full (552 5.2.2"), "{e}");
        // The sign-in.
        let mut s = scripted(&format!("{ehlo}535 5.7.8 Bad credentials\r\n"));
        let limits = ServerLimits {
            auth: vec!["PLAIN".into()],
            ..s.ehlo().unwrap()
        };
        let endpoint = Endpoint {
            host: "mail.example".into(),
            port: 587,
            user: "me".into(),
            oauth: false,
            implicit_tls: false,
            kerberos: false,
        };
        let e = text(s.authenticate(&endpoint, &limits, "pw").unwrap_err());
        assert_eq!(
            e,
            "mail.example refused the sign-in with PLAIN: The sign-in wasn't accepted (535 5.7.8 Bad credentials)"
        );
        // Without ENHANCEDSTATUSCODES: the text as it is.
        let mut s = scripted("250-fixture\r\n250 SIZE\r\n550 5.1.1 is just text\r\n");
        s.ehlo().unwrap();
        let e = text(s.expect("RSET", "Refused").unwrap_err());
        assert_eq!(e, "Refused: 550 5.1.1 is just text");
    }

    #[test]
    fn enhanced_codes_explain_refusals() {
        let reply = |code, line: &str| Reply {
            code,
            lines: vec![line.to_owned()],
            enhanced: true,
        };
        let gone = reply(550, "5.1.1 <x@example.com>: Recipient address rejected");
        assert_eq!(gone.enhanced(), Some((5, 1, 1)));
        assert!(gone.explained().starts_with("That mailbox doesn't exist (550 5.1.1"));
        let plain = reply(550, "No such user here");
        assert_eq!(plain.enhanced(), None);
        assert_eq!(plain.explained(), "550 No such user here");
        assert!(reply(452, "4.5.3 Too many recipients").too_many_recipients());
        assert!(reply(452, "Too many recipients").too_many_recipients());
        assert!(!reply(452, "4.2.2 Mailbox full").too_many_recipients());
        assert!(!reply(550, "5.7.1 Relaying denied").too_many_recipients());
        assert_eq!(enhanced_code("2.0.0 OK"), Some((2, 0, 0)));
        assert_eq!(enhanced_code("1.2.3 no"), None);
    }

    #[test]
    fn recipients_go_in_as_many_transactions_as_the_server_needs() {
        let a: lettre::Address = "a@x.org".parse().unwrap();
        let b: lettre::Address = "b@x.org".parse().unwrap();
        let script = "250 2.1.0 OK\r\n250 2.1.5 OK\r\n452 4.5.3 Too many recipients\r\n";
        let mut session = scripted(script);
        session.enhanced = true;
        let rcpt = |t: &lettre::Address| format!("RCPT TO:<{t}> ORCPT=rfc822;{}", xtext(AsRef::<str>::as_ref(t)));
        let replies =
            transaction(&mut session, "MAIL FROM:<me@x.org>", &[&a, &b], &rcpt, true).unwrap();
        assert_eq!(replies.len(), 3);
        assert!(replies[1].positive() && replies[2].too_many_recipients());
        let written = String::from_utf8(session.reader.into_inner().written).unwrap();
        assert_eq!(
            written,
            "MAIL FROM:<me@x.org>\r\nRCPT TO:<a@x.org> ORCPT=rfc822;a@x.org\r\nRCPT TO:<b@x.org> ORCPT=rfc822;b@x.org\r\n"
        );
    }

    /// RFC 5322 §3.6.4: every sent message has a Message-ID at the
    /// sender's domain (the alias's, sent as one), random each time, and
    /// never the computer's name.
    #[test]
    fn message_ids_use_the_senders_domain() -> Result<()> {
        let mut a = Account::new(crate::manage::model::Provider::Imap);
        a.email = "me@example.com".into();
        let d = Draft {
            to: "ana@example.com".into(),
            subject: "Hi".into(),
            ..Draft::default()
        };
        let id = |m: &Message| message_id_of(m);
        let first = id(&build(&a, &d, false)?);
        let second = id(&build(&a, &d, false)?);
        assert!(first.starts_with('<') && first.ends_with("@example.com>"), "{first}");
        assert_ne!(first, second);
        assert_eq!(first.len(), "<@example.com>".len() + 32);
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
        if !host.trim().is_empty() {
            assert!(!first.contains(host.trim()), "{first}");
        }
        let alias = Draft {
            send_as: "shop@alias.example".into(),
            ..d.clone()
        };
        assert!(id(&build(&a, &alias, false)?).ends_with("@alias.example>"));
        // Drafts saved on the server get none: their sent message will.
        assert!(id(&build(&a, &d, true)?).is_empty());
        assert!(new_message_id("ana@bücher.example").ends_with("@xn--bcher-kva.example>"));
        assert!(new_message_id("odd").ends_with("@localhost>"));
        Ok(())
    }

    #[test]
    fn recipient_fields_parse_names_and_allow_empty() {
        assert!(recipients("  ").unwrap().is_empty());
        let r = recipients("Ana <ana@example.com>, bo@example.com").unwrap();
        assert_eq!(r[0], (Some("Ana".into()), "ana@example.com".into()));
        assert_eq!(r[1].1, "bo@example.com");
    }
}
