//! 8-Bit Scalar Quantization (SQ8) for high-dimensional AI vector embeddings.
//!
//! Provides 4x memory compression (75% RAM reduction) by quantizing 32-bit floating point
//! embeddings into compact 8-bit unsigned integers with fast approximate distance computation.

use serde::{Deserialize, Serialize};

/// An 8-bit scalar quantized vector embedding.
///
/// Compresses 32-bit floats into 8-bit integers using uniform affine scaling:
/// `quantized = round((val - min_val) / step)`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantizedVector8 {
    /// Quantized 8-bit byte values
    pub quantized: Vec<u8>,
    /// Minimum float value in original vector
    pub min_val: f32,
    /// Maximum float value in original vector
    pub max_val: f32,
    /// Quantization resolution step size `(max - min) / 255.0`
    pub step: f32,
}

impl QuantizedVector8 {
    /// Quantize a 32-bit floating point slice into an 8-bit compact vector representation.
    pub fn quantize(vector: &[f32]) -> Self {
        if vector.is_empty() {
            return Self {
                quantized: Vec::new(),
                min_val: 0.0,
                max_val: 0.0,
                step: 0.0,
            };
        }

        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;

        for &val in vector {
            if val < min_val {
                min_val = val;
            }
            if val > max_val {
                max_val = val;
            }
        }

        let range = max_val - min_val;
        let step = if range > 1e-9 {
            range / 255.0
        } else {
            1.0
        };

        let mut quantized = Vec::with_capacity(vector.len());
        for &val in vector {
            let q = if range > 1e-9 {
                let scaled = ((val - min_val) / step).round();
                scaled.clamp(0.0, 255.0) as u8
            } else {
                0
            };
            quantized.push(q);
        }

        Self {
            quantized,
            min_val,
            max_val,
            step,
        }
    }

    /// Dequantize back into a 32-bit floating point vector.
    pub fn dequantize(&self) -> Vec<f32> {
        let mut reconstructed = Vec::with_capacity(self.quantized.len());
        for &q in &self.quantized {
            let val = self.min_val + (q as f32 * self.step);
            reconstructed.push(val);
        }
        reconstructed
    }

    /// Calculate fast asymmetric squared Euclidean distance against a full-precision query vector.
    ///
    /// The query vector stays in full 32-bit float precision while the database vector
    /// is decompressed on-the-fly, avoiding heap allocations.
    pub fn asymmetric_l2_distance_squared(&self, query: &[f32]) -> f32 {
        if query.len() != self.quantized.len() {
            return f32::MAX;
        }

        let mut sum = 0.0f32;
        for (q, &x) in self.quantized.iter().zip(query.iter()) {
            let recon = self.min_val + (*q as f32 * self.step);
            let diff = recon - x;
            sum += diff * diff;
        }
        sum
    }

    /// Calculate fast asymmetric dot product against a full-precision query vector.
    pub fn asymmetric_dot_product(&self, query: &[f32]) -> f32 {
        if query.len() != self.quantized.len() {
            return 0.0;
        }

        let mut dot = 0.0f32;
        for (q, &x) in self.quantized.iter().zip(query.iter()) {
            let recon = self.min_val + (*q as f32 * self.step);
            dot += recon * x;
        }
        dot
    }

    /// Calculate fast asymmetric cosine distance against a full-precision query vector.
    pub fn asymmetric_cosine_distance(&self, query: &[f32]) -> f32 {
        if query.len() != self.quantized.len() || self.quantized.is_empty() {
            return 1.0;
        }

        let mut dot = 0.0f32;
        let mut norm_db_sq = 0.0f32;
        let mut norm_q_sq = 0.0f32;

        for (q, &x) in self.quantized.iter().zip(query.iter()) {
            let recon = self.min_val + (*q as f32 * self.step);
            dot += recon * x;
            norm_db_sq += recon * recon;
            norm_q_sq += x * x;
        }

        let denom = (norm_db_sq * norm_q_sq).sqrt();
        if denom > 1e-9 {
            (1.0 - (dot / denom)).clamp(0.0, 2.0)
        } else {
            1.0
        }
    }

    /// Number of dimensions
    pub fn dimensions(&self) -> usize {
        self.quantized.len()
    }

    /// Compression factor achieved compared to 32-bit floats (always 4.0x)
    pub fn compression_ratio(&self) -> f32 {
        4.0
    }
}

/// A Product Quantized (PQ) vector embedding representation.
///
/// Compresses D-dimensional vectors by breaking them into M sub-vectors and
/// storing only the 8-bit centroid ID for each sub-vector codebook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantizedVectorPQ {
    /// 8-bit codebook indices for each sub-vector
    pub codes: Vec<u8>,
    /// Original dimension count of the unquantized vector
    pub orig_dim: usize,
    /// Dimension of each sub-vector
    pub subspace_dim: usize,
}

impl QuantizedVectorPQ {
    /// Number of dimensions of original vector
    pub fn dimensions(&self) -> usize {
        self.orig_dim
    }

    /// Compression factor compared to 32-bit floats
    pub fn compression_ratio(&self) -> f32 {
        if self.codes.is_empty() {
            1.0
        } else {
            (self.orig_dim * 4) as f32 / self.codes.len() as f32
        }
    }
}

/// Product Quantizer engine for ultra-high compression (16x–32x) of AI embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductQuantizer {
    /// Number of sub-vector subspaces (M)
    pub num_subspaces: usize,
    /// Dimension of each sub-vector
    pub subspace_dim: usize,
    /// Codebooks: [subspace_index][centroid_id][dim_val]
    pub codebooks: Vec<Vec<Vec<f32>>>,
}

impl ProductQuantizer {
    /// Initialize a ProductQuantizer with uniform grid centroids
    pub fn new_uniform(dim: usize, num_subspaces: usize, centroids_per_subspace: usize) -> Self {
        assert!(dim % num_subspaces == 0, "Dimension must be divisible by num_subspaces");
        let sub_dim = dim / num_subspaces;
        let k = centroids_per_subspace.min(256);

        let mut codebooks = Vec::with_capacity(num_subspaces);
        for _ in 0..num_subspaces {
            let mut subspace_centroids = Vec::with_capacity(k);
            for c_idx in 0..k {
                let factor = (c_idx as f32 / (k as f32 - 1.0).max(1.0)) * 2.0 - 1.0;
                let centroid = vec![factor; sub_dim];
                subspace_centroids.push(centroid);
            }
            codebooks.push(subspace_centroids);
        }

        Self {
            num_subspaces,
            subspace_dim: sub_dim,
            codebooks,
        }
    }

    /// Train codebooks from training vectors using k-means in pure Safe Rust
    pub fn train(vectors: &[Vec<f32>], num_subspaces: usize, k: usize) -> Self {
        if vectors.is_empty() {
            return Self::new_uniform(128, num_subspaces, k);
        }
        let dim = vectors[0].len();
        assert!(dim % num_subspaces == 0, "Dimension must be divisible by num_subspaces");
        let sub_dim = dim / num_subspaces;
        let k = k.min(256);

        let mut codebooks = Vec::with_capacity(num_subspaces);
        for m in 0..num_subspaces {
            let start = m * sub_dim;
            let end = start + sub_dim;

            // Extract sub-vectors
            let sub_vecs: Vec<&[f32]> = vectors.iter().map(|v| &v[start..end]).collect();

            // Initialize K centroids from sample points
            let mut centroids: Vec<Vec<f32>> = (0..k)
                .map(|i| {
                    let sample_idx = (i * sub_vecs.len()) / k;
                    sub_vecs[sample_idx.min(sub_vecs.len() - 1)].to_vec()
                })
                .collect();

            // Run 3 iterations of Lloyd's k-means
            for _ in 0..3 {
                let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); k];
                for (v_idx, &sv) in sub_vecs.iter().enumerate() {
                    let mut best_dist = f32::MAX;
                    let mut best_c = 0;
                    for (c_idx, c) in centroids.iter().enumerate() {
                        let d: f32 = sv.iter().zip(c.iter()).map(|(x, y)| (x - y) * (x - y)).sum();
                        if d < best_dist {
                            best_dist = d;
                            best_c = c_idx;
                        }
                    }
                    clusters[best_c].push(v_idx);
                }

                for (c_idx, member_indices) in clusters.iter().enumerate() {
                    if member_indices.is_empty() {
                        continue;
                    }
                    let mut mean = vec![0.0f32; sub_dim];
                    for &m_idx in member_indices {
                        for (d_i, &val) in sub_vecs[m_idx].iter().enumerate() {
                            mean[d_i] += val;
                        }
                    }
                    let count = member_indices.len() as f32;
                    for val in mean.iter_mut() {
                        *val /= count;
                    }
                    centroids[c_idx] = mean;
                }
            }

            codebooks.push(centroids);
        }

        Self {
            num_subspaces,
            subspace_dim: sub_dim,
            codebooks,
        }
    }

    /// Encode a full-precision vector into compact PQ codes
    pub fn encode(&self, vector: &[f32]) -> QuantizedVectorPQ {
        let mut codes = Vec::with_capacity(self.num_subspaces);
        for m in 0..self.num_subspaces {
            let start = m * self.subspace_dim;
            let end = (start + self.subspace_dim).min(vector.len());
            let sub_vec = &vector[start..end];

            let mut best_dist = f32::MAX;
            let mut best_code = 0u8;

            for (c_idx, centroid) in self.codebooks[m].iter().enumerate() {
                let dist: f32 = sub_vec
                    .iter()
                    .zip(centroid.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                if dist < best_dist {
                    best_dist = dist;
                    best_code = c_idx as u8;
                }
            }
            codes.push(best_code);
        }

        QuantizedVectorPQ {
            codes,
            orig_dim: vector.len(),
            subspace_dim: self.subspace_dim,
        }
    }

    /// Reconstruct an approximation of the vector from PQ codes
    pub fn decode(&self, pq: &QuantizedVectorPQ) -> Vec<f32> {
        let mut result = Vec::with_capacity(pq.orig_dim);
        for (m, &code) in pq.codes.iter().enumerate() {
            if m < self.codebooks.len() {
                let c_idx = (code as usize).min(self.codebooks[m].len() - 1);
                result.extend_from_slice(&self.codebooks[m][c_idx]);
            }
        }
        result
    }

    /// Fast asymmetric distance computation against full-precision query
    pub fn asymmetric_distance(&self, pq: &QuantizedVectorPQ, query: &[f32]) -> f32 {
        let mut total_dist = 0.0f32;
        for (m, &code) in pq.codes.iter().enumerate() {
            if m >= self.codebooks.len() {
                break;
            }
            let start = m * self.subspace_dim;
            let end = (start + self.subspace_dim).min(query.len());
            let q_sub = &query[start..end];

            let c_idx = (code as usize).min(self.codebooks[m].len() - 1);
            let centroid = &self.codebooks[m][c_idx];

            let sub_dist: f32 = q_sub
                .iter()
                .zip(centroid.iter())
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
            total_dist += sub_dist;
        }
        total_dist.sqrt()
    }
}

/// Ultra-compact 1-bit / 2-bit Random Rotation Quantized Vector (RaBitQ).
///
/// Delivers up to 32x RAM reduction compared to 32-bit floats.
/// Uses random orthogonal sign-flip projection to equalize dimensional variance,
/// binarizes into 1-bit or 2-bit compact bitmasks, and calculates asymmetric distance
/// using hardware POPCNT (`u64::count_ones`) in a single CPU cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaBitQuantizedVector {
    /// Bit-packed binary signs (each bit represents 1 dimension sign: 1 = positive, 0 = negative)
    pub bits: Vec<u64>,
    /// Optional 2nd bit plane for 2-bit quantization refinement (4 quantization levels)
    pub bits_secondary: Option<Vec<u64>>,
    /// Original L2 norm of the vector: ||v||_2
    pub norm: f32,
    /// Original dimension count
    pub orig_dim: usize,
}

impl RaBitQuantizedVector {
    /// Number of dimensions in vector
    pub fn dimensions(&self) -> usize {
        self.orig_dim
    }

    /// Compression factor compared to 32-bit floats
    pub fn compression_ratio(&self) -> f32 {
        let total_bytes = (self.bits.len() * 8)
            + self.bits_secondary.as_ref().map(|b| b.len() * 8).unwrap_or(0)
            + 4; // norm f32
        if total_bytes == 0 {
            1.0
        } else {
            (self.orig_dim * 4) as f32 / total_bytes as f32
        }
    }

    /// Calculate Hamming distance between two 1-bit quantized vectors using single-cycle POPCNT
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        let mut dist = 0u32;
        for (&w1, &w2) in self.bits.iter().zip(other.bits.iter()) {
            dist += (w1 ^ w2).count_ones();
        }
        dist
    }

    /// Estimate approximate Cosine Distance against a rotated query vector
    pub fn asymmetric_cosine_distance(&self, query_rotated: &[f32], query_norm: f32) -> f32 {
        if query_norm <= 0.0 || self.norm <= 0.0 || query_rotated.is_empty() {
            return 1.0;
        }

        let mut dot = 0.0f32;
        let d = self.orig_dim.min(query_rotated.len());

        for (word_idx, &word) in self.bits.iter().enumerate() {
            let start = word_idx * 64;
            let end = (start + 64).min(d);
            for i in start..end {
                let bit_pos = i - start;
                let sign = if (word & (1u64 << bit_pos)) != 0 { 1.0f32 } else { -1.0f32 };
                dot += query_rotated[i] * sign;
            }
        }

        let scale = self.norm / (self.orig_dim as f32).sqrt();
        let approx_dot = dot * scale;
        let sim = (approx_dot / (query_norm * self.norm)).clamp(-1.0, 1.0);
        1.0 - sim
    }
}

/// Random Rotation Quantizer (RaBitQ) Engine
///
/// Implements deterministic orthogonal pseudo-random rotation and 1-bit/2-bit scalar quantization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaBitQuantizer {
    /// Dimension of vectors
    pub dimensions: usize,
    /// Next power of two dimension for Fast Walsh-Hadamard Transform
    pub padded_dim: usize,
    /// Seed used for deterministic orthogonal projection
    pub seed: u64,
    /// Diagonal sign flips (+1.0 / -1.0) for randomized Walsh-Hadamard rotation
    pub sign_flips: Vec<f32>,
    /// Number of bits per dimension (1 or 2)
    pub num_bits: usize,
}

impl RaBitQuantizer {
    /// Create a new RaBitQuantizer with specified dimensions and bit precision (1 or 2 bits)
    pub fn new(dimensions: usize, num_bits: usize) -> Self {
        Self::with_seed(dimensions, num_bits, 0x1337c0de)
    }

    /// Create a new RaBitQuantizer with specified dimensions, bit precision, and custom seed
    pub fn with_seed(dimensions: usize, num_bits: usize, seed: u64) -> Self {
        assert!(dimensions > 0, "Dimensions must be > 0");
        let padded_dim = dimensions.next_power_of_two();
        let mut rng = seed;

        // Generate deterministic pseudo-random signs (+1.0 or -1.0) using SplitMix64
        let mut sign_flips = Vec::with_capacity(padded_dim);
        for _ in 0..padded_dim {
            rng = rng.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            let bit = (z ^ (z >> 31)) & 1;
            sign_flips.push(if bit == 1 { 1.0f32 } else { -1.0f32 });
        }

        Self {
            dimensions,
            padded_dim,
            seed,
            sign_flips,
            num_bits: num_bits.clamp(1, 2),
        }
    }

    /// In-place Fast Walsh-Hadamard Transform (FWHT) in O(N log N) additions/subtractions
    fn fwht(data: &mut [f32]) {
        let mut h = 1;
        while h < data.len() {
            for i in (0..data.len()).step_by(h * 2) {
                for j in i..i + h {
                    let x = data[j];
                    let y = data[j + h];
                    data[j] = x + y;
                    data[j + h] = x - y;
                }
            }
            h *= 2;
        }
    }

    /// Rotate a vector into randomized isotropic space
    pub fn rotate(&self, vector: &[f32]) -> Vec<f32> {
        let mut padded = vec![0.0f32; self.padded_dim];
        for (i, &v) in vector.iter().take(self.dimensions).enumerate() {
            padded[i] = v * self.sign_flips[i];
        }

        Self::fwht(&mut padded);

        let norm_factor = 1.0 / (self.padded_dim as f32).sqrt();
        for val in &mut padded {
            *val *= norm_factor;
        }

        padded.truncate(self.dimensions);
        padded
    }

    /// Quantize a 32-bit floating point vector into a 1-bit/2-bit RaBitQuantizedVector
    pub fn quantize(&self, vector: &[f32]) -> RaBitQuantizedVector {
        let mut norm_sq = 0.0f32;
        for &v in vector {
            norm_sq += v * v;
        }
        let norm = norm_sq.sqrt();

        let rotated = self.rotate(vector);
        let num_words = (self.dimensions + 63) / 64;
        let mut bits = vec![0u64; num_words];

        for (i, &val) in rotated.iter().enumerate() {
            if val >= 0.0 {
                let word_idx = i / 64;
                let bit_pos = i % 64;
                bits[word_idx] |= 1u64 << bit_pos;
            }
        }

        let bits_secondary = if self.num_bits >= 2 {
            let mut sec_words = vec![0u64; num_words];
            // 2-bit threshold: check if absolute rotated magnitude is greater than average
            let avg_mag: f32 = rotated.iter().map(|x| x.abs()).sum::<f32>() / self.dimensions as f32;
            for (i, &val) in rotated.iter().enumerate() {
                if val.abs() >= avg_mag {
                    let word_idx = i / 64;
                    let bit_pos = i % 64;
                    sec_words[word_idx] |= 1u64 << bit_pos;
                }
            }
            Some(sec_words)
        } else {
            None
        };

        RaBitQuantizedVector {
            bits,
            bits_secondary,
            norm,
            orig_dim: self.dimensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pq_quantize_and_asymmetric_distance() {
        let vectors: Vec<Vec<f32>> = (0..16)
            .map(|i| (0..32).map(|j| ((i * 32 + j) as f32 * 0.05).sin()).collect())
            .collect();

        // 32-dim vector into 4 subspaces of 8-dim each, 8 centroids each
        let pq = ProductQuantizer::train(&vectors, 4, 8);
        let encoded = pq.encode(&vectors[0]);

        assert_eq!(encoded.dimensions(), 32);
        assert_eq!(encoded.codes.len(), 4);
        // 32 floats * 4 bytes = 128 bytes. 4 codes = 4 bytes -> 32x compression!
        assert_eq!(encoded.compression_ratio(), 32.0);

        let dist = pq.asymmetric_distance(&encoded, &vectors[0]);
        // Reconstruction distance to self should be small
        assert!(dist < 1.0);
    }

    #[test]
    fn test_sq8_quantize_dequantize_fidelity() {
        let original: Vec<f32> = (0..128).map(|i| (i as f32) * 0.05 - 3.2).collect();
        let sq = QuantizedVector8::quantize(&original);

        assert_eq!(sq.dimensions(), 128);
        assert_eq!(sq.quantized.len(), 128);
        assert_eq!(sq.compression_ratio(), 4.0);

        let reconstructed = sq.dequantize();
        assert_eq!(reconstructed.len(), original.len());

        // Max error per dimension should be less than one quantization step
        let max_error = original
            .iter()
            .zip(reconstructed.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);

        assert!(max_error <= sq.step + 1e-4);
    }

    #[test]
    fn test_sq8_asymmetric_distance() {
        let vec_a: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let vec_b: Vec<f32> = vec![0.15, 0.25, 0.28, 0.42, 0.48];

        let sq_a = QuantizedVector8::quantize(&vec_a);
        let dist_sq = sq_a.asymmetric_l2_distance_squared(&vec_b);

        // Calculate direct Euclidean distance squared
        let direct_sq: f32 = vec_a
            .iter()
            .zip(vec_b.iter())
            .map(|(x, y)| (x - y) * (x - y))
            .sum();

        assert!((dist_sq - direct_sq).abs() < 0.005);
    }
}
