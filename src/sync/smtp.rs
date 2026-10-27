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
    let recipients = mailparse::addrparse(&d.to)
        .context("Enter valid recipient email addresses, separated by commas")?;
    let mut count = 0;
    for address in recipients.iter() {
        match address {
            mailparse::MailAddr::Single(s) => {
                builder = builder.to(Mailbox::new(s.display_name.clone(), s.addr.parse()?));
                count += 1;
            }
            mailparse::MailAddr::Group(g) => {
                for s in &g.addrs {
                    builder = builder.to(Mailbox::new(s.display_name.clone(), s.addr.parse()?));
                    count += 1;
                }
            }
        }
    }
    ensure!(count > 0, "Add at least one recipient.");
    if !d.reply_id.is_empty() {
        builder = builder.in_reply_to(d.reply_id.clone());
    }
    let mut multipart = MultiPart::mixed().singlepart(SinglePart::plain(d.body.clone()));
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
