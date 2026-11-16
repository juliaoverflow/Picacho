//! Display: GPUI views, assets, and desktop notifications.
mod assets;
mod desktop;
mod notify;
mod ui;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use ui::run;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use ui::run_on;
/// Proton's human verification window (a process of its own).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod verify;

mod html;
