//! Removes tracking from links in mail: campaign and click-id parameters
//! (`utm_source`, `fbclid`, …), and redirect wrappers whose real address is
//! in the link itself (Google's `/url?q=`, Outlook Safe Links, …).
//!
//! Only what can be removed without contacting anyone is removed. Opaque
//! click-tracking addresses (a newsletter's `click.example.com/abc123`)
//! hold no readable destination and are left as they are.
use std::borrow::Cow;

/// Query parameters that only identify a campaign, a click or a reader.
/// Matched without regard to case.
const TRACKING_PARAMS: &[&str] = &[
    // Ad and social click ids.
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "igshid",
    "epik",
    "srsltid",
    // Google Analytics cross-domain ids.
    "_ga",
    "_gl",
    // Mailchimp.
    "mc_cid",
    "mc_eid",
    // HubSpot.
    "_hsenc",
    "_hsmi",
    "__hssc",
    "__hstc",
    "__hsfp",
    "hsctatracking",
    // Marketo.
    "mkt_tok",
    // Klaviyo.
    "_kx",
    // Vero.
    "vero_id",
    "vero_conv",
    // Omeda.
    "oly_anon_id",
    "oly_enc_id",
    // Others common in newsletters.
    "ck_subscriber_id",
    "rb_clickid",
    "s_cid",
    "sc_cid",
    "wickedid",
    "ml_subscriber",
    "ml_subscriber_hash",
];

/// Prefixes of parameter names that are always tracking.
const TRACKING_PREFIXES: &[&str] = &["utm_", "pk_", "mtm_", "piwik_", "hmb_"];

/// Redirectors that carry their destination in a parameter:
/// (host, or a suffix of it starting with "."; path; parameters to look in).
const REDIRECTS: &[(&str, &str, &[&str])] = &[
    ("google.com", "/url", &["q", "url"]),
    ("www.google.com", "/url", &["q", "url"]),
    ("l.facebook.com", "/l.php", &["u"]),
    ("lm.facebook.com", "/l.php", &["u"]),
    ("l.messenger.com", "/l.php", &["u"]),
    ("l.instagram.com", "/", &["u"]),
    ("www.youtube.com", "/redirect", &["q"]),
    ("away.vk.com", "/away.php", &["to"]),
    ("slack-redir.net", "/link", &["url"]),
    ("www.linkedin.com", "/redir/redirect", &["url"]),
    (".safelinks.protection.outlook.com", "/", &["url"]),
    ("statics.teams.cdn.office.net", "/evergreen-assets/safelinks/1/atp-safelinks.html", &["url"]),
];

/// Redirects unwrapped one inside another, at most.
const MAX_UNWRAP: usize = 4;

/// `url` without tracking, or `None` when there was none to remove (or it
/// is not a web address).
pub fn clean_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let mut current = trimmed.to_owned();
    for _ in 0..MAX_UNWRAP {
        match unwrap_redirect(&current) {
            Some(inner) => current = inner,
            None => break,
        }
    }
    let current = strip_params(&current).unwrap_or(current);
    (current != trimmed).then_some(current)
}

/// The destination a known redirector points to.
fn unwrap_redirect(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path();
    let (_, _, keys) = REDIRECTS.iter().find(|(h, p, _)| {
        let host_matches = match h.strip_prefix('.') {
            Some(suffix) => host.ends_with(&format!(".{suffix}")),
            None => host == *h,
        };
        host_matches && (*p == "/" || path.eq_ignore_ascii_case(p))
    })?;
    let target = keys.iter().find_map(|key| {
        parsed
            .query_pairs()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.into_owned())
    })?;
    let target = target.trim();
    let lower = target.to_ascii_lowercase();
    // Only a web address; anything else stays wrapped.
    ((lower.starts_with("http://") || lower.starts_with("https://"))
        && url::Url::parse(target).is_ok())
    .then(|| target.to_owned())
}

/// Removes tracking parameters, leaving the rest of the address exactly as
/// it was written. `None` when nothing was removed.
fn strip_params(url: &str) -> Option<String> {
    let (before_fragment, fragment) = match url.find('#') {
        Some(i) => (&url[..i], Some(&url[i..])),
        None => (url, None),
    };
    let (base, query) = before_fragment.split_once('?')?;
    let pairs: Vec<&str> = query.split('&').collect();
    let kept: Vec<&str> = pairs
        .iter()
        .copied()
        .filter(|pair| !pair.is_empty() && !is_tracking(pair))
        .collect();
    if kept.len() == pairs.iter().filter(|p| !p.is_empty()).count() {
        return None;
    }
    let mut out = base.to_owned();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        out.push_str(fragment);
    }
    Some(out)
}

fn is_tracking(pair: &str) -> bool {
    let raw = pair.split('=').next().unwrap_or(pair);
    let key = percent_decode(raw).to_ascii_lowercase();
    TRACKING_PARAMS.contains(&key.as_str())
        || TRACKING_PREFIXES.iter().any(|p| key.starts_with(p))
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("{s}=").as_bytes())
        .next()
        .map(|(k, _)| k.into_owned())
        .unwrap_or_else(|| s.to_owned())
}

/// Plain text with tracking removed from the web addresses in it.
pub fn clean_text(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut last = 0;
    let mut changed = false;
    let lower = text.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = next_url_start(&lower, search) {
        let end = url_end(text, found);
        let candidate = &text[found..end];
        if let Some(clean) = clean_url(candidate) {
            out.push_str(&text[last..found]);
            out.push_str(&clean);
            last = end;
            changed = true;
        }
        search = end.max(found + 1);
    }
    if !changed {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
}

fn next_url_start(lower: &str, from: usize) -> Option<usize> {
    let rest = lower.get(from..)?;
    let http = rest.find("http://");
    let https = rest.find("https://");
    let first = match (http, https) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => a.or(b)?,
    };
    Some(from + first)
}

/// Where an address that starts at `start` ends: at whitespace or a
/// character that can't be in it, without trailing punctuation.
fn url_end(text: &str, start: usize) -> usize {
    let mut end = start;
    for (i, ch) in text[start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']') {
            break;
        }
        end = start + i + ch.len_utf8();
    }
    while end > start
        && text[..end].ends_with(['.', ',', ';', ':', '!', '?', '*'])
    {
        end -= 1;
    }
    end
}

/// HTML with tracking removed from its links' `href`s. The rest of the
/// markup is untouched.
pub fn clean_html_links(html: &str) -> Cow<'_, str> {
    let bytes = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut last = 0;
    let mut changed = false;
    let mut search = 0;
    while let Some(offset) = lower[search..].find("href") {
        let at = search + offset;
        search = at + 4;
        // An attribute name: after whitespace, followed by "=".
        if at == 0 || !bytes[at - 1].is_ascii_whitespace() {
            continue;
        }
        let mut i = at + 4;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let (start, end) = match bytes[i] {
            quote @ (b'"' | b'\'') => {
                let start = i + 1;
                match html[start..].find(quote as char) {
                    Some(len) => (start, start + len),
                    None => break,
                }
            }
            _ => {
                let start = i;
                let len = html[start..]
                    .find(|c: char| c.is_ascii_whitespace() || c == '>')
                    .unwrap_or(html.len() - start);
                (start, start + len)
            }
        };
        search = end;
        let raw = &html[start..end];
        let decoded = raw.replace("&amp;", "&").replace("&#38;", "&");
        if let Some(clean) = clean_url(&decoded) {
            out.push_str(&html[last..start]);
            out.push_str(
                &clean
                    .replace('&', "&amp;")
                    .replace('"', "&quot;")
                    .replace('\'', "&#39;"),
            );
            last = end;
            changed = true;
        }
    }
    if !changed {
        return Cow::Borrowed(html);
    }
    out.push_str(&html[last..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_tracking_parameters_and_keeps_the_rest() {
        assert_eq!(
            clean_url("https://shop.example/item?id=7&utm_source=news&UTM_Medium=email&fbclid=x#top")
                .as_deref(),
            Some("https://shop.example/item?id=7#top")
        );
        assert_eq!(
            clean_url("https://example.com/a?utm_campaign=x&_hsenc=y").as_deref(),
            Some("https://example.com/a")
        );
        // Other parameters stay exactly as written, encoding included.
        assert_eq!(
            clean_url("https://example.com/s?q=a%20b&mc_eid=1&page=2").as_deref(),
            Some("https://example.com/s?q=a%20b&page=2")
        );
        assert_eq!(clean_url("https://example.com/s?q=utm_source"), None);
        assert_eq!(clean_url("https://example.com/?ref=abc"), None, "not tracking alone");
        assert_eq!(clean_url("mailto:ana@example.com?utm_source=x"), None);
    }

    #[test]
    fn unwraps_redirects_that_name_their_destination() {
        assert_eq!(
            clean_url("https://www.google.com/url?q=https://example.com/a%3Futm_source%3Dx%26id%3D2&sa=D")
                .as_deref(),
            Some("https://example.com/a?id=2")
        );
        assert_eq!(
            clean_url(
                "https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fexample.org%2Fpage&data=abc&reserved=0"
            )
            .as_deref(),
            Some("https://example.org/page")
        );
        assert_eq!(
            clean_url("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.net%2F&h=AT0").as_deref(),
            Some("https://example.net/")
        );
        // A wrapped non-web address is left alone.
        assert_eq!(clean_url("https://www.google.com/url?q=javascript:alert(1)"), None);
        // An unknown click tracker can't be unwrapped.
        assert_eq!(clean_url("https://click.news.example/ls/click?upn=abc"), None);
    }

    #[test]
    fn cleans_links_in_html_and_text() {
        let html = r#"<p><a class=x href="https://a.example/?id=1&amp;utm_source=n">A</a>
            <a href='https://b.example/x?fbclid=1'>B</a> <a href=https://c.example/?gclid=2>C</a>
            <img src="https://a.example/p.png?utm_source=n"> data-href="x"</p>"#;
        let clean = clean_html_links(html);
        assert!(clean.contains(r#"href="https://a.example/?id=1""#), "{clean}");
        assert!(clean.contains("href='https://b.example/x'"), "{clean}");
        assert!(clean.contains("href=https://c.example/>"), "{clean}");
        // Only links; images and other attributes are untouched.
        assert!(clean.contains("p.png?utm_source=n"));
        assert!(matches!(clean_html_links("<a href=\"https://x.example/\">x</a>"), Cow::Borrowed(_)));

        let text = "See https://a.example/?utm_source=n. Or (https://b.example/p?id=1&fbclid=2), thanks";
        assert_eq!(
            clean_text(text),
            "See https://a.example/. Or (https://b.example/p?id=1), thanks"
        );
        assert!(matches!(clean_text("no links here"), Cow::Borrowed(_)));
    }
}
