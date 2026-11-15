mod display;
mod manage;
mod sync;

fn main() -> anyhow::Result<()> {
    // Started by a Proton sign-in to show its human verification page.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(sync::proton::VERIFY_FLAG) {
        display::verify::run(&args.next().unwrap_or_default());
    }
    let project = directories::ProjectDirs::from("tech", "pinal", "picacho")
        .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
    let path = std::env::var_os("PICACHO_DATA_DIR")
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
    let store = std::sync::Arc::new(manage::store::Store::open(&path)?);
    display::run(store);
    Ok(())
}
