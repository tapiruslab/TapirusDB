//! Database Backup, VACUUM, and WAL Replication Tests for TapirusDB
//!
//! Validates:
//! - In-place VACUUM defragmentation
//! - Online hot backup via `VACUUM INTO 'target.tapir'`
//! - Native `Connection::backup(&path)` API
//! - Streaming WAL frame export and replica absorption

use tapirus::Connection;
use tempfile::tempdir;

#[test]
fn test_vacuum_and_vacuum_into_hot_backup() {
    let dir = tempdir().expect("Create temp dir");
    let primary_db = dir.path().join("primary.tapir");
    let backup_db = dir.path().join("backup.tapir");

    // 1. Open primary database and insert records
    let conn = Connection::open(&primary_db).expect("Open primary db");
    conn.execute("CREATE TABLE sensory (id INTEGER PRIMARY KEY, sensor TEXT, value REAL);").unwrap();
    conn.execute("INSERT INTO sensory (id, sensor, value) VALUES (1, 'temp_c', 24.5);").unwrap();
    conn.execute("INSERT INTO sensory (id, sensor, value) VALUES (2, 'humidity_pct', 65.0);").unwrap();

    // 2. In-place VACUUM
    let _ = conn.vacuum().expect("Vacuum in place");

    // 3. VACUUM INTO backup file
    let backup_str = backup_db.to_str().unwrap().replace('\\', "/");
    let sql_vacuum = format!("VACUUM INTO '{backup_str}';");
    conn.execute(&sql_vacuum).expect("Vacuum into backup");

    assert!(backup_db.exists());

    // 4. Open backup database and verify all data is intact
    let backup_conn = Connection::open(&backup_db).expect("Open backup db");
    let rows = backup_conn.query("SELECT id, sensor, value FROM sensory ORDER BY id ASC;").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String>("sensor").unwrap(), "temp_c");
    assert_eq!(rows[1].get::<f64>("value").unwrap(), 65.0);
}

#[test]
fn test_connection_backup_api() {
    let dir = tempdir().expect("Create temp dir");
    let primary_db = dir.path().join("live.tapir");
    let snapshot_db = dir.path().join("snapshot.tapir");

    let conn = Connection::open(&primary_db).expect("Open live db");
    conn.execute("CREATE TABLE config (key TEXT, val TEXT);").unwrap();
    conn.execute("INSERT INTO config (key, val) VALUES ('model', 'deepseek-r1-qwen-7b');").unwrap();

    // Direct backup API call
    let total_pages = conn.backup(&snapshot_db).expect("Connection backup");
    assert!(total_pages >= 1);
    assert!(snapshot_db.exists());

    let snap_conn = Connection::open(&snapshot_db).expect("Open snapshot db");
    let rows = snap_conn.query("SELECT key, val FROM config;").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("val").unwrap(), "deepseek-r1-qwen-7b");
}

#[test]
fn test_streaming_wal_replication_primitives() {
    let dir = tempdir().expect("Create temp dir");
    let master_db = dir.path().join("master.tapir");
    let replica_db = dir.path().join("replica.tapir");

    // Master node
    let master = Connection::open(&master_db).expect("Open master db");
    master.execute("CREATE TABLE telemetry (id INTEGER PRIMARY KEY, msg TEXT);").unwrap();
    master.execute("INSERT INTO telemetry (id, msg) VALUES (1, 'Master Node Alive');").unwrap();

    // Snapshot master to replica
    master.backup(&replica_db).expect("Initial replica sync");

    // Open replica node
    let replica = Connection::open(&replica_db).expect("Open replica db");
    let rep_rows = replica.query("SELECT msg FROM telemetry WHERE id = 1;").unwrap();
    assert_eq!(rep_rows[0].get::<String>("msg").unwrap(), "Master Node Alive");

    // Master writes new data (accumulating in WAL)
    master.execute("INSERT INTO telemetry (id, msg) VALUES (2, 'Replicated Event');").unwrap();

    // Export WAL frames from master
    let frames = master.export_wal_frames().expect("Export WAL frames");
    assert!(!frames.is_empty(), "Expected WAL frames from master insert");

    // Apply WAL frames directly to replica
    for (page_id, data) in &frames {
        replica.apply_wal_frame(*page_id, data).expect("Apply WAL frame to replica");
    }

    // Verify replica immediately observes replicated change
    let rep_check = replica.query("SELECT msg FROM telemetry WHERE id = 2;").unwrap();
    assert_eq!(rep_check.len(), 1);
    assert_eq!(rep_check[0].get::<String>("msg").unwrap(), "Replicated Event");
}
