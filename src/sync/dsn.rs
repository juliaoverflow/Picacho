//! Delivery reports (DSN, RFC 3464) read from a received message: what
//! became of each recipient of a message sent with delivery receipts
//! asked for (RFC 3461), for the app to show.
use mailparse::ParsedMail;

/// What a delivery report says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeliveryReport {
    /// The ENVID it was asked for with (RFC 3464 §2.2.1, Original-Envelope-Id):
    /// the draft's id, as Picacho sends it.
    pub(crate) envelope_id: String,
    /// The server that wrote the report (Reporting-MTA), without its type.
    pub(crate) reporting_mta: String,
    /// Each recipient's outcome.
    pub(crate) recipients: Vec<RecipientStatus>,
}

/// One recipient's outcome in a delivery report (RFC 3464 §2.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecipientStatus {
    /// Who, as the sender wrote it (Original-Recipient) where the report
    /// says, else as delivered to (Final-Recipient); without its type.
    pub(crate) recipient: String,
    pub(crate) action: Action,
    /// The enhanced status code (RFC 3463), as class, subject, detail.
    pub(crate) status: Option<(u8, u16, u16)>,
    /// The receiving server's own words (Diagnostic-Code), without its
    /// type.
    pub(crate) diagnostic: String,
    /// The server that refused or took it (Remote-MTA), without its type.
    pub(crate) remote_mta: String,
    /// Until when the reporting server keeps trying (Will-Retry-Until), as
    /// written.
    pub(crate) retry_until: String,
}

/// What happened to a recipient's copy (RFC 3464 §2.3.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Action {
    /// It couldn't be delivered.
    Failed,
    /// Not yet, and still trying.
    Delayed,
    /// Delivered.
    Delivered,
    /// Passed to a system that doesn't report delivery.
    Relayed,
    /// Delivered, and sent on to other addresses.
    Expanded,
    /// An action this reader doesn't know.
    #[default]
    Other,
}

impl RecipientStatus {
    /// In a sentence, for showing: what happened and, for a failure or
    /// delay, why (the enhanced code's meaning where known).
    pub(crate) fn summary(&self) -> String {
        let action = match self.action {
            Action::Failed => "failed",
            Action::Delayed => "delayed",
            Action::Delivered => "delivered",
            Action::Relayed => "relayed",
            Action::Expanded => "expanded",
            Action::Other => "other",
        };
        let why = self
            .status
            .and_then(|(c, s, d)| crate::sync::smtp::meaning(c, s, d))
            .filter(|_| matches!(self.action, Action::Failed | Action::Delayed));
        match why {
            Some(why) => t!("sync-dsn-status-why", action = action, recipient = self.recipient, why = why),
            None => t!("sync-dsn-status", action = action, recipient = self.recipient),
        }
    }
}

/// The delivery report in `raw`, if it is one: a multipart/report whose
/// report-type is delivery-status (RFC 3464 §2), with a
/// message/delivery-status part, or message/global-delivery-status for
/// internationalized addresses (RFC 6533 §6).
pub(crate) fn parse(raw: &[u8]) -> Option<DeliveryReport> {
    let mail = mailparse::parse_mail(raw).ok()?;
    let report = find_report(&mail)?;
    let body = report.get_body_raw().ok()?;
    parse_fields(&body)
}

fn find_report<'a>(mail: &'a ParsedMail<'a>) -> Option<&'a ParsedMail<'a>> {
    let kind = mail.ctype.mimetype.to_ascii_lowercase();
    if kind == "message/delivery-status" || kind == "message/global-delivery-status" {
        return Some(mail);
    }
    let report = kind == "multipart/report"
        && mail
            .ctype
            .params
            .get("report-type")
            .is_some_and(|t| t.eq_ignore_ascii_case("delivery-status"));
    // A report may arrive inside a wrapper (a forward, a signed message);
    // only its own parts are looked in, at most a few levels down.
    if report || kind.starts_with("multipart/") {
        return mail.subparts.iter().take(16).find_map(|p| find_report(p));
    }
    None
}

/// The report's fields (RFC 3464 §2.1): a group of per-message fields,
/// then one group per recipient, each group ended by a blank line.
fn parse_fields(body: &[u8]) -> Option<DeliveryReport> {
    let text = String::from_utf8_lossy(body).replace("\r\n", "\n");
    let mut groups = text
        .split("\n\n")
        .filter(|g| !g.trim().is_empty())
        .map(fields);
    let message = groups.next()?;
    let field = |h: &[(String, String)], name: &str| -> String {
        h.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    // `type;value` fields (address-type, mta-name-type, diagnostic-type):
    // the value alone.
    let value = |s: String| -> String {
        match s.split_once(';') {
            Some((_, v)) => v.trim().to_owned(),
            None => s,
        }
    };
    let mut report = DeliveryReport {
        envelope_id: field(&message, "Original-Envelope-Id"),
        reporting_mta: value(field(&message, "Reporting-MTA")),
        recipients: Vec::new(),
    };
    for h in groups.take(1000) {
        let final_recipient = value(field(&h, "Final-Recipient"));
        if final_recipient.is_empty() {
            continue;
        }
        let original = value(field(&h, "Original-Recipient"));
        let action = match field(&h, "Action").to_ascii_lowercase().as_str() {
            "failed" => Action::Failed,
            "delayed" => Action::Delayed,
            "delivered" => Action::Delivered,
            "relayed" => Action::Relayed,
            "expanded" => Action::Expanded,
            _ => Action::Other,
        };
        report.recipients.push(RecipientStatus {
            recipient: decode_utf8_addr(if original.is_empty() {
                &final_recipient
            } else {
                &original
            }),
            action,
            status: crate::sync::smtp::enhanced_code(&field(&h, "Status")),
            diagnostic: value(field(&h, "Diagnostic-Code")),
            remote_mta: value(field(&h, "Remote-MTA")),
            retry_until: field(&h, "Will-Retry-Until"),
        });
    }
    (!report.recipients.is_empty()).then_some(report)
}

/// One group's fields, as (name, value): continuation lines (starting
/// with a space or tab) unfolded into the field before (RFC 5322 §2.2.3).
fn fields(group: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in group.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = out.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    out
}

/// An address as RFC 6533 §3's utf-8-addr-xtext writes it (`\x{E9}`) or
/// RFC 3461's xtext (`+2B`), decoded; others as they are.
fn decode_utf8_addr(address: &str) -> String {
    let mut out = String::with_capacity(address.len());
    let mut rest = address;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("\\x{")
            && let Some(end) = after.find('}')
            && let Some(decoded) = u32::from_str_radix(&after[..end], 16).ok().and_then(char::from_u32)
        {
            out.push(decoded);
            rest = &after[end + 1..];
            continue;
        }
        if c == '+'
            && let Some(hex) = rest.get(1..3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
            && byte.is_ascii()
        {
            out.push(byte as char);
            rest = &rest[3..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNCE: &[u8] = b"From: Mail Delivery System <MAILER-DAEMON@mx.example.com>\r\n\
To: qa@example.com\r\n\
Subject: Undelivered Mail Returned to Sender\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/report; report-type=delivery-status;\r\n\tboundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/plain\r\n\
\r\n\
Your message could not be delivered.\r\n\
--b1\r\n\
Content-Type: message/delivery-status\r\n\
\r\n\
Reporting-MTA: dns; mx.example.com\r\n\
Original-Envelope-Id: draft-42\r\n\
Arrival-Date: Mon, 23 Nov 2026 10:00:00 +0000\r\n\
\r\n\
Final-Recipient: rfc822; nobody@example.org\r\n\
Original-Recipient: rfc822;Nobody+2BTag@example.org\r\n\
Action: failed\r\n\
Status: 5.1.1\r\n\
Remote-MTA: dns; mx.example.org\r\n\
Diagnostic-Code: smtp; 550 5.1.1 <nobody@example.org>:\r\n\t\
Recipient address rejected: User unknown\r\n\
\r\n\
Final-Recipient: rfc822; slow@example.net\r\n\
Action: delayed\r\n\
Status: 4.4.1\r\n\
Will-Retry-Until: Fri, 27 Nov 2026 10:00:00 +0000\r\n\
\r\n\
Final-Recipient: rfc822; ana@example.com\r\n\
Action: delivered\r\n\
Status: 2.0.0\r\n\
\r\n\
--b1\r\n\
Content-Type: text/rfc822-headers\r\n\
\r\n\
Subject: Hello\r\n\
--b1--\r\n";

    /// RFC 3464: a report's per-message and per-recipient fields, folded
    /// lines unfolded, type prefixes dropped, xtext decoded.
    #[test]
    fn delivery_reports_say_what_became_of_each_recipient() {
        let report = parse(BOUNCE).expect("a delivery report");
        assert_eq!(report.envelope_id, "draft-42");
        assert_eq!(report.reporting_mta, "mx.example.com");
        assert_eq!(report.recipients.len(), 3);
        let [gone, slow, fine] = &report.recipients[..] else { unreachable!() };
        assert_eq!(gone.recipient, "Nobody+Tag@example.org");
        assert_eq!(gone.action, Action::Failed);
        assert_eq!(gone.status, Some((5, 1, 1)));
        assert_eq!(gone.remote_mta, "mx.example.org");
        assert!(gone.diagnostic.starts_with("550 5.1.1 <nobody@example.org>:"), "{}", gone.diagnostic);
        assert!(gone.diagnostic.ends_with("User unknown"), "{}", gone.diagnostic);
        assert_eq!(
            gone.summary(),
            "Nobody+Tag@example.org couldn't be delivered: That mailbox doesn't exist."
        );
        assert_eq!(slow.action, Action::Delayed);
        assert_eq!(slow.retry_until, "Fri, 27 Nov 2026 10:00:00 +0000");
        assert!(slow.summary().starts_with("slow@example.net is delayed"), "{}", slow.summary());
        assert_eq!(fine.summary(), "ana@example.com was delivered.");
    }

    /// RFC 6533 §6: an internationalized report, with its utf-8 address
    /// type; and messages that aren't reports aren't read as one.
    #[test]
    fn internationalized_reports_and_non_reports() {
        let report = parse(
            "Content-Type: multipart/report; report-type=delivery-status; boundary=x\r\n\r\n\
--x\r\nContent-Type: message/global-delivery-status\r\n\r\n\
Reporting-MTA: dns; mx.example\r\n\r\n\
Final-Recipient: utf-8; j\\x{F6}rg@b\\x{FC}cher.example\r\nAction: delivered\r\nStatus: 2.0.0\r\n\r\n\
--x--\r\n"
                .as_bytes(),
        )
        .expect("a report");
        assert_eq!(report.recipients[0].recipient, "jörg@bücher.example");
        assert!(parse(b"Subject: hi\r\nContent-Type: text/plain\r\n\r\nNot a report.\r\n").is_none());
        // A read receipt is a report too, but not of delivery.
        assert!(
            parse(b"Content-Type: multipart/report; report-type=disposition-notification; boundary=y\r\n\r\n--y\r\nContent-Type: message/disposition-notification\r\n\r\nFinal-Recipient: rfc822; a@b\r\n--y--\r\n")
                .is_none()
        );
    }
}
