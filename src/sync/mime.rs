//! MIME decoding shared by downloaded mail and sent copies.
use crate::manage::model::*;
use anyhow::Result;
use mailparse::MailHeaderMap;
pub fn parse_message(
    raw: &[u8],
    account: &Account,
    id: &str,
    folder: Folder,
    remote: &str,
    uid: u32,
) -> Result<Mail> {
    let p = mailparse::parse_mail(raw)?;
    let header = |name: &str| p.headers.get_first_value(name).unwrap_or_default();
    let mixed = p.ctype.mimetype.eq_ignore_ascii_case("multipart/mixed");
    let from_header = header("From");
    let (sender, from) = mailparse::addrparse(&from_header)
        .ok()
        .and_then(|v| v.extract_single_info())
        .map(|s| (s.display_name.unwrap_or_else(|| s.addr.clone()), s.addr))
        .unwrap_or_else(|| (from_header.clone(), from_header));
    let mut body = String::new();
    let mut html = None;
    let mut attachments = Vec::new();
    let mut leaves = Vec::new();
    collect_leaves(&p, &mut leaves);
    for (index, part) in leaves.iter().enumerate() {
        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"));
        if filename.is_some() || disposition.disposition == mailparse::DispositionType::Attachment {
            attachments.push(Attachment {
                name: filename.cloned().unwrap_or_else(|| "attachment".into()),
                size: part.get_body_raw().map(|b| b.len()).unwrap_or_default(),
                mime: part.ctype.mimetype.clone(),
                part: index,
            });
        } else if part.ctype.mimetype == "text/html" && html.is_none() {
            html = part.get_body().ok();
        } else if part.ctype.mimetype == "text/plain" && body.is_empty() {
            body = part.get_body().unwrap_or_default();
        }
    }
    if body.is_empty()
        && let Some(html) = &html
    {
        body = html_to_text(html);
    }
    let snippet = body
        .split_whitespace()
        .take(45)
        .collect::<Vec<_>>()
        .join(" ");
    let date = mailparse::dateparse(&header("Date")).unwrap_or_default();
    let subject = header("Subject");
    let has_attachments = mixed || !attachments.is_empty();
    Ok(Mail {
        id: id.into(),
        account: account.id.clone(),
        folder,
        remote_folder: remote.into(),
        uid,
        from,
        sender,
        to: header("To"),
        cc: header("Cc"),
        subject: if subject.is_empty() {
            "(No subject)".into()
        } else {
            subject
        },
        snippet,
        date,
        unread: true,
        starred: false,
        size: raw.len() as u32,
        body: if body.is_empty() { None } else { Some(body) },
        html,
        attachments,
        message_id: header("Message-ID"),
        references: {
            let refs = header("References");
            if refs.trim().is_empty() {
                header("In-Reply-To")
            } else {
                refs
            }
        },
        has_attachments,
        gmail_id: 0,
        pending: false,
    })
}
pub(crate) fn collect_leaves<'a>(
    p: &'a mailparse::ParsedMail<'a>,
    out: &mut Vec<&'a mailparse::ParsedMail<'a>>,
) {
    if p.subparts.is_empty() {
        out.push(p);
    } else {
        for part in &p.subparts {
            collect_leaves(part, out);
        }
    }
}
pub(crate) fn html_to_text(html: &str) -> String {
    // Search, snippets, replies and the plain-text reader use this fallback.
    let mut text = String::new();
    let mut tag = String::new();
    let mut inside = false;
    let mut hidden = 0;
    for ch in html.chars() {
        if ch == '<' {
            inside = true;
            tag.clear();
        } else if ch == '>' && inside {
            inside = false;
            let t = tag.to_lowercase();
            let t = t.trim();
            if t.starts_with("script") || t.starts_with("style") {
                hidden += 1;
            } else if t.starts_with("/script") || t.starts_with("/style") {
                hidden = 0;
            } else if hidden == 0
                && ["br", "/p", "/div", "/tr", "/h1", "/h2", "/li"]
                    .iter()
                    .any(|s| t.starts_with(s))
            {
                text.push('\n');
            }
        } else if inside {
            tag.push(ch);
        } else if hidden == 0 {
            text.push(ch);
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_html_alternative_without_treating_html_attachments_as_body() {
        let raw = b"Content-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\nContent-Type: multipart/alternative; boundary=a\r\n\r\n--a\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlain alternative\r\n--a\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nPGI+UmljaCBhbHRlcm5hdGl2ZTwvYj4=\r\n--a--\r\n--m\r\nContent-Type: text/html; name=attachment.html\r\nContent-Disposition: attachment; filename=attachment.html\r\n\r\n<script>attachment</script>\r\n--m--\r\n";
        let a = Account::new(Provider::Imap);
        let m = parse_message(raw, &a, "id", Folder::Inbox, "INBOX", 1).unwrap();
        assert_eq!(m.body.as_deref(), Some("Plain alternative"));
        assert_eq!(m.html.as_deref(), Some("<b>Rich alternative</b>"));
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(m.attachments[0].part, 2);
        assert!(!m.snippet.contains("<b>"));
        let html_only = parse_message(
            b"Content-Type: text/html; charset=utf-8\r\n\r\n<h1>HTML only</h1>",
            &a,
            "id",
            Folder::Inbox,
            "INBOX",
            1,
        )
        .unwrap();
        assert!(html_only.html.is_some());
        assert!(html_only.body.unwrap().contains("HTML only"));
        let mut old = serde_json::to_value(&m).unwrap();
        old.as_object_mut().unwrap().remove("html");
        assert!(serde_json::from_value::<Mail>(old).unwrap().html.is_none());
    }
}

/// What a reader shows under "details": addressing, the sending domains and
/// how the message was protected in transit and by its sender.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub from: String,
    pub reply_to: String,
    pub to: String,
    pub cc: String,
    pub date: String,
    pub subject: String,
    pub message_id: String,
    /// The envelope sender's domain (SPF's `smtp.mailfrom`, else Return-Path).
    pub mailed_by: Option<String>,
    /// Domains whose DKIM signatures cover the message.
    pub signed_by: Vec<String>,
    pub spf: Option<String>,
    pub dkim: Option<String>,
    pub dmarc: Option<String>,
    pub security: Security,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Security {
    /// The message itself is encrypted (OpenPGP or S/MIME).
    EndToEnd,
    /// The last hop into the mailbox used TLS.
    Tls,
    /// The last hop is recorded and did not use TLS.
    Plain,
    #[default]
    Unknown,
}
impl Security {
    pub fn label(self) -> &'static str {
        match self {
            Security::EndToEnd => "End-to-end encryption",
            Security::Tls => "Standard encryption (TLS)",
            Security::Plain => "Not encrypted in transit",
            Security::Unknown => "Not recorded",
        }
    }
}

fn domain_of(address: &str) -> Option<String> {
    let address = address
        .trim()
        .trim_matches(|c| c == '<' || c == '>' || c == '"');
    let domain = address.rsplit_once('@').map_or(address, |(_, d)| d);
    let domain = domain
        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_lowercase();
    (!domain.is_empty() && domain.contains('.')).then_some(domain)
}
/// The value after `key=` in an Authentication-Results or DKIM header.
fn property<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(key) {
        let start = from + i;
        let boundary = start == 0
            || matches!(
                lower.as_bytes()[start - 1],
                b' ' | b'\t' | b'\r' | b'\n' | b';'
            );
        let after = start + key.len();
        if boundary && lower[after..].trim_start().starts_with('=') {
            let rest = lower[after..].trim_start()[1..].trim_start();
            let offset = text.len() - rest.len();
            let end = rest
                .find(|c: char| c.is_whitespace() || c == ';' || c == ')' || c == '(')
                .unwrap_or(rest.len());
            return Some(&text[offset..offset + end]);
        }
        from = start + key.len();
    }
    None
}

pub fn details(raw: &[u8]) -> Details {
    let Ok((headers, _)) = mailparse::parse_headers(raw) else {
        return Details::default();
    };
    let one = |name: &str| headers.get_first_value(name).unwrap_or_default();
    let all = |name: &str| headers.get_all_values(name);
    // The topmost results were added by the receiving server; later ones may
    // be forged by the sender.
    let results = all("Authentication-Results")
        .into_iter()
        .chain(all("ARC-Authentication-Results"))
        .next()
        .unwrap_or_default();
    let verdict = |key: &str| property(&results, key).map(|v| v.to_ascii_lowercase());
    let mut signed_by: Vec<String> = Vec::new();
    for signature in all("DKIM-Signature") {
        if let Some(d) = property(&signature, "d").and_then(domain_of)
            && !signed_by.contains(&d)
        {
            signed_by.push(d);
        }
    }
    // When the receiver verified DKIM, show the domain it vouched for first.
    if verdict("dkim").as_deref() == Some("pass")
        && let Some(d) = property(&results, "header.d")
            .or_else(|| property(&results, "header.i"))
            .and_then(domain_of)
    {
        signed_by.retain(|s| s != &d);
        signed_by.insert(0, d);
    }
    let mailed_by = property(&results, "smtp.mailfrom")
        .and_then(domain_of)
        .or_else(|| domain_of(&one("Return-Path")));
    let content_type = one("Content-Type").to_ascii_lowercase();
    let security = if content_type.contains("multipart/encrypted")
        || content_type.contains("application/pkcs7-mime")
        || crate::manage::crypto::encrypted_part(raw).is_some()
    {
        Security::EndToEnd
    } else if let Some(hop) = all("Received").into_iter().next() {
        let hop = hop.to_ascii_lowercase();
        if hop.contains("esmtps")
            || hop.contains("esmtpsa")
            || hop.contains("tls")
            || hop.contains("cipher=")
            || hop.contains("https")
        {
            Security::Tls
        } else {
            Security::Plain
        }
    } else {
        Security::Unknown
    };
    Details {
        from: one("From"),
        reply_to: one("Reply-To"),
        to: one("To"),
        cc: one("Cc"),
        date: one("Date"),
        subject: one("Subject"),
        message_id: one("Message-ID"),
        mailed_by,
        signed_by,
        spf: verdict("spf"),
        dkim: verdict("dkim"),
        dmarc: verdict("dmarc"),
        security,
    }
}

#[cfg(test)]
mod details_tests {
    use super::*;
    #[test]
    fn reads_sending_domains_verdicts_and_transport_security() {
        let raw = b"Received: from mail.example.com (mail.example.com [192.0.2.1])\r\n\tby mx.local with ESMTPS id abc\r\n\t(version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384);\r\n\tMon, 1 Jan 2024 00:00:00 +0000\r\nAuthentication-Results: mx.local;\r\n\tdkim=pass header.i=@news.example.com header.s=s1;\r\n\tspf=pass (sender ok) smtp.mailfrom=bounce@bounces.example.com;\r\n\tdmarc=pass (p=REJECT) header.from=example.com\r\nDKIM-Signature: v=1; a=rsa-sha256; d=esp.example.net; s=k;\r\nDKIM-Signature: v=1; a=rsa-sha256; d=news.example.com; s=s1;\r\nReturn-Path: <other@elsewhere.org>\r\nFrom: News <hi@example.com>\r\nSubject: Hello\r\n\r\nBody";
        let d = details(raw);
        assert_eq!(d.mailed_by.as_deref(), Some("bounces.example.com"));
        assert_eq!(d.signed_by, vec!["news.example.com", "esp.example.net"]);
        assert_eq!(d.spf.as_deref(), Some("pass"));
        assert_eq!(d.dkim.as_deref(), Some("pass"));
        assert_eq!(d.dmarc.as_deref(), Some("pass"));
        assert_eq!(d.security, Security::Tls);
        assert_eq!(d.subject, "Hello");
        let plain =
            details(b"Received: from a by b with SMTP; x\r\nReturn-Path: <x@plain.org>\r\n\r\n");
        assert_eq!(plain.security, Security::Plain);
        assert_eq!(plain.mailed_by.as_deref(), Some("plain.org"));
        assert_eq!(plain.spf, None);
    }
}
