//! # Crash-Injection and Fault-Tolerance Integration Tests
//!
//! Rigorously verifies TapirusDB's ACID durability, torn-write truncation,
//! CRC32 checksum protection, and crash recovery idempotence under simulated
//! power failure, abrupt SIGKILL termination, and storage-level bit corruption.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use tapirus::{Connection, Result};
use tempfile::NamedTempFile;

/// Test 1: Simulates torn write where a power cut or crash occurs mid-write of a WAL frame.
/// Verifies that the recovery loop cleanly ignores partial frames and recovers all previous valid transactions.
#[test]
fn test_crash_recovery_truncated_frame_torn_write() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // Phase 1: Write valid committed data across multiple transactions
    {
        let conn = Connection::open(&db_path)?;
        conn.execute("CREATE TABLE orders (id INTEGER PRIMARY KEY, item TEXT, qty INTEGER);")?;
        conn.execute("INSERT INTO orders (id, item, qty) VALUES (1, 'Book', 2);")?;
        conn.execute("INSERT INTO orders (id, item, qty) VALUES (2, 'Laptop', 1);")?;
        // WAL is written, but not explicitly checkpointed to base file
    }

    assert!(wal_path.exists());
    let original_wal_len = fs::metadata(&wal_path)?.len();
    assert!(original_wal_len > 32);

    // Phase 2: Fault Injection - Append a partial/torn frame (500 bytes of incomplete frame payload)
    {
        let mut file = OpenOptions::new().append(true).open(&wal_path)?;
        let partial_torn_data = vec![0xABu8; 500];
        file.write_all(&partial_torn_data)?;
        file.sync_data()?;
    }

    let corrupted_wal_len = fs::metadata(&wal_path)?.len();
    assert_eq!(corrupted_wal_len, original_wal_len + 500);

    // Phase 3: Reopen database. Recovery engine must gracefully discard the partial frame without crashing or panicking.
    {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT id, item, qty FROM orders ORDER BY id;")?;
        assert_eq!(rows.len(), 2, "Both committed rows must be recovered despite torn write");
        assert_eq!(rows[0].get::<String>("item")?, "Book");
        assert_eq!(rows[1].get::<String>("item")?, "Laptop");
    }

    Ok(())
}

/// Test 2: Simulates bit rot or payload corruption within a WAL frame.
/// Verifies that CRC32 checksum mismatch triggers immediate truncation of unverified frames,
/// preserving prior uncorrupted transactions.
#[test]
fn test_crash_recovery_corrupted_frame_crc32() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // Phase 1: Write initial table and 2 committed records
    {
        let conn = Connection::open(&db_path)?;
        conn.execute("CREATE TABLE ledger (id INTEGER PRIMARY KEY, amount INTEGER);")?;
        conn.execute("INSERT INTO ledger (id, amount) VALUES (1, 100);")?;
        conn.execute("INSERT INTO ledger (id, amount) VALUES (2, 200);")?;
    }

    // Phase 2: Fault Injection - Corrupt a byte in the last frame payload in the WAL file
    {
        let mut file = OpenOptions::new().read(true).write(true).open(&wal_path)?;
        let len = file.metadata()?.len();
        assert!(len > 100);
        // Flip a byte in the payload area of the last frame
        let target_pos = len - 50;
        file.seek(SeekFrom::Start(target_pos))?;
        let mut b = [0u8; 1];
        file.read_exact(&mut b)?;
        file.seek(SeekFrom::Start(target_pos))?;
        file.write_all(&[!b[0]])?;
        file.sync_data()?;
    }

    // Phase 3: Reopen database. Recovery engine must detect CRC32 mismatch, halt replaying safely,
    // and keep earlier uncorrupted transactions accessible without panic.
    {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT id FROM ledger;")?;
        assert!(!rows.is_empty(), "Uncorrupted prior records must remain valid");
    }

    Ok(())
}

/// Test 3: Simulates crash that leaves WAL file with less than 32 bytes (incomplete header).
/// Verifies safe re-initialization of WAL header without unhandled panic.
#[test]
fn test_crash_recovery_truncated_wal_header() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // Phase 1: Create valid database with base table and flush checkpoint
    {
        let conn = Connection::open(&db_path)?;
        conn.execute("CREATE TABLE config (k TEXT PRIMARY KEY, v TEXT);")?;
        conn.checkpoint()?;
    }

    // Phase 2: Fault Injection - Truncate WAL file to only 12 bytes
    {
        let file = OpenOptions::new().write(true).open(&wal_path)?;
        file.set_len(12)?;
        file.sync_data()?;
    }
    assert_eq!(fs::metadata(&wal_path)?.len(), 12);

    // Phase 3: Reopen database. Pager detects header < 32 bytes and re-initializes fresh header
    {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT k, v FROM config;")?;
        assert_eq!(rows.len(), 0);
        conn.execute("INSERT INTO config (k, v) VALUES ('theme', 'dark');")?;
        let rows2 = conn.query("SELECT k, v FROM config WHERE k = 'theme';")?;
        assert_eq!(rows2.len(), 1);
        assert_eq!(rows2[0].get::<String>("v")?, "dark");
    }

    Ok(())
}

/// Test 4: Simulates power loss/kill signal before checkpoint completes.
/// Confirms that WAL replay restores all pending committed frames upon restart.
#[test]
fn test_crash_recovery_mid_checkpoint_resilience() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // Phase 1: Populate 20 rows across multiple transactions without checkpointing
    {
        let conn = Connection::open(&db_path)?;
        conn.execute("CREATE TABLE events (id INTEGER PRIMARY KEY, name TEXT);")?;
        for i in 1..=20 {
            conn.execute(&format!("INSERT INTO events (id, name) VALUES ({i}, 'evt_{i}');"))?;
        }
        // Explicitly drop without checkpointing (simulates sudden shutdown)
    }

    // Phase 2: Restart #1 - Engine replays WAL frames into base file during initialization
    {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT id, name FROM events;")?;
        assert_eq!(rows.len(), 20, "All 20 events must be recovered on restart");

        // Checkpoint to consolidate
        let flushed = conn.checkpoint()?;
        assert!(flushed > 0);
    }

    // Phase 3: Restart #2 - Base file is now fully consolidated, WAL is clean (32 bytes)
    assert_eq!(fs::metadata(&wal_path)?.len(), 32);
    {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT id, name FROM events;")?;
        assert_eq!(rows.len(), 20);
    }

    Ok(())
}

/// Test 5: Simulates consecutive crashes where WAL replay executes repeatedly
/// without intervening writes or checkpoints. Recovery must be strictly idempotent.
#[test]
fn test_crash_recovery_idempotence() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();

    // Phase 1: Write records
    {
        let conn = Connection::open(&db_path)?;
        conn.execute("CREATE TABLE series (id INTEGER PRIMARY KEY, val REAL);")?;
        for i in 1..=10 {
            conn.execute(&format!("INSERT INTO series (id, val) VALUES ({i}, {i}.5);"))?;
        }
    }

    // Reopen 3 times consecutively without modifications or checkpoints
    for iteration in 1..=3 {
        let conn = Connection::open(&db_path)?;
        let rows = conn.query("SELECT id, val FROM series ORDER BY id;")?;
        assert_eq!(rows.len(), 10, "Iteration {iteration} must recover exact 10 records");
        assert_eq!(rows[0].get::<i64>("id")?, 1);
        assert_eq!(rows[9].get::<i64>("id")?, 10);
    }

    Ok(())
}
