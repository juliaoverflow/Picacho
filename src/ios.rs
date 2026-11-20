//! Picacho on iOS. The host app (packaging/ios/main.m) calls
//! `picacho_ios_start` from `application:didFinishLaunchingWithOptions:`,
//! which opens the mail window, then drives frames and the app's lifecycle
//! through gpui-mobile's `gpui_ios_*` functions.

use crate::{display, manage};
use gpui_mobile::ios::ffi;
use std::rc::Rc;

/// Writes log records to standard error, which Xcode's console and
/// `xcrun devicectl device process launch --console` show.
struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{} {}] {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}

/// Opens the store and the mail window. Returns false if Picacho could not
/// start; the host then shows nothing rather than an empty window.
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_start() -> bool {
    static LOGGER: Stderr = Stderr;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));

    // Accounts and mail live in the app's sandbox, which iOS backs up.
    let Some(home) = std::env::var_os("HOME") else {
        log::error!("no home directory");
        return false;
    };
    let path = std::path::Path::new(&home).join("Library/Application Support/picacho");
    let store = match manage::store::Store::open(&path) {
        Ok(store) => std::sync::Arc::new(store),
        Err(e) => {
            log::error!("could not open the mail store at {}: {e:#}", path.display());
            return false;
        }
    };

    // The platform keeps the launch callback for `gpui_ios_did_finish_launching`.
    ffi::gpui_ios_initialize();
    let platform = Rc::new(gpui_mobile::ios::IosPlatform::new());
    display::run_on(
        gpui::Application::with_platform(platform),
        display::new_backend(store),
    );
    ffi::gpui_ios_did_finish_launching(std::ptr::null_mut());
    !ffi::gpui_ios_get_window().is_null()
}
