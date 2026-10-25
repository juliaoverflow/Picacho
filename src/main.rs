mod assets;
mod mail;
mod model;
mod oauth;
mod store;
mod ui;

fn main() -> anyhow::Result<()> {
    let project = directories::ProjectDirs::from("com", "pinal", "PinalMail")
        .ok_or_else(|| anyhow::anyhow!("Cannot find the application data directory"))?;
    let path = std::env::var_os("PINAL_MAIL_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| project.data_dir().to_owned());
    let store = std::sync::Arc::new(store::Store::open(&path)?);
    ui::run(store);
    Ok(())
}
