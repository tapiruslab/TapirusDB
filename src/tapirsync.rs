//! # TapirSync (`TPSYNC01`) — Edge-to-Cloud Incremental Replication Protocol
//!
//! Bridges single-file **TapirusDB (`.tapir`)** edge databases (mobile, IoT, desktop, local AI agents)
//! with distributed **TapisaurusDB** cloud clusters.
//!
//! ## Core Capabilities
//! - **Multi-Modal Incremental Delta Extraction**: Tracks mutations across Relational SQL tables,
//!   Document Collections (`__doc_*`), Knowledge Graph vertices/edges, and AI Agent Memories
//!   (including dense embeddings), emitting only changed records and deletion tombstones since
//!   the last synchronization watermark.
//! - **Deterministic HLC + LWW Conflict Resolution**: Every record carries a Hybrid Logical Clock
//!   (`wall_time_ms`, `logical_counter`, `node_id`) ensuring Strong Eventual Consistency (SEC)
//!   when offline edge devices and cloud nodes mutate the same record concurrently.
//! - **Hardware-Accelerated `TPSYNC01` Binary Framing**: Compact binary wire envelope with CRC32
//!   verification over header metadata and canonical record payloads.

use crate::Connection;
use crate::error::{Error, Result};
use crate::memory::EmbeddingEngine;
use crate::sql::catalog::DataType;
use crate::traits::Value;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// 8-byte wire magic header identifying a `TapirSync` v1 binary replication bundle.
pub const TAPIRSYNC_WIRE_MAGIC: &[u8; 8] = b"TPSYNC01";

/// Current `TapirSync` wire protocol version.
pub const TAPIRSYNC_PROTOCOL_VERSION: u16 = 1;

/// Hybrid Logical Clock (HLC) stamp providing monotonic causal ordering and deterministic
/// Last-Write-Wins (LWW) conflict tie-breaking across edge and cloud nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapirSyncHlc {
    /// Physical wall-clock timestamp in milliseconds since Unix epoch.
    pub wall_time_ms: u64,
    /// Monotonic logical counter for sub-millisecond causal events.
    pub logical_counter: u32,
    /// Unique identifier of the originating edge or cloud node.
    pub node_id: String,
}

impl Ord for TapirSyncHlc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.wall_time_ms
            .cmp(&other.wall_time_ms)
            .then_with(|| self.logical_counter.cmp(&other.logical_counter))
            .then_with(|| self.node_id.cmp(&other.node_id))
    }
}

impl PartialOrd for TapirSyncHlc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TapirSyncHlc {
    /// Create a new HLC initialized for `node_id` at `wall_time_ms`.
    pub fn new(node_id: impl Into<String>, wall_time_ms: u64) -> Self {
        Self {
            wall_time_ms,
            logical_counter: 0,
            node_id: node_id.into(),
        }
    }

    /// Advance the clock for a local mutation event.
    pub fn tick(&mut self, now_ms: u64) -> Self {
        if now_ms > self.wall_time_ms {
            self.wall_time_ms = now_ms;
            self.logical_counter = 0;
        } else {
            self.logical_counter = self.logical_counter.saturating_add(1);
        }
        self.clone()
    }

    /// Merge with a remote HLC timestamp upon receiving a replication bundle.
    pub fn observe(&mut self, remote: &TapirSyncHlc, now_ms: u64) -> Self {
        let max_wall = self.wall_time_ms.max(remote.wall_time_ms).max(now_ms);
        if max_wall == self.wall_time_ms && max_wall == remote.wall_time_ms {
            self.logical_counter = self
                .logical_counter
                .max(remote.logical_counter)
                .saturating_add(1);
        } else if max_wall == self.wall_time_ms {
            self.logical_counter = self.logical_counter.saturating_add(1);
        } else if max_wall == remote.wall_time_ms {
            self.wall_time_ms = max_wall;
            self.logical_counter = remote.logical_counter.saturating_add(1);
        } else {
            self.wall_time_ms = max_wall;
            self.logical_counter = 0;
        }
        self.clone()
    }
}

/// A single multi-modal logical replication record inside a `TapirSyncBundle`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TapirSyncRecord {
    /// Relational SQL table row insert or update.
    TableRowUpsert {
        /// Target SQL table name.
        table: String,
        /// Canonical primary-key or row identifier string.
        row_id: String,
        /// Column-to-JSON value map representing the row.
        columns: BTreeMap<String, serde_json::Value>,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// Relational SQL table row deletion tombstone.
    TableRowDelete {
        /// Target SQL table name.
        table: String,
        /// Deleted primary-key or row identifier string.
        row_id: String,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// Schema-less JSON document collection insert or update.
    DocumentUpsert {
        /// Target document collection name.
        collection: String,
        /// Document ID string.
        doc_id: String,
        /// Full JSON document payload.
        document: serde_json::Value,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// Schema-less JSON document deletion tombstone.
    DocumentDelete {
        /// Target document collection name.
        collection: String,
        /// Deleted document ID string.
        doc_id: String,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// Knowledge Graph vertex (node) upsert with optional dense vector embedding.
    GraphVertexUpsert {
        /// Unique vertex ID string.
        vertex_id: String,
        /// Vertex label / entity type.
        label: String,
        /// JSON object properties attached to the vertex.
        properties: serde_json::Value,
        /// Optional dense embedding vector associated with this entity.
        vector: Option<Vec<f32>>,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// Knowledge Graph directed edge upsert.
    GraphEdgeUpsert {
        /// Edge ID string.
        edge_id: String,
        /// Source vertex ID string.
        from_id: String,
        /// Target vertex ID string.
        to_id: String,
        /// Relationship type label.
        relation: String,
        /// Edge traversal weight.
        weight: f32,
        /// JSON object properties attached to the edge.
        properties: serde_json::Value,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
    /// AI Agent Memory entry upsert with semantic vector and multi-tenant scoping.
    AgentMemoryUpsert {
        /// Numeric memory entry ID.
        memory_id: u64,
        /// Raw text observation or fact.
        content: String,
        /// Optional dense vector embedding (e.g. 128D local feature hash or LLM embedding).
        vector: Option<Vec<f32>>,
        /// Intrinsic priority/importance score in `[0.0, 1.0]`.
        importance: f32,
        /// Categorical tags.
        tags: Vec<String>,
        /// Optional multi-agent isolation namespace.
        namespace: Option<String>,
        /// Optional conversation session identifier.
        session_id: Option<String>,
        /// Creation timestamp in Unix seconds.
        created_at: u64,
        /// Causal HLC version stamp.
        hlc: TapirSyncHlc,
    },
}

impl TapirSyncRecord {
    /// Return the globally unique canonical key for conflict resolution and watermark tracking.
    pub fn canonical_key(&self) -> String {
        match self {
            Self::TableRowUpsert { table, row_id, .. }
            | Self::TableRowDelete { table, row_id, .. } => format!("tbl:{table}:{row_id}"),
            Self::DocumentUpsert {
                collection, doc_id, ..
            }
            | Self::DocumentDelete {
                collection, doc_id, ..
            } => format!("doc:{collection}:{doc_id}"),
            Self::GraphVertexUpsert { vertex_id, .. } => format!("gv:{vertex_id}"),
            Self::GraphEdgeUpsert {
                from_id,
                to_id,
                relation,
                ..
            } => format!("ge:{from_id}:{relation}:{to_id}"),
            Self::AgentMemoryUpsert { memory_id, .. } => format!("mem:{memory_id}"),
        }
    }

    /// Return the causal HLC timestamp attached to this record.
    pub fn hlc(&self) -> &TapirSyncHlc {
        match self {
            Self::TableRowUpsert { hlc, .. }
            | Self::TableRowDelete { hlc, .. }
            | Self::DocumentUpsert { hlc, .. }
            | Self::DocumentDelete { hlc, .. }
            | Self::GraphVertexUpsert { hlc, .. }
            | Self::GraphEdgeUpsert { hlc, .. }
            | Self::AgentMemoryUpsert { hlc, .. } => hlc,
        }
    }

    /// Return `true` if this record is a deletion tombstone.
    pub fn is_delete(&self) -> bool {
        matches!(
            self,
            Self::TableRowDelete { .. } | Self::DocumentDelete { .. }
        )
    }

    /// Compute a deterministic, zero-allocation CRC32 digest over the record's logical content (excluding HLC).
    pub fn content_crc32(&self) -> u32 {
        let mut hasher = Hasher::new();
        match self {
            Self::TableRowUpsert {
                table,
                row_id,
                columns,
                ..
            } => {
                hasher.update(b"TRU");
                hasher.update(table.as_bytes());
                hasher.update(row_id.as_bytes());
                hasher.update(&(columns.len() as u32).to_le_bytes());
                for (k, v) in columns {
                    hasher.update(k.as_bytes());
                    hash_json_value(&mut hasher, v);
                }
            }
            Self::TableRowDelete { table, row_id, .. } => {
                hasher.update(b"TRD");
                hasher.update(table.as_bytes());
                hasher.update(row_id.as_bytes());
            }
            Self::DocumentUpsert {
                collection,
                doc_id,
                document,
                ..
            } => {
                hasher.update(b"DOU");
                hasher.update(collection.as_bytes());
                hasher.update(doc_id.as_bytes());
                hash_json_value(&mut hasher, document);
            }
            Self::DocumentDelete {
                collection, doc_id, ..
            } => {
                hasher.update(b"DOD");
                hasher.update(collection.as_bytes());
                hasher.update(doc_id.as_bytes());
            }
            Self::GraphVertexUpsert {
                vertex_id,
                label,
                properties,
                vector,
                ..
            } => {
                hasher.update(b"GVU");
                hasher.update(vertex_id.as_bytes());
                hasher.update(label.as_bytes());
                hash_json_value(&mut hasher, properties);
                if let Some(vec) = vector {
                    for f in vec {
                        hasher.update(&((*f as f64 * 1e6).round() as i64).to_le_bytes());
                    }
                }
            }
            Self::GraphEdgeUpsert {
                edge_id,
                from_id,
                to_id,
                relation,
                weight,
                properties,
                ..
            } => {
                hasher.update(b"GEU");
                hasher.update(edge_id.as_bytes());
                hasher.update(from_id.as_bytes());
                hasher.update(to_id.as_bytes());
                hasher.update(relation.as_bytes());
                hasher.update(&((*weight as f64 * 1e6).round() as i64).to_le_bytes());
                hash_json_value(&mut hasher, properties);
            }
            Self::AgentMemoryUpsert {
                memory_id,
                content,
                vector,
                importance,
                tags,
                namespace,
                session_id,
                created_at,
                ..
            } => {
                hasher.update(b"AMU");
                hasher.update(&memory_id.to_le_bytes());
                hasher.update(content.as_bytes());
                if let Some(vec) = vector {
                    for f in vec {
                        hasher.update(&((*f as f64 * 1e6).round() as i64).to_le_bytes());
                    }
                }
                hasher.update(&((*importance as f64 * 1e6).round() as i64).to_le_bytes());
                for tag in tags {
                    hasher.update(tag.as_bytes());
                }
                if let Some(ns) = namespace {
                    hasher.update(ns.as_bytes());
                }
                if let Some(sid) = session_id {
                    hasher.update(sid.as_bytes());
                }
                hasher.update(&created_at.to_le_bytes());
            }
        }
        hasher.finalize().max(1)
    }
}

fn hash_json_value(hasher: &mut Hasher, val: &serde_json::Value) {
    match val {
        serde_json::Value::Null => hasher.update(&[0]),
        serde_json::Value::Bool(b) => hasher.update(&[1, u8::from(*b)]),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                hasher.update(&[2]);
                hasher.update(&i.to_le_bytes());
            } else if let Some(u) = n.as_u64() {
                hasher.update(&[2]);
                hasher.update(&(u as i64).to_le_bytes());
            } else if let Some(f) = n.as_f64() {
                hasher.update(&[3]);
                let scaled = (f * 1e9).round() as i64;
                hasher.update(&scaled.to_le_bytes());
            }
        }
        serde_json::Value::String(s) => {
            hasher.update(&[4]);
            hasher.update(&(s.len() as u32).to_le_bytes());
            hasher.update(s.as_bytes());
        }
        serde_json::Value::Array(arr) => {
            hasher.update(&[5]);
            hasher.update(&(arr.len() as u32).to_le_bytes());
            for item in arr {
                hash_json_value(hasher, item);
            }
        }
        serde_json::Value::Object(map) => {
            hasher.update(&[6]);
            hasher.update(&(map.len() as u32).to_le_bytes());
            for (k, v) in map {
                hasher.update(k.as_bytes());
                hash_json_value(hasher, v);
            }
        }
    }
}

/// An atomic, CRC32-verified `TapirSync` (`TPSYNC01`) replication bundle transmitted
/// between an edge `.tapir` database and a `TapisaurusDB` server cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapirSyncBundle {
    /// Wire protocol version (`1`).
    pub protocol_version: u16,
    /// Identifier of the sender node (e.g. `"edge-iot-01"` or `"cloud-node-1"`).
    pub source_node_id: String,
    /// Prior sequence watermark on which this incremental bundle is based.
    pub base_sequence: u64,
    /// New sequence watermark after applying this bundle.
    pub target_sequence: u64,
    /// Sender's Hybrid Logical Clock stamp at bundle creation.
    pub hlc: TapirSyncHlc,
    /// Ordered list of multi-modal logical replication records.
    pub records: Vec<TapirSyncRecord>,
    /// Optional raw `TWR1` page-level WAL replication chunk for standby page archival.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_chunk_bytes: Option<Vec<u8>>,
    /// Hardware-accelerated CRC32 checksum protecting bundle metadata and all records.
    pub bundle_crc32: u32,
}

impl TapirSyncBundle {
    /// Construct a new `TapirSyncBundle` and compute its `bundle_crc32`.
    pub fn new(
        source_node_id: impl Into<String>,
        base_sequence: u64,
        target_sequence: u64,
        hlc: TapirSyncHlc,
        records: Vec<TapirSyncRecord>,
        wal_chunk_bytes: Option<Vec<u8>>,
    ) -> Self {
        let mut bundle = Self {
            protocol_version: TAPIRSYNC_PROTOCOL_VERSION,
            source_node_id: source_node_id.into(),
            base_sequence,
            target_sequence,
            hlc,
            records,
            wal_chunk_bytes,
            bundle_crc32: 0,
        };
        bundle.bundle_crc32 = bundle.calculate_crc();
        bundle
    }

    /// Compute the canonical CRC32 checksum across bundle header fields and all records.
    pub fn calculate_crc(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(TAPIRSYNC_WIRE_MAGIC);
        hasher.update(&self.protocol_version.to_le_bytes());
        hasher.update(self.source_node_id.as_bytes());
        hasher.update(&self.base_sequence.to_le_bytes());
        hasher.update(&self.target_sequence.to_le_bytes());
        hasher.update(&self.hlc.wall_time_ms.to_le_bytes());
        hasher.update(&self.hlc.logical_counter.to_le_bytes());
        hasher.update(self.hlc.node_id.as_bytes());
        hasher.update(&(self.records.len() as u32).to_le_bytes());
        for rec in &self.records {
            hasher.update(rec.canonical_key().as_bytes());
            hasher.update(&rec.content_crc32().to_le_bytes());
            hasher.update(&rec.hlc().wall_time_ms.to_le_bytes());
            hasher.update(&rec.hlc().logical_counter.to_le_bytes());
            hasher.update(rec.hlc().node_id.as_bytes());
        }
        if let Some(ref wal) = self.wal_chunk_bytes {
            hasher.update(&(wal.len() as u32).to_le_bytes());
            hasher.update(wal);
        }
        hasher.finalize()
    }

    /// Verify the cryptographic/CRC32 integrity and structural invariants of this bundle.
    pub fn verify(&self) -> Result<()> {
        if self.protocol_version != TAPIRSYNC_PROTOCOL_VERSION {
            return Err(Error::Corrupted(format!(
                "Unsupported TapirSync protocol version: {}",
                self.protocol_version
            )));
        }
        if self.target_sequence < self.base_sequence {
            return Err(Error::Corrupted(
                "TapirSync target_sequence cannot be less than base_sequence".into(),
            ));
        }
        let expected = self.calculate_crc();
        if self.bundle_crc32 != expected {
            return Err(Error::Corrupted(format!(
                "TapirSync bundle CRC32 mismatch: expected {expected:#010x}, found {:#010x}",
                self.bundle_crc32
            )));
        }
        Ok(())
    }

    /// Encode this bundle into a compact `TPSYNC01` binary wire frame.
    ///
    /// Wire layout (38-byte fixed header + payload):
    /// - `[0..8]`: `b"TPSYNC01"`
    /// - `[8..10]`: `protocol_version` (`u16` LE)
    /// - `[10..18]`: `base_sequence` (`u64` LE)
    /// - `[18..26]`: `target_sequence` (`u64` LE)
    /// - `[26..30]`: `record_count` (`u32` LE)
    /// - `[30..34]`: `bundle_crc32` (`u32` LE)
    /// - `[34..38]`: `payload_len` (`u32` LE)
    /// - `[38..38 + payload_len]`: UTF-8 JSON body of `TapirSyncBundle`
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let body = serde_json::to_vec(self).map_err(|e| {
            Error::Corrupted(format!("Failed to serialize TapirSyncBundle payload: {e}"))
        })?;
        let payload_len = u32::try_from(body.len()).map_err(|_| {
            Error::Corrupted("TapirSyncBundle payload exceeds 4 GiB wire limit".into())
        })?;
        let record_count = u32::try_from(self.records.len()).map_err(|_| {
            Error::Corrupted("TapirSyncBundle record count exceeds u32 limit".into())
        })?;

        let mut out = Vec::with_capacity(38 + body.len());
        out.extend_from_slice(TAPIRSYNC_WIRE_MAGIC);
        out.extend_from_slice(&self.protocol_version.to_le_bytes());
        out.extend_from_slice(&self.base_sequence.to_le_bytes());
        out.extend_from_slice(&self.target_sequence.to_le_bytes());
        out.extend_from_slice(&record_count.to_le_bytes());
        out.extend_from_slice(&self.bundle_crc32.to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decode a `TapirSyncBundle` from either a `TPSYNC01` binary wire frame or raw JSON bytes,
    /// verifying header consistency and `bundle_crc32` before returning.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bundle: Self = if bytes.starts_with(TAPIRSYNC_WIRE_MAGIC) {
            if bytes.len() < 38 {
                return Err(Error::Corrupted(
                    "Truncated TPSYNC01 binary wire header (< 38 bytes)".into(),
                ));
            }
            let version = u16::from_le_bytes([bytes[8], bytes[9]]);
            let base_seq = u64::from_le_bytes(bytes[10..18].try_into().unwrap());
            let target_seq = u64::from_le_bytes(bytes[18..26].try_into().unwrap());
            let record_count = u32::from_le_bytes(bytes[26..30].try_into().unwrap()) as usize;
            let header_crc = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
            let payload_len = u32::from_le_bytes(bytes[34..38].try_into().unwrap()) as usize;

            if bytes.len() != 38 + payload_len {
                return Err(Error::Corrupted(format!(
                    "TPSYNC01 frame length mismatch: expected {}, got {}",
                    38 + payload_len,
                    bytes.len()
                )));
            }
            let decoded: Self = serde_json::from_slice(&bytes[38..]).map_err(|e| {
                Error::Corrupted(format!("Invalid TPSYNC01 payload JSON: {e}"))
            })?;
            if decoded.protocol_version != version
                || decoded.base_sequence != base_seq
                || decoded.target_sequence != target_seq
                || decoded.records.len() != record_count
                || decoded.bundle_crc32 != header_crc
            {
                return Err(Error::Corrupted(
                    "TPSYNC01 binary envelope header does not match payload metadata".into(),
                ));
            }
            decoded
        } else {
            serde_json::from_slice(bytes).map_err(|e| {
                Error::Corrupted(format!("Failed to decode TapirSyncBundle JSON: {e}"))
            })?
        };
        bundle.verify()?;
        Ok(bundle)
    }
}

/// Replication acknowledgement receipt returned after applying a `TapirSyncBundle`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TapirSyncAck {
    /// Whether the bundle was verified and accepted.
    pub accepted: bool,
    /// Identifier of the node that applied the bundle.
    pub responder_node_id: String,
    /// Identifier of the source node that produced the bundle.
    pub source_node_id: String,
    /// New committed sequence watermark for `source_node_id`.
    pub applied_sequence: u64,
    /// Number of records applied to local storage.
    pub records_applied: usize,
    /// Number of incoming records skipped because local state held a newer HLC timestamp.
    pub conflicts_resolved: usize,
    /// Responder's updated Hybrid Logical Clock after applying the bundle.
    pub responder_hlc: TapirSyncHlc,
    /// CRC32 checksum over the acknowledgement fields.
    pub ack_crc32: u32,
}

impl TapirSyncAck {
    /// Construct and sign a new `TapirSyncAck`.
    pub fn new(
        accepted: bool,
        responder_node_id: impl Into<String>,
        source_node_id: impl Into<String>,
        applied_sequence: u64,
        records_applied: usize,
        conflicts_resolved: usize,
        responder_hlc: TapirSyncHlc,
    ) -> Self {
        let mut ack = Self {
            accepted,
            responder_node_id: responder_node_id.into(),
            source_node_id: source_node_id.into(),
            applied_sequence,
            records_applied,
            conflicts_resolved,
            responder_hlc,
            ack_crc32: 0,
        };
        ack.ack_crc32 = ack.calculate_crc();
        ack
    }

    /// Calculate CRC32 integrity checksum for this acknowledgement receipt.
    pub fn calculate_crc(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(&[u8::from(self.accepted)]);
        hasher.update(self.responder_node_id.as_bytes());
        hasher.update(self.source_node_id.as_bytes());
        hasher.update(&self.applied_sequence.to_le_bytes());
        hasher.update(&(self.records_applied as u64).to_le_bytes());
        hasher.update(&(self.conflicts_resolved as u64).to_le_bytes());
        hasher.update(&self.responder_hlc.wall_time_ms.to_le_bytes());
        hasher.update(&self.responder_hlc.logical_counter.to_le_bytes());
        hasher.update(self.responder_hlc.node_id.as_bytes());
        hasher.finalize()
    }
}

/// Stateful incremental replication cursor for an Edge `.tapir` database.
///
/// Tracks per-record content CRC32 digests and causal HLC stamps so that repeated calls
/// to [`Connection::tapirsync_export_bundle`] transmit only newly inserted, updated,
/// or deleted records since the previous sync cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapirSyncCursor {
    /// Unique identifier for this edge node (e.g. `"edge-mobile-01"`).
    pub node_id: String,
    /// Last exported sequence watermark.
    pub last_exported_sequence: u64,
    /// Last applied remote sequence watermark.
    pub last_applied_sequence: u64,
    /// Local Hybrid Logical Clock state.
    pub hlc: TapirSyncHlc,
    /// Whether to attach a raw `TWR1` page-level WAL replication chunk to exported bundles.
    pub include_wal_chunk: bool,
    /// Map of `canonical_key -> (content_crc32, hlc)`. A `content_crc32` of `0` denotes a deletion tombstone.
    pub known_records: BTreeMap<String, (u32, TapirSyncHlc)>,
}

impl TapirSyncCursor {
    /// Create a new incremental sync cursor for `node_id`.
    pub fn new(node_id: impl Into<String>) -> Self {
        let id = node_id.into();
        let now_ms = current_unix_ms();
        Self {
            node_id: id.clone(),
            last_exported_sequence: 0,
            last_applied_sequence: 0,
            hlc: TapirSyncHlc::new(id, now_ms),
            include_wal_chunk: false,
            known_records: BTreeMap::new(),
        }
    }

    /// Enable or disable embedding raw `TWR1` page-level WAL chunks inside exported bundles.
    pub fn with_wal_chunk(mut self, include: bool) -> Self {
        self.include_wal_chunk = include;
        self
    }
}

fn current_unix_ms() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn db_value_to_json(val: &Value) -> serde_json::Value {
    match val {
        Value::Null => serde_json::Value::Null,
        Value::Integer(i) => serde_json::json!(*i),
        Value::Real(r) => serde_json::Number::from_f64(*r)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Blob(b) => serde_json::json!({ "$blob": b }),
        Value::Vector(v) => serde_json::json!(v),
    }
}

fn json_to_sql_type(val: &serde_json::Value) -> &'static str {
    match val {
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            if val.is_f64() && !val.is_i64() && !val.is_u64() {
                "REAL"
            } else {
                "INTEGER"
            }
        }
        serde_json::Value::Object(map) if map.len() == 1 && map.contains_key("$blob") => "BLOB",
        _ => "TEXT",
    }
}

fn sanitize_ident(ident: &str) -> Result<&str> {
    if ident.is_empty() || !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::SqlSyntax(format!(
            "Invalid identifier '{ident}' in TapirSync record"
        )));
    }
    Ok(ident)
}

impl Connection {
    /// Export an incremental `TapirSyncBundle` (`TPSYNC01`) containing all SQL rows,
    /// Document collection records, Knowledge Graph vertices/edges, and AI Agent Memories
    /// added, modified, or deleted since `cursor.last_exported_sequence`.
    pub fn tapirsync_export_bundle(&self, cursor: &mut TapirSyncCursor) -> Result<TapirSyncBundle> {
        let now_ms = current_unix_ms();
        let base_seq = cursor.last_exported_sequence;
        let mut records = Vec::new();
        let mut seen_mutable_keys = BTreeSet::new();

        // 1. Export user-defined Relational SQL Tables
        for table_def in self.tables() {
            let table_name = &table_def.name;
            let pk_col = table_def
                .primary_key_index()
                .map(|idx| table_def.columns[idx].name.clone());
            let sql = format!("SELECT * FROM {table_name};");
            let rows = self.query(&sql)?;
            for (row_idx, row) in rows.iter().enumerate() {
                let mut columns = BTreeMap::new();
                for (col_name, val) in row.columns().iter().zip(row.values().iter()) {
                    columns.insert(col_name.clone(), db_value_to_json(val));
                }
                let row_id = pk_col
                    .as_ref()
                    .and_then(|pk| row.get_value(pk))
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("row_{}", row_idx + 1));

                let candidate = TapirSyncRecord::TableRowUpsert {
                    table: table_name.clone(),
                    row_id,
                    columns,
                    hlc: cursor.hlc.clone(),
                };
                let key = candidate.canonical_key();
                let crc = candidate.content_crc32();
                seen_mutable_keys.insert(key.clone());

                let changed = cursor
                    .known_records
                    .get(&key)
                    .is_none_or(|(old_crc, _)| *old_crc != crc);
                if changed {
                    let hlc = cursor.hlc.tick(now_ms);
                    let rec = match candidate {
                        TapirSyncRecord::TableRowUpsert {
                            table,
                            row_id,
                            columns,
                            ..
                        } => TapirSyncRecord::TableRowUpsert {
                            table,
                            row_id,
                            columns,
                            hlc: hlc.clone(),
                        },
                        other => other,
                    };
                    cursor.known_records.insert(key, (crc, hlc));
                    records.push(rec);
                }
            }
        }

        // 2. Export JSON Document Collections (`__doc_<name>`)
        for col_name in self.collections() {
            let col = self.collection(&col_name)?;
            for (doc_id_num, doc_json) in col.find_all()? {
                let doc_id = doc_id_num.to_string();
                let candidate = TapirSyncRecord::DocumentUpsert {
                    collection: col_name.clone(),
                    doc_id,
                    document: doc_json,
                    hlc: cursor.hlc.clone(),
                };
                let key = candidate.canonical_key();
                let crc = candidate.content_crc32();
                seen_mutable_keys.insert(key.clone());

                let changed = cursor
                    .known_records
                    .get(&key)
                    .is_none_or(|(old_crc, _)| *old_crc != crc);
                if changed {
                    let hlc = cursor.hlc.tick(now_ms);
                    let rec = match candidate {
                        TapirSyncRecord::DocumentUpsert {
                            collection,
                            doc_id,
                            document,
                            ..
                        } => TapirSyncRecord::DocumentUpsert {
                            collection,
                            doc_id,
                            document,
                            hlc: hlc.clone(),
                        },
                        other => other,
                    };
                    cursor.known_records.insert(key, (crc, hlc));
                    records.push(rec);
                }
            }
        }

        // 3. Emit tombstones for SQL table rows or Documents that were deleted locally
        let prior_keys: Vec<String> = cursor.known_records.keys().cloned().collect();
        for key in prior_keys {
            if seen_mutable_keys.contains(&key) {
                continue;
            }
            let Some((old_crc, _)) = cursor.known_records.get(&key) else {
                continue;
            };
            if *old_crc == 0 {
                // Already emitted tombstone
                continue;
            }
            if let Some(rest) = key.strip_prefix("tbl:") {
                if let Some((table, row_id)) = rest.split_once(':') {
                    let hlc = cursor.hlc.tick(now_ms);
                    records.push(TapirSyncRecord::TableRowDelete {
                        table: table.to_string(),
                        row_id: row_id.to_string(),
                        hlc: hlc.clone(),
                    });
                    cursor.known_records.insert(key, (0, hlc));
                }
            } else if let Some(rest) = key.strip_prefix("doc:")
                && let Some((collection, doc_id)) = rest.split_once(':')
            {
                let hlc = cursor.hlc.tick(now_ms);
                records.push(TapirSyncRecord::DocumentDelete {
                    collection: collection.to_string(),
                    doc_id: doc_id.to_string(),
                    hlc: hlc.clone(),
                });
                cursor.known_records.insert(key, (0, hlc));
            }
        }

        // 4. Export Knowledge Graph Nodes & Edges
        for node in self.graph_nodes() {
            let props: serde_json::Value = serde_json::from_str(&node.properties)
                .unwrap_or_else(|_| serde_json::json!({ "raw": node.properties }));
            let candidate = TapirSyncRecord::GraphVertexUpsert {
                vertex_id: node.id.to_string(),
                label: node.label,
                properties: props,
                vector: node.vector,
                hlc: cursor.hlc.clone(),
            };
            let key = candidate.canonical_key();
            let crc = candidate.content_crc32();
            if cursor
                .known_records
                .get(&key)
                .is_none_or(|(old_crc, _)| *old_crc != crc)
            {
                let hlc = cursor.hlc.tick(now_ms);
                let rec = match candidate {
                    TapirSyncRecord::GraphVertexUpsert {
                        vertex_id,
                        label,
                        properties,
                        vector,
                        ..
                    } => TapirSyncRecord::GraphVertexUpsert {
                        vertex_id,
                        label,
                        properties,
                        vector,
                        hlc: hlc.clone(),
                    },
                    other => other,
                };
                cursor.known_records.insert(key, (crc, hlc));
                records.push(rec);
            }
        }

        for edge in self.graph_edges() {
            let props: serde_json::Value = serde_json::from_str(&edge.properties)
                .unwrap_or_else(|_| serde_json::json!({}));
            let candidate = TapirSyncRecord::GraphEdgeUpsert {
                edge_id: edge.id.to_string(),
                from_id: edge.from_id.to_string(),
                to_id: edge.to_id.to_string(),
                relation: edge.label,
                weight: edge.weight,
                properties: props,
                hlc: cursor.hlc.clone(),
            };
            let key = candidate.canonical_key();
            let crc = candidate.content_crc32();
            if cursor
                .known_records
                .get(&key)
                .is_none_or(|(old_crc, _)| *old_crc != crc)
            {
                let hlc = cursor.hlc.tick(now_ms);
                let rec = match candidate {
                    TapirSyncRecord::GraphEdgeUpsert {
                        edge_id,
                        from_id,
                        to_id,
                        relation,
                        weight,
                        properties,
                        ..
                    } => TapirSyncRecord::GraphEdgeUpsert {
                        edge_id,
                        from_id,
                        to_id,
                        relation,
                        weight,
                        properties,
                        hlc: hlc.clone(),
                    },
                    other => other,
                };
                cursor.known_records.insert(key, (crc, hlc));
                records.push(rec);
            }
        }

        // 5. Export AI Agent Memories
        let mut memory_entries: Vec<_> = self.memory.read().all_entries().values().cloned().collect();
        memory_entries.sort_by_key(|e| e.id);
        for entry in memory_entries {
            let candidate = TapirSyncRecord::AgentMemoryUpsert {
                memory_id: entry.id,
                content: entry.content,
                vector: entry.vector,
                importance: entry.importance,
                tags: entry.tags,
                namespace: entry.namespace,
                session_id: entry.session_id,
                created_at: entry.timestamp,
                hlc: cursor.hlc.clone(),
            };
            let key = candidate.canonical_key();
            let crc = candidate.content_crc32();
            if cursor
                .known_records
                .get(&key)
                .is_none_or(|(old_crc, _)| *old_crc != crc)
            {
                let hlc = cursor.hlc.tick(now_ms);
                let rec = match candidate {
                    TapirSyncRecord::AgentMemoryUpsert {
                        memory_id,
                        content,
                        vector,
                        importance,
                        tags,
                        namespace,
                        session_id,
                        created_at,
                        ..
                    } => TapirSyncRecord::AgentMemoryUpsert {
                        memory_id,
                        content,
                        vector,
                        importance,
                        tags,
                        namespace,
                        session_id,
                        created_at,
                        hlc: hlc.clone(),
                    },
                    other => other,
                };
                cursor.known_records.insert(key, (crc, hlc));
                records.push(rec);
            }
        }

        let wal_chunk_bytes = if cursor.include_wal_chunk {
            Some(self.export_replication_chunk()?.to_bytes()?)
        } else {
            None
        };

        let target_seq = if records.is_empty() && wal_chunk_bytes.is_none() {
            base_seq
        } else {
            base_seq.saturating_add(1)
        };
        cursor.last_exported_sequence = target_seq;

        Ok(TapirSyncBundle::new(
            cursor.node_id.clone(),
            base_seq,
            target_seq,
            cursor.hlc.clone(),
            records,
            wal_chunk_bytes,
        ))
    }

    /// Apply an incoming `TapirSyncBundle` (`TPSYNC01`) from a cloud `TapisaurusDB` cluster
    /// or peer edge node into this local `.tapir` database, resolving concurrent conflicts
    /// deterministically via Hybrid Logical Clock Last-Write-Wins (HLC-LWW).
    pub fn tapirsync_apply_bundle(
        &self,
        bundle: &TapirSyncBundle,
        cursor: &mut TapirSyncCursor,
    ) -> Result<TapirSyncAck> {
        bundle.verify()?;
        let now_ms = current_unix_ms();
        cursor.hlc.observe(&bundle.hlc, now_ms);

        let mut records_applied = 0usize;
        let mut conflicts_resolved = 0usize;

        for rec in &bundle.records {
            let key = rec.canonical_key();
            cursor.hlc.observe(rec.hlc(), now_ms);

            if let Some((_, local_hlc)) = cursor.known_records.get(&key)
                && rec.hlc() < local_hlc
            {
                conflicts_resolved += 1;
                continue;
            }

            match rec {
                TapirSyncRecord::TableRowUpsert {
                    table,
                    row_id,
                    columns,
                    hlc,
                } => {
                    sanitize_ident(table)?;
                    self.apply_tapirsync_table_upsert(table, row_id, columns)?;
                    cursor.known_records.insert(
                        key,
                        (rec.content_crc32(), hlc.clone()),
                    );
                    records_applied += 1;
                }
                TapirSyncRecord::TableRowDelete { table, row_id, hlc } => {
                    sanitize_ident(table)?;
                    if let Some(table_def) = self.table(table) {
                        let pk_col = table_def
                            .primary_key_index()
                            .map(|idx| table_def.columns[idx].name.clone())
                            .unwrap_or_else(|| "id".to_string());
                        let param = if let Ok(i) = row_id.parse::<i64>() {
                            Value::Integer(i)
                        } else {
                            Value::Text(row_id.clone())
                        };
                        let sql = format!("DELETE FROM {table} WHERE {pk_col} = ?;");
                        let _ = self.execute_with_params(&sql, &[param]);
                    }
                    cursor.known_records.insert(key, (0, hlc.clone()));
                    records_applied += 1;
                }
                TapirSyncRecord::DocumentUpsert {
                    collection,
                    doc_id,
                    document,
                    hlc,
                } => {
                    let col = self.collection(collection)?;
                    let numeric_id = doc_id
                        .parse::<u64>()
                        .unwrap_or_else(|_| u64::from(crc32fast::hash(doc_id.as_bytes())));
                    if col.find_by_id(numeric_id)?.is_some() {
                        col.update_by_id(numeric_id, document)?;
                    } else {
                        col.insert_with_id(numeric_id, document)?;
                    }
                    cursor.known_records.insert(
                        key,
                        (rec.content_crc32(), hlc.clone()),
                    );
                    records_applied += 1;
                }
                TapirSyncRecord::DocumentDelete {
                    collection,
                    doc_id,
                    hlc,
                } => {
                    let col = self.collection(collection)?;
                    let numeric_id = doc_id
                        .parse::<u64>()
                        .unwrap_or_else(|_| u64::from(crc32fast::hash(doc_id.as_bytes())));
                    let _ = col.delete(numeric_id)?;
                    cursor.known_records.insert(key, (0, hlc.clone()));
                    records_applied += 1;
                }
                TapirSyncRecord::GraphVertexUpsert {
                    vertex_id,
                    label,
                    properties,
                    vector,
                    hlc,
                } => {
                    let numeric_id = vertex_id
                        .parse::<u64>()
                        .unwrap_or_else(|_| u64::from(crc32fast::hash(vertex_id.as_bytes())));
                    let props_str = serde_json::to_string(properties).unwrap_or_default();
                    self.graph_add_node_with_vector(
                        numeric_id,
                        label,
                        &props_str,
                        vector.as_deref(),
                    )?;
                    cursor.known_records.insert(
                        key,
                        (rec.content_crc32(), hlc.clone()),
                    );
                    records_applied += 1;
                }
                TapirSyncRecord::GraphEdgeUpsert {
                    from_id,
                    to_id,
                    relation,
                    weight,
                    properties,
                    hlc,
                    ..
                } => {
                    let from_num = from_id
                        .parse::<u64>()
                        .unwrap_or_else(|_| u64::from(crc32fast::hash(from_id.as_bytes())));
                    let to_num = to_id
                        .parse::<u64>()
                        .unwrap_or_else(|_| u64::from(crc32fast::hash(to_id.as_bytes())));
                    let props_str = serde_json::to_string(properties).unwrap_or_default();
                    self.graph_add_edge(from_num, to_num, relation, *weight, &props_str)?;
                    cursor.known_records.insert(
                        key,
                        (rec.content_crc32(), hlc.clone()),
                    );
                    records_applied += 1;
                }
                TapirSyncRecord::AgentMemoryUpsert {
                    content,
                    vector,
                    importance,
                    tags,
                    namespace,
                    session_id,
                    hlc,
                    ..
                } => {
                    let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
                    let effective_vec = match vector {
                        Some(v) if !v.is_empty() => v.clone(),
                        _ => crate::memory::DeterministicHashEmbedder::default().embed_text(content),
                    };
                    self.memory_remember_scoped(
                        content,
                        Some(&effective_vec),
                        *importance,
                        &tag_refs,
                        namespace.as_deref(),
                        session_id.as_deref(),
                    )?;
                    cursor.known_records.insert(
                        key,
                        (rec.content_crc32(), hlc.clone()),
                    );
                    records_applied += 1;
                }
            }
        }

        cursor.last_applied_sequence = cursor.last_applied_sequence.max(bundle.target_sequence);

        Ok(TapirSyncAck::new(
            true,
            cursor.node_id.clone(),
            bundle.source_node_id.clone(),
            bundle.target_sequence,
            records_applied,
            conflicts_resolved,
            cursor.hlc.clone(),
        ))
    }

    fn apply_tapirsync_table_upsert(
        &self,
        table: &str,
        row_id: &str,
        columns: &BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        if self.table(table).is_none() {
            let mut col_defs = Vec::new();
            let has_id_col = columns.contains_key("id");
            if !has_id_col {
                let id_type = if row_id.parse::<i64>().is_ok() {
                    "INTEGER PRIMARY KEY"
                } else {
                    "TEXT PRIMARY KEY"
                };
                col_defs.push(format!("id {id_type}"));
            }
            for (col_name, val) in columns {
                sanitize_ident(col_name)?;
                let sql_type = json_to_sql_type(val);
                if col_name.eq_ignore_ascii_case("id") {
                    col_defs.push(format!("{col_name} {sql_type} PRIMARY KEY"));
                } else {
                    col_defs.push(format!("{col_name} {sql_type}"));
                }
            }
            let create_sql = format!(
                "CREATE TABLE IF NOT EXISTS {table} ({});",
                col_defs.join(", ")
            );
            self.execute(&create_sql)?;
        }

        let table_def = self
            .table(table)
            .ok_or_else(|| Error::TableNotFound(table.to_string()))?;
        let pk_col = table_def
            .primary_key_index()
            .map(|idx| table_def.columns[idx].name.clone());

        if let Some(ref pk) = pk_col {
            let pk_val = if let Some(v) = columns.get(pk) {
                Value::from_json_parameter(v)?
            } else if let Ok(i) = row_id.parse::<i64>() {
                Value::Integer(i)
            } else {
                Value::Text(row_id.to_string())
            };
            let del_sql = format!("DELETE FROM {table} WHERE {pk} = ?;");
            let _ = self.execute_with_params(&del_sql, &[pk_val]);
        }

        let mut col_names = Vec::new();
        let mut placeholders = Vec::new();
        let mut params = Vec::new();

        for col_def in &table_def.columns {
            if let Some(json_val) = columns.get(&col_def.name) {
                col_names.push(col_def.name.clone());
                placeholders.push("?");
                let raw_val = match (&col_def.data_type, json_val) {
                    (DataType::Text, serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
                        Value::Text(json_val.to_string())
                    }
                    _ => Value::from_json_parameter(json_val)?,
                };
                params.push(raw_val);
            } else if col_def.primary_key {
                col_names.push(col_def.name.clone());
                placeholders.push("?");
                let pk_val = if let Ok(i) = row_id.parse::<i64>() {
                    Value::Integer(i)
                } else {
                    Value::Text(row_id.to_string())
                };
                params.push(pk_val);
            }
        }

        if !col_names.is_empty() {
            let insert_sql = format!(
                "INSERT INTO {table} ({}) VALUES ({});",
                col_names.join(", "),
                placeholders.join(", ")
            );
            self.execute_with_params(&insert_sql, &params)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tapirsync_binary_wire_roundtrip_and_tamper_detection() {
        let conn = Connection::open_in_memory().expect("open edge db");
        conn.execute("CREATE TABLE sensors (id INTEGER PRIMARY KEY, region TEXT, reading REAL);")
            .unwrap();
        conn.execute("INSERT INTO sensors VALUES (1, 'kl-edge', 36.5);")
            .unwrap();

        let col = conn.collection("alerts").unwrap();
        col.insert_one(&json!({"severity": "high", "code": 501}))
            .unwrap();

        conn.graph_add_node(10, "Device", r#"{"model":"TapirEdge-X1"}"#)
            .unwrap();
        conn.graph_add_node(20, "Gateway", r#"{"region":"ap-southeast-1"}"#)
            .unwrap();
        conn.graph_add_edge(10, 20, "UPLINKS_TO", 0.95, "").unwrap();
        conn.remember("Device 10 calibrated at 36.5C in Kuala Lumpur")
            .unwrap();

        let mut cursor = TapirSyncCursor::new("edge-device-kl-01");
        let bundle = conn
            .tapirsync_export_bundle(&mut cursor)
            .expect("export initial bundle");
        assert_eq!(bundle.base_sequence, 0);
        assert_eq!(bundle.target_sequence, 1);
        assert_eq!(bundle.records.len(), 6); // 1 SQL row + 1 Doc + 2 Graph vertices + 1 Graph edge + 1 Memory

        let wire_bytes = bundle.to_bytes().expect("encode TPSYNC01 wire frame");
        assert!(wire_bytes.starts_with(TAPIRSYNC_WIRE_MAGIC));

        let decoded = TapirSyncBundle::from_bytes(&wire_bytes).expect("decode TPSYNC01 wire frame");
        assert_eq!(decoded, bundle);

        // Tamper with a single byte in payload -> CRC32 must reject
        let mut tampered = wire_bytes.clone();
        let last_idx = tampered.len() - 2;
        tampered[last_idx] ^= 0xFF;
        assert!(TapirSyncBundle::from_bytes(&tampered).is_err());
    }

    #[test]
    fn test_tapirsync_incremental_delta_and_bidirectional_lww_convergence() {
        let edge_a = Connection::open_in_memory().expect("open edge_a");
        let edge_b = Connection::open_in_memory().expect("open edge_b");

        let mut cursor_a = TapirSyncCursor::new("edge-a");
        let mut cursor_b = TapirSyncCursor::new("edge-b");

        // 1. Edge A writes initial SQL, Document, Graph, and Agent Memory offline
        edge_a
            .execute("CREATE TABLE inventory (id INTEGER PRIMARY KEY, item TEXT, qty INTEGER);")
            .unwrap();
        edge_a
            .execute("INSERT INTO inventory VALUES (1, 'NVMe-4TB', 12);")
            .unwrap();
        edge_a
            .execute("INSERT INTO inventory VALUES (2, 'DDR5-64GB', 24);")
            .unwrap();
        edge_a
            .collection("configs")
            .unwrap()
            .insert_one(&json!({"mode": "autonomous"}))
            .unwrap();
        edge_a
            .remember("Warehouse A stocked 12 NVMe drives")
            .unwrap();

        let bundle_1 = edge_a.tapirsync_export_bundle(&mut cursor_a).unwrap();
        assert_eq!(bundle_1.records.len(), 4);

        // Immediate second export with zero changes must produce 0 records
        let bundle_noop = edge_a.tapirsync_export_bundle(&mut cursor_a).unwrap();
        assert!(bundle_noop.records.is_empty());

        // 2. Apply bundle_1 into Edge B
        let ack_1 = edge_b
            .tapirsync_apply_bundle(&bundle_1, &mut cursor_b)
            .unwrap();
        assert!(ack_1.accepted);
        assert_eq!(ack_1.records_applied, 4);
        assert_eq!(ack_1.conflicts_resolved, 0);

        let rows_b = edge_b
            .query("SELECT item, qty FROM inventory ORDER BY id;")
            .unwrap();
        assert_eq!(rows_b.len(), 2);
        assert_eq!(rows_b[0].get::<i64>("qty").unwrap(), 12);

        // 3. Edge A updates row 1 and deletes row 2; only those 2 deltas should be exported!
        edge_a
            .execute("UPDATE inventory SET qty = 19 WHERE id = 1;")
            .unwrap();
        edge_a
            .execute("DELETE FROM inventory WHERE id = 2;")
            .unwrap();

        let bundle_2 = edge_a.tapirsync_export_bundle(&mut cursor_a).unwrap();
        assert_eq!(bundle_2.records.len(), 2);
        assert!(bundle_2.records.iter().any(|r| r.is_delete()));

        let ack_2 = edge_b
            .tapirsync_apply_bundle(&bundle_2, &mut cursor_b)
            .unwrap();
        assert_eq!(ack_2.records_applied, 2);

        let rows_b_after = edge_b
            .query("SELECT id, qty FROM inventory ORDER BY id;")
            .unwrap();
        assert_eq!(rows_b_after.len(), 1);
        assert_eq!(rows_b_after[0].get::<i64>("id").unwrap(), 1);
        assert_eq!(rows_b_after[0].get::<i64>("qty").unwrap(), 19);

        // 4. Stale bundle replay with older HLC must be rejected by LWW conflict resolution
        let stale_record = TapirSyncRecord::TableRowUpsert {
            table: "inventory".to_string(),
            row_id: "1".to_string(),
            columns: BTreeMap::from([
                ("id".to_string(), json!(1)),
                ("item".to_string(), json!("NVMe-4TB")),
                ("qty".to_string(), json!(999)),
            ]),
            hlc: TapirSyncHlc {
                wall_time_ms: 1, // ancient timestamp
                logical_counter: 0,
                node_id: "stale-edge".to_string(),
            },
        };
        let stale_bundle = TapirSyncBundle::new(
            "stale-edge",
            0,
            1,
            TapirSyncHlc::new("stale-edge", 1),
            vec![stale_record],
            None,
        );
        let ack_stale = edge_b
            .tapirsync_apply_bundle(&stale_bundle, &mut cursor_b)
            .unwrap();
        assert_eq!(ack_stale.records_applied, 0);
        assert_eq!(ack_stale.conflicts_resolved, 1);

        // Value in Edge B must remain 19, not overwritten by stale 999
        let rows_b_final = edge_b
            .query("SELECT qty FROM inventory WHERE id = 1;")
            .unwrap();
        assert_eq!(rows_b_final[0].get::<i64>("qty").unwrap(), 19);
    }
}
