//! Continuous Streaming WAL Replication Engine for TapirusDB.
//!
//! Provides frame-level delta streaming replication from edge/primary nodes
//! to cloud replicas, remote standbys, and disaster recovery stores with
//! hardware-accelerated CRC32 verification and monotonic LSN sequence tracking.

use crate::error::{Error, Result};
use crate::pager::{PageId, Pager};
use crc32fast::Hasher;
use serde::{Deserialize, Serialize};

/// A single replicated WAL frame carrying page payload and integrity checksum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalReplicationFrame {
    /// Database Page ID (1-based)
    pub page_id: PageId,
    /// Total database size in pages after this frame was written
    pub db_size_in_pages: u32,
    /// Primary commit sequence number
    pub commit_seq: u32,
    /// Hardware-accelerated CRC32 checksum over the page frame payload
    pub crc32: u32,
    /// Raw unencrypted/decrypted page binary frame data
    pub data: Vec<u8>,
}

impl WalReplicationFrame {
    /// Verify that the stored CRC32 matches the calculated checksum of the page payload
    pub fn verify(&self) -> Result<()> {
        let mut hasher = Hasher::new();
        hasher.update(&self.page_id.to_le_bytes());
        hasher.update(&self.db_size_in_pages.to_le_bytes());
        hasher.update(&self.commit_seq.to_le_bytes());
        hasher.update(&self.data);
        let calculated = hasher.finalize();

        if self.crc32 != calculated {
            return Err(Error::PageCorrupted(self.page_id, self.crc32, calculated));
        }
        Ok(())
    }
}

/// An atomic streaming replication batch containing zero or more validated WAL frames
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalReplicationChunk {
    /// Monotonically increasing stream batch sequence number
    pub sequence_number: u64,
    /// Primary active checkpoint sequence number
    pub checkpoint_seq: u32,
    /// Primary total database size in pages at chunk snapshot time
    pub total_pages: u32,
    /// Number of frames packaged in this chunk
    pub frame_count: u32,
    /// Replicated frame list
    pub frames: Vec<WalReplicationFrame>,
    /// Global CRC32 checksum protecting the entire batch
    pub chunk_crc32: u32,
}

const REPLICATION_WIRE_MAGIC: &[u8; 4] = b"TWR1";

impl WalReplicationChunk {
    /// Create a new replication chunk from a list of frames
    pub fn new(
        sequence_number: u64,
        checkpoint_seq: u32,
        total_pages: u32,
        frames: Vec<WalReplicationFrame>,
    ) -> Self {
        let frame_count = frames.len() as u32;
        let mut chunk = Self {
            sequence_number,
            checkpoint_seq,
            total_pages,
            frame_count,
            frames,
            chunk_crc32: 0,
        };
        chunk.chunk_crc32 = chunk.calculate_crc();
        chunk
    }

    /// Calculate aggregate CRC32 checksum across chunk metadata and all contained frames
    pub fn calculate_crc(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(&self.sequence_number.to_le_bytes());
        hasher.update(&self.checkpoint_seq.to_le_bytes());
        hasher.update(&self.total_pages.to_le_bytes());
        hasher.update(&self.frame_count.to_le_bytes());
        for frame in &self.frames {
            hasher.update(&frame.page_id.to_le_bytes());
            hasher.update(&frame.crc32.to_le_bytes());
        }
        hasher.finalize()
    }

    /// Verify chunk-level and frame-level data integrity
    pub fn verify(&self) -> Result<()> {
        if self.frame_count as usize != self.frames.len() {
            return Err(Error::Corrupted("Replication frame count mismatch".into()));
        }
        let expected_crc = self.calculate_crc();
        if self.chunk_crc32 != expected_crc {
            return Err(Error::Corrupted(format!(
                "Replication chunk CRC mismatch: expected {expected_crc:#x}, found {:#x}",
                self.chunk_crc32
            )));
        }
        for frame in &self.frames {
            frame.verify()?;
        }
        Ok(())
    }

    /// Serialize chunk into a compact binary wire frame (`TWR1`) without JSON byte-array bloat.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let payload_bytes: usize = self.frames.iter().map(|f| 20 + f.data.len()).sum();
        let mut out = Vec::with_capacity(28 + payload_bytes);
        out.extend_from_slice(REPLICATION_WIRE_MAGIC);
        out.extend_from_slice(&self.sequence_number.to_le_bytes());
        out.extend_from_slice(&self.checkpoint_seq.to_le_bytes());
        out.extend_from_slice(&self.total_pages.to_le_bytes());
        out.extend_from_slice(&self.frame_count.to_le_bytes());
        out.extend_from_slice(&self.chunk_crc32.to_le_bytes());

        for frame in &self.frames {
            let data_len = u32::try_from(frame.data.len()).map_err(|_| {
                Error::Corrupted("Replication frame exceeds 4 GiB wire limit".into())
            })?;
            out.extend_from_slice(&frame.page_id.to_le_bytes());
            out.extend_from_slice(&frame.db_size_in_pages.to_le_bytes());
            out.extend_from_slice(&frame.commit_seq.to_le_bytes());
            out.extend_from_slice(&frame.crc32.to_le_bytes());
            out.extend_from_slice(&data_len.to_le_bytes());
            out.extend_from_slice(&frame.data);
        }
        Ok(out)
    }

    /// Deserialize chunk from compact binary (`TWR1`) or legacy JSON wire bytes and verify integrity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let chunk = if bytes.starts_with(REPLICATION_WIRE_MAGIC) {
            Self::decode_binary(bytes)?
        } else {
            serde_json::from_slice(bytes).map_err(|e| {
                Error::Corrupted(format!("Failed to deserialize replication chunk: {e}"))
            })?
        };
        chunk.verify()?;
        Ok(chunk)
    }

    fn decode_binary(bytes: &[u8]) -> Result<Self> {
        let corrupt =
            |msg: &str| Error::Corrupted(format!("Replication wire frame corrupted: {msg}"));
        if bytes.len() < 28 {
            return Err(corrupt("truncated chunk header"));
        }
        let read_u32 = |off: usize| -> Result<u32> {
            let s = bytes
                .get(off..off + 4)
                .ok_or_else(|| corrupt("unexpected EOF"))?;
            Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let sequence_number = u64::from_le_bytes(
            bytes[4..12]
                .try_into()
                .map_err(|_| corrupt("truncated sequence_number"))?,
        );
        let checkpoint_seq = read_u32(12)?;
        let total_pages = read_u32(16)?;
        let frame_count = read_u32(20)?;
        let chunk_crc32 = read_u32(24)?;

        let max_possible = (bytes.len() - 28) / 20;
        if (frame_count as usize) > max_possible {
            return Err(corrupt("frame_count exceeds payload size"));
        }

        let mut pos = 28usize;
        let mut frames = Vec::with_capacity(frame_count as usize);
        for _ in 0..frame_count {
            if pos + 20 > bytes.len() {
                return Err(corrupt("truncated frame header"));
            }
            let page_id = read_u32(pos)?;
            let db_size_in_pages = read_u32(pos + 4)?;
            let commit_seq = read_u32(pos + 8)?;
            let crc32 = read_u32(pos + 12)?;
            let data_len = read_u32(pos + 16)? as usize;
            pos += 20;
            let end = pos
                .checked_add(data_len)
                .ok_or_else(|| corrupt("frame length overflow"))?;
            let data = bytes
                .get(pos..end)
                .ok_or_else(|| corrupt("truncated frame payload"))?
                .to_vec();
            pos = end;
            frames.push(WalReplicationFrame {
                page_id,
                db_size_in_pages,
                commit_seq,
                crc32,
                data,
            });
        }
        if pos != bytes.len() {
            return Err(corrupt("trailing bytes after replication frames"));
        }

        Ok(Self {
            sequence_number,
            checkpoint_seq,
            total_pages,
            frame_count,
            frames,
            chunk_crc32,
        })
    }
}

/// Continuous WAL Replication Stream Producer
#[derive(Debug, Default, Clone)]
pub struct WalReplicationStream {
    last_seq: u64,
}

impl WalReplicationStream {
    /// Create a new replication stream starting at sequence 0
    pub fn new() -> Self {
        Self { last_seq: 0 }
    }

    /// Export a replication chunk containing all modified frames from the primary Pager
    pub fn export_chunk(&mut self, pager: &Pager) -> Result<WalReplicationChunk> {
        pager.require_committed_snapshot()?;
        self.last_seq = self
            .last_seq
            .checked_add(1)
            .ok_or_else(|| Error::TransactionError("Replication sequence exhausted".into()))?
            .max(u64::from(pager.header().change_counter));
        let seq = self.last_seq;
        let total_pages = pager.header().total_pages;

        let mut frames = Vec::new();

        // Export a complete committed snapshot, including evicted pages.
        for (pid, data) in pager.committed_pages()? {
            let mut hasher = Hasher::new();
            hasher.update(&pid.to_le_bytes());
            hasher.update(&total_pages.to_le_bytes());
            hasher.update(&(seq as u32).to_le_bytes());
            hasher.update(&data);
            let crc32 = hasher.finalize();

            frames.push(WalReplicationFrame {
                page_id: pid,
                db_size_in_pages: total_pages,
                commit_seq: seq as u32,
                crc32,
                data,
            });
        }

        // Sort frames by page_id for deterministic replication order
        frames.sort_unstable_by_key(|f| f.page_id);

        Ok(WalReplicationChunk::new(
            seq,
            pager.header().schema_cookie,
            total_pages,
            frames,
        ))
    }

    /// Create a replication chunk from current pager state at given sequence number
    pub fn create_chunk(pager: &Pager, seq: u64) -> Result<WalReplicationChunk> {
        let mut stream = Self {
            last_seq: seq.saturating_sub(1),
        };
        stream.export_chunk(pager)
    }
}

/// Continuous WAL Replication Receiver / Standby Consumer
#[derive(Debug, Default, Clone)]
pub struct WalReplicationReceiver {
    applied_seq: u64,
}

impl WalReplicationReceiver {
    /// Create a new replication receiver
    pub fn new() -> Self {
        Self { applied_seq: 0 }
    }

    /// Apply a validated replication chunk into a replica standby Pager
    pub fn apply_chunk(&mut self, pager: &mut Pager, chunk: &WalReplicationChunk) -> Result<usize> {
        // Enforce cryptographic integrity verification before touching storage
        chunk.verify()?;
        pager.require_committed_snapshot()?;
        let last = self.applied_seq.max(u64::from(pager.header().wal_sequence));
        if chunk.sequence_number <= last || chunk.sequence_number > u64::from(u32::MAX) {
            return Err(Error::TransactionError(
                "Replication sequence is duplicate, stale, or out of range".into(),
            ));
        }
        let num_frames = chunk.frames.len();
        let mut ids = std::collections::HashSet::new();
        if num_frames != chunk.total_pages as usize || chunk.total_pages == 0 {
            return Err(Error::Corrupted(
                "Replication requires a complete snapshot".into(),
            ));
        }
        for frame in &chunk.frames {
            if frame.page_id == 0
                || frame.page_id > chunk.total_pages
                || frame.data.len() != pager.page_size()
                || !ids.insert(frame.page_id)
            {
                return Err(Error::Corrupted("Invalid replication page layout".into()));
            }
        }
        let first = chunk
            .frames
            .iter()
            .find(|frame| frame.page_id == 1)
            .ok_or_else(|| Error::Corrupted("Replication snapshot lacks page 1".into()))?;
        let mut header = super::DatabaseHeader::from_bytes(&first.data)?;
        if header.page_size != pager.header().page_size || header.total_pages != chunk.total_pages {
            return Err(Error::Corrupted(
                "Replication header does not match snapshot".into(),
            ));
        }
        // Payloads are plaintext; encrypt with the receiver's credentials on commit.
        header.encryption_flags = pager.header().encryption_flags;
        header.salt = pager.header().salt;
        header.kcv = pager.header().kcv;
        header.compression_flags = pager.header().compression_flags;
        header.wal_sequence = chunk.sequence_number as u32;
        pager.begin_transaction()?;
        *pager.header_mut() = header;
        let result = (|| {
            for frame in &chunk.frames {
                pager.write_page(frame.page_id, &frame.data)?;
            }
            pager.commit_transaction()
        })();
        if let Err(error) = result {
            if pager.is_in_transaction() {
                pager.rollback_transaction()?;
            }
            return Err(error);
        }
        self.applied_seq = chunk.sequence_number;
        Ok(num_frames)
    }

    /// Return the last applied replication sequence number
    pub fn last_applied_sequence(&self) -> u64 {
        self.applied_seq
    }

    /// Apply a validated chunk into a replica pager
    pub fn apply_to_pager(pager: &mut Pager, chunk: &WalReplicationChunk) -> Result<usize> {
        let mut receiver = Self::new();
        receiver.apply_chunk(pager, chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_wire_roundtrip_and_legacy_json_compat() {
        let data = vec![0xAB; 4096];
        let mut hasher = Hasher::new();
        hasher.update(&1u32.to_le_bytes());
        hasher.update(&1u32.to_le_bytes());
        hasher.update(&1u32.to_le_bytes());
        hasher.update(&data);
        let frame = WalReplicationFrame {
            page_id: 1,
            db_size_in_pages: 1,
            commit_seq: 1,
            crc32: hasher.finalize(),
            data,
        };
        let chunk = WalReplicationChunk::new(1, 0, 1, vec![frame]);
        let bin = chunk.to_bytes().expect("encode binary");
        assert!(bin.starts_with(b"TWR1"));
        // 4KB binary frame should stay close to 4KB on wire, vs >10KB in JSON.
        assert!(bin.len() < 4200, "unexpected wire bloat: {}", bin.len());

        let decoded_bin = WalReplicationChunk::from_bytes(&bin).expect("decode binary");
        assert_eq!(decoded_bin, chunk);

        let json_legacy = serde_json::to_vec(&chunk).expect("encode legacy json");
        let decoded_json =
            WalReplicationChunk::from_bytes(&json_legacy).expect("decode legacy json");
        assert_eq!(decoded_json, chunk);
    }
}
