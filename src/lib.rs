//! # TapirusDB (`tapirus`)
//!
//! The embedded Rust database engine with SQL, vector search and local AI memory APIs.
//!
//! Stored entirely in a single `.tapir` file with zero background server processes.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod btree;
pub mod crypto;
pub mod document;
pub mod editions;
pub mod embedded;
pub mod error;
pub mod graph;
#[cfg(feature = "quad")]
pub mod iot;
#[cfg(feature = "std")]
pub mod lakehouse;
pub mod memory;
pub mod pager;
pub mod realtime;
pub mod session;
pub mod sql;
pub mod tap;
pub mod traits;
pub mod vector;

pub use btree::{BTreeEngine, PageHeader, PageType, TableLeafCell};
pub use crypto::DatabaseCipher;
pub use document::Collection;
pub use embedded::{
    DEFAULT_MICRO_BLOCK_SIZE, FlashBlockDevice, MICRO_DATABASE_MAGIC, MicroDatabase, MicroHeader,
    MicroPager, RamBlockDevice,
};
pub use error::{Error, Result};
pub use graph::{
    ChainMatch, Direction, Edge, GraphAdjacencyRecord, GraphChain, GraphEngine, GraphRagConfig,
    GraphRagContext, GraphRagEngine, GraphRagResult, Node, NodeFilter, TraversalStep,
};
pub use memory::{
    Bm25Index, Bm25Params, DeterministicHashEmbedder, EmbeddingEngine, HttpEmbeddingEngine,
    MemoryAccessScope, MemoryConfig, MemoryDeleteScope, MemoryDocument, MemoryEngine, MemoryEntry,
    MemoryFusion, MemoryRecallFilter, MemoryRecallResult, MemorySearch, MemorySearchDiagnostics,
    MemorySearchResponse, MemoryStats, reciprocal_rank_fusion,
};
#[cfg(feature = "cloud-s3")]
pub use pager::CloudS3RemoteStorage;
pub use pager::{
    DEFAULT_PAGE_SIZE, DatabaseHeader, MockRemoteRangeStorage, PageId, Pager, RemotePager,
    RemoteRangeReader, RemoteStorageAdapter, S3StorageConfig, StorageHealth, WalReplicationChunk,
    WalReplicationFrame, WalReplicationReceiver, WalReplicationStream,
};
pub use realtime::{ChangeEvent, ChangeOp, RealtimeBus};
pub use session::{Session, SessionDatabase, SnapshotSession};
pub use sql::{
    SQLExecutor, ScalarUdf, Statement, UdfRegistry, bind_parameters, parse_sql, parse_tokens,
};
pub use tap::{
    BitNetBlock, BitNetLinear, BitNetRmsNorm, ClassificationResult, RouteResult, ScoreResult,
    TapConfig, TapDeepEngine, TapEngine, TapInferenceEngine, TapRuntime, TapTokenizer, TapWeights,
    TernaryVal, VerifyResult,
};
pub use traits::{DatabaseConnection, FromValue, HashableValue, Row, Value, VectorIndexEngine};
pub use vector::{
    DistanceMetric, HnswIndex, PagedHnswHeader, PagedHnswIndex, PagedVectorStore, ProductQuantizer,
    QuantizedVector8, QuantizedVectorPQ, RaBitQuantizedVector, RaBitQuantizer, Vector,
};

/// Comprehensive diagnostic report of database page, cryptographic, and structural integrity
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntegrityReport {
    /// Format version
    pub version: u16,
    /// Page size in bytes
    pub page_size: usize,
    /// Total pages recorded in header
    pub total_pages: u32,
    /// Number of pages successfully verified
    pub pages_verified: u32,
    /// Header CRC32 checksum validity
    pub header_crc_valid: bool,
    /// Stored CRC32 checksum
    pub stored_crc: u32,
    /// Calculated CRC32 checksum
    pub calculated_crc: u32,
    /// Is page-level ChaCha20-Poly1305 encryption enabled
    pub is_encrypted: bool,
    /// Is transparent LZ4 page compression enabled
    pub is_compressed: bool,
    /// Active WAL sequence
    pub wal_sequence: u32,
    /// Number of user-defined relational tables
    pub tables_count: usize,
    /// Number of JSON document collections
    pub collections_count: usize,
    /// Knowledge graph nodes count
    pub graph_nodes_count: usize,
    /// Knowledge graph edges count
    pub graph_edges_count: usize,
    /// List of corruption or anomaly error descriptions (empty if 100% clean)
    pub errors: Vec<String>,
}

impl IntegrityReport {
    /// Check whether the database has 100% integrity with zero errors
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && self.header_crc_valid
    }
}

/// TapirusDB package version string
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use parking_lot::RwLock;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroize;

/// Configuration options for opening a TapirusDB connection
#[derive(Clone)]
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
    /// Source/index admission limits applied before loading persisted AI memory.
    pub memory: MemoryConfig,
    /// Open an existing database in non-blocking read-only mode without `.tapir-lock`.
    pub read_only: bool,
    /// Enable multi-process WAL coordination (acquires `.tapir-lock` only during writes).
    pub concurrent_access: bool,
    /// Maximum time to wait for `.tapir-lock` when `concurrent_access` is enabled.
    pub busy_timeout: std::time::Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            page_cache_capacity: 1024,
            auto_checkpoint: true,
            passphrase: None,
            encryption_key: None,
            memory: MemoryConfig::default(),
            read_only: false,
            concurrent_access: false,
            busy_timeout: std::time::Duration::ZERO,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("page_size", &self.page_size)
            .field("page_cache_capacity", &self.page_cache_capacity)
            .field("auto_checkpoint", &self.auto_checkpoint)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "encryption_key",
                &self.encryption_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("memory", &self.memory)
            .field("read_only", &self.read_only)
            .field("concurrent_access", &self.concurrent_access)
            .field("busy_timeout", &self.busy_timeout)
            .finish()
    }
}

impl Zeroize for Config {
    fn zeroize(&mut self) {
        if let Some(ref mut passphrase) = self.passphrase {
            passphrase.zeroize();
        }
        self.passphrase = None;
        if let Some(ref mut key) = self.encryption_key {
            key.zeroize();
        }
        self.encryption_key = None;
    }
}

fn take_cipher_from_config(
    config: &mut Config,
    get_salt: impl FnOnce() -> Result<[u8; 16]>,
) -> Result<Option<DatabaseCipher>> {
    let cipher = if let Some(mut key) = config.encryption_key.take() {
        let salt = get_salt()?;
        let cipher = DatabaseCipher::new(key, salt);
        key.zeroize();
        config.zeroize();
        Some(cipher)
    } else if let Some(mut pass) = config.passphrase.take() {
        let salt = get_salt()?;
        let cipher = DatabaseCipher::from_passphrase(&pass, salt);
        pass.zeroize();
        config.zeroize();
        Some(cipher)
    } else {
        None
    };
    Ok(cipher)
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
    pending_changes: Arc<parking_lot::Mutex<Vec<(usize, ChangeEvent)>>>,
    config: Config,
    memory_scope: Option<MemoryAccessScope>,
}

/// A wrapper that distributes cloned handles to one shared connection.
///
/// Handles returned by [`ConnectionPool::acquire`] share session and transaction state;
/// this type does not provide independent database sessions.
#[derive(Clone)]
pub struct ConnectionPool {
    conn: Connection,
}

fn predicted_insert_row_id_for_values(
    executor: &SQLExecutor,
    table: &str,
    columns: Option<&[String]>,
    values: &[Value],
) -> Option<u64> {
    let definition = executor.get_table(table)?;
    let primary_key_index = definition.primary_key_index();
    let row_id = if let Some(primary_key_index) = primary_key_index {
        let key_column = &definition.columns[primary_key_index].name;
        let value_index = columns
            .and_then(|columns| {
                columns
                    .iter()
                    .position(|column| column.eq_ignore_ascii_case(key_column))
            })
            .unwrap_or(primary_key_index);
        match values.get(value_index) {
            Some(Value::Integer(value)) => *value as u64,
            Some(Value::Null) | Some(_) | None => definition.next_row_id,
        }
    } else {
        definition.next_row_id
    };
    Some(row_id)
}

fn predicted_insert_row_id(executor: &SQLExecutor, statement: &Statement) -> Option<u64> {
    match statement {
        Statement::Insert {
            table,
            columns,
            values,
            ..
        } => predicted_insert_row_id_for_values(executor, table, columns.as_deref(), values),
        Statement::InsertMany {
            table,
            columns,
            rows,
            ..
        } if rows.len() == 1 => rows.first().and_then(|values| {
            predicted_insert_row_id_for_values(executor, table, columns.as_deref(), values)
        }),
        _ => None,
    }
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

    /// Clone the pool's shared connection handle.
    ///
    /// The returned handle shares transaction state with the pool and every other
    /// acquired handle. Use it as a shared session, not as an isolated connection.
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

    /// Open an existing database in non-blocking read-only mode without acquiring `.tapir-lock`.
    pub fn open_read_only<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_read_only_with_config(path, Config::default())
    }

    /// Open an existing database in non-blocking read-only mode with custom configuration.
    pub fn open_read_only_with_config<P: AsRef<Path>>(path: P, mut config: Config) -> Result<Self> {
        config.read_only = true;
        config.concurrent_access = false;
        Self::open_with_config(path, config)
    }

    /// Open a database file with multi-process WAL coordination and default 5s busy timeout.
    ///
    /// Multiple processes can read concurrently while writes coordinate safely via short-lived
    /// `.tapir-lock` acquisition and automatic WAL state synchronization.
    pub fn open_concurrent<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config = Config {
            concurrent_access: true,
            busy_timeout: std::time::Duration::from_secs(5),
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// Open a database file with multi-process WAL coordination and custom configuration.
    pub fn open_concurrent_with_config<P: AsRef<Path>>(
        path: P,
        mut config: Config,
    ) -> Result<Self> {
        config.concurrent_access = true;
        Self::open_with_config(path, config)
    }

    /// Open a database file with custom configuration options
    pub fn open_with_config<P: AsRef<Path>>(path: P, mut config: Config) -> Result<Self> {
        if config.read_only && !path.as_ref().exists() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Database file '{}' does not exist", path.as_ref().display()),
            )));
        }

        let cipher = take_cipher_from_config(&mut config, || read_or_generate_salt(path.as_ref()))?;

        let mut pager = if config.read_only {
            Pager::open_read_only_with_cipher(path.as_ref(), config.page_cache_capacity, cipher)?
        } else if config.concurrent_access {
            Pager::open_concurrent_with_cipher(
                path.as_ref(),
                config.page_size,
                config.page_cache_capacity,
                cipher,
                config.busy_timeout,
            )?
        } else {
            Pager::open_with_cipher(
                path.as_ref(),
                config.page_size,
                config.page_cache_capacity,
                cipher,
            )?
        };
        pager.set_auto_checkpoint(config.auto_checkpoint);
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::with_config(config.memory.clone())?;
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
        })
    }

    /// Open a temporary in-memory database
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_config(Config::default())
    }

    /// Open a temporary encrypted in-memory database using a user passphrase.
    /// Exported snapshots (`export_bytes`) remain encrypted at rest with ChaCha20-Poly1305.
    pub fn open_encrypted_in_memory(passphrase: &str) -> Result<Self> {
        let config = Config {
            passphrase: Some(passphrase.to_string()),
            ..Default::default()
        };
        Self::open_in_memory_with_config(config)
    }

    /// Open a temporary encrypted in-memory database using a raw 256-bit key.
    /// Exported snapshots (`export_bytes`) remain encrypted at rest with ChaCha20-Poly1305.
    pub fn open_encrypted_in_memory_with_key(mut key: [u8; 32]) -> Result<Self> {
        let config = Config {
            encryption_key: Some(key),
            ..Default::default()
        };
        key.zeroize();
        Self::open_in_memory_with_config(config)
    }

    /// Open temporary storage with explicit admission, cache, and optional encryption limits.
    pub fn open_in_memory_with_config(mut config: Config) -> Result<Self> {
        let cipher =
            take_cipher_from_config(&mut config, || Ok(crate::crypto::generate_random_salt()))?;
        let mut pager = Pager::open_in_memory_with_cipher(
            config.page_size,
            config.page_cache_capacity,
            cipher,
        )?;
        pager.set_auto_checkpoint(config.auto_checkpoint);
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::with_config(config.memory.clone())?;
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
        })
    }

    /// Open a database connection initialized directly from a raw byte buffer (e.g. from browser OPFS or IndexedDB).
    pub fn open_from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::open_from_bytes_with_config(bytes, Config::default(), None)
    }

    /// Open an encrypted database connection initialized directly from raw bytes with passphrase.
    pub fn open_encrypted_from_bytes(bytes: &[u8], passphrase: &str) -> Result<Self> {
        if bytes.len() < crate::pager::DATABASE_HEADER_SIZE {
            return Err(Error::Corrupted(
                "Byte buffer smaller than database header".into(),
            ));
        }
        let header = crate::pager::DatabaseHeader::from_bytes(
            &bytes[0..crate::pager::DATABASE_HEADER_SIZE],
        )?;
        let cipher = Some(DatabaseCipher::from_passphrase(passphrase, header.salt));
        Self::open_from_bytes_with_config(bytes, Config::default(), cipher)
    }

    /// Open a database connection from bytes with custom configuration and optional cipher
    pub fn open_from_bytes_with_config(
        bytes: &[u8],
        config: Config,
        cipher: Option<DatabaseCipher>,
    ) -> Result<Self> {
        let mut pager = Pager::open_in_memory_from_bytes(bytes, cipher)?;
        pager.set_auto_checkpoint(config.auto_checkpoint);
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::with_config(config.memory.clone())?;
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
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
        pager.set_auto_checkpoint(config.auto_checkpoint);
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::with_config(config.memory.clone())?;
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
        })
    }

    /// Open a bidirectional writable database connection synchronized with remote cloud storage (S3/R2)
    pub fn open_remote_writable(adapter: Arc<dyn RemoteStorageAdapter>) -> Result<Self> {
        let config = Config::default();
        let mut pager =
            Pager::open_remote_writable(adapter, Config::default().page_cache_capacity, None)?;
        pager.set_auto_checkpoint(config.auto_checkpoint);
        let mut executor = SQLExecutor::new(&mut pager)?;
        let mut memory = MemoryEngine::with_config(config.memory.clone())?;
        executor.load_graph_from_disk(&mut pager)?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        executor.load_vector_indexes_from_disk(&mut pager)?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
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

    /// Return `true` when page-level ChaCha20-Poly1305 encryption is active on this connection.
    pub fn is_encrypted(&self) -> bool {
        self.pager.read().header().encryption_flags != 0
    }

    /// Return a cloned connection handle strictly confined to a single owner namespace and optional session.
    ///
    /// Already-scoped handles can only narrow (for example binding a session inside the same namespace)
    /// and cannot escalate across namespaces or remove an existing session bound.
    pub fn scoped_memory(&self, namespace: &str, session_id: Option<&str>) -> Result<Self> {
        let next = MemoryAccessScope::new(namespace, session_id.map(str::to_owned))?;
        if let Some(existing) = &self.memory_scope {
            existing.resolve_target(Some(&next.namespace), next.session_id.as_deref())?;
            if existing.session_id.is_some() && next.session_id.is_none() {
                return Err(Error::PermissionDenied(
                    "Cannot broaden session scope on an already session-scoped connection".into(),
                ));
            }
        }
        let mut scoped = self.clone();
        scoped.memory_scope = Some(next);
        Ok(scoped)
    }

    /// Return the active owner isolation boundary, if this connection is memory-scoped.
    pub fn memory_scope(&self) -> Option<&MemoryAccessScope> {
        self.memory_scope.as_ref()
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

    /// Access the TAP heuristic decision engine
    pub fn tap(&self) -> &'static TapEngine {
        crate::tap::sql_bridge::get_global_tap_engine()
    }

    /// Register a custom scalar UDF closure callable directly in SQL queries
    pub fn register_scalar_function<F>(&self, name: &str, func: F)
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        self.executor.write().register_scalar_function(name, func);
    }

    /// Project a relational SQL table as graph nodes into the Knowledge Graph
    pub fn project_table_as_nodes(
        &self,
        table_name: &str,
        id_col: &str,
        label_col: Option<&str>,
        prop_cols: Option<&[&str]>,
    ) -> Result<usize> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor.project_table_as_nodes(&mut pager, table_name, id_col, label_col, prop_cols)
    }

    /// Project a relational SQL table as graph edges into the Knowledge Graph
    pub fn project_table_as_edges(
        &self,
        table_name: &str,
        from_col: &str,
        to_col: &str,
        label_col: Option<&str>,
        weight_col: Option<&str>,
        prop_cols: Option<&[&str]>,
    ) -> Result<usize> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor.project_table_as_edges(
            &mut pager, table_name, from_col, to_col, label_col, weight_col, prop_cols,
        )
    }

    /// Export a WAL replication chunk containing committed/cached frames
    pub fn export_replication_chunk(&self) -> Result<WalReplicationChunk> {
        let pager = self.pager.read();
        WalReplicationStream::create_chunk(&pager, u64::from(pager.header().change_counter).max(1))
    }

    /// Apply an incoming WAL replication chunk into this replica database
    pub fn apply_replication_chunk(&self, chunk: &WalReplicationChunk) -> Result<usize> {
        let mut pager = self.pager.write();
        let affected = WalReplicationReceiver::apply_to_pager(&mut pager, chunk)?;
        let mut executor = self.executor.write();
        executor.reload_from_pager(&mut pager)?;
        let mut memory = self.memory.write();
        *memory = MemoryEngine::with_config(memory.configuration().clone())?;
        executor.load_memory_from_disk(&mut pager, &mut memory)?;
        Ok(affected)
    }

    /// Return active WAL sequence number
    pub fn wal_sequence(&self) -> u32 {
        self.pager.read().header().wal_sequence
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
    pub fn graph_find_path(
        &self,
        start_id: u64,
        end_id: u64,
        max_depth: usize,
    ) -> Option<Vec<Edge>> {
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
        executor.graph_mut().add_node_with_vector(
            id,
            label,
            properties,
            vector.map(|v| v.to_vec()),
        )?;
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
    pub fn chain_from_vector(
        &self,
        query_vec: &[f32],
        top_k: usize,
    ) -> Result<ConnectionChain<'_>> {
        let executor = self.executor.read();
        let graph = executor.graph();
        let mut heap = std::collections::BinaryHeap::new();
        for node in graph.all_nodes() {
            if let Some(ref nv) = node.vector
                && nv.len() == query_vec.len()
            {
                let sim = crate::vector::cosine_similarity(query_vec, nv);
                heap.push((std::cmp::Reverse((sim * 100000.0) as i64), node.id));
                if heap.len() > top_k {
                    heap.pop();
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

    /// Execute an ACL and Tenant-aware GraphRAG query with multi-tenant isolation and role clearance
    pub fn graph_rag_query_scoped(
        &self,
        query_text: &str,
        query_vec: Option<&[f32]>,
        tenant_id: Option<&str>,
        allowed_roles: Option<&[&str]>,
        limit: usize,
    ) -> Result<GraphRagContext> {
        let mut config = GraphRagConfig::default().with_limit(limit);
        if let Some(t) = tenant_id {
            config = config.with_tenant(t);
        }
        if let Some(roles) = allowed_roles {
            config = config.with_roles(roles.iter().map(|&s| s.to_string()));
        }
        self.graph_rag_query(query_text, query_vec, &config)
    }

    // --- High-Performance Document DB API (MongoDB-like) ---

    /// Open or create a schema-less JSON Document Collection (analogous to MongoDB collection)
    pub fn collection(&self, name: &str) -> Result<Collection<'_>> {
        Collection::open(name, self)
    }

    // --- Native AI Agent Memory APIs ---

    fn mutate_memory<T>(
        &self,
        operation: impl FnOnce(&mut MemoryEngine, &mut SQLExecutor, &mut Pager) -> Result<T>,
    ) -> Result<T> {
        // Match the SQL lock order, and expose the memory mutation only after
        // its entire B-tree write has committed or joined the caller's transaction.
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        let mut memory = self.memory.write();
        let result = executor.atomic_statement(&mut pager, |executor, pager| {
            operation(&mut memory, executor, pager)
        });
        if result.is_err() {
            let mut restored = MemoryEngine::with_config(memory.configuration().clone())?;
            executor.load_memory_from_disk(&mut pager, &mut restored)?;
            *memory = restored;
        }
        result
    }

    fn restore_memory_from_pager(&self, pager: &mut Pager, executor: &SQLExecutor) -> Result<()> {
        let mut restored = MemoryEngine::with_config(self.memory.read().configuration().clone())?;
        executor.load_memory_from_disk(pager, &mut restored)?;
        *self.memory.write() = restored;
        Ok(())
    }

    fn validate_memory_input(vector: Option<&[f32]>, importance: f32) -> Result<()> {
        if !importance.is_finite() {
            return Err(Error::ConstraintViolation(
                "Memory importance must be finite".into(),
            ));
        }
        if vector.is_some_and(|v| v.is_empty() || v.iter().any(|x| !x.is_finite())) {
            return Err(Error::ConstraintViolation(
                "Memory embedding must contain finite components and be nonempty".into(),
            ));
        }
        Ok(())
    }

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
        let now_ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
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
        Self::validate_memory_input(vector, importance)?;
        let now_ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let doc = MemoryDocument {
            content: content.into(),
            embedding: vector.map(<[f32]>::to_vec),
            importance,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            namespace: namespace.map(str::to_owned),
            session_id: session_id.map(str::to_owned),
            ..Default::default()
        };
        let effective_doc = if let Some(scope) = self.memory_scope.as_ref() {
            scope.enforce_document(&doc)?
        } else {
            doc
        };
        self.mutate_memory(|mem, executor, pager| {
            let id = mem.write_document(&effective_doc, false, now_ts)?;
            let entry = mem.get_memory(id).expect("newly inserted memory");
            executor.persist_memory_entry(pager, entry)?;
            Ok(id)
        })
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
        Self::validate_memory_input(vector, importance)?;
        let doc = MemoryDocument {
            content: content.into(),
            embedding: vector.map(<[f32]>::to_vec),
            importance,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            ..Default::default()
        };
        let effective_doc = if let Some(scope) = self.memory_scope.as_ref() {
            scope.enforce_document(&doc)?
        } else {
            doc
        };
        self.mutate_memory(|mem, executor, pager| {
            let id = mem.write_document(&effective_doc, false, timestamp)?;
            let entry = mem.get_memory(id).expect("newly inserted memory");
            executor.persist_memory_entry(pager, entry)?;
            Ok(id)
        })
    }

    /// Recall relevant memories using hybrid Vector + BM25 + Temporal Decay scoring.
    pub fn memory_recall(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[f32]>,
        limit: usize,
        filter: &crate::memory::MemoryRecallFilter,
    ) -> Vec<crate::memory::MemoryRecallResult> {
        let now_ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
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
        let now_ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut mem = self.memory.write();
        mem.recall_rrf_scoped(
            query_text,
            query_vector,
            limit,
            rrf_k,
            now_ts,
            self.memory_scope.as_ref(),
        )
    }

    /// Turnkey Zero-Setup AI Agent Memory: remember plain text with automatic local embedding.
    ///
    /// Uses the embedded `DeterministicHashEmbedder` to generate deterministic 128D
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
        self.memory_remember_scoped(
            content,
            Some(&vector),
            importance,
            tags,
            namespace,
            session_id,
        )
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
        let filter = crate::memory::MemoryRecallFilter {
            namespace: namespace.map(str::to_string),
            session_id: session_id.map(str::to_string),
            ..Default::default()
        };
        self.memory_recall(Some(query), Some(&vector), limit, &filter)
    }

    /// Convenient 1-line AI Agent Memory: remember plain text with default importance.
    pub fn remember(&self, content: &str) -> Result<u64> {
        self.memory_remember_text(content, 0.5, &[])
    }

    /// Convenient 1-line AI Agent Memory: recall top-K relevant memories for a plain text query.
    pub fn recall(&self, query: &str, limit: usize) -> Vec<crate::memory::MemoryRecallResult> {
        self.memory_recall_text(query, limit)
    }

    /// Synthesize recalled memories into a prompt-ready context string formatted for LLM prompts.
    pub fn recall_prompt(&self, query: &str, limit: usize) -> String {
        let results = self.memory_recall_text(query, limit);
        if results.is_empty() {
            return String::new();
        }
        let mut prompt = String::from("### Recalled Agent Memory Context:\n");
        for (i, r) in results.iter().enumerate() {
            prompt.push_str(&format!(
                "{}. [Score: {:.2}] {}\n",
                i + 1,
                r.combined_score,
                r.entry.content
            ));
        }
        prompt
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
        let scoped_filter;
        let effective_filter = if let Some(scope) = self.memory_scope.as_ref() {
            match scope.enforce_recall_filter(filter) {
                Ok(enforced) => {
                    scoped_filter = enforced;
                    &scoped_filter
                }
                Err(_) => return Vec::new(),
            }
        } else {
            filter
        };
        let mut mem = self.memory.write();
        mem.recall(
            query_text,
            query_vector,
            limit,
            effective_filter,
            eval_timestamp,
        )
    }

    /// Link two memories together as an associative connection.
    pub fn memory_link(&self, id_a: u64, id_b: u64) -> Result<()> {
        let scope = self.memory_scope.as_ref();
        self.mutate_memory(|mem, executor, pager| {
            if let Some(scope) = scope {
                for id in [id_a, id_b] {
                    if let Some(entry) = mem.get_memory(id)
                        && !scope.matches_entry(entry)
                    {
                        return Err(Error::PermissionDenied(format!(
                            "Memory #{id} is outside scoped owner boundary"
                        )));
                    }
                }
            }
            mem.associate(id_a, id_b)?;
            for id in [id_a, id_b] {
                let entry = mem.get_memory(id).expect("validated associated memory");
                executor.persist_memory_entry(pager, entry)?;
            }
            Ok(())
        })
    }

    /// Retrieve a single memory by ID.
    pub fn memory_get(&self, id: u64) -> Option<crate::memory::MemoryEntry> {
        let mem = self.memory.read();
        mem.get_memory(id)
            .filter(|entry| {
                self.memory_scope
                    .as_ref()
                    .is_none_or(|scope| scope.matches_entry(entry))
            })
            .cloned()
    }

    /// Total count of memories currently held in the memory engine.
    pub fn memory_count(&self) -> usize {
        let mem = self.memory.read();
        if let Some(scope) = self.memory_scope.as_ref() {
            mem.all_entries()
                .values()
                .filter(|entry| scope.matches_entry(entry))
                .count()
        } else {
            mem.memory_count()
        }
    }

    /// Prune old or decayed memories below the given retention threshold using system time.
    pub fn memory_prune(&self, decay_threshold: f32, half_life_seconds: u64) -> usize {
        let now_ts = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
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
        self.memory_prune_at_checked(decay_threshold, half_life_seconds, eval_timestamp)
            .unwrap_or(0)
    }

    /// Atomically prune memories and propagate storage errors to the caller.
    pub fn memory_prune_at_checked(
        &self,
        decay_threshold: f32,
        half_life_seconds: u64,
        eval_timestamp: u64,
    ) -> Result<usize> {
        if !decay_threshold.is_finite() {
            return Err(Error::ConstraintViolation(
                "Retention threshold must be finite".into(),
            ));
        }
        let scope = self.memory_scope.as_ref();
        self.mutate_memory(|mem, executor, pager| {
            let removed_ids = mem.prune_decayed_ids_at_scoped(
                decay_threshold,
                half_life_seconds,
                eval_timestamp,
                scope,
            );
            for &id in &removed_ids {
                executor.delete_memory_entry(pager, id)?;
            }
            Ok(removed_ids.len())
        })
    }

    /// Explicitly delete and forget a memory by ID.
    pub fn memory_forget(&self, id: u64) -> bool {
        self.memory_forget_checked(id).unwrap_or(false)
    }

    /// Atomically delete a memory and propagate storage errors to the caller.
    pub fn memory_forget_checked(&self, id: u64) -> Result<bool> {
        let scope = self.memory_scope.as_ref();
        self.mutate_memory(|mem, executor, pager| {
            if let Some(scope) = scope
                && let Some(entry) = mem.get_memory(id)
                && !scope.matches_entry(entry)
            {
                return Err(Error::PermissionDenied(format!(
                    "Memory #{id} is outside scoped owner boundary"
                )));
            }
            let (removed, updated) = mem.remove_ids(&[id]);
            for id in &removed {
                executor.delete_memory_entry(pager, *id)?;
            }
            for id in updated {
                executor
                    .persist_memory_entry(pager, mem.get_memory(id).expect("associated source"))?;
            }
            Ok(!removed.is_empty())
        })
    }

    // --- Write-Ahead Log (WAL) & Durability ---

    /// Manually trigger a WAL checkpoint, flushing all committed pages
    /// from `.tapir-wal` to the main database file and resetting the log.
    pub fn checkpoint(&self) -> Result<usize> {
        self.persist_vector_snapshots()?;
        let mut pager = self.pager.write();
        pager.checkpoint()
    }

    /// Observe deferred maintenance errors and whether storage must be reopened.
    pub fn storage_health(&self) -> StorageHealth {
        self.pager.read().storage_health()
    }

    /// Install/clear a deterministic I/O fault for this connection only.
    #[cfg(feature = "fault-injection")]
    pub fn set_io_fault(&self, plan: Option<pager::fault::IoFaultPlan>) -> Result<()> {
        self.pager.read().set_io_fault(plan)
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

    /// Validate an entire native backup before atomically installing it.
    /// Overwrite never bypasses an active target lock or an uncheckpointed WAL.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn restore_backup(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        config: Config,
        overwrite: bool,
    ) -> Result<IntegrityReport> {
        if !source.as_ref().is_file() {
            return Err(Error::Corrupted("Backup source is not a file".into()));
        }
        let connection = Self::open_with_config(source, config)?;
        let report = connection.check_integrity()?;
        if !report.is_ok() {
            return Err(Error::Corrupted(format!(
                "Backup failed integrity validation: {:?}",
                report.errors
            )));
        }
        connection.close()?;
        connection
            .pager
            .write()
            .backup_to_options(destination.as_ref(), overwrite)?;
        Ok(report)
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

    /// Export the entire database as a contiguous raw byte vector.
    /// Perfect for WebAssembly browser export, OPFS sync, and network replication.
    pub fn export_bytes(&self) -> Result<Vec<u8>> {
        let mut pager = self.pager.write();
        pager.export_bytes()
    }

    /// Quantize a high-dimensional vector into an ultra-compact 1-bit or 2-bit representation using RaBitQ.
    ///
    /// Applies deterministic Fast Walsh-Hadamard Transform (FWHT) orthogonal rotation and bit quantization,
    /// reducing 1536-D embeddings from 6,144 bytes to ~196 bytes for battery-powered edge silicon.
    pub fn rabitq_quantize(
        &self,
        vector: &[f32],
        num_bits: usize,
    ) -> crate::vector::RaBitQuantizedVector {
        let quantizer = crate::vector::RaBitQuantizer::new(vector.len(), num_bits);
        quantizer.quantize(vector)
    }

    /// Evaluate hardware-accelerated single-cycle POPCNT Hamming Distance between two RaBitQ vectors.
    pub fn rabitq_distance(
        &self,
        a: &crate::vector::RaBitQuantizedVector,
        b: &crate::vector::RaBitQuantizedVector,
    ) -> u32 {
        a.hamming_distance(b)
    }

    /// Estimate approximate Cosine Similarity between a RaBitQ quantized vector and a raw query vector.
    pub fn rabitq_similarity(
        &self,
        quantized: &crate::vector::RaBitQuantizedVector,
        query: &[f32],
    ) -> f32 {
        let quantizer = crate::vector::RaBitQuantizer::new(query.len(), quantized.num_bits());
        let rotated = quantizer.rotate(query);
        let norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        1.0 - quantized.asymmetric_cosine_distance(&rotated, norm)
    }

    /// Perform a comprehensive cryptographic, page-level, and structural schema audit of the database.
    pub fn check_integrity(&self) -> Result<IntegrityReport> {
        let mut pager = self.pager.write();
        let _ = pager.checkpoint();
        let _ = pager.sync_header();
        let total_pages = pager.total_pages();
        let page_size = pager.page_size();
        let version = pager.header().version;
        let is_encrypted = pager.header().encryption_flags == 1;
        let is_compressed = pager.is_compressed();
        let wal_sequence = pager.header().wal_sequence;
        let stored_crc = pager.header().header_crc32;
        let calculated_crc = pager.header().calculate_crc();
        let header_crc_valid = stored_crc == calculated_crc;

        let mut errors = Vec::new();
        if !header_crc_valid {
            errors.push(format!("Header CRC32 mismatch: stored 0x{stored_crc:08X}, calculated 0x{calculated_crc:08X}"));
        }

        let mut pages_verified = 0;
        for pid in 1..=total_pages {
            match pager.read_page(pid) {
                Ok(page) => {
                    if page.len() != page_size {
                        errors.push(format!(
                            "Page {pid} corrupted: length {} != {page_size}",
                            page.len()
                        ));
                    } else {
                        pages_verified += 1;
                    }
                }
                Err(e) => {
                    errors.push(format!("Page {pid} read/decryption error: {e}"));
                }
            }
        }
        drop(pager);

        let executor = self.executor.read();
        let tables_count = executor.tables().len();
        let (graph_nodes_count, graph_edges_count) =
            (executor.graph().node_count(), executor.graph().edge_count());
        let collections_count = self.collections().len();

        Ok(IntegrityReport {
            version,
            page_size,
            total_pages,
            pages_verified,
            header_crc_valid,
            stored_crc,
            calculated_crc,
            is_encrypted,
            is_compressed,
            wal_sequence,
            tables_count,
            collections_count,
            graph_nodes_count,
            graph_edges_count,
            errors,
        })
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

    /// Persist all active vector index topologies and AI memory HNSW caches to disk snapshots
    pub fn persist_vector_snapshots(&self) -> Result<()> {
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        executor.persist_vector_indexes_to_disk(&mut pager)?;
        let mut memory = self.memory.write();
        executor.persist_memory_index_to_disk(&mut pager, &mut memory)?;
        Ok(())
    }

    /// Check if currently in an active transaction
    pub fn is_in_transaction(&self) -> bool {
        let pager = self.pager.read();
        pager.is_in_transaction()
    }

    /// Set the busy timeout used when coordinating writes in concurrent access mode.
    pub fn set_busy_timeout(&self, timeout: std::time::Duration) {
        self.pager.write().set_busy_timeout(timeout);
    }

    /// Return whether this connection was opened in read-only mode.
    pub fn is_read_only(&self) -> bool {
        self.pager.read().is_read_only()
    }

    pub(crate) fn write_generation(&self) -> u64 {
        self.pager.read().write_generation()
    }

    /// Return the row ID of the most recent successful `INSERT` on this connection.
    pub fn last_insert_rowid(&self) -> i64 {
        self.executor.read().last_insert_rowid()
    }

    /// Refresh read-only or idle concurrent connection state from committed on-disk frames.
    ///
    /// Returns `Ok(true)` if newer committed writes from another process were loaded.
    pub fn refresh_from_disk(&self) -> Result<bool> {
        let mut pager = self.pager.write();
        if pager.refresh_committed_state()? {
            let mut executor = self.executor.write();
            executor.reload_from_pager(&mut pager)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Capture an isolated read-only committed snapshot of pager + SQL executor.
    pub(crate) fn capture_committed_snapshot(&self) -> Result<(Pager, SQLExecutor)> {
        let mut pager = self.pager.write();
        let executor = self.executor.read();
        let snap_pager = pager.snapshot_clone()?;
        let snap_executor = executor.clone();
        Ok((snap_pager, snap_executor))
    }

    /// Construct an isolated read-only snapshot connection from captured committed parts.
    pub(crate) fn from_snapshot_parts(
        pager: Pager,
        executor: SQLExecutor,
        config: Config,
    ) -> Result<Self> {
        let memory = MemoryEngine::with_config(config.memory.clone())?;
        Ok(Self {
            pager: Arc::new(RwLock::new(pager)),
            executor: Arc::new(RwLock::new(executor)),
            memory: Arc::new(RwLock::new(memory)),
            realtime: Arc::new(RealtimeBus::new()),
            pending_changes: Arc::new(parking_lot::Mutex::new(Vec::new())),
            config,
            memory_scope: None,
        })
    }

    fn execute_parsed(&self, stmt: Statement) -> Result<usize> {
        let cdc_info = match &stmt {
            Statement::Insert { table, .. } | Statement::InsertMany { table, .. } => {
                Some((ChangeOp::Insert, table.clone()))
            }
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };
        let is_commit = matches!(&stmt, Statement::CommitTransaction);
        let is_rollback = matches!(&stmt, Statement::RollbackTransaction);
        let is_pragma = matches!(&stmt, Statement::Pragma { .. });
        let is_tx_end = matches!(
            &stmt,
            Statement::CommitTransaction
                | Statement::RollbackTransaction
                | Statement::Savepoint { .. }
                | Statement::RollbackToSavepoint { .. }
                | Statement::ReleaseSavepoint { .. }
        );

        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        if !is_pragma && !is_tx_end && pager.begin_concurrent_write()? {
            executor.reload_from_pager(&mut pager)?;
        }
        let change_row_id = predicted_insert_row_id(&executor, &stmt).unwrap_or(0);
        let rollback_to = if let Statement::RollbackToSavepoint { name } = &stmt {
            pager.savepoint_position(name)
        } else {
            None
        };

        let affected_res = executor.execute(&mut pager, stmt);
        let affected = match affected_res {
            Ok(n) => {
                if (is_rollback || rollback_to.is_some())
                    && let Err(e) = self.restore_memory_from_pager(&mut pager, &executor)
                {
                    let _ = pager.end_concurrent_write();
                    return Err(e);
                }
                n
            }
            Err(e) => {
                let _ = pager.end_concurrent_write();
                return Err(e);
            }
        };

        let in_transaction = pager.is_in_transaction();
        let savepoint_depth = pager.savepoint_depth();
        pager.end_concurrent_write()?;
        drop(executor);
        drop(pager);

        if let Some(position) = rollback_to {
            self.discard_changes_after_savepoint(position);
        }
        if is_rollback {
            self.finish_pending_changes(false);
        }
        if let Some((op, table)) = cdc_info {
            let now_ts = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.queue_change(
                ChangeEvent {
                    op,
                    table,
                    row_id: change_row_id,
                    timestamp: now_ts,
                    data: serde_json::json!({"affected_rows": affected}),
                },
                in_transaction,
                savepoint_depth,
            );
        }
        if is_commit {
            self.finish_pending_changes(true);
        }
        Ok(affected)
    }

    fn query_parsed(&self, stmt: Statement) -> Result<Vec<Row>> {
        if matches!(
            &stmt,
            Statement::BeginTransaction
                | Statement::CommitTransaction
                | Statement::RollbackTransaction
                | Statement::Savepoint { .. }
                | Statement::RollbackToSavepoint { .. }
                | Statement::ReleaseSavepoint { .. }
        ) {
            self.execute_parsed(stmt)?;
            return Ok(Vec::new());
        }

        let cdc_info = match &stmt {
            Statement::Insert { table, .. } | Statement::InsertMany { table, .. } => {
                Some((ChangeOp::Insert, table.clone()))
            }
            Statement::Update { table, .. } => Some((ChangeOp::Update, table.clone())),
            Statement::Delete { table, .. } => Some((ChangeOp::Delete, table.clone())),
            _ => None,
        };

        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        if cdc_info.is_some() {
            if pager.begin_concurrent_write()? {
                executor.reload_from_pager(&mut pager)?;
            }
        } else if pager.refresh_committed_state()? {
            executor.reload_from_pager(&mut pager)?;
        }
        let change_row_id = predicted_insert_row_id(&executor, &stmt).unwrap_or(0);

        let rows_res = executor.query(&mut pager, stmt);
        let rows = match rows_res {
            Ok(r) => r,
            Err(e) => {
                if cdc_info.is_some() {
                    let _ = pager.end_concurrent_write();
                }
                return Err(e);
            }
        };
        let in_transaction = pager.is_in_transaction();
        let savepoint_depth = pager.savepoint_depth();
        if cdc_info.is_some() {
            pager.end_concurrent_write()?;
        }
        drop(executor);
        drop(pager);

        if let Some((op, table)) = cdc_info {
            let now_ts = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.queue_change(
                ChangeEvent {
                    op,
                    table,
                    row_id: change_row_id,
                    timestamp: now_ts,
                    data: serde_json::json!({"affected_rows": rows.len()}),
                },
                in_transaction,
                savepoint_depth,
            );
        }
        Ok(rows)
    }

    /// Begin a transaction
    pub fn begin_transaction(&self) -> Result<()> {
        let mut pager = self.pager.write();
        if pager.begin_concurrent_write()? {
            let mut executor = self.executor.write();
            executor.reload_from_pager(&mut pager)?;
        }
        if let Err(e) = pager.begin_transaction() {
            let _ = pager.end_concurrent_write();
            return Err(e);
        }
        Ok(())
    }

    /// Commit the active transaction
    pub fn commit(&self) -> Result<()> {
        self.persist_vector_snapshots()?;
        let mut pager = self.pager.write();
        let res = pager.commit_transaction();
        let _ = pager.end_concurrent_write();
        res?;
        drop(pager);
        self.finish_pending_changes(true);
        Ok(())
    }

    /// Rollback the active transaction
    pub fn rollback(&self) -> Result<()> {
        let mut pager = self.pager.write();
        let res = pager.rollback_transaction();
        if res.is_ok() {
            let mut executor = self.executor.write();
            *executor = SQLExecutor::new(&mut pager)?;
            executor.load_graph_from_disk(&mut pager)?;
            let mut memory = self.memory.write();
            *memory = MemoryEngine::with_config(memory.configuration().clone())?;
            executor.load_memory_from_disk(&mut pager, &mut memory)?;
            executor.load_vector_indexes_from_disk(&mut pager)?;
        }
        let _ = pager.end_concurrent_write();
        res?;
        drop(pager);
        self.finish_pending_changes(false);
        Ok(())
    }

    fn queue_change(&self, event: ChangeEvent, in_transaction: bool, savepoint_depth: usize) {
        if in_transaction {
            self.pending_changes.lock().push((savepoint_depth, event));
            return;
        }
        let mut ready = {
            let mut pending = self.pending_changes.lock();
            let mut ready: Vec<_> = pending.drain(..).map(|(_, event)| event).collect();
            ready.push(event);
            ready
        };
        for event in ready.drain(..) {
            self.realtime.publish(&event);
        }
    }

    fn finish_pending_changes(&self, committed: bool) {
        let ready: Vec<_> = self
            .pending_changes
            .lock()
            .drain(..)
            .map(|(_, event)| event)
            .collect();
        if committed {
            for event in ready {
                self.realtime.publish(&event);
            }
        }
    }

    fn discard_changes_after_savepoint(&self, savepoint_position: usize) {
        self.pending_changes
            .lock()
            .retain(|(depth, _)| *depth <= savepoint_position);
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
    pub fn graph_pagerank(
        &self,
        damping_factor: f32,
        max_iterations: usize,
        tolerance: f32,
    ) -> std::collections::HashMap<u64, f32> {
        let executor = self.executor.read();
        executor
            .graph()
            .pagerank(damping_factor, max_iterations, tolerance)
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
        let plan = Self::try_compile_insert_plan(&tokens);
        let preparsed = if plan.is_none()
            && !tokens
                .iter()
                .any(|t| matches!(t, crate::sql::lexer::Token::QuestionMark))
        {
            crate::sql::parser::parse_tokens(&tokens).ok()
        } else {
            None
        };
        Ok(PreparedStatement {
            conn: self,
            tokens,
            plan,
            preparsed,
            encode_buf: std::cell::RefCell::new(Vec::with_capacity(256)),
        })
    }

    fn try_compile_insert_plan(tokens: &[crate::sql::lexer::Token]) -> Option<PreparedPlan> {
        use crate::sql::lexer::Token;
        let mut i = 0;
        if i < tokens.len() && tokens[i] == Token::Insert {
            i += 1;
        } else {
            return None;
        }
        if i < tokens.len() && tokens[i] == Token::Into {
            i += 1;
        } else {
            return None;
        }
        let table = match tokens.get(i) {
            Some(Token::Ident(name)) => {
                i += 1;
                name.clone()
            }
            _ => return None,
        };

        let mut columns = None;
        if i < tokens.len() && tokens[i] == Token::OpenParen {
            i += 1;
            let mut cols = Vec::new();
            while i < tokens.len() {
                match &tokens[i] {
                    Token::Ident(c) => cols.push(c.clone()),
                    _ => return None,
                }
                i += 1;
                if i < tokens.len() && tokens[i] == Token::Comma {
                    i += 1;
                    continue;
                } else if i < tokens.len() && tokens[i] == Token::CloseParen {
                    i += 1;
                    break;
                } else {
                    return None;
                }
            }
            columns = Some(cols);
        }

        if i < tokens.len() && tokens[i] == Token::Values {
            i += 1;
        } else {
            return None;
        }

        if i < tokens.len() && tokens[i] == Token::OpenParen {
            i += 1;
        } else {
            return None;
        }

        let mut param_count = 0;
        while i < tokens.len() {
            if tokens[i] == Token::QuestionMark {
                param_count += 1;
                i += 1;
                // handle optional ?1, ?2, etc.
                if i < tokens.len() && matches!(tokens[i], Token::IntLit(_)) {
                    i += 1;
                }
                if i < tokens.len() && tokens[i] == Token::Comma {
                    i += 1;
                    continue;
                } else if i < tokens.len() && tokens[i] == Token::CloseParen {
                    i += 1;
                    break;
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }

        if i < tokens.len() && tokens[i] == Token::Semicolon {
            i += 1;
        }

        if i == tokens.len() {
            Some(PreparedPlan::Insert {
                table,
                columns,
                param_count,
            })
        } else {
            None
        }
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

    /// High-throughput direct batch insert bypassing SQL parsing and per-row lock overhead.
    pub fn insert_rows_batch(
        &self,
        table: &str,
        columns: Option<&[String]>,
        rows: &[Vec<Value>],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut pager = self.pager.write();
        let mut executor = self.executor.write();
        if pager.begin_concurrent_write()? {
            executor.reload_from_pager(&mut pager)?;
        }
        let count_res = executor.execute_insert_batch(&mut pager, table, columns, rows);
        let in_tx = pager.is_in_transaction();
        let savepoint_depth = pager.savepoint_depth();
        pager.end_concurrent_write()?;
        drop(executor);
        drop(pager);

        let count = count_res?;
        if count > 0 {
            let now_ts = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.queue_change(
                ChangeEvent {
                    op: ChangeOp::Insert,
                    table: table.to_string(),
                    row_id: 0,
                    timestamp: now_ts,
                    data: serde_json::json!({ "affected_rows": count }),
                },
                in_tx,
                savepoint_depth,
            );
        }
        Ok(count)
    }

    /// Stream and ingest any supported dataset (CSV, TSV, JSONL, Parquet, SQLite, SQL dump, PDF, or Text)
    /// into this TapirusDB connection.
    #[cfg(feature = "std")]
    pub fn ingest_lakehouse(
        &self,
        source_pattern: &str,
        explicit_format: Option<&str>,
        options: &lakehouse::LakeIngestOptions,
    ) -> Result<lakehouse::IngestStats> {
        lakehouse::ingest_into_connection(self, source_pattern, explicit_format, options)
    }

    /// Export a SQL query result directly to Parquet, CSV, TSV, JSONL, or a universal SQL dump.
    #[cfg(feature = "std")]
    pub fn export_query_to_file(
        &self,
        sql: &str,
        dest: impl AsRef<Path>,
        explicit_format: Option<&str>,
        partition_by: Option<&str>,
    ) -> Result<lakehouse::ExportStats> {
        let rows = self.query(sql)?;
        lakehouse::export_rows_to_path(
            &rows,
            "query_export",
            dest.as_ref(),
            explicit_format,
            partition_by,
        )
    }

    /// Explicitly close the connection, persisting any dirty vector snapshots and checkpointing the WAL.
    pub fn close(&self) -> Result<()> {
        if self.is_in_transaction() {
            self.rollback()?;
        }
        self.persist_vector_snapshots()?;
        let mut pager = self.pager.write();
        pager.checkpoint()?;
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if Arc::strong_count(&self.pager) == 1 && !self.is_in_transaction() {
            let _ = self.persist_vector_snapshots();
        }
    }
}

#[derive(Debug, Clone)]
enum PreparedPlan {
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        param_count: usize,
    },
}

/// A prepared SQL statement with pre-tokenized query and cached token stream for parameterized execution
pub struct PreparedStatement<'a> {
    conn: &'a Connection,
    tokens: Vec<crate::sql::lexer::Token>,
    plan: Option<PreparedPlan>,
    preparsed: Option<Statement>,
    /// Reusable encode buffer for insert fast-path â€” avoids per-row Vec allocations
    encode_buf: std::cell::RefCell<Vec<u8>>,
}

impl<'a> PreparedStatement<'a> {
    /// Execute prepared non-query SQL command with parameter binding
    pub fn execute(&self, params: &[Value]) -> Result<usize> {
        if let Some(PreparedPlan::Insert {
            table,
            columns,
            param_count,
        }) = &self.plan
        {
            if params.len() != *param_count {
                return Err(Error::SqlSyntax(format!(
                    "Parameter count mismatch: expected {}, got {}",
                    param_count,
                    params.len()
                )));
            }
            let mut pager = self.conn.pager.write();
            let mut executor = self.conn.executor.write();
            if pager.begin_concurrent_write()? {
                executor.reload_from_pager(&mut pager)?;
            }
            let row_id =
                predicted_insert_row_id_for_values(&executor, table, columns.as_deref(), params)
                    .unwrap_or(0);
            let mut buf = self.encode_buf.borrow_mut();
            let exec_res = executor.execute_insert_fast(
                &mut pager,
                table,
                columns.as_deref(),
                params,
                Some(&mut buf),
            );
            drop(buf);
            let affected = match exec_res {
                Ok((affected, _)) => affected,
                Err(e) => {
                    let _ = pager.end_concurrent_write();
                    return Err(e);
                }
            };
            let in_transaction = pager.is_in_transaction();
            let savepoint_depth = pager.savepoint_depth();
            pager.end_concurrent_write()?;
            drop(executor);
            drop(pager);
            let now_ts = web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.conn.queue_change(
                ChangeEvent {
                    op: ChangeOp::Insert,
                    table: table.clone(),
                    row_id,
                    timestamp: now_ts,
                    data: serde_json::json!({"affected_rows": affected}),
                },
                in_transaction,
                savepoint_depth,
            );

            return Ok(affected);
        }

        let stmt = if let Some(p) = &self.preparsed {
            p.clone()
        } else {
            if self.tokens.is_empty() {
                return Ok(0);
            }
            let bound = crate::sql::parser::bind_parameters(&self.tokens, params)?;
            crate::sql::parser::parse_tokens(&bound)?
        };
        self.conn.execute_parsed(stmt)
    }

    /// Execute prepared SQL query returning rows with parameter binding
    pub fn query(&self, params: &[Value]) -> Result<Vec<Row>> {
        let stmt = if let Some(p) = &self.preparsed {
            p.clone()
        } else {
            if self.tokens.is_empty() {
                return Ok(Vec::new());
            }
            let bound = crate::sql::parser::bind_parameters(&self.tokens, params)?;
            crate::sql::parser::parse_tokens(&bound)?
        };
        self.conn.query_parsed(stmt)
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
            let mut pager = self.pager.write();
            let mut executor = self.executor.write();
            let rows = executor.atomic_statement(&mut pager, |executor, pager| {
                let before = executor.graph().clone();
                let rows = crate::graph::cypher::CypherExecutor::execute_mutation(
                    executor.graph_mut(),
                    trimmed,
                )?;
                let nodes: Vec<_> = executor
                    .graph()
                    .all_nodes()
                    .into_iter()
                    .filter(|node| before.get_node(node.id) != Some(*node))
                    .cloned()
                    .collect();
                let old_edges: std::collections::HashSet<_> =
                    before.all_edges().iter().map(|edge| edge.id).collect();
                let edges: Vec<_> = executor
                    .graph()
                    .all_edges()
                    .into_iter()
                    .filter(|edge| !old_edges.contains(&edge.id))
                    .cloned()
                    .collect();
                for node in nodes {
                    executor.persist_graph_node(pager, &node)?;
                }
                for edge in edges {
                    executor.persist_graph_edge(pager, &edge)?;
                }
                Ok(rows)
            })?;
            let affected: usize = rows
                .first()
                .and_then(|r| r.get::<i64>("nodes_created").ok())
                .map(|i| i as usize)
                .unwrap_or(0);
            return Ok(affected);
        }
        let stmt = {
            let mut executor = self.executor.write();
            match executor.get_or_parse_statement(trimmed)? {
                Some(s) => s,
                None => return Ok(0),
            }
        };
        self.execute_parsed(stmt)
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
        let stmt = {
            let mut executor = self.executor.write();
            match executor.get_or_parse_statement(trimmed)? {
                Some(s) => s,
                None => return Ok(Vec::new()),
            }
        };
        self.query_parsed(stmt)
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
    if let Some(header) = Pager::read_header_for_open(path)? {
        return Ok(header.salt);
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
        self.node_filters
            .push(crate::graph::NodeFilter::Label(label.into()));
        self
    }

    /// Filter candidate nodes by property presence
    pub fn filter_property<S1: Into<String>, S2: Into<String>>(mut self, key: S1, val: S2) -> Self {
        self.node_filters
            .push(crate::graph::NodeFilter::PropertyContains {
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
        let resolver = |id: u64| mem.get_memory(id).and_then(|e| e.vector.clone());

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
            .vector_near(query_vec, top_k, metric)
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
        let resolver = |id: u64| mem.get_memory(id).and_then(|e| e.vector.clone());

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
        conn.graph_add_node(1, "Person", r#"{"name":"Alex"}"#)
            .unwrap();
        conn.graph_add_node(2, "Project", r#"{"name":"TapirusDB"}"#)
            .unwrap();
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

    #[test]
    fn test_turnkey_tap_decision_engine() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let verdict = conn
            .tap()
            .classify(
                "Urgent billing payment failed on checkout",
                &["billing", "tech", "sales"],
            )
            .unwrap();
        assert_eq!(verdict.top_choice, "billing");
        assert!(verdict.confidence > 0.4);

        let verified = conn
            .tap()
            .verify(
                "Client provided active subscription receipt",
                "active subscription receipt valid",
            )
            .unwrap();
        assert!(verified.is_verified);
    }

    #[test]
    fn test_one_line_agent_memory_and_prompt() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let id = conn
            .remember("Alex prefers dark mode and high contrast themes")
            .unwrap();
        assert!(id > 0);

        let results = conn.recall("what theme does Alex prefer", 3);
        assert!(!results.is_empty());
        assert!(results[0].entry.content.contains("dark mode"));

        let prompt = conn.recall_prompt("Alex theme", 2);
        assert!(prompt.contains("Recalled Agent Memory Context"));
        assert!(prompt.contains("dark mode"));
    }

    #[test]
    fn test_bytes_export_and_import_roundtrip() {
        let conn1 = Connection::open_in_memory().expect("Open conn1");
        conn1
            .execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
            .unwrap();
        conn1
            .execute("INSERT INTO users (id, name) VALUES (1, 'Alice');")
            .unwrap();
        conn1
            .execute("INSERT INTO users (id, name) VALUES (2, 'Bob');")
            .unwrap();
        let mem_id = conn1
            .remember("Alice is a cryptographic engineer working on sovereign AI")
            .unwrap();
        assert!(mem_id > 0);

        let bytes = conn1.export_bytes().expect("Export bytes");
        assert!(!bytes.is_empty());
        assert_eq!(bytes.len() % 4096, 0);

        let conn2 = Connection::open_from_bytes(&bytes).expect("Open conn2 from bytes");
        let rows = conn2
            .query("SELECT id, name FROM users ORDER BY id;")
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<String>("name").unwrap(), "Alice");
        assert_eq!(rows[1].get::<String>("name").unwrap(), "Bob");

        let recalled = conn2.recall("Alice profession", 1);
        assert!(!recalled.is_empty());
        assert!(recalled[0].entry.content.contains("cryptographic engineer"));
    }

    #[test]
    fn test_connection_rabitq_quantize_and_similarity() {
        let conn = Connection::open_in_memory().expect("Open in-memory");
        let vec_a: Vec<f32> = (0..128).map(|i| (i as f32 * 0.1).sin()).collect();
        let vec_b: Vec<f32> = (0..128).map(|i| (i as f32 * 0.1).sin()).collect();
        let vec_c: Vec<f32> = (0..128).map(|i| -(i as f32 * 0.1).sin()).collect();

        // 1-Bit Quantization (Theoretical cosine fidelity ~ sqrt(2/pi) = ~0.798)
        let q_a = conn.rabitq_quantize(&vec_a, 1);
        let q_b = conn.rabitq_quantize(&vec_b, 1);
        let q_c = conn.rabitq_quantize(&vec_c, 1);

        assert_eq!(q_a.dimensions(), 128);
        assert_eq!(q_a.num_bits(), 1);

        // Hamming distance to identical vector must be 0!
        let dist_self = conn.rabitq_distance(&q_a, &q_b);
        assert_eq!(dist_self, 0);

        // Hamming distance to inverted vector must be large (~128 bit flips)
        let dist_inv = conn.rabitq_distance(&q_a, &q_c);
        assert!(dist_inv > 100);

        let sim_1bit = conn.rabitq_similarity(&q_a, &vec_a);
        assert!(sim_1bit > 0.70);

        // 2-Bit Quantization (Theoretical fidelity > 0.85)
        let q_2bit = conn.rabitq_quantize(&vec_a, 2);
        assert_eq!(q_2bit.num_bits(), 2);
        let sim_2bit = conn.rabitq_similarity(&q_2bit, &vec_a);
        assert!(sim_2bit > 0.75);
    }

    #[test]
    fn test_config_scrubs_and_redacts_secrets() {
        let mut cfg = Config {
            passphrase: Some("my-top-secret-passphrase".to_string()),
            encryption_key: Some([0x42u8; 32]),
            ..Default::default()
        };
        let dbg_str = format!("{cfg:?}");
        assert!(!dbg_str.contains("my-top-secret-passphrase"));
        assert!(dbg_str.contains("[REDACTED]"));

        cfg.zeroize();
        assert!(cfg.passphrase.is_none());
        assert!(cfg.encryption_key.is_none());

        let conn = Connection::open_encrypted_in_memory_with_key([0x7Au8; 32]).unwrap();
        assert!(conn.is_encrypted());
        assert!(conn.config().passphrase.is_none());
        assert!(conn.config().encryption_key.is_none());
    }

    #[test]
    fn test_encrypted_in_memory_export_and_import_roundtrip() {
        let conn = Connection::open_encrypted_in_memory("browser-opfs-secret").unwrap();
        assert!(conn.is_encrypted());
        conn.execute("CREATE TABLE vault (id INTEGER PRIMARY KEY, secret TEXT);")
            .unwrap();
        conn.execute("INSERT INTO vault VALUES (1, 'super-confidential-token');")
            .unwrap();

        let bytes = conn.export_bytes().unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(bytes.len() % 4096, 0);
        // Raw plaintext must never appear in encrypted exported bytes
        assert!(
            !bytes
                .windows(b"super-confidential-token".len())
                .any(|w| w == b"super-confidential-token")
        );

        // Plaintext import must fail because encryption_flags == 1
        assert!(Connection::open_from_bytes(&bytes).is_err());
        // Wrong passphrase must fail KCV/AEAD
        assert!(Connection::open_encrypted_from_bytes(&bytes, "wrong-pass").is_err());

        // Correct passphrase decrypts cleanly
        let restored =
            Connection::open_encrypted_from_bytes(&bytes, "browser-opfs-secret").unwrap();
        assert!(restored.is_encrypted());
        let rows = restored
            .query("SELECT secret FROM vault WHERE id = 1;")
            .unwrap();
        assert_eq!(
            rows[0].get::<String>("secret").unwrap(),
            "super-confidential-token"
        );
    }
}
