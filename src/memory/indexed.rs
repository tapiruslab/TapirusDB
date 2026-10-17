use super::*;
use crate::traits::VectorIndexEngine;
use crate::vector::hnsw::HnswConfig;
use crate::vector::{DistanceMetric, HnswIndex};
use sha2::{Digest, Sha256};
use std::borrow::Cow;

type Space = (Option<String>, usize);
type ExternalKey = (Option<String>, Option<String>, String);

pub(super) fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
fn external_key(entry: &MemoryEntry) -> Option<ExternalKey> {
    Some((
        entry.namespace.as_ref().map(|s| s.to_lowercase()),
        entry.session_id.clone(),
        entry.external_id.clone()?,
    ))
}
fn scope_keys(entry: &MemoryEntry) -> Vec<(u8, String)> {
    let mut keys = Vec::new();
    if let Some(namespace) = &entry.namespace {
        keys.push((0, namespace.to_lowercase()));
    }
    if let Some(session) = &entry.session_id {
        keys.push((1, session.clone()));
    }
    for tag in &entry.tags {
        keys.push((2, tag.to_lowercase()));
    }
    keys.sort();
    keys.dedup();
    keys
}
fn estimate(entry: &MemoryEntry) -> usize {
    let metadata = entry.tags.iter().map(String::len).sum::<usize>()
        + [
            &entry.namespace,
            &entry.session_id,
            &entry.external_id,
            &entry.source_version,
            &entry.embedding_model,
        ]
        .into_iter()
        .flatten()
        .map(String::len)
        .sum::<usize>();
    entry
        .content
        .len()
        .saturating_mul(6)
        .saturating_add(
            entry
                .vector
                .as_ref()
                .map_or(0, |v| v.len().saturating_mul(16)),
        )
        .saturating_add(metadata.saturating_mul(4))
        .saturating_add(2048)
}

fn unit_vector(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    vector
        .iter()
        .map(|v| {
            if norm > 0.0 {
                (f64::from(*v) / norm) as f32
            } else {
                0.0
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct RecordMeta {
    external: Option<ExternalKey>,
    scopes: Vec<(u8, String)>,
    space: Option<Space>,
    bytes: usize,
}
#[derive(Debug, Clone)]
struct VectorSpace {
    index: HnswIndex,
    documents: HashMap<u64, u64>,
    next_node: u64,
}

#[derive(Debug, Clone, Default)]
pub(super) struct MemoryRuntime {
    pub config: MemoryConfig,
    records: HashMap<u64, RecordMeta>,
    external: HashMap<ExternalKey, u64>,
    scopes: HashMap<(u8, String), HashSet<u64>>,
    spaces: HashMap<Space, usize>,
    estimated_bytes: usize,
    dirty: HashSet<u64>,
    indexes: HashMap<Space, VectorSpace>,
    locations: HashMap<u64, (Space, u64)>,
}

impl MemoryRuntime {
    pub fn changed(&mut self, id: u64, entry: Option<&MemoryEntry>) {
        if let Some(old) = self.records.remove(&id) {
            self.estimated_bytes = self.estimated_bytes.saturating_sub(old.bytes);
            if let Some(key) = old.external {
                self.external.remove(&key);
            }
            for key in old.scopes {
                if let Some(ids) = self.scopes.get_mut(&key) {
                    ids.remove(&id);
                    if ids.is_empty() {
                        self.scopes.remove(&key);
                    }
                }
            }
            if let Some(space) = old.space {
                if let Some(count) = self.spaces.get_mut(&space) {
                    *count -= 1;
                    if *count == 0 {
                        self.spaces.remove(&space);
                    }
                }
            }
        }
        if let Some(entry) = entry {
            let meta = RecordMeta {
                external: external_key(entry),
                scopes: scope_keys(entry),
                space: entry
                    .vector
                    .as_ref()
                    .map(|v| (entry.embedding_model.clone(), v.len())),
                bytes: estimate(entry),
            };
            if let Some(key) = &meta.external {
                self.external.insert(key.clone(), id);
            }
            for key in &meta.scopes {
                self.scopes.entry(key.clone()).or_default().insert(id);
            }
            if let Some(space) = &meta.space {
                *self.spaces.entry(space.clone()).or_default() += 1;
            }
            self.estimated_bytes = self.estimated_bytes.saturating_add(meta.bytes);
            self.records.insert(id, meta);
        }
        self.dirty.insert(id);
    }

    fn allowed(&self, search: &MemorySearch) -> Option<Cow<'_, HashSet<u64>>> {
        // Common single-scope queries borrow their posting list. Copying every
        // ID in a large namespace made an otherwise indexed query linear in N.
        let single = if search.tags.is_empty() && search.session_id.is_none() {
            search.namespace.as_ref().map(|ns| (0, ns.to_lowercase()))
        } else if search.tags.is_empty() && search.namespace.is_none() {
            search.session_id.as_ref().map(|id| (1, id.clone()))
        } else if search.namespace.is_none()
            && search.session_id.is_none()
            && search.tags.len() == 1
        {
            Some((2, search.tags[0].to_lowercase()))
        } else {
            None
        };
        if let Some(key) = single {
            return Some(
                self.scopes
                    .get(&key)
                    .map_or_else(|| Cow::Owned(HashSet::new()), Cow::Borrowed),
            );
        }
        let empty = HashSet::new();
        let mut lists = Vec::new();
        if let Some(ns) = &search.namespace {
            lists.push(self.scopes.get(&(0, ns.to_lowercase())).unwrap_or(&empty));
        }
        if let Some(session) = &search.session_id {
            lists.push(self.scopes.get(&(1, session.clone())).unwrap_or(&empty));
        }
        let tags: HashSet<u64> = search
            .tags
            .iter()
            .filter_map(|t| self.scopes.get(&(2, t.to_lowercase())))
            .flatten()
            .copied()
            .collect();
        if !search.tags.is_empty() {
            lists.push(&tags);
        }
        lists.sort_by_key(|ids| ids.len());
        let mut lists = lists.into_iter();
        let mut result = lists.next()?.clone();
        for ids in lists {
            result.retain(|id| ids.contains(id));
        }
        Some(Cow::Owned(result))
    }
}

impl MemoryEngine {
    fn ensure_runtime(&mut self) -> Result<()> {
        if self.runtime.records.len() == self.entries.len() {
            return Ok(());
        }
        // Derived state is deliberately excluded from serialized engines.
        let config = self.runtime.config.clone();
        let old = std::mem::replace(
            &mut self.runtime,
            MemoryRuntime {
                config,
                ..Default::default()
            },
        );
        let result = (|| {
            for entry in self.entries.values() {
                self.validate_entry(entry)?;
                self.runtime.changed(entry.id, Some(entry));
            }
            Ok(())
        })();
        if result.is_err() {
            self.runtime = old;
        }
        result
    }
    /// Create an engine with explicit resource admission limits.
    pub fn with_config(config: MemoryConfig) -> Result<Self> {
        config.validate()?;
        let mut engine = Self::new();
        engine.runtime.config = config;
        Ok(engine)
    }
    /// Current runtime limits; these do not alter the database file format.
    pub fn configuration(&self) -> &MemoryConfig {
        &self.runtime.config
    }
    /// Change admission limits and rebuild derived indexes lazily.
    pub fn configure(&mut self, config: MemoryConfig) -> Result<()> {
        self.ensure_runtime()?;
        config.validate()?;
        if self.runtime.spaces.len() > config.max_vector_spaces {
            return Err(Error::ConstraintViolation(
                "Existing model spaces exceed max_vector_spaces".into(),
            ));
        }
        // Configuration is rare. Validate sources before discarding any old index.
        let previous = std::mem::replace(&mut self.runtime.config, config);
        let checked = self
            .entries
            .values()
            .try_for_each(|entry| self.validate_entry(entry));
        if let Err(error) = checked {
            self.runtime.config = previous;
            return Err(error);
        }
        self.runtime.indexes.clear();
        self.runtime.locations.clear();
        self.runtime.dirty.extend(self.entries.keys().copied());
        Ok(())
    }
    pub(super) fn validate_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let config = &self.runtime.config;
        let reject = |message: &str| Error::ConstraintViolation(message.into());
        if entry.content.trim().is_empty() || entry.content.len() > config.max_document_bytes {
            return Err(reject(
                "Memory content is empty or exceeds max_document_bytes",
            ));
        }
        if !entry.importance.is_finite() || !(0.0..=1.0).contains(&entry.importance) {
            return Err(reject("Memory importance must be between zero and one"));
        }
        if entry.tags.len() > 256
            || entry
                .tags
                .iter()
                .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(reject(
                "Memory tags must be nonempty, at most 256 bytes each and 256 per document",
            ));
        }
        for value in [
            &entry.namespace,
            &entry.session_id,
            &entry.external_id,
            &entry.source_version,
            &entry.embedding_model,
        ]
        .into_iter()
        .flatten()
        {
            if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(reject(
                    "Memory identifiers must be nonempty, at most 512 bytes and contain no control characters",
                ));
            }
        }
        if entry.embedding_model.is_some() && entry.vector.is_none() {
            return Err(reject("embedding_model requires an embedding"));
        }
        if let Some(vector) = &entry.vector {
            if vector.is_empty()
                || vector.len() > config.max_vector_dimensions
                || vector.iter().any(|v| !v.is_finite())
            {
                return Err(reject(
                    "Embedding dimensions/components exceed memory limits",
                ));
            }
            if let Some(model) = &entry.embedding_model {
                if let Some(((_, dimension), _)) = self
                    .runtime
                    .spaces
                    .iter()
                    .find(|((m, d), _)| m.as_ref() == Some(model) && *d != vector.len())
                {
                    return Err(Error::DimensionMismatch(*dimension, vector.len()));
                }
            }
            let space = (entry.embedding_model.clone(), vector.len());
            if !self.runtime.spaces.contains_key(&space)
                && self.runtime.spaces.len() >= config.max_vector_spaces
            {
                return Err(reject("Memory vector-space budget exceeded"));
            }
        }
        if let Some(key) = external_key(entry) {
            if self
                .runtime
                .external
                .get(&key)
                .is_some_and(|id| *id != entry.id)
            {
                return Err(reject("Duplicate external_id within namespace/session"));
            }
        }
        let replacing = self.runtime.records.get(&entry.id);
        if self.entries.len() + usize::from(!self.entries.contains_key(&entry.id))
            > config.max_documents
        {
            return Err(reject("Memory document budget exceeded"));
        }
        let bytes = self
            .runtime
            .estimated_bytes
            .saturating_sub(replacing.map_or(0, |old| old.bytes))
            .saturating_add(estimate(entry));
        if bytes > config.max_estimated_bytes {
            return Err(reject("Memory estimated-resident-byte budget exceeded"));
        }
        Ok(())
    }
    pub(crate) fn write_document(
        &mut self,
        document: &MemoryDocument,
        upsert: bool,
        now: u64,
    ) -> Result<u64> {
        self.ensure_runtime()?;
        if document.expires_at.is_some() && document.ttl_seconds.is_some() {
            return Err(Error::ConstraintViolation(
                "Use expires_at or ttl_seconds, not both".into(),
            ));
        }
        let existing_id = document
            .external_id
            .as_ref()
            .and_then(|id| {
                self.runtime.external.get(&(
                    document.namespace.as_ref().map(|s| s.to_lowercase()),
                    document.session_id.clone(),
                    id.clone(),
                ))
            })
            .copied();
        if !upsert && existing_id.is_some() {
            return Err(Error::ConstraintViolation(
                "external_id already exists; use upsert_memory".into(),
            ));
        }
        if upsert && document.external_id.is_none() {
            return Err(Error::ConstraintViolation(
                "upsert_memory requires external_id".into(),
            ));
        }
        let old = existing_id.and_then(|id| self.entries.get(&id));
        if document
            .expected_revision
            .is_some_and(|revision| revision != old.map_or(0, |e| e.revision))
        {
            return Err(Error::ConstraintViolation(
                "Memory revision conflict".into(),
            ));
        }
        let id = existing_id.unwrap_or(self.next_id);
        if id == 0 || id == u64::MAX {
            return Err(Error::ConstraintViolation(
                "Memory ID space exhausted".into(),
            ));
        }
        let expires_at = match document.ttl_seconds {
            Some(ttl) => Some(now.checked_add(ttl).ok_or_else(|| {
                Error::ConstraintViolation("Memory expiry overflows Unix seconds".into())
            })?),
            None => document.expires_at,
        };
        let revision = old
            .map_or(0, |e| e.revision)
            .checked_add(1)
            .ok_or_else(|| Error::ConstraintViolation("Memory revision overflow".into()))?;
        let entry = MemoryEntry {
            id,
            content: document.content.clone(),
            vector: document.embedding.clone(),
            importance: document.importance,
            timestamp: old.map_or(now, |e| e.timestamp),
            last_accessed: old.map_or(now, |e| e.last_accessed),
            access_count: old.map_or(0, |e| e.access_count),
            tags: document.tags.clone(),
            associations: old.map_or_else(Vec::new, |e| e.associations.clone()),
            namespace: document.namespace.clone(),
            session_id: document.session_id.clone(),
            external_id: document.external_id.clone(),
            source_version: document.source_version.clone(),
            embedding_model: document.embedding_model.clone(),
            expires_at,
            revision,
            updated_at: now,
            content_hash: content_hash(&document.content),
        };
        self.restore_entry(entry)?;
        Ok(id)
    }
    /// Accounting for sources and indexes currently materialized in this engine.
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            documents: self.entries.len(),
            estimated_resident_bytes: self.runtime.estimated_bytes,
            max_estimated_bytes: self.runtime.config.max_estimated_bytes,
            vector_spaces: self.runtime.spaces.len(),
            indexed_vectors: self
                .runtime
                .indexes
                .values()
                .map(|s| s.documents.len())
                .sum(),
            pending_index_updates: self.runtime.dirty.len(),
        }
    }
    pub(crate) fn remove_ids(&mut self, ids: &[u64]) -> (Vec<u64>, Vec<u64>) {
        let ids: HashSet<_> = ids.iter().copied().collect();
        let mut removed = Vec::new();
        let mut updated = Vec::new();
        for &id in &ids {
            if self.entries.remove(&id).is_some() {
                self.bm25.remove_document(id);
                self.runtime.changed(id, None);
                removed.push(id);
            }
        }
        for entry in self.entries.values_mut() {
            let old = entry.associations.len();
            entry.associations.retain(|id| !ids.contains(id));
            if old != entry.associations.len() {
                updated.push(entry.id);
            }
        }
        removed.sort_unstable();
        updated.sort_unstable();
        (removed, updated)
    }
    /// Build/update derived vector indexes from authoritative source records.
    pub fn prepare_indexes(&mut self) -> Result<MemoryStats> {
        self.ensure_runtime()?;
        let mut dirty: Vec<_> = self.runtime.dirty.iter().copied().collect();
        dirty.sort_unstable();
        for id in dirty {
            if let Some((space, node)) = self.runtime.locations.remove(&id) {
                if let Some(index) = self.runtime.indexes.get_mut(&space) {
                    index.index.mark_deleted(node);
                    index.documents.remove(&node);
                }
            }
            if let Some(entry) = self.entries.get(&id) {
                if let Some(vector) = &entry.vector {
                    let key = (entry.embedding_model.clone(), vector.len());
                    let config = &self.runtime.config;
                    let index =
                        self.runtime
                            .indexes
                            .entry(key.clone())
                            .or_insert_with(|| VectorSpace {
                                index: HnswIndex::with_config_and_seed(
                                    vector.len(),
                                    DistanceMetric::Cosine,
                                    HnswConfig {
                                        m: config.hnsw_m,
                                        m0: config.hnsw_m * 2,
                                        ef_construction: config.ef_construction,
                                        ef_search: config.ef_search,
                                        quantize_sq8: false,
                                    },
                                    20_261_016,
                                ),
                                documents: HashMap::new(),
                                next_node: 1,
                            });
                    let node = index.next_node;
                    index.next_node = node.checked_add(1).ok_or_else(|| {
                        Error::ConstraintViolation("Memory index node space exhausted".into())
                    })?;
                    index.index.insert_vector(node, &unit_vector(vector))?;
                    index.documents.insert(node, id);
                    self.runtime.locations.insert(id, (key, node));
                }
            }
            self.runtime.dirty.remove(&id);
        }
        self.runtime
            .indexes
            .retain(|_, index| !index.documents.is_empty());
        for index in self.runtime.indexes.values_mut() {
            if index.index.tombstones.len() > index.documents.len() / 4 + 64 {
                index.index.rebuild()?;
            }
        }
        Ok(self.stats())
    }
    /// Search bounded channel candidates using indexed lexical/vector retrieval.
    pub fn search_indexed(
        &mut self,
        search: &MemorySearch,
        now: u64,
    ) -> Result<MemorySearchResponse> {
        search.validate(&self.runtime.config)?;
        self.prepare_indexes()?;
        let allowed = self.runtime.allowed(search);
        let eligible = |id: u64| {
            self.entries
                .get(&id)
                .is_some_and(|entry| entry.expires_at.is_none_or(|expiry| now < expiry))
        };
        let lexical = search
            .query
            .as_deref()
            .map(|text| {
                self.bm25
                    .search_where(text, search.candidate_limit, allowed.as_deref(), eligible)
            })
            .unwrap_or_default();
        let mut vector_hits = Vec::new();
        let mut vector_indexes_used = 0;
        if let Some(vector) = &search.embedding {
            let normalized_query = unit_vector(vector);
            if let Some(model) = &search.embedding_model {
                if let Some(((_, dimension), _)) = self
                    .runtime
                    .spaces
                    .iter()
                    .find(|((m, _), _)| m.as_ref() == Some(model))
                {
                    if *dimension != vector.len() {
                        return Err(Error::DimensionMismatch(*dimension, vector.len()));
                    }
                }
            }
            let key = (search.embedding_model.clone(), vector.len());
            if let Some(space) = self.runtime.indexes.get(&key) {
                vector_indexes_used = 1;
                if search.exact {
                    for &id in space.documents.values() {
                        if allowed.as_ref().is_some_and(|ids| !ids.contains(&id)) || !eligible(id) {
                            continue;
                        }
                        let score = cosine_similarity(
                            vector,
                            self.entries[&id].vector.as_ref().expect("indexed vector"),
                        );
                        if score > 0.0 {
                            vector_hits.push((id, score));
                        }
                    }
                    vector_hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    vector_hits.truncate(search.candidate_limit);
                } else {
                    // Map only small selective scopes to index IDs. Large scopes
                    // use the predicate during candidate search without an O(N) map.
                    let node_filter: Option<HashSet<u64>> = allowed
                        .as_ref()
                        .filter(|ids| {
                            ids.len() <= space.index.ef_search.max(search.candidate_limit)
                        })
                        .map(|ids| {
                            ids.iter()
                                .filter_map(|id| self.runtime.locations.get(id))
                                .filter(|(s, _)| *s == key)
                                .map(|(_, node)| *node)
                                .collect()
                        });
                    for (node, distance) in space.index.search_knn_where(
                        &normalized_query,
                        search.candidate_limit,
                        DistanceMetric::Cosine,
                        (node_filter.as_ref(), allowed.is_some()),
                        |node| {
                            space.documents.get(&node).is_some_and(|id| {
                                eligible(*id) && allowed.as_ref().is_none_or(|ids| ids.contains(id))
                            })
                        },
                        self.runtime.config.max_candidates,
                    )? {
                        if let Some(id) = space.documents.get(&node) {
                            let score = (1.0 - distance).clamp(0.0, 1.0);
                            if score > 0.0 {
                                vector_hits.push((*id, score));
                            }
                        }
                    }
                }
            }
        }
        let lexical_scores: HashMap<_, _> = lexical.iter().copied().collect();
        let semantic_scores: HashMap<_, _> = vector_hits.iter().copied().collect();
        let lexical_ids: Vec<_> = lexical.iter().map(|(id, _)| *id).collect();
        let vector_ids: Vec<_> = vector_hits.iter().map(|(id, _)| *id).collect();
        let channel_weight = (if search.query.is_some() {
            search.lexical_weight
        } else {
            0.0
        }) + (if search.embedding.is_some() {
            search.vector_weight
        } else {
            0.0
        });
        let fused: HashMap<_, _> = reciprocal_rank_fusion(
            &[
                (&lexical_ids, search.lexical_weight),
                (&vector_ids, search.vector_weight),
            ],
            search.rrf_k,
            search.candidate_limit.saturating_mul(2),
        )
        .into_iter()
        .collect();
        let total_weight = search.vector_weight
            + search.lexical_weight
            + search.recency_weight
            + search.importance_weight;
        let mut ranked = Vec::with_capacity(fused.len());
        for (&id, &rrf) in &fused {
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            let lexical_score = lexical_scores.get(&id).copied().unwrap_or(0.0);
            let semantic_score = match (&search.embedding, &entry.vector) {
                (Some(query), Some(vector))
                    if entry.embedding_model == search.embedding_model
                        && query.len() == vector.len() =>
                {
                    cosine_similarity(query, vector).max(0.0)
                }
                _ => 0.0,
            };
            let dt = now.saturating_sub(entry.updated_at.max(entry.timestamp)) as f32;
            let recency_score = if search.half_life_seconds == 0 {
                1.0
            } else {
                (-2.0f32.ln() * dt / search.half_life_seconds as f32).exp()
            };
            let relevance = match search.fusion {
                MemoryFusion::Rrf => rrf * (search.rrf_k + 1.0) / channel_weight.max(f32::EPSILON),
                MemoryFusion::Weighted => {
                    (search.vector_weight * semantic_score
                        + search.lexical_weight * lexical_score / (1.0 + lexical_score))
                        / channel_weight.max(f32::EPSILON)
                }
            };
            let score = ((search.vector_weight + search.lexical_weight) * relevance
                + search.recency_weight * recency_score
                + search.importance_weight * entry.importance)
                / total_weight;
            if score >= search.min_score {
                ranked.push((
                    id,
                    score.clamp(0.0, 1.0),
                    semantic_score,
                    lexical_score,
                    recency_score,
                ));
            }
        }
        let scored_candidates = fused.len();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(search.limit);
        let results = ranked
            .into_iter()
            .map(
                |(id, combined_score, semantic_score, lexical_score, recency_score)| {
                    let entry = self.entries.get_mut(&id).expect("ranked source");
                    entry.last_accessed = now;
                    entry.access_count = entry.access_count.saturating_add(1);
                    MemoryRecallResult {
                        entry: entry.clone(),
                        combined_score,
                        semantic_score,
                        lexical_score,
                        recency_score,
                        is_associative: false,
                    }
                },
            )
            .collect();
        Ok(MemorySearchResponse {
            results,
            diagnostics: MemorySearchDiagnostics {
                total_documents: self.entries.len(),
                candidate_limit: search.candidate_limit,
                lexical_candidates: lexical.len(),
                vector_candidates: semantic_scores.len(),
                scored_candidates,
                exact_vector_search: search.exact,
                vector_indexes_used,
                fusion: search.fusion,
            },
        })
    }
}
