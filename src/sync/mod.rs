//! Sync: provider discovery, authentication, and incoming/outgoing protocols.
//! Protocol helpers never depend on Display and never mutate the message database.
pub mod auth;
pub mod discover;
pub mod forwarding;
mod imap;
mod imap_ext;
pub mod mime;
pub mod models;
pub(crate) mod mta_sts;
pub mod oauth;
mod pop;
pub(crate) mod retry;
pub(crate) mod sasl;
pub(crate) mod sieve;
pub(crate) mod smtp;
pub(crate) mod stream;
pub(crate) mod transport;
pub(crate) mod wire;
