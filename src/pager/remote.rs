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
        let agent = ureq::AgentBuilder::new()
            .timeout_read(std::time::Duration::from_secs(30))
            .timeout_write(std::time::Duration::from_secs(30))
            .build();
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
        let mut req = self.agent.get(&url).set("Range", &range_header);

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.set("Authorization", token);
            } else {
                req = req.set("Authorization", &format!("Bearer {token}"));
            }
        }

        let resp = req.call().map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Cloud S3 range request failed for {url} [Range: {range_header}]: {e}"),
            ))
        })?;

        let mut reader = resp.into_reader();
        let mut buf = Vec::with_capacity(length);
        std::io::Read::read_to_end(&mut reader, &mut buf)?;
        Ok(buf)
    }

    fn total_size(&self) -> Result<u64> {
        let url = self.object_url();
        let mut req = self.agent.head(&url);

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.set("Authorization", token);
            } else {
                req = req.set("Authorization", &format!("Bearer {token}"));
            }
        }

        if let Ok(resp) = req.call() {
            if let Some(cl) = resp.header("content-length") {
                if let Ok(size) = cl.parse::<u64>() {
                    return Ok(size);
                }
            }
        }

        // Fallback: request 1-byte range to inspect Content-Range header
        let mut req_range = self.agent.get(&url).set("Range", "bytes=0-0");
        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req_range = req_range.set("Authorization", token);
            } else {
                req_range = req_range.set("Authorization", &format!("Bearer {token}"));
            }
        }

        let resp = req_range.call().map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to determine S3 object size for {url}: {e}"),
            ))
        })?;

        if let Some(cr) = resp.header("content-range") {
            if let Some(slash_idx) = cr.rfind('/') {
                let total_str = &cr[slash_idx + 1..];
                if let Ok(size) = total_str.trim().parse::<u64>() {
                    return Ok(size);
                }
            }
        }

        if let Some(cl) = resp.header("content-length") {
            if let Ok(size) = cl.parse::<u64>() {
                return Ok(size);
            }
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
        let mut req = self.agent.put(&url).set("Content-Type", "application/octet-stream");

        if let Some(ref token) = self.config.auth_token {
            if token.starts_with("Bearer ") || token.starts_with("AWS ") {
                req = req.set("Authorization", token);
            } else {
                req = req.set("Authorization", &format!("Bearer {token}"));
            }
        }

        req.send_bytes(data).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Cloud S3 PUT request failed for {url}: {e}"),
            ))
        })?;
        Ok(())
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
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

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
                    let mut req_buf = [0u8; 2048];
                    let n = stream.read(&mut req_buf).unwrap_or(0);
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
                        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
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
        let page1 = storage.fetch_range(0, 4096).expect("Failed to fetch page 1");
        assert_eq!(page1.len(), 4096);
        assert_eq!(page1[0], 0);
        assert_eq!(page1[1], 1);

        // 3. Test fetch_range [4096..8192]
        let page2 = storage.fetch_range(4096, 4096).expect("Failed to fetch page 2");
        assert_eq!(page2.len(), 4096);

        // 4. Test put_object
        let new_data = vec![42u8; 1024];
        storage.put_object(&new_data).expect("Failed to put object");
        let new_size = storage.total_size().expect("Failed to get updated size");
        assert_eq!(new_size, 1024);

        let fetched_new = storage.fetch_range(0, 1024).expect("Failed to fetch updated data");
        assert_eq!(fetched_new, new_data);

        running.store(false, Ordering::Relaxed);
        let _ = server_thread.join();
    }
}

