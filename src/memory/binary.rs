//! Compact, bounds-checked safe binary codecs for persisted AI Memory records and HNSW caches.

use super::MemoryEntry;
use crate::error::{Error, Result};
use crate::vector::hnsw::HnswNode;
use crate::vector::{DistanceMetric, HnswConfig, HnswSnapshot, QuantizedVector8};

pub(crate) const MEMORY_ENTRY_BIN_MAGIC: &[u8; 8] = b"TPMEM001";
const MEMORY_INDEX_BIN_MAGIC: &[u8; 8] = b"TPIDX001";

#[derive(Debug, Clone)]
pub(crate) struct CachedVectorSpace {
    pub model: Option<String>,
    pub dims: usize,
    pub next_node: u64,
    /// (node_id, doc_id, doc_fingerprint)
    pub documents: Vec<(u64, u64, u64)>,
    pub snapshot: HnswSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedMemoryIndexes {
    pub hnsw_m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub spaces: Vec<CachedVectorSpace>,
}

struct BinWriter {
    buf: Vec<u8>,
}

impl BinWriter {
    fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    fn write_bytes_raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_len(&mut self, len: usize, label: &str) -> Result<()> {
        let len_u32 = u32::try_from(len)
            .map_err(|_| Error::Corrupted(format!("{label} length exceeds u32::MAX")))?;
        self.write_u32(len_u32);
        Ok(())
    }

    fn write_str(&mut self, s: &str, label: &str) -> Result<()> {
        self.write_len(s.len(), label)?;
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }

    fn write_opt_str(&mut self, s: Option<&str>, label: &str) -> Result<()> {
        match s {
            Some(v) => {
                self.write_u8(1);
                self.write_str(v, label)?;
            }
            None => self.write_u8(0),
        }
        Ok(())
    }

    fn write_opt_u64(&mut self, v: Option<u64>) {
        match v {
            Some(n) => {
                self.write_u8(1);
                self.write_u64(n);
            }
            None => self.write_u8(0),
        }
    }

    fn write_f32_slice(&mut self, slice: &[f32], label: &str) -> Result<()> {
        self.write_len(slice.len(), label)?;
        for &val in slice {
            self.write_f32(val);
        }
        Ok(())
    }
}

struct BinReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BinReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize, label: &str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(Error::Corrupted(format!(
                "Truncated binary payload reading {label}"
            )));
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_u8(&mut self, label: &str) -> Result<u8> {
        Ok(self.take(1, label)?[0])
    }

    fn read_bool_flag(&mut self, label: &str) -> Result<bool> {
        match self.read_u8(label)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(Error::Corrupted(format!(
                "Invalid boolean flag {other} for {label}"
            ))),
        }
    }

    fn read_u32(&mut self, label: &str) -> Result<u32> {
        let s = self.take(4, label)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn read_u64(&mut self, label: &str) -> Result<u64> {
        let s = self.take(8, label)?;
        Ok(u64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }

    fn read_f32(&mut self, label: &str) -> Result<f32> {
        let s = self.take(4, label)?;
        Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn read_count(&mut self, min_item_bytes: usize, label: &str) -> Result<usize> {
        let count = self.read_u32(label)? as usize;
        let max_possible = self
            .remaining()
            .checked_div(min_item_bytes)
            .unwrap_or_else(|| self.remaining());
        if count > max_possible {
            return Err(Error::Corrupted(format!(
                "Invalid count {count} for {label} exceeds remaining payload"
            )));
        }
        Ok(count)
    }

    fn read_str(&mut self, label: &str) -> Result<String> {
        let len = self.read_u32(label)? as usize;
        let raw = self.take(len, label)?;
        std::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| Error::Corrupted(format!("Invalid UTF-8 in {label}")))
    }

    fn read_opt_str(&mut self, label: &str) -> Result<Option<String>> {
        if self.read_bool_flag(label)? {
            Ok(Some(self.read_str(label)?))
        } else {
            Ok(None)
        }
    }

    fn read_opt_u64(&mut self, label: &str) -> Result<Option<u64>> {
        if self.read_bool_flag(label)? {
            Ok(Some(self.read_u64(label)?))
        } else {
            Ok(None)
        }
    }

    fn read_f32_vec(&mut self, label: &str) -> Result<Vec<f32>> {
        let len = self.read_count(4, label)?;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_f32(label)?);
        }
        Ok(out)
    }

    fn ensure_eof(&self, label: &str) -> Result<()> {
        if self.pos != self.bytes.len() {
            return Err(Error::Corrupted(format!(
                "Trailing {} bytes in {label}",
                self.bytes.len() - self.pos
            )));
        }
        Ok(())
    }
}

/// Encode a `MemoryEntry` into tapirus's compact binary format (`TPMEM001`).
pub(crate) fn encode_memory_entry_binary(entry: &MemoryEntry) -> Result<Vec<u8>> {
    let vec_bytes = entry.vector.as_ref().map_or(0, |v| v.len() * 4);
    let mut w = BinWriter::with_capacity(128 + entry.content.len() + vec_bytes);
    w.write_bytes_raw(MEMORY_ENTRY_BIN_MAGIC);
    w.write_u64(entry.id);
    w.write_str(&entry.content, "content")?;
    match &entry.vector {
        Some(v) => {
            w.write_u8(1);
            w.write_f32_slice(v, "vector")?;
        }
        None => w.write_u8(0),
    }
    w.write_u64(entry.timestamp);
    w.write_u64(entry.last_accessed);
    w.write_f32(entry.importance);
    w.write_u32(entry.access_count);
    w.write_len(entry.tags.len(), "tags")?;
    for tag in &entry.tags {
        w.write_str(tag, "tag")?;
    }
    w.write_len(entry.associations.len(), "associations")?;
    for &assoc in &entry.associations {
        w.write_u64(assoc);
    }
    w.write_opt_str(entry.namespace.as_deref(), "namespace")?;
    w.write_opt_str(entry.session_id.as_deref(), "session_id")?;
    w.write_opt_str(entry.external_id.as_deref(), "external_id")?;
    w.write_opt_str(entry.source_version.as_deref(), "source_version")?;
    w.write_opt_str(entry.embedding_model.as_deref(), "embedding_model")?;
    w.write_opt_u64(entry.expires_at);
    w.write_u64(entry.revision);
    w.write_u64(entry.updated_at);
    w.write_str(&entry.content_hash, "content_hash")?;
    Ok(w.buf)
}

/// Decode a `MemoryEntry` serialized with [`encode_memory_entry_binary`].
pub(crate) fn decode_memory_entry_binary(bytes: &[u8]) -> Result<MemoryEntry> {
    let mut r = BinReader::new(bytes);
    let magic = r.take(MEMORY_ENTRY_BIN_MAGIC.len(), "magic")?;
    if magic != MEMORY_ENTRY_BIN_MAGIC {
        return Err(Error::Corrupted("Invalid binary memory entry magic".into()));
    }
    let id = r.read_u64("id")?;
    let content = r.read_str("content")?;
    let vector = if r.read_bool_flag("has_vector")? {
        Some(r.read_f32_vec("vector")?)
    } else {
        None
    };
    let timestamp = r.read_u64("timestamp")?;
    let last_accessed = r.read_u64("last_accessed")?;
    let importance = r.read_f32("importance")?;
    let access_count = r.read_u32("access_count")?;

    let tag_count = r.read_count(4, "tags")?;
    let mut tags = Vec::with_capacity(tag_count);
    for _ in 0..tag_count {
        tags.push(r.read_str("tag")?);
    }

    let assoc_count = r.read_count(8, "associations")?;
    let mut associations = Vec::with_capacity(assoc_count);
    for _ in 0..assoc_count {
        associations.push(r.read_u64("association")?);
    }

    let namespace = r.read_opt_str("namespace")?;
    let session_id = r.read_opt_str("session_id")?;
    let external_id = r.read_opt_str("external_id")?;
    let source_version = r.read_opt_str("source_version")?;
    let embedding_model = r.read_opt_str("embedding_model")?;
    let expires_at = r.read_opt_u64("expires_at")?;
    let revision = r.read_u64("revision")?;
    let updated_at = r.read_u64("updated_at")?;
    let content_hash = r.read_str("content_hash")?;
    r.ensure_eof("memory entry")?;

    Ok(MemoryEntry {
        id,
        content,
        vector,
        timestamp,
        last_accessed,
        importance,
        access_count,
        tags,
        associations,
        namespace,
        session_id,
        external_id,
        source_version,
        embedding_model,
        expires_at,
        revision,
        updated_at,
        content_hash,
    })
}

/// Decode a `MemoryEntry` supporting both compact binary (`TPMEM001`) and legacy JSON records.
pub(crate) fn decode_memory_entry_auto(id: u64, bytes: &[u8]) -> Result<MemoryEntry> {
    if bytes.starts_with(MEMORY_ENTRY_BIN_MAGIC) {
        decode_memory_entry_binary(bytes)
    } else {
        serde_json::from_slice::<MemoryEntry>(bytes)
            .map_err(|e| Error::Corrupted(format!("Invalid memory record {id}: {e}")))
    }
}

fn encode_metric(metric: DistanceMetric) -> u8 {
    match metric {
        DistanceMetric::Cosine => 0,
        DistanceMetric::Euclidean => 1,
        DistanceMetric::DotProduct => 2,
    }
}

fn decode_metric(v: u8) -> Result<DistanceMetric> {
    match v {
        0 => Ok(DistanceMetric::Cosine),
        1 => Ok(DistanceMetric::Euclidean),
        2 => Ok(DistanceMetric::DotProduct),
        _ => Err(Error::Corrupted(format!(
            "Unknown HNSW distance metric {v}"
        ))),
    }
}

/// Encode cached AI Memory HNSW indexes in compact binary (`TPIDX001` + trailing CRC32).
///
/// Nodes belonging to active documents omit duplicate `f32` vectors and rehydrate
/// them directly from [`MemoryEntry::vector`] during reload.
pub(crate) fn encode_cached_memory_indexes(cache: &CachedMemoryIndexes) -> Result<Vec<u8>> {
    let mut w = BinWriter::with_capacity(256 + cache.spaces.len() * 1024);
    w.write_bytes_raw(MEMORY_INDEX_BIN_MAGIC);
    w.write_len(cache.hnsw_m, "hnsw_m")?;
    w.write_len(cache.ef_construction, "ef_construction")?;
    w.write_len(cache.ef_search, "ef_search")?;
    w.write_len(cache.spaces.len(), "spaces")?;

    for space in &cache.spaces {
        w.write_opt_str(space.model.as_deref(), "space_model")?;
        w.write_len(space.dims, "dims")?;
        w.write_u64(space.next_node);

        w.write_len(space.documents.len(), "documents")?;
        let mut live_nodes = std::collections::HashSet::with_capacity(space.documents.len());
        for &(node_id, doc_id, fp) in &space.documents {
            w.write_u64(node_id);
            w.write_u64(doc_id);
            w.write_u64(fp);
            live_nodes.insert(node_id);
        }

        let snap = &space.snapshot;
        w.write_len(snap.dimensions, "snap_dimensions")?;
        w.write_u8(encode_metric(snap.metric));
        w.write_len(snap.config.m, "config.m")?;
        w.write_len(snap.config.m0, "config.m0")?;
        w.write_len(snap.config.ef_construction, "config.ef_construction")?;
        w.write_len(snap.config.ef_search, "config.ef_search")?;
        w.write_u8(u8::from(snap.config.quantize_sq8));
        w.write_opt_u64(snap.entry_point);
        w.write_len(snap.max_level, "max_level")?;

        w.write_len(snap.tombstones.len(), "tombstones")?;
        for &node_id in &snap.tombstones {
            w.write_u64(node_id);
        }

        w.write_len(snap.nodes.len(), "nodes")?;
        for node in &snap.nodes {
            w.write_u64(node.id);
            w.write_len(node.top_layer, "top_layer")?;
            w.write_len(node.layer_neighbors.len(), "layer_neighbors")?;
            for neighbors in &node.layer_neighbors {
                w.write_len(neighbors.len(), "neighbors")?;
                for &nbr in neighbors {
                    w.write_u64(nbr);
                }
            }
            // Live nodes recover vectors straight from MemoryEntry.vector on reload.
            if live_nodes.contains(&node.id) {
                w.write_u8(0);
            } else {
                w.write_u8(1);
                w.write_f32_slice(&node.vector, "tombstone_vector")?;
            }
            match &node.quantized {
                Some(q) => {
                    w.write_u8(1);
                    w.write_len(q.quantized.len(), "quantized")?;
                    w.write_bytes_raw(&q.quantized);
                    w.write_f32(q.min_val);
                    w.write_f32(q.max_val);
                    w.write_f32(q.step);
                }
                None => w.write_u8(0),
            }
        }
    }

    let crc = crc32fast::hash(&w.buf);
    w.write_u32(crc);
    Ok(w.buf)
}

/// Decode and CRC-verify cached AI Memory HNSW indexes encoded with [`encode_cached_memory_indexes`].
pub(crate) fn decode_cached_memory_indexes(bytes: &[u8]) -> Result<CachedMemoryIndexes> {
    if bytes.len() < MEMORY_INDEX_BIN_MAGIC.len() + 4 {
        return Err(Error::Corrupted("Truncated memory index cache".into()));
    }
    let (body, crc_slice) = bytes.split_at(bytes.len() - 4);
    let expected_crc = u32::from_le_bytes([crc_slice[0], crc_slice[1], crc_slice[2], crc_slice[3]]);
    if crc32fast::hash(body) != expected_crc {
        return Err(Error::Corrupted(
            "Memory index cache checksum mismatch".into(),
        ));
    }

    let mut r = BinReader::new(body);
    let magic = r.take(MEMORY_INDEX_BIN_MAGIC.len(), "index_magic")?;
    if magic != MEMORY_INDEX_BIN_MAGIC {
        return Err(Error::Corrupted("Invalid memory index cache magic".into()));
    }
    let hnsw_m = r.read_u32("hnsw_m")? as usize;
    let ef_construction = r.read_u32("ef_construction")? as usize;
    let ef_search = r.read_u32("ef_search")? as usize;
    let space_count = r.read_count(16, "spaces")?;
    let mut spaces = Vec::with_capacity(space_count);

    for _ in 0..space_count {
        let model = r.read_opt_str("space_model")?;
        let dims = r.read_u32("dims")? as usize;
        let next_node = r.read_u64("next_node")?;

        let doc_count = r.read_count(24, "documents")?;
        let mut documents = Vec::with_capacity(doc_count);
        for _ in 0..doc_count {
            let node_id = r.read_u64("node_id")?;
            let doc_id = r.read_u64("doc_id")?;
            let fp = r.read_u64("doc_fp")?;
            documents.push((node_id, doc_id, fp));
        }

        let dimensions = r.read_u32("snap_dimensions")? as usize;
        let metric = decode_metric(r.read_u8("metric")?)?;
        let cfg_m = r.read_u32("config.m")? as usize;
        let cfg_m0 = r.read_u32("config.m0")? as usize;
        let cfg_efc = r.read_u32("config.ef_construction")? as usize;
        let cfg_efs = r.read_u32("config.ef_search")? as usize;
        let quantize_sq8 = r.read_bool_flag("quantize_sq8")?;
        let entry_point = r.read_opt_u64("entry_point")?;
        let max_level = r.read_u32("max_level")? as usize;

        let tomb_count = r.read_count(8, "tombstones")?;
        let mut tombstones = Vec::with_capacity(tomb_count);
        for _ in 0..tomb_count {
            tombstones.push(r.read_u64("tombstone")?);
        }

        let node_count = r.read_count(16, "nodes")?;
        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let id = r.read_u64("node.id")?;
            let top_layer = r.read_u32("node.top_layer")? as usize;
            let layer_count = r.read_count(4, "node.layers")?;
            let mut layer_neighbors = Vec::with_capacity(layer_count);
            for _ in 0..layer_count {
                let nbr_count = r.read_count(8, "node.neighbors")?;
                let mut nbrs = Vec::with_capacity(nbr_count);
                for _ in 0..nbr_count {
                    nbrs.push(r.read_u64("nbr")?);
                }
                layer_neighbors.push(nbrs);
            }
            let vector = if r.read_bool_flag("has_inline_vector")? {
                r.read_f32_vec("tombstone_vector")?
            } else {
                Vec::new()
            };
            let quantized = if r.read_bool_flag("has_quantized")? {
                let qlen = r.read_count(1, "quantized")?;
                let qbytes = r.take(qlen, "quantized_bytes")?.to_vec();
                let min_val = r.read_f32("min_val")?;
                let max_val = r.read_f32("max_val")?;
                let step = r.read_f32("step")?;
                Some(QuantizedVector8 {
                    quantized: qbytes,
                    min_val,
                    max_val,
                    step,
                })
            } else {
                None
            };
            nodes.push(HnswNode {
                id,
                vector,
                top_layer,
                layer_neighbors,
                quantized,
            });
        }

        spaces.push(CachedVectorSpace {
            model,
            dims,
            next_node,
            documents,
            snapshot: HnswSnapshot {
                dimensions,
                metric,
                config: HnswConfig {
                    m: cfg_m,
                    m0: cfg_m0,
                    ef_construction: cfg_efc,
                    ef_search: cfg_efs,
                    quantize_sq8,
                },
                entry_point,
                max_level,
                nodes,
                tombstones,
            },
        });
    }

    r.ensure_eof("memory index cache")?;
    Ok(CachedMemoryIndexes {
        hnsw_m,
        ef_construction,
        ef_search,
        spaces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> MemoryEntry {
        MemoryEntry {
            id: 42,
            content: "User prefers dark theme and Bahasa Melayu responses.".into(),
            vector: Some(vec![0.125, -0.5, 0.75, 1.0]),
            timestamp: 1_700_000_000,
            last_accessed: 1_700_000_123,
            importance: 0.85,
            access_count: 7,
            tags: vec!["pref".into(), "ui".into()],
            associations: vec![1, 9],
            namespace: Some("agent_alpha".into()),
            session_id: Some("sess_9".into()),
            external_id: Some("doc-42".into()),
            source_version: Some("v2".into()),
            embedding_model: Some("bge-small".into()),
            expires_at: Some(1_800_000_000),
            revision: 3,
            updated_at: 1_700_000_100,
            content_hash: "abc123".into(),
        }
    }

    #[test]
    fn binary_memory_entry_roundtrips_and_beats_json_size() {
        let mut entry = sample_entry();
        entry.vector = Some((0..384).map(|i| (i as f32) * 0.001_37).collect());
        let bin = encode_memory_entry_binary(&entry).unwrap();
        let json = serde_json::to_vec(&entry).unwrap();
        assert!(bin.len() * 2 < json.len());

        let decoded = decode_memory_entry_auto(entry.id, &bin).unwrap();
        assert_eq!(decoded, entry);

        // Legacy JSON still decodes transparently
        let decoded_legacy = decode_memory_entry_auto(entry.id, &json).unwrap();
        assert_eq!(decoded_legacy, entry);
    }

    #[test]
    fn corrupted_binary_payload_returns_error_without_panic() {
        let entry = sample_entry();
        let bin = encode_memory_entry_binary(&entry).unwrap();
        assert!(decode_memory_entry_binary(&bin[..bin.len() - 3]).is_err());

        let mut bad_tag_count = bin.clone();
        // Overwrite magic check still ok; corrupt last byte
        let last = bad_tag_count.len() - 1;
        bad_tag_count[last] = 0xFF;
        // Corrupting trailing UTF-8 hash bytes returns Corrupted cleanly
        assert!(decode_memory_entry_binary(&bad_tag_count).is_err());
    }

    #[test]
    fn cached_index_binary_rejects_bit_flip() {
        let cache = CachedMemoryIndexes {
            hnsw_m: 16,
            ef_construction: 64,
            ef_search: 32,
            spaces: vec![CachedVectorSpace {
                model: Some("bge".into()),
                dims: 2,
                next_node: 2,
                documents: vec![(1, 42, 999)],
                snapshot: HnswSnapshot {
                    dimensions: 2,
                    metric: DistanceMetric::Cosine,
                    config: HnswConfig::default(),
                    entry_point: Some(1),
                    max_level: 0,
                    nodes: vec![HnswNode {
                        id: 1,
                        vector: vec![],
                        top_layer: 0,
                        layer_neighbors: vec![vec![]],
                        quantized: None,
                    }],
                    tombstones: vec![],
                },
            }],
        };
        let mut encoded = encode_cached_memory_indexes(&cache).unwrap();
        let decoded = decode_cached_memory_indexes(&encoded).unwrap();
        assert_eq!(decoded.spaces.len(), 1);
        assert_eq!(decoded.spaces[0].documents, vec![(1, 42, 999)]);

        encoded[12] ^= 0x01;
        assert!(decode_cached_memory_indexes(&encoded).is_err());
    }
}
