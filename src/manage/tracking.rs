//! Removes tracking from links in mail with url-sanitize: campaign and
//! click parameters (`utm_source`, `fbclid`, …) are stripped and known
//! redirect wrappers are unwrapped, by the ClearURLs, Brave and Firefox
//! rules url-sanitize merges (`assets/url-sanitize`, with their licenses).
//!
//! Only what can be removed without contacting anyone is removed. Opaque
//! click-tracking addresses (a newsletter's `click.example.com/abc123`)
//! hold no readable destination and are left as they are.
use std::borrow::Cow;
use std::sync::OnceLock;
use url_sanitize_core::{Catalog, SanitizeResult, Sanitizer, SanitizerOptions, SanitizerRule};

/// url-sanitize's merged rule catalog, as its `url-sanitize` 2.4.1 release
/// embeds it. Kept unchanged, so it can be replaced by a newer one.
const CATALOG_JSON: &str = include_str!("../../assets/url-sanitize/catalog.json");
/// Picacho's own rules, in the catalog's rule format, for trackers in mail
/// that the catalog doesn't know.
const OWN_RULES_JSON: &str = include_str!("../../assets/url-sanitize/picacho-rules.json");

/// The compiled rules, built on first use.
fn sanitizer() -> &'static Sanitizer {
    static SANITIZER: OnceLock<Sanitizer> = OnceLock::new();
    SANITIZER.get_or_init(|| {
        let mut catalog =
            Catalog::from_json(CATALOG_JSON).expect("the embedded url-sanitize catalog parses");
        let own: Vec<SanitizerRule> =
            serde_json::from_str(OWN_RULES_JSON).expect("Picacho's link rules parse");
        catalog.rules.extend(own);
        // Referral and affiliate codes stay; links aren't blocked.
        catalog.compile(SanitizerOptions::default())
    })
}

/// Compiles the rules in the background, so the first email shown with
/// links doesn't wait for them.
pub fn warm_up() {
    std::thread::spawn(|| {
        sanitizer();
    });
}

/// `url` without tracking, or `None` when there was none to remove (or it
/// is not a web address).
pub fn clean_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    match sanitizer().sanitize(trimmed) {
        SanitizeResult::Cleaned { url, .. } | SanitizeResult::Redirected { url, .. } => {
            (url != trimmed).then_some(url)
        }
        SanitizeResult::Unchanged { .. } | SanitizeResult::Blocked { .. } => None,
    }
}

/// Plain text with tracking removed from the web addresses in it.
pub fn clean_text(text: &str) -> Cow<'_, str> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("http://") && !lower.contains("https://") {
        return Cow::Borrowed(text);
    }
    let cleaned = sanitizer().sanitize_text(text).text;
    if cleaned == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(cleaned)
    }
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
            clean_url(
                "https://shop.example/item?id=7&utm_source=news&UTM_Medium=email&fbclid=x#top"
            )
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
        assert_eq!(
            clean_url("https://example.com/?ref=abc"),
            None,
            "not tracking alone"
        );
        assert_eq!(clean_url("mailto:ana@example.com?utm_source=x"), None);
    }

    #[test]
    fn unwraps_redirects_that_name_their_destination() {
        assert_eq!(
            clean_url(
                "https://www.google.com/url?q=https://example.com/a%3Futm_source%3Dx%26id%3D2&sa=D"
            )
            .as_deref(),
            Some("https://example.com/a?id=2")
        );
        assert_eq!(
            clean_url("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.net%2F&h=AT0")
                .as_deref(),
            Some("https://example.net/")
        );
        // A wrapped non-web address is left alone.
        assert_eq!(
            clean_url("https://www.google.com/url?q=javascript:alert(1)"),
            None
        );
        // An unknown click tracker can't be unwrapped.
        assert_eq!(
            clean_url("https://click.news.example/ls/click?upn=abc"),
            None
        );
    }

    #[test]
    fn email_click_trackers_open_the_page_they_lead_to() {
        // Best Buy's mail: a MessageGears click address holding a Best Buy
        // redirect in base64, which holds the page with campaign codes.
        let link = "https://trk.email.bestbuy.com/c/m/?T=2RLTeyJhaSI6OTQwMTc0ODEsImUiOiJtZy1lbmM6UGNWUWNrMGhiRDNmb2VOdmgvS0o1ZW5FdVFMY3ZjMVg3SGN4VlNvbnY2RFVsdXNTWkQ5ZmdXSDNFQnRXdGF3ZyIsInJpIjoiNDA5ODc1NDA1IiwicnEiOiIwMi1iMjYzMTctNmEyOTNmYTEyYzU1NDU0NmJhNTEyZmFmMzNmMGNmNTgiLCJtIjpmYWxzZSwidWkiOiI1MjAyMDgiLCJ1biI6IjRYJTNBZ2xvYmFsXzU4MTMxNTJfRlkyN1NhbGVRdWFkTW9kcy1VcGRhdGVVUkxzX0NlbGxQaG9uZXNfUU1fMDUxOTIwMjYlM0E1In0%3A%3AuqkeD-qoqyV027kx3JyC0A&mg_x_url=aHR0cHM6Ly93d3cuYmVzdGJ1eS5jb20vZWFzZS9lbWFpbD9ldXQ9MzUyMDE0NjM2MiZkZXN0PWh0dHBzJTNBJTJGJTJGd3d3LmJlc3RidXkuY29tJTJGc2l0ZSUyRmFsbC1lbGVjdHJvbmljcy1vbi1zYWxlJTJGYWxsLWNlbGwtcGhvbmVzLW9uLXNhbGUlMkZwY21jYXQxNjkwODk1ODYxNTUyLmMlM0ZpZCUzRHBjbWNhdDE2OTA4OTU4NjE1NTIlMjZzcmMlM0RNRyUyNnZlaGljbGUlM0RlbWFpbCUyNmpvYmlkJTNENzE4NTQlMjZjb21tY29kZSUzREJCWV8yMDI2MTExM19GcmlQTV9FQkZEJTI2ZXV0JTNEMzUyMDE0NjM2MiUyNnJlZiUzREU1MEMyMlIwMDclMjZsb2MlM0RCT0RZ&m=false";
        assert_eq!(
            clean_url(link).as_deref(),
            Some(
                "https://www.bestbuy.com/site/all-electronics-on-sale/all-cell-phones-on-sale/pcmcat1690895861552.c?id=pcmcat1690895861552"
            )
        );
        // Its product links keep what picks the product.
        assert_eq!(
            clean_url("https://www.bestbuy.com/site/x/6.p?skuId=6&src=MG&ref=E50").as_deref(),
            Some("https://www.bestbuy.com/site/x/6.p?skuId=6")
        );
    }

    #[test]
    fn cleans_links_in_html_and_text() {
        let html = r#"<p><a class=x href="https://a.example/?id=1&amp;utm_source=n">A</a>
            <a href='https://b.example/x?fbclid=1'>B</a> <a href=https://c.example/?gclid=2>C</a>
            <img src="https://a.example/p.png?utm_source=n"> data-href="x"</p>"#;
        let clean = clean_html_links(html);
        assert!(
            clean.contains(r#"href="https://a.example/?id=1""#),
            "{clean}"
        );
        assert!(clean.contains("href='https://b.example/x'"), "{clean}");
        assert!(clean.contains("href=https://c.example/>"), "{clean}");
        // Only links; images and other attributes are untouched.
        assert!(clean.contains("p.png?utm_source=n"));
        assert!(matches!(
            clean_html_links("<a href=\"https://x.example/\">x</a>"),
            Cow::Borrowed(_)
        ));

        let text =
            "See https://a.example/?utm_source=n. Or (https://b.example/p?id=1&fbclid=2), thanks";
        assert_eq!(
            clean_text(text),
            "See https://a.example/. Or (https://b.example/p?id=1), thanks"
        );
        assert!(matches!(clean_text("no links here"), Cow::Borrowed(_)));
    }
}
