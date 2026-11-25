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

    // Accounts and mail live in the app's sandbox; main.m keeps this folder
    // out of backups before it is opened.
    let Some(home) = std::env::var_os("HOME") else {
        log::error!("no home directory");
        return false;
    };
    let path = std::path::Path::new(&home).join("Library/Application Support/picacho");
    let open = |path: &std::path::Path| -> anyhow::Result<std::sync::Arc<manage::backend::Backend>> {
        let store = std::sync::Arc::new(manage::store::Store::open(path)?);
        manage::widgets::publish_soon(store.clone());
        Ok(display::new_backend(store))
    };
    let opened = open(&path);

    // The platform keeps the launch callback for `gpui_ios_did_finish_launching`.
    ffi::gpui_ios_initialize();
    let platform = Rc::new(gpui_mobile::ios::IosPlatform::new());
    let app = gpui::Application::with_platform(platform);
    match opened {
        Ok(backend) => display::run_on(app, backend),
        // A window says why (an encrypted database whose key is gone, say)
        // and offers to try again or start over.
        Err(e) => {
            log::error!("could not open the mail store at {}: {e:#}", path.display());
            let dir = path.clone();
            display::run_recovery_on(app, path, e, move || open(&dir));
        }
    }
    ffi::gpui_ios_did_finish_launching(std::ptr::null_mut());
    !ffi::gpui_ios_get_window().is_null()
}

unsafe extern "C" {
    /// Implemented by `packaging/ios/WidgetBridge.swift`: writes the widget
    /// snapshot into the shared App Group container and asks WidgetKit to
    /// reload Picacho's home-screen widget timelines.
    fn picacho_ios_publish_widgets(json: *const u8, len: usize);
}

/// Copies `json` into the iOS App Group container and reloads WidgetKit.
pub fn publish_widgets(json: &str) {
    // Safety: `json` points to `json.len()` valid UTF-8 bytes for the call's duration.
    unsafe { picacho_ios_publish_widgets(json.as_ptr(), json.len()) };
}
