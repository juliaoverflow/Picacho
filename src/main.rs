// A Windows app rather than a console one: launching it opens no console
// window. Debug builds keep the console for their output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod display;
mod manage;
mod sync;

fn main() -> anyhow::Result<()> {
    // Before any other thread starts.
    manage::memory::configure();
    // Before any window, tray icon or notification.
    #[cfg(windows)]
    display::windows::set_app_identity();
    // Started by a Proton sign-in to show its human verification page.
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new(sync::proton::VERIFY_FLAG)) {
        // On standard input, not in the arguments (see `proton::verify_human`).
        let mut url = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut url);
        display::verify::run(url.trim());
    }
    // Started by the window's process as it quits, to check mail in the
    // background with a small process of its own (`display::resident`).
    if first.as_deref() == Some(std::ffi::OsStr::new(display::resident::FLAG)) {
        if let Some(dir) = args.next() {
            return display::resident::run(std::path::Path::new(&dir));
        }
        return Ok(());
    }
    // Widget snapshot helpers for external Linux bars/scripts (Waybar, Polybar,
    // Eww, Conky, etc.): print JSON and exit without touching the running app.
    let widget_json_flag = first.as_deref() == Some(std::ffi::OsStr::new("--widget-json"));
    let widget_waybar_flag = first.as_deref() == Some(std::ffi::OsStr::new("--widget-waybar"));
    if widget_json_flag || widget_waybar_flag {
        let project = directories::ProjectDirs::from("tech", "pinal", "picacho")
            .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
        let path = std::env::var_os("PICACHO_DATA_DIR")
            .or_else(|| std::env::var_os("PINAL_MAIL_DATA_DIR"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| project.data_dir().to_owned());
        // The snapshot the app (or its background process) keeps up to date.
        // The mail store isn't opened here: Picacho may be running, and only
        // one process opens it (see `display::instance`). Before the first
        // snapshot is written, an empty one is printed.
        let snap = manage::widgets::read_cached(&path).unwrap_or_else(|| {
            manage::widgets::Snapshot::empty("green", chrono::Utc::now().timestamp())
        });
        if widget_waybar_flag {
            println!("{}", manage::widgets::waybar_json(&snap));
        } else {
            println!("{}", serde_json::to_string(&snap)?);
        }
        return Ok(());
    }
    let project = directories::ProjectDirs::from("tech", "pinal", "picacho")
        .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
    // The background process opens the window on the directory it used, and
    // direct launches may pass `mailto:` links or `.eml`/`.msg` files.
    let (chosen, requests) = display::launch::parse_args(std::env::args_os().skip(1));
    let path = chosen
        .or_else(|| {
            std::env::var_os("PICACHO_DATA_DIR")
                .or_else(|| std::env::var_os("PINAL_MAIL_DATA_DIR"))
                .map(std::path::PathBuf::from)
        })
        .unwrap_or_else(|| {
            let path = project.data_dir().to_owned();
            // Keep accounts and mail from earlier identifiers: com.pinal.Picacho,
            // and before that, when the app was called Pinal Mail.
            for name in ["Picacho", "PinalMail"] {
                if let Some(old) = directories::ProjectDirs::from("com", "pinal", name)
                    && !path.exists()
                    && old.data_dir().exists()
                {
                    let _ = std::fs::rename(old.data_dir(), &path);
                }
            }
            path
        });
    // Picacho may be running already, perhaps with no window (in the
    // background): show that one rather than open the cache twice.
    let listener = match display::instance::claim(&path, &requests) {
        Some(display::instance::Claim::Forwarded) => return Ok(()),
        // Running already but not answering: opening the cache too would
        // have two processes check the same accounts.
        Some(display::instance::Claim::Busy) => {
            anyhow::bail!("Picacho is already running but isn't responding. Quit it, then open Picacho again.")
        }
        Some(display::instance::Claim::Primary(listener)) => Some(listener),
        None => None,
    };
    for r in display::launch::load_pending(&path) {
        display::launch::push(r);
    }
    for r in requests {
        display::launch::push(r);
    }
    match manage::store::Store::open(&path) {
        Ok(store) => display::run(std::sync::Arc::new(store), listener),
        // An encrypted database whose key is gone, a keychain that refused:
        // a window says so and offers what can be done, rather than the
        // app quitting with a message nobody sees.
        Err(e) => {
            eprintln!("Picacho: {e:#}");
            display::run_recovery(path, e, listener);
        }
    }
    Ok(())
}
