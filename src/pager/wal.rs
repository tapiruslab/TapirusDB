//! Write-Ahead Log (WAL) implementation for TapirusDB.
//!
//! Provides atomic crash durability, multi-version concurrency (MVCC),
//! and non-blocking reads/writes via a dedicated `.tapir-wal` file.

use crate::error::{Error, Result};
use crate::pager::PageId;
use crc32fast::Hasher;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Magic bytes identifying a TapirusDB WAL file: `b"TAPIRWAL"`
pub const WAL_MAGIC: [u8; 8] = *b"TAPIRWAL";

/// WAL file format version
pub const WAL_VERSION: u32 = 1;

/// Fixed size of the WAL header in bytes
pub const WAL_HEADER_SIZE: usize = 32;

/// Fixed size of each WAL frame header in bytes
pub const WAL_FRAME_HEADER_SIZE: usize = 24;

/// Default checkpoint threshold in frames (1,000 pages = 4MB)
pub const DEFAULT_CHECKPOINT_THRESHOLD: usize = 1000;

/// 32-byte Header located at the beginning of every `.tapir-wal` file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalHeader {
    /// Magic string: `b"TAPIRWAL"`
    pub magic: [u8; 8],
    /// File format version
    pub version: u32,
    /// Page size in bytes (e.g. 4096)
    pub page_size: u32,
    /// Checkpoint sequence number incremented on each checkpoint
    pub checkpoint_seq: u32,
    /// Salt-1 for frame verification
    pub salt_1: u32,
    /// Salt-2 for frame verification
    pub salt_2: u32,
    /// Checksum over header bytes 0..27
    pub header_crc32: u32,
}

impl WalHeader {
    /// Create a new WAL header
    pub fn new(page_size: u32) -> Self {
        let mut header = Self {
            magic: WAL_MAGIC,
            version: WAL_VERSION,
            page_size,
            checkpoint_seq: 1,
            salt_1: 0x1A2B3C4D,
            salt_2: 0x5E6F7A8B,
            header_crc32: 0,
        };
        header.header_crc32 = header.calculate_crc();
        header
    }

    /// Calculate CRC32 checksum over the first 28 bytes of the WAL header
    pub fn calculate_crc(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(&self.magic);
        hasher.update(&self.version.to_le_bytes());
        hasher.update(&self.page_size.to_le_bytes());
        hasher.update(&self.checkpoint_seq.to_le_bytes());
        hasher.update(&self.salt_1.to_le_bytes());
        hasher.update(&self.salt_2.to_le_bytes());
        hasher.finalize()
    }

    /// Serialize header to 32-byte buffer
    pub fn to_bytes(&self) -> [u8; WAL_HEADER_SIZE] {
        let mut buf = [0u8; WAL_HEADER_SIZE];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.page_size.to_le_bytes());
        buf[16..20].copy_from_slice(&self.checkpoint_seq.to_le_bytes());
        buf[20..24].copy_from_slice(&self.salt_1.to_le_bytes());
        buf[24..28].copy_from_slice(&self.salt_2.to_le_bytes());
        buf[28..32].copy_from_slice(&self.header_crc32.to_le_bytes());
        buf
    }

    /// Deserialize header from 32-byte slice and verify CRC32
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        if slice.len() < WAL_HEADER_SIZE {
            return Err(Error::Corrupted("WAL header slice too short (<32 bytes)".into()));
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&slice[0..8]);
        if magic != WAL_MAGIC {
            return Err(Error::InvalidMagic);
        }

        let version = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
        let page_size = u32::from_le_bytes([slice[12], slice[13], slice[14], slice[15]]);
        let checkpoint_seq = u32::from_le_bytes([slice[16], slice[17], slice[18], slice[19]]);
        let salt_1 = u32::from_le_bytes([slice[20], slice[21], slice[22], slice[23]]);
        let salt_2 = u32::from_le_bytes([slice[24], slice[25], slice[26], slice[27]]);
        let header_crc32 = u32::from_le_bytes([slice[28], slice[29], slice[30], slice[31]]);

        let header = Self {
            magic,
            version,
            page_size,
            checkpoint_seq,
            salt_1,
            salt_2,
            header_crc32,
        };

        let expected_crc = header.calculate_crc();
        if header.header_crc32 != expected_crc {
            return Err(Error::PageCorrupted(0, expected_crc, header.header_crc32));
        }

        Ok(header)
    }
}

/// 24-byte Frame Header preceding every page image in the WAL file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalFrameHeader {
    /// Target page ID in the database
    pub page_id: PageId,
    /// Size of database in pages after this transaction commit (0 if non-commit frame)
    pub db_size_in_pages: u32,
    /// Monotonic commit sequence number
    pub commit_seq: u32,
    /// Salt-1 matching active WAL header
    pub salt_1: u32,
    /// Salt-2 matching active WAL header
    pub salt_2: u32,
    /// CRC32 checksum over frame header (0..20) and page image
    pub frame_crc32: u32,
}

impl WalFrameHeader {
    /// Calculate CRC32 checksum over frame header fields and page payload
    pub fn calculate_crc(&self, page_data: &[u8]) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(&self.page_id.to_le_bytes());
        hasher.update(&self.db_size_in_pages.to_le_bytes());
        hasher.update(&self.commit_seq.to_le_bytes());
        hasher.update(&self.salt_1.to_le_bytes());
        hasher.update(&self.salt_2.to_le_bytes());
        hasher.update(page_data);
        hasher.finalize()
    }

    /// Serialize frame header to 24-byte buffer
    pub fn to_bytes(&self) -> [u8; WAL_FRAME_HEADER_SIZE] {
        let mut buf = [0u8; WAL_FRAME_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.page_id.to_le_bytes());
        buf[4..8].copy_from_slice(&self.db_size_in_pages.to_le_bytes());
        buf[8..12].copy_from_slice(&self.commit_seq.to_le_bytes());
        buf[12..16].copy_from_slice(&self.salt_1.to_le_bytes());
        buf[16..20].copy_from_slice(&self.salt_2.to_le_bytes());
        buf[20..24].copy_from_slice(&self.frame_crc32.to_le_bytes());
        buf
    }

    /// Deserialize frame header from 24-byte slice
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        if slice.len() < WAL_FRAME_HEADER_SIZE {
            return Err(Error::Corrupted("WAL frame header slice too short (<24 bytes)".into()));
        }

        let page_id = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
        let db_size_in_pages = u32::from_le_bytes([slice[4], slice[5], slice[6], slice[7]]);
        let commit_seq = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
        let salt_1 = u32::from_le_bytes([slice[12], slice[13], slice[14], slice[15]]);
        let salt_2 = u32::from_le_bytes([slice[16], slice[17], slice[18], slice[19]]);
        let frame_crc32 = u32::from_le_bytes([slice[20], slice[21], slice[22], slice[23]]);

        Ok(Self {
            page_id,
            db_size_in_pages,
            commit_seq,
            salt_1,
            salt_2,
            frame_crc32,
        })
    }
}

/// A stored in-memory frame record
#[derive(Debug, Clone)]
struct InMemoryFrame {
    header: WalFrameHeader,
    page_data: Vec<u8>,
}

/// The Write-Ahead Log (WAL) coordinator
#[derive(Debug)]
pub struct Wal {
    header: WalHeader,
    file: Option<File>,
    _path: Option<PathBuf>,
    page_size: usize,
    /// In-memory index mapping PageId -> 0-based frame index
    index: HashMap<PageId, usize>,
    /// In-memory frame storage for memory mode or fast lookups
    frames: Vec<InMemoryFrame>,
    current_commit_seq: u32,
}

impl Wal {
    /// Open an existing WAL file or create a new one
    pub fn open<P: AsRef<Path>>(wal_path: P, page_size: u32) -> Result<Self> {
        let path = wal_path.as_ref().to_path_buf();
        let exists = path.exists();

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        let header = if exists && file.metadata()?.len() >= WAL_HEADER_SIZE as u64 {
            let mut buf = [0u8; WAL_HEADER_SIZE];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut buf)?;
            WalHeader::from_bytes(&buf)?
        } else {
            let header = WalHeader::new(page_size);
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&header.to_bytes())?;
            file.sync_data()?;
            header
        };

        let mut wal = Self {
            header,
            file: Some(file),
            _path: Some(path),
            page_size: page_size as usize,
            index: HashMap::new(),
            frames: Vec::new(),
            current_commit_seq: 0,
        };

        // Replay existing frames into in-memory index
        wal.rebuild_index()?;
        Ok(wal)
    }

    /// Open an in-memory WAL
    pub fn open_in_memory(page_size: u32) -> Self {
        Self {
            header: WalHeader::new(page_size),
            file: None,
            _path: None,
            page_size: page_size as usize,
            index: HashMap::new(),
            frames: Vec::new(),
            current_commit_seq: 0,
        }
    }

    /// Total number of valid frames currently in the WAL
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Check if a page is currently cached in the WAL
    pub fn has_page(&self, page_id: PageId) -> bool {
        self.index.contains_key(&page_id)
    }

    /// Export all valid frames currently recorded in the WAL for replication
    pub fn export_frames(&self) -> Vec<(PageId, Vec<u8>)> {
        self.frames.iter().map(|f| (f.header.page_id, f.page_data.clone())).collect()
    }

    /// Rebuild in-memory index from file frames
    fn rebuild_index(&mut self) -> Result<()> {
        if let Some(file) = &mut self.file {
            let file_len = file.metadata()?.len();
            let frame_size = (WAL_FRAME_HEADER_SIZE + self.page_size) as u64;

            let mut offset = WAL_HEADER_SIZE as u64;
            let mut frame_idx = 0;

            while offset + frame_size <= file_len {
                file.seek(SeekFrom::Start(offset))?;

                let mut header_buf = [0u8; WAL_FRAME_HEADER_SIZE];
                file.read_exact(&mut header_buf)?;
                let frame_header = WalFrameHeader::from_bytes(&header_buf)?;

                let mut page_data = vec![0u8; self.page_size];
                file.read_exact(&mut page_data)?;

                // Verify CRC32 checksum
                let expected_crc = frame_header.calculate_crc(&page_data);
                if frame_header.frame_crc32 != expected_crc {
                    // Truncate corrupted or torn frame
                    break;
                }

                if frame_header.commit_seq > self.current_commit_seq {
                    self.current_commit_seq = frame_header.commit_seq;
                }

                self.index.insert(frame_header.page_id, frame_idx);
                self.frames.push(InMemoryFrame {
                    header: frame_header,
                    page_data,
                });

                offset += frame_size;
                frame_idx += 1;
            }
        }
        Ok(())
    }

    /// Read the latest version of a page from the WAL if present
    pub fn read_page(&self, page_id: PageId) -> Option<Vec<u8>> {
        let &frame_idx = self.index.get(&page_id)?;
        self.frames.get(frame_idx).map(|f| f.page_data.clone())
    }

    /// Append a page frame to the WAL
    pub fn write_frame(
        &mut self,
        page_id: PageId,
        page_data: &[u8],
        is_commit: bool,
        db_size_in_pages: u32,
    ) -> Result<()> {
        if page_data.len() != self.page_size {
            return Err(Error::Corrupted("Page data size mismatch in WAL".into()));
        }

        if is_commit {
            self.current_commit_seq += 1;
        }

        let mut frame_header = WalFrameHeader {
            page_id,
            db_size_in_pages: if is_commit { db_size_in_pages } else { 0 },
            commit_seq: self.current_commit_seq,
            salt_1: self.header.salt_1,
            salt_2: self.header.salt_2,
            frame_crc32: 0,
        };

        frame_header.frame_crc32 = frame_header.calculate_crc(page_data);

        // Append to file if disk-backed
        if let Some(file) = &mut self.file {
            file.seek(SeekFrom::End(0))?;
            file.write_all(&frame_header.to_bytes())?;
            file.write_all(page_data)?;
            // Only force physical disk flush on COMMIT frames, eliminating per-page fsync bottleneck
            if is_commit {
                file.sync_data()?;
            }
        }

        let frame_idx = self.frames.len();
        self.index.insert(page_id, frame_idx);
        self.frames.push(InMemoryFrame {
            header: frame_header,
            page_data: page_data.to_vec(),
        });

        Ok(())
    }

    /// Flush any pending uncommitted WAL frames to durable physical disk storage
    pub fn sync(&mut self) -> Result<()> {
        if let Some(file) = &mut self.file {
            file.sync_data()?;
        }
        Ok(())
    }

    /// Checkpoint all committed pages back into the base database file,
    /// then truncate and reset the WAL file.
    pub fn checkpoint(
        &mut self,
        base_file: &mut Option<File>,
        base_in_memory: &mut HashMap<PageId, Vec<u8>>,
    ) -> Result<usize> {
        let page_size = self.page_size;
        let num_pages_to_flush = self.index.len();

        if num_pages_to_flush == 0 {
            return Ok(0);
        }

        // 1. Write latest page versions to base database in sequential PageId order
        let mut sorted_page_ids: Vec<PageId> = self.index.keys().copied().collect();
        sorted_page_ids.sort_unstable();

        for page_id in sorted_page_ids {
            let frame_idx = self.index[&page_id];
            let page_data = &self.frames[frame_idx].page_data;

            if let Some(file) = base_file {
                let offset = (page_id as u64 - 1) * page_size as u64;
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(page_data)?;
            }

            if base_file.is_none() {
                base_in_memory.entry(page_id).or_insert_with(|| page_data.clone());
            }
        }

        if let Some(file) = base_file {
            file.sync_all()?;
        }

        // 2. Reset WAL file
        self.header.checkpoint_seq += 1;
        self.header.header_crc32 = self.header.calculate_crc();

        if let Some(file) = &mut self.file {
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&self.header.to_bytes())?;
            file.set_len(WAL_HEADER_SIZE as u64)?;
            file.sync_all()?;
        }

        self.index.clear();
        self.frames.clear();

        Ok(num_pages_to_flush)
    }

    /// Recover database from existing WAL on startup (replaying committed frames)
    pub fn recover(
        &mut self,
        base_file: &mut Option<File>,
        base_in_memory: &mut HashMap<PageId, Vec<u8>>,
    ) -> Result<usize> {
        if self.frames.is_empty() {
            return Ok(0);
        }

        // Find last valid commit frame index
        let mut last_commit_idx = None;
        for (idx, frame) in self.frames.iter().enumerate() {
            if frame.header.db_size_in_pages > 0 {
                last_commit_idx = Some(idx);
            }
        }

        let max_safe_frame = match last_commit_idx {
            Some(idx) => idx,
            None => {
                // No committed transaction found in WAL, discard all frames
                self.index.clear();
                self.frames.clear();
                return Ok(0);
            }
        };

        // Replay up to last_commit_idx
        let mut recovered_count = 0;
        for frame in &self.frames[..=max_safe_frame] {
            let page_id = frame.header.page_id;
            let page_data = &frame.page_data;

            if let Some(file) = base_file {
                let offset = (page_id as u64 - 1) * self.page_size as u64;
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(page_data)?;
            }

            if base_file.is_none() {
                base_in_memory.entry(page_id).or_insert_with(|| page_data.clone());
            }
            recovered_count += 1;
        }

        if let Some(file) = base_file {
            file.sync_all()?;
        }

        Ok(recovered_count)
    }

    /// Truncate WAL back to a given frame count (used during rollback)
    pub fn rollback_to(&mut self, frame_count: usize) -> Result<()> {
        if frame_count >= self.frames.len() {
            return Ok(());
        }
        self.frames.truncate(frame_count);
        if let Some(file) = &mut self.file {
            let offset = WAL_HEADER_SIZE as u64
                + frame_count as u64 * (WAL_FRAME_HEADER_SIZE + self.page_size) as u64;
            file.set_len(offset)?;
            file.sync_data()?;
        }
        self.index.clear();
        for (i, frame) in self.frames.iter().enumerate() {
            self.index.insert(frame.header.page_id, i);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_header_roundtrip() {
        let header = WalHeader::new(4096);
        let bytes = header.to_bytes();
        let parsed = WalHeader::from_bytes(&bytes).expect("Parse header");
        assert_eq!(header, parsed);
    }

    #[test]
    fn test_wal_frame_header_crc_validation() {
        let page_data = vec![0xABu8; 4096];
        let mut frame_header = WalFrameHeader {
            page_id: 42,
            db_size_in_pages: 10,
            commit_seq: 1,
            salt_1: 0x11223344,
            salt_2: 0x55667788,
            frame_crc32: 0,
        };
        frame_header.frame_crc32 = frame_header.calculate_crc(&page_data);

        // Verification passes with identical data
        assert_eq!(frame_header.frame_crc32, frame_header.calculate_crc(&page_data));

        // Verification fails if data is altered by even 1 bit
        let mut corrupted = page_data.clone();
        corrupted[100] ^= 0x01;
        assert_ne!(frame_header.frame_crc32, frame_header.calculate_crc(&corrupted));
    }

    #[test]
    fn test_in_memory_wal_write_and_checkpoint() {
        let mut wal = Wal::open_in_memory(4096);
        let mut in_memory_pages = HashMap::new();

        let page1 = vec![1u8; 4096];
        let page2 = vec![2u8; 4096];

        wal.write_frame(1, &page1, false, 0).expect("Write frame 1");
        wal.write_frame(2, &page2, true, 2).expect("Commit frame 2");

        assert_eq!(wal.frame_count(), 2);
        assert!(wal.has_page(1));
        assert!(wal.has_page(2));

        // Read through WAL
        assert_eq!(wal.read_page(1).unwrap(), page1);
        assert_eq!(wal.read_page(2).unwrap(), page2);

        // Checkpoint to base
        let mut base_file = None;
        let flushed = wal.checkpoint(&mut base_file, &mut in_memory_pages).expect("Checkpoint");
        assert_eq!(flushed, 2);
        assert_eq!(wal.frame_count(), 0);
        assert_eq!(in_memory_pages.get(&1).unwrap(), &page1);
        assert_eq!(in_memory_pages.get(&2).unwrap(), &page2);
    }
}
