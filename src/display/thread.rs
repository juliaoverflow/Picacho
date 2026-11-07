//! Reading conversations: which messages belong together, where the quoted
//! history in a reply begins, and short recipient lists.

/// A subject without reply and forward prefixes ("Re:", "Fwd:", "AW:"…),
/// any number of them, compared case-insensitively.
pub(super) fn conversation_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let prefix = [
            "re:", "fwd:", "fw:", "aw:", "wg:", "sv:", "vs:", "tr:", "rv:", "réf:",
        ]
        .iter()
        .find(|p| lower.starts_with(*p));
        // "Re[2]:" and "Re (2):" as some clients write them.
        let numbered = lower.starts_with("re[") || lower.starts_with("re (");
        match (prefix, numbered) {
            (Some(p), _) => s = s[p.len()..].trim_start(),
            (None, true) => match s.find(':') {
                Some(i) if i < 8 => s = s[i + 1..].trim_start(),
                _ => break,
            },
            _ => break,
        }
    }
    s.to_lowercase()
}

/// Splits a plain-text reply into what was written now and the quoted
/// history below it, when there is one worth hiding.
pub(super) fn split_quoted(body: &str) -> (&str, Option<&str>) {
    let mut offset = 0;
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let attribution = (t.starts_with("On ") && t.ends_with("wrote:"))
            || (t.starts_with("Le ") && t.ends_with("a écrit :"))
            || (t.starts_with("Am ") && t.ends_with("schrieb:"))
            || t.starts_with("-----Original Message-----")
            || t.starts_with("---------- Forwarded message")
            || t.starts_with("________________________________")
            // Outlook: "From: …" followed by "Sent:" within two lines.
            || (t.starts_with("From:")
                && lines[i + 1..]
                    .iter()
                    .take(2)
                    .any(|l| l.trim_start().starts_with("Sent:")));
        // A run of quoted lines that continues to the end (ignoring a
        // trailing signature) also counts.
        let quoted_tail = t.starts_with('>')
            && lines[i..]
                .iter()
                .filter(|l| !l.trim().is_empty())
                .take_while(|l| !l.trim_start().starts_with("-- "))
                .all(|l| l.trim_start().starts_with('>'));
        if (attribution || quoted_tail) && i > 0 {
            let written = body[..offset].trim_end();
            if written.is_empty() {
                return (body, None);
            }
            return (written, Some(&body[offset..]));
        }
        offset += line.len();
    }
    (body, None)
}

/// CSS that hides the quoted history of an HTML reply, for the common ways
/// mail programs mark it. `None` when the email has none.
pub(super) fn hide_html_quotes(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let markers = [
        "gmail_quote",
        "type=\"cite\"",
        "type=cite",
        "moz-cite-prefix",
        "divrplyfwdmsg",
        "appendonsend",
        "yahoo_quoted",
        "protonmail_quote",
    ];
    if !markers.iter().any(|m| lower.contains(m)) {
        return None;
    }
    let style = "<style>\
        .gmail_quote,.gmail_extra,blockquote[type=cite],.moz-cite-prefix,\
        #divRplyFwdMsg,#divRplyFwdMsg~*,#appendonsend~*,.yahoo_quoted,\
        .protonmail_quote{display:none!important}\
        </style>";
    // Before </head> if there is one, else first: either way it applies.
    Some(match lower.find("</head>") {
        Some(i) => format!("{}{style}{}", &html[..i], &html[i..]),
        None => format!("{style}{html}"),
    })
}

/// Short names for a recipient list: display names (first names when
/// several), "me" for the reader's own address.
pub(super) fn short_names(list: &str, own: &[String]) -> Vec<String> {
    let parsed = crate::sync::smtp::recipients(list).unwrap_or_default();
    let several = parsed.len() > 1;
    parsed
        .into_iter()
        .map(|(name, email)| {
            if own.iter().any(|o| o.eq_ignore_ascii_case(&email)) {
                return "me".to_string();
            }
            match name.filter(|n| !n.trim().is_empty()) {
                Some(n) if several => n.split_whitespace().next().unwrap_or(&n).to_string(),
                Some(n) => n,
                None => email.split('@').next().unwrap_or(&email).to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn conversation_subjects_ignore_prefixes() {
        assert_eq!(conversation_subject("Re: RE: Fwd: Plans"), "plans");
        assert_eq!(conversation_subject("AW: Plans"), "plans");
        assert_eq!(conversation_subject("Re[2]: Plans"), "plans");
        assert_eq!(conversation_subject("Reading list"), "reading list");
    }

    #[test]
    fn quoted_history_is_split_from_the_reply() {
        let (written, quoted) =
            split_quoted("Sounds good!\n\nOn Mon, Ana wrote:\n> Lunch?\n> Bo\n");
        assert_eq!(written, "Sounds good!");
        assert!(quoted.unwrap().starts_with("On Mon"));
        let (written, quoted) = split_quoted("Yes\n> Lunch?\n>\n> Ana\n-- \nBo");
        assert_eq!(written, "Yes");
        assert!(quoted.is_some());
        // Inline answers between quotes are not hidden.
        let (_, quoted) = split_quoted("> Lunch?\nYes\n> Where?\nThe usual\n");
        assert!(quoted.is_none());
        let (_, quoted) = split_quoted("Hi\nFrom: Ana\nSent: Monday\nTo: Bo\n\nOld");
        assert!(quoted.is_some());
    }

    #[test]
    fn html_quotes_are_hidden_only_when_present() {
        assert!(hide_html_quotes("<p>Hi</p>").is_none());
        let html = hide_html_quotes("<html><head></head><body><div class=\"gmail_quote\">x</div>")
            .unwrap();
        assert!(html.contains("<style>") && html.find("<style>") < html.find("</head>"));
    }

    #[test]
    fn recipient_names_are_short() {
        let own = vec!["me@example.com".to_string()];
        assert_eq!(
            short_names("Ana Diaz <ana@x.com>, me@example.com, bo@x.com", &own),
            ["Ana", "me", "bo"]
        );
        assert_eq!(short_names("Ana Diaz <ana@x.com>", &own), ["Ana Diaz"]);
    }
}
