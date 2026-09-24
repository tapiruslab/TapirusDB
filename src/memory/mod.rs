//! Native Embedded AI Agent Memory Subsystem for TapirusDB.
//!
//! Provides a unified, ultra-lightweight memory engine combining:
//! - **Episodic & Semantic Storage**: Chronological observations and facts.
//! - **Hybrid Retrieval**: Dense Vector Similarity + Pure Safe-Rust BM25 Lexical Keyword Search.
//! - **Temporal Recency Decay**: Time-decay scoring ($e^{-\lambda \Delta t}$) with customizable half-life.
//! - **Graph Memory Association**: Associative links connecting related memories and entities.
//! - **Edge Microprocessor Optimization**: Ultra-low memory footprint (< 4 MB) for low-end CPUs and WASM.

pub mod bm25;
pub mod embedder;

pub use bm25::{reciprocal_rank_fusion, Bm25Index, Bm25Params};
pub use embedder::{DeterministicHashEmbedder, EmbeddingEngine, HttpEmbeddingEngine};

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// An individual memory unit recorded by an AI agent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique 64-bit Memory ID
    pub id: u64,
    /// Memory content (text observation, user dialogue, learned fact, or summary)
    pub content: String,
    /// High-dimensional dense embedding representation (optional)
    pub vector: Option<Vec<f32>>,
    /// Timestamp when this memory occurred (Unix epoch seconds)
    pub timestamp: u64,
    /// Timestamp when this memory was last retrieved/accessed
    pub last_accessed: u64,
    /// Memory importance / intrinsic priority score in $[0.0, 1.0]$ (default: 0.5)
    pub importance: f32,
    /// Access counter tracking how often this memory has been recalled
    pub access_count: u32,
    /// Semantic tags or categorical labels
    pub tags: Vec<String>,
    /// Associated memory IDs connected in the knowledge graph
    pub associations: Vec<u64>,
    /// Multi-agent namespace isolation (e.g. "agent_alpha", "crew_finance")
    #[serde(default)]
    pub namespace: Option<String>,
    /// Session or thread conversation identifier (e.g. "chat_123")
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Parameters and weights controlling multi-modal memory recall
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecallFilter {
    /// Weight assigned to dense vector semantic similarity (default: 0.45)
    pub vector_weight: f32,
    /// Weight assigned to BM25 lexical keyword matching (default: 0.35)
    pub bm25_weight: f32,
    /// Weight assigned to temporal recency decay (default: 0.10)
    pub recency_weight: f32,
    /// Weight assigned to intrinsic memory importance (default: 0.10)
    pub importance_weight: f32,
    /// Half-life in seconds for exponential temporal decay (default: 30 days = 2,592,000s)
    pub half_life_seconds: u64,
    /// Optional tag filters (must contain at least one of these tags if specified)
    pub required_tags: Vec<String>,
    /// Minimum combined score cutoff $[0.0, 1.0]$
    pub min_score: f32,
    /// Graph expansion hops to automatically include connected associative memories (default: 0)
    pub graph_expansion_hops: usize,
    /// Multi-agent namespace scope filter
    #[serde(default)]
    pub namespace: Option<String>,
    /// Session conversation scope filter
    #[serde(default)]
    pub session_id: Option<String>,
}

impl Default for MemoryRecallFilter {
    fn default() -> Self {
        Self {
            vector_weight: 0.45,
            bm25_weight: 0.35,
            recency_weight: 0.10,
            importance_weight: 0.10,
            half_life_seconds: 30 * 86400, // 30 days
            required_tags: Vec::new(),
            min_score: 0.0,
            graph_expansion_hops: 0,
            namespace: None,
            session_id: None,
        }
    }
}

impl MemoryRecallFilter {
    /// Configure custom weights for vector, bm25, recency, and importance
    pub fn with_weights(
        mut self,
        vector: f32,
        bm25: f32,
        recency: f32,
        importance: f32,
    ) -> Self {
        self.vector_weight = vector;
        self.bm25_weight = bm25;
        self.recency_weight = recency;
        self.importance_weight = importance;
        self
    }

    /// Set half-life duration in seconds for temporal decay
    pub fn with_half_life(mut self, seconds: u64) -> Self {
        self.half_life_seconds = seconds;
        self
    }

    /// Set required tags
    pub fn with_tags(mut self, tags: &[&str]) -> Self {
        self.required_tags = tags.iter().map(|t| t.to_string()).collect();
        self
    }

    /// Set graph expansion hops for associative recall
    pub fn with_graph_hops(mut self, hops: usize) -> Self {
        self.graph_expansion_hops = hops;
        self
    }
}

/// A recalled memory result containing the entry and its constituent scores
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecallResult {
    /// The retrieved memory entry
    pub entry: MemoryEntry,
    /// Total combined hybrid score
    pub combined_score: f32,
    /// Semantic vector similarity component $[0.0, 1.0]$
    pub semantic_score: f32,
    /// BM25 lexical keyword component $[0.0, 1.0]$
    pub lexical_score: f32,
    /// Temporal recency decay score $[0.0, 1.0]$
    pub recency_score: f32,
    /// Whether this result was retrieved via graph association expansion
    pub is_associative: bool,
}

/// The embedded AI Agent Memory Engine
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MemoryEngine {
    entries: HashMap<u64, MemoryEntry>,
    bm25: Bm25Index,
    next_id: u64,
}

impl MemoryEngine {
    /// Create a new empty MemoryEngine
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            bm25: Bm25Index::new(),
            next_id: 1,
        }
    }

    /// Store a new memory into the engine with optional multi-agent namespace and session scope
    pub fn remember_scoped(
        &mut self,
        content: &str,
        vector: Option<&[f32]>,
        importance: f32,
        tags: &[&str],
        now_ts: u64,
        namespace: Option<&str>,
        session_id: Option<&str>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let entry = MemoryEntry {
            id,
            content: content.to_string(),
            vector: vector.map(|v| v.to_vec()),
            timestamp: now_ts,
            last_accessed: now_ts,
            importance: importance.clamp(0.0, 1.0),
            access_count: 0,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            associations: Vec::new(),
            namespace: namespace.map(|s| s.to_string()),
            session_id: session_id.map(|s| s.to_string()),
        };

        // Index in BM25
        self.bm25.index_document(id, content);
        self.entries.insert(id, entry);

        id
    }

    /// Restore a persisted memory entry from disk storage
    pub fn restore_entry(&mut self, entry: MemoryEntry) {
        if entry.id >= self.next_id {
            self.next_id = entry.id + 1;
        }
        self.bm25.index_document(entry.id, &entry.content);
        self.entries.insert(entry.id, entry);
    }

    /// Store a new memory into the engine (default namespace)
    pub fn remember(
        &mut self,
        content: &str,
        vector: Option<&[f32]>,
        importance: f32,
        tags: &[&str],
        now_ts: u64,
    ) -> u64 {
        self.remember_scoped(content, vector, importance, tags, now_ts, None, None)
    }

    /// Associate two memories together with a bidirectional link
    pub fn associate(&mut self, id_a: u64, id_b: u64) -> Result<()> {
        if !self.entries.contains_key(&id_a) {
            return Err(Error::Corrupted(format!("Memory #{id_a} not found")));
        }
        if !self.entries.contains_key(&id_b) {
            return Err(Error::Corrupted(format!("Memory #{id_b} not found")));
        }

        if let Some(entry_a) = self.entries.get_mut(&id_a) {
            if !entry_a.associations.contains(&id_b) {
                entry_a.associations.push(id_b);
            }
        }
        if let Some(entry_b) = self.entries.get_mut(&id_b) {
            if !entry_b.associations.contains(&id_a) {
                entry_b.associations.push(id_a);
            }
        }

        Ok(())
    }

    /// Retrieve a single memory by ID and increment its access count
    pub fn get_memory_mut(&mut self, id: u64, now_ts: u64) -> Option<&mut MemoryEntry> {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.last_accessed = now_ts;
            entry.access_count += 1;
            Some(entry)
        } else {
            None
        }
    }

    /// Retrieve a single memory by ID without mutating access metrics
    pub fn get_memory(&self, id: u64) -> Option<&MemoryEntry> {
        self.entries.get(&id)
    }

    /// Return the total count of stored memories
    pub fn memory_count(&self) -> usize {
        self.entries.len()
    }

    /// Recall relevant memories using hybrid Vector + BM25 + Temporal Decay + Graph expansion
    pub fn recall(
        &mut self,
        query_text: Option<&str>,
        query_vector: Option<&[f32]>,
        limit: usize,
        filter: &MemoryRecallFilter,
        now_ts: u64,
    ) -> Vec<MemoryRecallResult> {
        if self.entries.is_empty() || limit == 0 {
            return Vec::new();
        }

        // 1. Evaluate BM25 scores if query text is present
        let bm25_scores = if let Some(q_text) = query_text {
            let matches = self.bm25.search(q_text, self.entries.len());
            matches.into_iter().collect::<HashMap<u64, f32>>()
        } else {
            HashMap::new()
        };

        let lambda = if filter.half_life_seconds > 0 {
            2.0f32.ln() / (filter.half_life_seconds as f32)
        } else {
            0.0
        };

        let mut scored_results: Vec<MemoryRecallResult> = Vec::with_capacity(self.entries.len());

        // 2. Score candidate memories
        for entry in self.entries.values() {
            // Check namespace filter
            if let Some(ref req_ns) = filter.namespace {
                match &entry.namespace {
                    Some(ns) if ns.eq_ignore_ascii_case(req_ns) => {}
                    _ => continue,
                }
            }

            // Check session filter
            if let Some(ref req_sess) = filter.session_id {
                match &entry.session_id {
                    Some(sess) if sess == req_sess => {}
                    _ => continue,
                }
            }

            // Check required tags filter
            if !filter.required_tags.is_empty() {
                let matches_tag = filter
                    .required_tags
                    .iter()
                    .any(|req_tag| entry.tags.iter().any(|t| t.eq_ignore_ascii_case(req_tag)));
                if !matches_tag {
                    continue;
                }
            }

            // A. Semantic Vector Score
            let semantic_score = match (query_vector, &entry.vector) {
                (Some(q_vec), Some(e_vec)) => cosine_similarity(q_vec, e_vec).max(0.0),
                _ => 0.0,
            };

            // B. Lexical BM25 Score
            let lexical_score = *bm25_scores.get(&entry.id).unwrap_or(&0.0);

            // C. Temporal Recency Decay Score: e^(-lambda * dt)
            let dt = now_ts.saturating_sub(entry.timestamp) as f32;
            let recency_score = (-lambda * dt).exp().clamp(0.0, 1.0);

            // D. Combined Weighted Score
            let combined_score = filter.vector_weight * semantic_score
                + filter.bm25_weight * lexical_score
                + filter.recency_weight * recency_score
                + filter.importance_weight * entry.importance;

            let has_query = query_text.is_some() || query_vector.is_some();
            let matches_query = if has_query {
                (query_text.is_some() && lexical_score > 0.0)
                    || (query_vector.is_some() && semantic_score > 0.0)
            } else {
                true
            };

            if matches_query && combined_score >= filter.min_score {
                scored_results.push(MemoryRecallResult {
                    entry: entry.clone(),
                    combined_score,
                    semantic_score,
                    lexical_score,
                    recency_score,
                    is_associative: false,
                });
            }
        }

        // Sort descending by combined score
        scored_results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 3. Graph Associative Expansion
        if filter.graph_expansion_hops > 0 && !scored_results.is_empty() {
            let mut existing_ids: HashSet<u64> = scored_results.iter().map(|r| r.entry.id).collect();
            let mut associative_results: Vec<MemoryRecallResult> = Vec::new();

            // Take the top candidate memories and expand their associations
            let base_count = scored_results.len().min(limit);
            for i in 0..base_count {
                let parent = &scored_results[i];
                for &assoc_id in &parent.entry.associations {
                    if !existing_ids.contains(&assoc_id) {
                        if let Some(assoc_entry) = self.entries.get(&assoc_id) {
                            if let Some(ref req_ns) = filter.namespace {
                                match &assoc_entry.namespace {
                                    Some(ns) if ns.eq_ignore_ascii_case(req_ns) => {}
                                    _ => continue,
                                }
                            }
                            if let Some(ref req_sess) = filter.session_id {
                                match &assoc_entry.session_id {
                                    Some(sess) if sess == req_sess => {}
                                    _ => continue,
                                }
                            }

                            existing_ids.insert(assoc_id);
                            // Discount associative memory score by 0.85
                            let assoc_score = parent.combined_score * 0.85;
                            associative_results.push(MemoryRecallResult {
                                entry: assoc_entry.clone(),
                                combined_score: assoc_score,
                                semantic_score: 0.0,
                                lexical_score: 0.0,
                                recency_score: 0.0,
                                is_associative: true,
                            });
                        }
                    }
                }
            }

            scored_results.extend(associative_results);
            scored_results.sort_by(|a, b| {
                b.combined_score
                    .partial_cmp(&a.combined_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        scored_results.truncate(limit);

        // Update access metrics for recalled memories
        for res in &scored_results {
            if let Some(entry) = self.entries.get_mut(&res.entry.id) {
                entry.last_accessed = now_ts;
                entry.access_count += 1;
            }
        }

        scored_results
    }

    /// Recall memories using Reciprocal Rank Fusion (RRF) combining BM25 lexical and Vector semantic ranks
    pub fn recall_rrf(
        &mut self,
        query_text: &str,
        query_vector: Option<&[f32]>,
        limit: usize,
        rrf_k: f32,
        now_ts: u64,
    ) -> Vec<MemoryRecallResult> {
        // 1. BM25 search
        let bm25_hits = self.bm25.search(query_text, self.entries.len());
        let bm25_ids: Vec<u64> = bm25_hits.iter().map(|(id, _)| *id).collect();

        // 2. Vector search (if vector is provided)
        let mut vector_hits: Vec<(u64, f32)> = Vec::new();
        if let Some(q_vec) = query_vector {
            for (id, entry) in &self.entries {
                if let Some(e_vec) = &entry.vector {
                    if e_vec.len() == q_vec.len() {
                        let dot: f32 = q_vec.iter().zip(e_vec.iter()).map(|(a, b)| a * b).sum();
                        let norm_q: f32 = q_vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                        let norm_e: f32 = e_vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                        let sim = if norm_q > 1e-9 && norm_e > 1e-9 {
                            dot / (norm_q * norm_e)
                        } else {
                            0.0
                        };
                        vector_hits.push((*id, sim));
                    }
                }
            }
            vector_hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
        let vector_ids: Vec<u64> = vector_hits.iter().map(|(id, _)| *id).collect();

        // 3. Fused using RRF
        let mut ranked_lists: Vec<(&[u64], f32)> = Vec::new();
        if !bm25_ids.is_empty() {
            ranked_lists.push((&bm25_ids, 1.0));
        }
        if !vector_ids.is_empty() {
            ranked_lists.push((&vector_ids, 1.0));
        }

        let fused = bm25::reciprocal_rank_fusion(&ranked_lists, rrf_k, limit);

        let mut results = Vec::new();
        for (id, fused_score) in fused {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.last_accessed = now_ts;
                entry.access_count += 1;
                results.push(MemoryRecallResult {
                    entry: entry.clone(),
                    combined_score: fused_score,
                    semantic_score: 0.0,
                    lexical_score: 0.0,
                    recency_score: 0.0,
                    is_associative: false,
                });
            }
        }
        results
    }

    /// Prune decayed memories and return the list of deleted IDs
    pub fn prune_decayed_ids_at(
        &mut self,
        decay_threshold: f32,
        half_life_seconds: u64,
        now_ts: u64,
    ) -> Vec<u64> {
        let lambda = if half_life_seconds > 0 {
            2.0f32.ln() / (half_life_seconds as f32)
        } else {
            0.0
        };

        let mut to_remove: Vec<u64> = Vec::new();
        for (&id, entry) in &self.entries {
            let dt = now_ts.saturating_sub(entry.timestamp) as f32;
            let recency = (-lambda * dt).exp();
            let retention_score = 0.6 * entry.importance + 0.4 * recency;
            if retention_score < decay_threshold {
                to_remove.push(id);
            }
        }

        for &id in &to_remove {
            self.entries.remove(&id);
            self.bm25.remove_document(id);
        }

        // Clean up associations in remaining entries
        for entry in self.entries.values_mut() {
            entry.associations.retain(|id| !to_remove.contains(id));
        }

        to_remove
    }

    /// Prune memories whose retention score falls below `decay_threshold`
    pub fn prune_decayed_at(
        &mut self,
        decay_threshold: f32,
        half_life_seconds: u64,
        now_ts: u64,
    ) -> usize {
        self.prune_decayed_ids_at(decay_threshold, half_life_seconds, now_ts).len()
    }

    /// Prune memories whose retention score falls below `decay_threshold` (alias for prune_decayed_at)
    pub fn prune(
        &mut self,
        decay_threshold: f32,
        half_life_seconds: u64,
        now_ts: u64,
    ) -> usize {
        self.prune_decayed_at(decay_threshold, half_life_seconds, now_ts)
    }


    /// Explicitly remove and forget a memory by ID, purging it from BM25 and associative links
    pub fn forget(&mut self, id: u64) -> bool {
        if self.entries.remove(&id).is_some() {
            self.bm25.remove_document(id);
            for entry in self.entries.values_mut() {
                entry.associations.retain(|&assoc_id| assoc_id != id);
            }
            true
        } else {
            false
        }
    }

    /// Get reference to a memory entry by ID
    pub fn get(&self, id: u64) -> Option<&MemoryEntry> {
        self.entries.get(&id)
    }

    /// Return all memory entries
    pub fn all_entries(&self) -> &HashMap<u64, MemoryEntry> {
        &self.entries
    }
}

/// Compute cosine similarity between two normalized or unnormalized float vectors in Safe Rust
pub fn cosine_similarity(u: &[f32], v: &[f32]) -> f32 {
    if u.len() != v.len() || u.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_u = 0.0f32;
    let mut norm_v = 0.0f32;

    for i in 0..u.len() {
        dot += u[i] * v[i];
        norm_u += u[i] * u[i];
        norm_v += v[i] * v[i];
    }

    let denom = norm_u.sqrt() * norm_v.sqrt();
    if denom > 1e-8 {
        (dot / denom).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_remember_and_hybrid_recall() {
        let mut engine = MemoryEngine::new();
        let now = 1_000_000;

        let vec1 = vec![1.0, 0.0, 0.0];
        let vec2 = vec![0.0, 1.0, 0.0];

        let id1 = engine.remember(
            "User prefers replies in Bahasa Melayu and lives in Kuala Lumpur",
            Some(&vec1),
            0.9,
            &["preference", "language"],
            now,
        );

        let id2 = engine.remember(
            "User is building a high performance database engine called TapirusDB",
            Some(&vec2),
            0.8,
            &["project", "systems"],
            now,
        );

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // A. BM25 keyword query
        let filter = MemoryRecallFilter::default();
        let results = engine.recall(Some("Bahasa Melayu"), None, 5, &filter, now);
        assert!(!results.is_empty());
        assert_eq!(results[0].entry.id, id1);

        // B. Vector semantic query
        let results_vec = engine.recall(None, Some(&vec2), 5, &filter, now);
        assert!(!results_vec.is_empty());
        assert_eq!(results_vec[0].entry.id, id2);
    }

    #[test]
    fn test_temporal_recency_decay() {
        let mut engine = MemoryEngine::new();
        let t0 = 1_000_000;
        let half_life = 86400; // 1 day half life

        // Store old memory (7 days ago) with medium importance
        let _id_old = engine.remember("Saw an interesting article on databases", None, 0.5, &[], t0);

        // Store new memory (just now) with same importance
        let t_now = t0 + 7 * 86400;
        let id_new = engine.remember("Saw an interesting article on databases", None, 0.5, &[], t_now);

        let filter = MemoryRecallFilter::default()
            .with_weights(0.0, 0.4, 0.6, 0.0) // Give high weight to recency
            .with_half_life(half_life);

        let results = engine.recall(Some("article databases"), None, 2, &filter, t_now);
        assert_eq!(results.len(), 2);
        // The newer memory must rank higher due to temporal decay
        assert_eq!(results[0].entry.id, id_new);
        assert!(results[0].combined_score > results[1].combined_score);
    }

    #[test]
    fn test_graph_associative_recall() {
        let mut engine = MemoryEngine::new();
        let now = 1_000_000;

        let id1 = engine.remember("Ahmad Faiz is the architect of TapirusDB", None, 0.9, &[], now);
        let id2 = engine.remember("TapirusDB uses ChaCha20-Poly1305 AEAD page encryption", None, 0.8, &[], now);

        engine.associate(id1, id2).expect("Associate memories");

        // Query only mentions "Ahmad Faiz"
        let filter = MemoryRecallFilter::default().with_graph_hops(1);
        let results = engine.recall(Some("Ahmad Faiz"), None, 5, &filter, now);

        // Should retrieve id1 directly, and id2 via associative graph link!
        assert!(results.len() >= 2);
        assert_eq!(results[0].entry.id, id1);
        assert_eq!(results[1].entry.id, id2);
        assert!(results[1].is_associative);
    }
}
