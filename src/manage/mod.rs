//! Manage: domain types, persistent cache, encryption, and account coordination.
//! Display calls Backend and Store; protocol details live in Sync.
pub mod avatar;
pub mod backend;
pub mod composing;
pub mod crypto;
pub mod forwarding;
pub mod model;
pub mod search;
pub mod status;
pub mod store;
pub mod subscriptions;
pub mod tracking;

pub mod reading;
