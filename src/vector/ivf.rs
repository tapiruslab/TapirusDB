//! Inverted File (IVF) Partitioned Vector Index Engine
//!
//! Partitions high-dimensional vector spaces into Voronoi cells (clusters) using
//! k-means clustering. Combines cluster pruning with 1-bit/2-bit RaBitQ quantization
//! or full-precision vectors to enable sub-millisecond retrieval across millions of vectors
//! on embedded memory budgets under 100% Safe Rust (`#![forbid(unsafe_code)]`).

use crate::vector::quantization::{RaBitQuantizedVector, RaBitQuantizer};
use crate::vector::simd::{simd_cosine_distance, simd_euclidean_distance};
use crate::vector::DistanceMetric;
use serde::{Deserialize, Serialize};

/// Configuration parameters for IVF index partitioning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfConfig {
    /// Dimension count of vectors
    pub dimensions: usize,
    /// Number of cluster centroids (Voronoi partitions, e.g. 16, 64, 256)
    pub num_clusters: usize,
    /// Default number of nearest clusters to inspect during queries
    pub default_n_probe: usize,
    /// Distance metric (Cosine or Euclidean)
    pub metric: DistanceMetric,
}

impl Default for IvfConfig {
    fn default() -> Self {
        Self {
            dimensions: 128,
            num_clusters: 16,
            default_n_probe: 4,
            metric: DistanceMetric::Cosine,
        }
    }
}

/// A Voronoi partition (cluster) in the IVF index containing assigned vectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfCluster {
    /// Centroid vector in high-dimensional space
    pub centroid: Vec<f32>,
    /// Unique 64-bit vector identifiers
    pub vector_ids: Vec<u64>,
    /// Optional compact 1-bit / 2-bit quantized vectors (RaBitQ)
    pub rabit_vectors: Vec<RaBitQuantizedVector>,
    /// Optional full-precision vectors (retained if quantization is disabled)
    pub raw_vectors: Vec<Vec<f32>>,
}

impl IvfCluster {
    /// Create a new cluster with a given centroid
    pub fn new(centroid: Vec<f32>) -> Self {
        Self {
            centroid,
            vector_ids: Vec::new(),
            rabit_vectors: Vec::new(),
            raw_vectors: Vec::new(),
        }
    }

    /// Number of vectors stored in this cluster
    pub fn len(&self) -> usize {
        self.vector_ids.len()
    }

    /// Check if cluster is empty
    pub fn is_empty(&self) -> bool {
        self.vector_ids.is_empty()
    }
}

/// Inverted File (IVF) Partitioned Vector Index with optional RaBitQ acceleration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IvfIndex {
    /// Index configuration
    pub config: IvfConfig,
    /// Cluster partitions
    pub clusters: Vec<IvfCluster>,
    /// Optional RaBitQ quantizer engine for ultra-low bit compression
    pub quantizer: Option<RaBitQuantizer>,
    /// Total count of vectors indexed
    pub total_vectors: usize,
}

impl IvfIndex {
    /// Create a new IVF index with trained centroids
    pub fn new(config: IvfConfig, centroids: Vec<Vec<f32>>, quantizer: Option<RaBitQuantizer>) -> Self {
        let clusters = centroids.into_iter().map(IvfCluster::new).collect();
        Self {
            config,
            clusters,
            quantizer,
            total_vectors: 0,
        }
    }

    /// Train centroids using k-means and build an IVF partitioned index from vectors
    pub fn train_and_build(
        vectors: &[(u64, Vec<f32>)],
        config: IvfConfig,
        quantizer: Option<RaBitQuantizer>,
    ) -> Self {
        assert!(!vectors.is_empty(), "Cannot train IVF index with empty vectors");
        let k = config.num_clusters.min(vectors.len()).max(1);
        let dim = config.dimensions;

        // Step 1: Initialize k centroids using deterministic stride sampling
        let stride = (vectors.len() / k).max(1);
        let mut centroids: Vec<Vec<f32>> = (0..k)
            .map(|i| {
                let idx = (i * stride).min(vectors.len() - 1);
                let mut c = vectors[idx].1.clone();
                if c.len() < dim {
                    c.resize(dim, 0.0);
                }
                c
            })
            .collect();

        // Step 2: Run k-means iterations (up to 8 iterations for fast convergence)
        let iterations = 8;
        for _ in 0..iterations {
            let mut cluster_sums = vec![vec![0.0f32; dim]; k];
            let mut cluster_counts = vec![0usize; k];

            for (_, vec) in vectors {
                let mut best_cluster = 0;
                let mut best_dist = f32::MAX;

                for (c_idx, centroid) in centroids.iter().enumerate() {
                    let d = match config.metric {
                        DistanceMetric::Cosine => simd_cosine_distance(vec, centroid),
                        DistanceMetric::Euclidean | DistanceMetric::DotProduct => {
                            simd_euclidean_distance(vec, centroid)
                        }
                    };
                    if d < best_dist {
                        best_dist = d;
                        best_cluster = c_idx;
                    }
                }

                cluster_counts[best_cluster] += 1;
                for (d_i, &val) in vec.iter().take(dim).enumerate() {
                    cluster_sums[best_cluster][d_i] += val;
                }
            }

            // Update centroid locations
            for (c_idx, centroid) in centroids.iter_mut().enumerate() {
                let count = cluster_counts[c_idx];
                if count > 0 {
                    let inv_count = 1.0 / (count as f32);
                    for d_i in 0..dim {
                        centroid[d_i] = cluster_sums[c_idx][d_i] * inv_count;
                    }
                    if config.metric == DistanceMetric::Cosine {
                        // Normalize centroid to unit sphere
                        let norm = centroid.iter().map(|&x| x * x).sum::<f32>().sqrt();
                        if norm > 1e-9 {
                            let inv_norm = 1.0 / norm;
                            for val in centroid.iter_mut() {
                                *val *= inv_norm;
                            }
                        }
                    }
                }
            }
        }

        // Step 3: Populate clusters and apply quantization if configured
        let mut index = Self::new(config, centroids, quantizer);
        for &(id, ref vec) in vectors {
            index.insert(id, vec);
        }

        index
    }

    /// Find the index of the nearest centroid for a given vector
    pub fn nearest_centroid(&self, vector: &[f32]) -> usize {
        if self.clusters.is_empty() {
            return 0;
        }

        let mut best_idx = 0;
        let mut best_dist = f32::MAX;

        for (i, cluster) in self.clusters.iter().enumerate() {
            let d = match self.config.metric {
                DistanceMetric::Cosine => simd_cosine_distance(vector, &cluster.centroid),
                DistanceMetric::Euclidean | DistanceMetric::DotProduct => {
                    simd_euclidean_distance(vector, &cluster.centroid)
                }
            };
            if d < best_dist {
                best_dist = d;
                best_idx = i;
            }
        }

        best_idx
    }

    /// Insert a vector into its nearest Voronoi cluster
    pub fn insert(&mut self, id: u64, vector: &[f32]) {
        let cluster_idx = self.nearest_centroid(vector);
        let cluster = &mut self.clusters[cluster_idx];

        cluster.vector_ids.push(id);
        if let Some(ref q) = self.quantizer {
            let quantized = q.quantize(vector);
            cluster.rabit_vectors.push(quantized);
        } else {
            cluster.raw_vectors.push(vector.to_vec());
        }

        self.total_vectors += 1;
    }

    /// Query the top-k nearest neighbors, inspecting `n_probe` nearest clusters
    pub fn search(&self, query: &[f32], top_k: usize, n_probe: usize) -> Vec<(u64, f32)> {
        if self.clusters.is_empty() || top_k == 0 {
            return Vec::new();
        }

        let probe_count = n_probe.max(1).min(self.clusters.len());

        // Step 1: Calculate distance to all cluster centroids
        let mut cluster_dists: Vec<(usize, f32)> = self
            .clusters
            .iter()
            .enumerate()
            .map(|(idx, cluster)| {
                let d = match self.config.metric {
                    DistanceMetric::Cosine => simd_cosine_distance(query, &cluster.centroid),
                    DistanceMetric::Euclidean | DistanceMetric::DotProduct => {
                        simd_euclidean_distance(query, &cluster.centroid)
                    }
                };
                (idx, d)
            })
            .collect();

        // Sort centroids to identify the `probe_count` closest partitions
        cluster_dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Step 2: Search only inside the probed clusters
        let mut candidates: Vec<(u64, f32)> = Vec::new();

        if let Some(ref q) = self.quantizer {
            let query_rotated = q.rotate(query);
            let query_norm = query.iter().map(|&x| x * x).sum::<f32>().sqrt();

            for &(c_idx, _) in cluster_dists.iter().take(probe_count) {
                let cluster = &self.clusters[c_idx];
                for (&id, q_vec) in cluster.vector_ids.iter().zip(cluster.rabit_vectors.iter()) {
                    let dist = q_vec.asymmetric_cosine_distance(&query_rotated, query_norm);
                    candidates.push((id, dist));
                }
            }
        } else {
            for &(c_idx, _) in cluster_dists.iter().take(probe_count) {
                let cluster = &self.clusters[c_idx];
                for (&id, raw_vec) in cluster.vector_ids.iter().zip(cluster.raw_vectors.iter()) {
                    let dist = match self.config.metric {
                        DistanceMetric::Cosine => simd_cosine_distance(query, raw_vec),
                        DistanceMetric::Euclidean | DistanceMetric::DotProduct => {
                            simd_euclidean_distance(query, raw_vec)
                        }
                    };
                    candidates.push((id, dist));
                }
            }
        }

        // Step 3: Sort candidates and return top-k
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(top_k);
        candidates
    }

    /// Return total vector count in the index
    pub fn len(&self) -> usize {
        self.total_vectors
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.total_vectors == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ivf_basic_clustering_and_search() {
        let dim = 32;
        let mut vectors = Vec::new();
        for i in 0..100 {
            let v: Vec<f32> = (0..dim).map(|j| ((i * dim + j) as f32 * 0.1).sin()).collect();
            vectors.push((i as u64, v));
        }

        let config = IvfConfig {
            dimensions: dim,
            num_clusters: 8,
            default_n_probe: 2,
            metric: DistanceMetric::Cosine,
        };

        let index = IvfIndex::train_and_build(&vectors, config, None);
        assert_eq!(index.len(), 100);
        assert_eq!(index.clusters.len(), 8);

        // Query with vector 0 (should return vector 0 as top candidate)
        let results = index.search(&vectors[0].1, 5, 4);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 0);
        assert!(results[0].1 < 1e-4);
    }

    #[test]
    fn test_ivf_rabitq_quantized_search() {
        let dim = 64;
        let quantizer = RaBitQuantizer::new(dim, 1);
        let mut vectors = Vec::new();
        for i in 0..60 {
            let v: Vec<f32> = (0..dim).map(|j| ((i * dim + j) as f32 * 0.05).cos()).collect();
            vectors.push((i as u64, v));
        }

        let config = IvfConfig {
            dimensions: dim,
            num_clusters: 6,
            default_n_probe: 3,
            metric: DistanceMetric::Cosine,
        };

        let index = IvfIndex::train_and_build(&vectors, config, Some(quantizer));
        assert_eq!(index.len(), 60);

        let results = index.search(&vectors[10].1, 3, 3);
        assert!(!results.is_empty());
        // Closest match should be ID 10
        assert_eq!(results[0].0, 10);
    }
}
