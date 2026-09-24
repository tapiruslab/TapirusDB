//! Tenth-Wave Real Engineering Capabilities Integration Tests
//!
//! Validates the full implementation of previously missing capabilities:
//! 1. Full SQL JOIN engine: INNER JOIN, LEFT OUTER JOIN, RIGHT OUTER JOIN, FULL OUTER JOIN.
//! 2. True SQL:2011 temporal Time-Travel queries (`SELECT ... AS OF TIMESTAMP <ts>`).
//! 3. Bidirectional Remote Storage Adapter (`open_remote_writable` and `push_remote`).
//! 4. Pluggable HTTP Neural Embeddings with seamless deterministic fallback.

use std::sync::Arc;
use tapirus::memory::embedder::{EmbeddingEngine, HttpEmbeddingEngine};
use tapirus::pager::remote::MockRemoteRangeStorage;
use tapirus::traits::Value;
use tapirus::Connection;

#[test]
fn test_sql_joins_inner_left_right_full() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute("CREATE TABLE depts (dept_id INTEGER PRIMARY KEY, dept_name TEXT)")
        .expect("Create depts");
    conn.execute("CREATE TABLE emps (emp_id INTEGER PRIMARY KEY, emp_name TEXT, d_id INTEGER)")
        .expect("Create emps");

    conn.execute("INSERT INTO depts (dept_id, dept_name) VALUES (1, 'Engineering')").unwrap();
    conn.execute("INSERT INTO depts (dept_id, dept_name) VALUES (2, 'Marketing')").unwrap();
    conn.execute("INSERT INTO depts (dept_id, dept_name) VALUES (3, 'Research')").unwrap();

    conn.execute("INSERT INTO emps (emp_id, emp_name, d_id) VALUES (101, 'Alice', 1)").unwrap();
    conn.execute("INSERT INTO emps (emp_id, emp_name, d_id) VALUES (102, 'Bob', 1)").unwrap();
    conn.execute("INSERT INTO emps (emp_id, emp_name, d_id) VALUES (103, 'Charlie', 2)").unwrap();
    conn.execute("INSERT INTO emps (emp_id, emp_name, d_id) VALUES (104, 'Dave', 999)").unwrap();

    // 1. INNER JOIN (Alice-Eng, Bob-Eng, Charlie-Mkt = 3 rows)
    let inner_rows = conn
        .query("SELECT emps.emp_name, depts.dept_name FROM emps INNER JOIN depts ON emps.d_id = depts.dept_id")
        .expect("INNER JOIN");
    assert_eq!(inner_rows.len(), 3);

    // 2. LEFT OUTER JOIN (Alice-Eng, Bob-Eng, Charlie-Mkt, Dave-NULL = 4 rows)
    let left_rows = conn
        .query("SELECT emps.emp_name, depts.dept_name FROM emps LEFT JOIN depts ON emps.d_id = depts.dept_id")
        .expect("LEFT JOIN");
    assert_eq!(left_rows.len(), 4);
    let dave_row = left_rows
        .iter()
        .find(|r| r.get_value("emps.emp_name") == Some(&Value::Text("Dave".into())))
        .expect("Dave found");
    assert_eq!(dave_row.get_value("depts.dept_name"), Some(&Value::Null));

    // 3. RIGHT OUTER JOIN (Alice-Eng, Bob-Eng, Charlie-Mkt, NULL-Research = 4 rows)
    let right_rows = conn
        .query("SELECT emps.emp_name, depts.dept_name FROM emps RIGHT JOIN depts ON emps.d_id = depts.dept_id")
        .expect("RIGHT JOIN");
    assert_eq!(right_rows.len(), 4);
    let research_row = right_rows
        .iter()
        .find(|r| r.get_value("depts.dept_name") == Some(&Value::Text("Research".into())))
        .expect("Research found");
    assert_eq!(research_row.get_value("emps.emp_name"), Some(&Value::Null));

    // 4. FULL OUTER JOIN (Alice-Eng, Bob-Eng, Charlie-Mkt, Dave-NULL, NULL-Research = 5 rows)
    let full_rows = conn
        .query("SELECT emps.emp_name, depts.dept_name FROM emps FULL OUTER JOIN depts ON emps.d_id = depts.dept_id")
        .expect("FULL OUTER JOIN");
    assert_eq!(full_rows.len(), 5);

    let has_dave_null = full_rows.iter().any(|r| {
        r.get_value("emps.emp_name") == Some(&Value::Text("Dave".into()))
            && r.get_value("depts.dept_name") == Some(&Value::Null)
    });
    let has_null_research = full_rows.iter().any(|r| {
        r.get_value("emps.emp_name") == Some(&Value::Null)
            && r.get_value("depts.dept_name") == Some(&Value::Text("Research".into()))
    });
    assert!(has_dave_null, "FULL JOIN must contain left unmatched row");
    assert!(has_null_research, "FULL JOIN must contain right unmatched row");
}

#[test]
fn test_point_in_time_time_travel_queries() {
    let conn = Connection::open_in_memory().expect("Open in-memory DB");

    conn.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, holder TEXT, balance INTEGER)")
        .expect("Create accounts");

    // Insert record at T1
    conn.execute("INSERT INTO accounts (id, holder, balance) VALUES (1, 'Siti', 1000)")
        .expect("Insert T1");
    let t1 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Sleep briefly to ensure distinct epoch second
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Update record at T2
    conn.execute("UPDATE accounts SET balance = 2500 WHERE id = 1")
        .expect("Update T2");
    let t2 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Sleep briefly
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Delete record at T3
    conn.execute("DELETE FROM accounts WHERE id = 1")
        .expect("Delete T3");
    let t3 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 1. Current state: row is deleted
    let live_rows = conn.query("SELECT id, balance FROM accounts").expect("Live query");
    assert_eq!(live_rows.len(), 0);

    // 2. Query AS OF TIMESTAMP T1: balance was 1000
    let rows_t1 = conn
        .query(&format!("SELECT id, balance FROM accounts AS OF TIMESTAMP {t1}"))
        .expect("Time travel T1");
    assert_eq!(rows_t1.len(), 1);
    assert_eq!(rows_t1[0].get_idx::<i64>(1).unwrap(), 1000);

    // 3. Query AS OF TIMESTAMP T2: balance was 2500
    let rows_t2 = conn
        .query(&format!("SELECT id, balance FROM accounts AS OF TIMESTAMP {t2}"))
        .expect("Time travel T2");
    assert_eq!(rows_t2.len(), 1);
    assert_eq!(rows_t2[0].get_idx::<i64>(1).unwrap(), 2500);

    // 4. Query AS OF TIMESTAMP T3 (or later): record is deleted
    let rows_t3 = conn
        .query(&format!("SELECT id, balance FROM accounts AS OF TIMESTAMP {t3}"))
        .expect("Time travel T3");
    assert_eq!(rows_t3.len(), 0);
}

#[test]
fn test_bidirectional_remote_cloud_storage_sync() {
    let mock_cloud = Arc::new(MockRemoteRangeStorage::empty());

    // 1. Open writable remote connection
    let writable_conn = Connection::open_remote_writable(mock_cloud.clone())
        .expect("Open remote writable");

    writable_conn
        .execute("CREATE TABLE cloud_ledger (entry_id INTEGER PRIMARY KEY, note TEXT)")
        .expect("Create ledger");

    writable_conn
        .execute("INSERT INTO cloud_ledger (entry_id, note) VALUES (1, 'reconciled_batch_99')")
        .expect("Insert remote");

    // 2. Push changes up to the remote cloud store
    writable_conn.push_remote().expect("Push remote sync");

    // 3. Verify cloud storage now has data
    assert!(mock_cloud.bytes_len() > 0, "Remote storage should contain pushed bytes");

    // 4. Open independent read-only connection against the same remote store
    let ro_conn = Connection::open_remote(mock_cloud).expect("Open remote read-only");
    let rows = ro_conn
        .query("SELECT entry_id, note FROM cloud_ledger WHERE entry_id = 1")
        .expect("Query from remote cloud");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String>("note").unwrap(), "reconciled_batch_99");
}

#[test]
fn test_http_embedding_engine_with_fallback() {
    // Instantiate HTTP engine pointing to mock/unreachable endpoint
    let engine = HttpEmbeddingEngine::new("http://127.0.0.1:11434/api/embeddings", "nomic-embed-text", 64);
    assert_eq!(engine.dimensions(), 64);

    // Should fall back seamlessly without panicking
    let vec1 = engine.embed_text("Hukum maqasid syariah dalam kewangan");
    let vec2 = engine.embed_text("Hukum maqasid syariah dalam kewangan");
    let vec3 = engine.embed_text("Prinsip integriti perisian pangkalan data");

    assert_eq!(vec1.len(), 64);
    assert_eq!(vec1, vec2, "Embeddings must be consistent and deterministic");
    assert_ne!(vec1, vec3, "Different text must produce different embeddings");

    // Must be L2 unit normalized
    let norm: f32 = vec1.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "Fallback embedding must be L2 normalized");
}
