//! Reusable HTML renderer. No dependency on MailApp, accounts, storage or sync.
//! Callers supply HTML and a remote-content policy; this component owns its lifecycle.
mod document;
mod resources;
#[cfg(test)]
mod tests;
mod view;
mod worker;

pub use view::{HtmlContextMenu, HtmlView, thumbnail};
