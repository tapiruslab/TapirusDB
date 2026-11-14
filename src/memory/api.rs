//! Portable requests for persisted document memory and hybrid retrieval.

use super::{MemoryConfig, MemoryFusion, MemoryRecallResult, MemorySearchResponse, MemoryStats};
use crate::{Connection, Error, Result};
use serde::{Deserialize, Serialize};

/// A document with an optional embedding supplied by the application's model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryDocument {
    /// Original text returned as a source citation in search results.
    pub content: String,
    /// Model-generated embedding; omission enables lexical retrieval only.
    pub embedding: Option<Vec<f32>>,
    /// Retention importance between zero and one.
    pub importance: f32,
    /// Application-defined source or category tags.
    pub tags: Vec<String>,
    /// Optional namespace filter, independent of authentication.
    pub namespace: Option<String>,
    /// Optional conversation/session identifier.
    pub session_id: Option<String>,
    /// Idempotency key scoped by namespace and session.
    pub external_id: Option<String>,
    /// Application source version.
    pub source_version: Option<String>,
    /// Embedding model identity; absent selects the legacy model space.
    pub embedding_model: Option<String>,
    /// Absolute expiry in Unix seconds. Mutually exclusive with ttl_seconds.
    pub expires_at: Option<u64>,
    /// Expiry relative to insertion/update time.
    pub ttl_seconds: Option<u64>,
    /// Conditional update revision; zero means the document must not exist.
    pub expected_revision: Option<u64>,
}

impl Default for MemoryDocument {
    fn default() -> Self {
        Self {
            content: String::new(),
            embedding: None,
            importance: 0.5,
            tags: Vec::new(),
            namespace: None,
            session_id: None,
            external_id: None,
            source_version: None,
            embedding_model: None,
            expires_at: None,
            ttl_seconds: None,
            expected_revision: None,
        }
    }
}

/// Hard owner boundary enforced across every memory operation of a scoped [`Connection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAccessScope {
    /// Mandatory owner namespace (compared case-insensitively).
    pub namespace: String,
    /// Optional session identifier bound to this owner (compared exactly).
    pub session_id: Option<String>,
}

fn validate_scope_token(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(Error::ConstraintViolation(format!(
            "Invalid memory scope {label}"
        )));
    }
    Ok(())
}

impl MemoryAccessScope {
    /// Validate and construct an owner memory isolation scope.
    pub fn new(namespace: impl Into<String>, session_id: Option<String>) -> Result<Self> {
        let namespace = namespace.into();
        validate_scope_token("namespace", &namespace)?;
        if let Some(ref session) = session_id {
            validate_scope_token("session_id", session)?;
        }
        Ok(Self {
            namespace,
            session_id,
        })
    }

    /// Return whether an entry belongs to this owner boundary.
    pub fn matches_entry(&self, entry: &super::MemoryEntry) -> bool {
        entry
            .namespace
            .as_ref()
            .is_some_and(|ns| ns.eq_ignore_ascii_case(&self.namespace))
            && self
                .session_id
                .as_ref()
                .is_none_or(|sess| entry.session_id.as_ref() == Some(sess))
    }

    /// Resolve effective write/query `(namespace, session_id)` or reject cross-owner access.
    pub fn resolve_target(
        &self,
        requested_namespace: Option<&str>,
        requested_session_id: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        if let Some(ns) = requested_namespace
            && !ns.eq_ignore_ascii_case(&self.namespace)
        {
            return Err(Error::PermissionDenied(format!(
                "Namespace '{ns}' is outside scoped owner boundary '{}'",
                self.namespace
            )));
        }
        let session = if let Some(bound_session) = &self.session_id {
            if let Some(req_session) = requested_session_id
                && req_session != bound_session
            {
                return Err(Error::PermissionDenied(format!(
                    "Session '{req_session}' is outside scoped owner session boundary"
                )));
            }
            Some(bound_session.clone())
        } else {
            requested_session_id.map(str::to_owned)
        };
        Ok((self.namespace.clone(), session))
    }

    /// Enforce this boundary onto a document before persistence.
    pub fn enforce_document(&self, document: &MemoryDocument) -> Result<MemoryDocument> {
        let (ns, sess) = self.resolve_target(
            document.namespace.as_deref(),
            document.session_id.as_deref(),
        )?;
        let mut scoped = document.clone();
        scoped.namespace = Some(ns);
        scoped.session_id = sess;
        Ok(scoped)
    }

    /// Enforce this boundary onto a structured search query.
    pub fn enforce_search(&self, search: &MemorySearch) -> Result<MemorySearch> {
        let (ns, sess) =
            self.resolve_target(search.namespace.as_deref(), search.session_id.as_deref())?;
        let mut scoped = search.clone();
        scoped.namespace = Some(ns);
        scoped.session_id = sess;
        Ok(scoped)
    }

    /// Enforce this boundary onto a legacy recall filter.
    pub fn enforce_recall_filter(
        &self,
        filter: &super::MemoryRecallFilter,
    ) -> Result<super::MemoryRecallFilter> {
        let (ns, sess) =
            self.resolve_target(filter.namespace.as_deref(), filter.session_id.as_deref())?;
        let mut scoped = filter.clone();
        scoped.namespace = Some(ns);
        scoped.session_id = sess;
        Ok(scoped)
    }

    /// Enforce this boundary onto a deletion scope after structural validation.
    pub fn enforce_delete_scope(&self, scope: &MemoryDeleteScope) -> Result<MemoryDeleteScope> {
        if scope.all {
            return Err(Error::PermissionDenied(
                "Global all=true deletion is forbidden on a scoped memory connection".into(),
            ));
        }
        let (ns, sess) =
            self.resolve_target(scope.namespace.as_deref(), scope.session_id.as_deref())?;
        let mut scoped = scope.clone();
        scoped.namespace = Some(ns);
        scoped.session_id = sess;
        Ok(scoped)
    }
}

/// Search text, supplied embeddings, or both, with explicit source filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemorySearch {
    /// BM25 query text. At least query or embedding is required.
    pub query: Option<String>,
    /// An embedding from the same model/dimension as stored documents.
    pub embedding: Option<Vec<f32>>,
    /// Maximum results, from zero to 10,000.
    pub limit: usize,
    /// Optional namespace restriction.
    pub namespace: Option<String>,
    /// Optional session restriction.
    pub session_id: Option<String>,
    /// Match at least one requested tag when this list is nonempty.
    pub tags: Vec<String>,
    /// Embedding model identity; vector searches never mix model spaces.
    pub embedding_model: Option<String>,
    /// Top candidates per channel before final ranking.
    pub candidate_limit: usize,
    /// Rank fusion or bounded weighted scoring.
    pub fusion: MemoryFusion,
    /// RRF smoothing constant.
    pub rrf_k: f32,
    /// Semantic contribution weight.
    pub vector_weight: f32,
    /// Keyword contribution weight.
    pub lexical_weight: f32,
    /// Recency contribution weight.
    pub recency_weight: f32,
    /// Importance contribution weight.
    pub importance_weight: f32,
    /// Recency half-life in seconds; zero disables decay.
    pub half_life_seconds: u64,
    /// Minimum final ranking score, from zero to one.
    pub min_score: f32,
    /// Deliberate exact vector scan for evaluation/small corpora.
    pub exact: bool,
}

impl Default for MemorySearch {
    fn default() -> Self {
        Self {
            query: None,
            embedding: None,
            limit: 5,
            namespace: None,
            session_id: None,
            tags: Vec::new(),
            embedding_model: None,
            candidate_limit: 128,
            fusion: MemoryFusion::Rrf,
            rrf_k: 20.0,
            vector_weight: 0.65,
            lexical_weight: 0.25,
            recency_weight: 0.05,
            importance_weight: 0.05,
            half_life_seconds: 30 * 86400,
            min_score: 0.0,
            exact: false,
        }
    }
}

impl Connection {
    /// Generate embeddings before an atomic upsert. Provider failures leave sources unchanged.
    /// Applications should pin the provider model ID to its actual model revision.
    pub fn upsert_embedded_memories(
        &self,
        provider: &impl super::EmbeddingProvider,
        documents: &[MemoryDocument],
    ) -> Result<Vec<u64>> {
        let config = self.memory.read().configuration().clone();
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        if documents.len() > config.max_batch_documents
            || provider.vector_dimensions() > config.max_vector_dimensions
        {
            return Err(Error::ConstraintViolation(
                "Embedding batch exceeds configured limits".into(),
            ));
        }
        let texts: Vec<&str> = documents.iter().map(|doc| doc.content.as_str()).collect();
        let vectors = provider.embed_documents(&texts)?;
        super::embedder::validate_embeddings(
            &vectors,
            documents.len(),
            provider.vector_dimensions(),
        )?;
        let model = provider.model_id();
        let documents: Vec<_> = documents
            .iter()
            .zip(vectors)
            .map(|(doc, vector)| {
                let mut doc = doc.clone();
                doc.embedding = Some(vector);
                doc.embedding_model = Some(model.clone());
                doc
            })
            .collect();
        self.upsert_memory_batch(&documents)
    }
    /// Portable JSON gateway shared by language bindings. Public SDKs expose
    /// named methods; operation names here are an internal transport contract.
    pub fn memory_request_json(&self, operation: &str, payload: &str) -> Result<String> {
        fn decode<T: serde::de::DeserializeOwned>(payload: &str) -> Result<T> {
            serde_json::from_str(payload).map_err(|e| Error::Serialization(e.to_string()))
        }
        fn encode(value: impl Serialize) -> Result<String> {
            serde_json::to_string(&value).map_err(|e| Error::Serialization(e.to_string()))
        }
        match operation {
            "store" => encode(self.store_memory(&decode(payload)?)?),
            "search" => encode(self.search_memories(&decode(payload)?)?),
            "forget" => encode(self.memory_forget_checked(decode(payload)?)?),
            "upsert" => encode(self.upsert_memory(&decode(payload)?)?),
            "store_batch" => {
                encode(self.store_memory_batch(&decode::<Vec<MemoryDocument>>(payload)?)?)
            }
            "upsert_batch" => {
                encode(self.upsert_memory_batch(&decode::<Vec<MemoryDocument>>(payload)?)?)
            }
            "search_detailed" => encode(self.search_memories_detailed(&decode(payload)?)?),
            "configure" => {
                self.configure_memory(decode(payload)?)?;
                encode(self.memory_stats())
            }
            "stats" => encode(self.memory_stats()),
            "storage_health" => encode(self.storage_health()),
            "prepare" => encode(self.prepare_memory_indexes()?),
            "purge_expired" => encode(self.purge_expired_memories()?),
            "delete" => encode(self.delete_memories(&decode(payload)?)?),
            _ => Err(Error::ConstraintViolation(
                "Unknown memory operation".into(),
            )),
        }
    }
    /// Persist document text and optional external embeddings in one atomic operation.
    /// Participates in the active SQL transaction, including savepoints.
    pub fn store_memory(&self, document: &MemoryDocument) -> Result<u64> {
        self.write_memory_batch(std::slice::from_ref(document), false, unix_time())
            .map(|ids| ids[0])
    }

    /// Insert or replace a source using its external ID within namespace/session.
    pub fn upsert_memory(&self, document: &MemoryDocument) -> Result<u64> {
        self.write_memory_batch(std::slice::from_ref(document), true, unix_time())
            .map(|ids| ids[0])
    }

    /// Insert sources in one atomic statement, joining any active SQL transaction.
    pub fn store_memory_batch(&self, documents: &[MemoryDocument]) -> Result<Vec<u64>> {
        self.write_memory_batch(documents, false, unix_time())
    }

    /// Idempotent batch source replacement in one atomic statement.
    pub fn upsert_memory_batch(&self, documents: &[MemoryDocument]) -> Result<Vec<u64>> {
        self.write_memory_batch(documents, true, unix_time())
    }

    fn write_memory_batch(
        &self,
        documents: &[MemoryDocument],
        upsert: bool,
        now: u64,
    ) -> Result<Vec<u64>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        if documents.len() > self.memory.read().configuration().max_batch_documents {
            return Err(Error::ConstraintViolation(
                "Memory batch exceeds max_batch_documents".into(),
            ));
        }
        self.mutate_memory(|memory, executor, pager| {
            let mut ids = Vec::with_capacity(documents.len());
            for document in documents {
                let scoped_document;
                let document = if let Some(scope) = self.memory_scope.as_ref() {
                    scoped_document = scope.enforce_document(document)?;
                    &scoped_document
                } else {
                    document
                };
                let id = memory.write_document(document, upsert, now)?;
                executor
                    .persist_memory_entry(pager, memory.get_memory(id).expect("stored source"))?;
                ids.push(id);
            }
            Ok(ids)
        })
    }

    /// Configure resource limits for this open connection and its cloned handles.
    /// Reapply on reopen, or specify Config::memory when opening the database.
    pub fn configure_memory(&self, config: MemoryConfig) -> Result<()> {
        if self.memory_scope.is_some() {
            return Err(Error::PermissionDenied(
                "Scoped memory connections cannot alter global memory configuration".into(),
            ));
        }
        self.memory.write().configure(config)
    }

    /// Report estimated source/index admission usage, not measured process RSS.
    pub fn memory_stats(&self) -> MemoryStats {
        self.memory.read().stats()
    }

    /// Materialize pending derived vector indexes before latency-sensitive queries.
    pub fn prepare_memory_indexes(&self) -> Result<MemoryStats> {
        self.memory.write().prepare_indexes()
    }

    /// Physically purge sources whose absolute expiry has passed.
    pub fn purge_expired_memories(&self) -> Result<usize> {
        self.purge_expired_memories_at(unix_time())
    }

    /// Purge expiry using an explicit Unix timestamp for deterministic maintenance.
    pub fn purge_expired_memories_at(&self, now: u64) -> Result<usize> {
        let scope = self.memory_scope.as_ref();
        self.mutate_memory(|memory, executor, pager| {
            let ids: Vec<_> = memory
                .all_entries()
                .values()
                .filter(|entry| {
                    entry.expires_at.is_some_and(|expiry| expiry <= now)
                        && scope.is_none_or(|s| s.matches_entry(entry))
                })
                .map(|entry| entry.id)
                .collect();
            let (removed, updated) = memory.remove_ids(&ids);
            for id in &removed {
                executor.delete_memory_entry(pager, *id)?;
            }
            for id in updated {
                executor.persist_memory_entry(
                    pager,
                    memory.get_memory(id).expect("remaining associated source"),
                )?;
            }
            Ok(removed.len())
        })
    }

    /// Delete sources matching explicit application filters in one transaction.
    /// Empty scopes are rejected unless all=true is deliberately supplied.
    pub fn delete_memories(&self, scope: &MemoryDeleteScope) -> Result<usize> {
        scope.validate()?;
        let scoped_delete;
        let effective_scope = if let Some(owner_scope) = self.memory_scope.as_ref() {
            scoped_delete = owner_scope.enforce_delete_scope(scope)?;
            &scoped_delete
        } else {
            scope
        };
        self.mutate_memory(|memory, executor, pager| {
            let ids: Vec<_> = memory
                .all_entries()
                .values()
                .filter(|entry| effective_scope.matches(entry))
                .map(|entry| entry.id)
                .collect();
            let (removed, updated) = memory.remove_ids(&ids);
            for id in &removed {
                executor.delete_memory_entry(pager, *id)?;
            }
            for id in updated {
                executor.persist_memory_entry(
                    pager,
                    memory.get_memory(id).expect("remaining associated source"),
                )?;
            }
            Ok(removed.len())
        })
    }

    /// Return source text/IDs and separate lexical, cosine and recency scores.
    /// No embedding model or cloud service is invoked by this method.
    pub fn search_memories(&self, search: &MemorySearch) -> Result<Vec<MemoryRecallResult>> {
        Ok(self.search_memories_detailed(search)?.results)
    }

    /// Indexed retrieval with candidate counts and ranking diagnostics.
    pub fn search_memories_detailed(&self, search: &MemorySearch) -> Result<MemorySearchResponse> {
        self.search_memories_at(search, unix_time())
    }

    /// Indexed retrieval evaluated at an explicit Unix timestamp.
    pub fn search_memories_at(
        &self,
        search: &MemorySearch,
        now: u64,
    ) -> Result<MemorySearchResponse> {
        let scoped_search;
        let effective_search = if let Some(scope) = self.memory_scope.as_ref() {
            scoped_search = scope.enforce_search(search)?;
            &scoped_search
        } else {
            search
        };
        self.pager.read().ensure_io_ready()?;
        self.memory.write().search_indexed(effective_search, now)
    }
}

pub(crate) fn unix_time() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl MemorySearch {
    pub(crate) fn validate(&self, config: &MemoryConfig) -> Result<()> {
        let reject = |text: &str| Error::ConstraintViolation(text.into());
        if self.query.as_ref().is_none_or(|q| q.trim().is_empty()) && self.embedding.is_none() {
            return Err(reject("Supply query text or an embedding"));
        }
        if self.query.as_ref().is_some_and(|q| q.len() > 65536) {
            return Err(reject("Memory query exceeds 65536 bytes"));
        }
        if self.limit > self.candidate_limit
            || self.candidate_limit == 0
            || self.candidate_limit > config.max_candidates
        {
            return Err(reject(
                "Require limit <= candidate_limit <= configured max_candidates",
            ));
        }
        if self.embedding.as_ref().is_some_and(|v| {
            v.is_empty()
                || v.len() > config.max_vector_dimensions
                || v.iter().any(|x| !x.is_finite())
        }) {
            return Err(reject("Invalid query embedding"));
        }
        let weights = [
            self.vector_weight,
            self.lexical_weight,
            self.recency_weight,
            self.importance_weight,
        ];
        if weights
            .iter()
            .any(|w| !w.is_finite() || !(0.0..=1_000_000.0).contains(w))
            || weights.iter().sum::<f32>() <= 0.0
            || (if self.query.is_some() {
                self.lexical_weight
            } else {
                0.0
            }) + (if self.embedding.is_some() {
                self.vector_weight
            } else {
                0.0
            }) <= 0.0
        {
            return Err(reject(
                "Memory ranking requires finite nonnegative weights and a positive query-channel weight",
            ));
        }
        if !self.rrf_k.is_finite()
            || !(1.0..=1_000_000.0).contains(&self.rrf_k)
            || !self.min_score.is_finite()
            || !(0.0..=1.0).contains(&self.min_score)
        {
            return Err(reject("Invalid RRF constant or minimum score"));
        }
        Ok(())
    }
}

/// Explicit selectors for atomic source deletion; filters are not authorization.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryDeleteScope {
    /// Namespace, compared case insensitively.
    pub namespace: Option<String>,
    /// Exact session ID.
    pub session_id: Option<String>,
    /// Exact external source ID.
    pub external_id: Option<String>,
    /// Exact application source revision.
    pub source_version: Option<String>,
    /// Delete versions other than this one, with another inclusive selector.
    pub except_source_version: Option<String>,
    /// Match at least one case-insensitive tag.
    pub tags: Vec<String>,
    /// Deliberately permit deleting every source.
    pub all: bool,
}
impl MemoryDeleteScope {
    fn validate(&self) -> Result<()> {
        if !self.all
            && self.namespace.is_none()
            && self.session_id.is_none()
            && self.external_id.is_none()
            && self.source_version.is_none()
            && self.tags.is_empty()
        {
            return Err(Error::ConstraintViolation(
                "Deletion requires explicit source filters or all=true".into(),
            ));
        }
        Ok(())
    }
    fn matches(&self, entry: &super::MemoryEntry) -> bool {
        self.namespace.as_ref().is_none_or(|ns| {
            entry
                .namespace
                .as_ref()
                .is_some_and(|value| value.to_lowercase() == ns.to_lowercase())
        }) && self
            .session_id
            .as_ref()
            .is_none_or(|id| entry.session_id.as_ref() == Some(id))
            && self
                .external_id
                .as_ref()
                .is_none_or(|id| entry.external_id.as_ref() == Some(id))
            && self
                .source_version
                .as_ref()
                .is_none_or(|version| entry.source_version.as_ref() == Some(version))
            && self
                .except_source_version
                .as_ref()
                .is_none_or(|version| entry.source_version.as_ref() != Some(version))
            && (self.tags.is_empty()
                || self.tags.iter().any(|tag| {
                    entry
                        .tags
                        .iter()
                        .any(|value| value.to_lowercase() == tag.to_lowercase())
                }))
    }
}
