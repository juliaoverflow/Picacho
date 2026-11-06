//! Sync: provider discovery, authentication, and incoming/outgoing protocols.
//! Protocol helpers never depend on Display and never mutate the message database.
pub mod discover;
mod imap;
pub mod mime;
pub mod oauth;
mod pop;
pub(crate) mod retry;
pub(crate) mod smtp;
pub(crate) mod transport;
pub(crate) mod wire;
