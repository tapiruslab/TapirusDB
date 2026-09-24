//! Integration tests for Write-Ahead Log (WAL), MVCC, and Crash Resilience.

use std::fs;
use tapirus::{Connection, Result};
use tempfile::NamedTempFile;

#[test]
fn test_wal_file_creation_and_checkpoint() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // 1. Open database and write data
    {
        let db = Connection::open(&db_path)?;

        db.execute(
            "CREATE TABLE sensor_logs (
                id INTEGER PRIMARY KEY,
                sensor TEXT NOT NULL,
                reading REAL
            );",
        )?;

        db.execute("INSERT INTO sensor_logs (id, sensor, reading) VALUES (1, 'Temp-01', 23.5);")?;
        db.execute("INSERT INTO sensor_logs (id, sensor, reading) VALUES (2, 'Temp-02', 24.1);")?;

        // Verify WAL file was created and contains frames
        assert!(wal_path.exists(), "WAL file should exist on disk");
        let wal_len = fs::metadata(&wal_path)?.len();
        // 32-byte header + at least 1 frame (24 + 4096 = 4120 bytes)
        assert!(wal_len > 32, "WAL file should contain written frames");

        // Query returns data directly from WAL (MVCC)
        let rows = db.query("SELECT sensor, reading FROM sensor_logs WHERE id = 1;")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String>("sensor")?, "Temp-01");

        // Manually trigger checkpoint
        let flushed = db.checkpoint()?;
        assert!(flushed > 0, "Checkpoint should flush frames to base file");

        // After checkpoint, WAL is truncated back to its 32-byte header
        let wal_len_after = fs::metadata(&wal_path)?.len();
        assert_eq!(wal_len_after, 32, "WAL file should be truncated to 32-byte header after checkpoint");
    }

    // 2. Reopen database from disk and verify data persists in base file
    {
        let db = Connection::open(&db_path)?;
        let rows = db.query("SELECT sensor, reading FROM sensor_logs;")?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<String>("sensor")?, "Temp-01");
        assert_eq!(rows[1].get::<String>("sensor")?, "Temp-02");
    }

    Ok(())
}

#[test]
fn test_wal_crash_recovery_simulation() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let db_path = temp_file.path().to_path_buf();
    let wal_path = db_path.with_extension("tapir-wal");

    // 1. Session 1: Write records, but DO NOT checkpoint before dropping connection (simulating crash)
    {
        let db = Connection::open(&db_path)?;

        db.execute(
            "CREATE TABLE telemetry (
                id INTEGER PRIMARY KEY,
                metric TEXT NOT NULL,
                val INTEGER
            );",
        )?;

        for i in 1..=10 {
            let sql = format!(
                "INSERT INTO telemetry (id, metric, val) VALUES ({i}, 'telemetry_metric', {i});"
            );
            db.execute(&sql)?;
        }

        // Data was written to WAL, connection drops without explicit checkpoint
    }

    // Verify WAL still exists with frames
    assert!(wal_path.exists());
    assert!(fs::metadata(&wal_path)?.len() > 32);

    // 2. Session 2: Recovery on startup — open connection afresh
    {
        let db = Connection::open(&db_path)?;

        // All 10 committed records should be fully recovered and visible
        let rows = db.query("SELECT id, val FROM telemetry;")?;
        assert_eq!(rows.len(), 10, "All 10 records must be recovered from WAL");

        for (idx, row) in rows.iter().enumerate() {
            let id: i64 = row.get("id")?;
            assert_eq!(id, (idx + 1) as i64);
        }
    }

    Ok(())
}
