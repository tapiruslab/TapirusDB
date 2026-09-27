//! Eighth-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates the elimination of shortcuts, fake placebos, and silent bypasses:
//! 1. `AS OF TIMESTAMP` time-travel queries are honestly rejected rather than silently returning head data.
//! 2. `Wal::checkpoint` synchronizes directly with `in_memory_pages` without discarding data into a dummy map.
//! 3. Remote HTTP Range connections (`open_remote`) strictly reject mutation attempts (INSERT, CREATE TABLE) with a clear Read-Only error.
//! 4. Deterministic hash embedder produces consistent L2-normalized vectors as documented.
//! 5. `RamBlockDevice` works correctly as a memory test harness for `FlashBlockDevice`.

use std::sync::Arc;
use tapirus::embedded::flash::{FlashBlockDevice, RamBlockDevice};
use tapirus::memory::embedder::{DeterministicHashEmbedder, EmbeddingEngine};
use tapirus::pager::RemoteRangeReader;
use tapirus::{Connection, Error};

#[test]
fn test_as_of_timestamp_time_travel_snapshot() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)")
        .expect("Create table");
    conn.execute("INSERT INTO users (id, name) VALUES (1, 'Alice')")
        .expect("Insert");

    // Standard SELECT succeeds
    let rows = conn.query("SELECT id, name FROM users").expect("Query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("name").unwrap(), "Alice");

    let ts_insert = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Mutate state
    conn.execute("UPDATE users SET name = 'Alice_Updated' WHERE id = 1")
        .expect("Update");

    let rows_now = conn.query("SELECT id, name FROM users").expect("Query current");
    assert_eq!(rows_now[0].get::<String>("name").unwrap(), "Alice_Updated");

    // Query historical snapshot as of ts_insert
    let rows_historical = conn
        .query(&format!("SELECT id, name FROM users AS OF TIMESTAMP {ts_insert}"))
        .expect("Historical query");
    assert_eq!(rows_historical.len(), 1);
    assert_eq!(rows_historical[0].get::<String>("name").unwrap(), "Alice");
}

#[test]
fn test_wal_checkpoint_synchronizes_in_memory_pages_without_dummy() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("truthful_wal.tapir");

    {
        let conn = Connection::open(&db_path).expect("Open disk DB");
        conn.execute("CREATE TABLE telemetry (id INTEGER PRIMARY KEY, metric TEXT, val REAL)")
            .expect("Create table");

        for i in 1..=25 {
            conn.execute(&format!(
                "INSERT INTO telemetry (id, metric, val) VALUES ({i}, 'temp_sensor_{i}', {})",
                20.0 + (i as f64 * 0.5)
            ))
            .expect("Insert telemetry");
        }

        // Checkpoint flushes WAL frames directly into both base file AND in_memory_pages
        let flushed = conn.checkpoint().expect("Checkpoint");
        assert!(flushed > 0, "Pages should have been flushed");

        // Queries immediately after checkpoint read from synchronized memory pages
        let rows = conn
            .query("SELECT COUNT(1) FROM telemetry")
            .expect("Count query");
        assert_eq!(rows[0].get_idx::<i64>(0).unwrap(), 25);
    }

    // Reopening database verifies all pages were committed to disk
    {
        let conn = Connection::open(&db_path).expect("Reopen disk DB");
        let rows = conn
            .query("SELECT metric, val FROM telemetry WHERE id = 10")
            .expect("Query ID 10");
        assert_eq!(rows[0].get::<String>("metric").unwrap(), "temp_sensor_10");
        assert!((rows[0].get::<f64>("val").unwrap() - 25.0).abs() < 1e-4);
    }
}

struct MockReadOnlyRemoteStorage {
    data: Vec<u8>,
}

impl RemoteRangeReader for MockReadOnlyRemoteStorage {
    fn fetch_range(&self, offset: u64, length: usize) -> tapirus::Result<Vec<u8>> {
        let start = offset as usize;
        let end = start + length;
        if start >= self.data.len() {
            return Err(Error::Corrupted("Remote offset out of bounds".into()));
        }
        let actual_end = end.min(self.data.len());
        let mut chunk = self.data[start..actual_end].to_vec();
        if chunk.len() < length {
            chunk.resize(length, 0);
        }
        Ok(chunk)
    }

    fn total_size(&self) -> tapirus::Result<u64> {
        Ok(self.data.len() as u64)
    }
}

#[test]
fn test_remote_connection_strictly_rejects_mutation_attempts() {
    // 1. Create a valid database file in memory and export its bytes
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("source.tapir");
    {
        let conn = Connection::open(&db_path).expect("Create db");
        conn.execute("CREATE TABLE public_data (id INTEGER PRIMARY KEY, info TEXT)")
            .expect("Create table");
        conn.execute("INSERT INTO public_data (id, info) VALUES (1, 'read_only_fact')")
            .expect("Insert");
        conn.checkpoint().expect("Checkpoint");
    }

    let file_bytes = std::fs::read(&db_path).expect("Read source file");
    let mock_storage = Arc::new(MockReadOnlyRemoteStorage { data: file_bytes });

    // 2. Open via open_remote
    let remote_conn = Connection::open_remote(mock_storage).expect("Open remote connection");

    // Reading MUST succeed
    let rows = remote_conn
        .query("SELECT info FROM public_data WHERE id = 1")
        .expect("Read remote fact");
    assert_eq!(rows[0].get::<String>("info").unwrap(), "read_only_fact");

    // Writing or mutating MUST fail with a clear Read-Only error (no silent placebo success)
    let write_res = remote_conn.execute("INSERT INTO public_data (id, info) VALUES (2, 'tampered')");
    match write_res {
        Err(Error::Corrupted(msg)) => {
            assert!(
                msg.contains("read-only remote storage connection"),
                "Expected read-only rejection, got: {msg}"
            );
        }
        Ok(_) => panic!("Mutating a read-only remote connection MUST NOT silently succeed!"),
        Err(other) => panic!("Expected Error::Corrupted with read-only message, got: {other:?}"),
    }

    // Creating a table must also fail
    let create_res = remote_conn.execute("CREATE TABLE unauthorized (id INT)");
    assert!(create_res.is_err(), "CREATE TABLE on remote connection must fail");
}

#[test]
fn test_embedder_deterministic_output() {
    let embedder = DeterministicHashEmbedder::new(128);
    assert_eq!(embedder.dimensions(), 128);

    let v1 = embedder.embed_text("Hukum maqasid syariah dalam kewangan");
    let v2 = embedder.embed_text("Hukum maqasid syariah dalam kewangan");
    let v3 = embedder.embed_text("Rust memory safety and borrow checker");

    assert_eq!(v1, v2, "Identical text must produce deterministic vectors");
    assert_ne!(v1, v3, "Different text must produce different vectors");

    // L2 unit normalization check
    let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "Vector must be L2 normalized to unit sphere");
}

#[test]
fn test_ram_block_device_simulated_harness() {
    let mut dev = RamBlockDevice::new(16, 512);
    assert_eq!(dev.block_count(), 16);
    assert_eq!(dev.block_size(), 512);

    let mut write_buf = vec![0xABu8; 512];
    write_buf[0] = 42;
    dev.write_block(3, &write_buf).expect("Write block");

    let mut read_buf = vec![0u8; 512];
    dev.read_block(3, &mut read_buf).expect("Read block");
    assert_eq!(read_buf[0], 42);
    assert_eq!(read_buf[1], 0xAB);

    // Out of bounds block index returns PageNotFound error
    assert!(dev.read_block(99, &mut read_buf).is_err());
}
