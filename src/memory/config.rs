//! Runtime admission limits and retrieval diagnostics.
use serde::{Deserialize, Serialize};

/// Admission limits for materialized memory and derived indexes.
/// Estimates are conservative accounting units, not a measured process RSS cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    /// Maximum live source records, including records awaiting expiry purge.
    pub max_documents: usize,
    /// Maximum estimated bytes for sources, lexical and vector indexes.
    pub max_estimated_bytes: usize,
    /// Maximum UTF-8 content bytes in one source.
    pub max_document_bytes: usize,
    /// Maximum embedding dimensionality.
    pub max_vector_dimensions: usize,
    /// Maximum model/dimension spaces per database.
    pub max_vector_spaces: usize,
    /// Maximum documents accepted by one atomic batch.
    pub max_batch_documents: usize,
    /// Maximum candidates from either retrieval channel.
    pub max_candidates: usize,
    /// HNSW upper-layer neighbors.
    pub hnsw_m: usize,
    /// HNSW construction beam.
    pub ef_construction: usize,
    /// HNSW query beam.
    pub ef_search: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_documents: 1_000_000,
            max_estimated_bytes: 1024 * 1024 * 1024,
            max_document_bytes: 1024 * 1024,
            max_vector_dimensions: 4096,
            max_vector_spaces: 32,
            max_batch_documents: 10_000,
            max_candidates: 4096,
            hnsw_m: 16,
            ef_construction: 96,
            ef_search: 64,
        }
    }
}

impl MemoryConfig {
    /// Smaller budgets for an OS-based edge device; not a bare-metal profile.
    pub fn edge() -> Self {
        Self {
            max_documents: 10_000,
            max_estimated_bytes: 16 * 1024 * 1024,
            max_document_bytes: 64 * 1024,
            max_vector_dimensions: 768,
            max_vector_spaces: 4,
            max_batch_documents: 256,
            max_candidates: 256,
            hnsw_m: 8,
            ef_construction: 48,
            ef_search: 32,
        }
    }
    /// Larger explicit budget for workstation retrieval experiments.
    pub fn research() -> Self {
        Self {
            max_estimated_bytes: usize::try_from(4u64 * 1024 * 1024 * 1024).unwrap_or(usize::MAX),
            max_vector_dimensions: 8192,
            ef_construction: 128,
            // High-dimensional vector search needs a wider graph traversal.
            // Candidate retention remains independently bounded per request.
            ef_search: 1024,
            ..Self::default()
        }
    }
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if self.max_documents == 0
            || self.max_estimated_bytes == 0
            || self.max_document_bytes == 0
            || self.max_vector_dimensions == 0
            || self.max_vector_spaces == 0
            || self.max_batch_documents == 0
            || self.max_candidates == 0
            || self.hnsw_m < 2
            || self.hnsw_m > 128
            || self.ef_construction < self.hnsw_m
            || self.ef_construction > 4096
            || self.ef_search == 0
            || self.ef_search > 65_536
        {
            return Err(crate::Error::ConstraintViolation(
                "Invalid memory resource/index configuration".into(),
            ));
        }
        Ok(())
    }
}

/// Ranking strategy over the union of indexed retrieval candidates.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryFusion {
    /// Reciprocal rank fusion avoids comparing incompatible lexical/vector scales.
    #[default]
    Rrf,
    /// Combine bounded lexical/vector scores with explicit weights.
    Weighted,
}

/// Source/index resource accounting for the open connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Number of stored sources, including expired records not physically purged.
    pub documents: usize,
    /// Conservative admission estimate; not observed RSS.
    pub estimated_resident_bytes: usize,
    /// Current estimate limit.
    pub max_estimated_bytes: usize,
    /// Number of active embedding model/dimension spaces.
    pub vector_spaces: usize,
    /// Active nodes currently materialized in derived HNSW indexes.
    pub indexed_vectors: usize,
    /// Mutated records awaiting lazy index synchronization.
    pub pending_index_updates: usize,
}

/// Work performed by one memory search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchDiagnostics {
    /// Stored source count.
    pub total_documents: usize,
    /// Requested top candidates per retrieval channel.
    pub candidate_limit: usize,
    /// Lexical candidates retained from the inverted index.
    pub lexical_candidates: usize,
    /// Vector candidates retained from the selected embedding space.
    pub vector_candidates: usize,
    /// Distinct candidates hydrated and ranked.
    pub scored_candidates: usize,
    /// Whether vector retrieval deliberately used an exact scan.
    pub exact_vector_search: bool,
    /// Number of derived vector indexes used.
    pub vector_indexes_used: usize,
    /// Ranking strategy.
    pub fusion: MemoryFusion,
}

/// Source results and reproducible retrieval diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResponse {
    /// Original sources and score components.
    pub results: Vec<super::MemoryRecallResult>,
    /// Retrieval work and strategy.
    pub diagnostics: MemorySearchDiagnostics,
}
