mod assets;
mod discover;
mod mail;
mod model;
mod notify;
mod oauth;
mod store;
mod ui;

fn main() -> anyhow::Result<()> {
    let project = directories::ProjectDirs::from("com", "pinal", "Picacho")
        .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
    let path = std::env::var_os("PICACHO_DATA_DIR")
        .or_else(|| std::env::var_os("PINAL_MAIL_DATA_DIR"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let path = project.data_dir().to_owned();
            // Keep accounts and mail from when the app was called Pinal Mail.
            if let Some(old) = directories::ProjectDirs::from("com", "pinal", "PinalMail")
                && !path.exists()
                && old.data_dir().exists()
            {
                let _ = std::fs::rename(old.data_dir(), &path);
            }
            path
        });
    let store = std::sync::Arc::new(store::Store::open(&path)?);
    ui::run(store);
    Ok(())
}
