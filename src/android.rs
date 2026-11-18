//! Picacho on Android: the activity loads this library and calls
//! `android_main` on a thread of its own.
//!
//! Checking mail in the background. The app's Java side
//! (android/app/src/main/java) has a foreground service, `MailService`, that
//! keeps the app's process alive and unfrozen while checking in the
//! background is on (Settings → General, on by default):
//!
//! - While the activity exists (on screen or not), its window checks mail
//!   and announces new mail whenever it isn't in front, as on a desktop.
//! - Once the activity is gone (swiped away from Recents), `android_main`
//!   returns and the background monitor (`manage::background`) checks
//!   instead, in the same process, with the same backend.
//! - If Android restarts the service in a new process (after reclaiming
//!   memory, or at boot), the service starts the monitor itself.
//!
//! GPUI's Android platform starts once per process, so an activity created
//! again in a process whose first activity is gone can't show a window: the
//! Java activity then restarts the process (`PicachoActivity`).
//!
//! The Java side calls in through `tech.pinal.picacho.Native`; this side
//! calls out to `Notifier` (notifications) and `MailService`.

use crate::{
    display,
    manage::{
        backend::Backend,
        background::{self, Monitor},
        store::Store,
    },
};
use gpui_mobile::android::jni as platform;
use jni::{
    Env, EnvUnowned, JValue, JavaVM, jni_sig, jni_str,
    objects::{JClass, JObject, JString},
    refs::Global,
};
use std::{
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

/// The activity's entry point (android-activity's NativeActivity glue).
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    init_logging();
    // Panics reach logcat rather than ending the activity silently.
    platform::install_panic_hook();

    // GPUI's platform can't start twice in a process; the Java activity
    // restarts the process instead (see `PicachoActivity`).
    if UI_STARTED.swap(true, Ordering::SeqCst) {
        log::warn!("the activity was created again in a running process; it restarts the process");
        return;
    }

    // Accounts and mail live in the app's private storage.
    let Some(path) = app.internal_data_path().map(|dir| dir.join("picacho")) else {
        log::error!("no internal data directory");
        return;
    };
    let backend = {
        let mut process = process();
        process.ui = true;
        // The window checks mail from here on; the monitor's watches end
        // before the window's begin.
        process.monitor = None;
        match process.backend(&path) {
            Ok(backend) => backend,
            Err(e) => {
                log::error!("could not open the mail store at {}: {e:#}", path.display());
                process.ui = false;
                return;
            }
        }
    };
    // Starts (or stops) the service that keeps checking once the activity
    // is gone.
    set_background(background::enabled(&backend.store));

    platform::init_platform(&app);
    let Some(gpui_platform) = platform::shared_platform() else {
        log::error!("the GPUI platform did not start");
        process().ui = false;
        return;
    };
    // Blocks on the activity's event loop; the app's window opens once the
    // system has given the activity its surface. Returns once the activity
    // is destroyed.
    display::run_on(
        gpui::Application::with_platform(gpui_platform.into_rc()),
        backend.clone(),
    );
    drop(backend);

    let mut process = process();
    process.ui = false;
    process.reconcile();
    if let Some(backend) = &process.backend {
        if process.monitor.is_none() {
            // Nothing checks now: the window's watches would keep their
            // connections open for nobody.
            backend.stop_all_watching();
        }
        backend.store.flush();
    }
}

fn init_logging() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("picacho"),
    );
}

/// The activity's window has run in this process (it can't again).
static UI_STARTED: AtomicBool = AtomicBool::new(false);
/// This side set up ndk-context (for the keychain) in a process the service
/// started; android-activity sets it up itself, once, when an activity is
/// created, so it is released first (`beforeActivity`).
static CONTEXT_OURS: AtomicBool = AtomicBool::new(false);

/// What the activity and the service share in the process.
struct Process {
    /// The mail backend, opened by whichever comes first and kept for the
    /// process's life: one store and one set of connections and sessions.
    backend: Option<Arc<Backend>>,
    /// Checks mail while the service runs and the activity's window doesn't.
    monitor: Option<Monitor>,
    /// The activity's window is running.
    ui: bool,
    /// `MailService` is running.
    service: bool,
}

static PROCESS: Mutex<Process> = Mutex::new(Process {
    backend: None,
    monitor: None,
    ui: false,
    service: false,
});

fn process() -> MutexGuard<'static, Process> {
    PROCESS.lock().unwrap_or_else(|e| e.into_inner())
}

impl Process {
    fn backend(&mut self, path: &Path) -> anyhow::Result<Arc<Backend>> {
        if let Some(backend) = &self.backend {
            return Ok(backend.clone());
        }
        let backend = display::new_backend(Arc::new(Store::open(path)?));
        self.backend = Some(backend.clone());
        Ok(backend)
    }

    /// Starts the monitor when the service runs without a window and
    /// checking in the background is on; stops it otherwise.
    fn reconcile(&mut self) {
        let Some(backend) = &self.backend else {
            self.monitor = None;
            return;
        };
        let wanted = self.service && !self.ui && background::enabled(&backend.store);
        if !wanted {
            self.monitor = None;
            return;
        }
        if self.monitor.is_some() {
            return;
        }
        let announce: background::Announce = Box::new(display::notify::announce);
        match Monitor::start(backend.clone(), announce) {
            Ok(monitor) => self.monitor = Some(monitor),
            Err(e) => log::error!("could not check mail in the background: {e}"),
        }
    }
}

/// Turns checking in the background on or off (Settings): starts or stops
/// the service, which remembers the choice for starting at boot.
pub fn set_background(on: bool) {
    with_java("MailService.setEnabled", |env, java| {
        env.call_static_method(
            &java.service,
            jni_str!("setEnabled"),
            jni_sig!("(Landroid/content/Context;Z)V"),
            &[JValue::Object(java.context.as_obj()), JValue::Bool(on)],
        )
        .map(drop)
    });
}

/// Shows a notification (`Notifier.show`). `key` picks the notification:
/// one with the same key replaces it. Tapping it opens Picacho and hands
/// `target` (`notify::Target`'s JSON; empty for none) back through
/// `Native.open`.
pub fn notify(key: &str, title: &str, text: &str, sub_text: &str, target: &str) {
    with_java("Notifier.show", |env, java| {
        let title = env.new_string(title)?;
        let text = env.new_string(text)?;
        let sub_text = env.new_string(sub_text)?;
        let target = env.new_string(target)?;
        env.call_static_method(
            &java.notifier,
            jni_str!("show"),
            jni_sig!(
                "(Landroid/content/Context;ILjava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"
            ),
            &[
                JValue::Object(java.context.as_obj()),
                JValue::Int(notification_id(key)),
                JValue::Object(&title),
                JValue::Object(&text),
                JValue::Object(&sub_text),
                JValue::Object(&target),
            ],
        )
        .map(drop)
    });
}

/// The text last copied between Android's clipboard and GPUI's (which, on
/// Android, is only the app's own). Only a change on one side is copied to the
/// other, so neither overwrites newer text with what the other had before.
static CLIPBOARD_SYNCED: Mutex<Option<String>> = Mutex::new(None);

/// Before a paste: text copied in another app since the last sync becomes
/// what GPUI pastes.
pub fn clipboard_from_system(cx: &mut gpui::App) {
    let mut text = None;
    with_java("Clipboard.read", |env, java| {
        let read = env
            .call_static_method(
                &java.clipboard,
                jni_str!("read"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::Object(java.context.as_obj())],
            )?
            .l()?;
        if !read.is_null() {
            // Safety: `Clipboard.read` returns a String.
            let read = unsafe { JString::from_raw(env, read.into_raw()) };
            text = Some(read.to_string());
        }
        Ok(())
    });
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return;
    };
    let mut synced = CLIPBOARD_SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    if synced.as_deref() != Some(text.as_str()) {
        *synced = Some(text.clone());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }
}

/// The system clipboard's HTML, for pasting with formatting.
pub fn clipboard_html() -> Option<String> {
    let mut html = None;
    with_java("Clipboard.readHtml", |env, java| {
        let read = env
            .call_static_method(
                &java.clipboard,
                jni_str!("readHtml"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::Object(java.context.as_obj())],
            )?
            .l()?;
        if !read.is_null() {
            // Safety: `Clipboard.readHtml` returns a String.
            let read = unsafe { JString::from_raw(env, read.into_raw()) };
            html = Some(read.to_string());
        }
        Ok(())
    });
    html.filter(|h| !h.trim().is_empty())
}

/// After a copy, and as the app goes to the background: text copied in
/// Picacho since the last sync goes to Android's clipboard, for other apps.
pub fn clipboard_to_system(cx: &mut gpui::App) {
    let Some(text) = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    let mut synced = CLIPBOARD_SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    if synced.as_deref() == Some(text.as_str()) {
        return;
    }
    *synced = Some(text.clone());
    drop(synced);
    with_java("Clipboard.write", |env, java| {
        let text = env.new_string(&text)?;
        env.call_static_method(
            &java.clipboard,
            jni_str!("write"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::Object(java.context.as_obj()), JValue::Object(&text)],
        )
        .map(drop)
    });
}

/// A step of Android's Back (`PicachoActivity`'s back callback).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Back {
    /// A back swipe started, or moved: how far toward going back, 0 to 1.
    Swiping { started: bool, progress: f32 },
    /// Go back (a swipe let go of past the point, the button, a key).
    Go,
    /// A back swipe let go of without going back.
    Cancelled,
}

/// Where Back goes: the window's (see `back_events`).
static BACK: Mutex<Option<futures::channel::mpsc::UnboundedSender<Back>>> = Mutex::new(None);
/// Whether the back callback is registered, as last told: 0 off, 1 on, 2
/// not told yet.
static BACK_ENABLED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

/// Android's Back from now on, for the window. A window opened later takes
/// it over; the earlier one's stream then ends.
pub fn back_events() -> futures::channel::mpsc::UnboundedReceiver<Back> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    *BACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
    BACK_ENABLED.store(2, Ordering::SeqCst);
    receiver
}

/// Tells Android whether Picacho has somewhere to go back to. Off at its
/// first screen, where Back is Android's own (home). Cheap when unchanged:
/// called on every frame.
pub fn set_back_enabled(on: bool) {
    if BACK_ENABLED.swap(u8::from(on), Ordering::SeqCst) == u8::from(on) {
        return;
    }
    with_java("PicachoActivity.setBackEnabled", |env, java| {
        env.call_static_method(
            &java.activity,
            jni_str!("setBackEnabled"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(on)],
        )
        .map(drop)
    });
}

/// When `drawing` last told Java, in milliseconds since `DRAWING_EPOCH`.
static DRAWING_TOLD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DRAWING_EPOCH: OnceLock<std::time::Instant> = OnceLock::new();

/// Picacho is drawing a frame: while frames keep coming, the window
/// prefers the display's fastest mode (`PicachoActivity.drawing`), and a
/// second after the last one it stops. Called on every frame; Java is told
/// at most every 250 ms, well inside that second.
pub fn drawing() {
    let epoch = *DRAWING_EPOCH.get_or_init(std::time::Instant::now);
    // Starts at 1 so the first frame is told.
    let now = epoch.elapsed().as_millis() as u64 + 1;
    let told = DRAWING_TOLD.load(Ordering::Relaxed);
    if told != 0 && now.saturating_sub(told) < 250 {
        return;
    }
    if DRAWING_TOLD
        .compare_exchange(told, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    // Java only posts to its UI thread: the frame doesn't wait on it.
    with_java("PicachoActivity.drawing", |env, java| {
        env.call_static_method(&java.activity, jni_str!("drawing"), jni_sig!("()V"), &[])
            .map(drop)
    });
}

/// Back reached Picacho's first screen as a key (before Android 13, where
/// there is no back callback): Picacho goes to the background.
pub fn leave() {
    with_java("PicachoActivity.leave", |env, java| {
        env.call_static_method(&java.activity, jni_str!("leave"), jni_sig!("()V"), &[])
            .map(drop)
    });
}

/// `Native.back(int phase, float progress)`: from the back callback, on
/// Android's UI thread; the window handles it on its own.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_back<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    phase: jni::sys::jint,
    progress: jni::sys::jfloat,
) {
    native(env, |_| {
        let back = match phase {
            0 | 1 => Back::Swiping {
                started: phase == 0,
                progress: progress.clamp(0., 1.),
            },
            2 => Back::Go,
            _ => Back::Cancelled,
        };
        if let Some(sender) = BACK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = sender.unbounded_send(back);
        }
        Ok(())
    });
}

/// `Native.open(String target)`: a notification was tapped, opening the
/// activity (`PicachoActivity`); `target` is what it showed. The window
/// opens it, now or once it is up (`notify::clicked`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_open<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    target: JString<'caller>,
) {
    native(env, |_| {
        if !target.is_null() {
            display::notify::clicked(&target.to_string());
        }
        Ok(())
    });
}

/// The system bars', display cutout's and keyboard's insets, in points, as
/// Android last reported them (`PicachoActivity.onInsets`). Edge to edge
/// (Android 15 and later), gpui-mobile's safe area covers the whole screen and
/// its keyboard height stays 0; the phone UI takes the larger of each.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemInsets {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    pub keyboard: f32,
}

static SYSTEM_INSETS: Mutex<SystemInsets> = Mutex::new(SystemInsets {
    top: 0.,
    bottom: 0.,
    left: 0.,
    right: 0.,
    keyboard: 0.,
});
/// Where a change of insets is told: the window's, which redraws (the
/// keyboard opening resizes nothing, so nothing else would).
static INSETS_CHANGED: Mutex<Option<futures::channel::mpsc::UnboundedSender<()>>> =
    Mutex::new(None);

pub fn system_insets() -> SystemInsets {
    *SYSTEM_INSETS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Changes of the system insets from now on, for the window to redraw. A
/// window opened later takes them over; the earlier one's stream then ends.
pub fn insets_events() -> futures::channel::mpsc::UnboundedReceiver<()> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    *INSETS_CHANGED.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender);
    receiver
}

/// `Native.insets(top, bottom, left, right, keyboard)`: from the activity's
/// window insets listener, on Android's UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_insets<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    top: jni::sys::jfloat,
    bottom: jni::sys::jfloat,
    left: jni::sys::jfloat,
    right: jni::sys::jfloat,
    keyboard: jni::sys::jfloat,
) {
    native(env, |_| {
        let clean = |v: f32| if v.is_finite() { v.max(0.) } else { 0. };
        *SYSTEM_INSETS.lock().unwrap_or_else(|e| e.into_inner()) = SystemInsets {
            top: clean(top),
            bottom: clean(bottom),
            left: clean(left),
            right: clean(right),
            keyboard: clean(keyboard),
        };
        if let Some(sender) = INSETS_CHANGED.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = sender.unbounded_send(());
        }
        Ok(())
    });
}

/// A notification id for `key`, the same in every process. The ongoing
/// service notification has its own id, below these.
fn notification_id(key: &str) -> i32 {
    // FNV-1a: stable across runs, unlike the standard library's hasher.
    let hash = key.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    // Positive, and clear of the service's ids (1..1000).
    (1000 + hash % (i32::MAX as u32 - 1000)) as i32
}

/// What calls into Java need, from `Native.attach`.
struct Java {
    vm: JavaVM,
    /// The application's context.
    context: Global<JObject<'static>>,
    notifier: Global<JClass<'static>>,
    service: Global<JClass<'static>>,
    clipboard: Global<JClass<'static>>,
    activity: Global<JClass<'static>>,
    /// Browser sign-ins (Gmail, Outlook, Pinal).
    sign_in: Global<JClass<'static>>,
    /// Proton's human verification page.
    verify: Global<JClass<'static>>,
    /// Security keys over USB and NFC.
    keys: Global<JClass<'static>>,
    /// Passkeys, through Android's Credential Manager.
    passkeys: Global<JClass<'static>>,
    /// The app's package name (`tech.pinal.picacho`, or the beta's).
    package: String,
}

static JAVA: OnceLock<Java> = OnceLock::new();

/// Runs `call` with the Java side, on this thread (attached for the call),
/// logging what went wrong. Java exceptions are cleared.
fn with_java(what: &str, call: impl FnOnce(&mut Env, &Java) -> jni::errors::Result<()>) {
    call_java(what, call);
}

/// `with_java`, keeping what `call` returns: `None` when the Java side
/// isn't attached, or the call failed or threw.
fn call_java<T>(
    what: &str,
    call: impl FnOnce(&mut Env, &Java) -> jni::errors::Result<T>,
) -> Option<T> {
    let Some(java) = JAVA.get() else {
        log::warn!("{what}: the Java side isn't attached");
        return None;
    };
    let result = java
        .vm
        .attach_current_thread(|env| -> jni::errors::Result<T> {
            let result = call(env, java);
            if env.exception_check() {
                env.exception_describe();
                env.exception_clear();
                // A call that threw has no result, whatever it returned.
                return Err(jni::errors::Error::JavaException);
            }
            result
        });
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("{what}: {e}");
            None
        }
    }
}

/// A Java string (or null) as a Rust one.
fn rust_string(env: &mut Env, object: JObject) -> Option<String> {
    if object.is_null() {
        return None;
    }
    // Safety: the callers pass objects their Java signatures make Strings.
    let string = unsafe { JString::from_raw(env, object.into_raw()) };
    Some(string.to_string())
}

/// Runs a native method's body, turning an error or panic into a log line
/// (never an exception the Java side would have to handle).
fn native(mut env: EnvUnowned<'_>, body: impl FnOnce(&mut Env) -> jni::errors::Result<()>) {
    let outcome = env.with_env(|env| -> jni::errors::Result<()> {
        let result = body(env);
        if env.exception_check() {
            env.exception_describe();
            env.exception_clear();
        }
        result
    });
    outcome.resolve::<jni::errors::LogErrorAndDefault>()
}

/// `Native.attach(Context)`: the activity and the service hand over the
/// application's context first thing. Kept for the process's life.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_attach<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    context: JObject<'caller>,
) {
    init_logging();
    native(env, |env| {
        if JAVA.get().is_some() {
            return Ok(());
        }
        // Inside a native method, find_class uses the app's class loader.
        let notifier = env.find_class(jni_str!("tech/pinal/picacho/Notifier"))?;
        let service = env.find_class(jni_str!("tech/pinal/picacho/MailService"))?;
        let clipboard = env.find_class(jni_str!("tech/pinal/picacho/Clipboard"))?;
        let activity = env.find_class(jni_str!("tech/pinal/picacho/PicachoActivity"))?;
        let sign_in = env.find_class(jni_str!("tech/pinal/picacho/SignIn"))?;
        let verify = env.find_class(jni_str!("tech/pinal/picacho/VerifyActivity"))?;
        let keys = env.find_class(jni_str!("tech/pinal/picacho/SecurityKeys"))?;
        let passkeys = env.find_class(jni_str!("tech/pinal/picacho/Passkeys"))?;
        let package = env
            .call_method(
                &context,
                jni_str!("getPackageName"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let package = rust_string(env, package).unwrap_or_default();
        let java = Java {
            vm: env.get_java_vm()?,
            context: env.new_global_ref(&context)?,
            notifier: env.new_global_ref(&notifier)?,
            service: env.new_global_ref(&service)?,
            clipboard: env.new_global_ref(&clipboard)?,
            activity: env.new_global_ref(&activity)?,
            sign_in: env.new_global_ref(&sign_in)?,
            verify: env.new_global_ref(&verify)?,
            keys: env.new_global_ref(&keys)?,
            passkeys: env.new_global_ref(&passkeys)?,
            package,
        };
        let _ = JAVA.set(java);
        Ok(())
    });
}

/// `Native.beforeActivity()`: the first activity of this process is about to
/// start. The window takes over from the monitor, and ndk-context is left for
/// android-activity to set up.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_beforeActivity<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) {
    native(env, |_| {
        let mut process = process();
        process.ui = true;
        process.monitor = None;
        if CONTEXT_OURS.swap(false, Ordering::SeqCst) {
            // Safety: set by `start_service_monitor`, and nothing else
            // released it.
            unsafe { ndk_context::release_android_context() };
        }
        Ok(())
    });
}

/// `Native.serviceStarted(String dataDir)`: `MailService` started. Without
/// the activity's window (a process Android started for the service), the
/// monitor starts. The store opens off the service's main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_serviceStarted<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    data_dir: JString<'caller>,
) {
    native(env, |_| {
        let path = std::path::PathBuf::from(data_dir.to_string());
        let spawned = std::thread::Builder::new()
            .name("picacho-service".into())
            .spawn(move || start_service_monitor(&path));
        if let Err(e) = spawned {
            log::error!("could not start checking mail in the background: {e}");
        }
        Ok(())
    });
}

fn start_service_monitor(path: &Path) {
    let mut process = process();
    process.service = true;
    if process.ui {
        // The window checks while it runs; the monitor starts once it's gone.
        return;
    }
    // The keychain finds the app through ndk-context, which only an
    // activity sets up.
    if !UI_STARTED.load(Ordering::SeqCst)
        && !CONTEXT_OURS.load(Ordering::SeqCst)
        && let Some(java) = JAVA.get()
    {
        // Safety: no activity has run in this process, so android-activity
        // hasn't set it; it's released before one does (`beforeActivity`).
        // The context is a global reference kept for the process's life.
        unsafe {
            ndk_context::initialize_android_context(
                java.vm.get_raw().cast(),
                java.context.as_obj().as_raw().cast(),
            );
        }
        CONTEXT_OURS.store(true, Ordering::SeqCst);
    }
    if let Err(e) = process.backend(path) {
        log::error!("could not open the mail store at {}: {e:#}", path.display());
        return;
    }
    process.reconcile();
}

/// `Native.serviceStopped()`: `MailService` ended (turned off, or stopped
/// from its notification); the monitor stops with it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_serviceStopped<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) {
    native(env, |_| {
        let mut process = process();
        process.service = false;
        process.reconcile();
        if process.monitor.is_none()
            && !process.ui
            && let Some(backend) = &process.backend
        {
            backend.stop_all_watching();
        }
        Ok(())
    });
}

/// `Native.shutdown()`: the process is about to end (it restarts for a new
/// activity). The monitor stops and preferences being written are.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_shutdown<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
) {
    native(env, |_| {
        let mut process = process();
        process.monitor = None;
        if let Some(backend) = &process.backend {
            backend.stop_all_watching();
            backend.store.flush();
        }
        Ok(())
    });
}

mod keys;
mod requests;

pub use keys::{KeySearch, Nfc, passkey};

/// Opens a browser sign-in's page (Gmail, Outlook, Pinal) in a Custom Tab,
/// or the browser. Until `sign_in_ended`, a foreground service keeps the
/// process from being frozen while the browser is in front: the sign-in's
/// loopback listener answers the browser's last request.
pub fn open_sign_in(url: &str) -> anyhow::Result<()> {
    let opened = call_java("SignIn.open", |env, java| {
        let url = env.new_string(url)?;
        env.call_static_method(
            &java.sign_in,
            jni_str!("open"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Z"),
            &[JValue::Object(java.context.as_obj()), JValue::Object(&url)],
        )?
        .z()
    });
    anyhow::ensure!(opened == Some(true), "Could not open the browser");
    Ok(())
}

/// The browser sign-in is over, however it ended.
pub fn sign_in_ended() {
    with_java("SignIn.ended", |env, java| {
        env.call_static_method(
            &java.sign_in,
            jni_str!("ended"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(java.context.as_obj())],
        )
        .map(drop)
    });
}

/// Where the browser's last page sends people back to Picacho: an
/// `intent:` address for the app's own scheme (its package name).
pub fn return_url() -> Option<String> {
    let package = &JAVA.get()?.package;
    let safe = !package.is_empty()
        && package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_');
    safe.then(|| format!("intent://signed-in#Intent;scheme={package};package={package};end"))
}

/// Shows Proton's human verification page at `url` (`VerifyActivity`) and
/// waits until it passes, `Some((token, type))`, or is closed, `None`.
pub fn verify(url: &str, cancel: &AtomicBool) -> anyhow::Result<Option<(String, String)>> {
    use requests::{Request, Waited};
    let request = Request::new();
    let id = request.id();
    let started = call_java("VerifyActivity.start", |env, java| {
        let url = env.new_string(url)?;
        env.call_static_method(
            &java.verify,
            jni_str!("start"),
            jni_sig!("(Landroid/content/Context;ILjava/lang/String;)Z"),
            &[
                JValue::Object(java.context.as_obj()),
                JValue::Int(id),
                JValue::Object(&url),
            ],
        )?
        .z()
    });
    anyhow::ensure!(
        started == Some(true),
        "Proton wants to check that it's you, but its check couldn't be shown."
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30 * 60);
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
            Waited::Cancelled => break Err(anyhow::anyhow!("Sign-in cancelled.")),
        }
    };
    with_java("VerifyActivity.close", |env, java| {
        env.call_static_method(
            &java.verify,
            jni_str!("close"),
            jni_sig!("(I)V"),
            &[JValue::Int(id)],
        )
        .map(drop)
    });
    outcome
}
