//! Manage: domain types, persistent cache, encryption, and account coordination.
//! Display calls Backend and Store; protocol details live in Sync.
pub mod avatar;
pub mod backend;
/// Checking for new mail without a window (desktop background, Android's
/// service). iOS only reads its preferences.
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub mod background;
pub mod composing;
pub mod crypto;
pub mod forwarding;
pub mod local_ai;
pub mod mail_kinds;
/// Keeping memory small in the background (desktop).
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
pub mod memory;
pub mod model;
/// Tracking numbers in mail, for the reader and each account's Summary.
pub mod parcels;
pub mod receipts;
pub mod search;
pub mod status;
pub mod store;
pub mod subscriptions;
pub mod tracking;

pub mod reading;
