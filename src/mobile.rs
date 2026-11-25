//! Picacho on phones. Android's activity loads this library as a shared
//! object (android.rs); iOS links it statically into the Xcode host app in
//! packaging/ios (ios.rs). Other systems run the `picacho` binary (main.rs),
//! and this library is empty there.
#![cfg(any(target_os = "android", target_os = "ios"))]

// First: `t!` (the interface's text) is for every module after it.
#[macro_use]
mod i18n;
mod display;
mod manage;
mod sync;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;
