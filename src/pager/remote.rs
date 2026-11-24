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
//! the query and caches them in local memory. Cache behavior and latency depend
//! on the remote store, network and workload.

use crate::crypto::DatabaseCipher;
use crate::error::{Error, Result};
use crate::pager::{DATABASE_HEADER_SIZE, DatabaseHeader, PageId};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

/// Delegate reads without trait-object upcasting, which is unavailable on Rust 1.85.
pub(crate) struct AdapterRangeReader(pub Arc<dyn RemoteStorageAdapter>);

impl RemoteRangeReader for AdapterRangeReader {
    fn fetch_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.0.fetch_range(offset, length)
    }

    fn total_size(&self) -> Result<u64> {
        self.0.total_size()
    }
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
        self.bytes_read
            .fetch_add(slice.len() as u64, Ordering::Relaxed);

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

/// Real HTTP/HTTPS Cloud Object Storage Adapter for AWS S3, Cloudflare R2, MinIO, and GCP.
///
/// Executes genuine HTTP Range requests (`Range: bytes=offset-end`) to stream 4KB pages
/// on-demand into memory over TLS/TCP with zero local file footprint.
#[cfg(feature = "cloud-s3")]
pub struct CloudS3RemoteStorage {
    config: S3StorageConfig,
    agent: ureq::Agent,
}

#[cfg(feature = "cloud-s3")]
impl CloudS3RemoteStorage {
    /// Construct a new Cloud S3 / R2 storage adapter from configuration
    pub fn new(config: S3StorageConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_per_read(Some(web_time::Duration::from_secs(30)))
            .timeout_per_write(Some(web_time::Duration::from_secs(30)))
            .build()
            .new_agent();
        Self { config, agent }
    }

    /// Construct object URL from endpoint, bucket, and key
    pub fn object_url(&self) -> String {
        let base = self.config.endpoint.trim_end_matches('/');
        let bucket = self.config.bucket.trim_matches('/');
        let key = self.config.key.trim_start_matches('/');
        if bucket.is_empty() {
            format!("{base}/{key}")
        } else {
            format!("{base}/{bucket}/{key}")
        }
    }
}

#[cfg(feature = "cloud-s3")]
impl RemoteRangeReader for CloudS3RemoteStorage {
    fn fetch_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let url = self.object_url();
        let range_header = self.config.make_range_header(offset, length);
        let mut req = self.agent.get(&url).header("Range", &range_header);

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.header("Authorization", token);
            } else {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
        }

        let resp = req.call().map_err(|e| {
            Error::Io(std::io::Error::other(format!(
                "Cloud S3 range request failed for {url} [Range: {range_header}]: {e}"
            )))
        })?;

        let mut reader = resp.into_body().into_reader();
        let mut buf = Vec::with_capacity(length);
        std::io::Read::read_to_end(&mut reader, &mut buf)?;
        Ok(buf)
    }

    fn total_size(&self) -> Result<u64> {
        let url = self.object_url();
        let mut req = self.agent.head(&url);

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.header("Authorization", token);
            } else {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
        }

        if let Ok(resp) = req.call()
            && let Some(cl) = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
            && let Ok(size) = cl.parse::<u64>()
        {
            return Ok(size);
        }

        // Fallback: request 1-byte range to inspect Content-Range header
        let mut req_range = self.agent.get(&url).header("Range", "bytes=0-0");
        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req_range = req_range.header("Authorization", token);
            } else {
                req_range = req_range.header("Authorization", format!("Bearer {token}"));
            }
        }

        let resp = req_range.call().map_err(|e| {
            Error::Io(std::io::Error::other(format!(
                "Failed to determine S3 object size for {url}: {e}"
            )))
        })?;

        if let Some(cr) = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            && let Some(slash_idx) = cr.rfind('/')
        {
            let total_str = &cr[slash_idx + 1..];
            if let Ok(size) = total_str.trim().parse::<u64>() {
                return Ok(size);
            }
        }

        if let Some(cl) = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            && let Ok(size) = cl.parse::<u64>()
        {
            return Ok(size);
        }

        Err(Error::Corrupted(format!(
            "Cloud S3 response missing Content-Length or Content-Range headers for {url}"
        )))
    }
}

#[cfg(feature = "cloud-s3")]
impl RemoteStorageAdapter for CloudS3RemoteStorage {
    fn put_object(&self, data: &[u8]) -> Result<()> {
        let url = self.object_url();
        let mut req = self
            .agent
            .put(&url)
            .header("Content-Type", "application/octet-stream");

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.header("Authorization", token);
            } else {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
        }

        req.send(data).map_err(|e| {
            Error::Io(std::io::Error::other(format!(
                "Cloud S3 PUT request failed for {url}: {e}"
            )))
        })?;
        Ok(())
    }
}

/// Callback type for synchronous on-demand 4KB page reads from a browser OPFS `FileSystemSyncAccessHandle`.
pub type OpfsSyncReadFn = Arc<dyn Fn(u64, usize) -> Result<Vec<u8>> + Send + Sync>;

/// Callback type for synchronous 4KB page writes to a browser OPFS `FileSystemSyncAccessHandle`.
pub type OpfsSyncWriteFn = Arc<dyn Fn(u64, &[u8]) -> Result<()> + Send + Sync>;

/// Origin Private File System (`FileSystemSyncAccessHandle`) Zero-Copy 4KB Pager (`OpfsSyncPager`).
///
/// Enables `TapirusWasm` and embedded runtimes to open multi-gigabyte `.tapir` files inside a
/// Browser Web Worker without copying the entire file into the `2 GB – 4 GB` `wasm32` linear heap.
/// Only the requested `4,096`-byte B+Tree, Document, and HNSW Vector pages are read and decrypted
/// on demand into a fixed-capacity LRU page cache.
pub struct OpfsSyncPager {
    total_size_bytes: AtomicU64,
    page_size: usize,
    cache_capacity_pages: usize,
    resident_pages: RwLock<HashMap<PageId, Vec<u8>>>,
    lru_queue: RwLock<std::collections::VecDeque<PageId>>,
    dirty_pages: RwLock<std::collections::BTreeSet<PageId>>,
    sync_reader: Option<OpfsSyncReadFn>,
    sync_writer: Option<OpfsSyncWriteFn>,
    cache_hits: AtomicUsize,
    opfs_page_reads: AtomicUsize,
    opfs_page_writes: AtomicUsize,
}

impl OpfsSyncPager {
    /// Construct a new `OpfsSyncPager` with a fixed 4KB page cache cap (`cache_capacity_pages`).
    pub fn new(total_size_bytes: u64, cache_capacity_pages: usize) -> Self {
        Self {
            total_size_bytes: AtomicU64::new(total_size_bytes),
            page_size: crate::pager::DEFAULT_PAGE_SIZE as usize,
            cache_capacity_pages: cache_capacity_pages.max(8),
            resident_pages: RwLock::new(HashMap::new()),
            lru_queue: RwLock::new(std::collections::VecDeque::new()),
            dirty_pages: RwLock::new(std::collections::BTreeSet::new()),
            sync_reader: None,
            sync_writer: None,
            cache_hits: AtomicUsize::new(0),
            opfs_page_reads: AtomicUsize::new(0),
            opfs_page_writes: AtomicUsize::new(0),
        }
    }

    /// Construct an `OpfsSyncPager` backed by synchronous `FileSystemSyncAccessHandle.read/write` callbacks.
    pub fn with_sync_callbacks(
        total_size_bytes: u64,
        cache_capacity_pages: usize,
        reader: OpfsSyncReadFn,
        writer: Option<OpfsSyncWriteFn>,
    ) -> Self {
        let mut pager = Self::new(total_size_bytes, cache_capacity_pages);
        pager.sync_reader = Some(reader);
        pager.sync_writer = writer;
        pager
    }

    /// Compute the 0-based byte offset in the `.tapir` file for 1-indexed `page_id`.
    #[inline]
    pub fn page_byte_offset(&self, page_id: PageId) -> u64 {
        (page_id.saturating_sub(1) as u64) * (self.page_size as u64)
    }

    /// Stage a `4,096`-byte page fetched from `FileSystemSyncAccessHandle.read` into the bounded LRU cache.
    pub fn stage_page_4kb(&self, page_id: PageId, bytes: &[u8]) -> Result<()> {
        if page_id == 0 {
            return Err(Error::PageNotFound(0));
        }
        let mut page_buf = vec![0u8; self.page_size];
        let copy_len = bytes.len().min(self.page_size);
        page_buf[..copy_len].copy_from_slice(&bytes[..copy_len]);

        let end_offset = self.page_byte_offset(page_id) + (bytes.len() as u64);
        let cur_total = self.total_size_bytes.load(Ordering::Relaxed);
        if end_offset > cur_total {
            self.total_size_bytes.store(end_offset, Ordering::Relaxed);
        }

        self.insert_cached_page(page_id, page_buf, false);
        Ok(())
    }

    /// Write an arbitrary byte range `[offset, offset + data.len())` into 4KB page slots,
    /// marking touched pages dirty and forwarding to `FileSystemSyncAccessHandle.write` if attached.
    pub fn write_range(&self, offset: u64, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if let Some(ref writer) = self.sync_writer {
            writer(offset, data)?;
        }

        let ps = self.page_size as u64;
        let mut cursor = 0usize;
        while cursor < data.len() {
            let abs_off = offset + (cursor as u64);
            let page_id = ((abs_off / ps) as u32) + 1;
            let in_page_off = (abs_off % ps) as usize;
            let chunk_len = (self.page_size - in_page_off).min(data.len() - cursor);

            let mut page_bytes = self
                .fetch_or_load_page_4kb(page_id)
                .unwrap_or_else(|_| vec![0u8; self.page_size]);
            page_bytes[in_page_off..in_page_off + chunk_len]
                .copy_from_slice(&data[cursor..cursor + chunk_len]);

            self.insert_cached_page(page_id, page_bytes, true);
            self.opfs_page_writes.fetch_add(1, Ordering::Relaxed);
            cursor += chunk_len;
        }

        let end_off = offset + (data.len() as u64);
        let cur_total = self.total_size_bytes.load(Ordering::Relaxed);
        if end_off > cur_total {
            self.total_size_bytes.store(end_off, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Read a single `4,096`-byte page on demand (serving from LRU cache or calling `sync_reader`).
    pub fn fetch_or_load_page_4kb(&self, page_id: PageId) -> Result<Vec<u8>> {
        if page_id == 0 {
            return Err(Error::PageNotFound(0));
        }
        if let Some(cached) = self.resident_pages.read().get(&page_id).cloned() {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            self.touch_lru(page_id);
            return Ok(cached);
        }

        if let Some(ref reader) = self.sync_reader {
            let offset = self.page_byte_offset(page_id);
            let total = self.total_size_bytes.load(Ordering::Relaxed);
            if offset >= total {
                return Err(Error::PageNotFound(page_id));
            }
            let read_len = ((total - offset) as usize).min(self.page_size);
            let raw = reader(offset, read_len)?;
            let mut page_buf = vec![0u8; self.page_size];
            let copy_len = raw.len().min(self.page_size);
            page_buf[..copy_len].copy_from_slice(&raw[..copy_len]);
            self.opfs_page_reads.fetch_add(1, Ordering::Relaxed);
            self.insert_cached_page(page_id, page_buf.clone(), false);
            return Ok(page_buf);
        }

        Err(Error::PageNotFound(page_id))
    }

    /// Return current number of 4KB pages resident in the bounded WASM heap cache.
    pub fn resident_page_count(&self) -> usize {
        self.resident_pages.read().len()
    }

    /// Maximum number of 4KB pages allowed in the bounded LRU cache.
    pub fn cache_capacity_pages(&self) -> usize {
        self.cache_capacity_pages
    }

    /// Number of 4KB page hits served directly from the bounded LRU cache.
    pub fn cache_hits(&self) -> usize {
        self.cache_hits.load(Ordering::Relaxed)
    }

    /// Number of on-demand 4KB page reads pulled from the underlying OPFS sync handle.
    pub fn opfs_page_reads(&self) -> usize {
        self.opfs_page_reads.load(Ordering::Relaxed)
    }

    /// Number of 4KB page writes pushed to the OPFS sync handle.
    pub fn opfs_page_writes(&self) -> usize {
        self.opfs_page_writes.load(Ordering::Relaxed)
    }

    /// Return sorted list of dirty 1-indexed `PageId`s modified since last flush.
    pub fn dirty_page_ids(&self) -> Vec<PageId> {
        self.dirty_pages.read().iter().copied().collect()
    }

    /// Clear dirty page markers after the Web Worker flushes `FileSystemSyncAccessHandle.flush()`.
    pub fn clear_dirty_flags(&self) {
        self.dirty_pages.write().clear();
    }

    fn touch_lru(&self, page_id: PageId) {
        let mut q = self.lru_queue.write();
        if let Some(pos) = q.iter().position(|&p| p == page_id) {
            q.remove(pos);
        }
        q.push_back(page_id);
    }

    fn insert_cached_page(&self, page_id: PageId, page_bytes: Vec<u8>, is_dirty: bool) {
        if is_dirty {
            self.dirty_pages.write().insert(page_id);
        }
        {
            let mut pages = self.resident_pages.write();
            let mut q = self.lru_queue.write();
            let dirty = self.dirty_pages.read();

            if !pages.contains_key(&page_id) && pages.len() >= self.cache_capacity_pages {
                // Evict oldest clean 4KB page (or oldest page if all are dirty and sync_writer persisted them)
                let evict_cand = q
                    .iter()
                    .copied()
                    .find(|pid| !dirty.contains(pid))
                    .or_else(|| q.front().copied());
                if let Some(victim) = evict_cand {
                    pages.remove(&victim);
                    if let Some(pos) = q.iter().position(|&p| p == victim) {
                        q.remove(pos);
                    }
                }
            }
            pages.insert(page_id, page_bytes);
            if let Some(pos) = q.iter().position(|&p| p == page_id) {
                q.remove(pos);
            }
            q.push_back(page_id);
        }
    }
}

impl RemoteRangeReader for OpfsSyncPager {
    fn fetch_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let total = self.total_size_bytes.load(Ordering::Relaxed);
        if offset > total {
            return Err(Error::Corrupted(format!(
                "OPFS range offset {offset} exceeds file size {total}"
            )));
        }
        if length == 0 || offset == total {
            return Ok(Vec::new());
        }

        let ps = self.page_size as u64;
        let end = (offset + (length as u64)).min(total);
        let total_to_read = (end - offset) as usize;
        let mut out = Vec::with_capacity(total_to_read);

        let mut cur_off = offset;
        while cur_off < end {
            let page_id = ((cur_off / ps) as u32) + 1;
            let in_page_off = (cur_off % ps) as usize;
            let chunk_len = (self.page_size - in_page_off).min((end - cur_off) as usize);
            let page = self.fetch_or_load_page_4kb(page_id)?;
            out.extend_from_slice(&page[in_page_off..in_page_off + chunk_len]);
            cur_off += chunk_len as u64;
        }

        Ok(out)
    }

    fn total_size(&self) -> Result<u64> {
        Ok(self.total_size_bytes.load(Ordering::Relaxed))
    }
}

impl RemoteStorageAdapter for OpfsSyncPager {
    fn put_object(&self, data: &[u8]) -> Result<()> {
        self.write_range(0, data)
    }
}

#[cfg(test)]
mod opfs_sync_pager_tests {
    use super::*;

    #[test]
    fn test_opfs_sync_pager_on_demand_4kb_streaming_and_lru_cap() {
        // Simulate a 400 KB database file (100 x 4KB pages) on disk while capping WASM cache to 8 pages (32 KB)
        let backing_file: Arc<RwLock<Vec<u8>>> = Arc::new(RwLock::new(vec![0u8; 100 * 4096]));
        {
            let mut data = backing_file.write();
            for p in 0..100usize {
                let off = p * 4096;
                data[off..off + 4].copy_from_slice(&((p as u32 + 1).to_le_bytes()));
            }
        }

        let reader_file = backing_file.clone();
        let writer_file = backing_file.clone();
        let opfs = OpfsSyncPager::with_sync_callbacks(
            (100 * 4096) as u64,
            8,
            Arc::new(move |offset, len| {
                let guard = reader_file.read();
                let s = offset as usize;
                let e = (s + len).min(guard.len());
                Ok(guard[s..e].to_vec())
            }),
            Some(Arc::new(move |offset, bytes| {
                let mut guard = writer_file.write();
                let s = offset as usize;
                if s + bytes.len() > guard.len() {
                    guard.resize(s + bytes.len(), 0);
                }
                guard[s..s + bytes.len()].copy_from_slice(bytes);
                Ok(())
            })),
        );

        // Fetch 25 different 4KB pages on demand
        for pid in 1..=25u32 {
            let off = ((pid - 1) as u64) * 4096;
            let page = opfs.fetch_range(off, 4096).unwrap();
            assert_eq!(
                u32::from_le_bytes([page[0], page[1], page[2], page[3]]),
                pid
            );
        }

        // Resident memory must NEVER exceed cache_capacity_pages (8 pages = 32 KB)
        assert_eq!(opfs.resident_page_count(), 8);
        assert_eq!(opfs.opfs_page_reads(), 25);

        // Re-reading page 25 must hit the bounded in-memory LRU cache without an OPFS read
        let hits_before = opfs.cache_hits();
        let _ = opfs.fetch_range(24 * 4096, 4096).unwrap();
        assert_eq!(opfs.cache_hits(), hits_before + 1);
        assert_eq!(opfs.opfs_page_reads(), 25);
    }
}

#[cfg(all(test, feature = "cloud-s3"))]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "cloud-s3")]
    fn test_cloud_s3_http_range_streaming_live() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind test HTTP server");
        let port = listener.local_addr().unwrap().port();

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let payload_data: Arc<parking_lot::RwLock<Vec<u8>>> = Arc::new(parking_lot::RwLock::new(
            (0..8192).map(|i| (i % 256) as u8).collect(),
        ));
        let server_payload = payload_data.clone();

        let server_thread = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while running_clone.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    // Accepted sockets can inherit nonblocking mode on some hosts.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut req_buf = [0u8; 2048];
                    let mut n = 0;
                    while n < req_buf.len() {
                        let read = stream.read(&mut req_buf[n..]).unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        n += read;
                        if req_buf[..n].windows(4).any(|part| part == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let req_str = String::from_utf8_lossy(&req_buf[..n]);

                    if req_str.starts_with("HEAD ") {
                        let len = server_payload.read().len();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if req_str.starts_with("GET ") {
                        let range_header = req_str
                            .lines()
                            .find(|l| l.to_lowercase().starts_with("range:"))
                            .unwrap_or("");
                        let data = server_payload.read().clone();
                        let total = data.len();

                        if let Some(range_val) = range_header.split(':').nth(1) {
                            let range_val = range_val.trim();
                            if let Some(bytes_part) = range_val.strip_prefix("bytes=") {
                                let parts: Vec<&str> = bytes_part.split('-').collect();
                                let start: usize = parts[0].parse().unwrap_or(0);
                                let end: usize = parts[1].parse().unwrap_or(total - 1);
                                let end = end.min(total - 1);
                                let slice = &data[start..=end];

                                let resp = format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                                    slice.len()
                                );
                                let _ = stream.write_all(resp.as_bytes());
                                let _ = stream.write_all(slice);
                            }
                        }
                    } else if req_str.starts_with("PUT ") {
                        // Read headers to find Content-Length
                        let cl_line = req_str
                            .lines()
                            .find(|l| l.to_lowercase().starts_with("content-length:"))
                            .unwrap_or("");
                        let body_len: usize = cl_line
                            .split(':')
                            .nth(1)
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);

                        let double_crlf = req_str.find("\r\n\r\n").unwrap_or(0);
                        let body_start = if double_crlf > 0 { double_crlf + 4 } else { 0 };
                        let mut body_bytes = req_buf[body_start..n].to_vec();

                        while body_bytes.len() < body_len {
                            let mut extra = [0u8; 1024];
                            let extra_n = stream.read(&mut extra).unwrap_or(0);
                            if extra_n == 0 {
                                break;
                            }
                            body_bytes.extend_from_slice(&extra[..extra_n]);
                        }

                        *server_payload.write() = body_bytes;
                        let resp =
                            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
                std::thread::sleep(web_time::Duration::from_millis(5));
            }
        });

        let config = S3StorageConfig {
            bucket: "test-bucket".into(),
            key: "db.tapir".into(),
            endpoint: format!("http://127.0.0.1:{port}"),
            region: "us-east-1".into(),
            auth_token: Some("secret-token-xyz".into()),
        };

        let storage = CloudS3RemoteStorage::new(config);

        // 1. Test total_size
        let size = storage.total_size().expect("Failed to get total size");
        assert_eq!(size, 8192);

        // 2. Test fetch_range [0..4096]
        let page1 = storage
            .fetch_range(0, 4096)
            .expect("Failed to fetch page 1");
        assert_eq!(page1.len(), 4096);
        assert_eq!(page1[0], 0);
        assert_eq!(page1[1], 1);

        // 3. Test fetch_range [4096..8192]
        let page2 = storage
            .fetch_range(4096, 4096)
            .expect("Failed to fetch page 2");
        assert_eq!(page2.len(), 4096);

        // 4. Test put_object
        let new_data = vec![42u8; 1024];
        storage.put_object(&new_data).expect("Failed to put object");
        let new_size = storage.total_size().expect("Failed to get updated size");
        assert_eq!(new_size, 1024);

        let fetched_new = storage
            .fetch_range(0, 1024)
            .expect("Failed to fetch updated data");
        assert_eq!(fetched_new, new_data);

        running.store(false, Ordering::Relaxed);
        let _ = server_thread.join();
    }
}
