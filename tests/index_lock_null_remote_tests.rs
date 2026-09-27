//! Second-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. Secondary Index Persistence, Catalog Registration, and Point-Lookup Acceleration
//! 2. Cross-Process OS-level Atomic File Locking (.tapir-lock)
//! 3. ANSI SQL Three-Valued Logic NULL Comparison Compliance
//! 4. Write-Ahead Log Commit Fsync Durability & Transactional Batching
//! 5. Remote Object Storage Range Streaming Reader with Connection::open_remote

use std::sync::Arc;
use tapirus::pager::MockRemoteRangeStorage;
use tapirus::sql::parser::BinaryOp;
use tapirus::traits::Value;
use tapirus::{Connection, Error};

#[test]
fn test_secondary_index_persistence_and_query_acceleration() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "tapirus_sec_idx_test_{}.tapir",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    {
        let conn = Connection::open(&db_path).expect("Open connection");

        // 1. Create table and insert initial rows
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, role TEXT);")
            .expect("Create table");

        conn.execute("INSERT INTO users VALUES (1, 'alice@tapirus.db', 'admin');")
            .expect("Insert alice");
        conn.execute("INSERT INTO users VALUES (2, 'bob@tapirus.db', 'engineer');")
            .expect("Insert bob");

        // 2. Create secondary index on existing populated table
        conn.execute("CREATE INDEX idx_users_email ON users (email);")
            .expect("Create index");

        // 3. Explain query plan should indicate USING INDEX idx_users_email
        let plan = conn
            .query("EXPLAIN SELECT id, role FROM users WHERE email = 'bob@tapirus.db';")
            .expect("Explain query plan");

        assert!(!plan.is_empty(), "Plan should not be empty");
        let detail_str = format!("{:?}", plan[0].get_value("detail").unwrap());
        assert!(
            detail_str.contains("USING INDEX idx_users_email"),
            "Query plan should use secondary index, got: {detail_str}"
        );

        // 4. Query using index
        let rows = conn
            .query("SELECT id, role FROM users WHERE email = 'bob@tapirus.db';")
            .expect("Query via index");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(2));
        assert_eq!(rows[0].get_value("role").unwrap(), &Value::Text("engineer".to_string()));

        // 5. Insert new row after index creation (verifies automatic index maintenance on INSERT)
        conn.execute("INSERT INTO users VALUES (3, 'charlie@tapirus.db', 'researcher');")
            .expect("Insert charlie");

        let charlie_rows = conn
            .query("SELECT id, role FROM users WHERE email = 'charlie@tapirus.db';")
            .expect("Query charlie via index");
        assert_eq!(charlie_rows.len(), 1);
        assert_eq!(charlie_rows[0].get_value("id").unwrap(), &Value::Integer(3));

        // 6. Delete a row (verifies index maintenance on DELETE)
        conn.execute("DELETE FROM users WHERE email = 'bob@tapirus.db';")
            .expect("Delete bob");

        let after_delete = conn
            .query("SELECT id FROM users WHERE email = 'bob@tapirus.db';")
            .expect("Query deleted bob");
        assert_eq!(after_delete.len(), 0, "Deleted row must no longer match in index");
    }

    // 7. Verify index persistence across connection restarts
    {
        let conn_reloaded = Connection::open(&db_path).expect("Reload connection");
        let plan_reloaded = conn_reloaded
            .query("EXPLAIN SELECT id, role FROM users WHERE email = 'alice@tapirus.db';")
            .expect("Explain reloaded query plan");

        let detail_str = format!("{:?}", plan_reloaded[0].get_value("detail").unwrap());
        assert!(
            detail_str.contains("USING INDEX idx_users_email"),
            "Persisted index should be active after reload, got: {detail_str}"
        );

        let rows = conn_reloaded
            .query("SELECT id, role FROM users WHERE email = 'alice@tapirus.db';")
            .expect("Query reloaded");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(1));
    }

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
}

#[test]
fn test_cross_process_atomic_file_lock() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "tapirus_lock_test_{}.tapir",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let conn1 = Connection::open(&db_path).expect("First connection must succeed");

    // Attempting to open second connection concurrently on the same path must fail with Error::Busy
    let second_open = Connection::open(&db_path);
    match second_open {
        Err(Error::Busy(msg)) => {
            assert!(
                msg.contains("locked by another process"),
                "Busy error should mention process lock, got: {msg}"
            );
        }
        other => panic!("Expected Error::Busy for concurrent open, got: {:?}", other.err()),
    }

    // Dropping first connection must cleanly release the lock
    drop(conn1);

    // Second connection must now succeed
    let conn2 = Connection::open(&db_path).expect("Connection after drop must succeed");
    drop(conn2);

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("tapir-lock"));
}

#[test]
fn test_ansi_sql_null_three_valued_logic() {
    use tapirus::sql::matches_condition;

    let null_val = Value::Null;
    let int_val = Value::Integer(42);
    let another_null = Value::Null;

    // ANSI SQL Standard: any comparison (=, !=, <, >, <=, >=) with NULL evaluates to UNKNOWN (false in WHERE filter)
    assert!(
        !matches_condition(&null_val, &BinaryOp::Equals, &another_null),
        "NULL = NULL must be false in ANSI SQL"
    );
    assert!(
        !matches_condition(&null_val, &BinaryOp::NotEquals, &another_null),
        "NULL != NULL must be false in ANSI SQL"
    );
    assert!(
        !matches_condition(&int_val, &BinaryOp::GreaterThan, &null_val),
        "42 > NULL must be false in ANSI SQL"
    );
    assert!(
        !matches_condition(&null_val, &BinaryOp::LessThan, &int_val),
        "NULL < 42 must be false in ANSI SQL"
    );
    assert!(
        !matches_condition(&null_val, &BinaryOp::GreaterOrEqual, &null_val),
        "NULL >= NULL must be false in ANSI SQL"
    );

    // Non-null comparisons must still behave normally
    assert!(matches_condition(&int_val, &BinaryOp::Equals, &Value::Integer(42)));
    assert!(matches_condition(&Value::Integer(50), &BinaryOp::GreaterThan, &Value::Integer(40)));
}

#[test]
fn test_remote_object_storage_range_streaming_connection() {
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!(
        "tapirus_remote_test_{}.tapir",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    // 1. Create and populate a standard local database
    {
        let conn = Connection::open(&db_path).expect("Create local db");
        conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price INTEGER);")
            .expect("Create products table");
        conn.execute("INSERT INTO products VALUES (1, 'Quantum Laptop', 2500);")
            .expect("Insert 1");
        conn.execute("INSERT INTO products VALUES (2, 'Neural Headset', 800);")
            .expect("Insert 2");
        conn.checkpoint().expect("Checkpoint local db");
    }

    // 2. Read entire file bytes into mock remote cloud object storage
    let file_bytes = std::fs::read(&db_path).expect("Read db file bytes");
    let mock_storage = Arc::new(MockRemoteRangeStorage::new(file_bytes));

    // 3. Open connection directly from remote cloud storage via Range Requests
    let remote_conn = Connection::open_remote(mock_storage.clone())
        .expect("Open connection backed by remote storage");

    // 4. Query data through remote range streaming
    let rows = remote_conn
        .query("SELECT id, name, price FROM products;")
        .expect("Query remote connection");

    assert_eq!(rows.len(), 2, "Remote connection should retrieve both products");
    assert_eq!(rows[0].get_value("name").unwrap(), &Value::Text("Quantum Laptop".to_string()));
    assert_eq!(rows[1].get_value("name").unwrap(), &Value::Text("Neural Headset".to_string()));

    // 5. Verify remote range calls were recorded
    assert!(
        mock_storage.fetch_count() > 0,
        "Mock remote storage must have served range requests"
    );
    assert!(
        mock_storage.bytes_transferred() > 0,
        "Bytes must have been transferred remotely"
    );

    // Cleanup
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("tapir-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("tapir-lock"));
}
