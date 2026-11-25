//! Manage: domain types, persistent cache, encryption, and account coordination.
//! Display calls Backend and Store; protocol details live in Sync.
/// The app lock's passcode, biometrics setting and wrong tries.
pub mod applock;
pub mod avatar;
pub mod backend;
/// Checking for new mail without a window (desktop background, Android's
/// service). iOS only reads its preferences.
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub mod background;
pub mod composing;
pub mod crypto;
pub mod forwarding;
/// Mailing lists' List-Help, -Subscribe, -Post, -Owner, -Archive and
/// List-Id headers (RFC 2369, RFC 2919).
pub mod list_headers;
pub mod local_ai;
/// The text of attachments, for the local full-text index.
pub mod attachment_text;
pub mod mail_kinds;
/// Keeping memory small in the background (desktop).
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
pub mod memory;
pub mod model;
pub mod openpgp;
/// Pinned and muted mail, saved searches, blocked senders and rules kept here.
pub mod organize;
pub mod protected_reply;
pub mod smime;
/// Tracking numbers in mail, for the reader and each account's Summary.
pub mod parcels;
pub mod receipts;
/// The formatted composer's document, read from and written as HTML.
pub mod richtext;
/// Spelling for the formatted composer.
pub mod spelling;
pub mod search;
pub mod status;
pub mod store;
pub mod subscriptions;
pub mod tracking;
/// Home-screen widget snapshots and deep-link actions (Android and iOS).
pub mod widgets;
/// The optional encrypted database: its key and the keychain entry it's in.
pub mod vault;

pub mod reading;
