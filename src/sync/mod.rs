//! Sync: provider discovery, authentication, and incoming/outgoing protocols.
//! Protocol helpers never depend on Display and never mutate the message database.
pub mod discover;
mod imap;
mod mime;
pub mod oauth;
mod pop;
pub(crate) mod smtp;
pub(crate) mod transport;
