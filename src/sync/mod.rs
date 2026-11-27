//! Sync: provider discovery, authentication, and incoming/outgoing protocols.
//! Protocol helpers never depend on Display and never mutate the message database.
pub mod auth;
pub mod caldav;
/// CTAP2 over NFC, for security keys held to a phone.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) mod ctap_nfc;
pub(crate) mod dsn;
pub mod carddav;
pub mod dav;
pub mod discover;
pub(crate) mod feed;
pub(crate) mod ews;
pub mod ical;
pub mod keys;
pub(crate) mod gmail;
pub(crate) mod google;
pub(crate) mod graph;
pub(crate) mod gss;
pub(crate) mod fido;
pub mod forwarding;
mod imap;
mod imap_ext;
pub(crate) mod jmap;
pub(crate) mod jscal;
pub mod mime;
pub mod msg;
pub mod models;
pub(crate) mod mta_sts;
pub mod oauth;
mod pop;
pub(crate) mod proton;
pub(crate) mod proton_push;
pub(crate) mod public_net;
pub(crate) mod rest;
pub(crate) mod retry;
pub(crate) mod sasl;
pub(crate) mod sieve;
pub(crate) mod sieve_parse;
pub(crate) mod smtp;
pub(crate) mod stream;
pub(crate) mod transport;
pub(crate) mod websocket;
pub(crate) mod wire;
