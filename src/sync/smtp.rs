//! Build a MIME message and deliver it once. Acceptance is the send boundary.
use crate::manage::model::{Account, Draft, Secret};
use anyhow::{Context, Result, bail, ensure};
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart, header};
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
    let from = Mailbox::new(Some(a.name.clone()), a.email.parse()?);
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
            format!("<{}>", if alias.is_empty() { &a.email } else { alias }),
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
        multipart = multipart.singlepart(
            Attachment::new(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
            .body(bytes, content_type(path)),
        );
    }
    Ok(builder.multipart(multipart)?)
}
/// An attachment's type from its name (image/png, application/pdf…), so
/// recipients' apps show it; unknown kinds are sent as plain bytes.
pub(crate) fn content_type(path: &std::path::Path) -> header::ContentType {
    let guess = mime_guess::from_path(path).first_or_octet_stream();
    header::ContentType::parse(guess.essence_str())
        .unwrap_or(header::ContentType::parse("application/octet-stream").unwrap())
}
/// What the server said about itself after EHLO that lettre doesn't keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerLimits {
    /// SIZE (RFC 1870): the largest message it accepts, if it says.
    pub(crate) size: Option<usize>,
    /// DSN (RFC 3461): it can report delivery.
    pub(crate) dsn: bool,
    pub(crate) eight_bit: bool,
    pub(crate) utf8: bool,
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

pub(crate) fn deliver(
    a: &Account,
    d: &Draft,
    secret: &Secret,
    tls: lettre::transport::smtp::client::TlsParameters,
) -> Result<Delivered> {
    use lettre::transport::smtp::{
        authentication::{Credentials, Mechanism},
        client::SmtpConnection,
        commands::{Data, Ehlo, Mail, Quit, Rcpt},
        extension::{ClientId, MailBodyParameter, MailParameter, RcptParameter},
    };
    let message = build(a, d, false)?;
    let bytes = message.formatted();
    let hello = ClientId::default();
    let timeout = Some(Duration::from_secs(45));
    let implicit = a.smtp_port == 465;
    let failed =
        "SMTP delivery failed. Your draft has been kept; check the server and account settings.";
    let mut conn = SmtpConnection::connect(
        (a.smtp_host.as_str(), a.smtp_port),
        timeout,
        &hello,
        implicit.then_some(&tls),
        None,
    )
    .context(failed)?;
    if !implicit {
        // Never send credentials or mail unencrypted.
        ensure!(
            conn.can_starttls(),
            "{} doesn't offer STARTTLS on port {}; Picacho only sends over encrypted connections.",
            a.smtp_host,
            a.smtp_port
        );
        conn.starttls(&tls, &hello).context(failed)?;
    }
    // lettre keeps only some EHLO keywords; ask again for SIZE and DSN.
    let ehlo = conn.command(Ehlo::new(hello)).context(failed)?;
    let limits = server_limits(ehlo.message().skip(1));
    let limit = limits.size.unwrap_or(DEFAULT_LIMIT);
    if bytes.len() > limit {
        bail!(
            "This message is {} with its attachments; {} accepts up to {}. Remove an attachment or share it by link.",
            megabytes(bytes.len()),
            a.smtp_host,
            megabytes(limit)
        );
    }
    let credentials = Credentials::new(
        if a.oauth {
            a.email.clone()
        } else {
            a.login().to_owned()
        },
        if a.oauth {
            secret.access_token.clone()
        } else {
            secret.password.clone()
        },
    );
    let mechanisms: &[Mechanism] = if a.oauth {
        &[Mechanism::Xoauth2]
    } else {
        &[Mechanism::Plain, Mechanism::Login]
    };
    conn.auth(mechanisms, &credentials).context(failed)?;
    let envelope = message.envelope();
    let mut mail_options = vec![];
    if limits.size.is_some() {
        mail_options.push(MailParameter::Size(bytes.len()));
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
            a.smtp_host
        );
        mail_options.push(MailParameter::SmtpUtfEight);
    }
    if !bytes.is_ascii() && limits.eight_bit {
        mail_options.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }
    let receipts = a.delivery_receipts && limits.dsn;
    if receipts {
        // RFC 3461: return headers only, identified by the draft.
        mail_options.push(MailParameter::Other {
            keyword: "RET".into(),
            value: Some("HDRS".into()),
        });
        mail_options.push(MailParameter::Other {
            keyword: "ENVID".into(),
            value: Some(xtext(&d.id)),
        });
    }
    conn.command(Mail::new(envelope.from().cloned(), mail_options))
        .context(failed)?;
    for to in envelope.to() {
        let params = if receipts {
            vec![RcptParameter::Other {
                keyword: "NOTIFY".into(),
                value: Some("SUCCESS,FAILURE,DELAY".into()),
            }]
        } else {
            vec![]
        };
        conn.command(Rcpt::new(to.clone(), params))
            .with_context(|| format!("The server refused the recipient {to}."))?;
    }
    conn.command(Data).context(failed)?;
    conn.message(&bytes).context(failed)?;
    // Accepted; a failed goodbye doesn't matter.
    let _ = conn.command(Quit);
    Ok(Delivered {
        message,
        receipts_unsupported: a.delivery_receipts && !limits.dsn,
    })
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
