//! Database Pager, File Header, and WAL management.
//!
//! Manages raw 4,096-byte database pages, memory-mapped or disk-backed file access,
//! write-ahead log (WAL) frame appending, page allocation, and header CRC32 validation.

pub mod compression;
pub mod remote;
pub mod wal;

use crate::error::{Error, Result};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use compression::{compress_page_frame, decompress_page_frame, COMPRESSED_PAGE_MAGIC};
pub use remote::{MockRemoteRangeStorage, RemotePager, RemoteRangeReader, RemoteStorageAdapter, S3StorageConfig};
pub use wal::{Wal, WalFrameHeader, WalHeader, DEFAULT_CHECKPOINT_THRESHOLD};

/// Default page size in bytes (4KB)
pub const DEFAULT_PAGE_SIZE: u16 = 4096;

/// Magic string bytes identifying a TapirusDB file: `b"TAPIRUS\0"`
pub const DATABASE_MAGIC: [u8; 8] = *b"TAPIRUS\0";

/// Current file format version
pub const FILE_FORMAT_VERSION: u16 = 1;

/// Database header size in bytes on Page 1
pub const DATABASE_HEADER_SIZE: usize = 100;

/// Identifier for a database page (1-indexed, Page 1 is root page)
pub type PageId = u32;

/// The 100-byte Database Header located at Offset 0..99 of Page 1
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseHeader {
    /// Magic bytes: `b"TAPIRUS\0"`
    pub magic: [u8; 8],
    /// Page size in bytes (e.g. 4096)
    pub page_size: u16,
    /// Format version
    pub version: u16,
    /// Change counter incremented on commits
    pub change_counter: u32,
    /// Total number of pages in file
    pub total_pages: u32,
    /// First page of freelist trunk
    pub freelist_trunk: u32,
    /// Number of free pages
    pub freelist_count: u32,
    /// Schema cookie incremented on DDL mutations
    pub schema_cookie: u32,
    /// User version for migrations
    pub user_version: u32,
    /// Root page of Vector Directory
    pub vector_index_page: u32,
    /// Active WAL sequence
    pub wal_sequence: u32,
    /// Checksum over header bytes
    pub header_crc32: u32,
    /// Encryption mode: 0 = Plaintext, 1 = ChaCha20-Poly1305 AEAD
    pub encryption_flags: u8,
    /// 16-byte random Database Salt
    pub salt: [u8; 16],
    /// 16-byte Key Check Value (KCV) tag for key authentication
    pub kcv: [u8; 16],
    /// Transparent Page Compression mode: 0 = None, 1 = LZ4 Block
    pub compression_flags: u8,
}

impl Default for DatabaseHeader {
    fn default() -> Self {
        let mut header = Self {
            magic: DATABASE_MAGIC,
            page_size: DEFAULT_PAGE_SIZE,
            version: FILE_FORMAT_VERSION,
            change_counter: 0,
            total_pages: 1, // Page 1 is allocated for header & root
            freelist_trunk: 0,
            freelist_count: 0,
            schema_cookie: 0,
            user_version: 0,
            vector_index_page: 0,
            wal_sequence: 0,
            header_crc32: 0,
            encryption_flags: 0,
            salt: [0u8; 16],
            kcv: [0u8; 16],
            compression_flags: 0,
        };
        header.header_crc32 = header.calculate_crc();
        header
    }
}

impl DatabaseHeader {
    /// Create a new DatabaseHeader with a specific page size
    pub fn new(page_size: u16) -> Self {
        let mut header = Self {
            page_size,
            ..Default::default()
        };
        header.header_crc32 = header.calculate_crc();
        header
    }

    /// Calculate CRC32 checksum over the first 44 bytes of the header
    pub fn calculate_crc(&self) -> u32 {
        let mut buf = [0u8; 44];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..10].copy_from_slice(&self.page_size.to_le_bytes());
        buf[10..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.change_counter.to_le_bytes());
        buf[16..20].copy_from_slice(&self.total_pages.to_le_bytes());
        buf[20..24].copy_from_slice(&self.freelist_trunk.to_le_bytes());
        buf[24..28].copy_from_slice(&self.freelist_count.to_le_bytes());
        buf[28..32].copy_from_slice(&self.schema_cookie.to_le_bytes());
        buf[32..36].copy_from_slice(&self.user_version.to_le_bytes());
        buf[36..40].copy_from_slice(&self.vector_index_page.to_le_bytes());
        buf[40..44].copy_from_slice(&self.wal_sequence.to_le_bytes());

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&buf);
        hasher.finalize()
    }

    /// Serialize header to 100-byte array
    pub fn to_bytes(&self) -> [u8; DATABASE_HEADER_SIZE] {
        let mut buf = [0u8; DATABASE_HEADER_SIZE];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..10].copy_from_slice(&self.page_size.to_le_bytes());
        buf[10..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.change_counter.to_le_bytes());
        buf[16..20].copy_from_slice(&self.total_pages.to_le_bytes());
        buf[20..24].copy_from_slice(&self.freelist_trunk.to_le_bytes());
        buf[24..28].copy_from_slice(&self.freelist_count.to_le_bytes());
        buf[28..32].copy_from_slice(&self.schema_cookie.to_le_bytes());
        buf[32..36].copy_from_slice(&self.user_version.to_le_bytes());
        buf[36..40].copy_from_slice(&self.vector_index_page.to_le_bytes());
        buf[40..44].copy_from_slice(&self.wal_sequence.to_le_bytes());
        let crc = self.calculate_crc();
        buf[44..48].copy_from_slice(&crc.to_le_bytes());
        buf[48] = self.encryption_flags;
        buf[49..65].copy_from_slice(&self.salt);
        buf[65..81].copy_from_slice(&self.kcv);
        buf[81] = self.compression_flags;
        // Remaining 18 bytes are zero-padded reserved
        buf
    }

    /// Parse header from 100-byte slice and verify CRC32
    pub fn from_bytes(slice: &[u8]) -> Result<Self> {
        if slice.len() < DATABASE_HEADER_SIZE {
            return Err(Error::Corrupted(
                "Slice is smaller than 100-byte header".into(),
            ));
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&slice[0..8]);
        if magic != DATABASE_MAGIC {
            return Err(Error::InvalidMagic);
        }

        let page_size = u16::from_le_bytes([slice[8], slice[9]]);
        if !page_size.is_power_of_two() || page_size < 512 {
            return Err(Error::InvalidPageSize(page_size));
        }

        let version = u16::from_le_bytes([slice[10], slice[11]]);
        let change_counter = u32::from_le_bytes([slice[12], slice[13], slice[14], slice[15]]);
        let total_pages = u32::from_le_bytes([slice[16], slice[17], slice[18], slice[19]]);
        let freelist_trunk = u32::from_le_bytes([slice[20], slice[21], slice[22], slice[23]]);
        let freelist_count = u32::from_le_bytes([slice[24], slice[25], slice[26], slice[27]]);
        let schema_cookie = u32::from_le_bytes([slice[28], slice[29], slice[30], slice[31]]);
        let user_version = u32::from_le_bytes([slice[32], slice[33], slice[34], slice[35]]);
        let vector_index_page = u32::from_le_bytes([slice[36], slice[37], slice[38], slice[39]]);
        let wal_sequence = u32::from_le_bytes([slice[40], slice[41], slice[42], slice[43]]);
        let header_crc32 = u32::from_le_bytes([slice[44], slice[45], slice[46], slice[47]]);

        let encryption_flags = slice[48];
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&slice[49..65]);
        let mut kcv = [0u8; 16];
        kcv.copy_from_slice(&slice[65..81]);
        let compression_flags = slice[81];

        let header = Self {
            magic,
            page_size,
            version,
            change_counter,
            total_pages,
            freelist_trunk,
            freelist_count,
            schema_cookie,
            user_version,
            vector_index_page,
            wal_sequence,
            header_crc32,
            encryption_flags,
            salt,
            kcv,
            compression_flags,
        };

        let calculated_crc = header.calculate_crc();
        if calculated_crc != header_crc32 {
            return Err(Error::PageCorrupted(1, calculated_crc, header_crc32));
        }

        Ok(header)
    }
}

#[derive(Debug, Clone)]
struct TransactionSavepoint {
    undo_pages: HashMap<PageId, Option<Vec<u8>>>,
    total_pages: u32,
    wal_frame_count: usize,
}

/// The Pager responsible for loading, allocating, caching, and writing database pages
pub struct Pager {
    header: DatabaseHeader,
    file: Option<File>,
    _path: Option<PathBuf>,
    _lock_file: Option<File>,
    _lock_path: Option<PathBuf>,
    in_memory_pages: HashMap<PageId, Vec<u8>>,
    wal: Option<Wal>,
    checkpoint_threshold: usize,
    cipher: Option<crate::crypto::DatabaseCipher>,
    transaction_savepoint: Option<TransactionSavepoint>,
    cache_capacity: usize,
    lru_order: VecDeque<PageId>,
    remote_reader: Option<Arc<dyn RemoteRangeReader>>,
    remote_writer: Option<Arc<dyn RemoteStorageAdapter>>,
}

#[allow(dead_code)]
fn is_process_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(windows)]
    {
        if let Ok(output) = std::process::Command::new("cmd")
            .args(["/C", &format!("tasklist /FI \"PID eq {pid}\" /NH")])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            return text.contains(&format!("{pid}"));
        }
        true
    }
    #[cfg(unix)]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(any(windows, unix)))]
    {
        true
    }
}

impl Pager {
    /// Open a disk file or create a fresh one with active Write-Ahead Logging
    pub fn open_with_config(path: &Path, page_size: u16, cache_capacity: usize) -> Result<Self> {
        Self::open_with_cipher(path, page_size, cache_capacity, None)
    }

    /// Open a disk file with an optional cryptographic cipher for Page-Level Encryption at Rest
    pub fn open_with_cipher(
        path: &Path,
        page_size: u16,
        _cache_capacity: usize,
        cipher: Option<crate::crypto::DatabaseCipher>,
    ) -> Result<Self> {
        // Atomic cross-process file lock via .tapir-lock
        let lock_path = path.with_extension("tapir-lock");
        let mut lock_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(lf) => lf,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let is_stale = if let Ok(content) = std::fs::read_to_string(&lock_path) {
                    if let Ok(pid) = content.trim().parse::<u32>() {
                        !is_process_alive(pid)
                    } else {
                        false
                    }
                } else {
                    false
                };

                if is_stale {
                    let _ = std::fs::remove_file(&lock_path);
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lock_path)
                        .map_err(|err| Error::Busy(format!("Failed to reclaim stale lockfile: {err}")))?
                } else {
                    return Err(Error::Busy(format!(
                        "Database file '{}' is locked by another process (active lockfile: {})",
                        path.display(),
                        lock_path.display()
                    )));
                }
            }
            Err(e) => return Err(Error::Io(e)),
        };
        let _ = write!(lock_file, "{}", std::process::id());
        let _ = lock_file.flush();

        struct LockGuard {
            file: Option<std::fs::File>,
            path: std::path::PathBuf,
            active: bool,
        }
        impl Drop for LockGuard {
            fn drop(&mut self) {
                if self.active {
                    self.file.take();
                    let _ = std::fs::remove_file(&self.path);
                }
            }
        }
        let mut _guard = LockGuard {
            file: Some(lock_file),
            path: lock_path.clone(),
            active: true,
        };

        let exists = path.exists();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let file_len = file.metadata()?.len();
        if exists && file_len > 0 && file_len < DATABASE_HEADER_SIZE as u64 {
            return Err(Error::Corrupted(
                "Database file exists but is smaller than minimum header size".into(),
            ));
        }

        let mut header = if exists && file_len >= DATABASE_HEADER_SIZE as u64 {
            let mut header_buf = [0u8; DATABASE_HEADER_SIZE];
            file.read_exact(&mut header_buf)?;
            let h = DatabaseHeader::from_bytes(&header_buf)?;

            // Verify encryption credentials
            if h.encryption_flags == 1 {
                if let Some(ref c) = cipher {
                    if !c.verify_kcv(&h.kcv) {
                        return Err(Error::DecryptionFailed(1));
                    }
                } else {
                    return Err(Error::EncryptedDatabase);
                }
            }
            h
        } else {
            let mut h = DatabaseHeader::new(page_size);
            if let Some(ref c) = cipher {
                h.encryption_flags = 1;
                h.salt = *c.salt();
                h.kcv = c.generate_kcv()?;
            }

            let mut page1 = vec![0u8; page_size as usize];
            page1[0..DATABASE_HEADER_SIZE].copy_from_slice(&h.to_bytes());

            let disk_page1 = if let Some(ref c) = cipher {
                c.encrypt_page(1, &page1)?
            } else {
                page1
            };

            file.seek(SeekFrom::Start(0))?;
            file.write_all(&disk_page1)?;
            file.sync_data()?;
            h
        };

        let mut in_memory_pages = HashMap::new();

        // Initialize WAL file: `path.tapir-wal`
        let wal_path = path.with_extension("tapir-wal");
        let mut wal = Wal::open(&wal_path, header.page_size as u32)?;

        // Run recovery if WAL contains committed frames
        let mut file_opt = Some(file);
        let mut recovered_pages = HashMap::new();
        wal.recover(&mut file_opt, &mut recovered_pages)?;

        // If encrypted, decrypt recovered pages
        for (pid, raw) in recovered_pages {
            let pt = if let Some(ref c) = cipher {
                c.decrypt_page(pid, &raw)?
            } else {
                raw
            };
            in_memory_pages.insert(pid, pt);
        }

        // Re-read header after recovery in case Page 1 was updated
        if let Some(f) = &mut file_opt {
            f.seek(SeekFrom::Start(0))?;
            let mut h_buf = [0u8; DATABASE_HEADER_SIZE];
            f.read_exact(&mut h_buf)?;
            if let Ok(recovered_header) = DatabaseHeader::from_bytes(&h_buf) {
                header = recovered_header;
            }
        }

        let cache_capacity = if _cache_capacity == 0 { 256 } else { _cache_capacity };
        let mut lru_order = VecDeque::new();
        for &pid in in_memory_pages.keys() {
            lru_order.push_back(pid);
        }

        _guard.active = false;
        let lock_file = _guard.file.take();

        Ok(Self {
            header,
            file: file_opt,
            _path: Some(path.to_path_buf()),
            _lock_file: lock_file,
            _lock_path: Some(lock_path),
            in_memory_pages,
            wal: Some(wal),
            checkpoint_threshold: DEFAULT_CHECKPOINT_THRESHOLD,
            cipher,
            transaction_savepoint: None,
            cache_capacity,
            lru_order,
            remote_reader: None,
            remote_writer: None,
        })
    }

    /// Open an in-memory database with active in-memory WAL
    pub fn open_in_memory(page_size: u16, cache_capacity: usize) -> Result<Self> {
        Self::open_in_memory_with_cipher(page_size, cache_capacity, None)
    }

    /// Open an in-memory database with optional cipher
    pub fn open_in_memory_with_cipher(
        page_size: u16,
        _cache_capacity: usize,
        cipher: Option<crate::crypto::DatabaseCipher>,
    ) -> Result<Self> {
        let mut header = DatabaseHeader::new(page_size);
        if let Some(ref c) = cipher {
            header.encryption_flags = 1;
            header.salt = *c.salt();
            header.kcv = c.generate_kcv()?;
        }

        let mut page1 = vec![0u8; page_size as usize];
        page1[0..DATABASE_HEADER_SIZE].copy_from_slice(&header.to_bytes());

        let mut in_memory_pages = HashMap::new();
        in_memory_pages.insert(1, page1);

        let wal = Wal::open_in_memory(page_size as u32);
        let cache_capacity = if _cache_capacity == 0 { 256 } else { _cache_capacity };
        let mut lru_order = VecDeque::new();
        lru_order.push_back(1);

        Ok(Self {
            header,
            file: None,
            _path: None,
            _lock_file: None,
            _lock_path: None,
            in_memory_pages,
            wal: Some(wal),
            checkpoint_threshold: DEFAULT_CHECKPOINT_THRESHOLD,
            cipher,
            transaction_savepoint: None,
            cache_capacity,
            lru_order,
            remote_reader: None,
            remote_writer: None,
        })
    }

    /// Open a read-only Pager streaming pages on-demand from remote cloud storage (S3/R2)
    pub fn open_remote(
        reader: Arc<dyn RemoteRangeReader>,
        _cache_capacity: usize,
        cipher: Option<crate::crypto::DatabaseCipher>,
    ) -> Result<Self> {
        let total_size = reader.total_size()?;
        if total_size < DATABASE_HEADER_SIZE as u64 {
            return Err(Error::Corrupted("Remote file too small for database header".into()));
        }

        let header_buf = reader.fetch_range(0, DATABASE_HEADER_SIZE)?;
        let header = DatabaseHeader::from_bytes(&header_buf)?;

        if header.encryption_flags == 1 {
            if let Some(ref c) = cipher {
                if !c.verify_kcv(&header.kcv) {
                    return Err(Error::DecryptionFailed(1));
                }
            } else {
                return Err(Error::EncryptedDatabase);
            }
        }

        let cache_capacity = if _cache_capacity == 0 { 256 } else { _cache_capacity };

        Ok(Self {
            header,
            file: None,
            _path: None,
            _lock_file: None,
            _lock_path: None,
            in_memory_pages: HashMap::new(),
            wal: None,
            checkpoint_threshold: DEFAULT_CHECKPOINT_THRESHOLD,
            cipher,
            transaction_savepoint: None,
            cache_capacity,
            lru_order: VecDeque::new(),
            remote_reader: Some(reader),
            remote_writer: None,
        })
    }

    /// Open a bidirectional writable Pager streaming and synchronizing with remote cloud storage (S3/R2)
    pub fn open_remote_writable(
        adapter: Arc<dyn RemoteStorageAdapter>,
        _cache_capacity: usize,
        cipher: Option<crate::crypto::DatabaseCipher>,
    ) -> Result<Self> {
        let total_size = adapter.total_size()?;
        let mut in_memory_pages = HashMap::new();
        let mut lru_order = VecDeque::new();

        let header = if total_size >= DATABASE_HEADER_SIZE as u64 {
            let header_buf = adapter.fetch_range(0, DATABASE_HEADER_SIZE)?;
            let h = DatabaseHeader::from_bytes(&header_buf)?;

            if h.encryption_flags == 1 {
                if let Some(ref c) = cipher {
                    if !c.verify_kcv(&h.kcv) {
                        return Err(Error::DecryptionFailed(1));
                    }
                } else {
                    return Err(Error::EncryptedDatabase);
                }
            }
            h
        } else if total_size == 0 {
            let mut h = DatabaseHeader::new(DEFAULT_PAGE_SIZE);
            if let Some(ref c) = cipher {
                h.encryption_flags = 1;
                h.salt = *c.salt();
                h.kcv = c.generate_kcv()?;
            }
            let mut page1 = vec![0u8; DEFAULT_PAGE_SIZE as usize];
            page1[0..DATABASE_HEADER_SIZE].copy_from_slice(&h.to_bytes());
            in_memory_pages.insert(1, page1);
            lru_order.push_back(1);
            h
        } else {
            return Err(Error::Corrupted("Remote file too small for database header".into()));
        };

        let cache_capacity = if _cache_capacity == 0 { 256 } else { _cache_capacity };
        let wal = Wal::open_in_memory(header.page_size as u32);

        Ok(Self {
            header,
            file: None,
            _path: None,
            _lock_file: None,
            _lock_path: None,
            in_memory_pages,
            wal: Some(wal),
            checkpoint_threshold: DEFAULT_CHECKPOINT_THRESHOLD,
            cipher,
            transaction_savepoint: None,
            cache_capacity,
            lru_order,
            remote_reader: Some(adapter.clone() as Arc<dyn RemoteRangeReader>),
            remote_writer: Some(adapter),
        })
    }

    /// Push local changes back to the remote cloud storage adapter
    pub fn push_remote(&mut self) -> Result<()> {
        let writer = self.remote_writer.clone().ok_or_else(|| {
            Error::Corrupted("No remote writable storage adapter configured".into())
        })?;

        let page_size = self.page_size();
        let total_pages = self.header.total_pages.max(1);
        let mut full_bytes = vec![0u8; total_pages as usize * page_size];

        // 1. Write header at start of Page 1
        let h_buf = self.header.to_bytes();
        full_bytes[0..DATABASE_HEADER_SIZE].copy_from_slice(&h_buf);

        // 2. Write Page 1 payload
        let page_1 = self.read_page(1)?;
        full_bytes[DATABASE_HEADER_SIZE..page_size].copy_from_slice(&page_1[DATABASE_HEADER_SIZE..page_size]);

        // 3. Write all subsequent pages
        for pid in 2..=total_pages {
            let page_data = self.read_page(pid)?;
            let offset = (pid as usize - 1) * page_size;
            full_bytes[offset..offset + page_size].copy_from_slice(&page_data);
        }

        // 4. Put object to remote adapter
        writer.put_object(&full_bytes)
    }

    /// Return reference to header
    pub fn header(&self) -> &DatabaseHeader {
        &self.header
    }

    /// Return mutable reference to header
    pub fn header_mut(&mut self) -> &mut DatabaseHeader {
        &mut self.header
    }

    /// Return page size in bytes
    pub fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    /// Return total pages count
    pub fn total_pages(&self) -> u32 {
        self.header.total_pages
    }

    /// Return active page cache capacity
    pub fn cache_capacity(&self) -> usize {
        self.cache_capacity
    }

    /// Return current count of in-memory resident pages
    pub fn in_memory_page_count(&self) -> usize {
        self.in_memory_pages.len()
    }

    /// Sync the current database header to disk
    pub fn sync_header(&mut self) -> Result<()> {
        self.header.header_crc32 = self.header.calculate_crc();
        if let Some(file) = &mut self.file {
            let header_bytes = self.header.to_bytes();
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&header_bytes)?;
            file.sync_data()?;
        }
        Ok(())
    }

    /// Set the root page of the master vector directory/snapshot in header and persist
    pub fn set_vector_index_page(&mut self, page_id: u32) -> Result<()> {
        self.header.vector_index_page = page_id;
        self.sync_header()
    }

    /// Return the root page of the master vector directory/snapshot from header
    pub fn vector_index_page(&self) -> u32 {
        self.header.vector_index_page
    }

    /// Update page access in LRU queue
    fn touch_lru(&mut self, page_id: PageId) {
        if let Some(pos) = self.lru_order.iter().position(|&pid| pid == page_id) {
            self.lru_order.remove(pos);
        }
        self.lru_order.push_back(page_id);
    }

    /// Evict clean pages when in-memory cache exceeds configured capacity.
    /// Only active for disk-backed databases where pages can be reloaded on demand.
    fn evict_if_needed(&mut self) {
        if self.file.is_none() && self.wal.is_none() {
            return;
        }
        let capacity = self.cache_capacity.max(4);
        while self.in_memory_pages.len() > capacity {
            let mut target = None;
            for (idx, &pid) in self.lru_order.iter().enumerate() {
                if pid == 1 {
                    continue; // Preserve page 1 header / schema root
                }
                if let Some(sp) = &self.transaction_savepoint {
                    if sp.undo_pages.contains_key(&pid) {
                        continue; // Preserve active uncommitted transactional pages
                    }
                }
                target = Some((idx, pid));
                break;
            }

            if let Some((idx, pid)) = target {
                self.lru_order.remove(idx);
                self.in_memory_pages.remove(&pid);
            } else {
                break;
            }
        }
    }

    /// Whether transparent page compression is enabled
    pub fn is_compressed(&self) -> bool {
        self.header.compression_flags == 1
    }

    /// Enable transparent LZ4 page compression for all pages > 1
    pub fn enable_compression(&mut self) {
        self.header.compression_flags = 1;
        self.header.header_crc32 = self.header.calculate_crc();
    }

    /// Helper to transparently decompress a page if compressed
    fn decompress_if_needed(&self, page_id: PageId, raw: Vec<u8>) -> Result<Vec<u8>> {
        let page_size = self.page_size();
        if page_id > 1
            && (self.is_compressed()
                || (!raw.is_empty() && raw[0] == compression::COMPRESSED_PAGE_MAGIC))
        {
            let decomp = compression::decompress_page_frame(&raw, page_size)?;
            if decomp.len() < page_size {
                let mut padded = decomp;
                padded.resize(page_size, 0);
                Ok(padded)
            } else {
                Ok(decomp)
            }
        } else {
            Ok(raw)
        }
    }

    /// Read data from a page by its ID.
    /// First checks in-memory cache, then WAL for the latest version; falls back to base disk file.
    pub fn read_page(&mut self, page_id: PageId) -> Result<Vec<u8>> {
        let page_size = self.page_size();
        if page_id == 0 || page_id > self.header.total_pages {
            return Err(Error::PageNotFound(page_id));
        }

        // 1. Check in-memory pages first (already decrypted plaintext)
        if let Some(data) = self.in_memory_pages.get(&page_id).cloned() {
            self.touch_lru(page_id);
            return Ok(data);
        }

        // 2. Check WAL index (MVCC latest version)
        if let Some(wal) = &self.wal {
            if let Some(raw) = wal.read_page(page_id) {
                let plaintext = if let Some(cipher) = &self.cipher {
                    cipher.decrypt_page(page_id, &raw)?
                } else {
                    raw
                };
                let final_page = self.decompress_if_needed(page_id, plaintext)?;
                self.in_memory_pages.insert(page_id, final_page.clone());
                self.touch_lru(page_id);
                self.evict_if_needed();
                return Ok(final_page);
            }
        }

        // 3. Fall back to reading from base disk file
        if let Some(file) = &mut self.file {
            let offset = (page_id as u64 - 1) * page_size as u64;
            file.seek(SeekFrom::Start(offset))?;
            let mut disk_buf = vec![0u8; page_size];
            file.read_exact(&mut disk_buf)?;

            let plaintext = if let Some(cipher) = &self.cipher {
                cipher.decrypt_page(page_id, &disk_buf)?
            } else {
                disk_buf
            };
            let final_page = self.decompress_if_needed(page_id, plaintext)?;
            self.in_memory_pages.insert(page_id, final_page.clone());
            self.touch_lru(page_id);
            self.evict_if_needed();
            return Ok(final_page);
        }

        // 4. Fall back to remote storage reader if configured
        if let Some(reader) = &self.remote_reader {
            let offset = (page_id as u64 - 1) * page_size as u64;
            let raw_bytes = reader.fetch_range(offset, page_size)?;
            let plaintext = if let Some(cipher) = &self.cipher {
                cipher.decrypt_page(page_id, &raw_bytes)?
            } else {
                raw_bytes
            };
            let final_page = self.decompress_if_needed(page_id, plaintext)?;
            self.in_memory_pages.insert(page_id, final_page.clone());
            self.touch_lru(page_id);
            self.evict_if_needed();
            return Ok(final_page);
        }

        Err(Error::PageNotFound(page_id))
    }

    /// Write data to a page by its ID via the Write-Ahead Log
    pub fn write_page(&mut self, page_id: PageId, data: &[u8]) -> Result<()> {
        if self.remote_reader.is_some() && self.file.is_none() && self.remote_writer.is_none() {
            return Err(Error::Corrupted(
                "Cannot write to read-only remote storage connection (HTTP Range reader is read-only)"
                    .into(),
            ));
        }

        // If connected to a writable remote storage, write page to in_memory_pages directly
        if self.file.is_none() && self.remote_writer.is_some() {
            self.in_memory_pages.insert(page_id, data.to_vec());
            self.touch_lru(page_id);
            return Ok(());
        }

        let page_size = self.page_size();
        if data.len() != page_size {
            return Err(Error::Corrupted(format!(
                "Invalid page data length: expected {}, got {}",
                page_size,
                data.len()
            )));
        }

        if page_id == 0 || page_id > self.header.total_pages {
            return Err(Error::PageNotFound(page_id));
        }

        // CRITICAL: Page 1 carries the DatabaseHeader in its first DATABASE_HEADER_SIZE bytes.
        // Callers such as the B+Tree engine construct page 1 with a zeroed header region
        // (e.g. `vec![0u8; page_size]` + `init_interior_page`). We always patch those bytes
        // from the authoritative `self.header` *before* the undo log, in-memory cache, and
        // WAL frame so that `total_pages` (and other header fields) are never lost.
        let patched_p1: Vec<u8>;
        let data: &[u8] = if page_id == 1 {
            let mut p = data.to_vec();
            let header_bytes = self.header.to_bytes();
            let len = header_bytes.len().min(p.len());
            p[..len].copy_from_slice(&header_bytes[..len]);
            patched_p1 = p;
            &patched_p1
        } else {
            data
        };

        // If in an active transaction, record original page data for undo rollback if not yet recorded
        let needs_undo = self.transaction_savepoint.as_ref().map(|sp| (!sp.undo_pages.contains_key(&page_id), sp.total_pages));
        if let Some((true, total_pages)) = needs_undo {
            let prev_data = if let Some(cached) = self.in_memory_pages.get(&page_id) {
                Some(cached.clone())
            } else if page_id <= total_pages {
                Some(self.read_page(page_id)?)
            } else {
                None
            };
            if let Some(sp) = &mut self.transaction_savepoint {
                sp.undo_pages.insert(page_id, prev_data);
            }
        }

        // Cache plaintext in memory for O(1) reads
        self.in_memory_pages.insert(page_id, data.to_vec());
        self.touch_lru(page_id);
        self.evict_if_needed();

        // Prepare page payload (compress if compression enabled and page_id > 1)
        let payload = if self.is_compressed() && page_id > 1 {
            let mut comp = compression::compress_page_frame(data);
            if comp.len() < page_size {
                comp.resize(page_size, 0);
            }
            comp
        } else {
            data.to_vec()
        };

        // Prepare encrypted page image for disk / WAL
        let disk_bytes = if let Some(cipher) = &self.cipher {
            cipher.encrypt_page(page_id, &payload)?
        } else {
            payload
        };

        // 1. Write frame to WAL or fallback to disk file
        let is_in_tx = self.is_in_transaction();
        let should_checkpoint = if let Some(wal) = &mut self.wal {
            let is_commit = !is_in_tx;
            wal.write_frame(page_id, &disk_bytes, is_commit, self.header.total_pages)?;

            // Auto-checkpoint only outside active transactions if WAL exceeds configured threshold
            !is_in_tx && wal.frame_count() >= self.checkpoint_threshold
        } else {
            // Fallback direct write if WAL is disabled
            if let Some(file) = &mut self.file {
                let offset = (page_id as u64 - 1) * page_size as u64;
                file.seek(SeekFrom::Start(offset))?;
                file.write_all(&disk_bytes)?;
                file.sync_data()?;
            }
            false
        };

        if should_checkpoint {
            self.checkpoint()?;
        }

        Ok(())
    }

    /// Allocate a new page at the end of the database file
    pub fn allocate_page(&mut self) -> Result<PageId> {
        if self.remote_reader.is_some() && self.file.is_none() && self.remote_writer.is_none() {
            return Err(Error::Corrupted(
                "Cannot allocate pages on read-only remote storage connection".into(),
            ));
        }

        let page_size = self.page_size();
        let new_id = self.header.total_pages + 1;
        self.header.total_pages = new_id;

        // If in an active transaction, record that new_id was newly allocated (undo removes it)
        if let Some(sp) = &mut self.transaction_savepoint {
            if !sp.undo_pages.contains_key(&new_id) {
                sp.undo_pages.insert(new_id, None);
            }
        }

        let empty_page = vec![0u8; page_size];
        if self.file.is_none() && self.remote_writer.is_some() {
            self.in_memory_pages.insert(new_id, empty_page);
            // Keep in-memory page 1 consistent with updated total_pages
            self.sync_header_into_page1_cache();
            return Ok(new_id);
        }
        if let Some(file) = &mut self.file {
            // Write the new empty page to extend the physical file
            let offset = (new_id as u64 - 1) * page_size as u64;
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(&empty_page)?;
            file.sync_data()?;
        }

        self.in_memory_pages.insert(new_id, empty_page);
        self.touch_lru(new_id);
        self.evict_if_needed();

        // CRITICAL: Keep in-memory page 1 consistent with updated total_pages FIRST,
        // then write a fresh WAL frame for page 1. Without the WAL frame, a pre-existing
        // stale page 1 frame in the WAL would be checkpointed to disk AFTER allocate_page's
        // direct header write, overwriting the correct total_pages and causing PageNotFound.
        self.sync_header_into_page1_cache();

        // Push the updated page 1 (with new total_pages in the header region) into the WAL
        // so that any checkpoint — explicit or auto — will write the authoritative header
        // rather than a stale older frame.
        if let Some(page1_data) = self.in_memory_pages.get(&1).cloned() {
            let is_in_tx = self.transaction_savepoint.is_some();
            let is_commit = !is_in_tx;
            let payload = if let Some(cipher) = &self.cipher {
                cipher.encrypt_page(1, &page1_data)?
            } else {
                page1_data.clone()
            };
            if let Some(wal) = &mut self.wal {
                wal.write_frame(1, &payload, is_commit, self.header.total_pages)?;
            } else if let Some(file) = &mut self.file {
                // WAL disabled: write the updated header directly to disk
                let header_bytes = self.header.to_bytes();
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&header_bytes)?;
                file.sync_data()?;
            }
        }

        Ok(new_id)
    }

    /// Patch the current `self.header` bytes into the in-memory cache of page 1
    /// so that WAL writes of page 1 always carry the authoritative total_pages.
    fn sync_header_into_page1_cache(&mut self) {
        let header_bytes = self.header.to_bytes();
        let page_size = self.header.page_size as usize;
        let page1 = self
            .in_memory_pages
            .entry(1)
            .or_insert_with(|| vec![0u8; page_size]);
        let len = header_bytes.len().min(page1.len());
        page1[..len].copy_from_slice(&header_bytes[..len]);
    }

    /// Flush all committed pages from the WAL to the base `.tapir` file,
    /// and synchronize them directly into the in-memory page cache.
    pub fn checkpoint(&mut self) -> Result<usize> {
        if let Some(mut wal) = self.wal.take() {
            let result = wal.checkpoint(&mut self.file, &mut self.in_memory_pages);
            self.wal = Some(wal);
            result
        } else {
            Ok(0)
        }
    }

    /// Flush all dirty pages to durable storage
    pub fn sync(&mut self) -> Result<()> {
        self.checkpoint()?;
        if let Some(file) = &mut self.file {
            file.sync_all()?;
        }
        Ok(())
    }

    /// Check if a transaction is currently active
    pub fn is_in_transaction(&self) -> bool {
        self.transaction_savepoint.is_some()
    }

    /// Begin a transaction savepoint in O(1) time and memory
    pub fn begin_transaction(&mut self) -> Result<()> {
        if self.transaction_savepoint.is_some() {
            return Err(Error::TransactionError("Transaction already in progress".into()));
        }
        let frame_count = self.wal.as_ref().map(|w| w.frame_count()).unwrap_or(0);
        self.transaction_savepoint = Some(TransactionSavepoint {
            undo_pages: HashMap::new(),
            total_pages: self.header.total_pages,
            wal_frame_count: frame_count,
        });
        Ok(())
    }

    /// Commit the current transaction
    pub fn commit_transaction(&mut self) -> Result<()> {
        if self.transaction_savepoint.take().is_none() {
            return Err(Error::TransactionError("No active transaction to commit".into()));
        }
        if let Some(wal) = &mut self.wal {
            wal.sync()?;
            if wal.frame_count() >= self.checkpoint_threshold {
                self.checkpoint()?;
            }
        } else {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Rollback the current transaction using the undo delta log
    pub fn rollback_transaction(&mut self) -> Result<()> {
        let savepoint = self.transaction_savepoint.take()
            .ok_or_else(|| Error::TransactionError("No active transaction to rollback".into()))?;

        for (page_id, original_data) in savepoint.undo_pages {
            match original_data {
                Some(prev) => {
                    self.in_memory_pages.insert(page_id, prev);
                    self.touch_lru(page_id);
                }
                None => {
                    self.in_memory_pages.remove(&page_id);
                    if let Some(pos) = self.lru_order.iter().position(|&pid| pid == page_id) {
                        self.lru_order.remove(pos);
                    }
                }
            }
        }
        self.header.total_pages = savepoint.total_pages;

        if let Some(wal) = &mut self.wal {
            wal.rollback_to(savepoint.wal_frame_count)?;
        }
        Ok(())
    }

    /// Create an atomic, point-in-time snapshot backup to a target file path
    pub fn backup_to(&mut self, dest_path: &Path) -> Result<u32> {
        // Prevent catastrophic self-overwrite on active database
        if let Some(ref lock_path) = self._lock_path {
            let active_db = lock_path.with_extension("tapir");
            if let (Ok(canon_dest), Ok(canon_active)) = (dest_path.canonicalize(), active_db.canonicalize()) {
                if canon_dest == canon_active {
                    return Err(Error::SqlSyntax(
                        "Cannot VACUUM INTO the currently active database file".into(),
                    ));
                }
            } else if dest_path == active_db {
                return Err(Error::SqlSyntax(
                    "Cannot VACUUM INTO the currently active database file".into(),
                ));
            }
        }

        // Flush all WAL frames to clean base state
        let _ = self.checkpoint()?;

        let mut dest_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dest_path)?;

        let total = self.header.total_pages;
        let page_size = self.page_size();

        for pid in 1..=total {
            let page_data = self.read_page(pid)?;
            let disk_page = if let Some(ref c) = self.cipher {
                c.encrypt_page(pid, &page_data)?
            } else {
                page_data
            };
            dest_file.seek(SeekFrom::Start((pid as u64 - 1) * page_size as u64))?;
            dest_file.write_all(&disk_page)?;
        }

        dest_file.sync_all()?;
        Ok(total)
    }

    /// Export active WAL frames for streaming replication to replica nodes
    pub fn export_wal_frames(&self) -> Result<Vec<(PageId, Vec<u8>)>> {
        if let Some(wal) = &self.wal {
            Ok(wal.export_frames())
        } else {
            Ok(Vec::new())
        }
    }

    /// Apply a replicated WAL frame from a remote primary node
    pub fn apply_wal_frame(&mut self, page_id: PageId, data: &[u8]) -> Result<()> {
        self.write_page(page_id, data)
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        // Release OS file handle first, then remove .tapir-lock file
        self._lock_file.take();
        if let Some(ref lock_path) = self._lock_path {
            let _ = std::fs::remove_file(lock_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_serialization_roundtrip() {
        let header = DatabaseHeader::default();
        let bytes = header.to_bytes();
        let parsed = DatabaseHeader::from_bytes(&bytes).expect("Header parse failed");
        assert_eq!(header, parsed);
    }

    #[test]
    fn test_in_memory_pager_read_write_alloc() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Failed to open in-memory pager");
        assert_eq!(pager.total_pages(), 1);

        let page1 = pager.read_page(1).expect("Failed to read page 1");
        assert_eq!(page1.len(), 4096);
        assert_eq!(&page1[0..8], b"TAPIRUS\0");

        let page2_id = pager.allocate_page().expect("Failed to allocate page 2");
        assert_eq!(page2_id, 2);
        assert_eq!(pager.total_pages(), 2);

        let mut custom_data = vec![42u8; 4096];
        custom_data[0..4].copy_from_slice(b"TEST");
        pager.write_page(page2_id, &custom_data).expect("Failed write");

        let read_back = pager.read_page(page2_id).expect("Failed to read back");
        assert_eq!(&read_back[0..4], b"TEST");
        assert_eq!(read_back[10], 42);
    }

    #[test]
    fn test_transaction_undo_rollback_and_page_restore() {
        let mut pager = Pager::open_in_memory(4096, 128).expect("Open in-memory");
        let page2_id = pager.allocate_page().expect("Allocate 2");
        let data1 = vec![1u8; 4096];
        pager.write_page(page2_id, &data1).expect("Write page 2");

        // Begin transaction
        pager.begin_transaction().expect("Begin transaction");
        assert!(pager.is_in_transaction());

        // Modify page 2 during transaction
        let data2 = vec![2u8; 4096];
        pager.write_page(page2_id, &data2).expect("Modify page 2");
        assert_eq!(pager.read_page(page2_id).unwrap()[0], 2);

        // Allocate page 3 during transaction
        let _page3_id = pager.allocate_page().expect("Allocate 3");
        assert_eq!(pager.total_pages(), 3);

        // Rollback transaction
        pager.rollback_transaction().expect("Rollback transaction");
        assert!(!pager.is_in_transaction());

        // Verify page 2 is reverted back to data1 (1u8)
        assert_eq!(pager.read_page(page2_id).unwrap()[0], 1);
        // Verify total_pages restored
        assert_eq!(pager.total_pages(), 2);
    }

    #[test]
    fn test_pager_lru_eviction_on_disk() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!(
            "tapirus_lru_test_{}.tapir",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        {
            // Open with small cache capacity of 5 pages
            let mut pager = Pager::open_with_config(&db_path, 4096, 5).expect("Open with capacity 5");
            assert_eq!(pager.cache_capacity(), 5);

            // Allocate and write 20 pages
            for i in 2..=20 {
                let pid = pager.allocate_page().expect("Allocate");
                let mut pdata = vec![i as u8; 4096];
                pdata[0] = i as u8;
                pager.write_page(pid, &pdata).expect("Write");
            }
            pager.sync().expect("Sync");

            // Verify in_memory_pages is strictly bounded
            assert!(pager.in_memory_page_count() <= 6);

            // Read back an early evicted page (e.g. page 2), verifying it is cleanly reloaded from WAL/disk
            let p2 = pager.read_page(2).expect("Reload evicted page 2");
            assert_eq!(p2[0], 2);
        }

        // Cleanup test file
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    }
}
