//! Display: GPUI views, assets, and desktop notifications.
mod assets;
mod desktop;
pub mod notify;
/// `mailto:` addresses and email files Picacho was opened with.
pub mod launch;
pub(crate) mod ui;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use ui::run;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub use ui::{new_backend, run_on};
/// One Picacho per data directory: later launches show its window.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod instance;
/// Picacho in the background: a small process without the window.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod resident;
/// The tray icon while Picacho runs in the background.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod tray;
/// Proton's human verification window (a process of its own).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod verify;
/// What Windows needs of Picacho itself, and its delay-loaded DLLs.
#[cfg(windows)]
pub mod windows;
/// Open With… for attachments on macOS and Linux.
pub(crate) mod open_with;

mod html;
