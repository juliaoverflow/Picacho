//! Reusable HTML renderer. No dependency on MailApp, accounts, storage or sync.
//! Callers supply HTML and a remote-content policy; this component owns its lifecycle.
mod document;
mod resources;
#[cfg(test)]
mod tests;
mod view;
mod worker;

pub use view::{HtmlContextMenu, HtmlView, thumbnail};

/// An address from an email that may be handed to the system to open: a web
/// page, or an email link. Anything else (`file:`, `smb:`, UNC paths, an
/// app's own scheme) could open or run something on this computer or the
/// network, so it is refused.
pub fn openable_url(url: &str) -> Option<url::Url> {
    let url = url::Url::parse(url.trim()).ok()?;
    (resources::remote_url(&url) || url.scheme() == "mailto").then_some(url)
}
