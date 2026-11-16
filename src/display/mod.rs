//! Display: GPUI views, assets, and desktop notifications.
mod assets;
mod desktop;
mod notify;
mod ui;
#[cfg(not(target_os = "android"))]
pub use ui::run;
#[cfg(target_os = "android")]
pub use ui::run_on;
/// Proton's human verification window (a process of its own).
#[cfg(not(target_os = "android"))]
pub mod verify;

mod html;
