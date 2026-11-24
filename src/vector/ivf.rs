//! Inverted File (IVF) Partitioned Vector Index Engine
//!
//! Partitions high-dimensional vector spaces into Voronoi cells (clusters) using
//! k-means clustering. Combines cluster pruning with 1-bit/2-bit RaBitQ quantization
//! or full-precision vectors. Measure recall, memory and latency on the target
//! dataset; this module does not establish a universal scale or timing claim.

use crate::vector::DistanceMetric;
use crate::vector::quantization::{RaBitQuantizedVector, RaBitQuantizer};
use crate::vector::simd::{
    simd_cosine_distance, simd_euclidean_distance, simd_euclidean_distance_squared,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Magic header for TapirusDB Disk-Backed IVF-EMEANS cluster page files
pub const TAPIR_IVF_EMEANS_MAGIC: &[u8; 8] = b"TPIVFE01";

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
    pub fn new(
        config: IvfConfig,
        centroids: Vec<Vec<f32>>,
        quantizer: Option<RaBitQuantizer>,
    ) -> Self {
        let clusters = centroids.into_iter().map(IvfCluster::new).collect();
        Self {
            config,
            clusters,
            quantizer,
            total_vectors: 0,
        }
    }

    /// Train centroids using Meta FAIR's E-Means (mini-batch EMA sufficient statistics,
    /// half-cosine annealing, and proportional dead-cluster revival in ~`2.5` epochs)
    /// and build an IVF partitioned index from vectors.
    pub fn train_and_build(
        vectors: &[(u64, Vec<f32>)],
        config: IvfConfig,
        quantizer: Option<RaBitQuantizer>,
    ) -> Self {
        Self::train_and_build_emeans(vectors, config, quantizer, 2.5, 0x5441_5049_5249_5646)
    }

    /// Train centroids with configurable E-Means epoch budget and deterministic PRNG seed.
    pub fn train_and_build_emeans(
        vectors: &[(u64, Vec<f32>)],
        config: IvfConfig,
        quantizer: Option<RaBitQuantizer>,
        target_epochs: f32,
        seed: u64,
    ) -> Self {
        assert!(
            !vectors.is_empty(),
            "Cannot train IVF index with empty vectors"
        );
        let k = config.num_clusters.min(vectors.len()).max(1);
        let dim = config.dimensions;
        let spherical = config.metric == DistanceMetric::Cosine;

        let centroids = train_emeans_centroids(vectors, dim, k, target_epochs, spherical, seed);

        // Populate clusters and apply quantization if configured
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

/// Fit `K` E-Means centroids using EMA sufficient statistics `(S_j, N_j)`, half-cosine annealing,
/// automatic scaling-law batch sizes, and proportional dead-cluster revival.
pub fn train_emeans_centroids(
    vectors: &[(u64, Vec<f32>)],
    dim: usize,
    k: usize,
    target_epochs: f32,
    spherical: bool,
    seed: u64,
) -> Vec<Vec<f32>> {
    let n = vectors.len();
    if n == 0 || dim == 0 {
        return Vec::new();
    }
    let k = k.clamp(1, n);
    if k == 1 {
        let mut c = vec![0.0f32; dim];
        for (_, v) in vectors {
            for (dst, &src) in c.iter_mut().zip(v.iter()) {
                *dst += src;
            }
        }
        let inv = 1.0 / (n as f32);
        for dst in &mut c {
            *dst *= inv;
        }
        if spherical {
            l2_normalize_vec(&mut c);
        }
        return vec![c];
    }

    let epochs = if n <= 64 {
        target_epochs.clamp(3.0, 8.0)
    } else {
        target_epochs.clamp(1.2, 6.0)
    };
    let batch_size = if n <= 64 {
        n
    } else {
        (k * 16).clamp(32, (n / 2).max(32)).min(2048)
    };
    let steps_per_epoch = (n as f32 / batch_size as f32).max(1.0);
    let total_steps = ((epochs * steps_per_epoch).ceil() as usize).max(4);
    let alpha_max = if batch_size == n {
        0.85
    } else {
        (2.0 / steps_per_epoch.sqrt()).clamp(0.08, 0.55)
    };

    let mut rng_state = seed ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut centroids = seed_emeans_farthest(vectors, k, dim, spherical, &mut rng_state);

    let init_mass = (batch_size as f32 / k as f32).max(1.0);
    let mut sums: Vec<Vec<f32>> = centroids
        .iter()
        .map(|c| c.iter().map(|&x| x * init_mass).collect())
        .collect();
    let mut masses = vec![init_mass; k];
    let mut stale_steps = vec![0usize; k];

    let mut batch_sums = vec![vec![0.0f32; dim]; k];
    let mut batch_counts = vec![0usize; k];
    let mut batch_furthest = vec![(0usize, -1.0f32); k];
    let mut perm: Vec<usize> = (0..n).collect();
    shuffle_indices(&mut perm, &mut rng_state);
    let mut cursor = 0usize;

    for step in 0..total_steps {
        let progress = step as f32 / (total_steps.saturating_sub(1).max(1)) as f32;
        let alpha = 0.1 * alpha_max + 0.9 * alpha_max * 0.5 * (1.0 + (PI * progress).cos());
        let one_minus_alpha = 1.0 - alpha;

        for j in 0..k {
            batch_sums[j].fill(0.0);
            batch_counts[j] = 0;
            batch_furthest[j] = (0, -1.0);
        }

        for _ in 0..batch_size {
            if cursor >= n {
                shuffle_indices(&mut perm, &mut rng_state);
                cursor = 0;
            }
            let idx = perm[cursor];
            cursor += 1;
            let v = &vectors[idx].1;

            let mut best_c = 0usize;
            let mut best_dist = f32::INFINITY;
            for (ci, c) in centroids.iter().enumerate() {
                let d = if spherical {
                    simd_cosine_distance(v, c)
                } else {
                    simd_euclidean_distance_squared(v, c)
                };
                if d < best_dist {
                    best_dist = d;
                    best_c = ci;
                }
            }

            for (s, &x) in batch_sums[best_c].iter_mut().zip(v.iter()) {
                *s += x;
            }
            batch_counts[best_c] += 1;
            if best_dist > batch_furthest[best_c].1 {
                batch_furthest[best_c] = (idx, best_dist);
            }
        }

        for j in 0..k {
            let cnt = batch_counts[j] as f32;
            for (sv, &bv) in sums[j].iter_mut().zip(batch_sums[j].iter()) {
                *sv = one_minus_alpha * (*sv) + alpha * bv;
            }
            masses[j] = one_minus_alpha * masses[j] + alpha * cnt;

            if batch_counts[j] == 0 {
                stale_steps[j] += 1;
            } else {
                stale_steps[j] = 0;
            }

            if masses[j] > 1e-6 {
                let inv_m = 1.0 / masses[j];
                for (cv, &sv) in centroids[j].iter_mut().zip(sums[j].iter()) {
                    *cv = sv * inv_m;
                }
                if spherical {
                    l2_normalize_vec(&mut centroids[j]);
                }
            }
        }

        // Proportional Dead-Cluster Revival
        let dead_mass_thresh = (0.05 * (batch_size as f32 / k as f32)).max(1e-4);
        let stale_limit = ((steps_per_epoch.ceil() as usize) * 2).max(3);
        for j in 0..k {
            if masses[j] < dead_mass_thresh || stale_steps[j] >= stale_limit {
                if let Some((donor, _)) = masses
                    .iter()
                    .enumerate()
                    .filter(|&(idx, &m)| {
                        idx != j && m > dead_mass_thresh * 2.0 && batch_counts[idx] >= 2
                    })
                    .max_by(|a, b| a.1.total_cmp(b.1))
                {
                    let half_mass = (masses[donor] * 0.5).max(1.0);
                    masses[donor] = half_mass;
                    masses[j] = half_mass;

                    let furthest_idx = batch_furthest[donor].0;
                    let mut new_c = if furthest_idx < n {
                        vectors[furthest_idx].1.clone()
                    } else {
                        centroids[donor].clone()
                    };
                    new_c.resize(dim, 0.0);
                    for (di, val) in new_c.iter_mut().enumerate() {
                        let eps = if (di + step + j) % 2 == 0 {
                            1e-3
                        } else {
                            -1e-3
                        };
                        *val += eps;
                    }
                    if spherical {
                        l2_normalize_vec(&mut new_c);
                    }
                    for (sv, &cv) in sums[j].iter_mut().zip(new_c.iter()) {
                        *sv = cv * half_mass;
                    }
                    for sv in &mut sums[donor] {
                        *sv *= 0.5;
                    }
                    centroids[j] = new_c;
                    stale_steps[j] = 0;
                }
            }
        }
    }

    centroids
}

fn seed_emeans_farthest(
    vectors: &[(u64, Vec<f32>)],
    k: usize,
    dim: usize,
    spherical: bool,
    rng_state: &mut u64,
) -> Vec<Vec<f32>> {
    let n = vectors.len();
    let mut centroids = Vec::with_capacity(k);
    let first = (splitmix64(rng_state) as usize) % n;
    let mut c0 = vectors[first].1.clone();
    c0.resize(dim, 0.0);
    if spherical {
        l2_normalize_vec(&mut c0);
    }
    centroids.push(c0);

    let probe_count = n.min(64);
    for _ in 1..k {
        let mut best_idx = 0usize;
        let mut best_min_dist = -1.0f32;
        for _ in 0..probe_count {
            let cand = (splitmix64(rng_state) as usize) % n;
            let v = &vectors[cand].1;
            let mut min_d = f32::INFINITY;
            for c in &centroids {
                let d = simd_euclidean_distance_squared(v, c);
                if d < min_d {
                    min_d = d;
                }
            }
            if min_d > best_min_dist {
                best_min_dist = min_d;
                best_idx = cand;
            }
        }
        let mut ck = vectors[best_idx].1.clone();
        ck.resize(dim, 0.0);
        if spherical {
            l2_normalize_vec(&mut ck);
        }
        centroids.push(ck);
    }
    centroids
}

fn l2_normalize_vec(v: &mut [f32]) {
    let norm = v.iter().map(|&x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        let inv = 1.0 / norm;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn shuffle_indices(slice: &mut [usize], rng_state: &mut u64) {
    for i in (1..slice.len()).rev() {
        let j = (splitmix64(rng_state) as usize) % (i + 1);
        slice.swap(i, j);
    }
}

fn compute_crc32_ivf(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Directory entry for a disk-resident E-Means Voronoi cluster page (`TPIVFE01`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskIvfClusterMeta {
    /// Zero-based Voronoi cluster index
    pub cluster_id: u32,
    /// Byte offset of this cluster's contiguous posting page inside the `.ivfe` file
    pub file_offset: u64,
    /// Byte length of this cluster's posting page on disk
    pub byte_len: u32,
    /// Number of vector records stored in this cluster page
    pub record_count: u32,
    /// CRC32 integrity checksum of this cluster's on-disk posting page bytes
    pub crc32: u32,
    /// Optional compact 1-bit/2-bit RaBitQ codes kept in RAM for fast candidate screening.
    pub rabit_entries: Vec<(u64, RaBitQuantizedVector)>,
}

/// Disk-Backed `IVF-EMEANS` Vector Index (`TPIVFE01`) for TapirusDB.
///
/// Keeps only E-Means `centroids` and compact per-cluster page directory (+ optional `RaBitQ`
/// codes) in RAM while paging full `f32` vector posting lists on disk with per-cluster `CRC32`
/// verification and a real-time `hot_buffer` for instant inserts/deletes.
#[derive(Debug, Clone)]
pub struct DiskBackedIvfEmeansIndex {
    /// Partitioning and distance metric configuration
    pub config: IvfConfig,
    /// Path to the on-disk `TPIVFE01` cluster page file
    pub disk_path: PathBuf,
    /// Fitted E-Means Voronoi cluster centroids held in RAM
    pub centroids: Vec<Vec<f32>>,
    /// Per-cluster disk page offset and CRC32 directory
    pub cluster_directory: Vec<DiskIvfClusterMeta>,
    /// Optional RaBitQ quantizer for compact in-RAM pre-screening
    pub quantizer: Option<RaBitQuantizer>,
    hot_buffer: HashMap<u64, Vec<f32>>,
    deleted_ids: HashSet<u64>,
    disk_vector_ids: HashSet<u64>,
}

impl DiskBackedIvfEmeansIndex {
    /// Initialize a new Disk-Backed `IVF-EMEANS` index at `disk_path`.
    pub fn new(
        config: IvfConfig,
        disk_path: impl Into<PathBuf>,
        quantizer: Option<RaBitQuantizer>,
    ) -> Self {
        Self {
            config,
            disk_path: disk_path.into(),
            centroids: Vec::new(),
            cluster_directory: Vec::new(),
            quantizer,
            hot_buffer: HashMap::new(),
            deleted_ids: HashSet::new(),
            disk_vector_ids: HashSet::new(),
        }
    }

    /// Insert or update a vector in the real-time hot buffer.
    pub fn insert(&mut self, id: u64, vector: Vec<f32>) {
        self.deleted_ids.remove(&id);
        self.hot_buffer.insert(id, vector);
    }

    /// Mark a vector ID as deleted across hot buffer and disk cluster pages.
    pub fn delete(&mut self, id: u64) -> bool {
        let in_hot = self.hot_buffer.remove(&id).is_some();
        let in_disk = self.disk_vector_ids.contains(&id);
        if in_disk {
            self.deleted_ids.insert(id);
        }
        in_hot || in_disk
    }

    /// Return total active vectors across disk cluster pages and hot buffer.
    pub fn len(&self) -> usize {
        let disk_active = self
            .disk_vector_ids
            .iter()
            .filter(|id| !self.deleted_ids.contains(id) && !self.hot_buffer.contains_key(id))
            .count();
        disk_active + self.hot_buffer.len()
    }

    /// Check if index contains zero active vectors.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Train E-Means centroids (`2.5` epochs) over all active vectors and flush contiguous
    /// per-cluster `TPIVFE01` posting list pages to `self.disk_path`.
    pub fn flush_to_disk(&mut self) -> std::io::Result<usize> {
        let mut all_vectors: HashMap<u64, Vec<f32>> = HashMap::new();
        if self.disk_path.exists() && !self.cluster_directory.is_empty() {
            for meta in &self.cluster_directory {
                if meta.record_count == 0 {
                    continue;
                }
                for (id, vec) in self.read_cluster_page(meta)? {
                    if !self.deleted_ids.contains(&id) {
                        all_vectors.insert(id, vec);
                    }
                }
            }
        }

        for (id, vec) in self.hot_buffer.drain() {
            if !self.deleted_ids.contains(&id) {
                all_vectors.insert(id, vec);
            }
        }
        self.deleted_ids.clear();

        if all_vectors.is_empty() {
            self.centroids.clear();
            self.cluster_directory.clear();
            self.disk_vector_ids.clear();
            return Ok(0);
        }

        let mut corpus: Vec<(u64, Vec<f32>)> = all_vectors.into_iter().collect();
        corpus.sort_by_key(|(id, _)| *id);

        let k = self.config.num_clusters.clamp(1, corpus.len());
        let dim = self.config.dimensions;
        let spherical = self.config.metric == DistanceMetric::Cosine;
        let centroids =
            train_emeans_centroids(&corpus, dim, k, 2.5, spherical, 0x5441_5049_5249_5646);

        let mut inverted: Vec<Vec<(u64, Vec<f32>)>> = vec![Vec::new(); centroids.len()];
        for (id, vec) in corpus {
            let cid = self.nearest_centroid_for(&vec, &centroids);
            inverted[cid].push((id, vec));
        }

        if let Some(parent) = self.disk_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = File::create(&self.disk_path)?;
        file.write_all(TAPIR_IVF_EMEANS_MAGIC)?;
        file.write_all(&(dim as u32).to_le_bytes())?;
        let metric_byte = match self.config.metric {
            DistanceMetric::Cosine => 0u8,
            DistanceMetric::Euclidean => 1u8,
            DistanceMetric::DotProduct => 2u8,
        };
        file.write_all(&[metric_byte])?;
        file.write_all(&(centroids.len() as u32).to_le_bytes())?;

        for c in &centroids {
            for &val in c {
                file.write_all(&val.to_le_bytes())?;
            }
        }

        let dir_offset_pos = file.stream_position()?;
        file.write_all(&0u64.to_le_bytes())?;

        let mut directory = Vec::with_capacity(centroids.len());
        let mut disk_ids = HashSet::new();

        for (cid, list) in inverted.into_iter().enumerate() {
            let page_offset = file.stream_position()?;
            let mut page_buf = Vec::with_capacity(list.len() * (8 + dim * 4));
            let mut rabit_entries = Vec::new();

            for (id, vec) in list {
                page_buf.extend_from_slice(&id.to_le_bytes());
                for &v in &vec {
                    page_buf.extend_from_slice(&v.to_le_bytes());
                }
                if let Some(ref q) = self.quantizer {
                    rabit_entries.push((id, q.quantize(&vec)));
                }
                disk_ids.insert(id);
            }

            let crc32 = compute_crc32_ivf(&page_buf);
            let byte_len = page_buf.len() as u32;
            let record_count = (byte_len as usize / (8 + dim * 4)) as u32;
            file.write_all(&page_buf)?;

            directory.push(DiskIvfClusterMeta {
                cluster_id: cid as u32,
                file_offset: page_offset,
                byte_len,
                record_count,
                crc32,
                rabit_entries,
            });
        }

        let dir_offset = file.stream_position()?;
        for meta in &directory {
            file.write_all(&meta.cluster_id.to_le_bytes())?;
            file.write_all(&meta.file_offset.to_le_bytes())?;
            file.write_all(&meta.byte_len.to_le_bytes())?;
            file.write_all(&meta.record_count.to_le_bytes())?;
            file.write_all(&meta.crc32.to_le_bytes())?;
        }

        file.seek(SeekFrom::Start(dir_offset_pos))?;
        file.write_all(&dir_offset.to_le_bytes())?;
        file.flush()?;

        let total = disk_ids.len();
        self.centroids = centroids;
        self.cluster_directory = directory;
        self.disk_vector_ids = disk_ids;
        Ok(total)
    }

    /// Reopen and verify an existing `TPIVFE01` Disk-Backed `IVF-EMEANS` index from disk.
    pub fn load_from_disk(
        disk_path: impl AsRef<Path>,
        default_n_probe: usize,
        quantizer: Option<RaBitQuantizer>,
    ) -> std::io::Result<Self> {
        let path = disk_path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != TAPIR_IVF_EMEANS_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid TPIVFE01 header magic",
            ));
        }

        let mut u32_buf = [0u8; 4];
        file.read_exact(&mut u32_buf)?;
        let dimensions = u32::from_le_bytes(u32_buf) as usize;

        let mut metric_buf = [0u8; 1];
        file.read_exact(&mut metric_buf)?;
        let metric = match metric_buf[0] {
            1 => DistanceMetric::Euclidean,
            2 => DistanceMetric::DotProduct,
            _ => DistanceMetric::Cosine,
        };

        file.read_exact(&mut u32_buf)?;
        let k = u32::from_le_bytes(u32_buf) as usize;

        let mut centroids = Vec::with_capacity(k);
        for _ in 0..k {
            let mut c = Vec::with_capacity(dimensions);
            for _ in 0..dimensions {
                file.read_exact(&mut u32_buf)?;
                c.push(f32::from_le_bytes(u32_buf));
            }
            centroids.push(c);
        }

        let mut u64_buf = [0u8; 8];
        file.read_exact(&mut u64_buf)?;
        let dir_offset = u64::from_le_bytes(u64_buf);
        file.seek(SeekFrom::Start(dir_offset))?;

        let mut directory = Vec::with_capacity(k);
        for _ in 0..k {
            file.read_exact(&mut u32_buf)?;
            let cluster_id = u32::from_le_bytes(u32_buf);
            file.read_exact(&mut u64_buf)?;
            let file_offset = u64::from_le_bytes(u64_buf);
            file.read_exact(&mut u32_buf)?;
            let byte_len = u32::from_le_bytes(u32_buf);
            file.read_exact(&mut u32_buf)?;
            let record_count = u32::from_le_bytes(u32_buf);
            file.read_exact(&mut u32_buf)?;
            let crc32 = u32::from_le_bytes(u32_buf);

            directory.push(DiskIvfClusterMeta {
                cluster_id,
                file_offset,
                byte_len,
                record_count,
                crc32,
                rabit_entries: Vec::new(),
            });
        }

        let mut index = Self {
            config: IvfConfig {
                dimensions,
                num_clusters: k.max(1),
                default_n_probe: default_n_probe.max(1),
                metric,
            },
            disk_path: path,
            centroids,
            cluster_directory: Vec::new(),
            quantizer,
            hot_buffer: HashMap::new(),
            deleted_ids: HashSet::new(),
            disk_vector_ids: HashSet::new(),
        };

        for mut meta in directory {
            let entries = index.read_cluster_page(&meta)?;
            if let Some(ref q) = index.quantizer {
                meta.rabit_entries = entries
                    .iter()
                    .map(|(id, vec)| (*id, q.quantize(vec)))
                    .collect();
            }
            for (id, _) in entries {
                index.disk_vector_ids.insert(id);
            }
            index.cluster_directory.push(meta);
        }

        Ok(index)
    }

    /// Search top-k nearest neighbors by probing `n_probe` E-Means cluster pages from disk
    /// and merging with `hot_buffer`.
    pub fn search(
        &self,
        query: &[f32],
        top_k: usize,
        n_probe: usize,
    ) -> std::io::Result<Vec<(u64, f32)>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let mut candidates: Vec<(u64, f32)> = Vec::new();
        for (&id, vec) in &self.hot_buffer {
            if self.deleted_ids.contains(&id) {
                continue;
            }
            let d = self.distance(query, vec);
            candidates.push((id, d));
        }

        if !self.centroids.is_empty() && !self.cluster_directory.is_empty() {
            let probe_count = n_probe.max(1).min(self.centroids.len());
            let mut cluster_dists: Vec<(usize, f32)> = self
                .centroids
                .iter()
                .enumerate()
                .map(|(idx, c)| (idx, self.distance(query, c)))
                .collect();
            cluster_dists.sort_by(|a, b| a.1.total_cmp(&b.1));

            for &(cid, _) in cluster_dists.iter().take(probe_count) {
                let meta = &self.cluster_directory[cid];
                if meta.record_count == 0 {
                    continue;
                }
                for (id, vec) in self.read_cluster_page(meta)? {
                    if self.deleted_ids.contains(&id) || self.hot_buffer.contains_key(&id) {
                        continue;
                    }
                    candidates.push((id, self.distance(query, &vec)));
                }
            }
        }

        candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
        candidates.truncate(top_k);
        Ok(candidates)
    }

    fn nearest_centroid_for(&self, vec: &[f32], centroids: &[Vec<f32>]) -> usize {
        let mut best_idx = 0usize;
        let mut best_dist = f32::INFINITY;
        for (i, c) in centroids.iter().enumerate() {
            let d = self.distance(vec, c);
            if d < best_dist {
                best_dist = d;
                best_idx = i;
            }
        }
        best_idx
    }

    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.config.metric {
            DistanceMetric::Cosine => simd_cosine_distance(a, b),
            DistanceMetric::Euclidean | DistanceMetric::DotProduct => simd_euclidean_distance(a, b),
        }
    }

    fn read_cluster_page(
        &self,
        meta: &DiskIvfClusterMeta,
    ) -> std::io::Result<Vec<(u64, Vec<f32>)>> {
        if meta.byte_len == 0 || meta.record_count == 0 {
            return Ok(Vec::new());
        }
        let mut file = File::open(&self.disk_path)?;
        file.seek(SeekFrom::Start(meta.file_offset))?;
        let mut buf = vec![0u8; meta.byte_len as usize];
        file.read_exact(&mut buf)?;

        let actual_crc = compute_crc32_ivf(&buf);
        if actual_crc != meta.crc32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Corrupted TPIVFE01 cluster page {} CRC32 mismatch",
                    meta.cluster_id
                ),
            ));
        }

        let dim = self.config.dimensions;
        let rec_size = 8 + dim * 4;
        let mut out = Vec::with_capacity(meta.record_count as usize);
        for r in 0..(meta.record_count as usize) {
            let base = r * rec_size;
            let id = u64::from_le_bytes([
                buf[base],
                buf[base + 1],
                buf[base + 2],
                buf[base + 3],
                buf[base + 4],
                buf[base + 5],
                buf[base + 6],
                buf[base + 7],
            ]);
            let mut vec = Vec::with_capacity(dim);
            for d in 0..dim {
                let off = base + 8 + d * 4;
                vec.push(f32::from_le_bytes([
                    buf[off],
                    buf[off + 1],
                    buf[off + 2],
                    buf[off + 3],
                ]));
            }
            out.push((id, vec));
        }
        Ok(out)
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
            let v: Vec<f32> = (0..dim)
                .map(|j| ((i * dim + j) as f32 * 0.1).sin())
                .collect();
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
            let v: Vec<f32> = (0..dim)
                .map(|j| ((i * dim + j) as f32 * 0.05).cos())
                .collect();
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

    #[test]
    fn test_disk_backed_ivf_emeans_persistence_and_search() {
        let dim = 32;
        let temp_path =
            std::env::temp_dir().join(format!("tapirus_ivf_emeans_{}.ivfe", std::process::id()));
        let config = IvfConfig {
            dimensions: dim,
            num_clusters: 8,
            default_n_probe: 3,
            metric: DistanceMetric::Cosine,
        };

        let mut index = DiskBackedIvfEmeansIndex::new(config, &temp_path, None);
        let mut query_vec = Vec::new();
        for i in 0..120u64 {
            let v: Vec<f32> = (0..dim)
                .map(|j| ((i as usize * dim + j) as f32 * 0.09).sin())
                .collect();
            if i == 25 {
                query_vec = v.clone();
            }
            index.insert(i, v);
        }

        let persisted = index.flush_to_disk().unwrap();
        assert_eq!(persisted, 120);
        assert_eq!(index.centroids.len(), 8);

        // Reopen from disk and verify exact recall on ID 25
        let reopened = DiskBackedIvfEmeansIndex::load_from_disk(&temp_path, 3, None).unwrap();
        assert_eq!(reopened.len(), 120);
        let hits = reopened.search(&query_vec, 5, 3).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, 25);
        assert!(hits[0].1 < 1e-4);

        let _ = std::fs::remove_file(&temp_path);
    }
}
