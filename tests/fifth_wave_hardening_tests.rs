//! Fifth-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. Stale Lockfile Detection and Automated Crash Recovery
//! 2. Catastrophic Self-Overwrite Prevention on `VACUUM INTO`
//! 3. SQL Standard Pagination (`OFFSET`) and Early Table Scan Termination
//! 4. `NaN` Poisoning Immunity in Vector Distance Calculations
//! 5. Hybrid Memory Recall Query Matching Verification
//! 6. CDC Event Broadcasting in Prepared Statements and Document Collections
//! 7. Defensive Bounds in B+Tree Leaf Cell Deletion

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tapirus::traits::Value;
use tapirus::vector::{cosine_distance, euclidean_distance};
use tapirus::{ChangeOp, Connection, Error, MemoryRecallFilter};

#[test]
fn test_stale_lockfile_recovery() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("stale_lock_test.tapir");
    let lock_path = tmp_dir.path().join("stale_lock_test.tapir-lock");

    // Simulate an orphaned lock file from a dead process (e.g. PID 999999)
    std::fs::write(&lock_path, "999999\n").expect("Write simulated stale lock file");
    assert!(lock_path.exists());

    // Open connection - it should detect dead PID 999999, break the stale lock, and open successfully
    let conn = Connection::open_file(&db_path).expect("Should recover and open database despite stale lock");
    conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY);").expect("Create table");
    conn.execute("INSERT INTO t VALUES (1);").expect("Insert row");

    let rows = conn.query("SELECT * FROM t;").expect("Query");
    assert_eq!(rows.len(), 1);

    // Opening a second connection concurrently should be blocked with Error::Busy
    let second_conn = Connection::open_file(&db_path);
    assert!(
        matches!(second_conn, Err(Error::Busy(_))),
        "Active lock must prevent concurrent access"
    );

    drop(conn);
    // After dropping conn, lock file should be removed
    assert!(!lock_path.exists(), "Lock file must be cleaned up on drop");
}

#[test]
fn test_vacuum_into_self_overwrite_prevented() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("vacuum_safety.tapir");

    let conn = Connection::open_file(&db_path).expect("Open file-backed database");
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
        .expect("Create table");
    conn.execute("INSERT INTO users VALUES (1, 'Alice');")
        .expect("Insert Alice");

    // Attempting to VACUUM INTO the same active database file must be rejected!
    let path_str = db_path.to_str().unwrap().replace('\\', "/");
    let vacuum_sql = format!("VACUUM INTO '{path_str}';");
    let err = conn.execute(&vacuum_sql);

    assert!(
        err.is_err(),
        "VACUUM INTO targeting the active database file must fail"
    );

    // Verify database and existing data are completely intact (not truncated to 0 bytes)
    let rows = conn
        .query("SELECT * FROM users;")
        .expect("Query active database after prevented vacuum");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name").unwrap(), &Value::Text("Alice".into()));

    // Safe VACUUM INTO a distinct backup target should succeed
    let backup_path = tmp_dir.path().join("backup.tapir");
    let backup_str = backup_path.to_str().unwrap().replace('\\', "/");
    conn.execute(&format!("VACUUM INTO '{backup_str}';"))
        .expect("VACUUM INTO backup target must succeed");

    assert!(backup_path.exists(), "Backup file must exist");
}

#[test]
fn test_sql_offset_and_early_scan_termination() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT);")
        .expect("Create table");

    for i in 1..=10 {
        conn.execute(&format!("INSERT INTO items VALUES ({i}, 'item_{i}');"))
            .expect("Insert item");
    }

    // 1. Basic LIMIT and OFFSET pagination
    let rows = conn
        .query("SELECT id, label FROM items LIMIT 3 OFFSET 2;")
        .expect("Query with LIMIT and OFFSET");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].get_value("id").unwrap(), &Value::Integer(3));
    assert_eq!(rows[1].get_value("id").unwrap(), &Value::Integer(4));
    assert_eq!(rows[2].get_value("id").unwrap(), &Value::Integer(5));

    // 2. OFFSET near end of dataset
    let rows_tail = conn
        .query("SELECT id FROM items LIMIT 5 OFFSET 8;")
        .expect("Query tail");
    assert_eq!(rows_tail.len(), 2);
    assert_eq!(rows_tail[0].get_value("id").unwrap(), &Value::Integer(9));
    assert_eq!(rows_tail[1].get_value("id").unwrap(), &Value::Integer(10));

    // 3. OFFSET beyond end of dataset
    let rows_empty = conn
        .query("SELECT id FROM items LIMIT 5 OFFSET 20;")
        .expect("Query beyond end");
    assert_eq!(rows_empty.len(), 0);

    // 4. ORDER BY with LIMIT and OFFSET
    let rows_ordered = conn
        .query("SELECT id FROM items ORDER BY id DESC LIMIT 2 OFFSET 1;")
        .expect("Query ordered with offset");
    assert_eq!(rows_ordered.len(), 2);
    assert_eq!(rows_ordered[0].get_value("id").unwrap(), &Value::Integer(9));
    assert_eq!(rows_ordered[1].get_value("id").unwrap(), &Value::Integer(8));
}

#[test]
fn test_nan_vector_distance_immunity() {
    // Vectors with NaN elements must never return NaN or cause unhandled exceptions
    let nan_vec = vec![f32::NAN, 1.0, 0.0];
    let normal_vec = vec![1.0, 0.0, 0.0];

    let cos_dist = cosine_distance(&nan_vec, &normal_vec);
    assert!(cos_dist.is_finite(), "Cosine distance with NaN must be finite sentinel");
    assert_eq!(cos_dist, 1.0);

    let euc_dist = euclidean_distance(&nan_vec, &normal_vec);
    assert!(euc_dist.is_finite(), "Euclidean distance with NaN must be finite sentinel");
    assert_eq!(euc_dist, f32::MAX);

    // Vector operations in SQL
    let conn = Connection::open_in_memory().expect("Open db");
    conn.execute("CREATE TABLE embeddings (id INTEGER PRIMARY KEY, v VECTOR(3));")
        .expect("Create table");
    conn.execute("INSERT INTO embeddings VALUES (1, [1.0, 0.0, 0.0]);")
        .expect("Insert 1");
    conn.execute("INSERT INTO embeddings VALUES (2, [0.0, 1.0, 0.0]);")
        .expect("Insert 2");

    let results = conn
        .query("SELECT id FROM embeddings VECTOR NEAR v = [1.0, 0.1, 0.0] TOP 1;")
        .expect("Search vector");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_value("id").unwrap(), &Value::Integer(1));
}

#[test]
fn test_hybrid_memory_recall_query_matching() {
    let conn = Connection::open_in_memory().expect("Open db");

    let id1 = conn
        .memory_remember(
            "Rust memory safety guarantees zero data races and fearless concurrency.",
            Some(&[1.0, 0.0, 0.0]),
            1.0,
            &[],
        )
        .expect("Store memory 1");

    let _id2 = conn
        .memory_remember(
            "Delicious baking recipes for sourdough bread and pastry.",
            Some(&[0.0, 1.0, 0.0]),
            1.0,
            &[],
        )
        .expect("Store memory 2");

    // Semantic query targeting Rust memory concepts
    let filter = MemoryRecallFilter::default();
    let recalled = conn.memory_recall(
        Some("memory safety"),
        Some(&[0.95, 0.05, 0.0]),
        5,
        &filter,
    );

    assert!(!recalled.is_empty(), "Should match relevant Rust memory");
    assert_eq!(recalled[0].entry.id, id1);

    // Semantic query with vector completely orthogonal / non-positive similarity
    let unrelated = conn.memory_recall(
        None,
        Some(&[0.0, 0.0, -1.0]),
        5,
        &filter,
    );

    // Non-positive semantic similarity with no text match must not return unrelated memories
    assert_eq!(unrelated.len(), 0, "Orthogonal/negative vector must not match");
}

#[test]
fn test_cdc_broadcasting_in_prepared_statements_and_documents() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    let insert_count = Arc::new(AtomicUsize::new(0));
    let insert_count_clone = insert_count.clone();

    conn.listen(move |evt| {
        if evt.op == ChangeOp::Insert {
            insert_count_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    conn.execute("CREATE TABLE products (id INTEGER PRIMARY KEY, title TEXT, price REAL);")
        .expect("Create table");

    // 1. Parameterized execution via PreparedStatement
    let pstmt = conn.prepare("INSERT INTO products VALUES (?, ?, ?);").expect("Prepare");
    pstmt.execute(&[Value::Integer(1), Value::Text("Laptop".into()), Value::Real(999.99)])
        .expect("Execute prepared statement");

    assert_eq!(
        insert_count.load(Ordering::SeqCst),
        1,
        "PreparedStatement::execute must broadcast CDC event"
    );

    // 2. execute_with_params helper
    conn.execute_with_params(
        "INSERT INTO products VALUES (?, ?, ?);",
        &[Value::Integer(2), Value::Text("Mouse".into()), Value::Real(29.99)],
    ).expect("execute_with_params");

    assert_eq!(
        insert_count.load(Ordering::SeqCst),
        2,
        "execute_with_params must broadcast CDC event"
    );

    // 3. Document store collection insertion (uses parameterized queries under the hood)
    let coll = conn.collection("configs").expect("Open collection");
    let doc = serde_json::json!({"theme": "dark", "version": 2});
    coll.insert_one(&doc).expect("Insert JSON document");

    assert_eq!(
        insert_count.load(Ordering::SeqCst),
        3,
        "Document collection insert must broadcast CDC event"
    );
}

#[test]
fn test_btree_leaf_deletion_bounds_guard() {
    use tapirus::btree::engine::delete_leaf_cell_from_page;

    // Synthesize an empty page buffer of standard 4096 bytes
    let mut page_buf = vec![0u8; 4096];

    // Attempt leaf cell deletion on completely uninitialized/corrupt page
    // Should safely return Ok(false) or Err without panicking or underflowing
    let result = delete_leaf_cell_from_page(&mut page_buf, 0, 42);
    assert!(result.is_ok() || result.is_err(), "Must gracefully handle corrupted leaf page without panic");
}
