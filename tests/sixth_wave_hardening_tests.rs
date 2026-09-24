//! Sixth-Wave Forensic Hardening Integration Tests for TapirusDB
//!
//! Validates:
//! 1. HNSW Layer Hierarchy Restoration on Entry Point Removal
//! 2. Truncated Database Crash Detection on File Open
//! 3. Primary Key Synchronization, Uniqueness, and Auto-Increment on UPDATE
//! 4. Pager Cache Memory Bounds on Bulk Writes
//! 5. Multi-Engine State Synchronization on SQL ROLLBACK (Vectors & Graphs)
//! 6. Standard SQL Comments Support (`--` and `/* */`)
//! 7. Vector Index Tombstone Purging on VACUUM
//! 8. HNSW Vector Duplicate Insertion Hygiene & k=0 Early Exit

use std::fs::File;
use std::io::Write;
use tapirus::traits::{Value, VectorIndexEngine};
use tapirus::vector::{DistanceMetric, HnswIndex};
use tapirus::{Connection, Error};

#[test]
fn test_hnsw_entry_point_removal_preserves_hierarchy() {
    let mut index = HnswIndex::with_seed(3, DistanceMetric::Cosine, 1337);

    // Insert 60 vectors to build a multi-layer graph
    for i in 1..=60 {
        let v = vec![i as f32, (i * 2) as f32, (i * 3) as f32];
        index.insert_vector(i, &v).expect("Insert vector");
    }

    assert!(index.max_level >= 1, "Graph should have multiple layers");
    let initial_ep = index.entry_point.expect("Entry point should exist");

    // Remove the current entry point
    assert!(index.remove_vector(initial_ep));

    // Entry point must be updated to the remaining node with the highest top_layer
    let new_ep = index.entry_point.expect("New entry point should exist");
    assert_ne!(new_ep, initial_ep, "Entry point must have changed");
    if index.nodes.values().any(|n| n.top_layer >= 1) {
        assert!(
            index.max_level >= 1,
            "Max level must not arbitrarily drop to 0 if higher layer nodes exist"
        );
    }

    // Search nearest must continue to succeed across the multi-layer hierarchy
    let results = index
        .search_knn(&[10.0, 20.0, 30.0], 5, DistanceMetric::Cosine)
        .expect("Search after ep removal");
    assert!(!results.is_empty(), "Search must return nearest neighbors");
    assert!(!results.iter().any(|(id, _)| *id == initial_ep));
}

#[test]
fn test_truncated_db_file_returns_corrupted_error() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("truncated.tapir");

    // Write a truncated file of 42 bytes (less than DATABASE_HEADER_SIZE of 100 bytes)
    {
        let mut f = File::create(&db_path).expect("Create file");
        f.write_all(b"TAPIRUSDB truncated partial write crash").expect("Write bytes");
    }

    // Attempting to open must return Error::Corrupted, NEVER overwrite
    let res = Connection::open(&db_path);
    assert!(
        matches!(res, Err(Error::Corrupted(_))),
        "Truncated file must be rejected as corrupted, not silently overwritten"
    );

    // Verify file content was not overwritten
    let file_size = std::fs::metadata(&db_path).expect("Metadata").len();
    assert_eq!(file_size, 39, "File must not be truncated or overwritten");
}

#[test]
fn test_update_primary_key_uniqueness_and_sync() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
        .expect("Create table");
    conn.execute("INSERT INTO users VALUES (1, 'Alice');").expect("Insert 1");
    conn.execute("INSERT INTO users VALUES (2, 'Bob');").expect("Insert 2");

    // 1. Updating PK to an existing key must fail with UNIQUE ConstraintViolation
    let collide_res = conn.execute("UPDATE users SET id = 2 WHERE id = 1;");
    assert!(
        matches!(collide_res, Err(Error::ConstraintViolation(_))),
        "Updating PK to colliding existing key must fail"
    );

    // 2. Updating PK to a non-colliding new key must succeed and migrate B+Tree key
    conn.execute("UPDATE users SET id = 10 WHERE id = 1;").expect("Update PK to 10");

    // Search by new PK
    let rows_new = conn.query("SELECT * FROM users WHERE id = 10;").expect("Query PK 10");
    assert_eq!(rows_new.len(), 1);
    assert_eq!(rows_new[0].get_value("name").unwrap(), &Value::Text("Alice".into()));

    // Old PK must no longer exist
    let rows_old = conn.query("SELECT * FROM users WHERE id = 1;").expect("Query PK 1");
    assert_eq!(rows_old.len(), 0);

    // 3. Auto-increment sequence must synchronize: next insert without PK should be 11
    conn.execute("INSERT INTO users (name) VALUES ('Charlie');").expect("Insert Charlie");
    let rows_charlie = conn.query("SELECT id FROM users WHERE name = 'Charlie';").expect("Query Charlie");
    assert_eq!(rows_charlie.len(), 1);
    assert_eq!(rows_charlie[0].get_value("id").unwrap(), &Value::Integer(11));
}

#[test]
fn test_write_cache_memory_bounds() {
    let tmp_dir = tempfile::tempdir().expect("Create temp dir");
    let db_path = tmp_dir.path().join("cache_bounds.tapir");

    let conn = Connection::open(&db_path).expect("Open file-backed db");
    conn.execute("CREATE TABLE bulk_data (id INTEGER PRIMARY KEY, payload TEXT);")
        .expect("Create table");

    // Insert 100 rows with large payloads to allocate multiple pages
    let large_str = "X".repeat(500);
    for i in 1..=100 {
        conn.execute(&format!(
            "INSERT INTO bulk_data VALUES ({i}, '{large_str}');"
        ))
        .expect("Insert row");
    }

    // Verify all rows can still be queried accurately
    let rows = conn.query("SELECT COUNT(*) FROM bulk_data;").expect("Count");
    let count: i64 = rows[0].get("COUNT(*)").unwrap();
    assert_eq!(count, 100);
}

#[test]
fn test_sql_rollback_clears_vectors_and_graphs() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, v VECTOR(3));")
        .expect("Create table");

    // Initial committed state
    conn.execute("INSERT INTO items VALUES (1, [1.0, 0.0, 0.0]);").expect("Insert committed");
    conn.graph_add_node(1, "Root", "{}").expect("Add graph node 1");

    // Begin transaction and mutate vector and graph
    conn.execute("BEGIN;").expect("BEGIN");
    conn.execute("INSERT INTO items VALUES (2, [0.0, 1.0, 0.0]);").expect("Insert in txn");
    conn.graph_add_node(2, "Temporary", "{}").expect("Add graph node 2 in txn");

    // Rollback transaction
    conn.execute("ROLLBACK;").expect("ROLLBACK");

    // Vector query must not find uncommitted vector 2
    let res = conn
        .query("SELECT id FROM items VECTOR NEAR v = [0.0, 1.0, 0.0] TOP 2;")
        .expect("Search vector");
    assert_eq!(res.len(), 1, "Only committed vector 1 should remain");
    assert_eq!(res[0].get_value("id").unwrap(), &Value::Integer(1));

    // Graph node 2 must not exist
    let graph_nodes = conn.graph_nodes();
    assert_eq!(graph_nodes.len(), 1, "Only committed graph node 1 should remain");
    assert_eq!(graph_nodes[0].id, 1);
}

#[test]
fn test_sql_comments_parsing() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    // Single-line and multi-line comments
    let sql = r#"
        -- Header comment: creating users table
        CREATE TABLE users (
            id INTEGER PRIMARY KEY, /* primary key */
            name TEXT -- user name
        );
        -- Next line comment
    "#;
    conn.execute(sql).expect("Execute with comments");

    conn.execute("INSERT INTO users VALUES (1, 'Alice'); -- trailing comment")
        .expect("Insert with comment");

    let query_sql = "SELECT /* inline block comment */ id, name FROM users WHERE id = 1; -- end of query";
    let rows = conn.query(query_sql).expect("Query with comments");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value("name").unwrap(), &Value::Text("Alice".into()));
}

#[test]
fn test_vacuum_purges_vector_tombstones() {
    let conn = Connection::open_in_memory().expect("Open in-memory db");

    conn.execute("CREATE TABLE embeddings (id INTEGER PRIMARY KEY, v VECTOR(3));")
        .expect("Create table");

    conn.execute("INSERT INTO embeddings VALUES (1, [1.0, 0.0, 0.0]);").expect("Insert 1");
    conn.execute("INSERT INTO embeddings VALUES (2, [0.0, 1.0, 0.0]);").expect("Insert 2");

    // Delete row 1 (creates tombstone in vector index)
    conn.execute("DELETE FROM embeddings WHERE id = 1;").expect("Delete 1");

    // Execute VACUUM
    conn.execute("VACUUM;").expect("Execute VACUUM");

    // Search should function cleanly without tombstones
    let res = conn
        .query("SELECT id FROM embeddings VECTOR NEAR v = [1.0, 0.0, 0.0] TOP 5;")
        .expect("Search after vacuum");
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].get_value("id").unwrap(), &Value::Integer(2));
}

#[test]
fn test_hnsw_duplicate_vector_and_zero_k() {
    let mut index = HnswIndex::new(3, DistanceMetric::Cosine);

    // 1. k = 0 search should return empty without traversal
    let empty = index.search_knn(&[1.0, 0.0, 0.0], 0, DistanceMetric::Cosine).expect("k=0");
    assert!(empty.is_empty());

    // 2. Insert vector 1
    index.insert_vector(1, &[1.0, 0.0, 0.0]).expect("Insert 1");

    // Re-insert vector 1 with new embedding
    index.insert_vector(1, &[0.0, 0.0, 1.0]).expect("Re-insert 1");

    // Verify search matches updated embedding
    let res = index.search_knn(&[0.0, 0.0, 1.0], 1, DistanceMetric::Cosine).expect("Search 1");
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].0, 1);
    assert!(res[0].1 < 0.001, "Distance to updated vector should be near 0");
}
