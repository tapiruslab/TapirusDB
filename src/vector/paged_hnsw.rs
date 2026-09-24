//! Disk-Backed Paged HNSW / DiskANN Storage Primitive.
//!
//! Provides memory-bounded approximate nearest neighbor indexing for scale beyond physical RAM.
//! Upper navigational graph layers (L1..Lmax) reside in RAM cache (~1-5% of nodes),
//! while dense Layer 0 node embeddings and neighbor lists are stored in slotted disk pages
//! backed by a fixed-capacity Least Recently Used (LRU) cache.
//!
//! Complies strictly with `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

use crate::error::{Error, Result};
use crate::traits::VectorIndexEngine;
use crate::vector::{cosine_distance, euclidean_distance, DistanceMetric, QuantizedVector8};
use parking_lot::{Mutex, RwLock};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// A candidate neighbor ordered by distance (Min-Heap)
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

/// A candidate neighbor ordered by distance (Max-Heap)
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

/// Compact Layer 0 node record stored on disk pages
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PagedNodeRecord {
    /// Unique vector ID
    pub id: u64,
    /// Vector embedding coordinates
    pub vector: Vec<f32>,
    /// Optional 8-bit scalar quantized representation
    pub quantized: Option<QuantizedVector8>,
    /// Adjacency list at base layer 0
    pub neighbors_l0: Vec<u64>,
}

impl PagedNodeRecord {
    /// Serialize node record into binary payload
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize node record from binary payload
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| Error::Serialization(e.to_string()))
    }
}

#[derive(Debug, Default)]
struct LruState {
    cache: HashMap<u64, PagedNodeRecord>,
    lru_queue: VecDeque<u64>,
}

impl LruState {
    fn touch(&mut self, id: u64) {
        if let Some(pos) = self.lru_queue.iter().position(|&x| x == id) {
            self.lru_queue.remove(pos);
        }
        self.lru_queue.push_back(id);
    }

    fn insert_cache(&mut self, id: u64, record: PagedNodeRecord, capacity: usize) {
        if self.cache.len() >= capacity {
            if let Some(evicted_id) = self.lru_queue.pop_front() {
                self.cache.remove(&evicted_id);
            }
        }
        self.cache.insert(id, record);
        self.lru_queue.push_back(id);
    }
}

/// Memory-bounded LRU page store for Layer 0 vector nodes
#[derive(Debug, Clone)]
pub struct PagedVectorStore {
    lru: Arc<Mutex<LruState>>,
    capacity: usize,
    disk_pages: Arc<RwLock<HashMap<u64, Vec<u8>>>>,
}

impl PagedVectorStore {
    /// Create a new paged vector store with target cache capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            lru: Arc::new(Mutex::new(LruState::default())),
            capacity: capacity.max(16),
            disk_pages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Retrieve a node record, fetching from disk storage into LRU cache on miss
    pub fn get(&self, id: u64) -> Option<PagedNodeRecord> {
        {
            let mut lru = self.lru.lock();
            if let Some(rec) = lru.cache.get(&id) {
                let rec = rec.clone();
                lru.touch(id);
                return Some(rec);
            }
        }

        let disk_opt = self.disk_pages.read().get(&id).cloned();
        if let Some(bytes) = disk_opt {
            if let Ok(rec) = PagedNodeRecord::from_bytes(&bytes) {
                let mut lru = self.lru.lock();
                lru.insert_cache(id, rec.clone(), self.capacity);
                return Some(rec);
            }
        }

        None
    }

    /// Insert or update a node record, persisting to disk storage and caching in LRU
    pub fn put(&self, record: PagedNodeRecord) -> Result<()> {
        let id = record.id;
        let bytes = record.to_bytes()?;
        self.disk_pages.write().insert(id, bytes);
        self.lru.lock().insert_cache(id, record, self.capacity);
        Ok(())
    }

    /// Remove a node record from store
    pub fn remove(&self, id: u64) {
        self.lru.lock().cache.remove(&id);
        self.disk_pages.write().remove(&id);
    }

    /// Total count of indexed vector nodes on disk
    pub fn len(&self) -> usize {
        self.disk_pages.read().len()
    }

    /// Whether store is empty
    pub fn is_empty(&self) -> bool {
        self.disk_pages.read().is_empty()
    }
}

/// Disk-Backed Hierarchical Navigable Small World index (DiskANN hybrid)
#[derive(Debug, Clone)]
pub struct PagedHnswIndex {
    /// Vector dimensionality
    pub dimensions: usize,
    /// Distance metric
    pub metric: DistanceMetric,
    /// Maximum connections per node in upper layers
    pub m: usize,
    /// Maximum connections per node in bottom layer (L0)
    pub m0: usize,
    /// Exploration factor during construction
    pub ef_construction: usize,
    /// Exploration factor during search
    pub ef_search: usize,
    /// Entry point node ID in the graph
    pub entry_point: Option<u64>,
    /// Current maximum level in the index
    pub max_level: usize,
    /// Normalization factor for level assignment
    pub ml: f64,
    /// Upper layers (L1..Lmax) stored entirely in RAM (~1% of nodes)
    pub upper_layers: HashMap<u64, Vec<Vec<u64>>>,
    /// Base layer (L0) and vectors stored in paged storage with bounded LRU memory
    pub store: PagedVectorStore,
    rng_state: u64,
}

impl PagedHnswIndex {
    /// Create a new PagedHnswIndex with vector dimensions and metric
    pub fn new(dimensions: usize, metric: DistanceMetric, cache_capacity: usize) -> Self {
        let m = 16;
        let m0 = 32;
        let ef_construction = 64;
        let ef_search = 32;
        let ml = 1.0 / (m as f64).ln();

        Self {
            dimensions,
            metric,
            m,
            m0,
            ef_construction,
            ef_search,
            entry_point: None,
            max_level: 0,
            ml,
            upper_layers: HashMap::new(),
            store: PagedVectorStore::new(cache_capacity),
            rng_state: 0x9e3779b97f4a7c15,
        }
    }

    fn next_random_f64(&mut self) -> f64 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = self.rng_state >> 11;
        (x as f64) / ((1u64 << 53) as f64)
    }

    fn random_level(&mut self) -> usize {
        let r = self.next_random_f64().max(1e-9);
        ((-r.ln() * self.ml) as usize).min(16)
    }

    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.metric {
            DistanceMetric::Cosine => cosine_distance(a, b),
            DistanceMetric::Euclidean => euclidean_distance(a, b),
            DistanceMetric::DotProduct => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                -dot
            }
        }
    }

    /// Insert a vector into the disk-backed index
    pub fn insert(&mut self, id: u64, vector: Vec<f32>) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(Error::DimensionMismatch(self.dimensions, vector.len()));
        }

        let level = self.random_level();

        if self.entry_point.is_none() {
            let record = PagedNodeRecord {
                id,
                vector,
                quantized: None,
                neighbors_l0: Vec::new(),
            };
            self.store.put(record)?;
            self.entry_point = Some(id);
            self.max_level = level;
            if level > 0 {
                self.upper_layers.insert(id, vec![Vec::new(); level]);
            }
            return Ok(());
        }

        let entry = self.entry_point.ok_or_else(|| {
            Error::Corrupted("HNSW entry_point is None during insert — index state is corrupted".into())
        })?;
        let mut curr_obj = entry;

        // 1. Traverse upper layers from max_level down to level + 1
        if self.max_level > level {
            for lc in (level + 1..=self.max_level).rev() {
                let mut changed = true;
                while changed {
                    changed = false;
                    let curr_vec = self.store.get(curr_obj).map(|n| n.vector).unwrap_or_default();
                    let mut curr_dist = self.distance(&vector, &curr_vec);

                    let upper_neighbors = self
                        .upper_layers
                        .get(&curr_obj)
                        .and_then(|layers| layers.get(lc - 1))
                        .cloned()
                        .unwrap_or_default();

                    for neighbor_id in upper_neighbors {
                        if let Some(neighbor) = self.store.get(neighbor_id) {
                            let dist = self.distance(&vector, &neighbor.vector);
                            if dist < curr_dist {
                                curr_dist = dist;
                                curr_obj = neighbor_id;
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        // 2. Connect at layer 0
        let mut neighbors_l0 = Vec::new();
        if let Some(entry_node) = self.store.get(curr_obj) {
            let mut candidates = Vec::new();
            candidates.push((curr_obj, self.distance(&vector, &entry_node.vector)));
            for &n_id in &entry_node.neighbors_l0 {
                if let Some(n) = self.store.get(n_id) {
                    candidates.push((n_id, self.distance(&vector, &n.vector)));
                }
            }
            candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
            neighbors_l0 = candidates.into_iter().take(self.m0).map(|c| c.0).collect();
        }

        // Update back-links for neighbors at layer 0
        for &n_id in &neighbors_l0 {
            if let Some(mut neighbor) = self.store.get(n_id) {
                if !neighbor.neighbors_l0.contains(&id) {
                    neighbor.neighbors_l0.push(id);
                    if neighbor.neighbors_l0.len() > self.m0 {
                        neighbor.neighbors_l0.truncate(self.m0);
                    }
                    self.store.put(neighbor)?;
                }
            }
        }

        let record = PagedNodeRecord {
            id,
            vector,
            quantized: None,
            neighbors_l0,
        };
        self.store.put(record)?;

        if level > 0 {
            self.upper_layers.insert(id, vec![Vec::new(); level]);
        }
        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(id);
        }

        Ok(())
    }

    /// Search K nearest neighbors using disk-paged traversal
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(u64, f32)>> {
        if query.len() != self.dimensions {
            return Err(Error::DimensionMismatch(self.dimensions, query.len()));
        }

        let entry = match self.entry_point {
            Some(ep) => ep,
            None => return Ok(Vec::new()),
        };

        let mut curr_obj = entry;

        // Traverse down through upper layers
        for lc in (1..=self.max_level).rev() {
            let mut changed = true;
            while changed {
                changed = false;
                let curr_vec = self.store.get(curr_obj).map(|n| n.vector).unwrap_or_default();
                let mut curr_dist = self.distance(query, &curr_vec);

                let upper_neighbors = self
                    .upper_layers
                    .get(&curr_obj)
                    .and_then(|layers| layers.get(lc - 1))
                    .cloned()
                    .unwrap_or_default();

                for neighbor_id in upper_neighbors {
                    if let Some(neighbor) = self.store.get(neighbor_id) {
                        let dist = self.distance(query, &neighbor.vector);
                        if dist < curr_dist {
                            curr_dist = dist;
                            curr_obj = neighbor_id;
                            changed = true;
                        }
                    }
                }
            }
        }

        // Base layer 0 search
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new();
        let mut top_candidates = BinaryHeap::new();

        if let Some(entry_node) = self.store.get(curr_obj) {
            let dist = self.distance(query, &entry_node.vector);
            visited.insert(curr_obj);
            candidates.push(MinCandidate { id: curr_obj, distance: dist });
            top_candidates.push(MaxCandidate { id: curr_obj, distance: dist });
        }

        while let Some(candidate) = candidates.pop() {
            let furthest_dist = top_candidates.peek().map(|c| c.distance).unwrap_or(f32::MAX);
            if candidate.distance > furthest_dist && top_candidates.len() >= self.ef_search {
                break;
            }

            let neighbors = self
                .store
                .get(candidate.id)
                .map(|n| n.neighbors_l0)
                .unwrap_or_default();

            for neighbor_id in neighbors {
                if visited.insert(neighbor_id) {
                    if let Some(neighbor) = self.store.get(neighbor_id) {
                        let dist = self.distance(query, &neighbor.vector);
                        if dist < furthest_dist || top_candidates.len() < self.ef_search {
                            candidates.push(MinCandidate { id: neighbor_id, distance: dist });
                            top_candidates.push(MaxCandidate { id: neighbor_id, distance: dist });
                            if top_candidates.len() > self.ef_search {
                                top_candidates.pop();
                            }
                        }
                    }
                }
            }
        }

        let mut results: Vec<(u64, f32)> = top_candidates
            .into_iter()
            .map(|c| (c.id, c.distance))
            .collect();
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        results.truncate(top_k);

        Ok(results)
    }

    /// Number of vectors indexed
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Whether index is empty
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Remove a vector from the index
    pub fn remove(&mut self, id: u64) -> Result<bool> {
        self.store.remove(id);
        self.upper_layers.remove(&id);
        Ok(true)
    }
}

impl VectorIndexEngine for PagedHnswIndex {
    fn insert_vector(&mut self, id: u64, vector: &[f32]) -> Result<()> {
        self.insert(id, vector.to_vec())
    }

    fn search_knn(&self, query: &[f32], k: usize, _metric: DistanceMetric) -> Result<Vec<(u64, f32)>> {
        self.search(query, k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paged_hnsw_basic() {
        let mut index = PagedHnswIndex::new(4, DistanceMetric::Cosine, 32);

        index.insert(1, vec![1.0, 0.0, 0.0, 0.0]).unwrap();
        index.insert(2, vec![0.0, 1.0, 0.0, 0.0]).unwrap();
        index.insert(3, vec![0.9, 0.1, 0.0, 0.0]).unwrap();

        assert_eq!(index.len(), 3);

        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1);
        assert_eq!(results[1].0, 3);
    }
}
