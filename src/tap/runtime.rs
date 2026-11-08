//! Safe-Rust High-Performance Tensor Inference Runtime for Tap.
//!
//! Executes non-autoregressive decision projections, categorical classifications,
//! scalar scores, and heuristic hypothesis comparisons without a model service.

use crate::error::{Error, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

/// Magic bytes for Tap serialized models: "TAP1"
pub const TAP_MODEL_MAGIC: [u8; 4] = *b"TAP1";

/// In-memory supplied or deterministic baseline weights for Tap decision projections.
#[derive(Debug, Clone)]
pub struct TapWeights {
    /// Dimension of the projection vectors (e.g. 64 or 128)
    pub dim: usize,
    /// Vocabulary size
    pub vocab_size: usize,
    /// Token embedding table (flattened vocab_size * dim)
    pub embedding_table: Vec<f32>,
    /// Dense linear projection weights for alignment (dim * dim)
    pub projection_matrix: Vec<f32>,
    /// Bias vector for projection (dim)
    pub projection_bias: Vec<f32>,
    /// Temperature for score normalization; calibration requires external evaluation.
    pub temperature: f32,
}

impl TapWeights {
    /// Maximum serialized model size accepted by the loader (128 MiB).
    const MAX_MODEL_BYTES: u64 = 128 * 1024 * 1024;

    /// Generates a deterministic baseline weight set. These weights are not trained.
    pub fn default_calibrated(dim: usize, vocab_size: usize) -> Self {
        let mut embedding_table = Vec::with_capacity(vocab_size * dim);

        // Deterministic pseudo-random generation using golden ratio hashing
        for i in 0..(vocab_size * dim) {
            let h = (i as u64)
                .wrapping_mul(0x9E3779B97F4A7C15)
                .wrapping_add(0xBF58476D1CE4E5B9);
            let normalized = ((h >> 32) as u32 as f32) / (u32::MAX as f32);
            embedding_table.push((normalized - 0.5) * 0.2);
        }

        let mut projection_matrix = vec![0.0; dim * dim];
        for i in 0..dim {
            projection_matrix[i * dim + i] = 1.0; // Identity initialization
        }

        Self {
            dim,
            vocab_size,
            embedding_table,
            projection_matrix,
            projection_bias: vec![0.0; dim],
            temperature: 0.85,
        }
    }

    /// Load bounded, finite weights from a binary `.tapmodel` file.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path).map_err(Error::Io)?;
        let file_len = file.metadata().map_err(Error::Io)?.len();
        if !(16..=Self::MAX_MODEL_BYTES).contains(&file_len) {
            return Err(Error::Internal(
                "Tap model size is invalid or exceeds 128 MiB".into(),
            ));
        }
        let mut file = BufReader::new(file);
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).map_err(Error::Io)?;
        if magic != TAP_MODEL_MAGIC {
            return Err(Error::Internal("Invalid Tap model magic header".into()));
        }

        let mut buf_u32 = [0u8; 4];
        file.read_exact(&mut buf_u32).map_err(Error::Io)?;
        let dim = u32::from_le_bytes(buf_u32) as usize;

        file.read_exact(&mut buf_u32).map_err(Error::Io)?;
        let vocab_size = u32::from_le_bytes(buf_u32) as usize;

        if dim == 0 || vocab_size == 0 {
            return Err(Error::Internal(
                "Tap model dimensions must be positive".into(),
            ));
        }

        let emb_len = vocab_size
            .checked_mul(dim)
            .ok_or_else(|| Error::Internal("Tap model dimensions overflow".into()))?;
        let proj_len = dim
            .checked_mul(dim)
            .ok_or_else(|| Error::Internal("Tap model dimensions overflow".into()))?;
        let float_count = emb_len
            .checked_add(proj_len)
            .and_then(|n| n.checked_add(dim))
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| Error::Internal("Tap model dimensions overflow".into()))?;
        let expected_len = u64::try_from(float_count)
            .ok()
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| n.checked_add(12))
            .ok_or_else(|| Error::Internal("Tap model dimensions overflow".into()))?;
        if file_len != expected_len {
            return Err(Error::Internal(
                "Tap model file length does not match its dimensions".into(),
            ));
        }

        let mut embedding_table = Vec::with_capacity(emb_len);
        for _ in 0..emb_len {
            file.read_exact(&mut buf_u32).map_err(Error::Io)?;
            let value = f32::from_le_bytes(buf_u32);
            if !value.is_finite() {
                return Err(Error::Internal(
                    "Tap model contains a non-finite weight".into(),
                ));
            }
            embedding_table.push(value);
        }

        let mut projection_matrix = Vec::with_capacity(proj_len);
        for _ in 0..proj_len {
            file.read_exact(&mut buf_u32).map_err(Error::Io)?;
            let value = f32::from_le_bytes(buf_u32);
            if !value.is_finite() {
                return Err(Error::Internal(
                    "Tap model contains a non-finite weight".into(),
                ));
            }
            projection_matrix.push(value);
        }

        let mut projection_bias = Vec::with_capacity(dim);
        for _ in 0..dim {
            file.read_exact(&mut buf_u32).map_err(Error::Io)?;
            let value = f32::from_le_bytes(buf_u32);
            if !value.is_finite() {
                return Err(Error::Internal(
                    "Tap model contains a non-finite weight".into(),
                ));
            }
            projection_bias.push(value);
        }

        file.read_exact(&mut buf_u32).map_err(Error::Io)?;
        let temperature = f32::from_le_bytes(buf_u32);
        if !temperature.is_finite() || temperature <= 0.0 {
            return Err(Error::Internal(
                "Tap model temperature must be finite and positive".into(),
            ));
        }

        Ok(Self {
            dim,
            vocab_size,
            embedding_table,
            projection_matrix,
            projection_bias,
            temperature,
        })
    }

    /// Save weights to a binary `.tapmodel` file
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let dim = u32::try_from(self.dim)
            .map_err(|_| Error::Internal("Tap model dimension exceeds u32".into()))?;
        let vocab_size = u32::try_from(self.vocab_size)
            .map_err(|_| Error::Internal("Tap model vocabulary exceeds u32".into()))?;
        let valid = dim > 0
            && vocab_size > 0
            && self.embedding_table.len() == self.dim.saturating_mul(self.vocab_size)
            && self.projection_matrix.len() == self.dim.saturating_mul(self.dim)
            && self.projection_bias.len() == self.dim
            && self.temperature.is_finite()
            && self.temperature > 0.0
            && self
                .embedding_table
                .iter()
                .chain(&self.projection_matrix)
                .chain(&self.projection_bias)
                .all(|value| value.is_finite());
        if !valid {
            return Err(Error::Internal("Tap model weights are invalid".into()));
        }
        let serialized_bytes = 12u64
            + 4u64
                * (self.embedding_table.len()
                    + self.projection_matrix.len()
                    + self.projection_bias.len()
                    + 1) as u64;
        if serialized_bytes > Self::MAX_MODEL_BYTES {
            return Err(Error::Internal("Tap model exceeds 128 MiB".into()));
        }
        let mut file = BufWriter::new(File::create(path).map_err(Error::Io)?);
        file.write_all(&TAP_MODEL_MAGIC).map_err(Error::Io)?;
        file.write_all(&dim.to_le_bytes()).map_err(Error::Io)?;
        file.write_all(&vocab_size.to_le_bytes())
            .map_err(Error::Io)?;

        for &val in &self.embedding_table {
            file.write_all(&val.to_le_bytes()).map_err(Error::Io)?;
        }
        for &val in &self.projection_matrix {
            file.write_all(&val.to_le_bytes()).map_err(Error::Io)?;
        }
        for &val in &self.projection_bias {
            file.write_all(&val.to_le_bytes()).map_err(Error::Io)?;
        }
        file.write_all(&self.temperature.to_le_bytes())
            .map_err(Error::Io)?;

        file.flush().map_err(Error::Io)?;
        Ok(())
    }
}

/// Runtime engine responsible for bare-metal vector evaluation
#[derive(Debug, Clone)]
pub struct TapRuntime {
    weights: TapWeights,
}

/// Alias for TapRuntime
pub type TapInferenceEngine = TapRuntime;

impl TapRuntime {
    /// Initialize with given weights
    pub fn new(weights: TapWeights) -> Self {
        Self { weights }
    }

    /// Dimension of model vectors
    pub fn dim(&self) -> usize {
        self.weights.dim
    }

    /// Project a list of token IDs into a normalized semantic state vector
    pub fn forward_pool(&self, token_ids: &[u32]) -> Vec<f32> {
        let dim = self.weights.dim;
        let mut pooled = vec![0.0; dim];
        if token_ids.is_empty() {
            return pooled;
        }

        let mut count = 0.0f32;
        for &tid in token_ids {
            let idx = (tid as usize) % self.weights.vocab_size;
            let offset = idx * dim;
            let emb_slice = &self.weights.embedding_table[offset..offset + dim];
            for (value, component) in pooled.iter_mut().zip(emb_slice) {
                *value += component;
            }
            count += 1.0;
        }

        if count > 0.0 {
            for value in &mut pooled {
                *value /= count;
            }
        }

        // Apply linear projection layer: y = W * x + b
        let mut projected = vec![0.0; dim];
        for (i, value) in projected.iter_mut().enumerate() {
            let mut sum = self.weights.projection_bias[i];
            let row_offset = i * dim;
            for (j, &component) in pooled.iter().enumerate() {
                sum += self.weights.projection_matrix[row_offset + j] * component;
            }
            *value = sum;
        }

        // L2 Normalize
        normalize_l2(&mut projected);
        projected
    }

    /// Compute cosine similarity between two normalized vectors
    pub fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0f32;
        let len = a.len().min(b.len());
        for i in 0..len {
            dot += a[i] * b[i];
        }
        dot.clamp(-1.0, 1.0)
    }

    /// Softmax with temperature scaling; output is not calibrated by default.
    pub fn softmax(&self, logits: &[f32]) -> Vec<f32> {
        if logits.is_empty() {
            return Vec::new();
        }
        let temp = if self.weights.temperature > 0.01 {
            self.weights.temperature
        } else {
            1.0
        };

        let mut max_val = f32::NEG_INFINITY;
        for &l in logits {
            let scaled = l / temp;
            if scaled > max_val {
                max_val = scaled;
            }
        }

        let mut exps = Vec::with_capacity(logits.len());
        let mut sum_exp = 0.0f32;
        for &l in logits {
            let e = ((l / temp) - max_val).exp();
            exps.push(e);
            sum_exp += e;
        }

        if sum_exp > 0.0 {
            for v in &mut exps {
                *v /= sum_exp;
            }
        }
        exps
    }

    /// Sigmoid activation for scalar score normalization.
    pub fn sigmoid(&self, val: f32) -> f32 {
        1.0 / (1.0 + (-val).exp())
    }
}

fn normalize_l2(v: &mut [f32]) {
    let mut sum_sq = 0.0f32;
    for &x in v.iter() {
        sum_sq += x * x;
    }
    let norm = sum_sq.sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weights_serialization() {
        let weights = TapWeights::default_calibrated(32, 100);
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("test.tapmodel");

        weights.save_to_file(&path).unwrap();
        let loaded = TapWeights::load_from_file(&path).unwrap();

        assert_eq!(loaded.dim, 32);
        assert_eq!(loaded.vocab_size, 100);
        assert_eq!(loaded.embedding_table.len(), 3200);
    }

    #[test]
    fn rejects_untrusted_model_shape_and_non_finite_weights() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("invalid.tapmodel");

        // A tiny file must not cause an allocation based on attacker-controlled dimensions.
        let mut malformed = TAP_MODEL_MAGIC.to_vec();
        malformed.extend_from_slice(&u32::MAX.to_le_bytes());
        malformed.extend_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, malformed).unwrap();
        assert!(TapWeights::load_from_file(&path).is_err());

        let valid = TapWeights::default_calibrated(4, 8);
        valid.save_to_file(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(TapWeights::load_from_file(&path).is_err());

        let mut bad = valid;
        bad.temperature = f32::INFINITY;
        assert!(bad.save_to_file(&path).is_err());
    }
}
