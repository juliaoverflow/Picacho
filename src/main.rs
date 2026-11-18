mod display;
mod manage;
mod sync;

fn main() -> anyhow::Result<()> {
    // Before any other thread starts.
    manage::memory::configure();
    // Started by a Proton sign-in to show its human verification page.
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new(sync::proton::VERIFY_FLAG)) {
        let url = args.next().unwrap_or_default();
        display::verify::run(&url.to_string_lossy());
    }
    // Started by the window's process as it quits, to check mail in the
    // background with a small process of its own (`display::resident`).
    if first.as_deref() == Some(std::ffi::OsStr::new(display::resident::FLAG)) {
        if let Some(dir) = args.next() {
            return display::resident::run(std::path::Path::new(&dir));
        }
        return Ok(());
    }
    let project = directories::ProjectDirs::from("tech", "pinal", "picacho")
        .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
    // The background process opens the window on the directory it used.
    let chosen = (first.as_deref() == Some(std::ffi::OsStr::new(display::resident::DATA_DIR_FLAG)))
        .then(|| args.next())
        .flatten();
    let path = chosen
        .or_else(|| std::env::var_os("PICACHO_DATA_DIR"))
        .or_else(|| std::env::var_os("PINAL_MAIL_DATA_DIR"))
        .map(std::path::PathBuf::from)
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
    let listener = match display::instance::claim(&path) {
        Some(display::instance::Claim::Forwarded) => return Ok(()),
        Some(display::instance::Claim::Primary(listener)) => Some(listener),
        None => None,
    };
    let store = std::sync::Arc::new(manage::store::Store::open(&path)?);
    display::run(store, listener);
    Ok(())
}
