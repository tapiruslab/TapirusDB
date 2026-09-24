//! Embedded AI Vector Types and Distance Metrics.
//!
//! Provides native vector similarity calculation directly inside embedded TapirusDB.

pub mod hnsw;
pub mod ivf;
pub mod paged_hnsw;
pub mod quantization;
pub mod simd;
pub use hnsw::{HnswConfig, HnswIndex, HnswSnapshot};
pub use ivf::{IvfCluster, IvfConfig, IvfIndex};
pub use paged_hnsw::{PagedHnswIndex, PagedNodeRecord, PagedVectorStore};
pub use quantization::{
    ProductQuantizer, QuantizedVector8, QuantizedVectorPQ, RaBitQuantizedVector, RaBitQuantizer,
};
pub use simd::{
    simd_cosine_distance, simd_dot_product, simd_euclidean_distance,
    simd_euclidean_distance_squared,
};

use serde::{Deserialize, Serialize};

/// High-dimensional floating point vector embedding
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vector(pub Vec<f32>);

impl Vector {
    /// Create a new Vector from a slice
    pub fn new(data: Vec<f32>) -> Self {
        Self(data)
    }

    /// Number of dimensions in vector
    pub fn dimensions(&self) -> usize {
        self.0.len()
    }

    /// As slice
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Supported vector distance metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistanceMetric {
    /// Cosine distance: 1.0 - CosineSimilarity
    Cosine,
    /// Euclidean distance (L2 norm)
    Euclidean,
    /// Dot Product
    DotProduct,
}

/// Calculate Cosine Distance between two floating point slices
///
/// Uses 16-lane parallel loop unrolling and multi-lane accumulators for SIMD/NEON auto-vectorization
/// under 100% Safe Rust (`#![forbid(unsafe_code)]`).
#[inline]
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    simd_cosine_distance(a, b)
}

/// Calculate Cosine Similarity between two floating point slices $[-1.0, 1.0]$
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine_distance(a, b)
}

/// Calculate Euclidean Distance between two floating point slices
///
/// Uses 16-lane parallel loop unrolling and multi-lane accumulators for SIMD/NEON auto-vectorization
/// under 100% Safe Rust (`#![forbid(unsafe_code)]`).
#[inline]
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    simd_euclidean_distance(a, b)
}

/// Calculate Dot Product between two floating point slices with 16-lane unrolling
#[inline]
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    simd_dot_product(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_identical() {
        let a = [1.0, 2.0, 3.0];
        let d = cosine_distance(&a, &a);
        assert!(d.abs() < 1e-5);
    }

    #[test]
    fn test_unrolled_vector_metrics() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let b = vec![2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0];
        let cos = cosine_distance(&a, &b);
        assert!(cos > 0.0 && cos < 0.1);

        let euc = euclidean_distance(&a, &b);
        let expected = (10.0f32 * 1.0).sqrt();
        assert!((euc - expected).abs() < 1e-4);

        let dp = dot_product(&a, &b);
        assert!(dp > 0.0);
    }
}
