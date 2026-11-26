//! Running the local models. Picacho's own runtime reads the small networks
//! `models/otp/train.py` writes; on macOS, Core ML runs the same network.
use anyhow::{Context, Result, ensure};
use std::path::Path;

/// Scores one sparse feature vector of (index, value): the probability the
/// model gives the positive class. Shared with the threads inference runs
/// on, away from the window's.
pub trait Scorer: Send + Sync {
    fn score(&self, x: &[(usize, f32)]) -> Result<f32>;
}

/// A bag of hashed features into one hidden layer and a logistic output,
/// read from the `PICAOTP1` format: dimensions and threshold, then the
/// weights, little-endian.
pub struct NativeModel {
    dim: usize,
    hidden: usize,
    pub threshold: f32,
    /// One row of `hidden` weights per feature.
    embedding: Vec<f32>,
    bias: Vec<f32>,
    output: Vec<f32>,
    output_bias: f32,
}

impl NativeModel {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| t!("manage-local-ai-runtime-couldnt-read", path = path.display()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 20 && &bytes[..8] == b"PICAOTP1",
            t!("manage-local-ai-runtime-not-a-picacho-model")
        );
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (dim, hidden) = (u32_at(8), u32_at(12));
        ensure!(
            dim.is_power_of_two() && dim <= 1 << 20 && (1..=256).contains(&hidden),
            t!("manage-local-ai-runtime-the-models-dimensions-are-out-of")
        );
        let floats = dim * hidden + 2 * hidden + 2;
        ensure!(
            bytes.len() == 16 + floats * 4,
            t!("manage-local-ai-runtime-the-model-file-is-incomplete")
        );
        let mut values = bytes[16..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c));
        let mut take = |n: usize| -> Vec<f32> { values.by_ref().take(n).collect() };
        let threshold = take(1)[0];
        let embedding = take(dim * hidden);
        let bias = take(hidden);
        let output = take(hidden);
        let output_bias = take(1)[0];
        ensure!(
            [threshold, output_bias]
                .iter()
                .chain(&embedding)
                .chain(&bias)
                .chain(&output)
                .all(|v| v.is_finite()),
            t!("manage-local-ai-runtime-the-model-has-invalid-weights")
        );
        Ok(Self {
            dim,
            hidden,
            threshold,
            embedding,
            bias,
            output,
            output_bias,
        })
    }
}

impl Scorer for NativeModel {
    fn score(&self, x: &[(usize, f32)]) -> Result<f32> {
        let mut h = self.bias.clone();
        for &(i, v) in x {
            ensure!(i < self.dim, "Feature out of range");
            let row = &self.embedding[i * self.hidden..(i + 1) * self.hidden];
            for (h, w) in h.iter_mut().zip(row) {
                *h += w * v;
            }
        }
        let z = self.output_bias
            + h.iter()
                .zip(&self.output)
                .map(|(h, w)| h.max(0.) * w)
                .sum::<f32>();
        Ok(1. / (1. + (-z).exp()))
    }
}

/// A token labeller: a bag of hashed features into one hidden layer and a
/// softmax over the labels, read from the `PICAWHN1` format: dimensions,
/// then a scale per embedding row, the embedding as 8-bit integers, and the
/// rest as 32-bit floats, little-endian. Small enough to run on any CPU for
/// every keystroke: a few thousand multiply-adds a token.
pub struct NativeTagger {
    dim: usize,
    hidden: usize,
    classes: usize,
    scale: Vec<f32>,
    /// One row of `hidden` weights per feature.
    embedding: Vec<i8>,
    bias: Vec<f32>,
    /// One row of `hidden` weights per label.
    output: Vec<f32>,
    output_bias: Vec<f32>,
}

impl NativeTagger {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| t!("manage-local-ai-runtime-couldnt-read", path = path.display()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 20 && &bytes[..8] == b"PICAWHN1",
            t!("manage-local-ai-runtime-not-a-picacho-tagger-model")
        );
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (dim, hidden, classes) = (u32_at(8), u32_at(12), u32_at(16));
        ensure!(
            dim.is_power_of_two()
                && dim <= 1 << 20
                && (1..=256).contains(&hidden)
                && (2..=1024).contains(&classes),
            t!("manage-local-ai-runtime-the-models-dimensions-are-out-of")
        );
        let floats = dim + hidden + classes * hidden + classes;
        ensure!(
            bytes.len() == 20 + dim * hidden + floats * 4,
            t!("manage-local-ai-runtime-the-model-file-is-incomplete")
        );
        let f32s = |from: usize, n: usize| -> Vec<f32> {
            bytes[from..from + n * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect()
        };
        let mut at = 20;
        let scale = f32s(at, dim);
        at += dim * 4;
        let embedding: Vec<i8> = bytes[at..at + dim * hidden]
            .iter()
            .map(|b| *b as i8)
            .collect();
        at += dim * hidden;
        let bias = f32s(at, hidden);
        at += hidden * 4;
        let output = f32s(at, classes * hidden);
        at += classes * hidden * 4;
        let output_bias = f32s(at, classes);
        ensure!(
            scale
                .iter()
                .chain(&bias)
                .chain(&output)
                .chain(&output_bias)
                .all(|v| v.is_finite()),
            t!("manage-local-ai-runtime-the-model-has-invalid-weights")
        );
        Ok(Self {
            dim,
            hidden,
            classes,
            scale,
            embedding,
            bias,
            output,
            output_bias,
        })
    }

    /// How many labels the model tells apart.
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// The probability of each label for one sparse feature vector of
    /// (index, value); features out of range are ignored.
    pub fn probabilities(&self, x: &[(usize, f32)]) -> Vec<f32> {
        let mut h = self.bias.clone();
        for &(i, v) in x {
            if i >= self.dim {
                continue;
            }
            let s = self.scale[i] * v;
            let row = &self.embedding[i * self.hidden..(i + 1) * self.hidden];
            for (h, w) in h.iter_mut().zip(row) {
                *h += f32::from(*w) * s;
            }
        }
        for h in &mut h {
            *h = h.max(0.);
        }
        let mut z: Vec<f32> = (0..self.classes)
            .map(|k| {
                self.output_bias[k]
                    + self.output[k * self.hidden..(k + 1) * self.hidden]
                        .iter()
                        .zip(&h)
                        .map(|(w, h)| w * h)
                        .sum::<f32>()
            })
            .collect();
        let max = z.iter().copied().fold(f32::MIN, f32::max);
        let mut total = 0.;
        for z in &mut z {
            *z = (*z - max).exp();
            total += *z;
        }
        for z in &mut z {
            *z /= total;
        }
        z
    }

    /// The most likely label and its probability.
    pub fn classify(&self, x: &[(usize, f32)]) -> (usize, f32) {
        self.probabilities(x)
            .into_iter()
            .enumerate()
            .fold(
                (0, f32::MIN),
                |best, (k, p)| if p > best.1 { (k, p) } else { best },
            )
    }
}

#[cfg(target_os = "macos")]
impl Scorer for super::coreml::CoreMlModel {
    fn score(&self, x: &[(usize, f32)]) -> Result<f32> {
        super::coreml::CoreMlModel::score(self, x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_files_that_are_not_whole_models() {
        assert!(NativeModel::parse(b"not a model").is_err());
        let mut bytes = b"PICAOTP1".to_vec();
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend([0u8; 4]);
        assert!(NativeModel::parse(&bytes).is_err(), "no weights");
        bytes.extend(std::iter::repeat_n(0u8, (4 * 2 + 2 * 2 + 1) * 4));
        let model = NativeModel::parse(&bytes).unwrap();
        assert_eq!(model.score(&[(1, 1.)]).unwrap(), 0.5);
        assert!(model.score(&[(9, 1.)]).is_err());
    }

    #[test]
    fn reads_whole_tagger_models_only() {
        assert!(NativeTagger::parse(b"PICAOTP1 not this one").is_err());
        let mut bytes = b"PICAWHN1".to_vec();
        for n in [4u32, 2, 3] {
            bytes.extend(n.to_le_bytes());
        }
        assert!(NativeTagger::parse(&bytes).is_err(), "no weights");
        // Scales, an embedding of 4 rows of 2, biases; one label favoured.
        for _ in 0..4 {
            bytes.extend(1f32.to_le_bytes());
        }
        bytes.extend([1u8, 0, 0, 1, 0, 0, 0, 0]);
        for v in [0f32, 0., 1., 0., 0., 1., 0., 0., 0., 0., 0.] {
            bytes.extend(v.to_le_bytes());
        }
        let model = NativeTagger::parse(&bytes).unwrap();
        let p = model.probabilities(&[(0, 5.)]);
        assert!((p.iter().sum::<f32>() - 1.).abs() < 1e-5);
        assert_eq!(model.classify(&[(0, 5.)]).0, 0);
        assert_eq!(model.classify(&[(1, 5.)]).0, 1);
        assert_eq!(model.classify(&[(99, 5.)]).1, model.classify(&[]).1);
    }
}
