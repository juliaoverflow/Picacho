//! Account pictures: one uploaded by the person, a bundled preset, or the
//! one their provider has (Google for Gmail sign-ins, otherwise Gravatar).
//! Pictures are re-encoded as small PNGs in the data directory.
use super::backend::Backend;
use super::model::{Account, Provider};
use anyhow::{Context, Result};
use std::path::PathBuf;

const SIZE: u32 = 256;

fn key(id: &str) -> String {
    format!("avatar:{id}")
}
fn source_key(id: &str) -> String {
    format!("avatar_source:{id}")
}

impl Backend {
    /// The account's current picture. Each saved picture has a new file name,
    /// so image caches never show an older one.
    pub fn avatar(&self, id: &str) -> Option<PathBuf> {
        let name = self.store.preference(&key(id))?;
        let path = self.store.path.join("avatars").join(name);
        path.is_file().then_some(path)
    }
    /// Decodes, crops to a square and saves a picture for the account.
    /// `source` records where it came from ("custom", "preset", "google",
    /// "gravatar"); any but "google" stops the Google picture replacing it.
    pub fn set_avatar(&self, id: &str, bytes: &[u8], source: &str) -> Result<PathBuf> {
        let image = image::load_from_memory(bytes).context("This image could not be read.")?;
        let square = image.resize_to_fill(SIZE, SIZE, image::imageops::FilterType::Lanczos3);
        let dir = self.store.path.join("avatars");
        std::fs::create_dir_all(&dir)?;
        let name = format!("{id}-{}.png", uuid::Uuid::new_v4().simple());
        let path = dir.join(&name);
        square.save_with_format(&path, image::ImageFormat::Png)?;
        let old = self.avatar(id);
        self.store.set_preference(&key(id), &name)?;
        self.store.set_preference(&source_key(id), source)?;
        if let Some(old) = old {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }
    /// Forgets the account's picture; its initials show again.
    pub fn remove_avatar(&self, id: &str) -> Result<()> {
        if let Some(old) = self.avatar(id) {
            let _ = std::fs::remove_file(old);
        }
        self.store.set_preference(&key(id), "")?;
        // A removed picture is not replaced by the Google one again.
        self.store.set_preference(&source_key(id), "none")
    }
    /// Saves the Google profile picture of a Gmail account signed in through
    /// the browser, unless the person chose or removed a picture.
    pub fn fetch_google_avatar(&self, a: &Account) -> Result<Option<PathBuf>> {
        if a.provider != Provider::Gmail || !a.oauth {
            return Ok(None);
        }
        if self.store.preference(&source_key(&a.id)).is_some() {
            return Ok(self.avatar(&a.id));
        }
        let secret = {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            self.credentials(a)?
        };
        match crate::sync::oauth::google_picture(&secret.access_token)? {
            Some(bytes) => self.set_avatar(&a.id, &bytes, "google").map(Some),
            None => {
                self.store.set_preference(&source_key(&a.id), "none")?;
                Ok(None)
            }
        }
    }
    /// Asks the account's provider for its picture now, replacing any other:
    /// Google's for Gmail accounts signed in through the browser, otherwise
    /// Gravatar's for the address. `None` when there is none.
    pub fn provider_avatar(&self, a: &Account) -> Result<Option<PathBuf>> {
        let (bytes, source) = if a.provider == Provider::Gmail && a.oauth {
            let secret = {
                let lock = self.account_lock(&a.id);
                let _operation = lock.lock().unwrap();
                self.credentials(a)?
            };
            (
                crate::sync::oauth::google_picture(&secret.access_token)?,
                "google",
            )
        } else {
            (crate::sync::oauth::gravatar(&a.email)?, "gravatar")
        };
        bytes
            .map(|bytes| self.set_avatar(&a.id, &bytes, source))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::store::Store;
    use std::sync::Arc;

    #[test]
    fn uploaded_pictures_are_square_pngs_with_fresh_names() -> Result<()> {
        let path = std::env::temp_dir().join(format!("picacho-avatar-{}", uuid::Uuid::new_v4()));
        let backend = Backend::new(Arc::new(Store::open(&path)?));
        let mut png = Vec::new();
        image::RgbImage::from_pixel(40, 20, image::Rgb([200, 80, 40]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        let first = backend.set_avatar("a", &png, "custom")?;
        let second = backend.set_avatar("a", &png, "custom")?;
        assert_ne!(first, second);
        assert!(!first.exists(), "the replaced picture is removed");
        assert_eq!(backend.avatar("a"), Some(second.clone()));
        assert_eq!(
            backend.store.preference(&source_key("a")).as_deref(),
            Some("custom")
        );
        assert_eq!(image::image_dimensions(&second)?, (SIZE, SIZE));
        assert!(backend.set_avatar("a", b"not an image", "custom").is_err());
        backend.remove_avatar("a")?;
        assert_eq!(backend.avatar("a"), None);
        std::fs::remove_dir_all(path)?;
        Ok(())
    }
}
