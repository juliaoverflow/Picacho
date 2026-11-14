//! Core ML runtime for the local models on Apple platforms: the model is
//! downloaded as a `.mlmodel`, compiled for this Mac once when installed,
//! and run by Core ML, which picks the CPU, GPU or Neural Engine itself.
use anyhow::{Context, Result, anyhow, ensure};
use objc2::AnyThread;
use objc2::rc::Retained;
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
    #[allow(deprecated)]
    let compiled = unsafe { MLModel::compileModelAtURL_error(&url(source)) }.map_err(error)?;
    let path = compiled
        .path()
        .map(|p| p.to_string())
        .context("Core ML gave no compiled model")?;
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
        let model =
            unsafe { MLModel::modelWithContentsOfURL_error(&url(compiled)) }.map_err(error)?;
        Ok(Self { model, dim })
    }

    /// The probability for one sparse feature vector of (index, value).
    pub fn score(&self, x: &[(usize, f32)]) -> Result<f32> {
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
            Ok(probability.objectAtIndexedSubscript(0).floatValue())
        }
    }
}
