//! # TapirusDB (`tapirus`)
//!
//! The Pure Safe-Rust Embedded Database Engine — SQLite Simplicity with Native AI Vector Search.
//!
//! Stored entirely in a single `.tapir` file with zero background server processes.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod btree;
pub mod crypto;
pub mod document;
pub mod embedded;
pub mod error;
pub mod graph;
pub mod memory;
pub mod pager;
pub mod sql;
pub mod realtime;
pub mod traits;
pub mod vector;

pub use btree::{BTreeEngine, PageHeader, PageType, TableLeafCell};
pub use crypto::DatabaseCipher;
pub use document::Collection;
pub use embedded::{
    FlashBlockDevice, MicroDatabase, MicroHeader, MicroPager, RamBlockDevice,
    DEFAULT_MICRO_BLOCK_SIZE, MICRO_DATABASE_MAGIC,
};
pub use error::{Error, Result};
pub use graph::{
    ChainMatch, Direction, Edge, GraphChain, GraphEngine, GraphRagConfig, GraphRagContext,
    GraphRagEngine, GraphRagResult, Node, NodeFilter, TraversalStep,
};
pub use memory::{
    reciprocal_rank_fusion, Bm25Index, Bm25Params, DeterministicHashEmbedder, EmbeddingEngine,
    HttpEmbeddingEngine, MemoryEngine, MemoryEntry, MemoryRecallFilter, MemoryRecallResult,
};
pub use pager::{
    DatabaseHeader, MockRemoteRangeStorage, PageId, Pager, RemotePager, RemoteRangeReader,
    RemoteStorageAdapter, S3StorageConfig, DEFAULT_PAGE_SIZE,
};
pub use realtime::{ChangeEvent, ChangeOp, RealtimeBus};
pub use sql::{bind_parameters, parse_sql, parse_tokens, SQLExecutor, Statement};
pub use traits::{DatabaseConnection, FromValue, Row, Value, VectorIndexEngine};
pub use vector::{DistanceMetric, HnswIndex, ProductQuantizer, QuantizedVector8, QuantizedVectorPQ, Vector};

/// TapirusDB package version string
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;

/// Configuration options for opening a TapirusDB connection
#[derive(Debug, Clone)]
pub struct Config {
    /// Size of each database page in bytes (default: 4096)
    pub page_size: u16,
    /// Maximum number of cached pages in memory
    pub page_cache_capacity: usize,
    /// Whether to enable auto-checkpointing for WAL
    pub auto_checkpoint: bool,
    /// Optional user passphrase for ChaCha20-Poly1305 encryption at rest
    pub passphrase: Option<String>,
    /// Optional raw 256-bit encryption key
    pub encryption_key: Option<[u8; 32]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            page_cache_capacity: 1024,
            auto_checkpoint: true,
            passphrase: None,
            encryption_key: None,
        }
    }
}

/// An open connection to a TapirusDB single-file database.
///
/// Implements `Clone` for cheap thread-safe multi-threaded access across worker pools,
/// HTTP servers, and async task runtimes using Single-Writer Multiple-Reader (SWMR) synchronization.
#[derive(Clone)]
pub struct Connection {
    pager: Arc<RwLock<Pager>>,
    executor: Arc<RwLock<SQLExecutor>>,
    memory: Arc<RwLock<MemoryEngine>>,
    realtime: Arc<RealtimeBus>,
    config: Config,
}

/// A thread-safe connection pool for distributing cloned TapirusDB connection handles across threads
#[derive(Clone)]
pub struct ConnectionPool {
    conn: Connection,
}

impl ConnectionPool {
    /// Create a new connection pool wrapping an open database connection
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Open a connection pool directly from a database file path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        Ok(Self::new(conn))
    }

    /// Open an in-memory connection pool
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Self::new(conn))
    }

    /// Acquire a thread-safe connection handle
    pub fn acquire(&self) -> Connection {
        self.conn.clone()
    }
}

impl Connection {
    /// Open an existing TapirusDB database file, or create a new one if it does not exist
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_with_config(path, Config::default())
    }

    /// Open an existing TapirusDB database file, or create a new one if it does not exist (alias for `open`)
    pub fn open_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open(path)
    }

    /// Open an encrypted TapirusDB single-file database using a user passphrase
    pub fn open_encrypted<P: AsRef<Path>>(path: P, passphrase: &str) -> Result<Self> {
        let config = Config {
            passphrase: Some(passphrase.to_string()),
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// Open an encrypted TapirusDB single-file database using a raw 256-bit key
    pub fn open_encrypted_with_key<P: AsRef<Path>>(path: P, key: [u8; 32]) -> Result<Self> {
        let config = Config {
            encryption_key: Some(key),
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// Open a database file with custom configuration options
    pub fn open_with_config<P: AsRef<Path>>(path: P, config: Config) -> Result<Self> {
        let cipher = if let Some(key) = config.encryption_key {
            let salt = read_or_generate_salt(path.as_ref())?;
            Some(DatabaseCipher::new(key, salt))
        } else if let Some(ref pass) = config.passphrase {
            let salt = read_or_generate_salt(path.as_ref())?;
            Some(DatabaseCipher::from_passphrase(pass, salt))
        } else {
            None
        };

        let mut pager = Pager::open_with_cipher(
            path.as_ref(),
            config.page_size,
            config.page_cache_capacity,
            cipher,
        )?;
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::new();
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            config,
        })
    }

    /// Open a temporary in-memory database
    pub fn open_in_memory() -> Result<Self> {
        let config = Config::default();
        let mut pager = Pager::open_in_memory(config.page_size, config.page_cache_capacity)?;
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::new();
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            config,
        })
    }

    /// Open a read-only connection backed directly by a remote object store (S3, Cloudflare R2, MinIO, GCP)
    /// streaming pages on-demand using HTTP Range requests.
    pub fn open_remote(reader: Arc<dyn RemoteRangeReader>) -> Result<Self> {
        Self::open_remote_with_config(reader, Config::default(), None)
    }

    /// Open a remote connection with custom configuration and optional cryptographic cipher
    pub fn open_remote_with_config(
        reader: Arc<dyn RemoteRangeReader>,
        config: Config,
        cipher: Option<DatabaseCipher>,
    ) -> Result<Self> {
        let mut pager = Pager::open_remote(reader, config.page_cache_capacity, cipher)?;
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::new();
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            config,
        })
    }

    /// Open a bidirectional writable database connection synchronized with remote cloud storage (S3/R2)
    pub fn open_remote_writable(adapter: Arc<dyn RemoteStorageAdapter>) -> Result<Self> {
        let mut pager = Pager::open_remote_writable(adapter, Config::default().page_cache_capacity, None)?;
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::new();
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            config: Config::default(),
        })
    }

    /// Push local changes back to the remote cloud storage adapter
    pub fn push_remote(&self) -> Result<()> {
        self.pager.write().push_remote()
    }

    /// Return reference to active configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Return reference to internal pager lock
    pub fn pager(&self) -> &Arc<RwLock<Pager>> {
        &self.pager
    }

    /// Return reference to internal executor lock
    pub fn executor(&self) -> &Arc<RwLock<SQLExecutor>> {
        &self.executor
    }

    /// Return reference to internal memory engine lock
    pub fn memory_engine(&self) -> &Arc<RwLock<MemoryEngine>> {
        &self.memory
    }

    // --- High-Performance Graph API ---

    /// Add a Node to the embedded Knowledge Graph
    pub fn graph_add_node(&self, id: u64, label: &str, properties: &str) -> Result<()> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor.graph_mut().add_node(id, label, properties)?;
        let node = executor.graph().get_node(id).cloned().unwrap();
        executor.persist_graph_node(&mut pager, &node)
    }

    /// Add a directional Edge connecting two nodes
    pub fn graph_add_edge(
        &self,
        from_id: u64,
        to_id: u64,
        label: &str,
        weight: f32,
        properties: &str,
    ) -> Result<u64> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let edge_id = executor
            .graph_mut()
            .add_edge(from_id, to_id, label, weight, properties)?;
        let edge = executor
            .graph()
            .all_edges()
            .into_iter()
            .find(|e| e.id == edge_id)
            .cloned();
        if let Some(edge) = edge {
            executor.persist_graph_edge(&mut pager, &edge)?;
        }
        Ok(edge_id)
    }

    /// Remove a Node and all its connected edges from the Knowledge Graph
    pub fn graph_remove_node(&self, id: u64) -> Result<bool> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let (removed, removed_edges) = executor.graph_mut().remove_node(id);
        if removed {
            executor.delete_graph_node(&mut pager, id)?;
            for eid in removed_edges {
                let _ = executor.delete_graph_edge(&mut pager, eid);
            }
        }
        Ok(removed)
    }

    /// Remove an Edge connecting two nodes
    pub fn graph_remove_edge(&self, id: u64) -> Result<bool> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let removed = executor.graph_mut().remove_edge(id);
        if removed {
            executor.delete_graph_edge(&mut pager, id)?;
        }
        Ok(removed)
    }

    /// Find adjacent neighbors of a node in O(1) time
    pub fn graph_neighbors(
        &self,
        id: u64,
        direction: Direction,
        label: Option<&str>,
    ) -> Vec<(Node, Edge)> {
        let executor = self.executor.read();
        executor.graph().neighbors(id, direction, label)
    }

    /// Find shortest path between two nodes using Breadth-First Search
    pub fn graph_find_path(&self, start_id: u64, end_id: u64, max_depth: usize) -> Option<Vec<Edge>> {
        let executor = self.executor.read();
        executor.graph().find_path(start_id, end_id, max_depth)
    }

    /// Extract an entity subgraph up to `depth` hops for GraphRAG context injection
    pub fn graph_subgraph(&self, center_id: u64, depth: usize) -> (Vec<Node>, Vec<Edge>) {
        let executor = self.executor.read();
        executor.graph().extract_subgraph(center_id, depth)
    }

    /// Add a Node to the embedded Knowledge Graph with an optional dense vector embedding
    pub fn graph_add_node_with_vector(
        &self,
        id: u64,
        label: &str,
        properties: &str,
        vector: Option<&[f32]>,
    ) -> Result<()> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor
            .graph_mut()
            .add_node_with_vector(id, label, properties, vector.map(|v| v.to_vec()))?;
        let node = executor.graph().get_node(id).cloned().unwrap();
        executor.persist_graph_node(&mut pager, &node)
    }

    /// Initiate a fluent Graph-to-Vector chaining query starting from a node ID
    pub fn chain(&self, start_id: u64) -> ConnectionChain<'_> {
        ConnectionChain::new(self, vec![start_id])
    }

    /// Initiate a fluent Graph-to-Vector chaining query from multiple seed node IDs
    pub fn chain_multi(&self, start_ids: Vec<u64>) -> ConnectionChain<'_> {
        ConnectionChain::new(self, start_ids)
    }

    /// Initiate a Vector-to-Graph chaining query seeded with top-k nearest nodes in the graph
    pub fn chain_from_vector(&self, query_vec: &[f32], top_k: usize) -> Result<ConnectionChain<'_>> {
        let executor = self.executor.read();
        let graph = executor.graph();
        let mut heap = std::collections::BinaryHeap::new();
        for node in graph.all_nodes() {
            if let Some(ref nv) = node.vector {
                if nv.len() == query_vec.len() {
                    let sim = crate::vector::cosine_similarity(query_vec, nv);
                    heap.push((std::cmp::Reverse((sim * 100000.0) as i64), node.id));
                    if heap.len() > top_k {
                        heap.pop();
                    }
                }
            }
        }
        let seed_ids: Vec<u64> = heap.into_iter().map(|(_, id)| id).collect();
        drop(executor);
        Ok(ConnectionChain::new(self, seed_ids))
    }

    /// Execute an accelerated GraphRAG query combining Product Quantization vector seeding,
    /// micro-hop graph traversal, and tri-modal Reciprocal Rank Fusion (RRF).
    pub fn graph_rag_query(
        &self,
        query_text: &str,
        query_vec: Option<&[f32]>,
        config: &GraphRagConfig,
    ) -> Result<GraphRagContext> {
        let executor = self.executor.read();
        let graph = executor.graph();
        GraphRagEngine::query(graph, query_text, query_vec, None, config)
    }

    /// Simplified GraphRAG search with default parameters and custom limit
    pub fn graph_rag_search(&self, query_text: &str, limit: usize) -> Result<GraphRagContext> {
        let config = GraphRagConfig::default().with_limit(limit);
        self.graph_rag_query(query_text, None, &config)
    }

    // --- High-Performance Document DB API (MongoDB-like) ---

    /// Open or create a schema-less JSON Document Collection (analogous to MongoDB collection)
    pub fn collection(&self, name: &str) -> Result<Collection<'_>> {
        Collection::open(name, self)
    }

    // --- Native AI Agent Memory APIs ---

    /// Store a new memory into the AI Agent Memory subsystem.
    ///
    /// The memory is indexed for hybrid semantic vector search and BM25 full-text keyword retrieval.
    pub fn memory_remember(
        &self,
        content: &str,
        vector: Option<&[f32]>,
        importance: f32,
        tags: &[&str],
    ) -> Result<u64> {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.memory_remember_at(content, vector, importance, tags, now_ts)
    }

    /// Store a memory with explicit namespace and session scoping (for multi-agent architectures).
    pub fn memory_remember_scoped(
        &self,
        content: &str,
        vector: Option<&[f32]>,
        importance: f32,
        tags: &[&str],
        namespace: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<u64> {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut mem = self.memory.write();
        let id = mem.remember_scoped(content, vector, importance, tags, now_ts, namespace, session_id);
        let entry = mem.get_memory(id).cloned();
        drop(mem);
        if let Some(entry) = entry {
            let mut pager = self.pager.write();
            let mut executor = self.executor.write();
            executor.persist_memory_entry(&mut pager, &entry)?;
        }
        Ok(id)
    }

    /// Store a memory with an explicit timestamp (useful for importing historic agent memories).
    pub fn memory_remember_at(
        &self,
        content: &str,
        vector: Option<&[f32]>,
        importance: f32,
        tags: &[&str],
        timestamp: u64,
    ) -> Result<u64> {
        let mut mem = self.memory.write();
        let id = mem.remember(content, vector, importance, tags, timestamp);
        let entry = mem.get_memory(id).cloned();
        drop(mem);
        if let Some(entry) = entry {
            let mut pager = self.pager.write();
            let mut executor = self.executor.write();
            executor.persist_memory_entry(&mut pager, &entry)?;
        }
        Ok(id)
    }

    /// Recall relevant memories using hybrid Vector + BM25 + Temporal Decay scoring.
    pub fn memory_recall(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[f32]>,
        limit: usize,
        filter: &crate::memory::MemoryRecallFilter,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.memory_recall_at(query_text, query_vector, limit, filter, now_ts)
    }

    /// Recall relevant memories using Reciprocal Rank Fusion (RRF) combining BM25 lexical and Vector ranks.
    pub fn memory_recall_rrf(
        &self,
        query_text: &str,
        query_vector: Option<&[f32]>,
        limit: usize,
        rrf_k: f32,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut mem = self.memory.write();
        mem.recall_rrf(query_text, query_vector, limit, rrf_k, now_ts)
    }

    /// Turnkey Zero-Setup AI Agent Memory: remember plain text with automatic local embedding.
    ///
    /// Uses the embedded safe-Rust `DeterministicHashEmbedder` to generate high-quality 128D
    /// feature-hashed vectors in sub-microsecond latency, requiring zero external API keys or PyTorch dependencies.
    pub fn memory_remember_text(
        &self,
        content: &str,
        importance: f32,
        tags: &[&str],
    ) -> Result<u64> {
        let embedder = crate::memory::DeterministicHashEmbedder::default();
        let vector = embedder.embed_text(content);
        self.memory_remember(content, Some(&vector), importance, tags)
    }

    /// Turnkey Zero-Setup AI Agent Memory: remember plain text scoped to a namespace and session.
    pub fn memory_remember_text_scoped(
        &self,
        content: &str,
        importance: f32,
        tags: &[&str],
        namespace: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<u64> {
        let embedder = crate::memory::DeterministicHashEmbedder::default();
        let vector = embedder.embed_text(content);
        self.memory_remember_scoped(content, Some(&vector), importance, tags, namespace, session_id)
    }

    /// Turnkey Zero-Setup AI Agent Memory: recall relevant memories for a plain text query with automatic local embedding.
    ///
    /// Automatically embeds the query using the local `DeterministicHashEmbedder` and performs hybrid
    /// Vector + BM25 keyword search with temporal recency decay.
    pub fn memory_recall_text(
        &self,
        query: &str,
        limit: usize,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let embedder = crate::memory::DeterministicHashEmbedder::default();
        let vector = embedder.embed_text(query);
        let filter = crate::memory::MemoryRecallFilter::default();
        self.memory_recall(Some(query), Some(&vector), limit, &filter)
    }

    /// Turnkey Zero-Setup AI Agent Memory: recall relevant memories scoped to a namespace and session.
    pub fn memory_recall_text_scoped(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
        session_id: Option<&str>,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let embedder = crate::memory::DeterministicHashEmbedder::default();
        let vector = embedder.embed_text(query);
        let mut filter = crate::memory::MemoryRecallFilter::default();
        filter.namespace = namespace.map(|s| s.to_string());
        filter.session_id = session_id.map(|s| s.to_string());
        self.memory_recall(Some(query), Some(&vector), limit, &filter)
    }

    /// Subscribe to real-time table mutations (Reactive Live Queries & Change Data Capture).
    pub fn subscribe<F>(&self, table: &str, listener: F) -> u64
    where
        F: Fn(&ChangeEvent) + Send + Sync + 'static,
    {
        self.realtime.subscribe(table, listener)
    }

    /// Unsubscribe an active real-time table subscription.
    pub fn unsubscribe(&self, subscription_id: u64) {
        self.realtime.unsubscribe(subscription_id);
    }

    /// Return reference to internal realtime event bus.
    pub fn realtime(&self) -> &Arc<RealtimeBus> {
        &self.realtime
    }

    /// Listen for all real-time Change Data Capture (CDC) events across all tables
    pub fn listen<F>(&self, callback: F) -> u64
    where
        F: Fn(&ChangeEvent) + Send + Sync + 'static,
    {
        self.realtime.subscribe("*", callback)
    }

    /// Recall relevant memories with an explicit evaluation timestamp.
    pub fn memory_recall_at(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[f32]>,
        limit: usize,
        filter: &crate::memory::MemoryRecallFilter,
        eval_timestamp: u64,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let mut mem = self.memory.write();
        mem.recall(query_text, query_vector, limit, filter, eval_timestamp)
    }

    /// Link two memories together as an associative connection.
    pub fn memory_link(&self, id_a: u64, id_b: u64) -> Result<()> {
        let mut mem = self.memory.write();
        mem.associate(id_a, id_b)?;
        let entry_a = mem.get_memory(id_a).cloned();
        let entry_b = mem.get_memory(id_b).cloned();
        drop(mem);
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        if let Some(ea) = entry_a {
            executor.persist_memory_entry(&mut pager, &ea)?;
        }
        if let Some(eb) = entry_b {
            executor.persist_memory_entry(&mut pager, &eb)?;
        }
        Ok(())
    }

    /// Retrieve a single memory by ID.
    pub fn memory_get(&self, id: u64) -> Option<crate::memory::MemoryEntry> {
        let mem = self.memory.read();
        mem.get_memory(id).cloned()
    }

    /// Total count of memories currently held in the memory engine.
    pub fn memory_count(&self) -> usize {
        let mem = self.memory.read();
        mem.memory_count()
    }

    /// Prune old or decayed memories below the given retention threshold using system time.
    pub fn memory_prune(&self, decay_threshold: f32, half_life_seconds: u64) -> usize {
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.memory_prune_at(decay_threshold, half_life_seconds, now_ts)
    }

    /// Prune old or decayed memories with an explicit evaluation timestamp.
    pub fn memory_prune_at(
        &self,
        decay_threshold: f32,
        half_life_seconds: u64,
        eval_timestamp: u64,
    ) -> usize {
        let mut mem = self.memory.write();
        let removed_ids = mem.prune_decayed_ids_at(decay_threshold, half_life_seconds, eval_timestamp);
        let count = removed_ids.len();
        drop(mem);
        if !removed_ids.is_empty() {
            let mut pager = self.pager.write();
            let mut executor = self.executor.write();
            for id in removed_ids {
                let _ = executor.delete_memory_entry(&mut pager, id);
            }
        }
        count
    }

    /// Explicitly delete and forget a memory by ID.
    pub fn memory_forget(&self, id: u64) -> bool {
        let mut mem = self.memory.write();
        let removed = mem.forget(id);
        drop(mem);
        if removed {
            let mut pager = self.pager.write();
            let mut executor = self.executor.write();
            let _ = executor.delete_memory_entry(&mut pager, id);
        }
        removed
    }

    // --- Write-Ahead Log (WAL) & Durability ---

    /// Manually trigger a WAL checkpoint, flushing all committed pages
    /// from `.tapir-wal` to the main database file and resetting the log.
    pub fn checkpoint(&self) -> Result<usize> {
        let mut pager = self.pager.write();
        pager.checkpoint()
    }

    /// Enable transparent LZ4 block compression for all pages > 1
    pub fn enable_compression(&self) {
        let mut pager = self.pager.write();
        pager.enable_compression();
    }

    /// Check whether transparent page compression is enabled on the database
    pub fn is_compressed(&self) -> bool {
        let pager = self.pager.read();
        pager.is_compressed()
    }

    /// Perform a non-blocking hot online backup of the active database into a target `.tapir` file.
    /// Checkpoints the WAL and writes encrypted/decrypted pages sequentially to `dest_path`.
    pub fn backup<P: AsRef<Path>>(&self, dest_path: P) -> Result<u32> {
        let mut pager = self.pager.write();
        pager.backup_to(dest_path.as_ref())
    }

    /// Vacuum and compact the database in place, flushing all WAL frames, synchronizing storage, and purging vector tombstones.
    pub fn vacuum(&self) -> Result<usize> {
        self.execute("VACUUM;")
    }

    /// Vacuum and create a compacted hot backup into a target file.
    pub fn vacuum_into<P: AsRef<Path>>(&self, dest_path: P) -> Result<u32> {
        let mut pager = self.pager.write();
        pager.backup_to(dest_path.as_ref())
    }

    /// Export uncheckpointed WAL frames for real-time streaming replication.
    pub fn export_wal_frames(&self) -> Result<Vec<(u32, Vec<u8>)>> {
        let pager = self.pager.read();
        pager.export_wal_frames()
    }

    /// Apply a replicated WAL frame directly into this database instance.
    pub fn apply_wal_frame(&self, page_id: u32, data: &[u8]) -> Result<()> {
        let mut pager = self.pager.write();
        pager.apply_wal_frame(page_id, data)
    }

    /// Persist all active vector index topologies directly to disk snapshot table
    pub fn persist_vector_snapshots(&self) -> Result<()> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor.persist_vector_indexes_to_disk(&mut pager)
    }

    /// Check if currently in an active transaction
    pub fn is_in_transaction(&self) -> bool {
        let pager = self.pager.read();
        pager.is_in_transaction()
    }

    /// Begin a transaction
    pub fn begin_transaction(&self) -> Result<()> {
        let mut pager = self.pager.write();
        pager.begin_transaction()
    }

    /// Commit the active transaction
    pub fn commit(&self) -> Result<()> {
        let _ = self.persist_vector_snapshots();
        let mut pager = self.pager.write();
        pager.commit_transaction()
    }

    /// Rollback the active transaction
    pub fn rollback(&self) -> Result<()> {
        let mut pager = self.pager.write();
        pager.rollback_transaction()?;
        let mut executor = self.executor.write();
        *executor = SQLExecutor::new(&mut pager)?;
        executor.load_graph_from_disk(&mut pager)?;
        let mut memory = self.memory.write();
        *memory = MemoryEngine::new();
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(())
    }

    // --- Introspection & Utility APIs ---

    /// Return all user-defined relational tables
    pub fn tables(&self) -> Vec<crate::sql::catalog::TableDef> {
        let executor = self.executor.read();
        executor
            .tables()
            .into_iter()
            .filter(|t| !t.name.starts_with("__doc_") && !t.name.starts_with("__sys_"))
            .collect()
    }

    /// Return schema definition for a specific table
    pub fn table(&self, name: &str) -> Option<crate::sql::catalog::TableDef> {
        let executor = self.executor.read();
        executor.get_table(name).cloned()
    }

    /// Return all view names defined in the database
    pub fn views(&self) -> Vec<String> {
        let executor = self.executor.read();
        executor.views().into_iter().map(|v| v.name).collect()
    }

    /// List all Document collection names
    pub fn collections(&self) -> Vec<String> {
        let executor = self.executor.read();
        executor
            .tables()
            .into_iter()
            .filter_map(|t| {
                if t.name.starts_with("__doc_") {
                    Some(t.name.trim_start_matches("__doc_").to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Return statistics on the embedded Knowledge Graph: `(node_count, edge_count)`
    pub fn graph_stats(&self) -> (usize, usize) {
        let executor = self.executor.read();
        (executor.graph().node_count(), executor.graph().edge_count())
    }

    /// Return all nodes in the embedded Knowledge Graph
    pub fn graph_nodes(&self) -> Vec<Node> {
        let executor = self.executor.read();
        executor.graph().all_nodes().into_iter().cloned().collect()
    }

    /// Return all edges in the embedded Knowledge Graph
    pub fn graph_edges(&self) -> Vec<Edge> {
        let executor = self.executor.read();
        executor.graph().all_edges().into_iter().cloned().collect()
    }

    /// Compute PageRank centrality scores for all nodes in the Knowledge Graph
    pub fn graph_pagerank(&self, damping_factor: f32, max_iterations: usize, tolerance: f32) -> std::collections::HashMap<u64, f32> {
        let executor = self.executor.read();
        executor.graph().pagerank(damping_factor, max_iterations, tolerance)
    }

    /// Calculate weighted shortest path between two nodes using Dijkstra's algorithm
    pub fn graph_dijkstra_path(&self, start_id: u64, end_id: u64) -> Option<(Vec<Edge>, f32)> {
        let executor = self.executor.read();
        executor.graph().dijkstra_shortest_path(start_id, end_id)
    }

    /// Execute a declarative openCypher query (`MATCH ... WHERE ... RETURN ...`)
    pub fn query_cypher(&self, cypher_sql: &str) -> Result<Vec<Row>> {
        let executor = self.executor.read();
        crate::graph::cypher::CypherExecutor::execute_query(executor.graph(), cypher_sql)
    }

    /// Convert the embedded Knowledge Graph to a contiguous Compressed Sparse Row (CSR) topology
    pub fn graph_to_csr(&self) -> crate::graph::csr::CsrGraph {
        let executor = self.executor.read();
        executor.graph().to_csr()
    }

    /// Execute a non-query SQL command
    pub fn execute(&self, sql: &str) -> Result<usize> {
        DatabaseConnection::execute(self, sql)
    }

    /// Execute a SQL query returning rows
    pub fn query(&self, sql: &str) -> Result<Vec<Row>> {
        DatabaseConnection::query(self, sql)
    }

    /// Prepare a SQL statement for safe parameterized execution
    pub fn prepare<'a>(&'a self, sql: &str) -> Result<PreparedStatement<'a>> {
        let tokens = crate::sql::lexer::tokenize(sql)?;
        Ok(PreparedStatement { conn: self, tokens })
    }

    /// Execute a parameterized non-query SQL command safely
    pub fn execute_with_params(&self, sql: &str, params: &[Value]) -> Result<usize> {
        let stmt = self.prepare(sql)?;
        stmt.execute(params)
    }

    /// Execute a parameterized SQL query returning rows safely
    pub fn query_with_params(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        let stmt = self.prepare(sql)?;
        stmt.query(params)
    }

    /// Execute a SQL query and return results formatted as a JSON array string
    pub fn query_json(&self, sql: &str) -> Result<String> {
        let rows = self.query(sql)?;
        serde_json::to_string(&rows)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize query rows to JSON: {e}")))
    }
}

/// A prepared SQL statement with pre-tokenized query and cached token stream for parameterized execution
pub struct PreparedStatement<'a> {
    conn: &'a Connection,
    tokens: Vec<crate::sql::lexer::Token>,
}

impl<'a> PreparedStatement<'a> {
    /// Execute prepared non-query SQL command with parameter binding
    pub fn execute(&self, params: &[Value]) -> Result<usize> {
        if self.tokens.is_empty() {
            return Ok(0);
        }
        let bound = crate::sql::parser::bind_parameters(&self.tokens, params)?;
        let stmt = crate::sql::parser::parse_tokens(&bound)?;

        // Capture mutation metadata for CDC broadcast
        let cdc_info = match &stmt {
            Statement::Insert { table, .. } => Some((ChangeOp::Insert, table.clone())),
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };

        let mut pager = self.conn.pager.write();
        let mut executor = self.conn.executor.write();
        let affected = executor.execute(&mut pager, stmt)?;

        if let Some((op, table)) = cdc_info {
            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.conn.realtime.publish(&ChangeEvent {
                op,
                table,
                row_id: affected as u64,
                timestamp: now_ts,
                data: serde_json::json!({"affected_rows": affected}),
            });
        }

        Ok(affected)
    }

    /// Execute prepared SQL query returning rows with parameter binding
    pub fn query(&self, params: &[Value]) -> Result<Vec<Row>> {
        if self.tokens.is_empty() {
            return Ok(Vec::new());
        }
        let bound = crate::sql::parser::bind_parameters(&self.tokens, params)?;
        let stmt = crate::sql::parser::parse_tokens(&bound)?;

        // Capture mutation metadata for CDC broadcast if RETURNING was used
        let cdc_info = match &stmt {
            Statement::Insert { table, .. } => Some((ChangeOp::Insert, table.clone())),
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };

        let mut pager = self.conn.pager.write();
        let mut executor = self.conn.executor.write();
        let rows = executor.query(&mut pager, stmt)?;

        if let Some((op, table)) = cdc_info {
            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.conn.realtime.publish(&ChangeEvent {
                op,
                table,
                row_id: rows.len() as u64,
                timestamp: now_ts,
                data: serde_json::json!({"affected_rows": rows.len()}),
            });
        }

        Ok(rows)
    }
}

impl DatabaseConnection for Connection {
    /// Execute a non-query SQL command (e.g. CREATE TABLE, INSERT, UPDATE, DELETE)
    fn execute(&self, sql: &str) -> Result<usize> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        let upper = trimmed.to_uppercase();
        if upper.starts_with("CREATE (") || upper.starts_with("CREATE(") {
            let mut executor = self.executor.write();
            let rows = crate::graph::cypher::CypherExecutor::execute_mutation(executor.graph_mut(), trimmed)?;
            let affected: usize = rows
                .first()
                .and_then(|r| r.get::<i64>("nodes_created").ok())
                .map(|i| i as usize)
                .unwrap_or(0);
            return Ok(affected);
        }
        let tokens = crate::sql::lexer::tokenize(trimmed)?;
        if tokens.is_empty() {
            return Ok(0);
        }
        let stmt = crate::sql::parser::parse_tokens(&tokens)?;

        // Capture mutation metadata for CDC broadcast
        let cdc_info = match &stmt {
            Statement::Insert { table, .. } => Some((ChangeOp::Insert, table.clone())),
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };

        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let affected = executor.execute(&mut pager, stmt)?;

        if let Some((op, table)) = cdc_info {
            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.realtime.publish(&ChangeEvent {
                op,
                table,
                row_id: affected as u64,
                timestamp: now_ts,
                data: serde_json::json!({"affected_rows": affected}),
            });
        }

        Ok(affected)
    }

    /// Execute a SQL query returning multiple rows (also supports openCypher MATCH queries)
    fn query(&self, sql: &str) -> Result<Vec<Row>> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let upper = trimmed.to_uppercase();
        if upper.starts_with("MATCH") || upper.starts_with("CYPHER") {
            return self.query_cypher(trimmed);
        }
        let tokens = crate::sql::lexer::tokenize(trimmed)?;
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let stmt = crate::sql::parser::parse_tokens(&tokens)?;

        // Capture mutation metadata for CDC broadcast if RETURNING was used
        let cdc_info = match &stmt {
            Statement::Insert { table, .. } => Some((ChangeOp::Insert, table.clone())),
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };

        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let rows = executor.query(&mut pager, stmt)?;

        if let Some((op, table)) = cdc_info {
            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.realtime.publish(&ChangeEvent {
                op,
                table,
                row_id: rows.len() as u64,
                timestamp: now_ts,
                data: serde_json::json!({"affected_rows": rows.len()}),
            });
        }

        Ok(rows)
    }

    /// Execute a parameterized non-query SQL command
    fn execute_with_params(&self, sql: &str, params: &[Value]) -> Result<usize> {
        Connection::execute_with_params(self, sql, params)
    }

    /// Execute a parameterized SQL query returning multiple rows
    fn query_with_params(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        Connection::query_with_params(self, sql, params)
    }
}

fn read_or_generate_salt(path: &Path) -> Result<[u8; 16]> {
    use std::io::Read;
    if path.exists() {
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut buf = [0u8; 100];
            if f.read_exact(&mut buf).is_ok() {
                let mut salt = [0u8; 16];
                salt.copy_from_slice(&buf[49..65]);
                if salt != [0u8; 16] {
                    return Ok(salt);
                }
            }
        }
    }
    Ok(crate::crypto::generate_random_salt())
}

/// Fluent Chaining Query Builder for a Database Connection
pub struct ConnectionChain<'a> {
    conn: &'a Connection,
    start_nodes: Vec<u64>,
    steps: Vec<crate::graph::TraversalStep>,
    node_filters: Vec<crate::graph::NodeFilter>,
    max_hops: usize,
}

impl<'a> ConnectionChain<'a> {
    /// Create a new ConnectionChain
    pub fn new(conn: &'a Connection, start_nodes: Vec<u64>) -> Self {
        Self {
            conn,
            start_nodes,
            steps: Vec::new(),
            node_filters: Vec::new(),
            max_hops: 10,
        }
    }

    /// Traverse outgoing edges: `(A) -[label]-> (B)`
    pub fn out<S: Into<String>>(mut self, label: Option<S>) -> Self {
        self.steps.push(crate::graph::TraversalStep {
            direction: Direction::Outgoing,
            edge_label: label.map(Into::into),
            min_weight: None,
        });
        self
    }

    /// Traverse incoming edges: `(A) <-[label]- (B)`
    pub fn in_<S: Into<String>>(mut self, label: Option<S>) -> Self {
        self.steps.push(crate::graph::TraversalStep {
            direction: Direction::Incoming,
            edge_label: label.map(Into::into),
            min_weight: None,
        });
        self
    }

    /// Traverse edges in either direction: `(A) --[label]-- (B)`
    pub fn both<S: Into<String>>(mut self, label: Option<S>) -> Self {
        self.steps.push(crate::graph::TraversalStep {
            direction: Direction::Both,
            edge_label: label.map(Into::into),
            min_weight: None,
        });
        self
    }

    /// Set minimum relationship weight required for traversal
    pub fn min_weight(mut self, weight: f32) -> Self {
        if let Some(last) = self.steps.last_mut() {
            last.min_weight = Some(weight);
        }
        self
    }

    /// Filter candidate nodes by label
    pub fn filter_label<S: Into<String>>(mut self, label: S) -> Self {
        self.node_filters.push(crate::graph::NodeFilter::Label(label.into()));
        self
    }

    /// Filter candidate nodes by property presence
    pub fn filter_property<S1: Into<String>, S2: Into<String>>(
        mut self,
        key: S1,
        val: S2,
    ) -> Self {
        self.node_filters.push(crate::graph::NodeFilter::PropertyContains {
            key: key.into(),
            val: val.into(),
        });
        self
    }

    /// Set maximum traversal hop depth
    pub fn max_hops(mut self, hops: usize) -> Self {
        self.max_hops = hops;
        self
    }

    /// Execute graph traversal and rank candidate nodes by Vector Similarity
    pub fn vector_near(
        self,
        query_vec: &[f32],
        top_k: usize,
        metric: DistanceMetric,
    ) -> Result<Vec<ChainMatch>> {
        let executor = self.conn.executor.read();
        let graph = executor.graph();

        let mem = self.conn.memory.read();
        let resolver = |id: u64| {
            mem.get_memory(id).and_then(|e| e.vector.clone())
        };

        let mut chain = crate::graph::GraphChain::new(graph, self.start_nodes);
        chain = chain.with_vector_resolver(&resolver);
        for step in self.steps {
            chain = match step.direction {
                Direction::Outgoing => chain.out(step.edge_label),
                Direction::Incoming => chain.in_(step.edge_label),
                Direction::Both => chain.both(step.edge_label),
            };
            if let Some(min_w) = step.min_weight {
                chain = chain.min_weight(min_w);
            }
        }
        for filter in self.node_filters {
            chain = match filter {
                crate::graph::NodeFilter::Label(l) => chain.filter_label(l),
                crate::graph::NodeFilter::PropertyContains { key, val } => {
                    chain.filter_property(key, val)
                }
            };
        }
        chain.max_hops(self.max_hops).vector_near(query_vec, top_k, metric)
    }

    /// Execute graph traversal and rank candidate nodes by BM25 full-text similarity
    pub fn bm25_search(self, query_text: &str, top_k: usize) -> Result<Vec<ChainMatch>> {
        let executor = self.conn.executor.read();
        let graph = executor.graph();
        let mut chain = crate::graph::GraphChain::new(graph, self.start_nodes);
        for step in self.steps {
            chain = match step.direction {
                Direction::Outgoing => chain.out(step.edge_label),
                Direction::Incoming => chain.in_(step.edge_label),
                Direction::Both => chain.both(step.edge_label),
            };
            if let Some(min_w) = step.min_weight {
                chain = chain.min_weight(min_w);
            }
        }
        for filter in self.node_filters {
            chain = match filter {
                crate::graph::NodeFilter::Label(l) => chain.filter_label(l),
                crate::graph::NodeFilter::PropertyContains { key, val } => {
                    chain.filter_property(key, val)
                }
            };
        }
        chain.max_hops(self.max_hops).bm25_search(query_text, top_k)
    }

    /// Execute graph traversal and rank candidate nodes using Hybrid Ranking (Vector + BM25)
    pub fn hybrid_rank(
        self,
        query_text: &str,
        query_vec: &[f32],
        top_k: usize,
        vector_weight: f32,
    ) -> Result<Vec<ChainMatch>> {
        let executor = self.conn.executor.read();
        let graph = executor.graph();

        let mem = self.conn.memory.read();
        let resolver = |id: u64| {
            mem.get_memory(id).and_then(|e| e.vector.clone())
        };

        let mut chain = crate::graph::GraphChain::new(graph, self.start_nodes);
        chain = chain.with_vector_resolver(&resolver);
        for step in self.steps {
            chain = match step.direction {
                Direction::Outgoing => chain.out(step.edge_label),
                Direction::Incoming => chain.in_(step.edge_label),
                Direction::Both => chain.both(step.edge_label),
            };
            if let Some(min_w) = step.min_weight {
                chain = chain.min_weight(min_w);
            }
        }
        for filter in self.node_filters {
            chain = match filter {
                crate::graph::NodeFilter::Label(l) => chain.filter_label(l),
                crate::graph::NodeFilter::PropertyContains { key, val } => {
                    chain.filter_property(key, val)
                }
            };
        }
        chain
            .max_hops(self.max_hops)
            .hybrid_rank(query_text, query_vec, top_k, vector_weight)
    }

    /// Collect all surviving nodes without vector scoring
    pub fn collect_nodes(self) -> Vec<Node> {
        let executor = self.conn.executor.read();
        let graph = executor.graph();
        let mut chain = crate::graph::GraphChain::new(graph, self.start_nodes);
        for step in self.steps {
            chain = match step.direction {
                Direction::Outgoing => chain.out(step.edge_label),
                Direction::Incoming => chain.in_(step.edge_label),
                Direction::Both => chain.both(step.edge_label),
            };
            if let Some(min_w) = step.min_weight {
                chain = chain.min_weight(min_w);
            }
        }
        for filter in self.node_filters {
            chain = match filter {
                crate::graph::NodeFilter::Label(l) => chain.filter_label(l),
                crate::graph::NodeFilter::PropertyContains { key, val } => {
                    chain.filter_property(key, val)
                }
            };
        }
        chain.max_hops(self.max_hops).collect_nodes()
    }

    /// Collect all surviving node IDs
    pub fn collect_ids(self) -> Vec<u64> {
        let executor = self.conn.executor.read();
        let graph = executor.graph();
        let mut chain = crate::graph::GraphChain::new(graph, self.start_nodes);
        for step in self.steps {
            chain = match step.direction {
                Direction::Outgoing => chain.out(step.edge_label),
                Direction::Incoming => chain.in_(step.edge_label),
                Direction::Both => chain.both(step.edge_label),
            };
            if let Some(min_w) = step.min_weight {
                chain = chain.min_weight(min_w);
            }
        }
        for filter in self.node_filters {
            chain = match filter {
                crate::graph::NodeFilter::Label(l) => chain.filter_label(l),
                crate::graph::NodeFilter::PropertyContains { key, val } => {
                    chain.filter_property(key, val)
                }
            };
        }
        chain.max_hops(self.max_hops).collect_ids()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let conn = Connection::open_in_memory().expect("Should open in-memory connection");
        assert_eq!(conn.config().page_size, 4096);
        assert_eq!(
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY);")
                .unwrap(),
            1
        );
        assert_eq!(
            conn.execute("INSERT INTO test (id) VALUES (42);").unwrap(),
            1
        );
        let rows = conn.query("SELECT id FROM test;").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<i64>("id").unwrap(), 42);
    }

    #[test]
    fn test_connection_graph_api() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        conn.graph_add_node(1, "Person", r#"{"name":"Faiz"}"#).unwrap();
        conn.graph_add_node(2, "Project", r#"{"name":"TapirusDB"}"#).unwrap();
        conn.graph_add_edge(1, 2, "FOUNDER_OF", 1.0, "").unwrap();

        let neighbors = conn.graph_neighbors(1, Direction::Outgoing, Some("FOUNDER_OF"));
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0.id, 2);
    }

    #[test]
    fn test_turnkey_memory_remember_and_recall_text() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let id1 = conn
            .memory_remember_text(
                "TapirusDB is a high performance safe rust embedded database",
                0.9,
                &["database", "rust"],
            )
            .unwrap();
        let id2 = conn
            .memory_remember_text(
                "The quick brown fox jumps over the lazy dog",
                0.3,
                &["animals"],
            )
            .unwrap();
        assert!(id1 > 0);
        assert!(id2 > 0);

        let results = conn.memory_recall_text("embedded safe rust database", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].entry.id, id1);
        assert!(results[0].combined_score > 0.0);
    }
}
