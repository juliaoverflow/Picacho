//! Manage: domain types, persistent cache, encryption, and account coordination.
//! Display calls Backend and Store; protocol details live in Sync.
pub mod backend;
pub mod crypto;
pub mod model;
pub mod status;
pub mod store;

pub mod reading;
