//! Integration tests for Native Page-Level Encryption at Rest (ChaCha20-Poly1305 AEAD).

use std::fs;
use tapirus::{Connection, Result};
use tempfile::NamedTempFile;

#[test]
fn test_encrypted_database_creation_and_reopen() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let path = temp_file.path().to_path_buf();
    let secret_passphrase = "top-secret-mars-rover-mission-key-2026";

    // 1. Create and populate encrypted database
    {
        let db = Connection::open_encrypted(&path, secret_passphrase)?;
        db.execute(
            "CREATE TABLE classified_intel (id INTEGER PRIMARY KEY, codename TEXT, telemetry VECTOR(3));"
        )?;
        db.execute(
            "INSERT INTO classified_intel (id, codename, telemetry) VALUES (1, 'PROJECT_PHOENIX', [0.99, 0.01, 0.50]);"
        )?;
        db.execute(
            "INSERT INTO classified_intel (id, codename, telemetry) VALUES (2, 'NEBULA_DEEP_PROBE', [0.12, 0.88, 0.34]);"
        )?;
        let flushed = db.checkpoint()?;
        assert!(flushed > 0);
    }

    // 2. Physical disk inspection: Plaintext leak audit
    let file_bytes = fs::read(&path).expect("Read database file bytes");
    assert!(file_bytes.len() >= 4096);

    // Verify magic bytes are visible in header
    assert_eq!(&file_bytes[0..8], b"TAPIRUS\0");

    // CRITICAL SECURITY AUDIT: Verify sensitive plaintexts DO NOT appear anywhere on disk!
    let file_str = String::from_utf8_lossy(&file_bytes);
    assert!(!file_str.contains("classified_intel"), "SECURITY LEAK: Table name found in plaintext on disk!");
    assert!(!file_str.contains("PROJECT_PHOENIX"), "SECURITY LEAK: Row data found in plaintext on disk!");
    assert!(!file_str.contains("NEBULA_DEEP_PROBE"), "SECURITY LEAK: Row data found in plaintext on disk!");

    // 3. Attempt reopening WITHOUT password: Must be rejected
    let unauth_attempt = Connection::open(&path);
    assert!(unauth_attempt.is_err(), "Unauthenticated open must fail on encrypted database");

    // 4. Attempt reopening with WRONG password: Must be rejected by KCV
    let wrong_attempt = Connection::open_encrypted(&path, "incorrect-hacker-password");
    assert!(wrong_attempt.is_err(), "Wrong password must be rejected");

    // 5. Reopen with CORRECT password: Must succeed and restore all data
    let db = Connection::open_encrypted(&path, secret_passphrase)?;
    let rows = db.query("SELECT id, codename, telemetry FROM classified_intel;")?;
    assert_eq!(rows.len(), 2);

    let name1: String = rows[0].get("codename")?;
    let name2: String = rows[1].get("codename")?;
    assert_eq!(name1, "PROJECT_PHOENIX");
    assert_eq!(name2, "NEBULA_DEEP_PROBE");

    Ok(())
}

#[test]
fn test_encrypted_database_wal_crash_resilience() -> Result<()> {
    let temp_file = NamedTempFile::new().expect("Create temp file");
    let path = temp_file.path().to_path_buf();
    let secret = "satellite-telemetry-encryption-key-77";

    // 1. Write frames to WAL without manual checkpoint
    {
        let db = Connection::open_encrypted(&path, secret)?;
        db.execute("CREATE TABLE sensors (id INTEGER PRIMARY KEY, reading TEXT);")?;
        for i in 1..=10 {
            db.execute(&format!("INSERT INTO sensors (id, reading) VALUES ({i}, 'SENSOR_CONFIDENTIAL_{i}');"))?;
        }
        // Connection drops here simulating sudden process termination with active WAL
    }

    // 2. Audit WAL file on disk
    let wal_path = path.with_extension("tapir-wal");
    assert!(wal_path.exists(), "WAL file must exist");
    let wal_bytes = fs::read(&wal_path).expect("Read WAL bytes");
    let wal_str = String::from_utf8_lossy(&wal_bytes);
    assert!(!wal_str.contains("SENSOR_CONFIDENTIAL_5"), "SECURITY LEAK: WAL contains plaintext data!");

    // 3. Reopen encrypted database: WAL recovery must replay encrypted frames correctly
    let db = Connection::open_encrypted(&path, secret)?;
    let rows = db.query("SELECT id, reading FROM sensors;")?;
    assert_eq!(rows.len(), 10);
    let val5: String = rows[4].get("reading")?;
    assert_eq!(val5, "SENSOR_CONFIDENTIAL_5");

    Ok(())
}
