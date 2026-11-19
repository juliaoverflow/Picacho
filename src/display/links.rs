//! Opening links from email.
//!
//! On a phone, a link tapped in an email opens only after a warning
//! (`mobile::link_sheet`): links are how fake sign-in pages are reached, and
//! a phone shows no address before the tap. Sites can be trusted from the
//! warning, so their links open at once from then on; the warning can be
//! turned off, and trusted sites removed, in Settings → Reading (phones).
//!
//! On a desktop links open at once; hovering one shows its address at the
//! bottom left of the mail view (`link_status`), as a browser does.
use super::*;

const WARN_PREFERENCE: &str = "link_warnings";
const TRUSTED_PREFERENCE: &str = "trusted_link_domains";

/// A link waiting for the person's answer to the warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinkConfirm {
    pub(super) url: String,
    /// The site it goes to, as it would be trusted (see `site`).
    pub(super) site: String,
    /// Keys the sheet's entrance.
    pub(super) key: u64,
}

/// What the warning's buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkChoice {
    Open,
    /// Trust the site, then open.
    TrustAndOpen,
    Copy,
    Cancel,
}

pub(super) struct LinkSafety {
    /// Phones warn before opening a link (Settings).
    pub(super) warn: bool,
    /// Sites whose links open without the warning, sorted.
    pub(super) trusted: Vec<String>,
    pub(super) confirm: Option<LinkConfirm>,
    /// The link under the pointer in the reader's email (desktop).
    pub(super) hovered: Option<String>,
    shown: u64,
}

impl LinkSafety {
    pub(super) fn load(store: &Store) -> Self {
        let trusted = store
            .preference(TRUSTED_PREFERENCE)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            warn: store.preference(WARN_PREFERENCE).as_deref() != Some("false"),
            trusted,
            confirm: None,
            hovered: None,
            shown: 0,
        }
    }

    /// Whether `url` goes to a trusted site or one under it. Under a
    /// public suffix (a site trusted before it was checked for one), only
    /// the site itself and its `www.` are trusted: the names under it
    /// belong to other people.
    fn trusts(&self, url: &url::Url) -> bool {
        let Some(host) = url.host_str().map(|h| h.trim_end_matches('.').to_lowercase()) else {
            return false;
        };
        self.trusted.iter().any(|site| {
            host == *site
                || host.strip_prefix("www.") == Some(site.as_str())
                || (!public_suffix(site) && host.ends_with(&format!(".{site}")))
        })
    }
}

/// The site a link goes to, as it is shown and trusted: its host, without
/// a leading `www.` unless what remains is a public suffix
/// (`www.blogspot.com` stays itself, or trusting it would trust every blog).
pub(super) fn site(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.trim_end_matches('.').to_lowercase();
    Some(match host.strip_prefix("www.") {
        Some(rest) if !public_suffix(rest) => rest.to_owned(),
        _ => host,
    })
}

/// Whether `name` is a public suffix (`com`, `co.uk`, `blogspot.com`):
/// a name under which different people register their own.
fn public_suffix(name: &str) -> bool {
    psl::suffix_str(name) == Some(name)
}

/// What about `url` is a known trick, in a few words each.
pub(super) fn link_dangers(url: &url::Url) -> Vec<&'static str> {
    let mut dangers = Vec::new();
    if url.scheme() == "http" {
        dangers.push("Not secure: the page isn't encrypted (http).");
    }
    match url.host() {
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => {
            dangers.push("It goes to a bare IP address, not a named site.");
        }
        Some(url::Host::Domain(host)) => {
            if host.split('.').any(|label| label.starts_with("xn--")) {
                dangers.push("Its address uses look-alike letters from other alphabets.");
            }
            if host.split('.').count() > 4 {
                dangers.push("Its address is unusually long; the real site is at its end.");
            }
        }
        None => {}
    }
    if !url.username().is_empty() || url.password().is_some() {
        dangers.push("Its address hides the real site after an @.");
    }
    dangers
}

impl MailApp {
    /// Opens a link from an email (clicked, or chosen from its menu): at
    /// once, or after the warning on a phone.
    pub(super) fn open_email_link(&mut self, url: String, cx: &mut Context<Self>) {
        self.menu = None;
        let url = if self.clean_links {
            crate::manage::tracking::clean_url(&url).unwrap_or(url)
        } else {
            url
        };
        let Some(parsed) = crate::display::html::openable_url(&url) else {
            self.notice("Picacho only opens web and email links.", true, cx);
            return;
        };
        let web = parsed.scheme() != "mailto";
        if web
            && self.mobile.on
            && self.links.warn
            && !self.links.trusts(&parsed)
            && let Some(site) = site(&parsed)
        {
            self.links.shown += 1;
            self.links.confirm = Some(LinkConfirm {
                url: parsed.to_string(),
                site,
                key: self.links.shown,
            });
            cx.notify();
            return;
        }
        self.open_external(parsed.as_str(), cx);
    }

    /// Hands `url` to the system: the browser, or the mail app.
    pub(super) fn open_external(&mut self, url: &str, cx: &mut Context<Self>) {
        // GPUI's `open_url` does nothing on Android.
        #[cfg(target_os = "android")]
        if !crate::android::open_url(url) {
            self.notice("No app on this phone can open this link.", true, cx);
        }
        #[cfg(not(target_os = "android"))]
        cx.open_url(url);
    }

    /// The warning's answer.
    pub(super) fn choose_link(&mut self, choice: LinkChoice, cx: &mut Context<Self>) {
        let Some(confirm) = self.links.confirm.take() else {
            return;
        };
        match choice {
            LinkChoice::Open => self.open_external(&confirm.url, cx),
            LinkChoice::TrustAndOpen => {
                if !self.links.trusted.contains(&confirm.site) {
                    self.links.trusted.push(confirm.site.clone());
                    self.links.trusted.sort();
                    self.save_trusted();
                }
                self.notice(
                    format!("Links to {} open without asking", confirm.site),
                    false,
                    cx,
                );
                self.open_external(&confirm.url, cx);
            }
            LinkChoice::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(confirm.url));
                self.notice("Link copied", false, cx);
            }
            LinkChoice::Cancel => {}
        }
        cx.notify();
    }

    pub(super) fn set_link_warnings(&mut self, on: bool, cx: &mut Context<Self>) {
        self.links.warn = on;
        self.backend
            .store
            .set_preference_soon(WARN_PREFERENCE, if on { "true" } else { "false" });
        cx.notify();
    }

    pub(super) fn untrust_site(&mut self, site: &str, cx: &mut Context<Self>) {
        self.links.trusted.retain(|s| s != site);
        self.save_trusted();
        cx.notify();
    }

    fn save_trusted(&self) {
        if let Ok(json) = serde_json::to_string(&self.links.trusted) {
            self.backend
                .store
                .set_preference_soon(TRUSTED_PREFERENCE, &json);
        }
    }

    /// The address of the link under the pointer, at the bottom left of
    /// the mail view, as a browser shows it (desktop).
    pub(super) fn link_status(&self) -> Option<AnyElement> {
        if self.mobile.on {
            return None;
        }
        let url = self.links.hovered.as_deref()?;
        Some(link_status_pill(url, self.colors()))
    }
}

/// A link's address over the bottom left of what holds it.
pub(super) fn link_status_pill(url: &str, c: Colors) -> AnyElement {
    let shown = url.strip_prefix("mailto:").unwrap_or(url).to_owned();
    div()
        .id("link-status")
        .debug_selector(|| "link-status".into())
        .absolute()
        .left(px(6.))
        .bottom(px(6.))
        .max_w(relative(0.7))
        .px_2()
        .py(px(3.))
        .rounded(px(6.))
        .bg(rgb(c.raised))
        .border_1()
        .border_color(rgb(c.border))
        .shadow_md()
        .text_size(px(12.))
        .text_color(rgb(c.muted))
        .truncate()
        .child(shown)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn sites_are_hosts_without_www() {
        assert_eq!(
            site(&url("https://www.Example.com/a")).as_deref(),
            Some("example.com")
        );
        assert_eq!(
            site(&url("https://mail.example.com")).as_deref(),
            Some("mail.example.com")
        );
    }

    #[test]
    fn trusting_a_site_trusts_the_sites_under_it() {
        let links = LinkSafety {
            warn: true,
            trusted: vec!["example.com".into()],
            confirm: None,
            hovered: None,
            shown: 0,
        };
        assert!(links.trusts(&url("https://example.com/x")));
        assert!(links.trusts(&url("https://login.example.com/x")));
        assert!(!links.trusts(&url("https://example.com.evil.net/x")));
        assert!(!links.trusts(&url("https://notexample.com/x")));
    }

    #[test]
    fn shared_suffixes_trust_only_themselves() {
        assert_eq!(site(&url("https://www.example.com/")).unwrap(), "example.com");
        assert_eq!(
            site(&url("https://www.blogspot.com/")).unwrap(),
            "www.blogspot.com"
        );
        let links = LinkSafety {
            warn: true,
            trusted: vec!["blogspot.com".into(), "www.github.io".into()],
            confirm: None,
            hovered: None,
            shown: 0,
        };
        assert!(links.trusts(&url("https://blogspot.com/")));
        assert!(links.trusts(&url("https://www.blogspot.com/")));
        assert!(!links.trusts(&url("https://someone.blogspot.com/")));
        assert!(links.trusts(&url("https://www.github.io/")));
        assert!(!links.trusts(&url("https://attacker.github.io/")));
    }

    #[test]
    fn known_tricks_are_named() {
        assert!(link_dangers(&url("https://example.com/")).is_empty());
        assert_eq!(link_dangers(&url("http://example.com/")).len(), 1);
        assert_eq!(link_dangers(&url("https://192.168.1.1/")).len(), 1);
        assert_eq!(link_dangers(&url("https://xn--pple-43d.com/")).len(), 1);
        assert_eq!(link_dangers(&url("https://bank.com@evil.net/")).len(), 1);
    }
}
