//! Running the local models. Picacho's own runtime reads the small networks
//! `models/otp/train.py` writes; on macOS, Core ML runs the same network.
use anyhow::{Context, Result, ensure};
use std::path::Path;

/// Scores one sparse feature vector of (index, value): the probability the
/// model gives the positive class.
pub trait Scorer {
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
            std::fs::read(path).with_context(|| format!("Couldn't read {}", path.display()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 20 && &bytes[..8] == b"PICAOTP1",
            "Not a Picacho model"
        );
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (dim, hidden) = (u32_at(8), u32_at(12));
        ensure!(
            dim.is_power_of_two() && dim <= 1 << 20 && (1..=256).contains(&hidden),
            "The model's dimensions are out of range"
        );
        let floats = dim * hidden + 2 * hidden + 2;
        ensure!(
            bytes.len() == 16 + floats * 4,
            "The model file is incomplete"
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
            "The model has invalid weights"
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
}
