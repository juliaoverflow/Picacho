//! Local AI: small models that run on this computer to label mail. Off by
//! default; nothing is downloaded until a feature that needs a model is
//! turned on in Settings → Local AI, and turning it off deletes the model.
//! Mail never leaves the computer for any of it.
//!
//! The features: hiding sign-in and verification codes (see `otp`), and
//! understanding dates and times typed where Picacho asks when ("tmrw at 6",
//! see `when`), confirming tracking numbers found in mail (see `parcel`),
//! and suggesting replies to received mail in its own language (see
//! `replies`).
#[cfg(target_os = "macos")]
mod coreml;
pub mod otp;
pub mod parcel;
pub mod replies;
pub mod runtime;
pub mod when;

use crate::manage::store::Store;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

/// Preference: Local AI is on ("true"/"false"; off when unset).
pub const ENABLED_PREFERENCE: &str = "local_ai";
/// Preference: sign-in codes are found and hidden.
pub const OTP_PREFERENCE: &str = "local_ai_otp";
/// Preference: typed dates and times are understood.
pub const WHEN_PREFERENCE: &str = "local_ai_when";
/// Preference: tracking numbers found in mail are confirmed by the model.
pub const PARCEL_PREFERENCE: &str = "local_ai_parcel";
/// Preference: replies are suggested under received mail.
pub const REPLIES_PREFERENCE: &str = "local_ai_replies";
/// Preference: the suggested replies chosen and turned away (JSON,
/// `replies::History`).
pub const REPLY_HISTORY_PREFERENCE: &str = "local_ai_reply_history";

/// Where the models are published: Picacho's model repositories on Hugging
/// Face (in the pinaltech "Picacho Models" collection), one per model. Each
/// file is checked against the size and SHA-256 below, so a changed file is
/// never installed. `PICACHO_MODELS_URL` points elsewhere (a mirror holding
/// every model's files, or a local server while developing).
const MODELS_URL: &str = "https://huggingface.co/pinaltech";

/// How a model is run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Scores one candidate at a time (Core ML on macOS).
    Scorer,
    /// Labels every token of a text, with Picacho's own runtime everywhere:
    /// it runs for every keystroke, where Core ML's overhead per call would
    /// cost more than the whole network.
    Tagger,
}

/// A downloadable model: its file there and here, and what it must be.
pub struct Model {
    /// Names the model in actions and settings.
    pub id: &'static str,
    pub label: &'static str,
    /// The features that need it, as Settings names them.
    pub features: &'static [&'static str],
    /// Its repository in the collection, and how it runs.
    repo: &'static str,
    pub kind: Kind,
    /// The published file.
    file: &'static str,
    /// Not published yet: installed from `local_dir` (the training
    /// directory in Picacho's source, where `train.py` writes it) rather
    /// than downloaded.
    local: bool,
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
    repo: "veil-otp",
    kind: Kind::Scorer,
    file: "otp-v1.mlmodel",
    local: false,
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
    repo: "veil-otp",
    kind: Kind::Scorer,
    file: "otp-v1.bin",
    local: false,
    installed: "otp-v1.bin",
    sha256: "7667b357195ca20d04e3505700ca5db72d335befbafc7e1dbe803eb765347bc6",
    size: 1_048_728,
    dim: otp::DIM,
    threshold: 0.3,
};

/// Epoch TBS, the date and time model (see `models/tbs/train.py`): the
/// same file on every platform.
pub const WHEN: Model = Model {
    id: "tbs",
    label: "Date and time model",
    features: &["Understand typed dates"],
    repo: "epoch-tbs",
    kind: Kind::Tagger,
    file: "tbs-v1.bin",
    local: false,
    installed: "tbs-v1.bin",
    sha256: "5035ed09e644033d7b53a7ea3dee558ba2099da874b0a5fc68baf2c032de0de5",
    size: 603_700,
    dim: when::DIM,
    threshold: 0.,
};

/// Vast Parcel, the tracking number model (see `models/parcel/train.py`):
/// a Core ML model on macOS, Picacho's own format elsewhere, the same
/// network either way. Until it is on Hugging Face, it is installed from
/// the files `train.py` writes in `models/parcel` (see `local_dir`).
#[cfg(target_os = "macos")]
pub const PARCEL: Model = PARCEL_COREML;
#[cfg(not(target_os = "macos"))]
pub const PARCEL: Model = PARCEL_NATIVE;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const PARCEL_COREML: Model = Model {
    id: "parcel",
    label: "Tracking number model",
    features: &["Find tracking numbers"],
    repo: "vast-parcel",
    kind: Kind::Scorer,
    file: "parcel-v1.mlmodel",
    local: false,
    installed: "parcel-v1.mlmodelc",
    sha256: "f48915c0952f34c7a844fb95acca333f243b858bd43ab1fb64f6b6c46efa7f2b",
    size: 524_674,
    dim: parcel::DIM,
    threshold: 0.5,
};
#[cfg_attr(target_os = "macos", allow(dead_code))]
const PARCEL_NATIVE: Model = Model {
    id: "parcel",
    label: "Tracking number model",
    features: &["Find tracking numbers"],
    repo: "vast-parcel",
    kind: Kind::Scorer,
    file: "parcel-v1.bin",
    local: false,
    installed: "parcel-v1.bin",
    sha256: "3afb6727202b9c5399adc18da800f9fb7d9f26ec1052aa9e757d3f4a54aa1585",
    size: 524_376,
    dim: parcel::DIM,
    threshold: 0.5,
};

/// Echo Replies, the reply suggestion model (see `models/replies/train.py`):
/// the same file on every platform, run by Picacho's own runtime. Until it
/// is on Hugging Face, it is installed from the file `train.py` writes in
/// `models/replies` (see `local_dir`).
pub const REPLIES: Model = Model {
    id: "replies",
    label: "Reply suggestion model",
    features: &["Suggest replies"],
    repo: "echo-replies",
    kind: Kind::Tagger,
    file: "replies-v1.bin",
    local: true,
    installed: "replies-v1.bin",
    sha256: "2e70213172b99b6537baf5a4b1c4594a5db1c6a2642c3e215ab0ffc144b27d2b",
    size: 1_187_716,
    dim: replies::DIM,
    threshold: 0.,
};

/// Every model Local AI can download.
pub const MODELS: [&Model; 4] = [&OTP, &WHEN, &PARCEL, &REPLIES];

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
/// Whether tracking numbers are confirmed by the model (Local AI on, and the
/// feature on).
pub fn parcel_enabled(store: &Store) -> bool {
    enabled(store) && store.preference(PARCEL_PREFERENCE).as_deref() == Some("true")
}
/// Whether replies are suggested (Local AI on, and the feature on).
pub fn replies_enabled(store: &Store) -> bool {
    enabled(store) && store.preference(REPLIES_PREFERENCE).as_deref() == Some("true")
}
/// The suggested replies chosen and turned away so far.
pub fn reply_history(store: &Store) -> replies::History {
    store
        .preference(REPLY_HISTORY_PREFERENCE)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
/// Whether typed dates are understood (Local AI on, and the feature on).
pub fn when_enabled(store: &Store) -> bool {
    enabled(store) && store.preference(WHEN_PREFERENCE).as_deref() == Some("true")
}

/// Where an unpublished model is installed from: `PICACHO_LOCAL_MODELS`
/// (a directory holding its files), or else its training directory in the
/// source Picacho was built from (`models/<id>`, where `train.py` writes
/// them).
pub fn local_dir(model: &Model) -> PathBuf {
    match std::env::var_os("PICACHO_LOCAL_MODELS") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join(model.id),
    }
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
        let bytes = match std::env::var("PICACHO_MODELS_URL") {
            Ok(base) => {
                let url = format!("{}/{}", base.trim_end_matches('/'), self.file);
                crate::sync::models::download(&url, self.size, received)?
            }
            Err(_) if self.local => self.read_local(received)?,
            Err(_) => crate::sync::models::download(&self.url(), self.size, received)?,
        };
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
        match self.kind {
            Kind::Scorer => self.load(store).map(drop),
            Kind::Tagger => self.load_tagger(store).map(drop),
        }
    }

    /// The unpublished model's file from `local_dir`.
    fn read_local(&self, received: &AtomicU64) -> Result<Vec<u8>> {
        let path = local_dir(self).join(self.file);
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "The {} isn't published yet, and {} isn't there to install it from \
                 (train it with models/{}/train.py)",
                self.label.to_lowercase(),
                path.display(),
                self.id
            )
        })?;
        received.store(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(bytes)
    }

    /// Where the model is published.
    fn url(&self) -> String {
        format!("{MODELS_URL}/{}/resolve/main/{}", self.repo, self.file)
    }

    #[cfg(target_os = "macos")]
    fn place(&self, downloaded: &std::path::Path, target: &std::path::Path) -> Result<()> {
        match self.kind {
            Kind::Scorer => coreml::compile(downloaded, target),
            Kind::Tagger => {
                runtime::NativeTagger::load(downloaded)?;
                std::fs::rename(downloaded, target)?;
                Ok(())
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    fn place(&self, downloaded: &std::path::Path, target: &std::path::Path) -> Result<()> {
        match self.kind {
            Kind::Scorer => drop(runtime::NativeModel::load(downloaded)?),
            Kind::Tagger => drop(runtime::NativeTagger::load(downloaded)?),
        }
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

    /// The installed labeller (a `Kind::Tagger` model).
    pub fn load_tagger(&self, store: &Store) -> Result<runtime::NativeTagger> {
        let path = self.path(store);
        ensure!(path.exists(), "{} isn't installed.", self.label);
        runtime::NativeTagger::load(&path)
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

/// The tracking number confirmer, with the installed model.
pub fn parcel_confirmer(store: &Store) -> Result<parcel::Confirmer> {
    Ok(parcel::Confirmer::new(
        PARCEL.load(store)?,
        PARCEL.threshold,
    ))
}

/// The reply suggester, with the installed model.
pub fn reply_suggester(store: &Store) -> Result<replies::Suggester> {
    replies::Suggester::new(REPLIES.load_tagger(store)?)
}

/// The date and time reader, with the installed model.
pub fn when_parser(store: &Store) -> Result<when::Parser> {
    Ok(when::Parser::new(WHEN.load_tagger(store)?))
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

/// A published model file for tests: the one `models/otp/train.py` just
/// wrote, if it is there and is the published one, else the published file,
/// downloaded once into `target/test-models` and checked like an install.
/// The model files aren't kept in git; they are published on Hugging Face.
#[cfg(test)]
pub(crate) fn test_model(file: &str) -> PathBuf {
    use std::sync::Mutex;
    static FETCHING: Mutex<()> = Mutex::new(());
    let model = [
        &OTP_NATIVE,
        &OTP_COREML,
        &WHEN,
        &PARCEL_NATIVE,
        &PARCEL_COREML,
    ]
    .into_iter()
    .find(|m| m.file == file)
    .unwrap_or_else(|| panic!("no published model is named {file}"));
    let published = |path: &std::path::Path| {
        std::fs::read(path).is_ok_and(|bytes| {
            bytes.len() as u64 == model.size && hex(&Sha256::digest(&bytes)) == model.sha256
        })
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let trained = root.join("models").join(model.id).join(file);
    if published(&trained) {
        return trained;
    }
    assert!(
        !model.local,
        "{} isn't published yet and the trained file isn't the one the app expects: \
         run models/{}/train.py --coreml and update its size and digest",
        trained.display(),
        model.id
    );
    let cached = root.join("target/test-models").join(file);
    let _fetching = FETCHING.lock().unwrap_or_else(|e| e.into_inner());
    if published(&cached) {
        return cached;
    }
    let url = model.url();
    let bytes = crate::sync::models::download(&url, model.size, &AtomicU64::new(0))
        .unwrap_or_else(|e| panic!("couldn't download {url} for the tests: {e:#}"));
    assert_eq!(
        (bytes.len() as u64, hex(&Sha256::digest(&bytes)).as_str()),
        (model.size, model.sha256),
        "{url} isn't the model the app expects: update its size and digest"
    );
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    let partial = cached.with_extension("partial");
    std::fs::write(&partial, &bytes).unwrap();
    std::fs::rename(&partial, &cached).unwrap();
    cached
}

/// The reply model `models/replies/train.py` wrote, while it isn't
/// published: None (and tests needing it skip, saying why) unless it is
/// there and is the one the app expects.
#[cfg(test)]
pub(crate) fn trained_replies() -> Option<PathBuf> {
    let path = local_dir(&REPLIES).join(REPLIES.file);
    match std::fs::read(&path) {
        Ok(bytes) if hex(&Sha256::digest(&bytes)) == REPLIES.sha256 => Some(path),
        _ => {
            eprintln!(
                "skipping: {} isn't the reply model the app expects (run models/replies/train.py \
                 and update REPLIES' size and digest)",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_models_are_the_ones_the_app_expects() {
        // The files on Hugging Face (or just trained) match the sizes and
        // digests here; `test_model` checks both, downloading if needed.
        for model in [OTP_NATIVE, OTP_COREML, WHEN, PARCEL_NATIVE, PARCEL_COREML] {
            let bytes = std::fs::read(test_model(model.file)).unwrap();
            assert_eq!(hex(&Sha256::digest(&bytes)), model.sha256, "{}", model.file);
            assert_eq!(bytes.len() as u64, model.size, "{}", model.file);
        }
        assert_eq!(OTP_NATIVE.threshold, OTP_COREML.threshold);
        runtime::NativeTagger::load(&test_model(WHEN.file)).unwrap();
    }

    /// Installs the date and time model as the app does, from Hugging
    /// Face, and reads a time with it. Needs the network:
    /// `cargo test installs_the_date_model -- --ignored`.
    #[test]
    #[ignore]
    fn installs_the_date_model_from_hugging_face() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        let received = AtomicU64::new(0);
        WHEN.install(&store, &received)?;
        assert!(WHEN.is_installed(&store));
        assert_eq!(
            received.load(std::sync::atomic::Ordering::Relaxed),
            WHEN.size
        );
        let parser = when_parser(&store)?;
        let now = chrono::Utc::now();
        assert!(parser.parse("tmrw at 6", &now).is_some());
        assert!(parser.parse("übermorgen um 18 uhr", &now).is_some());
        assert!(parser.parse("hello there", &now).is_none());
        WHEN.remove(&store)?;
        drop(store);
        std::fs::remove_dir_all(p)?;
        Ok(())
    }

    #[test]
    fn models_are_installed_loaded_and_removed_in_the_data_directory() -> Result<()> {
        let p = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&p)?;
        assert!(!OTP.is_installed(&store) && OTP.load(&store).is_err());
        // As install leaves it, without the download.
        std::fs::create_dir_all(models_dir(&store))?;
        std::fs::copy(
            test_model("otp-v1.bin"),
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
            test_model("otp-v1.bin"),
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
