//! Picacho on Android: the activity loads this library and calls
//! `android_main` on a thread of its own. Other systems run the `picacho`
//! binary (main.rs), and this library is empty there.
#![cfg(target_os = "android")]

mod display;
mod manage;
mod sync;

use gpui_mobile::android::jni;

/// The activity's entry point (android-activity's NativeActivity glue).
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("picacho"),
    );
    // Panics reach logcat rather than ending the activity silently.
    jni::install_panic_hook();

    // Accounts and mail live in the app's private storage.
    let Some(path) = app.internal_data_path().map(|dir| dir.join("picacho")) else {
        log::error!("no internal data directory");
        return;
    };
    let store = match manage::store::Store::open(&path) {
        Ok(store) => std::sync::Arc::new(store),
        Err(e) => {
            log::error!("could not open the mail store at {}: {e:#}", path.display());
            return;
        }
    };

    jni::init_platform(&app);
    let Some(platform) = jni::shared_platform() else {
        log::error!("the GPUI platform did not start");
        return;
    };
    // Blocks on the activity's event loop; the app's window opens once the
    // system has given the activity its surface.
    display::run_on(gpui::Application::with_platform(platform.into_rc()), store);
}
