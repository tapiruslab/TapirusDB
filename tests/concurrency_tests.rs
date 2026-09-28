//! Concurrency, Multi-Threading, and ACID Transaction Tests for TapirusDB
//!
//! Validates:
//! - Multi-threaded reader and writer execution with `ConnectionPool`
//! - Cloneable connection safety across worker threads
//! - ACID transaction isolation, `ROLLBACK`, and persistent `COMMIT`

use std::thread;
use tapirus::{Connection, ConnectionPool, Value};
use tempfile::NamedTempFile;

#[test]
fn test_concurrent_cloned_connections_and_pool() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("concurrent.tapir");

    let pool = ConnectionPool::open(&db_path).unwrap();
    let setup_conn = pool.acquire();
    setup_conn
        .execute("CREATE TABLE metrics (id INTEGER PRIMARY KEY, worker_id INTEGER, val REAL);")
        .unwrap();

    // Pre-populate rows
    for i in 1..=50 {
        setup_conn
            .execute(&format!("INSERT INTO metrics VALUES ({i}, 0, {}.5);", i * 10))
            .unwrap();
    }

    let mut handles = vec![];

    // Spawn 10 concurrent worker threads accessing cloned handles from the pool
    for worker_id in 1..=10 {
        let conn = pool.acquire();
        let handle = thread::spawn(move || {
            for _ in 0..20 {
                let rows = conn.query("SELECT * FROM metrics WHERE id = 10;").unwrap();
                assert_eq!(rows.len(), 1);
            }
            let my_id = 1000 + worker_id;
            conn.execute(&format!(
                "INSERT INTO metrics VALUES ({my_id}, {worker_id}, 999.0);"
            ))
            .unwrap();
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify all 10 worker inserts completed safely without deadlocks or corruption
    let final_conn = pool.acquire();
    let count_rows = final_conn.query("SELECT COUNT(id) FROM metrics;").unwrap();
    assert_eq!(count_rows[0].get_value("COUNT(id)"), Some(&Value::Integer(60)));
}

#[test]
fn test_acid_transactions_commit_and_rollback() {
    let temp_file = NamedTempFile::new().expect("Temp file");
    let path = temp_file.path().to_path_buf();

    {
        let conn = Connection::open(&path).expect("Open db");
        conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance REAL);")
            .expect("Create accounts");
        conn.execute("INSERT INTO accounts (id, balance) VALUES (1, 1000.0);")
            .expect("Insert account 1");

        // --- Transaction with ROLLBACK ---
        conn.execute("BEGIN;").expect("Begin transaction");
        conn.execute("UPDATE accounts SET balance = 500.0 WHERE id = 1;")
            .expect("Update in txn");
        conn.execute("INSERT INTO accounts (id, balance) VALUES (2, 500.0);")
            .expect("Insert in txn");

        let in_txn_rows = conn.query("SELECT id, balance FROM accounts;").unwrap();
        assert_eq!(in_txn_rows.len(), 2);

        conn.execute("ROLLBACK;").expect("Rollback transaction");

        // Verify state is restored to pre-transaction
        let after_rollback = conn.query("SELECT id, balance FROM accounts;").unwrap();
        assert_eq!(after_rollback.len(), 1);
        assert_eq!(after_rollback[0].get::<i64>("id").unwrap(), 1);
        assert_eq!(after_rollback[0].get::<f64>("balance").unwrap(), 1000.0);

        // --- Transaction with COMMIT ---
        conn.execute("BEGIN;").expect("Begin transaction 2");
        conn.execute("UPDATE accounts SET balance = 750.0 WHERE id = 1;")
            .expect("Update in txn 2");
        conn.execute("INSERT INTO accounts (id, balance) VALUES (3, 250.0);")
            .expect("Insert in txn 2");
        conn.execute("COMMIT;").expect("Commit transaction");
    }

    // Reopen database and verify persistent committed state
    {
        let conn_reopen = Connection::open(&path).expect("Reopen db");
        let rows = conn_reopen
            .query("SELECT id, balance FROM accounts;")
            .expect("Query reopened");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<i64>("id").unwrap(), 1);
        assert_eq!(rows[0].get::<f64>("balance").unwrap(), 750.0);
        assert_eq!(rows[1].get::<i64>("id").unwrap(), 3);
        assert_eq!(rows[1].get::<f64>("balance").unwrap(), 250.0);
    }
}

#[test]
fn test_four_model_atomic_transaction() {
    let temp_file = NamedTempFile::new().expect("Temp file");
    let path = temp_file.path().to_path_buf();

    let conn = Connection::open(&path).expect("Open db");

    // Initialize all 4 models:
    // 1. Relational SQL
    conn.execute("CREATE TABLE sql_state (id INTEGER PRIMARY KEY, status TEXT);").unwrap();
    conn.execute("INSERT INTO sql_state (id, status) VALUES (1, 'initial');").unwrap();

    // 2. Documents (JSON)
    let docs = conn.collection("user_payloads").unwrap();
    docs.insert_one(&serde_json::json!({"doc_id": 1, "data": "original_payload"})).unwrap();

    // 3. Knowledge Graph
    conn.graph_add_node(100, "DeviceA", r#"{"type": "edge_node"}"#).unwrap();
    conn.graph_add_node(200, "ServerB", r#"{"type": "cloud"}"#).unwrap();
    conn.graph_add_edge(100, 200, "ORIGINAL_LINK", 1.0, "{}").unwrap();

    // 4. Vector Embedding
    conn.execute("CREATE TABLE embeddings (id INTEGER PRIMARY KEY, v VECTOR(3));").unwrap();
    conn.execute("INSERT INTO embeddings VALUES (1, [1.0, 0.0, 0.0]);").unwrap();

    // === PHASE 1: ATOMIC ROLLBACK ACROSS ALL 4 MODELS ===
    conn.execute("BEGIN;").unwrap();

    // Mutate 1: SQL
    conn.execute("UPDATE sql_state SET status = 'in_transaction' WHERE id = 1;").unwrap();
    conn.execute("INSERT INTO sql_state (id, status) VALUES (2, 'temporary');").unwrap();

    // Mutate 2: JSON Document
    docs.insert_one(&serde_json::json!({"doc_id": 2, "data": "uncommitted_payload"})).unwrap();

    // Mutate 3: Graph (Node + Edge)
    conn.graph_add_node(300, "TempNode", "{}").unwrap();
    conn.graph_add_edge(100, 300, "TEMP_EDGE", 0.5, "{}").unwrap();

    // Mutate 4: Vector
    conn.execute("INSERT INTO embeddings VALUES (2, [0.0, 1.0, 0.0]);").unwrap();

    // Execute ROLLBACK
    conn.execute("ROLLBACK;").unwrap();

    // Verify Model 1 (SQL): Reverted!
    let sql_rows = conn.query("SELECT id, status FROM sql_state ORDER BY id ASC;").unwrap();
    assert_eq!(sql_rows.len(), 1);
    assert_eq!(sql_rows[0].get::<String>("status").unwrap(), "initial");

    // Verify Model 2 (JSON Documents): Reverted!
    assert_eq!(docs.count().unwrap(), 1, "Only initial document should remain");
    let doc_res = docs.find_by_id(2).unwrap();
    assert!(doc_res.is_none(), "Uncommitted document 2 must be rolled back");

    // Verify Model 3 (Graph): Reverted!
    assert_eq!(conn.graph_nodes().len(), 2, "Graph must only have initial 2 nodes");
    assert_eq!(conn.graph_edges().len(), 1, "Graph must only have initial 1 edge");
    assert!(conn.graph_edges().iter().all(|e| e.label != "TEMP_EDGE"));

    // Verify Model 4 (Vector): Reverted!
    let vec_res = conn.query("SELECT id FROM embeddings VECTOR NEAR v = [0.0, 1.0, 0.0] TOP 2;").unwrap();
    assert_eq!(vec_res.len(), 1);
    assert_eq!(vec_res[0].get_value("id").unwrap(), &Value::Integer(1));

    // === PHASE 2: ATOMIC COMMIT ACROSS ALL 4 MODELS ===
    conn.execute("BEGIN;").unwrap();

    conn.execute("UPDATE sql_state SET status = 'committed_v2' WHERE id = 1;").unwrap();
    docs.insert_with_id(3, &serde_json::json!({"doc_id": 3, "data": "committed_payload"})).unwrap();
    conn.graph_add_node(400, "PermanentNode", "{}").unwrap();
    conn.graph_add_edge(100, 400, "PERMANENT_LINK", 0.9, "{}").unwrap();
    conn.execute("INSERT INTO embeddings VALUES (3, [0.0, 0.0, 1.0]);").unwrap();

    conn.execute("COMMIT;").unwrap();

    // Reopen connection from disk to verify true physical ACID durability on disk
    drop(conn);
    let reopened = Connection::open(&path).expect("Reopen db");

    // Verify Model 1 (SQL) committed
    let sql_rows = reopened.query("SELECT id, status FROM sql_state ORDER BY id ASC;").unwrap();
    assert_eq!(sql_rows[0].get::<String>("status").unwrap(), "committed_v2");

    // Verify Model 2 (Documents) committed
    let docs_reopened = reopened.collection("user_payloads").unwrap();
    let doc3 = docs_reopened.find_by_id(3).unwrap();
    assert!(doc3.is_some(), "Committed document 3 must be persisted");
    assert_eq!(docs_reopened.count().unwrap(), 2);

    // Verify Model 3 (Graph) committed
    assert_eq!(reopened.graph_nodes().len(), 3);
    assert_eq!(reopened.graph_edges().len(), 2);

    // Verify Model 4 (Vector) committed
    let vec_res = reopened.query("SELECT id FROM embeddings VECTOR NEAR v = [0.0, 0.0, 1.0] TOP 2;").unwrap();
    assert!(vec_res.iter().any(|r| r.get_value("id") == Some(&Value::Integer(3))));
}

