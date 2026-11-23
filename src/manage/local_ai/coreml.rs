//! Core ML runtime for the local models on Apple platforms: the model is
//! downloaded as a `.mlmodel`, compiled for this Mac once when installed,
//! and run by Core ML, which picks the CPU, GPU or Neural Engine itself.
use anyhow::{Context, Result, anyhow, ensure};
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_core_ml::{
    MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel, MLMultiArray,
    MLMultiArrayDataType,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL};
use std::path::Path;

fn error(e: Retained<NSError>) -> anyhow::Error {
    anyhow!("{}", e.localizedDescription())
}

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

/// Compiles the `.mlmodel` at `source` into the model directory `target`
/// (a `.mlmodelc`), replacing any there.
pub fn compile(source: &Path, target: &Path) -> Result<()> {
    // The synchronous call is deprecated in favour of a completion handler,
    // but installing already runs off the main thread.
    let path = autoreleasepool(|_| {
        #[allow(deprecated)]
        let compiled = unsafe { MLModel::compileModelAtURL_error(&url(source)) }.map_err(error)?;
        compiled
            .path()
            .map(|p| p.to_string())
            .context("Core ML gave no compiled model")
    })?;
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    // The compiled model is in a temporary directory, maybe on another volume.
    if std::fs::rename(&path, target).is_err() {
        copy_dir(Path::new(&path), target)?;
        let _ = std::fs::remove_dir_all(&path);
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// A compiled model taking a `features` vector of `dim` floats and giving a
/// `probability`.
pub struct CoreMlModel {
    model: Retained<MLModel>,
    dim: usize,
}

impl CoreMlModel {
    pub fn load(compiled: &Path, dim: usize) -> Result<Self> {
        let model = autoreleasepool(|_| {
            unsafe { MLModel::modelWithContentsOfURL_error(&url(compiled)) }.map_err(error)
        })?;
        Ok(Self { model, dim })
    }

    /// The probability for one sparse feature vector of (index, value).
    ///
    /// Core ML autoreleases what a prediction makes (its input and output
    /// arrays among them). Picacho's task threads have no autorelease pool
    /// that is ever drained, so without one here every score would keep its
    /// arrays until the thread ended: tens of kilobytes each, for every code
    /// candidate in every message shown.
    pub fn score(&self, x: &[(usize, f32)]) -> Result<f32> {
        autoreleasepool(|_| self.score_in_pool(x))
    }

    fn score_in_pool(&self, x: &[(usize, f32)]) -> Result<f32> {
        unsafe {
            let shape = NSArray::from_retained_slice(&[NSNumber::new_usize(self.dim)]);
            let input = MLMultiArray::initWithShape_dataType_error(
                MLMultiArray::alloc(),
                &shape,
                MLMultiArrayDataType::Float32,
            )
            .map_err(error)?;
            ensure!(
                input.count() as usize == self.dim,
                "Core ML input has the wrong size"
            );
            // A fresh array is uninitialized: zero it, then set the features.
            #[allow(deprecated)]
            let data = std::slice::from_raw_parts_mut(
                input.dataPointer().as_ptr().cast::<f32>(),
                self.dim,
            );
            data.fill(0.);
            for &(i, v) in x {
                if let Some(slot) = data.get_mut(i) {
                    *slot = v;
                }
            }
            let value = MLFeatureValue::featureValueWithMultiArray(&input);
            let key = NSString::from_str("features");
            let dict: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_retained_objects(
                    &[&*key],
                    &[Retained::into_super(Retained::into_super(value))],
                );
            let provider = MLDictionaryFeatureProvider::initWithDictionary_error(
                MLDictionaryFeatureProvider::alloc(),
                &dict,
            )
            .map_err(error)?;
            let output = self
                .model
                .predictionFromFeatures_error(ProtocolObject::from_ref(&*provider))
                .map_err(error)?;
            let probability = output
                .featureValueForName(&NSString::from_str("probability"))
                .and_then(|v| v.multiArrayValue())
                .context("Core ML gave no probability")?;
            // Indexing an empty array raises an Objective-C exception, which
            // can't be caught here: it would end the app.
            ensure!(probability.count() > 0, "Core ML gave no probability");
            Ok(probability.objectAtIndexedSubscript(0).floatValue())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resident_kb() -> u64 {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    /// Scoring runs on threads without an autorelease pool of their own
    /// (Picacho's task threads); what Core ML autoreleases must not pile up.
    ///
    /// Memory is measured for the whole process, so the test runs itself
    /// again alone, in a process of its own, where other tests running at
    /// the same time can't add to it.
    #[test]
    fn scoring_does_not_grow_memory() -> Result<()> {
        const ALONE: &str = "PICACHO_COREML_MEMORY_TEST";
        if std::env::var_os(ALONE).is_none() {
            let out = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "manage::local_ai::coreml::tests::scoring_does_not_grow_memory",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(ALONE, "1")
                .output()?;
            let log = String::from_utf8_lossy(&out.stderr);
            ensure!(out.status.success(), "the measurement failed:\n{log}");
            ensure!(
                log.contains("resident memory grew"),
                "it didn't run:\n{log}"
            );
            return Ok(());
        }
        let dir = std::env::temp_dir().join(format!("coreml-{}", uuid::Uuid::new_v4()));
        let source = super::super::test_model("otp-v1.mlmodel");
        let compiled = dir.join("otp.mlmodelc");
        std::fs::create_dir_all(&dir)?;
        compile(&source, &compiled)?;
        let grown = std::thread::spawn(move || -> Result<u64> {
            let model = CoreMlModel::load(&compiled, super::super::otp::DIM)?;
            let x = [(1, 1.0), (7, 0.5)];
            for _ in 0..2_000 {
                model.score(&x)?;
            }
            let before = resident_kb();
            for _ in 0..20_000 {
                model.score(&x)?;
            }
            Ok(resident_kb().saturating_sub(before))
        })
        .join()
        .unwrap()?;
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("resident memory grew {grown} KB over 20,000 scores");
        assert!(grown < 20 * 1024, "grew {grown} KB");
        Ok(())
    }
}
