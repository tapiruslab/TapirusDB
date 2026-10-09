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

    /// Serialize chunk to binary JSON bytes for network streaming
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::Corrupted(format!("Failed to serialize replication chunk: {e}")))
    }

    /// Deserialize chunk from network stream bytes and perform cryptographic integrity verification
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let chunk: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::Corrupted(format!("Failed to deserialize replication chunk: {e}")))?;
        chunk.verify()?;
        Ok(chunk)
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
        self.last_seq += 1;
        let seq = self.last_seq;
        let total_pages = pager.header().total_pages;

        let mut frames = Vec::new();

        // Collect all active cached in-memory pages
        for (&pid, data) in pager.in_memory_pages() {
            let mut hasher = Hasher::new();
            hasher.update(&pid.to_le_bytes());
            hasher.update(&total_pages.to_le_bytes());
            hasher.update(&(seq as u32).to_le_bytes());
            hasher.update(data);
            let crc32 = hasher.finalize();

            frames.push(WalReplicationFrame {
                page_id: pid,
                db_size_in_pages: total_pages,
                commit_seq: seq as u32,
                crc32,
                data: data.clone(),
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

        let num_frames = chunk.frames.len();

        for frame in &chunk.frames {
            pager.in_memory_pages_mut().insert(frame.page_id, frame.data.clone());
            pager.touch_lru(frame.page_id);
        }

        // Update total pages from primary
        pager.header_mut().total_pages = chunk.total_pages;
        let hdr_bytes = pager.header().to_bytes();

        // Keep page 1 header synchronized
        if let Some(p1) = pager.in_memory_pages_mut().get_mut(&1) {
            let len = hdr_bytes.len().min(p1.len());
            p1[..len].copy_from_slice(&hdr_bytes[..len]);
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
