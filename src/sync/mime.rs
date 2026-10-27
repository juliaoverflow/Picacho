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
        has_attachments,
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
