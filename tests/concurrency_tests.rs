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
