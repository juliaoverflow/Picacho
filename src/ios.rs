//! Picacho on iOS. The host app (packaging/ios/main.m) calls
//! `picacho_ios_start` when its scene connects, which opens the mail window,
//! then drives frames and the app's lifecycle through gpui-mobile's
//! `gpui_ios_*` functions. The Swift beside it (packaging/ios/*.swift) does
//! what the system offers through its own frameworks, and calls in here.
//!
//! The mail backend is the process's, opened by whichever comes first: the
//! window, a background refresh, or a notification's button, which iOS may
//! each start the app for without the others (a background launch connects
//! no scene, so no window opens).
//!
//! Checking mail in the background. iOS doesn't let apps keep connections
//! open once they leave the screen. While checking in the background is on
//! (Settings → General, on by default), the host asks for background app
//! refresh (BGTaskScheduler); when iOS grants a moment, `background_refresh`
//! does one pass of the background monitor's work (`background::check_once`)
//! and announces new mail. iOS decides how often, by how Picacho is used.
//!
//! Notifications go through UserNotifications (Notifications.swift): tapping
//! one opens what it tells of, and its buttons (Mark as Read, Archive,
//! Delete, Reply) act as on the other systems (`notification_action`).

use crate::{
    display,
    manage::{self, backend::Backend, background, store::Store},
};
use gpui_mobile::ios::ffi;
use std::{
    ffi::c_void,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) mod keys;
mod requests;

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

/// Logging and the panic hook, once, for whichever entry point runs first.
fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        static LOGGER: Stderr = Stderr;
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Info);
        }
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            default_hook(info);
        }));
    });
}

/// Accounts and mail live in the app's sandbox; main.m keeps this folder
/// out of backups before it is opened.
fn data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/picacho"))
}

/// The process's backend, once opened: one store and one set of
/// connections and sessions for the window and the background.
static BACKEND: Mutex<Option<Arc<Backend>>> = Mutex::new(None);

/// The process's backend, opening the store if nothing has yet. A failure
/// isn't kept: the next caller tries again (the Keychain may only open once
/// the device has been unlocked).
fn backend() -> anyhow::Result<Arc<Backend>> {
    let mut opened = BACKEND.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(backend) = &*opened {
        return Ok(backend.clone());
    }
    let path = data_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let store = Arc::new(Store::open(&path)?);
    manage::widgets::publish_soon(store.clone());
    // Mail in Spotlight, when turned on (Settings → Privacy). A result
    // chosen comes through the scene (main.m, `picacho_ios_launch`).
    manage::system_search::start(store.clone(), |_| {});
    let backend = display::new_backend(store);
    // Taps that come before the window is up wait for it; buttons pressed
    // without a window act through `notification_action`.
    display::notify::set_background(display::notify::Background {
        backend: backend.clone(),
        open: None,
    });
    *opened = Some(backend.clone());
    Ok(backend)
}

/// Opens the store and the mail window. Returns false if Picacho could not
/// start; the host then shows nothing rather than an empty window.
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_start() -> bool {
    init();
    let Some(path) = data_dir() else {
        log::error!("no home directory");
        return false;
    };
    let opened = backend();
    if let Ok(backend) = &opened {
        set_background(background::enabled(&backend.store));
    }
    register_categories();

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
            display::run_recovery_on(app, path, e, || {
                let backend = backend()?;
                set_background(background::enabled(&backend.store));
                Ok(backend)
            });
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
    /// Notifications.swift: shows a notification, described by JSON (see
    /// `notify`).
    fn picacho_ios_notify(json: *const u8, len: usize);
    /// Notifications.swift: the notification categories, by JSON (see
    /// `register_categories`).
    fn picacho_ios_notification_categories(json: *const u8, len: usize);
    /// BackgroundRefresh.swift: asks for background app refresh, or stops.
    fn picacho_ios_set_background(on: bool);
    /// Verify.swift: shows Proton's verification page at `url` for request
    /// `id`, under `title`; false if it couldn't be shown.
    fn picacho_ios_verify_start(
        id: i32,
        url: *const u8,
        len: usize,
        title: *const u8,
        title_len: usize,
    ) -> bool;
    /// Verify.swift: closes request `id`'s page, if it still shows.
    fn picacho_ios_verify_close(id: i32);
}

/// Copies `json` into the iOS App Group container and reloads WidgetKit.
pub fn publish_widgets(json: &str) {
    // Safety: `json` points to `json.len()` valid UTF-8 bytes for the call's duration.
    unsafe { picacho_ios_publish_widgets(json.as_ptr(), json.len()) };
}

/// Shows a notification. `key` picks it: one with the same key replaces it
/// (a newer announcement for an account replaces the last). Tapping it
/// opens Picacho and hands `target` (`notify::Target`'s JSON; empty for
/// none) back through `picacho_ios_notification_tapped`; `category` names
/// its buttons (`notify::categories`), whose presses reach
/// `picacho_ios_notification_action`.
pub fn notify(key: &str, title: &str, subtitle: &str, body: &str, target: &str, category: &str) {
    let json = serde_json::json!({
        "id": key,
        "title": title,
        "subtitle": subtitle,
        "body": body,
        "target": target,
        "category": category,
        // Grouped by account, as Android groups them.
        "thread": subtitle,
    })
    .to_string();
    // Safety: `json` is valid for the call; Swift copies what it keeps.
    unsafe { picacho_ios_notify(json.as_ptr(), json.len()) };
}

/// Tells the Swift side which buttons each category of notification has
/// (at launch, and again when the language changes: the buttons' words).
pub fn register_categories() {
    let categories: Vec<_> = display::notify::categories()
        .iter()
        .map(|(id, buttons)| {
            serde_json::json!({
                "id": id,
                "actions": buttons.iter().map(|(key, label)| serde_json::json!({
                    "id": key,
                    "title": label,
                    "destructive": *key == "trash",
                    // Reply opens Picacho to write; the others act without.
                    "foreground": display::notify::quick_change(key).is_none(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let json = serde_json::Value::from(categories).to_string();
    // Safety: `json` is valid for the call.
    unsafe { picacho_ios_notification_categories(json.as_ptr(), json.len()) };
}

/// Turns checking in the background on or off (Settings): asks iOS for
/// background app refresh, or stops asking.
pub fn set_background(on: bool) {
    // Safety: no pointers.
    unsafe { picacho_ios_set_background(on) };
}

/// Shows Proton's human verification page at `url` in a sheet over the app
/// (Verify.swift) and waits until it passes, `Some((token, type))`, or is
/// closed, `None`.
pub fn verify(url: &str, cancel: &AtomicBool) -> anyhow::Result<Option<(String, String)>> {
    use requests::{Request, Waited};
    let request = Request::new();
    let id = request.id();
    let title = t!("ios-verify-that-its-you");
    // Safety: `url` and `title` are valid for the call; Swift copies them.
    let shown = unsafe {
        picacho_ios_verify_start(id, url.as_ptr(), url.len(), title.as_ptr(), title.len())
    };
    anyhow::ensure!(shown, t!("ios-proton-wants-to-check-that-its"));
    let deadline = std::time::Instant::now() + Duration::from_secs(30 * 60);
    let outcome = loop {
        match request.next(cancel, deadline) {
            Waited::Answer(Some(message)) => {
                match crate::sync::proton::verification_result(&message) {
                    Ok(Some(found)) => break Ok(Some(found)),
                    Ok(None) => {}
                    Err(()) => break Ok(None),
                }
            }
            Waited::Answer(None) | Waited::TimedOut => break Ok(None),
            Waited::Cancelled => break Err(anyhow::anyhow!(t!("ios-sign-in-cancelled"))),
        }
    };
    // Safety: no pointers.
    unsafe { picacho_ios_verify_close(id) };
    outcome
}

/// A completion the Swift side passed in: called once, from any thread,
/// with whether the work went.
struct Done {
    call: extern "C" fn(*mut c_void, bool),
    context: *mut c_void,
    called: bool,
}
// Safety: the Swift side's context is an object retained for this call,
// which it releases on whichever thread calls it.
unsafe impl Send for Done {}
impl Done {
    fn new(call: extern "C" fn(*mut c_void, bool), context: *mut c_void) -> Self {
        Self { call, context, called: false }
    }
    fn finish(mut self, ok: bool) {
        self.called = true;
        (self.call)(self.context, ok);
    }
}
impl Drop for Done {
    /// Never finished (a thread that couldn't be started took it along):
    /// called now, unsuccessful, so the Swift side's completion runs and
    /// its retained context is released.
    fn drop(&mut self) {
        if !self.called {
            self.called = true;
            (self.call)(self.context, false);
        }
    }
}

/// Whether the app lock is on, so the window is covered as Picacho leaves
/// the screen (main.m's `sceneWillResignActive:`), before iOS takes the
/// app switcher's picture of it: GPUI's own cover may not be drawn by then.
static PRIVACY_COVER: AtomicBool = AtomicBool::new(false);

/// Sets whether the window is covered as Picacho leaves the screen.
pub fn set_privacy_cover(on: bool) {
    PRIVACY_COVER.store(on, Ordering::Relaxed);
}

/// For main.m: cover the window now that Picacho leaves the screen.
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_privacy_cover() -> bool {
    PRIVACY_COVER.load(Ordering::Relaxed)
}

/// `len` UTF-8 bytes at `ptr` (null for none).
///
/// # Safety
/// `ptr` is null or points to `len` readable bytes for the call.
unsafe fn text(ptr: *const u8, len: usize) -> Option<String> {
    if ptr.is_null() || len > display::launch::MAX_REQUEST * 8 {
        return None;
    }
    // Safety: as the caller promises.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// A notification was tapped: `target` is what it showed. The window opens
/// it, now or once it is up (`notify::clicked`).
///
/// # Safety
/// `target` points to `len` readable bytes for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn picacho_ios_notification_tapped(target: *const u8, len: usize) {
    init();
    // Safety: as the caller promises.
    if let Some(target) = unsafe { text(target, len) } {
        display::notify::clicked(&target);
    }
}

/// How long iOS is asked to keep Picacho awake for the window to act on a
/// button (it acts as from its own list, which reports no end).
const GRACE: Duration = Duration::from_secs(8);

/// A button on a notification was pressed: `action` is its key, `target`
/// what the notification showed. Reply (which opens Picacho) goes to the
/// window as a tap that says to reply. Mark as Read, Archive and Delete
/// don't open Picacho: the window acts if there is one, otherwise the
/// backend does here. `done` is called once it has (iOS ends the app's
/// time then).
///
/// # Safety
/// `target` and `action` point to `target_len` and `action_len` readable
/// bytes for the call; `done` may be called once, from any thread, with
/// `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn picacho_ios_notification_action(
    target: *const u8,
    target_len: usize,
    action: *const u8,
    action_len: usize,
    done: extern "C" fn(*mut c_void, bool),
    context: *mut c_void,
) {
    init();
    let done = Done::new(done, context);
    // Safety: as the caller promises.
    let (target, action) = unsafe { (text(target, target_len), text(action, action_len)) };
    let (Some(json), Some(action)) = (target, action) else {
        return done.finish(false);
    };
    let Ok(mut target) = serde_json::from_str::<display::notify::Target>(&json) else {
        return done.finish(false);
    };
    let Some(change) = display::notify::quick_change(&action) else {
        target.action = Some(action);
        if let Ok(json) = serde_json::to_string(&target) {
            display::notify::clicked(&json);
        }
        return done.finish(true);
    };
    if display::notify::window_listening() {
        display::notify::quick(&json, &action);
        std::thread::spawn(move || {
            std::thread::sleep(GRACE);
            done.finish(true);
        });
        return;
    }
    std::thread::spawn(move || {
        let acted = backend().and_then(|b| display::notify::act(&b, &target, change));
        if let Err(e) = &acted {
            log::warn!("a notification's button couldn't act: {e:#}");
        }
        done.finish(acted.is_ok());
    });
}

/// Whether to ask iOS for background app refresh: checking in the
/// background is on (Settings → General). On if the store can't be read.
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_background_enabled() -> bool {
    init();
    backend()
        .map(|b| background::enabled(&b.store))
        .unwrap_or(true)
}

/// A background refresh is running: its time can be cut short.
static REFRESHING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// iOS granted a background refresh: one pass of the background monitor's
/// work, on a thread of its own. `done` is called with whether every check
/// went, once it's over (or at once, when checking in the background is
/// off, or a refresh is already running).
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_background_refresh(
    done: extern "C" fn(*mut c_void, bool),
    context: *mut c_void,
) {
    init();
    let done = Done::new(done, context);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut refreshing = REFRESHING.lock().unwrap_or_else(|e| e.into_inner());
        if refreshing.is_some() {
            return done.finish(true);
        }
        *refreshing = Some(cancel.clone());
    }
    let spawned = std::thread::Builder::new()
        .name("picacho-refresh".into())
        .spawn(move || {
            let ok = match backend() {
                Ok(backend) if background::enabled(&backend.store) => {
                    let announce: background::Announce = Box::new(display::notify::announce);
                    let ok = background::check_once(backend.clone(), &announce, cancel);
                    backend.store.flush();
                    ok
                }
                Ok(_) => true,
                Err(e) => {
                    log::warn!("could not check mail in the background: {e:#}");
                    false
                }
            };
            *REFRESHING.lock().unwrap_or_else(|e| e.into_inner()) = None;
            done.finish(ok);
        });
    if let Err(e) = spawned {
        // Its `done` went with the closure, and was called (unsuccessful)
        // as it was dropped.
        log::error!("could not check mail in the background: {e}");
        *REFRESHING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// iOS's time for the background refresh is up: it stops between steps
/// (and calls its `done`).
#[unsafe(no_mangle)]
pub extern "C" fn picacho_ios_background_expire() {
    if let Some(cancel) = &*REFRESHING.lock().unwrap_or_else(|e| e.into_inner()) {
        cancel.store(true, Ordering::Relaxed);
    }
}

/// Something to open (Shares.swift: what the Share extension handed over;
/// main.m: a Spotlight result chosen): a `mailto:` address, `attach:` and a
/// file's path, or a message's `picacho://widget/message` link. The window takes
/// it (`display::launch`); requests pushed together start one message.
///
/// # Safety
/// `request` points to `len` readable bytes for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn picacho_ios_launch(request: *const u8, len: usize) {
    init();
    // Safety: as the caller promises.
    if let Some(request) = unsafe { text(request, len) }.and_then(|r| display::launch::request(&r))
    {
        display::launch::push(request);
    }
}
