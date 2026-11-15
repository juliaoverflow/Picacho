//! Display: GPUI views, assets, and desktop notifications.
mod assets;
mod desktop;
mod notify;
mod ui;
pub use ui::run;
/// Proton's human verification window (a process of its own).
pub mod verify;

mod html;
