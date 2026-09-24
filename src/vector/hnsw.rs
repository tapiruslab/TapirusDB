//! Hierarchical Navigable Small World (HNSW) Vector Index.
//!
//! Provides sub-millisecond approximate nearest neighbor (ANN) search directly
//! inside TapirusDB.

use crate::error::{Error, Result};
use crate::traits::VectorIndexEngine;
use crate::vector::{cosine_distance, euclidean_distance, DistanceMetric};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

/// Candidate with smallest distance prioritized (Min-Heap)
#[derive(Debug, Clone, PartialEq)]
struct MinCandidate {
    id: u64,
    distance: f32,
}

impl Eq for MinCandidate {}

impl Ord for MinCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other.distance.partial_cmp(&self.distance).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for MinCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Candidate with largest distance prioritized (Max-Heap)
#[derive(Debug, Clone, PartialEq)]
struct MaxCandidate {
    id: u64,
    distance: f32,
}

impl Eq for MaxCandidate {}

impl Ord for MaxCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance.partial_cmp(&other.distance).unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for MaxCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Configuration parameters for tuning the HNSW graph construction and search
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct HnswConfig {
    /// Max neighbors per node at layer > 0 (default: 16)
    pub m: usize,
    /// Max neighbors per node at layer 0 (default: 32)
    pub m0: usize,
    /// Size of dynamic candidate list during construction (default: 64)
    pub ef_construction: usize,
    /// Size of dynamic candidate list during search (default: 32)
    pub ef_search: usize,
    /// Whether to enable 8-bit scalar quantization (SQ8) to reduce RAM by 4x
    pub quantize_sq8: bool,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            m: 16,
            m0: 32,
            ef_construction: 64,
            ef_search: 32,
            quantize_sq8: false,
        }
    }
}

/// A node in the HNSW multi-layer graph
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HnswNode {
    /// Unique record ID
    pub id: u64,
    /// Vector embedding values
    pub vector: Vec<f32>,
    /// Top layer this node appears in (0 is bottom layer)
    pub top_layer: usize,
    /// Adjacency lists for each layer: `layer_neighbors[layer] = Vec<u64>`
    pub layer_neighbors: Vec<Vec<u64>>,
    /// Optional 8-bit scalar quantized representation for ultra-fast low-memory search
    #[serde(default)]
    pub quantized: Option<crate::vector::QuantizedVector8>,
}

/// Persistent disk snapshot of an HNSW multi-layer graph topology
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HnswSnapshot {
    /// Vector dimensionality
    pub dimensions: usize,
    /// Distance metric
    pub metric: DistanceMetric,
    /// Index hyperparameter configuration
    pub config: HnswConfig,
    /// Entry point node ID in the graph
    pub entry_point: Option<u64>,
    /// Maximum level reached in the hierarchy
    pub max_level: usize,
    /// All node records in the index
    pub nodes: Vec<HnswNode>,
    /// List of deleted node IDs marked as tombstones
    pub tombstones: Vec<u64>,
}

/// In-memory Hierarchical Navigable Small World index
#[derive(Debug, Clone)]
pub struct HnswIndex {
    /// Number of dimensions per vector
    pub dimensions: usize,
    /// Distance metric
    pub metric: DistanceMetric,
    /// Max neighbors per node at layer > 0
    pub m: usize,
    /// Max neighbors per node at layer 0 (typically 2 * m)
    pub m0: usize,
    /// Size of dynamic candidate list during construction
    pub ef_construction: usize,
    /// Size of dynamic candidate list during search
    pub ef_search: usize,
    /// Active HNSW configuration
    pub config: HnswConfig,
    /// Entry point node ID
    pub entry_point: Option<u64>,
    /// Maximum level currently in the index
    pub max_level: usize,
    /// Level multiplier parameter `1 / ln(M)`
    pub ml: f64,
    /// Stored nodes
    pub nodes: std::collections::HashMap<u64, HnswNode>,
    /// Set of tombstoned/deleted node IDs awaiting vacuum
    pub tombstones: HashSet<u64>,
    /// Internal SplitMix64 uniform PRNG state
    pub rng_state: u64,
}

impl HnswIndex {
    /// Create a new HNSW index with target dimensions and distance metric
    pub fn new(dimensions: usize, metric: DistanceMetric) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15);
        Self::with_config_and_seed(dimensions, metric, HnswConfig::default(), seed)
    }

    /// Create a new HNSW index with custom seed for reproducible testing
    pub fn with_seed(dimensions: usize, metric: DistanceMetric, seed: u64) -> Self {
        Self::with_config_and_seed(dimensions, metric, HnswConfig::default(), seed)
    }

    /// Create a new HNSW index with custom configuration
    pub fn with_config(dimensions: usize, metric: DistanceMetric, config: HnswConfig) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15);
        Self::with_config_and_seed(dimensions, metric, config, seed)
    }

    /// Create a new HNSW index with custom configuration and random seed
    pub fn with_config_and_seed(dimensions: usize, metric: DistanceMetric, config: HnswConfig, seed: u64) -> Self {
        let m = config.m.max(2);
        let m0 = config.m0.max(2);
        let ef_construction = config.ef_construction.max(m);
        let ef_search = config.ef_search.max(1);
        let ml = 1.0 / (m as f64).ln();

        Self {
            dimensions,
            metric,
            m,
            m0,
            ef_construction,
            ef_search,
            config,
            entry_point: None,
            max_level: 0,
            ml,
            nodes: std::collections::HashMap::new(),
            tombstones: HashSet::new(),
            rng_state: if seed == 0 { 0xdeadbeefcafebabe } else { seed },
        }
    }

    /// Create a new HNSW index with explicit configurable parameters
    pub fn with_params(
        dimensions: usize,
        metric: DistanceMetric,
        m: usize,
        ef_construction: usize,
        ef_search: usize,
        quantize_sq8: bool,
    ) -> Self {
        let config = HnswConfig {
            m,
            m0: m * 2,
            ef_construction,
            ef_search,
            quantize_sq8,
        };
        Self::with_config(dimensions, metric, config)
    }

    /// Export the entire HNSW topology and nodes as a serializable snapshot for persistent disk storage
    pub fn export_snapshot(&self) -> HnswSnapshot {
        HnswSnapshot {
            dimensions: self.dimensions,
            metric: self.metric,
            config: self.config.clone(),
            entry_point: self.entry_point,
            max_level: self.max_level,
            nodes: self.nodes.values().cloned().collect(),
            tombstones: self.tombstones.iter().copied().collect(),
        }
    }

    /// Restore a fully structured HNSW index in O(N) from a persistent snapshot without re-indexing
    pub fn import_snapshot(snapshot: HnswSnapshot) -> Self {
        let m = snapshot.config.m.max(2);
        let m0 = snapshot.config.m0.max(2);
        let ml = 1.0 / (m as f64).ln();
        let mut nodes = std::collections::HashMap::new();
        for node in snapshot.nodes {
            nodes.insert(node.id, node);
        }
        let tombstones = snapshot.tombstones.into_iter().collect();

        Self {
            dimensions: snapshot.dimensions,
            metric: snapshot.metric,
            m,
            m0,
            ef_construction: snapshot.config.ef_construction,
            ef_search: snapshot.config.ef_search,
            config: snapshot.config,
            entry_point: snapshot.entry_point,
            max_level: snapshot.max_level,
            ml,
            nodes,
            tombstones,
            rng_state: 0xdeadbeefcafebabe,
        }
    }

    /// Compute distance between query and node, utilizing 8-bit scalar quantization if enabled
    #[inline]
    pub fn compute_node_distance(&self, query: &[f32], node: &HnswNode, metric: DistanceMetric) -> f32 {
        if let Some(ref q) = node.quantized {
            match metric {
                DistanceMetric::Cosine => q.asymmetric_cosine_distance(query),
                DistanceMetric::Euclidean => q.asymmetric_l2_distance_squared(query).sqrt(),
                DistanceMetric::DotProduct => {
                    let dot = q.asymmetric_dot_product(query);
                    if dot.is_finite() { -dot } else { f32::MAX }
                }
            }
        } else {
            self.distance_with_metric(query, &node.vector, metric)
        }
    }

    /// Calculate distance between two vectors using configured metric
    #[inline]
    pub fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self.distance_with_metric(a, b, self.metric)
    }

    /// Calculate distance between two vectors using an explicit metric override
    #[inline]
    pub fn distance_with_metric(&self, a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
        match metric {
            DistanceMetric::Cosine => cosine_distance(a, b),
            DistanceMetric::Euclidean => euclidean_distance(a, b),
            DistanceMetric::DotProduct => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                if dot.is_finite() { -dot } else { f32::MAX }
            }
        }
    }

    /// Number of active (non-tombstoned) vectors stored in the index
    pub fn len(&self) -> usize {
        self.nodes.len().saturating_sub(self.tombstones.len())
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Mark a vector as deleted via tombstone (soft delete in O(1))
    pub fn mark_deleted(&mut self, id: u64) -> bool {
        if self.nodes.contains_key(&id) {
            self.tombstones.insert(id)
        } else {
            false
        }
    }

    /// Check if a node ID is tombstoned/deleted
    pub fn is_deleted(&self, id: u64) -> bool {
        self.tombstones.contains(&id)
    }

    /// Purge all tombstoned nodes and re-wire graph connections (vacuum)
    pub fn vacuum(&mut self) -> usize {
        let dead_ids: Vec<u64> = self.tombstones.drain().collect();
        let count = dead_ids.len();
        for id in dead_ids {
            self.remove_vector(id);
        }
        count
    }

    /// Remove a vector node from HNSW index by ID
    pub fn remove_vector(&mut self, id: u64) -> bool {
        if self.nodes.remove(&id).is_some() {
            for node in self.nodes.values_mut() {
                for neighbors in node.layer_neighbors.iter_mut() {
                    neighbors.retain(|&nid| nid != id);
                }
            }
            if self.entry_point == Some(id) {
                let mut best_ep = None;
                let mut best_level = 0;
                for (&nid, node) in &self.nodes {
                    if best_ep.is_none() || node.top_layer > best_level {
                        best_ep = Some(nid);
                        best_level = node.top_layer;
                    }
                }
                self.entry_point = best_ep;
                self.max_level = best_level;
            }
            true
        } else {
            false
        }
    }

    /// Generate uniform pseudo-random float in (0.0, 1.0) using SplitMix64
    pub fn next_random_f64(&mut self) -> f64 {
        self.rng_state = self.rng_state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        let r = z ^ (z >> 31);
        let val = (r as f64) / (u64::MAX as f64);
        val.clamp(0.0000001, 0.9999999)
    }

    /// Generate random layer level for new node using uniform logarithmic scaling (Malkov & Yashunin, 2018)
    fn random_level(&mut self) -> usize {
        let rand_val = self.next_random_f64();
        let level = (-rand_val.ln() * self.ml).floor() as usize;
        level.min(16)
    }

    /// Search layer for nearest neighbors
    fn search_layer(
        &self,
        query: &[f32],
        entry_points: &[u64],
        ef: usize,
        layer: usize,
    ) -> Vec<(u64, f32)> {
        self.search_layer_with_metric(query, entry_points, ef, layer, self.metric)
    }

    /// Search layer for nearest neighbors using an explicit metric override
    fn search_layer_with_metric(
        &self,
        query: &[f32],
        entry_points: &[u64],
        ef: usize,
        layer: usize,
        metric: DistanceMetric,
    ) -> Vec<(u64, f32)> {
        let mut visited: HashSet<u64> = entry_points.iter().copied().collect();
        let mut candidates = BinaryHeap::new(); // Min-heap: closest explored first
        let mut nearest = BinaryHeap::new(); // Max-heap: furthest on top for eviction

        for &ep in entry_points {
            if let Some(node) = self.nodes.get(&ep) {
                let dist = self.compute_node_distance(query, node, metric);
                candidates.push(MinCandidate { id: ep, distance: dist });
                nearest.push(MaxCandidate { id: ep, distance: dist });
            }
        }

        while let Some(curr) = candidates.pop() {
            let furthest_dist = nearest.peek().map(|c| c.distance).unwrap_or(f32::MAX);
            if curr.distance > furthest_dist && nearest.len() >= ef {
                break;
            }

            if let Some(node) = self.nodes.get(&curr.id) {
                if layer < node.layer_neighbors.len() {
                    for &neighbor_id in &node.layer_neighbors[layer] {
                        if visited.insert(neighbor_id) {
                            if let Some(neighbor) = self.nodes.get(&neighbor_id) {
                                let dist = self.compute_node_distance(query, neighbor, metric);
                                if dist < furthest_dist || nearest.len() < ef {
                                    candidates.push(MinCandidate {
                                        id: neighbor_id,
                                        distance: dist,
                                    });
                                    nearest.push(MaxCandidate {
                                        id: neighbor_id,
                                        distance: dist,
                                    });

                                    if nearest.len() > ef {
                                        nearest.pop(); // Evict furthest
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut sorted: Vec<(u64, f32)> = nearest
            .into_iter()
            .map(|c| (c.id, c.distance))
            .collect();
        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        sorted
    }

    /// Search K-nearest neighbors for a query vector with optional candidate ID pre-filtering
    pub fn search_knn_filtered(
        &self,
        query: &[f32],
        k: usize,
        metric: DistanceMetric,
        filter: Option<&std::collections::HashSet<u64>>,
    ) -> Result<Vec<(u64, f32)>> {
        if query.len() != self.dimensions {
            return Err(Error::DimensionMismatch(self.dimensions, query.len()));
        }

        if k == 0 || self.nodes.is_empty() {
            return Ok(Vec::new());
        }

        let mut curr_entry = match self.entry_point {
            Some(ep) => ep,
            None => return Ok(Vec::new()),
        };

        // 1. Greedily traverse down to layer 0
        for l in (1..=self.max_level).rev() {
            let mut changed = true;
            while changed {
                changed = false;
                if let Some(curr_node) = self.nodes.get(&curr_entry) {
                    let curr_dist = self.compute_node_distance(query, curr_node, metric);

                    if l < curr_node.layer_neighbors.len() {
                        for &neighbor_id in &curr_node.layer_neighbors[l] {
                            if let Some(neighbor) = self.nodes.get(&neighbor_id) {
                                let dist = self.compute_node_distance(query, neighbor, metric);
                                if dist < curr_dist {
                                    curr_entry = neighbor_id;
                                    changed = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Search bottom layer 0 with ef_search (expanded if filtering is active)
        let ef_multiplier = if filter.is_some() { 4 } else { 1 };
        let ef = (self.ef_search.max(k)) * ef_multiplier;
        let candidates = self.search_layer_with_metric(query, &[curr_entry], ef, 0, metric);

        let mut results = Vec::with_capacity(k);
        for (id, dist) in candidates.into_iter() {
            if !self.tombstones.contains(&id) {
                if let Some(allowed) = filter {
                    if !allowed.contains(&id) {
                        continue;
                    }
                }
                results.push((id, dist));
                if results.len() >= k {
                    break;
                }
            }
        }

        Ok(results)
    }

    /// Search K-nearest neighbors for a query vector
    pub fn search_knn(
        &self,
        query: &[f32],
        k: usize,
        metric: DistanceMetric,
    ) -> Result<Vec<(u64, f32)>> {
        self.search_knn_filtered(query, k, metric, None)
    }
}

impl VectorIndexEngine for HnswIndex {
    fn insert_vector(&mut self, id: u64, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(Error::DimensionMismatch(self.dimensions, vector.len()));
        }

        // Clean up previous connections if ID already exists
        if self.nodes.contains_key(&id) {
            self.remove_vector(id);
        }

        let node_level = self.random_level();
        let mut layer_neighbors = vec![Vec::new(); node_level + 1];

        if let Some(mut curr_entry) = self.entry_point {
            let curr_max_level = self.max_level;

            // 1. Greedily traverse down from top layer to node_level + 1
            if curr_max_level > node_level {
                for l in (node_level + 1..=curr_max_level).rev() {
                    let mut changed = true;
                    while changed {
                        changed = false;
                        if let Some(curr_node) = self.nodes.get(&curr_entry) {
                            let curr_dist = self.compute_node_distance(vector, curr_node, self.metric);
                            if l < curr_node.layer_neighbors.len() {
                                for &neighbor_id in &curr_node.layer_neighbors[l] {
                                    if let Some(neighbor) = self.nodes.get(&neighbor_id) {
                                        let dist = self.compute_node_distance(vector, neighbor, self.metric);
                                        if dist < curr_dist {
                                            curr_entry = neighbor_id;
                                            changed = true;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 2. From min(node_level, curr_max_level) down to 0, find neighbors and connect
            let mut ep = vec![curr_entry];
            for l in (0..=node_level.min(curr_max_level)).rev() {
                let candidates = self.search_layer(vector, &ep, self.ef_construction, l);
                let max_neighbors = if l == 0 { self.m0 } else { self.m };

                let neighbor_ids: Vec<u64> = candidates
                    .iter()
                    .take(max_neighbors)
                    .map(|(nid, _)| *nid)
                    .collect();

                layer_neighbors[l] = neighbor_ids.clone();

                // Connect bidirectionally
                for &nid in &neighbor_ids {
                    if let Some(neighbor_node) = self.nodes.get_mut(&nid) {
                        if l < neighbor_node.layer_neighbors.len() {
                            neighbor_node.layer_neighbors[l].push(id);
                            if neighbor_node.layer_neighbors[l].len() > max_neighbors {
                                neighbor_node.layer_neighbors[l].truncate(max_neighbors);
                            }
                        }
                    }
                }

                ep = neighbor_ids;
            }

            if node_level > self.max_level {
                self.max_level = node_level;
                self.entry_point = Some(id);
            }
        } else {
            // First node in index
            self.entry_point = Some(id);
            self.max_level = node_level;
        }

        let quantized = if self.config.quantize_sq8 {
            Some(crate::vector::QuantizedVector8::quantize(vector))
        } else {
            None
        };

        let node = HnswNode {
            id,
            vector: vector.to_vec(),
            top_layer: node_level,
            layer_neighbors,
            quantized,
        };

        self.nodes.insert(id, node);
        Ok(())
    }

    fn search_knn(
        &self,
        query: &[f32],
        k: usize,
        metric: DistanceMetric,
    ) -> Result<Vec<(u64, f32)>> {
        self.search_knn_filtered(query, k, metric, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hnsw_knn_search() {
        let mut index = HnswIndex::new(3, DistanceMetric::Cosine);

        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let v3 = vec![0.0, 0.0, 1.0];
        let v4 = vec![0.95, 0.05, 0.0]; // Very close to v1

        index.insert_vector(1, &v1).expect("Insert v1");
        index.insert_vector(2, &v2).expect("Insert v2");
        index.insert_vector(3, &v3).expect("Insert v3");
        index.insert_vector(4, &v4).expect("Insert v4");

        assert_eq!(index.len(), 4);

        // Search for vector closest to v1
        let query = vec![0.98, 0.02, 0.0];
        let neighbors = index.search_knn(&query, 2, DistanceMetric::Cosine).expect("KNN search");

        assert_eq!(neighbors.len(), 2);
        let top_ids: Vec<u64> = neighbors.iter().map(|(id, _)| *id).collect();
        assert!(top_ids.contains(&1) && top_ids.contains(&4), "Expected [1, 4] in top 2, got {:?}", top_ids);
    }

    #[test]
    fn test_hnsw_tombstone_and_vacuum() {
        let mut index = HnswIndex::new(3, DistanceMetric::Cosine);

        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.98, 0.02, 0.0];
        let v3 = vec![0.0, 1.0, 0.0];

        index.insert_vector(1, &v1).expect("Insert v1");
        index.insert_vector(2, &v2).expect("Insert v2");
        index.insert_vector(3, &v3).expect("Insert v3");

        assert_eq!(index.len(), 3);

        // Soft delete v1 via tombstone
        assert!(index.mark_deleted(1));
        assert!(index.is_deleted(1));
        assert_eq!(index.len(), 2);

        // Search for vector closest to v1: v1 must NOT be returned
        let query = vec![1.0, 0.0, 0.0];
        let results = index.search_knn(&query, 2, DistanceMetric::Cosine).expect("KNN search");
        assert_eq!(results[0].0, 2); // v2 is closest non-deleted node
        assert!(!results.iter().any(|(id, _)| *id == 1));

        // Vacuum should purge v1 from physical storage
        let purged = index.vacuum();
        assert_eq!(purged, 1);
        assert_eq!(index.len(), 2);
        assert!(!index.nodes.contains_key(&1));
    }

    #[test]
    fn test_hnsw_random_level_logarithmic_distribution() {
        let mut index = HnswIndex::with_seed(3, DistanceMetric::Cosine, 42);
        let mut level_counts = [0usize; 17];

        // Sample 1,000 layer levels
        for _ in 0..1000 {
            let level = index.random_level();
            assert!(level <= 16);
            level_counts[level] += 1;
        }

        // Level 0 must have the highest frequency (base layer)
        assert!(level_counts[0] > level_counts[1], "Level 0 must dominate");
        // Layer counts should exhibit monotonic decay
        assert!(level_counts[1] >= level_counts[2]);
        // Total samples at higher levels should be smaller
        let higher_levels: usize = level_counts[3..].iter().sum();
        assert!(higher_levels < level_counts[0]);
    }
}
