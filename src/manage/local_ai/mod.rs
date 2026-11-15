//! Local AI: small models that run on this computer to label mail. Off by
//! default; nothing is downloaded until a feature that needs a model is
//! turned on in Settings → Local AI, and turning it off deletes the model.
//! Mail never leaves the computer for any of it.
//!
//! The first feature hides sign-in and verification codes (see `otp`).
#[cfg(target_os = "macos")]
mod coreml;
pub mod otp;
pub mod runtime;

use crate::manage::store::Store;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

/// Preference: Local AI is on ("true"/"false"; off when unset).
pub const ENABLED_PREFERENCE: &str = "local_ai";
/// Preference: sign-in codes are found and hidden.
pub const OTP_PREFERENCE: &str = "local_ai_otp";

/// Where the models are published: Picacho's model repository on Hugging
/// Face (in the pinaltech "Picacho Models" collection). Each file is checked
/// against the size and SHA-256 below, so a changed file is never installed.
/// `PICACHO_MODELS_URL` points elsewhere (a mirror, or a local server while
/// developing).
const MODELS_URL: &str = "https://huggingface.co/pinaltech/picacho-otp/resolve/main";

/// A downloadable model: its file there and here, and what it must be.
pub struct Model {
    /// Names the model in actions and settings.
    pub id: &'static str,
    pub label: &'static str,
    /// The features that need it, as Settings names them.
    pub features: &'static [&'static str],
    /// The published file.
    file: &'static str,
    /// What is kept in the models directory once installed: the file
    /// itself, or (Core ML) the model compiled from it.
    installed: &'static str,
    sha256: &'static str,
    pub size: u64,
    /// The features the model takes (Core ML needs the size), and the
    /// probability from which a candidate counts.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    dim: usize,
    pub threshold: f32,
}

/// The sign-in code model: a Core ML model on macOS, Picacho's own format
/// elsewhere. The same network either way (see `models/otp/train.py`).
#[cfg(target_os = "macos")]
pub const OTP: Model = OTP_COREML;
#[cfg(not(target_os = "macos"))]
pub const OTP: Model = OTP_NATIVE;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const OTP_COREML: Model = Model {
    id: "otp",
    label: "Sign-in code model",
    features: &["Hide sign-in codes"],
    file: "otp-v1.mlmodel",
    installed: "otp-v1.mlmodelc",
    sha256: "309484159aa766e287f9a0bffcc0f2d990ba524eaaed374ab63b25b3b0194c8a",
    size: 1_049_025,
    dim: otp::DIM,
    threshold: 0.3,
};
#[cfg_attr(target_os = "macos", allow(dead_code))]
const OTP_NATIVE: Model = Model {
    id: "otp",
    label: "Sign-in code model",
    features: &["Hide sign-in codes"],
    file: "otp-v1.bin",
    installed: "otp-v1.bin",
    sha256: "7667b357195ca20d04e3505700ca5db72d335befbafc7e1dbe803eb765347bc6",
    size: 1_048_728,
    dim: otp::DIM,
    threshold: 0.3,
};

/// Every model Local AI can download.
pub const MODELS: [&Model; 1] = [&OTP];

/// The model with this id.
pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.into_iter().find(|m| m.id == id)
}

/// Whether Local AI is turned on.
pub fn enabled(store: &Store) -> bool {
    store.preference(ENABLED_PREFERENCE).as_deref() == Some("true")
}
/// Whether sign-in codes are to be hidden (Local AI on, and the feature on).
pub fn otp_enabled(store: &Store) -> bool {
    enabled(store) && store.preference(OTP_PREFERENCE).as_deref() == Some("true")
}

/// The directory the models are kept in.
pub fn models_dir(store: &Store) -> PathBuf {
    store.path.join("models")
}

impl Model {
    fn path(&self, store: &Store) -> PathBuf {
        models_dir(store).join(self.installed)
    }

    /// Whether the model is on this computer.
    pub fn is_installed(&self, store: &Store) -> bool {
        self.path(store).exists()
    }

    /// Downloads the model, checks it is exactly the published file, and
    /// keeps it (compiled for Core ML on macOS). `received` counts the bytes
    /// downloaded so far. Blocks; run off the main thread.
    pub fn install(&self, store: &Store, received: &AtomicU64) -> Result<()> {
        let base = std::env::var("PICACHO_MODELS_URL").unwrap_or_else(|_| MODELS_URL.into());
        let url = format!("{}/{}", base.trim_end_matches('/'), self.file);
        let bytes = crate::sync::models::download(&url, self.size, received)?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        ensure!(
            bytes.len() as u64 == self.size && digest == self.sha256,
            "The downloaded model isn't the published one, so it wasn't installed."
        );
        let dir = models_dir(store);
        std::fs::create_dir_all(&dir)?;
        let partial = dir.join(format!("{}.partial", self.file));
        std::fs::write(&partial, &bytes)?;
        let result = self.place(&partial, &self.path(store));
        let _ = std::fs::remove_file(&partial);
        result?;
        // What was installed has to load.
        self.load(store).map(drop)
    }

    #[cfg(target_os = "macos")]
    fn place(&self, downloaded: &std::path::Path, target: &std::path::Path) -> Result<()> {
        coreml::compile(downloaded, target)
    }
    #[cfg(not(target_os = "macos"))]
    fn place(&self, downloaded: &std::path::Path, target: &std::path::Path) -> Result<()> {
        runtime::NativeModel::load(downloaded)?;
        std::fs::rename(downloaded, target)?;
        Ok(())
    }

    /// The installed model, ready to score.
    pub fn load(&self, store: &Store) -> Result<Box<dyn runtime::Scorer>> {
        let path = self.path(store);
        ensure!(path.exists(), "{} isn't installed.", self.label);
        #[cfg(target_os = "macos")]
        let scorer: Box<dyn runtime::Scorer> = Box::new(
            coreml::CoreMlModel::load(&path, self.dim)
                .context("Core ML couldn't load the model")?,
        );
        #[cfg(not(target_os = "macos"))]
        let scorer: Box<dyn runtime::Scorer> = {
            let model = runtime::NativeModel::load(&path)?;
            ensure!(model.threshold.is_finite(), "The model has no threshold");
            Box::new(model)
        };
        Ok(scorer)
    }

    /// The space the installed model takes, in bytes (0 when it isn't).
    pub fn installed_size(&self, store: &Store) -> u64 {
        size_of(&self.path(store))
    }

    /// Deletes the model from this computer.
    pub fn remove(&self, store: &Store) -> Result<()> {
        let path = self.path(store);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else if path.exists() {
            std::fs::remove_file(&path)
        } else {
            Ok(())
        }
        .with_context(|| format!("Couldn't delete {}", path.display()))
    }
}

/// The sign-in code detector, with the installed model.
pub fn otp_detector(store: &Store) -> Result<otp::Detector> {
    Ok(otp::Detector::new(OTP.load(store)?, OTP.threshold))
}

/// Deletes what isn't a current model: one left from an earlier version
/// (an `otp-v1.bin` once a later one is used), or a download that never
/// finished.
pub fn remove_stale(store: &Store) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(models_dir(store)) else {
        return Ok(());
    };
    let current: Vec<&str> = MODELS.iter().map(|m| m.installed).collect();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if current.iter().any(|c| name.to_str() == Some(c)) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .with_context(|| format!("Couldn't delete {}", path.display()))?;
    }
    Ok(())
}

/// Deletes every installed model.
pub fn remove_all(store: &Store) -> Result<()> {
    let dir = models_dir(store);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Couldn't delete {}", dir.display()))?;
    }
    Ok(())
}

/// The space the installed models take, in bytes.
pub fn installed_size(store: &Store) -> u64 {
    size_of(&models_dir(store))
}

/// The size of a file, or of everything in a directory.
fn size_of(path: &std::path::Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| size_of(&e.path()))
            .sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_models_are_the_trained_ones() {
        // The models in the repository are what gets published; their
        // digests here have to follow them when they are retrained.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/otp");
        for model in [OTP_NATIVE, OTP_COREML] {
            let bytes = std::fs::read(dir.join(model.file)).unwrap();
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(digest, model.sha256, "{}", model.file);
            assert_eq!(bytes.len() as u64, model.size, "{}", model.file);
            assert_eq!(model.threshold, OTP.threshold);
        }
    }

    #[test]
    fn models_are_installed_loaded_and_removed_in_the_data_directory() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        assert!(!OTP.is_installed(&store) && OTP.load(&store).is_err());
        // As install leaves it, without the download.
        std::fs::create_dir_all(models_dir(&store))?;
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/otp/otp-v1.bin"),
            models_dir(&store).join("otp-v1.bin"),
        )?;
        #[cfg(not(target_os = "macos"))]
        {
            assert!(OTP.is_installed(&store));
            assert_eq!(OTP.installed_size(&store), OTP.size);
            assert_eq!(model("otp").unwrap().id, OTP.id);
            assert!(model("nope").is_none());
            let detector = otp_detector(&store)?;
            assert_eq!(
                detector.codes("Your code", "Your verification code is 482913."),
                ["482913"]
            );
            OTP.remove(&store)?;
            assert!(!OTP.is_installed(&store));
        }
        // An older version's file and an unfinished download go; the
        // current model stays.
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/otp/otp-v1.bin"),
            models_dir(&store).join(OTP.installed),
        )?;
        std::fs::write(models_dir(&store).join("otp-v0.bin"), b"old")?;
        std::fs::write(models_dir(&store).join("otp-v1.bin.partial"), b"half")?;
        remove_stale(&store)?;
        assert!(!models_dir(&store).join("otp-v0.bin").exists());
        assert!(!models_dir(&store).join("otp-v1.bin.partial").exists());
        assert!(OTP.is_installed(&store) || cfg!(target_os = "macos"));
        assert!(installed_size(&store) > 0 || cfg!(not(target_os = "macos")));
        remove_all(&store)?;
        assert!(!models_dir(&store).exists());
        assert_eq!(installed_size(&store), 0);
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }
}
