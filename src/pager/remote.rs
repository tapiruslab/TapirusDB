//! # S3 / Cloudflare R2 Remote Storage Adapter & Remote Pager for TapirusDB
//!
//! Provides cloud-native, on-demand block storage reading via HTTP Range Requests
//! (`Range: bytes=offset-(offset+4095)`) directly against AWS S3, Cloudflare R2, MinIO,
//! or GCP Cloud Storage.
//!
//! # Architecture
//! In serverless and edge environments (e.g. AWS Lambda, Cloudflare Workers), downloading
//! an entire multi-gigabyte `.tapir` database file causes prohibitive cold-start latency.
//! The `RemotePager` streams only the specific 4KB B+Tree and Vector pages accessed by
//! the query and caches them in local memory, achieving sub-millisecond subsequent reads.

use crate::crypto::DatabaseCipher;
use crate::error::{Error, Result};
use crate::pager::{DatabaseHeader, PageId, DATABASE_HEADER_SIZE};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Trait for fetching byte slices from remote cloud object stores
pub trait RemoteRangeReader: Send + Sync {
    /// Fetch exact byte range `[offset, offset + length)` from remote storage
    fn fetch_range(&self, offset: u64, length: usize) -> Result<Vec<u8>>;
    /// Get the total size of the database object in bytes
    fn total_size(&self) -> Result<u64>;
}

/// Trait for bidirectional remote cloud object storage (read range + put object)
pub trait RemoteStorageAdapter: RemoteRangeReader {
    /// Write back entire database bytes to remote object storage
    fn put_object(&self, data: &[u8]) -> Result<()>;
}

/// In-memory mock range reader and writable storage for unit tests, local development, and simulated network environments
pub struct MockRemoteRangeStorage {
    data: RwLock<Vec<u8>>,
    fetch_count: AtomicUsize,
    bytes_read: AtomicU64,
    put_count: AtomicUsize,
}

impl MockRemoteRangeStorage {
    /// Create a new mock storage pre-loaded with raw database file bytes
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: RwLock::new(data),
            fetch_count: AtomicUsize::new(0),
            bytes_read: AtomicU64::new(0),
            put_count: AtomicUsize::new(0),
        }
    }

    /// Create empty mock remote storage
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Length of currently stored data in bytes
    pub fn bytes_len(&self) -> usize {
        self.data.read().len()
    }

    /// Number of remote range calls made
    pub fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::Relaxed)
    }

    /// Total bytes transferred over mock remote network
    pub fn bytes_transferred(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    /// Total number of put_object writes executed
    pub fn put_count(&self) -> usize {
        self.put_count.load(Ordering::Relaxed)
    }

    /// Extract snapshot of current storage bytes
    pub fn data(&self) -> Vec<u8> {
        self.data.read().clone()
    }
}

impl Default for MockRemoteRangeStorage {
    fn default() -> Self {
        Self::empty()
    }
}

impl RemoteRangeReader for MockRemoteRangeStorage {
    fn fetch_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let guard = self.data.read();
        let start = offset as usize;
        let end = start + length;
        if start > guard.len() {
            return Err(Error::Corrupted(format!(
                "Remote fetch out of bounds: offset {} exceeds file size {}",
                offset,
                guard.len()
            )));
        }

        let actual_end = end.min(guard.len());
        let slice = guard[start..actual_end].to_vec();

        self.fetch_count.fetch_add(1, Ordering::Relaxed);
        self.bytes_read.fetch_add(slice.len() as u64, Ordering::Relaxed);

        Ok(slice)
    }

    fn total_size(&self) -> Result<u64> {
        Ok(self.data.read().len() as u64)
    }
}

impl RemoteStorageAdapter for MockRemoteRangeStorage {
    fn put_object(&self, data: &[u8]) -> Result<()> {
        let mut guard = self.data.write();
        *guard = data.to_vec();
        self.put_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// S3 / Cloudflare R2 connection parameters for HTTP Range query requests
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3StorageConfig {
    /// Target bucket name (e.g. "tapirus-db-prod")
    pub bucket: String,
    /// Object path key in bucket (e.g. "databases/ecommerce.tapir")
    pub key: String,
    /// S3-compatible API endpoint (e.g. "https://<account_id>.r2.cloudflarestorage.com")
    pub endpoint: String,
    /// AWS / Cloud region (e.g. "auto", "us-east-1")
    pub region: String,
    /// Optional bearer or API token for signed requests
    pub auth_token: Option<String>,
}

impl S3StorageConfig {
    /// Construct standard HTTP Range header string for page reading
    pub fn make_range_header(&self, offset: u64, length: usize) -> String {
        let end = offset + length as u64 - 1;
        format!("bytes={}-{}", offset, end)
    }
}

/// Remote Pager managing on-demand 4KB page streaming and caching over S3 / R2
pub struct RemotePager {
    header: DatabaseHeader,
    reader: Arc<dyn RemoteRangeReader>,
    in_memory_pages: HashMap<PageId, Vec<u8>>,
    cache_capacity: usize,
    cipher: Option<DatabaseCipher>,
    cache_hits: usize,
    network_fetches: usize,
}

impl RemotePager {
    /// Open a remote database using any `RemoteRangeReader` backend
    pub fn open(
        reader: Arc<dyn RemoteRangeReader>,
        cache_capacity: usize,
        cipher: Option<DatabaseCipher>,
    ) -> Result<Self> {
        let total_size = reader.total_size()?;
        if total_size < DATABASE_HEADER_SIZE as u64 {
            return Err(Error::Corrupted(
                "Remote database file is smaller than minimum header size".into(),
            ));
        }

        // Fetch 100-byte database header via Range request [0..100]
        let header_bytes = reader.fetch_range(0, DATABASE_HEADER_SIZE)?;
        let mut header_buf = [0u8; DATABASE_HEADER_SIZE];
        header_buf.copy_from_slice(&header_bytes[..DATABASE_HEADER_SIZE]);

        let header = DatabaseHeader::from_bytes(&header_buf)?;

        // Verify cryptographic verification code if database is encrypted
        if header.encryption_flags == 1 {
            if let Some(ref c) = cipher {
                if !c.verify_kcv(&header.kcv) {
                    return Err(Error::DecryptionFailed(1));
                }
            } else {
                return Err(Error::EncryptedDatabase);
            }
        }

        Ok(Self {
            header,
            reader,
            in_memory_pages: HashMap::new(),
            cache_capacity: cache_capacity.max(64),
            cipher,
            cache_hits: 0,
            network_fetches: 0,
        })
    }

    /// Return database header
    pub fn header(&self) -> &DatabaseHeader {
        &self.header
    }

    /// Return page size
    pub fn page_size(&self) -> usize {
        self.header.page_size as usize
    }

    /// Return total pages count
    pub fn total_pages(&self) -> u32 {
        self.header.total_pages
    }

    /// Number of cache hits served from memory
    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    /// Number of network fetches made to remote storage
    pub fn network_fetches(&self) -> usize {
        self.network_fetches
    }

    /// Read a page by its ID (1-indexed)
    pub fn read_page(&mut self, page_id: PageId) -> Result<Vec<u8>> {
        let page_size = self.page_size();
        if page_id == 0 || page_id > self.header.total_pages {
            return Err(Error::PageNotFound(page_id));
        }

        // 1. Check in-memory LRU cache
        if let Some(page) = self.in_memory_pages.get(&page_id) {
            self.cache_hits += 1;
            return Ok(page.clone());
        }

        // 2. Fetch page bytes over remote storage range request
        let offset = (page_id as u64 - 1) * page_size as u64;
        let raw_bytes = self.reader.fetch_range(offset, page_size)?;
        self.network_fetches += 1;

        if raw_bytes.len() != page_size {
            return Err(Error::Corrupted(format!(
                "Incomplete page fetch: expected {} bytes, got {}",
                page_size,
                raw_bytes.len()
            )));
        }

        // 3. Decrypt page if encryption is active
        let plaintext = if let Some(ref cipher) = self.cipher {
            cipher.decrypt_page(page_id, &raw_bytes)?
        } else {
            raw_bytes
        };

        // Cache in memory for subsequent O(1) hits
        if self.in_memory_pages.len() >= self.cache_capacity {
            // Evict arbitrary entry if cache capacity reached
            if let Some(&first_key) = self.in_memory_pages.keys().next() {
                self.in_memory_pages.remove(&first_key);
            }
        }
        self.in_memory_pages.insert(page_id, plaintext.clone());

        Ok(plaintext)
    }
}
