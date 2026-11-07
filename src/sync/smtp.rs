//! Build a MIME message and deliver it once. Acceptance is the send boundary.
use crate::manage::model::{Account, Draft, Secret};
use anyhow::{Context, Result, ensure};
use std::time::Duration;

pub(crate) fn deliver(
    a: &Account,
    d: &Draft,
    secret: &Secret,
    tls: lettre::transport::smtp::client::TlsParameters,
) -> Result<lettre::Message> {
    use lettre::{
        Message, Transport,
        message::{Attachment, Mailbox, MultiPart, SinglePart},
        transport::smtp::{
            authentication::{Credentials, Mechanism},
            client::Tls,
        },
    };
    let mut builder = Message::builder()
        .from(Mailbox::new(Some(a.name.clone()), a.email.parse()?))
        .subject(&d.subject);
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
    ensure!(count > 0, "Add at least one recipient.");
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
        let meta = std::fs::metadata(path)?;
        total += meta.len() as usize;
        ensure!(
            total < 18 * 1024 * 1024,
            "Attachments exceed the 18 MB sending limit."
        );
        let bytes = std::fs::read(path)?;
        multipart = multipart.singlepart(
            Attachment::new(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
            .body(bytes, "application/octet-stream".parse()?),
        );
    }
    let message = builder.multipart(multipart)?;
    let transport = lettre::SmtpTransport::builder_dangerous(&a.smtp_host)
        .port(a.smtp_port)
        .tls(if a.smtp_port == 465 {
            Tls::Wrapper(tls)
        } else {
            Tls::Required(tls)
        })
        .timeout(Some(Duration::from_secs(45)))
        .credentials(Credentials::new(
            a.email.clone(),
            if a.oauth {
                secret.access_token.clone()
            } else {
                secret.password.clone()
            },
        ))
        .authentication(if a.oauth {
            vec![Mechanism::Xoauth2]
        } else {
            vec![Mechanism::Plain, Mechanism::Login]
        })
        .build();
    transport.send(&message).context(
        "SMTP delivery failed. Your draft has been kept; check the server and account settings.",
    )?;
    Ok(message)
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

    #[test]
    fn recipient_fields_parse_names_and_allow_empty() {
        assert!(recipients("  ").unwrap().is_empty());
        let r = recipients("Ana <ana@example.com>, bo@example.com").unwrap();
        assert_eq!(r[0], (Some("Ana".into()), "ana@example.com".into()));
        assert_eq!(r[1].1, "bo@example.com");
    }
}
